use std::collections::BTreeMap;
use std::path::PathBuf;
use wordcartel_core::block_tree::{self, BlockTree};
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::history::{Clock, EditKind, History, Transaction};
use wordcartel_core::layout::{ColMap, VisualRow};
use wordcartel_core::register::Register;
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
    pub scroll_row: usize, // visual rows to skip within the first visible logical line
    pub area: (u16, u16), // (width, height) cells of the editing area
    pub mode: RenderMode,
    /// Per-visible-logical-line layout cache (Task 3).
    /// Key = logical line index; value = (visual rows, source↔visual ColMap).
    pub line_layouts: BTreeMap<usize, (Vec<VisualRow>, ColMap)>,
}

#[derive(Debug, Clone)]
pub struct Editor {
    pub document: Document,
    pub view: View,
    pub status: String, // ephemeral feedback line
    pub quit: bool,
    pub desired_col: Option<usize>, // preserved visual column for vertical motion; None until the first vertical move
    pub register: Register, // in-process clipboard register (Task 9)
    pub pending_quit: bool, // true after first Quit with dirty buffer (double-Quit to confirm)
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
                scroll_row: 0,
                area,
                mode: RenderMode::LivePreview,
                line_layouts: BTreeMap::new(),
            },
            status: String::new(),
            quit: false,
            desired_col: None,
            register: Register::default(),
            pending_quit: false,
            pre_edit_rope: None,
            last_edit: None,
        }
    }

    /// The single mutation channel (spec §10.1). Caller passes the `Edit`
    /// describing the same `(range, replacement)` used to build the `ChangeSet`.
    pub fn apply(
        &mut self,
        txn: Transaction,
        edit: wordcartel_core::block_tree::Edit,
        kind: EditKind,
        clock: &dyn Clock,
    ) {
        let old_rope = self.document.buffer.snapshot(); // O(1) ropey clone
        let before = self.document.selection.clone();
        self.document.selection = self.document.history.commit_coalescing(
            txn,
            &mut self.document.buffer,
            before,
            clock,
            kind,
        );
        self.document.version += 1;
        self.document.dirty = true;
        self.pre_edit_rope = Some(old_rope);
        self.last_edit = Some(edit);
    }

    /// Undo the last revision. Sets `last_edit`/`pre_edit_rope` to `None` so
    /// Task 3's derive falls back to a full reparse.
    pub fn undo(&mut self) {
        if let Some(sel) = self.document.history.undo(&mut self.document.buffer) {
            self.document.selection = sel;
        }
        self.document.version += 1;
        self.document.dirty = true;
        self.last_edit = None;
        self.pre_edit_rope = None;
    }

    /// Redo the next revision. Sets `last_edit`/`pre_edit_rope` to `None` so
    /// Task 3's derive falls back to a full reparse.
    pub fn redo(&mut self) {
        if let Some(sel) = self.document.history.redo(&mut self.document.buffer) {
            self.document.selection = sel;
        }
        self.document.version += 1;
        self.document.dirty = true;
        self.last_edit = None;
        self.pre_edit_rope = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::block_tree::Edit;
    use wordcartel_core::change::ChangeSet;

    struct TestClock(std::cell::Cell<u64>);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0.get() }
    }

    #[test]
    fn new_editor_holds_text_and_clean_state() {
        let e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
        assert_eq!(e.document.buffer.to_string(), "# Hi\n\nbody\n");
        assert_eq!(e.document.selection.primary().from(), 0);
        assert_eq!(e.document.version, 0);
        assert!(!e.document.dirty);
        assert!(!e.document.blocks.top_level().is_empty());
    }

    #[test]
    fn apply_insert_mutates_text_selection_version() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        // insert "X" at offset 1 -> "aXb\n"
        let cs = ChangeSet::insert(1, "X", e.document.buffer.len());
        let txn = Transaction::new(cs).with_selection(Selection::single(2));
        e.apply(txn, Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        assert_eq!(e.document.buffer.to_string(), "aXb\n");
        assert_eq!(e.document.selection.primary().head, 2);
        assert_eq!(e.document.version, 1);
        assert!(e.document.dirty);
        assert!(e.pre_edit_rope.is_some());
        // Edit has no PartialEq — compare fields:
        assert_eq!(e.last_edit.as_ref().map(|x| (x.range.clone(), x.new_len)), Some((1..1, 1)));
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        let cs = ChangeSet::insert(1, "X", e.document.buffer.len());
        e.apply(Transaction::new(cs).with_selection(Selection::single(2)), Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        e.undo();
        assert_eq!(e.document.buffer.to_string(), "ab\n");
        assert!(e.last_edit.is_none()); // undo forces a full reparse in derive
        e.redo();
        assert_eq!(e.document.buffer.to_string(), "aXb\n");
    }
}
