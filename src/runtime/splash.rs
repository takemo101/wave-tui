//! The quiet startup/shutdown splash.
//!
//! Runs outside the main event loop — startup before it, shutdown after it —
//! so its key-to-skip handling never competes with the app's key mappings. The
//! frame budget is pure timing math and unit tested without a terminal.

use std::io::Stdout;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::theme::ThemeName;

/// Number of frames to draw to cover a splash's duration at its frame interval.
///
/// Pure timing math so the splash loop budget is unit-testable without a
/// terminal. Always at least one frame; saturates at `u16::MAX`.
fn splash_frame_budget(timing: crate::ui::SplashTiming) -> u16 {
    let interval_ms = timing.frame_interval.as_millis().max(1);
    let duration_ms = timing.duration.as_millis();
    let frames = duration_ms.div_ceil(interval_ms).max(1);
    frames.min(u16::MAX as u128) as u16
}

/// Draw the quiet lifecycle splash for `kind` until its duration elapses or any
/// key is pressed.
///
/// Runs outside the main event loop (startup before it, shutdown after it), so
/// its key-to-skip handling never interferes with the app's key mappings. Only
/// key events skip; other terminal events are left for the main loop's own
/// polling and do not change app behavior.
pub(super) fn run_splash(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: ThemeName,
    kind: crate::ui::SplashKind,
    low_power: bool,
) -> Result<()> {
    let timing = crate::ui::splash_timing(kind, low_power);
    let palette = theme.theme();
    let frames = splash_frame_budget(timing);

    for tick in 0..frames {
        terminal.draw(|frame| crate::ui::render_splash(kind, &palette, tick, frame))?;
        if event::poll(timing.frame_interval)? && matches!(event::read()?, Event::Key(_)) {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn splash_frame_budget_covers_duration_at_least_once() {
        let timing = crate::ui::SplashTiming {
            duration: Duration::from_millis(250),
            frame_interval: Duration::from_millis(100),
        };
        assert_eq!(splash_frame_budget(timing), 3);
    }

    #[test]
    fn low_power_splash_budget_is_no_larger_than_normal() {
        let normal = crate::ui::splash_timing(crate::ui::SplashKind::Startup, false);
        let low = crate::ui::splash_timing(crate::ui::SplashKind::Startup, true);
        assert!(splash_frame_budget(low) <= splash_frame_budget(normal));
    }
}
