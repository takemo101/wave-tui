//! Ratatui rendering only; domain mutation lives in [`crate::app`].
//!
//! This module renders the application from read-only [`App`] display data. It
//! never mutates app or domain state: rendering asks `App` for the visible
//! stations, selection, playback, visualizer frame, theme, and offline flag, and
//! draws them. Key handling, the terminal event loop, and search input live in
//! later tasks (`cli`/controller).
//!
//! Layout follows the [`LayoutTier`] policy from [`crate::layout`]:
//!
//! - [`LayoutTier::Wide`] renders the **Search Console** (search-first, ranked
//!   results as the largest region, sections + Now Playing + visualizer visible).
//! - [`LayoutTier::Medium`] and [`LayoutTier::Compact`] render **Split Mini**:
//!   both station context and Now Playing stay visible; compact never becomes a
//!   full-screen visualizer.
//!
//! All colors come from the active [`Theme`]; this module hard-codes no palette
//! values. Visualizer rendering (the shared "Spectrum Stack" and the other
//! Calm Suite modes) lives in the [`visualizer`] submodule.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, StatefulWidget,
        Table, TableState, Widget, Wrap,
    },
    Frame,
};

use std::time::Instant;

use crate::app::{Action, AgentPulseConnection, App, FocusPane, ListSource, SearchStatus};
use crate::layout::LayoutTier;
use crate::model::{CodecKind, PlaybackState, Station};
use crate::theme::Theme;

mod agent_pulse;
mod splash;
mod visualizer;

pub(crate) use splash::{SplashKind, SplashTiming};

/// Resolve splash timing for `kind`, picking the low-power budget when set.
///
/// A narrow wrapper so the CLI lifecycle can size the splash loop without
/// importing the private `splash` submodule directly.
pub(crate) fn splash_timing(kind: SplashKind, low_power: bool) -> SplashTiming {
    if low_power {
        splash::low_power_timing(kind)
    } else {
        splash::normal_timing(kind)
    }
}

/// Render one splash frame for `kind` at `tick` into the whole `frame` area.
///
/// Deterministic, theme-driven, and independent of app state; used by the CLI
/// startup/shutdown splash loop.
pub(crate) fn render_splash(kind: SplashKind, theme: &Theme, tick: u16, frame: &mut Frame) {
    splash::render_splash_into(kind, theme, tick, frame.area(), frame.buffer_mut());
}

/// Render the entire UI for the current terminal size.
///
/// This is the only entry point the controller/event loop calls each frame. It
/// reads display data from `app` and draws into the frame's buffer; it performs
/// no state mutation. `low_power` is the controller's `--low-power` flag: it
/// freezes the Agent Planets scope background and disc/tag geometry,
/// mirroring how the splash loop receives its low-power timing, without adding
/// persistent app state.
pub fn render(app: &App, low_power: bool, frame: &mut Frame) {
    render_into(
        app,
        low_power,
        Instant::now(),
        frame.area(),
        frame.buffer_mut(),
    );
}

/// Pure hit test for Agent Pulse mouse input.
///
/// Maps a click at (`column`, `row`) within the rendered `area` to the
/// read-only Agent Planets planet selection and `None` for every
/// other click.
/// The CLI event loop owns applying the returned action; this function never
/// mutates `App` and never returns playback, station, search, or settings
/// actions.
///
/// Selection requires a live view, so a stale, unavailable, or hidden
/// integration resolves no clicks either; the geometry lives in
/// [`agent_pulse`], shared with the canvas renderer. `low_power` must be the
/// same controller flag passed to [`render`], so clicks resolve against
/// exactly the tile rectangles that were drawn — frozen base geometry in low
/// power, audio-transformed geometry otherwise.
pub(crate) fn agent_pulse_hit_test(
    area: Rect,
    column: u16,
    row: u16,
    low_power: bool,
    app: &App,
) -> Option<Action> {
    agent_pulse::hit_test(area, column, row, low_power, app)
}

/// Render into an explicit area and buffer.
///
/// Split out from [`render`] so tests can drive rendering with a standalone
/// [`Buffer`], no real terminal or backend, and an injected `now` — rendering
/// itself never reads the clock, which keeps Agent Pulse motion deterministic
/// under test.
fn render_into(app: &App, low_power: bool, now: Instant, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme().theme();
    // Paint the themed canvas first so empty cells carry the background.
    buf.set_style(area, theme.base_style());

    // Signal View bypasses the normal Wide/Medium/Compact composition entirely:
    // it is an opt-in full-area visual-player surface, not a layout tier.
    if app.is_signal_view() {
        render_signal_view(app, &theme, area, buf);
        return;
    }

    match LayoutTier::from_size(area.width, area.height) {
        LayoutTier::Wide => render_wide(app, &theme, area, buf),
        LayoutTier::Medium => render_medium(app, &theme, area, buf),
        LayoutTier::Compact => render_compact(app, &theme, area, buf),
    }

    // The full-screen Agent Planets stage draws over the composed
    // normal layout; a no-op for standalone/hidden launches and while it is
    // closed.
    agent_pulse::render_canvas(app, &theme, low_power, now, area, buf);
}

// --- tier layouts --------------------------------------------------------

/// Wide "Search Console": search strip on top, a three-column body (browse
/// shortcuts, ranked results, Now Playing + visualizer), and a key-hint footer.
fn render_wide(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_search_strip(app, theme, rows[0], buf);

    let cols = Layout::horizontal([
        Constraint::Length(20),
        Constraint::Min(24),
        Constraint::Length(36),
    ])
    .split(rows[1]);

    render_browse(app, theme, cols[0], buf);
    render_station_list(app, theme, cols[1], buf, "Results", false);
    render_now_playing(app, theme, cols[2], buf, false);

    render_footer(app, theme, rows[2], buf, false);
}

/// Medium "Split Mini": search strip, a balanced list + Now Playing split, and a
/// footer. Both browsing and playback stay visible.
fn render_medium(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_search_strip(app, theme, rows[0], buf);

    let cols = Layout::horizontal([Constraint::Min(20), Constraint::Length(34)]).split(rows[1]);

    render_station_list(app, theme, cols[0], buf, "Stations", false);
    render_now_playing(app, theme, cols[1], buf, false);

    render_footer(app, theme, rows[2], buf, false);
    render_hidden_browse_modal(app, theme, area, buf);
}

/// Compact "Split Mini, reduced": a one-line search strip, a stacked station
/// list and compact Now Playing/visualizer, and a one-line status footer. It
/// keeps limited station context plus playback visible rather than going
/// full-screen visualizer.
fn render_compact(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(6),
        Constraint::Length(1),
    ])
    .split(area);

    render_search_strip(app, theme, rows[0], buf);
    render_station_list(app, theme, rows[1], buf, "Stations", true);
    render_now_playing(app, theme, rows[2], buf, true);
    render_footer(app, theme, rows[3], buf, true);
    render_hidden_browse_modal(app, theme, area, buf);
}

/// Draw the Browse source picker as a modal in non-wide layouts.
///
/// Medium and Compact intentionally do not reserve permanent space for Browse,
/// but the focus model still lets `Tab` land on it. Showing the same Browse
/// renderer in an overlay keeps the focused control visible without stealing
/// station/player space when Browse is not focused.
fn render_hidden_browse_modal(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if app.focus() != FocusPane::Sections || area.width == 0 || area.height == 0 {
        return;
    }

    let rail_rows = ListSource::browse_rail().len() as u16;
    let desired_width = 36;
    let desired_height = rail_rows.saturating_add(2);
    let width = if area.width <= desired_width {
        area.width
    } else {
        desired_width.min(area.width.saturating_sub(4))
    };
    let height = if area.height <= desired_height {
        area.height
    } else {
        desired_height.min(area.height.saturating_sub(2))
    };
    let modal = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );

    Clear.render(modal, buf);
    buf.set_style(modal, theme.base_style());
    render_browse(app, theme, modal, buf);
}

// --- Signal View (Quiet Signal) -----------------------------------------

/// Render the opt-in **Quiet Signal** visual-player surface for the current
/// station.
///
/// This bypasses the normal layout tiers: a thin mode/status label, a centered
/// title block (constrained to two lines), the largest flexible region for the
/// shared visualizer, and a subdued key-hint footer. Lower-priority rows (label,
/// hint) are dropped first on small terminals so the title and visualizer keep
/// the Center Stage composition rather than falling back to the compact UI.
/// Rendering stays read-only and reuses [`visualizer::render_visualizer`] over a
/// large `Rect`; it introduces no Signal View-specific visualizer.
fn render_signal_view(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Reserve the optional metadata label and footer hint only when there is
    // vertical room; the visualizer always takes the largest flexible region.
    let show_label = area.height >= 9;
    let show_hint = area.height >= 4;
    let title_h = if area.height >= 12 {
        5
    } else if area.height >= 7 {
        3
    } else {
        2
    };

    let mut constraints = Vec::new();
    if show_label {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(title_h));
    constraints.push(Constraint::Min(1));
    if show_hint {
        constraints.push(Constraint::Length(1));
    }

    let rows = Layout::vertical(constraints).split(area);
    let mut next = 0;
    if show_label {
        render_signal_view_label(app, theme, rows[next], buf);
        next += 1;
    }
    render_signal_view_title(app, theme, rows[next], buf);
    next += 1;
    visualizer::render_visualizer(theme, app, rows[next], buf);
    next += 1;
    if show_hint {
        render_signal_view_hint(theme, rows[next], buf);
    }
}

/// The thin `Signal View · <status>` mode label, centered and accented.
fn render_signal_view_label(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let status = signal_view_status_label(app.playback());
    Paragraph::new(Line::from(vec![
        Span::styled("Signal View", theme.accent_style()),
        Span::styled(format!(" · {status}"), Style::default().fg(theme.muted)),
    ]))
    .alignment(Alignment::Center)
    .style(theme.base_style())
    .render(area, buf);
}

/// Centered title block: an optional station + favorite metadata line, the
/// primary title (at most two lines), and an optional low-priority volume line.
/// Lower-priority rows are only added when the block has vertical room, so a
/// short title area still shows the title itself.
fn render_signal_view_title(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    Paragraph::new(signal_view_title_lines(app, theme, area.width, area.height))
        .alignment(Alignment::Center)
        .style(theme.base_style())
        .render(area, buf);
}

/// Build the centered Signal View title block lines without mutating the buffer.
fn signal_view_title_lines<'a>(app: &App, theme: &Theme, width: u16, height: u16) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    // Station name + subtle favorite star, above the title, when a current
    // station exists and the block is tall enough to spare the row. Match the
    // station-list language: show `★` only for actual favorites and show no
    // empty marker otherwise.
    if let Some(station) = app.current_station() {
        if height >= 3 {
            let mut spans = vec![Span::styled(
                station.name.as_str().to_string(),
                Style::default().fg(theme.muted),
            )];
            if app.current_station_is_favorite() {
                spans.push(Span::styled(" ★", Style::default().fg(theme.accent)));
            }
            lines.push(Line::from(spans));
        }
    }

    // Primary title, constrained to at most two lines so a long ICY title or
    // station name can never push the visualizer or hint out of view.
    for line in title_lines(&signal_view_primary_title(app), width) {
        lines.push(Line::from(Span::styled(
            line,
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )));
    }

    // Lowest-priority metadata: a thin, wide volume bar. Dropped first when the
    // title block is short.
    if app.current_station().is_some() && height >= 4 {
        lines.push(signal_view_volume_line(app, theme, width));
    }

    lines
}

/// Build a calm, near-full-width volume line for Signal View.
fn signal_view_volume_line<'a>(app: &App, theme: &Theme, width: u16) -> Line<'a> {
    let volume = app.settings().volume.get();
    let prefix = format!("volume {volume}% ");
    let bar_width = (width as usize)
        .saturating_sub(prefix.chars().count())
        .max(4);
    let filled = ((bar_width * volume as usize) + 50) / 100;
    let empty = bar_width.saturating_sub(filled);

    Line::from(vec![
        Span::styled(prefix, Style::default().fg(theme.muted)),
        Span::styled("─".repeat(filled), Style::default().fg(theme.accent)),
        Span::styled("·".repeat(empty), Style::default().fg(theme.muted)),
    ])
}

/// The subdued, low-contrast key-hint footer.
fn render_signal_view_hint(theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    Paragraph::new("z/Esc back · Space play · v visual · +/- volume · q quit")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted).bg(theme.background))
        .render(area, buf);
}

/// Primary title priority: ICY now-playing title, then current station name,
/// then the idle prompt when there is no current station.
fn signal_view_primary_title(app: &App) -> String {
    if let Some(title) = app.now_playing_title() {
        title.to_string()
    } else if let Some(station) = app.current_station() {
        station.name.as_str().to_string()
    } else {
        "Select a station, then press z".to_string()
    }
}

/// Compact playback-status label for the Signal View mode line.
fn signal_view_status_label(state: &PlaybackState) -> &'static str {
    match state {
        PlaybackState::Stopped => "stopped",
        PlaybackState::Connecting => "connecting",
        PlaybackState::Playing => "playing",
        PlaybackState::Failed(_) => "failed",
    }
}

/// Greedily wrap `text` into lines no wider than `width` characters, hard-
/// splitting any single word longer than the full width. A pure helper shared by
/// [`title_lines`].
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for raw in text.split_whitespace() {
        let mut word = raw.to_string();
        // Break a word that cannot fit on one line into width-sized chunks.
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(width).collect();
            lines.push(head);
            word = word.chars().skip(width).collect();
        }
        if word.is_empty() {
            continue;
        }
        if line.is_empty() {
            line = word;
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut line));
            line = word;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Constrain `text` to at most two centered lines that each fit `width`,
/// ellipsizing any overflow. Keeping the title to two lines is what prevents a
/// long ICY title from breaking the Signal View layout.
fn title_lines(text: &str, width: u16) -> Vec<String> {
    let width = width.max(1) as usize;
    let mut wrapped = wrap_words(text, width);
    if wrapped.len() <= 2 {
        return wrapped;
    }
    wrapped.truncate(2);
    // Make room for the ellipsis on the trailing line, then mark the overflow.
    let second = &mut wrapped[1];
    if second.chars().count() >= width {
        *second = second.chars().take(width.saturating_sub(1)).collect();
    }
    second.push('…');
    wrapped
}

// --- components ----------------------------------------------------------

/// Search input + state strip.
///
/// Emphasizes the search workflow: a search prompt, the live query text,
/// loading/cache/offline state, the visible result count, and the network
/// signal from the display data `App` exposes today.
fn render_search_strip(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = search_line(app, theme);
    if area.height >= 3 {
        let focused = app.focus() == FocusPane::Search;
        let block = bordered_block(theme, "Search", focused);
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(line)
            .style(theme.base_style())
            .render(inner, buf);
    } else {
        Paragraph::new(line)
            .style(theme.base_style())
            .render(area, buf);
    }
}

/// Build the search strip content line from read-only app data.
///
/// Shows the live query text (or a placeholder when empty), the search status
/// (loading / cache-or-fresh / offline / error), the visible result count, and
/// the network signal.
fn search_line<'a>(app: &App, theme: &Theme) -> Line<'a> {
    let muted = Style::default().fg(theme.muted);
    let query = app.search_query();
    let query_span = if query.is_empty() {
        Span::styled("type to search Radio Browser", muted)
    } else {
        Span::styled(
            query.to_string(),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        )
    };

    let mut spans = vec![
        Span::styled("/ ", theme.accent_style()),
        query_span,
        Span::raw("   "),
        search_status_span(app, theme),
        Span::raw("  "),
        Span::styled(
            format!("{} results", app.visible().len()),
            Style::default().fg(theme.foreground),
        ),
    ];

    // When Browse is filtering the current search-result population, show the
    // active filter context so the strip explains the narrowed list.
    if let Some(label) = app.active_filter_label() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!("filter: {label}"),
            theme.accent_style(),
        ));
    }

    spans.push(Span::raw("   "));
    spans.push(network_span(app, theme));
    Line::from(spans)
}

/// A search-status span (loading / cached / fresh / offline / error), themed.
fn search_status_span<'a>(app: &App, theme: &Theme) -> Span<'a> {
    match app.search_status() {
        SearchStatus::Idle => Span::styled("idle", Style::default().fg(theme.muted)),
        SearchStatus::Loading => Span::styled(
            "loading…",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        SearchStatus::Loaded { from_cache: true } => {
            Span::styled("cached", Style::default().fg(theme.muted))
        }
        SearchStatus::Loaded { from_cache: false } => {
            Span::styled("fresh", Style::default().fg(theme.playing))
        }
        SearchStatus::Offline => Span::styled(
            "offline",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ),
        SearchStatus::Error(message) => Span::styled(
            format!("error: {message}"),
            Style::default().fg(theme.error),
        ),
    }
}

/// Browse source rail: the Wide-tier "Quiet Source Rail" left column.
///
/// Renders the flat, actionable source picker — All Stations, Favorites, each
/// section, and every category — from [`ListSource::browse_rail`]. Two pieces of
/// state are shown distinctly: the Browse *selection* (the row the rail cursor is
/// on, drawn with the selection highlight) and the *active* source (the source
/// the visible list is built from, marked with a filled dot). Applying a
/// selected source and moving the cursor are keyboard concerns owned by a later
/// slice; this renderer only reflects current app state.
fn render_browse(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let focused = app.focus() == FocusPane::Sections;
    let block = bordered_block(theme, "Browse", focused);

    let rail = ListSource::browse_rail();
    let active = app.active_source();
    let items: Vec<ListItem> = rail
        .iter()
        .map(|source| browse_item(theme, *source, *source == active))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_style(theme.selection_style());

    let mut state = ListState::default();
    state.select(Some(
        app.browse_selected().min(rail.len().saturating_sub(1)),
    ));
    StatefulWidget::render(list, area, buf, &mut state);
}

/// Build one Browse rail row: categories indent under their section, and the
/// active source carries a filled-dot marker in the playing color so it reads as
/// applied even when it is not the current Browse selection.
fn browse_item<'a>(theme: &Theme, source: ListSource, active: bool) -> ListItem<'a> {
    let indent = if matches!(source, ListSource::Category(_)) {
        "  "
    } else {
        ""
    };
    let marker = if active { "● " } else { "  " };
    let label_style = if active {
        Style::default()
            .fg(theme.playing)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground)
    };
    ListItem::new(Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.playing)),
        Span::styled(format!("{indent}{}", source.title()), label_style),
    ]))
}

/// Station / search-result list with selected, favorite, and failed markers.
///
/// Reads the visible stations, selection index, favorite state, and session
/// failed state from `app`. The selected row is highlighted; favorites are
/// starred; session-failed stations are dimmed and struck through.
fn render_station_list(
    app: &App,
    theme: &Theme,
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    compact: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let focused = app.focus() == FocusPane::Stations;
    let block = bordered_block(
        theme,
        &format!("{title} ({})", app.visible().len()),
        focused,
    );

    if app.visible().is_empty() {
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(Line::styled(
            empty_list_note(app),
            Style::default().fg(theme.muted),
        ))
        .style(theme.base_style())
        .render(inner, buf);
        return;
    }

    let table_allowed = title == "Results" && !compact;
    match station_list_presentation(area.width, table_allowed) {
        StationListPresentation::FullTable => {
            render_station_table(app, theme, area, buf, block, StationTableDensity::Full);
        }
        StationListPresentation::CompactTable => {
            render_station_table(app, theme, area, buf, block, StationTableDensity::Compact);
        }
        StationListPresentation::List => {
            let items: Vec<ListItem> = app
                .visible()
                .iter()
                .map(|station| station_item(app, theme, station, compact))
                .collect();

            let list = List::new(items)
                .block(block)
                .highlight_symbol("▶ ")
                .highlight_style(theme.selection_style());

            let mut state = ListState::default();
            state.select(Some(app.selected_index()));
            StatefulWidget::render(list, area, buf, &mut state);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StationListPresentation {
    FullTable,
    CompactTable,
    List,
}

fn station_list_presentation(width: u16, table_allowed: bool) -> StationListPresentation {
    if !table_allowed || width < 48 {
        StationListPresentation::List
    } else if width < 68 {
        StationListPresentation::CompactTable
    } else {
        StationListPresentation::FullTable
    }
}

#[derive(Debug, Clone, Copy)]
enum StationTableDensity {
    Full,
    Compact,
}

fn render_station_table(
    app: &App,
    theme: &Theme,
    area: Rect,
    buf: &mut Buffer,
    block: Block,
    density: StationTableDensity,
) {
    let header_style = Style::default()
        .fg(theme.muted)
        .add_modifier(Modifier::BOLD);
    let header = match density {
        StationTableDensity::Full => Row::new(vec!["", "Station", "Codec", "Rate", "Locale"]),
        StationTableDensity::Compact => Row::new(vec!["", "Station", "Meta"]),
    }
    .style(header_style);

    let rows = app
        .visible()
        .iter()
        .enumerate()
        .map(|(index, station)| station_table_row(app, theme, station, index, density));

    let widths: Vec<Constraint> = match density {
        StationTableDensity::Full => vec![
            Constraint::Length(3),
            Constraint::Min(18),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(8),
        ],
        StationTableDensity::Compact => vec![
            Constraint::Length(3),
            Constraint::Min(18),
            Constraint::Length(16),
        ],
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme.selection_style());

    let mut state = TableState::default();
    state.select(Some(app.selected_index()));
    StatefulWidget::render(table, area, buf, &mut state);
}

fn station_table_row<'a>(
    app: &App,
    theme: &Theme,
    station: &Station,
    index: usize,
    density: StationTableDensity,
) -> Row<'a> {
    let favorite = app.is_favorite(station);
    let selected = index == app.selected_index();
    let failed = app.is_failed(&station.id);

    let marker = match (selected, favorite) {
        (true, true) => "▶★",
        (true, false) => "▶ ",
        (false, true) => " ★",
        (false, false) => "  ",
    };
    let station_name = if failed {
        format!("{} ✗", station.name.as_str())
    } else {
        station.name.as_str().to_string()
    };
    let name_style = if failed {
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default().fg(theme.foreground)
    };

    match density {
        StationTableDensity::Full => Row::new(vec![
            Cell::from(marker.to_string()).style(Style::default().fg(theme.accent)),
            Cell::from(station_name).style(name_style),
            Cell::from(codec_label(&station.codec).to_string())
                .style(Style::default().fg(theme.muted)),
            Cell::from(station_bitrate(station)).style(Style::default().fg(theme.muted)),
            Cell::from(station_location(station).unwrap_or_else(|| "—".to_string()))
                .style(Style::default().fg(theme.muted)),
        ]),
        StationTableDensity::Compact => Row::new(vec![
            Cell::from(marker.to_string()).style(Style::default().fg(theme.accent)),
            Cell::from(station_name).style(name_style),
            Cell::from(station_table_meta(station)).style(Style::default().fg(theme.muted)),
        ]),
    }
}

/// Build one station row: favorite star, name (dimmed if failed), failed mark,
/// and optional metadata when not compact.
fn station_item<'a>(app: &App, theme: &Theme, station: &Station, compact: bool) -> ListItem<'a> {
    let favorite = app.is_favorite(station);
    let failed = app.is_failed(&station.id);

    let mut spans = vec![Span::styled(
        if favorite { "★ " } else { "  " },
        Style::default().fg(theme.accent),
    )];

    let name_style = if failed {
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default().fg(theme.foreground)
    };
    spans.push(Span::styled(station.name.as_str().to_string(), name_style));

    if failed {
        spans.push(Span::styled(" ✗", Style::default().fg(theme.error)));
    }

    if !compact {
        spans.push(Span::styled(
            format!("  {}", station_meta(station)),
            Style::default().fg(theme.muted),
        ));
    }

    ListItem::new(Line::from(spans))
}

/// Now Playing metadata plus the shared Spectrum Stack visualizer.
///
/// `compact` drops the pane border and trims metadata + visualizer height so the
/// player stays visible beside the list in small terminals.
fn render_now_playing(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer, compact: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let inner = if compact {
        area
    } else {
        let focused = app.focus() == FocusPane::NowPlaying;
        let block = bordered_block(theme, "Now Playing", focused);
        let inner = block.inner(area);
        block.render(area, buf);
        inner
    };
    if inner.height == 0 {
        return;
    }

    // Reserve the lower region for the visualizer; metadata takes the rest.
    let spectrum_h = if compact {
        inner.height.saturating_sub(2).min(4)
    } else {
        (inner.height / 2).min(10)
    };
    let parts = Layout::vertical([Constraint::Min(0), Constraint::Length(spectrum_h)]).split(inner);

    Paragraph::new(now_playing_lines(app, theme, compact))
        .wrap(Wrap { trim: true })
        .style(theme.base_style())
        .render(parts[0], buf);

    visualizer::render_visualizer(theme, app, parts[1], buf);
}

/// Build the Now Playing metadata lines from read-only app data.
fn now_playing_lines<'a>(app: &App, theme: &Theme, compact: bool) -> Vec<Line<'a>> {
    let muted = Style::default().fg(theme.muted);
    let mut lines = Vec::new();

    match app.current_station() {
        Some(station) => {
            lines.push(Line::styled(
                station.name.as_str().to_string(),
                theme.accent_style(),
            ));
            // ICY now-playing title takes priority above station metadata; when
            // absent the station-level fields below remain the only source.
            if let Some(title) = app.now_playing_title() {
                lines.push(Line::styled(
                    title.to_string(),
                    Style::default()
                        .fg(theme.foreground)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(playback_span(theme, app.playback())));

            if !compact {
                if let Some(location) = station_location(station) {
                    lines.push(Line::styled(location, muted));
                }
                if !station.tags.is_empty() {
                    lines.push(Line::styled(station.tags.join(", "), muted));
                }
            }

            lines.push(Line::styled(station_meta(station), muted));
        }
        None => {
            lines.push(Line::styled("No station playing", muted));
            if !compact {
                lines.push(Line::styled("Press Enter to play a station", muted));
            }
        }
    }

    let gauge_width = if compact {
        VOLUME_GAUGE_WIDTH_COMPACT
    } else {
        VOLUME_GAUGE_WIDTH
    };
    lines.push(volume_gauge_line(
        theme,
        app.settings().volume.get(),
        gauge_width,
    ));

    // Quiet Companion: the Agent Pulse state-count summary lives in the
    // bordered Wide/Medium panes only; the compact pane keeps its trimmed
    // Split Mini metadata, and hidden/unavailable states reserve no row.
    if !compact {
        if let Some(line) = agent_pulse::summary_line(app, theme) {
            lines.push(line);
        }
    }
    lines
}

/// Cells in the Now Playing volume gauge for the bordered (wide/medium) panes.
const VOLUME_GAUGE_WIDTH: usize = 10;
/// A shorter gauge for the compact pane, which trims metadata to stay legible.
const VOLUME_GAUGE_WIDTH_COMPACT: usize = 6;

/// Filled cell count for `volume` (0..=100) across a `width`-cell gauge.
///
/// Rounds to the nearest cell so mid values read proportionally, and clamps into
/// `0..=width` so 0% is empty, 100% is full, and a zero-width gauge is safe.
fn volume_filled_cells(volume: u8, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    let filled = (volume as f32 / 100.0 * width as f32).round() as usize;
    filled.min(width)
}

/// Build the themed Now Playing volume gauge: a muted label, a filled bar, an
/// empty bar, and a highlighted percentage.
///
/// The "Pane Gauge" indicator (MIK-033) replaces the plain `Volume N%` text with
/// a visual level bar. Every span draws its color/weight from the active
/// [`Theme`] (muted label/empty, accent fill, bold-highlighted percentage), so
/// this carries no hard-coded palette values. The fill uses the `playing`
/// (output-level) color rather than the heading/focus accent, since the gauge
/// represents playback level.
fn volume_gauge_line<'a>(theme: &Theme, volume: u8, width: usize) -> Line<'a> {
    let filled = volume_filled_cells(volume, width);
    let empty = width - filled;
    Line::from(vec![
        Span::styled("Vol ", Style::default().fg(theme.muted)),
        Span::styled("█".repeat(filled), Style::default().fg(theme.playing)),
        Span::styled("░".repeat(empty), Style::default().fg(theme.muted)),
        Span::styled(
            format!(" {volume}%"),
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Footer key hints plus network/offline state.
fn render_footer(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer, compact: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let accent = theme.accent_style();
    let muted = Style::default().fg(theme.muted);

    let mut hints: Vec<(&str, &str)> = if compact {
        vec![
            ("Tab", "focus"),
            ("/", "search"),
            ("\u{21B5}", "play"),
            ("Spc", "stop"),
            ("f", "fav"),
            ("t", "theme"),
            ("v", "viz"),
            ("q", "quit"),
        ]
    } else {
        vec![
            ("Tab", "focus"),
            ("/", "search"),
            ("Enter", "play"),
            ("Space", "stop/play"),
            ("f", "favorite"),
            ("t", "theme"),
            ("v", "visualizer"),
            ("q", "quit"),
        ]
    };
    // Discovery for the eligible plugin launch only: the same connection
    // states that show the quiet Now Playing summary advertise the Agent
    // Planets canvas. Hidden (standalone/disabled/ineligible) and unavailable
    // states append nothing, so their footers stay byte-identical; the compact
    // tier keeps its trimmed hint row.
    if !compact
        && matches!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected | AgentPulseConnection::Stale
        )
    {
        hints.push(("a", "Agent Planets"));
    }

    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(key, accent));
        spans.push(Span::styled(format!(" {label}  "), muted));
    }
    spans.push(network_span(app, theme));
    if !compact {
        spans.push(Span::styled(format!("  · {}", app.theme().as_str()), muted));
    }

    Paragraph::new(Line::from(spans))
        .style(theme.base_style())
        .render(area, buf);
}

// --- small helpers -------------------------------------------------------

/// The empty-state note for the visible station list, specific to the active
/// source.
///
/// The Favorites source gets an explicit save hint rather than the generic "No
/// stations" line: an empty favorites list is a normal first-run state the user
/// resolves by pressing `f`. That hint is shown even offline — saved favorites
/// are retryable stream entries, not stations guaranteed to play offline, so the
/// empty state keeps guiding the user to save rather than implying offline
/// availability. Other sources keep the offline-aware generic note.
fn empty_list_note(app: &App) -> String {
    // A zero-match Browse filter over the current search population gets a
    // specific note ("No Jazz results in current search") rather than silently
    // implying the curated empty states below.
    if let Some(note) = app.search_filter_empty_note() {
        return note;
    }
    if app.active_source() == ListSource::Favorites {
        "No favorites yet — press f on a station to save it".to_string()
    } else if app.is_offline() {
        "No stations — offline".to_string()
    } else {
        "No stations".to_string()
    }
}

/// A bordered pane block whose border highlights when the pane is focused.
fn bordered_block(theme: &Theme, title: &str, focused: bool) -> Block<'static> {
    let border_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.border)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Line::styled(title.to_string(), theme.accent_style()))
        .style(theme.base_style())
}

/// An online/offline signal span, colored by the theme.
fn network_span<'a>(app: &App, theme: &Theme) -> Span<'a> {
    if app.is_offline() {
        Span::styled(
            "● OFFLINE",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("● online", Style::default().fg(theme.muted))
    }
}

/// A playback-state span, colored by the theme; carries the failure message.
fn playback_span<'a>(theme: &Theme, state: &PlaybackState) -> Span<'a> {
    match state {
        PlaybackState::Stopped => Span::styled("Stopped", Style::default().fg(theme.muted)),
        PlaybackState::Connecting => Span::styled("Connecting…", Style::default().fg(theme.accent)),
        PlaybackState::Playing => Span::styled(
            "Playing",
            Style::default()
                .fg(theme.playing)
                .add_modifier(Modifier::BOLD),
        ),
        PlaybackState::Failed(message) => Span::styled(
            format!("Failed: {message}"),
            Style::default().fg(theme.error),
        ),
    }
}

/// Compact codec/bitrate metadata label for a station.
fn station_meta(station: &Station) -> String {
    let codec = codec_label(&station.codec);
    match station.bitrate.map(|b| b.get()) {
        Some(kbps) => format!("{codec} · {kbps}k"),
        None => codec.to_string(),
    }
}

fn station_bitrate(station: &Station) -> String {
    station
        .bitrate
        .map(|bitrate| format!("{}k", bitrate.get()))
        .unwrap_or_else(|| "—".to_string())
}

fn station_table_meta(station: &Station) -> String {
    match station_location(station) {
        Some(location) => format!("{} · {}", station_meta(station), location),
        None => station_meta(station),
    }
}

/// Country/language metadata line, when present.
fn station_location(station: &Station) -> Option<String> {
    match (&station.country, &station.language) {
        (Some(country), Some(language)) => Some(format!("{country} · {language}")),
        (Some(country), None) => Some(country.clone()),
        (None, Some(language)) => Some(language.clone()),
        (None, None) => None,
    }
}

/// Short display label for a codec classification.
fn codec_label(codec: &CodecKind) -> &str {
    match codec {
        CodecKind::Mp3 => "mp3",
        CodecKind::Aac => "aac",
        CodecKind::Other(name) => name.as_str(),
        CodecKind::Unknown => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, SearchStatus};
    use crate::audio::AudioEvent;
    use crate::catalog::{Catalog, Category};
    use crate::model::{VisualizerMode, VizFrame};
    use crate::search::SearchResults;
    use crate::settings::Settings;
    use crate::theme::ThemeName;

    fn base_app() -> App {
        App::new(Settings::default(), Catalog::curated())
    }

    /// Render an app into a standalone buffer of the given size — no terminal.
    fn render_buffer(app: &App, width: u16, height: u16) -> Buffer {
        render_buffer_at(app, false, Instant::now(), width, height)
    }

    /// Render with explicit low-power and clock inputs, for deterministic
    /// Agent Pulse motion assertions.
    fn render_buffer_at(
        app: &App,
        low_power: bool,
        now: Instant,
        width: u16,
        height: u16,
    ) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_into(app, low_power, now, area, &mut buf);
        buf
    }

    /// Flatten a buffer's cell symbols into newline-separated text.
    fn buffer_text(buf: &Buffer) -> String {
        let area = *buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        out
    }

    /// True if any cell in the buffer carries `fg`.
    fn has_fg(buf: &Buffer, fg: ratatui::style::Color) -> bool {
        let area = *buf.area();
        (0..area.height).any(|y| (0..area.width).any(|x| buf.cell((x, y)).unwrap().fg == fg))
    }

    fn play_first(app: &mut App) {
        let id = app.selected_station().unwrap().id.clone();
        app.apply(Action::PlaySelected);
        app.apply(Action::Audio(AudioEvent::Playing { station: id }));
    }

    #[test]
    fn wide_renders_search_console_with_results_and_now_playing() {
        let app = base_app();
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);

        // Search-first strip with a prominent result count.
        assert!(text.contains("Search"));
        assert!(text.contains(&format!("{} results", app.visible().len())));
        // Browse shortcuts (Music / Spoken-News category context) are visible.
        assert!(text.contains("Browse"));
        assert!(text.contains("Music"));
        // Ranked results: the top visible station's name is shown.
        let top = app.visible().iter().next().unwrap().name.as_str();
        assert!(
            text.contains(top),
            "expected ranked result {top:?} in {text}"
        );
        // Now Playing and footer hints remain visible while browsing.
        assert!(text.contains("Now Playing"));
        assert!(text.contains("search"));
    }

    /// Render only the Browse rail into a standalone buffer so per-row
    /// assertions are not crossed by the Results / Now Playing columns.
    fn render_browse_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_browse(app, &theme, area, &mut buf);
        buf
    }

    /// Render only the Results/Stations pane into a standalone buffer so width
    /// breakpoints can be asserted without depending on full-layout columns.
    fn render_station_list_buffer(app: &App, width: u16, height: u16, compact: bool) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_station_list(app, &theme, area, &mut buf, "Results", compact);
        buf
    }

    /// The buffer line (relative row) that contains `needle`, if any.
    fn line_with(text: &str, needle: &str) -> Option<usize> {
        text.lines().position(|line| line.contains(needle))
    }

    #[test]
    fn wide_results_use_full_table_when_pane_is_wide_enough() {
        let app = base_app();
        let text = buffer_text(&render_station_list_buffer(&app, 76, 12, false));

        assert!(text.contains("Station"), "station header missing: {text}");
        assert!(text.contains("Codec"), "codec header missing: {text}");
        assert!(text.contains("Rate"), "rate header missing: {text}");
        assert!(text.contains("Locale"), "locale header missing: {text}");
        assert!(
            text.contains("Chillhop Radio"),
            "station row missing: {text}"
        );
        assert!(text.contains("mp3"), "codec cell missing: {text}");
        assert!(text.contains("320k"), "bitrate cell missing: {text}");
        assert!(text.contains("Germany"), "locale cell missing: {text}");
    }

    #[test]
    fn wide_results_collapse_to_compact_table_at_intermediate_width() {
        let app = base_app();
        let text = buffer_text(&render_station_list_buffer(&app, 56, 12, false));

        assert!(text.contains("Station"), "station header missing: {text}");
        assert!(text.contains("Meta"), "meta header missing: {text}");
        assert!(
            text.contains("Chillhop Radio"),
            "station row missing: {text}"
        );
        assert!(text.contains("mp3 · 320k"), "compact meta missing: {text}");
        assert!(
            !text.contains("Codec"),
            "full codec header should collapse: {text}"
        );
        assert!(
            !text.contains("Locale"),
            "full locale header should collapse: {text}"
        );
    }

    #[test]
    fn narrow_results_fall_back_to_current_list_row() {
        let app = base_app();
        let text = buffer_text(&render_station_list_buffer(&app, 42, 12, false));

        assert!(
            text.contains("Chillhop Radio"),
            "station row missing: {text}"
        );
        assert!(text.contains("mp3 · 320k"), "list metadata missing: {text}");
        assert!(
            !text.contains("Station"),
            "narrow fallback should not render header: {text}"
        );
        assert!(
            !text.contains("Meta"),
            "narrow fallback should not render header: {text}"
        );
    }

    #[test]
    fn results_table_preserves_selection_favorite_and_failed_markers() {
        let mut app = base_app();
        app.apply(Action::ToggleFavorite);
        let other = app.visible().as_slice()[1].id.clone();
        app.apply(Action::Audio(AudioEvent::Failed {
            station: other,
            message: "x".to_string(),
        }));

        let text = buffer_text(&render_station_list_buffer(&app, 76, 12, false));

        assert!(text.contains('★'), "favorite marker missing: {text}");
        assert!(text.contains('✗'), "failed marker missing: {text}");
        assert!(text.contains('▶'), "selection marker missing: {text}");
    }

    #[test]
    fn medium_and_compact_station_lists_keep_plain_rows() {
        let app = base_app();
        let medium = buffer_text(&render_buffer(&app, 100, 24));
        let compact = buffer_text(&render_buffer(&app, 70, 16));

        assert!(medium.contains("Stations"));
        assert!(medium.contains("Chillhop Radio"));
        assert!(
            !medium.contains("Codec"),
            "medium should keep compact list rows: {medium}"
        );
        assert!(
            !medium.contains("Meta"),
            "medium should keep compact list rows: {medium}"
        );
        assert!(compact.contains("Chillhop Radio"));
        assert!(
            !compact.contains("Meta"),
            "compact should keep compact list rows: {compact}"
        );
    }

    #[test]
    fn wide_browse_rail_lists_every_source_as_a_flat_picker() {
        // The whole flat rail is actionable source labels, drawn from catalog
        // state: the two cross-cutting sources, both sections, and categories.
        let app = base_app();
        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        for label in [
            "All Stations",
            "Favorites",
            "Music",
            "Lofi",
            "Jazz",
            "Spoken / News",
            "News",
            "Talk",
        ] {
            assert!(
                text.contains(label),
                "Browse rail missing {label:?}: {text}"
            );
        }
    }

    #[test]
    fn wide_browse_marks_the_active_source_distinctly() {
        // Default active source is All Stations: its row carries the active dot
        // and no other source row does.
        let app = base_app();
        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        let all_row = line_with(&text, "All Stations").expect("All Stations row");
        let lofi_row = line_with(&text, "Lofi").expect("Lofi row");
        assert!(
            text.lines().nth(all_row).unwrap().contains('●'),
            "active source not marked: {text}"
        );
        assert!(
            !text.lines().nth(lofi_row).unwrap().contains('●'),
            "inactive source wrongly marked active: {text}"
        );

        // Applying a category moves the active marker to that row.
        let mut app = base_app();
        app.apply(Action::ShowCategory(Category::Lofi));
        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        let all_row = line_with(&text, "All Stations").expect("All Stations row");
        let lofi_row = line_with(&text, "Lofi").expect("Lofi row");
        assert!(
            text.lines().nth(lofi_row).unwrap().contains('●'),
            "active marker did not follow the applied source: {text}"
        );
        assert!(
            !text.lines().nth(all_row).unwrap().contains('●'),
            "stale active marker on previous source: {text}"
        );
    }

    #[test]
    fn wide_browse_separates_selection_from_active_source() {
        // The Browse cursor (selection) and the active source are different
        // signals: parking the cursor on a row other than the active source must
        // show both markers, on their own rows.
        let mut app = base_app();
        assert_eq!(app.active_source(), ListSource::AllStations);
        app.apply(Action::SetFocus(FocusPane::Sections));
        let favorites_index = ListSource::browse_rail()
            .iter()
            .position(|s| *s == ListSource::Favorites)
            .unwrap();
        app.apply(Action::SetBrowseSelection(favorites_index));

        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        let active_row = line_with(&text, "All Stations").expect("All Stations row");
        let selected_row = line_with(&text, "Favorites").expect("Favorites row");
        assert_ne!(
            active_row, selected_row,
            "active and selected rows must differ for this case"
        );
        // The active source keeps its dot.
        assert!(text.lines().nth(active_row).unwrap().contains('●'));
        // The Browse selection carries the cursor symbol on its own row.
        assert!(
            text.lines().nth(selected_row).unwrap().contains('▶'),
            "Browse selection cursor missing: {text}"
        );
    }

    #[test]
    fn wide_search_strip_shows_live_query_and_loading_indicator() {
        let mut app = base_app();
        app.apply(Action::SetSearchQuery("lofi jazz".to_string()));
        app.apply(Action::SetSearchStatus(SearchStatus::Loading));
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);

        // The live query text is rendered, not the empty placeholder.
        assert!(text.contains("lofi jazz"), "live query missing: {text}");
        assert!(!text.contains("type to search"));
        // The loading indicator is visible.
        assert!(
            text.contains("loading"),
            "loading indicator missing: {text}"
        );
    }

    #[test]
    fn wide_search_strip_distinguishes_cached_from_fresh_results() {
        let mut cached = base_app();
        cached.apply(Action::SetSearchStatus(SearchStatus::Loaded {
            from_cache: true,
        }));
        assert!(buffer_text(&render_buffer(&cached, 130, 32)).contains("cached"));

        let mut fresh = base_app();
        fresh.apply(Action::SetSearchStatus(SearchStatus::Loaded {
            from_cache: false,
        }));
        assert!(buffer_text(&render_buffer(&fresh, 130, 32)).contains("fresh"));
    }

    #[test]
    fn medium_keeps_station_list_and_now_playing_visible() {
        // 100x24 resolves to Medium: width>=100 but height<28.
        assert_eq!(LayoutTier::from_size(100, 24), LayoutTier::Medium);
        let app = base_app();
        let buf = render_buffer(&app, 100, 24);
        let text = buffer_text(&buf);

        assert!(text.contains("Stations"));
        assert!(text.contains("Now Playing"));
        let top = app.visible().iter().next().unwrap().name.as_str();
        assert!(text.contains(top));
    }

    #[test]
    fn medium_shows_browse_modal_when_hidden_browse_has_focus() {
        assert_eq!(LayoutTier::from_size(100, 24), LayoutTier::Medium);
        let mut app = base_app();
        app.apply(Action::SetFocus(FocusPane::Sections));
        app.apply(Action::SetBrowseSelection(1));

        let text = buffer_text(&render_buffer(&app, 100, 24));

        assert!(
            text.contains("Browse"),
            "Browse modal title missing: {text}"
        );
        assert!(
            text.contains("Favorites"),
            "Browse modal should show source rows: {text}"
        );
        assert!(
            text.contains('▶'),
            "Browse modal should show the selected source cursor: {text}"
        );
    }

    #[test]
    fn compact_shows_browse_modal_when_hidden_browse_has_focus() {
        assert_eq!(LayoutTier::from_size(70, 16), LayoutTier::Compact);
        let mut app = base_app();
        app.apply(Action::SetFocus(FocusPane::Sections));
        app.apply(Action::SetBrowseSelection(1));

        let text = buffer_text(&render_buffer(&app, 70, 16));

        assert!(
            text.contains("Browse"),
            "Browse modal title missing: {text}"
        );
        assert!(
            text.contains("Favorites"),
            "Browse modal should show source rows: {text}"
        );
        assert!(
            text.contains('▶'),
            "Browse modal should show the selected source cursor: {text}"
        );
    }

    #[test]
    fn compact_keeps_station_context_and_player_without_fullscreen_visualizer() {
        // Compact needs width<72 or height<18 under the layout policy.
        assert_eq!(LayoutTier::from_size(70, 16), LayoutTier::Compact);
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9_f32; 16],
            0.8,
            vec![0.5_f32; 16],
        ))));
        play_first(&mut app);
        let buf = render_buffer(&app, 70, 16);
        let text = buffer_text(&buf);

        // At least one station row stays visible (limited context, not hidden).
        let top = app.visible().iter().next().unwrap().name.as_str();
        assert!(
            text.contains(top),
            "expected a station row in compact: {text}"
        );
        // The player stays visible: playback state is shown.
        assert!(text.contains("Playing"));
        // The compact visualizer is present but not full-screen: particles render
        // and the station list region above still has content. The heavy grains
        // (`∙`/`•`) are unique to the visualizer (the gauge uses `█`/`░`), so their
        // presence proves the columns drew.
        assert!(
            text.chars().any(|c| matches!(c, '∙' | '•')),
            "compact particle visualizer missing: {text}"
        );
    }

    #[test]
    fn spectrum_stack_is_shared_and_renders_themed_particles_across_tiers() {
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 16],
            1.0,
            vec![1.0_f32; 16],
        ))));
        play_first(&mut app);

        let theme = app.theme().theme();
        for (w, h) in [(130, 32), (100, 24), (70, 16)] {
            let buf = render_buffer(&app, w, h);
            // The particle columns render at every tier: heavy grains (`∙`/`•`) are
            // unique to the visualizer, so their presence proves it drew.
            assert!(
                buffer_text(&buf).chars().any(|c| matches!(c, '∙' | '•')),
                "no particles at {w}x{h}"
            );
            // The particles use theme spectrum colors, not an ad hoc palette.
            assert!(
                has_fg(&buf, theme.spectrum_low)
                    || has_fg(&buf, theme.spectrum_mid)
                    || has_fg(&buf, theme.spectrum_high),
                "spectrum colors not themed at {w}x{h}"
            );
        }
    }

    #[test]
    fn now_playing_shows_icy_title_when_present() {
        let mut app = base_app();
        let id = app.selected_station().unwrap().id.clone();
        play_first(&mut app);
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: id,
            title: "Live Track Title".to_string(),
        }));
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Live Track Title"),
            "ICY title missing from Now Playing: {text}"
        );
    }

    #[test]
    fn now_playing_falls_back_to_station_metadata_without_icy_title() {
        let mut app = base_app();
        let station = app.selected_station().unwrap().clone();
        let station_name = station.name.as_str().to_string();
        let meta = station_meta(&station);
        play_first(&mut app);
        // No ICY title received: station name and codec/bitrate stay the source.
        assert!(app.now_playing_title().is_none());
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);
        assert!(
            text.contains(&station_name),
            "station name fallback missing: {text}"
        );
        assert!(
            text.contains(&meta),
            "codec metadata fallback missing ({meta:?}): {text}"
        );
    }

    /// Representative size for each layout tier (wide, medium, compact). Mirrors
    /// the sizes used by the per-tier rendering tests above.
    const TIER_SIZES: [(u16, u16); 3] = [(130, 32), (100, 24), (70, 16)];

    #[test]
    fn offline_state_is_visible_in_every_tier() {
        // Offline must be obvious at every pane size, not just the wide console:
        // the network signal renders in the search strip / footer across tiers.
        let mut app = base_app();
        app.apply(Action::SetOffline(true));
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.contains("OFFLINE"),
                "offline indicator missing at {w}x{h}: {text}"
            );
        }
    }

    #[test]
    fn offline_search_status_is_visible_in_every_tier() {
        // A failed online search surfaces an explicit "offline" search status in
        // addition to the network signal, at every tier.
        let mut app = base_app();
        app.apply(Action::SetOffline(true));
        app.apply(Action::SetSearchStatus(SearchStatus::Offline));
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.to_lowercase().contains("offline"),
                "offline search status missing at {w}x{h}: {text}"
            );
        }
    }

    #[test]
    fn offline_still_shows_builtin_retry_candidates_in_every_tier() {
        // Going offline must not hide the built-in candidates the user can still
        // retry: the curated catalog stays visible and non-empty across tiers.
        let mut app = base_app();
        app.apply(Action::SetOffline(true));
        assert!(
            !app.visible().is_empty(),
            "offline must retain visible retry candidates"
        );
        let top = app.visible().iter().next().unwrap().name.as_str();
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.contains(top),
                "built-in retry candidate {top:?} missing offline at {w}x{h}: {text}"
            );
        }
    }

    #[test]
    fn failed_playback_surfaces_the_error_message() {
        let mut app = base_app();
        let id = app.selected_station().unwrap().id.clone();
        app.apply(Action::PlaySelected);
        app.apply(Action::Audio(AudioEvent::Failed {
            station: id,
            message: "boom".to_string(),
        }));
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);
        assert!(text.contains("Failed"));
        assert!(text.contains("boom"));
    }

    #[test]
    fn favorite_and_failed_markers_render() {
        let mut app = base_app();
        // Star the selected station.
        app.apply(Action::ToggleFavorite);
        // Fail a different visible station for the session.
        let other = app.visible().as_slice()[1].id.clone();
        app.apply(Action::Audio(AudioEvent::Failed {
            station: other,
            message: "x".to_string(),
        }));
        let buf = render_buffer(&app, 130, 32);
        let text = buffer_text(&buf);

        assert!(text.contains('★'), "favorite marker missing");
        assert!(text.contains('✗'), "failed marker missing");
        assert!(text.contains('▶'), "selection marker missing");
    }

    #[test]
    fn colors_come_from_the_active_theme() {
        // The accent color is theme-driven and differs between themes; rendering
        // must reflect the active theme rather than a fixed palette.
        let mut neon = base_app();
        neon.apply(Action::CycleTheme); // Minimal -> Neon
        assert_eq!(neon.theme(), ThemeName::Neon);

        let minimal_buf = render_buffer(&base_app(), 130, 32);
        let neon_buf = render_buffer(&neon, 130, 32);

        assert!(has_fg(
            &minimal_buf,
            Theme::for_name(ThemeName::Minimal).accent
        ));
        assert!(has_fg(&neon_buf, Theme::for_name(ThemeName::Neon).accent));
        // The neon accent should not appear in the minimal render.
        assert!(!has_fg(
            &minimal_buf,
            Theme::for_name(ThemeName::Neon).accent
        ));
    }

    #[test]
    fn added_theme_changes_rendered_output() {
        // MIK-029: an added theme must actually drive rendering. Cycle to Sakura
        // and prove its accent colors the buffer while the Minimal accent does
        // not — so the palette is sourced from the theme, not hard-coded in ui.rs.
        let mut sakura = base_app();
        // Minimal -> Neon -> CRT -> Solarized -> Midnight -> Sakura
        for _ in 0..5 {
            sakura.apply(Action::CycleTheme);
        }
        assert_eq!(sakura.theme(), ThemeName::Sakura);

        let minimal_buf = render_buffer(&base_app(), 130, 32);
        let sakura_buf = render_buffer(&sakura, 130, 32);

        let sakura_accent = Theme::for_name(ThemeName::Sakura).accent;
        assert!(
            has_fg(&sakura_buf, sakura_accent),
            "sakura accent missing from sakura render"
        );
        assert!(
            !has_fg(&minimal_buf, sakura_accent),
            "sakura accent leaked into the minimal render"
        );
    }

    // --- Favorites view: empty state and rendering (MIK-022) ------------

    /// A station fixture for favorites tests; the id doubles as the display name
    /// so rendered rows are easy to assert on.
    fn fav_station(id: &str) -> Station {
        use crate::model::{BitrateKbps, StationId, StationName, StationSource, StreamUrl};
        Station {
            id: StationId::new(id).unwrap(),
            name: StationName::new(id).unwrap(),
            url: StreamUrl::parse(format!("https://example.com/{id}.mp3")).unwrap(),
            homepage: None,
            country: None,
            language: None,
            tags: vec![],
            codec: CodecKind::Mp3,
            bitrate: Some(BitrateKbps::new(128).unwrap()),
            votes: Some(10),
            click_count: Some(10),
            source: StationSource::RadioBrowser,
        }
    }

    /// An app whose persisted favorites are exactly `ids`, in order.
    fn app_with_favorites(ids: &[&str]) -> App {
        let favorites =
            crate::settings::Favorites::from_stations(ids.iter().map(|id| fav_station(id)));
        let settings = Settings {
            favorites,
            ..Settings::default()
        };
        App::new(settings, Catalog::curated())
    }

    /// Activate the Favorites source through the Browse rail (the wired path).
    fn apply_favorites_source(app: &mut App) {
        let rail = ListSource::browse_rail();
        let fav_index = rail
            .iter()
            .position(|s| *s == ListSource::Favorites)
            .unwrap();
        app.apply(Action::SetBrowseSelection(fav_index));
        app.apply(Action::ApplyBrowseSelection);
    }

    #[test]
    fn empty_favorites_shows_a_helpful_save_hint() {
        // An empty Favorites view must be explicit and tell the user how to fill
        // it, not show the generic "No stations" line, at every tier.
        let mut app = app_with_favorites(&[]);
        apply_favorites_source(&mut app);
        assert_eq!(app.active_source(), ListSource::Favorites);
        assert!(app.visible().is_empty());

        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.contains("No favorites yet"),
                "favorites empty hint missing at {w}x{h}: {text}"
            );
            assert!(
                text.contains("press f"),
                "favorites save hint missing at {w}x{h}: {text}"
            );
        }
    }

    #[test]
    fn empty_favorites_hint_is_shown_even_offline() {
        // Offline must not swap the save hint for a generic offline note:
        // favorites are retryable saved entries, so the empty state keeps guiding
        // the user to save rather than implying offline playback availability.
        let mut app = app_with_favorites(&[]);
        apply_favorites_source(&mut app);
        app.apply(Action::SetOffline(true));
        let text = buffer_text(&render_buffer(&app, 130, 32));
        assert!(
            text.contains("No favorites yet"),
            "favorites hint lost while offline: {text}"
        );
    }

    #[test]
    fn non_empty_favorites_renders_saved_stations_with_markers() {
        // A populated Favorites view shows the saved stations with the normal
        // favorite star marker and not the empty-state hint.
        let mut app = app_with_favorites(&["fav-a", "fav-b"]);
        apply_favorites_source(&mut app);
        let text = buffer_text(&render_buffer(&app, 130, 32));
        assert!(text.contains("fav-a"), "saved favorite missing: {text}");
        assert!(text.contains("fav-b"), "saved favorite missing: {text}");
        assert!(text.contains('★'), "favorite marker missing: {text}");
        assert!(
            !text.contains("No favorites yet"),
            "empty hint shown for a non-empty favorites list: {text}"
        );
    }

    #[test]
    fn favorites_source_is_marked_active_in_browse() {
        // Building on MIK-019/021: when Favorites is the active source its Browse
        // rail row carries the active dot, so the rail reflects the applied view.
        let mut app = app_with_favorites(&["fav-a"]);
        apply_favorites_source(&mut app);
        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        let fav_row = line_with(&text, "Favorites").expect("Favorites row");
        assert!(
            text.lines().nth(fav_row).unwrap().contains('●'),
            "active Favorites source not marked in Browse: {text}"
        );
    }

    #[test]
    fn browse_rail_has_a_single_favorites_entry_labelled_favorites() {
        // Scope: a single favorites Browse mode. The rail shows exactly one
        // favorites row, plainly labelled `Favorites` — no `All Favorites` /
        // `Current Favorites` split.
        let app = base_app();
        let text = buffer_text(&render_browse_buffer(&app, 24, 16));
        assert!(text.contains("Favorites"), "Favorites row missing: {text}");
        assert!(
            !text.contains("All Favorites"),
            "unexpected All Favorites label: {text}"
        );
        assert!(
            !text.contains("Current Favorites"),
            "unexpected Current Favorites label: {text}"
        );
        let favorite_rows = text.lines().filter(|l| l.contains("Favorites")).count();
        assert_eq!(favorite_rows, 1, "exactly one favorites row: {text}");
    }

    // --- Browse-over-search filter context and empty state (MIK-047) ----

    #[test]
    fn search_strip_shows_active_search_filter_context() {
        // With a search-result population and a genre filter active, the search
        // strip surfaces the active filter context so Browse reads as filtering
        // the current search results.
        let mut jazz = fav_station("search-jazz");
        jazz.tags = vec!["jazz".to_string()];
        let mut app = base_app();
        app.apply(Action::SearchResults(SearchResults::from_stations([jazz])));
        app.apply(Action::ShowCategory(Category::Jazz));

        let text = buffer_text(&render_buffer(&app, 130, 32));

        assert!(
            text.contains("filter: Jazz"),
            "filter context missing: {text}"
        );
    }

    #[test]
    fn search_filter_zero_matches_shows_specific_empty_state() {
        // A genre filter matching zero search results shows a specific empty
        // state rather than silently falling back to curated stations.
        let mut house = fav_station("search-house");
        house.tags = vec!["house".to_string()];
        let mut app = base_app();
        app.apply(Action::SearchResults(SearchResults::from_stations([house])));
        app.apply(Action::ShowCategory(Category::Jazz));

        let text = buffer_text(&render_buffer(&app, 130, 32));

        assert!(
            text.contains("No Jazz results in current search"),
            "specific empty state missing: {text}"
        );
    }

    #[test]
    fn peak_dots_renders_in_every_layout_tier() {
        // Selecting PeakDots changes the visualizer at the full-UI level across
        // Wide, Medium, and Compact, while station context stays visible.
        let mut app = base_app();
        app.apply(Action::CycleVisualizerMode); // -> PeakDots
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9_f32; 16],
            0.8,
            vec![],
        ))));
        play_first(&mut app);
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(text.contains('●'), "peak dots missing at {w}x{h}: {text}");
        }
    }

    #[test]
    fn skyline_peaks_renders_in_every_layout_tier() {
        let mut app = app_in_mode(VisualizerMode::SkylinePeaks);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9_f32; 16],
            0.8,
            vec![],
        ))));
        play_first(&mut app);
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.contains('▀'),
                "skyline caps missing at {w}x{h}: {text}"
            );
        }
    }

    // --- Waveform / RMS visualizer modes (MIK-027) ----------------------

    /// Cycle a fresh app to `mode` via the public `v`-key action.
    fn app_in_mode(mode: VisualizerMode) -> App {
        let mut app = base_app();
        while app.visualizer_mode() != mode {
            app.apply(Action::CycleVisualizerMode);
        }
        app
    }

    #[test]
    fn waveform_and_ambient_modes_render_in_every_tier() {
        for mode in [
            VisualizerMode::WaveScope,
            VisualizerMode::MirrorWave,
            VisualizerMode::AmbientPulse,
        ] {
            let mut app = app_in_mode(mode);
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                vec![0.9; 16],
                0.8,
                vec![-0.8, -0.2, 0.4, 0.9, 0.1, -0.5, 0.6, -0.3],
            ))));
            play_first(&mut app);
            let glyphs: &[char] = match mode {
                VisualizerMode::WaveScope => &['•'],
                VisualizerMode::MirrorWave => &['┃'],
                VisualizerMode::AmbientPulse => &['░', '▒', '▓'],
                _ => &[],
            };
            for (w, h) in TIER_SIZES {
                let text = buffer_text(&render_buffer(&app, w, h));
                assert!(
                    glyphs.iter().any(|g| text.contains(*g)),
                    "{mode:?} missing at {w}x{h}: {text}"
                );
            }
        }
    }

    #[test]
    fn renders_without_panicking_across_sizes_and_tiers() {
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.5_f32; 16],
            0.5,
            vec![0.5_f32; 16],
        ))));
        play_first(&mut app);
        for (w, h) in [
            (0, 0),
            (1, 1),
            (10, 5),
            (40, 12),
            (70, 16),
            (72, 18),
            (100, 24),
            (130, 32),
            (200, 50),
        ] {
            let _ = render_buffer(&app, w, h);
        }
    }

    // --- Volume gauge (MIK-033) -----------------------------------------

    use crate::model::VolumePercent;

    #[test]
    fn volume_filled_cells_reflects_percent_and_is_safe() {
        // 0%, mid, and 100% map proportionally onto the gauge width.
        assert_eq!(volume_filled_cells(0, 10), 0);
        assert_eq!(volume_filled_cells(50, 10), 5);
        assert_eq!(volume_filled_cells(60, 10), 6);
        assert_eq!(volume_filled_cells(100, 10), 10);
        // Rounds to the nearest cell rather than truncating.
        assert_eq!(volume_filled_cells(55, 10), 6);
        // Never overflows the width; zero width is safe.
        assert_eq!(volume_filled_cells(100, 0), 0);
        assert_eq!(volume_filled_cells(100, 6), 6);
    }

    #[test]
    fn volume_gauge_line_is_theme_styled_with_filled_empty_and_percent() {
        // The gauge is built from theme-aware spans: a muted label, a filled bar,
        // an empty bar, and a highlighted percentage — at 0%, a mid value, 100%.
        let theme = Theme::for_name(ThemeName::Neon);
        for (vol, filled) in [(0u8, 0usize), (60, 6), (100, 10)] {
            let line = volume_gauge_line(&theme, vol, 10);

            // Muted label, sourced from the theme (no hard-coded palette).
            assert!(
                line.spans
                    .iter()
                    .any(|s| s.content.contains("Vol") && s.style.fg == Some(theme.muted)),
                "missing muted volume label for {vol}%"
            );

            // Filled cells: count matches the percent and uses the theme playing
            // (output-level) color rather than the heading/focus accent.
            let filled_span = line.spans.iter().find(|s| s.content.starts_with('█'));
            if filled == 0 {
                assert!(filled_span.is_none(), "0% must draw no filled cells");
            } else {
                let fs = filled_span.expect("filled gauge span");
                assert_eq!(fs.content.chars().count(), filled, "filled cell count");
                assert_eq!(fs.style.fg, Some(theme.playing), "filled not theme-colored");
            }

            // Empty cells fill the remainder, colored by the theme muted tone.
            let empty_span = line.spans.iter().find(|s| s.content.starts_with('░'));
            if filled == 10 {
                assert!(empty_span.is_none(), "100% must draw no empty cells");
            } else {
                let es = empty_span.expect("empty gauge span");
                assert_eq!(es.content.chars().count(), 10 - filled, "empty cell count");
                assert_eq!(es.style.fg, Some(theme.muted), "empty not theme-colored");
            }

            // The percentage stays visible and is highlighted (bold).
            let pct = line
                .spans
                .iter()
                .find(|s| s.content.contains(&format!("{vol}%")))
                .expect("percentage span");
            assert!(
                pct.style.add_modifier.contains(Modifier::BOLD),
                "percentage not highlighted for {vol}%"
            );
        }
    }

    #[test]
    fn now_playing_renders_volume_as_a_gauge_in_every_tier() {
        // Default volume is 60%: a full UI render shows the gauge glyphs and the
        // percentage at every layout tier, not just the plain "Volume N%" text.
        let app = base_app();
        assert_eq!(app.settings().volume.get(), 60);
        for (w, h) in TIER_SIZES {
            let text = buffer_text(&render_buffer(&app, w, h));
            assert!(
                text.contains('█'),
                "filled gauge missing at {w}x{h}: {text}"
            );
            assert!(text.contains('░'), "empty gauge missing at {w}x{h}: {text}");
            assert!(
                text.contains("60%"),
                "volume percent missing at {w}x{h}: {text}"
            );
        }
    }

    #[test]
    fn volume_gauge_fill_tracks_0_and_100_percent_in_full_render() {
        // The rendered gauge reflects volume edges: 0% is all empty cells (no
        // filled glyph from the gauge, with no visualizer frame active), and 100%
        // is all filled cells (no empty glyph). Volume actions/settings are
        // unchanged; only rendering reads the value.
        let mut app = base_app();
        app.apply(Action::SetVolume(VolumePercent::new(0).unwrap()));
        let text = buffer_text(&render_buffer(&app, 130, 32));
        assert!(text.contains("0%"), "0% label missing: {text}");
        assert!(text.contains('░'), "0% must draw an empty gauge: {text}");
        assert!(!text.contains('█'), "0% must draw no filled cells: {text}");

        app.apply(Action::SetVolume(VolumePercent::new(100).unwrap()));
        let text = buffer_text(&render_buffer(&app, 130, 32));
        assert!(text.contains("100%"), "100% label missing: {text}");
        assert!(text.contains('█'), "100% must draw a filled gauge: {text}");
        assert!(!text.contains('░'), "100% must draw no empty cells: {text}");
    }

    // --- Signal View: Quiet Signal render surface (MIK-052) -------------

    /// Count visualizer particle cells (`·∙•`) in a rendered buffer. The dust
    /// grains are unique to the Spectrum Stack, so their count is a stable proxy
    /// for how much area the visualizer was given.
    fn count_particle_cells(buf: &Buffer) -> usize {
        let area = *buf.area();
        let mut count = 0;
        for y in 0..area.height {
            for x in 0..area.width {
                if matches!(buf.cell((x, y)).unwrap().symbol(), "·" | "∙" | "•") {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn signal_view_hides_discovery_ui_and_shows_idle_copy_without_current_station() {
        // With no current station, `z` still opens Signal View: it hides the
        // Search/Browse/Stations discovery UI and shows the quiet idle prompt plus
        // the subdued back/quit key hint.
        let mut app = base_app();
        app.apply(Action::ToggleSignalView);
        assert!(app.current_station().is_none());

        let text = buffer_text(&render_buffer(&app, 100, 30));

        assert!(text.contains("Signal View"), "mode label missing: {text}");
        assert!(
            text.contains("Select a station, then press z"),
            "idle prompt missing: {text}"
        );
        assert!(text.contains("z/Esc back"), "key hint missing: {text}");
        assert!(!text.contains("Browse"), "discovery Browse leaked: {text}");
        assert!(
            !text.contains("Results"),
            "discovery Results leaked: {text}"
        );
        assert!(!text.contains("Search"), "discovery Search leaked: {text}");
    }

    #[test]
    fn signal_view_prefers_icy_title_over_station_name() {
        // The primary title prefers the ICY/Shoutcast now-playing title over the
        // current station name.
        let mut app = base_app();
        let id = app.selected_station().unwrap().id.clone();
        play_first(&mut app);
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: id,
            title: "Midnight Drive — Soft Neon Mix".to_string(),
        }));
        app.apply(Action::ToggleSignalView);

        let text = buffer_text(&render_buffer(&app, 120, 34));

        assert!(
            text.contains("Midnight Drive"),
            "ICY title not preferred as primary title: {text}"
        );
    }

    #[test]
    fn signal_view_hides_favorite_marker_when_current_station_is_not_favorite() {
        // Signal View uses the station-list favorite language: no marker when the
        // current station is not a favorite.
        let mut app = base_app();
        app.apply(Action::PlaySelected);
        app.apply(Action::ToggleSignalView);
        assert!(!app.current_station_is_favorite());

        let text = buffer_text(&render_buffer(&app, 100, 30));

        assert!(!text.contains('★'), "non-favorite star leaked: {text}");
        assert!(
            !text.contains('♥'),
            "heart marker should not render: {text}"
        );
        assert!(
            !text.contains('♡'),
            "empty favorite marker should not render: {text}"
        );
    }

    #[test]
    fn signal_view_shows_current_station_favorite_marker_as_star() {
        // The favorite state of the current station is shown like the station
        // list: a star only when it is actually favorited.
        let mut app = base_app();
        app.apply(Action::PlaySelected);
        app.apply(Action::ToggleCurrentFavorite);
        app.apply(Action::ToggleSignalView);
        assert!(app.current_station_is_favorite());

        let text = buffer_text(&render_buffer(&app, 100, 30));

        assert!(text.contains('★'), "favorite star missing: {text}");
        assert!(
            !text.contains('♥'),
            "heart marker should not render: {text}"
        );
        assert!(
            !text.contains('♡'),
            "empty favorite marker should not render: {text}"
        );
    }

    #[test]
    fn signal_view_renders_a_thin_wide_volume_bar_without_visualizer_label() {
        // Signal View should show volume as a calm, near-full-width thin bar.
        // The active visualizer mode is controlled by `v` and should not be
        // visually attached to the volume control line.
        let mut app = base_app();
        app.apply(Action::PlaySelected);
        app.apply(Action::SetVolume(VolumePercent::new(60).unwrap()));
        app.apply(Action::ToggleSignalView);

        let text = buffer_text(&render_buffer(&app, 100, 30));
        let volume_line = text
            .lines()
            .find(|line| line.contains("volume 60%"))
            .unwrap_or_else(|| panic!("volume line missing: {text}"));

        assert!(
            !volume_line.contains(app.visualizer_mode().as_str()),
            "visualizer mode should not be attached to volume line: {volume_line:?}"
        );
        assert!(
            !volume_line.contains("vol 60%"),
            "old compact volume label leaked: {volume_line:?}"
        );
        let bar_cells = volume_line
            .chars()
            .filter(|ch| matches!(ch, '─' | '·'))
            .count();
        assert!(bar_cells >= 40, "volume bar too short: {volume_line:?}");
    }

    #[test]
    fn signal_view_gives_visualizer_the_largest_flexible_region() {
        // A tall Signal View pane allocates a substantial visualizer region,
        // clearly larger than the capped normal Now Playing visualizer.
        let frame = VizFrame::new(vec![1.0_f32; 16], 1.0, vec![0.0; 32]);

        let mut signal = base_app();
        signal.apply(Action::PlaySelected);
        signal.apply(Action::Audio(AudioEvent::Viz(frame.clone())));
        signal.apply(Action::ToggleSignalView);
        let signal_buf = render_buffer(&signal, 120, 40);

        let mut normal = base_app();
        normal.apply(Action::PlaySelected);
        normal.apply(Action::Audio(AudioEvent::Viz(frame)));
        let normal_buf = render_buffer(&normal, 120, 40);

        let text = buffer_text(&signal_buf);
        assert!(text.contains("z/Esc back"), "key hint missing: {text}");

        let signal_cells = count_particle_cells(&signal_buf);
        let normal_cells = count_particle_cells(&normal_buf);
        assert!(
            signal_cells > 300,
            "Signal View visualizer region too small: {signal_cells} cells"
        );
        assert!(
            signal_cells > normal_cells,
            "Signal View visualizer ({signal_cells}) not larger than normal Now Playing ({normal_cells})"
        );
    }

    #[test]
    fn signal_view_tiny_and_small_terminals_do_not_panic() {
        // Tiny/small terminals must stay bounded and never panic; they keep the
        // Center Stage concept instead of falling back to the normal compact UI.
        let mut app = base_app();
        app.apply(Action::PlaySelected);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.5_f32; 16],
            0.5,
            vec![0.5_f32; 16],
        ))));
        app.apply(Action::ToggleSignalView);
        for (w, h) in [(0, 0), (1, 1), (2, 3), (6, 4), (12, 5), (20, 8), (40, 10)] {
            let buf = render_buffer(&app, w, h);
            assert_eq!(*buf.area(), Rect::new(0, 0, w, h));
        }
    }

    #[test]
    fn signal_view_long_title_is_constrained_to_two_lines() {
        // A very long title must never grow past two lines; overflow is ellipsized
        // and each line stays within the available width so layout cannot break.
        let long = "Extremely Long Now Playing Title That Keeps Going ".repeat(8);
        let lines = title_lines(&long, 24);

        assert!(
            lines.len() <= 2,
            "title must be at most two lines, got {}",
            lines.len()
        );
        assert!(
            lines.last().unwrap().contains('…'),
            "overflowing title must be ellipsized: {lines:?}"
        );
        for line in &lines {
            assert!(
                line.chars().count() <= 24,
                "title line exceeds width: {line:?}"
            );
        }
    }

    #[test]
    fn signal_view_long_title_render_stays_bounded() {
        // The long-title constraint holds through the full render path without
        // pushing layout regions out of view.
        let mut app = base_app();
        let id = app.selected_station().unwrap().id.clone();
        play_first(&mut app);
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: id,
            title: "Extremely ".repeat(40),
        }));
        app.apply(Action::ToggleSignalView);

        for (w, h) in [(40, 12), (80, 24), (120, 40)] {
            let buf = render_buffer(&app, w, h);
            // The footer hint still survives below the constrained title.
            assert!(
                buffer_text(&buf).contains("z/Esc back"),
                "hint pushed off-screen by long title at {w}x{h}"
            );
        }
    }

    // --- Agent Pulse: quiet count + full-screen Kinetic Collage -----------

    use crate::herdr::{AgentDetails, AgentId, AgentSnapshot, AgentStatus};
    use std::time::Duration;

    fn pulse_agent(
        workspace: &str,
        pane: &str,
        name: Option<&str>,
        status: AgentStatus,
    ) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new(workspace, pane),
            details: AgentDetails {
                name: name.map(str::to_string),
                agent: None,
                activity: None,
            },
            status,
        }
    }

    /// An app that received one Agent Pulse snapshot.
    fn app_with_agents(agents: Vec<AgentSnapshot>) -> App {
        let mut app = base_app();
        app.apply(Action::AgentSnapshot {
            agents,
            now: Instant::now(),
        });
        app
    }

    #[test]
    fn normal_wide_and_medium_show_only_the_active_count() {
        let app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        for (width, height) in [(120, 36), (80, 20)] {
            let text = buffer_text(&render_buffer(&app, width, height));
            assert!(
                text.contains("● 1 active"),
                "normal {width}x{height} view must show the tiny count: {text}"
            );
            assert!(
                !text.contains("research"),
                "the normal view never shows agent names: {text}"
            );
        }
    }

    #[test]
    fn compact_normal_layout_reserves_no_agent_pulse_row() {
        let app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        let text = buffer_text(&render_buffer(&app, 60, 16));
        assert!(
            !text.contains("active"),
            "compact keeps no Agent Pulse line: {text}"
        );
    }

    /// An eligible plugin launch whose integration is live, the way a
    /// successful snapshot would produce it.
    fn connected_agent_app() -> App {
        app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )])
    }

    /// A disabled or ineligible launch: the integration never leaves
    /// `Hidden`, even after the user pressed the (no-op) canvas toggle.
    fn hidden_agent_app() -> App {
        let mut app = base_app();
        app.apply(Action::ToggleAgentOverlay);
        app
    }

    #[test]
    fn eligible_normal_footer_advertises_a_agent_planets() {
        let app = connected_agent_app();
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            text.contains("a Agent Planets"),
            "eligible normal footer must advertise the canvas: {text}"
        );
        let compact = buffer_text(&render_buffer(&app, 60, 16));
        assert!(
            !compact.contains("Agent Planets"),
            "compact keeps its trimmed footer: {compact}"
        );
    }

    #[test]
    fn standalone_and_disabled_footer_do_not_advertise_agent_planets() {
        assert!(
            !buffer_text(&render_buffer(&base_app(), 120, 36)).contains("Agent Planets"),
            "standalone footer must not advertise the canvas"
        );
        let disabled = hidden_agent_app();
        assert!(
            !buffer_text(&render_buffer(&disabled, 120, 36)).contains("Agent Planets"),
            "disabled footer must not advertise the canvas"
        );
    }

    #[test]
    fn hidden_agent_actions_leave_standalone_render_unchanged() {
        let sizes = [(120, 36), (80, 20), (60, 16)];
        let mut app = base_app();
        let before: Vec<Buffer> = sizes
            .iter()
            .map(|&(width, height)| render_buffer(&app, width, height))
            .collect();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        for (index, &(width, height)) in sizes.iter().enumerate() {
            assert_eq!(
                render_buffer(&app, width, height),
                before[index],
                "hidden Agent Pulse actions changed the render at {width}x{height}"
            );
        }
    }

    /// Cells drawn with planet glyphs (body, orbit ring, or satellite)
    /// anywhere in the buffer. The Working arc `●` stays excluded because
    /// the quiet summary line shares that glyph, and the crater `░` because
    /// the normal-layout Now Playing gauge shares it.
    fn tile_cell_count(text: &str) -> usize {
        ["▓", "∘", "▪"]
            .iter()
            .map(|glyph| text.matches(glyph).count())
            .sum()
    }

    #[test]
    fn collage_canvas_covers_the_full_screen() {
        let mut app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        let normal = buffer_text(&render_buffer(&app, 120, 36));
        assert!(normal.contains("All Stations"), "sanity: {normal}");

        app.apply(Action::ToggleAgentOverlay);
        let canvas = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            canvas.contains("Agent Planets · 1 active"),
            "stage heading: {canvas}"
        );
        assert!(
            !canvas.contains("All Stations"),
            "the canvas replaces the whole player surface: {canvas}"
        );
    }

    #[test]
    fn agent_planets_canvas_keeps_permanent_named_tags() {
        let mut app = app_with_agents(vec![pulse_agent(
            "alpha",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        app.apply(Action::ToggleAgentOverlay);
        let unselected = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            unselected.contains("research"),
            "the named tag renders before selection: {unselected}"
        );
        assert!(
            unselected.contains("working"),
            "the tag status renders before selection: {unselected}"
        );
        assert!(
            !unselected.contains("alpha") && !unselected.contains("p1"),
            "private ids never render: {unselected}"
        );

        app.apply(Action::SelectNextAgent);
        let selected = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            selected.contains("research"),
            "the selected tag stays visible: {selected}"
        );
    }

    #[test]
    fn cross_workspace_agents_render_as_selectable_tiles() {
        let mut app = app_with_agents(vec![
            pulse_agent("alpha", "p1", Some("research"), AgentStatus::Working),
            pulse_agent("beta", "p1", Some("review"), AgentStatus::Idle),
        ]);
        app.apply(Action::ToggleAgentOverlay);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(tile_cell_count(&text) > 0, "tiles must render: {text}");

        // Every agent owns its own click target, so both workspaces' tiles
        // are present even when both share a pane id.
        let area = Rect::new(0, 0, 120, 36);
        let mut hit_ids = Vec::new();
        for row in 0..36 {
            for column in 0..120 {
                if let Some(Action::SelectAgent(id)) =
                    agent_pulse_hit_test(area, column, row, false, &app)
                {
                    if !hit_ids.contains(&id) {
                        hit_ids.push(id);
                    }
                }
            }
        }
        assert_eq!(hit_ids.len(), 2, "one tile hit target per agent");
    }

    #[test]
    fn agent_planets_stage_centers_now_playing_title_and_volume_bar() {
        let mut app = app_with_agents(vec![
            pulse_agent("ws", "p1", Some("research"), AgentStatus::Working),
            pulse_agent("ws", "p2", None, AgentStatus::Idle),
        ]);
        play_first(&mut app);
        let station = app.current_station().unwrap().id.clone();
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station,
            title: "Aurora Drift - Nightwave".to_string(),
        }));
        app.apply(Action::ToggleAgentOverlay);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            text.contains("Agent Planets"),
            "the stage heading renders: {text}"
        );
        assert!(text.contains("· 2 active"), "the heading counts: {text}");
        assert!(
            text.contains(app.now_playing_title().unwrap()),
            "the ICY title renders on the stage: {text}"
        );
        assert!(
            text.contains("volume 60%"),
            "the numeric volume renders: {text}"
        );
        assert!(
            text.contains("─"),
            "the volume line accent fill renders: {text}"
        );
    }

    #[test]
    fn agent_planets_stage_shows_calm_copy_without_a_station() {
        let mut app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        app.apply(Action::ToggleAgentOverlay);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            text.contains("no station playing"),
            "the stage keeps calm no-station copy: {text}"
        );
    }

    #[test]
    fn agent_planets_heading_uses_single_view_title_case() {
        let mut app = app_with_agents(vec![
            pulse_agent("ws", "p1", Some("research"), AgentStatus::Working),
            pulse_agent("ws", "p2", Some("review"), AgentStatus::Idle),
        ]);
        app.apply(Action::ToggleAgentOverlay);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            text.contains("Agent Planets · 2 active"),
            "the heading matches Single View Title Case: {text}"
        );
        assert!(
            !text.contains("AGENT PLANETS"),
            "no uppercase stage heading remains: {text}"
        );
    }

    #[test]
    fn agent_planets_reuses_the_exact_single_view_volume_line_in_its_title_block() {
        let mut app = app_with_agents(vec![
            pulse_agent("ws", "p1", Some("research"), AgentStatus::Working),
            pulse_agent("ws", "p2", Some("review"), AgentStatus::Idle),
        ]);
        play_first(&mut app);
        app.apply(Action::ToggleAgentOverlay);
        let stage = buffer_text(&render_buffer(&app, 120, 36));
        let signal_line =
            signal_view_volume_line(&app, &Theme::for_name(ThemeName::Minimal), 120).to_string();
        assert!(
            stage.contains(&signal_line),
            "the stage carries the byte-for-byte Signal View volume line: {stage}"
        );
        assert!(
            stage.contains("volume 60%"),
            "the Signal View volume prefix renders: {stage}"
        );
        assert!(
            !stage.contains("Vol "),
            "the Now Playing gauge label never renders on the stage: {stage}"
        );
    }

    #[test]
    fn signal_view_never_shows_the_collage_canvas() {
        let mut app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        app.apply(Action::ToggleSignalView);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(!text.contains("Agent Planets"), "no canvas: {text}");
        assert!(
            !text.contains("● 1 active"),
            "Signal View reserves no Pulse line: {text}"
        );

        // Even with the canvas open underneath, Signal View wins the surface.
        app.apply(Action::LeaveSignalView);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::ToggleSignalView);
        let text = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            !text.contains("Agent Planets"),
            "no canvas over Signal View"
        );
    }

    #[test]
    fn stale_and_unavailable_canvas_states_render_calmly() {
        let mut app = app_with_agents(vec![pulse_agent(
            "ws",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale = buffer_text(&render_buffer(&app, 120, 36));
        assert!(stale.contains("reconnecting"), "stale banner: {stale}");
        assert!(
            tile_cell_count(&stale) > 0,
            "stale keeps the last tile: {stale}"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let unavailable = buffer_text(&render_buffer(&app, 120, 36));
        assert!(
            unavailable.contains("agents · unavailable · retrying"),
            "calm unavailable copy: {unavailable}"
        );
        assert_eq!(
            tile_cell_count(&unavailable),
            0,
            "unavailable hides tiles: {unavailable}"
        );
    }
}
