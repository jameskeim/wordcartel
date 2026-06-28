use std::collections::BTreeMap;
use std::path::PathBuf;
use wordcartel_core::block_tree::{self, BlockTree};
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::history::{Clock, EditKind, History, Transaction};
use wordcartel_core::layout::{ColMap, VisualRow};
use wordcartel_core::register::Register;
use wordcartel_core::selection::Selection;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord, PartialOrd)]
pub struct BufferId(pub u64);

/// What to do once a pending save completes successfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostSaveAction { Quit, Open(std::path::PathBuf), New }

/// An in-flight "save, then act" request. Armed by `dispatch_save_then`;
/// consumed by `apply_result` when the save lands clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAfterSave {
    pub buffer_id: BufferId,
    pub version: u64,
    pub action: PostSaveAction,
    pub at_ms: u64,
}

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
    /// The document version last written to disk. `None` = never saved
    /// (new/scratch). `dirty()` is derived from this — no separate flag.
    pub saved_version: Option<u64>,
    /// Last-known on-disk fingerprint (captured at load, refreshed by the save
    /// merge). Used by dispatch_save to detect external modifications (§4.3).
    pub stored_fp: Option<crate::save::FileFingerprint>,
}

impl Document {
    /// Unsaved-work predicate (spec §4.3): clean iff the on-disk version
    /// equals the current version.
    pub fn dirty(&self) -> bool {
        Some(self.version) != self.saved_version
    }
    /// Record that version `v` is now on disk.
    pub fn mark_saved(&mut self, v: u64) {
        self.saved_version = Some(v);
    }
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
pub struct Buffer {
    pub id: BufferId,
    pub document: Document,
    pub view: View,
    // per-document transient state (relocated off Editor)
    pub desired_col: Option<usize>,
    pub pre_edit_rope: Option<ropey::Rope>,
    pub last_edit: Option<wordcartel_core::block_tree::Edit>,
    pub last_edit_at: Option<u64>,
    pub last_swap_at: Option<u64>,
    pub swap_in_flight: bool,
    pub pending_swap_body: Option<String>,
    pub pending_swap_path: Option<PathBuf>,
    // 5c: marks/ring/sel_history — wired in Tasks 5–10
    pub marks: std::collections::BTreeMap<char, usize>,
    pub jump_ring: Vec<usize>,
    pub ring_cursor: usize,
    pub sel_history: Vec<wordcartel_core::selection::Selection>,
    // 5f: per-buffer diagnostics store
    pub diagnostics: crate::diagnostics_run::DiagStore,
    // 5g: per-buffer fold state
    pub folds: crate::fold::FoldState,
}

impl Buffer {
    /// Pure construction of a Buffer with a caller-supplied id (id source = alloc_id).
    /// Mirrors the buffer push in `Editor::new_from_text` exactly.
    pub fn from_text(id: BufferId, text: &str, path: Option<PathBuf>, area: (u16, u16)) -> Buffer {
        let buffer = TextBuffer::from_str(text);
        let blocks = block_tree::full_parse_rope(&buffer.snapshot());
        let document = Document {
            buffer,
            selection: Selection::single(0),
            history: History::default(),
            blocks,
            version: 0,
            stored_fp: path.as_deref().and_then(crate::save::fingerprint),
            path,
            saved_version: Some(0),
        };
        let view = View {
            scroll: 0,
            scroll_row: 0,
            area,
            mode: RenderMode::LivePreview,
            line_layouts: BTreeMap::new(),
        };
        Buffer {
            id,
            document,
            view,
            desired_col: None,
            pre_edit_rope: None,
            last_edit: None,
            last_edit_at: None,
            last_swap_at: None,
            swap_in_flight: false,
            pending_swap_body: None,
            pending_swap_path: None,
            marks: Default::default(),
            jump_ring: Vec::new(),
            ring_cursor: 0,
            sel_history: Vec::new(),
            diagnostics: crate::diagnostics_run::DiagStore::new(),
            folds: crate::fold::FoldState::default(),
        }
    }

    /// Open `path` into a named Buffer, mirroring run()'s open branch:
    /// Ok → named clean; NotFound → named empty "new file" (`"\n"`); other errors propagate.
    pub fn from_file(id: BufferId, path: &std::path::Path, area: (u16, u16)) -> Result<Buffer, crate::file::OpenError> {
        match crate::file::open(path) {
            Ok(text) => Ok(Buffer::from_text(id, &text, Some(path.to_path_buf()), area)),
            Err(crate::file::OpenError::NotFound(_)) => Ok(Buffer::from_text(id, "\n", Some(path.to_path_buf()), area)),
            Err(e) => Err(e),
        }
    }

    /// Single mutation channel for THIS buffer's document (spec §10.1).
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        let cs = txn.changes.clone();                    // capture BEFORE commit consumes txn
        let old_rope = self.document.buffer.snapshot();
        let before = self.document.selection.clone();
        self.document.selection = self.document.history.commit_coalescing(txn, &mut self.document.buffer, before, clock, kind);
        self.document.version += 1;
        self.pre_edit_rope = Some(old_rope);
        self.last_edit = Some(edit);
        // 5c: marks & ring follow the text; the expand ladder resets on any edit.
        for v in self.marks.values_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        for v in self.jump_ring.iter_mut() {
            *v = wordcartel_core::change::map_pos(*v, &cs);
        }
        // 5g: fold anchors are heading STARTS — use Before bias so an insertion
        // at the heading's first byte does not push the anchor into the body.
        let remapped: std::collections::BTreeSet<usize> = self
            .folds
            .folded
            .iter()
            .map(|&b| wordcartel_core::change::map_pos_before(b, &cs))
            .collect();
        self.folds.folded = remapped;
        self.sel_history.clear();
        crate::recovery::record_snapshot(self.document.path.as_deref(), self.document.buffer.snapshot());
    }
    pub fn undo(&mut self) -> bool {
        match self.document.history.undo(&mut self.document.buffer) {
            Some(sel) => {
                self.document.selection = sel;
                self.document.version += 1;
                self.last_edit = None;
                self.pre_edit_rope = None;
                self.sel_history.clear();
                // 5g: drop fold anchors now past EOF; rebuild reconciles the rest.
                let len = self.document.buffer.len();
                self.folds.folded.retain(|&b| b <= len);
                true
            }
            None => false,
        }
    }
    pub fn redo(&mut self) -> bool {
        match self.document.history.redo(&mut self.document.buffer) {
            Some(sel) => {
                self.document.selection = sel;
                self.document.version += 1;
                self.last_edit = None;
                self.pre_edit_rope = None;
                self.sel_history.clear();
                // 5g: drop fold anchors now past EOF; rebuild reconciles the rest.
                let len = self.document.buffer.len();
                self.folds.folded.retain(|&b| b <= len);
                true
            }
            None => false,
        }
    }
}

/// Per-click tracking for double/triple-click detection.
#[derive(Default, Debug, Clone, Copy)]
pub struct ClickRecord {
    pub cell: (u16, u16),
    pub at_ms: u64,
    pub count: u8,
}

/// Transient mouse gesture state — reset on capture disable (reconcile clears drag).
#[derive(Default, Debug, Clone)]
pub struct MouseState {
    /// Byte-offset anchor for drag selection; None when no drag is active.
    pub anchor: Option<usize>,
    /// Last recorded click (cell, timestamp, repeat count).
    pub last_click: Option<ClickRecord>,
    /// True while a text-area drag is in progress.
    pub dragging: bool,
    /// True while the scrollbar thumb is being dragged.
    pub scrollbar_dragging: bool,
    /// Timestamp until which the scrollbar overlay remains visible after hover.
    pub scrollbar_until_ms: u64,
    /// Whether the scrollbar overlay is currently visible.
    pub scrollbar_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkPending { Set, Jump }

// MenuView is now Clone (#[derive(Clone, Debug)]); Editor intentionally remains !Clone.
#[derive(Debug)]
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub active: usize,
    pub next_buffer_id: u64,
    // global app state
    pub register: Register,
    pub status: String,
    pub quit: bool,
    pub prompt: Option<crate::prompt::Prompt>,
    /// Armed by `dispatch_save_then`; consumed by `apply_result` when the save lands.
    pub pending_after_save: Option<PendingAfterSave>,
    /// Carry the post-save action across an unnamed buffer's Save-As flow (Task 3).
    pub pending_save_as: Option<PostSaveAction>,
    pub filter_in_flight: Option<crate::filter::CancelFlag>,
    pub transform_in_flight: bool,
    pub minibuffer: Option<crate::minibuffer::Minibuffer>,
    pub pending_export: Option<crate::export::PendingExport>,
    pub pending_mark: Option<MarkPending>,
    pub clipboard_sync_request: Option<String>,
    pub clipboard_get_pending: Option<crate::clipboard::PasteIntent>,
    pub clipboard_notice_shown: bool,
    pub pending_keys: Vec<crate::keymap::KeyChord>,
    pub keymap: crate::keymap::KeyTrie,
    pub palette: Option<crate::palette::Palette>,
    pub menu: Option<crate::menu::MenuView>,
    /// Whether mouse capture is currently requested (toggled by `toggle_mouse_capture`).
    /// Seeded from config at startup; defaults to `true` in test/scratch contexts.
    pub mouse_capture: bool,
    /// Transient mouse gesture state; cleared by `reconcile_mouse_capture` on disable.
    pub mouse: MouseState,
    /// View/focus/writing-experience flags. Seeded from config; toggled by the 5 toggle_ commands.
    pub view_opts: crate::config::ViewConfig,
    /// Search/replace overlay state. XOR with prompt/minibuffer/palette/menu.
    pub search: Option<crate::search_overlay::SearchState>,
    /// Diagnostics configuration. Seeded from config at startup.
    pub diag_cfg: crate::config::DiagnosticsConfig,
    /// Personal dictionary loaded from `diag_cfg.dictionary` at startup.
    pub dictionary: std::collections::HashSet<String>,
    /// Session-level words to ignore (added via ignore-word command).
    pub session_ignores: std::collections::HashSet<String>,
    /// Quick-fix overlay state. XOR with prompt/minibuffer/palette/menu/search.
    pub diag: Option<crate::diag_overlay::DiagOverlay>,
    /// Outline picker overlay state. XOR with prompt/minibuffer/palette/menu/search/diag.
    pub outline: Option<crate::outline_overlay::OutlineOverlay>,
    /// Theme picker overlay state. XOR with all other overlays.
    pub theme_picker: Option<crate::theme_picker::ThemePicker>,
    /// Active theme + terminal color depth. Seeded at startup (real depth detection: plan ③).
    pub theme: wordcartel_core::theme::Theme,
    pub depth: wordcartel_core::theme::Depth,
    /// Heading-level glyph toggle from config (seeded at startup; used by runtime picker — Task 7).
    pub heading_glyph_cfg: Option<bool>,
    /// Whether session-resume restore is enabled (seeded from `cfg.state.resume` in run()).
    /// Gates `open_into_current`'s resume restore. Defaults false until run() seeds it.
    pub resume_enabled: bool,
}

impl Editor {
    pub fn new_from_text(text: &str, path: Option<PathBuf>, area: (u16, u16)) -> Editor {
        // Build the workspace, then allocate the buffer's id through the single
        // id source (alloc_id) so there is no second id-assignment path (Codex review).
        let (keymap, _) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig::default(),
            &crate::registry::Registry::builtins(),
        );
        let mut e = Editor {
            buffers: Vec::new(), active: 0, next_buffer_id: 0,
            register: Register::default(), status: String::new(), quit: false,
            prompt: None, pending_after_save: None, pending_save_as: None,
            filter_in_flight: None, transform_in_flight: false, minibuffer: None, pending_export: None,
            pending_mark: None,
            clipboard_sync_request: None, clipboard_get_pending: None, clipboard_notice_shown: false,
            pending_keys: Vec::new(),
            keymap,
            palette: None,
            menu: None,
            mouse_capture: true,
            mouse: MouseState::default(),
            view_opts: crate::config::ViewConfig::default(),
            search: None,
            diag_cfg: crate::config::DiagnosticsConfig::default(),
            dictionary: std::collections::HashSet::new(),
            session_ignores: std::collections::HashSet::new(),
            diag: None,
            outline: None,
            theme_picker: None,
            theme: wordcartel_core::theme::default(),
            depth: wordcartel_core::theme::Depth::Truecolor,
            heading_glyph_cfg: None,
            resume_enabled: false,
        };
        let id = e.alloc_id(); // -> BufferId(0); next_buffer_id becomes 1
        e.buffers.push(Buffer::from_text(id, text, path, area));
        e
    }

    #[inline] pub fn active(&self) -> &Buffer {
        debug_assert!(!self.buffers.is_empty() && self.active < self.buffers.len(), "len>=1 + active in range");
        &self.buffers[self.active]
    }
    #[inline] pub fn active_mut(&mut self) -> &mut Buffer {
        debug_assert!(!self.buffers.is_empty() && self.active < self.buffers.len(), "len>=1 + active in range");
        let i = self.active; &mut self.buffers[i]
    }
    pub fn by_id(&self, id: BufferId) -> Option<&Buffer> { self.buffers.iter().find(|b| b.id == id) }
    pub fn by_id_mut(&mut self, id: BufferId) -> Option<&mut Buffer> { self.buffers.iter_mut().find(|b| b.id == id) }
    /// Allocate a fresh, never-reused BufferId.
    pub fn alloc_id(&mut self) -> BufferId { let id = BufferId(self.next_buffer_id); self.next_buffer_id += 1; id }

    /// Replace the active buffer with a fresh unnamed scratch buffer.
    /// Caller must run `derive::rebuild` + `nav::ensure_visible` afterwards.
    pub fn replace_active_with_scratch(&mut self) {
        let id = self.alloc_id();
        let area = self.active().view.area;
        let a = self.active;
        self.buffers[a] = Buffer::from_text(id, "\n", None, area);
    }

    /// Open the minibuffer with the given prompt string.
    ///
    /// Invariant: prompt XOR minibuffer — only one may be active at a time.
    /// Clears `prompt`, `palette`, `menu`, and `pending_keys` before opening.
    pub fn open_minibuffer(&mut self, prompt: &str, kind: crate::minibuffer::MinibufferKind) {
        debug_assert!(self.prompt.is_none(), "prompt xor minibuffer: cannot open minibuffer while a modal prompt is active");
        self.prompt = None;
        self.pending_keys.clear();
        self.pending_mark = None;
        self.palette = None;
        self.menu = None;
        self.search = None;
        self.diag = None;
        self.outline = None;
        self.theme_picker = None;
        self.minibuffer = Some(crate::minibuffer::Minibuffer {
            prompt: prompt.into(),
            text: String::new(),
            cursor: 0,
            kind,
        });
    }

    /// Open a modal prompt, enforcing single-overlay XOR invariant.
    ///
    /// Clears `palette`, `minibuffer`, `menu`, and `pending_keys` before setting the prompt.
    /// At most one of {prompt, minibuffer, palette} is ever active at once.
    pub fn open_prompt(&mut self, p: crate::prompt::Prompt) {
        self.palette = None;
        self.minibuffer = None;
        self.menu = None;
        self.pending_keys.clear();
        self.pending_mark = None;
        self.search = None;
        self.diag = None;
        self.outline = None;
        self.theme_picker = None;
        self.prompt = Some(p);
    }

    /// Open the command palette, enforcing single-overlay XOR invariant.
    ///
    /// Clears `prompt`, `minibuffer`, `menu`, and `pending_keys` before opening.
    /// At most one of {prompt, minibuffer, palette, menu} is ever active at once.
    pub fn open_palette(&mut self) {
        self.prompt = None;
        self.minibuffer = None;
        self.menu = None;
        self.pending_keys.clear();
        self.pending_mark = None;
        self.search = None;
        self.diag = None;
        self.outline = None;
        self.theme_picker = None;
        self.palette = Some(crate::palette::Palette::default());
    }

    /// Open the search overlay, enforcing single-overlay XOR invariant.
    ///
    /// Clears `prompt`, `minibuffer`, `palette`, `menu`, and `pending_keys` before opening.
    /// At most one of {prompt, minibuffer, palette, menu, search} is ever active at once.
    pub fn open_search(&mut self, phase: crate::search_overlay::Phase, origin: usize) {
        self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.diag = None;
        self.outline = None;
        self.theme_picker = None;
        let bid = self.active().id;
        self.search = Some(crate::search_overlay::SearchState::open(phase, origin, bid));
    }

    /// Open the quick-fix overlay for a given diagnostic, enforcing single-overlay XOR invariant.
    ///
    /// Clears prompt/minibuffer/palette/menu/search + pending_keys + pending_mark.
    /// Records `opened_version` so `diag_apply_selected` can refuse a stale apply
    /// if the buffer is mutated while the overlay is open (Fix A4).
    pub fn open_diag(&mut self, d: wordcartel_core::diagnostics::Diagnostic) {
        self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None; self.search = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.outline = None;
        self.theme_picker = None;
        let bid = self.active().id;
        let ver = self.active().document.version;
        self.diag = Some(crate::diag_overlay::DiagOverlay::new(d, bid, ver));
    }

    /// Open the outline picker, enforcing single-overlay XOR invariant.
    pub fn open_outline(&mut self) {
        self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None;
        self.search = None; self.diag = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.theme_picker = None;
        let bid = self.active().id;
        let ver = self.active().document.version;
        let blocks = self.active().document.blocks.clone();
        let rope = self.active().document.buffer.snapshot();
        self.outline = Some(crate::outline_overlay::OutlineOverlay::open(bid, ver, &blocks, &rope));
    }

    /// Open the theme picker, enforcing the single-overlay XOR invariant.
    pub fn open_theme_picker(&mut self) {
        self.prompt = None; self.minibuffer = None; self.menu = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.search = None; self.diag = None; self.outline = None; self.palette = None;
        self.theme_picker = Some(crate::theme_picker::ThemePicker {
            query: String::new(), selected: 0, rows: Vec::new(),
            original: self.theme.clone(),
        });
        if let Some(tp) = self.theme_picker.as_mut() { crate::theme_picker::rebuild_rows(tp); }
    }

    /// Apply a theme: swap, re-derive the heading-glyph flag (cue mode forces ON;
    /// else the CONFIG override `heading_glyph_cfg` wins, else the theme's own flag —
    /// Codex I4, so a picker switch doesn't drop a configured override), relayout
    /// (heading_level_glyph is a layout input — §3.6/§3.7), keep caret visible.
    pub fn apply_theme(&mut self, mut theme: wordcartel_core::theme::Theme) {
        let cue = self.depth == wordcartel_core::theme::Depth::None || theme.monochrome;
        theme.heading_level_glyph = if cue { true }
            else { self.heading_glyph_cfg.unwrap_or(theme.heading_level_glyph) };
        self.theme = theme;
        crate::derive::rebuild(self);
        crate::nav::ensure_visible(self);
    }

    // Thin delegators — external callers unchanged.
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        self.active_mut().apply(txn, edit, kind, clock);
    }
    pub fn undo(&mut self) -> bool { self.active_mut().undo() }
    pub fn redo(&mut self) -> bool { self.active_mut().redo() }
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
    fn buffer_from_file_ok_named_clean() {
        let p = std::env::temp_dir().join(format!("wc-fromfile-{}.md", std::process::id()));
        std::fs::write(&p, "hello\nworld\n").unwrap();
        let mut e = Editor::new_from_text("\n", None, (40, 10)); // host editor for ids
        let id = e.alloc_id();
        let b = Buffer::from_file(id, &p, (40, 10)).expect("ok");
        assert_eq!(b.id, id);
        assert_eq!(b.document.buffer.to_string(), "hello\nworld\n");
        assert_eq!(b.document.path.as_deref(), Some(p.as_path()));
        assert!(!b.document.dirty(), "freshly opened file is clean");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn buffer_from_file_not_found_is_named_new_file() {
        let p = std::env::temp_dir().join(format!("wc-missing-{}.md", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("\n", None, (40, 10));
        let id = e.alloc_id();
        let b = Buffer::from_file(id, &p, (40, 10)).expect("NotFound → named empty buffer, not Err");
        assert_eq!(b.document.path.as_deref(), Some(p.as_path()));
        assert_eq!(b.document.buffer.to_string(), "\n");
    }

    #[test]
    fn buffer_from_file_binary_is_err() {
        let p = std::env::temp_dir().join(format!("wc-bin-{}.bin", std::process::id()));
        std::fs::write(&p, [0u8, 159, 146, 150]).unwrap(); // invalid UTF-8 / NUL
        let mut e = Editor::new_from_text("\n", None, (40, 10));
        let id = e.alloc_id();
        assert!(matches!(Buffer::from_file(id, &p, (40, 10)), Err(crate::file::OpenError::Binary(_))));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn new_from_text_still_builds_one_buffer_id_zero() {
        let e = Editor::new_from_text("abc\n", None, (40, 10));
        assert_eq!(e.active().id, BufferId(0));
        assert_eq!(e.active().document.buffer.to_string(), "abc\n");
        assert!(!e.resume_enabled, "default false until run() seeds it"); // field exists, defaults false
    }

    #[test]
    fn new_editor_holds_text_and_clean_state() {
        let e = Editor::new_from_text("# Hi\n\nbody\n", None, (80, 24));
        assert_eq!(e.active().document.buffer.to_string(), "# Hi\n\nbody\n");
        assert_eq!(e.active().document.selection.primary().from(), 0);
        assert_eq!(e.active().document.version, 0);
        assert!(!e.active().document.dirty());
        assert!(!e.active().document.blocks.top_level().is_empty());
    }

    #[test]
    fn apply_insert_mutates_text_selection_version() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        // insert "X" at offset 1 -> "aXb\n"
        let cs = ChangeSet::insert(1, "X", e.active().document.buffer.len());
        let txn = Transaction::new(cs).with_selection(Selection::single(2));
        e.apply(txn, Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "aXb\n");
        assert_eq!(e.active().document.selection.primary().head, 2);
        assert_eq!(e.active().document.version, 1);
        assert!(e.active().document.dirty());
        assert!(e.active().pre_edit_rope.is_some());
        // Edit has no PartialEq — compare fields:
        assert_eq!(e.active().last_edit.as_ref().map(|x| (x.range.clone(), x.new_len)), Some((1..1, 1)));
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        let cs = ChangeSet::insert(1, "X", e.active().document.buffer.len());
        e.apply(Transaction::new(cs).with_selection(Selection::single(2)), Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        let changed = e.undo();
        assert!(changed, "undo of a real edit must report change");
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
        assert!(e.active().last_edit.is_none()); // undo forces a full reparse in derive
        let changed = e.redo();
        assert!(changed, "redo of a real edit must report change");
        assert_eq!(e.active().document.buffer.to_string(), "aXb\n");
    }

    #[test]
    fn undo_on_empty_history_is_true_noop() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let v0 = e.active().document.version;
        e.active_mut().desired_col = Some(3);
        let changed = e.undo();
        assert!(!changed, "undo with empty history must report no change");
        assert_eq!(e.active().document.version, v0, "version must not move on a no-op undo");
        assert!(!e.active().document.dirty(), "a no-op undo must not dirty the buffer");
        assert_eq!(e.active().desired_col, Some(3), "a no-op undo must not reset desired_col");
    }

    #[test]
    fn redo_on_empty_history_is_true_noop() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let v0 = e.active().document.version;
        e.active_mut().desired_col = Some(3);
        let changed = e.redo();
        assert!(!changed, "redo with empty history must report no change");
        assert_eq!(e.active().document.version, v0, "version must not move on a no-op redo");
        assert!(!e.active().document.dirty(), "a no-op redo must not dirty the buffer");
        assert_eq!(e.active().desired_col, Some(3), "a no-op redo must not reset desired_col");
    }

    #[test]
    fn dirty_is_a_function_of_versions() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        assert!(!e.active().document.dirty(), "fresh buffer (saved_version=Some(0)) is clean");
        let clk = TestClock(std::cell::Cell::new(0));
        let cs = wordcartel_core::change::ChangeSet::insert(1, "X", e.active().document.buffer.len());
        e.apply(
            Transaction::new(cs).with_selection(Selection::single(2)),
            wordcartel_core::block_tree::Edit { range: 1..1, new_len: 1 },
            EditKind::Type, &clk,
        );
        assert!(e.active().document.dirty(), "after an edit, version != saved_version → dirty");
        let v = e.active().document.version;
        e.active_mut().document.mark_saved(v);
        assert!(!e.active().document.dirty(), "after mark_saved at current version → clean");
    }

    #[test]
    fn single_buffer_invariants_and_accessors() {
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        assert_eq!(e.buffers.len(), 1);
        assert_eq!(e.active, 0);
        assert_eq!(e.active().id, BufferId(0));
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
        // by_id resolves the active buffer; a bogus id is None.
        let id = e.active().id;
        assert!(e.by_id(id).is_some());
        assert!(e.by_id_mut(BufferId(999)).is_none());
    }

    #[test]
    fn alloc_id_is_monotonic_and_never_reuses() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // id 0 is taken by the initial buffer; next allocations are 1, 2, 3...
        let a = e.alloc_id();
        let b = e.alloc_id();
        assert_eq!(a, BufferId(1));
        assert_eq!(b, BufferId(2));
        assert_ne!(a, e.active().id); // never collides with the existing buffer's id
    }

    #[test]
    fn marks_follow_edits_above_them() {
        use wordcartel_core::change::ChangeSet;
        use wordcartel_core::history::Transaction;
        let clk = TestClock(std::cell::Cell::new(0));
        let mut e = Editor::new_from_text("abcdef", None, (80, 24));
        e.active_mut().marks.insert('a', 4); // mark at 'e'
        // insert "XY" at offset 1 → mark should shift 4 → 6
        let cs = ChangeSet::insert(1, "XY", e.active().document.buffer.len());
        e.apply(Transaction::new(cs), Edit { range: 1..1, new_len: 2 }, EditKind::Type, &clk);
        assert_eq!(e.active().marks.get(&'a'), Some(&6));
    }

    #[test]
    fn apply_clears_sel_history() {
        use wordcartel_core::change::ChangeSet;
        use wordcartel_core::history::Transaction;
        use wordcartel_core::selection::Selection;
        let clk = TestClock(std::cell::Cell::new(0));
        let mut e = Editor::new_from_text("abcdef", None, (80, 24));
        e.active_mut().sel_history.push(Selection::single(0));
        let cs = ChangeSet::insert(1, "X", e.active().document.buffer.len());
        e.apply(Transaction::new(cs), Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        assert!(e.active().sel_history.is_empty(), "edit must reset the expand ladder");
    }

    #[test]
    fn undo_clears_expand_ladder() {
        use wordcartel_core::change::ChangeSet;
        use wordcartel_core::history::Transaction;
        use wordcartel_core::selection::Selection;
        let clk = TestClock(std::cell::Cell::new(0));
        let mut e = Editor::new_from_text("abcdef", None, (80, 24));
        // Simulate an expand by pushing a selection onto sel_history.
        e.active_mut().sel_history.push(Selection::single(0));
        // Make an edit so there is history to undo.
        let cs = ChangeSet::insert(1, "X", e.active().document.buffer.len());
        e.apply(Transaction::new(cs), Edit { range: 1..1, new_len: 1 }, EditKind::Type, &clk);
        // apply already clears sel_history; push again to simulate a post-edit expand.
        e.active_mut().sel_history.push(Selection::single(3));
        // undo must clear the stale ladder.
        let changed = e.undo();
        assert!(changed, "undo of a real edit must report change");
        assert!(e.active().sel_history.is_empty(), "undo must clear the expand ladder (sel_history)");
    }

    #[test]
    fn open_minibuffer_clears_pending_keys() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_keys.push(crate::keymap::KeyChord {
            code: crossterm::event::KeyCode::Char('k'),
            mods: crossterm::event::KeyModifiers::CONTROL,
        });
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        assert!(e.pending_keys.is_empty(), "opening the minibuffer must clear a pending key sequence");
    }

    #[test]
    fn open_search_clears_siblings_and_open_others_clear_search() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        e.open_search(crate::search_overlay::Phase::Find, 0);
        assert!(e.search.is_some() && e.minibuffer.is_none() && e.prompt.is_none()
                && e.palette.is_none() && e.menu.is_none());
        e.open_palette();
        assert!(e.search.is_none(), "open_palette must clear search");
    }

    // 5g: fold anchor remap helpers — mirrored from the marks_follow_edits_above_them test.
    fn apply_insert(buf: &mut Buffer, at: usize, text: &str, clk: &TestClock) {
        let len_before = buf.document.buffer.len();
        let cs = ChangeSet::insert(at, text, len_before);
        let txn = Transaction::new(cs);
        let edit = wordcartel_core::block_tree::Edit { range: at..at, new_len: text.len() };
        buf.apply(txn, edit, EditKind::Type, clk);
    }
    fn apply_delete(buf: &mut Buffer, range: std::ops::Range<usize>, clk: &TestClock) {
        let len_before = buf.document.buffer.len();
        let cs = ChangeSet::delete(range.clone(), len_before);
        let txn = Transaction::new(cs);
        let edit = wordcartel_core::block_tree::Edit { range: range.clone(), new_len: 0 };
        buf.apply(txn, edit, EditKind::Other, clk);
    }

    #[test]
    fn fold_anchor_survives_insertion_above_it() {
        let mut ed = Editor::new_from_text("# A\n\nbody\n\n## B\n\nb2\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        let buf = ed.active_mut();
        let b_off = "# A\n\nbody\n\n".len(); // start of "## B"
        buf.folds.toggle(b_off);
        apply_insert(buf, 0, "X\n", &clk); // insert above the fold
        // anchor shifts by 2 and still lands on "## B".
        assert!(buf.folds.folded.contains(&(b_off + 2)));
    }

    #[test]
    fn fold_anchor_at_heading_start_uses_before_bias() {
        let mut ed = Editor::new_from_text("## H\nbody\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        let buf = ed.active_mut();
        buf.folds.toggle(0); // fold the heading at byte 0
        apply_insert(buf, 0, "Z", &clk);
        // Before-biased: the anchor stays at 0 (text is now "Z## H"), it is NOT
        // pushed to 1. (Whether 0 is still a heading start is decided later by
        // reconcile in rebuild — Task 5; here we only assert the remap bias.)
        assert!(buf.folds.folded.contains(&0));
        assert!(!buf.folds.folded.contains(&1));
    }

    #[test]
    fn undo_does_not_panic_and_clamps_fold_anchors() {
        let mut ed = Editor::new_from_text("## H\nbody\n", None, (80, 24));
        let clk = TestClock(std::cell::Cell::new(0));
        let buf = ed.active_mut();
        buf.folds.toggle(0);
        apply_delete(buf, 0.."## H\n".len(), &clk); // delete the heading line
        buf.undo();
        // Step-4 clamp guarantees no anchor points past EOF (no panic on later slice).
        // The definitive "deleted-heading fold is dropped" check lives in Task 5
        // (after rebuild's reconcile) — see `undo_then_rebuild_drops_dead_fold`.
        let len = buf.document.buffer.len();
        assert!(buf.folds.folded.iter().all(|&b| b <= len));
    }

    #[test]
    fn editor_seeds_default_theme_truecolor() {
        let ed = Editor::new_from_text("x", None, (80, 24));
        assert_eq!(ed.theme.name, "default");
        assert_eq!(ed.depth, wordcartel_core::theme::Depth::Truecolor);
    }
}
