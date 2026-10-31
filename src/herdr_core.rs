//! herdr-core observation adapter.
//!
//! This module is the only place allowed to know herdr-core environment
//! variables, the Unix socket transport, JSON-RPC framing, and raw
//! `terminal.list` payloads. It is observation-only: it sends exactly
//! `terminal.list {}`, never starts a process, never subscribes, and exposes
//! no focus or rename request. Everything it publishes is typed.

mod context;

pub(crate) use context::{context_from_env, HerdrCoreContext};

use crate::model::agent::{AgentDetails, AgentId, AgentSnapshot, AgentStatus, MonitorEvent};
use serde_json::Value;
use socket2::{Domain, SockAddr, Socket, Type};
use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Wait after each completed attempt (success or failure) before the next.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Absolute budget for one attempt: connect, write, and the complete
/// response frame together. Not a per-read timeout.
const ATTEMPT_DEADLINE: Duration = Duration::from_secs(3);

/// Upper bound on one response frame including its terminating newline; a
/// larger or unterminated reply is a failed poll.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// One terminal reduced to the only fields the adapter reads. Raw values stay
/// here; nothing else from the wire is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawTerminal {
    address: String,
    name: Option<String>,
    title: Option<String>,
    state: Option<String>,
    app: Option<String>,
    status_state: Option<String>,
}

fn terminal_list_request(request_id: u64) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id.to_string(),
        "method": "terminal.list",
        "params": {},
    });
    format!("{request}\n")
}

/// A string field, or `None` when absent or not a string.
fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(str::to_owned)
}

/// Reduces one wire entry. An entry without a usable address cannot be
/// tracked and is skipped; any other malformed field simply reads as absent,
/// so one odd field never drops a terminal that is otherwise present.
fn raw_terminal(value: &Value) -> Option<RawTerminal> {
    let object = value.as_object()?;
    let address = string_field(object, "address").filter(|address| !address.trim().is_empty())?;
    let status = object.get("status").and_then(Value::as_object);
    Some(RawTerminal {
        address,
        name: string_field(object, "name"),
        title: string_field(object, "title"),
        state: string_field(object, "state"),
        app: status.and_then(|status| string_field(status, "app")),
        status_state: status.and_then(|status| string_field(status, "state")),
    })
}

/// Validates one whole `terminal.list` response envelope (version, the id of
/// the request that was sent, no `error`, a `result.terminals` array) before
/// reading any entry. `None` is a failed poll; `Some(vec![])` is a successful
/// empty snapshot.
fn parse_terminal_list(line: &str, request_id: u64) -> Option<Vec<RawTerminal>> {
    let envelope: Value = serde_json::from_str(line.trim()).ok()?;
    let envelope = envelope.as_object()?;
    if envelope.get("jsonrpc")?.as_str()? != "2.0"
        || envelope.get("id")?.as_str()? != request_id.to_string()
        || envelope.contains_key("error")
    {
        return None;
    }
    let terminals = envelope.get("result")?.as_object()?.get("terminals")?;
    Some(
        terminals
            .as_array()?
            .iter()
            .filter_map(raw_terminal)
            .collect(),
    )
}

fn nonblank(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn normalize_status(status: Option<&str>) -> AgentStatus {
    match status {
        Some("working") => AgentStatus::Working,
        Some("blocked") => AgentStatus::Blocked,
        Some("done") => AgentStatus::Done,
        Some("idle") => AgentStatus::Idle,
        _ => AgentStatus::Unknown,
    }
}

/// Reduces one successful snapshot to the agents to display. The snapshot is
/// the whole truth: only a live terminal that currently reports a nonblank
/// `status.app` is shown, so a terminal whose status was cleared (the agent
/// exited and left a shell) or that is omitted or not live is gone, and
/// nothing is remembered between polls. The full wire address is the identity
/// (`t7` and `machine/t7` differ) and the first entry for an address wins.
fn normalize_snapshot(terminals: Vec<RawTerminal>) -> Vec<AgentSnapshot> {
    let mut seen = HashSet::new();
    let mut agents = Vec::new();
    for terminal in terminals {
        if terminal.state.as_deref() != Some("live") || seen.contains(&terminal.address) {
            continue;
        }
        let Some(app) = nonblank(terminal.app.as_deref()) else {
            continue;
        };
        agents.push(AgentSnapshot {
            id: AgentId::core(terminal.address.as_str()),
            details: AgentDetails {
                name: nonblank(terminal.name.as_deref()),
                agent: Some(app),
                activity: nonblank(terminal.title.as_deref()),
            },
            status: normalize_status(terminal.status_state.as_deref()),
        });
        seen.insert(terminal.address);
    }
    agents
}

/// One bounded attempt: connect, write the request, and read one complete
/// response frame, all inside a single absolute deadline started here. Any
/// failure, including running out of time, is `None`; the next normal poll
/// is the retry.
fn request_terminal_list_within(
    context: &HerdrCoreContext,
    request_id: u64,
    budget: Duration,
) -> Option<Vec<RawTerminal>> {
    let deadline = Instant::now() + budget;
    let mut stream = connect_within(&context.socket_path, deadline)?;
    write_request_within(
        &mut stream,
        terminal_list_request(request_id).as_bytes(),
        deadline,
    )?;
    let frame = read_frame(&mut stream, deadline, MAX_RESPONSE_BYTES)?;
    parse_terminal_list(&frame, request_id)
}

/// Time left before `deadline`, or `None` once it has passed. A zero timeout
/// is invalid for socket options, so an exhausted budget never reaches them.
fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// Connects with the remaining budget as the connect timeout. A connect that
/// would block (for example a full accept queue) is polled by `socket2` only
/// until the deadline. Writability alone is not trusted: the connection must
/// report no pending error and a peer address before it is used.
fn connect_within(path: &Path, deadline: Instant) -> Option<UnixStream> {
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None).ok()?;
    let address = SockAddr::unix(path).ok()?;
    socket
        .connect_timeout(&address, remaining(deadline)?)
        .ok()?;
    if socket.take_error().ok()?.is_some() {
        return None;
    }
    socket.peer_addr().ok()?;
    Some(UnixStream::from(socket))
}

fn write_request_within(stream: &mut UnixStream, bytes: &[u8], deadline: Instant) -> Option<()> {
    let mut written = 0;
    while written < bytes.len() {
        stream.set_write_timeout(Some(remaining(deadline)?)).ok()?;
        match stream.write(&bytes[written..]) {
            Ok(0) => return None,
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
}

/// Reads one newline-terminated frame of at most `max_frame_bytes` (the
/// newline counts) before `deadline`. The read timeout is recalculated for
/// every read, so bytes trickling in cannot extend the attempt. EOF before a
/// newline, an over-limit frame, or invalid UTF-8 is `None`; bytes after the
/// first newline belong to nothing this one-shot request expects and are
/// dropped with the connection.
fn read_frame(
    stream: &mut UnixStream,
    deadline: Instant,
    max_frame_bytes: usize,
) -> Option<String> {
    let mut frame: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?)).ok()?;
        let count = match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let received = &chunk[..count];
        let (taken, complete) = match received.iter().position(|&byte| byte == b'\n') {
            Some(newline) => (newline + 1, true),
            None => (count, false),
        };
        if frame.len() + taken > max_frame_bytes {
            return None;
        }
        frame.extend_from_slice(&received[..taken]);
        if complete {
            return String::from_utf8(frame).ok();
        }
    }
}

/// Handle for the background polling thread. Dropping it (or calling
/// [`HerdrCoreMonitor::stop`]) wakes the thread and joins it.
pub(crate) struct HerdrCoreMonitor {
    events: mpsc::Receiver<MonitorEvent>,
    stop: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl HerdrCoreMonitor {
    pub(crate) fn events(&self) -> &mpsc::Receiver<MonitorEvent> {
        &self.events
    }

    pub(crate) fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HerdrCoreMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn spawn_monitor(context: HerdrCoreContext) -> HerdrCoreMonitor {
    spawn_with_interval(context, POLL_INTERVAL)
}

/// Spawns the read-only `terminal.list` polling thread. Each iteration sends
/// one request, forwards a typed snapshot or failure, then waits `interval`
/// after the attempt completed unless the stop sender is dropped first.
fn spawn_with_interval(context: HerdrCoreContext, interval: Duration) -> HerdrCoreMonitor {
    spawn_with(context, interval, ATTEMPT_DEADLINE)
}

fn spawn_with(context: HerdrCoreContext, interval: Duration, budget: Duration) -> HerdrCoreMonitor {
    let (event_tx, events) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let spawn_failure = event_tx.clone();
    let handle = thread::Builder::new()
        .name("herdr-core-monitor".to_string())
        .spawn(move || {
            let mut request_id: u64 = 0;
            loop {
                request_id += 1;
                let event = match request_terminal_list_within(&context, request_id, budget) {
                    Some(terminals) => MonitorEvent::Snapshot(normalize_snapshot(terminals)),
                    None => MonitorEvent::Failed,
                };
                if event_tx.send(event).is_err() {
                    break;
                }
                match stop_rx.recv_timeout(interval) {
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    _ => break,
                }
            }
        });
    if handle.is_err() {
        let _ = spawn_failure.send(MonitorEvent::Failed);
    }
    HerdrCoreMonitor {
        events,
        stop: Some(stop_tx),
        handle: handle.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn terminals_response(id: u64, terminals: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":"{id}","result":{{"terminals":{terminals}}}}}"#)
    }

    fn parse(terminals: &str) -> Vec<RawTerminal> {
        parse_terminal_list(&terminals_response(1, terminals), 1).expect("valid envelope")
    }

    fn raw(address: &str, state: &str, app: Option<&str>, status: Option<&str>) -> RawTerminal {
        RawTerminal {
            address: address.to_string(),
            name: None,
            title: None,
            state: Some(state.to_string()),
            app: app.map(str::to_string),
            status_state: status.map(str::to_string),
        }
    }

    fn live(address: &str, app: Option<&str>, status: Option<&str>) -> RawTerminal {
        raw(address, "live", app, status)
    }

    fn statuses(snapshots: &[AgentSnapshot]) -> Vec<(AgentId, AgentStatus)> {
        snapshots
            .iter()
            .map(|agent| (agent.id.clone(), agent.status))
            .collect()
    }

    // --- request ---

    #[test]
    fn poll_constants_match_the_design_contract() {
        assert_eq!(POLL_INTERVAL, Duration::from_secs(5));
        assert_eq!(ATTEMPT_DEADLINE, Duration::from_secs(3));
    }

    #[test]
    fn request_is_one_line_jsonrpc_terminal_list_with_empty_params() {
        let request = terminal_list_request(7);
        assert!(request.ends_with('\n'));
        let body = request.trim_end_matches('\n');
        assert!(!body.contains('\n'));
        let value: Value = serde_json::from_str(body).expect("request must be JSON");
        assert_eq!(
            value,
            json!({"jsonrpc":"2.0","id":"7","method":"terminal.list","params":{}})
        );
    }

    // --- envelope ---

    #[test]
    fn valid_empty_snapshot_is_distinct_from_a_malformed_result() {
        assert_eq!(parse("[]"), Vec::new());
        for line in [
            r#"{"jsonrpc":"2.0","id":"1","result":{}}"#,
            r#"{"jsonrpc":"2.0","id":"1","result":{"terminals":null}}"#,
            r#"{"jsonrpc":"2.0","id":"1","result":{"terminals":{}}}"#,
            r#"{"jsonrpc":"2.0","id":"1","result":[]}"#,
            r#"{"jsonrpc":"2.0","id":"1","result":null}"#,
            r#"{"jsonrpc":"2.0","id":"1"}"#,
            r#"{"jsonrpc":"2.0","id":"1","result":{"agents":[]}}"#,
        ] {
            assert_eq!(parse_terminal_list(line, 1), None, "{line}");
        }
    }

    #[test]
    fn envelope_rejects_garbage_error_wrong_version_and_id_mismatch() {
        let terminals = r#""terminals":[]"#;
        for line in [
            String::new(),
            "not json".to_string(),
            "[]".to_string(),
            "null".to_string(),
            format!(r#"{{"jsonrpc":"1.0","id":"1","result":{{{terminals}}}}}"#),
            format!(r#"{{"id":"1","result":{{{terminals}}}}}"#),
            format!(r#"{{"jsonrpc":"2.0","result":{{{terminals}}}}}"#),
            format!(r#"{{"jsonrpc":"2.0","id":"2","result":{{{terminals}}}}}"#),
            format!(r#"{{"jsonrpc":"2.0","id":1,"result":{{{terminals}}}}}"#),
            format!(r#"{{"jsonrpc":"2.0","id":null,"result":{{{terminals}}}}}"#),
            r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32601,"message":"nope"}}"#.to_string(),
            format!(
                r#"{{"jsonrpc":"2.0","id":"1","result":{{{terminals}}},"error":{{"code":1,"message":"x"}}}}"#
            ),
        ] {
            assert_eq!(parse_terminal_list(&line, 1), None, "{line}");
        }
    }

    #[test]
    fn id_is_matched_against_the_request_that_was_sent() {
        let line = terminals_response(41, "[]");
        assert_eq!(parse_terminal_list(&line, 41), Some(Vec::new()));
        assert_eq!(parse_terminal_list(&line, 42), None);
    }

    // --- entry mapping ---

    #[test]
    fn entries_keep_only_the_allowed_fields() {
        let terminals = parse(
            r#"[{"address":"t7","name":"  planner  ","title":"  Fix the build  ","state":"live",
                "status":{"app":"  claude  ","state":"working","title":"hidden","msg":"secret","kind":"k","progress":0.5},
                "foreground":"vim","parent":"t1","reported":true,"cwd":"/private","argv":["x"],"output":"leak","labels":{"a":"b"}}]"#,
        );
        assert_eq!(
            terminals,
            vec![RawTerminal {
                address: "t7".to_string(),
                name: Some("  planner  ".to_string()),
                title: Some("  Fix the build  ".to_string()),
                state: Some("live".to_string()),
                app: Some("  claude  ".to_string()),
                status_state: Some("working".to_string()),
            }]
        );
    }

    #[test]
    fn malformed_entries_are_skipped_without_failing_the_poll() {
        let terminals = parse(
            r#"[1,"x",null,[],{},{"address":7},{"address":""},{"address":"   "},
                {"address":"ok","state":"live","status":{"app":"a","state":"idle"}},
                {"address":"odd","state":5,"title":6,"name":false,"status":"nope"}]"#,
        );
        let addresses: Vec<_> = terminals.iter().map(|t| t.address.as_str()).collect();
        assert_eq!(addresses, vec!["ok", "odd"]);
        assert_eq!(terminals[1].state, None);
        assert_eq!(terminals[1].app, None);
        assert_eq!(terminals[1].title, None);
    }

    #[test]
    fn addresses_are_kept_exactly_so_local_and_remote_t7_stay_distinct() {
        let agents = normalize_snapshot(parse(
            r#"[{"address":"t7","state":"live","status":{"app":"a","state":"idle"}},
                {"address":"box/t7","state":"live","status":{"app":"a","state":"idle"}}]"#,
        ));
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, AgentId::core("t7"));
        assert_eq!(agents[1].id, AgentId::core("box/t7"));
        assert_ne!(agents[0].id, agents[1].id);
    }

    #[test]
    fn fixture_maps_name_agent_activity_and_status() {
        let agents = normalize_snapshot(parse(
            r#"[{"address":"t1","name":"  planner ","title":" Fix the build ","state":"live",
                 "status":{"app":" claude ","state":"working","title":"STATUS TITLE"}}]"#,
        ));
        assert_eq!(
            agents,
            vec![AgentSnapshot {
                id: AgentId::core("t1"),
                details: AgentDetails {
                    name: Some("planner".to_string()),
                    agent: Some("claude".to_string()),
                    activity: Some("Fix the build".to_string()),
                },
                status: AgentStatus::Working,
            }]
        );
    }

    #[test]
    fn blank_name_is_none_and_never_falls_back_to_title_or_address() {
        for name in [None, Some(""), Some("   ")] {
            let mut terminal = live("t1", Some("claude"), Some("idle"));
            terminal.name = name.map(str::to_string);
            terminal.title = Some("A title".to_string());
            let agents = normalize_snapshot(vec![terminal]);
            assert_eq!(agents[0].details.name, None, "{name:?}");
            assert_eq!(agents[0].details.activity.as_deref(), Some("A title"));
        }
    }

    #[test]
    fn blank_title_is_no_activity() {
        let mut terminal = live("t1", Some("claude"), Some("idle"));
        terminal.title = Some("  ".to_string());
        assert_eq!(normalize_snapshot(vec![terminal])[0].details.activity, None);
    }

    #[test]
    fn status_state_maps_directly_and_everything_else_is_unknown() {
        for (state, expected) in [
            (Some("working"), AgentStatus::Working),
            (Some("blocked"), AgentStatus::Blocked),
            (Some("done"), AgentStatus::Done),
            (Some("idle"), AgentStatus::Idle),
            (Some("error"), AgentStatus::Unknown),
            (Some("Working"), AgentStatus::Unknown),
            (Some("bogus"), AgentStatus::Unknown),
            (Some(""), AgentStatus::Unknown),
            (None, AgentStatus::Unknown),
        ] {
            let agents = normalize_snapshot(vec![live("t1", Some("app"), state)]);
            assert_eq!(agents[0].status, expected, "{state:?}");
        }
    }

    // --- admission ---

    #[test]
    fn only_live_terminals_with_a_nonblank_app_are_admitted() {
        let agents = normalize_snapshot(vec![
            live("t1", Some("claude"), Some("idle")),
            live("t2", None, None),
            live("t3", Some(""), Some("working")),
            live("t4", Some("   "), Some("working")),
            raw("t5", "finished", Some("claude"), Some("done")),
            raw("t6", "starting", Some("claude"), Some("working")),
            RawTerminal {
                state: None,
                ..live("t7", Some("claude"), Some("working"))
            },
            live("t8", Some("tool"), Some("working")),
        ]);
        let ids: Vec<_> = agents.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids, vec![AgentId::core("t1"), AgentId::core("t8")]);
    }

    #[test]
    fn lifecycle_state_is_not_agent_status() {
        let agents = normalize_snapshot(vec![live("t1", Some("claude"), None)]);
        assert_eq!(agents[0].status, AgentStatus::Unknown);
    }

    #[test]
    fn name_title_or_presence_never_admit_a_terminal() {
        let mut terminal = live("t1", None, None);
        terminal.name = Some("claude".to_string());
        terminal.title = Some("Claude Code".to_string());
        assert!(normalize_snapshot(vec![terminal]).is_empty());
    }

    // --- snapshot is the whole truth (MIK-078) ---

    #[test]
    fn cleared_status_removes_the_terminal_and_a_later_app_report_returns_it() {
        let shown = normalize_snapshot(vec![live("t1", Some("claude"), Some("working"))]);
        assert_eq!(shown[0].details.agent.as_deref(), Some("claude"));

        let mut cleared = live("t1", None, None);
        cleared.title = Some("now".to_string());
        assert!(normalize_snapshot(vec![cleared]).is_empty());

        let back = normalize_snapshot(vec![live("t1", Some("claude"), Some("idle"))]);
        assert_eq!(back[0].status, AgentStatus::Idle);
        assert_eq!(back[0].details.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn omission_and_nonlive_state_remove_the_terminal() {
        assert!(normalize_snapshot(Vec::new()).is_empty());
        assert!(
            normalize_snapshot(vec![raw("t1", "finished", Some("claude"), Some("done"))])
                .is_empty()
        );
    }

    // --- current-app-only display (MIK-078) ---

    const TRACE_WORKING: &str = r#"[{"address":"t1","state":"live","reported":true,"title":"T","status":{"app":"claude","state":"working"}}]"#;
    const TRACE_DONE: &str = r#"[{"address":"t1","state":"live","reported":true,"status":{"app":"claude","state":"done"}}]"#;
    const TRACE_SHELL: &str = r#"[{"address":"t1","state":"live","reported":false,"status":null}]"#;
    const TRACE_RESTARTED: &str = r#"[{"address":"t1","state":"live","reported":true,"status":{"app":"claude","state":"working"}}]"#;

    #[test]
    fn agent_exit_trace_removes_the_planet_while_the_pane_stays_live() {
        let working = normalize_snapshot(parse(TRACE_WORKING));
        assert_eq!(
            statuses(&working),
            vec![(AgentId::core("t1"), AgentStatus::Working)]
        );
        // Turn completion: Done with the app still reported stays.
        let done = normalize_snapshot(parse(TRACE_DONE));
        assert_eq!(
            statuses(&done),
            vec![(AgentId::core("t1"), AgentStatus::Done)]
        );
        // The agent exits; the terminal stays live with no status. Repeatedly.
        for poll in 0..3 {
            assert!(
                normalize_snapshot(parse(TRACE_SHELL)).is_empty(),
                "post-exit poll {poll} must not re-admit the stale planet"
            );
        }
        // A fresh app report in the same terminal re-admits it.
        let restarted = normalize_snapshot(parse(TRACE_RESTARTED));
        assert_eq!(
            statuses(&restarted),
            vec![(AgentId::core("t1"), AgentStatus::Working)]
        );
        assert_eq!(restarted[0].details.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn missing_blank_or_wrong_typed_app_removes_a_previously_shown_agent_whatever_the_state() {
        for terminal in [
            r#"{"address":"t1","state":"live","status":{"state":"working"}}"#,
            r#"{"address":"t1","state":"live","status":{"app":null,"state":"working"}}"#,
            r#"{"address":"t1","state":"live","status":{"app":"","state":"done"}}"#,
            r#"{"address":"t1","state":"live","status":{"app":"   ","state":"working"}}"#,
            r#"{"address":"t1","state":"live","status":{"app":5,"state":"working"}}"#,
            r#"{"address":"t1","state":"live","status":{"app":["claude"],"state":"done"}}"#,
            r#"{"address":"t1","state":"live","reported":true,"status":null}"#,
            r#"{"address":"t1","state":"live","reported":true}"#,
        ] {
            let agents = normalize_snapshot(parse(&format!("[{terminal}]")));
            assert!(agents.is_empty(), "{terminal}");
        }
    }

    #[test]
    fn current_unknown_status_with_an_app_stays_as_unknown() {
        for status in ["error", "bogus", ""] {
            let line = format!(
                r#"[{{"address":"t1","state":"live","status":{{"app":"claude","state":"{status}"}}}}]"#
            );
            let agents = normalize_snapshot(parse(&line));
            assert_eq!(
                statuses(&agents),
                vec![(AgentId::core("t1"), AgentStatus::Unknown)],
                "{status}"
            );
        }
    }

    #[test]
    fn monitor_trace_over_the_socket_removes_the_exited_agent_and_readmits_a_restart() {
        let socket = SocketPath::new();
        let (_requests, server) = fake_core(
            &socket,
            vec![
                Behavior::Terminals(TRACE_WORKING),
                Behavior::Terminals(TRACE_DONE),
                Behavior::Terminals(TRACE_SHELL),
                Behavior::Terminals(TRACE_SHELL),
                Behavior::Terminals(TRACE_RESTARTED),
            ],
        );
        let monitor = spawn_with_interval(socket.context(), Duration::from_millis(5));
        let mut seen = Vec::new();
        for _ in 0..5 {
            let MonitorEvent::Snapshot(agents) = next_event(&monitor) else {
                panic!("every scripted poll is a successful snapshot");
            };
            seen.push(statuses(&agents));
        }
        let t1 = AgentId::core("t1");
        assert_eq!(
            seen,
            vec![
                vec![(t1.clone(), AgentStatus::Working)],
                vec![(t1.clone(), AgentStatus::Done)],
                Vec::new(),
                Vec::new(),
                vec![(t1, AgentStatus::Working)],
            ]
        );
        monitor.stop();
        server.join().unwrap();
    }

    #[test]
    fn a_failed_poll_between_valid_snapshots_is_failed_not_an_empty_success() {
        let socket = SocketPath::new();
        let (_requests, server) = fake_core(
            &socket,
            vec![
                Behavior::Terminals(TRACE_WORKING),
                Behavior::Hangup,
                Behavior::RpcError,
                Behavior::Terminals(TRACE_SHELL),
                Behavior::Terminals(TRACE_RESTARTED),
            ],
        );
        let monitor = spawn_with_interval(socket.context(), Duration::from_millis(5));
        let t1 = AgentId::core("t1");
        assert_eq!(
            next_event(&monitor),
            MonitorEvent::Snapshot(vec![AgentSnapshot {
                id: t1.clone(),
                details: AgentDetails {
                    name: None,
                    agent: Some("claude".to_string()),
                    activity: Some("T".to_string()),
                },
                status: AgentStatus::Working,
            }])
        );
        assert_eq!(next_event(&monitor), MonitorEvent::Failed);
        assert_eq!(next_event(&monitor), MonitorEvent::Failed);
        assert_eq!(next_event(&monitor), MonitorEvent::Snapshot(Vec::new()));
        let MonitorEvent::Snapshot(restarted) = next_event(&monitor) else {
            panic!("restart poll should succeed");
        };
        assert_eq!(statuses(&restarted), vec![(t1, AgentStatus::Working)]);
        monitor.stop();
        server.join().unwrap();
    }

    #[test]
    fn duplicate_addresses_in_one_snapshot_yield_one_agent() {
        let agents = normalize_snapshot(vec![
            live("t1", Some("claude"), Some("idle")),
            live("t1", Some("other"), Some("working")),
        ]);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].details.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn remote_and_local_addresses_are_removed_independently() {
        let agents = normalize_snapshot(vec![
            live("box/t7", None, None),
            live("t7", Some("claude"), Some("idle")),
        ]);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, AgentId::core("t7"));
        let agents = normalize_snapshot(vec![
            live("box/t7", Some("claude"), Some("idle")),
            live("t7", None, None),
        ]);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, AgentId::core("box/t7"));
    }

    // --- fake socket transport ---

    static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

    struct SocketPath(PathBuf);

    impl SocketPath {
        fn new() -> Self {
            let n = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!("wtc-{}-{n}.sock", std::process::id())))
        }

        fn context(&self) -> HerdrCoreContext {
            HerdrCoreContext {
                socket_path: self.0.clone(),
            }
        }
    }

    impl Drop for SocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// What a fake core does with one accepted connection.
    #[derive(Clone)]
    enum Behavior {
        /// Reply with this terminals array, echoing the request id.
        Terminals(&'static str),
        /// Reply with a JSON-RPC error.
        RpcError,
        /// Close without replying.
        Hangup,
    }

    /// Serves the scripted behaviors in order, one per connection, and sends
    /// every raw request line to the returned receiver.
    fn fake_core(
        path: &SocketPath,
        script: Vec<Behavior>,
    ) -> (mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = UnixListener::bind(&path.0).expect("bind fake core");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            for behavior in script {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: Value = serde_json::from_str(line.trim_end()).unwrap_or(Value::Null);
                let _ = tx.send(line.clone());
                let id = request["id"].clone();
                let reply = match behavior {
                    Behavior::Terminals(terminals) => format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"result":{{"terminals":{terminals}}}}}"#
                    ),
                    Behavior::RpcError => format!(
                        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32000,"message":"boom"}}}}"#
                    ),
                    Behavior::Hangup => continue,
                };
                let _ = reader.get_mut().write_all(format!("{reply}\n").as_bytes());
            }
        });
        (rx, handle)
    }

    const AGENT_WORKING: &str = r#"[{"address":"t1","state":"live","title":"T","status":{"app":"claude","state":"working"}}]"#;
    const AGENT_CLEARED: &str = r#"[{"address":"t1","state":"live","status":null}]"#;

    fn next_event(monitor: &HerdrCoreMonitor) -> MonitorEvent {
        monitor
            .events()
            .recv_timeout(Duration::from_secs(10))
            .expect("monitor event")
    }

    #[test]
    fn request_hits_the_wire_as_exact_terminal_list_and_maps_the_reply() {
        let socket = SocketPath::new();
        let (requests, server) = fake_core(&socket, vec![Behavior::Terminals(AGENT_WORKING)]);

        let terminals =
            request_terminal_list_within(&socket.context(), 9, ATTEMPT_DEADLINE).expect("success");
        let line = requests.recv().expect("request seen");
        assert!(line.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(line.trim_end()).unwrap(),
            json!({"jsonrpc":"2.0","id":"9","method":"terminal.list","params":{}})
        );
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].address, "t1");
        server.join().unwrap();
    }

    #[test]
    fn rpc_error_hangup_and_missing_socket_are_recoverable_failures() {
        let socket = SocketPath::new();
        let (_requests, server) = fake_core(&socket, vec![Behavior::RpcError, Behavior::Hangup]);
        assert_eq!(
            request_terminal_list_within(&socket.context(), 1, ATTEMPT_DEADLINE),
            None
        );
        assert_eq!(
            request_terminal_list_within(&socket.context(), 2, ATTEMPT_DEADLINE),
            None
        );
        server.join().unwrap();

        let missing = SocketPath::new();
        assert_eq!(
            request_terminal_list_within(&missing.context(), 1, ATTEMPT_DEADLINE),
            None
        );
    }

    #[test]
    fn monitor_polls_immediately_and_recovers_after_a_failure_with_the_current_snapshot() {
        let socket = SocketPath::new();
        let (requests, server) = fake_core(
            &socket,
            vec![
                Behavior::Terminals(AGENT_WORKING),
                Behavior::Hangup,
                Behavior::Terminals(AGENT_CLEARED),
            ],
        );
        let monitor = spawn_with_interval(socket.context(), Duration::from_millis(5));

        let MonitorEvent::Snapshot(first) = next_event(&monitor) else {
            panic!("first poll should succeed immediately");
        };
        assert_eq!(first[0].status, AgentStatus::Working);
        assert_eq!(first[0].details.agent.as_deref(), Some("claude"));

        assert_eq!(next_event(&monitor), MonitorEvent::Failed);

        let MonitorEvent::Snapshot(third) = next_event(&monitor) else {
            panic!("monitor should recover");
        };
        assert_eq!(third, Vec::new(), "the cleared terminal is gone");

        let methods: Vec<String> = requests
            .try_iter()
            .map(|line| serde_json::from_str::<Value>(&line).unwrap()["method"].to_string())
            .collect();
        assert!(
            methods.iter().all(|m| m == "\"terminal.list\""),
            "{methods:?}"
        );
        monitor.stop();
        server.join().unwrap();
    }

    #[test]
    fn stop_wakes_an_idle_monitor_and_joins() {
        let socket = SocketPath::new();
        let (_requests, server) = fake_core(&socket, vec![Behavior::Terminals("[]")]);
        let monitor = spawn_with_interval(socket.context(), Duration::from_secs(3600));
        assert!(matches!(next_event(&monitor), MonitorEvent::Snapshot(_)));

        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            monitor.stop();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("stop must wake the idle wait instead of sleeping the interval");
        server.join().unwrap();
    }

    #[test]
    fn drop_wakes_an_idle_monitor_and_joins() {
        let socket = SocketPath::new();
        let (_requests, server) = fake_core(&socket, vec![Behavior::Terminals("[]")]);
        let monitor = spawn_with_interval(socket.context(), Duration::from_secs(3600));
        assert!(matches!(next_event(&monitor), MonitorEvent::Snapshot(_)));

        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            drop(monitor);
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("drop must wake the idle wait");
        server.join().unwrap();
    }

    // --- attempt deadline and framing ---

    /// A peer that misbehaves on purpose. Dropping it releases the peer and
    /// joins its thread, so a failing test never leaves a blocked thread.
    struct Peer {
        stop: Option<mpsc::Sender<()>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            self.stop.take();
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Misbehavior {
        /// Read the request, then hold the connection open and silent.
        Stall,
        /// Read the request, then send one byte every few milliseconds with
        /// no newline, for as long as the client stays connected.
        Trickle,
    }

    fn misbehaving_core(path: &SocketPath, behavior: Misbehavior) -> Peer {
        let listener = UnixListener::bind(&path.0).expect("bind fake core");
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().expect("clone")).read_line(&mut line);
            loop {
                match stop_rx.recv_timeout(Duration::from_millis(10)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
                if matches!(behavior, Misbehavior::Trickle) && stream.write_all(b" ").is_err() {
                    return;
                }
            }
        });
        Peer {
            stop: Some(stop_tx),
            handle: Some(handle),
        }
    }

    /// Runs `work` on a helper thread and gives up waiting after `limit`, so a
    /// transport that never returns fails the test instead of hanging it. The
    /// helper ends when the caller drops the misbehaving peer.
    fn within<T: Send + 'static>(
        limit: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(limit).ok()
    }

    const SHORT_BUDGET: Duration = Duration::from_millis(300);
    /// Generous scheduler slack: far below "never", above any plausible delay.
    const SLACK: Duration = Duration::from_secs(5);

    #[test]
    fn stalled_reply_fails_within_the_attempt_budget() {
        let socket = SocketPath::new();
        let _peer = misbehaving_core(&socket, Misbehavior::Stall);
        let context = socket.context();
        let started = Instant::now();
        let result = within(SHORT_BUDGET + SLACK, move || {
            request_terminal_list_within(&context, 1, SHORT_BUDGET)
        });
        assert_eq!(result, Some(None), "must fail, not hang");
        assert!(started.elapsed() >= SHORT_BUDGET / 2);
    }

    #[test]
    fn trickled_bytes_cannot_extend_the_attempt_past_the_budget() {
        let socket = SocketPath::new();
        let _peer = misbehaving_core(&socket, Misbehavior::Trickle);
        let context = socket.context();
        let result = within(SHORT_BUDGET + SLACK, move || {
            request_terminal_list_within(&context, 1, SHORT_BUDGET)
        });
        assert_eq!(
            result,
            Some(None),
            "a peer sending a byte inside every read timeout must still hit the total deadline"
        );
    }

    #[test]
    fn stop_during_a_trickling_attempt_waits_only_the_remaining_budget() {
        let socket = SocketPath::new();
        let _peer = misbehaving_core(&socket, Misbehavior::Trickle);
        let monitor = spawn_with(socket.context(), Duration::from_secs(3600), SHORT_BUDGET);
        let started = Instant::now();
        let stopped = within(SHORT_BUDGET + SLACK, move || monitor.stop());
        assert!(stopped.is_some(), "stop must not wait on a trickling peer");
        assert!(started.elapsed() < SHORT_BUDGET + SLACK);
    }

    #[test]
    fn drop_during_a_stalled_attempt_waits_only_the_remaining_budget() {
        let socket = SocketPath::new();
        let _peer = misbehaving_core(&socket, Misbehavior::Stall);
        let monitor = spawn_with(socket.context(), Duration::from_secs(3600), SHORT_BUDGET);
        let dropped = within(SHORT_BUDGET + SLACK, move || drop(monitor));
        assert!(dropped.is_some(), "drop must not wait the poll interval");
    }

    #[test]
    fn a_timed_out_attempt_is_reported_as_a_plain_failure() {
        let socket = SocketPath::new();
        let _peer = misbehaving_core(&socket, Misbehavior::Stall);
        let monitor = spawn_with(socket.context(), Duration::from_secs(3600), SHORT_BUDGET);
        let event = monitor
            .events()
            .recv_timeout(SHORT_BUDGET + SLACK)
            .expect("timed out attempt must still report");
        assert_eq!(event, MonitorEvent::Failed);
        monitor.stop();
    }

    fn frame_over_pair(written: Vec<u8>, keep_open: bool, max: usize) -> Option<String> {
        let (mut client, mut server) = UnixStream::pair().expect("pair");
        let writer = thread::spawn(move || {
            let _ = server.write_all(&written);
            if keep_open {
                // Hold the peer open until the client gives up and closes.
                let mut sink = [0_u8; 1];
                let _ = server.read(&mut sink);
            }
        });
        let result = read_frame(&mut client, Instant::now() + SHORT_BUDGET, max);
        drop(client);
        writer.join().expect("writer");
        result
    }

    #[test]
    fn frame_requires_a_terminating_newline() {
        assert_eq!(
            frame_over_pair(b"{\"a\":1}\n".to_vec(), false, 64).as_deref(),
            Some("{\"a\":1}\n")
        );
        assert_eq!(frame_over_pair(b"{\"a\":1}".to_vec(), false, 64), None);
        assert_eq!(frame_over_pair(Vec::new(), false, 64), None);
    }

    #[test]
    fn frame_limit_counts_the_newline() {
        let mut exact = vec![b'x'; 63];
        exact.push(b'\n');
        assert_eq!(exact.len(), 64);
        assert_eq!(
            frame_over_pair(exact.clone(), false, 64).map(|f| f.len()),
            Some(64)
        );

        let mut over = vec![b'x'; 64];
        over.push(b'\n');
        assert_eq!(frame_over_pair(over, false, 64), None);
    }

    #[test]
    fn unterminated_input_at_the_limit_is_rejected_even_at_eof() {
        assert_eq!(frame_over_pair(vec![b'x'; 64], false, 64), None);
        assert_eq!(frame_over_pair(vec![b'x'; 200], true, 64), None);
    }

    #[test]
    fn valid_json_prefix_with_padding_and_garbage_past_the_limit_is_rejected() {
        let mut bytes = br#"{"jsonrpc":"2.0","id":"1","result":{"terminals":[]}}"#.to_vec();
        bytes.resize(64, b' ');
        bytes.extend_from_slice(b"garbage that never ends in a newline");
        assert_eq!(frame_over_pair(bytes, true, 64), None);
    }

    #[test]
    fn bytes_after_the_first_newline_are_ignored() {
        let frame = frame_over_pair(b"first\nsecond\n".to_vec(), false, 64);
        assert_eq!(frame.as_deref(), Some("first\n"));
    }

    #[test]
    fn non_utf8_frame_is_rejected() {
        assert_eq!(frame_over_pair(vec![0xff, 0xfe, b'\n'], false, 64), None);
    }

    /// A listener whose accept queue is full must produce a bounded failure,
    /// not a blocked connect. Linux-only: it relies on a non-blocking AF_UNIX
    /// connect to a full queue returning EAGAIN; macOS reports ECONNREFUSED
    /// instead, so this is compiled out there rather than run.
    #[cfg(target_os = "linux")]
    #[test]
    fn saturated_accept_backlog_fails_within_the_budget() {
        use socket2::{Domain, SockAddr, Socket, Type};

        let socket = SocketPath::new();
        let listener = Socket::new(Domain::UNIX, Type::STREAM, None).expect("socket");
        listener
            .bind(&SockAddr::unix(&socket.0).expect("addr"))
            .expect("bind");
        listener.listen(0).expect("listen");

        // Never accept; fill the queue with non-blocking connects.
        let mut fillers = Vec::new();
        let mut saturated = false;
        for _ in 0..16 {
            let filler = Socket::new(Domain::UNIX, Type::STREAM, None).expect("socket");
            filler.set_nonblocking(true).expect("nonblocking");
            match filler.connect(&SockAddr::unix(&socket.0).expect("addr")) {
                Ok(()) => fillers.push(filler),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    saturated = true;
                    break;
                }
                Err(error) => panic!("unexpected fill error: {error}"),
            }
        }
        assert!(saturated, "backlog never filled");

        let context = socket.context();
        let result = within(SHORT_BUDGET + SLACK, move || {
            request_terminal_list_within(&context, 1, SHORT_BUDGET)
        });
        assert_eq!(result, Some(None));
        drop(fillers);
        drop(listener);
    }

    #[test]
    fn unavailable_socket_reports_failed_without_panicking() {
        let socket = SocketPath::new();
        let monitor = spawn_with_interval(socket.context(), Duration::from_secs(3600));
        assert_eq!(next_event(&monitor), MonitorEvent::Failed);
        monitor.stop();
    }

    #[test]
    fn default_monitor_reports_failed_for_an_unreachable_socket_and_stops() {
        let socket = SocketPath::new();
        let monitor = spawn_monitor(socket.context());
        assert_eq!(next_event(&monitor), MonitorEvent::Failed);
        monitor.stop();
    }
}
