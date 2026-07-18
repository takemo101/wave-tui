//! Agent Pulse rendering: the tiny `● n active` summary and the full-screen,
//! music-reactive Beat Orbit canvas.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. The canvas derives a deterministic
//! concentric-ring layout from stable agent identity, then displaces and
//! brightens particles from the actual played-sample [`crate::model::VizFrame`]
//! (RMS plus the FFT column under each particle) — never from a timer.
//! Silence leaves a dim, static field; `--low-power` fixes positions and
//! trails while state colors and minimal brightness still update.
//!
//! Mouse input flows through [`hit_test`], which shares [`beat_orbit_layout`]
//! with rendering so a click resolves against the same stable slots that were
//! drawn, and returns only the read-only selection [`Action`]; the CLI event
//! loop owns applying it.
//!
//! Privacy: a selected particle may show the explicit Herdr agent `name`
//! only. No pane id, workspace id, cwd, or agent type is ever rendered.
//! All colors come from the active [`Theme`]; no palette values are added.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Widget},
};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::app::{Action, AgentPulseConnection, AgentView, App};
use crate::herdr::AgentStatus;
use crate::theme::Theme;

/// Rough number of particles per ring before another ring is added.
const RING_TARGET: usize = 8;
/// Maximum horizontal displacement (cells) at full music energy.
const MAX_RADIAL_X: f32 = 3.0;
/// Terminal cells are roughly twice as tall as wide; vertical motion and
/// radii use this factor so the orbit reads as circular.
const CELL_ASPECT_Y: f32 = 0.5;
/// Below this energy the field counts as silent: dim and static.
const SILENCE_ENERGY: f32 = 0.05;
/// Above this energy a working particle brightens to bold.
const BRIGHT_ENERGY: f32 = 0.6;
/// A displaced particle leaves its faint trail above this energy.
const TRAIL_ENERGY: f32 = 0.25;

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

/// Short lowercase state label for the selected-particle line.
fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// Particle glyph per status.
fn status_glyph(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "●",
        AgentStatus::Blocked => "◆",
        AgentStatus::Idle => "○",
        AgentStatus::Done => "✓",
        AgentStatus::Unknown => "?",
    }
}

/// Theme color per status: working is strongest, blocked demands attention,
/// idle/done/unknown stay muted.
fn status_color(status: AgentStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        AgentStatus::Working => theme.playing,
        AgentStatus::Blocked => theme.error,
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => theme.muted,
    }
}

// --- stable orbit layout ----------------------------------------------------

/// One placed particle: the index into `App::active_agents()`, its stable
/// cell anchor, and the slot angle used for radial displacement.
pub(super) struct OrbitParticle {
    pub(super) index: usize,
    pub(super) anchor: (u16, u16),
    pub(super) angle: f32,
}

/// Pure orbit geometry shared by rendering and hit testing.
pub(super) struct BeatOrbitLayout {
    pub(super) particles: Vec<OrbitParticle>,
}

/// Stable placement seed for an agent: a hash of its private identity, so a
/// status change never moves a particle and no pane detail is exposed.
fn seed_of(view: &AgentView) -> u64 {
    let mut hasher = DefaultHasher::new();
    view.id.hash(&mut hasher);
    hasher.finish()
}

/// Compute the concentric-ring layout for every agent inside `area`.
///
/// Deterministic and status-independent: agents are ordered by their stable
/// identity seed, then fill rings whose capacity grows outward. Every agent
/// gets exactly one particle regardless of density — dense areas only shrink
/// spacing. No randomness and no clock.
pub(super) fn beat_orbit_layout(agents: &[AgentView], area: Rect) -> BeatOrbitLayout {
    if agents.is_empty() || area.width == 0 || area.height == 0 {
        return BeatOrbitLayout {
            particles: Vec::new(),
        };
    }

    // Stable, status-independent placement order.
    let mut order: Vec<(u64, usize)> = (0..agents.len())
        .map(|index| (seed_of(&agents[index]), index))
        .collect();
    order.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| agents[a.1].id.cmp(&agents[b.1].id))
    });

    let n = order.len();
    let cx = area.x as f32 + (area.width.saturating_sub(1)) as f32 / 2.0;
    let cy = area.y as f32 + (area.height.saturating_sub(1)) as f32 / 2.0;
    let max_rx = ((area.width.saturating_sub(1)) as f32 / 2.0 - 1.0).max(1.0);
    let max_ry = ((area.height.saturating_sub(1)) as f32 / 2.0).max(1.0);

    let rings = n.div_ceil(RING_TARGET).clamp(1, (max_ry as usize).max(1));
    // Ring capacities grow with the ring number and are adjusted (outer rings
    // first, where there is the most room) to sum to exactly `n`.
    let weight_sum: usize = (1..=rings).sum();
    let mut caps: Vec<usize> = (1..=rings).map(|i| n * i / weight_sum).collect();
    let mut assigned: usize = caps.iter().sum();
    let mut fill = rings - 1;
    while assigned < n {
        caps[fill] += 1;
        assigned += 1;
        fill = if fill == 0 { rings - 1 } else { fill - 1 };
    }

    let mut particles = Vec::with_capacity(n);
    let mut cursor = order.into_iter().map(|(_, index)| index);
    for (ring, cap) in caps.iter().enumerate() {
        let fraction = (ring + 1) as f32 / (rings + 1) as f32;
        let rx = max_rx * fraction;
        let ry = max_ry * fraction;
        // A fixed per-ring phase offset keeps neighboring rings from lining
        // their slots up into visible spokes; it depends on the ring only,
        // so it is deterministic.
        let phase = ring as f32 * 0.7399 + 0.5;
        for slot in 0..*cap {
            let Some(index) = cursor.next() else {
                break;
            };
            let angle = std::f32::consts::TAU * slot as f32 / (*cap).max(1) as f32 + phase;
            let x = (cx + rx * angle.cos()).round() as i32;
            let y = (cy + ry * angle.sin()).round() as i32;
            let x = x.clamp(area.x as i32, (area.x + area.width - 1) as i32) as u16;
            let y = y.clamp(area.y as i32, (area.y + area.height - 1) as i32) as u16;
            particles.push(OrbitParticle {
                index,
                anchor: (x, y),
                angle,
            });
        }
    }

    BeatOrbitLayout { particles }
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

/// The orbit region inside the canvas: below the title/banner rows and above
/// the label/footer rows.
fn orbit_area(area: Rect) -> Rect {
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

/// Pure mouse hit test for the Beat Orbit canvas.
///
/// Maps a click on a particle's stable slot (a three-cell target centered on
/// its anchor) to the read-only [`Action::SelectAgent`]; returns `None`
/// whenever the canvas is closed, the integration is hidden, the connection
/// is stale or unavailable, Signal View is active, or the click misses every
/// particle. Clicks resolve against anchors, not displaced positions, so a
/// beat never steals a click.
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
    let layout = beat_orbit_layout(agents, orbit_area(area));
    for particle in &layout.particles {
        let (ax, ay) = particle.anchor;
        if row == ay && column + 1 >= ax && column <= ax + 1 {
            let view = agents.get(particle.index)?;
            return Some(Action::SelectAgent(view.id.clone()));
        }
    }
    None
}

// --- canvas rendering -------------------------------------------------------

/// Render the full-screen Beat Orbit canvas over the composed normal layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the title/count, the particle
/// field driven by the current [`crate::model::VizFrame`], the selected
/// explicit-name label, and a restrained footer hint. Stale renders the last
/// orbit frozen and dimmed with a `reconnecting` banner; Unavailable hides
/// every particle behind calm copy. `now` is injected by the render entry
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

    if agents.is_empty() {
        center_copy(buf, area, "agents · none active", muted);
        footer(buf, area, muted);
        return;
    }

    let orbit = orbit_area(area);
    let layout = beat_orbit_layout(agents, orbit);
    let frame = app.viz();
    let columns = super::visualizer::spectrum_columns(&frame.bands, orbit.width as usize);
    let selected_index = app
        .selected_agent()
        .and_then(|selected| agents.iter().position(|view| view.id == selected.id));

    for particle in &layout.particles {
        let Some(view) = agents.get(particle.index) else {
            continue;
        };
        // Energy: overall RMS blended with the FFT column under the anchor,
        // so louder music expands the orbit and each particle rides its own
        // slice of the spectrum.
        let column = (particle.anchor.0 - orbit.x) as usize;
        let band = columns.get(column).map_or(0.0, |c| c.0);
        let energy = (frame.rms * 0.65 + band * 0.35).clamp(0.0, 1.0);

        let animate = !low_power && !stale;
        let (x, y) = if animate {
            displaced(particle, energy, orbit)
        } else {
            particle.anchor
        };
        if animate && (x, y) != particle.anchor && energy > TRAIL_ENERGY {
            buf.set_string(particle.anchor.0, particle.anchor.1, "·", dim_muted);
        }

        let style = if selected_index == Some(particle.index) {
            with_stale(theme.selection_style(), stale)
        } else {
            particle_style(view.status, theme, energy, stale, low_power)
        };
        buf.set_string(x, y, status_glyph(view.status), style);
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

/// Anchor displaced outward along its slot angle by the current energy.
fn displaced(particle: &OrbitParticle, energy: f32, orbit: Rect) -> (u16, u16) {
    let radial = energy * MAX_RADIAL_X;
    let dx = (radial * particle.angle.cos()).round() as i32;
    let dy = (radial * particle.angle.sin() * CELL_ASPECT_Y).round() as i32;
    let x =
        (particle.anchor.0 as i32 + dx).clamp(orbit.x as i32, (orbit.x + orbit.width - 1) as i32);
    let y =
        (particle.anchor.1 as i32 + dy).clamp(orbit.y as i32, (orbit.y + orbit.height - 1) as i32);
    (x as u16, y as u16)
}

/// Particle style: theme status color, silence dims, strong signal
/// emboldens working particles (never in low power), done is always faded,
/// and stale dims everything.
fn particle_style(
    status: AgentStatus,
    theme: &Theme,
    energy: f32,
    stale: bool,
    low_power: bool,
) -> Style {
    let mut style = Style::default().fg(status_color(status, theme));
    if status == AgentStatus::Done {
        style = style.add_modifier(Modifier::DIM);
    }
    if energy < SILENCE_ENERGY {
        style = style.add_modifier(Modifier::DIM);
    } else if status == AgentStatus::Working && energy > BRIGHT_ENERGY && !low_power {
        style = style.add_modifier(Modifier::BOLD);
    }
    with_stale(style, stale)
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
    use crate::model::VizFrame;
    use crate::settings::Settings;
    use crate::theme::ThemeName;
    use std::time::Duration;

    const CANVAS: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };

    fn view(workspace: &str, pane: &str, status: AgentStatus) -> AgentView {
        AgentView {
            id: AgentId::new(workspace, pane),
            name: None,
            status,
            observed_at: Instant::now(),
        }
    }

    fn many_views(count: usize) -> Vec<AgentView> {
        (0..count)
            .map(|i| view("ws", &format!("p{i}"), AgentStatus::Working))
            .collect()
    }

    fn snap(workspace: &str, pane: &str, name: Option<&str>, status: AgentStatus) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new(workspace, pane),
            name: name.map(str::to_string),
            status,
        }
    }

    /// A connected app with the canvas open.
    fn orbit_app(agents: Vec<AgentSnapshot>) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents,
            now: Instant::now(),
        });
        app.apply(Action::ToggleAgentOverlay);
        app
    }

    fn set_frame(app: &mut App, rms: f32, band: f32) {
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![band; 16],
            rms,
            Vec::<f32>::new(),
        ))));
    }

    fn render_orbit(app: &App, low_power: bool, now: Instant) -> Buffer {
        let mut buf = Buffer::empty(CANVAS);
        let theme = Theme::for_name(ThemeName::Minimal);
        render_canvas(app, &theme, low_power, now, CANVAS, &mut buf);
        buf
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

    /// Positions of every particle glyph cell, in scan order.
    fn glyph_positions(buf: &Buffer) -> Vec<(u16, u16, String)> {
        let area = *buf.area();
        let mut positions = Vec::new();
        for y in 0..area.height {
            for x in 0..area.width {
                let symbol = buf.cell((x, y)).unwrap().symbol();
                if ["●", "◆", "○", "✓", "?"].contains(&symbol) {
                    positions.push((x, y, symbol.to_string()));
                }
            }
        }
        positions
    }

    // --- layout ----------------------------------------------------------

    #[test]
    fn orbit_slots_are_stable_when_status_changes() {
        let before = beat_orbit_layout(&[view("alpha", "p1", AgentStatus::Working)], CANVAS);
        let after = beat_orbit_layout(&[view("alpha", "p1", AgentStatus::Blocked)], CANVAS);
        assert_eq!(before.particles[0].anchor, after.particles[0].anchor);
    }

    #[test]
    fn orbit_slots_differ_for_identical_panes_in_different_workspaces() {
        let layout = beat_orbit_layout(
            &[
                view("alpha", "p1", AgentStatus::Working),
                view("beta", "p1", AgentStatus::Working),
            ],
            CANVAS,
        );
        assert_eq!(layout.particles.len(), 2);
        assert_ne!(layout.particles[0].anchor, layout.particles[1].anchor);
    }

    #[test]
    fn dense_layout_keeps_one_particle_per_agent() {
        let area = Rect::new(0, 0, 50, 15);
        let layout = beat_orbit_layout(&many_views(80), area);
        assert_eq!(layout.particles.len(), 80);
        for particle in &layout.particles {
            let (x, y) = particle.anchor;
            assert!(
                x < area.width && y < area.height,
                "anchor ({x}, {y}) escaped the {area:?}"
            );
        }
    }

    // --- music reactivity -------------------------------------------------

    #[test]
    fn rms_and_bands_move_orbit_without_timer_only_motion() {
        let t0 = Instant::now();
        let mut app = orbit_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Working),
            snap("ws", "p3", Some("three"), AgentStatus::Working),
        ]);
        set_frame(&mut app, 0.05, 0.0);
        let quiet = render_orbit(&app, false, t0);
        set_frame(&mut app, 0.90, 0.8);
        let loud = render_orbit(&app, false, t0);
        assert_ne!(quiet, loud, "audio frames must drive the orbit");
        assert_ne!(
            glyph_positions(&quiet),
            glyph_positions(&loud),
            "loud frames must displace particles, not just restyle them"
        );
    }

    #[test]
    fn silence_is_dim_and_static_across_time() {
        let t0 = Instant::now();
        let mut app = orbit_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Idle),
        ]);
        set_frame(&mut app, 0.0, 0.0);
        let first = render_orbit(&app, false, t0);
        let later = render_orbit(&app, false, t0 + Duration::from_secs(9));
        assert_eq!(first, later, "silent field must not animate with time");
        for (x, y, _) in glyph_positions(&first) {
            assert!(
                first
                    .cell((x, y))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::DIM),
                "silent particles must be dim"
            );
        }
    }

    #[test]
    fn low_power_keeps_positions_fixed_while_colors_remain() {
        let mut app = orbit_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Blocked),
            snap("ws", "p3", Some("three"), AgentStatus::Idle),
        ]);
        let t0 = Instant::now();
        set_frame(&mut app, 0.05, 0.0);
        let quiet = glyph_positions(&render_orbit(&app, true, t0));
        set_frame(&mut app, 0.90, 0.8);
        let loud_low = glyph_positions(&render_orbit(&app, true, t0));
        assert!(!quiet.is_empty());
        assert_eq!(quiet, loud_low, "low power fixes particle positions");

        // The same loud frame in normal power does move the orbit.
        let loud_normal = glyph_positions(&render_orbit(&app, false, t0));
        assert_ne!(loud_low, loud_normal);
    }

    // --- state, selection, and privacy -------------------------------------

    #[test]
    fn state_colors_come_from_the_theme() {
        let mut app = orbit_app(vec![
            snap("ws", "p1", Some("w"), AgentStatus::Working),
            snap("ws", "p2", Some("b"), AgentStatus::Blocked),
            snap("ws", "p3", Some("i"), AgentStatus::Idle),
            snap("ws", "p4", Some("d"), AgentStatus::Done),
        ]);
        set_frame(&mut app, 0.5, 0.4);
        let theme = Theme::for_name(ThemeName::Minimal);
        let buf = render_orbit(&app, false, Instant::now());
        for (x, y, glyph) in glyph_positions(&buf) {
            let style = buf.cell((x, y)).unwrap().style();
            match glyph.as_str() {
                "●" => assert_eq!(style.fg, Some(theme.playing), "working uses playing color"),
                "◆" => assert_eq!(style.fg, Some(theme.error), "blocked uses error color"),
                "○" => assert_eq!(style.fg, Some(theme.muted), "idle stays muted"),
                "✓" => {
                    assert_eq!(style.fg, Some(theme.muted), "done stays muted");
                    assert!(
                        style.add_modifier.contains(Modifier::DIM),
                        "done fades until its snapshot removes it"
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn selected_particle_shows_the_explicit_name_only() {
        let mut app = orbit_app(vec![snap(
            "alpha",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        let unselected = buffer_text(&render_orbit(&app, false, Instant::now()));
        assert!(
            !unselected.contains("research"),
            "no label before selection: {unselected}"
        );

        app.apply(Action::SelectNextAgent);
        let selected = buffer_text(&render_orbit(&app, false, Instant::now()));
        assert!(
            selected.contains("research · working"),
            "selected label missing: {selected}"
        );
    }

    #[test]
    fn selecting_an_unnamed_agent_shows_no_label_at_all() {
        let mut app = orbit_app(vec![snap("alpha", "p1", None, AgentStatus::Working)]);
        app.apply(Action::SelectNextAgent);
        assert!(app.selected_agent().is_some());
        let text = buffer_text(&render_orbit(&app, false, Instant::now()));
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
    fn stale_freezes_and_dims_the_last_orbit() {
        let mut app = orbit_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        set_frame(&mut app, 0.9, 0.8);
        let live = glyph_positions(&render_orbit(&app, false, Instant::now()));
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let buf = render_orbit(&app, false, Instant::now());
        let frozen = glyph_positions(&buf);
        assert_eq!(frozen.len(), 1, "the last orbit is retained");
        assert_ne!(live, frozen, "stale freezes particles onto their anchors");
        assert!(buffer_text(&buf).contains("reconnecting"));
        for (x, y, _) in frozen {
            assert!(
                buf.cell((x, y))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::DIM),
                "stale particles are dimmed"
            );
        }
    }

    #[test]
    fn unavailable_hides_particles_behind_calm_copy() {
        let mut app = orbit_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let buf = render_orbit(&app, false, Instant::now());
        assert!(glyph_positions(&buf).is_empty(), "no particles render");
        assert!(buffer_text(&buf).contains("agents · unavailable · retrying"));
    }

    #[test]
    fn canvas_is_a_noop_while_closed() {
        let mut app = orbit_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::CloseAgentOverlay);
        let buf = render_orbit(&app, false, Instant::now());
        assert_eq!(buf, Buffer::empty(CANVAS), "closed canvas draws nothing");
    }

    // --- hit testing --------------------------------------------------------

    #[test]
    fn clicking_a_particle_slot_selects_that_agent() {
        let mut app = orbit_app(vec![
            snap("alpha", "p1", Some("research"), AgentStatus::Working),
            snap("beta", "p1", Some("review"), AgentStatus::Idle),
        ]);
        let layout = beat_orbit_layout(app.active_agents(), orbit_area(CANVAS));
        let review_index = app
            .active_agents()
            .iter()
            .position(|view| view.name.as_deref() == Some("review"))
            .unwrap();
        let particle = layout
            .particles
            .iter()
            .find(|particle| particle.index == review_index)
            .unwrap();
        let (x, y) = particle.anchor;

        let action = hit_test(CANVAS, x, y, &app).expect("anchor click selects");
        app.apply(action);
        assert_eq!(
            app.selected_agent().unwrap().name.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn clicks_resolve_nothing_when_missed_stale_or_closed() {
        let mut app = orbit_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        assert!(hit_test(CANVAS, 0, 0, &app).is_none(), "corner miss");

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let layout = beat_orbit_layout(app.active_agents(), orbit_area(CANVAS));
        let (x, y) = layout.particles[0].anchor;
        assert!(
            hit_test(CANVAS, x, y, &app).is_none(),
            "stale ignores clicks"
        );

        let mut closed = orbit_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
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
        let app = orbit_app(vec![
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

        let mut unavailable = orbit_app(vec![snap("ws", "p1", None, AgentStatus::Working)]);
        unavailable.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert!(summary_line(&unavailable, &theme).is_none());
    }
}
