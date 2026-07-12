# Effort P3 — implementation plan: plugin timers + parameterized commands + on_change (the pomodoro slice)

**Spec:** `docs/superpowers/specs/2026-07-12-effort-p3-plugin-timers-design.md` (Codex-clean, 2026-07-12).
**Branch:** `effort-p3-plugin-timers` (already cut).
**Shape:** **No spike** — P1/P2 proved the `mlua` params (`!Send` capture, `set_hook`, named-registry
callbacks, borrowed-length). P3 is eight integration tasks built so the tree stays green after each,
ordered with **zero forward dependencies** (a task depends only on earlier tasks). Subagent-driven, TDD
per task (failing test → impl → green → commit), a per-task reviewer (spec-compliance + quality), then one
Fable whole-branch gate + one Codex pre-merge gate.

Anchor on symbol NAMES (lines drift). `cargo` + `grep` are ground truth, never an editor "unused"/
"undefined" hint (subagent edits are the most stale in an analyzer's view — verify with `cargo build`/
`test` before treating a diagnostic as real).

**Reconciliation with spec §13 (spec is authoritative).** This plan's ordering IS the spec's §13 task
sketch, unchanged: (1) scaffolding, (2) timers.rs rows + idle-free guardrails, (3) `wc.timer` + pump fire
+ `clear_plugin_wake_state`, (4) `on_change`, (5) parameterized commands, (6) `plugin_list` timer count,
(7) `pomodoro.lua` + e2e, (8) gates. Forward-dep-free (§ per-task dependency notes). One clarification: the
shared `Editor::clear_plugin_wake_state` (Critical-2 teardown) is **CREATED in Task 1** (with the editor
fields it resets — it clears the on_change fields too, which exist from Task 1, are unwired until Task 4,
and clearing a `None`/`false` field is harmless) and merely **WIRED into the teardown paths in Task 3**
(`perform_reload` + the null-host pump branch). Task 4 only wires the SETTING side (`has_on_change_subscriber`
recompute + `on_change_due` arm).

---

## Global constraints (bind EVERY task — copy into each implementer/reviewer dispatch)

1. **Binding constraints (spec §2 — settled law, NOT open to re-litigate):**
   - **Timer callbacks are OBSERVER-TIER.** A `wc.timer` callback runs under `ObserverGuard(observer=true)`
     — it may READ (`wc.text`/`selection`/`cursor`/`len`/`version`/`path`) + emit `wc.status`; it may
     **NOT** edit (`wc.insert`/`replace`/`set_selection`), dispatch (`wc.command`), or arm/cancel timers
     (`wc.timer`/`wc.timer_cancel`). This is the no-autonomous-background-mutation (no-data-loss) AND
     no-timer-spawns-timers guarantee, enforced by the SAME `InvokeState.observer` gate as P2 hooks.
   - **One-shot by DEFAULT; periodic is an EXPLICIT opt-in.** `wc.timer(ms, fn)` fires once + disarms;
     `wc.timer{ interval_ms, repeat=true, fn }` reschedules. Periodic is framed HONESTLY as a narrow,
     user-CONSENTED exception to the idle-free law (NOT "armed ≠ idle already fits" — both reviewers
     rejected that; builtins self-disarm, a repeating timer self-perpetuates).
   - **Timer guardrails:** min-interval floor **1000 ms** (sub-floor → typed Lua error at call); **max 8
     armed timers per plugin** (over-cap → typed error at call); **auto-disarm on reload / load-failure /
     null-host / bridge-attach-failure / shutdown** via `Editor::clear_plugin_wake_state` (clears the timer
     schedule AND the on_change subscription — Critical 2); fires through the EXISTING pump
     (`PLUGIN_PUMP_CHAIN_CAP=64`, `PUMP_CYCLE_TIME_BUDGET=500ms`, per-callback `CALLBACK_TIME_BUDGET=150ms`);
     **at most one pending callback per timer** (`pending` flag); repeating **reschedules from completion**,
     not accumulate missed ticks; **mark-then-fire ATOMICALLY per timer** (Critical 1 — `pending` is set
     only immediately before invoking THAT timer, so a cap-trip never strands a due timer).
   - **`on_change` is DEBOUNCED, NOT a per-keystroke hot-path hook.** It fires ~150 ms after edits settle
     (piggybacks the reconcile debounce, `RECONCILE_DEBOUNCE_MS`), observer-only, gated on a subscriber so
     it costs ZERO wake when unused. Typing stays instant — the hot path is untouched (on_change fires from
     the cold `Tick` path).
   - **The idle-free law (the crux).** Background work is edge-triggered, never wall-clock-polled; **idle is
     free**. With zero armed timers and no on_change subscriber, `next_wake`'s RESULT is identical to a
     no-plugins build; the periodic timer is the one consented, guarded exception (a due deadline yields at
     most ONE `recv_timeout(0)` wake via `saturating_sub`, then fires + reschedules ≥ 1 s out — a bounded
     cadence, NOT a spin).
2. **The two design LAWS are GATES.**
   - **(a) Input-validation LAW.** P3 adds NO offset/range API — `plugin_check_range` stays the sole
     chokepoint (inherited unchanged). The observer check added in front of the edit APIs for timer
     callbacks is strictly additive (rejects more).
   - **(b) Resource-bound LAW.** Every plugin-supplied string crossing into a leak/allocation is bounded
     BEFORE the allocation (borrowed-length-check-then-convert). New P3 inputs: `wc.timer` interval (integer
     — floor is the bound, no alloc), timer count (`PLUGIN_MAX_TIMERS_PER_PLUGIN`), parameterized-command
     arg (`PLUGIN_MAX_COMMAND_ARG` on borrowed bytes, at BOTH `wc.command` and the minibuffer submit),
     `wc.register_command{arg=…}` prompt (`PLUGIN_MAX_LABEL_LEN`, interned on commit).
3. **`#![forbid(unsafe_code)]` holds.** `Rc<RefCell<>>` + owned captures are safe; `mlua`'s `unsafe` stays
   in the dependency. `wordcartel-core` stays VM-free (no `mlua` import there — ever).
4. **`registry.rs` stays Lua-free** — no `mlua` import. The `CommandMeta.arg` field + the `dispatch`
   `Plugin` arm branch (open a minibuffer) + `plugin_list` are pure Rust (editor-state mutation, no `mlua`).
5. **Module-budget / anti-regrowth (GATE).** Figures are **PRODUCTION-line counts** per
   `wordcartel/tests/module_budgets.rs` (lines before the last `mod tests`; NOT raw file length):
   `app.rs` ≤ **1000** (899 now), `timers.rs` ≤ **400** (~338 now), `plugin/pump.rs` ≤ **350** (267 total,
   mostly production), `plugin/host.rs` ≤ **400**. The timer wiring must be a SEAM, not bulk: `timers.rs`
   grows by ROWS (two `fn` rows + two `SUBSYSTEMS` entries), `on_tick`/`next_wake` bodies unchanged (no
   Vec, no dispatch generalization); firing lives in the pump (a `fire_due_timers` helper), NOT in
   `on_tick` or `reduce`; `app.rs` gains only ~3 lines in `advance`. `clippy::too_many_lines` (threshold
   100) binds every new fn.
6. **Command-surface-contract conformance (GATE).** See the dedicated section below; each relevant task
   restates how it conforms.
7. **House style** (CLAUDE.md): dense hand-formatting, `—` em-dashes never `--`, no emoji, doc-comment
   every public item, snake_case/PascalCase/SCREAMING_SNAKE. **Do NOT run `cargo fmt`.** Match neighbors.
8. **Errors → status line, never console.** `print_*`/`dbg!` are deny-lints. Plugin errors route through
   `plugin::plugin_error` → `editor.status`.
9. **GATES before merge:** `cargo test` green (both crates); `cargo build` + `cargo test --no-run`
   warning-free for touched crates; `cargo clippy --workspace --all-targets` clean. Run
   `scripts/smoke/run.sh` in the pre-merge report (mandatory-run / advisory-pass). `cargo deny check` at
   release-checklist time (not a merge gate).
10. **Commit per task** with project trailers (Co-Authored-By: Claude Opus 4.8 + Claude-Session).
    Message form `feat(p3): …` / `test(p3):` / `chore(p3):` / `refactor(p3):` as fits.
11. **No ⚠OPEN flags** — the three spec §5 open points are resolved (observer-tier timers; editor-side
    `Vec<PluginTimer>` + two static rows + pump-side firing; debounced on_change). Do NOT re-open them.

---

## Command-surface-contract conformance (the plan honors it, task by task)

- **Law 1 (registry = single source of truth).** Parameterized plugin commands are ordinary `Registry`
  entries (one entry, `CommandMeta.arg = Some(prompt)`); arg collection routes through the shared
  `Minibuffer` seam (the same seam `SaveAs`/`Filter` use), never a parallel store (Task 5). `wc.command(
  name, arg)` routes through `reg.resolve_name` + `reg.dispatch`. Timers/`on_change` are host↔plugin data
  flow, not command-surface actors (like P2 events/config).
- **Law 2 (every user-settable option is a command).** P3 adds no `SettingsSnapshot` option; `wc.timer`/
  `wc.timer_cancel` are Lua APIs, not registry commands — N/A. Recurrence-guard test unaffected.
- **Law 3 (palette exhaustive).** A parameterized plugin command appears in the palette by derivation;
  selecting it opens its arg minibuffer then dispatches — no palette code change (Task 5). Palette-
  completeness test still holds over the registry.
- **Law 4 (menu ⊆ palette).** A parameterized command tagged with a `MenuCategory` appears in that menu;
  the row activates it (opening the arg prompt) exactly as the palette does. Subset holds by derivation.
- **Rule 10 (parameterized set-value commands = the Effort-P concern).** Task 5 realizes rule 10's
  argument-carrying command shape for PLUGIN commands, keeping set-value semantics clean (builtin
  conversion deferred).
- **`plugin_list` shows armed timers.** Task 6 extends the existing `plugin_list` builtin to READ
  `editor.pending_plugin_timers` — a display change to an existing command (laws 1/3/4 unchanged),
  satisfying §5's "armed timers visible" consent-symmetry.
- **No contract amendment required.**

---

## Task 1 — scaffolding: limits, editor fields, `PluginTimer`/`PluginDispatch`-agnostic types, `PluginEventKind::Change`

Inert foundation every later task builds on. Nothing fires/reads it yet; the tree stays green and the app
behaves identically.

**Model:** standard. **Files:** `wordcartel/src/limits.rs`, `wordcartel/src/editor.rs`,
`wordcartel/src/plugin/mod.rs`.

**TDD first:**
- `limits::tests::plugin_caps_are_sane` (extend the existing test) — the three new constants exist with
  the spec values.
- `editor::tests::new_editor_has_no_plugin_wake_state` (new, small) — a fresh `Editor` has
  `pending_plugin_timers` empty, `next_timer_handle == 0`, `on_change_due == None`,
  `has_on_change_subscriber == false`.
- `plugin::tests::event_from_str_parses_change` — `event_from_str("change") == Some(PluginEventKind::Change)`;
  `kind_str(Change) == "change"`.

**Implementation:**
- `limits.rs` — append (grep the `PLUGIN_MAX_*` block; add after `PLUGIN_PUMP_CHAIN_CAP`):
  ```rust
  /// P3 plugin-timer + parameterized-command caps.
  /// Min timer interval — the spin defense: a repeating timer reschedules to `now + interval >= now +
  /// 1000ms` from completion, so it wakes at most ~once/interval (a due deadline may yield ONE immediate
  /// zero-timeout wake, then fires and moves 1s+ out — bounded cadence, not a spin). Sub-floor → typed error.
  pub const PLUGIN_TIMER_MIN_INTERVAL_MS: u64 = 1000;
  /// Max armed timers per plugin (heavier than a hook — each keeps a wall-clock wake alive). Over → typed error.
  pub const PLUGIN_MAX_TIMERS_PER_PLUGIN: usize = 8;
  /// Max bytes of a parameterized-command argument (wc.command arg / the PluginArg minibuffer line),
  /// checked before the owning String allocation (resource-bound LAW).
  pub const PLUGIN_MAX_COMMAND_ARG: usize = 4096;
  ```
  Extend `plugin_caps_are_sane` with `assert_eq!` over the three.
- `plugin/mod.rs` — add the `Change` variant (exhaustive enum — grep `enum PluginEventKind`), the
  `event_from_str`/`kind_str` arms, and `PluginTimer`:
  ```rust
  // in enum PluginEventKind { Save, Open, BufferClose, Change }
  // in event_from_str: "change" => Some(PluginEventKind::Change),
  // in kind_str:        PluginEventKind::Change => "change",

  /// One armed plugin timer (P3 §3). The callback lives in the VM named registry under `key` (dies with
  /// the VM at reload); this struct is the SCHEDULE half, stored on `Editor` (so `next_wake(&Editor,_)`
  /// can see the next-due) and auto-disarmed by `Editor::clear_plugin_wake_state`.
  #[derive(Clone, Debug)]
  pub struct PluginTimer {
      pub handle: u64,          // opaque handle returned to Lua (monotonic, never reused)
      pub origin: String,       // owning plugin (per-plugin cap + plugin_list); from InvokeState.current
      pub key: String,          // "wc-timer-<handle>" — the VM-registry callback key
      pub next_due_ms: u64,     // wall-clock ms of the next fire
      pub interval_ms: u64,     // >= PLUGIN_TIMER_MIN_INTERVAL_MS (floor-checked at arm)
      pub repeat: bool,         // false = one-shot (remove after firing); true = reschedule-from-completion
      pub pending: bool,        // true ONLY while this timer's callback is in flight (one-pending-per-timer)
  }
  ```
- `editor.rs` — add four fields to `struct Editor` (grep `pub pending_plugin_events:` — add beside it):
  ```rust
  /// Armed plugin timers (P3 §3). Lives on Editor (not the host) so `timers::next_wake(&Editor,_)` sees
  /// the next-due. Auto-disarmed by `clear_plugin_wake_state`. Bounded by PLUGIN_MAX_TIMERS_PER_PLUGIN/plugin.
  pub pending_plugin_timers: Vec<crate::plugin::PluginTimer>,
  /// Monotonic handle allocator for wc.timer (never reused, even across reload).
  pub next_timer_handle: u64,
  /// on_change debounce deadline (P3 §6) — armed beside the reconcile debounce, self-clears on fire.
  pub on_change_due: Option<u64>,
  /// True iff some loaded plugin registered a `wc.on("change", …)` hook. Gates on_change's wake so it
  /// costs zero when unused. Recomputed at bridge-attach; cleared on teardown.
  pub has_on_change_subscriber: bool,
  ```
  Initialize all four in EVERY `Editor` constructor (grep the `pending_plugin_events:
  std::collections::VecDeque::new(),` init line — add `pending_plugin_timers: Vec::new(),
  next_timer_handle: 0, on_change_due: None, has_on_change_subscriber: false,`; the compiler forces every
  constructor site).
  Add the teardown helper (used by Tasks 3/4):
  ```rust
  impl Editor {
      /// Reset BOTH new P3 wake subsystems to their no-plugins baseline (P3 §3g, Codex Critical 2):
      /// the timer schedule AND the on_change subscription. Called from every teardown path
      /// (`perform_reload`, the null-host pump branch) so a dead subsystem never keeps waking the loop.
      /// The P2 queues (pending_plugin_calls/events/dispatch) are cleared separately by those sites.
      pub fn clear_plugin_wake_state(&mut self) {
          self.pending_plugin_timers.clear();
          self.has_on_change_subscriber = false;
          self.on_change_due = None;
      }
  }
  ```

**Migration:** the `Editor` constructor init sites (compiler-forced — grep `pending_plugin_events:` init);
the exhaustive `PluginEventKind` matches (`event_from_str`, `kind_str`, and any `match kind` in
`pump.rs`/`load.rs` — the compiler forces each to add a `Change` arm).

**Acceptance:** `cargo build -p wordcartel` warning-free; `cargo test -p wordcartel limits:: editor:: plugin::`
green; app runs unchanged; `module_budgets` unaffected.

**Contract:** N/A — no command surface touched.

---

## Task 2 — `timers.rs`: the two static deadline rows + the idle-free guardrail tests

The `next_wake` half of the timer/on_change subsystems, landed BEFORE any firing so the guardrail tests
prove zero-cost-at-rest against an empty timer set. Depends only on Task 1's editor fields.

**Model:** most-capable (the idle-free proof is the load-bearing invariant). **Files:**
`wordcartel/src/timers.rs`.

**TDD first (in `timers.rs` tests — extend the existing guardrail suite):**
- `next_wake_none_with_commands_only_plugin` — a fresh editor with the new fields at baseline (no timer,
  `has_on_change_subscriber == false`) ⇒ `next_wake(&e, 10_000) == None` (both new rows `None`).
- `on_change_deadline_none_without_subscriber` — set `e.on_change_due = Some(400)` but leave
  `has_on_change_subscriber == false` ⇒ the `on_change` row is `None`; flip the flag ⇒ `Some(400)`.
- `plugin_timer_deadline_reads_min_nonpending` — push two `PluginTimer`s (due 500 and 300, both
  `!pending`) ⇒ the `plugin_timer` row is `Some(300)`; mark the 300 one `pending` ⇒ `Some(500)`; mark both
  `pending` ⇒ `None`.
- The existing `next_wake_none_when_settled` + `gated_subsystems_yield_none` must still pass unchanged (the
  two new rows are `None` for their settled/no-plugin editors; `gated_subsystems_yield_none` selects
  builtins by name and is unaffected by two extra rows).

**Implementation** — append two rows to `SUBSYSTEMS` (grep `pub(crate) static SUBSYSTEMS`); builtins stay
fn-ptr rows, NO Vec upgrade, `next_wake`/`on_tick` bodies untouched:
```rust
/// on_change debounce (P3 §6): the content-settled deadline, GATED on a subscriber so it is zero-cost
/// when no plugin uses on_change (proportional-to-work). Edge-armed by an edit (like reconcile),
/// self-clearing on fire — stays inside the idle-free law.
fn on_change_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if e.has_on_change_subscriber { e.on_change_due } else { None }
}

/// Plugin-timer deadline (P3 §3): the soonest NON-pending armed timer's next-due. `None` when no timer
/// is armed (idle-free preserved). A `pending` timer is excluded (its callback is in flight — the
/// one-pending-per-timer rule; the same in-flight-gate shape as the builtin swap/diag/reconcile rows).
/// NOTE: a due timer's next-due may be in the past, so this can be < `now`; `run` uses `saturating_sub`
/// → one immediate wake, then the pump fires + reschedules to a future due (spec §4).
fn plugin_timer_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.pending_plugin_timers.iter().filter(|t| !t.pending).map(|t| t.next_due_ms).min()
}

pub(crate) static SUBSYSTEMS: &[TimedSubsystem] = &[
    // … the 8 existing builtins, UNCHANGED …
    TimedSubsystem { name: "on_change",    deadline: on_change_deadline },
    TimedSubsystem { name: "plugin_timer", deadline: plugin_timer_deadline },
];
```

**Acceptance:** `cargo test -p wordcartel timers::` green (new + existing guardrails); warning-free; clippy
clean; `module_budgets` `timers.rs` ≤ 400 (report the number).

**Contract:** N/A — internal wake-source table.

---

## Task 3 — `wc.timer` arm/cancel + the pump fire phase + `clear_plugin_wake_state` wiring

The timer engine: the `wc.timer`/`wc.timer_cancel` API (observer/floor/cap-checked), the pump's mark-then-
fire Phase 0 (Critical 1), and the teardown clears (Critical 2). Depends on Tasks 1 (fields/`PluginTimer`)
+ 2 (the `plugin_timer` row so an armed timer actually wakes the loop).

**Model:** most-capable (the mark-then-fire atomicity + the observer-tier boundary + the borrow
choreography). **Files:** `wordcartel/src/plugin/api.rs` (`install_timer`), `wordcartel/src/plugin/pump.rs`
(Phase 0 + the pump's null-host clear), `wordcartel/src/plugin/reload.rs` (`perform_reload` clear).

**TDD first (in `plugin/pump.rs` tests, using the `make`/`pump_test` idiom + the real `pump` where a
specific reg is needed):**
- `wc_timer_arms_and_next_wake_reflects_it` — a command callback calling `wc.timer(60000, fn)` arms one
  `PluginTimer`; `next_wake` returns its due; a second `wc.timer` → two; cancel one → one.
- `wc_timer_fires_during_inactivity_via_pump` — arm a 1000 ms one-shot; advance a `TestClock` past the
  due; `pump` (no input between) → the callback ran (a status/marker), the timer is REMOVED (one-shot),
  `next_wake == None` after.
- `wc_timer_repeat_reschedules_from_completion` — a `repeat=true` 1000 ms timer; fire; assert
  `next_due_ms == completion_now + 1000` and `pending == false`, and it is NOT removed.
- `wc_timer_below_floor_is_typed_error` — `wc.timer(999, fn)` → `pcall` false, error mentions "floor",
  nothing armed.
- `wc_timer_over_cap_is_typed_error` — arm 8, the 9th (same plugin) → `pcall` false, "limit", still 8 armed.
- `wc_timer_from_hook_is_rejected` — `wc.on("save", function() wc.timer(1000, function() end) end)` →
  fired via an event → the `wc.timer` `pcall` is false ("event hook or a timer callback"), nothing armed
  (observer gate).
- `timer_callback_is_observer_tier_cannot_edit` — a timer callback body `pcall(wc.insert,'X')` → the edit
  is BLOCKED ("editing is not allowed from an event hook"), the buffer is UNCHANGED, but `wc.status` in
  the same callback STILL works.
- `cap_trip_does_not_strand_a_due_timer` (Critical 1) — arm enough due timers (or drive a cascade) that a
  pump cycle trips `PLUGIN_PUMP_CHAIN_CAP` mid-fire-batch; assert every not-yet-fired due timer is still
  `!pending` and `plugin_timer_deadline` still returns its due (it fires next pump), never stranded.
- `no_bridge_does_not_strand_a_due_timer` (Codex Important 2) — a host with `lua: Some`, `bridge: None`
  (construct a `PluginHost::new()`, seed a due `PluginTimer` on the editor directly, do NOT
  `attach_bridge`); `fire_due_timers`/`pump` fires nothing AND leaves the timer `!pending` (still visible
  in `plugin_timer_deadline`) — no bridge-None strand.
- `floored_repeating_timer_bounded_wakes_not_a_spin` — advance a `TestClock` over N seconds driving a
  1000 ms repeating timer through repeated `pump`s; assert it fires ≤ N+1 times and every post-fire
  `next_due - now >= 1000` (bounded cadence, not a spin).
- `teardown_clears_both_subsystems` (Critical 2, timer half here; on_change half in Task 4) — arm a timer,
  `perform_reload` (or the null-host pump) → `pending_plugin_timers` empty AND `next_wake == None`.

**Implementation:**
- `plugin/api.rs` — `install_timer(lua, &wc, bridge)` in `install_editor_api` (grep `fn install_command`
  — add the call beside it). Two plain functions on the `wc` table, `timer` and `timer_cancel` (decided in
  spec §3b — no metatable). **Signature refinement of spec §3b (recorded, mechanical, not a design
  change):** `wc.timer(interval_ms, fn [, repeat_bool])` — positional with an optional `repeat` boolean,
  matching `wc.replace`'s existing `(a, b, text)` tuple-extraction idiom (cleaner than the spec's `{table}`
  sketch; the pomodoro demo calls `wc.timer(ms, fn)` for one-shot). The guardrails are identical:
  ```rust
  fn install_timer(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
      let editor = bridge.editor.clone();
      let clock = bridge.clock.clone();
      let invoke = bridge.invoke_state.clone();
      // wc.timer(interval_ms, fn [, repeat_bool])  — one-shot unless repeat is true.
      wc.set("timer", lua.create_function(
          move |lua, (interval_ms, func, repeat): (u64, mlua::Function, Option<bool>)| {
              if invoke.borrow().observer {
                  return Err(mlua::Error::runtime(
                      "plugin: wc.timer is not allowed from an event hook or a timer callback"));
              }
              if interval_ms < crate::limits::PLUGIN_TIMER_MIN_INTERVAL_MS {
                  return Err(mlua::Error::runtime("plugin: timer interval below the 1000 ms floor"));
              }
              let origin = invoke.borrow().current.clone().unwrap_or_default();
              let mut e = editor.try_borrow_mut()
                  .map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
              if e.pending_plugin_timers.iter().filter(|t| t.origin == origin).count()
                  >= crate::limits::PLUGIN_MAX_TIMERS_PER_PLUGIN {
                  return Err(mlua::Error::runtime("plugin: timer limit reached (max 8)"));
              }
              e.next_timer_handle += 1;
              let handle = e.next_timer_handle;
              let key = format!("wc-timer-{handle}");
              lua.set_named_registry_value(&key, func)?;   // persist the callback (dies with the VM)
              let now = clock.now_ms();
              e.pending_plugin_timers.push(crate::plugin::PluginTimer {
                  handle, origin, key, next_due_ms: now.saturating_add(interval_ms),
                  interval_ms, repeat: repeat.unwrap_or(false), pending: false,
              });
              Ok(handle as i64)   // Lua integer (i64); the monotonic counter never reaches 2^63
          })?)?;
      // wc.timer_cancel(handle) — remove + free the registry key; unknown handle → silent no-op.
      let editor = bridge.editor.clone();
      let invoke = bridge.invoke_state.clone();
      wc.set("timer_cancel", lua.create_function(move |lua, handle: i64| {
          if invoke.borrow().observer {
              return Err(mlua::Error::runtime(
                  "plugin: wc.timer_cancel is not allowed from an event hook or a timer callback"));
          }
          let handle = handle as u64;
          let mut e = editor.try_borrow_mut()
              .map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
          if let Some(pos) = e.pending_plugin_timers.iter().position(|t| t.handle == handle) {
              let key = e.pending_plugin_timers.remove(pos).key;
              lua.set_named_registry_value(&key, mlua::Value::Nil)?;   // free the callback
          }
          Ok(())
      })?)?;
      Ok(())
  }
  ```
  (Rationale for positional `(interval, fn, repeat?)` over the spec's `{table}` sketch: mlua's typed
  `create_function` tuple extraction is the cleaner path and matches `wc.replace`'s existing
  `(a, b, text)` idiom; the pomodoro demo uses `wc.timer(ms, fn)` for one-shot and would pass `true` for
  repeat. Recorded as a mechanical refinement of spec §3b, not a design change — the guardrails are
  identical.)
  **Note the observer check reads `invoke.borrow()` then drops it before `editor.try_borrow_mut()`** —
  never two borrows held across each other (the P2 discipline).
- `plugin/pump.rs` — **Phase 0** (grep `pub fn pump` — insert AFTER the null-host branch and BEFORE
  `let start = std::time::Instant::now();`… actually `start`/`units` must exist first; restructure: move
  `let start`/`let mut units` above Phase 0). Add a `fire_due_timers` helper to keep `pump` thin:
  ```rust
  // in pump(), after the null-host branch:
  let start = std::time::Instant::now();
  let mut units = 0usize;
  if self.fire_due_timers(editor, clock, &mut units, start) { return; }   // Phase 0 (mark-then-fire)
  loop { /* the existing P2 Phase A/B re-drain, unchanged */ }
  ```
  ```rust
  /// Phase 0 — fire due plugin timers (P3 §3d). **Invariant (Critical 1 + Codex Important 2): a timer is
  /// `pending` ONLY when its callback is DEFINITELY about to run — no guard checked AFTER marking
  /// (`cap_tripped`, bridge-None, VM-null) can strand it.** So EVERY guard is checked BEFORE `pending` is
  /// set: the VM-null guard is the pump's own `self.lua.is_none()` early-return; the bridge-None guard is
  /// hoisted to the TOP here (`lua: Some, bridge: None` after an attach_bridge failure — no `invoke_state`,
  /// no editor API installed, so NO timer can run: fire nothing, mark nothing); the cap guard is checked
  /// per iteration before marking. `pending` is set in the same short borrow immediately before invoke.
  /// Observer-tier callbacks (like hooks); reschedule-from-completion; one-pending-per-timer.
  fn fire_due_timers(&self, editor: &Rc<RefCell<Editor>>, clock: &dyn Clock,
                     units: &mut usize, start: std::time::Instant) -> bool {
      // Bridge-None guard HOISTED (Codex Important 2): without a bridge there is no invoke_state and no
      // editor API — no timer callback can run, so mark nothing (defense-in-depth beyond the
      // bridge-attach-failure path also calling clear_plugin_wake_state).
      let Some(invoke_state) = self.bridge.as_ref().map(|b| b.invoke_state.clone()) else { return false };
      let lua = self.lua.as_ref().expect("pump checked self.lua.is_some() before calling fire_due_timers");
      let now = clock.now_ms();
      // Snapshot due HANDLES only (no `pending` mutation yet):
      let due: Vec<u64> = {
          let e = editor.borrow();
          e.pending_plugin_timers.iter()
              .filter(|t| !t.pending && now >= t.next_due_ms).map(|t| t.handle).collect()
      };
      for handle in due {
          if self.cap_tripped(*units, start, editor) { return true; } // BEFORE mark — remaining NEVER marked
          *units += 1;
          // ALL guards passed (VM present, bridge present, cap ok) → mark-then-read atomically in the
          // instant before invoke; skip (no mark) if cancelled/removed since the snapshot.
          let key = {
              let mut e = editor.borrow_mut();
              match e.pending_plugin_timers.iter_mut().find(|t| t.handle == handle) {
                  Some(t) => { t.pending = true; t.key.clone() }
                  None => continue,   // cancelled/removed since the snapshot — nothing was marked
              }
          };
          let label = format!("timer#{handle}");
          let _guard = ObserverGuard::enter(invoke_state.clone(), &label, true);   // observer-tier
          let outcome: Result<mlua::Result<()>, String> = crate::panicx::catch(|| {
              let cb: mlua::Function = lua.named_registry_value(&key)?;
              self.with_time_guard(lua, || cb.call::<()>(()))
          });
          drop(_guard);
          if let Err(msg) = normalize(outcome) { crate::plugin::plugin_error(editor, &label, &msg); }
          // Reschedule FROM COMPLETION (or remove one-shot / already-cancelled).
          let now2 = clock.now_ms();
          let mut e = editor.borrow_mut();
          if let Some(pos) = e.pending_plugin_timers.iter().position(|t| t.handle == handle) {
              if e.pending_plugin_timers[pos].repeat {
                  e.pending_plugin_timers[pos].next_due_ms = now2.saturating_add(e.pending_plugin_timers[pos].interval_ms);
                  e.pending_plugin_timers[pos].pending = false;
              } else {
                  let key = e.pending_plugin_timers.remove(pos).key;
                  let _ = lua.set_named_registry_value(&key, mlua::Value::Nil);
              }
          }
      }
      false
  }
  ```
  (`ObserverGuard`, `normalize`, `with_time_guard`, `cap_tripped` all already exist in `pump.rs` — grep
  them; `fire_due_timers` reuses them verbatim.) **Null-host pump branch:** add `e.clear_plugin_wake_state();`
  beside the existing three-queue clear (grep `e.pending_plugin_dispatch.clear();` in the null-host arm).
- `plugin/reload.rs` — `perform_reload` step 5 (grep `e.pending_plugin_dispatch.clear();` in the
  queue-clear block): add `e.clear_plugin_wake_state();` in the SAME borrow block. (This clears the timer
  schedule + the on_change subscription; the rebuild's `attach_bridge` re-sets `has_on_change_subscriber`
  and the reloaded plugin re-arms — Task 4/§5.)

**Acceptance:** `cargo test -p wordcartel plugin::pump plugin::reload` green (all timer/observer/cap/
teardown tests); warning-free; clippy clean; `module_budgets` `pump.rs` ≤ 350 (report).

**Contract:** N/A — `wc.timer`/`wc.timer_cancel` are Lua APIs, not registry commands.

---

## Task 4 — `on_change`: the debounced content-settled event

Wire the `on_change` subsystem: recompute `has_on_change_subscriber`, arm `on_change_due` beside the
reconcile debounce, fire in `on_tick`. Depends on Tasks 1 (Change enum + fields) + 2 (the `on_change`
deadline row). Reuses the entire P2 event machinery (`fire_event`, the pump event-drain, `wc.on`).

**Model:** most-capable (the debounce-not-hot-path invariant). **Files:** `wordcartel/src/plugin/host.rs`
(`attach_bridge` recompute), `wordcartel/src/app.rs` (`advance` arm), `wordcartel/src/timers.rs`
(`on_tick` fire).

**TDD first:**
- `plugin::host::tests::attach_bridge_sets_on_change_subscriber` — load a plugin with `wc.on("change",
  fn)`, attach → `editor.has_on_change_subscriber == true`; a plugin without → `false`.
- `on_change_fires_debounced_after_settle` (in `timers.rs` or `e2e.rs`) — with an on_change subscriber:
  an edit arms `on_change_due`; a `Tick` BEFORE the debounce elapses fires nothing; a `Tick` AFTER →
  `fire_event(Change)` enqueued, the pump delivers it, the hook ran (status/marker); `on_change_due`
  cleared after.
- `on_change_is_not_per_keystroke` — three rapid edits (version bumps) within the debounce window arm
  `on_change_due` at most once-pending (the version-latch: re-arm only on version advance, never pushed by
  idle Ticks); after the burst settles, exactly ONE on_change fires — not three.
- `on_change_deadline_none_after_teardown` (Critical 2 on_change half) — subscribe + arm; `perform_reload`
  to null → `has_on_change_subscriber == false`, `on_change_due == None`, `next_wake == None`.

**Implementation:**
- `plugin/host.rs` — `attach_bridge` (grep `pub fn attach_bridge`), after the bridge is installed and the
  hooks are committed, recompute the flag (one line):
  ```rust
  // (inside attach_bridge, after self.bridge = Some(bridge); — the host's `hooks` are already committed)
  self.bridge.as_ref().unwrap().editor.borrow_mut().has_on_change_subscriber =
      self.hooks.iter().any(|h| h.kind == crate::plugin::PluginEventKind::Change);
  ```
  (Or set it via the `editor` handle before moving it into the bridge — the implementer picks the borrow-
  clean form; the invariant is "after attach, the flag reflects the host's committed Change hooks.")
- `app.rs` — `advance` (grep the reconcile-debounce arm: `b.reconcile.due_at = Some(now.saturating_add(
  crate::reconcile::RECONCILE_DEBOUNCE_MS));`). Arm `on_change_due` on the SAME version-latched condition,
  only when subscribed. Because `b = editor.active_mut()` borrows the editor, compute inside then set the
  editor-level field after the buffer borrow drops:
  ```rust
  // Replace the existing `{ let now…; let b = editor.active_mut(); if … { b.reconcile.due_at = …; } }`
  // block so it ALSO arms on_change_due (editor-level) when a plugin subscribes:
  {
      let now = clock.now_ms();
      let subscribed = editor.has_on_change_subscriber;
      let b = editor.active_mut();
      let arm = b.reconcile.maybe_stale && b.reconcile.in_flight_version.is_none()
          && (b.reconcile.due_at.is_none() || b.reconcile.armed_for_version != b.document.version);
      if arm {
          b.reconcile.due_at = Some(now.saturating_add(crate::reconcile::RECONCILE_DEBOUNCE_MS));
          b.reconcile.armed_for_version = b.document.version;
      }
      // on_change: same version-latched debounce, editor-level, only if a plugin subscribes.
      if arm && subscribed {
          editor.on_change_due = Some(now.saturating_add(crate::reconcile::RECONCILE_DEBOUNCE_MS));
      }
  }
  ```
  (~4 net lines; `app.rs` stays under 1000. The version-latch is inherited: `arm` is true only when the
  version advanced, so idle Ticks never push `on_change_due`.)
- `timers.rs` — `on_tick` (grep `pub(crate) fn on_tick`): add an on_change fire branch (fires from the
  cold Tick path — NOT the hot edit path):
  ```rust
  // in on_tick, after the reconcile dispatch branch:
  if editor.has_on_change_subscriber {
      if let Some(due) = editor.on_change_due {
          if now >= due {
              editor.on_change_due = None;
              let path = editor.active().document.path.clone();
              crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::Change, path.as_deref());
          }
      }
  }
  ```

**Acceptance:** `cargo test -p wordcartel plugin::host timers:: e2e::` green (subscriber recompute,
debounced fire, not-per-keystroke, teardown-clears); warning-free; clippy clean; typing latency unchanged
(the hot path — `advance`'s arm — only sets an `Option`, never invokes Lua).

**Contract:** `on_change` is a host↔plugin event (like `on_save`) — not a command-surface actor. N/A.

---

## Task 5 — parameterized commands (the widening + the full Copy-loss / constructor migration)

A plugin command may take a string argument. Independent of Tasks 3/4 (touches registry/minibuffer/load/
api/pump). Depends only on P2. **This task carries the heaviest migration — every site is enumerated
below (grep-verified 2026-07-12); a missed one is a compile error.**

**Model:** most-capable. **Files:** `wordcartel/src/registry.rs`, `wordcartel/src/plugin/mod.rs`,
`wordcartel/src/minibuffer.rs`, `wordcartel/src/plugin/api.rs`, `wordcartel/src/plugin/host.rs`,
`wordcartel/src/plugin/load.rs`, `wordcartel/src/plugin/pump.rs`.

**TDD first (the three dispatch cases are the load-bearing ones — Codex Critical):**
- `registry::tests::param_command_no_arg_opens_prompt` (case 3) — a `Plugin` entry with `meta.arg =
  Some("Prompt")` dispatched via `reg.dispatch(id, ctx)` (no arg) opens a `MinibufferKind::PluginArg { id }`
  with that prompt; `pending_plugin_calls` is still empty (nothing enqueued yet).
- `registry::tests::param_command_with_supplied_arg_does_not_reprompt` (case 2 — the bug this fixes) —
  `reg.dispatch_with_arg(id, ctx, Some("25".into()))` on the SAME parameterized entry enqueues
  `PluginCall { id, arg: Some("25") }` DIRECTLY and opens NO minibuffer (`ctx.editor.minibuffer.is_none()`).
- `registry::tests::nullary_plugin_command_dispatches_with_none` (case 4) — a `Plugin` entry with
  `meta.arg == None` via `reg.dispatch` pushes `PluginCall { id, arg: None }`, no minibuffer.
- `registry::tests::builtin_dispatch_ignores_supplied_arg` (case 1) — `dispatch_with_arg(CommandId("save"),
  ctx, Some("x".into()))` runs the builtin (Handled), arg dropped.
- `minibuffer::tests::plugin_arg_submit_enqueues_call_with_arg` — an open `PluginArg` minibuffer with text
  "25", Enter → `pending_plugin_calls` gains `PluginCall { id, arg: Some("25") }`.
- `plugin::load::tests::register_command_with_arg_prompt` — `wc.register_command{ …, arg='Minutes:' }` →
  `reg.meta(id).arg == Some("Minutes:")`; absent → `None`.
- `plugin::pump::tests::wc_command_with_arg_reaches_callback_no_reprompt` — `wc.command('t.echo', 'hi')` on
  a parameterized `t.echo` → the pump's `drain_one_dispatch` calls `dispatch_with_arg(…, Some("hi"))`,
  which enqueues the call directly (NO minibuffer opened), and the callback `function(arg)` receives "hi".
- `plugin::pump::tests::param_command_callback_receives_arg` — a palette-style dispatch of a
  parameterized command collects "25" via the minibuffer, and the callback receives "25".
- `arg_over_cap_is_rejected` — an arg > `PLUGIN_MAX_COMMAND_ARG` at BOTH `wc.command` and the minibuffer
  submit → rejected (typed error / status), nothing enqueued.

**Implementation + COMPLETE migration lists:**
- **`registry.rs` `CommandMeta` gains `arg: Option<&'static str>`** (grep `pub struct CommandMeta`). All 3
  literal constructors (grep `CommandMeta {`): `register` (`registry.rs:110` → `arg: None`),
  `register_stateful` (`:118` → `arg: None`), `register_plugin` (`:131` → `arg` from its new param).
- **`registry.rs::register_plugin` signature gains `arg: Option<&'static str>`** (`:124`). **All 12 call
  sites** (grep `register_plugin(` — verified 2026-07-12): the doc example (`:143` → `…, None)`), the prod
  commit loop (`plugin/load.rs:345`), and the 10 registry tests (`:1019, :1029, :1034, :1036, :1045,
  :1065, :1066, :1088, :1091, :1113` → each adds a trailing `None`). The compiler forces every one.
- **`registry.rs::dispatch` — add `dispatch_with_arg`; keep `dispatch` a thin wrapper (Codex Critical).**
  The bug: unconditionally opening a `PluginArg` minibuffer whenever `meta.arg == Some` IGNORES an
  already-supplied arg (from `wc.command(name, arg)` → `PluginDispatch.arg`, or a resolved prompt) and
  would wrongly re-prompt. Fix: `dispatch` must know whether an arg was supplied. Add `dispatch_with_arg`
  (verified `dispatch(&self, id, ctx)` at `registry.rs:741` pushes `PluginCall { id }` for the `Plugin`
  arm); keep the existing `dispatch(id, ctx)` signature as a delegating wrapper so **all 21 existing
  `reg.dispatch(id, ctx)` callers stay unchanged** (grep `reg.dispatch(` — input.rs, app.rs, pump.rs,
  registry tests: 21 sites; only `drain_one_dispatch` migrates, below):
  ```rust
  /// Dispatch `id` with NO argument supplied — palette/keybinding/menu path (the 21 existing callers).
  /// A parameterized plugin command (`meta.arg == Some`) that reaches here with no arg opens its prompt.
  pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
      self.dispatch_with_arg(id, ctx, None)
  }

  /// Dispatch `id`, threading an OPTIONAL already-collected argument. `arg == Some` means the value is
  /// in hand (from `wc.command(name, arg)` or a resolved `PluginArg` prompt) — enqueue directly, NEVER
  /// re-prompt. Covers all four cases:
  pub fn dispatch_with_arg(&self, id: CommandId, ctx: &mut Ctx, arg: Option<String>) -> CommandResult {
      match self.index.get(&id) {
          Some(&i) => match &self.entries[i].handler {
              // 1. Builtin — nullary; any supplied arg is dropped (builtins take no arg today).
              HandlerKind::Builtin(h) => { let _ = arg; h(ctx) }
              HandlerKind::Plugin => {
                  match (self.entries[i].meta.arg, arg) {
                      // 2. arg SUPPLIED (wc.command with arg, or the user already answered the prompt) →
                      //    enqueue directly, no minibuffer. (Also covers a nullary plugin command that a
                      //    plugin passed an arg to — the callback simply ignores the extra value.)
                      (_, Some(supplied)) => ctx.editor.pending_plugin_calls.push_back(
                          crate::plugin::PluginCall { id, arg: Some(supplied) }),
                      // 3. DECLARES an arg (`meta.arg == Some`) but none supplied (palette/keybinding) →
                      //    open the PluginArg prompt; its submit re-enters via case 2's direct enqueue.
                      (Some(prompt), None) => ctx.editor.minibuffer = Some(crate::minibuffer::Minibuffer {
                          prompt: prompt.to_string(), text: String::new(), cursor: 0,
                          kind: crate::minibuffer::MinibufferKind::PluginArg { id },
                      }),
                      // 4. Nullary plugin command, no arg → enqueue nullary (today's behavior).
                      (None, None) => ctx.editor.pending_plugin_calls.push_back(
                          crate::plugin::PluginCall { id, arg: None }),
                  }
                  CommandResult::Handled
              }
          },
          None => { ctx.editor.status = format!("unknown command: {}", id.0); CommandResult::Noop }
      }
  }
  ```
  (`registry.rs` stays Lua-free — opening a minibuffer is editor-state mutation, no `mlua`. The
  `PluginArg` minibuffer submit — Task 5's `minibuffer.rs` arm — pushes `PluginCall { id, arg: Some(text) }`
  DIRECTLY, i.e. it is case 2 completing after the prompt, NOT another `dispatch` round.)
- **`plugin/pump.rs::drain_one_dispatch` threads `d.arg`** (grep `reg.dispatch(id, &mut ctx)` at
  `pump.rs:187`): change the call to `reg.dispatch_with_arg(id, &mut ctx, d.arg.clone())` — so a
  `wc.command("pomodoro.start", "50")` enqueues `PluginCall { id, arg: Some("50") }` without re-prompting
  (case 2), while `wc.command("pomodoro.start")` with no arg opens the prompt (case 3). This is the ONLY
  `dispatch` caller that migrates to `dispatch_with_arg`.
- **`plugin/mod.rs` `PluginCall` gains `arg: Option<String>` (drops `Copy`)** (grep `struct PluginCall` —
  currently `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` → **remove `Copy`**, keep the rest). `PluginDispatch`
  gains `arg: Option<String>` (grep `struct PluginDispatch`). **Every construction site** (grep
  `PluginCall\|PluginDispatch` — verified 2026-07-12), each `{ id … }` → `{ id, …, arg: None }` unless it
  supplies one:
  - Production (3): the `dispatch_with_arg` `Plugin` arm above (the 4 match cases construct
    `PluginCall { id, arg: … }`); `plugin/pump.rs:197` `invoke_call(… call: PluginCall)` (now passes
    `call.arg` to the Lua fn — below; `call` is moved in by value, fine for non-Copy) and `:187`
    `drain_one_dispatch` (calls `dispatch_with_arg(id, ctx, d.arg.clone())` — above); `plugin/api.rs:437`
    `install_command` constructs `PluginDispatch { origin, name, … }` → add `arg` (from `wc.command`'s
    optional 2nd param — below).
  - `registry.rs` tests (2) — **Codex Important 1 audit result:** the ONLY indexed accesses to any of the
    three plugin queues (grep `pending_plugin_calls\[`/`_dispatch\[`/`_events\[` → exactly `:1056`, `:1104`)
    are **`assert_eq!(e.pending_plugin_calls[0], crate::plugin::PluginCall { id })`**. `assert_eq!` takes
    the index by **reference** (`match (&a, &b)`), so it does NOT move out — **no `Copy` is required, no
    compile error**. The ONLY change at both sites is the RHS literal: `PluginCall { id }` → `{ id,
    arg: None }` (`PartialEq`/`Eq` are retained). There is **no `let x = queue[i];` move-out anywhere** in
    the real source, so no `&queue[i]`/`.clone()`/`.front()` fix is needed.
  - `plugin/host.rs` tests (15): `PluginCall` at `:257, :420, :433, :530, :555, :613, :622, :744, :765,
    :860, :887, :915, :939, :1024, :1064` → each `{ id, arg: None }`; the `PluginDispatch` at `:746` → add
    `arg: None`.
  - `plugin/reload.rs` tests (4): `PluginCall` `:258`, `:359`; `PluginDispatch` `:261`, `:362` → add the field.
  - `e2e.rs`: any `PluginCall`/`PluginDispatch` constructor the compiler flags → add the field.
- **`plugin/host.rs::PendingReg` gains `pub arg: Option<String>`** (grep `pub struct PendingReg`). Its ONE
  construction site — `plugin/api.rs:81` (`install_registration` → parse `spec.arg` and add the field).
- **`plugin/load.rs` commit plumbing** (grep in `load_one`): the commit tuple type
  `Vec<(CommandId, &'static str, Option<MenuCategory>)>` (`:311`) → add `Option<&'static str>` (the
  interned arg); the phase-1 push `committed.push((id, label, p.menu))` (`:323`) →
  `committed.push((id, label, p.menu, p.arg.as_deref().map(crate::plugin::intern)))` (intern the prompt
  only on the committed survivor — same intern-on-commit discipline as name/label); the phase-2 loop
  `for (id, label, menu) in committed { reg.register_plugin(id, label, menu) … }` (`:344`) →
  `for (id, label, menu, arg) in committed { reg.register_plugin(id, label, menu, arg) … }`.
- **`plugin/api.rs::install_registration`** (grep `PendingReg {`, `api.rs:81`) — parse the optional `arg`
  prompt with the borrowed-length-check-then-convert pattern (cap `PLUGIN_MAX_LABEL_LEN`, like `label`):
  ```rust
  let arg_raw: Option<mlua::String> = spec.get("arg")?;
  if let Some(a) = &arg_raw {
      if a.as_bytes().len() > crate::limits::PLUGIN_MAX_LABEL_LEN {
          return Err(mlua::Error::runtime("plugin: command arg prompt too long")); }
  }
  let arg = match &arg_raw { Some(a) => Some(a.to_str()?.to_owned()), None => None };
  // … push PendingReg { name_full, label, menu, func, arg };
  ```
- **`plugin/api.rs::install_command`** (grep `wc.set("command"`, `:421`) — `wc.command(name, arg?)`:
  extract an optional 2nd `mlua::String` arg, cap on borrowed bytes against `PLUGIN_MAX_COMMAND_ARG`
  BEFORE owning, push `PluginDispatch { origin, name: …, arg: Some(a.to_str()?.to_owned()) }` (or `None`):
  ```rust
  move |_, (name, arg): (mlua::String, Option<mlua::String>)| {
      // … observer + name-length + queue-cap checks (unchanged P2) …
      let arg = match &arg {
          Some(a) => {
              if a.as_bytes().len() > crate::limits::PLUGIN_MAX_COMMAND_ARG {
                  return Err(mlua::Error::runtime("plugin: command arg too long")); }
              Some(a.to_str()?.to_owned())
          }
          None => None,
      };
      e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
          origin, name: name.to_str()?.to_owned(), arg });
      Ok(())
  }
  ```
- **`plugin/pump.rs::invoke_call`** (grep `fn invoke_call`, `:197`) — pass the arg to the Lua callback:
  `cb.call::<()>((call.arg,))` (the arg becomes the callback's first parameter; a nullary command's `fn`
  ignores the extra nil). **`drain_one_dispatch`** (`:179`) pushes `PluginCall { id, arg: d.arg.clone() }`.
- **`minibuffer.rs` `MinibufferKind` gains `PluginArg { id: CommandId }`** (stays `Copy` — `CommandId` is
  Copy; the prompt reuses `Minibuffer.prompt`). Add its `Enter` submit arm (grep the `match mb.kind`
  block, `:111-117`):
  ```rust
  MinibufferKind::PluginArg { id } => {
      if mb.text.len() > crate::limits::PLUGIN_MAX_COMMAND_ARG {
          editor.status = "plugin: command arg too long".into();
      } else {
          editor.pending_plugin_calls.push_back(
              crate::plugin::PluginCall { id, arg: Some(mb.text) });
      }
  }
  ```
  (`MinibufferKind` derives `Copy` — grep `enum MinibufferKind`; `PluginArg { id: CommandId }` preserves it.)

**Acceptance:** `cargo test -p wordcartel registry:: minibuffer:: plugin::` green (all TDD cases + every
pre-existing test that constructs `PluginCall`/`CommandMeta`/calls `register_plugin` — the migration must
leave them green); `cargo build`/`test --no-run` warning-free; clippy clean.

**Contract (laws 1/3/4 + rule 10):** parameterized plugin commands are single registry entries, appear in
palette/menu by derivation, collect args via the shared minibuffer seam; `wc.command(name,arg)` routes
through `reg.dispatch`. Task realizes rule 10's argument-carrying shape for plugin commands.

---

## Task 6 — `plugin_list` shows armed timers

Trivial: extend the existing `plugin_list` builtin to read the new field. Depends on Task 1
(`pending_plugin_timers`).

**Model:** standard. **Files:** `wordcartel/src/registry.rs`.

**TDD first:**
- `registry::tests::plugin_list_reports_armed_timers` — dispatch `plugin_list` with N armed
  `pending_plugin_timers` on the editor → status includes "N timers".

**Implementation** — the `plugin_list` handler (grep `r.register("plugin_list"`, `:725`):
```rust
r.register("plugin_list", "List Plugins", Some(MenuCategory::Settings), |c| {
    let inv = &c.editor.plugin_inventory;
    let ok = inv.iter().filter(|r| r.error.is_none()).count();
    let failed = inv.len() - ok;
    let cmds: usize = inv.iter().map(|r| r.commands).sum();
    let hooks: usize = inv.iter().map(|r| r.hooks).sum();
    let timers = c.editor.pending_plugin_timers.len();   // P3: live armed-timer count
    c.editor.status = format!(
        "plugins: {ok} ok ({cmds} cmds, {hooks} hooks, {timers} timers), {failed} failed");
    CommandResult::Handled
});
```

**Acceptance:** `cargo test -p wordcartel registry::` green; warning-free; clippy clean.

**Contract:** law 1/3/4 unchanged — a display change to an existing registered command; satisfies §5's
"armed timers visible" consent-symmetry.

---

## Task 7 — `pomodoro.lua` demo + clock-driven e2e (the driver)

The concrete plugin that de-speculates the whole slice. Depends on Tasks 3 (`wc.timer`) + 5 (parameterized
commands) + P2 config.

**Model:** standard. **Files:** `wordcartel/tests/fixtures/plugins/pomodoro.lua` (new),
`wordcartel/src/e2e.rs`.

**Fixture** — `pomodoro.lua` (verbatim from spec §11; observer-safe: the COMMAND callbacks arm/cancel the
timer, the TIMER callback only calls `wc.status`):
```lua
local default_min = (wc.config and wc.config.minutes) or 25
local armed = nil
wc.register_command{
    name = 'start', label = 'Pomodoro: Start', menu = 'View', arg = 'Minutes (blank = default):',
    fn = function(arg)
        local minutes = tonumber(arg) or default_min
        if armed then wc.timer_cancel(armed) end
        armed = wc.timer(minutes * 60 * 1000, function()
            wc.status(string.format('Pomodoro: %d min session complete', minutes)); armed = nil
        end)
        wc.status(string.format('Pomodoro: %d min session started', minutes))
    end,
}
wc.register_command{
    name = 'cancel', label = 'Pomodoro: Cancel', menu = 'View',
    fn = function()
        if armed then wc.timer_cancel(armed); armed = nil; wc.status('Pomodoro: cancelled')
        else wc.status('Pomodoro: no session running') end
    end,
}
```

**e2e test** (`e2e.rs`, mirroring `wordcount_lua_e2e_success_demo` + advancing a `TestClock`):
`pomodoro_lua_e2e_success_demo` — load the fixture via `load_phase` with `[plugins.config.pomodoro]
minutes = 25`; dispatch `pomodoro.start` supplying arg `""` (uses the default) → assert
`editor.pending_plugin_timers.len() == 1` and `next_wake == Some(now + 25·60·1000)`; **advance the
`TestClock` past the deadline** and `reduce(Tick)` + `pump` with NO input between → assert the status reads
"session complete" (the observer-tier callback ran DURING inactivity), the timer is removed (one-shot),
`next_wake == None`; re-arm, dispatch `pomodoro.cancel` → `pending_plugin_timers` empty, `next_wake ==
None`; re-arm then `perform_reload` → `pending_plugin_timers` empty + `has_on_change_subscriber == false`
+ `next_wake == None` (auto-disarm). No wall-clock sleep — time advanced deterministically.

**Acceptance:** `cargo test -p wordcartel e2e::pomodoro` green; the demo fires during inactivity, cancels,
and auto-disarms on reload.

**Contract:** exercises the parameterized command (`pomodoro.start` with an arg) through the real dispatch,
and `plugin_list`'s timer count while armed — the contract surfaces in action.

---

## Task 8 — full guardrail + contract gate consolidation

Consolidate any spec §4/§12 guardrail or contract test not already landed task-locally, and re-run the
invariant set over a P3-loaded registry.

**Model:** standard. **Files:** `wordcartel/src/plugin/` + `timers.rs` + `e2e.rs` test modules.

**Tests (any not already covered by Tasks 2-7):**
- **Idle-free guardrail set (spec §4):** `next_wake_none_with_commands_only_plugin`,
  `teardown_clears_both_subsystems_next_wake_none`, `floored_repeating_timer_bounded_wakes_not_a_spin`,
  `cap_trip_does_not_strand_a_due_timer`, `on_change_deadline_none_without_subscriber` — confirm all are
  present and green (they land in Tasks 2/3/4; Task 8 verifies the full set exists).
- **Contract invariants (merge GATEs):** palette-completeness + menu-subset re-run over a registry that
  has a parameterized plugin command (it appears in palette; a `menu=Some(cat)` one in that menu);
  `plugin_list` in the Settings menu.
- **Loaded-but-idle (extends the P2/swap family):** a plugin with an on_change hook but NO armed timer,
  driven idle → zero hook invocations, `next_wake == None`, `timers::next_wake` unchanged across idle
  Ticks (on_change only arms on a real edit).

**Acceptance:** `cargo test -p wordcartel` fully green; `cargo clippy --workspace --all-targets` clean;
`scripts/smoke/run.sh` run + its one-line summary quoted; `module_budgets` green (app.rs ≤ 1000,
timers.rs ≤ 400, pump.rs ≤ 350 — report all three). The pomodoro demo passes.

---

## Final gates (after Task 8)

1. **Fable whole-branch review** — cross-task invariants: the idle-free law holds (both new rows `None` at
   rest; the exception is exactly scoped + anti-spin); mark-then-fire never strands a due timer on a
   cap-trip; teardown clears BOTH subsystems on every path (reload/null-host/exhaustion/bridge-fail);
   observer-tier timers cannot edit/dispatch/arm; on_change never fires per-keystroke (typing latency
   unchanged); no `mlua` leaked into `registry.rs`/`wordcartel-core`; the `PluginCall` Copy-loss migration
   is complete (no site left constructing the old shape). Fable compiles probes against the branch.
2. **Codex pre-merge gate** — independent GO/NO-GO: spec conformance, both LAWS' completeness, the idle-free
   proof re-checked against the merged code, module budgets, clippy, contract invariants, the migration
   completeness.
Re-run each after fixes until clean/GO. Then merge `--no-ff` to the trunk, verify tests on the merged
result, delete the branch. Push only when asked.

---

## Ledger

Track in `$(git rev-parse --git-path sdd)/progress.md`: one line per completed task + commit range. After
any compaction, trust the ledger + `git log` over recollection; never re-dispatch a task it marks done.
Cross-task signature evolution to record (so a later task doesn't re-litigate it):
- `CommandMeta` gains `arg: Option<&'static str>` (Task 5; 3 constructors + `register_plugin`'s 12 call
  sites + the `dispatch` arm).
- `PluginCall` drops `Copy`, gains `arg: Option<String>`; `PluginDispatch` gains `arg` (Task 5; ~26 sites —
  3 prod + registry/host/reload/e2e tests, enumerated in Task 5).
- `PendingReg` gains `arg: Option<String>` (Task 5; sole ctor `api.rs:81`); `load_one`'s commit tuple gains
  the interned arg (Task 5).
- `Editor` gains `pending_plugin_timers`/`next_timer_handle`/`on_change_due`/`has_on_change_subscriber` +
  `clear_plugin_wake_state` (Task 1); `PluginTimer` + `PluginEventKind::Change` (Task 1).
- `SUBSYSTEMS` gains `on_change`/`plugin_timer` rows (Task 2); the pump gains `fire_due_timers` Phase 0
  (Task 3); `MinibufferKind::PluginArg` (Task 5).
