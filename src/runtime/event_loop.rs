//! The terminal event loop and adapter event draining.
//!
//! Owns the render/poll cadence and the one pass that folds every queued audio,
//! search, and Herdr event into app state. The loop only dispatches actions and
//! observes the resulting state: the [`App`] reducer stays pure and rendering
//! stays read-only.

use std::io::Stdout;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::agent_host::AgentHostMonitor;
use crate::app::{Action, App};
use crate::audio::{AudioEvent, AudioHandle};

use super::debounce::SearchDebounce;
use super::input::{
    apply_agent_focus_result, apply_agent_rename_result, apply_monitor_event,
    handle_key_with_monitor, handle_mouse, Flow,
};
use super::persistence::Persistence;
use super::search_worker::{apply_search_response, SearchRequest, SearchResponse};

/// Event-loop poll cadence under normal operation.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Slower poll cadence when `--low-power` is set (audio is unaffected).
///
/// Kept at or below `500ms - SEARCH_DEBOUNCE` so debounced search still fires
/// inside the spec's 300–500ms responsiveness band even in low-power mode.
const POLL_INTERVAL_LOW_POWER: Duration = Duration::from_millis(150);

/// The adapter endpoints the event loop talks to: the audio runtime handle, the
/// search worker's request/response channels, and the optional agent host
/// monitor. Bundled so the event loop keeps a small, readable signature.
pub(super) struct Adapters<'a> {
    pub(super) audio: &'a AudioHandle,
    pub(super) request_tx: &'a Sender<SearchRequest>,
    pub(super) response_rx: &'a Receiver<SearchResponse>,
    /// The optional Agent Pulse host monitor; `None` for standalone,
    /// ineligible, or `--no-agent-pulse` launches.
    pub(super) monitor: Option<&'a AgentHostMonitor>,
}

/// The terminal event loop: render, fire due searches, handle input, and drain
/// audio/search events until the user quits.
pub(super) fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    adapters: &Adapters,
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
            let _ = adapters
                .request_tx
                .send(SearchRequest::Query { query, generation });
        }

        if event::poll(poll_interval)? {
            match event::read()? {
                Event::Key(key)
                    if handle_key_with_monitor(
                        key,
                        app,
                        adapters.audio,
                        debounce,
                        persistence,
                        adapters.monitor,
                    ) == Flow::Quit =>
                {
                    return Ok(());
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    handle_mouse(
                        mouse,
                        Rect::new(0, 0, size.width, size.height),
                        low_power,
                        Instant::now(),
                        app,
                    );
                }
                _ => {}
            }
        }

        drain_runtime_events(app, adapters, debounce, persistence);
    }
}

/// Fold every event queued by the adapters into app state, in one pass.
///
/// Ordering is part of the contract: all pending audio events first, then all
/// pending search responses, then — only when an agent host is live —
/// the monitor's snapshots, focus results, and rename results, followed by the
/// per-loop `AgentTick`. Every channel is drained to empty rather than one
/// event per loop, so a burst never lags behind rendering.
fn drain_runtime_events(
    app: &mut App,
    adapters: &Adapters,
    debounce: &SearchDebounce,
    persistence: &Persistence,
) {
    while let Ok(audio_event) = adapters.audio.event_rx.try_recv() {
        apply_audio_event(app, audio_event, persistence);
    }

    while let Ok(response) = adapters.response_rx.try_recv() {
        apply_search_response(app, debounce, response);
    }

    if let Some(monitor) = adapters.monitor {
        while let Ok(event) = monitor.events().try_recv() {
            apply_monitor_event(app, event, Instant::now());
        }
        while let Ok(result) = monitor.focus_events().try_recv() {
            apply_agent_focus_result(app, result, Instant::now());
        }
        while let Ok(completion) = monitor.rename_events().try_recv() {
            apply_agent_rename_result(app, completion, Instant::now());
        }
        // Stale/unavailable thresholds advance every loop, so the 15-second
        // unavailable state occurs even when no monitor event arrives (e.g.
        // the socket stays silent).
        app.apply(Action::AgentTick {
            now: Instant::now(),
        });
    }
}

/// Fold an audio runtime event into app state, persisting when it changed
/// settings (e.g. a newly playing station becomes the persisted previous one).
/// Visualizer frames are applied without a persistence check to avoid churn.
fn apply_audio_event(app: &mut App, event: AudioEvent, persistence: &Persistence) {
    if matches!(event, AudioEvent::Viz { .. }) {
        app.apply(Action::Audio(event));
        return;
    }
    let before = app.settings().clone();
    app.apply(Action::Audio(event));
    if app.settings() != &before {
        persistence.save(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    use crate::app::SearchStatus;
    use crate::audio::AudioCommand;
    use crate::catalog::Catalog;
    use crate::model::agent::AgentCapabilities;
    use crate::model::VolumePercent;
    use crate::search::SearchResults;
    use crate::settings::Settings;

    use super::super::debounce::SEARCH_DEBOUNCE;

    /// Controller scaffolding without a terminal: an app on the curated
    /// catalog plus the debounce and persistence the loop threads through.
    fn controller() -> (App, SearchDebounce, Persistence) {
        (
            App::new(Settings::default(), Catalog::curated()),
            SearchDebounce::new(SEARCH_DEBOUNCE),
            Persistence::new(VolumePercent::new(50).unwrap()),
        )
    }

    /// A fake audio handle that also hands back the event sender, so a test can
    /// queue runtime events the drain pass is expected to fold into the app.
    fn fake_audio_with_events() -> (AudioHandle, Receiver<AudioCommand>, Sender<AudioEvent>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        (AudioHandle::new(command_tx, event_rx), command_rx, event_tx)
    }

    #[test]
    fn one_drain_pass_empties_the_audio_and_search_channels() {
        // Characterizes the event-loop drain: every queued event is folded in a
        // single pass, so a burst of audio events or search responses can never
        // lag one-per-frame behind rendering.
        let (audio, _cmd_rx, event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (mut app, mut debounce, persistence) = controller();

        // Two audio events queue up while the loop was busy rendering. Both are
        // global (request-independent), so the second one winning proves the
        // whole backlog was folded rather than just its head.
        event_tx
            .send(AudioEvent::VolumeChanged(VolumePercent::new(30).unwrap()))
            .unwrap();
        event_tx
            .send(AudioEvent::VolumeChanged(VolumePercent::new(70).unwrap()))
            .unwrap();

        // A search response for the current generation queues behind them.
        debounce.note_query("jazz", Instant::now());
        let (_query, generation) = debounce
            .take_due(Instant::now() + SEARCH_DEBOUNCE)
            .expect("a non-empty query is scheduled");
        response_tx
            .send(SearchResponse {
                generation,
                result: Ok(SearchResults::empty()),
                from_cache: true,
            })
            .unwrap();

        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: None,
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert!(
            audio.event_rx.try_recv().is_err(),
            "the audio channel is drained to empty in one pass"
        );
        assert!(
            response_rx.try_recv().is_err(),
            "the search channel is drained to empty in one pass"
        );
        assert_eq!(
            app.settings().volume.get(),
            70,
            "the whole audio backlog is folded, so the newest event wins"
        );
        assert_eq!(
            app.search_status(),
            &SearchStatus::Loaded { from_cache: true },
            "the drained response updated search status"
        );
    }

    #[test]
    fn drained_search_responses_still_ignore_stale_generations() {
        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (mut app, mut debounce, persistence) = controller();

        debounce.note_query("jazz", Instant::now());
        let (_query, stale) = debounce
            .take_due(Instant::now() + SEARCH_DEBOUNCE)
            .expect("a non-empty query is scheduled");
        // A newer keystroke supersedes the in-flight generation.
        debounce.note_query("jazzy", Instant::now());
        response_tx
            .send(SearchResponse {
                generation: stale,
                result: Ok(SearchResults::empty()),
                from_cache: false,
            })
            .unwrap();

        let status_before = app.search_status().clone();
        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: None,
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert_eq!(
            app.search_status(),
            &status_before,
            "a superseded response must not land, even when drained in bulk"
        );
        assert!(
            !app.visible().is_empty(),
            "the stale (empty) result set must not replace the visible list"
        );
    }

    fn pulse_agent(pane: &str) -> crate::model::agent::AgentSnapshot {
        use crate::model::agent::{AgentDetails, AgentId, AgentSnapshot, AgentStatus};
        AgentSnapshot {
            id: AgentId::herdr("ws", pane),
            details: AgentDetails {
                name: Some(pane.to_string()),
                agent: None,
                activity: None,
            },
            status: AgentStatus::Working,
        }
    }

    #[test]
    fn host_events_drain_to_empty_in_one_pass_and_connect_the_app() {
        use crate::app::AgentPulseConnection;
        use crate::model::agent::{
            FocusResult, MonitorEvent, RenameCompletion, RenameRequestId, RenameResult,
        };

        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (mut app, debounce, persistence) = controller();
        let (monitor, control) = AgentHostMonitor::synthetic(AgentCapabilities::HERDR);
        control
            .events
            .send(MonitorEvent::Snapshot(vec![pulse_agent("alpha")]))
            .unwrap();
        control
            .focus_results
            .send(FocusResult::Unsupported)
            .unwrap();
        control
            .rename_results
            .send(RenameCompletion {
                request: RenameRequestId::FIRST,
                result: RenameResult::Missing,
            })
            .unwrap();

        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: Some(&monitor),
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert!(monitor.events().try_recv().is_err());
        assert!(monitor.focus_events().try_recv().is_err());
        assert!(monitor.rename_events().try_recv().is_err());
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        assert_eq!(
            app.agent_focus_notice(Instant::now()),
            Some("pane focus requires Herdr 0.7.0+"),
            "Herdr keeps its unsupported-version feedback"
        );
    }

    #[test]
    fn a_read_only_host_drains_without_any_herdr_version_notice() {
        use crate::model::agent::{
            FocusResult, MonitorEvent, RenameCompletion, RenameRequestId, RenameResult,
        };

        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (mut app, debounce, persistence) = controller();
        let (monitor, control) = AgentHostMonitor::synthetic_read_only();
        app.configure_agent_capabilities(monitor.capabilities());
        control
            .events
            .send(MonitorEvent::Snapshot(vec![pulse_agent("alpha")]))
            .unwrap();
        control
            .focus_results
            .send(FocusResult::Unsupported)
            .unwrap();
        control
            .rename_results
            .send(RenameCompletion {
                request: RenameRequestId::FIRST,
                result: RenameResult::Unsupported,
            })
            .unwrap();

        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: Some(&monitor),
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert_eq!(app.active_agents().len(), 1, "observation still works");
        assert_eq!(app.agent_focus_notice(Instant::now()), None);
        assert_eq!(app.agent_rename_notice(Instant::now()), None);
    }

    #[test]
    fn a_drained_late_rename_completion_cannot_settle_a_reopened_editor() {
        use crate::model::agent::{
            MonitorEvent, RenameCompletion, RenameRequest, RenameRequestId, RenameResult,
        };

        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (mut app, debounce, persistence) = controller();
        let (monitor, control) = AgentHostMonitor::synthetic(AgentCapabilities::HERDR);
        app.configure_agent_capabilities(monitor.capabilities());
        control
            .events
            .send(MonitorEvent::Snapshot(vec![pulse_agent("alpha")]))
            .unwrap();
        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: Some(&monitor),
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);

        // First editor submits, is cancelled, and a second editor opens.
        app.apply(Action::OpenAgentRename);
        let first = app.agent_rename_request().expect("can submit");
        assert!(monitor.rename_agent(RenameRequest {
            request: first.request,
            target: first.target.clone(),
            name: first.name.clone(),
        }));
        app.apply(Action::SubmitAgentRename);
        app.apply(Action::CloseAgentRename);
        app.apply(Action::OpenAgentRename);
        app.apply(Action::AppendAgentRename('!'));

        // The stale result and an unrelated future id both land in one drain.
        for (request, result) in [
            (first.request, RenameResult::Renamed),
            (
                RenameRequestId::FIRST.following().following(),
                RenameResult::Missing,
            ),
        ] {
            control
                .rename_results
                .send(RenameCompletion { request, result })
                .unwrap();
        }
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert!(monitor.rename_events().try_recv().is_err(), "fully drained");
        assert!(app.is_agent_rename_open(), "neither stale result lands");
        assert_eq!(app.agent_rename_input(), Some("alpha!"));
        assert!(!app.agent_rename_is_submitting());
        assert_eq!(app.agent_rename_notice(Instant::now()), None);
    }

    #[test]
    fn low_power_poll_keeps_search_fire_within_spec_ceiling() {
        assert!(SEARCH_DEBOUNCE + POLL_INTERVAL_LOW_POWER <= Duration::from_millis(500));
    }
}
