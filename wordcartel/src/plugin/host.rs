//! The plugin VM host: owns the one `mlua` VM + bridge, and the pump (Task 5). `null()` is the
//! no-VM host used for `--no-plugins`, load failure, and tests that don't exercise plugins
//! (mirrors `NullProvider`). Task 2 is inert: no VM is ever constructed, nothing is pumped.
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

    /// Whether this host owns a live VM. `false` for [`PluginHost::null`].
    pub fn has_vm(&self) -> bool {
        self.lua.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_host_constructs_and_pumps_noop() {
        // Task 2: the null host holds no VM. Once `pump` exists (Task 5) it is a no-op on an
        // empty queue — here we only assert the null host exists and is inert.
        let host = PluginHost::null();
        assert!(!host.has_vm());
    }
}
