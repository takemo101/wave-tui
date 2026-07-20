//! Planet surface and interior status treatment.
//!
//! Turns one laid-out tile from [`super::geometry`] into the cells a planet
//! draws: its disc body, its stable Banded Worlds identity surface, the
//! interior status cells for the current played frame, the selected planet's
//! corner focus brackets, and its explicit-name label candidate.
//!
//! Two rules shape this module. Status is interior-only — every status cell
//! is an existing body cell, so nothing state-derived ever draws outside the
//! disc mask. And every status animation derives from the played phase
//! signature plus the identity seed, never a wall clock, so identical frames
//! freeze the treatment in place by construction.

use ratatui::{layout::Rect, text::Line};

use super::geometry::{disc_geometry, rect_contains, CollageTile, DiscGeometry, DiscMask};
use crate::herdr::AgentStatus;
use crate::model::{PhaseTrace, VizFrame};

/// Glyph filling a planet's round body.
pub(super) const PLANET_BODY_GLYPH: &str = "▓";

/// Glyph shading a stable seed-derived crater inside a planet body.
pub(super) const CRATER_GLYPH: &str = "░";
/// Body cells in Working's narrow bright interior surface band.
pub(super) const WORKING_BAND: usize = 3;
/// Minimum body cells before an interior status cell may appear: the
/// one-cell disc keeps its body but no status detail.
pub(super) const STATUS_MIN_BODY: usize = 2;
/// Minimum body cells before optional crater detail appears.
pub(super) const CRATER_MIN_BODY: usize = 6;

/// Deterministic quantization of the primary-phase coordinates: identical
/// frames yield identical signatures, and elapsed time never contributes.
pub(super) fn phase_signature(trace: &PhaseTrace) -> u64 {
    trace.pairs().fold(0u64, |acc, (x, y)| {
        let qx = ((x + 1.0) * 127.0).round() as u64;
        let qy = ((y + 1.0) * 127.0).round() as u64;
        acc.rotate_left(7) ^ qx.wrapping_mul(0x9E37_79B9).wrapping_add(qy)
    })
}

/// One selection focus bracket: a corner cell of the selected planet's
/// tile and its corner glyph. Decorative only, never a hit target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FocusBracket {
    pub(super) cell: (u16, u16),
    pub(super) glyph: &'static str,
}

/// One explicit display-name label constrained to a planet tile.
pub(super) struct PlanetLabel {
    pub(super) rect: Rect,
    pub(super) text: String,
}

/// One agent's planet in field cells, derived purely from its tile, status,
/// selection, and the current phase frame — every field follows the
/// audio-transformed `rect`. Only the Working band and Blocked pulse
/// consult phase data — every other cell keeps still geometry across audio
/// frames.
pub(super) struct PlanetGeometry {
    /// The fixed mask this planet's slot chose.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) mask: DiscMask,
    pub(super) body: Vec<(u16, u16)>,
    pub(super) craters: Vec<(u16, u16)>,
    /// Working's narrow bright band of existing body cells for this frame;
    /// empty for every other status and on one-cell discs. Silence freezes
    /// it in place — it never disappears.
    pub(super) status_band: Vec<(u16, u16)>,
    /// Blocked's single weakly pulsing interior error cell — an existing
    /// crater/surface cell; `None` for every other status and on one-cell
    /// discs.
    pub(super) error_cell: Option<(u16, u16)>,
    /// Whether Blocked's weak pulse sits in its bright half for this frame.
    /// Silence freezes it at its last played value.
    pub(super) error_lift: bool,
    /// Four corner focus brackets; empty unless this planet is selected.
    pub(super) brackets: Vec<FocusBracket>,
    /// Only visible disc body cells select a planet; brackets are
    /// decorative.
    pub(super) hit_cells: Vec<(u16, u16)>,
}

/// The three Banded Worlds surface families. A family is stable private
/// identity language — never a status, audio, time, or selection signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanetSurface {
    BandedGas,
    IceCap,
    CrateredRock,
}

/// The two stable spectrum-gradient positions a planet identity owns: the
/// body paints `base_position`, the surface pattern paints
/// `accent_position`. Both resolve through the active theme's
/// `spectrum_color`, so no fixed palette value ever appears.
#[derive(Clone, Copy)]
pub(super) struct PlanetPalette {
    pub(super) base_position: f32,
    pub(super) accent_position: f32,
}

/// The stable identity-seeded surface family.
pub(super) fn planet_surface(seed: u64) -> PlanetSurface {
    match seed % 3 {
        0 => PlanetSurface::BandedGas,
        1 => PlanetSurface::IceCap,
        _ => PlanetSurface::CrateredRock,
    }
}

/// The stable identity-seeded palette pair: distinct base and accent
/// positions on the theme spectrum gradient.
pub(super) fn planet_palette(seed: u64) -> PlanetPalette {
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
pub(super) fn surface_cells(
    surface: PlanetSurface,
    geometry: &PlanetGeometry,
    seed: u64,
) -> Vec<(u16, u16)> {
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
pub(super) fn decorative_cell(
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

/// The interior surface-status cells of one planet for the current frame.
pub(super) struct SurfaceStatus {
    pub(super) band: Vec<(u16, u16)>,
    pub(super) error_cell: Option<(u16, u16)>,
    pub(super) error_lift: bool,
}

/// The played frame that drives interior status treatment for a captured
/// (stale or low-power) composition, and the live fallback while the App
/// holds no audible capture yet. An audible current frame drives it
/// directly; a silent one freezes the treatment on the most recent audible
/// frame still in the handed-in history. With no audible frame in reach the
/// silent frame itself is the rest source — analyzer silence repeats a
/// stable frame, so the treatment stays still by construction. Live
/// rendering prefers `App::status_viz`, which survives silence beyond the
/// bounded history.
pub(super) fn status_frame<'a>(frame: &'a VizFrame, history: &'a [VizFrame]) -> &'a VizFrame {
    if frame.is_audible() {
        return frame;
    }
    history.iter().find(|old| old.is_audible()).unwrap_or(frame)
}

/// Choose the interior status cells from the existing disc body — status
/// never draws outside the mask. Every animation state derives only from
/// the played phase signature plus the identity seed — never wall-clock —
/// so identical frames freeze the treatment in place: silence, stale, and
/// `--low-power` hold the last played-frame treatment via [`status_frame`]
/// and the App frame captures instead of suppressing or dimming it.
/// Working keeps a narrow bright band through the body cells that advances
/// on each newly played frame; Blocked weakly pulses one stable seed-chosen
/// crater/surface cell in the error color; Idle, Done, and Unknown keep no
/// status cells at all. Bodies under [`STATUS_MIN_BODY`] cells — the
/// one-cell disc — keep no status detail.
pub(super) fn surface_status(
    tile: &CollageTile,
    body: &[(u16, u16)],
    status: AgentStatus,
    frame: &VizFrame,
) -> SurfaceStatus {
    if body.len() < STATUS_MIN_BODY {
        return SurfaceStatus {
            band: Vec::new(),
            error_cell: None,
            error_lift: false,
        };
    }
    let len = body.len() as u64;
    let phase = phase_signature(frame.primary_phase()).wrapping_add(tile.seed);
    let band = if status == AgentStatus::Working {
        let start = (phase % len) as usize;
        (0..WORKING_BAND.min(body.len() - 1))
            .map(|step| body[(start + step) % body.len()])
            .collect()
    } else {
        Vec::new()
    };
    let error_cell = (status == AgentStatus::Blocked).then(|| body[(tile.seed % len) as usize]);
    let error_lift = status == AgentStatus::Blocked && (phase ^ (phase >> 5)) % 5 < 2;
    SurfaceStatus {
        band,
        error_cell,
        error_lift,
    }
}

/// The selected planet's four corner focus brackets: the tile's own corner
/// cells, so the brackets surround the allocated disc area, bounded to the
/// tile by construction. A corner that would crowd the body gap or leave
/// the field is dropped.
pub(super) fn focus_brackets(
    tile: &CollageTile,
    disc: &DiscGeometry,
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
            Some(FocusBracket { cell, glyph })
        })
        .collect()
}

/// Derive one planet's body, interior status cells, and — for the selected
/// planet only — focus brackets from its tile. Craters and the identity
/// surface stay stable; only the Working band and Blocked pulse follow the
/// played phase frame.
pub(super) fn planet_label_candidate(
    tile: &CollageTile,
    geometry: &PlanetGeometry,
    name: Option<&str>,
) -> Option<PlanetLabel> {
    let name: String = name?
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if name.is_empty() || tile.rect.width == 0 || tile.rect.height == 0 {
        return None;
    }

    let ellipsis_width = Line::raw("…").width() as u16;
    let available_width = tile.rect.width;
    if available_width < ellipsis_width {
        return None;
    }
    let mut text = String::new();
    for character in name.chars() {
        let mut next = text.clone();
        next.push(character);
        if Line::raw(next.as_str()).width() as u16 > available_width {
            break;
        }
        text = next;
    }
    if text.len() < name.len() {
        while Line::raw(format!("{text}…")).width() as u16 > available_width {
            text.pop();
        }
        text.push('…');
    }
    let width = Line::raw(text.as_str()).width() as u16;
    let y = geometry
        .body
        .iter()
        .map(|&(_, y)| y)
        .max()?
        .saturating_add(1);
    if y >= tile.rect.y + tile.rect.height {
        return None;
    }
    let x = tile.rect.x + tile.rect.width.saturating_sub(width) / 2;
    let rect = Rect::new(x, y, width, 1);
    (rect.x >= tile.rect.x
        && rect.y >= tile.rect.y
        && rect.x + rect.width <= tile.rect.x + tile.rect.width
        && rect.y + rect.height <= tile.rect.y + tile.rect.height)
        .then_some(PlanetLabel { rect, text })
}

pub(super) fn planet_geometry(
    tile: &CollageTile,
    area: Rect,
    status: AgentStatus,
    frame: &VizFrame,
    selected: bool,
) -> PlanetGeometry {
    let disc = disc_geometry(tile.mask, tile.rect, area);
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
    let SurfaceStatus {
        band: status_band,
        error_cell,
        error_lift,
    } = surface_status(tile, &body, status, frame);
    let brackets = if selected {
        focus_brackets(tile, &disc, area)
    } else {
        Vec::new()
    };
    PlanetGeometry {
        mask: disc.mask,
        body: body.clone(),
        craters,
        status_band,
        error_cell,
        error_lift,
        brackets,
        hit_cells: body,
    }
}
