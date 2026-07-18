//! Agent Pulse rendering: the tiny `● n active` summary and the full-screen,
//! music-reactive Dual Phase Scope canvas.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. Behind the agent frames the canvas plots
//! two real played-audio phase portraits from the current
//! [`crate::model::VizFrame`] — paired samples on X/Y axes (stereo
//! left/right, or documented mono lags), never an amplitude-over-time
//! waveform — plus up to two dim phosphor-persistence layers from the real
//! prior frames in `App::viz_history()`. Every agent keeps one stable,
//! deterministically placed frame rectangle whose state-colored edge, bounded
//! audio-driven displacement, and soft shadow trails carry over from the
//! Kinetic Collage; the frame interior is a single centered status core
//! whose orientation advances only with new played-audio phase data while
//! Working, and stays stationary otherwise. Nothing moves from a timer:
//! identical frames render identical cells. A frame at or below the silence
//! threshold draws no trace or persistence at all — analyzer silence carries
//! non-empty all-zero traces that would otherwise pile a point cluster at
//! the canvas center — so silence stays calm, dim, and still. Stale renders
//! the reducer-captured final composition dimmed; `--low-power` renders the
//! App-captured first frame so trace, frame, shadow, and spinner geometry
//! stay frozen while state colors keep refreshing.
//!
//! Mouse input flows through [`hit_test`], which shares [`collage_layout`]
//! with rendering so a click resolves against exactly the frame rectangles
//! that were drawn (background, vignette, and shadow cells resolve nothing),
//! and returns only the read-only selection [`Action`]; the CLI event loop
//! owns applying it.
//!
//! Privacy: a selected frame may show the explicit Herdr agent `name` only.
//! No pane id, workspace id, cwd, or agent type is ever rendered. All colors
//! come from the active [`Theme`]; no palette values are added.

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
use crate::model::{PhaseTrace, VizFrame};
use crate::theme::Theme;

/// Below this energy/magnitude the scope counts as silent: dim and still.
/// Frames at or below this RMS draw no phase trace or persistence, because
/// analyzer silence still carries non-empty all-zero traces. Shared with the
/// App's low-power capture audibility gate.
const SILENCE_ENERGY: f32 = crate::model::SILENCE_RMS;
/// Above this energy a working frame's edge glow brightens to bold and the
/// live traces shed their dim modifier.
const BRIGHT_ENERGY: f32 = 0.6;
/// A frame grows a shadow layer from a prior frame above this energy.
const SHADOW_ENERGY: f32 = 0.1;
/// Maximum soft shadow trail layers taken from recent history frames.
const SHADOW_LAYERS: usize = 2;
/// Maximum dim phosphor-persistence layers taken from recent history frames.
const PERSISTENCE_LAYERS: usize = 2;
/// Upper bound on a frame's base width so sparse canvases stay tile-like.
const TILE_MAX_W: u16 = 18;
/// Upper bound on a frame's base height so sparse canvases stay tile-like.
const TILE_MAX_H: u16 = 9;
/// Spectrum-gradient position of the primary phase trace: the theme's main
/// visualizer color.
const PRIMARY_TRACE_POSITION: f32 = 0.85;
/// Spectrum-gradient position of the secondary trace: the complementary
/// visualizer color.
const SECONDARY_TRACE_POSITION: f32 = 0.3;
/// Glyph plotting one primary-trace phase point.
const PRIMARY_TRACE_GLYPH: &str = "•";
/// Glyph plotting one secondary-trace phase point.
const SECONDARY_TRACE_GLYPH: &str = "◦";
/// Glyph plotting one dim phosphor-persistence point.
const PERSISTENCE_GLYPH: &str = "·";
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

/// Theme color per status: working is strongest, blocked demands attention,
/// idle/done/unknown stay muted.
fn status_color(status: AgentStatus, theme: &Theme) -> Color {
    match status {
        AgentStatus::Working => theme.playing,
        AgentStatus::Blocked => theme.error,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => theme.muted,
    }
}

/// The one-cell status core for a frame interior: the Working spinner turns
/// only with new played-audio phase data (`true` marks it as spinning);
/// every other status is a stationary glyph. The core is status language,
/// never agent identity.
fn status_core(status: AgentStatus, seed: u64, frame: &VizFrame) -> (&'static str, bool) {
    match status {
        AgentStatus::Working => (spinner_glyph(seed, frame), true),
        AgentStatus::Idle => ("◌", false),
        AgentStatus::Blocked => ("×", false),
        AgentStatus::Done => ("·", false),
        AgentStatus::Unknown => ("·", false),
    }
}

/// The Working spinner frame: its orientation is a pure function of the real
/// primary-phase data plus the identity seed, so it advances only when new
/// audio arrives and a room of working agents never ticks in lockstep.
fn spinner_glyph(seed: u64, frame: &VizFrame) -> &'static str {
    const FRAMES: [&str; 4] = ["◜", "◝", "◞", "◟"];
    let phase = phase_signature(&frame.primary_phase);
    FRAMES[(phase.wrapping_add(seed) % FRAMES.len() as u64) as usize]
}

/// Deterministic quantization of the primary-phase coordinates: identical
/// frames yield identical signatures, and elapsed time never contributes.
fn phase_signature(trace: &PhaseTrace) -> u64 {
    trace.x.iter().zip(&trace.y).fold(0u64, |acc, (&x, &y)| {
        let qx = ((x + 1.0) * 127.0).round() as u64;
        let qy = ((y + 1.0) * 127.0).round() as u64;
        acc.rotate_left(7) ^ qx.wrapping_mul(0x9E37_79B9).wrapping_add(qy)
    })
}

// --- pure scope geometry ----------------------------------------------------

/// One plotted phase-portrait point in canvas cells.
struct PhaseCell {
    x: u16,
    y: u16,
}

/// One phase-trace layer behind the agent frames: its plotted cells, its
/// position on the theme's spectrum gradient, its glyph, and whether it
/// renders dimmed (persistence, or a quiet live trace).
struct PhaseLayer {
    cells: Vec<PhaseCell>,
    glyph: &'static str,
    color_position: f32,
    dim: bool,
}

/// One placed agent frame: the index into `App::active_agents()`, its stable
/// identity seed and staggered base rectangle, the audio-transformed drawn
/// rectangle, its energy, and up to two soft shadow trail rectangles derived
/// from real prior frames.
struct CollageTile {
    index: usize,
    seed: u64,
    /// The stable pre-transform rectangle. Rendering draws only the
    /// audio-transformed `rect`; this stays so tests can assert that an
    /// agent's identity placement never moves with audio or status.
    #[cfg_attr(not(test), allow(dead_code))]
    base_rect: Rect,
    rect: Rect,
    energy: f32,
    shadows: Vec<Rect>,
}

/// The music background behind the frames: the dual phase-scope layers and
/// the normalized breathing vignette ring radius.
struct CollageBackground {
    layers: Vec<PhaseLayer>,
    vignette: f32,
}

/// Pure Dual Phase Scope geometry shared by rendering and hit testing.
struct CollageLayout {
    background: CollageBackground,
    tiles: Vec<CollageTile>,
}

/// Stable placement seed for an agent: a hash of its private identity, so a
/// status change never moves a frame and no pane detail is exposed.
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

/// Map a normalized phase coordinate in `-1.0..=1.0` to a centered column:
/// zero is exactly the horizontal middle of `area`.
fn phase_x(area: Rect, value: f32) -> u16 {
    let half = area.width.saturating_sub(1) as f32 / 2.0;
    let x = (area.x as f32 + half + value * half).round() as i32;
    x.clamp(
        area.x as i32,
        (area.x + area.width).saturating_sub(1) as i32,
    ) as u16
}

/// Map a normalized phase coordinate in `-1.0..=1.0` to a centered row,
/// inverted so positive values plot upward like an oscilloscope.
fn phase_y(area: Rect, value: f32) -> u16 {
    let half = area.height.saturating_sub(1) as f32 / 2.0;
    let y = (area.y as f32 + half - value * half).round() as i32;
    y.clamp(
        area.y as i32,
        (area.y + area.height).saturating_sub(1) as i32,
    ) as u16
}

/// Plot a normalized paired-coordinate trace onto the centered canvas. The
/// pairs come straight from played audio; no column scan or wall-clock input
/// exists here, so the result can never scroll.
fn phase_cells(trace: &PhaseTrace, area: Rect) -> Vec<PhaseCell> {
    trace
        .x
        .iter()
        .zip(&trace.y)
        .map(|(&x, &y)| PhaseCell {
            x: phase_x(area, x),
            y: phase_y(area, y),
        })
        .collect()
}

/// The scope's phase layers in draw order: up to two dim persistence layers
/// (oldest first) from the most recent history frames, then the
/// complementary secondary trace, then the primary trace on top.
///
/// Any frame at or below [`SILENCE_ENERGY`] RMS contributes nothing at all:
/// analyzer silence still carries non-empty all-zero traces, and plotting
/// them would pile a point cluster at the canvas center. Skipping the frame
/// keeps silence calm, dim, and still by construction, with no FFT-ripple or
/// waveform substitute.
fn phase_layers(frame: &VizFrame, history: &[VizFrame], area: Rect) -> Vec<PhaseLayer> {
    let mut layers = Vec::new();
    for old in history.iter().take(PERSISTENCE_LAYERS).rev() {
        if old.rms <= SILENCE_ENERGY {
            continue;
        }
        layers.push(PhaseLayer {
            cells: phase_cells(&old.primary_phase, area),
            glyph: PERSISTENCE_GLYPH,
            color_position: PRIMARY_TRACE_POSITION,
            dim: true,
        });
    }
    if frame.rms > SILENCE_ENERGY {
        // RMS gently brightens the live traces; it never adds motion.
        let dim = frame.rms <= BRIGHT_ENERGY;
        layers.push(PhaseLayer {
            cells: phase_cells(&frame.secondary_phase, area),
            glyph: SECONDARY_TRACE_GLYPH,
            color_position: SECONDARY_TRACE_POSITION,
            dim,
        });
        layers.push(PhaseLayer {
            cells: phase_cells(&frame.primary_phase, area),
            glyph: PRIMARY_TRACE_GLYPH,
            color_position: PRIMARY_TRACE_POSITION,
            dim,
        });
    }
    layers
}

/// Compute the full scope geometry for `agents` inside `area`.
///
/// Deterministic and clock-free: each agent's staggered base rectangle comes
/// only from its identity hash and the canvas grid (dense terminals shrink
/// frame size and spacing rather than omitting frames), the audio transform
/// comes only from the frame's RMS and the frame's assigned band, and the
/// phase layers, persistence, and shadow trails come only from `frame` and
/// `history` (most recent first). Freezing — stale or low power — is done by
/// the caller handing in a captured frame/history, never by a flag here.
fn collage_layout(
    agents: &[AgentView],
    frame: &VizFrame,
    history: &[VizFrame],
    area: Rect,
) -> CollageLayout {
    if area.width == 0 || area.height == 0 {
        return CollageLayout {
            background: CollageBackground {
                layers: Vec::new(),
                vignette: VIGNETTE_BASE,
            },
            tiles: Vec::new(),
        };
    }

    let background = CollageBackground {
        layers: phase_layers(frame, history, area),
        vignette: VIGNETTE_BASE + frame.rms.clamp(0.0, 1.0) * VIGNETTE_SWING,
    };

    // Stable, status-independent slot order across the canvas grid.
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

    // Grid shape targeting roughly square-looking (2:1 cell aspect) frames;
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
            let row = (slot / cols).min(rows - 1);
            let col = slot % cols;
            let cw = (cell_x(col + 1) - cell_x(col)).max(1);
            let ch = (cell_y(row + 1) - cell_y(row)).max(1);
            let tile_w = ((cw * 2 / 3).max(1) as u16).min(TILE_MAX_W);
            let tile_h = ((ch * 2 / 3).max(1) as u16).min(TILE_MAX_H);
            // Staggered placement: odd rows shift like brickwork and a tiny
            // identity jitter keeps the grid asymmetric, all clamped so no
            // frame ever leaves the canvas.
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
            // never changes. Silence is the base geometry by construction.
            let rect = if energy <= SILENCE_ENERGY {
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

            let shadows = history
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
                .collect();

            CollageTile {
                index,
                seed,
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

/// The scope region inside the canvas: below the title/banner rows and above
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

/// Pure mouse hit test for the Dual Phase Scope canvas.
///
/// Maps a click inside a frame's drawn rectangle to the read-only
/// [`Action::SelectAgent`]; returns `None` whenever the canvas is closed, the
/// integration is hidden, the connection is stale or unavailable, Signal View
/// is active, or the click misses every frame. Background phase, vignette,
/// and shadow cells resolve nothing. Overlapping frames resolve
/// topmost-first, with the selected frame in front, matching draw order.
/// `low_power` must mirror the render flag: it resolves against the
/// App-captured frozen frame exactly as [`render_canvas`] draws it (hit
/// testing is Connected-only, so the stale capture never applies here).
pub(super) fn hit_test(
    area: Rect,
    column: u16,
    row: u16,
    low_power: bool,
    app: &App,
) -> Option<Action> {
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
    let frame = if low_power {
        app.low_power_viz()
            .map(|(frame, _)| frame)
            .unwrap_or_else(|| app.viz())
    } else {
        app.viz()
    };
    let layout = collage_layout(agents, frame, &[], collage_area(area));
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

/// Render the full-screen Dual Phase Scope over the composed layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the title/count, the breathing
/// vignette, the phosphor-persistence and dual phase-trace layers, each
/// frame's shadow trails, state edge glow, and centered status core, the
/// selected explicit-name label beside its frame, and a restrained footer
/// hint. Stale renders the reducer-captured final composition dimmed under a
/// `reconnecting` banner; Unavailable hides every frame and trace behind calm
/// copy; `--low-power` renders the App-captured first frame so geometry stays
/// frozen while state colors refresh. `now` is injected by the render entry
/// point but deliberately unused: motion derives from audio frames only.
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
    // Geometry-source precedence: stale always wins with the display captured
    // by the reducer at the Connected→Stale edge; otherwise `--low-power`
    // renders the App-captured first frame so no trace, frame, shadow, or
    // spinner geometry advances; live renders use the current frame plus the
    // real prior frames behind it.
    let live_history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
    let fallback = (app.viz(), live_history.as_slice());
    let (frame, history) = if stale {
        app.stale_viz().unwrap_or(fallback)
    } else if low_power {
        app.low_power_viz().unwrap_or(fallback)
    } else {
        fallback
    };
    let layout = collage_layout(agents, frame, history, canvas);

    render_vignette(buf, canvas, layout.background.vignette, theme, stale);
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
        center_copy(buf, area, "agents · none active", muted);
        footer(buf, area, muted);
        return;
    }

    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));

    // Soft shadow trails sit behind every frame.
    for tile in &layout.tiles {
        for shadow in &tile.shadows {
            for y in shadow.y..shadow.y + shadow.height {
                for x in shadow.x..shadow.x + shadow.width {
                    buf.set_string(x, y, "∙", with_stale(dim_muted, stale));
                }
            }
        }
    }

    // Frames draw in stable slot order; the selected frame comes forward last.
    for tile in &layout.tiles {
        if Some(tile.index) == selected_index {
            continue;
        }
        render_tile(buf, tile, agents, theme, frame, false, stale, low_power);
    }
    if let Some(selected) = selected_index {
        if let Some(tile) = layout.tiles.iter().find(|tile| tile.index == selected) {
            render_tile(buf, tile, agents, theme, frame, true, stale, low_power);
        }
    }

    // Selected label: the explicit Herdr name only, placed beside its frame.
    // An unnamed selection shows no label at all — never a pane id, cwd, or
    // agent-type fallback.
    if let Some(view) = app.selected_agent() {
        if let Some(name) = &view.name {
            if let Some(tile) = selected_index
                .and_then(|selected| layout.tiles.iter().find(|tile| tile.index == selected))
            {
                let label = format!("{name} · {}", status_label(view.status));
                let rect = selection_label_rect(tile.rect, area, label.chars().count() as u16);
                let style = Style::default().fg(theme.foreground);
                buf.set_stringn(
                    rect.x,
                    rect.y,
                    &label,
                    rect.width as usize,
                    with_stale(style, stale),
                );
            }
        }
    }

    footer(buf, area, muted);
}

/// Draw one agent frame: the state edge glow around the border, a plain
/// interior, and a single centered status core.
#[allow(clippy::too_many_arguments)]
fn render_tile(
    buf: &mut Buffer,
    tile: &CollageTile,
    agents: &[AgentView],
    theme: &Theme,
    viz: &VizFrame,
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
    let base = theme.base_style();
    let rect = tile.rect;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let on_edge = x == rect.x
                || x == rect.x + rect.width - 1
                || y == rect.y
                || y == rect.y + rect.height - 1;
            if on_edge {
                buf.set_string(x, y, "▒", edge);
            } else {
                buf.set_string(x, y, " ", base);
            }
        }
    }

    let (glyph, spinning) = status_core(view.status, tile.seed, viz);
    let mut core = Style::default().fg(status_color(view.status, theme));
    if matches!(view.status, AgentStatus::Done | AgentStatus::Unknown) {
        core = core.add_modifier(Modifier::DIM);
    }
    if tile.energy <= SILENCE_ENERGY {
        core = core.add_modifier(Modifier::DIM);
    } else if spinning && tile.energy > BRIGHT_ENERGY && !low_power {
        core = core.add_modifier(Modifier::BOLD);
    }
    buf.set_string(
        rect.x + rect.width / 2,
        rect.y + rect.height / 2,
        glyph,
        with_stale(core, stale),
    );
}

/// Where the selected `name · status` label sits: the nearest in-bounds row
/// below the selected frame, or the row above it when the footer row would
/// collide, nudged left so the label stays inside the canvas.
fn selection_label_rect(frame_rect: Rect, area: Rect, width: u16) -> Rect {
    let footer_row = (area.y + area.height).saturating_sub(1);
    let below = frame_rect.y + frame_rect.height;
    let y = if below < footer_row {
        below
    } else {
        frame_rect.y.saturating_sub(1).max(area.y)
    };
    let max_x = (area.x + area.width).saturating_sub(width).max(area.x);
    let x = frame_rect.x.min(max_x);
    Rect::new(x, y, width.min(area.width), 1)
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

/// Edge glow style: theme status color, silence dims, strong signal emboldens
/// working frames (never in low power), done is always faded.
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
    use crate::model::PhaseTrace;
    use crate::settings::Settings;
    use crate::theme::ThemeName;
    use std::time::Duration;

    const CANVAS: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };

    /// Glyphs only agent frames (edges or status cores) may use. `·` is
    /// deliberately excluded: the Done/Unknown core shares it with the
    /// vignette, phosphor persistence, and copy separators.
    const FRAME_GLYPHS: [&str; 7] = ["▒", "◜", "◝", "◞", "◟", "◌", "×"];

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

    /// A non-silent frame whose two phase traces carry real-looking paired
    /// coordinates; `offset` shifts every pair so different audio data yields
    /// different scope geometry, never a timer.
    fn phase_frame_with_offset(offset: f32) -> VizFrame {
        let points = 48;
        let sample = |cycles: f32, i: usize| {
            ((i as f32 / points as f32) * std::f32::consts::TAU * cycles + offset).sin() * 0.8
        };
        let x: Vec<f32> = (0..points).map(|i| sample(1.0, i)).collect();
        let y: Vec<f32> = (0..points).map(|i| sample(2.0, i)).collect();
        let sx: Vec<f32> = (0..points).map(|i| sample(3.0, i) * 0.6).collect();
        let sy: Vec<f32> = (0..points).map(|i| sample(1.0, i) * 0.6).collect();
        VizFrame::with_phase(
            vec![0.5; 16],
            0.5,
            Vec::<f32>::new(),
            PhaseTrace::new(x, y),
            PhaseTrace::new(sx, sy),
        )
    }

    fn phase_frame() -> VizFrame {
        phase_frame_with_offset(0.2)
    }

    fn older_phase_frame() -> VizFrame {
        phase_frame_with_offset(0.9)
    }

    /// The Task-1 silence shape: RMS zero but non-empty, all-zero phase
    /// traces that would plot as a bright centered point cluster if the
    /// renderer failed to treat near-zero RMS as silence.
    fn silent_phase_frame() -> VizFrame {
        let zeros = || PhaseTrace::new(vec![0.0; 48], vec![0.0; 48]);
        VizFrame::with_phase(vec![0.0; 16], 0.0, Vec::<f32>::new(), zeros(), zeros())
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

    /// Shadow cells use a glyph no background or frame cell shares.
    fn count_shadow_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('∙').count()
    }

    fn count_primary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('•').count()
    }

    fn count_secondary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('◦').count()
    }

    /// Cells drawn with agent-frame glyphs (edges or cores).
    fn count_frame_cells(buf: &Buffer) -> usize {
        let text = buffer_text(buf);
        FRAME_GLYPHS
            .iter()
            .map(|glyph| text.matches(glyph).count())
            .sum()
    }

    /// Every non-blank cell of the collage region — vignette, phase layers,
    /// shadows, frames, and cores — as `(x, y, symbol)`, so tests can compare
    /// whole-field geometry between renders.
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

    /// Whole-field geometry — cell positions and glyphs, never colors — so
    /// freeze tests can compare stale/low-power output against live output
    /// whose colors intentionally differ (stale dimming, state colors).
    fn phase_and_core_geometry(buf: &Buffer) -> Vec<(u16, u16, String)> {
        field_cells(buf)
    }

    /// A connected app with one agent per interesting status over `frame`.
    fn status_app(frame: VizFrame) -> App {
        let mut app = collage_app(vec![
            snap("ws", "w", Some("w"), AgentStatus::Working),
            snap("ws", "i", Some("i"), AgentStatus::Idle),
            snap("ws", "b", Some("b"), AgentStatus::Blocked),
            snap("ws", "d", Some("d"), AgentStatus::Done),
        ]);
        push_frame(&mut app, frame);
        app
    }

    /// The rendered core glyph at the center of the first agent with `status`.
    fn core_glyph(app: &App, status: AgentStatus) -> String {
        let buf = render_collage_for(app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let index = app
            .active_agents()
            .iter()
            .position(|view| view.status == status)
            .unwrap();
        let tile = layout
            .tiles
            .iter()
            .find(|tile| tile.index == index)
            .unwrap();
        let (x, y) = (
            tile.rect.x + tile.rect.width / 2,
            tile.rect.y + tile.rect.height / 2,
        );
        buf.cell((x, y)).unwrap().symbol().to_string()
    }

    /// A connected single-agent app that received one phase frame.
    fn connected_app_with_phase(offset: f32) -> App {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame_with_offset(offset));
        app
    }

    /// Connected, saw `first`, went stale, then received `later` audio that
    /// must not thaw the frozen composition.
    fn stale_app_captured_from(first: f32, later: f32) -> App {
        let mut app = connected_app_with_phase(first);
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        push_frame(&mut app, phase_frame_with_offset(later));
        app
    }

    /// Low-power policy on, captured `first`, then received `later` audio
    /// that must not replace the frozen geometry capture.
    fn low_power_app_captured_from(first: f32, later: f32) -> App {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        push_frame(&mut app, phase_frame_with_offset(first));
        push_frame(&mut app, phase_frame_with_offset(later));
        app
    }

    fn render_phase_and_cores(app: &App, low_power: bool) -> Buffer {
        render_collage_for(app, low_power, Instant::now())
    }

    // --- pure layout -------------------------------------------------------

    #[test]
    fn frame_seed_and_staggered_rect_stay_stable_for_an_agent_identity() {
        let area = Rect::new(0, 0, 120, 36);
        let agent = view("alpha", "p1", AgentStatus::Working);
        let first = collage_layout(
            std::slice::from_ref(&agent),
            &frame(0.0, vec![0.0; 16]),
            &[],
            area,
        );
        let later = collage_layout(&[agent], &phase_frame(), &[frame(0.1, vec![0.1; 16])], area);
        assert_eq!(first.tiles[0].seed, later.tiles[0].seed);
        assert_eq!(first.tiles[0].base_rect, later.tiles[0].base_rect);
    }

    #[test]
    fn dense_collage_keeps_one_frame_per_agent() {
        let area = Rect::new(0, 0, 50, 15);
        let layout = collage_layout(&agents(80), &frame(0.5, vec![0.5; 16]), &[], area);
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
    fn frame_rect_is_stable_when_status_changes() {
        let before = collage_layout(
            &[view("alpha", "p1", AgentStatus::Working)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
        );
        let after = collage_layout(
            &[view("alpha", "p1", AgentStatus::Blocked)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
        );
        assert_eq!(before.tiles[0].base_rect, after.tiles[0].base_rect);
        assert_eq!(before.tiles[0].rect, after.tiles[0].rect);
    }

    #[test]
    fn frames_differ_for_identical_panes_in_different_workspaces() {
        let layout = collage_layout(
            &[
                view("alpha", "p1", AgentStatus::Working),
                view("beta", "p1", AgentStatus::Working),
            ],
            &frame(0.0, vec![0.0; 16]),
            &[],
            CANVAS,
        );
        assert_eq!(layout.tiles.len(), 2);
        assert_ne!(layout.tiles[0].base_rect, layout.tiles[1].base_rect);
    }

    // --- dual phase scope --------------------------------------------------

    #[test]
    fn phase_cells_map_normalized_pairs_onto_the_centered_canvas() {
        let area = Rect::new(10, 5, 21, 11);
        let trace = PhaseTrace::new([0.0, -1.0, 1.0], [0.0, -1.0, 1.0]);
        let cells = phase_cells(&trace, area);
        assert_eq!(
            (cells[0].x, cells[0].y),
            (20, 10),
            "the origin plots at the canvas center"
        );
        assert_eq!(
            (cells[1].x, cells[1].y),
            (10, 15),
            "-1/-1 plots left/bottom"
        );
        assert_eq!((cells[2].x, cells[2].y), (30, 5), "+1/+1 plots right/top");
    }

    #[test]
    fn dual_phase_scope_draws_two_centered_non_scrolling_traces() {
        let buf = render_collage(2, phase_frame(), vec![older_phase_frame()], false);
        assert!(count_primary_phase_cells(&buf) > 0, "primary trace draws");
        assert!(
            count_secondary_phase_cells(&buf) > 0,
            "secondary trace draws"
        );
        let text = buffer_text(&buf);
        for glyph in ["▁", "~", "≈", "≋"] {
            assert!(
                !text.contains(glyph),
                "no scrolling-waveform glyph {glyph} may render"
            );
        }
    }

    #[test]
    fn phase_scope_uses_audio_pairs_not_elapsed_time() {
        let mut app = collage_app(vec![
            snap("ws", "p0", None, AgentStatus::Working),
            snap("ws", "p1", None, AgentStatus::Working),
        ]);
        push_frame(&mut app, phase_frame());
        assert_eq!(
            render_collage_for(&app, false, Instant::now()),
            render_collage_for(&app, false, Instant::now() + Duration::from_secs(9)),
            "identical frame data at different instants renders identical cells"
        );
    }

    #[test]
    fn silence_renders_no_phase_trace_even_with_nonempty_zero_traces() {
        let buf = render_collage(2, silent_phase_frame(), vec![silent_phase_frame()], false);
        assert_eq!(
            count_primary_phase_cells(&buf),
            0,
            "silent primary trace draws nothing"
        );
        assert_eq!(
            count_secondary_phase_cells(&buf),
            0,
            "silent secondary trace draws nothing"
        );

        // A silent history frame adds no phosphor behind a live trace either.
        let with_silent_history =
            render_collage(2, phase_frame(), vec![silent_phase_frame()], false);
        let without_history = render_collage(2, phase_frame(), vec![], false);
        assert_eq!(
            with_silent_history, without_history,
            "silent history leaves no persistence"
        );
    }

    #[test]
    fn phosphor_persistence_adds_dim_dots_only_from_real_history_frames() {
        let with = render_collage(2, phase_frame(), vec![older_phase_frame()], false);
        let without = render_collage(2, phase_frame(), vec![], false);
        let dots = |buf: &Buffer| buffer_text(buf).matches('·').count();
        assert!(
            dots(&with) > dots(&without),
            "a real history frame grows dim persistence dots"
        );
    }

    // --- status cores ------------------------------------------------------

    #[test]
    fn working_core_changes_with_new_audio_while_idle_and_blocked_stay_still() {
        let first = status_app(phase_frame_with_offset(0.1));
        let next = status_app(phase_frame_with_offset(0.7));
        assert_ne!(
            core_glyph(&first, AgentStatus::Working),
            core_glyph(&next, AgentStatus::Working),
            "new audio data rotates the working spinner"
        );
        assert_eq!(
            core_glyph(&first, AgentStatus::Idle),
            core_glyph(&next, AgentStatus::Idle)
        );
        assert_eq!(
            core_glyph(&first, AgentStatus::Blocked),
            core_glyph(&next, AgentStatus::Blocked)
        );
        assert_eq!(
            core_glyph(&first, AgentStatus::Done),
            core_glyph(&next, AgentStatus::Done)
        );
    }

    #[test]
    fn status_cores_use_documented_glyphs_and_colors() {
        let app = status_app(phase_frame());
        assert!(
            ["◜", "◝", "◞", "◟"].contains(&core_glyph(&app, AgentStatus::Working).as_str()),
            "working shows a spinner frame"
        );
        assert_eq!(core_glyph(&app, AgentStatus::Idle), "◌");
        assert_eq!(core_glyph(&app, AgentStatus::Blocked), "×");
        assert_eq!(core_glyph(&app, AgentStatus::Done), "·");

        // The blocked core takes the theme's error color; done stays dim.
        let theme = Theme::for_name(ThemeName::Minimal);
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let center_style = |status: AgentStatus| {
            let index = app
                .active_agents()
                .iter()
                .position(|view| view.status == status)
                .unwrap();
            let tile = layout
                .tiles
                .iter()
                .find(|tile| tile.index == index)
                .unwrap();
            buf.cell((
                tile.rect.x + tile.rect.width / 2,
                tile.rect.y + tile.rect.height / 2,
            ))
            .unwrap()
            .style()
        };
        assert_eq!(center_style(AgentStatus::Blocked).fg, Some(theme.error));
        assert!(center_style(AgentStatus::Done)
            .add_modifier
            .contains(Modifier::DIM));
    }

    #[test]
    fn frame_interiors_hold_no_album_art_between_edge_and_core() {
        let app = status_app(phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        for tile in &layout.tiles {
            let rect = tile.rect;
            let center = (rect.x + rect.width / 2, rect.y + rect.height / 2);
            for y in rect.y + 1..rect.y + rect.height - 1 {
                for x in rect.x + 1..rect.x + rect.width - 1 {
                    if (x, y) == center {
                        continue;
                    }
                    assert_eq!(
                        buf.cell((x, y)).unwrap().symbol(),
                        " ",
                        "interior cell ({x}, {y}) must hold no art"
                    );
                }
            }
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
            "loud frames must move background and frames, not just restyle them"
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
        assert_eq!(first, later, "silent scope must not animate with time");
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

    // --- low power and freeze behavior -------------------------------------

    #[test]
    fn stale_and_low_power_freeze_phase_and_spinner_geometry() {
        let live = render_phase_and_cores(&connected_app_with_phase(0.3), false);
        let stale = render_phase_and_cores(&stale_app_captured_from(0.3, 0.9), false);
        let low_power = render_phase_and_cores(&low_power_app_captured_from(0.3, 0.9), true);
        assert!(
            !phase_and_core_geometry(&live).is_empty(),
            "sanity: the live field renders"
        );
        assert_eq!(
            phase_and_core_geometry(&stale),
            phase_and_core_geometry(&live)
        );
        assert_eq!(
            phase_and_core_geometry(&low_power),
            phase_and_core_geometry(&live)
        );
    }

    #[test]
    fn low_power_keeps_geometry_fixed_while_colors_refresh() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        push_frame(&mut app, phase_frame_with_offset(0.2));
        let first = render_collage_for(&app, true, Instant::now());
        push_frame(&mut app, phase_frame_with_offset(0.8));
        let later = render_collage_for(&app, true, Instant::now());
        assert!(!field_cells(&first).is_empty());
        assert_eq!(
            field_cells(&first),
            field_cells(&later),
            "low power freezes trace, frame, shadow, and core geometry"
        );

        // The same later frame in normal power does move the scope.
        let live = render_collage_for(&app, false, Instant::now());
        assert_ne!(field_cells(&later), field_cells(&live));

        // A fresh snapshot still recolors the frozen frame edge in place.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Blocked)],
            now: Instant::now(),
        });
        let theme = Theme::for_name(ThemeName::Minimal);
        let recolored = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = collage_layout(app.active_agents(), captured, &[], collage_area(CANVAS));
        let corner = (layout.tiles[0].rect.x, layout.tiles[0].rect.y);
        assert_eq!(
            recolored.cell(corner).unwrap().style().fg,
            Some(theme.error),
            "the frozen edge takes the fresh blocked color"
        );
    }

    #[test]
    fn low_power_capture_skips_startup_silence_until_an_audible_frame() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        push_frame(&mut app, silent_phase_frame());
        assert!(
            app.low_power_viz().is_none(),
            "startup silence retains no capture"
        );
        // Before any audible frame, low power falls back to the live silent
        // frame: a calm, traceless field.
        let silent_render = render_collage_for(&app, true, Instant::now());
        assert_eq!(count_primary_phase_cells(&silent_render), 0);
        assert_eq!(count_secondary_phase_cells(&silent_render), 0);

        // The first audible frame becomes the frozen geometry; later audio
        // keeps colors fresh but never thaws it.
        push_frame(&mut app, phase_frame_with_offset(0.3));
        let captured = render_collage_for(&app, true, Instant::now());
        assert!(
            count_primary_phase_cells(&captured) > 0,
            "the audible capture draws its trace"
        );
        push_frame(&mut app, phase_frame_with_offset(0.9));
        let later = render_collage_for(&app, true, Instant::now());
        assert_eq!(
            field_cells(&captured),
            field_cells(&later),
            "geometry stays frozen on the first audible frame"
        );
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
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
    fn selected_named_frame_shows_only_name_and_status_near_its_frame() {
        let mut app = app_with_named_and_unnamed_agents();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let text = buffer_text(&buf);
        assert!(
            text.contains("research · working"),
            "selected explicit-name label missing: {text}"
        );
        assert!(!text.contains("workspace-1"), "workspace ids never render");
        assert!(!text.contains("pane-1"), "pane ids never render");
        assert!(!text.contains("claude"), "raw pane details never render");

        // The label hugs the selected frame instead of a fixed footer row.
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let selected_index = app
            .active_agents()
            .iter()
            .position(|view| view.name.as_deref() == Some("research"))
            .unwrap();
        let rect = layout
            .tiles
            .iter()
            .find(|tile| tile.index == selected_index)
            .unwrap()
            .rect;
        let label_row = text
            .lines()
            .position(|line| line.contains("research · working"))
            .unwrap() as u16;
        assert!(
            label_row == rect.y + rect.height || label_row + 1 == rect.y,
            "label row {label_row} must sit adjacent to its frame {rect:?}"
        );
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
        push_frame(&mut app, older_phase_frame());
        push_frame(&mut app, phase_frame());
        let live = render_collage_for(&app, false, Instant::now());
        let live_field = field_cells(&live);
        assert!(
            count_shadow_cells(&live) > 0,
            "sanity: the final live frame has shadows to freeze"
        );
        assert!(
            count_primary_phase_cells(&live) > 0,
            "sanity: the final live frame has a phase trace to freeze"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale_buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(
            field_cells(&stale_buf),
            live_field,
            "stale freezes the exact background, shadow, and frame geometry of the last live frame"
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

        // Later live audio frames and elapsed time must not thaw the scope.
        push_frame(&mut app, frame(0.1, vec![0.05; 16]));
        push_frame(&mut app, phase_frame_with_offset(1.4));
        let later = render_collage_for(&app, false, Instant::now() + Duration::from_secs(9));
        assert_eq!(
            later, stale_buf,
            "stale output is invariant across later audio frames and time"
        );
    }

    #[test]
    fn unavailable_hides_frames_and_traces_behind_calm_copy() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(count_frame_cells(&buf), 0, "no agent frames render");
        assert_eq!(count_primary_phase_cells(&buf), 0, "no phase trace renders");
        assert_eq!(count_secondary_phase_cells(&buf), 0);
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
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

        let action = hit_test(CANVAS, x, y, false, &app).expect("a tile click selects");
        app.apply(action);
        assert_eq!(
            app.selected_agent().unwrap().name.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn clicks_resolve_only_tile_cells_never_background_or_shadows() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, older_phase_frame());
        push_frame(&mut app, phase_frame());
        let canvas = collage_area(CANVAS);
        let history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
        let layout = collage_layout(app.active_agents(), app.viz(), &history, canvas);
        let inside_a_tile = |x: u16, y: u16| {
            layout
                .tiles
                .iter()
                .any(|tile| rect_contains(tile.rect, x, y))
        };

        let mut checked_background = false;
        for layer in &layout.background.layers {
            for cell in &layer.cells {
                if !inside_a_tile(cell.x, cell.y) {
                    checked_background = true;
                    assert!(
                        hit_test(CANVAS, cell.x, cell.y, false, &app).is_none(),
                        "a background phase cell at ({}, {}) must resolve nothing",
                        cell.x,
                        cell.y
                    );
                }
            }
        }
        assert!(
            checked_background,
            "sanity: some phase cell sat outside tiles"
        );

        let mut checked_shadow = false;
        for tile in &layout.tiles {
            for shadow in &tile.shadows {
                for y in shadow.y..shadow.y + shadow.height {
                    for x in shadow.x..shadow.x + shadow.width {
                        if !inside_a_tile(x, y) {
                            checked_shadow = true;
                            assert!(
                                hit_test(CANVAS, x, y, false, &app).is_none(),
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
        assert!(hit_test(CANVAS, 0, 0, false, &app).is_none(), "corner miss");

        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let tile = &layout.tiles[0];
        let (x, y) = (
            tile.rect.x + tile.rect.width / 2,
            tile.rect.y + tile.rect.height / 2,
        );
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "stale ignores clicks"
        );

        let mut closed = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        closed.apply(Action::CloseAgentOverlay);
        assert!(
            hit_test(CANVAS, x, y, false, &closed).is_none(),
            "closed canvas ignores clicks"
        );
    }

    #[test]
    fn low_power_hit_testing_matches_the_frozen_drawn_tiles() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        // The first audible frame — quiet enough to sit on base geometry —
        // becomes the frozen low-power capture; the later loud frame moves
        // normal-power tiles off their base rectangles.
        push_frame(
            &mut app,
            VizFrame::with_phase(
                vec![0.0; 16],
                0.1,
                Vec::<f32>::new(),
                PhaseTrace::new([0.1], [0.1]),
                PhaseTrace::empty(),
            ),
        );
        push_frame(&mut app, frame(0.9, vec![0.9; 16]));
        let canvas = collage_area(CANVAS);
        let moved = collage_layout(app.active_agents(), app.viz(), &[], canvas).tiles[0].rect;
        let (captured, _) = app.low_power_viz().expect("low power captured a frame");
        let frozen = &collage_layout(app.active_agents(), captured, &[], canvas).tiles[0];
        let base = frozen.rect;
        assert_eq!(base, frozen.base_rect, "the quiet capture sits on base");
        assert_ne!(moved, base, "sanity: a loud frame moves the tile off base");

        // The frozen tile's own cells still select in low power.
        let (x, y) = (base.x + base.width / 2, base.y + base.height / 2);
        assert!(
            hit_test(CANVAS, x, y, true, &app).is_some(),
            "a drawn low-power tile cell selects"
        );

        // A cell only the audio-moved rectangle covers holds no drawn tile in
        // low power, so it must resolve nothing there.
        let mut checked = false;
        for y in moved.y..moved.y + moved.height {
            for x in moved.x..moved.x + moved.width {
                if !rect_contains(base, x, y) {
                    checked = true;
                    assert!(
                        hit_test(CANVAS, x, y, true, &app).is_none(),
                        "an undrawn cell ({x}, {y}) must resolve nothing in low power"
                    );
                }
            }
        }
        assert!(checked, "sanity: the moved rect exposes cells off base");
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
