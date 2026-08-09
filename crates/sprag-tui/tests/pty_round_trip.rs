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

use serde_json::{Value, json};
use sprag_host::keymap::{Keymap, PrefixMode, Routed};
use sprag_host::wire::{
    FULL_TEXT_SLOT, KILL_SESSION_ACTION, LAYOUT_SLOT, NEW_SESSION_ACTION, NEW_WINDOW_ACTION,
    PANES_SLOT, RENAME_SESSION_ACTION, SELECT_PANE_ACTION, SESSIONS_SLOT, SPLIT_ACTION, TREE_SLOT,
    WINDOWS_SLOT,
};
use sprag_host::{mux_action_path, pane_input_path};
use sprag_rpc::HostConn;
use sprag_terminal::CommandBuilder;
use sprag_terminal::pty::Pty;
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

/// The rectangle the PANES get out of that terminal — one row less, because the client reserves the
/// bottom row for its status line (R316, `sprag_tui::Split`).
///
/// **A separate constant and not `BOOT_PTY` with a comment**, because the difference is the whole
/// content of a claim this file makes forty times: the client reports the area it can actually give
/// the panes, so the arbitrated window is that area and not the terminal. A test that expected the
/// terminal's own height would be asserting that the status row is drawn OVER a pane's last line.
const BOOT_PANES: (u16, u16) = panes_of(BOOT_PTY);

/// The rectangle a `sprag-tui` on a `terminal`-sized pseudoterminal gives its PANES — the terminal
/// less its status row.
///
/// ONE derivation, so a test that resizes a client and then asserts what the daemon arbitrated is
/// stating the client's rule rather than re-deriving it. `sprag_tui::Split::of` is the rule; this is
/// what it means for a caller that only wants the number.
const fn panes_of(terminal: (u16, u16)) -> (u16, u16) {
    (terminal.0, terminal.1 - 1)
}

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

/// Every session the daemon holds, by name, in its own order — what a test about a session being
/// BORN or ENDING asserts on.
fn session_names(conn: &mut HostConn) -> Vec<String> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )
    .ok()
    .and_then(|value| value.as_array().cloned())
    .unwrap_or_default()
    .iter()
    .filter_map(|row| Some(row["name"].as_str()?.to_owned()))
    .collect()
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

/// Every whitespace character removed — the only sound reading of RENDERED text for a needle that
/// may be WRAPPED.
///
/// ⚠⚠ **A WORD ON A 40-COLUMN PANE IS NOT A WORD IN ITS TEXT.** Measured at 4x oversubscription:
/// a pane split off an 80-column terminal holds `"coin@host:~$ elsewhe\nre"`, so
/// `contains("elsewhere")` is FALSE about a screen a person can read the word on. That is a nuisance
/// for a positive check and a **VACUITY for a negative one** — `!contains("elsewhere")` passes for
/// a pane that is showing exactly the thing the assertion exists to forbid, and passes silently.
///
/// **APPLY IT TO THE NEEDLE TOO**, or a multi-word needle stops matching the moment the text is
/// folded. That direction is safe for a negative claim in a way the raw read is not: folding can
/// only make `!contains` STRICTER — it can join two words the screen showed apart and fail — and it
/// can never let a wrapped occurrence through.
///
/// ⚠ **THIS EXISTED AS A LOCAL CLOSURE AT ONE SITE IN THIS FILE**, needle-folding and all, while
/// seven other negative assertions over pane text read raw. R334's shape (a status-row site that
/// folded whitespace, which is why a grep for the quoted sentence could never find it) and R331's
/// (*a sweep fixes the sites that match the pattern, not the ones that matter*). Promoted here so
/// there is one of it.
fn fold(text: &str) -> String {
    text.chars()
        .filter(|glyph| !glyph.is_whitespace())
        .collect()
}

/// [`pane_text_of`] read through [`fold`] — what a NEGATIVE claim about a pane's screen must use.
fn pane_words(conn: &mut HostConn, session: &str, pane: u64) -> String {
    fold(&pane_text_of(conn, session, pane))
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
    // CLOSE THE REPEAT WINDOW FIRST, and this is the one place it is enforced rather than
    // remembered. R308 registered the hazard and left it to each caller: after an `-r` binding the
    // prefix table stays armed for `repeat-time`, so a character typed inside it is read as a
    // PREFIX key and never reaches the pane. The probe below is `.`, and R310 bound `prefix .` to
    // `move-window --before` — which opens a PROMPT that then eats every following character, so
    // the two arrow tests hung for the full deadline rather than failing on one lost keystroke.
    //
    // ⚠ This SLEPT `repeat-time + POLL` and the comment beside it argued that sleeping was honest
    // here because nothing observable moves when a timer in another process expires. The argument
    // was wrong in its premise rather than in its reasoning: the window does not run from the sleep,
    // it runs from the moment the acting KEY ARRIVED at the client, which is earlier than anything
    // this side saw — so the margin was never `POLL`, it was `POLL` minus an unknown. An ACT ends
    // the mode with no clock in it at all. See [`end_the_repeat_window`].
    end_the_repeat_window(tui);
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
fn wait_for(what: &str, observe: impl FnMut() -> Result<(), String>) {
    wait_bounded(what, observe, String::new);
}

/// [`wait_for`] with a STANDING diagnostic the condition itself cannot supply, rendered once at the
/// deadline — see [`Tui::wait_for`], which is the only caller that has one.
///
/// Once, and not per poll, because a failing wait takes [`DEADLINE`] over [`POLL`] looks and this
/// binary's tests run in parallel: a diagnostic built two thousand times is load applied to the
/// other tests' own margins (R343 measured that shape breaking a debounce gate on the macOS runner).
/// The condition's own `Err` is still the LAST one taken rather than a fresh reading, which is the
/// property [`wait_for`]'s doc explains.
fn wait_bounded(
    what: &str,
    mut observe: impl FnMut() -> Result<(), String>,
    standing: impl Fn() -> String,
) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = "nothing was observed at all".to_owned();
    while Instant::now() < deadline {
        match observe() {
            Ok(()) => return,
            Err(state) => last = state,
        }
        std::thread::sleep(POLL);
    }
    panic!(
        "timed out after {DEADLINE:?} waiting for {what}\n  last observation: {last}{}",
        standing(),
    );
}

/// `Ok` when `got` is what was wanted, else `got` rendered as [`wait_for`]'s diagnostic.
fn settled<T: PartialEq + std::fmt::Debug>(got: T, want: &T) -> Result<(), String> {
    if got == *want {
        Ok(())
    } else {
        Err(format!("{got:?}"))
    }
}

/// `Ok` when the client's top row reads `want`, else [`Tui::picture`] as the diagnostic.
fn painted(tui: &Tui, want: &str) -> Result<(), String> {
    if tui.row(0) == want {
        return Ok(());
    }
    Err(tui.picture())
}

/// `Ok` when the client's STATUS ROW reads `want`, else [`Tui::picture`] as the diagnostic.
///
/// # ⚠ THE ROW ALONE COST THREE ROUNDS, AND THIS IS WHAT IT COST THEM
///
/// Forty-four waits here used to read this row through [`settled`], whose diagnostic is the row and
/// nothing else. [`painted`] has stated since it was written that one row cannot separate the three
/// ways such an assertion fails — *painted something else*, *painted nothing*, *GONE* — and a client
/// that exited has left the alternate screen, so every row of it reads blank rather than stale.
///
/// **Measured, on a real CI failure**: `timed out after 45s … last observation: ""`. That empty
/// string is a client that had EXITED, and it was carried across R343, R344 and R345 as an
/// unattributable "pty flake" — three occurrences, two platforms, three different tests — because
/// the one thing that separates a hang from a departure was the one thing not printed. The run that
/// finally named it (R345) did so from [`Tui::status_trail`], a LOSSLESS record that was sitting in
/// this same struct the whole time and that no status-row wait ever consulted.
///
/// So the picture is not a nicety here, it is the instrument: an assertion's message is part of it,
/// and a gate that cannot say what it saw costs a reproduce cycle every time it fires.
fn says(tui: &Tui, want: &str) -> Result<(), String> {
    if tui.row(STATUS_ROW) == want {
        return Ok(());
    }
    Err(tui.picture())
}

/// [`says`] for a row asserted by its CONTENTS rather than whole — the same diagnostic, because the
/// three failures it cannot tell apart are the same three.
fn mentions(tui: &Tui, needle: &str) -> Result<(), String> {
    if tui.row(STATUS_ROW).contains(needle) {
        return Ok(());
    }
    Err(tui.picture())
}

/// Wait until `read` answers the SAME value twice in a row, a settle window apart, and answer it.
///
/// A "STILL" test rather than a "DONE" test, and the file already records why that distinction
/// matters (the smoke's `8 reads of a NUMBER` intermittent). What it is FOR here is making an
/// assertion about a wake able to FAIL: a fixture whose own setup still has a change in flight
/// delivers the next thing on that change's wake, so a check that a delivery woke a client would
/// pass with the delivery's wake deleted. Settling first leaves the act under test as the only
/// thing that can move anything.
fn wait_for_still<T: PartialEq + Copy + std::fmt::Debug>(mut read: impl FnMut() -> T) -> T {
    let deadline = Instant::now() + DEADLINE;
    let mut last = read();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(150));
        let now = read();
        if now == last {
            return now;
        }
        last = now;
    }
    panic!("timed out after {DEADLINE:?} waiting for a reading to stop moving (last {last:?})");
}

/// Close any repeat window this client is holding, so the NEXT prefix is a prefix.
///
/// # Why a keystroke and not a sleep
///
/// A `-r` binding leaves the prefix table armed for `repeat-time`, and inside that window `C-b` is
/// `send-prefix` — tmux's own rule — so a test that starts a second gesture with a prefix has its
/// prefix typed into the pane and the key behind it follows. The obvious answer is to wait the
/// window out, and this file did, nine times. **That clock cannot be read from here.** The window
/// runs from the moment the acting key ARRIVED at the client, an instant no test observes, and the
/// margin left over is whatever the sleep exceeds `repeat-time` by — which is why a run at 2x CPU
/// oversubscription found two of those sites failing with the whole gesture in the pane.
///
/// One keystroke ends it instead, with no clock anywhere: a key bound in NEITHER table routes
/// `ToPane`, and `Routed::next` makes that `PrefixMode::ToPane`. The window is shut when this
/// returns, whatever was left of it, because the mode is one keystroke long by construction.
///
/// # ...and why it takes the character back
///
/// The key reaches the PANE, which is the price of closing the window without a clock — and a
/// character left on the child's line is not free. Measured at 4x CPU oversubscription: with four
/// `q`s standing on a shell's line, the unprefixed `C-Left`s that a later claim sends to that same
/// shell moved the cursor over them, and the word typed to prove the keys had arrived was inserted
/// in the middle of them. So the erase is part of the gesture, not tidiness: the line the child
/// holds is exactly what it was.
///
/// Both keys go out in ONE write, which is what makes the erase reliable rather than another race —
/// the child's line discipline sees the pair together, and neither of them is the client's.
fn end_the_repeat_window(tui: &mut Tui) {
    // ASSERTED, not assumed. A future default binding on either key would silently turn "close the
    // window" into "run something", and the failure would look like the flake this replaces.
    let bound_nowhere = |mode, name| {
        matches!(
            Keymap::default().route(
                mode,
                Instant::now(),
                name,
                sprag_input::Modifiers::default()
            ),
            Routed::ToPane,
        )
    };
    let armed = PrefixMode::Repeating {
        until: Instant::now() + Duration::from_secs(60),
    };
    assert!(
        bound_nowhere(armed, "q"),
        "the window-closing key must be bound in neither table, or this closes nothing",
    );
    assert!(
        bound_nowhere(PrefixMode::ToPane, "Backspace"),
        "the erase must be the CHILD's key once the window is shut, or it never reaches the line",
    );
    tui.type_bytes(b"q\x7f");
}

/// `Ok` when this client has PAINTED a status row containing `want` in SOME frame, else every
/// distinct row it has painted.
///
/// **The sound reading for a message that EXPIRES**, and the reason is the one [`Tui::status_trail`]
/// is written around: a `display-time` message is on the row for its deadline and then gone, so
/// `tui.row(STATUS_ROW)` inside a [`wait_for`] can only see it by happening to look while it is
/// there — and under load the whole test process is descheduled for longer than the message lives.
/// The trail is a history of the client's frames, so this question has no clock in it.
///
/// It is a STRICTLY WEAKER claim than the one a poll makes, and that is why it is not the default:
/// this says the row HELD `want` at some point, where `says(&tui, &landing)` says
/// the row IS the landing now. Use it where the sentence is the thing under test and expiring is
/// what it is FOR; a claim about where a client came to rest is a claim about a state that does not
/// pass, and a poll is the honest instrument for that.
fn announced(tui: &Tui, want: &str) -> Result<(), String> {
    if tui.status_rows().iter().any(|row| row.contains(want)) {
        return Ok(());
    }
    Err(format!("the rows painted were {:?}", tui.status_rows()))
}

/// `Ok` when SOME row of the client's screen contains `want`, else the whole screen.
///
/// [`painted`] pins the TOP row exactly, which is right for a pane's own output. A prompt is drawn
/// on the LAST row, and how far down that is depends on the harness's terminal height — so a test
/// that named the row would be asserting the geometry rather than the sentence. What matters here
/// is that the user can read it.
fn shows(tui: &mut Tui, want: &str) -> Result<(), String> {
    if tui.rows().iter().any(|row| row.contains(want)) {
        return Ok(());
    }
    Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
}

// ----- the client, on a pseudoterminal -----

/// A live `sprag-tui` on a pseudoterminal, plus everything needed to drive it and to see what it
/// painted.
struct Tui {
    /// Held so the pty stays open and can be resized; dropping it would EOF the client's input.
    master: Pty,
    /// The client's input end. Held for the same reason: a dropped writer is an EOF.
    writer: std::fs::File,
    /// The client process. Behind a lock so [`Tui::liveness`] is a `&self` question — see it for
    /// why that matters to every wait in this file.
    child: Mutex<Child>,
    /// Everything the client has written, as the emulator that consumed it — see the module docs
    /// for why the assertions read a screen and not a byte stream.
    screen: Arc<Mutex<Emulator>>,
    /// How many bytes the client has written, which is how [`Tui::holds_the_terminal`] knows it is
    /// safe to type.
    written: Arc<AtomicUsize>,
    /// Every DISTINCT status row the client has painted, in order, recorded by the READER THREAD at
    /// each FRAME the client closed.
    ///
    /// ⚠ A POLL CANNOT WITNESS A STATE THAT PASSES BETWEEN TWO LOOKS. A displayed message is on the
    /// row for its `display-time` and then gone, and a test loop that samples every [`POLL`] sees it
    /// only if it happens to look while it is there — which under full-suite load it does not, since
    /// the whole process can be descheduled for longer than the message lives. That is not a timing
    /// constant to raise: sampling a transient is unsound at ANY interval, and the two earlier
    /// attempts at this gate (R326's fixed 3-second window, R327's `rows_until_settled`) each
    /// tightened the sampler without leaving the sampling.
    ///
    /// **The granularity is the CLIENT'S OWN FRAME, which is why this is a history and not a
    /// sample.** Recording per `read` removed the poll and kept the sampling: the OS decides how
    /// much of a busy client's output comes back at once, so successive frames coalesced into one
    /// batch and only the last of them was recorded — measured at 2x CPU oversubscription, where
    /// this trail came back as `["[beta] 0:0*"]`, holding neither the starting row nor the message.
    /// Now that the client brackets each frame in DEC private mode 2026 (see its `Screen`), the
    /// reader splits a batch at the frame CLOSES inside it and records one row per frame. A read
    /// carrying four frames records four rows; a read carrying half a frame records none until the
    /// rest arrives. Nothing the OS does to the read boundaries can lose a frame, so a row this
    /// client painted is a row this trail holds.
    ///
    /// The pair with [`Tui::said`] stands, and is now about POSITION rather than about soundness:
    /// this says WHERE — the status row and not a pane — where the byte stream cannot.
    status_trail: Arc<Mutex<Vec<String>>>,
    /// Every byte the client has ever written.
    ///
    /// The only LOSSLESS record of what a client said. A screen is a state that gets overwritten
    /// and can therefore be missed by any observer that is not looking at the instant it holds;
    /// the byte stream is a history, so "did this client ever say X" is answered without a clock,
    /// a poll or a scheduling assumption anywhere in it.
    ///
    /// It cannot say WHERE the text was painted, which is why it does not replace the trail: the
    /// pair is "it said this" (here) plus "it came to rest on that" (the trail's last row).
    transcript: Arc<Mutex<Vec<u8>>>,
}

impl Drop for Tui {
    fn drop(&mut self) {
        // A test that failed before detaching leaves a client attached to a daemon that is about to
        // be killed; ending it here keeps a failure from stranding a process on the machine.
        let mut child = self.child.lock().expect("the child mutex");
        let _ = child.kill();
        let _ = child.wait();
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
        let mut pair = Pty::open(BOOT_PTY.0, BOOT_PTY.1).expect("open a pseudoterminal");

        // Hermetic: if the connect ever failed, the client would spawn a daemon of its own, and it
        // must be THIS build's rather than whatever is on the tester's PATH.
        command.env("SPRAG_GUI_HOST_BIN", sprag_term_bin());
        // The client loads terminfo from `TERM`; naming one keeps the sequences it writes
        // independent of the terminal the test suite happens to be running in.
        command.env("TERM", "xterm-256color");

        // `spawn` takes the slave and drops it, so the master reads EOF when the child exits.
        // The second answer is the child's cgroup join, which is `NotAsked` here and everywhere
        // else that offers no cgroup — this test drives a CLIENT, not a pane of the daemon's.
        let (child, _joined) = pair
            .spawn(&command, None)
            .expect("spawn sprag-tui on the pty");

        let screen = Arc::new(Mutex::new(Emulator::new(BOOT_PTY.0, BOOT_PTY.1)));
        let written = Arc::new(AtomicUsize::new(0));
        let mut reader = pair.reader().expect("clone the pty reader");
        let writer = pair.writer().expect("take the pty writer");
        let status_trail = Arc::new(Mutex::new(Vec::new()));
        let transcript = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn({
            let (screen, written) = (Arc::clone(&screen), Arc::clone(&written));
            let status_trail = Arc::clone(&status_trail);
            let transcript = Arc::clone(&transcript);
            move || {
                let mut buf = [0u8; 8192];
                let mut splitter = FrameSplitter::new();
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    transcript
                        .lock()
                        .expect("the transcript mutex")
                        .extend_from_slice(&buf[..n]);
                    {
                        let mut emulator = screen.lock().expect("the screen mutex");
                        let mut from = 0;
                        for end in splitter.closes_in(&buf[..n]) {
                            // The frame's own bytes, terminator included: what the client presented.
                            emulator.advance(&buf[from..end]);
                            from = end;
                            // Read INSIDE the same lock the frame was applied under, so the trail
                            // cannot record a row from a state no frame produced. See `status_trail`
                            // for why this is here and not in a polling loop.
                            let row = VtPort::screen(&*emulator)
                                .row_text(STATUS_ROW)
                                .trim_end()
                                .to_owned();
                            let mut trail = status_trail.lock().expect("the status trail mutex");
                            if trail.last() != Some(&row) {
                                trail.push(row);
                            }
                        }
                        // Whatever follows the last close is a frame the client has NOT presented
                        // yet, or output that belongs to no frame at all (a mode sequence, a
                        // forwarded notification). Applied, so a poll still sees the newest state;
                        // not recorded, because the trail counts frames.
                        emulator.advance(&buf[from..n]);
                    }
                    // AFTER the emulator, so a reader that sees the count move can also see
                    // everything that moved it.
                    written.fetch_add(n, Ordering::Release);
                }
            }
        });

        Self {
            master: pair,
            writer,
            child: Mutex::new(child),
            screen,
            written,
            status_trail,
            transcript,
        }
    }

    /// Every distinct status row this client has painted, in order — see [`Tui::status_trail`].
    fn status_rows(&self) -> Vec<String> {
        self.status_trail
            .lock()
            .expect("the status trail mutex")
            .clone()
    }

    /// Whether this client has EVER written `text` — see [`Tui::transcript`].
    ///
    /// The sound way to ask "did it say that" about a sentence which expires: the answer does not
    /// depend on when anybody looked. A client repaints a whole row, so the sentence appears in the
    /// stream contiguously; a wrapped one would not, and no message this suite asserts on is near
    /// the width.
    fn said(&self, text: &str) -> bool {
        self.transcript
            .lock()
            .expect("the transcript mutex")
            .windows(text.len())
            .any(|window| window == text.as_bytes())
    }

    /// Everything this client wrote, split into what it painted INSIDE an atomic frame (DEC private
    /// mode 2026) and what it wrote OUTSIDE one — plus how many frames it closed.
    ///
    /// Read off [`Tui::transcript`] for the reason that field exists: a screen is a state that gets
    /// overwritten, and the question here is about every byte the client has ever emitted rather
    /// than about any moment. Nothing is dropped — every byte lands in exactly one of the two — so
    /// the two halves replayed together are the whole conversation, which is what lets an assertion
    /// about one of them mean something about all of it.
    fn framed_and_loose(&self) -> (Vec<u8>, Vec<u8>, usize) {
        const OPEN: &[u8] = FRAME_OPEN;
        const CLOSE: &[u8] = FRAME_CLOSE;
        let bytes = self
            .transcript
            .lock()
            .expect("the transcript mutex")
            .clone();
        let (mut framed, mut loose, mut frames) = (Vec::new(), Vec::new(), 0);
        let (mut at, mut inside) = (0, false);
        while at < bytes.len() {
            if !inside && bytes[at..].starts_with(OPEN) {
                inside = true;
                at += OPEN.len();
            } else if inside && bytes[at..].starts_with(CLOSE) {
                inside = false;
                frames += 1;
                at += CLOSE.len();
            } else {
                if inside { &mut framed } else { &mut loose }.push(bytes[at]);
                at += 1;
            }
        }
        (framed, loose, frames)
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
            .resize(cols, rows, 0, 0)
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

    /// Whether the client has asked THIS terminal to report focus changes (DEC private mode 1004) —
    /// read off the emulator that consumed the mode, like every other assertion here.
    ///
    /// The enabling half of R319: a client that never asked cannot know the person left, so this is
    /// the claim that fails first if the outward path is torn out at its source.
    fn asked_for_focus_reports(&self) -> bool {
        self.local_modes().focus_tracking
    }

    /// The desktop notification the client FORWARDED to this terminal, and how many it has sent —
    /// the emulator standing in for the person's own terminal emulator.
    ///
    /// This is why the reading emulator is sprag's own: the bytes a client writes outward are
    /// exactly the bytes a pane's child writes inward, so the surface that latches one latches the
    /// other. What a real kitty would pop up as a toast, this reads back as a `Notification` —
    /// including the URGENCY, which is the part no rival forwards at all.
    fn forwarded(&self) -> (Option<sprag_vt::Notification>, u64) {
        let emulator = self.screen.lock().expect("the screen mutex");
        (
            VtPort::notification(&*emulator).cloned(),
            VtPort::notification_seq(&*emulator),
        )
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

    /// The column `col` read down every row of the screen, INCLUDING the client's status row.
    fn column(&self, col: u16) -> String {
        let rows = {
            let emulator = self.screen.lock().expect("the screen mutex");
            VtPort::screen(&*emulator).rows()
        };
        (0..rows)
            .map(|row| self.cell(col, row).unwrap_or(' '))
            .collect()
    }

    /// The column `col` read down the rows the PANES occupy — how a divider is asserted, and the
    /// diagnostic when one is not where it should be.
    ///
    /// The last row is excluded because it is the client's own status line (R316), which is outside
    /// the tiling by construction: a divider that reached it would be the client painting chrome
    /// over its own sentence. Asserting over the whole screen instead would make every divider test
    /// fail on a glyph the tiling never claimed — which is exactly what it did before this existed.
    fn pane_column(&self, col: u16) -> String {
        let column = self.column(col);
        column.chars().take(column.chars().count() - 1).collect()
    }

    /// Whether the client is still running, for a diagnostic — a client that has EXITED left the
    /// alternate screen on its way out, so its last painted frame is not what a reader sees.
    ///
    /// **`&self`, and that is the whole reason the child is behind a lock.** `Child::try_wait` needs
    /// `&mut`, which made this a `&mut self` question — and a wait that reads the screen holds the
    /// client immutably, so forty-four of them could not ask it even though every one of them wanted
    /// the answer (see [`says`]). Reaping is not what the caller is doing; asking is.
    fn liveness(&self) -> String {
        match self.child.lock().expect("the child mutex").try_wait() {
            Ok(None) => "running".to_owned(),
            Ok(Some(status)) => format!("EXITED {status:?}"),
            Err(error) => format!("unknown ({error})"),
        }
    }

    /// EVERYTHING this harness knows about what the client has done — the ONE diagnostic every wait
    /// on this client's screen fails with.
    ///
    /// Three facts, and no two of them answer the same question:
    ///
    /// * the SCREEN as it stands, which says what a person would be looking at;
    /// * the LOSSLESS trail of every status row ever painted ([`Tui::status_trail`]), which says
    ///   what the client did while nobody was looking — the half a screen can never hold, since a
    ///   screen is a state that gets overwritten;
    /// * whether the client is still THERE, which is what tells a blank screen that was painted
    ///   from a blank screen that is simply gone.
    ///
    /// Drop any one of them and a real failure becomes unattributable; R345 measured all three
    /// being needed to explain one 45-second timeout.
    fn picture(&self) -> String {
        format!("{:?} {}", self.rows(), self.standing())
    }

    /// The two facts about this client that a CONDITION can never put in its own diagnostic: what it
    /// painted while nobody was looking, and whether it is still there.
    ///
    /// Split out of [`Tui::picture`] so [`Tui::wait_for`] can append it to a bespoke observation
    /// without printing the screen twice — most of those already print the rows they are about, and
    /// neither of these.
    fn standing(&self) -> String {
        format!(
            "(status painted {:?}, client: {})",
            self.status_rows(),
            self.liveness(),
        )
    }

    /// [`wait_for`], for a condition about THIS client — the deadline carries [`Tui::standing`]
    /// whatever the condition itself chose to say.
    ///
    /// **A wait about a client is a method ON the client, so that it cannot be spelled without
    /// one.** Twenty-eight waits here read this client's screen through a bespoke condition, and
    /// every one of them could say what was painted and none of them could say whether anybody was
    /// still painting — the distinction that took three rounds to make about the status row (see
    /// [`says`]). Nothing enforced it, because the wait was a free function and the client was
    /// simply not in the room.
    fn wait_for(&self, what: &str, observe: impl FnMut() -> Result<(), String>) {
        wait_bounded(what, observe, || self.standing());
    }

    /// Wait for the client to exit, and fail rather than block if it does not.
    ///
    /// Bounded deliberately: `Child::wait` has no deadline, so a client that failed to act on a
    /// detach would stall the whole suite instead of failing it — which is how this test first
    /// behaved, and a gate that can hang is a gate nobody will keep running.
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            match self
                .child
                .lock()
                .expect("the child mutex")
                .try_wait()
                .expect("wait for sprag-tui")
            {
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
/// [`attached_client`] with `config` reaching BOTH processes — the daemon and the client.
///
/// ⚠ **`attached_client_with` gives the config to the CLIENT ONLY**, which is right for a claim about
/// a keymap (the keymap is a client's) and WRONG for any claim that also depends on a daemon-side
/// option: the daemon then reads the developer's own `~/.config/sprag/config.toml`. R331 wrote a
/// resize gate that way and it passed for that reason — this machine's file happens to say
/// `window-size = "manual"`, which is the very option the test was setting. On a machine without
/// that line it would have failed, which is the recorded shape (R318, R319) arriving a third time.
///
/// So: a test whose subject is an OPTION uses this. A test whose subject is the two processes
/// DISAGREEING gives them one config home each, which is the CLI gate
/// `the_policy_note_comes_from_the_daemon_and_not_from_the_callers_own_config`.
fn attached_client_under(
    config: &ConfigHome,
    program: &[&str],
) -> (Daemon, PathBuf, HostConn, String, Tui) {
    let home = config.as_str().to_owned();
    attached_client_using(
        |sock, session| Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", home.as_str())]),
        program,
        Some(config.as_str()),
    )
}

fn attached_client_with(
    launch: impl FnOnce(&Path, &str) -> Tui,
    program: &[&str],
) -> (Daemon, PathBuf, HostConn, String, Tui) {
    attached_client_using(launch, program, None)
}

/// The one boot-and-attach both fixtures above share, with the daemon's config home as the axis
/// they differ on.
fn attached_client_using(
    launch: impl FnOnce(&Path, &str) -> Tui,
    program: &[&str],
    daemon_config: Option<&str>,
) -> (Daemon, PathBuf, HostConn, String, Tui) {
    let (daemon, sock) = spawn_daemon_with_config(program, daemon_config);
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
        || settled(pane_size(&mut conn, &session), &Some(BOOT_PANES)),
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
        painted(&tui, "hello")
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
        painted(&tui, "abc")
    });

    // The alternative Backspace spelling — normalised by the host, not by this terminal.
    tui.type_bytes(&[0x08]);
    wait_for("the erase to come back painted", || {
        painted(&tui, "ab").map_err(|screen| {
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
        BOOT_PANE, BOOT_PANES,
        "the pane must start at a size the terminal is not, or nothing below is a measurement",
    );
    wait_for("the attach to size the pane to the terminal", || {
        settled(pane_size(&mut conn, &session), &Some(BOOT_PANES))
    });

    // Something on screen BEFORE the reshape, so the assertion after it is about content and not
    // about an empty pane agreeing with an empty pane.
    tui.type_bytes(b"wide");
    wait_for("the typed text to come back painted", || {
        painted(&tui, "wide")
    });

    let terminal = (100, 30);
    let resized = panes_of(terminal);
    tui.resize(terminal.0, terminal.1);
    wait_for("the window change to reach the pane", || {
        settled(pane_size(&mut conn, &session), &Some(resized))
    });

    // ...and the pane's content survives being reshaped under it. A resize that reached the pty and
    // lost the screen would pass every assertion above and be unusable.
    wait_for(
        "the reshaped pane to still hold what the child said",
        || {
            painted(&tui, "wide").map_err(|screen| {
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

    // `attached_client_via` has already settled the pane at BOOT_PANES, which is claim 1: the CLI
    // named the session and the socket, or there would be no attached client to have sized.
    assert_ne!(
        BOOT_PANE, BOOT_PANES,
        "the pane must start at a size the terminal is not, or nothing above is a measurement",
    );

    let terminal = (100, 30);
    let resized = panes_of(terminal);
    tui.resize(terminal.0, terminal.1);
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

/// **The window ORDER, reached from a KEY** — a real `sprag-tui` on a real PTY presses `prefix >`,
/// `prefix <` and `prefix .`, and the DAEMON's own window list is what is asserted.
///
/// Measured at `1ba0164` before a line was written: the order was WALKED by `prefix n`/`p`, PAINTED
/// by the GUI's window strip, and changeable by nothing in the product — no key, no CLI verb, no
/// wire action. The rival's only gesture for it is a MOUSE DRAG in its tab bar
/// (`MouseAction::MoveTab`; herdr `9a4ce5e1` has no CLI `tab move` and no `KeysConfig` binding), so
/// this test is the half neither product had.
///
/// ⚠ **The session is walked BACK onto window "0" before anything is moved, and that is
/// load-bearing rather than tidy.** `prefix c` selects what it creates, so after two of them the
/// scope is a window whose pane is a SHELL — and `pane_text_of(.., 0)` reads the SCOPED window, so
/// the `cat` pane is `"<unreadable>"` from there. The first version of this test compared two
/// unreadable strings and called it "not one character reached the pane": a VACUOUS assertion, of
/// exactly the class this project has now caught five times, and the harness is what found it.
#[test]
fn the_order_keys_move_a_window_and_the_prompt_key_anchors_it() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&tui, "before"));
    // Three windows, so a step has somewhere to go and an anchor has something to name.
    for _ in 0..2 {
        tui.type_bytes(PREFIX);
        tui.type_bytes(b"c");
    }
    wait_for("three windows to exist", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![
                ("0".to_owned(), false),
                ("1".to_owned(), false),
                ("2".to_owned(), true),
            ],
        )
    });
    // Back onto "0" — the `cat` window, whose pane is the only one that can be read unambiguously.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"n");
    wait_for("the session to be back on the cat window", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![
                ("0".to_owned(), true),
                ("1".to_owned(), false),
                ("2".to_owned(), false),
            ],
        )
    });

    // 1. `prefix >` — one place toward the back. The window the session is ON moves, and the
    // session STAYS ON IT, which is the property a client that recomputed an index would break.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b">");
    wait_for("the key to move the current window one place later", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![
                ("1".to_owned(), false),
                ("0".to_owned(), true),
                ("2".to_owned(), false),
            ],
        )
    });

    // 2. `prefix <` — the other way, and it really is the other way rather than the same key twice.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"<");
    wait_for("the key to move it back one place earlier", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![
                ("0".to_owned(), true),
                ("1".to_owned(), false),
                ("2".to_owned(), false),
            ],
        )
    });

    // 3. `prefix .` — tmux's own default for `move-window`, which there prompts for an INDEX and
    // here asks for a WINDOW NAME. Typed one keystroke at a time and committed with Enter, exactly
    // as the rename prompt beside it is, because it is the same surface.
    //
    // The seed is EMPTY here where a rename's is the current name, so what lands is `2` and not
    // `02`: a prompt that seeded with the subject would name a window that does not exist.
    let shell_before = pane_text_of(&mut conn, &session, 0);
    assert!(
        shell_before.contains("before"),
        "the fixture pane really is readable, or the assertion below says nothing: {shell_before:?}",
    );
    tui.type_bytes(PREFIX);
    tui.type_bytes(b".");
    tui.type_bytes(b"2\r");
    wait_for(
        "the anchored move to reach the daemon's window list",
        || {
            settled(
                windows_of(&mut conn, &session),
                &vec![
                    ("1".to_owned(), false),
                    ("0".to_owned(), true),
                    ("2".to_owned(), false),
                ],
            )
        },
    );
    assert_eq!(
        pane_text_of(&mut conn, &session, 0),
        shell_before,
        "the anchor was typed AT THE CLIENT: not one character of it reached the pane",
    );

    // ...and CANCELLING gives the keyboard back with the order untouched. `C-c` rather than
    // `Escape`, for the reason the rename prompt's own cancel states.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b".");
    tui.type_bytes(b"1");
    tui.type_bytes(b"\x03");
    typing_follows(&mut tui, &mut conn, &session, 0);
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![
            ("1".to_owned(), false),
            ("0".to_owned(), true),
            ("2".to_owned(), false),
        ],
        "a cancelled prompt moves nothing, and the keyboard is the pane's again",
    );
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
    wait_for("the client to be painting", || painted(&tui, "before"));
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

/// **R323's PANE VERB, PRESSED** — `prefix !` on a real client, and the DAEMON's own window list.
///
/// Measured at `b588a41`, before this round: `sprag bind-key F9 break-pane` answered *"break-pane"
/// is not an action* — about a verb the SAME BINARY dispatches, and one whose client-side call
/// (`HostClient::break_pane`) had existed since the GUI palette got the gesture. The keyboard could
/// not say a word the product had. It is tmux's own `prefix !`, so a tmux user's fingers already
/// carry it.
///
/// **The split is what makes the break visible**, and that is not fixture ceremony: breaking a
/// window's ONLY pane moves it into a window of its own, which looks exactly like nothing
/// happening. The window must hold two panes for the act to have an observable half.
///
/// The three assertions each fail differently: a window is CREATED; the pane that moved is the one
/// that had FOCUS (not the window's first); and the client is still alive afterwards.
#[test]
fn the_break_key_puts_the_focused_pane_in_a_window_of_its_own() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    // Typed first, so the key below is proven to act on a client that was WORKING.
    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&tui, "before"));
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), true)],
        "the session boots with one window, which is what makes the break visible",
    );

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    wait_for("the split to give the window two panes", || {
        settled(pane_ids(&mut conn, &session).len(), &2)
    });
    let focused = active_pane(&mut conn, &session).expect("the split selects the pane it made");

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"!");
    wait_for("the break key to give the pane a window of its own", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });
    // `panes` reads the CURRENT window, which the break selects — so this says the new window holds
    // exactly the pane that had focus, and the one left behind is elsewhere.
    assert_eq!(
        pane_ids(&mut conn, &session),
        vec![focused],
        "the pane that moved is the one the user was on, alone in the window it made",
    );
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client is still alive, having broken a pane out from under itself",
    );
}

/// **R323's TWO SESSION VERBS, PRESSED** — and the pair is what proves the FOLLOW.
///
/// `new` and `kill-session` ship BOUND TO NOTHING (tmux binds neither, and the second has the
/// largest blast radius in the vocabulary), so this writes a config that binds them — which makes
/// the test a statement about the whole path a user walks: a `config.toml`, read by a shipped
/// client, reaching a daemon.
///
/// # Why the two are one test
///
/// `new` is TWO acts — create a session, and point this client at it — and the second is invisible
/// to any assertion about the registry: a client that created a session and stayed where it was
/// would leave the same two rows behind. What discriminates is killing "this client's session"
/// NEXT: if the follow did not happen, the kill takes the BOOT session instead, and the row that
/// disappears is the wrong one. Two keys, and only the pair can tell the two worlds apart.
///
/// Measured before this round: both verbs answered *"is not an action"* at `bind-key`, so neither
/// key existed to press.
#[test]
fn the_session_keys_make_a_session_follow_it_and_kill_the_one_they_landed_on() {
    let config = ConfigHome::new(
        "[[bind]]\nkey = \"N\"\naction = \"new\"\n\n[[bind]]\nkey = \"Q\"\naction = \"kill-session\"\n",
    );
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&tui, "before"));
    assert_eq!(
        session_names(&mut conn),
        vec![session.clone()],
        "the daemon holds one session, which is what makes the birth visible",
    );

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"N");
    wait_for("the key to make a second session", || {
        settled(session_names(&mut conn).len(), &2)
    });
    let born = session_names(&mut conn)
        .into_iter()
        .find(|name| *name != session)
        .expect("a session that is not the one we booted on");
    // THE FOLLOW, read from the daemon's own attachment count rather than from this client's
    // screen: the client is the thing under test, so its picture of where it is proves nothing.
    wait_for("the client to be attached to the session it made", || {
        settled(
            (attached(&mut conn, &session), attached(&mut conn, &born)),
            &(0, 1),
        )
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"Q");
    wait_for(
        "the kill key to end the session this client landed on",
        || settled(session_names(&mut conn), &vec![session.clone()]),
    );
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client is alive on the session that is left, having killed the one it was on",
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
    wait_for("the client to be painting", || painted(&tui, "before"));
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
    wait_for("the client to be painting", || painted(&tui, "before"));

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

/// **`prefix x` ASKS, and what it asks NAMES THE WINDOW IT IS ABOUT TO TAKE** — R309.
///
/// tmux's key with tmux's own guard, one level below `prefix &`. The two halves this drives that no
/// unit test can:
///
/// 1. **The sentence a user actually reads is the one the live arrangement produces.** With a
///    sibling pane the question is bare; with the pane alone in its window the SAME key adds a line
///    saying the window ends. tmux cannot do this — its prompt is a fixed string in the binding —
///    and it is the whole reason the guard is worth shipping rather than just the verb.
/// 2. **The cascade reaches the daemon through a keystroke.** The second kill leaves the window
///    with nothing, and the daemon's own window list is what says the window is gone — read from
///    the daemon, never from the client that pressed the key.
///
/// The NO half runs first and discriminates: a client that killed and asked afterwards would pass a
/// yes-only test.
#[test]
fn the_pane_kill_key_says_what_it_will_take_and_takes_it() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&tui, "before"));

    // A second window, so emptying this one cannot end the session the test still has to read.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"c");
    wait_for("a second window to exist", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });
    // Two panes in the window the keys will act on.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    wait_for("a second pane in this window", || {
        settled(pane_ids(&mut conn, &session).len(), &2)
    });
    let pair = pane_ids(&mut conn, &session);

    // WITH A SIBLING: the question is asked and says nothing about a window, because nothing else
    // is about to go. Answered NO.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"x");
    wait_for("the question to be on screen", || {
        shows(&mut tui, "Kill pane")
    });
    assert!(
        shows(&mut tui, "last pane").is_err(),
        "a pane with a sibling is not described as its window's last: {:?}",
        tui.rows(),
    );
    tui.type_bytes(b"n");
    // The pane the SPLIT made, which is the one the session is on and therefore the one a refused
    // kill leaves the keyboard pointed at. `pane` here is an id, not an index.
    typing_follows(&mut tui, &mut conn, &session, pair[1]);
    assert_eq!(
        pane_ids(&mut conn, &session),
        pair,
        "a refused kill takes nothing, and the keyboard is the pane's again",
    );

    // ...and YES takes exactly the one pane, leaving the window.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"x");
    wait_for("the question again", || shows(&mut tui, "Kill pane"));
    tui.type_bytes(b"y");
    wait_for("the answered kill to reach the daemon", || {
        settled(pane_ids(&mut conn, &session).len(), &1)
    });
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), false), ("1".to_owned(), true)],
        "one pane went and the window it was in did not",
    );

    // NOW THE LAST ONE. The same key, and the question grows the line the arrangement earned.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"x");
    wait_for("the question to name the escalation", || {
        shows(&mut tui, "last pane")
    });
    tui.type_bytes(b"y");
    wait_for("the window to go with its last pane", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), true)],
        )
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client survived destroying the window it was projecting",
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
        painted(&tui, "left")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");

    let (near, far) = halves(BOOT_PANES.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });

    tui.wait_for("a divider to stand between the two panes", || {
        let column = tui.pane_column(near);
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
        painted(&tui, "top")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"\"");

    let (near, far) = halves(BOOT_PANES.1);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(BOOT_PANES.0, near), (BOOT_PANES.0, far)],
        )
    });

    tui.wait_for("a divider to stand between the two panes", || {
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
        painted(&tui, "before")
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
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
    // decision rather than a race. Read through [`fold`]: a leak would land at the cursor, which
    // after the echo above can be near the right edge, and a wrapped leak reads as no leak.
    let held = pane_words(&mut conn, &session, 0);
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
        painted(&tui, "before")
    });

    // `-h` puts the panes side by SIDE, so pane 0 is the LEFT one and focus lands on the right.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
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

    // ...AT THE EDGE: nothing moves, and nothing fails — and since R316 the client SAYS SO. That
    // last part is asserted here rather than in a test of its own, because this is the fixture
    // that reaches the edge: a directional arm whose refusal has no driver is the shape the debt
    // question keeps finding.
    tui.type_bytes(PREFIX);
    tui.type_bytes(ARROW_LEFT);
    tui.wait_for("the edge to be reported on the status row", || {
        settled(
            tui.row(BOOT_PTY.1 - 1)
                .contains("select-pane -L: nowhere to go"),
            &true,
        )
        .map_err(|got| format!("{got}: row reads {:?}", tui.row(BOOT_PTY.1 - 1)))
    });
    // ⚠ THE REPEAT WINDOW AGAIN, and this time it was a BINDING that opened the hole rather than a
    // timing change. The arrow above is `-r`, so for `repeat-time` afterwards the prefix table is
    // still live — and `Keymap::route` lets an UNBOUND key inside that window fall through to the
    // pane, which is why `"stayed"` used to arrive whole. R315 bound `prefix s` to `choose-tree`,
    // so its first character stopped falling through and the rest went into a chooser's query.
    //
    // The product is right and this check inherited a window it never declared — R308's registered
    // hazard, bitten by a round three later. Waiting past it is the recorded fix, and it is done
    // through the helper that ENFORCES the wait rather than by a bare sleep here.
    typing_follows(&mut tui, &mut conn, &session, left);
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
    let held = pane_words(&mut conn, &session, 0);
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
        painted(&tui, "before")
    });

    // `-h` puts the panes side by SIDE, so pane 0 is the LEFT one and focus lands on the right.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
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
    // The swap arrows are `-r`, so the window the press above opened is still running and the
    // prefix below would be a SELF-SEND with the arrow following it into the pane. Measured: with
    // all four bytes in one read this fails every time, and it only ever passed because a client
    // slower than `repeat-time` had let the window lapse between them.
    end_the_repeat_window(&mut tui);
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
        painted(&tui, "mine")
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

    let (near, far) = halves(BOOT_PANES.0);
    wait_for(
        "the client to re-tile around a split it did not make",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
            )
        },
    );
    tui.wait_for("a divider to appear without a key being typed", || {
        let column = tui.pane_column(near);
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
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("the split to settle", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });
    // ...and then for THIS CLIENT to have re-tiled around it. The daemon publishes the new pane
    // before the split's own reply reaches the client, so "the daemon has two panes" does not yet
    // mean "the client has moved onto the new one" — and typing at that moment would land in the
    // pane the user just split, which is what makes this wait part of the test rather than
    // decoration. The divider is the visible proof, as `a_split_made_by_another_client_re_tiles`
    // uses it.
    //
    // ⚠⚠ **AND IT IS NOT ENOUGH — R334 registered this as measured and UNEXPLAINED, and R335
    // reproduced it (1 run in 8 at 4x oversubscription) and explained it.** The failure reads
    // `elsewhere..followed` in PANE 0: the word typed BEFORE the select, then the probe dots, then
    // the word after. So `elsewhere` was routed to pane 0, and the race is not about focus at all —
    // it is between THIS TEST'S OWN TWO ACTS. `type_bytes` writes to the pty master and flushes;
    // nothing observes the client CONSUMING those bytes. The select then goes out on a different
    // connection. Under load the client has not read `elsewhere` yet when the select lands, so it
    // routes the whole word to the pane it has just been moved to.
    //
    // That also explains the reading R334 could not attribute: `typing_follows` proves where a key
    // went at an EARLIER moment, and proves nothing about bytes still sitting in the pty. Hence the
    // wait below, which is the condition the claim is actually about — R327's rule, that an
    // observation window ends on the thing being claimed rather than on a proxy for it.
    tui.wait_for("the client to re-tile around its own split", || {
        let column = tui.pane_column(near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {near} reads {column:?}"))
        }
    });
    let split_pane = pane_ids(&mut conn, &session)
        .into_iter()
        .find(|id| *id != 0)
        .expect("the split made a second pane");
    tui.type_bytes(b"elsewhere");
    // ⚠ THE ORDERING THIS TEST'S CLAIM NEEDS. The negative assertion at the end is *what was typed
    // before the select stayed where it was typed*, which is only a claim about the product once
    // those keys have DEMONSTRABLY been delivered. Waiting for them here is not decoration and not a
    // longer timeout: it removes the concurrency instead of tolerating it, so a failure below is the
    // client failing to follow rather than this harness racing itself.
    wait_for(
        "the keys typed before the select to land in the pane they were typed into",
        || {
            let held = pane_words(&mut conn, &session, split_pane);
            if held.contains("elsewhere") {
                Ok(())
            } else {
                Err(format!("pane {split_pane} holds {held:?}"))
            }
        },
    );

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
    // ⚠ READ WHITESPACE-FOLDED, and that is not tidiness: this is a NEGATIVE assertion, so a word
    // that WRAPPED reads as absent and the check passes about a pane that is showing it. See
    // [`pane_words`] — the wrap was measured on the other pane of this very split.
    let held = pane_words(&mut conn, &session, 0);
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
    wait_for("the client to be painting", || painted(&tui, "before"));

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
        painted(&tui, "beforeafter")
    });
    assert_eq!(
        pane_size(&mut conn, "prod"),
        Some(BOOT_PANES),
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
    wait_for("the client to be painting", || painted(&tui, "live"));

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
        Some(BOOT_PANES),
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
    let (near, _far) = halves(BOOT_PANES.0);
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
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });

    // Drag the divider ten columns left. 1-based on the wire, so the divider sitting at 0-based
    // column `near` is `near + 1` to the protocol.
    let moved = near - 10;
    tui.type_bytes(format!("\x1b[<0;{};3M", near + 1).as_bytes());
    tui.type_bytes(format!("\x1b[<32;{};3M", moved + 1).as_bytes());
    tui.type_bytes(format!("\x1b[<0;{};3m", moved + 1).as_bytes());

    // Both children, at the sizes the moved boundary implies — asked of the daemon.
    let (want_near, want_far) = (moved, BOOT_PANES.0 - moved - 1);
    wait_for("the drag to reach both children's PTYs", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(want_near, BOOT_PANES.1), (want_far, BOOT_PANES.1)],
        )
    });

    // ...and the line the user is pointing at is drawn where they dragged it.
    tui.wait_for("the divider to be painted in its new column", || {
        let column = tui.pane_column(moved);
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

    /// Replace the config a live client is reading — the edit a person makes in their editor while
    /// their client is running, which `ClientConfig::refresh` notices on the next keystroke.
    fn rewrite(&self, text: &str) {
        std::fs::write(self.0.join("sprag").join("config.toml"), text).expect("rewrite config");
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
    wait_for("the client to be painting", || painted(&tui, "live"));

    // The OLD prefix is now just a key: both bytes reach `cat`, which echoes them back.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"d");
    wait_for(
        "the old prefix to reach the pane as an ordinary key",
        || painted(&tui, "live^Bd"),
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
    wait_for("the client to be painting", || painted(&tui, "live"));

    // `prefix k` with `k` unbound: swallowed by the client, and NOT delivered to the child.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"k");
    // A second, ordinary keystroke that DOES reach the pane, so the absence above is read from a
    // screen that has since been repainted rather than from one that simply had not caught up.
    tui.type_bytes(b"x");
    wait_for("the following ordinary key to reach the pane", || {
        painted(&tui, "livex")
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
    wait_for("the client to be painting", || painted(&tui, "live"));

    // `C-a` is nobody's prefix yet: both bytes reach the child, which echoes them.
    tui.type_bytes(&[0x01]);
    tui.type_bytes(b"d");
    wait_for("C-a to reach the pane as an ordinary key", || {
        painted(&tui, "live^Ad")
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
        painted(&tui, "live^Ad^Bd")
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
    // What the daemon hears is the TERMINAL less the client's status row, so every expectation
    // below folds over `panes_of` rather than over the pty sizes.
    wait_for("the first client's area to become the window", || {
        settled(
            pane_size(&mut conn, &session),
            &Some(panes_of(FIRST_CLIENT)),
        )
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
            panes_of(FIRST_CLIENT).0.min(panes_of(SECOND_CLIENT).0),
            panes_of(FIRST_CLIENT).1.min(panes_of(SECOND_CLIENT).1),
        ),
    );
}

#[test]
fn window_size_largest_takes_the_widest_and_the_tallest() {
    window_size_policy_case(
        "largest",
        (
            panes_of(FIRST_CLIENT).0.max(panes_of(SECOND_CLIENT).0),
            panes_of(FIRST_CLIENT).1.max(panes_of(SECOND_CLIENT).1),
        ),
    );
}

#[test]
fn window_size_latest_takes_the_client_that_reported_last() {
    window_size_policy_case("latest", panes_of(SECOND_CLIENT));
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

    // The TERMINAL sizes the two clients are given, and what the panes get out of them: the
    // client keeps its bottom row for the status line, so the arbitrated window is one row shorter
    // than either terminal.
    let (wide_pty, narrow_pty) = ((100u16, 24u16), (80u16, 24u16));
    let (wide, narrow) = (panes_of(wide_pty), panes_of(narrow_pty));
    let mut first = Tui::attach(&sock, &session);
    wait_for("the first client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    first.resize(wide_pty.0, wide_pty.1);
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
    first.wait_for("the divider to be drawn at the wide column", || {
        let column = first.pane_column(wide_near);
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
    second.resize(narrow_pty.0, narrow_pty.1);

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
    first.wait_for(
        "the un-reporting client to redraw its divider at the narrowed column",
        || {
            let column = first.pane_column(narrow_near);
            if column.chars().all(|glyph| glyph == '\u{2502}') {
                Ok(())
            } else {
                Err(format!(
                    "column {narrow_near} reads {column:?} (wide column {wide_near} reads {:?})",
                    first.pane_column(wide_near)
                ))
            }
        },
    );
}

// ----- the `window-size manual` gate -----

/// The size an operator PINS below — deliberately not the boot pane's ([`BOOT_PANE`]), not the
/// attaching client's pseudoterminal ([`BOOT_PANES`]) and not the area it resizes to
/// ([`MANUAL_CLIENT`]). Every number a daemon could fall back to is a different number, so a pass
/// cannot be an accident.
const PINNED: (u16, u16) = (111, 33);

/// The TERMINAL the client in the `manual` gate is given. Chosen SMALLER than [`PINNED`] in both
/// dimensions, so the pinned window is one the client cannot show whole — the case a policy that
/// quietly took the client's area would have to produce, and the one a user hits (a big pinned
/// session viewed from a small terminal).
///
/// What the daemon RECORDS for it is [`panes_of`] this, not this: the client keeps its bottom row.
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
        let reported = panes_of(MANUAL_CLIENT);
        let want = format!("[{}x{}]", reported.0, reported.1);
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
        settled(
            pane_size(&mut conn, &session),
            &Some(panes_of(MANUAL_CLIENT)),
        )
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
        // The AREAS the clients reported, which are their terminals less the status row each
        // keeps — `list-clients` prints what the daemon was told, not what the pty is.
        let ok = [panes_of(FOLD_FIRST), panes_of(FOLD_SECOND)]
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
    // Folded over what the clients REPORT, which is each terminal less its status row.
    let (first, second) = (panes_of(FOLD_FIRST), panes_of(FOLD_SECOND));
    resize_window_fold_case("-A", (first.0.max(second.0), first.1.max(second.1)));
}

#[test]
fn resize_window_takes_the_smallest_client_per_dimension() {
    let (first, second) = (panes_of(FOLD_FIRST), panes_of(FOLD_SECOND));
    resize_window_fold_case("-a", (first.0.min(second.0), first.1.min(second.1)));
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
    let terminal = (90, 28);
    let base = panes_of(terminal);
    client.resize(terminal.0, terminal.1);
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

/// **R331's VERB, PRESSED** — `resize-window -a` from a real key, on a client whose own area is the
/// thing being folded.
///
/// Measured before this round: `sprag bind-key R resize-window -a` answered *"resize-window" is a
/// verb a keystroke could mean and sprag does not bind it yet*. It was the last-but-one entry in
/// [`vocabulary`]'s keyboard gap, and what it was waiting on was not a grammar — it was a
/// `HostClient` call and a daemon that says which policy it arbitrated under.
///
/// # Why `-a` and not `-x`/`-y`
///
/// `-a` is the spelling that cannot be faked. An exact rectangle is a number the config file
/// carried, so a client that sent the ask and a client that pinned locally would be
/// indistinguishable; the SMALLEST fold is resolved from the areas the DAEMON has been told about,
/// and the only one here is this client's — which the client reports as its terminal LESS its status
/// row ([`panes_of`]). So the pin landing on that number says the request crossed, was resolved
/// there, and came back.
///
/// The client is deliberately resized to something neither the daemon's boot pane nor the pty's own
/// size, so the number cannot be right by accident.
///
/// # The un-pin is the second half, and it is what makes the first one a PIN
///
/// A window whose size merely follows its only client looks exactly like a pinned one while nothing
/// changes. Pressing the second key hands the size back, and then the daemon's `manual` deferral
/// puts the panes back on the same rectangle — so the observable is the STORED size, read through a
/// second `resize-window -R` whose refusal-or-answer depends on it. Instead of that indirection this
/// re-pins to an exact rectangle first, moves the panes off the client's area, and then folds: the
/// panes coming BACK to the client's own area is a fact only a fold could produce.
#[test]
fn the_resize_key_pins_the_window_to_the_area_this_client_reported() {
    let config = ConfigHome::new(
        "[options]\nwindow-size = \"manual\"\n\n[[bind]]\nkey = \"R\"\naction = \"resize-window -a\"\n\n\
         [[bind]]\nkey = \"W\"\naction = \"resize-window -R 10\"\n",
    );
    // BOTH processes, and the daemon is the half that matters: `window-size` is read by the DAEMON,
    // so a fixture that gave this file to the client alone would be asserting against whatever the
    // developer's own config says — which on the machine this was written on happens to be the same
    // value. See `attached_client_under`.
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_under(&config, &["cat"]);

    tui.type_bytes(b"before");
    wait_for("the client to be painting", || painted(&tui, "before"));

    // OFF the answer first, and by an EXACT pin so the move below cannot be the client's own report
    // arriving late. A fixture that left the window already on the client's area would pass whether
    // the key did anything at all — R330's vacuous-gate finding, on a different verb.
    let elsewhere = (123, 37);
    sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "-x",
            &elsewhere.0.to_string(),
            "-y",
            &elsewhere.1.to_string(),
        ],
    );
    wait_for("the window to be somewhere no client reported", || {
        settled(pane_size(&mut conn, &session), &Some(elsewhere))
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"R");
    wait_for("the key to fold this client's own reported area", || {
        settled(pane_size(&mut conn, &session), &Some(BOOT_PANES))
    });
    // ...and it STAYS, which is what tells a PIN from the arbitration merely catching up: under
    // `manual` an un-pinned window defers to the default policy and would land on the same number,
    // so the discriminator is the re-pin below rather than this rectangle alone.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PANES),
        "the folded pin did not hold",
    );
    // THE DISCRIMINATOR, and the ADJUST spelling driven from a KEY in the same press: a relative
    // resize moves what the window IS, so it lands 10 columns off the folded number — a fact no
    // deferral could produce. Bound rather than run through the CLI because this is the one
    // spelling whose FLAG is chosen from the sign of a number, and the binding is where that
    // happens; without this press no test moved a window from a key by anything but a fold.
    end_the_repeat_window(&mut tui);
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"W");
    wait_for("the relative key to move the fold key's own pin", || {
        settled(
            pane_size(&mut conn, &session),
            &Some((BOOT_PANES.0 + 10, BOOT_PANES.1)),
        )
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "and the client is still alive, having resized the window it is painting",
    );
}

/// **A key that STORED a size the daemon is not laying anything out over SAYS SO** (R331) — the
/// third outcome, and the whole reason this verb could be bound at all.
///
/// # Why a sentence and not a repaint
///
/// Every other key in this vocabulary either changes the screen or is refused. `resize-window` under
/// a policy that is not `manual` does neither: the daemon accepts the request, stores the rectangle,
/// and goes on laying the panes out over what the clients report. Nothing moves and nothing was
/// refused — so without a row the key is indistinguishable from one that is not bound, which is this
/// project's own definition of a defect (R316's measurement, one verb over).
///
/// # What makes it a claim about the DAEMON
///
/// The sentence names the policy the daemon is arbitrating under, and it is the DAEMON's answer that
/// carries it (`wire::WindowPin`). The client's own `XDG_CONFIG_HOME` here is the same file, so this
/// gate cannot tell the two authorities apart — the CLI gate
/// `the_policy_note_comes_from_the_daemon_and_not_from_the_callers_own_config` is where that claim
/// lives, with a config home per process. What THIS gate says is that the sentence reaches a person
/// at a display client, which no CLI test can show.
///
/// The CONTROL is the second half and it runs after: with `manual` in force the same key pins for
/// real, the panes move, and the row goes back to naming where the client is. A build that warned
/// on every pin would fail it.
#[test]
fn a_pin_the_policy_ignores_says_so_on_the_status_row() {
    let config = ConfigHome::new(
        "[options]\nwindow-size = \"largest\"\n\n[[bind]]\nkey = \"R\"\naction = \"resize-window -x 90 -y 25\"\n\n\
         [[bind]]\nkey = \"U\"\naction = \"resize-window -u\"\n",
    );
    // The DAEMON reads `window-size`, so it gets this file too — see `attached_client_under`.
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_under(&config, &["cat"]);

    // THE CONTROL FIRST: the row says where the client is, so a row that had been showing the
    // sentence all along cannot pass.
    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    // Under `largest` with one client, the window IS that client's area — which is what makes the
    // pin below inert and this the state the sentence is about.
    wait_for("the window to be this client's own area", || {
        settled(pane_size(&mut conn, &session), &Some(BOOT_PANES))
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"R");
    wait_for(
        "the row to say the size was stored and is not in force",
        || announced(&tui, "window-size is largest"),
    );
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PANES),
        "the pin was inert, which is what the sentence is about",
    );
    // ONE row carrying BOTH, over the trail rather than off the row now: the sentence is a note on
    // `display-time`, so a read taken after the wait above may be looking at a row it has already
    // left. Requiring one frame to hold both is also the stronger claim — two rows each holding
    // half would be a client that said the policy and the rectangle at different moments.
    assert!(
        tui.status_rows()
            .iter()
            .any(|row| row.contains("window-size is largest") && row.contains("90x25")),
        "the sentence carries the RECTANGLE, which a key press shows nowhere else: {:?}",
        tui.status_rows(),
    );

    // THE UN-PIN, from the other side of the same failure: it removes something that was doing
    // nothing, so nothing on screen moves and only a sentence says the key did anything at all.
    end_the_repeat_window(&mut tui);
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"U");
    wait_for("the row to say the un-pin changed nothing visible", || {
        announced(&tui, "un-pinned")
    });
    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PANES),
        "and nothing moved, which is why it needed saying",
    );

    // THE CONTROL, on the other side of the same key: with `manual` in force the pin is performed,
    // the panes move, and there is nothing to say. `set-option` edits the one file both processes
    // read, so nothing is restarted.
    let flipped = sprag_on(&sock, &config, &["set-option", "window-size", "manual"]);
    assert!(
        flipped.status.success(),
        "set-option failed: {}",
        String::from_utf8_lossy(&flipped.stderr),
    );
    wait_for("the row to come back before the second press", || {
        says(&tui, &where_it_is)
    });
    end_the_repeat_window(&mut tui);
    // The trail is append-only, so an index into it now is a mark on the history: everything from
    // here on is what THIS press painted, and the two sentences the first half of the test put on
    // the row are behind it.
    let before_the_real_pin = tui.status_rows().len();
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"R");
    wait_for("the same key to move the panes for real", || {
        settled(pane_size(&mut conn, &session), &Some((90, 25)))
    });
    assert!(
        !tui.status_rows()[before_the_real_pin..]
            .iter()
            .any(|row| row.contains("window-size")),
        "a pin the daemon USES said something anyway, in some frame: {:?}",
        &tui.status_rows()[before_the_real_pin..],
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

    // ⚠ **THE NAMED ARM, driven while the named window is NOT the current one** (R331's debt
    // question). Every other resize in this tree acts on the scope, so `ResizeWindowAsk`'s window
    // key was written by the CLI and read by the daemon with no test in which the two windows
    // DISAGREE — a request that dropped the name would have landed here, silently, on `born`.
    //
    // It re-pins window "0" to the same rectangle it already holds, which is deliberate: what is
    // asserted is where the pin did NOT go. `born` is current and un-pinned, so it follows the
    // client below; a resize that had acted on the scope would have pinned it instead and that wait
    // would never settle.
    let renamed = sprag_on(
        &sock,
        &config,
        &[
            "resize-window",
            "-t",
            &session,
            "0",
            "-x",
            &pinned.0.to_string(),
            "-y",
            &pinned.1.to_string(),
        ],
    );
    assert!(
        renamed.status.success(),
        "a resize NAMING a window that is not the current one: {}",
        String::from_utf8_lossy(&renamed.stderr),
    );

    let terminal = (60, 20);
    // What the client REPORTS out of that terminal — one row less, kept for its status line.
    let client = panes_of(terminal);
    let mut tui = Tui::attach(&sock, &session);
    wait_for("the client", || match attached(&mut conn, &session) {
        0 => Err("nobody".to_owned()),
        _ => Ok(()),
    });
    tui.resize(terminal.0, terminal.1);

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
        painted(&tui, "hello")
    });

    // The terminal GROWS past the pinned window. The client re-reports, the daemon ignores it, the
    // tiling is the same tiling over the same window — and the client clears before repainting.
    tui.resize(100, 30);

    wait_for("the pane to survive a clear that moved nothing", || {
        painted(&tui, "hello")
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
    wait_for("the client to be painting", || painted(&tui, "live"));

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

/// **A bound `switch-client -t` ASKS which session, and the answer moves this client** — R314's
/// asking arm, end to end on a real pseudoterminal.
///
/// The arm ships BINDABLE AND UNBOUND (tmux's `prefix s` is a chooser and this is a name prompt, so
/// its key is not taken), which is exactly why this test binds it from a config file: an arm nobody
/// can reach is an arm nobody drives, and R314 wrote this after finding the whole chain —
/// `Ask::of` → `Subject::SwitchTo` → `commit` → `switch_session_named` — driven by NOTHING.
///
/// THE FIXTURE MAKES THE TWO OUTCOMES DISAGREE. A wrong answer (`ghost`, which no session carries)
/// must leave the client exactly where it is, and a right one (`elsewhere`) must move it — so a
/// prompt that committed whatever was typed, and one that committed nothing at all, both fail.
///
/// The pane's own text is the second record: `cat` echoes what it is given, so a name that reached
/// the shell would be visible there. That is the assertion the rename prompt's test makes for its
/// own key, made again here because a NEW ask arm gets the guarantee only if it routes the same way.
#[test]
fn a_bound_switch_client_asks_which_session_and_the_answer_moves_the_client() {
    let config = ConfigHome::new("[[bind]]\nkey = \"s\"\naction = \"switch-client -t\"\n");
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    let _ = &sock;
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "elsewhere" } }),
    )
    .expect("new_session answers");

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));
    let shell_before = pane_text_of(&mut conn, &session, 0);
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "the client starts on the session it booted into",
    );

    // A WRONG answer first, so a prompt that moved the client on any answer fails here rather than
    // passing the happy path below. The PROMPT ITSELF is waited for, because the assertion after it
    // ("the client did not move") is satisfied by nothing happening at all — the vacuous shape this
    // project has been caught by four times.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"s");
    wait_for("the bound key to open the switch prompt", || {
        shows(&mut tui, "(switch-client)")
    });
    tui.type_bytes(b"ghost\r");
    wait_for(
        "the daemon's refusal to be painted under the prompt",
        || shows(&mut tui, "no session is called \"ghost\""),
    );
    wait_for(
        "the refused answer to leave the client where it was",
        || settled(attached(&mut conn, &session), &1),
    );
    assert_eq!(
        attached(&mut conn, "elsewhere"),
        0,
        "and it certainly did not land on the other session",
    );

    // ⚠ THE PROMPT IS STILL OPEN, holding what was typed — this module's stated rule ("a user who
    // has to retype a name they just typed has been told off rather than helped"), and the reason
    // the next answer starts with `C-u`. Written the other way round first, the second attempt
    // pressed the prefix INTO the open editor and the screen read `ghostselsewhere`: the product
    // behaved exactly as documented and the TEST was wrong. Asserted here rather than worked
    // around silently.
    wait_for("the refused text to still be in the editor", || {
        shows(&mut tui, "ghost")
    });

    // ...and the RIGHT answer moves it. `C-u` clears; the seed is EMPTY for this subject, so what
    // lands is exactly what is typed after it.
    tui.type_bytes(b"\x15elsewhere\r");
    wait_for("the answered prompt to move this client", || {
        settled(attached(&mut conn, "elsewhere"), &1)
    });
    assert_eq!(
        attached(&mut conn, &session),
        0,
        "...and off the one it was on: a switch LEAVES as well as arrives",
    );

    // NOT the shell. Every character of both answers was the client's.
    assert_eq!(
        pane_text_of(&mut conn, &session, 0),
        shell_before,
        "the session names were typed AT THE CLIENT: not one character reached the pane",
    );
}

/// **R314 THROUGH THE SHIPPED TUI, on a real pseudoterminal: a session is reachable from the
/// keyboard, at the front that had NO way to reach one at all.**
///
/// Before this round a `sprag-tui` user had to detach and run `sprag attach` in a shell; the three
/// session chords lived in `sprag-gui` alone, in a table this vocabulary could not see. So the
/// claim is not "another key works" — it is that this binary now performs a verb it has never had.
///
/// **THE FIXTURE IS BUILT SO THE ANSWERS DISAGREE.** Three sessions and the client starts on the
/// middle one, so `prefix )` and `prefix (` name DIFFERENT sessions; the daemon's viewer badge is
/// what is read, not the screen, because that is the fact the switch changes and no amount of
/// looking at glyphs establishes it. `prefix L` then goes BACK, which discriminates once more: it
/// must name the session visited before, not simply the neighbour.
///
/// REVERT-PROOF: resolve the step in the client (the `session_neighbour` walk R314 deleted) and the
/// badge assertions still pass — which is why the LAST one exists: after two steps and a `-l` the
/// client is on the session it visited, and a walk that ignored the visit history lands elsewhere.
/// Drop the `SwitchClient` arm from this binary's perform and every wait times out.
#[test]
fn the_prefix_reaches_another_session_and_then_the_one_before_it() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    for name in ["alpha", "beta"] {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": name } }),
        )
        .expect("new_session answers");
    }
    let _ = &sock;
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "the client starts on the BOOT session, which is first in the daemon's order",
    );

    // `prefix )` — one step forward. The daemon walks; this client sent only a direction.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b")");
    wait_for("the client to step onto alpha", || {
        settled(attached(&mut conn, "alpha"), &1)
    });
    assert_eq!(
        attached(&mut conn, &session),
        0,
        "and it really LEFT: the badge on the session it came from fell",
    );

    // `prefix )` again — the ring keeps going, from where it now is.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b")");
    wait_for("the client to step onto beta", || {
        settled(attached(&mut conn, "beta"), &1)
    });

    // `prefix (` — one step BACK, which on this fixture is alpha and NOT the boot session. This is
    // the assertion a direction-blind walk fails.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"(");
    wait_for("the client to step back onto alpha", || {
        settled(attached(&mut conn, "alpha"), &1)
    });

    // `prefix L` — the session VISITED before this one, which is beta. A neighbour walk would
    // answer the boot session here, so this is what tells the history from the ring.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"L");
    wait_for("the client to go back to the session it visited", || {
        settled(attached(&mut conn, "beta"), &1)
    });
    assert_eq!(
        attached(&mut conn, &session),
        0,
        "the boot session is still empty — the -l went to the VISIT, not to the neighbour",
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
/// The lapse is a sleep PAST `repeat-time`, which is one-directional: the window closes at a
/// deadline the client computed when it acted, so overshooting it cannot fail in the other
/// direction. That reasoning is sound and it was written about the third claim only.
///
/// ⚠ THE SECOND CLAIM USED TO BE A RACE, and the paragraph above is why nobody looked: this test
/// typed the second `%` only after WAITING for the pane to echo the first, and that wait is a round
/// trip through three processes. It had to finish inside a 100 ms window. Measured at
/// `4289edf`, before the round that found it: **2 failures in 5 isolated runs**, on a tree where
/// nothing about repeat had changed. The diagnostic was `live^B%` — the exact screen the
/// revert-proof below predicts for a BROKEN product, so the flake was indistinguishable from the
/// defect this gate exists to catch.
///
/// The fix is structural rather than a longer timeout. The two `%` are typed BACK TO BACK, so what
/// has to happen inside the window is the client reading two bytes it already holds rather than an
/// echo travelling to `cat` and back; nothing observable is waited for while the clock runs. The
/// window is then set an order of magnitude above any plausible scheduling delay, so the remaining
/// margin is not a number anybody has to tune. Both halves are needed: raising the window alone
/// would leave the round trip inside it, and typing back to back alone would leave a 100 ms budget
/// for a client the whole suite is competing with.
///
/// REVERT-PROOF: answer `PrefixMode::ToPane` for a repeating act (i.e. ignore `-r`) and the second
/// claim times out — the screen reads `live^B%`, the `%` having gone straight to `cat`.
#[test]
fn a_repeat_binding_acts_twice_on_one_prefix_and_then_lets_go() {
    let config = ConfigHome::new(
        "[options]\nrepeat-time = 1000\n\
         [[bind]]\nkey = \"%\"\naction = \"send-prefix\"\nrepeat = true\n",
    );
    let (_daemon, _sock, _conn, _session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));

    // The prefix ONCE, then the bound key TWICE with nothing observed in between: the repeat window
    // is running from the moment the client acts on the first `%`, so anything waited for here is
    // waited for on the clock. Both bytes are already in the client's pipe when it acts.
    tui.type_bytes(&[0x02]); // the prefix, ONCE
    tui.type_bytes(b"%");
    tui.type_bytes(b"%"); // no second prefix. This is `-r`.
    wait_for("both acts to reach the pane on one prefix", || {
        painted(&tui, "live^B^B")
    });

    // Past the deadline the client itself computed — so the window is shut, not merely likely to be.
    std::thread::sleep(Duration::from_millis(1500));
    tui.type_bytes(b"%");
    wait_for("the lapsed window to hand the key back to the pane", || {
        painted(&tui, "live^B^B%")
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
    let (left, right) = halves(BOOT_PANES.0);
    wait_for("the split to reach two real PTYs", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left, BOOT_PANES.1), (right, BOOT_PANES.1)],
        )
    });
    // ASSERTED, not assumed: the split leaves the session on the pane it opened, the RIGHT one — so
    // `C-Left` below moves the boundary on that pane's near side and GROWS it.
    assert_eq!(active_pane(&mut conn, &session), Some(1));

    // ONE prefix, then the resize key. tmux's own default, and its own byte sequence: xterm sends
    // Ctrl+Left as CSI 1;5D.
    // ONE PREFIX, THREE PRESSES, NOTHING OBSERVED IN BETWEEN. The repeat window opens when the
    // client acts on the first press and lasts `repeat-time` (500 ms by default), so anything
    // waited for between the presses is waited for ON THAT CLOCK — and a `pane_sizes` round trip
    // to the daemon is exactly what does not fit under load. Measured at 2x CPU oversubscription:
    // this gate timed out at 45 s with the boundary at -1, the two repeat presses having gone to
    // the shell as ordinary keys. Its sibling `a_repeat_binding_acts_twice_...` had the same defect
    // and was fixed one round earlier; this one was missed because it never failed unloaded.
    //
    // -3 rather than -1 is also the STRONGER discriminator, not a weaker one: a build that ignored
    // `-r` lands on -1, so the two outcomes are told apart by the same single assertion that used
    // to need two.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;5D");
    tui.type_bytes(b"\x1b[1;5D");
    tui.type_bytes(b"\x1b[1;5D");
    wait_for("three presses on one prefix to move the boundary", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left - 3, BOOT_PANES.1), (right + 3, BOOT_PANES.1)],
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
    end_the_repeat_window(&mut tui);
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;5C"); // Ctrl+Right
    wait_for("the boundary to come back one cell", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left - 2, BOOT_PANES.1), (right + 2, BOOT_PANES.1)],
        )
    });

    // And the FIVE-cell family, on tmux's own second key.
    end_the_repeat_window(&mut tui);
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"\x1b[1;3C"); // Alt+Right
    wait_for("the coarse key to move five cells at once", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left + 3, BOOT_PANES.1), (right - 3, BOOT_PANES.1)],
        )
    });

    // THE BINDING IS UNDER THE PREFIX, not in the root table: the same bytes with NO prefix move
    // nothing. Without this the test would pass over a client that resized on every arrow key and
    // took the chord away from the program in the pane for good — which is the whole reason these
    // eight defaults are not root bindings (`C-Left` is word-motion in readline).
    end_the_repeat_window(&mut tui);
    tui.type_bytes(b"\x1b[1;5D");
    tui.type_bytes(b"\x1b[1;5D");
    // Something the pane WILL answer, sent after them, so this waits on a real event rather than on
    // a duration: if the arrows had been swallowed the sizes would have moved before this arrives.
    tui.type_bytes(b"done");
    tui.wait_for("the unprefixed keys to reach the pane", || {
        if tui.rows().iter().any(|row| row.contains("done")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });
    assert_eq!(
        pane_sizes(&mut conn, &session),
        vec![(left + 3, BOOT_PANES.1), (right - 3, BOOT_PANES.1)],
        "an arrow with no prefix is the PANE's, so the arrangement did not move",
    );
}

/// **A repeat window is judged on when a key ARRIVED, not on when the client got round to it.**
///
/// The gate above types its three presses back to back for a stated reason — the window is running
/// while the client works — and that reasoning was only half of the answer. It made the TEST stop
/// observing anything on the clock; it did nothing about the CLIENT, which reads the window's clock
/// once per keystroke at the moment it routes that keystroke, after the previous one's round trip to
/// the daemon and repaint have already been paid for. Three presses that a terminal delivered in ONE
/// read are then judged at three different times, and a slow enough machine closes the window
/// between them: measured at 2x CPU oversubscription, the gate above failed 3 runs in 6, landing on
/// -1 (the first press acted, the repeats went to the shell as raw arrow keys) or on 0.
///
/// **`repeat-time` here is 1 ms, and that is what makes this deterministic rather than a load
/// experiment.** One `scene/invoke` round trip is orders of magnitude more than a millisecond on any
/// machine, so a client that reads its clock after acting has ALWAYS closed the window by the second
/// press, on an idle machine as surely as on a loaded one. What the window measures is the thing
/// under test, so shrinking it below the client's own cost is the discriminator, not a trick: a
/// build that stamps a keystroke when the terminal handed it over passes at 1 ms, and a build that
/// stamps it when it gets round to it fails at any value under the cost of one act.
///
/// The presses are ONE `write`, which is what "arrived together" means at this seam: a pty write of
/// 19 bytes is atomic, so they are in the client's terminal before its first read and cannot be
/// split across two of them. That is exactly what an auto-repeating keyboard delivers to a busy
/// client, and it is the case a person hits by holding a key down.
///
/// -3 is the whole claim in one number: -1 is the build that judges on handling time, 0 is a build
/// that never reached the daemon at all.
#[test]
fn a_repeat_window_is_judged_on_when_a_key_arrived_not_on_when_it_was_handled() {
    let config = ConfigHome::new("[options]\nrepeat-time = 1\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(&[0x02]);
    tui.type_bytes(b"%");
    let (left, right) = halves(BOOT_PANES.0);
    wait_for("the split to reach two real PTYs", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(left, BOOT_PANES.1), (right, BOOT_PANES.1)],
        )
    });

    // ONE write: the prefix and three presses are in the terminal together, so nothing about when
    // the client reads them can put them in different reads.
    tui.type_bytes(b"\x02\x1b[1;5D\x1b[1;5D\x1b[1;5D");
    wait_for(
        "three presses delivered in one read to act as one gesture",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(left - 3, BOOT_PANES.1), (right + 3, BOOT_PANES.1)],
            )
        },
    );
}

/// **Everything the person can READ was painted inside ONE atomic frame.**
///
/// A repaint is a difference — rows an arrangement moved, a divider drawn between two panes, a
/// status row rewritten — and a terminal presenting each of those writes as it arrives shows the
/// reader the states BETWEEN two arrangements. DEC private mode 2026 is the protocol that says
/// "everything between these two is one update"; sprag's own emulator has honoured it for a pane's
/// CHILD since long before this client emitted it, and this is the claim that it now pays the same
/// courtesy outward, to the terminal it is itself a child of.
///
/// # The assertion, and why it is made this way round
///
/// A test that looked for the sequences would pass over a client that emitted a bracket once and
/// painted everything else outside it — which is exactly the failure a sixth paint site introduces.
/// So the transcript is SPLIT and the LOOSE half is replayed on its own: whatever this client wrote
/// outside a frame, put on a terminal by itself, must leave a screen with nothing on it. Setting a
/// mode, asking for focus reports and forwarding a notification are all legitimately outside a
/// frame — none of them is something a person reads off the grid — and the split says so precisely,
/// where a count of escape sequences could not.
///
/// The FRAMED half is replayed too, and that is the half that keeps the first from passing
/// vacuously: a client that wrote nothing at all would satisfy "nothing outside a frame". Replaying
/// only the frames has to reproduce the panes, the divider between them and the status row — the
/// whole picture, from the atomic updates alone.
#[test]
fn everything_the_person_can_read_was_painted_inside_one_atomic_frame() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));

    // A SECOND frame, and one that redraws most of the screen: the split re-tiles both panes, draws
    // a divider and rewrites the status row, which is the repaint a person would actually see tear.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });
    tui.wait_for("a divider to stand between the two panes", || {
        let column = tui.pane_column(near);
        if column.chars().all(|glyph| glyph == '\u{2502}') {
            Ok(())
        } else {
            Err(format!("column {near} reads {column:?}: {:?}", tui.rows()))
        }
    });

    let (framed, loose, frames) = tui.framed_and_loose();
    assert!(
        frames >= 2,
        "a client that painted a boot frame and a re-tile closed at least two atomic updates, \
         not {frames}",
    );

    let replayed = |bytes: &[u8]| {
        let mut emulator = Emulator::new(BOOT_PTY.0, BOOT_PTY.1);
        emulator.advance(bytes);
        let screen = VtPort::screen(&emulator);
        (0..screen.rows())
            .map(|row| screen.row_text(row).trim_end().to_owned())
            .collect::<Vec<_>>()
    };

    let outside = replayed(&loose);
    assert!(
        outside.iter().all(String::is_empty),
        "everything this client wrote OUTSIDE an atomic frame, replayed on a terminal of its own, \
         leaves nothing a person could read: {outside:?}",
    );

    // ...and the frames alone carry the whole picture, which is what stops the claim above from
    // being one about a client that painted nothing.
    let inside = replayed(&framed);
    assert!(
        inside[0].starts_with("live"),
        "the panes are painted from the atomic frames alone: {inside:?}",
    );
    assert!(
        inside
            .iter()
            .any(|row| row.chars().nth(usize::from(near)) == Some('\u{2502}')),
        "so is the divider between them: {inside:?}",
    );
    assert!(
        inside
            .iter()
            .any(|row| row.contains(&format!("[{session}]"))),
        "and so is the row this client speaks in: {inside:?}",
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
    wait_for("the client to be painting", || painted(&tui, "live"));

    tui.type_bytes(&[0x01]); // C-a, the prefix this user declared
    tui.type_bytes(b"?");
    tui.wait_for("the key table to open on the user's own prefix", || {
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

    // IT SCROLLS. The LAST ROW of the table is past the bottom of a 24-row terminal, so reaching it
    // is a statement that the page key moved the view rather than that the rows happened to fit.
    //
    // ⚠ It watched for the vocabulary HEADING until R323, and three new forms pushed that heading
    // off the TOP of the last page — a test that failed because the table grew, which is the shape
    // of a check anchored to a position rather than to an end. The last FORM is derived from the
    // vocabulary itself, so it moves with it.
    let last_form = *sprag_host::keymap::BoundAction::vocabulary()
        .last()
        .expect("the vocabulary is not empty");
    assert!(
        !tui.rows().iter().any(|row| row.contains(last_form)),
        "the last row must be off screen before the scroll, or paging proves nothing: {:?}",
        tui.rows(),
    );
    tui.type_bytes(b"\x1b[6~"); // PageDown
    tui.type_bytes(b"\x1b[6~");
    tui.type_bytes(b"\x1b[6~");
    tui.wait_for("the page key to reach the end of the table", || {
        if tui.rows().iter().any(|row| row.contains(last_form)) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    // NOT ONE CHARACTER REACHES THE SHELL while the table is up.
    tui.type_bytes(b"whoami");

    tui.type_bytes(b"q");
    // The panes come back, and the row underneath is the one the client left there.
    wait_for("the panes to come back", || painted(&tui, "live"));

    // The pane is still the keyboard's, and it never saw the six characters above: `cat` echoes
    // what reaches it, so a leak would read `livewhoamiX` here.
    tui.type_bytes(b"X");
    wait_for("the pane to have the keyboard again", || {
        painted(&tui, "liveX")
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
    tui.wait_for("the pane to be printing", || {
        if tui.rows().iter().any(|row| row.contains("tick")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    tui.type_bytes(&[0x02]); // prefix
    tui.type_bytes(b"?");
    tui.wait_for("the key table to open", || {
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

/// **A POINTER DOES NOT REACH WHAT THE KEY TABLE IS COVERING** — and the same drag moves the
/// boundary the moment the table is closed.
///
/// Found by the debt audit rather than by writing the feature: `sprag-tui`'s mouse arm had no idea
/// an overlay existed, so while a full-screen table was up a press on the cell where a divider
/// happens to be started a DRAG and resized the arrangement — a change with no gesture the user
/// could see and nothing on screen to explain it. The keystroke path was closed and this was not,
/// which is the shape R306 met one round earlier when a PASTE reached the shell behind its prompt.
///
/// **The control is the whole test.** The first half asserts nothing moved, which any broken drag
/// would also satisfy — a wrong column, a child that never asked for tracking, an escape sequence
/// this client cannot decode. The second half sends the SAME BYTES with the table closed and
/// requires the boundary to move, so the first half is a statement about the overlay rather than
/// about the drag.
///
/// The prompt is deliberately NOT covered by this rule and that asymmetry is the rule itself: it
/// borrows one row, so everything a pointer can reach is visible and is what the user meant.
#[test]
fn a_pointer_does_not_reach_a_divider_under_the_key_table() {
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
    let (near, far) = halves(BOOT_PANES.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"?");
    tui.wait_for("the key table to cover the arrangement", || {
        if tui.rows().iter().any(|row| row.contains("zoom-pane")) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });

    // The exact gesture `a_divider_drag_moves_the_boundary_and_both_children` uses, on the exact
    // column that test drags, while the table is up.
    let moved = near - 10;
    let drag = |tui: &mut Tui| {
        tui.type_bytes(format!("\x1b[<0;{};3M", near + 1).as_bytes());
        tui.type_bytes(format!("\x1b[<32;{};3M", moved + 1).as_bytes());
        tui.type_bytes(format!("\x1b[<0;{};3m", moved + 1).as_bytes());
    };
    drag(&mut tui);
    // Something the CLIENT will answer, sent after the drag, so this waits on a real event rather
    // than on a duration: the table scrolls on a key it does own, which cannot happen before the
    // three reports above have been read off the same stream.
    tui.type_bytes(b"\x1b[6~");
    tui.type_bytes(b"\x1b[6~");
    tui.type_bytes(b"\x1b[6~");
    // The table's LAST ROW, derived — not its vocabulary HEADING, which three new forms pushed off
    // the top of the last page at R323. A barrier anchored to a position stops being a barrier the
    // moment the thing it is inside grows.
    let last_form = *sprag_host::keymap::BoundAction::vocabulary()
        .last()
        .expect("the vocabulary is not empty");
    tui.wait_for("the client to have consumed everything sent since", || {
        if tui.rows().iter().any(|row| row.contains(last_form)) {
            Ok(())
        } else {
            Err(format!("{:?} (client: {})", tui.rows(), tui.liveness()))
        }
    });
    assert_eq!(
        pane_sizes(&mut conn, &session),
        vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        "a press under the table must not claim the divider it cannot be seen to be on",
    );

    // THE CONTROL: close it, send the same bytes, and the boundary moves.
    tui.type_bytes(b"q");
    tui.wait_for("the panes to come back", || {
        if tui.rows().iter().any(|row| row.contains("zoom-pane")) {
            Err(format!("the table is still up: {:?}", tui.rows()))
        } else {
            Ok(())
        }
    });
    drag(&mut tui);
    let (want_near, want_far) = (moved, BOOT_PANES.0 - moved - 1);
    wait_for(
        "the same drag to move the boundary with nothing over it",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(want_near, BOOT_PANES.1), (want_far, BOOT_PANES.1)],
            )
        },
    );
}

/// **R315 THROUGH THE SHIPPED `sprag-tui`, on a real pseudoterminal: `prefix s` puts every session,
/// window and pane on the screen, and the row a person picks is where they end up.**
///
/// The gesture this front has never had. `switch-client -t` (R314) asks a user to TYPE a name, which
/// is no help to the user who cannot name their other session — and `sprag ls` in a shell was the
/// only answer sprag had to *"what is there?"*, which is R308's finding one noun over.
///
/// **THE FIXTURE IS BUILT SO THE READINGS DISAGREE.** Three sessions, and the one picked is neither
/// the one the client is on nor the neighbour a `switch-client -n` would step to — so a chooser that
/// ignored the cursor and a chooser that stepped a ring both fail here. The DAEMON's viewer badge is
/// what is read, not the screen: that is the fact a switch changes, and no amount of looking at
/// glyphs establishes it.
///
/// The pane's own text is the second record: `cat` echoes what it is given, so a character that
/// reached the shell instead of the chooser would be visible there.
///
/// REVERT-PROOF: drop the `ChooseTree` arm from `Ask::of` and the first wait times out with the
/// panes still painted; commit the row's LABEL instead of its target and the last assertion still
/// passes — which is why the sibling wire test, not this one, is what pins the identity.
#[test]
fn the_prefix_opens_a_chooser_and_the_row_that_is_picked_is_where_the_client_lands() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    for name in ["alpha", "beta"] {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": name } }),
        )
        .expect("new_session answers");
    }
    let _ = &sock;
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));
    let shell_before = pane_text_of(&mut conn, &session, 0);
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "the client starts on the session it booted into",
    );

    // `prefix s` — tmux's own key for this.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"s");
    wait_for("the chooser to open", || shows(&mut tui, "(choose-tree)"));
    // ...and it lists what is THERE, which is the whole difference from a name prompt.
    for name in [&session, "alpha", "beta"] {
        wait_for(&format!("the chooser to list {name}"), || {
            shows(&mut tui, name)
        });
    }
    wait_for(
        "the chooser to say how big a session is, so two rows can be told apart",
        || shows(&mut tui, "1 window, 1 pane"),
    );

    // TYPE TO NARROW, then pick. `beta` is the LAST session in the daemon's order and the client is
    // on the FIRST, so this is neither where it is nor one step along it.
    tui.type_bytes(b"beta");
    tui.type_bytes(b"\r");
    wait_for("the picked row to move this client", || {
        settled(attached(&mut conn, "beta"), &1)
    });
    assert_eq!(
        attached(&mut conn, &session),
        0,
        "...and off the one it was on: a switch LEAVES as well as arrives",
    );
    assert_eq!(
        attached(&mut conn, "alpha"),
        0,
        "and it did not land on the neighbour a session STEP would have taken",
    );

    // NOT the shell. Every character of the query was the client's.
    assert_eq!(
        pane_text_of(&mut conn, &session, 0),
        shell_before,
        "the query was typed AT THE CLIENT: not one character reached the pane",
    );

    // ...and the chooser is GONE from the screen, so the client is projecting its new session
    // rather than still holding a list over it. What happens to the keyboard afterwards is the
    // CANCEL test's claim, and it is driven there on a `cat` pane that can be read back — this
    // session's pane runs the machine's `$SHELL`, whose echo is not a fixture.
    tui.wait_for("the chooser to leave the screen", || {
        if tui.rows().iter().any(|row| row.contains("(choose-tree)")) {
            Err(format!("{:?}", tui.rows()))
        } else {
            Ok(())
        }
    });
}

/// **A chooser that is CANCELLED changes nothing, and gives the keyboard straight back.**
///
/// The half the happy path cannot claim: a surface that owns every key has to be leavable, and a
/// person who opens a list to look at it must not be moved by having done so. Escape is the gesture;
/// what it must NOT do is what is asserted.
///
/// REVERT-PROOF: make `Typed::Cancel` commit instead and the badge assertion fails; make it leave
/// the overlay up and the typed marker never reaches the pane.
#[test]
fn a_cancelled_chooser_moves_nobody_and_hands_the_keyboard_back() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "elsewhere" } }),
    )
    .expect("new_session answers");
    let _ = &sock;
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"s");
    wait_for("the chooser to open", || shows(&mut tui, "(choose-tree)"));
    // Move the cursor OFF the row the client is on, so a cancel that committed would be observable.
    tui.type_bytes(&[0x1b, b'[', b'B']); // ArrowDown
    tui.type_bytes(&[0x1b, b'[', b'B']);
    tui.type_bytes(&[0x1b, b'[', b'B']);
    tui.type_bytes(b"\x03"); // C-c, which cancels without depending on an escape timeout

    tui.type_bytes(b"cancelled");
    wait_for("the keyboard to be the pane's again", || {
        let held = pane_text_of(&mut conn, &session, 0);
        if held.contains("cancelled") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "and the client never moved: a list a person looked at and closed is not a gesture",
    );
    assert_eq!(attached(&mut conn, "elsewhere"), 0);
}

/// **AN OPEN CHOOSER IS LIVE: a session another client makes while a person is reading the list
/// appears in it, and the cursor does not move.**
///
/// ⚠ **THIS TEST EXISTS BECAUSE THE DEBT SWEEP FOUND `Pick::refresh` HAD ONE CALLER.** It was the
/// GUI's per-frame reconcile, so THIS front's list was a photograph — and `Pick`'s own module doc
/// said *"refreshed from the daemon, so the list is LIVE while a person reads it"*, which was false
/// for half the product. A durable doc claim with no driver on one of its two subjects.
///
/// THE FIXTURE MAKES THE TWO READINGS DISAGREE: the new session is made by a SEPARATE connection
/// while the overlay is up, so a chooser that had photographed its rows cannot show it however long
/// the test waits.
///
/// The second assertion is the other half and is what makes the refresh safe rather than merely
/// live: the cursor is an IDENTITY, so a list growing under it leaves it where the person left it.
///
/// REVERT-PROOF: drop the `Pick::refresh` call from the wake path and the first wait times out with
/// the old rows still painted.
#[test]
fn a_session_made_while_the_chooser_is_open_appears_in_it() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &sock;
    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"s");
    wait_for("the chooser to open", || shows(&mut tui, "(choose-tree)"));
    wait_for("...listing the session this client is on", || {
        shows(&mut tui, &session)
    });
    assert!(
        !tui.rows().iter().any(|row| row.contains("latecomer")),
        "the session under test does not exist yet: {:?}",
        tui.rows(),
    );

    // ANOTHER connection makes a session while the list is on the screen. This is the whole claim:
    // nothing this client did put it there.
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "latecomer" } }),
    )
    .expect("new_session answers");

    wait_for("the new session to appear in the open list", || {
        shows(&mut tui, "latecomer")
    });
    // ...AND THE CURSOR DID NOT MOVE. It opened on this client's own session, and a list that grew
    // under it must leave it there — which is what an identity cursor buys and a row number does
    // not. Read by CANCELLING and checking nobody moved: a cursor that had drifted onto the new row
    // would still be harmless here, so the sharper assertion is the one the unit test makes; what
    // this adds is that the refresh did not disturb the live surface.
    tui.type_bytes(b"\x03");
    tui.type_bytes(b"after");
    wait_for("the keyboard to come back to the pane", || {
        let held = pane_text_of(&mut conn, &session, 0);
        if held.contains("after") {
            Ok(())
        } else {
            Err(format!("pane 0 holds {held:?}"))
        }
    });
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "and a list that changed under a reader moved nobody",
    );
    assert_eq!(attached(&mut conn, "latecomer"), 0);
}

// ----- what a key DID, on the row this client speaks in (R316) -----

/// The two sequences a client wraps ONE frame in — DEC private mode 2026, set and reset.
///
/// The bytes are spelled here rather than built from termwiz's escape vocabulary because this is
/// the READING side: a test that asked the same library the client writes with to say what the
/// client should have written would agree with it about a wrong answer. What a terminal is owed is
/// a literal byte string, so that is what is compared against.
const FRAME_OPEN: &[u8] = b"\x1b[?2026h";
/// The reset half of [`FRAME_OPEN`] — where a frame ENDS, and so where the status trail records.
const FRAME_CLOSE: &[u8] = b"\x1b[?2026l";

/// Where the frames a client presented END inside a stream that arrives in arbitrary pieces.
///
/// The reader thread is handed whatever the OS felt like returning: four frames in one batch, or
/// half of a frame terminator with the other half in the next read. Both cases are the SAME
/// mistake if the scanner has no memory — the first records one row for four frames, the second
/// misses a frame entirely — and both are what made the per-`read` trail a sampler. Carrying the
/// partial match across batches is the whole of what makes the granularity the client's frame
/// rather than the OS's read size.
///
/// A type rather than two locals in the thread's closure, so the claim can be made where it lives:
/// [`a_frame_boundary_survives_a_read_that_splits_it`] drives it with the batches an OS would have
/// to be provoked into producing.
struct FrameSplitter {
    /// How much of [`FRAME_CLOSE`] stood at the end of the last batch.
    matched: usize,
}

impl FrameSplitter {
    /// A splitter that has seen nothing.
    const fn new() -> Self {
        Self { matched: 0 }
    }

    /// The index ONE PAST each frame close inside `batch` — so `batch[..end]` is everything up to
    /// and including a terminator, and the slice after the last one is a frame still open.
    fn closes_in(&mut self, batch: &[u8]) -> Vec<usize> {
        let mut closes = Vec::new();
        for (at, byte) in batch.iter().enumerate() {
            // FRAME_CLOSE begins with the only ESC in it, so a failed match can restart at this
            // byte and never needs to look further back than one — no partial-overlap table.
            self.matched = if *byte == FRAME_CLOSE[self.matched] {
                self.matched + 1
            } else {
                usize::from(*byte == FRAME_CLOSE[0])
            };
            if self.matched == FRAME_CLOSE.len() {
                self.matched = 0;
                closes.push(at + 1);
            }
        }
        closes
    }
}

/// **The status trail's granularity is the CLIENT's frame, not the OS's read.**
///
/// The two batchings an OS can impose are the two ways a per-`read` trail loses a frame, and this
/// drives both directly rather than trying to provoke a scheduler into producing them: several
/// frames arriving at once, and one frame's terminator torn in half across two reads. A scanner
/// with no memory reports 1 close for the first and 0 for the second, which is exactly the trail
/// that came back holding neither the starting row nor the message at 2x oversubscription.
#[test]
fn a_frame_boundary_survives_a_read_that_splits_it() {
    let frame = |body: &str| [FRAME_OPEN, body.as_bytes(), FRAME_CLOSE].concat();

    // FOUR frames in ONE batch: four closes, not one.
    let mut splitter = FrameSplitter::new();
    let batch = [frame("a"), frame("b"), frame("c"), frame("d")].concat();
    assert_eq!(
        splitter.closes_in(&batch).len(),
        4,
        "a read carrying four frames ends four of them",
    );

    // ...and each close is where the terminator ACTUALLY ends, so the bytes before it are that
    // frame's and no other's.
    let mut splitter = FrameSplitter::new();
    let one = frame("live");
    assert_eq!(splitter.closes_in(&one), vec![one.len()]);

    // A TERMINATOR TORN IN HALF. Neither batch holds the whole sequence, so a scanner that starts
    // afresh on each finds nothing at all; the close belongs to the batch the last byte arrived in.
    let mut splitter = FrameSplitter::new();
    let (head, tail) = one.split_at(one.len() - 3);
    assert_eq!(splitter.closes_in(head), Vec::<usize>::new());
    assert_eq!(splitter.closes_in(tail), vec![3]);

    // A FALSE START is not a close, and does not eat the real one behind it: an ESC that opens
    // something else leaves the scanner able to match a terminator beginning at the very next byte.
    let mut splitter = FrameSplitter::new();
    let noise = [b"\x1b[?2026", FRAME_CLOSE].concat();
    assert_eq!(
        splitter.closes_in(&noise),
        vec![noise.len()],
        "the prefix that diverged is not a frame, and the terminator after it still is",
    );
}

/// The status row's index on a [`BOOT_PTY`]-sized terminal — the last one, which is what
/// `sprag_tui::Split` reserves.
///
/// Derived rather than typed, so a terminal size change here cannot leave a test asserting about a
/// pane's last line while calling it the status row.
const STATUS_ROW: u16 = BOOT_PTY.1 - 1;

/// **THE GATE for R316: a key bound to a session that does not exist SAYS SO.**
///
/// The defect this round opened on was MEASURED on this exact fixture before a line was written: a
/// live `sprag-tui`, a binding of `switch-client -t ghost`, and a screen that came back
/// **byte-for-byte identical** to the one an UNBOUND key left. A user could not tell a typo in
/// their config from a broken build, and no surface in the product could tell them.
///
/// Three claims, and the middle one is what makes the first discriminating:
///
/// * **The refusal is painted, and it names the session that is not there.** `no session called
///   "ghost"` — the noun read off [`BoundAction::names`] rather than off the action's grouping,
///   which is `client` and would have made this line say `no client called "ghost"`.
/// * **The CONTROL says nothing.** An unbound key in the same table, pressed on the same client,
///   leaves the status row reading where the client is. Without this, a row that had simply
///   painted the refusal at boot would pass.
/// * **Nothing moved.** The client is still attached to the session it started on, so the sentence
///   is about a refusal rather than about a switch that happened to be reported.
///
/// It presses the CONTROL FIRST, which is R303's rule applied to a surface rather than to a lock:
/// the reading that must be able to fail is taken before the one that must succeed, so a row that
/// was already showing the sentence cannot be mistaken for one that just started to.
#[test]
fn a_key_bound_to_a_session_that_does_not_exist_says_so() {
    let config = ConfigHome::new("[[bind]]\nkey = \"g\"\naction = \"switch-client -t ghost\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    tui.type_bytes(b"live");
    wait_for("the client to be painting", || painted(&tui, "live"));
    let where_it_is = format!("[{session}]");
    wait_for("the status row to say where the client is", || {
        mentions(&tui, &where_it_is)
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });

    // THE CONTROL, pressed first: `q` is bound to nothing, so the row must go on saying where the
    // client is. A row that reported every keystroke would fail here.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"q");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        tui.row(STATUS_ROW).contains(&where_it_is),
        "an UNBOUND key says nothing, so the row still reads where the client is: {:?}",
        tui.row(STATUS_ROW),
    );

    // ...and the bound one names what is not there.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"g");
    wait_for("the refusal to be painted on the status row", || {
        announced(&tui, "no session called \"ghost\"")
    });

    assert_eq!(
        attached(&mut conn, &session),
        1,
        "the refused switch left the client exactly where it was",
    );
    // The name never reached the shell — `cat` echoes what it is given, so a keystroke that leaked
    // past the keymap would be visible in the pane's own text.
    let text = pane_words(&mut conn, &session, 0);
    assert!(
        !text.contains("ghost"),
        "the bound key is the CLIENT's and must not also reach the child: {text:?}",
    );
}

/// **The message EXPIRES on its own deadline**, rather than at the next keystroke — the one timer
/// in this client's event loop.
///
/// The claim needs a client that is doing NOTHING afterwards, because that is the state a
/// message-clearing that rode on the next event would survive: press the key, touch nothing, and
/// watch the row come back. `display-time` is set to a value long enough to be observed and short
/// enough not to slow the suite, from the user's own config — which is also the only thing that
/// drives the option end to end.
#[test]
fn the_message_goes_away_on_its_own_and_the_row_comes_back() {
    let config = ConfigHome::new(
        "[options]\ndisplay-time = 400\n\n[[bind]]\nkey = \"g\"\naction = \"switch-client -t ghost\"\n",
    );
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    let _ = &mut conn;

    let where_it_is = format!("[{session}]");
    wait_for("the status row to say where the client is", || {
        mentions(&tui, &where_it_is)
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"g");
    wait_for("the refusal to be painted", || {
        announced(&tui, "no session called \"ghost\"")
    });

    // NOTHING IS TYPED from here on. A client that cleared the row on its next event would hold the
    // sentence forever in this state, which is the whole point of waiting rather than pressing.
    wait_for(
        "the row to come back with no keystroke to prompt it",
        || {
            mentions(&tui, &where_it_is)
                .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
        },
    );
}

/// **The status row names the session AND its windows, and follows both.**
///
/// The half of R316 that is not a refusal: a terminal client had no chrome at all, so *which
/// session am I on* and *which of its windows* were questions this front could not answer — the
/// GUI's session rail and tab strip have always answered them. The row is DERIVED from the daemon
/// on every paint, so this drives it by changing the daemon's answer rather than by pressing the
/// key that draws it.
///
/// Two moves, each of which a hand-maintained row would get wrong in a different way: a window is
/// CREATED (the list grows and the marker moves), and the session is RENAMED from another
/// connection (the name follows, which a client remembering its own would not).
#[test]
fn the_status_row_follows_the_session_and_its_windows() {
    let (_daemon, _sock, mut conn, session, tui) = attached_client();

    wait_for(
        "the row to name the boot session and its one window",
        || says(&tui, &format!("[{session}] 0:0*")),
    );

    // A SECOND window, made from a separate connection — so what moves the row is the daemon's
    // answer and not this client's own keystroke.
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_WINDOW_ACTION), "args": { "session": session } }),
    )
    .expect("new_window answers");
    wait_for("the row to grow a window and move the marker", || {
        says(&tui, &format!("[{session}] 0:0 1:1*"))
    });

    // ...and the NAME follows a rename this client did not make. A row holding the name it attached
    // with would still read the old one — R303's impostor, one surface up.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(RENAME_SESSION_ACTION),
            "args": { "session": session, "name": "renamed" },
        }),
    )
    .expect("rename_session answers");
    wait_for("the row to follow the rename", || {
        says(&tui, "[renamed] 0:0 1:1*")
    });
}

/// **The status row is the client's OWN, and the panes do not reach it.**
///
/// The geometry claim R316 makes forty times over in this file's other expectations, asserted once
/// directly: the daemon's window is the terminal LESS the status row, so a pane's last line lands
/// on the row above it and the row below carries the client's own text. A client that painted its
/// row over a pane would pass every size assertion here and lose the pane's bottom line.
#[test]
fn the_panes_stop_one_row_above_what_the_client_says() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    assert_eq!(
        pane_size(&mut conn, &session),
        Some(BOOT_PANES),
        "the daemon's window is what the client REPORTED, which is the terminal less its row",
    );
    assert_eq!(
        BOOT_PANES.1 + 1,
        BOOT_PTY.1,
        "...and that is exactly one row, or the assertion below is about the wrong line",
    );

    // Fill the pane past its own last line, so the row under it is one the child WOULD have written
    // to if it had been given the whole terminal.
    for line in 0..BOOT_PTY.1 {
        tui.type_bytes(format!("line{line}\r").as_bytes());
    }
    tui.wait_for("the child's output to reach the pane's last line", || {
        settled(tui.row(BOOT_PANES.1 - 1).trim_end().is_empty(), &false)
            .map_err(|got| format!("{got}: rows {:?}", tui.rows()))
    });

    let status = tui.row(STATUS_ROW);
    assert!(
        status.starts_with(&format!("[{session}]")),
        "the last row is the CLIENT's, not the pane's overflow: {status:?} (screen {:?})",
        tui.rows(),
    );
}

/// **A key naming a WINDOW that is not there says so too** — the same defect one level down, and
/// the arm the round's own audit found driven by nothing.
///
/// `select_window` answered `()` until R316, so a client could not tell a name the daemon refused
/// from one it selected. The daemon has always refused an unknown name; what was missing was the
/// fact crossing back. This drives the whole chain — config, keymap, wire client, report, row — on
/// a name nothing carries, and the CONTROL is the same key bound to a window that does exist.
#[test]
fn a_key_bound_to_a_window_that_does_not_exist_says_so() {
    let config = ConfigHome::new(
        "[[bind]]\nkey = \"w\"\naction = \"select-window -t nowindow\"\n\n\
         [[bind]]\nkey = \"e\"\naction = \"select-window -t 0\"\n",
    );
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to name the boot window", || {
        says(&tui, &where_it_is)
    });

    // THE CONTROL FIRST: the window that EXISTS is selected and says nothing, so a row that
    // reported every keystroke fails here rather than passing the case below.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"e");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        tui.row(STATUS_ROW),
        where_it_is,
        "selecting the window this client is already on says nothing",
    );

    // ...and the name nothing carries is NAMED — by the DAEMON since R325. This used to read the
    // client's own `no window called "nowindow"`, built from the binding's own words because a
    // payload-free refusal carried nothing; the registry says `no window NAMED`, and one of the two
    // is a guess about the other end's vocabulary.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"w");
    wait_for("the refusal to name the window that is not there", || {
        announced(&tui, "nowindow")
    });
    // Over the TRAIL, like the wait above: a refusal is on `display-time`, so a row read after the
    // wait returned may be one the client has already left. The name and the WORDING are two claims
    // and both are made here — a client that named the window but paraphrased the registry would
    // satisfy the wait and fail this.
    assert!(
        tui.status_rows()
            .iter()
            .any(|row| row.contains("no window named \"nowindow\"")),
        "the sentence is the registry's own, not this client's paraphrase: {:?}",
        tui.status_rows(),
    );

    let text = pane_words(&mut conn, &session, 0);
    assert!(
        !text.contains("nowindow"),
        "the bound key is the CLIENT's and must not also reach the child: {text:?}",
    );
}

/// **`display-time 0` puts the silence back, which is what its own doc says it is for.**
///
/// The one value that undoes this round's whole surface, reachable only by asking for it — and the
/// arm nothing drove until the audit asked. It is the option's DECISION (see
/// `sprag_host::options::DISPLAY_TIME`), so a build that quietly floored it at some minimum would
/// be honouring a config nobody wrote.
///
/// The CONTROL is the same key on the same client with the default in force, one test above: there
/// the row carries the sentence, here it never does.
#[test]
fn display_time_zero_reports_nothing_at_all() {
    let config = ConfigHome::new(
        "[options]\ndisplay-time = 0\n\n[[bind]]\nkey = \"g\"\naction = \"switch-client -t ghost\"\n",
    );
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"g");
    // Sampled over a window far longer than the default `display-time`, so a message that appeared
    // for even one frame would be caught — the assertion is an ABSENCE and needs a duration behind
    // it rather than one look.
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        assert_eq!(
            tui.row(STATUS_ROW),
            where_it_is,
            "`display-time 0` is a message that has already expired, so the row never changes",
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // ...and the KEY still ran: the option silences the report, not the verb. Nothing moved because
    // no session is called `ghost`, which is what makes this the same fixture as the test above.
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "the client is still on the session it started from",
    );
}

/// **The other FOUR "nowhere to go" arms are driven here** — the swap's edge, the resize's
/// boundary, a window already at the end of its session's order, and (R323) a break with nothing to
/// break out of.
///
/// `select-pane -L`'s edge has its own fixture (the arrow walk), and these had NONE: they are
/// the "a branch reachable only from a state no test builds" shape, found by the round's own audit
/// asking which reports had a driver. One fixture reaches all four because a session with ONE
/// window holding ONE pane is exactly the state each of them refuses in.
///
/// The BREAK's is the likeliest of the four to be met by an actual person: `prefix !` pressed with
/// one pane on screen. Measured against a live daemon — it answers *"pane 0 is its window's only
/// pane, no window holds it, or the name is taken"* — so the client either says something or looks
/// broken.
///
/// The CONTROL is the boot row itself: every press is checked against the row it must NOT leave
/// alone, and the row returns to naming the session between them.
#[test]
fn the_swap_the_resize_and_the_move_all_say_when_they_go_nowhere() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();
    let _ = &mut conn;

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    // Each press is separated by a wait for the row to come BACK, so a sentence left over from the
    // press before cannot be read as this one's — the vacuous shape this file has been caught by.
    //
    // ⚠ **THREE OF THE FOUR ARE NOT REFUSALS AND ONE IS, AND THE ROW NOW SAYS WHICH** (R325).
    // Measured on exactly THIS fixture, which is the half that had to be measured rather than
    // reasoned: the first three answer OK carrying an outcome word — nothing above, this is the
    // only window, the boundary is at an edge — so the CLIENT's own report is the whole of what
    // anyone knows. Only `break-pane` is REFUSED, and the daemon states why, so its sentence takes
    // the row instead of a generic that read the same four words for four different situations.
    //
    // ⚠ A first draft of this table expected the daemon's sentence for `resize-pane` too, from a
    // CLI probe where NOBODY WAS ATTACHED — there the verb refuses (*"nothing is watching that
    // window"*). A pty fixture has a client, so its window is measured and the same key is an edge.
    // **The state a probe builds is part of what it measures**; running it here is what said so.
    //
    // Pairing all four in ONE table is what makes this discriminating: a build that showed the
    // daemon's sentence everywhere fails the first three rows, and a build that showed the client's
    // everywhere — which is what shipped until R325 — fails the fourth.
    for (keys, want) in [
        (&b"\x1b[1;2A"[..], "swap-pane -U: nowhere to go"),
        (&b"\x1b[1;5A"[..], "resize-pane -U 1: nowhere to go"),
        (&b"<"[..], "move-window -p: nowhere to go"),
        (&b"!"[..], "cannot break the only pane in a window"),
    ] {
        // PAST THE REPEAT WINDOW: the arrows are `-r`, so inside it the prefix table is still live
        // and the next chord's first character would be a self-send (R308's hazard, R315's bite).
        // PAST THE REPEAT WINDOW: the arrows are `-r`, and inside a window the prefix below is a
        // self-send. Closed by a keystroke rather than waited out — see `end_the_repeat_window`.
        end_the_repeat_window(&mut tui);
        tui.type_bytes(PREFIX);
        tui.type_bytes(keys);
        wait_for(&format!("{want:?} to be painted"), || announced(&tui, want));
        wait_for("the row to come back before the next press", || {
            says(&tui, &where_it_is)
        });
    }
}

// ----- what somebody ELSE asked this client to say (R317) -----

/// **THE GATE for R317: a sentence chosen by ANOTHER PROCESS reaches a person's screen.**
///
/// The measurement this round opened with, inverted. At `5acde43`, against exactly this fixture,
/// every route a second process had left the screen unmoved: `report-agent blocked` was accepted and
/// changed nothing on it, `send-keys` put the words INSIDE the person's program, and a pane child's
/// OSC 9 reached the terminal front nowhere at all. Only that client's own keyboard could write the
/// row.
///
/// Four claims, in the order that makes the middle two discriminating:
///
/// * **The CONTROL first.** Before anything is sent, the row says where the client is. Without this,
///   a row that had somehow been showing the sentence all along would pass.
/// * **The sentence arrives** — chosen by a `sprag` process that shares nothing with this client but
///   a daemon, and painted on the row a keystroke writes.
/// * **The CLI is told WHO it reached**, by client id, which is what makes the answer a value rather
///   than an `ok`.
/// * **The words never reach the SHELL.** The boot pane runs `cat`, which echoes what it is given,
///   so a message that had gone down `send-keys`'s road — the one route that existed before this —
///   would be visible in the pane's own text. This is what separates a MESSAGE from typing.
///
/// REVERT-PROOF: drop the `store_message` call from the wire poll thread's wake (i.e. collect the
/// message and throw it away) and the second claim times out with the row still naming the session,
/// while `sprag display-message` goes on reporting the delivery — which is precisely the shape of
/// defect R316 was about, one seam further out.
#[test]
fn a_message_sent_by_another_process_reaches_the_person_at_this_client() {
    let (_daemon, sock, mut conn, session, tui) = attached_client();

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    let out = Command::new(sprag_cli_bin())
        .args(["display-message", "-t", &session, "the deploy finished"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .output()
        .expect("run the sprag CLI");
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "display-message failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    wait_for("the message to be painted on the status row", || {
        announced(&tui, "the deploy finished")
    });

    // WHO it reached, named. A delivery to nobody and a delivery to this client must not read alike,
    // which is the whole reason the verb answers a list.
    assert!(
        said.starts_with("shown to gui-"),
        "the answer names the client it reached, not just `ok`: {said:?}",
    );
    assert!(
        !said.contains("nobody"),
        "a client IS attached, so this is not the empty delivery: {said:?}",
    );

    // ...and it was a MESSAGE, not typing: `cat` echoes what it is given, so a word that had gone
    // into the pane would be in the pane's own text.
    let text = pane_words(&mut conn, &session, 0);
    assert!(
        !text.contains("deploy"),
        "the sentence is the CLIENT's to paint and must never reach the child: {text:?}",
    );
}

/// **A message with nobody attached says so, and a message to a client that is not there is
/// REFUSED** — the two negatives, which must not read alike.
///
/// The first is an ANSWER (the daemon did what was asked; there was no audience) and the second is a
/// caller's MISTAKE (they named a target that does not exist). Collapsing them would send an agent
/// looking for a person who is right there — R301's "one set of bytes for three causes", in the verb
/// this round adds.
///
/// The CONTROL is the third call: the same message, to the client that IS attached, on the same
/// daemon at the same instant. Without it, a build that refused everything would pass the first two.
#[test]
fn a_message_to_nobody_and_a_message_to_a_stranger_answer_differently() {
    let (_daemon, sock, mut conn, session, tui) = attached_client();
    let _ = &mut conn;
    let _ = &tui;

    let run = |args: &[&str]| -> (bool, String, String) {
        let out = Command::new(sprag_cli_bin())
            .args(args)
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .output()
            .expect("run the sprag CLI");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // A session NOBODY is attached to — the empty audience. It is created here rather than reusing
    // the boot session, because this client is attached to that one.
    let (made, _, why) = run(&["new", "quiet"]);
    assert!(made, "a second session for the empty audience: {why}");
    let (ok, out, err) = run(&["display-message", "-t", "quiet", "anybody there"]);
    assert!(ok, "an empty audience is an ANSWER, not a failure: {err}");
    assert_eq!(out.trim(), "shown to nobody: no client is attached");

    // A client that does not exist — the caller's mistake.
    let (ok, out, err) = run(&[
        "display-message",
        "-t",
        &session,
        "-c",
        "gui-nobody-0",
        "hello",
    ]);
    assert!(
        !ok,
        "a target that is not there is refused, not delivered to nobody: {out:?}"
    );
    // NAMES WHO IS THERE. It used to point at `sprag list-clients`, because a payload-free refusal
    // gave the CLI nothing but another command to suggest; since R325 the daemon answers with the
    // fact that verb would have printed.
    assert!(
        err.contains("gui-nobody-0") && err.contains("these are: gui-"),
        "the refusal names what was asked for AND who is actually attached: {err:?}",
    );

    // THE CONTROL, on the same daemon at the same instant: the real audience still works, so the two
    // refusals above are about their targets rather than about a build that cannot deliver at all.
    let (ok, out, err) = run(&["display-message", "-t", &session, "and this one lands"]);
    assert!(ok, "the control must succeed: {err}");
    assert!(
        out.starts_with("shown to gui-"),
        "the control reached the attached client: {out:?}",
    );
}

/// **An ALERT stays until a person touches a key**, where a note goes away on `display-time`.
///
/// The property this round claims over every rival surface: a timer is a bet that somebody is
/// looking, and the case where missing a message is the failure is exactly the case where they are
/// not. herdr's most urgent toast is an eight-second one (`sync_toast_deadline`, read at
/// `9a4ce5e1`); tmux has no such state at all.
///
/// The CONTROL is the NOTE, sent first through the same verb on the same client: it expires with
/// nothing pressed, which is what proves the alert's persistence is the severity's doing rather than
/// a client that stopped clearing messages.
#[test]
fn an_alert_waits_for_a_keystroke_where_a_note_waits_for_the_clock() {
    let config = ConfigHome::new("[options]\ndisplay-time = 300\n");
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    let _ = &mut conn;

    let where_it_is = format!("[{session}] 0:0*");
    let send = |severity: &str, text: &str| {
        let out = Command::new(sprag_cli_bin())
            .args(["display-message", "-t", &session, "-s", severity, text])
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .env("XDG_CONFIG_HOME", config.as_str())
            .output()
            .expect("run the sprag CLI");
        assert!(
            out.status.success(),
            "display-message -s {severity} failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    };
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    // THE CONTROL: a NOTE expires on its own, with nothing pressed.
    send("note", "a passing note");
    // `announced`, not a poll: `display-time` is 300 ms here, so the note is on the row for
    // three POLLs and a descheduled test process sees none of them. The row EXPIRING is the next
    // claim and stays a poll, because a client at rest is a state that does not pass.
    wait_for("the note to be painted", || {
        announced(&tui, "a passing note")
    });
    wait_for("the note to expire with no keystroke", || {
        says(&tui, &where_it_is)
    });

    // ...and an ALERT does not. Sampled over a window many times `display-time`, so a client that
    // treated every message alike would be caught rather than merely raced.
    send("alert", "the deploy needs you");
    wait_for("the alert to be painted", || {
        mentions(&tui, "the deploy needs you")
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });
    let deadline = Instant::now() + Duration::from_millis(1800);
    while Instant::now() < deadline {
        assert!(
            tui.row(STATUS_ROW).contains("the deploy needs you"),
            "an alert has no deadline; the row reads {:?} after {:?} of a {}ms display-time",
            tui.row(STATUS_ROW),
            deadline - Instant::now(),
            300,
        );
        std::thread::sleep(Duration::from_millis(40));
    }

    // THE MARK: an alert says so in front of its sentence. The audit's finding was that
    // `Message::severity` had NO reader while its own doc claimed a surface marked something.
    assert!(
        tui.row(STATUS_ROW).contains("alert: the deploy needs you"),
        "an alert is MARKED, so a person can see why the row is not clearing: {:?}",
        tui.row(STATUS_ROW),
    );

    // ...until a person touches a key. `Escape` is bound to nothing and reaches no pane arm, so what
    // clears the row is the ACKNOWLEDGEMENT and not something the key happened to do.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"\x1b");
    wait_for("the keystroke to acknowledge the alert", || {
        says(&tui, &where_it_is)
    });

    // A WARNING is on the clock like a note and carries NO mark — the arm between the two, which
    // nothing drove through the verb until the audit asked.
    send("warn", "the retry did not help");
    wait_for("the warning to be painted", || {
        announced(&tui, "the retry did not help")
    });
    // Over the TRAIL and not the row, and that is what keeps the claim from going vacuous when the
    // wait above stops requiring the sentence to still be there: read at a moment, this would pass
    // on a client that had marked the warning and then let it expire. Read over every frame, it
    // says the mark was never painted at all — the stronger sentence, and the one meant.
    assert!(
        !tui.status_rows().iter().any(|row| row.contains("warn:")),
        "only an ALERT is marked; a warning explains itself: {:?}",
        tui.status_rows(),
    );
    wait_for("the warning to expire on the clock, like a note", || {
        says(&tui, &where_it_is)
    });
}

/// **A message a person sent does not take the row from a live ALERT** — the precedence rule, driven
/// end to end through two `sprag` processes and a real client.
///
/// A unit test pins `Message::over`; this pins that both the daemon's slot and the client's row
/// actually consult it, which is the "a unit test on a method is not a test that the caller calls
/// it" rule this project keeps re-learning.
///
/// The CONTROL is the second half: the SAME two messages in the other order, where the alert DOES
/// take the row from the note. Without it, a client that simply ignored every message after the
/// first would pass.
#[test]
fn a_note_does_not_take_the_row_from_a_live_alert() {
    let (_daemon, sock, mut conn, session, mut tui) = attached_client();
    let _ = &mut conn;

    let where_it_is = format!("[{session}] 0:0*");
    let send = |severity: &str, text: &str| {
        let out = Command::new(sprag_cli_bin())
            .args(["display-message", "-t", &session, "-s", severity, text])
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .output()
            .expect("run the sprag CLI");
        assert!(out.status.success(), "display-message -s {severity} failed");
    };
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    send("alert", "the deploy needs you");
    wait_for("the alert to be painted", || {
        mentions(&tui, "the deploy needs you")
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });
    send("note", "a passing note");
    // The WINDOW is still a window — the note has to arrive and lose, and nothing observable says
    // when it has — but nothing is SAMPLED inside it any more. A note that took the row for one
    // frame between two of the old 40 ms reads was invisible to them and is a frame in the trail,
    // so the claim is now about every frame this client painted rather than about thirty of them.
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        !tui.status_rows()
            .iter()
            .any(|row| row.contains("a passing note")),
        "a note must not take the row from a live alert, and no frame of this client's had one: \
         {:?}",
        tui.status_rows(),
    );
    assert!(
        tui.row(STATUS_ROW).contains("the deploy needs you"),
        "and the alert is still the one being shown: {:?}",
        tui.row(STATUS_ROW),
    );

    // THE CONTROL, the other way round: acknowledge, put a NOTE up, and the alert takes it.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"\x1b");
    wait_for("the alert to be acknowledged", || says(&tui, &where_it_is));
    send("note", "a passing note");
    wait_for("the note to be painted", || {
        announced(&tui, "a passing note")
    });
    send("alert", "the deploy needs you");
    wait_for("the alert to take the row from the note", || {
        mentions(&tui, "the deploy needs you")
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });
}

/// **A `-c` message reaches a client attached to a DIFFERENT session than the request's scope.**
///
/// The address is the point: `-t` scopes the request and `-c` names the person, and the two need
/// not agree. A build in which `-c` quietly fell back to the scope would deliver to nobody here, so
/// the CONTROL is the scope itself — `elsewhere` is a session this client is not attached to.
///
/// ⚠ **WHAT A REVERT-PROOF SETTLED, AND WHAT IT DID NOT.** Deleting the `channels.bump` from
/// `display_message` leaves this GREEN. Chasing that produced two facts and one open question,
/// recorded here rather than guessed at:
///
/// * The first version of this test had an unconsumed wake in flight from its own setup, so it
///   could not have failed either way. The settle below is what fixed that, and it is the shape
///   R315 named: choose a fixture where the two readings actually disagree.
/// * Measured after settling: a cross-session mutation (`new-window`, `rename-session`, a message
///   delivered to nobody) does NOT move this client's `scene/revision`, so clients are not woken by
///   other sessions' traffic — `sprag_host::notify`'s stated contract holds.
/// * **STILL OPEN**: the message arrives promptly with the bump deleted, and this client's session
///   revision does not visibly move for the delivery either way. So the wake that carries a
///   cross-session message is NOT attributed. The bump is kept because it is the only wake this
///   code owns and it is correct; what is missing is the assertion that it is the one doing the
///   work. Do not read the green above as proof that it is.
#[test]
fn a_named_client_is_reached_from_a_request_scoped_to_another_session() {
    let (_daemon, sock, mut conn, session, tui) = attached_client();

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    let run = |args: &[&str]| -> (bool, String, String) {
        let out = Command::new(sprag_cli_bin())
            .args(args)
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .output()
            .expect("run the sprag CLI");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // A session this client is NOT attached to, for the request to be scoped to.
    let (made, _, why) = run(&["new", "elsewhere"]);
    assert!(made, "a second session to scope the request to: {why}");
    let (listed, clients, why) = run(&["list-clients", "-t", &session]);
    assert!(listed, "list-clients failed: {why}");
    let client = clients
        .split(':')
        .next()
        .expect("a client id")
        .trim()
        .to_owned();
    assert!(
        client.starts_with("gui-"),
        "the fixture needs the attached client's id: {clients:?}",
    );

    // ⚠ SETTLE FIRST — see the note above. Until this session's revision has stopped moving, a
    // change left over from the setup can deliver the message and the assertion below cannot fail.
    let revision = |conn: &mut HostConn, session: &str| -> u64 {
        conn.call("scene/revision", json!({ "session": session }))
            .ok()
            .and_then(|value| value["revision"].as_u64().or_else(|| value.as_u64()))
            .expect("the daemon answers its own scene revision")
    };
    let settled_at = wait_for_still(|| revision(&mut conn, &session));

    let (ok, out, err) = run(&[
        "display-message",
        "-t",
        "elsewhere",
        "-c",
        &client,
        "across the sessions",
    ]);
    assert!(ok, "a `-c` target outside the scope is reachable: {err}");
    assert!(
        out.contains(&client),
        "the delivery names the client it crossed to: {out:?}",
    );
    wait_for("the cross-session message to be painted", || {
        announced(&tui, "across the sessions")
    });
    let _ = settled_at;
    assert_eq!(
        attached(&mut conn, &session),
        1,
        "a message moves nobody: the client is still on the session it was watching",
    );
}

/// **A message carrying a control character never reaches the terminal** — refused at the CLI, with
/// the rule named, and refused again at the daemon so no other caller can slip past.
///
/// The words are written into somebody's terminal, so a newline forges a row of the status line and
/// an escape is obeyed by the emulator. The rival TRUNCATES and strips silently
/// (`sanitized_notification_text`, read at `9a4ce5e1`) and answers `shown`, so a caller whose message
/// was mangled never learns.
///
/// The CONTROL is the same sentence with the escape removed, which is accepted — so the refusal is
/// about the character rather than about the verb.
#[test]
fn a_message_with_an_escape_in_it_is_refused_by_name() {
    let (_daemon, sock, mut conn, session, tui) = attached_client();
    let _ = &mut conn;
    let _ = &tui;

    let run = |text: &str| -> (bool, String, String) {
        let out = Command::new(sprag_cli_bin())
            .args(["display-message", "-t", &session, text])
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .output()
            .expect("run the sprag CLI");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (ok, _, err) = run("wiped: \u{1b}[2J");
    assert!(!ok, "an escape sequence must not be paintable");
    assert!(
        err.contains("control characters") && err.contains("escape"),
        "the refusal names the rule and why it exists: {err:?}",
    );
    let (ok, _, err) = run("two\nrows");
    assert!(!ok, "a newline must not forge a second row: {err:?}");

    // THE CONTROL: the same sentence without the escape is accepted, so the refusal is the
    // character's doing.
    let (ok, out, err) = run("wiped: nothing");
    assert!(ok, "the control must be accepted: {err}");
    assert!(out.starts_with("shown to gui-"), "{out:?}");

    // THE OTHER THREE RULES, each pinned by the WHOLE SENTENCE an operator reads. They had no
    // driver until the debt sweep asked: a refusal a user meets is a sentence this project pins,
    // and a grammar checked only by a unit test is a grammar the CLI could stop applying.
    let (ok, _, err) = run("   ");
    assert!(!ok, "a blank message is refused");
    assert!(
        err.contains("a message cannot be blank"),
        "and says so in the rule's own words: {err:?}",
    );
    let too_long = "x".repeat(sprag_host::report::MessageText::MAX_BYTES + 1);
    let (ok, _, err) = run(&too_long);
    assert!(!ok, "a message longer than a row is refused");
    assert!(
        err.contains("at most 200 bytes") && err.contains("201"),
        "and names the bound AND the length offered: {err:?}",
    );
}

/// **The refusals a user meets when they get the VERB wrong** — a severity that is not one, and no
/// message at all — plus the form a person actually types: `display-message` with **no `-t`**.
///
/// Every other gate here passes `-t`, so the DEFAULT scope — the session the connection resolves to
/// — was reached by nothing. That is the shape the sweep hunts: a branch reachable only from a state
/// no test builds, and in this case it is the state a user is in.
#[test]
fn the_verb_refuses_a_bad_severity_and_works_with_no_target_at_all() {
    let (_daemon, sock, mut conn, session, tui) = attached_client();
    let _ = &mut conn;

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    let run = |args: &[&str]| -> (bool, String, String) {
        let out = Command::new(sprag_cli_bin())
            .args(args)
            .env("SPRAG_HOST_RPC_SOCK", &sock)
            .output()
            .expect("run the sprag CLI");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (ok, _, err) = run(&["display-message", "-s", "shout", "hello"]);
    assert!(!ok, "a severity this build does not know is refused");
    assert!(
        err.contains("shout") && err.contains("note|warn|alert"),
        "and names what was offered AND what exists: {err:?}",
    );
    let (ok, _, err) = run(&["display-message"]);
    assert!(!ok, "the verb needs something to show");
    assert!(
        err.contains("needs a message to show"),
        "and says what is missing: {err:?}",
    );

    // ...and with NO `-t`, which is what a person at a shell types. The scope resolves to this
    // daemon's default session, which is the one this client is attached to, so the sentence lands.
    let (ok, out, err) = run(&["display-message", "the default scope works"]);
    assert!(ok, "display-message with no target is a normal call: {err}");
    assert!(out.starts_with("shown to gui-"), "{out:?}");
    wait_for("the message sent with no -t to be painted", || {
        announced(&tui, "the default scope works")
    });
}

// ----- what a pane's OWN CHILD asked for (R318) -----

/// A boot pane whose child turns each line it is given into an `OSC 9` notification, with the tty's
/// echo OFF — so the typing itself paints nothing and the ONLY thing that can move the screen is
/// the notification. `cat` at the end keeps the pane alive between raises.
///
/// The child is the notifier rather than the test writing the OSC through some side channel,
/// because the claim is about what a PANE'S CHILD can do: a build script, a test runner, anything a
/// person actually leaves running.
const NOTIFIER: &[&str] = &[
    "sh",
    "-c",
    "stty -echo; while IFS= read -r line; do printf '\\033]9;%s\\007' \"$line\"; done; cat",
];

/// **THE GATE for R318: the words a pane's child raised reach the person watching that session.**
///
/// Measured at `3114923`, against exactly this fixture: the daemon latched the notification (the
/// `panes` slot answered `{"title":null,"body":"build finished: 3 errors","seq":1}`) and the
/// client's screen was **byte-for-byte unchanged** — all twenty-four rows identical before and
/// after — while `sprag display-message` moved the row on the same client at the same instant. The
/// whole feature ended at a latch nobody was obliged to read.
///
/// Four claims, in the order that makes them discriminating:
///
/// * **The CONTROL first.** The row says where the client is before anything is raised, so a row
///   that had somehow been showing the sentence all along would fail here rather than pass below.
/// * **The DAEMON sees it**, which separates "the emulator did not capture it" from "nothing
///   delivered it" — the two failures that look identical from the screen.
/// * **The WORDS arrive on the row, naming the pane** in the spelling that reaches it, so a person
///   reading the message knows what to pass to `select-pane -t`.
/// * **The words never reach the SHELL.** A message is painted BY the client, not typed INTO the
///   pane; the child here echoes nothing, so anything in the pane's own text would mean the
///   sentence went down `send-keys`'s road.
///
/// REVERT-PROOF: delete the `on_attention` wiring at the daemon's boot spawn (`sprag-term.rs`) and
/// the third claim times out with the row still naming the session, while the second still passes —
/// which is precisely the state this round found.
#[test]
fn a_notification_a_pane_child_raised_reaches_the_person_at_this_client() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(Tui::attach, NOTIFIER);

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    // Settle the session so the raise below is the only thing in flight — without this a wake
    // already on its way could carry the delivery and the assertion could not fail (R315's rule).
    let _ = wait_for_still(|| pane_size(&mut conn, &session));

    tui.type_bytes(b"build finished: 3 errors\r");

    wait_for("the DAEMON to latch the pane's notification", || {
        let panes = conn
            .call(
                "scene/query",
                json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("panes answers");
        let note = panes
            .as_array()
            .and_then(|rows| rows.first())
            .map(|row| row["notification"].clone())
            .unwrap_or(Value::Null);
        settled(
            note["body"].as_str() == Some("build finished: 3 errors"),
            &true,
        )
        .map_err(|got| format!("{got}: the pane row's notification is {note}"))
    });

    wait_for("the child's words to be painted on the status row", || {
        announced(&tui, "pane 0: build finished: 3 errors")
    });

    // ...and it was a MESSAGE the client painted, not text in the pane: this child echoes nothing
    // and prints only the escape, which carries no cells.
    let text = pane_words(&mut conn, &session, 0);
    assert!(
        !text.contains(&fold("build finished")),
        "a notification stamps no cells; the sentence is the CLIENT's to paint: {text:?}",
    );
}

/// **A child that says its notification is CRITICAL gets a row that waits for a person** — where an
/// ordinary one goes away on `display-time`.
///
/// This is the property no rival has. tmux models no severity at all; herdr's API caller cannot
/// express one (`handle_notification_show` hardcodes its LOWEST toast kind) and their most urgent
/// toast is an eight-second timer, so stepping away from the desk loses it either way. Here the
/// CHILD chooses, through kitty's `u=` urgency, and `u=2` means the words stay until a keystroke.
///
/// The CONTROL is the ORDINARY notification raised first through the same path on the same client:
/// it expires, so a build where every message stayed forever would fail here before reaching the
/// claim.
#[test]
fn a_critical_notification_holds_the_row_until_a_key_is_pressed() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        Tui::attach,
        &[
            "sh",
            "-c",
            "stty -echo; while IFS= read -r line; do printf '\\033]99;u=%s;%s\\007' \
             \"${line%% *}\" \"${line#* }\"; done; cat",
        ],
    );
    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    let _ = wait_for_still(|| pane_size(&mut conn, &session));

    // THE CONTROL: `u=1`, an ordinary notification. It paints and then goes away on its own.
    tui.type_bytes(b"1 the ordinary one\r");
    wait_for("the ordinary notification to paint", || {
        announced(&tui, "the ordinary one")
    });
    wait_for("...and to expire without anybody touching a key", || {
        says(&tui, &where_it_is)
    });

    // THE CLAIM: `u=2`. It paints, and it is STILL there after several display-times.
    tui.type_bytes(b"2 the build needs you\r");
    wait_for("the critical notification to paint", || {
        mentions(&tui, "the build needs you")
            .map_err(|got| format!("{got}: row reads {:?}", tui.row(STATUS_ROW)))
    });
    std::thread::sleep(Duration::from_millis(2500));
    let held = tui.row(STATUS_ROW);
    assert!(
        held.contains("the build needs you"),
        "an alert waits for a person, not for a clock: {held:?}",
    );
    assert!(
        held.contains("alert"),
        "and it is MARKED, so a person who did not see it arrive knows why the row is not \
         clearing: {held:?}",
    );

    // A keystroke acknowledges it — `prefix q` is bound to nothing, so it reaches no pane and runs
    // no action: the row clearing is the acknowledgement and nothing else.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"q");
    wait_for("the alert to clear once a person touched a key", || {
        says(&tui, &where_it_is)
    });
}

/// **`monitor-notification off` puts the silence back, and the BELL is a separate switch.**
///
/// Two options rather than one, because the two sources have nothing in common but the word
/// "attention": a build tool that raises a notification per file and a shell that rings on tab
/// completion are different nuisances, and one switch over both would make either cost the user the
/// other. This drives that claim rather than asserting it: the notification is silenced and the bell
/// still speaks, on ONE daemon reading ONE config file.
///
/// **THE ORDER IS THE WHOLE TEST, and the first version had it backwards** — found by a revert-proof
/// that came back GREEN with the switches forced ON. That version sent the notification, then the
/// bell, then asserted the row did not hold the notification's words: but an equal severity replaces
/// what is showing, so the bell had taken the row either way and the assertion could not fail.
///
/// So the BELL GOES FIRST and is watched all the way through — it paints, then it expires — which
/// establishes on this daemon, with this config, that the routing path works and that a routed
/// message reaches the row inside `display-time`. Only then is the silenced source sent, and the
/// absence is sampled past that measured lifetime rather than at an arbitrary moment.
#[test]
fn each_attention_source_has_its_own_switch() {
    let config = ConfigHome::new("[options]\nmonitor-notification = \"off\"\n");
    // The child raises a NOTIFICATION for a line starting `n`, and a BELL for anything else.
    let (_daemon, sock) = spawn_daemon_with_config(
        &[
            "sh",
            "-c",
            "stty -echo; while IFS= read -r line; do case \"$line\" in n*) \
             printf '\\033]9;%s\\007' \"$line\";; *) printf '\\007';; esac; done; cat",
        ],
        Some(config.as_str()),
    );
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);
    let mut tui = Tui::attach(&sock, &session);
    wait_for("the client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    let _ = wait_for_still(|| pane_size(&mut conn, &session));

    // THE CONTROL FIRST: the BELL is a different switch and is still on, so it reaches the row —
    // and then goes away on its own deadline. Both halves are watched, which is what turns this into
    // a measured bound for the silence below rather than a guess at one.
    tui.type_bytes(b"ring\r");
    wait_for("the bell to reach the row", || {
        announced(&tui, "pane 0: bell")
    });
    wait_for("...and to expire, leaving the row as it was", || {
        says(&tui, &where_it_is)
    });

    // THE SILENCED HALF. The daemon must still LATCH it — the switch is about delivery, not about
    // capture, and a build that stopped capturing would break the pane list's own dot.
    tui.type_bytes(b"notification while switched off\r");
    wait_for("the DAEMON to latch it even though nobody is told", || {
        let panes = conn
            .call(
                "scene/query",
                json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("panes answers");
        let note = panes
            .as_array()
            .and_then(|rows| rows.first())
            .map(|row| row["notification"]["body"].clone())
            .unwrap_or(Value::Null);
        settled(note.as_str(), &Some("notification while switched off"))
            .map_err(|got| format!("{got}: the pane row's notification is {note}"))
    });
    // Past the lifetime the bell above MEASURED on this same client: it painted and cleared inside
    // this window, so a delivery that had been made would have been painted in it too. Read over
    // the TRAIL rather than sampled at twenty moments — a row that appeared and cleared between two
    // of those reads was invisible to them, and a silenced source that flashes once is not silent.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !tui.status_rows()
            .iter()
            .any(|row| row.contains("notification while switched off")),
        "the silenced source must not paint, in ANY frame: {:?}",
        tui.status_rows(),
    );
    assert_eq!(
        tui.status_rows().last().map(String::as_str),
        Some(where_it_is.as_str()),
        "...and nothing else may either, so the client came to rest where it started: {:?}",
        tui.status_rows(),
    );
}

// ----- where a message goes when the person is NOT HERE (R319) -----

/// **THE GATE for R319: a message follows the person out of the room, and only then.**
///
/// Measured at `f3eca8f` against exactly this fixture: a pane's child raised a notification with the
/// client's terminal UNFOCUSED, the row was painted, and this terminal was asked for nothing and told
/// nothing — `input_modes().focus_tracking` false, `notification_seq()` zero. The delivery ended at a
/// row in a window nobody was looking at, which is R318's own defect one layer further out.
///
/// Five claims, in the order that makes them discriminating:
///
/// * **The client ASKS.** Without DEC private mode 1004 on this terminal there is no way to learn
///   the person left, so this is the enabling fact and it fails first if the path is torn out.
/// * **THE CONTROL, FIRST and watched all the way through**: with the person HERE the words reach
///   the row and then expire, and NOTHING is copied out. That is a measured bound rather than a
///   guess at one — a build that copied every message would fail here instead of passing below.
/// * **The person leaves, and the next message follows them** — as a notification carrying the
///   session's own name, so somebody with four sessions is told which one wants them.
/// * **The focus report never reaches the SHELL.** `termwiz 0.23.3` turns `CSI O` into the two
///   keystrokes `Alt-[` and `O` (pinned in `key_round_trip.rs`), so a client that asked for the mode
///   without decoding it would type them into whatever the person left running. This child echoes
///   nothing and prints only what it is told to, so anything in the pane's text is the defect.
/// * **They come back, and the copies stop.** The state is an edge in both directions; a decoder
///   that only ever set `Away` would pass every claim above.
///
/// REVERT-PROOF: drop the `Outward::watch_focus(true, ..)` call in `run()` and the first claim fails;
/// drop the `outward.forward(..)` beside `take_message` and the third does, with the row still
/// painting — which is precisely the state this round found.
#[test]
fn a_message_follows_the_person_out_of_the_room() {
    // ⚠ THE POLICY IS READ FROM A CONFIG THIS TEST OWNS, and that is not ceremony: without it the
    // client reads whichever `config.toml` the machine running the suite happens to have, so a
    // developer who set `notify-outward = "off"` for themselves would see this fail against correct
    // code. R318 found exactly that defect in its own option test; this is the same fixture rule one
    // round later. The value is the DEFAULT, written out, so the test states what it depends on.
    let config = ConfigHome::new("[options]\nnotify-outward = \"unfocused\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        NOTIFIER,
    );

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    // Settle the session so the raise below is the only thing in flight (R315's rule).
    let _ = wait_for_still(|| pane_size(&mut conn, &session));

    assert!(
        tui.asked_for_focus_reports(),
        "the client must ask this terminal to report focus, or it cannot know the person left",
    );

    // THE CONTROL: the person is HERE (a terminal reports a CHANGE, so nothing said means nothing
    // has changed since the mode was set). The words reach the row and expire on their own.
    tui.type_bytes(b"the first one\r");
    wait_for("the control's words to reach the row", || {
        announced(&tui, "pane 0: the first one")
    });
    wait_for("...and to expire, leaving the row as it was", || {
        says(&tui, &where_it_is)
    });
    assert_eq!(
        tui.forwarded().1,
        0,
        "a person reading the row needs no second copy of it: {:?}",
        tui.forwarded().0,
    );

    // THE PERSON LEAVES. `CSI O` is what a terminal writes when its window loses focus.
    tui.type_bytes(b"\x1b[O");
    tui.type_bytes(b"the second one\r");

    // THE FOCUS REPORT WAS DECODED, and this is where that is provable rather than where it looks
    // provable. The child reads a LINE and echoes nothing, so a report routed to it as keystrokes
    // would be swallowed into the line it is waiting on — and come back inside its own notification,
    // where the emulator would end the OSC at the escape and stamp the tail into the pane as cells.
    // So the discriminating claim is that the daemon latched the child's words EXACTLY, and that the
    // pane holds none of them.
    //
    // ⚠ The obvious assertion here — that the pane's text contains no `[` or `O` — is VACUOUS
    // against this fixture, which is why it is not the one being made: `stty -echo` means nothing
    // typed at the child stamps a cell whatever it is.
    wait_for("the DAEMON to latch exactly what the child said", || {
        let panes = conn
            .call(
                "scene/query",
                json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("panes answers");
        let note = panes
            .as_array()
            .and_then(|rows| rows.first())
            .map(|row| row["notification"]["body"].clone())
            .unwrap_or(Value::Null);
        settled(note.as_str(), &Some("the second one"))
            .map_err(|got| format!("{got}: the pane row's notification is {note}"))
    });

    wait_for("the copy to reach the person's own terminal", || {
        let (note, seq) = tui.forwarded();
        settled(
            note.as_ref().map(|note| note.body.clone()),
            &Some(format!("[{session}] pane 0: the second one")),
        )
        .map_err(|got| format!("{got}: seq is {seq}"))
    });
    // ...and the row still says it too: the copy is a copy, not a redirection.
    wait_for("the row to say it as well", || {
        announced(&tui, "pane 0: the second one")
    });

    let text = pane_words(&mut conn, &session, 0);
    assert!(
        !text.contains(&fold("the second one")),
        "a notification stamps no cells, and a report routed to the child would put its tail \
         here: {text:?}",
    );

    // THEY COME BACK, and the copies stop — the edge in the other direction.
    tui.type_bytes(b"\x1b[I");
    tui.type_bytes(b"the third one\r");
    wait_for("the third message to reach the row", || {
        announced(&tui, "pane 0: the third one")
    });
    let (note, seq) = tui.forwarded();
    assert_eq!(
        seq, 1,
        "a person who came back gets no copy: the terminal holds {note:?}",
    );
}

/// **A child's own URGENCY reaches the person's desktop** — the whole chain, across three processes:
/// kitty `u=2` in a pane, `Severity::Alert` on the daemon's routed message, `u=2` out to the terminal
/// the person is actually sitting at.
///
/// This is the property no rival has, and it is the reason the outward form is detected at all.
/// herdr's `build_osc99_notification` hardcodes `i=1:d=0` and emits no `u=` key, so a build that says
/// *a person is needed* is forwarded as an ordinary notification; their API caller cannot express
/// urgency in the first place.
///
/// The CONTROL is an ORDINARY notification through the same path on the same client: it is forwarded
/// too, at `u=1`, so a build that hardcoded either digit would fail one of the two.
///
/// The payload lands in the notification's TITLE rather than its body, and that is kitty's protocol
/// rather than a choice here: a single unencoded chunk with no `p=` key IS the title.
#[test]
fn a_childs_own_urgency_reaches_the_persons_terminal() {
    let (_daemon, sock) = spawn_daemon_running(&[
        "sh",
        "-c",
        "stty -echo; while IFS= read -r line; do printf '\\033]99;u=%s;%s\\007' \
         \"${line%% *}\" \"${line#* }\"; done; cat",
    ]);
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);
    // The client believes it is running in kitty, which is the one terminal whose own protocol
    // carries an urgency — announced the way kitty announces itself to every child.
    // The config is this test's own, for the reason the gate above states: the policy must come from
    // a file the test wrote, never from the developer's.
    let config = ConfigHome::new("[options]\nnotify-outward = \"unfocused\"\n");
    let mut tui = Tui::attach_with_env(
        &sock,
        &session,
        &[
            ("KITTY_WINDOW_ID", "1"),
            ("XDG_CONFIG_HOME", config.as_str()),
        ],
    );
    wait_for("the client to attach", || {
        match attached(&mut conn, &session) {
            0 => Err("nobody attached".to_owned()),
            _ => Ok(()),
        }
    });
    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });
    let _ = wait_for_still(|| pane_size(&mut conn, &session));
    tui.type_bytes(b"\x1b[O");

    // THE CONTROL: an ordinary notification is forwarded, and it is forwarded as ORDINARY.
    tui.type_bytes(b"1 the ordinary one\r");
    wait_for("the ordinary notification to reach the terminal", || {
        let (note, seq) = tui.forwarded();
        settled(
            note.as_ref()
                .map(|note| (note.title.clone().unwrap_or_default(), note.urgency)),
            &Some((
                format!("[{session}] pane 0: the ordinary one"),
                sprag_vt::Urgency::Normal,
            )),
        )
        .map_err(|got| format!("{got}: seq is {seq}"))
    });

    // THE CLAIM: the child says a person is needed, and the person's terminal is told exactly that.
    tui.type_bytes(b"2 the build needs you\r");
    wait_for(
        "the critical notification to reach the terminal as critical",
        || {
            let (note, seq) = tui.forwarded();
            settled(
                note.as_ref()
                    .map(|note| (note.title.clone().unwrap_or_default(), note.urgency)),
                &Some((
                    format!("[{session}] pane 0: the build needs you"),
                    sprag_vt::Urgency::Critical,
                )),
            )
            .map_err(|got| format!("{got}: seq is {seq}"))
        },
    );
}

/// **THE POLICY IS THIS CLIENT'S OWN** — two clients on ONE daemon, reading two config files, taking
/// two different decisions about the SAME message at the same instant.
///
/// This is the axis herdr cannot reach: their suppression reads
/// `foreground_client_outer_focus`, which is whichever client their server last promoted, so one
/// person's window decides for everybody attached. R317 made the mailbox per-client and this makes
/// the policy per-client, which is the same fact about the same person.
///
/// It is also the only test that proves the option is read from the USER'S FILE by the shipped
/// binary — every unit test around it hands the policy in — and the only one that drives `off`,
/// whose whole content is an absence and which is therefore only meaningful beside a client that
/// DOES forward the same message.
///
/// Neither client asks for focus reports, and that is a claim rather than a detail: the two policies
/// here do not depend on where the person is, so the mode and the read-ahead it makes necessary are
/// not paid for.
#[test]
fn the_outward_policy_is_this_clients_own_and_not_the_daemons() {
    let (_daemon, sock) = spawn_daemon_running(&["cat"]);
    let mut conn = observe(&sock);
    let session = boot_session(&mut conn);

    let loud = ConfigHome::new("[options]\nnotify-outward = \"always\"\n");
    let silent = ConfigHome::new("[options]\nnotify-outward = \"off\"\n");
    let mut always = Tui::attach_with_env(&sock, &session, &[("XDG_CONFIG_HOME", loud.as_str())]);
    let mut off = Tui::attach_with_env(&sock, &session, &[("XDG_CONFIG_HOME", silent.as_str())]);
    wait_for("both clients to attach", || {
        match attached(&mut conn, &session) {
            2 => Ok(()),
            n => Err(format!("{n} attached")),
        }
    });
    let where_it_is = format!("[{session}] 0:0*");
    for tui in [&always, &off] {
        wait_for("each row to say where its client is", || {
            says(tui, &where_it_is)
        });
    }
    let _ = wait_for_still(|| pane_size(&mut conn, &session));
    for (tui, name) in [(&always, "always"), (&off, "off")] {
        assert!(
            !tui.asked_for_focus_reports(),
            "{name} does not depend on where the person is, so its terminal must not be asked",
        );
    }

    // ONE message, addressed to the session, delivered to both mailboxes.
    let said = sprag_on(
        &sock,
        &loud,
        &[
            "display-message",
            "-t",
            &session,
            "one message, two clients",
        ],
    );
    assert!(said.status.success(), "display-message succeeded");

    // THE CONTROL: it reaches BOTH rows, so the difference below is about the policy and not about
    // one client having missed the message.
    //
    // ⚠ ONE observation window, LATCHING per client — not a `wait_for` each. A displayed message is
    // TIMED: it leaves the row again on its own. Waiting for `always` to show it and only then
    // starting to watch `off` spends the second client's whole display window on the first client's
    // round trip, and what the second wait then observes is the row AFTER the message has expired —
    // which reads exactly like a message that never arrived. That is the failure this test produced
    // under full-suite load at `4289edf` (`false at off: row reads "[0] 0:0*"`): a claim about a
    // TIMED message needs one observation window, not two. Latching rather than requiring both rows
    // to hold it at the SAME instant, so the gate does not swap one race for a tighter one.
    wait_for("the message to reach both rows", || {
        announced(&always, "one message, two clients")
            .and(announced(&off, "one message, two clients"))
            .map_err(|got| format!("{got} (the other client's rows are the pair to it)"))
    });

    // THE CLAIM: only the client whose file said `always` copied it out.
    wait_for("the loud client to copy it out", || {
        let (note, seq) = always.forwarded();
        settled(
            note.as_ref().map(|note| note.body.clone()),
            &Some(format!("[{session}] one message, two clients")),
        )
        .map_err(|got| format!("{got}: seq is {seq}"))
    });
    let (note, seq) = off.forwarded();
    assert_eq!(
        seq, 0,
        "`off` is the silence sprag had before this existed: the terminal holds {note:?}",
    );

    // Both are torn down here; `Tui::drop` ends each client.
    let _ = (&mut always, &mut off);
}

/// **A PERSON'S OWN `Alt-[` STILL REACHES THEIR PANE** — the branch that protects a binding from the
/// decoder that has to read ahead of it.
///
/// Found by the debt sweep, not by the design: `read_input` holds the event it read ahead and routes
/// it on the next turn, and every test above drives the case where that read-ahead found the OTHER
/// HALF OF A REPORT. Nothing drove the case where it found a keystroke — which is the case a person
/// with `Alt-[` bound reaches every time they press it, and the one where a lost event is a key that
/// silently did nothing.
///
/// The read-ahead is ARMED here (`unfocused` is the policy that asks), which is what makes this a
/// claim about the pushback rather than about a client that never looked: with `off` the bracket is
/// routed the instant it arrives and this would pass with no pushback in the code at all.
///
/// The pane echoes, so what the child received is readable as cells: the line discipline renders the
/// escape in caret notation, and `^[[x` is `ESC [ x` arriving whole and in order.
#[test]
fn a_persons_own_bracket_key_still_reaches_their_pane() {
    let config = ConfigHome::new("[options]\nnotify-outward = \"unfocused\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    wait_for("the row to say where the client is", || {
        says(&tui, &format!("[{session}] 0:0*"))
    });
    let _ = wait_for_still(|| pane_size(&mut conn, &session));
    assert!(
        tui.asked_for_focus_reports(),
        "the read-ahead must be armed, or this test proves nothing",
    );

    // `ESC [ x`: the parser makes it `Alt-[` and `x`, the read-ahead finds `x`, and `x` is not a
    // report — so BOTH have to reach the pane, in order.
    tui.type_bytes(b"\x1b[x");
    wait_for(
        "the person's own bracket and the key behind it to reach the pane",
        || {
            let text = pane_text_of(&mut conn, &session, 0);
            settled(text.contains("^[[x"), &true)
                .map_err(|got| format!("{got}: pane reads {text:?}"))
        },
    );
}

/// **The terminal is asked to stop reporting focus on the way out** — the release half, which
/// termwiz cannot do because termwiz did not set the mode.
///
/// Found by the debt sweep: `MouseMirror::release` exists for exactly this reason and this client now
/// owns a second mode with the same lifetime. A client that exited with 1004 still on would leave the
/// person's SHELL being told about every window switch, which a prompt renders as `^[[I`.
///
/// The CONTROL is the first assertion — the mode is ON while the client is live — so a build that
/// never asked for it at all could not pass this by doing nothing.
#[test]
fn the_focus_mode_is_given_back_when_the_client_leaves() {
    let config = ConfigHome::new("[options]\nnotify-outward = \"unfocused\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    wait_for("the row to say where the client is", || {
        says(&tui, &format!("[{session}] 0:0*"))
    });
    assert!(
        tui.asked_for_focus_reports(),
        "THE CONTROL: the mode is on while somebody is here to be reported about",
    );

    tui.type_bytes(&[0x02]); // the prefix
    tui.type_bytes(b"d"); // detach
    let status = tui.wait();
    assert!(
        status.success(),
        "the client exits on detach, not {status:?}"
    );
    wait_for("the daemon to release the client", || {
        settled(attached(&mut conn, &session), &0)
    });
    assert!(
        !tui.asked_for_focus_reports(),
        "a client that left must stop this terminal reporting focus to whatever runs next",
    );
}

/// **THE NOTIFICATION NAMES THE SESSION THE PERSON IS ON NOW** — not the one their client attached
/// to.
///
/// Found by the debt sweep, not by the design: the first version held the session in `Outward`,
/// taken once at start-up. A client can move (`switch-client`, R314; the chooser, R315) without
/// exiting, so that copy went stale the moment somebody pressed `prefix )` — and it went stale
/// against the STATUS ROW beside it, which re-derives the session every frame. The two surfaces
/// would have named different sessions for the same instant.
///
/// The CONTROL is the first forward, from the session the client started on: without it a build that
/// named no session at all would pass the claim below by accident.
#[test]
fn the_copy_names_the_session_the_client_is_on_now() {
    // `always`, so the claim is about WHICH session is named rather than about focus — one variable.
    let config = ConfigHome::new("[options]\nnotify-outward = \"always\"\n");
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "alpha" } }),
    )
    .expect("new_session answers");
    wait_for("the row to say where the client is", || {
        says(&tui, &format!("[{session}] 0:0*"))
    });

    // THE CONTROL: a message from where it started, naming where it started.
    let first = sprag_on(
        &sock,
        &config,
        &["display-message", "-t", &session, "before"],
    );
    assert!(first.status.success(), "display-message succeeded");
    wait_for("the first copy to name the session it started on", || {
        let (note, seq) = tui.forwarded();
        settled(
            note.as_ref().map(|note| note.body.clone()),
            &Some(format!("[{session}] before")),
        )
        .map_err(|got| format!("{got}: seq is {seq}"))
    });

    // `prefix )` — the client steps onto alpha and stays alive.
    tui.type_bytes(&[0x02]);
    tui.type_bytes(b")");
    wait_for("the client to step onto alpha", || {
        settled(attached(&mut conn, "alpha"), &1)
    });

    // THE CLAIM: the next copy names where it IS.
    let second = sprag_on(&sock, &config, &["display-message", "-t", "alpha", "after"]);
    assert!(second.status.success(), "display-message succeeded");
    wait_for("the copy to name the session the client moved to", || {
        let (note, seq) = tui.forwarded();
        settled(
            note.as_ref().map(|note| note.body.clone()),
            &Some("[alpha] after".to_owned()),
        )
        .map_err(|got| format!("{got}: seq is {seq}"))
    });
}

/// **A CHANGED `notify-outward` TAKES EFFECT WITHOUT RESTARTING THE CLIENT** — the same live-file
/// promise `sprag bind-key` rests on, applied to the one option this round added.
///
/// Found by the debt sweep. This client re-reads `config.toml` on every keystroke
/// (`ClientConfig::refresh`), so a policy frozen at start-up would have been the single setting in
/// that file needing a restart — and the failure would be invisible: the user edits, nothing
/// happens, and there is nothing to read that says why.
///
/// The claim is made through the TERMINAL's own mode rather than through a forward, because that is
/// the observable a person actually has: `off` means their terminal stops being asked about focus.
/// Both directions are driven, so a mirror that could only turn the mode on would fail.
#[test]
fn an_edited_notify_outward_takes_effect_without_a_restart() {
    let config = ConfigHome::new("[options]\nnotify-outward = \"off\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    wait_for("the row to say where the client is", || {
        says(&tui, &format!("[{session}] 0:0*"))
    });
    let _ = wait_for_still(|| pane_size(&mut conn, &session));
    assert!(
        !tui.asked_for_focus_reports(),
        "THE CONTROL: `off` asks this terminal nothing",
    );

    // The user edits their file and presses a key — the edge this client re-reads on.
    config.rewrite("[options]\nnotify-outward = \"unfocused\"\n");
    tui.type_bytes(&[0x02]); // the prefix: a key that reaches no pane and runs no action
    wait_for("the client to ask for focus reports after the edit", || {
        settled(tui.asked_for_focus_reports(), &true)
    });

    // ...and back, which is the direction a mirror that only ever turned things on would fail.
    config.rewrite("[options]\nnotify-outward = \"off\"\n");
    tui.type_bytes(&[0x02]);
    wait_for(
        "the client to give the mode back after the second edit",
        || settled(tui.asked_for_focus_reports(), &false),
    );
}

/// **`prefix z` ZOOMS, in a live terminal client** — closing the one keymap arm the register
/// (item 15, R288/R289) recorded as having no live driver at all.
///
/// ⚠ **The entry's WORDING was stale and is corrected by measuring it**: it said this file *"mentions
/// zoom zero times"*, and it now mentions it five times — every one of them in the KEY TABLE checks
/// (`list-keys` printing the `zoom-pane` row). The substance stood: nothing drove the ARM, which is
/// the client's own `zoom_pane` + reconcile + repaint in one breath. *A register item's wording is a
/// claim* — the sixth instance.
///
/// The zoom is a claim about what this client DRAWS and not about the pane set, which is what makes
/// the divider the right observable: the daemon still holds two panes of the same size throughout, so
/// a test reading `pane_sizes` would see nothing happen at all.
///
/// Four claims, ordered so each can fail on its own:
///
/// * **THE CONTROL, first**: two panes with a divider between them, which is the state a zoom has to
///   change and the state a build that zoomed nothing would leave.
/// * **The zoom takes the whole area**: the divider column is gone and the zoomed pane's own text is
///   painted past where the divider stood — so a client that merely stopped drawing the LINE would
///   fail the second half.
/// * **The daemon still holds both panes**, unchanged in size: a zoom that had closed or resized one
///   would be a different bug wearing this one's clothes.
/// * **It comes back**: the same key un-zooms, which a one-way flag would fail.
#[test]
fn the_zoom_key_gives_the_focused_pane_the_whole_area_and_gives_it_back() {
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client();

    // Something to recognise the zoomed pane BY: an empty pane filling the screen is
    // indistinguishable from an empty pane that did not move.
    tui.type_bytes(b"zoomee");
    wait_for("the typed text to come back painted", || {
        painted(&tui, "zoomee")
    });
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"%");

    let (near, far) = halves(BOOT_PANES.0);
    wait_for("both panes to reach their own half's size", || {
        settled(
            pane_sizes(&mut conn, &session),
            &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
        )
    });
    // THE CONTROL: the divider is standing, so the absence asserted below is a change.
    tui.wait_for("a divider to stand between the two panes", || {
        let column = tui.pane_column(near);
        settled(column.chars().all(|glyph| glyph == '\u{2502}'), &true)
            .map_err(|got| format!("{got}: column {near} reads {column:?}"))
    });
    // The split left the focus on the NEW pane, so go back to the one whose text we can name.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"o");
    wait_for(
        "the focus to come back to the pane that has text in it",
        || settled(active_pane(&mut conn, &session), &Some(0)),
    );

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"z");
    tui.wait_for(
        "the divider to go, leaving the zoomed pane the whole area",
        || {
            let column = tui.pane_column(near);
            settled(column.contains('\u{2502}'), &false)
                .map_err(|got| format!("{got}: column {near} still reads {column:?}"))
        },
    );
    // ...and the pane really OCCUPIES it: its own text is painted, and the columns the divider and
    // the other pane had are this pane's now.
    tui.wait_for("the zoomed pane to be painted across the terminal", || {
        settled(tui.span(0, 0..BOOT_PANES.0).starts_with("zoomee"), &true)
            .map_err(|got| format!("{got}: row 0 reads {:?}", tui.row(0)))
    });

    // ⚠ AND THE CHILD IS TOLD, which is what I expected NOT to happen and measured wrong: the first
    // version of this assertion said a zoom "must not resize a pane" and the daemon answered
    // `[(80, 23), (40, 23)]`. It is right and the assertion was wrong — a zoom that left the child
    // at half width would leave an editor reflowed for a pane it is no longer being shown in, which
    // is tmux's behaviour too. The pane SET is what a zoom may not touch; the zoomed pane's SIZE is
    // exactly what it changes.
    wait_for(
        "the zoomed pane's own child to be told it has the whole area",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(BOOT_PANES.0, BOOT_PANES.1), (far, BOOT_PANES.1)],
            )
        },
    );

    // ...and it comes back.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"z");
    tui.wait_for("the divider to come back when the zoom is released", || {
        let column = tui.pane_column(near);
        settled(column.chars().all(|glyph| glyph == '\u{2502}'), &true)
            .map_err(|got| format!("{got}: column {near} reads {column:?}"))
    });
    // ...and the child is told THAT too, which is the half a client that only repainted would miss.
    wait_for(
        "the released pane's child to be given its half back",
        || {
            settled(
                pane_sizes(&mut conn, &session),
                &vec![(near, BOOT_PANES.1), (far, BOOT_PANES.1)],
            )
        },
    );
}

/// A display client meeting a daemon that lacks an address it reads says the daemon is OLD, in the
/// same words the CLI and the agent surface use.
///
/// # Measured, and the first fix changed nothing
///
/// Against a peer that passes the handshake and serves no address, this client exited at boot with
/// `scene/query /sprag_mux/external/panes: host rpc error: UnknownIntrospectPath` — a Rust enum
/// variant at an operator. **Exiting is the right shape**: a display client with no panes to paint
/// has nothing to do, and failing loudly at boot beats painting a broken screen. Only the sentence
/// was wrong.
///
/// It is worth having as a live test rather than as a unit test over the mapping, because the first
/// attempt at the fix went into `read_slot` — a helper whose own doc warns that a copy will forget
/// the treatment — and the read that actually fails first was the copy. **Re-running this probe is
/// what said the fix had changed nothing a person sees.**
#[test]
fn a_client_meeting_an_older_daemon_says_so_instead_of_naming_a_variant() {
    let peer = stale_peer();
    let tui = Tui::attach_with_env(peer.sock(), "0", &[]);

    // It EXITS, and what it left on the terminal is the sentence.
    // Read WITHOUT whitespace, on both sides: a sentence this long wraps, and a terminal wraps
    // mid-word — so a row-joined screen contains `build o` + `f sprag` and matches no phrase
    // anybody wrote. The claim is about the words that reached the person, not about where the
    // 80th column fell.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut said = String::new();
    while Instant::now() < deadline {
        said = tui.rows().join("");
        if fold(&said).contains(&fold("does not serve")) || said.contains("UnknownIntrospectPath") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let screen = fold(&said);
    assert!(
        !screen.contains("UnknownIntrospectPath"),
        "a Rust variant name must not reach an operator: {said}",
    );
    for phrase in [
        "this daemon does not serve /sprag_mux/external/panes",
        "older than this build",
        "sprag kill-server",
    ] {
        assert!(
            screen.contains(&fold(phrase)),
            "the sentence names {phrase:?}: {said}",
        );
    }
}

/// **THE PROBE R324 OPENED WITH: what a RUNNING client does when its daemon cannot act.**
///
/// Register item 48 left this as the last unhandled half of the version-skew class, and as a
/// SURFACE DECISION rather than a defect: *"`WireHost::request` logs at debug and returns `None` —
/// a deliberate policy for a repaint loop (swallow is honest, not silent) ... whether a repaint
/// loop should say so is a question about how noisy a degraded client is allowed to be."*
///
/// The question is answerable now, and the answer this round takes is: **a person's GESTURE gets an
/// answer, and a poll does not shout.** The method is the discriminator — a `scene/invoke` happens
/// only because somebody acted, where a `scene/query` happens on every wake — so nothing has to be
/// passed down to the transport for it to know which it is looking at.
///
/// Measured at `d651f50` with exactly this fixture: `prefix c` against a daemon that serves every
/// read and knows no verb left the status row saying where the client was, unchanged, forever. The
/// window was not created and nothing on the screen said so.
///
/// The CONTROL is the row itself, asserted BEFORE the press: without it, a row that had been
/// showing the sentence all along would pass.
///
/// **The policy's OTHER half — that a failing READ says nothing — is gated in `sprag-client`'s
/// `tests/skew.rs` and not here, and the reason is a fixture this front cannot build**: a client
/// boots by reading its windows, its panes and its layout, so a peer missing those never lets one
/// start (measured, twice, while writing this). The transport-level gate can point a booted client
/// at an address it then loses, which is the state the policy is about.
#[test]
fn a_key_that_reaches_a_daemon_too_old_to_act_says_so() {
    let (_daemon, upstream) = spawn_daemon_running(&["cat"]);
    let sock = socket_path();
    let peer = sprag_peer::OldDaemon::proxying(&sock, &upstream, sprag_peer::Missing::actions());
    let mut conn = observe(&upstream);
    let session = boot_session(&mut conn);
    let mut tui = Tui::attach_with_env(peer.sock(), &session, &[]);

    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the client to be painting the session it is on", || {
        says(&tui, &where_it_is)
    });
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), true)],
        "one window, so a `prefix c` that LANDED would be visible behind the peer",
    );

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"c");
    let folded = |row: &str| -> String { row.chars().filter(|c| !c.is_whitespace()).collect() };
    tui.wait_for("the row to say the daemon could not do it", || {
        let rows = tui.status_rows();
        let row = rows
            .iter()
            .find(|row| folded(row).contains("doesnotperform"));
        let flat = row.map(|row| folded(row)).unwrap_or_default();
        // "does not PERFORM", which is the acting half — the reading half says "does not serve",
        // and the first draft of this probe watched for that one. It timed out against a working
        // fix, which is what a test looking for the wrong words looks like from the outside.
        if flat.contains("doesnotperform") {
            Ok(())
        } else {
            Err(format!("the rows painted were {rows:?}"))
        }
    });
    // Over every frame, not over the row now: the sentence expires, so a read taken after the wait
    // would be looking at a row the client has already left and the claim would go vacuous.
    assert!(
        !tui.status_rows()
            .iter()
            .any(|row| row.contains("UnknownInvokePath")),
        "a Rust variant name must not reach a person: {:?}",
        tui.status_rows(),
    );

    // ...and NOTHING HAPPENED, which is the half that makes the sentence worth painting.
    assert_eq!(
        windows_of(&mut conn, &session),
        vec![("0".to_owned(), true)],
        "the peer performs nothing, so the client is reporting a real failure",
    );
    assert_eq!(
        tui.liveness(),
        "running",
        "a client that cannot act is still a client: it says so and stays",
    );
}

/// A daemon OLDER than this client, serving NO address — [`sprag_peer`]'s, since R324. This file
/// had written one out for itself, one of four copies with three different refusal policies.
fn stale_peer() -> sprag_peer::OldDaemon {
    sprag_peer::OldDaemon::serving_nothing(&socket_path())
}

/// **THE GATE for R325's HEAD: a kill that took MORE than it was asked for says so.**
///
/// # The measurement this opened on, at `d1833df`
///
/// A live `sprag-tui`, `detach-on-destroy = next`, a session with one window and a spare session to
/// land in. `prefix &` — kill this window — destroyed the person's SESSION, moved them silently to
/// the neighbouring one, and left the status row **byte-for-byte unchanged**, still naming the
/// session that had just died. The daemon had answered `{"ended":"session"}` on the very same
/// reply; `HostClient::kill_window` returned `()`, the last two acting methods in that trait to do
/// so, so both display clients dropped it (R316's shape, met a final time).
///
/// # Three claims, and the CONTROL is what makes the first discriminating
///
/// 1. **The row is idle first** — without this, a row that had been showing the sentence all along
///    would pass.
/// 2. **The daemon really did take the session**, read from the daemon's own session list and not
///    from the client under test.
/// 3. **The client SAID SO**, in `Ended::beyond`'s wording — the same clause `sprag kill-window`
///    has printed since R309, so the two mouths cannot drift.
///
/// The `detach-on-destroy = next` policy is load-bearing rather than incidental: under the default
/// the client LEAVES, and a client that is gone has no row to be judged on. The case worth a
/// sentence is exactly the one where the person is still sitting there.
#[test]
fn a_window_kill_that_took_the_session_says_so() {
    let config = ConfigHome::new("[options]\ndetach-on-destroy = \"next\"\n");
    let (_daemon, sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    // A spare session, so the cascade stops at the SESSION and this client has somewhere to land.
    // Made through the CLI rather than a key, so the fixture does not depend on the client under
    // test having created it.
    let made = Command::new(sprag_cli_bin())
        .args(["new", "beta"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .output()
        .expect("run sprag new");
    assert!(made.status.success(), "the spare session must exist");

    // THE CONTROL: the row is idle before the key, so the sentence below is this key's.
    let where_it_is = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_is)
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"&");
    tui.type_bytes(b"y");

    // ⚠ EVERY DISTINCT ROW OVER ONE WINDOW, not three `wait_for`s in a row. A claim about a TIMED
    // message cannot be made by waiting for it and then asserting again later: `wait_for` returns on
    // its FIRST match and the row moves on, so a second assertion reads whatever replaced it. R325.1
    // misdiagnosed a working fix on exactly that, and R326's own first cut of the third assertion
    // below was VACUOUS for exactly that — it sampled only after the row had settled, by which time
    // a message that should not have existed had already come and gone. The mutation that put the
    // second sentence back came out GREEN, which is what said so.
    let rows = rows_until_settled(&tui, "the session went with it", "[beta] 0:0*");

    // Asked of the TRANSCRIPT, not of the rows: the sentence expires, and a row record samples at
    // `read` granularity, which coalesces under load (see [`Tui::transcript`]). The rows are still
    // the diagnostic, and the settled-row assertion below is still asked of them.
    assert!(
        tui.said("the session went with it"),
        "the kill reached past the window it named and must say so: {rows:?}",
    );

    // ONE GESTURE, ONE SENTENCE. The out-of-band path says *"session ... was destroyed"* — a passive
    // sentence about somebody ELSE's act — and for 150 ms R326 said it here too, over the top of this
    // gesture's own answer a fifth of the way into its `display-time`. A person who pressed
    // `prefix &` is not told their session was destroyed by parties unknown.
    assert!(
        !rows.iter().any(|row| row.contains("was destroyed")),
        "the gesture answered already; a second sentence blaming nobody must not follow it: {rows:?}",
    );

    // R326 CLOSES THE HALF R325.1 MEASURED AND LEFT: once the sentence expired the row named the
    // session that had just died (*"the client had switched to `beta` and the row went back to
    // `[0] 0:0*`"*). The switch happens inside this gesture's own dispatch now, so the row this
    // client settles on already names where it landed.
    assert_eq!(
        rows.last().map(String::as_str),
        Some("[beta] 0:0*"),
        "the row this client settles on must name where it IS, not the session that died: {rows:?}",
    );

    // ...and it is TRUE, read from the daemon rather than from the client that said it.
    wait_for("the daemon to be holding only the spare session", || {
        settled(session_names(&mut conn), &vec!["beta".to_owned()])
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "the client switched rather than leaving, which is what makes the sentence worth painting",
    );
}

/// **THE GATE for R328: a bound `move-pane` opens a chooser that says what it is FOR, and a pick
/// MOVES THE PANE** — driven at the shipped binary, on a real pseudoterminal.
///
/// # Why this exists and why a unit test could not stand in for it
///
/// R326 measured four documented values of `detach-on-destroy` doing NOTHING on this front, and the
/// reason was not a wrong answer anywhere — it was that no test drove the shipped client, so a
/// front that never called the resolve looked exactly like one that did. R328 adds a binding, a
/// trait method, an errand and a header across two crates; the unit gates prove each piece and
/// none of them proves that pressing the key moves a pane.
///
/// # The fixture is built so a MOVE is the only thing that could produce the end state
///
/// The client makes a second window with `prefix c`, so the focused pane is the ONLY pane of
/// window `1` and the only other pane in the session is window `0`'s. That makes the pick
/// unambiguous — `Errand::accepts` admits pane rows other than the mover, and there is exactly one
/// — so `Enter` needs no navigation and the test asserts the ACT rather than the arrow keys.
///
/// It also makes the outcome unmistakable: the move empties window `1`, which CLOSES it. A session
/// that ends with one window holding two panes cannot be produced by a chooser that merely went
/// somewhere, by a no-op, or by a client that painted a list and dropped the key.
///
/// The HEADER is asserted before the commit, and it is the half a mover-and-target check would
/// miss: two errands paint the same rows, so a front that opened the RIGHT list under the WRONG
/// question would move the pane and still have failed the person reading it.
#[test]
fn the_move_pane_key_opens_a_chooser_that_says_so_and_a_pick_moves_the_pane() {
    let config = ConfigHome::new("[[bind]]\nkey = \"m\"\naction = \"move-pane -v\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // A SECOND WINDOW, so the focused pane is the only one in its window and the only candidate is
    // elsewhere. Its emptying is what makes the move visible.
    tui.type_bytes(PREFIX);
    tui.type_bytes(b"c");
    wait_for("the key to make a second window", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), false), ("1".to_owned(), true)],
        )
    });

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"m");
    // IT SAYS WHAT IT IS FOR. `(move-pane -v)` is `chooser::Errand::asking()`, the canonical
    // spelling — the same words `bind-key` takes and `list-keys` prints. Before R328 this surface
    // said `(choose-tree)` whatever it had been opened to do.
    tui.wait_for(
        "the chooser to open under the question it was opened for",
        || {
            let row = tui.row(0);
            settled(row.contains("(move-pane -v)"), &true)
                .map_err(|got| format!("{got}: row reads {row:?}"))
        },
    );

    tui.type_bytes(b"\r");
    // THE ACT: window `1` held only the mover, so the move empties and closes it, and the survivor
    // holds both panes. Read from the DAEMON, not from the client that asked for it.
    wait_for("the pick to move the pane out of its window", || {
        settled(
            windows_of(&mut conn, &session),
            &vec![("0".to_owned(), true)],
        )
    });
    assert_eq!(
        pane_ids(&mut conn, &session).len(),
        2,
        "the surviving window holds both panes: the move carried one in rather than closing it",
    );
    assert_eq!(
        tui.liveness(),
        "running",
        "a move is not a departure; the client stays on the session it moved within",
    );
}

/// Each window of `session` and how many panes it holds, off the `tree` slot — the read that can
/// tell WHICH window a join landed in, where the windows list only says which still exist.
fn windows_and_panes(conn: &mut HostConn, session: &str) -> Vec<(String, usize)> {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(TREE_SLOT) }),
    )
    .ok()
    .and_then(|value| value.as_array().cloned())
    .unwrap_or_default()
    .iter()
    .find(|row| row["name"].as_str() == Some(session))
    .and_then(|row| row["windows"].as_array().cloned())
    .unwrap_or_default()
    .iter()
    .filter_map(|window| {
        Some((
            window["name"].as_str()?.to_owned(),
            window["panes"].as_array()?.len(),
        ))
    })
    .collect()
}

/// **THE GATE for R329: a bound `join-pane` opens a chooser that says what it is FOR, and a pick
/// puts the pane in THAT window** — driven at the shipped binary, on a real pseudoterminal.
///
/// # Why this exists
///
/// R328's own post-push debt question found that nothing pressed the key it had added: a front that
/// never calls looks identical to one that does, which is R326's class. This round adds a binding,
/// a wire grammar, two registry entries, a trait method and an errand, and every unit gate below
/// proves a piece while none of them proves that pressing the key moves a pane.
///
/// # THREE windows, because two cannot tell a right landing from a wrong one
///
/// With only the source and one destination, every join — and every mis-addressed join — produces
/// the same window list, so the gate would pass on a client that landed the pane anywhere. So the
/// client makes TWO more windows: the focused pane is the only pane of window `2`, and `0` and `1`
/// are both offerable. `ArrowDown` walks the cursor from the first offered room to the second, and
/// the end state names which one took the pane.
///
/// That is also what pins `Errand::accepts` end to end: the cursor steps over the SESSION row, the
/// pane rows and window `2` itself, so one press of `ArrowDown` moving it from `0` to `1` is only
/// true if the rows between them are unpickable.
///
/// # What this gate does NOT prove, and where that is proved instead
///
/// It does not discriminate an identity-addressed commit from a name-addressed one: the chooser
/// REFRESHES while it is open, so a stale label repairs itself before Enter and the two spellings
/// agree in any fixture a pseudoterminal can hold steady. That claim is made where it is
/// deterministic — at the registry, where the rename shuffle lands a name-addressed join in a
/// window nobody chose, and at the GUI's menu row, which paints once and never refreshes.
///
/// REVERT-PROOF: drop the `ArrowDown` and the pane lands in window `0`; make `Errand::accepts`
/// admit every row and it lands in window `0` too (the cursor stops on a pane row instead); open
/// the errand as `Goto` and three windows survive with the header reading `(choose-tree)`.
#[test]
fn the_join_pane_key_opens_a_chooser_that_says_so_and_a_pick_puts_the_pane_in_that_window() {
    let config = ConfigHome::new("[[bind]]\nkey = \"j\"\naction = \"join-pane\"\n");
    let (_daemon, _sock, mut conn, session, mut tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    for expected in [
        vec![("0".to_owned(), false), ("1".to_owned(), true)],
        vec![
            ("0".to_owned(), false),
            ("1".to_owned(), false),
            ("2".to_owned(), true),
        ],
    ] {
        tui.type_bytes(PREFIX);
        tui.type_bytes(b"c");
        wait_for("the key to make a window", || {
            settled(windows_of(&mut conn, &session), &expected)
        });
    }

    tui.type_bytes(PREFIX);
    tui.type_bytes(b"j");
    // IT SAYS WHAT IT IS FOR — `chooser::Errand::asking()`, the canonical spelling `bind-key` takes
    // and `list-keys` prints. Two errands paint the same rows, so opening the right list under the
    // wrong question is a defect no end-state check can see.
    tui.wait_for(
        "the chooser to open under the question it was opened for",
        || {
            let row = tui.row(0);
            settled(row.contains("(join-pane)"), &true)
                .map_err(|got| format!("{got}: row reads {row:?}"))
        },
    );

    tui.type_bytes(&[0x1b, b'[', b'B']); // ArrowDown: from window `0`'s row to window `1`'s.
    tui.type_bytes(b"\r");

    // THE ACT, read from the DAEMON rather than from the client that asked for it: window `2` held
    // only the mover, so the join empties and CLOSES it, and window `1` — not `0` — holds two.
    wait_for("the pick to put the pane in the window it named", || {
        settled(
            windows_and_panes(&mut conn, &session),
            &vec![("0".to_owned(), 1), ("1".to_owned(), 2)],
        )
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "a join is not a departure; the client stays on the session it moved within",
    );
}

/// EVERY DISTINCT STATUS ROW from now until the row SETTLES on `landing` — the one observation
/// window every claim about a timed message is made from.
///
/// ## Why the window ends on a CONDITION and not on a clock
///
/// This file records the rule three times over (R324, R325.1, R326): a claim about a message that
/// EXPIRES cannot be made by a `wait_for` and a later assertion, because `wait_for` returns on its
/// FIRST match and the row moves on. R326 fixed that by collecting every distinct row over a fixed
/// **3-second** window — which closed the sampling hole and left a second one behind it.
///
/// **Measured after R327's debt question: 1 full-workspace run in 6.** Under the load the whole
/// suite applies, the gesture, the daemon round trip and the message's own `display-time` do not
/// fit in three seconds, so the window ended with the message still on screen and the *"the row
/// settles on where it landed"* assertion read the message as the settled row.
///
/// **⚠ R333 MEASURED IT AGAIN: 1 full-workspace run in 3, and the third attempt is the one that
/// stops sampling.** The diagnostic was `["[0] 0:0*", "[beta] 0:0*"]` — the sentence missing
/// entirely — and four isolated runs showed the client emits it in order every time, so nothing was
/// wrong with the ORDER. The loop simply never looked while it was there: a `display-time` message
/// lives on the row for well under a second, and under full-suite load this process can be
/// descheduled for longer than that. **Sampling a transient is unsound at ANY interval**, so both
/// previous fixes were tightening a clock that cannot be made tight enough.
///
/// The rows now come from [`Tui::status_trail`], which the READER THREAD appends to as it applies
/// each batch — every row the client painted is in it whether anybody was looking or not. This
/// function only decides when to STOP waiting, and the generous cap is a failure bound rather than a
/// sample point: a run that never settles returns everything the client ever painted, and the
/// caller's own assertion prints the whole list. One list, every claim, no clock to tune.
fn rows_until_settled(tui: &Tui, sentence: &str, landing: &str) -> Vec<String> {
    tui.wait_for(
        &format!("this client to say {sentence:?} and then come to rest on {landing:?}"),
        || {
            // BOTH conditions, and the sentence is the one that was missing. ⚠ SETTLING IS NOT THE
            // CLAIM: a client can reach the row it lands on BEFORE it writes the sentence its
            // gesture owes, and a window that ended on the landing alone has stopped watching
            // before the thing under test happened. Measured at 2x CPU oversubscription, where this
            // returned `["[0] 0:0*", "[beta] 0:0*"]` and the caller's assertion failed against a
            // LOSSLESS byte stream — so the sentence was not missed, it had not been written yet.
            // That is R327's own rule biting one condition further in: end the window on what the
            // claim is about.
            //
            // The LAST row, not "any row": a message can carry the landing's name inside it, and
            // the second claim is about what the client comes to rest on.
            let rows = tui.status_rows();
            if rows.last().map(String::as_str) == Some(landing) && tui.said(sentence) {
                return Ok(());
            }
            Err(format!("the rows painted were {rows:?}"))
        },
    );
    tui.status_rows()
}

/// A live `sprag-tui` under `detach-on-destroy = policy`, with a spare session `beta` to land in
/// and its own boot session destroyed OUT OF BAND by the `sprag` CLI.
///
/// Handed back after the kill has been ACKNOWLEDGED by the daemon, so the caller's first `wait_for`
/// is watching the client rather than racing the killer.
fn client_whose_session_is_destroyed(policy: &str) -> (Daemon, ConfigHome, HostConn, String, Tui) {
    let config = ConfigHome::new(&format!("[options]\ndetach-on-destroy = \"{policy}\"\n"));
    let (daemon, _sock, mut conn, session, tui) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );

    // THE CONTROL, and it runs BEFORE the spare exists because the spare is what everything below
    // is about: the row names the session this client booted into, so every reading later is the
    // kill's doing.
    let where_it_was = format!("[{session}] 0:0*");
    wait_for("the row to say where the client is", || {
        says(&tui, &where_it_was)
    });

    // OUT OF BAND, and BACK TO BACK ON ONE CONNECTION: a spare session to land in, and then this
    // client's own session destroyed. Not a key, deliberately — a gesture gets its own answer
    // (`Report::cascaded`), and this path is the one where nobody at this keyboard did anything.
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "beta" } }),
    )
    .expect("the spare session must exist");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(KILL_SESSION_ACTION), "args": { "name": session } }),
    )
    .expect("the killer must have worked");
    (daemon, config, conn, session, tui)
}

/// **THE GATE for R326: a session destroyed under a terminal client MOVES it, and it says so.**
///
/// # The measurement this opened on, at `6884445`
///
/// A live `sprag-tui`, `detach-on-destroy = "next"`, a spare session to land in, and `sprag
/// kill-session 0` run from another process. The client **stayed exactly where it was** — still
/// `running`, still painting `[0] 0:0*`, naming a session the daemon no longer held, for as long as
/// it was left alone. The same reading under `off`, `previous` and `no-detached`.
///
/// The default policy worked, and it worked for a reason that has nothing to do with this front: a
/// DETACH is performed by the wire client's own poll thread and needs no client's cooperation. The
/// four SWITCH policies need a UI-thread resolve, `sprag-gui` called it from its per-frame
/// reconcile, and **this front had never called it at all** — so four of the five values of a
/// documented option did nothing on half the product.
///
/// # Three claims, and the third is the one R325.1 left behind
///
/// 1. **It says what happened**, in [`sprag_host::report::Report::lost_session`]'s wording — the
///    one place either frontend takes the sentence from.
/// 2. **It is TRUE**, read from the daemon's own session list rather than from the client saying it.
/// 3. **The row then names where it landed** — asserted AFTER the sentence expires, which is the
///    half R325.1 measured and left: *"the status row still names the dead session"*. The order of
///    these two waits is the test: a row read too early sees the message, and a message read too
///    late has expired ([`wait_for`] prints its LAST observation, which is how that reading was
///    misdiagnosed once already).
#[test]
fn a_destroyed_session_moves_the_terminal_client_and_says_so() {
    let (_daemon, _config, mut conn, session, tui) = client_whose_session_is_destroyed("next");

    // ONE OBSERVATION WINDOW for both claims about the row, for the reason
    // [`rows_until_settled`] states — and this test is where that reason was MEASURED. Its three
    // `wait_for`s failed 1 full-workspace run in 6: under load the sentence had come and gone
    // before the first one sampled, and the diagnostic printed the settled row (`[beta] 0:0*`),
    // which reads as *"it never said anything"* rather than as *"you looked too late"*.
    let says = format!("session {session:?} was destroyed; now on \"beta\"");
    let rows = rows_until_settled(&tui, &says, "[beta] 0:0*");
    // Asked of the TRANSCRIPT — see the sibling gate above and [`Tui::transcript`].
    assert!(
        tui.said(&says),
        "the client must say what happened to it: {rows:?}",
    );
    // ...and once the sentence expires the row names WHERE THIS CLIENT IS, not the session that
    // died. `Status` is derived from the host every frame, so this is a claim about the ATTACHMENT
    // having moved and not about a repaint.
    assert_eq!(
        rows.last().map(String::as_str),
        Some("[beta] 0:0*"),
        "the row this client settles on must name where it landed: {rows:?}",
    );

    // ...and it is TRUE, read from the daemon rather than from the client that said it.
    wait_for("the daemon to be holding only the spare session", || {
        settled(session_names(&mut conn), &vec!["beta".to_owned()])
    });
    assert_eq!(
        tui.liveness(),
        "running",
        "a switch policy moves the client; it does not end it",
    );
}

/// **All four SWITCH policies move this client** — the property that was false for every one of
/// them, so a gate for one would leave three unmeasured.
///
/// The landing is `beta` for all four here and that is not an accident worth hiding: with exactly
/// two sessions, the ±1 list neighbour and both MRU fallbacks resolve to the same survivor. What
/// separates the policies is which session they PREFER among several, which
/// `destroy_successor`'s own unit tests decide; what this drives is the half those tests cannot
/// reach — that the policy is applied at all, by the shipped binary, on a real pseudoterminal.
///
/// The DEFAULT is the fifth value and is not here: it detaches, and it is driven by
/// [`the_client_leaves_when_its_session_is_destroyed`] on the path it has always taken.
#[test]
fn every_switch_policy_moves_the_terminal_client() {
    for policy in ["off", "no-detached", "next", "previous"] {
        let (_daemon, _config, _conn, _session, tui) = client_whose_session_is_destroyed(policy);
        wait_for(
            &format!("`detach-on-destroy = {policy:?}` to move the client to the survivor"),
            || says(&tui, "[beta] 0:0*"),
        );
        assert_eq!(
            tui.liveness(),
            "running",
            "`detach-on-destroy = {policy:?}` must switch, not leave",
        );
    }
}

/// The DEFAULT policy detaches, which is tmux's rule and the one value of this option that worked
/// before R326 — kept as the CONTROL for the four above: without it, a build that detached under
/// every policy would satisfy nothing here and a build that switched under every policy would look
/// the same as a correct one.
#[test]
fn the_client_leaves_when_its_session_is_destroyed() {
    let (_daemon, _config, mut conn, _session, tui) = client_whose_session_is_destroyed("on");
    tui.wait_for(
        "the client to leave the terminal it cannot serve",
        || match tui.liveness() {
            gone if gone.starts_with("EXITED") => Ok(()),
            still => Err(still),
        },
    );
    assert_eq!(
        session_names(&mut conn),
        vec!["beta".to_owned()],
        "the spare session outlives the client that could not reach it",
    );
}

/// **THE GATE FOR R327: `no-detached` LEAVES rather than join a session somebody else is in.**
///
/// R326 measured the opposite, on exactly this fixture: the client whose session was destroyed
/// walked into the session the other one was sitting in, its row reading `[beta] 0:0*` with `beta`
/// holding two clients. It reproduced 2 of 5 full-workspace runs and never once alone — it needed
/// the LOAD — so the gate was REMOVED rather than shipped flaky, and the cause was written down.
///
/// # ⚠ THIS IS COMPOSITION COVERAGE, NOT THE DISCRIMINATOR — measured, not assumed
///
/// R327 restored it because it now PASSES, and it passes stably: **10 of 10 runs green**, and green
/// again under the full-workspace load that used to break it. What it is NOT is the gate that would
/// catch the defect coming back. Measured, by mutation:
///
/// * remove the fresh re-read entirely (`plan_successor` decides on the mirror alone, which is the
///   pre-R327 product) and this reddens **1 run in 3** — the failure reading exactly R326's:
///   `running`, row `[beta] 0:0*`;
/// * keep the read and decide the OCCUPANCY on the mirror anyway and it stays green **10 of 10**.
///
/// So the staleness this exists to punish is still only visible when the wake ordering cooperates,
/// which is what R326 found and why the gate was removed rather than shipped. **The deterministic
/// discriminators are elsewhere and both exist**: the daemon half is
/// `a_dead_scope_still_reads_the_registry_and_still_refuses_a_session_over_the_real_socket`
/// (`sprag-host`, over the real socket, red under either dispatch path regressing), and the client
/// half is `the_occupancy_comes_from_now_and_the_order_comes_from_what_the_person_saw`
/// (`sprag-client`, red under either list being read for the other's question). A future reader
/// must not treat a green here as coverage of the decision.
///
/// What it does buy, and why it is worth its second pseudoterminal: it is the only place the whole
/// composition runs — two shipped clients, a real attachment count, a real out-of-band kill — and
/// the only fixture that reaches [`sprag_host::wake::Lost::Detached`], a switch policy that ran out
/// of places to go, which had no production coverage at all while it was gone.
///
/// # What it takes for the reading to mean anything
///
/// The survivor must be genuinely OCCUPIED — by a second real client on its own pseudoterminal, not
/// by a number written into a fixture — because the whole claim is that this client can see what
/// another client did. And the neighbour is asserted to still hold exactly ONE client afterwards:
/// a build that joined it would be caught by the row, but a build that joined it and then left
/// would not, and that is a different bug with the same screen.
#[test]
fn no_detached_leaves_rather_than_join_an_occupied_session() {
    let config = ConfigHome::new("[options]\ndetach-on-destroy = \"no-detached\"\n");
    let (_daemon, sock, mut conn, session, mine) = attached_client_with(
        |sock, session| {
            Tui::attach_with_env(sock, session, &[("XDG_CONFIG_HOME", config.as_str())])
        },
        &["cat"],
    );
    let made = Command::new(sprag_cli_bin())
        .args(["new", "beta"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .output()
        .expect("run sprag new");
    assert!(made.status.success(), "the only survivor must exist");

    // SOMEBODY ELSE IS ALREADY IN IT — a second client on its own pseudoterminal, so the count this
    // policy turns on is one a real attach produced.
    let _neighbour = Tui::attach_with_env(&sock, "beta", &[("XDG_CONFIG_HOME", config.as_str())]);
    wait_for(
        "the daemon to count the neighbour as attached to beta",
        || settled(attached(&mut conn, "beta"), &1),
    );
    // THE CONTROL, before the kill: this client is where it booted, so every reading below is the
    // kill's doing.
    let where_it_was = format!("[{session}] 0:0*");
    wait_for("the row to say where this client is", || {
        says(&mine, &where_it_was)
    });

    // OUT OF BAND, by a third process — the path where nobody at either keyboard did anything.
    let killed = Command::new(sprag_cli_bin())
        .args(["kill-session", &session])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .output()
        .expect("run sprag kill-session");
    assert!(
        killed.status.success(),
        "the killer must have worked: {:?}",
        String::from_utf8_lossy(&killed.stderr),
    );

    mine.wait_for(
        "the client to LEAVE rather than sit down beside somebody",
        || match mine.liveness() {
            gone if gone.starts_with("EXITED") => Ok(()),
            still => Err(format!("{still}; row reads {:?}", mine.row(STATUS_ROW))),
        },
    );
    assert_eq!(
        attached(&mut conn, "beta"),
        1,
        "beta held one client before the kill and must hold exactly one after it — a build that \
         joined and then left would pass the reading above and is a different defect",
    );
}
