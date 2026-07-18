//! Agent Pulse rendering: the tiny `● n active` summary and the full-screen,
//! music-reactive Kinetic Collage canvas.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. The canvas gives every agent one stable,
//! procedurally generated album-art tile whose motif, palette arrangement, and
//! staggered base rectangle derive only from the private agent identity hash.
//! The actual played-sample [`crate::model::VizFrame`] drives everything that
//! moves: RMS plus each tile's assigned FFT band scale and offset the tile and
//! grow up to two soft shadow trails from the real prior frames in
//! `App::viz_history()`; behind the tiles a low-contrast waveform/FFT trace
//! and a breathing theme-phosphor vignette follow the same frame — never a
//! timer. Silence leaves a dim, still collage; `--low-power` freezes tile
//! geometry, trails, trace, and vignette while state edge colors and minimal
//! brightness still update.
//!
//! Mouse input flows through [`hit_test`], which shares [`collage_layout`]
//! with rendering so a click resolves against exactly the tile rectangles that
//! were drawn (background, vignette, and shadow cells resolve nothing), and
//! returns only the read-only selection [`Action`]; the CLI event loop owns
//! applying it.
//!
//! Privacy: a selected tile may show the explicit Herdr agent `name` only. No
//! pane id, workspace id, cwd, or agent type is ever rendered. All colors come
//! from the active [`Theme`]; no palette values are added.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Widget},
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::app::{Action, AgentPulseConnection, AgentView, App};
use crate::herdr::AgentStatus;
use crate::model::VizFrame;
use crate::theme::Theme;

/// Below this energy/magnitude the collage counts as silent: dim and still.
const SILENCE_ENERGY: f32 = 0.05;
/// Above this energy a working tile's edge glow brightens to bold.
const BRIGHT_ENERGY: f32 = 0.6;
/// A tile grows a shadow layer from a prior frame above this energy.
const SHADOW_ENERGY: f32 = 0.1;
/// Maximum soft shadow trail layers taken from recent history frames.
const SHADOW_LAYERS: usize = 2;
/// Upper bound on a tile's base width so sparse collages stay tile-like.
const TILE_MAX_W: u16 = 18;
/// Upper bound on a tile's base height so sparse collages stay tile-like.
const TILE_MAX_H: u16 = 9;
/// Spatial undulation cycles of the FFT trace across the canvas width; a pure
/// function of column position, so the trace meanders without any clock input.
const TRACE_WAVES: f32 = 1.5;
/// Normalized breathing vignette ring radius at silence.
const VIGNETTE_BASE: f32 = 0.62;
/// How far full RMS pushes the vignette ring outward.
const VIGNETTE_SWING: f32 = 0.3;
/// Half-thickness of the vignette ring in normalized distance.
const VIGNETTE_BAND: f32 = 0.05;

// --- quiet normal-layout summary -------------------------------------------

/// The one-line Now Playing summary for Wide/Medium tiers: only `● n active`.
///
/// `None` when hidden (standalone/ineligible) or unavailable, so those states
/// reserve no row and standalone output stays byte-identical. Stale dims the
/// last known count; the canvas owns every richer state description.
pub(super) fn summary_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    let count = app.active_agents().len();
    let text = format!("● {count} active");
    match app.agent_pulse_connection() {
        AgentPulseConnection::Hidden | AgentPulseConnection::Unavailable => None,
        AgentPulseConnection::Connected => {
            let color = if count > 0 {
                theme.playing
            } else {
                theme.muted
            };
            Some(Line::from(Span::styled(text, Style::default().fg(color))))
        }
        AgentPulseConnection::Stale => Some(Line::from(Span::styled(
            text,
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ))),
    }
}

// --- status presentation helpers -------------------------------------------

/// Short lowercase state label for the selected-tile line.
fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// Theme color per status: working is strongest, blocked demands attention,
/// idle/done/unknown stay muted.
fn status_color(status: AgentStatus, theme: &Theme) -> Color {
    match status {
        AgentStatus::Working => theme.playing,
        AgentStatus::Blocked => theme.error,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => theme.muted,
    }
}

// --- pure collage geometry --------------------------------------------------

/// Abstract album-art motif families. Which one an agent gets — and how its
/// palette is arranged — comes only from the stable identity hash, so the art
/// never morphs or swaps with audio or status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlbumMotif {
    /// Concentric record-like rings around a center hole.
    Record,
    /// One asymmetric diagonal band across the tile.
    Diagonal,
    /// Alternating vertical stripes.
    Stripe,
    /// A frame-within-a-frame inset border.
    Frame,
}

impl AlbumMotif {
    const ALL: [AlbumMotif; 4] = [
        AlbumMotif::Record,
        AlbumMotif::Diagonal,
        AlbumMotif::Stripe,
        AlbumMotif::Frame,
    ];
}

/// One placed agent tile: the index into `App::active_agents()`, its stable
/// identity seed, motif, and staggered base rectangle, the audio-transformed
/// drawn rectangle, its energy, and up to two soft shadow trail rectangles
/// derived from real prior frames.
struct CollageTile {
    index: usize,
    seed: u64,
    motif: AlbumMotif,
    base_rect: Rect,
    rect: Rect,
    energy: f32,
    shadows: Vec<Rect>,
}

/// One column of the background waveform/FFT trace.
struct TraceCell {
    x: u16,
    y: u16,
    magnitude: f32,
    position: f32,
}

/// The music background behind the tiles: the trace polyline and the
/// normalized breathing vignette ring radius.
struct CollageBackground {
    trace: Vec<TraceCell>,
    vignette: f32,
}

/// Pure Kinetic Collage geometry shared by rendering and hit testing.
struct CollageLayout {
    background: CollageBackground,
    tiles: Vec<CollageTile>,
}

/// Stable placement seed for an agent: a hash of its private identity, so a
/// status change never moves a tile and no pane detail is exposed.
fn seed_of(view: &AgentView) -> u64 {
    let mut hasher = DefaultHasher::new();
    view.id.hash(&mut hasher);
    hasher.finish()
}

/// The agent's assigned FFT band, by identity; zero when the frame is empty.
fn band_of(seed: u64, bands: &[f32]) -> f32 {
    if bands.is_empty() {
        0.0
    } else {
        bands[seed as usize % bands.len()]
    }
}

/// Deterministic -1/+1 motion direction from one identity bit.
fn tile_dir(seed: u64, bit: u32) -> i32 {
    if (seed >> bit) & 1 == 0 {
        1
    } else {
        -1
    }
}

/// Clamp a proposed rectangle into `area`, keeping at least one cell.
fn clamp_rect(x: i32, y: i32, width: u16, height: u16, area: Rect) -> Rect {
    let width = width.clamp(1, area.width);
    let height = height.clamp(1, area.height);
    let x = x.clamp(area.x as i32, (area.x + area.width - width) as i32) as u16;
    let y = y.clamp(area.y as i32, (area.y + area.height - height) as i32) as u16;
    Rect::new(x, y, width, height)
}

/// Trace y-coordinate for a signed displacement in `-1.0..=1.0`. Zero is
/// exactly the vertical middle, so silence and low power are flat by
/// construction.
fn trace_y(area: Rect, displacement: f32) -> u16 {
    let cy = area.y as f32 + area.height.saturating_sub(1) as f32 / 2.0;
    let amp = area.height.saturating_sub(1) as f32 / 2.0 * 0.9;
    let y = (cy - displacement * amp).round() as i32;
    y.clamp(
        area.y as i32,
        (area.y + area.height).saturating_sub(1) as i32,
    ) as u16
}

/// One background trace column per cell of `area`'s width.
///
/// Uses the actual time-domain waveform when the frame carries one, otherwise
/// the FFT bands riding a fixed spatial undulation. `low_power` freezes the
/// trace to a flat, dim baseline.
fn trace_cells(frame: &VizFrame, area: Rect, low_power: bool) -> Vec<TraceCell> {
    let width = area.width as usize;
    let columns: Vec<(f32, f32, f32)> = if !frame.waveform.is_empty() {
        super::visualizer::waveform_columns(&frame.waveform, width)
            .into_iter()
            .map(|(sample, position)| (sample, sample.abs(), position))
            .collect()
    } else {
        let spectrum = super::visualizer::spectrum_columns(&frame.bands, width);
        if spectrum.is_empty() {
            let last = width.saturating_sub(1).max(1) as f32;
            (0..width)
                .map(|col| (0.0, 0.0, col as f32 / last))
                .collect()
        } else {
            spectrum
                .into_iter()
                .map(|(magnitude, position)| {
                    let ripple = (position * std::f32::consts::TAU * TRACE_WAVES).sin();
                    (magnitude * ripple, magnitude, position)
                })
                .collect()
        }
    };
    columns
        .into_iter()
        .enumerate()
        .map(|(col, (displacement, magnitude, position))| {
            let (displacement, magnitude) = if low_power {
                (0.0, 0.0)
            } else {
                (displacement, magnitude)
            };
            TraceCell {
                x: area.x + col as u16,
                y: trace_y(area, displacement),
                magnitude,
                position,
            }
        })
        .collect()
}

/// Compute the full Collage geometry for `agents` inside `area`.
///
/// Deterministic and clock-free: each agent's motif and staggered base
/// rectangle come only from its identity hash and the collage grid (dense
/// terminals shrink tile size and spacing rather than omitting tiles), the
/// audio transform comes only from the frame's RMS and the tile's assigned
/// band, and shadow trails come only from `history` (most recent first).
/// `low_power` keeps every tile on its base rectangle with no shadows and a
/// frozen background; callers keep using the frame for color and brightness
/// only.
fn collage_layout(
    agents: &[AgentView],
    frame: &VizFrame,
    history: &[VizFrame],
    area: Rect,
    low_power: bool,
) -> CollageLayout {
    if area.width == 0 || area.height == 0 {
        return CollageLayout {
            background: CollageBackground {
                trace: Vec::new(),
                vignette: VIGNETTE_BASE,
            },
            tiles: Vec::new(),
        };
    }

    let vignette = if low_power {
        VIGNETTE_BASE
    } else {
        VIGNETTE_BASE + frame.rms.clamp(0.0, 1.0) * VIGNETTE_SWING
    };
    let background = CollageBackground {
        trace: trace_cells(frame, area, low_power),
        vignette,
    };

    // Stable, status-independent slot order across the collage grid.
    let mut order: Vec<(u64, usize)> = (0..agents.len())
        .map(|index| (seed_of(&agents[index]), index))
        .collect();
    order.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| agents[a.1].id.cmp(&agents[b.1].id))
    });

    let n = order.len();
    if n == 0 {
        return CollageLayout {
            background,
            tiles: Vec::new(),
        };
    }

    // Grid shape targeting roughly square-looking (2:1 cell aspect) tiles;
    // dense counts shrink cells before anything else. Overlap only appears
    // when agents outnumber cells entirely.
    let w = area.width as usize;
    let h = area.height as usize;
    let mut rows = ((n as f32 * 2.0 * h as f32 / w as f32).sqrt().ceil() as usize)
        .clamp(1, h)
        .min(n);
    let mut cols = n.div_ceil(rows);
    if cols > w {
        cols = w;
        rows = n.div_ceil(cols).clamp(1, h);
    }
    let cell_x = |col: usize| area.x as usize + col * w / cols;
    let cell_y = |row: usize| area.y as usize + row * h / rows;

    let tiles = order
        .into_iter()
        .enumerate()
        .map(|(slot, (seed, index))| {
            let motif = AlbumMotif::ALL[seed as usize % AlbumMotif::ALL.len()];
            let row = (slot / cols).min(rows - 1);
            let col = slot % cols;
            let cw = (cell_x(col + 1) - cell_x(col)).max(1);
            let ch = (cell_y(row + 1) - cell_y(row)).max(1);
            let tile_w = ((cw * 2 / 3).max(1) as u16).min(TILE_MAX_W);
            let tile_h = ((ch * 2 / 3).max(1) as u16).min(TILE_MAX_H);
            // Staggered collage placement: odd rows shift like brickwork and a
            // tiny identity jitter keeps the grid asymmetric, all clamped so
            // no tile ever leaves the canvas.
            let brick = if row % 2 == 1 { (cw / 3) as i32 } else { 0 };
            let jitter_x = (seed % 3) as i32 - 1;
            let jitter_y = ((seed >> 3) % 3) as i32 - 1;
            let base_rect = clamp_rect(
                cell_x(col) as i32 + (cw as i32 - tile_w as i32) / 2 + brick + jitter_x,
                cell_y(row) as i32 + (ch as i32 - tile_h as i32) / 2 + jitter_y,
                tile_w,
                tile_h,
                area,
            );

            let band = band_of(seed, &frame.bands);
            let energy = (frame.rms * 0.55 + band * 0.45).clamp(0.0, 1.0);

            // Audio motion: bounded scale and offset only; the base rectangle
            // (and so the art) never changes. Silence and low power are the
            // base geometry by construction.
            let rect = if low_power || energy <= SILENCE_ENERGY {
                base_rect
            } else {
                let grow_w = (energy * 2.0).round() as i32;
                let grow_h = energy.round() as i32;
                let dx = tile_dir(seed, 0) * (energy * 1.4).round() as i32;
                let dy = tile_dir(seed, 1) * (energy * 0.9).round() as i32;
                clamp_rect(
                    base_rect.x as i32 - grow_w / 2 + dx,
                    base_rect.y as i32 - grow_h / 2 + dy,
                    base_rect.width + grow_w as u16,
                    base_rect.height + grow_h as u16,
                    area,
                )
            };

            let shadows = if low_power {
                Vec::new()
            } else {
                history
                    .iter()
                    .take(SHADOW_LAYERS)
                    .enumerate()
                    .filter_map(|(age, old)| {
                        let old_energy =
                            (old.rms * 0.55 + band_of(seed, &old.bands) * 0.45).clamp(0.0, 1.0);
                        if old_energy <= SHADOW_ENERGY {
                            return None;
                        }
                        let step = age as i32 + 1;
                        Some(clamp_rect(
                            base_rect.x as i32 - tile_dir(seed, 0) * step * 2,
                            base_rect.y as i32 + step,
                            base_rect.width,
                            base_rect.height,
                            area,
                        ))
                    })
                    .collect()
            };

            CollageTile {
                index,
                seed,
                motif,
                base_rect,
                rect,
                energy,
                shadows,
            }
        })
        .collect();

    CollageLayout { background, tiles }
}

// --- canvas geometry --------------------------------------------------------

/// Whether the full-screen canvas exists at all: overlay open, integration
/// visible, and Signal View (which keeps its own input/display contract)
/// inactive.
fn canvas_active(app: &App) -> bool {
    app.is_agent_overlay_open()
        && !app.is_signal_view()
        && app.agent_pulse_connection() != AgentPulseConnection::Hidden
}

/// The collage region inside the canvas: below the title/banner rows and above
/// the label/footer rows.
fn collage_area(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(4),
    )
}

/// Whether (`x`, `y`) falls inside `rect`.
fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

// --- hit testing ------------------------------------------------------------

/// Pure mouse hit test for the Kinetic Collage canvas.
///
/// Maps a click inside a tile's drawn rectangle to the read-only
/// [`Action::SelectAgent`]; returns `None` whenever the canvas is closed, the
/// integration is hidden, the connection is stale or unavailable, Signal View
/// is active, or the click misses every tile. Background trace, vignette, and
/// shadow cells resolve nothing. Overlapping tiles resolve topmost-first, with
/// the selected tile in front, matching draw order.
pub(super) fn hit_test(area: Rect, column: u16, row: u16, app: &App) -> Option<Action> {
    if app.agent_pulse_connection() != AgentPulseConnection::Connected {
        return None;
    }
    if !canvas_active(app) || area.width < 8 || area.height < 5 {
        return None;
    }
    let agents = app.active_agents();
    if agents.is_empty() || !rect_contains(area, column, row) {
        return None;
    }
    let layout = collage_layout(agents, app.viz(), &[], collage_area(area), false);
    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));
    let topmost = layout
        .tiles
        .iter()
        .filter(|tile| Some(tile.index) == selected_index)
        .chain(
            layout
                .tiles
                .iter()
                .rev()
                .filter(|tile| Some(tile.index) != selected_index),
        );
    for tile in topmost {
        if rect_contains(tile.rect, column, row) {
            let view = agents.get(tile.index)?;
            return Some(Action::SelectAgent(view.id.clone()));
        }
    }
    None
}

// --- canvas rendering -------------------------------------------------------

/// Render the full-screen Kinetic Collage over the composed layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the title/count, the breathing
/// vignette and waveform/FFT trace, each tile's shadow trails and album-art
/// motif with its state edge glow, the selected explicit-name label, and a
/// restrained footer hint. Stale renders the frozen last live collage dimmed
/// under a `reconnecting` banner; Unavailable hides every tile behind calm
/// copy. `now` is injected by the render entry point but deliberately unused:
/// motion derives from audio frames only.
pub(super) fn render_canvas(
    app: &App,
    theme: &Theme,
    low_power: bool,
    _now: Instant,
    area: Rect,
    buf: &mut Buffer,
) {
    if !canvas_active(app) || area.width < 8 || area.height < 5 {
        return;
    }
    Clear.render(area, buf);
    buf.set_style(area, theme.base_style());

    let connection = app.agent_pulse_connection();
    let agents = app.active_agents();
    let stale = connection == AgentPulseConnection::Stale;
    let muted = Style::default().fg(theme.muted);
    let dim_muted = muted.add_modifier(Modifier::DIM);

    // Title row: name plus the same tiny count language as the normal line.
    let mut title = theme.accent_style();
    if stale {
        title = title.add_modifier(Modifier::DIM);
    }
    set_row(buf, area, area.y, 1, "Agent Pulse", title);
    let count = format!(" · {} active", agents.len());
    set_row(
        buf,
        area,
        area.y,
        1 + "Agent Pulse".len() as u16,
        &count,
        if stale { dim_muted } else { muted },
    );

    if connection == AgentPulseConnection::Unavailable {
        center_copy(buf, area, "agents · unavailable · retrying", muted);
        footer(buf, area, muted);
        return;
    }

    if stale {
        set_row(buf, area, area.y + 1, 1, "stale · reconnecting", dim_muted);
    }

    let canvas = collage_area(area);
    // Stale renders the display captured by the reducer at the
    // Connected→Stale edge, freezing the exact last live collage and trails
    // (then dimmed below); live renders use the current frame plus the real
    // prior frames behind it. Only `--low-power` freezes geometry further.
    let (frame, history): (&VizFrame, Vec<VizFrame>) = match app.stale_viz().filter(|_| stale) {
        Some((frame, history)) => (frame, history.to_vec()),
        None => (app.viz(), app.viz_history().skip(1).cloned().collect()),
    };
    let layout = collage_layout(agents, frame, &history, canvas, low_power);

    render_vignette(buf, canvas, layout.background.vignette, theme, stale);
    for cell in &layout.background.trace {
        let style = Style::default()
            .fg(theme.spectrum_color(cell.position))
            .add_modifier(Modifier::DIM);
        buf.set_string(
            cell.x,
            cell.y,
            trace_glyph(cell.magnitude),
            with_stale(style, stale),
        );
    }

    if agents.is_empty() {
        center_copy(buf, area, "agents · none active", muted);
        footer(buf, area, muted);
        return;
    }

    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));

    // Soft shadow trails sit behind every tile.
    for tile in &layout.tiles {
        for shadow in &tile.shadows {
            for y in shadow.y..shadow.y + shadow.height {
                for x in shadow.x..shadow.x + shadow.width {
                    buf.set_string(x, y, "∙", with_stale(dim_muted, stale));
                }
            }
        }
    }

    // Tiles draw in stable slot order; the selected tile comes forward last.
    for tile in &layout.tiles {
        if Some(tile.index) == selected_index {
            continue;
        }
        render_tile(buf, tile, agents, theme, false, stale, low_power);
    }
    if let Some(selected) = selected_index {
        if let Some(tile) = layout.tiles.iter().find(|tile| tile.index == selected) {
            render_tile(buf, tile, agents, theme, true, stale, low_power);
        }
    }

    // Selected label: the explicit Herdr name only. An unnamed selection
    // shows no label at all — never a pane id, cwd, or agent-type fallback.
    if let Some(view) = app.selected_agent() {
        if let Some(name) = &view.name {
            let label = format!("{name} · {}", status_label(view.status));
            let style = Style::default().fg(theme.foreground);
            set_row(
                buf,
                area,
                area.y + area.height - 2,
                1,
                &label,
                with_stale(style, stale),
            );
        }
    }

    footer(buf, area, muted);
}

/// Draw one album-art tile: its interior motif in identity-arranged theme
/// colors and its state edge glow around the border.
fn render_tile(
    buf: &mut Buffer,
    tile: &CollageTile,
    agents: &[AgentView],
    theme: &Theme,
    selected: bool,
    stale: bool,
    low_power: bool,
) {
    let Some(view) = agents.get(tile.index) else {
        return;
    };
    let edge = if selected {
        theme.selection_style()
    } else {
        edge_style(view.status, theme, tile.energy, low_power)
    };
    let edge = with_stale(edge, stale);
    let (first, second) = motif_palette(tile.seed, theme);
    let rect = tile.rect;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let on_edge = x == rect.x
                || x == rect.x + rect.width - 1
                || y == rect.y
                || y == rect.y + rect.height - 1;
            if on_edge {
                buf.set_string(x, y, "▒", edge);
                continue;
            }
            // Sample the art at its stable base scale: audio growth reveals
            // the pattern's edge instead of stretching the composition.
            let rx = (x - rect.x).min(tile.base_rect.width - 1);
            let ry = (y - rect.y).min(tile.base_rect.height - 1);
            let (glyph, leading) = motif_cell(tile.motif, tile.seed, rx, ry, tile.base_rect);
            let mut style = Style::default().fg(if leading { first } else { second });
            if tile.energy <= SILENCE_ENERGY {
                style = style.add_modifier(Modifier::DIM);
            }
            buf.set_string(x, y, glyph, with_stale(style, stale));
        }
    }
}

/// Two theme colors arranged by identity for a tile's interior art. Muted and
/// the state colors are deliberately excluded so the edge glow stays the only
/// state signal.
fn motif_palette(seed: u64, theme: &Theme) -> (Color, Color) {
    let pool = [
        theme.accent,
        theme.spectrum_low,
        theme.spectrum_mid,
        theme.spectrum_high,
        theme.foreground,
    ];
    (
        pool[(seed >> 8) as usize % pool.len()],
        pool[(seed >> 16) as usize % pool.len()],
    )
}

/// Interior motif glyph for a tile-relative cell, plus which palette color it
/// takes. Pure in (motif, seed, position, rect size): audio never reaches it.
fn motif_cell(motif: AlbumMotif, seed: u64, rx: u16, ry: u16, rect: Rect) -> (&'static str, bool) {
    let w = rect.width.max(1) as f32;
    let h = rect.height.max(1) as f32;
    match motif {
        AlbumMotif::Record => {
            let cx = (w - 1.0) / 2.0;
            let cy = (h - 1.0) / 2.0;
            let nx = (rx as f32 - cx) / cx.max(0.5);
            let ny = (ry as f32 - cy) / cy.max(0.5);
            let dist = (nx * nx + ny * ny).sqrt();
            if dist < 0.25 {
                ("◌", true)
            } else if ((dist * 3.0) as u32).is_multiple_of(2) {
                ("▒", false)
            } else {
                ("░", true)
            }
        }
        AlbumMotif::Diagonal => {
            let falling = (seed >> 5) & 1 == 0;
            let px = rx as f32 / (w - 1.0).max(1.0);
            let py = ry as f32 / (h - 1.0).max(1.0);
            let along = if falling { px - py } else { px + py - 1.0 };
            if along.abs() < 0.25 {
                (if falling { "╲" } else { "╱" }, true)
            } else {
                ("░", false)
            }
        }
        AlbumMotif::Stripe => {
            let step = (rect.width / 4).max(1);
            if (rx / step).is_multiple_of(2) {
                ("▒", true)
            } else {
                ("░", false)
            }
        }
        AlbumMotif::Frame => {
            let ring = rx
                .min(rect.width - 1 - rx)
                .min(ry)
                .min(rect.height - 1 - ry);
            if ring == 1 {
                ("▒", true)
            } else {
                ("░", false)
            }
        }
    }
}

/// Draw the breathing theme-phosphor vignette ring for a normalized radius.
fn render_vignette(buf: &mut Buffer, area: Rect, radius: f32, theme: &Theme, stale: bool) {
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

/// Trace glyph weight by magnitude: heavier water for louder audio.
fn trace_glyph(magnitude: f32) -> &'static str {
    if magnitude < SILENCE_ENERGY {
        "·"
    } else if magnitude < 0.35 {
        "~"
    } else if magnitude < 0.7 {
        "≈"
    } else {
        "≋"
    }
}

/// Edge glow style: theme status color, silence dims, strong signal emboldens
/// working tiles (never in low power), done is always faded.
fn edge_style(status: AgentStatus, theme: &Theme, energy: f32, low_power: bool) -> Style {
    let mut style = Style::default().fg(status_color(status, theme));
    if status == AgentStatus::Done {
        style = style.add_modifier(Modifier::DIM);
    }
    if energy <= SILENCE_ENERGY {
        style = style.add_modifier(Modifier::DIM);
    } else if status == AgentStatus::Working && energy > BRIGHT_ENERGY && !low_power {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn with_stale(style: Style, stale: bool) -> Style {
    if stale {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// Write `text` on `row` starting `indent` cells in, clipped to the area.
fn set_row(buf: &mut Buffer, area: Rect, row: u16, indent: u16, text: &str, style: Style) {
    if row >= area.y + area.height || indent >= area.width {
        return;
    }
    let x = area.x + indent;
    buf.set_stringn(x, row, text, (area.width - indent) as usize, style);
}

/// Centered single-line copy for the empty/unavailable states.
fn center_copy(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height / 2;
    buf.set_stringn(x, y, text, area.width as usize, style);
}

/// Restrained footer hint on the canvas' last row.
fn footer(buf: &mut Buffer, area: Rect, style: Style) {
    set_row(
        buf,
        area,
        area.y + area.height - 1,
        1,
        "Tab/↑↓/click select · a/Esc close",
        style,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::audio::AudioEvent;
    use crate::catalog::Catalog;
    use crate::herdr::{AgentId, AgentSnapshot};
    use crate::settings::Settings;
    use crate::theme::ThemeName;
    use std::time::Duration;

    const CANVAS: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };

    /// Glyphs only tile art (interior or edge) may use.
    const TILE_GLYPHS: [&str; 5] = ["░", "▒", "╱", "╲", "◌"];

    fn view(workspace: &str, pane: &str, status: AgentStatus) -> AgentView {
        AgentView {
            id: AgentId::new(workspace, pane),
            name: None,
            status,
            observed_at: Instant::now(),
        }
    }

    fn agents(count: usize) -> Vec<AgentView> {
        (0..count)
            .map(|i| view("ws", &format!("p{i}"), AgentStatus::Working))
            .collect()
    }

    fn frame(rms: f32, bands: Vec<f32>) -> VizFrame {
        VizFrame::new(bands, rms, Vec::<f32>::new())
    }

    fn snap(workspace: &str, pane: &str, name: Option<&str>, status: AgentStatus) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new(workspace, pane),
            name: name.map(str::to_string),
            status,
        }
    }

    /// A connected app with the canvas open.
    fn collage_app(agents: Vec<AgentSnapshot>) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents,
            now: Instant::now(),
        });
        app.apply(Action::ToggleAgentOverlay);
        app
    }

    /// One named and one unnamed agent whose raw ids would be recognizable if
    /// they ever leaked into the buffer.
    fn app_with_named_and_unnamed_agents() -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents: vec![
                snap(
                    "workspace-1",
                    "pane-1",
                    Some("research"),
                    AgentStatus::Working,
                ),
                snap("workspace-1", "claude", None, AgentStatus::Working),
            ],
            now: Instant::now(),
        });
        app
    }

    fn push_frame(app: &mut App, frame: VizFrame) {
        app.apply(Action::Audio(AudioEvent::Viz(frame)));
    }

    fn render_collage_for(app: &App, low_power: bool, now: Instant) -> Buffer {
        let mut buf = Buffer::empty(CANVAS);
        let theme = Theme::for_name(ThemeName::Minimal);
        render_canvas(app, &theme, low_power, now, CANVAS, &mut buf);
        buf
    }

    /// Render an open canvas of `count` working agents after feeding the prior
    /// `history` frames (oldest first) and then the current `frame`.
    fn render_collage(
        count: usize,
        frame: VizFrame,
        history: Vec<VizFrame>,
        low_power: bool,
    ) -> Buffer {
        let snaps = (0..count)
            .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
            .collect();
        let mut app = collage_app(snaps);
        for old in history {
            push_frame(&mut app, old);
        }
        push_frame(&mut app, frame);
        render_collage_for(&app, low_power, Instant::now())
    }

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

    /// Shadow cells use a glyph no background or tile cell shares.
    fn count_shadow_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('∙').count()
    }

    /// Cells drawn with tile-art glyphs.
    fn count_tile_cells(buf: &Buffer) -> usize {
        let text = buffer_text(buf);
        TILE_GLYPHS
            .iter()
            .map(|glyph| text.matches(glyph).count())
            .sum()
    }

    /// Every non-blank cell of the collage region — vignette, trace, shadows,
    /// and tiles — as `(x, y, symbol)`, so tests can compare whole-field
    /// geometry between renders.
    fn field_cells(buf: &Buffer) -> Vec<(u16, u16, String)> {
        let canvas = collage_area(*buf.area());
        let mut cells = Vec::new();
        for y in canvas.y..canvas.y + canvas.height {
            for x in canvas.x..canvas.x + canvas.width {
                let symbol = buf.cell((x, y)).unwrap().symbol();
                if symbol != " " {
                    cells.push((x, y, symbol.to_string()));
                }
            }
        }
        cells
    }

    // --- pure collage layout ----------------------------------------------

    #[test]
    fn tile_motif_and_staggered_rect_stay_stable_for_an_agent_identity() {
        let area = Rect::new(0, 0, 120, 36);
        let agent = view("alpha", "p1", AgentStatus::Working);
        let first = collage_layout(
            std::slice::from_ref(&agent),
            &frame(0.0, vec![0.0; 16]),
            &[],
            area,
            false,
        );
        let later = collage_layout(
            &[agent],
            &frame(0.9, vec![0.9; 16]),
            &[frame(0.1, vec![0.1; 16])],
            area,
            false,
        );
        assert_eq!(first.tiles[0].motif, later.tiles[0].motif);
        assert_eq!(first.tiles[0].base_rect, later.tiles[0].base_rect);
    }

    #[test]
    fn dense_collage_keeps_one_tile_per_agent() {
        let area = Rect::new(0, 0, 50, 15);
        let layout = collage_layout(&agents(80), &frame(0.5, vec![0.5; 16]), &[], area, false);
        assert_eq!(layout.tiles.len(), 80);
        for tile in &layout.tiles {
            assert!(tile.rect.width >= 1 && tile.rect.height >= 1);
            assert!(
                tile.rect.x + tile.rect.width <= area.width
                    && tile.rect.y + tile.rect.height <= area.height,
                "tile {:?} escaped the {area:?}",
                tile.rect
            );
        }
    }

    #[test]
    fn tile_art_and_rect_are_stable_when_status_changes() {
        let before = collage_layout(
            &[view("alpha", "p1", AgentStatus::Working)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
            false,
        );
        let after = collage_layout(
            &[view("alpha", "p1", AgentStatus::Blocked)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
            false,
        );
        assert_eq!(before.tiles[0].motif, after.tiles[0].motif);
        assert_eq!(before.tiles[0].base_rect, after.tiles[0].base_rect);
        assert_eq!(before.tiles[0].rect, after.tiles[0].rect);
    }

    #[test]
    fn tiles_differ_for_identical_panes_in_different_workspaces() {
        let layout = collage_layout(
            &[
                view("alpha", "p1", AgentStatus::Working),
                view("beta", "p1", AgentStatus::Working),
            ],
            &frame(0.0, vec![0.0; 16]),
            &[],
            CANVAS,
            false,
        );
        assert_eq!(layout.tiles.len(), 2);
        assert_ne!(layout.tiles[0].base_rect, layout.tiles[1].base_rect);
    }

    #[test]
    fn low_power_layout_is_static_with_no_shadows() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = collage_layout(
            &agents(4),
            &frame(0.9, vec![0.9; 16]),
            &[frame(0.5, vec![0.5; 16])],
            area,
            true,
        );
        assert_eq!(layout.background.vignette, VIGNETTE_BASE);
        let baseline = trace_y(area, 0.0);
        for cell in &layout.background.trace {
            assert_eq!(cell.y, baseline, "low-power trace is flat");
        }
        for tile in &layout.tiles {
            assert_eq!(tile.rect, tile.base_rect, "low-power tiles sit still");
            assert!(tile.shadows.is_empty(), "low power grows no shadows");
        }
    }

    // --- music reactivity -------------------------------------------------

    #[test]
    fn rms_and_fft_expand_tiles_and_add_soft_shadow_trails() {
        let quiet = render_collage(4, frame(0.05, vec![0.05; 16]), vec![], false);
        let loud = render_collage(
            4,
            frame(0.9, vec![0.9; 16]),
            vec![frame(0.4, vec![0.4; 16])],
            false,
        );
        assert_ne!(quiet, loud);
        assert!(count_shadow_cells(&loud) > count_shadow_cells(&quiet));
    }

    #[test]
    fn rms_and_bands_move_the_collage_not_elapsed_time() {
        let t0 = Instant::now();
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.05, vec![0.0; 16]));
        let quiet = render_collage_for(&app, false, t0);
        let quiet_again = render_collage_for(&app, false, t0 + Duration::from_secs(9));
        assert_eq!(quiet, quiet_again, "time alone never animates the collage");

        push_frame(&mut app, frame(0.90, vec![0.8; 16]));
        let loud = render_collage_for(&app, false, t0);
        assert_ne!(quiet, loud, "audio frames must drive the collage");
        assert_ne!(
            field_cells(&quiet),
            field_cells(&loud),
            "loud frames must move background and tiles, not just restyle them"
        );
        assert!(
            buffer_text(&loud).contains('≈') || buffer_text(&loud).contains('≋'),
            "a loud frame draws a heavy background trace"
        );
    }

    #[test]
    fn silence_is_dim_and_still_across_time() {
        let t0 = Instant::now();
        let mut app = collage_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Idle),
        ]);
        push_frame(&mut app, frame(0.0, vec![0.0; 16]));
        let first = render_collage_for(&app, false, t0);
        let later = render_collage_for(&app, false, t0 + Duration::from_secs(9));
        assert_eq!(first, later, "silent collage must not animate with time");
        assert_eq!(count_shadow_cells(&first), 0, "silence leaves no shadows");
        for (x, y, _) in field_cells(&first) {
            assert!(
                first
                    .cell((x, y))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::DIM),
                "silent field cell ({x}, {y}) must be dim"
            );
        }
    }

    #[test]
    fn low_power_keeps_geometry_fixed_while_colors_remain() {
        let mut app = collage_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Blocked),
            snap("ws", "p3", Some("three"), AgentStatus::Idle),
        ]);
        let t0 = Instant::now();
        push_frame(&mut app, frame(0.05, vec![0.0; 16]));
        let quiet = render_collage_for(&app, true, t0);
        push_frame(&mut app, frame(0.90, vec![0.8; 16]));
        let loud_low = render_collage_for(&app, true, t0);
        assert!(!field_cells(&quiet).is_empty());
        assert_eq!(
            field_cells(&quiet),
            field_cells(&loud_low),
            "low power fixes tile, trace, and vignette geometry"
        );
        assert_eq!(
            count_shadow_cells(&loud_low),
            0,
            "low power draws no shadows"
        );

        // The same loud frame in normal power does move the collage.
        let loud_normal = render_collage_for(&app, false, t0);
        assert_ne!(field_cells(&loud_low), field_cells(&loud_normal));
    }

    // --- state, selection, and privacy -------------------------------------

    #[test]
    fn state_edge_glow_comes_from_the_theme() {
        let mut app = collage_app(vec![
            snap("ws", "p1", Some("w"), AgentStatus::Working),
            snap("ws", "p2", Some("b"), AgentStatus::Blocked),
            snap("ws", "p3", Some("i"), AgentStatus::Idle),
            snap("ws", "p4", Some("d"), AgentStatus::Done),
        ]);
        push_frame(&mut app, frame(0.5, vec![0.4; 16]));
        let theme = Theme::for_name(ThemeName::Minimal);
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(
            app.active_agents(),
            app.viz(),
            &[],
            collage_area(CANVAS),
            false,
        );
        for tile in &layout.tiles {
            let status = app.active_agents()[tile.index].status;
            let style = buf.cell((tile.rect.x, tile.rect.y)).unwrap().style();
            assert_eq!(
                style.fg,
                Some(status_color(status, &theme)),
                "{status:?} edge glow must take its theme color"
            );
            assert_eq!(
                style.add_modifier.contains(Modifier::DIM),
                status == AgentStatus::Done,
                "only done fades while energy is up: {status:?}"
            );
        }
    }

    #[test]
    fn selected_explicit_name_is_the_only_rendered_agent_detail() {
        let mut app = app_with_named_and_unnamed_agents();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            text.contains("research · working"),
            "selected explicit-name label missing: {text}"
        );
        assert!(!text.contains("workspace-1"), "workspace ids never render");
        assert!(!text.contains("pane-1"), "pane ids never render");
        assert!(!text.contains("claude"), "raw pane details never render");
    }

    #[test]
    fn no_label_renders_before_selection() {
        let app = collage_app(vec![snap(
            "alpha",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !text.contains("research"),
            "no label before selection: {text}"
        );
    }

    #[test]
    fn selecting_an_unnamed_agent_shows_no_label_at_all() {
        let mut app = collage_app(vec![snap("alpha", "p1", None, AgentStatus::Working)]);
        app.apply(Action::SelectNextAgent);
        assert!(app.selected_agent().is_some());
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !text.contains("· working"),
            "an unnamed selection must not reveal any fallback label: {text}"
        );
        assert!(!text.contains("p1"), "pane ids never render: {text}");
        assert!(
            !text.contains("alpha"),
            "workspace ids never render: {text}"
        );
    }

    // --- connection states --------------------------------------------------

    #[test]
    fn stale_freezes_the_last_live_collage_dimmed_and_time_invariant() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.4, vec![0.4; 16]));
        push_frame(&mut app, frame(0.9, vec![0.8; 16]));
        let live = render_collage_for(&app, false, Instant::now());
        let live_field = field_cells(&live);
        assert!(
            count_shadow_cells(&live) > 0,
            "sanity: the final live frame has shadows to freeze"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale_buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(
            field_cells(&stale_buf),
            live_field,
            "stale freezes the exact background, shadow, and tile geometry of the last live frame"
        );
        assert!(buffer_text(&stale_buf).contains("reconnecting"));
        for (x, y, _) in field_cells(&stale_buf) {
            assert!(
                stale_buf
                    .cell((x, y))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::DIM),
                "every stale field cell is dimmed"
            );
        }

        // Later live audio frames and elapsed time must not thaw the collage.
        push_frame(&mut app, frame(0.1, vec![0.05; 16]));
        push_frame(&mut app, frame(0.7, vec![0.6; 16]));
        let later = render_collage_for(&app, false, Instant::now() + Duration::from_secs(9));
        assert_eq!(
            later, stale_buf,
            "stale output is invariant across later audio frames and time"
        );
    }

    #[test]
    fn unavailable_hides_tiles_behind_calm_copy() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(count_tile_cells(&buf), 0, "no tiles render");
        assert!(buffer_text(&buf).contains("agents · unavailable · retrying"));
    }

    #[test]
    fn canvas_is_a_noop_while_closed() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::CloseAgentOverlay);
        let buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(buf, Buffer::empty(CANVAS), "closed canvas draws nothing");
    }

    // --- hit testing --------------------------------------------------------

    #[test]
    fn clicking_a_tile_cell_selects_that_agent() {
        let mut app = collage_app(vec![
            snap("alpha", "p1", Some("research"), AgentStatus::Working),
            snap("beta", "p1", Some("review"), AgentStatus::Idle),
        ]);
        let layout = collage_layout(
            app.active_agents(),
            app.viz(),
            &[],
            collage_area(CANVAS),
            false,
        );
        let review_index = app
            .active_agents()
            .iter()
            .position(|view| view.name.as_deref() == Some("review"))
            .unwrap();
        let tile = layout
            .tiles
            .iter()
            .find(|tile| tile.index == review_index)
            .unwrap();
        let x = tile.rect.x + tile.rect.width / 2;
        let y = tile.rect.y + tile.rect.height / 2;

        let action = hit_test(CANVAS, x, y, &app).expect("a tile click selects");
        app.apply(action);
        assert_eq!(
            app.selected_agent().unwrap().name.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn clicks_resolve_only_tile_cells_never_background_or_shadows() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.4, vec![0.4; 16]));
        push_frame(&mut app, frame(0.9, vec![0.8; 16]));
        let canvas = collage_area(CANVAS);
        let history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
        let layout = collage_layout(app.active_agents(), app.viz(), &history, canvas, false);
        let inside_a_tile = |x: u16, y: u16| {
            layout
                .tiles
                .iter()
                .any(|tile| rect_contains(tile.rect, x, y))
        };

        for cell in &layout.background.trace {
            if !inside_a_tile(cell.x, cell.y) {
                assert!(
                    hit_test(CANVAS, cell.x, cell.y, &app).is_none(),
                    "a background trace cell at ({}, {}) must resolve nothing",
                    cell.x,
                    cell.y
                );
            }
        }
        let mut checked_shadow = false;
        for tile in &layout.tiles {
            for shadow in &tile.shadows {
                for y in shadow.y..shadow.y + shadow.height {
                    for x in shadow.x..shadow.x + shadow.width {
                        if !inside_a_tile(x, y) {
                            checked_shadow = true;
                            assert!(
                                hit_test(CANVAS, x, y, &app).is_none(),
                                "a shadow cell at ({x}, {y}) must resolve nothing"
                            );
                        }
                    }
                }
            }
        }
        assert!(checked_shadow, "sanity: some shadow cell sat outside tiles");
    }

    #[test]
    fn clicks_resolve_nothing_when_missed_stale_or_closed() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        assert!(hit_test(CANVAS, 0, 0, &app).is_none(), "corner miss");

        let layout = collage_layout(
            app.active_agents(),
            app.viz(),
            &[],
            collage_area(CANVAS),
            false,
        );
        let tile = &layout.tiles[0];
        let (x, y) = (
            tile.rect.x + tile.rect.width / 2,
            tile.rect.y + tile.rect.height / 2,
        );
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert!(
            hit_test(CANVAS, x, y, &app).is_none(),
            "stale ignores clicks"
        );

        let mut closed = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        closed.apply(Action::CloseAgentOverlay);
        assert!(
            hit_test(CANVAS, x, y, &closed).is_none(),
            "closed canvas ignores clicks"
        );
    }

    // --- quiet summary ------------------------------------------------------

    #[test]
    fn summary_shows_only_the_active_count() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let app = collage_app(vec![
            snap("ws", "p1", Some("research"), AgentStatus::Working),
            snap("ws", "p2", Some("review"), AgentStatus::Idle),
        ]);
        let line = summary_line(&app, &theme).expect("connected summary");
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "● 2 active");
    }

    #[test]
    fn summary_is_absent_when_hidden_or_unavailable() {
        let theme = Theme::for_name(ThemeName::Minimal);
        let hidden = App::new(Settings::default(), Catalog::curated());
        assert!(summary_line(&hidden, &theme).is_none());

        let mut unavailable = collage_app(vec![snap("ws", "p1", None, AgentStatus::Working)]);
        unavailable.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert!(summary_line(&unavailable, &theme).is_none());
    }
}
