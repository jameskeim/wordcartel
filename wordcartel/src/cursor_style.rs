//! Writing-caret appearance: DECSCUSR shape/blink composition, the edge-triggered
//! reconcile that emits it (zero writes at rest), and the process-global restore latch
//! the panic hook consults. Sibling of `chrome::reconcile_mouse_capture`.

use crossterm::cursor::SetCursorStyle;
use crate::config::CaretShape;
use crate::editor::Editor;

/// The caret style the caret SHOULD currently have, as OUR own `Copy + PartialEq` pair
/// `(shape, blink)`. Global today (reads only the two options); the seam exists so a per-context
/// map could slot in later WITHOUT rewiring the reconcile — NOT a user-facing feature. `None` ⇒
/// Default shape ⇒ emit nothing (blink inert). crossterm's `SetCursorStyle` is not `PartialEq`
/// (C-11), so the latch/comparison uses this pair, not the crossterm type.
pub fn desired_caret_style(editor: &Editor) -> Option<(CaretShape, bool)> {
    match editor.caret_shape {
        CaretShape::Default => None,
        _ => Some((editor.caret_shape, editor.caret_blink)),
    }
}

/// Map our (shape, blink) pair to the crossterm DECSCUSR command — the ONLY place the crossterm
/// type is produced, at the `execute!` call. Total; `Default` maps to `DefaultUserShape` (never
/// reached via `desired_caret_style`, which returns `None` for Default — kept total so there is
/// no unreachable arm).
pub fn to_set_cursor_style(shape: CaretShape, blink: bool) -> SetCursorStyle {
    match (shape, blink) {
        (CaretShape::Default, _)       => SetCursorStyle::DefaultUserShape,
        (CaretShape::Block, true)      => SetCursorStyle::BlinkingBlock,
        (CaretShape::Block, false)     => SetCursorStyle::SteadyBlock,
        (CaretShape::Beam, true)       => SetCursorStyle::BlinkingBar,
        (CaretShape::Beam, false)      => SetCursorStyle::SteadyBar,
        (CaretShape::Underline, true)  => SetCursorStyle::BlinkingUnderScore,
        (CaretShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
    }
}

/// Edge-triggered: emit a DECSCUSR escape ONLY when the desired style differs from what was last
/// applied. Never writes at rest. Latches success into `applied` (OUR pair) and (on the first real
/// write) into the process-global restore flag. Best-effort: a failed write leaves the latch so it
/// is retried next change — never spun on.
pub fn reconcile_cursor_style<W: std::io::Write>(
    editor: &Editor, backend: &mut W, applied: &mut Option<(CaretShape, bool)>,
) {
    match desired_caret_style(editor) {
        Some(style) if *applied != Some(style) => {
            let cs = to_set_cursor_style(style.0, style.1);
            if crossterm::execute!(backend, cs).is_ok() {
                *applied = Some(style);
                restore::mark_written();
            }
        }
        None if applied.is_some()
            && crossterm::execute!(backend, SetCursorStyle::DefaultUserShape).is_ok() =>
        {
            *applied = None;
        }
        _ => {} // desired == applied, or both "unmanaged": zero writes at rest.
    }
}

/// Process-global "did we ever write a concrete DECSCUSR style?" latch. It must be reachable
/// from the `'static` panic hook (which has no `&Editor`), so it is a module static, not an
/// Editor field. (`restore_caret_if_written` is added in Task 5.)
pub mod restore {
    use crossterm::cursor::SetCursorStyle;
    use std::sync::atomic::{AtomicBool, Ordering};
    static EVER_WROTE: AtomicBool = AtomicBool::new(false);
    /// Called by the reconcile each time it successfully writes a concrete style.
    pub fn mark_written() { EVER_WROTE.store(true, Ordering::Relaxed); }
    /// True iff the reconcile ever emitted a DECSCUSR style this process.
    pub fn ever_wrote() -> bool { EVER_WROTE.load(Ordering::Relaxed) }

    /// Emit DefaultUserShape iff we ever wrote — used by the three managed term.rs restore sites.
    pub fn restore_caret_if_written<W: std::io::Write>(backend: &mut W) {
        if ever_wrote() { let _ = crossterm::execute!(backend, SetCursorStyle::DefaultUserShape); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::config::CaretShape;
    use crossterm::cursor::SetCursorStyle;

    fn ed_with(shape: CaretShape, blink: bool) -> Editor {
        let mut e = Editor::new_from_text("x\n", None, (40, 12));
        e.set_caret_shape(shape); e.set_caret_blink(blink); e
    }

    #[test]
    fn desired_composition_table() {
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Default, true)),  None);
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Default, false)), None);
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Block, true)),     Some((CaretShape::Block, true)));
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Block, false)),    Some((CaretShape::Block, false)));
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Beam, true)),      Some((CaretShape::Beam, true)));
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Beam, false)),     Some((CaretShape::Beam, false)));
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Underline, true)), Some((CaretShape::Underline, true)));
        assert_eq!(desired_caret_style(&ed_with(CaretShape::Underline, false)),Some((CaretShape::Underline, false)));
    }

    #[test]
    fn to_set_cursor_style_maps_all_concrete_combos() {
        assert!(matches!(to_set_cursor_style(CaretShape::Block, true),     SetCursorStyle::BlinkingBlock));
        assert!(matches!(to_set_cursor_style(CaretShape::Block, false),    SetCursorStyle::SteadyBlock));
        assert!(matches!(to_set_cursor_style(CaretShape::Beam, true),      SetCursorStyle::BlinkingBar));
        assert!(matches!(to_set_cursor_style(CaretShape::Beam, false),     SetCursorStyle::SteadyBar));
        assert!(matches!(to_set_cursor_style(CaretShape::Underline, true), SetCursorStyle::BlinkingUnderScore));
        assert!(matches!(to_set_cursor_style(CaretShape::Underline, false),SetCursorStyle::SteadyUnderScore));
    }

    #[test]
    fn default_shape_writes_nothing() {
        let e = ed_with(CaretShape::Default, true);
        let mut buf: Vec<u8> = Vec::new();
        let mut applied: Option<(CaretShape, bool)> = None;
        reconcile_cursor_style(&e, &mut buf, &mut applied);
        assert!(buf.is_empty(), "Default shape must emit no DECSCUSR");
        assert!(applied.is_none());
    }

    #[test]
    fn concrete_shape_writes_once_then_rests() {
        let e = ed_with(CaretShape::Beam, true);
        let mut buf: Vec<u8> = Vec::new();
        let mut applied: Option<(CaretShape, bool)> = None;
        reconcile_cursor_style(&e, &mut buf, &mut applied);
        assert!(!buf.is_empty(), "first reconcile writes the style");
        assert_eq!(applied, Some((CaretShape::Beam, true)));
        let n = buf.len();
        reconcile_cursor_style(&e, &mut buf, &mut applied); // idle-free guardrail
        assert_eq!(buf.len(), n, "second reconcile at rest writes nothing");
    }

    #[test]
    fn runtime_back_to_default_unmanages_once() {
        let mut e = ed_with(CaretShape::Beam, true);
        let mut buf: Vec<u8> = Vec::new();
        let mut applied: Option<(CaretShape, bool)> = None;
        reconcile_cursor_style(&e, &mut buf, &mut applied);
        e.set_caret_shape(CaretShape::Default);
        let n = buf.len();
        reconcile_cursor_style(&e, &mut buf, &mut applied);
        assert!(buf.len() > n, "→Default emits one DefaultUserShape");
        assert!(applied.is_none());
        let m = buf.len();
        reconcile_cursor_style(&e, &mut buf, &mut applied);
        assert_eq!(buf.len(), m, "then rests");
    }

    #[test]
    fn restore_latch_is_monotonic() {
        // restore::EVER_WROTE is a process-global static shared across all tests in the binary,
        // so assert only the monotonic (false→true) transition, never that it is false at start.
        restore::mark_written();
        assert!(restore::ever_wrote(), "mark_written latches ever_wrote true");
    }

    #[test]
    fn restore_caret_if_written_gated_by_latch() {
        // Process-global latch: assert only the two directions we can force deterministically.
        // never-written direction is only assertable if nothing else in the binary latched it;
        // guard on ever_wrote() so the test is order-independent.
        let mut buf: Vec<u8> = Vec::new();
        if !restore::ever_wrote() {
            restore::restore_caret_if_written(&mut buf);
            assert!(buf.is_empty(), "never wrote → restore emits nothing");
        }
        restore::mark_written();
        assert!(restore::ever_wrote());
        let mut buf2: Vec<u8> = Vec::new();
        restore::restore_caret_if_written(&mut buf2);
        assert!(!buf2.is_empty(), "after mark_written → restore emits DefaultUserShape");
    }
}
