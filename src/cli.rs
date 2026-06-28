//! CLI argument parsing and the terminal/event-loop boundary.
//!
//! This module is the executable boundary: it parses untrusted CLI arguments
//! once into typed values (per "Parse, don't validate"), then drives the
//! terminal event loop that wires [`crate::app`], [`crate::audio`],
//! [`crate::search`], [`crate::settings`], and [`crate::ui`] together.
//!
//! Module boundaries are respected: the [`App`] reducer stays pure (this module
//! only dispatches [`Action`]s and observes the resulting state), rendering does
//! not mutate state, and the blocking Radio Browser transport runs on a
//! dedicated worker thread so neither network latency nor an async runtime ever
//! blocks rendering or input.
//!
//! The pure controller helpers — argument parsing ([`parse_args`]), key mapping
//! ([`map_key`]), the search debounce ([`SearchDebounce`]), and the startup
//! auto-play decision ([`startup_play_command`]) — are unit tested without a
//! terminal, audio device, or network. The terminal setup/teardown and the
//! event loop are thin adapters verified manually (see `docs/`).

use std::io::{self, Stdout};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::{Action, App, FocusPane, SearchStatus};
use crate::audio::{AudioCommand, AudioEvent, AudioHandle, AudioRuntime, AudioRuntimeConfig};
use crate::catalog::Catalog;
use crate::model::{PlaybackState, SearchQuery, VolumePercent};
use crate::search::{RadioBrowserClient, SearchCache, SearchError, SearchResults};
use crate::settings::{self, Settings};
use crate::theme::ThemeName;

/// Search debounce window. Within the 300–500ms band required by the spec: long
/// enough to coalesce keystrokes, short enough to feel responsive.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

/// Event-loop poll cadence under normal operation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Slower poll cadence when `--low-power` is set (audio is unaffected).
///
/// Kept at or below `500ms - SEARCH_DEBOUNCE` so debounced search still fires
/// inside the spec's 300–500ms responsiveness band even in low-power mode.
const POLL_INTERVAL_LOW_POWER: Duration = Duration::from_millis(150);

/// `--help` / usage text.
const USAGE: &str = "\
wave-tui — terminal-first internet radio for work sessions

USAGE:
    wave-tui [OPTIONS] [SEARCH]

OPTIONS:
    --theme <name>                Theme for this run: minimal | neon | crt
    --volume <0-100>              Startup volume override
    --no-auto-play                Start silently even if a previous station exists
    --audio-output-device <name>  CPAL output device name
    --low-power                   Lower UI update cadence (audio unaffected)
    --search <query>              Start in search mode with this query
    -h, --help                    Print help
    -V, --version                 Print version

ARGS:
    [SEARCH]                      Optional positional search query (same as --search)";

// === CLI parsing =========================================================

/// Parsed, typed CLI arguments. Boundary strings are converted to domain values
/// (`ThemeName`, `VolumePercent`) once, here, so the rest of the app trusts them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliArgs {
    pub theme: Option<ThemeName>,
    pub volume: Option<VolumePercent>,
    pub no_auto_play: bool,
    pub audio_output_device: Option<String>,
    pub low_power: bool,
    pub search: Option<String>,
}

/// What a successful parse asked the program to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliInvocation {
    /// Run the player with the given arguments.
    Run(CliArgs),
    /// Print usage and exit.
    Help,
    /// Print the version and exit.
    Version,
}

/// A recoverable CLI parsing error reported at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    UnknownFlag(String),
    MissingValue(String),
    InvalidVolume(String),
    InvalidTheme(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag(flag) => write!(f, "unknown option: {flag}"),
            CliError::MissingValue(flag) => write!(f, "missing value for option: {flag}"),
            CliError::InvalidVolume(raw) => {
                write!(f, "invalid --volume {raw:?} (expected an integer 0-100)")
            }
            CliError::InvalidTheme(raw) => {
                write!(
                    f,
                    "invalid --theme {raw:?} (expected minimal, neon, or crt)"
                )
            }
        }
    }
}

impl std::error::Error for CliError {}

/// Parse CLI arguments (excluding the program name) into a [`CliInvocation`].
///
/// Supports both `--flag value` and `--flag=value` forms for value options. The
/// first non-flag argument is treated as a positional search query. Unknown
/// flags and out-of-range values are recoverable [`CliError`]s, not panics.
pub fn parse_args<I>(args: I) -> Result<CliInvocation, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = CliArgs::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        // Split `--flag=value` once; `inline` carries the value when present.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };

        // Fetch a value from the inline form or the next argument.
        let mut take_value = |flag: &str| -> Result<String, CliError> {
            if let Some(value) = inline.clone() {
                Ok(value)
            } else {
                iter.next()
                    .ok_or_else(|| CliError::MissingValue(flag.to_string()))
            }
        };

        match flag.as_str() {
            "-h" | "--help" => return Ok(CliInvocation::Help),
            "-V" | "--version" => return Ok(CliInvocation::Version),
            "--no-auto-play" => parsed.no_auto_play = true,
            "--low-power" => parsed.low_power = true,
            "--theme" => {
                let raw = take_value("--theme")?;
                parsed.theme =
                    Some(ThemeName::parse(&raw).map_err(|_| CliError::InvalidTheme(raw))?);
            }
            "--volume" => {
                let raw = take_value("--volume")?;
                parsed.volume =
                    Some(VolumePercent::parse(&raw).map_err(|_| CliError::InvalidVolume(raw))?);
            }
            "--audio-output-device" => {
                parsed.audio_output_device = Some(take_value("--audio-output-device")?);
            }
            "--search" => parsed.search = Some(take_value("--search")?),
            other if other.starts_with('-') => {
                return Err(CliError::UnknownFlag(other.to_string()));
            }
            // First bare positional argument is a search query.
            _ => {
                if parsed.search.is_none() {
                    parsed.search = Some(arg);
                }
            }
        }
    }

    Ok(CliInvocation::Run(parsed))
}

// === Key mapping =========================================================

/// The controller-level meaning of a key press, independent of whether the
/// search input is active. Kept small and comparable so [`map_key`] is fully
/// unit testable without a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Quit,
    FocusNext,
    FocusPrevious,
    SelectNext,
    SelectPrevious,
    SelectFirst,
    SelectLast,
    /// `Enter`: play the selected station.
    Play,
    /// `Space`: stop/play toggle.
    TogglePlayback,
    ToggleFavorite,
    CycleTheme,
    VolumeUp,
    VolumeDown,
    /// `/`: focus the search strip.
    BeginSearch,
    /// A printable character typed while the search strip is focused.
    SearchChar(char),
    /// `Backspace` while the search strip is focused.
    SearchBackspace,
    /// `Esc` while the search strip is focused: clear and leave search.
    ClearSearch,
    /// No mapped meaning for this key in this mode.
    Ignore,
}

/// Map a terminal key event to a [`KeyOutcome`].
///
/// `searching` is `true` when the search strip is focused: printable keys then
/// edit the query instead of triggering navigation commands. Navigation keys
/// (`Tab`, arrows, `Enter`, `Home`/`End`) work in both modes; `Ctrl+C` always
/// quits.
pub fn map_key(key: KeyEvent, searching: bool) -> KeyOutcome {
    // Only act on presses (and auto-repeat); ignore key releases so terminals
    // that report both do not double-fire.
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return KeyOutcome::Ignore;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => KeyOutcome::Quit,
            _ => KeyOutcome::Ignore,
        };
    }

    // Navigation keys behave the same whether or not search input is active.
    match key.code {
        KeyCode::Tab => return KeyOutcome::FocusNext,
        KeyCode::BackTab => return KeyOutcome::FocusPrevious,
        KeyCode::Up => return KeyOutcome::SelectPrevious,
        KeyCode::Down => return KeyOutcome::SelectNext,
        KeyCode::Home => return KeyOutcome::SelectFirst,
        KeyCode::End => return KeyOutcome::SelectLast,
        KeyCode::Enter => return KeyOutcome::Play,
        _ => {}
    }

    if searching {
        match key.code {
            KeyCode::Esc => KeyOutcome::ClearSearch,
            KeyCode::Backspace => KeyOutcome::SearchBackspace,
            KeyCode::Char(c) => KeyOutcome::SearchChar(c),
            _ => KeyOutcome::Ignore,
        }
    } else {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => KeyOutcome::Quit,
            KeyCode::Char('j') => KeyOutcome::SelectNext,
            KeyCode::Char('k') => KeyOutcome::SelectPrevious,
            KeyCode::Char('g') => KeyOutcome::SelectFirst,
            KeyCode::Char('G') => KeyOutcome::SelectLast,
            KeyCode::Char('/') => KeyOutcome::BeginSearch,
            KeyCode::Char(' ') => KeyOutcome::TogglePlayback,
            KeyCode::Char('+') | KeyCode::Char('=') => KeyOutcome::VolumeUp,
            KeyCode::Char('-') | KeyCode::Char('_') => KeyOutcome::VolumeDown,
            KeyCode::Char('f') => KeyOutcome::ToggleFavorite,
            KeyCode::Char('t') => KeyOutcome::CycleTheme,
            _ => KeyOutcome::Ignore,
        }
    }
}

// === Search debounce =====================================================

/// Whether a query change scheduled a pending search or cleared the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryChange {
    /// The query is empty; any pending/in-flight search is cancelled.
    Cleared,
    /// A non-empty query was scheduled to fire after the debounce window.
    Scheduled,
}

/// A pending, not-yet-fired search.
#[derive(Debug, Clone)]
struct PendingSearch {
    query: SearchQuery,
    generation: u64,
    due: Instant,
}

/// Debounce-and-staleness state for online search.
///
/// Every query change bumps a monotonic generation counter, so a search fired
/// for an older keystroke can be recognized as stale and ignored when its result
/// returns ([`SearchDebounce::is_current`]). A non-empty query schedules a fire
/// time `debounce` in the future; [`SearchDebounce::take_due`] yields it once the
/// window elapses.
#[derive(Debug)]
pub struct SearchDebounce {
    debounce: Duration,
    generation: u64,
    pending: Option<PendingSearch>,
}

impl SearchDebounce {
    /// Build a debounce with the given window.
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            generation: 0,
            pending: None,
        }
    }

    /// The latest query generation. Results tagged with an older generation are
    /// stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether `generation` matches the latest query generation (i.e. no newer
    /// keystroke has superseded it).
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// Record a query change. Bumps the generation (invalidating any in-flight
    /// search) and either schedules a fire after the debounce window or clears
    /// the pending search when the query is empty/whitespace.
    pub fn note_query(&mut self, raw: &str, now: Instant) -> QueryChange {
        // Every change advances the generation so any in-flight search for an
        // earlier query is recognized as stale when its result returns.
        self.generation += 1;
        match SearchQuery::parse(raw) {
            Ok(query) => {
                self.pending = Some(PendingSearch {
                    query,
                    generation: self.generation,
                    due: now + self.debounce,
                });
                QueryChange::Scheduled
            }
            Err(_) => {
                self.pending = None;
                QueryChange::Cleared
            }
        }
    }

    /// Take the pending search if its debounce window has elapsed by `now`.
    pub fn take_due(&mut self, now: Instant) -> Option<(SearchQuery, u64)> {
        match &self.pending {
            Some(pending) if now >= pending.due => {
                let pending = self.pending.take().expect("checked Some above");
                Some((pending.query, pending.generation))
            }
            _ => None,
        }
    }
}

// === Startup auto-play ===================================================

/// The audio command to issue at startup, if any.
///
/// On a normal launch the previous station is auto-played at the persisted
/// volume; `--no-auto-play` or the absence of a previous station starts silently
/// (the spec's first-launch / failed-previous behavior).
pub fn startup_play_command(settings: &Settings, no_auto_play: bool) -> Option<AudioCommand> {
    if no_auto_play {
        return None;
    }
    settings
        .previous_station
        .clone()
        .map(|station| AudioCommand::Play {
            station: Box::new(station),
            volume: settings.volume,
        })
}

// === Settings overrides and persistence policy ===========================

/// Apply per-run CLI overrides onto loaded settings.
///
/// `--theme` and `--volume` adjust the in-memory settings used for this run
/// (display, audio startup volume, and the base for `+`/`-` stepping). What is
/// later *persisted* is governed separately by [`Persistence`].
fn apply_overrides(mut settings: Settings, args: &CliArgs) -> Settings {
    if let Some(theme) = args.theme {
        settings.theme = theme;
    }
    if let Some(volume) = args.volume {
        settings.volume = volume;
    }
    settings
}

/// Persistence policy for a single run.
///
/// `--volume` is a one-run startup override: it must not be written back to disk
/// merely because the app shut down cleanly or because some *other* setting
/// (favorites, previous station) changed. The saved volume is only updated when
/// the user actually changes volume via `+`/`-` during the session. Everything
/// else — favorites, previous station, and the `--theme` override — persists as
/// normal.
struct Persistence {
    /// The volume that was on disk before CLI overrides were applied.
    baseline_volume: VolumePercent,
    /// Whether the user changed volume via `+`/`-` during this run.
    user_changed_volume: bool,
}

impl Persistence {
    fn new(baseline_volume: VolumePercent) -> Self {
        Self {
            baseline_volume,
            user_changed_volume: false,
        }
    }

    /// Record that the user changed volume, so it is now theirs to keep.
    fn mark_user_changed_volume(&mut self) {
        self.user_changed_volume = true;
    }

    /// The settings to actually write to disk: identical to `current`, except the
    /// volume falls back to the saved baseline when the user has not changed it
    /// (discarding any one-run `--volume` override).
    fn settings_to_save(&self, current: &Settings) -> Settings {
        if self.user_changed_volume {
            current.clone()
        } else {
            Settings {
                volume: self.baseline_volume,
                ..current.clone()
            }
        }
    }

    /// Persist the app's settings under this policy (best-effort).
    fn save(&self, app: &App) {
        let _ = settings::save(&self.settings_to_save(app.settings()));
    }
}

// === Search worker thread ================================================

/// A request sent to the blocking search worker thread.
enum SearchRequest {
    Query { query: SearchQuery, generation: u64 },
    Shutdown,
}

/// A response from the search worker, tagged with the generation it was fired
/// for so the controller can drop stale results.
struct SearchResponse {
    generation: u64,
    result: Result<SearchResults, SearchError>,
    from_cache: bool,
}

/// The blocking Radio Browser worker: owns the HTTP client and the query cache,
/// isolating `reqwest::blocking` on its own thread so rendering never blocks.
/// Cached queries are served without a second network call.
fn search_worker(rx: Receiver<SearchRequest>, tx: Sender<SearchResponse>) {
    let client = RadioBrowserClient::new();
    let mut cache = SearchCache::new();
    while let Ok(request) = rx.recv() {
        let (query, generation) = match request {
            SearchRequest::Query { query, generation } => (query, generation),
            SearchRequest::Shutdown => break,
        };
        let response = match &client {
            Ok(client) => {
                let from_cache = cache.contains(&query);
                match client.search_cached(&mut cache, &query) {
                    Ok(results) => SearchResponse {
                        generation,
                        result: Ok(results),
                        from_cache,
                    },
                    Err(err) => SearchResponse {
                        generation,
                        result: Err(err),
                        from_cache: false,
                    },
                }
            }
            Err(err) => SearchResponse {
                generation,
                result: Err(err.clone()),
                from_cache: false,
            },
        };
        if tx.send(response).is_err() {
            break;
        }
    }
}

// === Terminal lifecycle ==================================================

/// RAII guard owning the terminal in raw/alternate-screen mode.
///
/// Restoration runs in [`Drop`], so the terminal is restored on a normal quit,
/// on a recoverable error returned from the event loop, and on a panic.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating terminal")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

// === Entry point and event loop ==========================================

/// CLI entry point invoked by `main`.
pub fn run() -> Result<()> {
    let invocation =
        parse_args(std::env::args().skip(1)).map_err(|err| anyhow::anyhow!("{err}\n\n{USAGE}"))?;
    match invocation {
        CliInvocation::Help => {
            println!("{USAGE}");
            Ok(())
        }
        CliInvocation::Version => {
            println!("wave-tui {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliInvocation::Run(args) => run_app(args),
    }
}

/// Bootstrap settings, catalog, audio runtime, and the search worker, then run
/// the terminal event loop. Settings are persisted on relevant changes and on a
/// clean shutdown.
fn run_app(args: CliArgs) -> Result<()> {
    // Load persisted settings, falling back to safe defaults on a missing or
    // corrupt file, then apply per-run CLI overrides. The baseline volume (the
    // value actually on disk) is remembered so the one-run `--volume` override is
    // not written back on shutdown unless the user changes volume during the run.
    let saved = settings::load().unwrap_or_default();
    let mut persistence = Persistence::new(saved.volume);
    let settings = apply_overrides(saved, &args);

    let mut app = App::new(settings, Catalog::curated());

    let audio = AudioRuntime::spawn(AudioRuntimeConfig {
        output_device: args.audio_output_device.clone(),
        low_power: args.low_power,
    });
    let _ = audio
        .command_tx
        .send(AudioCommand::SetVolume(app.settings().volume));

    // Startup auto-play: reflect Connecting in the UI and ask audio to play.
    if let Some(command) = startup_play_command(app.settings(), args.no_auto_play) {
        if let Some(previous) = app.settings().previous_station.clone() {
            app.apply(Action::Audio(AudioEvent::Connecting {
                station: previous.id,
            }));
        }
        let _ = audio.command_tx.send(command);
    }

    let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
    let (response_tx, response_rx) = mpsc::channel::<SearchResponse>();
    let worker: JoinHandle<()> = thread::spawn(move || search_worker(request_rx, response_tx));

    let mut debounce = SearchDebounce::new(SEARCH_DEBOUNCE);

    // Optional startup search: focus the strip and schedule the query.
    if let Some(query) = args.search.clone() {
        app.apply(Action::SetFocus(FocusPane::Search));
        app.apply(Action::SetSearchQuery(query.clone()));
        if matches!(
            debounce.note_query(&query, Instant::now()),
            QueryChange::Scheduled
        ) {
            app.apply(Action::SetSearchStatus(SearchStatus::Loading));
        }
    }

    let runtime = Runtime {
        audio: &audio,
        request_tx: &request_tx,
        response_rx: &response_rx,
    };

    let mut guard = TerminalGuard::new()?;
    let loop_result = event_loop(
        &mut guard.terminal,
        &mut app,
        &runtime,
        &mut debounce,
        &mut persistence,
        args.low_power,
    );

    // Clean shutdown: stop audio, stop the worker, persist final state under the
    // run's persistence policy (the `--volume` override is discarded unless the
    // user changed volume). The terminal is restored when `guard` drops.
    let _ = audio.command_tx.send(AudioCommand::Shutdown);
    let _ = request_tx.send(SearchRequest::Shutdown);
    persistence.save(&app);
    let _ = worker.join();

    loop_result
}

/// Whether the event loop should keep running.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// The runtime I/O endpoints the event loop talks to: the audio runtime handle
/// and the search worker's request/response channels. Bundled so the event loop
/// keeps a small, readable signature.
struct Runtime<'a> {
    audio: &'a AudioHandle,
    request_tx: &'a Sender<SearchRequest>,
    response_rx: &'a Receiver<SearchResponse>,
}

/// The terminal event loop: render, fire due searches, handle input, and drain
/// audio/search events until the user quits.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    runtime: &Runtime,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
    low_power: bool,
) -> Result<()> {
    let poll_interval = if low_power {
        POLL_INTERVAL_LOW_POWER
    } else {
        POLL_INTERVAL
    };

    loop {
        terminal.draw(|frame| crate::ui::render(app, frame))?;

        if let Some((query, generation)) = debounce.take_due(Instant::now()) {
            let _ = runtime
                .request_tx
                .send(SearchRequest::Query { query, generation });
        }

        if event::poll(poll_interval)? {
            if let Event::Key(key) = event::read()? {
                if handle_key(key, app, runtime.audio, debounce, persistence) == Flow::Quit {
                    return Ok(());
                }
            }
        }

        while let Ok(audio_event) = runtime.audio.event_rx.try_recv() {
            apply_audio_event(app, audio_event, persistence);
        }

        while let Ok(response) = runtime.response_rx.try_recv() {
            apply_search_response(app, debounce, response);
        }
    }
}

/// Translate a key event into app actions and audio/search side effects.
fn handle_key(
    key: KeyEvent,
    app: &mut App,
    audio: &AudioHandle,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
) -> Flow {
    let searching = app.focus() == FocusPane::Search;
    let before = app.settings().clone();

    match map_key(key, searching) {
        KeyOutcome::Quit => return Flow::Quit,
        KeyOutcome::Ignore => {}
        KeyOutcome::FocusNext => app.apply(Action::FocusNext),
        KeyOutcome::FocusPrevious => app.apply(Action::FocusPrevious),
        KeyOutcome::SelectNext => app.apply(Action::SelectNext),
        KeyOutcome::SelectPrevious => app.apply(Action::SelectPrevious),
        KeyOutcome::SelectFirst => app.apply(Action::SelectFirst),
        KeyOutcome::SelectLast => app.apply(Action::SelectLast),
        KeyOutcome::Play => {
            app.apply(Action::PlaySelected);
            if let Some(station) = app.current_station().cloned() {
                let _ = audio.command_tx.send(AudioCommand::Play {
                    station: Box::new(station),
                    volume: app.settings().volume,
                });
            }
        }
        KeyOutcome::TogglePlayback => {
            app.apply(Action::TogglePlayback);
            match app.playback() {
                PlaybackState::Connecting => {
                    if let Some(station) = app.current_station().cloned() {
                        let _ = audio.command_tx.send(AudioCommand::Play {
                            station: Box::new(station),
                            volume: app.settings().volume,
                        });
                    }
                }
                PlaybackState::Stopped => {
                    let _ = audio.command_tx.send(AudioCommand::Stop);
                }
                _ => {}
            }
        }
        KeyOutcome::ToggleFavorite => app.apply(Action::ToggleFavorite),
        KeyOutcome::CycleTheme => app.apply(Action::CycleTheme),
        KeyOutcome::VolumeUp => {
            app.apply(Action::VolumeUp);
            persistence.mark_user_changed_volume();
            let _ = audio
                .command_tx
                .send(AudioCommand::SetVolume(app.settings().volume));
        }
        KeyOutcome::VolumeDown => {
            app.apply(Action::VolumeDown);
            persistence.mark_user_changed_volume();
            let _ = audio
                .command_tx
                .send(AudioCommand::SetVolume(app.settings().volume));
        }
        KeyOutcome::BeginSearch => app.apply(Action::SetFocus(FocusPane::Search)),
        KeyOutcome::SearchChar(c) => {
            let mut query = app.search_query().to_string();
            query.push(c);
            update_search(app, debounce, query);
        }
        KeyOutcome::SearchBackspace => {
            let mut query = app.search_query().to_string();
            query.pop();
            update_search(app, debounce, query);
        }
        KeyOutcome::ClearSearch => {
            debounce.note_query("", Instant::now());
            app.apply(Action::SetSearchQuery(String::new()));
            app.apply(Action::SetSearchStatus(SearchStatus::Idle));
            app.apply(Action::ClearSearch);
            app.apply(Action::SetFocus(FocusPane::Stations));
        }
    }

    if app.settings() != &before {
        persistence.save(app);
    }
    Flow::Continue
}

/// Update the live query text and (re)schedule or cancel the debounced search.
fn update_search(app: &mut App, debounce: &mut SearchDebounce, query: String) {
    app.apply(Action::SetSearchQuery(query.clone()));
    match debounce.note_query(&query, Instant::now()) {
        QueryChange::Scheduled => app.apply(Action::SetSearchStatus(SearchStatus::Loading)),
        QueryChange::Cleared => {
            app.apply(Action::SetSearchStatus(SearchStatus::Idle));
            app.apply(Action::ClearSearch);
        }
    }
}

/// Fold an audio runtime event into app state, persisting when it changed
/// settings (e.g. a newly playing station becomes the persisted previous one).
/// Visualizer frames are applied without a persistence check to avoid churn.
fn apply_audio_event(app: &mut App, event: AudioEvent, persistence: &Persistence) {
    if matches!(event, AudioEvent::Viz(_)) {
        app.apply(Action::Audio(event));
        return;
    }
    let before = app.settings().clone();
    app.apply(Action::Audio(event));
    if app.settings() != &before {
        persistence.save(app);
    }
}

/// Apply a search response, ignoring stale generations and mapping recoverable
/// errors to offline/error search status.
fn apply_search_response(app: &mut App, debounce: &SearchDebounce, response: SearchResponse) {
    if !debounce.is_current(response.generation) {
        return; // a newer keystroke superseded this search.
    }
    match response.result {
        Ok(results) => {
            app.apply(Action::SearchResults(results));
            app.apply(Action::SetSearchStatus(SearchStatus::Loaded {
                from_cache: response.from_cache,
            }));
            app.apply(Action::SetOffline(false));
        }
        Err(SearchError::Network(_)) => {
            app.apply(Action::SetOffline(true));
            app.apply(Action::SetSearchStatus(SearchStatus::Offline));
        }
        Err(SearchError::Decode(message)) => {
            app.apply(Action::SetSearchStatus(SearchStatus::Error(message)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // --- parse_args ------------------------------------------------------

    fn parse(args: &[&str]) -> Result<CliInvocation, CliError> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_args_defaults_to_an_empty_run() {
        assert_eq!(parse(&[]).unwrap(), CliInvocation::Run(CliArgs::default()));
    }

    #[test]
    fn parse_args_reads_all_flags_in_space_form() {
        let invocation = parse(&[
            "--theme",
            "neon",
            "--volume",
            "42",
            "--no-auto-play",
            "--audio-output-device",
            "Speakers",
            "--low-power",
            "--search",
            "lofi",
        ])
        .unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                theme: Some(ThemeName::Neon),
                volume: Some(VolumePercent::new(42).unwrap()),
                no_auto_play: true,
                audio_output_device: Some("Speakers".to_string()),
                low_power: true,
                search: Some("lofi".to_string()),
            })
        );
    }

    #[test]
    fn parse_args_accepts_equals_form_for_values() {
        let invocation = parse(&["--theme=crt", "--volume=10", "--search=jazz radio"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                theme: Some(ThemeName::Crt),
                volume: Some(VolumePercent::new(10).unwrap()),
                search: Some("jazz radio".to_string()),
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn parse_args_treats_first_positional_as_search() {
        let invocation = parse(&["ambient"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                search: Some("ambient".to_string()),
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn parse_args_handles_help_and_version() {
        assert_eq!(parse(&["--help"]).unwrap(), CliInvocation::Help);
        assert_eq!(parse(&["-h"]).unwrap(), CliInvocation::Help);
        assert_eq!(parse(&["--version"]).unwrap(), CliInvocation::Version);
        assert_eq!(parse(&["-V"]).unwrap(), CliInvocation::Version);
    }

    #[test]
    fn parse_args_rejects_unknown_flags_and_bad_values() {
        assert_eq!(
            parse(&["--nope"]).unwrap_err(),
            CliError::UnknownFlag("--nope".to_string())
        );
        assert!(matches!(
            parse(&["--volume", "999"]).unwrap_err(),
            CliError::InvalidVolume(_)
        ));
        assert!(matches!(
            parse(&["--theme", "solarized"]).unwrap_err(),
            CliError::InvalidTheme(_)
        ));
        assert!(matches!(
            parse(&["--volume"]).unwrap_err(),
            CliError::MissingValue(_)
        ));
    }

    // --- map_key ---------------------------------------------------------

    #[test]
    fn map_key_navigation_mode_bindings_match_the_spec() {
        assert_eq!(map_key(key(KeyCode::Tab), false), KeyOutcome::FocusNext);
        assert_eq!(
            map_key(key(KeyCode::BackTab), false),
            KeyOutcome::FocusPrevious
        );
        assert_eq!(
            map_key(key(KeyCode::Char('j')), false),
            KeyOutcome::SelectNext
        );
        assert_eq!(map_key(key(KeyCode::Down), false), KeyOutcome::SelectNext);
        assert_eq!(
            map_key(key(KeyCode::Char('k')), false),
            KeyOutcome::SelectPrevious
        );
        assert_eq!(map_key(key(KeyCode::Up), false), KeyOutcome::SelectPrevious);
        assert_eq!(map_key(key(KeyCode::Enter), false), KeyOutcome::Play);
        assert_eq!(
            map_key(key(KeyCode::Char(' ')), false),
            KeyOutcome::TogglePlayback
        );
        assert_eq!(
            map_key(key(KeyCode::Char('+')), false),
            KeyOutcome::VolumeUp
        );
        assert_eq!(
            map_key(key(KeyCode::Char('=')), false),
            KeyOutcome::VolumeUp
        );
        assert_eq!(
            map_key(key(KeyCode::Char('-')), false),
            KeyOutcome::VolumeDown
        );
        assert_eq!(
            map_key(key(KeyCode::Char('f')), false),
            KeyOutcome::ToggleFavorite
        );
        assert_eq!(
            map_key(key(KeyCode::Char('t')), false),
            KeyOutcome::CycleTheme
        );
        assert_eq!(
            map_key(key(KeyCode::Char('/')), false),
            KeyOutcome::BeginSearch
        );
        assert_eq!(map_key(key(KeyCode::Char('q')), false), KeyOutcome::Quit);
        assert_eq!(map_key(key(KeyCode::Esc), false), KeyOutcome::Quit);
    }

    #[test]
    fn map_key_search_mode_edits_query_instead_of_commands() {
        // Printable keys become query text, including ones that are commands in
        // navigation mode.
        assert_eq!(
            map_key(key(KeyCode::Char('q')), true),
            KeyOutcome::SearchChar('q')
        );
        assert_eq!(
            map_key(key(KeyCode::Char('j')), true),
            KeyOutcome::SearchChar('j')
        );
        assert_eq!(
            map_key(key(KeyCode::Backspace), true),
            KeyOutcome::SearchBackspace
        );
        assert_eq!(map_key(key(KeyCode::Esc), true), KeyOutcome::ClearSearch);
        // Navigation still works while searching.
        assert_eq!(map_key(key(KeyCode::Tab), true), KeyOutcome::FocusNext);
        assert_eq!(map_key(key(KeyCode::Down), true), KeyOutcome::SelectNext);
        assert_eq!(map_key(key(KeyCode::Enter), true), KeyOutcome::Play);
    }

    #[test]
    fn map_key_ctrl_c_always_quits_and_release_events_are_ignored() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_c, false), KeyOutcome::Quit);
        assert_eq!(map_key(ctrl_c, true), KeyOutcome::Quit);

        let release = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)
            .modifiers
            .is_empty()
            .then(|| {
                let mut e = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
                e.kind = KeyEventKind::Release;
                e
            })
            .unwrap();
        assert_eq!(map_key(release, false), KeyOutcome::Ignore);
    }

    // --- SearchDebounce --------------------------------------------------

    #[test]
    fn low_power_poll_keeps_search_fire_within_spec_ceiling() {
        assert!(SEARCH_DEBOUNCE + POLL_INTERVAL_LOW_POWER <= Duration::from_millis(500));
    }

    #[test]
    fn debounce_schedules_after_the_window_and_fires_once() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();
        assert_eq!(debounce.note_query("lofi", t0), QueryChange::Scheduled);

        // Not due before the window elapses.
        assert!(debounce.take_due(t0 + Duration::from_millis(200)).is_none());

        // Due after the window; fires exactly once.
        let due = debounce.take_due(t0 + Duration::from_millis(300));
        assert!(due.is_some());
        assert_eq!(due.unwrap().0.as_str(), "lofi");
        assert!(debounce.take_due(t0 + Duration::from_millis(400)).is_none());
    }

    #[test]
    fn debounce_empty_query_clears_pending() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();
        debounce.note_query("jazz", t0);
        assert_eq!(debounce.note_query("   ", t0), QueryChange::Cleared);
        assert!(debounce.take_due(t0 + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn debounce_generations_distinguish_fresh_from_stale_results() {
        let mut debounce = SearchDebounce::new(Duration::from_millis(300));
        let t0 = Instant::now();

        debounce.note_query("a", t0);
        let (_, first_gen) = debounce.take_due(t0 + Duration::from_millis(300)).unwrap();
        assert!(debounce.is_current(first_gen));

        // A newer keystroke supersedes the in-flight search.
        debounce.note_query("ab", t0 + Duration::from_millis(310));
        assert!(!debounce.is_current(first_gen));
        assert!(debounce.is_current(debounce.generation()));
    }

    // --- startup_play_command -------------------------------------------

    fn settings_with_previous() -> Settings {
        let raw = r#"{
            "volume": 55,
            "theme": "minimal",
            "previous_station": {
                "id": "demo", "name": "Demo",
                "url": "https://example.com/a.mp3",
                "homepage": null, "country": null, "language": null,
                "tags": [], "codec": "Mp3", "bitrate": null,
                "votes": null, "click_count": null, "source": "BuiltIn"
            },
            "favorites": []
        }"#;
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn startup_auto_plays_previous_station_at_persisted_volume() {
        let settings = settings_with_previous();
        match startup_play_command(&settings, false) {
            Some(AudioCommand::Play { station, volume }) => {
                assert_eq!(station.id.as_str(), "demo");
                assert_eq!(volume.get(), 55);
            }
            other => panic!("expected a Play command, got {other:?}"),
        }
    }

    #[test]
    fn startup_is_silent_with_no_auto_play_or_no_previous_station() {
        let settings = settings_with_previous();
        assert!(startup_play_command(&settings, true).is_none());
        assert!(startup_play_command(&Settings::default(), false).is_none());
    }

    // --- CLI volume override vs. persistence -----------------------------

    #[test]
    fn cli_volume_override_drives_runtime_startup_volume() {
        // Loaded/saved settings carry the persisted volume; --volume overrides it
        // for this run, so the runtime (audio SetVolume / auto-play) uses 50.
        let saved = Settings {
            volume: VolumePercent::new(60).unwrap(),
            ..Settings::default()
        };
        let args = CliArgs {
            volume: Some(VolumePercent::new(50).unwrap()),
            ..CliArgs::default()
        };
        let effective = apply_overrides(saved, &args);
        assert_eq!(
            effective.volume.get(),
            50,
            "runtime startup volume reflects the --volume override"
        );
    }

    #[test]
    fn clean_shutdown_without_user_change_keeps_saved_volume() {
        // Baseline (on-disk) volume is 60; the run uses the override 50. With no
        // user +/- during the run, persistence must write back the saved 60, not
        // the run override 50.
        let baseline = VolumePercent::new(60).unwrap();
        let persistence = Persistence::new(baseline);
        let current = Settings {
            volume: VolumePercent::new(50).unwrap(),
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(
            to_save.volume.get(),
            60,
            "shutdown must not persist the one-run volume override"
        );
    }

    #[test]
    fn user_volume_change_during_run_is_persisted() {
        // Once the user presses +/-, the changed volume is theirs to keep.
        let mut persistence = Persistence::new(VolumePercent::new(60).unwrap());
        persistence.mark_user_changed_volume();
        let current = Settings {
            volume: VolumePercent::new(80).unwrap(),
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(
            to_save.volume.get(),
            80,
            "a user volume change persists the new value"
        );
    }

    #[test]
    fn persistence_preserves_other_fields_including_theme_override() {
        // Non-volume state (favorites, previous station, theme override) always
        // persists, even when the volume override is being discarded.
        let persistence = Persistence::new(VolumePercent::new(60).unwrap());
        let current = Settings {
            volume: VolumePercent::new(50).unwrap(),
            theme: ThemeName::Neon,
            ..Settings::default()
        };
        let to_save = persistence.settings_to_save(&current);
        assert_eq!(to_save.volume.get(), 60, "volume override discarded");
        assert_eq!(
            to_save.theme,
            ThemeName::Neon,
            "theme override still persists"
        );
    }
}
