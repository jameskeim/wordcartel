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
        e.clear_plugin_wake_state(); // P3 §3g: drop the timer schedule + on_change subscription of the dead VM
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
    if !host.has_vm() {                                                    //    exhaustion/disabled reverted
        inventory.clear();                                                //    the registry to builtins-only;
    }                                                                     //    don't over-report a dead VM
    let mut e = editor.borrow_mut();                                       // 8. re-resolve bindings + report
    e.keymap_rebuild = true;
    e.plugin_inventory = inventory;
    if let Some(w) = warns.first() { e.set_status(crate::status::StatusKind::Info, w.clone()); }
    else {
        let ok = e.plugin_inventory.iter().filter(|r| r.error.is_none()).count();
        e.set_status(crate::status::StatusKind::Info, format!("plugins reloaded ({ok} ok)"));
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
        let status = editor.borrow().status_text().to_string();
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
            e.pending_plugin_calls.push_back(crate::plugin::PluginCall { id: CommandId("x"), arg: None });
            e.pending_plugin_events.push_back(crate::plugin::PluginEvent {
                kind: crate::plugin::PluginEventKind::Save, path: None });
            e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
                origin: "o".into(), name: "n".into(), arg: None });
            // P3 §3g: a reload must also clear the timer schedule + on_change subscription of the dead VM.
            e.pending_plugin_timers.push(crate::plugin::PluginTimer {
                handle: 1, origin: "p".into(), key: "wc-timer-1".into(),
                next_due_ms: 500, interval_ms: 1_000, repeat: false, pending: false });
            e.has_on_change_subscriber = true;
            e.on_change_due = Some(500);
            e.plugins_reload_requested = true;
        }
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        let e = editor.borrow();
        assert!(e.pending_plugin_calls.is_empty(), "calls queue cleared");
        assert!(e.pending_plugin_events.is_empty(), "events queue cleared");
        assert!(e.pending_plugin_dispatch.is_empty(), "dispatch queue cleared");
        assert!(e.pending_plugin_timers.is_empty(), "the reload clears the timer schedule (P3 §3g)");
        assert!(!e.has_on_change_subscriber, "the reload clears the on_change subscription");
        assert_eq!(e.on_change_due, None, "the reload clears the on_change due");
        // Task 4's on_change_deadline_none_after_teardown half: the cleared subscriber/due must
        // also drop out of next_wake — a torn-down host leaves no phantom wake armed.
        assert_eq!(crate::timers::next_wake(&e, 10_000), None,
            "a torn-down host must leave next_wake unarmed by the on_change row");
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
            e.pending_plugin_calls.push_back(crate::plugin::PluginCall { id: CommandId("x"), arg: None });
            e.pending_plugin_events.push_back(crate::plugin::PluginEvent {
                kind: crate::plugin::PluginEventKind::Save, path: None });
            e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
                origin: "o".into(), name: "n".into(), arg: None });
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

    // -----------------------------------------------------------------------
    // P2 Task 9 (spec §12, merge GATEs): command-surface-contract LAWs re-run over a
    // POST-RELOAD registry — palette-completeness (LAW 3) and menu ⊆ palette (LAW 4) are
    // structural over ANY registry state, but `perform_reload` rebuilds the registry from
    // scratch (retain_builtins → re-load), so this is the one seam worth proving explicitly.
    // -----------------------------------------------------------------------

    /// LAW 3 + LAW 4 over a post-reload registry: a reloaded plugin's `menu=Some(Edit)`
    /// command appears in BOTH the palette (exhaustively) and the Edit menu group, and the
    /// two plugin-lifecycle builtins (`plugins_reload`/`plugin_list`) appear in the Settings
    /// menu group — the same shape `plugin_menu_tagged_command_appears_in_menu_menu_none_is_
    /// palette_only` (menu.rs) proves for a fresh load, now proven across the reload seam.
    #[test]
    fn post_reload_registry_satisfies_palette_completeness_and_menu_subset() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        std::fs::write(plugdir.path().join("p.lua"),
            "wc.register_command{ name='a', label='Plugin Edit Thing', menu='Edit', fn=function() end }")
            .unwrap();
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();

        // Reach the law through the RELOAD seam (not a fresh load_phase) — perform_reload
        // tears the registry down to builtins-only and rebuilds it; that is the seam these
        // laws must hold across.
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        let a_id = reg.resolve_name("p.a").expect("the menu-tagged command survives reload");

        let (km, warns) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        assert!(warns.is_empty(), "post-reload registry must build a clean keymap: {warns:?}");

        // LAW 3: palette is exhaustive over the POST-RELOAD registry.
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        let ids: std::collections::HashSet<_> = p.rows.iter().map(|r| r.id).collect();
        for (id, _) in reg.commands() {
            assert!(ids.contains(&id), "palette missing registered command {} after reload", id.0);
        }
        assert_eq!(p.rows.len(), reg.commands().count(), "row count == registry command count post-reload");

        // LAW 4: menu ⊆ palette — the reloaded plugin's menu=Some(Edit) command appears in the
        // Edit group, and the Settings-tagged plugin-lifecycle builtins appear in Settings.
        let ed = editor.borrow();
        let view = crate::menu::build(&reg, &km, &ed);
        let edit_items: Vec<_> = view.groups.iter()
            .find(|(cat, _)| *cat == crate::registry::MenuCategory::Edit)
            .map(|(_, items)| items.clone())
            .unwrap_or_default();
        assert!(edit_items.iter().any(|(_, action)| *action == crate::menu::MenuRowAction::Command(a_id)),
            "the reloaded menu=Some(Edit) plugin command must appear in the Edit menu group: {edit_items:?}");

        let settings_items: Vec<_> = view.groups.iter()
            .find(|(cat, _)| *cat == crate::registry::MenuCategory::Settings)
            .map(|(_, items)| items.clone())
            .unwrap_or_default();
        let reload_id = reg.resolve_name("plugins_reload").expect("builtin survives reload");
        let list_id = reg.resolve_name("plugin_list").expect("builtin survives reload");
        assert!(settings_items.iter().any(|(_, action)| *action == crate::menu::MenuRowAction::Command(reload_id)),
            "plugins_reload must appear in the Settings menu post-reload: {settings_items:?}");
        assert!(settings_items.iter().any(|(_, action)| *action == crate::menu::MenuRowAction::Command(list_id)),
            "plugin_list must appear in the Settings menu post-reload: {settings_items:?}");
    }

    /// LAW 7 (command-surface contract) across the reload seam: a `keymap.patches` binding of
    /// a plugin command resolves in `build_keymap` BEFORE a reload, and — for a plugin that
    /// survives the reload unchanged — RE-resolves afterward too. Mirrors menu.rs's
    /// `plugin_command_bound_via_patch_resolves_and_survives_preset_switch`, now proven across
    /// a reload instead of a preset switch.
    #[test]
    fn plugin_command_bound_via_patch_reresolves_after_reload() {
        let plugdir = tempfile::tempdir().unwrap();
        let cfgdir = tempfile::tempdir().unwrap();
        write_plugin(plugdir.path(), "cmd");
        let cfg_path = write_config(cfgdir.path(), plugdir.path());

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let editor = editor_handle();
        let tx = msg_tx();

        let mut warns = Vec::new();
        load_phase(&mut reg, &mut host, &plugins_cfg(plugdir.path()), None, &mut warns);
        let id_before = reg.resolve_name("p.cmd").expect("registered before reload");

        let patch = crate::config::KeymapPatch {
            bind: [("ctrl-alt-p".to_string(), "p.cmd".to_string())].into_iter().collect(),
            unbind: vec![], cua: None, wordstar: None,
        };
        let (km_before, warns_before) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch.clone()] }, &reg);
        assert!(warns_before.is_empty(), "the patch must resolve BEFORE reload: {warns_before:?}");
        assert_eq!(km_before.chord_for(id_before).as_deref(), Some("ctrl-alt-p"));

        // Reload the SAME (unchanged) plugin source.
        editor.borrow_mut().plugins_reload_requested = true;
        perform_reload(&mut host, &mut reg, &editor, &[cfg_path], None, false, &tx);
        let id_after = reg.resolve_name("p.cmd").expect("still registered after reload");

        let (km_after, warns_after) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch] }, &reg);
        assert!(warns_after.is_empty(), "the patch must RE-resolve after reload (LAW 7): {warns_after:?}");
        assert_eq!(km_after.chord_for(id_after).as_deref(), Some("ctrl-alt-p"),
            "the patch-bound plugin command must re-resolve post-reload");
    }
}
