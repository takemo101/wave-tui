//! Agent Pulse rendering: the tiny `● n active` summary and the full-screen,
//! music-reactive **Agent Planets** stage.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. The stage centers the same hierarchy as
//! Single View: an `Agent Planets · n active` heading, a title block with
//! the current ICY/station title over the exact Single View volume line,
//! the Dual Phase Scope field, and a footer. Behind the planets
//! the field plots two real played-audio phase portraits from the current
//! [`crate::model::VizFrame`] — paired samples on X/Y axes (stereo
//! left/right, or documented mono lags), never an amplitude-over-time
//! waveform — plus up to two dim phosphor-persistence layers from the real
//! prior frames in `App::viz_history()`. Every agent keeps one stable,
//! deterministically placed slot whose bounded audio-driven displacement
//! carries over from the Kinetic Collage; inside it the renderer draws a
//! planet body from one of four explicit disc masks — 7×5, 5×3, 3×3, or a
//! single cell — never a calculated rectangle/ellipse silhouette and never
//! a full-tile shadow. Each identity owns a stable Banded Worlds surface
//! (banded gas, ice cap, or cratered rock) painted with two theme spectrum
//! colors inside the mask; the surface is identity language only and never
//! encodes status.
//! Status is atmosphere language, and the thin atmosphere is the only
//! planet decoration: a glow on an explicit offset cycle outside the disc,
//! gapped one cell off the body, in the status color; cells that would
//! leave the tile or crowd the body gap are dropped rather than bent. The
//! atmosphere's animation state derives only from the played phase frame
//! plus the identity seed, never wall-clock — Working carries a bright
//! accent segment traveling around the ring, Blocked a short, weakly
//! irregular pulsing segment on the error color, Idle a slow regular
//! breathing brightness, Done a slow dim afterglow pulse, and Unknown
//! stays near-static neutral — and never any cross-like glyph. The
//! selected planet alone gains four corner focus brackets bounded to its
//! tile — decoration, never a hit target. Nothing moves from a timer:
//! identical frames render identical cells. A frame at or
//! below the silence threshold draws no trace or persistence at all —
//! analyzer silence carries non-empty all-zero traces that would otherwise
//! pile a point cluster at the field center — so silence stays calm, dim,
//! and still. Stale renders the reducer-captured final composition dimmed;
//! `--low-power` renders the App-captured first frame so trace, disc,
//! atmosphere, and bracket geometry stay frozen while state colors keep
//! refreshing.
//!
//! Mouse input flows through [`hit_test`], which shares [`collage_layout`]
//! and [`planet_geometry`] with rendering so a click resolves against
//! exactly the disc body cells that were drawn (scope, vignette,
//! atmosphere, brackets, and empty cells resolve nothing), and
//! returns only the read-only selection [`Action`]; the CLI event loop
//! owns applying it.
//!
//! Privacy: the stage shows the explicit Herdr agent `name` only. No pane
//! id, workspace id, cwd, or agent type is ever rendered. All colors come
//! from the active [`Theme`]; no palette values are added.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
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
/// Glyph for one thin status-atmosphere cell outside the body gap.
const ATMOSPHERE_GLYPH: &str = "▒";
/// Ring cells in the Working atmosphere's traveling accent segment.
const WORKING_SEGMENT: u64 = 3;
/// Ring cells in the Blocked atmosphere's short pulsing error segment.
const BLOCKED_SEGMENT: u64 = 2;
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

/// One placed agent slot: the index into `App::active_agents()`, its stable
/// identity seed and staggered base rectangle, the audio-transformed drawn
/// rectangle, and its energy.
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
/// phase layers and persistence come only from `frame` and `history` (most
/// recent first). Freezing — stale or low power — is done by the caller
/// handing in a captured frame/history, never by a flag here.
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

            CollageTile {
                index,
                seed,
                base_rect,
                rect,
                energy,
            }
        })
        .collect();

    CollageLayout { background, tiles }
}

// --- planet geometry --------------------------------------------------------

/// The four fixed, terminal-safe disc masks a planet body may use. Explicit
/// row masks — never an equation-derived rectangle/ellipse — keep every
/// planet unmistakably round and free of cross-like silhouettes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscMask {
    Large7x5,
    Medium5x3,
    Small3x3,
    Dot,
}

/// Explicit 7×5 disc rows; only non-space characters become body cells.
const LARGE_DISC: [&str; 5] = ["  ███  ", " █████ ", "███████", " █████ ", "  ███  "];
/// Explicit 5×3 disc rows.
const MEDIUM_DISC: [&str; 3] = [" ███ ", "█████", " ███ "];
/// Explicit 3×3 disc rows.
const SMALL_DISC: [&str; 3] = [" █ ", "███", " █ "];
/// The one-cell fallback disc for the densest fields.
const DOT_DISC: [&str; 1] = ["█"];

/// Explicit clockwise thin-atmosphere offset cycle around the 7×5 mask: an
/// octagon one gap cell outside the disc silhouette, as offsets from the
/// mask origin. Every cell keeps at least a one-cell gap off the disc body.
const LARGE_ATMOSPHERE: [(i32, i32); 16] = [
    (2, -2),
    (3, -2),
    (4, -2),
    (6, -1),
    (7, 0),
    (8, 2),
    (7, 4),
    (6, 5),
    (4, 6),
    (3, 6),
    (2, 6),
    (0, 5),
    (-1, 4),
    (-2, 2),
    (-1, 0),
    (0, -1),
];
/// Clockwise thin-atmosphere offsets around the 5×3 mask.
const MEDIUM_ATMOSPHERE: [(i32, i32); 12] = [
    (1, -2),
    (2, -2),
    (3, -2),
    (5, -1),
    (6, 1),
    (5, 3),
    (3, 4),
    (2, 4),
    (1, 4),
    (-1, 3),
    (-2, 1),
    (-1, -1),
];
/// Clockwise thin-atmosphere offsets around the 3×3 mask.
const SMALL_ATMOSPHERE: [(i32, i32); 8] = [
    (1, -2),
    (3, -1),
    (4, 1),
    (3, 3),
    (1, 4),
    (-1, 3),
    (-2, 1),
    (-1, -1),
];

impl DiscMask {
    /// The largest mask whose fixed footprint fits a `width`×`height` slot:
    /// dense fields fall through 7×5 → 5×3 → 3×3 → one cell.
    fn for_bound(width: u16, height: u16) -> DiscMask {
        if width >= 7 && height >= 5 {
            DiscMask::Large7x5
        } else if width >= 5 && height >= 3 {
            DiscMask::Medium5x3
        } else if width >= 3 && height >= 3 {
            DiscMask::Small3x3
        } else {
            DiscMask::Dot
        }
    }

    fn rows(self) -> &'static [&'static str] {
        match self {
            DiscMask::Large7x5 => &LARGE_DISC,
            DiscMask::Medium5x3 => &MEDIUM_DISC,
            DiscMask::Small3x3 => &SMALL_DISC,
            DiscMask::Dot => &DOT_DISC,
        }
    }

    fn width(self) -> u16 {
        self.rows()[0].chars().count() as u16
    }

    fn height(self) -> u16 {
        self.rows().len() as u16
    }
}

/// The clockwise thin-atmosphere offsets for `mask`; the one-cell disc
/// cannot keep the required body gap, so it keeps no decoration at all.
fn atmosphere_cycle(mask: DiscMask) -> &'static [(i32, i32)] {
    match mask {
        DiscMask::Large7x5 => &LARGE_ATMOSPHERE,
        DiscMask::Medium5x3 => &MEDIUM_ATMOSPHERE,
        DiscMask::Small3x3 => &SMALL_ATMOSPHERE,
        DiscMask::Dot => &[],
    }
}

/// One placed disc: its chosen mask, its top-left mask origin (kept signed
/// so decoration overhang can clip at the field edge), and its body cells
/// inside `area`.
struct DiscGeometry {
    mask: DiscMask,
    origin: (i32, i32),
    body: Vec<(u16, u16)>,
}

/// Choose the largest mask fitting `bound`, center it inside `bound`, and
/// convert only non-space mask characters into body cells clipped to `area`.
fn disc_geometry(bound: Rect, area: Rect) -> DiscGeometry {
    let mask = DiscMask::for_bound(bound.width, bound.height);
    let origin = (
        bound.x as i32 + (bound.width as i32 - mask.width() as i32) / 2,
        bound.y as i32 + (bound.height as i32 - mask.height() as i32) / 2,
    );
    let mut body = Vec::new();
    for (row, cells) in mask.rows().iter().enumerate() {
        for (col, glyph) in cells.chars().enumerate() {
            if glyph == ' ' {
                continue;
            }
            let x = origin.0 + col as i32;
            let y = origin.1 + row as i32;
            if x >= 0 && y >= 0 && rect_contains(area, x as u16, y as u16) {
                body.push((x as u16, y as u16));
            }
        }
    }
    DiscGeometry { mask, origin, body }
}

/// One thin status-atmosphere cell; `accent` marks membership in the
/// status segment — Working's traveling accent or Blocked's short pulse —
/// selected by the played phase signature plus the identity seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AtmosphereCell {
    cell: (u16, u16),
    accent: bool,
}

/// One selection focus bracket: a corner cell of the selected planet's
/// tile and its corner glyph. Decorative only, never a hit target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusBracket {
    cell: (u16, u16),
    glyph: &'static str,
}

/// One agent's planet in field cells, derived purely from its tile, status,
/// selection, and the current phase frame — every field follows the
/// audio-transformed `rect`. Only the atmosphere's segment and lift state
/// consult phase data — every other cell keeps still geometry across audio
/// frames.
struct PlanetGeometry {
    /// The fixed mask this planet's slot chose.
    #[cfg_attr(not(test), allow(dead_code))]
    mask: DiscMask,
    body: Vec<(u16, u16)>,
    craters: Vec<(u16, u16)>,
    /// The thin status atmosphere outside the one-cell body gap — the only
    /// planet decoration.
    atmosphere: Vec<AtmosphereCell>,
    /// Whether the atmosphere's breathing/pulse treatment sits in its
    /// bright half for this frame: Idle's regular breathing, Done's slow
    /// afterglow pulse, and Blocked's weak segment pulse. Always false for
    /// Unknown and at silence.
    atmosphere_lift: bool,
    /// Four corner focus brackets; empty unless this planet is selected.
    brackets: Vec<FocusBracket>,
    /// Only visible disc body cells select a planet; atmosphere and
    /// brackets are decorative.
    hit_cells: Vec<(u16, u16)>,
}

/// The three Banded Worlds surface families. A family is stable private
/// identity language — never a status, audio, time, or selection signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanetSurface {
    BandedGas,
    IceCap,
    CrateredRock,
}

/// The two stable spectrum-gradient positions a planet identity owns: the
/// body paints `base_position`, the surface pattern paints
/// `accent_position`. Both resolve through the active theme's
/// `spectrum_color`, so no fixed palette value ever appears.
#[derive(Clone, Copy)]
struct PlanetPalette {
    base_position: f32,
    accent_position: f32,
}

/// The stable identity-seeded surface family.
fn planet_surface(seed: u64) -> PlanetSurface {
    match seed % 3 {
        0 => PlanetSurface::BandedGas,
        1 => PlanetSurface::IceCap,
        _ => PlanetSurface::CrateredRock,
    }
}

/// The stable identity-seeded palette pair: distinct base and accent
/// positions on the theme spectrum gradient.
fn planet_palette(seed: u64) -> PlanetPalette {
    const POSITIONS: [f32; 4] = [0.16, 0.38, 0.62, 0.84];
    let base = seed as usize % POSITIONS.len();
    PlanetPalette {
        base_position: POSITIONS[base],
        accent_position: POSITIONS[(base + 1 + ((seed >> 4) as usize % 2)) % POSITIONS.len()],
    }
}

/// The body cells a surface family paints in the accent color: gas bands,
/// the ice cap's polar rows, or the rock world's crater marks. A pure
/// function of the already-derived body geometry and identity seed, so the
/// pattern rides the bounded audio transform without ever morphing; bodies
/// too small for crater detail drop the surface pattern before the body.
fn surface_cells(surface: PlanetSurface, geometry: &PlanetGeometry, seed: u64) -> Vec<(u16, u16)> {
    if geometry.body.len() < CRATER_MIN_BODY {
        return Vec::new();
    }
    let rows = || {
        let top = geometry.body.iter().map(|&(_, y)| y).min().unwrap_or(0);
        let bottom = geometry.body.iter().map(|&(_, y)| y).max().unwrap_or(0);
        (top, bottom - top + 1)
    };
    match surface {
        PlanetSurface::CrateredRock => geometry.craters.clone(),
        PlanetSurface::BandedGas => {
            let (top, _) = rows();
            geometry
                .body
                .iter()
                .copied()
                .filter(|&(_, y)| ((y - top) as u64 + seed).is_multiple_of(3))
                .collect()
        }
        PlanetSurface::IceCap => {
            let (top, height) = rows();
            let cap_rows = height.div_ceil(3);
            geometry
                .body
                .iter()
                .copied()
                .filter(|&(_, y)| y < top + cap_rows)
                .collect()
        }
    }
}

/// Admit one decorative offset: the cell must stay inside the field and
/// the planet's tile and keep at least a one-cell gap off every body cell.
/// Offsets that fail are dropped rather than moved, so decoration never
/// crowds the disc or leaks outside its tile.
fn decorative_cell(
    disc: &DiscGeometry,
    tile: &CollageTile,
    area: Rect,
    dx: i32,
    dy: i32,
) -> Option<(u16, u16)> {
    let x = disc.origin.0 + dx;
    let y = disc.origin.1 + dy;
    if x < 0 || y < 0 {
        return None;
    }
    let cell = (x as u16, y as u16);
    (rect_contains(area, cell.0, cell.1)
        && rect_contains(tile.rect, cell.0, cell.1)
        && !disc
            .body
            .iter()
            .any(|&(bx, by)| (bx as i32 - x).abs() <= 1 && (by as i32 - y).abs() <= 1))
    .then_some(cell)
}

/// The thin status atmosphere — the only planet decoration: the mask's
/// full atmosphere cycle with the out-of-tile and gap-crowding cells
/// dropped, so the ring cells themselves never move. Every animation state
/// derives only from the played phase signature plus the identity seed —
/// never wall-clock — and a silent frame rests every treatment. Working
/// selects a traveling accent segment; Blocked a short segment whose hop
/// and weak pulse come from a scrambled phase, keeping it irregular where
/// Working travels; Idle a slow regular breathing lift; Done a slow
/// afterglow pulse; Unknown stays near-static with neither.
fn atmosphere_ring(
    tile: &CollageTile,
    disc: &DiscGeometry,
    status: AgentStatus,
    frame: &VizFrame,
    area: Rect,
) -> (Vec<AtmosphereCell>, bool) {
    let cells: Vec<(u16, u16)> = atmosphere_cycle(disc.mask)
        .iter()
        .filter_map(|&(dx, dy)| decorative_cell(disc, tile, area, dx, dy))
        .collect();
    if cells.is_empty() {
        return (Vec::new(), false);
    }
    let silent = tile.energy <= SILENCE_ENERGY;
    let len = cells.len() as u64;
    let phase = phase_signature(&frame.primary_phase).wrapping_add(tile.seed);
    let segment = if silent {
        None
    } else {
        match status {
            AgentStatus::Working => Some((phase % len, WORKING_SEGMENT)),
            AgentStatus::Blocked => Some(((phase ^ (phase >> 7)) % len, BLOCKED_SEGMENT)),
            _ => None,
        }
    };
    let lift = !silent
        && match status {
            AgentStatus::Working => true,
            AgentStatus::Idle => phase % 6 < 3,
            AgentStatus::Blocked => (phase ^ (phase >> 5)) % 5 < 2,
            AgentStatus::Done => phase % 9 < 3,
            AgentStatus::Unknown => false,
        };
    let atmosphere = cells
        .into_iter()
        .enumerate()
        .map(|(index, cell)| AtmosphereCell {
            cell,
            accent: segment
                .is_some_and(|(start, span)| (index as u64 + len - start) % len < span.min(len)),
        })
        .collect();
    (atmosphere, lift)
}

/// The selected planet's four corner focus brackets: the tile's own corner
/// cells, so the brackets surround the allocated disc area, bounded to the
/// tile by construction. A corner that would crowd the body gap, leave the
/// field, or land on an atmosphere cell is dropped.
fn focus_brackets(
    tile: &CollageTile,
    disc: &DiscGeometry,
    atmosphere: &[AtmosphereCell],
    area: Rect,
) -> Vec<FocusBracket> {
    let right = (tile.rect.x + tile.rect.width - 1) as i32;
    let bottom = (tile.rect.y + tile.rect.height - 1) as i32;
    let corners = [
        (tile.rect.x as i32, tile.rect.y as i32, "┌"),
        (right, tile.rect.y as i32, "┐"),
        (tile.rect.x as i32, bottom, "└"),
        (right, bottom, "┘"),
    ];
    corners
        .into_iter()
        .filter_map(|(x, y, glyph)| {
            let cell = decorative_cell(disc, tile, area, x - disc.origin.0, y - disc.origin.1)?;
            let occupied = atmosphere.iter().any(|glow| glow.cell == cell);
            (!occupied).then_some(FocusBracket { cell, glyph })
        })
        .collect()
}

/// Derive one planet's body, thin status atmosphere, and — for the
/// selected planet only — focus brackets from its tile. Craters and
/// atmosphere cells stay identity-stable; only the atmosphere's segment
/// and lift state follow the played phase frame.
fn planet_geometry(
    tile: &CollageTile,
    area: Rect,
    status: AgentStatus,
    frame: &VizFrame,
    selected: bool,
) -> PlanetGeometry {
    let disc = disc_geometry(tile.rect, area);
    let body = disc.body.clone();
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
    let (atmosphere, atmosphere_lift) = atmosphere_ring(tile, &disc, status, frame, area);
    let brackets = if selected {
        focus_brackets(tile, &disc, &atmosphere, area)
    } else {
        Vec::new()
    };
    PlanetGeometry {
        mask: disc.mask,
        body: body.clone(),
        craters,
        atmosphere,
        atmosphere_lift,
        brackets,
        hit_cells: body,
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

/// The centered Agent Planets stage partitions: heading, title block
/// (current title with the Single View volume line beneath it), scope/planet
/// field, and footer rows.
struct AgentStageLayout {
    heading: Rect,
    title_block: Rect,
    field: Rect,
    footer: Rect,
}

/// Partition the stage. The field takes every flexible row; the title block
/// keeps a second title row only when the terminal is tall enough, so small
/// stages retain a positive field.
fn agent_stage_layout(area: Rect) -> AgentStageLayout {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if area.height >= 15 { 3 } else { 2 }),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    AgentStageLayout {
        heading: rows[0],
        title_block: rows[1],
        field: rows[2],
        footer: rows[3],
    }
}

/// Whether (`x`, `y`) falls inside `rect`.
fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

// --- hit testing ------------------------------------------------------------

/// Pure mouse hit test for the Dual Phase Scope canvas.
///
/// Maps a click on a planet's drawn disc body cells — the exact
/// [`planet_geometry`] hit cells the renderer draws — to the read-only
/// [`Action::SelectAgent`]; returns `None` whenever the canvas is closed, the
/// integration is hidden, the connection is stale or unavailable, Signal View
/// is active, or the click misses every planet body. Scope phase, vignette,
/// atmosphere, bracket, and empty cells resolve nothing. Overlapping planets resolve
/// topmost-first, with the selected planet in front, matching draw order.
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
    let canvas = agent_stage_layout(area).field;
    let layout = collage_layout(agents, frame, &[], canvas);
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
        let Some(view) = agents.get(tile.index) else {
            continue;
        };
        let geometry = planet_geometry(tile, canvas, view.status, frame, false);
        if geometry.hit_cells.contains(&(column, row)) {
            return Some(Action::SelectAgent(view.id.clone()));
        }
    }
    None
}

// --- canvas rendering -------------------------------------------------------

/// Render the full-screen Agent Planets stage over the composed layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the centered stage chrome
/// (heading, the current ICY/station title with the exact Single View
/// volume line beneath it, and footer) and, inside the stage field, the
/// breathing vignette, the phosphor-persistence and dual phase-trace
/// layers, and each planet in ordered passes — thin status atmosphere,
/// disc-mask body with its Banded Worlds surface, then the selected
/// planet's four corner focus brackets — with the selected planet drawn
/// last. Stale
/// renders the reducer-captured final composition dimmed under a
/// `reconnecting` note; Unavailable hides the field and tags behind calm
/// copy; `--low-power` renders the App-captured first frame so geometry
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
    render_agent_planets_stage(app, theme, low_power, area, buf);
}

/// Draw the stage chrome and the scope/planet field.
fn render_agent_planets_stage(
    app: &App,
    theme: &Theme,
    low_power: bool,
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
    render_stage_footer(theme, stage.footer, buf);

    if connection == AgentPulseConnection::Unavailable {
        center_copy(buf, stage.field, "agents · unavailable · retrying", muted);
        return;
    }

    let field = stage.field;
    // Geometry-source precedence: stale always wins with the display captured
    // by the reducer at the Connected→Stale edge; otherwise `--low-power`
    // renders the App-captured first frame so no trace, disc, atmosphere,
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
    let layout = collage_layout(agents, frame, history, field);

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
        return;
    }

    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));

    // One geometry per tile, shared by every planet pass; the selected
    // planet alone derives its focus brackets.
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
                frame,
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
        render_planet(buf, tile, geometry, agents, theme, stale, low_power);
    }
    if let Some(selected) = selected_index {
        if let Some((tile, geometry)) = layout
            .tiles
            .iter()
            .zip(&geometries)
            .find(|(tile, _)| tile.index == selected)
        {
            render_planet(buf, tile, geometry, agents, theme, stale, low_power);
        }
    }

    render_agent_details_modal(app, theme, stale, field, buf);
}

/// Centered stage heading: `Agent Planets · n active` in the same Title
/// Case presentation as Single View, with the quiet reconnect note appended
/// while the connection is stale.
fn render_stage_heading(theme: &Theme, count: usize, stale: bool, area: Rect, buf: &mut Buffer) {
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
fn stage_primary_title(app: &App) -> String {
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
fn render_stage_title_block(app: &App, theme: &Theme, stale: bool, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut style = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);
    if stale {
        style = style.add_modifier(Modifier::DIM);
    }
    let mut title = super::title_lines(&stage_primary_title(app), area.width);
    title.truncate((area.height as usize).saturating_sub(1).max(1));
    let mut lines: Vec<Line> = title
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect();
    if area.height >= 2 {
        lines.push(super::signal_view_volume_line(app, theme, area.width));
    }
    Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(theme.base_style())
        .render(area, buf);
}

/// Centered restrained footer: selection, player, and close hints. `z` is
/// deliberately not advertised — Single View is not a stage action.
fn render_stage_footer(theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    Paragraph::new("Tab/↑↓/click select · Enter details · Space play · a/Esc close")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted))
        .render(area, buf);
}

/// Render the selected planet's compact, read-only details record.
fn render_agent_details_modal(
    app: &App,
    theme: &Theme,
    stale: bool,
    field: Rect,
    buf: &mut Buffer,
) {
    let Some(details) = app.selected_agent_details() else {
        return;
    };
    if field.width < 12 || field.height < 5 {
        return;
    }
    let status = app
        .selected_agent()
        .map(|agent| status_label(agent.status))
        .unwrap_or("unknown");
    let mut rows = Vec::new();
    if let Some(name) = &details.name {
        rows.push(("name", name.as_str()));
    }
    if let Some(agent) = &details.agent {
        rows.push(("agent", agent.as_str()));
    }
    rows.push(("status", status));
    if let Some(activity) = &details.activity {
        rows.push(("activity", activity.as_str()));
    }

    let width = field.width.clamp(12, 48);
    let height = ((rows.len() + 3) as u16).min(field.height).max(5);
    let area = Rect::new(
        field.x + field.width.saturating_sub(width) / 2,
        field.y + field.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (label, value) in rows {
        let prefix = format!("{label}: ");
        if label == "activity" {
            let inner_width = area.width.saturating_sub(2) as usize;
            let first_width = inner_width.saturating_sub(prefix.chars().count()).max(1);
            let second_width = inner_width.saturating_sub(prefix.chars().count()).max(1);
            let chars: Vec<char> = value.chars().collect();
            let first: String = chars.iter().take(first_width).collect();
            lines.push(Line::from(vec![
                Span::styled(prefix.clone(), Style::default().fg(theme.muted)),
                Span::styled(first, Style::default().fg(theme.foreground)),
            ]));
            if chars.len() > first_width {
                let remaining = &chars[first_width..];
                let truncated = remaining.len() > second_width;
                let keep = second_width.saturating_sub(usize::from(truncated));
                let mut second: String = remaining.iter().take(keep).collect();
                if truncated {
                    second.push('…');
                }
                lines.push(Line::from(vec![
                    Span::styled(
                        " ".repeat(prefix.chars().count()),
                        Style::default().fg(theme.muted),
                    ),
                    Span::styled(second, Style::default().fg(theme.foreground)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(theme.muted)),
                Span::styled(value.to_string(), Style::default().fg(theme.foreground)),
            ]));
        }
    }
    let mut title = Style::default().fg(theme.accent);
    if stale {
        title = title.add_modifier(Modifier::DIM);
    }
    Clear.render(area, buf);
    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(title)
                .title(Span::styled(
                    if stale {
                        " Agent details · reconnecting "
                    } else {
                        " Agent details "
                    },
                    title,
                )),
        )
        .style(if stale {
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(theme.foreground)
        })
        .render(area, buf);
}

/// Draw one agent planet in order: its thin status atmosphere in the
/// status color — Working's traveling accent segment bolds, Blocked's
/// short segment stands out of a dim error ring and weakly pulses, Idle
/// breathes and Done slowly pulses between dim and plain muted, Unknown
/// stays dim and near-static — then the disc-mask body with its stable
/// Banded Worlds surface, and — for the selected planet only — its four
/// corner focus brackets drawn as a foreground line color in the theme
/// selection accent, never a painted selection background. Silence and
/// low power rest every brightening so frozen frames stay calm. Selection
/// never restyles the atmosphere; the brackets are the only focus
/// treatment.
fn render_planet(
    buf: &mut Buffer,
    tile: &CollageTile,
    geometry: &PlanetGeometry,
    agents: &[AgentView],
    theme: &Theme,
    stale: bool,
    low_power: bool,
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

    let animate = !silent && !low_power;
    for glow in &geometry.atmosphere {
        let mut style = Style::default().fg(status_color(view.status, theme));
        match view.status {
            AgentStatus::Working => {
                if glow.accent && animate {
                    style = style.add_modifier(Modifier::BOLD);
                }
            }
            AgentStatus::Blocked => {
                if !glow.accent {
                    style = style.add_modifier(Modifier::DIM);
                } else if geometry.atmosphere_lift && animate {
                    style = style.add_modifier(Modifier::BOLD);
                }
            }
            AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => {
                if !(geometry.atmosphere_lift && animate) {
                    style = style.add_modifier(Modifier::DIM);
                }
            }
        }
        buf.set_string(
            glow.cell.0,
            glow.cell.1,
            ATMOSPHERE_GLYPH,
            own_emphasis(with_stale(quiet_dim(style), stale)),
        );
    }

    // Banded Worlds surface: identity chooses the family and the theme
    // spectrum pair; status never colors the body — state stays on the
    // atmosphere.
    let surface = planet_surface(tile.seed);
    let palette = planet_palette(tile.seed);
    let paint = |color: Color| {
        let mut style = Style::default().fg(color);
        if matches!(view.status, AgentStatus::Done | AgentStatus::Unknown) {
            style = style.add_modifier(Modifier::DIM);
        }
        own_emphasis(with_stale(quiet_dim(style), stale))
    };
    let base = paint(theme.spectrum_color(palette.base_position));
    let accent = paint(theme.spectrum_color(palette.accent_position));
    let accent_cells: HashSet<(u16, u16)> = surface_cells(surface, geometry, tile.seed)
        .into_iter()
        .collect();
    for &(x, y) in &geometry.body {
        if accent_cells.contains(&(x, y)) {
            let glyph = if surface == PlanetSurface::CrateredRock {
                CRATER_GLYPH
            } else {
                PLANET_BODY_GLYPH
            };
            buf.set_string(x, y, glyph, accent);
        } else {
            buf.set_string(x, y, PLANET_BODY_GLYPH, base);
        }
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

/// Planet cells fully own their emphasis: painting over a dim scope cell
/// (vignette, phosphor persistence) must not inherit its modifiers, because
/// Ratatui merges styles per cell. Subtract exactly the emphasis modifiers
/// the composed style did not add itself.
fn own_emphasis(style: Style) -> Style {
    style.remove_modifier((Modifier::DIM | Modifier::BOLD).difference(style.add_modifier))
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

fn with_stale(style: Style, stale: bool) -> Style {
    if stale {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// Centered single-line copy for the empty/unavailable states.
fn center_copy(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height / 2;
    buf.set_stringn(x, y, text, area.width as usize, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::audio::AudioEvent;
    use crate::catalog::Catalog;
    use crate::herdr::{AgentDetails, AgentId, AgentSnapshot};
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

    /// The stage's scope/planet field on the standard test canvas.
    fn stage_field() -> Rect {
        agent_stage_layout(CANVAS).field
    }

    /// Glyphs only agent planets (bodies, craters, and status atmospheres)
    /// may use. `·` stays excluded: the vignette, phosphor persistence,
    /// and copy separators share it.
    const PLANET_GLYPHS: [&str; 3] = [PLANET_BODY_GLYPH, CRATER_GLYPH, ATMOSPHERE_GLYPH];

    fn view(workspace: &str, pane: &str, status: AgentStatus) -> AgentView {
        AgentView {
            id: AgentId::new(workspace, pane),
            details: AgentDetails::default(),
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
            details: AgentDetails {
                name: name.map(str::to_string),
                agent: None,
                activity: None,
            },
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

    /// One unnamed agent whose raw ids and status label would be
    /// recognizable if a fallback side tag ever leaked them.
    fn app_with_only_unnamed_agent() -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("workspace-1", "pane-1", None, AgentStatus::Working)],
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

    fn count_primary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('•').count()
    }

    fn count_secondary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('◦').count()
    }

    /// Cells drawn with agent-planet glyphs (bodies, craters, or
    /// atmospheres) inside the stage field, so chrome rows (heading, title
    /// block, and footer) never count as planets.
    fn count_planet_cells(buf: &Buffer) -> usize {
        field_cells(buf)
            .iter()
            .filter(|(_, _, symbol)| PLANET_GLYPHS.contains(&symbol.as_str()))
            .count()
    }

    /// Every non-blank cell of the stage field — vignette, phase layers,
    /// planets, and tags — as `(x, y, symbol)`, so tests can compare
    /// whole-field geometry between renders.
    fn field_cells(buf: &Buffer) -> Vec<(u16, u16, String)> {
        let canvas = agent_stage_layout(*buf.area()).field;
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

    /// The selectable disc body cells of one laid-out tile — exactly what
    /// `planet_geometry` exposes as `hit_cells`. Independent of the phase
    /// frame, status, and selection: decoration never joins the hit set.
    fn tile_hit_cells(tile: &CollageTile, canvas: Rect) -> Vec<(u16, u16)> {
        planet_geometry(tile, canvas, AgentStatus::Working, &phase_frame(), false).hit_cells
    }

    fn cell_text(buf: &Buffer, x: u16, y: u16) -> String {
        buf.cell((x, y)).unwrap().symbol().to_string()
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
        // The persistence layer plots the prior frame's primary phase pairs;
        // some of those cells must show the dot only when history exists.
        let persistence = phase_cells(&older_phase_frame().primary_phase, stage_field());
        let grown = persistence.iter().any(|cell| {
            cell_text(&with, cell.x, cell.y) == PERSISTENCE_GLYPH
                && cell_text(&without, cell.x, cell.y) != PERSISTENCE_GLYPH
        });
        assert!(grown, "a real history frame grows dim persistence dots");
    }

    // --- status atmosphere and focus brackets --------------------------------

    /// The laid-out tile for one agent alone in `area` under `frame`.
    fn tile_for(agent: &AgentView, area: Rect, frame: &VizFrame) -> CollageTile {
        collage_layout(std::slice::from_ref(agent), frame, &[], area)
            .tiles
            .remove(0)
    }

    /// A hand-built tile roomy enough for the complete atmosphere
    /// cycle: an 18×9 slot holding the centered 7×5 disc with
    /// two rows and five columns of margin, well inside the area.
    fn roomy_tile(area: Rect) -> CollageTile {
        let rect = Rect::new(area.x + 10, area.y + 10, 18, 9);
        CollageTile {
            index: 0,
            seed: 21,
            base_rect: rect,
            rect,
            energy: 0.5,
        }
    }

    /// Whether `cell` keeps at least a one-cell gap off every body cell.
    fn gapped_from_body(geometry: &PlanetGeometry, cell: (u16, u16)) -> bool {
        geometry.body.iter().all(|&(bx, by)| {
            (bx as i32 - cell.0 as i32).abs() > 1 || (by as i32 - cell.1 as i32).abs() > 1
        })
    }

    #[test]
    fn stable_identity_produces_a_round_body_craters_and_gapped_atmosphere() {
        let area = Rect::new(0, 0, 120, 36);
        let agent = view("work-a", "pane-1", AgentStatus::Working);
        let first = planet_geometry(
            &tile_for(&agent, area, &phase_frame()),
            area,
            AgentStatus::Working,
            &phase_frame(),
            false,
        );
        assert!(!first.body.is_empty(), "the live body renders cells");
        assert!(!first.atmosphere.is_empty(), "a roomy slot keeps a glow");
        assert_eq!(first.hit_cells, first.body, "only body cells select");
        assert!(first
            .atmosphere
            .iter()
            .all(|glow| gapped_from_body(&first, glow.cell)));
    }

    /// The atmosphere ring cells of `status` on the roomy tile under
    /// `offset` audio, so animation tests can separate the still ring from
    /// its moving treatment.
    fn roomy_atmosphere(status: AgentStatus, offset: f32) -> PlanetGeometry {
        let area = Rect::new(0, 0, 120, 36);
        planet_geometry(
            &roomy_tile(area),
            area,
            status,
            &phase_frame_with_offset(offset),
            false,
        )
    }

    /// The accent-segment cells of `geometry` in ring order.
    fn accent_cells(geometry: &PlanetGeometry) -> Vec<(u16, u16)> {
        geometry
            .atmosphere
            .iter()
            .filter(|glow| glow.accent)
            .map(|glow| glow.cell)
            .collect()
    }

    /// A deterministic spread of audio-frame offsets for treatment
    /// searches: enough distinct frames that every duty cycle shows both
    /// halves.
    fn offset_sweep() -> impl Iterator<Item = f32> {
        (1..=24).map(|step| step as f32 * 0.13)
    }

    #[test]
    fn atmosphere_is_the_only_decoration_and_its_ring_never_moves() {
        for status in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            let first = roomy_atmosphere(status, 0.2);
            let later = roomy_atmosphere(status, 0.7);
            assert!(
                !first.atmosphere.is_empty(),
                "{status:?} keeps a visible atmosphere"
            );
            let ring = |geometry: &PlanetGeometry| -> Vec<(u16, u16)> {
                geometry.atmosphere.iter().map(|glow| glow.cell).collect()
            };
            assert_eq!(
                ring(&first),
                ring(&later),
                "{status:?} ring cells never move with audio"
            );
        }
    }

    #[test]
    fn working_accent_segment_travels_only_with_the_played_phase_frame() {
        let first = roomy_atmosphere(AgentStatus::Working, 0.2);
        let again = roomy_atmosphere(AgentStatus::Working, 0.2);
        assert_eq!(
            first.atmosphere, again.atmosphere,
            "identical frames keep an identical segment"
        );
        let later = roomy_atmosphere(AgentStatus::Working, 0.7);
        assert_eq!(accent_cells(&first).len(), WORKING_SEGMENT as usize);
        assert_eq!(accent_cells(&later).len(), WORKING_SEGMENT as usize);
        assert_ne!(
            accent_cells(&first),
            accent_cells(&later),
            "new played audio moves the accent segment"
        );

        let area = Rect::new(0, 0, 120, 36);
        let mut shifted_tile = roomy_tile(area);
        shifted_tile.seed += 1;
        let shifted = planet_geometry(
            &shifted_tile,
            area,
            AgentStatus::Working,
            &phase_frame_with_offset(0.2),
            false,
        );
        assert_ne!(
            accent_cells(&first),
            accent_cells(&shifted),
            "the identity seed fixes each planet's segment phase"
        );
    }

    #[test]
    fn idle_and_done_breathe_with_audio_while_unknown_stays_static() {
        for status in [AgentStatus::Idle, AgentStatus::Done] {
            let lifts: HashSet<bool> = offset_sweep()
                .map(|offset| roomy_atmosphere(status, offset).atmosphere_lift)
                .collect();
            assert_eq!(
                lifts,
                HashSet::from([false, true]),
                "{status:?} breathing takes both halves across played audio"
            );
            assert!(
                offset_sweep()
                    .all(|offset| accent_cells(&roomy_atmosphere(status, offset)).is_empty()),
                "{status:?} never carries an accent segment"
            );
        }

        let reference = roomy_atmosphere(AgentStatus::Unknown, 0.2);
        for offset in offset_sweep() {
            let unknown = roomy_atmosphere(AgentStatus::Unknown, offset);
            assert_eq!(
                unknown.atmosphere, reference.atmosphere,
                "unknown atmosphere ignores played audio"
            );
            assert!(!unknown.atmosphere_lift, "unknown never lifts");
            assert!(accent_cells(&unknown).is_empty());
        }
    }

    #[test]
    fn silence_rests_every_atmosphere_treatment() {
        let area = Rect::new(0, 0, 120, 36);
        let mut tile = roomy_tile(area);
        tile.energy = 0.0;
        for status in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            let geometry = planet_geometry(&tile, area, status, &silent_phase_frame(), false);
            assert!(
                !geometry.atmosphere.is_empty(),
                "{status:?} keeps its still ring at silence"
            );
            assert!(
                accent_cells(&geometry).is_empty(),
                "{status:?} keeps no accent segment at silence"
            );
            assert!(
                !geometry.atmosphere_lift,
                "{status:?} keeps no lift at silence"
            );
        }
    }

    #[test]
    fn selected_planets_keep_four_bounded_corner_brackets() {
        let area = Rect::new(0, 0, 120, 36);
        let tile = roomy_tile(area);
        let unselected = planet_geometry(&tile, area, AgentStatus::Working, &phase_frame(), false);
        assert!(
            unselected.brackets.is_empty(),
            "brackets exist only for the selected planet"
        );

        let selected = planet_geometry(&tile, area, AgentStatus::Working, &phase_frame(), true);
        assert_eq!(selected.brackets.len(), 4, "four corner brackets");
        let glyphs: HashSet<&str> = selected
            .brackets
            .iter()
            .map(|bracket| bracket.glyph)
            .collect();
        assert_eq!(
            glyphs,
            HashSet::from(["┌", "┐", "└", "┘"]),
            "one bracket per corner glyph"
        );
        let corners: HashSet<(u16, u16)> = HashSet::from([
            (tile.rect.x, tile.rect.y),
            (tile.rect.x + tile.rect.width - 1, tile.rect.y),
            (tile.rect.x, tile.rect.y + tile.rect.height - 1),
            (
                tile.rect.x + tile.rect.width - 1,
                tile.rect.y + tile.rect.height - 1,
            ),
        ]);
        for bracket in &selected.brackets {
            assert!(
                corners.contains(&bracket.cell),
                "brackets sit on the tile corners, bounded to the tile"
            );
        }
        assert_eq!(
            selected.hit_cells, unselected.hit_cells,
            "selection never grows the hit set"
        );
    }

    #[test]
    fn decorative_cells_stay_gapped_inside_the_tile_and_out_of_hit_cells() {
        let area = Rect::new(0, 0, 120, 36);
        let tile = roomy_tile(area);
        for status in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            let geometry = planet_geometry(&tile, area, status, &phase_frame(), true);
            let hit: HashSet<(u16, u16)> = geometry.hit_cells.iter().copied().collect();
            let decorative: Vec<(u16, u16)> = geometry
                .atmosphere
                .iter()
                .map(|glow| glow.cell)
                .chain(geometry.brackets.iter().map(|bracket| bracket.cell))
                .collect();
            assert!(!decorative.is_empty(), "{status:?} keeps decoration");
            for cell in decorative {
                assert!(
                    rect_contains(tile.rect, cell.0, cell.1),
                    "{status:?} decorative cell {cell:?} stays inside the tile"
                );
                assert!(
                    gapped_from_body(&geometry, cell),
                    "{status:?} decorative cell {cell:?} keeps the body gap"
                );
                assert!(
                    !hit.contains(&cell),
                    "{status:?} decorative cell {cell:?} never joins hit_cells"
                );
            }
        }
    }

    #[test]
    fn blocked_atmosphere_keeps_a_short_irregular_error_segment() {
        let mut app = collage_app(vec![snap("ws", "b", Some("b"), AgentStatus::Blocked)]);
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let field_text: String = field_cells(&buf)
            .into_iter()
            .map(|(_, _, symbol)| symbol)
            .collect();
        for cross in ['×', '╳', '╲', '╱', '✕', '+'] {
            assert!(!field_text.contains(cross));
        }
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        let blocked = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Blocked,
            app.viz(),
            false,
        );
        let theme = Theme::for_name(ThemeName::Minimal);
        assert!(!blocked.atmosphere.is_empty());
        assert_eq!(
            accent_cells(&blocked).len(),
            BLOCKED_SEGMENT as usize,
            "blocked keeps its short segment"
        );
        for glow in &blocked.atmosphere {
            let cell = buf.cell(glow.cell).unwrap();
            assert_eq!(cell.symbol(), ATMOSPHERE_GLYPH);
            assert_eq!(
                cell.style().fg,
                Some(theme.error),
                "the whole blocked atmosphere stays on the error color"
            );
            assert_eq!(
                cell.style().add_modifier.contains(Modifier::DIM),
                !glow.accent,
                "the segment stands out of the dim error ring"
            );
        }

        // The segment hops and weakly pulses with new played audio —
        // deterministically, never from a timer.
        assert_eq!(
            accent_cells(&roomy_atmosphere(AgentStatus::Blocked, 0.2)),
            accent_cells(&roomy_atmosphere(AgentStatus::Blocked, 0.2)),
            "identical frames keep an identical segment"
        );
        let positions: HashSet<Vec<(u16, u16)>> = offset_sweep()
            .map(|offset| accent_cells(&roomy_atmosphere(AgentStatus::Blocked, offset)))
            .collect();
        assert!(
            positions.len() > 1,
            "the blocked segment hops across played audio frames"
        );
        let pulses: HashSet<bool> = offset_sweep()
            .map(|offset| roomy_atmosphere(AgentStatus::Blocked, offset).atmosphere_lift)
            .collect();
        assert_eq!(
            pulses,
            HashSet::from([false, true]),
            "the blocked segment pulse takes both halves across played audio"
        );
    }

    #[test]
    fn stage_renders_no_old_ring_arc_or_satellite_glyphs() {
        let mut app = status_app(phase_frame());
        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let field: String = field_cells(&buf)
            .into_iter()
            .map(|(_, _, symbol)| symbol)
            .collect();
        for glyph in ["∘", "●", "▪"] {
            assert!(
                !field.contains(glyph),
                "old ring/arc/satellite glyph {glyph} may not render"
            );
        }
    }

    #[test]
    fn no_decorative_orbit_particle_cells_render_inside_the_tile() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Idle)]);
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        let tile = &layout.tiles[0];
        for y in tile.rect.y..tile.rect.y + tile.rect.height {
            for x in tile.rect.x..tile.rect.x + tile.rect.width {
                assert_ne!(
                    buf.cell((x, y)).unwrap().symbol(),
                    "·",
                    "no orbit-particle dot may render inside the tile at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn working_accent_segment_bolds_and_advances_in_the_buffer() {
        let drawn_accent = |offset: f32| -> Vec<(u16, u16)> {
            let app = connected_app_with_phase(offset);
            let buf = render_collage_for(&app, false, Instant::now());
            let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
            let geometry = planet_geometry(
                &layout.tiles[0],
                stage_field(),
                AgentStatus::Working,
                app.viz(),
                false,
            );
            let bold: Vec<(u16, u16)> = geometry
                .atmosphere
                .iter()
                .map(|glow| glow.cell)
                .filter(|&cell| {
                    buf.cell(cell)
                        .unwrap()
                        .style()
                        .add_modifier
                        .contains(Modifier::BOLD)
                })
                .collect();
            let accent = accent_cells(&geometry);
            assert_eq!(bold, accent, "the buffer bolds exactly the accent segment");
            assert_eq!(
                bold.len(),
                WORKING_SEGMENT as usize,
                "the drawn working segment keeps its length"
            );
            bold
        };
        assert_ne!(
            drawn_accent(0.2),
            drawn_accent(0.7),
            "new played audio advances the drawn segment"
        );
    }

    #[test]
    fn selected_planet_draws_four_selection_brackets_that_never_select() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let unselected = buffer_text(&render_collage_for(&app, false, Instant::now()));
        for bracket in ['┌', '┐', '└', '┘'] {
            assert!(
                !unselected.contains(bracket),
                "brackets render only for the selected planet"
            );
        }

        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let theme = Theme::for_name(ThemeName::Minimal);
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        let geometry = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Working,
            app.viz(),
            true,
        );
        assert_eq!(geometry.brackets.len(), 4, "four drawn corner brackets");
        for bracket in &geometry.brackets {
            let cell = buf.cell(bracket.cell).unwrap();
            assert_eq!(cell.symbol(), bracket.glyph);
            let style = cell.style();
            assert_eq!(
                style.fg,
                Some(theme.selection_bg),
                "focus brackets use the visible selection accent as a line color",
            );
            assert_ne!(
                style.bg,
                Some(theme.selection_bg),
                "focus brackets never paint the selection background",
            );
            assert!(style.add_modifier.contains(Modifier::BOLD));
            assert!(
                hit_test(CANVAS, bracket.cell.0, bracket.cell.1, false, &app).is_none(),
                "a bracket cell never selects"
            );
        }
    }

    #[test]
    fn low_power_render_keeps_the_frozen_atmosphere_unbrightened() {
        let app = low_power_app_captured_from(0.3, 0.9);
        let buf = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = collage_layout(app.active_agents(), captured, &[], stage_field());
        let geometry = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Working,
            captured,
            false,
        );
        let mut checked = 0;
        for glow in &geometry.atmosphere {
            checked += 1;
            assert!(
                !buf.cell(glow.cell)
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::BOLD),
                "low power never brightens the frozen accent segment"
            );
        }
        assert!(checked > 0, "sanity: the frozen planet keeps its glow");
    }

    #[test]
    fn dense_planet_field_renders_one_selectable_body_per_agent() {
        let snaps: Vec<AgentSnapshot> = (0..80)
            .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
            .collect();
        let app = collage_app(snaps);
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        assert_eq!(layout.tiles.len(), 80);
        let mut bodies = std::collections::HashSet::new();
        for tile in &layout.tiles {
            let geometry =
                planet_geometry(tile, stage_field(), AgentStatus::Working, app.viz(), false);
            assert!(!geometry.body.is_empty(), "every dense planet keeps a body");
            let disc = disc_geometry(tile.rect, stage_field());
            let center = (
                (disc.origin.0 + disc.mask.width() as i32 / 2) as u16,
                (disc.origin.1 + disc.mask.height() as i32 / 2) as u16,
            );
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
        // decoration overhang) must not care which statuses the planets
        // carry.
        let reference = phase_frame();
        let layout = collage_layout(&agents(3), &reference, &[], stage_field());
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
            let canvas = stage_field();
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

    // --- disc-mask planets -------------------------------------------------

    /// A hand-built tile whose bound is far larger than the largest disc
    /// mask, as only a huge sparse canvas could offer one.
    fn oversized_tile(area: Rect) -> CollageTile {
        let rect = Rect::new(area.x + 4, area.y + 6, 20, 11);
        CollageTile {
            index: 0,
            seed: 9,
            base_rect: rect,
            rect,
            energy: 0.4,
        }
    }

    /// How many of `count` dense laid-out agents keep a non-empty planet body.
    fn dense_planet_body_count(count: usize, area: Rect) -> usize {
        let layout = collage_layout(&agents(count), &frame(0.5, vec![0.5; 16]), &[], area);
        layout
            .tiles
            .iter()
            .filter(|tile| {
                !planet_geometry(tile, area, AgentStatus::Working, &phase_frame(), false)
                    .body
                    .is_empty()
            })
            .count()
    }

    /// Render one agent named `name` under `status` after `frame`.
    fn rendered_surface(name: &str, status: AgentStatus, frame: VizFrame) -> Buffer {
        let mut app = collage_app(vec![snap("ws", name, Some(name), status)]);
        push_frame(&mut app, frame);
        render_collage_for(&app, false, Instant::now())
    }

    /// Every drawn planet-surface cell (body or crater glyph) with its
    /// foreground color, so stability tests compare surface pattern and
    /// palette at once.
    fn surface_geometry(buf: &Buffer) -> Vec<(u16, u16, String, Option<Color>)> {
        let canvas = agent_stage_layout(*buf.area()).field;
        let mut cells = Vec::new();
        for y in canvas.y..canvas.y + canvas.height {
            for x in canvas.x..canvas.x + canvas.width {
                let cell = buf.cell((x, y)).unwrap();
                let symbol = cell.symbol();
                if symbol == PLANET_BODY_GLYPH || symbol == CRATER_GLYPH {
                    cells.push((x, y, symbol.to_string(), cell.style().fg));
                }
            }
        }
        cells
    }

    #[test]
    fn disc_mask_caps_oversized_bounds_and_keeps_dense_body_cells() {
        let area = Rect::new(0, 0, 120, 36);
        let tile = oversized_tile(area);
        let geometry = planet_geometry(&tile, area, AgentStatus::Idle, &phase_frame(), false);
        assert_eq!(
            geometry.mask,
            DiscMask::Large7x5,
            "an oversized bound caps at the largest fixed mask"
        );
        assert!(!geometry.body.is_empty());
        assert!(geometry.body.len() <= 7 * 5, "the fixed disc stays capped");
        assert_eq!(dense_planet_body_count(80, Rect::new(0, 0, 50, 15)), 80);
    }

    #[test]
    fn seed_stably_selects_each_banded_world_surface_and_palette() {
        assert_eq!(planet_surface(0), PlanetSurface::BandedGas);
        assert_eq!(planet_surface(1), PlanetSurface::IceCap);
        assert_eq!(planet_surface(2), PlanetSurface::CrateredRock);
        assert_eq!(
            planet_palette(17).base_position,
            planet_palette(17).base_position
        );
        assert_ne!(
            planet_palette(0).base_position,
            planet_palette(1).base_position
        );
    }

    #[test]
    fn surface_cells_are_stable_across_audio_status_and_time() {
        let first = rendered_surface("gas", AgentStatus::Working, phase_frame_with_offset(0.1));
        let later = rendered_surface("gas", AgentStatus::Blocked, phase_frame_with_offset(0.8));
        assert_eq!(surface_geometry(&first), surface_geometry(&later));
    }

    #[test]
    fn pocket_body_paints_base_and_accent_theme_spectrum_colors() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let canvas = stage_field();
        let layout = collage_layout(app.active_agents(), app.viz(), &[], canvas);
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz(), false);
        let theme = Theme::for_name(ThemeName::Minimal);
        let palette = planet_palette(tile.seed);
        let base = theme.spectrum_color(palette.base_position);
        let accent = theme.spectrum_color(palette.accent_position);
        assert_ne!(base, accent, "an identity owns two distinct theme colors");
        let colors: HashSet<Option<Color>> = geometry
            .body
            .iter()
            .map(|&(x, y)| buf.cell((x, y)).unwrap().style().fg)
            .collect();
        assert!(
            colors.contains(&Some(base)),
            "the body paints its base color"
        );
        assert!(
            colors.contains(&Some(accent)),
            "the surface pattern paints its accent color"
        );
        assert_eq!(colors.len(), 2, "only the identity pair colors the body");
    }

    #[test]
    fn disc_geometry_drives_selection_not_the_oversized_rect() {
        let app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        let canvas = stage_field();
        let layout = collage_layout(app.active_agents(), app.viz(), &[], canvas);
        let tile = &layout.tiles[0];
        assert!(
            tile.rect.width > 7,
            "sanity: the sparse layout offers an oversized bound"
        );
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz(), false);
        let &(x, y) = geometry.body.first().expect("a disc body");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a disc body cell selects"
        );
        let (x, y) = geometry
            .atmosphere
            .first()
            .expect("a roomy disc keeps an atmosphere")
            .cell;
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "an atmosphere cell never selects"
        );

        let hit: HashSet<(u16, u16)> = geometry.hit_cells.iter().copied().collect();
        let (x, y) = (tile.rect.y..tile.rect.y + tile.rect.height)
            .flat_map(|y| (tile.rect.x..tile.rect.x + tile.rect.width).map(move |x| (x, y)))
            .find(|cell| !hit.contains(cell))
            .expect("the oversized rect keeps cells off the disc");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "an oversized-rect-only cell resolves nothing"
        );
    }

    // --- music reactivity -------------------------------------------------

    #[test]
    fn rms_and_fft_move_planets_without_any_shadow_trails() {
        let quiet = render_collage(4, frame(0.05, vec![0.05; 16]), vec![], false);
        let loud = render_collage(
            4,
            frame(0.9, vec![0.9; 16]),
            vec![frame(0.4, vec![0.4; 16])],
            false,
        );
        assert_ne!(quiet, loud);
        for buf in [&quiet, &loud] {
            assert_eq!(
                buffer_text(buf).matches('∙').count(),
                0,
                "no shadow-trail cell may render at any energy"
            );
        }
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
            "low power freezes trace, disc, atmosphere, and bracket geometry"
        );

        // The same later frame in normal power does move the scope.
        let live = render_collage_for(&app, false, Instant::now());
        assert_ne!(field_cells(&later), field_cells(&live));

        // A fresh snapshot still recolors the frozen planet's atmosphere in
        // place.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Blocked)],
            now: Instant::now(),
        });
        let theme = Theme::for_name(ThemeName::Minimal);
        let recolored = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = collage_layout(app.active_agents(), captured, &[], stage_field());
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, stage_field(), AgentStatus::Blocked, captured, false);
        let (x, y) = geometry
            .atmosphere
            .first()
            .expect("the frozen planet keeps its atmosphere")
            .cell;
        assert_eq!(
            recolored.cell((x, y)).unwrap().style().fg,
            Some(theme.error),
            "the frozen atmosphere takes the fresh blocked color"
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
    fn atmosphere_renders_all_five_status_treatments_from_the_theme() {
        let theme = Theme::for_name(ThemeName::Minimal);
        for status in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            let mut app = collage_app(vec![snap("ws", "p1", Some("one"), status)]);
            push_frame(&mut app, phase_frame());
            let buf = render_collage_for(&app, false, Instant::now());
            let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
            let geometry =
                planet_geometry(&layout.tiles[0], stage_field(), status, app.viz(), false);
            assert!(
                !geometry.atmosphere.is_empty(),
                "{status:?} keeps a visible atmosphere"
            );
            let expected_accents = match status {
                AgentStatus::Working => WORKING_SEGMENT as usize,
                AgentStatus::Blocked => BLOCKED_SEGMENT as usize,
                _ => 0,
            };
            assert_eq!(
                accent_cells(&geometry).len(),
                expected_accents,
                "{status:?} keeps its segment length"
            );
            for glow in &geometry.atmosphere {
                let cell = buf.cell(glow.cell).unwrap();
                assert_eq!(cell.symbol(), ATMOSPHERE_GLYPH);
                let style = cell.style();
                assert_eq!(
                    style.fg,
                    Some(status_color(status, &theme)),
                    "{status:?} atmosphere must take its theme color"
                );
                let bold = style.add_modifier.contains(Modifier::BOLD);
                let dim = style.add_modifier.contains(Modifier::DIM);
                match status {
                    AgentStatus::Working => {
                        assert_eq!(bold, glow.accent, "working bolds exactly its segment");
                        assert!(!dim, "the working ring never fades while energy is up");
                    }
                    AgentStatus::Blocked => {
                        assert_eq!(dim, !glow.accent, "the blocked ring dims off the segment");
                        assert_eq!(
                            bold,
                            glow.accent && geometry.atmosphere_lift,
                            "the blocked segment bolds only on its pulse"
                        );
                    }
                    AgentStatus::Idle | AgentStatus::Done => {
                        assert!(!bold, "{status:?} never bolds");
                        assert_eq!(
                            dim, !geometry.atmosphere_lift,
                            "{status:?} brightness follows its breathing/pulse lift"
                        );
                    }
                    AgentStatus::Unknown => {
                        assert!(!bold && dim, "unknown stays dim and neutral");
                    }
                }
            }
        }
    }

    #[test]
    fn unnamed_planet_has_no_tag_and_no_private_fallback() {
        let mut app = app_with_only_unnamed_agent();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        assert!(app.selected_agent().is_some(), "sanity: something selected");
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(!text.contains("workspace-1"), "workspace ids never render");
        assert!(!text.contains("pane-1"), "pane ids never render");
        assert!(
            !text.contains("working"),
            "an unnamed selection leaks no status tag"
        );
    }

    #[test]
    fn selecting_an_unnamed_agent_shows_no_label_at_all() {
        let mut app = collage_app(vec![snap("alpha", "p1", None, AgentStatus::Working)]);
        app.apply(Action::SelectNextAgent);
        assert!(app.selected_agent().is_some());
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !text.contains("working"),
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
        app.apply(Action::SelectNextAgent);
        push_frame(&mut app, older_phase_frame());
        push_frame(&mut app, phase_frame());
        let live = render_collage_for(&app, false, Instant::now());
        let live_field = field_cells(&live);
        assert!(
            count_primary_phase_cells(&live) > 0,
            "sanity: the final live frame has a phase trace to freeze"
        );
        assert!(
            buffer_text(&live).contains('┌'),
            "sanity: the selected planet keeps focus brackets to freeze"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale_buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(
            field_cells(&stale_buf),
            live_field,
            "stale freezes the exact background, disc, and decoration geometry of the last live frame"
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
        app.apply(Action::SelectNextAgent);
        push_frame(&mut app, phase_frame());
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let buf = render_collage_for(&app, false, Instant::now());
        assert_eq!(count_planet_cells(&buf), 0, "no agent planets render");
        assert_eq!(count_primary_phase_cells(&buf), 0, "no phase trace renders");
        assert_eq!(count_secondary_phase_cells(&buf), 0);
        let text = buffer_text(&buf);
        for bracket in ['┌', '┐', '└', '┘'] {
            assert!(
                !text.contains(bracket),
                "unavailable hides the selected planet's focus brackets"
            );
        }
        assert!(text.contains("agents · unavailable · retrying"));
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
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
    fn clicks_resolve_only_planet_cells_never_the_background() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, older_phase_frame());
        push_frame(&mut app, phase_frame());
        let canvas = stage_field();
        let history: Vec<VizFrame> = app.viz_history().skip(1).cloned().collect();
        let layout = collage_layout(app.active_agents(), app.viz(), &history, canvas);
        let planet_cells: HashSet<(u16, u16)> = layout
            .tiles
            .iter()
            .flat_map(|tile| tile_hit_cells(tile, canvas))
            .collect();
        let on_a_planet = |x: u16, y: u16| planet_cells.contains(&(x, y));

        let mut checked_background = false;
        for layer in &layout.background.layers {
            for cell in &layer.cells {
                if !on_a_planet(cell.x, cell.y) {
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
            "sanity: some phase cell sat outside planets"
        );
    }

    #[test]
    fn body_click_selects_but_decoration_scope_and_empty_cells_do_not() {
        let mut app = collage_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Idle),
        ]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        let canvas = stage_field();
        let layout = collage_layout(app.active_agents(), app.viz(), &[], canvas);
        let planet_cells: HashSet<(u16, u16)> = layout
            .tiles
            .iter()
            .flat_map(|tile| tile_hit_cells(tile, canvas))
            .collect();

        let tile = &layout.tiles[0];
        let status = app.active_agents()[tile.index].status;
        let geometry = planet_geometry(tile, canvas, status, app.viz(), false);
        let &(x, y) = geometry.body.first().expect("a planet keeps body cells");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a body cell selects"
        );
        let (x, y) = geometry
            .atmosphere
            .first()
            .expect("a roomy planet keeps an atmosphere")
            .cell;
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "an atmosphere cell never selects"
        );

        let (x, y) = layout
            .background
            .layers
            .iter()
            .flat_map(|layer| layer.cells.iter())
            .map(|cell| (cell.x, cell.y))
            .find(|cell| !planet_cells.contains(cell))
            .expect("some scope cell sits outside every planet");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "a scope-only cell never selects"
        );

        let (x, y) = (canvas.y..canvas.y + canvas.height)
            .flat_map(|y| (canvas.x..canvas.x + canvas.width).map(move |x| (x, y)))
            .find(|cell| !planet_cells.contains(cell))
            .expect("the canvas keeps empty cells");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "an empty cell never selects"
        );
    }

    #[test]
    fn clicks_resolve_nothing_when_missed_stale_or_closed() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        assert!(hit_test(CANVAS, 0, 0, false, &app).is_none(), "corner miss");

        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
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
        let canvas = stage_field();
        let live_layout = collage_layout(app.active_agents(), app.viz(), &[], canvas);
        let moved = &live_layout.tiles[0];
        let (captured, _) = app.low_power_viz().expect("low power captured a frame");
        let frozen_layout = collage_layout(app.active_agents(), captured, &[], canvas);
        let frozen = &frozen_layout.tiles[0];
        assert_eq!(
            frozen.rect, frozen.base_rect,
            "the quiet capture sits on base"
        );
        assert_ne!(
            moved.rect, frozen.rect,
            "sanity: a loud frame moves the tile off base"
        );

        // The frozen planet's own body cells still select in low power.
        let frozen_cells = tile_hit_cells(frozen, canvas);
        let &(x, y) = frozen_cells.first().expect("the frozen planet has cells");
        assert!(
            hit_test(CANVAS, x, y, true, &app).is_some(),
            "a drawn low-power planet cell selects"
        );

        // A cell only the audio-moved planet covers holds no drawn planet in
        // low power, so it must resolve nothing there.
        let frozen_set: HashSet<(u16, u16)> = frozen_cells.into_iter().collect();
        let mut checked = false;
        for &(x, y) in &tile_hit_cells(moved, canvas) {
            if !frozen_set.contains(&(x, y)) {
                checked = true;
                assert!(
                    hit_test(CANVAS, x, y, true, &app).is_none(),
                    "an undrawn cell ({x}, {y}) must resolve nothing in low power"
                );
            }
        }
        assert!(
            checked,
            "sanity: the moved planet exposes cells off the frozen planet"
        );
    }

    #[test]
    fn low_power_selects_frozen_planet_cells_but_scope_cells_do_nothing() {
        let app = low_power_app_captured_from(0.3, 0.9);
        let canvas = stage_field();
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = collage_layout(app.active_agents(), captured, &[], canvas);
        let cells = tile_hit_cells(&layout.tiles[0], canvas);
        let &(x, y) = cells.first().expect("the frozen planet keeps cells");
        assert!(
            hit_test(CANVAS, x, y, true, &app).is_some(),
            "a frozen body cell selects in low power"
        );

        let planet_cells: HashSet<(u16, u16)> = cells.into_iter().collect();
        let (x, y) = layout
            .background
            .layers
            .iter()
            .flat_map(|layer| layer.cells.iter())
            .map(|cell| (cell.x, cell.y))
            .find(|cell| !planet_cells.contains(cell))
            .expect("the captured scope keeps cells off the planet");
        assert!(
            hit_test(CANVAS, x, y, true, &app).is_none(),
            "a scope-only cell selects nothing in low power"
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

    // --- agent planets stage ------------------------------------------------

    #[test]
    fn planet_disc_masks_are_round_and_never_draw_rectangle_shadows() {
        let mut app = collage_app(
            (0..3)
                .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
                .collect(),
        );
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let field = agent_stage_layout(CANVAS).field;
        let layout = collage_layout(app.active_agents(), app.viz(), &[], field);
        assert!(
            layout.tiles.iter().any(|tile| {
                planet_geometry(tile, field, AgentStatus::Working, app.viz(), false).mask
                    == DiscMask::Large7x5
            }),
            "a sparse stage keeps at least one 7x5 disc"
        );
        let text = buffer_text(&buf);
        assert_eq!(
            text.matches('∙').count(),
            0,
            "no full-tile shadow cell may render"
        );
        assert!(!text.contains('╲'));
        assert!(!text.contains('╱'));
    }

    #[test]
    fn dense_disc_masks_reduce_7x5_to_5x3_to_3x3_then_one_cell() {
        assert_eq!(DiscMask::for_bound(18, 9), DiscMask::Large7x5);
        assert_eq!(DiscMask::for_bound(6, 4), DiscMask::Medium5x3);
        assert_eq!(DiscMask::for_bound(4, 3), DiscMask::Small3x3);
        assert_eq!(DiscMask::for_bound(2, 1), DiscMask::Dot);

        let sparse_area = Rect::new(0, 0, 120, 28);
        let sparse = collage_layout(&agents(3), &frame(0.0, vec![0.0; 16]), &[], sparse_area);
        assert!(sparse.tiles.iter().any(|tile| {
            planet_geometry(
                tile,
                sparse_area,
                AgentStatus::Working,
                &phase_frame(),
                false,
            )
            .mask
                == DiscMask::Large7x5
        }));

        let dense_area = Rect::new(0, 0, 50, 15);
        let dense = collage_layout(&agents(80), &frame(0.5, vec![0.5; 16]), &[], dense_area);
        assert_eq!(dense.tiles.len(), 80);
        for tile in &dense.tiles {
            let geometry = planet_geometry(
                tile,
                dense_area,
                AgentStatus::Working,
                &phase_frame(),
                false,
            );
            assert!(
                !geometry.body.is_empty(),
                "every dense planet keeps at least one body cell"
            );
            if geometry.mask == DiscMask::Dot {
                assert!(
                    geometry.atmosphere.is_empty(),
                    "a one-cell disc suppresses decoration instead of crowding"
                );
            }
        }
    }

    #[test]
    fn stage_hides_details_until_the_selected_agent_opens_the_modal() {
        let mut app = collage_app(vec![AgentSnapshot {
            id: AgentId::new("workspace-private", "pane-private"),
            details: AgentDetails {
                name: Some("research".to_string()),
                agent: Some("pi".to_string()),
                activity: Some("Review the modal".to_string()),
            },
            status: AgentStatus::Working,
        }]);
        push_frame(&mut app, phase_frame());
        let stage = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(!stage.contains("research"));
        assert!(!stage.contains("working"));
        assert!(!stage.contains("pi"));

        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let modal = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(modal.contains("Agent details"));
        assert!(modal.contains("name: research"));
        assert!(modal.contains("agent: pi"));
        assert!(modal.contains("status: working"));
        assert!(modal.contains("activity: Review the modal"));
        assert!(!modal.contains("workspace-private"));
        assert!(!modal.contains("pane-private"));
    }

    #[test]
    fn long_activity_wraps_to_two_lines_without_growing_the_modal() {
        let activity =
            "012345678901234567890123456789012345678901234567890123456789012345678901234567890";
        let mut app = collage_app(vec![AgentSnapshot {
            id: AgentId::new("workspace-private", "pane-private"),
            details: AgentDetails {
                name: None,
                agent: Some("pi".to_string()),
                activity: Some(activity.to_string()),
            },
            status: AgentStatus::Working,
        }]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let modal = buffer_text(&render_collage_for(&app, false, Instant::now()));
        let activity_line = modal
            .lines()
            .find(|line| line.contains("activity: "))
            .expect("first activity line");
        assert!(activity_line.contains("012345678901234567890123456789012345"));
        assert!(
            modal.contains('…'),
            "second line truncates with an ellipsis"
        );
        assert!(
            !modal.contains(activity),
            "a third activity line never renders"
        );
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
