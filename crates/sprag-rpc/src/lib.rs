//! sprag's RPC transport **policy**: a fixed-path, always-on Unix-domain
//! socket endpoint over pinion's winit-free [`RpcIngress`] seam (PR-47), with
//! a SIGUSR1/SIGUSR2 runtime on/off control.
//!
//! PR-47's three layers are: the dispatch core (pinion, transport-agnostic),
//! the transport *mechanism* (pinion's reusable
//! [`UnixSocketTransport`]), and
//! the transport *policy* -- when/where/whether the endpoint is exposed. This
//! crate is that third layer, and it is **frontend-agnostic**: the windowed
//! GUI and the headless host both mount their socket through [`mount`],
//! differing only in the socket name and the env-var names ([`SocketOpts`]).
//! Path resolution, boot state, the runtime control, and the process-lifetime
//! ownership therefore live here once, not copied per frontend.
//!
//! The caller supplies the `Arc<dyn RpcIngress>` -- the GUI's comes from
//! pinion-shell's `on_rpc_ingress` hook; the headless host's is a channel
//! funnel into its single dispatch owner -- and this mounts the socket onto
//! it. GPU-free: the only deps are the seam and the socket adapter.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use pinion_rpc::RpcIngress;
use pinion_rpc_transport::{Exposure, TransportControl, UnixSocketTransport};

pub mod call;
pub mod client;
pub mod endpoint;
pub mod grammar;
pub use call::{
    FillError, Flag, GrammarError, PublishedArg, PublishedForm, build_call, read_surface,
    selector_of,
};
pub use client::{
    ACTION_REFUSED, ATTACHED_PARAM, BUILD, BUILD_FIELD, CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD,
    CLIENT_MESSAGES_METHOD, CLIENT_PARAM, CLIENT_SIZE_METHOD, COLS_PARAM, CallError,
    EVENTS_CHANGED_METHOD, EVENTS_SUBSCRIBE_METHOD, EVENTS_UNSUBSCRIBE_METHOD, EVENTS_WAIT_METHOD,
    HOST_SILENT, HostConn, INVALID_PARAMS, MESSAGE_FIELD, NEEDLE_PARAM, PANE_PARAM,
    PANE_WAIT_OUTPUT_METHOD, PATTERN_PARAM, PROTOCOL_FIELD, PROTOCOL_PARAM, ROWS_PARAM, RpcFault,
    SESSION_PARAM, SINCE_PARAM, SUBSCRIPTION_PARAM, ScopeAsk, ScopeFault, UNKNOWN_ACTION_FAULT,
    UNKNOWN_SLOT_FAULT, UNSTATED_REFUSAL_FAULT, WINDOW_PARAM, WIRE_PROTOCOL, gui_client_prefix,
    new_gui_client_id, request_label,
};
pub use endpoint::{CLIENT_SOCKET_ENV, CLIENT_SOCKET_ENVS, EndpointOrigin, HostEndpoint};
pub use grammar::{ActionGrammar, ArgGrammar, CallForm, FormKind, WireSurface};

/// The well-known host socket file name — the endpoint a `sprag-term` daemon binds and a
/// `sprag-gui` client connect-or-spawns on. Defined here, in the shared transport crate, so
/// the daemon's default ([`SocketOpts::socket_name`]) and the GUI's default (its
/// `host_socket()`) name ONE fact once, the way both ends of a wire must (mirroring
/// [`SESSION_PARAM`]). A per-instance override still travels by env var; this is only the
/// default both sides fall back to.
pub const HOST_SOCKET_NAME: &str = "sprag-host.sock";

/// The daemon's EXECUTABLE name — the same kind of fact as [`HOST_SOCKET_NAME`] one layer over, and
/// here for the same reason: the processes that LAUNCH a daemon and the ones that RECOGNISE one must
/// name it once.
///
/// # ⚠⚠⚠ It was spelled seven times before this existed
///
/// `sprag-gui`'s connect-or-spawn, the CLI's own spawn, two latency harnesses and a host test each
/// wrote the string out. That is the shape this workspace records the cost of — *two copies of one
/// rule, and the failure of letting them drift is silent* — and it had already grown to seven.
///
/// ⚠⚠ WHAT MAKES IT MORE THAN TIDINESS: `kill-server` RECOGNISES a daemon by this name before it
/// signals it (a socket may be served by something that is not a daemon, and the first version of
/// that check SIGTERMed a test harness). A rename that reached the launchers and missed the
/// recogniser would leave the product unable to stop its own daemon, and it would say so with a
/// sentence about a process that is not one.
pub const DAEMON_BIN_NAME: &str = "sprag-term";

/// The headless host's endpoint POLICY: the well-known [`HOST_SOCKET_NAME`] socket, overridable
/// by `SPRAG_HOST_RPC_SOCK`, enabled unless `SPRAG_HOST_RPC` is falsey. Defined here so the
/// daemon that BINDS it (`sprag-term`) and every client that DRIVES it over the same env vars
/// (the `sprag` CLI) share ONE `SocketOpts`, not a re-spelled copy each. (`sprag-gui` uses its
/// own override env `SPRAG_GUI_HOST_SOCK`, so it resolves the path itself rather than this
/// policy.)
pub const HOST_SOCKET: SocketOpts = SocketOpts {
    socket_name: HOST_SOCKET_NAME,
    path_env: "SPRAG_HOST_RPC_SOCK",
    enable_env: "SPRAG_HOST_RPC",
};

/// A frontend's socket policy: the endpoint's default file name plus the env
/// vars that override its path and its boot state. The *mechanism* (bind,
/// control, lifetime) is shared; only these names differ per frontend, so the
/// GUI and the host cannot collide on one socket path by default.
#[derive(Clone, Copy)]
pub struct SocketOpts {
    /// Default socket file name under the runtime dir (e.g. `sprag-gui.sock`).
    pub socket_name: &'static str,
    /// Env var giving an explicit full socket path (overrides the default).
    pub path_env: &'static str,
    /// Env var whose falsey token (`0`/`off`/`false`/`no`) boots the endpoint
    /// withdrawn; absent or any other value boots it enabled ("always open").
    pub enable_env: &'static str,
}

/// Process-lifetime **sole owner** of the endpoint's [`TransportControl`]. It
/// keeps the socket bound for the whole process AND is where the control
/// thread reads the control to switch exposure -- one owner serves both, so no
/// `Arc` is needed and liveness does not depend on the control thread having
/// spawned. Never dropped; a stale socket file is reclaimed by the next
/// `serve`. One endpoint per process.
///
/// The reclaim is CONDITIONAL as of pinion R1478: `serve` no longer unlinks the
/// path unconditionally, it `connect`s on `EADDRINUSE` and reclaims only a path
/// nobody answers on -- a LIVE endpoint is refused rather than displaced. That
/// refusal is unreachable for the daemon, which takes its single-instance
/// `flock` before it ever mounts and exits quietly when it loses (see
/// `sprag-term.rs`), so the two guards agree: the flock decides who serves, and
/// the bind can no longer overrule it.
static ENDPOINT: OnceLock<TransportControl> = OnceLock::new();

/// Mount the always-on RPC socket for `ingress` under `opts`. Binds the
/// fixed-path Unix socket AT its boot exposure, installs the
/// SIGUSR1(enable)/SIGUSR2(disable) runtime control, and hands the control to
/// its process-lifetime owner.
///
/// The boot policy is declared to the BIND ([`Exposure`], pinion R1469
/// delivering PINION-PR48) rather than applied after it. The difference is not
/// cosmetic: `serve` armed the accept loop already serving, so between it and a
/// post-bind withdraw there was a window, and a client landing in it was not
/// exposed for that instant -- it was accepted WHILE SERVING, and withdrawing
/// deliberately leaves in-flight connections alone, so it stayed served for its
/// whole session by an endpoint the operator asked to be withdrawn.
///
/// The guarantee is STRUCTURAL, and saying so is the honest report: racing a
/// client against the bind could not witness the window from out here -- 30
/// trials refused 30/30 with the old post-bind withdraw and 30/30 with this,
/// on a harness proven able to see the difference (the same race against a
/// boot-SERVING endpoint reports 30/30 served). So this bought correctness by
/// construction plus a call site that finally says what it means, not a
/// reproduced bug fixed. Do not go looking for a failing case to pin it with;
/// there is no external witness to find.
///
/// Non-fatal on bind failure (logged at `warn`; the caller's other transports,
/// e.g. stdin, are unaffected). Call once per process.
pub fn mount(ingress: Arc<dyn RpcIngress>, opts: SocketOpts) {
    // The endpoint, not a bare path: what this process BINDS is reported with the same
    // provenance a client reports what it CONNECTS to, so the two ends of one wire are read the
    // same way in one log (`endpoint.rs`).
    let endpoint = HostEndpoint::for_opts(opts);
    let exposure = Exposure::from_serving(boot_enabled(opts));
    let control = match UnixSocketTransport::serve_with_exposure(endpoint.path(), ingress, exposure)
    {
        Ok(control) => control,
        Err(error) => {
            tracing::warn!(
                target: "sprag_rpc",
                socket = opts.socket_name,
                endpoint = %endpoint,
                %error,
                "RPC socket bind failed; endpoint unavailable"
            );
            return;
        }
    };
    let enabled = control.is_enabled();
    // Owner set BEFORE the control thread, so the socket is bound-and-owned
    // even if the thread never spawns. `set` only fails on an impossible
    // second mount in one process; the fresh control then drops and unbinds
    // while the first stays owned.
    let _ = ENDPOINT.set(control);
    install_control_signals(opts.socket_name);
    tracing::info!(
        target: "sprag_rpc",
        socket = opts.socket_name,
        endpoint = %endpoint,
        enabled,
        "RPC socket mounted (SIGUSR1 enables, SIGUSR2 disables)"
    );
}

/// Install the runtime on/off control: a dedicated thread parks on SIGUSR1
/// (enable) and SIGUSR2 (disable) and sets [`TransportControl::set_enabled`]
/// to the corresponding LEVEL, read from the process-lifetime [`ENDPOINT`].
/// Level rather than a blind flip is idempotent and assertable -- `kill -USR2`
/// always withdraws the endpoint no matter its current state. A dedicated
/// `Signals::forever` thread (not an async-signal handler) does the work on a
/// normal stack, so there is no async-signal-safety constraint and no
/// `unsafe`. Non-fatal on failure: the endpoint stays at its boot state, just
/// not runtime-controllable.
fn install_control_signals(socket_name: &'static str) {
    use signal_hook::consts::{SIGUSR1, SIGUSR2};
    let mut signals = match signal_hook::iterator::Signals::new([SIGUSR1, SIGUSR2]) {
        Ok(signals) => signals,
        Err(error) => {
            tracing::warn!(
                target: "sprag_rpc",
                socket = socket_name,
                %error,
                "SIGUSR1/2 handler unavailable; RPC endpoint fixed at its boot state"
            );
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("sprag-rpc-control".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                let Some(control) = ENDPOINT.get() else {
                    continue;
                };
                let enable = signal == SIGUSR1;
                control.set_enabled(enable);
                tracing::info!(target: "sprag_rpc", socket = socket_name, enabled = enable, "RPC socket set by signal");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(
            target: "sprag_rpc",
            socket = socket_name,
            %error,
            "RPC control thread not spawned; endpoint fixed at its boot state"
        );
    }
}

/// The fixed endpoint path from the environment for `opts` (precedence:
/// explicit override -> per-user runtime dir -> temp dir) — the SAME resolution [`mount`]
/// uses, exposed so a daemon can derive per-endpoint sidecar files (its single-instance lock,
/// its log) from the ONE path that identifies it, rather than from a second, independently
/// spelled default that an override could desynchronize.
///
/// The PATH alone, for the callers whose subject is a file name derived from it. A caller that
/// will REPORT the endpoint wants [`HostEndpoint::for_opts`] instead, which keeps the provenance
/// this drops.
#[must_use]
pub fn socket_path(opts: SocketOpts) -> PathBuf {
    HostEndpoint::for_opts(opts).into_path()
}

/// A runtime-directory path for `file_name` (`$XDG_RUNTIME_DIR/<file_name>`, else
/// the temp dir) — the SSOT for where a frontend places a runtime file such as the
/// well-known host socket a GUI connect-or-spawns a daemon on. The same
/// resolution [`mount`] uses for its endpoint, so a spawner and the socket policy
/// agree on the directory.
#[must_use]
pub fn runtime_path(file_name: &str) -> PathBuf {
    resolve_socket_path(None, std::env::var_os("XDG_RUNTIME_DIR"), file_name)
}

/// Pure path resolution (env values injected so it is testable): an explicit
/// path override wins as a full path; otherwise the socket sits under
/// `XDG_RUNTIME_DIR`; with neither, under the temp dir. A fixed,
/// execution-independent location is the point -- the endpoint is found the
/// same way regardless of how the process was launched.
///
/// `pub(crate)` for [`endpoint`], which layers provenance over this one resolution rather than
/// spelling a second copy of the runtime-dir rule.
pub(crate) fn resolve_socket_path(
    explicit: Option<OsString>,
    xdg_runtime: Option<OsString>,
    socket_name: &str,
) -> PathBuf {
    if let Some(explicit) = explicit {
        return PathBuf::from(explicit);
    }
    if let Some(dir) = xdg_runtime {
        return PathBuf::from(dir).join(socket_name);
    }
    std::env::temp_dir().join(socket_name)
}

/// Whether the endpoint serves connections at boot, from `opts.enable_env`.
fn boot_enabled(opts: SocketOpts) -> bool {
    parse_boot_enabled(std::env::var(opts.enable_env).ok().as_deref())
}

/// Pure boot-state policy (value injected so it is testable): default on
/// ("always open"); a falsey token boots the endpoint withdrawn.
fn parse_boot_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_override_wins_as_full_path() {
        let path = resolve_socket_path(
            Some(OsString::from("/tmp/custom.sock")),
            Some(OsString::from("/run/user/1000")),
            "sprag-gui.sock",
        );
        assert_eq!(path, PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn xdg_runtime_dir_hosts_the_named_socket_without_override() {
        let path = resolve_socket_path(
            None,
            Some(OsString::from("/run/user/1000")),
            "sprag-host.sock",
        );
        assert_eq!(path, PathBuf::from("/run/user/1000/sprag-host.sock"));
    }

    #[test]
    fn temp_dir_fallback_when_no_xdg_runtime() {
        let path = resolve_socket_path(None, None, "sprag-gui.sock");
        assert_eq!(path, std::env::temp_dir().join("sprag-gui.sock"));
    }

    #[test]
    fn boot_enabled_defaults_on() {
        // Absent, empty, and any non-falsey token all mean "always open".
        assert!(parse_boot_enabled(None));
        assert!(parse_boot_enabled(Some("1")));
        assert!(parse_boot_enabled(Some("on")));
        assert!(parse_boot_enabled(Some("true")));
        assert!(parse_boot_enabled(Some("anything-else")));
    }

    #[test]
    fn boot_withdrawn_by_falsey_tokens_case_and_space_insensitive() {
        assert!(!parse_boot_enabled(Some("0")));
        assert!(!parse_boot_enabled(Some("off")));
        assert!(!parse_boot_enabled(Some("false")));
        assert!(!parse_boot_enabled(Some("no")));
        assert!(!parse_boot_enabled(Some("  OFF  ")));
        assert!(!parse_boot_enabled(Some("No")));
    }
}
