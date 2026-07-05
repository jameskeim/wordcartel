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
        if let Some(t) = app::rebuild_keymap_if_requested(&mut self.editor, &[], &self.reg) {
            self.keymap = t;
        }
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
    fn mouse_move(&mut self, col: u16, row: u16) {
        self.step(Msg::Input(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: col, row, modifiers: KeyModifiers::NONE,
        })));
    }
    fn mouse_down(&mut self, col: u16, row: u16) {
        self.step(Msg::Input(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: col, row, modifiers: KeyModifiers::NONE,
        })));
    }

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

// ---------------------------------------------------------------------------
// A1 journeys 3 + 4: pinned mode and hidden mode dwell gate.
// ---------------------------------------------------------------------------

/// A1 journey 3: pinned — the bar is always there; Esc closes the dropdown ONLY.
#[test]
fn journey_pinned_bar_persists_across_dropdown_close() {
    let mut h = Harness::new("hello world\n", None, (40, 8));
    h.editor.menu_bar_mode = crate::config::MenuBarMode::Pinned;
    h.tick(); // render with the mode applied
    assert!(h.row(0).contains(" File "), "pinned bar visible before any menu use");
    assert!(h.row(1).contains("hello"), "text shifted below the bar");
    h.key(KeyCode::F(10));
    assert!(h.editor.menu.is_some(), "F10 opens the dropdown");
    h.key(KeyCode::Esc);
    assert!(h.editor.menu.is_none(), "Esc closes the dropdown");
    assert!(h.row(0).contains(" File "), "the bar PERSISTS after Esc (the state split)");
}

/// A1 journey 4: hidden — the dwell is mode-gated (non-vacuous form, spec M4).
#[test]
fn journey_hidden_never_reveals_on_dwell() {
    let mut h = Harness::new("hello world\n", None, (40, 8));
    h.editor.menu_bar_mode = crate::config::MenuBarMode::Hidden;
    h.mouse_move(5, 0);
    h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
    h.tick();
    assert!(!h.editor.mouse.menu_bar_revealed, "Hidden mode must never arm/reveal");
    assert!(h.row(0).contains("hello"), "row 0 is still text");
    h.key(KeyCode::F(10));
    assert!(h.row(0).contains(" File "), "F10 still opens");
    h.key(KeyCode::Esc);
    assert!(h.row(0).contains("hello"), "Esc closes FULLY in hidden mode");
}

/// A1 journey 1: dwell-reveal (rest), grace-hide (leave), and grace-cancel (return).
#[test]
fn journey_auto_dwell_reveal_and_grace_hide() {
    let mut h = Harness::new("hello world\n", None, (40, 8));
    // default mode is Auto; row 0 is text while unrevealed
    assert!(h.row(0).contains("hello"));
    h.mouse_move(5, 0);
    h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
    h.tick();
    assert!(h.row(0).contains(" File "), "bar revealed after the dwell");
    assert!(h.row(1).contains("hello"), "text reserved down one row");
    // leave: grace, not instant
    h.mouse_move(5, 5);
    assert!(h.row(0).contains(" File "), "still revealed during the grace");
    h.advance_ms(crate::mouse::MENU_LEAVE_GRACE_MS + 1);
    h.tick();
    assert!(h.row(0).contains("hello"), "hidden after the grace; text back on row 0");
    // reveal again, then leave-and-return WITHIN the grace: the bar survives
    h.mouse_move(5, 0);
    h.advance_ms(crate::mouse::MENU_DWELL_MS + 1);
    h.tick();
    h.mouse_move(5, 5);
    h.advance_ms(100); // < grace
    h.mouse_move(5, 0);
    h.advance_ms(crate::mouse::MENU_LEAVE_GRACE_MS + 1);
    h.tick();
    assert!(h.row(0).contains(" File "), "return within the grace keeps the bar");
}

/// A1 journey 2: a drag across row 0 never arms the dwell.
#[test]
fn journey_drag_never_reveals() {
    let mut h = Harness::new("hello world\nmore text here\n", None, (40, 8));
    h.mouse_down(2, 1);            // start a text drag (dragging = true)
    h.mouse_move(2, 0);            // motion onto row 0 mid-drag
    h.advance_ms(crate::mouse::MENU_DWELL_MS + 10);
    h.tick();
    assert!(!h.editor.mouse.menu_bar_revealed, "drag must not arm the dwell");
    assert!(h.row(0).contains("hello"), "row 0 stays text");
}

/// A1 journey 5: a row-0 click while unrevealed is a TEXT click.
#[test]
fn journey_row0_click_unrevealed_edits_text() {
    let mut h = Harness::new("hello world\n", None, (40, 8));
    h.mouse_down(4, 0);
    assert!(h.editor.menu.is_none(), "no menu opened");
    assert_eq!(h.editor.active().document.selection.primary().head, 4,
        "the click placed the caret in the text");
}

/// A6 journey: opening the palette and pressing End reaches the LAST registered
/// command without filtering, and Enter dispatches it.
///
/// The last command is `keymap_wordstar` (D1+A5 registration order) — benign and
/// observable: dispatch flips active_keymap_preset and sets the switch status.
/// The reach-without-typing property is the contract; selected must be within
/// the visible window (selected - scroll_top < 15) before Enter.
#[test]
fn journey_palette_end_reaches_last_command() {
    // A tall document keeps the palette's row math honest under scrolling.
    let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut h = Harness::new(&text, None, (80, 24));
    h.ctrl('p'); // open the Command Palette
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    h.key(KeyCode::End); // jump to the last row
    let p = h.editor.palette.as_ref().unwrap();
    let total = p.rows.len();
    let last_idx = total.saturating_sub(1);
    assert_eq!(p.selected, last_idx, "End must land on the last row (idx={last_idx})");
    // Windowing invariant: selection is within the visible window.
    assert!(p.selected.saturating_sub(p.scroll_top) < 15,
        "End: selected={} scroll_top={} must be within the 15-row window",
        p.selected, p.scroll_top);
    // Last label on screen — confirms the window shows the tail.
    let last_label = p.rows[last_idx].label.clone();
    assert!(h.screen_contains(&last_label),
        "last command label {last_label:?} must be visible on screen after End");
    // Enter dispatches keymap_wordstar (last registered command) → keymap switches,
    // status is set, palette closes. Verifies the end-of-list dispatch path (spec I4).
    h.key(KeyCode::Enter);
    assert!(h.editor.palette.is_none(), "Enter closes the palette");
    assert_eq!(h.editor.active_keymap_preset, "wordstar",
        "keymap_wordstar must be dispatched and set the active preset");
    assert_eq!(h.editor.status, "keymap: wordstar",
        "the switch status must survive through the rebuild");
}

// ---------------------------------------------------------------------------
// B1+B2 word-wrap journeys
// ---------------------------------------------------------------------------

/// B1 journey: typing past the viewport edge wraps at word boundaries — no
/// row ends mid-word. End/Home/Up/Down navigate across the wrap without panic.
#[test]
fn journey_typing_never_breaks_midword() {
    // 12-wide viewport; "the quick brown fox jumps over" wraps to 3 screen rows.
    // With word-wrap: row 0 = "the quick " (10), row 1 = "brown fox " (10),
    // row 2 = "jumps over" (10). The trailing spaces on rows 0-1 are the
    // original inter-word spaces, not padding — "quick" and "brown" are on
    // different rows, so "quick b" never appears as a contiguous run.
    let mut h = Harness::new("", None, (12, 8));
    h.type_str("the quick brown fox jumps over");
    assert!(h.screen_contains("the quick"), "first two words on same row");
    assert!(h.screen_contains("brown fox"), "second pair on same row");
    assert!(h.screen_contains("jumps over"), "last pair on same row");
    assert!(!h.screen_contains("quick b"), "quick and brown are NOT on the same row");

    // Caret is on screen (on the last row, end of text).
    let pos = crate::nav::screen_pos(&h.editor);
    assert!(pos.is_some(), "caret must be on screen after typing");

    // End/Home/Up/Down across the wrap — must not panic.
    h.key(KeyCode::End);
    h.key(KeyCode::Home);
    h.key(KeyCode::Up);
    h.key(KeyCode::Down);
    // Text still intact on screen after navigation.
    assert!(h.screen_contains("the quick"), "text intact after navigation");
}

/// B1+B2 journey: a two-level nested list item that wraps renders the bullet at
/// the indent column and the continuation text under the text column (hanging-indent).
/// The ACTIVE line renders raw with no glyph (Fable plan I-2); the caret must be
/// moved OFF the item line before asserting the bullet — Down navigates there.
#[test]
fn journey_nested_list_wraps_hanging() {
    // 12-wide; "  - alpha beta" → prefix_width 4, "alpha" at col 4, "beta" wraps.
    //   row 0: "  • alpha   " (bullet '•' at col 2, text col 4, space hangs)
    //   row 1: "    beta    " (continuation: spacer cols 0..4, "beta" at col 4)
    // Harness::new does a full parse → correct block tree; caret starts at byte 0
    // (line 0, ACTIVE). Two Down presses cross the two visual rows of line 0 and
    // land on "more" (line 1, active) → line 0 is now INACTIVE → glyph renders.
    let mut h = Harness::new("  - alpha beta\nmore\n", None, (12, 8));
    h.key(KeyCode::Down); // visual row 0 → visual row 1 (beta continuation)
    h.key(KeyCode::Down); // visual row 1 → line 1 ("more")
    // Bullet must appear at indent col 2 on the first screen row.
    assert!(h.screen_contains("\u{2022} alpha"),
        "bullet + item text on row 0:\n{:#?}", h.screen());
    // Continuation row: four-space spacer then "beta" (text under text column).
    assert!(h.screen_contains("    beta"),
        "continuation at col 4 on row 1:\n{:#?}", h.screen());
}

// ---------------------------------------------------------------------------
// C4 journeys: close-buffer with dirty-confirm prompt
// ---------------------------------------------------------------------------

/// C4 journey — Save & close path: a dirty named buffer with a scratch neighbor is
/// palette-dispatched through close_buffer → close_confirm prompt → 's' → save +
/// close. The file on disk receives the typed text; the buffer is gone; the status
/// row reads "saved — closed".
///
/// Arrange: a real tempfile (created empty so stored_fp matches the on-disk
/// fingerprint — else dispatch_save raises the external-change modal). A scratch
/// buffer is installed as the neighbor so close has somewhere to go after the last
/// ordinary buffer is replaced.
#[test]
fn journey_close_dirty_save_and_close() {
    // Create the empty tempfile BEFORE Harness::new so stored_fp == fingerprint(path)
    // (else dispatch_save raises the external-change modal instead of saving — spec I4).
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let mut h = Harness::new("", Some(path.clone()), (80, 24));
    // Scratch provides a neighbor so close_buffer_now's last-ordinary branch runs
    // (replaces the named slot with a fresh untitled — buffer count stays at 2).
    h.editor.install_scratch();
    let orig_id = h.editor.active().id;
    // Dirty the named buffer.
    h.type_str("hello");
    assert!(h.dirty(), "buffer must be dirty before close");
    // Palette-dispatch close_buffer: ctrl-p → type "close" → Enter.
    // "close" uniquely fuzzy-matches "Close Buffer" (case-insensitive; only one hit).
    // dispatch_overlay_command closes the palette, runs close_buffer → dirty → open_prompt.
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    h.type_str("close");
    h.key(KeyCode::Enter);
    assert!(h.editor.prompt.is_some(),
        "close_buffer on a dirty named buffer must open the close-confirm prompt");
    assert!(h.screen_contains("[S]ave & close"),
        "close-confirm message must appear on the status row:\n{:#?}", h.screen());
    // 's' → CloseSave: dispatch_save_then dispatches a save job; InlineExecutor runs
    // it inline (within reduce); the merge calls close_buffer_now then "saved — closed".
    h.key(KeyCode::Char('s'));
    assert!(h.editor.by_id(orig_id).is_none(),
        "named buffer must be gone after Save & close");
    assert!(h.editor.active().id != orig_id, "a neighbor is active after close");
    // The neighbor (a fresh untitled in the last-ordinary slot) is what renders now:
    // the closed buffer's typed text must no longer be on screen.
    assert!(!h.screen_contains("hello"),
        "closed buffer's text must be gone from the screen:\n{:#?}", h.screen());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello",
        "typed text must have been written to disk");
    assert_eq!(h.status(), "saved — closed",
        "status must confirm save and close");
}

/// C4 journey — Discard path: same arrange plus a real swap file written via the
/// swap API. After 'd', the buffer is closed, the file on disk is UNCHANGED, and
/// the swap file still exists (decision 1 pin: Discard does not delete the swap).
#[test]
fn journey_close_dirty_discard_leaves_file_and_swap() {
    // File with original content. Created before Harness::new so stored_fp is consistent.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    std::fs::write(&path, "original\n").unwrap();
    let mut h = Harness::new("original\n", Some(path.clone()), (80, 24));
    h.editor.install_scratch();
    // Dirty the buffer: type at the start so the buffer diverges from disk.
    h.type_str("draft");
    assert!(h.dirty(), "buffer must be dirty before close");
    // Write a stub swap file (simulates the auto-swap writer having fired).
    let sp = crate::swap::swap_path(Some(&path)).unwrap();
    crate::swap::write_atomic(&sp, "stub swap").unwrap();
    assert!(sp.exists(), "precondition: swap file must exist before close");
    let orig_id = h.editor.active().id;
    // Palette-dispatch close_buffer → close-confirm prompt.
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    h.type_str("close");
    h.key(KeyCode::Enter);
    assert!(h.editor.prompt.is_some(),
        "close_buffer on a dirty named buffer must open the close-confirm prompt");
    assert!(h.screen_contains("[D]iscard"),
        "close-confirm message must appear on the status row:\n{:#?}", h.screen());
    // 'd' → CloseDiscard: close_buffer_now runs immediately, swap NOT deleted.
    h.key(KeyCode::Char('d'));
    assert!(h.editor.by_id(orig_id).is_none(),
        "named buffer must be gone after Discard");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n",
        "disk file must be UNCHANGED after Discard");
    assert!(sp.exists(), "swap file must survive Discard (decision 1 pin)");
    let _ = std::fs::remove_file(&sp);
}

// ---------------------------------------------------------------------------
// C2 journey: caret reflow vs. Reflow Buffer scope separation
// ---------------------------------------------------------------------------

/// C2 journey — caret reflow only transforms the item under the caret; Reflow
/// Buffer (via the Command Palette) transforms the whole document.
#[test]
fn journey_transform_scopes() {
    // Three-item tight list — all items long enough to genuinely rewrap at 72 cols.
    let item1 = "- first item here with text that is long enough to be reflowed at seventy two character width indeed\n";
    let item2 = "- second item here with text that is long enough to be reflowed at seventy two character width indeed\n";
    let item3 = "- third item here with text that is long enough to be reflowed at seventy two character width indeed\n";
    let text = format!("{item1}{item2}{item3}");
    let mut h = Harness::new(&text, None, (80, 24));

    // Place the caret inside item 2's body (past "- second item here").
    let item2_start = item1.len();
    h.editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(item2_start + 10);

    // Ctrl+T opens the transform chooser.
    h.ctrl('t');
    assert!(h.editor.prompt.is_some(), "Ctrl+T must open the transform chooser");
    assert!(h.screen_contains("[r]eflow"),
        "transform chooser message must appear on screen:\n{:#?}", h.screen());

    // 'r' — reflow the transform unit under the caret (item 2 only).
    h.key(KeyCode::Char('r'));
    let after_item2 = h.doc_text();
    assert_ne!(after_item2, text, "item 2 must have been reflowed by caret reflow");
    assert!(after_item2.starts_with(item1),
        "item 1 must be verbatim after caret reflow:\n{after_item2:?}");
    assert!(after_item2.ends_with(item3),
        "item 3 must be verbatim after caret reflow:\n{after_item2:?}");

    // Reflow Buffer via the Command Palette — transforms the whole document.
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "Ctrl+P must open the palette");
    h.type_str("reflow buffer");
    // Precondition: "reflow buffer" filters the palette to the Reflow Buffer row.
    {
        let p = h.editor.palette.as_ref().unwrap();
        assert!(!p.rows.is_empty(), "palette must have at least one match for 'reflow buffer'");
        assert_eq!(p.rows[0].label, "Reflow Buffer",
            "first match must be Reflow Buffer: {:?}",
            p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>());
    }
    h.key(KeyCode::Enter);
    let after_all = h.doc_text();
    assert_ne!(after_all, after_item2,
        "Reflow Buffer must change the document (items 1 and 3 still have long single lines)");
    // Items 1 and 3 were still long single lines after the caret reflow —
    // Reflow Buffer must have reflowed them (they are no longer verbatim).
    assert!(!after_all.contains(item1),
        "item 1 must be reflowed by Reflow Buffer (original long line gone):\n{after_all:?}");
    assert!(!after_all.contains(item3),
        "item 3 must be reflowed by Reflow Buffer (original long line gone):\n{after_all:?}");
}
