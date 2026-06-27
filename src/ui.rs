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
//! values. The [`render_spectrum`] "Spectrum Stack" renderer is shared across
//! every tier.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap},
    Frame,
};

use crate::app::{App, FocusPane, SearchStatus};
use crate::catalog::Section;
use crate::layout::LayoutTier;
use crate::model::{CodecKind, PlaybackState, Station};
use crate::theme::Theme;

/// Render the entire UI for the current terminal size.
///
/// This is the only entry point the controller/event loop calls each frame. It
/// reads display data from `app` and draws into the frame's buffer; it performs
/// no state mutation.
pub fn render(app: &App, frame: &mut Frame) {
    render_into(app, frame.area(), frame.buffer_mut());
}

/// Render into an explicit area and buffer.
///
/// Split out from [`render`] so tests can drive rendering with a standalone
/// [`Buffer`], with no real terminal or backend.
fn render_into(app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme().theme();
    // Paint the themed canvas first so empty cells carry the background.
    buf.set_style(area, theme.base_style());

    match LayoutTier::from_size(area.width, area.height) {
        LayoutTier::Wide => render_wide(app, &theme, area, buf),
        LayoutTier::Medium => render_medium(app, &theme, area, buf),
        LayoutTier::Compact => render_compact(app, &theme, area, buf),
    }
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

    render_sections(app, theme, cols[0], buf);
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

    Line::from(vec![
        Span::styled("/ ", theme.accent_style()),
        query_span,
        Span::raw("   "),
        search_status_span(app, theme),
        Span::raw("  "),
        Span::styled(
            format!("{} results", app.visible().len()),
            Style::default().fg(theme.foreground),
        ),
        Span::raw("   "),
        network_span(app, theme),
    ])
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

/// Music / Spoken-News browse shortcuts. Wide-tier "category context" pane.
fn render_sections(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let focused = app.focus() == FocusPane::Sections;
    let block = bordered_block(theme, "Browse", focused);
    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = Vec::new();
    for section in Section::ALL {
        lines.push(Line::styled(section.title(), theme.accent_style()));
        for category in section.categories() {
            lines.push(Line::styled(
                format!("  {}", category.title()),
                Style::default().fg(theme.muted),
            ));
        }
    }
    Paragraph::new(lines)
        .style(theme.base_style())
        .render(inner, buf);
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
        let note = if app.is_offline() {
            "No stations — offline"
        } else {
            "No stations"
        };
        Paragraph::new(Line::styled(note, Style::default().fg(theme.muted)))
            .style(theme.base_style())
            .render(inner, buf);
        return;
    }

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

    render_spectrum(theme, app, parts[1], buf);
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

    lines.push(Line::styled(
        format!("Volume {}%", app.settings().volume.get()),
        Style::default().fg(theme.foreground),
    ));
    lines
}

/// The shared "Spectrum Stack": vertical FFT bars colored by the active theme.
///
/// This single renderer is used by every layout tier. Bands map to columns and
/// fill from the bottom up; each column's color comes from the theme's
/// low/mid/high spectrum split via [`Theme::spectrum_color`].
fn render_spectrum(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let frame = app.viz();
    let bands = &frame.bands;
    if bands.is_empty() {
        return;
    }

    let count = (bands.len() as u16).min(area.width) as usize;
    let last = bands.len().saturating_sub(1).max(1) as f32;
    for (i, band) in bands.iter().take(count).enumerate() {
        let position = i as f32 / last;
        let color = theme.spectrum_color(position);
        let magnitude = band.clamp(0.0, 1.0);
        let filled = (magnitude * area.height as f32).round() as u16;
        let x = area.x + i as u16;
        for row in 0..filled {
            let y = area.y + area.height - 1 - row;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char('█').set_fg(color);
            }
        }
    }
}

/// Footer key hints plus network/offline state.
fn render_footer(app: &App, theme: &Theme, area: Rect, buf: &mut Buffer, compact: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let accent = theme.accent_style();
    let muted = Style::default().fg(theme.muted);

    let hints: &[(&str, &str)] = if compact {
        &[
            ("Tab", "focus"),
            ("/", "search"),
            ("\u{21B5}", "play"),
            ("Spc", "stop"),
            ("f", "fav"),
            ("t", "theme"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("Tab", "focus"),
            ("/", "search"),
            ("Enter", "play"),
            ("Space", "stop/play"),
            ("f", "favorite"),
            ("t", "theme"),
            ("q", "quit"),
        ]
    };

    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(*key, accent));
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
    use crate::catalog::Catalog;
    use crate::model::VizFrame;
    use crate::settings::Settings;
    use crate::theme::ThemeName;

    fn base_app() -> App {
        App::new(Settings::default(), Catalog::curated())
    }

    /// Render an app into a standalone buffer of the given size — no terminal.
    fn render_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_into(app, area, &mut buf);
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
    fn compact_keeps_station_context_and_player_without_fullscreen_visualizer() {
        // Compact needs width<72 or height<18 under the layout policy.
        assert_eq!(LayoutTier::from_size(70, 16), LayoutTier::Compact);
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9_f32; 16],
            0.8,
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
        // The compact visualizer is present but not full-screen: bars render and
        // the station list region above still has content.
        assert!(text.contains('█'));
    }

    #[test]
    fn spectrum_stack_is_shared_and_renders_themed_bars_across_tiers() {
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 16],
            1.0,
        ))));
        play_first(&mut app);

        let theme = app.theme().theme();
        for (w, h) in [(130, 32), (100, 24), (70, 16)] {
            let buf = render_buffer(&app, w, h);
            assert!(buffer_text(&buf).contains('█'), "no bars at {w}x{h}");
            // Bars use theme spectrum colors, not an ad hoc palette.
            assert!(
                has_fg(&buf, theme.spectrum_low)
                    || has_fg(&buf, theme.spectrum_mid)
                    || has_fg(&buf, theme.spectrum_high),
                "spectrum colors not themed at {w}x{h}"
            );
        }
    }

    #[test]
    fn offline_state_is_visible() {
        let mut app = base_app();
        app.apply(Action::SetOffline(true));
        let buf = render_buffer(&app, 130, 32);
        assert!(buffer_text(&buf).contains("OFFLINE"));
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
    fn renders_without_panicking_across_sizes_and_tiers() {
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.5_f32; 16],
            0.5,
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
}
