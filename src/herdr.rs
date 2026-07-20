//! Herdr plugin integration adapter.
//!
//! This module is the only place allowed to know Herdr environment
//! variables, the Unix socket transport, JSON-RPC framing, and raw
//! `agent.list` payloads. Everything it exposes is typed.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
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

/// Recoverable outcome of an explicit request to rename the selected live
/// agent. Raw Herdr errors and identifiers remain inside this adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameResult {
    Renamed,
    Unsupported,
    Missing,
    Unavailable,
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

/// The sole Agent Planets metadata edit request. An empty user input becomes
/// JSON null so Herdr clears the explicit name rather than storing whitespace.
fn agent_rename_request(request_id: u64, id: &AgentId, name: Option<&str>) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id.to_string(),
        "method": "agent.rename",
        "params": { "target": id.pane_target(), "name": name },
    });
    format!("{request}\n")
}

/// Normalize a focus reply without passing raw JSON, server messages, or
/// identifiers beyond this adapter boundary. The official protocol uses a
/// JSON-RPC success result; an absent/moved target is any other server error.
fn parse_agent_focus(line: &str) -> FocusResult {
    match response_kind(line) {
        ResponseKind::Success => FocusResult::Focused,
        ResponseKind::Unsupported => FocusResult::Unsupported,
        ResponseKind::Missing => FocusResult::Missing,
        ResponseKind::Unavailable => FocusResult::Unavailable,
    }
}

fn parse_agent_rename(line: &str) -> RenameResult {
    match response_kind(line) {
        ResponseKind::Success => RenameResult::Renamed,
        ResponseKind::Unsupported => RenameResult::Unsupported,
        ResponseKind::Missing => RenameResult::Missing,
        ResponseKind::Unavailable => RenameResult::Unavailable,
    }
}

/// Classify a JSON-RPC response without preserving its raw server text.
enum ResponseKind {
    Success,
    Unsupported,
    Missing,
    Unavailable,
}

fn response_kind(line: &str) -> ResponseKind {
    let Ok(response) = serde_json::from_str::<serde_json::Value>(line) else {
        return ResponseKind::Unavailable;
    };
    if response.get("result").is_some() {
        return ResponseKind::Success;
    }
    if response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_i64)
        == Some(-32601)
    {
        ResponseKind::Unsupported
    } else if response.get("error").is_some() {
        ResponseKind::Missing
    } else {
        ResponseKind::Unavailable
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

fn request_agent_rename(context: &HerdrContext, id: &AgentId, name: Option<&str>) -> RenameResult {
    let Ok(mut stream) = UnixStream::connect(&context.socket_path) else {
        return RenameResult::Unavailable;
    };
    if stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT)).is_err()
        || stream
            .write_all(agent_rename_request(0, id, name).as_bytes())
            .is_err()
    {
        return RenameResult::Unavailable;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => RenameResult::Unavailable,
        Ok(_) => parse_agent_rename(line.trim_end()),
    }
}
/// The blocking socket round-trip a focus request performs. Behind an
/// indirection so tests can substitute a deterministic blocked transport
/// without a real socket; production always uses [`request_agent_focus`].
type FocusTransport = Arc<dyn Fn(&HerdrContext, &AgentId) -> FocusResult + Send + Sync>;

/// Releases the in-flight slot on every exit path, including a panicking
/// worker, so one bad request cannot disable focus for the rest of the session.
struct InFlightGuard(Arc<AtomicBool>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Bounds explicit focus requests to at most one in-flight socket worker.
///
/// Focus is a user-visible "go look at that pane now" action, so a repeat
/// while a request is still running is ignored rather than queued: replaying a
/// backlog of stale jumps after a stalled socket recovers would be worse than
/// dropping them. The slot is released before the result is published, so the
/// next press after a completed request always dispatches.
struct FocusDispatcher {
    context: HerdrContext,
    results: mpsc::Sender<FocusResult>,
    in_flight: Arc<AtomicBool>,
    transport: FocusTransport,
}

impl FocusDispatcher {
    fn new(context: HerdrContext, results: mpsc::Sender<FocusResult>) -> Self {
        Self {
            context,
            results,
            in_flight: Arc::new(AtomicBool::new(false)),
            transport: Arc::new(request_agent_focus),
        }
    }

    fn dispatch(&self, id: AgentId) {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let guard = InFlightGuard(Arc::clone(&self.in_flight));
        let context = self.context.clone();
        let results = self.results.clone();
        let transport = Arc::clone(&self.transport);
        let worker = thread::Builder::new()
            .name("herdr-focus".to_string())
            .spawn(move || {
                // Scoped so the slot reopens before the result is published.
                let result = {
                    let _guard = guard;
                    transport(&context, &id)
                };
                let _ = results.send(result);
            });
        if worker.is_err() {
            // The guard moved into the failed closure and was dropped with it,
            // so the slot is already free; report a recoverable failure.
            let _ = self.results.send(FocusResult::Unavailable);
        }
    }

    #[cfg(test)]
    fn is_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// Handle for the background polling thread. Dropping it (or calling
/// [`HerdrMonitor::stop`]) wakes the thread and joins it.
pub(crate) struct HerdrMonitor {
    context: HerdrContext,
    events: mpsc::Receiver<MonitorEvent>,
    focus_events: mpsc::Receiver<FocusResult>,
    focus: FocusDispatcher,
    rename_events: mpsc::Receiver<RenameResult>,
    rename_event_tx: mpsc::Sender<RenameResult>,
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

    /// Typed results of explicit background rename requests. They remain
    /// separate from polling, so a slow or failed edit cannot delay recovery.
    pub(crate) fn rename_events(&self) -> &mpsc::Receiver<RenameResult> {
        &self.rename_events
    }

    /// Dispatch a focus request through the local socket without blocking the
    /// UI event loop. At most one request is in flight at a time, so holding
    /// `o`/`O` against a stalled socket cannot spawn unbounded detached
    /// workers; extra presses are ignored and only a typed result crosses back
    /// to the controller.
    pub(crate) fn focus_agent(&self, id: AgentId) {
        self.focus.dispatch(id);
    }

    /// Dispatch an explicit name edit without blocking rendering or keyboard
    /// input. Only the typed result crosses back to the controller.
    pub(crate) fn rename_agent(&self, id: AgentId, name: Option<String>) {
        let context = self.context.clone();
        let result_tx = self.rename_event_tx.clone();
        thread::spawn(move || {
            let _ = result_tx.send(request_agent_rename(&context, &id, name.as_deref()));
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
    let (rename_event_tx, rename_events) = mpsc::channel();
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
        focus: FocusDispatcher::new(monitor_context.clone(), focus_event_tx),
        context: monitor_context,
        events: event_rx,
        focus_events,
        rename_events,
        rename_event_tx,
        stop: Some(stop_tx),
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, Mutex};
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
    fn rename_request_targets_only_the_opaque_pane_and_clears_blank_names_with_null() {
        let id = AgentId::new("workspace-private", "pane-7");
        let named: serde_json::Value =
            serde_json::from_str(agent_rename_request(9, &id, Some("Research")).trim_end())
                .expect("rename request must be JSON");
        assert_eq!(named["method"], "agent.rename");
        assert_eq!(
            named["params"],
            serde_json::json!({ "target": "pane-7", "name": "Research" })
        );
        assert!(!named.to_string().contains("workspace-private"));

        let cleared: serde_json::Value =
            serde_json::from_str(agent_rename_request(10, &id, None).trim_end())
                .expect("clear rename request must be JSON");
        assert_eq!(cleared["params"]["name"], serde_json::Value::Null);
    }

    /// A deterministic stand-in for the local socket: every call blocks until
    /// the test releases it, so "one request is still in flight" is a state the
    /// test controls rather than a timing guess.
    struct BlockedTransport {
        entered: mpsc::Sender<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
        calls: Arc<AtomicUsize>,
        result: FocusResult,
    }

    impl BlockedTransport {
        fn install(dispatcher: &mut FocusDispatcher, result: FocusResult) -> BlockedHandle {
            let (entered_tx, entered) = mpsc::channel();
            let release = Arc::new((Mutex::new(false), Condvar::new()));
            let calls = Arc::new(AtomicUsize::new(0));
            let transport = BlockedTransport {
                entered: entered_tx,
                release: Arc::clone(&release),
                calls: Arc::clone(&calls),
                result,
            };
            dispatcher.transport = Arc::new(move |_context: &HerdrContext, _id: &AgentId| {
                transport.calls.fetch_add(1, Ordering::SeqCst);
                let _ = transport.entered.send(());
                let (lock, cvar) = &*transport.release;
                let mut released = lock.lock().expect("release lock");
                while !*released {
                    released = cvar.wait(released).expect("release wait");
                }
                transport.result
            });
            BlockedHandle {
                entered,
                release,
                calls,
            }
        }
    }

    struct BlockedHandle {
        entered: mpsc::Receiver<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
        calls: Arc<AtomicUsize>,
    }

    impl BlockedHandle {
        /// Blocks until a worker has actually entered the transport, so later
        /// assertions never race the spawned thread.
        fn await_entry(&self) {
            self.entered
                .recv_timeout(Duration::from_secs(2))
                .expect("a focus worker must reach the transport");
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn release(&self) {
            let (lock, cvar) = &*self.release;
            *lock.lock().expect("release lock") = true;
            cvar.notify_all();
        }
    }

    fn test_dispatcher() -> (FocusDispatcher, mpsc::Receiver<FocusResult>) {
        let (results_tx, results) = mpsc::channel();
        let context = HerdrContext {
            socket_path: PathBuf::from("/nonexistent/herdr-focus-test.sock"),
        };
        (FocusDispatcher::new(context, results_tx), results)
    }

    #[test]
    fn repeated_focus_input_keeps_at_most_one_request_in_flight() {
        let (mut dispatcher, results) = test_dispatcher();
        let blocked = BlockedTransport::install(&mut dispatcher, FocusResult::Focused);
        let id = AgentId::new("ws-1", "pane-1");

        for _ in 0..8 {
            dispatcher.dispatch(id.clone());
        }
        blocked.await_entry();

        assert_eq!(
            blocked.calls(),
            1,
            "repeated focus input must not reach the transport more than once"
        );
        assert_eq!(
            results.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "no result may arrive while the transport is still blocked"
        );

        blocked.release();
        assert_eq!(
            results.recv_timeout(Duration::from_secs(2)),
            Ok(FocusResult::Focused)
        );
        assert_eq!(
            results.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "ignored repeats must not queue extra results"
        );
        assert_eq!(blocked.calls(), 1);
    }

    #[test]
    fn focus_is_dispatchable_again_after_the_previous_request_completes() {
        let (mut dispatcher, results) = test_dispatcher();
        let blocked = BlockedTransport::install(&mut dispatcher, FocusResult::Focused);
        let id = AgentId::new("ws-1", "pane-1");

        dispatcher.dispatch(id.clone());
        blocked.await_entry();
        dispatcher.dispatch(id.clone());
        assert_eq!(blocked.calls(), 1, "the second press must be ignored");

        blocked.release();
        assert_eq!(
            results.recv_timeout(Duration::from_secs(2)),
            Ok(FocusResult::Focused)
        );

        // The in-flight slot is released before the result is published, so a
        // retry observed after the result is deterministic, not a race.
        dispatcher.dispatch(id);
        blocked.await_entry();
        assert_eq!(
            blocked.calls(),
            2,
            "focus must be retryable once the previous request completes"
        );
        assert_eq!(
            results.recv_timeout(Duration::from_secs(2)),
            Ok(FocusResult::Focused)
        );
    }

    #[test]
    fn the_guard_preserves_every_focus_result_unchanged() {
        for result in [
            FocusResult::Focused,
            FocusResult::Unsupported,
            FocusResult::Missing,
            FocusResult::Unavailable,
        ] {
            let (mut dispatcher, results) = test_dispatcher();
            let blocked = BlockedTransport::install(&mut dispatcher, result);
            dispatcher.dispatch(AgentId::new("ws-1", "pane-1"));
            blocked.await_entry();
            blocked.release();
            assert_eq!(results.recv_timeout(Duration::from_secs(2)), Ok(result));
        }
    }

    #[test]
    fn a_panicking_focus_worker_does_not_strand_the_in_flight_slot() {
        let (mut dispatcher, _results) = test_dispatcher();
        dispatcher.transport = Arc::new(|_context: &HerdrContext, _id: &AgentId| {
            panic!("focus transport panicked");
        });
        let id = AgentId::new("ws-1", "pane-1");

        dispatcher.dispatch(id.clone());
        // Wait for the slot to be released rather than for the thread itself;
        // the panicking worker is detached.
        let deadline = 200;
        for _ in 0..deadline {
            if !dispatcher.is_in_flight() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !dispatcher.is_in_flight(),
            "a panicking worker must release the in-flight slot"
        );

        let blocked = BlockedTransport::install(&mut dispatcher, FocusResult::Focused);
        dispatcher.dispatch(id);
        blocked.await_entry();
        assert_eq!(
            blocked.calls(),
            1,
            "focus must recover after a worker panic"
        );
        blocked.release();
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
