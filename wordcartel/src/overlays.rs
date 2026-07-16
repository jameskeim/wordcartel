//! Input-overlay dispatch hub. Static fn-pointer table; one row per overlay, keyed by an
//! exhaustive `OverlayId`. Collapses the hand-parallel overlay enumerations (is-active,
//! intercept-chain, mouse, render, XOR-close) into one table + delegating folds. Extracted
//! from editor.rs/app.rs/mouse.rs/render_overlays.rs (Effort H21).
//!
//! Plugin-forward (the shape `timers.rs` reserved for plugin timers, which shipped as ONE
//! static row reading dynamic `Editor::pending_plugin_timers`): a future plugin panel is ONE
//! static `OverlayId::PluginPanel` row whose slots read dynamic `editor.plugin_panel` state —
//! content submitted edge-triggered / version-stamped / capped by the P3 pump, painted by a
//! builtin Rust painter, keys forwarded to Lua as events. The row is static; the content is
//! dynamic. No `PluginPanel` variant ships in H21 (it would be dead code and defeat the
//! exhaustiveness guarantee).
use crate::editor::Editor;

/// Every input overlay, exhaustive. A new overlay is a new variant; `row()` then forces it
/// into `OVERLAYS`, and every table-derived consumer inherits it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum OverlayId {
    Splash, Menu, Palette, ThemePicker, CursorPicker, FileBrowser,
    Prompt, Minibuffer, Search, Diag, Outline,
}

impl OverlayId {
    /// All variants, in intercept-chain order (splash first — §2.6). `Splash` MUST stay
    /// index 0: the intercept loop skips `ALL[1..]` after firing the splash row, and the
    /// paint early-return keys off it.
    pub(crate) const ALL: &'static [OverlayId] = &[
        OverlayId::Splash, OverlayId::Menu, OverlayId::Palette, OverlayId::ThemePicker,
        OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Prompt,
        OverlayId::Minibuffer, OverlayId::Search, OverlayId::Diag, OverlayId::Outline,
    ];

    /// The table row for this id. EXHAUSTIVE match — a new variant fails to compile until it
    /// is placed here (the guarantee that closes the silent-UI leak). The `OVERLAYS[i]`
    /// indices are pinned by the bijection test.
    pub(crate) fn row(self) -> &'static OverlayRow {
        match self {
            OverlayId::Splash       => &OVERLAYS[0],
            OverlayId::Menu         => &OVERLAYS[1],
            OverlayId::Palette      => &OVERLAYS[2],
            OverlayId::ThemePicker  => &OVERLAYS[3],
            OverlayId::CursorPicker => &OVERLAYS[4],
            OverlayId::FileBrowser  => &OVERLAYS[5],
            OverlayId::Prompt       => &OVERLAYS[6],
            OverlayId::Minibuffer   => &OVERLAYS[7],
            OverlayId::Search       => &OVERLAYS[8],
            OverlayId::Diag         => &OVERLAYS[9],
            OverlayId::Outline      => &OVERLAYS[10],
        }
    }
}

/// One overlay's routing slots. Fields grow as H21 folds each axis (is_active → intercept →
/// close → mouse → render); Task 1 introduces `is_active` only.
pub(crate) struct OverlayRow {
    /// Read only by the guardrail tests today (bijection/uniqueness) and reserved as the
    /// stable plugin identity for a future panel; unread in a non-test release build.
    #[allow(dead_code)]
    pub(crate) name: &'static str,
    /// Read only by the bijection test today; reserved plugin identity. Unread in release.
    #[allow(dead_code)]
    pub(crate) id: OverlayId,
    pub(crate) is_active: fn(&Editor) -> bool,
}

/// The overlay table, in `ALL` order. Non-capturing closures coerce to the fn-pointer fields.
pub(crate) static OVERLAYS: &[OverlayRow] = &[
    OverlayRow { name: "splash",        id: OverlayId::Splash,       is_active: |e| e.splash.is_some() },
    OverlayRow { name: "menu",          id: OverlayId::Menu,         is_active: |e| e.menu.is_some() },
    OverlayRow { name: "palette",       id: OverlayId::Palette,      is_active: |e| e.palette.is_some() },
    OverlayRow { name: "theme_picker",  id: OverlayId::ThemePicker,  is_active: |e| e.theme_picker.is_some() },
    OverlayRow { name: "cursor_picker", id: OverlayId::CursorPicker, is_active: |e| e.cursor_picker.is_some() },
    OverlayRow { name: "file_browser",  id: OverlayId::FileBrowser,  is_active: |e| e.file_browser.is_some() },
    OverlayRow { name: "prompt",        id: OverlayId::Prompt,       is_active: |e| e.prompt.is_some() },
    OverlayRow { name: "minibuffer",    id: OverlayId::Minibuffer,   is_active: |e| e.minibuffer.is_some() },
    OverlayRow { name: "search",        id: OverlayId::Search,       is_active: |e| e.search.is_some() },
    OverlayRow { name: "diag",          id: OverlayId::Diag,         is_active: |e| e.diag.is_some() },
    OverlayRow { name: "outline",       id: OverlayId::Outline,      is_active: |e| e.outline.is_some() },
];

/// True iff any input overlay owns the screen — the single source for both
/// `Editor::has_active_input_overlay` and `mouse::no_overlay_open`. Includes `splash`
/// (Q4 delta): the mouse path now treats the splash as active, so dwell timers cannot arm
/// under it.
pub(crate) fn any_active(editor: &Editor) -> bool {
    OverlayId::ALL.iter().any(|id| (id.row().is_active)(editor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    /// Enum↔table bijection + Splash-first ordering. Pins the invariants the exhaustive
    /// `row()` match cannot: order and identity across ALL / OVERLAYS.
    #[test]
    fn overlay_table_is_a_bijection_splash_first() {
        assert_eq!(OverlayId::ALL.len(), OVERLAYS.len(), "ALL and OVERLAYS same length");
        assert_eq!(OverlayId::ALL[0], OverlayId::Splash, "Splash must be row 0 (§2.6 skip + precedence)");
        for (i, id) in OverlayId::ALL.iter().enumerate() {
            assert_eq!(OVERLAYS[i].id, *id, "OVERLAYS order matches ALL at {i}");
            assert_eq!(id.row().id, *id, "row() round-trips id for {id:?}");
        }
        // names unique
        let mut names: Vec<&str> = OVERLAYS.iter().map(|r| r.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "overlay names are unique");
    }

    /// `any_active` is true for each overlay individually (subsumes render.rs's B11 census
    /// for the is-active axis; the full sweep lands in the sweep task).
    #[test]
    fn any_active_true_for_each_overlay() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mk = || Editor::new_from_text("hello world\n", None, (40, 12));
        let mut e = mk(); e.open_search(crate::search_overlay::Phase::Find, 0);
        assert!(any_active(&e), "search");
        let mut e = mk(); e.open_palette();
        assert!(any_active(&e), "palette");
        let mut e = mk(); e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        assert!(any_active(&e), "splash");
        let e = mk();
        assert!(!any_active(&e), "no overlay ⇒ false");
    }

    /// Q4 delta (spec §3): with the splash up, a mouse-Moved must NOT arm the menu-bar or
    /// scrollbar dwell timers. `no_overlay_open` now counts splash, so `mouse::handle` routes
    /// the event to the overlay path and returns before the dwell-arming block runs.
    #[test]
    fn no_dwell_arming_while_splash_is_up() {
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello\n", None, (40, 12));
        e.menu_bar_mode = crate::config::MenuBarMode::Auto;
        e.scrollbar_mode = crate::config::TransientMode::Auto;
        e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        // A move onto row 0 (menu-bar dwell region) and the right edge (scrollbar region).
        for (col, row) in [(5u16, 0u16), (39u16, 5u16)] {
            let ev = MouseEvent { kind: MouseEventKind::Moved, column: col, row, modifiers: KeyModifiers::NONE };
            crate::mouse::handle(&mut e, ev, &reg, &km, &ex, &clock, &tx);
        }
        assert!(e.mouse.menu_reveal_due.is_none(), "no menu dwell armed under splash");
        assert!(e.mouse.scrollbar_reveal_due.is_none(), "no scrollbar dwell armed under splash");
    }
}
