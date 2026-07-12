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
            return;
        }
        let start = std::time::Instant::now();
        let mut units = 0usize;
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
