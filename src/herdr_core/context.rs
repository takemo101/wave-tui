//! herdr-core eligibility and Unix socket endpoint resolution.
//!
//! Resolution mirrors the pinned herdr-core `Paths::from_env` (socket
//! precedence, session validation, `platform/unix.rs` defaults) but is a pure
//! function of an injected [`CoreEnv`] and [`CoreFs`], so no test mutates the
//! process environment or touches a real home directory. wave-tui adds one
//! rule of its own: configuration and runtime-path fallbacks apply only when
//! the process was launched inside a core terminal (`HC_TERMINAL_ID`).

use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HerdrCoreContext {
    pub(crate) socket_path: PathBuf,
}

/// The environment variables resolution reads, captured once.
#[derive(Debug, Default, Clone)]
pub(super) struct CoreEnv {
    pub(super) hc_socket: Option<OsString>,
    pub(super) hc_terminal_id: Option<OsString>,
    pub(super) hc_terminal_socket: Option<OsString>,
    /// Read with `env::var().ok()` like the core, so a non-Unicode value
    /// behaves as unset.
    pub(super) hc_session: Option<String>,
    pub(super) home: Option<OsString>,
    pub(super) xdg_state_home: Option<OsString>,
    pub(super) xdg_config_home: Option<OsString>,
    pub(super) xdg_runtime_dir: Option<OsString>,
}

impl CoreEnv {
    fn from_process() -> Self {
        let var = |name: &str| std::env::var_os(name);
        Self {
            hc_socket: var("HC_SOCKET"),
            hc_terminal_id: var("HC_TERMINAL_ID"),
            hc_terminal_socket: var("HC_TERMINAL_SOCKET"),
            hc_session: std::env::var("HC_SESSION").ok(),
            home: var("HOME"),
            xdg_state_home: var("XDG_STATE_HOME"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
        }
    }
}

/// The only filesystem and identity facts resolution consults.
pub(super) trait CoreFs {
    /// The text of the core config file, or `None` when it cannot be read.
    fn read_config(&self, path: &Path) -> Option<String>;
    fn is_dir(&self, path: &Path) -> bool;
    fn uid(&self) -> u32;
}

struct ProcessFs;

impl CoreFs for ProcessFs {
    fn read_config(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn uid(&self) -> u32 {
        // SAFETY: `getuid` has no preconditions, takes no pointers, and
        // cannot fail.
        unsafe { libc::getuid() }
    }
}

/// Reads eligibility and the endpoint from the process environment. Returns
/// `None` for every launch that is not under a core.
pub(crate) fn context_from_env() -> Option<HerdrCoreContext> {
    resolve(&CoreEnv::from_process(), &ProcessFs)
}

/// An explicit path value is kept raw; only an all-blank one is rejected.
fn nonblank_path(value: &Option<OsString>) -> Option<PathBuf> {
    let value = value.as_ref()?;
    if value.to_str().is_some_and(|text| text.trim().is_empty()) {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Environment values the core itself treats as unset only when empty.
fn nonempty_path(value: &Option<OsString>) -> Option<PathBuf> {
    value
        .as_ref()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Deserialize)]
struct CoreConfig {
    socket: Option<String>,
}

/// The configured socket. A missing, unreadable, or malformed file and an
/// unknown field are all ignored, as the core ignores a failed config load
/// when choosing its socket.
fn configured_socket(env: &CoreEnv, fs: &dyn CoreFs) -> Option<PathBuf> {
    let config_home =
        nonempty_path(&env.xdg_config_home).unwrap_or_else(|| home(env).join(".config"));
    let path = config_home.join("herdr-core").join("config.toml");
    let config: CoreConfig = toml::from_str(&fs.read_config(&path)?).ok()?;
    nonblank_path(&config.socket.map(OsString::from))
}

fn home(env: &CoreEnv) -> PathBuf {
    env.home
        .as_ref()
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The core's session name: trimmed, `default` when blank, and refused when it
/// could not be a folder name.
fn session(raw: Option<&str>) -> Option<String> {
    let name = raw
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("default");
    let invalid = name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || r#"/\:*?"<>|"#.contains(c));
    (!invalid).then(|| name.to_owned())
}

/// The core's default socket for the session, per its Unix platform layer.
fn session_socket(env: &CoreEnv, fs: &dyn CoreFs) -> Option<PathBuf> {
    let session = session(env.hc_session.as_deref())?;
    let state = nonempty_path(&env.xdg_state_home)
        .unwrap_or_else(|| home(env).join(".local/state"))
        .join("herdr-core")
        .join(&session);
    let run = nonempty_path(&env.xdg_runtime_dir)
        .or_else(|| Some(PathBuf::from(format!("/run/user/{}", fs.uid()))).filter(|p| fs.is_dir(p)))
        .map_or_else(|| state, |run| run.join("herdr-core"));
    Some(run.join(format!("{session}.sock")))
}

/// Eligibility and endpoint. A nonblank `HC_SOCKET` alone is eligible. Every
/// other path requires a nonblank `HC_TERMINAL_ID` and then takes, in order,
/// the injected `HC_TERMINAL_SOCKET`, the configured socket, and the session
/// default. `HC_SESSION`, a default path, or `HC_TERMINAL_SOCKET` alone never
/// opt in.
fn resolve(env: &CoreEnv, fs: &dyn CoreFs) -> Option<HerdrCoreContext> {
    let socket_path = nonblank_path(&env.hc_socket).or_else(|| {
        nonblank_path(&env.hc_terminal_id)?;
        nonblank_path(&env.hc_terminal_socket)
            .or_else(|| configured_socket(env, fs))
            .or_else(|| session_socket(env, fs))
    })?;
    Some(HerdrCoreContext { socket_path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeFs {
        config: Option<String>,
        dirs: Vec<PathBuf>,
        uid: u32,
        config_reads: RefCell<Vec<PathBuf>>,
    }

    impl CoreFs for FakeFs {
        fn read_config(&self, path: &Path) -> Option<String> {
            self.config_reads.borrow_mut().push(path.to_path_buf());
            self.config.clone()
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.iter().any(|dir| dir == path)
        }

        fn uid(&self) -> u32 {
            self.uid
        }
    }

    fn os(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    fn terminal_env() -> CoreEnv {
        CoreEnv {
            hc_terminal_id: os("t7"),
            home: os("/home/u"),
            ..CoreEnv::default()
        }
    }

    fn socket(env: &CoreEnv, fs: &FakeFs) -> Option<PathBuf> {
        resolve(env, fs).map(|context| context.socket_path)
    }

    #[test]
    fn hc_socket_alone_is_eligible_and_kept_raw() {
        let env = CoreEnv {
            hc_socket: os(" /tmp/my core.sock "),
            ..CoreEnv::default()
        };
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from(" /tmp/my core.sock "))
        );
    }

    #[test]
    fn explicit_socket_needs_no_fallback_data() {
        let env = CoreEnv {
            hc_socket: os("/tmp/core.sock"),
            hc_session: Some("a/b".to_string()),
            ..CoreEnv::default()
        };
        let fs = FakeFs::default();
        assert_eq!(socket(&env, &fs), Some(PathBuf::from("/tmp/core.sock")));
        assert!(fs.config_reads.borrow().is_empty());
    }

    #[test]
    fn blank_hc_socket_is_not_eligible_without_terminal_id() {
        for blank in ["", "   ", "\t"] {
            let env = CoreEnv {
                hc_socket: os(blank),
                hc_terminal_socket: os("/tmp/injected.sock"),
                home: os("/home/u"),
                ..CoreEnv::default()
            };
            assert_eq!(socket(&env, &FakeFs::default()), None, "{blank:?}");
        }
    }

    #[test]
    fn session_default_and_home_alone_are_not_eligible() {
        let env = CoreEnv {
            hc_session: Some("work".to_string()),
            home: os("/home/u"),
            xdg_runtime_dir: os("/run/x"),
            ..CoreEnv::default()
        };
        assert_eq!(socket(&env, &FakeFs::default()), None);
        assert_eq!(socket(&CoreEnv::default(), &FakeFs::default()), None);
    }

    #[test]
    fn terminal_socket_alone_is_not_eligible() {
        let env = CoreEnv {
            hc_terminal_socket: os("/tmp/injected.sock"),
            ..CoreEnv::default()
        };
        assert_eq!(socket(&env, &FakeFs::default()), None);
    }

    #[test]
    fn blank_terminal_id_is_not_eligible() {
        for blank in ["", " ", "\n"] {
            let mut env = terminal_env();
            env.hc_terminal_id = os(blank);
            env.hc_terminal_socket = os("/tmp/injected.sock");
            assert_eq!(socket(&env, &FakeFs::default()), None, "{blank:?}");
        }
    }

    #[test]
    fn terminal_socket_applies_with_terminal_id() {
        let mut env = terminal_env();
        env.hc_terminal_socket = os("/tmp/injected.sock");
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from("/tmp/injected.sock"))
        );
    }

    #[test]
    fn hc_socket_beats_terminal_socket_beats_config_beats_fallback() {
        let fs = FakeFs {
            config: Some("socket = \"/cfg/core.sock\"".to_string()),
            ..FakeFs::default()
        };
        let mut env = terminal_env();
        env.hc_socket = os("/explicit.sock");
        env.hc_terminal_socket = os("/injected.sock");
        assert_eq!(socket(&env, &fs), Some(PathBuf::from("/explicit.sock")));

        env.hc_socket = os("  ");
        assert_eq!(socket(&env, &fs), Some(PathBuf::from("/injected.sock")));

        env.hc_terminal_socket = os("");
        assert_eq!(socket(&env, &fs), Some(PathBuf::from("/cfg/core.sock")));

        let no_config = FakeFs::default();
        assert_eq!(
            socket(&env, &no_config),
            Some(PathBuf::from(
                "/home/u/.local/state/herdr-core/default/default.sock"
            ))
        );
    }

    #[test]
    fn config_is_read_from_the_core_config_path() {
        let fs = FakeFs::default();
        socket(&terminal_env(), &fs);
        assert_eq!(
            *fs.config_reads.borrow(),
            vec![PathBuf::from("/home/u/.config/herdr-core/config.toml")]
        );

        let mut env = terminal_env();
        env.xdg_config_home = os("/xdg/config");
        let fs = FakeFs::default();
        socket(&env, &fs);
        assert_eq!(
            *fs.config_reads.borrow(),
            vec![PathBuf::from("/xdg/config/herdr-core/config.toml")]
        );

        env.xdg_config_home = os("");
        let fs = FakeFs::default();
        socket(&env, &fs);
        assert_eq!(
            *fs.config_reads.borrow(),
            vec![PathBuf::from("/home/u/.config/herdr-core/config.toml")]
        );
    }

    #[test]
    fn config_is_not_read_when_an_earlier_endpoint_wins() {
        let fs = FakeFs::default();
        let mut env = terminal_env();
        env.hc_terminal_socket = os("/injected.sock");
        socket(&env, &fs);
        assert!(fs.config_reads.borrow().is_empty());
    }

    #[test]
    fn config_failures_fall_back_to_the_session_socket() {
        let fallback = Some(PathBuf::from(
            "/home/u/.local/state/herdr-core/default/default.sock",
        ));
        for config in [
            None,
            Some("socket = ".to_string()),
            Some("not toml at all [[[".to_string()),
            Some("socket = 42".to_string()),
            Some("socket = \"\"".to_string()),
            Some("socket = \"   \"".to_string()),
            Some("[other]\nkey = 1".to_string()),
            Some(String::new()),
        ] {
            let fs = FakeFs {
                config: config.clone(),
                ..FakeFs::default()
            };
            assert_eq!(socket(&terminal_env(), &fs), fallback, "{config:?}");
        }
    }

    #[test]
    fn config_ignores_unknown_fields_and_keeps_the_socket_raw() {
        let fs = FakeFs {
            config: Some(
                "theme = \"x\"\nsocket = \" /cfg/s.sock\"\n[future]\nanything = [1, 2]\n"
                    .to_string(),
            ),
            ..FakeFs::default()
        };
        assert_eq!(
            socket(&terminal_env(), &fs),
            Some(PathBuf::from(" /cfg/s.sock"))
        );
    }

    #[test]
    fn fallback_prefers_a_nonempty_runtime_dir() {
        let mut env = terminal_env();
        env.xdg_runtime_dir = os("/run/xdg");
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from("/run/xdg/herdr-core/default.sock"))
        );
    }

    #[test]
    fn fallback_uses_run_user_uid_only_when_it_is_a_directory() {
        let fs = FakeFs {
            uid: 501,
            dirs: vec![PathBuf::from("/run/user/501")],
            ..FakeFs::default()
        };
        assert_eq!(
            socket(&terminal_env(), &fs),
            Some(PathBuf::from("/run/user/501/herdr-core/default.sock"))
        );

        let other_uid = FakeFs {
            uid: 502,
            dirs: vec![PathBuf::from("/run/user/501")],
            ..FakeFs::default()
        };
        assert_eq!(
            socket(&terminal_env(), &other_uid),
            Some(PathBuf::from(
                "/home/u/.local/state/herdr-core/default/default.sock"
            ))
        );
    }

    #[test]
    fn fallback_without_runtime_dir_sits_in_the_session_state_dir() {
        let mut env = terminal_env();
        env.hc_session = Some("work".to_string());
        env.xdg_runtime_dir = os("");
        let path = socket(&env, &FakeFs::default()).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/u/.local/state/herdr-core/work/work.sock")
        );
        assert_eq!(
            path.to_string_lossy().matches("herdr-core").count(),
            1,
            "no double herdr-core"
        );
    }

    #[test]
    fn xdg_state_home_overrides_home_state_only_when_nonempty() {
        let mut env = terminal_env();
        env.xdg_state_home = os("/xdg/state");
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from("/xdg/state/herdr-core/default/default.sock"))
        );
        env.xdg_state_home = os("");
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from(
                "/home/u/.local/state/herdr-core/default/default.sock"
            ))
        );
    }

    #[test]
    fn unset_home_defaults_to_root_like_the_core() {
        let mut env = terminal_env();
        env.home = None;
        let fs = FakeFs::default();
        assert_eq!(
            socket(&env, &fs),
            Some(PathBuf::from(
                "/.local/state/herdr-core/default/default.sock"
            ))
        );
        assert_eq!(
            *fs.config_reads.borrow(),
            vec![PathBuf::from("/.config/herdr-core/config.toml")]
        );
    }

    #[test]
    fn session_is_trimmed_and_defaults_when_blank() {
        for (raw, name) in [
            (Some("smoke "), "smoke"),
            (Some("  "), "default"),
            (None, "default"),
            (Some("MyTest"), "MyTest"),
            (Some("a.b-c_d"), "a.b-c_d"),
        ] {
            let mut env = terminal_env();
            env.hc_session = raw.map(str::to_owned);
            assert_eq!(
                socket(&env, &FakeFs::default()),
                Some(PathBuf::from(format!(
                    "/home/u/.local/state/herdr-core/{name}/{name}.sock"
                ))),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn invalid_session_makes_the_fallback_ineligible() {
        for bad in [
            "a/b", "a\\b", "a:b", "..", ".", "a\tb", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b",
        ] {
            let mut env = terminal_env();
            env.hc_session = Some(bad.to_string());
            assert_eq!(socket(&env, &FakeFs::default()), None, "{bad:?}");
        }
    }

    #[test]
    fn invalid_session_does_not_block_a_configured_or_injected_socket() {
        let mut env = terminal_env();
        env.hc_session = Some("a/b".to_string());
        env.hc_terminal_socket = os("/injected.sock");
        assert_eq!(
            socket(&env, &FakeFs::default()),
            Some(PathBuf::from("/injected.sock"))
        );
        env.hc_terminal_socket = None;
        let fs = FakeFs {
            config: Some("socket = \"/cfg.sock\"".to_string()),
            ..FakeFs::default()
        };
        assert_eq!(socket(&env, &fs), Some(PathBuf::from("/cfg.sock")));
    }
}
