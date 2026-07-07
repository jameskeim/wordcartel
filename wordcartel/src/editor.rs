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
/// `Quit` is the single-buffer save-then-quit; `ContinueQuitDrain` advances the
/// multi-buffer quit state machine (Effort 6) after each buffer's save lands.
/// `CloseBuffer { id }` closes the target buffer (C4 close-confirm path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostSaveAction { Quit, ContinueQuitDrain, CloseBuffer { id: BufferId } }

/// Effort 6 multi-buffer quit: how the drain disposes of each dirty buffer.
/// `Copy` so `let mode = drain.mode;` copies out without holding a borrow on
/// `quit_drain` across `is_dirty`/`switch_to`/`dispatch_save_then` (Codex I-new-1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuitMode { SaveAll, ReviewEach }

/// Effort 6 multi-buffer quit drain: the FIFO of dirty buffers still to dispose
/// of, plus the chosen disposition. Driven one buffer per step by `drive_quit_drain`.
#[derive(Clone, Debug)]
pub struct QuitDrain {
    pub queue: std::collections::VecDeque<BufferId>,
    pub mode: QuitMode,
}

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
    blocks: BlockTree, // derived cache — write only via set_blocks
    pub version: u64,
    /// Monotonic id of `blocks`: bumped on EVERY `set_blocks` call (parse phase +
    /// reconcile merge). Identifies the current tree across the reconcile-merge
    /// boundary (where `version` is unchanged). Keys the FoldView + layout caches.
    blocks_generation: u64,
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

    /// Read the derived block tree (private field — writes go through `set_blocks`).
    #[inline]
    pub fn blocks(&self) -> &wordcartel_core::block_tree::BlockTree { &self.blocks }
    /// The block-tree identity token; changes on every `set_blocks`. Keys the FoldView + layout caches.
    #[inline]
    pub fn blocks_generation(&self) -> u64 { self.blocks_generation }
    /// The ONLY way to write `blocks` — bumps `blocks_generation` so no writer can bypass the
    /// cache-identity token (valid-by-construction). Unconditional bump on each call; a caller
    /// wanting write-on-change guards the CALL (see the reconcile merge), not the bump.
    pub fn set_blocks(&mut self, blocks: wordcartel_core::block_tree::BlockTree) {
        self.blocks = blocks;
        self.blocks_generation = self.blocks_generation.wrapping_add(1);
    }
    /// Take the derived block tree out by value, leaving a valid empty placeholder.
    /// TRANSIENT CONTRACT: the caller MUST write a real tree back (`set_blocks`, or the
    /// reconcile fallback's `apply_parse_result`) on EVERY path — until then the document
    /// holds an empty tree behind a stale `blocks_generation`. Does NOT bump the generation
    /// (only `set_blocks` does). Used by the incremental parse path to hand the parser
    /// ownership of the old tree (F4 — no clone).
    pub(crate) fn take_blocks(&mut self) -> wordcartel_core::block_tree::BlockTree {
        std::mem::replace(
            &mut self.blocks,
            wordcartel_core::block_tree::empty_tree(self.buffer.len()),
        )
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

/// 9a: a persistent marked block — a half-open `[start, end)` byte range that
/// follows the text across edits (see `Buffer::apply`). `hidden` drives folding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkedBlock { pub start: usize, pub end: usize, pub hidden: bool }

/// Memoized fold view keyed by `(blocks_generation, folds.epoch)`. Aliased to
/// preempt `clippy::type_complexity` under the deny gate (no `#[allow]`).
type FoldViewCache = std::cell::RefCell<Option<(u64, u64, std::rc::Rc<crate::fold::FoldView>)>>;

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
    /// per-buffer block-tree reconcile store (incremental-soundness effort)
    pub reconcile: crate::reconcile::ReconcileStore,
    // 5g: per-buffer fold state
    pub folds: crate::fold::FoldState,
    /// Memoized fold view, keyed by (blocks_generation, folds.epoch). Interior
    /// mutability so the accessor is `&self` (nav reads via `&Editor`).
    pub fold_view_cache: FoldViewCache,
    /// Generation the folded set was last reconciled (pruned) against. `None` on a
    /// fresh Buffer → the first rebuild always reconciles (covers reload/recovery).
    pub last_reconciled_generation: Option<u64>,
    /// Key `view.line_layouts` is currently valid for (Component 3, Task 3).
    pub layout_key: Option<crate::derive::LayoutKey>,
    // 9a: persistent marked block (half-open [start,end)) + a deferred begin anchor.
    pub marked_block: Option<MarkedBlock>,
    pub pending_block_begin: Option<usize>,
}

impl Buffer {
    /// Pure construction of a Buffer with a caller-supplied id (id source = alloc_id).
    /// Mirrors the buffer push in `Editor::new_from_text` exactly.
    pub fn from_text(id: BufferId, text: &str, path: Option<PathBuf>, area: (u16, u16)) -> Buffer {
        let buffer = TextBuffer::from_str(text);
        let blocks = match crate::panicx::catch(|| block_tree::full_parse_rope(&buffer.snapshot())) {
            Ok(t) => t,
            // No previous tree at construction — fall back to the empty tree so the
            // buffer still opens instead of crashing on an upstream parse panic.
            Err(_) => block_tree::empty_tree(text.len()),
        };
        let document = Document {
            buffer,
            selection: Selection::single(0),
            history: History::default(),
            blocks,
            version: 0,
            blocks_generation: 0,
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
            reconcile: crate::reconcile::ReconcileStore::default(),
            folds: crate::fold::FoldState::default(),
            fold_view_cache: std::cell::RefCell::new(None),
            last_reconciled_generation: None,
            layout_key: None,
            marked_block: None,
            pending_block_begin: None,
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
        self.folds.remap(&cs);
        // 9A: the marked block follows the text. start uses map_pos, end + pending use
        // map_pos_before → boundary inserts stay outside the half-open [start,end).
        self.pending_block_begin = self.pending_block_begin
            .map(|p| wordcartel_core::change::map_pos_before(p, &cs));
        if let Some(b) = self.marked_block.as_mut() {
            b.start = wordcartel_core::change::map_pos(b.start, &cs);
            b.end   = wordcartel_core::change::map_pos_before(b.end, &cs);
        }
        if self.marked_block.is_some_and(|b| b.start >= b.end) {
            self.marked_block = None; // collapsed → clear
        }
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
                self.folds.clamp(len);
                // 9A: undo/redo bypass apply's mapping → clear the block (acting on stale offsets unsafe).
                self.marked_block = None;
                self.pending_block_begin = None;
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
                self.folds.clamp(len);
                // 9A: undo/redo bypass apply's mapping → clear the block (acting on stale offsets unsafe).
                self.marked_block = None;
                self.pending_block_begin = None;
                true
            }
            None => false,
        }
    }

    /// Clear the visible-line layout cache AND its key — the invariant is
    /// "layout_key == Some(k) ⟹ line_layouts valid for k". Route every EXTERNAL
    /// line_layouts clear through this (Resize, reload/recovery).
    pub fn invalidate_layout(&mut self) {
        self.view.line_layouts.clear();
        self.layout_key = None;
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
    /// Deadline (ms) at which the auto-mode menu bar reveals (armed by a pointer
    /// dwell on row 0; re-armed on every row-0 motion — reveal fires after REST).
    pub menu_reveal_due: Option<u64>,
    /// Deadline (ms) at which a revealed auto-mode bar hides (armed ONCE on the
    /// first pointer-leave; cancelled by re-entering row 0 — the leave grace).
    pub menu_hide_due: Option<u64>,
    /// Whether the auto-mode bar is currently revealed (meaningless in other modes).
    pub menu_bar_revealed: bool,
    /// Right-edge dwell deadline for the Auto-mode scrollbar (armed on rest at col w-1).
    pub scrollbar_reveal_due: Option<u64>,
    /// Leave-grace deadline for the Auto-mode scrollbar (armed once on leave).
    pub scrollbar_hide_due: Option<u64>,
    /// Whether the Auto-mode scrollbar is currently dwell-revealed (independent of
    /// `scrollbar_until_ms`, which is the scroll-activity channel).
    pub scrollbar_revealed: bool,
    /// Bottom-row dwell deadline for the Auto-mode status line.
    pub status_reveal_due: Option<u64>,
    /// Leave-grace deadline for the Auto-mode status line.
    pub status_hide_due: Option<u64>,
    /// Whether the Auto-mode status line is currently dwell-revealed.
    pub status_revealed: bool,
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
    /// True while the last block-tree parse panicked (M4-rest). Dedupes the
    /// status notice so a persistently-panicking document does not spam it.
    pub parse_degraded: bool,
    pub quit: bool,
    pub prompt: Option<crate::prompt::Prompt>,
    /// Armed by `dispatch_save_then`; consumed by `apply_result` when the save lands.
    pub pending_after_save: Option<PendingAfterSave>,
    /// Carry the post-save action across an unnamed buffer's Save-As flow (Task 3).
    pub pending_save_as: Option<PostSaveAction>,
    /// The target awaiting an OverwriteSaveAs confirmation (existing-file Save-As). (Task 3)
    pub pending_save_overwrite: Option<PathBuf>,
    /// The target awaiting an OverwriteWriteBlock confirmation (^KW existing file). (9A Task 4)
    pub pending_write_block: Option<PathBuf>,
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
    /// The resolved preset name currently driving the loop-local keymap trie ("cua" or "wordstar").
    /// Seeded from config at startup; mutated by keymap_cua/keymap_wordstar commands.
    pub active_keymap_preset: String,
    /// Set by keymap switch commands; consumed by rebuild_keymap_if_requested after each reduce.
    pub keymap_rebuild: bool,
    pub palette: Option<crate::palette::Palette>,
    pub menu: Option<crate::menu::MenuView>,
    /// Whether mouse capture is currently requested (toggled by `toggle_mouse_capture`).
    /// Seeded from config at startup; defaults to `true` in test/scratch contexts.
    pub mouse_capture: bool,
    /// Menu bar visibility mode (seeded from `[menu] bar`; mutated only by menu_bar_pin).
    pub menu_bar_mode: crate::config::MenuBarMode,
    /// The mode menu_bar_pin restores on unpin (registry handlers cannot see Config).
    pub menu_bar_unpinned_mode: crate::config::MenuBarMode,
    /// Scrollbar visibility mode (seeded from `[view] scrollbar`; mutated at runtime).
    pub scrollbar_mode: crate::config::TransientMode,
    /// Status-line visibility mode (seeded from `[view] status_line`; mutated at runtime).
    pub status_line_mode: crate::config::TransientMode,
    /// Transient mouse gesture state; cleared by `reconcile_mouse_capture` on disable.
    pub mouse: MouseState,
    /// View/focus/writing-experience flags. Seeded from config; toggled by the 5 toggle_ commands.
    pub view_opts: crate::config::ViewConfig,
    /// Search/replace overlay state. XOR with prompt/minibuffer/palette/menu.
    pub search: Option<crate::search_overlay::SearchState>,
    /// Diagnostics configuration. Seeded from config at startup.
    pub diag_cfg: crate::config::DiagnosticsConfig,
    /// Export configuration (pdf engine, typography). Seeded from config at startup.
    pub export_cfg: crate::config::ExportConfig,
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
    /// File browser overlay state. XOR with all other overlays.
    pub file_browser: Option<crate::file_browser::FileBrowser>,
    /// Active theme + terminal color depth. Seeded at startup (real depth detection: plan ③).
    pub theme: wordcartel_core::theme::Theme,
    pub depth: wordcartel_core::theme::Depth,
    /// Chrome disposition (Full/Zen). Seeded from `[theme] chrome` at startup; toggled at
    /// runtime by the `toggle_chrome` command (T7). Passed to `resolve_theme` on re-derive.
    pub chrome_disposition: wordcartel_core::theme::ChromeDisposition,
    /// Canvas opacity (Opaque/Transparent). Seeded from `[theme] canvas` at startup; toggled at
    /// runtime by `toggle_canvas`. Render-only — never re-derives the theme.
    pub canvas: wordcartel_core::theme::CanvasMode,
    /// Request flag: `toggle_chrome` command sets this; the run-loop re-derives and clears it.
    /// Mirrors the `settings_save_requested` pattern (grounding A.8).
    pub theme_rederive: bool,
    /// Heading-level glyph toggle from config (seeded at startup; used by runtime picker — Task 7).
    pub heading_glyph_cfg: Option<bool>,
    /// Whether session-resume restore is enabled (seeded from `cfg.state.resume` in run()).
    /// Gates `open_into_current`'s resume restore. Defaults false until run() seeds it.
    pub resume_enabled: bool,
    /// Effort 6: the permanent path-less *scratch* buffer's id. `None` in unit
    /// contexts that never call `install_scratch`. Scratch is identified by id,
    /// not by a name field.
    pub scratch_id: Option<BufferId>,
    /// Most-recently-used buffer ids, most-recent first. Drives the switcher palette.
    pub mru: Vec<BufferId>,
    /// Effort 6: the in-progress multi-buffer quit drain, if any. `Some` while the
    /// Save-All / Review-each state machine is disposing of dirty buffers.
    pub quit_drain: Option<QuitDrain>,
    /// Effort 6: set by `apply_result`'s ContinueQuitDrain arm to ask the JobDone
    /// funnel (`apply_job_result`) to re-drive the drain once the save merge lands.
    pub quit_drain_advance: bool,
    /// Set by the `save_settings` command; consumed by Task 4's perform_settings_save
    /// hook in the run loop. Cleared after processing.
    pub settings_save_requested: bool,
    /// Provenance of the currently-active theme. Seeded from the default theme at
    /// startup; updated by the theme picker's Enter arm on confirmed selection, and
    /// by the run() startup path after theme resolution. Used by the diff law's
    /// rule-1 comparison (spec N-3: Builtin vs File is always a divergence).
    pub theme_identity: crate::settings::ThemeIdentity,
}

const UNDO_EVICTED_HINT: &str = "Undo history full — oldest dropped";

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
            register: Register::default(), status: String::new(), parse_degraded: false, quit: false,
            prompt: None, pending_after_save: None, pending_save_as: None, pending_save_overwrite: None,
            pending_write_block: None,
            filter_in_flight: None, transform_in_flight: false, minibuffer: None, pending_export: None,
            pending_mark: None,
            clipboard_sync_request: None, clipboard_get_pending: None, clipboard_notice_shown: false,
            pending_keys: Vec::new(),
            keymap,
            active_keymap_preset: "cua".into(),
            keymap_rebuild: false,
            palette: None,
            menu: None,
            mouse_capture: true,
            menu_bar_mode: crate::config::MenuBarMode::Auto,
            menu_bar_unpinned_mode: crate::config::MenuBarMode::Auto,
            scrollbar_mode: crate::config::TransientMode::Auto,
            // Status line defaults On — the idle info line is always shown out of the box
            // (preserves the pre-density behavior); Zen (chrome = zen) flips it to Auto.
            status_line_mode: crate::config::TransientMode::On,
            mouse: MouseState::default(),
            view_opts: crate::config::ViewConfig::default(),
            search: None,
            diag_cfg: crate::config::DiagnosticsConfig::default(),
            export_cfg: crate::config::ExportConfig::default(),
            dictionary: std::collections::HashSet::new(),
            session_ignores: std::collections::HashSet::new(),
            diag: None,
            outline: None,
            theme_picker: None,
            file_browser: None,
            theme: wordcartel_core::theme::default(),
            depth: wordcartel_core::theme::Depth::Truecolor,
            chrome_disposition: wordcartel_core::theme::ChromeDisposition::Full,
            canvas: wordcartel_core::theme::CanvasMode::Opaque,
            theme_rederive: false,
            heading_glyph_cfg: None,
            resume_enabled: false,
            scratch_id: None,
            mru: Vec::new(),
            quit_drain: None,
            quit_drain_advance: false,
            settings_save_requested: false,
            theme_identity: crate::settings::ThemeIdentity::Builtin("terminal-plain".into()),
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
    /// The active buffer's fold view, memoized by (blocks_generation, folds.epoch).
    /// Pure: never mutates document/fold state, so it takes `&self` and is usable
    /// from the `&Editor` nav helpers.
    pub fn active_fold_view(&self) -> std::rc::Rc<crate::fold::FoldView> {
        let b = self.active();
        let key = (b.document.blocks_generation, b.folds.epoch());
        {
            let cache = b.fold_view_cache.borrow();
            match &*cache {
                Some((g, e, rc)) if *g == key.0 && *e == key.1 => return rc.clone(),
                _ => {}
            }
        } // Ref dropped here, before borrow_mut below
        let view = std::rc::Rc::new(
            crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer));
        *b.fold_view_cache.borrow_mut() = Some((key.0, key.1, view.clone()));
        view
    }

    pub fn by_id(&self, id: BufferId) -> Option<&Buffer> { self.buffers.iter().find(|b| b.id == id) }
    pub fn by_id_mut(&mut self, id: BufferId) -> Option<&mut Buffer> { self.buffers.iter_mut().find(|b| b.id == id) }
    /// Allocate a fresh, never-reused BufferId.
    pub fn alloc_id(&mut self) -> BufferId { let id = BufferId(self.next_buffer_id); self.next_buffer_id += 1; id }

    /// Rows reserved by the menu bar at the top of the frame (0 or 1). THE single
    /// source of row-0 geometry truth — render/mouse/nav read this, never
    /// `menu.is_some()` directly (the dropdown-open checks in overlay routing are
    /// the deliberate exception: they mean "dropdown open", not geometry).
    pub fn menu_bar_rows(&self) -> u16 {
        let bar = match self.menu_bar_mode {
            crate::config::MenuBarMode::Pinned => true,
            crate::config::MenuBarMode::Auto => self.mouse.menu_bar_revealed,
            crate::config::MenuBarMode::Hidden => false,
        };
        u16::from(bar || self.menu.is_some())
    }

    /// Effort 6: create the permanent *scratch* buffer and record its id.
    /// Appended AFTER the launch buffer so the launch buffer stays at index 0
    /// (active). Idempotent guard: a second call is a no-op.
    pub fn install_scratch(&mut self) {
        if self.scratch_id.is_some() { return; }
        let id = self.alloc_id();
        let area = self.active().view.area;
        self.buffers.push(Buffer::from_text(id, "", None, area)); // empty (len 0)
        self.scratch_id = Some(id);
        // Seed MRU: active buffer first, scratch last.
        let active_id = self.buffers[self.active].id;
        self.mru = vec![active_id, id];
    }
    /// True iff `id` is the scratch buffer.
    #[inline] pub fn is_scratch(&self, id: BufferId) -> bool { self.scratch_id == Some(id) }
    /// Scratch-aware unsaved-work predicate. Scratch is NEVER dirty (it has no
    /// file and is auto-persisted to session state). All workspace logic uses this.
    pub fn is_dirty(&self, id: BufferId) -> bool {
        if self.is_scratch(id) { return false; }
        self.by_id(id).is_some_and(|b| b.document.dirty())
    }

    /// Move `id` to the front of the MRU list.
    pub fn touch_mru(&mut self, id: BufferId) {
        self.mru.retain(|&x| x != id);
        self.mru.insert(0, id);
    }
    /// Set the active buffer by index and record it MRU-front. Out-of-range → no-op.
    pub fn switch_to_index(&mut self, idx: usize) {
        if idx >= self.buffers.len() { return; }
        self.active = idx;
        let id = self.buffers[idx].id;
        self.touch_mru(id);
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
        self.file_browser = None;
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
        self.file_browser = None;
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
        self.file_browser = None;
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
        self.file_browser = None;
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
        self.file_browser = None;
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
        self.file_browser = None;
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
        self.file_browser = None;
        self.theme_picker = Some(crate::theme_picker::ThemePicker {
            query: String::new(), selected: 0, rows: Vec::new(),
            scroll_top: 0, original: self.theme.clone(), previewed: None,
        });
        if let Some(tp) = self.theme_picker.as_mut() { crate::theme_picker::rebuild_rows(tp); }
    }

    /// Open the buffer-switcher palette, enforcing the single-overlay XOR invariant.
    ///
    /// Sets `palette.kind = Buffers` and seeds rows from the MRU-ordered buffer
    /// list (via `workspace::buffer_switch_rows`). `rebuild_rows` is called
    /// immediately so rows are available before the first render.
    pub fn open_buffer_switcher(&mut self) {
        self.prompt = None;
        self.minibuffer = None;
        self.menu = None;
        self.pending_keys.clear();
        self.pending_mark = None;
        self.search = None;
        self.diag = None;
        self.outline = None;
        self.theme_picker = None;
        self.file_browser = None;
        let source_rows: Vec<crate::palette::PaletteRow> =
            crate::workspace::buffer_switch_rows(self)
                .into_iter()
                .map(|(id, label)| crate::palette::PaletteRow {
                    id: crate::registry::CommandId("palette"),
                    label,
                    chord: String::new(),
                    buffer: Some(id),
                })
                .collect();
        let mut p = crate::palette::Palette {
            kind: crate::palette::PaletteKind::Buffers,
            source_rows,
            ..Default::default()
        };
        // rebuild_rows for Buffers kind ignores the registry and keymap;
        // pass a dummy registry so the signature is satisfied.
        let reg = crate::registry::Registry::builtins();
        crate::palette::rebuild_rows(&mut p, &reg, &crate::keymap::KeyTrie::default());
        self.palette = Some(p);
    }

    /// Open the file browser at `dir`, enforcing the single-overlay XOR invariant.
    pub fn open_file_browser(&mut self, dir: std::path::PathBuf) {
        self.prompt = None; self.minibuffer = None; self.menu = None; self.palette = None;
        self.pending_keys.clear(); self.pending_mark = None;
        self.search = None; self.diag = None; self.outline = None; self.theme_picker = None;
        self.file_browser = Some(crate::file_browser::FileBrowser {
            dir, query: String::new(), entries: Vec::new(), selected: 0, scroll_top: 0,
        });
        if let Some(fb) = self.file_browser.as_mut() { crate::file_browser::rebuild_entries(fb); }
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

    /// Surface the undo-eviction hint iff an edit landed on the STILL-active buffer
    /// this reduce AND it evicted. Consumes `last_evicted` (resets to 0) so a later
    /// undo/redo/switch — which change `version` without a fresh eviction — do not
    /// replay the stale hint. Called once per reduce from the run loop.
    pub fn note_undo_eviction(&mut self, pre_id: BufferId, pre_version: u64) {
        let fire = {
            let b = self.active();
            b.id == pre_id && b.document.version != pre_version
                && b.document.history.last_evicted > 0
        };
        if fire {
            self.status = UNDO_EVICTED_HINT.to_string();
            self.active_mut().document.history.last_evicted = 0;
        }
    }

    // Thin delegators — external callers unchanged.
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        self.active_mut().apply(txn, edit, kind, clock);
    }
    pub fn undo(&mut self) -> bool { self.active_mut().undo() }
    pub fn redo(&mut self) -> bool { self.active_mut().redo() }

    // ------------------------------------------------------------------
    // Shared option setters (contract law 6 — single setter per user-settable option).
    // Every write path (individual commands + density::apply_bundle) routes through these.
    // ------------------------------------------------------------------

    /// Set the scrollbar transient mode and clear its stale dwell state. The single
    /// setter both the `scrollbar_*` commands and `density::apply_bundle` call (contract law 6).
    pub fn set_scrollbar_mode(&mut self, mode: crate::config::TransientMode) {
        self.scrollbar_mode = mode;
        self.mouse.scrollbar_reveal_due = None;
        self.mouse.scrollbar_hide_due = None;
        self.mouse.scrollbar_revealed = false;
    }

    /// Set the status-line transient mode (Off coerces to Auto — status has no true Off,
    /// no-silent-UI) and clear its stale dwell state.
    pub fn set_status_line_mode(&mut self, mode: crate::config::TransientMode) {
        use crate::config::TransientMode;
        self.status_line_mode = if mode == TransientMode::Off { TransientMode::Auto } else { mode };
        self.mouse.status_reveal_due = None;
        self.mouse.status_hide_due = None;
        self.mouse.status_revealed = false;
    }

    /// Set the menu-bar mode, keeping `menu_bar_unpinned_mode` (the mode `menu_bar_pin`
    /// restores on unpin) consistent, and clear menu dwell state. Generalizes menu_bar_pin.
    pub fn set_menu_bar_mode(&mut self, mode: crate::config::MenuBarMode) {
        use crate::config::MenuBarMode;
        if mode == MenuBarMode::Pinned {
            if self.menu_bar_mode != MenuBarMode::Pinned { self.menu_bar_unpinned_mode = self.menu_bar_mode; }
        } else {
            self.menu_bar_unpinned_mode = mode;
        }
        self.menu_bar_mode = mode;
        self.mouse.menu_reveal_due = None;
        self.mouse.menu_hide_due = None;
        self.mouse.menu_bar_revealed = false;
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

    // ------------------------------------------------------------------
    // Task 2: TransientMode fields + dwell-timer defaults
    // ------------------------------------------------------------------

    #[test]
    fn editor_seeds_transient_modes_and_mouse_dwell_defaults() {
        let e = Editor::new_from_text("x\n", None, (40, 8));
        assert_eq!(e.scrollbar_mode, crate::config::TransientMode::Auto);
        assert_eq!(e.status_line_mode, crate::config::TransientMode::On);
        assert_eq!(e.mouse.scrollbar_reveal_due, None);
        assert!(!e.mouse.status_revealed);
    }

    // ------------------------------------------------------------------
    // F2: shared cached FoldView (blocks_generation + folds.epoch key)
    // ------------------------------------------------------------------

    #[test]
    fn active_fold_view_reuses_rc_when_unchanged() {
        let e = Editor::new_from_text("# A\nbody\n", None, (80, 24));
        let v1 = e.active_fold_view();
        let v2 = e.active_fold_view();
        assert!(std::rc::Rc::ptr_eq(&v1, &v2), "same state → cached Rc reused");
    }

    #[test]
    fn active_fold_view_recomputes_on_generation_bump() {
        let mut e = Editor::new_from_text("# A\nbody\n", None, (80, 24));
        let v1 = e.active_fold_view();
        // Bump the generation via the sole write path (unchanged tree) — exercises
        // the real accessor, mirroring the sibling derive.rs rerun test.
        let t = e.active().document.blocks().clone();
        e.active_mut().document.set_blocks(t);
        let v2 = e.active_fold_view();
        assert!(!std::rc::Rc::ptr_eq(&v1, &v2), "generation bump invalidates the cache");
    }

    #[test]
    fn active_fold_view_recomputes_on_fold_toggle() {
        let mut e = Editor::new_from_text("# A\nbody\n", None, (80, 24));
        let v1 = e.active_fold_view();
        e.active_mut().folds.toggle(0); // fold the "# A" heading at byte 0
        let v2 = e.active_fold_view();
        assert!(!std::rc::Rc::ptr_eq(&v1, &v2), "fold epoch bump invalidates the cache");
    }

    #[test]
    fn cached_foldview_equals_fresh() {
        let mut e = Editor::new_from_text("# A\nbody\n", None, (80, 24));
        e.active_mut().folds.toggle(0);
        let cached = e.active_fold_view();
        let fresh = {
            let b = e.active();
            crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer)
        };
        assert_eq!(*cached, fresh, "cached view is byte-identical to a fresh compute");
    }

    #[test]
    fn merge_bumps_generation_invalidates() {
        // Regression guard: the reconcile merge adopts a new tree WITHOUT bumping
        // document.version — it bumps blocks_generation instead. The FoldView cache
        // keys on blocks_generation, so it must still invalidate across the merge.
        let mut e = Editor::new_from_text("# A\nbody\n", None, (80, 24));
        let id = e.active().id;
        let v1 = e.active_fold_view();
        let other_tree = wordcartel_core::block_tree::full_parse_rope(
            &TextBuffer::from_str("# A\n## B\nbody\n").snapshot());
        {
            let b = e.by_id_mut(id).expect("active buffer by id");
            b.document.set_blocks(other_tree); // adopt the new tree + bump generation, via the sole write path
        }
        let v2 = e.active_fold_view();
        assert!(!std::rc::Rc::ptr_eq(&v1, &v2), "merge generation bump must invalidate the FoldView cache");
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
        assert!(buf.folds.folded().contains(&(b_off + 2)));
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
        assert!(buf.folds.folded().contains(&0));
        assert!(!buf.folds.folded().contains(&1));
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
        assert!(buf.folds.folded().iter().all(|&b| b <= len));
    }

    #[test]
    fn editor_seeds_default_theme_truecolor() {
        let ed = Editor::new_from_text("x", None, (80, 24));
        assert_eq!(ed.theme.name, "terminal-plain"); // default() now named terminal-plain (D5)
        assert_eq!(ed.depth, wordcartel_core::theme::Depth::Truecolor);
    }

    // Editor has NO insert/delete helpers (Codex) — drive edits through apply, building the
    // changeset with the existing build_multi_replace. The editor.rs test module's TestClock
    // is `Cell<u64>`-based (editor.rs:522): `TestClock(std::cell::Cell::new(0))` — Codex.
    fn ap(e: &mut Editor, edits: &[(usize, usize, &str)]) {
        let doc_len = e.active().document.buffer.len();
        let owned: Vec<(usize, usize, String)> = edits.iter().map(|(a,b,s)| (*a,*b,s.to_string())).collect();
        let (cs, edit) = crate::commands::build_multi_replace(&owned, doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs);
        e.apply(txn, edit, wordcartel_core::history::EditKind::Other, &TestClock(std::cell::Cell::new(0)));
    }

    #[test]
    fn marked_block_tracks_edits_and_collapses() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        ap(&mut e, &[(0, 0, "XX")]); // insert "XX" at byte 0 → block shifts right by 2
        let b = e.active().marked_block.unwrap();
        assert_eq!((b.start, b.end), (8, 13));
        let len = e.active().document.buffer.len();
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: len, hidden: false });
        ap(&mut e, &[(0, len, "")]); // delete the whole region → collapse → cleared
        assert!(e.active().marked_block.is_none(), "fully-deleted block clears");
    }

    #[test]
    fn marked_block_boundary_inserts_stay_outside() {
        let mut e = Editor::new_from_text("ab cd\n", None, (40, 10));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 5, hidden: false }); // "cd"
        ap(&mut e, &[(5, 5, "X")]); // insert at end → end stays (map_pos_before), block does NOT grow
        assert_eq!(e.active().marked_block.unwrap().end, 5);
        ap(&mut e, &[(3, 3, "Y")]); // insert at start → start moves past (map_pos), block does NOT grow at front
        assert_eq!(e.active().marked_block.unwrap().start, 4);
    }

    #[test]
    fn undo_clears_marked_block() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        ap(&mut e, &[(0, 0, "Z")]); // make history non-empty
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 2, hidden: false });
        e.undo(); // Editor::undo exists (editor.rs:512)
        assert!(e.active().marked_block.is_none(), "undo clears the block (it bypasses apply mapping)");
    }

    #[test]
    fn open_buffer_switcher_yields_buffers_kind_palette() {
        let mut e = Editor::new_from_text("doc\n", None, (40, 10));
        e.install_scratch();
        e.buffers[0].document.path = Some(std::path::PathBuf::from("/tmp/doc.md"));
        e.open_buffer_switcher();
        let p = e.palette.as_ref().expect("palette opened by open_buffer_switcher");
        assert!(matches!(p.kind, crate::palette::PaletteKind::Buffers),
            "kind must be Buffers, not Commands");
        assert!(!p.rows.is_empty(), "rows populated immediately (no hydrate needed)");
        assert!(p.rows.iter().all(|r| r.buffer.is_some()),
            "all buffer-switcher rows carry a BufferId");
        // rows[0] should be doc (install_scratch seeds MRU as [doc, scratch])
        assert!(p.rows.iter().any(|r| r.label.contains("doc.md")));
        assert!(p.rows.iter().any(|r| r.label == "*scratch*"));
    }

    #[test]
    fn install_scratch_adds_permanent_pathless_buffer() {
        let mut e = Editor::new_from_text("doc\n", None, (40, 10));
        assert_eq!(e.buffers.len(), 1);
        assert_eq!(e.scratch_id, None, "no scratch until installed");
        e.install_scratch();
        assert_eq!(e.buffers.len(), 2, "scratch appended");
        let sid = e.scratch_id.expect("scratch_id set");
        assert!(e.is_scratch(sid));
        let sb = e.by_id(sid).unwrap();
        assert!(sb.document.path.is_none(), "scratch has no path");
        assert_eq!(e.active, 0, "launch buffer stays active");
    }

    #[test]
    fn is_dirty_excludes_scratch_even_when_edited() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("doc\n", None, (40, 10));
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        // Edit the scratch buffer directly via build_multi_replace + Buffer::apply.
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "hi".into())], 0);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(2));
        e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        assert!(e.by_id(sid).unwrap().document.dirty(), "raw predicate says dirty");
        assert!(!e.is_dirty(sid), "is_dirty excludes scratch");
        // An edited ordinary buffer IS dirty via is_dirty.
        let aid = e.buffers[0].id;
        let (cs2, edit2) = crate::commands::build_multi_replace(&[(0, 0, "x".into())], 4);
        let txn2 = wordcartel_core::history::Transaction::new(cs2)
            .with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn2, edit2, wordcartel_core::history::EditKind::Other, &C(0));
        assert!(e.is_dirty(aid), "ordinary edited buffer is dirty via is_dirty");
    }

    #[test]
    fn switch_to_index_sets_active_and_touches_mru() {
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        e.install_scratch(); // [doc(0), scratch(1)], mru = [doc, scratch]
        let scratch = e.scratch_id.unwrap();
        e.switch_to_index(1);
        assert_eq!(e.active, 1);
        assert_eq!(e.mru.first().copied(), Some(scratch), "switched buffer is MRU-front");
    }

    #[test]
    fn scratch_buffer_derive_rebuild_smoke() {
        // An empty (len 0) scratch buffer must survive derive::rebuild without panic.
        let mut e = Editor::new_from_text("doc\n", None, (40, 10));
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        let idx = e.buffers.iter().position(|b| b.id == sid).unwrap();
        e.active = idx;
        crate::derive::rebuild(&mut e);
        assert_eq!(e.by_id(sid).unwrap().document.buffer.len(), 0);
    }

    // ── Task 2 (M5): eviction hint wiring ─────────────────────────────────────

    /// Guard: `Editor::apply` is a pure delegator — the eviction hint now lives in
    /// `note_undo_eviction` (wired once per reduce in the run loop). A tiny edit that
    /// does not evict must leave `status` unchanged regardless of which path set it.
    #[test]
    fn apply_does_not_set_hint_when_no_eviction() {
        use wordcartel_core::change::ChangeSet;
        use wordcartel_core::history::Transaction;
        let clk = TestClock(std::cell::Cell::new(0));
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let cs = ChangeSet::insert(0, "x", e.active().document.buffer.len());
        e.apply(Transaction::new(cs), Edit { range: 0..0, new_len: 1 }, EditKind::Type, &clk);
        assert_ne!(
            e.status,
            UNDO_EVICTED_HINT,
            "a tiny edit must not set the eviction hint"
        );
    }

    #[test]
    fn note_undo_eviction_fires_once_on_active_edit_with_eviction() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().document.version += 1;              // simulate an edit
        e.active_mut().document.history.last_evicted = 1;  // that evicted
        e.note_undo_eviction(id, v);
        assert_eq!(e.status, UNDO_EVICTED_HINT);
        assert_eq!(e.active().document.history.last_evicted, 0, "consumed");
        // A later version bump (e.g. undo) must NOT replay the stale hint:
        e.status.clear();
        let v2 = e.active().document.version;
        e.active_mut().document.version += 1;
        e.note_undo_eviction(id, v2);
        assert_ne!(e.status, UNDO_EVICTED_HINT, "no re-fire after reset");
    }

    #[test]
    fn note_undo_eviction_ignores_no_edit_and_switch() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().document.history.last_evicted = 1;
        e.note_undo_eviction(id, v);                       // version unchanged → no edit
        assert_ne!(e.status, UNDO_EVICTED_HINT, "no edit → no hint");
        e.active_mut().document.version += 1;
        e.note_undo_eviction(BufferId(id.0.wrapping_add(999)), v); // id mismatch → switch
        assert_ne!(e.status, UNDO_EVICTED_HINT, "id mismatch (switch) → no hint");
    }

    #[test]
    fn menu_bar_rows_truth_table() {
        use crate::config::MenuBarMode as M;
        let mut e = Editor::new_from_text("x\n", None, (20, 6));
        for (mode, revealed, open, want) in [
            (M::Hidden, false, false, 0u16), (M::Hidden, true, false, 0), (M::Hidden, false, true, 1),
            (M::Auto, false, false, 0), (M::Auto, true, false, 1), (M::Auto, false, true, 1),
            (M::Pinned, false, false, 1), (M::Pinned, true, true, 1),
        ] {
            e.menu_bar_mode = mode;
            e.mouse.menu_bar_revealed = revealed;
            e.menu = if open { Some(crate::menu::empty()) } else { None };
            assert_eq!(e.menu_bar_rows(), want, "mode={mode:?} revealed={revealed} open={open}");
        }
    }

    // ------------------------------------------------------------------
    // Task 1 (A3): shared option setters
    // ------------------------------------------------------------------

    #[test]
    fn setters_set_field_and_clear_dwell() {
        use crate::config::{TransientMode, MenuBarMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        // scrollbar: mode set + BOTH dwell timers and the revealed flag cleared.
        e.mouse.scrollbar_revealed = true;
        e.mouse.scrollbar_reveal_due = Some(9);
        e.mouse.scrollbar_hide_due = Some(9);
        e.set_scrollbar_mode(TransientMode::On);
        assert_eq!(e.scrollbar_mode, TransientMode::On);
        assert!(!e.mouse.scrollbar_revealed
            && e.mouse.scrollbar_reveal_due.is_none()
            && e.mouse.scrollbar_hide_due.is_none());
        // status: Off coerces to Auto (no true Off) + all status dwell cleared.
        e.mouse.status_revealed = true;
        e.mouse.status_reveal_due = Some(7);
        e.mouse.status_hide_due = Some(7);
        e.set_status_line_mode(TransientMode::Off);
        assert_eq!(e.status_line_mode, TransientMode::Auto);
        assert!(!e.mouse.status_revealed
            && e.mouse.status_reveal_due.is_none()
            && e.mouse.status_hide_due.is_none());
        // menu: mode set + all menu dwell cleared.
        e.mouse.menu_bar_revealed = true;
        e.mouse.menu_reveal_due = Some(3);
        e.mouse.menu_hide_due = Some(3);
        e.set_menu_bar_mode(MenuBarMode::Auto);
        assert_eq!(e.menu_bar_mode, MenuBarMode::Auto);
        assert!(!e.mouse.menu_bar_revealed
            && e.mouse.menu_reveal_due.is_none()
            && e.mouse.menu_hide_due.is_none());
    }

    #[test]
    fn set_menu_bar_mode_keeps_unpinned_mode_consistent() {
        use crate::config::MenuBarMode;
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.set_menu_bar_mode(MenuBarMode::Auto);
        assert_eq!(e.menu_bar_mode, MenuBarMode::Auto);
        assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Auto, "non-Pinned set → remembered");
        e.set_menu_bar_mode(MenuBarMode::Pinned);
        assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Auto, "Pinned set → remembers prior non-Pinned");
        e.set_menu_bar_mode(MenuBarMode::Hidden);
        assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Hidden);
    }

    #[test]
    fn apply_bundle_keeps_menu_bar_unpinned_mode_consistent() {
        // INTENTIONAL change (spec-accepted): apply_bundle now routes menu_bar through
        // set_menu_bar_mode, so FULL (Pinned) remembers the prior non-Pinned mode as the unpin
        // target; previously apply_bundle left menu_bar_unpinned_mode stale.
        use crate::config::MenuBarMode;
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.set_menu_bar_mode(MenuBarMode::Hidden); // prior non-Pinned mode = Hidden
        crate::density::apply_bundle(&mut e, &crate::density::FULL); // FULL sets Pinned
        assert_eq!(e.menu_bar_mode, MenuBarMode::Pinned);
        assert_eq!(e.menu_bar_unpinned_mode, MenuBarMode::Hidden, "FULL remembers the prior mode as unpin target");
    }
}
