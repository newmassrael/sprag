//! WHICH daemon a process talks to — as a value that also carries WHY it is that one.
//!
//! Every sprag process that speaks the host wire has to answer one question first: which socket.
//! The answer used to be a bare [`PathBuf`], and a path alone drops the only part an operator
//! needs when it is wrong — whether something NAMED it or whether the process merely defaulted.
//!
//! That omission is not hypothetical. A probe once exported `SPRAG_HOST_RPC_SOCK` and ran a
//! client that reads `SPRAG_GUI_HOST_SOCK`; the client found nothing, silently fell back to the
//! well-known socket, and drove the machine's LIVE daemon for an afternoon. Nothing it printed
//! could have said so, because by the time anything was printed the provenance was gone.
//!
//! So the endpoint is a TYPE here, and its [`Display`](std::fmt::Display) renders the path AND
//! its origin. A message that names the endpoint therefore names where it came from
//! structurally — a call site cannot forget to, having never held a bare path to print.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::{HOST_SOCKET, HOST_SOCKET_NAME, SocketOpts, resolve_socket_path};

/// The env var a DISPLAY CLIENT overrides its host endpoint with — `sprag attach --tui` pins it
/// for the client it launches, and the GUI pixel smoke pins it for the daemon it booted.
///
/// Named here, beside the daemon's own [`HOST_SOCKET`] policy, because the two are one endpoint
/// seen from two ends: a client that resolves them independently of the daemon is exactly how a
/// probe's client and a probe's daemon end up on different sockets.
pub const CLIENT_SOCKET_ENV: &str = "SPRAG_GUI_HOST_SOCK";

/// What a client consults, in precedence order, before the well-known default.
///
/// `SPRAG_HOST_RPC_SOCK` (the daemon's own path env) is second and that is the load-bearing
/// entry: `sprag-term` exports it into every pane it births (`sprag_host::pane_env_source` — not
/// linked, because this crate sits BELOW that one and must not depend on it), so a client started
/// INSIDE a pane belongs to the daemon that owns that pane — `$TMUX` semantics. `sprag-mcp`
/// already resolves by this var; the display clients were the one surface that ignored it.
pub const CLIENT_SOCKET_ENVS: [&str; 2] = [CLIENT_SOCKET_ENV, HOST_SOCKET.path_env];

/// How an endpoint's path was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointOrigin {
    /// The value of this env var.
    Named(&'static str),
    /// Handed to the process directly rather than resolved from the environment — a CLI flag, a
    /// test harness. The string says by what, and is rendered, so "given" is never anonymous.
    Given(&'static str),
    /// Nothing named it: the well-known socket under the runtime dir. The vars that WERE
    /// consulted are carried by the endpoint ([`HostEndpoint::checked`]) so the rendering can
    /// tell an operator which knob would have changed the answer.
    Default,
}

/// A host socket, and how this process came to be pointed at it.
///
/// Construct with [`client`](Self::client) (a display client's precedence),
/// [`for_opts`](Self::for_opts) (a daemon or the CLI, from its [`SocketOpts`]) or
/// [`given`](Self::given) (a path this process was handed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEndpoint {
    path: PathBuf,
    origin: EndpointOrigin,
    /// The env vars consulted, in precedence order — empty for a [`Given`](EndpointOrigin::Given)
    /// endpoint, which consulted none.
    checked: Vec<&'static str>,
}

impl HostEndpoint {
    /// The endpoint a DISPLAY CLIENT (the GUI, the terminal client) connect-or-spawns on:
    /// [`CLIENT_SOCKET_ENV`], else the daemon's own `SPRAG_HOST_RPC_SOCK`, else the well-known
    /// socket under the runtime dir.
    ///
    /// An EMPTY value counts as unset — an empty path names no socket, and the alternative is a
    /// connect failure against `""` that reads like a missing daemon. That is the rule the
    /// requested-session env already follows on this path.
    #[must_use]
    pub fn client() -> Self {
        Self::resolve(
            &[
                (CLIENT_SOCKET_ENV, std::env::var_os(CLIENT_SOCKET_ENV)),
                (HOST_SOCKET.path_env, std::env::var_os(HOST_SOCKET.path_env)),
            ],
            std::env::var_os("XDG_RUNTIME_DIR"),
            HOST_SOCKET_NAME,
        )
    }

    /// The endpoint `opts` describes — the daemon's own view of where it listens, and the `sprag`
    /// CLI's of where it asks. One env var, then the well-known name under the runtime dir.
    #[must_use]
    pub fn for_opts(opts: SocketOpts) -> Self {
        Self::resolve(
            &[(opts.path_env, std::env::var_os(opts.path_env))],
            std::env::var_os("XDG_RUNTIME_DIR"),
            opts.socket_name,
        )
    }

    /// An endpoint this process was HANDED — a `--socket` flag, a test harness — described by
    /// `by` (rendered, so the reader learns who chose it).
    #[must_use]
    pub fn given(by: &'static str, path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            origin: EndpointOrigin::Given(by),
            checked: Vec::new(),
        }
    }

    /// The socket path itself, for the connect.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How the path was decided.
    #[must_use]
    pub fn origin(&self) -> EndpointOrigin {
        self.origin
    }

    /// The env vars consulted, in precedence order (empty for a
    /// [`Given`](EndpointOrigin::Given) endpoint).
    #[must_use]
    pub fn checked(&self) -> &[&'static str] {
        &self.checked
    }

    /// Consume the endpoint for its path — for a caller that needs the path alone (a daemon
    /// deriving its lock and log files from the one path that identifies it).
    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }

    /// `error`, re-reported with this endpoint in front of it — the ONE way a wire failure is
    /// told, so no failure can name a daemon without saying which one it means.
    ///
    /// The [`kind`](io::Error::kind) is preserved: a caller matching on `NotFound` or
    /// `ConnectionRefused` must keep working through the added context.
    #[must_use]
    pub fn context(&self, error: &io::Error) -> io::Error {
        io::Error::new(error.kind(), format!("{self}: {error}"))
    }

    /// The shared precedence walk (env values injected so it is testable without touching the
    /// process environment): the first candidate with a NON-EMPTY value names the path;
    /// otherwise the well-known `socket_name` under the runtime dir.
    fn resolve(
        candidates: &[(&'static str, Option<OsString>)],
        xdg_runtime: Option<OsString>,
        socket_name: &str,
    ) -> Self {
        let checked: Vec<&'static str> = candidates.iter().map(|(var, _)| *var).collect();
        for (var, value) in candidates {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                return Self {
                    path: PathBuf::from(value),
                    origin: EndpointOrigin::Named(var),
                    checked,
                };
            }
        }
        Self {
            path: resolve_socket_path(None, xdg_runtime, socket_name),
            origin: EndpointOrigin::Default,
            checked,
        }
    }
}

impl fmt::Display for HostEndpoint {
    /// The path, then its provenance in parentheses — the form every wire failure is reported in.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.display())?;
        match self.origin {
            EndpointOrigin::Named(var) => write!(f, " (named by {var})"),
            EndpointOrigin::Given(by) => write!(f, " (given by {by})"),
            EndpointOrigin::Default => {
                write!(f, " (the well-known default; ")?;
                for (i, var) in self.checked.iter().enumerate() {
                    if i > 0 {
                        write!(f, " and ")?;
                    }
                    write!(f, "{var}")?;
                }
                let verb = if self.checked.len() == 1 { "is" } else { "are" };
                write!(f, " {verb} unset)")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A candidate list from string values, for the precedence tests.
    fn env(pairs: &[(&'static str, Option<&str>)]) -> Vec<(&'static str, Option<OsString>)> {
        pairs
            .iter()
            .map(|(var, value)| (*var, value.map(OsString::from)))
            .collect()
    }

    fn client_like(gui: Option<&str>, host: Option<&str>) -> HostEndpoint {
        HostEndpoint::resolve(
            &env(&[(CLIENT_SOCKET_ENV, gui), (HOST_SOCKET.path_env, host)]),
            Some(OsString::from("/run/user/1000")),
            HOST_SOCKET_NAME,
        )
    }

    #[test]
    fn the_client_override_wins_over_the_daemon_env() {
        let endpoint = client_like(Some("/tmp/gui.sock"), Some("/tmp/host.sock"));
        assert_eq!(endpoint.path(), Path::new("/tmp/gui.sock"));
        assert_eq!(endpoint.origin(), EndpointOrigin::Named(CLIENT_SOCKET_ENV));
    }

    /// The level R278's incident was missing: a client born inside a pane belongs to the daemon
    /// that owns the pane, and that daemon exports its own path env into every pane it births.
    #[test]
    fn the_daemon_env_names_the_endpoint_when_no_client_override_does() {
        let endpoint = client_like(None, Some("/tmp/host.sock"));
        assert_eq!(endpoint.path(), Path::new("/tmp/host.sock"));
        assert_eq!(
            endpoint.origin(),
            EndpointOrigin::Named(HOST_SOCKET.path_env),
            "the daemon this process was launched under is the endpoint, not the well-known one",
        );
    }

    #[test]
    fn nothing_named_it_falls_back_to_the_well_known_socket() {
        let endpoint = client_like(None, None);
        assert_eq!(endpoint.path(), Path::new("/run/user/1000/sprag-host.sock"));
        assert_eq!(endpoint.origin(), EndpointOrigin::Default);
    }

    /// An empty override is a mistake, not an address: treating it as a path would connect to
    /// `""` and report a missing daemon, which is the wrong diagnosis of the wrong problem.
    #[test]
    fn an_empty_value_does_not_name_an_endpoint() {
        let endpoint = client_like(Some(""), Some("/tmp/host.sock"));
        assert_eq!(endpoint.path(), Path::new("/tmp/host.sock"));
        assert_eq!(
            endpoint.origin(),
            EndpointOrigin::Named(HOST_SOCKET.path_env)
        );
    }

    #[test]
    fn display_names_the_var_that_chose_the_path() {
        let rendered = client_like(Some("/tmp/gui.sock"), None).to_string();
        assert_eq!(rendered, "/tmp/gui.sock (named by SPRAG_GUI_HOST_SOCK)");
    }

    /// The item-4 fix, asserted: the default is still the default, but it says so and lists every
    /// var that would have changed it.
    #[test]
    fn display_says_when_nothing_named_the_path() {
        let rendered = client_like(None, None).to_string();
        assert_eq!(
            rendered,
            "/run/user/1000/sprag-host.sock (the well-known default; SPRAG_GUI_HOST_SOCK and \
             SPRAG_HOST_RPC_SOCK are unset)",
        );
    }

    /// One consulted var reads as a sentence too — the daemon/CLI side, which has only its own.
    #[test]
    fn display_agrees_with_itself_for_a_single_consulted_var() {
        let endpoint = HostEndpoint::resolve(
            &env(&[(HOST_SOCKET.path_env, None)]),
            Some(OsString::from("/run/user/1000")),
            HOST_SOCKET_NAME,
        );
        assert_eq!(
            endpoint.to_string(),
            "/run/user/1000/sprag-host.sock (the well-known default; SPRAG_HOST_RPC_SOCK is \
             unset)",
        );
    }

    #[test]
    fn a_given_endpoint_says_who_gave_it() {
        let endpoint = HostEndpoint::given("the test harness", "/tmp/probe.sock");
        assert_eq!(endpoint.path(), Path::new("/tmp/probe.sock"));
        assert_eq!(
            endpoint.to_string(),
            "/tmp/probe.sock (given by the test harness)"
        );
        assert!(
            endpoint.checked().is_empty(),
            "a given endpoint consulted no env var and must not claim to have",
        );
    }

    /// The context wrapper is what every wire failure is reported through, so it must not lose
    /// the kind a caller may match on.
    #[test]
    fn context_names_the_endpoint_and_keeps_the_error_kind() {
        let endpoint = HostEndpoint::given("the test harness", "/tmp/probe.sock");
        let wrapped = endpoint.context(&io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "Connection refused",
        ));
        assert_eq!(wrapped.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(
            wrapped.to_string(),
            "/tmp/probe.sock (given by the test harness): Connection refused",
        );
    }
}
