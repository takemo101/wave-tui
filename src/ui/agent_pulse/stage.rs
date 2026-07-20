//! Agent Planets stage rendering: the stage chrome and the scope/planet
//! field.
//!
//! Read-only over [`App`]: this module draws what [`super::geometry`] and
//! [`super::surface`] derived, and never mutates app state or touches the
//! Herdr adapter. It owns the source-of-geometry precedence — stale wins
//! with the reducer-captured composition, then `--low-power` with the
//! App-captured first frame, then live — and hands the resulting frames and
//! frozen orbit seconds to the pure geometry, which is what makes freezing a
//! matter of inputs rather than a flag threaded through the renderer.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use std::collections::HashSet;
use std::time::Instant;

use super::geometry::{
    agent_stage_layout, collage_layout, rects_overlap, CollageLayout, CollageTile, SILENCE_ENERGY,
    SUN_GLYPH, VIGNETTE_BAND,
};
use super::modal::{render_agent_focus_notice, render_agent_table_modal};
use super::surface::{
    planet_geometry, planet_label_candidate, planet_palette, planet_surface, status_frame,
    surface_cells, PlanetGeometry, PlanetSurface, CRATER_GLYPH, PLANET_BODY_GLYPH,
};
use crate::app::{AgentPulseConnection, AgentView, App};
use crate::herdr::AgentStatus;
use crate::model::VizFrame;
use crate::theme::Theme;

/// Draw the stage chrome and the scope/planet field.
pub(super) fn render_agent_planets_stage(
    app: &App,
    theme: &Theme,
    low_power: bool,
    now: Instant,
    area: Rect,
    buf: &mut Buffer,
) {
    let connection = app.agent_pulse_connection();
    let agents = app.active_agents();
    let stale = connection == AgentPulseConnection::Stale;
    let muted = Style::default().fg(theme.muted);

    let stage = agent_stage_layout(area);
    render_stage_heading(theme, agents.len(), stale, stage.heading, buf);
    render_stage_title_block(app, theme, stale, stage.title_block, buf);
    render_stage_footer(theme, app.is_agent_details_open(), stage.footer, buf);

    if connection == AgentPulseConnection::Unavailable {
        center_copy(buf, stage.field, "agents · unavailable · retrying", muted);
        render_agent_focus_notice(app, theme, stage.field, now, buf);
        return;
    }

    let field = stage.field;
    // Geometry-source precedence: stale always wins with the display captured
    // by the reducer at the Connected→Stale edge; otherwise `--low-power`
    // renders the App-captured first frame so no audio-driven trace, disc,
    // or bracket geometry advances; live renders use the current frame plus
    // the real prior frames behind it.
    let live_history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
    let fallback = (app.viz(), live_history.as_slice());
    let (frame, history) = if stale {
        app.stale_viz().unwrap_or(fallback)
    } else if low_power {
        app.low_power_viz().unwrap_or(fallback)
    } else {
        fallback
    };
    // Orbit phases share the render's freeze rules: live and stale read the
    // current effective Working time at `now` — stale phases were banked by
    // the reducer at the Connected→Stale edge, so they hold still on their
    // own — while low power reads the layout captured at low-power entry,
    // so no planet moves at all. Only the slow monotonic clock moves a
    // planet; audio never does.
    let orbit_secs = super::orbit_secs_for(app, agents, low_power, now);
    let layout = collage_layout(agents, &orbit_secs, frame, history, field);

    render_vignette(buf, field, layout.background.vignette, theme, stale);
    for layer in &layout.background.layers {
        let mut style = Style::default().fg(theme.spectrum_color(layer.color_position));
        if layer.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        let style = with_stale(style, stale);
        for cell in &layer.cells {
            buf.set_string(cell.x, cell.y, layer.glyph, style);
        }
    }

    if agents.is_empty() {
        center_copy(buf, field, "agents · none active", muted);
        render_agent_focus_notice(app, theme, field, now, buf);
        return;
    }

    // The static theme-derived sun: field-centered decoration drawn before
    // every planet, never a hit target, never moved by audio, time, or
    // status. It rests dim with the silent scope and hides with the field
    // when the integration is unavailable.
    if let Some((x, y)) = layout.sun {
        let mut sun_style = Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD);
        if frame.rms() <= SILENCE_ENERGY {
            sun_style = sun_style.add_modifier(Modifier::DIM);
        }
        buf.set_string(x, y, SUN_GLYPH, own_emphasis(with_stale(sun_style, stale)));
    }

    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));

    // One geometry per tile, shared by every planet pass; the selected
    // planet alone derives its focus brackets. Live interior status
    // treatment reads the App-held last audible frame, so silence of any
    // length — beyond the bounded display history — freezes it instead of
    // advancing or suppressing it. Stale and low power keep deriving from
    // their own captured frames, never a later live capture.
    let treatment_frame = if stale || low_power {
        status_frame(frame, history)
    } else {
        app.status_viz()
            .unwrap_or_else(|| status_frame(frame, history))
    };
    let geometries: Vec<PlanetGeometry> = layout
        .tiles
        .iter()
        .map(|tile| {
            let status = agents
                .get(tile.index)
                .map(|view| view.status)
                .unwrap_or(AgentStatus::Unknown);
            planet_geometry(
                tile,
                field,
                status,
                treatment_frame,
                Some(tile.index) == selected_index,
            )
        })
        .collect();

    // Planets draw in stable slot order; the selected planet comes forward
    // last.
    for (tile, geometry) in layout.tiles.iter().zip(&geometries) {
        if Some(tile.index) == selected_index {
            continue;
        }
        render_planet(buf, tile, geometry, agents, theme, stale);
    }
    if let Some(selected) = selected_index {
        if let Some((tile, geometry)) = layout
            .tiles
            .iter()
            .zip(&geometries)
            .find(|(tile, _)| tile.index == selected)
        {
            render_planet(buf, tile, geometry, agents, theme, stale);
        }
    }

    render_planet_labels(
        buf,
        &layout,
        &geometries,
        agents,
        theme,
        stale,
        selected_index,
    );
    render_agent_table_modal(app, theme, stale, field, now, buf);
}

/// Centered stage heading: `Agent Planets · n active` in the same Title
/// Case presentation as Single View, with the quiet reconnect note appended
/// while the connection is stale.
pub(super) fn render_stage_heading(
    theme: &Theme,
    count: usize,
    stale: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut heading = theme.accent_style();
    let mut count_style = Style::default().fg(theme.muted);
    if stale {
        heading = heading.add_modifier(Modifier::DIM);
        count_style = count_style.add_modifier(Modifier::DIM);
    }
    let mut spans = vec![
        Span::styled("Agent Planets", heading),
        Span::styled(format!(" · {count} active"), count_style),
    ];
    if stale {
        spans.push(Span::styled(
            " · reconnecting",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }
    Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .style(theme.base_style())
        .render(area, buf);
}

/// The stage's primary title: ICY now-playing title, then station name, then
/// calm no-station copy. Mirrors the Signal View priority without exposing
/// any agent data.
pub(super) fn stage_primary_title(app: &App) -> String {
    if let Some(title) = app.now_playing_title() {
        title.to_string()
    } else if let Some(station) = app.current_station() {
        station.name.as_str().to_string()
    } else {
        "no station playing".to_string()
    }
}

/// Centered title block: the current title truncated to the row budget with
/// the same two-line wrapping approach as Signal View, then the exact Single
/// View volume line as the lowest title-metadata row — the same placement it
/// has in Signal View. The line is reused verbatim, so it is never restyled
/// here.
pub(super) fn render_stage_title_block(
    app: &App,
    theme: &Theme,
    stale: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);
    if stale {
        style = style.add_modifier(Modifier::DIM);
    }
    let mut title = crate::ui::title_lines(&stage_primary_title(app), area.width);
    title.truncate((area.height as usize).saturating_sub(1).max(1));
    let mut lines: Vec<Line> = title
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect();
    if area.height >= 2 {
        lines.push(crate::ui::signal_view_volume_line(app, theme, area.width));
    }
    Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(theme.base_style())
        .render(area, buf);
}

/// Centered restrained footer: selection, player, and close hints. `z` is
/// deliberately not advertised — Single View is not a stage action. Pane
/// focus belongs to the agent table while it is open, so its `O` hint never
/// competes with the modal-local control.
pub(super) fn render_stage_footer(theme: &Theme, table_open: bool, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let hint = if table_open {
        "Tab/↑↓ select · Enter/Esc close · a close"
    } else {
        "Tab/↑↓/click select · Enter table · O open pane · Space play · a/Esc close"
    };
    Paragraph::new(hint)
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted))
        .render(area, buf);
}

/// Draw one agent planet in order: the disc-mask body with its stable
/// Banded Worlds surface, then the interior status cells over the
/// identity paint — Working's narrow bright band bolds its identity
/// cells and advances only with newly played frames, Blocked's single
/// error cell weakly pulses between dim and plain `theme.error`, Idle
/// stays still and muted, Done and Unknown keep the whole body dim —
/// and, for the selected planet only, its four corner focus brackets
/// drawn as a foreground line color in the theme selection accent, never
/// a painted selection background. Silence, stale, and low power freeze
/// the status treatment at its last played frame — the geometry already
/// derives from that frame, so the band stays bright and the pulse holds
/// its half instead of resting or dimming away. Selection never restyles
/// the body; the brackets are the only focus treatment.
pub(super) fn render_planet(
    buf: &mut Buffer,
    tile: &CollageTile,
    geometry: &PlanetGeometry,
    agents: &[AgentView],
    theme: &Theme,
    stale: bool,
) {
    let Some(view) = agents.get(tile.index) else {
        return;
    };
    let silent = tile.energy <= SILENCE_ENERGY;
    let quiet_dim = |style: Style| {
        if silent {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    };

    // Banded Worlds surface: identity chooses the family and the theme
    // spectrum pair; status never picks the body palette — state stays on
    // the interior status cells painted after it.
    let surface = planet_surface(tile.seed);
    let palette = planet_palette(tile.seed);
    let paint = |color: Color| {
        let mut style = Style::default().fg(color);
        if matches!(
            view.status,
            AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown
        ) {
            style = style.add_modifier(Modifier::DIM);
        }
        own_emphasis(with_stale(quiet_dim(style), stale))
    };
    let base = paint(theme.spectrum_color(palette.base_position));
    let accent = paint(theme.spectrum_color(palette.accent_position));
    let accent_cells: HashSet<(u16, u16)> = surface_cells(surface, geometry, tile.seed)
        .into_iter()
        .collect();
    let glyph_for = |cell: (u16, u16)| {
        if accent_cells.contains(&cell) && surface == PlanetSurface::CrateredRock {
            CRATER_GLYPH
        } else {
            PLANET_BODY_GLYPH
        }
    };
    for &(x, y) in &geometry.body {
        let style = if accent_cells.contains(&(x, y)) {
            accent
        } else {
            base
        };
        buf.set_string(x, y, glyph_for((x, y)), style);
    }

    // Interior status cells repaint existing body cells after the identity
    // surface, so status never draws outside the disc mask. The geometry
    // already derives from the frozen frame under silence, stale, and low
    // power, so the treatment holds its last played state here instead of
    // being suppressed or dimmed away.
    for &(x, y) in &geometry.status_band {
        let color = if accent_cells.contains(&(x, y)) {
            theme.spectrum_color(palette.accent_position)
        } else {
            theme.spectrum_color(palette.base_position)
        };
        buf.set_string(
            x,
            y,
            glyph_for((x, y)),
            own_emphasis(with_stale(
                Style::default().fg(color).add_modifier(Modifier::BOLD),
                stale,
            )),
        );
    }
    if let Some(cell) = geometry.error_cell {
        let mut style = Style::default().fg(theme.error);
        if !geometry.error_lift {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_string(
            cell.0,
            cell.1,
            glyph_for(cell),
            own_emphasis(with_stale(style, stale)),
        );
    }

    for bracket in &geometry.brackets {
        buf.set_string(
            bracket.cell.0,
            bracket.cell.1,
            bracket.glyph,
            own_emphasis(with_stale(
                quiet_dim(
                    Style::default()
                        .fg(theme.selection_bg)
                        .add_modifier(Modifier::BOLD),
                ),
                stale,
            )),
        );
    }
}

/// Render only explicit Herdr names directly beneath their own planet disc.
/// Candidates stay within the planet tile and stage field. A candidate that
/// would overwrite a sun, any planet body/focus bracket, or an earlier label
/// is omitted; that never removes or changes a planet body.
pub(super) fn render_planet_labels(
    buf: &mut Buffer,
    layout: &CollageLayout,
    geometries: &[PlanetGeometry],
    agents: &[AgentView],
    theme: &Theme,
    stale: bool,
    selected_index: Option<usize>,
) {
    let mut displayed = Vec::new();
    for (tile, geometry) in layout.tiles.iter().zip(geometries) {
        let Some(candidate) = planet_label_candidate(
            tile,
            geometry,
            agents
                .get(tile.index)
                .and_then(|view| view.details.name.as_deref()),
        ) else {
            continue;
        };
        let collides_sun = layout
            .sun
            .is_some_and(|(x, y)| rects_overlap(candidate.rect, Rect::new(x, y, 1, 1)));
        let collides_planet = geometries.iter().any(|geometry| {
            geometry
                .body
                .iter()
                .chain(geometry.brackets.iter().map(|bracket| &bracket.cell))
                .any(|&(x, y)| rects_overlap(candidate.rect, Rect::new(x, y, 1, 1)))
        });
        if collides_sun
            || collides_planet
            || displayed
                .iter()
                .any(|&label| rects_overlap(candidate.rect, label))
        {
            continue;
        }

        let mut style = Style::default().fg(if Some(tile.index) == selected_index {
            theme.accent
        } else {
            theme.muted
        });
        if stale {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_stringn(
            candidate.rect.x,
            candidate.rect.y,
            candidate.text.as_str(),
            candidate.rect.width as usize,
            own_emphasis(style),
        );
        displayed.push(candidate.rect);
    }
}

/// Planet cells fully own their emphasis: painting over a dim scope cell
/// (vignette, phosphor persistence) must not inherit its modifiers, because
/// Ratatui merges styles per cell. Subtract exactly the emphasis modifiers
/// the composed style did not add itself.
pub(super) fn own_emphasis(style: Style) -> Style {
    style.remove_modifier((Modifier::DIM | Modifier::BOLD).difference(style.add_modifier))
}

/// Draw the breathing theme-phosphor vignette ring for a normalized radius.
pub(super) fn render_vignette(
    buf: &mut Buffer,
    area: Rect,
    radius: f32,
    theme: &Theme,
    stale: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let half_w = area.width as f32 / 2.0;
    let half_h = area.height as f32 / 2.0;
    let cx = area.x as f32 + half_w - 0.5;
    let cy = area.y as f32 + half_h - 0.5;
    let style = with_stale(
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::DIM),
        stale,
    );
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let nx = (x as f32 - cx) / half_w;
            let ny = (y as f32 - cy) / half_h;
            let dist = (nx * nx + ny * ny).sqrt();
            if (dist - radius).abs() <= VIGNETTE_BAND {
                buf.set_string(x, y, "·", style);
            }
        }
    }
}

pub(super) fn with_stale(style: Style, stale: bool) -> Style {
    if stale {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// Centered single-line copy for the empty/unavailable states.
pub(super) fn center_copy(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height / 2;
    buf.set_stringn(x, y, text, area.width as usize, style);
}
