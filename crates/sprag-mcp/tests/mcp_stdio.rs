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
    SELECT_WINDOW_ACTION, SET_FLOATING_ACTION, SPAWN_ACTION, SPLIT_ACTION, SelectAsk,
    ZOOM_PANE_ACTION,
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
fn sprag_term_bin() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_sprag-mcp"))
        .parent()
        .expect("the built sprag-mcp has a directory")
        .join("sprag-term");
    assert!(
        path.exists(),
        "{} is not built. This test drives a binary that belongs to another package, so cargo does \
         not build it for `-p sprag-mcp` alone — run `cargo test --workspace`, or \
         `cargo build -p sprag-host --bins` first.",
        path.display(),
    );
    path
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
            Some(_) => format!("{SOCK_ENV}=\"$1\" \"$0\""),
            None => format!("unset {SOCK_ENV}; \"$0\""),
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
        let mut cmd = Command::new("setsid");
        cmd.arg("--fork")
            .arg(env!("CARGO_BIN_EXE_sprag-mcp"))
            .env_remove(SOCK_ENV);
        let mut server = Self::from_command(cmd);
        let mut shim = server.child.take().expect("the setsid shim is a child");
        let status = shim.wait().expect("wait for the setsid shim to exit");
        assert!(
            status.success(),
            "setsid --fork exited {status}; the server was never orphaned"
        );
        server
    }

    /// Spawn `cmd` with its three streams piped and its readers running.
    ///
    /// The direct child is always held here; a caller whose child is a shim about to be reaped takes
    /// it out itself (see [`Self::spawn_orphaned`]), so `Drop` never reports having killed a process
    /// that had already exited.
    fn from_command(mut cmd: Command) -> Self {
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
        let children = format!("/proc/{shell}/task/{shell}/children");
        let deadline = Instant::now() + DEADLINE;
        loop {
            let listed = std::fs::read_to_string(&children).unwrap_or_else(|error| {
                panic!(
                    "cannot read {children}: {error}. This test needs the kernel's process-children \
                     list (CONFIG_PROC_CHILDREN) to check that the intermediate shell forked."
                )
            });
            if !listed.trim().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the intermediate shell never forked: it exec'd into the server, so no ancestor \
                 carries {SOCK_ENV} and this test would prove less than it claims"
            );
            std::thread::sleep(POLL);
        }
        let environ = std::fs::read(format!("/proc/{shell}/environ"))
            .expect("read the intermediate shell's environ");
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
        !first_text.contains("BRAVO-TWO"),
        "pane 1 did not receive pane 2's text: {first_text}"
    );
    let second_text = server.wait_for_tool_count("read_pane", json!({ "pane": 2 }), "BRAVO-TWO", 2);
    assert!(
        !second_text.contains("ALPHA-ONE"),
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
    assert_eq!(one.lines().count(), 1, "one pane, one line: {one}");
    assert!(
        one.contains("state=blocked"),
        "and it is the right one: {one}"
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

    // The diagnosable answer for "why does my agent pane show nothing": no manifest claims it, so no
    // rule was even consulted.
    let quiet = server.call_tool("agent_explain", json!({ "pane": 2 }));
    assert!(
        quiet.contains("no agent manifest claims this pane") && quiet.contains("[[agent]]"),
        "a pane with no agent is explained as exactly that: {quiet}"
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
        !tail.contains("the-build-is-done"),
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
            "timeout_seconds" => json!(1),
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
        assert!(
            !errored || text.contains("not by you"),
            "{name} did not reach a pane one window over — it must succeed, or refuse on \
             authorship, and it did neither: {text}",
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

    // A name already taken is refused, and the sentence says what to do about it.
    let taken = server.call_tool_error("open_pane", json!({ "name": "build" }));
    assert_eq!(
        taken,
        "Error: could not open a pane called \"build\": the name may already be taken by another \
         pane, or be blank, over 80 bytes, all digits, or contain a control character. Call \
         list_panes to see which names are in use.",
        "the WHOLE sentence: the daemon's refusal is a bare `InvokeRejected` with no payload, so \
         what an agent reads must be written here — and must REPLACE that variant name, not \
         trail it",
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
    server.call_tool(
        "write_pane",
        json!({ "pane": "build", "text": "echo alive" }),
    );
    let read = server.call_tool("read_pane", json!({ "pane": "build" }));
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
        all.contains(&format!("pane {} (id 0) on /dev/pts/", number(0))),
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
        let refused = server.call_tool_error(
            verb,
            json!({ "window": theirs.clone(), "name": "whatever" }),
        );
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

    server.call_tool("write_pane", json!({ "pane": "there", "text": "pwd" }));
    let mut screen = String::new();
    for _ in 0..200 {
        screen = server.call_tool("read_pane", json!({ "pane": "there" }));
        if screen.contains(elsewhere.to_str().expect("a utf-8 temp dir")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        screen.contains(elsewhere.to_str().expect("a utf-8 temp dir")),
        "the shell started where the agent asked: {screen:?}",
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
#[test]
fn a_tool_against_an_older_daemon_says_so() {
    let host = old_host();
    // IN a pane, so the tools that need a caller identity reach the wire instead of stopping at
    // their own pre-flight. The peer serves nothing, so working out which session that pane is in
    // fails too — silently, which is what leaves each sentence below attributable to its own tool.
    let mut server = McpServer::spawn_in_pane(host.sock(), 1);
    let mut wrong = Vec::new();
    for (tool, args) in [
        ("list_panes", json!({})),
        ("read_pane", json!({ "pane": 1 })),
        ("pane_layout", json!({})),
        ("select_pane", json!({ "pane": 1 })),
        ("send_keys", json!({ "pane": 1, "keys": ["Enter"] })),
        ("write_pane", json!({ "pane": 1, "text": "x" })),
        ("open_pane", json!({ "dir": "right" })),
        ("display_message", json!({ "message": "hi" })),
    ] {
        let said = server.call_tool_error(tool, args);
        if said.contains("UnknownIntrospectPath") || said.contains("UnknownInvokePath") {
            wrong.push(format!("{tool} printed a Rust variant name: {said}"));
        } else if !said.contains("older than this") {
            wrong.push(format!("{tool} did not say the daemon is old: {said}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "an agent must be told its daemon predates the tool:\n  {}",
        wrong.join("\n  "),
    );
}
