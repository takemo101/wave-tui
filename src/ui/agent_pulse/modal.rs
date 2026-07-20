//! The Agent table modal and the transient focus notice.
//!
//! Both are centered overlays inside the stage field, read-only over
//! [`App`], and independent of planet geometry. The table lists every active
//! agent in the same sorted display order as their planets and styles the
//! shared selection through a Ratatui `TableState`; the notice reports a
//! focus or rename result without any agent metadata or identifier.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Span,
    widgets::{
        Block, Borders, Cell, Clear, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
    },
};
use std::time::Instant;

use crate::app::App;
use crate::herdr::AgentStatus;
use crate::theme::Theme;

// --- status presentation helpers -------------------------------------------

/// Short lowercase state label for the selected-frame line.
fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// Truncate one table cell without dropping a column at narrow widths.
fn ellipsize_cell(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        value.to_string()
    } else if width <= 1 {
        "…".to_string()
    } else {
        let mut truncated: String = value.chars().take(width - 1).collect();
        truncated.push('…');
        truncated
    }
}

/// The Agent table's fixed responsive column proportions. They deliberately
/// keep every approved field available, even in the narrowest modal.
pub(super) const AGENT_TABLE_WIDTHS: [Constraint; 4] = [
    Constraint::Percentage(25),
    Constraint::Percentage(20),
    Constraint::Percentage(15),
    Constraint::Percentage(40),
];
const AGENT_TABLE_MAX_ROWS: usize = 10;

/// Resolve the actual Ratatui column widths so cell text can ellipsize before
/// rendering. This uses the exact constraints and spacing passed to `Table`.
fn agent_table_widths(inner_width: u16) -> [usize; 4] {
    let columns = Layout::horizontal(AGENT_TABLE_WIDTHS)
        .spacing(1)
        .split(Rect::new(0, 0, inner_width, 1));
    std::array::from_fn(|index| columns[index].width as usize)
}

/// One data or header row, clipped to its responsive Ratatui column width.
fn agent_table_row(cells: [&str; 4], widths: [usize; 4]) -> Row<'static> {
    Row::new(
        cells
            .into_iter()
            .zip(widths)
            .map(|(value, width)| Cell::from(ellipsize_cell(value, width)))
            .collect::<Vec<_>>(),
    )
}

/// Center a table that takes 90% of its field, capped at 100 cells, and
/// reserves only ten scrolling data rows plus fixed table chrome.
pub(super) fn agent_table_modal_area(field: Rect, agent_count: usize) -> Rect {
    let width = (field.width.saturating_mul(90) / 100).clamp(12, 100);
    let width = width.min(field.width);
    // Top border + header + up to ten scrolling rows + a dedicated modal
    // footer + bottom border. The table itself scrolls enough rows to keep
    // selection visible through `TableState`.
    let height = (agent_count.min(AGENT_TABLE_MAX_ROWS) as u16 + 4)
        .min(field.height)
        .max(5);
    Rect::new(
        field.x + field.width.saturating_sub(width) / 2,
        field.y + field.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Render the centered, read-only table of every active agent in the same
/// sorted display order as their planets. A Ratatui `TableState` keeps the
/// shared selection visible in its ten-row viewport without a marker glyph.
pub(super) fn render_agent_table_modal(
    app: &App,
    theme: &Theme,
    stale: bool,
    field: Rect,
    now: Instant,
    buf: &mut Buffer,
) {
    if !app.is_agent_details_open() {
        render_agent_focus_notice(app, theme, field, now, buf);
        return;
    }
    if field.width < 12 || field.height < 5 {
        return;
    }

    let agents = app.active_agents();
    let notice = app.agent_focus_notice(now);
    let area = agent_table_modal_area(field, agents.len());
    let widths = agent_table_widths(area.width.saturating_sub(2));
    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));
    let rows = agents.iter().map(|view| {
        agent_table_row(
            [
                view.details.name.as_deref().unwrap_or("—"),
                view.details.agent.as_deref().unwrap_or("—"),
                status_label(view.status),
                view.details.activity.as_deref().unwrap_or("—"),
            ],
            widths,
        )
    });
    let mut muted = Style::default().fg(theme.muted);
    if stale {
        muted = muted.add_modifier(Modifier::DIM);
    }
    let footer = if let Some(input) = app.agent_rename_input() {
        let mut prompt = format!("Name: {input}");
        if stale {
            prompt.push_str(" · reconnecting");
        } else if app.agent_rename_is_submitting() {
            prompt.push_str(" · saving");
        } else if let Some(rename_notice) = app.agent_rename_notice(now) {
            prompt.push_str(" · ");
            prompt.push_str(rename_notice);
        } else {
            prompt.push_str(" · Enter save");
        }
        prompt
    } else {
        notice
            .map(str::to_owned)
            .unwrap_or_else(|| "O open pane · r rename · Enter/Esc close".to_string())
    };
    let header = agent_table_row(["Name", "Agent", "Status", "Activity"], widths)
        .style(muted.add_modifier(Modifier::BOLD));
    let mut title = Style::default().fg(theme.accent);
    if stale {
        title = title.add_modifier(Modifier::DIM);
    }
    let highlight = if stale {
        theme.selection_style().add_modifier(Modifier::DIM)
    } else {
        theme.selection_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(title)
        .title(Span::styled(
            if stale {
                " Agent table · reconnecting "
            } else {
                " Agent table "
            },
            title,
        ));
    let table = Table::new(rows, AGENT_TABLE_WIDTHS)
        .header(header)
        .column_spacing(1)
        .style(if stale {
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(theme.foreground)
        })
        .row_highlight_style(highlight);

    Clear.render(area, buf);
    block.render(area, buf);
    let table_area = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    let mut state = TableState::default();
    state.select(selected_index);
    StatefulWidget::render(table, table_area, buf, &mut state);

    // This is deliberately outside `Table::footer`: it spans the entire
    // modal interior below the columns while staying inside the outer border.
    let footer_area = Rect::new(
        area.x + 1,
        area.y + area.height - 2,
        area.width.saturating_sub(2),
        1,
    );
    Paragraph::new(footer)
        .alignment(Alignment::Center)
        .style(muted)
        .render(footer_area, buf);
}

/// A temporary centered focus result when the Agent table is not open.
/// It has no agent metadata or identifiers and never changes selection.
pub(super) fn render_agent_focus_notice(
    app: &App,
    theme: &Theme,
    field: Rect,
    now: Instant,
    buf: &mut Buffer,
) {
    let Some(notice) = app.agent_focus_notice(now) else {
        return;
    };
    if field.width < 18 || field.height < 3 {
        return;
    }
    let width = field.width.min(42);
    let area = Rect::new(
        field.x + field.width.saturating_sub(width) / 2,
        field.y + field.height.saturating_sub(3) / 2,
        width,
        3,
    );
    Clear.render(area, buf);
    Paragraph::new(notice)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.error))
                .title(Span::styled(
                    " Agent Planets ",
                    Style::default().fg(theme.accent),
                )),
        )
        .style(Style::default().fg(theme.foreground))
        .render(area, buf);
}
