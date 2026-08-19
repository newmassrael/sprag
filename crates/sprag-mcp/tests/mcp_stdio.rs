//! **THE gate for the agent-facing surface**: the shipped `sprag-mcp` binary, spoken to the way an
//! MCP client speaks to it — newline-delimited JSON-RPC 2.0 on its stdin/stdout — against a real
//! `sprag-term` daemon with real panes.
//!
//! Until this file existed, `sprag-mcp` had no integration harness at all. Every one of its tests
//! called a handler function directly, which leaves three whole layers unexercised:
//!
//! * **The protocol loop.** `serve` and `dispatch` had no test of any kind. Whether a real
//!   `initialize` line gets a response, whether a NOTIFICATION correctly gets none, whether one
//!   malformed line ends the session or is dropped — none of it was observed. A server that answers
//!   a notification, or dies on a stray blank line, is broken for every client while every unit test
//!   stays green.
//! * **stdout as a protocol channel.** The crate's rule is that stdout carries only protocol JSON
//!   and every diagnostic goes to stderr. That is a property of the PROCESS, so no in-process test
//!   can hold it, and its failure mode is total: one log line on stdout desynchronises the client.
//! * **The wire vocabulary.** Every tool spells a path with the [`sprag_host::wire`] SSOT and hands
//!   it to a daemon written in another crate. A unit test over hand-written JSON agrees with itself
//!   about all of those names — the seam R253 was bitten at, and the one the `sprag` CLI's live test
//!   holds for the CLI's four field names but not for this crate's slots.
//!
//! # What only a live run can prove
//!
//! The strongest single claim here is **the 1-based pane number reaches the pane it names**. A tool
//! argument of `2` has to become a host pane id that is not `2`, through one list query, and land in
//! the second pane's PTY and nowhere else. Distinct text is typed into each of two panes and each is
//! read back: the numbering, the id mapping, the text action, the key action and the read slot all
//! have to be right at once, and crosstalk in either direction fails it.
//!
//! # The socket resolve, and why this harness must defend against its own subject
//!
//! `host_sock`'s two layers are what let the server self-configure in any pane:
//! its own `SPRAG_HOST_RPC_SOCK`, else the first `/proc` ancestor that carries one. The second layer
//! is a hazard *for a test suite*, because sprag's own developers run this suite inside a sprag pane
//! — where the ancestor walk finds their LIVE terminal. A test that forgot to set the env var would
//! not fail; it would type into the panes the author is working in.
//!
//! So the precedence is not a detail, it is the safety property, and it is asserted here rather than
//! assumed: an ancestor advertising a DIFFERENT daemon is set up deliberately, and the answer must
//! come from the socket the child was given. The complementary test proves the walk works at all,
//! which is the reason the crate is usable in a real pane and had never been observed.
//!
//! Both rest on two premises about `/proc` that were measured before they were written down: a
//! shell's `/proc/<pid>/environ` is its EXEC-time snapshot, so an ancestor that has since `unset` the
//! variable still advertises it (this is also why the real feature works — a pane shell's environ
//! carries what the daemon exported), and `/bin/sh` here forks its last command rather than
//! `exec`ing into it. The second premise is not portable, so [`McpServer::assert_forked`] turns it
//! into an assertion: a shell that tail-execs makes this file fail loudly instead of quietly proving
//! less.
//!
//! # Waiting
//!
//! Every wait polls the CONDITION the assertion reads. Three processes and two sockets sit between a
//! tool call and a painted cell, so a fixed sleep would be a flake, a slow test, or both. The one
//! place a *negative* is asserted — a notification produces no line — uses ordering rather than a
//! quiet window: the next request's response must be the FIRST line to arrive, which is only true if
//! the notification produced none.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_host::mux_action_path;
use sprag_host::wire::{
    KILL_WINDOW_ACTION, NEW_SESSION_ACTION, NEW_WINDOW_ACTION, RENAME_PANE_ACTION,
    REPORT_AGENT_ACTION, SELECT_WINDOW_ACTION, SET_FLOATING_ACTION, SPAWN_ACTION, SPLIT_ACTION,
    SelectAsk, ZOOM_PANE_ACTION,
};
use sprag_rpc::HostConn;

/// How long any single condition may take before this file calls it a failure.
///
/// A deadline for a HANG, not a guess at how long the work takes: every wait returns the instant its
/// condition holds. It has to clear the server's own 6-second host-connect window, because one test
/// deliberately points it at a socket nobody is serving and waits for the error that produces.
const DEADLINE: Duration = Duration::from_secs(30);

/// How often a polled condition is re-checked.
const POLL: Duration = Duration::from_millis(50);

/// The env var the daemon exports to its panes, and the one both resolve layers read.
const SOCK_ENV: &str = "SPRAG_HOST_RPC_SOCK";

/// The IDENTITY half of the same rendezvous — which pane a process is running in. Named from the
/// host crate rather than respelled, so this file cannot drift from what the daemon publishes.
const PANE_ENV_VAR: &str = sprag_host::PANE_ENV_VAR;

/// The boot-pane size of the daemon a test normally gets — deliberately not a common default, so
/// "the answer came from THIS daemon" is checkable off the pane list alone.
const BOOT_PANE: (u16, u16) = (40, 6);

/// A second daemon's boot-pane size, for the resolve-order tests: the two daemons are told apart by
/// the geometry they report, which needs no child behaviour and no waiting.
const OTHER_PANE: (u16, u16) = (33, 7);

/// The pane for the test whose child paints a link, an image and a command cycle: the artifacts have
/// to still be ON SCREEN when the tools ask, and six rows scroll the link away.
const TALL_PANE: (u16, u16) = (40, 12);

// ----- the daemon -----

/// Kills and reaps the spawned daemon on scope exit (including a test panic), and unlinks its socket
/// so a failed run leaves no file behind either. The kill comes first: the daemon holds the socket
/// open until it exits.
///
/// A near-copy of `sprag-host`'s own `HostChild` and `sprag-tui`'s `Daemon`, deliberately not shared:
/// they are different packages, and exporting a test harness from a library — or adding a crate to
/// hold twenty lines — would cost more than the copy does.
struct Daemon(Child, PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// The `sprag-term` binary this file drives: a sibling of the `sprag-mcp` cargo built for the test.
///
/// Cargo sets `CARGO_BIN_EXE_*` only for binaries of the package under test, and the daemon belongs
/// to `sprag-host` — so the path is derived rather than given, and its ABSENCE is a loud failure
/// rather than a skip. A skipped gate is a green tick over an untested claim, which is the failure
/// mode this whole file exists to prevent.
///
/// ⚠⚠⚠ **AND R367 MEASURED THE STALENESS HALF RIGHT HERE.** A mutation that removed a whole wire
/// key from the daemon left `the_agent_tools_report_the_daemons_own_verdict` GREEN, because
/// `cargo test -p sprag-mcp` builds the `sprag-host` LIB and never its BINS — so this file was
/// driving a daemon from older source while asserting about the edit. The absence check this
/// replaces cannot see that: the binary is there. [`sprag_gate::sibling_bin`] asks cargo's own
/// depfile instead, and it is shared with the pty suite so neither site can drift from the other.
fn sprag_term_bin() -> PathBuf {
    sprag_gate::sibling_bin(env!("CARGO_BIN_EXE_sprag-mcp"), "sprag-term")
}

/// The `sprag` CLI binary, reached the same way and for the same reason.
///
/// One gate here needs a REAL REPORTER — `sprag hook claude` is the process whose build the agent
/// surface judges, and it belongs to `sprag-host` like the daemon does. A hand-written report over
/// the wire would state whatever this test chose to state, which is the one thing the gate must not
/// be allowed to decide.
fn sprag_cli_bin() -> PathBuf {
    sprag_gate::sibling_bin(env!("CARGO_BIN_EXE_sprag-mcp"), "sprag")
}

/// A state home unique to this CALL, for the tests whose subject is a file rather than the wire.
///
/// Never the developer's: `sprag-gate`'s ambient-home guard fails a suite that writes under one,
/// and the parallel siblings in this binary would collide on a path keyed only on the pid.
fn state_home() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("sprag-mcp-state-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the state home");
    dir
}

/// Run the REAL `sprag hook claude` for `pane` against the daemon at `sock`, with `state` as its
/// state home, and wait for it to exit.
///
/// The payload is an agent's own `UserPromptSubmit` — the event that opens a turn — on stdin, which
/// is how the agent runs it. Its exit status is deliberately not judged: a hook swallows every
/// failure and always exits 0, on purpose, so what a caller learns from it is the report that landed
/// or the word it left behind.
fn run_hook(sock: &Path, state: &Path, pane: u64) {
    let mut child = Command::new(sprag_cli_bin())
        .args(["hook", "claude"])
        .env(SOCK_ENV, sock)
        .env(PANE_ENV_VAR, pane.to_string())
        .env("XDG_STATE_HOME", state)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the sprag hook");
    child
        .stdin
        .take()
        .expect("the hook's stdin")
        .write_all(br#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#)
        .expect("write the hook's payload");
    child.wait().expect("the hook exits");
}

/// A socket path unique to this CALL (pid + a per-binary counter).
///
/// The counter is load-bearing: cargo runs this file's tests as parallel threads of one binary, and
/// each spawn unlinks its path first — a path keyed only on the pid would remove the socket a
/// sibling test is serving on.
fn socket_path() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-mcp-it-{}-{n}.sock", std::process::id()))
}

/// Spawn a non-daemon `sprag-term` whose boot pane runs `program`, at `size`.
///
/// Usually `cat`: it blocks on its PTY so the pane stays alive for the test's duration, and what is
/// typed at it comes straight back, which is how a write is proven to have ARRIVED without needing a
/// shell.
fn spawn_daemon(program: &[&str], size: (u16, u16)) -> (Daemon, PathBuf) {
    spawn_daemon_with(program, size, &[])
}

/// The one spawn, with `env` overrides on the DAEMON's own environment — what a test needs when the
/// thing under test is a file the daemon reads rather than a pane it runs (`XDG_CONFIG_HOME`, so a
/// test can point the daemon at a config of its own without touching the environment its parallel
/// siblings are reading).
fn spawn_daemon_with(
    program: &[&str],
    size: (u16, u16),
    env: &[(&str, &str)],
) -> (Daemon, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let mut command = Command::new(sprag_term_bin());
    command
        .arg("--size")
        .arg(format!("{}x{}", size.0, size.1))
        .arg("--")
        .args(program)
        .env(SOCK_ENV, &sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn the sprag-term daemon");
    (Daemon(child, sock.clone()), sock)
}

/// Add a pane running `program` to the daemon at `sock`, returning its host id.
///
/// Over the mux spawn action rather than through the `sprag` CLI: that binary belongs to a third
/// package, and the point here is a second pane, not a second way of asking for one.
/// The pane ids the daemon's UNSCOPED `panes` slot lists — the current window's, in its order.
/// The panes of ONE named window, whichever window the session is currently on — R311's
/// `WINDOW_PARAM`, used here to reach into a window an agent opened DETACHED (so it is by
/// construction not the current one).
fn mux_query_panes_in(sock: &Path, window: &str) -> Vec<u64> {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/query",
        json!({
            "path": mux_action_path(sprag_host::wire::PANES_SLOT),
            sprag_rpc::WINDOW_PARAM: window,
        }),
    )
    .expect("the pane list")
    .as_array()
    .map(|panes| {
        panes
            .iter()
            .filter_map(|pane| pane.get("id")?.as_u64())
            .collect()
    })
    .unwrap_or_default()
}

/// The name of the window the session is CURRENTLY on — the one fact `open_window` must not move
/// and `select_window` must.
fn mux_current_window(sock: &Path) -> String {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(sprag_host::wire::WINDOWS_SLOT) }),
    )
    .expect("the window list")
    .as_array()
    .into_iter()
    .flatten()
    .find(|window| window["current"].as_bool().unwrap_or(false))
    .and_then(|window| window["name"].as_str())
    .expect("a session always has a current window")
    .to_owned()
}

fn mux_query_panes(sock: &Path) -> Vec<u64> {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(sprag_host::wire::PANES_SLOT) }),
    )
    .expect("the pane list")
    .as_array()
    .map(|panes| {
        panes
            .iter()
            .filter_map(|pane| pane.get("id")?.as_u64())
            .collect()
    })
    .unwrap_or_default()
}

fn add_pane(sock: &Path, program: &[&str]) -> u64 {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": program } }),
    )
    .expect("spawn a pane")
    .as_u64()
    .expect("the spawn action answers with a pane id")
}

/// Invoke one mux action on the daemon at `sock` and return its answer.
///
/// The arrangement `pane_layout` reports can only be BUILT by acting on the daemon, and three of the
/// actions used here (`split`, `set_floating`, `zoom_pane`) have no `sprag-mcp` tool — this crate
/// drives them the way any other client would, over the same wire.
fn mux_invoke(sock: &Path, action: &str, args: Value) -> Value {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(action), "args": args }),
    )
    .unwrap_or_else(|error| panic!("{action}: {error}"))
}

/// Divide `pane` and spawn a new one into the half it opens, returning the new pane's id.
fn split_pane(sock: &Path, pane: u64, dir: &str) -> u64 {
    mux_invoke(
        sock,
        SPLIT_ACTION,
        json!({ "pane": pane, "dir": dir, "cmd": ["cat"] }),
    )
    .as_u64()
    .expect("the split action answers with a pane id")
}

// ----- the server, as a client sees it -----

/// The shipped `sprag-mcp` process plus the two ends of its protocol pipe.
///
/// Reading is done by a thread into a channel rather than inline, for two reasons: a response that
/// never comes has to become a failed assertion instead of a hung suite, and the stderr stream has
/// to be drained concurrently or a chatty log run would fill its pipe and block the server mid-reply.
struct McpServer {
    /// The direct child. `None` once it has been reaped — the orphan case waits for its shim to exit
    /// before talking to the server, so there is nothing left to reap.
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        // Closing stdin is how a client ends an MCP stdio server: the read loop sees EOF and returns.
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl McpServer {
    /// The ordinary spawn: a direct child told exactly which daemon to talk to.
    ///
    /// Setting the variable is not a convenience — it is the guard that keeps this suite off the
    /// author's live terminal, since a child with no variable would resolve one from the ancestor
    /// chain. `the_child_env_socket_wins_over_an_ancestors` is what proves the guard holds.
    fn spawn(sock: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag-mcp"));
        cmd.env(SOCK_ENV, sock);
        Self::from_command(cmd)
    }

    /// The ordinary spawn, with a STATE HOME of its own.
    ///
    /// One tool here reads a file rather than the wire — a hook that could not deliver leaves word
    /// under `$XDG_STATE_HOME/sprag` and the daemon is by definition the party that cannot know —
    /// so the reader and the writer have to agree on which home, and neither may be the developer's
    /// (`sprag-gate`'s ambient-home guard, and the parallel siblings in this binary).
    fn spawn_with_state(sock: &Path, state: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag-mcp"));
        cmd.env(SOCK_ENV, sock).env("XDG_STATE_HOME", state);
        Self::from_command(cmd)
    }

    /// A server that is RUNNING IN pane `pane` of the daemon at `sock` — the production shape for
    /// `pane_layout`'s "you are here", where the daemon exported the id and its own address into one
    /// environment and the MCP client forwarded both.
    fn spawn_in_pane(sock: &Path, pane: u64) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag-mcp"));
        cmd.env(SOCK_ENV, sock).env(PANE_ENV_VAR, pane.to_string());
        Self::from_command(cmd)
    }

    /// A server talking to `sock`, whose ANCESTOR advertises a pane of a DIFFERENT daemon.
    ///
    /// The case that must mark nothing. Pane ids are per-daemon and start at zero, so a box running
    /// two sprag terminals has two pane `1`s: an id taken from the nearest environment that happens
    /// to carry one names a real, plausible pane of the terminal being asked about, and the mark
    /// would be wrong in the one way nobody can see. The child's own environment carries the socket
    /// and no id, which is exactly what a client that forwards one variable and not the other leaves
    /// behind.
    fn spawn_behind_foreign_pane(own: &Path, ancestor: &Path, ancestor_pane: u64) -> Self {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("unset {PANE_ENV_VAR}; {SOCK_ENV}=\"$1\" \"$0\""))
            .arg(env!("CARGO_BIN_EXE_sprag-mcp"))
            .arg(own)
            .env(SOCK_ENV, ancestor)
            .env(PANE_ENV_VAR, ancestor_pane.to_string());
        let server = Self::from_command(cmd);
        server.assert_forked(ancestor);
        server
    }

    /// A direct child with the given log level, for the stdout-purity claim.
    fn spawn_logging(sock: &Path, level: &str) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag-mcp"));
        cmd.env(SOCK_ENV, sock).env("SPRAG_LOG", level);
        Self::from_command(cmd)
    }

    /// A server whose PARENT advertises `ancestor`, and whose own environment holds `own` (or, for
    /// `None`, nothing at all).
    ///
    /// The intermediate is `/bin/sh`, which forks its last command here rather than `exec`ing into it
    /// — measured, and asserted by [`Self::assert_forked`] so a shell that behaves otherwise fails
    /// this file instead of silently weakening it. In the `None` form the shell `unset`s the variable
    /// before running the server: the child's own environment loses it while the shell's
    /// `/proc/<pid>/environ` keeps advertising it, because that file is the EXEC-time snapshot. That
    /// is not a trick played on the subject — it is the production case, where a pane shell was
    /// exec'd with the daemon's socket in its environment.
    fn spawn_behind_ancestor(own: Option<&Path>, ancestor: &Path) -> Self {
        let script = match own {
            // `$0` is the server binary, `$1` the socket it is given: passed as arguments so no path
            // is ever pasted into a shell word.
            //
            // ⚠ THE TRAILING `exit $?` IS WHAT MAKES THE SHELL FORK, and it is the whole premise:
            // this fixture needs the shell to SURVIVE as an ancestor carrying the variable. A POSIX
            // shell may exec straight into the LAST command of a `-c` script instead of forking, and
            // whether it does is the shell's own choice — measured: Linux's `dash` forks here and
            // macOS's `sh` (bash) execs, so `assert_forked` fired *"it exec'd into the server"* on
            // one runner and not the other. Giving the script something to do afterwards removes
            // the choice.
            Some(_) => format!("{SOCK_ENV}=\"$1\" \"$0\"; exit $?"),
            None => format!("unset {SOCK_ENV}; \"$0\"; exit $?"),
        };
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .arg(env!("CARGO_BIN_EXE_sprag-mcp"))
            .arg(own.unwrap_or(Path::new("")))
            .env(SOCK_ENV, ancestor);
        let server = Self::from_command(cmd);
        server.assert_forked(ancestor);
        server
    }

    /// A server with NO reachable ancestor: `setsid --fork` re-parents it away from this test
    /// process, so the walk leaves the suite's own chain entirely.
    ///
    /// This is the only way the "not inside a sprag terminal" answer is provable from a suite that
    /// might itself be running inside one. Clearing the variable is not enough — the walk would climb
    /// past this test binary into the author's shell and find their live daemon. Re-parenting lands
    /// the server under `init` or the user's session sub-reaper, neither of which can carry a
    /// per-instance socket: they were started at boot and at login, before any daemon existed.
    ///
    /// The shim is reaped BEFORE any request goes out, so the re-parenting has already happened when
    /// the resolve runs. Without that wait the shim would still be the parent for a moment, and the
    /// walk would climb through this test binary — which is exactly the data-dependent flake this
    /// construction exists to remove.
    fn spawn_orphaned() -> Self {
        // ⚠ `sh` AND NOT `setsid`: **macOS has no `setsid` binary at all**, so this spawn failed
        // there with `No such file or directory` and took three tests with it — the first macOS run
        // of this suite is what said so. A POSIX shell backgrounding a command does the same job:
        // the shell forks, exits at once, and its child is re-parented to `init` or the session
        // sub-reaper. ONE path on both platforms, so the runner that has `setsid` exercises the code
        // the runner without it will run.
        //
        // ⚠ The fd dance is the part that is easy to get wrong. POSIX: *"if job control is disabled,
        // the standard input for an asynchronous list, before any explicit redirections, shall be
        // assigned to /dev/null."* A server whose stdin is `/dev/null` reads EOF and exits, which
        // would look exactly like the crash this test is trying to distinguish from an answer. So
        // the real stdin is saved to fd 3 BEFORE the `&`, handed back explicitly, and closed behind
        // it. Only stdin needs this; stdout and stderr are inherited as they are.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(r#"exec 3<&0; "$0" <&3 3<&- &"#)
            .arg(env!("CARGO_BIN_EXE_sprag-mcp"))
            .env_remove(SOCK_ENV);
        let mut server = Self::from_command(cmd);
        let mut shim = server.child.take().expect("the forking shim is a child");
        let status = shim.wait().expect("wait for the forking shim to exit");
        assert!(
            status.success(),
            "the shim exited {status}; the server was never orphaned"
        );
        server
    }

    /// Spawn `cmd` with its three streams piped and its readers running.
    ///
    /// The direct child is always held here; a caller whose child is a shim about to be reaped takes
    /// it out itself (see [`Self::spawn_orphaned`]), so `Drop` never reports having killed a process
    /// that had already exited.
    fn from_command(mut cmd: Command) -> Self {
        // ⚠⚠⚠⚠ **THE PANE THIS SUITE'S RUNNER IS ITSELF IN MUST NOT LEAK INTO THE SERVER IT
        // SPAWNS** — register item 226, which named ONE gate and had three. Run from a shell inside
        // a sprag pane, `sprag-mcp` inherited the RUNNER's `SPRAG_PANE` and answered *"no pane 49
        // on this host"* where `open_pane_refuses_when_the_server_is_not_inside_a_pane` demanded the
        // refusal of a server that is in NO pane. **The debt-repayment loop's own agent runs in a
        // pane**, so this is a red it meets every time and never causes.
        //
        // ⚠⚠ ONLY WHEN THE CALLER DID NOT ASK FOR ONE. `Command::get_envs` reports the explicit
        // overrides, so `in_pane` still gets exactly the pane it named — the harness stops leaking
        // and every pane below stays a stated intention rather than an inheritance.
        let asked_for_a_pane = cmd
            .get_envs()
            .any(|(key, value)| key == std::ffi::OsStr::new(PANE_ENV_VAR) && value.is_some());
        if !asked_for_a_pane {
            cmd.env_remove(PANE_ENV_VAR);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("spawn {cmd:?}: {error}"));
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return; // the test finished; stop reading.
                }
            }
        });
        // Line by line rather than `read_to_string`: that call returns at EOF, so the sink would stay
        // empty for the whole of a run and every mid-run read of it would report no diagnostics at
        // all. Measured, not guessed — the first version of this harness did exactly that, and the
        // "the logs went somewhere" control below is what caught it.
        let collected = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&collected);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { return };
                let Ok(mut text) = sink.lock() else { return };
                text.push_str(&line);
                text.push('\n');
            }
        });

        Self {
            child: Some(child),
            stdin: Some(stdin),
            lines,
            stderr: collected,
            next_id: 1,
        }
    }

    /// Assert the intermediate shell really forked, and really advertises `ancestor`.
    ///
    /// Both halves of [`Self::spawn_behind_ancestor`]'s premise, made into assertions. `children`
    /// is polled because the fork is a race with this call, not because it is uncertain.
    fn assert_forked(&self, ancestor: &Path) {
        let shell = self
            .child
            .as_ref()
            .expect("the shell is still a child")
            .id();
        // ⚠ `ps` AND NOT `/proc/<pid>/task/<pid>/children`: that file does not exist off Linux, and
        // reading it took three tests down on the first macOS run of this suite. `ps -A -o
        // pid=,ppid=` is POSIX and answers the same question — does anything call this shell its
        // parent — on both runners, so the platform that has `/proc` exercises the code the platform
        // without it will run.
        let has_child = || {
            let listed = Command::new("ps")
                .args(["-A", "-o", "pid=,ppid="])
                .output()
                .expect("ps -A: the process table is how this test sees a fork");
            String::from_utf8_lossy(&listed.stdout)
                .lines()
                .filter_map(|row| {
                    let mut cols = row.split_whitespace();
                    let _pid = cols.next()?;
                    cols.next()?.parse::<u32>().ok()
                })
                .any(|ppid| ppid == shell)
        };
        let deadline = Instant::now() + DEADLINE;
        loop {
            if has_child() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the intermediate shell never forked: it exec'd into the server, so no ancestor \
                 carries {SOCK_ENV} and this test would prove less than it claims"
            );
            std::thread::sleep(POLL);
        }
        // Through the same portable door the product's own ancestor walk uses now, for the same
        // reason: `/proc/<pid>/environ` is not there on every platform this suite runs on.
        let environ = sprag_terminal::procfs::environ(shell)
            .expect("read the intermediate shell's exec environment");
        let wanted = format!("{SOCK_ENV}={}", ancestor.display());
        assert!(
            environ
                .split(|&b| b == 0)
                .filter_map(|record| std::str::from_utf8(record).ok())
                .any(|record| record == wanted),
            "the intermediate shell must advertise {wanted} for the ancestor walk to have anything \
             to find"
        );
    }

    /// Send one JSON-RPC request and return the whole response message.
    ///
    /// The id is asserted to match, which is what makes every negative in this file work: if a
    /// previous notification, bad line, or id-less request had produced output, that line would
    /// arrive here first and fail on the id rather than being silently consumed.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write_line(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
        );
        let response = self.next_message();
        assert_eq!(
            response["jsonrpc"], "2.0",
            "every response is JSON-RPC 2.0: {response}"
        );
        assert_eq!(
            response["id"],
            json!(id),
            "the response echoes the request id — an unsolicited line before it would land here: \
             {response}"
        );
        response
    }

    /// Send a notification (no `id`), which must never be answered.
    fn notify(&mut self, method: &str) {
        self.write_line(&json!({ "jsonrpc": "2.0", "method": method }).to_string());
    }

    /// Send an arbitrary line, for the malformed-input claim.
    fn write_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("the server's stdin is open");
        writeln!(stdin, "{line}").expect("write to the server's stdin");
        stdin.flush().expect("flush the server's stdin");
    }

    /// The next line the server wrote, parsed. A line that never comes is a failure, not a hang.
    fn next_message(&mut self) -> Value {
        match self.lines.recv_timeout(DEADLINE) {
            Ok(line) => serde_json::from_str(&line).unwrap_or_else(|error| {
                panic!("the server wrote a non-JSON line {line:?}: {error}")
            }),
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "the server wrote nothing within {DEADLINE:?}\nstderr so far:\n{}",
                    self.stderr()
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the server closed its stdout\nstderr:\n{}", self.stderr())
            }
        }
    }

    /// The `instructions` primer this server hands an agent at `initialize` — what an agent READS
    /// before it calls anything, and therefore a claim under test like any other.
    ///
    /// Asked of the LIVE server rather than of the function that builds it: a test that called the
    /// builder would be checking the string this binary would send, not the one it did.
    fn primer(&mut self) -> String {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "primer-probe", "version": "0" }
            }),
        )["result"]["instructions"]
            .as_str()
            .expect("the primer is a string")
            .to_owned()
    }

    /// Call a tool WITHOUT judging the outcome — for a caller measuring which tools succeed.
    fn call_tool_raw(&mut self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({ "name": name, "arguments": args }))
    }

    /// Call a tool that is expected to succeed, returning its text content.
    fn call_tool(&mut self, name: &str, args: Value) -> String {
        let response = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let result = &response["result"];
        assert_ne!(
            result["isError"],
            json!(true),
            "{name} failed: {}",
            tool_text(result)
        );
        tool_text(result)
    }

    /// Call a tool that is expected to FAIL, returning the error text.
    ///
    /// A tool-level failure is `isError` content, not a JSON-RPC error — the MCP distinction between
    /// "your request was malformed" and "I understood you and the answer is bad news". Asserting the
    /// shape here is what keeps a business error from being promoted into a protocol one.
    fn call_tool_error(&mut self, name: &str, args: Value) -> String {
        let response = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let result = &response["result"];
        assert!(
            response.get("error").is_none(),
            "a tool-level failure stays out of the JSON-RPC error channel: {response}"
        );
        assert_eq!(
            result["isError"],
            json!(true),
            "{name} was expected to fail: {}",
            tool_text(result)
        );
        tool_text(result)
    }

    /// Poll `tool` until its text contains `needle`, returning that text.
    ///
    /// Waits on the CONDITION the assertion reads, and drives the tool under test while doing it.
    fn wait_for_tool(&mut self, name: &str, args: Value, needle: &str) -> String {
        self.wait_for_tool_count(name, args, needle, 1)
    }

    /// Poll `tool` until `needle` appears at least `want` times.
    ///
    /// The count matters where a pane runs `cat`: one occurrence is the pane's own echo of the
    /// keystrokes, which proves only that the bytes reached that PTY. The SECOND is `cat` writing the
    /// line back, which is the round trip — the child process received a completed line. Waiting on
    /// the count rather than asserting it after a first sighting is what keeps the two from racing.
    fn wait_for_tool_count(
        &mut self,
        name: &str,
        args: Value,
        needle: &str,
        want: usize,
    ) -> String {
        let deadline = Instant::now() + DEADLINE;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.call_tool(name, args.clone());
            if last.matches(needle).count() >= want {
                return last;
            }
            std::thread::sleep(POLL);
        }
        panic!("{name} never reported {needle:?} {want} time(s); last answer was:\n{last}")
    }

    /// Everything the server has written to stderr so far.
    fn stderr(&self) -> String {
        self.stderr.lock().expect("stderr sink").clone()
    }

    /// Poll stderr until it contains `needle`.
    ///
    /// A poll rather than a read: the diagnostic and the protocol reply travel on two pipes drained
    /// by two threads, so "the response arrived" says nothing about whether the log line has been
    /// collected yet. Asserting on one read of the sink would be a race that passes on a quiet machine.
    fn wait_for_stderr(&self, needle: &str) -> String {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let text = self.stderr();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "the server never logged {needle:?} to stderr; it wrote:\n{text}"
            );
            std::thread::sleep(POLL);
        }
    }
}

/// The text of a `tools/call` result's first content block.
/// Every whitespace character removed — the only sound reading of a pane's RENDERED text for a
/// needle that may be WRAPPED.
///
/// ⚠⚠ **A WORD ON A NARROW PANE IS NOT A WORD IN ITS TEXT.** Measured in the TUI's pty suite at 4x
/// oversubscription: a pane split off an 80-column terminal held `"coin@host:~$ elsewhe\nre"`, so
/// `contains("elsewhere")` was FALSE about a screen a person reads the word on. For a positive check
/// that is a nuisance; for a **NEGATIVE** one it is a VACUITY — `!contains(w)` passes for a pane
/// showing exactly the `w` the assertion forbids, and passes silently. A leak lands wherever the
/// cursor happens to be, which is the one thing these assertions cannot assume.
///
/// Folding can only make a negative claim STRICTER (it may join two words the screen showed apart),
/// never weaker — so it is the safe direction. Apply it to the NEEDLE too when the needle has a
/// space in it.
fn fold(text: &str) -> String {
    text.chars()
        .filter(|glyph| !glyph.is_whitespace())
        .collect()
}

fn tool_text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result carries text content: {result}"))
        .to_owned()
}

// ----- the protocol loop -----

/// The handshake, the roster, and the four ways a client's input can be something other than a
/// well-formed request — all through the real process, which is where `serve` and `dispatch` live.
///
/// Each negative is asserted by ORDERING rather than by waiting out a quiet window: the request that
/// follows it must be answered by the FIRST line the server writes. An extra line — a response to a
/// notification, an answer to an id-less request, an error for a blank line — would arrive first and
/// fail the id check inside [`McpServer::request`].
///
/// REVERT-PROOF (all three measured red): make the `id.is_none()` arm WRITE a message and the id
/// check fails on the next request; propagate the JSON parse error out of `serve` instead of dropping
/// the line and the survival assertion fails; answer an unknown method with `respond(.., json!({}))`
/// and the -32601 assertion fails.
///
/// Deleting the `id.is_none()` arm outright does NOT go red, and the difference is worth naming: an
/// unmatched notification falls through to `respond_error`, which returns early precisely because
/// there is no id to answer. The arm is what makes the drop deliberate and logged; the SILENCE is
/// held one layer down as well.
#[test]
fn the_shipped_server_speaks_the_protocol_and_survives_bad_input() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn(&sock);

    // The handshake, with a version the client made up: it is echoed rather than corrected, which is
    // how a client newer than this server still completes an initialize.
    let response = server.request(
        "initialize",
        json!({ "protocolVersion": "2030-01-01", "capabilities": {} }),
    );
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2030-01-01");
    assert_eq!(result["serverInfo"]["name"], "sprag-mcp");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "a tools-only server advertises the tools capability: {result}"
    );
    assert!(
        result["instructions"]
            .as_str()
            .expect("the primer is a string")
            .contains("sibling"),
        "the primer tells the agent what it is looking at: {result}"
    );

    // The notification every MCP client sends next. It must produce NO line.
    server.notify("notifications/initialized");
    // ...proved by the next response being the first line to arrive.
    assert_eq!(
        server.request("ping", json!({}))["result"],
        json!({}),
        "ping answers with an empty result"
    );

    // The roster, off the real process rather than off the function that builds it.
    let listed = server.request("tools/list", json!({}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool has a name"))
        .collect();
    for wanted in [
        "list_panes",
        "read_pane",
        "read_last_command",
        "read_pane_links",
        "read_pane_images",
        "find_in_pane",
        "regex_in_pane",
        "agent_state",
        "agent_explain",
        "wait_for_change",
        "write_pane",
        "send_keys",
    ] {
        assert!(names.contains(&wanted), "{wanted} is advertised: {names:?}");
    }

    // A line that is not JSON at all is dropped, and the server keeps serving — the property that
    // decides whether one stray byte on the pipe ends the session or is shrugged off.
    server.write_line("this is not json");
    server.write_line("");
    assert_eq!(
        server.request("ping", json!({}))["result"],
        json!({}),
        "the server survived a malformed line and a blank one"
    );

    // A request with no id cannot be answered, and must not be answered.
    server.write_line(&json!({ "jsonrpc": "2.0", "method": "tools/list" }).to_string());
    assert_eq!(server.request("ping", json!({}))["result"], json!({}));

    // An unknown METHOD is a JSON-RPC protocol error, unlike an unknown TOOL (which is `isError`
    // content). The two are different failures and this is where the difference is visible.
    let unknown = server.request("no/such/method", json!({}));
    assert_eq!(
        unknown["error"]["code"], -32601,
        "method not found: {unknown}"
    );
    assert!(
        unknown["result"].is_null(),
        "an error response carries no result: {unknown}"
    );

    // And an unknown TOOL is the other kind.
    let text = server.call_tool_error("no_such_tool", json!({}));
    assert!(
        text.contains("unknown tool"),
        "an unknown tool names itself: {text}"
    );
}

/// stdout carries ONLY protocol JSON, even with logging turned all the way up.
///
/// The crate's rule is that stdout is the wire and every diagnostic goes to stderr. It is a property
/// of the process — no in-process test can hold it — and its failure mode is total: one log line
/// interleaved into stdout desynchronises every client, which then fails on the NEXT message rather
/// than on the bad one.
///
/// `trace` is used because it is the loudest setting the binary offers, so a subscriber wired to the
/// wrong writer has the most opportunity to prove it.
///
/// REVERT-PROOF: point `init_tracing`'s writer at `io::stdout` and the JSON assertion fails on the
/// first log line.
#[test]
fn stdout_carries_only_protocol_json_when_logging_is_loud() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_logging(&sock, "trace");

    // Each of these is parsed as JSON by `request`/`call_tool`, so a log line landing on stdout
    // fails inside the harness with the offending line quoted.
    server.request("initialize", json!({ "protocolVersion": "2025-06-18" }));
    server.request("tools/list", json!({}));
    // A notification is the noisiest thing a client can send at a server that logs one per drop, and
    // it is answered with nothing — so anything that appears on stdout because of it is a log line.
    server.notify("notifications/initialized");
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains("pane 1:"),
        "the pane list still answers with logging on: {listed}"
    );

    // ...and the logs did go somewhere. This is the CONTROL, and it is not optional: a subscriber
    // silenced altogether would satisfy every assertion above, and "stdout stayed clean" would be
    // true for the uninteresting reason.
    server.wait_for_stderr("sprag-mcp starting");
    // The dropped notification is logged at DEBUG, which the default `warn` filter would discard —
    // so its presence proves `SPRAG_LOG` was read and this run really is the loud one.
    let logged = server.wait_for_stderr("notification ignored");
    assert!(
        !logged.is_empty(),
        "the level filter honoured SPRAG_LOG: {logged:?}"
    );
}

// ----- the bridge to a live daemon -----

/// **The strongest claim in this file**: a 1-based pane number reaches the pane it names, and no
/// other.
///
/// Two panes, distinct text typed into each through `write_pane`, each read back through
/// `read_pane`. For this to pass, the tool argument `2` has to become a host id that is not `2`
/// (the second pane's id here is 1), through one list query, and land in that pane's PTY alone. The
/// panes run `cat`, so the text coming back is a round trip rather than a local echo of the request.
///
/// Crosstalk is asserted in BOTH directions, because a mapping that is off by one in a single place
/// would still put something in both panes.
///
/// REVERT-PROOF: use the 1-based number as the host id in `pane_id_for` and every read fails; drop
/// the `enter` key from `write_pane` and `cat` never echoes, so the wait times out.
#[test]
fn a_pane_number_reaches_that_pane_and_no_other() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let second = add_pane(&sock, &["cat"]);
    let mut server = McpServer::spawn(&sock);

    // The list, once the spawned pane is in it. The geometry is this daemon's, which is what the
    // resolve-order tests read to tell two daemons apart.
    let listed = server.wait_for_tool("list_panes", json!({}), "pane 2:");
    assert!(
        listed.contains("2 pane(s)")
            && listed.contains(&format!("{}x{}", BOOT_PANE.0, BOOT_PANE.1)),
        "the list reports both panes and their size: {listed}"
    );
    assert!(
        listed.contains(&format!("pane 2: id={second}")),
        "pane 2 is the pane the daemon gave id {second} — the number is a position, not an id: \
         {listed}"
    );

    // Distinct text into each pane...
    let wrote = server.call_tool("write_pane", json!({ "pane": 1, "text": "ALPHA-ONE" }));
    assert!(
        wrote.contains("pressed Enter"),
        "the default is to run what was typed: {wrote}"
    );
    server.call_tool("write_pane", json!({ "pane": 2, "text": "BRAVO-TWO" }));

    // ...and each pane holds its own and not the other's. TWICE is the load-bearing count: the first
    // occurrence is the pane's own echo of the keystrokes and proves only that the bytes reached that
    // PTY, while the second is `cat` writing the line back — so the CHILD PROCESS received a
    // completed line. That is what the Enter buys, and one occurrence is what dropping it leaves.
    let first_text = server.wait_for_tool_count("read_pane", json!({ "pane": 1 }), "ALPHA-ONE", 2);
    assert!(
        !fold(&first_text).contains("BRAVO-TWO"),
        "pane 1 did not receive pane 2's text: {first_text}"
    );
    let second_text = server.wait_for_tool_count("read_pane", json!({ "pane": 2 }), "BRAVO-TWO", 2);
    assert!(
        !fold(&second_text).contains("ALPHA-ONE"),
        "pane 2 did not receive pane 1's text: {second_text}"
    );

    // `send_keys` is the other input path — named keys rather than text — and it reaches the same
    // pane. `cat` echoes the characters back once the Enter completes its line.
    server.call_tool(
        "send_keys",
        json!({ "pane": 2, "keys": ["Z", "Q", "9", "Enter"] }),
    );
    server.wait_for_tool("read_pane", json!({ "pane": 2 }), "ZQ9");

    // `tail_lines` trims from the end, so the earliest line of a pane that has scrolled is dropped.
    let tail = server.call_tool("read_pane", json!({ "pane": 2, "tail_lines": 1 }));
    assert_eq!(tail.lines().count(), 1, "one line was asked for: {tail:?}");

    // A pane number nobody has is an error that says how many there are — the answer that lets an
    // agent correct itself instead of retrying.
    let missing = server.call_tool_error("read_pane", json!({ "pane": 99 }));
    assert!(
        missing.contains("no pane 99") && missing.contains("2 pane(s)"),
        "the refusal names the miss and the count: {missing}"
    );
}

/// The read tools whose answers come out of a query slot rather than out of the pane list, against a
/// real daemon — which is the only place the slot SPELLINGS are checked.
///
/// `find_in_pane` and `regex_in_pane` read two different slot families built by two different
/// functions; the links, images and last-command tools each read a slot of their own. A unit test
/// over hand-written JSON would agree with itself about every one of those names. Here the daemon
/// has to recognise them.
///
/// The empty answers are as load-bearing as the full ones: "this pane shows no links" has to be
/// distinguishable from "the links slot does not exist", and only a live daemon can tell them apart.
///
/// REVERT-PROOF: misspell any of `find_slot_for`, `regex_slot_for`, `LINKS_SLOT` or
/// `LAST_COMMAND_SLOT` and that tool's assertion fails with the daemon's own refusal.
#[test]
fn the_query_slots_every_read_tool_names_are_the_daemons_own() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn(&sock);
    server.call_tool("write_pane", json!({ "pane": 1, "text": "NEEDLE-HERE" }));
    server.wait_for_tool("read_pane", json!({ "pane": 1 }), "NEEDLE-HERE");

    // A literal search: the matching lines, numbered from the oldest retained line.
    let found = server.call_tool(
        "find_in_pane",
        json!({ "pane": 1, "needle": "NEEDLE-HERE" }),
    );
    assert!(
        found.contains("NEEDLE-HERE") && found.contains(':'),
        "matches come back as LINE: text: {found}"
    );
    // ...and a miss is an answer, not an error: the agent asked a well-formed question.
    let miss = server.call_tool(
        "find_in_pane",
        json!({ "pane": 1, "needle": "NOT-IN-THIS-PANE" }),
    );
    assert!(
        miss.contains("no matches"),
        "a miss says so plainly: {miss}"
    );

    // The same text as a PATTERN — a different slot family, and the shape rather than the string.
    let matched = server.call_tool(
        "regex_in_pane",
        json!({ "pane": 1, "pattern": "NEE.LE-HERE" }),
    );
    assert!(
        matched.contains("NEEDLE-HERE"),
        "the regex matched through the daemon: {matched}"
    );
    // A refused pattern is an ERROR and not an empty result: "your pattern is wrong" and "nothing
    // matched" are different answers, and an agent that cannot tell them apart retries forever.
    let refused = server.call_tool_error("regex_in_pane", json!({ "pane": 1, "pattern": "[" }));
    assert!(
        refused.contains("invalid pattern"),
        "a bad pattern is reported with its reason: {refused}"
    );

    // The slots whose answer here is EMPTY. Each has to reach a real slot to answer at all.
    let links = server.call_tool("read_pane_links", json!({ "pane": 1 }));
    assert!(
        links.contains("no OSC-8 hyperlinks"),
        "an empty links slot is an answer: {links}"
    );
    let images = server.call_tool("read_pane_images", json!({ "pane": 1 }));
    assert!(
        images.contains("no inline images"),
        "an empty images list is an answer: {images}"
    );
    // `cat` emits no OSC 133, so the last-command slot is null — and the tool says what to do about
    // it rather than reporting an empty command.
    let last = server.call_tool("read_last_command", json!({ "pane": 1 }));
    assert!(
        last.contains("use read_pane"),
        "a pane with no shell integration is told where to look instead: {last}"
    );
}

/// The other half of the three list-shaped read tools: a pane that actually HAS a link, an image and
/// a finished command.
///
/// `the_query_slots_every_read_tool_names_are_the_daemons_own` proves each slot exists and that an
/// empty answer is an answer. It cannot prove the tools RENDER anything, because a pane running `cat`
/// emits no OSC 8, no graphics and no OSC 133 — so every one of those three answers is the empty
/// branch, and a tool that formatted its non-empty case wrongly would pass. This test is the reason
/// the pair is worth two tests rather than one: the child here emits all three, in the forms sprag's
/// own emulator is already proved to parse, and each tool has to turn a real slot value into the text
/// an agent reads.
///
/// The pane is deliberately taller than the others: the artifacts have to still be ON SCREEN when the
/// tools ask, and a six-row pane scrolls the link off while the command cycle is still printing.
///
/// REVERT-PROOF (all four measured red): render a link run without its `uri` and the destination
/// assertion fails; report an image's `id` in place of its pixel size and the size assertion fails;
/// have `read_last_command` print the raw slot instead of slicing `{command, output, exit_status}`
/// and the exit-status assertion fails; drop the exit code from the pane list's shell line and the
/// last assertion fails.
#[test]
fn the_list_read_tools_render_what_a_pane_actually_shows() {
    // One OSC 8 hyperlink with an id, one 2x2 Kitty RGBA image (16 payload bytes, base64), and one
    // full OSC 133 cycle: prompt (A), the typed command, input end (B), output start (C), one output
    // line, command end (D) with exit 0. Then `cat`, so the pane stays alive and quiet. Every form is
    // copied from a fixture this workspace already drives through the same emulator.
    let (_daemon, sock) = spawn_daemon(
        &[
            "sh",
            "-c",
            "printf '\\033]8;id=x1;https://example.com/manual\\007the manual\\033]8;;\\007\\r\\n'; \
             printf '\\033_Ga=T,f=32,s=2,v=2,i=7;AAECAwQFBgcICQoLDA0ODw==\\033\\\\'; \
             printf '\\033]133;A\\007$ echo hi\\033]133;B\\007\\r\\n\
             \\033]133;C\\007hi\\r\\n\\033]133;D;0\\007'; cat",
        ],
        TALL_PANE,
    );
    let mut server = McpServer::spawn(&sock);

    // The link's DESTINATION as data — the answer tmux cannot give at all, since `capture-pane`
    // flattens OSC 8 to its display text and drops the URI.
    // The wait is on the link EXISTING; what is rendered about it belongs in the assertion, so a
    // regression in the rendering fails at once instead of timing the wait out.
    let links = server.wait_for_tool("read_pane_links", json!({ "pane": 1 }), "1 link(s)");
    assert!(
        links.contains("1 link(s)")
            && links.contains("\"the manual\" -> https://example.com/manual")
            && links.contains("id=x1"),
        "the displayed text, the URI it points at, and the link id: {links}"
    );

    // An image cannot be read, but its PRESENCE and geometry can — which is what the tool claims.
    let images = server.wait_for_tool("read_pane_images", json!({ "pane": 1 }), "image #");
    assert!(
        images.contains("1 image(s)") && images.contains("image #7: 2x2 px at cell"),
        "the id and the pixel size the child transmitted: {images}"
    );

    // The command-scoped read: sliced at the shell's marks, not the whole screen.
    let last = server.wait_for_tool("read_last_command", json!({ "pane": 1 }), "echo hi");
    assert!(
        last.contains("$ echo hi") && last.contains("[exit 0]") && last.contains("--- output ---"),
        "the command line and how it ended: {last}"
    );
    assert_eq!(
        last.lines().last(),
        Some("hi"),
        "and the output is the command's alone, not the screen's: {last}"
    );

    // The same cycle also drives the pane list's shell-integration line, which no other test here
    // reaches: a resting pane has none, so it is only observable on a pane that emitted marks.
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains("shell: at a prompt (last command exit 0)"),
        "the pane list reports the shell as idle and how the last command ended: {listed}"
    );
}

/// The two `agent_*` tools against a real daemon — the composition R257 shipped unwitnessed.
///
/// The verdict is the DAEMON's: the detector runs daemon-side (H3's D2), so this proves the bridge
/// carries a real evaluation rather than that a fixture round-trips. Four field names (`state`,
/// `name`, `rule`, `seq`) are read out of a key (`agent`) another crate writes, and the pane that has
/// none must be reported as having none — the one collapse D3 forbids, and the one a unit test with a
/// hand-written entry cannot fail to get right.
///
/// The pane is agent-SHAPED rather than a credentialed agent: a gate that needs an API key is a gate
/// nobody runs. `blocked` is the state used because it rests on evidence PRESENT on the screen (a
/// bottom-anchored choice list), so it publishes on sight and this test needs no settle window.
///
/// REVERT-PROOF (all three measured red): drop the explicit no-agent line from `agent_state` and the
/// shell pane's assertion fails; stop reading `rule` off the wire and the rule assertion fails; have
/// `agent_explain` omit the remedy and the config.toml assertion fails.
///
/// One mutation that a live gate CANNOT see, measured GREEN and recorded rather than assumed:
/// defaulting the parsed `state` to `idle`. A real daemon omits the `agent` key entirely for a pane no
/// manifest claims, so the parse returns `None` before a state is ever read, and the defensive default
/// is unreachable from here. The population that would expose it — a daemon sending `agent: null` or
/// an object with no state — does not exist on this wire, which is why that guard's home is the unit
/// test over hand-written JSON and why this file must not claim it. An integration harness does not
/// subsume the tests beneath it.
#[test]
fn the_agent_tools_report_the_daemons_own_verdict() {
    // `claude`'s resting glyph in the title (OSC 2) is the fingerprint; the numbered choice list in
    // the bottom rows is what the `dialog-choice-list` rule reads. Then `cat`, so the pane goes quiet.
    let (_daemon, sock) = spawn_daemon(
        &[
            "sh",
            "-c",
            "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\
             \\342\\235\\257 1. Yes\\n  2. No\\n'; cat",
        ],
        BOOT_PANE,
    );
    let shell = add_pane(&sock, &["cat"]);
    let mut server = McpServer::spawn(&sock);

    // The pane list carries the verdict on the claimed pane...
    let listed = server.wait_for_tool("list_panes", json!({}), "agent: state=blocked");
    assert!(
        listed.contains("agent: state=blocked name=claude rule=dialog-choice-list seq="),
        "the verdict surfaces field for field: {listed}"
    );
    // ...and says nothing about an agent on the pane running a shell. A pane per `agent:` line is
    // the additive rule; two lines would mean the shell had been described as an agent at rest.
    assert_eq!(
        listed.matches("agent: state=").count(),
        1,
        "only the claimed pane contributes an agent line: {listed}"
    );

    // `agent_state` with no argument answers about the SET, and the pane with no verdict is reported
    // EXPLICITLY — "this is not an agent" and "this agent is waiting" are opposite instructions.
    let all = server.call_tool("agent_state", json!({}));
    assert!(
        all.contains("pane 1: id=0 state=blocked name=claude rule=dialog-choice-list"),
        "the claimed pane reads as blocked: {all}"
    );
    assert!(
        all.contains(&format!("pane 2: id={shell} no agent"))
            && all.contains("not the same as idle"),
        "the shell pane is answered rather than omitted: {all}"
    );

    // Naming one pane narrows the same reading.
    let one = server.call_tool("agent_state", json!({ "pane": 1 }));
    assert_eq!(
        one.matches("state=").count(),
        1,
        "one pane, one verdict: {one}"
    );
    assert!(
        one.contains("state=blocked"),
        "and it is the right one: {one}"
    );

    // ⚠⚠⚠ **AND IT SAYS WHAT THE PANE IS ASKING** (R367) — the whole point of asking this tool
    // about a blocked pane, driven end to end: a real daemon parsed a real pty's screen, and the
    // question crossed the wire, the mouth and MCP stdio without a caller reading a screen.
    //
    // Until R367 this surface published `blocked` and nothing else, so an agent that saw a sibling
    // stop had to `read_pane` and re-derive a menu this daemon had ALREADY parsed for the run
    // surface — off the same screen, in the same instant.
    assert!(
        one.contains("1. Yes") && one.contains("2. No"),
        "every option a caller could pick has to reach it: {one}"
    );
    let takes_enter = one
        .lines()
        .find(|line| line.contains("a bare Enter takes this one"))
        .unwrap_or_default();
    assert!(
        takes_enter.contains("1. Yes"),
        "WHICH option a bare Enter would take is the difference between confirming a tool call and \
         declining it, and it must be the one the agent's own marker is on: {one}"
    );
    assert!(
        one.contains("answer_pane") && one.contains("Do NOT type the number with send_keys"),
        "⚠⚠⚠ ...and the mouth must name the tool that ANSWERS IT HERE. Until R369 this sentence \
         pointed at a run argument — a consent declared before a loop — so the surface that \
         publishes the question named the one act its reader could not perform, and the act it \
         could perform was the forbidden one: {one}"
    );

    // A pane nobody has is an ERROR even though the whole-set form takes no argument: a caller who
    // named a pane asked about that pane.
    let missing = server.call_tool_error("agent_state", json!({ "pane": 99 }));
    assert!(
        missing.contains("no pane 99"),
        "the refusal names the miss: {missing}"
    );

    // `agent_explain` reports the rule out of the verdict the detector already produced — so it
    // cannot disagree with `agent_state` — and names the remedy, which is what makes a
    // mis-detection fixable without a release.
    let why = server.call_tool("agent_explain", json!({ "pane": 1 }));
    assert!(
        why.contains("`dialog-choice-list`") && why.contains("config.toml"),
        "the explanation names the rule that fired and what to edit: {why}"
    );
    assert!(
        why.contains("`claude`"),
        "and whose manifest claimed the pane: {why}"
    );
    // ...and the MENU IT READ, which is the sharpest evidence the verdict is right — a `blocked`
    // that names a rule and shows no question is exactly the reading a caller cannot check (R367).
    assert!(
        why.contains("1. Yes") && why.contains("2. No"),
        "explaining a blocked verdict means showing what the detector read as the menu: {why}"
    );

    // The diagnosable answer for "why does my agent pane show nothing": no manifest claims it, so no
    // rule was even consulted.
    let quiet = server.call_tool("agent_explain", json!({ "pane": 2 }));
    assert!(
        quiet.contains("no agent manifest claims this pane") && quiet.contains("[[agent]]"),
        "a pane with no agent is explained as exactly that: {quiet}"
    );
}

/// ⚠⚠⚠⚠⚠ **AN AGENT IS TOLD WHETHER THE REPORTER IT IS BELIEVING CAN STILL SPEAK, AND WHETHER IT
/// IS THIS DAEMON'S CODE** — register item 474, the agent-facing half of the pair a person has had
/// at a command line since items 344 and 473.
///
/// # What was wrong, and why every other gate here stayed green through it
///
/// A REPORTED verdict OUTRANKS the screen and never expires, so two things can make it a lie that
/// no reader of this surface could see. The LOUD one: the reporter has stopped being able to
/// deliver, and the last thing it managed to say stands for ever — measured at an hour of `working`
/// against a pane whose screen had said `MILESTONE REACHED` the whole time. The QUIET one, which the
/// register calls worse: the reporter speaks perfectly, for code this daemon has never run, which is
/// the ORDINARY state after a `cargo build` replaces the hook binary under a live daemon.
///
/// `sprag agent <pane>` prints both. **`agent_state` and `agent_explain` printed neither**, and the
/// reader that acts on those tools is a supervising LOOP — the one party with no screen to glance at
/// and no second surface to consult. Every assertion in this file about an agent verdict was true
/// while both sentences were missing, because a missing caveat is invisible to a test that does not
/// ask for it.
///
/// # Four answers about the build, and each is staged by a different party
///
/// * **IS this image** — the REAL `sprag hook claude`, reporting against the real daemon, so the
///   build in the row is one process's true stamp compared against another's.
/// * **is NOT** — the SAME report, read back through [`sprag_peer::OldDaemon`] answering the
///   handshake with a build no image in this tree can be. Nothing about the reporter changes between
///   this arm and the one above; only the daemon does, which is what makes the sentence attributable
///   to the COMPARISON rather than to the report. It is the fixture `sprag agent`'s own gate uses,
///   in the crate that exists so a stand-in daemon has one meaning in this workspace.
/// * **DID NOT SAY** — a report that OMITS the key, which is the exact wire every hook older than
///   register item 459 sends, and what a person typing `sprag report-agent` still sends. An omission
///   is not a hand-set field: there is no value here to get wrong.
/// * and **MUTE**, which is a different fact about the same reporter and is asserted BESIDE the
///   build sentence rather than instead of it — a reporter can be mute and this daemon's image at
///   once, and a surface that printed only one of the two would have re-created the asymmetry the
///   item is named for.
///
/// ⚠ The mute half is staged by making the hook FAIL for real — a second run of the same binary
/// against a socket nobody serves — rather than by writing its breadcrumb by hand. The file's
/// existence is the message, and a test that wrote the message would be asserting its own spelling.
///
/// ⚠⚠ **A STATE HOME OF ITS OWN**, shared by the hook that writes and the server that reads: that
/// breadcrumb is the one fact on this surface the daemon cannot be asked for, because the condition
/// being reported is that the daemon was unreachable.
#[test]
fn an_agent_is_told_whether_the_reporter_it_believes_is_mute_or_another_build() {
    /// A build no image in this tree can be. Twelve hex digits, the shape `build.rs` stamps, so it
    /// is refused for what it SAYS and not for how it is spelled.
    const NOT_THIS_IMAGE: &str = "0000deadbeef";

    let state = state_home();
    let (_daemon, sock) = spawn_daemon_with(
        &["cat"],
        BOOT_PANE,
        &[("XDG_STATE_HOME", &state.display().to_string())],
    );
    let silent_pane = add_pane(&sock, &["cat"]);

    // THE CONTROL, and it is what makes every word below attributable: before any reporter speaks,
    // a `cat` pane is an agent to no rule at all, so the surface says so and says nothing about a
    // build.
    let mut server = McpServer::spawn_with_state(&sock, &state);
    let before = server.call_tool("agent_state", json!({ "pane": 1 }));
    assert!(
        before.contains("no agent") && !before.contains("build"),
        "nothing on this screen is an agent to any rule yet: {before}"
    );

    // ── The reporter the whole key was written for, running for real. ──
    run_hook(&sock, &state, 0);
    let own = server.wait_for_tool("agent_state", json!({ "pane": 1 }), "source=hook:claude");

    // ── ARM 1: the reporter IS this daemon's image, and BOTH agent tools say so. ──
    for (tool, said) in [
        ("agent_state", own.clone()),
        (
            "agent_explain",
            server.call_tool("agent_explain", json!({ "pane": 1 })),
        ),
    ] {
        assert!(
            said.contains("is the image of the daemon at") && said.contains(sprag_rpc::BUILD),
            "⚠⚠⚠⚠⚠ AN AGENT MUST BE ABLE TO READ THIS. The hook stated its build and the daemon \
             holds both halves, so the mouth a supervising loop reads has to say whether the \
             verdict it is about to act on came from this daemon's own code — {tool} said: {said}",
        );
        assert!(
            said.contains(&sock.display().to_string()),
            "⚠⚠⚠⚠ ...and it must say WHOSE build it compared. This server is a SIBLING of the \
             daemon, not the daemon, so «this daemon» would leave a caller unable to tell which of \
             three images is meant — {tool} said: {said}",
        );
        assert!(
            !said.contains("MUTE"),
            "a reporter that has just delivered is not mute: {tool} said: {said}",
        );
    }

    // ── ARM 2: the SAME report, read back through a daemon built from other code. ──
    let skewed = sprag_peer::OldDaemon::proxying(
        &socket_path(),
        &sock,
        sprag_peer::Missing::answering(&[(sprag_rpc::BUILD_FIELD, json!(NOT_THIS_IMAGE))]),
    );
    let mut through_skew = McpServer::spawn_with_state(skewed.sock(), &state);
    let skew = through_skew.call_tool("agent_state", json!({ "pane": 1 }));
    assert!(
        skew.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠⚠ THIS IS THE WHOLE HAZARD: a verdict that outranks the screen, produced by code this \
         daemon has never run. A rebuild replaces the hook under every live daemon at once, so it \
         is the ORDINARY state after one — and a mouth that stays quiet here leaves a loop acting \
         on a report about another build: {skew}",
    );
    assert!(
        skew.contains(sprag_rpc::BUILD) && skew.contains(NOT_THIS_IMAGE),
        "⚠⚠⚠ and it names BOTH builds — one of them alone tells a reader nothing about which is \
         which: {skew}",
    );
    assert!(
        !skew.contains("is the image of"),
        "a reporter that is not this daemon's image must not also be called one: {skew}",
    );
    let skew_why = through_skew.call_tool("agent_explain", json!({ "pane": 1 }));
    assert!(
        skew_why.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠ BOTH MOUTHS OR NEITHER. `explain` is the tool a caller reaches for when a verdict looks \
         wrong, which is exactly when this is the answer: {skew_why}",
    );
    drop(through_skew);
    drop(skewed);

    // ── ARM 3: a reporter older than the key, which says nothing about its build at all. ──
    mux_invoke(
        &sock,
        REPORT_AGENT_ACTION,
        json!({ "id": silent_pane, "state": "working", "source": "hook:claude" }),
    );
    let unsaid = server.wait_for_tool("agent_state", json!({ "pane": 2 }), "source=hook:claude");
    assert!(
        unsaid.contains("did not say which build it is"),
        "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE, and this is the arm a tidy-looking edit folds \
         into the first. Silence here would convert *nobody knows* into *nothing is wrong* — the \
         exact inversion `AGENT_BUILD_KEY` exists to end: {unsaid}",
    );
    assert!(
        !unsaid.contains("is the image of") && !unsaid.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠ the answers stay separate: a reporter that did not say is neither of the others: \
         {unsaid}",
    );

    // ── ARM 4: the LOUD half, staged by a hook that really cannot deliver. ──
    run_hook(
        Path::new("/nonexistent/sprag-there-is-no-daemon.sock"),
        &state,
        0,
    );
    let mute = server.wait_for_tool("agent_state", json!({ "pane": 1 }), "MUTE");
    assert!(
        mute.contains("could not reach the daemon"),
        "⚠⚠⚠⚠ the reporter's OWN account of why it failed, which is the sentence a hook otherwise \
         writes where nothing reads: {mute}",
    );
    assert!(
        mute.contains("state=working") && mute.contains("read_pane is the better witness"),
        "⚠⚠⚠ the stale verdict is still published — it has to be, a report never expires — and what \
         this adds is the instruction that makes it survivable: {mute}",
    );
    assert!(
        mute.contains("is the image of the daemon at"),
        "⚠⚠⚠⚠⚠ BOTH SENTENCES, NOT ONE. A reporter can be mute and this daemon's image at the same \
         time, and printing one of the two would rebuild the very asymmetry item 474 is the name \
         of: {mute}",
    );
    let mute_why = server.call_tool("agent_explain", json!({ "pane": 1 }));
    assert!(
        mute_why.contains("MUTE") && !mute_why.contains("pre-H3 daemon"),
        "⚠⚠ and `explain` owes a REPORTED verdict this instead of the rule remedy it cannot use: a \
         report fires no rule, so «pre-H3 daemon» was a false diagnosis printed at a pane a live \
         hook had spoken for a second earlier: {mute_why}",
    );
}

/// ⚠⚠⚠⚠⚠ **THE SURFACE AN AGENT READS FIRST QUALIFIES WHAT IT SHOWS** — register item 475, and the
/// residue item 474 was closed with.
///
/// # The gap, and why closing it at `agent_state` was not enough
///
/// 474 gave the two AGENT TOOLS the caveats. `list_panes` is the tool an agent calls before it knows
/// there is an agent to ask about — the doc comment at the top of this binary says so, and every
/// other tool here describes itself in terms of it. It rendered `state=… source=hook:claude seq=…`
/// and qualified none of it, so the FIRST thing an agent learned about a sibling was the one thing
/// it had no way to check: whether that report is live, or the frozen last words of a reporter that
/// can no longer speak, or perfectly current news about a build this daemon has never run.
///
/// A reader who stops at the listing is not a careless reader — stopping there is what a listing is
/// FOR. So the row itself has to say when there is something to go and ask about.
///
/// # ⚠⚠⚠⚠ Why this is a word and `agent_state` is a paragraph
///
/// The parity argument for silence is real: `sprag panes` says none of this either, because a caveat
/// per row buries a twelve-pane listing. What breaks the symmetry is COST — a person reading
/// `sprag panes` is one keystroke from `sprag agent <pane>`, and an agent reading this is one TOOL
/// CALL and one LLM turn from `agent_state`. So the listing pays a word and names the tool that
/// holds the sentence, and this gate holds BOTH halves of that bargain: the marker is there, and the
/// paragraph is not.
///
/// # The same three staged parties 474's gate drives, through the listing
///
/// * the REAL `sprag hook claude` against the real daemon — which must leave the row UNMARKED, so
///   that an unmarked row means *checked, and it agrees* rather than *nothing was checked*;
/// * that same report read back through [`sprag_peer::OldDaemon`] answering the handshake with a
///   build no image in this tree can be — the ordinary state after a `cargo build`;
/// * a report that OMITS the build key, which is what every hook older than item 459 sends;
/// * and MUTE, staged by running the real hook against a socket nobody serves, asserted BESIDE the
///   build answer rather than instead of it.
#[test]
fn an_agent_reading_the_listing_alone_cannot_believe_a_stale_report() {
    /// A build no image in this tree can be, in the shape `build.rs` stamps.
    const NOT_THIS_IMAGE: &str = "0000deadbeef";

    let state = state_home();
    let (_daemon, sock) = spawn_daemon_with(
        &["cat"],
        BOOT_PANE,
        &[("XDG_STATE_HOME", &state.display().to_string())],
    );
    let silent_pane = add_pane(&sock, &["cat"]);

    // THE CONTROL that makes every marker below attributable: two `cat` panes are an agent to no
    // rule at all, so the listing carries no verdict and therefore nothing to qualify.
    let mut server = McpServer::spawn_with_state(&sock, &state);
    let before = server.call_tool("list_panes", json!({}));
    assert!(
        !before.contains('⚠') && !before.contains("agent:"),
        "nothing in this window is an agent to any rule yet, so nothing is qualified: {before}"
    );

    // ── ARM 1: the real reporter, this daemon's own image. SILENCE IS THE ANSWER. ──
    run_hook(&sock, &state, 0);
    let live = server.wait_for_tool("list_panes", json!({}), "source=hook:claude");
    assert!(
        !live.contains('⚠'),
        "⚠⚠⚠⚠⚠ THE MARKER HAS TO BE EARNED OR IT IS NOISE. A reporter that has just delivered, from \
         the code this daemon is running, is the ORDINARY row — and a listing that flagged it would \
         train a reader to skip the flag on the row that matters: {live}"
    );
    assert!(
        !live.contains("REPORTED this state") && !live.contains("read_pane is the better witness"),
        "⚠⚠⚠⚠ ...and the listing must not become the paragraph. A sentence per row is the cost this \
         surface has refused before, and it is why the marker is a word that NAMES agent_state \
         rather than a caveat block twelve panes deep: {live}"
    );

    // ── ARM 2: the SAME report, read back through a daemon built from other code. ──
    let skewed = sprag_peer::OldDaemon::proxying(
        &socket_path(),
        &sock,
        sprag_peer::Missing::answering(&[(sprag_rpc::BUILD_FIELD, json!(NOT_THIS_IMAGE))]),
    );
    let mut through_skew = McpServer::spawn_with_state(skewed.sock(), &state);
    let skew = through_skew.call_tool("list_panes", json!({}));
    assert!(
        skew.contains("⚠ other-build"),
        "⚠⚠⚠⚠⚠ THIS IS THE WHOLE HAZARD, ON THE SURFACE THAT MEETS IT FIRST: a verdict that \
         outranks the screen, produced by code this daemon has never run. Nothing about the \
         reporter changed between this row and the unmarked one above — only the daemon did: {skew}"
    );
    assert!(
        skew.contains("agent_state pane 1 says what to do"),
        "⚠⚠⚠⚠ and a doubt with no address is a doubt a reader has to spend a TURN resolving. The \
         marker names the tool AND the number this listing just taught: {skew}"
    );
    assert!(
        skew.contains("source=hook:claude seq="),
        "⚠⚠ the verdict itself still renders field for field — this qualifies the row, it does not \
         replace it: {skew}"
    );
    drop(through_skew);
    drop(skewed);

    // ── ARM 3: a reporter older than the key, which says nothing about its build at all. ──
    mux_invoke(
        &sock,
        REPORT_AGENT_ACTION,
        json!({ "id": silent_pane, "state": "working", "source": "hook:claude" }),
    );
    let unsaid = server.wait_for_tool("list_panes", json!({}), "⚠ reporter-build-unsaid");
    assert!(
        unsaid.contains("agent_state pane 2 says what to do"),
        "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE, and this is the arm a tidy-looking edit folds \
         into the silent one. Marking it is what makes the UNMARKED row of arm 1 mean something: \
         {unsaid}"
    );
    assert!(
        !unsaid.contains("⚠ other-build"),
        "⚠⚠⚠ the answers stay apart — a reporter that did not say is not a reporter that disagrees: \
         {unsaid}"
    );

    // ── ARM 4: the LOUD half, staged by a hook that really cannot deliver. ──
    run_hook(
        Path::new("/nonexistent/sprag-there-is-no-daemon.sock"),
        &state,
        0,
    );
    let mute = server.wait_for_tool("list_panes", json!({}), "⚠ mute");
    assert!(
        mute.contains("state=working") && mute.contains("⚠ mute — agent_state pane 1"),
        "⚠⚠⚠⚠ the stale verdict is still published — a report never expires — and what the row adds \
         is the one word that stops a scanner acting on it: {mute}"
    );
    assert!(
        !mute.split('\n').any(|row| row.contains("⚠ mute")
            && (row.contains("other-build") || row.contains("build-unsaid"))),
        "⚠⚠⚠⚠⚠ MUTE AND THE BUILD ARE TWO FACTS, AND THIS REPORTER IS MUTE *AND* THIS DAEMON'S \
         IMAGE. Marking a build doubt here would be inventing one: {mute}"
    );
}

/// `agent_explain` warns about a REFUSED `config.toml` before it explains anything — the case where
/// its own remedy sends a reader in a circle.
///
/// The explanation's value is that it names what to edit. When the file it names will not parse, the
/// daemon keeps the last list that worked and says so only to a log, so an agent reading this tool
/// is told to write an `[[agent]]` block that may already be written — and the pane the block was
/// meant to claim reports as claimed by nobody, which reads as a fingerprint problem rather than a
/// syntax one. The caveat is what separates those two readings.
///
/// Live rather than unit because the caveat is a second host call the tool makes on its own: nothing
/// in the arguments asks for it, so a wiring that never made the call would satisfy every unit test
/// of the wording. `manifest_caveat_line`'s own test holds the wording; this holds that a reader gets
/// it at all.
///
/// REVERT-PROOF: drop the `manifest_caveat()` prefix in `tool_agent_explain` and both assertions
/// fail — the daemon is left reporting the refusal to a log nobody reads, which is the state this
/// round ends.
#[test]
fn agent_explain_warns_when_the_daemon_has_refused_the_manifest_file() {
    let dir = std::env::temp_dir().join(format!("sprag-mcp-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    // Valid TOML, invalid MANIFEST — nothing else in the file stops working, which is what makes the
    // failure silent.
    std::fs::write(
        dir.join("sprag").join("config.toml"),
        "[[agent]]\nname = \"claude\"\ndisable = [\"nope\"]\n",
    )
    .expect("write the broken config");

    let (_daemon, sock) = spawn_daemon_with(
        &["cat"],
        BOOT_PANE,
        &[("XDG_CONFIG_HOME", &dir.display().to_string())],
    );
    let mut server = McpServer::spawn(&sock);

    let why = server.call_tool("agent_explain", json!({ "pane": 1 }));
    assert!(
        why.starts_with("NOTE before reading this"),
        "the caveat leads, because it changes how the rest is read: {why}"
    );
    assert!(
        why.contains("nope") && why.contains("no agent manifest claims this pane"),
        "the daemon's own sentence, in front of the reading an unparsed claim produces: {why}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **THE live gate for `wait_for_change`, and R258's lesson is why it exists**: nothing in the
/// arguments asks for a park, a cursor, or an event, so a wiring that made neither host call would
/// satisfy every unit test of the wording. Only driving the real server against a real daemon can
/// tell whether the tool does what its description promises.
///
/// Three properties, and the third is the one a poll loop could never have:
///
/// 1. a change made while the tool is BLOCKED reaches the caller — it does not have to be asked for
///    a second time;
/// 2. the answer names the change and its subject, not a bare "something happened";
/// 3. a quiet terminal is reported as quiet rather than as a failure, so an agent does not read
///    silence as breakage.
#[test]
fn wait_for_change_blocks_and_reports_what_moved() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn(&sock);

    // Establish the cursor at the present, the way any first call does.
    let quiet = server.call_tool("wait_for_change", json!({ "timeout_seconds": 1 }));
    assert!(
        quiet.contains("Nothing changed"),
        "a quiet terminal is an ANSWER, not an error: {quiet}",
    );

    // Now make a change while the tool is parked. The split runs on this thread AFTER the server has
    // been asked to wait, so the tool is genuinely blocked when it happens — a tool that polled once
    // and returned would report nothing.
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    let mover = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
        )
        .expect("spawn a pane while the tool waits");
    });

    let moved = server.call_tool("wait_for_change", json!({ "timeout_seconds": 20 }));
    mover.join().expect("the mover thread finished");

    assert!(
        moved.contains("pane_created"),
        "the change that happened DURING the wait is what comes back: {moved}",
    );
    assert!(
        moved.contains("pane_created: pane 2 (id 1)"),
        "and it names its subject in BOTH vocabularies — the 1-based number every other tool here \
         takes, and the host id a human sees in `sprag panes`: {moved}",
    );

    // R292: the filter narrows the WAKE server-side, and its refusals reach the agent as sentences it
    // can act on. Both are asserted here rather than in a test of their own, because they need the
    // same live daemon and neither needs a second one.
    let narrowed = server.call_tool(
        "wait_for_change",
        json!({ "timeout_seconds": 1, "pane": 2, "kinds": ["pane_closed"] }),
    );
    assert!(
        narrowed.contains("Nothing changed"),
        "a wait narrowed to a pane CLOSING is not answered by anything that has happened: \
         {narrowed}",
    );

    let unknown = server.call_tool_error(
        "wait_for_change",
        json!({ "timeout_seconds": 1, "kinds": ["pane_output"] }),
    );
    assert!(
        unknown.contains("is not a change this terminal reports"),
        "an unknown kind is refused by the daemon that owns the vocabulary: {unknown}",
    );
    assert!(
        unknown.contains("pane_job_changed"),
        "and the refusal offers the whole vocabulary, so the fix is not a guess: {unknown}",
    );
    assert!(
        !unknown.contains("host rpc error"),
        "reaching the agent as the sentence itself, not behind a transport's phrase for a fault \
         nobody anticipated: {unknown}",
    );

    let nonexistent = server.call_tool_error(
        "wait_for_change",
        json!({ "timeout_seconds": 1, "pane": 99 }),
    );
    assert!(
        nonexistent.contains("no pane 99"),
        "and a pane that does not exist is refused BEFORE the park, in this surface's own sentence, \
         rather than waiting forever on a subject nothing can name: {nonexistent}",
    );
}

/// **THE R291 claim, end to end through the surface an agent actually has**: "wait until the
/// command in that pane finishes" is answerable without polling.
///
/// Before this, the only tool that could answer it was `pane_processes`, and calling it in a loop is
/// what `wait_for_change`'s own description tells a caller not to do — each turn of that loop is a
/// full `/proc` pass. `agent_state`'s wait is about an AI reading a screen and says nothing about a
/// build.
///
/// Three properties, and the first is the one most easily got wrong:
///
/// 1. **the establishing sweep is SILENT.** The quiet park below is longer than the daemon's sweep
///    interval, so sweeps certainly ran inside it — and they reported nothing. A watch that
///    announced a pane's first reading would fail here, and would otherwise look identical.
/// 2. a job started AFTER that is reported, naming the pane;
/// 3. the report is a `pane_job_changed`, not an inference from output — the pane is running `sleep`,
///    which prints nothing at all, so no output-matching wait could see it.
///
/// `bash -i` because job control is the mechanism under test: a non-interactive shell runs its
/// commands in its OWN process group and never hands the terminal over, so the fact would never
/// move.
#[test]
fn an_agent_waits_for_a_job_to_start_without_polling() {
    let (_daemon, sock) = spawn_daemon(&["bash", "--norc", "-i"], (80, 24));
    let mut server = McpServer::spawn(&sock);

    // The shell has reached its prompt and owns its own terminal. Read through this surface's own
    // answer rather than slept for, so the state the test starts from is one the daemon published.
    server.wait_for_tool("pane_processes", json!({}), "bash  bash");

    // PROPERTY 1. Park for longer than the daemon's sweep interval on a terminal where nothing is
    // happening. ONE call, not a loop: R292 made output stop returning this tool, so a freshly
    // painted prompt no longer produces an answer to re-call past.
    let quiet = server.call_tool("wait_for_change", json!({ "timeout_seconds": 8 }));
    assert!(
        quiet.contains("Nothing changed"),
        "sweeps ran through that window and published NOTHING — a first reading is not a change: \
         {quiet}",
    );

    // PROPERTY 2. A job the user starts takes the terminal from the shell.
    server.call_tool("write_pane", json!({ "pane": 1, "text": "sleep 300" }));

    // ⚠ ONE CALL, NARROWED — and this is R292's whole claim at the surface that pays for it. R291
    // had to wrap this in a re-call loop, because the pane's own output returned the tool with "the
    // scene moved but nothing structural changed" before the sample carrying the event ever landed.
    // The loop was not a poll (each turn parked in the daemon) but every turn cost a tool result and
    // an LLM turn. Now the caller names what it wants and is woken once.
    let moved = server.call_tool(
        "wait_for_change",
        json!({
            "timeout_seconds": 20,
            "pane": 1,
            "kinds": ["pane_job_changed", "pane_closed"],
        }),
    );
    assert!(
        moved.contains("pane_job_changed: pane 1 (id 0)"),
        "the change names the pane in the NUMBER pane_processes takes, so the follow-up read this \
         tool's description promises can actually be made: {moved}",
    );
    assert!(
        !moved.contains("Nothing changed"),
        "and it was ANSWERED, not timed out — the filter narrowed the wake without losing it: \
         {moved}",
    );

    // PROPERTY 3, stated as its own read: the job really is the silent one, so nothing about this
    // could have come from watching output.
    let running = server.wait_for_tool("pane_processes", json!({}), "sleep 300");
    assert!(
        running.contains("sleep  sleep 300\n"),
        "and the pane is running exactly the command that produced no output: {running}",
    );
}

/// **THE live gate for `wait_for_output`** — a real daemon, a real PTY, and the three answers this
/// tool can give, each driven rather than asserted about.
///
/// It is a live test rather than a unit one because every interesting property of this tool is a
/// property of the PATH: that the park is released by the pane's OWN output (not by a poll), that
/// the search reads what the pane KEPT (not what it is showing), and — the one reading the rendered
/// answers caught — that a deadline is told apart from a terminal that cannot be reached. None of
/// those survives being mocked.
#[test]
fn an_agent_waits_for_the_output_it_named_and_a_timeout_is_not_a_failure() {
    let (_daemon, sock) = spawn_daemon(&["cat"], (80, 6));
    let mut server = McpServer::spawn(&sock);

    // PROPERTY 1. A timeout is an ANSWER, not an error: the pane is quiet and stays quiet.
    let quiet = server.call_tool(
        "wait_for_output",
        json!({ "pane": 1, "needle": "never-printed-by-cat", "timeout_seconds": 2 }),
    );
    assert!(
        quiet.contains("has not printed") && quiet.contains("nothing failed"),
        "a quiet pane answers 'not yet' and says plainly that nothing broke — an agent that read \
         this as a failure would stop waiting for a build that is still running: {quiet}",
    );
    assert!(
        !quiet.contains("Transport(") && !quiet.contains("os error"),
        "and it carries no Rust-shaped internals, which is what the first version of this \
         rendering leaked to an agent beside a sentence saying nothing had failed: {quiet}",
    );

    // PROPERTY 2. The pane produces, and the park is released BY THAT OUTPUT. `cat` echoes, so the
    // fact becomes true on demand — and the marker is followed by enough lines to push it off a
    // six-row screen, which is PROPERTY 3 in the same breath.
    server.call_tool(
        "write_pane",
        json!({ "pane": 1, "text": "the-build-is-done" }),
    );
    let matched = server.call_tool(
        "wait_for_output",
        json!({ "pane": 1, "needle": "the-build-is-done", "timeout_seconds": 20 }),
    );
    assert!(
        matched.contains("printed \"the-build-is-done\""),
        "the wait was answered by the pane's own output: {matched}",
    );

    // PROPERTY 3. THE DISCRIMINATOR AGAINST A RE-READING POLL. Push the marker far past any recent
    // window, then wait for it again: a tool that re-read the pane's last N lines — which is
    // exactly what the rival surface's 100 ms poll does — would find nothing and time out.
    for filler in 0..40 {
        server.call_tool(
            "write_pane",
            json!({ "pane": 1, "text": format!("filler-{filler}") }),
        );
    }
    // Waited for rather than slept past: `cat`'s echo is asynchronous, and a first version of this
    // control read the screen before a single filler had landed and "passed" for the wrong reason.
    // Twice, because on a `cat` pane the first sighting is the PTY's echo of the keystrokes and the
    // second is the child writing the line back.
    let tail = server.wait_for_tool_count(
        "read_pane",
        json!({ "pane": 1, "tail_lines": 6 }),
        "filler-39",
        2,
    );
    assert!(
        !fold(&tail).contains("the-build-is-done"),
        "the control: the marker is outside the recent window a polling reader would look at: \
         {tail}",
    );
    let scrolled = server.call_tool(
        "wait_for_output",
        json!({ "pane": 1, "needle": "the-build-is-done", "timeout_seconds": 20 }),
    );
    assert!(
        scrolled.contains("printed \"the-build-is-done\""),
        "and it is STILL matched, from the pane's retained output — the property a poll that \
         re-reads the screen structurally cannot have: {scrolled}",
    );

    // PROPERTY 4. A pattern the engine refuses is an ERROR, never 'no match yet' — an agent that
    // could not tell those apart would wait out its whole timeout on a typo and then retry it.
    let refused = server.call_tool_error(
        "wait_for_output",
        json!({ "pane": 1, "pattern": "unclosed(", "timeout_seconds": 5 }),
    );
    assert!(
        refused.contains("invalid pattern") && refused.contains("unclosed group"),
        "the engine's own explanation reaches the agent: {refused}",
    );
}

/// **THE surface R343 measured as blind, driven at both agent mouths** — a needle a person reads on
/// one line that the pane broke in half at its right edge.
///
/// `wait_for_output` was the worst-affected door in the product: not slow, not partial, it simply
/// **never fired**, and an agent waiting on a build whose marker happened to wrap waited out its
/// whole timeout on output that was already there. `find_in_pane` answered "no matches" for text on
/// the screen. Both are one search, and it walks LOGICAL lines now.
///
/// THE CONTROL IS `read_pane` IN THE SAME TEST, and it is a control in both directions: the
/// RENDERED view still carries the row break — that is what the screen looks like, and a capture
/// that quietly rejoined lines would stop describing it — while the SEARCH answers about content.
/// The two disagreeing here is the design, and asserting both is what stops a later round
/// "fixing" the render to match.
#[test]
fn an_agent_finds_and_waits_for_a_needle_the_pane_broke_at_its_edge() {
    // Twenty columns, and a marker 24 characters long: one logical line over two rows, with the
    // needle `done-now` straddling the margin (it ends at column 20, the first of row 1).
    let (_daemon, sock) = spawn_daemon(&["cat"], (20, 6));
    let mut server = McpServer::spawn(&sock);
    let marker = "the-build-is-done-now-ok";

    server.call_tool("write_pane", json!({ "pane": 1, "text": marker }));
    // Twice: on a `cat` pane the first sighting is the PTY's echo of the keystrokes and the second
    // is the child writing the line back — waiting for one would race the other.
    let screen = server.wait_for_tool_count("read_pane", json!({ "pane": 1 }), "the-build-is-", 2);

    // THE CONTROL, and the fixture check in one: the pane really did break the marker, so the
    // rendered view does NOT contain it and every assertion below is about the wrap.
    assert!(
        !screen.contains(marker) && fold(&screen).contains(marker),
        "the fixture must wrap: the marker is on the screen but not on any one row of it: \
         {screen:?}",
    );

    let found = server.call_tool("find_in_pane", json!({ "pane": 1, "needle": "done-now" }));
    assert!(
        found.contains(marker),
        "the search finds the needle across the wrap AND quotes back the whole logical line, not \
         the twenty columns that fit on a row: {found}",
    );

    let waited = server.call_tool(
        "wait_for_output",
        json!({ "pane": 1, "needle": "done-now", "timeout_seconds": 20 }),
    );
    assert!(
        waited.contains("printed \"done-now\"") && waited.contains(marker),
        "and the wait fires for it — before R344 this timed out on output already on the screen: \
         {waited}",
    );
}

/// ⚠⚠⚠ **AND THE AGENT CAN NOW ASK FOR THE TEXT THE PROGRAM WROTE**, which until this round it
/// could not.
///
/// The gate above pins that `read_pane` reports the ROW break, because its published description
/// promises *"what a human sees in that pane"* and a capture that quietly rejoined lines would stop
/// describing the screen. That left an agent reasoning about CONTENT with no address at all: the
/// pane's width is set by whoever attached a client to it, so the same output read twice could
/// differ and nothing in either answer said so. `find_in_pane` answered on the written axis and
/// `read_pane` on the rendered one, and there was no way to ask the first question of a whole pane.
///
/// `line_breaks` names WHOSE breaks the caller wants. Both answers are asserted in one test,
/// because either alone is a claim about a single reading rather than about the distinction.
#[test]
fn an_agent_reads_a_pane_by_the_screens_line_breaks_or_by_the_programs() {
    let (_daemon, sock) = spawn_daemon(&["cat"], (20, 6));
    let mut server = McpServer::spawn(&sock);
    let marker = "the-build-is-done-now-ok";

    server.call_tool("write_pane", json!({ "pane": 1, "text": marker }));
    // Twice, for the reason the gate above gives: the echo first, then the child's own write.
    let rendered =
        server.wait_for_tool_count("read_pane", json!({ "pane": 1 }), "the-build-is-", 2);
    assert!(
        !rendered.contains(marker),
        "THE CONTROL AND THE FIXTURE CHECK: the pane really broke the marker, and the default \
         read still describes the screen: {rendered:?}",
    );

    let written = server.call_tool("read_pane", json!({ "pane": 1, "line_breaks": "program" }));
    assert!(
        written.contains(marker),
        "⚠⚠ THE SAME PANE, THE LINE THE PROGRAM WROTE. Without this an agent quoting a pane back \
         to a model, or matching a phrase in it, was reading the width of somebody else's \
         window: {written:?}",
    );

    let refused =
        server.call_tool_error("read_pane", json!({ "pane": 1, "line_breaks": "sideways" }));
    assert!(
        refused.contains("line_breaks") && refused.contains("sideways"),
        "and a word the vocabulary does not publish is refused NAMING BOTH the argument and what \
         was sent, rather than falling back to a default the caller did not choose: {refused:?}",
    );
}

/// **R292 deleted the two re-call helpers that used to live here**, and their absence is the claim.
///
/// `wait_until_quiet` re-called the tool until an answer said the terminal was quiet, and
/// `wait_until_reported` re-called it until an answer named a kind. Both existed for one reason: a
/// pane's OUTPUT released the old park and the tool answered it truthfully ("the scene moved but
/// nothing structural changed"), so a single call could return before the change the caller wanted.
/// Output no longer returns this tool, so both loops collapsed into one call each in
/// `an_agent_waits_for_a_job_to_start_without_polling` — which is the surface's own measure of the
/// round: one tool result and one LLM turn where there used to be as many as the terminal was chatty.
/// **THE live gate for `select_pane`**: the tool moves a fact that lives in the DAEMON and that
/// another surface reports, so a wiring that answered plausibly while sending nothing — or that
/// sent to the wrong pane — would satisfy every unit test of its wording.
///
/// Read back through `list_panes` rather than through the daemon directly, because that pairing is
/// what an agent actually does: it moves the user, then reads the list to see where they are. Both
/// halves are this crate's own surface, and they are wired by different code.
#[test]
fn select_pane_moves_the_session_and_list_panes_says_so() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
    )
    .expect("a second pane to move between");
    let mut server = McpServer::spawn(&sock);

    let listed = server.call_tool("list_panes", json!({}));
    let active_line = |listed: &str| {
        let marked: Vec<String> = listed
            .lines()
            .filter(|line| line.contains("(active)"))
            .map(str::to_owned)
            .collect();
        assert_eq!(marked.len(), 1, "exactly one pane is active: {listed}");
        marked[0].clone()
    };
    assert!(
        active_line(&listed).contains("pane 1:"),
        "the session starts on its first pane: {listed}",
    );

    let moved = server.call_tool("select_pane", json!({ "pane": 2 }));
    assert!(
        moved.contains("The user is now on pane 2"),
        "the tool reports the move it made: {moved}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 2:"),
        "and the pane list — the surface an agent re-reads — agrees",
    );

    // A re-select is a legitimate no-op, and it must not read as a failure to an agent.
    let again = server.call_tool("select_pane", json!({ "pane": 2 }));
    assert!(
        again.contains("was already on pane 2; nothing moved"),
        "a re-select says nothing moved rather than claiming it did: {again}",
    );

    // A pane number nobody holds is an ERROR with the count, the way every pane tool answers one.
    let ghost = server.call_tool_error("select_pane", json!({ "pane": 99 }));
    assert!(
        ghost.contains("no pane 99"),
        "an unknown pane names itself: {ghost}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 2:"),
        "and the refusal left the session where it was",
    );
}

/// **The live gate for the DIRECTIONAL arm** — the head of debt item 23, and the reason it is a live
/// test rather than only a unit one: the whole point of the argument is that the daemon resolves the
/// direction against its own arrangement in the same step it moves, so a tool that assembled the
/// answer from a layout read would pass every test of its wording and still join two instants.
///
/// The fixture is two panes side by side (a spawn appends to the arrangement's spine), the session
/// starting on the first. Every press either moves a state the one before it established or holds an
/// edge the one before it reached — R297's rule, learnt from a GUI test that pressed left twice
/// against a fixture already on the leftmost pane and asserted nothing.
#[test]
fn select_pane_takes_a_direction_and_says_when_there_is_nothing_that_way() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
    )
    .expect("a second pane to move between");
    let mut server = McpServer::spawn(&sock);
    let active_line = |listed: &str| {
        let marked: Vec<String> = listed
            .lines()
            .filter(|line| line.contains("(active)"))
            .map(str::to_owned)
            .collect();
        assert_eq!(marked.len(), 1, "exactly one pane is active: {listed}");
        marked[0].clone()
    };
    // ASSERTED, not assumed: the start is what makes the first press a test.
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 1:"),
        "the session starts on its first pane",
    );

    let right = server.call_tool("select_pane", json!({ "dir": "right" }));
    assert!(
        right.contains("Moved the user one pane right") && right.contains("pane 2"),
        "a direction moves and names where it landed IN THIS SURFACE'S numbers: {right}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 2:"),
        "and the pane list — a different code path — agrees",
    );

    // At the far edge: honest, not a failure, and NOT the same sentence as a re-select. The user
    // stays put, which is what a person pressing an arrow at the side of their layout expects.
    let edge = server.call_tool("select_pane", json!({ "dir": "right" }));
    assert!(
        edge.contains("There is nothing to the right of pane 2") && edge.contains("edge of the"),
        "the edge names the direction asked for and the pane still held: {edge}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 2:"),
        "and nothing moved",
    );

    // Back left, so the fixture ends where a reader can see the walk was real in both directions.
    let back = server.call_tool("select_pane", json!({ "dir": "left" }));
    assert!(
        back.contains("Moved the user one pane left") && back.contains("pane 1"),
        "and the way back is a move again: {back}",
    );

    // The argument shape, as an agent meets it: exactly one naming per call, with a sentence saying
    // what each one means — the daemon can only answer `Rejected`, which names neither.
    let neither = server.call_tool_error("select_pane", json!({}));
    assert!(
        neither.contains("either 'pane'") && neither.contains("'dir'"),
        "a call naming nothing is told what the two choices are: {neither}",
    );
    let both = server.call_tool_error("select_pane", json!({ "pane": 1, "dir": "left" }));
    assert!(
        both.contains("name the target two different ways"),
        "and a call naming both is told they are alternatives: {both}",
    );
    let nonsense = server.call_tool_error("select_pane", json!({ "dir": "sideways" }));
    assert!(
        nonsense.contains("left, right, up, down"),
        "an invented direction is answered with the vocabulary: {nonsense}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 1:"),
        "and none of the three refusals moved the user",
    );
}

/// The agent asks for the pane next to a pane it NAMES, and next to its OWN — the two questions a
/// bare direction cannot ask, against a real daemon.
///
/// Both are paired with the SAME direction asked with no origin, and the pairs answer differently.
/// That pairing is the test: an origin that the daemon dropped would leave every one of these
/// sentences reading exactly like its control, which is precisely how an old daemon fails and why
/// the wire protocol number moved.
///
/// `from_here` is the one an agent cannot reach any other way. The server resolves it from its OWN
/// environment, so it costs no listing at all — and a NUMBER would have had to be looked up first,
/// which is the positional handle a pane closing silently reassigns.
#[test]
fn an_agent_steps_a_direction_from_a_pane_it_names_and_from_its_own() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    for _ in 0..2 {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
        )
        .expect("two more panes, so an origin in the middle has two different sides");
    }
    // The server runs "inside" pane id 1 — the MIDDLE of the three, so its own left and right are
    // both real panes and neither is where the user starts.
    let mut server = McpServer::spawn_in_pane(&sock, 1);
    let active_line = |listed: &str| {
        let marked: Vec<String> = listed
            .lines()
            .filter(|line| line.contains("(active)"))
            .map(str::to_owned)
            .collect();
        assert_eq!(marked.len(), 1, "exactly one pane is active: {listed}");
        marked[0].clone()
    };
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        active_line(&listed).contains("pane 1:"),
        "ASSERTED, not assumed: the session starts on its first pane: {listed}",
    );

    // THE CONTROL: from where the user is (pane 1, the leftmost), left is the edge.
    let control = server.call_tool("select_pane", json!({ "dir": "left" }));
    assert!(
        control.contains("There is nothing to the left of pane 1"),
        "from the user's own pane, left is the edge: {control}",
    );

    // ...and from pane 2 the same word RESOLVES to pane 1 — which is where the user already is, so
    // the answer is the fourth word from a direction, reachable only because an origin exists.
    let already = server.call_tool("select_pane", json!({ "dir": "left", "from": 2 }));
    assert!(
        already.contains("already on pane 1") && already.contains("one step left of pane 2"),
        "a step onto the pane the user is on is a no-op, not an edge: {already}",
    );

    // Now put them on the far side, so the same request MOVES them and the two sentences are
    // separated by the fixture rather than by reading.
    server.call_tool("select_pane", json!({ "pane": 3 }));
    let named = server.call_tool("select_pane", json!({ "dir": "left", "from": 2 }));
    assert!(
        named.contains("Moved the user one pane left of pane 2")
            && named.contains("they are now on pane 1"),
        "an origin the caller NAMED, and both panes in the sentence: {named}",
    );

    // The agent's own pane, with no listing read at all: right of pane 2 (the server's pane id 1 is
    // this listing's pane 2) is pane 3.
    let mine = server.call_tool("select_pane", json!({ "dir": "right", "from_here": true }));
    assert!(
        mine.contains("Moved the user one pane right of the pane you are running in")
            && mine.contains("they are now on pane 3"),
        "the agent's OWN pane is an origin it never has to look up: {mine}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 3:"),
        "and the pane list — a different code path — agrees",
    );

    // An origin at the window's edge: nothing moves, and the sentence names the ORIGIN and the
    // user's pane SEPARATELY, because with an origin they are two different panes.
    let edge = server.call_tool("select_pane", json!({ "dir": "left", "from": 1 }));
    assert!(
        edge.contains("There is nothing to the left of pane 1")
            && edge.contains("the user is still on pane 3"),
        "two panes, two facts: {edge}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 3:"),
        "and the user really did stay",
    );

    // A NAME resolves as an origin exactly as it does as a target — the durable handle, on the
    // argument that decides where a person's cursor goes.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(sprag_host::wire::RENAME_PANE_ACTION),
            "args": { "pane": 0, "name": "build" },
        }),
    )
    .expect("name the first pane");
    let by_name = server.call_tool("select_pane", json!({ "dir": "right", "from": "build" }));
    assert!(
        by_name.contains("right of pane 1 (\"build\")") && by_name.contains("now on pane 2"),
        "a named origin, named back: {by_name}",
    );

    // The argument shape an agent meets, with a sentence for each mistake — the daemon can answer
    // only `Rejected`, which names none of them.
    let both = server.call_tool_error(
        "select_pane",
        json!({ "dir": "left", "from": 1, "from_here": true }),
    );
    assert!(
        both.contains("both say where to step FROM"),
        "two origins is a caller bug, not a precedence to guess: {both}",
    );
    let stray = server.call_tool_error("select_pane", json!({ "pane": 1, "from": 2 }));
    assert!(
        stray.contains("needs 'dir'"),
        "an origin with nothing to be the origin OF is refused rather than ignored: {stray}",
    );
    // A FILLED-IN DEFAULT asks for nothing. `from_here: false` beside an explicit origin is not
    // "two origins", and beside a `pane` it is not "an origin with no direction" — it is a client
    // that serialises every field of its argument struct, which is the same case `SelectAsk::parse`
    // decided for an explicit null one layer down. The first draft of this round refused both.
    let defaulted = server.call_tool(
        "select_pane",
        json!({ "dir": "left", "from": 2, "from_here": false }),
    );
    assert!(
        defaulted.contains("left of pane 2"),
        "from_here: false is absent, so the named origin stands: {defaulted}",
    );
    let plain = server.call_tool("select_pane", json!({ "pane": 2, "from_here": false }));
    assert!(
        plain.contains("The user is now on pane 2"),
        "and it does not turn a plain select into a refusal either: {plain}",
    );
    let ghost = server.call_tool_error("select_pane", json!({ "dir": "left", "from": 99 }));
    assert!(
        ghost.contains("no pane 99"),
        "an origin that is not there names itself: {ghost}",
    );
    assert!(
        active_line(&server.call_tool("list_panes", json!({}))).contains("pane 2:"),
        "and none of the three refusals moved the user",
    );
    let nonsense = server.call_tool_error("select_pane", json!({ "dir": "left", "from_here": 1 }));
    assert!(
        nonsense.contains("must be true or false"),
        "a value that is neither boolean nor absent has no reading: {nonsense}",
    );
}

/// A server that is NOT inside a pane cannot step from one, and says which argument to use instead.
///
/// The same class as `open_pane`'s refusal one tool over: this surface's answer to "I don't know
/// where you are" is a sentence naming the remedy, never a plausible pane.
#[test]
fn from_here_refuses_when_the_server_is_not_inside_a_pane() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
    )
    .expect("a second pane, so a step COULD have gone somewhere");
    // No pane env: the ordinary spawn, which is an agent outside the terminal.
    let mut server = McpServer::spawn(&sock);

    let refused =
        server.call_tool_error("select_pane", json!({ "dir": "right", "from_here": true }));
    assert!(
        refused.contains("not running inside a sprag pane") && refused.contains("'from'"),
        "it says what it could not know and what to send instead: {refused}",
    );
    // THE CONTROL: the same call with an origin it CAN resolve works against the same server.
    let named = server.call_tool("select_pane", json!({ "dir": "right", "from": 1 }));
    assert!(
        named.contains("Moved the user one pane right of pane 1"),
        "so the refusal is about the origin, not about the tool: {named}",
    );
}

// ----- the socket resolve -----

/// The child's own `SPRAG_HOST_RPC_SOCK` beats an ancestor's — the precedence this suite's safety
/// rests on.
///
/// Two daemons run. The server's parent shell advertises the SECOND one; the server itself is given
/// the FIRST. The answer must come from the first, and the two are told apart by the geometry their
/// pane list reports.
///
/// This is not a detail of layering. sprag's developers run this suite inside a sprag pane, where the
/// ancestor walk resolves their LIVE terminal — so if the precedence ever inverted, a test that
/// types into a pane would type into the panes the author is working in. The guard is that every
/// spawn here sets the variable, and this test is what proves setting it is enough.
///
/// REVERT-PROOF: try `ancestor_sock()` before the env var in `host_sock` and the geometry assertion
/// reports the other daemon's size.
#[test]
fn the_child_env_socket_wins_over_an_ancestors() {
    let (_mine, mine) = spawn_daemon(&["cat"], BOOT_PANE);
    let (_theirs, theirs) = spawn_daemon(&["cat"], OTHER_PANE);
    let mut server = McpServer::spawn_behind_ancestor(Some(&mine), &theirs);

    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains(&format!("{}x{}", BOOT_PANE.0, BOOT_PANE.1)),
        "the answer came from the daemon the CHILD was given: {listed}"
    );
    assert!(
        !listed.contains(&format!("{}x{}", OTHER_PANE.0, OTHER_PANE.1)),
        "and not from the one its ancestor advertises: {listed}"
    );
}

/// With no variable of its own, the server finds the daemon its ANCESTOR carries — the two-layer
/// resolve that makes the crate usable in a real pane, and that had never been run.
///
/// This is the production path: an MCP client spawned by an agent inside a pane does not necessarily
/// forward the pane shell's environment, so the server walks up until it finds a process that has it.
/// Here the intermediate shell `unset`s the variable before running the server, so the child's own
/// environment lacks it while the shell's `/proc/<pid>/environ` — the exec-time snapshot — still
/// advertises it. That is exactly the shape of the real case, where the shell was exec'd with the
/// daemon's socket exported and the agent's subprocess did not inherit it.
///
/// The walk stops at the FIRST ancestor that has one, so the daemon found is this test's and never
/// the machine's, whatever the suite is running inside.
///
/// REVERT-PROOF: delete the `ancestor_sock()` fallback from `host_sock` and every tool here reports
/// "not inside a sprag terminal".
#[test]
fn with_no_env_of_its_own_the_server_finds_the_ancestors_daemon() {
    let (_daemon, sock) = spawn_daemon(&["cat"], OTHER_PANE);
    let mut server = McpServer::spawn_behind_ancestor(None, &sock);

    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains(&format!("{}x{}", OTHER_PANE.0, OTHER_PANE.1)),
        "the ancestor's daemon answered: {listed}"
    );
}

/// Outside a sprag terminal every tool says so, and a daemon that cannot be reached says something
/// DIFFERENT — two failures an agent has to be able to tell apart.
///
/// "There is no sprag here, these tools do not apply to your session" and "sprag is here and I could
/// not reach it" call for opposite reactions, so they must not share a message. Both are `isError`
/// content rather than JSON-RPC errors: the request was well-formed and the answer is bad news.
///
/// The first half is why [`McpServer::spawn_orphaned`] exists. Clearing the variable is not enough
/// from a suite that may itself be inside a pane — the walk would climb past this test binary into
/// the author's shell. Re-parenting the server lands it under `init` or the login sub-reaper, neither
/// of which can carry a per-instance socket.
///
/// REVERT-PROOF: give `host_sock` a hard-coded default path and the "not inside" assertion fails;
/// collapse the two messages into one and the second assertion fails.
#[test]
fn a_missing_terminal_and_an_unreachable_one_are_different_answers() {
    // No socket in this process and none in any ancestor it can still reach.
    let mut nowhere = McpServer::spawn_orphaned();
    let text = nowhere.call_tool_error("list_panes", json!({}));
    assert!(
        text.contains("not inside a sprag terminal"),
        "the tools name the situation instead of failing opaquely: {text}"
    );
    drop(nowhere);

    // A socket that exists as a path but that nobody serves: reachable question, unreachable host.
    let dead = std::env::temp_dir().join(format!("sprag-mcp-nobody-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&dead);
    let mut unreachable = McpServer::spawn(&dead);
    let text = unreachable.call_tool_error("list_panes", json!({}));
    assert!(
        text.contains("cannot reach the sprag host") && text.contains(&dead.display().to_string()),
        "an unreachable host names the path it tried: {text}"
    );
    assert!(
        !text.contains("not inside a sprag terminal"),
        "...and does not claim the session has no terminal: {text}"
    );
}

/// Map each pane's host id to the 1-based number this surface's tools take, off `list_panes` itself.
///
/// Read rather than assumed, because the numbering is the pane POOL's order and nothing promises it
/// matches the order panes were created in. It also makes the pinned drawing below immune to
/// confusing the two integers: a boot pane is id `0` and number `1`, so no pane in this file has an
/// id equal to its number, and a rendering that printed one where the other belongs cannot pass.
fn pane_numbers(server: &mut McpServer) -> Vec<(u64, usize)> {
    server
        .call_tool("list_panes", json!({}))
        .lines()
        .filter_map(|line| {
            let (head, rest) = line.trim().strip_prefix("pane ")?.split_once(": id=")?;
            let id = rest.split_whitespace().next()?.parse().ok()?;
            Some((id, head.parse().ok()?))
        })
        .collect()
}

/// **WHERE the panes are, as the agent that must choose one by position receives it** — the head of
/// debt-register item 14.
///
/// Until this tool existed an agent could make a pane active, read it and type into it, but could
/// not learn where any of them WERE: it could not resolve "the pane to the right of mine", which is
/// how a person names a pane. The daemon knew all along — the arrangement is a published slot and
/// adjacency is `LayoutWire::neighbor` — but the only way to ask over the wire was the `select_pane`
/// ACTION, which answers by MOVING THE USER.
///
/// The shape driven here is one no unit test can produce and no other test in this file reaches: a
/// nested split, a pane zoomed to fill the window, and a FLOAT. The float is worth its own mention —
/// R287 could only unit-test that line because no CLI verb floats a pane, and this harness talks to
/// the daemon directly.
///
/// REVERT-PROOF (each measured red, and each killing THIS test alone): read `PANES_SLOT` in place of
/// `LAYOUT_SLOT`; print the host id where the pane number belongs; number a leaf whose pane is not
/// in the list instead of saying so; drop the socket check from `own_pane`.
#[test]
fn the_layout_tool_answers_where_the_panes_are_and_which_one_is_next_to_which() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    // `0 | (right over below)`, then a fourth pane taken OUT of the tiling.
    let right = split_pane(&sock, 0, "horizontal");
    let below = split_pane(&sock, right, "vertical");
    let floated = add_pane(&sock, &["cat"]);
    mux_invoke(
        &sock,
        SET_FLOATING_ACTION,
        json!({ "id": floated, "floating": true }),
    );
    mux_invoke(
        &sock,
        ZOOM_PANE_ACTION,
        json!({ "pane": below, "on": true }),
    );

    // The server believes it is running IN the pane on the right, which is what gives a direction
    // something to be relative to.
    let mut server = McpServer::spawn_in_pane(&sock, right);
    let numbers: Vec<(u64, usize)> = pane_numbers(&mut server);
    let number = |id: u64| {
        numbers
            .iter()
            .find(|(pane, _)| *pane == id)
            .unwrap_or_else(|| panic!("pane {id} is in the list: {numbers:?}"))
            .1
    };
    let drawing = server.call_tool("pane_layout", json!({}));
    let revision = drawing
        .lines()
        .next()
        .and_then(|line| line.rsplit_once("revision ")?.1.strip_suffix("):"))
        .unwrap_or_else(|| panic!("the answer heads with the arrangement's revision: {drawing}"))
        .to_owned();

    assert_eq!(
        drawing,
        format!(
            "How YOUR WINDOW's panes are arranged (revision {revision}):\n\
             \n\
             50% left|right\n\
             ├─ pane {n0} (id 0)\n\
             └─ 50% top|bottom\n\
             \x20  ├─ pane {nr} (id {right})  (you are here)\n\
             \x20  └─ pane {nb} (id {below})  (fills the window)\n\
             floating: pane {nf} (id {floated})\n\
             \n\
             Which pane is next to which (a direction not listed has no pane that way — that pane \
             is at that edge of the window):\n\
             \x20 pane {n0}: right=pane {nr}\n\
             \x20 pane {nr}: left=pane {n0}, down=pane {nb}\n\
             \x20 pane {nb}: left=pane {n0}, up=pane {nr}\n\
             \n\
             Pass a pane NUMBER (not an id) to read_pane, write_pane, send_keys or select_pane. \
             Which pane the user is typing into right now is list_panes' answer, not this one. To \
             MOVE the user beside a pane, do not read a number from here and select it — that is \
             two moments; call select_pane with 'dir' plus 'from' or 'from_here: true' and the \
             terminal resolves it in one.\n",
            n0 = number(0),
            nr = number(right),
            nb = number(below),
            nf = number(floated),
        ),
        "the whole answer, from the daemon's own arrangement",
    );
}

/// A pane id published by a DIFFERENT daemon marks nothing.
///
/// Ids are per-daemon and start at zero, so a box running two sprag terminals has two pane `1`s:
/// taking the first `SPRAG_PANE` within reach would mark a real, plausible pane of the terminal
/// being asked about — wrong in the one way a reader cannot see. The pair is only trusted from an
/// environment whose ADDRESS half is the socket this process actually asked.
///
/// The companion of `the_child_env_socket_wins_over_an_ancestors`, on the identity half of the same
/// rendezvous, and the reason the resolve is a pair rather than two independent lookups.
#[test]
fn a_pane_id_advertised_for_another_daemon_marks_nothing() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let (_other, other_sock) = spawn_daemon(&["cat"], OTHER_PANE);

    // The ancestor advertises the OTHER daemon, and a pane id that exists in BOTH.
    let mut server = McpServer::spawn_behind_foreign_pane(&sock, &other_sock, 0);
    let drawing = server.call_tool("pane_layout", json!({}));
    assert!(
        drawing.contains("pane 1 (id 0)") && !drawing.contains("you are here"),
        "the pane is drawn and NOT claimed as ours: {drawing}",
    );

    // The control: the same id, published with the socket actually in use, does mark.
    let mut inside = McpServer::spawn_in_pane(&sock, 0);
    let mine = inside.call_tool("pane_layout", json!({}));
    assert!(
        mine.contains("pane 1 (id 0)  (you are here)"),
        "THE CONTROL: only the address half differs, so a resolver ignoring it marks both: {mine}",
    );
}

/// An agent opens a pane of its OWN, runs something in it, and closes it again — the whole loop
/// against a real daemon and the shipped binary.
///
/// This is the round's claim end to end: every other tool on this surface works on a pane somebody
/// else opened, and the four rounds before it built "run it over there and wait" on top of a first
/// step the agent could not take. What is pinned here rather than in a unit test is the part a unit
/// test cannot see — that the provenance the daemon recorded is the same fact the CLOSE gate reads
/// back, through two separate processes and a socket.
#[test]
fn an_agent_opens_a_pane_of_its_own_and_closes_it_again() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    let dir = std::env::temp_dir();
    let opened = server.call_tool("open_pane", json!({ "cwd": dir.to_str().unwrap() }));
    assert!(
        opened.starts_with(&format!(
            "Opened pane 2 in {}, running a shell.",
            dir.display()
        )),
        "it names the pane's NUMBER and where it opened: {opened}",
    );
    assert!(
        opened.contains("2 pane(s) in this window (list_windows for the session's others):"),
        "and re-lists every pane, so the caller's map of numbers is repaired in the same \
         answer: {opened}",
    );
    assert!(
        opened.contains("      opened by: you (yours to close)"),
        "the listing marks the new pane as the agent's own: {opened}",
    );
    // The tool's own description promises this, and an agent believes the description: opening a
    // work pane must not move where the USER is typing. It holds because the birth is a SPAWN —
    // a `split` selects its new pane (tmux's rule) and would have taken the cursor with it.
    assert!(
        opened.contains("  pane 1: id=0 40x6 command=cat title=(none) (active)"),
        "the person is still on the pane they were on: {opened}",
    );

    // The boot pane is the PERSON's, and no agent may close it. Checked before the happy path so a
    // gate that refused nothing could not pass this test by having already closed everything.
    let refused = server.call_tool_error("close_pane", json!({ "pane": 1 }));
    assert!(
        refused.contains("pane 1 was opened by a person, not by you"),
        "a pane nobody claims is refused, in a sentence that says why: {refused}",
    );

    // AND SO IS ITS RESOURCE GRANT. ⚠ `grant_pane` shipped WITHOUT this guard and the debt question
    // found it: the primer promises every changing tool acts only on a pane the agent opened, and a
    // new writing tool that skipped the rule made that sentence false. How much of a person's own
    // machine their work may use is theirs to decide; an agent that could lower it would be taking
    // cores from work it cannot see.
    let ungranted = server.call_tool_error("grant_pane", json!({ "pane": 1, "share": 10 }));
    assert!(
        ungranted.contains("pane 1 was opened by a person, not by you"),
        "a person's pane is not an agent's to hold back: {ungranted}",
    );
    assert!(
        ungranted.contains("your OWN pane"),
        "and the refusal says what to do instead, because the useful action still exists: \
         {ungranted}",
    );
    assert!(
        server
            .call_tool("list_panes", json!({}))
            .contains("2 pane(s)"),
        "and the refusal really refused — both panes are still here",
    );

    // A SECOND work pane, so closing the first one really does renumber something. Without it the
    // renumbering sentence would be untested in the direction it exists for.
    let second = server.call_tool("open_pane", json!({}));
    assert!(
        second.contains("Opened pane 3"),
        "an open APPENDS, so the numbers already handed out do not move: {second}",
    );

    let closed = server.call_tool("close_pane", json!({ "pane": 2 }));
    assert!(
        closed.starts_with(
            "Closed pane 2 (id 1), which you had opened. The panes after it have MOVED UP a number:"
        ),
        "the close names what it ended, in both vocabularies, and says the map moved: {closed}",
    );
    assert!(
        closed.contains("  pane 2: id=2 ")
            && closed.contains("2 pane(s) in this window (list_windows for the session's others):"),
        "and the re-listing PROVES it moved — the pane that was 3 is now 2: {closed}",
    );

    // The last pane makes no claim about renumbering, because nothing followed it. A fixed sentence
    // would be false here, which is what reading the rendered answer caught.
    let last = server.call_tool("close_pane", json!({ "pane": 2 }));
    assert!(
        last.starts_with(
            "Closed pane 2 (id 2), which you had opened. It was the last pane, so the others keep \
             their numbers:"
        ),
        "closing the last pane says so instead: {last}",
    );
    assert!(
        last.contains("1 pane(s) in this window (list_windows for the session's others):"),
        "and only the person's pane is left: {last}",
    );
}

/// **A pane NAME reaches ANOTHER WINDOW of the session, and a NUMBER still does not** — R311's
/// whole claim, end to end through the shipped server against a real daemon.
///
/// Measured at `dac6ef7` before a line was written: `read_pane {pane: "buildout"}` from a sibling
/// window answered *"no pane is called \"buildout\"; no pane in this terminal has a name yet"* —
/// BOTH halves false — while `rename_pane` and `swap_pane` crossed a window freely, because a write
/// is a mux action and a read was a scene path into a scene that holds one window.
///
/// The CONTROLS are what make it mean something, and there are three: the agent's own pane does
/// NOT hold what was written across the window (so the write really crossed), a NUMBER that would
/// name the far pane is still refused window-locally (so the contract a number carries is intact),
/// and the refusal for an absent name now lists the session's named panes WITH their windows.
#[test]
fn a_pane_name_reaches_another_window_and_a_number_does_not() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    // A second WINDOW holding one pane, named — then back to the boot window, which is where the
    // agent runs. `new_window` selects what it creates, so the select back is what makes the far
    // window far.
    mux_invoke(&sock, NEW_WINDOW_ACTION, json!({}));
    // The new window's birth pane, read off the daemon's own list for that window rather than
    // guessed: `new_window` selected it, so the unscoped `panes` slot is already its list.
    let far = mux_query_panes(&sock)
        .first()
        .copied()
        .expect("the new window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "buildout" }),
    );
    mux_invoke(&sock, SELECT_WINDOW_ACTION, json!({ "window": "0" }));

    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // 1. The agent's own window is ONE pane, and the listing says so about the WINDOW.
    let here = server.call_tool("list_panes", json!({}));
    assert!(
        here.contains("1 pane(s)") && !here.contains("buildout"),
        "the agent's own window does not hold the far pane: {here}",
    );

    // 2. `list_windows` is what tells it the other window exists, and hands it the NAME.
    let windows = server.call_tool("list_windows", json!({}));
    assert!(
        windows.contains("2 window(s)")
            && windows.contains("you are here")
            && windows.contains("\"buildout\""),
        "list_windows names the other window and the pane in it: {windows}",
    );

    // 3. The NAME reaches it — write, then read it back.
    let wrote = server.call_tool(
        "write_pane",
        json!({ "pane": "buildout", "text": "printf R311-CROSSED" }),
    );
    assert!(
        wrote.contains("(window") && !wrote.contains("pane 1"),
        "the answer names the pane the only honest way for one a number cannot reach: {wrote}",
    );
    let read = server.wait_for_tool("read_pane", json!({ "pane": "buildout" }), "R311-CROSSED");
    assert!(
        read.contains("R311-CROSSED"),
        "read across the window: {read}"
    );

    // CONTROL A — the agent's OWN pane does not hold it, so the write really crossed.
    let mine = server.call_tool("read_pane", json!({ "pane": 1 }));
    assert!(
        !mine.contains("R311-CROSSED"),
        "the write went to the far window, not to the agent's own pane: {mine}",
    );

    // CONTROL B — a NUMBER is still window-local. `pane: 2` names nothing here even though the
    // session holds a second pane, which is the contract `list_panes` defines and R311 keeps.
    let refused = server.call_tool_error("read_pane", json!({ "pane": 2 }));
    assert!(
        refused.contains("no pane 2") && refused.contains("Call list_panes."),
        "a number reaches only the agent's own window: {refused}",
    );

    // CONTROL C — an absent NAME is refused with the session's named panes AND their windows,
    // where the old sentence claimed no pane in the terminal had a name at all.
    let unknown = server.call_tool_error("read_pane", json!({ "pane": "nope" }));
    assert_eq!(
        unknown,
        "Error: no pane is called \"nope\"; the session's named panes are \"buildout\" \
         (window 1). Call list_windows.",
    );
}

/// **THE RATCHET: every tool the roster declares a `pane` argument for resolves a NAME in ANOTHER
/// window — and the list is DERIVED FROM THE ROSTER, not written here.**
///
/// # Why the list is derived
///
/// R311 widened seven tools and its primer claimed all of them; the owner's debt question found it
/// after the push. The fix was a test — with a HAND-WRITTEN list of twelve tools. Measuring at
/// `e7be5eb` showed that list was itself incomplete: **eleven tools refused, not the five the debt
/// register recorded**, and the two the list omitted (`agent_explain`, `wait_for_change`) were
/// exactly the two the corrected sentence forgot. A hand-written list of what to check is the same
/// class of defect one level up, so this walks `tools/list` and takes every tool whose own
/// `inputSchema` declares a `pane` property.
///
/// A tool added later with a `pane` argument is therefore checked the day it appears, and a tool
/// that resolves its pane against the caller's own window fails here by name.
///
/// # What it asserts, and why not "succeeds"
///
/// That the ADDRESS RESOLVES — never that the call succeeds. Four of these verbs are gated on
/// authorship (R294): a pane a person opened is refused by `close_pane`, `rename_pane`,
/// `swap_pane` and `resize_pane` whatever window it is in, and that is the correct answer. What
/// must not happen is the refusal being about the NAME, which is what "no pane is called
/// \"faraway\"" was for eleven tools — a true sentence about the wrong subject.
#[test]
fn the_whole_roster_reaches_a_pane_one_window_over() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    mux_invoke(&sock, NEW_WINDOW_ACTION, json!({}));
    let far = mux_query_panes(&sock)
        .first()
        .copied()
        .expect("the new window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "faraway" }),
    );
    mux_invoke(&sock, SELECT_WINDOW_ACTION, json!({ "window": "0" }));
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // ⚠ THE FIXTURE MUST DISCRIMINATE. A one-window daemon would make "reaches another window"
    // trivially true — which is verbatim the mistake R311's first skew probe made. Assert the two
    // panes really are in different windows before believing anything below.
    let windows = server.call_tool("list_windows", json!({}));
    assert!(
        windows.contains("window 0 (current, you are here)") && windows.contains("window 1"),
        "the fixture must hold TWO windows or nothing below discriminates: {windows}",
    );

    // A sample value per ARGUMENT NAME, not per tool — so a tool added later out of arguments that
    // already appear here needs no edit, and one that introduces a NEW argument fails loudly
    // naming it. Failing closed is the point: a silent skip would be a tool this ratchet believes
    // it covered.
    let sample = |argument: &str| -> Option<Value> {
        Some(match argument {
            "pane" => json!("faraway"),
            "needle" | "pattern" | "text" => json!("zz"),
            "keys" => json!(["Escape"]),
            "name" => json!("faraway"),
            "dir" => json!("left"),
            "with" => json!("faraway"),
            // The arrangement verbs R335 added. Both name a real thing of this fixture, because
            // the claim under test is that the request REACHED the far pane — a sample the
            // resolver rejects would fail for the wrong reason and prove nothing.
            "window" => json!("0"),
            "target" => json!("faraway"),
            "timeout_seconds" => json!(1),
            // The loop's discriminator (R355). `orchestrator` is the form that names a `pane`,
            // which is the argument this ratchet is about — and the run never starts, because the
            // far pane is somebody else's and the authorship refusal below is what proves the
            // request reached it.
            "plugin" => json!("orchestrator"),
            // The consent's two needles (R369). Any words will do here: the far pane belongs to
            // somebody else, so the authorship refusal fires before a question is ever read — and
            // that refusal is the evidence the request REACHED a pane a window over, which is all
            // this ratchet is about.
            "asked" | "answer" => json!("zz"),
            _ => return None,
        })
    };

    let roster = server.request("tools/list", json!({}));
    let tools = roster["result"]["tools"]
        .as_array()
        .expect("the roster is a list")
        .clone();
    let mut checked: Vec<String> = Vec::new();
    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .expect("every tool is named")
            .to_owned();
        let schema = &tool["inputSchema"]["properties"];
        if schema.get(SelectAsk::PANE_KEY).is_none() {
            continue;
        }
        // Every argument the tool REQUIRES, plus the pane. A tool with no `required` list gets the
        // pane alone, which is the shape most reads have.
        let mut args = serde_json::Map::new();
        args.insert(SelectAsk::PANE_KEY.to_owned(), json!("faraway"));
        for required in tool["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let argument = required.as_str().expect("a required argument is named");
            let value = sample(argument).unwrap_or_else(|| {
                panic!(
                    "{name} requires an argument this ratchet has no sample for ({argument:?}). \
                     Add one to `sample` — a skipped tool is a tool this test believes it covered."
                )
            });
            args.insert(argument.to_owned(), value);
        }
        // `wait_for_output` needs a search language and would otherwise park for its full default
        // minute; `wait_for_change` is bounded the same way. Both take these as OPTIONAL arguments,
        // so the required-set above does not carry them.
        if name.starts_with("wait_for") {
            args.insert("timeout_seconds".to_owned(), json!(1));
            if name == "wait_for_output" {
                args.insert("needle".to_owned(), json!("zz"));
            }
        }
        // `grant_pane` refuses a request that sets nothing — deliberately, because a grant with no
        // settings is somebody who meant something and typed it wrong. That refusal is not about
        // WHERE the pane is, which is what this ratchet measures, so it is given a setting to
        // carry. The weight every pane is born with, so a host that enforces nothing and a host
        // that does are both left exactly as they were.
        if name == "grant_pane" {
            args.insert("share".to_owned(), json!(100));
        }
        let answer = server.call_tool_raw(&name, Value::Object(args));
        let errored = answer["result"]["isError"] == json!(true);
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        // ⚠ THE FIRST VERSION OF THIS ASSERTION WAS TOO WEAK, and a revert-proof is what found it:
        // it checked only that the sentence was not "no pane is called", so a tool that resolved
        // the NAME and then sent the request WITHOUT the window — which reaches the wrong window's
        // scene and fails some other way — passed. Dropping the window out of `pane_params` left
        // this green while the whole round's mechanism was gone.
        //
        // So the assertion is total: the call SUCCEEDS, or it refuses on AUTHORSHIP (R294's gate,
        // which is about who opened the pane and not about where it is). Any other error means the
        // request did not arrive at the pane the caller named.
        // A THIRD legitimate outcome, and it is not a loophole: `grant_pane` writes to a cgroup,
        // and a host with no delegated subtree has none to write to. That refusal is a fact about
        // the MACHINE — it is identical for a pane in the caller's own window — so it says nothing
        // about whether the request reached the pane it named, which is all this ratchet measures.
        // ⚠ Measured, not reasoned: without this the test passes on this developer's box (systemd
        // user delegation available) and FAILS on macOS, where `with_shares` is `cfg`-ed out
        // entirely. Reproduce with `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/bus`.
        let unenforced = text.contains("no cgroup subtree");
        assert!(
            !errored || text.contains("not by you") || unenforced,
            "{name} did not reach a pane one window over — it must succeed, refuse on \
             authorship, or refuse because this host enforces nothing, and it did none: {text}",
        );
        checked.push(name);
    }

    // The ratchet is worth nothing if it walked an empty list, and it must cover the tools the
    // measurement at `e7be5eb` found REFUSING — naming them here is what makes this a regression
    // pin for that specific defect rather than a shape test that would pass on an empty roster.
    for wanted in [
        "read_pane",
        "read_pane_images",
        "pane_processes",
        "agent_state",
        "agent_explain",
        "resize_pane",
        "rename_pane",
        "close_pane",
        "select_pane",
        "swap_pane",
        "wait_for_output",
        "wait_for_change",
        "pane_layout",
    ] {
        assert!(
            checked.iter().any(|name| name == wanted),
            "the roster no longer declares a `pane` argument for {wanted}, so this ratchet stopped \
             covering it: checked {checked:?}",
        );
    }
    assert!(
        checked.len() >= 13,
        "the ratchet walked too few tools to be measuring anything: {checked:?}",
    );

    // The PRIMER's claim, checked against what was just measured — the sentence R311 got wrong is
    // now a claim under test rather than prose somebody keeps up to date.
    let primer = server.primer();
    assert!(
        primer.contains(
            "A pane NAME reaches ANY window of this session, at EVERY tool here that \
                         takes a `pane`"
        ),
        "the primer must claim exactly the reach the roster was just measured to have: {primer}",
    );
    assert!(
        !primer.contains("resolve a name inside your own window"),
        "and it must not still describe a split that no longer exists: {primer}",
    );
}

/// An agent NAMES its work pane and then addresses it by that name while its NUMBER moves under it.
///
/// This is the round's claim end to end, and the middle step is the whole of it: a pane is opened,
/// a pane BEFORE it closes, and the same name still reaches the same pane. A test that named a pane
/// and read it back without moving anything would pass on a build where the name was a decoration.
#[test]
fn a_named_pane_answers_to_its_name_after_its_number_has_moved() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // Two work panes: the first exists only to be closed, so the second one's number really shifts.
    let doomed = server.call_tool("open_pane", json!({ "name": "scratch" }));
    assert!(
        doomed.contains("Opened pane 2")
            && doomed.contains(
                "It is called \"scratch\" — pass that as `pane` instead of the number, which will \
                 shift if an earlier pane closes."
            ),
        "the answer offers the name AND says why to use it: {doomed}",
    );
    let build = server.call_tool("open_pane", json!({ "name": "build" }));
    assert!(build.contains("Opened pane 3"), "{build}");
    assert!(
        build.contains("  pane 3: name=\"build\" id=2 "),
        "the listing carries the name beside the number it stands in for: {build}",
    );

    // A name already taken is refused with the ONE fact the daemon observed, and it names the pane
    // that holds the name — which the guess this replaces could not, because it was not there.
    //
    // Until R325 consumed PINION-PR82 an agent read *"the name may already be taken by another
    // pane, or be blank, over 80 bytes, all digits, or contain a control character. Call
    // list_panes to see which names are in use."*: five causes and a command to run, because a
    // bare `InvokeRejected` left this surface nothing else to offer.
    let taken = server.call_tool_error("open_pane", json!({ "name": "build" }));
    assert_eq!(
        taken, "Error: pane 2 is already called \"build\"",
        "the WHOLE sentence, and it is the DAEMON's — an agent reasons about facts, not lists",
    );
    assert!(
        !taken.contains(" or ") && !taken.contains("may already"),
        "and it is not a disjunction, and does not hedge: {taken}",
    );

    // THE MOVE. Closing pane 2 renumbers pane 3 to pane 2 — the exact failure this feature exists
    // for, since an agent holding "3" would now be typing into a different pane.
    let closed = server.call_tool("close_pane", json!({ "pane": "scratch" }));
    assert!(
        closed.starts_with("Closed pane 2 (id 1), which you had opened."),
        "a NAME reaches the close gate too, resolved against the same listing: {closed}",
    );
    assert!(
        closed.contains("  pane 2: name=\"build\" id=2 "),
        "and the pane that was 3 is now 2 — the number moved: {closed}",
    );

    // EVERY tool that takes a `pane` takes the name — not just the ones this test drives for their
    // own sake. The sweep is the point: the first version of the resolver gave four of these two
    // separate readings of the pane list (a query hidden behind the argument parse, then the
    // listing the tool reads for itself), which is the torn read a name exists to prevent
    // reintroduced by the feature that prevents it. Nothing failed; the tools still worked.
    for tool in [
        "read_pane",
        "read_pane_links",
        "read_pane_images",
        "read_last_command",
        "agent_state",
        "agent_explain",
        "pane_processes",
    ] {
        let answer = server.call_tool(tool, json!({ "pane": "build" }));
        assert!(
            !answer.starts_with("Error:"),
            "{tool} must take a NAME where it takes a number: {answer}",
        );
    }

    // The DRAWING carries it too, and that is where it matters most: `pane_layout` is where an
    // agent CHOOSES a pane, so answering only in numbers would hand it the vocabulary that moves.
    // It costs no extra read — the pane list is already in hand for the numbering.
    let drawing = server.call_tool("pane_layout", json!({}));
    assert!(
        drawing.contains("pane 2 (id 2) name=\"build\""),
        "the arrangement names the pane in the handle that survives a close: {drawing}",
    );

    // The claim: the name did not.
    //
    // ⚠⚠⚠ READ AT THE PROGRAM'S OWN LINE BREAKS, AND THAT IS THE WHOLE OF REGISTER ITEM 205. This
    // asked for the default (`screen`) and asserted a ten-character string against ROWS of a
    // FORTY-COLUMN pane — so whether it passed depended on how long the shell's prompt happened to
    // be. **Measured, two hosts, one variable**: green on a workstation whose prompt is short, RED
    // on `pc4`, where the prompt is
    // `icp@ivis-tpeg:/mnt/ICP-Working/remote-build/sprag/crates/sprag-mcp(main)$ ` — seventy-three
    // characters — so `echo alive` wrapped after `echo a` and the row-joined read found `a` and
    // `live` apart. Deterministic on both, which is what made it a defect rather than a flake.
    //
    // ⚠⚠ THE REMEDY IS THE PRODUCT'S OWN WORD, not a trick this test invented: `line_breaks` exists
    // because the two slots answer different questions — *where the TERMINAL broke the lines* and
    // *where the PROGRAM did* — and the reader that must not depend on a pane's width is the second
    // one. What this gate is about is a NAME reaching a pane; the terminal's wrap points were never
    // part of the claim, and asserting through them made somebody's working directory a variable.
    server.call_tool(
        "write_pane",
        json!({ "pane": "build", "text": "echo alive" }),
    );
    let read = server.call_tool(
        "read_pane",
        json!({ "pane": "build", "line_breaks": "program" }),
    );
    assert!(
        read.contains("echo alive"),
        "the name reached the pane it was given to, at a number it no longer has: {read}",
    );

    // Renaming is the same gate as closing: the person's pane is refused.
    let refused = server.call_tool_error("rename_pane", json!({ "pane": 1, "name": "theirs" }));
    assert!(
        refused.contains("pane 1 was opened by a person, not by you"),
        "a pane's name is what a PERSON reads on it: {refused}",
    );

    // ⚠⚠ AND SO IS STOPPING WHAT A PERSON'S PANE IS RUNNING, which is the strongest form of this
    // gate on the surface: a rename changes a label and a stop ends somebody's work.
    let refused = server.call_tool_error("stop_job", json!({ "pane": 1 }));
    assert!(
        refused.contains("pane 1 was opened by a person, not by you")
            && refused.contains("theirs to decide"),
        "an agent must not end work it did not start: {refused}",
    );
    // ⚠ And a word this tool does not send is refused WITH THE LIST — an agent told only that its
    // argument was wrong will guess again, which is the one thing a closed vocabulary is for.
    let mistyped = server.call_tool_error("stop_job", json!({ "pane": "build", "signal": "maim" }));
    assert!(
        mistyped.contains("interrupt")
            && mistyped.contains("terminate")
            && mistyped.contains("kill"),
        "the refusal lists every word the tool takes: {mistyped}",
    );
    // The agent's OWN pane answers, and the answer names what received the stop and refuses to
    // claim obedience — the distinction the tool exists to make.
    let stopped = server.call_tool("stop_job", json!({ "pane": "build" }));
    assert!(
        stopped.contains("process group")
            && stopped.contains("interrupted")
            && stopped.contains("not obedience"),
        "a stop names what it reached and says what it does not promise: {stopped}",
    );

    let renamed = server.call_tool("rename_pane", json!({ "pane": "build", "name": "tests" }));
    assert!(
        renamed.starts_with("pane 2 is now called \"tests\".")
            && renamed.contains("a name reaches any window of this session"),
        "a rename reports the name that was RECORDED, and why to use it: {renamed}",
    );
    let gone = server.call_tool_error("read_pane", json!({ "pane": "build" }));
    assert!(
        gone.contains("no pane is called \"build\"") && gone.contains("\"tests\""),
        "the old name stops resolving and the refusal lists the names in use: {gone}",
    );

    let cleared = server.call_tool("rename_pane", json!({ "pane": "tests" }));
    assert!(
        cleared.starts_with("pane 2 has no name now;"),
        "and a rename with no name takes it away: {cleared}",
    );
    assert!(
        !server.call_tool("list_panes", json!({})).contains("name="),
        "the listing says nothing about a name once no pane has one",
    );
}

/// A pane opened by ANOTHER pane's agent is refused too, and the refusal names which pane to go
/// and ask.
///
/// The distinct case from the one above: "nobody opened this" and "somebody else opened this" are
/// different mistakes, and a gate that only compared against `None` would let one agent close
/// another's work pane.
#[test]
fn an_agent_cannot_close_a_pane_another_pane_opened() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut theirs = McpServer::spawn_in_pane(&sock, 0);
    theirs.call_tool("open_pane", json!({}));

    // A second agent, in a pane of its own, sees the pane and is told whose it is.
    let mine = add_pane(&sock, &["cat"]);
    let mut server = McpServer::spawn_in_pane(&sock, mine);
    let refused = server.call_tool_error("close_pane", json!({ "pane": 2 }));
    assert!(
        refused.contains("pane 2 was opened by pane 1, not by you"),
        "the refusal names the pane that did open it, in this surface's numbers: {refused}",
    );
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains("      opened by: pane 1\n"),
        "and the listing says the same thing, without claiming it as ours: {listed}",
    );
}

/// **The live gate for `swap_pane`**: an agent PLACES the pane it opened, and is refused every other
/// one — against a real daemon and the shipped binary.
///
/// It is a live test rather than a unit one for two reasons a unit test cannot reach. The gate reads
/// a provenance the DAEMON recorded through a socket, so this is the third verb proving that one
/// fact is one fact (`close_pane` and `rename_pane` are the other two). And the arrangement is read
/// back through `pane_layout` — a different code path from the tool's own answer — because a tool
/// that reported a trade it had not made would otherwise pass on its own wording.
///
/// The ORDER is deliberate: the refusals are checked BEFORE the happy path, so a gate that refused
/// nothing could not pass this test by having already moved everything.
#[test]
fn an_agent_places_the_pane_it_opened_and_no_other() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_pane", json!({ "name": "build" }));
    server.call_tool("open_pane", json!({ "name": "logs" }));
    // ASSERTED, not assumed: the two opens APPEND, so the spine is 1 | 2 | 3 and the presses below
    // each have somewhere to go.
    let before = server.call_tool("pane_layout", json!({}));
    assert!(
        before.contains("pane 1: right=pane 2")
            && before.contains("pane 2: left=pane 1, right=pane 3"),
        "the fixture starts with the person's pane LEFTMOST — read from the adjacency table, \
         because the pane NUMBERS are pool order and a swap deliberately does not move them: \
         {before}",
    );

    // The PERSON's pane is not the agent's to place — the same sentence the close and the rename
    // give, on the same fact.
    let refused = server.call_tool_error("swap_pane", json!({ "pane": 1, "dir": "right" }));
    assert!(
        refused.contains("pane 1 was opened by a person, not by you")
            && refused.contains("their arrangement"),
        "it says why, and that the reason is the arrangement rather than the pane: {refused}",
    );
    assert_eq!(
        server.call_tool("pane_layout", json!({})),
        before,
        "and the refusal really refused — the arrangement is untouched",
    );

    // Neither partner, and both: one sentence each, because the daemon can only answer `Rejected`.
    let neither = server.call_tool_error("swap_pane", json!({ "pane": "build" }));
    assert!(
        neither.contains("either 'with'") && neither.contains("'dir'"),
        "a call naming no partner is told what the two choices are: {neither}",
    );
    let both = server.call_tool_error(
        "swap_pane",
        json!({ "pane": "build", "with": 3, "dir": "left" }),
    );
    assert!(
        both.contains("name the partner two different ways"),
        "and a call naming both is told they are alternatives: {both}",
    );

    // THE HAPPY PATH, by direction: the agent's own pane trades with the person's.
    let moved = server.call_tool("swap_pane", json!({ "pane": "build", "dir": "left" }));
    assert!(
        moved.contains("Moved pane 2 (\"build\") one place left")
            && moved.contains("pane 1")
            && moved.contains("Nobody's cursor moved"),
        "it names both panes and says what it did NOT do: {moved}",
    );
    let after = server.call_tool("pane_layout", json!({}));
    assert!(
        after.contains("pane 2: right=pane 1")
            && after.contains("pane 1: left=pane 2, right=pane 3"),
        "and the arrangement — a different code path — agrees the two traded PLACES: {after}",
    );
    assert!(
        after.contains("pane 1 (id 0)  (you are here)"),
        "the agent's own pane keeps its NUMBER through a trade, which is the panes/layout split: \
         a number is pool order and a swap moves cells: {after}",
    );

    // AT THE EDGE: honest, not a failure, and not the same sentence as a trade with itself. `build`
    // is leftmost now, which the reading above established rather than assumed.
    let edge = server.call_tool("swap_pane", json!({ "pane": "build", "dir": "left" }));
    assert!(
        edge.contains("There is nothing to the left of pane 2 (\"build\")")
            && edge.contains("edge of the window"),
        "the edge names the direction asked for and the pane still held: {edge}",
    );
    assert_eq!(
        server.call_tool("pane_layout", json!({})),
        after,
        "and nothing moved",
    );

    // ...and by PARTNER, naming the person's pane as the one displaced: that is legal, because a
    // swap places the pane the caller owns and the other one goes where it came from.
    let named = server.call_tool("swap_pane", json!({ "pane": "build", "with": "logs" }));
    assert!(
        named.contains("pane 2 (\"build\") and pane 3 (\"logs\") have traded places"),
        "the partner arm names both panes in this surface's handles: {named}",
    );
    let swapped = server.call_tool("pane_layout", json!({}));
    assert!(
        swapped.contains("pane 3: right=pane 1")
            && swapped.contains("pane 1: left=pane 3, right=pane 2"),
        "and the two really traded — `logs` is leftmost now and `build` is where it was: \
         {swapped}",
    );

    // A pane traded with ITSELF is a no-op with its own sentence, never a failure.
    let itself = server.call_tool("swap_pane", json!({ "pane": "build", "with": "build" }));
    assert!(
        itself.contains("is the pane you asked to trade it with"),
        "and it is not reported as an error: {itself}",
    );
}

/// A pane opened by ANOTHER pane's agent cannot be PLACED either — the second half of the gate, on
/// the verb where "somebody else opened this" is the interesting case.
#[test]
fn an_agent_cannot_place_a_pane_another_pane_opened() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut theirs = McpServer::spawn_in_pane(&sock, 0);
    theirs.call_tool("open_pane", json!({}));

    let mine = add_pane(&sock, &["cat"]);
    let mut server = McpServer::spawn_in_pane(&sock, mine);
    let refused = server.call_tool_error("swap_pane", json!({ "pane": 2, "dir": "left" }));
    assert!(
        refused.contains("pane 2 was opened by pane 1, not by you"),
        "the refusal names the pane that did open it, in this surface's numbers: {refused}",
    );
}

/// A server that is not inside a pane refuses to open one at all.
///
/// There would be nobody to record as the opener, so the pane could never be closed by this tool —
/// litter from birth. `own_pane` returning `None` is an ordinary situation (an agent outside a
/// pane, or one that outlived it), so it is answered with a sentence rather than left to fail
/// somewhere further in.
#[test]
fn open_pane_refuses_when_the_server_is_not_inside_a_pane() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn(&sock);
    let refused = server.call_tool_error("open_pane", json!({}));
    assert!(
        refused.contains("not running inside a sprag pane"),
        "it says what it could not learn: {refused}",
    );
    assert!(
        server
            .call_tool("list_panes", json!({}))
            .contains("1 pane(s)"),
        "and nothing was opened",
    );

    // The CONTROL: the same daemon, the same tool, from a server that IS in a pane.
    let mut inside = McpServer::spawn_in_pane(&sock, 0);
    assert!(
        inside
            .call_tool("open_pane", json!({}))
            .contains("Opened pane 2"),
        "THE CONTROL: only the pane half of the environment differs",
    );
}

/// `pane_processes` against a REAL daemon: the job that owns a pane's terminal, which is the one
/// fact about a sibling pane that no other tool on this surface can produce.
///
/// The daemon's boot pane runs `cat` and a second pane runs `sleep`, so the two rows differ in the
/// way the whole tool exists for — and both are compared against `list_panes`, which reports what
/// each pane was SPAWNED with. A tool that merely re-read the spawn label would agree with
/// `list_panes` everywhere and this is what would catch it: the process row carries the ARGUMENTS
/// (`sleep 600`), which the label does not have at all.
///
/// The `pane` argument narrows to one pane, and an out-of-range number is refused rather than
/// answered empty — the caller asked about that pane.
#[test]
fn an_agent_reads_what_each_pane_is_running() {
    let (_daemon, sock) = spawn_daemon(&["cat"], (80, 24));
    let sleeper = add_pane(&sock, &["sleep", "600"]);
    let mut server = McpServer::spawn(&sock);
    let numbers = pane_numbers(&mut server);
    let number = |id: u64| {
        numbers
            .iter()
            .find(|(pane, _)| *pane == id)
            .unwrap_or_else(|| panic!("pane {id} is in the list: {numbers:?}"))
            .1
    };

    // Polled: a freshly spawned child takes a moment to become its terminal's foreground group.
    let all = server.wait_for_tool("pane_processes", json!({}), "sleep 600");
    assert!(
        all.starts_with("What each pane is running, sampled "),
        "the answer leads with how old it is: {all}",
    );
    assert!(
        all.contains(&format!("pane {} (id 0) on /dev/", number(0))),
        "each pane is named in this surface's numbers AND the host's ids, with its device: {all}",
    );
    assert!(
        all.contains("cat  cat\n"),
        "the boot pane's job is the cat it runs: {all}",
    );

    // ONE pane, and the argv is the part `list_panes` cannot carry.
    let one = server.call_tool("pane_processes", json!({ "pane": number(sleeper) }));
    assert!(
        one.contains(&format!("pane {} (id {sleeper})", number(sleeper)))
            && one.contains("sleep  sleep 600\n"),
        "the named pane's job carries the arguments it is running: {one}",
    );
    assert!(
        !one.contains(&format!("pane {} (id 0)", number(0))),
        "and narrowing means only that pane: {one}",
    );

    // The pane LIST, from the same server: the label stops at the program name.
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains("sleep") && !listed.contains("sleep 600"),
        "list_panes carries the spawn label, not the command line: {listed}",
    );

    let refused = server.call_tool_error("pane_processes", json!({ "pane": 99 }));
    assert!(
        refused.contains("no pane 99"),
        "a pane this terminal does not have is refused, not answered empty: {refused}",
    );
}

/// **The live gate for `resize_pane`**, and the first time an agent has been able to change how big
/// anything is — against a real daemon and the shipped binary.
///
/// The window is PINNED through the real `sprag` CLI rather than reported by a display client,
/// because a cell has no length until somebody has measured the window and an MCP server is not a
/// display client. That is the same precondition the verb states everywhere else, met the one way a
/// headless test can meet it.
///
/// The ORDER is `an_agent_places_the_pane_it_opened_and_no_other`'s and for its reason: the refusal
/// is checked BEFORE the happy path, so a gate that refused nothing could not pass by having
/// already resized everything. The arrangement is read back through `pane_layout`, a different code
/// path from the tool's own answer, so a tool that reported a move it had not made would not pass on
/// its own wording.
#[test]
fn an_agent_sizes_the_pane_it_opened_and_no_other() {
    let dir = std::env::temp_dir().join(format!(
        "sprag-mcp-resize-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    std::fs::write(
        dir.join("sprag").join("config.toml"),
        "[options]\nwindow-size = \"manual\"\n",
    )
    .expect("write the config");
    let home = dir.display().to_string();
    let (_daemon, sock) = spawn_daemon_with(&["cat"], BOOT_PANE, &[("XDG_CONFIG_HOME", &home)]);

    let cli = PathBuf::from(env!("CARGO_BIN_EXE_sprag-mcp"))
        .parent()
        .expect("the built sprag-mcp has a directory")
        .join("sprag");
    assert!(
        cli.exists(),
        "{} is not built — run `cargo test --workspace`, or `cargo build -p sprag-host --bins`",
        cli.display(),
    );
    let pinned = Command::new(&cli)
        .args(["resize-window", "-t", "0", "-x", "101", "-y", "30"])
        .env(SOCK_ENV, &sock)
        .env("XDG_CONFIG_HOME", &home)
        .output()
        .expect("run the sprag CLI");
    assert!(
        pinned.status.success(),
        "the window is pinned: {}",
        String::from_utf8_lossy(&pinned.stderr),
    );

    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_pane", json!({ "name": "build" }));
    let before = server.call_tool("pane_layout", json!({}));

    // THE PERSON's pane is not the agent's to size — the same fact the close, the rename and the
    // swap are gated on, now with the reason this verb has: how big their panes are is theirs.
    let refused = server.call_tool_error("resize_pane", json!({ "pane": 1, "dir": "right" }));
    assert!(
        refused.contains("pane 1 was opened by a person, not by you")
            && refused.contains("How big their panes are is their arrangement"),
        "it says why, and the reason is the SIZE rather than the placement: {refused}",
    );
    assert_eq!(
        server.call_tool("pane_layout", json!({})),
        before,
        "and the refusal really refused",
    );

    // Its OWN pane, by name. 100 usable columns at an even share puts the boundary at 50; twelve
    // cells left of that is 38.
    let moved = server.call_tool(
        "resize_pane",
        json!({ "pane": "build", "dir": "left", "cells": 12 }),
    );
    assert!(
        moved.contains("boundary 12 cells")
            && moved.contains("gave up exactly that much")
            && moved.contains("read_pane"),
        "it says how far it went and what to do next: {moved}",
    );
    let after = server.call_tool("pane_layout", json!({}));
    assert_ne!(after, before, "the arrangement really moved");
    assert!(
        before.contains("50% left|right") && after.contains("38% left|right"),
        "the DRAWING carries the share, and it is the number this test's own arithmetic \
         predicted: 100 usable columns, an even share at 50, twelve cells left of it is 38. \
         before:\n{before}\nafter:\n{after}",
    );

    // A distance past the wall reports what it ACTUALLY got — the fact no outcome word carries, and
    // the one an agent needs to know not to ask again.
    let clamped = server.call_tool(
        "resize_pane",
        json!({ "pane": "build", "dir": "left", "cells": 500 }),
    );
    assert!(
        clamped.contains("of the 500 asked for") && clamped.contains("no more room that way"),
        "a clamped move says so: {clamped}",
    );

    // The grammar refuses what it has no reading for, HERE, because the daemon can only answer
    // `Rejected` for a malformed request.
    let no_dir = server.call_tool_error("resize_pane", json!({ "pane": "build" }));
    assert!(no_dir.contains("which way the BOUNDARY"), "{no_dir}");
    let zero = server.call_tool_error(
        "resize_pane",
        json!({ "pane": "build", "dir": "left", "cells": 0 }),
    );
    assert!(zero.contains("1 or more"), "{zero}");
}

/// An agent reads a FAR window's ARRANGEMENT by naming a pane in it — and the drawing hands back
/// the handles that reach it, never numbers that would land somewhere else.
///
/// `pane_layout` takes the same `pane` argument every other tool takes rather than a second
/// `window` grammar: an agent that has just been told about a pane one window over
/// (`list_windows`, `wait_for_change`) asks about that window with the vocabulary it already has.
#[test]
fn the_arrangement_of_another_window_is_read_by_naming_a_pane_in_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    mux_invoke(&sock, NEW_WINDOW_ACTION, json!({}));
    let far = mux_query_panes(&sock)
        .first()
        .copied()
        .expect("the new window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "faraway" }),
    );
    // TWO panes over there, so the far drawing has a SPLIT in it — a one-pane window would render
    // identically whichever window was read, and the assertion below would prove nothing.
    mux_invoke(
        &sock,
        SPLIT_ACTION,
        json!({ "pane": far, "dir": "horizontal" }),
    );
    mux_invoke(&sock, SELECT_WINDOW_ACTION, json!({ "window": "0" }));
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // The caller's OWN window first — the control, and it must NOT look like the far one.
    let mine = server.call_tool("pane_layout", json!({}));
    assert!(
        mine.contains("How YOUR WINDOW's panes are arranged") && mine.contains("pane 1 (id 0)"),
        "the unnarrowed drawing is still the caller's own window: {mine}",
    );
    assert!(
        !mine.contains("faraway"),
        "and it does not hold the far pane, or the two drawings could not be told apart: {mine}",
    );

    let over_there = server.call_tool("pane_layout", json!({ "pane": "faraway" }));
    assert!(
        over_there.contains("How WINDOW 1's panes are arranged"),
        "naming a pane draws ITS window: {over_there}",
    );
    assert!(
        over_there.contains("name=\"faraway\""),
        "and the far pane is in the drawing: {over_there}",
    );
    assert!(
        over_there.contains("its panes carry no numbers here; address them by NAME"),
        "which says why there are no numbers rather than leaving a reader to wonder: {over_there}",
    );
    // ⚠ THE POINT. A number in a far window's drawing would be read straight back as `pane: N` and
    // land on a DIFFERENT pane — the positional confusion the whole name grammar exists to remove.
    assert!(
        !over_there.contains("pane 1 (id"),
        "no numbers for a window that is not the caller's: {over_there}",
    );
    assert_ne!(mine, over_there, "two windows, two drawings");
}

/// **A LIVE PANE ONE WINDOW OVER IS NOT "GONE" — a shipped defect, measured, in code R311 did not
/// touch.**
///
/// `pane_processes` and `wait_for_change` both read something REGISTRY- or SESSION-wide and named
/// its rows against ONE window's listing, so every row belonging to another window came out as
/// *"pane ? (id N, gone since the pane list was read)"*. Measured through the shipped server
/// against a real two-window daemon at `e7be5eb`, the answer read:
///
/// ```text
/// pane ? (id 1, gone since the pane list was read) on /dev/pts/13, child process 1208219
///   running (job 1208219): 1208219 bash /bin/bash
/// ```
///
/// — the tty and the pid of a pane it had just called gone, in the same breath. The residual
/// sentence was written for a genuine race (a pane that exits between two reads) and had come to
/// fire for the ordinary case on any multi-window session.
///
/// Driven live rather than only unit-pinned, because the unit fixture is one this round wrote and
/// the failure was in what a real daemon's registry-wide reading looks like from one window.
#[test]
fn a_running_pane_in_another_window_is_not_reported_as_gone() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    mux_invoke(&sock, NEW_WINDOW_ACTION, json!({}));
    let far = mux_query_panes(&sock)
        .first()
        .copied()
        .expect("the new window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "faraway" }),
    );
    mux_invoke(&sock, SELECT_WINDOW_ACTION, json!({ "window": "0" }));
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // The fixture must really be two windows, and the far pane must really be running something.
    let windows = server.call_tool("list_windows", json!({}));
    assert!(
        windows.contains("window 1") && windows.contains("\"faraway\""),
        "the fixture holds a NAMED pane one window over: {windows}",
    );

    let running = server.call_tool("pane_processes", json!({}));
    assert!(
        running.contains(&format!("pane id {far} (window 1)")),
        "a pane one window over is named by WHERE IT IS: {running}",
    );
    assert!(
        !running.contains(&format!("pane ? (id {far}")),
        "and never as gone — it is running, and this answer says so two lines down: {running}",
    );
    // The CONTROL: the caller's OWN pane is still numbered, so the fix did not simply stop
    // numbering everything.
    assert!(
        running.contains("pane 1 (id 0)"),
        "your own window still numbers its panes: {running}",
    );
}

/// **The round's claim end to end: an agent makes itself a place to work, works in it, and THEN
/// shows the person — three acts, and only the third takes their screen.**
///
/// The middle assertion is the whole of it. `open_window` must leave the session exactly where it
/// was: the daemon's `new_window` SELECTS by default (measured at `37d3971`, `current` went
/// `0` → `agentwork`), so a tool that merely wrapped it would take a person's screen every time an
/// agent decided to do some work. The fixture starts on a window that is not the one being created,
/// so "did not move" and "moved" are different strings.
#[test]
fn an_agent_opens_a_window_of_its_own_works_in_it_and_then_shows_the_person() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let booted = mux_current_window(&sock);

    let opened = server.call_tool("open_window", json!({ "name": "agentwork" }));
    assert!(
        opened.contains("Opened window agentwork")
            && opened.contains("The user did NOT move and cannot see it yet")
            && opened.contains("It is yours to close_window and rename_window."),
        "the answer says what happened, what did NOT, and what the window is now for: {opened}",
    );
    assert_eq!(
        mux_current_window(&sock),
        booted,
        "⚠ THE POINT: a window an agent opens does not move the person",
    );
    // And it really was created — otherwise "did not move" would pass on a tool that did nothing.
    let listed = server.call_tool("list_windows", json!({}));
    assert!(
        listed.contains("window agentwork") && listed.contains("(current, you are here)"),
        "the window exists and the agent is still in its own: {listed}",
    );

    // It WORKS in there, across the window, by the name it gave the pane (R311/R312).
    let far = mux_query_panes_in(&sock, "agentwork")
        .first()
        .copied()
        .expect("the new window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "build" }),
    );
    server.call_tool(
        "write_pane",
        json!({ "pane": "build", "text": "echo R313-DONE" }),
    );
    let mut screen = String::new();
    for _ in 0..200 {
        screen = server.call_tool("read_pane", json!({ "pane": "build" }));
        if screen.contains("R313-DONE") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        screen.contains("R313-DONE"),
        "the agent drove a pane of its OWN window from its own: {screen:?}",
    );

    // THEN it shows the person, and that is the act that moves them.
    let shown = server.call_tool("select_window", json!({ "window": "agentwork" }));
    assert!(
        shown.contains("The user is now looking at window agentwork")
            && shown.contains("this is their whole screen, not a pane of it"),
        "the answer says what it took: {shown}",
    );
    assert_eq!(
        mux_current_window(&sock),
        "agentwork",
        "and the session really moved — which is what makes the earlier assertion mean something",
    );
}

/// A window a PERSON made is refused by both destructive window verbs, and a window the agent made
/// is accepted by both — R294's authorship gate one level up.
///
/// Both halves in one test with the SAME two verbs, because the assertion that matters is the
/// DIFFERENCE: a gate that refused everything would pass a refusal-only test.
#[test]
fn an_agent_closes_and_renames_only_the_windows_it_opened() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let theirs = mux_current_window(&sock);
    server.call_tool("open_window", json!({ "name": "mine" }));

    for verb in ["close_window", "rename_window"] {
        // Each verb driven with the arguments IT takes. One shared `{window, name}` was only ever
        // possible because `close_window` swallowed the `name` it does not declare — the defect
        // [`every_tool_that_publishes_a_closed_argument_set_enforces_it`](self) closed.
        let mut args = json!({ "window": theirs.clone() });
        if verb == "rename_window" {
            args["name"] = json!("whatever");
        }
        let refused = server.call_tool_error(verb, args);
        assert!(
            refused.contains(&format!(
                "window {theirs} was opened by a person, not by you"
            )) && refused.contains("Only a window you opened yourself with open_window is yours."),
            "{verb} refuses a person's window and says why: {refused}",
        );
    }
    // The window is still there — a refusal that had already acted would be worse than none.
    assert!(
        server
            .call_tool("list_windows", json!({}))
            .contains(&format!("window {theirs}")),
        "the refused window is untouched",
    );

    let renamed = server.call_tool("rename_window", json!({ "window": "mine", "name": "ours" }));
    assert!(
        renamed.contains("Window mine is now called \"ours\"")
            && renamed.contains("That is its ADDRESS too"),
        "its own window renames, and the answer says the name is the handle: {renamed}",
    );
    let closed = server.call_tool("close_window", json!({ "window": "ours" }));
    assert!(
        closed.contains("Closed window ours, which you had opened, and every pane in it"),
        "and closes: {closed}",
    );
    let left = server.call_tool("list_windows", json!({}));
    assert!(
        !left.contains("window ours") && left.contains(&format!("window {theirs}")),
        "the agent's window went and the person's stayed: {left}",
    );
}

/// **An agent can force the SHAPE of a window it opened, and is TOLD when the pin does nothing**
/// (R331).
///
/// # Why this tool exists at all
///
/// A pane's columns are what `read_pane` sees, and a window's size is what decides them. Before
/// this the agent surface could widen a pane's SHARE of a window (`resize_pane`) and could not
/// change the window — so an agent reading a wide table in a window a person had sized small had no
/// move to make.
///
/// # The three claims, and the third is the one an agent cannot check for itself
///
/// * a person's window is REFUSED, `rename_window`'s gate one verb over;
/// * its own window takes the rectangle, and the panes are re-tiled to it — read from the DAEMON's
///   pane list, not from the tool's own sentence;
/// * with `window-size` NOT `manual` the daemon stores the size and lays nothing out over it, and
///   the answer SAYS SO. An agent has no screen; a tool that reported "resized" there would have it
///   act on columns that do not exist. This is the half a code reading cannot check — the sentence
///   comes from the daemon's own policy, so the fixture flips the option through the real verb.
#[test]
fn an_agent_resizes_only_its_own_window_and_is_told_when_the_pin_is_inert() {
    // The DAEMON's own config home: `window-size` is read by the daemon, and the third claim below
    // is about what IT arbitrates under. A test that pointed only this process at a file would be
    // asserting against whatever the developer's own config says (R331).
    let dir = std::env::temp_dir().join(format!(
        "sprag-mcp-winsize-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    std::fs::write(
        dir.join("sprag").join("config.toml"),
        "[options]\nwindow-size = \"manual\"\n",
    )
    .expect("write the config");
    let home = dir.display().to_string();
    let (_daemon, sock) = spawn_daemon_with(&["cat"], BOOT_PANE, &[("XDG_CONFIG_HOME", &home)]);
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_sprag-mcp"))
        .parent()
        .expect("the built sprag-mcp has a directory")
        .join("sprag");
    let sprag = |args: &[&str]| -> String {
        let out = Command::new(&cli)
            .args(args)
            .env(SOCK_ENV, &sock)
            .env("XDG_CONFIG_HOME", &home)
            .output()
            .expect("run the sprag CLI");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let theirs = mux_current_window(&sock);
    server.call_tool("open_window", json!({ "name": "mine" }));

    let refused = server.call_tool_error(
        "resize_window",
        json!({ "window": theirs.clone(), "cols": 100, "rows": 30 }),
    );
    assert!(
        refused.contains(&format!(
            "window {theirs} was opened by a person, not by you"
        )),
        "a person's window is not an agent's to reshape: {refused}",
    );

    // HALF a rectangle, refused where it was typed and naming which argument to fix.
    let half = server.call_tool_error("resize_window", json!({ "window": "mine", "cols": 100 }));
    assert!(
        half.contains("'cols' AND 'rows' together"),
        "half a rectangle is a size nobody chose: {half}",
    );

    let pinned = server.call_tool(
        "resize_window",
        json!({ "window": "mine", "cols": 100, "rows": 30 }),
    );
    assert!(
        pinned.contains("Window mine is pinned to 100x30 cells")
            && pinned.contains("read_pane now sees those columns"),
        "its own window takes the rectangle: {pinned}",
    );
    // THE DAEMON's account, not the tool's: the panes of the agent's own window were really
    // re-tiled. Read through the CLI's window-scoped listing, because the agent's window is not the
    // session's current one.
    // The agent's window is not the session's current one, so it is SELECTED first — `panes` reads
    // the current window, and asserting on the person's would say nothing about the pin.
    let _ = sprag(&["select-window", "-t", "0", "mine"]);
    let listed = sprag(&["panes", "-t", "0"]);
    assert!(
        listed.contains("100x30"),
        "the pin reached the agent's own panes: {listed:?}",
    );

    // ...and the same call under a policy that reads no pin. `set-option` edits the one file the
    // daemon reads, so nothing is restarted and the answer changes because the DAEMON's rule did.
    let _ = sprag(&["set-option", "window-size", "largest"]);
    let inert = server.call_tool(
        "resize_window",
        json!({ "window": "mine", "cols": 120, "rows": 40 }),
    );
    assert!(
        inert.contains("Nothing moved:") && inert.contains("window-size is largest"),
        "a stored-but-inert pin must not read as a resize: {inert}",
    );
    assert!(
        inert.contains("120x40"),
        "and it names the rectangle it stored, which the agent can see nowhere else: {inert}",
    );

    let released = server.call_tool("resize_window", json!({ "window": "mine" }));
    assert!(
        released.contains("un-pinned and follows the clients watching it again"),
        "no rectangle at all is the un-pin: {released}",
    );
}

/// **An agent tidying up its own workbench cannot end a person's SESSION.**
///
/// R309 made a kill cascade: a session's last window ends the session, and the last session ends
/// the daemon. So `close_window` refuses when the window is the session's only one — even though
/// the agent opened it, and even though every other rule says it may.
///
/// ⚠ **The state has to be BUILT, and the first version of this measurement could not fail**: it
/// asked the agent to close its own window while the person's window was still there, which is
/// simply a legal close. The person's window is killed out of band here so the agent's really is
/// the last one.
#[test]
fn an_agent_cannot_close_the_last_window_of_a_session() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let theirs = mux_current_window(&sock);
    server.call_tool("open_window", json!({ "name": "solo" }));
    // Out of band, as a person would. The agent's own pane goes with it, which is why this state is
    // unusual — and exactly why the guard must still hold when it arises.
    mux_invoke(&sock, KILL_WINDOW_ACTION, json!({ "window": theirs }));

    let refused = server.call_tool_error("close_window", json!({ "window": "solo" }));
    assert!(
        refused.contains("window solo is this session's only window")
            && refused.contains("would end the SESSION")
            && refused.contains("close_window will not do that"),
        "it refuses, and the sentence says what it was protecting: {refused}",
    );
    assert!(
        server
            .call_tool("list_windows", json!({}))
            .contains("window solo"),
        "and the window is still there — a refusal that had already acted is worse than none",
    );
}

/// **THE WINDOW RATCHET: every tool the roster declares a `window` argument for is safe to point at
/// a PERSON's window — and the list is DERIVED FROM THE ROSTER, not written here.**
///
/// R312 built the same shape for the pane address and a revert-proof then showed its first version
/// was too weak. This is its window-level twin, and the property is different: a window verb aimed
/// at a person's window must either be HARMLESS (a read, a select — moving somebody is allowed and
/// `select_pane` has always been) or refuse on AUTHORSHIP. What must not happen is a destructive
/// verb going through, which is what `close_window` and `rename_window` would do without
/// `Window::opened_by` — and what herdr's `tab.close` does today (`app/api/tabs.rs:225` gates only
/// on a worktree-group confirmation, read at `9a4ce5e1`).
///
/// A tool added later with a `window` argument is checked the day it appears.
#[test]
fn every_window_tool_is_safe_to_point_at_a_persons_window() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let theirs = mux_current_window(&sock);
    // A SECOND window the agent owns, so the fixture can tell "refuses everything" from "refuses a
    // person's" — the discrimination the whole gate is about.
    server.call_tool("open_window", json!({ "name": "mine" }));

    let sample = |argument: &str| -> Option<Value> {
        Some(match argument {
            "window" => json!(theirs),
            "name" => json!("renamed-by-an-agent"),
            "relative" => json!("next"),
            // `join_pane` takes a window AND a pane, so it enters this sweep too — and the pane it
            // is handed is the BOOT pane, which a person opened. Both halves of the gate are
            // therefore under test at once: a person's window as the destination, a person's pane
            // as the subject.
            "pane" => json!(1),
            _ => return None,
        })
    };

    let roster = server.request("tools/list", json!({}));
    let mut checked: Vec<String> = Vec::new();
    for tool in roster["result"]["tools"].as_array().expect("a roster") {
        let name = tool["name"]
            .as_str()
            .expect("every tool is named")
            .to_owned();
        if tool["inputSchema"]["properties"].get("window").is_none() {
            continue;
        }
        let mut args = serde_json::Map::new();
        args.insert("window".to_owned(), json!(theirs));
        for required in tool["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let argument = required.as_str().expect("a required argument is named");
            let value = sample(argument).unwrap_or_else(|| {
                panic!(
                    "{name} requires an argument this ratchet has no sample for ({argument:?}). \
                     Add one — a skipped tool is a tool this test believes it covered."
                )
            });
            args.insert(argument.to_owned(), value);
        }
        let answer = server.call_tool_raw(&name, Value::Object(args));
        let errored = answer["result"]["isError"] == json!(true);
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            !errored || text.contains("was opened by a person, not by you"),
            "{name} aimed at a person's window neither succeeded harmlessly nor refused on \
             AUTHORSHIP: {text}",
        );
        checked.push(name);
    }

    // ⚠ AND THE WINDOW IS STILL THERE. Without this the assertion above passes on a tool that
    // destroyed it and said so cheerfully — which is exactly the failure the gate exists for.
    let left = server.call_tool("list_windows", json!({}));
    assert!(
        left.contains(&format!("window {theirs}")),
        "the person's window survived every verb pointed at it: {left}",
    );

    for wanted in ["select_window", "close_window", "rename_window"] {
        assert!(
            checked.iter().any(|name| name == wanted),
            "the roster no longer declares a `window` argument for {wanted}: {checked:?}",
        );
    }
}

/// **THE PANES OF A WINDOW AN AGENT OPENED ARE ITS OWN — and until R313's own audit they were
/// nobody's, so the agent was told they belonged to a PERSON.**
///
/// Measured through the shipped server: an agent opened a window, and `rename_pane`, `close_pane`
/// and `resize_pane` on the birth pane inside it all answered *"was opened by a PERSON, not by
/// you"* — false, since the agent's own request had just created it — while `close_window`
/// destroyed that same pane without a murmur. The daemon passed `None` for the birth pane's opener
/// on a comment reasoning from `new_session`, and that premise moved in the round that let a
/// caller which is not a person make a window.
///
/// The CONTROL is in the same test: a pane of a PERSON's window is still refused, so this is not
/// "the gate stopped working".
#[test]
fn the_panes_of_a_window_an_agent_opened_are_the_agents_too() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_window", json!({ "name": "agentwork" }));
    let far = mux_query_panes_in(&sock, "agentwork")
        .first()
        .copied()
        .expect("the window's birth pane");
    // Named out of band so the test can address it, since a window's birth pane is deliberately
    // unnamed (the request's `name` is the WINDOW's).
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "inmine" }),
    );

    let renamed = server.call_tool("rename_pane", json!({ "pane": "inmine", "name": "built" }));
    assert!(
        renamed.contains("is now called \"built\""),
        "the pane of the agent's OWN window is the agent's to name: {renamed}",
    );
    // THE CONTROL — a pane of the PERSON's window is still refused, so the gate still gates.
    let theirs = server.call_tool_error("rename_pane", json!({ "pane": 1, "name": "nope" }));
    assert!(
        theirs.contains("was opened by a person, not by you"),
        "and a person's pane is still theirs: {theirs}",
    );

    let closed = server.call_tool("close_pane", json!({ "pane": "built" }));
    assert!(
        closed.contains("which you had opened"),
        "and it is the agent's to close: {closed}",
    );
}

/// `open_window` starts its shell WHERE THE AGENT SAYS — `open_pane`'s `cwd`, which this tool did
/// not take at all until R313's audit noticed the asymmetry was an artifact rather than a decision.
#[test]
fn a_window_an_agent_opens_starts_where_it_asks() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    // A directory that is NOT the server's own, or the assertion could not tell "honoured the
    // argument" from "inherited the default".
    let elsewhere = std::env::temp_dir();
    assert_ne!(
        std::env::current_dir().expect("a cwd"),
        elsewhere,
        "the fixture's directory must differ from the default or nothing below discriminates",
    );
    server.call_tool(
        "open_window",
        json!({ "name": "elsewhere", "cwd": elsewhere.to_str().expect("a utf-8 temp dir") }),
    );
    let far = mux_query_panes_in(&sock, "elsewhere")
        .first()
        .copied()
        .expect("the window's birth pane");
    mux_invoke(
        &sock,
        RENAME_PANE_ACTION,
        json!({ "pane": far, "name": "there" }),
    );

    // What `pwd` PRINTS is the resolved path, and macOS's `TMPDIR` is a symlink
    // (`/var/folders/…` → `/private/var/folders/…`). Comparing against the path the fixture handed
    // over compares two spellings of one directory.
    let wanted = elsewhere
        .canonicalize()
        .expect("the temp dir resolves")
        .to_str()
        .expect("a utf-8 temp dir")
        .to_owned();
    // ⚠ ASKED THROUGH `find_in_pane` RATHER THAN GREPPED OFF `read_pane` (R344). The pane is
    // narrow and that path is 55 characters on macOS, so what a person reads on one line arrives
    // on the screen across two — plus `/bin/bash` there prints Apple's *"the default interactive
    // shell is now zsh"* banner into the same screen. This used to strip every newline out of the
    // rendered text, which is blunter than the emulator's own wrap flag and welds unrelated lines
    // together; the search walks LOGICAL lines now, so the question "did this pane print that
    // path" has a product answer and no caller has to fold around anything.
    //
    // ⚠ A MISS ECHOES THE NEEDLE, so `contains(wanted)` alone would pass on `no matches for
    // "/private/var/…"`. Both halves are asserted for that reason.
    server.call_tool("write_pane", json!({ "pane": "there", "text": "pwd" }));
    let mut found = String::new();
    for _ in 0..200 {
        found = server.call_tool(
            "find_in_pane",
            json!({ "pane": "there", "needle": wanted.clone() }),
        );
        if !found.contains("no matches") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !found.contains("no matches") && found.contains(&wanted),
        "the shell started where the agent asked ({wanted}): {found:?}",
    );
    // A directory that is not one is refused BEFORE anything is created, naming the path.
    let refused = server.call_tool_error("open_window", json!({ "cwd": "/no/such/place/at/all" }));
    assert!(
        refused.contains("/no/such/place/at/all") && refused.contains("is not a directory"),
        "and a bad path is a sentence naming it: {refused}",
    );
}

/// **An agent is told when its message reached NOBODY** — the answer that must not read like
/// success, driven against a real daemon through the shipped server.
///
/// An MCP server is not a display client: it connects to the daemon and attaches to nothing, so
/// this fixture IS the "no window is attached" case rather than a simulation of it. That makes the
/// claim exact: an agent calling `display_message` on a terminal nobody is watching is told so, in
/// words that tell it what to do instead.
///
/// The CONTROL is the refusal below it: a message the grammar rejects comes back as an ERROR, so
/// the empty delivery above is a SUCCESS carrying bad news rather than the tool failing for
/// everything. Two different outcomes, two different shapes — which is the distinction R301 spent a
/// round on and this verb inherits.
#[test]
fn a_message_that_reached_nobody_says_so_rather_than_reporting_success() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    let answer = server.call_tool_raw(
        "display_message",
        json!({ "message": "the build finished", "severity": "alert" }),
    );
    assert_ne!(
        answer["result"]["isError"],
        json!(true),
        "an empty audience is an ANSWER, not a tool failure: {answer}",
    );
    let text = tool_text(&answer["result"]);
    assert!(
        text.contains("NOBODY SAW IT"),
        "the agent must be told plainly, not congratulated: {text:?}",
    );
    assert!(
        text.contains("Do not treat it as delivered"),
        "...and told what to do instead: {text:?}",
    );

    // THE CONTROL: a message the grammar refuses is an ERROR, and it names the rule rather than
    // being truncated into acceptability the way the rival's is.
    let refused = server.call_tool_error("display_message", json!({ "message": "two\nrows" }));
    assert!(
        refused.contains("control characters"),
        "a refusal names the rule it broke: {refused}",
    );
    let refused = server.call_tool_error(
        "display_message",
        json!({ "message": "fine", "severity": "shout" }),
    );
    assert!(
        refused.contains("note|warn|alert") && refused.contains("shout"),
        "an unknown severity names what was offered and what exists: {refused}",
    );
}

/// An agent's tools answer about the session the agent's PANE is in, not about the daemon's
/// default one.
///
/// # Why this front needs its own drive
///
/// The `sprag` CLI had this defect and was fixed at its params builder; this server is a second
/// client with its own, and *"a claim must be driven at every front that has one"*. It matters more
/// here than there: a person typing `sprag` at a shell can see which session they are in, and an
/// agent cannot — running inside a pane is the only thing it knows about its own position, and
/// every tool below reads the workspace on its behalf.
///
/// The fixture puts the agent's pane in a session the daemon would NOT pick, which is what makes
/// each answer attributable: a server that ignored its own pane would answer about `0` throughout.
#[test]
fn an_agents_tools_answer_about_the_session_its_pane_is_in() {
    let (_daemon, sock) = spawn_daemon(&["cat"], (80, 24));
    // A second session, with two panes of its own. Over the wire, like every other fixture here.
    let created = mux_invoke(&sock, NEW_SESSION_ACTION, json!({ "name": "work" }));
    assert_eq!(created.as_str(), Some("work"), "the second session exists");
    let mine = spawn_pane_in(&sock, "work");
    let sibling = spawn_pane_in(&sock, "work");

    let mut server = McpServer::spawn_in_pane(&sock, mine);
    // Asserted on `id=`, never on the row number: a row is numbered by POSITION in this listing, so
    // `pane 2:` names the second row of whatever session came back — which is exactly the string a
    // wrong answer would also contain. (Written the other way first, and the control below is what
    // said so.)
    let listed = server.call_tool("list_panes", json!({}));
    assert!(
        listed.contains(&format!("id={mine} ")) && listed.contains(&format!("id={sibling} ")),
        "the agent is listed its OWN session's panes ({mine}, {sibling}): {listed}",
    );
    assert!(
        !listed.contains("id=0 "),
        "and not the default session's, which holds the daemon's boot pane: {listed}",
    );

    // THE CONTROL — a server that is in NO pane still gets the daemon's default session, which is
    // the behaviour every caller outside the workspace has. Without it this test would also pass
    // against a server that had simply stopped reading the default.
    let mut outside = McpServer::spawn(&sock);
    let theirs = outside.call_tool("list_panes", json!({}));
    assert!(
        theirs.contains("id=0 ") && !theirs.contains(&format!("id={sibling} ")),
        "a caller in no pane is answered about the daemon's default session: {theirs}",
    );
}

/// Spawn a pane INSIDE a named session — the scope is a sibling of `path`, never a member of
/// `args`, which is the daemon's own grammar (`sprag_host::wire::SESSION_PARAM`).
///
/// Written after the daemon refused the other spelling with `InvokeTypeMismatch`: a session name
/// smuggled into an action's arguments is an unknown argument, and it says so rather than guessing.
fn spawn_pane_in(sock: &Path, session: &str) -> u64 {
    let mut conn = HostConn::connect(sock, DEADLINE).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": mux_action_path(SPAWN_ACTION),
            "args": { "cmd": ["cat"] },
        }),
    )
    .expect("spawn a pane in the named session")
    .as_u64()
    .expect("the spawn action answers with a pane id")
}

/// A daemon OLDER than this build: it serves no address and knows no action — [`sprag_peer`]'s,
/// since R324, where this file had written one out for itself.
fn old_host() -> sprag_peer::OldDaemon {
    sprag_peer::OldDaemon::serving_nothing_or_acting(&socket_path())
}

/// An agent asking a daemon that predates its tools is told the daemon is OLD, not that its
/// arguments are wrong.
///
/// ⚠ **IT ENUMERATES THE ROSTER, since R335's debt sweep.** It was a hand-written list of
/// `(tool, args)` pairs — so the two roster ratchets in this file found R335's four new tools by
/// construction and this one silently did not cover them, which is the failure mode a ratchet
/// exists to prevent, in a ratchet. It now walks `tools/list` and **fails closed** on a tool it has
/// no entry for, exactly as `the_whole_roster_reaches_a_pane_one_window_over` does.
///
/// A tool is either DRIVEN (it reaches the wire, so it can leak a Rust variant name) or listed as
/// `SKEW_EXEMPT` with the reason it cannot — every exemption is a sentence somebody wrote, not an
/// absence. **`SKEW_EXEMPT` is currently empty**: all 34 tools reach the wire and all 34 name the
/// skew, where the hand-written list checked 12. R338's `pane_resources` was caught by it on the
/// round it was added, which is the enumeration paying for itself a second time.
///
/// ⚠⚠ **THE ENUMERATION FOUND A LIVE DEFECT ON ITS FIRST RUN.** `wait_for_change` answered *"the
/// host did not report a scene revision"* — a fact with no cause and no remedy, which is debt item
/// 9's class, reached through the one path 21 unchecked tools had been hiding. Its `scene/revision`
/// read SUCCEEDS against an old daemon and carries no `revision` key, so the `?` never fires and
/// that sentence was the whole answer an agent got. Fixed at the source rather than exempted.
#[test]
fn a_tool_against_an_older_daemon_says_so() {
    let host = old_host();
    // IN a pane, so the tools that need a caller identity reach the wire instead of stopping at
    // their own pre-flight. The peer serves nothing, so working out which session that pane is in
    // fails too — silently, which is what leaves each sentence below attributable to its own tool.
    let mut server = McpServer::spawn_in_pane(host.sock(), 1);

    // What each tool is DRIVEN with. A tool absent from both this and `SKEW_EXEMPT` fails the run.
    let driven = |tool: &str| -> Option<Value> {
        Some(match tool {
            "list_panes" | "list_windows" | "list_sessions" | "pane_layout" => json!({}),
            // Takes nothing at all: a machine is not divided by session. It still reaches the
            // wire, which is the whole claim this ratchet makes about a tool.
            "machine_health" => json!({}),
            "read_pane" | "read_last_command" | "read_pane_links" | "read_pane_images"
            | "pane_processes" | "pane_resources" | "agent_state" | "agent_explain"
            | "select_pane" | "break_pane" | "zoom_pane" | "close_pane" | "rename_pane"
            | "stop_job" => {
                json!({ "pane": 1 })
            }
            "send_keys" => json!({ "pane": 1, "keys": ["Enter"] }),
            "write_pane" => json!({ "pane": 1, "text": "x" }),
            // Nothing at all: it takes a `name` and a `cwd` and neither is needed to reach the
            // wire. It was driven with `dir` — `resize_pane`'s argument, which `open_pane` has
            // never had — for as long as this table has existed, and the tool ran anyway with it
            // dropped. What made that possible is the defect
            // [`every_tool_that_publishes_a_closed_argument_set_enforces_it`](self) closed.
            "open_pane" => json!({}),
            "display_message" => json!({ "message": "hi" }),
            "find_in_pane" => json!({ "pane": 1, "needle": "x" }),
            "regex_in_pane" => json!({ "pane": 1, "pattern": "x" }),
            "swap_pane" => json!({ "pane": 1, "with": 1 }),
            "grant_pane" => json!({ "pane": 1, "share": 100 }),
            "resize_pane" => json!({ "pane": 1, "dir": "left" }),
            "join_pane" => json!({ "pane": 1, "window": "0" }),
            "move_pane" => json!({ "pane": 1, "target": 1, "dir": "left" }),
            "wait_for_output" => json!({ "pane": 1, "needle": "x", "timeout_seconds": 1 }),
            "wait_for_change" => json!({ "timeout_seconds": 1 }),
            "open_window" => json!({}),
            "select_window" => json!({ "window": "0" }),
            "close_window" => json!({ "window": "0" }),
            "rename_window" => json!({ "window": "0", "name": "x" }),
            "resize_window" => json!({ "window": "0" }),
            // The three that reach the LOOP. `orchestrate` reads this daemon's guardrail defaults
            // before it does anything else — a tool that could not learn the ceiling must not
            // invent one — so an old daemon is met on that read; the other two read the run list.
            "orchestrate" => json!({ "plugin": "orchestrator", "pane": 1, "stimulus": "x" }),
            "list_runs" => json!({}),
            "cancel_run" => json!({ "run": 0 }),
            // ⚠ R369. It resolves the pane through this surface's own addressing before it builds
            // a call, so an old daemon is met on THAT read — which is the same door `orchestrate`
            // meets it at, and the reason neither needs an exemption.
            "answer_pane" => json!({ "pane": 1, "asked": "x", "answer": "x" }),
            _ => return None,
        })
    };
    // The tools whose refusal would come from somewhere OTHER than the wire, each with the reason.
    // An exemption is a sentence somebody wrote; an absence is what this ratchet refuses to allow.
    //
    // ⚠⚠ **IT IS EMPTY, AND IT IS EMPTY BECAUSE EVERY GUESS AT IT WAS WRONG.** R335 first wrote
    // seven exemptions from reading the code — the five window tools *"resolve a name before the
    // wire"*, the two waiting tools *"park"* — and driving them refuted six outright: they all say
    // the daemon is old. The seventh, `wait_for_change`, did not — and that was a live defect, not
    // an exemption (see below). **An exemption written from reading is a guess; drive it.**
    const SKEW_EXEMPT: &[(&str, &str)] = &[];

    let roster = server.request("tools/list", json!({}));
    let tools: Vec<String> = roster["result"]["tools"]
        .as_array()
        .expect("the roster is a list")
        .iter()
        .map(|tool| tool["name"].as_str().expect("named").to_owned())
        .collect();
    let mut wrong = Vec::new();
    let mut checked = 0_usize;
    for tool in &tools {
        if let Some((_, why)) = SKEW_EXEMPT.iter().find(|(name, _)| name == tool) {
            assert!(!why.is_empty());
            continue;
        }
        let args = driven(tool).unwrap_or_else(|| {
            panic!(
                "{tool} is advertised and this skew ratchet neither drives it nor exempts it. Add                  it to `driven`, or to SKEW_EXEMPT with the reason its refusal does not come from                  the wire — a skipped tool is a tool this test believes it covered."
            )
        });
        checked += 1;
        let said = server.call_tool_error(tool, args);
        if said.contains("UnknownIntrospectPath") || said.contains("UnknownInvokePath") {
            wrong.push(format!("{tool} printed a Rust variant name: {said}"));
        } else if !said.contains("older than this") {
            wrong.push(format!("{tool} did not say the daemon is old: {said}"));
        }
    }
    assert!(
        checked >= tools.len() - SKEW_EXEMPT.len(),
        "the ratchet walked {checked} of {} tools",
        tools.len(),
    );
    assert!(
        wrong.is_empty(),
        "an agent must be told its daemon predates the tool:\n  {}",
        wrong.join("\n  "),
    );
}

/// **AN AGENT CAN TAKE THE PANE IT OPENED OUT OF SOMEBODY'S WINDOW** — item 56a's first half, and
/// the one that needed the daemon to change.
///
/// Measured at `9727042`: the agent surface served `open_pane` / `close_pane` / `swap_pane` /
/// `resize_pane` and no `break_pane` at all, so an agent whose work had outgrown a pane beside a
/// person could only close it. The register priced that as a missing tool. It was more than one:
/// wrapping the wire action would have taken the person's whole screen (the break SELECTED what it
/// made) and left the agent unable to close the window afterwards (the break recorded no opener),
/// which is why `BREAK_PANE_ACTION` now takes a `WindowBirth` and `WIRE_PROTOCOL` moved.
///
/// Three claims, in the order they fail differently:
///
/// * the pane MOVED and kept what was in it (a break is not a re-spawn);
/// * the PERSON did not move — the sentence and the daemon's own current window agree;
/// * the window is the agent's, PROVED by closing it rather than by reading a listing.
///
/// The CONTROL is first: a pane the person opened is refused, so a gate that refused nothing could
/// not reach the happy path with everything already broken out.
#[test]
fn an_agent_breaks_its_own_pane_out_without_moving_the_person() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let home = mux_current_window(&sock);

    // THE CONTROL. The boot pane is the person's, and the refusal must name the rule.
    let refused = server.call_tool_error("break_pane", json!({ "pane": 1 }));
    assert!(
        refused.contains("was opened by a person, not by you")
            && refused.contains("Where their panes sit is their arrangement"),
        "a person's pane is refused, in a sentence that says why: {refused}",
    );
    assert_eq!(
        mux_current_window(&sock),
        home,
        "and the refusal really refused — the person is where they were",
    );

    server.call_tool("open_pane", json!({ "name": "buildout" }));
    // Something IN the pane, so "it kept its contents" is a claim about bytes rather than about a
    // pane id. `write_pane` types it at the shell; the echo is what read_pane will find.
    server.call_tool(
        "write_pane",
        json!({ "pane": "buildout", "text": "echo brokenoutproof" }),
    );
    // ⚠⚠⚠⚠⚠ AND THE FIXTURE WAITS FOR THE ECHO THROUGH THE PRODUCT'S OWN DOOR, rather than reading
    // once and hoping. `write_pane` returns when the keystrokes are IN; the pane's echo is the
    // shell's own work and arrives on the shell's schedule, so a `read_pane` on the next line is a
    // race — measured, as a whole-workspace run where this exact assertion fired with `before`
    // EMPTY while nothing was wrong with the product at all.
    //
    // ⚠⚠⚠ IT IS `wait_for_output` AND NOT A SLEEP OR A POLL WRITTEN HERE, for this suite's own
    // reason: the tool exists because *read the pane now* cannot answer *has it printed yet*, and a
    // fixture that solved the same problem privately would be a second answer to it — one no
    // product change can keep in step, and one that hides the door being broken.
    server.call_tool(
        "wait_for_output",
        json!({ "pane": "buildout", "needle": "brokenoutproof", "timeout_seconds": 20 }),
    );
    let before = server.call_tool("read_pane", json!({ "pane": "buildout" }));
    assert!(
        before.contains("brokenoutproof"),
        "the fixture must put something in the pane or the claim below is vacuous: {before}",
    );

    let broken = server.call_tool(
        "break_pane",
        json!({ "pane": "buildout", "name": "mywork" }),
    );
    assert!(
        broken.contains("into a window of its own, called mywork")
            && broken.contains("The user did NOT move and cannot see it"),
        "the answer names the window and says the person stayed: {broken}",
    );
    assert!(
        broken.contains("It is yours to close_window and rename_window."),
        "and says the window is the agent's — which is the half the daemon had to learn: {broken}",
    );
    // THE SENTENCE IS A CLAIM ABOUT THE DAEMON, so the daemon is asked. A tool that printed this
    // and selected the window anyway would pass on the text alone.
    assert_eq!(
        mux_current_window(&sock),
        home,
        "the break was detached: the session is still on the window it was on",
    );
    assert!(
        server
            .call_tool("list_panes", json!({}))
            .contains("1 pane(s)"),
        "and the pane really left the person's window",
    );

    let after = server.call_tool("read_pane", json!({ "pane": "buildout" }));
    assert!(
        after.contains("brokenoutproof"),
        "the pane was MOVED, not re-spawned — its scrollback rode along: {after}",
    );

    // THE PROVENANCE, proved by ACTING on it rather than by reading a line about it.
    let closed = server.call_tool("close_window", json!({ "window": "mywork" }));
    assert!(
        closed.contains("mywork"),
        "the agent closes the window its own break created: {closed}",
    );
    assert!(
        !server
            .call_tool("list_windows", json!({}))
            .contains("mywork"),
        "and it is gone",
    );
}

/// **AN AGENT CAN PUT A PANE BACK, AND SAY WHERE IT LANDS** — item 56a's other half.
///
/// `join_pane` appends into a window; `move_pane` names a SIDE of a particular pane. Both are
/// driven here against a real daemon, and both are checked against `pane_layout` — the arrangement
/// as the daemon draws it — rather than against their own sentences, because a tool that answered
/// correctly and sent nothing would pass on prose.
///
/// The `dir` claim is the one worth the fixture: this surface takes the same four compass words at
/// every tool, and the wire takes tmux's axis-plus-`before` pair. `left` and `right` are therefore
/// the discriminating pair — an implementation that dropped the `before` half would put the pane on
/// the wrong side and answer identically.
#[test]
fn an_agent_joins_and_places_only_the_panes_it_opened() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let home = mux_current_window(&sock);
    server.call_tool("open_window", json!({ "name": "bench" }));
    server.call_tool("open_pane", json!({ "name": "worker" }));

    // THE CONTROLS, both gates, before anything moves.
    let refused = server.call_tool_error("join_pane", json!({ "pane": 1, "window": "bench" }));
    assert!(
        refused.contains("was opened by a person, not by you"),
        "a person's pane cannot be joined away: {refused}",
    );
    let refused = server.call_tool_error(
        "move_pane",
        json!({ "pane": 1, "target": "worker", "dir": "left" }),
    );
    assert!(
        refused.contains("was opened by a person, not by you"),
        "nor placed: {refused}",
    );

    let joined = server.call_tool("join_pane", json!({ "pane": "worker", "window": "bench" }));
    assert!(
        joined.contains("into window bench"),
        "the join names where it went: {joined}",
    );
    assert_eq!(
        mux_current_window(&sock),
        home,
        "a join moves a pane, never the person",
    );
    assert!(
        server
            .call_tool("list_panes", json!({}))
            .contains("1 pane(s)"),
        "and the agent's pane left the person's window",
    );

    // A SECOND pane of the agent's, joined into the same window, so a placement has something to
    // land beside that the gate will not refuse.
    server.call_tool("open_pane", json!({ "name": "helper" }));
    server.call_tool("join_pane", json!({ "pane": "helper", "window": "bench" }));

    // LEFT and RIGHT of the SAME target, read back off the daemon's own drawing. This is the pair
    // that discriminates: an implementation that carried the axis and dropped the side would answer
    // both of these identically and put the pane on one side twice.
    let left = server.call_tool(
        "move_pane",
        json!({ "pane": "helper", "target": "worker", "dir": "left" }),
    );
    assert!(
        left.contains("to the left of"),
        "the answer says which side it landed on: {left}",
    );
    let drawn_left = server.call_tool("pane_layout", json!({ "pane": "worker" }));

    let right = server.call_tool(
        "move_pane",
        json!({ "pane": "helper", "target": "worker", "dir": "right" }),
    );
    assert!(
        right.contains("to the right of"),
        "and the other side is a different sentence: {right}",
    );
    let drawn_right = server.call_tool("pane_layout", json!({ "pane": "worker" }));

    // The daemon draws its neighbour table by ID, so the claim is spelled in ids: after landing
    // LEFT of `worker`, the pane on worker's left is `helper` — and after landing right, the pane
    // on its RIGHT is. Naming the neighbour (rather than asserting a `left=` appears at all) is
    // what makes this fail for a placement that landed on the wrong side of the right target.
    let helper = pane_id_in(&drawn_left, "helper");
    assert_eq!(
        neighbours_of(&drawn_left, "worker"),
        format!("left=pane id {helper}"),
        "after landing LEFT of worker, helper is what is on worker's left:\n{drawn_left}",
    );
    let helper = pane_id_in(&drawn_right, "helper");
    assert!(
        neighbours_of(&drawn_right, "worker").ends_with(&format!("right=pane id {helper}")),
        "and after landing RIGHT of it, helper is what is on its right:\n{drawn_right}",
    );

    // A move BETWEEN windows is the same request, and the source emptying is the answer's own
    // sentence — the fact a caller cannot see from a listing it read a moment earlier.
    server.call_tool("break_pane", json!({ "pane": "helper", "name": "briefly" }));
    let back = server.call_tool(
        "move_pane",
        json!({ "pane": "helper", "target": "worker", "dir": "down" }),
    );
    assert!(
        back.contains("below") && back.contains("that window closed"),
        "one request crosses a window, and says the emptied source went with it: {back}",
    );
    assert!(
        !server
            .call_tool("list_windows", json!({}))
            .contains("briefly"),
        "and it really did",
    );
}

/// The pane id `pane_layout` gives the pane it draws with `name`.
///
/// A window that is not the caller's carries NO NUMBERS — the drawing says so itself — so a named
/// pane one window over is addressed by id in the neighbour table and by name in the tree. This is
/// the join between the two halves of one drawing.
fn pane_id_in(drawing: &str, name: &str) -> String {
    let line = drawing
        .lines()
        .find(|line| line.contains(&format!("name={name:?}")))
        .unwrap_or_else(|| panic!("no pane is drawn as {name:?}:\n{drawing}"));
    line.rsplit_once("pane id ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("the tree line for {name:?} carries an id: {line}"))
        .to_owned()
}

/// What `pane_layout`'s neighbour table says is next to the pane drawn with `name`.
///
/// A helper rather than an inline `find`, because the assertion it serves is a comparison of two
/// drawings and the interesting line is one of several — quoting the whole drawing twice would hide
/// which part moved.
fn neighbours_of(drawing: &str, name: &str) -> String {
    let head = format!("  pane id {}:", pane_id_in(drawing, name));
    drawing
        .lines()
        .skip_while(|line| !line.starts_with("Which pane is next to which"))
        .find_map(|line| line.strip_prefix(&head))
        .unwrap_or_else(|| panic!("no neighbour line for {name:?}:\n{drawing}"))
        .trim()
        .to_owned()
}

/// **THE FOUR SENTENCES A ZOOM CAN ANSWER**, pinned as a pure function.
///
/// [`render_resize`]'s rule one verb over: a live daemon can be driven into the two TOGGLE cases
/// easily and into the two already-there cases only by racing itself, so the wording is fixed here
/// and the live gate drives what a live gate can. Each says what to do NEXT, which is the half an
/// agent cannot infer from a boolean pair.
#[test]
fn the_zoom_answers_say_which_of_the_four_states_it_left() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    let refused = server.call_tool_error("zoom_pane", json!({ "pane": 1 }));
    assert!(
        refused.contains("was opened by a person, not by you")
            && refused.contains("Which pane fills their window decides what they can see"),
        "a person's pane is not the agent's to zoom: {refused}",
    );

    server.call_tool("open_pane", json!({ "name": "wide" }));
    let filled = server.call_tool("zoom_pane", json!({ "pane": "wide" }));
    assert!(
        filled.contains("now fills its window")
            && filled.contains("read_pane sees it at the window's full width"),
        "the zoom says what changed and why an agent wanted it: {filled}",
    );
    // ALREADY THERE — the same call again, which is the case a boolean pair alone would not
    // distinguish from the one above.
    let again = server.call_tool("zoom_pane", json!({ "pane": "wide", "on": true }));
    assert!(
        again.contains("was already filling its window; nothing moved"),
        "asking for a state it is in says so rather than repeating the first sentence: {again}",
    );
    let back = server.call_tool("zoom_pane", json!({ "pane": "wide", "on": false }));
    assert!(
        back.contains("no longer fills its window") && back.contains("visible again"),
        "and the arrangement comes back: {back}",
    );
    let already = server.call_tool("zoom_pane", json!({ "pane": "wide", "on": false }));
    assert!(
        already.contains("was not filling its window"),
        "the fourth state has its own sentence too: {already}",
    );
}

/// **THE THIRD MOUTH REFUSES IN ITS OWN WORDS** — R323's finding, standing on this surface until
/// R335.
///
/// `unknown tool: X` is the sentence a TYPO gets, and it was the only sentence this server had. The
/// shell mouth stopped answering that way at R323, the keyboard at the same round, and the agent
/// surface kept it because there was nothing to ask: no table said which verbs an agent may have.
/// Now one does, so four different questions get four different answers, driven here through the
/// shipped binary.
///
/// The CONTROL is the fourth: a word that names no verb at all is still a typo, and says so.
#[test]
fn a_tool_that_is_not_there_says_which_kind_of_absence_it_is() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // THE SHELL'S SPELLING of a verb this surface really serves. A near miss, not a mistake.
    let near = server.call_tool_error("break-pane", json!({}));
    assert!(
        near.contains("sprag calls that `break_pane` here"),
        "a caller that typed the CLI spelling is told the tool's name: {near}",
    );

    // A VERB WITH NO TOOL YET. It must not read as a refusal — nothing about it is refused.
    let gap = server.call_tool_error("move_window", json!({}));
    assert!(
        gap.contains("sprag DOES have that verb")
            && gap.contains("`sprag move-window`")
            && gap.contains("it is a gap"),
        "a verb no tool serves is named as a gap, with the mouth that has it: {gap}",
    );

    // A VERB AN AGENT MAY NOT ASK FOR. It must say WHICH RULE, because that is what tells a caller
    // whether to look for another tool or to stop looking.
    let refused = server.call_tool_error("kill_server", json!({}));
    assert!(
        refused.contains("there will not be one")
            && refused.contains("it reaches outside the session the agent works in"),
        "a refusal names the rule it is, in the vocabulary's own words: {refused}",
    );
    let theirs = server.call_tool_error("set_option", json!({}));
    assert!(
        theirs.contains("it is the person's own"),
        "and a different rule reads differently: {theirs}",
    );

    // THE CONTROL. Without it every assertion above is satisfied by a server that never says
    // `unknown tool` at all.
    let typo = server.call_tool_error("no_such_tool", json!({}));
    assert!(
        typo.contains("unknown tool: no_such_tool") && typo.contains("tools/list"),
        "a word that names nothing is still a typo, and is told where the list is: {typo}",
    );
}

/// ⚠⚠ **AN AGENT CAN SAY WHAT READY LOOKS LIKE, AND NO TURN IS SPENT BEFORE IT** — the loop's
/// answer to the one thing every caller of it does first: open a pane and start something in it.
///
/// A pane is born a SHELL. The program an agent means to drive starts a moment later, and a run
/// that begins in that window feeds the shell — which runs the stimulus as a command. This drives
/// the whole thing the way an agent would: open a pane, type the program's start line, and
/// orchestrate with `ready_when` naming what the program prints when it is up.
///
/// ⚠⚠ **NO WAIT BETWEEN THE WRITE AND THE RUN, AND THAT IS THE CLAIM.** A pty echo is asynchronous,
/// so this is exactly the case that used to be decided by scheduling: the agent hands the daemon a
/// command line and starts a run in the same breath, with the echo of that line still in flight.
/// An earlier form of this gate needed a sleep to pass, which is the product asking the caller to
/// paper over its own race.
///
/// The marker is COMPOSED (`READY-%s` → `READY-OK`) so it cannot appear in the typed line at all —
/// the shape a caller should reach for, and the one the barrier can answer by construction. A
/// marker that IS in the typed line is refused outright and named; the plugin's own gate drives
/// both halves of that.
#[test]
fn an_agent_names_what_ready_looks_like_and_the_loop_waits_for_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_pane", json!({ "name": "slow" }));
    // ⚠⚠ THE STAND-IN MUST EAT WHAT IT IS GIVEN, and `</dev/tty` is what makes it. A background
    // job of a non-interactive shell reads stdin from /dev/null, so without that redirection the
    // stand-in consumes nothing: an early stimulus would sit in the pty buffer, the peer would read
    // it when it started, and the run would converge either way. Three seconds is twice what three
    // turns floored at the observe timeout can span.
    server.call_tool(
        "write_pane",
        json!({
            "pane": "slow",
            "text": "while read early; do echo \"SHELL-ATE $early\"; done </dev/tty & sleep 3; \
                     kill $! 2>/dev/null; printf 'READY-%s\\n' OK; exec sh -c 'while read l; do \
                     echo \"PEER-SAW $l\"; done'",
        }),
    );
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "slow",
            "stimulus": "ping",
            "ready_when": { "match": "prints", "marker": "READY-OK" },
            "sentinel": "PEER-SAW ping",
            "max_iterations": 3,
        }),
    );
    let ended = server.wait_for_tool("list_runs", json!({}), "converged");
    assert!(
        ended.contains("the sentinel appeared"),
        "the run waited for the peer to come up and then drove it to its sentinel: {ended}",
    );
    let seen = server.call_tool("read_pane", json!({ "pane": "slow" }));
    assert!(
        seen.contains("PEER-SAW ping"),
        "the peer itself must have seen the stimulus: {seen}",
    );
    // ⚠ `SHELL-ATE ping`, not bare `SHELL-ATE` — the pane echoes the command line that STARTED the
    // stand-in, and that line contains the bare word.
    assert!(
        !seen.contains("SHELL-ATE ping"),
        "nothing may have been typed while the pane was still the stand-in shell: {seen}",
    );
    assert!(
        ended.contains("after 1 iterations"),
        "and ONE turn was enough, because none was spent on the shell that was there first: \
         {ended}",
    );
}

/// ⚠⚠ **AN AGENT THAT NAMES A MARKER MUST SAY WHICH QUESTION IT IS ASKING** — the readiness value
/// is an object, and the shape a caller wrote before the bump is refused rather than guessed at.
///
/// Reading the old string as either kind would answer a caller's question with the other one and
/// never say so. `WIRE_PROTOCOL` moved for exactly this (21 → 22), so the refusal is the contract.
#[test]
fn a_readiness_barrier_that_does_not_say_which_question_it_asks_is_refused() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_pane", json!({ "name": "target" }));
    // The PRE-BUMP spelling: a bare needle, with no word for what it is matched against.
    let refused = server.call_tool_error(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "target",
            "stimulus": "ping",
            "ready_when": "READY-OK",
            "max_iterations": 1,
        }),
    );
    assert!(
        !refused.is_empty(),
        "a run must not start from a readiness barrier this daemon cannot read",
    );
    // And a word outside the closed set is refused too — a vocabulary that accepted anything would
    // make the published `enum` an affirmative false statement.
    let bad_word = server.call_tool_error(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "target",
            "stimulus": "ping",
            "ready_when": { "match": "appears", "marker": "READY-OK" },
            "max_iterations": 1,
        }),
    );
    assert!(
        !bad_word.is_empty(),
        "`appears` is not one of the two questions this daemon answers",
    );
}

/// ⚠⚠ **AN AGENT CAN OPEN A PANE THAT IS THE PROGRAM, NOT A SHELL RUNNING IT** — the structural
/// answer to the echo, and the one that removes the hazard instead of detecting it.
///
/// The daemon's spawn has taken an argv all along; this tool did not offer it, so every
/// agent-opened pane was a shell and every loop had to start its program by TYPING into one. That
/// is what puts a command line on the screen for a readiness marker to match, and what let an
/// agent prompt come back as `sh: not found`. With `cmd` there is nothing to echo: the pane is the
/// program from its first byte.
#[test]
fn an_agent_opens_a_pane_that_is_the_program_rather_than_a_shell_running_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool(
        "open_pane",
        json!({
            "name": "tool",
            // Announces itself, then answers each line — a stand-in for a REPL, started WITHOUT a
            // shell, so nothing it is told to wait for can appear before it exists.
            "cmd": ["/bin/sh", "-c", "printf 'TOOL-UP\n'; while read l; do echo \"TOOL-SAW $l\"; done"],
        }),
    );
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "tool",
            "stimulus": "ping",
            // ⚠⚠ `shows`, AND THAT IS THE POINT. On a pane opened with `cmd` there is no shell and
            // no echo, so the ONLY thing that can put this text on the screen is the program — and
            // then "is it there?" is both safe and the only question that always terminates. With
            // `prints` the caller would be racing their own program's banner: it is printed the
            // instant the pane is born, so a run that starts a moment later waits for a line that
            // has already been said. Measured, on the first form of this gate.
            "ready_when": { "match": "shows", "marker": "TOOL-UP" },
            "sentinel": "TOOL-SAW ping",
            "max_iterations": 3,
        }),
    );
    let ended = server.wait_for_tool("list_runs", json!({}), "converged");
    assert!(
        ended.contains("the sentinel appeared"),
        "the run drove the program the pane IS: {ended}",
    );
    // ⚠ AND NO SHELL EVER SAW THE STIMULUS — there was no shell. A pane opened the old way would
    // have `sh:` somewhere on it the moment a stimulus arrived early.
    let seen = server.call_tool("read_pane", json!({ "pane": "tool" }));
    assert!(
        !seen.contains("not found"),
        "nothing was run as a shell command: {seen}",
    );
}

/// ⚠⚠⚠ **THE AGENT IS TOLD ITS `Ctrl-C` DID NOT STOP ANYTHING, BY THE TOOL THAT SENT IT.**
///
/// This is the ai-loop's stop, end to end through the door an agent actually calls. `send_keys`
/// answered `Sent 1 key(s)` whether the job was interrupted or the byte was swallowed as text, and
/// the fact that a full-screen program has turned signals off was written on **`stop_job`'s**
/// description — a tool the agent did not call, because it reached for the chord a person would.
///
/// The SUBJECT is a pane whose program ran `stty -isig`, which is what every editor, every
/// full-screen TUI and every interactive agent CLI does on startup. The CONTROL is a pane that
/// never touched its terminal: it must answer with NO caveat, because a warning printed on every
/// keystroke is one an agent learns to skip, and then it is not a warning.
///
/// ⚠ The answer must also name the REMEDY. An agent told only that nothing happened does the one
/// thing that cannot work: it sends the key again.
#[test]
fn send_keys_tells_an_agent_when_its_ctrl_c_cannot_become_a_signal() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    // The pane an agent drives: a program that has taken its terminal, announcing only AFTER the
    // `stty`, so the assertion below cannot race the shell that runs it.
    server.call_tool(
        "open_pane",
        json!({
            "name": "raw",
            "cmd": ["/bin/sh", "-c", "stty -isig; printf 'RAW-UP\n'; exec cat"],
        }),
    );
    server.wait_for_tool("read_pane", json!({ "pane": "raw" }), "RAW-UP");

    let sent = server.call_tool(
        "send_keys",
        json!({ "pane": "raw", "keys": ["c"], "ctrl": true }),
    );
    assert!(
        sent.contains("Ctrl-C") && sent.contains("raised NO signal"),
        "⚠⚠⚠ the tool that wrote the byte is the one that has to say the signal did not follow — \
         the agent called THIS, not stop_job: {sent}",
    );
    assert!(
        sent.contains("taken its terminal raw"),
        "and WHY, because a caller retries a raw pane differently from a rebound character: {sent}",
    );
    assert!(
        sent.contains("stop_job"),
        "and WHAT TO DO — an agent told only that nothing happened sends the key again, which is \
         the one move that cannot work: {sent}",
    );

    // THE CONTROL: a pane that never reconfigured its terminal, driven through the same tool with
    // the same chord. Here the byte really does become a SIGINT, and there is nothing to report.
    server.call_tool(
        "open_pane",
        json!({
            "name": "cooked",
            "cmd": ["/bin/sh", "-c", "printf 'COOKED-UP\n'; exec cat"],
        }),
    );
    server.wait_for_tool("read_pane", json!({ "pane": "cooked" }), "COOKED-UP");
    let quiet = server.call_tool(
        "send_keys",
        json!({ "pane": "cooked", "keys": ["c"], "ctrl": true }),
    );
    assert!(
        !quiet.contains("raised NO signal"),
        "a pane whose terminal DOES raise the signal is not warned about — a caveat on every \
         keystroke is noise, and an agent that learns to skip it is not warned by it: {quiet}",
    );

    // ⚠⚠ AND THE SAME THROUGH `write_pane`, because a `0x03` reaches a pane as literal text at
    // least as often as it reaches one as a key, and a warning that depended on which door the
    // caller used would be a property of the spelling rather than of the pane.
    let typed = server.call_tool(
        "write_pane",
        json!({ "pane": "raw", "text": "\u{3}", "enter": false }),
    );
    assert!(
        typed.contains("Ctrl-C") && typed.contains("stop_job"),
        "the verb that types is in exactly the position the verb that keys was: {typed}",
    );
}

/// ⚠⚠ **AN ARGUMENT THIS SURFACE DOES NOT TAKE IS REFUSED, NOT SWALLOWED** — the other half of a
/// typo, and the half every tool here PUBLISHED and none of them enforced.
///
/// A misspelled TOOL name has been an answer an agent can act on since R323. A misspelled ARGUMENT
/// was answered `success` with the argument dropped: every one of these tools declares
/// `additionalProperties: false` and nothing ever read that declaration, so the schema a client is
/// handed was an affirmative false statement about what the door accepts.
///
/// # ⚠ It cost this file a gate, which is how it was found
///
/// The time-ceiling gate below opened its pane with a `cmd` argument. `open_pane` has never had
/// one — it runs a shell, always — so the pane it drove was a login shell and the test's own
/// account of what it was measuring was wrong from the day it was written. Nothing said so,
/// because the call worked.
///
/// The shape that matters is the same defect on the loop's own verbs: `max_second` for
/// `max_seconds` is one keystroke, and swallowed it means a run the caller believes it bounded and
/// the daemon bounds only by its defaults. An ignored ceiling makes the loop do MORE, silently,
/// and answers success.
///
/// # Why it walks the roster and fails closed
///
/// The subject is the PUBLICATION, so the gate reads the same publication a client does and holds
/// every tool that closes its argument set to having closed it. A tool added later is covered the
/// day it appears; one that publishes a closed set and does not enforce it fails here BY NAME. The
/// count at the end is what makes a tool that quietly stopped being checked impossible to miss.
#[test]
fn every_tool_that_publishes_a_closed_argument_set_enforces_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    let roster = server.request("tools/list", json!({}));
    let tools = roster["result"]["tools"]
        .as_array()
        .expect("the roster is a list")
        .clone();
    let mut checked: Vec<String> = Vec::new();
    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .expect("every tool is named")
            .to_owned();
        // The DECLARATION is what asks to be enforced: a tool that leaves its argument set open is
        // not held to a closed one, so this can never be stricter than what the caller was told.
        if tool["inputSchema"]["additionalProperties"] != json!(false) {
            continue;
        }
        // The bogus argument ALONE, so a tool that refuses it cannot be doing anything else: no
        // required argument is present, and a tool that ran anyway would answer about its own
        // missing arguments instead. Nothing here reaches a pane.
        let refused = server.call_tool_error(&name, json!({ "no_such_argument": 1 }));
        assert!(
            refused.contains("no_such_argument"),
            "{name} publishes a closed argument set and swallowed one outside it — an agent that \
             mistypes an argument is told the call worked and never learns what it actually did: \
             {refused}",
        );
        assert!(
            refused.contains(&format!("{name} takes")),
            "and the refusal must hand back the arguments it DOES take, or a caller that guessed \
             is left guessing again: {refused}",
        );
        checked.push(name);
    }
    assert_eq!(
        checked.len(),
        tools.len(),
        "every tool this server serves closes its argument set, so every one of them is checked \
         here; {} of {} were: the ones missing are {:?}",
        checked.len(),
        tools.len(),
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default())
            .filter(|name| !checked.iter().any(|done| done == name))
            .collect::<Vec<_>>(),
    );

    // THE CONTROL. Every assertion above is satisfied by a server that refuses EVERYTHING, which
    // would be a worse defect wearing this gate's colours.
    let accepted = server.call_tool("read_pane", json!({ "pane": 1 }));
    assert!(
        !accepted.starts_with("Error:"),
        "a call made only of declared arguments still goes through: {accepted}",
    );
}

// ----- the orchestration loop's door (R355) -----

/// Open a pane of this agent's own that ECHOES what is injected into it and RUNS nothing, and
/// return once it is one.
///
/// # ⚠⚠ Why the loop's gates cannot just ask for the pane they want
///
/// `open_pane` runs a SHELL. It takes a `name` and a `cwd` and has never taken a command — so the
/// eight gates that asked it for `["/bin/sh", "-c", "exec cat"]` were driving a login shell, and
/// every stimulus they injected was EXECUTED by it rather than echoed back. Nothing said so,
/// because the argument was swallowed whole (the defect
/// [`every_tool_that_publishes_a_closed_argument_set_enforces_it`](self) closed).
///
/// The difference is not cosmetic. A drive loop's fixture must react to a stimulus without acting
/// on it: against a shell, `stimulus: "sleep 1"` is a real second of sleeping per turn and
/// `"echo bounded"` is a command whose output is indistinguishable from the echo the orchestrator
/// is watching for. So the shell is replaced, by the one means this surface offers — typing.
///
/// ⚠ The marker is waited for TWICE. The first `ECHO-READY` is the shell echoing the line as it is
/// typed; only the second is `printf` having RUN, which is the instant the pane is `cat`. A run
/// started on the first sighting would have its opening turns eaten by a shell that is still
/// starting up.
fn open_echo_pane(server: &mut McpServer, name: &str) {
    server.call_tool("open_pane", json!({ "name": name }));
    server.call_tool(
        "write_pane",
        // ⚠ The marker ENDS ITS ROW. Without the newline the cursor stays on it, the first
        // stimulus is echoed onto the same row, and the row then reads as neither the marker nor
        // the stimulus — which the orchestrator correctly judges to be output of the pane's own.
        // A fixture that manufactures an answer would hide exactly the silence it is here to show.
        json!({ "pane": name, "text": "printf 'ECHO-READY\\n'; exec cat" }),
    );
    server.wait_for_tool_count("read_pane", json!({ "pane": name }), "ECHO-READY", 2);
}

/// ⚠⚠ **AN AGENT CAN ASK FOR A BOUNDED LOOP, AND THE BOUND IS THE PLATFORM'S** — the whole of L2, at
/// the mouth it was missing from.
///
/// # What only a live run can prove here
///
/// Every part of this was already built and reachable by nobody: the plugin, the driver, the
/// iteration ceiling. What had never been observed is that an AGENT can get at them — that a
/// `tools/call` becomes a real run against a real pane, bounded where it was told to be, and that
/// the outcome is still readable afterwards. The number `3` is what makes it a claim rather than a
/// smoke test: a run that ignored its guardrail would report the daemon's default of 100 and this
/// would fail with the number it saw.
///
/// The pane is one the agent OPENED, because that is the only kind it may drive — see the twin
/// below, which is the same call against somebody else's pane.
#[test]
fn an_agent_starts_a_bounded_loop_and_reads_how_it_ended() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "loop-target");

    let started = server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "loop-target",
            "stimulus": "echo bounded",
            "max_iterations": 3,
        }),
    );
    assert!(
        started.contains("Run 0 started") && started.contains("bounded"),
        "a run id comes back at once, and the answer says the run is bounded: {started}",
    );

    let ended = server.wait_for_tool("list_runs", json!({}), "exhausted");
    assert!(
        ended.contains("exhausted — it ran out of iterations after 3 iterations"),
        "THE GUARDRAIL BOUND IT, AND SAID WHICH ONE: the agent asked for three turns and got \
         three, where this daemon's own default is {}. The named ceiling is the other half — an \
         agent told only `exhausted` cannot tell the turn ceiling it chose from the wall-clock \
         deadline it inherited, and the two have different remedies. {ended}",
        sprag_host::plugins::DEFAULT_MAX_ITERATIONS,
    );

    // ⚠⚠ AND WHAT THE THREE TURNS ACTUALLY DID. A total tells an agent its loop failed and gives
    // it nothing to act on but running the loop again and watching — which is the turn-by-turn
    // watching `orchestrate` exists to remove. One line per step, from the plugin's own words.
    assert!(
        ended.contains("What its steps did:"),
        "a finished run accounts for its steps, not only its total: {ended}",
    );
    for turn in 1..=3 {
        assert!(
            ended.contains(&format!("    {turn}. ")),
            "every step the run took is in the journal, and step {turn} is missing: {ended}",
        );
    }
    // ⚠⚠ AND THE ACCOUNT IS SPECIFIC ABOUT WHAT CAME BACK. This pane runs `cat`: it parrots the
    // stimulus and produces nothing of its own — and NOTHING ON A SCREEN can tell `cat` writing
    // the text back from the pty echoing it, because they render identically. So the honest
    // account is that the peer said nothing, which is a different finding from a pane that showed
    // nothing at all and different again from one that answered. A journal that called this
    // "reacted" was reporting the kernel's work as the peer's.
    assert!(
        ended.contains("THE PEER SAID NOTHING"),
        "each line carries the PLUGIN's account of that step, which is the only place the \
         difference between a peer that answered and one that merely echoed can appear: {ended}",
    );
    assert!(
        ended.contains("bytes"),
        "and the cost is reported in the run's OWN unit, which is what stops a byte budget from \
         being read as a token one: {ended}",
    );
}

/// ⚠⚠⚠ **A CANCELLED RUN TELLS THE AGENT WHAT BECAME OF ITS WORK** — the half that reached the
/// wire and died at the mouth.
///
/// `cancelled after N iterations` is consistent with two opposite states of the world: the peer
/// stopped, or it is still going and still spending somebody's money. An agent cannot tell them
/// apart and cannot derive the answer — only the daemon that tried the stop knows. The listing was
/// printing the ceiling and the failure and dropping this, so the fact existed on the wire and
/// never reached the one reader who acts on it.
///
/// ⚠ The pane here runs `exec cat`, so its OWN program is the peer — the case where a cut-short run
/// is refused the reach that would close the pane, and therefore the case where the answer is *your
/// work is still running*. That is the answer worth having, and it is the one that was missing.
#[test]
fn a_cancelled_run_tells_the_agent_whether_its_peer_is_still_working() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "cancelme");

    let started = server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "cancelme",
            "stimulus": "x",
            // ⚠ The agent surface's own ceiling, not a bigger number: an agent may TIGHTEN a
            // guardrail and never loosen one, so asking for more is refused before the run starts.
            "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
            "max_iterations": sprag_host::plugins::DEFAULT_MAX_ITERATIONS,
        }),
    );
    assert!(started.contains("Run 0 started"), "{started}");
    server.wait_for_tool("list_runs", json!({}), "still running");

    let cancelled = server.call_tool("cancel_run", json!({ "run": 0 }));
    assert!(!cancelled.is_empty(), "the cancel is taken: {cancelled}");

    let ended = server.wait_for_tool("list_runs", json!({}), "cancelled");
    assert!(
        ended.contains("still running"),
        "⚠⚠ THE ANSWER AN AGENT ACTS ON: a cancelled run must say whether its peer stopped, and \
         this pane's own program IS the peer — so the honest answer is that it did not: {ended}",
    );
}

/// ⚠⚠ **A LOOP IS REFUSED AGAINST A PANE THE AGENT DOES NOT OWN** — the rule the five other writing
/// tools keep, and the reason this one had to have it.
///
/// Without it `orchestrate` is a LAUNDERING PATH around every one of them: an agent refused
/// `write_pane` on a person's pane could have driven that same pane through a plugin run, injecting
/// the same bytes through a door nobody had put the check on. R340 recorded exactly this shape when
/// `grant_pane` shipped without the guard its four neighbours had.
///
/// The control is the twin above: the same verb, the same daemon, a pane the agent opened.
#[test]
fn an_agent_cannot_loop_a_pane_it_does_not_own() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);

    let refused = server.call_tool_error(
        "orchestrate",
        json!({ "plugin": "orchestrator", "pane": 1, "stimulus": "echo mine" }),
    );
    assert!(
        refused.contains("opened by a person") && refused.contains("orchestrate will not touch it"),
        "it is refused in the words of the ownership rule, naming this verb: {refused}",
    );
    assert!(
        server.call_tool("list_runs", json!({})).contains("no runs"),
        "and nothing was started",
    );
}

/// ⚠⚠ **A GUARDRAIL AN AGENT COULD RAISE IS NOT A GUARDRAIL** — the cost decision, driven at both
/// ends.
///
/// The wire accepts any bound from anyone, which is right for a person driving their own machine.
/// This surface is the one that knows who is asking, so it is the one that holds an agent to the
/// daemon's own published defaults — and it REFUSES rather than silently clamping, because a run
/// that quietly did something other than what it was asked is how a guardrail becomes folklore.
///
/// Both directions are here: asking for more is refused NAMING the ceiling, and asking for less is
/// honoured. A gate with only the first half would pass over an implementation that refused
/// everything.
#[test]
fn an_agent_may_tighten_a_guardrail_and_never_loosen_one() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "budget");

    let ceiling = sprag_host::plugins::DEFAULT_MAX_ITERATIONS;
    let refused = server.call_tool_error(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "budget",
            "stimulus": "x",
            "max_iterations": u64::from(ceiling) + 1,
        }),
    );
    assert!(
        refused.contains(&format!("at most {ceiling}")),
        "the refusal names the ceiling the daemon published, so the agent knows what to ask for: \
         {refused}",
    );

    // THE OTHER HALF: under the ceiling is honoured, so this is a bound and not a ban.
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "budget",
            "stimulus": "x",
            "max_iterations": 2,
        }),
    );
    let ended = server.wait_for_tool("list_runs", json!({}), "exhausted");
    assert!(ended.contains("after 2 iterations"), "{ended}");
}

/// ⚠⚠ **THE CLAMP HELD A CEILING IT WAS NEVER TAUGHT** — the payoff of a mouth whose arguments are
/// DERIVED from the daemon's publication rather than written down in it.
///
/// `max_seconds` is a guardrail this build added. Not one line of the clamp mentions it: the rule
/// is *any published argument whose name the daemon also publishes a default for is a ceiling*, so
/// a bound that reached the grammar reached the authority policy in the same compile. That is the
/// property the whole publication surface was built for, and until this argument existed nothing
/// had ever added a ceiling to test it with.
///
/// ⚠ This gate says the ceiling EXISTS and binds; it cannot say the number came from the daemon,
/// because this daemon and this binary are one build and agree by construction. That half is
/// [`the_ceiling_an_agent_is_held_to_is_the_daemons_and_not_this_binarys`](self)'s, against a peer
/// that publishes a different one.
///
/// ⚠ AND THE OTHER HALF: a run under the ceiling is honoured AND ends by the clock it named, with
/// the ceiling that stopped it in the answer. Without that, this gate would pass over a surface
/// that refused every `max_seconds` there is.
#[test]
fn an_agent_is_held_to_a_time_ceiling_the_clamp_was_never_told_about() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    // A DEAF pane, made one the only way this surface can: `open_pane` runs a SHELL — it has no
    // `cmd` argument and never had one — so the shell is told to go deaf. `stty -echo` stops the
    // kernel echoing what is injected and the reader discards what it reads, so once this line has
    // run, nothing this run does can reach the screen. See the second half for why the gate needs
    // a pane that cannot react rather than one that reacts fast.
    server.call_tool("open_pane", json!({ "name": "deaf" }));
    server.call_tool(
        "write_pane",
        json!({
            "pane": "deaf",
            "text": "stty -echo; printf DEAF-READY; exec cat >/dev/null",
        }),
    );
    // ⚠ AND IT IS NOT DEAF UNTIL IT SAYS SO. Until that line has RUN, the pane is an ordinary
    // shell with echo on, and a run that starts driving in that window has its first stimulus
    // echoed back — a pane that cannot hear it, read as one that reacted. TWICE is the
    // load-bearing count: the first `DEAF-READY` is the shell echoing the line as it is typed, and
    // only the second is `printf` running, which is the instant echo is actually off.
    server.wait_for_tool_count("read_pane", json!({ "pane": "deaf" }), "DEAF-READY", 2);

    let ceiling = sprag_host::plugins::DEFAULT_MAX_SECONDS;
    let refused = server.call_tool_error(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "deaf",
            "stimulus": "x",
            "max_seconds": ceiling + 1,
        }),
    );
    assert!(
        refused.contains("max_seconds") && refused.contains(&format!("at most {ceiling}")),
        "a time bound above this daemon's own is refused naming the ceiling, exactly as the other \
         two are, with no rule of its own: {refused}",
    );

    // THE OTHER HALF — under the ceiling, and the clock is what ends it.
    //
    // ⚠⚠ THE PANE MUST BE ONE THAT CANNOT REACT, and this gate was RED IN CI for wanting the
    // opposite. Against a pane that echoes, a step ends as soon as the echo lands, so which
    // ceiling binds is a race between the box's speed and the second on the clock: this machine
    // took just over a second to run the hundred turns and read `duration`, and CI's took under
    // one and read `iterations`. A gate that asks which of two ceilings fired must make the other
    // one UNREACHABLE, not merely slower.
    //
    // Deaf, it is arithmetic instead: every step waits out the orchestrator's own half-second
    // observe timeout, so a one-second run can complete two or three turns and NEVER a hundred —
    // on any machine, since a slower box only makes the floor higher.
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "deaf",
            "stimulus": "x",
            "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
            "max_iterations": 100,
            "max_seconds": 1,
        }),
    );
    let ended = server.wait_for_tool("list_runs", json!({}), "exhausted");
    assert!(
        ended.contains("it ran out of duration"),
        "an agent is told WHICH ceiling stopped its loop, and here it is the clock rather than \
         the hundred turns it also asked for — three ceilings with three different remedies, and \
         `exhausted` alone names none of them: {ended}",
    );
    // ⚠ AND THE GATE CHECKS ITS OWN PREMISE. If the pane ever reacted — `stty` missing, echo back
    // on, a shell that writes something — the floor under each step would be gone and the half
    // above would be back to racing the clock, passing or failing by how fast the box is. The
    // journal says which pane this actually ran against, so the race cannot come back unnoticed.
    assert!(
        ended.contains("the pane did not react to the stimulus at all"),
        "this run must have driven a DEAF pane, or it is not the deadline being measured but the \
         speed of this machine: {ended}",
    );
}

/// ⚠⚠ **THE CEILING AN AGENT IS HELD TO IS THE DAEMON'S, NOT THIS BINARY'S** — the load-bearing
/// claim under the whole authority policy, and until now it was only a comment.
///
/// `guardrail_defaults` says it reads the daemon *"though this binary could see both"*, because a
/// number compiled into a separately-built client is a different number the day the two are not the
/// same build. Nothing drove it — and nothing COULD, with the tools this workspace had: every
/// daemon a test can start is built from these same constants, so a mouth that had quietly compiled
/// `DEFAULT_MAX_SECONDS` in would agree with every one of them. **An absence cannot ask this
/// question**; a client using its own constant survives every absence perfectly.
///
/// So the witness is a peer that PUBLISHES A DIFFERENT CEILING (`sprag_peer::Missing::answering`,
/// added for this): a real daemon behind it, one key of one answer replaced. Sixty seconds is far
/// under this build's own hour and far over the peer's five.
///
/// ⚠ Two controls, because one refusal proves little: four seconds is ACCEPTED (so five is a bound
/// and not a ban), and `max_iterations: 60` is ACCEPTED (so the mouth re-read the whole slot from
/// the daemon rather than special-casing the key the peer moved).
#[test]
fn the_ceiling_an_agent_is_held_to_is_the_daemons_and_not_this_binarys() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let peer = sprag_peer::OldDaemon::proxying(
        &socket_path(),
        &sock,
        sprag_peer::Missing::answering(&[("max_seconds", json!(5))]),
    );
    let mut server = McpServer::spawn_in_pane(peer.sock(), 0);
    open_echo_pane(&mut server, "skew");

    let ask = |guardrail: &str, value: u64| {
        json!({
            "plugin": "orchestrator",
            "pane": "skew",
            "stimulus": "x",
            guardrail: value,
        })
    };

    // Sixty seconds is over the ceiling THIS DAEMON publishes, which is five.
    let refused = server.call_tool_error("orchestrate", ask("max_seconds", 60));
    assert!(
        refused.contains("at most 5"),
        "the mouth must hold the agent to the number the daemon answered. Reading its own \
         compiled default ({}) would have let sixty through: {refused}",
        sprag_host::plugins::DEFAULT_MAX_SECONDS,
    );

    // CONTROL: under the peer's ceiling is honoured, so five is a bound and not a ban.
    server.call_tool("orchestrate", ask("max_seconds", 4));
    // CONTROL: the ceiling the peer did NOT move still binds where it always did, so the mouth read
    // the whole published slot rather than one key it was taught about.
    server.call_tool("orchestrate", ask("max_iterations", 60));
}

/// ⚠⚠ **AN AGENT SEES AND CANCELS ITS OWN RUNS, AND NOBODY ELSE'S** — the answer to *whose runs are
/// these?*, which the registry itself cannot give.
///
/// The run registry is DAEMON-WIDE: the `runs` slot answers with every run the host holds, whatever
/// session asked. So an unfiltered tool would show an agent the person's work and let it cancel
/// somebody else's loop. What makes the filter possible is the provenance the daemon now records at
/// submit time, stamped by this server from its own pane and never taken from the caller.
///
/// Two agents in two panes, against one daemon, is the fixture that can express it: a single agent
/// cannot tell "my runs" from "all runs".
#[test]
fn one_agents_runs_are_invisible_to_another() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut first = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut first, "firsts");
    first.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "firsts",
            "stimulus": "x",
            "max_iterations": 2,
        }),
    );
    let mine = first.wait_for_tool("list_runs", json!({}), "exhausted");
    assert!(
        mine.contains("Run 0"),
        "the first agent sees its own: {mine}"
    );

    // The SECOND agent, in the pane the first one opened — a different pane of the same daemon, so
    // the registry it reads is the same one and only the provenance tells them apart.
    let target = first.call_tool("list_panes", json!({}));
    let mut second = McpServer::spawn_in_pane(&sock, pane_id_named(&target, "firsts"));
    let theirs = second.call_tool("list_runs", json!({}));
    assert!(
        theirs.contains("no runs"),
        "the second agent started nothing, and the daemon's registry is not its own list: {theirs}",
    );
    let refused = second.call_tool_error("cancel_run", json!({ "run": 0 }));
    assert!(
        refused.contains("not one of yours"),
        "and it cannot stop a loop it did not start: {refused}",
    );

    // THE CONTROL: the first agent CAN still see run 0, so the second's blindness is about
    // ownership and not about the run having gone away.
    assert!(first.call_tool("list_runs", json!({})).contains("Run 0"));
}

/// The host id of the pane called `name` in a `list_panes` answer.
///
/// Reads the `id=` the listing prints rather than the 1-based number, because what a server is
/// SPAWNED in is a host id — the variable the daemon exports — and a number would mean the row it
/// happened to be on.
fn pane_id_named(listing: &str, name: &str) -> u64 {
    listing
        .lines()
        .find(|line| line.contains(&format!("\"{name}\"")))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix("id="))
        })
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("no pane called {name:?} in:\n{listing}"))
}

/// ⚠⚠ **AN AGENT WAITS FOR ITS OWN LOOP INSTEAD OF POLLING FOR IT** — L3, and the half of the
/// loop's door that R355 shipped without.
///
/// # What this is worth, and why a poll is not the same thing
///
/// `orchestrate` exists so an agent does not spend its turns driving a loop. Until a run's end was
/// an EVENT, the only way for the agent that started one to learn it had finished was to call
/// `list_runs` again — and for an agent every call is a TURN. The feature saved the turns inside
/// the loop and charged them back at the end.
///
/// The claim is driven the way it is used: start a run, then WAIT — one call, no polling — and
/// require the wait to report the run by id. The bound is what makes it a claim rather than a
/// sleep: `wait_for_change` returns when the change lands, so a wait that reported nothing would
/// fail here with its own timeout rather than passing quietly.
#[test]
fn an_agent_waits_for_its_own_loop_to_finish_rather_than_polling() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "waited");
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "waited",
            "stimulus": "echo waited",
            "max_iterations": 2,
        }),
    );

    // ONE call. No `list_runs` in between — the point is that none is needed.
    let woke = server.call_tool(
        "wait_for_change",
        json!({ "kinds": ["run_finished"], "timeout_seconds": 30 }),
    );
    assert!(
        woke.contains("run_finished"),
        "the wait must report the kind it was asked for: {woke}",
    );
    assert!(
        woke.contains('0'),
        "and the run it names is the one that was started: {woke}",
    );

    // ...and the outcome is where the event says to look, which is what makes carrying only the id
    // the right choice.
    let ended = server.call_tool("list_runs", json!({}));
    assert!(
        ended.contains("exhausted — it ran out of iterations after 2 iterations"),
        "the run really did finish, at the guardrail the agent asked for — and it names which \
         ceiling that was: {ended}",
    );
}

/// ⚠⚠ **A CANCEL WAKES THE SAME WAIT** — the property that makes one wait sufficient.
///
/// `cancelled` is a terminal state, so an agent that asked its loop to stop learns it HAS stopped
/// from the event it was already parked on. Without this an agent would need a second mechanism for
/// the case it caused itself, which is the shape `wait_for_change`'s own disjunction exists to
/// avoid one level down.
#[test]
fn a_cancelled_loop_wakes_the_wait_that_was_parked_on_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "stopped");
    // A ceiling this test will not reach, so what ends the run is provably the cancel.
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "stopped",
            "stimulus": "sleep 1",
            "max_iterations": 100,
        }),
    );
    server.wait_for_tool("list_runs", json!({}), "still running");

    server.call_tool("cancel_run", json!({ "run": 0 }));
    let woke = server.call_tool(
        "wait_for_change",
        json!({ "kinds": ["run_finished"], "timeout_seconds": 30 }),
    );
    assert!(
        woke.contains("run_finished"),
        "a cancel is a terminal state and must wake the same wait: {woke}",
    );
    assert!(
        server
            .call_tool("list_runs", json!({}))
            .contains("cancelled"),
        "and the run ended cancelled, not exhausted",
    );
}

/// ⚠⚠ **A RUNNING LOOP SAYS WHAT IT HAS SPENT, BEFORE IT HAS SPENT IT ALL** — L4.
///
/// # The question `still running` could not answer
///
/// The driver counts iterations and accumulates a typed cost from the first step, and published
/// both ONLY in the terminal outcome. So an agent watching a long loop could not tell PROGRESS from
/// STUCK, and could not see spend until the spending was over — which is a strange property for a
/// feature whose selling point is a cost ceiling.
///
/// The claim is driven where it matters: read a run WHILE it is running and require a non-zero
/// iteration count, then read it again after it ends and require the last progress to AGREE with
/// the outcome. The second half is what stops the first from being satisfied by a counter that
/// counts something else.
#[test]
fn a_running_loop_reports_its_progress_and_agrees_with_its_own_outcome() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "counted");
    // A ceiling high enough that this test observes it mid-flight rather than after it.
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "counted",
            "stimulus": "sleep 1",
            "max_iterations": 100,
        }),
    );

    let mid = server.wait_for_tool("list_runs", json!({}), "still running — 1");
    assert!(
        mid.contains("bytes spent so far"),
        "a running run reports its spend in the run's OWN unit: {mid}",
    );

    // The AGREEMENT half: cancel it, then require the outcome's numbers to be the ones progress was
    // reporting. A counter that ran ahead of the work, or lagged behind it, fails here.
    server.call_tool("cancel_run", json!({ "run": 0 }));
    let ended = server.wait_for_tool("list_runs", json!({}), "cancelled");
    let iterations = |text: &str, marker: &str| -> u64 {
        text.split(marker)
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no count after {marker:?} in {text:?}"))
    };
    assert!(
        iterations(&ended, "cancelled after ") >= iterations(&mid, "still running — "),
        "the outcome cannot report FEWER iterations than progress already showed:\n{mid}\n{ended}",
    );
}

/// ⚠⚠ **A LOOP THAT FINISHES BEFORE THE AGENT LOOKS IS STILL WAITED FOR** — the race that made the
/// event only half an answer.
///
/// `orchestrate` returns the instant the run is submitted, and a short run finishes before the
/// agent gets its next turn. `wait_for_change` anchors its cursor at the PRESENT on its first call,
/// so an agent whose first wait comes after the run ended would park on a record already written
/// and wait out its whole timeout — worse than polling, because it looks like the loop is still
/// going.
///
/// The fixture FORCES the order rather than racing it: the run is driven to completion and OBSERVED
/// finished through `list_runs` before the wait is ever issued. A wait that only worked when it got
/// there first would fail here every time.
#[test]
fn a_loop_that_ended_before_the_first_wait_is_reported_by_it() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "already-done");
    server.call_tool(
        "orchestrate",
        json!({
            "plugin": "orchestrator",
            "pane": "already-done",
            "stimulus": "echo quick",
            "max_iterations": 1,
        }),
    );
    // THE ORDER, forced: the run is over before anybody waits for it.
    server.wait_for_tool("list_runs", json!({}), "exhausted");

    let woke = server.call_tool(
        "wait_for_change",
        json!({ "kinds": ["run_finished"], "timeout_seconds": 20 }),
    );
    assert!(
        woke.contains("run_finished") && woke.contains("run 0"),
        "the wait must report a change that landed before it was called, naming the run: {woke}",
    );
}

// ----- answering a blocked peer, from the surface that publishes the question (R369) -----

/// Open a pane of this agent's own that draws a REAL tool-permission dialog and reports which key
/// moved it, and return once the daemon reads it as `blocked`.
///
/// # ⚠⚠⚠ Why the peer has to say WHICH KEY it acted on
///
/// Every claim below is about which keystrokes the daemon SENT, and a fixture that watched only the
/// outcome would pass for a run that typed a digit it did not need — the exact over-typing the
/// consent contract exists to prevent. So the pane is the witness: it prints
/// `took <option> via <byte>` when a key moves it, and prints nothing at all when none does.
///
/// ⚠ The words are assembled by `printf`'s FORMAT rather than written out, and that is not style.
/// The script is TYPED into a shell, so a literal `took 3 via 51` in it would be echoed onto the
/// pane's own scrollback and every assertion below would read the fixture's source instead of its
/// behaviour. `took %s via %s` in the command line cannot be mistaken for `took 3 via 51` in the
/// output.
///
/// ⚠⚠ Three options, and the middle one is the measured hazard: `Yes` / `Yes, and do not ask again`
/// is what a real permission dialog offers, and `and` is on two of them. That is what makes the
/// ambiguity refusal below a claim rather than a degenerate case.
fn open_asking_pane(server: &mut McpServer, name: &str) {
    server.call_tool("open_pane", json!({ "name": name }));
    server.call_tool(
        "write_pane",
        json!({
            "pane": name,
            // The OSC 2 title is `claude`'s resting fingerprint — without it no agent manifest
            // claims the pane and the daemon consults no rule at all, which is a different state
            // from `blocked` and would make this gate about nothing.
            "text": "stty -icanon -echo; printf '\\033]2;\\342\\234\\263 Claude Code\\007'; \
                     sel=1; \
                     d() { printf '\\033[2J\\033[H'; printf 'Do you want to proceed?\\r\\n'; i=1; \
                     for l in 'Yes' 'Yes, and do not ask again' 'No, and tell me why'; do \
                     if [ \"$i\" = \"$sel\" ]; then printf '\\342\\235\\257 '; else printf '  '; fi; \
                     printf '%s. %s\\r\\n' \"$i\" \"$l\"; i=$((i+1)); done; }; d; \
                     while :; do \
                     k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'); \
                     [ -n \"$k\" ] || exit 0; \
                     case \"$k\" in 49|50|51) sel=$((k-48));; esac; \
                     printf '\\033[2J\\033[H'; printf 'took %s via %s\\r\\n' \"$sel\" \"$k\"; \
                     exec cat; done"
        }),
    );
    server.wait_for_tool("agent_state", json!({ "pane": name }), "state=blocked");
}

/// ⚠⚠⚠ **AN AGENT ANSWERS ITS PEER'S DIALOG FROM THE SURFACE THAT SHOWED IT THE DIALOG** — R369's
/// whole claim, driven end to end through a real daemon, a real pty and a real menu.
///
/// # What only a live run can prove, and what both unit sides missed before
///
/// R366b's lesson is that a unit gate on each side of this supplies the other side's half: the
/// plugin's gates hand it a fixture screen, and the mouth's gates hand it a hand-built outcome. The
/// first end-to-end run through a real daemon found two defects both sets had passed. So this
/// drives the whole path — `tools/call` → the wire → a run on its own thread → a keystroke into a
/// pty → the detector re-reading that pane's screen → the outcome back out — and the assertions are
/// about what the PANE received.
///
/// # ⚠⚠⚠ The safety claim is the second half, and it is the one worth the fixture
///
/// The peer's marker is on option 1 (`Yes`). The caller authorises option 3 (`No, and tell me why`).
/// A machine that answered by pressing Enter — the one key whose landing place is known — would
/// APPROVE the command the caller declined. The pane says which byte moved it, so this gate can
/// tell those apart, and `via 51` is the digit `3`.
///
/// ⚠ REVERT-PROOF: make the answering act send Enter when the marker is elsewhere and the pane
/// reports `took 1 via 10` — a `Yes` nobody authorised.
#[test]
fn an_agent_answers_a_blocked_peer_in_the_agents_own_words() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_asking_pane(&mut server, "asker");

    // ⚠⚠ THE SURFACE THAT SHOWS THE QUESTION POINTS AT THIS TOOL. The debt R369 pays is exactly
    // this sentence: a pane surface that could say what a peer was asking and named a RUN argument
    // as the remedy — which a supervisor reading a neighbour's screen has no run to declare on.
    let seen = server.call_tool("list_panes", json!({}));
    assert!(
        seen.contains("Do you want to proceed?") && seen.contains("answer_pane"),
        "the question, and the tool that answers it, in the same answer: {seen}",
    );

    // ⚠⚠⚠ FIRST, THE REFUSAL — and it goes first because it types NOTHING, which leaves the dialog
    // standing for the second half. `and` is carried by `Yes, and do not ask again` AND by
    // `No, and tell me why`: a grant and a refusal. A first-match policy would pick one of two
    // opposites on the caller's behalf.
    let ambiguous = server.call_tool(
        "answer_pane",
        json!({ "pane": "asker", "asked": "proceed", "answer": "and" }),
    );
    assert!(
        ambiguous.contains("blocked")
            && ambiguous.contains("more than one option carries the authorised answer"),
        "⚠⚠⚠ two options carry the authorised words, so NOTHING is answered and the run says which \
         reason it was — a reader who cannot tell `my needle matched nothing` from `it \
         matched twice` cannot fix either. ⚠ Asserted as the SENTENCE and not the wire word \
         `ambiguous`, because the sentence is what this mouth owes a reader: a reason with no \
         remedy in it is a diagnostic: {ambiguous}",
    );
    let untouched = server.call_tool("read_pane", json!({ "pane": "asker" }));
    assert!(
        !untouched.contains("via 4")
            && !untouched.contains("via 5")
            && !untouched.contains("via 1"),
        "⚠⚠⚠ AND THE PANE IS THE WITNESS: it prints `took <option> via <byte>` for any key it \
         receives, and it must have received none. A refusal that typed something first would be \
         the product deciding and then apologising: {untouched}",
    );
    assert!(
        untouched.contains("Do you want to proceed?"),
        "the dialog is still up, which is what makes the answer below a real second act: \
         {untouched}",
    );

    // ...AND NOW THE ANSWER, naming the option the caller wants in the agent's own words.
    let answered = server.call_tool(
        "answer_pane",
        json!({
            "pane": "asker",
            "asked": "Do you want to proceed?",
            "answer": "No, and tell me why",
        }),
    );
    assert!(
        answered.contains("converged"),
        "the run's whole job was one answer, so it converges rather than looping: {answered}",
    );
    assert!(
        answered.contains("It answered 1 of its peer's questions under your consent"),
        "⚠⚠ and it SAYS a decision was taken on somebody's behalf. A count of approvals is what \
         makes an answer given by a machine auditable rather than merely convenient: {answered}",
    );
    assert!(
        answered.contains("No, and tell me why"),
        "⚠ and the journal names the option in WORDS, not only by number — a number cannot be \
         audited once the dialog is gone: {answered}",
    );

    let witness = server.wait_for_tool("read_pane", json!({ "pane": "asker" }), "took 3 via 51");
    assert!(
        witness.contains("took 3 via 51"),
        "⚠⚠⚠ THE PEER TOOK OPTION 3, MOVED BY THE DIGIT — and the digit is 51, which is `3`. This \
         is the claim the whole contract is for: the peer's own marker was on `Yes`, so a machine \
         that pressed the one key with a known landing place would have APPROVED the command this \
         caller declined: {witness}",
    );
    assert!(
        !witness.contains("via 10"),
        "⚠⚠⚠ AND NO ENTER FOLLOWED IT. The peer left the question on the digit alone, so an Enter \
         sent anyway would have landed on whatever it showed next — which after an approval is \
         frequently a second dialog: {witness}",
    );
}

/// ⚠⚠ **A PERSON'S PANE IS REFUSED, and answering is refused for the reason every other writing
/// tool is** — R340's rule, applied to the door this round opened.
///
/// A new tool on this surface inherits the surface's promises including the ones it does not
/// mention. Answering a dialog TYPES into a pane, so a tool that skipped the ownership check would
/// be a laundering path around `write_pane` and `send_keys`: an agent refused a keystroke could
/// press the person's approval button instead, which is the single most consequential key on the
/// screen.
#[test]
fn answering_a_pane_the_agent_did_not_open_is_refused() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    let theirs = add_pane(&sock, &["cat"]);

    let refused = server.call_tool_error(
        "answer_pane",
        json!({ "pane": 2, "asked": "proceed", "answer": "Yes" }),
    );
    assert!(
        refused.contains("opened by a person, not by you")
            && refused.contains("Only a pane you opened yourself"),
        "the refusal is the surface's ownership rule, in the words every other writing tool \
         refuses in — not a parse error and not a sentence written for this tool alone: {refused}",
    );
    assert!(
        refused.contains("tell them what you would answer"),
        "⚠⚠ and it says the honest alternative. A refusal that leaves an agent with nothing to do \
         is how the next one gets routed around with send_keys: {refused}",
    );
    let _ = theirs;
}

/// ⚠⚠ **BOTH NEEDLES ARE REQUIRED, AND AN INCOMPLETE CONSENT IS REFUSED RATHER THAN DROPPED.**
///
/// The failure this closes is R366's, one surface up: a unit read field-by-field goes MISSING
/// rather than malformed, so a caller who sent half of one would have had the half silently
/// discarded. Here the two needles are flat — the tool has exactly one unit, so there is nothing to
/// group against — and what makes that safe is that neither has a default to fall back to.
///
/// ⚠ The QUESTION needle is the one an agent will be tempted to leave out, and it is the one that
/// matters most: without it a `Yes` written for *"overwrite the draft?"* answers *"delete the
/// production database?"*.
#[test]
fn answering_without_naming_the_question_is_refused() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    server.call_tool("open_pane", json!({ "name": "quiet" }));

    let no_question =
        server.call_tool_error("answer_pane", json!({ "pane": "quiet", "answer": "Yes" }));
    assert!(
        no_question.contains("WHICH QUESTION"),
        "⚠⚠⚠ a consent with no question is a consent to whatever is on the screen: {no_question}",
    );
    let no_option = server.call_tool_error(
        "answer_pane",
        json!({ "pane": "quiet", "asked": "proceed" }),
    );
    assert!(
        no_option.contains("WHICH OPTION"),
        "and one with no option authorises every option, which makes every real menu ambiguous: \
         {no_option}",
    );
    // ⚠ A NUMBER IS NOT A CONSENT, and the tool refuses it rather than reading it as words: the
    // needles are declared as strings, so `answer: 2` never becomes a selection.
    let numbered = server.call_tool_error(
        "answer_pane",
        json!({ "pane": "quiet", "asked": "proceed", "answer": 2 }),
    );
    assert!(
        numbered.contains("WHICH OPTION"),
        "a digit is refused where words are asked for — a number means something different on \
         every screen: {numbered}",
    );
}

/// ⚠⚠ **A PANE THAT IS NOT ASKING IS LEFT ALONE, and the answer says so rather than claiming a
/// success.**
///
/// The race this tool lives inside: an agent reads `blocked`, decides, and calls — and by then the
/// person sitting there may have answered it. A tool that typed the caller's option into whatever
/// the pane had become would be the worst available outcome, and one that reported `converged` with
/// no further word would hide it.
#[test]
fn answering_a_pane_that_is_not_asking_types_nothing_and_says_so() {
    let (_daemon, sock) = spawn_daemon(&["cat"], BOOT_PANE);
    let mut server = McpServer::spawn_in_pane(&sock, 0);
    open_echo_pane(&mut server, "at-rest");

    let answered = server.call_tool(
        "answer_pane",
        json!({ "pane": "at-rest", "asked": "proceed", "answer": "Yes" }),
    );
    assert!(
        answered.contains("is not asking anything"),
        "⚠⚠⚠ the run says WHICH of the two zero-answer endings this is. `converged` alone reads as \
         `your answer went in`, and this pane was never asked a thing: {answered}",
    );
    assert!(
        !answered.contains("It answered 1"),
        "⚠⚠ and the tally does not claim a decision nobody took: {answered}",
    );
    let untouched = server.call_tool("read_pane", json!({ "pane": "at-rest" }));
    assert!(
        !untouched.contains("Yes"),
        "⚠⚠⚠ AND NOT ONE BYTE REACHED THE PANE — it echoes what is typed into it, so the \
         caller's authorised words would be plainly there: {untouched}",
    );
}
