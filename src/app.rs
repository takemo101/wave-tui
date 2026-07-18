//! App state, actions, reducers, focus, selection, and temporary failures.
//!
//! This module owns the application state machine. Per the project guidelines
//! (Tell, Don't Ask / Law of Demeter), UI rendering asks [`App`] for display
//! data and dispatches [`Action`]s; it must not reach into nested state and
//! mutate it directly. All state transitions live in [`App::apply`] and its
//! private reducer helpers.
//!
//! The reducer is deliberately free of side effects: it mutates in-memory state
//! only. It does not perform file IO (settings persistence), drive the audio
//! runtime, or run searches. The surrounding controller observes the resulting
//! state (e.g. a [`PlaybackState::Connecting`] with a current station) to issue
//! audio commands, persist settings, and kick off searches. This keeps the
//! reducer pure and testable without a terminal, audio device, or network.

use crate::audio::AudioEvent;
use crate::catalog::{
    station_matches_category, station_matches_section, Catalog, Category, Section,
    SessionStationHealth, Stations,
};
use crate::herdr::{self, AgentId, AgentSnapshot, AgentStatus};
use crate::model::{PlaybackState, Station, StationId, VisualizerMode, VizFrame, VolumePercent};
use crate::search::SearchResults;
use crate::settings::Settings;
use crate::theme::ThemeName;

use std::collections::VecDeque;
use std::time::Instant;

/// Step applied to the volume for a single `VolumeUp`/`VolumeDown` action.
const VOLUME_STEP: i32 = 5;

/// Number of visualizer bands held in the current frame when idle/stopped.
const VIZ_BANDS: usize = 16;

/// Number of previous visualizer frames kept for PeakDots trail rendering.
const VIZ_TRAIL_FRAMES: usize = 5;

/// Current visualizer frame plus the trailing frames.
const VIZ_HISTORY_FRAMES: usize = VIZ_TRAIL_FRAMES + 1;

/// A focusable region of the UI.
///
/// `Tab`/`Shift+Tab` move focus between panes in a stable, predictable order via
/// [`Action::FocusNext`]/[`Action::FocusPrevious`]. The order mirrors the wide
/// "Search Console" layout reading order (search strip, section shortcuts,
/// station list, now playing) but is layout-independent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    /// Search input strip.
    Search,
    /// Browse source rail: the flat source picker (All Stations, Favorites,
    /// sections, and categories).
    Sections,
    /// The visible station list (catalog or search results).
    Stations,
    /// Now Playing, transport, and visualizer.
    NowPlaying,
}

impl FocusPane {
    /// Every pane, in focus-cycling order.
    pub const ALL: [FocusPane; 4] = [
        FocusPane::Search,
        FocusPane::Sections,
        FocusPane::Stations,
        FocusPane::NowPlaying,
    ];

    /// The next pane in cycling order, wrapping back to the first.
    pub fn next(self) -> Self {
        match self {
            FocusPane::Search => FocusPane::Sections,
            FocusPane::Sections => FocusPane::Stations,
            FocusPane::Stations => FocusPane::NowPlaying,
            FocusPane::NowPlaying => FocusPane::Search,
        }
    }

    /// The previous pane in cycling order, wrapping back to the last.
    pub fn previous(self) -> Self {
        match self {
            FocusPane::Search => FocusPane::NowPlaying,
            FocusPane::Sections => FocusPane::Search,
            FocusPane::Stations => FocusPane::Sections,
            FocusPane::NowPlaying => FocusPane::Stations,
        }
    }
}

/// Top-level display surface selected by the app.
///
/// Signal View is a temporary, opt-in visual-player surface for the current
/// station. It is display-only state: it changes what is rendered, not focus,
/// source, selection, search, playback, or settings, and it is never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// Normal Search/Browse/Stations/Now Playing TUI.
    Normal,
    /// Opt-in visual-player surface for the current station.
    SignalView,
}

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
/// process start time. The view deliberately carries no pane id, cwd, or
/// agent type: the explicit `name` is the only displayable label, and the
/// private [`AgentId`] exists solely for identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentView {
    pub(crate) id: AgentId,
    /// Explicit Herdr agent name; the only label the UI may ever show.
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
pub(crate) enum AgentOverlay {
    Closed,
    Open,
}

/// The visualizer display frozen at the Connected→Stale edge: the
/// then-current frame plus the prior frames behind it (most recent first),
/// so the canvas can keep drawing the exact last live current and trails.
#[derive(Debug)]
struct StaleViz {
    frame: VizFrame,
    history: Vec<VizFrame>,
}

/// All Agent Pulse state owned by [`App`]: live agents only.
///
/// Process-local only: nothing here is persisted, no completed history is
/// kept, and the reducer never touches the Herdr socket — typed snapshots
/// and failures arrive as [`Action`]s from the controller over the existing
/// event-loop boundary.
#[derive(Debug)]
struct AgentPulse {
    connection: AgentPulseConnection,
    /// Live agents across the current socket's workspaces, in display
    /// (sorted) order.
    active: Vec<AgentView>,
    /// Identity of the selected active agent.
    selected: Option<AgentId>,
    overlay: AgentOverlay,
    /// When the last successful snapshot arrived.
    last_success: Option<Instant>,
    /// When the current failure streak began; cleared by any success.
    first_failure: Option<Instant>,
    /// Display snapshot captured when the connection dims to `Stale`;
    /// cleared by a fresh agent snapshot and by `Unavailable`.
    stale_viz: Option<StaleViz>,
}

impl AgentPulse {
    /// The standalone default: hidden and inert.
    fn hidden() -> Self {
        Self {
            connection: AgentPulseConnection::Hidden,
            active: Vec::new(),
            selected: None,
            overlay: AgentOverlay::Closed,
            last_success: None,
            first_failure: None,
            stale_viz: None,
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
        }
    }
}

/// Sort active agents by state (working, blocked, idle, done, unknown), then
/// by explicit name (named agents before unnamed ones), with the stable
/// identity as the final tiebreaker so equal entries keep a deterministic
/// order across snapshots.
fn sort_active_agents(agents: &mut [AgentView]) {
    agents.sort_by(|a, b| {
        a.status_rank()
            .cmp(&b.status_rank())
            .then_with(|| match (&a.name, &b.name) {
                (Some(a_name), Some(b_name)) => a_name.cmp(b_name),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Status of the online search, shown in the search strip.
///
/// Pure display state owned by [`App`]: the controller sets it as a search
/// progresses (`Loading` before a fetch, `Loaded` afterward, distinguishing a
/// cache hit from a fresh fetch, or `Offline`/`Error` on failure). This carries
/// no IO, debounce, or network concern itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchStatus {
    /// No search in progress; showing the catalog or last results.
    #[default]
    Idle,
    /// A search request is in flight.
    Loading,
    /// Results are loaded; `from_cache` is `true` for a cache hit.
    Loaded { from_cache: bool },
    /// The search could not run because the network is unreachable.
    Offline,
    /// The search failed for another reason; carries a short message.
    Error(String),
}

/// What the visible station list is currently showing.
///
/// The app tracks the active source explicitly instead of letting the visible
/// list be anonymous, so Browse (a flat source picker) and Search clearing can
/// reason about it. Per the design deck the Wide Browse pane offers `All
/// Stations`, `Favorites`, both sections, and every category as sources. The
/// catalog-derived sources (All, Section, Category) and `Favorites` (from
/// persisted settings) build their list in the reducer; `Search` results are
/// produced by the controller and arrive via [`Action::SearchResults`], so the
/// reducer only records that the search source is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSource {
    /// The full curated catalog, ranked.
    AllStations,
    /// One curated section (Music or Spoken/News), ranked.
    Section(Section),
    /// One curated category, ranked.
    Category(Category),
    /// The persisted favorites collection.
    Favorites,
    /// Online search results.
    Search,
}

impl ListSource {
    /// Whether this source is the online search source.
    fn is_search(self) -> bool {
        matches!(self, ListSource::Search)
    }

    /// Human-readable label for this source, drawn from catalog state for
    /// sections and categories so the Browse rail never duplicates ad hoc
    /// labels.
    pub fn title(self) -> &'static str {
        match self {
            ListSource::AllStations => "All Stations",
            ListSource::Favorites => "Favorites",
            ListSource::Section(section) => section.title(),
            ListSource::Category(category) => category.title(),
            ListSource::Search => "Search",
        }
    }

    /// The flat Browse source rail, in display order: All Stations, Favorites,
    /// then each section immediately followed by its categories.
    ///
    /// `Search` is never part of the rail; it is entered by typing a query, not
    /// picked from Browse.
    pub fn browse_rail() -> Vec<ListSource> {
        let mut rail = vec![ListSource::AllStations, ListSource::Favorites];
        for section in Section::ALL {
            rail.push(ListSource::Section(section));
            for &category in section.categories() {
                rail.push(ListSource::Category(category));
            }
        }
        rail
    }
}

/// An intent dispatched to the [`App`] reducer.
///
/// Actions map to the keyboard model in `docs/SPEC.md`. UI translates key events
/// and audio runtime events into actions; the reducer owns the resulting state
/// transition. The enum is the app's public mutation contract.
#[derive(Debug, Clone)]
pub enum Action {
    /// Move focus to the next pane (`Tab`).
    FocusNext,
    /// Move focus to the previous pane (`Shift+Tab`).
    FocusPrevious,
    /// Move focus directly to a specific pane (e.g. `/` focuses the search
    /// strip; `Esc` returns focus to the station list).
    SetFocus(FocusPane),
    /// Move selection down within the visible station list (`j`/`Down`).
    SelectNext,
    /// Move selection up within the visible station list (`k`/`Up`).
    SelectPrevious,
    /// Jump selection to the first visible station.
    SelectFirst,
    /// Jump selection to the last visible station.
    SelectLast,
    /// Play the currently selected station (`Enter`).
    PlaySelected,
    /// Stop/Play toggle for the current station (`Space`).
    TogglePlayback,
    /// Toggle favorite state of the selected station (`f`).
    ToggleFavorite,
    /// Cycle to the next theme (`t`).
    CycleTheme,
    /// Cycle to the next visualizer mode (`v`).
    CycleVisualizerMode,
    /// Increase volume by one step (`+`).
    VolumeUp,
    /// Decrease volume by one step (`-`).
    VolumeDown,
    /// Set volume to an explicit value (e.g. CLI/persisted restore).
    SetVolume(VolumePercent),
    /// Replace the visible list with the full curated catalog, ranked.
    ShowCatalog,
    /// Replace the visible list with one curated section, ranked.
    ShowSection(Section),
    /// Replace the visible list with one curated category, ranked.
    ShowCategory(Category),
    /// Replace the visible list with online search results.
    SearchResults(SearchResults),
    /// Clear the search and restore the previous non-search source.
    ClearSearch,
    /// Move the Browse source-picker selection to an explicit index.
    SetBrowseSelection(usize),
    /// Move the Browse source-picker selection down one row (`j`/`Down` while the
    /// Browse rail is focused).
    BrowseSelectNext,
    /// Move the Browse source-picker selection up one row (`k`/`Up` while the
    /// Browse rail is focused).
    BrowseSelectPrevious,
    /// Jump the Browse source-picker selection to the first rail row.
    BrowseSelectFirst,
    /// Jump the Browse source-picker selection to the last rail row.
    BrowseSelectLast,
    /// Apply the currently selected Browse source and hand focus to Stations
    /// (`Enter` while the Browse rail is focused).
    ApplyBrowseSelection,
    /// Replace the live search query text shown in the search strip.
    SetSearchQuery(String),
    /// Update the search status shown in the search strip.
    SetSearchStatus(SearchStatus),
    /// Set the offline flag (network/Radio Browser reachability).
    SetOffline(bool),
    /// Apply an audio runtime event to playback state.
    Audio(AudioEvent),
    /// Toggle the temporary Signal View display mode (`z`).
    ToggleSignalView,
    /// Return from Signal View to the normal TUI (`Esc`/`z` while in Signal View).
    LeaveSignalView,
    /// Toggle favorite state of the current station shown in Signal View (`f`).
    ToggleCurrentFavorite,
    /// Apply a fresh Herdr `agent.list` snapshot covering every workspace
    /// served by the current control socket.
    AgentSnapshot {
        agents: Vec<AgentSnapshot>,
        now: Instant,
    },
    /// Record a failed Herdr poll (socket error, timeout, malformed reply).
    AgentPollFailed { now: Instant },
    /// Re-evaluate the stale/unavailable threshold without a monitor event.
    AgentTick { now: Instant },
    /// Toggle the Agent Pulse overlay (`a`); a no-op while the integration is
    /// hidden or Signal View is active.
    ToggleAgentOverlay,
    /// Close the Agent Pulse overlay (`Esc` while it is open).
    CloseAgentOverlay,
    /// Move the overlay selection down the sorted active-agent list.
    SelectNextAgent,
    /// Move the overlay selection up the sorted active-agent list.
    SelectPreviousAgent,
    /// Select an active agent by its stable identity (mouse/particle
    /// selection).
    SelectAgent(AgentId),
}

/// The application state.
///
/// Owns focus, the visible station list and selection, playback state, the
/// current visualizer frame, persisted [`Settings`] intent, session-only
/// station health, and the offline flag. Construction is always valid: the
/// selection never points outside the visible stations.
#[derive(Debug)]
pub struct App {
    catalog: Catalog,
    settings: Settings,
    health: SessionStationHealth,
    search_population: Option<Stations>,
    visible: Stations,
    selected: usize,
    source: ListSource,
    previous_source: ListSource,
    browse_selected: usize,
    focus: FocusPane,
    playback: PlaybackState,
    current: Option<Station>,
    viz: VizFrame,
    viz_history: VecDeque<VizFrame>,
    offline: bool,
    search_query: String,
    search_status: SearchStatus,
    now_playing_title: Option<String>,
    display_mode: DisplayMode,
    agent_pulse: AgentPulse,
}

impl App {
    /// Build the app from restored settings and the curated catalog.
    ///
    /// The visible list starts as the full curated catalog, ranked by playback
    /// likelihood. The current station defaults to the persisted previous
    /// station so `Space` can resume it without re-selecting, but playback
    /// starts `Stopped`: the controller decides whether to auto-play.
    pub fn new(settings: Settings, catalog: Catalog) -> Self {
        let visible = catalog.stations().ranked();
        let current = settings.previous_station.clone();
        let viz = VizFrame::silent(VIZ_BANDS);
        let viz_history = VecDeque::from([viz.clone()]);
        Self {
            catalog,
            settings,
            health: SessionStationHealth::new(),
            search_population: None,
            visible,
            selected: 0,
            source: ListSource::AllStations,
            previous_source: ListSource::AllStations,
            browse_selected: 0,
            focus: FocusPane::Stations,
            playback: PlaybackState::Stopped,
            current,
            viz,
            viz_history,
            offline: false,
            search_query: String::new(),
            search_status: SearchStatus::Idle,
            now_playing_title: None,
            display_mode: DisplayMode::Normal,
            agent_pulse: AgentPulse::hidden(),
        }
    }

    /// Apply an action, mutating state in place.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::FocusNext => self.focus = self.focus.next(),
            Action::FocusPrevious => self.focus = self.focus.previous(),
            Action::SetFocus(pane) => self.focus = pane,
            Action::SelectNext => self.select_next(),
            Action::SelectPrevious => self.select_previous(),
            Action::SelectFirst => self.selected = 0,
            Action::SelectLast => self.selected = self.visible.len().saturating_sub(1),
            Action::PlaySelected => self.play_selected(),
            Action::TogglePlayback => self.toggle_playback(),
            Action::ToggleFavorite => self.toggle_favorite(),
            Action::CycleTheme => self.settings.theme = self.settings.theme.next(),
            Action::CycleVisualizerMode => {
                self.settings.visualizer = self.settings.visualizer.next()
            }
            Action::VolumeUp => self.change_volume(VOLUME_STEP),
            Action::VolumeDown => self.change_volume(-VOLUME_STEP),
            Action::SetVolume(volume) => self.settings.volume = volume,
            Action::ShowCatalog => self.show_source(ListSource::AllStations),
            Action::ShowSection(section) => self.show_source(ListSource::Section(section)),
            Action::ShowCategory(category) => self.show_source(ListSource::Category(category)),
            Action::ClearSearch => self.clear_search(),
            Action::SetBrowseSelection(index) => self.browse_selected = index,
            Action::BrowseSelectNext => self.browse_select_next(),
            Action::BrowseSelectPrevious => {
                self.browse_selected = self.browse_selected.saturating_sub(1)
            }
            Action::BrowseSelectFirst => self.browse_selected = 0,
            Action::BrowseSelectLast => self.browse_selected = Self::browse_last_index(),
            Action::ApplyBrowseSelection => self.apply_browse_selection(),
            Action::SearchResults(results) => self.apply_search_results(results),
            Action::SetSearchQuery(query) => self.search_query = query,
            Action::SetSearchStatus(status) => self.search_status = status,
            Action::SetOffline(offline) => self.offline = offline,
            Action::Audio(event) => self.apply_audio(event),
            Action::ToggleSignalView => self.toggle_signal_view(),
            Action::LeaveSignalView => self.display_mode = DisplayMode::Normal,
            Action::ToggleCurrentFavorite => self.toggle_current_favorite(),
            Action::AgentSnapshot { agents, now } => self.apply_agent_snapshot(agents, now),
            Action::AgentPollFailed { now } => self.mark_agent_poll_failed(now),
            Action::AgentTick { now } => self.refresh_agent_staleness(now),
            Action::ToggleAgentOverlay => self.toggle_agent_overlay(),
            Action::CloseAgentOverlay => self.close_agent_overlay(),
            Action::SelectNextAgent => self.select_next_agent(),
            Action::SelectPreviousAgent => self.select_previous_agent(),
            Action::SelectAgent(id) => self.select_agent(id),
        }
    }

    // --- selection -------------------------------------------------------

    /// Move selection down one row, never past the last visible station.
    fn select_next(&mut self) {
        if self.visible.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1).min(self.visible.len() - 1);
        }
    }

    /// Move selection up one row, never below the first station.
    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Restore the bounds invariant after the visible list changes.
    fn clamp_selection(&mut self) {
        if self.visible.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.visible.len() {
            self.selected = self.visible.len() - 1;
        }
    }

    /// Replace the visible list and reset selection to a safe position.
    fn replace_visible(&mut self, stations: Stations) {
        self.visible = stations;
        self.selected = 0;
        self.clamp_selection();
    }

    /// Replace the visible list but keep the current selection index, clamped.
    ///
    /// Used when the active list shrinks in place (e.g. removing the selected
    /// favorite): keeping the index lands selection on the next valid row, falls
    /// back to the previous row when the last row was removed, and resolves to
    /// the empty-state position when nothing remains.
    fn refresh_visible_keeping_selection(&mut self, stations: Stations) {
        self.visible = stations;
        self.clamp_selection();
    }

    // --- list source -----------------------------------------------------

    /// Apply a source: record it as active and rebuild the visible list from the
    /// current population, resetting/clamping selection safely.
    ///
    /// When a successful search population exists, `AllStations`, `Section`, and
    /// `Category` act as filters over that population. Without one, they keep the
    /// curated catalog fallback. `Favorites` is always rebuilt from persisted
    /// [`Settings::favorites`] and never scoped to search results.
    fn show_source(&mut self, source: ListSource) {
        self.source = source;
        let stations = self.source_stations(source);
        self.replace_visible(stations);
    }

    /// Build the station list for a source from either the search population or
    /// curated fallback, depending on what the source represents.
    fn source_stations(&self, source: ListSource) -> Stations {
        match source {
            ListSource::AllStations | ListSource::Search => self
                .search_population
                .clone()
                .unwrap_or_else(|| self.catalog.stations().ranked()),
            ListSource::Section(section) => self.section_source_stations(section),
            ListSource::Category(category) => self.category_source_stations(category),
            ListSource::Favorites => self.favorite_stations(),
        }
    }

    /// Build a section source from the current search population when present,
    /// otherwise from the curated catalog.
    fn section_source_stations(&self, section: Section) -> Stations {
        if let Some(population) = &self.search_population {
            population
                .iter()
                .filter(|station| station_matches_section(station, section))
                .cloned()
                .collect()
        } else {
            self.catalog.section_stations(section)
        }
    }

    /// Build a category source from the current search population when present,
    /// otherwise from the curated catalog.
    fn category_source_stations(&self, category: Category) -> Stations {
        if let Some(population) = &self.search_population {
            population
                .iter()
                .filter(|station| station_matches_category(station, category))
                .cloned()
                .collect()
        } else {
            self.catalog.category_stations(category)
        }
    }

    /// The persisted favorites as a station list, in saved (insertion) order.
    ///
    /// Favorites are user-curated, so they are presented in saved order rather
    /// than re-ranked by playback likelihood like the catalog sources.
    fn favorite_stations(&self) -> Stations {
        self.settings.favorites.iter().cloned().collect()
    }

    /// Clear the successful search population and rebuild the active Browse
    /// source from curated data while preserving the selected Browse source.
    ///
    /// If an older Search source is active, fall back to the remembered previous
    /// non-search source. Focusing the search strip with `/` does not enter the
    /// Search source, so clearing before any results land keeps the current
    /// source untouched.
    fn clear_search(&mut self) {
        self.search_population = None;
        let source = if self.source.is_search() {
            self.previous_source
        } else {
            self.source
        };
        self.show_source(source);
    }

    // --- browse rail ------------------------------------------------------

    /// The last selectable index in the Browse source rail.
    fn browse_last_index() -> usize {
        ListSource::browse_rail().len().saturating_sub(1)
    }

    /// Move the Browse selection down one row, never past the last rail source.
    fn browse_select_next(&mut self) {
        self.browse_selected = (self.browse_selected + 1).min(Self::browse_last_index());
    }

    /// Apply the Browse-selected source and hand focus to Stations.
    ///
    /// The selection index is clamped against the rail so a stale/oversized
    /// cursor still resolves to a real source. Applying routes through
    /// [`Self::show_source`], so the visible list, active source, and selection
    /// bounds stay consistent (including `Favorites`, built from persisted
    /// settings).
    fn apply_browse_selection(&mut self) {
        let rail = ListSource::browse_rail();
        let index = self.browse_selected.min(rail.len().saturating_sub(1));
        if let Some(&source) = rail.get(index) {
            self.show_source(source);
        }
        self.focus = FocusPane::Stations;
    }

    /// The first non-failed station index at or after `start`, wrapping once.
    ///
    /// Returns `None` only when every visible station is marked failed (or the
    /// list is empty), so selection stays on a viable candidate when one exists.
    fn next_viable_index(&self, start: usize) -> Option<usize> {
        let slice = self.visible.as_slice();
        let n = slice.len();
        if n == 0 {
            return None;
        }
        (0..n)
            .map(|offset| (start + offset) % n)
            .find(|&i| !self.health.is_failed(&slice[i].id))
    }

    // --- favorites / theme / volume -------------------------------------

    /// Toggle the selected station's favorite state through settings.
    ///
    /// Adding and removing both flow through [`crate::settings::Favorites`], so
    /// deduplication (same id or URL) is enforced by the collection rather than
    /// here. This mutates persisted intent only; writing the file is the
    /// controller's job.
    ///
    /// When the Favorites source is active the visible list is rebuilt from the
    /// updated collection so a removal disappears immediately, keeping selection
    /// in place (clamped) rather than resetting to the top.
    fn toggle_favorite(&mut self) {
        let Some(station) = self.selected_station().cloned() else {
            return;
        };
        if self.settings.favorites.contains(&station) {
            self.settings.favorites.remove(&station);
        } else {
            self.settings.favorites.add(station);
        }
        if self.source == ListSource::Favorites {
            let stations = self.favorite_stations();
            self.refresh_visible_keeping_selection(stations);
        }
    }

    /// Toggle the current station's favorite state through settings.
    ///
    /// Unlike [`Self::toggle_favorite`], this targets the app's current station
    /// (the one Signal View presents), not the hidden station-list selection. No
    /// current station means there is nothing to favorite, so it is a no-op.
    ///
    /// Adding and removing flow through [`crate::settings::Favorites`] so identity
    /// dedupe is enforced there. When the Favorites source is active the visible
    /// list is rebuilt from the updated collection, keeping selection in place
    /// (clamped) like [`Self::toggle_favorite`].
    fn toggle_current_favorite(&mut self) {
        let Some(station) = self.current.clone() else {
            return;
        };
        if self.settings.favorites.contains(&station) {
            self.settings.favorites.remove(&station);
        } else {
            self.settings.favorites.add(station);
        }
        if self.source == ListSource::Favorites {
            let stations = self.favorite_stations();
            self.refresh_visible_keeping_selection(stations);
        }
    }

    /// Flip the top-level display mode between Normal and Signal View.
    ///
    /// This is display-only: it touches no focus, source, selection, search, or
    /// playback state, so background activity continues unchanged underneath.
    fn toggle_signal_view(&mut self) {
        self.display_mode = match self.display_mode {
            DisplayMode::Normal => DisplayMode::SignalView,
            DisplayMode::SignalView => DisplayMode::Normal,
        };
    }

    /// Shift volume by `delta`, clamping into the valid `0..=100` range.
    fn change_volume(&mut self, delta: i32) {
        let next = self.settings.volume.get() as i32 + delta;
        self.settings.volume = VolumePercent::clamped(next);
    }

    // --- playback --------------------------------------------------------

    /// Promote the selected station to the current station and begin connecting.
    fn play_selected(&mut self) {
        if let Some(station) = self.selected_station().cloned() {
            // Switching to a different station drops the old ICY title so a stale
            // one never lingers; resuming the same station keeps it.
            if self.current.as_ref().map(|c| &c.id) != Some(&station.id) {
                self.now_playing_title = None;
            }
            self.current = Some(station);
            self.playback = PlaybackState::Connecting;
        }
    }

    /// `Space` semantics: stop while active, reconnect a stopped/failed current.
    fn toggle_playback(&mut self) {
        match self.playback {
            PlaybackState::Playing | PlaybackState::Connecting => {
                self.playback = PlaybackState::Stopped;
            }
            PlaybackState::Stopped | PlaybackState::Failed(_) => {
                if self.current.is_some() {
                    self.playback = PlaybackState::Connecting;
                }
            }
        }
    }

    /// Fold an audio runtime event into playback/visualizer/health state.
    fn apply_audio(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Connecting { .. } => {
                self.playback = PlaybackState::Connecting;
            }
            AudioEvent::Playing { station } => self.on_playing(station),
            AudioEvent::Stopped => self.playback = PlaybackState::Stopped,
            AudioEvent::Failed { station, message } => self.on_failed(station, message),
            AudioEvent::VolumeChanged(volume) => self.settings.volume = volume,
            AudioEvent::Viz(frame) => self.set_viz_frame(frame),
            AudioEvent::IcyTitle { station, title } => self.on_icy_title(station, title),
        }
    }

    /// Store the latest visualizer frame and retain the short history used by
    /// PeakDots to render real, audio-frame-driven trails.
    fn set_viz_frame(&mut self, frame: VizFrame) {
        self.viz = frame.clone();
        self.viz_history.push_front(frame);
        self.viz_history.truncate(VIZ_HISTORY_FRAMES);
    }

    /// Apply an ICY title, but only when it belongs to the current station.
    ///
    /// Title events race with station switches: an event emitted for a station
    /// the user has since left is stale and must be ignored so the displayed
    /// title always matches what is actually playing.
    fn on_icy_title(&mut self, station: StationId, title: String) {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.id == station)
        {
            self.now_playing_title = Some(title);
        }
    }

    /// Playback started: mark playing, persist it as the previous station, and
    /// clear any session-failed mark left from an earlier failed attempt.
    fn on_playing(&mut self, station: StationId) {
        self.playback = PlaybackState::Playing;
        self.health.recover(&station);
        if let Some(current) = &self.current {
            if current.id == station {
                self.settings.previous_station = Some(current.clone());
            }
        }
    }

    /// Playback failed: mark the station failed for the session, reflect the
    /// failure when it was the current station, and move selection to the next
    /// viable candidate so the user can play something that still works.
    fn on_failed(&mut self, station: StationId, message: String) {
        self.health.mark_failed(&station);
        let is_current = self
            .current
            .as_ref()
            .is_some_and(|current| current.id == station);
        if is_current {
            self.playback = PlaybackState::Failed(message);
        }
        let start = self
            .visible
            .as_slice()
            .iter()
            .position(|s| s.id == station)
            .unwrap_or(self.selected);
        if let Some(index) = self.next_viable_index(start) {
            self.selected = index;
        }
    }

    /// Store a successful search population and rebuild the active Browse source
    /// from it, preserving the active filter instead of switching to an anonymous
    /// search-only source.
    ///
    /// Older `Search` state is still handled defensively by restoring the
    /// remembered previous non-search source before rebuilding. Receiving results
    /// means the network round-trip succeeded, so the offline flag is cleared.
    fn apply_search_results(&mut self, results: SearchResults) {
        let stations: Stations = results.into_vec().into_iter().collect();
        self.search_population = Some(stations);
        self.offline = false;
        let source = if self.source.is_search() {
            self.previous_source
        } else {
            self.source
        };
        self.show_source(source);
    }

    // --- agent pulse (Herdr integration) ---------------------------------

    /// Fold a successful `agent.list` snapshot into Agent Pulse state.
    ///
    /// Live-only reconciliation: the snapshot fully replaces the active
    /// view. Agents keep their `observed_at` while their identity and status
    /// are unchanged and reset it on a status change. A `done` agent stays
    /// in the active list (so the UI can dim it) until a later snapshot
    /// omits it; nothing is recorded once an agent disappears. A success
    /// always recovers the connection to `Connected`.
    fn apply_agent_snapshot(&mut self, agents: Vec<AgentSnapshot>, now: Instant) {
        let pulse = &mut self.agent_pulse;
        let previous = std::mem::take(&mut pulse.active);
        let mut active: Vec<AgentView> = agents
            .into_iter()
            .map(|snapshot| {
                let carried = previous
                    .iter()
                    .find(|view| view.id == snapshot.id && view.status == snapshot.status);
                AgentView {
                    observed_at: carried.map_or(now, |view| view.observed_at),
                    id: snapshot.id,
                    name: snapshot.name,
                    status: snapshot.status,
                }
            })
            .collect();
        sort_active_agents(&mut active);
        pulse.active = active;
        pulse.clamp_selection();
        pulse.connection = AgentPulseConnection::Connected;
        pulse.last_success = Some(now);
        pulse.first_failure = None;
        pulse.stale_viz = None;
    }

    /// Record a failed poll: the first failure of a streak dims state to
    /// `Stale`, and [`herdr::STALE_AFTER`] without a success makes the
    /// integration `Unavailable`. Last-known agents are retained so the UI
    /// can dim them while stale.
    fn mark_agent_poll_failed(&mut self, now: Instant) {
        // Capture the live display exactly once, at the Connected→Stale
        // edge, so rendering can freeze the last current and trails.
        if self.agent_pulse.connection == AgentPulseConnection::Connected {
            self.agent_pulse.stale_viz = Some(StaleViz {
                frame: self.viz.clone(),
                history: self.viz_history.iter().skip(1).cloned().collect(),
            });
        }
        let pulse = &mut self.agent_pulse;
        if pulse.first_failure.is_none() {
            pulse.first_failure = Some(now);
        }
        pulse.connection = if Self::agent_response_overdue(pulse, now) {
            AgentPulseConnection::Unavailable
        } else {
            AgentPulseConnection::Stale
        };
        if pulse.connection == AgentPulseConnection::Unavailable {
            pulse.stale_viz = None;
        }
    }

    /// Downgrade to `Unavailable` once [`herdr::STALE_AFTER`] has passed
    /// without a successful snapshot. Called on a timer by the controller so
    /// the threshold applies even when no further monitor event arrives; it
    /// never upgrades state and never reveals a hidden integration.
    fn refresh_agent_staleness(&mut self, now: Instant) {
        let pulse = &mut self.agent_pulse;
        if pulse.connection == AgentPulseConnection::Hidden {
            return;
        }
        if Self::agent_response_overdue(pulse, now) {
            pulse.connection = AgentPulseConnection::Unavailable;
            pulse.stale_viz = None;
        }
    }

    /// Whether the reference point (the last success, or else the start of
    /// the current failure streak) is at least [`herdr::STALE_AFTER`] old.
    fn agent_response_overdue(pulse: &AgentPulse, now: Instant) -> bool {
        let Some(reference) = pulse.last_success.or(pulse.first_failure) else {
            return false;
        };
        now.duration_since(reference) >= herdr::STALE_AFTER
    }

    /// Whether Agent Pulse actions may run at all: the integration must have
    /// shown evidence of life (not `Hidden`), and Signal View must not be
    /// active — Signal View keeps its restricted key contract and never
    /// shows or opens Agent Pulse.
    fn agent_pulse_interactive(&self) -> bool {
        self.agent_pulse.connection != AgentPulseConnection::Hidden
            && self.display_mode != DisplayMode::SignalView
    }

    /// Whether selection actions may run: the canvas must be open and the
    /// connection `Connected`, matching the mouse hit-test gate — stale and
    /// unavailable freeze the last composition, selection included, so no
    /// input may act on data that may no longer be current. Close/toggle
    /// stay on [`Self::agent_pulse_interactive`].
    fn agent_selection_interactive(&self) -> bool {
        self.agent_pulse_interactive()
            && self.agent_pulse.overlay == AgentOverlay::Open
            && self.agent_pulse.connection == AgentPulseConnection::Connected
    }

    fn toggle_agent_overlay(&mut self) {
        if !self.agent_pulse_interactive() {
            return;
        }
        self.agent_pulse.overlay = match self.agent_pulse.overlay {
            AgentOverlay::Closed => AgentOverlay::Open,
            AgentOverlay::Open => AgentOverlay::Closed,
        };
    }

    fn close_agent_overlay(&mut self) {
        if !self.agent_pulse_interactive() {
            return;
        }
        self.agent_pulse.overlay = AgentOverlay::Closed;
    }

    /// Move the overlay selection down one row, never past the last agent;
    /// with no selection it starts at the first sorted agent.
    fn select_next_agent(&mut self) {
        if !self.agent_selection_interactive() {
            return;
        }
        let pulse = &mut self.agent_pulse;
        let index = match pulse.selected_index() {
            Some(index) => (index + 1).min(pulse.active.len().saturating_sub(1)),
            None => 0,
        };
        pulse.selected = pulse.active.get(index).map(|view| view.id.clone());
    }

    /// Move the overlay selection up one row, never above the first agent;
    /// with no selection it starts at the last sorted agent.
    fn select_previous_agent(&mut self) {
        if !self.agent_selection_interactive() {
            return;
        }
        let pulse = &mut self.agent_pulse;
        let index = match pulse.selected_index() {
            Some(index) => index.saturating_sub(1),
            None => pulse.active.len().saturating_sub(1),
        };
        pulse.selected = pulse.active.get(index).map(|view| view.id.clone());
    }

    /// Select an active agent by its identity; unknown agents change nothing.
    fn select_agent(&mut self, id: AgentId) {
        if !self.agent_selection_interactive() {
            return;
        }
        let pulse = &mut self.agent_pulse;
        if pulse.active.iter().any(|view| view.id == id) {
            pulse.selected = Some(id);
        }
    }

    // --- queries (read-only, for UI/controller) -------------------------

    /// The currently focused pane.
    pub fn focus(&self) -> FocusPane {
        self.focus
    }

    /// The visible station list.
    pub fn visible(&self) -> &Stations {
        &self.visible
    }

    /// The selected index into [`Self::visible`].
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The active station-list source.
    pub fn active_source(&self) -> ListSource {
        self.source
    }

    /// The Browse source-picker selection index.
    pub fn browse_selected(&self) -> usize {
        self.browse_selected
    }

    /// The selected station, if any are visible.
    pub fn selected_station(&self) -> Option<&Station> {
        self.visible.as_slice().get(self.selected)
    }

    /// The current playback state.
    pub fn playback(&self) -> &PlaybackState {
        &self.playback
    }

    /// The current station (playing, connecting, or last/previous).
    pub fn current_station(&self) -> Option<&Station> {
        self.current.as_ref()
    }

    /// The live ICY/Shoutcast title for the current station, when one has been
    /// received. `None` falls the UI back to station-level metadata.
    pub fn now_playing_title(&self) -> Option<&str> {
        self.now_playing_title.as_deref()
    }

    /// The most recent visualizer frame.
    pub fn viz(&self) -> &VizFrame {
        &self.viz
    }

    /// The current visualizer frame followed by up to five previous frames.
    ///
    /// This short, non-persisted history is used only for visualizers that need a
    /// real audio-frame trail; the first item is always the same frame as
    /// [`Self::viz`].
    pub fn viz_history(&self) -> impl ExactSizeIterator<Item = &VizFrame> + DoubleEndedIterator {
        self.viz_history.iter()
    }

    /// Whether the app is in an offline/unreachable-network state.
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// The live search query text shown in the search strip.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// The current search status shown in the search strip.
    pub fn search_status(&self) -> &SearchStatus {
        &self.search_status
    }

    /// Whether the app has a successful search result population available as
    /// the Browse filtering source.
    pub fn has_search_population(&self) -> bool {
        self.search_population.is_some()
    }

    /// Label for the active Browse source when it is filtering search results.
    pub fn active_filter_label(&self) -> Option<&'static str> {
        if !self.has_search_population() {
            return None;
        }
        match self.source {
            ListSource::AllStations | ListSource::Section(_) | ListSource::Category(_) => {
                Some(self.source.title())
            }
            ListSource::Favorites | ListSource::Search => None,
        }
    }

    /// Specific empty-state copy for a zero-match Browse filter over search
    /// results. `AllStations` and `Favorites` keep their existing generic states.
    pub fn search_filter_empty_note(&self) -> Option<String> {
        if !self.has_search_population() || !self.visible.is_empty() {
            return None;
        }
        match self.source {
            ListSource::Section(section) => {
                Some(format!("No {} results in current search", section.title()))
            }
            ListSource::Category(category) => {
                Some(format!("No {} results in current search", category.title()))
            }
            ListSource::AllStations | ListSource::Favorites | ListSource::Search => None,
        }
    }

    /// The active theme name.
    pub fn theme(&self) -> ThemeName {
        self.settings.theme
    }

    /// The active visualizer mode.
    ///
    /// All supported visualizer modes have renderers; the reducer cycles through
    /// the persisted mode order used by the `v` key.
    pub fn visualizer_mode(&self) -> VisualizerMode {
        self.settings.visualizer
    }

    /// Read-only access to settings (volume, theme, favorites, previous station).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Whether a station is a current favorite (by id or URL identity).
    pub fn is_favorite(&self, station: &Station) -> bool {
        self.settings.favorites.contains(station)
    }

    /// Whether the current station is a favorite.
    ///
    /// Targets the current station (what Signal View shows), not the hidden
    /// station-list selection. `false` when there is no current station.
    pub fn current_station_is_favorite(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|station| self.settings.favorites.contains(station))
    }

    /// The active top-level display mode.
    pub fn display_mode(&self) -> DisplayMode {
        self.display_mode
    }

    /// Whether the full-screen Signal View surface is active.
    pub fn is_signal_view(&self) -> bool {
        self.display_mode == DisplayMode::SignalView
    }

    /// Whether a station is marked failed for this session.
    pub fn is_failed(&self, id: &StationId) -> bool {
        self.health.is_failed(id)
    }
}

/// Agent Pulse queries (read-only, for UI/controller).
///
/// These accessors are the only way UI and CLI observe Agent Pulse state;
/// mutation goes through [`App::apply`] like every other state transition.
impl App {
    /// The Agent Pulse connection state; `Hidden` for standalone launches.
    pub(crate) fn agent_pulse_connection(&self) -> AgentPulseConnection {
        self.agent_pulse.connection
    }

    /// Live agents across the current socket's workspaces, in display
    /// (sorted) order.
    pub(crate) fn active_agents(&self) -> &[AgentView] {
        &self.agent_pulse.active
    }

    /// The selected active agent, if one is still active.
    pub(crate) fn selected_agent(&self) -> Option<&AgentView> {
        let index = self.agent_pulse.selected_index()?;
        self.agent_pulse.active.get(index)
    }

    /// Whether the Agent Pulse overlay is open.
    pub(crate) fn is_agent_overlay_open(&self) -> bool {
        self.agent_pulse.overlay == AgentOverlay::Open
    }

    /// The visualizer display captured when the connection dimmed to
    /// `Stale`: the frozen current frame plus the prior trail frames.
    /// `None` while connected, unavailable, or hidden.
    pub(crate) fn stale_viz(&self) -> Option<(&VizFrame, &[VizFrame])> {
        let stale = self.agent_pulse.stale_viz.as_ref()?;
        Some((&stale.frame, stale.history.as_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{AgentId, AgentSnapshot, AgentStatus};
    use crate::model::{
        BitrateKbps, CodecKind, StationId, StationName, StationSource, StreamUrl, VisualizerMode,
        VolumePercent,
    };
    use crate::settings::Favorites;
    use crate::theme::ThemeName;
    use std::time::{Duration, Instant};

    fn station(id: &str, url: &str) -> Station {
        Station {
            id: StationId::new(id).unwrap(),
            name: StationName::new(id).unwrap(),
            url: StreamUrl::parse(url).unwrap(),
            homepage: None,
            country: None,
            language: None,
            tags: vec![],
            codec: CodecKind::Mp3,
            bitrate: Some(BitrateKbps::new(128).unwrap()),
            votes: Some(10),
            click_count: Some(10),
            source: StationSource::RadioBrowser,
        }
    }

    /// An app whose visible list is exactly `ids`, in order, with a known
    /// playback-equal score so ranking preserves order for predictable indices.
    fn app_with(ids: &[&str]) -> App {
        let stations: Vec<Station> = ids
            .iter()
            .map(|id| station(id, &format!("https://example.com/{id}.mp3")))
            .collect();
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations(
            stations,
        )));
        app
    }

    fn visible_ids(app: &App) -> Vec<String> {
        app.visible()
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect()
    }

    #[test]
    fn focus_cycles_predictably_forward_and_back() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        // Forward cycles through every pane and wraps.
        let start = app.focus();
        let mut seen = vec![start];
        for _ in 0..FocusPane::ALL.len() {
            app.apply(Action::FocusNext);
            seen.push(app.focus());
        }
        assert_eq!(seen.first(), seen.last(), "FocusNext wraps to the start");
        // Previous is the inverse of next.
        let before = app.focus();
        app.apply(Action::FocusNext);
        app.apply(Action::FocusPrevious);
        assert_eq!(app.focus(), before);
    }

    #[test]
    fn selection_never_leaves_visible_bounds() {
        let mut app = app_with(&["a", "b", "c"]);
        assert_eq!(app.selected_index(), 0);
        // Up at the top stays at the top.
        app.apply(Action::SelectPrevious);
        assert_eq!(app.selected_index(), 0);
        // Down moves and clamps at the last row.
        for _ in 0..10 {
            app.apply(Action::SelectNext);
        }
        assert_eq!(app.selected_index(), 2);
        assert_eq!(app.selected_station().unwrap().id.as_str(), "c");
        // First/last jumps respect bounds.
        app.apply(Action::SelectFirst);
        assert_eq!(app.selected_index(), 0);
        app.apply(Action::SelectLast);
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn selection_is_safe_on_empty_visible_list() {
        let mut app = app_with(&[]);
        assert_eq!(app.selected_index(), 0);
        assert!(app.selected_station().is_none());
        app.apply(Action::SelectNext);
        app.apply(Action::SelectLast);
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn toggle_favorite_adds_then_removes_and_dedupes() {
        let mut app = app_with(&["a", "b"]);
        let selected = app.selected_station().cloned().unwrap();
        assert!(!app.is_favorite(&selected));

        app.apply(Action::ToggleFavorite);
        assert!(app.is_favorite(&selected));
        assert_eq!(app.settings().favorites.len(), 1);

        // Toggling again removes it.
        app.apply(Action::ToggleFavorite);
        assert!(!app.is_favorite(&selected));
        assert_eq!(app.settings().favorites.len(), 0);
    }

    #[test]
    fn toggle_favorite_uses_url_identity_across_distinct_selections() {
        // Two stations with different ids but the same URL are the same favorite.
        let a = station("id-a", "https://shared.example/s.mp3");
        let b = station("id-b", "https://shared.example/s.mp3");
        let mut app = app_with(&[]);
        app.apply(Action::SearchResults(SearchResults::from_stations([a, b])));

        // Favorite index 0 (id-a): added.
        app.apply(Action::ToggleFavorite);
        assert_eq!(app.settings().favorites.len(), 1);

        // Toggling index 1 (id-b) sees it as the same favorite by URL identity,
        // so it is removed rather than duplicated. Identity dedupe flows through
        // the reducer instead of comparing only the station id.
        app.apply(Action::SelectNext);
        app.apply(Action::ToggleFavorite);
        assert_eq!(app.settings().favorites.len(), 0);
    }

    #[test]
    fn cycle_theme_advances_through_the_six_themes() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.theme(), ThemeName::Minimal);
        for expected in [
            ThemeName::Neon,
            ThemeName::Crt,
            ThemeName::Solarized,
            ThemeName::Midnight,
            ThemeName::Sakura,
            ThemeName::Minimal,
        ] {
            app.apply(Action::CycleTheme);
            assert_eq!(app.theme(), expected);
        }
    }

    #[test]
    fn visualizer_mode_defaults_to_spectrum_stack() {
        let app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);
    }

    #[test]
    fn cycle_visualizer_mode_advances_through_the_six_modes() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::SkylinePeaks);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::WaveScope);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::MirrorWave);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::AmbientPulse);
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);
    }

    #[test]
    fn cycling_visualizer_mode_updates_persisted_settings() {
        // The selected mode lives in settings so it persists like theme/volume.
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::CycleVisualizerMode);
        assert_eq!(app.settings().visualizer, VisualizerMode::PeakDots);
    }

    #[test]
    fn volume_steps_clamp_within_range() {
        let settings = Settings {
            volume: VolumePercent::new(98).unwrap(),
            ..Settings::default()
        };
        let mut app = App::new(settings, Catalog::curated());
        app.apply(Action::VolumeUp);
        assert_eq!(app.settings().volume.get(), 100, "clamps at the ceiling");

        app.apply(Action::SetVolume(VolumePercent::new(2).unwrap()));
        app.apply(Action::VolumeDown);
        assert_eq!(app.settings().volume.get(), 0, "clamps at the floor");
    }

    #[test]
    fn play_selected_sets_current_and_connecting() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::SelectNext);
        app.apply(Action::PlaySelected);
        assert_eq!(app.current_station().unwrap().id.as_str(), "b");
        assert_eq!(app.playback(), &PlaybackState::Connecting);
    }

    #[test]
    fn toggle_playback_stops_active_and_resumes_current() {
        let mut app = app_with(&["a"]);
        app.apply(Action::PlaySelected);
        app.apply(Action::Audio(AudioEvent::Playing {
            station: StationId::new("a").unwrap(),
        }));
        assert_eq!(app.playback(), &PlaybackState::Playing);

        // Space while playing stops.
        app.apply(Action::TogglePlayback);
        assert_eq!(app.playback(), &PlaybackState::Stopped);

        // Space while stopped with a current station reconnects.
        app.apply(Action::TogglePlayback);
        assert_eq!(app.playback(), &PlaybackState::Connecting);
    }

    #[test]
    fn toggle_playback_is_noop_without_a_current_station() {
        let mut app = app_with(&["a"]);
        // Nothing has been played yet and there is no previous station.
        assert!(app.current_station().is_none());
        app.apply(Action::TogglePlayback);
        assert_eq!(app.playback(), &PlaybackState::Stopped);
    }

    #[test]
    fn audio_playing_updates_previous_station() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::PlaySelected); // current = "a"
        assert!(app.settings().previous_station.is_none());

        app.apply(Action::Audio(AudioEvent::Playing {
            station: StationId::new("a").unwrap(),
        }));
        assert_eq!(app.playback(), &PlaybackState::Playing);
        assert_eq!(
            app.settings()
                .previous_station
                .as_ref()
                .unwrap()
                .id
                .as_str(),
            "a"
        );
    }

    #[test]
    fn audio_failed_marks_session_and_presents_next_viable() {
        let mut app = app_with(&["a", "b", "c"]);
        app.apply(Action::PlaySelected); // current/selected = "a" at index 0

        app.apply(Action::Audio(AudioEvent::Failed {
            station: StationId::new("a").unwrap(),
            message: "boom".to_string(),
        }));

        // Marked failed for the session and surfaced as a failure for current.
        assert!(app.is_failed(&StationId::new("a").unwrap()));
        assert_eq!(app.playback(), &PlaybackState::Failed("boom".to_string()));
        // Selection advanced off the failed station to the next viable one.
        assert_eq!(app.selected_station().unwrap().id.as_str(), "b");
        assert!(!app.is_failed(&StationId::new("b").unwrap()));
    }

    #[test]
    fn audio_failed_keeps_selection_viable_when_some_remain() {
        let mut app = app_with(&["a", "b", "c"]);
        // Fail b and c; selection must land on the only viable station, a.
        app.apply(Action::Audio(AudioEvent::Failed {
            station: StationId::new("b").unwrap(),
            message: "x".to_string(),
        }));
        app.apply(Action::Audio(AudioEvent::Failed {
            station: StationId::new("c").unwrap(),
            message: "x".to_string(),
        }));
        assert_eq!(app.selected_station().unwrap().id.as_str(), "a");
    }

    #[test]
    fn audio_playing_recovers_a_previously_failed_station() {
        let mut app = app_with(&["a", "b"]);
        let a = StationId::new("a").unwrap();
        app.apply(Action::Audio(AudioEvent::Failed {
            station: a.clone(),
            message: "transient".to_string(),
        }));
        assert!(app.is_failed(&a));

        // A later successful play of the same station clears the session mark.
        app.apply(Action::PlaySelected);
        app.apply(Action::SelectFirst);
        app.apply(Action::PlaySelected); // current = "a"
        app.apply(Action::Audio(AudioEvent::Playing { station: a.clone() }));
        assert!(!app.is_failed(&a));
    }

    #[test]
    fn icy_title_updates_now_playing_for_current_station() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::PlaySelected); // current = "a"
        assert!(app.now_playing_title().is_none());

        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: StationId::new("a").unwrap(),
            title: "Artist - Hit".to_string(),
        }));
        assert_eq!(app.now_playing_title(), Some("Artist - Hit"));
    }

    #[test]
    fn icy_title_from_a_non_current_station_is_ignored() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::PlaySelected); // current = "a"

        // A late event from a station the user already left must not show.
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: StationId::new("b").unwrap(),
            title: "Stale Title".to_string(),
        }));
        assert!(app.now_playing_title().is_none());
    }

    #[test]
    fn switching_station_clears_a_previous_icy_title() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::PlaySelected); // current = "a"
        app.apply(Action::Audio(AudioEvent::IcyTitle {
            station: StationId::new("a").unwrap(),
            title: "On A".to_string(),
        }));
        assert_eq!(app.now_playing_title(), Some("On A"));

        // Move to a different station: the stale title must not linger.
        app.apply(Action::SelectNext);
        app.apply(Action::PlaySelected); // current = "b"
        assert!(app.now_playing_title().is_none());
    }

    #[test]
    fn audio_viz_updates_current_frame() {
        let mut app = app_with(&["a"]);
        let frame = VizFrame::new([0.1, 0.9, 0.5], 0.7, [-0.5, 0.0, 0.5]);
        app.apply(Action::Audio(AudioEvent::Viz(frame.clone())));
        assert_eq!(app.viz(), &frame);
    }

    #[test]
    fn audio_viz_keeps_current_plus_five_trailing_frames() {
        let mut app = app_with(&["a"]);
        for index in 0..8 {
            app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
                [index as f32 / 10.0],
                0.0,
                [],
            ))));
        }

        let history: Vec<f32> = app.viz_history().map(|frame| frame.bands[0]).collect();

        assert_eq!(history.len(), 6, "current plus five trailing frames");
        assert_eq!(history, vec![0.7, 0.6, 0.5, 0.4, 0.3, 0.2]);
    }

    #[test]
    fn audio_volume_changed_updates_settings() {
        let mut app = app_with(&["a"]);
        app.apply(Action::Audio(AudioEvent::VolumeChanged(
            VolumePercent::new(33).unwrap(),
        )));
        assert_eq!(app.settings().volume.get(), 33);
    }

    #[test]
    fn search_results_replace_visible_and_reset_selection() {
        let mut app = app_with(&["a", "b", "c"]);
        app.apply(Action::SelectLast);
        assert_eq!(app.selected_index(), 2);

        // Smaller result set must not leave selection out of bounds.
        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        assert_eq!(visible_ids(&app), vec!["x"]);
        assert_eq!(app.selected_index(), 0);
        assert_eq!(app.selected_station().unwrap().id.as_str(), "x");
    }

    #[test]
    fn search_results_clear_offline_flag() {
        let mut app = app_with(&["a"]);
        app.apply(Action::SetOffline(true));
        assert!(app.is_offline());
        app.apply(Action::SearchResults(SearchResults::empty()));
        assert!(!app.is_offline());
    }

    #[test]
    fn search_query_and_status_update_via_actions() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        // Defaults: empty query, idle status.
        assert_eq!(app.search_query(), "");
        assert_eq!(app.search_status(), &SearchStatus::Idle);

        app.apply(Action::SetSearchQuery("lofi jazz".to_string()));
        assert_eq!(app.search_query(), "lofi jazz");

        app.apply(Action::SetSearchStatus(SearchStatus::Loading));
        assert_eq!(app.search_status(), &SearchStatus::Loading);

        app.apply(Action::SetSearchStatus(SearchStatus::Loaded {
            from_cache: true,
        }));
        assert_eq!(
            app.search_status(),
            &SearchStatus::Loaded { from_cache: true }
        );
    }

    #[test]
    fn offline_flag_toggles() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert!(!app.is_offline());
        app.apply(Action::SetOffline(true));
        assert!(app.is_offline());
        app.apply(Action::SetOffline(false));
        assert!(!app.is_offline());
    }

    #[test]
    fn set_focus_targets_a_specific_pane() {
        // `/` and Esc need to move focus to an exact pane, not just cycle.
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SetFocus(FocusPane::Search));
        assert_eq!(app.focus(), FocusPane::Search);
        app.apply(Action::SetFocus(FocusPane::NowPlaying));
        assert_eq!(app.focus(), FocusPane::NowPlaying);
    }

    #[test]
    fn show_section_replaces_visible_with_catalog_section() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::SpokenNews));
        assert!(!app.visible().is_empty());
        // Every visible station belongs to the requested section.
        let spoken: Vec<String> = app
            .catalog
            .section_stations(Section::SpokenNews)
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(visible_ids(&app), spoken);
        assert_eq!(app.selected_index(), 0);
    }

    // --- ListSource / source-aware reducer (MIK-018) --------------------

    #[test]
    fn active_source_defaults_to_all_stations() {
        let app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.active_source(), ListSource::AllStations);
        assert_eq!(app.browse_selected(), 0);
    }

    #[test]
    fn show_catalog_sets_all_stations_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::Music));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));

        app.apply(Action::ShowCatalog);
        assert_eq!(app.active_source(), ListSource::AllStations);
        let all: Vec<String> = app
            .catalog
            .stations()
            .ranked()
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(visible_ids(&app), all);
    }

    #[test]
    fn show_section_sets_section_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::SpokenNews));
        assert_eq!(
            app.active_source(),
            ListSource::Section(Section::SpokenNews)
        );
    }

    #[test]
    fn show_category_sets_category_source_and_visible() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowCategory(Category::Lofi));
        assert_eq!(app.active_source(), ListSource::Category(Category::Lofi));
        let lofi: Vec<String> = app
            .catalog
            .category_stations(Category::Lofi)
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(visible_ids(&app), lofi);
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn selection_is_clamped_safely_after_every_source_change() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        // Move selection to the end of the full catalog.
        app.apply(Action::ShowCatalog);
        app.apply(Action::SelectLast);
        assert!(app.selected_index() > 0);

        // A narrower source must not leave selection out of bounds.
        app.apply(Action::ShowCategory(Category::Lofi));
        assert_eq!(app.selected_index(), 0);
        assert!(app.selected_index() < app.visible().len().max(1));
        // The selected station is real (or the list is empty).
        if !app.visible().is_empty() {
            assert!(app.selected_station().is_some());
        }
    }

    #[test]
    fn search_results_preserve_active_browse_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::Music));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));

        let mut x = station("x", "https://example.com/x.mp3");
        x.tags = vec!["jazz".to_string()];
        app.apply(Action::SearchResults(SearchResults::from_stations([x])));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));
        assert_eq!(visible_ids(&app), vec!["x"]);
    }

    #[test]
    fn clearing_search_restores_previous_non_search_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::Music));

        let mut x = station("x", "https://example.com/x.mp3");
        x.tags = vec!["jazz".to_string()];
        app.apply(Action::SearchResults(SearchResults::from_stations([x])));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));

        app.apply(Action::ClearSearch);
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));
        // The restored list is the section's stations, not the search results.
        let music: Vec<String> = app
            .catalog
            .section_stations(Section::Music)
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(visible_ids(&app), music);
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn clearing_search_before_results_land_keeps_the_current_source() {
        // Focusing the search strip does not by itself enter the Search source;
        // clearing before any results arrive must stay on the active source
        // rather than restore a stale `previous_source` (default All Stations).
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowCategory(Category::Lofi));
        let lofi_visible = visible_ids(&app);
        assert_eq!(app.active_source(), ListSource::Category(Category::Lofi));

        app.apply(Action::SetFocus(FocusPane::Search));
        // No SearchResults have been applied yet.
        app.apply(Action::ClearSearch);

        assert_eq!(app.active_source(), ListSource::Category(Category::Lofi));
        assert_eq!(visible_ids(&app), lofi_visible);
    }

    #[test]
    fn clearing_search_defaults_to_all_stations_without_a_previous_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        app.apply(Action::ClearSearch);
        assert_eq!(app.active_source(), ListSource::AllStations);
    }

    #[test]
    fn repeated_searches_keep_the_original_previous_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowCategory(Category::Jazz));

        // Two searches in a row (as keystrokes produce) must not overwrite the
        // remembered non-search source with `Search`.
        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("y", "https://example.com/y.mp3"),
        ])));

        app.apply(Action::ClearSearch);
        assert_eq!(app.active_source(), ListSource::Category(Category::Jazz));
    }

    #[test]
    fn browse_all_stations_uses_search_population_when_available() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("search-a", "https://example.com/search-a.mp3"),
            station("search-b", "https://example.com/search-b.mp3"),
        ])));

        assert_eq!(app.active_source(), ListSource::AllStations);
        assert_eq!(visible_ids(&app), vec!["search-a", "search-b"]);

        app.apply(Action::ShowCatalog);
        assert_eq!(visible_ids(&app), vec!["search-a", "search-b"]);
    }

    #[test]
    fn browse_category_filters_full_search_population_not_current_visible() {
        let mut jazz = station("jazz", "https://example.com/jazz.mp3");
        jazz.tags = vec!["jazz".to_string()];
        let mut house = station("house", "https://example.com/house.mp3");
        house.tags = vec!["house".to_string()];

        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations([
            jazz.clone(),
            house.clone(),
        ])));

        app.apply(Action::ShowCategory(Category::Jazz));
        assert_eq!(visible_ids(&app), vec!["jazz"]);

        app.apply(Action::ShowCategory(Category::Electronic));
        assert_eq!(visible_ids(&app), vec!["house"]);
    }

    #[test]
    fn new_search_results_preserve_active_browse_filter() {
        let mut first_jazz = station("first-jazz", "https://example.com/first-jazz.mp3");
        first_jazz.tags = vec!["jazz".to_string()];
        let mut first_house = station("first-house", "https://example.com/first-house.mp3");
        first_house.tags = vec!["house".to_string()];

        let mut second_jazz = station("second-jazz", "https://example.com/second-jazz.mp3");
        second_jazz.tags = vec!["smooth jazz".to_string()];
        let mut second_house = station("second-house", "https://example.com/second-house.mp3");
        second_house.tags = vec!["techno".to_string()];

        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations([
            first_jazz,
            first_house,
        ])));
        app.apply(Action::ShowCategory(Category::Jazz));

        app.apply(Action::SearchResults(SearchResults::from_stations([
            second_jazz,
            second_house,
        ])));

        assert_eq!(app.active_source(), ListSource::Category(Category::Jazz));
        assert_eq!(visible_ids(&app), vec!["second-jazz"]);
    }

    #[test]
    fn clearing_search_preserves_browse_source_and_rebuilds_from_curated() {
        let mut search_jazz = station("search-jazz", "https://example.com/search-jazz.mp3");
        search_jazz.tags = vec!["jazz".to_string()];

        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SearchResults(SearchResults::from_stations([
            search_jazz,
        ])));
        app.apply(Action::ShowCategory(Category::Jazz));
        assert_eq!(visible_ids(&app), vec!["search-jazz"]);

        app.apply(Action::ClearSearch);

        let curated_jazz = app
            .catalog
            .category_stations(Category::Jazz)
            .iter()
            .map(|station| station.id.as_str().to_string())
            .collect::<Vec<_>>();
        assert_eq!(app.active_source(), ListSource::Category(Category::Jazz));
        assert_eq!(visible_ids(&app), curated_jazz);
    }

    #[test]
    fn favorites_source_ignores_search_population() {
        let favorite = station("fav-only", "https://example.com/fav-only.mp3");
        let settings = Settings {
            favorites: Favorites::from_stations([favorite.clone()]),
            ..Settings::default()
        };
        let mut app = App::new(settings, Catalog::curated());

        let mut search_jazz = station("search-jazz", "https://example.com/search-jazz.mp3");
        search_jazz.tags = vec!["jazz".to_string()];
        app.apply(Action::SearchResults(SearchResults::from_stations([
            search_jazz,
        ])));
        apply_favorites_source(&mut app);

        assert_eq!(app.active_source(), ListSource::Favorites);
        assert_eq!(visible_ids(&app), vec!["fav-only"]);
    }

    #[test]
    fn search_filter_display_helpers_describe_active_search_filter_empty_state() {
        let mut house = station("house", "https://example.com/house.mp3");
        house.tags = vec!["house".to_string()];

        let mut app = App::new(Settings::default(), Catalog::curated());
        assert!(!app.has_search_population());
        assert_eq!(app.active_filter_label(), None);
        assert_eq!(app.search_filter_empty_note(), None);

        app.apply(Action::SearchResults(SearchResults::from_stations([house])));
        assert!(app.has_search_population());
        assert_eq!(app.active_filter_label(), Some("All Stations"));

        app.apply(Action::ShowCategory(Category::Jazz));
        assert_eq!(app.active_filter_label(), Some("Jazz"));
        assert_eq!(
            app.search_filter_empty_note(),
            Some("No Jazz results in current search".to_string())
        );
    }

    #[test]
    fn browse_selection_is_tracked_through_the_reducer() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.browse_selected(), 0);
        app.apply(Action::SetBrowseSelection(3));
        assert_eq!(app.browse_selected(), 3);
    }

    #[test]
    fn browse_selection_moves_within_rail_bounds() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.browse_selected(), 0);
        // Up at the top stays at the top.
        app.apply(Action::BrowseSelectPrevious);
        assert_eq!(app.browse_selected(), 0);
        // Down advances one row.
        app.apply(Action::BrowseSelectNext);
        assert_eq!(app.browse_selected(), 1);
        // Last jumps to the final rail row and never past it.
        let last = ListSource::browse_rail().len() - 1;
        app.apply(Action::BrowseSelectLast);
        assert_eq!(app.browse_selected(), last);
        app.apply(Action::BrowseSelectNext);
        assert_eq!(app.browse_selected(), last, "Down at the end stays put");
        // First jumps back to the top.
        app.apply(Action::BrowseSelectFirst);
        assert_eq!(app.browse_selected(), 0);
    }

    #[test]
    fn applying_browse_selection_sets_source_and_hands_focus_to_stations() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SetFocus(FocusPane::Sections));
        // Park the Browse cursor on a known category and apply it.
        let rail = ListSource::browse_rail();
        let lofi_index = rail
            .iter()
            .position(|s| *s == ListSource::Category(Category::Lofi))
            .unwrap();
        app.apply(Action::SetBrowseSelection(lofi_index));
        app.apply(Action::ApplyBrowseSelection);

        assert_eq!(app.active_source(), ListSource::Category(Category::Lofi));
        assert_eq!(app.focus(), FocusPane::Stations);
        // The visible list is the applied source's stations, selection reset.
        let lofi: Vec<String> = app
            .catalog
            .category_stations(Category::Lofi)
            .iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(visible_ids(&app), lofi);
        assert_eq!(app.selected_index(), 0);
    }

    #[test]
    fn applying_browse_favorites_records_source_and_builds_from_settings() {
        // Applying Favorites records the source, hands off focus, and builds the
        // visible list from persisted favorites (empty here, so an empty list).
        let mut app = App::new(Settings::default(), Catalog::curated());
        let rail = ListSource::browse_rail();
        let fav_index = rail
            .iter()
            .position(|s| *s == ListSource::Favorites)
            .unwrap();
        app.apply(Action::SetBrowseSelection(fav_index));
        app.apply(Action::ApplyBrowseSelection);
        assert_eq!(app.active_source(), ListSource::Favorites);
        assert_eq!(app.focus(), FocusPane::Stations);
        assert!(app.visible().is_empty());
    }

    #[test]
    fn applying_browse_selection_clamps_an_out_of_range_cursor() {
        // A stale/oversized Browse index must still land on a real rail source.
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::SetBrowseSelection(usize::MAX));
        app.apply(Action::ApplyBrowseSelection);
        let last = *ListSource::browse_rail().last().unwrap();
        assert_eq!(app.active_source(), last);
        assert_eq!(app.focus(), FocusPane::Stations);
    }

    #[test]
    fn browse_rail_is_a_flat_list_of_sources_and_categories() {
        let rail = ListSource::browse_rail();
        // Leads with the two cross-cutting sources.
        assert_eq!(rail[0], ListSource::AllStations);
        assert_eq!(rail[1], ListSource::Favorites);
        // Each section is immediately followed by its own categories.
        let music_at = rail
            .iter()
            .position(|s| *s == ListSource::Section(Section::Music))
            .unwrap();
        for (offset, &category) in Section::Music.categories().iter().enumerate() {
            assert_eq!(rail[music_at + 1 + offset], ListSource::Category(category));
        }
        // Every catalog source appears exactly once; Search never does.
        assert!(rail.contains(&ListSource::Section(Section::SpokenNews)));
        assert!(rail.contains(&ListSource::Category(Category::Talk)));
        assert!(!rail.contains(&ListSource::Search));
        // 2 cross-cutting + 2 sections + every category.
        let categories = Section::ALL
            .iter()
            .map(|s| s.categories().len())
            .sum::<usize>();
        assert_eq!(rail.len(), 2 + Section::ALL.len() + categories);
    }

    #[test]
    fn browse_rail_has_exactly_one_favorites_entry_titled_favorites() {
        // Scope: a single favorites Browse mode. The rail must contain exactly one
        // favorites source, labelled plainly `Favorites` — no `All Favorites` or
        // `Current Favorites` split.
        let rail = ListSource::browse_rail();
        let favorites: Vec<ListSource> = rail
            .iter()
            .copied()
            .filter(|s| *s == ListSource::Favorites)
            .collect();
        assert_eq!(favorites, vec![ListSource::Favorites]);
        assert_eq!(ListSource::Favorites.title(), "Favorites");
        // No rail label uses the dropped two-mode wording.
        for source in &rail {
            assert_ne!(source.title(), "All Favorites");
            assert_ne!(source.title(), "Current Favorites");
        }
    }

    #[test]
    fn browse_rail_titles_come_from_catalog_state() {
        assert_eq!(ListSource::AllStations.title(), "All Stations");
        assert_eq!(ListSource::Favorites.title(), "Favorites");
        assert_eq!(
            ListSource::Section(Section::SpokenNews).title(),
            Section::SpokenNews.title()
        );
        assert_eq!(
            ListSource::Category(Category::Lofi).title(),
            Category::Lofi.title()
        );
    }

    // --- Favorites ListSource behavior (MIK-021) ------------------------

    /// Build an app whose persisted favorites are exactly `ids`, in order.
    fn app_with_favorites(ids: &[&str]) -> App {
        let favorites = Favorites::from_stations(
            ids.iter()
                .map(|id| station(id, &format!("https://example.com/{id}.mp3"))),
        );
        let settings = Settings {
            favorites,
            ..Settings::default()
        };
        App::new(settings, Catalog::curated())
    }

    /// Activate the Favorites source through the Browse rail, the wired path.
    fn apply_favorites_source(app: &mut App) {
        let rail = ListSource::browse_rail();
        let fav_index = rail
            .iter()
            .position(|s| *s == ListSource::Favorites)
            .unwrap();
        app.apply(Action::SetBrowseSelection(fav_index));
        app.apply(Action::ApplyBrowseSelection);
    }

    #[test]
    fn favorites_source_lists_persisted_favorites_and_is_playable() {
        let mut app = app_with_favorites(&["fav-a", "fav-b"]);
        apply_favorites_source(&mut app);

        assert_eq!(app.active_source(), ListSource::Favorites);
        // Persisted favorites are reachable from the Favorites source.
        assert_eq!(visible_ids(&app), vec!["fav-a", "fav-b"]);
        assert_eq!(app.selected_index(), 0);

        // ...and playable like any other source.
        app.apply(Action::PlaySelected);
        assert_eq!(app.current_station().unwrap().id.as_str(), "fav-a");
        assert_eq!(app.playback(), &PlaybackState::Connecting);
    }

    #[test]
    fn empty_favorites_stays_on_favorites_source() {
        // Empty Favorites must not silently fall back to All Stations.
        let mut app = app_with_favorites(&[]);
        apply_favorites_source(&mut app);

        assert_eq!(app.active_source(), ListSource::Favorites);
        assert!(app.visible().is_empty());
        assert!(app.selected_station().is_none());
    }

    #[test]
    fn toggling_favorite_in_favorites_source_removes_it_from_visible_immediately() {
        let mut app = app_with_favorites(&["a", "b", "c"]);
        apply_favorites_source(&mut app);
        // Select the middle favorite, then unfavorite it.
        app.apply(Action::SelectNext); // index 1 == "b"
        app.apply(Action::ToggleFavorite);

        // Removed from both the persisted collection and the visible list.
        assert_eq!(app.settings().favorites.len(), 2);
        assert_eq!(visible_ids(&app), vec!["a", "c"]);
        // Selection stays in place and now points at the next valid row.
        assert_eq!(app.selected_index(), 1);
        assert_eq!(app.selected_station().unwrap().id.as_str(), "c");
    }

    #[test]
    fn removing_first_favorite_keeps_selection_on_the_next_row() {
        let mut app = app_with_favorites(&["a", "b", "c"]);
        apply_favorites_source(&mut app);
        app.apply(Action::SelectFirst); // index 0 == "a"
        app.apply(Action::ToggleFavorite);

        assert_eq!(visible_ids(&app), vec!["b", "c"]);
        assert_eq!(app.selected_index(), 0);
        assert_eq!(app.selected_station().unwrap().id.as_str(), "b");
    }

    #[test]
    fn removing_last_favorite_clamps_selection_to_the_previous_row() {
        let mut app = app_with_favorites(&["a", "b", "c"]);
        apply_favorites_source(&mut app);
        app.apply(Action::SelectLast); // index 2 == "c"
        app.apply(Action::ToggleFavorite);

        assert_eq!(visible_ids(&app), vec!["a", "b"]);
        assert_eq!(app.selected_index(), 1);
        assert_eq!(app.selected_station().unwrap().id.as_str(), "b");
    }

    #[test]
    fn removing_the_only_favorite_yields_an_empty_favorites_state() {
        let mut app = app_with_favorites(&["only"]);
        apply_favorites_source(&mut app);
        app.apply(Action::ToggleFavorite);

        // Empty state on the Favorites source, not a fallback to All Stations.
        assert_eq!(app.active_source(), ListSource::Favorites);
        assert!(app.visible().is_empty());
        assert_eq!(app.selected_index(), 0);
        assert!(app.selected_station().is_none());
    }

    #[test]
    fn toggling_favorite_outside_favorites_source_leaves_visible_untouched() {
        // Unfavoriting while browsing a catalog source must not mutate the list.
        let mut app = app_with(&["a", "b", "c"]);
        app.apply(Action::ToggleFavorite); // favorites "a" (visible is Search source)
        let before = visible_ids(&app);
        app.apply(Action::ToggleFavorite); // unfavorite "a"
        assert_eq!(visible_ids(&app), before);
    }

    #[test]
    fn applying_favorites_resets_selection_to_the_top() {
        // Applying the source replaces visible and resets selection, distinct from
        // the clamp-in-place behavior of an in-source removal.
        let mut app = app_with_favorites(&["a", "b", "c"]);
        apply_favorites_source(&mut app);
        app.apply(Action::SelectLast);
        assert_eq!(app.selected_index(), 2);

        apply_favorites_source(&mut app);
        assert_eq!(app.selected_index(), 0);
        assert_eq!(visible_ids(&app), vec!["a", "b", "c"]);
    }

    #[test]
    fn clearing_search_restores_favorites_contents() {
        let mut app = app_with_favorites(&["a", "b"]);
        apply_favorites_source(&mut app);
        assert_eq!(app.active_source(), ListSource::Favorites);

        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        assert_eq!(app.active_source(), ListSource::Favorites);

        app.apply(Action::ClearSearch);
        assert_eq!(app.active_source(), ListSource::Favorites);
        assert_eq!(visible_ids(&app), vec!["a", "b"]);
    }

    // --- Signal View display mode (MIK-050) -----------------------------

    #[test]
    fn signal_view_toggle_is_display_only() {
        let mut app = app_with(&["one", "two"]);
        app.apply(Action::SetSearchQuery("jazz".to_string()));
        app.apply(Action::SetFocus(FocusPane::Search));
        let visible_before = visible_ids(&app);
        let selected_before = app.selected_index();
        let source_before = app.active_source();
        let query_before = app.search_query().to_string();

        app.apply(Action::ToggleSignalView);

        assert!(app.is_signal_view());
        assert_eq!(visible_ids(&app), visible_before);
        assert_eq!(app.selected_index(), selected_before);
        assert_eq!(app.active_source(), source_before);
        assert_eq!(app.search_query(), query_before);
        assert_eq!(app.focus(), FocusPane::Search);

        app.apply(Action::ToggleSignalView);

        assert!(!app.is_signal_view());
        assert_eq!(visible_ids(&app), visible_before);
    }

    #[test]
    fn leave_signal_view_is_idempotent() {
        let mut app = App::new(Settings::default(), Catalog::curated());

        app.apply(Action::LeaveSignalView);
        assert_eq!(app.display_mode(), DisplayMode::Normal);

        app.apply(Action::ToggleSignalView);
        assert_eq!(app.display_mode(), DisplayMode::SignalView);

        app.apply(Action::LeaveSignalView);
        assert_eq!(app.display_mode(), DisplayMode::Normal);

        app.apply(Action::LeaveSignalView);
        assert_eq!(app.display_mode(), DisplayMode::Normal);
    }

    #[test]
    fn toggle_current_favorite_uses_current_station_not_hidden_selection() {
        let mut app = app_with(&["selected", "current"]);
        app.apply(Action::SelectLast);
        app.apply(Action::PlaySelected);
        app.apply(Action::SelectFirst);

        let current = app.current_station().cloned().expect("current station");
        let hidden_selection = app.selected_station().cloned().expect("selected station");
        assert_ne!(current.id, hidden_selection.id);

        app.apply(Action::ToggleCurrentFavorite);

        assert!(app.is_favorite(&current));
        assert!(!app.is_favorite(&hidden_selection));
        assert!(app.current_station_is_favorite());

        app.apply(Action::ToggleCurrentFavorite);

        assert!(!app.is_favorite(&current));
        assert!(!app.current_station_is_favorite());
    }

    #[test]
    fn toggle_current_favorite_without_current_station_is_noop() {
        let mut app = app_with(&["selected"]);
        let selected = app.selected_station().cloned().expect("selected station");

        app.apply(Action::ToggleCurrentFavorite);

        assert!(!app.is_favorite(&selected));
        assert!(!app.current_station_is_favorite());
    }

    #[test]
    fn toggle_current_favorite_in_favorites_source_refreshes_visible() {
        let mut app = app_with_favorites(&["a", "b", "c"]);
        apply_favorites_source(&mut app);
        // Play the middle favorite so it becomes the current station, then move
        // the hidden selection elsewhere to prove current drives the removal.
        app.apply(Action::SelectNext); // index 1 == "b"
        app.apply(Action::PlaySelected); // current = "b"
        app.apply(Action::SelectFirst); // hidden selection back to "a"

        app.apply(Action::ToggleCurrentFavorite);

        // "b" (current) is removed from the visible Favorites list immediately,
        // and selection is clamped in place like existing favorite behavior.
        assert_eq!(app.settings().favorites.len(), 2);
        assert_eq!(visible_ids(&app), vec!["a", "c"]);
        assert_eq!(app.selected_index(), 0);
    }

    // --- Herdr Agent Pulse reducer state (Current: live-only) ------------

    /// A typed agent entry as the Herdr adapter would deliver it.
    fn agent(
        workspace: &str,
        pane: &str,
        name: Option<&str>,
        status: AgentStatus,
    ) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new(workspace, pane),
            name: name.map(str::to_string),
            status,
        }
    }

    fn agent_id(workspace: &str, pane: &str) -> AgentId {
        AgentId::new(workspace, pane)
    }

    fn agent_snapshot(agents: Vec<AgentSnapshot>, now: Instant) -> Action {
        Action::AgentSnapshot { agents, now }
    }

    /// An app that received one Agent Pulse snapshot.
    fn app_with_agents(agents: Vec<AgentSnapshot>) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(agents, Instant::now()));
        app
    }

    #[test]
    fn stale_viz_is_captured_at_the_stale_edge_and_cleared_on_recovery() {
        let mut app = app_with_agents(vec![agent("ws", "p1", None, AgentStatus::Working)]);
        let older = VizFrame::new(vec![0.2; 16], 0.2, Vec::<f32>::new());
        let last = VizFrame::new(vec![0.8; 16], 0.9, Vec::<f32>::new());
        app.apply(Action::Audio(AudioEvent::Viz(older.clone())));
        app.apply(Action::Audio(AudioEvent::Viz(last.clone())));
        assert!(app.stale_viz().is_none(), "connected keeps no snapshot");

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        let (frame, history) = app
            .stale_viz()
            .expect("the stale edge captures the display");
        assert_eq!(frame, &last, "the frozen frame is the last live frame");
        assert_eq!(
            history.first(),
            Some(&older),
            "prior trail frames are retained"
        );

        // Later audio and repeated failures do not move the snapshot.
        app.apply(Action::Audio(AudioEvent::Viz(VizFrame::new(
            vec![0.1; 16],
            0.1,
            Vec::<f32>::new(),
        ))));
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert_eq!(app.stale_viz().unwrap().0, &last);

        // A fresh snapshot recovers and clears the frozen display.
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Working)],
            Instant::now(),
        ));
        assert!(app.stale_viz().is_none(), "recovery clears the snapshot");

        // Unavailable clears it as well: no lights means no frozen field.
        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        assert!(app.stale_viz().is_some());
        app.apply(Action::AgentPollFailed {
            now: Instant::now() + crate::herdr::STALE_AFTER + Duration::from_secs(60),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );
        assert!(app.stale_viz().is_none(), "unavailable clears the snapshot");
    }

    fn active_names(app: &App) -> Vec<Option<&str>> {
        app.active_agents()
            .iter()
            .map(|view| view.name.as_deref())
            .collect()
    }

    #[test]
    fn agent_pulse_defaults_to_hidden_and_empty() {
        // Standalone launches must keep the exact pre-integration appearance:
        // no connection, no agents, no overlay, no selection.
        let app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Hidden);
        assert!(app.active_agents().is_empty());
        assert!(!app.is_agent_overlay_open());
        assert!(app.selected_agent().is_none());
    }

    #[test]
    fn agent_snapshot_connects_and_sorts_working_blocked_idle_done_unknown() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let now = Instant::now();
        app.apply(agent_snapshot(
            vec![
                agent("ws", "p1", Some("unknown"), AgentStatus::Unknown),
                agent("ws", "p2", Some("done"), AgentStatus::Done),
                agent("ws", "p3", Some("idle"), AgentStatus::Idle),
                agent("ws", "p4", Some("blocked"), AgentStatus::Blocked),
                agent("ws", "p5", Some("w-z"), AgentStatus::Working),
                agent("ws", "p6", Some("w-a"), AgentStatus::Working),
            ],
            now,
        ));

        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        assert_eq!(
            active_names(&app),
            vec![
                Some("w-a"),
                Some("w-z"),
                Some("blocked"),
                Some("idle"),
                Some("done"),
                Some("unknown"),
            ]
        );
    }

    #[test]
    fn agents_with_equal_status_sort_named_first_then_by_stable_identity() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(
            vec![
                agent("ws", "b-pane", None, AgentStatus::Working),
                agent("ws", "z-pane", Some("aaa"), AgentStatus::Working),
                agent("ws", "a-pane", None, AgentStatus::Working),
                agent("ws", "y-pane", Some("bbb"), AgentStatus::Working),
            ],
            Instant::now(),
        ));

        // Named agents sort alphabetically before unnamed ones; unnamed
        // agents keep a deterministic order via their stable identity.
        assert_eq!(
            active_names(&app),
            vec![Some("aaa"), Some("bbb"), None, None]
        );
        assert_eq!(app.active_agents()[2].id, agent_id("ws", "a-pane"));
        assert_eq!(app.active_agents()[3].id, agent_id("ws", "b-pane"));
    }

    #[test]
    fn identical_pane_ids_from_two_workspaces_remain_distinct_and_selectable() {
        let mut app = app_with_agents(vec![
            agent("alpha", "p1", Some("research"), AgentStatus::Working),
            agent("beta", "p1", Some("review"), AgentStatus::Idle),
        ]);
        assert_eq!(app.active_agents().len(), 2, "one particle per agent");

        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectAgent(agent_id("beta", "p1")));
        assert_eq!(
            app.selected_agent().unwrap().name.as_deref(),
            Some("review")
        );
    }

    #[test]
    fn done_agent_remains_live_until_the_next_snapshot_omits_it() {
        let t0 = Instant::now();
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::AgentSnapshot {
            agents: vec![agent("alpha", "p1", Some("research"), AgentStatus::Done)],
            now: t0,
        });
        assert_eq!(app.active_agents().len(), 1);
        assert_eq!(app.active_agents()[0].status, AgentStatus::Done);

        // Unchanged polling snapshots keep the done agent (and its observed
        // time) in the live view so the UI can render it dimmed.
        app.apply(Action::AgentSnapshot {
            agents: vec![agent("alpha", "p1", Some("research"), AgentStatus::Done)],
            now: t0 + Duration::from_secs(2),
        });
        assert_eq!(app.active_agents().len(), 1);
        assert_eq!(app.active_agents()[0].observed_at, t0);

        // The next snapshot that omits it removes it; nothing is retained.
        app.apply(Action::AgentSnapshot {
            agents: vec![],
            now: t0 + Duration::from_secs(5),
        });
        assert!(app.active_agents().is_empty());
    }

    #[test]
    fn observed_time_is_kept_while_status_is_unchanged_and_reset_on_change() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Working)],
            t0,
        ));

        // Same status in a later snapshot keeps the first observation time.
        let t1 = t0 + Duration::from_secs(5);
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Working)],
            t1,
        ));
        let view = &app.active_agents()[0];
        assert_eq!(view.observed_at, t0);
        assert_eq!(t1.duration_since(view.observed_at), Duration::from_secs(5));

        // A status change resets the local observation time.
        let t2 = t0 + Duration::from_secs(9);
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Blocked)],
            t2,
        ));
        assert_eq!(app.active_agents()[0].observed_at, t2);
    }

    #[test]
    fn observed_time_is_tracked_per_workspace_qualified_identity() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(agent_snapshot(
            vec![agent("alpha", "p1", Some("research"), AgentStatus::Working)],
            t0,
        ));

        // A same-pane agent from another workspace is a new identity: it gets
        // its own observation time without disturbing the first agent's.
        let t1 = t0 + Duration::from_secs(5);
        app.apply(agent_snapshot(
            vec![
                agent("alpha", "p1", Some("research"), AgentStatus::Working),
                agent("beta", "p1", Some("review"), AgentStatus::Working),
            ],
            t1,
        ));
        let research = app
            .active_agents()
            .iter()
            .find(|view| view.id == agent_id("alpha", "p1"))
            .unwrap();
        let review = app
            .active_agents()
            .iter()
            .find(|view| view.id == agent_id("beta", "p1"))
            .unwrap();
        assert_eq!(research.observed_at, t0);
        assert_eq!(review.observed_at, t1);
    }

    #[test]
    fn first_poll_failure_is_stale_and_fifteen_seconds_without_success_is_unavailable() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", Some("alpha"), AgentStatus::Working)],
            t0,
        ));

        // First failure dims to stale; the last known agents are retained.
        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(5),
        });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);
        assert_eq!(active_names(&app), vec![Some("alpha")]);

        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(14),
        });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);

        // Fifteen seconds without a success makes it unavailable.
        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(15),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );
    }

    #[test]
    fn poll_failure_before_any_snapshot_is_stale_then_unavailable() {
        // The first event proving the integration is alive may be a failure;
        // the 15-second window then runs from the start of the failure streak.
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(Action::AgentPollFailed { now: t0 });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);
        assert!(app.active_agents().is_empty());

        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(15),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );
    }

    #[test]
    fn agent_tick_downgrades_to_unavailable_without_new_events() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();

        // A tick never reveals a hidden integration.
        app.apply(Action::AgentTick { now: t0 });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Hidden);

        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Working)],
            t0,
        ));
        app.apply(Action::AgentTick {
            now: t0 + Duration::from_secs(5),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );

        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(6),
        });
        app.apply(Action::AgentTick {
            now: t0 + Duration::from_secs(14),
        });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);

        // The 15-second threshold applies even when no further monitor event
        // arrives — only the tick observes it.
        app.apply(Action::AgentTick {
            now: t0 + Duration::from_secs(15),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );

        // A monitor that goes completely silent (no failure event at all)
        // also times out via ticks.
        let mut quiet = App::new(Settings::default(), Catalog::curated());
        quiet.apply(agent_snapshot(vec![], t0));
        quiet.apply(Action::AgentTick {
            now: t0 + Duration::from_secs(15),
        });
        assert_eq!(
            quiet.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );
    }

    #[test]
    fn a_fresh_snapshot_recovers_from_stale_and_unavailable() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", Some("one"), AgentStatus::Working)],
            t0,
        ));
        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(5),
        });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);

        // Fresh state replaces stale state.
        let t1 = t0 + Duration::from_secs(7);
        app.apply(agent_snapshot(
            vec![agent("ws", "p2", Some("two"), AgentStatus::Working)],
            t1,
        ));
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        assert_eq!(active_names(&app), vec![Some("two")]);

        // Recovery also works from unavailable.
        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(8),
        });
        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(23),
        });
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Unavailable
        );
        app.apply(agent_snapshot(
            vec![agent("ws", "p3", Some("three"), AgentStatus::Idle)],
            t0 + Duration::from_secs(25),
        ));
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        assert_eq!(active_names(&app), vec![Some("three")]);
    }

    #[test]
    fn an_empty_snapshot_is_connected_with_no_active_agents() {
        // Connected-with-none-active is a real state, distinct from hidden
        // and from unavailable.
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(vec![], Instant::now()));
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        assert!(app.active_agents().is_empty());

        // Selection on an empty list stays cleared even with the overlay open.
        app.apply(Action::ToggleAgentOverlay);
        assert!(app.is_agent_overlay_open());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::SelectPreviousAgent);
        assert!(app.selected_agent().is_none());
    }

    #[test]
    fn overlay_toggle_is_a_noop_while_agent_pulse_is_hidden() {
        // Standalone/ineligible launches: `a` does nothing, and no overlay
        // state can be reached.
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ToggleAgentOverlay);
        assert!(!app.is_agent_overlay_open());
        app.apply(Action::SelectNextAgent);
        app.apply(Action::SelectAgent(agent_id("ws", "p1")));
        assert!(app.selected_agent().is_none());
    }

    #[test]
    fn overlay_opens_and_closes_without_touching_radio_state() {
        let mut app = app_with(&["a", "b"]);
        app.apply(Action::PlaySelected);
        app.apply(Action::SetSearchQuery("jazz".to_string()));
        app.apply(Action::SetFocus(FocusPane::Search));
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", None, AgentStatus::Working)],
            Instant::now(),
        ));
        let visible_before = visible_ids(&app);
        let selected_before = app.selected_index();

        app.apply(Action::ToggleAgentOverlay);
        assert!(app.is_agent_overlay_open());
        app.apply(Action::ToggleAgentOverlay);
        assert!(!app.is_agent_overlay_open());
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::CloseAgentOverlay);
        assert!(!app.is_agent_overlay_open());

        // Station selection, focus, search, and playback are untouched.
        assert_eq!(visible_ids(&app), visible_before);
        assert_eq!(app.selected_index(), selected_before);
        assert_eq!(app.focus(), FocusPane::Search);
        assert_eq!(app.search_query(), "jazz");
        assert_eq!(app.playback(), &PlaybackState::Connecting);
        assert_eq!(app.current_station().unwrap().id.as_str(), "a");
        assert_eq!(app.display_mode(), DisplayMode::Normal);
    }

    #[test]
    fn signal_view_suppresses_all_agent_overlay_actions() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(
            vec![
                agent("ws", "p1", Some("alpha"), AgentStatus::Working),
                agent("ws", "p2", Some("beta"), AgentStatus::Working),
            ],
            Instant::now(),
        ));

        app.apply(Action::ToggleSignalView);
        app.apply(Action::ToggleAgentOverlay);
        assert!(!app.is_agent_overlay_open(), "Signal View suppresses `a`");
        app.apply(Action::SelectNextAgent);
        app.apply(Action::SelectAgent(agent_id("ws", "p1")));
        assert!(app.selected_agent().is_none());
        assert!(app.is_signal_view(), "Signal View itself is unaffected");

        // Leaving Signal View restores the toggle.
        app.apply(Action::LeaveSignalView);
        app.apply(Action::ToggleAgentOverlay);
        assert!(app.is_agent_overlay_open());

        // Entering Signal View with the overlay open suppresses everything
        // too, including close.
        app.apply(Action::ToggleSignalView);
        app.apply(Action::CloseAgentOverlay);
        assert!(app.is_agent_overlay_open());
    }

    #[test]
    fn overlay_selection_moves_and_clamps_within_the_sorted_active_list() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(
            vec![
                agent("ws", "p1", Some("gamma"), AgentStatus::Working),
                agent("ws", "p2", Some("alpha"), AgentStatus::Working),
                agent("ws", "p3", Some("beta"), AgentStatus::Working),
            ],
            Instant::now(),
        ));
        app.apply(Action::ToggleAgentOverlay);

        let selected_name = |app: &App| app.selected_agent().and_then(|view| view.name.clone());

        // Next with no selection starts at the first sorted agent.
        app.apply(Action::SelectNextAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("alpha"));
        app.apply(Action::SelectNextAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("beta"));
        app.apply(Action::SelectNextAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("gamma"));
        // Down at the end stays put.
        app.apply(Action::SelectNextAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("gamma"));

        // Selecting an unknown identity changes nothing.
        app.apply(Action::SelectAgent(agent_id("ws", "missing")));
        assert_eq!(selected_name(&app).as_deref(), Some("gamma"));

        app.apply(Action::SelectPreviousAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("beta"));
        app.apply(Action::SelectPreviousAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("alpha"));
        // Up at the top stays put.
        app.apply(Action::SelectPreviousAgent);
        assert_eq!(selected_name(&app).as_deref(), Some("alpha"));

        // Direct selection by identity (mouse path).
        app.apply(Action::SelectAgent(agent_id("ws", "p3")));
        assert_eq!(selected_name(&app).as_deref(), Some("beta"));
    }

    #[test]
    fn select_previous_with_no_selection_starts_from_the_last_agent() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(
            vec![
                agent("ws", "p1", Some("alpha"), AgentStatus::Working),
                agent("ws", "p2", Some("beta"), AgentStatus::Working),
            ],
            Instant::now(),
        ));
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectPreviousAgent);
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("beta"));
    }

    #[test]
    fn agent_selection_requires_an_open_overlay() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(agent_snapshot(
            vec![agent("ws", "p1", Some("alpha"), AgentStatus::Working)],
            Instant::now(),
        ));

        app.apply(Action::SelectNextAgent);
        app.apply(Action::SelectAgent(agent_id("ws", "p1")));
        assert!(app.selected_agent().is_none());
    }

    #[test]
    fn stale_freezes_the_agent_selection_until_recovery() {
        // Selection matches the mouse hit-test gate: it changes only while
        // `Connected`. Stale keeps the frozen composition's selection intact
        // and ignores every selection action, while close/toggle still work.
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        let two_agents = || {
            vec![
                agent("ws", "p1", Some("alpha"), AgentStatus::Working),
                agent("ws", "p2", Some("beta"), AgentStatus::Working),
            ]
        };
        app.apply(agent_snapshot(two_agents(), t0));
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("alpha"));

        app.apply(Action::AgentPollFailed {
            now: t0 + Duration::from_secs(5),
        });
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);

        // Stale: keyboard movement and identity selection are all inert.
        app.apply(Action::SelectNextAgent);
        app.apply(Action::SelectPreviousAgent);
        app.apply(Action::SelectAgent(agent_id("ws", "p2")));
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("alpha"));

        // Close and toggle keep working while stale.
        app.apply(Action::CloseAgentOverlay);
        assert!(!app.is_agent_overlay_open());
        app.apply(Action::ToggleAgentOverlay);
        assert!(app.is_agent_overlay_open());

        // A fresh snapshot recovers the connection and re-enables selection.
        app.apply(agent_snapshot(two_agents(), t0 + Duration::from_secs(10)));
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        app.apply(Action::SelectNextAgent);
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("beta"));
    }

    #[test]
    fn selection_is_cleared_when_the_selected_agent_disappears() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        let t0 = Instant::now();
        app.apply(agent_snapshot(
            vec![
                agent("ws", "p1", Some("alpha"), AgentStatus::Working),
                agent("ws", "p2", Some("beta"), AgentStatus::Working),
            ],
            t0,
        ));
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectAgent(agent_id("ws", "p2")));
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("beta"));

        app.apply(agent_snapshot(
            vec![agent("ws", "p1", Some("alpha"), AgentStatus::Working)],
            t0 + Duration::from_secs(5),
        ));
        assert!(app.selected_agent().is_none());

        // Selecting again over the fresh list works.
        app.apply(Action::SelectNextAgent);
        assert_eq!(app.selected_agent().unwrap().name.as_deref(), Some("alpha"));
    }

    #[test]
    fn selected_agent_exposes_only_the_explicit_name() {
        // An unnamed agent stays selectable, but there is nothing else to
        // show: the view carries no pane id, cwd, or agent-type fallback.
        let mut app = app_with_agents(vec![agent("ws", "p1", None, AgentStatus::Working)]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let selected = app.selected_agent().expect("unnamed agent is selectable");
        assert_eq!(selected.name, None);
    }
}
