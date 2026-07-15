//! In-process Lua plugin commands (Effort P1). A single `mlua` VM per app instance, hosted by
//! [`host::PluginHost`], registers commands into the existing [`crate::registry::Registry`]
//! (`host`/`api`) and is populated by the filesystem/config loader (`load`). The registry seam
//! ([`PluginCall`] + [`intern`]) lets the `Plugin` dispatch arm enqueue without running any Lua;
//! [`host::PluginHost::pump`] is the sole place a queued call's Lua callback actually runs, and
//! [`plugin_error`] is this module's single formatting/routing point for every plugin failure
//! (a caught panic, a Lua `error()`, or a typed API error) into `editor.status`.
pub mod host;
mod pump;
pub mod api;
pub mod load;
pub mod reload;
pub mod settings;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Mutex;

use crate::editor::Editor;
use crate::registry::CommandId;

/// A queued plugin-command invocation. The pump (Task 5) looks up the Lua callback by `id` and
/// passes `arg` as the callback's first parameter. Lives on [`crate::editor::Editor`] so both
/// registry dispatch and the pump reach it. No longer `Copy` — `arg` is an owned `String` (Task
/// 5's parameterized-commands widening).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCall {
    pub id: CommandId,
    /// The already-collected argument (from `wc.command(name, arg)` or a resolved `PluginArg`
    /// minibuffer prompt), or `None` for a nullary command.
    pub arg: Option<String>,
}

/// The P2/P3 event kinds (exhaustive — adding a kind is a deliberate act every match
/// handles). `Change` (P3 §6) is a debounced buffer-content notification, distinct from the
/// P2 cold-path save/open/close trio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEventKind {
    Save,
    Open,
    BufferClose,
    Change,
}

/// One fired event, queued on [`crate::editor::Editor::pending_plugin_events`]. Payload is
/// OWNED (by drain time the buffer may be gone/changed), path clamped to
/// [`crate::limits::PLUGIN_MAX_EVENT_PAYLOAD`] at capture.
#[derive(Clone, Debug)]
pub struct PluginEvent {
    pub kind: PluginEventKind,
    pub path: Option<String>,
}

/// One discovered plugin's load outcome, for `plugin_list` + reload reporting (owned, bounded by
/// the discover/load caps). `error: None` means the plugin loaded cleanly; `commands`/`hooks` are
/// its committed counts (both `0` alongside an `error`, since a failed plugin commits nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRecord {
    pub name: String,
    pub commands: usize,
    pub hooks: usize,
    pub error: Option<String>,
}

/// A queued `wc.command` dispatch (fire-and-forget). `origin` names the requesting plugin
/// cmd/hook for error attribution; `name` is the raw target (resolved at drain — no call-time
/// registry snapshot; see `plugin::host::PluginHost::pump`'s `drain_one_dispatch`). `arg` is the
/// optional 2nd `wc.command(name, arg)` argument, threaded to `Registry::dispatch_with_arg`
/// (Task 5).
#[derive(Clone, Debug)]
pub struct PluginDispatch {
    pub origin: String,
    pub name: String,
    pub arg: Option<String>,
}

/// One armed plugin timer (P3 §3). The callback lives in the VM named registry under `key` (dies with
/// the VM at reload); this struct is the SCHEDULE half, stored on `Editor` (so `next_wake(&Editor,_)`
/// can see the next-due) and auto-disarmed by `Editor::clear_plugin_wake_state`.
#[derive(Clone, Debug)]
pub struct PluginTimer {
    pub handle: u64,          // opaque handle returned to Lua (monotonic, never reused)
    pub origin: String,       // owning plugin STEM (per-plugin cap + wc.timer_cancel scoping); the
                               // part of InvokeState.current before the first '.', NOT the full command id
    pub key: String,          // "wc-timer-<handle>" — the VM-registry callback key
    pub next_due_ms: u64,     // wall-clock ms of the next fire
    pub interval_ms: u64,     // >= PLUGIN_TIMER_MIN_INTERVAL_MS (floor-checked at arm)
    pub repeat: bool,         // false = one-shot (remove after firing); true = reschedule-from-completion
    pub pending: bool,        // true ONLY while this timer's callback is in flight (one-pending-per-timer)
}

/// Parse a hook event name (the `menu_from_str` parse-to-enum precedent — the enum IS the
/// bound). An unknown name is a typed Lua error at `wc.on` time, never stored, never interned.
pub fn event_from_str(s: &str) -> Option<PluginEventKind> {
    match s {
        "save" => Some(PluginEventKind::Save),
        "open" => Some(PluginEventKind::Open),
        "buffer_close" => Some(PluginEventKind::BufferClose),
        "change" => Some(PluginEventKind::Change),
        _ => None,
    }
}

/// The inverse of [`event_from_str`] — the string a fired event's `{ kind = … }` table payload
/// carries back to the hook callback.
pub(crate) fn kind_str(k: PluginEventKind) -> &'static str {
    match k {
        PluginEventKind::Save => "save",
        PluginEventKind::Open => "open",
        PluginEventKind::BufferClose => "buffer_close",
        PluginEventKind::Change => "change",
    }
}

/// Capture-and-enqueue an event at a fire site (cold-path only — save/open/close; never
/// per-keystroke). One clamp + one push; drained the same frame by the pump. Path clamped at
/// capture (resource-bound LAW: the queue holds bounded owned data even for a pathological
/// path).
pub(crate) fn fire_event(editor: &mut Editor, kind: PluginEventKind, path: Option<&std::path::Path>) {
    let path = path.map(|p| cap_status(p.to_string_lossy().as_bytes(), crate::limits::PLUGIN_MAX_EVENT_PAYLOAD));
    editor.pending_plugin_events.push_back(PluginEvent { kind, path });
}

/// The intern pool backing [`intern`] — module-level (not function-local) so the test-only
/// [`intern_pool_len`] reader can share the SAME static rather than a lookalike copy.
static INTERN_POOL: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// Intern a runtime string to `&'static str` (leak-once). PERMANENT — callers MUST cap length
/// and count on the raw `String` BEFORE calling this (resource-bound LAW). De-dupes so
/// re-interning an equal string does not leak twice.
///
/// # Examples
/// ```
/// # use wordcartel::plugin::intern;
/// let a = intern("my-plugin.my-command");
/// let b = intern("my-plugin.my-command");
/// assert_eq!(a, b);
/// ```
pub fn intern(s: &str) -> &'static str {
    let mut g = INTERN_POOL.lock().expect("intern pool");
    let set = g.get_or_insert_with(HashSet::new);
    if let Some(existing) = set.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// Test-only membership reader over [`intern`]'s pool — lets a guardrail assert "this failed
/// load leaked nothing" (P2 §7b two-phase commit) by probing for a string the failed load would
/// have interned. `INTERN_POOL` is process-wide, shared by every test in the binary — under
/// `cargo test`'s default parallel execution, a whole-pool-SIZE before/after comparison is racy
/// (an unrelated test interning any new string concurrently perturbs the count; reproduced
/// empirically — a full-suite run saw the count drift by 2 during a single guardrail test's
/// window). A MEMBERSHIP probe on one specific string is race-free instead, as long as that
/// string is unique to the scenario under test (no other test in the suite ever interns the same
/// literal) — a concurrently-running unrelated test cannot perturb whether ITS strings are absent.
#[cfg(test)]
pub(crate) fn intern_pool_contains(s: &str) -> bool {
    INTERN_POOL.lock().expect("intern pool").as_ref().is_some_and(|set| set.contains(s))
}

/// The single formatting/routing point for all plugin errors (spec §7's `plugin_error(editor,
/// name, err)` seam) — a caught panic, a Lua `error()`, or a typed API error surfaced from a
/// callback — into `editor.status` (never the console; `print_*`/`dbg!` remain deny-lints).
/// Called ONLY from [`host::PluginHost::pump`], which never holds an outer borrow across Lua
/// (Phase A's drain is gone by the time Phase B's callback runs), so the plain `borrow_mut`
/// here is exactly as safe as every `wc.*` closure's own — no `try_borrow_mut` needed. Truncated
/// on a char boundary the same way `wc.status` is — a callback failure is not exempt from the
/// resource-bound LAW just because the message originates on the Rust side. Caps `msg` on its
/// BORROWED bytes to the budget left after the `"plugin {name}: "` prefix — the same
/// bound-before-alloc shape as `wc.status` — so a multi-KB `mlua::Error` message (`host::normalize`'s
/// one unavoidable `e.to_string()`, itself bounded only by the VM heap cap) is truncated BEFORE
/// the `format!` runs, never after: the old code formatted the full message first and only then
/// capped the result, a needless double allocation of the untruncated string.
pub(crate) fn plugin_error(editor: &Rc<RefCell<Editor>>, name: &str, msg: &str) {
    let prefix_len = "plugin ".len() + name.len() + ": ".len();
    let budget = crate::limits::PLUGIN_MAX_STATUS_LEN.saturating_sub(prefix_len);
    let capped_msg = cap_status(msg.as_bytes(), budget);
    let mut e = editor.borrow_mut();
    e.set_status(crate::status::StatusKind::Info, format!("plugin {name}: {capped_msg}"));
}

/// Truncate `bytes` to at most `max` bytes, backing off to the nearest UTF-8 char boundary —
/// never splits a multi-byte sequence — then decode (lossily: a raw plugin-supplied byte string
/// is not guaranteed to be valid UTF-8, and this must never panic). Shared by `api::install_status`
/// (on the Lua-BORROWED bytes, before any Rust allocation — the resource-bound LAW's
/// borrowed-length-check-then-convert pattern) and [`plugin_error`] (on an already-owned
/// message), so every string that reaches `editor.status` shares one bound
/// (`limits::PLUGIN_MAX_STATUS_LEN`).
pub(crate) fn cap_status(bytes: &[u8], max: usize) -> String {
    let mut end = bytes.len().min(max);
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_from_str_parses_change() {
        assert_eq!(event_from_str("change"), Some(PluginEventKind::Change));
        assert_eq!(kind_str(PluginEventKind::Change), "change");
    }

    #[test]
    fn intern_is_stable() {
        let a = intern("intern-stable-test.cmd");
        let b = intern("intern-stable-test.cmd");
        assert_eq!(a.as_ptr(), b.as_ptr(), "re-interning an equal string must not leak twice");
        let c = intern("intern-stable-test.other");
        assert_ne!(a, c);
    }

    #[test]
    fn cap_status_passes_short_strings_through_unchanged() {
        assert_eq!(cap_status(b"hello", 100), "hello");
    }

    #[test]
    fn cap_status_truncates_ascii_at_the_exact_max() {
        let s = "a".repeat(10);
        assert_eq!(cap_status(s.as_bytes(), 5), "aaaaa");
    }

    #[test]
    fn cap_status_backs_off_a_split_multibyte_char() {
        // 'é' is 2 bytes; a max landing on its second byte must back off before the char
        // entirely rather than emitting a corrupted/split codepoint.
        let s = "a\u{e9}"; // 1-byte 'a' + 2-byte 'é' = 3 bytes total
        assert_eq!(cap_status(s.as_bytes(), 2), "a");
    }

    #[test]
    fn cap_status_never_panics_on_max_zero() {
        assert_eq!(cap_status(b"hello", 0), "");
    }

    #[test]
    fn plugin_error_sets_and_truncates_editor_status() {
        let editor = Rc::new(RefCell::new(Editor::new_from_text("x", None, (40, 10))));
        plugin_error(&editor, "t.cmd", "boom");
        assert_eq!(editor.borrow().status_text(), "plugin t.cmd: boom");

        let long = "z".repeat(crate::limits::PLUGIN_MAX_STATUS_LEN + 50);
        plugin_error(&editor, "t.cmd", &long);
        assert_eq!(editor.borrow().status_text().len(), crate::limits::PLUGIN_MAX_STATUS_LEN);
    }
}
