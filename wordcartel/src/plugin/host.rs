//! The plugin VM host: owns the one `mlua` VM + bridge, and the pump. `null()` is the no-VM
//! host used for `--no-plugins`, load failure, and tests that don't exercise plugins (mirrors
//! `NullProvider`). Task 4 adds the real VM ([`PluginHost::new`]) and the registration sink
//! ([`PendingReg`]) the load layer (`plugin::load`) drains into the `Registry` after each
//! plugin's script executes, atomically per plugin. Task 5 adds the [`Bridge`] (the live
//! `Rc<RefCell<Editor>>` + `Sender<Msg>` + clock the `wc.*` editor API closures capture,
//! installed by [`PluginHost::attach_bridge`]) and [`PluginHost::pump`] — the two-phase
//! drain-then-invoke that is the only place plugin Lua callbacks run.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::editor::Editor;
use crate::registry::MenuCategory;
#[cfg(test)]
use crate::registry::CommandId;
use wordcartel_core::history::Clock;

/// One command a plugin's `wc.register_command` staged during exec — raw strings + the
/// callback, interned/registry-written ONLY at commit (P2 §7b two-phase). Carrying an
/// `mlua::Function` is sound: it is a GC-rooted handle into the loader's own VM, dropped if the
/// plugin fails preflight.
pub struct PendingReg {
    pub name_full: String,   // "<stem>.<name>" — raw, cap-checked, NOT yet interned
    pub label: String,       // raw, cap-checked
    pub menu: Option<MenuCategory>,
    pub func: mlua::Function, // stored under wc-cmd-<id> only on commit
}

#[cfg(test)]
thread_local! {
    /// When set, the NEXT fallible commit write in `load_one` returns a synthetic mlua error —
    /// exercises the commit-time-exhaustion fatal path without exhausting real memory.
    pub(crate) static FAIL_NEXT_COMMIT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The plugin VM's live connection to app state, installed once by [`PluginHost::attach_bridge`]
/// (Task 7's `run()`, after the `Rc<RefCell<Editor>>` wrap exists; tests call it directly).
/// Owns an `Rc<dyn Clock>` — not a borrowed `&dyn Clock` — because the `wc.*` edit closures are
/// `'static` (`create_function`) and so cannot borrow `run()`'s clock; each edit closure clones
/// this `Rc` and passes `&*clock` to `submit_transaction`.
pub struct Bridge {
    pub editor: Rc<RefCell<Editor>>,
    pub msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
    pub clock: Rc<dyn Clock>,
    /// What the pump is currently invoking, shared with every `wc.*` closure (each captures a
    /// clone) — the observer-only enforcement cell (P2 §3e). `pub(crate)`, not `pub` like the
    /// other `Bridge` fields: `InvokeState` itself is `pub(crate)` (an internal implementation
    /// detail, never meant to leak past this crate's boundary).
    pub(crate) invoke_state: Rc<RefCell<InvokeState>>,
}

/// What the pump is currently invoking, shared with every `wc.*` closure (each captures a
/// clone). `current` names the plugin command/hook (attribution — unused by any check yet;
/// wired for `wc.command`'s own observer check, Task 7). `observer` is true exactly while a
/// HOOK runs — the edit APIs check it and degrade to a typed error (the observer-only binding
/// constraint, enforced in code, not by trust).
pub(crate) struct InvokeState {
    pub current: Option<String>,
    pub observer: bool,
}

/// A committed hook: its VM-registry key + kind + a label for `plugin_error` attribution.
/// Owned `String`s on the host — never interned (unlike command ids): hooks die with the VM at
/// reload, so interning them would be a reload-shaped leak.
pub struct HookEntry {
    pub kind: crate::plugin::PluginEventKind,
    pub key: String,
    pub label: String,
}

/// RAII: sets [`InvokeState::observer`] `true` + `current` to the invoking hook's label for the
/// duration of ONE hook callback, and resets both on drop — normal return AND unwind alike (the
/// [`HookGuard`] pattern) — so a panicking hook can never leak observer mode onto the next
/// command callback.
struct ObserverGuard {
    state: Rc<RefCell<InvokeState>>,
}

impl ObserverGuard {
    fn enter(state: Rc<RefCell<InvokeState>>, label: &str) -> ObserverGuard {
        {
            let mut s = state.borrow_mut();
            s.observer = true;
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

/// The plugin VM host. `lua: None` is the null host (no VM, no plugins); `lua: Some(_)` owns
/// the one real `mlua::Lua` for this app instance. `bridge` is `None` until
/// [`PluginHost::attach_bridge`] installs it (and always `None` for the null host).
pub struct PluginHost {
    lua: Option<mlua::Lua>, // None in the null host
    bridge: Option<Bridge>,
    /// Committed `wc.on` hooks across every loaded plugin, in load-then-registration order —
    /// owned, NOT interned (die with the VM at reload; P2 §3b).
    hooks: Vec<HookEntry>,
}

/// Runaway-callback wall-clock budget for the `set_hook` guard (spec §7: "~100–250 ms"; the
/// Task 1 spike (§11.3) measured ~168µs of hook overhead at 10k-instruction granularity —
/// negligible against any value in that range, so the midpoint is used). This is the line
/// between "plugin bug" and "editor hang" — a direct no-silent-UI-waits requirement.
pub(crate) const CALLBACK_TIME_BUDGET: Duration = Duration::from_millis(150);

/// Load-phase wall-clock budget for a plugin's top-level `exec()` (P2 §7a) — an order of
/// magnitude over [`CALLBACK_TIME_BUDGET`] because legitimate plugin init does more work
/// (table-building, registering several commands) than a single callback invocation. Closes the
/// gap where a runaway top-level loop (`while true do end`) hung the whole editor at startup,
/// unguarded: worst case is now ~1s per hung plugin — reported via `LoadReport` and survivable,
/// not the old ∞.
pub(crate) const LOAD_TIME_BUDGET: Duration = Duration::from_secs(1);

impl PluginHost {
    /// A null host — no VM, no plugins. Used for `--no-plugins`, load failure, and tests
    /// that don't exercise plugins (mirrors `NullProvider`).
    ///
    /// # Examples
    /// ```
    /// # use wordcartel::plugin::host::PluginHost;
    /// let host = PluginHost::null();
    /// assert!(!host.has_vm());
    /// ```
    pub fn null() -> PluginHost {
        PluginHost { lua: None, bridge: None, hooks: Vec::new() }
    }

    /// Build the real VM: a safe `Lua::new()` (never `unsafe_new` — no debug/ffi libraries
    /// exposed to plugin code) with the spike-confirmed heap cap wired in
    /// ([`spike_confirmed_mem_cap`] — Task 1 item 2 came back GREEN on vendored Lua 5.4, so the
    /// cap is retained rather than dropped per spec §7's documented-red allowance). Returns
    /// `Err` only if `set_memory_limit` itself fails.
    ///
    /// # Examples
    /// ```
    /// # use wordcartel::plugin::host::PluginHost;
    /// let host = PluginHost::new().expect("VM construction");
    /// assert!(host.has_vm());
    /// ```
    pub fn new() -> mlua::Result<PluginHost> {
        let lua = mlua::Lua::new(); // safe ctor — no debug/ffi libs, never unsafe_new
        if let Some(cap) = spike_confirmed_mem_cap() {
            lua.set_memory_limit(cap)?;
        }
        Ok(PluginHost { lua: Some(lua), bridge: None, hooks: Vec::new() })
    }

    /// Extend the committed hook list with `hs` — called by `load_sources` AFTER `load_one`
    /// returns (the `lua` borrow it held is released by then, so `&mut host` is free — same
    /// discipline as the fatal-null path).
    pub(crate) fn append_hooks(&mut self, hs: Vec<HookEntry>) {
        self.hooks.extend(hs);
    }

    /// Whether this host owns a live VM. `false` for [`PluginHost::null`].
    pub fn has_vm(&self) -> bool {
        self.lua.is_some()
    }

    /// The live VM, or `None` for the null host — the load-core's (`plugin::load::load_sources`)
    /// early-out: a null host loads nothing.
    pub(crate) fn lua(&self) -> Option<&mlua::Lua> {
        self.lua.as_ref()
    }

    /// Attach the live editor handle + message channel + clock and install the `wc.*` editor
    /// API (`wc.text`, `wc.insert`, …) into the VM. A no-op on the null host (nothing to
    /// install). Idempotent w.r.t. call count is NOT guaranteed — call once, at the point the
    /// `Rc<RefCell<Editor>>` wrap first exists (`run()`'s job in Task 7; tests call it directly
    /// with a `TestClock`).
    pub fn attach_bridge(
        &mut self,
        editor: Rc<RefCell<Editor>>,
        msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
        clock: Rc<dyn Clock>,
    ) -> mlua::Result<()> {
        let Some(lua) = self.lua.as_ref() else { return Ok(()) };
        let invoke_state = Rc::new(RefCell::new(InvokeState { current: None, observer: false }));
        let bridge = Bridge { editor, msg_tx, clock, invoke_state };
        crate::plugin::api::install_editor_api(lua, &bridge)?;
        self.bridge = Some(bridge);
        Ok(())
    }

    /// Drain-then-invoke, single pass (P1 has no `wc.command` — §5/§1 — so no callback can grow
    /// the queue mid-pump; a re-drain loop and a chain cap are P2 concerns). Takes the HANDLE,
    /// never `&mut Editor`, so no borrow is held across Lua:
    ///
    /// - **Phase A** drains `editor.pending_plugin_calls` under a short `borrow_mut` scope that
    ///   drops immediately (`std::mem::take` into a local `Vec`).
    /// - **Phase B** invokes each drained call's stored Lua callback with NO outer borrow held —
    ///   so each `wc.*` closure's own `try_borrow_mut` succeeds. Every invocation is wrapped in
    ///   [`crate::panicx::catch`] (the spike found `mlua` resumes a raw Rust panic rather than
    ///   converting it — `panicx` is the SOLE backstop, no gaps) and in [`Self::with_time_guard`]
    ///   (the `set_hook` runaway-time abort). No `clock` parameter — the clock lives in the
    ///   bridge, captured by the edit closures at [`Self::attach_bridge`] time.
    pub fn pump(&mut self, editor: &Rc<RefCell<Editor>>) {
        let Some(lua) = self.lua.as_ref() else {
            // Null-host discipline (P2 §3d): fire sites push events UNCONDITIONALLY (they don't
            // know whether a VM is live), so under `--no-plugins` this queue — and the P1 call
            // queue — would grow without bound unless cleared here every pump.
            let mut e = editor.borrow_mut();
            e.pending_plugin_calls.clear();
            e.pending_plugin_events.clear();
            return;
        };
        // Phase A — drain calls + events under one short borrow that drops immediately.
        let (calls, events): (Vec<crate::plugin::PluginCall>, Vec<crate::plugin::PluginEvent>) = {
            let mut e = editor.borrow_mut();
            (
                std::mem::take(&mut e.pending_plugin_calls).into_iter().collect(),
                std::mem::take(&mut e.pending_plugin_events).into_iter().collect(),
            )
        };
        // Phase B — invoke calls with NO outer borrow held.
        for call in calls {
            let key = format!("wc-cmd-{}", call.id.0);
            let outcome: Result<mlua::Result<()>, String> = crate::panicx::catch(|| {
                let cb: mlua::Function = lua.named_registry_value(&key)?;
                self.with_time_guard(lua, || cb.call::<()>(()))
            });
            if let Err(msg) = normalize(outcome) {
                crate::plugin::plugin_error(editor, call.id.0, &msg);
            }
        }
        // Phase C — drain events (P2 §3d). INTERIM: single-pass — hooks are observer-only and
        // cannot enqueue, so a single pass is correct for THIS task; Task 7 rewrites this into
        // the unified re-drain loop (dispatch/call/event) with chain/time caps. No bridge (VM
        // exists but attach_bridge never ran) → nothing to invoke against; the events were
        // already taken above, so this is a harmless drop, not a leak.
        let Some(invoke_state) = self.bridge.as_ref().map(|b| b.invoke_state.clone()) else { return };
        for ev in events {
            for h in self.hooks.iter().filter(|h| h.kind == ev.kind) {
                let key = h.key.clone();
                let label = h.label.clone();
                // RAII guard so a panicking hook can't leak observer mode onto the next unit.
                let _obs = ObserverGuard::enter(invoke_state.clone(), &label);
                let outcome: Result<mlua::Result<()>, String> = crate::panicx::catch(|| {
                    let cb: mlua::Function = lua.named_registry_value(&key)?;
                    let arg = event_table(lua, &ev)?;
                    self.with_time_guard(lua, || cb.call::<()>((arg,)))
                });
                if let Err(msg) = normalize(outcome) {
                    crate::plugin::plugin_error(editor, &label, &msg);
                }
            }
        }
    }

    /// The `set_hook` runaway guard (spike §11.3, GREEN), fixed at [`CALLBACK_TIME_BUDGET`] — a
    /// thin forwarder to the free [`with_time_guard`] fn. Scoped tightly around ONE callback
    /// invocation and ALWAYS removed afterward (`remove_hook`) — via a [`HookGuard`] whose
    /// `Drop` runs on both the normal-return path and an unwind (`f` is the plugin callback;
    /// `panicx::catch`, the pump's sole panic backstop, resumes a raw Rust panic THROUGH this
    /// frame rather than converting it — see [`Self::pump`]) — a leaked hook would fire during
    /// the NEXT unrelated Lua call (registration, another plugin's callback).
    fn with_time_guard<T>(&self, lua: &mlua::Lua, f: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        with_time_guard(lua, CALLBACK_TIME_BUDGET, f)
    }
}

/// The `set_hook` runaway guard, parameterized by budget so load ([`LOAD_TIME_BUDGET`]) and
/// callbacks ([`CALLBACK_TIME_BUDGET`]) share one mechanism. RAII `HookGuard` removes the hook on
/// return AND unwind. `f`'s scope is one exec/callback invocation — an instruction-count hook
/// (every 10k instructions, spike §11.3 GREEN) checks elapsed wall time and aborts with a typed
/// Lua error once `budget` is exceeded.
pub(crate) fn with_time_guard<T>(
    lua: &mlua::Lua,
    budget: Duration,
    f: impl FnOnce() -> mlua::Result<T>,
) -> mlua::Result<T> {
    let start = std::time::Instant::now();
    lua.set_hook(mlua::HookTriggers::new().every_nth_instruction(10_000), move |_lua, _dbg| {
        if start.elapsed() > budget {
            Err(mlua::Error::runtime("plugin: exceeded time budget"))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    let _guard = HookGuard(lua);
    f()
}

/// RAII: removes `with_time_guard`'s `set_hook` on drop — normal return AND unwind alike — so a
/// panicking plugin callback can never leak a stale hook onto the VM (safe Rust; no `unsafe`,
/// per the crate's `#![forbid(unsafe_code)]`).
struct HookGuard<'a>(&'a mlua::Lua);

impl Drop for HookGuard<'_> {
    fn drop(&mut self) {
        self.0.remove_hook();
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

/// The VM heap cap (64 MiB) the Task 1 spike confirmed `set_memory_limit` enforces on vendored
/// Lua 5.4 (item 2: GREEN — an over-cap allocation returns a Lua memory error rather than
/// aborting the process). Kept as a named fn rather than a bare constant so a build on which
/// this finding no longer holds has one place to flip to `None` and drop the cap (spec §7's
/// documented-red allowance) — the always-on registration caps (`limits::PLUGIN_MAX_*`) remain
/// the real bound either way, since `set_memory_limit` never bounds the permanent
/// interned-string leaks Task 4 caps separately.
fn spike_confirmed_mem_cap() -> Option<usize> {
    Some(64 << 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::plugin::load::load_sources;
    use crate::plugin::PluginCall;
    use crate::registry::Registry;
    use crate::test_support::TestClock;
    use proptest::prelude::*;

    fn sources(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(s, src)| (s.to_string(), src.to_string())).collect()
    }

    /// Channel + clock a test bridge needs; `msg_tx` is unused until Task 7 wires `wc.command`
    /// dispatch — a plain unbound channel is enough to satisfy the `Bridge`'s field.
    fn test_bridge_parts() -> (std::sync::mpsc::Sender<crate::app::Msg>, Rc<dyn Clock>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        (tx, Rc::new(TestClock::new(0)))
    }

    /// Register ONE command (`t.cmd`, `fn = <src's function>`) into a fresh `Registry`, attach a
    /// bridge over a fresh `Editor::new_from_text(text, ..)`, and enqueue that command's call —
    /// ready for the test to `host.pump(&editor)` and assert on the outcome.
    fn make(src_fn_body: &str, text: &str) -> (PluginHost, Rc<RefCell<Editor>>, CommandId) {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = format!("wc.register_command{{ name='cmd', label='C', fn=function() {src_fn_body} end }}");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", &src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1), "test plugin must register cleanly: {:?}", reports[0].result);
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text(text, None, (40, 10))));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id });
        (host, editor, id)
    }

    fn whole_text(editor: &Rc<RefCell<Editor>>) -> String {
        let e = editor.borrow();
        let buf = &e.active().document.buffer;
        buf.slice(0..buf.len())
    }

    #[test]
    fn null_host_pumps_noop_on_empty_queue() {
        // The null host holds no VM; pump must be a harmless no-op (no panic, nothing enqueued
        // to drain either way).
        let mut host = PluginHost::null();
        assert!(!host.has_vm());
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "x");
    }

    #[test]
    fn callback_budget_constant_unchanged() {
        // The Task 3 rename (TIME_BUDGET → CALLBACK_TIME_BUDGET) must not move the value —
        // only LOAD_TIME_BUDGET is new.
        assert_eq!(CALLBACK_TIME_BUDGET, std::time::Duration::from_millis(150));
    }

    #[test]
    fn new_host_has_a_live_vm() {
        let host = PluginHost::new().expect("VM construction must succeed");
        assert!(host.has_vm());
        assert!(host.lua().is_some());
    }

    #[test]
    fn pump_runs_enqueued_plugin_command() {
        let (mut host, editor, _id) = make("wc.insert('X')", "hello");
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "Xhello", "the command's wc.insert must land at the caret");
    }

    #[test]
    fn pump_holds_no_borrow_during_lua() {
        // wc.text() then wc.insert() then wc.cursor() in sequence: each call takes and drops
        // its own short borrow — no "editor busy" from an outer borrow the pump might hold.
        let (mut host, editor, _id) = make(
            "local t = wc.text(); wc.insert('Y'); local c = wc.cursor(); wc.status(t .. '/' .. tostring(c))",
            "ab",
        );
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "Yab");
        let status = editor.borrow().status.clone();
        assert_eq!(status, "ab/1", "wc.text saw the pre-insert buffer, wc.cursor the post-insert caret");
    }

    #[test]
    fn wc_replace_reversed_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make("wc.replace(10, 2, 'x')", "hello world");
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "hello world", "reversed range must not mutate the buffer");
        let status = editor.borrow().status.clone();
        assert!(status.contains("plugin"), "status: {status}");
        assert!(!status.to_lowercase().contains("panic"), "must be a clean degrade, not a caught panic: {status}");
    }

    #[test]
    fn wc_replace_oob_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make("wc.replace(0, 999, 'x')", "hello world");
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "hello world", "out-of-bounds range must not mutate the buffer");
        let status = editor.borrow().status.clone();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
    }

    #[test]
    fn wc_replace_mid_char_offset_is_typed_error_no_panic() {
        // "h\u{e9}llo" — 'é' occupies bytes [1,3); byte offset 2 lands mid-char.
        let (mut host, editor, _id) = make("wc.replace(2, 3, 'x')", "h\u{e9}llo");
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "h\u{e9}llo", "a mid-char offset must not mutate the buffer");
        let status = editor.borrow().status.clone();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
    }

    #[test]
    fn wc_text_reversed_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make(
            "local ok, err = pcall(wc.text, 10, 2); wc.status(tostring(ok) .. ':' .. tostring(err))",
            "hi",
        );
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "hi");
        let status = editor.borrow().status.clone();
        assert!(status.starts_with("false:"), "wc.text(10,2) must fail (pcall false), got: {status}");
        assert!(!status.to_lowercase().contains("panic"), "must be a clean degrade, not a caught panic: {status}");
    }

    #[test]
    fn wc_insert_over_paste_max_is_typed_error_buffer_unchanged() {
        // Built inside Lua (`string.rep`) so the Rust-side source stays tiny even though the
        // cap itself is multi-megabyte.
        let (mut host, editor, _id) = make(
            &format!("wc.insert(string.rep('a', {}))", crate::limits::PASTE_MAX_BYTES + 1),
            "doc",
        );
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "doc", "an over-cap insert must never reach a Tendril alloc/ChangeSet");
        let status = editor.borrow().status.clone();
        assert!(status.contains("plugin"), "status: {status}");
    }

    #[test]
    fn wc_status_over_max_is_truncated_on_char_boundary() {
        let (mut host, editor, _id) =
            make(&format!("wc.status(string.rep('a', {}))", crate::limits::PLUGIN_MAX_STATUS_LEN + 100), "doc");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status.len(), crate::limits::PLUGIN_MAX_STATUS_LEN);
        assert!(status.chars().all(|c| c == 'a'));
    }

    #[test]
    fn wc_status_truncation_backs_off_a_split_multibyte_char() {
        let prefix_len = crate::limits::PLUGIN_MAX_STATUS_LEN - 1;
        let (mut host, editor, _id) =
            make(&format!("wc.status(string.rep('a', {prefix_len}) .. '\u{e9}')"), "doc");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        // The 2-byte 'é' straddles the cap; truncation must back off before its lead byte
        // rather than splitting it (which would panic a naive byte-slice) or over-including it.
        assert_eq!(status.len(), prefix_len);
        assert!(status.chars().all(|c| c == 'a'));
    }

    #[test]
    fn lua_error_in_callback_is_isolated_and_reported() {
        let (mut host, editor, _id) = make("error('boom')", "x");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert!(status.contains("boom"), "status: {status}");
        assert_eq!(whole_text(&editor), "x", "editor must be untouched by a callback error");
    }

    #[test]
    fn panicking_callback_is_isolated_and_subsequent_pump_still_works() {
        let mut host = PluginHost::new().expect("VM construction");
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");

        // A raw Lua-callable Rust closure that panics, stored directly under the pump's
        // expected named-registry key — bypassing wc.register_command entirely. The Task 1
        // spike found mlua does NOT convert a Rust panic to an mlua::Error (it resumes the raw
        // panic), so this exercises panicx::catch as the sole backstop, not a redundant one.
        let panic_id = CommandId(crate::plugin::intern("panic-test.boom"));
        {
            let lua = host.lua().expect("live VM");
            let f = lua
                .create_function(|_, ()| -> mlua::Result<()> { panic!("callback panic") })
                .expect("create_function");
            lua.set_named_registry_value(&format!("wc-cmd-{}", panic_id.0), f).expect("store callback");
        }
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: panic_id });
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert!(status.contains("callback panic"), "status: {status}");
        assert_eq!(whole_text(&editor), "x", "a panicking callback must not touch the editor");

        // A subsequent pump of a normal command still runs — the panic did not poison the
        // VM or the pump loop.
        let mut reg = Registry::builtins();
        let src = "wc.register_command{ name='good', label='Good', fn=function() wc.insert('G') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("u", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let good_id = reg.resolve_name("u.good").expect("registered");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: good_id });
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "Gx");
    }

    #[test]
    fn editor_busy_on_nested_reentry_degrades_not_panics() {
        // White-box: hold a borrow across the call to force the defensive try_borrow_mut Err
        // path — the normal pump path never does this (Phase A's borrow is always gone before
        // Phase B invokes any callback).
        let mut host = PluginHost::new().expect("VM construction");
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");

        let held = editor.borrow_mut(); // simulate a nested re-entry
        let lua = host.lua().expect("live VM");
        let result: mlua::Result<()> = lua.load("wc.insert('Z')").exec();
        drop(held);

        let err = result.expect_err("a nested borrow must degrade, not succeed");
        assert!(err.to_string().contains("editor busy"), "error: {err}");
        assert_eq!(whole_text(&editor), "x", "no mutation happened under the nested borrow");
    }

    #[test]
    fn wc_set_selection_in_bounds_sets_selection() {
        let (mut host, editor, _id) = make("wc.set_selection(1, 3)", "hello");
        host.pump(&editor);
        let sel = editor.borrow().active().document.selection.primary();
        assert_eq!((sel.anchor, sel.head), (1, 3));
    }

    #[test]
    fn wc_set_selection_out_of_bounds_snaps_no_panic() {
        // "hi" has len 2; head=999 is far out of bounds — must SNAP to buf.len() (the required
        // TDD case), never reject/error/panic.
        let (mut host, editor, _id) = make("wc.set_selection(0, 999)", "hi");
        host.pump(&editor);
        let sel = editor.borrow().active().document.selection.primary();
        assert_eq!((sel.anchor, sel.head), (0, 2), "head snapped to buffer length, not rejected");
        let status = editor.borrow().status.clone();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
        assert!(!status.contains("plugin:"), "must succeed silently, no typed error: {status}");
    }

    #[test]
    fn wc_selection_returns_the_current_selection() {
        let (mut host, editor, _id) = make(
            "wc.set_selection(1, 3); local s = wc.selection(); wc.status(tostring(s.anchor) .. ':' .. tostring(s.head))",
            "hello",
        );
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "1:3");
    }

    #[test]
    fn wc_len_returns_buffer_byte_length() {
        let (mut host, editor, _id) = make("wc.status(tostring(wc.len()))", "hello");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "5");
    }

    #[test]
    fn wc_version_returns_buffer_version_after_an_edit() {
        let (mut host, editor, _id) = make("wc.insert('x'); wc.status(tostring(wc.version()))", "y");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "1", "version bumps once per applied transaction");
    }

    #[test]
    fn wc_path_is_nil_for_an_unsaved_buffer() {
        let (mut host, editor, _id) = make("wc.status(tostring(wc.path()))", "x");
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "nil");
    }

    #[test]
    fn wc_path_returns_the_path_for_a_named_buffer() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='cmd', label='C', fn=function() wc.status(tostring(wc.path())) end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text(
            "x",
            Some(std::path::PathBuf::from("/tmp/note.md")),
            (40, 10),
        )));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id });

        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "/tmp/note.md");
    }

    #[test]
    fn wc_register_command_after_load_errors_not_silent_noop() {
        // A command callback calling wc.register_command post-load must degrade to a typed Lua
        // error surfaced via plugin_error — never a silent no-op into a never-drained sink, and
        // never a new command reaching the (already-frozen) Registry.
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='cmd', label='C', fn=function() \
                       wc.register_command{ name='evil', label='Evil', fn=function() end } \
                   end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1), "the OUTER registration, at load time, still works");
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");
        let before = reg.commands().count();

        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id });

        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert!(status.contains("register_command"), "status: {status}");
        assert!(status.contains("only available during plugin load"), "status: {status}");
        assert_eq!(reg.commands().count(), before, "no new command registered post-load");
        assert!(reg.resolve_name("t.evil").is_none());
        assert_eq!(whole_text(&editor), "x", "no unrelated editor mutation from the degrade");
    }

    // -----------------------------------------------------------------------
    // spec §8's no-panic property test: the whole range-taking wc.* surface
    // (wc.replace, the wc.text read) fuzzed with hostile (a, b, text) via a real pump.
    // -----------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Drives random `(a, b, text)` — including reversed ranges, out-of-bounds offsets, and
        /// mid-char offsets on a multibyte doc — through `wc.replace` AND `wc.text` via a REAL
        /// `PluginHost::pump`. Complements the three hand-picked cases above (`wc_replace_*`/
        /// `wc_text_*`) by proving `plugin_check_range`'s pre-validation (the input-validation
        /// LAW) holds over the WHOLE input space the generator can produce, not just those
        /// cases. Two invariants: (1) no Rust panic ever surfaces as a caught-panic status
        /// message (a genuine panic — as opposed to a clean typed-error degrade — would read
        /// literally "panic" per `panicx::panic_message`'s unknown-payload fallback, or a raw
        /// core panic phrase like "byte index ... is not a char boundary"); (2) buffer
        /// coherence — the only possible mutation (`wc.replace`) can never grow the buffer past
        /// one insert's worth of the supplied text, and the trailing `wc.text` read never
        /// changes the buffer at all (a pure read).
        #[test]
        fn prop_wc_range_surface_never_panics_or_corrupts(
            doc in proptest::collection::vec(
                proptest::sample::select(vec!['a', 'b', 'é', '中', '🙂', '\n']),
                0..=20usize,
            ).prop_map(|cs| cs.into_iter().collect::<String>()),
            a in 0usize..40,
            b in 0usize..40,
            text in proptest::string::string_regex("[aé中]{0,4}").unwrap(),
        ) {
            let mut reg = Registry::builtins();
            let mut host = PluginHost::new().unwrap();
            let src = format!(
                "wc.register_command{{ name='rep', label='R', fn=function() wc.replace({a}, {b}, [[{text}]]) end }}\n\
                 wc.register_command{{ name='rd', label='D', fn=function() wc.text({a}, {b}) end }}"
            );
            let reports = load_sources(&mut reg, &mut host, &sources(&[("fuzz", &src)]), &BTreeMap::new(), &mut Vec::new());
            prop_assert_eq!(reports[0].result.clone(), Ok(2), "both commands must register cleanly");

            let editor = Rc::new(RefCell::new(Editor::new_from_text(&doc, None, (40, 10))));
            let (tx, clock) = test_bridge_parts();
            host.attach_bridge(editor.clone(), tx, clock).unwrap();
            let rep_id = reg.resolve_name("fuzz.rep").unwrap();
            let rd_id = reg.resolve_name("fuzz.rd").unwrap();

            let doc_bytes = doc.len();
            let text_bytes = text.len();

            editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: rep_id });
            host.pump(&editor); // wc.replace's outcome — Ok or a typed error, never a raw panic
            let status_after_replace = editor.borrow().status.clone();
            prop_assert!(!status_after_replace.to_lowercase().contains("panic"),
                "status: {status_after_replace}");
            let after_replace_len = editor.borrow().active().document.buffer.len();
            prop_assert!(after_replace_len <= doc_bytes + text_bytes,
                "buffer grew past the one-insert upper bound: {after_replace_len} > {doc_bytes}+{text_bytes}");

            editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: rd_id });
            host.pump(&editor); // wc.text's outcome — Ok or a typed error, never a raw panic
            let status_after_text = editor.borrow().status.clone();
            prop_assert!(!status_after_text.to_lowercase().contains("panic"),
                "status: {status_after_text}");
            // wc.text is a pure read — the buffer must be byte-identical to after the replace.
            let final_text = whole_text(&editor);
            prop_assert_eq!(final_text.len(), after_replace_len);
        }
    }

    // -----------------------------------------------------------------------
    // P2 Task 6: the event system — wc.on / fire_event / the pump's event-drain phase /
    // observer-only enforcement.
    // -----------------------------------------------------------------------

    /// Load `src` (which may call `wc.on`), attach a fresh bridge over `Editor::new_from_text(text, ..)`,
    /// and return the loaded `(host, editor, reg, reports)` — the shared arrange step for every
    /// event-system test below (mirrors `make`, but without enqueueing a `PluginCall`, since
    /// these tests push `PluginEvent`s instead).
    fn make_hooked(src: &str, text: &str) -> (PluginHost, Rc<RefCell<Editor>>, Registry, Vec<crate::plugin::load::LoadReport>) {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        let editor = Rc::new(RefCell::new(Editor::new_from_text(text, None, (40, 10))));
        let (tx, clock) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx, clock).expect("bridge attaches on a live VM");
        (host, editor, reg, reports)
    }

    #[test]
    fn wc_on_rejects_65th_hook() {
        // The resource-bound LAW's per-plugin hook cap (PLUGIN_MAX_HOOKS_PER_PLUGIN = 64) —
        // mirrors load_rejects_257th_command's shape for the command cap.
        let src = "for i=1,65 do wc.on('save', function() end) end";
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let reports = load_sources(&mut reg, &mut host, &sources(&[("many", src)]), &BTreeMap::new(), &mut Vec::new());
        let err = reports[0].result.as_ref().expect_err("the 65th wc.on must fail the plugin's exec");
        assert!(err.to_lowercase().contains("too many hooks"), "error: {err}");
        assert_eq!(reports[0].hooks, 0, "nothing committed once exec itself failed");
    }

    #[test]
    fn on_save_hook_fires_with_path_payload() {
        let src = "wc.on('save', function(ev) wc.status(ev.kind .. ':' .. tostring(ev.path)) end)";
        let (mut host, editor, _reg, reports) = make_hooked(src, "x");
        assert_eq!(reports[0].result, Ok(0), "a hook-only plugin registers zero commands");
        assert_eq!(reports[0].hooks, 1, "the LoadReport must carry the real committed hook count");

        editor.borrow_mut().pending_plugin_events.push_back(crate::plugin::PluginEvent {
            kind: crate::plugin::PluginEventKind::Save,
            path: Some("/x".to_string()),
        });
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert_eq!(status, "save:/x");
    }

    #[test]
    fn hooks_fire_in_registration_order() {
        let src = "\
            order = {}\n\
            wc.on('save', function(ev) table.insert(order, 'a') end)\n\
            wc.on('save', function(ev) table.insert(order, 'b') end)";
        let (mut host, editor, _reg, reports) = make_hooked(src, "x");
        assert_eq!(reports[0].hooks, 2);

        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Save, path: None });
        host.pump(&editor);

        let lua = host.lua().unwrap();
        let order_table: mlua::Table = lua.globals().get("order").unwrap();
        let order: Vec<String> = order_table.sequence_values::<String>()
            .collect::<mlua::Result<Vec<_>>>().unwrap();
        assert_eq!(order, vec!["a".to_string(), "b".to_string()],
            "two hooks on the same event must fire in registration order");
    }

    #[test]
    fn hook_error_is_isolated_other_hooks_run() {
        let src = "\
            second_ran = false\n\
            wc.on('save', function(ev) error('boom') end)\n\
            wc.on('save', function(ev) second_ran = true end)";
        let (mut host, editor, _reg, reports) = make_hooked(src, "x");
        assert_eq!(reports[0].hooks, 2);

        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Save, path: None });
        host.pump(&editor);

        let status = editor.borrow().status.clone();
        assert!(status.contains("boom"), "the first hook's error must be reported: {status}");
        let lua = host.lua().unwrap();
        let second_ran: bool = lua.globals().get("second_ran").unwrap();
        assert!(second_ran, "a failing hook must not stop the remaining hooks for that event");
        assert_eq!(whole_text(&editor), "x", "editor must be untouched by a failing hook");
    }

    #[test]
    fn event_with_no_hooks_is_dropped() {
        // Only a `save` hook exists; firing `open` (no subscriber) must be a harmless no-op.
        let src = "wc.on('save', function(ev) wc.status('should not run') end)";
        let (mut host, editor, _reg, _reports) = make_hooked(src, "x");
        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Open, path: None });
        host.pump(&editor);
        assert_eq!(editor.borrow().status, "", "an event with no matching hook must invoke nothing");
        assert_eq!(whole_text(&editor), "x");
    }

    #[test]
    fn null_host_clears_event_queue() {
        // --no-plugins: fire sites push events UNCONDITIONALLY, so the null-host pump must
        // clear the queue every cycle rather than leaving it to grow without bound.
        let mut host = PluginHost::null();
        assert!(!host.has_vm());
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Save, path: Some("/x".into()) });
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: CommandId(crate::plugin::intern("null-host-test.cmd")) });
        host.pump(&editor);
        assert!(editor.borrow().pending_plugin_events.is_empty(), "the null host must clear pending_plugin_events");
        assert!(editor.borrow().pending_plugin_calls.is_empty(), "the null host must clear pending_plugin_calls too");
    }

    #[test]
    fn wc_on_after_load_errors() {
        // Mirrors wc_register_command_after_load_errors_not_silent_noop for the second
        // registration verb: a callback calling wc.on post-load must degrade to a typed error,
        // never a silent no-op into a never-drained sink.
        let src = "wc.register_command{ name='cmd', label='C', fn=function() \
                       wc.on('save', function() end) \
                   end }";
        let (mut host, editor, reg, reports) = make_hooked(src, "x");
        assert_eq!(reports[0].result, Ok(1), "the OUTER command registration, at load time, still works");
        assert_eq!(reports[0].hooks, 0, "no hook was registered at LOAD time by this plugin");
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id });
        host.pump(&editor);
        let status = editor.borrow().status.clone();
        assert!(status.contains("wc.on"), "status: {status}");
        assert!(status.contains("only available during plugin load"), "status: {status}");
        assert_eq!(whole_text(&editor), "x", "no unrelated editor mutation from the degrade");
    }

    #[test]
    fn hooks_commit_atomically_with_commands() {
        // A self-colliding plugin (same command name registered twice) must commit ZERO
        // commands AND zero hooks — the atomic-per-plugin guarantee now spans both verbs.
        let src = "\
            wc.on('save', function() end)\n\
            wc.register_command{ name='x', label='X1', fn=function() end }\n\
            wc.register_command{ name='x', label='X2', fn=function() end }";
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("atomic", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err(), "the self-collision must fail the plugin");
        assert_eq!(reports[0].hooks, 0, "a failed plugin must commit zero hooks too");
        assert!(reg.resolve_name("atomic.x").is_none());
        // No wc-ev-atomic-0 key should have been written either — the same two-phase-commit
        // discipline the command keys already get.
        assert!(
            host.lua().unwrap().named_registry_value::<mlua::Function>("wc-ev-atomic-0").is_err(),
            "no wc-ev-<stem>-<i> hook key may exist for a plugin that failed preflight"
        );
    }

    /// The observer-only binding constraint (P2 §3e): while a hook runs, `wc.insert`/
    /// `wc.replace`/`wc.set_selection` are BLOCKED with a typed error and the buffer/selection
    /// are UNCHANGED; `wc.status` is the one allowed mutation. A normal command callback
    /// dispatched afterward (via `pending_plugin_calls`, NOT an event) still succeeds — the
    /// `InvokeState::observer` flag resets after the hook, including after a hook that PANICS
    /// (the `ObserverGuard`'s `Drop` runs on unwind too).
    #[test]
    fn hook_cannot_edit_observer_guard() {
        let src = "\
            wc.register_command{ name='good', label='G', fn=function() wc.insert('G') end }\n\
            wc.on('save', function(ev)\n\
                local o1, e1 = pcall(wc.insert, 'X')\n\
                local o2, e2 = pcall(wc.replace, 0, 0, 'X')\n\
                local o3, e3 = pcall(wc.set_selection, 0, 1)\n\
                ok1, err1, ok2, ok3 = o1, tostring(e1), o2, o3\n\
            end)\n\
            wc.on('save', function(ev) wc.status('ok') end)";
        let (mut host, editor, reg, reports) = make_hooked(src, "hello");
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[0].hooks, 2);

        let sel_before = editor.borrow().active().document.selection.primary();
        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Save, path: None });
        host.pump(&editor);

        let lua = host.lua().unwrap();
        let ok1: bool = lua.globals().get("ok1").unwrap();
        let ok2: bool = lua.globals().get("ok2").unwrap();
        let ok3: bool = lua.globals().get("ok3").unwrap();
        assert!(!ok1, "wc.insert must be blocked from a hook");
        assert!(!ok2, "wc.replace must be blocked from a hook");
        assert!(!ok3, "wc.set_selection must be blocked from a hook");
        let err1: String = lua.globals().get("err1").unwrap();
        assert!(err1.contains("editing is not allowed from an event hook"), "err1: {err1}");

        assert_eq!(whole_text(&editor), "hello", "a blocked edit must not mutate the buffer");
        let sel_after = editor.borrow().active().document.selection.primary();
        assert_eq!((sel_before.anchor, sel_before.head), (sel_after.anchor, sel_after.head),
            "a blocked wc.set_selection must not move the selection");
        assert_eq!(editor.borrow().status, "ok",
            "wc.status must STILL succeed from a hook — the one allowed mutation surface");

        // A raw Rust-panicking "hook" — bypassing wc.on entirely, mirroring
        // panicking_callback_is_isolated_and_subsequent_pump_still_works — proves the
        // ObserverGuard's Drop resets `observer` on UNWIND too, not just normal return.
        let panic_kind = crate::plugin::PluginEventKind::Open;
        let panic_key = "wc-ev-panic-test-0".to_string();
        {
            let f = lua.create_function(|_, _t: mlua::Table| -> mlua::Result<()> {
                panic!("hook panic")
            }).expect("create_function");
            lua.set_named_registry_value(&panic_key, f).expect("store raw panic hook");
        }
        host.hooks.push(HookEntry { kind: panic_kind, key: panic_key, label: "panic-test.on_open".to_string() });
        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: panic_kind, path: None });
        host.pump(&editor);
        let status_after_panic = editor.borrow().status.clone();
        assert!(status_after_panic.contains("hook panic"), "status: {status_after_panic}");

        // The flag must be reset — a normal (non-hook) command callback's wc.insert must still
        // succeed, proving observer mode did not leak past either the clean-error hook or the
        // panicking one.
        let good_id = reg.resolve_name("t.good").expect("registered");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: good_id });
        host.pump(&editor);
        assert_eq!(whole_text(&editor), "Ghello",
            "a normal command's wc.insert must still work after an observer-mode hook (incl. a panicking one)");
    }
}
