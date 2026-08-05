//! **THE gate for the terminal-client front**: a real `sprag-tui` binary, on a real pseudoterminal,
//! against a real `sprag-term` daemon — typed into, resized, and detached from.
//!
//! Every other test in this crate is a pure function checked in isolation, and slice 2 proved why
//! that is not enough: nineteen of them were green around a binary that could not attach at all,
//! because a PTY with no winsize reports `0x0` and nothing that never ran a process could see it.
//! What runs here is the shipped artifact, spawned the way a user spawns it, talking to the daemon
//! over the same socket.
//!
//! # What each test actually proves
//!
//! The claim of this front is that a session can be attached over ssh. Broken into what can be
//! observed from outside the client:
//!
//! * **Typing works.** A keystroke written to the PTY master reaches the CHILD PROCESS in the pane,
//!   and what the child does about it comes back and is painted. The client is in raw mode, so its
//!   terminal echoes nothing — every character that appears on the master travelled the whole loop
//!   through the daemon. That is the strongest form of this assertion available and it is why the
//!   test types into `cat`: an echo is a round trip, unambiguously.
//! * **Resizing works.** A window change reaches the pane's PTY, asserted by asking the DAEMON what
//!   size the pane is rather than by looking at the screen. A screen assertion would pass on a
//!   client that merely repainted at the new size while leaving the child on the old winsize —
//!   which is exactly the bug this half is for.
//! * **Detaching works.** The prefix table ends the client and the session survives it, which is
//!   the difference between a multiplexer and a terminal emulator.
//!
//! # How the screen is read
//!
//! The bytes the client writes are ESCAPE SEQUENCES, and diffed ones at that: `termwiz`'s surface
//! sends the smallest update it can, so a word typed one letter at a time never appears as one
//! contiguous string on the wire. Asserting on the raw stream would be asserting on the diffing
//! algorithm. So the master's output is fed to a real [`Emulator`] and the assertions are made on
//! the resulting SCREEN — the same "assert the cells, not the bytes" discipline the VT battery
//! uses, and a meeting of two independent implementations rather than a circle: termwiz's terminfo
//! renderer writes the sequences, and sprag's own emulator decides what they mean.
//!
//! # Waiting
//!
//! Every wait here polls the CONDITION the assertion reads, never a duration. Three processes and
//! two sockets sit between a keystroke and a painted cell, so a fixed sleep would be either a flake
//! or a slow test, and usually both on different machines.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{
    Child as PtyChild, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};
use serde_json::{Value, json};
use sprag_host::wire::{
    FULL_TEXT_SLOT, LAYOUT_SLOT, NEW_SESSION_ACTION, PANES_SLOT, RENAME_SESSION_ACTION,
    SELECT_PANE_ACTION, SESSIONS_SLOT, SPLIT_ACTION, WINDOWS_SLOT,
};
use sprag_host::{mux_action_path, pane_input_path};
use sprag_rpc::HostConn;
use sprag_vt::{Emulator, InputModes, MouseProtocol, VtPort};

/// How long any single condition may take before the test calls it a failure.
///
/// Generous on purpose: it is a deadline for a HANG, not a guess at how long the work takes. Every
/// wait returns the instant its condition holds, so raising this costs a passing run nothing and
/// only buys a loaded machine room to finish.
///
/// **15s was not generous, and it was measured rather than argued.** The slowest legitimate wait in
/// this file is `a_click_in_the_second_pane_arrives_in_that_panes_own_columns`, which types an
/// 88-character command into a pane's shell and waits for the echo: 9–13 seconds in ISOLATION, on a
/// quiet machine, of a 15-second cap. It passed only because it happened to fit. Adding tests to this
/// binary — which run in parallel — pushed it over, so a green suite turned amber for a reason that
/// had nothing to do with what any of the tests assert.
///
/// The cost is in one place, and it is worth recording where: typed input is delivered at roughly
/// **100–124ms per character** in bulk (measured with a throwaway probe: 1 char 222ms, 10 chars
/// 384ms, 20 chars 2.1s, 40 chars 5.0s), so 88 keystrokes are ~8 of that test's ~10 seconds. That is
/// a real property of the input path rather than of this harness, and it is the thing to fix; a
/// deadline is not the place to hold the line on it. Until then this is set where a HANG is still
/// caught promptly and a slow-but-working wait is not called a failure.
const DEADLINE: Duration = Duration::from_secs(45);

/// How often a condition is re-checked while waiting.
const POLL: Duration = Duration::from_millis(20);

/// The size the client's terminal boots at — deliberately DIFFERENT from the size the daemon gives
/// its boot pane, so "the pane matched the terminal" cannot be true by accident.
const BOOT_PTY: (u16, u16) = (80, 24);

/// The size the daemon's boot pane starts at, which no test expects to still hold once a client has
/// attached to it.
const BOOT_PANE: (u16, u16) = (40, 6);

// ----- the daemon -----

/// Kills and reaps the spawned daemon on scope exit (including a test panic), and unlinks its
/// socket so a failed run leaves no file behind either. The kill comes first: the daemon holds the
/// socket open until it exits.
///
/// A near-copy of `sprag-host`'s own `HostChild`, and deliberately not shared with it: they are
/// different packages, and exporting a test harness from a library — or adding a third crate to
/// hold twenty lines — would cost more than the copy does.
struct Daemon(Child, PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// The `sprag-term` binary this test drives: the sibling of the `sprag-tui` cargo built for it.
///
/// Cargo sets `CARGO_BIN_EXE_*` only for binaries of the package under test, and the daemon belongs
/// to `sprag-host` — so its path is derived rather than given, and its ABSENCE is a loud failure
/// rather than a skip. A skipped gate is a green tick over an untested claim, which is the failure
/// mode this whole file exists to prevent.
fn sprag_term_bin() -> PathBuf {
    sibling_bin("sprag-term")
}

/// The `sprag` management CLI, for the one test whose subject is how that CLI LAUNCHES this client
/// (`sprag attach --tui`) rather than what the client then does.
fn sprag_cli_bin() -> PathBuf {
    sibling_bin("sprag")
}

/// A `sprag-host` binary beside the `sprag-tui` cargo built for this test.
///
/// Cargo sets `CARGO_BIN_EXE_*` only for binaries of the package under test, and these belong to
/// `sprag-host` — so the path is derived rather than given, and its ABSENCE is a loud failure
/// rather than a skip. A skipped gate is a green tick over an untested claim, which is the failure
/// mode this whole file exists to prevent.
fn sibling_bin(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_sprag-tui"))
        .parent()
        .expect("the built sprag-tui has a directory")
        .join(name);
    assert!(
        path.exists(),
        "{} is not built. This test drives a binary that belongs to another package, so cargo \
         does not build it for `-p sprag-tui` alone — run `cargo test --workspace`, or \
         `cargo build -p sprag-host --bin {name}` first.",
        path.display(),
    );
    path
}

/// A socket path unique to this CALL, under the temp dir.
///
/// The counter is load-bearing: `cargo test` runs this file's tests as parallel threads of one
/// binary, so a path keyed only on the pid would be the same string in every test, and each test
/// unlinks its path before spawning — i.e. removes the socket a sibling is serving on.
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-tui-pty-{}-{n}.sock", std::process::id()))
}

/// Spawn a daemon whose boot pane runs `program`.
///
/// Usually `cat` — an echo that keeps its PTY open, so a keystroke that arrives comes straight back
/// and a pane that is idle stays alive. The mouse tests pass something else, because their claim
/// needs a child that ASKS for tracking and `cat` never does.
fn spawn_daemon_running(program: &[&str]) -> (Daemon, PathBuf) {
    spawn_daemon_with_config(program, None)
}

/// The same daemon, reading `config` as the user's config home.
///
/// `window-size` is a DAEMON-side option (like `default-command`), so a test about it has to point
/// the DAEMON at the file — pointing only the clients there would leave the arbitration reading
/// whatever config the machine running the suite happens to have.
fn spawn_daemon_with_config(program: &[&str], config: Option<&str>) -> (Daemon, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(sprag_term_bin())
        .arg("--size")
        .arg(format!("{}x{}", BOOT_PANE.0, BOOT_PANE.1))
        .arg("--")
        .args(program)
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .envs(config.map(|home| ("XDG_CONFIG_HOME", home)))
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn the sprag-term daemon");
    (Daemon(child, sock.clone()), sock)
}

// ----- the wire, as an observer -----

/// A request connection to the daemon, for the assertions the SCREEN cannot make.
fn observe(sock: &Path) -> HostConn {
    HostConn::connect(sock, DEADLINE).expect("connect to the daemon socket")
}

/// One registry-wide `sessions` row, by name.
fn session_row(conn: &mut HostConn, name: &str) -> Option<Value> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )
    .ok()?
    .as_array()?
    .iter()
    .find(|row| row["name"].as_str() == Some(name))
    .cloned()
}

/// The name of the daemon's one boot session, read off the wire rather than assumed — the naming
/// rule for a session nobody named belongs to the daemon, not to this test.
fn boot_session(conn: &mut HostConn) -> String {
    let sessions = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(SESSIONS_SLOT) }),
        )
        .expect("the sessions slot answers");
    let rows = sessions.as_array().expect("sessions is a list");
    assert_eq!(rows.len(), 1, "one boot session: {sessions}");
    rows[0]["name"]
        .as_str()
        .expect("a session has a name")
        .to_owned()
}

/// How many clients the daemon counts as attached to `session`. An unattached session omits the
/// field, which reads back as 0.
fn attached(conn: &mut HostConn, session: &str) -> u64 {
    session_row(conn, session).map_or(0, |row| row["attached"].as_u64().unwrap_or(0))
}

/// The `(cols, rows)` of `session`'s first pane, as the DAEMON reports it — the authority on what
/// the child's winsize is, which no amount of looking at the client's screen can establish.
fn pane_size(conn: &mut HostConn, session: &str) -> Option<(u16, u16)> {
    let panes = conn
        .call(
            "scene/query",
            json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
        )
        .ok()?;
    let pane = panes.as_array()?.first()?.clone();
    let dim = |key: &str| u16::try_from(pane[key].as_u64()?).ok();
    Some((dim("cols")?, dim("rows")?))
}

/// The `(cols, rows)` of EVERY pane of `session`, in the daemon's own order.
///
/// The multi-pane assertion, and it is made against the daemon rather than the screen for the same
/// reason [`pane_size`] is: a client that tiled its surface correctly while telling both children
/// they still had the whole terminal would paint a picture that looks right and wrap every line in
/// the wrong column.
fn pane_sizes(conn: &mut HostConn, session: &str) -> Vec<(u16, u16)> {
    let Ok(panes) = conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    ) else {
        return Vec::new();
    };
    panes
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|pane| {
                    let dim = |key: &str| u16::try_from(pane[key].as_u64()?).ok();
                    Some((dim("cols")?, dim("rows")?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The pane the DAEMON says `session`'s current window is on — the fact a directional key moves,
/// and the one no client is the author of.
///
/// Read from the `panes` slot rather than from the client's screen on purpose: the claim a
/// `select-pane -L` test has to make is that SESSION state moved, and a painted focus ring is this
/// client's projection of that. [`None`] while no row carries the flag.
fn active_pane(conn: &mut HostConn, session: &str) -> Option<u64> {
    let panes = conn
        .call(
            "scene/query",
            json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
        )
        .ok()?;
    panes
        .as_array()?
        .iter()
        .find(|pane| pane["active"] == json!(true))
        .and_then(|pane| pane["id"].as_u64())
}

/// The ids of `session`'s panes, in the daemon's own order — what a caller naming a split's target
/// needs, and a fact only the host has.
fn pane_ids(conn: &mut HostConn, session: &str) -> Vec<u64> {
    let Ok(panes) = conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    ) else {
        return Vec::new();
    };
    panes
        .as_array()
        .map(|rows| rows.iter().filter_map(|pane| pane["id"].as_u64()).collect())
        .unwrap_or_default()
}

/// The tracking level the DAEMON reports for `pane` (`"click"` / `"button"` / `"any"`), or [`None`]
/// while its child is asking for nothing.
///
/// The per-pane authority, which is what a test about WHICH pane a report reaches has to wait on: a
/// display client's own mirror is the MAXIMUM over the panes, so with two tracking children it says
/// nothing about either one of them.
fn pane_mouse(conn: &mut HostConn, session: &str, pane: u64) -> Option<String> {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()?
    .as_array()?
    .iter()
    .find(|entry| entry["id"].as_u64() == Some(pane))?
    .get("mouse")?
    .as_str()
    .map(str::to_owned)
}

/// What the DAEMON says pane 0 of the session holds — scrollback and visible text together.
///
/// The other half of every screen diagnostic: a client painting nothing and a pane holding nothing
/// look identical from the client's terminal, and they are opposite bugs.
fn pane_text(conn: &mut HostConn, session: &str) -> String {
    pane_text_of(conn, session, 0)
}

/// [`pane_text`] for a pane named by ID — what a test that SPLIT needs, since the pane it created
/// is not pane 0 and reading pane 0 would answer about the wrong child entirely.
fn pane_text_of(conn: &mut HostConn, session: &str, pane: u64) -> String {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": pane_input_path(pane, FULL_TEXT_SLOT) }),
    )
    .ok()
    .and_then(|value| value.as_str().map(str::to_owned))
    .unwrap_or_else(|| "<unreadable>".to_owned())
}

/// Wait until THIS CLIENT routes what is typed into `pane`, by sending one `.` at a time until one
/// more of them is on that pane's screen than was there before.
///
/// **The daemon's `active` flag moving is NOT this.** A client routes keystrokes through its own
/// mirror and adopts a move on its next wake, so a marker typed straight after a directional key can
/// still be in flight to the pane the client had not left yet. Observed rather than slept through,
/// because the wake is a wake and not a duration.
///
/// It COUNTS rather than looking for a dot, and that is the difference between a check and a
/// decoration: a shell prompt may already hold one (a hostname, a path), so "there is a dot there"
/// can be true before anything at all has landed.
///
/// Without this, `the_arrow_keys_walk_the_arrangement_and_stop_at_its_edge` failed once under a full
/// workspace load with pane 0 holding `"beforeed"` — the first five characters of its marker had gone
/// to the pane the client was still routing to. R297 wrote the wait against the DAEMON's fact; the
/// sibling test one screen down had already recorded the mechanism ("the client learns of the select
/// on its next wake") and solved it with one character typed until it lands.
fn typing_follows(tui: &mut Tui, conn: &mut HostConn, session: &str, pane: u64) {
    let before = pane_text_of(conn, session, pane).matches('.').count();
    wait_for(
        &format!("this client's typing to follow the session onto pane {pane}"),
        || {
            tui.type_bytes(b".");
            let held = pane_text_of(conn, session, pane);
            if held.matches('.').count() > before {
                Ok(())
            } else {
                Err(format!("pane {pane} holds {held:?}"))
            }
        },
    );
}

/// Poll `observe` until it reports the condition holds, or fail after [`DEADLINE`] naming `what`
/// and the LAST thing that was there instead.
///
/// One closure that both tests and describes, rather than a condition plus a diagnostic: the two
/// would need the same `&mut` connection, and the observation worth printing is the last one taken
/// rather than a fresh one made after the fact. The diagnostic is not decoration — three processes
/// stand between an act and its observation, so "timed out" alone cannot tell a client that painted
/// the wrong thing from one that painted nothing from one that never started.
fn wait_for(what: &str, mut observe: impl FnMut() -> Result<(), String>) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = "nothing was observed at all".to_owned();
    while Instant::now() < deadline {
        match observe() {
            Ok(()) => return,
            Err(state) => last = state,
        }
        std::thread::sleep(POLL);
    }
    panic!("timed out after {DEADLINE:?} waiting for {what}\n  last observation: {last}");
}

/// `Ok` when `got` is what was wanted, else `got` rendered as [`wait_for`]'s diagnostic.
fn settled<T: PartialEq + std::fmt::Debug>(got: T, want: &T) -> Result<(), String> {
    if got == *want {
        Ok(())
    } else {
        Err(format!("{got:?}"))
    }
}

/// `Ok` when the client's top row reads `want`, else the WHOLE painted screen as the diagnostic.
///
/// The whole screen, not the row that failed, because the three ways this assertion fails look
/// identical from one row: the client painted something else, the client painted nothing, or the
/// client is GONE — and a client that exited has left the alternate screen, so every row reads
/// blank rather than stale. Only the full picture separates them.
fn painted(tui: &mut Tui, want: &str) -> Result<(), String> {
    if tui.row(0) == want {
        return Ok(());
    }
    Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
}

// ----- the client, on a pseudoterminal -----

/// A live `sprag-tui` on a pseudoterminal, plus everything needed to drive it and to see what it
/// painted.
struct Tui {
    /// Held so the pty stays open and can be resized; dropping it would EOF the client's input.
    master: Box<dyn MasterPty + Send>,
    /// The client's input end. Held for the same reason: a dropped writer is an EOF.
    writer: Box<dyn Write + Send>,
    child: Box<dyn PtyChild>,
    /// Everything the client has written, as the emulator that consumed it — see the module docs
    /// for why the assertions read a screen and not a byte stream.
    screen: Arc<Mutex<Emulator>>,
    /// How many bytes the client has written, which is how [`Tui::holds_the_terminal`] knows it is
    /// safe to type.
    written: Arc<AtomicUsize>,
}

impl Drop for Tui {
    fn drop(&mut self) {
        // A test that failed before detaching leaves a client attached to a daemon that is about to
        // be killed; ending it here keeps a failure from stranding a process on the machine.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tui {
    /// Start `sprag-tui` on a fresh pseudoterminal, attached to `session` on `sock`.
    fn attach(sock: &Path, session: &str) -> Self {
        Self::attach_with_env(sock, session, &[])
    }

    /// [`Tui::attach`] with EXTRA env vars — the keymap test points `XDG_CONFIG_HOME` at a config it
    /// wrote, because a keymap the BINARY reads at startup is the one thing a unit test over
    /// `command()` cannot prove.
    fn attach_with_env(sock: &Path, session: &str, envs: &[(&str, &str)]) -> Self {
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sprag-tui"));
        command.env("SPRAG_GUI_HOST_SOCK", sock);
        command.env("SPRAG_GUI_SESSION", session);
        for (key, value) in envs {
            command.env(key, value);
        }
        Self::start(command)
    }

    /// The same client, reached the way a USER reaches it: `sprag attach --tui SESSION`.
    ///
    /// The session and the socket are NOT set here — naming them is the CLI's job, and a test that
    /// pre-set them would pass whether or not the CLI passed anything on. What is set is
    /// `SPRAG_TUI_BIN`, so the `sprag` under test launches the `sprag-tui` under test rather than
    /// an installed one, and `SPRAG_HOST_RPC_SOCK`, so the CLI's own pre-flight reaches this
    /// test's daemon.
    fn attach_via_cli(sock: &Path, session: &str) -> Self {
        let mut command = CommandBuilder::new(sprag_cli_bin());
        command.args(["attach", session, "--tui"]);
        command.env("SPRAG_HOST_RPC_SOCK", sock);
        command.env("SPRAG_TUI_BIN", env!("CARGO_BIN_EXE_sprag-tui"));
        Self::start(command)
    }

    /// Put `command` on a fresh pseudoterminal and start reading what it paints.
    fn start(mut command: CommandBuilder) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                cols: BOOT_PTY.0,
                rows: BOOT_PTY.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open a pseudoterminal");

        // Hermetic: if the connect ever failed, the client would spawn a daemon of its own, and it
        // must be THIS build's rather than whatever is on the tester's PATH.
        command.env("SPRAG_GUI_HOST_BIN", sprag_term_bin());
        // The client loads terminfo from `TERM`; naming one keeps the sequences it writes
        // independent of the terminal the test suite happens to be running in.
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn sprag-tui on the pty");
        // The child holds the slave now; drop ours so the master reads EOF when it exits.
        drop(pair.slave);

        let screen = Arc::new(Mutex::new(Emulator::new(BOOT_PTY.0, BOOT_PTY.1)));
        let written = Arc::new(AtomicUsize::new(0));
        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("clone the pty reader");
        let writer = pair.master.take_writer().expect("take the pty writer");
        std::thread::spawn({
            let (screen, written) = (Arc::clone(&screen), Arc::clone(&written));
            move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    screen.lock().expect("the screen mutex").advance(&buf[..n]);
                    // AFTER the emulator, so a reader that sees the count move can also see
                    // everything that moved it.
                    written.fetch_add(n, Ordering::Release);
                }
            }
        });

        Self {
            master: pair.master,
            writer,
            child,
            screen,
            written,
        }
    }

    /// Whether the client has TAKEN the terminal — the edge before which nothing may be typed.
    ///
    /// **This is not fussiness, it is the difference between a passing test and a test that types
    /// into a void.** `set_raw_mode` calls `tcsetattr` with `TCSAFLUSH`, which PURGES the input
    /// queue: anything written to the master before the client got there is discarded by the
    /// kernel, silently and completely. Typing on the "the daemon counts a client" edge alone hits
    /// that window reliably — the binary attaches BEFORE it takes the terminal, on purpose, so that
    /// a failure to reach the daemon can be printed on an ordinary screen.
    ///
    /// One byte from the client is a sound witness: the first thing it writes is the mode-setting
    /// sequence `set_raw_mode` emits AFTER its `tcsetattr`, so a byte having arrived means the
    /// purge is behind us.
    fn holds_the_terminal(&self) -> bool {
        self.written.load(Ordering::Acquire) > 0
    }

    /// Type `bytes` at the client's terminal, exactly as a keyboard would deliver them.
    fn type_bytes(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to the pty");
        self.writer.flush().expect("flush the pty");
    }

    /// Resize the client's window — a real `TIOCSWINSZ`, so the client is woken by a real
    /// `SIGWINCH` rather than by anything this test told it.
    ///
    /// The reading emulator is resized to match, because it stands in for the terminal ATTACHED to
    /// this pty: a real one reshapes its own screen when its window changes, and one that did not
    /// would start disagreeing with the client about where a cell is the moment the sizes diverged.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.master
            .resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize the pty");
        self.screen
            .lock()
            .expect("the screen mutex")
            .resize(cols, rows);
    }

    /// The input modes the client put this terminal into — what it asked of a terminal it borrowed.
    fn local_modes(&self) -> InputModes {
        VtPort::input_modes(&*self.screen.lock().expect("the screen mutex"))
    }

    /// The WINDOW TITLE the client set on this terminal (`OSC 0` / `OSC 2`), `None` while it has set
    /// none — read off the emulator that consumed the escape, like every other assertion here.
    ///
    /// This is the client's agent surface (it paints no chrome of its own), so it is read the same
    /// way its cells are: the client emits the OSC, sprag's own emulator decides what it meant.
    fn title(&self) -> Option<String> {
        VtPort::title(&*self.screen.lock().expect("the screen mutex")).map(str::to_owned)
    }

    /// Every row of the client's painted screen — the failure diagnostic, never an assertion.
    fn rows(&self) -> Vec<String> {
        let emulator = self.screen.lock().expect("the screen mutex");
        let screen = VtPort::screen(&*emulator);
        (0..screen.rows())
            .map(|row| screen.row_text(row).trim_end().to_owned())
            .collect()
    }

    /// One row of the client's painted screen, trailing blanks trimmed.
    ///
    /// Trimmed because a pane narrower than the terminal leaves the rest of the row blank, and a
    /// test that asserted on the padding would be asserting on the pane's size rather than on what
    /// the child said.
    fn row(&self, row: u16) -> String {
        let emulator = self.screen.lock().expect("the screen mutex");
        VtPort::screen(&*emulator)
            .row_text(row)
            .trim_end()
            .to_owned()
    }

    /// The character painted at `(col, row)`, or `None` past the end of that row.
    ///
    /// UNtrimmed, unlike [`Tui::row`]: a divider's column is exactly what the padding question is
    /// about here, so the blanks have to still be there to count through. Sound for these
    /// assertions because everything to the left of a divider in these tests is ASCII or blank —
    /// one char per column — and a wide cluster would break the correspondence.
    fn cell(&self, col: u16, row: u16) -> Option<char> {
        let emulator = self.screen.lock().expect("the screen mutex");
        VtPort::screen(&*emulator)
            .row_text(row)
            .chars()
            .nth(usize::from(col))
    }

    /// Columns `cols` of one row, trailing blanks trimmed — one PANE's share of a row.
    ///
    /// The multi-pane form of [`Tui::row`], and the distinction is not pedantry: `row` trims from
    /// the screen's right edge, so with two panes side by side it stops at the DIVIDER and returns
    /// the left pane's text with the padding and the line still attached. Asserting a pane's content
    /// means asserting inside that pane's own columns.
    fn span(&self, row: u16, cols: std::ops::Range<u16>) -> String {
        let text: String = cols.map(|col| self.cell(col, row).unwrap_or(' ')).collect();
        text.trim_end().to_owned()
    }

    /// The column `col` read down every row of the screen — how a divider is asserted, and the
    /// diagnostic when one is not where it should be.
    fn column(&self, col: u16) -> String {
        let rows = {
            let emulator = self.screen.lock().expect("the screen mutex");
            VtPort::screen(&*emulator).rows()
        };
        (0..rows)
            .map(|row| self.cell(col, row).unwrap_or(' '))
            .collect()
    }

    /// Whether the client is still running, for a diagnostic — a client that has EXITED left the
    /// alternate screen on its way out, so its last painted frame is not what a reader sees.
    fn liveness(&mut self) -> String {
        match self.child.try_wait() {
            Ok(None) => "running".to_owned(),
            Ok(Some(status)) => format!("EXITED {status:?}"),
            Err(error) => format!("unknown ({error})"),
        }
    }

    /// Wait for the client to exit, and fail rather than block if it does not.
    ///
    /// Bounded deliberately: `Child::wait` has no deadline, so a client that failed to act on a
    /// detach would stall the whole suite instead of failing it — which is how this test first
    /// behaved, and a gate that can hang is a gate nobody will keep running.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            match self.child.try_wait().expect("wait for sprag-tui") {
                Some(status) => return status,
                None => std::thread::sleep(POLL),
            }
        }
        panic!("sprag-tui did not exit within {DEADLINE:?} of being told to detach");
    }
}

/// Spawn a daemon, attach a client to it, and wait until the attach has SETTLED.
///
/// Settled means three things, and every one of them was learned by a test that skipped it.
///
/// 1. The daemon COUNTS the client — an attach can fail outright while the process stays up (R225).
/// 2. The client HOLDS THE TERMINAL — see [`Tui::holds_the_terminal`]; before this, everything
///    typed is purged by the kernel and the test measures nothing.
/// 3. The pane has reached the terminal's size — an attach RESIZES the pane, and a test that began
///    asserting before that landed would be racing a reflow it never asked for.
fn attached_client() -> (Daemon, PathBuf, HostConn, String, Tui) {
    attached_client_with(Tui::attach, &["cat"])
}

/// [`attached_client`] with the client started by `launch` — the seam the CLI-launch test needs.
///
/// Parameterised rather than copied because the four waits below are what "attached" MEANS here,
/// and a second arrangement that settled on its own waits could differ from this one in ways that
/// look like the launcher's doing.
fn attached_client_via(launch: fn(&Path, &str) -> Tui) -> (Daemon, PathBuf, HostConn, String, Tui) {
    attached_client_with(launch, &["cat"])
}

/// [`attached_client_via`] whose boot pane runs `program` — the mouse tests need a child that ASKS
/// for tracking, and `cat` never does.
fn attached_client_with(
    launch: impl FnOnce(&Path, &str) -> Tui,
    program: &[&str],
) -> (Daemon, PathBuf, HostConn, String, Tui) {
    let (daemon, sock) = spawn_daemon_running(program);
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);
    let tui = launch(&sock, &session);
    wait_for(
        "the daemon to count the client as attached",
        || match attached(&mut conn, &session) {
            0 => Err("0 attached clients".to_owned()),
            _ => Ok(()),
        },
    );
    wait_for("the client to take the terminal", || {
        if tui.holds_the_terminal() {
            Ok(())
        } else {
            Err("the client has written nothing yet".to_owned())
        }
    });
    wait_for(
        "the attach to settle the pane at the terminal's size",
        || settled(pane_size(&mut conn, &session), &Some(BOOT_PTY)),
    );
    (daemon, sock, conn, session, tui)
}

/// The modes the client puts the LOCAL terminal into, asserted through the emulator that received
/// them rather than by matching escape bytes — the same discipline as every screen assertion here.
///
/// Two claims, and both are about what the client asked for on a terminal it does not own:
///
/// * **Bracketed paste is ON**, because the client handles the paste event it produces. A paste
///   then reaches the pane as one edit rather than as a burst of keystrokes.
/// * **Mouse reporting is OFF**, and this is the one that needs a test. `set_raw_mode` turns
///   any-event mouse reporting ON by default, and a client that captured the mouse and dropped
///   every report would silently cost the user click-drag selection and wheel scrolling in their
///   own terminal. Nothing about the screen would look wrong; only this says so.
#[test]
fn the_client_takes_the_paste_and_leaves_the_mouse() {
    let (_daemon, _sock, _conn, _session, tui) = attached_client();

    let modes = tui.local_modes();
    assert!(
        modes.bracketed_paste,
        "a paste must arrive as a paste: {modes:?}",
    );
    assert_eq!(
        modes.mouse_protocol,
        MouseProtocol::None,
        "the mouse still belongs to the user's terminal: {modes:?}",
    );
}

/// **THE agent surface for this client** (H3 slice 5): a pane whose screen says an agent is waiting
/// for an answer turns into a WINDOW TITLE on the terminal the client borrowed.
///
/// # Why this test and not a unit test over the digest
///
/// The digest itself is a pure function with its own tests. What only the shipped binary runs is the
/// composition, which is four joints long and green at every joint if any one of them is broken: the
/// daemon must publish the `agent` key, `sprag-client` must parse it onto its cache, the client must
/// ask for it (`HostClient::pane_agent`), and the paint path must turn a CHANGED digest into an escape
/// this terminal receives. R253 paid for that lesson at this exact seam — unit-green, inert in the
/// binary — and this client has no other chrome, so a break here is a front with nothing to show.
///
/// The pane is agent-SHAPED rather than a credentialed agent, the discipline every measurement in H3
/// has followed: a `printf` paints `claude`'s resting title (its fingerprint) and a bottom-anchored
/// choice list (the `dialog-choice-list` rule), then `cat` holds the pane open and says nothing more.
/// `Blocked` is chosen because it is asserted by evidence PRESENT on the screen, so it publishes on
/// sight — no settle window, and therefore no sleep in this test.
///
/// The title is read off the emulator that consumed the client's bytes, so the assertion is on what a
/// terminal would DISPLAY, not on which escape the client chose to spell it with.
#[test]
fn a_blocked_agent_pane_reaches_the_terminals_window_title() {
    // The resting glyph in the title (OSC 2) is the fingerprint; the numbered choice list in the
    // bottom rows is what makes the verdict `Blocked` rather than `Idle`.
    let (_daemon, _sock, _conn, session, tui) = attached_client_with(
        Tui::attach,
        &[
            "sh",
            "-c",
            "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\
             \\342\\235\\257 1. Yes\\n  2. No\\n'; cat",
        ],
    );

    // The pane id is the daemon's, and the boot pane is 0 — the same id `sprag panes` prints and the
    // MCP tools take, which is why the title names it at all.
    let want = format!("sprag: {session} \u{2014} claude needs an answer (pane 0)");
    wait_for(
        "the agent's state to reach the terminal's window title",
        || settled(tui.title(), &Some(want.clone())),
    );
}

/// **THE round trip.** A keystroke typed at the client's terminal reaches the child process in the
/// pane, and what the child sends back is painted onto that same terminal.
///
/// Nothing weaker would do. The client is in raw mode — its own terminal echoes nothing — so the
/// text this test finds on the screen cannot have come from anywhere but `cat`, on the other side
/// of the daemon. That is the whole claim of the front, observed from outside every process that
/// implements it.
#[test]
fn typing_reaches_the_child_and_comes_back_painted() {
    let (_daemon, _sock, _conn, _session, mut tui) = attached_client();

    tui.type_bytes(b"hello");

    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "hello")
    });
}

/// A named key crosses as a NAME, not as the bytes this terminal spells it with — the property the
/// key decoder exists for, at the only place it can be observed end to end.
///
/// `Backspace` is the key that shows it: this terminal sends `DEL` (`0x7f`) for it, and the pane's
/// line discipline erases a character only if what reached its PTY was the byte IT considers erase.
/// A client that forwarded bytes blindly would still pass — so the test types the OTHER spelling,
/// `BS` (`0x08`), which is what a terminal configured the old way sends and what the pane's `cat`
/// would print as a control character rather than act on. It erases, because the client named the
/// key and the host re-encoded it.
#[test]
fn a_key_crosses_as_a_name_and_is_re_encoded_for_the_child() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"abc");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "abc")
    });

    // The alternative Backspace spelling — normalised by the host, not by this terminal.
    tui.type_bytes(&[0x08]);
    wait_for("the erase to come back painted", || {
        painted(&mut tui, "ab").map_err(|screen| {
            format!(
                "{screen}; the pane holds {:?}",
                pane_text(&mut conn, &session)
            )
        })
    });
}

/// A window change reaches the PANE's pty, not just the client's surface.
///
/// Asserted against the DAEMON's report of the pane size, which is the only place the difference
/// shows: a client that repainted at the new size while leaving the child on the old winsize would
/// look completely correct on screen and would break every full-screen program in the pane.
///
/// Both edges are checked, and the first is the one that is easy to miss — ATTACHING is itself a
/// size change, because the pane was created at a size that has nothing to do with the terminal now
/// showing it.
#[test]
fn a_window_change_reaches_the_panes_pty() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    assert_ne!(
        BOOT_PANE, BOOT_PTY,
        "the pane must start at a size the terminal is not, or nothing below is a measurement",
    );
    wait_for("the attach to size the pane to the terminal", || {
        settled(pane_size(&mut conn, &session), &Some(BOOT_PTY))
    });

    // Something on screen BEFORE the reshape, so the assertion after it is about content and not
    // about an empty pane agreeing with an empty pane.
    tui.type_bytes(b"wide");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "wide")
    });

    let resized = (100, 30);
    tui.resize(resized.0, resized.1);
    wait_for("the window change to reach the pane", || {
        settled(pane_size(&mut conn, &session), &Some(resized))
    });

    // ...and the pane's content survives being reshaped under it. A resize that reached the pty and
    // lost the screen would pass every assertion above and be unusable.
    wait_for(
        "the reshaped pane to still hold what the child said",
        || {
            painted(&mut tui, "wide").map_err(|screen| {
                format!(
                    "{screen}; the pane holds {:?}",
                    pane_text(&mut conn, &session)
                )
            })
        },
    );
}

/// `sprag attach --tui SESSION` lands a WORKING client on this terminal, and a window change still
/// reaches it.
///
/// The CLI's own suite pins the flag parse and the env it exports, but its client is a stand-in
/// that prints that env and exits — so it cannot see either claim here, and both need a real
/// terminal to be true or false:
///
/// 1. **The exported env reaches a real client.** Nothing in [`Tui::attach_via_cli`] names the
///    session or the socket to `sprag-tui`; only the CLI does. A pane that ends up shaped like
///    this pseudoterminal therefore could not have been scoped any other way.
/// 2. **A window change still reaches it** — the client is live on a real terminal, not merely
///    started.
///
/// MEASURED, by giving the terminal client the WINDOW client's launch (`own_session` + spawn +
/// wait): this test fails at the FIRST wait — `0 attached clients` for the whole 15s deadline —
/// because `setsid` leaves the child with no controlling terminal and `/dev/tty` is exactly the
/// name for the one it no longer has. The client dies on its first line with ENXIO, having
/// connected to nothing. That is why the CLI `exec`s this client where it spawns the window.
#[test]
fn the_cli_launches_a_client_a_window_change_still_reaches() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_via(Tui::attach_via_cli);

    // `attached_client_via` has already settled the pane at BOOT_PTY, which is claim 1: the CLI
    // named the session and the socket, or there would be no attached client to have sized.
    assert_ne!(
        BOOT_PANE, BOOT_PTY,
        "the pane must start at a size the terminal is not, or nothing above is a measurement",
    );

    let resized = (100, 30);
    tui.resize(resized.0, resized.1);
    wait_for(
        "the window change to reach a client the CLI launched",
        || settled(pane_size(&mut conn, &session), &Some(resized)),
    );
}

/// The prefix key, as the byte a terminal sends for `Ctrl-B`.
const PREFIX: &[u8] = &[0x02];

/// The bytes a terminal sends for the LEFT arrow in the normal cursor-key mode — `CSI D`, which is
/// what the client's own decoder has to turn back into the name `ArrowLeft` its keymap is spelled
/// in. Written in ONE call so the three bytes reach the parser together.
const ARROW_LEFT: &[u8] = b"\x1b[D";

/// ...and the RIGHT arrow, `CSI C`.
const ARROW_RIGHT: &[u8] = b"\x1b[C";

/// The bytes a terminal sends for SHIFT + the left arrow — `CSI 1;2D`, the modified form, where
/// parameter 2 is the modifier mask (`1 + 1` for shift).
///
/// This is a DIFFERENT byte sequence from [`ARROW_LEFT`], not the same one with a flag, which is
/// exactly why the binding it drives had to be driven live rather than reasoned about: a client
/// whose decoder dropped the parameters would report `ArrowLeft` and silently run the SELECT.
const SHIFT_ARROW_LEFT: &[u8] = b"\x1b[1;2D";

/// ...and SHIFT + the right arrow, `CSI 1;2C`.
const SHIFT_ARROW_RIGHT: &[u8] = b"\x1b[1;2C";

/// The `windows` slot for `session` — every window's NAME and which one is current, as the DAEMON
/// holds it. The authority a window key is judged against: a client that painted a second window
/// without the daemon having one would pass any check made on its own screen.
fn windows_of(conn: &mut HostConn, session: &str) -> Vec<(String, bool)> {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .ok()
    .and_then(|value| value.as_array().cloned())
    .unwrap_or_default()
    .iter()
    .filter_map(|w| {
        Some((
            w["name"].as_str()?.to_owned(),
            w["current"].as_bool().unwrap_or(false),
        ))
    })
    .collect()
}

/// **The window level, reached from a KEY** — a real `sprag-tui` on a real PTY presses `prefix c`
/// and then `prefix n` / `prefix p`, and the DAEMON's own window list is what is asserted.
///
/// Measured at `7a8f93f` before the arms existed: `prefix c` was `Routed::Swallow`, so the session
/// still held one window and the client painted the same pane. Every window verb the daemon serves
/// — and that the GUI palette already offers — was unreachable from a key, which for `sprag-tui`
/// (no palette) meant unreachable at all.
///
/// The three assertions are ordered so each failure says something different: the window was
/// CREATED and selected; the walk moves to a DIFFERENT window and wraps; and the walk lands where
/// the daemon says, not where the client guessed.
#[test]
fn the_window_keys_create_and_walk_the_sessions_windows() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    // Something typed first, so the keys below are proven to act on a WORKING client.
    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&mut tui, "before"));
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), true)],
        "the session boots with one window, which is what makes the create visible",
    );

    // 1. `prefix c` — tmux's most-pressed key after the splits.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"c");
    wait_for("the key to create a second window and select it", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });

    // 2. `prefix n` — the walk WRAPS, which is what makes a window list a ring.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"n");
    wait_for("the next-window key to wrap onto the first", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), true), ("1".to_owned(), false)],
        )
    });

    // 3. `prefix p` — the other way, wrapping back.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"p");
    wait_for("the previous-window key to wrap onto the last", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });

    assert_eq!(
        tui.liveness(),
        "running",
        "and the client is still alive, having driven three window keys",
    );
}

/// **A NAME can be typed at this client, and the keystrokes that carry it never reach the shell.**
///
/// The gap R306 measured before writing anything: `prefix ,` was UNBOUND, so it was
/// `Routed::Swallow` and the mode is one key long — which means the `build` and the `Enter` that a
/// tmux user types next went to the PANE and the shell ran `build`. That is what the second
/// assertion here is for, and it is the half that discriminates: a client that swallowed the `,`
/// and did nothing else would still leave the window renamed by nobody and `build` in the shell.
///
/// The ESCAPE half is asserted last and in the same test, because the two are one property seen
/// from both sides: while the prompt is up the keyboard is the prompt's, and the moment it closes
/// the keyboard is the pane's again. Two tests would each pin half of that and neither would pin
/// the transition.
#[test]
fn a_name_typed_at_the_prompt_renames_the_window_and_never_reaches_the_shell() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&mut tui, "before"));
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), true)],
        "the session boots with one window, whose name is what the prompt will move",
    );
    let shell_before = pane_text_of(&mut conn, &session, 0);

    // `prefix ,` — tmux's rename key. Then more text, one keystroke at a time, and `Enter`.
    //
    // The first pass AMENDS: the editor opens holding the window's current name with the cursor at
    // its end, so what lands is `0-x` and not `-x`. That is the seed being REAL — a prompt that
    // opened empty (or one that cleared on the first keystroke, which is what the rival does) would
    // rename the window to `-x`, and it is also the assertion that proves the answer is not simply
    // whatever was typed.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b",");
    tui.type_bytes(b"-x\r");
    wait_for("the amended name to reach the daemon's window list", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0-x".to_owned(), true)],
        )
    });

    // ...and `C-u` is how a user starts over, which is the same chord the shell behind the prompt
    // would have used. `-x` above also proves the answer never re-enters a parser: a leading dash
    // is a NAME here, where tmux's `command-prompt` substitutes into a command line and has to
    // quote to keep it from becoming a flag.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b",");
    tui.type_bytes(b"\x15build\r");
    wait_for(
        "the replaced name to reach the daemon's window list",
        || {
            settled(
                windows_of(&mut conn, &session),
                &vec![("build".to_owned(), true)],
            )
        },
    );

    // ...and NOT the shell. `cat` echoes what it is given, so the pane's own text is the record of
    // what got past the prompt: before this round, every one of those characters did.
    let shell_after = pane_text_of(&mut conn, &session, 0);
    assert_eq!(
        shell_after, shell_before,
        "the name was typed AT THE CLIENT: not one character of it reached the pane",
    );

    // A PASTE is the prompt's too. Bracketed (DEC 2004), which is how a terminal delivers one and
    // why it arrives as its own event rather than as keystrokes — the path that was still going
    // straight to the shell after the keystroke path had been closed.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b",");
    let shell_before_paste = pane_text_of(&mut conn, &session, 0);
    tui.type_bytes(b"\x1b[200~-pasted\x1b[201~\r");
    wait_for(
        "the pasted text to reach the daemon as part of the name",
        || {
            settled(
                windows_of(&mut conn, &session),
                &vec![("build-pasted".to_owned(), true)],
            )
        },
    );
    assert_eq!(
        pane_text_of(&mut conn, &session, 0),
        shell_before_paste,
        "and not one character of the paste reached the pane",
    );

    // CANCELLING gives the keyboard back, and the window keeps the name it has.
    //
    // `C-c` rather than `Escape`, and the reason is a property of terminals rather than of this
    // client: a lone `\x1b` is the START of an escape sequence as far as any parser is concerned, so
    // a byte typed straight after it arrives as `Alt+<that key>` instead of as two keystrokes.
    // Escape does cancel — for a user who pauses, which is what a user cancelling does — but a TEST
    // that typed it and then immediately typed again would be asserting the parser's timeout.
    // `C-c` and `C-g` are one byte each and mean cancel at every shell prompt, which is why the
    // editor takes all three.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b",");
    tui.type_bytes(b"discarded\x03");
    typing_follows(&mut tui, &mut conn, &session, 0);
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("build-pasted".to_owned(), true)],
        "a cancelled prompt renames nothing, and the client is typing into the pane again",
    );
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client survived asking, answering and cancelling",
    );
}

/// **`prefix &` ASKS before it destroys, and only a `y` destroys anything.**
///
/// tmux's key with tmux's own guard. R305 shipped `kill-window` bindable and UNBOUND because there
/// was no prompt to guard it with, and recorded that its arm therefore had no live coverage at all
/// — this is that coverage, on both answers.
///
/// The NO half runs first and is the one that discriminates: a client that performed the verb and
/// asked afterwards, or that treated any key as consent, would pass a yes-only test.
#[test]
fn the_kill_key_asks_and_only_a_yes_takes_the_window() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&mut tui, "before"));

    // A second window, so a kill has something to take that is not the session itself.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"c");
    wait_for("a second window to exist", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });

    // ASKED, and answered NO — with `n`, which is not a key the guard has any special reading of:
    // anything but `y` is no, which is tmux's rule and the safe direction for a question whose yes
    // cannot be taken back.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"&");
    tui.type_bytes(b"n");
    typing_follows(&mut tui, &mut conn, &session, 1);
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), false), ("1".to_owned(), true)],
        "a refused kill takes nothing, and the keyboard is the pane's again",
    );

    // ...and YES takes it. The session is left on the survivor, which is the daemon's own choice.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"&");
    tui.type_bytes(b"y");
    wait_for("the answered kill to reach the daemon", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), true)],
        )
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client survived destroying a window it was projecting",
    );
}

/// The ARRANGEMENT's pane order for `session`'s current window — the fact a swap moves and the pane
/// LISTING does not.
///
/// [`pane_ids`] answers the pool, which is deliberately unmoved by a swap (`panes` says WHO,
/// `layout` says WHERE), so a swap test that read it would pass over a daemon that did nothing at
/// all. This reads the arena the layout slot publishes and takes its leaves in order.
fn tiled_order(conn: &mut HostConn, session: &str) -> Vec<u64> {
    let Ok(layout) = conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(LAYOUT_SLOT) }),
    ) else {
        return Vec::new();
    };
    // The arena's nodes in index order are not paint order; the tree's `panes()` walk is. Reading
    // the leaves as they appear is enough HERE because a one-split window's arena holds its two
    // leaves in the order the split made them, which the assertions below pin explicitly.
    layout["tree"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["leaf"].as_u64())
                .collect()
        })
        .unwrap_or_default()
}

/// What an 80x24 terminal divides into, computed the way the layouter computes it so the numbers in
/// the tests below are derived rather than copied: one cell for the divider, the remainder split
/// with the odd cell on the far side.
///
/// Written out because these are the assertions that would otherwise be four magic numbers, and a
/// magic number in a geometry test is indistinguishable from the geometry being wrong.
const fn halves(extent: u16) -> (u16, u16) {
    let avail = extent - 1;
    (avail / 2, avail - avail / 2)
}

/// **THE multi-pane gate.** `prefix %` divides the pane, and both halves reach their CHILDREN at the
/// sizes the layouter gave them.
///
/// Three claims, and the first two are the ones a screenshot could not make:
///
/// * the daemon reports two panes whose sizes are the two halves of 80 columns with a divider
///   column taken out — so the layouter's arithmetic reached two real PTYs, not just a surface;
/// * the pane that was there keeps what its child said, painted in the half it now occupies;
/// * a VERTICAL line stands between them, which is what `-h` means and the one thing that would
///   still look right if the direction were inverted everywhere else consistently.
#[test]
fn a_split_gives_each_child_its_own_half_of_the_terminal() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    // Typed BEFORE the split, so the assertion after it is that content SURVIVED being re-tiled —
    // an empty pane agreeing with an empty pane would prove nothing.
    tui.type_bytes(b"left");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "left")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");

    let (near, far) = halves(BOOT_PTY.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });

    wait_for("a divider to stand between the two panes", || {
        let column = tui.column(near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {near} reads {column:?}: {:?}", tui.rows()))
        }
    });

    assert_eq!(
        tui.span(0, 0..near),
        "left",
        "the pane that was there keeps what its child said, inside its own columns now",
    );
}

/// ...and `prefix "` divides the ROWS instead, which is the assertion that makes the one above mean
/// something.
///
/// **Neither test alone can catch the two keys being swapped**, and that is the whole reason this
/// one exists: a client that mapped both to the same direction, or exchanged them, still splits and
/// still shows two panes. R227 recorded exactly this failure one layer down — a CLI test that ran
/// each form and counted panes would have passed a CLI that mapped `-v` to horizontal.
#[test]
fn the_other_split_key_divides_rows_instead_of_columns() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"top");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "top")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"\"");

    let (near, far) = halves(BOOT_PTY.1);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(BOOT_PTY.0, near), (BOOT_PTY.0, far)],
        )
    });

    wait_for("a divider to stand between the two panes", || {
        let row = tui.row(near);
        if !row.is_empty() && row.chars().all(|glyph| glyph == '\u{2500}') {
            Ok(())
        } else {
            Err(format!("row {near} reads {row:?}: {:?}", tui.rows()))
        }
    });
}

/// Keys follow the focus the prefix moves, which is the whole of what focus MEANS in a client the
/// daemon has no active pane for.
///
/// The measurement is made at the pane that can be read unambiguously: pane 0 runs `cat`, so
/// anything reaching it comes back. After a split, focus is on the NEW pane (tmux's behaviour), so
/// what is typed must NOT appear in pane 0 — and after `prefix o` wraps focus back, it must. Both
/// directions are needed: a client that sent every key to pane 0 regardless would pass the second
/// assertion alone, and one that sent them nowhere would pass the first.
#[test]
fn keys_follow_the_focus_the_prefix_moves() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"before");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "before")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PTY.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });

    // Into the NEW pane, which is not `cat`. It must not reach pane 0.
    tui.type_bytes(b"elsewhere");
    // Then back to pane 0, where `cat` will echo whatever arrives.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"o");
    tui.type_bytes(b"back");

    wait_for(
        "the keys typed after the focus moved back to reach pane 0",
        || {
            let held = pane_text(&mut conn, &session);
            if held.contains("back") {
                Ok(())
            } else {
                Err(format!("pane 0 holds {held:?}"))
            }
        },
    );

    // The negative half, checked only once the positive one has landed: `back` arriving is proof
    // that everything typed before it has been delivered too, so an absent `elsewhere` is a
    // decision rather than a race.
    let held = pane_text(&mut conn, &session);
    assert!(
        !held.contains("elsewhere"),
        "what was typed while the new pane had focus must not reach pane 0: {held:?}",
    );
}

/// **The arrow keys walk the ARRANGEMENT, and the edge of it is quiet** — `prefix Left` / `prefix
/// Right` in the shipped binary, against a real daemon, with nothing bound by this test.
///
/// This is the first live coverage of a keymap action reaching the DAEMON rather than being carried
/// out inside the client: `%`, `"` and `o` are all resolved by `sprag-tui` itself, so every one of
/// them would still pass over a client that never sent a `select_pane` at all. Here the client
/// sends a DIRECTION and adopts whatever comes back, so what is asserted is the daemon's walk over
/// its own arrangement.
///
/// Three claims, and the third is the one that makes the first two mean something:
///
/// * **`prefix Left` from the right half lands on pane 0**, whose `cat` echoes what is typed next;
/// * **`prefix Right` goes back**, so the two flags are not one direction spelled twice — the
///   inversion `the_other_split_key_divides_rows_instead_of_columns` guards one layer down;
/// * **`prefix Left` AT THE LEFT EDGE moves nothing.** A client that read these as a CYCLE would
///   wrap here and land on the right pane, and both assertions above would still have passed. The
///   edge is also not an ERROR — the daemon answers the unmoved pane — so nothing is expected to
///   fail, only to stay.
///
/// The daemon's `active` flag is read alongside the typing, because the typing alone would also be
/// satisfied by a client that moved its own ring and told nobody.
#[test]
fn the_arrow_keys_walk_the_arrangement_and_stop_at_its_edge() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"before");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "before")
    });

    // `-h` puts the panes side by SIDE, so pane 0 is the LEFT one and focus lands on the right.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PTY.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });
    let ids = pane_ids(&mut conn, &session);
    let (left, right) = (ids[0], ids[1]);
    wait_for("the split to leave the session on the NEW pane", || {
        settled(active_pane(&mut conn, &session), &Some(right))
    });

    // LEFT: onto pane 0, where `cat` echoes.
    tui.type_bytes(PREFIX);
    tui.type_bytes(ARROW_LEFT);
    wait_for("the left arrow to move the session onto pane 0", || {
        settled(active_pane(&mut conn, &session), &Some(left))
    });
    typing_follows(&mut tui, &mut conn, &session, left);
    tui.type_bytes(b"reached");
    wait_for("what was typed after the move to reach pane 0", || {
        let held = pane_text(&mut conn, &session);
        if held.contains("reached") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });

    // ...AT THE EDGE: nothing moves, and nothing fails.
    tui.type_bytes(PREFIX);
    tui.type_bytes(ARROW_LEFT);
    tui.type_bytes(b"stayed");
    wait_for("what was typed at the edge to reach pane 0 as well", || {
        let held = pane_text(&mut conn, &session);
        if held.contains("stayed") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });
    assert_eq!(
        active_pane(&mut conn, &session),
        Some(left),
        "the left arrow at the left edge must leave the session where it was",
    );

    // RIGHT: back off pane 0, so the two flags are two directions.
    tui.type_bytes(PREFIX);
    tui.type_bytes(ARROW_RIGHT);
    wait_for("the right arrow to move the session back", || {
        settled(active_pane(&mut conn, &session), &Some(right))
    });
    typing_follows(&mut tui, &mut conn, &session, right);
    tui.type_bytes(b"elsewhere");

    // ...and LEFT once more, whose echo is what dates the negative assertion below. The far pane is
    // never READ: its width is one half of the terminal minus a shell prompt of whatever length
    // this machine's `$USER@$HOST` makes, so a marker typed there may come back wrapped. Ordering
    // is what makes the absence mean something instead — `landed` arriving in pane 0 proves
    // everything typed before it was delivered somewhere.
    tui.type_bytes(PREFIX);
    tui.type_bytes(ARROW_LEFT);
    wait_for("the left arrow to bring the session back to pane 0", || {
        settled(active_pane(&mut conn, &session), &Some(left))
    });
    typing_follows(&mut tui, &mut conn, &session, left);
    tui.type_bytes(b"landed");
    wait_for("the last marker to reach pane 0", || {
        let held = pane_text(&mut conn, &session);
        if held.contains("landed") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });
    let held = pane_text(&mut conn, &session);
    assert!(
        !held.contains("elsewhere"),
        "what was typed after moving right must not reach pane 0: {held:?}",
    );
}

/// **The SHIFTED arrow moves the PANE, and the cursor stays with it** — `prefix S-Left` in the
/// shipped binary, against a real daemon, with nothing bound by this test.
///
/// The twin of `the_arrow_keys_walk_the_arrangement_and_stop_at_its_edge`, and it exists as a LIVE
/// test rather than a unit one for a reason that was a real risk and not a ritual: `S-ArrowLeft` is a
/// different byte sequence from `ArrowLeft` (`CSI 1;2D` against `CSI D`), so a default binding on a
/// modified key is a promise about this client's DECODER. A decoder that dropped the modifier would
/// report `ArrowLeft`, run the SELECT, and leave a shipped default binding that parses and never
/// fires — the exact shape the keymap's own docs say the vocabulary must not have.
///
/// Three claims:
///
/// * **the ARRANGEMENT moves** — read from the layout slot, because the pane LISTING is deliberately
///   unmoved by a swap and a test reading it would pass over a daemon that did nothing;
/// * **the ACTIVE pane does not** — which is what tells this apart from the select one modifier key
///   away. A client that ran `select-pane -L` here would move the cursor and leave the layout alone,
///   i.e. it would fail both of the first two assertions in opposite directions;
/// * **the EDGE is quiet** — the same pane, pressed again with nothing to its left, moves nothing and
///   fails nothing.
#[test]
fn the_shifted_arrows_move_the_pane_and_leave_the_cursor_on_it() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"before");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "before")
    });

    // `-h` puts the panes side by SIDE, so pane 0 is the LEFT one and focus lands on the right.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PTY.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });
    let ids = pane_ids(&mut conn, &session);
    let (left, right) = (ids[0], ids[1]);
    wait_for("the split to leave the session on the NEW pane", || {
        settled(active_pane(&mut conn, &session), &Some(right))
    });
    assert_eq!(
        tiled_order(&mut conn, &session),
        vec![left, right],
        "the split put the new pane on the RIGHT, which is what the presses below depend on",
    );

    // SHIFT-LEFT: the pane the user is on trades places with pane 0.
    tui.type_bytes(PREFIX);
    tui.type_bytes(SHIFT_ARROW_LEFT);
    wait_for("the shifted left arrow to trade the two panes", || {
        settled(tiled_order(&mut conn, &session), &vec![right, left])
    });
    assert_eq!(
        active_pane(&mut conn, &session),
        Some(right),
        "and the user is still on the pane they moved — a swap moves a PANE, not a cursor, which \
         is the whole difference from the unshifted arrow",
    );

    // ...AT THE EDGE: nothing moves, and nothing fails. A client that read this as a CYCLE would
    // send the pane back to the right and the assertion below would catch it.
    tui.type_bytes(PREFIX);
    tui.type_bytes(SHIFT_ARROW_LEFT);
    tui.type_bytes(PREFIX);
    tui.type_bytes(SHIFT_ARROW_RIGHT);
    wait_for("the shifted RIGHT arrow to trade them back", || {
        settled(tiled_order(&mut conn, &session), &vec![left, right])
    });
    assert_eq!(
        active_pane(&mut conn, &session),
        Some(right),
        "the cursor stayed put across all three presses",
    );

    // The pane's CHILD is still reachable, which is what says the swap moved a leaf rather than
    // rebuilding one: the session is on `right` and typing still lands there.
    typing_follows(&mut tui, &mut conn, &session, right);
}

/// An arrangement changed by ANOTHER client reaches this one — the property that makes a
/// multiplexer's clients views of one session rather than three unrelated programs.
///
/// The split is made over the observer's connection, which is exactly what `sprag split-window` or
/// a second attached client does; nothing is typed at the terminal at all. The client learns of it
/// through the host's change notification and must RE-TILE, not merely repaint: painting the old
/// arrangement would leave the new pane nowhere and both children at the wrong size.
///
/// `cmd` is named here where the client's own `%` does not name one, because this test is the
/// second client rather than the first: `cat` keeps the new pane's PTY open and makes the pane's
/// arrival observable without depending on whatever `$SHELL` is on the machine running the suite.
#[test]
fn a_split_made_by_another_client_re_tiles_this_one() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"mine");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "mine")
    });

    let first = pane_ids(&mut conn, &session);
    assert_eq!(
        first.len(),
        1,
        "one pane before the outside split: {first:?}"
    );
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": mux_action_path(SPLIT_ACTION),
            "args": { "pane": first[0], "dir": "horizontal", "cmd": ["cat"] },
        }),
    )
    .expect("the outside split answers");

    let (near, far) = halves(BOOT_PTY.0);
    wait_for(
        "the client to re-tile around a split it did not make",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
            )
        },
    );
    wait_for("a divider to appear without a key being typed", || {
        let column = tui.column(near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {near} reads {column:?}: {:?}", tui.rows()))
        }
    });
    assert_eq!(
        tui.span(0, 0..near),
        "mine",
        "and the pane this client was showing keeps its content",
    );
}

/// A `select-pane` made by ANOTHER client moves THIS one's focus — the property that makes the
/// active pane session state rather than each client's private idea of where it is looking.
///
/// The select is sent over the observer's connection, which is exactly what `sprag select-pane` or
/// a second attached client does; no key is typed at the terminal for it. The proof is where the
/// NEXT keystrokes land: pane 0 runs `cat` and echoes, so text typed after the outside select must
/// reach it, and a client that kept its own focus would still be typing into the pane the split
/// left it on.
///
/// The negative half matters as much: this client was focused on the NEW pane when the select
/// arrived, so a client that always typed into pane 0 would pass the positive half alone.
#[test]
fn a_select_pane_made_by_another_client_moves_this_ones_focus() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    // Split from the terminal, which leaves this client focused on the NEW pane (tmux's rule).
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PTY.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });
    // ...and then for THIS CLIENT to have re-tiled around it. The daemon publishes the new pane
    // before the split's own reply reaches the client, so "the daemon has two panes" does not yet
    // mean "the client has moved onto the new one" — and typing at that moment would land in the
    // pane the user just split, which is what makes this wait part of the test rather than
    // decoration. The divider is the visible proof, as `a_split_made_by_another_client_re_tiles`
    // uses it.
    wait_for("the client to re-tile around its own split", || {
        let column = tui.column(near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {near} reads {column:?}"))
        }
    });
    tui.type_bytes(b"elsewhere");

    // The outside select: pane 0, the one running `cat`.
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": mux_action_path(SELECT_PANE_ACTION),
            "args": { "pane": 0 },
        }),
    )
    .expect("the outside select answers");

    // The client learns of the select on its next wake, so the first keystroke after it may still
    // be in flight to the pane it was on. ONE character, typed until it lands, is the only
    // observation of THIS CLIENT's focus available from outside it — and once it has landed,
    // everything typed after goes to the same pane.
    wait_for(
        "this client's focus to follow a selection it did not make",
        || {
            tui.type_bytes(b".");
            let held = pane_text(&mut conn, &session);
            if held.contains('.') {
                Ok(())
            } else {
                // The daemon's own answer rides the diagnostic: a client that failed to FOLLOW and
                // a select that never landed are opposite bugs and look identical from pane 0.
                let rows = conn
                    .call(
                        "scene/query",
                        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
                    )
                    .unwrap_or_default();
                Err(format!(
                    "pane 0 holds {held:?}; the daemon's panes are {rows}"
                ))
            }
        },
    );
    tui.type_bytes(b"followed");
    wait_for("the keys after it to reach that pane whole", || {
        let held = pane_text(&mut conn, &session);
        if held.contains("followed") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });
    let held = pane_text(&mut conn, &session);
    assert!(
        !held.contains("elsewhere"),
        "what was typed before the outside select stayed in the pane it was typed into: {held:?}",
    );
}

/// The prefix table ends the client, and the SESSION outlives it — the difference between a
/// multiplexer and a terminal emulator.
///
/// The detach is driven as the two keystrokes a user types, through the same decoder every other
/// key crosses, so this is the prefix mechanism observed in the shipped binary rather than in the
/// unit test of the routing function.
/// A REAL client SURVIVES a rename of the session it is attached to, and goes on painting it.
///
/// The live driver for R303, and the only thing that proves the CLIENT uses the attached scope: the
/// daemon serving it is a separate claim (`sprag-host`'s
/// `an_attached_client_follows_a_rename_where_a_name_scoped_one_is_captured_by_an_impostor`), and a
/// client that never asked for it would pass that one and die here.
///
/// **Measured before it was written**, against a daemon at `2402e62`: `sprag attach alpha --tui`
/// EXITED the moment `sprag rename-session` ran — its next scoped read carried the retired name,
/// the daemon refused it, and `detach_reason` reads any refusal as "my session is gone". A control
/// in the same run renamed a DIFFERENT session and the client lived, which is what made the
/// instrument mean something; that control is the first half of this test.
///
/// The client's DEATH is deliberately not what is asserted, because it is not the only shape the
/// failure takes: reverting `attach_and_follow`'s scope switch and running THIS harness leaves the
/// client a live process painting nothing — frozen on the frame it had when the name was retired
/// (measured; the reverted run's last observation is `["before", …] (client: running)`). Both are
/// the same defect and neither is the definition of it, so what this asserts is the thing a user
/// actually has: a client that is still counted on its session and still painting it.
///
/// The three assertions are ordered so each one's failure says something different:
///
/// 1. renaming ANOTHER session must not disturb this client (the control — a client that ignored
///    every refusal would pass 2 and 3 while asserting nothing);
/// 2. renaming ITS OWN session leaves it running and still counted as a viewer, now under the new
///    name (the attachment moved, R302, and the client's scope moved with it);
/// 3. it still PAINTS the pane — typed after the rename, so the whole round trip (keystroke → the
///    session's pane → the frame back) is exercised on the far side of the address change, not just
///    the process's continued existence.
#[test]
fn a_rename_of_its_session_leaves_the_client_attached_and_painting() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let rename = |conn: &mut HostConn, from: &str, to: &str| {
        conn.call(
            "scene/invoke",
            json!({
                "session": from,
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": to },
            }),
        )
        .expect("rename_session answers");
    };

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&mut tui, "before"));

    // 1. THE CONTROL: another session is renamed. Nothing about this client may move.
    let mut admin = observe(&sock);
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "bystander" } }),
        )
        .expect("new_session answers");
    rename(&mut admin, "bystander", "renamed-bystander");
    wait_for("the client to still be counted on its own session", || {
        settled(attached(&mut conn, &session), &1)
    });

    // 2. ITS OWN session is renamed, out of band, by a third party.
    rename(&mut admin, &session, "prod");
    wait_for("the viewer badge to follow the name", || {
        settled(attached(&mut conn, "prod"), &1)
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "the client must outlive a rename of the session it is viewing",
    );

    // 3. ...and it is still a WORKING client, not merely a live process.
    // The row ACCUMULATES (the pane's `cat` echoes onto the same line), so the expected text is
    // both halves — which is stronger than matching the new one alone: it says the client is
    // painting the SAME pane it was before the rename, not a fresh view of something else.
    tui.type_bytes(b"after");
    wait_for("the client to paint through the new address", || {
        painted(&mut tui, "beforeafter")
    });
    assert_eq!(
        pane_size(&mut conn, "prod"),
        Some(BOOT_PTY),
        "and it is still the session's client, holding the pane at its terminal's size",
    );

    // 4. AND IT SAYS SO. Surviving the rename is not enough if the client goes on LABELLING itself
    // with the retired name — which is exactly what it did when the survival landed and the name was
    // still a string cached at boot: MEASURED, one `OSC 2` in a whole capture, `sprag: alpha`, for a
    // client the daemon was reporting on `production`. The terminal title is the one label a test
    // can read from outside the process, and it is fed by the same `current_session()` the session
    // rail highlights with and the next/previous walk indexes by.
    wait_for(
        "the terminal's title to name the session's NEW name",
        || settled(tui.title(), &Some("sprag: prod".to_owned())),
    );
}

#[test]
fn the_prefix_detaches_and_the_session_lives_on() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    // Something typed first, so the detach is proven to end a client that was WORKING rather than
    // one that had never got going.
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    tui.type_bytes(&[0x02]); // the prefix
    tui.type_bytes(b"d"); // detach
    let status = tui.wait();
    assert!(
        status.success(),
        "the client exits successfully on detach, not {status:?}",
    );

    wait_for("the daemon to release the client", || {
        settled(attached(&mut conn, &session), &0)
    });
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PTY),
        "the session and its pane outlive the client that was viewing them",
    );
}

/// The child that ASKS for the mouse: button-event tracking (1002) with the SGR encoding (1006),
/// then an echo that makes what it receives visible.
///
/// Three things the `stty` does, and each was found by the fixture failing without it:
///
/// * **`-icanon min 1 time 0`** — a pane's PTY starts CANONICAL, so the line discipline holds every
///   byte until a newline, and a mouse report has none. MEASURED: with the pane left canonical the
///   report reached the child only when a `\r` was typed after it, which is what proved the rest of
///   the chain was already working. Every real mouse-tracking program (an editor, a pager) puts its
///   terminal in raw mode for the same reason, so this is the faithful arrangement rather than a
///   concession to the test.
/// * **`-echo`** — otherwise the line discipline echoes the report back RAW, and raw is an escape
///   sequence the pane's emulator would INTERPRET instead of print, leaving nothing to assert on.
/// * **`cat -v`** — renders the report's ESC as `^[`, so what the child received arrives in the
///   pane's grid as the literal text it is.
const MOUSE_CHILD: [&str; 3] = [
    "sh",
    "-c",
    "stty -echo -icanon min 1 time 0; printf '\\033[?1002h\\033[?1006h'; exec cat -v",
];

/// The client's terminal reports the mouse EXACTLY WHEN a pane's child has asked it to, and at the
/// level the child asked for.
///
/// This is the whole design of [`MouseMirror`](sprag-tui's binary): the local terminal is made to
/// behave as it would have if the child were running in it directly. Capturing the pointer takes
/// the user's own click-drag selection and wheel away, so doing it while nothing wants the reports
/// would be a cost with nothing on the other side — and doing it at ANY-EVENT when the child asked
/// for button-event would put a wire message on every pointer movement for the host to discard.
///
/// The OFF half of this claim is `the_client_takes_the_paste_and_leaves_the_mouse`, whose pane runs
/// `cat`: same client, same terminal, a child that never asks, and the mouse stays the user's.
/// Neither test means much without the other — one arrangement each.
#[test]
fn the_local_terminal_tracks_the_mouse_only_because_a_child_asked() {
    let (_daemon, _sock, _conn, _session, tui) = attached_client_with(Tui::attach, &MOUSE_CHILD);

    wait_for("the client to mirror the child's tracking level", || {
        let modes = tui.local_modes();
        settled(modes.mouse_protocol, &MouseProtocol::ButtonEvent)
    });
}

/// A click on this terminal reaches the CHILD PROCESS in the pane it landed on, as a report.
///
/// The full chain, and every link is a real one: an SGR report written to the pty master →
/// termwiz's parser → the client's edge decoder → the pane under the cell → the wire's `mouse`
/// action → the host's mode gate → `encode_mouse` → the pane's PTY → the child. What comes back is
/// read off the pane's own text as the DAEMON holds it, so nothing about the client's painting can
/// make it pass.
///
/// The coordinates ROUND TRIP, and that is the assertion's edge: the report goes in 1-based
/// (protocol), is carried 0-based ([`MouseInput`](sprag_input::MouseInput)), and is encoded 1-based
/// again — so the numbers that come back must be the numbers that went in. A decoder that forgot
/// the conversion would return `5;4` for a `4;3` click, which reads as a click one cell away rather
/// than as a broken pipeline.
#[test]
fn a_click_reaches_the_child_as_a_report_at_the_cell_it_landed_on() {
    let (_daemon, _sock, mut conn, session, mut tui) =
        attached_client_with(Tui::attach, &MOUSE_CHILD);

    // Tracking must be ON before the click, or the host's gate would drop it and this test would be
    // measuring the gate rather than the path.
    wait_for(
        "the child's tracking to reach the client's terminal",
        || {
            settled(
                tui.local_modes().mouse_protocol,
                &MouseProtocol::ButtonEvent,
            )
        },
    );

    // A left press at column 4, row 3 — the terminal's own 1-based spelling, exactly as an emulator
    // would send it.
    tui.type_bytes(b"\x1b[<0;4;3M");

    wait_for("the report to reach the child and be echoed back", || {
        let text = pane_text(&mut conn, &session);
        if text.contains("[<0;4;3M") {
            Ok(())
        } else {
            Err(format!("the pane holds {text:?}"))
        }
    });
}

/// A wheel notch reaches the child too, as the press xterm spells it — and the pane it is addressed
/// to is the one under the pointer.
///
/// Separate from the click because the wheel is the one edge that is NOT a button state: xterm
/// reports it as pseudo-button 64/65 with no release, so a decoder that read it off the button mask
/// would both mis-name it and invent a release for it. The unit tests pin that reasoning; this pins
/// that the result of it survives the wire.
#[test]
fn a_wheel_notch_reaches_the_child_as_the_press_it_is() {
    let (_daemon, _sock, mut conn, session, mut tui) =
        attached_client_with(Tui::attach, &MOUSE_CHILD);

    wait_for(
        "the child's tracking to reach the client's terminal",
        || {
            settled(
                tui.local_modes().mouse_protocol,
                &MouseProtocol::ButtonEvent,
            )
        },
    );

    tui.type_bytes(b"\x1b[<64;7;5M");

    wait_for("the notch to reach the child", || {
        let text = pane_text(&mut conn, &session);
        if text.contains("[<64;7;5M") {
            Ok(())
        } else {
            Err(format!("the pane holds {text:?}"))
        }
    });

    // The HORIZONTAL wheel, which is a different pseudo-button on the same direction flag. Sent
    // after the vertical one and asserted beside it, because the two are told apart by an axis bit
    // that a decoder could read for one and not the other: 66 in must be 66 out, not 64.
    tui.type_bytes(b"\x1b[<66;7;5M");
    wait_for(
        "a horizontal notch to reach the child as ITS button",
        || {
            let text = pane_text(&mut conn, &session);
            if text.contains("[<66;7;5M") {
                Ok(())
            } else {
                Err(format!("the pane holds {text:?}"))
            }
        },
    );
}

/// A click in the SECOND pane arrives in that pane's own columns — the half of the click path that
/// a single-pane arrangement cannot test at all.
///
/// **MEASURED as the reason this test exists**: forwarding the SCREEN cell instead of the
/// pane-local one leaves every other test in this file green, because the only pane a single-pane
/// client has starts at the origin, where the two coordinate spaces are the same numbers. The
/// subtraction is invisible until a pane begins somewhere else, and then it is wrong for every
/// click in it.
///
/// The arithmetic is derived rather than copied: [`halves`] gives the divider's column, the second
/// pane starts one past it, and the fifth column of that pane is therefore `near + 1 + 4` on the
/// screen. The report goes in naming the SCREEN column and must come back naming `5` — the child's
/// own — so a client that forwarded the screen cell would echo `45` and fail on the number.
#[test]
fn a_click_in_the_second_pane_arrives_in_that_panes_own_columns() {
    // BOTH panes track, and the boot one does so from birth. That is what makes the divider
    // assertion below mean anything: with a non-tracking pane on the divider's left, the host's own
    // gate would discard a misdirected report and the guard under test would be unobservable.
    let (_daemon, _sock, mut conn, session, mut tui) =
        attached_client_with(Tui::attach, &MOUSE_CHILD);

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    wait_for("the split to reach both children", || {
        settled(pane_sizes(&mut conn, &session).len(), &2)
    });
    let second = *pane_ids(&mut conn, &session)
        .get(1)
        .expect("the split made a second pane");

    // The new shell must have TAKEN ITS TERMINAL before anything is typed at it, and this wait is
    // not belt-and-braces: a shell sets its termios with `TCSAFLUSH` on startup exactly as this
    // client does (see `Tui::holds_the_terminal`), so type-ahead delivered before that point is
    // PURGED by the kernel — silently, and only some of it. Its first written byte is the prompt,
    // which it prints after that setup, so a pane holding anything at all is a shell that will
    // keep what it is given.
    //
    // This wait was ADDED after the fact and the reason is worth recording: the test passed for as
    // long as it did because the client was slow. R244 measured the 88 characters below at
    // ~124 ms EACH and called them "an accidental synchroniser"; R246 made a keystroke cheap, the
    // delay went away, and the race it had been hiding surfaced 2 runs in 3.
    wait_for(
        "the split's shell to take its terminal",
        || match pane_text_of(&mut conn, &session, second).trim().is_empty() {
            true => Err("the new pane has printed nothing yet".to_owned()),
            false => Ok(()),
        },
    );

    // The split focuses the new pane, so this types into IT: the shell there is put in the raw,
    // mouse-tracking state a real editor would put it in. The command is typed rather than made the
    // pane's birth argv because a split spawns the host's `$SHELL` and takes no command.
    tui.type_bytes(
        b"stty -echo -icanon min 1 time 0; printf '\x5c033[?1002h\x5c033[?1006h'; exec cat -v\r",
    );
    // THE PANE's tracking mode, read off the daemon — not this client's mirror, which is what this
    // wait used to read under this very comment.
    //
    // R244 measured that mistake and could not land the fix: BOTH panes track here (the boot one
    // from birth), so the client's mirror is already `ButtonEvent` before the split's child has
    // asked for anything, and the wait was satisfied in microseconds. What kept the test honest was
    // the 88 characters above taking ~11 seconds to type, during which the child got there anyway.
    // R246 made a keystroke cheap and the clicks below started arriving before the child had
    // enabled tracking, where the host's own per-pane gate correctly DROPS them: 2 runs in 5.
    //
    // The daemon's `mouse` key is the authority, it is per pane, and it is ADDITIVE — absent until
    // the child asks — so "not yet" and "not tracking" are the same observation, which is the one
    // this wait wants.
    wait_for(
        "the second pane's child to ask for the mouse",
        || match pane_mouse(&mut conn, &session, second) {
            Some(level) if level == "button" => Ok(()),
            other => Err(format!("pane {second} reports {other:?}")),
        },
    );

    // A whole click on the DIVIDER first — press AND release. It is nobody's cell, so nothing may
    // be forwarded anywhere, and what makes that assertable is the click AFTER it: once the real
    // one has arrived, the divider's has had every chance to.
    //
    // The release is not decoration. MEASURED without it: the child received `\x1b[<32;5;3M` — a
    // DRAG — because two presses with movement between them and no release IS a drag, and the
    // decoder read the sequence exactly right. A terminal never sends that, so the test was the
    // thing that was wrong. The button state is tracked even for events that reach no pane, which
    // is also correct: the pointer belongs to the terminal, not to whatever it happens to be over.
    let (near, _far) = halves(BOOT_PTY.0);
    tui.type_bytes(format!("\x1b[<0;{};3M", near + 1).as_bytes());
    tui.type_bytes(format!("\x1b[<0;{};3m", near + 1).as_bytes());

    // Then the second pane's fifth column, on the screen, 1-based as the protocol spells it.
    let screen_col = near + 1 + 4 + 1;
    tui.type_bytes(format!("\x1b[<0;{screen_col};3M").as_bytes());

    wait_for(
        "the report to arrive in the second pane's OWN columns",
        || {
            let text = pane_text_of(&mut conn, &session, second);
            if text.contains("[<0;5;3M") {
                Ok(())
            } else {
                Err(format!("pane {second} holds {text:?}"))
            }
        },
    );
    let text = pane_text_of(&mut conn, &session, second);
    assert_eq!(
        text.matches("[<0;").count(),
        1,
        "exactly the one report reached this child — pane {second} holds {text:?}",
    );
    // ...and NOTHING reached the pane on the divider's left, which is where a lookup written as
    // "the last pane starting at or before this column" would have sent it.
    let neighbour = pane_text_of(&mut conn, &session, 0);
    assert!(
        !neighbour.contains("[<0;"),
        "a click on the divider column belongs to no pane and is not handed to the one beside \
         it — pane 0 holds {neighbour:?}",
    );
}

/// Dragging a divider moves the boundary AND reaches both children's PTYs.
///
/// The claim a screenshot could not make: the sizes are read from the DAEMON, so a client that
/// merely redrew the line in a new column while leaving both children on their old winsize would
/// fail here — the same distinction `a_window_change_reaches_the_panes_pty` exists for, one gesture
/// down.
///
/// The gesture is a real one: press ON the divider, drag to a new column, release. The press is
/// what claims the drag, and it has to be, because the pointer leaves the divider the moment the
/// drag begins — a client recognising the divider on every event would resize once and then start
/// clicking into whichever pane the pointer had entered.
#[test]
fn a_divider_drag_moves_the_boundary_and_both_children() {
    // The boot pane tracks so the client captures the pointer at all; the pane the split creates
    // runs a plain shell, which is enough — the drag is the CLIENT's gesture, not a child's input.
    let (_daemon, _sock, mut conn, session, mut tui) =
        attached_client_with(Tui::attach, &MOUSE_CHILD);
    wait_for(
        "the child's tracking to reach the client's terminal",
        || {
            settled(
                tui.local_modes().mouse_protocol,
                &MouseProtocol::ButtonEvent,
            )
        },
    );

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PTY.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PTY.1), (far, BOOT_PTY.1)],
        )
    });

    // Drag the divider ten columns left. 1-based on the wire, so the divider sitting at 0-based
    // column `near` is `near + 1` to the protocol.
    let moved = near - 10;
    tui.type_bytes(format!("\x1b[<0;{};3M", near + 1).as_bytes());
    tui.type_bytes(format!("\x1b[<32;{};3M", moved + 1).as_bytes());
    tui.type_bytes(format!("\x1b[<0;{};3m", moved + 1).as_bytes());

    // Both children, at the sizes the moved boundary implies — asked of the daemon.
    let (want_near, want_far) = (moved, BOOT_PTY.0 - moved - 1);
    wait_for("the drag to reach both children's PTYs", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(want_near, BOOT_PTY.1), (want_far, BOOT_PTY.1)],
        )
    });

    // ...and the line the user is pointing at is drawn where they dragged it.
    wait_for("the divider to be painted in its new column", || {
        let column = tui.column(moved);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {moved} reads {column:?}: {:?}", tui.rows()))
        }
    });

    // THE RELEASE ENDED IT. A click afterwards must reach the child again, and this is the only
    // assertion that can say so — MEASURED: recognising the divider on every event instead of
    // claiming it on the press passes everything above, because by the time the release arrives the
    // divider has MOVED UNDER THE POINTER, so the release re-claims the drag instead of ending it
    // and the client swallows every click from then on.
    tui.type_bytes(b"\x1b[<0;3;3M");
    wait_for("a click after the drag to reach the child again", || {
        let text = pane_text(&mut conn, &session);
        if text.contains("[<0;3;3M") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {text:?}"))
        }
    });
}

/// A temporary `$XDG_CONFIG_HOME` holding `text` as the user's `config.toml`, removed on drop —
/// including on a panicked assertion, so a failed run leaves no directory behind.
///
/// Unique per CALL for the reason [`socket_path`] is: these tests run as parallel threads of one
/// binary, and a shared directory would have them reading each other's config.
struct ConfigHome(PathBuf);

impl Drop for ConfigHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ConfigHome {
    fn new(text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sprag-tui-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("sprag")).expect("temp config dir");
        std::fs::write(dir.join("sprag").join("config.toml"), text).expect("write config");
        Self(dir)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("a utf-8 temp path")
    }
}

/// **THE GATE for H2: a keymap in the user's file reaches the SHIPPED BINARY.**
///
/// Every other test of this round drives `command()` with a keymap handed to it, which proves the
/// routing and says nothing about whether `run()` ever reads a config — the seam where the whole
/// feature can be absent while every unit test stays green. Only running the real client against a
/// real `config.toml` on a real pseudoterminal can tell them apart.
///
/// Two claims, and the FIRST is the one that discriminates:
///
/// * With `prefix = "C-a"`, the OLD prefix is an ordinary keystroke: `C-b` then `d` reaches the
///   pane, `cat` echoes it, and the letter appears on screen. A binary that ignored the config would
///   have taken `C-b` as its prefix and DETACHED on the `d`, so nothing would ever paint — which is
///   exactly this assertion timing out.
/// * The declared prefix then works: `C-a d` detaches, and the daemon releases the client.
///
/// The expected row is `live^Bd`, and the `^B` is not noise to be explained away — it is the
/// PANE's line discipline echoing `0x02` in caret notation (`echoctl`), which is what a control
/// character reaching a canonical PTY looks like. Its presence is a second, independent statement
/// that the byte travelled: a client that had swallowed `C-b` as its prefix would show neither the
/// caret nor the `d`.
#[test]
fn a_prefix_declared_in_the_users_config_reaches_the_client() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // Typed first, so what follows is proven to act on a client that was WORKING.
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    // The OLD prefix is now just a key: both bytes reach `cat`, which echoes them back.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"d");
    wait_for(
        "the old prefix to reach the pane as an ordinary key",
        || painted(&mut tui, "live^Bd"),
    );

    // ...and the DECLARED prefix is the one that opens the table.
    tui.type_bytes(&[0x01]); // C-a
    tui.type_bytes(b"d"); // detach-client
    let status = tui.wait();
    assert!(
        status.success(),
        "the client exits successfully on the configured detach, not {status:?}",
    );
    wait_for("the daemon to release the client", || {
        settled(attached(&mut conn, &session), &0)
    });
}

/// Rewrite this config home's `config.toml`, for the runtime-rebind gate.
impl ConfigHome {
    fn path(&self) -> PathBuf {
        self.0.join("sprag").join("config.toml")
    }
}

/// Run the shipped `sprag` CLI against `config`, with NO socket — the config-editing verbs (a
/// binding, an option) need no daemon, which is half of what makes them a CLIENT's rather than a
/// server's.
fn sprag_config(config: &ConfigHome, args: &[&str]) -> std::process::Output {
    let out = Command::new(sprag_cli_bin())
        .args(args)
        .env("XDG_CONFIG_HOME", config.as_str())
        .output()
        .expect("run the sprag CLI");
    assert!(
        out.status.success(),
        "sprag {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// Run the shipped `sprag` CLI against a daemon on `sock` AND the user's `config` — for a verb that
/// needs both, which `resize-window` is: the socket carries the session it acts on, and the config is
/// where it reads the `window-size` policy it reports the gap in.
fn sprag_on(sock: &Path, config: &ConfigHome, args: &[&str]) -> std::process::Output {
    let out = Command::new(sprag_cli_bin())
        .args(args)
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("XDG_CONFIG_HOME", config.as_str())
        .output()
        .expect("run the sprag CLI");
    assert!(
        out.status.success(),
        "sprag {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// **THE GATE for H2 slice 2: `sprag bind-key` reaches a client that is ALREADY RUNNING.**
///
/// Every unit test of this round drives the table directly, or drives a `KeymapFile` in-process.
/// None of them can say whether the shipped client ever looks at the file again after it starts —
/// which is the seam where "at runtime" can be entirely absent while everything stays green. Only
/// the real binary, attached to a real daemon through a real pseudoterminal, with the real CLI
/// editing the real file underneath it, can tell them apart.
///
/// Three claims, in the order that makes the middle one discriminating:
///
/// * **Before the bind, `prefix k` does NOTHING.** `k` is unbound, so the client swallows it — and
///   the pane must not receive it either. Establishing this first is what stops the last claim
///   passing for the wrong reason: a client that detached on any unbound key would satisfy it.
/// * `sprag bind-key k detach-client` writes the file while the client holds the terminal.
/// * The key is `k` and not `c`: R305 gave `c` a DEFAULT (`new-window`), and this test needs one
///   the shipped table does not mention, or its FIRST claim would be false.
/// * **`prefix k` now detaches**, and the daemon's attached count falls. There is no reattach
///   anywhere in this test.
///
/// REVERT-PROOF: make `refreshed` return the loaded table without calling `refresh` (i.e. read the
/// config once at startup, as slice 1 did) and the third claim times out — the client keeps
/// swallowing `k` forever while `sprag list-keys` prints it bound, which is exactly the divergence
/// between the printed table and the live one that this design exists to make impossible.
#[test]
fn a_bind_key_run_while_attached_reaches_the_running_client() {
    let config = ConfigHome::new("");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // Typed first, so everything after this is proven to act on a client that was WORKING.
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    // `prefix k` with `k` unbound: swallowed by the client, and NOT delivered to the child.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"k");
    // A second, ordinary keystroke that DOES reach the pane, so the absence above is read from a
    // screen that has since been repainted rather than from one that simply had not caught up.
    tui.type_bytes(b"x");
    wait_for("the following ordinary key to reach the pane", || {
        painted(&mut tui, "livex")
    });
    assert!(
        !pane_text(&mut conn, &session).contains('k'),
        "an unbound command key must not be delivered to the child: {:?}",
        pane_text(&mut conn, &session),
    );

    // The edit, by the shipped CLI, into the file the running client read at startup.
    sprag_config(&config, &["bind-key", "k", "detach-client"]);
    assert!(
        std::fs::read_to_string(config.path())
            .expect("the CLI wrote it")
            .contains("detach-client"),
        "the file really changed",
    );

    // ...and the same two keystrokes now mean what the file says, with no reattach.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"k");
    let status = tui.wait();
    assert!(
        status.success(),
        "the client detaches on the newly bound key, not {status:?}",
    );
    wait_for("the daemon to release the client", || {
        settled(attached(&mut conn, &session), &0)
    });
}

/// **THE GATE for H2's options half: `sprag set-option` reaches a client that is ALREADY RUNNING.**
///
/// The unit tests drive an `Options` table in process, and the CLI tests drive the file with nothing
/// attached. Neither can say whether the shipped client reads the `[options]` table at all, let alone
/// whether it reads it AGAIN after starting — the same seam slice 2's gate exists for, one table over.
///
/// Three claims, each failing in a DIFFERENT direction:
///
/// * **Before the set, `C-a` is an ordinary key.** It reaches `cat`, which echoes `^Ad`. A client
///   that had somehow taken `C-a` as its prefix would consume it and detach on the `d`.
/// * **After the set, the OLD prefix is an ordinary key.** `C-b d` must ALSO reach the pane. This is
///   the discriminating claim: a client that never re-read its config still holds `C-b` as its prefix,
///   so it would DETACH here — and this assertion times out instead.
/// * **After the set, `C-a d` detaches.** The new prefix opens the table, with no reattach anywhere.
///
/// REVERT-PROOF: drop the `refresh` in `refreshed` (read the config once at startup) and the second
/// claim fails — the client detaches on `C-b d` while `sprag show-options` prints `prefix C-a`, which
/// is precisely the divergence between the printed table and the live one that this design forbids.
#[test]
fn a_set_option_run_while_attached_reaches_the_running_client() {
    let config = ConfigHome::new("");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // Typed first, so everything after this is proven to act on a client that was WORKING.
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    // `C-a` is nobody's prefix yet: both bytes reach the child, which echoes them.
    tui.type_bytes(&[0x01]);
    tui.type_bytes(b"d");
    wait_for("C-a to reach the pane as an ordinary key", || {
        painted(&mut tui, "live^Ad")
    });

    // The edit, by the shipped CLI, into the file the running client read at startup.
    sprag_config(&config, &["set-option", "prefix", "C-a"]);
    assert!(
        std::fs::read_to_string(config.path())
            .expect("the CLI wrote it")
            .contains("prefix"),
        "the file really changed",
    );

    // The OLD prefix is now an ordinary key — the claim a client holding a stale table FAILS, by
    // detaching here instead of echoing.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"d");
    wait_for("the abandoned prefix to reach the pane", || {
        painted(&mut tui, "live^Ad^Bd")
    });

    // ...and the DECLARED prefix is the one that opens the table, with no reattach.
    tui.type_bytes(&[0x01]);
    tui.type_bytes(b"d");
    let status = tui.wait();
    assert!(
        status.success(),
        "the client detaches on the newly set prefix, not {status:?}",
    );
    wait_for("the daemon to release the client", || {
        settled(attached(&mut conn, &session), &0)
    });
}

// ----- the window-size gate -----

/// The two clients this gate attaches, and the window each policy must produce from them.
///
/// The dimensions are CROSSED on purpose — the wider client is the shorter one — so the three
/// policies give three DIFFERENT answers, and neither `largest` nor `smallest` is any single
/// client's own area. A daemon that picked a whole client rather than folding per dimension is
/// caught, and so is one that ignores the policy and keeps the last writer.
const FIRST_CLIENT: (u16, u16) = (100, 24);
const SECOND_CLIENT: (u16, u16) = (80, 30);

/// **THE GATE for `window-size`**: two real clients of different sizes, on real pseudoterminals,
/// against a real daemon reading a real `config.toml` — and the pane takes the size the POLICY
/// says, not the size of whoever wrote last.
///
/// This is the claim the whole front is for, and no unit test can make it. The arbitration is a
/// pure function with its own tests; what those cannot say is whether a second client's area ever
/// reaches the daemon, whether the daemon reads the user's policy, or whether a client lays its
/// panes out over the answer instead of over its own terminal. Before this round the pane simply
/// took the size of the client that most recently laid out, and the other client was never told —
/// so `smallest` here is the discriminating case: it demands the SMALL client's width and the
/// SHORT client's height while the daemon's most recent report is neither.
fn window_size_policy_case(policy: &str, want: (u16, u16)) {
    let config = ConfigHome::new(&format!("[options]\nwindow-size = \"{policy}\"\n"));
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let mut first = Tui::attach(&sock, &session);
    wait_for("the first client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    first.resize(FIRST_CLIENT.0, FIRST_CLIENT.1);
    wait_for("the first client's area to become the window", || {
        settled(pane_size(&mut conn, &session), &Some(FIRST_CLIENT))
    });

    // The SECOND client, reported last — so `latest` is its area and the other two policies must
    // disagree with it.
    let mut second = Tui::attach(&sock, &session);
    wait_for(
        "the daemon to count two attached clients",
        || match attached(&mut conn, &session) {
            2 => Ok(()),
            n => Err(format!("{n} attached")),
        },
    );
    second.resize(SECOND_CLIENT.0, SECOND_CLIENT.1);

    wait_for(
        &format!("window-size {policy} to settle the pane at {want:?}"),
        || {
            settled(pane_size(&mut conn, &session), &Some(want))
                .map_err(|got| format!("{got}; clients are {FIRST_CLIENT:?} and {SECOND_CLIENT:?}"))
        },
    );

    // ...and it STAYS there. Without this the test would pass on a daemon that merely passed
    // through the second client's resize on its way to somewhere else, and it is also what would
    // catch two clients fighting: a pane both of them keep rewriting cannot hold one size.
    let held = pane_size(&mut conn, &session);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        pane_size(&mut conn, &session),
        held,
        "the window moved after it had settled: two clients are still fighting over it"
    );
    assert_eq!(held, Some(want), "window-size {policy}");
}

#[test]
fn window_size_smallest_takes_the_narrowest_and_the_shortest() {
    window_size_policy_case(
        "smallest",
        (
            FIRST_CLIENT.0.min(SECOND_CLIENT.0),
            FIRST_CLIENT.1.min(SECOND_CLIENT.1),
        ),
    );
}

#[test]
fn window_size_largest_takes_the_widest_and_the_tallest() {
    window_size_policy_case(
        "largest",
        (
            FIRST_CLIENT.0.max(SECOND_CLIENT.0),
            FIRST_CLIENT.1.max(SECOND_CLIENT.1),
        ),
    );
}

#[test]
fn window_size_latest_takes_the_client_that_reported_last() {
    window_size_policy_case("latest", SECOND_CLIENT);
}

/// **The announce half of `window-size`**: a client that reported NOTHING still re-tiles when
/// somebody else changes the window.
///
/// The policy gates above cannot make this claim, and measuring proved it: removing the scene bump
/// from the daemon's `client/size` handler leaves all three of them green. The reason is that the
/// client which REPORTS re-reads the window inline and resizes the panes itself, so the pane sizes —
/// the only thing those tests observe — are already right without anyone else waking. The client
/// that did not report is the one left holding a stale window, and what it gets wrong is where it
/// draws: it lays the arrangement out over a rectangle the session has left.
///
/// So this asserts on the FIRST client's SCREEN. It splits at 100 columns, where the divider lands
/// at `halves(100)`, and then a second client attaches at 80 under `window-size smallest`. The
/// window becomes 80 wide, and the divider on the first client's own terminal has to MOVE to
/// `halves(80)` — a column it can only find by learning that the window moved.
#[test]
fn a_client_that_reported_nothing_re_tiles_when_the_window_moves() {
    let config = ConfigHome::new("[options]\nwindow-size = \"smallest\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let wide = (100u16, 24u16);
    let narrow = (80u16, 24u16);
    let mut first = Tui::attach(&sock, &session);
    wait_for("the first client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    first.resize(wide.0, wide.1);
    wait_for("the wide client's area to become the window", || {
        settled(pane_size(&mut conn, &session), &Some(wide))
    });

    // Split from the FIRST client, so the arrangement it is about to re-tile is one it made.
    first.type_bytes(PREFIX);
    first.type_bytes(b"%");
    let (wide_near, wide_far) = halves(wide.0);
    wait_for("the split to divide the wide window", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(wide_near, wide.1), (wide_far, wide.1)],
        )
    });
    wait_for("the divider to be drawn at the wide column", || {
        let column = first.column(wide_near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {wide_near} reads {column:?}"))
        }
    });

    // A SECOND, narrower client. It reports; the first one does not. Under `smallest` the window
    // becomes 80 wide, and the first client has to hear about it.
    let mut second = Tui::attach(&sock, &session);
    wait_for(
        "the daemon to count two attached clients",
        || match attached(&mut conn, &session) {
            2 => Ok(()),
            n => Err(format!("{n} attached")),
        },
    );
    second.resize(narrow.0, narrow.1);

    let (narrow_near, narrow_far) = halves(narrow.0);
    assert_ne!(
        narrow_near, wide_near,
        "the two windows must put the divider in DIFFERENT columns or this proves nothing",
    );
    wait_for(
        "the panes to take their share of the narrowed window",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(narrow_near, narrow.1), (narrow_far, narrow.1)],
            )
        },
    );
    // THE claim: the first client's own screen now draws the divider where the NARROW window puts
    // it. A client holding a stale window keeps drawing it at `wide_near`.
    wait_for(
        "the un-reporting client to redraw its divider at the narrowed column",
        || {
            let column = first.column(narrow_near);
            if column.chars().all(|glyph| glyph == '\u{2502}') {
                Ok(())
            } else {
                Err(format!(
                    "column {narrow_near} reads {column:?} (wide column {wide_near} reads {:?})",
                    first.column(wide_near)
                ))
            }
        },
    );
}

// ----- the `window-size manual` gate -----

/// The size an operator PINS below — deliberately not the boot pane's ([`BOOT_PANE`]), not the
/// attaching client's pseudoterminal ([`BOOT_PTY`]) and not the area it resizes to
/// ([`MANUAL_CLIENT`]). Every number a daemon could fall back to is a different number, so a pass
/// cannot be an accident.
const PINNED: (u16, u16) = (111, 33);

/// What the client in the `manual` gate reports. Chosen SMALLER than [`PINNED`] in both dimensions,
/// so the pinned window is one the client cannot show whole — the case a policy that quietly took
/// the client's area would have to produce, and the one a user hits (a big pinned session viewed
/// from a small terminal).
const MANUAL_CLIENT: (u16, u16) = (60, 20);

/// **THE GATE for `window-size manual` + `sprag resize-window`**: a PINNED window does not follow
/// the clients attached to it.
///
/// No unit test can make this claim. The arbitration is a pure function with its own tests, and what
/// those cannot say is whether the pinned size ever reaches a pane, whether the daemon re-derives it
/// when a client arrives, or whether an attaching client still overwrites it — which is exactly what
/// happened before this round, measured: a session whose panes a user had arranged at one size was
/// reflowed by any client that attached, permanently, with nothing that could hold it.
///
/// Four claims on one live session, in the order that makes each discriminating:
///
/// * **With NO client attached at all**, `resize-window` moves the panes off the boot size. Every
///   other policy answers `None` in this state — nobody has reported an area — so this claim alone
///   separates a `manual` the daemon PERFORMS from a value the option table merely accepts.
/// * **A real client attaches on a real pseudoterminal and reports a smaller area. The panes stay
///   pinned.** This is what the feature is for, and a daemon ignoring the pin gives `MANUAL_CLIENT`.
/// * **It holds for a settle window**, so a daemon that passed the pin through on its way to the
///   client's size — the shape a race would take — is caught rather than sampled at the right moment.
/// * **The client detaches and the panes are STILL pinned.** Detach is its own re-derivation path
///   (`window_moved`), and a pin read only on attach would fail here.
#[test]
fn a_pinned_window_does_not_follow_the_client_that_attaches_to_it() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    // The boot pane's size, so the first claim is a MOVE rather than a coincidence.
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PANE),
        "the boot pane starts at the daemon's default size"
    );
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &PINNED.0.to_string(),
            "-y",
            &PINNED.1.to_string(),
        ],
    );
    wait_for("the pin to reach the pane with nobody attached", || {
        settled(pane_size(&mut conn, &session), &Some(PINNED))
    });

    let mut client = Tui::attach(&sock, &session);
    wait_for("the client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    client.resize(MANUAL_CLIENT.0, MANUAL_CLIENT.1);
    // The client's report has to have LANDED before "the pane did not move" means anything —
    // otherwise this passes on a daemon that simply had not heard from it yet. `list-clients` is the
    // daemon's own account of what it was told, so it is the honest thing to wait on.
    wait_for("the daemon to record the client's own area", || {
        let listing = String::from_utf8_lossy(
            &sprag_on(&sock, &config, &["list-clients", "-t", &session]).stdout,
        )
        .into_owned();
        let want = format!("[{}x{}]", MANUAL_CLIENT.0, MANUAL_CLIENT.1);
        if listing.contains(&want) {
            Ok(())
        } else {
            Err(format!("{listing:?} does not yet show {want}"))
        }
    });
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(PINNED),
        "the pinned window followed the client that attached to it"
    );

    // ...and it STAYS. Without this the gate would pass on a daemon that merely happened to be
    // between writes at the moment it was sampled.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(PINNED),
        "the pin was overwritten after it had settled"
    );
    assert!(
        client.holds_the_terminal(),
        "the client died rather than viewing a window bigger than itself"
    );

    // The DETACH path, which re-derives on its own (`window_moved`) — a pin consulted only when a
    // client arrives leaves the panes at the departing client's size here.
    drop(client);
    wait_for("the client to go", || match attached(&mut conn, &session) {
        0 => Ok(()),
        n => Err(format!("{n} still attached")),
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(PINNED),
        "the panes moved when the client left a PINNED window"
    );
}

/// **The other half of `manual`: `-u` hands the window back to its clients.**
///
/// The gate above proves a pin HOLDS; this proves it is not a one-way door, which is the claim a user
/// who pinned by mistake depends on. It is also the only test that makes the un-pinned case
/// observable while a client is attached: with nothing pinned, `manual` defers to the DEFAULT policy,
/// so the attached client's own area becomes the window again — a daemon that treated an un-pinned
/// `manual` as "no window at all" would leave the panes wherever the pin had put them.
#[test]
fn un_pinning_hands_the_window_back_to_the_attached_client() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let mut client = Tui::attach(&sock, &session);
    wait_for("the client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    client.resize(MANUAL_CLIENT.0, MANUAL_CLIENT.1);
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &PINNED.0.to_string(),
            "-y",
            &PINNED.1.to_string(),
        ],
    );
    wait_for("the pin to win over the attached client", || {
        settled(pane_size(&mut conn, &session), &Some(PINNED))
    });

    sprag_on(&sock, &config, &["resize-window", "-t", &session, "-u"]);
    wait_for("the client's own area to become the window again", || {
        settled(pane_size(&mut conn, &session), &Some(MANUAL_CLIENT))
    });
}

// ----- resize-window's DERIVED and RELATIVE forms -----

/// The two clients the derived-form gate attaches. CROSSED — the wider one is the shorter one — so
/// `-a` and `-A` each fold to a rectangle that is NEITHER client's own area, and a daemon that
/// answered by picking a whole client is caught rather than accidentally right.
const FOLD_FIRST: (u16, u16) = (100, 24);
const FOLD_SECOND: (u16, u16) = (80, 30);

/// **THE GATE for `resize-window -a` / `-A`**: the CLI names a rule and the DAEMON answers the
/// rectangle.
///
/// This is the claim that made these forms worth building where they are built. `-A` over a 100x24
/// and an 80x30 client is 100x30 — a rectangle NEITHER client reported — so it can only come from
/// folding per dimension, which is `arbitrate`, which lives beside the reports. A CLI computing it
/// would need both clients' areas over the wire and its own fold: a second geometry model, which is
/// the defect this front spent three rounds removing.
///
/// The pin is then INDEPENDENT of the clients that produced it, which is the difference between
/// `resize-window -A` and `window-size largest`: the second client resizes and the window does not
/// follow it.
fn resize_window_fold_case(flag: &str, want: (u16, u16)) {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let mut first = Tui::attach(&sock, &session);
    wait_for("the first client", || match attached(&mut conn, &session) {
        0 => Err("nobody".to_owned()),
        _ => Ok(()),
    });
    first.resize(FOLD_FIRST.0, FOLD_FIRST.1);
    let mut second = Tui::attach(&sock, &session);
    wait_for("both clients", || match attached(&mut conn, &session) {
        2 => Ok(()),
        n => Err(format!("{n} attached")),
    });
    second.resize(FOLD_SECOND.0, FOLD_SECOND.1);
    // Both reports have to have LANDED before a fold of them means anything.
    wait_for("both areas to reach the daemon", || {
        let listing = String::from_utf8_lossy(
            &sprag_on(&sock, &config, &["list-clients", "-t", &session]).stdout,
        )
        .into_owned();
        let ok = [FOLD_FIRST, FOLD_SECOND]
            .iter()
            .all(|(cols, rows)| listing.contains(&format!("[{cols}x{rows}]")));
        if ok {
            Ok(())
        } else {
            Err(format!("{listing:?}"))
        }
    });

    // THE claim, and the CLI's own line is part of it: the daemon answers the rectangle, and the CLI
    // prints what it was told rather than anything it worked out.
    let printed = String::from_utf8_lossy(
        &sprag_on(&sock, &config, &["resize-window", "-t", &session, flag]).stdout,
    )
    .into_owned();
    assert!(
        printed.contains(&format!("{}x{}", want.0, want.1)),
        "sprag resize-window {flag} printed {printed:?}, not the {want:?} the fold gives"
    );
    wait_for(&format!("{flag} to pin {want:?}"), || {
        settled(pane_size(&mut conn, &session), &Some(want))
    });

    // ...and the pin does not follow the clients it came FROM. That is the whole difference between
    // this and setting the matching `window-size` policy, and a daemon that had merely switched
    // policies would move here.
    second.resize(FOLD_SECOND.0 - 10, FOLD_SECOND.1 - 5);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(want),
        "the pin followed the client it was folded from"
    );
}

#[test]
fn resize_window_takes_the_largest_client_per_dimension() {
    resize_window_fold_case(
        "-A",
        (
            FOLD_FIRST.0.max(FOLD_SECOND.0),
            FOLD_FIRST.1.max(FOLD_SECOND.1),
        ),
    );
}

#[test]
fn resize_window_takes_the_smallest_client_per_dimension() {
    resize_window_fold_case(
        "-a",
        (
            FOLD_FIRST.0.min(FOLD_SECOND.0),
            FOLD_FIRST.1.min(FOLD_SECOND.1),
        ),
    );
}

/// **A relative resize moves what the window IS, not what it was pinned to.**
///
/// The discriminating fixture is a window with NOTHING pinned under a DERIVED policy: the window is
/// the attached client's area, so `-R`/`-D` must answer that plus the delta. A resolver that took its
/// basis from the stored pin would have no basis at all here and refuse — which is exactly what it
/// should do when there is no window either, and the test after this one holds that line.
#[test]
fn a_relative_resize_starts_from_the_window_the_clients_gave_it() {
    // `latest`, not `manual` — so the basis can only be the DERIVED window.
    let config = ConfigHome::new("[options]\nwindow-size = \"latest\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let mut client = Tui::attach(&sock, &session);
    wait_for("the client", || match attached(&mut conn, &session) {
        0 => Err("nobody".to_owned()),
        _ => Ok(()),
    });
    let base = (90, 28);
    client.resize(base.0, base.1);
    wait_for("the client's area to become the window", || {
        settled(pane_size(&mut conn, &session), &Some(base))
    });

    let printed = String::from_utf8_lossy(
        &sprag_on(
            &sock,
            &config,
            &["resize-window", "-t", &session, "-R", "12", "-U", "6"],
        )
        .stdout,
    )
    .into_owned();
    let want = (base.0 + 12, base.1 - 6);
    assert!(
        printed.contains(&format!("{}x{}", want.0, want.1)),
        "printed {printed:?}: -R 12 -U 6 off a {base:?} window is {want:?} (-U SHORTENS)"
    );

    // The panes do not move, because the policy in force is not `manual` — the pin is stored and
    // inert. Switching the policy is what makes the SAME stored rectangle live, with nothing re-sent.
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(base),
        "a pin took effect under `latest`"
    );
    sprag_config(&config, &["set-option", "window-size", "manual"]);
    // A mux action is what re-derives; a file write wakes nobody (R241).
    sprag_on(
        &sock,
        &config,
        &["resize-pane", "-t", &session, "0", "-x", "40", "-y", "10"],
    );
    wait_for("the stored rectangle to become the window", || {
        settled(pane_size(&mut conn, &session), &Some(want))
    });
}

/// **A description that cannot be resolved is REFUSED, and refusing is not un-pinning.**
///
/// The pair `-a` and `-R` with no basis: no client has reported an area and nothing is pinned, so
/// neither names a rectangle. A resolver that answered "no size" for these would UN-PIN the window —
/// the opposite of what was asked — so the pin standing afterwards is the assertion.
#[test]
fn an_unresolvable_resize_window_is_refused_and_leaves_the_pin_alone() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let pinned = (77, 21);
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &pinned.0.to_string(),
            "-y",
            &pinned.1.to_string(),
        ],
    );
    wait_for("the pin", || {
        settled(pane_size(&mut conn, &session), &Some(pinned))
    });

    // `-a` folds the clients, and there are none — a pinned window is not a substitute for a report.
    let refused = Command::new(sprag_cli_bin())
        .args(["resize-window", "-t", &session, "-a"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("XDG_CONFIG_HOME", config.as_str())
        .output()
        .expect("run the sprag CLI");
    assert!(
        !refused.status.success(),
        "a fold of no clients was accepted: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(pinned),
        "a REFUSED resize un-pinned the window"
    );
}

/// **The claim that stands in for tmux's per-WINDOW `window-size` option: it is already expressible.**
///
/// sprag has ONE global `window-size` value, where tmux's is a window option. That looks like a
/// missing tier, and the reason it is not is a consequence of `manual`'s deferral rather than an
/// argument — so it is measured here instead of asserted in a doc.
///
/// Under one global `manual`: a window with a pin holds it, and a SIBLING window with no pin defers
/// to the default policy and follows the attached client. So "this window is fixed, that one follows
/// my terminal" — the thing a per-window option would be for — is reachable today, per window,
/// through the pin that is already per window.
///
/// Switching between them is what makes it observable, and it also exercises the path that would
/// break if the pin were read off the wrong window: `select-window` re-derives at the action
/// boundary, and each window must get ITS OWN answer.
#[test]
fn one_global_manual_still_gives_each_window_its_own_size() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    // Window "0" is current and gets a pin no client will ever report.
    let pinned = (111, 33);
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &pinned.0.to_string(),
            "-y",
            &pinned.1.to_string(),
        ],
    );
    // A SECOND window, born current and un-pinned.
    let born =
        String::from_utf8_lossy(&sprag_on(&sock, &config, &["new-window", "-t", &session]).stdout)
            .trim()
            .to_owned();
    assert_ne!(born, "0", "the new window has its own name: {born:?}");

    let client = (60, 20);
    let mut tui = Tui::attach(&sock, &session);
    wait_for("the client", || match attached(&mut conn, &session) {
        0 => Err("nobody".to_owned()),
        _ => Ok(()),
    });
    tui.resize(client.0, client.1);

    // The un-pinned window follows the client: `manual` with nothing pinned defers to the default
    // policy, which is the decision this test is really about. Answering `None` instead would leave
    // this window to whatever the client sized its own panes to, which is last round's defect.
    wait_for("the un-pinned window to follow the client", || {
        settled(pane_size(&mut conn, &session), &Some(client))
    });

    // ...and its PINNED sibling, under the very same global value, does not.
    sprag_on(&sock, &config, &["select-window", "-t", &session, "0"]);
    wait_for("the pinned window to keep its own size", || {
        settled(pane_size(&mut conn, &session), &Some(pinned))
    });

    // Back again, so the two answers are not one lucky ordering.
    sprag_on(&sock, &config, &["select-window", "-t", &session, &born]);
    wait_for("the un-pinned window to follow the client again", || {
        settled(pane_size(&mut conn, &session), &Some(client))
    });
}

/// The size the window is PINNED to below — smaller than both terminal sizes the client takes, so
/// every pane rectangle is the same rectangle before and after the resize.
const PINNED_SMALL: (u16, u16) = (40, 10);

/// **A clear that moved no rectangle still redraws the panes.**
///
/// The one case [`PaintCache`](sprag_tui::PaintCache)'s arrangement check cannot see: the surface is
/// blanked while every pane keeps its rectangle and every row keeps its stamp. A client that did not
/// tell the cache it had cleared would skip every row and leave the user looking at nothing.
///
/// Reaching it takes a PINNED window, and that is the point rather than an inconvenience: with a
/// derived window the client's own area IS the window, so a resize moves every rectangle and the
/// arrangement check covers it. Pinned, the client's terminal grows past a window that does not
/// follow it — the panes do not move, and the resize path still clears because a shrunken screen
/// cannot be trusted to hold what the old one did.
///
/// Written because the revert-proof for that `forget()` passed against the whole suite, twice: the
/// first attempt at this test used a divider drag onto the divider's own column, which reaches
/// nothing at all — `MouseEdges` emits no edge for a pointer that did not move, so there was no
/// drag, no clear, and a test that asserted the screen was fine because nothing had happened to it.
#[test]
fn a_clear_that_moved_no_rectangle_still_redraws_the_panes() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let (_daemon, sock) = spawn_daemon_with_config(&["cat"], Some(config.as_str()));
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &PINNED_SMALL.0.to_string(),
            "-y",
            &PINNED_SMALL.1.to_string(),
        ],
    );

    let mut tui = Tui::attach(&sock, &session);
    wait_for(
        "the daemon to count the client as attached",
        || match attached(&mut conn, &session) {
            0 => Err("0 attached clients".to_owned()),
            _ => Ok(()),
        },
    );
    wait_for("the client to take the terminal", || {
        match tui.holds_the_terminal() {
            true => Ok(()),
            false => Err("the client has written nothing yet".to_owned()),
        }
    });
    // The pin holds against the attaching client, which is `window-size manual`'s own claim and
    // here is also the precondition: the rectangle must not be the client's.
    wait_for("the pinned window to reach the pane", || {
        settled(pane_size(&mut conn, &session), &Some(PINNED_SMALL))
    });

    tui.type_bytes(b"hello");
    wait_for("the typed text to come back painted", || {
        painted(&mut tui, "hello")
    });

    // The terminal GROWS past the pinned window. The client re-reports, the daemon ignores it, the
    // tiling is the same tiling over the same window — and the client clears before repainting.
    tui.resize(100, 30);

    wait_for("the pane to survive a clear that moved nothing", || {
        painted(&mut tui, "hello")
    });
    // ...and it STAYS, rather than being one frame a later repaint takes back.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        tui.row(0),
        "hello",
        "the pane's own cells outlive a clear that moved nothing: {:?}",
        tui.rows(),
    );
}

/// **THE GATE for H2 slice 4's root table: a `-n` binding acts with NO prefix, and the key it took
/// never reaches the child.**
///
/// Both halves are needed and only the second is easy to get wrong. A client that consulted the root
/// table and then ALSO forwarded the keystroke would look correct in every screenshot of the action
/// — the split happens — while quietly typing a control character into the user's shell every time
/// they used their own binding.
///
/// The action is `split-window -h` rather than `detach-client` deliberately: a detaching client
/// cannot then be asked what the pane received. Three claims, in the order that makes them
/// discriminating:
///
/// * The client is painting, so what follows acts on a client that was WORKING.
/// * `C-o` alone — no prefix — splits. Nothing else in this client's vocabulary splits on a bare
///   key, and the daemon is the authority for the count.
/// * The pane's own text holds no `^O`. That is the line discipline's caret echo of `0x0f`, the
///   same evidence the prefix gate uses in the opposite direction: there it PROVES the byte
///   travelled, here its absence proves it did not.
///
/// REVERT-PROOF: drop the root lookup from `Keymap::route` and the second claim times out while the
/// third starts failing too — the key becomes an ordinary keystroke and `^O` appears in the shell.
#[test]
fn a_root_binding_acts_with_no_prefix_and_the_key_never_reaches_the_pane() {
    let config = ConfigHome::new(
        "[[bind]]\nkey = \"C-o\"\naction = \"split-window -h\"\ntable = \"root\"\n",
    );
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    // No prefix. This is the whole of what `-n` means.
    tui.type_bytes(&[0x0f]);
    wait_for("the root binding to split the window", || {
        settled(pane_ids(&mut conn, &session).len(), &2)
    });

    let text = pane_text(&mut conn, &session);
    assert!(
        !text.contains("^O"),
        "a root-bound key is the CLIENT's and must not also reach the child: {text:?}",
    );
    assert!(
        text.contains("live"),
        "...while the keys that were never bound still did: {text:?}",
    );
}

/// **THE GATE for H2 slice 4's repeat: one prefix, two commands — and the window really closes.**
///
/// The third claim is the one that cannot be faked. A client that simply never left the prefix table
/// would satisfy the first two and be catastrophically broken; only typing again AFTER the window
/// has lapsed can tell "the window is open for `repeat-time`" from "the window is open forever".
///
/// The binding is `send-prefix`, and the choice is what makes the whole test readable from ONE pane.
/// Written first against `split-window`, it failed on the third claim for a reason worth recording:
/// a split MOVES THE FOCUS to the pane it creates, so the key typed after the lapse went to a
/// freshly born shell rather than to the `cat` this test can read — and a fresh shell's startup
/// `tcsetattr` purges type-ahead anyway (R246). `send-prefix` changes no focus, spawns no child, and
/// makes the SAME keystroke produce two different, visible bytes depending on the window:
///
/// * inside it, `%` is the client's and puts `^B` in the pane — the line discipline's caret echo of
///   the `0x02` the action sent;
/// * after it, `%` is the program's and puts a literal `%` there.
///
/// `repeat-time` is 100 ms and the lapse is a 500 ms sleep — five times the window. That is not a
/// race: the window closes at a deadline the client computed when it acted, so sleeping PAST it is a
/// one-directional guarantee rather than something being waited for.
///
/// REVERT-PROOF: answer `PrefixMode::ToPane` for a repeating act (i.e. ignore `-r`) and the second
/// claim times out — the screen reads `live^B%`, the `%` having gone straight to `cat`.
#[test]
fn a_repeat_binding_acts_twice_on_one_prefix_and_then_lets_go() {
    let config = ConfigHome::new(
        "[options]\nrepeat-time = 100\n\
         [[bind]]\nkey = \"%\"\naction = \"send-prefix\"\nrepeat = true\n",
    );
    let (_daemon, _sock, _conn, _session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    tui.type_bytes(&[0x02]); // the prefix, ONCE
    tui.type_bytes(b"%");
    wait_for("the bound key to send the prefix", || {
        painted(&mut tui, "live^B")
    });

    // No second prefix. This is `-r`.
    tui.type_bytes(b"%");
    wait_for("the repeat to act again without a second prefix", || {
        painted(&mut tui, "live^B^B")
    });

    // Past the deadline the client itself computed — so the window is shut, not merely likely to be.
    std::thread::sleep(Duration::from_millis(500));
    tui.type_bytes(b"%");
    wait_for("the lapsed window to hand the key back to the pane", || {
        painted(&mut tui, "live^B^B%")
    });
}

/// **`prefix C-Left` moves a real boundary in a real client** — the first gesture other than a
/// pointer drag in `sprag-gui` that has ever changed a split's share, driven here on a terminal
/// that has no pointer at all.
///
/// The assertion is on the DAEMON's pane sizes rather than on this client's screen, `pane_sizes`'s
/// standing reason: a client that tiled its own surface correctly while telling both children they
/// still had the whole terminal would paint a picture that looks right and wrap every line in the
/// wrong column. The whole point of the verb is the number the CHILD is given.
///
/// Three claims a plausible wrong implementation passes only one of:
///
/// * the key reaches the daemon at all — `sprag-tui` had no resize surface of any kind before this;
/// * the DIRECTION moves the boundary, so `C-Left` takes columns from the pane on its left and
///   `C-Right` gives them back, from the same focused pane;
/// * `-r` holds the prefix table open, so the second press needs no second prefix — which is what
///   makes dragging a boundary ten cells one gesture instead of twenty keystrokes.
#[test]
fn a_key_moves_a_real_boundary_and_repeats_without_a_second_prefix() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();
    tui.type_bytes(&[0x02]); // prefix
    tui.type_bytes(b"%"); // side by side
    let (left, right) = halves(BOOT_PTY.0);
    wait_for("the split to reach two real PTYs", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left, BOOT_PTY.1), (right, BOOT_PTY.1)],
        )
    });
    // ASSERTED, not assumed: the split leaves the session on the pane it opened, the RIGHT one — so
    // `C-Left` below moves the boundary on that pane's near side and GROWS it.
    assert_eq!(active_pane(&mut conn, &session), Some(1));

    // ONE prefix, then the resize key. tmux's own default, and its own byte sequence: xterm sends
    // Ctrl+Left as CSI 1;5D.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;5D");
    wait_for("the boundary to move one cell left", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left - 1, BOOT_PTY.1), (right + 1, BOOT_PTY.1)],
        )
    });

    // NO SECOND PREFIX. This is `-r`, and it is what makes the verb usable: a boundary is dragged
    // to where it looks right, which is a dozen presses.
    tui.type_bytes(b"\x1b[1;5D");
    tui.type_bytes(b"\x1b[1;5D");
    wait_for("two more presses inside the repeat window", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left - 3, BOOT_PTY.1), (right + 3, BOOT_PTY.1)],
        )
    });

    // THE DISCRIMINATOR for the direction: the opposite flag from the same pane gives the columns
    // back. A verb that grew whichever pane asked would have shrunk it here too.
    //
    // The SLEEP is not padding — it is what makes the next press a PREFIX. The repeat window from
    // the presses above is still open, and inside it `C-b` is `send-prefix` (the self-send), so a
    // prefix typed here would go to the shell and the arrow after it would follow. Waiting past
    // `repeat-time` is the honest way to start a new gesture, and the first version of this test
    // did not and measured exactly that.
    let past_the_repeat_window = || std::thread::sleep(Duration::from_millis(700));
    past_the_repeat_window();
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;5C"); // Ctrl+Right
    wait_for("the boundary to come back one cell", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left - 2, BOOT_PTY.1), (right + 2, BOOT_PTY.1)],
        )
    });

    // And the FIVE-cell family, on tmux's own second key.
    past_the_repeat_window();
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;3C"); // Alt+Right
    wait_for("the coarse key to move five cells at once", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left + 3, BOOT_PTY.1), (right - 3, BOOT_PTY.1)],
        )
    });

    // THE BINDING IS UNDER THE PREFIX, not in the root table: the same bytes with NO prefix move
    // nothing. Without this the test would pass over a client that resized on every arrow key and
    // took the chord away from the program in the pane for good — which is the whole reason these
    // eight defaults are not root bindings (`C-Left` is word-motion in readline).
    past_the_repeat_window();
    tui.type_bytes(b"\x1b[1;5D");
    tui.type_bytes(b"\x1b[1;5D");
    // Something the pane WILL answer, sent after them, so this waits on a real event rather than on
    // a duration: if the arrows had been swallowed the sizes would have moved before this arrives.
    tui.type_bytes(b"done");
    wait_for("the unprefixed keys to reach the pane", || {
        if tui.rows().iter().any(|row| row.contains("done")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });
    assert_eq!(
        pane_sizes(&mut conn, &session),
        vec![(left + 3, BOOT_PTY.1), (right - 3, BOOT_PTY.1)],
        "an arrow with no prefix is the PANE's, so the arrangement did not move",
    );
}

/// **THE GATE for R308: the key table a user opens is the table the user HAS.**
///
/// Every unit test of the view hands it a `Keymap` and asks what rows come out, which says nothing
/// about whether the shipped binary ever opens one — the seam where a whole surface can be absent
/// while the suite stays green, and the seam [`a_prefix_declared_in_the_users_config_reaches_the_
/// client`] exists for one round earlier. So this runs the real client, on a real pseudoterminal,
/// against a real `config.toml`.
///
/// **The prefix is rebound, and that is the discriminator.** The rows must read `C-a z`, not
/// `C-b z`: a view built from `Keymap::default()` — the easiest wrong implementation, and the one a
/// unit test seeded with defaults could never catch — would paint the second. It is the same claim
/// the palette's hint column makes on the other frontend, made here through a shipped binary.
///
/// Three more, each of which has failed some multiplexer before:
///
/// * **The panes come back.** A view that painted over the arrangement and did not repaint on close
///   would leave a screen of stale rows, which is what an overlay costs if the close path forgets.
/// * **Nothing typed at it reaches the shell.** R306 found exactly that leak on the prompt beside
///   this one — a pasted name running in the user's shell — so the surface added a round later is
///   asked the same question. `whoami` is chosen because `cat` would echo it and the assertion after
///   the close would read `livewhoamiX`.
/// * **It scrolls.** The table is longer than 24 rows by construction (a unit test asserts that),
///   so a view that could not scroll would be one that hides half of itself.
#[test]
fn the_key_table_shows_the_users_own_table_and_gives_the_panes_back() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let (_daemon, _sock, _conn, _session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // Typed first, so everything after it is proven to act on a client that was WORKING.
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&mut tui, "live"));

    tui.type_bytes(&[0x01]); // C-a, the prefix this user declared
    tui.type_bytes(b"?");
    wait_for("the key table to open on the user's own prefix", || {
        let rows = tui.rows();
        let shows = |want: &str| rows.iter().any(|row| row.contains(want));
        if shows("keys") && shows("C-a z") && shows("zoom-pane") {
            Ok(())
        } else {
            Err(format!("{rows:?} (client: {})", tui.liveness()))
        }
    });
    assert!(
        !tui.rows().iter().any(|row| row.contains("C-b z")),
        "the view must show the prefix in force, not the default one: {:?}",
        tui.rows(),
    );

    // IT SCROLLS. The vocabulary section is past the bottom of a 24-row terminal, so reaching it is
    // a statement that the page key moved the view rather than that the rows happened to fit.
    let vocabulary = sprag_host::keyhelp::KeyHelp::VOCABULARY_HEADING;
    assert!(
        !tui.rows().iter().any(|row| row.contains(vocabulary)),
        "the last section must be off screen before the scroll, or paging proves nothing: {:?}",
        tui.rows(),
    );
    tui.type_bytes(b"\x1b[6~"); // PageDown
    tui.type_bytes(b"\x1b[6~");
    tui.type_bytes(b"\x1b[6~");
    wait_for("the page key to reach the end of the table", || {
        if tui.rows().iter().any(|row| row.contains(vocabulary)) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    // NOT ONE CHARACTER REACHES THE SHELL while the table is up.
    tui.type_bytes(b"whoami");

    tui.type_bytes(b"q");
    // The panes come back, and the row underneath is the one the client left there.
    wait_for("the panes to come back", || painted(&mut tui, "live"));

    // The pane is still the keyboard's, and it never saw the six characters above: `cat` echoes
    // what reaches it, so a leak would read `livewhoamiX` here.
    tui.type_bytes(b"X");
    wait_for("the pane to have the keyboard again", || {
        painted(&mut tui, "liveX")
    });
}

/// **A REPAINT DOES NOT WIPE THE KEY TABLE** — R306's failure, on the surface added after it.
///
/// A resize or a host wake drew the frame straight over the prompt while that client went on eating
/// every keystroke, and the fix was to draw the overlay from `paint` itself. This is the same claim
/// one surface over, and it is a test OF ITS OWN because of what it took to make it discriminate.
///
/// **A resize does not drive it and neither does a wake with a quiet pane.** R306 recorded why and
/// the first version of this test measured it again: a terminal keeps glyphs nobody writes over, so
/// an overlay survives a repaint that draws nothing where it is. What is needed is a pane that
/// PRINTS — output landing in the cells the table is drawn over — so the rows can only still be
/// there if the paint path put them back. Hence a boot program that ticks on its own, which is also
/// why this cannot share the `cat` the leak test needs.
///
/// REVERT-PROOF: drop the `Overlay::Showing` arm from `paint` and this fails; the version of this
/// assertion that used a resize passed with that arm gone, which is what a vacuous test looks like.
#[test]
fn a_pane_printing_underneath_does_not_wipe_the_key_table() {
    let (_daemon, _sock, _conn, _session, mut tui) = attached_client_with(
        Tui::attach,
        &["sh", "-c", "while :; do printf 'tick\\n'; sleep 0.1; done"],
    );
    wait_for("the pane to be printing", || {
        if tui.rows().iter().any(|row| row.contains("tick")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    tui.type_bytes(&[0x02]); // prefix
    tui.type_bytes(b"?");
    wait_for("the key table to open", || {
        if tui.rows().iter().any(|row| row.contains("zoom-pane")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    // A dozen ticks land under the table, each one a repaint of the cells it occupies.
    std::thread::sleep(Duration::from_millis(1_200));
    let rows = tui.rows();
    assert!(
        rows.iter().any(|row| row.contains("zoom-pane")),
        "the table must survive the repaints its own pane caused: {rows:?}",
    );
    assert!(
        !rows.iter().any(|row| row.contains("tick")),
        "and it must still be OPAQUE — a pane showing through is the same defect half-fixed: \
         {rows:?}",
    );
}
