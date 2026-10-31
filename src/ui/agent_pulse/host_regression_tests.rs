use super::{geometry::agent_stage_layout, hit_test, modal::agent_table_modal_area, render_canvas};
use crate::app::{Action, App};
use crate::audio::AudioEvent;
use crate::catalog::Catalog;
use crate::model::agent::{
    AgentCapabilities, AgentDetails, AgentId, AgentSnapshot, AgentStatus, FocusResult,
};
use crate::model::{PhaseTrace, PlaybackRequestSeq, VizFrame};
use crate::settings::Settings;
use crate::theme::{Theme, ThemeName};
use ratatui::{buffer::Buffer, layout::Rect};
use std::time::{Duration, Instant};

const AREA: Rect = Rect::new(0, 0, 120, 36);

#[derive(Clone, Copy, Debug)]
enum Host {
    Herdr,
    Core,
}

impl Host {
    fn id(self) -> AgentId {
        match self {
            Self::Herdr => AgentId::herdr("PRIVATE_WORKSPACE_MARKER", "PRIVATE_PANE_MARKER_PRIVATE_SOCKET_MARKER_PRIVATE_CWD_MARKER_PRIVATE_RAWSTATUS_MARKER"),
            Self::Core => AgentId::core("PRIVATE_ADDRESS_MARKER_PRIVATE_SOCKET_MARKER_PRIVATE_CWD_MARKER_PRIVATE_RAWSTATUS_MARKER"),
        }
    }
    fn snapshot(self, named: bool) -> AgentSnapshot {
        AgentSnapshot {
            id: self.id(),
            details: AgentDetails {
                name: named.then(|| "ExplicitName".to_string()),
                agent: Some("ApprovedAgent".to_string()),
                activity: Some("ApprovedActivity".to_string()),
            },
            status: AgentStatus::Working,
        }
    }
    fn app(self, now: Instant, named: bool) -> App {
        let mut app = App::new(Settings::default(), Catalog::curated());
        app.configure_agent_capabilities(match self {
            Self::Herdr => AgentCapabilities::HERDR,
            Self::Core => AgentCapabilities::READ_ONLY,
        });
        app.apply(Action::AgentSnapshot {
            agents: vec![self.snapshot(named)],
            now,
        });
        app.apply(Action::ToggleAgentOverlay);
        app.apply(Action::SelectAgent(self.id()));
        app
    }
}

fn frame(offset: f32) -> VizFrame {
    let samples: Vec<f32> = (0..48)
        .map(|i| ((i as f32 * 0.23) + offset).sin() * 0.8)
        .collect();
    let other: Vec<f32> = (0..48)
        .map(|i| ((i as f32 * 0.41) + offset).sin() * 0.7)
        .collect();
    VizFrame::with_phase(
        vec![0.5; 16],
        0.5,
        Vec::<f32>::new(),
        PhaseTrace::new(samples.clone(), other.clone()),
        PhaseTrace::new(other, samples),
    )
}

fn push(app: &mut App, frame: VizFrame) {
    if app.playback_request().is_none() {
        app.apply(Action::PlaySelected(PlaybackRequestSeq::new().next_id()));
    }
    app.apply(Action::Audio(AudioEvent::Viz {
        request: app.playback_request().unwrap(),
        frame,
    }));
}

fn render(app: &App, low: bool, now: Instant) -> Buffer {
    let mut buffer = Buffer::empty(AREA);
    render_canvas(
        app,
        &Theme::for_name(ThemeName::Minimal),
        low,
        now,
        AREA,
        &mut buffer,
    );
    buffer
}

fn symbols(buffer: &Buffer, area: Rect) -> String {
    let mut result = String::new();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            result.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        result.push('\n');
    }
    result
}

fn hits(app: &App, low: bool, now: Instant) -> Vec<(u16, u16)> {
    (AREA.y..AREA.bottom())
        .flat_map(|y| {
            (AREA.x..AREA.right())
                .filter_map(move |x| hit_test(AREA, x, y, low, now, app).map(|_| (x, y)))
        })
        .collect()
}

#[test]
fn both_hosts_freeze_stale_canvas_and_selected_modal_then_recover_live() {
    for host in [Host::Herdr, Host::Core] {
        for modal in [false, true] {
            let t = Instant::now();
            let edge = t + Duration::from_secs(2);
            let later = t + Duration::from_secs(10);
            let mut app = host.app(t, true);
            push(&mut app, frame(0.2));
            if modal {
                app.apply(Action::OpenAgentDetails);
                assert!(app.is_agent_details_open());
            }
            let live = render(&app, false, edge);
            app.apply(Action::AgentPollFailed { now: edge });
            let stale = render(&app, false, edge);
            let field = agent_stage_layout(AREA).field;
            let modal_area = agent_table_modal_area(field, 1);
            for y in field.y..field.bottom() {
                for x in field.x..field.right() {
                    // The reconnect suffix changes only the modal's title span.
                    if modal && y == modal_area.y && x > modal_area.x && x < modal_area.x + 30 {
                        continue;
                    }
                    assert_eq!(
                        live.cell((x, y)).unwrap().symbol(),
                        stale.cell((x, y)).unwrap().symbol(),
                        "{host:?} at ({x},{y})"
                    );
                }
            }
            push(&mut app, frame(1.4));
            app.apply(Action::AgentTick { now: later });
            assert_eq!(
                stale,
                render(&app, false, later),
                "{host:?}: stale composition must remain frozen"
            );
            assert!(hits(&app, false, later).is_empty());
            app.apply(Action::AgentSnapshot {
                agents: vec![host.snapshot(true)],
                now: later,
            });
            let recovered = render(&app, false, later);
            assert_ne!(recovered, stale);
            assert!(symbols(&recovered, agent_stage_layout(AREA).heading).contains("Agent Planets"));
            assert!(!hits(&app, false, later).is_empty());
            // A live control pauses the working orbit over exactly the same
            // interval via an Idle snapshot, rather than a failed poll.
            let mut control = host.app(t, true);
            push(&mut control, frame(0.2));
            if modal {
                control.apply(Action::OpenAgentDetails);
            }
            let mut idle = host.snapshot(true);
            idle.status = AgentStatus::Idle;
            control.apply(Action::AgentSnapshot {
                agents: vec![idle],
                now: edge,
            });
            push(&mut control, frame(1.4));
            control.apply(Action::AgentSnapshot {
                agents: vec![host.snapshot(true)],
                now: later,
            });
            assert_eq!(
                recovered,
                render(&control, false, later),
                "{host:?}: recovery exactly matches a live composition"
            );
            assert_eq!(hits(&app, false, later), hits(&control, false, later));
        }
    }
}

#[test]
fn both_hosts_low_power_capture_freezes_render_and_hit_geometry() {
    for host in [Host::Herdr, Host::Core] {
        let t = Instant::now();
        let mut app = host.app(t, false);
        app.configure_low_power_visuals(true);
        push(&mut app, frame(0.2));
        let initial = render(&app, true, t);
        let initial_hits = hits(&app, true, t);
        assert!(!initial_hits.is_empty());
        push(&mut app, frame(1.4));
        let later = t + Duration::from_secs(60);
        assert_eq!(initial, render(&app, true, later), "{host:?}");
        assert_eq!(initial_hits, hits(&app, true, later), "{host:?}");
        assert_ne!(
            initial,
            render(&app, false, later),
            "live frame or orbit must advance"
        );
    }
}

#[test]
fn both_hosts_only_render_approved_fields_and_core_has_no_control_hints() {
    for host in [Host::Herdr, Host::Core] {
        let t = Instant::now();
        let mut app = host.app(t, false);
        push(&mut app, frame(0.2));
        let stage = symbols(&render(&app, false, t), AREA);
        assert!(!stage.contains("ApprovedAgent"));
        assert!(!stage.contains("ApprovedActivity"));
        app.apply(Action::OpenAgentDetails);
        assert!(app.is_agent_details_open());
        let table = symbols(&render(&app, false, t), AREA);
        assert!(table.contains("ApprovedAgent"));
        assert!(table.contains("ApprovedActivity"));
        if matches!(host, Host::Core) {
            for text in [&stage, &table] {
                assert!(!text.contains("open pane"));
                assert!(!text.contains("rename"));
            }
        }
        app.apply(Action::AgentFocusResult {
            result: FocusResult::Missing,
            now: t,
        });
        let notice = symbols(&render(&app, false, t), AREA);
        for text in [&stage, &table, &notice] {
            for marker in [
                "PRIVATE_WORKSPACE_MARKER",
                "PRIVATE_PANE_MARKER",
                "PRIVATE_ADDRESS_MARKER",
                "PRIVATE_SOCKET_MARKER",
                "PRIVATE_CWD_MARKER",
                "PRIVATE_RAWSTATUS_MARKER",
            ] {
                assert!(!text.contains(marker), "{host:?} leaked {marker}");
            }
        }
    }
}
