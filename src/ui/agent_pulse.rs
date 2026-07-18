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
//! deterministically placed bounding rectangle whose bounded audio-driven
//! displacement and soft shadow trails carry over from the Kinetic Collage;
//! inside it the renderer draws a Ringed Planet — a clipped round body with
//! stable seed-derived craters and a thin orbit ring — never a square frame.
//! Status is orbit language: Working's complete playing-colored ring carries
//! a bright arc whose position advances only with new played-audio phase
//! data; Idle keeps a muted still ring; Blocked draws an error-colored
//! broken orbit with a stable seed-derived gap and never any cross-like
//! glyph; Done dims and keeps a small satellite on its orbit; Unknown stays
//! muted and dim. Nothing moves from a timer: identical frames render
//! identical cells. A frame at or below the silence threshold draws no
//! trace or persistence at all — analyzer silence carries non-empty
//! all-zero traces that would otherwise pile a point cluster at the canvas
//! center — so silence stays calm, dim, and still. Stale renders the
//! reducer-captured final composition dimmed; `--low-power` renders the
//! App-captured first frame so trace, planet, shadow, and ring-arc geometry
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
/// Glyph filling a planet's round body.
const PLANET_BODY_GLYPH: &str = "▓";
/// Glyph shading a stable seed-derived crater inside a planet body.
const CRATER_GLYPH: &str = "░";
/// Glyph tracing a planet's orbit ring.
const RING_GLYPH: &str = "∘";
/// Glyph for the bright audio-positioned arc on a Working planet's ring.
const WORKING_ARC_GLYPH: &str = "●";
/// Glyph for the small satellite a Done planet keeps on its orbit.
const SATELLITE_GLYPH: &str = "▪";
/// Minimum drawn-bound width before optional orbit-ring cells appear.
const RING_MIN_W: u16 = 5;
/// Minimum drawn-bound height before optional orbit-ring cells appear.
const RING_MIN_H: u16 = 3;
/// Minimum body cells before optional crater detail appears.
const CRATER_MIN_BODY: usize = 6;
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

// --- planet geometry --------------------------------------------------------

/// One agent's Ringed Planet in canvas cells, derived purely from its tile,
/// status, and the current phase frame. `base_body` follows the stable
/// `base_rect` so tests can pin identity geometry; every other field follows
/// the audio-transformed `rect`. Only `working_arc` consults phase data —
/// every other status keeps still geometry across audio frames.
struct PlanetGeometry {
    /// The round body derived from the stable pre-transform rectangle.
    #[cfg_attr(not(test), allow(dead_code))]
    base_body: Vec<(u16, u16)>,
    body: Vec<(u16, u16)>,
    craters: Vec<(u16, u16)>,
    ring: Vec<(u16, u16)>,
    working_arc: Vec<(u16, u16)>,
    satellite: Option<(u16, u16)>,
    /// Body and ring cells only: the future planet-only selection targets.
    #[cfg_attr(not(test), allow(dead_code))]
    hit_cells: Vec<(u16, u16)>,
}

/// Whether the cell offset (`dx`, `dy`) from a planet center falls inside
/// the terminal-aspect-aware ellipse with the given semi-axes.
fn is_body_cell(dx: i32, dy: i32, radius_x: i32, radius_y: i32) -> bool {
    dx * dx * radius_y * radius_y + dy * dy * radius_x * radius_x
        <= radius_x * radius_x * radius_y * radius_y
}

/// Center and semi-axes of the body ellipse inscribed in `rect`.
fn ellipse_of(rect: Rect) -> (i32, i32, i32, i32) {
    let cx = rect.x as i32 + (rect.width as i32 - 1) / 2;
    let cy = rect.y as i32 + (rect.height as i32 - 1) / 2;
    let radius_x = ((rect.width as i32 - 1) / 2).max(1);
    let radius_y = ((rect.height as i32 - 1) / 2).max(1);
    (cx, cy, radius_x, radius_y)
}

/// The round body cells for `rect`, clipped to `area`. A one- or two-cell
/// dense bound keeps exactly its center cell so every agent stays visible.
fn body_cells(rect: Rect, area: Rect) -> Vec<(u16, u16)> {
    let (cx, cy, radius_x, radius_y) = ellipse_of(rect);
    if rect.width as u32 * rect.height as u32 <= 2 {
        return vec![(cx as u16, cy as u16)];
    }
    let mut cells = Vec::new();
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            if is_body_cell(x as i32 - cx, y as i32 - cy, radius_x, radius_y)
                && rect_contains(area, x, y)
            {
                cells.push((x, y));
            }
        }
    }
    cells
}

/// The ordered orbit-ring cells around the body ellipse at `radius_x + 1`,
/// clipped to `area` and excluding body cells; empty when the bound is too
/// small for optional ring detail. Ring inclination flattens by one row for
/// half of the identity seeds, so neighbours read differently but each
/// planet's orbit stays recognizable.
fn ring_cells(rect: Rect, area: Rect, seed: u64) -> Vec<(u16, u16)> {
    if rect.width < RING_MIN_W || rect.height < RING_MIN_H {
        return Vec::new();
    }
    let (cx, cy, radius_x, radius_y) = ellipse_of(rect);
    let ring_x = radius_x + 1;
    let ring_y = if (seed >> 2) & 1 == 0 {
        radius_y
    } else {
        (radius_y - 1).max(1)
    };
    let samples = ((ring_x + ring_y) * 4).max(8);
    let mut cells: Vec<(u16, u16)> = Vec::new();
    for step in 0..samples {
        let t = step as f32 * std::f32::consts::TAU / samples as f32;
        let x = cx + (ring_x as f32 * t.cos()).round() as i32;
        let y = cy + (ring_y as f32 * t.sin()).round() as i32;
        if x < 0 || y < 0 {
            continue;
        }
        let cell = (x as u16, y as u16);
        if !rect_contains(area, cell.0, cell.1)
            || is_body_cell(x - cx, y - cy, radius_x, radius_y)
            || cells.contains(&cell)
        {
            continue;
        }
        cells.push(cell);
    }
    cells
}

/// The Blocked orbit: the complete ring minus one deterministic contiguous
/// arc segment, so the gap is stable across audio frames and wall-clock
/// time and never reads as a cross.
fn broken_ring(ring: &[(u16, u16)], seed: u64) -> Vec<(u16, u16)> {
    if ring.len() < 3 {
        return ring.to_vec();
    }
    let gap_len = (ring.len() / 4).max(2);
    let gap_start = (seed % ring.len() as u64) as usize;
    ring.iter()
        .enumerate()
        .filter_map(|(index, &cell)| {
            let offset = (index + ring.len() - gap_start) % ring.len();
            (offset >= gap_len).then_some(cell)
        })
        .collect()
}

/// The bright Working arc: a short contiguous segment of the complete ring
/// whose position is a pure function of the real primary-phase data plus
/// the identity seed — new audio moves it; elapsed time cannot.
fn working_arc(ring: &[(u16, u16)], seed: u64, frame: &VizFrame) -> Vec<(u16, u16)> {
    if ring.is_empty() {
        return Vec::new();
    }
    let len = ring.len();
    let arc_len = (len / 6).clamp(2, len);
    let start = (phase_signature(&frame.primary_phase).wrapping_add(seed) % len as u64) as usize;
    (0..arc_len)
        .map(|step| ring[(start + step) % len])
        .collect()
}

/// Derive one planet's full cell geometry from its tile. Craters index into
/// the body by identity seed, so their arrangement rides along with the
/// bounded audio transform but never reshuffles; tiny dense bounds drop the
/// optional crater/ring/satellite detail before ever omitting the body.
fn planet_geometry(
    tile: &CollageTile,
    area: Rect,
    status: AgentStatus,
    frame: &VizFrame,
) -> PlanetGeometry {
    let base_body = body_cells(tile.base_rect, area);
    let body = body_cells(tile.rect, area);
    let ring = ring_cells(tile.rect, area, tile.seed);
    let craters = if body.len() >= CRATER_MIN_BODY {
        let first = (tile.seed % body.len() as u64) as usize;
        let second = ((tile.seed >> 11) % body.len() as u64) as usize;
        let mut craters = vec![body[first]];
        if second != first {
            craters.push(body[second]);
        }
        craters
    } else {
        Vec::new()
    };
    let working_arc = if status == AgentStatus::Working {
        working_arc(&ring, tile.seed, frame)
    } else {
        Vec::new()
    };
    let satellite = if status == AgentStatus::Done && !ring.is_empty() {
        Some(ring[((tile.seed >> 7) % ring.len() as u64) as usize])
    } else {
        None
    };
    let hit_cells = body.iter().chain(&ring).copied().collect();
    PlanetGeometry {
        base_body,
        body,
        craters,
        ring,
        working_arc,
        satellite,
        hit_cells,
    }
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
/// planet's shadow trails, round body, craters, and status orbit ring, the
/// selected explicit-name label beside its planet, and a restrained footer
/// hint. Stale renders the reducer-captured final composition dimmed under a
/// `reconnecting` banner; Unavailable hides every planet and trace behind
/// calm copy; `--low-power` renders the App-captured first frame so geometry
/// stays frozen while state colors refresh. `now` is injected by the render
/// entry point but deliberately unused: motion derives from audio frames
/// only.
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

    // Planets draw in stable slot order; the selected planet comes forward
    // last.
    for tile in &layout.tiles {
        if Some(tile.index) == selected_index {
            continue;
        }
        render_planet(
            buf, tile, agents, theme, frame, canvas, false, stale, low_power,
        );
    }
    if let Some(selected) = selected_index {
        if let Some(tile) = layout.tiles.iter().find(|tile| tile.index == selected) {
            render_planet(
                buf, tile, agents, theme, frame, canvas, true, stale, low_power,
            );
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

/// Draw one agent planet in order: round body fill, stable crater shading,
/// then the status orbit language — a complete playing-colored ring with a
/// bright audio-positioned arc while Working, a muted still ring while
/// Idle, an error-colored broken orbit while Blocked (never a cross), a dim
/// ring plus a small satellite while Done, and a dim muted ring for
/// Unknown. Selection restyles the existing body/ring cells; it never draws
/// a rectangle.
#[allow(clippy::too_many_arguments)]
fn render_planet(
    buf: &mut Buffer,
    tile: &CollageTile,
    agents: &[AgentView],
    theme: &Theme,
    viz: &VizFrame,
    area: Rect,
    selected: bool,
    stale: bool,
    low_power: bool,
) {
    let Some(view) = agents.get(tile.index) else {
        return;
    };
    let geometry = planet_geometry(tile, area, view.status, viz);
    let silent = tile.energy <= SILENCE_ENERGY;
    let quiet_dim = |style: Style| {
        if silent {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    };

    let mut body = Style::default().fg(theme.muted);
    if matches!(view.status, AgentStatus::Done | AgentStatus::Unknown) {
        body = body.add_modifier(Modifier::DIM);
    }
    let body = with_stale(quiet_dim(body), stale);
    for &(x, y) in &geometry.body {
        buf.set_string(x, y, PLANET_BODY_GLYPH, body);
    }
    let crater = body.add_modifier(Modifier::DIM);
    for &(x, y) in &geometry.craters {
        buf.set_string(x, y, CRATER_GLYPH, crater);
    }

    let (ring, ring_style) = match view.status {
        AgentStatus::Working => (
            geometry.ring.clone(),
            edge_style(view.status, theme, tile.energy, low_power),
        ),
        AgentStatus::Idle => (
            geometry.ring.clone(),
            quiet_dim(Style::default().fg(theme.muted)),
        ),
        AgentStatus::Blocked => (
            broken_ring(&geometry.ring, tile.seed),
            quiet_dim(Style::default().fg(theme.error)),
        ),
        AgentStatus::Done | AgentStatus::Unknown => (
            geometry.ring.clone(),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ),
    };
    let ring_style = if selected {
        theme.selection_style()
    } else {
        ring_style
    };
    let ring_style = with_stale(ring_style, stale);
    for &(x, y) in &ring {
        buf.set_string(x, y, RING_GLYPH, ring_style);
    }

    // The bright arc rides the complete Working ring; only new real audio
    // data repositions it.
    let mut arc = if selected {
        theme.selection_style()
    } else {
        edge_style(AgentStatus::Working, theme, tile.energy, low_power)
    };
    if !silent && !low_power {
        arc = arc.add_modifier(Modifier::BOLD);
    }
    let arc = with_stale(arc, stale);
    for &(x, y) in &geometry.working_arc {
        buf.set_string(x, y, WORKING_ARC_GLYPH, arc);
    }

    if let Some((x, y)) = geometry.satellite {
        let satellite = with_stale(
            quiet_dim(Style::default().fg(theme.muted).add_modifier(Modifier::DIM)),
            stale,
        );
        buf.set_string(x, y, SATELLITE_GLYPH, satellite);
    }
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

    /// Glyphs only agent planets (bodies, craters, orbit rings, Working
    /// arcs, and Done satellites) may use. `·` stays excluded: the vignette,
    /// phosphor persistence, and copy separators share it.
    const PLANET_GLYPHS: [&str; 5] = [
        PLANET_BODY_GLYPH,
        CRATER_GLYPH,
        RING_GLYPH,
        WORKING_ARC_GLYPH,
        SATELLITE_GLYPH,
    ];

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

    /// Cells drawn with agent-planet glyphs (bodies, craters, or rings).
    fn count_planet_cells(buf: &Buffer) -> usize {
        let text = buffer_text(buf);
        PLANET_GLYPHS
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

    /// A connected app with one agent per status over `frame`.
    fn status_app(frame: VizFrame) -> App {
        let mut app = collage_app(vec![
            snap("ws", "w", Some("w"), AgentStatus::Working),
            snap("ws", "i", Some("i"), AgentStatus::Idle),
            snap("ws", "b", Some("b"), AgentStatus::Blocked),
            snap("ws", "d", Some("d"), AgentStatus::Done),
            snap("ws", "u", Some("u"), AgentStatus::Unknown),
        ]);
        push_frame(&mut app, frame);
        app
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

    // --- ringed planets ----------------------------------------------------

    /// The laid-out tile for one agent alone in `area` under `frame`.
    fn tile_for(agent: &AgentView, area: Rect, frame: &VizFrame) -> CollageTile {
        collage_layout(std::slice::from_ref(agent), frame, &[], area)
            .tiles
            .remove(0)
    }

    /// The ring cells a status actually draws: Blocked breaks its orbit,
    /// every other status keeps the complete ring.
    fn drawn_ring(geometry: &PlanetGeometry, seed: u64, status: AgentStatus) -> Vec<(u16, u16)> {
        if status == AgentStatus::Blocked {
            broken_ring(&geometry.ring, seed)
        } else {
            geometry.ring.clone()
        }
    }

    /// Every drawn planet cell (body, status ring, arc, satellite) of the
    /// first agent with `status`, as `(x, y, symbol)` from the buffer.
    fn planet_cells_for(app: &App, status: AgentStatus) -> Vec<(u16, u16, String)> {
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
        let geometry = planet_geometry(tile, collage_area(CANVAS), status, app.viz());
        let mut cells: Vec<(u16, u16)> = geometry.body.clone();
        cells.extend(drawn_ring(&geometry, tile.seed, status));
        cells.extend(geometry.working_arc.iter().copied());
        cells.extend(geometry.satellite);
        cells.sort_unstable();
        cells.dedup();
        cells
            .into_iter()
            .map(|(x, y)| (x, y, buf.cell((x, y)).unwrap().symbol().to_string()))
            .collect()
    }

    /// The audio-positioned Working arc cells of the first working agent.
    fn arc_cells_for(app: &App) -> Vec<(u16, u16)> {
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let index = app
            .active_agents()
            .iter()
            .position(|view| view.status == AgentStatus::Working)
            .unwrap();
        let tile = layout
            .tiles
            .iter()
            .find(|tile| tile.index == index)
            .unwrap();
        planet_geometry(tile, collage_area(CANVAS), AgentStatus::Working, app.viz()).working_arc
    }

    #[test]
    fn stable_identity_produces_a_round_body_craters_and_ring() {
        let area = Rect::new(0, 0, 120, 36);
        let agent = view("work-a", "pane-1", AgentStatus::Working);
        let first = planet_geometry(
            &tile_for(&agent, area, &phase_frame()),
            area,
            AgentStatus::Working,
            &phase_frame(),
        );
        let later = planet_geometry(
            &tile_for(&agent, area, &phase_frame_with_offset(0.8)),
            area,
            AgentStatus::Working,
            &phase_frame_with_offset(0.8),
        );
        assert_eq!(
            first.base_body, later.base_body,
            "identity fixes the base body"
        );
        assert_eq!(first.craters, later.craters, "identity fixes the craters");
        assert!(!first.body.is_empty(), "the live body renders cells");
        assert!(first.ring.len() >= 2, "a roomy planet keeps an orbit ring");
        let expected: Vec<(u16, u16)> = first.body.iter().chain(&first.ring).copied().collect();
        assert_eq!(
            first.hit_cells, expected,
            "hit cells are exactly the body and ring cells"
        );
    }

    #[test]
    fn blocked_planet_uses_a_broken_error_orbit_without_cross_glyphs() {
        let mut app = collage_app(vec![snap("ws", "b", Some("b"), AgentStatus::Blocked)]);
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let text = buffer_text(&buf);
        for cross in ['×', '╳', '╲', '╱', '✕', '+'] {
            assert!(
                !text.contains(cross),
                "no cross-like glyph {cross} may render"
            );
        }

        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, collage_area(CANVAS), AgentStatus::Blocked, app.viz());
        let broken = broken_ring(&geometry.ring, tile.seed);
        assert!(broken.len() >= 2, "the broken orbit keeps visible cells");
        assert!(
            broken.len() < geometry.ring.len(),
            "the blocked orbit must carry a gap"
        );
        let theme = Theme::for_name(ThemeName::Minimal);
        for &(x, y) in &broken {
            assert_eq!(buf.cell((x, y)).unwrap().symbol(), RING_GLYPH);
            assert_eq!(buf.cell((x, y)).unwrap().style().fg, Some(theme.error));
        }
        for cell in geometry.ring.iter().filter(|cell| !broken.contains(cell)) {
            assert_ne!(
                buf.cell((cell.0, cell.1)).unwrap().symbol(),
                RING_GLYPH,
                "gap cell {cell:?} must stay free of ring cells"
            );
        }
    }

    #[test]
    fn working_arc_changes_with_audio_but_other_planet_states_do_not() {
        let quiet = status_app(phase_frame_with_offset(0.1));
        let loud = status_app(phase_frame_with_offset(0.7));
        assert!(
            !arc_cells_for(&quiet).is_empty(),
            "working keeps a bright arc"
        );
        assert_ne!(
            arc_cells_for(&quiet),
            arc_cells_for(&loud),
            "new audio data moves the working arc"
        );
        for status in [
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            assert_eq!(
                planet_cells_for(&quiet, status),
                planet_cells_for(&loud, status),
                "{status:?} planet geometry must ignore audio frames"
            );
        }
    }

    #[test]
    fn done_planet_keeps_a_dim_satellite_and_unknown_has_none() {
        let app = status_app(phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        let geometry_for = |status: AgentStatus| {
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
            planet_geometry(tile, collage_area(CANVAS), status, app.viz())
        };
        let (x, y) = geometry_for(AgentStatus::Done)
            .satellite
            .expect("a done planet keeps a satellite");
        assert_eq!(buf.cell((x, y)).unwrap().symbol(), SATELLITE_GLYPH);
        assert!(
            buf.cell((x, y))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::DIM),
            "the satellite stays dim"
        );
        assert!(
            geometry_for(AgentStatus::Unknown).satellite.is_none(),
            "unknown never gains a satellite"
        );
    }

    #[test]
    fn dense_planet_field_renders_one_selectable_body_per_agent() {
        let snaps: Vec<AgentSnapshot> = (0..80)
            .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
            .collect();
        let app = collage_app(snaps);
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], collage_area(CANVAS));
        assert_eq!(layout.tiles.len(), 80);
        let mut bodies = std::collections::HashSet::new();
        for tile in &layout.tiles {
            let geometry =
                planet_geometry(tile, collage_area(CANVAS), AgentStatus::Working, app.viz());
            assert!(!geometry.body.is_empty(), "every dense planet keeps a body");
            let (cx, cy, _, _) = ellipse_of(tile.rect);
            let center = (cx as u16, cy as u16);
            assert!(
                PLANET_GLYPHS.contains(&buf.cell(center).unwrap().symbol()),
                "dense planet center {center:?} must draw a planet glyph"
            );
            bodies.insert(center);
        }
        assert_eq!(bodies.len(), 80, "one distinct visible body per agent");
    }

    #[test]
    fn planets_do_not_change_phase_scope_cells_or_elapsed_time_behavior() {
        let working = render_collage(3, phase_frame(), vec![], false);
        let mut statuses_app = collage_app(vec![
            snap("ws", "p0", None, AgentStatus::Blocked),
            snap("ws", "p1", None, AgentStatus::Done),
            snap("ws", "p2", None, AgentStatus::Idle),
        ]);
        push_frame(&mut statuses_app, phase_frame());
        let t0 = Instant::now();
        let statuses = render_collage_for(&statuses_app, false, t0);
        assert_eq!(
            statuses,
            render_collage_for(&statuses_app, false, t0 + Duration::from_secs(9)),
            "elapsed time never changes the planet field"
        );

        // Scope cells outside every planet's bounds (rect plus the one-cell
        // ring overhang) must not care which statuses the planets carry.
        let reference = phase_frame();
        let layout = collage_layout(&agents(3), &reference, &[], collage_area(CANVAS));
        let outside_planets = |x: u16, y: u16| {
            !layout.tiles.iter().any(|tile| {
                let margin = Rect::new(
                    tile.rect.x.saturating_sub(1),
                    tile.rect.y.saturating_sub(1),
                    tile.rect.width + 2,
                    tile.rect.height + 2,
                );
                rect_contains(margin, x, y)
            })
        };
        let scope_cells = |buf: &Buffer| -> Vec<(u16, u16, String)> {
            let canvas = collage_area(CANVAS);
            let mut cells = Vec::new();
            for y in canvas.y..canvas.y + canvas.height {
                for x in canvas.x..canvas.x + canvas.width {
                    let symbol = buf.cell((x, y)).unwrap().symbol();
                    if (symbol == PRIMARY_TRACE_GLYPH || symbol == SECONDARY_TRACE_GLYPH)
                        && outside_planets(x, y)
                    {
                        cells.push((x, y, symbol.to_string()));
                    }
                }
            }
            cells
        };
        let unoccluded = scope_cells(&working);
        assert!(
            !unoccluded.is_empty(),
            "sanity: scope cells show around planets"
        );
        assert_eq!(
            unoccluded,
            scope_cells(&statuses),
            "planet status never changes the scope"
        );
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
    fn stale_and_low_power_freeze_phase_and_planet_geometry() {
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

        // A fresh snapshot still recolors the frozen planet's orbit in place.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Blocked)],
            now: Instant::now(),
        });
        let theme = Theme::for_name(ThemeName::Minimal);
        let recolored = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = collage_layout(app.active_agents(), captured, &[], collage_area(CANVAS));
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, collage_area(CANVAS), AgentStatus::Blocked, captured);
        let (x, y) = broken_ring(&geometry.ring, tile.seed)[0];
        assert_eq!(
            recolored.cell((x, y)).unwrap().style().fg,
            Some(theme.error),
            "the frozen orbit takes the fresh blocked color"
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
    fn state_ring_colors_come_from_the_theme() {
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
            let geometry = planet_geometry(tile, collage_area(CANVAS), status, app.viz());
            let ring = drawn_ring(&geometry, tile.seed, status);
            let (x, y) = ring[0];
            let style = buf.cell((x, y)).unwrap().style();
            assert_eq!(
                style.fg,
                Some(status_color(status, &theme)),
                "{status:?} orbit ring must take its theme color"
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
        assert_eq!(count_planet_cells(&buf), 0, "no agent planets render");
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
