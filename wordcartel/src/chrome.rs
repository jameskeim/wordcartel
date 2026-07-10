//! Extracted verbatim from app.rs (Effort H1 round 2).

/// Recompute `editor.mouse.scrollbar_visible` from the clock, honoring the mode.
///
/// Must be called at the top of the run loop (with `clock.now_ms()`) so that
/// the scrollbar fades exactly when `scrollbar_until_ms` or a dwell deadline
/// expires, driven by the loop's `deadline` (not an idle Tick).
///
/// **Fire order is load-bearing:** dwell/grace deadlines are fired FIRST so that
/// a deadline landing exactly on `now_ms` flips `scrollbar_revealed` before we
/// read it to compute `scrollbar_visible`.
pub fn recompute_scrollbar_visible(editor: &mut crate::editor::Editor, now_ms: u64) {
    use crate::config::TransientMode;
    // Fire the Auto dwell/grace deadlines FIRST (armed by the mouse Moved arm), so a
    // deadline landing exactly on `now_ms` flips `scrollbar_revealed` BEFORE we read it.
    if editor.scrollbar_mode == TransientMode::Auto {
        if editor.mouse.scrollbar_reveal_due.is_some_and(|d| now_ms >= d) {
            editor.mouse.scrollbar_reveal_due = None;
            editor.mouse.scrollbar_revealed = true;
        }
        if editor.mouse.scrollbar_hide_due.is_some_and(|d| now_ms >= d) {
            editor.mouse.scrollbar_hide_due = None;
            editor.mouse.scrollbar_revealed = false;
        }
    } else {
        editor.mouse.scrollbar_reveal_due = None;
        editor.mouse.scrollbar_hide_due = None;
        editor.mouse.scrollbar_revealed = false;
    }
    editor.mouse.scrollbar_visible = match editor.scrollbar_mode {
        TransientMode::On  => true,
        TransientMode::Off => false,
        // Auto: scroll activity (the existing channel) OR a live right-edge dwell.
        TransientMode::Auto => now_ms < editor.mouse.scrollbar_until_ms
            || editor.mouse.scrollbar_revealed,
    };
}

/// Fire the auto-mode menu-bar deadlines (armed by the mouse Moved arm). Gated on
/// Auto — a stale due must never fire in Pinned/Hidden (spec M2).
pub fn recompute_menu_bar(editor: &mut crate::editor::Editor, now_ms: u64) {
    if editor.menu_bar_mode != crate::config::MenuBarMode::Auto {
        // Defense-in-depth (Fable plan-review M5): dues arm only in Auto and every
        // mode transition clears them, so this state is unreachable — but CLEARING
        // (never firing) here makes the deadline-array no-spin invariant
        // unconditional instead of resting on the transition-clears.
        editor.mouse.menu_reveal_due = None;
        editor.mouse.menu_hide_due = None;
        return;
    }
    if editor.mouse.menu_reveal_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_reveal_due = None;
        editor.mouse.menu_bar_revealed = true;
    }
    if editor.mouse.menu_hide_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_hide_due = None;
        editor.mouse.menu_bar_revealed = false;
    }
}

/// Whether the NORMAL idle status info line should paint. A message / prompt /
/// search / minibuffer force it regardless of mode (no-silent-UI) — those are
/// handled in render.rs before this is consulted; this governs only the idle line.
pub fn status_line_visible(editor: &crate::editor::Editor) -> bool {
    use crate::config::TransientMode;
    match editor.status_line_mode {
        TransientMode::On  => true,
        // Off is never assigned to status (coerced to Auto at parse); treat defensively as Auto.
        TransientMode::Off | TransientMode::Auto =>
            !editor.status.is_empty()
                || editor.mouse.status_revealed
                || editor.prompt.is_some()
                || editor.search.is_some()
                || editor.minibuffer.is_some(),
    }
}

/// Fire the Auto-mode status dwell/grace deadlines (armed by the mouse Moved arm).
pub fn recompute_status_line(editor: &mut crate::editor::Editor, now_ms: u64) {
    use crate::config::TransientMode;
    if editor.status_line_mode != TransientMode::Auto {
        editor.mouse.status_reveal_due = None;
        editor.mouse.status_hide_due = None;
        return;
    }
    if editor.mouse.status_reveal_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.status_reveal_due = None;
        editor.mouse.status_revealed = true;
    }
    if editor.mouse.status_hide_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.status_hide_due = None;
        editor.mouse.status_revealed = false;
    }
}

/// Reconcile the terminal's mouse-capture state with `editor.mouse_capture`.
///
/// Enables or disables mouse capture on the backend when the desired state
/// diverges from `applied`. On disable, clears drag state so no stale Up
/// events are awaited for a capture that will never arrive.
pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut crate::editor::Editor, backend: &mut W, applied: &mut bool) {
    if editor.mouse_capture != *applied {
        if editor.mouse_capture {
            if crossterm::execute!(backend, crossterm::event::EnableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        } else {
            // clear drag state regardless of IO outcome — it is local state,
            // not tied to the terminal write succeeding.
            editor.mouse.dragging = false;
            editor.mouse.scrollbar_dragging = false;
            editor.mouse.anchor = None;
            editor.mouse.menu_reveal_due = None;
            editor.mouse.menu_hide_due = None;
            editor.mouse.menu_bar_revealed = false;
            if crossterm::execute!(backend, crossterm::event::DisableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::TestClock;

    #[test]
    fn scrollbar_visible_recomputed_against_clock() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.mouse.scrollbar_until_ms = 1000;
        crate::chrome::recompute_scrollbar_visible(&mut e, 500); // before deadline
        assert!(e.mouse.scrollbar_visible);
        crate::chrome::recompute_scrollbar_visible(&mut e, 1200); // after
        assert!(!e.mouse.scrollbar_visible);
    }

    #[test]
    fn scrollbar_visible_respects_mode() {
        use crate::config::TransientMode;
        use crate::chrome::recompute_scrollbar_visible;
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        // On: always visible regardless of activity/dwell.
        e.scrollbar_mode = TransientMode::On;
        recompute_scrollbar_visible(&mut e, 10_000);
        assert!(e.mouse.scrollbar_visible, "On → always visible");
        // Off: never visible even with fresh activity.
        e.scrollbar_mode = TransientMode::Off;
        e.mouse.scrollbar_until_ms = 20_000;
        recompute_scrollbar_visible(&mut e, 10_000);
        assert!(!e.mouse.scrollbar_visible, "Off → never visible");
        // Auto: visible while activity OR dwell holds; hidden once both lapse.
        e.scrollbar_mode = TransientMode::Auto;
        e.mouse.scrollbar_until_ms = 20_000; e.mouse.scrollbar_revealed = false;
        recompute_scrollbar_visible(&mut e, 10_000);
        assert!(e.mouse.scrollbar_visible, "Auto + activity → visible");
        e.mouse.scrollbar_until_ms = 0; e.mouse.scrollbar_revealed = true;
        recompute_scrollbar_visible(&mut e, 10_000);
        assert!(e.mouse.scrollbar_visible, "Auto + dwell-revealed → visible");
        e.mouse.scrollbar_revealed = false;
        recompute_scrollbar_visible(&mut e, 10_000);
        assert!(!e.mouse.scrollbar_visible, "Auto + neither → hidden");
    }

    // A1 Task 3 — cases 7 + 8 (app-level: recompute_menu_bar / reconcile)

    /// Case 7: recompute fires in Auto; in non-Auto it clears without firing.
    #[test]
    fn recompute_fires_and_is_mode_gated() {
        use crate::editor::Editor;
        use crate::config::MenuBarMode;

        // Auto: past reveal deadline → revealed = true.
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.menu_bar_mode = MenuBarMode::Auto;
        e.mouse.menu_reveal_due = Some(100);
        crate::chrome::recompute_menu_bar(&mut e, 101);
        assert!(e.mouse.menu_bar_revealed, "Auto: past reveal due must fire");
        assert!(e.mouse.menu_reveal_due.is_none(), "reveal due cleared after firing");

        // Pinned: past reveal deadline → cleared WITHOUT firing (defense-in-depth).
        let mut e2 = Editor::new_from_text("x\n", None, (40, 8));
        e2.menu_bar_mode = MenuBarMode::Pinned;
        e2.mouse.menu_reveal_due = Some(100);
        crate::chrome::recompute_menu_bar(&mut e2, 101);
        assert!(!e2.mouse.menu_bar_revealed, "Pinned: due CLEARED, revealed NOT set");
        assert!(e2.mouse.menu_reveal_due.is_none(), "due cleared in Pinned");

        // Auto: past hide deadline → revealed = false.
        let mut e3 = Editor::new_from_text("x\n", None, (40, 8));
        e3.menu_bar_mode = MenuBarMode::Auto;
        e3.mouse.menu_bar_revealed = true;
        e3.mouse.menu_hide_due = Some(200);
        crate::chrome::recompute_menu_bar(&mut e3, 201);
        assert!(!e3.mouse.menu_bar_revealed, "Auto: past hide due must fire → unrevealed");
        assert!(e3.mouse.menu_hide_due.is_none(), "hide due cleared after firing");
    }

    /// Case 8: capture-off clears all three menu-bar fields.
    #[test]
    fn capture_disable_clears_menu_bar_state() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.mouse_capture = true;
        e.mouse.menu_bar_revealed = true;
        e.mouse.menu_reveal_due = Some(500);
        e.mouse.menu_hide_due = Some(900);
        let mut buf = Vec::<u8>::new();
        let mut applied = true;
        e.mouse_capture = false;
        crate::chrome::reconcile_mouse_capture(&mut e, &mut buf, &mut applied);
        assert!(!e.mouse.menu_bar_revealed, "revealed cleared on capture disable");
        assert!(e.mouse.menu_reveal_due.is_none(), "menu_reveal_due cleared on capture disable");
        assert!(e.mouse.menu_hide_due.is_none(), "menu_hide_due cleared on capture disable");
    }

    /// Finding 1 regression: wheel event sets scrollbar_until_ms; recomputing
    /// immediately after (now == t, t < t+1200) must yield visible == true.
    /// A later recompute at t+1300 must yield false (bar fades after deadline).
    #[test]
    fn wheel_then_recompute_makes_scrollbar_visible() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        crate::derive::rebuild(&mut e);
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let t: u64 = 5000;
        let clk = TestClock(t);
        let (tx, _rx) = std::sync::mpsc::channel();
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // Dispatch the scroll event (sets scrollbar_until_ms = t + 1200).
        crate::mouse::handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx);
        // Recompute at t (now < until) — bar must be visible.
        crate::chrome::recompute_scrollbar_visible(&mut e, t);
        assert!(e.mouse.scrollbar_visible, "scrollbar must be visible immediately after a scroll event");
        // Recompute after the fade deadline — bar must hide.
        crate::chrome::recompute_scrollbar_visible(&mut e, t + 1300);
        assert!(!e.mouse.scrollbar_visible, "scrollbar must hide after scrollbar_until_ms expires");
    }

    // Task 4 — status_line_visible

    /// `status_line_visible` must return false under Auto with no message/reveal,
    /// force-true on a non-empty status message (no-silent-UI), and true under On always.
    #[test]
    fn status_line_visible_forces_on_message_even_in_auto() {
        use crate::config::TransientMode;
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        e.status_line_mode = TransientMode::Auto;
        e.mouse.status_revealed = false;
        e.status.clear();
        assert!(!crate::chrome::status_line_visible(&e), "Auto idle + no message → info line hidden (calm)");
        e.status = "saved".into();
        assert!(crate::chrome::status_line_visible(&e), "a message force-reveals even under Auto (no-silent-UI)");
        e.status.clear();
        e.mouse.status_revealed = true;
        assert!(crate::chrome::status_line_visible(&e), "Auto + dwell-revealed → visible");
        e.status_line_mode = TransientMode::On;
        e.mouse.status_revealed = false;
        assert!(crate::chrome::status_line_visible(&e), "On → always visible");
    }
}
