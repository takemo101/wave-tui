//! Pure Agent Planets geometry: the phase-scope trace layers, the
//! seed-derived solar orbits, the fixed disc masks, and the stage
//! partition.
//!
//! Nothing here reads [`crate::app::App`], the active [`crate::theme::Theme`],
//! or a wall clock, and nothing draws. Every function is a deterministic
//! function of its arguments, so the renderer and [`super::hit_test`] can
//! call the same geometry and resolve a click against exactly the cells that
//! were drawn. Freezing — stale or `--low-power` — is expressed by the caller
//! handing in captured frames and frozen orbit seconds, never by a flag here.

use ratatui::layout::{Constraint, Layout, Rect};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::app::AgentView;
use crate::model::{PhaseTrace, VizFrame};

/// Below this energy/magnitude the scope counts as silent: dim and still.
/// Frames at or below this RMS draw no phase trace or persistence, because
/// analyzer silence still carries non-empty all-zero traces. Shared with the
/// App's low-power capture audibility gate.
pub(super) const SILENCE_ENERGY: f32 = crate::model::SILENCE_RMS;
/// Above this energy a working frame's edge glow brightens to bold and the
/// live traces shed their dim modifier.
pub(super) const BRIGHT_ENERGY: f32 = 0.6;
/// Maximum dim phosphor-persistence layers taken from recent history frames.
pub(super) const PERSISTENCE_LAYERS: usize = 2;
/// Upper bound on a frame's base width so sparse canvases stay tile-like.
pub(super) const TILE_MAX_W: u16 = 18;
/// Upper bound on a frame's base height so sparse canvases stay tile-like.
pub(super) const TILE_MAX_H: u16 = 9;
/// Spectrum-gradient position of the primary phase trace: the theme's main
/// visualizer color.
pub(super) const PRIMARY_TRACE_POSITION: f32 = 0.85;
/// Spectrum-gradient position of the secondary trace: the complementary
/// visualizer color.
pub(super) const SECONDARY_TRACE_POSITION: f32 = 0.3;
/// Glyph plotting one primary-trace phase point.
pub(super) const PRIMARY_TRACE_GLYPH: &str = "•";
/// Glyph plotting one secondary-trace phase point.
pub(super) const SECONDARY_TRACE_GLYPH: &str = "◦";
/// Glyph plotting one dim phosphor-persistence point.
pub(super) const PERSISTENCE_GLYPH: &str = "·";

/// Glyph of the single static, theme-derived sun at the field center.
pub(super) const SUN_GLYPH: &str = "☀";
/// Slowest seed-derived orbit period, in seconds per full turn.
pub(super) const ORBIT_SLOWEST_SECS_PER_TURN: u64 = 240;
/// Fastest seed-derived orbit period, in seconds per full turn.
pub(super) const ORBIT_FASTEST_SECS_PER_TURN: u64 = 90;
/// Cells kept clear between the sun and a planet body's nearest cell.
pub(super) const SUN_BODY_GAP: u16 = 2;
/// Margin around a planet's disc mask forming its tile: the corner focus
/// brackets sit on this frame, gapped off the body.
pub(super) const PLANET_FRAME_MARGIN: u16 = 2;

/// Normalized breathing vignette ring radius at silence.
pub(super) const VIGNETTE_BASE: f32 = 0.62;
/// How far full RMS pushes the vignette ring outward.
pub(super) const VIGNETTE_SWING: f32 = 0.3;
/// Half-thickness of the vignette ring in normalized distance.
pub(super) const VIGNETTE_BAND: f32 = 0.05;

/// One plotted phase-portrait point in canvas cells.
pub(super) struct PhaseCell {
    pub(super) x: u16,
    pub(super) y: u16,
}

/// One phase-trace layer behind the agent frames: its plotted cells, its
/// position on the theme's spectrum gradient, its glyph, and whether it
/// renders dimmed (persistence, or a quiet live trace).
pub(super) struct PhaseLayer {
    pub(super) cells: Vec<PhaseCell>,
    pub(super) glyph: &'static str,
    pub(super) color_position: f32,
    pub(super) dim: bool,
}

/// One placed agent planet: the index into `App::active_agents()`, its
/// stable identity seed, its fixed disc mask, the tile rectangle framing the
/// mask on its orbit position, and its energy (used only for quiet identity
/// dimming — never for motion).
pub(super) struct CollageTile {
    pub(super) index: usize,
    pub(super) seed: u64,
    /// The fixed disc mask this planet's density slot chose.
    pub(super) mask: DiscMask,
    /// The planet's tile: the mask footprint plus [`PLANET_FRAME_MARGIN`],
    /// centered on the orbit position.
    pub(super) rect: Rect,
    pub(super) energy: f32,
}

/// The music background behind the frames: the dual phase-scope layers and
/// the normalized breathing vignette ring radius.
pub(super) struct CollageBackground {
    pub(super) layers: Vec<PhaseLayer>,
    pub(super) vignette: f32,
}

/// Pure solar-system geometry shared by rendering and hit testing: the
/// scope background, the static field-centered sun cell, and the planets.
pub(super) struct CollageLayout {
    pub(super) background: CollageBackground,
    /// The sun's cell; `None` only when the field has no interior at all.
    pub(super) sun: Option<(u16, u16)>,
    pub(super) tiles: Vec<CollageTile>,
}

/// Stable placement seed for an agent: a hash of its private identity, so a
/// status change never moves a frame and no pane detail is exposed.
pub(super) fn seed_of(view: &AgentView) -> u64 {
    let mut hasher = DefaultHasher::new();
    view.id.hash(&mut hasher);
    hasher.finish()
}

/// The agent's assigned FFT band, by identity; zero when the frame is empty.
pub(super) fn band_of(seed: u64, bands: &[f32]) -> f32 {
    if bands.is_empty() {
        0.0
    } else {
        bands[seed as usize % bands.len()]
    }
}

/// The seed-derived orbit period in seconds per full turn: deliberately
/// slow and bounded, so planets drift rather than spin.
pub(super) fn orbit_secs_per_turn(seed: u64) -> f32 {
    let span = ORBIT_SLOWEST_SECS_PER_TURN - ORBIT_FASTEST_SECS_PER_TURN + 1;
    (ORBIT_FASTEST_SECS_PER_TURN + (seed >> 7) % span) as f32
}

/// The seed-derived initial orbit angle, in turns.
pub(super) fn orbit_initial_turns(seed: u64) -> f32 {
    ((seed >> 17) % 1024) as f32 / 1024.0
}

/// The seed-derived position between the smallest and largest orbit radius
/// the field can offer.
pub(super) fn orbit_radius_fraction(seed: u64) -> f32 {
    ((seed >> 33) % 997) as f32 / 996.0
}

/// Clamp a proposed rectangle into `area`, keeping at least one cell.
pub(super) fn clamp_rect(x: i32, y: i32, width: u16, height: u16, area: Rect) -> Rect {
    let width = width.clamp(1, area.width);
    let height = height.clamp(1, area.height);
    let x = x.clamp(area.x as i32, (area.x + area.width - width) as i32) as u16;
    let y = y.clamp(area.y as i32, (area.y + area.height - height) as i32) as u16;
    Rect::new(x, y, width, height)
}

/// Map a normalized phase coordinate in `-1.0..=1.0` to a centered column:
/// zero is exactly the horizontal middle of `area`.
pub(super) fn phase_x(area: Rect, value: f32) -> u16 {
    let half = area.width.saturating_sub(1) as f32 / 2.0;
    let x = (area.x as f32 + half + value * half).round() as i32;
    x.clamp(
        area.x as i32,
        (area.x + area.width).saturating_sub(1) as i32,
    ) as u16
}

/// Map a normalized phase coordinate in `-1.0..=1.0` to a centered row,
/// inverted so positive values plot upward like an oscilloscope.
pub(super) fn phase_y(area: Rect, value: f32) -> u16 {
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
pub(super) fn phase_cells(trace: &PhaseTrace, area: Rect) -> Vec<PhaseCell> {
    trace
        .pairs()
        .map(|(x, y)| PhaseCell {
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
pub(super) fn phase_layers(frame: &VizFrame, history: &[VizFrame], area: Rect) -> Vec<PhaseLayer> {
    let mut layers = Vec::new();
    for old in history.iter().take(PERSISTENCE_LAYERS).rev() {
        if old.rms() <= SILENCE_ENERGY {
            continue;
        }
        layers.push(PhaseLayer {
            cells: phase_cells(old.primary_phase(), area),
            glyph: PERSISTENCE_GLYPH,
            color_position: PRIMARY_TRACE_POSITION,
            dim: true,
        });
    }
    if frame.rms() > SILENCE_ENERGY {
        // RMS gently brightens the live traces; it never adds motion.
        let dim = frame.rms() <= BRIGHT_ENERGY;
        layers.push(PhaseLayer {
            cells: phase_cells(frame.secondary_phase(), area),
            glyph: SECONDARY_TRACE_GLYPH,
            color_position: SECONDARY_TRACE_POSITION,
            dim,
        });
        layers.push(PhaseLayer {
            cells: phase_cells(frame.primary_phase(), area),
            glyph: PRIMARY_TRACE_GLYPH,
            color_position: PRIMARY_TRACE_POSITION,
            dim,
        });
    }
    layers
}

/// Compute the full solar-system geometry for `agents` inside `area`.
///
/// Deterministic given its inputs: each agent's orbit radius, initial angle,
/// and slow angular speed derive only from its identity hash and the field
/// size, and its position adds `orbit_secs[index]` — that agent's elapsed
/// Working seconds from the reducer — worth of rotation (missing entries
/// read as zero). Audio contributes the phase layers, the vignette, and the
/// per-planet energy only; it never scales, offsets, or otherwise moves a
/// planet. Orbit paths never produce cells of their own. Freezing — stale or
/// low power — is done by the caller handing in captured frames and frozen
/// orbit seconds, never by a flag here.
///
/// Dense fields shrink masks before radii and drop a body only when even the
/// one-cell disc cannot keep [`SUN_BODY_GAP`] off the centered sun — the sun
/// itself is never dropped.
pub(super) fn collage_layout(
    agents: &[AgentView],
    orbit_secs: &[f32],
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
            sun: None,
            tiles: Vec::new(),
        };
    }

    let background = CollageBackground {
        layers: phase_layers(frame, history, area),
        vignette: VIGNETTE_BASE + frame.rms().clamp(0.0, 1.0) * VIGNETTE_SWING,
    };
    let sun = (area.x + area.width / 2, area.y + area.height / 2);
    // The largest orbit half-extents the field allows around the sun cell.
    let ext_x = (sun.0 - area.x).min(area.x + area.width - 1 - sun.0);
    let ext_y = (sun.1 - area.y).min(area.y + area.height - 1 - sun.1);

    // Stable, status-independent draw order.
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
            sun: Some(sun),
            tiles: Vec::new(),
        };
    }

    // Density-derived mask cap: the same per-agent slot sizing the grid
    // layout used, kept purely to shrink disc masks as agents multiply.
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
    let tile_w = (((w / cols) * 2 / 3).max(1) as u16).min(TILE_MAX_W);
    let tile_h = (((h / rows) * 2 / 3).max(1) as u16).min(TILE_MAX_H);

    let tiles = order
        .into_iter()
        .filter_map(|(seed, index)| {
            // The largest density-capped mask whose orbit band still fits:
            // radii keep the body clear of the sun gap and hold the whole
            // tile inside the field. Masks fall through before any body is
            // dropped; a body drops only when even the one-cell disc cannot
            // keep the gap.
            let mut mask = DiscMask::for_bound(tile_w, tile_h);
            let (min_rx, max_rx, min_ry, max_ry) = loop {
                let half_mask_w = mask.width() / 2;
                let half_mask_h = mask.height() / 2;
                let min_rx = (half_mask_w + SUN_BODY_GAP) as f32;
                let max_rx = ext_x as f32 - (half_mask_w + PLANET_FRAME_MARGIN) as f32;
                let min_ry = (half_mask_h + SUN_BODY_GAP) as f32;
                let max_ry = ext_y as f32 - (half_mask_h + PLANET_FRAME_MARGIN) as f32;
                if max_rx >= min_rx && max_ry >= min_ry {
                    break (min_rx, max_rx, min_ry, max_ry);
                }
                mask = mask.smaller()?;
            };

            // The seed-derived circular orbit, drawn as an ellipse in cell
            // space so 2:1 terminal cells keep it visually round; only the
            // caller-supplied Working seconds advance the angle.
            let fraction = orbit_radius_fraction(seed);
            let rx = min_rx + fraction * (max_rx - min_rx);
            let ry = min_ry + fraction * (max_ry - min_ry);
            let secs = orbit_secs.get(index).copied().unwrap_or(0.0);
            let turns = orbit_initial_turns(seed) + secs / orbit_secs_per_turn(seed);
            let angle = turns * std::f32::consts::TAU;
            let center_x = (sun.0 as f32 + rx * angle.cos()).round() as i32;
            let center_y = (sun.1 as f32 + ry * angle.sin()).round() as i32;
            let rect = clamp_rect(
                center_x - (mask.width() / 2 + PLANET_FRAME_MARGIN) as i32,
                center_y - (mask.height() / 2 + PLANET_FRAME_MARGIN) as i32,
                mask.width() + 2 * PLANET_FRAME_MARGIN,
                mask.height() + 2 * PLANET_FRAME_MARGIN,
                area,
            );

            let band = band_of(seed, frame.bands());
            let energy = (frame.rms() * 0.55 + band * 0.45).clamp(0.0, 1.0);

            Some(CollageTile {
                index,
                seed,
                mask,
                rect,
                energy,
            })
        })
        .collect();

    CollageLayout {
        background,
        sun: Some(sun),
        tiles,
    }
}

/// The four fixed, terminal-safe disc masks a planet body may use. Explicit
/// row masks — never an equation-derived rectangle/ellipse — keep every
/// planet unmistakably round and free of cross-like silhouettes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiscMask {
    Large7x5,
    Medium5x3,
    Small3x3,
    Dot,
}

/// Explicit 7×5 disc rows; only non-space characters become body cells.
pub(super) const LARGE_DISC: [&str; 5] = ["  ███  ", " █████ ", "███████", " █████ ", "  ███  "];
/// Explicit 5×3 disc rows.
pub(super) const MEDIUM_DISC: [&str; 3] = [" ███ ", "█████", " ███ "];
/// Explicit 3×3 disc rows.
pub(super) const SMALL_DISC: [&str; 3] = [" █ ", "███", " █ "];
/// The one-cell fallback disc for the densest fields.
pub(super) const DOT_DISC: [&str; 1] = ["█"];

impl DiscMask {
    /// The largest mask whose fixed footprint fits a `width`×`height` slot:
    /// dense fields fall through 7×5 → 5×3 → 3×3 → one cell.
    pub(super) fn for_bound(width: u16, height: u16) -> DiscMask {
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

    /// The next smaller fixed mask, down to the one-cell disc.
    pub(super) fn smaller(self) -> Option<DiscMask> {
        match self {
            DiscMask::Large7x5 => Some(DiscMask::Medium5x3),
            DiscMask::Medium5x3 => Some(DiscMask::Small3x3),
            DiscMask::Small3x3 => Some(DiscMask::Dot),
            DiscMask::Dot => None,
        }
    }

    pub(super) fn rows(self) -> &'static [&'static str] {
        match self {
            DiscMask::Large7x5 => &LARGE_DISC,
            DiscMask::Medium5x3 => &MEDIUM_DISC,
            DiscMask::Small3x3 => &SMALL_DISC,
            DiscMask::Dot => &DOT_DISC,
        }
    }

    pub(super) fn width(self) -> u16 {
        self.rows()[0].chars().count() as u16
    }

    pub(super) fn height(self) -> u16 {
        self.rows().len() as u16
    }
}

/// One placed disc: its chosen mask, its top-left mask origin (kept signed
/// so decoration overhang can clip at the field edge), and its body cells
/// inside `area`.
pub(super) struct DiscGeometry {
    pub(super) mask: DiscMask,
    pub(super) origin: (i32, i32),
    pub(super) body: Vec<(u16, u16)>,
}

/// Center `mask` inside `bound` and convert only non-space mask characters
/// into body cells clipped to `area`. The mask is the tile's own fixed
/// choice, so an oversized or margin-framed bound never changes the disc.
pub(super) fn disc_geometry(mask: DiscMask, bound: Rect, area: Rect) -> DiscGeometry {
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

pub(super) fn rects_overlap(left: Rect, right: Rect) -> bool {
    left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
}

/// The centered Agent Planets stage partitions: heading, title block
/// (current title with the Single View volume line beneath it), scope/planet
/// field, and footer rows.
pub(super) struct AgentStageLayout {
    pub(super) heading: Rect,
    pub(super) title_block: Rect,
    pub(super) field: Rect,
    pub(super) footer: Rect,
}

/// Partition the stage. The field takes every flexible row; the title block
/// keeps a second title row only when the terminal is tall enough, so small
/// stages retain a positive field.
pub(super) fn agent_stage_layout(area: Rect) -> AgentStageLayout {
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
pub(super) fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}
