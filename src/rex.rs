//! Rex Agent Planets adapter. Only this module sees Rex commands and payloads.
//! One local server/session, declared OSC 7501 records only; no screen scraping.
//! The bounded CLI runs on one owned thread, never on the audio/UI loop.

mod command;
mod context;

pub(crate) use context::{context_from_env, RexContext};

use crate::model::agent::{
    AgentDetails, AgentId, AgentSnapshot, AgentStatus, FocusResult, MonitorEvent,
};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const ATTEMPT_DEADLINE: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const OBSERVE: &str = include_str!("rex/observe.lua");
const FOCUS: &str = r#"local result, err = rex.session.focus_block{block_id=rex.args.block_id}
if not result then error(err) end
return {focused=true}"#;
use serde_json::Value;
use std::collections::HashSet;

fn nonblank(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

fn parse_snapshot(session: &str, bytes: &[u8]) -> Option<Vec<AgentSnapshot>> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    if value.get("version")?.as_u64()? != 1 {
        return None;
    }
    let entries = value.get("entries")?;
    // Rex encodes a Lua empty table as {}, not []. Accept only the empty
    // object, never a malformed nonempty object as a successful empty poll.
    if entries.as_object().is_some_and(|object| object.is_empty()) {
        return Some(vec![]);
    }
    let mut seen = HashSet::new();
    let mut agents = Vec::new();
    for entry in entries.as_array()? {
        let Some(block) = nonblank(entry.get("block_id")) else {
            continue;
        };
        let Some(record) = entry.get("record_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(app) = nonblank(entry.get("app")) else {
            continue;
        };
        let id = AgentId::rex(session, &block, record);
        if !seen.insert(id.clone()) {
            continue;
        }
        agents.push(AgentSnapshot {
            id,
            details: AgentDetails {
                name: nonblank(entry.get("name")),
                agent: Some(app),
                activity: None,
            },
            status: match entry.get("state").and_then(Value::as_str) {
                Some("working") => AgentStatus::Working,
                Some("blocked") => AgentStatus::Blocked,
                Some("done") => AgentStatus::Done,
                Some("idle") => AgentStatus::Idle,
                _ => AgentStatus::Unknown,
            },
        });
    }
    Some(agents)
}

fn cli_command(context: &RexContext, script: &str, args: Value) -> Command {
    let mut command = Command::new(&context.executable);
    command.args([
        "--autostart=false",
        "--timeout",
        "3s",
        "--server",
        &context.server,
        "do",
        "--session",
        &context.session,
        "--json",
        "--eval",
        script,
        "--args",
        &args.to_string(),
    ]);
    // A selected block/client inherited from the parent must not retarget us.
    command.env_remove("REX_BLOCK").env_remove("REX_CLIENT");
    command
}

fn run_cli(context: &RexContext, script: &str, args: Value, stop: &AtomicBool) -> Option<Vec<u8>> {
    command::run(
        &mut cli_command(context, script, args),
        ATTEMPT_DEADLINE,
        MAX_RESPONSE_BYTES,
        stop,
    )
}

enum Request {
    Focus(AgentId),
    Stop,
}

pub(crate) struct RexMonitor {
    events: mpsc::Receiver<MonitorEvent>,
    focus_events: mpsc::Receiver<FocusResult>,
    requests: mpsc::Sender<Request>,
    stopping: Arc<AtomicBool>,
    focus_busy: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl RexMonitor {
    pub(crate) fn events(&self) -> &mpsc::Receiver<MonitorEvent> {
        &self.events
    }
    pub(crate) fn focus_events(&self) -> &mpsc::Receiver<FocusResult> {
        &self.focus_events
    }

    pub(crate) fn focus_agent(&self, id: AgentId) {
        if self.stopping.load(Ordering::Acquire) || self.focus_busy.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.requests.send(Request::Focus(id)).is_err() {
            self.focus_busy.store(false, Ordering::Release);
        }
    }

    pub(crate) fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.requests.send(Request::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RexMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn spawn_monitor(context: RexContext) -> RexMonitor {
    spawn_with(context, POLL_INTERVAL, run_cli)
}

fn spawn_with(
    context: RexContext,
    interval: Duration,
    run: impl Fn(&RexContext, &str, Value, &AtomicBool) -> Option<Vec<u8>> + Send + 'static,
) -> RexMonitor {
    let (events_tx, events) = mpsc::channel();
    let (focus_tx, focus_events) = mpsc::channel();
    let (requests, requests_rx) = mpsc::channel();
    let stopping = Arc::new(AtomicBool::new(false));
    let focus_busy = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let busy = focus_busy.clone();
    let spawn_failure = events_tx.clone();
    let handle = thread::Builder::new()
        .name("rex-monitor".into())
        .spawn(move || {
            let mut next_poll = Instant::now();
            while !stop.load(Ordering::Acquire) {
                if Instant::now() >= next_poll {
                    let snapshot = run(&context, OBSERVE, serde_json::json!({}), &stop)
                        .and_then(|bytes| parse_snapshot(&context.session, &bytes));
                    let event = snapshot
                        .map(MonitorEvent::Snapshot)
                        .unwrap_or(MonitorEvent::Failed);
                    if events_tx.send(event).is_err() {
                        break;
                    }
                    next_poll = Instant::now() + interval;
                }
                match requests_rx.recv_timeout(next_poll.saturating_duration_since(Instant::now()))
                {
                    Ok(Request::Focus(id)) => {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let result = match id.rex_pane_target() {
                            Some((session, block)) if session == context.session => {
                                let response = run(
                                    &context,
                                    FOCUS,
                                    serde_json::json!({"block_id":block}),
                                    &stop,
                                )
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                                if response
                                    .as_ref()
                                    .and_then(|v| v.get("focused"))
                                    .and_then(Value::as_bool)
                                    == Some(true)
                                {
                                    FocusResult::Focused
                                } else {
                                    FocusResult::Unavailable
                                }
                            }
                            _ => FocusResult::Missing,
                        };
                        busy.store(false, Ordering::Release);
                        let _ = focus_tx.send(result);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
        });
    if handle.is_err() {
        let _ = spawn_failure.send(MonitorEvent::Failed);
    }
    RexMonitor {
        events,
        focus_events,
        requests,
        stopping,
        focus_busy,
        handle: handle.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::agent::AgentStatus;

    fn fixture_context() -> RexContext {
        RexContext {
            executable: "/nonexistent/rex".into(),
            server: "unix:///tmp/test-rex.sock".into(),
            session: "session:test".into(),
        }
    }

    #[test]
    fn cli_is_shell_free_and_never_autostarts_or_uses_ambient_targets() {
        let context = fixture_context();
        let command = cli_command(&context, OBSERVE, serde_json::json!({}));
        let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
        assert!(args.contains(&"--autostart=false"));
        assert!(args.windows(2).any(|a| a == ["--server", &context.server]));
        assert!(args
            .windows(2)
            .any(|a| a == ["--session", &context.session]));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "REX_BLOCK" && value.is_none()));
        for forbidden in [
            ".format",
            "\"process\"",
            "\"pwd\"",
            "record.msg",
            "\"write\"",
            "rex.exec",
            "rex.servers",
        ] {
            assert!(!OBSERVE.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn monitor_polls_recovers_focuses_once_and_refuses_foreign_targets() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let polls = std::sync::atomic::AtomicUsize::new(0);
        let monitor = spawn_with(
            fixture_context(),
            Duration::from_millis(40),
            move |_, script, args, _| {
                if script == OBSERVE {
                    if polls.fetch_add(1, Ordering::Relaxed) == 0 {
                        return None;
                    }
                    return Some(br#"{"version":1,"entries":{}}"#.to_vec());
                }
                assert_eq!(args["block_id"], "block:test");
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                Some(br#"{"focused":true}"#.to_vec())
            },
        );
        assert_eq!(
            monitor
                .events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            MonitorEvent::Failed
        );
        assert_eq!(
            monitor
                .events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            MonitorEvent::Snapshot(vec![])
        );
        let target = AgentId::rex("session:test", "block:test", "");
        monitor.focus_agent(target.clone());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        monitor.focus_agent(target);
        release_tx.send(()).unwrap();
        assert_eq!(
            monitor
                .focus_events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            FocusResult::Focused
        );
        assert!(
            started_rx.recv_timeout(Duration::from_millis(20)).is_err(),
            "a busy focus was queued"
        );
        monitor.focus_agent(AgentId::rex("other-session", "block:test", ""));
        assert_eq!(
            monitor
                .focus_events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            FocusResult::Missing
        );
        monitor.focus_agent(AgentId::core("foreign"));
        assert_eq!(
            monitor
                .focus_events()
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            FocusResult::Missing
        );
        let start = Instant::now();
        monitor.stop();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dropping_monitor_cancels_an_active_attempt_and_joins() {
        let (started_tx, started_rx) = mpsc::channel();
        let monitor = spawn_with(fixture_context(), POLL_INTERVAL, move |_, _, _, stop| {
            started_tx.send(()).unwrap();
            command::run(
                &mut {
                    let mut c = Command::new("/bin/sleep");
                    c.arg("10");
                    c
                },
                ATTEMPT_DEADLINE,
                1024,
                stop,
            )
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        drop(monitor);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    #[ignore = "requires an explicitly provisioned disposable Rex session"]
    fn live_rex_observes_and_focuses_disposable_session() {
        assert_eq!(std::env::var("WAVE_TUI_REX_LIVE_TEST").as_deref(), Ok("1"));
        let context =
            context_from_env().expect("REX_SERVER and REX_SESSION must name the test session");
        let monitor = spawn_monitor(context);
        let MonitorEvent::Snapshot(agents) = monitor
            .events()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            panic!("live Rex poll failed")
        };
        assert_eq!(agents.len(), 5);
        assert!(agents
            .iter()
            .all(|a| a.details.agent.as_deref() == Some("wave-tui-test")));
        for state in [
            AgentStatus::Working,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            assert!(agents.iter().any(|a| a.status == state));
        }
        monitor.focus_agent(agents[0].id.clone());
        assert_eq!(
            monitor
                .focus_events()
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            FocusResult::Focused
        );
        monitor.stop();
    }

    #[test]
    fn snapshot_maps_reported_records_without_screen_or_message_data() {
        let data = br#"{"version":1,"entries":[
            {"block_id":"b1","record_id":"","app":"pi","state":"working","name":"Main","msg":"PRIVATE"},
            {"block_id":"b1","record_id":"worker","app":"pi","state":"blocked"},
            {"block_id":"b2","record_id":"","app":"cargo","state":"error"},
            {"block_id":"b3","record_id":"","app":"  ","state":"working"}
        ]}"#;
        let agents = parse_snapshot("s1", data).unwrap();
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0].status, AgentStatus::Working);
        assert_eq!(agents[0].details.agent.as_deref(), Some("pi"));
        assert_eq!(agents[0].details.name.as_deref(), Some("Main"));
        assert_eq!(agents[0].details.activity, None);
        assert_eq!(agents[1].status, AgentStatus::Blocked);
        assert_eq!(agents[2].status, AgentStatus::Unknown);
        assert_ne!(agents[0].id, agents[1].id);
        assert_ne!(agents[0].id, parse_snapshot("s2", data).unwrap()[0].id);
        assert!(!format!("{agents:?}").contains("PRIVATE"));
    }

    #[test]
    fn empty_lua_tables_are_empty_snapshots_but_bad_envelopes_fail() {
        for data in [
            r#"{"version":1,"entries":{}}"#,
            r#"{"version":1,"entries":[]}"#,
        ] {
            assert_eq!(parse_snapshot("s", data.as_bytes()), Some(vec![]));
        }
        for data in [
            "null",
            "{}",
            r#"{"version":2,"entries":[]}"#,
            r#"{"version":1,"entries":{"unexpected":1}}"#,
            r#"{"version":1,"entries":null}"#,
        ] {
            assert!(parse_snapshot("s", data.as_bytes()).is_none(), "{data}");
        }
    }

    #[test]
    fn control_characters_in_declared_text_never_reach_the_ui() {
        let data = br#"{"version":1,"entries":[
            {"block_id":"b","record_id":"","app":"pi","name":"bad\u001b[2J"},
            {"block_id":"c","record_id":"","app":"pi\u0007"}
        ]}"#;
        let agents = parse_snapshot("s", data).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].details.name, None);
    }

    #[test]
    fn malformed_or_duplicate_identities_cannot_create_phantom_planets() {
        let data = br#"{"version":1,"entries":[
            {"block_id":"b","record_id":"","app":"pi","state":"done"},
            {"block_id":"b","record_id":"","app":"pi","state":"idle"},
            {"block_id":"b","app":"pi"},
            {"block_id":"","record_id":"","app":"pi"},
            {"block_id":"c","record_id":"","app":"pi","state":"future"}
        ]}"#;
        let agents = parse_snapshot("s", data).unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].status, AgentStatus::Done);
        assert_eq!(agents[1].status, AgentStatus::Unknown);
        assert_eq!(
            parse_snapshot("s", br#"{"version":1,"entries":{}}"#),
            Some(vec![])
        );
    }
}
