//! Terminal ownership for a run.
//!
//! The raw-mode/alternate-screen entry and its RAII restoration live here, so
//! every exit path — normal quit, recoverable error, or panic — leaves the
//! user's terminal as it was found.

use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::herdr::HerdrMonitor;

/// Whether the terminal should capture mouse events for this run.
///
/// Mouse capture exists only to feed `Event::Mouse` to the Agent Pulse
/// overlay's read-only click selection, and it changes terminal behavior
/// (native text selection needs Shift+drag while captured). So it follows
/// the monitor exactly: standalone, ineligible, and `--no-agent-pulse`
/// launches keep their pre-integration terminal behavior untouched.
pub(super) fn mouse_capture_for(monitor: Option<&HerdrMonitor>) -> bool {
    monitor.is_some()
}

/// RAII guard owning the terminal in raw/alternate-screen mode.
///
/// Restoration runs in [`Drop`], so the terminal is restored on a normal quit,
/// on a recoverable error returned from the event loop, and on a panic.
pub(super) struct TerminalGuard {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Whether mouse capture was enabled and must be released on drop.
    mouse_capture: bool,
}

impl TerminalGuard {
    pub(super) fn new(mouse_capture: bool) -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        let mut stdout = io::stdout();
        if mouse_capture {
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
                .context("entering alternate screen")?;
        } else {
            execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating terminal")?;
        Ok(Self {
            terminal,
            mouse_capture,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse_capture {
            let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
        }
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr;

    #[test]
    fn mouse_capture_follows_the_monitor() {
        // Standalone/ineligible/disabled launches have no monitor and must
        // keep exact pre-integration terminal behavior: no mouse capture.
        assert!(!mouse_capture_for(None));

        // Only a live monitor (an eligible plugin launch) turns capture on.
        // The socket path does not need to work; the monitor only needs to
        // exist, exactly as in run_app.
        let monitor = herdr::spawn_monitor(herdr::HerdrContext {
            socket_path: "/nonexistent/wave-tui-cli-test.sock".into(),
        });
        assert!(mouse_capture_for(Some(&monitor)));
        monitor.stop();
    }
}
