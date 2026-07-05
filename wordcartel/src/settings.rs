//! Settings mirror, diff law, atomic overrides writer (D1+A5 Task 3).
//!
//! Produces the `SettingsSnapshot` / `OverridesFile` types and the four-rule
//! contradiction-only-removal diff law consumed by Task 4's `perform_settings_save`.
//! The `ThemeIdentity` provenance tag threads through the Editor so that rule 1
//! can distinguish "user switched from a file-theme to a builtin" from a coincidence.

use serde::{Serialize, Deserialize};

// ---------------------------------------------------------------------------
// ThemeIdentity — provenance tag for the active theme
// ---------------------------------------------------------------------------

/// Provenance of the currently-active theme. `File` = loaded from a path
/// (`[theme] file`); `Builtin(name)` = resolved by name (the registry or
/// `[theme] name`). The diff law's rule-1 comparison uses this: `Builtin(n)`
/// vs `File` is always a divergence regardless of whether the two themes look
/// identical at the colour level (spec N-3, the I-4 bug class).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeIdentity {
    File,
    Builtin(String),
}

// ---------------------------------------------------------------------------
// SettingsSnapshot — one config- or runtime-level view of tracked settings
// ---------------------------------------------------------------------------

/// A frozen view of every setting the diff law tracks. Passed to
/// `compute_overrides` as `runtime` (live editor) or `baseline` (config layer).
#[derive(Debug, Clone)]
pub struct SettingsSnapshot {
    pub keymap_preset: String,
    pub theme_identity: ThemeIdentity,
    pub view_typewriter: bool,
    pub view_focus: bool,
    pub view_measure: bool,
    pub view_wrap_guide: bool,
    pub view_word_count: bool,
    pub menu_bar: crate::config::MenuBarMode,
    pub mouse_capture: bool,
}

// ---------------------------------------------------------------------------
// OverridesFile — serde mirror of settings-overrides.toml
// ---------------------------------------------------------------------------

pub const OVERRIDES_HEADER: &str =
    "# managed by wcartel — edits may be overwritten by Save Settings\n";

/// "hidden"/"auto"/"pinned" — MenuBarMode has no serde derive; this mapping mirrors
/// load()'s string match (config.rs) and MUST stay in sync with it.
pub fn menu_bar_str(mode: crate::config::MenuBarMode) -> &'static str {
    match mode {
        crate::config::MenuBarMode::Hidden => "hidden",
        crate::config::MenuBarMode::Auto   => "auto",
        crate::config::MenuBarMode::Pinned => "pinned",
    }
}

/// The machine-owned overrides file (settings-overrides.toml): every field optional,
/// presence-sensitive — the diff law's rules 2/3 need "the layer HAS key K" exactly,
/// which config::load cannot answer (it folds defaults). Parsed and written ONLY here.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverridesFile {
    #[serde(skip_serializing_if = "Option::is_none")] pub keymap: Option<OKeymap>,
    #[serde(skip_serializing_if = "Option::is_none")] pub theme:  Option<OTheme>,
    #[serde(skip_serializing_if = "Option::is_none")] pub view:   Option<OView>,
    #[serde(skip_serializing_if = "Option::is_none")] pub menu:   Option<OMenu>,
    #[serde(skip_serializing_if = "Option::is_none")] pub mouse:  Option<OMouse>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OKeymap {
    #[serde(skip_serializing_if = "Option::is_none")] pub preset: Option<String>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OTheme {
    #[serde(skip_serializing_if = "Option::is_none")] pub name: Option<String>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OView {
    #[serde(skip_serializing_if = "Option::is_none")] pub typewriter: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub focus:      Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub measure:    Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub wrap_guide: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub word_count: Option<bool>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OMenu {
    #[serde(skip_serializing_if = "Option::is_none")] pub bar: Option<String>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OMouse {
    #[serde(skip_serializing_if = "Option::is_none")] pub capture: Option<bool>,
}

// ---------------------------------------------------------------------------
// Snapshot builders
// ---------------------------------------------------------------------------

/// Build a `ThemeIdentity` from a `ThemeConfig`. If the config loads a theme by
/// file path (and has no name override), mark it `File`; otherwise `Builtin(resolved_name)`.
pub fn theme_identity_of(
    theme_cfg: &crate::config::ThemeConfig,
    resolved_name: &str,
) -> ThemeIdentity {
    if theme_cfg.file.is_some() && theme_cfg.name.is_none() {
        ThemeIdentity::File
    } else {
        ThemeIdentity::Builtin(resolved_name.to_string())
    }
}

/// Build a config-level `SettingsSnapshot`. The caller must supply the already-resolved
/// theme name — `resolve_theme` takes an `EnvSnapshot` so MUST run before this call;
/// do NOT resolve inside. Preset is resolved via `keymap::resolve_preset`.
pub fn snapshot_of(cfg: &crate::config::Config, resolved_theme_name: &str) -> SettingsSnapshot {
    SettingsSnapshot {
        keymap_preset: crate::keymap::resolve_preset(&cfg.keymap.preset).to_string(),
        theme_identity: theme_identity_of(&cfg.theme, resolved_theme_name),
        view_typewriter: cfg.view.typewriter,
        view_focus:      cfg.view.focus,
        view_measure:    cfg.view.measure,
        view_wrap_guide: cfg.view.wrap_guide,
        view_word_count: cfg.view.word_count,
        menu_bar:        cfg.menu.bar,
        mouse_capture:   cfg.mouse.mouse_capture,
    }
}

/// Build a runtime `SettingsSnapshot` from the live Editor.
pub fn runtime_snapshot(editor: &crate::editor::Editor) -> SettingsSnapshot {
    SettingsSnapshot {
        keymap_preset:   editor.active_keymap_preset.clone(),
        theme_identity:  editor.theme_identity.clone(),
        view_typewriter: editor.view_opts.typewriter,
        view_focus:      editor.view_opts.focus,
        view_measure:    editor.view_opts.measure,
        view_wrap_guide: editor.view_opts.wrap_guide,
        view_word_count: editor.view_opts.word_count,
        menu_bar:        editor.menu_bar_mode,
        mouse_capture:   editor.mouse_capture,
    }
}

// ---------------------------------------------------------------------------
// parse_overrides / parse_mask
// ---------------------------------------------------------------------------

/// Parse an overrides file. A corrupt or empty file returns the default (all absent).
/// Matches load()'s corruption-tolerance: a bad machine file must not brick the app.
pub fn parse_overrides(bytes: &str) -> OverridesFile {
    toml::from_str::<OverridesFile>(bytes).unwrap_or_default()
}

/// Parse the `--config` layer as an overrides MASK. Identical to `parse_overrides`
/// for all keys except `[theme]`: if EITHER `name` OR `file` is present in the
/// `[theme]` section, the returned `OverridesFile.theme` is set to
/// `Some(OTheme { name: Some("") })` — presence is all the mask-guard checks, so the
/// actual value does not matter. This collapses file-provenance at parse time so that
/// a `--config [theme] file = …` guard is indistinguishable from a name guard for
/// the purpose of rule-3 protection. Corrupt input → empty layer (same as parse_overrides).
pub fn parse_mask(bytes: &str) -> OverridesFile {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct MaskFile {
        keymap: Option<OKeymap>,
        theme:  Option<MaskTheme>,
        view:   Option<OView>,
        menu:   Option<OMenu>,
        mouse:  Option<OMouse>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct MaskTheme {
        name: Option<String>,
        file: Option<String>,
    }

    let mask: MaskFile = toml::from_str(bytes).unwrap_or_default();
    // Collapse theme provenance: name OR file → presence sentinel (empty name string).
    let theme = mask.theme.and_then(|t| {
        if t.name.is_some() || t.file.is_some() {
            Some(OTheme { name: Some(String::new()) })
        } else {
            None
        }
    });
    OverridesFile { keymap: mask.keymap, theme, view: mask.view, menu: mask.menu, mouse: mask.mouse }
}

// ---------------------------------------------------------------------------
// compute_overrides — the four-rule diff law
// ---------------------------------------------------------------------------

/// The contradiction-only-removal diff law (spec D3, user-ratified; rules 1-4 + the
/// rule-3 mask-guard). Generic per-key helper: write on divergence; keep an existing
/// override that matches runtime; remove a contradicted override only when unmasked.
fn diff_key<T: PartialEq + Clone>(rt: &T, base: &T, existing: Option<&T>, masked: bool) -> Option<T> {
    if rt != base { return Some(rt.clone()); }
    match existing {
        Some(e) if e == rt  => Some(e.clone()),  // rule 2: saved intent survives coincidence
        Some(e) if masked   => Some(e.clone()),  // rule 3 guard: masked key never removed
        Some(_)             => None,             // rule 3: contradicted + unmasked → remove
        None                => None,             // rule 4: absent key stays absent
    }
}

/// Returns `Some(t)` when `any` is true, `None` otherwise. Used to lift a section
/// struct into its `Option` wrapper only when at least one field is non-None.
fn some_if<T>(t: T, any: bool) -> Option<T> {
    if any { Some(t) } else { None }
}

/// Apply the four-rule diff law and return the new `OverridesFile`.
/// `runtime` = live editor state; `baseline` = config layer active at startup;
/// `existing` = the current machine overrides (may be empty/corrupt);
/// `mask` = the `--config` layer (parsed with `parse_mask`).
pub fn compute_overrides(
    runtime:  &SettingsSnapshot,
    baseline: &SettingsSnapshot,
    existing: &OverridesFile,
    mask:     &OverridesFile,
) -> OverridesFile {
    // --- keymap ---
    let preset = diff_key(
        &runtime.keymap_preset, &baseline.keymap_preset,
        existing.keymap.as_ref().and_then(|k| k.preset.as_ref()),
        mask.keymap.is_some(),
    );
    let has_preset = preset.is_some();
    let keymap = some_if(OKeymap { preset }, has_preset);

    // --- theme (bespoke provenance logic — spec N-3/N-4) ---
    //
    // `mask.theme.is_some()` is the provenance-collapsed sentinel: T4 calls
    // `parse_mask` on the --config layer, which sets theme presence when EITHER
    // `name` OR `file` is present (file-mask arm), so the guard fires correctly
    // for both name- and file-configured --config themes.
    let theme_masked = mask.theme.is_some();
    let theme_name: Option<String> = match (&runtime.theme_identity, &baseline.theme_identity) {
        (ThemeIdentity::Builtin(n), b) if *b != ThemeIdentity::Builtin(n.clone()) => Some(n.clone()),
        (rt, _) => match existing.theme.as_ref().and_then(|t| t.name.as_ref()) {
            Some(e) if *rt == ThemeIdentity::Builtin(e.clone()) => Some(e.clone()),
            Some(e) if theme_masked => Some(e.clone()),
            Some(_) | None => None,
        },
    };
    let has_theme = theme_name.is_some();
    let theme = some_if(OTheme { name: theme_name }, has_theme);

    // --- view ---
    let ex_view    = existing.view.as_ref();
    let view_masked = mask.view.is_some();
    let typewriter = diff_key(
        &runtime.view_typewriter, &baseline.view_typewriter,
        ex_view.and_then(|v| v.typewriter.as_ref()),
        view_masked,
    );
    let focus = diff_key(
        &runtime.view_focus, &baseline.view_focus,
        ex_view.and_then(|v| v.focus.as_ref()),
        view_masked,
    );
    let measure = diff_key(
        &runtime.view_measure, &baseline.view_measure,
        ex_view.and_then(|v| v.measure.as_ref()),
        view_masked,
    );
    let wrap_guide = diff_key(
        &runtime.view_wrap_guide, &baseline.view_wrap_guide,
        ex_view.and_then(|v| v.wrap_guide.as_ref()),
        view_masked,
    );
    let word_count = diff_key(
        &runtime.view_word_count, &baseline.view_word_count,
        ex_view.and_then(|v| v.word_count.as_ref()),
        view_masked,
    );
    let any_view = typewriter.is_some() || focus.is_some() || measure.is_some()
        || wrap_guide.is_some() || word_count.is_some();
    let view = some_if(OView { typewriter, focus, measure, wrap_guide, word_count }, any_view);

    // --- menu ---
    let rt_bar   = menu_bar_str(runtime.menu_bar).to_string();
    let base_bar = menu_bar_str(baseline.menu_bar).to_string();
    let bar = diff_key(
        &rt_bar, &base_bar,
        existing.menu.as_ref().and_then(|m| m.bar.as_ref()),
        mask.menu.is_some(),
    );
    let has_bar = bar.is_some();
    let menu = some_if(OMenu { bar }, has_bar);

    // --- mouse ---
    let capture = diff_key(
        &runtime.mouse_capture, &baseline.mouse_capture,
        existing.mouse.as_ref().and_then(|m| m.capture.as_ref()),
        mask.mouse.is_some(),
    );
    // Option<bool>: Copy — no move conflict on the struct literal below.
    let any_mouse = capture.is_some();
    let mouse = some_if(OMouse { capture }, any_mouse);

    OverridesFile { keymap, theme, view, menu, mouse }
}

// ---------------------------------------------------------------------------
// save_overrides / write_overrides — atomic writer over the Fs seam
// ---------------------------------------------------------------------------

/// Serialize `of` and atomically write it to `path`, creating the parent dir when
/// needed. chmod 0700 is applied to the parent ONLY if it did not already exist
/// (the config dir also holds hand-owned files — a save must not tighten the user's
/// own 0755 directory, unlike the swap dir which is machine-only).
pub(crate) fn save_overrides(
    fs:   &dyn crate::fsx::Fs,
    path: &std::path::Path,
    of:   &OverridesFile,
) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let parent = path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let existed = parent.exists();
    std::fs::create_dir_all(&parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed {
            std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    { let _ = existed; }
    let body = toml::to_string(of)
        .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    let bytes = format!("{OVERRIDES_HEADER}{body}");
    write_overrides(fs, path, bytes.as_bytes())
}

/// Write pre-serialized bytes to `path` via the Fs seam (0600, dir-fsynced).
/// Called by `save_overrides`; exported for Task 4's direct-write path.
pub(crate) fn write_overrides(
    fs:   &dyn crate::fsx::Fs,
    path: &std::path::Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    crate::fsx::atomic_replace(fs, path, bytes, crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::Fixed(0o600),
        dir_fsync: true,
    })
}

// ---------------------------------------------------------------------------
// perform_settings_save — the run()-loop's save gate (D1+A5 Task 4)
// ---------------------------------------------------------------------------

/// Perform a requested settings save: refusals first, then diff + atomic write.
/// Returns the new overrides snapshot on success (the caller replaces its copy so
/// rules 2/3 stay correct for a second save in the same session). Sets editor.status
/// in every arm — no silent UI.
pub(crate) fn perform_settings_save(
    editor:        &mut crate::editor::Editor,
    no_config:     bool,
    overrides_path: Option<&std::path::Path>,
    baseline:      &SettingsSnapshot,
    existing:      &OverridesFile,
    mask:          &OverridesFile,
    fs:            &dyn crate::fsx::Fs,
) -> Option<OverridesFile> {
    if no_config {
        editor.status = "settings: disabled by --no-config".into();
        return None;
    }
    let Some(path) = overrides_path else {
        editor.status = "settings: no config directory".into();
        return None;
    };
    let runtime = runtime_snapshot(editor);
    let of = compute_overrides(&runtime, baseline, existing, mask);
    match save_overrides(fs, path, &of) {
        Ok(()) => { editor.status = "settings saved".into(); Some(of) }
        Err(e) => { editor.status = format!("settings: {e}"); None }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "wc-settings-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn snap(preset: &str, theme: ThemeIdentity, tw: bool) -> SettingsSnapshot {
        SettingsSnapshot { keymap_preset: preset.into(), theme_identity: theme,
            view_typewriter: tw, view_focus: false, view_measure: false,
            view_wrap_guide: false, view_word_count: false,
            menu_bar: crate::config::MenuBarMode::Auto, mouse_capture: true }
    }

    #[test]
    fn rule1_divergence_writes_and_rule4_absent_otherwise() {
        let rt = snap("wordstar", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.keymap.as_ref().unwrap().preset.as_deref(), Some("wordstar"));
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(true));
        assert!(of.theme.is_none(), "un-diverged never-saved key stays absent");
        assert!(of.mouse.is_none() && of.menu.is_none());
    }

    #[test]
    fn rule2_keeps_coinciding_saved_key_across_baselines() {
        // The cross-project walkthrough: override typewriter=false; project-B baseline
        // ALSO false; runtime false → KEEP, not remove.
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let base_b = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let existing = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base_b, &existing, &OverridesFile::default());
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(false), "saved intent survives coincidence");
    }

    #[test]
    fn rule3_removes_on_contradiction_unless_masked() {
        // User toggled back to the baseline value → the override contradicts → REMOVE...
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let existing = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base, &existing, &OverridesFile::default());
        assert!(of.view.is_none(), "explicit un-save removes the key");
        // ...UNLESS the --config layer sets the key (mask-guard): KEEP verbatim.
        let mask = parse_overrides("[view]\ntypewriter=true\n");
        let of2 = compute_overrides(&rt, &base, &existing, &mask);
        assert_eq!(of2.view.as_ref().unwrap().typewriter, Some(false), "masked key never removed");
    }

    #[test]
    fn theme_mask_guard_is_provenance_typed() {
        // --config sets [theme] FILE (not name): runtime File == baseline File; the saved
        // name contradicts → rule-3 candidate — the FILE mask must still guard it (N-4).
        let rt = snap("cua", ThemeIdentity::File, false);
        let base = snap("cua", ThemeIdentity::File, false);
        let existing = parse_overrides("[theme]\nname='gruvbox'\n");
        let mask = parse_overrides("[theme]\nname='x'\n"); // name-mask arm
        let of = compute_overrides(&rt, &base, &existing, &mask);
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
        // The file-mask arm needs mask presence for [theme] file — OTheme carries name only,
        // so the mask snapshot for theme is provenance-collapsed at PARSE time: T4 parses the
        // --config layer with parse_mask (below), which sets theme presence when EITHER
        // name OR file is present. Here simulate it directly:
        let mask_file = parse_mask("[theme]\nfile='/tmp/x.yaml'\n");
        let of2 = compute_overrides(&rt, &base, &existing, &mask_file);
        assert_eq!(of2.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"), "file-mask guards the name override");
    }

    #[test]
    fn theme_rules_2_and_3_compare_by_provenance() {
        // rule 2: runtime Builtin(n) matching the saved name → keep.
        let rt = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let base = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let existing = parse_overrides("[theme]\nname='gruvbox'\n");
        let of = compute_overrides(&rt, &base, &existing, &OverridesFile::default());
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
        // rule 3: runtime File, saved name contradicts, no mask → removed.
        let rt2 = snap("cua", ThemeIdentity::File, false);
        let base2 = snap("cua", ThemeIdentity::File, false);
        let of2 = compute_overrides(&rt2, &base2, &existing, &OverridesFile::default());
        assert!(of2.theme.is_none());
    }

    #[test]
    fn theme_collision_pick_over_file_theme_writes_name() {
        // Fable plan I3(a): runtime Builtin(n) vs baseline File → rule 1 WRITES the name
        // even when n collides with the file theme's scheme name (the I-4 bug class).
        let rt = snap("cua", ThemeIdentity::Builtin("gruvbox".into()), false);
        let base = snap("cua", ThemeIdentity::File, false);
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.theme.as_ref().unwrap().name.as_deref(), Some("gruvbox"));
    }

    #[test]
    fn genuine_masked_session_divergence_still_writes() {
        // Fable plan I3(b): the mask-guard protects rule 3 only — rule 1 is untouched.
        let rt = snap("cua", ThemeIdentity::Builtin("default".into()), true);
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let mask = parse_overrides("[view]\ntypewriter=false\n");
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &mask);
        assert_eq!(of.view.as_ref().unwrap().typewriter, Some(true), "explicit change wins through the mask");
    }

    #[test]
    fn corrupt_overrides_parse_to_an_empty_layer() {
        // Fable plan I3(d) / spec Error handling: a corrupt machine file must not brick.
        assert_eq!(parse_overrides("not [valid toml"), OverridesFile::default());
        assert_eq!(parse_mask("also ["), OverridesFile::default());
    }

    #[test]
    fn save_overrides_roundtrips_and_headers() {
        let d = tempdir(); // reuse the config.rs idiom: a small local tempdir helper
        let path = d.join("settings-overrides.toml");
        let of = OverridesFile {
            menu: Some(OMenu { bar: Some("pinned".into()) }),
            mouse: Some(OMouse { capture: Some(false) }),
            ..OverridesFile::default()
        };
        save_overrides(&crate::fsx::RealFs, &path, &of).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(OVERRIDES_HEADER));
        assert!(text.contains("bar = \"pinned\"") || text.contains("bar = 'pinned'"));
        assert_eq!(parse_overrides(&text), of, "round-trip identity");
        // all-empty → header only
        save_overrides(&crate::fsx::RealFs, &path, &OverridesFile::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), OVERRIDES_HEADER);
        // idempotence: same input twice → identical bytes
        save_overrides(&crate::fsx::RealFs, &path, &of).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        save_overrides(&crate::fsx::RealFs, &path, &of).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "byte-identical re-save");
    }

    #[test]
    fn save_overrides_surfaces_io_failure() {
        struct FailFs;
        impl crate::fsx::Fs for FailFs {
            fn create_excl(&self, _: &std::path::Path, _: u32) -> std::io::Result<Box<dyn crate::fsx::WriteSync>> {
                Err(std::io::Error::other("boom")) // io_other_error is deny — the repo idiom (fsx.rs:394+)
            }
            fn existing_mode(&self, _: &std::path::Path) -> Option<u32> { None }
            fn rename(&self, _: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn sync_dir(&self, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> { Ok(()) }
        }
        let d = tempdir();
        let err = save_overrides(&FailFs, &d.join("o.toml"), &OverridesFile::default()).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn save_overrides_creates_the_parent_dir() {
        let d = tempdir();
        let path = d.join("nested").join("settings-overrides.toml");
        save_overrides(&crate::fsx::RealFs, &path, &OverridesFile::default()).unwrap();
        assert!(path.is_file());
    }

    // D1+A5 Task 4 — perform_settings_save behavior pins -----------------------

    // Thin Editor-like helper: only the fields perform_settings_save reads.
    fn test_editor() -> crate::editor::Editor {
        crate::editor::Editor::new_from_text("doc\n", None, (80, 24))
    }
    fn empty_snap() -> SettingsSnapshot {
        snap("cua", ThemeIdentity::Builtin("default".into()), false)
    }

    #[test]
    fn save_refused_under_no_config() {
        // --no-config → status "settings: disabled by --no-config"; no file written.
        let d = tempdir();
        let path = d.join("o.toml");
        let mut e = test_editor();
        let result = perform_settings_save(
            &mut e, true, Some(&path),
            &empty_snap(), &OverridesFile::default(), &OverridesFile::default(),
            &crate::fsx::RealFs,
        );
        assert!(result.is_none(), "must return None on --no-config refusal");
        assert_eq!(e.status, "settings: disabled by --no-config");
        assert!(!path.exists(), "no file must be written on --no-config refusal");
    }

    #[test]
    fn save_refused_without_config_dir() {
        // overrides_path = None (config_dir() returned None) → status "settings: no config directory".
        let mut e = test_editor();
        let result = perform_settings_save(
            &mut e, false, None,
            &empty_snap(), &OverridesFile::default(), &OverridesFile::default(),
            &crate::fsx::RealFs,
        );
        assert!(result.is_none(), "must return None when overrides_path is None");
        assert_eq!(e.status, "settings: no config directory");
    }

    #[test]
    fn save_failure_surfaces_io_error() {
        // A FailFs → status starts "settings: " and includes the IO error string.
        struct FailFs;
        impl crate::fsx::Fs for FailFs {
            fn create_excl(&self, _: &std::path::Path, _: u32) -> std::io::Result<Box<dyn crate::fsx::WriteSync>> {
                Err(std::io::Error::other("boom"))
            }
            fn existing_mode(&self, _: &std::path::Path) -> Option<u32> { None }
            fn rename(&self, _: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn sync_dir(&self, _: &std::path::Path) -> std::io::Result<()> { unreachable!() }
            fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> { Ok(()) }
        }
        let d = tempdir();
        let path = d.join("o.toml");
        let mut e = test_editor();
        let result = perform_settings_save(
            &mut e, false, Some(&path),
            &empty_snap(), &OverridesFile::default(), &OverridesFile::default(),
            &FailFs,
        );
        assert!(result.is_none(), "must return None on IO error");
        assert!(e.status.starts_with("settings: "),
            "status must start with 'settings: ': {:?}", e.status);
        assert!(e.status.contains("boom"),
            "status must include the IO error string: {:?}", e.status);
    }

    #[test]
    fn save_success_sets_status_and_returns_snapshot() {
        // A successful save → status "settings saved"; returned OverridesFile == computed.
        let d = tempdir();
        let path = d.join("o.toml");
        // Runtime diverges from baseline on keymap.preset → the override must appear.
        let runtime_snap = snap("wordstar", ThemeIdentity::Builtin("default".into()), false);
        let baseline_snap = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let mut e = test_editor();
        e.active_keymap_preset = "wordstar".into();
        let result = perform_settings_save(
            &mut e, false, Some(&path),
            &baseline_snap, &OverridesFile::default(), &OverridesFile::default(),
            &crate::fsx::RealFs,
        );
        assert!(result.is_some(), "must return Some on success");
        assert_eq!(e.status, "settings saved");
        let of = result.unwrap();
        let expected = compute_overrides(&runtime_snap, &baseline_snap, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of, expected, "returned OverridesFile must equal compute_overrides result");
        // The file must exist and round-trip.
        assert!(path.is_file(), "overrides file must be written");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_overrides(&text), of, "written file must round-trip");
    }
}
