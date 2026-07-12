# Effort P3 — async + periodic + parameterized commands + dynamic menu: grounding (facts + open forks)

**Status:** GROUNDING (2026-07-12, HEAD 8a06ab8 post-P2). Facts-only code-surface map + the design
forks that must be resolved (by the human) BEFORE a spec can be authored. Sibling of
`effort-p-grounding.md` / `effort-p2-grounding.md`. Anchors are symbol NAMES.

---

## 1. Goal (as decomposed) — but reshaped by the findings below

P3 was decomposed as: plugin **async** (host-runs-the-slow-part off the hot path) + **periodic**
timers + **parameterized** commands + a plugin-contributed **dynamic menu section**. The grounding
reveals the async piece is fundamentally constrained by Send/!Send (§Forks), so the scope is a
human decision, not a given.

## 2. Code-surface map (verified 2026-07-12)

**Job substrate — `jobs.rs`.** `Job{ buffer_id, class: ResultClass{BufferLocal|Durability},
version, kind: JobKind{Save,SwapWrite,Reparse,#[cfg(test)]CoalesceProbe}, run: Box<dyn FnOnce()->
JobResult + Send> }`; `JobResult{ .., merge: Box<dyn FnOnce(&mut Editor) + Send> }`;
`trait Executor{dispatch,drain}`; `InlineExecutor` (test, synchronous), `ThreadExecutor::new(wake:
Sender<()>)` (one FIFO worker thread `wcartel-jobs`). `is_stale(r,editor)` — Durability never stale;
real kinds do their own version check inside `merge` (see `save.rs::do_save_to`). **`Job::run` is the
Send boundary** — captures must be `Send` (owned rope snapshot, PathBuf), NEVER `Rc<RefCell<Editor>>`
or `mlua::Lua`/`mlua::Function` (both `!Send`).

**Merge glue — `jobs_apply.rs`.** `apply_result(r, &mut Editor)` → `is_stale` then `(r.merge)(editor)`
+ Save post-processing; `apply_job_outcome(outcome, editor, ex, clock, msg_tx)`. Called from the run
loop's reduce-tail `for o in ex.drain() {...}` — runs after EVERY Msg, on the main thread, no borrow.

**Wake path — `app.rs::run()`.** `ThreadExecutor::new(wake_tx)`; a relay thread does
`while wake_rx.recv() { msg_tx.send(Msg::Tick) }`; the loop blocks on `msg_rx.recv_timeout(timeout)`
where `timeout = timers::next_wake(...) - now` or **3600s** idle fallback. Every async source
(jobs via Tick, `filter.rs` via `Msg::FilterDone`, harper via `Msg::DiagnosticsDone`, input,
clipboard) wakes the loop the SAME way: send on a `Sender<Msg>` clone. `plugin_host.pump(&editor,
&reg, &executor, &clock, &msg_tx)` runs once/iteration after reduce, no borrow held. **NOTE:
`Msg::JobDone(JobOutcome)` exists but nothing sends it — dead/unwired seam (confirm remove vs. finish
for lower-latency plugin async).**

**Timers — `timers.rs`.** `TimedSubsystem{ name: &'static str, deadline: fn(&Editor,u64)->Option<u64> }`;
`static SUBSYSTEMS: &'static [TimedSubsystem]` (8 builtins, bare fn ptrs); `next_wake(editor,now) =
SUBSYSTEMS.iter().filter_map(deadline).min()`. **Module doc says the slice "upgrades to a Vec when
Effort P needs dynamic (plugin) timer registration" — a forward-declared, NOT-built stub.** No
registration API. Also `on_tick`'s per-subsystem dispatch is HAND-WRITTEN (not table-driven off
SUBSYSTEMS), so a plugin timer needs a second generalization pass, not just "make it a Vec". Every
builtin `deadline` is EDGE-TRIGGERED off real Editor state — never a bare wall-clock re-arm; guardrail
tests `next_wake_none_when_settled` / `gated_subsystems_yield_none` pin idle→None→3600s block.

**Parameterized commands — `registry.rs`.** `CommandId(pub &'static str)` (Copy); `Handler = fn(&mut
Ctx)->CommandResult` (NULLARY — no arg); `HandlerKind{Builtin(Handler)|Plugin}`; `dispatch(id,ctx)`.
`plugin::PluginCall{id}` (Copy), `PluginDispatch{origin,name}` — NO arg field. Arg-collection
precedent = `minibuffer.rs` `MinibufferKind{Filter,GotoLine,SaveAs,WriteBlock,WrapColumn}` — a CLOSED
per-kind submit table, not a generic path. Threading an arg needs a real widening (new Handler shape
or additive `HandlerKind` arm; a `String` arg breaks `PluginCall`'s `Copy`) touching registry/dispatch/
pump/api/minibuffer.

**Menu + dynamic sections — `menu.rs` / `registry.rs`.** `MenuCategory{File,Edit,Block,Format,View,
Documents,Settings,Export}`; menu REBUILT from registry per-open via `build(reg,keymap,editor)` →
`grouped_commands`. A dynamic-section seam ALREADY EXISTS: `DynamicSection{ category, rows: fn(&Editor)
->Vec<(String,MenuRowAction)> }`, `static DYNAMIC_SECTIONS` (one entry: Documents →
`workspace::documents_menu_rows`); `MenuRowAction{Command(CommandId)|SwitchBuffer(BufferId)}`
(exhaustive-by-design). For a PLUGIN dynamic section: `DYNAMIC_SECTIONS` must stop being a static of
bare fn ptrs (can't close over a Lua callback); `MenuRowAction` needs a 3rd `Plugin(...)` variant;
and row-generation would invoke Lua AT MENU-BUILD TIME — a NEW plugin-invocation site beyond pump's
dispatch/call/event triad.

**Pump/host post-P2 — `plugin/{pump,host,mod}.rs`.** `pump(&mut self, &Rc<RefCell<Editor>>, &Registry,
&dyn Executor, &dyn Clock, &Sender<Msg>)` re-drain loop over dispatch/calls/events, capped
`PLUGIN_PUMP_CHAIN_CAP` + `PUMP_CYCLE_TIME_BUDGET(500ms)`. `Bridge{editor,msg_tx,clock,invoke_state}`;
`PluginHost{lua:Option<Lua>,bridge,hooks}` — the `!Send` confinement. `attach_bridge(editor,msg_tx,
clock)`. `with_time_guard(lua,budget,f)`. Caps in `limits.rs` (PLUGIN_MAX_* — no async/timer caps yet).

**Reference async model — `harper_ls.rs`.** `DiagnosticsProvider` with a PERSISTENT lazily-spawned
worker thread (`ensure_running`), request channel in + `msg_tx`(Msg) out, `FlushGuard` RAII
guaranteeing a terminal completion Msg even on panic. Best structural fit for a long-lived/periodic
plugin async task (vs. the one-shot `Job` substrate; `filter.rs` is a third ad-hoc idiom).

## 3. THE FORKS (human must decide before spec) — see the pause note in the ledger

- **F1 — What IS "plugin async", given `Job::run: Send` vs. `!Send` Lua?** A plugin can't run a Lua
  body off-thread. Options: (A) a CLOSED menu of Rust primitives via `wc.async{op,args,on_done}`
  (shell-out / file-read / sleep-until / http?), Lua completion on the main thread; (B) main-thread
  DEFERRED/scheduled callbacks only (no real offload — just "run this Lua later/periodically");
  (C) DEFER async+timers entirely (decomposition said "ships when a real plugin needs it"), ship only
  the cleanly-buildable P3 pieces now.
- **F2 — Periodic timers vs. the idle-free law.** True `wc.timer(interval, fn)` is NOT edge-triggered.
  (A) allow true periodic (an ARMED plugin timer legitimately keeps the loop off the 3600s idle
  fallback — "armed ≠ idle", a stated exception); (B) only "fire once after the NEXT real edit"
  (strict idle-free preserved); (C) minimum-interval floor to prevent a de-facto spin.
- **F3 — P3 scope.** Given F1's constraint, do parameterized commands + dynamic menu section (both
  cleanly buildable + clearly useful) become the core of P3, with async/timers deferred or narrowed?

## 4. Buildable-cleanly (regardless of F1/F2): parameterized commands + dynamic menu section
Both are real widenings but well-anchored (registry Handler/HandlerKind + MinibufferKind; DynamicSection
+ MenuRowAction 3rd variant + a menu-build-time Lua invocation site). No Send/!Send obstruction.

---

## 5. BINDING CONSTRAINTS — LOCKED SCOPE (human-decided 2026-07-12, after Codex + Fable red-team)

**P3 = "the pomodoro slice" — scheduled & interactive plugins.** The forks F1/F2/F3 above are
RESOLVED. Rationale: both reviewers dismantled the edge-triggered-only "timer" (F2-A is dominated —
same plumbing cost as periodic, leaky semantics, redundant with events+async). The remaining tension
(ship periodic now vs. defer until a real driver) is dissolved by **building the driver in-effort**:
a bare-bones `pomodoro.lua` demo IS the concrete plugin that de-speculates the timer and validates
its shape — the same pattern as `insert_date.lua` (P1 commands) and `wordcount.lua` (P2 hooks).

**IN SCOPE (build in P3):**

1. **`wc.timer` — plugin timers, guarded (Fable's safe shape + Codex's caps).**
   - **One-shot by DEFAULT**: `wc.timer(interval_ms, fn)` fires once then disarms (edge-armed,
     self-clearing — stays inside the existing idle-free law).
   - **Periodic is an EXPLICIT opt-in**: `wc.timer{ interval_ms, repeat=true, fn }`. This is the
     genuinely-new wall-clock behavior — framed HONESTLY as a narrow, user-consented exception to the
     idle-free law (NOT "armed ≠ idle already fits" — both reviewers rejected that as a rationalization;
     builtins self-DISARM and re-quiesce to None, a repeating timer self-perpetuates. It's a real,
     scoped exception the user opts into by installing+arming a plugin).
   - **`next_wake` stays None at REST, provably**: zero armed plugin timers ⇒ the fold is byte-identical
     to today; existing guardrail tests (`next_wake_none_when_settled`, `gated_subsystems_yield_none`)
     pass unchanged. NEW guardrail tests required: no-armed-timer ⇒ next_wake == builtin-only fold;
     a plugin with only commands/hooks ⇒ next_wake == None; an armed repeating timer ⇒ Some(..);
     cancel/reload ⇒ back to None; a floored repeating timer never drives recv_timeout(0) (anti-spin).
   - **Guardrails (merge of both reviewers):** min-interval floor **1000 ms** (sub-floor → typed Lua
     error at call, P2 borrowed-value-check pattern); **max 8 armed timers/plugin** (over-cap → typed
     error); **auto-disarm on plugins_reload / load-failure / null-host / shutdown** (store armed timers
     on the host like `hooks: Vec<HookEntry>` — whole-VM teardown drops them free, same invariant proven
     for hooks/queues in the P2 gate); fires through the EXISTING pump (`PLUGIN_PUMP_CHAIN_CAP=64`,
     `PUMP_CYCLE_TIME_BUDGET=500ms`, per-callback `CALLBACK_TIME_BUDGET=150ms`); **at most one pending
     callback per timer** (no backlog while a prior is in flight); repeating **reschedules from
     completion, not accumulate missed ticks**; **cancellable + visible** — `wc.timer.cancel(handle)`
     and armed timers surfaced in `plugin_list`.
   - **Requires the timers.rs work (grounding §2):** `static SUBSYSTEMS: &'static [TimedSubsystem]`
     (fn-ptr rows) → a form holding plugin-registered rows that close over the timer's next-due state
     (a `Vec` on Editor/host — a bare fn ptr can't hold an `mlua::Function`/state), AND generalize
     `on_tick`'s hand-written per-subsystem dispatch to fire plugin-timer rows. Builtins stay fn-ptr rows.

2. **Parameterized commands** — a command that takes an argument (pomodoro: "start / set duration N").
   The widening (§4): registry `Handler`/`HandlerKind` (new arg-carrying handler shape or additive arm),
   `PluginCall`/`PluginDispatch` (an arg field — breaks their `Copy`), a generic `MinibufferKind`
   arg-collection variant, and `wc.command`/`wc.register_command` Lua signatures. The arg is capped
   (resource-bound LAW — new `PLUGIN_MAX_*`).

3. **`on_change` event** — a P2-events-family member, replacing Codex's `after_idle` primitive so there's
   ONE timer idiom, not two. **DESIGN CONSTRAINT (flag): it must NOT be a per-keystroke hot-path hook**
   (the P2 decomposition explicitly forbids on_key/on_edit). It fires **debounced / after edits settle**
   (edge-armed like the builtin reconcile/diagnostics deadlines — "content settled after a change"),
   observer-only. Fable designs the coalescing so typing stays instant.

4. **`pomodoro.lua` demo + clock-driven e2e test** — the driver. Bare-bones: arms a timer (25-min real
   default), a start/cancel command (parameterized for duration), a callback that notifies on fire. The
   e2e test **advances the `Clock`** to fire it deterministically (no wall-clock sleep — the existing
   timer/job tests already drive time this way). Validates: fire-DURING-inactivity, the callback tier,
   auto-disarm on reload, cancel, next_wake==None at rest.

**OPEN DESIGN POINTS for Fable to resolve in the spec (flag any genuine product/safety fork to human):**
- **Timer callback TIER.** Bare-bones pomodoro (a status notification) is served by observer-tier +
  `wc.status` (already observer-allowed). A richer timer (open a dismissable modal, dispatch a command,
  edit) needs COMMAND-tier. Pick the minimal tier the pomodoro demo actually needs; if command-tier,
  ensure a timer callback still cannot arm timers past the per-plugin cap (no timer-spawns-timers spin).
- **`on_change` coalescing mechanism** — how it edge-arms/debounces without a hot-path hook (piggyback
  the reconcile-settle path? its own deadline row?).
- **timers.rs upgrade shape** — Vec-on-Editor vs. Vec-on-host for plugin timer rows; how `on_tick`
  dispatches them.

**DEFERRED to their own future efforts (NOT P3), each with its own demo driver:**
- **subprocess `wc.async`** (closed Rust primitive; driver = a formatter/linter plugin). Separate effort.
- **plugin-contributed dynamic menu section** (`DYNAMIC_SECTIONS` → dynamic + `MenuRowAction` 3rd
  variant + Lua-at-menu-build). Separate effort.

**Process/branch:** same pipeline as P2 — Fable authors spec (Codex-gated to clean) + plan (Codex-gated),
subagent-driven TDD execution, both final gates, --no-ff merge, auto-push. Autonomous; same 3 stop
conditions. Branch `effort-p3-plugin-timers`.
