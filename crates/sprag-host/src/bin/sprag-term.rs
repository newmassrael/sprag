//! `sprag-term` — the headless terminal-multiplexer RPC server (GPU-free).
//!
//! Starts a workspace — with one initial pane (a shell, or the command after
//! `--`) on a pseudoterminal, or, as a `--daemon`, RESTORED from its last durability
//! snapshot (else empty; see below) — and serves
//! pinion's scene-as-data wire -- panes +
//! the `/sprag_mux` control surface + the `/sprag_plugins` platform -- over two
//! transports at once (DESIGN.md §1/§3): the process stdin/stdout (one
//! JSON-RPC request per line) AND an always-on Unix domain socket. The socket
//! is there no matter how the process was launched, so an AI peer reaches the
//! platform without wiring fd 0/1. Both transports funnel into one dispatch
//! owner, so they share a single consistent workspace view.
//!
//! ```text
//! sprag-term [--daemon] [--size COLSxROWS] [-- <program> [args...]]
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
//!
//! ## `--daemon`
//!
//! `--daemon` boots the long-lived multiplexer a GUI connect-or-spawns: it self-daemonizes
//! (fork, the parent exits so the spawner reaps a short-lived intermediate and the real
//! daemon reparents to init; `setsid` drops the controlling terminal), redirects stdio to the
//! endpoint's log (`<socket>.log`), and holds a single-instance advisory `flock` on
//! `<socket>.lock` so a race to spawn one leaves exactly one alive. The lock and log derive
//! from the endpoint path, so an overridden socket keeps its own pair. It never boots a STRAY
//! pane — every pane belongs to a client's session, and one nobody attached to would be unseen
//! and would pin the self-cleaning count above zero forever. Instead it RESTORES its durability
//! snapshot if one survived a reboot (the `durability` ring — sessions, windows, layout and pane
//! working directories rebuilt, each pane re-running its recorded command — an allowlisted program
//! — or a shell in its cwd), else boots empty. A natural last-pane exit KEEPS the snapshot (so a
//! transient program exit retries next boot). The daemon lifecycle otherwise PRESERVES the
//! snapshot — a reboot, a crash, a natural close, and a plain `kill-server` all leave it, so the
//! workspace comes back; only `sprag kill-server --purge` destroys the saved workspace (CLI-side).
//! Standalone mode (no `--daemon`) never persists and is unchanged.

// A binary crate: `cargo doc` builds it with private items and its crate-root doc links to the
// bin's own internals. `private_intra_doc_links` guards LIBRARY public-API docs (which publish
// without private items); a bin has no such surface, so the lint is a structural false positive
// here (mirrors `sprag-gui`) — declared crate-wide so a future internal link cannot re-break it.
#![allow(rustdoc::private_intra_doc_links)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use pinion_core::SceneRevision;
use signal_hook::consts::{SIGINT, SIGTERM};
use sprag_host::{
    FrameIngress, Host, HostState, RunRegistry, bump_on_dirty, dispatch_frames, load_snapshot,
    pane_exit_hook, save_if_changed, snapshot_path, spawn_reaper, stdin_frames,
};
use sprag_rpc::HOST_SOCKET;
use sprag_terminal::{CommandBuilder, SessionRegistry, Snapshot};
use tracing_subscriber::{EnvFilter, fmt};

fn main() -> io::Result<()> {
    let args = parse_args();

    // The endpoint path resolved ONCE, the same way `mount` resolves it — the daemon's single
    // identity. Its lock and log are DERIVED from it (`<socket>.lock` / `<socket>.log`), so an
    // override (`SPRAG_HOST_RPC_SOCK`) keeps all three in step: two daemons on two sockets get
    // two locks (tmux's per-socket-server model), and a spawned daemon's lock always matches the
    // socket its spawner will connect to. For the default socket these derive to the same
    // `sprag-host.lock` / `sprag-host.log` as a fixed name would.
    let sock = sprag_rpc::socket_path(HOST_SOCKET);

    // A daemon self-daemonizes as the FIRST act of `main`, before any thread exists
    // (fork duplicates only the calling thread, so a fork after `spawn_reaper`/`mount` would
    // leave the child missing them mid-lock), then holds a single-instance lock for the whole
    // run. If another daemon already owns it, exit quietly — its socket is the one to use.
    // Standalone mode is untouched: no fork, stdio kept, one boot pane.
    let _instance = if args.daemon {
        daemonize(&sock)?;
        match acquire_single_instance(&sock)? {
            Some(lock) => Some(lock),
            None => return Ok(()),
        }
    } else {
        None
    };

    // The daemon's tracing subscriber, installed AFTER daemonize so its output lands in the
    // endpoint's log (stderr is redirected there). Quiet by default (`warn`), with the durability
    // ring at `info` so an operator sees "restored N panes" without opting in; override via
    // `SPRAG_LOG` (RUST_LOG syntax). Stderr, never stdout — stdout carries the stdin/stdout wire.
    init_tracing();

    // The one Workspace owner (shared with the GUI as a code component): boot the
    // initial pane through it, then wrap it in HostState to serve the RPC surface.
    //
    // The initial pane's `on_dirty` bumps the shared scene-version token, so its
    // output wakes any parked async `scene/waitFor` (the change-notification a wire
    // client long-polls instead of busy-polling snapshots). The revision is created
    // BEFORE the spawn so the bumper and HostState share the one token.
    let revision = Arc::new(SceneRevision::new());
    let host = Host::new((args.cols, args.rows));
    // The persistent snapshot path, used only in the daemon arms below.
    let snap_path = snapshot_path(&sock);
    // Self-cleaning lifetime: when the LAST live pane across all sessions exits, the daemon has
    // nothing left to serve, so it ends — the tmux convention. `spawn_reaper` owns a dedicated
    // thread that runs the liveness scan OFF the PTY reader threads (so a pane Drop that joins
    // a reader can never deadlock the scan) and returns the registry-free death-signal every
    // pane's `on_exit` feeds. The exit action is INJECTED here (the library names neither exit
    // nor SIGTERM): it raises SIGTERM, so BOTH shutdown edges (an operator's Ctrl-C and the
    // last pane dying) funnel through the ONE `install_shutdown` routine that cancels + joins
    // in-flight plugin runs.
    //
    // The reaper does NOT invalidate the snapshot. A last-pane exit is AMBIGUOUS — it may be a
    // deliberate close OR a re-run program that just exited (a restored `ssh` whose network was
    // not up yet at boot). Deleting on it would let a TRANSIENT exit destroy the very session the
    // ring exists to preserve. So the daemon lifecycle PRESERVES the snapshot (the next daemon
    // restores it — the cmux-durable model); only an explicit `sprag kill-server --purge` destroys
    // the saved workspace (CLI-side). The reaper also does NOT exit
    // while a restore is IN FLIGHT (`restoring`): a re-run program that exits fast must not kill
    // the daemon mid-loop, before the rest of the session is re-spawned.
    let restoring = Arc::new(AtomicBool::new(false));
    let reaper_restoring = Arc::clone(&restoring);
    let on_pane_exit = spawn_reaper(
        Arc::clone(host.registry()),
        Arc::new(move || {
            if reaper_restoring.load(Ordering::Acquire) {
                return; // a restore is spawning panes — a mid-loop exit must not end the daemon
            }
            let _ = signal_hook::low_level::raise(SIGTERM);
        }),
    );
    // A daemon RESTORES its last durable snapshot if one survived the reboot, else boots EMPTY:
    // every pane belongs to a client's session, so an un-restored boot pane would be a shell nobody
    // sees AND would pin the self-cleaning live count above zero for the daemon's life. Restore
    // re-runs each recorded pane's command (an allowlisted program) or a shell in its cwd, with the
    // SAME reaper/repaint hooks a boot pane gets (the D4 birth-at-host seam). A corrupt or absent
    // snapshot leaves the registry empty — the pre-durability behaviour. Standalone still boots its
    // one pane (the `sprag-term -- cmd` contract + the `wire_client` tests rely on it) and never
    // persists.
    if args.daemon {
        if let Some(snapshot) = load_snapshot(&snap_path) {
            // Gate the reaper across the restore loop, then run it with the exact-command allowlist
            // (read once from the environment here, injected so the host does not touch it).
            restoring.store(true, Ordering::Release);
            let outcome = host.restore(
                snapshot,
                &sprag_host::restore_allowlist(),
                || Some(bump_on_dirty(&revision)),
                || Some(pane_exit_hook(&on_pane_exit)),
            );
            restoring.store(false, Ordering::Release);
            match outcome {
                Ok(n) => tracing::info!(
                    target: "sprag_host::durability",
                    "restored {n} pane(s) from {}",
                    snap_path.display()
                ),
                Err(e) => tracing::warn!(
                    target: "sprag_host::durability",
                    "snapshot at {} is unusable ({e}); booting empty",
                    snap_path.display()
                ),
            }
        }
        // The durability save loop: persist the live shape so the NEXT daemon can rebuild it.
        spawn_snapshot_saver(Arc::clone(host.registry()), snap_path);
    } else {
        host.spawn(
            args.command,
            args.label,
            args.cols,
            args.rows,
            Some(bump_on_dirty(&revision)),
            Some(pane_exit_hook(&on_pane_exit)),
        )
        .map_err(io::Error::other)?;
    }
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

/// Install the daemon's global `tracing` subscriber — the same env-filtered, stderr, first-wins
/// (`try_init`) shape the GUI uses, so the library's events (the durability ring, and anything the
/// host emits) reach the daemon's log without ad-hoc `eprintln`.
///
/// Default filter `warn,sprag_host::durability=info`: quiet everywhere, except the durability ring
/// narrates its restore/save at `info` so an operator sees the reboot payoff by default. Override
/// with `SPRAG_LOG` (RUST_LOG syntax, e.g. `SPRAG_LOG=sprag_host=debug`). Stderr only — stdout
/// carries the JSON-RPC wire.
fn init_tracing() {
    let filter = EnvFilter::try_from_env("SPRAG_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,sprag_host::durability=info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_timer(fmt::time::uptime())
        .try_init();
}

/// How often the daemon re-snapshots its live shape to disk — the loss window a hard reboot can
/// cost. Small enough that a layout change or a `cd` is captured promptly, large enough that the
/// background snapshot is negligible; a save only WRITES when the shape changed, so an idle daemon
/// does no disk I/O.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

/// Spawn the durability save loop (daemon only): every [`SNAPSHOT_INTERVAL`], project the registry
/// to a [`Snapshot`] and write it ATOMICALLY — but only when it differs from the last one saved, so
/// an idle daemon rewrites nothing. This is the cmux-parity ring: a reboot ends the daemon and every
/// PTY, but the snapshot on disk lets the NEXT daemon rebuild the sessions, windows, layout and
/// working directories a live PTY could never carry across.
///
/// The `snapshot` projection takes the registry then each workspace lock SEQUENTIALLY (never
/// nested), so this background thread never contends with dispatch beyond a brief membership read.
/// A transient save error is logged and retried next tick (`last` is left unchanged), so a full disk
/// or a momentary permission glitch does not silently stop persistence.
fn spawn_snapshot_saver(registry: Arc<Mutex<SessionRegistry>>, path: PathBuf) {
    thread::spawn(move || {
        // `save_if_changed` owns the write-if-changed dedup (tested in `durability`); the loop is
        // just it on a timer, carrying the last-saved snapshot between ticks.
        let mut last: Option<Snapshot> = None;
        loop {
            thread::sleep(SNAPSHOT_INTERVAL);
            if let Err(e) = save_if_changed(&path, &registry, &mut last) {
                tracing::warn!(
                    target: "sprag_host::durability",
                    "snapshot save to {} failed: {e}",
                    path.display()
                );
            }
        }
    });
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

/// The parsed command line.
struct BootArgs {
    cols: u16,
    rows: u16,
    /// The initial pane's command + its display label — used only in standalone mode; a
    /// daemon boots no pane and never reads them.
    command: CommandBuilder,
    label: String,
    /// `--daemon`: self-daemonize, boot empty, single-instance (see the module docs).
    daemon: bool,
}

/// Parse `[--daemon]` and `[--size COLSxROWS]` then an optional command (after `--`, or the
/// first bare argument). Falls back to `$SHELL` at 80x24.
fn parse_args() -> BootArgs {
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut daemon = false;
    let mut args = std::env::args().skip(1);
    let mut command: Option<(CommandBuilder, String)> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--daemon" => daemon = true,
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
    BootArgs {
        cols,
        rows,
        command,
        label,
        daemon,
    }
}

/// Self-daemonize: `fork`, the PARENT exits (so a spawner reaps a short-lived intermediate and
/// the real daemon reparents to init — no `PR_SET_PDEATHSIG`, no zombie), then the CHILD
/// starts a new session (`setsid`, dropping any controlling terminal) and redirects its stdio
/// to `sock`'s log (`<socket>.log`).
///
/// MUST be the first act of `main`: `fork` duplicates only the calling thread, so forking after
/// a thread is spawned would leave the child missing it, possibly mid-lock.
fn daemonize(sock: &Path) -> io::Result<()> {
    // SAFETY: this is called before any thread is spawned, so the forked child is
    // single-threaded. That is the whole safety argument: with no other thread, no lock can be
    // held across the fork, so the child may call even the NON-async-signal-safe code in
    // `redirect_stdio` (open/alloc) without risking a deadlock on an inconsistent lock. (Only
    // `setsid` runs here directly, and it IS async-signal-safe.)
    match unsafe { libc::fork() } {
        -1 => return Err(io::Error::last_os_error()),
        0 => {}                     // child: become the daemon
        _ => std::process::exit(0), // parent: hand off and go
    }
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    redirect_stdio(&sock.with_extension("log"))
}

/// Point stdin at `/dev/null` and stdout+stderr at `log_path` — a detached daemon has no
/// terminal to inherit, and its `tracing`/panic output must land somewhere an operator can
/// read rather than vanish.
fn redirect_stdio(log_path: &Path) -> io::Result<()> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let devnull = OpenOptions::new().read(true).open("/dev/null")?;
    // dup2 the targets onto the standard fds; the source handles close on scope exit, leaving
    // 0/1/2 as independent duplicates pointing at the log / null.
    for (src, dst) in [
        (devnull.as_raw_fd(), 0),
        (log.as_raw_fd(), 1),
        (log.as_raw_fd(), 2),
    ] {
        if unsafe { libc::dup2(src, dst) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Take the daemon's single-instance lock, `<socket>.lock` — DERIVED from the endpoint path so
/// each socket has its own lock: two daemons on two sockets do not contend (tmux's per-socket
/// model), and a spawned daemon's lock always matches the socket its spawner will connect to.
fn acquire_single_instance(sock: &Path) -> io::Result<Option<File>> {
    flock_guard(&sock.with_extension("lock"))
}

/// A non-blocking exclusive advisory `flock` on `path`: `Some(file)` when taken (held for as
/// long as the returned handle lives — dropping it releases the lock), `None` when another
/// holder already owns it.
///
/// Acquired BEFORE `mount`, so two daemons racing connect-or-spawn cannot both bind the
/// socket: the loser sees the lock held and exits before touching it. The guard lives here,
/// in the daemon's boot, and not in the transport — pinion's `serve` removes the socket path
/// before binding, so an unguarded second bind would silently orphan the winner's clients.
fn flock_guard(path: &Path) -> io::Result<Option<File>> {
    // The lock file is a rendezvous, not storage: create it if absent, never truncate it
    // (its contents are irrelevant — the advisory lock, not the bytes, is the signal).
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(None),
        _ => Err(error),
    }
}

/// Parse a `COLSxROWS` size specifier.
fn parse_size(spec: &str) -> Option<(u16, u16)> {
    let (w, h) = spec.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-instance lock admits exactly one holder, and releasing it lets the next in —
    /// the mechanism that makes a connect-or-spawn RACE leave exactly one daemon alive. Proven
    /// without forking: two separate opens of one path are independent flock holders even
    /// within a process, so the second is refused while the first lives.
    #[test]
    fn the_single_instance_lock_admits_one_holder_at_a_time() {
        let path =
            std::env::temp_dir().join(format!("sprag-host-flock-test-{}.lock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let first = flock_guard(&path)
            .expect("io ok")
            .expect("the lock is free, so it is taken");
        assert!(
            flock_guard(&path).expect("io ok").is_none(),
            "a second holder is refused while the first lives",
        );

        drop(first);
        let third = flock_guard(&path)
            .expect("io ok")
            .expect("released, so the next daemon takes it");
        drop(third);
        let _ = std::fs::remove_file(&path);
    }
}
