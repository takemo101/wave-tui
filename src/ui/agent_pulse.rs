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
//! Status is orbit language on an explicit octagonal ring mask: Working's
//! complete playing-colored ring carries a bright arc whose position
//! advances only with new played-audio phase data; Idle keeps a muted still
//! ring; Blocked draws an error-colored broken orbit with a stable
//! seed-derived gap and never any cross-like glyph; Done dims and keeps a
//! small satellite on its orbit; Unknown stays muted and dim. Every
//! explicitly named planet keeps a permanent two-line side tag — its name
//! over its normalized status — placed collision-aware right, left, below,
//! then above its disc against every other disc and tag; the selected tag
//! brightens and draws last. Unnamed planets keep no tag. Nothing moves
//! from a timer: identical frames render identical cells. A frame at or
//! below the silence threshold draws no trace or persistence at all —
//! analyzer silence carries non-empty all-zero traces that would otherwise
//! pile a point cluster at the field center — so silence stays calm, dim,
//! and still. Stale renders the reducer-captured final composition dimmed;
//! `--low-power` renders the App-captured first frame so trace, disc,
//! ring-arc, and tag geometry stay frozen while state colors keep
//! refreshing.
//!
//! Mouse input flows through [`hit_test`], which shares [`collage_layout`]
//! and [`planet_geometry`] with rendering so a click resolves against
//! exactly the planet body/ring cells that were drawn (scope, vignette,
//! tags, and empty cells resolve nothing), and returns only the read-only
//! selection [`Action`]; the CLI event loop owns applying it.
//!
//! Privacy: side tags show the explicit Herdr agent `name` only. No pane
//! id, workspace id, cwd, or agent type is ever rendered. All colors come
//! from the active [`Theme`]; no palette values are added.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
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
/// Glyph tracing a planet's orbit ring.
const RING_GLYPH: &str = "∘";
/// Glyph for the bright audio-positioned arc on a Working planet's ring.
const WORKING_ARC_GLYPH: &str = "●";
/// Glyph for the small satellite a Done planet keeps on its orbit.
const SATELLITE_GLYPH: &str = "▪";
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

/// Explicit clockwise orbit cells around each mask, as offsets from the mask
/// origin. Every ring is an octagon hugging the disc silhouette — diagonal
/// corner steps included — so the orbit can never read as a vertical or
/// horizontal cross.
const LARGE_RING: [(i32, i32); 16] = [
    (2, -1),
    (3, -1),
    (4, -1),
    (5, 0),
    (6, 1),
    (7, 2),
    (6, 3),
    (5, 4),
    (4, 5),
    (3, 5),
    (2, 5),
    (1, 4),
    (0, 3),
    (-1, 2),
    (0, 1),
    (1, 0),
];
/// Clockwise orbit offsets around the 5×3 mask.
const MEDIUM_RING: [(i32, i32); 12] = [
    (1, -1),
    (2, -1),
    (3, -1),
    (4, 0),
    (5, 1),
    (4, 2),
    (3, 3),
    (2, 3),
    (1, 3),
    (0, 2),
    (-1, 1),
    (0, 0),
];
/// Clockwise orbit offsets around the 3×3 mask.
const SMALL_RING: [(i32, i32); 8] = [
    (1, -1),
    (2, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 2),
    (-1, 1),
    (0, 0),
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

/// The clockwise orbit offsets for `mask`; the one-cell disc keeps none.
fn ring_mask(mask: DiscMask) -> &'static [(i32, i32)] {
    match mask {
        DiscMask::Large7x5 => &LARGE_RING,
        DiscMask::Medium5x3 => &MEDIUM_RING,
        DiscMask::Small3x3 => &SMALL_RING,
        DiscMask::Dot => &[],
    }
}

/// One placed disc: its chosen mask, its top-left mask origin (kept signed
/// so ring overhang can clip at the field edge), and its body cells inside
/// `area`.
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

/// The full orbit cells for a placed mask, clipped to `area`, in the stable
/// clockwise mask order the Working arc and Blocked gap index into.
fn ring_cells(mask: DiscMask, origin: (i32, i32), area: Rect) -> Vec<(u16, u16)> {
    ring_mask(mask)
        .iter()
        .filter_map(|&(dx, dy)| {
            let x = origin.0 + dx;
            let y = origin.1 + dy;
            (x >= 0 && y >= 0 && rect_contains(area, x as u16, y as u16))
                .then_some((x as u16, y as u16))
        })
        .collect()
}

/// One agent's planet in field cells, derived purely from its tile, status,
/// and the current phase frame. `base_body` follows the stable `base_rect`
/// so tests can pin identity geometry; every other field follows the
/// audio-transformed `rect`. Only `working_arc` consults phase data — every
/// other status keeps still geometry across audio frames.
struct PlanetGeometry {
    /// The disc body derived from the stable pre-transform rectangle.
    #[cfg_attr(not(test), allow(dead_code))]
    base_body: Vec<(u16, u16)>,
    /// The fixed mask this planet's slot chose.
    #[cfg_attr(not(test), allow(dead_code))]
    mask: DiscMask,
    body: Vec<(u16, u16)>,
    craters: Vec<(u16, u16)>,
    /// The status-drawn orbit cells: Blocked's stable gap already removed.
    ring: Vec<(u16, u16)>,
    working_arc: Vec<(u16, u16)>,
    satellite: Option<(u16, u16)>,
    /// Body plus drawn ring cells only: the planet-only selection targets
    /// shared by [`hit_test`] and the tag collision layout.
    hit_cells: Vec<(u16, u16)>,
    /// Status-independent bound of the body plus the complete orbit. Tag
    /// placement anchors here so Blocked's removed gap — which may clip an
    /// extreme drawn-ring cell — can never shift a tag between statuses.
    tag_bound: Rect,
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
    let base_body = disc_geometry(tile.base_rect, area).body;
    let disc = disc_geometry(tile.rect, area);
    let full_ring = ring_cells(disc.mask, disc.origin, area);
    let body = disc.body;
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
        working_arc(&full_ring, tile.seed, frame)
    } else {
        Vec::new()
    };
    let satellite = if status == AgentStatus::Done && !full_ring.is_empty() {
        Some(full_ring[((tile.seed >> 7) % full_ring.len() as u64) as usize])
    } else {
        None
    };
    let tag_bound = cell_bounds(body.iter().chain(&full_ring).copied());
    let ring = if status == AgentStatus::Blocked {
        broken_ring(&full_ring, tile.seed)
    } else {
        full_ring
    };
    let hit_cells = body.iter().chain(&ring).copied().collect();
    PlanetGeometry {
        base_body,
        mask: disc.mask,
        body,
        craters,
        ring,
        working_arc,
        satellite,
        hit_cells,
        tag_bound,
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
/// Maps a click on a planet's drawn body or ring cells — the exact
/// [`planet_geometry`] hit cells the renderer draws — to the read-only
/// [`Action::SelectAgent`]; returns `None` whenever the canvas is closed, the
/// integration is hidden, the connection is stale or unavailable, Signal View
/// is active, or the click misses every planet. Scope phase, vignette,
/// side-tag, and empty cells resolve nothing. Overlapping planets resolve
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
        let geometry = planet_geometry(tile, canvas, view.status, frame);
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
/// breathing vignette, the
/// phosphor-persistence and dual phase-trace layers, each planet's disc-mask
/// body, craters, and status orbit ring, and every named planet's permanent
/// two-line side tag with the selected tag bright and drawn last. Stale
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
    // renders the App-captured first frame so no trace, disc, ring-arc, or
    // tag geometry advances; live renders use the current frame plus the
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

    // One geometry per tile, shared by planet rendering and tag layout so a
    // tag can never disagree with the disc it annotates.
    let geometries: Vec<PlanetGeometry> = layout
        .tiles
        .iter()
        .map(|tile| {
            let status = agents
                .get(tile.index)
                .map(|view| view.status)
                .unwrap_or(AgentStatus::Unknown);
            planet_geometry(tile, field, status, frame)
        })
        .collect();

    // Planets draw in stable slot order; the selected planet comes forward
    // last.
    for (tile, geometry) in layout.tiles.iter().zip(&geometries) {
        if Some(tile.index) == selected_index {
            continue;
        }
        render_planet(buf, tile, geometry, agents, theme, false, stale, low_power);
    }
    if let Some(selected) = selected_index {
        if let Some((tile, geometry)) = layout
            .tiles
            .iter()
            .zip(&geometries)
            .find(|(tile, _)| tile.index == selected)
        {
            render_planet(buf, tile, geometry, agents, theme, true, stale, low_power);
        }
    }

    // Permanent side tags draw over every disc; the bright selected tag
    // comes last so it stays readable even on the collision fallback.
    let silent = frame.rms <= SILENCE_ENERGY;
    let tags = planet_tag_placements(agents, &layout.tiles, &geometries, selected_index, field);
    for tag in tags.iter().filter(|tag| !tag.selected) {
        render_tag(buf, tag, theme, stale, silent);
    }
    if let Some(tag) = tags.iter().find(|tag| tag.selected) {
        render_tag(buf, tag, theme, stale, silent);
    }
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
    Paragraph::new("Tab/↑↓/click select · Space play · +/- volume · a/Esc close")
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted))
        .render(area, buf);
}

/// Draw one agent planet in order: disc-mask body fill, stable crater
/// shading, then the status orbit language — a complete playing-colored
/// ring with a bright audio-positioned arc while Working, a muted still
/// ring while Idle, an error-colored broken orbit while Blocked (never a
/// cross), a dim ring plus a small satellite while Done, and a dim muted
/// ring for Unknown. Selection restyles the existing body/ring cells; it
/// never draws a rectangle.
#[allow(clippy::too_many_arguments)]
fn render_planet(
    buf: &mut Buffer,
    tile: &CollageTile,
    geometry: &PlanetGeometry,
    agents: &[AgentView],
    theme: &Theme,
    selected: bool,
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

    // Banded Worlds surface: identity chooses the family and the theme
    // spectrum pair; status never colors the body — state stays on the ring.
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

    // The drawn ring cells already carry Blocked's stable gap; status only
    // picks the orbit's color language here.
    let ring_style = match view.status {
        AgentStatus::Working => edge_style(view.status, theme, tile.energy, low_power),
        AgentStatus::Idle => quiet_dim(Style::default().fg(theme.muted)),
        AgentStatus::Blocked => quiet_dim(Style::default().fg(theme.error)),
        AgentStatus::Done | AgentStatus::Unknown => {
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM)
        }
    };
    let ring_style = if selected {
        theme.selection_style()
    } else {
        ring_style
    };
    let ring_style = own_emphasis(with_stale(ring_style, stale));
    for &(x, y) in &geometry.ring {
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
    let arc = own_emphasis(with_stale(arc, stale));
    for &(x, y) in &geometry.working_arc {
        buf.set_string(x, y, WORKING_ARC_GLYPH, arc);
    }

    if let Some((x, y)) = geometry.satellite {
        let satellite = own_emphasis(with_stale(
            quiet_dim(Style::default().fg(theme.muted).add_modifier(Modifier::DIM)),
            stale,
        ));
        buf.set_string(x, y, SATELLITE_GLYPH, satellite);
    }
}

/// Planet cells fully own their emphasis: painting over a dim scope cell
/// (vignette, phosphor persistence) must not inherit its modifiers, because
/// Ratatui merges styles per cell. Subtract exactly the emphasis modifiers
/// the composed style did not add itself.
fn own_emphasis(style: Style) -> Style {
    style.remove_modifier((Modifier::DIM | Modifier::BOLD).difference(style.add_modifier))
}

/// Axis-aligned bounding rectangle of a cell set. Every laid-out planet
/// keeps at least its center body cell, so the empty fallback never fires
/// for real tiles.
fn cell_bounds(cells: impl IntoIterator<Item = (u16, u16)>) -> Rect {
    let mut cells = cells.into_iter();
    let Some((first_x, first_y)) = cells.next() else {
        return Rect::default();
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (first_x, first_x, first_y, first_y);
    for (x, y) in cells {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Rect::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
}

/// Widest name column a side tag reserves before truncating with an
/// ellipsis.
const TAG_NAME_MAX: usize = 12;
/// The longest normalized status label (`working`/`blocked`/`unknown`), so
/// a tag's width — and therefore its placement — never shifts when only its
/// status changes.
const TAG_STATUS_MAX: usize = 7;

/// One named planet's permanent two-line side tag: the explicit Herdr name
/// over the normalized status, plus its placed rectangle and selection
/// emphasis. Unnamed planets never produce one.
struct PlanetTag {
    /// The index into `App::active_agents()` this tag annotates.
    #[cfg_attr(not(test), allow(dead_code))]
    agent_index: usize,
    rect: Rect,
    name: String,
    status: AgentStatus,
    selected: bool,
}

/// Ellipsis-truncate an explicit name to the tag name column.
fn tag_name(name: &str) -> String {
    if name.chars().count() <= TAG_NAME_MAX {
        name.to_string()
    } else {
        let mut out: String = name.chars().take(TAG_NAME_MAX - 1).collect();
        out.push('…');
        out
    }
}

/// Whether any cell of `rect` is already reserved.
fn tag_rect_collides(rect: Rect, occupied: &HashSet<(u16, u16)>) -> bool {
    (rect.y..rect.y + rect.height)
        .any(|y| (rect.x..rect.x + rect.width).any(|x| occupied.contains(&(x, y))))
}

/// The in-priority-order tag candidates beside `bound`: right, left, below,
/// above — each fully inside `field`.
fn tag_candidate_rects(bound: Rect, width: u16, height: u16, field: Rect) -> Vec<Rect> {
    let mut candidates = Vec::new();
    let field_right = field.x + field.width;
    let field_bottom = field.y + field.height;
    let side_y = (bound.y + bound.height / 2)
        .min(field_bottom.saturating_sub(height))
        .max(field.y);
    let clamp_x = |x: u16| x.min(field_right.saturating_sub(width)).max(field.x);
    if (bound.x + bound.width) as u32 + width as u32 <= field_right as u32 {
        candidates.push(Rect::new(bound.x + bound.width, side_y, width, height));
    }
    if bound.x >= field.x + width {
        candidates.push(Rect::new(bound.x - width, side_y, width, height));
    }
    let below = bound.y + bound.height;
    if below as u32 + height as u32 <= field_bottom as u32 {
        candidates.push(Rect::new(clamp_x(bound.x), below, width, height));
    }
    if bound.y >= field.y + height {
        candidates.push(Rect::new(clamp_x(bound.x), bound.y - height, width, height));
    }
    candidates
}

/// The clamped right-side fallback used when every candidate collides; the
/// renderer draws colliding tags after every disc so even this stays
/// readable.
fn tag_fallback_rect(bound: Rect, width: u16, height: u16, field: Rect) -> Rect {
    let x = (bound.x + bound.width)
        .min((field.x + field.width).saturating_sub(width))
        .max(field.x);
    let y = (bound.y + bound.height / 2)
        .min((field.y + field.height).saturating_sub(height))
        .max(field.y);
    Rect::new(x, y, width, height)
}

/// Layout every named planet's permanent two-line side tag.
///
/// Placement walks the stable tile order and is selection-independent —
/// selecting a planet only brightens its tag, it never reflows the stage.
/// Each tag tries right, left, below, then above its disc bound, rejecting
/// candidates that collide with any planet's body/ring cells or an
/// already-reserved tag; the chosen cells are reserved before the next
/// planet places. When every candidate collides the clamped right fallback
/// wins. Unnamed agents contribute nothing — never a pane id, workspace id,
/// cwd, or agent-type fallback.
fn planet_tag_placements(
    agents: &[AgentView],
    tiles: &[CollageTile],
    geometries: &[PlanetGeometry],
    selected_index: Option<usize>,
    field: Rect,
) -> Vec<PlanetTag> {
    if field.width == 0 || field.height == 0 {
        return Vec::new();
    }
    let mut occupied: HashSet<(u16, u16)> = geometries
        .iter()
        .flat_map(|geometry| geometry.hit_cells.iter().copied())
        .collect();
    let mut tags = Vec::new();
    for (tile, geometry) in tiles.iter().zip(geometries) {
        let Some(view) = agents.get(tile.index) else {
            continue;
        };
        let Some(name) = &view.name else {
            continue;
        };
        let name = tag_name(name);
        let width = (name.chars().count().max(TAG_STATUS_MAX) as u16).min(field.width);
        let height = 2.min(field.height);
        let bound = geometry.tag_bound;
        let rect = tag_candidate_rects(bound, width, height, field)
            .into_iter()
            .find(|rect| !tag_rect_collides(*rect, &occupied))
            .unwrap_or_else(|| tag_fallback_rect(bound, width, height, field));
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                occupied.insert((x, y));
            }
        }
        tags.push(PlanetTag {
            agent_index: tile.index,
            rect,
            name,
            status: view.status,
            selected: selected_index == Some(tile.index),
        });
    }
    tags
}

/// Draw one two-line side tag: the explicit name over the normalized
/// status. Tags stay muted so the planets keep the stage; the selected tag
/// takes the theme selection emphasis, and silence/stale dim tags with the
/// rest of the field.
fn render_tag(buf: &mut Buffer, tag: &PlanetTag, theme: &Theme, stale: bool, silent: bool) {
    let mut style = if tag.selected {
        theme.selection_style()
    } else {
        Style::default().fg(theme.muted)
    };
    if silent {
        style = style.add_modifier(Modifier::DIM);
    }
    let style = own_emphasis(with_stale(style, stale));
    buf.set_stringn(
        tag.rect.x,
        tag.rect.y,
        &tag.name,
        tag.rect.width as usize,
        style,
    );
    if tag.rect.height >= 2 {
        buf.set_stringn(
            tag.rect.x,
            tag.rect.y + 1,
            status_label(tag.status),
            tag.rect.width as usize,
            style,
        );
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
            details: AgentDetails::default(),
            name: None,
            status,
            observed_at: Instant::now(),
        }
    }

    fn named_view(workspace: &str, pane: &str, name: &str, status: AgentStatus) -> AgentView {
        AgentView {
            details: AgentDetails {
                name: Some(name.to_string()),
                agent: None,
                activity: None,
            },
            name: Some(name.to_string()),
            ..view(workspace, pane, status)
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

    /// Cells drawn with agent-planet glyphs (bodies, craters, or rings)
    /// inside the stage field, so chrome rows (heading, title block, and
    /// footer) never count as planets.
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

    /// The selectable body/ring cells of one laid-out tile — the same union
    /// `planet_geometry` exposes as `hit_cells`. Independent of the phase
    /// frame: only the Working arc (a ring subset) consults it.
    fn tile_hit_cells(tile: &CollageTile, canvas: Rect) -> Vec<(u16, u16)> {
        planet_geometry(tile, canvas, AgentStatus::Working, &phase_frame()).hit_cells
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

    // --- ringed planets ----------------------------------------------------

    /// The laid-out tile for one agent alone in `area` under `frame`.
    fn tile_for(agent: &AgentView, area: Rect, frame: &VizFrame) -> CollageTile {
        collage_layout(std::slice::from_ref(agent), frame, &[], area)
            .tiles
            .remove(0)
    }

    /// Every drawn planet cell (body, status ring, arc, satellite) of the
    /// first agent with `status`, as `(x, y, symbol)` from the buffer. The
    /// geometry's `ring` already carries Blocked's stable gap.
    fn planet_cells_for(app: &App, status: AgentStatus) -> Vec<(u16, u16, String)> {
        let buf = render_collage_for(app, false, Instant::now());
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
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
        let geometry = planet_geometry(tile, stage_field(), status, app.viz());
        let mut cells: Vec<(u16, u16)> = geometry.body.clone();
        cells.extend(geometry.ring.iter().copied());
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
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
        planet_geometry(tile, stage_field(), AgentStatus::Working, app.viz()).working_arc
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
        // The cross prohibition guards the planet field; the chrome footer
        // legitimately spells `+/-` for the volume hint.
        let field_text: String = field_cells(&buf)
            .into_iter()
            .map(|(_, _, symbol)| symbol)
            .collect();
        for cross in ['×', '╳', '╲', '╱', '✕', '+'] {
            assert!(
                !field_text.contains(cross),
                "no cross-like glyph {cross} may render in the field"
            );
        }

        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        let tile = &layout.tiles[0];
        let blocked = planet_geometry(tile, stage_field(), AgentStatus::Blocked, app.viz());
        let full = planet_geometry(tile, stage_field(), AgentStatus::Idle, app.viz());
        assert!(
            blocked.ring.len() >= 2,
            "the broken orbit keeps visible cells"
        );
        assert!(
            blocked.ring.len() < full.ring.len(),
            "the blocked orbit must carry a gap"
        );
        let theme = Theme::for_name(ThemeName::Minimal);
        for &(x, y) in &blocked.ring {
            assert_eq!(buf.cell((x, y)).unwrap().symbol(), RING_GLYPH);
            assert_eq!(buf.cell((x, y)).unwrap().style().fg, Some(theme.error));
        }
        for cell in full.ring.iter().filter(|cell| !blocked.ring.contains(cell)) {
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
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
            planet_geometry(tile, stage_field(), status, app.viz())
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        assert_eq!(layout.tiles.len(), 80);
        let mut bodies = std::collections::HashSet::new();
        for tile in &layout.tiles {
            let geometry = planet_geometry(tile, stage_field(), AgentStatus::Working, app.viz());
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
        // ring overhang) must not care which statuses the planets carry.
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
                !planet_geometry(tile, area, AgentStatus::Working, &phase_frame())
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
        let geometry = planet_geometry(&tile, area, AgentStatus::Idle, &phase_frame());
        assert_eq!(
            geometry.mask,
            DiscMask::Large7x5,
            "an oversized bound caps at the largest fixed mask"
        );
        assert!(!geometry.body.is_empty());
        let bound = geometry.tag_bound;
        assert!(bound.width <= 7 + 2, "body plus ring overhang stays capped");
        assert!(bound.height <= 5 + 2);
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
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz());
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
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz());
        let &(x, y) = geometry.body.first().expect("a disc body");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a disc body cell selects"
        );
        let &(x, y) = geometry.ring.first().expect("a disc ring");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a disc ring cell selects"
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

    // --- side-tag candidate branches ----------------------------------------

    /// Which `planet_tag_placements` candidate a synthetic fixture forces.
    #[derive(Clone, Copy)]
    enum PlacementCase {
        Right,
        Left,
        Below,
        Above,
        AllCollide,
    }

    const TAG_FIELD: Rect = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 10,
    };

    /// A synthetic planet whose selectable cells are exactly `cells`.
    fn synthetic_planet(cells: &[(u16, u16)]) -> PlanetGeometry {
        PlanetGeometry {
            base_body: Vec::new(),
            mask: DiscMask::Dot,
            body: cells.to_vec(),
            craters: Vec::new(),
            ring: Vec::new(),
            working_arc: Vec::new(),
            satellite: None,
            hit_cells: cells.to_vec(),
            tag_bound: cell_bounds(cells.iter().copied()),
        }
    }

    /// A hand-built tile carrying only its agent index; the synthetic
    /// geometry supplies every cell.
    fn synthetic_tile(index: usize) -> CollageTile {
        CollageTile {
            index,
            seed: index as u64,
            base_rect: Rect::new(0, 0, 1, 1),
            rect: Rect::new(0, 0, 1, 1),
            energy: 0.0,
        }
    }

    /// The named fixture: a 3×3 block bounded by x 15..=17, y 4..=6. With a
    /// one-char name the tag is 7×2, so inside `TAG_FIELD` every candidate —
    /// right (18, 5), left (8, 5), below (15, 7), above (15, 2) — stays in
    /// bounds and only deliberate blockers knock one out.
    fn synthetic_named() -> PlanetGeometry {
        let cells: Vec<(u16, u16)> = (15..18).flat_map(|x| (4..7).map(move |y| (x, y))).collect();
        synthetic_planet(&cells)
    }

    /// Run `planet_tag_placements` against blockers occupying every
    /// candidate preceding the one `case` forces, and return the named
    /// planet's tag.
    fn forced_tag(case: PlacementCase) -> PlanetTag {
        let blockers: Vec<(u16, u16)> = match case {
            PlacementCase::Right => Vec::new(),
            PlacementCase::Left => vec![(20, 5)],
            PlacementCase::Below => vec![(20, 5), (9, 6)],
            PlacementCase::Above => vec![(20, 5), (9, 6), (16, 7)],
            PlacementCase::AllCollide => vec![(20, 5), (9, 6), (16, 7), (16, 3)],
        };
        let agents = vec![
            named_view("ws", "p0", "n", AgentStatus::Working),
            view("ws", "p1", AgentStatus::Working),
        ];
        let tiles = [synthetic_tile(0), synthetic_tile(1)];
        let geometries = [synthetic_named(), synthetic_planet(&blockers)];
        let mut tags = planet_tag_placements(&agents, &tiles, &geometries, None, TAG_FIELD);
        assert_eq!(tags.len(), 1, "only the named planet places a tag");
        tags.remove(0)
    }

    #[test]
    fn tag_placement_exercises_right_left_below_above_and_fallback() {
        assert_eq!(
            forced_tag(PlacementCase::Right).rect,
            Rect::new(18, 5, 7, 2)
        );
        assert_eq!(forced_tag(PlacementCase::Left).rect, Rect::new(8, 5, 7, 2));
        assert_eq!(
            forced_tag(PlacementCase::Below).rect,
            Rect::new(15, 7, 7, 2)
        );
        assert_eq!(
            forced_tag(PlacementCase::Above).rect,
            Rect::new(15, 2, 7, 2)
        );
        assert_eq!(
            forced_tag(PlacementCase::AllCollide).rect,
            Rect::new(18, 5, 7, 2),
            "when every candidate collides the clamped right fallback wins"
        );
    }

    #[test]
    fn working_and_blocked_place_identical_tag_rects_even_in_low_power() {
        // Tag placement must be status-independent: Blocked's removed gap
        // may clip an extreme ring cell, and a shrunken drawn-ring bound
        // must never shift the tag. Several identity seeds vary which ring
        // cells the gap removes.
        let field = stage_field();
        let place = |pane: &str, status: AgentStatus| {
            let agents = vec![named_view("ws", pane, "one", status)];
            let layout = collage_layout(&agents, &phase_frame(), &[], field);
            let geometries: Vec<PlanetGeometry> = layout
                .tiles
                .iter()
                .map(|tile| planet_geometry(tile, field, status, &phase_frame()))
                .collect();
            planet_tag_placements(&agents, &layout.tiles, &geometries, None, field)
                .remove(0)
                .rect
        };
        for pane in ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"] {
            assert_eq!(
                place(pane, AgentStatus::Working),
                place(pane, AgentStatus::Blocked),
                "status must not move the tag for pane {pane}"
            );
        }

        // Low power: geometry comes from the captured frame; a fresh
        // Blocked snapshot may retreat the orbit but not the frozen tag.
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        push_frame(&mut app, phase_frame_with_offset(0.3));
        let captured_tags = |app: &App| {
            let (captured, _) = app.low_power_viz().expect("policy captured a frame");
            let agents = app.active_agents();
            let layout = collage_layout(agents, captured, &[], field);
            let geometries: Vec<PlanetGeometry> = layout
                .tiles
                .iter()
                .map(|tile| planet_geometry(tile, field, agents[tile.index].status, captured))
                .collect();
            planet_tag_placements(agents, &layout.tiles, &geometries, None, field)
        };
        let working_rect = captured_tags(&app)[0].rect;
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Blocked)],
            now: Instant::now(),
        });
        assert_eq!(
            captured_tags(&app)[0].rect,
            working_rect,
            "a fresh blocked snapshot never moves the frozen tag"
        );
    }

    #[test]
    fn long_tag_names_truncate_with_ellipsis_and_keep_status() {
        let long = "supercalifragilistic";
        let name = tag_name(long);
        assert_eq!(name.chars().count(), TAG_NAME_MAX);
        assert!(name.ends_with('…'));

        let mut app = collage_app(vec![snap("ws", "p1", Some(long), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            text.contains("supercalifr…"),
            "the truncated name renders: {text}"
        );
        assert!(
            !text.contains("supercalifra"),
            "the untruncated name never renders"
        );
        assert!(text.contains("working"), "the status line stays visible");
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
            "low power freezes trace, disc, ring, and tag geometry"
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
        let layout = collage_layout(app.active_agents(), captured, &[], stage_field());
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, stage_field(), AgentStatus::Blocked, captured);
        let (x, y) = geometry.ring[0];
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
        let layout = collage_layout(app.active_agents(), app.viz(), &[], stage_field());
        for tile in &layout.tiles {
            let status = app.active_agents()[tile.index].status;
            let geometry = planet_geometry(tile, stage_field(), status, app.viz());
            let (x, y) = geometry.ring[0];
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
    fn selected_named_planet_tag_shows_only_name_and_status_at_its_placement() {
        let mut app = app_with_named_and_unnamed_agents();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let text = buffer_text(&buf);
        assert!(
            text.contains("research"),
            "selected explicit-name tag missing: {text}"
        );
        assert!(!text.contains("workspace-1"), "workspace ids never render");
        assert!(!text.contains("pane-1"), "pane ids never render");
        assert!(!text.contains("claude"), "raw pane details never render");

        // The tag sits exactly at its collision-aware placement beside the
        // selected planet, name row over status row, with the selection
        // emphasis instead of the muted tag color.
        let tag = stage_tags(&app)
            .into_iter()
            .find(|tag| tag.selected)
            .expect("the named selection keeps its tag");
        assert_eq!(tag.name, "research");
        assert_eq!(cell_text(&buf, tag.rect.x, tag.rect.y), "r");
        assert_eq!(cell_text(&buf, tag.rect.x, tag.rect.y + 1), "w");
        let theme = Theme::for_name(ThemeName::Minimal);
        assert_eq!(
            buf.cell((tag.rect.x, tag.rect.y)).unwrap().style().fg,
            theme.selection_style().fg,
            "the selected tag takes the selection emphasis"
        );
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
    fn named_planet_tag_renders_before_any_selection() {
        let app = collage_app(vec![snap(
            "alpha",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        assert!(app.selected_agent().is_none(), "sanity: nothing selected");
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            text.contains("research"),
            "the permanent tag renders without selection: {text}"
        );
        assert!(!text.contains("alpha"), "workspace ids never render");
        assert!(!text.contains("p1"), "pane ids never render");
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
        push_frame(&mut app, older_phase_frame());
        push_frame(&mut app, phase_frame());
        let live = render_collage_for(&app, false, Instant::now());
        let live_field = field_cells(&live);
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
            "stale freezes the exact background, disc, and tag geometry of the last live frame"
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
    fn ring_or_body_click_selects_but_scope_tags_and_empty_cells_do_not() {
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
        let geometry = planet_geometry(tile, canvas, status, app.viz());
        let &(x, y) = geometry.body.first().expect("a planet keeps body cells");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a body cell selects"
        );
        let &(x, y) = geometry
            .ring
            .first()
            .expect("a roomy planet keeps ring cells");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_some(),
            "a ring cell selects"
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

        let tags = stage_tags(&app);
        let tag = tags.first().expect("named planets keep tags");
        let (x, y) = (tag.rect.y..tag.rect.y + tag.rect.height)
            .flat_map(|y| (tag.rect.x..tag.rect.x + tag.rect.width).map(move |x| (x, y)))
            .find(|cell| !planet_cells.contains(cell))
            .expect("the tag keeps cells off every planet");
        assert!(
            hit_test(CANVAS, x, y, false, &app).is_none(),
            "a tag-only cell never selects"
        );

        let tag_cells: HashSet<(u16, u16)> = tags
            .iter()
            .flat_map(|tag| {
                (tag.rect.y..tag.rect.y + tag.rect.height).flat_map(move |y| {
                    (tag.rect.x..tag.rect.x + tag.rect.width).map(move |x| (x, y))
                })
            })
            .collect();
        let (x, y) = (canvas.y..canvas.y + canvas.height)
            .flat_map(|y| (canvas.x..canvas.x + canvas.width).map(move |x| (x, y)))
            .find(|cell| !planet_cells.contains(cell) && !tag_cells.contains(cell))
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

        // The frozen planet's own body/ring cells still select in low power.
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
            "a frozen body/ring cell selects in low power"
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

    /// The tag placements the stage renderer computes for `app` on the
    /// standard test canvas — the same geometry/selection inputs
    /// `render_canvas` uses.
    fn stage_tags(app: &App) -> Vec<PlanetTag> {
        let field = agent_stage_layout(CANVAS).field;
        let agents = app.active_agents();
        let layout = collage_layout(agents, app.viz(), &[], field);
        let geometries: Vec<PlanetGeometry> = layout
            .tiles
            .iter()
            .map(|tile| planet_geometry(tile, field, agents[tile.index].status, app.viz()))
            .collect();
        let selected = app
            .selected_agent()
            .and_then(|view| agents.iter().position(|other| other.id == view.id));
        planet_tag_placements(agents, &layout.tiles, &geometries, selected, field)
    }

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
                planet_geometry(tile, field, AgentStatus::Working, app.viz()).mask
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
            planet_geometry(tile, sparse_area, AgentStatus::Working, &phase_frame()).mask
                == DiscMask::Large7x5
        }));

        let dense_area = Rect::new(0, 0, 50, 15);
        let dense = collage_layout(&agents(80), &frame(0.5, vec![0.5; 16]), &[], dense_area);
        assert_eq!(dense.tiles.len(), 80);
        for tile in &dense.tiles {
            assert!(
                !planet_geometry(tile, dense_area, AgentStatus::Working, &phase_frame())
                    .body
                    .is_empty(),
                "every dense planet keeps at least one body cell"
            );
        }
    }

    #[test]
    fn named_planets_render_two_line_side_tags_and_unnamed_planets_do_not() {
        let mut app = app_with_named_and_unnamed_agents();
        app.apply(Action::ToggleAgentOverlay);
        push_frame(&mut app, phase_frame());
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            text.contains("research"),
            "a named planet keeps its permanent tag without selection: {text}"
        );
        assert!(
            text.contains("working"),
            "the tag's second line carries the status: {text}"
        );
        assert!(!text.contains("workspace-1"), "workspace ids never render");
        assert!(!text.contains("pane-1"), "pane ids never render");
        assert!(!text.contains("claude"), "raw pane details never render");
        let name_row = text
            .lines()
            .position(|line| line.contains("research"))
            .unwrap();
        assert!(
            text.lines().nth(name_row + 1).unwrap().contains("working"),
            "the status renders directly under the name"
        );

        let tags = stage_tags(&app);
        assert_eq!(tags.len(), 1, "only the named planet owns a tag");
        assert_eq!(
            app.active_agents()[tags[0].agent_index].name.as_deref(),
            Some("research"),
            "the tag annotates exactly the named agent"
        );
    }

    #[test]
    fn selected_tag_draws_last_and_tag_layout_avoids_discs_and_tags() {
        let mut app = collage_app(
            (0..8)
                .map(|i| {
                    snap(
                        "ws",
                        &format!("p{i}"),
                        Some(&format!("agent-{i}")),
                        AgentStatus::Working,
                    )
                })
                .collect(),
        );
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let tags = stage_tags(&app);
        assert_eq!(tags.len(), 8, "every named planet keeps a tag");

        let tag = tags
            .iter()
            .find(|tag| tag.selected)
            .expect("the selection owns a tag");
        assert_eq!(
            cell_text(&buf, tag.rect.x, tag.rect.y),
            tag.name.chars().next().unwrap().to_string(),
            "the selected tag draws on top at its placement"
        );

        // Non-fallback tags overlap no disc and no other tag.
        let field = agent_stage_layout(CANVAS).field;
        let layout = collage_layout(app.active_agents(), app.viz(), &[], field);
        let disc_cells: HashSet<(u16, u16)> = layout
            .tiles
            .iter()
            .flat_map(|tile| {
                planet_geometry(tile, field, AgentStatus::Working, app.viz()).hit_cells
            })
            .collect();
        let tag_cells = |tag: &PlanetTag| -> Vec<(u16, u16)> {
            (tag.rect.y..tag.rect.y + tag.rect.height)
                .flat_map(|y| (tag.rect.x..tag.rect.x + tag.rect.width).map(move |x| (x, y)))
                .collect()
        };
        let mut seen: HashSet<(u16, u16)> = HashSet::new();
        for tag in &tags {
            for cell in tag_cells(tag) {
                assert!(
                    !disc_cells.contains(&cell),
                    "tag cell {cell:?} of {} must avoid every disc",
                    tag.name
                );
                assert!(
                    seen.insert(cell),
                    "tag cell {cell:?} of {} must avoid every other tag",
                    tag.name
                );
            }
        }
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
