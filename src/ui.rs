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

use crate::app::{App, FocusPane, ListSource, SearchStatus};
use crate::layout::LayoutTier;
use crate::model::{CodecKind, PlaybackState, Station, VisualizerMode};
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

    render_visualizer(theme, app, parts[1], buf);
}

/// Draw the visualizer pane using the app's selected [`VisualizerMode`].
///
/// Every mode in the five-mode Calm Suite has a dedicated renderer, each driven
/// from the real [`VizFrame`] (FFT bands, RMS, or the time-domain waveform) and
/// each stretched to the full pane width. The match is exhaustive on purpose
/// (MIK-026): adding a future mode is a compile error here until it is wired,
/// rather than silently falling back to the Spectrum Stack.
fn render_visualizer(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    match app.visualizer_mode() {
        VisualizerMode::SpectrumStack => render_spectrum(theme, app, area, buf),
        VisualizerMode::PeakDots => render_peak_dots(theme, app, area, buf),
        VisualizerMode::WaveScope => render_wave_scope(theme, app, area, buf),
        VisualizerMode::MirrorWave => render_mirror_wave(theme, app, area, buf),
        VisualizerMode::AmbientPulse => render_ambient_pulse(theme, app, area, buf),
    }
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
    let columns = spectrum_columns(&app.viz().bands, area.width as usize);
    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        let color = theme.spectrum_color(position);
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

/// The "Peak Dots" visualizer: one themed dot per pane column, placed at the
/// column's FFT peak height instead of a filled bar.
///
/// A distinct renderer from [`render_spectrum`]: it shares the full-pane-width
/// [`spectrum_columns`] sampling and the theme's low/mid/high spectrum split, but
/// draws only the peak cell of each column as a dot so the visualizer reads as a
/// quieter peak band. Columns whose magnitude rounds to zero (silent/empty
/// frames) draw nothing. Pure function of the current [`VizFrame`]; it carries no
/// animation or mode-specific state.
fn render_peak_dots(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = spectrum_columns(&app.viz().bands, area.width as usize);
    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        let filled = (magnitude * area.height as f32).round() as u16;
        if filled == 0 {
            continue;
        }
        let color = theme.spectrum_color(position);
        let x = area.x + i as u16;
        // The peak sits at the top of where a filled bar would reach.
        let y = area.y + area.height - filled;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char('●').set_fg(color);
        }
    }
}

/// The "WaveScope" visualizer: an oscilloscope trace of [`VizFrame::waveform`].
///
/// One trace point per pane column, sampled from the full-width
/// [`waveform_columns`] interpolation so the scope spans the whole pane. A
/// sample of `0.0` sits on the vertical center, `+1.0` reaches the top, and
/// `-1.0` the bottom; the point's color comes from the theme's spectrum split
/// keyed on the signal's amplitude so louder excursions read brighter. Empty and
/// all-zero waveforms both render as a flat baseline (stable silence), per the
/// MIK-024 reviewer note. Pure function of the current [`VizFrame`].
fn render_wave_scope(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = waveform_columns(&app.viz().waveform, area.width as usize);
    let center = (area.height - 1) / 2;
    let half = (area.height - 1) as f32 / 2.0;
    let top = area.y;
    let bottom = area.y + area.height - 1;
    for (i, (sample, _position)) in columns.into_iter().enumerate() {
        let offset = (sample * half).round() as i32;
        let y = ((area.y + center) as i32 - offset).clamp(top as i32, bottom as i32) as u16;
        let color = theme.spectrum_color(sample.abs());
        let x = area.x + i as u16;
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char('•').set_fg(color);
        }
    }
}

/// The "MirrorWave" visualizer: a symmetrical oscilloscope around the center.
///
/// For each pane column the waveform sample's magnitude is mirrored above and
/// below the vertical center, giving a calmer, balanced scope than the raw
/// [`render_wave_scope`] trace. The center cell is always drawn so the baseline
/// stays visible, and louder samples extend the symmetric bars further out.
/// Color follows the theme's spectrum split keyed on amplitude. Empty and
/// all-zero waveforms render as the flat center baseline (silence). Pure
/// function of the current [`VizFrame`].
fn render_mirror_wave(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = waveform_columns(&app.viz().waveform, area.width as usize);
    let center = area.y + (area.height - 1) / 2;
    let reach_max = (area.height - 1) / 2;
    let bottom = area.y + area.height - 1;
    for (i, (sample, _position)) in columns.into_iter().enumerate() {
        let amplitude = sample.abs();
        let reach = (amplitude * reach_max as f32).round() as u16;
        let color = theme.spectrum_color(amplitude);
        let x = area.x + i as u16;
        for r in 0..=reach {
            let up = center as i32 - r as i32;
            if up >= area.y as i32 {
                if let Some(cell) = buf.cell_mut((x, up as u16)) {
                    cell.set_char('┃').set_fg(color);
                }
            }
            let down = center + r;
            if down <= bottom {
                if let Some(cell) = buf.cell_mut((x, down)) {
                    cell.set_char('┃').set_fg(color);
                }
            }
        }
    }
}

/// The "AmbientPulse" visualizer: a low-noise glow driven by RMS and bands.
///
/// Each column blends the interpolated FFT band magnitude with the frame RMS
/// into a calm level, drawn as a vertically centered shaded band whose height
/// and shade density (`░`/`▒`/`▓`) grow with that level. When the frame carries
/// no bands the RMS alone pulses uniformly across the pane, so the mode stays
/// real-data-driven rather than animating on its own. A silent frame draws
/// nothing. Bands are stretched to the full pane width via [`spectrum_columns`].
fn render_ambient_pulse(theme: &Theme, app: &App, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let frame = app.viz();
    let rms = frame.rms;
    // Prefer the band shape for the per-column glow; with no bands the RMS still
    // pulses the whole pane so the mode reflects real playback level.
    let columns = if frame.bands.is_empty() {
        let last_col = area.width.saturating_sub(1) as usize;
        (0..area.width as usize)
            .map(|col| {
                let position = if last_col == 0 {
                    0.0
                } else {
                    col as f32 / last_col as f32
                };
                (rms, position)
            })
            .collect::<Vec<_>>()
    } else {
        spectrum_columns(&frame.bands, area.width as usize)
    };

    for (i, (magnitude, position)) in columns.into_iter().enumerate() {
        let level = (magnitude * 0.6 + rms * 0.4).clamp(0.0, 1.0);
        let shade = if level < 0.15 {
            None
        } else if level < 0.45 {
            Some('░')
        } else if level < 0.75 {
            Some('▒')
        } else {
            Some('▓')
        };
        let Some(shade) = shade else { continue };

        let fill = (level * area.height as f32).round() as u16;
        if fill == 0 {
            continue;
        }
        let start = area.y + (area.height - fill) / 2;
        let color = theme.spectrum_color(position);
        let x = area.x + i as u16;
        for y in start..(start + fill).min(area.y + area.height) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(shade).set_fg(color);
            }
        }
    }
}

/// Resample the time-domain `waveform` into one `(sample, position)` column per
/// pane cell so the waveform modes fill the full pane width.
///
/// `sample` is the interpolated signed amplitude in `-1.0..=1.0`, linearly
/// interpolated between the two nearest waveform points; `position` is the
/// column's normalized `0.0..=1.0` location, used for the theme color split. An
/// empty waveform is treated as flat silence: every column samples `0.0`, so an
/// empty and an all-zero waveform render identically (a flat baseline), per the
/// MIK-024 reviewer note. Returns an empty vector only for zero width.
fn waveform_columns(waveform: &[f32], width: usize) -> Vec<(f32, f32)> {
    if width == 0 {
        return Vec::new();
    }
    let last_col = width - 1;
    let position_of = |col: usize| {
        if last_col == 0 {
            0.0
        } else {
            col as f32 / last_col as f32
        }
    };
    if waveform.is_empty() {
        return (0..width).map(|col| (0.0, position_of(col))).collect();
    }
    let last_sample = waveform.len() - 1;
    (0..width)
        .map(|col| {
            let position = position_of(col);
            let point = position * last_sample as f32;
            let lo = point.floor() as usize;
            let hi = (lo + 1).min(last_sample);
            let frac = point - lo as f32;
            let sample = waveform[lo] + (waveform[hi] - waveform[lo]) * frac;
            (sample.clamp(-1.0, 1.0), position)
        })
        .collect()
}

/// Resample the FFT `bands` into one `(magnitude, position)` column per pane
/// cell so the Spectrum Stack fills the full pane width.
///
/// `magnitude` is the bar height in `0.0..=1.0`, linearly interpolated between
/// the two nearest bands so columns stay smooth when the pane is wider than the
/// band count. `position` is the column's normalized `0.0..=1.0` location, used
/// by [`Theme::spectrum_color`] so the low/mid/high color split stretches across
/// the whole pane rather than only the first `bands.len()` columns.
///
/// Pure and deterministic; returns an empty vector for empty bands or zero
/// width, so callers stay safe for tiny panes and silent/empty frames.
fn spectrum_columns(bands: &[f32], width: usize) -> Vec<(f32, f32)> {
    if bands.is_empty() || width == 0 {
        return Vec::new();
    }
    let last_band = bands.len() - 1;
    let last_col = width - 1;
    (0..width)
        .map(|col| {
            let position = if last_col == 0 {
                0.0
            } else {
                col as f32 / last_col as f32
            };
            // Map the column onto the band range and interpolate between the two
            // nearest bands. Endpoints land exactly on the first/last band.
            let sample = position * last_band as f32;
            let lo = sample.floor() as usize;
            let hi = (lo + 1).min(last_band);
            let frac = sample - lo as f32;
            let magnitude = bands[lo] + (bands[hi] - bands[lo]) * frac;
            (magnitude.clamp(0.0, 1.0), position)
        })
        .collect()
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
            ("v", "viz"),
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
            ("v", "visualizer"),
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

/// The empty-state note for the visible station list, specific to the active
/// source.
///
/// The Favorites source gets an explicit save hint rather than the generic "No
/// stations" line: an empty favorites list is a normal first-run state the user
/// resolves by pressing `f`. That hint is shown even offline — saved favorites
/// are retryable stream entries, not stations guaranteed to play offline, so the
/// empty state keeps guiding the user to save rather than implying offline
/// availability. Other sources keep the offline-aware generic note.
fn empty_list_note(app: &App) -> &'static str {
    if app.active_source() == ListSource::Favorites {
        "No favorites yet — press f on a station to save it"
    } else if app.is_offline() {
        "No stations — offline"
    } else {
        "No stations"
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

    /// Render only the Browse rail into a standalone buffer so per-row
    /// assertions are not crossed by the Results / Now Playing columns.
    fn render_browse_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_browse(app, &theme, area, &mut buf);
        buf
    }

    /// The buffer line (relative row) that contains `needle`, if any.
    fn line_with(text: &str, needle: &str) -> Option<usize> {
        text.lines().position(|line| line.contains(needle))
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
            vec![1.0_f32; 16],
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

    // --- Spectrum pane-width usage (MIK-025) ----------------------------

    #[test]
    fn spectrum_columns_resamples_to_full_width_preserving_endpoints() {
        // The helper produces exactly one column per pane cell, so the bars use
        // the full pane width instead of the band count.
        let bands = [0.2_f32, 0.4, 0.6, 0.8];
        let cols = spectrum_columns(&bands, 16);
        assert_eq!(cols.len(), 16, "one column per pane cell");

        // Endpoints map to the first/last band magnitude exactly.
        assert!((cols.first().unwrap().0 - 0.2).abs() < 1e-6);
        assert!((cols.last().unwrap().0 - 0.8).abs() < 1e-6);

        // Positions span the full 0.0..=1.0 range so the low/mid/high color
        // split stretches across the whole pane.
        assert_eq!(cols.first().unwrap().1, 0.0);
        assert!((cols.last().unwrap().1 - 1.0).abs() < 1e-6);

        // Monotonic bands resample to non-decreasing magnitudes (smooth fill).
        for pair in cols.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0 - 1e-6,
                "interpolated magnitudes should not jitter for monotonic bands"
            );
        }
    }

    #[test]
    fn spectrum_columns_is_deterministic() {
        let bands = [0.1_f32, 0.5, 0.9];
        assert_eq!(spectrum_columns(&bands, 24), spectrum_columns(&bands, 24));
    }

    #[test]
    fn spectrum_columns_is_safe_for_empty_bands_and_zero_width() {
        assert!(spectrum_columns(&[], 40).is_empty());
        assert!(spectrum_columns(&[0.5_f32; 8], 0).is_empty());
        // A single band still produces a full-width run without panicking.
        assert_eq!(spectrum_columns(&[0.7_f32], 5).len(), 5);
    }

    #[test]
    fn spectrum_fills_full_pane_width_when_wider_than_bands() {
        // Regression: the old renderer drew only min(bands.len(), width)
        // columns, leaving the right side of a wide pane blank. With far more
        // pane cells than bands, every column must now carry a bar.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render_spectrum(&theme, &app, area, &mut buf);

        let bottom = area.height - 1;
        for x in 0..area.width {
            assert_eq!(
                buf.cell((x, bottom)).unwrap().symbol(),
                "█",
                "column {x} not filled across the full pane width"
            );
        }
    }

    #[test]
    fn spectrum_is_safe_for_tiny_panes_and_silent_frames() {
        let theme = Theme::for_name(ThemeName::Minimal);
        // Silent frame: bands present but zero, so no bars are drawn.
        let mut silent = base_app();
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        // Empty frame: no bands at all.
        let mut empty = base_app();
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));

        for app in [&silent, &empty] {
            for (w, h) in [(1, 1), (2, 1), (1, 4), (40, 1)] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_spectrum(&theme, app, area, &mut buf);
                assert!(
                    !buffer_text(&buf).contains('█'),
                    "silent/empty frame must draw no bars at {w}x{h}"
                );
            }
        }
    }

    // --- PeakDots visualizer mode (MIK-026) -----------------------------

    use crate::model::VisualizerMode;

    /// Render only the active visualizer into a standalone buffer, the routed
    /// path used by Now Playing (mode-aware) rather than a fixed renderer.
    fn render_viz_buffer(app: &App, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = app.theme().theme();
        render_visualizer(&theme, app, area, &mut buf);
        buf
    }

    #[test]
    fn selecting_peak_dots_changes_the_rendered_visualizer() {
        // The default SpectrumStack fills bars; cycling to PeakDots routes to a
        // distinct renderer that emphasizes the per-column peak with a dot.
        let mut app = base_app();
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);

        let stack_text = buffer_text(&render_viz_buffer(&app, 32, 8));
        assert!(stack_text.contains('█'), "spectrum stack bars missing");
        assert!(
            !stack_text.contains('●'),
            "spectrum stack must not draw peak dots"
        );

        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);
        let dots_text = buffer_text(&render_viz_buffer(&app, 32, 8));
        assert!(
            dots_text.contains('●'),
            "peak dots must draw dot markers: {dots_text}"
        );
        assert_ne!(
            stack_text, dots_text,
            "the rendered visualizer must change with the selected mode"
        );
    }

    #[test]
    fn peak_dots_fills_full_pane_width_with_real_bands() {
        // Far more pane cells than bands: every column carries a peak dot, drawn
        // from the shared full-width sampling helper, not just bands.len() cells.
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut app = base_app();
        app.apply(Action::CycleVisualizerMode); // SpectrumStack -> PeakDots
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 4],
            1.0,
            vec![],
        ))));
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        render_peak_dots(&theme, &app, area, &mut buf);

        // Full magnitude lands the peak dot on the top row of every column.
        for x in 0..area.width {
            assert_eq!(
                buf.cell((x, 0)).unwrap().symbol(),
                "●",
                "column {x} not capped with a peak dot across the full pane width"
            );
        }
    }

    #[test]
    fn peak_dots_uses_theme_spectrum_colors() {
        // Dots are colored by the low/mid/high spectrum split, not an ad hoc
        // palette, so they stay theme-driven like the shared Spectrum Stack.
        let mut app = base_app();
        app.apply(Action::CycleTheme); // Minimal -> Neon
        app.apply(Action::CycleVisualizerMode); // -> PeakDots
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0_f32; 8],
            1.0,
            vec![],
        ))));
        let theme = app.theme().theme();
        let buf = render_viz_buffer(&app, 30, 8);
        assert!(
            has_fg(&buf, theme.spectrum_low)
                || has_fg(&buf, theme.spectrum_mid)
                || has_fg(&buf, theme.spectrum_high),
            "peak dot colors not themed"
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
    fn peak_dots_is_safe_for_tiny_panes_and_silent_frames() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let mut silent = base_app();
        silent.apply(Action::CycleVisualizerMode);
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
        let mut empty = base_app();
        empty.apply(Action::CycleVisualizerMode);
        empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            Vec::<f32>::new(),
            0.0,
            vec![],
        ))));

        for app in [&silent, &empty] {
            for (w, h) in [(1, 1), (2, 1), (1, 4), (40, 1)] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_peak_dots(&theme, app, area, &mut buf);
                assert!(
                    !buffer_text(&buf).contains('●'),
                    "silent/empty frame must draw no peak dots at {w}x{h}"
                );
            }
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

    /// The set of buffer rows (relative y) whose line contains `glyph`.
    fn rows_with(text: &str, glyph: char) -> std::collections::BTreeSet<usize> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| line.contains(glyph))
            .map(|(y, _)| y)
            .collect()
    }

    /// True if the cell at `(x, y)` carries `glyph`.
    fn cell_is(buf: &Buffer, x: u16, y: u16, glyph: &str) -> bool {
        buf.cell((x, y)).unwrap().symbol() == glyph
    }

    #[test]
    fn wave_scope_renders_a_waveform_trace() {
        // WaveScope draws one trace point per column from VizFrame::waveform; a
        // non-flat waveform produces a trace that varies in height.
        let mut app = app_in_mode(VisualizerMode::WaveScope);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![-1.0, -0.5, 0.0, 0.5, 1.0],
        ))));
        let text = buffer_text(&render_viz_buffer(&app, 16, 7));
        assert!(text.contains('•'), "wave scope trace missing: {text}");
        assert!(
            rows_with(&text, '•').len() > 1,
            "wave scope trace is flat for a non-flat waveform: {text}"
        );
    }

    #[test]
    fn wave_scope_treats_empty_and_zeroed_waveform_as_flat_silence() {
        // MIK-024 reviewer note: empty and all-zero waveforms are both flat
        // silence and must render identically — a single baseline row.
        let render = |wf: Vec<f32>| {
            let mut app = app_in_mode(VisualizerMode::WaveScope);
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                vec![],
                0.0,
                wf,
            ))));
            buffer_text(&render_viz_buffer(&app, 16, 7))
        };
        let empty = render(vec![]);
        let zeroed = render(vec![0.0; 8]);
        assert_eq!(
            empty, zeroed,
            "empty and zeroed waveform must render identically"
        );
        assert_eq!(
            rows_with(&empty, '•').len(),
            1,
            "silence must be a single flat baseline: {empty}"
        );
    }

    #[test]
    fn wave_scope_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::WaveScope);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.2, -0.2, 0.6, -0.6],
        ))));
        let (w, h) = (40u16, 8u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            assert!(
                (0..h).any(|y| cell_is(&buf, x, y, "•")),
                "wave scope column {x} empty (not full width)"
            );
        }
    }

    #[test]
    fn mirror_wave_is_symmetric_around_center() {
        let mut app = app_in_mode(VisualizerMode::MirrorWave);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.8; 8],
        ))));
        let h = 7u16;
        let buf = render_viz_buffer(&app, 20, h);
        assert!(buffer_text(&buf).contains('┃'), "mirror wave bars missing");
        let center = (h - 1) / 2;
        let x = 5u16;
        for r in 0..=center {
            assert_eq!(
                cell_is(&buf, x, center - r, "┃"),
                cell_is(&buf, x, center + r, "┃"),
                "mirror wave not symmetric at offset {r}"
            );
        }
    }

    #[test]
    fn mirror_wave_reflects_waveform_and_is_flat_for_silence() {
        let render = |wf: Vec<f32>| {
            let mut app = app_in_mode(VisualizerMode::MirrorWave);
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                vec![],
                0.0,
                wf,
            ))));
            buffer_text(&render_viz_buffer(&app, 20, 7))
        };
        let loud = render(vec![0.9; 8]);
        let silent = render(vec![]);
        assert!(
            rows_with(&loud, '┃').len() > rows_with(&silent, '┃').len(),
            "louder waveform must reach further from center: {loud}"
        );
        assert_eq!(
            rows_with(&silent, '┃').len(),
            1,
            "silence must be a single baseline row: {silent}"
        );
        assert_eq!(
            silent,
            render(vec![0.0; 8]),
            "empty and zeroed waveform must render identically"
        );
    }

    #[test]
    fn mirror_wave_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::MirrorWave);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.0,
            vec![0.3, -0.3, 0.7],
        ))));
        let (w, h) = (40u16, 8u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            assert!(
                (0..h).any(|y| cell_is(&buf, x, y, "┃")),
                "mirror wave column {x} empty (not full width)"
            );
        }
    }

    /// True if `text` contains any ambient shade glyph.
    fn has_ambient_shade(text: &str) -> bool {
        text.chars().any(|c| matches!(c, '░' | '▒' | '▓'))
    }

    #[test]
    fn ambient_pulse_is_rms_driven_not_fake_animation() {
        // Real RMS + bands produce ambient shading; a silent frame draws nothing
        // (proving the mode reacts to data instead of animating on its own).
        let mut active = app_in_mode(VisualizerMode::AmbientPulse);
        active.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.9; 8],
            0.8,
            vec![],
        ))));
        let active_text = buffer_text(&render_viz_buffer(&active, 24, 7));
        assert!(
            has_ambient_shade(&active_text),
            "ambient pulse drew nothing for real data: {active_text}"
        );

        let mut silent = app_in_mode(VisualizerMode::AmbientPulse);
        silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(8))));
        let silent_text = buffer_text(&render_viz_buffer(&silent, 24, 7));
        assert!(
            !has_ambient_shade(&silent_text),
            "ambient pulse must be silent for a silent frame: {silent_text}"
        );
    }

    #[test]
    fn ambient_pulse_pulses_from_rms_without_bands() {
        // RMS alone (no bands) still drives the ambient display.
        let mut app = app_in_mode(VisualizerMode::AmbientPulse);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![],
            0.9,
            vec![],
        ))));
        let text = buffer_text(&render_viz_buffer(&app, 24, 7));
        assert!(has_ambient_shade(&text), "rms-only ambient missing: {text}");
    }

    #[test]
    fn ambient_pulse_fills_full_pane_width() {
        let mut app = app_in_mode(VisualizerMode::AmbientPulse);
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![1.0; 8],
            1.0,
            vec![],
        ))));
        let (w, h) = (40u16, 7u16);
        let buf = render_viz_buffer(&app, w, h);
        for x in 0..w {
            let filled = (0..h).any(|y| {
                let s = buf.cell((x, y)).unwrap().symbol();
                s == "░" || s == "▒" || s == "▓"
            });
            assert!(filled, "ambient column {x} empty (not full width)");
        }
    }

    #[test]
    fn each_visualizer_mode_renders_distinctly() {
        // Cycling through every mode with the same frame yields five distinct
        // renders, so each mode has its own visual language.
        let frame = VizFrame::new(
            vec![1.0, 0.2, 0.8, 0.4, 0.9, 0.1, 0.6, 0.3],
            0.7,
            vec![-0.9, -0.3, 0.2, 0.8, 0.5, -0.6, 0.1, 0.7],
        );
        let mut app = base_app();
        let mut texts = Vec::new();
        for _ in 0..VisualizerMode::ALL.len() {
            app.apply(Action::Audio(AudioEvent::Viz(frame.clone())));
            texts.push(buffer_text(&render_viz_buffer(&app, 32, 9)));
            app.apply(Action::CycleVisualizerMode);
        }
        for i in 0..texts.len() {
            for j in (i + 1)..texts.len() {
                assert_ne!(texts[i], texts[j], "modes {i} and {j} render identically");
            }
        }
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
    fn waveform_and_ambient_modes_are_safe_for_tiny_panes_and_silent_frames() {
        for mode in [
            VisualizerMode::WaveScope,
            VisualizerMode::MirrorWave,
            VisualizerMode::AmbientPulse,
        ] {
            let mut silent = app_in_mode(mode);
            silent.apply(Action::Audio(AudioEvent::Viz(VizFrame::silent(16))));
            let mut empty = app_in_mode(mode);
            empty.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                Vec::<f32>::new(),
                0.0,
                Vec::<f32>::new(),
            ))));
            for app in [&silent, &empty] {
                for (w, h) in [(0u16, 0u16), (1, 1), (2, 1), (1, 4), (40, 1)] {
                    let _ = render_viz_buffer(app, w, h);
                }
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
}
