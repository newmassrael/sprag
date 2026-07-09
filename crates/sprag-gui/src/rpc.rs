//! The GUI's RPC socket endpoint: its transport *policy* (socket name + env
//! vars) over the shared [`sprag_rpc`] mechanism.
//!
//! pinion's built-in RPC transport reads frames off the process's own stdin
//! and writes responses to stdout, so the endpoint only existed where the
//! parent wired fd 0/1 -- every session had to launch the GUI with stdin bound
//! to a live pipe. [`mount`] instead binds a fixed-path, always-on Unix socket
//! through pinion's winit-free `on_rpc_ingress` seam (PR-47): the endpoint is
//! there no matter how the process was launched, and the socket and pinion's
//! still-present stdin reader share one `RpcIngress` -> one dispatch core
//! (purely additive).
//!
//! The bind/boot/SIGUSR1-2-control/lifetime *mechanism* lives in [`sprag_rpc`]
//! (shared with the headless host); this module only names the GUI's policy:
//! socket `sprag-gui.sock`, path override `SPRAG_RPC_SOCK`, boot state
//! `SPRAG_RPC` (a falsey token boots it withdrawn). Wired in `main` through
//! [`pinion_shell::ShellConfig::on_rpc_ingress`].

use std::sync::Arc;

use pinion_shell::RpcIngress;
use sprag_rpc::SocketOpts;

/// The GUI endpoint's policy: `$XDG_RUNTIME_DIR/sprag-gui.sock` (override
/// `SPRAG_RPC_SOCK`), enabled unless `SPRAG_RPC` is falsey.
const GUI_SOCKET: SocketOpts = SocketOpts {
    socket_name: "sprag-gui.sock",
    path_env: "SPRAG_RPC_SOCK",
    enable_env: "SPRAG_RPC",
};

/// The `on_rpc_ingress` hook: mount the always-on GUI RPC socket onto the
/// shell's ingress (the same dispatch core the stdin reader drives). Non-fatal
/// on bind failure; the GUI and pinion's stdin RPC keep working without it.
pub fn mount(ingress: Arc<dyn RpcIngress>) {
    sprag_rpc::mount(ingress, GUI_SOCKET);
}
