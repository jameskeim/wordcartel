//! In-process Lua plugin commands (Effort P1). A single `mlua` VM per app instance, hosted by
//! [`host::PluginHost`], registers commands into the existing [`crate::registry::Registry`]
//! (`host`/`api`) and is populated by the filesystem/config loader (`load`). The registry seam
//! ([`PluginCall`] + [`intern`]) lets the `Plugin` dispatch arm enqueue without running any Lua;
//! [`host::PluginHost::pump`] is the sole place a queued call's Lua callback actually runs, and
//! [`plugin_error`] is this module's single formatting/routing point for every plugin failure
//! (a caught panic, a Lua `error()`, or a typed API error) into `editor.status`.
pub mod host;
pub mod api;
pub mod load;

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Mutex;

use crate::editor::Editor;
use crate::registry::CommandId;

/// A queued plugin-command invocation. `Copy` id only — the pump (Task 5) looks up the Lua
/// callback by this id. Lives on [`crate::editor::Editor`] so both registry dispatch and the
/// pump reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginCall {
    pub id: CommandId,
}

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
    static POOL: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut g = POOL.lock().expect("intern pool");
    let set = g.get_or_insert_with(HashSet::new);
    if let Some(existing) = set.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
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
    e.status = format!("plugin {name}: {capped_msg}");
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
        assert_eq!(editor.borrow().status, "plugin t.cmd: boom");

        let long = "z".repeat(crate::limits::PLUGIN_MAX_STATUS_LEN + 50);
        plugin_error(&editor, "t.cmd", &long);
        assert_eq!(editor.borrow().status.len(), crate::limits::PLUGIN_MAX_STATUS_LEN);
    }
}
