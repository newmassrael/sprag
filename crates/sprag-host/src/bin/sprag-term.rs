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
//! sprag-term --drive <run-id> [-t <session>]
//! ```
//!
//! The second form is a DIFFERENT PROGRAM wearing the same image: not a multiplexer at all but a
//! client of one, driving a single already-registered run over the wire and reporting its outcome
//! on stdout. It is what the daemon spawns to put a run in its own process; see
//! [`sprag_host::drive`] for what it connects and why. The split happens before any argument is
//! parsed, because the terminal's parser reads an unknown first word as a pane command.
//!
//! With no command the initial pane runs `$SHELL` (else `/bin/sh`). Socket
//! policy: `$XDG_RUNTIME_DIR/sprag-host.sock` (override `SPRAG_HOST_RPC_SOCK`),
//! enabled unless `SPRAG_HOST_RPC` is falsey; `kill -USR1`/`-USR2` enable /
//! disable it live.
//!
//! Every pane this server births is told two things about the world it is in, so a process INSIDE a
//! pane can talk back rather than only be scraped (tmux's `$TMUX` / `$TMUX_PANE`): `SPRAG_PANE` is
//! its own pane id — the `id` every wire method addressing a pane takes — and `SPRAG_HOST_RPC_SOCK`
//! is this server's endpoint, so a `sprag` run inside a pane reaches the daemon that owns it with no
//! argument. Both are birth-time, as any environment is: a process outliving its pane keeps an id
//! the daemon answers as unknown, never one that comes to mean a different pane. As a server it runs until SIGINT/SIGTERM OR until its LAST
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

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use signal_hook::consts::{SIGINT, SIGTERM};
use sprag_host::agent::SWEEP_INTERVAL;
use sprag_host::attention::spawn_attention_router;
use sprag_host::{
    AgentClock, AttachmentRegistry, ChannelRegistry, FrameIngress, Host, HostState, JobWatch,
    RunRegistry, SavedHistory, bump_on_dirty, dispatch_channel, dispatch_frames, history_dir,
    history_limits, load_pane_history, load_snapshot, pane_attention_hook, pane_exit_hook,
    save_histories_if_changed, save_if_changed, snapshot_path, spawn_reaper, stdin_frames,
    sweep_once,
};
use sprag_rpc::HOST_SOCKET;
use sprag_terminal::{CommandBuilder, PaneId, SessionRegistry, Snapshot};
use sprag_vt::HistoryLimits;
use tracing_subscriber::{EnvFilter, fmt};

/// The session a standalone boot pane lands in — the registry's boot session, the one an
/// unscoped request resolves to. Named here because its pane's change-notification token must be
/// looked up by that name before `HostState` exists to answer for it.
const BOOT_SESSION: &str = "0";

/// What a `--drive` argv asked this image to drive.
struct DriveOrder {
    /// The id the daemon registered the run under — what an order names when it reaches the
    /// driver, and what the driver re-reads the run's row by.
    run: u64,
    /// The session every connection this driver opens is scoped to, or the daemon's unscoped
    /// default. It must be the scope the run's panes live in: a park scoped elsewhere is refused
    /// at the door (`RemotePaneAccess::parking_on`).
    session: Option<String>,
    /// ⚠⚠⚠⚠⚠ WHICH WINDOW of that session the run's pane is in — register item 690, and the other
    /// half of the address. Without it every request this driver makes is read against whatever
    /// window the session is CURRENTLY showing, so anybody selecting another window kills a run
    /// that is working. `None` for a daemon too old to name one, which keeps that daemon's
    /// behaviour exactly as it was.
    window: Option<String>,
}

/// **WHICH PROGRAM THIS IMAGE IS** — a driver order, or `None` for the terminal.
///
/// ⚠⚠⚠ This is read BEFORE [`parse_args`], and cannot move into it. That parser treats the first
/// word it does not know as the PROGRAM TO RUN IN THE BOOT PANE, so a `--drive` reaching it would
/// not be a mis-parse the user sees — it would silently open a terminal running a pane called
/// `--drive`. A driver is a different program wearing the same image, and the split belongs where
/// no parse has happened yet.
///
/// # Errors
///
/// A `--drive` with no run id after it, a run id that is not a number, or a `-t`/`--session` with
/// no name after it. **All three fail rather than fall back to booting a terminal** — a daemon that
/// spawned a driver and got a multiplexer would hold a socket nobody asked it to hold.
fn drive_order<I: Iterator<Item = String>>(mut args: I) -> io::Result<Option<DriveOrder>> {
    let mut run: Option<u64> = None;
    let mut session: Option<String> = None;
    let mut window: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            sprag_host::drive::DRIVE_FLAG => {
                let raw = args.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "{} takes the run id to drive",
                            sprag_host::drive::DRIVE_FLAG
                        ),
                    )
                })?;
                run = Some(raw.parse::<u64>().map_err(|why| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{raw:?} is not a run id: {why}"),
                    )
                })?);
            }
            "-t" | "--session" => {
                session = Some(args.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{arg} takes the session name to scope to"),
                    )
                })?);
            }
            // ⚠ Register item 690 — see `DriveOrder::window`. Refused with a sentence rather than
            // ignored, on `-t`'s rule beside it: a daemon that meant to name a window and named
            // nothing would otherwise spawn a driver that silently follows the current window,
            // which is exactly the defect this flag exists to remove.
            sprag_host::drive::DRIVE_WINDOW_FLAG | "--window" => {
                window = Some(args.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{arg} takes the window name the run's pane is in"),
                    )
                })?);
            }
            // Anything else belongs to the terminal's own parser. A driver argv is written by the
            // daemon, not by hand, so there is nothing else here to understand.
            _ => {}
        }
    }

    Ok(run.map(|run| DriveOrder {
        run,
        session,
        window,
    }))
}

fn main() -> io::Result<()> {
    // ⛔⛔⛔⛔⛔ **BEFORE ANYTHING ELSE, THIS IMAGE SAYS WHAT IT IS** — register item 763.
    //
    // A driver is started from a HANDLE to the daemon's own image rather than from the name that
    // image goes by (`sprag_host::image_to_spawn`), because a promotion replaces the file while the
    // daemon is running and the name stops being a file. The kernel takes `comm` from the path it
    // was handed, so a child started that way answers to `exe` — and `comm` is what every identity
    // check in this repository compares (`leftover_driver`, `daemon_pid`, `pane_processes`). This
    // is where the name comes back, and it is FIRST so the window in which it is wrong is the
    // shortest this program can make it.
    sprag_host::name_this_image();
    // ⚠⚠⚠⚠⚠ THE DRIVER SPLIT IS THE FIRST THING `main` DOES. Everything below boots a
    // multiplexer — a lock, a log, a pane, a socket — and a driver wants none of it: it is a client
    // of a daemon that already exists, and it reaches that daemon by the SAME endpoint resolution
    // (`SPRAG_HOST_RPC_SOCK`, inherited from the daemon that spawned it), so the two can never
    // disagree about which host they mean.
    if let Some(order) = drive_order(std::env::args().skip(1))? {
        let sock = sprag_rpc::socket_path(HOST_SOCKET);
        return sprag_host::drive::drive(
            &sock,
            order.session.as_deref(),
            order.window.as_deref(),
            order.run,
        );
    }

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

    // ⛔⛔⛔⛔⛔ **A HOME THIS PROCESS WAS TOLD AND COULD NOT USE IS SAID OUT LOUD** — register item
    // 802, and this is the FIRST thing said because everything below writes through those two
    // directories.
    //
    // The XDG specification makes a non-absolute `XDG_STATE_HOME` / `XDG_CONFIG_HOME` invalid, so
    // sprag ignores it and falls back to the user's home — correct, and until now completely
    // silent. Whoever set the variable meant to choose a directory; they got somebody else's, and
    // nothing anywhere said the choice had been dropped. Measured cost on this machine: a test
    // harness whose `TMPDIR` was set-and-empty derived a relative state home from
    // `std::env::temp_dir()` (item 794's shape), and fourteen daemons wrote the developer's real
    // `~/.local/state/sprag` while the harness's cleanup removed a relative directory that was
    // never the one being written.
    //
    // ⚠ A WARNING RATHER THAN A REFUSAL, deliberately: a person whose environment is wrong must
    // still get a working multiplexer, and the fallback is what the spec asks for. What was
    // missing was not the behaviour but the sentence.
    for said in sprag_host::refused_homes() {
        tracing::warn!(target: "sprag_host::durability", "{said}");
    }

    // Take the cgroup subtree a pane's share is enforced in, BEFORE any pane is born (R336).
    //
    // Two things change on the machine and neither is undone by dropping the handle: this process
    // moves into a scope of its own, and that scope's controllers are turned on for its children.
    // So the daemon does not hold the tree open — the pane-birth path re-derives it by adopting the
    // same root, which is idempotent by construction.
    //
    // It also fixes a lifetime that was wrong by accident: a daemon that inherits the launching
    // terminal's transient scope DIES WITH THAT TERMINAL, which is the opposite of what a
    // multiplexer promises. Its own scope outlives the window that started it.
    #[cfg(target_os = "linux")]
    let shares = take_share_subtree();

    // The one Workspace owner (shared with the GUI as a code component): boot the
    // initial pane through it, then wrap it in HostState to serve the RPC surface.
    //
    // The initial pane's `on_dirty` bumps ITS SESSION's scene-version token, so its output wakes
    // the parked async `scene/waitFor` replies on that session (the change-notification a wire
    // client long-polls instead of busy-polling snapshots) and no others. The channels are created
    // BEFORE the spawn so the bumper and HostState share the one registry — two would leave the
    // boot pane announcing on a token nobody ever waits on.
    let channels = Arc::new(sprag_host::ChannelRegistry::default());
    // Every pane this daemon births is told which pane it is and where THIS daemon listens
    // (`sprag_host::pane_env_source`), so a process inside a pane can report rather than only be
    // scraped. Installed here, at the site that also `mount`s the endpoint below, because publishing
    // an address is a promise to serve it — the GUI's in-process host, which serves no host socket,
    // installs nothing and its panes are spawned exactly as before. The path is `sock`, resolved
    // above once, so what a pane is told and what `mount` binds cannot differ.
    // And every AGENT this daemon launches is launched already reporting: the pane environment says
    // where to report, and this says how. Installed at the same site for the same reason — the
    // instrumentation names the `sprag` beside this binary, and a host that serves no socket has
    // nowhere for a report to go, so the GUI's in-process host installs neither half.
    // And what each of those launches is CALLED, so a daemon that has to restart to adopt new code
    // brings its agents back into the conversations they were in rather than into fresh ones. It
    // rides beside the instrumentation because it reads what the instrumentation added — a host
    // installing one without the other would record names nothing wrote, or write names nothing
    // records.
    let host = Host::new((args.cols, args.rows))
        .with_pane_env(sprag_host::pane_env_source(&sock))
        .with_pane_args(sprag_host::pane_args_source())
        .with_pane_identity(sprag_host::pane_identity_source());
    // Every pane this daemon births lands in the subtree taken above, so a person's session cannot
    // be starved by whichever neighbour spawned the most threads. A host with no tree spawns
    // exactly as it always did.
    #[cfg(target_os = "linux")]
    let host = match shares {
        Some(tree) => host.with_shares(tree),
        None => host,
    };
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
    // The per-client attachment map and the ATTENTION ROUTER, built HERE — before any pane exists —
    // because a pane's child can ask for a person from its very first batch of output, and the boot
    // pane is spawned below. The map is handed to `HostState` further down, so the router's
    // deliveries and the dispatch layer's reads are one map rather than two that agree at first.
    let attachments = Arc::new(std::sync::Mutex::new(AttachmentRegistry::default()));
    let attention = Arc::new(spawn_attention_router(
        Arc::clone(host.registry()),
        Arc::clone(&attachments),
        Arc::clone(&channels),
    ));
    // THE RUN REGISTRY IS BUILT HERE, not by `HostState::new`, because the predecessor's run log
    // has to be read into it before the saver starts writing over that file.
    let runs = Arc::new(Mutex::new(sprag_host::runs::RunRegistry::default()));
    let runs_file = sprag_host::runs_path(&sock);
    if args.daemon {
        if let Some(log) = sprag_host::load_runs(&runs_file) {
            // ⚠ Every run in it was driven by a process that is gone. `restore` marks the ones that
            // were still going as INTERRUPTED and drops their provenance — see `RunRegistry::restore`
            // for why a restored run must belong to nobody.
            runs.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .restore(&log);
        }
        if let Some(snapshot) = load_snapshot(&snap_path) {
            // Run it with the exact-command allowlist (read once from the environment here,
            // injected so the host does not touch it). The reaper needs no gate from this side:
            // `restore` claims the daemon's life for the length of its own loop.
            let outcome = host.restore(
                snapshot,
                &sprag_host::restore_allowlist(),
                |session| Some(bump_on_dirty(&channels.revision(session))),
                || Some(pane_exit_hook(&on_pane_exit)),
                || Some(pane_attention_hook(&attention)),
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
            Some((runs_file, Arc::clone(&runs))),
        );
    } else {
        host.spawn(
            args.command,
            args.label,
            args.cols,
            args.rows,
            sprag_terminal::PaneBirthHooks {
                on_dirty: Some(bump_on_dirty(&channels.revision(BOOT_SESSION))),
                on_exit: Some(pane_exit_hook(&on_pane_exit)),
                on_attention: Some(pane_attention_hook(&attention)),
            },
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
    // A file that is already broken at BOOT is reportable from the FIRST request, not from the first
    // sweep five seconds later — and published HERE, synchronously, rather than as the waker's first
    // act, because the socket below would otherwise be racing a thread that has not been scheduled
    // yet. `false`: the clock was just constructed from these rules, so no reload is owed.
    adopt_manifests(&agents, &manifests, false);
    spawn_agent_waker(
        Arc::clone(host.registry()),
        Arc::clone(&agents),
        Arc::clone(&channels),
        manifests,
    );
    // ⚠⚠⚠⚠⚠ **AND HERE A RESTART STOPS KILLING THE RUNS IT INHERITED** — register item 543's sixth
    // brick, and the reason it is HERE rather than beside the `restore` that read the log: that
    // call happens before a single pane has come back, and a run cannot be put back on a driver
    // whose pane does not exist. Everything a resume needs is in hand at this point and at no
    // earlier one — the restored panes, the agent clock a pane access reads verdicts from, and the
    // channels a resumed run's news is announced on.
    //
    // ⚠ AFTER the durability saver has started, which costs nothing: a run waiting to be put back
    // is still in the registry, still `interrupted`, and still carries the place and the request
    // the log gave it — so a save that lands in this window writes exactly what it read.
    // ⚠⚠⚠⚠⚠ **THIS IMAGE CAN BE A DRIVER, AND IT IS THE ONLY ONE THAT CAN** — register item 544.
    // `sprag_host::driver_spawn` starts `current_exe()` with `--drive`, and this binary is the one
    // that answers that argv. The library used to mint it for whichever host happened to be
    // assembling a scene; the day `run-driver-process` defaulted to `on`, eighteen test binaries
    // began spawning THEMSELVES and waiting for the copy to drive a run. So the ability is granted
    // here, by the image that has it, and the OPTION decides whether to use it.
    let spawn_driver: Option<sprag_host::DriverSpawns> =
        sprag_host::config::option_is_on(sprag_host::options::RUN_DRIVER_PROCESS)
            .then(|| Arc::new(sprag_host::driver_spawn) as sprag_host::DriverSpawns);
    if args.daemon {
        put_back_inherited_runs(
            &host,
            &runs,
            &channels,
            &on_pane_exit,
            &attention,
            &agents,
            spawn_driver.clone(),
        );
    }
    let mut state = HostState::new(host, channels, Some(on_pane_exit))
        .with_runs(runs)
        .with_attachments(attachments)
        .with_attention(attention)
        .with_agents(agents);
    if let Some(spawn) = spawn_driver {
        state = state.driving_out_of_process(spawn);
    }

    // One dispatch owner (this thread) serialises all dispatch; the always-on
    // socket and stdin are producers of RpcFrames into it, so a socket client
    // and a stdin line share one consistent HostState view.
    // Made through `dispatch_channel` rather than `mpsc::channel` so the output-wait signal is
    // wired into `state` by construction: a `pane/waitForOutput` parked on this daemon is woken by
    // a pane's own output through this very sender.
    let (tx, rx) = dispatch_channel(&state);
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

/// Ask systemd for a delegated cgroup subtree and turn on what a pane's share needs, reporting
/// either way (R336).
///
/// Never fatal, by design. A person opening a terminal is not asking for resource control, and a
/// machine that cannot give one — no systemd, no session bus, a container, cgroup v1 — must still
/// get their terminal. What it must NOT do is stay quiet about it: a share that is set and silently
/// not enforced is worse than one the daemon said it could not enforce, so both outcomes are logged
/// and the unenforceable one carries its reason.
///
/// The handle is dropped on purpose. `adopt` acts on the MACHINE — the process is moved, the
/// controllers are on — and the pane-birth path re-derives the same tree from the same root.
#[cfg(target_os = "linux")]
fn take_share_subtree() -> Option<Arc<sprag_terminal::Tree>> {
    use sprag_terminal::share::{Enforcement, Tree};

    let delegated = match sprag_host::delegation::acquire(std::process::id()) {
        Ok(delegated) => delegated,
        Err(error) => {
            tracing::warn!(%error, "pane shares will not be enforced");
            return None;
        }
    };
    // Asked AFTER the move, about the cgroup this process is in NOW: the answer before it would
    // have described the terminal's scope, which is the one that cannot weight anything.
    match Enforcement::probe() {
        Enforcement::Available { home } => match Tree::adopt(home.clone()) {
            Ok(tree) => {
                tracing::info!(
                    unit = delegated.unit(),
                    root = %home.display(),
                    "pane shares enforced"
                );
                Some(Arc::new(tree))
            }
            Err(error) => {
                tracing::warn!(%error, "pane shares will not be enforced");
                None
            }
        },
        Enforcement::Unenforceable { reason } => {
            // A scope that was delegated and still cannot weight is worth its own sentence: it
            // means the user manager handed over a subtree without the CPU controller in it.
            tracing::warn!(%reason, unit = delegated.unit(), "pane shares will not be enforced");
            None
        }
    }
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
/// ⚠⚠⚠⚠⚠ **PUT EVERY RUN THIS DAEMON INHERITED BACK ON A DRIVER, WHERE ITS OWN LOG SAYS IT WAS** —
/// register item 543's sixth brick, and the point of the five before it.
///
/// # ⚠⚠⚠⚠⚠ What this ends
///
/// *"A run's machine is never persisted, so every restart kills every run."* That sentence was prose
/// inside a closed register entry for three days before it got a number, and it is what makes
/// promoting this daemon's own build a destructive act — four supervisors share one daemon (item
/// 196), so a promotion killed three other repositories' loops and the cheap way round it was to
/// split daemons and pay for a second GUI (items 526 / 285). A restart that resumes runs is a
/// promotion nobody has to schedule around.
///
/// # ⚠⚠⚠ Each run is answered on its own, and a refusal is not a boot failure
///
/// One inherited run whose pane did not come back must not cost the others theirs, and none of them
/// may cost the daemon its boot: everything here degrades to the honest `interrupted` such a run
/// already reports. What each refusal costs a person is one log line saying which run and why —
/// which is the whole difference from the silence this replaces.
///
/// ⚠⚠ **THE POOL IS FOUND FROM THE PANE, NOT FROM A SESSION NAME**, because a run log records
/// neither: `pool_holding` answers which window's pool a pane is sitting in, and that is the pool a
/// pane access must speak. A pane the restore did not bring back has no pool, and that is the
/// honest reason not to resume the run that drove it.
/// Whether `pid` is a `sprag-term` DRIVER left over from a predecessor daemon on `endpoint` —
/// register item 526.
///
/// # ⚠⚠⚠⚠⚠ Three conditions, and dropping any one of them makes this dangerous
///
/// It ends what it identifies, so it may not identify anything else. **The name** keeps it from
/// signalling whatever program has since been given a recycled pid. **The endpoint, read from the
/// process's own environment**, is what makes it OURS: sockets split the world (register item 526's
/// own reasoning), so a `sprag-term` holding a different one belongs to a different daemon and a
/// different person's panes — `daemon_pid`'s note in the `cli` suite records the round where a
/// probe that skipped this SIGKILLed a developer's own terminal. And **not this process**, because
/// this daemon holds that endpoint too.
///
/// ⚠ It cannot be a live daemon: this boot only reaches it having found the socket free to take.
fn leftover_driver(pid: u32, endpoint: &std::path::Path) -> bool {
    if pid == std::process::id() {
        return false;
    }
    let want = format!("SPRAG_HOST_RPC_SOCK={}", endpoint.display());
    sprag_terminal::procfs::pids_named(sprag_rpc::DAEMON_BIN_NAME).contains(&pid)
        && sprag_terminal::procfs::environ(pid).is_some_and(|environ| {
            environ
                .split(|byte| *byte == 0)
                .any(|value| value == want.as_bytes())
        })
}

/// **END A DRIVER A DEAD DAEMON LEFT BEHIND**, answering whether there was one to end — register
/// items 526 and 740, and the one place in this binary that signals another process.
///
/// # ⚠⚠⚠⚠⚠ Why the two loops below share it rather than each spelling a `kill`
///
/// Because they used to differ, and the difference was not a decision anybody took. Item 526 ends
/// the leftover of a run it is PUTTING BACK, so that a pane does not get two processes typing at
/// it. Item 740 measured what that leaves out: a promotion is usually a DOCUMENT change, a changed
/// document brings back nothing, and so the kill loop — which iterates the runs that came back —
/// ran over an empty list exactly when it was most needed. **Measured on this machine 2026-08-30**:
/// sixty-three boot readings, zero drivers ended, and three occasions on which the boot wrote *STILL
/// TYPING into that pane* and moved on.
///
/// The answer channel is the same on both sides and so is the remedy: a driver reports its outcome
/// on the stdout pipe of the process that spawned it (`crate::drive`'s module doc), so a driver
/// whose daemon is gone can finish hours of work that NOBODY WILL EVER BE ABLE TO READ. Item 526
/// judged that worth a kill for a run coming back. For a run that is NOT coming back the same
/// argument is strictly stronger — there is no successor driver to read the pane either, so what is
/// being preserved by leaving it alive is an answer with no reader at all.
///
/// ⚠ The residue, stated rather than hidden: whatever that loop did since its last persist is lost,
/// and — unlike item 526's arm — its run is not put back, so the work STOPS. That is the cost, and
/// item 740 is where it was weighed: the alternative is a stranger typing into a restored pane
/// forever. The row says which happened, so a person can start the loop again knowing why.
///
/// ⚠⚠ It waits for the process to actually leave the table, bounded. The caller's next act is to
/// tell a reader what it did, and *ended* has to have happened before it is said.
fn end_leftover_driver(pid: u32, endpoint: &std::path::Path) -> bool {
    if !leftover_driver(pid, endpoint) {
        return false;
    }
    // SAFETY: `pid` was recorded by a predecessor of THIS daemon as a run's driver, and
    // `leftover_driver` has just confirmed it is a live `sprag-term` holding this daemon's own
    // endpoint in its environment — which is what makes it ours to end.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until && leftover_driver(pid, endpoint) {
        thread::sleep(Duration::from_millis(20));
    }
    true
}

fn put_back_inherited_runs(
    host: &Host,
    runs: &Arc<Mutex<RunRegistry>>,
    channels: &Arc<ChannelRegistry>,
    on_pane_exit: &Arc<dyn Fn() + Send + Sync>,
    attention: &Arc<sprag_host::attention::AttentionRouter>,
    agents: &Arc<AgentClock>,
    spawn_driver: Option<sprag_host::DriverSpawns>,
) {
    // ⚠ The list is taken and the lock released before anything is built: putting one run back
    // takes the registry again (`put_back`), and a workspace lock before that.
    let inheritance = runs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .inheritance();
    // ⛔⛔⛔⛔⛔ **AND WHAT THIS BOOT IS NOT PUTTING BACK IS SAID FIRST** — register item 737. A
    // promotion is usually a DOCUMENT change (`sprag-plugin/build.rs`: *"the dangerous case is the
    // common one"*), a changed document makes a new run by construction (item 544), and until this
    // existed the whole of that arrived as an empty list — indistinguishable from a predecessor
    // that had no runs to leave. The person who promoted the build learned it by reading rows one
    // at a time and guessing.
    //
    // ⚠ Per run and NOT as a total: *two runs stayed behind* cannot be acted on, and the reasons
    // are not interchangeable — a foreign fingerprint is somebody's promotion, and a missing
    // request is a predecessor that never wrote one down.
    let endpoint = sprag_rpc::socket_path(HOST_SOCKET);
    for run in &inheritance.withheld {
        // ⛔⛔⛔⛔⛔ **AND WHAT WAS STILL DRIVING IT IS ENDED, WHICH UNTIL REGISTER ITEM 740 IT WAS
        // NOT.** The loop below ends the leftover driver of every run it PUTS BACK (item 526), and
        // a run that is not put back was never reached by it — so its driver, a process of its own
        // since item 544's stage 1, went on typing into that pane with its answer bound for a pipe
        // that closed with the dead daemon.
        //
        // ⚠⚠⚠⚠⚠ **AND THAT IS THE COMMON CASE, NOT THE RARE ONE**, which is what made the omission
        // matter: `sprag-plugin/build.rs` says the restart that motivates persisting a run at all is
        // a DOCUMENT change, a changed document withholds every run in the log at once (item 737),
        // and so the kill loop below ran over an empty list exactly when there was most to end.
        // **Measured 2026-08-30 on this daemon's own log**: sixty-three boot readings, `put a run
        // this daemon inherited back` zero times, `ended the driver process …` zero times, and
        // three runs reported as STILL TYPING and left alone.
        //
        // ⚠⚠ ITEM 737 LEFT THIS UNDECIDED ON PURPOSE and named the cost of each way: ending it
        // stops a loop that may still be working, leaving it costs an answer nobody can ever read.
        // Item 740 took it, and the deciding fact is that for a WITHHELD run there is no successor
        // driver either — so *leaving it alive* preserves work that no reader will ever meet, on a
        // pane this boot has just restored for somebody else. `end_leftover_driver`'s doc carries
        // the argument; the residue is that the loop stops, and the row says so.
        let ended = run
            .driver
            .filter(|pid| end_leftover_driver(*pid, &endpoint));
        // ⚠⚠⚠ WRITTEN TO THE ROW AS WELL AS TO THIS LOG, and that is register item 744's class
        // rather than a courtesy: this line reaches whoever was watching the terminal the daemon was
        // restarted in, and a promotion's whole point is that nobody has to be. The person who comes
        // back to `sprag runs` is the one who has to decide whether to start the loop again.
        if let Some(pid) = ended
            && !runs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .ended_leftover_driver(run.id, pid)
        {
            // ⚠ THE DOOR'S REFUSAL IS SAID RATHER THAN DISCARDED. It can only answer `false` if
            // this registry no longer holds the run `inheritance()` handed over a moment ago, or
            // holds it with no withheld reason — either way the two have stopped agreeing, which is
            // the shape `inheritance` itself reports one file over. A signal was sent to a process
            // and the row is about to not say so, and that is not something to swallow.
            tracing::warn!(
                target: "sprag_host::runs",
                run = run.id.0,
                driver = pid,
                "this boot ended a leftover driver and could not record it against its run, so \
                 the row will not say the process was stopped",
            );
        }
        // ⚠ ONE SPELLING, READ TWICE — `withheld_sentence`'s rule, and this clause is composed by
        // the same function the row uses (`plugins::leftover_sentence`) for that reason.
        let leftover = ended.map_or_else(String::new, |pid| {
            format!("; {}", sprag_host::plugins::leftover_sentence(pid))
        });
        tracing::warn!(
            target: "sprag_host::runs",
            run = run.id.0,
            label = %run.label,
            documents = sprag_plugin::STATECHARTS_FINGERPRINT,
            "this boot is not putting a predecessor's run back, and it stays interrupted: {}{}",
            sprag_host::plugins::withheld_sentence(&run.why),
            leftover,
        );
    }
    for run in inheritance.resumed {
        // ⛔⛔⛔⛔⛔ **FIRST, END WHAT THE PREDECESSOR LEFT DRIVING THIS RUN** — register item 526,
        // and it is the difference between a promotion and two processes typing at one agent.
        //
        // A driver is a process of its own (register item 544) and outliving its daemon is a
        // property that item's stage 1 built ON PURPOSE, so somebody else's loop is not stopped
        // mid-step by a build swap that was not theirs. What that does NOT survive is the run's
        // ANSWER: a driver reports its outcome on the stdout pipe of the process that spawned it,
        // and that process is gone — so a leftover driver can finish hours of work that nobody
        // will ever be able to read, while this boot puts the same run back on a second driver
        // beside it. **Measured 2026-08-25 before this existed: five `sprag-term` against one
        // socket for two loops, where three is right.**
        //
        // ⚠⚠ So the LOG is the channel that survives a restart, and the run comes back through it.
        // The leftover process does not: it is ended here, deliberately and not as tidying.
        //
        // ⚠ THROUGH THE SAME DOOR AS THE LOOP ABOVE — register item 740. The kill and its bounded
        // wait were spelled here and nowhere else, which is why the withheld arm did not have them;
        // `end_leftover_driver` carries both, and there is now one place to read what a boot does
        // to a process it did not start.
        if let Some(pid) = run.driver
            && end_leftover_driver(pid, &endpoint)
        {
            tracing::warn!(
                target: "sprag_host::runs",
                run = run.id.0,
                driver = pid,
                "ended the driver process this daemon's predecessor left behind, so the run is \
                 put back on one driver rather than two",
            );
        }
        // ⛔⛔⛔⛔⛔ **THE PANE THE RUN IS ON, WHICH IS NOT ALWAYS THE PANE IT WAS ASKED OVER** —
        // register item 771. `InheritedRun::pane` prefers what the run's own driver last reported
        // over the request's number, because a loop REPLACES its inner session as it works and
        // every replacement is a new pane. Reading the request alone put a run that had moved back
        // over a pane that had closed — or, as measured, refused to put it back at all.
        let Some(pane) = run.pane() else {
            not_resumed(runs, &run, &sprag_host::runs::NotResumed::NoPane);
            continue;
        };
        let held = host
            .registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pool_holding(pane);
        let Some(home) = held else {
            not_resumed(
                runs,
                &run,
                &sprag_host::runs::NotResumed::PaneGone {
                    pane: pane.0,
                    // ⚠ WHICH OF THE TWO THIS IS, because the remedies differ — see
                    // `plugins::not_resumed_sentence`. `drove` is `Some` exactly when the run's own
                    // driver reported a pane, so this is that fact and not a guess from the number.
                    reported: run.drove.is_some(),
                },
            );
            continue;
        };
        // ⚠⚠ THE SAME SURFACE A REQUEST GETS, built by the same function — see
        // `sprag_host::plugin_host`. A second assembly here could resume a run on a thread inside
        // a daemon configured to drive runs in processes of their own.
        let plugins = sprag_host::plugin_host(
            home.pool,
            runs,
            &home.session,
            // ⚠⚠ THE WINDOW THE SAME WALK FOUND — register item 690. A run put back on a driver
            // needs the address a fresh one gets, and the pane's window is exactly as much a part
            // of that address as its session: a resumed driver told no window would follow the
            // session's current one and die the moment somebody looked elsewhere.
            &home.window,
            channels,
            &sprag_host::DaemonShared {
                on_pane_exit: Some(Arc::clone(on_pane_exit)),
                attention: Some(Arc::clone(attention)),
                agents: Some(Arc::clone(agents)),
                spawn_driver: spawn_driver.clone(),
                // ⚠ AND WHERE A RESUMED RUN'S ASKER SITS — register item 689. A run this daemon
                // inherited is put back through the SAME surface a request gets, so it must be
                // given the same reach: an inherited run whose asker came back in another window
                // would otherwise be put back as belonging to no conversation.
                seats_elsewhere: Some(sprag_host::seats_of(Arc::clone(host.registry()))),
                // ⚠⚠ AND WHERE ITS PANE WENT — register item 682, on the line above's argument: a
                // run this daemon inherited is put back through the SAME surface a request gets,
                // so a pane that moved windows while the predecessor was dying must not turn the
                // restored run into one that dies on its first injection.
                panes_elsewhere: Some(sprag_host::pools_of(Arc::clone(host.registry()))),
                ..sprag_host::DaemonShared::none()
            },
        );
        // ⛔⛔⛔⛔⛔ `Boot` — register item 774, and it is a statement of fact about THIS function's
        // own caller: `host.restore` ran a hundred lines up and re-spawned every pane in the
        // snapshot, so whatever each of these runs was watching is gone with the process that was
        // doing it. A run put back at `working` without this waits for a turn nobody started.
        match plugins.put_back(&run, sprag_plugin::Resumed::Boot) {
            Ok(()) => tracing::info!(
                target: "sprag_host::runs",
                run = run.id.0,
                pane = pane.0,
                "put a run this daemon inherited back where its log said: {}",
                run.label,
            ),
            // ⛔⛔⛔⛔⛔ AND THE REFUSAL GOES TO THE ROW AS WELL AS TO THIS LOG — register item 771,
            // the third and last way out of this loop. `put_back` refuses a plugin word this build
            // no longer spells, a guardrail it cannot parse and a machine it will not place, and
            // every one of those left the row saying `interrupted` and nothing else.
            // ⚠⚠⚠⚠⚠ **`Display` AND NOT `Debug`, BECAUSE THIS ONE REACHES A PERSON.** `InvokeError`
            // carries a `Display` written for exactly this (pinion's R1699: *"`Debug` is what eight
            // call sites across three screens were using to put a refusal in front of somebody, and
            // `Debug` is Rust syntax"*), and the sibling line above it — an operator's log — uses
            // `{why:?}` legitimately, because there the variant name IS the useful half. Rendered
            // once, here, so the row and this boot's log carry the same sentence.
            Err(why) => not_resumed(
                runs,
                &run,
                &sprag_host::runs::NotResumed::Refused(why.to_string()),
            ),
        }
    }
}

/// ⛔⛔⛔⛔⛔ **SAY, ONCE, WHY A RUN THIS BOOT MEANT TO PUT BACK IS STAYING INTERRUPTED** — register
/// item 771, and the one exit from [`put_back_inherited_runs`]'s resume loop.
///
/// # ⚠⚠⚠⚠⚠ It writes to BOTH mouths, which is the whole of the item
///
/// The operator's log is read by whoever was watching the terminal the daemon was restarted in, and
/// **a promotion exists so that nobody has to be**. The person who comes back to `sprag runs` is the
/// one who has to decide whether to start the loop again — register item 744's class, and the same
/// argument the withheld loop above already takes for its own two facts. Before this, all three
/// exits below wrote to the log alone.
///
/// ⚠ One spelling for both, composed by `plugins::not_resumed_sentence` — `withheld_sentence`'s
/// rule: two compositions of one refusal are free to drift into disagreeing about one run.
fn not_resumed(
    runs: &Arc<Mutex<RunRegistry>>,
    run: &sprag_host::runs::InheritedRun,
    why: &sprag_host::runs::NotResumed,
) {
    let said = sprag_host::plugins::not_resumed_sentence(why);
    if !runs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .not_resumed(run.id, why.clone())
    {
        // ⚠ THE DOOR'S REFUSAL IS SAID RATHER THAN DISCARDED — the withheld loop's rule above. It
        // can only answer `false` if this registry no longer holds the run `inheritance()` handed
        // over a moment ago, or holds it with a withheld reason that would contradict this one:
        // either way the two have stopped agreeing, and the row is about to not say why it stayed.
        tracing::warn!(
            target: "sprag_host::runs",
            run = run.id.0,
            "this boot could not put a run back and could not record why against it, so the row \
             will say `interrupted` and nothing else: {said}",
        );
    }
    tracing::warn!(
        target: "sprag_host::runs",
        run = run.id.0,
        label = %run.label,
        "this boot could not put a predecessor's run back, and it stays interrupted: {said}",
    );
}

fn spawn_durability_saver(
    registry: Arc<Mutex<SessionRegistry>>,
    path: PathBuf,
    history_dir: PathBuf,
    history_limits: HistoryLimits,
    runs: Option<(PathBuf, Arc<Mutex<sprag_host::runs::RunRegistry>>)>,
) {
    thread::spawn(move || {
        // `save_if_changed` / `save_histories_if_changed` own the write-if-changed dedup (both
        // tested in their own modules); the loop is just them on a timer, carrying what was last
        // saved between ticks.
        let mut last: Option<Snapshot> = None;
        let mut last_histories: HashMap<PaneId, SavedHistory> = HashMap::new();
        let mut last_runs: Option<sprag_host::runs::RunLog> = None;
        loop {
            thread::sleep(SNAPSHOT_INTERVAL);
            if let Err(e) = save_if_changed(&path, &registry, &mut last) {
                tracing::warn!(
                    target: "sprag_host::durability",
                    "snapshot save to {} failed: {e}",
                    path.display()
                );
            }
            // ⚠ A THIRD INDEPENDENT HALF. It rides the same tick because there is no reason for a
            // second timer, and it fails on its own terms: an unwritable run log must not cost the
            // workspace its shape, exactly as an unwritable history must not.
            if let Some((path, registry)) = runs.as_ref()
                && let Err(e) = sprag_host::save_runs_if_changed(path, registry, &mut last_runs)
            {
                tracing::warn!(
                    target: "sprag_host::durability",
                    "run log save to {} failed: {e}",
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
/// # Why a sweep is needed at all
///
/// A candidate is created by an OBSERVATION, and until slice 3 the only thing that observed was the
/// pane-list query. So a daemon nobody had ever queried held no state for any pane — measured, not
/// reasoned: a first-ever one-shot read of an agent pane on a five-second-old daemon answered nothing,
/// because the read was itself that pane's first observation and a resting verdict has to hold. That
/// makes "ask once and get an answer" false for exactly the caller slice 5 is FOR: an MCP or CLI peer
/// on a daemon with no attached frontend.
///
/// The fix is not to poll the panes. [`sprag_host::AgentRegistry::owes_evaluation`] answers `false` for a settled
/// pane under unchanged rules, which is every pane in a quiet workspace, so the screen read behind it
/// never happens. What recurs is the walk.
///
/// # Cost — MEASURED (R260), where this paragraph used to argue
///
/// With nothing pending the thread is BLOCKED, not spinning, and wakes on [`SWEEP_INTERVAL`] to
/// discover new panes, carry a manifest edit and bound memory. With something pending it wakes at that
/// deadline: one wake per transition.
///
/// One sweep over a quiet workspace, on `sprag-latency`'s rows (i9-14900HX, `--release`, five runs,
/// minima): **1.96-2.33 us at one pane, 2.50-2.95 us at eight, 7.01-8.38 us at sixty-four** — against
/// a five-second period, 0.00014% of one core at the top end. The terms:
///
/// * per PANE, 0.039-0.043 us to ask whether it owes an evaluation, flat across a 64x span
///   (-0.02 to +0.06 ns per remembered pane) because the question is three hash lookups and no walk;
/// * per WAKE, twice, `any_due` over every tracker — 0.021-0.024 us at one pane and 0.082-0.094 us at
///   sixty-four, about 1.0 ns per tracker visited, which is the same order as the 1.35-1.68 ns per
///   visit R255 inferred from a different row. Twice because the park chooses a sleep from it and a
///   candidate appearing can cut that sleep short, so the answer cannot be carried across;
/// * per SWEEP, the census (1.93-2.25 us at sixty-four panes), the prune it feeds (0.64-0.74 us), and
///   the manifest re-read (1.84-2.20 us with a file, 0.58-0.66 us with none).
///
/// **Two things this paragraph got wrong while it was an argument, both now measured.** It said what
/// recurs is "a pane-id read each", which named the smallest per-pane term and omitted the clock lock
/// the walk takes ONCE PER PANE. And it priced the whole thread as marginal against
/// [`spawn_durability_saver`] — "the same locks at the same interval, and that one does strictly
/// more". The locks are genuinely shared. The manifest re-read is not: R254 moved the reload onto this
/// thread and priced its SCHEDULING (no new thread, no timer, no wake), which is true and is a
/// different claim, while the saver reads no file at all — it writes when the shape moved and is
/// otherwise silent. At one pane that unshared term is **94%** of the sweep, and at sixty-four it is
/// still 26%. Nothing here asks to be changed; what asked to be changed was the paragraph.
///
/// [`sprag_host::AgentRegistry::retain_live`]'s "the census is a by-product of work it is doing anyway" is true
/// about the WALK and not about the cost: building the live set is 2.9x to 3.0x the prune it serves.
///
/// Lock discipline is [`spawn_durability_saver`]'s, for the same reason: the registry lock is taken
/// and RELEASED to clone out the pools, then each workspace lock is taken on its own, never nested.
/// The clock's lock is taken inside a workspace lock (the screen is only reachable there) and never
/// the other way round.
///
/// What those locks cost was R260's one open term and is now measured (R261, on
/// [`sprag_host::sweep_once`]): with the pass running at seven to twelve MILLION times this
/// interval, a concurrent pane-list reader's median moves +0.4 to +0.8 us against a control doing
/// the same work on a registry it does not share. The recurring pass is free. The pass after a
/// manifest reload is not the same object — 44 to 58 us for three panes — and the reason it is
/// documented rather than redesigned is on `sweep_once`.
fn spawn_agent_waker(
    registry: Arc<Mutex<SessionRegistry>>,
    agents: Arc<AgentClock>,
    channels: Arc<ChannelRegistry>,
    mut manifests: sprag_host::config::AgentManifests,
) {
    thread::spawn(move || {
        let mut last_sweep = Instant::now();
        // The foreground-job watch is this thread's alone and outlives no pass but every one of
        // them — which is the whole of what it is for: a change is only visible against a reading
        // the daemon already had. Constructed here rather than passed in because nothing else in
        // the process reads it; the day something does, it moves out to where the clock lives.
        let jobs = JobWatch::new();
        loop {
            // Blocked until there is something to do. Returns early when a candidate APPEARS, which is
            // not itself work — the guard below sends that wake straight back to the park.
            agents.park_until_due(SWEEP_INTERVAL);
            let now = Instant::now();
            // Two reasons to do work on a wake that is not a sweep. A deadline has come due — the
            // clock publishing a verdict nothing else would confirm — or a pane has been RELEASED
            // from a report and its published state is one nobody stands behind any more. The second
            // is the arrival `AgentClock::observe`'s docs anticipated: work asked for by another
            // thread, which needs both the signal (`AgentClock::release` sends it) and a reason to act
            // when nothing is due, which is this.
            let owed = agents.with(|state| state.any_due(now) || state.any_owes_look());
            let sweep = now.duration_since(last_sweep) >= SWEEP_INTERVAL;
            if !owed && !sweep {
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
                let replaced = manifests.refresh();
                adopt_manifests(&agents, &manifests, replaced);
            }
            // The pass itself is `sprag_host::sweep_once` — every input it reads is a library type,
            // so what is left here is the scheduling and nothing else. It also has to be callable:
            // R261 measured what its locks cost by running it against a live registry while another
            // thread served requests, which is not a thing a closure in this file can be asked to do.
            // ⚠⚠ WHERE A MUTE BREADCRUMB IS LOOKED FOR AND WHOSE IT WOULD HAVE TO BE, said out loud
            // here because a pass that derived it would be asserting whatever host and whichever
            // daemon generation it ran under (register items 700 and 711). The generation is THIS
            // process's — the daemon is the party that minted the pane numbers these files are keyed
            // on, so it needs no wire hop to know which generation they mean.
            let state_dir = sprag_host::durability::state_dir();
            let mute =
                sprag_host::MuteReader::new(&state_dir, Some(sprag_host::wire::generation()));
            sweep_once(&registry, &agents, &jobs, &channels, &mute, now, sweep);
        }
    });
}

/// Publish what the user's manifest file says into the clock every client reads: the ruleset in
/// force when a re-read REPLACED it, and why that ruleset is not the user's whether or not it did.
///
/// One function because the two halves come from one act of reading and are published under one
/// lock. `replaced` is [`AgentManifests::refresh`](sprag_host::config::AgentManifests::refresh)'s
/// own answer at the sweep, and `false` at boot, where the clock was constructed from these rules
/// already.
///
/// The report is NOT inside the `replaced` branch, and that is the point of writing this out: a
/// broken edit replaces nothing — `refresh` answers `false` and keeps the last list that worked —
/// which is exactly the edit a user needs told about. Publishing it beside the reload would have
/// covered every case except the one the report exists for.
///
/// Rendered here, at the end that knows the file is `config.toml`, for
/// [`sprag_host::HostClient::global_commands`]'s reason: a client re-rendering a
/// [`ConfigError`](sprag_host::ConfigError) at the far end of the wire names a file it had to guess.
fn adopt_manifests(
    agents: &AgentClock,
    manifests: &sprag_host::config::AgentManifests,
    replaced: bool,
) {
    agents.with(|state| {
        if replaced {
            state.reload(manifests.rules().clone());
        }
        state.set_manifest_report(manifests.unusable().map(ToString::to_string));
    });
}

/// Install SIGINT/SIGTERM graceful shutdown: on the first such signal, cancel
/// and join in-flight plugin runs (so a slow AI turn aborts and its worker
/// threads reap; the pane shells receive SIGHUP when our PTY masters close on
/// exit) then exit. Non-fatal if the handler cannot be installed -- the process
/// then just terminates on the signal, as default.
///
/// ⚠⚠ THE JOIN IS BOUNDED, and this is the path that number is FOR: a person who signalled the
/// daemon has to get their prompt back even when a worker will not come back, and this handler is
/// the last code that runs before the process ends. What the deadline costs is
/// [`RunRegistry::stop_all_within`]'s to state.
///
/// ⛔⛔⛔⛔⛔ **THE TWO LINES THAT MATTERED ARE ONE CALL NOW** — register item 664. This used to hold
/// the registry lock, `cancel_all` it, and join under that lock, and both halves of that were
/// wrong for a run driven in a process of its own: the sweep published nothing, so the driver was
/// never woken; and had it been, the row it would have come back to read was behind the very lock
/// this handler was sitting on. `stop_all_within` holds the ask-then-wait ORDER where it can be
/// gated, and takes the lock per pass so the daemon can still answer the question it just asked.
fn install_shutdown(runs: Arc<Mutex<RunRegistry>>) {
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) {
        Ok(signals) => signals,
        Err(_) => return,
    };
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = RunRegistry::stop_all_within(&runs, RunRegistry::JOIN_DEADLINE);
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
