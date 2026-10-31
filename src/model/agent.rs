//! Host-neutral Agent Planets vocabulary.
//!
//! These types describe agents, their normalized status, and the typed
//! outcomes of explicit requests, independent of which host supplies them.
//! Adapters ([`crate::herdr`] and the read-only herdr-core adapter) translate
//! their own wire formats into this vocabulary; the app and UI depend only on
//! this module.

use std::hash::{Hash, Hasher};

/// Which Agent Planets host a run may use, as chosen with `--agent-host`.
///
/// A per-run boundary value, never persisted. `Auto` prefers an eligible
/// Herdr plugin environment and only then an eligible herdr-core terminal; the
/// explicit variants resolve exactly one host and never fall back to another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentHostSelection {
    #[default]
    Auto,
    Herdr,
    HerdrCore,
    Off,
}

impl AgentHostSelection {
    /// Strict boundary parser for the `--agent-host` value. Accepts exactly
    /// `auto`, `herdr`, `herdr-core`, and `off` (case-insensitive, trimmed);
    /// anything else is `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "herdr" => Some(Self::Herdr),
            "herdr-core" => Some(Self::HerdrCore),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

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
    Core {
        address: String,
    },
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

    /// Identity of a herdr-core agent address. Intended for the herdr-core
    /// adapter; other modules use it only to build test fixtures.
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

/// Opaque, monotonic identity of one submitted rename. The app allocates it at
/// submission and the host echoes it back on the matching completion, so a late
/// result can only ever settle the submission that produced it. The value is
/// private: it is compared, never displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenameRequestId(u64);

impl RenameRequestId {
    /// The first id an app hands out.
    pub(crate) const FIRST: Self = Self(1);

    /// The id allocated after this one.
    pub(crate) fn following(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// One explicit rename handed to a host: the submission it belongs to, the
/// opaque target, and the normalized name (`None` clears the explicit name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRequest {
    pub(crate) request: RenameRequestId,
    pub(crate) target: AgentId,
    pub(crate) name: Option<String>,
}

/// A host's typed answer to one [`RenameRequest`]: the submission it settles
/// plus the unchanged recoverable [`RenameResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenameCompletion {
    pub(crate) request: RenameRequestId,
    pub(crate) result: RenameResult,
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

    /// An observe-only host (herdr-core): neither request exists, so the
    /// controls are inert rather than hidden-but-live.
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

    /// Records each `write` call as its own entry, so the exact write
    /// sequence and its boundaries (not a concatenation or a final digest)
    /// can be compared.
    #[derive(Default)]
    struct RecordingHasher(Vec<Vec<u8>>);

    impl Hasher for RecordingHasher {
        fn finish(&self) -> u64 {
            0
        }

        fn write(&mut self, bytes: &[u8]) {
            self.0.push(bytes.to_vec());
        }
    }

    fn recorded<T: Hash>(value: &T) -> Vec<Vec<u8>> {
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
    fn recorded_writes_keep_their_boundaries_instead_of_concatenating() {
        // Fields that split the same bytes differently must record differently;
        // a concatenating recorder would make these two identities look equal.
        assert_ne!(
            recorded(&AgentId::herdr("ab", "c")),
            recorded(&AgentId::herdr("a", "bc"))
        );
        assert_eq!(
            recorded(&AgentId::herdr("ws", "p1")),
            vec![b"ws".to_vec(), vec![0xff], b"p1".to_vec(), vec![0xff],],
            "workspace then pane, each str followed by its 0xff terminator, no discriminant"
        );
        assert_eq!(
            recorded(&AgentId::core("addr")),
            vec![b"core".to_vec(), vec![0xff], b"addr".to_vec(), vec![0xff],],
            "core hashes its tag then the address"
        );
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
    fn agent_host_selection_defaults_to_auto() {
        assert_eq!(AgentHostSelection::default(), AgentHostSelection::Auto);
    }

    #[test]
    fn agent_host_selection_parses_exactly_the_four_documented_values() {
        let cases = [
            ("auto", AgentHostSelection::Auto),
            ("herdr", AgentHostSelection::Herdr),
            ("herdr-core", AgentHostSelection::HerdrCore),
            ("off", AgentHostSelection::Off),
            (" Herdr-Core ", AgentHostSelection::HerdrCore),
        ];
        for (raw, expected) in cases {
            assert_eq!(AgentHostSelection::parse(raw), Some(expected), "{raw:?}");
        }
        for raw in [
            "",
            "core",
            "herdr_core",
            "herdrcore",
            "on",
            "all",
            "herdr,core",
        ] {
            assert_eq!(AgentHostSelection::parse(raw), None, "{raw:?}");
        }
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
