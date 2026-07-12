//! The plugin VM host: owns the one `mlua` VM + bridge, and the pump (Task 5). `null()` is the
//! no-VM host used for `--no-plugins`, load failure, and tests that don't exercise plugins
//! (mirrors `NullProvider`). Task 4 adds the real VM ([`PluginHost::new`]) and the registration
//! sink ([`PendingReg`]) the load layer (`plugin::load`) drains into the `Registry` after each
//! plugin's script executes, atomically per plugin. The bridge (`Rc<RefCell<Editor>>` +
//! `Sender<Msg>` + clock) and `pump` (callback invocation) land in Task 5.

use crate::registry::{CommandId, MenuCategory};

/// One command a plugin's `wc.register_command` call queued during its exec pass. Collected
/// into a shared sink — the registration closure can't hold `&mut Registry` across the Lua
/// boundary, so it appends here instead — and drained into the `Registry` by the loader
/// (`plugin::load::load_one`) only after the WHOLE plugin's preflight passes (atomic
/// per-plugin commit: a failing plugin registers zero commands).
pub struct PendingReg {
    pub id: CommandId,
    pub label: &'static str,
    pub menu: Option<MenuCategory>,
}

/// The plugin VM host. `lua: None` is the null host (no VM, no plugins); `lua: Some(_)` owns
/// the one real `mlua::Lua` for this app instance.
pub struct PluginHost {
    lua: Option<mlua::Lua>, // None in the null host
                             // bridge + pending drain added in Task 5
}

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
        PluginHost { lua: None }
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
        Ok(PluginHost { lua: Some(lua) })
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

    #[test]
    fn null_host_constructs_and_pumps_noop() {
        // The null host holds no VM. Once `pump` exists (Task 5) it is a no-op on an empty
        // queue — here we only assert the null host exists and is inert.
        let host = PluginHost::null();
        assert!(!host.has_vm());
        assert!(host.lua().is_none());
    }

    #[test]
    fn new_host_has_a_live_vm() {
        let host = PluginHost::new().expect("VM construction must succeed");
        assert!(host.has_vm());
        assert!(host.lua().is_some());
    }
}
