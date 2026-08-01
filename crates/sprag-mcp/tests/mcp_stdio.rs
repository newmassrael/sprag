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
use sprag_host::wire::SPAWN_ACTION;
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
        moved.contains("pane id="),
        "and it names its subject, so the caller knows what to re-read: {moved}",
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
