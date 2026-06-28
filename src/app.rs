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
use crate::catalog::{Catalog, Category, Section, SessionStationHealth, Stations};
use crate::model::{PlaybackState, Station, StationId, VizFrame, VolumePercent};
use crate::search::SearchResults;
use crate::settings::Settings;
use crate::theme::ThemeName;

/// Step applied to the volume for a single `VolumeUp`/`VolumeDown` action.
const VOLUME_STEP: i32 = 5;

/// Number of visualizer bands held in the current frame when idle/stopped.
const VIZ_BANDS: usize = 16;

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
    /// Section/category shortcuts for Music and Spoken/News.
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
/// Stations`, `Favorites`, both sections, and every category as sources; this
/// slice models the full set but only the catalog-derived sources (All,
/// Section, Category) build their list here. `Favorites` contents and `Search`
/// results are produced by other slices/controllers; the reducer only records
/// that one of them is active.
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
    /// Replace the live search query text shown in the search strip.
    SetSearchQuery(String),
    /// Update the search status shown in the search strip.
    SetSearchStatus(SearchStatus),
    /// Set the offline flag (network/Radio Browser reachability).
    SetOffline(bool),
    /// Apply an audio runtime event to playback state.
    Audio(AudioEvent),
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
    visible: Stations,
    selected: usize,
    source: ListSource,
    previous_source: ListSource,
    browse_selected: usize,
    focus: FocusPane,
    playback: PlaybackState,
    current: Option<Station>,
    viz: VizFrame,
    offline: bool,
    search_query: String,
    search_status: SearchStatus,
    now_playing_title: Option<String>,
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
        Self {
            catalog,
            settings,
            health: SessionStationHealth::new(),
            visible,
            selected: 0,
            source: ListSource::AllStations,
            previous_source: ListSource::AllStations,
            browse_selected: 0,
            focus: FocusPane::Stations,
            playback: PlaybackState::Stopped,
            current,
            viz: VizFrame::silent(VIZ_BANDS),
            offline: false,
            search_query: String::new(),
            search_status: SearchStatus::Idle,
            now_playing_title: None,
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
            Action::VolumeUp => self.change_volume(VOLUME_STEP),
            Action::VolumeDown => self.change_volume(-VOLUME_STEP),
            Action::SetVolume(volume) => self.settings.volume = volume,
            Action::ShowCatalog => self.show_source(ListSource::AllStations),
            Action::ShowSection(section) => self.show_source(ListSource::Section(section)),
            Action::ShowCategory(category) => self.show_source(ListSource::Category(category)),
            Action::ClearSearch => self.clear_search(),
            Action::SetBrowseSelection(index) => self.browse_selected = index,
            Action::SearchResults(results) => self.apply_search_results(results),
            Action::SetSearchQuery(query) => self.search_query = query,
            Action::SetSearchStatus(status) => self.search_status = status,
            Action::SetOffline(offline) => self.offline = offline,
            Action::Audio(event) => self.apply_audio(event),
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

    // --- list source -----------------------------------------------------

    /// Apply a non-search source: record it as active and rebuild the visible
    /// list from the catalog, resetting/clamping selection safely.
    ///
    /// Only catalog-derived sources (All, Section, Category) build their list in
    /// this slice. `Favorites` contents come from a later slice, so applying it
    /// here records the source without rebuilding the list; `Search` is never
    /// routed through here (it arrives via [`Self::apply_search_results`]).
    fn show_source(&mut self, source: ListSource) {
        self.source = source;
        match source {
            ListSource::AllStations => self.replace_visible(self.catalog.stations().ranked()),
            ListSource::Section(section) => {
                self.replace_visible(self.catalog.section_stations(section))
            }
            ListSource::Category(category) => {
                self.replace_visible(self.catalog.category_stations(category))
            }
            // Favorites list contents are out of scope for this slice; Search is
            // applied via search results, not this path. Selection is still
            // clamped so the bounds invariant holds after the source change.
            ListSource::Favorites | ListSource::Search => self.clamp_selection(),
        }
    }

    /// Restore the previous non-search source when search is cleared.
    ///
    /// `previous_source` is only ever set to a non-search source (see
    /// [`Self::apply_search_results`]), so restoring it always lands on a real
    /// catalog/favorites source rather than search results.
    fn clear_search(&mut self) {
        self.show_source(self.previous_source);
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
    fn toggle_favorite(&mut self) {
        let Some(station) = self.selected_station().cloned() else {
            return;
        };
        if self.settings.favorites.contains(&station) {
            self.settings.favorites.remove(&station);
        } else {
            self.settings.favorites.add(station);
        }
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
            AudioEvent::Viz(frame) => self.viz = frame,
            AudioEvent::IcyTitle { station, title } => self.on_icy_title(station, title),
        }
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

    /// Replace the visible list with search results and reset selection.
    ///
    /// Entering the search source remembers the previous non-search source so
    /// clearing search can restore it. Successive searches (e.g. one per
    /// keystroke) must not overwrite that memory with `Search`, so it is only
    /// captured on the transition into search.
    ///
    /// Receiving results means the network round-trip succeeded, so the offline
    /// flag is cleared.
    fn apply_search_results(&mut self, results: SearchResults) {
        if !self.source.is_search() {
            self.previous_source = self.source;
        }
        self.source = ListSource::Search;
        self.replace_visible(results.into_vec().into_iter().collect());
        self.offline = false;
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

    /// The active theme name.
    pub fn theme(&self) -> ThemeName {
        self.settings.theme
    }

    /// Read-only access to settings (volume, theme, favorites, previous station).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Whether a station is a current favorite (by id or URL identity).
    pub fn is_favorite(&self, station: &Station) -> bool {
        self.settings.favorites.contains(station)
    }

    /// Whether a station is marked failed for this session.
    pub fn is_failed(&self, id: &StationId) -> bool {
        self.health.is_failed(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BitrateKbps, CodecKind, StationId, StationName, StationSource, StreamUrl, VolumePercent,
    };
    use crate::theme::ThemeName;

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
    fn cycle_theme_advances_through_the_trio() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.theme(), ThemeName::Minimal);
        app.apply(Action::CycleTheme);
        assert_eq!(app.theme(), ThemeName::Neon);
        app.apply(Action::CycleTheme);
        assert_eq!(app.theme(), ThemeName::Crt);
        app.apply(Action::CycleTheme);
        assert_eq!(app.theme(), ThemeName::Minimal);
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
        let frame = VizFrame::new([0.1, 0.9, 0.5], 0.7);
        app.apply(Action::Audio(AudioEvent::Viz(frame.clone())));
        assert_eq!(app.viz(), &frame);
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
    fn searching_sets_search_source_and_remembers_previous() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::Music));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));

        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        assert_eq!(app.active_source(), ListSource::Search);
        assert_eq!(visible_ids(&app), vec!["x"]);
    }

    #[test]
    fn clearing_search_restores_previous_non_search_source() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.apply(Action::ShowSection(Section::Music));

        app.apply(Action::SearchResults(SearchResults::from_stations([
            station("x", "https://example.com/x.mp3"),
        ])));
        assert_eq!(app.active_source(), ListSource::Search);

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
    fn browse_selection_is_tracked_through_the_reducer() {
        let mut app = App::new(Settings::default(), Catalog::curated());
        assert_eq!(app.browse_selected(), 0);
        app.apply(Action::SetBrowseSelection(3));
        assert_eq!(app.browse_selected(), 3);
    }
}
