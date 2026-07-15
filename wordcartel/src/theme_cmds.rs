//! Extracted verbatim from app.rs (Effort H1 round 2).

/// Honor a requested keymap rebuild (spec D2). Returns the new trie for the caller to
/// swap into its loop-local. A half-typed prefix must not complete against the new base
/// (spec I-3): the buffer drops, and the status clears ONLY when it is the pending "…"
/// prompt — a switch status set in the same reduce must survive to the draw.
pub(crate) fn rebuild_keymap_if_requested(
    editor: &mut crate::editor::Editor,
    patches: &[crate::config::KeymapPatch],
    reg: &crate::registry::Registry,
) -> Option<crate::keymap::KeyTrie> {
    if !editor.keymap_rebuild { return None; }
    editor.keymap_rebuild = false;
    let (trie, kw) = crate::keymap::build_keymap(&crate::config::KeymapConfig {
        preset: editor.active_keymap_preset.clone(),
        patches: patches.to_vec(),
    }, reg);
    if !editor.pending_keys.is_empty() {
        editor.pending_keys.clear();
        // clear_transient_status() subsumes the former `ends_with('…')` guard: it clears only a
        // Transient occupant (the pending-chord "…" preview is Transient), never a held message.
        editor.clear_transient_status();
    }
    if let Some(w) = kw.first() { editor.set_status(crate::status::StatusKind::Info, w.clone()); }
    Some(trie)
}

/// Re-derive the active theme when `toggle_chrome` sets the request flag. Called from the
/// run-loop between-reduces region, BEFORE the settings-save arm (a same-cycle toggle+save
/// must persist the post-rederive state — plan-mandated order, grounding A.9). Runs the
/// COMPLETE resolve pipeline (base → derive_chrome → Ansi16 policy → user styles → cue
/// glyph) so user overrides are not smeared (Codex r1 Critical). Returns `true` when a
/// rederive occurred; `false` when the flag was not set.
///
/// A picker COMMIT (Enter in the theme picker) sets `editor.theme_identity` to
/// `Builtin(n)` without touching `cfg.theme` — the live pick governs. To ensure the
/// rederive uses the picker-committed name rather than reverting to the startup config,
/// we build an EFFECTIVE ThemeConfig: when the identity is `Builtin(n)`, we override
/// `name = Some(n)` and clear `file`; when `File`, the config path governs.
/// User overrides (styles/depth/chrome/heading_level_glyph) ride along in both cases.
pub(crate) fn rederive_theme_if_requested(
    editor: &mut crate::editor::Editor,
    theme_cfg: &crate::config::ThemeConfig,
    env: &crate::theme_resolve::EnvSnapshot,
) -> bool {
    if !editor.theme_rederive { return false; }
    editor.theme_rederive = false;
    let effective = match &editor.theme_identity {
        crate::settings::ThemeIdentity::Builtin(n) => {
            let mut tc = theme_cfg.clone();
            tc.name = Some(n.clone());
            tc.file = None;
            tc
        }
        crate::settings::ThemeIdentity::File => theme_cfg.clone(),
    };
    let resolved = crate::theme_resolve::resolve_theme(&effective, env, editor.chrome_disposition);
    editor.depth = resolved.depth; // re-seed depth (cheap; cold path)
    editor.apply_theme(resolved.theme);
    true
}

/// Apply the theme-picker's currently-selected built-in as a live preview and
/// record the name in `tp.previewed` — the single funnel for identity threading.
/// `pub(crate)` so mouse.rs can call it after a wheel-scroll selection change.
/// Calls `derive_chrome` before `apply_theme` so the preview respects the active
/// chrome disposition (grounding A.9 D3) — except at `Depth::Ansi16` on Rgb themes,
/// where `apply_ansi16_chrome_policy` runs INSTEAD of derivation (the policy checks
/// sentinels; deriving first would fill them) so previews apply the sentinel-fill table
/// rather than quantized derived values (Finding 2, pre-merge gate).
pub(crate) fn preview_selected_theme(editor: &mut crate::editor::Editor) {
    // Read the name first (drops the borrow), then apply, then set the field.
    let name = editor.theme_picker.as_ref().and_then(|tp| tp.rows.get(tp.selected).cloned());
    if let Some(name) = name {
        if let Some(mut theme) = wordcartel_core::theme::Theme::builtin(&name) {
            // Mirror resolve_theme's depth policy (D3): Ansi16 + Rgb-based theme → sentinel-fill
            // table only (skip derive_chrome to preserve the sentinel state the policy checks).
            // All other paths → derive full Rgb chrome ladder; non-Rgb themes: derive is a no-op.
            use wordcartel_core::theme::{Color, Depth};
            if editor.depth == Depth::Ansi16 && matches!(theme.base_bg, Color::Rgb { .. }) {
                crate::theme_resolve::apply_ansi16_chrome_policy(&mut theme, editor.depth);
            } else {
                theme.derive_chrome(editor.chrome_disposition); // derive before apply (D3)
            }
            editor.apply_theme(theme);
            // name still owned — `theme` did not borrow it; safe to re-borrow tp.
            if let Some(tp) = editor.theme_picker.as_mut() { tp.previewed = Some(name); }
        }
    }
}

/// Commit the theme picker — the shared commit path for the keyboard Enter arm
/// and the mouse click-to-commit arm. Closes the picker and, when a theme was
/// previewed, records its name in `theme_identity`.
pub(crate) fn commit_theme_picker(editor: &mut crate::editor::Editor) {
    if let Some(tp) = editor.theme_picker.take() {
        if let Some(n) = tp.previewed {
            editor.theme_identity = crate::settings::ThemeIdentity::Builtin(n);
        } // untouched open→commit: no preview applied, identity unchanged (spec I-1)
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::Editor;

    fn cua_keymap() -> crate::keymap::KeyTrie {
        let (t, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        t
    }

    #[test]
    fn switch_status_survives_the_rebuild() {
        // Fable plan C1: the rebuild must NOT wipe the switch status set in the same
        // reduce (no pending prefix in play → status untouched by the helper).
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        e.set_status(crate::status::StatusKind::Info, "keymap: wordstar");
        let t = crate::theme_cmds::rebuild_keymap_if_requested(&mut e, &[], &reg);
        assert!(t.is_some());
        assert_eq!(e.status_text(), "keymap: wordstar", "the pinned switch copy reaches the draw");
    }

    #[test]
    fn patches_survive_the_switch() {
        // Fable plan I3(c): a GLOBAL patch bind holds under both bases through the
        // real helper (the same patch slice run() passes from cfg.keymap.patches).
        use crate::keymap::{parse_seq, Resolution};
        let patches = vec![crate::config::KeymapPatch {
            bind: [("ctrl-g".to_string(), "copy".to_string())].into(),
            ..Default::default() }];
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        let t = crate::theme_cmds::rebuild_keymap_if_requested(&mut e, &patches, &reg).unwrap();
        let g = parse_seq("ctrl-g").unwrap();
        assert!(matches!(t.resolve(&g), Resolution::Command(crate::registry::CommandId("copy"))),
            "the global patch rides onto the new base");
    }

    #[test]
    fn rebuild_seam_swaps_the_trie_and_clears_pending() {
        // Manual seam: seed pending_keys with ctrl-k (Pending under BOTH presets), set the
        // flag via dispatch, then run the same rebuild the loop runs; assert ctrl-w resolves
        // to scroll_line_up afterward and pending_keys is EMPTY (spec I-3).
        use crate::keymap::{parse_seq, Resolution};
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        e.pending_keys = parse_seq("ctrl-k").unwrap();
        e.set_status(crate::status::StatusKind::Info, "ctrl-k \u{2026}");
        e.active_keymap_preset = "wordstar".into();
        e.keymap_rebuild = true;
        // The REAL production helper — the test proves run()'s code, not a copy (Fable I4).
        let mut keymap = cua_keymap();
        if let Some(t) = crate::theme_cmds::rebuild_keymap_if_requested(&mut e, &[], &reg) {
            keymap = t;
        }
        assert!(e.pending_keys.is_empty(), "pending prefix must not survive the rebuild");
        assert!(e.status_text().is_empty(), "the pending '…' prompt is cleared");
        let cw = parse_seq("ctrl-w").unwrap();
        assert!(matches!(keymap.resolve(&cw), Resolution::Command(crate::registry::CommandId("scroll_line_up"))));
    }

    // -----------------------------------------------------------------------
    // Task 6 (E3+E4): rederive_theme_if_requested seam test
    // -----------------------------------------------------------------------

    #[test]
    fn rederive_arm_reresolves() {
        // Seam test calling the REAL helper: toggling chrome_disposition + setting the flag,
        // then calling rederive_theme_if_requested, flips the bar face between the §B.3
        // full and zen Chrome bg hexes for flexoki-dark.
        use wordcartel_core::theme::{ChromeDisposition, Color, SemanticElement};
        use crate::theme_resolve::EnvSnapshot;
        use crate::theme_cmds::rederive_theme_if_requested;
        use crate::settings::ThemeIdentity;

        let tc = crate::config::ThemeConfig {
            name: Some("flexoki-dark".into()),
            ..Default::default()
        };
        // Simulate a truecolor terminal so derive_chrome produces Rgb values.
        let env = EnvSnapshot {
            no_color: false,
            colorterm: Some("truecolor".into()),
            term: Some("xterm-256color".into()),
        };

        let mut editor = Editor::new_from_text("x", None, (80, 24));
        // Set identity to match tc so rederive uses flexoki-dark.
        editor.theme_identity = ThemeIdentity::Builtin("flexoki-dark".into());

        // Install flexoki-dark at Full via the real rederive path.
        editor.chrome_disposition = ChromeDisposition::Full;
        editor.theme_rederive = true;
        let did = rederive_theme_if_requested(&mut editor, &tc, &env);
        assert!(did, "flag was set — must return true");
        assert!(!editor.theme_rederive, "flag must be cleared after rederive");
        let full_bg = editor.theme.face(SemanticElement::Chrome).bg;
        // §II.5 flexoki-dark FULL Chrome bg = #2a2828 (unified elevation ladder; flexoki is stable)
        assert_eq!(full_bg, Some(Color::Rgb { r: 0x2a, g: 0x28, b: 0x28 }),
            "flexoki-dark Full Chrome bg must match §II.5: got {full_bg:?}");

        // Now switch to Zen and rederive.
        editor.chrome_disposition = ChromeDisposition::Zen;
        editor.theme_rederive = true;
        let did2 = rederive_theme_if_requested(&mut editor, &tc, &env);
        assert!(did2, "Zen rederive must return true");
        assert!(!editor.theme_rederive, "flag must be cleared");
        let zen_bg = editor.theme.face(SemanticElement::Chrome).bg;
        // §II.5 flexoki-dark ZEN Chrome bg = #1e1c1c (unified elevation ladder; flexoki is stable)
        assert_eq!(zen_bg, Some(Color::Rgb { r: 0x1e, g: 0x1c, b: 0x1c }),
            "flexoki-dark Zen Chrome bg must match §II.5: got {zen_bg:?}");

        // No-op when flag is not set.
        editor.theme_rederive = false;
        let did3 = rederive_theme_if_requested(&mut editor, &tc, &env);
        assert!(!did3, "must return false when flag is not set");
    }

    /// Finding 1 (pre-merge gate): rederive must use the PICKER-COMMITTED identity, not
    /// the startup config. Arrange: cfg.theme names flexoki-dark (or is empty), but
    /// editor.theme_identity = Builtin("tokyo-night") — as set by a picker Enter commit.
    /// After rederive the applied theme must be tokyo-night, and its ChromeOverlay must
    /// flip between Full and Zen §B.3 values.
    #[test]
    fn rederive_respects_picker_committed_theme() {
        use wordcartel_core::theme::{ChromeDisposition, Color, SemanticElement};
        use crate::theme_resolve::EnvSnapshot;
        use crate::theme_cmds::rederive_theme_if_requested;
        use crate::settings::ThemeIdentity;

        // cfg.theme has flexoki-dark — this is the "startup config" that would revert if
        // rederive ignores the live identity.
        let tc = crate::config::ThemeConfig {
            name: Some("flexoki-dark".into()),
            ..Default::default()
        };
        let env = EnvSnapshot {
            no_color: false,
            colorterm: Some("truecolor".into()),
            term: Some("xterm-256color".into()),
        };

        let mut editor = Editor::new_from_text("x", None, (80, 24));
        // Simulate a picker commit: identity is now tokyo-night.
        editor.theme_identity = ThemeIdentity::Builtin("tokyo-night".into());

        // Full disposition rederive: must yield tokyo-night with Full ChromeOverlay.
        editor.chrome_disposition = ChromeDisposition::Full;
        editor.theme_rederive = true;
        let did = rederive_theme_if_requested(&mut editor, &tc, &env);
        assert!(did, "flag was set — must return true");
        assert_eq!(editor.theme.name, "tokyo-night",
            "rederive must apply the picker-committed identity, not the config name");
        let full_overlay_bg = editor.theme.face(SemanticElement::ChromeOverlay).bg;
        // §II.5 pin: tokyo FULL ChromeOverlay bg = #3d405a — the modal shares the dropdown
        // (ChromeMuted) level-2 tone (3-tone ladder, user decision 2026-07-06).
        assert_eq!(full_overlay_bg, Some(Color::Rgb { r: 0x3d, g: 0x40, b: 0x5a }),
            "tokyo-night Full ChromeOverlay bg (§II.5 final): got {full_overlay_bg:?}");

        // Zen disposition rederive: same identity, overlay should collapse.
        editor.chrome_disposition = ChromeDisposition::Zen;
        editor.theme_rederive = true;
        let did2 = rederive_theme_if_requested(&mut editor, &tc, &env);
        assert!(did2, "Zen rederive must return true");
        assert_eq!(editor.theme.name, "tokyo-night", "identity preserved across disposition change");
        let zen_overlay_bg = editor.theme.face(SemanticElement::ChromeOverlay).bg;
        // §II.5 pin: tokyo ZEN ChromeOverlay bg = #2c2d40 (= ZEN ChromeMuted bg — 3-tone ladder).
        assert_eq!(zen_overlay_bg, Some(Color::Rgb { r: 0x2c, g: 0x2d, b: 0x40 }),
            "tokyo-night Zen ChromeOverlay bg (§II.5 final): got {zen_overlay_bg:?}");
    }

    /// Finding 2 (pre-merge gate): preview_selected_theme must apply the Ansi16 sentinel-fill
    /// policy so a preview in an Ansi16 terminal shows the fixed table (DarkGray) not a
    /// quantized derived value. Arrange: editor.depth = Ansi16, picker row = flexoki-dark.
    /// flexoki-dark canvas (#100f0f) quantizes to Black → dark arm: Chrome bg = DarkGray.
    #[test]
    fn preview_applies_ansi16_policy() {
        use wordcartel_core::theme::{Depth, Color, SemanticElement};
        use crate::theme_cmds::preview_selected_theme;

        let mut editor = Editor::new_from_text("x", None, (80, 24));
        editor.depth = Depth::Ansi16;
        // Open a picker with flexoki-dark as the selected row.
        editor.open_theme_picker();
        {
            let tp = editor.theme_picker.as_mut().unwrap();
            tp.rows = vec!["flexoki-dark".to_string()];
            tp.selected = 0;
        }
        preview_selected_theme(&mut editor);
        let chrome_bg = editor.theme.face(SemanticElement::Chrome).bg;
        // Dark canvas arm: Chrome bg must be DarkGray (fixed table), not a quantized derived value.
        assert_eq!(chrome_bg, Some(Color::DarkGray),
            "Ansi16 preview must apply sentinel-fill policy: Chrome bg = DarkGray, got {chrome_bg:?}");
    }
}
