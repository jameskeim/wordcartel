//! The plugin VM host: owns the one `mlua` VM + bridge + committed hooks, and the VM's
//! lifecycle. `null()` is the no-VM host used for `--no-plugins`, load failure, and tests that
//! don't exercise plugins (mirrors the empty `ProviderSet`, diag_provider.rs's hermetic default).
//! Task 4 adds the real VM ([`PluginHost::new`])
//! and the registration sink ([`PendingReg`]) the load layer (`plugin::load`) drains into the
//! `Registry` after each plugin's script executes, atomically per plugin. Task 5 adds the
//! [`Bridge`] (the live `Rc<RefCell<Editor>>` + `Sender<Msg>` + clock the `wc.*` editor API
//! closures capture, installed by [`PluginHost::attach_bridge`]). The re-drain pump itself
//! ([`PluginHost::pump`] — the only place plugin Lua callbacks run) is a sibling module,
//! `plugin::pump` (module-size seam, P2 Task 7): its `impl PluginHost` block lives there and
//! reaches this module's `lua`/`bridge`/`hooks` fields via `pub(super)`.

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
    pub arg: Option<String>, // raw arg-prompt, cap-checked; interned only at commit (Task 5)
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
    /// A17 T9 §9.3 — the plugin emit-side display-slot rate-limit, shared between every `wc.*`
    /// emit closure (`install_status`/`install_notify`, `plugin::api`) and the pump
    /// (`plugin::pump::PluginHost::pump`, which advances the tick once per cycle). Same
    /// `Rc<RefCell<_>>`-shared-cell shape as `invoke_state` above, for the same reason: the
    /// closures are `'static` and outlive this `&Bridge` borrow.
    pub(crate) emit_throttle: Rc<RefCell<EmitThrottle>>,
}

/// What the pump is currently invoking, shared with every `wc.*` closure (each captures a
/// clone). `current` names the running plugin command/hook — `wc.command`'s own error
/// attribution (`origin`, §5a) reads it. `observer` is true exactly while a HOOK runs — the
/// edit APIs AND `wc.command` check it and degrade to a typed error (the observer-only binding
/// constraint, enforced in code, not by trust).
pub(crate) struct InvokeState {
    pub current: Option<String>,
    pub observer: bool,
}

/// A17 T9 §9.3 — per-source-label emit-side rate-limit state for the plugin display-slot path. A
/// looping plugin (`while true do wc.status('x') end`-style, or a hot on-edit hook) must not
/// repaint the status slot every callback: at most [`crate::limits::MESSAGES_EMIT_MAX_PER_TICK`]
/// slot updates are admitted per pump TICK (one [`crate::plugin::pump::PluginHost::pump`] cycle —
/// [`EmitThrottle::advance_tick`] is called once at the top of every `pump()`, so a runaway Lua
/// loop that fires many emits from a SINGLE callback invocation — all within one pump cycle —
/// shares one tick's quota) per source LABEL (`InvokeState::current` — there is no stable plugin
/// id, §3.5; a `None` label shares one conservative bucket, so it can only over-throttle, never
/// under-throttle, relative to a hypothetical stable-id scheme). Excess emits still reach history
/// (`Editor::record_status_history_only`) — only the slot write is dropped.
#[derive(Default)]
pub(crate) struct EmitThrottle {
    tick: u64,
    // label -> (tick it was last admitted on, how many admissions happened that tick).
    admitted: std::collections::HashMap<Option<String>, (u64, usize)>,
}

impl EmitThrottle {
    /// Advance to a new tick — called once per [`crate::plugin::pump::PluginHost::pump`] cycle,
    /// before any unit runs, so every emit during that cycle (including all iterations of a
    /// single runaway callback's loop) shares the new tick's quota.
    pub(crate) fn advance_tick(&mut self) {
        self.tick += 1;
    }

    /// `true` iff a display-slot update for `label` is admitted THIS tick (and records the
    /// admission so the next call for the same label/tick sees the updated count); `false` means
    /// the caller must write history-only instead of touching the slot.
    pub(crate) fn admit(&mut self, label: &Option<String>) -> bool {
        let entry = self.admitted.entry(label.clone()).or_insert((0, 0));
        if entry.0 != self.tick {
            *entry = (self.tick, 0); // a new tick resets this label's count
        }
        if entry.1 < crate::limits::MESSAGES_EMIT_MAX_PER_TICK {
            entry.1 += 1;
            true
        } else {
            false
        }
    }
}

/// A committed hook: its VM-registry key + kind + a label for `plugin_error` attribution.
/// Owned `String`s on the host — never interned (unlike command ids): hooks die with the VM at
/// reload, so interning them would be a reload-shaped leak. `Clone` lets the pump's
/// `hooks_for` snapshot the matching hooks out of `self.hooks` before invoking any of them —
/// no borrow of `self.hooks` is held across a callback (mirrors Phase A's "one short borrow,
/// nothing held across Lua" discipline, applied to the hook table too).
#[derive(Clone)]
pub struct HookEntry {
    pub kind: crate::plugin::PluginEventKind,
    pub key: String,
    pub label: String,
}

/// The plugin VM host. `lua: None` is the null host (no VM, no plugins); `lua: Some(_)` owns
/// the one real `mlua::Lua` for this app instance. `bridge` is `None` until
/// [`PluginHost::attach_bridge`] installs it (and always `None` for the null host). Fields are
/// `pub(super)` (not private) so `plugin::pump`'s `impl PluginHost` block — the module-size
/// seam the re-drain loop lives in (P2 Task 7) — can reach them.
pub struct PluginHost {
    pub(super) lua: Option<mlua::Lua>, // None in the null host
    pub(super) bridge: Option<Bridge>,
    /// Committed `wc.on` hooks across every loaded plugin, in load-then-registration order —
    /// owned, NOT interned (die with the VM at reload; P2 §3b).
    pub(super) hooks: Vec<HookEntry>,
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
    /// that don't exercise plugins (mirrors the empty `ProviderSet`, diag_provider.rs's
    /// hermetic default).
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
        // P3 §3g: recompute BEFORE `editor` moves into the `Bridge` — the host's `hooks` are
        // already committed by this point (load_phase/load_sources runs before attach_bridge on
        // both the cold-start and reload paths), so this reflects the real subscriber set.
        editor.borrow_mut().has_on_change_subscriber =
            self.hooks.iter().any(|h| h.kind == crate::plugin::PluginEventKind::Change);
        let invoke_state = Rc::new(RefCell::new(InvokeState { current: None, observer: false }));
        let emit_throttle = Rc::new(RefCell::new(EmitThrottle::default()));
        let bridge = Bridge { editor, msg_tx, clock, invoke_state, emit_throttle };
        crate::plugin::api::install_editor_api(lua, &bridge)?;
        self.bridge = Some(bridge);
        Ok(())
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

    /// Channel + clock a test bridge needs — a plain unbound channel is enough to satisfy the
    /// `Bridge`'s field.
    fn test_bridge_parts() -> (std::sync::mpsc::Sender<crate::app::Msg>, Rc<dyn Clock>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        (tx, Rc::new(TestClock::new(0)))
    }

    /// Register ONE command (`t.cmd`, `fn = <src's function>`) into a fresh `Registry`, attach a
    /// bridge over a fresh `Editor::new_from_text(text, ..)`, and enqueue that command's call —
    /// ready for the test to `host.pump_test(&editor)` and assert on the outcome.
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });
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
        host.pump_test(&editor);
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
        host.pump_test(&editor);
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
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "Yab");
        let status = editor.borrow().status_text().to_string();
        assert_eq!(status, "ab/1", "wc.text saw the pre-insert buffer, wc.cursor the post-insert caret");
    }

    #[test]
    fn wc_replace_reversed_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make("wc.replace(10, 2, 'x')", "hello world");
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "hello world", "reversed range must not mutate the buffer");
        let status = editor.borrow().status_text().to_string();
        assert!(status.contains("plugin"), "status: {status}");
        assert!(!status.to_lowercase().contains("panic"), "must be a clean degrade, not a caught panic: {status}");
    }

    #[test]
    fn wc_replace_oob_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make("wc.replace(0, 999, 'x')", "hello world");
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "hello world", "out-of-bounds range must not mutate the buffer");
        let status = editor.borrow().status_text().to_string();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
    }

    #[test]
    fn wc_replace_mid_char_offset_is_typed_error_no_panic() {
        // "h\u{e9}llo" — 'é' occupies bytes [1,3); byte offset 2 lands mid-char.
        let (mut host, editor, _id) = make("wc.replace(2, 3, 'x')", "h\u{e9}llo");
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "h\u{e9}llo", "a mid-char offset must not mutate the buffer");
        let status = editor.borrow().status_text().to_string();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
    }

    #[test]
    fn wc_text_reversed_range_is_typed_error_no_panic() {
        let (mut host, editor, _id) = make(
            "local ok, err = pcall(wc.text, 10, 2); wc.status(tostring(ok) .. ':' .. tostring(err))",
            "hi",
        );
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "hi");
        let status = editor.borrow().status_text().to_string();
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
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "doc", "an over-cap insert must never reach a Tendril alloc/ChangeSet");
        let status = editor.borrow().status_text().to_string();
        assert!(status.contains("plugin"), "status: {status}");
    }

    #[test]
    fn wc_status_over_max_is_truncated_on_char_boundary() {
        let (mut host, editor, _id) =
            make(&format!("wc.status(string.rep('a', {}))", crate::limits::PLUGIN_MAX_STATUS_LEN + 100), "doc");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        assert_eq!(status.len(), crate::limits::PLUGIN_MAX_STATUS_LEN);
        assert!(status.chars().all(|c| c == 'a'));
    }

    #[test]
    fn wc_status_truncation_backs_off_a_split_multibyte_char() {
        let prefix_len = crate::limits::PLUGIN_MAX_STATUS_LEN - 1;
        let (mut host, editor, _id) =
            make(&format!("wc.status(string.rep('a', {prefix_len}) .. '\u{e9}')"), "doc");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        // The 2-byte 'é' straddles the cap; truncation must back off before its lead byte
        // rather than splitting it (which would panic a naive byte-slice) or over-including it.
        assert_eq!(status.len(), prefix_len);
        assert!(status.chars().all(|c| c == 'a'));
    }

    // -----------------------------------------------------------------------
    // A17 T9 — wc.status reroute + wc.notify + the emit-side rate-limit (spec §9.1-§9.3).
    // -----------------------------------------------------------------------

    #[test]
    fn wc_status_still_works_as_a_plugin_tagged_info() {
        let (mut host, editor, _id) = make("wc.status('hi')", "x");
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status_text(), "hi");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Info);
        assert!(matches!(e.status().unwrap().source(), crate::status::StatusSource::Plugin { .. }));
    }

    #[test]
    fn wc_notify_error_sets_a_sticky_error_from_a_plugin() {
        let (mut host, editor, _id) = make("wc.notify('error', 'compile failed')", "x");
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn wc_notify_unknown_severity_does_not_emit_a_silent_info() {
        // Unknown severity is a typed Lua error (wrapped in pcall so the plugin doesn't abort),
        // never a silent Info write.
        let (mut host, editor, _id) = make("pcall(function() wc.notify('bogus', 'x') end)", "y");
        host.pump_test(&editor);
        assert!(!editor.borrow().has_visible_status(), "unknown severity must NOT emit a silent Info");
    }

    #[test]
    fn wc_notify_source_is_plugin_tagged_by_label() {
        let (mut host, editor, _id) = make("wc.notify('warning', 'careful')", "x");
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert!(matches!(e.status().unwrap().source(), crate::status::StatusSource::Plugin { .. }),
            "wc.notify must attribute StatusSource::Plugin, not Host");
    }

    #[test]
    fn wc_status_burst_in_one_callback_is_throttled_to_one_slot_update_per_tick() {
        // A single Lua callback invocation runs inside ONE unit of ONE pump cycle — a runaway
        // `while true do wc.status(...) end`-style burst must not repaint the slot on every
        // iteration (§9.3). MESSAGES_EMIT_MAX_PER_TICK == 1, so only the FIRST admitted emit of
        // this tick lands in the display slot; the rest are dropped from the slot but still
        // recorded to history (never silently lost).
        assert_eq!(crate::limits::MESSAGES_EMIT_MAX_PER_TICK, 1,
            "this test's slot-content assertion assumes the v1 quota of 1");
        let (mut host, editor, _id) =
            make("wc.status('a'); wc.status('b'); wc.status('c')", "x");
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status_text(), "a", "only the first emit of the tick wins the display slot");
        let texts: Vec<&str> = e.status_history().entries().iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["a", "b", "c"], "every emit still reaches history, throttled or not");
    }

    #[test]
    fn error_and_warning_emits_bypass_the_burst_throttle() {
        // A17 F2 (final gate): the emit throttle bounds only Info/Log chatter. In one callback,
        // `wc.status('working')` (Info) wins the slot first, then `wc.notify('error','boom')` shares
        // the SAME per-label bucket in the SAME tick — but Error is exempt, so it reaches the normal
        // slot path (and Q1 lets a more-severe Error displace the Info). The visible slot must be the
        // Error, not the throttled-to-history Info. (Pre-fix the Error was demoted to history-only.)
        assert_eq!(crate::limits::MESSAGES_EMIT_MAX_PER_TICK, 1,
            "this test assumes the v1 quota of 1 so the second emit would be throttled if not exempt");
        let (mut host, editor, _id) =
            make("wc.status('working'); wc.notify('error', 'boom')", "x");
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status_text(), "boom", "an Error must bypass the throttle and win the slot");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
    }

    #[test]
    fn wc_status_throttle_admits_again_on_the_next_pump_tick() {
        // Two SEPARATE pump cycles are two separate ticks — the throttle must not carry a
        // label's exhausted quota over to the next cycle (a legitimately-paced plugin, one
        // wc.status per keystroke/command, must never be silently throttled).
        let (mut host, editor, id) = make("wc.status('first')", "x");
        host.pump_test(&editor);
        assert_eq!(editor.borrow().status_text(), "first");
        // A second, separate pump() cycle over the SAME command id: `invoke_call` resolves the
        // callback directly from `call.id` (never through `reg`), so re-enqueueing `id` re-runs
        // the identical 'first' emit — on a NEW tick, which must be freshly admitted.
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });
        host.pump_test(&editor);
        let e = editor.borrow();
        assert_eq!(e.status_text(), "first", "the second tick's emit is admitted (not carried-over-throttled)");
        assert_eq!(e.status_history().entries().len(), 1,
            "identical adjacent messages coalesce via repeat (§5.2), not a second entry");
        assert_eq!(e.status_history().entries().back().unwrap().repeat(), 2);
    }

    #[test]
    fn wc_status_throttle_buckets_none_label_emits_together() {
        // A timer callback's label is "timer#<handle>" (Some), never None in practice — but the
        // throttle's None-label bucket (§9.3's "label-less emits share one conservative bucket")
        // is exercised directly here against `EmitThrottle::admit` (a pure unit, no VM needed):
        // two None-label emits in the SAME tick must share the one-per-tick quota.
        let mut th = crate::plugin::host::EmitThrottle::default();
        th.advance_tick();
        assert!(th.admit(&None), "the first None-label emit this tick is admitted");
        assert!(!th.admit(&None), "a second None-label emit the SAME tick shares the bucket — denied");
        th.advance_tick();
        assert!(th.admit(&None), "a new tick resets the None-label bucket");
    }

    #[test]
    fn lua_error_in_callback_is_isolated_and_reported() {
        let (mut host, editor, _id) = make("error('boom')", "x");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: panic_id, arg: None });
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        assert!(status.contains("callback panic"), "status: {status}");
        assert_eq!(whole_text(&editor), "x", "a panicking callback must not touch the editor");

        // A subsequent pump of a normal command still runs — the panic did not poison the
        // VM or the pump loop.
        let mut reg = Registry::builtins();
        let src = "wc.register_command{ name='good', label='Good', fn=function() wc.insert('G') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("u", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let good_id = reg.resolve_name("u.good").expect("registered");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: good_id, arg: None });
        host.pump_test(&editor);
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
        host.pump_test(&editor);
        let sel = editor.borrow().active().document.selection.primary();
        assert_eq!((sel.anchor, sel.head), (1, 3));
    }

    #[test]
    fn wc_set_selection_out_of_bounds_snaps_no_panic() {
        // "hi" has len 2; head=999 is far out of bounds — must SNAP to buf.len() (the required
        // TDD case), never reject/error/panic.
        let (mut host, editor, _id) = make("wc.set_selection(0, 999)", "hi");
        host.pump_test(&editor);
        let sel = editor.borrow().active().document.selection.primary();
        assert_eq!((sel.anchor, sel.head), (0, 2), "head snapped to buffer length, not rejected");
        let status = editor.borrow().status_text().to_string();
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
        assert!(!status.contains("plugin:"), "must succeed silently, no typed error: {status}");
    }

    #[test]
    fn wc_selection_returns_the_current_selection() {
        let (mut host, editor, _id) = make(
            "wc.set_selection(1, 3); local s = wc.selection(); wc.status(tostring(s.anchor) .. ':' .. tostring(s.head))",
            "hello",
        );
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        assert_eq!(status, "1:3");
    }

    #[test]
    fn wc_len_returns_buffer_byte_length() {
        let (mut host, editor, _id) = make("wc.status(tostring(wc.len()))", "hello");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        assert_eq!(status, "5");
    }

    #[test]
    fn wc_version_returns_buffer_version_after_an_edit() {
        let (mut host, editor, _id) = make("wc.insert('x'); wc.status(tostring(wc.version()))", "y");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
        assert_eq!(status, "1", "version bumps once per applied transaction");
    }

    #[test]
    fn wc_path_is_nil_for_an_unsaved_buffer() {
        let (mut host, editor, _id) = make("wc.status(tostring(wc.path()))", "x");
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });

        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });

        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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

            editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: rep_id, arg: None });
            host.pump_test(&editor); // wc.replace's outcome — Ok or a typed error, never a raw panic
            let status_after_replace = editor.borrow().status_text().to_string();
            prop_assert!(!status_after_replace.to_lowercase().contains("panic"),
                "status: {status_after_replace}");
            let after_replace_len = editor.borrow().active().document.buffer.len();
            prop_assert!(after_replace_len <= doc_bytes + text_bytes,
                "buffer grew past the one-insert upper bound: {after_replace_len} > {doc_bytes}+{text_bytes}");

            editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: rd_id, arg: None });
            host.pump_test(&editor); // wc.text's outcome — Ok or a typed error, never a raw panic
            let status_after_text = editor.borrow().status_text().to_string();
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

    /// P3 §3g: `attach_bridge` recomputes `has_on_change_subscriber` off the host's committed
    /// hooks — true when at least one `wc.on('change', …)` is registered, false otherwise
    /// (mirrors `make_hooked`'s "load then attach" ordering that `attach_bridge` relies on).
    #[test]
    fn attach_bridge_sets_on_change_subscriber() {
        let (_host, editor, _reg, reports) =
            make_hooked("wc.on('change', function(ev) end)", "x");
        assert_eq!(reports[0].hooks, 1);
        assert!(editor.borrow().has_on_change_subscriber, "a change hook must set the subscriber flag");

        let (_host2, editor2, _reg2, reports2) =
            make_hooked("wc.on('save', function(ev) end)", "x");
        assert_eq!(reports2[0].hooks, 1);
        assert!(!editor2.borrow().has_on_change_subscriber,
            "a plugin with only a save hook must NOT set the on_change subscriber flag");
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
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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
        host.pump_test(&editor);

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
        host.pump_test(&editor);

        let status = editor.borrow().status_text().to_string();
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
        host.pump_test(&editor);
        assert_eq!(editor.borrow().status_text(), "", "an event with no matching hook must invoke nothing");
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: CommandId(crate::plugin::intern("null-host-test.cmd")), arg: None });
        editor.borrow_mut().pending_plugin_dispatch.push_back(
            crate::plugin::PluginDispatch { origin: "x".to_string(), name: "select_all".to_string(), arg: None });
        host.pump_test(&editor);
        assert!(editor.borrow().pending_plugin_events.is_empty(), "the null host must clear pending_plugin_events");
        assert!(editor.borrow().pending_plugin_calls.is_empty(), "the null host must clear pending_plugin_calls too");
        assert!(editor.borrow().pending_plugin_dispatch.is_empty(), "the null host must clear pending_plugin_dispatch too");
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
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });
        host.pump_test(&editor);
        let status = editor.borrow().status_text().to_string();
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
        host.pump_test(&editor);

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
        assert_eq!(editor.borrow().status_text(), "ok",
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
        host.pump_test(&editor);
        let status_after_panic = editor.borrow().status_text().to_string();
        assert!(status_after_panic.contains("hook panic"), "status: {status_after_panic}");

        // The flag must be reset — a normal (non-hook) command callback's wc.insert must still
        // succeed, proving observer mode did not leak past either the clean-error hook or the
        // panicking one.
        let good_id = reg.resolve_name("t.good").expect("registered");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: good_id, arg: None });
        host.pump_test(&editor);
        assert_eq!(whole_text(&editor), "Ghello",
            "a normal command's wc.insert must still work after an observer-mode hook (incl. a panicking one)");
    }

    // -----------------------------------------------------------------------
    // P2 Task 7: wc.command + the final unified re-drain pump + chain/time caps.
    // -----------------------------------------------------------------------

    /// A plugin command calling `wc.command("select_all")` (a nullary builtin) has the
    /// builtin's observable effect after the pump — routed through the SAME `Registry::dispatch`
    /// the palette/menu/keys use (contract law 1), never a side channel. Uses the REAL `pump`
    /// (not `pump_test`) since resolving `"select_all"` needs the SAME `reg` the plugin loaded
    /// against.
    #[test]
    fn wc_command_dispatches_a_builtin() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='cmd', label='C', fn=function() wc.command('select_all') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hello", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        let sel = editor.borrow().active().document.selection.primary();
        assert_eq!((sel.anchor, sel.head), (0, 5), "select_all must select the whole 5-byte buffer");
    }

    /// `a.cmd` calls `wc.command("b.cmd")` which inserts `X`; ONE `pump` call (internally a
    /// multi-iteration re-drain) lands `X` — the re-drain loop picks the enqueued dispatch back
    /// up, resolves it to a `Plugin` entry, enqueues the resulting `PluginCall`, and drains that
    /// too, all within the same cycle.
    #[test]
    fn wc_command_chains_a_plugin_command() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src_a = "wc.register_command{ name='cmd', label='A', fn=function() wc.command('b.cmd') end }";
        let src_b = "wc.register_command{ name='cmd', label='B', fn=function() wc.insert('X') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("a", src_a), ("b", src_b)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[1].result, Ok(1));
        let a_id = reg.resolve_name("a.cmd").expect("registered under a.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hi", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: a_id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        assert_eq!(whole_text(&editor), "Xhi",
            "one pump cycle must re-drain the enqueued dispatch through to b.cmd's wc.insert");
    }

    // ── Task 5: parameterized commands — dispatch_with_arg cases 2/3 end-to-end ────────────

    /// `wc.command('b.echo', 'hi')` on a parameterized `b.echo` (`arg = 'Value:'`) — the pump's
    /// `drain_one_dispatch` threads the supplied arg through `dispatch_with_arg`, which enqueues
    /// the `PluginCall` DIRECTLY (case 2): NO minibuffer opens, and the callback's `arg`
    /// parameter receives "hi" in the SAME pump cycle.
    #[test]
    fn wc_command_with_arg_reaches_callback_no_reprompt() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src_a = "wc.register_command{ name='cmd', label='A', fn=function() wc.command('b.echo', 'hi') end }";
        let src_b = "wc.register_command{ name='echo', label='B', arg='Value:', fn=function(arg) wc.insert(arg) end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("a", src_a), ("b", src_b)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[1].result, Ok(1));
        let a_id = reg.resolve_name("a.cmd").expect("registered under a.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("X", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: a_id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        assert!(editor.borrow().minibuffer.is_none(), "a directly-supplied arg must never open the prompt");
        assert_eq!(whole_text(&editor), "hiX",
            "the callback's arg parameter must receive the supplied \"hi\" — no re-prompt round trip");
    }

    /// A palette-style dispatch of a parameterized command (`reg.dispatch`, no arg supplied)
    /// opens the `PluginArg` prompt; answering it (simulated via `minibuffer::intercept`'s real
    /// Enter path) enqueues `PluginCall { id, arg: Some("25") }`, and one `pump` call runs the
    /// callback with that arg — case 3 followed by case 2 completing.
    #[test]
    fn param_command_callback_receives_arg() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='echo', label='Echo', arg='Minutes:', fn=function(arg) wc.insert(arg) end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("t.echo").expect("registered under t.echo");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hi", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);

        // Palette-style dispatch, no arg in hand — case 3: opens the PluginArg prompt.
        {
            let mut e = editor.borrow_mut();
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clock, executor: &ex, msg_tx: tx.clone(), fs: crate::test_support::test_fs() };
            let r = reg.dispatch(id, &mut ctx);
            assert_eq!(r, crate::commands::CommandResult::Handled);
        }
        assert!(editor.borrow().pending_plugin_calls.is_empty(), "nothing enqueued until the prompt is answered");
        assert!(matches!(editor.borrow().minibuffer.as_ref().map(|m| &m.kind),
            Some(crate::minibuffer::MinibufferKind::PluginArg { id: pid }) if *pid == id));

        // Answer the prompt: type "25", then Enter through the real submit path.
        editor.borrow_mut().minibuffer.as_mut().unwrap().insert('2');
        editor.borrow_mut().minibuffer.as_mut().unwrap().insert('5');
        let enter = crossterm::event::KeyEvent {
            code: crossterm::event::KeyCode::Enter,
            modifiers: crossterm::event::KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let msg = crate::app::Msg::Input(crossterm::event::Event::Key(enter));
        {
            let mut e = editor.borrow_mut();
            let ctx = crate::overlays::DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &crate::test_support::test_fs() };
            crate::minibuffer::intercept(msg, &mut e, &ctx);
        }
        assert!(editor.borrow().minibuffer.is_none());
        assert_eq!(editor.borrow().pending_plugin_calls.len(), 1);

        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());
        assert_eq!(whole_text(&editor), "25hi", "the callback must receive the collected \"25\" arg");
    }

    /// H21 XOR regression: the plugin pump drains `wc.command` dispatches UNCONDITIONALLY, so a
    /// plugin timer/event calling `wc.command('<parameterized-cmd>')` (no arg) while ANOTHER
    /// overlay is open (here: the palette) reaches `dispatch_with_arg`'s `(Some(prompt), None)`
    /// arm. That arm must `close_all` before opening its PluginArg minibuffer — otherwise two
    /// overlays would be active at once, violating the single-active invariant the H21 table
    /// (mouse/render/close dispatch) is built on. Asserts EXACTLY one overlay is active after the
    /// pump — the new minibuffer — and the previously-open palette is now closed.
    #[test]
    fn param_command_dispatch_under_open_overlay_holds_xor() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='echo', label='Echo', arg='Value:', fn=function(arg) wc.insert(arg) end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("t.echo").expect("registered under t.echo");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hi", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");

        // Another overlay is already open when the plugin dispatch fires.
        editor.borrow_mut().open_palette();
        assert!(editor.borrow().palette.is_some(), "palette open precondition");

        // Enqueue an arg-less dispatch of the parameterized command — the exact shape a plugin
        // timer/event produces via `wc.command('t.echo')` — and run the real pump.
        editor.borrow_mut().pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
            origin: "t.echo".to_string(), name: "t.echo".to_string(), arg: None,
        });
        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        let e = editor.borrow();
        assert!(e.palette.is_none(), "the previously-open palette must be closed by close_all");
        assert!(matches!(e.minibuffer.as_ref().map(|m| &m.kind),
            Some(crate::minibuffer::MinibufferKind::PluginArg { id: pid }) if *pid == id),
            "the PluginArg minibuffer must be the sole surviving overlay");
        let active = crate::overlays::OverlayId::ALL.iter()
            .filter(|oid| (oid.row().is_active)(&e)).count();
        assert_eq!(active, 1, "exactly one overlay may be active — XOR invariant");
    }

    /// An arg over `PLUGIN_MAX_COMMAND_ARG` passed to `wc.command(name, arg)` is rejected as a
    /// typed `pcall`-able error, with nothing enqueued (mirrors
    /// `wc_command_over_length_and_full_queue_reject`'s over-length-name check).
    #[test]
    fn wc_command_arg_over_cap_is_rejected() {
        let over_cap = crate::limits::PLUGIN_MAX_COMMAND_ARG + 1;
        let (mut host, editor, _id) = make(
            &format!(
                "local ok, err = pcall(wc.command, 'select_all', string.rep('a', {over_cap}))\n\
                 _G.ok = ok"
            ),
            "x",
        );
        host.pump_test(&editor);
        assert!(editor.borrow().pending_plugin_dispatch.is_empty(), "an over-cap arg must not be enqueued");
        let ok: bool = host.lua().unwrap().globals().get("ok").unwrap();
        assert!(!ok, "wc.command with an over-cap arg must be rejected");
    }

    /// `wc.command("nope")` (an unresolvable name) degrades to a `plugin_error` status naming
    /// the ORIGIN (the calling command), never a panic, and never mutates the editor.
    #[test]
    fn wc_command_unknown_name_reports_error() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='cmd', label='C', fn=function() wc.command('nope') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("t", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("t.cmd").expect("registered under t.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        let status = editor.borrow().status_text().to_string();
        assert!(status.contains("t.cmd"), "status must name the origin: {status}");
        assert!(status.contains("nope"), "status must name the unresolved target: {status}");
        assert!(!status.to_lowercase().contains("panic"), "status: {status}");
        assert_eq!(whole_text(&editor), "x", "an unresolved dispatch must not mutate the buffer");
    }

    /// Two independent rejections at the `wc.command` call site (§5a), each surfaced as a typed
    /// `pcall`-able error with nothing queued: a name longer than `PLUGIN_MAX_COMMAND_REF`, and
    /// a dispatch already at `PLUGIN_MAX_PENDING_DISPATCH`. Both checks run purely inside ONE
    /// Lua callback (no pump re-drain involved in the queue-filling loop itself), so the
    /// results are captured in Lua globals — immune to any status-line overwrite from the
    /// pump's SUBSEQUENT draining of the successfully-queued fillers.
    #[test]
    fn wc_command_over_length_and_full_queue_reject() {
        let cap = crate::limits::PLUGIN_MAX_PENDING_DISPATCH;
        let over_len = crate::limits::PLUGIN_MAX_COMMAND_REF + 1;
        let (mut host, editor, _id) = make(
            &format!(
                "local ok1, err1 = pcall(wc.command, string.rep('a', {over_len}))\n\
                 for i=1,{cap} do wc.command('select_all') end\n\
                 local ok2, err2 = pcall(wc.command, 'select_all')\n\
                 g_ok1, g_err1, g_ok2, g_err2 = ok1, tostring(err1), ok2, tostring(err2)"
            ),
            "hello",
        );
        host.pump_test(&editor);

        let lua = host.lua().unwrap();
        let ok1: bool = lua.globals().get("g_ok1").unwrap();
        let err1: String = lua.globals().get("g_err1").unwrap();
        let ok2: bool = lua.globals().get("g_ok2").unwrap();
        let err2: String = lua.globals().get("g_err2").unwrap();
        assert!(!ok1, "an over-length command name must be rejected");
        assert!(err1.contains("too long"), "err1: {err1}");
        assert!(!ok2, "wc.command over the queue cap must be rejected");
        assert!(err2.contains("queue full"), "err2: {err2}");
    }

    /// `wc.on("save", function() wc.command("x") end)` — `wc.command` is BLOCKED from a hook
    /// (observer mode): mutation-by-proxy, and `on_save`→`save` would self-cascade. A typed
    /// error, nothing queued.
    #[test]
    fn wc_command_from_hook_is_rejected() {
        let src = "wc.on('save', function(ev) \
                       local ok, err = pcall(wc.command, 'x'); \
                       wc.status(tostring(ok) .. ':' .. tostring(err)) \
                   end)";
        let (mut host, editor, _reg, reports) = make_hooked(src, "x");
        assert_eq!(reports[0].hooks, 1);

        editor.borrow_mut().pending_plugin_events.push_back(
            crate::plugin::PluginEvent { kind: crate::plugin::PluginEventKind::Save, path: None });
        host.pump_test(&editor);

        let status = editor.borrow().status_text().to_string();
        assert!(status.starts_with("false:"), "wc.command from a hook must be rejected: {status}");
        assert!(status.contains("event hook"), "status: {status}");
        assert!(editor.borrow().pending_plugin_dispatch.is_empty(), "nothing may be queued from a blocked wc.command");
    }

    /// `a.cmd` dispatches `b.cmd` dispatches `a.cmd` … an infinite ping-pong — the chain cap
    /// (`PLUGIN_PUMP_CHAIN_CAP`) must terminate the cascade deterministically: all three queues
    /// cleared, a truncation status set, and the editor's actual CONTENT untouched (only queued
    /// plugin work is advisory and dropped, never partial document mutation).
    #[test]
    fn pump_chain_cap_truncates_pingpong() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let src_a = "wc.register_command{ name='cmd', label='A', fn=function() wc.command('b.cmd') end }";
        let src_b = "wc.register_command{ name='cmd', label='B', fn=function() wc.command('a.cmd') end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("a", src_a), ("b", src_b)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[1].result, Ok(1));
        let a_id = reg.resolve_name("a.cmd").expect("registered under a.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hello", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: a_id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());

        let e = editor.borrow();
        assert!(e.pending_plugin_calls.is_empty(), "the chain-cap trip must clear the call queue");
        assert!(e.pending_plugin_events.is_empty(), "the chain-cap trip must clear the event queue");
        assert!(e.pending_plugin_dispatch.is_empty(), "the chain-cap trip must clear the dispatch queue");
        assert!(e.status_text().to_lowercase().contains("truncat"), "status: {}", e.status_text());
        assert_eq!(e.active().document.buffer.to_string(), "hello",
            "the editor's actual content must be untouched by the truncated cascade");
    }

    /// With each unit deliberately burning ~40ms of real wall time (a spin-wait on `os.clock()`,
    /// safely under `CALLBACK_TIME_BUDGET`'s 150ms per-unit abort), a ping-pong cascade between
    /// two plugin commands crosses `PUMP_CYCLE_TIME_BUDGET` (500ms wall) LONG before it could
    /// ever reach `PLUGIN_PUMP_CHAIN_CAP` (64 units, which at ~40ms/unit would take ~2.5s) —
    /// isolating the wall-clock cap's truncation from the count cap's.
    #[test]
    fn pump_cycle_time_budget_truncates() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().expect("VM construction");
        let busy = "local t0 = os.clock(); while os.clock() - t0 < 0.04 do end";
        let src_a = format!(
            "runs = 0\n\
             wc.register_command{{ name='cmd', label='A', fn=function() {busy}; runs = runs + 1; wc.command('b.cmd') end }}"
        );
        let src_b = format!(
            "wc.register_command{{ name='cmd', label='B', fn=function() {busy}; wc.command('a.cmd') end }}"
        );
        let reports = load_sources(&mut reg, &mut host, &sources(&[("a", &src_a), ("b", &src_b)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[1].result, Ok(1));
        let a_id = reg.resolve_name("a.cmd").expect("registered under a.cmd");

        let editor = Rc::new(RefCell::new(Editor::new_from_text("hello", None, (40, 10))));
        let (tx, clock_rc) = test_bridge_parts();
        host.attach_bridge(editor.clone(), tx.clone(), clock_rc).expect("bridge attaches on a live VM");
        editor.borrow_mut().pending_plugin_calls.push_back(PluginCall { id: a_id, arg: None });

        let ex = crate::jobs::InlineExecutor::default();
        let clock = TestClock::new(0);
        let started = std::time::Instant::now();
        host.pump(&editor, &reg, &ex, &clock, &tx, &crate::test_support::test_fs());
        let elapsed = started.elapsed();

        let e = editor.borrow();
        assert!(e.pending_plugin_calls.is_empty());
        assert!(e.pending_plugin_dispatch.is_empty());
        drop(e);
        let lua = host.lua().unwrap();
        let runs: i64 = lua.globals().get("runs").unwrap();
        assert!(runs > 0, "at least one unit must have run before truncation: runs={runs}");
        assert!(runs < 64, "the wall-clock cap must trip well before the 64-unit chain cap: runs={runs}");
        assert!(elapsed < std::time::Duration::from_millis(2000),
            "the wall-clock cap must bound total pump time far under the chain-cap-only ceiling: {elapsed:?}");
    }
}
