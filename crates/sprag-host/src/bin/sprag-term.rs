//! `sprag-term` — the headless terminal-multiplexer RPC server (GPU-free).
//!
//! Starts a workspace with one initial pane (a shell, or the command after
//! `--`) on a pseudoterminal and serves pinion's scene-as-data wire -- panes +
//! the `/sprag_mux` control surface + the `/sprag_plugins` platform -- over two
//! transports at once (DESIGN.md §1/§3): the process stdin/stdout (one
//! JSON-RPC request per line) AND an always-on Unix domain socket. The socket
//! is there no matter how the process was launched, so an AI peer reaches the
//! platform without wiring fd 0/1. Both transports funnel into one dispatch
//! owner, so they share a single consistent workspace view.
//!
//! ```text
//! sprag-term [--size COLSxROWS] [-- <program> [args...]]
//! ```
//!
//! With no command the initial pane runs `$SHELL` (else `/bin/sh`). Socket
//! policy: `$XDG_RUNTIME_DIR/sprag-host.sock` (override `SPRAG_HOST_RPC_SOCK`),
//! enabled unless `SPRAG_HOST_RPC` is falsey; `kill -USR1`/`-USR2` enable /
//! disable it live. As a server it runs until SIGINT/SIGTERM (which cancels +
//! joins in-flight plugin runs), not until stdin EOF.

use std::io;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use signal_hook::consts::{SIGINT, SIGTERM};
use sprag_host::{FrameIngress, Host, HostState, RunRegistry, dispatch_frames, stdin_frames};
use sprag_rpc::SocketOpts;
use sprag_terminal::CommandBuilder;

/// The headless host endpoint policy: `$XDG_RUNTIME_DIR/sprag-host.sock`
/// (override `SPRAG_HOST_RPC_SOCK`), enabled unless `SPRAG_HOST_RPC` is falsey.
const HOST_SOCKET: SocketOpts = SocketOpts {
    socket_name: "sprag-host.sock",
    path_env: "SPRAG_HOST_RPC_SOCK",
    enable_env: "SPRAG_HOST_RPC",
};

fn main() -> io::Result<()> {
    let (cols, rows, command, label) = parse_args();
    // The one Workspace owner (shared with the GUI as a code component): boot the
    // initial pane through it, then wrap it in HostState to serve the RPC surface.
    let host = Host::new((cols, rows));
    host.spawn(command, label, cols, rows, None)
        .map_err(io::Error::other)?;
    let state = HostState::new(host);

    // One dispatch owner (this thread) serialises all dispatch; the always-on
    // socket and stdin are producers of RpcFrames into it, so a socket client
    // and a stdin line share one consistent HostState view.
    let (tx, rx) = mpsc::channel();
    // The always-on Unix socket (execution-independent; SIGUSR1/2 controllable).
    sprag_rpc::mount(Arc::new(FrameIngress::new(tx.clone())), HOST_SOCKET);
    // Graceful shutdown: SIGINT/SIGTERM cancels + joins in-flight plugin runs.
    install_shutdown(Arc::clone(state.runs()));
    // stdin as an additional transport: ends on its own EOF, but the socket
    // keeps the server alive (a `/dev/null` stdin no longer terminates it).
    let stdin_tx = tx.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        stdin_frames(stdin.lock(), &stdin_tx);
    });
    // The socket's ingress holds senders in its accept threads, so `rx` stays
    // open for the process lifetime; drop this local sender so only the live
    // transports keep it open.
    drop(tx);
    dispatch_frames(&state, rx);
    Ok(())
}

/// Install SIGINT/SIGTERM graceful shutdown: on the first such signal, cancel
/// and join in-flight plugin runs (so a slow AI turn aborts and its worker
/// threads reap; the pane shells receive SIGHUP when our PTY masters close on
/// exit) then exit. Non-fatal if the handler cannot be installed -- the process
/// then just terminates on the signal, as default.
fn install_shutdown(runs: Arc<Mutex<RunRegistry>>) {
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) {
        Ok(signals) => signals,
        Err(_) => return,
    };
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            let mut runs = runs.lock().unwrap_or_else(PoisonError::into_inner);
            runs.cancel_all();
            runs.join_all();
            std::process::exit(0);
        }
    });
}

/// Parse `[--size COLSxROWS]` then an optional command (after `--`, or the
/// first bare argument). Falls back to `$SHELL` at 80x24.
fn parse_args() -> (u16, u16, CommandBuilder, String) {
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut args = std::env::args().skip(1);
    let mut command: Option<(CommandBuilder, String)> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(parse_size) {
                    cols = w;
                    rows = h;
                }
            }
            "--" => {
                if let Some(program) = args.next() {
                    command = Some(sprag_terminal::command_from_parts(program, &mut args));
                }
                break;
            }
            _ => {
                command = Some(sprag_terminal::command_from_parts(arg, &mut args));
                break;
            }
        }
    }

    let (command, label) = command.unwrap_or_else(sprag_terminal::default_shell_command);
    (cols, rows, command, label)
}

/// Parse a `COLSxROWS` size specifier.
fn parse_size(spec: &str) -> Option<(u16, u16)> {
    let (w, h) = spec.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
