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
//! prior frames in `App::viz_history()`. Over the scope sits a quiet solar
//! system: one static theme-derived sun at the field center (decoration,
//! never a hit target) with every agent planet on its own seed-derived
//! invisible circular orbit — radius, initial angle, and slow bounded
//! angular speed all derive from the identity hash, and no orbit guide
//! line ever renders. Only Working planets move: their phase advances with
//! the elapsed monotonic Working time the reducer tracks, so a
//! Working→non-Working transition freezes a planet at its current angle
//! and a later Working stretch resumes from it. Audio never scales,
//! offsets, or otherwise transforms a planet body. The renderer draws each
//! body from one of four explicit disc masks — 7×5, 5×3, 3×3, or a
//! single cell — never a calculated rectangle/ellipse silhouette and never
//! a full-tile shadow. Dense fields shrink masks (and orbit radii scale to
//! the field) without omitting agents; only when even the one-cell disc
//! cannot keep a gap off the sun is that body dropped — never the sun.
//! Each identity owns a stable Banded Worlds surface
//! (banded gas, ice cap, or cratered rock) painted with two theme spectrum
//! colors inside the mask; the surface's palette is identity language and
//! never varies with status.
//! Status is quiet interior surface language and never draws outside the
//! disc mask: it reuses existing body/surface cells in active-theme
//! colors, and every change derives only from the played phase frame plus
//! the identity seed, never wall-clock. Working advances a narrow bright
//! identity-surface band through the body cells on each newly played
//! frame; Blocked weakly pulses one existing crater/surface cell in the
//! theme error color; Idle stays still and muted; Done keeps the whole
//! body dim; Unknown stays muted and nearly still — never any cross-like
//! glyph, ring, particle, or exterior decoration. One-cell discs keep
//! their body but omit status detail entirely. The
//! selected planet alone gains four corner focus brackets bounded to its
//! tile — decoration, never a hit target. Apart from Working orbit motion,
//! nothing moves from a timer: identical frames at identical orbit phases
//! render identical cells. A frame at or
//! below the silence threshold draws no trace or persistence at all —
//! analyzer silence carries non-empty all-zero traces that would otherwise
//! pile a point cluster at the field center — so silence stays calm, dim,
//! and still. Stale renders the reducer-captured final composition dimmed —
//! the reducer freezes Working orbit phases at the same edge; `--low-power`
//! renders the App-captured first frame and the frozen orbit phases so
//! trace, disc, and bracket geometry stay frozen while state colors keep
//! refreshing. Unavailable hides the sun and planets entirely.
//!
//! Mouse input flows through [`hit_test`], which calls the very same
//! [`collage_layout`] and [`planet_geometry`] the renderer does — that
//! sharing is why a click resolves against exactly the disc body cells that
//! were drawn (scope, vignette, brackets, and empty cells resolve nothing) —
//! and returns only the read-only selection [`Action`]; the CLI event loop
//! owns applying it.
//!
//! The rendering itself lives in focused child modules: [`geometry`] holds
//! the pure scope/orbit/disc layout that drawing and hit testing share,
//! [`surface`] turns one laid-out tile into body, identity-surface, interior
//! status, and bracket cells, and [`stage`] and [`modal`] draw them. This
//! file stays the narrow facade: the summary line, the two entry points the
//! rest of the UI calls, and the App-to-geometry adaptation both entry
//! points share.
//!
//! Privacy: the stage shows the explicit Herdr agent `name` only. No pane
//! id, workspace id, cwd, or agent type is ever rendered. All colors come
//! from the active [`Theme`]; no palette values are added.

mod geometry;
mod modal;
mod stage;
mod surface;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Widget},
};
use std::time::Instant;

use crate::app::{Action, AgentPulseConnection, AgentView, App};
use crate::theme::Theme;

use geometry::{agent_stage_layout, collage_layout, rect_contains};
use surface::planet_geometry;

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

// --- shared App adaptation --------------------------------------------------

/// Whether the full-screen canvas exists at all: overlay open, integration
/// visible, and Signal View (which keeps its own input/display contract)
/// inactive.
fn canvas_active(app: &App) -> bool {
    app.is_agent_overlay_open()
        && !app.is_signal_view()
        && app.agent_pulse_connection() != AgentPulseConnection::Hidden
}

/// The per-agent orbit seconds behind a composition at `now`. Low power
/// prefers the App-captured frozen layout — every planet, an active Working
/// stretch included, holds the angle captured at low-power entry, and agents
/// unknown to the capture rest at phase zero — falling back to the live
/// effective Working time until a capture exists. Live and stale read the
/// live time directly: stale phases were banked by the reducer at the
/// Connected→Stale edge, so they hold still on their own.
fn orbit_secs_for(app: &App, agents: &[AgentView], low_power: bool, now: Instant) -> Vec<f32> {
    agents
        .iter()
        .map(|view| {
            if low_power {
                if let Some(secs) = app.low_power_orbit_secs(&view.id) {
                    return secs;
                }
            }
            app.agent_orbit_secs(&view.id, now)
        })
        .collect()
}

// --- hit testing ------------------------------------------------------------

/// Pure mouse hit test for the Dual Phase Scope canvas.
///
/// Maps a click on a planet's drawn disc body cells — the exact
/// [`planet_geometry`] hit cells the renderer draws — to the read-only
/// [`Action::SelectAgent`]; returns `None` whenever the canvas is closed, the
/// integration is hidden, the connection is stale or unavailable, Signal View
/// is active, or the click misses every planet body. Scope phase, vignette,
/// sun, bracket, and empty cells resolve nothing. Overlapping planets
/// resolve
/// topmost-first, with the selected planet in front, matching draw order.
/// `low_power` must mirror the render flag: it resolves against the
/// App-captured frozen frame and frozen orbit layout exactly as
/// [`render_canvas`] draws them (hit testing is Connected-only, so the
/// stale capture never applies here). `now` must be the same instant handed
/// to [`render_canvas`], so a click resolves against the exact orbit
/// positions that were drawn — live positions normally, the frozen captured
/// layout in low power.
pub(super) fn hit_test(
    area: Rect,
    column: u16,
    row: u16,
    low_power: bool,
    now: Instant,
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
    let orbit_secs = orbit_secs_for(app, agents, low_power, now);
    let canvas = agent_stage_layout(area).field;
    let layout = collage_layout(agents, &orbit_secs, frame, &[], canvas);
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

// --- rendering facade -------------------------------------------------------

/// Render the full-screen Agent Planets stage over the composed layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the centered stage chrome
/// (heading, the current ICY/station title with the exact Single View
/// volume line beneath it, and footer) and, inside the stage field, the
/// breathing vignette, the phosphor-persistence and dual phase-trace
/// layers, and each planet in ordered passes — disc-mask body with its
/// Banded Worlds surface, its interior status cells, then the selected
/// planet's four corner focus brackets — with the selected planet drawn
/// last. Stale
/// renders the reducer-captured final composition dimmed under a
/// `reconnecting` note, its orbit phases banked by the reducer at the same
/// edge; Unavailable hides the sun, field, and tags behind calm
/// copy; `--low-power` renders the App-captured first frame so scope and
/// status geometry stay frozen while state colors refresh, and holds every
/// planet at the orbit angle the App captured with that frame — the whole
/// solar layout freezes at low-power entry. `now` is the monotonic render
/// instant and feeds exactly one thing: the elapsed Working time advancing
/// current Working orbit phases in live rendering — scope traces and every
/// other cell still derive from audio frames only.
pub(super) fn render_canvas(
    app: &App,
    theme: &Theme,
    low_power: bool,
    now: Instant,
    area: Rect,
    buf: &mut Buffer,
) {
    if !canvas_active(app) || area.width < 8 || area.height < 5 {
        return;
    }
    Clear.render(area, buf);
    buf.set_style(area, theme.base_style());
    stage::render_agent_planets_stage(app, theme, low_power, now, area, buf);
}

#[cfg(test)]
mod tests {
    use super::geometry::{
        agent_stage_layout, collage_layout, disc_geometry, phase_cells, CollageLayout, CollageTile,
        DiscMask, PERSISTENCE_GLYPH, PRIMARY_TRACE_GLYPH, SECONDARY_TRACE_GLYPH, SUN_GLYPH,
        VIGNETTE_BAND, VIGNETTE_BASE,
    };
    use super::modal::{agent_table_modal_area, AGENT_TABLE_WIDTHS};
    use super::surface::{
        planet_geometry, planet_palette, planet_surface, PlanetGeometry, PlanetSurface,
        CRATER_GLYPH, PLANET_BODY_GLYPH, WORKING_BAND,
    };
    use super::*;
    use crate::app::Action;
    use crate::audio::AudioEvent;
    use crate::catalog::Catalog;
    use crate::herdr::{AgentDetails, AgentId, AgentSnapshot, AgentStatus};
    use crate::model::{PhaseTrace, VizFrame};
    use crate::settings::Settings;
    use crate::theme::ThemeName;
    use ratatui::layout::Constraint;
    use ratatui::style::Color;
    use std::collections::HashSet;
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

    /// The solar layout at orbit phase zero: every planet resting on its
    /// seed-derived initial angle, as a freshly seen agent renders.
    fn layout_at_rest(
        agents: &[AgentView],
        frame: &VizFrame,
        history: &[VizFrame],
        area: Rect,
    ) -> CollageLayout {
        collage_layout(agents, &[], frame, history, area)
    }

    /// Glyphs only agent planets (bodies and craters) may use. `·` stays
    /// excluded: the vignette, phosphor persistence, and copy separators
    /// share it.
    const PLANET_GLYPHS: [&str; 2] = [PLANET_BODY_GLYPH, CRATER_GLYPH];

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

    /// Feed a visualizer frame to `app`, starting playback first if needed:
    /// frames are only accepted for the currently expected playback request.
    fn push_frame(app: &mut App, frame: VizFrame) {
        if app.playback_request().is_none() {
            app.apply(Action::PlaySelected(
                crate::model::PlaybackRequestSeq::new().next_id(),
            ));
        }
        let request = app
            .playback_request()
            .expect("playback started for the frame fixture");
        app.apply(Action::Audio(AudioEvent::Viz { request, frame }));
    }

    fn render_collage_for(app: &App, low_power: bool, now: Instant) -> Buffer {
        render_collage_in(app, low_power, now, CANVAS)
    }

    fn render_collage_in(app: &App, low_power: bool, now: Instant, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        let theme = Theme::for_name(ThemeName::Minimal);
        render_canvas(app, &theme, low_power, now, area, &mut buf);
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

    fn stage_footer_text(buf: &Buffer) -> String {
        let footer = agent_stage_layout(CANVAS).footer;
        (footer.y..footer.y + footer.height)
            .flat_map(|y| {
                (footer.x..footer.x + footer.width).map(move |x| buf.cell((x, y)).unwrap().symbol())
            })
            .collect()
    }

    fn count_primary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('•').count()
    }

    fn count_secondary_phase_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('◦').count()
    }

    /// Cells drawn with agent-planet glyphs (bodies, craters, or
    /// craters) inside the stage field, so chrome rows (heading, title
    /// block, and footer) never count as planets.
    fn count_planet_cells(buf: &Buffer) -> usize {
        field_cells(buf)
            .iter()
            .filter(|(_, _, symbol)| PLANET_GLYPHS.contains(&symbol.as_str()))
            .count()
    }

    /// The bright Working-band cells inside the stage field: planet glyphs
    /// drawn with the band's BOLD emphasis.
    fn bold_planet_cells(buf: &Buffer) -> Vec<(u16, u16)> {
        let field = stage_field();
        let mut cells = Vec::new();
        for y in field.y..field.y + field.height {
            for x in field.x..field.x + field.width {
                let cell = buf.cell((x, y)).unwrap();
                if cell.style().add_modifier.contains(Modifier::BOLD)
                    && PLANET_GLYPHS.contains(&cell.symbol())
                {
                    cells.push((x, y));
                }
            }
        }
        cells
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
    fn orbit_position_is_stable_per_identity_and_advances_only_with_orbit_secs() {
        let area = Rect::new(0, 0, 120, 36);
        let agent = view("alpha", "p1", AgentStatus::Working);
        let first = layout_at_rest(
            std::slice::from_ref(&agent),
            &frame(0.0, vec![0.0; 16]),
            &[],
            area,
        );
        let later = layout_at_rest(
            std::slice::from_ref(&agent),
            &phase_frame(),
            &[frame(0.1, vec![0.1; 16])],
            area,
        );
        assert_eq!(first.tiles[0].seed, later.tiles[0].seed);
        assert_eq!(
            first.tiles[0].rect, later.tiles[0].rect,
            "audio frames never move a planet off its orbit position"
        );

        let advanced = collage_layout(
            std::slice::from_ref(&agent),
            &[40.0],
            &frame(0.0, vec![0.0; 16]),
            &[],
            area,
        );
        assert_ne!(
            first.tiles[0].rect, advanced.tiles[0].rect,
            "elapsed Working seconds advance the orbit position"
        );
    }

    #[test]
    fn dense_collage_keeps_one_frame_per_agent() {
        let area = Rect::new(0, 0, 50, 15);
        let layout = layout_at_rest(&agents(80), &frame(0.5, vec![0.5; 16]), &[], area);
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
        let before = layout_at_rest(
            &[view("alpha", "p1", AgentStatus::Working)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
        );
        let after = layout_at_rest(
            &[view("alpha", "p1", AgentStatus::Blocked)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
        );
        assert_eq!(before.tiles[0].rect, after.tiles[0].rect);
    }

    #[test]
    fn frames_differ_for_identical_panes_in_different_workspaces() {
        let layout = layout_at_rest(
            &[
                view("alpha", "p1", AgentStatus::Working),
                view("beta", "p1", AgentStatus::Working),
            ],
            &frame(0.0, vec![0.0; 16]),
            &[],
            CANVAS,
        );
        assert_eq!(layout.tiles.len(), 2);
        assert_ne!(layout.tiles[0].rect, layout.tiles[1].rect);
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
        // Idle planets hold their frozen orbit angles, so with no Working
        // agent the whole stage — scope included — must be time-invariant.
        let mut app = collage_app(vec![
            snap("ws", "p0", None, AgentStatus::Idle),
            snap("ws", "p1", None, AgentStatus::Idle),
        ]);
        push_frame(&mut app, phase_frame());
        assert_eq!(
            render_collage_for(&app, false, Instant::now()),
            render_collage_for(&app, false, Instant::now() + Duration::from_secs(40)),
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
        let persistence = phase_cells(older_phase_frame().primary_phase(), stage_field());
        let grown = persistence.iter().any(|cell| {
            cell_text(&with, cell.x, cell.y) == PERSISTENCE_GLYPH
                && cell_text(&without, cell.x, cell.y) != PERSISTENCE_GLYPH
        });
        assert!(grown, "a real history frame grows dim persistence dots");
    }

    // --- interior surface status and focus brackets --------------------------

    /// The laid-out tile for one agent alone in `area` under `frame`.
    fn tile_for(agent: &AgentView, area: Rect, frame: &VizFrame) -> CollageTile {
        layout_at_rest(std::slice::from_ref(agent), frame, &[], area)
            .tiles
            .remove(0)
    }

    /// A hand-built tile roomy enough for the largest planet: an 18×9
    /// slot holding the centered 7×5 disc with two rows and five columns
    /// of margin, well inside the area.
    fn roomy_tile(area: Rect) -> CollageTile {
        let rect = Rect::new(area.x + 10, area.y + 10, 18, 9);
        CollageTile {
            index: 0,
            seed: 21,
            mask: DiscMask::for_bound(rect.width, rect.height),
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
    fn stable_identity_produces_a_round_body_with_body_only_hit_cells() {
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
        assert_eq!(first.hit_cells, first.body, "only body cells select");
        assert!(
            !first.status_band.is_empty(),
            "a live working planet keeps its interior band"
        );
        for cell in &first.status_band {
            assert!(
                first.body.contains(cell),
                "the band reuses existing body cells"
            );
        }
    }

    /// The planet geometry of `status` on the roomy tile under `offset`
    /// audio, so treatment tests can separate the still body from its
    /// interior status cells.
    fn roomy_status(status: AgentStatus, offset: f32) -> PlanetGeometry {
        let area = Rect::new(0, 0, 120, 36);
        planet_geometry(
            &roomy_tile(area),
            area,
            status,
            &phase_frame_with_offset(offset),
            false,
        )
    }

    /// A deterministic spread of audio-frame offsets for treatment
    /// searches: enough distinct frames that every duty cycle shows both
    /// halves.
    fn offset_sweep() -> impl Iterator<Item = f32> {
        (1..=24).map(|step| step as f32 * 0.13)
    }

    #[test]
    fn status_cells_never_leave_the_disc_body() {
        for status in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            let geometry = roomy_status(status, 0.2);
            let body: HashSet<(u16, u16)> = geometry.body.iter().copied().collect();
            for cell in &geometry.status_band {
                assert!(
                    body.contains(cell),
                    "{status:?} band cell {cell:?} stays on the body"
                );
            }
            if let Some(cell) = geometry.error_cell {
                assert!(
                    body.contains(&cell),
                    "{status:?} error cell {cell:?} stays on the body"
                );
            }
        }
    }

    #[test]
    fn working_band_advances_only_with_the_played_phase_frame() {
        let first = roomy_status(AgentStatus::Working, 0.2);
        let again = roomy_status(AgentStatus::Working, 0.2);
        assert_eq!(
            first.status_band, again.status_band,
            "identical frames keep an identical band"
        );
        let later = roomy_status(AgentStatus::Working, 0.7);
        assert_eq!(first.status_band.len(), WORKING_BAND);
        assert_eq!(later.status_band.len(), WORKING_BAND);
        assert_ne!(
            first.status_band, later.status_band,
            "new played audio advances the band"
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
            first.status_band, shifted.status_band,
            "the identity seed fixes each planet's band phase"
        );
    }

    #[test]
    fn blocked_pulses_one_stable_interior_error_cell() {
        let first = roomy_status(AgentStatus::Blocked, 0.2);
        let cell = first.error_cell.expect("blocked keeps its error cell");
        assert!(
            first.body.contains(&cell),
            "the error cell is an existing body cell"
        );
        assert!(
            first.status_band.is_empty(),
            "blocked never carries a working band"
        );
        assert_eq!(
            first.error_lift,
            roomy_status(AgentStatus::Blocked, 0.2).error_lift,
            "identical frames keep an identical pulse"
        );
        for offset in offset_sweep() {
            assert_eq!(
                roomy_status(AgentStatus::Blocked, offset).error_cell,
                Some(cell),
                "the pulsing cell never moves with audio"
            );
        }
        let pulses: HashSet<bool> = offset_sweep()
            .map(|offset| roomy_status(AgentStatus::Blocked, offset).error_lift)
            .collect();
        assert_eq!(
            pulses,
            HashSet::from([false, true]),
            "the weak pulse takes both halves across played audio"
        );
    }

    #[test]
    fn idle_done_and_unknown_keep_still_status_free_bodies() {
        for status in [AgentStatus::Idle, AgentStatus::Done, AgentStatus::Unknown] {
            let reference = roomy_status(status, 0.2);
            for offset in offset_sweep() {
                let geometry = roomy_status(status, offset);
                assert_eq!(
                    geometry.body, reference.body,
                    "{status:?} body ignores played audio"
                );
                assert!(
                    geometry.status_band.is_empty(),
                    "{status:?} keeps no working band"
                );
                assert!(
                    geometry.error_cell.is_none(),
                    "{status:?} keeps no error cell"
                );
                assert!(!geometry.error_lift, "{status:?} keeps no pulse lift");
            }
        }
    }

    #[test]
    fn silence_freezes_the_working_band_and_blocked_pulse_in_place() {
        let area = Rect::new(0, 0, 120, 36);
        let mut tile = roomy_tile(area);
        tile.energy = 0.0;

        // A silent planet keeps its full interior treatment: the Working
        // band never disappears and the Blocked cell stays visible, each
        // frozen because identical frames derive identical treatment.
        let working = planet_geometry(
            &tile,
            area,
            AgentStatus::Working,
            &silent_phase_frame(),
            false,
        );
        assert_eq!(
            working.status_band.len(),
            WORKING_BAND,
            "a silent Working planet keeps its frozen band"
        );
        let again = planet_geometry(
            &tile,
            area,
            AgentStatus::Working,
            &silent_phase_frame(),
            false,
        );
        assert_eq!(
            working.status_band, again.status_band,
            "repeated silence holds the band still"
        );

        let blocked = planet_geometry(
            &tile,
            area,
            AgentStatus::Blocked,
            &silent_phase_frame(),
            false,
        );
        assert!(
            blocked.error_cell.is_some(),
            "the blocked cell stays visible at silence"
        );
        assert_eq!(
            blocked.error_lift,
            planet_geometry(
                &tile,
                area,
                AgentStatus::Blocked,
                &silent_phase_frame(),
                false
            )
            .error_lift,
            "repeated silence holds the pulse in its frozen half"
        );

        for status in [AgentStatus::Idle, AgentStatus::Done, AgentStatus::Unknown] {
            let geometry = planet_geometry(&tile, area, status, &silent_phase_frame(), false);
            assert!(
                !geometry.body.is_empty(),
                "{status:?} keeps its still body at silence"
            );
            assert!(
                geometry.status_band.is_empty(),
                "{status:?} keeps no band at silence"
            );
        }
    }

    #[test]
    fn silent_frames_hold_the_last_played_band_and_pulse_treatment() {
        // The render path derives interior status from the last audible
        // frame still in the display history, so the moment audio goes
        // silent the band keeps its exact last played position and
        // brightness instead of being suppressed.
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let t0 = Instant::now();
        let played = bold_planet_cells(&render_collage_for(&app, false, t0));
        assert_eq!(
            played.len(),
            WORKING_BAND,
            "sanity: the played frame draws its bright band"
        );

        push_frame(&mut app, silent_phase_frame());
        let silent = bold_planet_cells(&render_collage_for(&app, false, t0));
        assert_eq!(
            silent, played,
            "silence freezes the bright band at its last played cells"
        );

        // The Blocked pulse likewise holds the half it froze in rather
        // than resting dim.
        let mut blocked_app = collage_app(vec![snap("ws", "b", Some("b"), AgentStatus::Blocked)]);
        push_frame(&mut blocked_app, phase_frame());
        let layout = layout_at_rest(
            blocked_app.active_agents(),
            blocked_app.viz(),
            &[],
            stage_field(),
        );
        let geometry = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Blocked,
            &phase_frame(),
            false,
        );
        let pulse = geometry.error_cell.expect("blocked keeps its error cell");
        let dim_at = |app: &App| {
            render_collage_for(app, false, t0)
                .cell(pulse)
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::DIM)
        };
        let played_dim = dim_at(&blocked_app);
        push_frame(&mut blocked_app, silent_phase_frame());
        assert_eq!(
            dim_at(&blocked_app),
            played_dim,
            "silence freezes the pulse in its last played half"
        );
    }

    #[test]
    fn sustained_silence_beyond_the_display_history_holds_the_treatment() {
        // The App-held audible capture — not the bounded display history —
        // is the silence rest source, so the Working band and Blocked pulse
        // hold from the silence edge indefinitely, long after every audible
        // frame has been evicted from the trail, and resume only when real
        // audio returns.
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let t0 = Instant::now();
        let played = bold_planet_cells(&render_collage_for(&app, false, t0));
        assert_eq!(
            played.len(),
            WORKING_BAND,
            "sanity: the played frame draws its bright band"
        );

        for _ in 0..32 {
            push_frame(&mut app, silent_phase_frame());
        }
        assert!(
            app.viz_history().all(|frame| !frame.is_audible()),
            "sanity: sustained silence evicted every audible frame"
        );
        assert_eq!(
            bold_planet_cells(&render_collage_for(&app, false, t0)),
            played,
            "the band holds its last played cells beyond the display history"
        );

        // The Blocked pulse likewise holds the half it froze in across the
        // same sustained silence.
        let mut blocked = collage_app(vec![snap("ws", "b", Some("b"), AgentStatus::Blocked)]);
        push_frame(&mut blocked, phase_frame());
        let layout = layout_at_rest(blocked.active_agents(), blocked.viz(), &[], stage_field());
        let pulse = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Blocked,
            &phase_frame(),
            false,
        )
        .error_cell
        .expect("blocked keeps its error cell");
        let dim_at = |app: &App| {
            render_collage_for(app, false, t0)
                .cell(pulse)
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::DIM)
        };
        let played_dim = dim_at(&blocked);
        for _ in 0..32 {
            push_frame(&mut blocked, silent_phase_frame());
        }
        assert_eq!(
            dim_at(&blocked),
            played_dim,
            "the pulse holds its frozen half beyond the display history"
        );

        // A resumed audible frame drives the treatment again exactly as it
        // would drive a session that never went silent.
        push_frame(&mut app, phase_frame_with_offset(0.9));
        let mut fresh = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut fresh, phase_frame_with_offset(0.9));
        assert_eq!(
            bold_planet_cells(&render_collage_for(&app, false, t0)),
            bold_planet_cells(&render_collage_for(&fresh, false, t0)),
            "an audible frame resumes the live treatment"
        );
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
    fn brackets_stay_gapped_inside_the_tile_and_out_of_hit_cells() {
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
            assert!(
                !geometry.brackets.is_empty(),
                "{status:?} keeps its selection brackets"
            );
            for bracket in &geometry.brackets {
                let cell = bracket.cell;
                assert!(
                    rect_contains(tile.rect, cell.0, cell.1),
                    "{status:?} bracket cell {cell:?} stays inside the tile"
                );
                assert!(
                    gapped_from_body(&geometry, cell),
                    "{status:?} bracket cell {cell:?} keeps the body gap"
                );
                assert!(
                    !hit.contains(&cell),
                    "{status:?} bracket cell {cell:?} never joins hit_cells"
                );
            }
        }
    }

    #[test]
    fn blocked_renders_a_weak_interior_error_pulse_without_crosses() {
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
        assert!(
            !field_text.contains('▒'),
            "no atmosphere glyph may render for a blocked planet"
        );
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
        let tile = &layout.tiles[0];
        let blocked = planet_geometry(tile, stage_field(), AgentStatus::Blocked, app.viz(), false);
        let theme = Theme::for_name(ThemeName::Minimal);
        let pulse = blocked.error_cell.expect("blocked keeps its error cell");
        let cell = buf.cell(pulse).unwrap();
        assert!(
            PLANET_GLYPHS.contains(&cell.symbol()),
            "the pulse reuses the existing body glyph"
        );
        assert_eq!(
            cell.style().fg,
            Some(theme.error),
            "the pulsing cell takes the theme error color"
        );
        assert_eq!(
            cell.style().add_modifier.contains(Modifier::DIM),
            !blocked.error_lift,
            "the weak pulse dims off its lift"
        );
        assert!(
            !cell.style().add_modifier.contains(Modifier::BOLD),
            "the weak pulse never bolds"
        );

        // Every other body cell keeps the identity surface pair.
        let palette = planet_palette(tile.seed);
        let base = theme.spectrum_color(palette.base_position);
        let accent = theme.spectrum_color(palette.accent_position);
        for &(x, y) in blocked.body.iter().filter(|&&body_cell| body_cell != pulse) {
            let fg = buf.cell((x, y)).unwrap().style().fg;
            assert!(
                fg == Some(base) || fg == Some(accent),
                "body cell ({x}, {y}) keeps its identity color"
            );
        }
    }

    #[test]
    fn stage_renders_no_ring_arc_satellite_or_atmosphere_glyphs() {
        let mut app = status_app(phase_frame());
        app.apply(Action::SelectNextAgent);
        let buf = render_collage_for(&app, false, Instant::now());
        let field: String = field_cells(&buf)
            .into_iter()
            .map(|(_, _, symbol)| symbol)
            .collect();
        for glyph in ["∘", "●", "▪", "▒"] {
            assert!(
                !field.contains(glyph),
                "old ring/arc/satellite/atmosphere glyph {glyph} may not render"
            );
        }
    }

    #[test]
    fn no_decorative_orbit_particle_cells_render_inside_the_tile() {
        // Silence keeps the scope traceless, so the only legitimate `·`
        // cells are the breathing vignette ring's own — any other dot inside
        // the tile would be an orbit particle or guide.
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Idle)]);
        push_frame(&mut app, silent_phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let field = stage_field();
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], field);
        let tile = &layout.tiles[0];
        let on_vignette_ring = |x: u16, y: u16| {
            let half_w = field.width as f32 / 2.0;
            let half_h = field.height as f32 / 2.0;
            let nx = (x as f32 - (field.x as f32 + half_w - 0.5)) / half_w;
            let ny = (y as f32 - (field.y as f32 + half_h - 0.5)) / half_h;
            let dist = (nx * nx + ny * ny).sqrt();
            (dist - VIGNETTE_BASE).abs() <= VIGNETTE_BAND
        };
        for y in tile.rect.y..tile.rect.y + tile.rect.height {
            for x in tile.rect.x..tile.rect.x + tile.rect.width {
                assert!(
                    buf.cell((x, y)).unwrap().symbol() != "·" || on_vignette_ring(x, y),
                    "no orbit-particle dot may render inside the tile at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn working_band_bolds_and_advances_in_the_buffer() {
        let drawn_band = |offset: f32| -> Vec<(u16, u16)> {
            let app = connected_app_with_phase(offset);
            let buf = render_collage_for(&app, false, Instant::now());
            let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
            let geometry = planet_geometry(
                &layout.tiles[0],
                stage_field(),
                AgentStatus::Working,
                app.viz(),
                false,
            );
            let bold: HashSet<(u16, u16)> = geometry
                .body
                .iter()
                .copied()
                .filter(|&cell| {
                    buf.cell(cell)
                        .unwrap()
                        .style()
                        .add_modifier
                        .contains(Modifier::BOLD)
                })
                .collect();
            let band: HashSet<(u16, u16)> = geometry.status_band.iter().copied().collect();
            assert_eq!(bold, band, "the buffer bolds exactly the interior band");
            assert_eq!(
                bold.len(),
                WORKING_BAND,
                "the drawn working band keeps its length"
            );
            let mut cells: Vec<(u16, u16)> = bold.into_iter().collect();
            cells.sort_unstable();
            cells
        };
        assert_ne!(
            drawn_band(0.2),
            drawn_band(0.7),
            "new played audio advances the drawn band"
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
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
                hit_test(
                    CANVAS,
                    bracket.cell.0,
                    bracket.cell.1,
                    false,
                    Instant::now(),
                    &app
                )
                .is_none(),
                "a bracket cell never selects"
            );
        }
    }

    #[test]
    fn low_power_render_freezes_the_bright_band_on_the_captured_frame() {
        let app = low_power_app_captured_from(0.3, 0.9);
        let buf = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = layout_at_rest(app.active_agents(), captured, &[], stage_field());
        let frozen = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Working,
            captured,
            false,
        );
        assert_eq!(
            frozen.status_band.len(),
            WORKING_BAND,
            "sanity: the frozen frame keeps a band in geometry"
        );
        let live = planet_geometry(
            &layout.tiles[0],
            stage_field(),
            AgentStatus::Working,
            app.viz(),
            false,
        );
        assert_ne!(
            frozen.status_band, live.status_band,
            "sanity: later audio would place the band elsewhere"
        );
        // The captured band renders bright and in place — low power freezes
        // the last played treatment instead of suppressing its brightening.
        let band: HashSet<(u16, u16)> = frozen.status_band.iter().copied().collect();
        for &cell in &frozen.body {
            assert_eq!(
                buf.cell(cell)
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::BOLD),
                band.contains(&cell),
                "low power bolds exactly the captured frame's band at {cell:?}"
            );
        }
    }

    #[test]
    fn dense_planet_field_renders_one_selectable_body_per_agent() {
        let snaps: Vec<AgentSnapshot> = (0..80)
            .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
            .collect();
        let app = collage_app(snaps);
        let buf = render_collage_for(&app, false, Instant::now());
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
        assert_eq!(layout.tiles.len(), 80, "no dense agent loses its orbit");
        for tile in &layout.tiles {
            let geometry =
                planet_geometry(tile, stage_field(), AgentStatus::Working, app.viz(), false);
            assert!(!geometry.body.is_empty(), "every dense planet keeps a body");
            let disc = disc_geometry(tile.mask, tile.rect, stage_field());
            let center = (
                (disc.origin.0 + disc.mask.width() as i32 / 2) as u16,
                (disc.origin.1 + disc.mask.height() as i32 / 2) as u16,
            );
            assert!(
                PLANET_GLYPHS.contains(&buf.cell(center).unwrap().symbol()),
                "dense planet center {center:?} must draw a planet glyph"
            );
        }
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
        let layout = layout_at_rest(&agents(3), &reference, &[], stage_field());
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
            mask: DiscMask::for_bound(rect.width, rect.height),
            rect,
            energy: 0.4,
        }
    }

    /// How many of `count` dense laid-out agents keep a non-empty planet body.
    fn dense_planet_body_count(count: usize, area: Rect) -> usize {
        let layout = layout_at_rest(&agents(count), &frame(0.5, vec![0.5; 16]), &[], area);
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
        let later = rendered_surface("gas", AgentStatus::Working, phase_frame_with_offset(0.8));
        assert_eq!(surface_geometry(&first), surface_geometry(&later));

        // A status change keeps every surface cell and glyph in place;
        // only the interior status cells restyle over the identity paint.
        let blocked = rendered_surface("gas", AgentStatus::Blocked, phase_frame_with_offset(0.1));
        let positions = |cells: Vec<(u16, u16, String, Option<Color>)>| -> Vec<(u16, u16, String)> {
            cells
                .into_iter()
                .map(|(x, y, symbol, _)| (x, y, symbol))
                .collect()
        };
        assert_eq!(
            positions(surface_geometry(&first)),
            positions(surface_geometry(&blocked))
        );
    }

    #[test]
    fn pocket_body_paints_base_and_accent_theme_spectrum_colors() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        let canvas = stage_field();
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], canvas);
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], canvas);
        let tile = &layout.tiles[0];
        assert!(
            tile.rect.width > 7,
            "sanity: the sparse layout offers an oversized bound"
        );
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz(), false);
        let &(x, y) = geometry.body.first().expect("a disc body");
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_some(),
            "a disc body cell selects"
        );

        let hit: HashSet<(u16, u16)> = geometry.hit_cells.iter().copied().collect();
        let (x, y) = (tile.rect.y..tile.rect.y + tile.rect.height)
            .flat_map(|y| (tile.rect.x..tile.rect.x + tile.rect.width).map(move |x| (x, y)))
            .find(|cell| !hit.contains(cell))
            .expect("the oversized rect keeps cells off the disc");
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_none(),
            "an oversized-rect-only cell resolves nothing"
        );
    }

    // --- solar orbits and audio boundaries ---------------------------------

    /// The planet-glyph cells (bodies and craters) of a rendered field.
    fn planet_cells_of(buf: &Buffer) -> Vec<(u16, u16, String)> {
        field_cells(buf)
            .into_iter()
            .filter(|(_, _, symbol)| PLANET_GLYPHS.contains(&symbol.as_str()))
            .collect()
    }

    #[test]
    fn audio_reshapes_the_scope_but_never_moves_planet_bodies() {
        let t0 = Instant::now();
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.05, vec![0.05; 16]));
        let quiet = render_collage_for(&app, false, t0);
        push_frame(&mut app, phase_frame());
        let loud = render_collage_for(&app, false, t0);
        assert_ne!(quiet, loud, "audio frames must drive the scope");
        assert_eq!(
            planet_cells_of(&quiet),
            planet_cells_of(&loud),
            "no RMS/FFT scale or offset may move a planet body"
        );
        for buf in [&quiet, &loud] {
            assert_eq!(
                buffer_text(buf).matches('∙').count(),
                0,
                "no shadow-trail cell may render at any energy"
            );
        }
    }

    #[test]
    fn working_planets_orbit_with_elapsed_time_while_the_scope_holds_still() {
        let t0 = Instant::now();
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let first = render_collage_for(&app, false, t0);
        let later = render_collage_for(&app, false, t0 + Duration::from_secs(40));
        assert_ne!(
            planet_cells_of(&first),
            planet_cells_of(&later),
            "elapsed monotonic time advances a Working planet's orbit"
        );

        // The scope traces themselves never follow the clock: every primary
        // trace cell not covered by a planet stays put.
        let planets: HashSet<(u16, u16)> = planet_cells_of(&first)
            .iter()
            .chain(planet_cells_of(&later).iter())
            .map(|&(x, y, _)| (x, y))
            .collect();
        let trace_cells = |buf: &Buffer| -> Vec<(u16, u16)> {
            field_cells(buf)
                .into_iter()
                .filter(|(x, y, symbol)| {
                    symbol == PRIMARY_TRACE_GLYPH && !planets.contains(&(*x, *y))
                })
                .map(|(x, y, _)| (x, y))
                .collect()
        };
        assert_eq!(
            trace_cells(&first),
            trace_cells(&later),
            "elapsed time never moves the phase scope"
        );
    }

    #[test]
    fn non_working_planets_freeze_at_their_captured_angle_and_resume() {
        let t0 = Instant::now();
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());

        // Working→Idle after 40 elapsed seconds freezes the planet at its
        // then-current angle: away from its initial rest position, and
        // invariant under any later clock time.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Idle)],
            now: t0 + Duration::from_secs(40),
        });
        let frozen = render_collage_for(&app, false, t0 + Duration::from_secs(41));
        let frozen_later = render_collage_for(&app, false, t0 + Duration::from_secs(140));
        assert_eq!(
            planet_cells_of(&frozen),
            planet_cells_of(&frozen_later),
            "a non-Working planet holds its captured angle"
        );
        let mut rested = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Idle)]);
        push_frame(&mut rested, phase_frame());
        let rested = render_collage_for(&rested, false, Instant::now());
        assert_ne!(
            planet_cells_of(&frozen),
            planet_cells_of(&rested),
            "the captured angle reflects the elapsed Working stretch"
        );

        // Working again resumes the orbit from the captured angle.
        let t1 = t0 + Duration::from_secs(200);
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Working)],
            now: t1,
        });
        assert_eq!(
            planet_cells_of(&render_collage_for(&app, false, t1)),
            planet_cells_of(&frozen),
            "resuming starts exactly at the captured angle"
        );
        assert_ne!(
            planet_cells_of(&render_collage_for(
                &app,
                false,
                t1 + Duration::from_secs(40)
            )),
            planet_cells_of(&frozen),
            "a resumed Working stretch moves the planet again"
        );
    }

    #[test]
    fn sun_renders_static_centered_and_is_never_a_hit_target() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, phase_frame());
        let field = stage_field();
        let sun = (field.x + field.width / 2, field.y + field.height / 2);
        let t0 = Instant::now();
        let first = render_collage_for(&app, false, t0);
        assert_eq!(
            cell_text(&first, sun.0, sun.1),
            SUN_GLYPH,
            "the sun draws at the field center"
        );
        let later = render_collage_for(&app, false, t0 + Duration::from_secs(40));
        assert_eq!(
            cell_text(&later, sun.0, sun.1),
            SUN_GLYPH,
            "the sun never moves with time"
        );
        push_frame(&mut app, phase_frame_with_offset(0.9));
        let louder = render_collage_for(&app, false, t0);
        assert_eq!(
            cell_text(&louder, sun.0, sun.1),
            SUN_GLYPH,
            "the sun never moves with audio"
        );
        assert!(
            hit_test(CANVAS, sun.0, sun.1, false, t0, &app).is_none(),
            "the sun is decoration, never a hit target"
        );
    }

    #[test]
    fn orbit_guide_lines_never_render() {
        // At silence the field may hold only the vignette ring, the sun, and
        // planet bodies — no glyph tracing an orbit path exists at all.
        let mut app = collage_app(vec![
            snap("ws", "p1", None, AgentStatus::Working),
            snap("ws", "p2", None, AgentStatus::Idle),
        ]);
        push_frame(&mut app, silent_phase_frame());
        let buf = render_collage_for(&app, false, Instant::now());
        for (x, y, symbol) in field_cells(&buf) {
            assert!(
                symbol == "·" || symbol == SUN_GLYPH || PLANET_GLYPHS.contains(&symbol.as_str()),
                "unexpected field glyph {symbol:?} at ({x}, {y}) — orbit guides may not render"
            );
        }
    }

    #[test]
    fn silence_is_dim_and_still_across_time() {
        let t0 = Instant::now();
        // Non-Working planets hold still, so a silent field is fully
        // time-invariant; Working orbit motion is a clock concern, not an
        // audio one, and is covered separately.
        let mut app = collage_app(vec![
            snap("ws", "p1", None, AgentStatus::Idle),
            snap("ws", "p2", None, AgentStatus::Done),
        ]);
        push_frame(&mut app, frame(0.0, vec![0.0; 16]));
        let first = render_collage_for(&app, false, t0);
        let later = render_collage_for(&app, false, t0 + Duration::from_secs(40));
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
            "low power freezes trace, disc, and bracket geometry"
        );

        // The same later frame in normal power does move the scope.
        let live = render_collage_for(&app, false, Instant::now());
        assert_ne!(field_cells(&later), field_cells(&live));

        // A fresh snapshot still recolors the frozen planet's interior
        // status cells in place.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Blocked)],
            now: Instant::now(),
        });
        let theme = Theme::for_name(ThemeName::Minimal);
        let recolored = render_collage_for(&app, true, Instant::now());
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = layout_at_rest(app.active_agents(), captured, &[], stage_field());
        let tile = &layout.tiles[0];
        let geometry = planet_geometry(tile, stage_field(), AgentStatus::Blocked, captured, false);
        let (x, y) = geometry
            .error_cell
            .expect("the frozen planet keeps its error cell");
        assert_eq!(
            recolored.cell((x, y)).unwrap().style().fg,
            Some(theme.error),
            "the frozen body's error cell takes the fresh blocked color"
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

    #[test]
    fn low_power_freezes_working_orbits_at_the_captured_layout() {
        let t0 = Instant::now();
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.configure_low_power_visuals(true);
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Working)],
            now: t0,
        });
        app.apply(Action::ToggleAgentOverlay);
        push_frame(&mut app, phase_frame());

        // The audible capture freezes the whole solar layout: later clock
        // time never advances a low-power orbit, an active Working stretch
        // included.
        let at_capture = render_collage_for(&app, true, t0);
        for later in [40u64, 400] {
            assert_eq!(
                planet_cells_of(&render_collage_for(
                    &app,
                    true,
                    t0 + Duration::from_secs(later)
                )),
                planet_cells_of(&at_capture),
                "the low-power orbit holds the captured angle at +{later}s"
            );
        }

        // Status transitions after the capture bank real orbit time, but
        // the frozen layout never reflects it.
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Idle)],
            now: t0 + Duration::from_secs(40),
        });
        assert_eq!(
            planet_cells_of(&render_collage_for(
                &app,
                true,
                t0 + Duration::from_secs(140)
            )),
            planet_cells_of(&at_capture),
            "banked transitions never move the frozen planet"
        );
    }

    #[test]
    fn low_power_hit_testing_resolves_the_frozen_orbit_layout() {
        let t0 = Instant::now();
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.configure_low_power_visuals(true);
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Working)],
            now: t0,
        });
        app.apply(Action::ToggleAgentOverlay);
        push_frame(&mut app, phase_frame());

        // Every cell resolves identically at any later instant: hit testing
        // reads the same frozen orbit layout the renderer draws.
        let hits_at = |now: Instant| -> Vec<(u16, u16)> {
            let field = stage_field();
            let mut cells = Vec::new();
            for y in field.y..field.y + field.height {
                for x in field.x..field.x + field.width {
                    if hit_test(CANVAS, x, y, true, now, &app).is_some() {
                        cells.push((x, y));
                    }
                }
            }
            cells
        };
        let at_capture = hits_at(t0);
        assert!(
            !at_capture.is_empty(),
            "sanity: the planet body is clickable"
        );
        assert_eq!(
            hits_at(t0 + Duration::from_secs(40)),
            at_capture,
            "a later clock never moves the low-power hit targets"
        );
    }

    #[test]
    fn low_power_rests_agents_first_observed_after_the_capture_at_phase_zero() {
        let t0 = Instant::now();
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.configure_low_power_visuals(true);
        app.apply(Action::AgentSnapshot {
            agents: vec![snap("ws", "p1", Some("one"), AgentStatus::Working)],
            now: t0,
        });
        app.apply(Action::ToggleAgentOverlay);
        push_frame(&mut app, phase_frame());

        // A Working agent first observed after the audible capture is
        // missing from the frozen orbit map: it rests at zero seconds — its
        // seed-derived initial angle — never the live effective Working time.
        app.apply(Action::AgentSnapshot {
            agents: vec![
                snap("ws", "p1", Some("one"), AgentStatus::Working),
                snap("ws", "p2", Some("two"), AgentStatus::Working),
            ],
            now: t0 + Duration::from_secs(5),
        });
        assert_eq!(
            app.low_power_orbit_secs(&AgentId::new("ws", "p2")),
            Some(0.0),
            "an agent unknown to the capture reads zero orbit seconds"
        );

        let at_first_sight = render_collage_for(&app, true, t0 + Duration::from_secs(5));
        assert_ne!(
            planet_cells_of(&render_collage_for(
                &app,
                false,
                t0 + Duration::from_secs(45)
            )),
            planet_cells_of(&at_first_sight),
            "sanity: the same span moves live Working planets"
        );
        for later in [45u64, 405] {
            assert_eq!(
                planet_cells_of(&render_collage_for(
                    &app,
                    true,
                    t0 + Duration::from_secs(later)
                )),
                planet_cells_of(&at_first_sight),
                "the late-observed planet holds its initial angle at +{later}s"
            );
        }

        // Hit testing reads the same frozen layout the renderer draws.
        let hits_at = |now: Instant| -> Vec<(u16, u16)> {
            let field = stage_field();
            let mut cells = Vec::new();
            for y in field.y..field.y + field.height {
                for x in field.x..field.x + field.width {
                    if hit_test(CANVAS, x, y, true, now, &app).is_some() {
                        cells.push((x, y));
                    }
                }
            }
            cells
        };
        let hits = hits_at(t0 + Duration::from_secs(5));
        assert!(!hits.is_empty(), "sanity: planet bodies are clickable");
        assert_eq!(
            hits_at(t0 + Duration::from_secs(405)),
            hits,
            "a later clock never moves the late-observed hit targets"
        );
    }

    // --- state, selection, and privacy -------------------------------------

    #[test]
    fn body_renders_all_five_status_treatments_from_the_theme() {
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
            let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
            let tile = &layout.tiles[0];
            let geometry = planet_geometry(tile, stage_field(), status, app.viz(), false);
            let palette = planet_palette(tile.seed);
            let base = theme.spectrum_color(palette.base_position);
            let accent = theme.spectrum_color(palette.accent_position);
            let band: HashSet<(u16, u16)> = geometry.status_band.iter().copied().collect();
            match status {
                AgentStatus::Working => {
                    assert_eq!(band.len(), WORKING_BAND, "working keeps its band length")
                }
                _ => assert!(band.is_empty(), "{status:?} keeps no band"),
            }
            for &cell in &geometry.body {
                let style = buf.cell(cell).unwrap().style();
                let bold = style.add_modifier.contains(Modifier::BOLD);
                let dim = style.add_modifier.contains(Modifier::DIM);
                match status {
                    AgentStatus::Working => {
                        assert_eq!(bold, band.contains(&cell), "working bolds exactly its band");
                        assert!(!dim, "the working body never fades while energy is up");
                        assert!(
                            style.fg == Some(base) || style.fg == Some(accent),
                            "the band brightens identity colors, never a status color"
                        );
                    }
                    AgentStatus::Blocked => {
                        assert!(!bold, "the blocked body never bolds");
                        if Some(cell) == geometry.error_cell {
                            assert_eq!(
                                style.fg,
                                Some(theme.error),
                                "the pulsing cell takes the theme error color"
                            );
                            assert_eq!(
                                dim, !geometry.error_lift,
                                "the weak pulse dims off its lift"
                            );
                        } else {
                            assert!(
                                style.fg == Some(base) || style.fg == Some(accent),
                                "off-pulse cells keep identity colors"
                            );
                            assert!(!dim, "off-pulse cells stay plain while energy is up");
                        }
                    }
                    AgentStatus::Idle => {
                        assert!(!bold, "idle never bolds");
                        assert!(dim, "idle keeps the whole body muted");
                        assert!(
                            style.fg == Some(base) || style.fg == Some(accent),
                            "idle keeps identity colors"
                        );
                    }
                    AgentStatus::Done | AgentStatus::Unknown => {
                        assert!(!bold, "{status:?} never bolds");
                        assert!(dim, "{status:?} keeps the whole body dim");
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

    #[test]
    fn explicit_name_renders_beneath_its_planet_while_unnamed_stays_label_free() {
        let named = collage_app(vec![snap(
            "private-workspace",
            "private-pane",
            Some("Named"),
            AgentStatus::Working,
        )]);
        let named_text = buffer_text(&render_collage_for(&named, false, Instant::now()));
        assert!(
            named_text.contains("Named"),
            "explicit Herdr name is the sole stage label"
        );
        for private_value in ["private-workspace", "private-pane"] {
            assert!(
                !named_text.contains(private_value),
                "agent-private fallback must never render: {private_value}"
            );
        }

        let unnamed = collage_app(vec![snap(
            "other-workspace",
            "other-pane",
            None,
            AgentStatus::Idle,
        )]);
        let unnamed_text = buffer_text(&render_collage_for(&unnamed, false, Instant::now()));
        assert!(!unnamed_text.contains("other-workspace"));
        assert!(!unnamed_text.contains("other-pane"));
    }

    #[test]
    fn long_explicit_name_is_ellipsized_to_its_planet_tile() {
        let long_name = "A deliberately overlong explicit Herdr planet label";
        let app = collage_app(vec![snap(
            "ws",
            "pane",
            Some(long_name),
            AgentStatus::Working,
        )]);
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !text.contains(long_name),
            "long labels never overflow their tiles"
        );
        assert!(
            text.contains('…'),
            "long label uses a visible ellipsis: {text}"
        );
    }

    #[test]
    fn colliding_explicit_name_labels_are_suppressed_without_dropping_planets() {
        let agents = (0..80)
            .map(|index| {
                snap(
                    "ws",
                    &format!("pane-{index}"),
                    Some("A"),
                    AgentStatus::Working,
                )
            })
            .collect();
        let app = collage_app(agents);
        let area = Rect::new(0, 0, 40, 16);
        let buffer = render_collage_in(&app, false, Instant::now(), area);
        let field = agent_stage_layout(area).field;
        let mut rendered_labels = 0;
        for y in field.y..field.y + field.height {
            for x in field.x..field.x + field.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    rendered_labels += usize::from(cell.symbol() == "A");
                }
            }
        }
        assert!(
            rendered_labels > 0,
            "at least one non-colliding label remains visible"
        );
        assert!(
            rendered_labels < 80,
            "colliding label candidates are suppressed"
        );
        let unnamed_agents = (0..80)
            .map(|index| snap("ws", &format!("pane-{index}"), None, AgentStatus::Working))
            .collect();
        let without_labels =
            render_collage_in(&collage_app(unnamed_agents), false, Instant::now(), area);
        let planet_cells = |buffer: &Buffer| {
            let mut cells = Vec::new();
            for y in field.y..field.y + field.height {
                for x in field.x..field.x + field.width {
                    if let Some(cell) = buffer.cell((x, y)) {
                        let symbol = cell.symbol();
                        if symbol == PLANET_BODY_GLYPH || symbol == CRATER_GLYPH {
                            cells.push((x, y, symbol.to_string()));
                        }
                    }
                }
            }
            cells
        };
        assert_eq!(
            planet_cells(&buffer),
            planet_cells(&without_labels),
            "labels never replace planet bodies"
        );
    }

    #[test]
    fn selected_label_uses_accent_and_stale_label_dims_without_changing_that_color() {
        let mut app = collage_app(vec![snap(
            "ws",
            "pane",
            Some("Selected"),
            AgentStatus::Working,
        )]);
        app.apply(Action::SelectNextAgent);
        let theme = Theme::for_name(ThemeName::Minimal);
        let live = render_collage_for(&app, false, Instant::now());
        let live_style = live
            .content()
            .iter()
            .find(|cell| cell.symbol() == "S")
            .map(|cell| cell.style());
        assert_eq!(
            live_style.map(|style| style.fg),
            Some(Some(theme.accent)),
            "selected label renders in the accent color"
        );
        assert!(
            live_style.is_some_and(|style| !style.add_modifier.contains(Modifier::DIM)),
            "selected live label is not dimmed"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale = render_collage_for(&app, false, Instant::now());
        let stale_style = stale
            .content()
            .iter()
            .find(|cell| cell.symbol() == "S")
            .map(|cell| cell.style());
        assert_eq!(
            stale_style.map(|style| style.fg),
            Some(Some(theme.accent)),
            "stale label keeps its selected accent color"
        );
        assert!(
            stale_style.is_some_and(|style| style.add_modifier.contains(Modifier::DIM)),
            "stale label stays visible but dims"
        );
    }

    #[test]
    fn unavailable_hides_explicit_planet_labels() {
        let mut app = collage_app(vec![snap(
            "ws",
            "pane",
            Some("Hidden"),
            AgentStatus::Working,
        )]);
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(1),
        });
        let text = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !text.contains("Hidden"),
            "unavailable hides every planet label"
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
        assert!(!text.contains(SUN_GLYPH), "unavailable hides the sun too");
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
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

        let action =
            hit_test(CANVAS, x, y, false, Instant::now(), &app).expect("a tile click selects");
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &history, canvas);
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
                        hit_test(CANVAS, cell.x, cell.y, false, Instant::now(), &app).is_none(),
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], canvas);
        let planet_cells: HashSet<(u16, u16)> = layout
            .tiles
            .iter()
            .flat_map(|tile| tile_hit_cells(tile, canvas))
            .collect();

        let tile = layout
            .tiles
            .iter()
            .find(|tile| app.active_agents()[tile.index].status == AgentStatus::Working)
            .expect("the working agent keeps a tile");
        let geometry = planet_geometry(tile, canvas, AgentStatus::Working, app.viz(), false);
        let &(x, y) = geometry.body.first().expect("a planet keeps body cells");
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_some(),
            "a body cell selects"
        );
        let &(x, y) = geometry
            .status_band
            .first()
            .expect("the working planet keeps its interior band");
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_some(),
            "an interior status cell is a body cell and selects"
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
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_none(),
            "a scope-only cell never selects"
        );

        let (x, y) = (canvas.y..canvas.y + canvas.height)
            .flat_map(|y| (canvas.x..canvas.x + canvas.width).map(move |x| (x, y)))
            .find(|cell| !planet_cells.contains(cell))
            .expect("the canvas keeps empty cells");
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_none(),
            "an empty cell never selects"
        );
    }

    #[test]
    fn clicks_resolve_nothing_when_missed_stale_or_closed() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        assert!(
            hit_test(CANVAS, 0, 0, false, Instant::now(), &app).is_none(),
            "corner miss"
        );

        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], stage_field());
        let tile = &layout.tiles[0];
        let (x, y) = (
            tile.rect.x + tile.rect.width / 2,
            tile.rect.y + tile.rect.height / 2,
        );
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &app).is_none(),
            "stale ignores clicks"
        );

        let mut closed = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        closed.apply(Action::CloseAgentOverlay);
        assert!(
            hit_test(CANVAS, x, y, false, Instant::now(), &closed).is_none(),
            "closed canvas ignores clicks"
        );
    }

    #[test]
    fn low_power_hit_testing_holds_the_frozen_orbit_angle() {
        let mut app = collage_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.configure_low_power_visuals(true);
        push_frame(&mut app, phase_frame());
        let canvas = stage_field();

        // Elapsed Working time moves the live orbit position; the low-power
        // capture holds the whole layout at the angle it froze with the
        // frame, so hit testing never advances with the clock.
        let later = Instant::now() + Duration::from_secs(40);
        let moved_layout = collage_layout(app.active_agents(), &[40.0], app.viz(), &[], canvas);
        let moved = &moved_layout.tiles[0];
        let rest_layout = layout_at_rest(app.active_agents(), app.viz(), &[], canvas);
        let rest = &rest_layout.tiles[0];
        assert_ne!(
            moved.rect, rest.rect,
            "sanity: elapsed Working time moves the live tile"
        );

        // A cell only the frozen (captured) position covers still selects in
        // low power at `later`, while live hit testing has moved off it.
        let rest_cells = tile_hit_cells(rest, canvas);
        let moved_set: HashSet<(u16, u16)> = tile_hit_cells(moved, canvas).into_iter().collect();
        let &(x, y) = rest_cells
            .iter()
            .find(|cell| !moved_set.contains(cell))
            .expect("the frozen position exposes cells off the moved planet");
        assert!(
            hit_test(CANVAS, x, y, true, later, &app).is_some(),
            "a low-power click resolves the frozen orbit position"
        );
        assert!(
            hit_test(CANVAS, x, y, false, later, &app).is_none(),
            "live hit testing has moved off the frozen cell"
        );

        // A cell only the live moved position covers holds no frozen planet:
        // low power never advances with the clock.
        let rest_set: HashSet<(u16, u16)> = rest_cells.into_iter().collect();
        let mut checked = false;
        for &(x, y) in &moved_set {
            if !rest_set.contains(&(x, y)) {
                checked = true;
                assert!(
                    hit_test(CANVAS, x, y, true, later, &app).is_none(),
                    "a moved-position-only cell ({x}, {y}) resolves nothing in low power"
                );
            }
        }
        assert!(
            checked,
            "sanity: the moved position exposes cells off the frozen planet"
        );
    }

    #[test]
    fn low_power_selects_frozen_planet_cells_but_scope_cells_do_nothing() {
        let app = low_power_app_captured_from(0.3, 0.9);
        let canvas = stage_field();
        let (captured, _) = app.low_power_viz().expect("policy captured a frame");
        let layout = layout_at_rest(app.active_agents(), captured, &[], canvas);
        let cells = tile_hit_cells(&layout.tiles[0], canvas);
        let &(x, y) = cells.first().expect("the frozen planet keeps cells");
        assert!(
            hit_test(CANVAS, x, y, true, Instant::now(), &app).is_some(),
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
            hit_test(CANVAS, x, y, true, Instant::now(), &app).is_none(),
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
        let layout = layout_at_rest(app.active_agents(), app.viz(), &[], field);
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
        let sparse = layout_at_rest(&agents(3), &frame(0.0, vec![0.0; 16]), &[], sparse_area);
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
        let dense = layout_at_rest(&agents(80), &frame(0.5, vec![0.5; 16]), &[], dense_area);
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
                    geometry.status_band.is_empty() && geometry.error_cell.is_none(),
                    "a one-cell disc omits status detail instead of crowding"
                );
            }
        }
    }

    #[test]
    fn one_cell_discs_keep_their_body_and_omit_status_detail() {
        let area = Rect::new(0, 0, 50, 15);
        let rect = Rect::new(10, 5, 2, 1);
        let tile = CollageTile {
            index: 0,
            seed: 5,
            mask: DiscMask::for_bound(rect.width, rect.height),
            rect,
            energy: 0.5,
        };
        for status in [AgentStatus::Working, AgentStatus::Blocked] {
            let geometry = planet_geometry(&tile, area, status, &phase_frame(), false);
            assert_eq!(geometry.mask, DiscMask::Dot);
            assert!(!geometry.body.is_empty(), "the dot disc keeps its body");
            assert!(
                geometry.status_band.is_empty(),
                "{status:?} omits the band on a one-cell disc"
            );
            assert!(
                geometry.error_cell.is_none(),
                "{status:?} omits the pulse on a one-cell disc"
            );
        }
    }

    #[test]
    fn stage_hides_agent_data_until_the_selected_planet_opens_the_table() {
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
        let stage_buffer = render_collage_for(&app, false, Instant::now());
        let stage = buffer_text(&stage_buffer);
        assert!(
            stage.contains("research"),
            "explicit Herdr names label their planets"
        );
        assert!(!stage.contains("working"));
        assert!(!stage.contains("pi"));
        assert!(stage_footer_text(&stage_buffer).contains("O open pane"));

        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let modal_buffer = render_collage_for(&app, false, Instant::now());
        let modal = buffer_text(&modal_buffer);
        assert!(modal.contains("Agent table"));
        for heading in ["Name", "Agent", "Status", "Activity"] {
            assert!(modal.contains(heading), "missing {heading} header: {modal}");
        }
        assert!(
            !modal.contains('|'),
            "table has no hand-built cell dividers"
        );
        assert!(modal.contains("O open pane"), "table owns the focus hint");
        assert!(
            modal.contains("r rename"),
            "table exposes its inline Name rename entry point"
        );
        assert!(
            !stage_footer_text(&modal_buffer).contains("O open pane"),
            "stage footer must not repeat the table-local focus hint"
        );
        assert!(
            !modal.contains('▶'),
            "table selection uses only row styling"
        );
        assert!(modal.contains("research"));
        assert!(modal.contains("pi"));
        assert!(modal.contains("working"));
        assert!(modal.contains("Review the modal"));
        assert!(!modal.contains("workspace-private"));
        assert!(!modal.contains("pane-private"));
    }

    #[test]
    fn agent_table_lists_all_agents_and_styles_the_shared_selection() {
        let mut app = collage_app(vec![
            AgentSnapshot {
                id: AgentId::new("ws", "p1"),
                details: AgentDetails {
                    name: Some("alpha".to_string()),
                    agent: Some("pi".to_string()),
                    activity: Some("First pass".to_string()),
                },
                status: AgentStatus::Working,
            },
            AgentSnapshot {
                id: AgentId::new("ws", "p2"),
                details: AgentDetails {
                    name: Some("beta".to_string()),
                    agent: Some("claude".to_string()),
                    activity: Some("Second pass".to_string()),
                },
                status: AgentStatus::Idle,
            },
        ]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);

        let theme = Theme::for_name(ThemeName::Minimal);
        let first_buffer = render_collage_for(&app, false, Instant::now());
        let first = buffer_text(&first_buffer);
        assert!(first.contains("Name"));
        assert!(first.contains("Agent"));
        assert!(first.contains("Status"));
        assert!(first.contains("Activity"));
        assert!(first.contains("alpha"));
        assert!(first.contains("beta"));
        assert!(first.contains("First pass"));
        assert!(first.contains("Second pass"));
        assert!(
            !first.contains('▶'),
            "table uses no selection marker: {first}"
        );
        assert!(first.contains("O open pane"));
        let alpha_y = (0..CANVAS.height)
            .find(|&y| {
                (0..CANVAS.width)
                    .map(|x| first_buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
                    .contains("alpha")
            })
            .expect("selected alpha row");
        let alpha_x = (0..CANVAS.width)
            .find(|&x| first_buffer.cell((x, alpha_y)).unwrap().symbol() == "a")
            .expect("alpha cell");
        let alpha_cell = first_buffer.cell((alpha_x, alpha_y)).unwrap();
        assert_eq!(alpha_cell.fg, theme.selection_fg);
        assert_eq!(alpha_cell.bg, theme.selection_bg);

        app.apply(Action::SelectNextAgent);
        let second = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(
            !second.contains('▶'),
            "table uses no selection marker: {second}"
        );
    }

    #[test]
    fn agent_table_modal_has_outer_borders_and_a_centered_full_width_footer() {
        let mut app = collage_app(vec![AgentSnapshot {
            id: AgentId::new("ws", "p1"),
            details: AgentDetails {
                name: Some("alpha".to_string()),
                agent: Some("pi".to_string()),
                activity: Some("Review".to_string()),
            },
            status: AgentStatus::Working,
        }]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);

        let buffer = render_collage_for(&app, false, Instant::now());
        let area = agent_table_modal_area(stage_field(), app.active_agents().len());
        let footer = "O open pane · r rename · Enter/Esc close";
        let footer_x = area.x + 1 + (area.width - 1 - footer.chars().count() as u16) / 2;
        let footer_y = area.y + area.height - 2;

        for (x, y, border) in [
            (area.x, area.y, "┌"),
            (area.x + area.width - 1, area.y, "┐"),
            (area.x, area.y + area.height - 1, "└"),
            (area.x + area.width - 1, area.y + area.height - 1, "┘"),
        ] {
            assert_eq!(buffer.cell((x, y)).unwrap().symbol(), border);
        }
        assert_eq!(buffer.cell((area.x, area.y + 1)).unwrap().symbol(), "│");
        assert_eq!(
            buffer
                .cell((area.x + area.width - 1, area.y + 1))
                .unwrap()
                .symbol(),
            "│"
        );
        let footer_line: String = (area.x..area.x + area.width)
            .map(|x| buffer.cell((x, footer_y)).unwrap().symbol())
            .collect();
        assert_eq!(
            buffer.cell((footer_x, footer_y)).unwrap().symbol(),
            "O",
            "modal controls occupy a dedicated full-width footer below the table columns: {footer_line:?}"
        );

        let header_y = (area.y + 1..footer_y)
            .find(|&y| {
                (area.x + 1..area.x + area.width - 1)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
                    .contains("Name")
            })
            .expect("header row");
        let first_row_y = (area.y + 1..footer_y)
            .find(|&y| {
                (area.x + 1..area.x + area.width - 1)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
                    .contains("alpha")
            })
            .expect("first data row");
        assert_eq!(
            first_row_y,
            header_y + 1,
            "the header has neither a separator nor a bottom margin before data rows"
        );
    }

    #[test]
    fn table_selection_follows_keyboard_navigation_across_planets() {
        let mut app = collage_app(vec![
            AgentSnapshot {
                id: AgentId::new("ws", "p1"),
                details: AgentDetails {
                    name: Some("alpha".to_string()),
                    agent: Some("pi".to_string()),
                    activity: Some("First pass".to_string()),
                },
                status: AgentStatus::Working,
            },
            AgentSnapshot {
                id: AgentId::new("ws", "p2"),
                details: AgentDetails {
                    name: Some("beta".to_string()),
                    agent: Some("claude".to_string()),
                    activity: Some("Second pass".to_string()),
                },
                status: AgentStatus::Idle,
            },
        ]);
        push_frame(&mut app, phase_frame());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let first = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(!first.contains('▶'));
        assert!(first.contains("alpha") && first.contains("beta"));

        app.apply(Action::SelectNextAgent);
        let second = buffer_text(&render_collage_for(&app, false, Instant::now()));
        assert!(second.contains("Agent table"));
        assert!(!second.contains('▶'));
        assert!(second.contains("alpha"));
        assert!(second.contains("claude"));
        assert!(second.contains("idle"));
        assert!(second.contains("Second pass"));
    }

    #[test]
    fn narrow_table_keeps_all_four_columns_with_ellipses() {
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
        let area = Rect::new(0, 0, 20, 12);
        let mut buffer = Buffer::empty(area);
        let theme = Theme::for_name(ThemeName::Minimal);
        render_canvas(&app, &theme, false, Instant::now(), area, &mut buffer);
        let modal = buffer_text(&buffer);
        let header = modal
            .lines()
            .find(|line| line.contains("Na…") && line.contains("Act…"))
            .expect("four-column header");
        assert!(header.contains("Na…"));
        assert!(header.contains('…'), "narrow headers ellipsize");
        assert!(!header.contains('|'), "columns are native table cells");
        assert!(modal.contains('…'), "narrow cells ellipsize");
        assert!(
            !modal.contains(activity),
            "activity never spills beyond its cell"
        );
    }

    #[test]
    fn table_caps_the_modal_width_and_scrolls_at_ten_rows() {
        assert_eq!(
            AGENT_TABLE_WIDTHS,
            [
                Constraint::Percentage(25),
                Constraint::Percentage(20),
                Constraint::Percentage(15),
                Constraint::Percentage(40),
            ]
        );
        assert_eq!(
            agent_table_modal_area(Rect::new(0, 0, 120, 20), 1).width,
            100,
            "wide fields cap the 90% modal at 100 cells"
        );
        assert_eq!(
            agent_table_modal_area(Rect::new(0, 0, 80, 20), 1).width,
            72,
            "smaller fields use 90% of their width"
        );

        let snapshots = (0..12)
            .map(|index| AgentSnapshot {
                id: AgentId::new("ws", format!("p{index:02}")),
                details: AgentDetails {
                    name: Some(format!("row-{index:02}")),
                    agent: Some("pi".to_string()),
                    activity: Some("scroll test".to_string()),
                },
                status: AgentStatus::Working,
            })
            .collect();
        let mut app = collage_app(snapshots);
        push_frame(&mut app, phase_frame());
        for _ in 0..12 {
            app.apply(Action::SelectNextAgent);
        }
        app.apply(Action::OpenAgentDetails);

        let buffer = render_collage_for(&app, false, Instant::now());
        let modal_area = agent_table_modal_area(stage_field(), app.active_agents().len());
        let buffer_ref = &buffer;
        let modal: String = (modal_area.y..modal_area.y + modal_area.height)
            .flat_map(|y| {
                (modal_area.x..modal_area.x + modal_area.width)
                    .map(move |x| buffer_ref.cell((x, y)).map_or(" ", |cell| cell.symbol()))
            })
            .collect();
        assert!(modal.contains("row-11"), "selected final row stays visible");
        assert!(
            !modal.contains("row-00"),
            "viewport scrolls past the first row"
        );
        assert_eq!(
            modal.matches("row-").count(),
            10,
            "at most ten data rows render"
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

    // --- characterization digests ------------------------------------------
    //
    // Whole-buffer and whole-area hit-map fingerprints that pin the composed
    // output of the stage: every glyph, both colors, and both modifier sets of
    // every cell, plus the selection every cell resolves. The named tests
    // above each guard one documented rule; these guard the composition as a
    // whole, so a refactor that moves code between modules cannot shift a
    // single cell, color, or hit target without failing here. They are
    // deliberately opaque — a mismatch means "read the named tests that also
    // broke", or, if only a digest moved, that a rule no test names yet
    // changed.
    //
    // Reproducibility: every fixture pins one `Instant` across the snapshot
    // action and the render/hit call, so live Working orbit phases rest at
    // exactly zero rather than at a few microseconds of elapsed Working time.

    fn fnv(hash: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(hash, |acc, byte| {
            (acc ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

    /// Fingerprint every cell's symbol, colors, and modifiers.
    fn buffer_digest(buf: &Buffer) -> u64 {
        let area = *buf.area();
        let mut hash = FNV_OFFSET;
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let cell = buf.cell((x, y)).unwrap();
                let style = cell.style();
                hash = fnv(hash, cell.symbol().as_bytes());
                hash = fnv(
                    hash,
                    format!(
                        "{:?}/{:?}/{:?}/{:?}",
                        style.fg, style.bg, style.add_modifier, style.sub_modifier
                    )
                    .as_bytes(),
                );
            }
        }
        hash
    }

    /// Sweep [`hit_test`] over every cell of `area`: the digest pins which
    /// cell resolves which agent, and the count pins how many cells are
    /// selectable at all — decoration, scope, sun, and brackets must keep
    /// resolving nothing.
    fn hit_map_digest(app: &App, area: Rect, low_power: bool, now: Instant) -> (u64, usize) {
        let mut hash = FNV_OFFSET;
        let mut hits = 0usize;
        for row in area.y..area.y + area.height {
            for column in area.x..area.x + area.width {
                match hit_test(area, column, row, low_power, now, app) {
                    Some(Action::SelectAgent(id)) => {
                        hits += 1;
                        hash = fnv(hash, format!("{column},{row}={id:?}").as_bytes());
                    }
                    Some(other) => {
                        panic!("hit testing resolved a non-selection action: {other:?}")
                    }
                    None => hash = fnv(hash, b"."),
                }
            }
        }
        (hash, hits)
    }

    /// The hit-map digest of an `area` where every cell resolves nothing:
    /// what a closed, stale, or unavailable stage must produce.
    fn all_miss_digest(area: Rect) -> (u64, usize) {
        let cells = area.width as usize * area.height as usize;
        ((0..cells).fold(FNV_OFFSET, |hash, _| fnv(hash, b".")), 0)
    }

    /// A connected, overlay-open app whose snapshot instant is `now`, so a
    /// render at the same `now` sees every Working orbit at phase zero.
    fn pinned_app(agents: Vec<AgentSnapshot>, frames: Vec<VizFrame>, now: Instant) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot { agents, now });
        app.apply(Action::ToggleAgentOverlay);
        for frame in frames {
            push_frame(&mut app, frame);
        }
        app
    }

    /// One named agent per status, with a real history frame behind the
    /// current one, and the first planet selected: exercises every surface
    /// treatment, the persistence layers, labels, and focus brackets at once.
    fn characterization_statuses(now: Instant) -> App {
        let mut app = pinned_app(
            vec![
                snap("ws", "w", Some("worker"), AgentStatus::Working),
                snap("ws", "i", Some("idler"), AgentStatus::Idle),
                snap("ws", "b", Some("blocked"), AgentStatus::Blocked),
                snap("ws", "d", Some("doner"), AgentStatus::Done),
                snap("ws", "u", Some("unknown"), AgentStatus::Unknown),
            ],
            vec![older_phase_frame(), phase_frame()],
            now,
        );
        app.apply(Action::SelectNextAgent);
        app
    }

    /// Twenty-four unnamed Working agents: the dense tier where disc masks
    /// shrink and orbit radii scale to the field.
    fn characterization_dense(now: Instant) -> App {
        pinned_app(
            (0..24)
                .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
                .collect(),
            vec![phase_frame()],
            now,
        )
    }

    #[test]
    fn characterization_wide_stage_pins_its_buffer_and_hit_map() {
        let now = Instant::now();
        let app = characterization_statuses(now);
        let buf = render_collage_in(&app, false, now, CANVAS);

        assert_eq!(
            buffer_digest(&buf),
            4_134_155_508_641_885_490,
            "the composed wide stage changed: symbols, colors, or emphasis moved"
        );
        assert_eq!(
            hit_map_digest(&app, CANVAS, false, now),
            (5_223_541_943_661_906_365, 113),
            "the wide stage hit map changed: a cell resolves a different agent"
        );
    }

    #[test]
    fn characterization_dense_field_pins_its_buffer_and_hit_map() {
        let now = Instant::now();
        let app = characterization_dense(now);
        let buf = render_collage_in(&app, false, now, CANVAS);

        assert_eq!(
            buffer_digest(&buf),
            4_995_872_266_946_714_365,
            "the dense planet field changed: mask fallthrough or orbit radii moved"
        );
        assert_eq!(
            hit_map_digest(&app, CANVAS, false, now),
            (17_496_996_632_573_064_648, 220),
            "the dense field hit map changed"
        );
    }

    #[test]
    fn characterization_pins_every_layout_tier() {
        let now = Instant::now();
        let app = characterization_statuses(now);
        // Both digests per tier, not just the buffer: narrow tiers are where
        // mask fallthrough and clamping engage, so they are exactly where the
        // drawn body and the hit cells could silently desynchronize. Pinning
        // the buffer alone would let that pass as an intentional visual edit.
        let digests: Vec<(u64, (u64, usize))> = [
            Rect::new(0, 0, 100, 30),
            Rect::new(0, 0, 80, 24),
            Rect::new(0, 0, 60, 20),
            Rect::new(0, 0, 40, 14),
            Rect::new(0, 0, 24, 8),
        ]
        .into_iter()
        .map(|area| {
            (
                buffer_digest(&render_collage_in(&app, false, now, area)),
                hit_map_digest(&app, area, false, now),
            )
        })
        .collect();

        assert_eq!(
            digests,
            vec![
                (4_134_155_508_641_885_490, (5_223_541_943_661_906_365, 113)),
                (1_856_660_865_147_035_786, (6_886_043_681_781_501_908, 105)),
                (16_766_417_207_939_638_788, (10_069_271_558_196_335_542, 53)),
                (5_724_836_692_193_432_250, (6_305_808_145_671_547_361, 5)),
                (3_410_854_746_564_216_135, (6_954_891_355_991_016_997, 0)),
            ],
            "a layout tier's composed stage or hit map changed"
        );
    }

    #[test]
    fn characterization_pins_stale_and_low_power_visuals() {
        let now = Instant::now();

        let mut stale = characterization_statuses(now);
        stale.apply(Action::AgentPollFailed { now });
        push_frame(&mut stale, phase_frame_with_offset(1.7));
        let stale_buf = render_collage_in(&stale, false, now, CANVAS);
        assert_eq!(
            buffer_digest(&stale_buf),
            13_212_696_367_813_413_304,
            "the frozen, dimmed stale composition changed"
        );
        assert_eq!(
            hit_map_digest(&stale, CANVAS, false, now),
            all_miss_digest(CANVAS),
            "stale must resolve no selection anywhere"
        );

        // Non-Working agents only: their orbit phase is zero regardless of the
        // wall clock the low-power capture reads internally, so the frozen
        // low-power layout is reproducible.
        let mut low_power = App::new(Settings::default(), Catalog::curated());
        low_power.apply(Action::AgentSnapshot {
            agents: vec![
                snap("ws", "i", Some("idler"), AgentStatus::Idle),
                snap("ws", "b", Some("blocked"), AgentStatus::Blocked),
                snap("ws", "d", Some("doner"), AgentStatus::Done),
            ],
            now,
        });
        low_power.apply(Action::ToggleAgentOverlay);
        low_power.configure_low_power_visuals(true);
        push_frame(&mut low_power, phase_frame());
        push_frame(&mut low_power, phase_frame_with_offset(2.3));
        let low_power_buf = render_collage_in(&low_power, true, now, CANVAS);
        assert_eq!(
            buffer_digest(&low_power_buf),
            3_224_760_117_707_571_664,
            "the frozen low-power composition changed"
        );
        assert_eq!(
            hit_map_digest(&low_power, CANVAS, true, now),
            (1_171_295_634_309_455_529, 67),
            "low-power hit testing no longer matches the frozen layout"
        );
    }

    #[test]
    fn characterization_pins_modal_and_unavailable_stages() {
        let now = Instant::now();

        let mut modal = characterization_statuses(now);
        modal.apply(Action::OpenAgentDetails);
        assert_eq!(
            buffer_digest(&render_collage_in(&modal, false, now, CANVAS)),
            7_094_265_579_201_153_038,
            "the agent table modal composition changed"
        );

        let mut rename = characterization_statuses(now);
        rename.apply(Action::OpenAgentDetails);
        rename.apply(Action::OpenAgentRename);
        rename.apply(Action::AppendAgentRename('n'));
        assert_eq!(
            buffer_digest(&render_collage_in(&rename, false, now, CANVAS)),
            17_122_930_139_269_336_662,
            "the inline rename footer composition changed"
        );

        let mut unavailable = characterization_statuses(now);
        unavailable.apply(Action::AgentPollFailed {
            now: now + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert_eq!(
            buffer_digest(&render_collage_in(&unavailable, false, now, CANVAS)),
            4_148_401_627_383_118_481,
            "the unavailable stage composition changed"
        );
    }
}
