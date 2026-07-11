#![cfg(test)]
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use wordcartel_core::block_tree::{BlockTree, full_parse_rope};

use crate::app::{self, Msg, reduce};
use crate::editor::Editor;
use crate::jobs::{Executor, InlineExecutor, Job, JobOutcome, ThreadExecutor};
use crate::keymap::{self, KeyTrie};
use crate::registry::Registry;
use crate::render;
use crate::test_support::{TestClock, key_char, press};

/// The e2e Harness runs the deterministic `InlineExecutor` by default (all seed
/// journeys). The R1 bench also needs the REAL threaded executor to measure the
/// off-thread reconcile merge, so the executor is an enum that dispatches to
/// either backend. `&self.ex` still coerces to `&dyn Executor` for `reduce`.
enum BenchExecutor {
    Inline(InlineExecutor),
    Thread(ThreadExecutor),
}

impl Executor for BenchExecutor {
    fn dispatch(&self, job: Job) {
        match self {
            BenchExecutor::Inline(e) => e.dispatch(job),
            BenchExecutor::Thread(e) => e.dispatch(job),
        }
    }
    fn drain(&self) -> Vec<JobOutcome> {
        match self {
            BenchExecutor::Inline(e) => e.drain(),
            BenchExecutor::Thread(e) => e.drain(),
        }
    }
}

/// Coarse per-stage timings for one `step_timed` call, plus the fine-grained
/// derive spans drained after `advance`. The bench sums `spans` by label to get
/// per-keystroke `parse`/`heading_starts`/`foldview`/`layout_fill`; `total` is
/// `t_reduce + t_advance + t_render`.
struct PhaseTimes {
    t_reduce: std::time::Duration,
    t_advance: std::time::Duration,
    t_render: std::time::Duration,
    spans: Vec<(&'static str, std::time::Duration)>,
}

struct Harness {
    editor: Editor,
    reg: Registry,
    keymap: KeyTrie,
    ex: BenchExecutor,
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
        let ex = BenchExecutor::Inline(InlineExecutor::default());
        let term = Terminal::new(TestBackend::new(size.0, size.1)).expect("test terminal");
        let (tx, _rx) = mpsc::channel();
        let mut h = Harness { editor, reg, keymap, ex, term, tx, _rx, now: 0 };
        crate::derive::rebuild(&mut h.editor);
        h.render();
        h
    }

    /// Construct a harness backed by the given executor (R1 bench: threaded runs
    /// exercise the real off-thread reconcile merge). Mirrors `new` otherwise.
    fn new_with(text: &str, path: Option<PathBuf>, size: (u16, u16), ex: BenchExecutor) -> Self {
        let mut editor = Editor::new_from_text(text, path, size);
        editor.diag_cfg.enabled = false;
        let reg = Registry::builtins();
        let (keymap, _warn) = keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
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
        if let Some(t) = crate::theme_cmds::rebuild_keymap_if_requested(&mut self.editor, &[], &self.reg) {
            self.keymap = t;
        }
        self.editor.note_undo_eviction(pre_id, pre_version);
        app::advance(&mut self.editor, &clock);
        self.render();
        keep
    }

    /// Timed mirror of `step` for the R1 bench: identical production sequence
    /// (reduce → note_undo_eviction → advance → render) but each coarse stage is
    /// wrapped in `Instant::now()/elapsed()`, and the fine-grained derive spans
    /// (`parse`/`heading_starts`/`foldview`/`layout_fill`) recorded inside
    /// `derive::rebuild` are drained after `advance`. Spans accumulate across BOTH
    /// the post-command rebuild (in `reduce`) and the pre-draw rebuild (in
    /// `advance`); the caller sums them per label to get the true per-keystroke
    /// derive cost (the second rebuild is memoized, so only cache-hit residue).
    fn step_timed(&mut self, msg: Msg) -> PhaseTimes {
        let (pre_id, pre_version) = { let b = self.editor.active(); (b.id, b.document.version) };
        let clock = TestClock(self.now);
        // Clear any residue so this step's spans are attributable to this step.
        let _ = crate::derive::bench_spans::drain();
        let t0 = std::time::Instant::now();
        let _keep = reduce(msg, &mut self.editor, &self.reg, &self.keymap, &self.ex, &clock, &self.tx);
        let t_reduce = t0.elapsed();
        if let Some(t) = crate::theme_cmds::rebuild_keymap_if_requested(&mut self.editor, &[], &self.reg) {
            self.keymap = t;
        }
        self.editor.note_undo_eviction(pre_id, pre_version);
        let t1 = std::time::Instant::now();
        app::advance(&mut self.editor, &clock);
        let t_advance = t1.elapsed();
        let spans = crate::derive::bench_spans::drain();
        let t2 = std::time::Instant::now();
        self.render();
        let t_render = t2.elapsed();
        PhaseTimes { t_reduce, t_advance, t_render, spans }
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
/// The last command is `save_settings` (D1+A5 T3 registration order) — benign and
/// observable: dispatch sets `settings_save_requested = true`. The reach-without-typing
/// property is the contract; selected must be within the visible window
/// (selected - scroll_top < 15) before Enter.
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
    // Enter dispatches save_settings (last registered command) → settings_save_requested
    // is set, palette closes. Verifies the end-of-list dispatch path (spec I4).
    h.key(KeyCode::Enter);
    assert!(h.editor.palette.is_none(), "Enter closes the palette");
    assert!(h.editor.settings_save_requested,
        "save_settings must be dispatched and set settings_save_requested");
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

// ---------------------------------------------------------------------------
// D1+A5 journey: keymap-switch ctrl-w scopes
// ---------------------------------------------------------------------------

/// D1+A5 journey: ctrl-w changes meaning when the keymap preset switches. Both
/// directions of the flip are pinned — neither observation is vacuous.
///
/// CUA ctrl-w = expand_selection (the selection grows from an empty caret).
/// WordStar ctrl-w = scroll_line_up (scroll decrements; selection stays empty).
#[test]
fn journey_keymap_switch_scopes() {
    // Build a 40-line doc so a caret placed at line 30 sets scroll > 0.
    let text: String = "line\n".repeat(40);
    let mut h = Harness::new(&text, None, (80, 24));

    // Place the caret at the start of line 30 (byte 150 = 30 × 5).
    // A direct selection write does NOT scroll — ensure_visible adjusts the
    // viewport (review-mandated: nothing in the harness path calls it for us).
    h.editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(150);
    crate::nav::ensure_visible(&mut h.editor);
    h.render();

    // Precondition: the viewport has scrolled past line 0.
    let scroll_after_move = h.editor.active().view.scroll;
    assert!(scroll_after_move > 0,
        "scroll must be > 0 after moving caret to line 30 (got {scroll_after_move})");

    // Precondition: the selection is a collapsed caret before CUA ctrl-w.
    assert!(h.editor.active().document.selection.primary().is_empty(),
        "selection must be empty before CUA ctrl-w");

    // CUA ctrl-w = expand_selection: the selection must grow from the caret.
    h.ctrl('w');
    assert!(!h.editor.active().document.selection.primary().is_empty(),
        "CUA ctrl-w must expand the selection from a collapsed caret");

    // Collapse the selection back to a caret (head position) so the post-switch
    // assertion starts from an empty selection — a clean baseline.
    let head = h.editor.active().document.selection.primary().head;
    h.editor.active_mut().document.selection =
        wordcartel_core::selection::Selection::single(head);

    // Switch to WordStar via the Command Palette.
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    h.type_str("keymap: wordstar");
    // Precondition: the palette top row must be "Keymap: WordStar".
    {
        let p = h.editor.palette.as_ref().unwrap();
        assert!(!p.rows.is_empty(), "'keymap: wordstar' must match at least one command");
        assert_eq!(p.rows[0].label, "Keymap: WordStar",
            "top palette row must be 'Keymap: WordStar': {:?}",
            p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>());
    }
    h.key(KeyCode::Enter);
    assert!(h.status().contains("keymap: wordstar"),
        "status must confirm the preset switch: {:?}", h.status());

    // WordStar ctrl-w = scroll_line_up: selection stays empty; scroll decrements by 1.
    let scroll_before = h.editor.active().view.scroll;
    assert!(scroll_before > 0,
        "scroll must still be > 0 before WordStar ctrl-w (got {scroll_before})");
    h.ctrl('w');
    assert!(h.editor.active().document.selection.primary().is_empty(),
        "WordStar ctrl-w must leave the selection empty");
    assert_eq!(h.editor.active().view.scroll, scroll_before - 1,
        "WordStar ctrl-w must decrement scroll by exactly 1 (was {scroll_before})");
}

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

// ---------------------------------------------------------------------------
// E3+E4 chrome journey
// ---------------------------------------------------------------------------

/// E3+E4 journey: toggle chrome to Zen via the Command Palette under tokyo-night,
/// then persist the disposition via Save Settings.
///
/// Step 1: Install resolved tokyo-night (Rgb bases, Truecolor depth — the toggle's
///   normal arm fires and sets an exact "chrome: zen" status).
/// Step 2: Open the palette; assert palette rows render on screen (text-only harness;
///   themed-interior cell styling owned by T5's `tokyo_overlay_interior_is_themed`
///   render pin — delegated, noted here).
/// Step 3: Dispatch `toggle_chrome` via the palette → status == "chrome: zen" exactly.
/// Step 4: Assert the overrides file carries `[theme] chrome = "zen"` after calling
///   `perform_settings_save` directly — the harness has no save-loop arm, so the
///   D1+A5-era live-path shape is used: palette dispatch sets the flag; we clear
///   the flag and call the helper directly with a temp-dir path.
#[test]
fn journey_chrome_zen_toggle() {
    use wordcartel_core::theme::ChromeDisposition;

    let mut h = Harness::new("hello world\n", None, (80, 24));

    // Step 1: install resolved tokyo-night.
    // derive_chrome before apply so chrome faces are fully resolved (grounding A.9/D3).
    {
        let mut theme = wordcartel_core::theme::tokyo_night();
        theme.derive_chrome(ChromeDisposition::Full);
        h.editor.apply_theme(theme);
        h.editor.theme_identity =
            crate::settings::ThemeIdentity::Builtin("tokyo-night".into());
        h.render();
    }
    // Precondition: document text on screen (theme change must not blank the frame).
    assert!(h.screen_contains("hello world"), "text must be visible after theme set:\n{:#?}", h.screen());

    // Step 2: open the palette; assert it renders rows on screen (text-only proxy
    // for the palette overlay; themed interior owned by T5's render pin).
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    assert!(!h.editor.palette.as_ref().unwrap().rows.is_empty(), "unfiltered palette must have rows");
    {
        // row[0] of the unfiltered palette is the first registered command — assert
        // it appears on screen as the text observable for the palette overlay.
        let row0 = h.editor.palette.as_ref().unwrap().rows[0].label.clone();
        assert!(h.screen_contains(&row0),
            "palette row 0 label {row0:?} must render on screen:\n{:#?}", h.screen());
    }

    // Step 3: filter to "chrome" → top row must be "Chrome: Full/Zen"; Enter dispatches.
    h.type_str("chrome");
    {
        let p = h.editor.palette.as_ref().unwrap();
        assert!(!p.rows.is_empty(), "'chrome' must match at least one command");
        assert_eq!(p.rows[0].label, "Chrome: Full/Zen",
            "top row must be 'Chrome: Full/Zen': {:?}",
            p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>());
    }
    h.key(KeyCode::Enter);
    // tokyo-night: Rgb bases, Truecolor depth → normal toggle arm → exact status.
    assert_eq!(h.status(), "chrome: zen",
        "toggle under tokyo-night must set status to 'chrome: zen'");
    assert_eq!(h.editor.chrome_disposition, ChromeDisposition::Zen,
        "chrome_disposition must be Zen after toggle");

    // Step 4: dispatch "Save Settings" via the palette (sets settings_save_requested),
    // then drive perform_settings_save directly (harness has no save-loop arm).
    h.ctrl('p');
    assert!(h.editor.palette.is_some(), "ctrl-p must open the palette");
    h.type_str("save settings");
    {
        let p = h.editor.palette.as_ref().unwrap();
        assert!(!p.rows.is_empty(), "'save settings' must match at least one command");
        assert_eq!(p.rows[0].label, "Save Settings",
            "top palette row must be 'Save Settings': {:?}",
            p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>());
    }
    h.key(KeyCode::Enter); // dispatches save_settings → sets settings_save_requested = true
    assert!(h.editor.settings_save_requested,
        "save_settings must set settings_save_requested");

    // Build a baseline snapshot representing the no-chrome-config startup state.
    // baseline.chrome_disposition = Full → diff against runtime Zen → writes "zen".
    let baseline = crate::settings::SettingsSnapshot {
        keymap_preset:      "cua".into(),
        theme_identity:     crate::settings::ThemeIdentity::Builtin("terminal-plain".into()),
        view_typewriter:    false,
        view_focus:         false,
        view_measure:       false,
        view_wrap_guide:    false,
        view_word_count:    false,
        view_wrap_column:   72,
        view_scrollbar:     crate::config::TransientMode::Auto,
        view_status_line:   crate::config::TransientMode::On,
        view_splash:        true,
        menu_bar:           crate::config::MenuBarMode::Auto,
        mouse_capture:      true,
        chrome_disposition: ChromeDisposition::Full,
        canvas:             wordcartel_core::theme::CanvasMode::Opaque,
        clipboard_provider: crate::config::ClipboardProvider::Auto,
    };
    // Mirror the run-loop pattern: clear the flag, then write the file.
    h.editor.settings_save_requested = false;
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("settings-overrides.toml");
    let of = crate::settings::perform_settings_save(
        &mut h.editor,
        false,
        Some(&path),
        &baseline,
        &crate::settings::OverridesFile::default(),
        &crate::settings::OverridesFile::default(),
        &crate::fsx::RealFs,
    );
    assert!(of.is_some(), "perform_settings_save must succeed");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("chrome = \"zen\""),
        "overrides file must carry '[theme] chrome = \"zen\"';\ngot:\n{text}");
}

#[test]
fn e2e_splash_first_frame_then_key_dismisses_and_is_consumed() {
    let mut h = Harness::new("hello behind\n", None, (80, 24));
    // Mirror run()'s startup wiring (app.rs: gate → resolve against the live keymap →
    // set before the first draw). view_opts carries the ViewConfig default (splash on).
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, false, h.editor.prompt.is_some());
    assert!(show, "default config + no flag + no prompt shows the splash");
    h.editor.splash = Some(crate::splash::Splash::new(&h.keymap, "0.1.0"));
    h.render();
    assert!(h.screen_contains("wordcartel"), "wordmark on the first frame");
    assert!(h.screen_contains("press any key"), "footer on the first frame");
    assert!(!h.screen_contains("hello behind"), "the splash owns the screen");
    // The first key press dismisses AND is consumed (not typed into the buffer).
    let keep = h.step(Msg::Input(Event::Key(key_char('x'))));
    assert!(keep);
    assert!(h.editor.splash.is_none(), "splash cleared by the first key");
    assert_eq!(h.doc_text(), "hello behind\n", "the dismissing key was consumed");
    assert!(h.screen_contains("hello behind"), "dismiss reveals the document");
    // The NEXT key edits normally.
    h.type_str("y");
    assert_eq!(h.doc_text(), "yhello behind\n");
}

#[test]
fn e2e_splash_mouse_click_dismisses_without_editing() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    h.editor.splash = Some(crate::splash::Splash::new(&h.keymap, "0.1.0"));
    h.render();
    assert!(!h.screen_contains("hello"));
    h.mouse_down(10, 5);
    assert!(h.editor.splash.is_none(), "mouse-down dismisses");
    assert_eq!(h.doc_text(), "hello\n", "the click did not edit anything");
    assert!(h.screen_contains("hello"));
}

#[test]
fn e2e_no_splash_flag_suppresses_first_frame_splash() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, true, h.editor.prompt.is_some());
    assert!(!show, "--no-splash wins over the enabled config default");
    // run() therefore leaves editor.splash = None → the first frame is the plain editor.
    h.render();
    assert!(h.screen_contains("hello"));
    assert!(!h.screen_contains("press any key"));
}

#[test]
fn e2e_recovery_prompt_pending_suppresses_splash() {
    let mut h = Harness::new("hello\n", None, (80, 24));
    // Also probe the defense-in-depth belt (editor.rs open_prompt): set a splash
    // BEFORE opening the recovery prompt, proving the two can never coexist even if a
    // future startup-gate change let both get set — the render must show the prompt
    // only, never the wordmark underneath it.
    let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &h.reg);
    h.editor.splash = Some(crate::splash::Splash::new(&km, "0.1.0"));
    h.editor.open_prompt(crate::prompt::Prompt::swap_recovery());
    let show = crate::splash::show_at_startup(
        h.editor.view_opts.splash, false, h.editor.prompt.is_some());
    assert!(!show, "a pending recovery prompt suppresses the splash");
    assert!(h.editor.splash.is_none(), "open_prompt clears any pending splash (belt)");
    h.render();
    assert!(h.screen_contains("Recovery file found"),
        "the recovery prompt is what the user sees:\n{:#?}", h.screen());
    assert!(!h.screen_contains("wordcartel") && !h.screen_contains("press any key"),
        "the splash must never be painted over a modal prompt:\n{:#?}", h.screen());
}

// ===========================================================================
// R1 typing-latency bench (exploratory measurement — NOT a correctness gate).
//
// Drives the REAL reduce → advance(derive::rebuild) → render loop through the
// e2e Harness's timed `step_timed`, across a scenario matrix (N × structure ×
// edit-class × executor), with a realistic-cadence burst driver that lets the
// 150ms reconcile debounce fire BETWEEN keystrokes. Decomposes per-keystroke
// tail latency by phase (reduce | parse | heading_starts | foldview |
// layout_fill | render | total) and fits log-log slopes vs N to localize the
// O(document) typing cost.
//
// Run: cargo test --release e2e_bench -- --nocapture --test-threads=1
// (Debug Instant timing is meaningless — release is mandatory.)
// ===========================================================================
#[cfg(test)]
mod e2e_bench {
    use super::{BenchExecutor, Harness, PhaseTimes};
    use crate::app::Msg;
    use crate::jobs::ThreadExecutor;
    use crate::test_support::{key_char, press};
    use crossterm::event::{Event, KeyCode, KeyModifiers};
    use std::collections::BTreeMap;

    // -- configuration -------------------------------------------------------
    const N_VALUES: &[usize] = &[1_000, 4_000, 16_000, 64_000, 256_000, 1_000_000];
    const VIEWPORT: (u16, u16) = (100, 40);
    const CADENCE_GAP_MS: u64 = 180; // ~5-6 cps inter-key gap
    const FRAME_MS: u64 = 16; // ~60Hz tick granularity between keystrokes
    const PHASES: &[&str] =
        &["reduce", "parse", "heading_starts", "foldview", "layout_fill", "render", "total"];

    /// Reps per N — capped at high N so wall time stays bounded (logged in caps).
    fn reps_for(n: usize) -> usize {
        match n {
            x if x <= 16_000 => 5,
            x if x <= 64_000 => 4,
            x if x <= 256_000 => 3,
            _ => 2,
        }
    }
    /// Spread-edit repeats shrink at high N (each rebuild is O(doc)).
    fn spread_reps_for(n: usize) -> usize { if n >= 256_000 { 10 } else { 20 } }
    /// Sustained-burst char count (slightly shorter at 1M).
    fn sustained_chars_for(n: usize) -> usize { if n >= 1_000_000 { 30 } else { 40 } }

    // -- fixture generators (tile a structure template to ~target bytes) ------
    fn gen_flat_prose(target: usize) -> String {
        const W: &[&str] = &[
            "the", "quick", "brown", "fox", "jumps", "over", "a", "lazy", "dog", "while",
            "clouds", "drift", "slowly", "across", "the", "calm", "autumn", "sky", "above",
            "distant", "hills", "where", "rivers", "wind", "through", "quiet", "green", "valleys",
        ];
        let mut s = String::with_capacity(target + 256);
        let mut wi = 0usize;
        while s.len() < target {
            for _ in 0..5 {
                let mut col = 0usize;
                loop {
                    let w = W[wi % W.len()];
                    if col > 0 && col + 1 + w.len() > 80 { break; }
                    if col > 0 { s.push(' '); col += 1; }
                    s.push_str(w);
                    col += w.len();
                    wi += 1;
                }
                s.push('\n');
            }
            s.push('\n');
        }
        s
    }
    fn gen_nested_list(target: usize) -> String {
        let mut s = String::with_capacity(target + 256);
        let mut i = 0usize;
        while s.len() < target {
            s.push_str(&format!("- item {i} at top level with some descriptive text here\n\n"));
            s.push_str(&format!("  - nested child {i} carrying a bit more text to fill the line\n\n"));
            s.push_str(&format!("    - deep grandchild {i} with yet more words for real width\n\n"));
            i += 1;
        }
        s
    }
    fn gen_heading_dense(target: usize) -> String {
        let mut s = String::with_capacity(target + 256);
        let mut i = 0usize;
        while s.len() < target {
            s.push_str(&format!("## Section {i}\n\n"));
            for l in 0..4 {
                s.push_str(&format!("Body line {i}.{l} with a reasonable amount of prose to pad it.\n"));
            }
            s.push('\n');
            i += 1;
        }
        s
    }
    fn gen_code_heavy(target: usize) -> String {
        let mut s = String::with_capacity(target + 256);
        let mut i = 0usize;
        while s.len() < target {
            s.push_str(&format!("Paragraph {i} introducing the code block below.\n\n"));
            s.push_str("```rust\n");
            for l in 0..6 {
                s.push_str(&format!("    let value_{i}_{l} = compute(item, {l}); // note\n"));
            }
            s.push_str("```\n\n");
            i += 1;
        }
        s
    }
    fn gen_giant_table(target: usize) -> String {
        let mut s = String::with_capacity(target + 256);
        s.push_str("| id | name | description | value |\n");
        s.push_str("|----|------|-------------|-------|\n");
        let mut i = 0usize;
        while s.len() < target {
            s.push_str(&format!("| {i} | row {i} | a description cell for row {i} here | {} |\n", i * 7));
            i += 1;
        }
        s
    }
    type FixtureGen = fn(usize) -> String;
    fn structures() -> Vec<(&'static str, FixtureGen)> {
        vec![
            ("flat-prose", gen_flat_prose as FixtureGen),
            ("nested-loose-list", gen_nested_list),
            ("heading-dense", gen_heading_dense),
            ("code-heavy", gen_code_heavy),
            ("giant-table", gen_giant_table),
        ]
    }
    fn heading_count(text: &str) -> usize {
        text.lines().filter(|l| l.trim_start().starts_with('#')).count()
    }

    // -- sample store --------------------------------------------------------
    #[derive(Default)]
    struct Samples {
        // (n_bytes, n_headings, structure, edit_class, phase) -> micros samples
        map: BTreeMap<(usize, usize, &'static str, &'static str, &'static str), Vec<u128>>,
    }
    impl Samples {
        fn push(&mut self, n: usize, hd: usize, st: &'static str, ec: &'static str, ph: &'static str, us: u128) {
            self.map.entry((n, hd, st, ec, ph)).or_default().push(us);
        }
        /// Record one timed step: coarse reduce/render + summed derive spans + total.
        fn record(&mut self, n: usize, hd: usize, st: &'static str, ec: &'static str, pt: &PhaseTimes) {
            let mut by: BTreeMap<&'static str, u128> = BTreeMap::new();
            for (lbl, d) in &pt.spans { *by.entry(lbl).or_insert(0) += d.as_micros(); }
            self.push(n, hd, st, ec, "reduce", pt.t_reduce.as_micros());
            for lbl in ["parse", "heading_starts", "foldview", "layout_fill"] {
                self.push(n, hd, st, ec, lbl, *by.get(lbl).unwrap_or(&0));
            }
            self.push(n, hd, st, ec, "render", pt.t_render.as_micros());
            let total = pt.t_reduce.as_micros() + pt.t_advance.as_micros() + pt.t_render.as_micros();
            self.push(n, hd, st, ec, "total", total);
        }
        fn p99(&self, n: usize, st: &str, ec: &str, ph: &str) -> Option<u128> {
            self.stat(n, st, ec, ph).map(|(_, _, p99, _)| p99)
        }
        /// (p50, p95, p99, max) for the first cell matching (n, st, ec, ph).
        fn stat(&self, n: usize, st: &str, ec: &str, ph: &str) -> Option<(u128, u128, u128, u128)> {
            for ((kn, _hd, kst, kec, kph), v) in &self.map {
                if *kn == n && *kst == st && *kec == ec && *kph == ph {
                    let mut s = v.clone();
                    s.sort_unstable();
                    return Some((pct(&s, 50.0), pct(&s, 95.0), pct(&s, 99.0), *s.last().unwrap_or(&0)));
                }
            }
            None
        }
        fn to_csv(&self) -> String {
            let mut out = String::new();
            out.push_str("n_bytes,n_headings,structure,edit_class,phase,samples,p50_us,p95_us,p99_us,max_us,overshoot_120,overshoot_60\n");
            for ((n, hd, st, ec, ph), v) in &self.map {
                let mut s = v.clone();
                s.sort_unstable();
                let (p50, p95, p99, mx) = (pct(&s, 50.0), pct(&s, 95.0), pct(&s, 99.0), *s.last().unwrap_or(&0));
                let (o120, o60) = if *ph == "total" {
                    (if p99 > 8000 { "1" } else { "0" }, if p99 > 16000 { "1" } else { "0" })
                } else {
                    ("", "")
                };
                out.push_str(&format!(
                    "{n},{hd},{st},{ec},{ph},{},{p50},{p95},{p99},{mx},{o120},{o60}\n",
                    s.len()
                ));
            }
            out
        }
    }

    fn pct(sorted: &[u128], p: f64) -> u128 {
        if sorted.is_empty() { return 0; }
        let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    /// Least-squares slope of ln(y) on ln(x) over points with y > 0.
    fn loglog_slope(points: &[(f64, f64)]) -> f64 {
        let pts: Vec<(f64, f64)> =
            points.iter().filter(|(_, y)| *y > 0.0).map(|(x, y)| (x.ln(), y.ln())).collect();
        if pts.len() < 2 { return 0.0; }
        let n = pts.len() as f64;
        let sx: f64 = pts.iter().map(|p| p.0).sum();
        let sy: f64 = pts.iter().map(|p| p.1).sum();
        let sxx: f64 = pts.iter().map(|p| p.0 * p.0).sum();
        let sxy: f64 = pts.iter().map(|p| p.0 * p.1).sum();
        let denom = n * sxx - sx * sx;
        if denom.abs() < 1e-9 { return 0.0; }
        (n * sxy - sx * sy) / denom
    }
    fn slope_label(sl: f64) -> &'static str {
        if sl < 0.3 { "flat" } else if sl >= 0.7 { "linear" } else { "sub-linear" }
    }

    // -- burst drivers -------------------------------------------------------
    fn char_boundary(text: &str, mut b: usize) -> usize {
        while b < text.len() && !text.is_char_boundary(b) { b += 1; }
        b.min(text.len())
    }
    fn midpoint(text: &str) -> usize {
        let mut m = char_boundary(text, text.len() / 2);
        while m < text.len() && text.as_bytes()[m] == b'\n' {
            m = char_boundary(text, m + 1);
        }
        m
    }
    /// Move the caret to `byte` (clamped) and scroll it into view — an unmeasured
    /// seek (mirrors the direct-selection pattern in the seed journeys).
    fn seek(h: &mut Harness, byte: usize) {
        let len = h.editor.active().document.buffer.len();
        let b = byte.min(len);
        h.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(b);
        crate::nav::ensure_visible(&mut h.editor);
        h.render();
    }
    /// One matrix cell's fixed context, threaded through the burst drivers so
    /// each driver stays within the argument budget.
    struct Cell<'a> {
        n: usize,
        hd: usize,
        st: &'static str,
        ec: &'static str,      // edit_class tag for Input frames
        ec_tick: &'static str, // edit_class tag for the reconcile-tick frames
        text: &'a str,
    }

    /// Advance the clock to the next keystroke in 16ms frames, ticking each frame
    /// so the 150ms reconcile debounce fires BETWEEN keystrokes as in production.
    /// Tick-frame phase times are recorded under the reconcile-tick edit_class.
    fn cadence_gap(h: &mut Harness, s: &mut Samples, c: &Cell) {
        let mut elapsed = 0u64;
        while elapsed < CADENCE_GAP_MS {
            h.advance_ms(FRAME_MS);
            elapsed += FRAME_MS;
            let pt = h.step_timed(Msg::Tick);
            s.record(c.n, c.hd, c.st, c.ec_tick, &pt);
        }
    }

    fn drive_sustained(h: &mut Harness, s: &mut Samples, c: &Cell) {
        seek(h, midpoint(c.text));
        for i in 0..sustained_chars_for(c.n) {
            let ch = (b'a' + (i as u8 % 26)) as char;
            let pt = h.step_timed(Msg::Input(Event::Key(key_char(ch))));
            s.record(c.n, c.hd, c.st, c.ec, &pt);
            cadence_gap(h, s, c);
        }
    }
    fn drive_enter(h: &mut Harness, s: &mut Samples, c: &Cell, double: bool) {
        let reps = spread_reps_for(c.n);
        for k in 0..reps {
            let approx = ((k + 1) * c.text.len()) / (reps + 1);
            // paragraph end = the next newline at/after the spread point.
            let end = c.text[approx.min(c.text.len())..].find('\n').map(|o| approx + o).unwrap_or(c.text.len());
            seek(h, end);
            let pt = h.step_timed(press(KeyCode::Enter, KeyModifiers::NONE));
            s.record(c.n, c.hd, c.st, c.ec, &pt);
            if double {
                let pt2 = h.step_timed(press(KeyCode::Enter, KeyModifiers::NONE));
                s.record(c.n, c.hd, c.st, c.ec, &pt2);
            }
            cadence_gap(h, s, c);
        }
    }
    fn drive_heading(h: &mut Harness, s: &mut Samples, c: &Cell) {
        let reps = spread_reps_for(c.n);
        for k in 0..reps {
            let approx = ((k + 1) * c.text.len()) / (reps + 1);
            let ls = c.text[..approx.min(c.text.len())].rfind('\n').map(|o| o + 1).unwrap_or(0);
            seek(h, ls);
            for ch in "# Head".chars() {
                let pt = h.step_timed(Msg::Input(Event::Key(key_char(ch))));
                s.record(c.n, c.hd, c.st, c.ec, &pt);
                cadence_gap(h, s, c);
            }
        }
    }

    fn make_harness(text: &str, threaded: bool) -> Harness {
        if threaded {
            let (wtx, _wrx) = std::sync::mpsc::channel::<()>();
            Harness::new_with(text, None, VIEWPORT, BenchExecutor::Thread(ThreadExecutor::new(wtx)))
        } else {
            Harness::new(text, None, VIEWPORT)
        }
    }
    fn run_cell(s: &mut Samples, c: &Cell, kind: &str, threaded: bool) {
        for _ in 0..reps_for(c.n) {
            let mut h = make_harness(c.text, threaded);
            match kind {
                "sustained-char-burst" => drive_sustained(&mut h, s, c),
                "enter-at-paragraph-end" => drive_enter(&mut h, s, c, false),
                "double-enter" => drive_enter(&mut h, s, c, true),
                "heading-edit" => drive_heading(&mut h, s, c),
                other => panic!("unknown edit-class kind {other}"),
            }
        }
    }

    // -- diagnostics-landing probe (secondary) -------------------------------
    fn make_diags(text: &str) -> Vec<wordcartel_core::diagnostics::Diagnostic> {
        use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind};
        let mut v = Vec::new();
        let mut off = char_boundary(text, midpoint(text).saturating_sub(1000));
        for _ in 0..200 {
            let end = char_boundary(text, (off + 3).min(text.len()));
            if off >= end { break; }
            v.push(Diagnostic { range: off..end, kind: DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] });
            off = char_boundary(text, end + 7);
            if off >= text.len() { break; }
        }
        v
    }
    fn diagnostics_probe(s: &mut Samples) {
        for &n in N_VALUES {
            let text = gen_flat_prose(n);
            let hd = heading_count(&text);
            for _ in 0..reps_for(n) {
                let mut h = make_harness(&text, false);
                seek(&mut h, midpoint(&text));
                // Baseline render (no active diagnostics).
                let base = h.step_timed(Msg::Tick);
                s.push(n, hd, "flat-prose", "diagnostics-landing-baseline", "render", base.t_render.as_micros());
                // E7 T2: diagnostics compute/display are Review-gated now (draft-quiet); the
                // Harness seeds diag_cfg.enabled = false for hermeticity, so without both of
                // these the DiagnosticsDone landing below would measure the empty (un-placed)
                // path instead of the placed/painted path this probe intends (spec §7.4).
                h.editor.diag_cfg.enabled = true;
                h.editor.active_mut().view.mode = crate::editor::RenderMode::Review;
                // Inject diagnostics for the current version → placed render path arms.
                let bid = h.editor.active().id;
                let ver = h.version();
                let diags = make_diags(&text);
                let land = h.step_timed(Msg::DiagnosticsDone { buffer_id: bid, version: ver, diagnostics: diags });
                s.push(n, hd, "flat-prose", "diagnostics-landing", "render", land.t_render.as_micros());
                let nxt = h.step_timed(Msg::Tick);
                s.push(n, hd, "flat-prose", "diagnostics-landing-next", "render", nxt.t_render.as_micros());
            }
        }
    }

    // -- reporting -----------------------------------------------------------
    fn cell(s: &Samples, st: &str, ec: &str, ph: &str) -> String {
        let mut pts = Vec::new();
        for &n in N_VALUES {
            if let Some(p) = s.p99(n, st, ec, ph) { pts.push((n as f64, p as f64)); }
        }
        let sl = loglog_slope(&pts);
        format!("{sl:.2} {}", slope_label(sl))
    }

    fn build_slope_table(s: &Samples, caps: &[String]) -> String {
        let sts = structures();
        let mut md = String::new();
        md.push_str("# R1 typing-latency bench — slope table (log-log p99 vs N)\n\n");
        md.push_str("Viewport 100x40. Cursor seeks the doc MIDPOINT. Realistic ~180ms cadence; the 150ms reconcile debounce fires between keystrokes. `flat` = slope < 0.3; `linear` = slope >= 0.7.\n\n");

        // Headline: sustained-char-burst, phase x structure.
        md.push_str("## Headline — sustained-char-burst (Input frames)\n\n");
        md.push_str("| phase |");
        for (name, _) in &sts { md.push_str(&format!(" {name} |")); }
        md.push('\n');
        md.push_str("|-------|");
        for _ in &sts { md.push_str("------|"); }
        md.push('\n');
        for ph in PHASES {
            md.push_str(&format!("| {ph} |"));
            for (name, _) in &sts {
                md.push_str(&format!(" {} |", cell(s, name, "sustained-char-burst", ph)));
            }
            md.push('\n');
        }
        md.push_str("\nOffending sites when linear: `heading_starts` = derive.rs `rebuild_downstream` `outline::heading_starts` (whole-doc block-tree walk, gated on `blocks_generation` which bumps every edit); `foldview` = `active_fold_view` -> editor.rs:554 -> `fold::FoldView::compute` (whole-doc walk, same defeated gate); `parse` = derive.rs `rebuild` parse phase (incremental widen / full_parse). Positive control: `layout_fill` (derive.rs visible-line loop, O(visible)) and `render` MUST be flat.\n\n");

        // parse slope per structure under enter / double-enter (assertion 3).
        md.push_str("## parse slope by structure (widen / gap-materialization edit classes)\n\n");
        md.push_str("| structure | sustained-char-burst | enter-at-paragraph-end | double-enter | heading-edit |\n");
        md.push_str("|-----------|----------------------|------------------------|--------------|--------------|\n");
        for (name, _) in &sts {
            md.push_str(&format!("| {name} | {} | {} | {} | {} |\n",
                cell(s, name, "sustained-char-burst", "parse"),
                cell(s, name, "enter-at-paragraph-end", "parse"),
                cell(s, name, "double-enter", "parse"),
                cell(s, name, "heading-edit", "parse")));
        }
        md.push('\n');

        // Reconcile-tick (threaded executor): p99/max of the merge-landing Tick vs N.
        md.push_str("## Reconcile-tick hitch — threaded executor (total per Tick, us)\n\n");
        md.push_str("edit_class = sustained-char-burst[threaded]+reconcile-tick. The merge (tree-eq compare + set_blocks + downstream rebuild) lands on a Tick between keystrokes.\n\n");
        md.push_str("| structure |");
        for &n in N_VALUES { md.push_str(&format!(" {} p99/max |", nk(n))); }
        md.push('\n');
        md.push_str("|-----------|");
        for _ in N_VALUES { md.push_str("-----------|"); }
        md.push('\n');
        for (name, _) in &sts {
            md.push_str(&format!("| {name} |"));
            for &n in N_VALUES {
                match s.stat(n, name, "sustained-char-burst[threaded]+reconcile-tick", "total") {
                    Some((_, _, p99, mx)) => md.push_str(&format!(" {p99}/{mx} |")),
                    None => md.push_str(" -/- |"),
                }
            }
            md.push('\n');
        }
        md.push_str("\nInline (upper-bound) reconcile-tick totals for comparison (full_parse runs on the Tick's reduce):\n\n");
        md.push_str("| structure |");
        for &n in N_VALUES { md.push_str(&format!(" {} p99/max |", nk(n))); }
        md.push('\n');
        md.push_str("|-----------|");
        for _ in N_VALUES { md.push_str("-----------|"); }
        md.push('\n');
        for (name, _) in &sts {
            md.push_str(&format!("| {name} |"));
            for &n in N_VALUES {
                match s.stat(n, name, "sustained-char-burst+reconcile-tick", "total") {
                    Some((_, _, p99, mx)) => md.push_str(&format!(" {p99}/{mx} |")),
                    None => md.push_str(" -/- |"),
                }
            }
            md.push('\n');
        }

        // Diagnostics-landing render deltas.
        md.push_str("\n## Diagnostics-landing render (flat-prose, us p99)\n\n");
        md.push_str("| N | baseline render | landing render | next render |\n");
        md.push_str("|---|-----------------|----------------|-------------|\n");
        for &n in N_VALUES {
            let b = s.p99(n, "flat-prose", "diagnostics-landing-baseline", "render").unwrap_or(0);
            let l = s.p99(n, "flat-prose", "diagnostics-landing", "render").unwrap_or(0);
            let x = s.p99(n, "flat-prose", "diagnostics-landing-next", "render").unwrap_or(0);
            md.push_str(&format!("| {} | {b} | {l} | {x} |\n", nk(n)));
        }

        md.push_str("\n## Caps / scoping (no silent truncation)\n\n");
        for c in caps { md.push_str(&format!("- {c}\n")); }
        md
    }
    fn nk(n: usize) -> String {
        if n >= 1_000_000 { format!("{}M", n / 1_000_000) } else { format!("{}K", n / 1_000) }
    }

    fn write_outputs(csv: &str, slopes: &str) {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".superpowers")
            .join("sdd");
        let _ = std::fs::create_dir_all(&base);
        let _ = std::fs::write(base.join("r1-bench.csv"), csv);
        let _ = std::fs::write(base.join("r1-bench-slopes.md"), slopes);
    }

    // Release-only bench — Debug Instant timing is meaningless, and running it in the
    // default `cargo test` suite adds minutes and overwrites the recorded release CSV.
    // Run explicitly: `cargo test -p wordcartel --release e2e_bench -- --ignored --nocapture --test-threads=1`.
    #[test]
    #[ignore = "release-only bench; run with --release --ignored (see comment)"]
    #[allow(clippy::print_stdout, clippy::print_stderr)] // bench harness prints its CSV / slope report by design
    fn r1_typing_latency_bench() {
        let mut s = Samples::default();
        let sts = structures();
        let edit_classes: &[(&'static str, &'static str, &'static str)] = &[
            ("sustained-char-burst", "sustained-char-burst", "sustained-char-burst+reconcile-tick"),
            ("enter-at-paragraph-end", "enter-at-paragraph-end", "enter-at-paragraph-end+reconcile-tick"),
            ("double-enter", "double-enter", "double-enter+reconcile-tick"),
            ("heading-edit", "heading-edit", "heading-edit+reconcile-tick"),
        ];

        // Inline matrix — full cross product (derive phases are executor-independent;
        // the inline reconcile-tick row is the upper bound with full_parse on-thread).
        for &n in N_VALUES {
            for (st, gen) in &sts {
                let text = gen(n);
                let hd = heading_count(&text);
                eprintln!("[bench] inline  N={:>8} struct={:<18} bytes={} headings={}", n, st, text.len(), hd);
                for (kind, ec, ec_tick) in edit_classes {
                    let c = Cell { n, hd, st, ec, ec_tick, text: &text };
                    run_cell(&mut s, &c, kind, false);
                }
            }
        }
        // Threaded matrix — sustained-char-burst only (the true off-thread reconcile
        // merge hitch); distinct edit_class so it does not collide with inline rows.
        for &n in N_VALUES {
            for (st, gen) in &sts {
                let text = gen(n);
                let hd = heading_count(&text);
                eprintln!("[bench] thread  N={:>8} struct={:<18} bytes={}", n, st, text.len());
                let c = Cell {
                    n, hd, st,
                    ec: "sustained-char-burst[threaded]",
                    ec_tick: "sustained-char-burst[threaded]+reconcile-tick",
                    text: &text,
                };
                run_cell(&mut s, &c, "sustained-char-burst", true);
            }
        }
        // Diagnostics-landing probe.
        eprintln!("[bench] diagnostics-landing probe");
        diagnostics_probe(&mut s);

        let caps = vec![
            "reps per N: N<=16K -> 5, 64K -> 4, 256K -> 3, 1M -> 2".to_string(),
            "spread-edit reps (enter/double-enter/heading-edit): N>=256K -> 10, else 20".to_string(),
            "sustained-char-burst chars: N=1M -> 30, else 40".to_string(),
            "threaded-executor matrix limited to sustained-char-burst (reconcile-tick focus); all other edit classes measured inline only".to_string(),
            "diagnostics-landing probe: flat-prose only, 200 spelling diagnostics near the midpoint".to_string(),
        ];

        let csv = s.to_csv();
        let slopes = build_slope_table(&s, &caps);

        // Report-only invariant checks (NEVER panic — we WANT to see the slopes).
        let hs = loglog_from(&s, "flat-prose", "sustained-char-burst", "heading_starts");
        let fv = loglog_from(&s, "flat-prose", "sustained-char-burst", "foldview");
        let lf = loglog_from(&s, "flat-prose", "sustained-char-burst", "layout_fill");
        let rd = loglog_from(&s, "flat-prose", "sustained-char-burst", "render");
        eprintln!("[bench] HEADLINE flat-prose sustained-char-burst slopes:");
        eprintln!("  heading_starts = {hs:.2} ({})", slope_label(hs));
        eprintln!("  foldview       = {fv:.2} ({})", slope_label(fv));
        eprintln!("  layout_fill    = {lf:.2} ({})  [positive control — must be flat]", slope_label(lf));
        eprintln!("  render         = {rd:.2} ({})  [positive control — must be flat]", slope_label(rd));
        if lf >= 0.3 || rd >= 0.3 {
            eprintln!("[bench] WARNING: positive control NOT flat (layout_fill={lf:.2}, render={rd:.2}) — harness may be mis-measuring.");
        }

        println!("{csv}");
        println!("{slopes}");
        write_outputs(&csv, &slopes);
    }

    fn loglog_from(s: &Samples, st: &str, ec: &str, ph: &str) -> f64 {
        let mut pts = Vec::new();
        for &n in N_VALUES {
            if let Some(p) = s.p99(n, st, ec, ph) { pts.push((n as f64, p as f64)); }
        }
        loglog_slope(&pts)
    }
}
