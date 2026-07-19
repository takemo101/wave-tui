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
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::app::{Action, App, FocusPane, SearchStatus};
use crate::audio::{AudioCommand, AudioEvent, AudioHandle, AudioRuntime, AudioRuntimeConfig};
use crate::catalog::Catalog;
use crate::herdr::{self, HerdrMonitor, MonitorEvent};
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
    --theme <name>                Theme for this run: minimal | neon | crt |
                                  solarized | midnight | sakura
    --volume <0-100>              Startup volume override
    --no-auto-play                Start silently even if a previous station exists
    --audio-output-device <name>  CPAL output device name
    --low-power                   Lower UI update cadence (audio unaffected)
    --no-agent-pulse              Disable the Herdr Agent Pulse integration for
                                  this run (never persisted)
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
    /// Disable the Herdr Agent Pulse integration for this run only; never
    /// persisted and never applied to settings.
    pub no_agent_pulse: bool,
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
                    "invalid --theme {raw:?} \
                     (expected minimal, neon, crt, solarized, midnight, or sakura)"
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
            "--no-agent-pulse" => parsed.no_agent_pulse = true,
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
    /// `Esc` in navigation mode: quit the normal UI, or back out of Signal View
    /// when that mode is active.
    ExitOrBack,
    /// `z`: toggle the Signal View display mode.
    ToggleSignalView,
    /// `a`: toggle the Agent Pulse overlay. The reducer keeps it a no-op
    /// while the integration is hidden or Signal View is active.
    ToggleAgentPulse,
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
    /// `v`: cycle the visualizer mode.
    CycleVisualizerMode,
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
            KeyCode::Char('q') => KeyOutcome::Quit,
            KeyCode::Esc => KeyOutcome::ExitOrBack,
            KeyCode::Char('z') => KeyOutcome::ToggleSignalView,
            KeyCode::Char('a') => KeyOutcome::ToggleAgentPulse,
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
            KeyCode::Char('v') => KeyOutcome::CycleVisualizerMode,
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

/// Whether the terminal should capture mouse events for this run.
///
/// Mouse capture exists only to feed `Event::Mouse` to the Agent Pulse
/// overlay's read-only click selection, and it changes terminal behavior
/// (native text selection needs Shift+drag while captured). So it follows
/// the monitor exactly: standalone, ineligible, and `--no-agent-pulse`
/// launches keep their pre-integration terminal behavior untouched.
fn mouse_capture_for(monitor: Option<&HerdrMonitor>) -> bool {
    monitor.is_some()
}

/// RAII guard owning the terminal in raw/alternate-screen mode.
///
/// Restoration runs in [`Drop`], so the terminal is restored on a normal quit,
/// on a recoverable error returned from the event loop, and on a panic.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Whether mouse capture was enabled and must be released on drop.
    mouse_capture: bool,
}

impl TerminalGuard {
    fn new(mouse_capture: bool) -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        let mut stdout = io::stdout();
        if mouse_capture {
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
                .context("entering alternate screen")?;
        } else {
            execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating terminal")?;
        Ok(Self {
            terminal,
            mouse_capture,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse_capture {
            let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
        }
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

// === Startup/shutdown splash =============================================

/// Number of frames to draw to cover a splash's duration at its frame interval.
///
/// Pure timing math so the splash loop budget is unit-testable without a
/// terminal. Always at least one frame; saturates at `u16::MAX`.
fn splash_frame_budget(timing: crate::ui::SplashTiming) -> u16 {
    let interval_ms = timing.frame_interval.as_millis().max(1);
    let duration_ms = timing.duration.as_millis();
    let frames = duration_ms.div_ceil(interval_ms).max(1);
    frames.min(u16::MAX as u128) as u16
}

/// Draw the quiet lifecycle splash for `kind` until its duration elapses or any
/// key is pressed.
///
/// Runs outside the main event loop (startup before it, shutdown after it), so
/// its key-to-skip handling never interferes with the app's key mappings. Only
/// key events skip; other terminal events are left for the main loop's own
/// polling and do not change app behavior.
fn run_splash(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: ThemeName,
    kind: crate::ui::SplashKind,
    low_power: bool,
) -> Result<()> {
    let timing = crate::ui::splash_timing(kind, low_power);
    let palette = theme.theme();
    let frames = splash_frame_budget(timing);

    for tick in 0..frames {
        terminal.draw(|frame| crate::ui::render_splash(kind, &palette, tick, frame))?;
        if event::poll(timing.frame_interval)? && matches!(event::read()?, Event::Key(_)) {
            break;
        }
    }

    Ok(())
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
    // Low-power visual policy: configured exactly once, before the first
    // audio event, so the App can freeze the first visual frame's geometry.
    app.configure_low_power_visuals(args.low_power);

    // Agent Pulse monitor: created only when the official Herdr plugin
    // environment is eligible and `--no-agent-pulse` is absent. `None` keeps
    // standalone behavior exact. The polling thread is joined on every exit
    // path: explicitly in the shutdown block on a normal return, and by
    // `HerdrMonitor`'s `Drop` on the early `?` returns below.
    let monitor = herdr::context_from_env(args.no_agent_pulse).map(herdr::spawn_monitor);

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
        monitor: monitor.as_ref(),
    };

    let mut guard = TerminalGuard::new(mouse_capture_for(monitor.as_ref()))?;

    // Startup splash: a quiet, skippable transition after entering the alternate
    // screen and before the main UI loop. It does not touch app/audio state.
    run_splash(
        &mut guard.terminal,
        app.settings().theme,
        crate::ui::SplashKind::Startup,
        args.low_power,
    )?;

    let loop_result = event_loop(
        &mut guard.terminal,
        &mut app,
        &runtime,
        &mut debounce,
        &mut persistence,
        args.low_power,
    );

    // Shutdown splash: only after a clean event-loop exit and before terminal
    // restore. Skipped if the loop returned an error (best-effort; never masks
    // the loop's result).
    if loop_result.is_ok() {
        let _ = run_splash(
            &mut guard.terminal,
            app.settings().theme,
            crate::ui::SplashKind::Shutdown,
            args.low_power,
        );
    }

    // Clean shutdown: stop audio, stop the worker, persist final state under the
    // run's persistence policy (the `--volume` override is discarded unless the
    // user changed volume). The terminal is restored when `guard` drops.
    let _ = audio.command_tx.send(AudioCommand::Shutdown);
    let _ = request_tx.send(SearchRequest::Shutdown);
    if let Some(monitor) = monitor {
        monitor.stop();
    }
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
    /// The optional Herdr Agent Pulse monitor; `None` for standalone,
    /// ineligible, or `--no-agent-pulse` launches.
    monitor: Option<&'a HerdrMonitor>,
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
        terminal.draw(|frame| crate::ui::render(app, low_power, frame))?;

        if let Some((query, generation)) = debounce.take_due(Instant::now()) {
            let _ = runtime
                .request_tx
                .send(SearchRequest::Query { query, generation });
        }

        if event::poll(poll_interval)? {
            match event::read()? {
                Event::Key(key)
                    if handle_key(key, app, runtime.audio, debounce, persistence) == Flow::Quit =>
                {
                    return Ok(());
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    handle_mouse(
                        mouse,
                        Rect::new(0, 0, size.width, size.height),
                        low_power,
                        app,
                    );
                }
                _ => {}
            }
        }

        while let Ok(audio_event) = runtime.audio.event_rx.try_recv() {
            apply_audio_event(app, audio_event, persistence);
        }

        while let Ok(response) = runtime.response_rx.try_recv() {
            apply_search_response(app, debounce, response);
        }

        if let Some(monitor) = runtime.monitor {
            while let Ok(event) = monitor.events().try_recv() {
                apply_monitor_event(app, event, Instant::now());
            }
            // Stale/unavailable thresholds advance every loop, so the
            // 15-second unavailable state occurs even when no monitor event
            // arrives (e.g. the socket stays silent).
            app.apply(Action::AgentTick {
                now: Instant::now(),
            });
        }
    }
}

/// Whether the Browse source rail currently holds focus, so list-navigation and
/// `Enter` act on the source picker instead of the station list.
fn browse_focused(app: &App) -> bool {
    app.focus() == FocusPane::Sections
}

/// Which list (if any) the focused pane navigates with list-navigation keys.
///
/// Only the Stations and Browse rail panes are navigable lists; the search strip
/// and Now Playing are not, so list-navigation keys must do nothing there rather
/// than leaking into the hidden station cursor.
enum NavTarget {
    Stations,
    Browse,
    None,
}

/// Resolve which list the current focus navigates.
fn nav_target(app: &App) -> NavTarget {
    match app.focus() {
        FocusPane::Stations => NavTarget::Stations,
        FocusPane::Sections => NavTarget::Browse,
        FocusPane::Search | FocusPane::NowPlaying => NavTarget::None,
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
    // Signal View gates input to a small allowed subset. Route it first, before
    // the normal focus-aware handling, so discovery/navigation keys are ignored
    // silently while the mode is active. Keys are mapped as navigation (the
    // search strip is hidden) so allowed controls work regardless of the
    // background focus that is preserved underneath Signal View.
    if app.is_signal_view() {
        return handle_signal_view_key(map_key(key, false), app, audio, persistence);
    }

    // The Kinetic Collage canvas gate is routed after Signal View
    // (which never shows Agent Pulse) and before the normal focus-aware
    // handling. Keys are mapped as navigation — the canvas only opens from
    // navigation mode and never moves focus into the search strip — so
    // canvas-local keys (tile selection, close) are consumed before station
    // navigation, while every unconsumed outcome falls through to the normal
    // handling below exactly once. That keeps the documented global player
    // controls (playback, volume, theme, favorite, visualizer) available with
    // their normal semantics and side effects, without recursive dispatch.
    let collage_open = app.is_agent_overlay_open();
    let searching = !collage_open && app.focus() == FocusPane::Search;
    let outcome = map_key(key, searching);
    if collage_open {
        if let Some(flow) = handle_collage_key(outcome.clone(), app) {
            return flow;
        }
    }

    let before = app.settings().clone();

    match outcome {
        KeyOutcome::Quit | KeyOutcome::ExitOrBack => return Flow::Quit,
        KeyOutcome::ToggleSignalView => app.apply(Action::ToggleSignalView),
        // The reducer keeps this a no-op for standalone/ineligible launches,
        // so `a` stays harmless when no monitor was ever created.
        KeyOutcome::ToggleAgentPulse => app.apply(Action::ToggleAgentOverlay),
        KeyOutcome::Ignore => {}
        KeyOutcome::FocusNext => app.apply(Action::FocusNext),
        KeyOutcome::FocusPrevious => app.apply(Action::FocusPrevious),
        // List-navigation keys act only on the focused navigable list: the Browse
        // source rail moves its source cursor, the station list moves its
        // selection, and non-list panes (search strip, Now Playing) ignore them so
        // navigation never leaks into the hidden station cursor. `map_key` stays
        // focus-agnostic; the focus-aware split lives here in the controller.
        KeyOutcome::SelectNext => match nav_target(app) {
            NavTarget::Stations => app.apply(Action::SelectNext),
            NavTarget::Browse => app.apply(Action::BrowseSelectNext),
            NavTarget::None => {}
        },
        KeyOutcome::SelectPrevious => match nav_target(app) {
            NavTarget::Stations => app.apply(Action::SelectPrevious),
            NavTarget::Browse => app.apply(Action::BrowseSelectPrevious),
            NavTarget::None => {}
        },
        KeyOutcome::SelectFirst => match nav_target(app) {
            NavTarget::Stations => app.apply(Action::SelectFirst),
            NavTarget::Browse => app.apply(Action::BrowseSelectFirst),
            NavTarget::None => {}
        },
        KeyOutcome::SelectLast => match nav_target(app) {
            NavTarget::Stations => app.apply(Action::SelectLast),
            NavTarget::Browse => app.apply(Action::BrowseSelectLast),
            NavTarget::None => {}
        },
        KeyOutcome::Play => {
            if browse_focused(app) {
                // Browse focused: Enter applies the selected source and hands
                // focus to Stations rather than starting playback.
                app.apply(Action::ApplyBrowseSelection);
            } else {
                app.apply(Action::PlaySelected);
                if let Some(station) = app.current_station().cloned() {
                    let _ = audio.command_tx.send(AudioCommand::Play {
                        station: Box::new(station),
                        volume: app.settings().volume,
                    });
                }
            }
        }
        // `Space` is the Stations transport toggle. It only acts while the station
        // list is focused; in other panes it must not move or toggle playback (in
        // the search strip `map_key` already routes Space to query text).
        KeyOutcome::TogglePlayback => {
            if app.focus() == FocusPane::Stations {
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
        }
        KeyOutcome::ToggleFavorite => app.apply(Action::ToggleFavorite),
        KeyOutcome::CycleTheme => app.apply(Action::CycleTheme),
        KeyOutcome::CycleVisualizerMode => app.apply(Action::CycleVisualizerMode),
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

/// Route a key while Signal View is active.
///
/// Only the spec's allowed subset acts: `z`/`Esc` leave the mode, `q` quits,
/// `Space` toggles playback, `+`/`-` adjust volume, `v`/`t` cycle visualizer and
/// theme, and `f` favorites the *current* station (not the hidden station-list
/// selection). Every other key — search, focus movement, station navigation, and
/// station selection — is ignored silently. Background search/list state is left
/// untouched.
fn handle_signal_view_key(
    outcome: KeyOutcome,
    app: &mut App,
    audio: &AudioHandle,
    persistence: &mut Persistence,
) -> Flow {
    let before = app.settings().clone();
    match outcome {
        KeyOutcome::Quit => return Flow::Quit,
        KeyOutcome::ExitOrBack | KeyOutcome::ToggleSignalView => {
            app.apply(Action::LeaveSignalView);
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
        // In Signal View `f` targets the current station shown on screen, not the
        // hidden station-list selection.
        KeyOutcome::ToggleFavorite => app.apply(Action::ToggleCurrentFavorite),
        KeyOutcome::CycleTheme => app.apply(Action::CycleTheme),
        KeyOutcome::CycleVisualizerMode => app.apply(Action::CycleVisualizerMode),
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
        // Disabled keys are silent no-ops: search, focus movement, station
        // navigation, station selection, and Agent Pulse do nothing while
        // Signal View is active.
        KeyOutcome::Ignore
        | KeyOutcome::ToggleAgentPulse
        | KeyOutcome::FocusNext
        | KeyOutcome::FocusPrevious
        | KeyOutcome::SelectNext
        | KeyOutcome::SelectPrevious
        | KeyOutcome::SelectFirst
        | KeyOutcome::SelectLast
        | KeyOutcome::Play
        | KeyOutcome::BeginSearch
        | KeyOutcome::SearchChar(_)
        | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch => {}
    }

    if app.settings() != &before {
        persistence.save(app);
    }
    Flow::Continue
}

/// Route a canvas-local key while the Kinetic Collage canvas is open,
/// or return `None` to delegate the outcome to the normal handling path.
///
/// Canvas-local: `Tab`/arrows (and their `j`/`k` synonyms) move the tile
/// selection, and `a`/`Esc` close the canvas; `q`/`Ctrl+C` still quit.
/// Station selection (`Enter`), list jumps, and search entry are consumed so
/// canvas input can never play a station, move station selection, or enter
/// the search strip. `z` is also consumed as a no-op: Single View never opens
/// over Agent Planets, while `z` outside the canvas keeps its documented
/// Signal View toggle. Every other outcome — playback, volume, theme,
/// favorite, visualizer — is deliberately not handled here: the caller runs
/// the existing non-canvas branch exactly once, so the documented global
/// player controls keep their normal semantics and side effects without
/// recursive dispatch.
fn handle_collage_key(outcome: KeyOutcome, app: &mut App) -> Option<Flow> {
    match outcome {
        KeyOutcome::Quit => Some(Flow::Quit),
        KeyOutcome::ToggleAgentPulse | KeyOutcome::ExitOrBack => {
            app.apply(Action::CloseAgentOverlay);
            Some(Flow::Continue)
        }
        KeyOutcome::FocusNext | KeyOutcome::SelectNext => {
            app.apply(Action::SelectNextAgent);
            Some(Flow::Continue)
        }
        KeyOutcome::FocusPrevious | KeyOutcome::SelectPrevious => {
            app.apply(Action::SelectPreviousAgent);
            Some(Flow::Continue)
        }
        // Consumed: the station list and search surfaces stay suppressed
        // behind the canvas.
        KeyOutcome::BeginSearch
        | KeyOutcome::SearchChar(_)
        | KeyOutcome::SearchBackspace
        | KeyOutcome::ClearSearch
        | KeyOutcome::SelectFirst
        | KeyOutcome::SelectLast
        | KeyOutcome::Play
        | KeyOutcome::ToggleSignalView => Some(Flow::Continue),
        _ => None,
    }
}

/// Fold a typed Herdr monitor event into app state at the current time.
///
/// Poll failures arrive here as recoverable reducer state (stale, then
/// unavailable), never as an event-loop error.
fn apply_monitor_event(app: &mut App, event: MonitorEvent, now: Instant) {
    match event {
        MonitorEvent::Snapshot(agents) => app.apply(Action::AgentSnapshot { agents, now }),
        MonitorEvent::Failed => app.apply(Action::AgentPollFailed { now }),
    }
}

/// Route a mouse event through the pure UI hit test.
///
/// Only actions returned by [`crate::ui::agent_pulse_hit_test`] are applied —
/// read-only Kinetic Collage tile selection by contract — so every click
/// outside the canvas keeps its current behavior: none. `low_power` is the
/// same controller flag the render call uses, so clicks resolve against the
/// tile geometry that was actually drawn.
fn handle_mouse(mouse: MouseEvent, area: Rect, low_power: bool, app: &mut App) {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    if let Some(action) =
        crate::ui::agent_pulse_hit_test(area, mouse.column, mouse.row, low_power, app)
    {
        app.apply(action);
    }
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
    use crate::app::ListSource;
    use crate::catalog::{Category, Section};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // --- focus-aware key dispatch (event-loop controller) ----------------

    /// A fake audio handle whose command channel can be drained to assert what
    /// playback side effects a key press produced, without a real device.
    fn fake_audio() -> (AudioHandle, Receiver<AudioCommand>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        (
            AudioHandle {
                command_tx,
                event_rx,
            },
            command_rx,
        )
    }

    /// Controller scaffolding: an app on the curated catalog plus the debounce
    /// and persistence the event loop threads through `handle_key`.
    fn controller() -> (App, SearchDebounce, Persistence) {
        (
            App::new(Settings::default(), Catalog::curated()),
            SearchDebounce::new(SEARCH_DEBOUNCE),
            Persistence::new(VolumePercent::new(50).unwrap()),
        )
    }

    #[test]
    fn sections_focus_routes_navigation_to_browse_not_stations() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.apply(Action::SetFocus(FocusPane::Sections));
        let station_before = app.selected_index();

        handle_key(
            key(KeyCode::Char('j')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.browse_selected(), 1, "j moves the Browse selection");
        assert_eq!(
            app.selected_index(),
            station_before,
            "station selection must not move while Browse is focused"
        );

        handle_key(
            key(KeyCode::Char('k')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            app.browse_selected(),
            0,
            "k moves the Browse selection back"
        );
    }

    #[test]
    fn sections_focus_enter_applies_source_and_hands_focus_without_playing() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.apply(Action::SetFocus(FocusPane::Sections));
        let rail = ListSource::browse_rail();
        let music_index = rail
            .iter()
            .position(|s| *s == ListSource::Section(Section::Music))
            .unwrap();
        app.apply(Action::SetBrowseSelection(music_index));

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert_eq!(app.active_source(), ListSource::Section(Section::Music));
        assert_eq!(app.focus(), FocusPane::Stations);
        // Applying a Browse source must not start playback.
        assert!(
            cmd_rx.try_recv().is_err(),
            "no audio command should be sent when applying a Browse source"
        );
        assert!(app.current_station().is_none());
    }

    #[test]
    fn stations_focus_navigation_and_enter_playback_are_unchanged() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        // Default focus is Stations.
        assert_eq!(app.focus(), FocusPane::Stations);

        handle_key(
            key(KeyCode::Char('j')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.selected_index(), 1, "j moves the station selection");
        assert_eq!(app.browse_selected(), 0, "Browse selection stays put");

        let expected = app.selected_station().unwrap().id.clone();
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.current_station().unwrap().id, expected);
        match cmd_rx.try_recv() {
            Ok(AudioCommand::Play { station, .. }) => assert_eq!(station.id, expected),
            other => panic!("expected a Play command for the selected station, got {other:?}"),
        }
    }

    #[test]
    fn clearing_search_in_the_event_loop_restores_previous_source() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        // A non-search source is active, then a search lands (as a response would
        // deliver it), so the active Browse source is preserved over the search
        // population.
        app.apply(Action::ShowSection(Section::Music));
        app.apply(Action::SetFocus(FocusPane::Search));
        app.apply(Action::SearchResults(SearchResults::empty()));
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));

        // Esc while the search strip is focused clears search end-to-end and
        // keeps the Browse source, handing focus to Stations.
        handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.active_source(), ListSource::Section(Section::Music));
        assert_eq!(app.focus(), FocusPane::Stations);
        assert_eq!(app.search_query(), "");
    }

    #[test]
    fn clearing_search_before_results_land_keeps_the_current_source() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        // A non-default source is active; the user opens search but no results
        // arrive before they back out with Esc.
        app.apply(Action::ShowCategory(Category::Lofi));
        let lofi_top = app.selected_station().unwrap().id.clone();

        handle_key(
            key(KeyCode::Char('/')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.focus(), FocusPane::Search);

        handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        // The original source and its list stay put; focus returns to Stations.
        assert_eq!(app.active_source(), ListSource::Category(Category::Lofi));
        assert_eq!(app.selected_station().unwrap().id, lofi_top);
        assert_eq!(app.focus(), FocusPane::Stations);
    }

    #[test]
    fn sections_focus_does_not_treat_category_jump_keys_as_station_jumps() {
        // Home/End (and g/G) move the Browse cursor, not the hidden station list,
        // while the Browse rail is focused.
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.apply(Action::SetFocus(FocusPane::Sections));
        let station_before = app.selected_index();

        handle_key(
            key(KeyCode::End),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            app.browse_selected(),
            ListSource::browse_rail().len() - 1,
            "End jumps the Browse cursor to the last source"
        );
        assert_eq!(app.selected_index(), station_before);
        let _ = Category::Lofi; // Category is exercised by the reducer tests.
    }

    #[test]
    fn search_focus_navigation_moves_neither_station_nor_browse_selection() {
        // Up/Down/Home/End while the search strip is focused must not leak into
        // the station list (or the Browse rail); search keeps its focus.
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.apply(Action::SetFocus(FocusPane::Search));
        let station_before = app.selected_index();
        let browse_before = app.browse_selected();

        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Home, KeyCode::End] {
            handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
        }

        assert_eq!(
            app.selected_index(),
            station_before,
            "search focus must not move station selection"
        );
        assert_eq!(
            app.browse_selected(),
            browse_before,
            "search focus must not move the Browse selection"
        );
        assert_eq!(
            app.focus(),
            FocusPane::Search,
            "list-navigation keys keep search focus"
        );
    }

    #[test]
    fn now_playing_focus_navigation_does_not_move_station_selection() {
        // j/k/arrows/End while Now Playing is focused must not move the station
        // cursor; Now Playing is not a navigable list.
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.apply(Action::SetFocus(FocusPane::NowPlaying));
        let before = app.selected_index();

        for code in [KeyCode::Char('j'), KeyCode::Down, KeyCode::End] {
            handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
        }

        assert_eq!(
            app.selected_index(),
            before,
            "Now Playing focus must not move station selection"
        );
    }

    #[test]
    fn now_playing_focus_space_does_not_toggle_playback() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        // Play a station so there is a current station and Connecting playback.
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Connecting);
        let _ = cmd_rx.try_recv(); // drain the Play command from Enter.

        app.apply(Action::SetFocus(FocusPane::NowPlaying));
        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert_eq!(
            app.playback(),
            &PlaybackState::Connecting,
            "Space outside Stations must not toggle playback"
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "no audio command should be sent for Space outside Stations"
        );
    }

    #[test]
    fn sections_focus_space_does_not_toggle_playback() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Connecting);
        let _ = cmd_rx.try_recv(); // drain the Play command from Enter.

        app.apply(Action::SetFocus(FocusPane::Sections));
        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert_eq!(
            app.playback(),
            &PlaybackState::Connecting,
            "Space while Browse is focused must not toggle playback"
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "no audio command should be sent for Space while Browse is focused"
        );
    }

    #[test]
    fn stations_focus_space_still_toggles_playback() {
        // Guard the unchanged Stations behavior: Space stops the connecting/playing
        // current station and issues the Stop command.
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Connecting);
        let _ = cmd_rx.try_recv(); // drain the Play command from Enter.

        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Stopped);
        match cmd_rx.try_recv() {
            Ok(AudioCommand::Stop) => {}
            other => panic!("expected a Stop command for Space in Stations, got {other:?}"),
        }
    }

    #[test]
    fn v_key_cycles_visualizer_mode_through_the_controller() {
        use crate::model::VisualizerMode;
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);

        handle_key(
            key(KeyCode::Char('v')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);
        assert_eq!(app.settings().visualizer, VisualizerMode::PeakDots);
    }

    // --- Signal View key gating ------------------------------------------

    #[test]
    fn map_key_maps_z_to_signal_view_toggle_in_navigation_mode() {
        assert_eq!(
            map_key(key(KeyCode::Char('z')), false),
            KeyOutcome::ToggleSignalView
        );
        // While the search strip is focused, `z` is ordinary query text.
        assert_eq!(
            map_key(key(KeyCode::Char('z')), true),
            KeyOutcome::SearchChar('z')
        );
    }

    #[test]
    fn signal_view_z_enters_from_normal_and_exits_when_active() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        assert!(!app.is_signal_view());

        let flow = handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Continue);
        assert!(
            app.is_signal_view(),
            "z enters Signal View from normal mode"
        );

        let flow = handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Continue);
        assert!(!app.is_signal_view(), "z exits Signal View while active");
    }

    #[test]
    fn signal_view_escape_returns_to_normal_instead_of_quitting() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_signal_view());

        let flow = handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            flow,
            Flow::Continue,
            "Esc must not quit while in Signal View"
        );
        assert!(!app.is_signal_view(), "Esc leaves Signal View");
    }

    #[test]
    fn signal_view_q_still_quits() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_signal_view());

        let flow = handle_key(
            key(KeyCode::Char('q')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Quit, "q quits even while in Signal View");
    }

    #[test]
    fn signal_view_ignores_search_and_navigation_keys_silently() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        let selected_before = app.selected_index();
        let browse_before = app.browse_selected();
        let focus_before = app.focus();
        let source_before = app.active_source();
        let playback_before = app.playback().clone();

        for code in [
            KeyCode::Char('/'),
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Enter,
        ] {
            let flow = handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
            assert_eq!(flow, Flow::Continue, "{code:?} must be a silent no-op");
        }

        assert!(
            app.is_signal_view(),
            "disabled keys do not leave Signal View"
        );
        assert_eq!(app.selected_index(), selected_before);
        assert_eq!(app.browse_selected(), browse_before);
        assert_eq!(app.focus(), focus_before);
        assert_eq!(app.active_source(), source_before);
        assert_eq!(app.playback(), &playback_before);
        assert!(app.search_query().is_empty());
    }

    #[test]
    fn signal_view_allows_playback_volume_theme_and_visualizer_keys() {
        use crate::model::VisualizerMode;
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        // Play a station so Signal View has a current station and Connecting state.
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Connecting);
        let _ = cmd_rx.try_recv(); // drain the Play command from Enter.

        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        // Space toggles playback for the current station while Signal View is active.
        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.playback(), &PlaybackState::Stopped);
        match cmd_rx.try_recv() {
            Ok(AudioCommand::Stop) => {}
            other => panic!("expected a Stop command for Space in Signal View, got {other:?}"),
        }

        // `v` cycles the visualizer mode.
        assert_eq!(app.visualizer_mode(), VisualizerMode::SpectrumStack);
        handle_key(
            key(KeyCode::Char('v')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.visualizer_mode(), VisualizerMode::PeakDots);

        // `t` cycles the theme.
        let theme_before = app.settings().theme;
        handle_key(
            key(KeyCode::Char('t')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_ne!(app.settings().theme, theme_before, "t cycles theme");

        // `+` raises volume and emits a SetVolume command.
        handle_key(
            key(KeyCode::Char('+')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        match cmd_rx.try_recv() {
            Ok(AudioCommand::SetVolume(_)) => {}
            other => panic!("expected a SetVolume command for + in Signal View, got {other:?}"),
        }

        assert!(
            app.is_signal_view(),
            "allowed keys do not leave Signal View"
        );
    }

    #[test]
    fn signal_view_f_favorites_current_station_not_hidden_selection() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        // Play the first station, then move the hidden selection away from it.
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        handle_key(
            key(KeyCode::Char('j')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let current = app.current_station().cloned().expect("current station");
        let hidden = app.selected_station().cloned().expect("selected station");
        assert_ne!(current.id, hidden.id, "current and selection must differ");

        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        handle_key(
            key(KeyCode::Char('f')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert!(app.is_favorite(&current), "f favorites the current station");
        assert!(
            !app.is_favorite(&hidden),
            "f must not favorite the hidden station-list selection"
        );
        assert!(app.current_station_is_favorite());
    }

    // --- Agent Pulse: flag, key routing, monitor events, and mouse --------

    use crate::app::AgentPulseConnection;
    use crate::herdr::{AgentId, AgentSnapshot, AgentStatus, MonitorEvent};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    fn pulse_agent(pane: &str) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new("ws", pane),
            // Distinct names keep the reducer's name sort stable.
            name: Some(pane.to_string()),
            status: AgentStatus::Working,
        }
    }

    /// Bring the integration to life the way an eligible plugin launch would:
    /// a successful snapshot moves the reducer out of `Hidden`.
    fn connect_agent_pulse(app: &mut App, panes: &[&str]) {
        app.apply(Action::AgentSnapshot {
            agents: panes.iter().map(|pane| pulse_agent(pane)).collect(),
            now: Instant::now(),
        });
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn parse_args_reads_no_agent_pulse() {
        let invocation = parse(&["--no-agent-pulse"]).unwrap();
        assert_eq!(
            invocation,
            CliInvocation::Run(CliArgs {
                no_agent_pulse: true,
                ..CliArgs::default()
            })
        );
    }

    #[test]
    fn usage_documents_no_agent_pulse() {
        assert!(
            USAGE.contains("--no-agent-pulse"),
            "help text must document --no-agent-pulse"
        );
    }

    #[test]
    fn no_agent_pulse_leaves_settings_untouched() {
        // The flag is a one-run disable: it must not flow into the in-memory
        // settings (and therefore never into what persistence writes back).
        let saved = Settings::default();
        let args = CliArgs {
            no_agent_pulse: true,
            ..CliArgs::default()
        };
        assert_eq!(apply_overrides(saved.clone(), &args), saved);
    }

    #[test]
    fn map_key_maps_a_to_agent_pulse_toggle_in_navigation_mode() {
        assert_eq!(
            map_key(key(KeyCode::Char('a')), false),
            KeyOutcome::ToggleAgentPulse
        );
        // While the search strip is focused, `a` stays ordinary query text.
        assert_eq!(
            map_key(key(KeyCode::Char('a')), true),
            KeyOutcome::SearchChar('a')
        );
    }

    #[test]
    fn a_is_harmless_while_agent_pulse_is_hidden() {
        // Standalone/ineligible launches never leave Hidden, so `a` must not
        // open the overlay or disturb any existing state.
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        let settings_before = app.settings().clone();

        let flow = handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert_eq!(flow, Flow::Continue);
        assert!(!app.is_agent_overlay_open(), "hidden pulse keeps a inert");
        assert_eq!(app.focus(), FocusPane::Stations);
        assert_eq!(app.settings(), &settings_before);
        assert!(cmd_rx.try_recv().is_err(), "a must not touch audio");
    }

    #[test]
    fn a_toggles_the_agent_overlay_when_the_integration_is_live() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        assert!(!app.is_agent_overlay_open());

        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_agent_overlay_open(), "a opens the overlay");

        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(!app.is_agent_overlay_open(), "a closes the overlay again");
    }

    #[test]
    fn signal_view_ignores_the_agent_pulse_toggle() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);

        handle_key(
            key(KeyCode::Char('z')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_signal_view());

        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(
            !app.is_agent_overlay_open(),
            "Signal View must not open Agent Pulse"
        );
        assert!(app.is_signal_view(), "a must not leave Signal View");
    }

    #[test]
    fn agent_overlay_esc_closes_without_quitting_and_q_still_quits() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        let focus_before = app.focus();

        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let flow = handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Continue, "Esc closes the overlay, not the app");
        assert!(!app.is_agent_overlay_open());
        assert_eq!(app.focus(), focus_before, "closing preserves focus");

        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let flow = handle_key(
            key(KeyCode::Char('q')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Quit, "q quits even while the overlay is open");
    }

    #[test]
    fn agent_overlay_routes_navigation_to_agents_not_stations() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let station_before = app.selected_index();
        let browse_before = app.browse_selected();
        let focus_before = app.focus();

        handle_key(
            key(KeyCode::Tab),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            app.selected_agent().and_then(|agent| agent.name.as_deref()),
            Some("alpha"),
            "Tab selects the first agent"
        );

        handle_key(
            key(KeyCode::Down),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            app.selected_agent().and_then(|agent| agent.name.as_deref()),
            Some("beta"),
            "Down moves the agent selection"
        );

        handle_key(
            key(KeyCode::Up),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            app.selected_agent().and_then(|agent| agent.name.as_deref()),
            Some("alpha"),
            "Up moves the agent selection back"
        );

        assert_eq!(
            app.selected_index(),
            station_before,
            "overlay navigation must not move station selection"
        );
        assert_eq!(app.browse_selected(), browse_before);
        assert_eq!(
            app.focus(),
            focus_before,
            "Tab is consumed before focus movement"
        );
    }

    #[test]
    fn agent_overlay_enter_cannot_play_a_station() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert!(app.current_station().is_none(), "Enter must not play");
        assert_eq!(app.playback(), &PlaybackState::Stopped);
        assert!(
            cmd_rx.try_recv().is_err(),
            "no audio command may leave the overlay"
        );
        assert!(app.is_agent_overlay_open(), "Enter keeps the overlay open");
    }

    #[test]
    fn collage_a_and_escape_toggle_without_changing_station_focus() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        let focus = app.focus();

        let flow = handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Continue);
        assert!(app.is_agent_overlay_open());
        assert_eq!(app.focus(), focus);

        let flow = handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(flow, Flow::Continue);
        assert!(!app.is_agent_overlay_open());
        assert_eq!(app.focus(), focus);
    }

    #[test]
    fn collage_tile_selection_does_not_move_station_selection() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        let station = app.selected_index();
        app.apply(Action::ToggleAgentOverlay);

        handle_key(
            key(KeyCode::Tab),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.selected_agent().is_some());
        assert_eq!(app.selected_index(), station);
    }

    #[test]
    fn collage_stale_keys_cannot_change_the_selection_but_still_close() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        app.apply(Action::ToggleAgentOverlay);
        let mut press = |app: &mut App, code| {
            handle_key(key(code), app, &audio, &mut debounce, &mut persistence)
        };

        press(&mut app, KeyCode::Tab);
        let selected = app.selected_agent().map(|view| view.id.clone());
        assert!(selected.is_some(), "connected selection works");

        apply_monitor_event(&mut app, MonitorEvent::Failed, Instant::now());
        assert_eq!(app.agent_pulse_connection(), AgentPulseConnection::Stale);

        // Stale: selection keys are inert, matching the mouse hit-test gate.
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected_agent().map(|view| view.id.clone()), selected);

        // `Esc` still closes and `a` still reopens the canvas while stale.
        press(&mut app, KeyCode::Esc);
        assert!(!app.is_agent_overlay_open());
        press(&mut app, KeyCode::Char('a'));
        assert!(app.is_agent_overlay_open());
    }

    /// Drive every documented global player shortcut through `handle_key`
    /// while the Kinetic Collage canvas is open, asserting each keeps its
    /// normal semantics and side effects: the canvas consumes none of them.
    fn assert_global_shortcuts_work(app: &mut App) {
        let (audio, cmd_rx) = fake_audio();
        let (_, mut debounce, mut persistence) = controller();
        let mut press = |app: &mut App, code| {
            handle_key(key(code), app, &audio, &mut debounce, &mut persistence)
        };

        // Theme.
        let theme_before = app.settings().theme;
        press(app, KeyCode::Char('t'));
        assert_ne!(app.settings().theme, theme_before, "t cycles the theme");

        // Visualizer mode.
        let visualizer_before = app.settings().visualizer;
        press(app, KeyCode::Char('v'));
        assert_ne!(
            app.settings().visualizer,
            visualizer_before,
            "v cycles the visualizer"
        );

        // Volume, including the audio side effect.
        let volume_before = app.settings().volume;
        press(app, KeyCode::Char('+'));
        assert_ne!(app.settings().volume, volume_before, "+ raises the volume");
        assert!(matches!(cmd_rx.try_recv(), Ok(AudioCommand::SetVolume(_))));

        // Favorite keeps its normal semantics (the station-list selection).
        let favorites_before = app.settings().favorites.len();
        press(app, KeyCode::Char('f'));
        assert_ne!(
            app.settings().favorites.len(),
            favorites_before,
            "f toggles the selected favorite"
        );

        // The Space transport toggle keeps its normal semantics and audio
        // side effects: it stops the connecting station, then reconnects it.
        press(app, KeyCode::Char(' '));
        assert_eq!(app.playback(), &PlaybackState::Stopped);
        assert!(matches!(cmd_rx.try_recv(), Ok(AudioCommand::Stop)));
        press(app, KeyCode::Char(' '));
        assert!(
            matches!(cmd_rx.try_recv(), Ok(AudioCommand::Play { .. })),
            "Space reconnects the current station from the canvas"
        );

        assert!(
            app.is_agent_overlay_open(),
            "the canvas stays open throughout"
        );
    }

    /// Press the station-list, search, and jump keys while the Kinetic
    /// Collage canvas is open and assert the canvas consumes them: station
    /// selection, focus, query, playback, and the audio channel all stay
    /// untouched and the canvas stays open.
    fn assert_search_and_station_navigation_are_inert(app: &mut App) {
        let (audio, cmd_rx) = fake_audio();
        let (_, mut debounce, mut persistence) = controller();
        let station_before = app.selected_index();
        let focus_before = app.focus();
        let query_before = app.search_query().to_string();
        let playback_before = app.playback().clone();
        let playing_before = app.current_station().map(|station| station.id.clone());

        for code in [
            KeyCode::Enter,
            KeyCode::Char('/'),
            KeyCode::Home,
            KeyCode::End,
        ] {
            let flow = handle_key(key(code), app, &audio, &mut debounce, &mut persistence);
            assert_eq!(flow, Flow::Continue, "{code:?} is consumed by the canvas");
        }

        assert_eq!(app.selected_index(), station_before);
        assert_eq!(app.focus(), focus_before, "search focus is never entered");
        assert_eq!(app.search_query(), query_before);
        assert_eq!(app.playback(), &playback_before, "Enter cannot play");
        assert_eq!(
            app.current_station().map(|station| station.id.clone()),
            playing_before
        );
        assert!(cmd_rx.try_recv().is_err(), "no audio command may be sent");
        assert!(app.is_agent_overlay_open());
    }

    #[test]
    fn collage_keeps_global_shortcuts_and_suppresses_search_navigation() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);

        // Start playback from the normal surface, then open the canvas.
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(matches!(cmd_rx.try_recv(), Ok(AudioCommand::Play { .. })));
        app.apply(Action::ToggleAgentOverlay);

        assert_global_shortcuts_work(&mut app);
        assert_search_and_station_navigation_are_inert(&mut app);
    }

    #[test]
    fn z_is_consumed_in_agent_planets_but_toggles_signal_view_elsewhere() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut canvas, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut canvas, &["alpha"]);
        canvas.apply(Action::ToggleAgentOverlay);

        // `z` is consumed while the canvas is open: Single View never opens
        // over Agent Planets, and the canvas stays exactly as it was.
        handle_key(
            key(KeyCode::Char('z')),
            &mut canvas,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(
            !canvas.is_signal_view(),
            "z must not enter Signal View from the canvas"
        );
        assert!(canvas.is_agent_overlay_open(), "the canvas stays open");

        // Outside the canvas the same eligible app keeps the documented
        // Single View toggle.
        let (mut normal, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut normal, &["alpha"]);
        handle_key(
            key(KeyCode::Char('z')),
            &mut normal,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(
            normal.is_signal_view(),
            "z still toggles Signal View outside the canvas"
        );
    }

    /// The canvas-sized terminal area the collage mouse tests click within.
    fn canvas_area() -> Rect {
        Rect::new(0, 0, 100, 30)
    }

    /// A controller app whose Agent Pulse integration is live with two
    /// agents, the way an eligible plugin launch would produce it.
    fn connected_collage_app() -> App {
        let (mut app, _debounce, _persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        app
    }

    /// Scan the open Kinetic Collage canvas for the first cell the pure hit
    /// test resolves to a planet, so mouse tests target the actual drawn
    /// planet body/ring cells instead of assuming any particular shape.
    fn first_collage_tile_hit(app: &App) -> (u16, u16) {
        let area = canvas_area();
        for row in 0..area.height {
            for column in 0..area.width {
                if crate::ui::agent_pulse_hit_test(area, column, row, false, app).is_some() {
                    return (column, row);
                }
            }
        }
        panic!("an open canvas exposes tile targets");
    }

    #[test]
    fn collage_click_selects_a_tile_without_moving_station_selection() {
        let mut app = connected_collage_app();
        app.apply(Action::ToggleAgentOverlay);
        let station_index = app.selected_index();

        // Find a click the pure hit test maps, then route it through the
        // event-loop path.
        let (x, y) = first_collage_tile_hit(&app);

        handle_mouse(left_click(x, y), canvas_area(), false, &mut app);
        assert!(
            app.selected_agent().is_some(),
            "a tile click selects that agent"
        );
        assert_eq!(
            app.selected_index(),
            station_index,
            "a tile click must not move station selection"
        );
    }

    #[test]
    fn collage_low_power_mouse_path_uses_frozen_tile_geometry() {
        let mut app = connected_collage_app();
        app.configure_low_power_visuals(true);
        app.apply(Action::ToggleAgentOverlay);
        // The first audible frame — quiet enough to sit on base geometry —
        // becomes the App-captured frozen low-power geometry; the later loud
        // frame moves normal-power tiles off their base rectangles while low
        // power keeps drawing the capture.
        app.apply(Action::Audio(AudioEvent::Viz(
            crate::model::VizFrame::with_phase(
                vec![0.0; 16],
                0.1,
                Vec::<f32>::new(),
                crate::model::PhaseTrace::new([0.1], [0.1]),
                crate::model::PhaseTrace::empty(),
            ),
        )));
        app.apply(Action::Audio(AudioEvent::Viz(crate::model::VizFrame::new(
            vec![0.9; 16],
            0.9,
            Vec::<f32>::new(),
        ))));
        let area = canvas_area();

        // A discriminating cell: covered by an audio-moved planet's body or
        // ring, but empty in the frozen low-power collage.
        let moved_only = (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (column, row)))
            .find(|&(column, row)| {
                crate::ui::agent_pulse_hit_test(area, column, row, false, &app).is_some()
                    && crate::ui::agent_pulse_hit_test(area, column, row, true, &app).is_none()
            });
        let (x, y) = moved_only.expect("a loud frame moves some tile cell off its base rect");

        handle_mouse(left_click(x, y), area, true, &mut app);
        assert!(
            app.selected_agent().is_none(),
            "a low-power click resolves only the frozen drawn tiles"
        );
    }

    #[test]
    fn collage_key_routing_is_unchanged_by_low_power_visual_configuration() {
        // `run_app` configures the low-power visual policy once at startup;
        // the canvas keys must behave exactly as without the configuration.
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        app.configure_low_power_visuals(true);
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        let mut press = |app: &mut App, code| {
            handle_key(key(code), app, &audio, &mut debounce, &mut persistence)
        };

        press(&mut app, KeyCode::Char('a'));
        assert!(app.is_agent_overlay_open(), "a still opens the canvas");
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.selected_agent().and_then(|agent| agent.name.as_deref()),
            Some("alpha"),
            "Tab still selects the first agent"
        );
        press(&mut app, KeyCode::Esc);
        assert!(!app.is_agent_overlay_open(), "Esc still closes the canvas");
        press(&mut app, KeyCode::Char('a'));
        assert!(app.is_agent_overlay_open(), "a still reopens the canvas");
    }

    #[test]
    fn mouse_capture_follows_the_monitor() {
        // Standalone/ineligible/disabled launches have no monitor and must
        // keep exact pre-integration terminal behavior: no mouse capture.
        assert!(!mouse_capture_for(None));

        // Only a live monitor (an eligible plugin launch) turns capture on.
        // The socket path does not need to work; the monitor only needs to
        // exist, exactly as in run_app.
        let monitor = herdr::spawn_monitor(crate::herdr::HerdrContext {
            socket_path: "/nonexistent/wave-tui-cli-test.sock".into(),
        });
        assert!(mouse_capture_for(Some(&monitor)));
        monitor.stop();
    }

    #[test]
    fn monitor_events_reach_the_reducer_as_recoverable_state() {
        let (mut app, _debounce, _persistence) = controller();

        apply_monitor_event(
            &mut app,
            MonitorEvent::Snapshot(vec![pulse_agent("alpha")]),
            Instant::now(),
        );
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );

        apply_monitor_event(&mut app, MonitorEvent::Failed, Instant::now());
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Stale,
            "a poll failure is recoverable reducer state, not an error"
        );

        apply_monitor_event(
            &mut app,
            MonitorEvent::Snapshot(vec![pulse_agent("alpha")]),
            Instant::now(),
        );
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected,
            "a fresh snapshot recovers from stale"
        );
    }

    #[test]
    fn mouse_clicks_apply_only_hit_test_actions() {
        let (mut app, _debounce, _persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        let station_before = app.selected_index();
        let area = Rect::new(0, 0, 80, 24);

        // The hit test is the only source of mouse actions; a click that
        // lands on no tile cell changes nothing.
        handle_mouse(left_click(5, 5), area, false, &mut app);
        assert_eq!(app.selected_index(), station_before);
        assert!(app.selected_agent().is_none());
        assert_eq!(app.playback(), &PlaybackState::Stopped);
        assert!(app.is_agent_overlay_open());

        // Non-click mouse events (movement, scroll, release) are ignored.
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(moved, area, false, &mut app);
        assert_eq!(app.selected_index(), station_before);
        assert!(app.selected_agent().is_none());
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
            "--no-agent-pulse",
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
                no_agent_pulse: true,
                search: Some("lofi".to_string()),
            })
        );
    }

    #[test]
    fn parse_args_accepts_every_stable_theme_name() {
        let cases = [
            ("minimal", ThemeName::Minimal),
            ("neon", ThemeName::Neon),
            ("crt", ThemeName::Crt),
            ("solarized", ThemeName::Solarized),
            ("midnight", ThemeName::Midnight),
            ("sakura", ThemeName::Sakura),
        ];
        for (raw, expected) in cases {
            let invocation = parse(&["--theme", raw]).unwrap();
            assert_eq!(
                invocation,
                CliInvocation::Run(CliArgs {
                    theme: Some(expected),
                    ..CliArgs::default()
                }),
                "--theme {raw} should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn invalid_theme_error_lists_every_supported_name() {
        let message = CliError::InvalidTheme("aurora".to_string()).to_string();
        for name in ["minimal", "neon", "crt", "solarized", "midnight", "sakura"] {
            assert!(
                message.contains(name),
                "invalid-theme error {message:?} should mention {name}"
            );
        }
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
            parse(&["--theme", "aurora"]).unwrap_err(),
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
            map_key(key(KeyCode::Char('v')), false),
            KeyOutcome::CycleVisualizerMode
        );
        assert_eq!(
            map_key(key(KeyCode::Char('/')), false),
            KeyOutcome::BeginSearch
        );
        assert_eq!(map_key(key(KeyCode::Char('q')), false), KeyOutcome::Quit);
        // `Esc` in navigation mode is "exit or back": it quits the normal UI but
        // backs out of Signal View when that mode is active.
        assert_eq!(map_key(key(KeyCode::Esc), false), KeyOutcome::ExitOrBack);
        assert_eq!(
            map_key(key(KeyCode::Char('z')), false),
            KeyOutcome::ToggleSignalView
        );
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
            map_key(key(KeyCode::Char('v')), true),
            KeyOutcome::SearchChar('v')
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

    // --- splash timing ---------------------------------------------------

    #[test]
    fn splash_frame_budget_covers_duration_at_least_once() {
        let timing = crate::ui::SplashTiming {
            duration: Duration::from_millis(250),
            frame_interval: Duration::from_millis(100),
        };
        assert_eq!(splash_frame_budget(timing), 3);
    }

    #[test]
    fn low_power_splash_budget_is_no_larger_than_normal() {
        let normal = crate::ui::splash_timing(crate::ui::SplashKind::Startup, false);
        let low = crate::ui::splash_timing(crate::ui::SplashKind::Startup, true);
        assert!(splash_frame_budget(low) <= splash_frame_budget(normal));
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
