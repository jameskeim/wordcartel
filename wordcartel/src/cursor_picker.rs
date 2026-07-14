//! Caret-shape picker overlay state (C1 T6 stub — minimal shape only so the
//! `Editor::has_active_input_overlay` census is exhaustive and testable; T7 fills in
//! the picker's row-building, navigation, and preview/commit/cancel logic).

use crate::config::CaretShape;

/// Live caret-picker overlay state. `selected` indexes the shape row under the cursor;
/// `original_shape`/`original_blink` snapshot the caret settings in effect when the
/// picker opened, so Esc/cancel can restore them after a live preview (T7).
#[derive(Debug)]
pub struct CursorPicker {
    pub selected: usize,
    pub original_shape: CaretShape,
    pub original_blink: bool,
}
