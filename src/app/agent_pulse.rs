//! Agent Pulse lifecycle, display, and interaction state.
//!
//! This is the app-internal substate behind the optional Herdr Agent Pulse
//! companion: the connection ladder, the live agent table, selection, the
//! details modal, the inline rename input, recoverable focus/rename notices,
//! the stale visual freeze, and per-agent solar-orbit phase.
//!
//! It is deliberately narrow. [`AgentPulse`] owns every Agent Pulse field and
//! exposes only whole operations, so the core radio reducer in [`super`] can
//! delegate an [`Action`](super::Action) without reaching into Agent Pulse
//! internals. Nothing here can reach core radio state: an update sees only
//! what its caller passes in ([`DisplayMode`] for the interaction gate, an
//! [`Instant`], and — at the stale edge — a frozen visualizer snapshot), so
//! no Herdr update can move audio, search, settings, or station selection.
//!
//! Like the rest of the reducer this is pure and process-local: no socket, no
//! file IO, no persistence. Typed snapshots and failures arrive as actions
//! from the controller, and the private [`AgentId`] never leaves this module
//! except back to the Herdr adapter through the controller.

use crate::herdr::{
    self, AgentDetails, AgentId, AgentSnapshot, AgentStatus, FocusResult, RenameResult,
};
use crate::model::VizFrame;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::DisplayMode;

/// How long a recoverable pane-focus result remains visible in the stage.
const AGENT_FOCUS_NOTICE_FOR: Duration = Duration::from_secs(4);

/// Connection state of the optional Herdr Agent Pulse integration.
///
/// `Hidden` is the standalone/ineligible default: no Agent Pulse UI exists
/// and every Agent Pulse action is a no-op, so pre-integration behavior is
/// exactly unchanged. The other states follow the design's recovery ladder:
/// `Connected` after a successful snapshot, `Stale` after the first failed
/// poll, and `Unavailable` once [`herdr::STALE_AFTER`] passes without a
/// success. A fresh snapshot always recovers to `Connected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentPulseConnection {
    Hidden,
    Connected,
    Stale,
    Unavailable,
}

/// One live agent as Agent Pulse displays it.
///
/// `observed_at` is when this app first saw the agent in its current status —
/// a locally derived estimate, not an assertion about the agent's true
/// process start time. The view carries only the approved modal details; the
/// private [`AgentId`] exists solely for identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentView {
    pub(crate) id: AgentId,
    pub(crate) details: AgentDetails,
    /// Transitional mirror for the legacy Side Tag renderer; Task 3 removes it.
    pub(crate) name: Option<String>,
    pub(crate) status: AgentStatus,
    pub(crate) observed_at: Instant,
}

impl AgentView {
    /// Sort rank per the design: working, blocked, idle, done, then unknown.
    fn status_rank(&self) -> u8 {
        match self.status {
            AgentStatus::Working => 0,
            AgentStatus::Blocked => 1,
            AgentStatus::Idle => 2,
            AgentStatus::Done => 3,
            AgentStatus::Unknown => 4,
        }
    }
}

/// Visibility of the temporary Agent Pulse overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentOverlay {
    Closed,
    Open,
}

/// Ephemeral details modal for the selected Agent Planets identity.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentDetailsOverlay {
    Closed,
    Open(AgentId),
}

/// A short, process-local feedback record for the explicit pane-focus action.
/// It holds no pane, workspace, or server-error text.
#[derive(Debug)]
struct AgentFocusNotice {
    result: FocusResult,
    shown_at: Instant,
}

/// Ephemeral inline Name input owned by the App reducer. The private identity
/// is held only long enough for the controller to return it to `herdr`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentRenameOverlay {
    Closed,
    Editing {
        id: AgentId,
        input: String,
        submitting: bool,
    },
}

/// Short, modal-local feedback from an explicit rename request. Like focus,
/// this retains no raw server text or private identifiers.
#[derive(Debug)]
struct AgentRenameNotice {
    result: RenameResult,
    shown_at: Instant,
}

/// The visualizer display frozen at the Connected→Stale edge: the
/// then-current frame plus the prior frames behind it (most recent first),
/// so the canvas can keep drawing the exact last live current and trails.
#[derive(Debug)]
struct StaleViz {
    frame: VizFrame,
    history: Vec<VizFrame>,
}

/// One agent's private solar-orbit phase source: the Working time banked by
/// completed Working stretches plus the start of the still-running stretch.
///
/// The reducer captures a stretch into `banked` when the agent stops Working
/// (or at the Connected→Stale freeze edge) and re-opens `working_since` when
/// Working returns, so a planet resumes its orbit from the captured angle.
/// Process-local presentation state — never persisted, never exposed beyond
/// the derived elapsed-seconds accessors.
#[derive(Debug, Default)]
struct AgentOrbit {
    /// Working time captured from completed Working stretches.
    banked: Duration,
    /// When the current Working stretch began; `None` while not Working.
    working_since: Option<Instant>,
}

/// All Agent Pulse state owned by [`App`](super::App): live agents only.
///
/// Process-local only: nothing here is persisted, no completed history is
/// kept, and the reducer never touches the Herdr socket — typed snapshots
/// and failures arrive as [`Action`](super::Action)s from the controller over
/// the existing event-loop boundary.
#[derive(Debug)]
pub(super) struct AgentPulse {
    connection: AgentPulseConnection,
    /// Live agents across the current socket's workspaces, in display
    /// (sorted) order.
    active: Vec<AgentView>,
    /// Identity of the selected active agent.
    selected: Option<AgentId>,
    overlay: AgentOverlay,
    details: AgentDetailsOverlay,
    rename: AgentRenameOverlay,
    focus_notice: Option<AgentFocusNotice>,
    rename_notice: Option<AgentRenameNotice>,
    /// When the last successful snapshot arrived.
    last_success: Option<Instant>,
    /// When the current failure streak began; cleared by any success.
    first_failure: Option<Instant>,
    /// Display snapshot captured when the connection dims to `Stale`;
    /// cleared by a fresh agent snapshot and by `Unavailable`.
    stale_viz: Option<StaleViz>,
    /// Per-agent solar-orbit phase sources, keyed by the private identity;
    /// entries live exactly as long as their agent stays in a snapshot.
    orbits: HashMap<AgentId, AgentOrbit>,
}

impl AgentPulse {
    /// The standalone default: hidden and inert.
    pub(super) fn hidden() -> Self {
        Self {
            connection: AgentPulseConnection::Hidden,
            active: Vec::new(),
            selected: None,
            overlay: AgentOverlay::Closed,
            details: AgentDetailsOverlay::Closed,
            rename: AgentRenameOverlay::Closed,
            focus_notice: None,
            rename_notice: None,
            last_success: None,
            first_failure: None,
            stale_viz: None,
            orbits: HashMap::new(),
        }
    }

    // --- lifecycle -------------------------------------------------------

    /// Fold a fresh `agent.list` snapshot into the live table: rebuild the
    /// sorted display order, carry orbit phase and first-observed times
    /// across, drop a selection whose agent left, and recover to `Connected`.
    pub(super) fn apply_snapshot(&mut self, agents: Vec<AgentSnapshot>, now: Instant) {
        let previous = std::mem::take(&mut self.active);
        let mut previous_orbits = std::mem::take(&mut self.orbits);
        let mut orbits = HashMap::new();
        let mut active: Vec<AgentView> = agents
            .into_iter()
            .map(|snapshot| {
                let carried = previous
                    .iter()
                    .find(|view| view.id == snapshot.id && view.status == snapshot.status);
                // Orbit phase: a Working→non-Working transition banks the
                // stretch (freezing the planet at its current angle); a
                // non-Working→Working transition re-opens a stretch so the
                // orbit resumes from the captured phase. Omitted agents are
                // simply never carried over.
                let mut orbit = previous_orbits.remove(&snapshot.id).unwrap_or_default();
                match (orbit.working_since, snapshot.status == AgentStatus::Working) {
                    (Some(since), false) => {
                        orbit.banked += now.saturating_duration_since(since);
                        orbit.working_since = None;
                    }
                    (None, true) => orbit.working_since = Some(now),
                    _ => {}
                }
                orbits.insert(snapshot.id.clone(), orbit);
                AgentView {
                    observed_at: carried.map_or(now, |view| view.observed_at),
                    id: snapshot.id,
                    name: snapshot.details.name.clone(),
                    details: snapshot.details,
                    status: snapshot.status,
                }
            })
            .collect();
        sort_active_agents(&mut active);
        self.active = active;
        self.orbits = orbits;
        self.clamp_selection();
        self.connection = AgentPulseConnection::Connected;
        self.last_success = Some(now);
        self.first_failure = None;
        self.stale_viz = None;
    }

    /// Record a failed poll: the first failure of a streak dims state to
    /// `Stale`, and [`herdr::STALE_AFTER`] without a success makes the
    /// integration `Unavailable`. Last-known agents are retained so the UI
    /// can dim them while stale.
    ///
    /// `freeze_viz` supplies the live visualizer display (current frame plus
    /// trail) and is called exactly at the Connected→Stale edge, so a caller
    /// never pays for the clone on later failures in the same streak.
    pub(super) fn mark_poll_failed(
        &mut self,
        now: Instant,
        freeze_viz: impl FnOnce() -> (VizFrame, Vec<VizFrame>),
    ) {
        // Capture the live display exactly once, at the Connected→Stale
        // edge, so rendering can freeze the last current and trails.
        if self.connection == AgentPulseConnection::Connected {
            let (frame, history) = freeze_viz();
            self.stale_viz = Some(StaleViz { frame, history });
            // Freeze every Working orbit at the same edge so the solar layout
            // holds its exact last live positions; a recovery snapshot
            // re-opens Working stretches from the frozen phase.
            for orbit in self.orbits.values_mut() {
                if let Some(since) = orbit.working_since.take() {
                    orbit.banked += now.saturating_duration_since(since);
                }
            }
        }
        if self.first_failure.is_none() {
            self.first_failure = Some(now);
        }
        self.connection = if self.response_overdue(now) {
            AgentPulseConnection::Unavailable
        } else {
            AgentPulseConnection::Stale
        };
        if self.connection == AgentPulseConnection::Unavailable {
            self.drop_frozen_overlays();
        }
    }

    /// Downgrade to `Unavailable` once [`herdr::STALE_AFTER`] has passed
    /// without a successful snapshot. Called on a timer by the controller so
    /// the threshold applies even when no further monitor event arrives; it
    /// never upgrades state and never reveals a hidden integration.
    pub(super) fn refresh_staleness(&mut self, now: Instant) {
        if self.connection == AgentPulseConnection::Hidden {
            return;
        }
        if self.response_overdue(now) {
            self.connection = AgentPulseConnection::Unavailable;
            self.drop_frozen_overlays();
        }
    }

    /// Whether the reference point (the last success, or else the start of
    /// the current failure streak) is at least [`herdr::STALE_AFTER`] old.
    fn response_overdue(&self, now: Instant) -> bool {
        let Some(reference) = self.last_success.or(self.first_failure) else {
            return false;
        };
        now.duration_since(reference) >= herdr::STALE_AFTER
    }

    /// Drop the frozen display and the modals that may no longer describe
    /// anything live, on the way to `Unavailable`.
    fn drop_frozen_overlays(&mut self) {
        self.stale_viz = None;
        self.details = AgentDetailsOverlay::Closed;
        self.rename = AgentRenameOverlay::Closed;
    }

    // --- interaction gates -----------------------------------------------

    /// Whether Agent Pulse actions may run at all: the integration must have
    /// shown evidence of life (not `Hidden`), and Signal View must not be
    /// active — Signal View keeps its restricted key contract and never
    /// shows or opens Agent Pulse.
    fn interactive(&self, display: DisplayMode) -> bool {
        self.connection != AgentPulseConnection::Hidden && display != DisplayMode::SignalView
    }

    /// Whether selection actions may run: the canvas must be open and the
    /// connection `Connected`, matching the mouse hit-test gate — stale and
    /// unavailable freeze the last composition, selection included, so no
    /// input may act on data that may no longer be current. Close/toggle
    /// stay on [`Self::interactive`].
    fn selection_interactive(&self, display: DisplayMode) -> bool {
        self.interactive(display)
            && self.overlay == AgentOverlay::Open
            && self.connection == AgentPulseConnection::Connected
    }

    // --- stage, details, and selection ------------------------------------

    pub(super) fn toggle_overlay(&mut self, display: DisplayMode) {
        if !self.interactive(display) {
            return;
        }
        self.overlay = match self.overlay {
            AgentOverlay::Closed => AgentOverlay::Open,
            AgentOverlay::Open => {
                self.details = AgentDetailsOverlay::Closed;
                self.rename = AgentRenameOverlay::Closed;
                AgentOverlay::Closed
            }
        };
    }

    pub(super) fn close_overlay(&mut self, display: DisplayMode) {
        if !self.interactive(display) {
            return;
        }
        self.overlay = AgentOverlay::Closed;
        self.details = AgentDetailsOverlay::Closed;
        self.rename = AgentRenameOverlay::Closed;
    }

    pub(super) fn open_details(&mut self, display: DisplayMode) {
        if !self.selection_interactive(display) {
            return;
        }
        if let Some(id) = self.selected.clone() {
            self.details = AgentDetailsOverlay::Open(id);
        }
    }

    pub(super) fn close_details(&mut self) {
        self.dismiss_modals();
    }

    /// Dismiss the ephemeral modals without touching the stage itself, for
    /// when another surface takes over the screen (entering Signal View).
    pub(super) fn dismiss_modals(&mut self) {
        self.details = AgentDetailsOverlay::Closed;
        self.rename = AgentRenameOverlay::Closed;
    }

    /// Move the overlay selection to the next sorted agent, wrapping from
    /// the last back to the first; with no selection it starts at the first
    /// sorted agent. An open details modal follows the new selection.
    pub(super) fn select_next(&mut self, display: DisplayMode) {
        if !self.selection_interactive(display) {
            return;
        }
        if self.active.is_empty() {
            self.selected = None;
            return;
        }
        let index = match self.selected_index() {
            Some(index) => (index + 1) % self.active.len(),
            None => 0,
        };
        self.selected = self.active.get(index).map(|view| view.id.clone());
        self.follow_selection_with_details();
    }

    /// Move the overlay selection to the previous sorted agent, wrapping from
    /// the first back to the last; with no selection it starts at the last
    /// sorted agent. An open details modal follows the new selection.
    pub(super) fn select_previous(&mut self, display: DisplayMode) {
        if !self.selection_interactive(display) {
            return;
        }
        if self.active.is_empty() {
            self.selected = None;
            return;
        }
        let last = self.active.len() - 1;
        let index = match self.selected_index() {
            Some(0) | None => last,
            Some(index) => index - 1,
        };
        self.selected = self.active.get(index).map(|view| view.id.clone());
        self.follow_selection_with_details();
    }

    /// Select an active agent by its identity; unknown agents change nothing.
    pub(super) fn select(&mut self, id: AgentId, display: DisplayMode) {
        if !self.selection_interactive(display) || self.is_details_open() {
            return;
        }
        if self.active.iter().any(|view| view.id == id) {
            self.selected = Some(id);
        }
    }

    /// Index of the selected agent in the sorted active list, when it is
    /// still an active agent.
    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.active.iter().position(|view| &view.id == selected)
    }

    /// Drop the selection when its agent left the active list.
    fn clamp_selection(&mut self) {
        if self.selected_index().is_none() {
            self.selected = None;
            self.details = AgentDetailsOverlay::Closed;
            self.rename = AgentRenameOverlay::Closed;
        }
    }

    /// Re-point an open details modal at the current selection so keyboard
    /// navigation cycles agents without a separate hidden selection; closes
    /// the modal if the selection is gone. A closed modal stays closed.
    fn follow_selection_with_details(&mut self) {
        if matches!(self.details, AgentDetailsOverlay::Open(_)) {
            self.details = match &self.selected {
                Some(id) => AgentDetailsOverlay::Open(id.clone()),
                None => AgentDetailsOverlay::Closed,
            };
        }
    }

    // --- inline rename ----------------------------------------------------

    pub(super) fn open_rename(&mut self, display: DisplayMode) {
        if !self.selection_interactive(display) || !self.is_details_open() {
            return;
        }
        let Some(agent) = self.selected_agent() else {
            return;
        };
        self.rename = AgentRenameOverlay::Editing {
            id: agent.id.clone(),
            input: agent.details.name.clone().unwrap_or_default(),
            submitting: false,
        };
        self.rename_notice = None;
    }

    pub(super) fn append_rename(&mut self, character: char) {
        if self.connection != AgentPulseConnection::Connected {
            return;
        }
        if let AgentRenameOverlay::Editing {
            input, submitting, ..
        } = &mut self.rename
        {
            if !*submitting {
                input.push(character);
                self.rename_notice = None;
            }
        }
    }

    pub(super) fn backspace_rename(&mut self) {
        if self.connection != AgentPulseConnection::Connected {
            return;
        }
        if let AgentRenameOverlay::Editing {
            input, submitting, ..
        } = &mut self.rename
        {
            if !*submitting {
                input.pop();
                self.rename_notice = None;
            }
        }
    }

    pub(super) fn submit_rename(&mut self) {
        if self.connection != AgentPulseConnection::Connected {
            return;
        }
        if let AgentRenameOverlay::Editing { submitting, .. } = &mut self.rename {
            if !*submitting {
                *submitting = true;
                self.rename_notice = None;
            }
        }
    }

    pub(super) fn close_rename(&mut self) {
        self.rename = AgentRenameOverlay::Closed;
        self.rename_notice = None;
    }

    pub(super) fn record_rename_result(&mut self, result: RenameResult, now: Instant) {
        if self.connection != AgentPulseConnection::Connected {
            if let AgentRenameOverlay::Editing { submitting, .. } = &mut self.rename {
                *submitting = false;
                self.rename_notice = Some(AgentRenameNotice {
                    result: RenameResult::Unavailable,
                    shown_at: now,
                });
            }
            return;
        }
        let AgentRenameOverlay::Editing {
            id,
            input,
            submitting: _,
        } = &self.rename
        else {
            return;
        };
        if result == RenameResult::Renamed {
            let name = (!input.trim().is_empty()).then(|| input.trim().to_owned());
            if let Some(agent) = self.active.iter_mut().find(|agent| agent.id == *id) {
                agent.name = name.clone();
                agent.details.name = name;
            }
            self.rename = AgentRenameOverlay::Closed;
            self.rename_notice = None;
        } else if let AgentRenameOverlay::Editing { submitting, .. } = &mut self.rename {
            *submitting = false;
            self.rename_notice = Some(AgentRenameNotice {
                result,
                shown_at: now,
            });
        }
    }

    // --- explicit pane focus ----------------------------------------------

    pub(super) fn record_focus_result(&mut self, result: FocusResult, now: Instant) {
        self.focus_notice = match result {
            FocusResult::Focused => None,
            _ => Some(AgentFocusNotice {
                result,
                shown_at: now,
            }),
        };
    }

    /// The opaque selected-pane target only while the open stage has a fresh
    /// snapshot. The controller passes it straight back to the Herdr adapter;
    /// UI and persistence never receive a text representation.
    pub(super) fn focus_target(&self, display: DisplayMode) -> Option<AgentId> {
        self.selection_interactive(display)
            .then(|| self.selected_agent().map(|agent| agent.id.clone()))
            .flatten()
    }

    /// Short modal-local feedback for an explicit pane-focus attempt.
    pub(super) fn focus_notice(&self, now: Instant) -> Option<&'static str> {
        let notice = self.focus_notice.as_ref()?;
        if now.saturating_duration_since(notice.shown_at) >= AGENT_FOCUS_NOTICE_FOR {
            return None;
        }
        match notice.result {
            FocusResult::Focused => None,
            FocusResult::Unsupported => Some("pane focus requires Herdr 0.7.0+"),
            FocusResult::Missing => Some("pane is no longer available"),
            FocusResult::Unavailable => Some("pane focus unavailable · retrying"),
            FocusResult::NoSelection => Some("select a live planet first"),
        }
    }

    // --- queries ----------------------------------------------------------

    pub(super) fn connection(&self) -> AgentPulseConnection {
        self.connection
    }

    /// Live agents across the current socket's workspaces, in display
    /// (sorted) order.
    pub(super) fn active(&self) -> &[AgentView] {
        &self.active
    }

    /// The selected active agent, if one is still active.
    pub(super) fn selected_agent(&self) -> Option<&AgentView> {
        let index = self.selected_index()?;
        self.active.get(index)
    }

    pub(super) fn is_overlay_open(&self) -> bool {
        self.overlay == AgentOverlay::Open
    }

    pub(super) fn is_details_open(&self) -> bool {
        matches!(self.details, AgentDetailsOverlay::Open(_))
    }

    pub(super) fn is_rename_open(&self) -> bool {
        matches!(self.rename, AgentRenameOverlay::Editing { .. })
    }

    /// Current inline Name input. The empty string is intentional: it maps to
    /// a JSON null clear request at the Herdr boundary.
    pub(super) fn rename_input(&self) -> Option<&str> {
        let AgentRenameOverlay::Editing { input, .. } = &self.rename else {
            return None;
        };
        Some(input)
    }

    /// Whether a submitted inline rename is awaiting its asynchronous typed
    /// outcome. The UI stays responsive but prevents duplicate submissions.
    pub(super) fn rename_is_submitting(&self) -> bool {
        matches!(
            self.rename,
            AgentRenameOverlay::Editing {
                submitting: true,
                ..
            }
        )
    }

    /// Opaque target plus normalized request value for the controller. This
    /// never exposes the private id to UI or persistence, and stale snapshots
    /// intentionally return `None` without clearing the user's input.
    pub(super) fn rename_request(&self) -> Option<(AgentId, Option<String>)> {
        if self.connection != AgentPulseConnection::Connected {
            return None;
        }
        let AgentRenameOverlay::Editing {
            id,
            input,
            submitting: false,
        } = &self.rename
        else {
            return None;
        };
        Some((
            id.clone(),
            (!input.trim().is_empty()).then(|| input.trim().to_owned()),
        ))
    }

    /// Short inline-name failure copy. It deliberately omits server text and
    /// private identifiers, and only remains while the input is still open.
    pub(super) fn rename_notice(&self, now: Instant) -> Option<&'static str> {
        if !self.is_rename_open() {
            return None;
        }
        let notice = self.rename_notice.as_ref()?;
        if now.saturating_duration_since(notice.shown_at) >= AGENT_FOCUS_NOTICE_FOR {
            return None;
        }
        match notice.result {
            RenameResult::Unsupported => Some("rename requires Herdr 0.7.0+"),
            RenameResult::Missing => Some("agent is no longer available"),
            RenameResult::Unavailable => Some("rename unavailable · retrying"),
            RenameResult::Renamed => None,
        }
    }

    /// Approved details for the selected identity while the table modal is open.
    /// Kept test-only because production presentation reads the complete live
    /// display-order table rather than a single selected record.
    #[cfg(test)]
    pub(super) fn selected_details(&self) -> Option<&AgentDetails> {
        let AgentDetailsOverlay::Open(id) = &self.details else {
            return None;
        };
        let selected = self.selected_agent()?;
        (&selected.id == id).then_some(&selected.details)
    }

    /// Total Working seconds behind an agent's solar-orbit phase at `now`:
    /// the time banked by completed Working stretches plus the live current
    /// stretch. A frozen (non-Working) agent ignores `now` entirely, so its
    /// planet holds the captured angle; an unknown identity is zero.
    pub(super) fn orbit_secs(&self, id: &AgentId, now: Instant) -> f32 {
        let Some(orbit) = self.orbits.get(id) else {
            return 0.0;
        };
        let live = orbit
            .working_since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        (orbit.banked + live).as_secs_f32()
    }

    /// Every known agent's effective orbit seconds at `now`: the whole solar
    /// layout in one value, for the low-power freeze capture.
    pub(super) fn orbit_secs_snapshot(&self, now: Instant) -> HashMap<AgentId, f32> {
        self.orbits
            .keys()
            .map(|id| (id.clone(), self.orbit_secs(id, now)))
            .collect()
    }

    /// The visualizer display captured when the connection dimmed to
    /// `Stale`: the frozen current frame plus the prior trail frames.
    /// `None` while connected, unavailable, or hidden.
    pub(super) fn stale_viz(&self) -> Option<(&VizFrame, &[VizFrame])> {
        let stale = self.stale_viz.as_ref()?;
        Some((&stale.frame, stale.history.as_slice()))
    }
}

/// Sort active agents by state (working, blocked, idle, done, then unknown), then
/// by the first available approved label, with the stable identity as the final
/// tiebreaker so equal entries keep a deterministic order across snapshots.
fn sort_active_agents(agents: &mut [AgentView]) {
    agents.sort_by(|a, b| {
        let a_label = a.details.name.as_ref().or(a.details.agent.as_ref());
        let b_label = b.details.name.as_ref().or(b.details.agent.as_ref());
        a.status_rank()
            .cmp(&b.status_rank())
            .then_with(|| match (a_label, b_label) {
                (Some(a_label), Some(b_label)) => a_label.cmp(b_label),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}
