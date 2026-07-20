//! Input effects: the single adapter boundary a routed key passes through.
//!
//! Key *interpretation* stays in [`crate::cli::map_key`] and mode *policy* in
//! [`super::key_policy`], which decides — as data — what a mapped outcome means
//! in the active mode. This module owns the other half: it selects the mode,
//! runs exactly one policy, and applies the resulting [`Route`] against audio,
//! search, persistence, and Herdr.
//!
//! Two properties are structural rather than incidental here:
//!
//! - **Routed once.** A key is classified by exactly one mode policy. Only the
//!   Agent Planets stage may return [`Route::FallThrough`], and it falls
//!   through to normal-mode policy once; normal mode never falls through, so
//!   no key can reach two handlers and issue duplicate adapter commands.
//! - **Saved once.** Settings are snapshotted once per key and compared once
//!   after routing, so a key produces at most one persistence write regardless
//!   of which mode handled it.
//!
//! Every handler is driven in tests with a fake audio handle and no terminal.

use std::time::Instant;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::{Action, AgentPulseConnection, App, FocusPane, SearchStatus};
use crate::audio::{AudioCommand, AudioHandle};
use crate::cli::map_key;
use crate::herdr::{self, HerdrMonitor, MonitorEvent};
use crate::model::PlaybackState;

use super::debounce::{QueryChange, SearchDebounce};
use super::key_policy::{self, Effect, Route};
use super::persistence::Persistence;

/// Whether the event loop should keep running.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Flow {
    Continue,
    Quit,
}

/// Translate a key event into app actions and audio/search side effects.
///
/// Unit tests use this no-monitor entrypoint; the live event loop supplies the
/// eligible Herdr monitor through [`handle_key_with_monitor`].
#[cfg(test)]
fn handle_key(
    key: KeyEvent,
    app: &mut App,
    audio: &AudioHandle,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
) -> Flow {
    handle_key_with_monitor(key, app, audio, debounce, persistence, None)
}

pub(super) fn handle_key_with_monitor(
    key: KeyEvent,
    app: &mut App,
    audio: &AudioHandle,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
    monitor: Option<&HerdrMonitor>,
) -> Flow {
    // Snapshot settings once, before any routing, and compare once after: a
    // key produces at most one persistence write no matter which mode ran.
    let before = app.settings().clone();
    let flow = apply_route(
        route_key(key, app),
        app,
        audio,
        debounce,
        persistence,
        monitor,
    );

    // Quitting skips the save, exactly as before: teardown owns the final write.
    if flow == Flow::Continue && app.settings() != &before {
        persistence.save(app);
    }
    flow
}

/// Pick the mode that owns this key and return its single routing decision.
///
/// Mode precedence is unchanged: Signal View first (it never shows Agent
/// Pulse), then the Agent Planets stage and its modals, then normal mode. The
/// stage is the only mode that may defer, and it defers exactly once — to
/// normal-mode policy — so the documented global player controls (playback,
/// volume, theme, favorite, visualizer) keep their normal semantics over the
/// stage without any recursive dispatch.
fn route_key(key: KeyEvent, app: &App) -> Route {
    // Signal View maps keys as navigation: the search strip is hidden, so
    // allowed controls work regardless of the background focus preserved
    // underneath the mode.
    if app.is_signal_view() {
        return key_policy::signal_view(&map_key(key, false));
    }

    // The stage also maps as navigation — it only opens from navigation mode
    // and never moves focus into the search strip.
    let stage_open = app.is_agent_overlay_open();
    let searching = !stage_open && app.focus() == FocusPane::Search;
    let outcome = map_key(key, searching);

    if stage_open {
        let route = if app.is_agent_rename_open() {
            key_policy::rename(key, &outcome)
        } else if app.is_agent_details_open() {
            key_policy::details(key, &outcome)
        } else {
            key_policy::stage(&outcome)
        };
        // Only the stage (never a modal) can defer, and only to normal mode.
        if !matches!(route, Route::FallThrough) {
            return route;
        }
    }

    key_policy::normal(&outcome, app.focus())
}

/// Apply one routing decision. This is the only place a key reaches an adapter.
fn apply_route(
    route: Route,
    app: &mut App,
    audio: &AudioHandle,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
    monitor: Option<&HerdrMonitor>,
) -> Flow {
    match route {
        Route::Quit => Flow::Quit,
        // `FallThrough` is resolved in `route_key`; normal mode never emits it.
        Route::Consumed | Route::FallThrough => Flow::Continue,
        Route::Act(action) => {
            app.apply(action);
            Flow::Continue
        }
        Route::Effect(effect) => {
            apply_effect(effect, app, audio, debounce, persistence, monitor);
            Flow::Continue
        }
    }
}

/// Perform the adapter work a routed key asked for.
///
/// Each effect appears exactly once, so a given key can issue at most one audio
/// command, one debounce update, or one Herdr request — the property the
/// duplicated per-mode handlers used to leave to inspection.
fn apply_effect(
    effect: Effect,
    app: &mut App,
    audio: &AudioHandle,
    debounce: &mut SearchDebounce,
    persistence: &mut Persistence,
    monitor: Option<&HerdrMonitor>,
) {
    match effect {
        Effect::PlaySelected => {
            // Allocate the request first so the app expects exactly the attempt
            // the runtime is about to start.
            let request = audio.next_playback_request();
            app.apply(Action::PlaySelected(request));
            // Send only what the reducer accepted. `PlaySelected` is a no-op
            // when nothing is selectable (an empty list), and the still-current
            // station from an earlier play must not be restarted under a request
            // the app never recorded — that would make the restarted stream's
            // events look stale forever.
            if app.playback_request() == Some(request) {
                send_play(app, audio, request);
            }
        }
        Effect::TogglePlayback => {
            // The id is allocated up front and only becomes the expected request
            // if the toggle actually resumes playback.
            let request = audio.next_playback_request();
            app.apply(Action::TogglePlayback(request));
            match app.playback() {
                PlaybackState::Connecting => send_play(app, audio, request),
                PlaybackState::Stopped => {
                    let _ = audio.command_tx.send(AudioCommand::Stop);
                }
                _ => {}
            }
        }
        Effect::VolumeUp => set_volume(Action::VolumeUp, app, audio, persistence),
        Effect::VolumeDown => set_volume(Action::VolumeDown, app, audio, persistence),
        Effect::SearchChar(character) => {
            let mut query = app.search_query().to_string();
            query.push(character);
            update_search(app, debounce, query);
        }
        Effect::SearchBackspace => {
            let mut query = app.search_query().to_string();
            query.pop();
            update_search(app, debounce, query);
        }
        Effect::ClearSearch => {
            debounce.note_query("", Instant::now());
            app.apply(Action::SetSearchQuery(String::new()));
            app.apply(Action::SetSearchStatus(SearchStatus::Idle));
            app.apply(Action::ClearSearch);
            app.apply(Action::SetFocus(FocusPane::Stations));
        }
        Effect::FocusAgentPane => focus_selected_agent(app, monitor),
        Effect::SubmitRename => submit_agent_rename(app, monitor),
    }
}

/// Start the current station under `request`, if there is one to start.
fn send_play(app: &App, audio: &AudioHandle, request: crate::model::PlaybackRequestId) {
    if let Some(station) = app.current_station().cloned() {
        let _ = audio.command_tx.send(AudioCommand::Play {
            request,
            station: Box::new(station),
            volume: app.settings().volume,
        });
    }
}

/// Fold a volume step through the reducer, mark it user-changed for the save
/// policy, and push the resulting level to the audio runtime.
fn set_volume(action: Action, app: &mut App, audio: &AudioHandle, persistence: &mut Persistence) {
    app.apply(action);
    persistence.mark_user_changed_volume();
    let _ = audio
        .command_tx
        .send(AudioCommand::SetVolume(app.settings().volume));
}

/// Explicitly focus the selected pane through the already-eligible monitor.
/// The App issues an opaque target only for the open Connected stage, so stale
/// and unavailable snapshots cannot produce a socket call. Socket I/O runs in
/// a one-shot monitor worker; its typed result returns through `focus_events`
/// and never blocks input or rendering. Immediate local failures remain
/// recoverable, modal-local feedback and never change stage/modal selection.
fn focus_selected_agent(app: &mut App, monitor: Option<&HerdrMonitor>) {
    match app.agent_focus_target() {
        Some(id) => match monitor {
            Some(monitor) => monitor.focus_agent(id),
            None => apply_agent_focus_result(app, herdr::FocusResult::Unavailable, Instant::now()),
        },
        None if app.agent_pulse_connection() == AgentPulseConnection::Connected => {
            apply_agent_focus_result(app, herdr::FocusResult::NoSelection, Instant::now());
        }
        None => apply_agent_focus_result(app, herdr::FocusResult::Unavailable, Instant::now()),
    }
}

/// Apply the typed result from a background explicit pane-focus request.
pub(super) fn apply_agent_focus_result(app: &mut App, result: herdr::FocusResult, now: Instant) {
    app.apply(Action::AgentFocusResult { result, now });
}

/// Submit the inline table Name input via the already-eligible monitor. The
/// target is opaque outside `herdr`, socket work runs on its one-shot worker,
/// and stale state leaves the user input intact without dispatching anything.
fn submit_agent_rename(app: &mut App, monitor: Option<&HerdrMonitor>) {
    let Some((id, name)) = app.agent_rename_request() else {
        return;
    };
    app.apply(Action::SubmitAgentRename);
    match monitor {
        Some(monitor) => monitor.rename_agent(id, name),
        None => apply_agent_rename_result(app, herdr::RenameResult::Unavailable, Instant::now()),
    }
}

/// Fold a typed async `agent.rename` result into reducer-owned table state.
pub(super) fn apply_agent_rename_result(app: &mut App, result: herdr::RenameResult, now: Instant) {
    app.apply(Action::AgentRenameResult { result, now });
}

/// Fold a typed Herdr monitor event into app state at the current time.
///
/// Poll failures arrive here as recoverable reducer state (stale, then
/// unavailable), never as an event-loop error.
pub(super) fn apply_monitor_event(app: &mut App, event: MonitorEvent, now: Instant) {
    match event {
        MonitorEvent::Snapshot(agents) => app.apply(Action::AgentSnapshot { agents, now }),
        MonitorEvent::Failed => app.apply(Action::AgentPollFailed { now }),
    }
}

/// Route a mouse event through the pure UI hit test.
///
/// Only actions returned by [`crate::ui::agent_pulse_hit_test`] are applied —
/// read-only Agent Planets body selection by contract — so every click
/// outside the canvas keeps its current behavior: none. `low_power` is the
/// same controller flag the render call uses and `now` the same monotonic
/// clock the render loop reads, so clicks resolve against the planet
/// geometry — frozen or Working-orbit-advanced — that was actually drawn.
pub(super) fn handle_mouse(
    mouse: MouseEvent,
    area: Rect,
    low_power: bool,
    now: Instant,
    app: &mut App,
) {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    if let Some(action) =
        crate::ui::agent_pulse_hit_test(area, mouse.column, mouse.row, low_power, now, app)
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

#[cfg(test)]
mod tests {
    use super::*;
    // The routing half now lives in `key_policy`, so these behavior tests name
    // the key vocabulary they drive directly instead of inheriting it.
    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::cli::{map_key, KeyOutcome};

    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use crate::app::ListSource;
    use crate::audio::AudioEvent;
    use crate::catalog::{Catalog, Category, Section};
    use crate::herdr::{AgentDetails, AgentId, AgentSnapshot, AgentStatus};
    use crate::model::{PlaybackRequestId, VolumePercent};
    use crate::search::SearchResults;
    use crate::settings::Settings;

    use super::super::debounce::SEARCH_DEBOUNCE;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A fake audio handle whose command channel can be drained to assert what
    /// playback side effects a key press produced, without a real device.
    fn fake_audio() -> (AudioHandle, Receiver<AudioCommand>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (_event_tx, event_rx) = mpsc::channel();
        (AudioHandle::new(command_tx, event_rx), command_rx)
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

    /// The request id on the `Play` command the controller just sent.
    fn sent_play_request(cmd_rx: &Receiver<AudioCommand>) -> PlaybackRequestId {
        match cmd_rx.try_recv() {
            Ok(AudioCommand::Play { request, .. }) => request,
            other => panic!("expected a Play command, got {other:?}"),
        }
    }

    #[test]
    fn enter_sends_the_request_the_app_is_told_to_expect() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert_eq!(
            app.playback_request(),
            Some(sent_play_request(&cmd_rx)),
            "the command and the reducer must agree on the live request"
        );
    }

    #[test]
    fn replaying_the_same_station_allocates_a_new_request() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let first = sent_play_request(&cmd_rx);

        // Enter again on the very same station: a distinct attempt, so a
        // distinct request. Station identity is never reused as event identity.
        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let second = sent_play_request(&cmd_rx);

        assert_ne!(first, second, "a replay is a new playback request");
        assert_eq!(app.playback_request(), Some(second));
    }

    /// Switch to the (empty, on a default profile) Favorites source. Applying
    /// the Browse selection hands focus back to Stations, so `Enter` is live
    /// with nothing selectable.
    fn show_empty_favorites(app: &mut App) {
        let rail = ListSource::browse_rail();
        let favorites = rail
            .iter()
            .position(|source| *source == ListSource::Favorites)
            .expect("favorites is on the browse rail");
        app.apply(Action::SetBrowseSelection(favorites));
        app.apply(Action::ApplyBrowseSelection);
    }

    #[test]
    fn enter_with_nothing_selectable_sends_no_command_and_keeps_the_live_request() {
        // Regression: the reducer refuses to play when nothing is selected, but
        // the controller used to send `Play` anyway from the *previous* current
        // station. That restarted the runtime under a request the app never
        // recorded, so every event from the restarted stream was rejected as
        // stale and the UI silently diverged from what was audible.
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let played = sent_play_request(&cmd_rx);
        let playing_station = app.current_station().cloned().expect("a current station");

        show_empty_favorites(&mut app);
        assert_eq!(app.focus(), FocusPane::Stations);
        assert!(
            app.selected_station().is_none(),
            "the empty favorites list has nothing to play"
        );

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );

        assert!(
            cmd_rx.try_recv().is_err(),
            "a play the reducer refused must not reach the runtime"
        );
        assert_eq!(
            app.playback_request(),
            Some(played),
            "the live request is unchanged, so the playing stream's events stay valid"
        );
        assert_eq!(
            app.current_station().map(|station| station.id.clone()),
            Some(playing_station.id),
            "the current station is untouched"
        );
    }

    #[test]
    fn space_stop_clears_the_expected_request_and_resume_allocates_a_new_one() {
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let played = sent_play_request(&cmd_rx);

        // Space stops: nothing is expected, so the stopped session's workers
        // cannot affect state as they drain.
        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(matches!(cmd_rx.try_recv(), Ok(AudioCommand::Stop)));
        assert_eq!(app.playback_request(), None);

        // Space again resumes as a fresh request.
        handle_key(
            key(KeyCode::Char(' ')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        let resumed = sent_play_request(&cmd_rx);
        assert_ne!(played, resumed);
        assert_eq!(app.playback_request(), Some(resumed));
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

    fn pulse_agent(pane: &str) -> AgentSnapshot {
        AgentSnapshot {
            id: AgentId::new("ws", pane),
            // Distinct names keep the reducer's name sort stable.
            details: AgentDetails {
                name: Some(pane.to_string()),
                agent: None,
                activity: None,
            },
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

    #[test]
    fn agent_focus_key_dispatches_socket_io_off_the_event_loop() {
        let socket_path = std::env::temp_dir().join(format!(
            "wave-tui-focus-key-{}-{}.sock",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let listener = UnixListener::bind(&socket_path).expect("bind focus test socket");
        let (focus_started_tx, focus_started_rx) = mpsc::channel();
        let (poll_replied_tx, poll_replied_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.expect("accept focus test socket");
                let mut request = String::new();
                BufReader::new(stream.try_clone().expect("clone stream"))
                    .read_line(&mut request)
                    .expect("read request");
                if request.contains("agent.focus") {
                    focus_started_tx.send(()).expect("signal focus request");
                    thread::sleep(Duration::from_millis(250));
                    stream
                        .write_all(
                            b"{\"jsonrpc\":\"2.0\",\"id\":\"0\",\"error\":{\"code\":-32000}}\n",
                        )
                        .expect("reply to focus request");
                    break;
                }
                stream
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"result\":{\"agents\":[]}}\n")
                    .expect("reply to poll request");
                poll_replied_tx.send(()).expect("signal poll reply");
            }
        });
        let monitor = herdr::spawn_monitor(herdr::HerdrContext {
            socket_path: socket_path.clone(),
        });
        poll_replied_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("initial monitor poll completes before focus request");
        let (audio, _commands) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);

        let started = Instant::now();
        handle_key_with_monitor(
            key(KeyCode::Char('O')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
            Some(&monitor),
        );
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "the key handler must dispatch focus I/O instead of waiting for the socket"
        );
        focus_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("background focus request started");
        let result = monitor
            .focus_events()
            .recv_timeout(Duration::from_secs(1))
            .expect("typed focus result returns to the event loop");
        assert_eq!(result, herdr::FocusResult::Missing);
        apply_agent_focus_result(&mut app, result, Instant::now());
        assert_eq!(
            app.agent_focus_notice(Instant::now()),
            Some("pane is no longer available")
        );

        monitor.stop();
        server.join().expect("focus test server exits");
        std::fs::remove_file(socket_path).expect("remove focus test socket");
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
    fn o_is_stage_local_and_a_missing_monitor_keeps_details_open_with_feedback() {
        assert_eq!(
            map_key(key(KeyCode::Char('o')), false),
            KeyOutcome::FocusAgentPane
        );
        assert_eq!(
            map_key(key(KeyCode::Char('O')), false),
            KeyOutcome::FocusAgentPane
        );

        let (audio, _command_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let selected = app.selected_agent().map(|agent| agent.id.clone());

        handle_key(
            key(KeyCode::Char('O')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_agent_overlay_open());
        assert!(app.is_agent_details_open());
        assert_eq!(app.selected_agent().map(|agent| agent.id.clone()), selected);
        assert_eq!(
            app.agent_focus_notice(Instant::now()),
            Some("pane focus unavailable · retrying")
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
    fn enter_opens_details_only_for_selected_planet_and_modal_consumes_controls() {
        let (mut app, mut debounce, mut persistence) = controller();
        let (audio, command_rx) = fake_audio();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        let playback = app.playback().clone();

        assert_eq!(
            handle_key(
                key(KeyCode::Enter),
                &mut app,
                &audio,
                &mut debounce,
                &mut persistence,
            ),
            Flow::Continue
        );
        assert!(app.is_agent_details_open());
        assert_eq!(app.playback(), &playback);
        assert!(command_rx.try_recv().is_err());

        for code in [
            KeyCode::Char(' '),
            KeyCode::Char('+'),
            KeyCode::Char('z'),
            KeyCode::Char('t'),
            KeyCode::Char('/'),
        ] {
            handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
            assert!(app.is_agent_details_open(), "{code:?} is modal-local");
        }
        handle_key(
            key(KeyCode::Esc),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(!app.is_agent_details_open());
        assert!(app.is_agent_overlay_open(), "Esc closes only details");

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_agent_details_open());
        handle_key(
            key(KeyCode::Char('a')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(!app.is_agent_details_open());
        assert!(!app.is_agent_overlay_open(), "a closes the whole stage");
    }

    #[test]
    fn inline_rename_consumes_table_controls_and_keeps_failed_or_stale_input() {
        let (audio, command_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        let focus = app.focus();
        let playback = app.playback().clone();
        let settings = app.settings().clone();

        handle_key(
            key(KeyCode::Char('r')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.agent_rename_input(), Some("alpha"));
        handle_key(
            key(KeyCode::Char('!')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        handle_key(
            key(KeyCode::Backspace),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.agent_rename_input(), Some("alpha"));

        for code in [
            KeyCode::Tab,
            KeyCode::Char(' '),
            KeyCode::Char('+'),
            KeyCode::Char('f'),
            KeyCode::Char('t'),
            KeyCode::Char('v'),
            KeyCode::Char('/'),
        ] {
            handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
            assert!(app.is_agent_rename_open(), "{code:?} stays inside rename");
        }
        assert_eq!(app.focus(), focus);
        assert_eq!(app.playback(), &playback);
        assert_eq!(app.settings(), &settings);
        assert!(command_rx.try_recv().is_err());

        handle_key(
            key(KeyCode::Enter),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(app.agent_rename_input(), Some("alpha +ftv/"));
        assert_eq!(
            app.agent_rename_notice(Instant::now()),
            Some("rename unavailable · retrying")
        );

        app.apply(Action::AgentPollFailed {
            now: Instant::now(),
        });
        handle_key(
            key(KeyCode::Char('x')),
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
        assert_eq!(app.agent_rename_input(), Some("alpha +ftv/"));
        assert!(!app.agent_rename_is_submitting());
    }

    #[test]
    fn escape_cancels_inline_rename_without_closing_the_table_or_stage() {
        let (audio, _command_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        handle_key(
            key(KeyCode::Char('r')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(app.is_agent_rename_open());

        assert_eq!(
            handle_key(
                key(KeyCode::Esc),
                &mut app,
                &audio,
                &mut debounce,
                &mut persistence,
            ),
            Flow::Continue
        );
        assert!(!app.is_agent_rename_open());
        assert!(app.is_agent_details_open());
        assert!(app.is_agent_overlay_open());
    }

    #[test]
    fn modal_navigation_keys_cycle_planets_while_details_stay_open() {
        let (audio, _cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha", "beta"]);
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);

        // Two agents: next and previous both hop to the other planet, and
        // wrapping keeps every key productive from either end.
        for (code, expected) in [
            (KeyCode::Tab, "beta"),
            (KeyCode::Down, "alpha"),
            (KeyCode::Char('j'), "beta"),
            (KeyCode::BackTab, "alpha"),
            (KeyCode::Up, "beta"),
            (KeyCode::Char('k'), "alpha"),
        ] {
            let flow = handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
            assert_eq!(flow, Flow::Continue);
            assert!(app.is_agent_details_open(), "{code:?} keeps details open");
            assert_eq!(
                app.selected_agent_details()
                    .and_then(|detail| detail.name.as_deref()),
                Some(expected),
                "{code:?} moves the modal to the adjacent planet"
            );
        }
        assert!(app.is_agent_overlay_open());
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

    /// Scan the open Agent Planets canvas for the first cell the pure hit
    /// test resolves to a planet at `now`, so mouse tests target the actual
    /// drawn planet body cells instead of assuming any particular shape.
    fn first_collage_tile_hit(app: &App, now: Instant) -> (u16, u16) {
        let area = canvas_area();
        for row in 0..area.height {
            for column in 0..area.width {
                if crate::ui::agent_pulse_hit_test(area, column, row, false, now, app).is_some() {
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
        let now = Instant::now();
        let (x, y) = first_collage_tile_hit(&app, now);

        handle_mouse(left_click(x, y), canvas_area(), false, now, &mut app);
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
    fn collage_low_power_mouse_path_resolves_the_frozen_orbit_layout() {
        let mut app = connected_collage_app();
        app.configure_low_power_visuals(true);
        app.apply(Action::ToggleAgentOverlay);
        let request = crate::model::PlaybackRequestSeq::new().next_id();
        app.apply(Action::PlaySelected(request));
        app.apply(Action::Audio(AudioEvent::Viz {
            request,
            frame: crate::model::VizFrame::with_phase(
                vec![0.0; 16],
                0.1,
                Vec::<f32>::new(),
                crate::model::PhaseTrace::new([0.1], [0.1]),
                crate::model::PhaseTrace::empty(),
            ),
        }));
        let area = canvas_area();

        // The audible frame froze the whole solar layout. A discriminating
        // cell: covered by a frozen planet body at the captured angle, but
        // left behind by the live orbit-advanced position at `later` — a
        // hit test that tracked the clock would miss it.
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(40);
        let frozen_only = (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (column, row)))
            .find(|&(column, row)| {
                crate::ui::agent_pulse_hit_test(area, column, row, true, t0, &app).is_some()
                    && crate::ui::agent_pulse_hit_test(area, column, row, false, later, &app)
                        .is_none()
            });
        let (x, y) = frozen_only.expect("elapsed Working time moves some live planet cell");

        handle_mouse(left_click(x, y), area, true, later, &mut app);
        assert!(
            app.selected_agent().is_some(),
            "a low-power click resolves the frozen captured orbit angle"
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
        handle_mouse(left_click(5, 5), area, false, Instant::now(), &mut app);
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
        handle_mouse(moved, area, false, Instant::now(), &mut app);
        assert_eq!(app.selected_index(), station_before);
        assert!(app.selected_agent().is_none());
    }

    /// Drain the audio channel into a list of command labels.
    fn drained(cmd_rx: &Receiver<AudioCommand>) -> Vec<&'static str> {
        let mut commands = Vec::new();
        while let Ok(command) = cmd_rx.try_recv() {
            commands.push(match command {
                AudioCommand::Play { .. } => "Play",
                AudioCommand::Stop => "Stop",
                AudioCommand::SetVolume(_) => "SetVolume",
                _ => "other",
            });
        }
        commands
    }

    #[test]
    fn one_key_issues_at_most_one_audio_command_in_every_mode() {
        // The acceptance criterion the policy/effect split exists to guarantee:
        // whichever mode claims a key, the key reaches the adapter boundary
        // once. The stage fall-through is the interesting case — `+` is routed
        // by the stage policy *and* by normal policy, so a double dispatch
        // would surface here as two SetVolume commands.
        let (audio, cmd_rx) = fake_audio();
        let (mut app, mut debounce, mut persistence) = controller();
        connect_agent_pulse(&mut app, &["alpha"]);

        // Normal mode.
        handle_key(
            key(KeyCode::Char('+')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(drained(&cmd_rx), ["SetVolume"], "normal mode volume");

        // Agent Planets stage: `+` falls through to normal mode exactly once.
        app.apply(Action::ToggleAgentOverlay);
        assert!(app.is_agent_overlay_open());
        handle_key(
            key(KeyCode::Char('+')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(
            drained(&cmd_rx),
            ["SetVolume"],
            "the stage must not dispatch a fallen-through key twice"
        );

        // Stage-consumed keys reach no adapter at all.
        for code in [KeyCode::Enter, KeyCode::Char('/'), KeyCode::Home] {
            handle_key(key(code), &mut app, &audio, &mut debounce, &mut persistence);
        }
        assert!(
            drained(&cmd_rx).is_empty(),
            "stage-consumed keys issue no audio command"
        );

        // Details modal consumes `+` entirely: no volume command escapes.
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        handle_key(
            key(KeyCode::Char('+')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert!(
            drained(&cmd_rx).is_empty(),
            "the details modal consumes player controls"
        );

        // Signal View: allowed, and still exactly one command.
        app.apply(Action::CloseAgentOverlay);
        app.apply(Action::ToggleSignalView);
        assert!(app.is_signal_view());
        handle_key(
            key(KeyCode::Char('+')),
            &mut app,
            &audio,
            &mut debounce,
            &mut persistence,
        );
        assert_eq!(drained(&cmd_rx), ["SetVolume"], "Signal View volume");
    }
}
