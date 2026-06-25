// wordcartel/src/term.rs — terminal lifecycle: raw mode, alt screen, panic restore.
//
// §15.7: the panic hook restores the terminal BEFORE chaining to the previous
// hook, so the user always gets their shell back even on a crash.

use std::io::{self, Stdout};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, EnableBracketedPaste},
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
    /// If any step after `enable_raw_mode` fails, raw mode and the alternate
    /// screen are rolled back before returning the error so the terminal is
    /// always left in a usable state (no raw-mode leak).
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        // From this point forward any failure must roll back raw mode.
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e);
        }
        // Enable bracketed paste (best-effort: if the terminal doesn't support it, ignore).
        let _ = execute!(stdout, EnableBracketedPaste);
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, Show);
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
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, Show);
    }
}

// ---------------------------------------------------------------------------
// install_panic_hook — call once from `app::run` before entering the loop
// ---------------------------------------------------------------------------

/// Install a panic hook that restores the terminal before chaining to the
/// previous hook. Safe to call multiple times (uses `std::sync::Once`).
pub fn install_panic_hook() {
    use std::sync::Once;
    static HOOK_INSTALLED: Once = Once::new();
    HOOK_INSTALLED.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort emergency dump (try_lock; never deadlock).
            crate::recovery::dump_on_panic();
            // Restore the terminal.
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, Show);
            prev(info);
        }));
    });
}
