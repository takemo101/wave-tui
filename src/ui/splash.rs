//! Quiet startup/shutdown splash rendering.
//!
//! This focused submodule owns the splash's pixel-art logo, message layout,
//! deterministic wave-frame animation, and timing configuration. It is a pure
//! lifecycle transition: it reads only a [`Theme`] and a tick, never app/audio
//! state, and never registers a [`crate::model::VisualizerMode`]. All colors
//! come from the active theme; no ad hoc palette literals live here.
//!
//! The startup logo is assembled from fixed-width letter cells joined in code so
//! every rendered row has identical display width. Equal-width rows are what lets
//! the centered logo block stay column-aligned: centering each row by its own
//! content width only produces a clean block when those widths match, so the art
//! must never be left to per-line centering of ragged rows.

use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Widget},
};

use crate::theme::Theme;

/// Each `WAVE` glyph is a fixed 5x5 block cell. Joining the same-row slice of
/// every letter with a constant gap yields rows of identical display width, so
/// the centered logo never staggers. Keep all rows exactly five columns wide.
const LOGO_W: [&str; 5] = ["█   █", "█   █", "█ █ █", "██ ██", "█   █"];
const LOGO_A: [&str; 5] = [" ███ ", "█   █", "█████", "█   █", "█   █"];
const LOGO_V: [&str; 5] = ["█   █", "█   █", "█   █", " █ █ ", "  █  "];
const LOGO_E: [&str; 5] = ["█████", "█    ", "████ ", "█    ", "█████"];
/// Spaces between adjacent letter cells; widens the mark for legibility.
const LOGO_GAP: &str = "   ";

const STARTUP_LABEL: &str = "wave-tui";
const STARTUP_MESSAGE: &str = "settling into the signal";
const SHUTDOWN_TITLE: &str = "thanks for listening";
const SHUTDOWN_MESSAGE: &str = "see you next wave";
/// Light wave glyphs for the animated line; intentionally calmer than the heavy
/// spectrum bars so the splash never reads as a playback visualizer.
const WAVE_GLYPHS: [char; 3] = ['~', '≈', '∿'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplashKind {
    Startup,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplashTiming {
    pub duration: Duration,
    pub frame_interval: Duration,
}

/// Assemble the five logo rows, each guaranteed to share one display width.
fn logo_rows() -> [String; 5] {
    let mut rows: [String; 5] = Default::default();
    for (row, slot) in rows.iter_mut().enumerate() {
        *slot = format!(
            "{}{gap}{}{gap}{}{gap}{}",
            LOGO_W[row],
            LOGO_A[row],
            LOGO_V[row],
            LOGO_E[row],
            gap = LOGO_GAP,
        );
    }
    rows
}

/// Normal-mode timing: ~1.1s startup, ~0.7s shutdown at a calm ~11fps cadence.
pub(super) fn normal_timing(kind: SplashKind) -> SplashTiming {
    match kind {
        SplashKind::Startup => SplashTiming {
            duration: Duration::from_millis(1_100),
            frame_interval: Duration::from_millis(90),
        },
        SplashKind::Shutdown => SplashTiming {
            duration: Duration::from_millis(700),
            frame_interval: Duration::from_millis(90),
        },
    }
}

/// Low-power timing: shorter duration and a longer frame interval so the splash
/// draws fewer frames while keeping the same visual language.
pub(super) fn low_power_timing(kind: SplashKind) -> SplashTiming {
    match kind {
        SplashKind::Startup => SplashTiming {
            duration: Duration::from_millis(700),
            frame_interval: Duration::from_millis(140),
        },
        SplashKind::Shutdown => SplashTiming {
            duration: Duration::from_millis(450),
            frame_interval: Duration::from_millis(140),
        },
    }
}

/// Render one splash frame for `kind` at `tick` into `buf`, centered in `area`.
///
/// Deterministic: the same `(kind, theme, tick)` always paints the same cells.
pub(super) fn render_splash_into(
    kind: SplashKind,
    theme: &Theme,
    tick: u16,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    buf.set_style(area, theme.base_style());
    Clear.render(area, buf);

    // Generous, but clamped to the screen so tiny terminals stay safe. The block
    // adds horizontal breathing room; blank lines space the rows vertically.
    let (content_height, splash_width) = match kind {
        SplashKind::Startup => (STARTUP_LINE_COUNT, 56),
        SplashKind::Shutdown => (SHUTDOWN_LINE_COUNT, 44),
    };
    let splash_height = area.height.min(content_height);
    let splash_width = area.width.min(splash_width);
    let splash_area = centered_rect(area, splash_width, splash_height);

    // Only the shutdown farewell carries the animated wave line; the startup
    // splash is a calm, static logo card (still timed and skippable).
    let lines = match kind {
        SplashKind::Startup => startup_lines(theme),
        SplashKind::Shutdown => shutdown_lines(theme, wave_line(splash_area.width, tick)),
    };

    // No `Wrap`: trimming whitespace would collapse the logo's interior spacing
    // and re-stagger the rows. Equal-width rows + center alignment keep the block
    // aligned; short message lines fit without wrapping.
    let paragraph = Paragraph::new(lines).alignment(Alignment::Center).block(
        Block::default()
            .style(theme.base_style())
            .padding(Padding::horizontal(2)),
    );

    paragraph.render(splash_area, buf);
}

/// Total startup lines (blank + logo + blanks + label + blank + message + blank;
/// no wave line). The trailing blank keeps breathing room below the message.
const STARTUP_LINE_COUNT: u16 = 12;
/// Total shutdown lines (title + blank spacers + message + wave).
const SHUTDOWN_LINE_COUNT: u16 = 7;

fn startup_lines(theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(STARTUP_LINE_COUNT as usize);
    let logo_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    lines.push(Line::from(""));
    for row in logo_rows() {
        lines.push(Line::from(Span::styled(row, logo_style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        STARTUP_LABEL,
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        STARTUP_MESSAGE,
        Style::default().fg(theme.foreground),
    )));
    // Trailing blank: breathing room below the message (startup stays static, no
    // wave line).
    lines.push(Line::from(""));
    debug_assert_eq!(lines.len(), STARTUP_LINE_COUNT as usize);
    lines
}

fn shutdown_lines(theme: &Theme, wave: String) -> Vec<Line<'static>> {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            SHUTDOWN_TITLE,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            SHUTDOWN_MESSAGE,
            Style::default().fg(theme.foreground),
        )),
        Line::from(""),
        Line::from(Span::styled(wave, Style::default().fg(theme.playing))),
        Line::from(""),
    ];
    debug_assert_eq!(lines.len(), SHUTDOWN_LINE_COUNT as usize);
    lines
}

/// Center a `width`x`height` rect inside `area`, clamping to fit.
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

/// Build the animated wave line: glyphs phase-shift by `tick` so successive
/// frames differ deterministically.
fn wave_line(width: u16, tick: u16) -> String {
    if width == 0 {
        return String::new();
    }
    let visible = width.saturating_sub(4).max(1) as usize;
    (0..visible)
        .map(|i| WAVE_GLYPHS[(i + tick as usize) % WAVE_GLYPHS.len()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    /// Render a splash into a grid of per-cell symbols for inspection.
    fn render_grid(kind: SplashKind, tick: u16) -> Vec<Vec<String>> {
        let theme = ThemeName::Minimal.theme();
        let area = Rect::new(0, 0, 64, 18);
        let mut buf = Buffer::empty(area);
        render_splash_into(kind, &theme, tick, area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn render_to_string(kind: SplashKind, tick: u16) -> String {
        render_grid(kind, tick)
            .into_iter()
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Rows (by y) that contain at least one logo block glyph, and the leftmost
    /// column where that glyph appears on each.
    fn logo_row_left_edges(grid: &[Vec<String>]) -> Vec<(usize, usize)> {
        grid.iter()
            .enumerate()
            .filter_map(|(y, row)| row.iter().position(|c| c == "█").map(|x| (y, x)))
            .collect()
    }

    /// The y of the first row whose trimmed text equals `needle`.
    fn row_of_text(grid: &[Vec<String>], needle: &str) -> Option<usize> {
        grid.iter().position(|row| row.concat().trim() == needle)
    }

    #[test]
    fn logo_letter_cells_are_uniform_five_columns() {
        for letter in [LOGO_W, LOGO_A, LOGO_V, LOGO_E] {
            for cell in letter {
                assert_eq!(
                    cell.chars().count(),
                    5,
                    "logo cell {cell:?} must be exactly five columns wide"
                );
            }
        }
    }

    #[test]
    fn assembled_logo_rows_share_one_width() {
        let widths: Vec<usize> = logo_rows().iter().map(|r| r.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all logo rows must share a single display width, got {widths:?}"
        );
    }

    #[test]
    fn startup_splash_contains_logo_label_and_message() {
        let out = render_to_string(SplashKind::Startup, 0);
        // Distinctive middle rows of the assembled WAVE mark.
        assert!(out.contains("█ █ █"), "W center band should render");
        assert!(out.contains("█████"), "A/E full bands should render");
        assert!(out.contains("wave-tui"));
        assert!(out.contains("settling into the signal"));
    }

    #[test]
    fn startup_logo_rows_are_left_aligned_not_staggered() {
        let grid = render_grid(SplashKind::Startup, 0);
        let edges = logo_row_left_edges(&grid);
        assert_eq!(
            edges.len(),
            5,
            "exactly five logo rows should render, got {edges:?}"
        );
        // The rows must be contiguous (a single block, no gaps).
        let ys: Vec<usize> = edges.iter().map(|(y, _)| *y).collect();
        assert!(
            ys.windows(2).all(|w| w[1] == w[0] + 1),
            "logo rows must be contiguous, got rows {ys:?}"
        );
        // Every row's leftmost glyph must sit in the same column: a centered block
        // of equal-width rows, never a per-line-centered ragged stagger.
        let left = edges[0].1;
        assert!(
            edges.iter().all(|(_, x)| *x == left),
            "logo rows must share one left edge (no stagger), got {edges:?}"
        );
    }

    #[test]
    fn startup_splash_renders_no_wave_glyph_line() {
        // The animated wave line was removed from startup; it must not appear on
        // any frame, so the startup card stays a calm, static logo.
        for tick in 0..6 {
            let out = render_to_string(SplashKind::Startup, tick);
            for glyph in WAVE_GLYPHS {
                assert!(
                    !out.contains(glyph),
                    "startup splash must not render wave glyph {glyph:?} (tick {tick})"
                );
            }
        }
    }

    #[test]
    fn startup_splash_is_static_across_ticks() {
        // With no wave line, startup is identical every frame (still timed/skippable).
        let first = render_to_string(SplashKind::Startup, 0);
        let second = render_to_string(SplashKind::Startup, 1);
        assert_eq!(first, second, "startup splash should not animate");
    }

    #[test]
    fn startup_has_breathing_room_between_sections() {
        let grid = render_grid(SplashKind::Startup, 0);
        let last_logo_row = logo_row_left_edges(&grid)
            .last()
            .map(|(y, _)| *y)
            .expect("logo should render");
        let label_row = row_of_text(&grid, "wave-tui").expect("label should render");
        let message_row =
            row_of_text(&grid, "settling into the signal").expect("message should render");
        assert!(
            label_row >= last_logo_row + 2,
            "expected at least one blank row between logo (row {last_logo_row}) \
             and label (row {label_row})"
        );
        assert!(
            message_row >= label_row + 2,
            "expected at least one blank row between label (row {label_row}) \
             and message (row {message_row})"
        );
    }

    #[test]
    fn startup_ends_with_blank_spacer_below_message() {
        // The startup card must keep one blank line below `settling into the
        // signal` for breathing room (and no wave line in its place).
        let theme = ThemeName::Minimal.theme();
        let lines = startup_lines(&theme);
        let n = lines.len();
        assert!(
            lines[n - 1].width() == 0,
            "startup must end with a blank spacer line below the message"
        );
        assert!(
            lines[n - 2].width() > 0,
            "the message line should sit directly above the trailing blank"
        );
    }

    #[test]
    fn shutdown_splash_contains_farewell_message() {
        let out = render_to_string(SplashKind::Shutdown, 0);
        assert!(out.contains("thanks for listening"));
        assert!(out.contains("see you next wave"));
    }

    #[test]
    fn shutdown_has_breathing_room_between_lines() {
        let grid = render_grid(SplashKind::Shutdown, 0);
        let title_row = row_of_text(&grid, "thanks for listening").expect("title should render");
        let message_row = row_of_text(&grid, "see you next wave").expect("message should render");
        assert!(
            message_row >= title_row + 2,
            "expected a blank row between farewell title (row {title_row}) \
             and message (row {message_row})"
        );
    }

    #[test]
    fn shutdown_wave_frame_changes_by_tick() {
        // The wave animation now lives only on the shutdown farewell.
        let first = render_to_string(SplashKind::Shutdown, 0);
        let second = render_to_string(SplashKind::Shutdown, 1);
        assert_ne!(first, second, "wave animation should change between ticks");
        assert!(first.contains('~') || first.contains('≈') || first.contains('∿'));
        assert!(second.contains('~') || second.contains('≈') || second.contains('∿'));
    }

    #[test]
    fn low_power_timing_is_not_more_expensive_than_normal() {
        let normal = normal_timing(SplashKind::Startup);
        let low = low_power_timing(SplashKind::Startup);
        assert!(low.duration <= normal.duration);
        assert!(low.frame_interval >= normal.frame_interval);
    }

    #[test]
    fn tiny_area_is_safe() {
        let theme = ThemeName::Minimal.theme();
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(area);
        render_splash_into(SplashKind::Startup, &theme, 0, area, &mut buf);
    }
}
