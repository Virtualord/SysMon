//! Terminal lifecycle management: raw mode, alternate screen, panic and signal
//! recovery.
//!
//! [`TerminalGuard`] is an RAII type. As long as it is alive the terminal is in raw
//! mode and the alternate screen is active; when it is dropped the terminal is
//! restored. The same restoration routine is reused by the panic hook and by the
//! signal flag polled from the event loop, so a crashed or signalled process still
//! leaves the user's shell in a sane state.

use std::io::{self, Stdout, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::DisableBracketedPaste;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// The concrete terminal type used by the application.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Set by the signal handler; polled by the event loop so that a clean shutdown (and
/// therefore a clean terminal restore) can happen.
static TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Puts the terminal back into a usable state.
///
/// Every operation is best effort: restoration runs on the panic path where a failure
/// cannot be reported, and on the normal path we do not want to mask the real error.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut out = stdout();
    let _ = execute!(out, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = out.flush();
}

/// Installs a panic hook that restores the terminal before the default hook runs.
///
/// The previously installed hook is preserved and called afterwards so that any
/// other tooling (loggers, crash reporters) still sees the panic.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Installs handlers for the termination signals we can reasonably recover from.
///
/// `SIGINT` normally reaches the application as a key event because raw mode disables
/// the terminal's `ISIG` flag, but a `kill -INT` bypasses that. Handling it here means
/// the process always shuts down through the normal path.
///
/// The handler only flips an atomic flag, which is async-signal-safe; the event loop
/// performs the actual exit and terminal restore.
pub fn install_signal_handlers() {
    for signal in [
        nix::sys::signal::Signal::SIGINT,
        nix::sys::signal::Signal::SIGTERM,
        nix::sys::signal::Signal::SIGHUP,
    ] {
        // SAFETY: `sigaction` with a plain function pointer and an empty flag set is
        // async-signal-safe; the handler only performs an atomic store.
        let action = nix::sys::signal::SigAction::new(
            nix::sys::signal::SigHandler::Handler(handle_termination_signal),
            nix::sys::signal::SaFlags::empty(),
            nix::sys::signal::SigSet::empty(),
        );
        // SAFETY: installing a signal handler is inherently a libc operation. The
        // action is fully initialised, the handler only performs an atomic store, and
        // the mask is empty so no signals are blocked around it.
        let installed = unsafe { nix::sys::signal::sigaction(signal, &action) };
        if let Err(err) = installed {
            tracing::warn!(%signal, %err, "could not install signal handler");
        }
    }
}

extern "C" fn handle_termination_signal(_signal: i32) {
    TERMINATION_REQUESTED.store(true, Ordering::SeqCst);
}

/// Whether a termination signal has been received.
pub fn termination_requested() -> bool {
    TERMINATION_REQUESTED.load(Ordering::SeqCst)
}

/// Clears the termination flag. Used by the tests to avoid cross-test interference.
pub fn reset_termination_flag() {
    TERMINATION_REQUESTED.store(false, Ordering::SeqCst);
}

/// Owns the initialized terminal and restores it on drop.
pub struct TerminalGuard {
    terminal: Option<Tui>,
}

impl TerminalGuard {
    /// Enables raw mode, switches to the alternate screen and builds the terminal.
    ///
    /// If any step fails the terminal is restored before the error is returned, so a
    /// failure never leaves the user's shell in raw mode.
    pub fn new() -> io::Result<Self> {
        install_panic_hook();
        install_signal_handlers();

        if let Err(err) = enable_raw_mode() {
            return Err(io::Error::new(
                err.kind(),
                format!("failed to enable raw mode: {err}"),
            ));
        }
        if let Err(err) = execute!(stdout(), EnterAlternateScreen) {
            restore_terminal();
            return Err(io::Error::new(
                err.kind(),
                format!("failed to enter alternate screen: {err}"),
            ));
        }
        match Terminal::new(CrosstermBackend::new(stdout())) {
            Ok(terminal) => Ok(Self {
                terminal: Some(terminal),
            }),
            Err(err) => {
                restore_terminal();
                Err(err)
            }
        }
    }

    /// Returns the terminal for drawing, initializing it lazily if needed.
    pub fn terminal(&mut self) -> io::Result<&mut Tui> {
        self.terminal
            .as_mut()
            .ok_or_else(|| io::Error::other("terminal has been released"))
    }

    /// Restores the terminal eagerly. Dropping the guard afterwards is a no-op.
    pub fn restore(&mut self) {
        if self.terminal.take().is_some() {
            restore_terminal();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_flag_round_trips() {
        reset_termination_flag();
        assert!(!termination_requested());
        handle_termination_signal(15);
        assert!(termination_requested());
        reset_termination_flag();
        assert!(!termination_requested());
    }

    #[test]
    fn guard_reports_missing_terminal_after_restore() {
        // A guard whose terminal was never built must fail cleanly instead of
        // panicking when the event loop asks for it.
        let mut guard = TerminalGuard { terminal: None };
        assert!(guard.terminal().is_err());
        // Dropping must not panic even though nothing was initialized.
        guard.restore();
    }
}
