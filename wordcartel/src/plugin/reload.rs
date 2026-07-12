//! Shared plugin load orchestration: `load_phase` (used by startup AND reload) and `perform_reload`
//! (whole-VM teardown + registry revert + queue clear + keymap re-resolve, P2 §6). Keeps app.rs a
//! thin caller (anti-regrowth): the reload BODY lives here, not in run().

/// Resolve the plugins dir, discover, load, and populate host hooks + the inventory + warnings.
/// Shared by startup and reload so both paths are byte-identical. On a commit-time VM-exhaustion
/// `load_sources` already nulled the host + reverted the registry (§7b registry half); this fn
/// reflects that in the inventory/warnings.
///
/// The `[plugins].dir` field wins over the default `<xdg>/wordcartel/plugins`; when neither
/// resolves the phase warns loudly (never silences — no-silent-UI) and loads nothing.
///
/// # Parameters
/// - `reg` — the registry plugin commands commit into.
/// - `host` — the live VM (or the null host, in which case nothing loads).
/// - `plugins` — the `[plugins]` config section (dir/disable/per-plugin config).
/// - `xdg` — the resolved XDG config dir, if any (the default-dir base).
/// - `warns` — the caller-owned warning channel; skipped/failed plugins and a missing dir push here.
///
/// # Returns
/// One [`crate::plugin::PluginRecord`] per discovered plugin (skipped, failed, or loaded), for
/// `plugin_list` + reload reporting.
pub(crate) fn load_phase(
    reg: &mut crate::registry::Registry,
    host: &mut crate::plugin::host::PluginHost,
    plugins: &crate::config::PluginsConfig,
    xdg: Option<&std::path::Path>,
    warns: &mut Vec<String>,
) -> Vec<crate::plugin::PluginRecord> {
    // [plugins].dir wins; else <xdg>/wordcartel/plugins; else warn (not silence — gap 6).
    let dir = plugins.dir.clone()
        .or_else(|| xdg.map(|x| x.join("wordcartel").join("plugins")));
    let Some(dir) = dir else {
        warns.push("plugins: no config directory found (set [plugins].dir)".into());
        return Vec::new();
    };
    let disc = crate::plugin::load::discover(&dir, &plugins.disable);
    let mut inventory = Vec::new();
    for r in &disc.skipped {
        warns.push(format!("plugin {} skipped: {}",
            r.plugin, r.result.as_ref().err().cloned().unwrap_or_default()));
        inventory.push(crate::plugin::PluginRecord {
            name: r.plugin.clone(), commands: 0, hooks: 0,
            error: r.result.as_ref().err().cloned() });
    }
    for r in crate::plugin::load::load_sources(reg, host, &disc.sources, &plugins.config, warns) {
        // LoadReport = { plugin, result: Result<usize/*commands*/, String>, hooks: usize } (Task 6):
        // commands from `result`, hooks from the real `hooks` field — no side-channel placeholder.
        match &r.result {
            Ok(n) => inventory.push(crate::plugin::PluginRecord {
                name: r.plugin.clone(), commands: *n, hooks: r.hooks, error: None }),
            Err(e) => {
                warns.push(format!("plugin {}: {e}", r.plugin));
                inventory.push(crate::plugin::PluginRecord {
                    name: r.plugin.clone(), commands: 0, hooks: r.hooks, // hooks == 0 on failure (atomic)
                    error: Some(e.clone()) });
            }
        }
    }
    inventory
}

/// The between-reduces reload seam (§6d). Reverts the plugin subsystem and rebuilds it from a fresh
/// re-read of the `[plugins]` config section (⚠OPEN 2 = A). Never runs under a Lua frame — the run
/// loop calls it AFTER `pump()` (which quiesced) and BEFORE `rebuild_keymap_if_requested` (so a
/// `keymap_rebuild` set here is honored the same iteration).
///
/// The sequence is a whole-subsystem revert: null the VM → registry to builtins-only → clear all
/// three plugin queues → rebuild from the re-read config → re-attach the bridge. It is idempotent —
/// a reload that itself fatals (commit-time VM exhaustion) leaves a clean builtins-only state, and a
/// double `retain_builtins` is safe.
///
/// # Parameters
/// - `host` — the live VM to tear down and rebuild.
/// - `reg` — the registry to revert to builtins-only and reload into.
/// - `editor` — the shared editor handle (flag/queues/inventory/status live here).
/// - `all_paths` — the config layer paths to re-read (only `[plugins]` is taken).
/// - `xdg` — the resolved XDG config dir (the default-dir base for `load_phase`).
/// - `no_plugins` — the session `--no-plugins` flag; forces the host to stay null.
/// - `msg_tx` — the loop message channel the rebuilt bridge clones.
pub(crate) fn perform_reload(
    host: &mut crate::plugin::host::PluginHost,
    reg: &mut crate::registry::Registry,
    editor: &std::rc::Rc<std::cell::RefCell<crate::editor::Editor>>,
    all_paths: &[std::path::PathBuf],
    xdg: Option<&std::path::Path>,
    no_plugins: bool,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    { editor.borrow_mut().plugins_reload_requested = false; }              // 1. clear the flag
    let (cfg, _warns) = crate::config::load(all_paths);                    // 2. re-read config…
    let plugins = cfg.plugins;                                             //    …take ONLY [plugins]
    *host = crate::plugin::host::PluginHost::null();                       // 3. tear down the VM
    reg.retain_builtins();                                                 // 4. registry → builtins-only
    {                                                                      // 5. drain stale queues
        let mut e = editor.borrow_mut();
        e.pending_plugin_calls.clear();
        e.pending_plugin_events.clear();
        e.pending_plugin_dispatch.clear();
    }
    let mut warns = Vec::new();
    let mut inventory = Vec::new();
    if no_plugins || !plugins.enabled {                                    // 6. rebuild (or stay null)
        warns.push(if no_plugins { "plugins disabled (--no-plugins)".into() }
                   else { "plugins disabled by config".into() });
    } else {
        match crate::plugin::host::PluginHost::new() {                     //    retry a failed VM
            Ok(mut h) => {
                inventory = load_phase(reg, &mut h, &plugins, xdg, &mut warns);
                if h.has_vm() {                                            //    load_sources may have nulled it (exhaustion)
                    if let Err(e) = h.attach_bridge(editor.clone(), msg_tx.clone(),
                        std::rc::Rc::new(crate::app::SystemClock)
                            as std::rc::Rc<dyn wordcartel_core::history::Clock>) {
                        warns.push(format!("plugin bridge failed to attach: {e}"));
                    }
                    *host = h;                                             // 7. re-attach → NEW VM
                } // else: exhaustion already reverted host(null)+reg(builtins); the editor-side
                  //       clear (step 5) + keymap_rebuild (step 8) below complete the subsystem revert.
            }
            Err(e) => warns.push(format!("plugins disabled: {e}")),
        }
    }
    let mut e = editor.borrow_mut();                                       // 8. re-resolve bindings + report
    e.keymap_rebuild = true;
    e.plugin_inventory = inventory;
    if let Some(w) = warns.first() { e.status = w.clone(); }
    else {
        e.status = format!("plugins reloaded ({} ok)",
            e.plugin_inventory.iter().filter(|r| r.error.is_none()).count());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::config::PluginsConfig;
    use crate::editor::Editor;
    use crate::plugin::host::PluginHost;
    use crate::registry::{CommandId, Registry};

    fn editor_handle() -> Rc<RefCell<Editor>> {
        Rc::new(RefCell::new(Editor::new_from_text("x\n", None, (40, 10))))
    }

    fn msg_tx() -> std::sync::mpsc::Sender<crate::app::Msg> {
        // Leak the receiver so the sender stays live for the whole test (the reload's cloned
        // sender must not observe a disconnected channel).
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(rx);
        tx
    }

    fn test_clock() -> Rc<dyn wordcartel_core::history::Clock> {
        Rc::new(crate::test_support::TestClock::new(0))
    }

    fn plugins_cfg(dir: &std::path::Path) -> PluginsConfig {
        PluginsConfig { dir: Some(dir.to_path_buf()), ..Default::default() }
    }

    /// Write a config file pointing `[plugins].dir` at `plugin_dir` and return its path — the
    /// `all_paths` `perform_reload` re-reads. Kept alive by the caller's `TempDir`.
    fn write_config(cfg_dir: &std::path::Path, plugin_dir: &std::path::Path) -> std::path::PathBuf {
        let p = cfg_dir.join("config.toml");
        std::fs::write(&p, format!("[plugins]\ndir = {:?}\n", plugin_dir.to_str().unwrap())).unwrap();
        p
    }

    fn write_plugin(dir: &std::path::Path, cmd: &str) {
        std::fs::write(dir.join("p.lua"),
            format!("wc.register_command{{ name='{cmd}', label='{cmd}', fn=function() end }}")).unwrap();
    }

    #[test]
    fn reload_replaces_changed_plugin() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();

        // Initial load via the SHARED load_phase (same code path startup uses).
        let mut warns = Vec::new();
        let inv = load_phase(&mut reg, &mut host, &plugins_cfg(plugdir.path()), None, &mut warns);
        host.attach_bridge(editor.clone(), tx.clone(), test_clock()).unwrap();
        editor.borrow_mut().plugin_inventory = inv;
        assert!(reg.resolve_name("p.a").is_some(), "p.a registered after the initial load");

        // Rewrite the plugin to register a DIFFERENT command, then reload.
        write_plugin(plugdir.path(), "b");
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);

        assert!(reg.resolve_name("p.a").is_none(), "the old command is gone after reload");
        assert!(reg.resolve_name("p.b").is_some(), "the new command resolves after reload");
        assert!(host.has_vm(), "reload rebuilt a live VM");
        assert!(!editor.borrow().plugins_reload_requested, "the request flag is cleared");
    }

    #[test]
    fn reload_drops_removed_plugin_binding() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();

        let mut warns = Vec::new();
        load_phase(&mut reg, &mut host, &plugins_cfg(plugdir.path()), None, &mut warns);
        assert!(reg.resolve_name("p.a").is_some());
        let patches = vec![crate::config::KeymapPatch {
            bind: [("ctrl-g".to_string(), "p.a".to_string())].into(),
            ..Default::default() }];

        // Reload removes p.a (the plugin now registers a different command).
        write_plugin(plugdir.path(), "b");
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);

        assert!(editor.borrow().keymap_rebuild, "reload requested a keymap rebuild");
        assert!(reg.resolve_name("p.a").is_none(), "the removed command is gone");

        // The rebuild arm (run-loop) drops the now-stale binding with a warning.
        let dropped = crate::theme_cmds::rebuild_keymap_if_requested(
            &mut editor.borrow_mut(), &patches, &reg);
        assert!(dropped.is_some(), "the rebuild produced a new trie");
        let status = editor.borrow().status.clone();
        assert!(status.contains("unknown command 'p.a'"),
            "the vanished binding surfaces a warning: {status}");
    }

    #[test]
    fn reload_clears_stale_queues() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();
        {
            let mut e = editor.borrow_mut();
            e.pending_plugin_calls.push_back(crate::plugin::PluginCall { id: CommandId("x") });
            e.pending_plugin_events.push_back(crate::plugin::PluginEvent {
                kind: crate::plugin::PluginEventKind::Save, path: None });
            e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
                origin: "o".into(), name: "n".into() });
            e.plugins_reload_requested = true;
        }
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        let e = editor.borrow();
        assert!(e.pending_plugin_calls.is_empty(), "calls queue cleared");
        assert!(e.pending_plugin_events.is_empty(), "events queue cleared");
        assert!(e.pending_plugin_dispatch.is_empty(), "dispatch queue cleared");
    }

    #[test]
    fn reload_with_no_plugins_flag_is_builtins_only() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let builtins = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();
        editor.borrow_mut().plugins_reload_requested = true;
        // no_plugins = true → the host stays null, the registry builtins-only.
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, true, &tx);

        assert!(!host.has_vm(), "a --no-plugins reload leaves a null host");
        assert_eq!(reg.commands().count(), builtins, "the registry is builtins-only");
        assert!(reg.resolve_name("p.a").is_none(), "no plugin command loaded");
        assert!(editor.borrow().keymap_rebuild, "a keymap rebuild is still requested");
    }

    #[test]
    fn reload_recovers_from_failed_vm() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::null(); // simulate a startup where PluginHost::new() had failed
        let editor = editor_handle();
        let tx = msg_tx();
        assert!(!host.has_vm());
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);

        assert!(host.has_vm(), "reload doubles as recovery — a live VM now exists");
        assert!(reg.resolve_name("p.a").is_some(), "the plugin loaded on the recovery reload");
    }

    #[test]
    fn reload_rereads_plugins_config_section() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        // The plugin captures wc.config.min_words at LOAD into a global we can read from the VM.
        std::fs::write(plugdir.path().join("p.lua"),
            "CFG_MIN = (wc.config and wc.config.min_words) or -1\n\
             wc.register_command{ name='a', label='A', fn=function() end }").unwrap();
        let cfg_path = cfgdir.path().join("config.toml");
        let write_cfg = |min: i64| std::fs::write(&cfg_path, format!(
            "[plugins]\ndir = {:?}\n\n[plugins.config.p]\nmin_words = {min}\n",
            plugdir.path().to_str().unwrap())).unwrap();

        let mut reg = Registry::builtins();
        let mut host = PluginHost::null();
        let editor = editor_handle();
        let tx = msg_tx();

        write_cfg(100);
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, std::slice::from_ref(&cfg_path), None, false, &tx);
        let v1: i64 = host.lua().unwrap().globals().get("CFG_MIN").unwrap();
        assert_eq!(v1, 100, "the first reload sees min_words = 100");

        write_cfg(200);
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        let v2: i64 = host.lua().unwrap().globals().get("CFG_MIN").unwrap();
        assert_eq!(v2, 200, "the reload re-read [plugins.config.p] — the new value reaches wc.config");
    }

    #[test]
    fn commit_exhaustion_during_reload_reverts_whole_subsystem() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "a");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let builtins = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();
        // Pre-seed the queues so the editor-side revert is observable.
        {
            let mut e = editor.borrow_mut();
            e.pending_plugin_calls.push_back(crate::plugin::PluginCall { id: CommandId("x") });
            e.pending_plugin_events.push_back(crate::plugin::PluginEvent {
                kind: crate::plugin::PluginEventKind::Save, path: None });
            e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
                origin: "o".into(), name: "n".into() });
            e.plugins_reload_requested = true;
        }
        // Arm the fault seam: the reload's load_phase commit write for p.a is forced to fail →
        // VM exhaustion → whole-subsystem revert.
        crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.set(true));
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.set(false)); // disarm defensively

        assert!(!host.has_vm(), "commit-time exhaustion nulls the host");
        assert_eq!(reg.commands().count(), builtins, "the registry reverts to builtins-only");
        assert!(reg.resolve_name("p.a").is_none(), "the exhausting plugin's command is gone");
        let e = editor.borrow();
        assert!(e.pending_plugin_calls.is_empty(), "calls queue cleared");
        assert!(e.pending_plugin_events.is_empty(), "events queue cleared");
        assert!(e.pending_plugin_dispatch.is_empty(), "dispatch queue cleared");
        assert!(e.keymap_rebuild, "keymap rebuild requested — plugin bindings must re-resolve");
    }
}
