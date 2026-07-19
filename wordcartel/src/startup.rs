//! Startup seeding — the one place a freshly-built [`Editor`] is filled in from the loaded
//! configuration.
//!
//! Extracted out of `app::run` (C5 re-gate I-R1). `run` is the shell's dispatch hub and is
//! budgeted as one (`tests/module_budgets.rs`); this module carries a single, self-contained
//! axis of change — "what a config layer is allowed to say about a new session" — so a new
//! option seeds itself here rather than growing the hub. Everything in here happens exactly
//! once, before the terminal guard is installed and before the first frame.

use crate::config::{Cli, Config};
use crate::editor::Editor;
use crate::settings::{OverridesFile, SettingsSnapshot};
use crate::theme_resolve::EnvSnapshot;
use std::path::Path;

/// The provenance snapshots the settings-write guard needs for the rest of the session,
/// alongside the environment reading the theme was resolved against.
///
/// `overrides` is returned by value because the save path replaces its copy after a
/// successful write (second-save correctness) — it is state, not a constant.
pub(crate) struct StartupSeed {
    /// The environment as it looked at startup; the runtime theme rederive re-reads config
    /// against this same snapshot rather than re-sampling the environment mid-session.
    pub env: EnvSnapshot,
    /// The settings as the HAND-EDITED layers alone define them (no overrides file) — the
    /// baseline a machine-owned write is diffed against.
    pub baseline: SettingsSnapshot,
    /// The machine-owned overrides file as it currently is on disk (all-absent when absent).
    pub overrides: OverridesFile,
    /// The `--config` layer, parsed through `parse_mask` so theme provenance is collapsed at
    /// load time (file vs name are indistinguishable for the guard).
    pub mask: OverridesFile,
}

/// Apply the resolved configuration to a fresh `editor`: option modes through their shared
/// setters, the active theme, the personal dictionary, and the settings-provenance snapshots.
///
/// `warns` collects the startup warning stream — parse warnings from the chrome/canvas
/// dispositions and from theme resolution join whatever config loading already reported, and
/// are surfaced together once the terminal is up.
///
/// `baseline_cfg` is the same configuration loaded from the hand-edited layers ONLY; `cfg`
/// includes the machine-owned overrides file. Both are needed: the editor runs on `cfg`, the
/// write guard diffs against `baseline_cfg`.
///
/// # Examples
///
/// ```ignore
/// let seed = startup::seed_from_config(
///     &mut editor, &cfg, &baseline_cfg, &cli, overrides_path.as_deref(), &*fs, &mut warns);
/// // `seed.overrides` is replaced wholesale after a successful settings write.
/// ```
pub(crate) fn seed_from_config(editor: &mut Editor, cfg: &Config, baseline_cfg: &Config,
    cli: &Cli, overrides_path: Option<&Path>, fs: &dyn crate::fsx::Fs, warns: &mut Vec<String>)
    -> StartupSeed
{
    // Seed mouse_capture from config (default true; may be overridden by config layers).
    editor.mouse_capture = cfg.mouse.mouse_capture;
    editor.view_opts = cfg.view.clone();
    // Seed the option modes through the shared setters (contract law 6 — no direct field writes;
    // set_status_line_mode also enforces the no-true-Off invariant). Dwell-clears are no-ops at
    // startup (no dwell pending yet).
    editor.set_scrollbar_mode(cfg.view.scrollbar);
    editor.set_status_line_mode(cfg.view.status_line);
    editor.set_caret_shape(cfg.view.caret_shape);
    editor.set_caret_blink(cfg.view.caret_blink);
    editor.set_messages_min_kind(cfg.view.messages_min_kind);
    editor.set_clipboard_provider(cfg.clipboard.provider);
    editor.set_show_clutter(cfg.files.show_clutter);
    editor.set_file_type_filter(cfg.files.type_filter);
    editor.clear_clipboard_provider_dirty(); // worker gets the initial plan below; no redundant rebuild
    editor.resume_enabled = cfg.state.resume; // gates open_into_current's resume restore (Effort 7)
    editor.diag_cfg = cfg.diagnostics.clone();
    editor.export_cfg = cfg.export.clone();
    editor.set_menu_bar_mode(cfg.menu.bar);
    // Startup unpin-target policy: when config itself pins the bar, unpin returns to Auto — override
    // the setter's generic remember-current (the pre-seed mode is not meaningful here).
    if cfg.menu.bar == crate::config::MenuBarMode::Pinned {
        editor.menu_bar_unpinned_mode = crate::config::MenuBarMode::Auto;
    }
    editor.active_keymap_preset = crate::keymap::resolve_preset(&cfg.keymap.preset).to_string();

    // Resolve and seed the active theme + color depth (once, at startup — §3.6).
    let env = EnvSnapshot::from_env();
    // Parse the chrome disposition from config; seed editor field; pass to resolve.
    let (chrome_disp, chrome_warn) = crate::theme_resolve::parse_chrome(&cfg.theme.chrome);
    if let Some(w) = chrome_warn { warns.push(w); }
    editor.chrome_disposition = chrome_disp;
    let (canvas_mode, canvas_warn) = crate::theme_resolve::parse_canvas(&cfg.theme.canvas);
    if let Some(w) = canvas_warn { warns.push(w); }
    editor.canvas = canvas_mode;
    // fs-chokepoint-allow: (w) config-class
    let resolved = crate::theme_resolve::resolve_theme(&cfg.theme, &env, chrome_disp);
    editor.theme = resolved.theme;
    editor.depth = resolved.depth;
    editor.heading_glyph_cfg = cfg.theme.heading_level_glyph; // for runtime picker switches (Task 7)
    warns.extend(resolved.warnings); // join the existing startup warning stream

    // D1+A5 Task 4: baseline resolve (WITHOUT the overrides layer) + three snapshots.
    // baseline_cfg was loaded from hand_paths only; the overrides file is NOT in it.
    // fs-chokepoint-allow: (w) config-class
    let baseline_resolved = crate::theme_resolve::resolve_theme(
        &baseline_cfg.theme, &env, wordcartel_core::theme::ChromeDisposition::Full);
    let baseline = crate::settings::snapshot_of(baseline_cfg, &baseline_resolved.theme.name);
    // Overrides snapshot: the current machine-owned file (all-absent when the file doesn't exist).
    let overrides = overrides_path
        .filter(|p| crate::fsx::is_file_via(fs, p))
        .and_then(|p| fs.read_capped(p, crate::limits::MAX_CONFIG_BYTES).ok().flatten())
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| crate::settings::parse_overrides(&s))
        .unwrap_or_default();
    // Mask snapshot: parse the --config layer via parse_mask so theme provenance is
    // collapsed at load time (file vs name are indistinguishable for the guard).
    let mask = cli.config_path.as_ref()
        .filter(|c| crate::fsx::is_file_via(fs, c))
        .and_then(|c| fs.read_capped(c, crate::limits::MAX_CONFIG_BYTES).ok().flatten())
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| crate::settings::parse_mask(&s))
        .unwrap_or_default();
    // Seed theme_identity from the MERGED config's provenance — an overrides/hand `name`
    // wins over `file` per theme_identity_of's rule; use editor.theme.name since resolved.theme
    // was already moved into the editor above.
    editor.theme_identity = crate::settings::theme_identity_of(&cfg.theme, &editor.theme.name);

    // Load the personal dictionary from disk (missing/unreadable/over-cap/invalid-UTF-8 → empty; no abort).
    if let Some(dict_path) = &cfg.diagnostics.dictionary {
        // fs-chokepoint-allow: (w) config-class
        if let Some(text) = crate::file::bounded_read_opt(dict_path, crate::limits::MAX_OPEN_BYTES)
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }

    StartupSeed { env, baseline, overrides, mask }
}
