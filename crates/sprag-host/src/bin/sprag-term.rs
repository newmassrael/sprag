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
//! disable it live. As a server it runs until SIGINT/SIGTERM OR until its LAST
//! live pane exits — the self-cleaning tmux convention (a host with nothing left
//! to serve ends). Both edges funnel through ONE shutdown routine that cancels +
//! joins in-flight plugin runs (the last-pane edge raises SIGTERM into it), so
//! neither abandons a run. Not until stdin EOF.

use std::io;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use pinion_core::SceneRevision;
use signal_hook::consts::{SIGINT, SIGTERM};
use sprag_host::{
    FrameIngress, Host, HostState, RunRegistry, bump_on_dirty, dispatch_frames, pane_exit_hook,
    spawn_reaper, stdin_frames,
};
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
    //
    // The initial pane's `on_dirty` bumps the shared scene-version token, so its
    // output wakes any parked async `scene/waitFor` (the change-notification a wire
    // client long-polls instead of busy-polling snapshots). The revision is created
    // BEFORE the spawn so the bumper and HostState share the one token.
    let revision = Arc::new(SceneRevision::new());
    let host = Host::new((cols, rows));
    // Self-cleaning lifetime: when the LAST live pane across all sessions exits, the daemon has
    // nothing left to serve, so it ends — the tmux convention. `spawn_reaper` owns a dedicated
    // thread that runs the liveness scan OFF the PTY reader threads (so a pane Drop that joins
    // a reader can never deadlock the scan) and returns the registry-free death-signal every
    // pane's `on_exit` feeds. The exit action is INJECTED here (the library names neither exit
    // nor SIGTERM): it raises SIGTERM, so BOTH shutdown edges (an operator's Ctrl-C and the
    // last pane dying) funnel through the ONE `install_shutdown` routine that cancels + joins
    // in-flight plugin runs. A daemon with no panes has no hook and cannot exit before its
    // first pane; raising before the handler is installed (an instant-exit boot command) falls
    // back to SIGTERM's default terminate, harmless since no run is in flight before boot.
    let on_pane_exit = spawn_reaper(
        Arc::clone(host.registry()),
        Arc::new(|| {
            let _ = signal_hook::low_level::raise(SIGTERM);
        }),
    );
    host.spawn(
        command,
        label,
        cols,
        rows,
        Some(bump_on_dirty(&revision)),
        Some(pane_exit_hook(&on_pane_exit)),
    )
    .map_err(io::Error::other)?;
    let state = HostState::new(host, revision, Some(on_pane_exit));

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
