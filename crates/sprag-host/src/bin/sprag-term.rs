//! `sprag-term` — the headless terminal-multiplexer RPC server (GPU-free).
//!
//! Starts a workspace — with one initial pane (a shell, or the command after
//! `--`) on a pseudoterminal, or, as a `--daemon`, RESTORED from its last durability
//! snapshot (else empty; see below) — and serves
//! pinion's scene-as-data wire -- panes +
//! the `/sprag_mux` control surface + the `/sprag_plugins` platform -- over two
//! transports at once (DESIGN.md §1 + §3): the process stdin/stdout (one
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
//! — or a shell in its cwd), else boots empty. Each pane also comes back with its SCROLLBACK, saved
//! beside the snapshot as replayable terminal bytes (the `history` module) and bounded by
//! `SPRAG_RESTORE_HISTORY` lines — `0` turns it off, which stops saving and restoring without
//! deleting anything. A natural last-pane exit KEEPS the snapshot (so a
//! transient program exit retries next boot). The daemon lifecycle otherwise PRESERVES the
//! snapshot — a reboot, a crash, a natural close, and a plain `kill-server` all leave it, so the
//! workspace comes back; only `sprag kill-server --purge` destroys the saved workspace, snapshot
//! and pane histories alike (CLI-side).
//! Standalone mode (no `--daemon`) never persists and is unchanged.

// A binary crate: `cargo doc` builds it with private items and its crate-root doc links to the
// bin's own internals. `private_intra_doc_links` guards LIBRARY public-API docs (which publish
// without private items); a bin has no such surface, so the lint is a structural false positive
// here (mirrors `sprag-gui`) — declared crate-wide so a future internal link cannot re-break it.
#![allow(rustdoc::private_intra_doc_links)]

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGINT, SIGTERM};
use sprag_host::{
    AgentClock, ChannelRegistry, FrameIngress, Host, HostState, RunRegistry, SavedHistory,
    bump_on_dirty, dispatch_frames, history_dir, history_limits, load_pane_history, load_snapshot,
    pane_exit_hook, save_histories_if_changed, save_if_changed, snapshot_path, spawn_reaper,
    stdin_frames,
};
use sprag_rpc::HOST_SOCKET;
use sprag_terminal::{CommandBuilder, PaneId, SessionRegistry, Snapshot, Workspace};
use sprag_vt::HistoryLimits;
use tracing_subscriber::{EnvFilter, fmt};

/// The session a standalone boot pane lands in — the registry's boot session, the one an
/// unscoped request resolves to. Named here because its pane's change-notification token must be
/// looked up by that name before `HostState` exists to answer for it.
const BOOT_SESSION: &str = "0";

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
    // The initial pane's `on_dirty` bumps ITS SESSION's scene-version token, so its output wakes
    // the parked async `scene/waitFor` replies on that session (the change-notification a wire
    // client long-polls instead of busy-polling snapshots) and no others. The channels are created
    // BEFORE the spawn so the bumper and HostState share the one registry — two would leave the
    // boot pane announcing on a token nobody ever waits on.
    let channels = Arc::new(sprag_host::ChannelRegistry::default());
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
    // the saved workspace (CLI-side).
    //
    // "Do not exit mid-restore" is NOT stated here. It used to be — an `AtomicBool` this closure
    // read — and that was one binary teaching itself a rule the library also needed for a plain
    // `new_session`. `Host::restore` now holds a `BirthPin` across its re-spawn loop, which says
    // the same thing where every caller inherits it, and says the half the flag could not: a
    // restore that spawned NOTHING releases the claim and re-asks, instead of leaving a daemon
    // running with an empty registry.
    let on_pane_exit = spawn_reaper(
        Arc::clone(host.registry()),
        Arc::new(move || {
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
    // Where each pane's scrollback lives, and how much of it survives a restart. Both read ONCE
    // here and injected, so the library names no state directory and reads no environment. A zero
    // limit disables history on BOTH edges — nothing saved, nothing replayed — while leaving
    // whatever is already on disk untouched (`kill-server --purge` is the one destroying verb).
    let hist_dir = history_dir(&sock);
    let hist_limits = history_limits();
    if args.daemon {
        if let Some(snapshot) = load_snapshot(&snap_path) {
            // Run it with the exact-command allowlist (read once from the environment here,
            // injected so the host does not touch it). The reaper needs no gate from this side:
            // `restore` claims the daemon's life for the length of its own loop.
            let outcome = host.restore(
                snapshot,
                &sprag_host::restore_allowlist(),
                |session| Some(bump_on_dirty(&channels.revision(session))),
                || Some(pane_exit_hook(&on_pane_exit)),
                |id| match hist_limits.lines {
                    0 => Vec::new(),
                    _ => load_pane_history(&hist_dir, id),
                },
            );
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
        spawn_durability_saver(
            Arc::clone(host.registry()),
            snap_path,
            hist_dir,
            hist_limits,
        );
    } else {
        host.spawn(
            args.command,
            args.label,
            args.cols,
            args.rows,
            Some(bump_on_dirty(&channels.revision(BOOT_SESSION))),
            Some(pane_exit_hook(&on_pane_exit)),
        )
        .map_err(io::Error::other)?;
    }
    // The agent-state memory (H3), shared by the pane list that reads it and the waker that keeps
    // its clock. The two are installed TOGETHER because a registry without a waker publishes
    // `Blocked` promptly and `Idle` only by luck — see `spawn_agent_waker`.
    //
    // The manifests come from the user's `config.toml` layered over the built-ins, and the HOLDER
    // goes to the waker rather than being read here: the file is read once at start-up and then only
    // when the sweep looks again, which is what keeps a compile of every agent's patterns off a path
    // served on every client wake (`config::AgentManifests`).
    let manifests = sprag_host::config::AgentManifests::load();
    let agents = Arc::new(AgentClock::new(manifests.rules().clone()));
    spawn_agent_waker(
        Arc::clone(host.registry()),
        Arc::clone(&agents),
        Arc::clone(&channels),
        manifests,
    );
    let state = HostState::new(host, channels, Some(on_pane_exit)).with_agents(agents);

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

/// Spawn the durability save loop (daemon only): every [`SNAPSHOT_INTERVAL`], persist the
/// workspace's SHAPE (the [`Snapshot`]) and its CONTENT (each pane's scrollback) — each written
/// ATOMICALLY, and only when it differs from what was last saved, so an idle daemon rewrites
/// nothing. This is the cmux-parity ring: a reboot ends the daemon and every PTY, but what is on
/// disk lets the NEXT daemon rebuild the sessions, windows, layout, working directories and
/// scrollback a live PTY could never carry across.
///
/// One thread drives both halves so they are written from the same tick rather than drifting
/// apart on two timers. Both projections take the registry then each workspace lock SEQUENTIALLY
/// (never nested), so this background thread never contends with dispatch beyond a brief
/// membership read.
///
/// A transient save error is logged and retried next tick (the last-saved state is left
/// unchanged), so a full disk or a momentary permission glitch does not silently stop persistence.
/// The two halves fail INDEPENDENTLY: an unwritable history must not cost the workspace its shape.
fn spawn_durability_saver(
    registry: Arc<Mutex<SessionRegistry>>,
    path: PathBuf,
    history_dir: PathBuf,
    history_limits: HistoryLimits,
) {
    thread::spawn(move || {
        // `save_if_changed` / `save_histories_if_changed` own the write-if-changed dedup (both
        // tested in their own modules); the loop is just them on a timer, carrying what was last
        // saved between ticks.
        let mut last: Option<Snapshot> = None;
        let mut last_histories: HashMap<PaneId, SavedHistory> = HashMap::new();
        loop {
            thread::sleep(SNAPSHOT_INTERVAL);
            if let Err(e) = save_if_changed(&path, &registry, &mut last) {
                tracing::warn!(
                    target: "sprag_host::durability",
                    "snapshot save to {} failed: {e}",
                    path.display()
                );
            }
            if let Err(e) = save_histories_if_changed(
                &history_dir,
                &registry,
                history_limits,
                &mut last_histories,
            ) {
                tracing::warn!(
                    target: "sprag_host::durability",
                    "pane history save to {} failed: {e}",
                    history_dir.display()
                );
            }
        }
    });
}

/// How often the agent waker SWEEPS: discover panes nobody has asked about, and forget panes that are
/// gone.
///
/// # Why a sweep is needed at all, and what it costs
///
/// A candidate is created by an OBSERVATION, and until slice 3 the only thing that observed was the
/// pane-list query. So a daemon nobody had ever queried held no state for any pane — measured, not
/// reasoned: a first-ever one-shot read of an agent pane on a five-second-old daemon answered nothing,
/// because the read was itself that pane's first observation and a resting verdict has to hold. That
/// makes "ask once and get an answer" false for exactly the caller slice 5 is FOR: an MCP or CLI peer
/// on a daemon with no attached frontend.
///
/// The fix is not to poll the panes. The sweep observes a pane only while it is UNKNOWN, and a pane is
/// unknown once — so the evaluation cost is one screen read per pane for the life of the daemon, not
/// one per interval. What recurs is the walk itself: the registry lock, then each workspace lock, and
/// a pane-id read each. [`spawn_durability_saver`] already takes exactly those locks at exactly this
/// interval and then does strictly more (a history epoch per pane, and an encode when one moved), so
/// the marginal cost of this thread on an idle daemon is a second cheap lock acquisition every five
/// seconds. M3's "a quiet workspace costs nothing" is about the evaluation, and the evaluation is what
/// the sweep does not repeat.
///
/// Five seconds because it is [`SNAPSHOT_INTERVAL`]'s reason applied to a different fact: it bounds how
/// long after a pane's birth its state can be unknown to a caller who never asks twice.
const AGENT_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Spawn the agent settle waker (daemon only): confirm the verdicts whose window has closed, wake the
/// clients of the sessions whose answers moved, and forget the panes that are gone.
///
/// # Why a thread exists at all
///
/// The pane list evaluates a pane when a CLIENT asks, and a client asks when the session's scene
/// revision moves — which pane output and user actions advance. That drives every verdict resting on
/// evidence PRESENT on the screen: the output that paints a dialog is the same event that wakes the
/// reader, so `Blocked` reaches a person on sight.
///
/// It does not drive the other half. A verdict resting on an ABSENCE — the agent stopped working, so
/// it is `Idle` and wants you — has to hold for the settle window before it is believed, and the last
/// thing to move the revision was **the output that stopped**. In a workspace with one agent pane and
/// nothing else happening there is no second event, so without this thread that pane would sit at its
/// previous state until something unrelated happened to wake a client. The failure is worse than a
/// plain hang because it is data-dependent: six busy agent panes supply each other's wakes, so it
/// works in exactly the case the feature is for and freezes in the quiet one.
///
/// This thread also OBSERVES rather than merely bumping, and that is the difference between waking a
/// reader and having an answer to give one. With no client attached a bump wakes nobody, so a later
/// one-shot reader (`sprag` on the CLI, an MCP call) would find a tracker that had never been asked.
/// Confirming here means the read path stays a pure read.
///
/// # It waits on an EVENT, not on a tick — and the first version of this did not
///
/// The obvious shape is to compute the nearest deadline, sleep until it, and repeat. That was written,
/// and a live drive against the daemon showed it never published anything: at start-up nothing is
/// pending, so the nearest deadline is `None`, so the thread slept for the prune interval — and the
/// pane-list query that then created a two-second candidate had no way to tell it. **The mechanism
/// built to serve a deadline nothing else would serve was itself waiting for an event nothing
/// produced**, which is D9's own defect one level up, and only driving the binary found it.
///
/// A shorter sleep is not the fix: waking a few times a second to ask whether anything is pending yet
/// would take every workspace lock several times a second on a daemon where nothing is happening,
/// which is the cost M3 measured away. So the appearance of a candidate is an event —
/// [`AgentClock::park_until_due`] parks on it — and the loop below wakes for exactly three reasons: a
/// deadline came due, a candidate appeared (so the sleep has to be re-planned around a nearer
/// deadline), or the prune interval elapsed. Only the first and third do any work.
///
/// # Cost
///
/// With nothing pending the thread is BLOCKED, not spinning, and wakes on the sweep interval to
/// discover new panes and bound memory. With something pending it wakes at that deadline: one wake per
/// transition.
///
/// Lock discipline is [`spawn_durability_saver`]'s, for the same reason: the registry lock is taken
/// and RELEASED to clone out the pools, then each workspace lock is taken on its own, never nested.
/// The clock's lock is taken inside a workspace lock (the screen is only reachable there) and never
/// the other way round.
fn spawn_agent_waker(
    registry: Arc<Mutex<SessionRegistry>>,
    agents: Arc<AgentClock>,
    channels: Arc<ChannelRegistry>,
    mut manifests: sprag_host::config::AgentManifests,
) {
    thread::spawn(move || {
        let mut last_sweep = Instant::now();
        loop {
            // Blocked until there is something to do. Returns early when a candidate APPEARS, which is
            // not itself work — the guard below sends that wake straight back to the park.
            agents.park_until_due(AGENT_SWEEP_INTERVAL);
            let now = Instant::now();
            let due = agents.with(|state| state.any_due(now));
            let sweep = now.duration_since(last_sweep) >= AGENT_SWEEP_INTERVAL;
            if !due && !sweep {
                continue;
            }
            if sweep {
                last_sweep = now;
                // The user's manifests, re-read from a wake this thread already has — the whole of
                // what "runtime reload on the same terms as the keymap" means for a daemon. A client
                // re-reads on the keystroke whose meaning the table decides; there is no keystroke
                // here, and this sweep is the wake that exists. Reading the file per EVALUATION was
                // never available: a manifest owns compiled patterns.
                //
                // It runs before the walk, so the panes the edit invalidates are served by the very
                // pass that invalidated them rather than one sweep later.
                if manifests.refresh() {
                    agents.with(|state| state.reload(manifests.rules().clone()));
                }
            }
            // Phase 1 — registry lock ONLY: clone out each session's pools as handles, keeping the
            // session NAME beside each so a published change can wake that session's clients and no
            // others.
            let pools: Vec<(String, Arc<Mutex<Workspace>>)> = {
                let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
                reg.sessions()
                    .iter()
                    .flat_map(|session| {
                        let name = session.name().to_owned();
                        session
                            .windows()
                            .iter()
                            .map(move |window| (name.clone(), Arc::clone(window.workspace())))
                    })
                    .collect()
            };
            // Phase 2 — registry lock released. Each pool under its own lock.
            let mut live: HashSet<PaneId> = HashSet::new();
            let mut moved: HashSet<String> = HashSet::new();
            for (session, pool) in &pools {
                let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
                for pane in pool.panes() {
                    let id = pane.id();
                    live.insert(id);
                    // Three reasons to ask about a pane, and a pane can be more than one at once on
                    // the sweep that first sees it. DUE: the window has closed on a candidate, so what
                    // publishes it is the CLOCK — the third input a pending transition has. UNKNOWN:
                    // nobody has ever looked at this pane, so it has no state to be waiting on, and
                    // only the sweep can give it one. STALE: the pane is settled and known and its
                    // answer was reached under a ruleset the user has since edited — the input that
                    // moved was not on its screen, so nothing else will ever bring it back. None of
                    // the three applies to a settled pane under unchanged rules, which is every pane
                    // in a quiet workspace.
                    let ask = agents.with(|state| {
                        state.is_due(id, now) || (sweep && (!state.knows(id) || state.stale(id)))
                    });
                    if !ask {
                        continue;
                    }
                    let before = agents.with(|state| state.seq(id));
                    let title = pane.title();
                    pane.pty().with_screen(|screen| {
                        agents.observe(
                            id,
                            screen,
                            title.as_deref(),
                            now,
                            sprag_host::config::agent_settle,
                        );
                    });
                    if agents.with(|state| state.seq(id)) != before {
                        moved.insert(session.clone());
                    }
                    // THE LOOP'S LIVENESS RESTS ON THIS. `park_until_due` returns immediately for a
                    // deadline already past, so a due pane that an observation does not RESOLVE sends
                    // this thread round at full speed forever. It cannot happen: `is_due` is
                    // `since + settle <= now`, which is exactly `settle`'s own publish condition, and
                    // every other path through `observe` either publishes or re-dates the candidate to
                    // `now`. Stated as an assertion rather than a comment because the failure is a
                    // spin rather than a wrong answer — the mutation that removed the observe above
                    // took the scene revision from 264 to 6,178,283 in twelve seconds.
                    debug_assert!(
                        !agents.with(|state| state.is_due(id, now)),
                        "pane {id} is still due after being observed at the same instant",
                    );
                }
            }
            // A tracker must not outlive its pane. The census is DAEMON-WIDE, which is why it is
            // built here and never in the pane-list query: that walk sees one session, and pruning
            // against it would forget every other session's panes.
            if sweep {
                agents.with(|state| state.retain_live(&live));
            }
            // Wake the clients of the sessions whose published answer moved, and only those.
            for session in &moved {
                channels.bump(session);
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

    // No `--` command: the user's `default-command`, then `$SHELL` — the same resolver every other
    // pane birth uses.
    let (command, label) = command.unwrap_or_else(sprag_host::config::default_pane_command);
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
