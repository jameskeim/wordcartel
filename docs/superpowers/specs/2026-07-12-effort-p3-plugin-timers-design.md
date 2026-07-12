# Effort P3 — plugin timers + parameterized commands + on_change: the pomodoro slice

**Status:** SPEC (2026-07-12). Effort **P** phase 3 of the in-process Lua plugin system, scope LOCKED by
`docs/design/effort-p3-grounding.md` §5 (human-decided after the Codex + Fable timer red-team). Builds on
the shipped P1 (commands) + P2 (events/config/reload/`wc.command`). **Subprocess `wc.async` and the
plugin-contributed dynamic menu section are DEFERRED to their own future efforts — NOT specced here.**

Binding sources (authoritative): `docs/design/effort-p3-grounding.md` §5 (locked scope), `CLAUDE.md` (the
resource-behavior law + the two design LAWS), `docs/design/command-surface-contract.md`. Real code
surface verified against HEAD `8a06ab8` (post-P2): `timers.rs`, `app.rs::run`/`advance`, `registry.rs`,
`minibuffer.rs`, `reconcile.rs`, `plugin/{pump,host,api,load,reload,mod}.rs`, `editor.rs`, `limits.rs`.
Anchor on symbol NAMES.

The two P1/P2 design LAWS remain binding on every new plugin-input site:

> **LAW (input-validation).** Every plugin API that accepts a byte offset/range pre-validates it against
> the live buffer via `plugin_check_range` and degrades to a typed Lua error. (P3 adds no offset/range
> API — inherited unchanged.)

> **LAW (resource-bound).** Every plugin-supplied string crossing into a permanent leak or a Rust/Lua
> allocation MUST be bounded — borrowed-length-check-then-convert — BEFORE the allocation.

And P3 lives under the **resource-behavior law** (the crux of this effort):

> Background/periodic work is **edge-triggered by a real content/state change, never level-triggered off
> wall-clock**. **Idle is free**: with no input and nothing armed, the loop BLOCKS.

---

## 1. Goal & scope

**Goal.** Ship "the pomodoro slice": a plugin can **schedule** work on the wall clock (`wc.timer`,
guarded), **react to content settling** (`on_change`, debounced/observer-only), and **take an argument**
(parameterized commands) — driven and validated by a bare-bones `pomodoro.lua` demo, the P3 counterpart
to `insert_date.lua` (P1) and `wordcount.lua` (P2).

**Success demo.** `pomodoro.lua` registers a parameterized `pomodoro.start` (duration in minutes), which
arms a one-shot `wc.timer`; when it fires — *during genuine inactivity* — the observer-tier callback sets
`wc.status("Pomodoro: 25 min up")`; `pomodoro.cancel` cancels it; `plugin_list` shows the armed timer;
`plugins_reload` disarms it. A clock-driven e2e test advances the `Clock` to fire deterministically.

### In scope (P3)
- **`wc.timer` — guarded plugin timers** (§3): one-shot default, explicit `repeat`, 1000 ms floor, max 8
  per plugin, fired through the existing pump, observer-tier callback, one-pending-per-timer,
  reschedule-from-completion, `wc.timer.cancel`, auto-disarm on reload/failure/shutdown, and the airtight
  idle-free proof (§4).
- **`on_change` event** (§6): a P2-events-family member, debounced/content-settled (NOT per-keystroke),
  observer-only.
- **Parameterized commands** (§7): a plugin command may take a string argument; `wc.command(name, arg)`
  supplies it; palette/keybinding dispatch collects it via a `MinibufferKind::PluginArg` prompt.
- **New limits** (§9) + the resource/input audit (§8).
- **`pomodoro.lua`** demo + clock-driven e2e test (§11).

### NOT in P3 (deferred, each its own future effort)
- **Subprocess `wc.async`** (closed Rust primitive; driver = a formatter/linter plugin).
- **Plugin-contributed dynamic menu section** (`DYNAMIC_SECTIONS` → dynamic + `MenuRowAction` 3rd variant
  + Lua-at-menu-build).
- **Command-tier timer callbacks** (a timer that can edit/dispatch/open a modal) — see §3e; observer-tier
  is the P3 choice on safety grounds, command-tier is a clean future widening if a real plugin needs it.
- **Builtin set-value command conversion** (the contract's rule-10 collapse of N explicit-set builtins
  into one parameterized command) — P3 lays the parameterized-command groundwork but does not convert
  builtins.

---

## 2. Binding constraints restated (§5 — settled law)

- **Timers: one-shot by default; periodic is an explicit, honestly-framed exception.** `wc.timer(ms, fn)`
  fires once and disarms — edge-armed, self-clearing, inside the existing idle-free law. `wc.timer{ ms,
  repeat=true, fn }` is the genuinely-new wall-clock behavior: **a narrow, user-CONSENTED exception** the
  user opts into by installing and arming a plugin. **This spec does NOT claim "armed ≠ idle already
  fits"** (both reviewers rejected that: builtins self-DISARM and re-quiesce to `None`; a repeating timer
  self-perpetuates). It is framed as a scoped exception, made safe by the guardrails (§5) and provably
  zero-cost when unused (§4).
- **`next_wake` stays `None` at rest** — zero armed timers + no `on_change` subscriber ⇒ the fold result
  is identical to today (§4 proves it; existing guardrail tests pass unchanged).
- **`on_change` is NOT a hot-path hook** — debounced/content-settled, never per-keystroke; typing stays
  instant (§6).
- **`#![forbid(unsafe_code)]` holds; `wordcartel-core` stays Lua-free.**

---

## 3. `wc.timer` — the timer system

### a. Where the timer state lives (resolves grounding §5 open-point 3)

The authoritative armed-timer table lives **on the `Editor`**, not the host:

```rust
// editor.rs — new field beside pending_plugin_events (P2):
/// Armed plugin timers (P3). Schedule state + the VM-registry callback key. Lives on Editor (not
/// the host) because `timers::next_wake(&Editor, _)` must see the next-due to keep the loop's block
/// timeout correct — a bare `fn(&Editor,u64)->Option<u64>` deadline row cannot reach the host. Auto-
/// disarmed by `perform_reload`'s existing queue-clear block and the pump's null-host branch (the VM's
/// callback keys die with the VM regardless). Bounded by PLUGIN_MAX_TIMERS_PER_PLUGIN per origin.
pub pending_plugin_timers: Vec<crate::plugin::PluginTimer>,
/// Monotonic handle allocator for wc.timer (never reused, even across reload — a stale handle from a
/// torn-down plugin simply matches nothing on cancel).
pub next_timer_handle: u64,
```

```rust
// plugin/mod.rs — new:
/// One armed plugin timer. The callback lives in the VM named registry under `key` (dies with the
/// VM on reload — same discipline as hooks); this struct is the SCHEDULE half.
#[derive(Clone, Debug)]
pub struct PluginTimer {
    pub handle: u64,
    pub origin: String,        // owning plugin (for the per-plugin cap + plugin_list); from InvokeState.current
    pub key: String,           // "wc-timer-<handle>" — the VM-registry callback key
    pub next_due_ms: u64,      // wall-clock ms of the next fire
    pub interval_ms: u64,      // >= PLUGIN_TIMER_MIN_INTERVAL_MS (floor-checked at arm)
    pub repeat: bool,          // false = one-shot (remove after firing); true = reschedule-from-completion
    pub pending: bool,         // true while a fire's callback is in flight (one-pending-per-timer)
}
```

**Rationale vs. grounding's "store on the host like `hooks`."** The BINDING invariant is *auto-disarm on
teardown*, not the storage location. Editor-side storage preserves it identically (`perform_reload`
already clears the three plugin queues under one borrow — §6d of P2; P3 adds
`e.pending_plugin_timers.clear()` there, one line, same site), while being the ONLY option that lets the
`&Editor`-only `next_wake` deadline see the schedule without threading the host into `next_wake`/`on_tick`
(both of which take no host — verified: `timers::next_wake(editor, now)`, `on_tick(editor, ex, clock,
msg_tx)`). The callbacks still die with the VM (their keys are in the nulled `mlua::Lua`). This also
**avoids the grounding's forecast `SUBSYSTEMS: &'static [..]` → `Vec` upgrade** (§3c) — a strict
simplification.

### b. The `wc.*` API surface (callback-time; `plugin/api.rs`)

- **`wc.timer(interval_ms, fn) -> handle`** and **`wc.timer{ interval_ms=…, repeat=true|false, fn=… } ->
  handle`** — arm a timer, return an opaque integer handle. Installed in `install_editor_api` (a new
  `install_timer` beside `install_command`). The closure (it receives `lua`):
  1. **Observer check** (`bridge.invoke_state.borrow().observer`) → typed error `"plugin: wc.timer is
     not allowed from an event hook or a timer callback"`. This is the whole no-timer-spawns-timers +
     no-hook-arms-a-spin guarantee, reusing the P2 `InvokeState` mechanism (a hook AND a timer callback
     both run under `ObserverGuard(observer=true)` — §3e). Only a **command callback** (observer=false)
     can arm a timer.
  2. **Floor check** — `interval_ms < PLUGIN_TIMER_MIN_INTERVAL_MS (1000)` → typed error `"plugin: timer
     interval below the 1000 ms floor"`. (An integer arg — no allocation; the check is the spin defense.)
  3. Borrow the editor (`try_borrow_mut` → "editor busy" on nested re-entry, as every P2 edit closure).
  4. **Per-plugin cap** — `e.pending_plugin_timers.iter().filter(|t| t.origin == origin).count() >=
     PLUGIN_MAX_TIMERS_PER_PLUGIN (8)` → typed error `"plugin: timer limit reached (max 8)"`. `origin =
     invoke_state.current` (the running command's id, exactly like `wc.command`'s origin).
  5. Allocate a handle (`e.next_timer_handle += 1`), store `fn` under `key = format!("wc-timer-{handle}")`
     in the VM registry (`lua.set_named_registry_value(&key, fn)`), push a `PluginTimer { handle, origin,
     key, next_due_ms: now + interval_ms, interval_ms, repeat, pending: false }`. `now` comes from the
     bridge clock (`bridge.clock.now_ms()`). Return `handle`.
- **`wc.timer.cancel(handle)`** — installed as a field on the `wc.timer` table (so `wc.timer(…)` is
  callable AND `wc.timer.cancel(…)` exists — the table is a callable with a `cancel` method via a
  metatable `__call`, or `wc.timer` is a table `{ cancel = fn }` and arming is `wc.timer.arm(…)`; **decided:
  `wc.timer` is the arm function and `wc.timer_cancel(handle)` is the cancel function** — two plain
  functions, no metatable gymnastics; §12 records the rationale). Observer-checked the same way; removes
  the matching `PluginTimer` (by handle) from `e.pending_plugin_timers` and frees its registry key
  (`lua.set_named_registry_value(&key, mlua::Value::Nil)` — so a cancelled timer's callback does not linger
  in the registry until reload). Unknown handle → silent no-op (idempotent cancel).

Both are **callback-time only** (they need the live editor + clock via the bridge). Calling them at LOAD
time errors like the other editor APIs (`wc` has no `timer` field until `attach_bridge` runs). Calling
them from a hook or timer callback errors via the observer check.

### c. `timers.rs` upgrade — TWO static rows, no Vec, `on_tick` untouched (airtight)

`SUBSYSTEMS` stays a `&'static [TimedSubsystem]` of bare `fn` pointers. It gains **two** builtin rows —
both plain `fn(&Editor, u64) -> Option<u64>` reading editor mirror state:

```rust
// timers.rs — new rows appended to SUBSYSTEMS (builtins stay fn-ptr rows; NO Vec upgrade):

/// on_change debounce (P3 §6): the content-settled deadline, GATED on a subscriber so it is zero-cost
/// when no plugin uses on_change (proportional-to-work). Edge-armed by an edit (like reconcile), self-
/// clearing on fire — stays inside the idle-free law.
fn on_change_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if e.has_on_change_subscriber { e.on_change_due } else { None }
}

/// Plugin-timer deadline (P3 §3): the soonest NON-pending armed timer's next-due. `None` when no timer
/// is armed (idle-free preserved) — see §4's proof. A `pending` timer is excluded (its callback is in
/// flight — the one-pending-per-timer rule; the same in-flight-gate shape as the builtin swap/diag/
/// reconcile rows). NOTE: a due timer's next-due may be in the past, so this can be < `now`; `run` uses
/// `saturating_sub(now)` → one immediate wake, then the pump fires + reschedules to a future due (§4).
fn plugin_timer_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.pending_plugin_timers.iter().filter(|t| !t.pending).map(|t| t.next_due_ms).min()
}
```

```rust
pub(crate) static SUBSYSTEMS: &[TimedSubsystem] = &[
    // … the 8 existing builtins, UNCHANGED …
    TimedSubsystem { name: "on_change",     deadline: on_change_deadline },
    TimedSubsystem { name: "plugin_timer",  deadline: plugin_timer_deadline },
];
```

**`next_wake` and `on_tick` are otherwise untouched.** `next_wake` folds the 10 rows' min (unchanged
body). **`on_tick` gains NO plugin-timer dispatch** — timer FIRING moves to the pump (§3d), which is the
only site with the host VM + the editor handle + the clock all at once (`on_tick` has no host). This is
strictly more consistent with §5's binding "fires through the EXISTING pump" than generalizing `on_tick`
would be, and it means `on_tick`'s hand-written dispatch (the piece the grounding feared needing a second
generalization pass) is **not touched at all**.

### d. Firing + rescheduling — a pump phase (`plugin/pump.rs`)

The pump (`pump(&mut self, editor, reg, ex, clock, msg_tx)`) already runs once per loop iteration after
`reduce`, holds the host VM (`self.lua`), the editor handle, and a `&dyn Clock`. P3 adds **Phase 0 — the
timer-fire pass — at the top of `pump`, before the P2 re-drain loop:**

> **`pending` invariant (Codex Critical 1 — no cap-stranding).** A timer is `pending` ONLY while its
> callback is genuinely in flight. A cap-trip mid-batch must never leave a due timer `pending` without
> having fired — that would exclude it from `plugin_timer_deadline` AND the fire-set forever (stranded:
> never wakes the loop, never reschedules). Therefore `pending` is set **mark-then-fire atomically, per
> timer, immediately before invoking THAT timer's callback** — NOT for the whole due set up front. A
> cap-trip leaves every not-yet-reached due timer **un-marked and still due**, so the next pump
> reconsiders it.

```
Phase 0 (timer fire): now = clock.now_ms();
  under ONE short borrow, snapshot the due HANDLES (no `pending` mutation yet):
    due = editor.pending_plugin_timers.iter().filter(|t| !t.pending && now >= t.next_due_ms)
                                       .map(|t| t.handle).collect();   // handles only — state re-read per timer
  drop the borrow. Then, with NO outer borrow held, for EACH handle in `due` (in order):
    if cap_tripped(units, start, editor) { return; }   // STOP — the remaining due timers are NEVER
                                                        //  marked pending (still due next pump); no strand.
    units += 1;                                          // shares the P2 cycle caps
    { // mark-then-fire ATOMICALLY, under a short borrow:
      look up the timer by handle; if it was cancelled/removed mid-batch, skip (continue).
      set THIS timer's `pending = true` and read its `key`.   // pending set ONLY here, immediately pre-fire
    } // drop the borrow
    invoke the callback (key) under ObserverGuard(observer=true) + panicx::catch + with_time_guard(
        CALLBACK_TIME_BUDGET) — the SAME shape as invoke_hook (§3e observer-tier).
    on completion (whether Ok or a caught error), under a short borrow, look up the timer by handle:
      if it still exists (not cancelled mid-callback) and repeat:
          next_due_ms = clock.now_ms() + interval_ms;  pending = false;   // FROM COMPLETION, not the tick
      else: remove it from editor.pending_plugin_timers (one-shot, or repeat-but-cancelled) AND free its
            registry key (`set_named_registry_value(key, Nil)`).
```

Then the existing P2 re-drain loop runs (dispatches/calls/events). A timer callback is observer-tier so it
cannot enqueue dispatches/calls/timers — it contributes at most `≤ (armed timers, ≤ 8·N)` units and cannot
cascade. This satisfies every §5 guardrail:
- **one-pending-per-timer, no strand:** `pending` is set only in the mark-then-fire step immediately
  before a callback and cleared on that callback's completion (or the timer is removed) — so a timer is
  `pending` iff its callback is in flight. A cap-trip returns before marking the not-yet-reached due
  timers, which stay `!pending && now >= next_due` and fire on the next pump. **A cap-trip cannot orphan a
  due timer.**
- **reschedule-from-completion:** `next_due_ms = now_after_callback + interval_ms` — a long callback or a
  busy loop never produces a burst of catch-up fires (missed ticks are dropped, not accumulated).
- **pump-budgeted:** each fire counts against `PLUGIN_PUMP_CHAIN_CAP (64)` + `PUMP_CYCLE_TIME_BUDGET
  (500 ms)`; each callback is bounded by `CALLBACK_TIME_BUDGET (150 ms)`.

**The wake path (how a resting loop learns a timer is due):** `next_wake` returns
`plugin_timer_deadline`'s min → the loop's `recv_timeout(timeout)` blocks that long → on timeout it gets
`Msg::Tick` → `reduce(Tick)` (builtin `on_tick` work) → `pump` Phase 0 fires the now-due timer. **Anti-spin
(softened per Codex Important 1):** `app.rs::run` computes the timeout as `deadline.saturating_sub(now)`,
so a deadline that is *already due* legitimately yields **one** `recv_timeout(0)` — the loop wakes
immediately, the pump fires the timer, and (repeat) reschedules it to a FUTURE deadline `>= now + 1000 ms`.
So the claim is NOT "never `recv_timeout(0)`" but: **a due timer yields at most one zero-timeout wake, then
fires and reschedules from completion, so a floored repeating timer wakes at most ~once per interval and
cannot spin** — which holds precisely because Critical 1's fix guarantees the fire actually happens (a
stranded `pending` timer would defeat it by never rescheduling). See §4.

### e. Timer callback TIER — observer-tier (resolves grounding §5 open-point 1)

**A timer callback runs under `ObserverGuard(observer=true)` — identical to a hook.** It may **read**
(`wc.text`/`selection`/`cursor`/`len`/`version`/`path`) and **emit `wc.status`**; it may **NOT** edit
(`wc.insert`/`replace`/`set_selection`), dispatch (`wc.command`), or arm/cancel timers (`wc.timer`/
`wc.timer_cancel`) — all gated by the existing observer check.

Two independent reasons this is the correct minimal choice (not merely "enough for the demo"):
1. **No-data-loss.** A command-tier timer could **autonomously edit the document on a wall-clock fire while
   the user is away** — a background mutation with no user action behind it, the exact class the
   no-data-loss law exists to prevent. Observer-tier makes an away-from-keyboard timer physically unable to
   change the document. This is a *safety* argument, not just a scoping one.
2. **No timer-spawns-timers.** Observer mode blocks `wc.timer`, so a timer callback cannot arm another
   timer to build a spin — for free, via the same mechanism.

The bare-bones pomodoro (a status notification) is fully served: `wc.status("Pomodoro: 25 min up")`.
**Limitation, stated honestly:** an observer-tier timer cannot open a "take a break" modal, dispatch a
command, or chain a work→break phase from the callback. Chaining is covered by `repeat=true` (a recurring
tick) or by the user re-dispatching `pomodoro.start`; a richer command-tier timer is a **deliberate future
widening** (guarded so it still cannot arm timers past the per-plugin cap). This is a resolved product call
on safety grounds — see the summary's heads-up, not a `⚠OPEN`.

### f. Guardrails summary (all from §5)

| Guardrail | Mechanism | Enforced where |
|---|---|---|
| One-shot default | `repeat=false` unless explicitly set | `wc.timer` parse (§3b) |
| Explicit `repeat` | `wc.timer{ repeat=true }` | `wc.timer` parse |
| Min-interval floor 1000 ms | typed error < floor | `wc.timer` call (§3b step 2) |
| Max 8 armed / plugin | typed error over cap | `wc.timer` call (§3b step 4) |
| Auto-disarm on reload/failure/shutdown | `pending_plugin_timers.clear()` + VM null drops keys | `perform_reload` step 5; pump null-host branch (§3g) |
| Fires through the pump, budgeted | Phase 0 + the P2 caps | `pump` (§3d) |
| One pending per timer | `pending` flag excludes from fire-set + next_wake | `pump` Phase 0 (§3d) |
| Reschedule from completion | `next_due = now_after_callback + interval` | `pump` Phase 0 (§3d) |
| Cancellable | `wc.timer_cancel(handle)` | `plugin/api.rs` (§3b) |
| Visible | armed count in `plugin_list` | `registry.rs` (§10) |

### g. Auto-disarm sites — BOTH subsystems (Codex Critical 2: timers AND on_change)

Every teardown path must reset **both** new subsystems to their no-plugin baseline — not just the timer
schedule. `on_change` is a wake source too: a stale `has_on_change_subscriber` OR a leftover
`on_change_due` after a plugin is gone would keep `on_change_deadline` firing wakes for a dead subsystem
(the swap-thrash class). So define **one reset the every path calls:**

```rust
// A single helper (plugin::reload or editor method) — the whole-plugin-subsystem "back to no-plugins":
fn clear_plugin_wake_state(e: &mut Editor) {
    e.pending_plugin_timers.clear();      // timer schedule
    e.has_on_change_subscriber = false;   // on_change subscription
    e.on_change_due = None;               // any armed on_change deadline
    // (the P2 queue clears — pending_plugin_calls/events/dispatch — stay where they already are)
}
```

Called from every teardown path:
- **`plugins_reload`** — `perform_reload` (`plugin/reload.rs`) already nulls the VM (all `wc-timer-*`/
  `wc-ev-*` keys die) and clears the three plugin queues under one borrow; P3 calls
  `clear_plugin_wake_state(e)` in that SAME block. Then the rebuild re-attaches the bridge, which
  RE-SETS `has_on_change_subscriber` from the fresh host's hooks (§5) and the reloaded plugin re-arms its
  timers — so a plugin that still subscribes/arms is restored, and one that no longer does is left at the
  baseline. `e.next_timer_handle` stays monotonic (a stale handle matches nothing).
- **Load-failure / VM-exhaustion fatal path** — `perform_reload` reverts the whole subsystem (null host +
  `retain_builtins` + queue clear); `clear_plugin_wake_state(e)` runs in the same block, and because the
  host ends null, `attach_bridge` never re-sets `has_on_change_subscriber` → it stays `false`. At
  *startup* the editor has no armed timers/subscription yet (nothing to clear).
- **Bridge-attach failure** — if `attach_bridge` returns `Err` during a reload, the host is left without a
  bridge and `has_on_change_subscriber` is NOT re-set → the `clear_plugin_wake_state` baseline stands
  (false/empty). No stale wake.
- **Null-host / `--no-plugins` pump branch** — the P2 pump's null-host early-return clears the three
  queues; P3 calls `clear_plugin_wake_state(e)` there too, so under `--no-plugins` (or after a reload to
  null) both subsystems sit at baseline and `next_wake == None`.
- **Shutdown** — the loop exit drops the editor; nothing can fire after the loop ends.

**Cross-reload invariant (restated):** after ANY teardown, both the timer schedule AND the on_change
subscription are empty ⇒ `plugin_timer_deadline == None` and `on_change_deadline == None` ⇒ `next_wake`'s
result is identical to a no-plugins build. A plugin that survives reload has both re-established by the
rebuild; a plugin that's gone leaves neither behind.

---

## 4. The idle-free proof (airtight — this is where the review pushes hardest)

**Claim.** With zero armed plugin timers and no `on_change` subscriber, `next_wake`'s RESULT is `None`
whenever it is `None` today; the existing guardrail tests pass unchanged.

**Precise statement (not "byte-identical slice" — the slice honestly gains 2 rows; the RESULT is
identical).** `next_wake = SUBSYSTEMS.iter().filter_map(deadline).min()`. P3 appends exactly two rows:
- `on_change_deadline(e,_) = if e.has_on_change_subscriber { e.on_change_due } else { None }`. With no
  `on_change` plugin loaded, `has_on_change_subscriber == false` ⇒ **always `None`**.
- `plugin_timer_deadline(e,_) = e.pending_plugin_timers.iter().filter(!pending).map(next_due).min()`. With
  no timer armed, `pending_plugin_timers` is empty ⇒ **`None`**.

So for any editor state in which the 8 builtins fold to `None` (settled/no-plugin — what
`next_wake_none_when_settled` pins), the two new terms are also `None`, and `min` over `{None×10} = None`.
The result equals today's for every pre-P3 case. **Consequences:**
- `next_wake_none_when_settled` (a fresh no-plugin editor) — passes: both new rows `None`.
- `gated_subsystems_yield_none` — passes: it selects builtins by name and iterates asserting the *named*
  ones are `None`; the two new rows are neither named in its set nor break its loop.
- A plugin that registers only commands/hooks (no timer, no on_change) — `next_wake == None` at rest
  (both new rows `None`). **New guardrail test required.**

**When the exception is active (a plugin armed a timer / subscribed to on_change): the departure from
idle-free is exactly scoped to that plugin and provably NOT a spin (softened per Codex Important 1).**
- An armed repeating timer ⇒ `plugin_timer_deadline == Some(next_due)`, `next_wake == Some(next_due)`, the
  loop blocks `next_due.saturating_sub(now)`, wakes, fires (Phase 0), reschedules
  `next_due = now_after_callback + interval (>= 1000 ms)`. **The claim is NOT "never `recv_timeout(0)`"** —
  `saturating_sub` legitimately yields `0` ONCE when a deadline is already due (the loop then wakes
  immediately and fires). The real guarantee: **each fire pushes `next_due` at least 1000 ms into the
  future from completion, so a floored repeating timer produces at most ~one wake per interval and cannot
  spin.** This holds ONLY because Critical 1's mark-then-fire fix guarantees the fire actually happens — a
  stranded `pending` timer would never reschedule and could sit at a past `next_due`, but it is EXCLUDED
  from `plugin_timer_deadline` (`!pending` filter) so it would produce *no* wake at all rather than a spin;
  the fix ensures it is never stranded in the first place.
- Cancel / reload ⇒ `clear_plugin_wake_state` empties `pending_plugin_timers` (and the on_change
  subscription) ⇒ both new rows `None` ⇒ `next_wake` back to `None` (free at rest). **New guardrail test.**
- on_change: `on_change_due` is armed only in `advance`'s existing reconcile-debounce block (§6), under
  the same version-latch (idle Ticks never push it), and only when `has_on_change_subscriber`; it
  self-clears on fire. So an on_change plugin adds a wake ~150 ms after typing stops and then returns to
  `None` — edge-armed, self-clearing, inside the law.

**New guardrail tests (§5 list), all in `timers.rs`/`plugin` tests:**
1. `next_wake_none_with_commands_only_plugin` — a plugin with commands/hooks but no timer/on_change ⇒
   `next_wake == None` at rest.
2. `armed_repeating_timer_wakes_then_cancel_returns_to_none` — arm ⇒ `Some`; cancel ⇒ `None`.
3. `teardown_clears_both_subsystems_next_wake_none` — arm a timer AND subscribe on_change; `perform_reload`
   to null (or `--no-plugins`) ⇒ `pending_plugin_timers` empty, `has_on_change_subscriber == false`,
   `on_change_due == None` ⇒ `next_wake == None` (Critical 2's cross-reload invariant).
4. `floored_repeating_timer_bounded_wakes_not_a_spin` — advance a `TestClock` over N seconds driving a
   1000 ms repeating timer through `pump`; assert it fires **≤ N+1 times** (at most one immediate
   zero-timeout wake plus one per interval) and every post-fire `next_due - now >= 1000` — a bounded
   cadence, not a spin.
5. `on_change_deadline_none_without_subscriber` — `on_change_due` set but `has_on_change_subscriber ==
   false` ⇒ `None`; flip the flag ⇒ `Some`.
6. `cap_trip_does_not_strand_a_due_timer` (Critical 1) — arm >`PLUGIN_PUMP_CHAIN_CAP` worth of work so a
   pump cycle trips the cap mid-fire-batch; assert every not-yet-fired due timer is still `!pending` and
   `plugin_timer_deadline` still returns its due time (it fires on the next pump), never stranded.

---

## 5. `has_on_change_subscriber` — the subscriber gate

A cached `bool` on `Editor`, recomputed whenever the hook set changes:

```rust
// editor.rs:
/// True iff some loaded plugin registered a `wc.on("change", …)` hook. Gates `on_change_deadline` so
/// on_change costs ZERO wake when no plugin uses it (proportional-to-work). Recomputed at bridge-attach
/// and after every reload from the host's committed hooks.
pub has_on_change_subscriber: bool,
```

**Set** in `PluginHost::attach_bridge` (and thus on every reload, which re-attaches): after the hooks are
committed, `editor.has_on_change_subscriber = self.hooks.iter().any(|h| h.kind ==
PluginEventKind::Change)`. `attach_bridge` already holds the editor handle and the host's `hooks`; this is
one line. On a null host / `--no-plugins`, `attach_bridge` early-returns without setting it.

**Cleared** — critically — on EVERY teardown via `clear_plugin_wake_state` (§3g): `perform_reload`, the
VM-exhaustion fatal path, a bridge-attach failure, and the null-host pump branch all reset it to `false`
(alongside `on_change_due = None` and the timer schedule). So a reload that removes the on_change plugin,
or a reload-to-null, leaves the flag `false` and `on_change_deadline` at `None` — no dead subsystem keeps
waking the loop (Codex Critical 2). Only a rebuild that RE-attaches a bridge with a live on_change hook
re-sets it to `true`.

---

## 6. `on_change` — the debounced content-settled event

A new member of the P2 event family (reuses the entire `wc.on` / `fire_event` / pump-event-drain
machinery). Grounding §5 open-point 2 resolved: **its own edge-armed, version-latched deadline, armed
alongside the reconcile debounce, gated on a subscriber** — NOT a per-keystroke hook.

- **`PluginEventKind::Change`** added (exhaustive enum + `event_from_str("change")` + `kind_str`). `wc.on`
  gains no new code — it already parses via `event_from_str` and caps at `PLUGIN_MAX_HOOKS_PER_PLUGIN`.
- **`editor.on_change_due: Option<u64>`** (new field). **Armed** in `advance` (`app.rs:401-409`), inside
  the SAME `if b.reconcile.maybe_stale && … && (due_at.is_none() || armed_for_version != version)` block
  that arms the reconcile debounce — so it is version-latched identically (an idle `Tick` cannot push it
  forever; a real edit that advances the version arms it). Only armed when
  `editor.has_on_change_subscriber` (else the field stays `None` and the deadline is `None`). Value =
  `now + RECONCILE_DEBOUNCE_MS (150)` — reuses the existing content-settle debounce, so on_change fires at
  the same "typing stopped, content settled" moment reconcile does. (Arming an `Editor`-level field from
  inside a `let b = editor.active_mut()` borrow requires computing the value then setting `editor.on_change_due`
  after the buffer borrow drops — a 3-line restructure of the block, noted for the plan.)
- **Fired** in `on_tick` (which runs in `reduce`'s Tick arm): when `has_on_change_subscriber` and
  `on_change_due` is Some and `now >= on_change_due`, clear `on_change_due` and
  `fire_event(editor, PluginEventKind::Change, editor.active().document.path.as_deref())`. The pump's
  existing event-drain phase delivers it to the on_change hooks (observer-only, dropped if no hook —
  the P2 `event_with_no_hooks_is_dropped` path). One `on_tick` branch (~4 lines) — `on_tick` grows by a
  row, not bulk.
- **Why it can't fire per-keystroke:** the 150 ms debounce re-arms on each edit-version bump, so it only
  elapses 150 ms after the LAST keystroke — the exact coalescing the builtin reconcile/diagnostics
  deadlines already use. Typing never invokes a hook (the hot path is untouched; on_change fires from the
  cold `Tick` path after settle). This honors the P2 decomposition's ban on `on_key`/`on_edit`.
- **Payload:** `{ kind = "change", path = <active path>|nil }` — the P2 event table, path clamped by
  `fire_event`'s existing `PLUGIN_MAX_EVENT_PAYLOAD`.

Multi-buffer note: `on_change_due` is editor-level, fired for whatever buffer is active at settle time; the
hook reads `wc.text()` = the active buffer. A rare edit-A-then-switch-to-B-before-150 ms race fires
on_change for B — acceptable imprecision (documented); the alternative (per-buffer arming) is deferred
until a plugin needs it.

---

## 7. Parameterized commands — a plugin command that takes an argument

The widening (grounding §4), scoped to plugin commands (pomodoro: "start with duration N"):

### a. Data-model changes
- **`registry.rs` `CommandMeta`** gains `arg: Option<&'static str>` — `Some(prompt)` marks the command
  parameterized (the prompt is interned like `label`; `None` = nullary, every existing command). `Copy`
  is preserved (`&'static str` is Copy).
- **`plugin/mod.rs` `PluginCall`** gains `arg: Option<String>` → **loses `Copy`** (owns a `String`).
  `PluginDispatch` gains `arg: Option<String>` (already non-Copy). These carry the collected/supplied arg
  to the pump.
- **`minibuffer.rs` `MinibufferKind`** gains `PluginArg { id: CommandId }` — **stays `Copy`** (`CommandId`
  is Copy; the prompt reuses the existing `Minibuffer.prompt: String` field, so no `String` enters the
  enum). Its `Enter` submit arm: `MinibufferKind::PluginArg { id } => { let arg = mb.text; /* cap-checked
  */ editor.pending_plugin_calls.push_back(PluginCall { id, arg: Some(arg) }); }`.

### b. Dispatch paths
- **`registry.rs` `dispatch`'s `Plugin` arm** branches on `self.meta(id).arg`:
  - `None` (nullary) → today's behavior: `push PluginCall { id, arg: None }`.
  - `Some(prompt)` (parameterized) AND no arg is being supplied at this dispatch → open the arg minibuffer:
    `ctx.editor.minibuffer = Some(Minibuffer { prompt: prompt.into(), text: "", cursor: 0, kind:
    MinibufferKind::PluginArg { id } })`. (Consistent with `SaveAs`/`Filter` commands that open a
    minibuffer; `dispatch` already mutates `ctx.editor`.) `registry.rs` stays Lua-free (no `mlua`).
- **`wc.command(name, arg?)`** (plugin→command) — supplies the arg directly: enqueue
  `PluginDispatch { origin, name, arg: Some(arg) }`; the pump's `drain_one_dispatch` resolves the name and
  pushes `PluginCall { id, arg }` (bypassing the minibuffer — the plugin already has the arg).
- **`pump.rs` `invoke_call`** passes the arg to the Lua callback: `cb.call::<()>((call.arg,))` — the arg
  becomes the callback's first parameter (`fn = function(arg) … end`; a nullary command's `fn = function()
  … end` simply ignores the extra nil, which Lua permits).

### c. `wc.register_command`
Gains an optional `arg` field: `wc.register_command{ name, label, menu, fn, arg = "Minutes:" }`. Parsed in
`install_registration` (borrowed-length-check-then-convert on the prompt string, cap
`PLUGIN_MAX_LABEL_LEN`), interned into `CommandMeta.arg` at commit (the two-phase commit path). Absent →
`None` (nullary).

### c2. `CommandMeta.arg` constructor + registration/commit migration — COMPLETE (Codex Important 3)
Adding `arg: Option<&'static str>` to `CommandMeta` (`registry.rs:84`) forces every `CommandMeta { .. }`
constructor and the plugin-command registration/commit plumbing that must now carry a per-command prompt
from `wc.register_command` to `register_plugin`. From `grep -rn "CommandMeta {" wordcartel/src` +
`register`/`register_plugin` + `PendingReg`/commit-tuple (verified 2026-07-12):

- **`registry.rs` `CommandMeta` constructors (3):** `:110` (`register` — builtins, → `arg: None`), `:118`
  (`register_stateful` — → `arg: None`), `:131` (`register_plugin` — → `arg` from its new param). Builtins
  are all nullary, so `register`/`register_stateful` signatures are UNCHANGED (they hard-code `arg: None`
  in the literal); only `register_plugin` grows a param.
- **`registry.rs::register_plugin` (`:124`)** signature grows: `register_plugin(&mut self, id: CommandId,
  label: &'static str, menu: Option<MenuCategory>, arg: Option<&'static str>) -> Result<(), RegisterError>`.
  All **12** callers (`grep -rn "register_plugin(" wordcartel/src`, verified 2026-07-12): the sole
  production caller = `plugin/load.rs`'s commit loop (`:345`, below — passes the new `arg`); the doc
  example (`registry.rs:143` → pass `None`); and **10 test call sites** in `registry.rs` (`:1019`,
  `:1029`, `:1034`, `:1036`, `:1045`, `:1065`, `:1066`, `:1088`, `:1091`, `:1113` → each pass `None`).
  Every non-commit-loop caller passes `None`.
- **`plugin/host.rs::PendingReg` (`:26`)** gains `pub arg: Option<String>` (the raw prompt, staged like
  `name_full`/`label` — cap-checked, NOT yet interned). Constructed at **`plugin/api.rs:81`**
  (`install_registration` → parse `spec.arg` and add the field).
- **`plugin/load.rs` commit plumbing:** the commit tuple `committed: Vec<(CommandId, &'static str,
  Option<MenuCategory>)>` (`:311`) → `Vec<(CommandId, &'static str, Option<MenuCategory>,
  Option<&'static str>)>` (add the interned arg); the phase-1 push `committed.push((id, label, p.menu))`
  (`:323`) → `committed.push((id, label, p.menu, p.arg.as_deref().map(crate::plugin::intern)))` (intern
  the prompt only on the committed survivor — same intern-on-commit discipline as name/label, §7b of P2);
  the phase-2 loop `for (id, label, menu) in committed { reg.register_plugin(id, label, menu) }` (`:344`)
  → `for (id, label, menu, arg) in committed { reg.register_plugin(id, label, menu, arg) }`.
- The compiler forces every one; the plan re-greps.

### d. Arg cap (resource-bound LAW)
The collected/supplied arg is a plugin/user string entering an owned `PluginCall.arg: String`. New cap
`PLUGIN_MAX_COMMAND_ARG (4096 bytes)`, checked BEFORE the owning allocation at BOTH entry points:
`wc.command`'s borrowed `mlua::String` bytes, and the `MinibufferKind::PluginArg` submit (`mb.text.len()`
— over-cap → typed status error, nothing enqueued). Same pattern as `wc.command`'s `PLUGIN_MAX_COMMAND_REF`.

### e. Migration (PluginCall loses `Copy`) — COMPLETE grep-anchored list (Codex Important 2)
`PluginCall` currently derives `Clone, Copy, Debug, PartialEq, Eq` (`plugin/mod.rs:26`). Adding
`arg: Option<String>` keeps `Clone, Debug, PartialEq, Eq` (so the `assert_eq!(…, PluginCall { id })` tests
still compile with `arg: None`) but **drops `Copy`**. `PluginDispatch` (already non-Copy) gains
`arg: Option<String>`. Every site, from `grep -rn "PluginCall\|PluginDispatch" wordcartel/src`
(verified 2026-07-12) — each `PluginCall { id … }` → `{ id, …, arg: None }` unless it supplies an arg:

- **Production (3):** `registry.rs:746` (`dispatch` `Plugin` arm → `{ id, arg: None }`); `plugin/pump.rs`
  — `invoke_call(&self, editor, call: PluginCall)` at `:197` (now reads `call.arg`; `call` is passed by
  value — fine, non-Copy move) and `drain_one_dispatch(… d: &PluginDispatch)` at `:179` (reads `d.arg`,
  pushes `PluginCall { id, arg: d.arg.clone() }`); `plugin/api.rs:437` (`install_command` constructs
  `PluginDispatch { origin, name, … }` → add `arg`).
- **`registry.rs` tests (2):** `:1056`, `:1104` (`assert_eq!(e.pending_plugin_calls[0], PluginCall { id })`
  → `{ id, arg: None }`; `PartialEq` retained so these still work).
- **`plugin/host.rs` tests (15):** `PluginCall { id … }` at `:257, :420, :433, :530, :555, :613, :622,
  :744, :765, :860, :887, :915, :939, :1024, :1064` → each `{ id, arg: None }`; plus the `PluginDispatch`
  at `:746` → add `arg: None`.
- **`plugin/reload.rs` tests (4):** `PluginCall` at `:258`, `:359` and `PluginDispatch` at `:261`, `:362`
  → add the field.
- **`e2e.rs`:** any `PluginCall`/`PluginDispatch` constructor the compiler flags (P2 e2e journeys) → add
  the field. (The field type `editor.pending_plugin_calls: VecDeque<PluginCall>` at `editor.rs:398` and
  `pending_plugin_dispatch` at `:406` are unchanged.)

The plan re-greps at execution time; the compiler forces completeness on every one.

---

## 8. Resource-bound + input-validation audit (every new plugin-input site)

**Input-validation LAW:** P3 adds NO offset/range API — inherited unchanged (`plugin_check_range` still
the sole chokepoint). New inputs are integers/strings/functions — audited below.

| # | New plugin input | Crosses into | Bound (before the allocation/effect) |
|---|---|---|---|
| 1 | `wc.timer` interval (integer) | a wall-clock wake | `PLUGIN_TIMER_MIN_INTERVAL_MS (1000)` floor → typed error below it (spin defense). No ceiling needed — an integer, not an allocation; a far-future due is one harmless `min` term. |
| 2 | `wc.timer` count | `PluginTimer` on the editor + one VM-registry callback key | `PLUGIN_MAX_TIMERS_PER_PLUGIN (8)` per origin → typed error at call; each timer is `O(1)` state + one bounded callback (dies with the VM at reload). |
| 3 | `wc.timer` callback (function) | VM named registry (`wc-timer-<handle>`) | bounded by the count cap (2); freed on cancel/reload. |
| 4 | timer callback execution | editor state | mechanically **observer-only** (§3e) — edits/`wc.command`/`wc.timer` rejected via `InvokeState.observer`; no autonomous background mutation; status writes share `PLUGIN_MAX_STATUS_LEN`. |
| 5 | timer firing (cascade) | main-thread time | per-fire `CALLBACK_TIME_BUDGET (150 ms)`; per-cycle `PLUGIN_PUMP_CHAIN_CAP (64)` + `PUMP_CYCLE_TIME_BUDGET (500 ms)`; `pending` flag → one in flight per timer. Observer-tier ⇒ a timer cannot enqueue → cannot self-cascade. |
| 6 | parameterized-command arg (string) | owned `PluginCall.arg: String` | `PLUGIN_MAX_COMMAND_ARG (4096)` on the borrowed `mlua::String` bytes (`wc.command`) AND on `mb.text` (minibuffer submit) — before the owning allocation. |
| 7 | `wc.register_command{ arg = "…" }` prompt | interned `CommandMeta.arg: &'static str` (permanent) | borrowed-length cap `PLUGIN_MAX_LABEL_LEN` before intern (the two-phase commit path — §7b of P2). |
| 8 | `on_change` event name / payload | the P2 event queue | `event_from_str` parse-to-enum (the enum is the bound); payload path clamped by `PLUGIN_MAX_EVENT_PAYLOAD` in `fire_event`. Fired by the HOST, debounced + subscriber-gated (§6) — no unbounded arming. |

Error routing (all rows): typed `mlua::Error` → `pcall`-able; uncaught → `plugin_error` → status line.
Never a panic, never a console write, never a blocked input loop, never an autonomous document edit.

---

## 9. New limits (`limits.rs`)

```rust
/// P3 plugin-timer + parameterized-command caps.
/// Min timer interval — the spin defense: a repeating timer reschedules to `now + interval >= now +
/// 1000ms` from completion, so it wakes at most ~once/interval (a due deadline may yield ONE immediate
/// zero-timeout wake, then fires and moves 1s+ into the future — bounded cadence, not a spin). Sub-floor
/// → typed error.
pub const PLUGIN_TIMER_MIN_INTERVAL_MS: u64 = 1000;
/// Max armed timers per plugin (heavier than a hook — each keeps a wall-clock wake alive). Over → typed error.
pub const PLUGIN_MAX_TIMERS_PER_PLUGIN: usize = 8;
/// Max bytes of a parameterized-command argument (wc.command arg / the PluginArg minibuffer line),
/// checked before the owning String allocation (resource-bound LAW).
pub const PLUGIN_MAX_COMMAND_ARG: usize = 4096;
```

`ON_CHANGE` reuses the existing `reconcile::RECONCILE_DEBOUNCE_MS (150)` — no new debounce constant.
`plugin_caps_are_sane` extends over the three new constants.

---

## 10. Command-surface-contract conformance

- **Law 1 (registry = single source of truth).** Parameterized plugin commands are ordinary `Registry`
  entries (one entry, `CommandMeta.arg = Some(prompt)`); arg collection routes through the shared
  `Minibuffer` seam (the same seam `SaveAs`/`Filter` use), never a parallel store. `wc.command(name,arg)`
  routes through `reg.resolve_name` + `reg.dispatch`. Timers/`on_change` are host↔plugin data flow, not
  command-surface actors (like P2 events/config).
- **Law 2 (every user-settable option is a command).** P3 adds no `SettingsSnapshot` option. `wc.timer`/
  `wc.timer_cancel` are Lua APIs, not registry commands — N/A. The recurrence-guard test is unaffected.
- **Law 3 (palette exhaustive).** A parameterized plugin command appears in the palette by derivation
  (it is a registered command). Selecting it from the palette opens its arg minibuffer, then dispatches —
  no palette code change; the palette-completeness test still holds over the registry.
- **Law 4 (menu ⊆ palette).** A parameterized command tagged with a `MenuCategory` appears in that menu
  (it is a registered command); the menu row activates it (opening the arg prompt) exactly as the palette
  does. Subset holds by derivation.
- **Rule 10 (parameterized set-value commands are the Effort-P concern).** P3 realizes rule 10's
  argument-carrying command shape for PLUGIN commands, keeping set-value semantics clean — the exact
  groundwork the contract anticipated ("keep set-value semantics clean so P can later collapse the N
  explicit-set commands into one parameterized command"). Builtin conversion is deferred (§1).
- **`plugin_list` surfaces armed timers (Codex Important 4 — concrete).** The existing `plugin_list`
  builtin handler (`registry.rs:725`) today reads only `c.editor.plugin_inventory` (commands/hooks). P3
  adds a READ of the new `c.editor.pending_plugin_timers` field and shows the armed-timer total. Exact
  handler body:
  ```rust
  r.register("plugin_list", "List Plugins", Some(MenuCategory::Settings), |c| {
      let inv = &c.editor.plugin_inventory;
      let ok = inv.iter().filter(|r| r.error.is_none()).count();
      let failed = inv.len() - ok;
      let cmds: usize = inv.iter().map(|r| r.commands).sum();
      let hooks: usize = inv.iter().map(|r| r.hooks).sum();
      let timers = c.editor.pending_plugin_timers.len();          // P3: the live armed-timer count
      c.editor.status = format!(
          "plugins: {ok} ok ({cmds} cmds, {hooks} hooks, {timers} timers), {failed} failed");
      CommandResult::Handled
  });
  ```
  A display change to an existing command — conformant (laws 1/3/4 unchanged); it satisfies §5's "armed
  timers visible" consent-symmetry (the user can see, and via `pomodoro.cancel`/`plugins_reload` stop, a
  plugin keeping the loop awake).
- **No contract amendment required.**

---

## 11. `pomodoro.lua` demo + clock-driven e2e test

**`wordcartel/tests/fixtures/plugins/pomodoro.lua`** (bare-bones, observer-safe):
```lua
-- pomodoro.lua — P3's success-criterion demo: a parameterized start command that arms a one-shot
-- wc.timer whose observer-tier callback notifies via wc.status when the session elapses DURING
-- inactivity. Default duration from [plugins.config.pomodoro] minutes, overridable by the command arg.
local default_min = (wc.config and wc.config.minutes) or 25
local armed = nil  -- the current session's timer handle (module-local; captured at load)

wc.register_command{
    name = 'start', label = 'Pomodoro: Start', menu = 'View', arg = 'Minutes (blank = default):',
    fn = function(arg)
        local minutes = tonumber(arg) or default_min
        if armed then wc.timer_cancel(armed) end            -- restart replaces the prior session
        armed = wc.timer(minutes * 60 * 1000, function()    -- one-shot; observer-tier callback
            wc.status(string.format('Pomodoro: %d min session complete', minutes))
            armed = nil
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
(Note: `wc.timer`/`wc.timer_cancel` are called from a COMMAND callback — observer=false — so they are
permitted; the TIMER callback only calls `wc.status` — observer=true — which is allowed. This exercises
exactly the tier boundary §3e specifies.)

**Clock-driven e2e test** (`e2e.rs`, mirroring `wordcount_lua_e2e_success_demo` + the timer-test idiom of
advancing a `TestClock`): load `pomodoro.lua` via `load_phase` with `[plugins.config.pomodoro] minutes =
25`; dispatch `pomodoro.start` with arg `""` (uses the 25-min default) → assert a timer is armed
(`editor.pending_plugin_timers.len() == 1`) and `next_wake == Some(now + 25·60·1000)`; **advance the
`TestClock` past the deadline, `reduce(Tick)` + `pump`** → assert the status reads "session complete" AND
that this fired **during inactivity** (no input between arm and fire — the whole point); assert
`plugin_list` shows "1 timers"; dispatch `pomodoro.cancel` → `next_wake == None`; re-arm then
`perform_reload` → `pending_plugin_timers` empty + `next_wake == None` (auto-disarm). No wall-clock sleep —
time is advanced deterministically.

---

## 12. Anti-regrowth / module budgets

All figures below are **PRODUCTION-line counts** per `wordcartel/tests/module_budgets.rs` (which counts
lines before the last `mod tests`, NOT raw file length — e.g. `app.rs` is 4529 raw / **899** production).
Confirmed against the module_budgets test at HEAD `8a06ab8` (P2 shipped app.rs at 899).

- **`timers.rs` (budget 400; ~338 production now).** Gains two ~3-line `fn` deadline rows + two
  `SUBSYSTEMS` entries + a ~4-line `on_tick` on_change branch. `next_wake`/`on_tick` bodies otherwise
  unchanged (no Vec, no dispatch generalization) — the table grows by ROWS, exactly what its budget test
  (`timers_rs_grows_by_rows_not_bulk`) permits. Comfortably within 400.
- **`app.rs` (budget 1000; 899 production now).** Gains: ~3 lines in `advance` (arm `on_change_due`
  beside the reconcile debounce). Timer disarm + the `clear_plugin_wake_state` helper live in
  `plugin/reload.rs` (NOT app.rs); no timer firing, no dispatch logic in `app.rs` (it's in the pump).
  Net `app.rs` growth single-digit — stays under 1000 (gate-enforced by `module_budgets`).
- **`plugin/pump.rs` (budget 350; 267 lines total, mostly production).** Gains the Phase-0 timer-fire
  pass + reschedule (~50-70 lines). Within 350; if tight, the fire pass extracts to a `fire_due_timers`
  helper (thin, one axis).
- **`plugin/host.rs` (400) / `plugin/api.rs`.** `api.rs` gains `install_timer` + the arg parse (flat seam,
  no dispatcher edit). `attach_bridge` gains the one-line `has_on_change_subscriber` recompute. Within
  budget.
- **`registry.rs`.** `CommandMeta.arg` field + the `dispatch` `Plugin` arm branch (~4 lines) + `plugin_list`
  format tweak. Stays Lua-free (the minibuffer-open is editor-state mutation, no `mlua`). A data/table
  extension, not a new dispatcher.
- **`minibuffer.rs`.** One `MinibufferKind` variant + one submit arm — the closed per-kind table the
  grounding named as the arg-collection precedent.

---

## 13. Task-decomposition sketch (ordering for the plan; no forward deps)

1. **Limits + editor fields + `PluginEventKind::Change`** — `PLUGIN_TIMER_MIN_INTERVAL_MS`/
   `PLUGIN_MAX_TIMERS_PER_PLUGIN`/`PLUGIN_MAX_COMMAND_ARG`; `editor.pending_plugin_timers`/
   `next_timer_handle`/`on_change_due`/`has_on_change_subscriber`; `PluginTimer`; `Change` +
   `event_from_str`/`kind_str`. Inert scaffolding, tree green.
2. **`timers.rs` rows + the idle-free guardrail tests** — `on_change_deadline` + `plugin_timer_deadline`
   + the two `SUBSYSTEMS` entries; the §4 new guardrail tests (they pass with an empty timer set,
   proving zero-cost-at-rest before any arming exists). Existing timer tests unchanged.
3. **`wc.timer` arm/cancel + the pump fire phase** — `install_timer`/`wc.timer_cancel` (observer/floor/cap
   checks), `pump` Phase 0 (mark-then-fire-atomically/reschedule-from-completion/one-pending — Critical 1),
   the `clear_plugin_wake_state` helper wired into `perform_reload` + the null-host pump branch (Critical
   2 — clears timers AND on_change), the arm/fire/cancel/reload + cap-no-strand guardrail tests.
4. **`on_change`** — `has_on_change_subscriber` recompute in `attach_bridge`; arm `on_change_due` in
   `advance`; fire in `on_tick`; the debounce + no-hot-path + subscriber-gate tests.
5. **Parameterized commands** — `CommandMeta.arg`; `PluginCall`/`PluginDispatch` arg (drop `Copy`,
   migrate every site); `MinibufferKind::PluginArg` + submit; `dispatch` branch; `wc.register_command{arg}`
   + `wc.command(name,arg)`; `invoke_call` arg passing; the arg cap; palette/keybinding/`wc.command`
   dispatch tests.
6. **`plugin_list` timer count** — the format extension + test.
7. **`pomodoro.lua` demo + clock-driven e2e** (§11) — the driver that de-speculates the whole slice.
8. **Gates** — contract invariants (palette/menu over a parameterized-command registry), the full
   idle-free guardrail set, module budgets, smoke, the demo.

Dependency notes: 1 → everything; 2 before 3 (the rows must exist before firing wakes the loop); 3 before
7 (the demo arms a timer); 5 independent of 3/4 (can parallel), but 7 needs both 3 and 5.

---

## 14. ⚠OPEN — HUMAN DECISION flags

**None.** All three grounding §5 open design points resolved with a clearly-correct answer:
1. **Timer tier → observer-tier** — minimal for the demo AND the safer choice (no autonomous background
   document mutation; no-timer-spawns-timers for free). A resolved product/safety call, not a fork.
2. **`on_change` coalescing → its own version-latched deadline armed beside the reconcile debounce,
   subscriber-gated** — clearly correct (edge-armed, self-clearing, zero-cost when unused, no hot-path).
3. **`timers.rs` shape → editor-side `Vec` + two static fn-ptr rows + pump-side firing** (no
   `SUBSYSTEMS`→`Vec`, no `on_tick` generalization) — a strict simplification of the grounding's forecast,
   forced by `next_wake`/`on_tick` taking `&Editor` (not the host), with the auto-disarm invariant
   preserved by clearing in `perform_reload`'s existing block.

(One product heads-up for the human, NOT a blocking fork: observer-tier timers cannot open a modal or
chain a work→break phase from the callback — the bare-bones demo doesn't need it, and command-tier is a
clean, guarded future widening if a real pomodoro plugin wants richer notification.)
