//! Herdr plugin integration adapter.
//!
//! This module is the only place allowed to know Herdr environment
//! variables, the Unix socket transport, JSON-RPC framing, and raw
//! `agent.list` payloads. Everything it exposes is typed.

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
}

// `AgentStatus`, `AgentSnapshot`, and `AgentId` are `pub` (not `pub(crate)`)
// because they appear in the public `app::Action` enum, like
// `audio::AudioEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

/// Stable agent identity across every workspace served by the current
/// control socket: the workspace-qualified pane.
///
/// The parts stay private so no caller can display or leak the raw pane or
/// workspace ids; identity is only compared, ordered, and hashed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentId {
    workspace_id: String,
    pane_id: String,
}

impl AgentId {
    pub(crate) fn new(workspace_id: impl Into<String>, pane_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            pane_id: pane_id.into(),
        }
    }

    /// Private transport-only pane target. The opaque value never leaves this
    /// adapter as text; it is used only in an explicit `agent.focus` request.
    fn pane_target(&self) -> &str {
        &self.pane_id
    }
}

/// Recoverable outcome of an explicit request to focus the selected live pane.
/// No outcome contains raw Herdr identifiers or server messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusResult {
    Focused,
    Unsupported,
    Missing,
    Unavailable,
    NoSelection,
}

/// The only agent-list fields that may reach the read-only details modal.
/// Identity/location/session fields stay behind the private [`AgentId`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentDetails {
    pub(crate) name: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) activity: Option<String>,
}

/// One normalized agent from the current control socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub(crate) id: AgentId,
    pub(crate) details: AgentDetails,
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

/// A complete plugin environment (including a non-empty workspace id) is
/// still required for eligibility, but the workspace id is not retained:
/// `agent.list` aggregates every workspace served by the current socket.
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
    workspace_id.filter(|id| !id.is_empty())?;
    Some(HerdrContext {
        socket_path: PathBuf::from(socket_path),
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
    name: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    terminal_title: Option<String>,
    #[serde(default)]
    agent_status: Option<String>,
}

fn nonblank(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// Parses one `agent.list` response line, normalizing every agent the
/// current socket returns across all of its workspaces. Returns `None` when
/// the framing, `result`, or `agents` array is malformed; entries missing
/// required ids are dropped.
fn parse_agent_list(line: &str) -> Option<Vec<AgentSnapshot>> {
    let response: RawResponse = serde_json::from_str(line).ok()?;
    let agents = response.result?.agents?;
    Some(
        agents
            .into_iter()
            .filter_map(|value| serde_json::from_value::<RawAgent>(value).ok())
            .map(|raw| AgentSnapshot {
                id: AgentId::new(raw.workspace_id, raw.pane_id),
                details: AgentDetails {
                    name: nonblank(raw.name),
                    agent: nonblank(raw.agent),
                    activity: nonblank(raw.terminal_title),
                },
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

/// The sole pane-control request. Its target is an opaque pane id retained
/// only in memory from the current `agent.list` snapshot.
fn agent_focus_request(request_id: u64, id: &AgentId) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id.to_string(),
        "method": "agent.focus",
        "params": { "target": id.pane_target() },
    });
    format!("{request}\n")
}

/// Normalize a focus reply without passing raw JSON, server messages, or
/// identifiers beyond this adapter boundary. The official protocol uses a
/// JSON-RPC success result; an absent/moved target is any other server error.
fn parse_agent_focus(line: &str) -> FocusResult {
    let Ok(response) = serde_json::from_str::<serde_json::Value>(line) else {
        return FocusResult::Unavailable;
    };
    if response.get("result").is_some() {
        return FocusResult::Focused;
    }
    if response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_i64)
        == Some(-32601)
    {
        FocusResult::Unsupported
    } else if response.get("error").is_some() {
        FocusResult::Missing
    } else {
        FocusResult::Unavailable
    }
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
    parse_agent_list(line.trim_end())
}

fn poll_once(context: &HerdrContext, request_id: u64) -> MonitorEvent {
    match request_agent_list(context, request_id) {
        Some(agents) => MonitorEvent::Snapshot(agents),
        None => MonitorEvent::Failed,
    }
}

fn request_agent_focus(context: &HerdrContext, id: &AgentId) -> FocusResult {
    let Ok(mut stream) = UnixStream::connect(&context.socket_path) else {
        return FocusResult::Unavailable;
    };
    if stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT)).is_err()
        || stream
            .write_all(agent_focus_request(0, id).as_bytes())
            .is_err()
    {
        return FocusResult::Unavailable;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => FocusResult::Unavailable,
        Ok(_) => parse_agent_focus(line.trim_end()),
    }
}

/// Handle for the background polling thread. Dropping it (or calling
/// [`HerdrMonitor::stop`]) wakes the thread and joins it.
pub(crate) struct HerdrMonitor {
    context: HerdrContext,
    events: mpsc::Receiver<MonitorEvent>,
    focus_events: mpsc::Receiver<FocusResult>,
    focus_event_tx: mpsc::Sender<FocusResult>,
    stop: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl HerdrMonitor {
    pub(crate) fn events(&self) -> &mpsc::Receiver<MonitorEvent> {
        &self.events
    }

    /// Typed results of explicit background pane-focus requests. This stays
    /// separate from monitor polling: a focus timeout cannot delay or change
    /// the `agent.list` recovery ladder.
    pub(crate) fn focus_events(&self) -> &mpsc::Receiver<FocusResult> {
        &self.focus_events
    }

    /// Dispatch a focus request through the local socket without blocking the
    /// UI event loop. The one-shot worker keeps the same bounded I/O and
    /// recoverable result semantics as polling, then returns only a typed
    /// result to the controller.
    pub(crate) fn focus_agent(&self, id: AgentId) {
        let context = self.context.clone();
        let result_tx = self.focus_event_tx.clone();
        thread::spawn(move || {
            let _ = result_tx.send(request_agent_focus(&context, &id));
        });
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
    let (focus_event_tx, focus_events) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let monitor_context = context.clone();
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
        context: monitor_context,
        events: event_rx,
        focus_events,
        focus_event_tx,
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
        // The workspace id is no longer retained, but an incomplete plugin
        // environment (no workspace id) must stay ineligible.
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
    fn parses_agents_from_every_workspace_with_qualified_identity() {
        let parsed = parse_agent_list(concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"alpha","name":"research","agent_status":"working"},"#,
            r#"{"pane_id":"p1","workspace_id":"beta","name":"review","agent_status":"idle"}"#,
            r#"]}}"#,
        ))
        .unwrap();

        assert_eq!(parsed.len(), 2);
        assert_ne!(parsed[0].id, parsed[1].id);
    }

    #[test]
    fn parses_every_workspace_agent_without_filtering() {
        let line = concat!(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":["#,
            r#"{"pane_id":"p1","workspace_id":"ws-1","agent":"claude","name":"impl","cwd":"~/repo","agent_status":"working"},"#,
            r#"{"pane_id":"p2","workspace_id":"ws-2","agent_status":"idle"},"#,
            r#"{"pane_id":"p3","workspace_id":"ws-1","agent_status":"blocked"}"#,
            r#"]}}"#,
        );
        let agents = parse_agent_list(line).expect("valid payload");
        assert_eq!(
            agents,
            vec![
                AgentSnapshot {
                    id: AgentId::new("ws-1", "p1"),
                    details: AgentDetails {
                        name: Some("impl".to_string()),
                        agent: Some("claude".to_string()),
                        activity: None,
                    },
                    status: AgentStatus::Working,
                },
                AgentSnapshot {
                    id: AgentId::new("ws-2", "p2"),
                    details: AgentDetails::default(),
                    status: AgentStatus::Idle,
                },
                AgentSnapshot {
                    id: AgentId::new("ws-1", "p3"),
                    details: AgentDetails::default(),
                    status: AgentStatus::Blocked,
                },
            ]
        );
    }

    #[test]
    fn parser_keeps_allowed_detail_fields_and_ignores_location_metadata() {
        let parsed = parse_agent_list(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":[{"pane_id":"pane-private","workspace_id":"workspace-private","name":"  research  ","agent":"  claude  ","terminal_title":"  Review the modal  ","agent_status":"working","cwd":"/private","tab_id":"tab-private","terminal_id":"term-private"}]}}"#,
        )
        .expect("valid payload");
        let detail = &parsed[0].details;
        assert_eq!(detail.name.as_deref(), Some("research"));
        assert_eq!(detail.agent.as_deref(), Some("claude"));
        assert_eq!(detail.activity.as_deref(), Some("Review the modal"));
        assert_eq!(parsed[0].status, AgentStatus::Working);
    }

    #[test]
    fn parser_omits_blank_allowed_detail_fields_without_private_fallbacks() {
        let parsed = parse_agent_list(
            r#"{"jsonrpc":"2.0","id":1,"result":{"agents":[{"pane_id":"pi","workspace_id":"claude","name":" ","agent":"\t","terminal_title":" ","cwd":"/private","agent_status":"idle"}]}}"#,
        )
        .expect("valid payload");
        assert_eq!(parsed[0].details, AgentDetails::default());
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
        let statuses: Vec<AgentStatus> = parse_agent_list(line)
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
        let agents = parse_agent_list(line).expect("valid payload");
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
        let agents = parse_agent_list(line).expect("valid payload");
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
        let agents = parse_agent_list(line).expect("valid payload");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, AgentId::new("ws-1", "p3"));
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
                parse_agent_list(line).is_none(),
                "payload should be rejected: {line}"
            );
        }
    }

    #[test]
    fn monitor_reports_failure_and_stops_cleanly() {
        let context = HerdrContext {
            socket_path: PathBuf::from("/nonexistent/herdr-agent-pulse-test.sock"),
        };
        let monitor = spawn_monitor(context);
        let event = monitor
            .events()
            .recv_timeout(Duration::from_secs(2))
            .expect("monitor must report an event");
        assert!(matches!(event, MonitorEvent::Failed));
        monitor.stop();
    }

    #[test]
    fn focus_request_targets_only_the_opaque_pane_id() {
        let request = agent_focus_request(9, &AgentId::new("other-workspace", "pane-7"));
        let body = request.trim_end();
        let value: serde_json::Value = serde_json::from_str(body).expect("request must be JSON");
        assert_eq!(value["method"], "agent.focus");
        assert_eq!(value["params"], serde_json::json!({ "target": "pane-7" }));
        assert!(!body.contains("other-workspace"));
    }

    #[test]
    fn focus_response_distinguishes_success_unsupported_missing_and_transport_failures() {
        assert_eq!(
            parse_agent_focus(r#"{"jsonrpc":"2.0","id":"1","result":{}}"#),
            FocusResult::Focused
        );
        assert_eq!(
            parse_agent_focus(
                r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32601,"message":"method not found"}}"#
            ),
            FocusResult::Unsupported
        );
        assert_eq!(
            parse_agent_focus(
                r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32000,"message":"pane not found"}}"#
            ),
            FocusResult::Missing
        );
        assert_eq!(parse_agent_focus("not json"), FocusResult::Unavailable);
    }
}
