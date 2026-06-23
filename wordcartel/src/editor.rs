use std::path::PathBuf;
use wordcartel_core::block_tree::{self, BlockTree};
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::history::History;
use wordcartel_core::selection::Selection;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderMode {
    LivePreview,
    SourceHighlighted,
    SourcePlain,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub buffer: TextBuffer,
    pub selection: Selection,
    pub history: History,
    pub blocks: BlockTree, // derived cache (Task 3 maintains)
    pub version: u64,
    pub path: Option<PathBuf>,
    pub dirty: bool, // unsaved changes
}

#[derive(Debug, Clone)]
pub struct View {
    pub scroll: usize,    // first visible LOGICAL line index
    pub area: (u16, u16), // (width, height) cells of the editing area
    pub mode: RenderMode,
    // line_layouts cache added in Task 3
}

#[derive(Debug, Clone)]
pub struct Editor {
    pub document: Document,
    pub view: View,
    pub status: String, // ephemeral feedback line
    pub quit: bool,
    pub desired_col: Option<usize>, // preserved visual column for vertical motion; None until the first vertical move
    // Threaded from `apply` → `derive` for the O(region) incremental reparse (Effort 3c):
    pub pre_edit_rope: Option<ropey::Rope>, // O(1) snapshot taken BEFORE the edit
    pub last_edit: Option<wordcartel_core::block_tree::Edit>, // the block_tree edit (range, new_len); None ⇒ full reparse
}

impl Editor {
    pub fn new_from_text(text: &str, path: Option<PathBuf>, area: (u16, u16)) -> Editor {
        let buffer = TextBuffer::from_str(text);
        let selection = Selection::single(0);
        let history = History::default();
        let blocks = block_tree::full_parse_rope(&buffer.snapshot());
        Editor {
            document: Document {
                buffer,
                selection,
                history,
                blocks,
                version: 0,
                path,
                dirty: false,
            },
            view: View {
                scroll: 0,
                area,
                mode: RenderMode::LivePreview,
            },
            status: String::new(),
            quit: false,
            desired_col: None,
            pre_edit_rope: None,
            last_edit: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_editor_holds_text_and_clean_state() {
        let e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
        assert_eq!(e.document.buffer.to_string(), "# Hi\n\nbody\n");
        assert_eq!(e.document.selection.primary().from(), 0);
        assert_eq!(e.document.version, 0);
        assert!(!e.document.dirty);
        assert!(!e.document.blocks.top_level().is_empty());
    }
}
