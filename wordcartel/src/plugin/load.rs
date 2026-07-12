//! The filesystem-free load core ([`load_sources`], Task 4) and the filesystem/config
//! discovery layer (`discover`, Task 6) that drives it: exec each plugin source into the host
//! VM, cap + intern its registrations, and commit them into the `Registry` atomically per
//! plugin — a failing plugin (parse error, an over-cap string, a collision) registers ZERO
//! commands, isolated from every other plugin in the batch.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use crate::limits::{PLUGIN_MAX_COMMANDS_PER_PLUGIN, PLUGIN_MAX_STEM_LEN};
use crate::plugin::host::{PendingReg, PluginHost};
use crate::registry::Registry;

/// The outcome of loading ONE plugin source: `Ok(n_commands)` registered, or `Err(reason)` — a
/// parse/exec error, an over-length stem/name/label, a per-plugin count cap, or a collision.
/// A plugin is atomic: any `Err` means ZERO of its commands were committed to the `Registry`.
pub struct LoadReport {
    pub plugin: String,
    pub result: Result<usize, String>,
}

/// Filesystem-free load core: exec each `(stem, source)` into the host VM, collect
/// registrations, commit into `reg` ATOMICALLY per plugin. Per-plugin failure is isolated —
/// one bad plugin does not stop the batch — AND all-or-nothing — a failing plugin leaves ZERO
/// commands registered. The null host (`host.lua()` is `None`) is a no-op: nothing loads, an
/// empty report list.
///
/// Every host→Lua entry point is wrapped in [`crate::panicx::catch`] — the Task 1 spike found
/// `mlua` does NOT convert a Rust panic to an `Err` at the call site (it resumes the raw
/// panic), so `catch_unwind` here is the SOLE backstop, not a redundant one.
pub fn load_sources(
    reg: &mut Registry,
    host: &PluginHost,
    sources: &[(String, String)],
) -> Vec<LoadReport> {
    let Some(lua) = host.lua() else { return Vec::new() };
    let mut reports = Vec::new();
    for (stem_raw, src) in sources {
        let result = crate::panicx::catch(|| load_one(reg, lua, stem_raw, src)).unwrap_or_else(Err);
        reports.push(LoadReport { plugin: stem_raw.clone(), result });
    }
    reports
}

/// Load + register ONE plugin, atomically.
///
/// 1. Cap the stem (a Rust-level `&str` — the caller's, not plugin-Lua-supplied — so it needs
///    no `mlua::String` extraction) against `PLUGIN_MAX_STEM_LEN`, then intern it.
/// 2. Install a FRESH registration sink (`wc.register_command`) scoped to this plugin, then
///    exec its source. A parse/exec error bails with nothing committed — the sink's Lua-side
///    callbacks (if any were stored before the error) are inert dead keys, harmless.
/// 3. Preflight EVERY pending registration — a collision with the live `Registry` OR with
///    another entry in this same batch, and a re-confirmed per-plugin count cap — BEFORE
///    committing ANY of them.
/// 4. Only on a clean preflight: commit all. Every `register_plugin` call is then provably
///    `Ok` (the preflight already ruled out the sole possible failure), so a stray `Err` there
///    would be a logic bug, not a plugin-author mistake — an `.expect` documents that invariant.
fn load_one(reg: &mut Registry, lua: &mlua::Lua, stem_raw: &str, src: &str) -> Result<usize, String> {
    if stem_raw.len() > PLUGIN_MAX_STEM_LEN {
        return Err(format!(
            "plugin stem too long ({} bytes, max {PLUGIN_MAX_STEM_LEN})",
            stem_raw.len()
        ));
    }
    let stem = crate::plugin::intern(stem_raw);

    let sink: Rc<RefCell<Vec<PendingReg>>> = Rc::new(RefCell::new(Vec::new()));
    let count = Rc::new(Cell::new(0usize));
    crate::plugin::api::install_registration(lua, stem, sink.clone(), count.clone())
        .map_err(|e| format!("plugin {stem}: {e}"))?;
    lua.load(src).set_name(stem).exec().map_err(|e| format!("plugin {stem}: {e}"))?;

    let pending: Vec<PendingReg> = sink.borrow_mut().drain(..).collect();
    if pending.len() > PLUGIN_MAX_COMMANDS_PER_PLUGIN {
        return Err(format!(
            "plugin {stem}: too many commands ({}, max {PLUGIN_MAX_COMMANDS_PER_PLUGIN})",
            pending.len()
        ));
    }
    // Preflight: every id must be free of the LIVE registry AND unique within this batch —
    // checked BEFORE committing any of them (the atomic-per-plugin guarantee).
    let mut seen = HashSet::new();
    for p in &pending {
        if reg.resolve_name(p.id.0).is_some() || !seen.insert(p.id) {
            return Err(format!("plugin {stem}: duplicate command id {}", p.id.0));
        }
    }
    let n = pending.len();
    for p in pending {
        reg.register_plugin(p.id, p.label, p.menu)
            .expect("preflight already ruled out every possible Duplicate for this plugin");
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MenuCategory;

    fn sources(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(s, src)| (s.to_string(), src.to_string())).collect()
    }

    #[test]
    fn load_registers_command_into_registry() {
        let mut reg = Registry::builtins();
        let host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='hello', label='Hello', fn=function() end }";
        let reports = load_sources(&mut reg, &host, &sources(&[("greet", src)]));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("greet.hello").expect("registered");
        let meta = reg.meta(id).unwrap();
        assert_eq!(meta.label, "Hello");
        assert_eq!(meta.menu, None);
        // callback stored under the id's registry key and callable.
        let cb: mlua::Function = host.lua().unwrap()
            .named_registry_value(&format!("wc-cmd-{}", id.0)).expect("callback stored");
        cb.call::<()>(()).expect("callback runs");
    }

    #[test]
    fn load_namespaces_id_by_stem() {
        let mut reg = Registry::builtins();
        let host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='insert', label='Insert', fn=function() end }";
        load_sources(&mut reg, &host, &sources(&[("date", src)]));
        assert!(reg.resolve_name("date.insert").is_some());
        assert!(reg.resolve_name("insert").is_none());
    }

    #[test]
    fn load_rejects_over_length_name() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        let long = "a".repeat(crate::limits::PLUGIN_MAX_NAME_LEN + 1);
        let src = format!("wc.register_command{{ name='{long}', label='L', fn=function() end }}");
        let reports = load_sources(&mut reg, &host, &sources(&[("p", &src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "nothing interned into the registry");
    }

    #[test]
    fn load_rejects_over_length_label() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        let long = "a".repeat(crate::limits::PLUGIN_MAX_LABEL_LEN + 1);
        let src = format!("wc.register_command{{ name='x', label='{long}', fn=function() end }}");
        let reports = load_sources(&mut reg, &host, &sources(&[("p", &src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_rejects_over_length_stem() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        let long_stem = "s".repeat(crate::limits::PLUGIN_MAX_STEM_LEN + 1);
        let src = "wc.register_command{ name='x', label='X', fn=function() end }";
        let reports = load_sources(&mut reg, &host, &sources(&[(&long_stem, src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_rejects_257th_command() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        let src = "for i=1,257 do \
                       wc.register_command{ name='cmd'..i, label='L'..i, fn=function() end } \
                   end";
        let reports = load_sources(&mut reg, &host, &sources(&[("many", src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "nothing interned into the registry");
    }

    #[test]
    fn load_rejects_bad_menu() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', menu='Nonsense', fn=function() end }";
        let reports = load_sources(&mut reg, &host, &sources(&[("p", src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_accepts_known_menu() {
        let mut reg = Registry::builtins();
        let host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', menu='Edit', fn=function() end }";
        let reports = load_sources(&mut reg, &host, &sources(&[("p", src)]));
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("p.x").unwrap();
        assert_eq!(reg.meta(id).unwrap().menu, Some(MenuCategory::Edit));
    }

    #[test]
    fn load_reports_collision() {
        let mut reg = Registry::builtins();
        let host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', fn=function() end }";
        let reports = load_sources(
            &mut reg, &host,
            &sources(&[("p", src), ("p", src)]),
        );
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].result, Ok(1));
        assert!(reports[1].result.is_err(), "second plugin collides on the same id");
        assert_eq!(reg.commands().filter(|(id, _)| id.0 == "p.x").count(), 1);
    }

    #[test]
    fn load_is_atomic_per_plugin() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let host = PluginHost::new().unwrap();
        // Same plugin, same name registered twice — a self-collision within one exec pass.
        let src = "wc.register_command{ name='x', label='X1', fn=function() end }\n\
                   wc.register_command{ name='x', label='X2', fn=function() end }";
        let reports = load_sources(&mut reg, &host, &sources(&[("atomic", src)]));
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "neither command committed");
        assert!(reg.resolve_name("atomic.x").is_none());
    }

    #[test]
    fn load_parse_error_is_reported_not_fatal() {
        let mut reg = Registry::builtins();
        let host = PluginHost::new().unwrap();
        let good = "wc.register_command{ name='hello', label='Hello', fn=function() end }";
        let bad = "end"; // syntax error — unexpected 'end'
        let reports = load_sources(
            &mut reg, &host,
            &sources(&[("bad", bad), ("good", good)]),
        );
        assert_eq!(reports.len(), 2);
        assert!(reports[0].result.is_err());
        assert_eq!(reports[1].result, Ok(1));
        assert!(reg.resolve_name("good.hello").is_some());
    }
}
