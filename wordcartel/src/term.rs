// wordcartel/src/term.rs — terminal lifecycle: raw mode, alt screen, panic restore.
//
// §15.7: the panic hook restores the terminal BEFORE chaining to the previous
// hook, so the user always gets their shell back even on a crash.

use std::io::{self, Stdout};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

// ---------------------------------------------------------------------------
// TerminalGuard — RAII wrapper: enable on new(), restore on drop()
// ---------------------------------------------------------------------------

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    /// Enable raw mode, enter the alternate screen, and return a `TerminalGuard`
    /// whose `Drop` impl will restore the terminal.
    ///
    /// If `enable_mouse` is true, mouse capture is enabled (best-effort).
    ///
    /// If any step after `enable_raw_mode` fails, raw mode and the alternate
    /// screen are rolled back before returning the error so the terminal is
    /// always left in a usable state (no raw-mode leak).
    pub fn new(enable_mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        // From this point forward any failure must roll back raw mode.
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e);
        }
        // Enable bracketed paste (best-effort: if the terminal doesn't support it, ignore).
        let _ = execute!(stdout, EnableBracketedPaste);
        // Enable mouse capture only when requested (best-effort).
        if enable_mouse {
            let _ = execute!(stdout, EnableMouseCapture);
        }
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
                return Err(e);
            }
        };
        Ok(Self { terminal })
    }

    /// Borrow the inner `Terminal` for drawing.
    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
    }
}

// ---------------------------------------------------------------------------
// install_panic_hook — call once from `app::run` before entering the loop
// ---------------------------------------------------------------------------

/// Returns `true` when `panicking` is the main thread that owns the terminal.
///
/// Extracted for testability: the hook itself cannot be unit-tested without
/// real terminal I/O, but this predicate can be.
pub(crate) fn should_handle_panic(
    panicking: std::thread::ThreadId,
    main: std::thread::ThreadId,
) -> bool {
    panicking == main
}

/// Install a panic hook that restores the terminal before chaining to the
/// previous hook. Safe to call multiple times (uses `std::sync::Once`).
///
/// Only the main thread (the one that called `install_panic_hook`) triggers
/// the dump + terminal restore.  A non-main-thread panic (job worker,
/// clipboard helper, input reader) is caught by the thread's own
/// `catch_unwind`; the hook must not touch the terminal there, or it corrupts
/// the live UI while the main loop keeps running.
pub fn install_panic_hook() {
    use std::sync::Once;
    static HOOK_INSTALLED: Once = Once::new();
    HOOK_INSTALLED.call_once(|| {
        let main_id = std::thread::current().id();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Non-main-thread panic (job/clipboard/input worker): caught by the
            // thread's own catch_unwind; must NOT restore the terminal or it
            // corrupts the live UI. No-op here.
            if !should_handle_panic(std::thread::current().id(), main_id) { return; }
            // Best-effort emergency dump (try_lock; never deadlock).
            crate::recovery::dump_on_panic();
            // Restore the terminal.
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
            prev(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_handle_panic_same_thread_is_true() {
        let id = std::thread::current().id();
        assert!(should_handle_panic(id, id),
            "same thread id must be handled (main-thread panic)");
    }

    #[test]
    fn should_handle_panic_different_thread_is_false() {
        let main_id = std::thread::current().id();
        let worker_id = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        assert!(!should_handle_panic(worker_id, main_id),
            "worker thread id must NOT be handled (non-main panic)");
    }
}
