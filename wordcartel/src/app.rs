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
    FilterDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        range: std::ops::Range<usize>,
        cursor: usize,
        disposition: crate::filter::Disposition,
        outcome: crate::filter::RunResult,
    },
    ExportDone {
        buffer_id: crate::editor::BufferId,
        target: std::path::PathBuf,
        result: Result<crate::export::ExportResult, crate::filter::FilterError>,
        /// True when the user explicitly confirmed overwriting an existing
        /// target via the OverwriteExport prompt.  False when export was
        /// dispatched because the target did not exist at check time — in which
        /// case finalization must refuse to clobber a target that appeared in
        /// the meantime (TOCTOU guard; Codex pre-merge gate).
        overwrite_confirmed: bool,
    },
    Tick,
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::Input(_) => f.write_str("Input(..)"),
            Msg::JobDone(_) => f.write_str("JobDone(..)"),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => f
                .debug_struct("FilterDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("range", range)
                .field("cursor", cursor)
                .field("disposition", disposition)
                .field("outcome", outcome)
                .finish(),
            Msg::ExportDone { buffer_id, target, .. } => f
                .debug_struct("ExportDone")
                .field("buffer_id", buffer_id)
                .field("target", target)
                .finish(),
            Msg::Tick => f.write_str("Tick"),
        }
    }
}

/// Merge a finished job's effect on the foreground, honoring staleness (§10.3).
pub fn apply_result(r: JobResult, editor: &mut Editor) {
    if is_stale(&r, editor) {
        return; // buffer-local merge for a closed buffer, or a stale coalescible
    }
    let (kind, version, buffer_id, class) = (r.kind, r.version, r.buffer_id, r.class);
    // Mechanical-routing assertion (spec §3.4): a buffer-local merge must resolve
    // to a live buffer here; durability merges may target a now-missing buffer.
    debug_assert!(
        class == crate::jobs::ResultClass::Durability || editor.by_id(buffer_id).is_some(),
        "buffer-local result for a missing buffer slipped past is_stale"
    );
    (r.merge)(editor);
    // Save & quit: exit once the awaited save version lands clean for that buffer.
    if kind == crate::jobs::JobKind::Save
        && editor.quit_after_save == Some(version)
        && editor.by_id(buffer_id).map(|b| b.document.saved_version) == Some(Some(version))
    {
        editor.quit = true;
    }
}

fn apply_filter_done(
    editor: &mut crate::editor::Editor,
    buffer_id: crate::editor::BufferId,
    version: u64,
    range: std::ops::Range<usize>,
    cursor: usize,
    disposition: crate::filter::Disposition,
    outcome: crate::filter::RunResult,
    clock: &dyn wordcartel_core::history::Clock,
) {
    // Clears the single in-flight slot unconditionally. Correct under the current
    // single-in-flight invariant (one filter at a time across the whole editor);
    // would need per-buffer tracking if multi-buffer concurrent filters land (Effort 6).
    editor.filter_in_flight = None;
    let stale = editor.by_id(buffer_id).map(|b| b.document.version) != Some(version);
    match outcome {
        _ if stale => {
            editor.status = "filter discarded - buffer changed".into();
        }
        crate::filter::RunResult::Err(err) => {
            editor.status = crate::filter::describe_error(&err);
        }
        crate::filter::RunResult::Stdout(text) => {
            let apply_result = if let Some(b) = editor.by_id_mut(buffer_id) {
                let doc_len = b.document.buffer.len();
                let (from, to, at) = match disposition {
                    crate::filter::Disposition::Filter => (range.start, range.end, range.start),
                    crate::filter::Disposition::Insert => (cursor, cursor, cursor),
                };
                let (cs, edit) = crate::commands::build_range_replace(from, to, &text, doc_len);
                let txn = wordcartel_core::history::Transaction::new(cs)
                    .with_selection(wordcartel_core::selection::Selection::single(at + text.len()));
                b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
                true
            } else {
                false
            };
            if apply_result {
                crate::derive::rebuild(editor);
                crate::nav::ensure_visible(editor);
                editor.status = "filter applied".into();
            }
        }
    }
}

fn apply_export_done(
    editor: &mut crate::editor::Editor,
    target: std::path::PathBuf,
    result: Result<crate::export::ExportResult, crate::filter::FilterError>,
    overwrite_confirmed: bool,
) {
    // TOCTOU guard (Codex pre-merge gate): run_export only prompts for overwrite
    // if the target existed at check time.  When it did not (overwrite_confirmed
    // == false) but the target has since appeared, refuse to clobber it silently
    // — the user never agreed to replace it.  (A pre-existing target always went
    // through the OverwriteExport prompt, so overwrite_confirmed is true there.)
    // The residual check-to-write window is microseconds vs. the whole pandoc
    // run; an unsafe-free atomic no-replace rename is unavailable under
    // #![forbid(unsafe_code)].
    if !overwrite_confirmed && target.exists() {
        if let Ok(crate::export::ExportResult::TempReady(tmp)) = &result {
            let _ = std::fs::remove_file(tmp);
        }
        editor.status = format!(
            "export target {} appeared — re-run export to overwrite",
            target.display()
        );
        return;
    }
    match result {
        Ok(crate::export::ExportResult::Bytes(bytes)) => {
            match file::save_atomic_bytes(&target, &bytes) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.status = status;
                }
                Err(e) => {
                    editor.status = format!("export write failed: {e}");
                }
            }
        }
        Ok(crate::export::ExportResult::TempReady(tmp)) => {
            match std::fs::rename(&tmp, &target) {
                Ok(()) => {
                    let status = format!("exported {}", target.display());
                    editor.status = status;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    editor.status = format!("export rename failed: {e}");
                }
            }
        }
        Err(e) => {
            editor.status = crate::filter::describe_error(&e);
        }
    }
}

/// Execute the action chosen in a modal prompt, then clear the prompt.
pub fn resolve_prompt(
    action: PromptAction,
    editor: &mut Editor,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    match action {
        PromptAction::Cancel => {
            editor.pending_export = None;
        }
        PromptAction::QuitAnyway => { editor.quit = true; }
        PromptAction::SaveAndQuit => {
            let v = editor.active().document.version;
            editor.prompt = None; // dismiss the quit-confirm modal first
            { let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() }; crate::save::dispatch_save(&mut ctx); }
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
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::overwrite_save(&mut ctx);
        }
        PromptAction::Recover => {
            // Capture body + orphan path BEFORE load_recovered, which replaces the
            // whole active Buffer and would reset pending_swap_path to None (4r moved
            // these fields onto Buffer). Then clean up the orphan after loading.
            let staged = {
                let b = editor.active_mut();
                b.pending_swap_body
                    .take()
                    .map(|body| (body, b.pending_swap_path.take()))
            };
            if let Some((body, orphan)) = staged {
                crate::save::load_recovered(editor, &body);
                if let Some(p) = orphan {
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
        PromptAction::OverwriteExport => {
            if let Some(pe) = editor.pending_export.take() {
                // User explicitly confirmed clobbering the existing target.
                crate::export::do_export(editor, &pe.ext, &pe.target, msg_tx, true);
            }
        }
        PromptAction::Transform(kind) => {
            crate::transform::dispatch_transform(editor, kind, clock, msg_tx);
        }
    }
    editor.prompt = None;
}

/// Submit a minibuffer line as a filter command.
///
/// Splits the line on whitespace to build the argv (no shell, no quoting —
/// `shell: false` is the security default; shell invocation is opt-in only).
/// An empty line sets a status message and returns without dispatching.
fn submit_filter_line(
    editor: &mut Editor,
    line: &str,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) {
    let argv: Vec<String> = line.split_whitespace().map(String::from).collect();
    if argv.is_empty() {
        editor.status = "filter: no command given".into();
        return;
    }
    let spec = crate::filter::FilterSpec {
        argv,
        shell: false,
        disposition: crate::filter::Disposition::Filter,
        input: crate::filter::Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10),
        max_output: 1 << 20,
    };
    crate::filter::dispatch_filter(editor, spec, msg_tx.clone());
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
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
                    editor.pending_export = None;
                } else if let crossterm::event::KeyCode::Char(ch) = key.code {
                    if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                        resolve_prompt(action, editor, ex, clock, msg_tx);
                    }
                }
            }
            // Merge a directly-delivered background result even under a modal.
            Msg::JobDone(r) => apply_result(r, editor),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
                apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
            }
            Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
                apply_export_done(editor, target, result, overwrite_confirmed);
            }
            // Resize/Tick/other input: ignored for the modal, but results still drain below.
            _ => {}
        }
        // Always drain ready results (merges the awaited save&quit result).
        for r in ex.drain() { apply_result(r, editor); }
        return !editor.quit;
    }

    // Minibuffer intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/Tick)
    // fall through to the normal match arm below — a FilterDone must apply even while
    // the minibuffer is open (see test `minibuffer_does_not_starve_filterdone`).
    if editor.minibuffer.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        editor.minibuffer.as_mut().unwrap().insert(c);
                    }
                    crossterm::event::KeyCode::Backspace => {
                        editor.minibuffer.as_mut().unwrap().backspace();
                    }
                    crossterm::event::KeyCode::Left => {
                        editor.minibuffer.as_mut().unwrap().left();
                    }
                    crossterm::event::KeyCode::Right => {
                        editor.minibuffer.as_mut().unwrap().right();
                    }
                    crossterm::event::KeyCode::Esc => {
                        // Dismiss the minibuffer (dismiss > cancel): this Esc is consumed
                        // here and does NOT reach the filter-cancel Esc check below, so
                        // any in-flight filter continues running.
                        editor.minibuffer = None;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let line = editor.minibuffer.take().unwrap().text;
                        submit_filter_line(editor, &line, msg_tx);
                    }
                    _ => {}
                }
            }
            for r in ex.drain() { apply_result(r, editor); }
            return !editor.quit;
        }
        // non-key (FilterDone/JobDone/Tick/Resize) falls through to the normal match below
    }

    let before = editor.active().document.version;
    match msg {
        Msg::Input(Event::Key(key)) => {
            if key.kind == crossterm::event::KeyEventKind::Press
                && key.code == crossterm::event::KeyCode::Esc
                && editor.filter_in_flight.is_some()
            {
                // Esc here cancels an in-flight filter. Safe only while no other handler claims
                // bare Esc before this point — Task 4's minibuffer Esc must be ordered to run
                // BEFORE this check (minibuffer-dismiss takes precedence over filter-cancel).
                // Revisit if any new bare-Esc binding is added.
                editor.filter_in_flight.take().unwrap().cancel();
                editor.status = "cancelling...".into();
                for r in ex.drain() {
                    apply_result(r, editor);
                }
                return !editor.quit;
            }
            match input::key_to_command_id(key) {
                Some(KeyAction::Id(id)) => {
                    let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
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
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
        }
        Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
            apply_export_done(editor, target, result, overwrite_confirmed);
        }
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.active().document.dirty()
                && !editor.active().swap_in_flight
                && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
            {
                editor.active_mut().swap_in_flight = true;
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
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

    // Warm the pandoc probe cache so the first export command doesn't pay latency.
    let _ = crate::export::probe_pandoc();

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
        let keep = reduce(msg, &mut editor, &reg, &executor, &clock, &msg_tx);
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
    use crate::app::Msg;
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
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → modal up, NOT quit yet
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(ctrl_q), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "dirty quit must raise modal");
        assert!(!e.quit);
        // Press 'q' → routed to QuitAnyway via the modal.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &ex, &clk, &tx);
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
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            let ev = Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE });
            assert!(crate::app::reduce(crate::app::Msg::Input(ev), &mut e, &reg, &ex, &clk, &tx));
        }
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn filterdone_replaces_range_when_fresh() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let msg = Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) };
        crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
        // one undo step restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
    }

    #[test]
    fn filterdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let stale_v = e.active().document.version;
        e.active_mut().document.version += 1; // simulate an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: stale_v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n", "stale filter result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
    }

    #[test]
    fn filterdone_failure_shows_status_keeps_buffer() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult, FilterError};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter,
            outcome: RunResult::Err(FilterError::NonZero { code: "Exited(3)".into(), stderr: "boom".into() }) },
            &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
        assert!(e.status.contains("boom") && e.status.contains('3'));
    }

    #[test]
    fn dispatch_filter_runs_real_command_and_delivers_filterdone() {
        // One live-thread integration test (deterministic: block on the channel).
        use crate::editor::Editor;
        use crate::filter::{dispatch_filter, FilterSpec, Disposition, Input};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let spec = FilterSpec { argv: vec!["tr".into(),"a-z".into(),"A-Z".into()], shell: false,
            disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
        dispatch_filter(&mut e, spec, tx);
        let msg = rx.recv().expect("FilterDone must arrive"); // blocks; no timing assert
        match msg { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "ABC\n"),
                    other => panic!("expected FilterDone Stdout, got {other:?}") }
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
        let (tx, _rx) = std::sync::mpsc::channel();
        // Clock past the idle threshold.
        struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let clk = C(crate::swap::T_IDLE_MS + 5);
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &ex, &clk, &tx);
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
        let (tx, _rx) = std::sync::mpsc::channel();
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // First Ctrl+Q → modal up, not quit.
        crate::app::reduce(crate::app::Msg::Input(ctrl_q.clone()), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.prompt.is_some() && !e.quit);
        // Press 'q' → routed to QuitAnyway.
        let q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(q), &mut e, &reg, &ex, &clk, &tx);
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
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
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
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
        assert_eq!(e.quit_after_save, None, "no job dispatched → do not arm quit-after-save");
        assert!(!e.quit);
    }

    #[test]
    fn apply_result_merges_fresh_and_drops_stale() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().document.version = 5;
        // Fresh one-shot (Save is never stale): merges.
        crate::app::apply_result(JobResult { buffer_id: id, class: ResultClass::Durability, version: 3, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "saved".into()) }, &mut e);
        assert_eq!(e.status, "saved");
        // Stale coalescible: dropped.
        crate::app::apply_result(JobResult { buffer_id: id, class: ResultClass::BufferLocal, version: 3, kind: JobKind::CoalesceProbe,
            merge: Box::new(|ed: &mut Editor| ed.status = "STALE".into()) }, &mut e);
        assert_eq!(e.status, "saved", "stale coalescible result must be dropped");
    }

    #[test]
    fn buffer_local_result_for_missing_buffer_is_dropped() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A buffer-local merge for a non-existent buffer must NOT run.
        crate::app::apply_result(JobResult {
            buffer_id: BufferId(999), class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "SHOULD NOT RUN".into()),
        }, &mut e);
        assert_ne!(e.status, "SHOULD NOT RUN", "buffer-local merge for a missing buffer is dropped");
    }

    #[test]
    fn buffer_local_result_for_live_buffer_merges() {
        use crate::editor::Editor;
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let id = e.active().id;
        crate::app::apply_result(JobResult {
            buffer_id: id, class: ResultClass::BufferLocal,
            version: 1, kind: JobKind::Save,
            merge: Box::new(|ed: &mut Editor| ed.status = "merged".into()),
        }, &mut e);
        assert_eq!(e.status, "merged");
    }

    #[test]
    fn recover_loads_body_and_deletes_orphan_swap_file() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        // An orphan swap file on disk + a buffer staged for recovery.
        let p = std::env::temp_dir().join(format!("wc-recover-orphan-{}.swp", std::process::id()));
        std::fs::write(&p, "stub").unwrap();
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        e.active_mut().pending_swap_body = Some("recovered body\n".into());
        e.active_mut().pending_swap_path = Some(p.clone());
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::resolve_prompt(PromptAction::Recover, &mut e, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "recovered body\n",
            "recovered content loaded into the active buffer");
        assert!(!p.exists(), "orphan swap file must be deleted on Recover");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn minibuffer_routing_and_submit_dispatches_filter() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.open_minibuffer("> ");
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        for c in "cat".chars() { crate::app::reduce(Msg::Input(key(c)), &mut e, &reg, &ex, &clk, &tx); }
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat");
        // Enter submits -> dispatch_filter -> a FilterDone arrives, minibuffer cleared
        let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.minibuffer.is_none(), "submit clears the minibuffer");
        match rx.recv().unwrap() { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "abc\n"),
                                   o => panic!("expected FilterDone, got {o:?}") }
    }

    #[test]
    fn minibuffer_does_not_starve_filterdone() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_minibuffer("> ");
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open minibuffer");
    }

    #[test]
    fn exportdone_bytes_writes_file_beside_source() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        // Create a temp directory and source file path.
        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");

        let output_path = tmp_dir.join("notes.html");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;

        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);

        // The output file must exist beside the source.
        assert!(output_path.exists(), "exported file must exist");
        let got = std::fs::read(&output_path).expect("read exported file");
        assert_eq!(got, content_bytes);
        assert!(e.status.contains("exported"), "status must say exported");

        // Clean up.
        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_unconfirmed_refuses_when_target_appeared() {
        // TOCTOU guard (Codex pre-merge gate): export was dispatched because the
        // target did not exist (overwrite_confirmed=false), but a target file has
        // since appeared.  Finalization must NOT clobber it — the user never
        // agreed to overwrite — and must leave the existing content intact.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-toctou-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        // Simulate the race: a file appeared at the target between the check and
        // the completion.
        std::fs::write(&output_path, b"PRE-EXISTING\n").expect("write racing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(b"<h1>Hello</h1>\n".to_vec())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);

        // The racing file is untouched; status tells the user to re-run.
        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, b"PRE-EXISTING\n", "unconfirmed export must not clobber an appeared target");
        assert!(e.status.contains("re-run"), "status must prompt a re-run, got: {}", e.status);

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_confirmed_overwrites_existing_target() {
        // The complement: when the user confirmed the overwrite
        // (overwrite_confirmed=true), an existing target IS replaced.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-confirmed-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        std::fs::write(&output_path, b"OLD\n").expect("write existing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let new_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(new_bytes.clone())),
            overwrite_confirmed: true,
        };
        crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);

        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, new_bytes, "confirmed export must overwrite the existing target");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_under_prompt_still_applies() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-prompt-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("doc.md");
        std::fs::write(&source, "x\n").expect("write source");
        let output_path = tmp_dir.join("doc.html");

        let mut e = Editor::new_from_text("x\n", Some(source.clone()), (80, 24));
        e.active_mut().document.version = 1; // dirty → prompt would normally be up
        // Manually raise a prompt to simulate the overlay scenario.
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());

        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<p>x</p>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &ex, &clk, &tx);

        assert!(output_path.exists(), "ExportDone under prompt must still write the file");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn transform_chooser_maps_keys_to_kinds() {
        use crate::prompt::{Prompt, PromptAction};
        use crate::transform::TransformKind;
        let p = Prompt::transform_chooser();
        assert_eq!(p.action_for('r'), Some(PromptAction::Transform(TransformKind::Reflow)));
        assert_eq!(p.action_for('u'), Some(PromptAction::Transform(TransformKind::Unwrap)));
        assert_eq!(p.action_for('v'), Some(PromptAction::Transform(TransformKind::Ventilate)));
        assert_eq!(p.action_for('x'), None);
    }

    #[test]
    fn ctrl_t_opens_the_transform_chooser() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = Event::Key(KeyEvent { code: KeyCode::Char('t'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(key), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "Ctrl+T must open the transform chooser");
        assert_eq!(
            e.prompt.as_ref().unwrap().action_for('r'),
            Some(crate::prompt::PromptAction::Transform(crate::transform::TransformKind::Reflow)),
        );
    }

    #[test]
    fn reflow_whole_buffer_applies_one_undoable_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau\n";
        let mut e = Editor::new_from_text(long, None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // dispatch_transform takes (editor, kind, clock, msg_tx) — see Task 3 Step 6.
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, &TestClock(0), &tx);
        let after = e.active().document.buffer.to_string();
        assert_ne!(after, long, "reflow should rewrap the long line");
        // exactly one undo restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), long);
    }

    #[test]
    fn transform_with_identical_output_makes_no_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // Already one-sentence-per-line: ventilate is a no-op → no edit, "already" status.
        let text = "Short.\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        let v0 = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::transform::dispatch_transform(&mut e, TransformKind::Ventilate, &TestClock(0), &tx);
        assert_eq!(e.active().document.buffer.to_string(), text);
        assert_eq!(e.active().document.version, v0, "no-op transform must not bump version");
        assert!(e.status.contains("already"));
    }

    #[test]
    fn durability_result_for_missing_buffer_still_runs() {
        use crate::editor::{Editor, BufferId};
        use crate::jobs::{JobResult, JobKind, ResultClass};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        // A durability completion runs even though its buffer is gone (e.g. closed).
        crate::app::apply_result(JobResult {
            buffer_id: BufferId(999), class: ResultClass::Durability,
            version: 1, kind: JobKind::SwapWrite,
            merge: Box::new(|ed: &mut Editor| ed.status = "durability ran".into()),
        }, &mut e);
        assert_eq!(e.status, "durability ran");
    }
}
