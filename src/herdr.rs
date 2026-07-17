//! Herdr plugin integration adapter.
//!
//! This module is the only place allowed to know Herdr environment
//! variables, the Unix socket transport, JSON-RPC framing, and raw
//! `agent.list` payloads. Everything it exposes is typed.

// Temporary dead-code allowance: nothing consumes this adapter until the
// monitor is wired into the cli event loop and app reducer.
#![allow(dead_code)]

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(15);

const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HerdrContext {
    pub socket_path: PathBuf,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStatus {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSnapshot {
    pub pane_id: String,
    pub agent: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MonitorEvent {
    Snapshot(Vec<AgentSnapshot>),
    Failed,
}

/// Reads plugin eligibility from the process environment. Returns `None`
/// for every standalone, incomplete, or explicitly disabled launch.
pub(crate) fn context_from_env(disabled: bool) -> Option<HerdrContext> {
    context_from_vars(
        disabled,
        std::env::var("HERDR_ENV").ok(),
        std::env::var("HERDR_SOCKET_PATH").ok(),
        std::env::var("HERDR_WORKSPACE_ID").ok(),
    )
}

fn context_from_vars(
    disabled: bool,
    herdr_env: Option<String>,
    socket_path: Option<String>,
    workspace_id: Option<String>,
) -> Option<HerdrContext> {
    if disabled {
        return None;
    }
    if herdr_env.as_deref() != Some("1") {
        return None;
    }
    let socket_path = socket_path.filter(|path| !path.is_empty())?;
    let workspace_id = workspace_id.filter(|id| !id.is_empty())?;
    Some(HerdrContext {
        socket_path: PathBuf::from(socket_path),
        workspace_id,
    })
}

#[derive(Deserialize)]
struct RawResponse {
    result: Option<RawResult>,
}

#[derive(Deserialize)]
struct RawResult {
    agents: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct RawAgent {
    pane_id: String,
    workspace_id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    agent_status: Option<String>,
}

/// Parses one `agent.list` response line and keeps only agents in the
/// requested workspace. Returns `None` when the framing, `result`, or
/// `agents` array is malformed; entries missing required ids are dropped.
fn parse_agent_list(line: &str, workspace_id: &str) -> Option<Vec<AgentSnapshot>> {
    let response: RawResponse = serde_json::from_str(line).ok()?;
    let agents = response.result?.agents?;
    Some(
        agents
            .into_iter()
            .filter_map(|value| serde_json::from_value::<RawAgent>(value).ok())
            .filter(|raw| raw.workspace_id == workspace_id)
            .map(|raw| AgentSnapshot {
                pane_id: raw.pane_id,
                agent: raw.agent,
                name: raw.name,
                cwd: raw.cwd,
                status: normalize_status(raw.agent_status.as_deref()),
            })
            .collect(),
    )
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

fn agent_list_request(request_id: u64) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id.to_string(),
        "method": "agent.list",
        "params": {},
    });
    format!("{request}\n")
}

fn request_agent_list(context: &HerdrContext, request_id: u64) -> Option<Vec<AgentSnapshot>> {
    let mut stream = UnixStream::connect(&context.socket_path).ok()?;
    stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT)).ok()?;
    stream
        .write_all(agent_list_request(request_id).as_bytes())
        .ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line).ok()?;
    if read == 0 {
        return None;
    }
    parse_agent_list(line.trim_end(), &context.workspace_id)
}

fn poll_once(context: &HerdrContext, request_id: u64) -> MonitorEvent {
    match request_agent_list(context, request_id) {
        Some(agents) => MonitorEvent::Snapshot(agents),
        None => MonitorEvent::Failed,
    }
}

/// Handle for the background polling thread. Dropping it (or calling
/// [`HerdrMonitor::stop`]) wakes the thread and joins it.
pub(crate) struct HerdrMonitor {
    events: mpsc::Receiver<MonitorEvent>,
    stop: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl HerdrMonitor {
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

impl Drop for HerdrMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Spawns the read-only `agent.list` polling thread. Each iteration sends
/// one request, forwards a typed snapshot or failure, then sleeps for
/// [`POLL_INTERVAL`] unless the stop sender is dropped first.
pub(crate) fn spawn_monitor(context: HerdrContext) -> HerdrMonitor {
    let (event_tx, event_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let mut request_id: u64 = 0;
        loop {
            request_id += 1;
            if event_tx.send(poll_once(&context, request_id)).is_err() {
                break;
            }
            match stop_rx.recv_timeout(POLL_INTERVAL) {
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                _ => break,
            }
        }
    });
    HerdrMonitor {
        events: event_rx,
        stop: Some(stop_tx),
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    const WORKSPACE: &str = "ws-1";

    fn eligible_vars() -> (Option<String>, Option<String>, Option<String>) {
        (
            Some("1".to_string()),
            Some("/tmp/herdr.sock".to_string()),
            Some(WORKSPACE.to_string()),
        )
    }

    #[test]
    fn polling_constants_match_the_design_contract() {
        assert_eq!(POLL_INTERVAL, Duration::from_secs(5));
        assert_eq!(STALE_AFTER, Duration::from_secs(15));
    }

    #[test]
    fn context_accepts_exact_plugin_environment() {
        let (herdr_env, socket, workspace) = eligible_vars();
        let context = context_from_vars(false, herdr_env, socket, workspace)
            .expect("exact plugin environment should be eligible");
        assert_eq!(context.socket_path, PathBuf::from("/tmp/herdr.sock"));
        assert_eq!(context.workspace_id, WORKSPACE);
    }

    #[test]
    fn context_rejects_missing_or_inexact_herdr_env() {
        let candidates = [
            None,
            Some(String::new()),
            Some("0".to_string()),
            Some("true".to_string()),
            Some("1 ".to_string()),
        ];
        for herdr_env in candidates {
            let (_, socket, workspace) = eligible_vars();
            assert!(
                context_from_vars(false, herdr_env.clone(), socket, workspace).is_none(),
                "HERDR_ENV {herdr_env:?} should be ineligible"
            );
        }
    }

    #[test]
    fn context_rejects_missing_or_empty_socket_path() {
        for socket in [None, Some(String::new())] {
            let (herdr_env, _, workspace) = eligible_vars();
            assert!(
                context_from_vars(false, herdr_env, socket.clone(), workspace).is_none(),
                "socket path {socket:?} should be ineligible"
            );
        }
    }

    #[test]
    fn context_rejects_missing_or_empty_workspace_id() {
        for workspace in [None, Some(String::new())] {
            let (herdr_env, socket, _) = eligible_vars();
            assert!(
                context_from_vars(false, herdr_env, socket, workspace.clone()).is_none(),
                "workspace {workspace:?} should be ineligible"
            );
        }
    }

    #[test]
    fn context_rejects_disabled_launch_even_when_eligible() {
        let (herdr_env, socket, workspace) = eligible_vars();
        assert!(context_from_vars(true, herdr_env, socket, workspace).is_none());
    }

    #[test]
    fn request_is_single_line_jsonrpc_agent_list() {
        let request = agent_list_request(7);
        assert!(request.ends_with('\n'));
        let body = request.trim_end_matches('\n');
        assert!(!body.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(body).expect("request must be JSON");
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "7");
        assert_eq!(value["method"], "agent.list");
        assert_eq!(value["params"], serde_json::json!({}));
    }

    #[test]
    fn parses_and_filters_current_workspace_agents() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"ws-1","agent":"claude","name":"impl","cwd":"~/repo","agent_status":"working"},"#,
            r#"{"pane_id":"p2","workspace_id":"ws-2","agent_status":"idle"},"#,
            r#"{"pane_id":"p3","workspace_id":"ws-1","agent_status":"blocked"}"#,
            r#"]}}"#,
        );
        let agents = parse_agent_list(line, WORKSPACE).expect("valid payload");
        assert_eq!(
            agents,
            vec![
                AgentSnapshot {
                    pane_id: "p1".to_string(),
                    agent: Some("claude".to_string()),
                    name: Some("impl".to_string()),
                    cwd: Some("~/repo".to_string()),
                    status: AgentStatus::Working,
                },
                AgentSnapshot {
                    pane_id: "p3".to_string(),
                    agent: None,
                    name: None,
                    cwd: None,
                    status: AgentStatus::Blocked,
                },
            ]
        );
    }

    #[test]
    fn normalizes_every_documented_status() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"ws-1","agent_status":"working"},"#,
            r#"{"pane_id":"p2","workspace_id":"ws-1","agent_status":"blocked"},"#,
            r#"{"pane_id":"p3","workspace_id":"ws-1","agent_status":"done"},"#,
            r#"{"pane_id":"p4","workspace_id":"ws-1","agent_status":"idle"}"#,
            r#"]}}"#,
        );
        let statuses: Vec<AgentStatus> = parse_agent_list(line, WORKSPACE)
            .expect("valid payload")
            .into_iter()
            .map(|agent| agent.status)
            .collect();
        assert_eq!(
            statuses,
            vec![
                AgentStatus::Working,
                AgentStatus::Blocked,
                AgentStatus::Done,
                AgentStatus::Idle,
            ]
        );
    }

    #[test]
    fn unknown_or_missing_status_normalizes_to_unknown() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"ws-1","agent_status":"rebooting"},"#,
            r#"{"pane_id":"p2","workspace_id":"ws-1"}"#,
            r#"]}}"#,
        );
        let agents = parse_agent_list(line, WORKSPACE).expect("valid payload");
        assert_eq!(agents.len(), 2);
        assert!(agents
            .iter()
            .all(|agent| agent.status == AgentStatus::Unknown));
    }

    #[test]
    fn agent_status_field_carries_the_status() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"ws-1","agent_status":"working"},"#,
            r#"{"pane_id":"p2","workspace_id":"ws-1","status":"working"}"#,
            r#"]}}"#,
        );
        let agents = parse_agent_list(line, WORKSPACE).expect("valid payload");
        assert_eq!(agents.len(), 2);
        assert_eq!(
            agents[0].status,
            AgentStatus::Working,
            "agent_status:\"working\" must stay Working"
        );
        assert_eq!(
            agents[1].status,
            AgentStatus::Unknown,
            "a legacy status key must not be read"
        );
    }

    #[test]
    fn entries_missing_required_ids_are_dropped() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"workspace_id":"ws-1","agent_status":"working"},"#,
            r#"{"pane_id":"p2","agent_status":"working"},"#,
            r#"{"pane_id":"p3","workspace_id":"ws-1","agent_status":"working"}"#,
            r#"]}}"#,
        );
        let agents = parse_agent_list(line, WORKSPACE).expect("valid payload");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].pane_id, "p3");
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        let malformed = [
            "not json",
            r#"{"jsonrpc":"2.0","id":1}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":{}}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601}}"#,
        ];
        for line in malformed {
            assert!(
                parse_agent_list(line, WORKSPACE).is_none(),
                "payload should be rejected: {line}"
            );
        }
    }

    #[test]
    fn monitor_reports_failure_and_stops_cleanly() {
        let context = HerdrContext {
            socket_path: PathBuf::from("/nonexistent/herdr-agent-pulse-test.sock"),
            workspace_id: WORKSPACE.to_string(),
        };
        let monitor = spawn_monitor(context);
        let event = monitor
            .events()
            .recv_timeout(Duration::from_secs(2))
            .expect("monitor must report an event");
        assert!(matches!(event, MonitorEvent::Failed));
        monitor.stop();
    }
}
