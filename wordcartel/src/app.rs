// wordcartel/src/app.rs — testable `step` + the real crossterm `run` loop.
//
// Design: terminal IO lives ONLY in `run`; `step` is pure and unit-testable.
// The real loop calls `step` then draws — `step` never touches the terminal.

use crossterm::event::Event;
#[cfg(test)]
use crossterm::event::KeyEvent;
use std::path::PathBuf;

use crate::{commands, derive, editor::Editor, file, input, render, term};
use crate::jobs::{is_stale, Executor, JobResult};
use crate::registry::{Ctx, Registry};
use crate::input::KeyAction;
use wordcartel_core::history::Clock;

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Msg, apply_result, reduce — unified message type and reducer
// ---------------------------------------------------------------------------

pub enum Msg {
    Input(Event),
    JobDone(JobResult),
    Tick,
}

/// Merge a finished job's effect on the foreground, honoring staleness (§10.3).
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if is_stale(r.kind, r.version, editor.document.version) {
        return; // version moved on: discard, don't rebase
    }
    (r.merge)(editor);
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    ex: &dyn Executor,
    clock: &dyn Clock,
) -> bool {
    let before = editor.document.version;
    match msg {
        Msg::Input(Event::Key(key)) => {
            match input::key_to_command_id(key) {
                Some(KeyAction::Id(id)) => {
                    let mut ctx = Ctx { editor, clock, executor: ex };
                    reg.dispatch(id, &mut ctx);
                }
                Some(KeyAction::Insert(c)) => {
                    commands::run(commands::Command::InsertChar(c), editor, clock);
                }
                None => {}
            }
        }
        Msg::Input(Event::Resize(w, h)) => {
            editor.view.area = (w, h);
            derive::rebuild(editor);
        }
        Msg::Input(_) => {}
        Msg::JobDone(r) => apply_result(r, editor),
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.document.dirty()
                && crate::swap::due(now, editor.last_edit_at, editor.last_swap_at)
            {
                let mut ctx = Ctx { editor, clock, executor: ex };
                crate::swap::dispatch_swap_write(&mut ctx);
                // Provisionally mark; the merge confirms with the same ts.
                ctx.editor.last_swap_at = Some(now);
            }
        }
    }
    if editor.document.version != before {
        editor.last_edit_at = Some(clock.now_ms());
    }
    // Fold any other results that became ready while handling this message.
    for r in ex.drain() {
        apply_result(r, editor);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

/// Legacy synchronous dispatch path retained for its existing tests; production
/// uses `reduce` + the registry.
///
/// Translate one key event, run the resulting command (if any), then return
/// `true` while the app should keep running (`false` → caller should exit).
///
/// All editor mutation goes through `commands::run`; this function adds no
/// logic of its own beyond the translation.
#[cfg(test)]
pub fn step(editor: &mut Editor, key: KeyEvent, clock: &dyn Clock) -> bool {
    if let Some(cmd) = input::key_to_command(key) {
        commands::run(cmd, editor, clock);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// SystemClock — used only by the real `run` loop, never by unit tests
// ---------------------------------------------------------------------------

struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// run — the real terminal loop; terminal IO lives entirely here
// ---------------------------------------------------------------------------

/// Open `path` (or scratch buffer), install the terminal guard, then loop:
/// draw → read event → step → repeat until `editor.quit`.
pub fn run(path: Option<PathBuf>) -> std::io::Result<()> {
    // Install the panic hook (once) so the terminal is restored on panic.
    term::install_panic_hook();

    // Determine the initial terminal size.
    let (cols, rows) = crossterm::terminal::size()?;
    let area = (cols, rows);

    // Open the file and branch on errors per §C5.
    let editor = match path.as_deref() {
        None => {
            // No path given: scratch buffer.
            Editor::new_from_text("\n", None, area)
        }
        Some(p) => match file::open(p) {
            Ok(text) => {
                Editor::new_from_text(&text, Some(p.to_path_buf()), area)
            }
            Err(file::OpenError::NotFound(_)) => {
                // New file: empty buffer NAMED with the path; first save creates it.
                let mut e = Editor::new_from_text("\n", Some(p.to_path_buf()), area);
                e.status = "new file".to_string();
                e
            }
            Err(e @ file::OpenError::Binary(_))
            | Err(e @ file::OpenError::Permission(_))
            | Err(e @ file::OpenError::IsDir(_)) => {
                // Rejected target: UNNAMED buffer so a save can't clobber it.
                let mut ed = Editor::new_from_text("\n", None, area);
                ed.status = e.to_string();
                ed
            }
            Err(e @ file::OpenError::Io(_)) => {
                // Generic IO error: also unnamed.
                let mut ed = Editor::new_from_text("\n", None, area);
                ed.status = e.to_string();
                ed
            }
        },
    };

    let mut editor = editor;

    // Install the terminal guard: enable raw mode + enter alternate screen.
    let mut guard = term::TerminalGuard::new()?;

    // Initial derive so the first draw has up-to-date layouts.
    derive::rebuild(&mut editor);

    let reg = Registry::builtins();
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let executor = crate::jobs::ThreadExecutor::new(wake_tx);

    // Worker → loop wake relay: each result nudges the loop to drain. reduce()'s
    // trailing ex.drain() does the actual merge, so Msg::Tick is the nudge.
    {
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            while wake_rx.recv().is_ok() {
                if msg_tx.send(Msg::Tick).is_err() { break; }
            }
        });
    }

    // Input thread: blocks on read(); forwards every event. Detached — dies with
    // the process on quit (read() can't be interrupted portably).
    {
        let msg_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("wcartel-input".into())
            .spawn(move || {
                while let Ok(ev) = crossterm::event::read() {
                    if msg_tx.send(Msg::Input(ev)).is_err() { break; }
                }
            })
            .expect("spawn input thread");
    }

    let clock = SystemClock;
    guard.terminal().draw(|f| render::render(f, &editor))?;
    loop {
        let now = clock.now_ms();
        let timeout = crate::swap::next_deadline_ms(now, editor.last_edit_at, editor.last_swap_at)
            .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
            .unwrap_or(std::time::Duration::from_secs(3600)); // idle: effectively block
        let msg = match msg_rx.recv_timeout(timeout) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Msg::Tick,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let keep = reduce(msg, &mut editor, &reg, &executor, &clock);
        guard.terminal().draw(|f| render::render(f, &editor))?;
        if !keep { break; }
    }

    // Restore the terminal BEFORE the executor drops: ThreadExecutor::drop joins
    // the worker, which may still be completing an in-flight save_atomic on a slow
    // filesystem. Dropping the guard first guarantees the user gets their terminal
    // back immediately; we still join (don't abandon an in-flight atomic save — that
    // is the "never lose work" behavior). A BOUNDED save&quit wait lands in Effort 4b-2.
    drop(guard);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase) before any implementation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use crate::editor::Editor;
    use wordcartel_core::history::Clock;
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    struct TestClock(u64);
    impl Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0 }
    }

    /// Build a KeyEvent for a printable character (no modifiers, Press).
    fn key_char(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Build a KeyEvent for Ctrl+<char> (Press).
    fn key_ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // -------------------------------------------------------------------------
    // Brief's required failing test (Task 12 step 1)
    // -------------------------------------------------------------------------

    /// Feed "hi" then double Ctrl+Q; confirm the buffer holds "hi\n" and quit.
    #[test]
    fn step_processes_typing_and_quit() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let clk = TestClock(0);
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → pending confirm, NOT quit yet
        crate::app::step(&mut e, key_ctrl('q'), &clk);
        assert!(!e.quit);
        // Second Ctrl+Q: force quit
        crate::app::step(&mut e, key_ctrl('q'), &clk);
        assert!(e.quit);
        assert_eq!(e.document.buffer.to_string(), "hi\n");
    }

    // -------------------------------------------------------------------------
    // key_to_command mapping tests
    // -------------------------------------------------------------------------

    /// Ctrl+S maps to Command::Save.
    #[test]
    fn key_to_command_ctrl_s_is_save() {
        use crate::commands::Command;
        let k = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(crate::input::key_to_command(k), Some(Command::Save)));
    }

    /// Shift+Right maps to Move { dir: Right, extend: true }.
    #[test]
    fn key_to_command_shift_right_extends() {
        use crate::commands::{Command, Dir};
        let k = KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command(k),
            Some(Command::Move { dir: Dir::Right, extend: true })
        ));
    }

    /// A printable char maps to InsertChar.
    #[test]
    fn key_to_command_printable_is_insert_char() {
        use crate::commands::Command;
        let k = key_char('A');
        assert!(matches!(crate::input::key_to_command(k), Some(Command::InsertChar('A'))));
    }

    /// An unmapped key (F5) returns None.
    #[test]
    fn key_to_command_unmapped_is_none() {
        let k = KeyEvent {
            code: KeyCode::F(5),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    /// Release events return None (double-input guard).
    #[test]
    fn key_to_command_release_is_none() {
        let k = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    #[test]
    fn reduce_handles_typing_via_registry() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        for c in "hi".chars() {
            let ev = Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE });
            assert!(crate::app::reduce(crate::app::Msg::Input(ev), &mut e, &reg, &ex, &clk));
        }
        assert_eq!(e.document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn tick_writes_swap_when_idle_elapsed_and_dirty() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let doc_path = std::env::temp_dir().join(format!(
            "wc-tick-swap-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let mut e = Editor::new_from_text("\n", Some(doc_path.clone()), (80, 24));
        e.document.version = 1;            // dirty (saved_version=Some(0))
        e.last_edit_at = Some(0);
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        // Clock past the idle threshold.
        struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let clk = C(crate::swap::T_IDLE_MS + 5);
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &ex, &clk);
        assert!(e.last_swap_at.is_some(), "an idle Tick on a dirty buffer writes a swap");
        let sp = crate::swap::swap_path(Some(&doc_path)).unwrap();
        assert!(sp.exists());
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }

    #[test]
    fn apply_result_merges_fresh_and_drops_stale() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        e.document.version = 5;
        // Fresh one-shot (Save is never stale): merges.
        crate::app::apply_result(JobResult { version: 3, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "saved".into()) }, &mut e);
        assert_eq!(e.status, "saved");
        // Stale coalescible: dropped.
        crate::app::apply_result(JobResult { version: 3, kind: JobKind::CoalesceProbe,
            merge: Box::new(|ed: &mut Editor| ed.status = "STALE".into()) }, &mut e);
        assert_eq!(e.status, "saved", "stale coalescible result must be dropped");
    }
}
