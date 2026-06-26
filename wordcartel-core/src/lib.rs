//! Wordcartel edit kernel: pure, headless buffer + undo + selection.
//! Canonical position = byte offset (usize) into the buffer.
#![forbid(unsafe_code)]

pub mod block_tree;
pub mod buffer;
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

/// A byte offset into a buffer's text. The kernel's canonical position type.
pub type BytePos = usize;
