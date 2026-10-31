//! Cross-host Agent Planets regression matrix.
//!
//! The same normalized observation sequence is driven through the one `App`
//! reducer for a Herdr-shaped host (workspace-qualified ids, full
//! capabilities) and a herdr-core-shaped host (single-address ids, read-only
//! capabilities). Lifecycle and selection must come out identical; only the
//! explicit-request capabilities may differ. Identity values are deliberately
//! different per host, so nothing here compares host-specific geometry or
//! hashes. Time is fake: every instant is an offset from one `Instant`.

use super::*;
use crate::model::agent::{AgentDetails, AgentStatus, RenameResult};
use crate::model::PlaybackRequestSeq;
use std::time::Duration;

#[derive(Clone, Copy)]
struct Host {
    label: &'static str,
    capabilities: AgentCapabilities,
    id: fn(&str) -> AgentId,
}

fn herdr_id(key: &str) -> AgentId {
    AgentId::herdr("ws-private", key)
}

fn core_id(key: &str) -> AgentId {
    AgentId::core(format!("core-addr-{key}"))
}

const HERDR: Host = Host {
    label: "herdr",
    capabilities: AgentCapabilities::HERDR,
    id: herdr_id,
};

const CORE: Host = Host {
    label: "core",
    capabilities: AgentCapabilities::READ_ONLY,
    id: core_id,
};

impl Host {
    fn agent(&self, key: &str, name: &str, status: AgentStatus) -> AgentSnapshot {
        AgentSnapshot {
            id: (self.id)(key),
            details: AgentDetails {
                name: Some(name.to_string()),
                agent: None,
                activity: None,
            },
            status,
        }
    }

    fn app(&self) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.configure_agent_capabilities(self.capabilities);
        app
    }

    fn pair(&self) -> Vec<AgentSnapshot> {
        vec![
            self.agent("p2", "beta", AgentStatus::Idle),
            self.agent("p1", "alpha", AgentStatus::Working),
        ]
    }
}

/// Everything lifecycle and selection expose, with no host-specific value.
#[derive(Debug, PartialEq, Eq)]
struct Observation {
    connection: AgentPulseConnection,
    names: Vec<String>,
    selected: Option<String>,
    overlay_open: bool,
    details_open: bool,
    frozen_display: bool,
}

fn observe(app: &App) -> Observation {
    let name = |view: &AgentView| view.name.clone().unwrap_or_default();
    Observation {
        connection: app.agent_pulse_connection(),
        names: app.active_agents().iter().map(name).collect(),
        selected: app.selected_agent().map(name),
        overlay_open: app.is_agent_overlay_open(),
        details_open: app.is_agent_details_open(),
        frozen_display: app.stale_viz().is_some(),
    }
}

fn expect(
    connection: AgentPulseConnection,
    names: &[&str],
    selected: Option<&str>,
    overlay_open: bool,
    details_open: bool,
    frozen_display: bool,
) -> Observation {
    Observation {
        connection,
        names: names.iter().map(|name| name.to_string()).collect(),
        selected: selected.map(str::to_string),
        overlay_open,
        details_open,
        frozen_display,
    }
}

type Trace = Vec<(&'static str, Observation)>;

fn mark(trace: &mut Trace, label: &'static str, app: &App) {
    trace.push((label, observe(app)));
}

fn assert_trace(host: Host, got: Trace, want: Trace) {
    assert_eq!(got.len(), want.len(), "{}: trace length", host.label);
    for ((label, got), (want_label, want)) in got.iter().zip(&want) {
        assert_eq!(label, want_label, "{}: step order", host.label);
        assert_eq!(got, want, "{}: step `{label}`", host.label);
    }
}

use AgentPulseConnection::{Connected, Hidden, Stale, Unavailable};

fn lifecycle_trace(host: Host) -> Trace {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut app = host.app();
    let mut trace = Trace::new();

    // Hidden stays inert: a tick and the stage toggle reveal nothing.
    app.apply(Action::AgentTick { now: t0 });
    app.apply(Action::ToggleAgentOverlay);
    mark(&mut trace, "hidden", &app);

    app.apply(Action::AgentSnapshot {
        agents: host.pair(),
        now: t0,
    });
    mark(&mut trace, "connected", &app);

    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectNextAgent);
    app.apply(Action::OpenAgentDetails);
    mark(&mut trace, "stage-open", &app);

    app.apply(Action::AgentPollFailed { now: at(1_000) });
    mark(&mut trace, "first-failure", &app);

    // Stale data cannot be navigated or re-targeted.
    app.apply(Action::SelectNextAgent);
    app.apply(Action::SelectPreviousAgent);
    app.apply(Action::SelectAgent((host.id)("p2")));
    app.apply(Action::OpenAgentDetails);
    mark(&mut trace, "stale-gated", &app);

    app.apply(Action::AgentPollFailed { now: at(14_998) });
    app.apply(Action::AgentTick { now: at(14_999) });
    mark(&mut trace, "just-below-threshold", &app);

    app.apply(Action::AgentTick { now: at(15_000) });
    mark(&mut trace, "exact-threshold", &app);

    app.apply(Action::SelectNextAgent);
    app.apply(Action::SelectAgent((host.id)("p2")));
    app.apply(Action::OpenAgentDetails);
    mark(&mut trace, "unavailable-gated", &app);

    app.apply(Action::AgentSnapshot {
        agents: host.pair(),
        now: at(16_000),
    });
    mark(&mut trace, "recovered", &app);

    app.apply(Action::SelectNextAgent);
    app.apply(Action::OpenAgentDetails);
    mark(&mut trace, "selection-resumes", &app);

    app.apply(Action::AgentSnapshot {
        agents: vec![host.agent("p1", "alpha", AgentStatus::Working)],
        now: at(17_000),
    });
    mark(&mut trace, "selected-agent-left", &app);

    app.apply(Action::AgentSnapshot {
        agents: Vec::new(),
        now: at(18_000),
    });
    mark(&mut trace, "all-agents-left", &app);

    app.apply(Action::AgentPollFailed { now: at(19_000) });
    mark(&mut trace, "second-streak-first-failure", &app);

    app.apply(Action::AgentTick { now: at(32_999) });
    mark(&mut trace, "second-streak-below-threshold", &app);

    app.apply(Action::AgentPollFailed { now: at(33_000) });
    mark(&mut trace, "second-streak-exact-threshold", &app);

    trace
}

fn expected_lifecycle_trace() -> Trace {
    let both = ["alpha", "beta"];
    vec![
        ("hidden", expect(Hidden, &[], None, false, false, false)),
        (
            "connected",
            expect(Connected, &both, None, false, false, false),
        ),
        (
            "stage-open",
            expect(Connected, &both, Some("alpha"), true, true, false),
        ),
        (
            "first-failure",
            expect(Stale, &both, Some("alpha"), true, true, true),
        ),
        (
            "stale-gated",
            expect(Stale, &both, Some("alpha"), true, true, true),
        ),
        (
            "just-below-threshold",
            expect(Stale, &both, Some("alpha"), true, true, true),
        ),
        (
            "exact-threshold",
            expect(Unavailable, &both, Some("alpha"), true, false, false),
        ),
        (
            "unavailable-gated",
            expect(Unavailable, &both, Some("alpha"), true, false, false),
        ),
        (
            "recovered",
            expect(Connected, &both, Some("alpha"), true, false, false),
        ),
        (
            "selection-resumes",
            expect(Connected, &both, Some("beta"), true, true, false),
        ),
        (
            "selected-agent-left",
            expect(Connected, &["alpha"], None, true, false, false),
        ),
        (
            "all-agents-left",
            expect(Connected, &[], None, true, false, false),
        ),
        (
            "second-streak-first-failure",
            expect(Stale, &[], None, true, false, true),
        ),
        (
            "second-streak-below-threshold",
            expect(Stale, &[], None, true, false, true),
        ),
        (
            "second-streak-exact-threshold",
            expect(Unavailable, &[], None, true, false, false),
        ),
    ]
}

#[test]
fn lifecycle_and_selection_follow_one_trace_on_both_hosts() {
    for host in [HERDR, CORE] {
        assert_trace(host, lifecycle_trace(host), expected_lifecycle_trace());
    }
}

fn failure_first_trace(host: Host) -> Trace {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut app = host.app();
    let mut trace = Trace::new();

    // The first event proving life may be a failure; the 15 s window then runs
    // from the start of that streak.
    app.apply(Action::AgentPollFailed { now: t0 });
    mark(&mut trace, "first-failure", &app);
    app.apply(Action::AgentTick { now: at(14_999) });
    mark(&mut trace, "just-below-threshold", &app);
    app.apply(Action::AgentTick { now: at(15_000) });
    mark(&mut trace, "exact-threshold", &app);
    app.apply(Action::AgentSnapshot {
        agents: vec![host.agent("p1", "alpha", AgentStatus::Working)],
        now: at(16_000),
    });
    mark(&mut trace, "recovered", &app);

    trace
}

#[test]
fn a_failure_before_any_snapshot_follows_one_trace_on_both_hosts() {
    let expected = || {
        vec![
            (
                "first-failure",
                expect(Stale, &[], None, false, false, false),
            ),
            (
                "just-below-threshold",
                expect(Stale, &[], None, false, false, false),
            ),
            (
                "exact-threshold",
                expect(Unavailable, &[], None, false, false, false),
            ),
            (
                "recovered",
                expect(Connected, &["alpha"], None, false, false, false),
            ),
        ]
    };
    for host in [HERDR, CORE] {
        assert_trace(host, failure_first_trace(host), expected());
    }
}

#[test]
fn explicit_requests_follow_the_host_capabilities_not_the_identity_kind() {
    for (host, live) in [(HERDR, true), (CORE, false)] {
        let now = Instant::now();
        let mut app = host.app();
        app.apply(Action::AgentSnapshot {
            agents: host.pair(),
            now,
        });
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        assert!(app.is_agent_details_open(), "{}: details open", host.label);
        assert_eq!(app.agent_capabilities(), host.capabilities);

        // Focus: a target exists only on a host that can focus, and only on
        // fresh data.
        assert_eq!(
            app.agent_focus_target(),
            live.then(|| (host.id)("p1")),
            "{}: focus target",
            host.label
        );

        // Rename: the editor, its request, and submission exist only when the
        // host can rename.
        app.apply(Action::OpenAgentRename);
        assert_eq!(app.is_agent_rename_open(), live, "{}: editor", host.label);
        assert_eq!(app.agent_rename_request().is_some(), live);
        app.apply(Action::SubmitAgentRename);
        assert_eq!(app.agent_rename_is_submitting(), live);

        // Results the host could never have produced leave no notice.
        app.apply(Action::AgentFocusResult {
            result: FocusResult::Unsupported,
            now,
        });
        let request = app
            .agent_rename_in_flight()
            .unwrap_or(RenameRequestId::FIRST);
        app.apply(Action::AgentRenameResult {
            completion: RenameCompletion {
                request,
                result: RenameResult::Unsupported,
            },
            now,
        });
        assert_eq!(app.agent_focus_notice(now).is_some(), live);
        assert_eq!(app.agent_rename_notice(now).is_some(), live);

        // Stale data revokes every explicit request on both hosts.
        app.apply(Action::AgentPollFailed { now });
        assert_eq!(app.agent_pulse_connection(), Stale);
        assert_eq!(app.agent_focus_target(), None, "{}: stale", host.label);
        assert!(app.agent_rename_request().is_none());
        assert!(app.is_agent_details_open(), "stale keeps the record");
    }
}

/// Everything the core radio reducer owns, read through its public queries.
#[derive(Debug, PartialEq)]
struct RadioFingerprint {
    focus: FocusPane,
    visible: Vec<String>,
    selected_index: usize,
    source: ListSource,
    browse_selected: usize,
    playback: PlaybackState,
    playback_request: Option<PlaybackRequestId>,
    current_station: Option<String>,
    now_playing_title: Option<String>,
    viz: VizFrame,
    offline: bool,
    settings_save_failed: bool,
    search_query: String,
    search_status: SearchStatus,
    settings: Settings,
    display_mode: DisplayMode,
}

fn fingerprint(app: &App) -> RadioFingerprint {
    RadioFingerprint {
        focus: app.focus(),
        visible: app
            .visible()
            .iter()
            .map(|station| station.id.as_str().to_string())
            .collect(),
        selected_index: app.selected_index(),
        source: app.active_source(),
        browse_selected: app.browse_selected(),
        playback: app.playback().clone(),
        playback_request: app.playback_request(),
        current_station: app
            .current_station()
            .map(|station| station.id.as_str().to_string()),
        now_playing_title: app.now_playing_title().map(str::to_string),
        viz: app.viz().clone(),
        offline: app.is_offline(),
        settings_save_failed: app.settings_save_failed(),
        search_query: app.search_query().to_string(),
        search_status: app.search_status().clone(),
        settings: app.settings().clone(),
        display_mode: app.display_mode(),
    }
}

#[test]
fn host_observation_never_moves_radio_audio_search_or_settings_state() {
    for host in [HERDR, CORE] {
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let mut app = host.app();
        app.apply(Action::SetSearchQuery("jazz".to_string()));
        app.apply(Action::SetSearchStatus(SearchStatus::Loading));
        app.apply(Action::SelectNext);
        app.apply(Action::SetFocus(FocusPane::NowPlaying));
        app.apply(Action::PlaySelected(PlaybackRequestSeq::new().next_id()));
        app.apply(Action::ToggleFavorite);
        app.apply(Action::VolumeUp);
        app.apply(Action::SetOffline(true));
        let before = fingerprint(&app);
        let settings_before = app.settings().clone();

        // Every observation shape both hosts deliver: first snapshot, stage
        // and details, status changes, disappearance, failure, the exact-15 s
        // threshold by tick, recovery, and an empty snapshot.
        app.apply(Action::AgentSnapshot {
            agents: host.pair(),
            now: t0,
        });
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectNextAgent);
        app.apply(Action::OpenAgentDetails);
        app.apply(Action::AgentSnapshot {
            agents: vec![host.agent("p1", "alpha", AgentStatus::Blocked)],
            now: at(1),
        });
        app.apply(Action::AgentPollFailed { now: at(2) });
        app.apply(Action::AgentTick { now: at(16) });
        assert_eq!(app.agent_pulse_connection(), Unavailable);
        app.apply(Action::AgentSnapshot {
            agents: host.pair(),
            now: at(17),
        });
        app.apply(Action::AgentSnapshot {
            agents: Vec::new(),
            now: at(18),
        });
        app.apply(Action::CloseAgentOverlay);

        assert_eq!(
            fingerprint(&app),
            before,
            "{}: an observation moved core radio state",
            host.label
        );
        assert_eq!(
            app.settings(),
            &settings_before,
            "{}: settings intent changed, so a save would be due",
            host.label
        );
    }
}
