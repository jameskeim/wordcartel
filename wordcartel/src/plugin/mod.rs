//! In-process Lua plugin commands (Effort P1). A single `mlua` VM per app instance, hosted by
//! [`host::PluginHost`], registers commands into the existing [`crate::registry::Registry`]
//! (`host`/`api`) and is populated by the filesystem/config loader (`load`). Task 3 opens the
//! registry seam ([`PluginCall`] + [`intern`]) — still no Lua runs; the `Plugin` dispatch arm
//! only enqueues. Task 2's skeleton (`PluginHost::null`) remains inert.
pub mod host;
pub mod api;
pub mod load;

use std::collections::HashSet;
use std::sync::Mutex;

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
}
