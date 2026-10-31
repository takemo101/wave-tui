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

    fn named_agent(
        id: crate::model::agent::AgentId,
        name: &str,
        status: crate::model::agent::AgentStatus,
    ) -> crate::model::agent::AgentSnapshot {
        crate::model::agent::AgentSnapshot {
            id,
            details: crate::model::agent::AgentDetails {
                name: Some(name.to_string()),
                agent: None,
                activity: None,
            },
            status,
        }
    }

    fn herdr_host_id(key: &str) -> crate::model::agent::AgentId {
        crate::model::agent::AgentId::herdr("ws", key)
    }

    fn core_host_id(key: &str) -> crate::model::agent::AgentId {
        crate::model::agent::AgentId::core(format!("core-addr-{key}"))
    }

    fn active_names(app: &App) -> Vec<String> {
        app.active_agents()
            .iter()
            .map(|view| view.name.clone().unwrap_or_default())
            .collect()
    }

    /// An app whose host last answered 20 s ago, with the stage open and the
    /// details modal on `alpha`: only a tick that runs *after* this pass's
    /// events can leave it connected with the modal still open.
    fn app_last_answered_long_ago(
        capabilities: AgentCapabilities,
        id: fn(&str) -> crate::model::agent::AgentId,
    ) -> App {
        use crate::model::agent::AgentStatus;

        let (mut app, _debounce, _persistence) = controller();
        app.configure_agent_capabilities(capabilities);
        let long_ago = Instant::now()
            .checked_sub(Duration::from_secs(20))
            .expect("monotonic clock has 20 s of history");
        app.apply(Action::AgentSnapshot {
            agents: vec![named_agent(id("p1"), "alpha", AgentStatus::Working)],
            now: long_ago,
        });
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        assert!(app.is_agent_details_open());
        app
    }

    /// Queue several events per host channel and drain once. Every channel
    /// must empty, and each channel's *last* event must win, in the order
    /// snapshot, focus, rename, tick.
    fn assert_ordered_multi_event_drain(
        capabilities: AgentCapabilities,
        id: fn(&str) -> crate::model::agent::AgentId,
    ) {
        use crate::app::AgentPulseConnection;
        use crate::model::agent::{
            AgentStatus, FocusResult, MonitorEvent, RenameCompletion, RenameRequestId, RenameResult,
        };

        let live = capabilities.rename;
        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (_, debounce, persistence) = controller();
        let (monitor, control) = AgentHostMonitor::synthetic(capabilities);
        let mut app = app_last_answered_long_ago(monitor.capabilities(), id);

        // A live host's editor is submitted before the drain; a read-only host
        // never opens one.
        app.apply(Action::OpenAgentRename);
        assert_eq!(app.is_agent_rename_open(), live);
        let submitted = app.agent_rename_request();
        if let Some(request) = submitted.clone() {
            assert!(monitor.rename_agent(request));
            app.apply(Action::SubmitAgentRename);
        }
        let submission = submitted.as_ref().map(|request| request.request);
        let settings_before = app.settings().clone();
        let search_before = app.search_status().clone();
        let selection_before = (app.selected_index(), app.playback().clone());

        control
            .events
            .send(MonitorEvent::Snapshot(vec![
                named_agent(id("p2"), "beta", AgentStatus::Idle),
                named_agent(id("p1"), "alpha", AgentStatus::Working),
            ]))
            .unwrap();
        control.events.send(MonitorEvent::Failed).unwrap();
        control
            .events
            .send(MonitorEvent::Snapshot(vec![
                named_agent(id("p3"), "gamma", AgentStatus::Done),
                named_agent(id("p1"), "alpha", AgentStatus::Working),
            ]))
            .unwrap();
        for result in [
            FocusResult::Unsupported,
            FocusResult::Unavailable,
            FocusResult::Missing,
        ] {
            control.focus_results.send(result).unwrap();
        }
        // The first completion settles the submission as a failure and keeps
        // the editor; the second names the same, now settled, submission and
        // must be ignored. Reversed, the rename would succeed and close it.
        let first = submission.unwrap_or(RenameRequestId::FIRST);
        for result in [RenameResult::Unavailable, RenameResult::Renamed] {
            control
                .rename_results
                .send(RenameCompletion {
                    request: first,
                    result,
                })
                .unwrap();
        }

        let adapters = Adapters {
            audio: &audio,
            request_tx: &request_tx,
            response_rx: &response_rx,
            monitor: Some(&monitor),
        };
        drain_runtime_events(&mut app, &adapters, &debounce, &persistence);

        assert!(monitor.events().try_recv().is_err(), "snapshots drained");
        assert!(monitor.focus_events().try_recv().is_err(), "focus drained");
        assert!(
            monitor.rename_events().try_recv().is_err(),
            "rename drained"
        );

        // Snapshots: the newest wins, and the failure between them recovered.
        assert_eq!(active_names(&app), ["alpha", "gamma"]);
        assert_eq!(
            app.agent_pulse_connection(),
            AgentPulseConnection::Connected
        );
        // Tick last: had it run first, a 20 s-old host would be Unavailable
        // and would have closed the modal before the snapshots recovered it.
        assert!(app.is_agent_details_open(), "tick ran after the events");

        let now = Instant::now();
        if live {
            assert_eq!(
                app.agent_focus_notice(now),
                Some("pane is no longer available"),
                "the last focus result wins"
            );
            assert!(app.is_agent_rename_open());
            assert!(!app.agent_rename_is_submitting());
            assert_eq!(app.agent_rename_input(), Some("alpha"));
            assert_eq!(
                app.agent_rename_notice(now),
                Some("rename unavailable · retrying"),
                "the first completion settled it; the later one was ignored"
            );
            assert_eq!(control.rename_request_ids(), vec![submission.unwrap()]);
        } else {
            assert_eq!(app.agent_focus_notice(now), None);
            assert!(!app.is_agent_rename_open());
            assert_eq!(app.agent_rename_notice(now), None);
            assert!(control.rename_request_ids().is_empty());
        }
        assert_eq!(control.focus_requests(), 0, "draining never dispatches");

        // Observation moved none of the radio-owned state.
        assert_eq!(app.settings(), &settings_before);
        assert_eq!(app.search_status(), &search_before);
        assert_eq!(
            (app.selected_index(), app.playback().clone()),
            selection_before
        );
    }

    #[test]
    fn a_herdr_host_drains_many_events_per_channel_in_snapshot_focus_rename_tick_order() {
        assert_ordered_multi_event_drain(AgentCapabilities::HERDR, herdr_host_id);
    }

    #[test]
    fn a_read_only_host_drains_many_events_per_channel_in_the_same_order() {
        assert_ordered_multi_event_drain(AgentCapabilities::READ_ONLY, core_host_id);
    }

    #[test]
    fn rename_completions_are_folded_after_the_snapshot_events_of_the_same_pass() {
        use crate::model::agent::{AgentStatus, MonitorEvent, RenameCompletion, RenameResult};

        let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
        let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
        let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
        let (_, debounce, persistence) = controller();
        let (monitor, control) = AgentHostMonitor::synthetic(AgentCapabilities::HERDR);
        let mut app = app_last_answered_long_ago(monitor.capabilities(), herdr_host_id);
        app.apply(Action::OpenAgentRename);
        let request = app.agent_rename_request().expect("editor can submit");
        let submission = request.request;
        assert!(monitor.rename_agent(request));
        app.apply(Action::SubmitAgentRename);

        // The pass ends with the host failing again, so by the time the
        // completion is folded the table is Stale. A completion applied first
        // would still see Connected and report the agent as gone instead.
        control
            .events
            .send(MonitorEvent::Snapshot(vec![named_agent(
                herdr_host_id("p1"),
                "alpha",
                AgentStatus::Working,
            )]))
            .unwrap();
        control.events.send(MonitorEvent::Failed).unwrap();
        control
            .rename_results
            .send(RenameCompletion {
                request: submission,
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

        assert_eq!(
            app.agent_pulse_connection(),
            crate::app::AgentPulseConnection::Stale
        );
        assert!(app.is_agent_rename_open(), "the editor and input survive");
        assert_eq!(
            app.agent_rename_notice(Instant::now()),
            Some("rename unavailable · retrying"),
            "settled against the stale table, not the pre-pass connected one"
        );
    }

    #[test]
    fn a_silent_live_host_still_ticks_to_unavailable_on_every_drain() {
        use crate::app::AgentPulseConnection;

        for (capabilities, id) in [
            (
                AgentCapabilities::HERDR,
                herdr_host_id as fn(&str) -> crate::model::agent::AgentId,
            ),
            (AgentCapabilities::READ_ONLY, core_host_id),
        ] {
            let (audio, _cmd_rx, _event_tx) = fake_audio_with_events();
            let (request_tx, _request_rx) = mpsc::channel::<SearchRequest>();
            let (_response_tx, response_rx) = mpsc::channel::<SearchResponse>();
            let (_, debounce, persistence) = controller();
            let (monitor, _control) = AgentHostMonitor::synthetic(capabilities);

            // No monitor: nothing ticks, so the old host stays "connected".
            let mut standalone = app_last_answered_long_ago(capabilities, id);
            let none = Adapters {
                audio: &audio,
                request_tx: &request_tx,
                response_rx: &response_rx,
                monitor: None,
            };
            drain_runtime_events(&mut standalone, &none, &debounce, &persistence);
            assert_eq!(
                standalone.agent_pulse_connection(),
                AgentPulseConnection::Connected
            );

            // A live but silent monitor: the tick alone crosses 15 s.
            let mut app = app_last_answered_long_ago(capabilities, id);
            let live = Adapters {
                audio: &audio,
                request_tx: &request_tx,
                response_rx: &response_rx,
                monitor: Some(&monitor),
            };
            drain_runtime_events(&mut app, &live, &debounce, &persistence);
            assert_eq!(
                app.agent_pulse_connection(),
                AgentPulseConnection::Unavailable
            );
            assert!(!app.is_agent_details_open(), "unavailable drops the modal");
            assert_eq!(active_names(&app), ["alpha"], "last-known agents remain");
        }
    }

    #[test]
    fn low_power_poll_keeps_search_fire_within_spec_ceiling() {
        assert!(SEARCH_DEBOUNCE + POLL_INTERVAL_LOW_POWER <= Duration::from_millis(500));
    }
}
