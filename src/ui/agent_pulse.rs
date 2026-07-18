//! Agent Pulse rendering: the tiny `● n active` summary and the full-screen,
//! music-reactive Bioluminescent Current canvas.
//!
//! Everything here is read-only presentation over the Agent Pulse display
//! accessors on [`App`]: this module never calls the Herdr adapter, opens
//! sockets, or mutates app state. The canvas derives a continuous flow line
//! from the actual played-sample [`crate::model::VizFrame`] FFT bands, places
//! one stable light per agent along that current, and drives each light's
//! glow, size, and short upstream trail from RMS, its assigned band, and the
//! real recent frames in `App::viz_history()` — never from a timer. Silence
//! leaves a dim, still current; `--low-power` freezes flow, light positions,
//! and trails while state colors and minimal brightness still update.
//!
//! Mouse input flows through [`hit_test`], which shares [`current_layout`]
//! with rendering so a click resolves against the same light cells that were
//! drawn, and returns only the read-only selection [`Action`]; the CLI event
//! loop owns applying it.
//!
//! Privacy: a selected light may show the explicit Herdr agent `name` only.
//! No pane id, workspace id, cwd, or agent type is ever rendered. All colors
//! come from the active [`Theme`]; no palette values are added.

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
use crate::model::VizFrame;
use crate::theme::Theme;

/// Fraction of the flow region's half-height a full-magnitude band may swing.
const FLOW_SWING: f32 = 0.8;
/// Spatial undulation cycles across the canvas width; a pure function of the
/// column position, so the current meanders without any clock input.
const FLOW_WAVES: f32 = 1.5;
/// Below this energy/magnitude the field counts as silent: dim and still.
const SILENCE_ENERGY: f32 = 0.05;
/// Above this energy a working light brightens to bold.
const BRIGHT_ENERGY: f32 = 0.6;
/// A light leaves upstream trail cells above this energy.
const TRAIL_ENERGY: f32 = 0.1;
/// Maximum prior frames contributing one trail cell each per light.
const TRAIL_CELLS: usize = 3;

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

/// Short lowercase state label for the selected-light line.
fn status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

/// Light glyph per status.
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

// --- pure current geometry --------------------------------------------------

/// One column of the flow polyline: its cell plus the interpolated band
/// magnitude and normalized position used for glyph weight and gradient color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FlowCell {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) magnitude: f32,
    pub(super) position: f32,
}

/// One placed agent light: the index into `App::active_agents()`, its stable
/// identity seed and flow column, the drawn cell, its music-driven halo
/// radius and energy, and the upstream trail cells derived from real prior
/// frames.
pub(super) struct CurrentLight {
    pub(super) index: usize,
    /// The identity hash behind the anchor: rendering only needs the derived
    /// `anchor_x`, but the seed is the stability contract tests pin down.
    #[allow(dead_code)]
    pub(super) anchor_seed: u64,
    pub(super) anchor_x: u16,
    pub(super) cell: (u16, u16),
    pub(super) radius: u16,
    pub(super) energy: f32,
    pub(super) trail_cells: Vec<(u16, u16)>,
}

/// Pure Bioluminescent Current geometry shared by rendering and hit testing.
pub(super) struct CurrentLayout {
    pub(super) flow_cells: Vec<FlowCell>,
    pub(super) lights: Vec<CurrentLight>,
}

/// Stable placement seed for an agent: a hash of its private identity, so a
/// status change never moves a light and no pane detail is exposed.
fn seed_of(view: &AgentView) -> u64 {
    let mut hasher = DefaultHasher::new();
    view.id.hash(&mut hasher);
    hasher.finish()
}

/// One `(magnitude, position)` flow column per cell of `width`.
///
/// Reuses the shared Spectrum Stack resampler; an empty/missing frame yields
/// zero-magnitude columns so the current still renders as a flat, dim line.
fn flow_columns(bands: &[f32], width: usize) -> Vec<(f32, f32)> {
    let columns = super::visualizer::spectrum_columns(bands, width);
    if !columns.is_empty() {
        return columns;
    }
    let last = width.saturating_sub(1).max(1) as f32;
    (0..width).map(|col| (0.0, col as f32 / last)).collect()
}

/// Flow y-coordinate for a column: the vertical middle displaced by the band
/// magnitude riding a fixed spatial undulation. Zero magnitude is exactly the
/// flat middle line, so silence and low power are still by construction.
fn flow_y(area: Rect, magnitude: f32, position: f32) -> u16 {
    let cy = area.y as f32 + area.height.saturating_sub(1) as f32 / 2.0;
    let amp = area.height.saturating_sub(1) as f32 / 2.0 * FLOW_SWING;
    let ripple = (position * std::f32::consts::TAU * FLOW_WAVES).sin();
    let y = (cy - magnitude * amp * ripple).round() as i32;
    y.clamp(
        area.y as i32,
        (area.y + area.height).saturating_sub(1) as i32,
    ) as u16
}

/// Local flow direction at `col` as a -1/0/1 slope sign, used to nudge an
/// energetic light along its current rather than straight off it.
fn flow_tangent(flow: &[FlowCell], col: usize) -> i32 {
    if flow.is_empty() {
        return 0;
    }
    let left = flow[col.saturating_sub(1)].y as i32;
    let right = flow[(col + 1).min(flow.len() - 1)].y as i32;
    (right - left).signum()
}

/// Compute the full Current geometry for `agents` inside `area`.
///
/// Deterministic and clock-free: the flow polyline comes only from the frame's
/// FFT bands, every agent gets exactly one light at a stable, status-independent
/// column (dense terminals shrink spacing rather than omitting lights), and
/// trail cells come only from `history` (most recent first). `low_power`
/// returns the flat baseline geometry with no trails; callers keep using the
/// frame for color and brightness only.
pub(super) fn current_layout(
    agents: &[AgentView],
    frame: &VizFrame,
    history: &[VizFrame],
    area: Rect,
    low_power: bool,
) -> CurrentLayout {
    if area.width == 0 || area.height == 0 {
        return CurrentLayout {
            flow_cells: Vec::new(),
            lights: Vec::new(),
        };
    }

    let width = area.width as usize;
    let columns = flow_columns(&frame.bands, width);
    let flow_cells: Vec<FlowCell> = columns
        .iter()
        .enumerate()
        .map(|(col, &(magnitude, position))| {
            let displacement = if low_power { 0.0 } else { magnitude };
            FlowCell {
                x: area.x + col as u16,
                y: flow_y(area, displacement, position),
                magnitude,
                position,
            }
        })
        .collect();

    // Prior-frame flow columns, resampled once per frame, for trail cells.
    let history_columns: Vec<Vec<(f32, f32)>> = history
        .iter()
        .take(TRAIL_CELLS)
        .map(|old| flow_columns(&old.bands, width))
        .collect();

    // Stable, status-independent placement order along the current.
    let mut order: Vec<(u64, usize)> = (0..agents.len())
        .map(|index| (seed_of(&agents[index]), index))
        .collect();
    order.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| agents[a.1].id.cmp(&agents[b.1].id))
    });

    let n = order.len().max(1);
    let baseline = flow_y(area, 0.0, 0.0);
    let lights = order
        .into_iter()
        .enumerate()
        .map(|(slot, (seed, index))| {
            // Even spread plus a tiny identity jitter keeps every agent its
            // own column while staying deterministic; dense fields only
            // shrink spacing, never drop a light.
            let base = (slot * width + width / 2) / n;
            let jitter = (seed % 3) as i32 - 1;
            let anchor_x = (area.x as i32 + base as i32 + jitter)
                .clamp(area.x as i32, (area.x + area.width - 1) as i32)
                as u16;
            let col = (anchor_x - area.x) as usize;
            let band = columns.get(col).map_or(0.0, |column| column.0);
            let energy = (frame.rms * 0.55 + band * 0.45).clamp(0.0, 1.0);
            let radius = if low_power {
                0
            } else {
                (energy * 2.0).round() as u16
            };

            let cell = if low_power {
                (anchor_x, baseline)
            } else {
                let dy = flow_tangent(&flow_cells, col) * radius.min(1) as i32;
                let y = (flow_cells[col].y as i32 + dy)
                    .clamp(area.y as i32, (area.y + area.height - 1) as i32)
                    as u16;
                (anchor_x, y)
            };

            let trail_cells = if low_power || energy <= TRAIL_ENERGY {
                Vec::new()
            } else {
                history_columns
                    .iter()
                    .enumerate()
                    .filter_map(|(age, old)| {
                        let x = anchor_x as i32 - (age as i32 + 1);
                        if x < area.x as i32 {
                            return None;
                        }
                        let old_col = (x - area.x as i32) as usize;
                        let &(magnitude, position) = old.get(old_col)?;
                        Some((x as u16, flow_y(area, magnitude, position)))
                    })
                    .collect()
            };

            CurrentLight {
                index,
                anchor_seed: seed,
                anchor_x,
                cell,
                radius,
                energy,
                trail_cells,
            }
        })
        .collect();

    CurrentLayout { flow_cells, lights }
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

/// The flow region inside the canvas: below the title/banner rows and above
/// the label/footer rows.
fn flow_area(area: Rect) -> Rect {
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

/// Pure mouse hit test for the Bioluminescent Current canvas.
///
/// Maps a click on a light's current cells (its core plus halo width on the
/// drawn row, or its stable baseline anchor so low-power/quiet fields stay
/// clickable) to the read-only [`Action::SelectAgent`]; returns `None`
/// whenever the canvas is closed, the integration is hidden, the connection
/// is stale or unavailable, Signal View is active, or the click misses every
/// light. Flow and trail cells resolve nothing.
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
    let flow = flow_area(area);
    let layout = current_layout(agents, app.viz(), &[], flow, false);
    let baseline = flow_y(flow, 0.0, 0.0);
    for light in &layout.lights {
        let (x, y) = light.cell;
        let span = light.radius.max(1);
        let on_light = row == y && column + span >= x && column <= x + span;
        let on_anchor = row == baseline && column == light.anchor_x;
        if on_light || on_anchor {
            let view = agents.get(light.index)?;
            return Some(Action::SelectAgent(view.id.clone()));
        }
    }
    None
}

// --- canvas rendering -------------------------------------------------------

/// Render the full-screen Bioluminescent Current over the composed layout.
///
/// A no-op unless the canvas is active, so normal and standalone output is
/// untouched. Clears the full area, then draws the title/count, the
/// FFT-derived flow line, each agent's trail, halo, and state-colored light,
/// the selected explicit-name label, and a restrained footer hint. Stale
/// renders the frozen baseline geometry dimmed under a `reconnecting` banner;
/// Unavailable hides every light behind calm copy. `now` is injected by the
/// render entry point but deliberately unused: motion derives from audio
/// frames only.
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

    let flow = flow_area(area);
    // Stale renders the display captured by the reducer at the
    // Connected→Stale edge, freezing the exact last live current and trails
    // (then dimmed below); live renders use the current frame plus the real
    // prior frames behind it. Only `--low-power` flattens geometry.
    let (frame, history): (&VizFrame, Vec<VizFrame>) = match app.stale_viz().filter(|_| stale) {
        Some((frame, history)) => (frame, history.to_vec()),
        None => (app.viz(), app.viz_history().skip(1).cloned().collect()),
    };
    let layout = current_layout(agents, frame, &history, flow, low_power);

    for cell in &layout.flow_cells {
        let mut style = Style::default().fg(theme.spectrum_color(cell.position));
        if cell.magnitude < SILENCE_ENERGY {
            style = style.add_modifier(Modifier::DIM);
        }
        buf.set_string(
            cell.x,
            cell.y,
            flow_glyph(cell.magnitude),
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

    for light in &layout.lights {
        let Some(view) = agents.get(light.index) else {
            continue;
        };
        for &(x, y) in &light.trail_cells {
            buf.set_string(x, y, "∙", dim_muted);
        }
        // Halo cells widen the light with energy; dim so the core stays the
        // brightest point of its glow.
        let halo = Style::default()
            .fg(status_color(view.status, theme))
            .add_modifier(Modifier::DIM);
        for step in 1..=light.radius {
            for x in [
                light.cell.0.saturating_sub(step),
                light.cell.0.saturating_add(step),
            ] {
                if x != light.cell.0 && rect_contains(flow, x, light.cell.1) {
                    buf.set_string(x, light.cell.1, "◦", with_stale(halo, stale));
                }
            }
        }

        let style = if selected_index == Some(light.index) {
            with_stale(theme.selection_style(), stale)
        } else {
            light_style(view.status, theme, light.energy, stale, low_power)
        };
        buf.set_string(light.cell.0, light.cell.1, status_glyph(view.status), style);
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

/// Flow glyph weight by band magnitude: heavier water for louder bands.
fn flow_glyph(magnitude: f32) -> &'static str {
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

/// Light style: theme status color, silence dims, strong signal emboldens
/// working lights (never in low power), done is always faded, and stale dims
/// everything.
fn light_style(
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
    fn current_app(agents: Vec<AgentSnapshot>) -> App {
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

    fn render_current_for(app: &App, low_power: bool, now: Instant) -> Buffer {
        let mut buf = Buffer::empty(CANVAS);
        let theme = Theme::for_name(ThemeName::Minimal);
        render_canvas(app, &theme, low_power, now, CANVAS, &mut buf);
        buf
    }

    /// Render an open canvas of `count` working agents after feeding the prior
    /// `history` frames (oldest first) and then the current `frame`.
    fn render_current(
        count: usize,
        frame: VizFrame,
        history: Vec<VizFrame>,
        low_power: bool,
    ) -> Buffer {
        let snaps = (0..count)
            .map(|i| snap("ws", &format!("p{i}"), None, AgentStatus::Working))
            .collect();
        let mut app = current_app(snaps);
        for old in history {
            push_frame(&mut app, old);
        }
        push_frame(&mut app, frame);
        render_current_for(&app, low_power, Instant::now())
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

    /// Positions of every light glyph cell, in scan order.
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

    /// Trail cells use a glyph no flow or light cell shares.
    fn count_trail_cells(buf: &Buffer) -> usize {
        buffer_text(buf).matches('∙').count()
    }

    /// Every non-blank cell of the flow region — flow, trails, halos, and
    /// lights — as `(x, y, symbol)`, so tests can compare whole-field
    /// geometry between renders.
    fn field_cells(buf: &Buffer) -> Vec<(u16, u16, String)> {
        let flow = flow_area(*buf.area());
        let mut cells = Vec::new();
        for y in flow.y..flow.y + flow.height {
            for x in flow.x..flow.x + flow.width {
                let symbol = buf.cell((x, y)).unwrap().symbol();
                if symbol != " " {
                    cells.push((x, y, symbol.to_string()));
                }
            }
        }
        cells
    }

    // --- pure current layout ----------------------------------------------

    #[test]
    fn current_flow_tracks_fft_shape_not_elapsed_time() {
        let area = Rect::new(0, 0, 80, 24);
        let low = current_layout(
            &agents(3),
            &frame(0.2, vec![0.1, 0.8, 0.2]),
            &[],
            area,
            false,
        );
        let high = current_layout(
            &agents(3),
            &frame(0.2, vec![0.8, 0.1, 0.8]),
            &[],
            area,
            false,
        );
        assert_ne!(low.flow_cells, high.flow_cells);
    }

    #[test]
    fn each_light_has_a_stable_anchor_and_a_short_history_trail() {
        let area = Rect::new(0, 0, 80, 24);
        let agents = agents(6);
        let first = current_layout(&agents, &frame(0.3, vec![0.2; 12]), &[], area, false);
        let next = current_layout(
            &agents,
            &frame(0.7, vec![0.8; 12]),
            &[frame(0.2, vec![0.1; 12])],
            area,
            false,
        );
        assert_eq!(first.lights[0].anchor_seed, next.lights[0].anchor_seed);
        assert!(!next.lights[0].trail_cells.is_empty());
    }

    #[test]
    fn current_anchors_are_stable_when_status_changes() {
        let before = current_layout(
            &[view("alpha", "p1", AgentStatus::Working)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
            false,
        );
        let after = current_layout(
            &[view("alpha", "p1", AgentStatus::Blocked)],
            &frame(0.4, vec![0.4; 16]),
            &[],
            CANVAS,
            false,
        );
        assert_eq!(before.lights[0].anchor_x, after.lights[0].anchor_x);
        assert_eq!(before.lights[0].anchor_seed, after.lights[0].anchor_seed);
    }

    #[test]
    fn current_anchors_differ_for_identical_panes_in_different_workspaces() {
        let layout = current_layout(
            &[
                view("alpha", "p1", AgentStatus::Working),
                view("beta", "p1", AgentStatus::Working),
            ],
            &frame(0.0, vec![0.0; 16]),
            &[],
            CANVAS,
            false,
        );
        assert_eq!(layout.lights.len(), 2);
        assert_ne!(layout.lights[0].anchor_x, layout.lights[1].anchor_x);
    }

    #[test]
    fn dense_current_keeps_one_light_per_agent() {
        let area = Rect::new(0, 0, 50, 15);
        let layout = current_layout(&agents(80), &frame(0.5, vec![0.5; 16]), &[], area, false);
        assert_eq!(layout.lights.len(), 80);
        for light in &layout.lights {
            let (x, y) = light.cell;
            assert!(
                x < area.width && y < area.height,
                "light ({x}, {y}) escaped the {area:?}"
            );
        }
    }

    #[test]
    fn low_power_layout_is_flat_with_no_trails() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = current_layout(
            &agents(4),
            &frame(0.9, vec![0.9; 16]),
            &[frame(0.5, vec![0.5; 16])],
            area,
            true,
        );
        let baseline = flow_y(area, 0.0, 0.0);
        for cell in &layout.flow_cells {
            assert_eq!(cell.y, baseline, "low-power flow is flat");
        }
        for light in &layout.lights {
            assert_eq!(
                light.cell.1, baseline,
                "low-power lights sit on the baseline"
            );
            assert_eq!(light.radius, 0);
            assert!(light.trail_cells.is_empty());
        }
    }

    // --- music reactivity -------------------------------------------------

    #[test]
    fn louder_audio_changes_light_glow_size_and_trails() {
        let quiet = render_current(4, frame(0.05, vec![0.05; 16]), vec![], false);
        let loud = render_current(
            4,
            frame(0.9, vec![0.9; 16]),
            vec![frame(0.4, vec![0.4; 16])],
            false,
        );
        assert_ne!(quiet, loud);
        assert!(count_trail_cells(&loud) > count_trail_cells(&quiet));
    }

    #[test]
    fn rms_and_bands_move_the_current_not_elapsed_time() {
        let t0 = Instant::now();
        let mut app = current_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Working),
            snap("ws", "p3", Some("three"), AgentStatus::Working),
        ]);
        push_frame(&mut app, frame(0.05, vec![0.0; 16]));
        let quiet = render_current_for(&app, false, t0);
        push_frame(&mut app, frame(0.90, vec![0.8; 16]));
        let loud = render_current_for(&app, false, t0);
        assert_ne!(quiet, loud, "audio frames must drive the current");
        assert_ne!(
            glyph_positions(&quiet),
            glyph_positions(&loud),
            "loud frames must move lights along the flow, not just restyle them"
        );
    }

    #[test]
    fn silence_is_dim_and_still_across_time() {
        let t0 = Instant::now();
        let mut app = current_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Idle),
        ]);
        push_frame(&mut app, frame(0.0, vec![0.0; 16]));
        let first = render_current_for(&app, false, t0);
        let later = render_current_for(&app, false, t0 + Duration::from_secs(9));
        assert_eq!(first, later, "silent current must not animate with time");
        assert_eq!(count_trail_cells(&first), 0, "silence leaves no trails");
        for (x, y, _) in glyph_positions(&first) {
            assert!(
                first
                    .cell((x, y))
                    .unwrap()
                    .style()
                    .add_modifier
                    .contains(Modifier::DIM),
                "silent lights must be dim"
            );
        }
    }

    #[test]
    fn low_power_keeps_positions_fixed_while_colors_remain() {
        let mut app = current_app(vec![
            snap("ws", "p1", Some("one"), AgentStatus::Working),
            snap("ws", "p2", Some("two"), AgentStatus::Blocked),
            snap("ws", "p3", Some("three"), AgentStatus::Idle),
        ]);
        let t0 = Instant::now();
        push_frame(&mut app, frame(0.05, vec![0.0; 16]));
        let quiet = glyph_positions(&render_current_for(&app, true, t0));
        push_frame(&mut app, frame(0.90, vec![0.8; 16]));
        let loud_low = glyph_positions(&render_current_for(&app, true, t0));
        assert!(!quiet.is_empty());
        assert_eq!(quiet, loud_low, "low power fixes light positions");
        assert_eq!(
            count_trail_cells(&render_current_for(&app, true, t0)),
            0,
            "low power draws no trails"
        );

        // The same loud frame in normal power does move the current.
        let loud_normal = glyph_positions(&render_current_for(&app, false, t0));
        assert_ne!(loud_low, loud_normal);
    }

    // --- state, selection, and privacy -------------------------------------

    #[test]
    fn state_colors_come_from_the_theme() {
        let mut app = current_app(vec![
            snap("ws", "p1", Some("w"), AgentStatus::Working),
            snap("ws", "p2", Some("b"), AgentStatus::Blocked),
            snap("ws", "p3", Some("i"), AgentStatus::Idle),
            snap("ws", "p4", Some("d"), AgentStatus::Done),
        ]);
        push_frame(&mut app, frame(0.5, vec![0.4; 16]));
        let theme = Theme::for_name(ThemeName::Minimal);
        let buf = render_current_for(&app, false, Instant::now());
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
    fn selected_explicit_name_is_the_only_rendered_agent_detail() {
        let mut app = app_with_named_and_unnamed_agents();
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let text = buffer_text(&render_current_for(&app, false, Instant::now()));
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
        let app = current_app(vec![snap(
            "alpha",
            "p1",
            Some("research"),
            AgentStatus::Working,
        )]);
        let text = buffer_text(&render_current_for(&app, false, Instant::now()));
        assert!(
            !text.contains("research"),
            "no label before selection: {text}"
        );
    }

    #[test]
    fn selecting_an_unnamed_agent_shows_no_label_at_all() {
        let mut app = current_app(vec![snap("alpha", "p1", None, AgentStatus::Working)]);
        app.apply(Action::SelectNextAgent);
        assert!(app.selected_agent().is_some());
        let text = buffer_text(&render_current_for(&app, false, Instant::now()));
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
    fn stale_freezes_the_last_live_field_dimmed_and_time_invariant() {
        let mut app = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.4, vec![0.4; 16]));
        push_frame(&mut app, frame(0.9, vec![0.8; 16]));
        let live = render_current_for(&app, false, Instant::now());
        let live_field = field_cells(&live);
        assert!(
            count_trail_cells(&live) > 0,
            "sanity: the final live frame has trails to freeze"
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let stale_buf = render_current_for(&app, false, Instant::now());
        assert_eq!(
            field_cells(&stale_buf),
            live_field,
            "stale freezes the exact flow, trail, and light geometry of the last live frame"
        );
        assert_eq!(
            count_trail_cells(&stale_buf),
            count_trail_cells(&live),
            "stale retains the last live trails"
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

        // Later live audio frames and elapsed time must not thaw the field.
        push_frame(&mut app, frame(0.1, vec![0.05; 16]));
        push_frame(&mut app, frame(0.7, vec![0.6; 16]));
        let later = render_current_for(&app, false, Instant::now() + Duration::from_secs(9));
        assert_eq!(
            later, stale_buf,
            "stale output is invariant across later audio frames and time"
        );
    }

    #[test]
    fn unavailable_hides_lights_behind_calm_copy() {
        let mut app = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        let buf = render_current_for(&app, false, Instant::now());
        assert!(glyph_positions(&buf).is_empty(), "no lights render");
        assert!(buffer_text(&buf).contains("agents · unavailable · retrying"));
    }

    #[test]
    fn canvas_is_a_noop_while_closed() {
        let mut app = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        app.apply(Action::CloseAgentOverlay);
        let buf = render_current_for(&app, false, Instant::now());
        assert_eq!(buf, Buffer::empty(CANVAS), "closed canvas draws nothing");
    }

    // --- hit testing --------------------------------------------------------

    #[test]
    fn clicking_a_light_cell_selects_that_agent() {
        let mut app = current_app(vec![
            snap("alpha", "p1", Some("research"), AgentStatus::Working),
            snap("beta", "p1", Some("review"), AgentStatus::Idle),
        ]);
        let layout = current_layout(
            app.active_agents(),
            app.viz(),
            &[],
            flow_area(CANVAS),
            false,
        );
        let review_index = app
            .active_agents()
            .iter()
            .position(|view| view.name.as_deref() == Some("review"))
            .unwrap();
        let light = layout
            .lights
            .iter()
            .find(|light| light.index == review_index)
            .unwrap();
        let (x, y) = light.cell;

        let action = hit_test(CANVAS, x, y, &app).expect("a light click selects");
        app.apply(action);
        assert_eq!(
            app.selected_agent().unwrap().name.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn clicks_resolve_only_light_cells_never_flow_or_trail_cells() {
        let mut app = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        push_frame(&mut app, frame(0.9, vec![0.8; 16]));
        let flow = flow_area(CANVAS);
        let layout = current_layout(app.active_agents(), app.viz(), &[], flow, false);
        let light = &layout.lights[0];
        let light_columns: Vec<u16> = (light.cell.0.saturating_sub(light.radius.max(1))
            ..=light.cell.0 + light.radius.max(1))
            .collect();
        let baseline = flow_y(flow, 0.0, 0.0);
        for cell in &layout.flow_cells {
            if light_columns.contains(&cell.x) {
                continue;
            }
            if cell.y == baseline && cell.x == light.anchor_x {
                continue;
            }
            assert!(
                hit_test(CANVAS, cell.x, cell.y, &app).is_none(),
                "a bare flow cell at ({}, {}) must resolve nothing",
                cell.x,
                cell.y
            );
        }
    }

    #[test]
    fn clicks_resolve_nothing_when_missed_stale_or_closed() {
        let mut app = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
        assert!(hit_test(CANVAS, 0, 0, &app).is_none(), "corner miss");

        let layout = current_layout(
            app.active_agents(),
            app.viz(),
            &[],
            flow_area(CANVAS),
            false,
        );
        let (x, y) = layout.lights[0].cell;
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert!(
            hit_test(CANVAS, x, y, &app).is_none(),
            "stale ignores clicks"
        );

        let mut closed = current_app(vec![snap("ws", "p1", Some("one"), AgentStatus::Working)]);
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
        let app = current_app(vec![
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

        let mut unavailable = current_app(vec![snap("ws", "p1", None, AgentStatus::Working)]);
        unavailable.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert!(summary_line(&unavailable, &theme).is_none());
    }
}
