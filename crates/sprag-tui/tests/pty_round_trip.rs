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
use sprag_host::wire::{FULL_TEXT_SLOT, PANES_SLOT, SESSIONS_SLOT, SPLIT_ACTION};
use sprag_host::{mux_action_path, pane_input_path};
use sprag_rpc::HostConn;
use sprag_vt::{Emulator, InputModes, MouseProtocol, VtPort};

/// How long any single condition may take before the test calls it a failure.
///
/// Generous on purpose: it is a deadline for a HANG, not a guess at how long the work takes. Every
/// wait returns the instant its condition holds, so raising this costs a passing run nothing and
/// only buys a loaded machine room to finish.
const DEADLINE: Duration = Duration::from_secs(15);

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
    let path = PathBuf::from(env!("CARGO_BIN_EXE_sprag-tui"))
        .parent()
        .expect("the built sprag-tui has a directory")
        .join("sprag-term");
    assert!(
        path.exists(),
        "{} is not built. This test drives the daemon binary, which belongs to another package, \
         so cargo does not build it for `-p sprag-tui` alone — run `cargo test --workspace`, or \
         `cargo build -p sprag-host --bin sprag-term` first.",
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

/// Spawn a daemon whose boot pane runs `cat` — an echo that keeps its PTY open, so a keystroke that
/// arrives comes straight back and a pane that is idle stays alive.
fn spawn_daemon() -> (Daemon, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(sprag_term_bin())
        .arg("--size")
        .arg(format!("{}x{}", BOOT_PANE.0, BOOT_PANE.1))
        .arg("--")
        .arg("cat")
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
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

/// What the DAEMON says pane 0 of the session holds — scrollback and visible text together.
///
/// The other half of every screen diagnostic: a client painting nothing and a pane holding nothing
/// look identical from the client's terminal, and they are opposite bugs.
fn pane_text(conn: &mut HostConn, session: &str) -> String {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": pane_input_path(0, FULL_TEXT_SLOT) }),
    )
    .ok()
    .and_then(|value| value.as_str().map(str::to_owned))
    .unwrap_or_else(|| "<unreadable>".to_owned())
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
        let pair = native_pty_system()
            .openpty(PtySize {
                cols: BOOT_PTY.0,
                rows: BOOT_PTY.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open a pseudoterminal");

        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sprag-tui"));
        command.env("SPRAG_GUI_HOST_SOCK", sock);
        command.env("SPRAG_GUI_SESSION", session);
        // Hermetic: if the connect above ever failed, the client would spawn a daemon of its own,
        // and it must be THIS build's rather than whatever is on the tester's PATH.
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
    let (daemon, sock) = spawn_daemon();
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);
    let tui = Tui::attach(&sock, &session);
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

/// The prefix key, as the byte a terminal sends for `Ctrl-B`.
const PREFIX: &[u8] = &[0x02];

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

/// The prefix table ends the client, and the SESSION outlives it — the difference between a
/// multiplexer and a terminal emulator.
///
/// The detach is driven as the two keystrokes a user types, through the same decoder every other
/// key crosses, so this is the prefix mechanism observed in the shipped binary rather than in the
/// unit test of the routing function.
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
