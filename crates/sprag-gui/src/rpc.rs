//! §5.7 PR-47 — the always-on RPC socket endpoint (sprag-owned transport
//! policy over pinion's winit-free ingress seam).
//!
//! pinion's built-in RPC transport reads JSON-RPC frames off the process's
//! own stdin and writes responses to stdout, so the endpoint only exists
//! where the parent wired fd 0 / fd 1 — every session had to launch
//! sprag-gui with stdin bound to a live pipe (the manual FIFO dance), and it
//! could never be toggled at runtime (fd 0 is fixed at exec). This module
//! mounts a *fixed-path Unix domain socket* instead
//! ([`pinion_rpc_transport::UnixSocketTransport`], pinion R1263/PR-47): the
//! endpoint is always there no matter how the process was launched, and the
//! same dispatch core serves it — the socket and pinion's still-present
//! stdin reader share one [`RpcIngress`], so both drive the identical RPC
//! method vocabulary (`scene/snapshot`, `scene/type`, ...).
//!
//! **The transport policy is sprag's** — PR-47's third layer. pinion owns
//! the dispatch core (unchanged) and provides the socket *mechanism*
//! (reusable, seam-only); sprag decides *where* the endpoint lives, *whether*
//! it is exposed, and *when*. Runtime on/off is therefore not a framework
//! mechanism but a consequence of sprag holding the [`TransportControl`]:
//!
//! - **Path** — `$SPRAG_RPC_SOCK` if set (explicit override, e.g. tests),
//!   else `$XDG_RUNTIME_DIR/sprag-gui.sock` (the per-user runtime dir, the
//!   FHS-correct home for a control socket), else `$TMPDIR/sprag-gui.sock`.
//! - **Boot state** — enabled unless `SPRAG_RPC` is a falsey token
//!   (`0` / `off` / `false` / `no`). The default is "always open" — the
//!   whole point of the endpoint.
//! - **Runtime control** — `kill -USR1 <pid>` enables the endpoint,
//!   `kill -USR2 <pid>` disables it (a dedicated [`signal_hook`] thread sets
//!   [`TransportControl::set_enabled`] to the level). Level, not a blind
//!   flip: the outcome is deterministic regardless of the current state, and
//!   a duplicated or lost signal cannot invert the intent. While off the
//!   socket stays bound but refuses new connections, so an agent path can be
//!   withdrawn or re-exposed live.
//!
//! Mounting is wired in `main` through [`pinion_shell::ShellConfig::on_rpc_ingress`];
//! [`mount`] is the hook. A bind failure is logged and left non-fatal — the
//! GUI (and pinion's stdin RPC path) keep working without the socket.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use pinion_rpc_transport::{TransportControl, UnixSocketTransport};
use pinion_shell::RpcIngress;

/// The socket filename under the resolved directory.
const SOCKET_NAME: &str = "sprag-gui.sock";

/// Process-lifetime **sole owner** of the live endpoint's [`TransportControl`].
/// It keeps the socket bound for the whole process (dropping the control at
/// the end of the [`mount`] hook would unbind it immediately) AND it is where
/// the signal thread reads the control to switch exposure — one owner serves
/// both, so no `Arc` is needed and endpoint liveness does not depend on the
/// control thread having spawned. Never dropped, so pinion's graceful
/// `shutdown`/`remove_file` never runs on a clean exit; the stale socket file
/// is reclaimed by the next `serve` (it removes the path before binding).
static ENDPOINT: OnceLock<TransportControl> = OnceLock::new();

/// Mount the always-on RPC socket. The `on_rpc_ingress` hook: binds the
/// fixed-path Unix socket, feeds accepted frames into the shared `ingress`
/// (same dispatch core as the stdin reader), applies the boot on/off policy,
/// and installs the SIGUSR1 runtime toggle. Runs once on the main thread
/// before the event loop starts; the transport owns its own threads.
///
/// A bind failure is non-fatal: logged at `warn`, the endpoint is simply
/// absent and the rest of the shell (including pinion's stdin RPC) is
/// unaffected.
pub fn mount(ingress: Arc<dyn RpcIngress>) {
    let path = socket_path();
    let control = match UnixSocketTransport::serve(&path, ingress) {
        Ok(control) => control,
        Err(error) => {
            tracing::warn!(
                target: "sprag_gui::rpc",
                path = %path.display(),
                %error,
                "RPC socket bind failed; endpoint unavailable (stdin RPC unaffected)"
            );
            return;
        }
    };
    control.set_enabled(boot_enabled());
    let enabled = control.is_enabled();
    // Hand the control to its process-lifetime owner BEFORE installing the
    // signal thread, so the socket is bound-and-owned even if the thread
    // never spawns. `set` only fails if the once-only hook ran twice (it does
    // not); on that impossible path the fresh control drops and unbinds while
    // the first stays owned.
    let _ = ENDPOINT.set(control);
    install_control_signals();
    tracing::info!(
        target: "sprag_gui::rpc",
        path = %path.display(),
        enabled,
        "RPC socket mounted (SIGUSR1 enables, SIGUSR2 disables)"
    );
}

/// Install the runtime on/off control: a dedicated thread parks on SIGUSR1
/// (enable) and SIGUSR2 (disable) and sets [`TransportControl::set_enabled`]
/// to the corresponding LEVEL, read from the process-lifetime [`ENDPOINT`].
/// Level rather than a blind flip is idempotent and assertable — `kill -USR2`
/// always withdraws the endpoint no matter its current state, and a duplicated
/// or lost signal cannot invert the intent. A dedicated `Signals::forever`
/// thread (not an async-signal handler) does the work on a normal stack, so
/// there is no async-signal-safety constraint and no `unsafe`. Non-fatal on
/// failure: the endpoint stays at its boot state, just not runtime-controllable.
fn install_control_signals() {
    use signal_hook::consts::{SIGUSR1, SIGUSR2};
    let mut signals = match signal_hook::iterator::Signals::new([SIGUSR1, SIGUSR2]) {
        Ok(signals) => signals,
        Err(error) => {
            tracing::warn!(
                target: "sprag_gui::rpc",
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
                tracing::info!(target: "sprag_gui::rpc", enabled = enable, "RPC socket set by signal");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(
            target: "sprag_gui::rpc",
            %error,
            "RPC control thread not spawned; endpoint fixed at its boot state"
        );
    }
}

/// The fixed endpoint path from the environment (precedence: explicit
/// override -> per-user runtime dir -> temp dir).
fn socket_path() -> PathBuf {
    resolve_socket_path(
        std::env::var_os("SPRAG_RPC_SOCK"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

/// Pure path resolution (env values passed in so it is testable): an
/// explicit `SPRAG_RPC_SOCK` wins as a full path; otherwise the socket sits
/// under `XDG_RUNTIME_DIR`; with neither, under the temp dir. A fixed,
/// execution-independent location is the point — the endpoint is found the
/// same way regardless of how the process was launched.
fn resolve_socket_path(explicit: Option<OsString>, xdg_runtime: Option<OsString>) -> PathBuf {
    if let Some(explicit) = explicit {
        return PathBuf::from(explicit);
    }
    if let Some(dir) = xdg_runtime {
        return PathBuf::from(dir).join(SOCKET_NAME);
    }
    std::env::temp_dir().join(SOCKET_NAME)
}

/// Whether the endpoint serves connections at boot.
fn boot_enabled() -> bool {
    parse_boot_enabled(std::env::var("SPRAG_RPC").ok().as_deref())
}

/// Pure boot-state policy (value passed in so it is testable): default on
/// ("always open"); a falsey `SPRAG_RPC` token boots the endpoint withdrawn.
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
        );
        assert_eq!(path, PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn xdg_runtime_dir_hosts_the_socket_without_override() {
        let path = resolve_socket_path(None, Some(OsString::from("/run/user/1000")));
        assert_eq!(path, PathBuf::from("/run/user/1000/sprag-gui.sock"));
    }

    #[test]
    fn temp_dir_fallback_when_no_xdg_runtime() {
        let path = resolve_socket_path(None, None);
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
