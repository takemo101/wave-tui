//! Agent Pulse rendering: the Quiet Companion summary and the Status
//! Constellation overlay.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. Mouse input flows through [`hit_test`],
//! which shares [`overlay_layout`] with rendering so a click resolves against
//! exactly the geometry that was drawn, and returns only the read-only
//! selection/disclosure [`Action`]s; the CLI event loop owns applying them.
//!
//! All colors come from the active [`Theme`]; no palette values are added.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use std::time::{Duration, Instant};

use crate::app::{Action, AgentPulseConnection, AgentView, App};
use crate::herdr::AgentStatus;
use crate::theme::Theme;

/// Most active-list rows shown at once; the list stays a short companion to
/// the constellation, windowed so the selected agent is always visible.
const LIST_CAP: usize = 5;
/// Most completed-history rows shown when the disclosure is expanded.
const COMPLETED_CAP: usize = 4;
/// Preferred overlay width; shrinks on narrow terminals.
const OVERLAY_DESIRED_WIDTH: u16 = 56;
/// Below this area the overlay drops the constellation and information card,
/// keeping the readable list and disclosure (the compact fallback).
const COMPACT_WIDTH: u16 = 60;
const COMPACT_HEIGHT: u16 = 20;

/// Display order for status counts and node/list sorting context: mirrors the
/// active-agent sort rank in `app`.
const STATUS_ORDER: [AgentStatus; 5] = [
    AgentStatus::Working,
    AgentStatus::Blocked,
    AgentStatus::Idle,
    AgentStatus::Done,
    AgentStatus::Unknown,
];

// --- Quiet Companion summary ---------------------------------------------

/// The one-line Now Playing summary for Wide/Medium tiers.
///
/// `None` when hidden (standalone/ineligible) or unavailable, so those states
/// reserve no row and standalone output stays byte-identical. Connected shows
/// state counts (or the calm connected-empty copy); stale dims the last known
/// state and appends the `stale · reconnecting` marker.
pub(super) fn summary_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    match app.agent_pulse_connection() {
        AgentPulseConnection::Hidden | AgentPulseConnection::Unavailable => None,
        AgentPulseConnection::Connected => {
            Some(Line::from(status_count_spans(app.active_agents(), theme)))
        }
        AgentPulseConnection::Stale => {
            let mut spans = status_count_spans(app.active_agents(), theme);
            spans.push(Span::styled(
                " · stale · reconnecting",
                Style::default().fg(theme.muted),
            ));
            Some(Line::from(spans).patch_style(Style::default().add_modifier(Modifier::DIM)))
        }
    }
}

/// State-count spans like `● 2 working · ○ 1 idle`, in status order, or the
/// calm `agents · none active` copy when no agent is active.
fn status_count_spans<'a>(agents: &[AgentView], theme: &Theme) -> Vec<Span<'a>> {
    if agents.is_empty() {
        return vec![Span::styled(
            "agents · none active",
            Style::default().fg(theme.muted),
        )];
    }
    let mut spans = Vec::new();
    for status in STATUS_ORDER {
        let count = agents.iter().filter(|view| view.status == status).count();
        if count == 0 {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(theme.muted)));
        }
        spans.push(Span::styled(
            format!("{} {count} {}", status_glyph(status), status_label(status)),
            Style::default().fg(status_color(status, theme)),
        ));
    }
    spans
}

// --- status presentation helpers ------------------------------------------

/// Short lowercase state label shared by the summary, list, and card.
fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// Constellation node / summary glyph per status.
fn status_glyph(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "●",
        AgentStatus::Blocked => "◆",
        AgentStatus::Idle => "○",
        AgentStatus::Done => "✓",
        AgentStatus::Unknown => "?",
    }
}

/// Theme color per status: strong color only for useful signal (activity and
/// attention); idle/done/unknown stay muted.
fn status_color(status: AgentStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        AgentStatus::Working => theme.playing,
        AgentStatus::Blocked => theme.error,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => theme.muted,
    }
}

/// How long a freshly observed status keeps its one-shot acknowledgement.
///
/// `App` resets `observed_at` whenever a pane's status changes, so "recently
/// observed in this status" is exactly "the state just changed" — the
/// highlight derives purely from existing state and expires on its own, with
/// no per-frame mutation or toast.
const STATUS_CHANGE_HIGHLIGHT: Duration = Duration::from_secs(2);

/// Whether a status observed `elapsed` ago still shows its one restrained
/// visual acknowledgement.
fn recent_change_highlight(elapsed: Duration) -> bool {
    elapsed < STATUS_CHANGE_HIGHLIGHT
}

/// Whether a working node renders its dimmed pulse phase.
///
/// The slow, quiet pulse alternates every two seconds of the agent's observed
/// duration; low-power rendering is static (never dimmed by phase). Pure so
/// the motion contract is testable without a terminal or clock.
fn working_pulse_dim(elapsed: Duration, low_power: bool) -> bool {
    !low_power && elapsed.as_secs() % 4 >= 2
}

/// Node style per status at an observed-state age: a brief bold
/// acknowledgement right after a status change, then the working pulse phase
/// (static under low power), a static blocked node, and a dimmed done node;
/// all colors are theme-sourced.
fn node_style(status: AgentStatus, theme: &Theme, elapsed: Duration, low_power: bool) -> Style {
    let mut style = Style::default().fg(status_color(status, theme));
    if recent_change_highlight(elapsed) {
        style = style.add_modifier(Modifier::BOLD);
    }
    match status {
        AgentStatus::Working if working_pulse_dim(elapsed, low_power) => {
            style.add_modifier(Modifier::DIM)
        }
        AgentStatus::Done => style.add_modifier(Modifier::DIM),
        _ => style,
    }
}

/// Estimated duration label (`<1m`, `~12m`, `~2h`) for an observed-state age.
fn format_observed_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        "<1m".to_string()
    } else if secs < 3600 {
        format!("~{}m", secs / 60)
    } else {
        format!("~{}h", secs / 3600)
    }
}

// --- overlay geometry ------------------------------------------------------

/// One overlay content row, in draw order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// `stale · reconnecting` banner (stale only).
    Banner,
    /// The Status Constellation node row.
    Nodes,
    /// Spacer row.
    Blank,
    /// One active-list row; carries the index into `App::active_agents`.
    ListRow(usize),
    /// `… n more` marker for active agents beyond the visible window.
    ListMore(usize),
    /// Information-card title line (selected agent metadata or key hint).
    CardTitle,
    /// Information-card status/duration line.
    CardStatus,
    /// The `Completed (n)` disclosure line.
    Disclosure,
    /// One expanded completed-history row (index into newest-first history).
    CompletedRow(usize),
    /// `… n more` marker for completed history beyond the visible rows.
    CompletedMore(usize),
    /// Calm connected-empty copy.
    EmptyCopy,
    /// Unavailable copy.
    UnavailableCopy,
}

/// Pure overlay geometry shared by rendering and hit testing.
struct OverlayLayout {
    /// The centered, cleared overlay rect (including its border).
    overlay: Rect,
    /// Content rows top to bottom, clipped to the overlay's inner area.
    rows: Vec<(Rect, RowKind)>,
    /// Constellation node hit targets: rect plus active-agent index.
    nodes: Vec<(Rect, usize)>,
}

/// Compute the overlay geometry, or `None` when no overlay exists: closed,
/// hidden integration, Signal View active, or a degenerate area.
fn overlay_layout(app: &App, area: Rect) -> Option<OverlayLayout> {
    if !app.is_agent_overlay_open() || app.is_signal_view() {
        return None;
    }
    let connection = app.agent_pulse_connection();
    if connection == AgentPulseConnection::Hidden {
        return None;
    }
    if area.width < 12 || area.height < 5 {
        return None;
    }

    let compact = area.width < COMPACT_WIDTH || area.height < COMPACT_HEIGHT;
    let active = app.active_agents();
    let completed_len = app.completed_agents().len();

    let mut kinds: Vec<RowKind> = Vec::new();
    match connection {
        AgentPulseConnection::Hidden => return None,
        AgentPulseConnection::Unavailable => kinds.push(RowKind::UnavailableCopy),
        AgentPulseConnection::Connected | AgentPulseConnection::Stale => {
            if connection == AgentPulseConnection::Stale {
                kinds.push(RowKind::Banner);
            }
            if active.is_empty() {
                kinds.push(RowKind::EmptyCopy);
            } else {
                if !compact {
                    kinds.push(RowKind::Nodes);
                    kinds.push(RowKind::Blank);
                }
                let visible = active.len().min(LIST_CAP);
                let selected = selected_index(app).unwrap_or(0);
                let start = selected
                    .saturating_sub(visible.saturating_sub(1))
                    .min(active.len() - visible);
                for index in start..start + visible {
                    kinds.push(RowKind::ListRow(index));
                }
                if active.len() > visible {
                    kinds.push(RowKind::ListMore(active.len() - visible));
                }
                if !compact {
                    kinds.push(RowKind::Blank);
                    kinds.push(RowKind::CardTitle);
                    kinds.push(RowKind::CardStatus);
                }
            }
            kinds.push(RowKind::Disclosure);
            if app.completed_agents_disclosed() && completed_len > 0 {
                let shown = completed_len.min(COMPLETED_CAP);
                for index in 0..shown {
                    kinds.push(RowKind::CompletedRow(index));
                }
                if completed_len > shown {
                    kinds.push(RowKind::CompletedMore(completed_len - shown));
                }
            }
        }
    }

    // Center the overlay like the hidden-Browse modal: full width on tiny
    // panes, otherwise the preferred width with a margin.
    let width = if area.width <= OVERLAY_DESIRED_WIDTH {
        area.width
    } else {
        OVERLAY_DESIRED_WIDTH.min(area.width.saturating_sub(4))
    };
    let desired_height = (kinds.len() as u16).saturating_add(2);
    let height = if area.height <= desired_height {
        area.height
    } else {
        desired_height.min(area.height.saturating_sub(2))
    };
    let overlay = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let inner = Rect::new(
        overlay.x + 1,
        overlay.y + 1,
        overlay.width.saturating_sub(2),
        overlay.height.saturating_sub(2),
    );

    let mut rows = Vec::new();
    let mut nodes = Vec::new();
    for (offset, kind) in kinds.into_iter().enumerate() {
        let y = inner.y + offset as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        if kind == RowKind::Nodes {
            // Node slots: a 3-cell hit target per agent, spaced every 4 cells.
            for index in 0..active.len() {
                let x = row.x + 1 + (index as u16) * 4;
                if x + 3 > row.x + row.width {
                    break;
                }
                nodes.push((Rect::new(x, y, 3, 1), index));
            }
        }
        rows.push((row, kind));
    }

    Some(OverlayLayout {
        overlay,
        rows,
        nodes,
    })
}

/// Index of the overlay-selected agent within the sorted active list.
fn selected_index(app: &App) -> Option<usize> {
    let selected = app.selected_agent()?;
    app.active_agents()
        .iter()
        .position(|view| view.pane_id == selected.pane_id)
}

/// Whether (`x`, `y`) falls inside `rect`.
fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

// --- hit testing -----------------------------------------------------------

/// Pure mouse hit test for the overlay; see `ui::agent_pulse_hit_test`.
///
/// Returns only read-only selection/disclosure actions, and `None` whenever
/// the overlay is closed, the integration is hidden, the connection is stale
/// or unavailable, Signal View is active, or the click is outside the overlay
/// or on a non-interactive row.
pub(super) fn hit_test(area: Rect, column: u16, row: u16, app: &App) -> Option<Action> {
    if app.agent_pulse_connection() != AgentPulseConnection::Connected {
        return None;
    }
    let layout = overlay_layout(app, area)?;
    if !rect_contains(layout.overlay, column, row) {
        return None;
    }
    for (rect, index) in &layout.nodes {
        if rect_contains(*rect, column, row) {
            let pane = app.active_agents().get(*index)?.pane_id.clone();
            return Some(Action::SelectAgent(pane));
        }
    }
    for (rect, kind) in &layout.rows {
        if !rect_contains(*rect, column, row) {
            continue;
        }
        match kind {
            RowKind::ListRow(index) => {
                let pane = app.active_agents().get(*index)?.pane_id.clone();
                return Some(Action::SelectAgent(pane));
            }
            RowKind::Disclosure => return Some(Action::ToggleCompletedAgents),
            _ => return None,
        }
    }
    None
}

// --- overlay rendering -----------------------------------------------------

/// Render the Status Constellation overlay over the composed normal layout.
///
/// A no-op unless the overlay is open and the integration is visible, so
/// normal and standalone output is untouched. Clears the centered rect, then
/// draws the constellation, short active list, information card, and
/// completed disclosure from the shared [`overlay_layout`] geometry. `now`
/// and `low_power` are injected by the render entry point, keeping motion
/// deterministic and clock-free here.
pub(super) fn render_overlay(
    app: &App,
    theme: &Theme,
    low_power: bool,
    now: Instant,
    area: Rect,
    buf: &mut Buffer,
) {
    let Some(layout) = overlay_layout(app, area) else {
        return;
    };
    let stale = app.agent_pulse_connection() == AgentPulseConnection::Stale;
    let selected = selected_index(app);

    Clear.render(layout.overlay, buf);
    buf.set_style(layout.overlay, theme.base_style());
    super::bordered_block(theme, "Agent Pulse", false).render(layout.overlay, buf);

    for (rect, kind) in &layout.rows {
        let line = match kind {
            RowKind::Blank | RowKind::Nodes => continue,
            RowKind::Banner => {
                Line::styled("stale · reconnecting", Style::default().fg(theme.muted))
            }
            RowKind::ListRow(index) => match app.active_agents().get(*index) {
                Some(view) => list_row_line(view, selected == Some(*index), theme, now),
                None => continue,
            },
            RowKind::ListMore(hidden) => {
                Line::styled(format!("… {hidden} more"), Style::default().fg(theme.muted))
            }
            RowKind::CardTitle => card_title_line(app, theme),
            RowKind::CardStatus => match app.selected_agent() {
                Some(view) => Line::from(vec![
                    Span::styled(
                        status_label(view.status).to_string(),
                        Style::default().fg(status_color(view.status, theme)),
                    ),
                    Span::styled(
                        format!(
                            " for {}",
                            format_observed_duration(view.observed_duration(now))
                        ),
                        Style::default().fg(theme.muted),
                    ),
                ]),
                None => continue,
            },
            RowKind::Disclosure => {
                let arrow = if app.completed_agents_disclosed() {
                    "▾"
                } else {
                    "▸"
                };
                Line::from(vec![
                    Span::styled(
                        format!("Completed ({})", app.completed_agents().len()),
                        Style::default().fg(theme.foreground),
                    ),
                    Span::styled(format!(" {arrow}"), theme.accent_style()),
                ])
            }
            RowKind::CompletedRow(index) => match app.completed_agents().nth(*index) {
                Some(entry) => Line::styled(
                    format!(
                        "✓ {} · {} ago",
                        entry.agent.display_name(),
                        format_observed_duration(now.saturating_duration_since(entry.completed_at))
                    ),
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                ),
                None => continue,
            },
            RowKind::CompletedMore(hidden) => {
                Line::styled(format!("… {hidden} more"), Style::default().fg(theme.muted))
            }
            RowKind::EmptyCopy => {
                Line::styled("agents · none active", Style::default().fg(theme.muted))
            }
            RowKind::UnavailableCopy => Line::styled(
                "agents · unavailable · retrying",
                Style::default().fg(theme.muted),
            ),
        };
        let line = if stale {
            line.patch_style(Style::default().add_modifier(Modifier::DIM))
        } else {
            line
        };
        Paragraph::new(line)
            .style(theme.base_style())
            .render(*rect, buf);
    }

    // Constellation nodes, drawn cell-precise over their hit targets.
    for (rect, index) in &layout.nodes {
        let Some(view) = app.active_agents().get(*index) else {
            continue;
        };
        let mut style = if selected == Some(*index) {
            theme.selection_style()
        } else {
            node_style(view.status, theme, view.observed_duration(now), low_power)
        };
        if stale {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_string(rect.x + 1, rect.y, status_glyph(view.status), style);
    }
}

/// One short active-list row: selection cursor, display name, agent type,
/// short cwd label, state, and estimated state duration.
fn list_row_line<'a>(view: &AgentView, selected: bool, theme: &Theme, now: Instant) -> Line<'a> {
    let cursor = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground)
    };
    let muted = Style::default().fg(theme.muted);

    let mut spans = vec![
        Span::styled(cursor.to_string(), theme.accent_style()),
        Span::styled(view.display_name().to_string(), name_style),
    ];
    if let Some(agent) = &view.agent {
        if agent != view.display_name() {
            spans.push(Span::styled(format!("  {agent}"), muted));
        }
    }
    if let Some(cwd) = &view.cwd {
        spans.push(Span::styled(format!("  {cwd}"), muted));
    }
    spans.push(Span::styled(
        format!("  {}", status_label(view.status)),
        Style::default().fg(status_color(view.status, theme)),
    ));
    spans.push(Span::styled(
        format!(" {}", format_observed_duration(view.observed_duration(now))),
        muted,
    ));
    Line::from(spans)
}

/// Information-card title: selected-agent metadata, or a quiet key hint while
/// nothing is selected.
fn card_title_line<'a>(app: &App, theme: &Theme) -> Line<'a> {
    let muted = Style::default().fg(theme.muted);
    match app.selected_agent() {
        Some(view) => {
            let mut spans = vec![Span::styled(
                view.display_name().to_string(),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            )];
            if let Some(agent) = &view.agent {
                if agent != view.display_name() {
                    spans.push(Span::styled(format!(" · {agent}"), muted));
                }
            }
            if let Some(cwd) = &view.cwd {
                spans.push(Span::styled(format!(" · {cwd}"), muted));
            }
            Line::from(spans)
        }
        None => Line::from(Span::styled("Tab/↑↓ select · a/Esc close", muted)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::catalog::Catalog;
    use crate::herdr::AgentSnapshot;
    use crate::settings::Settings;
    use crate::theme::ThemeName;

    fn snap(pane: &str, name: &str, status: AgentStatus) -> AgentSnapshot {
        AgentSnapshot {
            pane_id: pane.to_string(),
            agent: Some("claude".to_string()),
            name: Some(name.to_string()),
            cwd: Some("~/radio".to_string()),
            status,
        }
    }

    fn pulse_app(agents: Vec<AgentSnapshot>) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents,
            now: Instant::now(),
        });
        app
    }

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn summary_counts_use_the_specified_quiet_copy() {
        let app = pulse_app(vec![
            snap("pane-a", "alpha", AgentStatus::Working),
            snap("pane-b", "beta", AgentStatus::Working),
            snap("pane-c", "gamma", AgentStatus::Idle),
        ]);
        let theme = Theme::for_name(ThemeName::Minimal);
        let line = summary_line(&app, &theme).expect("connected summary");
        assert_eq!(line_text(&line), "● 2 working · ○ 1 idle");
    }

    #[test]
    fn summary_is_absent_when_hidden_or_unavailable() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let hidden = App::new(Settings::default(), Catalog::curated());
        assert!(summary_line(&hidden, &theme).is_none());

        let mut unavailable = pulse_app(vec![snap("pane-a", "alpha", AgentStatus::Working)]);
        // Well past the stale threshold measured from the snapshot above.
        unavailable.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert!(summary_line(&unavailable, &theme).is_none());
    }

    #[test]
    fn observed_duration_formats_as_calm_estimates() {
        assert_eq!(format_observed_duration(Duration::from_secs(30)), "<1m");
        assert_eq!(
            format_observed_duration(Duration::from_secs(12 * 60 + 5)),
            "~12m"
        );
        assert_eq!(
            format_observed_duration(Duration::from_secs(2 * 3600 + 120)),
            "~2h"
        );
    }

    #[test]
    fn low_power_motion_is_static_and_normal_motion_pulses() {
        // Low power never dims: static nodes regardless of elapsed time.
        for secs in 0..600 {
            assert!(
                !working_pulse_dim(Duration::from_secs(secs), true),
                "low-power motion must be static at {secs}s"
            );
        }
        // Normal motion alternates between both phases over a slow cycle.
        let phases: std::collections::HashSet<bool> = (0..8)
            .map(|secs| working_pulse_dim(Duration::from_secs(secs), false))
            .collect();
        assert_eq!(phases.len(), 2, "normal motion must pulse");
    }

    #[test]
    fn node_styles_stay_quiet_and_theme_sourced() {
        let theme = Theme::for_name(ThemeName::Minimal);
        // Settled ages on opposite pulse phases, outside the highlight window.
        let bright = Duration::from_secs(4);
        let dimmed = Duration::from_secs(6);

        // Working carries the playing color and only its pulse phase dims it.
        assert_eq!(
            node_style(AgentStatus::Working, &theme, bright, false).fg,
            Some(theme.playing)
        );
        assert!(node_style(AgentStatus::Working, &theme, dimmed, false)
            .add_modifier
            .contains(Modifier::DIM));
        // Low-power working nodes are static across phases.
        assert_eq!(
            node_style(AgentStatus::Working, &theme, bright, true),
            node_style(AgentStatus::Working, &theme, dimmed, true)
        );
        // Blocked is static: the pulse phase never changes it.
        assert_eq!(
            node_style(AgentStatus::Blocked, &theme, bright, false),
            node_style(AgentStatus::Blocked, &theme, dimmed, false)
        );
        assert_eq!(
            node_style(AgentStatus::Blocked, &theme, bright, false).fg,
            Some(theme.error)
        );
        // Done dims.
        assert!(node_style(AgentStatus::Done, &theme, bright, false)
            .add_modifier
            .contains(Modifier::DIM));
    }

    #[test]
    fn fresh_status_change_is_acknowledged_once_then_expires() {
        let theme = Theme::for_name(ThemeName::Minimal);
        // Inside the window the node is emphasized; after it, never again.
        assert!(recent_change_highlight(Duration::from_secs(1)));
        assert!(!recent_change_highlight(STATUS_CHANGE_HIGHLIGHT));
        assert!(
            node_style(AgentStatus::Blocked, &theme, Duration::from_secs(1), false)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !node_style(AgentStatus::Blocked, &theme, Duration::from_secs(30), false)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn clicking_a_named_list_row_selects_that_pane() {
        let mut app = pulse_app(vec![
            snap("pane-a", "alpha", AgentStatus::Working),
            snap("pane-b", "beta", AgentStatus::Blocked),
        ]);
        app.apply(Action::ToggleAgentOverlay);

        let area = Rect::new(0, 0, 130, 32);
        let mut buf = Buffer::empty(area);
        super::super::render_into(&app, false, Instant::now(), area, &mut buf);
        let row = (0..area.height)
            .find(|&y| {
                (0..area.width)
                    .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                    .collect::<String>()
                    .contains("beta")
            })
            .expect("beta list row rendered");

        let layout = overlay_layout(&app, area).expect("overlay layout");
        let action = hit_test(area, layout.overlay.x + 2, row, &app);
        match action {
            Some(Action::SelectAgent(pane)) => assert_eq!(pane, "pane-b"),
            other => panic!("expected SelectAgent(pane-b), got {other:?}"),
        }
    }
}
