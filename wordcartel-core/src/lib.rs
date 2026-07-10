//! Wordcartel edit kernel: pure, headless buffer + undo + selection.
//! Canonical position = byte offset (usize) into the buffer.
#![forbid(unsafe_code)]
// H7: cast-soundness gate for the pure kernel — the offset/region math concentrates here
// and unsafe is already forbidden, so hold the higher bar. Benign widenings carry an
// item-local #[allow] + one-line reason (the reason-carrying-allow idiom used for
// too_many_lines / print_stdout). The shell is deliberately NOT gated (~70 terminal-
// coordinate casts stay unannotated).
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]

pub mod block_tree;
pub mod buffer;

#[cfg(any(test, fuzzing))]
pub mod test_support;

#[cfg(test)]
mod proptest_strategies;
pub mod change;
pub mod count;
pub mod history;
pub mod register;
pub mod selection;

pub mod diagnostics;
pub mod layout;
pub mod md_parse;
pub mod outline;
pub mod search;
pub mod style;
pub mod textobj;
pub mod theme;

/// A byte offset into a buffer's text. The kernel's canonical position type.
pub type BytePos = usize;
