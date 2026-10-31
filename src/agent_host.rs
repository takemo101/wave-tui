//! Agent host facade: the one door the runtime uses to reach an Agent Planets
//! host.
//!
//! The runtime, event loop, and input effects speak only this module's
//! [`AgentHostContext`] and [`AgentHostMonitor`]. Today the facade wraps the
//! Herdr monitor and nothing else: production selects Herdr only, there is no
//! host registry, no trait object, and no multi-host aggregation. A new host
//! becomes one more variant here, never a framework.
//!
//! What the facade guarantees:
//!
//! - **Typed surface.** Snapshots, focus results, and rename results arrive on
//!   three separate receivers in the shared [`crate::model::agent`]
//!   vocabulary; no transport data (sockets, pane ids, JSON) crosses it.
//! - **Capabilities as startup context.** [`AgentHostContext::capabilities`]
//!   names the explicit requests the host honors. The runtime hands them to the
//!   `App` once, before the first agent event.
//! - **Capability-gated dispatch.** [`AgentHostMonitor::focus_agent`] and
//!   [`AgentHostMonitor::rename_agent`] are no-ops for an unsupported request,
//!   so even a caller that skipped the reducer gate cannot reach a host that
//!   lacks the request.
//! - **Owned lifetime.** [`AgentHostMonitor::stop`] (or dropping it) stops and
//!   joins the host's polling thread.

use std::sync::mpsc::Receiver;

use crate::herdr::{self, HerdrContext, HerdrMonitor};
use crate::model::agent::{
    AgentCapabilities, AgentId, FocusResult, MonitorEvent, RenameCompletion, RenameRequest,
};

/// Eligibility of an Agent Planets host for this launch, with the host's
/// startup context. Absent for standalone, ineligible, and `--no-agent-pulse`
/// launches, so nothing is created and behavior stays exactly standalone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentHostContext {
    Herdr(HerdrContext),
}

impl AgentHostContext {
    /// Read host eligibility from the process environment. `disabled` is the
    /// `--no-agent-pulse` flag. Production selects Herdr only.
    pub(crate) fn from_env(disabled: bool) -> Option<Self> {
        herdr::context_from_env(disabled).map(Self::Herdr)
    }

    /// The explicit requests this host honors.
    pub(crate) fn capabilities(&self) -> AgentCapabilities {
        match self {
            Self::Herdr(_) => AgentCapabilities::HERDR,
        }
    }

    /// Start the host's background monitor.
    pub(crate) fn spawn_monitor(self) -> AgentHostMonitor {
        let capabilities = self.capabilities();
        let backend = match self {
            Self::Herdr(context) => Backend::Herdr(herdr::spawn_monitor(context)),
        };
        AgentHostMonitor {
            capabilities,
            backend,
        }
    }
}

enum Backend {
    Herdr(HerdrMonitor),
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
            #[cfg(test)]
            Backend::Synthetic(synthetic) => &synthetic.events,
        }
    }

    /// Typed results of explicit focus requests, separate from polling.
    pub(crate) fn focus_events(&self) -> &Receiver<FocusResult> {
        match &self.backend {
            Backend::Herdr(monitor) => monitor.focus_events(),
            #[cfg(test)]
            Backend::Synthetic(synthetic) => &synthetic.focus_events,
        }
    }

    /// Typed completions of explicit rename requests, separate from polling.
    /// Each carries the id of the submission it settles.
    pub(crate) fn rename_events(&self) -> &Receiver<RenameCompletion> {
        match &self.backend {
            Backend::Herdr(monitor) => monitor.rename_events(),
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
            #[cfg(test)]
            Backend::Synthetic(synthetic) => synthetic.record_rename(request),
        }
    }

    /// Stop the host and join its polling thread.
    pub(crate) fn stop(self) {
        match self.backend {
            Backend::Herdr(monitor) => monitor.stop(),
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
    use crate::model::agent::{RenameRequestId, RenameResult};
    use std::path::PathBuf;
    use std::time::Duration;

    fn dead_socket_context() -> AgentHostContext {
        AgentHostContext::Herdr(HerdrContext {
            socket_path: PathBuf::from("/nonexistent/wave-tui-agent-host.sock"),
        })
    }

    #[test]
    fn disabled_launches_create_no_host_context() {
        assert!(AgentHostContext::from_env(true).is_none());
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
}
