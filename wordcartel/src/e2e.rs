#![cfg(test)]
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
// NOTE: `BlockTree`/`full_parse_rope`/`ropey` imports are added in Task 3 (only the Task-3
// journeys/accessors use them) so Task 2's `--no-run` gate stays warning-free (Fable I-1).

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

    // — input sugar (Task 2 subset — the rest are added in Task 3, so each task's `--no-run`
    //   gate stays warning-free; Fable I-1) —
    fn type_str(&mut self, s: &str) { for c in s.chars() { self.step(Msg::Input(Event::Key(key_char(c)))); } }
    fn ctrl(&mut self, c: char) -> bool { self.step(press(KeyCode::Char(c), KeyModifiers::CONTROL)) }

    // — state assertions —
    fn doc_text(&self) -> String { self.editor.active().document.buffer.to_string() }
    fn dirty(&self) -> bool { self.editor.active().document.dirty() }
    fn saved_version(&self) -> Option<u64> { self.editor.active().document.saved_version } // Option, not u64 (editor.rs:64)
    fn status(&self) -> &str { &self.editor.status }

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
