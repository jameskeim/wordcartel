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
use crate::app::{Msg, Handled};

/// The non-editor dispatch context, bundled so every overlay `intercept` (and later `mouse`)
/// fn shares ONE signature. The editor is passed SEPARATELY as `&mut Editor` — deliberately
/// EXCLUDED here to avoid a `&mut` aliasing tangle in the table loop (contrast
/// `registry::Ctx`, which OWNS `editor: &mut Editor` and holds `msg_tx` by VALUE for a
/// `'static` spawned thread; `DispatchCtx` borrows `msg_tx` — it never outlives the loop).
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a crate::registry::Registry,
    pub(crate) keymap: &'a crate::keymap::KeyTrie,
    pub(crate) ex: &'a dyn crate::jobs::Executor,
    pub(crate) clock: &'a dyn wordcartel_core::history::Clock,
    pub(crate) msg_tx: &'a std::sync::mpsc::Sender<Msg>,
    /// The filesystem seam (owned handle — the listing thread clones it in).
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}

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

/// Where an overlay paints. Every OverlayId answers this axis (the render-coverage test
/// asserts it) WITHOUT forcing false uniformity: frame-owned surfaces carry a painter fn; the
/// status-row trio carry a marker (their painting stays in render.rs, untouched).
/// `Copy` so the render loop / coverage test can read `id.row().render` out of the
/// `&'static OverlayRow` by value (both variants — a fn pointer and a unit — are Copy).
#[derive(Clone, Copy)]
pub(crate) enum RenderSite {
    /// Painted by `render_overlays`. The paint SEQUENCE is `RENDER_ORDER` — a permutation
    /// distinct from OVERLAYS/intercept order (§2.3.2).
    Frame(fn(&mut ratatui::Frame, &mut Editor, &crate::render::ChromeStyles)),
    /// Painted on the shared status row inside `render.rs` (search bar / minibuffer / prompt).
    /// NOT relocated by H21 — the marker exists only so the axis is exhaustive (absent from
    /// RENDER_ORDER, which covers only the Frame overlays).
    StatusRow,
}

/// Frame-paint order — a permutation over the Frame-site overlays ONLY (the StatusRow trio
/// are absent; they paint in render.rs). DISTINCT from OVERLAYS/intercept order. Grounded
/// verbatim against `render_overlays::paint`'s block sequence: splash, palette, outline,
/// theme_picker, cursor_picker, file_browser, menu DROPDOWN, diag. (The always-on menu BAR
/// chrome is NOT in this walk — it is painted by a standalone step pinned at the `Menu` slot;
/// only the dropdown is the `Menu` row's Frame painter — spec §2.3.1/§2.3.2.)
pub(crate) static RENDER_ORDER: &[OverlayId] = &[
    OverlayId::Splash, OverlayId::Palette, OverlayId::Outline, OverlayId::ThemePicker,
    OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Menu, OverlayId::Diag,
];

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
    /// The overlay's input interceptor. Read by `reduce_dispatch`'s table loop (H10 fold):
    /// each active overlay gets first refusal at the message in `ALL` order.
    pub(crate) intercept: fn(Msg, &mut Editor, &DispatchCtx) -> Handled,
    /// Close this overlay (null its own field). Read by `close_all`, which folds the
    /// hand-written sibling-null lists in every `open_*`, `dispatch_overlay_command`, and the
    /// registry `"menu"` command into one loop.
    pub(crate) close: fn(&mut Editor),
    /// The overlay's mouse slot. Read by `mouse::route_overlay`'s find-active-then-call
    /// dispatch (Task 4): the active overlay's slot consumes the event (wheel/click/click-away),
    /// replacing the hand-written `if editor.X.is_some()` chain.
    pub(crate) mouse: fn(&mut Editor, crossterm::event::MouseEvent, ratatui::layout::Rect, &DispatchCtx),
    /// Where this overlay paints (§2.3). `Frame(fn)` overlays paint into the frame via
    /// `render_overlays`, walked in `RENDER_ORDER`; the status-row trio carry `StatusRow`
    /// (painted in render.rs, untouched). Read by the render loop and the coverage test.
    pub(crate) render: RenderSite,
}

/// The overlay table, in `ALL` order. Non-capturing closures coerce to the fn-pointer fields.
pub(crate) static OVERLAYS: &[OverlayRow] = &[
    OverlayRow { name: "splash",        id: OverlayId::Splash,       is_active: |e| e.splash.is_some(),
        intercept: crate::splash::intercept, close: |e| e.splash = None, mouse: crate::splash::mouse,
        render: RenderSite::Frame(crate::render_overlays::paint_splash) },
    OverlayRow { name: "menu",          id: OverlayId::Menu,         is_active: |e| e.menu.is_some(),
        intercept: crate::menu::intercept, close: |e| e.menu = None, mouse: crate::mouse::mouse_menu,
        render: RenderSite::Frame(crate::render_overlays::paint_menu_dropdown) },
    OverlayRow { name: "palette",       id: OverlayId::Palette,      is_active: |e| e.palette.is_some(),
        intercept: crate::palette::intercept, close: |e| e.palette = None, mouse: crate::mouse::mouse_palette,
        render: RenderSite::Frame(crate::render_overlays::paint_palette) },
    OverlayRow { name: "theme_picker",  id: OverlayId::ThemePicker,  is_active: |e| e.theme_picker.is_some(),
        intercept: crate::theme_picker::intercept, close: |e| e.theme_picker = None, mouse: crate::mouse::mouse_theme_picker,
        render: RenderSite::Frame(crate::render_overlays::paint_theme_picker) },
    OverlayRow { name: "cursor_picker", id: OverlayId::CursorPicker, is_active: |e| e.cursor_picker.is_some(),
        intercept: crate::cursor_picker::intercept, close: |e| e.cursor_picker = None, mouse: crate::mouse::mouse_cursor_picker,
        render: RenderSite::Frame(crate::render_overlays::paint_cursor_picker) },
    OverlayRow { name: "file_browser",  id: OverlayId::FileBrowser,  is_active: |e| e.file_browser.is_some(),
        intercept: crate::file_browser::intercept, close: |e| e.file_browser = None, mouse: crate::mouse::mouse_file_browser,
        render: RenderSite::Frame(crate::render_overlays::paint_file_browser) },
    OverlayRow { name: "prompt",        id: OverlayId::Prompt,       is_active: |e| e.prompt.is_some(),
        intercept: crate::prompts::intercept, close: |e| e.prompt = None, mouse: crate::mouse::mouse_prompt,
        render: RenderSite::StatusRow },
    OverlayRow { name: "minibuffer",    id: OverlayId::Minibuffer,   is_active: |e| e.minibuffer.is_some(),
        intercept: crate::minibuffer::intercept, close: |e| e.minibuffer = None, mouse: crate::mouse::mouse_minibuffer,
        render: RenderSite::StatusRow },
    OverlayRow { name: "search",        id: OverlayId::Search,       is_active: |e| e.search.is_some(),
        intercept: crate::search_ui::intercept, close: |e| e.search = None, mouse: crate::mouse::mouse_search,
        render: RenderSite::StatusRow },
    OverlayRow { name: "diag",          id: OverlayId::Diag,         is_active: |e| e.diag.is_some(),
        intercept: crate::diag_overlay::intercept, close: |e| e.diag = None, mouse: crate::mouse::mouse_diag,
        render: RenderSite::Frame(crate::render_overlays::paint_diag) },
    OverlayRow { name: "outline",       id: OverlayId::Outline,      is_active: |e| e.outline.is_some(),
        intercept: crate::outline_overlay::intercept, close: |e| e.outline = None, mouse: crate::mouse::mouse_outline,
        render: RenderSite::Frame(crate::render_overlays::paint_outline) },
];

/// True iff any input overlay owns the screen — the single source for both
/// `Editor::has_active_input_overlay` and `mouse::no_overlay_open`. Includes `splash`
/// (Q4 delta): the mouse path now treats the splash as active, so dwell timers cannot arm
/// under it.
pub(crate) fn any_active(editor: &Editor) -> bool {
    OverlayId::ALL.iter().any(|id| (id.row().is_active)(editor))
}

/// Close every overlay (hold the single-overlay XOR invariant). Replaces the sibling-null
/// lists in every `open_*`, in `dispatch_overlay_command`, in the registry `"menu"` command,
/// and (Task 4) `route_overlay`'s Down-left close arms. NOT the `save.rs` post-buffer-replace
/// stale-clears — those clear only `search`/`diag` for content staleness, not the XOR set.
pub(crate) fn close_all(editor: &mut Editor) {
    for row in OVERLAYS { (row.close)(editor); }
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

    /// Render-axis coverage: every OverlayId has a RenderSite (exhaustive by `row()`), and
    /// RENDER_ORDER contains EXACTLY the ids whose RenderSite is Frame — Splash first, no
    /// StatusRow overlay in the paint walk, no Frame overlay missing from it.
    #[test]
    fn render_order_is_exactly_the_frame_overlays() {
        assert_eq!(RENDER_ORDER[0], OverlayId::Splash, "paint early-return keys off Splash first");
        let frame_ids: Vec<OverlayId> = OverlayId::ALL.iter().copied()
            .filter(|id| matches!(id.row().render, RenderSite::Frame(_)))
            .collect();
        let mut walk = RENDER_ORDER.to_vec();
        let mut frame_sorted = frame_ids.clone();
        walk.sort_by_key(|id| format!("{id:?}"));
        frame_sorted.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(walk, frame_sorted, "RENDER_ORDER == the set of Frame-site overlays");
        // The StatusRow trio must NOT appear in the paint walk.
        for id in [OverlayId::Prompt, OverlayId::Minibuffer, OverlayId::Search] {
            assert!(matches!(id.row().render, RenderSite::StatusRow), "{id:?} is StatusRow");
            assert!(!RENDER_ORDER.contains(&id), "{id:?} not in RENDER_ORDER");
        }
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

    /// The input fold must preserve the real chain order: splash fires BEFORE marks fires
    /// BEFORE the other overlays. With BOTH a pending mark AND the splash up, a key-Press
    /// must dismiss the SPLASH (not resolve the mark) — proving splash still precedes marks.
    #[test]
    fn splash_intercept_precedes_marks() {
        use crossterm::event::{KeyEvent, KeyCode, KeyEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello\n", None, (40, 12));
        e.splash = Some(crate::splash::Splash::new(&km, "0.0.0"));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let key = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE };
        crate::app::reduce(
        crate::app::Msg::Input(crossterm::event::Event::Key(key)),
            &mut e,
            &reg,
            &km,
            &ex,
            &clock,
            &tx,
        &crate::test_support::test_fs(),
        );
        assert!(e.splash.is_none(), "splash dismissed first");
        assert!(e.pending_mark.is_some(), "the mark was NOT consumed — splash preceded marks");
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
            crate::mouse::handle(&mut e, ev, &reg, &km, &ex, &clock, &tx, &crate::test_support::test_fs());
        }
        assert!(e.mouse.menu_reveal_due.is_none(), "no menu dwell armed under splash");
        assert!(e.mouse.scrollbar_reveal_due.is_none(), "no scrollbar dwell armed under splash");
    }

    /// A mouse Down under an open list overlay is consumed by the overlay's mouse slot —
    /// it must NOT fall through to an editor gesture (no click-through while a modal is up).
    #[test]
    fn click_under_overlay_does_not_move_caret() {
        use crossterm::event::{MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("hello world\ntwo\n", None, (40, 12));
        crate::derive::rebuild(&mut e);
        e.open_palette();
        let before = crate::nav::head(&e);
        // A click well outside the palette rect (bottom-left) — with no palette open this
        // would move the caret; under the palette it is consumed (close-away or no-op).
        let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 11,
            modifiers: KeyModifiers::NONE };
        crate::mouse::handle(&mut e, ev, &reg, &km, &ex, &clock, &tx, &crate::test_support::test_fs());
        // Either the palette closed (click-away) or stayed; in NEITHER case did the editor
        // caret jump to the clicked cell — the event never reached the editor gesture path.
        assert_eq!(crate::nav::head(&e), before, "click under palette did not move the caret");
    }

    /// The completeness sweep (subsumes render.rs's B11 census). For EACH overlay: open it,
    /// assert exactly one row is_active (XOR); a key-Press routed through `reduce` is consumed
    /// (buffer version unchanged — no keystroke leak); and the overlay's OWN `mouse` slot,
    /// called DIRECTLY with a text-band Down-left, does not panic and does not mutate the
    /// buffer (a per-slot no-data-loss guardrail on a stray click while a modal is up).
    /// Assertion (c) calls the row's `mouse` fn directly rather than through `mouse::handle`:
    /// `handle` gates ALL click routing on "some overlay is active" and then hands off to
    /// whichever overlay is active, so once ANY overlay is open, routing through `handle`
    /// would pass regardless of what THIS overlay's own slot does — it cannot prove
    /// per-slot behavior, only that overlay-vs-no-overlay dispatch happened (already covered
    /// by (a)). Calling the slot directly makes the assertion load-bearing per overlay.
    #[test]
    #[allow(clippy::type_complexity)] // test-local table of (name, opener closure) pairs; not a public API surface
    fn every_overlay_is_active_xor_and_consumes_key_and_click() {
        use crossterm::event::{Event, KeyEvent, KeyCode, KeyEventKind, KeyEventState,
            MouseEvent, MouseEventKind, MouseButton, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        // (name, opener) — every OverlayId, opened via a real path.
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(&crate::fsx::RealFs, std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            // (a) exactly one active
            let active = OverlayId::ALL.iter().filter(|id| (id.row().is_active)(&e)).count();
            assert_eq!(active, 1, "{name}: exactly one overlay active (XOR)");
            // (b) a key-Press is consumed — the buffer version must not change (every overlay
            // intercept returns Handled::Done for ALL key messages, so 'z' never reaches the buffer).
            let v0 = e.active().document.version;
            let key = KeyEvent { code: KeyCode::Char('z'), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE };
            crate::app::reduce(crate::app::Msg::Input(Event::Key(key)), &mut e, &reg, &km, &ex, &clock, &tx, &crate::test_support::test_fs());
            assert_eq!(e.active().document.version, v0, "{name}: key-Press did not edit the buffer");
            // (c) the overlay's OWN mouse slot, called directly, does not panic and does not
            // mutate the buffer on a stray text-band Down-left click (see the doc comment
            // above for why this bypasses `mouse::handle`). Re-open in case the key above
            // dismissed the overlay (e.g. splash Press dismisses it).
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            let id = *OverlayId::ALL.iter().find(|id| (id.row().is_active)(&e))
                .unwrap_or_else(|| panic!("{name}: no active overlay to find its mouse slot"));
            let (w, h) = e.active().view.area;
            let area = ratatui::layout::Rect::new(0, 0, w, h);
            let ctx = DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &crate::test_support::test_fs() };
            let v0 = e.active().document.version;
            let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left),
                column: 6, row: 9, modifiers: KeyModifiers::NONE };
            (id.row().mouse)(&mut e, click, area, &ctx);
            assert_eq!(e.active().document.version, v0,
                "{name}: mouse slot did not mutate the buffer on a stray click (no data loss)");
        }
    }

    /// A21: every overlay's mouse slot consumes a `Moved` (hover) without panic and without
    /// mutating the document buffer — the no-data-loss guardrail for hover, across all 11 slots
    /// (the four OUT overlays + splash must no-op it too). Mirrors the Down-leg assertion in
    /// `every_overlay_is_active_xor_and_consumes_key_and_click`, calling the slot directly.
    #[test]
    #[allow(clippy::type_complexity)] // test-local table of (name, opener closure) pairs
    fn every_overlay_consumes_moved_without_panic_or_data_loss() {
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(&crate::fsx::RealFs, std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("hello world\nsecond line here\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            let id = *OverlayId::ALL.iter().find(|id| (id.row().is_active)(&e))
                .unwrap_or_else(|| panic!("{name}: no active overlay"));
            let (w, h) = e.active().view.area;
            let area = ratatui::layout::Rect::new(0, 0, w, h);
            let ctx = DispatchCtx { reg: &reg, keymap: &km, ex: &ex, clock: &clock, msg_tx: &tx, fs: &crate::test_support::test_fs() };
            let v0 = e.active().document.version;
            let moved = MouseEvent { kind: MouseEventKind::Moved, column: 6, row: 9, modifiers: KeyModifiers::NONE };
            (id.row().mouse)(&mut e, moved, area, &ctx);
            assert_eq!(e.active().document.version, v0,
                "{name}: Moved did not mutate the buffer (no data loss)");
        }
    }

    /// `close_all` clears EVERY overlay (the XOR-close axis). Opens all 11 in turn (each via a
    /// real `open_*` or field set), asserts it was active, then asserts `close_all` clears it.
    #[test]
    #[allow(clippy::type_complexity)] // test-local table of (name, opener closure) pairs; not a public API surface
    fn close_all_clears_every_overlay() {
        let diag_fixture = || wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
            message: "m".into(), suggestions: Vec::new(),
        };
        let openers: Vec<(&str, Box<dyn Fn(&mut Editor)>)> = vec![
            ("search",        Box::new(|e: &mut Editor| e.open_search(crate::search_overlay::Phase::Find, 0))),
            ("minibuffer",    Box::new(|e: &mut Editor| e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter))),
            ("palette",       Box::new(|e: &mut Editor| e.open_palette())),
            ("outline",       Box::new(|e: &mut Editor| e.open_outline())),
            ("theme_picker",  Box::new(|e: &mut Editor| e.open_theme_picker())),
            ("file_browser",  Box::new(|e: &mut Editor| e.open_file_browser(&crate::fsx::RealFs, std::path::PathBuf::from(".")))),
            ("prompt",        Box::new(|e: &mut Editor| e.open_prompt(crate::prompt::Prompt::swap_recovery()))),
            ("cursor_picker", Box::new(|e: &mut Editor| e.open_cursor_picker())),
            ("diag",          Box::new(move |e: &mut Editor| e.open_diag(diag_fixture()))),
            ("menu",          Box::new(|e: &mut Editor| { e.menu = Some(crate::menu::empty()); })),
            ("splash",        Box::new(|e: &mut Editor| { e.splash = Some(crate::splash::Splash::new(
                &crate::keymap::KeyTrie::default(), "0.0.0")); })),
        ];
        for (name, open) in openers {
            let mut e = Editor::new_from_text("x\n", None, (40, 12));
            crate::derive::rebuild(&mut e);
            open(&mut e);
            assert!(any_active(&e), "{name}: precondition — overlay open");
            close_all(&mut e);
            assert!(!any_active(&e), "{name}: close_all cleared it");
        }
    }
}
