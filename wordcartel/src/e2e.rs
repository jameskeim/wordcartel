#![cfg(test)]
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use wordcartel_core::block_tree::{BlockTree, full_parse_rope};

use crate::app::{self, Msg, reduce};
use crate::editor::Editor;
use crate::jobs::InlineExecutor;
use crate::keymap::{self, KeyTrie};
use crate::registry::Registry;
use crate::render;
use crate::test_support::{TestClock, key_char, press};

struct Harness {
    editor: Editor,
    reg: Registry,
    keymap: KeyTrie,
    ex: InlineExecutor,
    term: Terminal<TestBackend>,
    tx: Sender<Msg>,
    _rx: Receiver<Msg>,
    now: u64,
}

impl Harness {
    /// NOTE (Fable M-4): the first frame here is NOT identical to production's first frame for
    /// buffers with restored fold/scroll state — `run()` runs an extra pre-first-draw block
    /// (app.rs:2059: folded-cursor SnapOut + `ensure_visible`) that this omits. Harmless for the
    /// seed journeys (fresh buffer, cursor 0, no folds); revisit if a journey restores fold/scroll.
    fn new(text: &str, path: Option<PathBuf>, size: (u16, u16)) -> Self {
        let mut editor = Editor::new_from_text(text, path, size);
        editor.diag_cfg.enabled = false; // hermeticity: no real diagnostics thread (spec I3)
        let reg = Registry::builtins();
        let (keymap, _warn) = keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let term = Terminal::new(TestBackend::new(size.0, size.1)).expect("test terminal");
        let (tx, _rx) = mpsc::channel();
        let mut h = Harness { editor, reg, keymap, ex, term, tx, _rx, now: 0 };
        crate::derive::rebuild(&mut h.editor);
        h.render();
        h
    }

    /// The shared production sequence: snapshot → reduce → note_undo_eviction → advance → render.
    /// NOTE: `run()` additionally runs `drain_clipboard_intents` + `reconcile_mouse_capture`
    /// between `note_undo_eviction` and `advance`; the harness omits them (terminal-output only,
    /// state-orthogonal for the seed journeys). A clipboard/mouse journey must add them.
    fn step(&mut self, msg: Msg) -> bool {
        let (pre_id, pre_version) = { let b = self.editor.active(); (b.id, b.document.version) };
        let clock = TestClock(self.now);
        let keep = reduce(msg, &mut self.editor, &self.reg, &self.keymap, &self.ex, &clock, &self.tx);
        self.editor.note_undo_eviction(pre_id, pre_version);
        app::advance(&mut self.editor, &clock);
        self.render();
        keep
    }

    fn render(&mut self) {
        let editor = &mut self.editor;
        self.term.draw(|f| render::render(f, editor)).expect("draw");
    }

    // — input sugar —
    fn type_str(&mut self, s: &str) { for c in s.chars() { self.step(Msg::Input(Event::Key(key_char(c)))); } }
    fn ctrl(&mut self, c: char) -> bool { self.step(press(KeyCode::Char(c), KeyModifiers::CONTROL)) }
    fn alt(&mut self, c: char) -> bool { self.step(press(KeyCode::Char(c), KeyModifiers::ALT)) }
    fn key(&mut self, code: KeyCode) -> bool { self.step(press(code, KeyModifiers::NONE)) }
    fn resize(&mut self, w: u16, h: u16) {
        self.term.backend_mut().resize(w, h);         // sync the TestBackend cell grid
        self.step(Msg::Input(Event::Resize(w, h)));   // update the editor's buffer areas
    }

    fn advance_ms(&mut self, ms: u64) { self.now = self.now.saturating_add(ms); }
    fn tick(&mut self) -> bool { self.step(Msg::Tick) }

    // — state accessors —
    fn doc_text(&self) -> String { self.editor.active().document.buffer.to_string() }
    fn dirty(&self) -> bool { self.editor.active().document.dirty() }
    fn saved_version(&self) -> Option<u64> { self.editor.active().document.saved_version } // Option, not u64 (editor.rs:64)
    fn status(&self) -> &str { &self.editor.status }
    fn blocks(&self) -> &BlockTree { self.editor.active().document.blocks() }
    fn folded(&self) -> &std::collections::BTreeSet<usize> { self.editor.active().folds.folded() }
    fn maybe_stale(&self) -> bool { self.editor.active().reconcile.maybe_stale }
    fn in_flight(&self) -> Option<u64> { self.editor.active().reconcile.in_flight_version }
    fn reconcile_blocks_version(&self) -> u64 { self.editor.active().reconcile.blocks_version }
    fn version(&self) -> u64 { self.editor.active().document.version }
    fn rope(&self) -> ropey::Rope { self.editor.active().document.buffer.snapshot() }

    // — screen assertions —
    fn row(&self, y: u16) -> String {
        let buf = self.term.backend().buffer();
        let w = buf.area().width;
        (0..w).map(|x| buf[(x, y)].symbol()).collect()
    }
    fn screen(&self) -> Vec<String> {
        let h = self.term.backend().buffer().area().height;
        (0..h).map(|y| self.row(y)).collect()
    }
    fn screen_contains(&self, needle: &str) -> bool { self.screen().iter().any(|r| r.contains(needle)) }
}

#[test]
fn e2e_type_shows_in_doc_and_render() {
    let mut h = Harness::new("", None, (80, 24));
    h.type_str("hello");
    assert_eq!(h.doc_text(), "hello");
    assert!(h.screen_contains("hello"), "typed text must appear on screen:\n{:#?}", h.screen());
}

#[test]
fn e2e_save_writes_file_and_reloads() {
    // Create the empty tempfile BEFORE Harness::new so stored_fp == fingerprint(path)
    // (else dispatch_save raises the external-change modal instead of saving — spec I4).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let mut h = Harness::new("", Some(path.clone()), (80, 24));
    h.type_str("hello\n");
    h.ctrl('s'); // save runs inline under InlineExecutor; reduce drains before returning
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    assert_eq!(h.status(), "Saved");
    assert!(!h.dirty());
    assert!(h.saved_version().is_some(), "saved_version set after a successful save");
    // (Fable M-5: the post-save swap::delete touches state_dir() which create_dir_all's the real
    //  XDG state dir — empty, nothing written; negligible + matches the existing save tests.)
    // Reload: a fresh harness opening the same file round-trips.
    let h2 = Harness::new(&std::fs::read_to_string(&path).unwrap(), Some(path.clone()), (80, 24));
    assert_eq!(h2.doc_text(), "hello\n");
}

#[test]
fn e2e_resize_does_not_blank_the_screen() {
    let mut h = Harness::new("hello", None, (80, 24));
    assert!(h.screen_contains("hello"));
    h.resize(80, 24);  // SAME dims — the SIGWINCH class that blanked via a stale layout_key
    assert!(h.screen_contains("hello"), "same-dim resize blanked the screen:\n{:#?}", h.screen());
    h.resize(100, 30); // different dims
    assert!(h.screen_contains("hello"), "resize blanked the screen:\n{:#?}", h.screen());
}

#[test]
fn e2e_reconcile_converges_a_stale_tree() {
    let mut h = Harness::new("# A\n\nbody\n", None, (80, 24));
    // Plant a deliberately-wrong tree + mark stale (mirrors reconcile.rs:104-126).
    {
        let b = h.editor.active_mut();
        // A deliberately-wrong tree (empty), mirroring reconcile.rs:104-126's plant.
        let len = b.document.buffer.len();
        b.document.set_blocks(wordcartel_core::block_tree::empty_tree(len));
        b.reconcile.maybe_stale = true;
    }
    // Precondition: genuinely divergent from a full parse of the real text.
    let want = full_parse_rope(&h.rope());
    assert_ne!(h.blocks(), &want, "planted tree must differ from full_parse (else vacuous)");
    // Drive the debounce: one tick to arm (advance sets due_at = now+150), then
    // advance past the deadline + tick to dispatch.
    h.tick();                                   // advance arms due_at = now + 150
    h.advance_ms(crate::reconcile::RECONCILE_DEBOUNCE_MS + 1);
    h.tick();                                   // now >= due_at → reduce dispatches reparse; InlineExecutor runs it; reduce drains
    // Machinery ran:
    assert!(!h.maybe_stale());
    assert!(h.in_flight().is_none());
    assert_eq!(h.reconcile_blocks_version(), h.version());
    // Content converged:
    assert_eq!(h.blocks(), &full_parse_rope(&h.rope()));
}

#[test]
fn e2e_undo_redo() {
    let mut h = Harness::new("", None, (80, 24));
    h.type_str("abc");                 // frozen clock → ONE coalesced revision (COALESCE_MS=500)
    assert_eq!(h.doc_text(), "abc");
    h.ctrl('z');                       // undo → reverts the whole coalesced insert
    assert_eq!(h.doc_text(), "");
    assert!(!h.screen_contains("abc"), "undone text must be gone from the screen");
    h.ctrl('y');                       // redo
    assert_eq!(h.doc_text(), "abc");
}

#[test]
fn e2e_quit_dirty_raises_modal_not_silent_quit() {
    let mut h = Harness::new("x", None, (80, 24));
    h.type_str("y");                   // dirty
    let keep = h.ctrl('q');
    assert!(keep, "dirty Ctrl+Q must NOT quit silently");
    assert!(h.editor.prompt.is_some(), "dirty Ctrl+Q must raise the quit_multi modal");
    // Discard path: 'r' (review each) → 'd' (discard) quits.
    h.key(KeyCode::Char('r'));
    let keep2 = h.key(KeyCode::Char('d'));
    assert!(!keep2, "review→discard must quit");
}

#[test]
fn e2e_fold_hides_body_in_render() {
    let mut h = Harness::new("# Head\n\nsecret body line\n\n# Other\n", None, (80, 24));
    assert!(h.screen_contains("secret body line"), "body must render BEFORE folding (else vacuous)");
    // Cursor is at the top (byte 0, inside "# Head"); Alt+Z folds that section.
    h.alt('z');
    assert!(!h.folded().is_empty(), "Alt+Z must fold the heading");
    assert!(h.screen_contains("Head"), "the heading stays visible");
    assert!(!h.screen_contains("secret body line"), "the folded body must be hidden:\n{:#?}", h.screen());
}
