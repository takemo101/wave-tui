//! Host-neutral Agent Planets vocabulary.
//!
//! These types describe agents, their normalized status, and the typed
//! outcomes of explicit requests, independent of which host supplies them.
//! Adapters (currently only [`crate::herdr`]) translate their own wire
//! formats into this vocabulary; the app and UI depend only on this module.

use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

/// Stable, opaque agent identity.
///
/// The representation is private so no caller can display or leak raw host
/// identifiers; identity is only compared, ordered, and hashed. A Herdr
/// identity is the workspace-qualified pane served by the current control
/// socket; a core identity is a single host-assigned address with no
/// workspace. The two kinds never compare equal.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AgentId(Identity);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identity {
    Herdr {
        workspace_id: String,
        pane_id: String,
    },
    // Constructed only by tests until the herdr-core adapter exists.
    #[cfg_attr(not(test), allow(dead_code))]
    Core { address: String },
}

/// Marker mixed into the hash of core identities only.
const CORE_HASH_TAG: &str = "core";

/// Herdr identities must hash exactly the sequence the former
/// `#[derive(Hash)]` struct produced (workspace, then pane, with no enum
/// discriminant): the UI derives each planet's placement seed from this hash,
/// so any change would move every existing planet.
impl Hash for AgentId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.0 {
            Identity::Herdr {
                workspace_id,
                pane_id,
            } => {
                workspace_id.hash(state);
                pane_id.hash(state);
            }
            Identity::Core { address } => {
                CORE_HASH_TAG.hash(state);
                address.hash(state);
            }
        }
    }
}

/// Herdr identities keep the exact `Debug` text of the former derived struct
/// (`AgentId { workspace_id: .., pane_id: .. }`): the UI characterization
/// digests format ids with `{:?}`, so a different shape would read as drift.
impl std::fmt::Debug for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Identity::Herdr {
                workspace_id,
                pane_id,
            } => f
                .debug_struct("AgentId")
                .field("workspace_id", workspace_id)
                .field("pane_id", pane_id)
                .finish(),
            Identity::Core { address } => {
                f.debug_struct("AgentId").field("address", address).finish()
            }
        }
    }
}

impl AgentId {
    /// Identity of a pane in a Herdr workspace. Intended for the Herdr
    /// adapter; other modules use it only to build test fixtures.
    pub(crate) fn herdr(workspace_id: impl Into<String>, pane_id: impl Into<String>) -> Self {
        Self(Identity::Herdr {
            workspace_id: workspace_id.into(),
            pane_id: pane_id.into(),
        })
    }

    /// Identity of a herdr-core agent address. Test-only until the core
    /// adapter lands.
    #[cfg(test)]
    pub(crate) fn core(address: impl Into<String>) -> Self {
        Self(Identity::Core {
            address: address.into(),
        })
    }

    /// Transport-only pane target for an explicit Herdr request. The opaque
    /// value must never be displayed; only `crate::herdr` may call this
    /// (enforced by a source-scan test). Core identities have no pane target.
    pub(crate) fn herdr_pane_target(&self) -> Option<&str> {
        match &self.0 {
            Identity::Herdr { pane_id, .. } => Some(pane_id),
            Identity::Core { .. } => None,
        }
    }
}

/// Recoverable outcome of an explicit request to focus the selected live pane.
/// No outcome contains raw host identifiers or server messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusResult {
    Focused,
    Unsupported,
    Missing,
    Unavailable,
    NoSelection,
}

/// Recoverable outcome of an explicit request to rename the selected live
/// agent. Raw host errors and identifiers stay inside the adapter.
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

/// One normalized agent from the active host.
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

/// The explicit requests the active host can honor.
///
/// Host-neutral startup context, not persistence: the runtime derives it from
/// the selected host once, before the first agent event, and the App gates the
/// matching controls on it at the reducer and effect boundary. Observation
/// (the live table, selection, details) needs no capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentCapabilities {
    /// Whether the host can jump to the selected agent's pane (`o`/`O`).
    pub(crate) focus: bool,
    /// Whether the host can rename the selected agent (`r`/`R`).
    pub(crate) rename: bool,
}

impl AgentCapabilities {
    /// Herdr's local socket supports explicit pane focus and rename.
    pub(crate) const HERDR: Self = Self {
        focus: true,
        rename: true,
    };

    /// An observe-only host: neither request exists, so the controls are inert
    /// rather than hidden-but-live.
    // Constructed only by tests until a read-only host is selected.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const READ_ONLY: Self = Self {
        focus: false,
        rename: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// Records every byte a value feeds its hasher, so the exact write
    /// sequence (not just a final digest) can be compared.
    #[derive(Default)]
    struct RecordingHasher(Vec<u8>);

    impl Hasher for RecordingHasher {
        fn finish(&self) -> u64 {
            0
        }

        fn write(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
    }

    fn recorded<T: Hash>(value: &T) -> Vec<u8> {
        let mut hasher = RecordingHasher::default();
        value.hash(&mut hasher);
        hasher.0
    }

    fn seeded<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn legacy_pair(workspace: &str, pane: &str) -> (String, String) {
        (workspace.to_string(), pane.to_string())
    }

    #[test]
    fn herdr_identity_hashes_the_exact_legacy_workspace_then_pane_sequence() {
        for (workspace, pane) in [("ws-1", "pane-7"), ("", ""), ("alpha", "p1")] {
            let id = AgentId::herdr(workspace, pane);
            let legacy = legacy_pair(workspace, pane);
            assert_eq!(recorded(&id), recorded(&legacy));
            assert_eq!(seeded(&id), seeded(&legacy));
        }
    }

    #[test]
    fn herdr_identity_seed_is_pinned_so_planet_placement_cannot_drift() {
        // Pins the DefaultHasher digest `geometry` derives planet seeds from.
        let expected = seeded(&legacy_pair("ws-1", "p1"));
        assert_eq!(seeded(&AgentId::herdr("ws-1", "p1")), expected);
        assert_ne!(
            seeded(&AgentId::herdr("ws-1", "p1")),
            seeded(&AgentId::herdr("ws-1", "p2"))
        );
    }

    #[test]
    fn herdr_and_core_identities_are_never_equal() {
        let herdr = AgentId::herdr("ws-1", "pane-1");
        let core = AgentId::core("pane-1");
        assert_ne!(herdr, core);
        assert_eq!(core, AgentId::core("pane-1"));
        assert_ne!(core, AgentId::core("pane-2"));
        assert_ne!(recorded(&herdr), recorded(&core));
    }

    #[test]
    fn herdr_ordering_is_workspace_then_pane_and_core_sorts_after() {
        let mut ids = vec![
            AgentId::core("a"),
            AgentId::herdr("ws-2", "p1"),
            AgentId::herdr("ws-1", "p2"),
            AgentId::herdr("ws-1", "p1"),
            AgentId::core("0"),
        ];
        ids.sort();
        assert_eq!(
            ids,
            vec![
                AgentId::herdr("ws-1", "p1"),
                AgentId::herdr("ws-1", "p2"),
                AgentId::herdr("ws-2", "p1"),
                AgentId::core("0"),
                AgentId::core("a"),
            ]
        );
    }

    #[test]
    fn only_herdr_identities_expose_a_transport_pane_target() {
        assert_eq!(
            AgentId::herdr("ws-1", "pane-7").herdr_pane_target(),
            Some("pane-7")
        );
        assert_eq!(AgentId::core("pane-7").herdr_pane_target(), None);
    }

    #[test]
    fn herdr_debug_text_is_the_legacy_struct_shape() {
        assert_eq!(
            format!("{:?}", AgentId::herdr("ws", "p1")),
            r#"AgentId { workspace_id: "ws", pane_id: "p1" }"#
        );
    }

    #[test]
    fn identity_debug_does_not_fabricate_a_workspace_for_core() {
        let rendered = format!("{:?}", AgentId::core("addr"));
        assert!(!rendered.contains("workspace"));
    }

    #[test]
    fn only_the_herdr_adapter_reads_the_transport_pane_target() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let name = path.to_string_lossy();
                if name.ends_with("src/herdr.rs") || name.ends_with("src/model/agent.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source");
                if text.contains("herdr_pane_target") {
                    offenders.push(name.into_owned());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "raw pane targeting escaped the Herdr adapter: {offenders:?}"
        );
    }

    #[test]
    fn capability_presets_name_exactly_the_supported_requests() {
        let herdr = AgentCapabilities::HERDR;
        let read_only = AgentCapabilities::READ_ONLY;
        assert_eq!((herdr.focus, herdr.rename), (true, true));
        assert_eq!((read_only.focus, read_only.rename), (false, false));
        assert_ne!(AgentCapabilities::HERDR, AgentCapabilities::READ_ONLY);
        let copied = AgentCapabilities::HERDR;
        assert_eq!(copied, AgentCapabilities::HERDR);
    }

    #[test]
    fn snapshot_and_event_carry_the_shared_vocabulary() {
        let snapshot = AgentSnapshot {
            id: AgentId::core("addr"),
            details: AgentDetails::default(),
            status: AgentStatus::Working,
        };
        let event = MonitorEvent::Snapshot(vec![snapshot.clone()]);
        assert_eq!(event, MonitorEvent::Snapshot(vec![snapshot]));
        assert_ne!(event, MonitorEvent::Failed);
        assert_ne!(FocusResult::Focused, FocusResult::NoSelection);
        assert_ne!(RenameResult::Renamed, RenameResult::Missing);
    }
}
