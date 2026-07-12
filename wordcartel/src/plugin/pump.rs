//! The plugin pump: the unified re-drain loop that is the ONLY place plugin Lua callbacks run
//! (P2 Task 7 module-size seam — split out of `plugin::host` to keep that module's VM-lifecycle
//! responsibility within its hub budget; `PluginHost`'s `lua`/`bridge`/`hooks` fields are
//! `pub(super)` in `host.rs` so the `impl PluginHost` block below can reach them). One pump
//! cycle drains `wc.command` dispatches, plugin-command calls, and fired events (× their hooks)
//! to quiescence or to a cap — re-taking all three queues after each pass, since a command
//! callback can `wc.command` (→ new calls/dispatches) and a dispatched builtin can fire an event
//! (→ new hook invocations). Two BETWEEN-UNITS caps bound the cycle (§5c):
//! [`crate::limits::PLUGIN_PUMP_CHAIN_CAP`] (a deterministic unit count) and
//! [`PUMP_CYCLE_TIME_BUDGET`] (wall clock, whole cycle).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::editor::Editor;
use wordcartel_core::history::Clock;

use super::host::{CALLBACK_TIME_BUDGET, HookEntry, InvokeState, PluginHost, with_time_guard};

/// The pump's re-drain loop wall-clock budget (§5c) — a BETWEEN-UNITS cap on the cycle's total
/// elapsed time, checked before dequeuing the next unit (never preemptive: it cannot interrupt
/// a unit already running). Exists because [`crate::limits::PLUGIN_PUMP_CHAIN_CAP`] alone
/// permits `CAP × CALLBACK_TIME_BUDGET` of stall (64 × 150 ms ≈ 9.6 s — many short units
/// summing large), which the count cap cannot express but the wall clock can.
pub(crate) const PUMP_CYCLE_TIME_BUDGET: Duration = Duration::from_millis(500);

/// RAII: sets [`InvokeState::observer`] and `current` to the invoking unit's label for the
/// duration of ONE unit (a hook callback OR — P2 Task 7 — a command callback, so `wc.command`'s
/// `origin` attribution works from either), and resets both on drop — normal return AND unwind
/// alike (the `HookGuard` pattern) — so a panicking unit can never leak observer mode onto the
/// next one.
struct ObserverGuard {
    state: Rc<RefCell<InvokeState>>,
}

impl ObserverGuard {
    /// `observer = true` for a hook (blocks edits/`wc.command` — mutation-by-proxy); `false`
    /// for a command callback (a command IS allowed to edit/dispatch — that is its purpose).
    fn enter(state: Rc<RefCell<InvokeState>>, label: &str, observer: bool) -> ObserverGuard {
        {
            let mut s = state.borrow_mut();
            s.observer = observer;
            s.current = Some(label.to_string());
        }
        ObserverGuard { state }
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        let mut s = self.state.borrow_mut();
        s.observer = false;
        s.current = None;
    }
}

impl PluginHost {
    /// One pump cycle: drains `wc.command` dispatches, plugin-command calls, and fired events
    /// (× their hooks) to quiescence or to a cap — the unified re-drain loop (§5c). P1's pump
    /// was a deliberate single pass ("nothing can enqueue mid-pump"); P2 breaks that premise
    /// twice — a command callback can `wc.command` (→ new calls/dispatches) and a dispatched
    /// builtin can fire an event (→ new hook invocations) — so the loop re-takes all three
    /// queues after each pass until every queue is empty.
    ///
    /// Takes the dispatch context (`reg`/`ex`/`clock`/`msg_tx` — mirrors `reduce`'s params) so a
    /// `wc.command` target routes through the SAME `Registry::dispatch` the palette/menu/keys
    /// use (contract law 1: never a side channel). Takes the editor HANDLE, never `&mut Editor`,
    /// so no borrow is held across Lua — Phase A's drain always drops before Phase B invokes
    /// anything.
    ///
    /// Two BETWEEN-UNITS caps bound the cycle (checked before dequeuing the next unit; neither
    /// preempts a unit already running — a running Lua callback/hook is bounded instead by the
    /// per-invocation [`CALLBACK_TIME_BUDGET`], and a `wc.command`-dispatched builtin is bounded
    /// by builtins being hot-path-safe by construction):
    /// [`crate::limits::PLUGIN_PUMP_CHAIN_CAP`] (a deterministic unit count — one dispatch, one
    /// call, or one hook invocation) and [`PUMP_CYCLE_TIME_BUDGET`] (wall clock, whole cycle —
    /// bounds `CAP × CALLBACK_TIME_BUDGET` of possible stall that the count cap alone permits).
    /// On trip, [`Self::cap_tripped`] clears all three queues and sets a `plugin_error` status —
    /// dropping beats carrying over: queued plugin work is advisory, and deferring it to the
    /// next frame would let a hostile cascade starve every subsequent frame instead of one.
    pub fn pump(&mut self, editor: &Rc<RefCell<Editor>>, reg: &crate::registry::Registry,
                ex: &dyn crate::jobs::Executor, clock: &dyn Clock,
                msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
        if self.lua.is_none() {
            // Null-host discipline (P2 §3d): fire sites push events/dispatches UNCONDITIONALLY
            // (they don't know whether a VM is live), so under `--no-plugins` these queues would
            // grow without bound unless cleared here every pump.
            let mut e = editor.borrow_mut();
            e.pending_plugin_calls.clear();
            e.pending_plugin_events.clear();
            e.pending_plugin_dispatch.clear();
            e.clear_plugin_wake_state(); // P3 §3g: a torn-down (null) host leaves NO armed timer/subscriber
            return;
        }
        let start = std::time::Instant::now();
        let mut units = 0usize;
        if self.fire_due_timers(editor, clock, &mut units, start) { return; } // Phase 0 (mark-then-fire)
        loop {
            // Phase A — take all three queues under ONE short borrow that drops immediately.
            let (dispatches, calls, events) = {
                let mut e = editor.borrow_mut();
                (
                    std::mem::take(&mut e.pending_plugin_dispatch),
                    std::mem::take(&mut e.pending_plugin_calls),
                    std::mem::take(&mut e.pending_plugin_events),
                )
            };
            if dispatches.is_empty() && calls.is_empty() && events.is_empty() {
                break;
            }
            // Phase B — process with NO outer borrow held; check both caps BETWEEN units.
            for d in dispatches {
                if self.cap_tripped(units, start, editor) { return; }
                units += 1;
                self.drain_one_dispatch(editor, reg, ex, clock, msg_tx, &d);
            }
            for c in calls {
                if self.cap_tripped(units, start, editor) { return; }
                units += 1;
                self.invoke_call(editor, c);
            }
            for ev in events {
                for h in self.hooks_for(ev.kind) {
                    if self.cap_tripped(units, start, editor) { return; }
                    units += 1;
                    self.invoke_hook(editor, &ev, &h);
                }
            }
        }
    }

    /// Phase 0 — fire due plugin timers (P3 §3d). **Invariant (Critical 1 + Codex Important 2): a
    /// timer is `pending` ONLY when its callback is DEFINITELY about to run — no guard checked
    /// AFTER marking (`cap_tripped`, bridge-None, VM-null) can strand it.** So EVERY guard is
    /// checked BEFORE `pending` is set: the VM-null guard is the pump's own `self.lua.is_none()`
    /// early-return; the bridge-None guard is hoisted to the TOP here (`lua: Some, bridge: None`
    /// after an attach_bridge failure — no `invoke_state`, no editor API installed, so NO timer can
    /// run: fire nothing, mark nothing); the cap guard is checked per iteration before marking.
    /// `pending` is set in the same short borrow immediately before invoke. Observer-tier callbacks
    /// (like hooks); reschedule-from-completion; one-pending-per-timer. Returns `true` iff a cap
    /// tripped and the caller (`pump`) must stop this cycle.
    fn fire_due_timers(&self, editor: &Rc<RefCell<Editor>>, clock: &dyn Clock,
                       units: &mut usize, start: std::time::Instant) -> bool {
        // Bridge-None guard HOISTED (Codex Important 2): without a bridge there is no invoke_state
        // and no editor API — no timer callback can run, so mark nothing (defense-in-depth beyond
        // the bridge-attach-failure path also calling clear_plugin_wake_state).
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
            // ALL guards passed (VM present, bridge present, cap ok) → mark-then-read atomically in
            // the instant before invoke; skip (no mark) if cancelled/removed since the snapshot.
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
                    let interval = e.pending_plugin_timers[pos].interval_ms;
                    e.pending_plugin_timers[pos].next_due_ms = now2.saturating_add(interval);
                    e.pending_plugin_timers[pos].pending = false;
                } else {
                    let key = e.pending_plugin_timers.remove(pos).key;
                    let _ = lua.set_named_registry_value(&key, mlua::Value::Nil);
                }
            }
        }
        false
    }

    /// Test-only pump convenience: builds a throwaway `Registry::builtins()` +
    /// `InlineExecutor` + `TestClock` + channel dispatch context and calls the real
    /// [`Self::pump`]. The P1/P2 host tests exercise callback/isolation/event behavior that
    /// needs no SPECIFIC registry — a test that DOES need one (a `wc.command` target, a
    /// ping-pong cascade between two plugin commands) calls [`Self::pump`] directly with its
    /// own `reg`.
    #[cfg(test)]
    pub(crate) fn pump_test(&mut self, editor: &Rc<RefCell<Editor>>) {
        let reg = crate::registry::Registry::builtins();
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        self.pump(editor, &reg, &ex, &clock, &tx);
    }

    /// Between-units cap check for the pump's re-drain loop (§5c) — `true` means EITHER cap
    /// tripped and the caller must stop immediately (no further unit is dequeued this cycle).
    /// On trip: clear all three queues and set a status naming the truncation (one short borrow) —
    /// the editor itself is left intact, only queued plugin work is dropped. This is a subsystem
    /// event, not a single plugin's fault, so it does NOT go through `plugin_error`'s
    /// `"plugin {name}: "` framing (which would read "plugin plugins: …").
    fn cap_tripped(&self, units: usize, start: std::time::Instant, editor: &Rc<RefCell<Editor>>) -> bool {
        if units < crate::limits::PLUGIN_PUMP_CHAIN_CAP && start.elapsed() <= PUMP_CYCLE_TIME_BUDGET {
            return false;
        }
        let mut e = editor.borrow_mut();
        e.pending_plugin_calls.clear();
        e.pending_plugin_events.clear();
        e.pending_plugin_dispatch.clear();
        e.status = "plugin work truncated (chain cap)".to_string();
        true
    }

    /// Resolve + run ONE `wc.command` dispatch (§5b) — `reg.resolve_name` is called HERE, at
    /// drain time (not at `wc.command` call time — no derived call-time name-set snapshot,
    /// contract law 1). An unknown name degrades to a `plugin_error` naming the origin, never a
    /// panic. `Some(id)` builds a `Ctx` under one short `borrow_mut` and calls
    /// `reg.dispatch(id, &mut ctx)` — the SAME path the palette/menu/keys use: a `Builtin` runs
    /// synchronously inside that borrow (builtin handlers never call Lua); a `Plugin` entry
    /// enqueues a `PluginCall` back onto `editor.pending_plugin_calls`, picked up by the next
    /// re-drain iteration. The borrow drops before any Lua runs.
    fn drain_one_dispatch(
        &self,
        editor: &Rc<RefCell<Editor>>,
        reg: &crate::registry::Registry,
        ex: &dyn crate::jobs::Executor,
        clock: &dyn Clock,
        msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
        d: &crate::plugin::PluginDispatch,
    ) {
        let Some(id) = reg.resolve_name(&d.name) else {
            crate::plugin::plugin_error(editor, &d.origin, &format!("unknown command '{}'", d.name));
            return;
        };
        let mut e = editor.borrow_mut();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock, executor: ex, msg_tx: msg_tx.clone() };
        reg.dispatch(id, &mut ctx);
    }

    /// Invoke ONE drained [`crate::plugin::PluginCall`]'s stored Lua callback (the P1 shape).
    /// Wrapped in an `ObserverGuard(observer=false)` when a bridge is live, so
    /// [`InvokeState::current`] names the running command — `wc.command`'s own `origin`
    /// attribution (§5a) reads it. Every invocation is wrapped in [`crate::panicx::catch`] (the
    /// spike found `mlua` resumes a raw Rust panic rather than converting it — `panicx` is the
    /// SOLE backstop, no gaps) and in [`Self::with_time_guard`] (the `set_hook` runaway-time
    /// abort).
    fn invoke_call(&self, editor: &Rc<RefCell<Editor>>, call: crate::plugin::PluginCall) {
        let Some(lua) = self.lua.as_ref() else { return };
        let key = format!("wc-cmd-{}", call.id.0);
        let _guard = self.bridge.as_ref()
            .map(|b| ObserverGuard::enter(b.invoke_state.clone(), call.id.0, false));
        let outcome: Result<mlua::Result<()>, String> = crate::panicx::catch(|| {
            let cb: mlua::Function = lua.named_registry_value(&key)?;
            self.with_time_guard(lua, || cb.call::<()>(()))
        });
        if let Err(msg) = normalize(outcome) {
            crate::plugin::plugin_error(editor, call.id.0, &msg);
        }
    }

    /// Committed hooks whose kind matches `kind`, cloned out before invocation — see
    /// [`HookEntry`]'s doc for why (no borrow of `self.hooks` held across a callback).
    fn hooks_for(&self, kind: crate::plugin::PluginEventKind) -> Vec<HookEntry> {
        self.hooks.iter().filter(|h| h.kind == kind).cloned().collect()
    }

    /// Invoke ONE hook for ONE fired event (the P1/Task-6 shape). Wrapped in an
    /// `ObserverGuard(observer=true)` — RAII: resets on both normal return and unwind (the
    /// `HookGuard` pattern), so a panicking hook can never leak observer mode onto the next
    /// unit. No bridge (VM exists but `attach_bridge` never ran) → nothing to invoke against, a
    /// harmless no-op.
    fn invoke_hook(&self, editor: &Rc<RefCell<Editor>>, ev: &crate::plugin::PluginEvent, h: &HookEntry) {
        let Some(lua) = self.lua.as_ref() else { return };
        let Some(invoke_state) = self.bridge.as_ref().map(|b| b.invoke_state.clone()) else { return };
        let key = h.key.clone();
        let label = h.label.clone();
        let _guard = ObserverGuard::enter(invoke_state, &label, true);
        let outcome: Result<mlua::Result<()>, String> = crate::panicx::catch(|| {
            let cb: mlua::Function = lua.named_registry_value(&key)?;
            let arg = event_table(lua, ev)?;
            self.with_time_guard(lua, || cb.call::<()>((arg,)))
        });
        if let Err(msg) = normalize(outcome) {
            crate::plugin::plugin_error(editor, &label, &msg);
        }
    }

    /// The `set_hook` runaway guard (spike §11.3, GREEN), fixed at [`CALLBACK_TIME_BUDGET`] — a
    /// thin forwarder to the free `with_time_guard` fn (`plugin::host`, shared with the load
    /// phase's own budget). Scoped tightly around ONE callback invocation and ALWAYS removed
    /// afterward — a leaked hook would fire during the NEXT unrelated Lua call (registration,
    /// another plugin's callback).
    fn with_time_guard<T>(&self, lua: &mlua::Lua, f: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        with_time_guard(lua, CALLBACK_TIME_BUDGET, f)
    }
}

/// Build the one Lua table argument a hook callback receives: `{ kind = "save"|"open"|
/// "buffer_close", path = <string>|nil }` (P2 §3a). Deliberately minimal — additive fields are
/// backward-compatible, removing one would not be.
fn event_table(lua: &mlua::Lua, ev: &crate::plugin::PluginEvent) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("kind", crate::plugin::kind_str(ev.kind))?;
    t.set("path", ev.path.clone())?;
    Ok(t)
}

/// Flatten the pump's two-layer outcome (an outer caught panic, an inner `mlua::Result` from a
/// missing callback / Lua `error()` / a typed API error) to a single error message — a panic and
/// a Lua-side failure surface identically to [`crate::plugin::plugin_error`].
fn normalize(outcome: Result<mlua::Result<()>, String>) -> Result<(), String> {
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(panic_msg) => Err(panic_msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use crate::editor::Editor;
    use crate::plugin::load::load_sources;
    use crate::plugin::{PluginEvent, PluginEventKind, PluginTimer};
    use crate::registry::Registry;
    use crate::test_support::TestClock;

    /// An ADVANCEABLE test clock: a shared `Rc<Cell<u64>>` so the bridge's `Rc<dyn Clock>`
    /// (arm-time reads) and the pump's `&dyn Clock` (fire-time reads) observe the SAME wall
    /// clock — cloning shares the underlying cell. `TestClock` (a fixed value) cannot express
    /// "arm at t, advance, fire" the timer engine needs.
    #[derive(Clone)]
    struct SharedClock(Rc<Cell<u64>>);
    impl SharedClock {
        fn new(ms: u64) -> Self { SharedClock(Rc::new(Cell::new(ms))) }
        fn set(&self, ms: u64) { self.0.set(ms); }
    }
    impl Clock for SharedClock {
        fn now_ms(&self) -> u64 { self.0.get() }
    }

    fn whole_text(editor: &Rc<RefCell<Editor>>) -> String {
        let e = editor.borrow();
        let buf = &e.active().document.buffer;
        buf.slice(0..buf.len())
    }

    /// A fresh live VM + bridge over `Editor::new_from_text(text, ..)`, wired to a shared,
    /// advanceable clock (returned so the test can advance it between arm and fire). `wc.timer`
    /// is installed by `attach_bridge` → `install_editor_api`.
    fn timer_host(text: &str) -> (PluginHost, Rc<RefCell<Editor>>, SharedClock) {
        let mut host = PluginHost::new().expect("VM construction");
        let editor = Rc::new(RefCell::new(Editor::new_from_text(text, None, (40, 10))));
        let (tx, _rx) = std::sync::mpsc::channel();
        std::mem::forget(_rx); // keep the bridge's sender live for the test's lifetime
        let clock = SharedClock::new(0);
        host.attach_bridge(editor.clone(), tx, Rc::new(clock.clone()) as Rc<dyn Clock>)
            .expect("bridge attaches on a live VM");
        (host, editor, clock)
    }

    /// Seed a due timer DIRECTLY onto the editor (bypassing the per-plugin arm cap) with a no-op
    /// Rust callback stored under its `wc-timer-<handle>` key — the arrange step for the strand
    /// guardrails, which need many due timers / a bridge-less host without routing through `wc.timer`.
    fn seed_due_noop(host: &PluginHost, editor: &Rc<RefCell<Editor>>, handle: u64, due: u64, repeat: bool) {
        let lua = host.lua().expect("live VM");
        let key = format!("wc-timer-{handle}");
        let f = lua.create_function(|_, ()| -> mlua::Result<()> { Ok(()) }).expect("create_function");
        lua.set_named_registry_value(&key, f).expect("store callback");
        editor.borrow_mut().pending_plugin_timers.push(PluginTimer {
            handle, origin: "seed".into(), key, next_due_ms: due, interval_ms: 1_000, repeat, pending: false,
        });
    }

    /// Load ONE hook plugin (`src` may call `wc.on`) + attach a bridge over `text` — the arrange
    /// step for the observer-gate (`wc.timer` from a hook) test.
    fn load_hook(src: &str, text: &str) -> (PluginHost, Rc<RefCell<Editor>>) {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let sources = vec![("t".to_string(), src.to_string())];
        load_sources(&mut reg, &mut host, &sources, &BTreeMap::new(), &mut Vec::new());
        let editor = Rc::new(RefCell::new(Editor::new_from_text(text, None, (40, 10))));
        let (tx, _rx) = std::sync::mpsc::channel();
        std::mem::forget(_rx);
        host.attach_bridge(editor.clone(), tx, Rc::new(TestClock::new(0))).expect("bridge attaches");
        (host, editor)
    }

    /// A throwaway dispatch context + a real `pump` firing at the shared clock's current time.
    fn pump_at(host: &mut PluginHost, editor: &Rc<RefCell<Editor>>, clock: &SharedClock) {
        let reg = Registry::builtins();
        let ex = crate::jobs::InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        host.pump(editor, &reg, &ex, clock, &tx);
    }

    // -----------------------------------------------------------------------
    // wc.timer / wc.timer_cancel — arm, next-wake, floor, cap, observer gate.
    // -----------------------------------------------------------------------

    #[test]
    fn wc_timer_arms_and_next_wake_reflects_it() {
        let (host, editor, _clock) = timer_host("x");
        let lua = host.lua().unwrap();
        lua.load("h1 = wc.timer(60000, function() end)").exec().unwrap();
        assert_eq!(editor.borrow().pending_plugin_timers.len(), 1, "one wc.timer arms one PluginTimer");
        assert_eq!(crate::timers::next_wake(&editor.borrow(), 0), Some(60_000),
            "next_wake reflects the armed timer's due (armed at t=0, +60000ms)");
        lua.load("h2 = wc.timer(60000, function() end)").exec().unwrap();
        assert_eq!(editor.borrow().pending_plugin_timers.len(), 2, "a second wc.timer → two");
        lua.load("wc.timer_cancel(h1)").exec().unwrap();
        assert_eq!(editor.borrow().pending_plugin_timers.len(), 1, "cancel one → one");
        assert_eq!(editor.borrow().pending_plugin_timers[0].handle, 2, "h1 cancelled, h2 remains");
    }

    #[test]
    fn wc_timer_below_floor_is_typed_error() {
        let (host, editor, _clock) = timer_host("x");
        host.lua().unwrap().load("ok, err = pcall(wc.timer, 999, function() end); err = tostring(err)")
            .exec().unwrap();
        let lua = host.lua().unwrap();
        let ok: bool = lua.globals().get("ok").unwrap();
        let err: String = lua.globals().get("err").unwrap();
        assert!(!ok, "a sub-floor interval must be a pcall-false error");
        assert!(err.contains("floor"), "err must name the floor: {err}");
        assert!(editor.borrow().pending_plugin_timers.is_empty(), "nothing armed below the floor");
    }

    #[test]
    fn wc_timer_over_cap_is_typed_error() {
        let (host, editor, _clock) = timer_host("x");
        host.lua().unwrap().load(
            "for i=1,8 do wc.timer(1000, function() end) end\n\
             ok, err = pcall(wc.timer, 1000, function() end); err = tostring(err)").exec().unwrap();
        let lua = host.lua().unwrap();
        let ok: bool = lua.globals().get("ok").unwrap();
        let err: String = lua.globals().get("err").unwrap();
        assert!(!ok, "the 9th timer for one plugin must be rejected");
        assert!(err.contains("limit"), "err must name the limit: {err}");
        assert_eq!(editor.borrow().pending_plugin_timers.len(), 8, "still exactly 8 armed");
    }

    #[test]
    fn wc_timer_from_hook_is_rejected() {
        // A timer callback must not be arm-able from an event hook (observer-tier): mutation-by-
        // proxy AND a timer-spawns-timers spin vector are both closed at the arm gate.
        let src = "wc.on('save', function(ev) \
                       local ok, err = pcall(wc.timer, 1000, function() end); \
                       wc.status(tostring(ok) .. ':' .. tostring(err)) \
                   end)";
        let (mut host, editor) = load_hook(src, "x");
        editor.borrow_mut().pending_plugin_events.push_back(
            PluginEvent { kind: PluginEventKind::Save, path: None });
        host.pump_test(&editor);
        let status = editor.borrow().status.clone();
        assert!(status.starts_with("false:"), "wc.timer from a hook must be rejected: {status}");
        assert!(status.contains("event hook or a timer callback"), "status: {status}");
        assert!(editor.borrow().pending_plugin_timers.is_empty(), "nothing armed from a hook");
    }

    // -----------------------------------------------------------------------
    // The pump's Phase 0 — mark-then-fire, reschedule-from-completion, one-shot.
    // -----------------------------------------------------------------------

    #[test]
    fn wc_timer_fires_during_inactivity_via_pump() {
        let (mut host, editor, clock) = timer_host("x");
        host.lua().unwrap().load("wc.timer(1000, function() wc.status('fired') end)").exec().unwrap();
        assert_eq!(editor.borrow().pending_plugin_timers.len(), 1);
        clock.set(1500); // past the 1000ms due; NO input between arm and fire
        pump_at(&mut host, &editor, &clock);
        assert_eq!(editor.borrow().status, "fired", "the one-shot callback ran during inactivity");
        assert!(editor.borrow().pending_plugin_timers.is_empty(), "a one-shot is removed after firing");
        assert_eq!(crate::timers::next_wake(&editor.borrow(), 1500), None, "nothing armed after the one-shot");
    }

    #[test]
    fn wc_timer_repeat_reschedules_from_completion() {
        let (mut host, editor, clock) = timer_host("x");
        host.lua().unwrap().load("wc.timer(1000, function() wc.status('r') end, true)").exec().unwrap();
        clock.set(2500); // fires at completion_now = 2500
        pump_at(&mut host, &editor, &clock);
        let e = editor.borrow();
        assert_eq!(e.pending_plugin_timers.len(), 1, "a repeating timer is NOT removed");
        assert_eq!(e.pending_plugin_timers[0].next_due_ms, 3500,
            "reschedule FROM COMPLETION: completion_now(2500) + interval(1000)");
        assert!(!e.pending_plugin_timers[0].pending, "pending is cleared after the reschedule");
        assert_eq!(e.status, "r", "the repeating callback ran");
    }

    #[test]
    fn timer_callback_is_observer_tier_cannot_edit() {
        let (mut host, editor, clock) = timer_host("hello");
        host.lua().unwrap().load(
            "wc.timer(1000, function() \
                 local ok, err = pcall(wc.insert, 'X'); \
                 wc.status('fired:' .. tostring(ok) .. ':' .. tostring(err)) \
             end)").exec().unwrap();
        clock.set(1500);
        pump_at(&mut host, &editor, &clock);
        assert_eq!(whole_text(&editor), "hello", "a timer callback must NOT edit the buffer (observer-tier)");
        let status = editor.borrow().status.clone();
        assert!(status.starts_with("fired:false:"), "wc.insert must be blocked from a timer callback: {status}");
        assert!(status.contains("editing is not allowed from an event hook"), "status: {status}");
        // wc.status STILL worked (it set the status above) — the one allowed observer surface.
    }

    // -----------------------------------------------------------------------
    // Strand guardrails (Critical 1 + Codex Important 2) + the anti-spin bound.
    // -----------------------------------------------------------------------

    #[test]
    fn cap_trip_does_not_strand_a_due_timer() {
        // Seed CAP+1 due REPEATING timers directly so a single pump's Phase-0 fire-batch trips
        // PLUGIN_PUMP_CHAIN_CAP mid-batch. The invariant: every not-yet-fired due timer is left
        // `!pending` (fires next pump), NEVER stranded pending against a dead callback.
        let (mut host, editor, clock) = timer_host("x");
        let n = (crate::limits::PLUGIN_PUMP_CHAIN_CAP + 1) as u64;
        for h in 1..=n {
            seed_due_noop(&host, &editor, h, 0, true); // due at 0, repeating
        }
        clock.set(10_000); // all due
        pump_at(&mut host, &editor, &clock);
        let e = editor.borrow();
        assert!(e.pending_plugin_timers.iter().all(|t| !t.pending),
            "NO timer may be left marked pending after a mid-batch cap trip");
        assert!(e.pending_plugin_timers.iter().any(|t| t.next_due_ms == 0),
            "at least one timer never fired — still due at its original (past) time");
        assert_eq!(crate::timers::next_wake(&e, 10_000), Some(0),
            "the un-fired due timer still drives an (immediate) wake — not stranded");
        assert!(e.status.to_lowercase().contains("truncat"), "the cap trip sets a truncation status: {}", e.status);
    }

    #[test]
    fn no_bridge_does_not_strand_a_due_timer() {
        // lua: Some, bridge: None (an attach_bridge that never ran / failed): NO invoke_state,
        // no editor API — so `fire_due_timers` must fire NOTHING and mark NOTHING.
        let mut host = PluginHost::new().expect("VM construction");
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        seed_due_noop(&host, &editor, 1, 0, false); // due, but no bridge attached
        let reg = Registry::builtins();
        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(10_000);
        let (tx, _rx) = std::sync::mpsc::channel();
        host.pump(&editor, &reg, &ex, &clock, &tx);
        let e = editor.borrow();
        assert_eq!(e.pending_plugin_timers.len(), 1, "the timer must remain armed under a bridge-None host");
        assert!(!e.pending_plugin_timers[0].pending, "a bridge-None pump must never mark a timer pending");
        assert_eq!(crate::timers::next_wake(&e, 10_000), Some(0), "the timer still drives a wake (not stranded)");
    }

    #[test]
    fn floored_repeating_timer_bounded_wakes_not_a_spin() {
        // A 1000ms repeating timer driven over N seconds fires ≤ N+1 times, and every post-fire
        // cadence is >= the floor — a bounded wake rate, never a per-pump spin.
        let (mut host, editor, clock) = timer_host("x");
        host.lua().unwrap().load("fires = 0; wc.timer(1000, function() fires = fires + 1 end, true)")
            .exec().unwrap();
        let n: u64 = 5;
        for step in 1..=n {
            clock.set(step * 1000); // advance exactly one interval per pump
            pump_at(&mut host, &editor, &clock);
            let e = editor.borrow();
            let t = &e.pending_plugin_timers[0];
            assert!(!t.pending, "the timer is never left pending between pumps");
            assert!(t.next_due_ms.saturating_sub(clock.now_ms()) >= 1_000,
                "post-fire cadence (next_due - now) must be >= the 1000ms floor, step {step}");
        }
        let fires: i64 = host.lua().unwrap().globals().get("fires").unwrap();
        assert!(fires <= (n as i64) + 1, "bounded wakes: {fires} fires over {n}s must be <= N+1");
        assert_eq!(fires, n as i64, "exactly one fire per advanced interval");
    }

    #[test]
    fn teardown_clears_both_subsystems() {
        // The null-host pump branch (P3 §3g) clears the timer schedule + the on_change
        // subscription via `clear_plugin_wake_state` — a torn-down host leaves NO armed wake.
        let mut host = PluginHost::null();
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        {
            let mut e = editor.borrow_mut();
            e.pending_plugin_timers.push(PluginTimer {
                handle: 1, origin: "p".into(), key: "wc-timer-1".into(),
                next_due_ms: 500, interval_ms: 1_000, repeat: false, pending: false,
            });
            e.has_on_change_subscriber = true;
            e.on_change_due = Some(500);
        }
        host.pump_test(&editor);
        let e = editor.borrow();
        assert!(e.pending_plugin_timers.is_empty(), "the null-host pump clears the timer schedule");
        assert!(!e.has_on_change_subscriber, "the on_change subscription is cleared too");
        assert_eq!(e.on_change_due, None, "the on_change due is cleared");
        assert_eq!(crate::timers::next_wake(&e, 10_000), None, "a torn-down host arms no wake");
    }
}
