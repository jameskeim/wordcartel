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
use crate::prompt::PromptAction;
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
    if is_stale(r.kind, r.version, editor.active().document.version) {
        return; // version moved on: discard, don't rebase
    }
    let (kind, version) = (r.kind, r.version);
    (r.merge)(editor);
    // Save & quit: exit once the awaited save version lands clean.
    if kind == crate::jobs::JobKind::Save
        && editor.quit_after_save == Some(version)
        && editor.active().document.saved_version == Some(version)
    {
        editor.quit = true;
    }
}

/// Execute the action chosen in a modal prompt, then clear the prompt.
pub fn resolve_prompt(action: PromptAction, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock) {
    match action {
        PromptAction::Cancel => {}
        PromptAction::QuitAnyway => { editor.quit = true; }
        PromptAction::SaveAndQuit => {
            let v = editor.active().document.version;
            editor.prompt = None; // dismiss the quit-confirm modal first
            { let mut ctx = Ctx { editor, clock, executor: ex }; crate::save::dispatch_save(&mut ctx); }
            // Arm quit-after-save ONLY if a save job was actually dispatched.
            // dispatch_save dispatches nothing when there is no path (status set)
            // or when it raised an external-mod modal (editor.prompt now Some) —
            // in those cases abort the quit and let the user resolve (Codex #4).
            if editor.active().document.path.is_some() && editor.prompt.is_none() {
                editor.quit_after_save = Some(v);
                editor.quit_after_save_at = Some(clock.now_ms());
            }
            return; // prompt handled above; must NOT clear an external-mod modal
        }
        PromptAction::Reload => crate::save::reload_from_disk(editor),
        PromptAction::Overwrite => {
            let mut ctx = Ctx { editor, clock, executor: ex };
            crate::save::overwrite_save(&mut ctx);
        }
        PromptAction::Recover => {
            if let Some(body) = editor.active_mut().pending_swap_body.take() {
                // Load the swap content into the buffer, mark dirty (saved_version
                // stays None), keep the original path.
                crate::save::load_recovered(editor, &body);
                // Delete the orphan swap so next launch doesn't re-prompt.
                // (Recovered work is now in the live buffer and will be swapped
                // under the new pid.)
                if let Some(p) = editor.active_mut().pending_swap_path.take() {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        PromptAction::DiscardSwap => {
            if let Some(p) = editor.active_mut().pending_swap_path.take() {
                let _ = std::fs::remove_file(p);
            } else {
                crate::swap::delete(editor.active().document.path.as_deref());
            }
        }
        PromptAction::OpenOriginal => {
            editor.active_mut().pending_swap_body = None;
            editor.active_mut().pending_swap_path = None;
        }
    }
    editor.prompt = None;
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    ex: &dyn Executor,
    clock: &dyn Clock,
) -> bool {
    // Active modal intercepts KEY INPUT only (§5.3). Background results and ticks
    // must still be processed — a JobDone arriving while a modal is up (e.g. an
    // in-flight save completing during the quit-confirm prompt) must not be
    // dropped, or save&quit would hang waiting for a result it already discarded.
    if editor.prompt.is_some() {
        match msg {
            Msg::Input(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
                if key.code == crossterm::event::KeyCode::Esc {
                    editor.prompt = None; // Esc cancels any prompt
                } else if let crossterm::event::KeyCode::Char(ch) = key.code {
                    if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                        resolve_prompt(action, editor, ex, clock);
                    }
                }
            }
            // Merge a directly-delivered background result even under a modal.
            Msg::JobDone(r) => apply_result(r, editor),
            // Resize/Tick/other input: ignored for the modal, but results still drain below.
            _ => {}
        }
        // Always drain ready results (merges the awaited save&quit result).
        for r in ex.drain() { apply_result(r, editor); }
        return !editor.quit;
    }

    let before = editor.active().document.version;
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
            editor.active_mut().view.area = (w, h);
            derive::rebuild(editor);
        }
        Msg::Input(_) => {}
        Msg::JobDone(r) => apply_result(r, editor),
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.active().document.dirty()
                && !editor.active().swap_in_flight
                && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
            {
                editor.active_mut().swap_in_flight = true;
                let mut ctx = Ctx { editor, clock, executor: ex };
                crate::swap::dispatch_swap_write(&mut ctx);
            }
        }
    }
    if editor.active().document.version != before {
        editor.active_mut().last_edit_at = Some(clock.now_ms());
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

    // Recovery-on-open (§5.1).
    // Named files: use assess() with content-hash comparison.
    // Scratch buffers: their swap is pid-keyed, so look for an orphan from a
    // dead previous session (pre-merge blocker #1).
    if editor.active().document.path.is_some() {
        // Read F's current bytes once for the predicate.
        let file_bytes = editor.active().document.path.as_deref().and_then(|p| std::fs::read(p).ok());
        match crate::swap::assess(editor.active().document.path.as_deref(), file_bytes.as_deref()) {
            crate::swap::RecoveryDecision::OpenNormally => {}
            crate::swap::RecoveryDecision::DiscardSilently => {
                crate::swap::delete(editor.active().document.path.as_deref());
            }
            crate::swap::RecoveryDecision::Prompt(_h, body) => {
                editor.active_mut().pending_swap_body = Some(body);
                editor.prompt = Some(crate::prompt::Prompt::swap_recovery());
                editor.status = "Recovery file found".into();
            }
        }
    } else if let Some((sp, _header, body)) = crate::swap::find_orphan_scratch_swap() {
        editor.active_mut().pending_swap_body = Some(body);
        editor.active_mut().pending_swap_path = Some(sp);
        editor.prompt = Some(crate::prompt::Prompt::swap_recovery());
        editor.status = "Recovery file found".into();
    }

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
    const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;

    guard.terminal().draw(|f| render::render(f, &editor))?;
    loop {
        let now = clock.now_ms();
        // Bounded save&quit: if waiting for an in-flight save to complete and
        // 5 s have elapsed since the last edit, re-raise the quit-confirm modal.
        if let Some(_v) = editor.quit_after_save {
            let waited = now.saturating_sub(editor.quit_after_save_at.unwrap_or(now));
            if waited > SAVE_QUIT_TIMEOUT_MS {
                editor.quit_after_save = None;
                editor.quit_after_save_at = None;
                editor.prompt = Some(crate::prompt::Prompt::quit_confirm());
                editor.status = "Save still running — choose again".into();
            }
        }
        let swap_deadline = crate::swap::next_deadline_ms(now, editor.active().last_edit_at, editor.active().last_swap_at);
        let sq_deadline = editor.quit_after_save_at.map(|t| t.saturating_add(SAVE_QUIT_TIMEOUT_MS));
        let deadline = match (swap_deadline, sq_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let timeout = deadline
            .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
            .unwrap_or(std::time::Duration::from_secs(3600));
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
    // is the "never lose work" behavior). The 5 s save&quit guard above bounds the wait.
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

    // -------------------------------------------------------------------------
    // Brief's required failing test (Task 12 step 1)
    // -------------------------------------------------------------------------

    /// Feed "hi" then Ctrl+Q (modal) then 'q' (QuitAnyway); confirm the buffer holds "hi\n" and quit.
    #[test]
    fn step_processes_typing_and_quit() {
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → modal up, NOT quit yet
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(ctrl_q), &mut e, &reg, &ex, &clk);
        assert!(e.prompt.is_some(), "dirty quit must raise modal");
        assert!(!e.quit);
        // Press 'q' → routed to QuitAnyway via the modal.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &ex, &clk);
        assert!(e.quit);
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
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
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
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
        e.active_mut().document.version = 1;            // dirty (saved_version=Some(0))
        e.active_mut().last_edit_at = Some(0);
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        // Clock past the idle threshold.
        struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let clk = C(crate::swap::T_IDLE_MS + 5);
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &ex, &clk);
        assert!(e.active().last_swap_at.is_some(), "an idle Tick on a dirty buffer writes a swap");
        let sp = crate::swap::swap_path(Some(&doc_path)).unwrap();
        assert!(sp.exists());
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }

    #[test]
    fn quit_with_unsaved_raises_modal_then_quit_anyway_exits() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // First Ctrl+Q → modal up, not quit.
        crate::app::reduce(crate::app::Msg::Input(ctrl_q.clone()), &mut e, &reg, &ex, &clk);
        assert!(e.prompt.is_some() && !e.quit);
        // Press 'q' → routed to QuitAnyway.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &ex, &clk);
        assert!(e.quit, "Quit anyway exits");
        assert!(e.prompt.is_none(), "prompt cleared");
    }

    #[test]
    fn save_and_quit_sets_quit_after_save_and_exits_on_matching_result() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        use crate::prompt::PromptAction;
        let p = std::env::temp_dir().join(format!("wc-savequit-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk);
        assert_eq!(e.quit_after_save, Some(1));
        assert!(!e.quit, "not yet — waiting for the save result");
        for r in ex.drain() { crate::app::apply_result(r, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_and_quit_on_unnamed_buffer_does_not_arm_quit_after_save() {
        // No path → dispatch_save dispatches NO job. quit_after_save must stay None,
        // or the app would wait forever for a result that never comes (Codex #4).
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk);
        assert_eq!(e.quit_after_save, None, "no job dispatched → do not arm quit-after-save");
        assert!(!e.quit);
    }

    #[test]
    fn apply_result_merges_fresh_and_drops_stale() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        e.active_mut().document.version = 5;
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
