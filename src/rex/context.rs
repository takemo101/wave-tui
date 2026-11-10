//! Eligibility is local and explicit: never discover servers or sessions.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RexContext {
    pub(super) executable: PathBuf,
    pub(super) server: String,
    pub(super) session: String,
}

#[cfg(test)]
impl RexContext {
    pub(crate) fn test_context() -> Self {
        Self {
            executable: PathBuf::from("/nonexistent/rex"),
            server: "unix:///tmp/rex-test.sock".into(),
            session: "session:test".into(),
        }
    }
}

fn resolve(
    server: Option<&str>,
    session: Option<&str>,
    executable: Option<PathBuf>,
) -> Option<RexContext> {
    let server = server?.trim();
    let session = session?.trim();
    let endpoint = url::Url::parse(server).ok()?;
    if endpoint.scheme() != "unix"
        || endpoint.host_str().is_some()
        || endpoint.path().len() < 2
        || session.is_empty()
    {
        return None;
    }
    Some(RexContext {
        executable: executable?,
        server: server.to_owned(),
        session: session.to_owned(),
    })
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn cli_candidates(paths: &std::ffi::OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    // Do not let a checkout-local executable win through '.', an empty PATH
    // component, or a relative directory when the player is launched there.
    std::env::split_paths(paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("rex"))
}

fn find_cli() -> Option<PathBuf> {
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(path) = cli_candidates(&paths).find(|path| executable(path)) {
            return Some(path);
        }
    }
    // The Mac app is self-contained; installing a PATH symlink is optional.
    #[cfg(target_os = "macos")]
    for app in ["Rex.app", "Rex Beta.app"] {
        let path = PathBuf::from("/Applications")
            .join(app)
            .join("Contents/Helpers/Rex Server.rex/Contents/MacOS/rex");
        if executable(&path) {
            return Some(path);
        }
    }
    None
}

pub(crate) fn context_from_env() -> Option<RexContext> {
    // Do not even inspect PATH or the app bundle outside a qualified launch.
    let server = std::env::var("REX_SERVER").ok()?;
    let session = std::env::var("REX_SESSION").ok()?;
    resolve(Some(&server), Some(&session), find_cli())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_lookup_ignores_empty_and_relative_path_components() {
        let candidates: Vec<_> = cli_candidates(std::ffi::OsStr::new(
            ":.:relative/bin:/safe/bin::/other/bin:",
        ))
        .collect();
        assert_eq!(
            candidates,
            [
                PathBuf::from("/safe/bin/rex"),
                PathBuf::from("/other/bin/rex")
            ]
        );
    }

    #[test]
    fn requires_an_injected_local_server_session_and_cli() {
        let exe = Some(PathBuf::from("/test/rex"));
        let c = resolve(
            Some("unix:///tmp/rex.sock"),
            Some("session:test"),
            exe.clone(),
        )
        .unwrap();
        assert_eq!(c.session, "session:test");
        for (server, session, cli) in [
            (None, Some("session:test"), exe.clone()),
            (Some("unix:///tmp/rex.sock"), None, exe.clone()),
            (Some("unix:///tmp/rex.sock"), Some("  "), exe.clone()),
            (
                Some("https://remote.example"),
                Some("session:test"),
                exe.clone(),
            ),
            (Some("ssh://host"), Some("session:test"), exe.clone()),
            (
                Some("unix://host/tmp/socket"),
                Some("session:test"),
                exe.clone(),
            ),
            (Some("unix:///tmp/rex.sock"), Some("session:test"), None),
        ] {
            assert!(resolve(server, session, cli).is_none());
        }
    }
}
