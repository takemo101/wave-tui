//! Agent host facade: the one door the runtime uses to reach an Agent Planets
//! host.
//!
//! The runtime, event loop, and input effects speak only this module's
//! [`AgentHostContext`] and [`AgentHostMonitor`]. A run has at most one host:
//! the Herdr plugin monitor (focus and rename) or the read-only herdr-core
//! monitor. There is no host registry, no trait object, and no multi-host
//! aggregation; a host is one more variant here, never a framework.
//!
//! What the facade guarantees:
//!
//! - **One host, chosen once.** [`AgentHostContext::from_env`] applies the
//!   `--agent-host` selection and the legacy `--no-agent-pulse` disable. A
//!   disabled or `off` launch resolves no eligibility at all; `auto` tries
//!   Herdr and only then herdr-core; an explicit host resolves only its own
//!   adapter and never falls back. The choice is never revisited, so a host
//!   that later fails does not switch the run to the other.
//! - **Typed surface.** Snapshots, focus results, and rename results arrive on
//!   three separate receivers in the shared [`crate::model::agent`]
//!   vocabulary; no transport data (sockets, pane ids, JSON) crosses it.
//! - **Capabilities as startup context.** [`AgentHostContext::capabilities`]
//!   names the explicit requests the host honors. The runtime hands them to the
//!   `App` once, before the first agent event.
//! - **Capability-gated dispatch.** [`AgentHostMonitor::focus_agent`] and
//!   [`AgentHostMonitor::rename_agent`] are no-ops for an unsupported request,
//!   so even a caller that skipped the reducer gate cannot reach a host that
//!   lacks the request. herdr-core is read-only: it has no control channel to
//!   reach, so its focus and rename receivers never yield anything.
//! - **Owned lifetime.** [`AgentHostMonitor::stop`] (or dropping it) stops and
//!   joins the host's polling thread.

use std::sync::mpsc::{self, Receiver};

use crate::herdr::{self, HerdrContext, HerdrMonitor};
use crate::herdr_core::{self, HerdrCoreContext, HerdrCoreMonitor};
use crate::model::agent::{
    AgentCapabilities, AgentHostSelection, AgentId, FocusResult, MonitorEvent, RenameCompletion,
    RenameRequest,
};

/// Eligibility of an Agent Planets host for this launch, with the host's
/// startup context. Absent for standalone, ineligible, `off`, and
/// `--no-agent-pulse` launches, so nothing is created and behavior stays
/// exactly standalone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentHostContext {
    Herdr(HerdrContext),
    Core(HerdrCoreContext),
}

impl AgentHostContext {
    /// Read host eligibility from the process environment. `disabled` is the
    /// legacy `--no-agent-pulse` flag, which wins over `selection` (the
    /// `--agent-host` value).
    pub(crate) fn from_env(disabled: bool, selection: AgentHostSelection) -> Option<Self> {
        select(
            disabled,
            selection,
            || herdr::context_from_env(false),
            herdr_core::context_from_env,
        )
    }

    /// The explicit requests this host honors.
    pub(crate) fn capabilities(&self) -> AgentCapabilities {
        match self {
            Self::Herdr(_) => AgentCapabilities::HERDR,
            Self::Core(_) => AgentCapabilities::READ_ONLY,
        }
    }

    /// Start the host's background monitor.
    pub(crate) fn spawn_monitor(self) -> AgentHostMonitor {
        let capabilities = self.capabilities();
        let backend = match self {
            Self::Herdr(context) => Backend::Herdr(herdr::spawn_monitor(context)),
            Self::Core(context) => Backend::Core(CoreHost::new(herdr_core::spawn_monitor(context))),
        };
        AgentHostMonitor {
            capabilities,
            backend,
        }
    }
}

/// Pure host selection with injected eligibility resolvers, so the whole
/// matrix is testable without touching the process environment. Each resolver
/// runs at most once and only when its host can still be chosen: a disabled or
/// `off` launch resolves neither, `auto` resolves herdr-core only when Herdr
/// was absent or ineligible, and an explicit host resolves only itself.
fn select(
    disabled: bool,
    selection: AgentHostSelection,
    herdr_context: impl FnOnce() -> Option<HerdrContext>,
    core_context: impl FnOnce() -> Option<HerdrCoreContext>,
) -> Option<AgentHostContext> {
    if disabled {
        return None;
    }
    match selection {
        AgentHostSelection::Off => None,
        AgentHostSelection::Herdr => herdr_context().map(AgentHostContext::Herdr),
        AgentHostSelection::HerdrCore => core_context().map(AgentHostContext::Core),
        AgentHostSelection::Auto => herdr_context()
            .map(AgentHostContext::Herdr)
            .or_else(|| core_context().map(AgentHostContext::Core)),
    }
}

/// A running herdr-core monitor. The core host is observation only, so it has
/// no focus or rename channel; these receivers belong to the facade, have no
/// sender, and therefore stay permanently empty.
struct CoreHost {
    monitor: HerdrCoreMonitor,
    focus_events: Receiver<FocusResult>,
    rename_events: Receiver<RenameCompletion>,
}

impl CoreHost {
    fn new(monitor: HerdrCoreMonitor) -> Self {
        let (_, focus_events) = mpsc::channel();
        let (_, rename_events) = mpsc::channel();
        Self {
            monitor,
            focus_events,
            rename_events,
        }
    }
}

enum Backend {
    Herdr(HerdrMonitor),
    Core(CoreHost),
    #[cfg(test)]
    Synthetic(synthetic::Backend),
}

/// Handle for a running host monitor: typed event channels plus explicit,
/// capability-gated requests. Dropping it stops and joins the host's thread.
pub(crate) struct AgentHostMonitor {
    capabilities: AgentCapabilities,
    backend: Backend,
}

impl AgentHostMonitor {
    /// The explicit requests this host honors.
    pub(crate) fn capabilities(&self) -> AgentCapabilities {
        self.capabilities
    }

    /// Typed `agent.list`-style snapshots and poll failures.
    pub(crate) fn events(&self) -> &Receiver<MonitorEvent> {
        match &self.backend {
            Backend::Herdr(monitor) => monitor.events(),
            Backend::Core(core) => core.monitor.events(),
            #[cfg(test)]
            Backend::Synthetic(synthetic) => &synthetic.events,
        }
    }

    /// Typed results of explicit focus requests, separate from polling.
    pub(crate) fn focus_events(&self) -> &Receiver<FocusResult> {
        match &self.backend {
            Backend::Herdr(monitor) => monitor.focus_events(),
            Backend::Core(core) => &core.focus_events,
            #[cfg(test)]
            Backend::Synthetic(synthetic) => &synthetic.focus_events,
        }
    }

    /// Typed completions of explicit rename requests, separate from polling.
    /// Each carries the id of the submission it settles.
    pub(crate) fn rename_events(&self) -> &Receiver<RenameCompletion> {
        match &self.backend {
            Backend::Herdr(monitor) => monitor.rename_events(),
            Backend::Core(core) => &core.rename_events,
            #[cfg(test)]
            Backend::Synthetic(synthetic) => &synthetic.rename_events,
        }
    }

    /// Ask the host to focus an agent's pane without blocking. A no-op when the
    /// host has no focus capability; the typed result returns on
    /// [`Self::focus_events`].
    pub(crate) fn focus_agent(&self, id: AgentId) {
        if !self.capabilities.focus {
            return;
        }
        match &self.backend {
            Backend::Herdr(monitor) => monitor.focus_agent(id),
            // Unreachable while the core descriptor is READ_ONLY; if it ever
            // advertised focus, silence still beats a misleading result.
            Backend::Core(_) => {}
            #[cfg(test)]
            Backend::Synthetic(synthetic) => synthetic.record_focus(),
        }
    }

    /// Ask the host to rename an agent (`None` clears the name) without
    /// blocking. Returns whether the host accepted the request: only then does
    /// a matching completion arrive on [`Self::rename_events`]. A host without
    /// the rename capability, or one whose single rename slot is busy, refuses
    /// (`false`) and produces no completion, so the caller settles the
    /// submission itself.
    pub(crate) fn rename_agent(&self, request: RenameRequest) -> bool {
        if !self.capabilities.rename {
            return false;
        }
        match &self.backend {
            Backend::Herdr(monitor) => monitor.rename_agent(request),
            Backend::Core(_) => false,
            #[cfg(test)]
            Backend::Synthetic(synthetic) => synthetic.record_rename(request),
        }
    }

    /// Stop the host and join its polling thread.
    pub(crate) fn stop(self) {
        match self.backend {
            Backend::Herdr(monitor) => monitor.stop(),
            Backend::Core(core) => core.monitor.stop(),
            #[cfg(test)]
            Backend::Synthetic(_) => {}
        }
    }
}

/// A synthetic host for tests: no socket, no thread. Tests feed its channels
/// through [`SyntheticControl`] and observe which requests actually reached it,
/// which is how a read-only host is proven to receive none.
#[cfg(test)]
pub(crate) mod synthetic {
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    use super::{AgentCapabilities, AgentHostMonitor, Backend as HostBackend};
    use crate::model::agent::{
        FocusResult, MonitorEvent, RenameCompletion, RenameRequest, RenameRequestId,
    };

    #[derive(Default)]
    struct Dispatched {
        focus: usize,
        renames: Vec<RenameRequest>,
        /// When set, the host's single rename slot is busy: requests are
        /// refused and never recorded as dispatched.
        rename_busy: bool,
    }

    pub(super) struct Backend {
        pub(super) events: Receiver<MonitorEvent>,
        pub(super) focus_events: Receiver<FocusResult>,
        pub(super) rename_events: Receiver<RenameCompletion>,
        dispatched: Arc<Mutex<Dispatched>>,
    }

    impl Backend {
        pub(super) fn record_focus(&self) {
            self.dispatched.lock().unwrap().focus += 1;
        }

        pub(super) fn record_rename(&self, request: RenameRequest) -> bool {
            let mut dispatched = self.dispatched.lock().unwrap();
            if dispatched.rename_busy {
                return false;
            }
            dispatched.renames.push(request);
            true
        }
    }

    /// The test-side ends of a synthetic host.
    pub(crate) struct SyntheticControl {
        pub(crate) events: Sender<MonitorEvent>,
        pub(crate) focus_results: Sender<FocusResult>,
        pub(crate) rename_results: Sender<RenameCompletion>,
        dispatched: Arc<Mutex<Dispatched>>,
    }

    impl SyntheticControl {
        /// How many focus requests reached the host.
        pub(crate) fn focus_requests(&self) -> usize {
            self.dispatched.lock().unwrap().focus
        }

        /// The names of the rename requests that reached the host.
        pub(crate) fn rename_requests(&self) -> Vec<Option<String>> {
            let dispatched = self.dispatched.lock().unwrap();
            dispatched
                .renames
                .iter()
                .map(|request| request.name.clone())
                .collect()
        }

        /// The submission ids of the rename requests that reached the host.
        pub(crate) fn rename_request_ids(&self) -> Vec<RenameRequestId> {
            let dispatched = self.dispatched.lock().unwrap();
            dispatched
                .renames
                .iter()
                .map(|request| request.request)
                .collect()
        }

        /// Make the host refuse (or accept again) rename requests, as a busy
        /// single worker slot would.
        pub(crate) fn set_rename_busy(&self, busy: bool) {
            self.dispatched.lock().unwrap().rename_busy = busy;
        }
    }

    impl AgentHostMonitor {
        /// A synthetic host with the given capabilities.
        pub(crate) fn synthetic(capabilities: AgentCapabilities) -> (Self, SyntheticControl) {
            let (events_tx, events) = mpsc::channel();
            let (focus_tx, focus_events) = mpsc::channel();
            let (rename_tx, rename_events) = mpsc::channel();
            let dispatched = Arc::new(Mutex::new(Dispatched::default()));
            let monitor = Self {
                capabilities,
                backend: HostBackend::Synthetic(Backend {
                    events,
                    focus_events,
                    rename_events,
                    dispatched: Arc::clone(&dispatched),
                }),
            };
            let control = SyntheticControl {
                events: events_tx,
                focus_results: focus_tx,
                rename_results: rename_tx,
                dispatched,
            };
            (monitor, control)
        }

        /// A synthetic observe-only host: neither focus nor rename.
        pub(crate) fn synthetic_read_only() -> (Self, SyntheticControl) {
            Self::synthetic(AgentCapabilities::READ_ONLY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::agent::{
        AgentDetails, AgentSnapshot, AgentStatus, RenameRequestId, RenameResult,
    };
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::time::Duration;

    fn dead_socket_context() -> AgentHostContext {
        AgentHostContext::Herdr(HerdrContext {
            socket_path: PathBuf::from("/nonexistent/wave-tui-agent-host.sock"),
        })
    }

    #[test]
    fn disabled_launches_create_no_host_context() {
        for selection in ALL_SELECTIONS {
            assert!(AgentHostContext::from_env(true, selection).is_none());
        }
    }

    const ALL_SELECTIONS: [AgentHostSelection; 4] = [
        AgentHostSelection::Auto,
        AgentHostSelection::Herdr,
        AgentHostSelection::HerdrCore,
        AgentHostSelection::Off,
    ];

    fn herdr_context() -> HerdrContext {
        HerdrContext {
            socket_path: PathBuf::from("/nonexistent/herdr.sock"),
        }
    }

    fn core_context() -> HerdrCoreContext {
        HerdrCoreContext {
            socket_path: PathBuf::from("/nonexistent/hc.sock"),
        }
    }

    /// Runs `select` with fixed eligibility and reports how many times each
    /// resolver was consulted, so "never evaluated" is provable.
    fn selected(
        disabled: bool,
        selection: AgentHostSelection,
        herdr_eligible: bool,
        core_eligible: bool,
    ) -> (Option<AgentHostContext>, usize, usize) {
        let herdr_calls = Cell::new(0);
        let core_calls = Cell::new(0);
        let chosen = select(
            disabled,
            selection,
            || {
                herdr_calls.set(herdr_calls.get() + 1);
                herdr_eligible.then(herdr_context)
            },
            || {
                core_calls.set(core_calls.get() + 1);
                core_eligible.then(core_context)
            },
        );
        (chosen, herdr_calls.get(), core_calls.get())
    }

    fn herdr_host() -> Option<AgentHostContext> {
        Some(AgentHostContext::Herdr(herdr_context()))
    }

    fn core_host() -> Option<AgentHostContext> {
        Some(AgentHostContext::Core(core_context()))
    }

    #[test]
    fn auto_prefers_an_eligible_herdr_without_resolving_core() {
        for core_eligible in [true, false] {
            let (chosen, herdr_calls, core_calls) =
                selected(false, AgentHostSelection::Auto, true, core_eligible);
            assert_eq!(chosen, herdr_host());
            assert_eq!((herdr_calls, core_calls), (1, 0));
        }
    }

    #[test]
    fn auto_falls_back_to_core_only_when_herdr_is_ineligible() {
        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::Auto, false, true);
        assert_eq!(chosen, core_host());
        assert_eq!((herdr_calls, core_calls), (1, 1));

        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::Auto, false, false);
        assert_eq!(chosen, None);
        assert_eq!((herdr_calls, core_calls), (1, 1));
    }

    #[test]
    fn explicit_herdr_resolves_only_herdr_and_never_falls_back() {
        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::Herdr, true, true);
        assert_eq!(chosen, herdr_host());
        assert_eq!((herdr_calls, core_calls), (1, 0));

        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::Herdr, false, true);
        assert_eq!(chosen, None, "an eligible core must not stand in for Herdr");
        assert_eq!((herdr_calls, core_calls), (1, 0));
    }

    #[test]
    fn explicit_core_resolves_only_core_and_never_falls_back() {
        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::HerdrCore, true, true);
        assert_eq!(chosen, core_host());
        assert_eq!((herdr_calls, core_calls), (0, 1));

        let (chosen, herdr_calls, core_calls) =
            selected(false, AgentHostSelection::HerdrCore, true, false);
        assert_eq!(chosen, None, "an eligible Herdr must not stand in for core");
        assert_eq!((herdr_calls, core_calls), (0, 1));
    }

    #[test]
    fn off_and_the_legacy_disable_resolve_nothing_in_any_environment() {
        for (herdr_eligible, core_eligible) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let (chosen, herdr_calls, core_calls) = selected(
                false,
                AgentHostSelection::Off,
                herdr_eligible,
                core_eligible,
            );
            assert_eq!(chosen, None);
            assert_eq!((herdr_calls, core_calls), (0, 0));

            for selection in ALL_SELECTIONS {
                let (chosen, herdr_calls, core_calls) =
                    selected(true, selection, herdr_eligible, core_eligible);
                assert_eq!(chosen, None, "{selection:?} with --no-agent-pulse");
                assert_eq!((herdr_calls, core_calls), (0, 0), "{selection:?}");
            }
        }
    }

    #[test]
    fn selection_matrix_matches_the_documented_table() {
        // (selection, herdr eligible, core eligible) -> expected host.
        let cases = [
            (AgentHostSelection::Auto, true, true, herdr_host()),
            (AgentHostSelection::Auto, true, false, herdr_host()),
            (AgentHostSelection::Auto, false, true, core_host()),
            (AgentHostSelection::Auto, false, false, None),
            (AgentHostSelection::Herdr, true, true, herdr_host()),
            (AgentHostSelection::Herdr, true, false, herdr_host()),
            (AgentHostSelection::Herdr, false, true, None),
            (AgentHostSelection::Herdr, false, false, None),
            (AgentHostSelection::HerdrCore, true, true, core_host()),
            (AgentHostSelection::HerdrCore, false, true, core_host()),
            (AgentHostSelection::HerdrCore, true, false, None),
            (AgentHostSelection::HerdrCore, false, false, None),
            (AgentHostSelection::Off, true, true, None),
        ];
        for (selection, herdr_eligible, core_eligible, expected) in cases {
            let (chosen, _, _) = selected(false, selection, herdr_eligible, core_eligible);
            assert_eq!(
                chosen, expected,
                "{selection:?} herdr={herdr_eligible} core={core_eligible}"
            );
        }
    }

    #[test]
    fn the_core_context_is_read_only_and_herdr_keeps_focus_and_rename() {
        assert_eq!(
            AgentHostContext::Core(core_context()).capabilities(),
            AgentCapabilities::READ_ONLY
        );
        assert_eq!(
            AgentHostContext::Herdr(herdr_context()).capabilities(),
            AgentCapabilities::HERDR
        );
    }

    #[test]
    fn the_herdr_context_advertises_herdr_capabilities() {
        assert_eq!(
            dead_socket_context().capabilities(),
            AgentCapabilities::HERDR
        );
    }

    #[test]
    fn a_spawned_herdr_monitor_carries_the_context_capabilities_and_stops() {
        let monitor = dead_socket_context().spawn_monitor();
        assert_eq!(monitor.capabilities(), AgentCapabilities::HERDR);
        // The first poll against a missing socket is a recoverable failure
        // delivered on the typed events channel, not a panic or hang.
        let event = monitor
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("the first poll reports a typed event");
        assert_eq!(event, MonitorEvent::Failed);
        assert!(monitor.focus_events().try_recv().is_err());
        assert!(monitor.rename_events().try_recv().is_err());
        monitor.stop();
    }

    #[test]
    fn dropping_the_monitor_joins_the_host_thread() {
        // Drop must not hang: the polling thread wakes on the closed stop
        // channel and is joined.
        let monitor = dead_socket_context().spawn_monitor();
        drop(monitor);
    }

    #[test]
    fn a_synthetic_host_forwards_typed_events_on_three_separate_channels() {
        let (monitor, control) = AgentHostMonitor::synthetic(AgentCapabilities::HERDR);
        control.events.send(MonitorEvent::Failed).unwrap();
        control.focus_results.send(FocusResult::Missing).unwrap();
        let completion = RenameCompletion {
            request: RenameRequestId::FIRST,
            result: RenameResult::Renamed,
        };
        control.rename_results.send(completion).unwrap();

        assert_eq!(monitor.events().try_recv(), Ok(MonitorEvent::Failed));
        assert_eq!(monitor.focus_events().try_recv(), Ok(FocusResult::Missing));
        assert_eq!(monitor.rename_events().try_recv(), Ok(completion));
        assert!(monitor.events().try_recv().is_err());
    }

    fn rename(request: RenameRequestId, name: Option<&str>) -> RenameRequest {
        RenameRequest {
            request,
            target: AgentId::herdr("ws", "p1"),
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn supported_requests_reach_the_host() {
        let (monitor, control) = AgentHostMonitor::synthetic(AgentCapabilities::HERDR);
        let first = RenameRequestId::FIRST;

        monitor.focus_agent(AgentId::herdr("ws", "p1"));
        assert!(monitor.rename_agent(rename(first, Some("alpha"))));
        assert!(monitor.rename_agent(rename(first.following(), None)));

        assert_eq!(control.focus_requests(), 1);
        assert_eq!(
            control.rename_requests(),
            vec![Some("alpha".to_string()), None]
        );
        assert_eq!(control.rename_request_ids(), vec![first, first.following()]);
    }

    #[test]
    fn a_read_only_host_never_receives_focus_or_rename_requests() {
        let (monitor, control) = AgentHostMonitor::synthetic_read_only();
        assert_eq!(monitor.capabilities(), AgentCapabilities::READ_ONLY);

        monitor.focus_agent(AgentId::herdr("ws", "p1"));
        assert!(!monitor.rename_agent(rename(RenameRequestId::FIRST, Some("alpha"))));

        assert_eq!(control.focus_requests(), 0);
        assert!(control.rename_requests().is_empty());
    }

    #[test]
    fn stopping_a_synthetic_host_is_clean() {
        let (monitor, _control) = AgentHostMonitor::synthetic_read_only();
        monitor.stop();
    }

    /// A fake herdr-core socket: answers every `terminal.list` line with one
    /// live terminal and records each request line it receives.
    struct FakeCoreSocket {
        path: PathBuf,
        requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
        server: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeCoreSocket {
        fn start() -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::os::unix::net::UnixListener;
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::{Arc, Mutex};

            let path = std::env::temp_dir().join(format!(
                "wave-tui-core-{}-{:?}.sock",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("bind fake core socket");
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let done = Arc::new(AtomicBool::new(false));
            let (seen, stop) = (Arc::clone(&requests), Arc::clone(&done));
            let server = std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok((stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut line = String::new();
                    let mut reader = BufReader::new(&stream);
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        continue;
                    }
                    let id = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|request| request.get("id").cloned())
                        .unwrap_or(serde_json::Value::Null);
                    seen.lock().unwrap().push(line);
                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "terminals": [{
                            "address": "t7",
                            "name": "build",
                            "title": "running tests",
                            "state": "live",
                            "reported": true,
                            "status": { "app": "demo-agent", "state": "working" },
                        }]},
                    });
                    let mut writer = &stream;
                    let _ = writeln!(writer, "{response}");
                }
            });
            Self {
                path,
                requests,
                done,
                server: Some(server),
            }
        }

        fn context(&self) -> AgentHostContext {
            AgentHostContext::Core(HerdrCoreContext {
                socket_path: self.path.clone(),
            })
        }

        fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|line| {
                    let request: serde_json::Value = serde_json::from_str(line).unwrap();
                    request["method"].as_str().unwrap_or_default().to_string()
                })
                .collect()
        }
    }

    impl Drop for FakeCoreSocket {
        fn drop(&mut self) {
            self.done.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_core_host_delivers_a_snapshot_and_never_dispatches_focus_or_rename() {
        let socket = FakeCoreSocket::start();
        let monitor = socket.context().spawn_monitor();
        assert_eq!(monitor.capabilities(), AgentCapabilities::READ_ONLY);

        let event = monitor
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("the first core poll reports a typed event");
        assert_eq!(
            event,
            MonitorEvent::Snapshot(vec![AgentSnapshot {
                id: AgentId::core("t7"),
                details: AgentDetails {
                    name: Some("build".to_string()),
                    agent: Some("demo-agent".to_string()),
                    activity: Some("running tests".to_string()),
                },
                status: AgentStatus::Working,
            }])
        );

        // Explicit requests are dropped at the facade: no dispatch, no
        // misleading feedback, no completion of any kind.
        monitor.focus_agent(AgentId::core("t7"));
        assert!(!monitor.rename_agent(RenameRequest {
            request: RenameRequestId::FIRST,
            target: AgentId::core("t7"),
            name: Some("renamed".to_string()),
        }));
        assert!(monitor.focus_events().try_recv().is_err());
        assert!(monitor.rename_events().try_recv().is_err());

        monitor.stop();
        assert_eq!(
            socket.methods(),
            vec!["terminal.list".to_string()],
            "the only wire request is the read-only terminal.list poll"
        );
    }

    #[test]
    fn dropping_a_core_monitor_joins_its_thread() {
        let socket = FakeCoreSocket::start();
        let monitor = socket.context().spawn_monitor();
        monitor
            .events()
            .recv_timeout(Duration::from_secs(5))
            .expect("the first core poll reports a typed event");
        // The idle worker wakes on the closed stop channel and is joined.
        drop(monitor);
    }
}
