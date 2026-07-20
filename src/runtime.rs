//! Runtime composition root: everything a run owns outside argument parsing.
//!
//! [`run`] turns a parsed [`CliArgs`] into a live session — settings, catalog,
//! audio runtime, search worker, optional Herdr monitor, terminal, splash, and
//! event loop — and tears it back down in a fixed order. Argument parsing and
//! key *mapping* stay in [`crate::cli`], which this module depends on and which
//! never depends back on any terminal or adapter lifecycle detail.
//!
//! The children are private: `debounce` (search scheduling policy),
//! `persistence` (per-run save policy), `search_worker` (the blocking Radio
//! Browser thread), `terminal` (raw-mode ownership), `splash` (lifecycle
//! frames), `input` (key/mouse routing), and `event_loop` (render, poll, and
//! adapter draining).

mod debounce;
mod event_loop;
mod input;
mod persistence;
mod search_worker;
mod splash;
mod terminal;

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use anyhow::Result;

use crate::app::{Action, App, FocusPane, SearchStatus};
use crate::audio::{AudioCommand, AudioHandle, AudioRuntime, AudioRuntimeConfig};
use crate::catalog::Catalog;
use crate::cli::{self, apply_overrides, CliArgs, CliInvocation};
use crate::herdr::{self, HerdrMonitor};
use crate::model::PlaybackRequestId;
use crate::settings::{self, Settings};

use debounce::{QueryChange, SearchDebounce, SEARCH_DEBOUNCE};
use event_loop::{event_loop, Adapters};
use persistence::Persistence;
use search_worker::{search_worker, SearchRequest, SearchResponse};
use splash::run_splash;
use terminal::{mouse_capture_for, TerminalGuard};

/// Program entry point invoked by `main`.
///
/// Parsing, help, and version text belong to [`crate::cli`]; this function only
/// decides what the parsed invocation means for the process — print and exit,
/// or start a full run.
pub fn run() -> Result<()> {
    let invocation = cli::parse_args(std::env::args().skip(1))
        .map_err(|err| anyhow::anyhow!("{err}\n\n{}", cli::usage()))?;
    match invocation {
        CliInvocation::Help => {
            println!("{}", cli::usage());
            Ok(())
        }
        CliInvocation::Version => {
            println!("wave-tui {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliInvocation::Run(args) => run_app(args),
    }
}

/// The audio command to issue at startup, if any.
///
/// On a normal launch the previous station is auto-played at the persisted
/// volume; `--no-auto-play` or the absence of a previous station starts silently
/// (the spec's first-launch / failed-previous behavior).
fn startup_play_command(
    settings: &Settings,
    no_auto_play: bool,
    request: PlaybackRequestId,
) -> Option<AudioCommand> {
    if no_auto_play {
        return None;
    }
    settings
        .previous_station
        .clone()
        .map(|station| AudioCommand::Play {
            request,
            station: Box::new(station),
            volume: settings.volume,
        })
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
    //
    // The reducer is driven by the same resume intent `Space` uses (the app
    // starts Stopped with the previous station as `current`), so the startup
    // attempt records its request id exactly like every other play path. The
    // `Connecting` event echoed back by the runtime is then accepted rather
    // than rejected as unexpected.
    let startup_request = audio.next_playback_request();
    if let Some(command) = startup_play_command(app.settings(), args.no_auto_play, startup_request)
    {
        app.apply(Action::TogglePlayback(startup_request));
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

    let adapters = Adapters {
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
        &adapters,
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
    signal_shutdown(&audio, &request_tx, monitor);
    persistence.save(&mut app);
    let _ = worker.join();

    loop_result
}

/// Ask every runtime worker to stop, in the order a clean shutdown relies on:
/// audio first (so the device is released promptly), then the search worker,
/// then the Herdr monitor thread.
///
/// Only signalling happens here. The caller still owns persisting final state
/// and joining the search worker, so the documented shutdown ordering —
/// signal, save, join — stays visible at the call site.
fn signal_shutdown(
    audio: &AudioHandle,
    request_tx: &Sender<SearchRequest>,
    monitor: Option<HerdrMonitor>,
) {
    let _ = audio.command_tx.send(AudioCommand::Shutdown);
    let _ = request_tx.send(SearchRequest::Shutdown);
    if let Some(monitor) = monitor {
        monitor.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    use crate::audio::AudioEvent;

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
        let expected = crate::model::PlaybackRequestSeq::new().next_id();
        match startup_play_command(&settings, false, expected) {
            Some(AudioCommand::Play {
                request,
                station,
                volume,
            }) => {
                assert_eq!(station.id.as_str(), "demo");
                assert_eq!(volume.get(), 55);
                assert_eq!(
                    request, expected,
                    "startup auto-play is a normal, identified playback request"
                );
            }
            other => panic!("expected a Play command, got {other:?}"),
        }
    }

    #[test]
    fn startup_is_silent_with_no_auto_play_or_no_previous_station() {
        let settings = settings_with_previous();
        let request = crate::model::PlaybackRequestSeq::new().next_id();
        assert!(startup_play_command(&settings, true, request).is_none());
        assert!(startup_play_command(&Settings::default(), false, request).is_none());
    }

    /// A fake audio handle that also hands back the event sender, so a test can
    /// queue runtime events the drain pass is expected to fold into the app.
    fn fake_audio_with_events() -> (AudioHandle, Receiver<AudioCommand>, Sender<AudioEvent>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        (AudioHandle::new(command_tx, event_rx), command_rx, event_tx)
    }

    #[test]
    fn shutdown_signals_audio_then_the_search_worker() {
        // Characterizes worker shutdown signalling: both adapters are told to
        // stop, so a clean quit never leaves the audio device or the blocking
        // search thread running while the caller saves and joins.
        let (audio, cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();

        signal_shutdown(&audio, &request_tx, None);

        assert!(
            matches!(cmd_rx.try_recv(), Ok(AudioCommand::Shutdown)),
            "audio is asked to shut down first"
        );
        assert!(
            matches!(request_rx.try_recv(), Ok(SearchRequest::Shutdown)),
            "the search worker is asked to shut down"
        );
    }

    #[test]
    fn shutdown_signalling_survives_already_dead_workers() {
        // A worker thread that died early (panic, closed channel) must not turn
        // a clean quit into a failure: the sends are best-effort.
        let (audio, cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
        drop(cmd_rx);
        drop(request_rx);

        signal_shutdown(&audio, &request_tx, None);
    }
}
