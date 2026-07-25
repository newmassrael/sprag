//! Integration test: the `sprag` management CLI drives a real `sprag-term` over the socket.
//!
//! Both binaries are the built artifacts (`CARGO_BIN_EXE_*`), so a break in the wire vocabulary
//! the CLI shares with the daemon — or in the CLI's own output — fails in CI, not by hand.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use sprag_host::mux_action_path;
use sprag_host::wire::{NEW_SESSION_ACTION, PANES_SLOT, SPAWN_ACTION, WINDOWS_SLOT};
use sprag_rpc::{CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, HostConn};

/// Reaps the spawned host process and its socket file on drop — including on a panicked
/// assertion, so a failed run leaks neither.
struct HostChild(Child, PathBuf);
impl Drop for HostChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// A socket path unique to this CALL (pid + a per-binary counter), so parallel test threads in
/// one binary never unlink each other's sockets — the `wire_client` R152/R153 race lesson.
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-cli-it-{}-{n}.sock", std::process::id()))
}

/// Spawn a NON-daemon `sprag-term` serving `sock`, its boot pane running `cat` (which blocks on
/// its PTY, so the session stays live for the test's duration).
fn spawn_host() -> (HostChild, PathBuf) {
    spawn_host_env(&[])
}

/// [`spawn_host`] with EXTRA env vars on the daemon — the ssh test prepends a stand-in `ssh` to the
/// daemon's `PATH`, because the daemon (not the CLI) is what spawns the pane and resolves the program.
fn spawn_host_env(envs: &[(&str, &str)]) -> (HostChild, PathBuf) {
    spawn_host_with(&["cat"], envs)
}

/// [`spawn_host`] whose boot pane runs `program [args…]` instead of `cat` — for a test that needs
/// the pane to have PRINTED something (the search tests), not just to echo.
fn spawn_host_running(program_and_args: &[&str]) -> (HostChild, PathBuf) {
    spawn_host_with(program_and_args, &[])
}

/// The one spawn: the boot command plus any daemon env overrides.
fn spawn_host_with(program_and_args: &[&str], envs: &[(&str, &str)]) -> (HostChild, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg("--")
        .args(program_and_args)
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn the sprag-term host binary");
    (HostChild(child, sock.clone()), sock)
}

/// The result of running the `sprag` CLI: its stdout, its stderr, and whether it exited 0.
struct CliRun {
    stdout: String,
    stderr: String,
    ok: bool,
}

/// Run the `sprag` CLI against `sock`.
fn sprag(sock: &Path, args: &[&str]) -> CliRun {
    sprag_env(sock, args, &[])
}

/// Run the `sprag` CLI against `sock` with EXTRA env vars — the attach test points `SPRAG_GUI_BIN`
/// at a harmless stand-in (`/usr/bin/env`) for the real GUI window, so the launch + env
/// propagation are provable headlessly.
fn sprag_env(sock: &Path, args: &[&str], envs: &[(&str, &str)]) -> CliRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag"));
    cmd.args(args).env("SPRAG_HOST_RPC_SOCK", sock);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("run the sprag CLI");
    CliRun {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        ok: output.status.success(),
    }
}

#[test]
fn the_cli_lists_creates_and_kills_sessions_over_the_socket() {
    let (_host, sock) = spawn_host();

    // ls: the boot session "0" is the one an unscoped request lands in. (The CLI's own connect
    // window absorbs the host's bind race.)
    let run = sprag(&sock, &["ls"]);
    assert!(run.ok, "ls succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("0:") && run.stdout.contains("(default)"),
        "ls shows the boot session as the default: {}",
        run.stdout,
    );

    // new work: creates it and prints the name a client would scope to.
    let run = sprag(&sock, &["new", "work"]);
    assert!(run.ok, "new succeeded: {}", run.stderr);
    assert_eq!(run.stdout.trim(), "work", "new prints the created name");

    // ls now lists it, and "0" is still the default.
    let run = sprag(&sock, &["ls"]);
    assert!(run.stdout.contains("work"), "ls shows it: {}", run.stdout);
    assert!(run.stdout.contains("(default)"), "default unmoved");

    // kill-session work: a non-last kill removes it.
    let run = sprag(&sock, &["kill-session", "work"]);
    assert!(run.ok, "kill-session succeeded: {}", run.stderr);
    assert!(run.stdout.contains("killed work"), "kill-session confirms");

    // ...and it is gone, while the default survives (it was not the last).
    let run = sprag(&sock, &["ls"]);
    assert!(!run.stdout.contains("work"), "the killed session is gone");
    assert!(
        run.stdout.contains("0:"),
        "a non-last kill leaves the default"
    );

    // Killing an unknown session fails cleanly — non-zero exit AND a clean message, not the raw
    // wire error the mapping at `sprag.rs` replaces.
    let run = sprag(&sock, &["kill-session", "ghost"]);
    assert!(!run.ok, "killing an unknown session fails");
    assert!(
        run.stderr.contains("no session named") && !run.stderr.contains("host rpc error"),
        "the refusal is a clean message, not the raw wire error: {}",
        run.stderr,
    );
}

/// `kill-server` ends the daemon: it succeeds, and a follow-up command then finds no server.
#[test]
fn the_cli_kill_server_ends_the_daemon() {
    let (_host, sock) = spawn_host();

    // The server is up.
    assert!(sprag(&sock, &["ls"]).ok, "server up before kill-server");

    // kill-server stops it.
    let run = sprag(&sock, &["kill-server"]);
    assert!(run.ok, "kill-server succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("server stopped"),
        "kill-server confirms"
    );

    // The daemon exits asynchronously after the last kill's reply, so poll `ls` until it can no
    // longer reach a server (a bounded wait, not a fixed sleep — the "a flake is a bug" rule).
    let gone = (0..40).any(|_| {
        if sprag(&sock, &["ls"]).ok {
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        } else {
            true
        }
    });
    assert!(
        gone,
        "kill-server ended the daemon: ls no longer finds a server"
    );
}

/// `kill-server --purge` also ends the daemon and confirms the workspace was purged. (The snapshot
/// deletion itself needs a `--daemon` with a persistent state dir — it is smoke-tested; here we
/// prove the flag is accepted and the end-the-daemon path still works.)
#[test]
fn the_cli_kill_server_purge_ends_the_daemon() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["ls"]).ok, "server up before kill-server");

    let run = sprag(&sock, &["kill-server", "--purge"]);
    assert!(run.ok, "kill-server --purge succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("purged"),
        "kill-server --purge confirms the purge: {}",
        run.stdout,
    );
    let gone = (0..40).any(|_| {
        if sprag(&sock, &["ls"]).ok {
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        } else {
            true
        }
    });
    assert!(gone, "kill-server --purge ended the daemon");
}

/// An unknown `kill-server` argument is refused BEFORE any kill, so a typo cannot end the daemon —
/// the server is still reachable afterwards.
#[test]
fn kill_server_refuses_an_unknown_arg() {
    let (_host, sock) = spawn_host();
    let run = sprag(&sock, &["kill-server", "--force"]);
    assert!(!run.ok, "an unknown kill-server arg is refused");
    assert!(
        run.stderr.contains("--purge"),
        "the error names the only accepted flag: {}",
        run.stderr,
    );
    assert!(
        sprag(&sock, &["ls"]).ok,
        "the refused kill-server did NOT end the daemon",
    );
}

/// The window subcommands drive the SCOPED mux window actions over the socket: list, create +
/// select, select, rename, and kill a window — each `-t SESSION`.
#[test]
fn the_cli_manages_windows_over_the_socket() {
    let (_host, sock) = spawn_host();

    // windows -t 0: the boot session has one window "0", current.
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(run.ok, "windows succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("0 (current)"),
        "one boot window, current: {}",
        run.stdout,
    );

    // new-window -t 0 logs: creates + selects it, printing its name.
    let run = sprag(&sock, &["new-window", "-t", "0", "logs"]);
    assert!(run.ok, "new-window succeeded: {}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        "logs",
        "new-window prints the created name"
    );
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(
        run.stdout.contains("logs (current)"),
        "the new window is listed and current: {}",
        run.stdout,
    );

    // select-window -t 0 0: moves current back to "0".
    assert!(sprag(&sock, &["select-window", "-t", "0", "0"]).ok);
    assert!(
        sprag(&sock, &["windows", "-t", "0"])
            .stdout
            .contains("0 (current)"),
        "the current window moved back to 0",
    );

    // rename-window -t 0 main: renames the CURRENT window ("0") to "main".
    assert!(sprag(&sock, &["rename-window", "-t", "0", "main"]).ok);
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(
        run.stdout.contains("main (current)"),
        "the current window was renamed: {}",
        run.stdout,
    );

    // kill-window -t 0 logs: removes a non-last window.
    assert!(sprag(&sock, &["kill-window", "-t", "0", "logs"]).ok);
    assert!(
        !sprag(&sock, &["windows", "-t", "0"])
            .stdout
            .contains("logs"),
        "the killed window is gone",
    );

    // An unknown session is a clean pre-flight error; a missing -t is an argument error.
    let ghost = sprag(&sock, &["windows", "-t", "ghost"]);
    assert!(!ghost.ok, "windows on a ghost session fails");
    assert!(
        ghost.stderr.contains("no session named"),
        "clean error: {}",
        ghost.stderr,
    );
    let noarg = sprag(&sock, &["windows"]);
    assert!(!noarg.ok, "windows without -t fails");
    assert!(
        noarg.stderr.contains("target session is required"),
        "arg error: {}",
        noarg.stderr,
    );
}

/// break-pane and join-pane MOVE a pane between windows over the CLI (plus the refusal paths). The
/// pane set-up (spawn a second pane, read ids) goes over the wire — the CLI has no pane-spawn verb —
/// while the moves themselves go through the `sprag` binary, so its arg parsing, dispatch, and
/// output are what is under test; the deep behaviour is the registry + `wire_client` tests' job.
#[test]
fn the_cli_breaks_and_joins_panes_over_the_socket() {
    let (_host, sock) = spawn_host();
    let mut c = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");

    // The boot window "0" has one pane; break-pane on the ONLY pane is refused, cleanly.
    let refused = sprag(&sock, &["break-pane", "-t", "0", "0"]);
    assert!(!refused.ok, "breaking the only pane is refused");
    assert!(
        refused.stderr.contains("break-pane refused"),
        "clean refusal: {}",
        refused.stderr,
    );
    // A missing pane id is an argument error.
    let noarg = sprag(&sock, &["break-pane", "-t", "0"]);
    assert!(
        !noarg.ok && noarg.stderr.contains("needs a pane id"),
        "arg error: {}",
        noarg.stderr,
    );

    // Spawn a second pane into window "0" (over the wire) so it has one to break out.
    let extra = spawn_pane(&mut c);
    assert_eq!(pane_count(&mut c), 2, "window 0 now has two panes");

    // break-pane PANE: move it out into a NEW window (born current); the new name prints.
    let broke = sprag(&sock, &["break-pane", "-t", "0", &extra.to_string()]);
    assert!(broke.ok, "break-pane succeeded: {}", broke.stderr);
    let new_window = broke.stdout.trim().to_owned();
    assert!(
        window_names(&mut c).contains(&new_window),
        "the new window {new_window:?} exists: {:?}",
        window_names(&mut c),
    );
    assert_eq!(
        pane_count(&mut c),
        1,
        "the new (current) window holds only the moved pane",
    );

    // join-pane PANE WINDOW: move it back into "0"; the emptied source (the new window) closes.
    let joined = sprag(&sock, &["join-pane", "-t", "0", &extra.to_string(), "0"]);
    assert!(joined.ok, "join-pane succeeded: {}", joined.stderr);
    assert!(
        joined.stdout.contains("source window closed"),
        "the emptied source closed: {}",
        joined.stdout,
    );
    assert!(
        !window_names(&mut c).contains(&new_window),
        "the emptied source window is gone: {:?}",
        window_names(&mut c),
    );
    assert_eq!(pane_count(&mut c), 2, "both panes back in window 0");

    // join-pane to a non-existent window is a clean refusal.
    let ghost = sprag(&sock, &["join-pane", "-t", "0", &extra.to_string(), "nope"]);
    assert!(
        !ghost.ok && ghost.stderr.contains("join-pane refused"),
        "clean refusal: {}",
        ghost.stderr,
    );
}

/// Spawn a `cat` pane into the current window of session "0" over the wire, returning its id.
fn spawn_pane(conn: &mut HostConn) -> u64 {
    conn.call(
        "scene/invoke",
        json!({ "session": "0", "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
    )
    .expect("spawn a pane")
    .as_u64()
    .expect("spawn returns the pane id")
}

/// How many panes the current window of session "0" holds, over the mux `panes` slot.
fn pane_count(conn: &mut HostConn) -> usize {
    conn.call(
        "scene/query",
        json!({ "session": "0", "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()
    .and_then(|v| v.as_array().map(Vec::len))
    .unwrap_or(0)
}

/// The window names of session "0", over the mux `windows` slot.
fn window_names(conn: &mut HostConn) -> Vec<String> {
    conn.call(
        "scene/query",
        json!({ "session": "0", "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|w| w["name"].as_str().map(str::to_owned))
                .collect()
        })
    })
    .unwrap_or_default()
}

/// Killing a session's LAST window ends the SESSION (tmux) — driven over the CLI: `kill-window`
/// with no target kills the current (only) window, and the session is then gone.
#[test]
fn the_cli_kill_window_on_the_last_window_ends_the_session() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "created a session");

    // kill-window with no window ⇒ the current (and only) one; its removal ends the session.
    let run = sprag(&sock, &["kill-window", "-t", "work"]);
    assert!(
        run.ok,
        "kill-window on the last window succeeded: {}",
        run.stderr
    );

    let ls = sprag(&sock, &["ls"]);
    assert!(
        !ls.stdout.contains("work"),
        "the session went with its last window: {}",
        ls.stdout,
    );
    assert!(
        ls.stdout.contains("0:"),
        "the default survives (work was not the last session)",
    );
}

/// `attach` PRE-FLIGHTS (does the session exist?) over the same connect-only path, then launches
/// the GUI scoped to that session and pinned to THIS daemon's socket. `/usr/bin/env` stands in for
/// the real GUI window (prints its env, exits 0), so the launch + env propagation are provable
/// without a display.
#[test]
fn the_cli_attach_preflights_then_launches_the_gui_scoped_to_the_session() {
    let (_host, sock) = spawn_host();
    assert!(
        sprag(&sock, &["new", "work"]).ok,
        "created a session to attach to"
    );

    // A missing session is a CLEAN pre-flight error — the "no session" message, NOT a
    // "could not launch sprag-gui" one, which proves no GUI was launched on the bad name.
    let bad = sprag_env(
        &sock,
        &["attach", "ghost"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(!bad.ok, "attach to a missing session fails");
    assert!(
        bad.stderr.contains("no session named"),
        "clean pre-flight error, no gui launch: {}",
        bad.stderr,
    );

    // No name is an argument error.
    let noarg = sprag(&sock, &["attach"]);
    assert!(!noarg.ok, "attach with no name fails");
    assert!(
        noarg.stderr.contains("needs a session name"),
        "arg error: {}",
        noarg.stderr,
    );

    // A real session: the pre-flight passes and the GUI is launched with the session and THIS
    // socket in its env (the stand-in inherits the CLI's stdout, so its env dump is captured here).
    let ok = sprag_env(
        &sock,
        &["attach", "work"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(
        ok.ok,
        "attach to a real session launches the gui and succeeds: {}",
        ok.stderr
    );
    assert!(
        ok.stdout.contains("SPRAG_GUI_SESSION=work"),
        "the gui is handed the session to adopt: {}",
        ok.stdout,
    );
    assert!(
        ok.stdout
            .contains(&format!("SPRAG_GUI_HOST_SOCK={}", sock.display())),
        "the gui is pinned to THIS daemon's socket, not a default: {}",
        ok.stdout,
    );
}

/// `list-clients` + the `ls` attached count, END TO END over the real socket (R-PR67): the CLI
/// reads the daemon's live per-client attachment state, so this pins the CLI's parse + wire read +
/// formatting against a REAL attached client — not a mocked slot.
///
/// The test itself opens a `HostConn`, announces a client id (`client/hello`) and attaches to the
/// default session (`client/attach`), then holds that connection open across the CLI runs. While
/// it is held: `sprag ls` shows `(1 attached)` on that session, `sprag list-clients` lists the
/// client -> session line, and `-t` filters by session (a match keeps it, an unknown session is a
/// clean pre-flight error). When the connection DROPS, the daemon releases the attachment
/// (`on_disconnect`, crash-safe), and `list-clients` empties + `ls` loses the badge — polled,
/// because the release is delivered asynchronously on the daemon's reader thread.
#[test]
fn the_cli_lists_attached_clients_and_shows_the_attached_count() {
    let (_host, sock) = spawn_host();

    // With no attached client, list-clients is empty (exit 0) and ls carries no badge.
    let empty = sprag(&sock, &["list-clients"]);
    assert!(
        empty.ok,
        "list-clients with no clients succeeds: {}",
        empty.stderr
    );
    assert!(
        empty.stdout.trim().is_empty(),
        "no clients ⇒ no lines: {:?}",
        empty.stdout,
    );
    assert!(
        !sprag(&sock, &["ls"]).stdout.contains("attached"),
        "an unviewed session shows no attached badge",
    );

    {
        // Attach a real client to the default session "0" and HOLD the connection open.
        let mut attacher =
            HostConn::connect(&sock, Duration::from_secs(5)).expect("attacher connects");
        attacher
            .call(
                CLIENT_HELLO_METHOD,
                json!({ CLIENT_PARAM: "cli-test-client" }),
            )
            .expect("client/hello accepted");
        attacher
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach accepted");

        // The attach is delivered on the daemon's dispatch thread; poll the CLI until it shows.
        assert!(
            wait_for(Duration::from_secs(5), || {
                sprag(&sock, &["list-clients"])
                    .stdout
                    .contains("cli-test-client: 0")
            }),
            "list-clients lists the attached client and its session",
        );

        let ls = sprag(&sock, &["ls"]);
        assert!(
            ls.stdout.contains("(1 attached)"),
            "ls shows the attached count on the viewed session: {}",
            ls.stdout,
        );

        // -t filters by session: the default "0" keeps the client...
        let matched = sprag(&sock, &["list-clients", "-t", "0"]);
        assert!(matched.ok, "list-clients -t 0 succeeds: {}", matched.stderr);
        assert!(
            matched.stdout.contains("cli-test-client: 0"),
            "the client attached to 0 survives the -t 0 filter: {}",
            matched.stdout,
        );
        // ...and an unknown session is a clean pre-flight error, not an empty success.
        let unknown = sprag(&sock, &["list-clients", "-t", "ghost"]);
        assert!(!unknown.ok, "list-clients -t <missing> fails");
        assert!(
            unknown.stderr.contains("no session named"),
            "a clean pre-flight error: {}",
            unknown.stderr,
        );

        // attacher drops here: its socket closes with no explicit detach.
    }

    // The released attachment must empty the listing and drop the badge (crash-safe on_disconnect).
    assert!(
        wait_for(Duration::from_secs(5), || {
            sprag(&sock, &["list-clients"]).stdout.trim().is_empty()
                && !sprag(&sock, &["ls"]).stdout.contains("attached")
        }),
        "closing the connection releases the attachment, not leaks it",
    );
}

/// Poll `predicate` until it holds or `timeout` elapses. The CLI's per-client attachment reads
/// depend on the daemon's async `on_disconnect`, so a populated/emptied assertion is polled, not
/// asserted once.
fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    predicate()
}

/// `sprag find` end to end: the CLI sweeps the session's panes through the host's `find.<needle>`
/// query and prints each matching line as `PANE:LINE: text`.
///
/// The boot pane prints two lines, one of which matches twice — so this pins the two properties a
/// grep-shaped output has to get right: the matching line appears ONCE (deduped, not once per
/// match), and the non-matching line does not appear at all. The needle carries a SPACE, which also
/// proves the path-carried argument survives the wire verbatim from a shell argument.
#[test]
fn the_cli_find_prints_matching_lines_from_the_session() {
    let (_host, sock) =
        spawn_host_running(&["sh", "-c", "printf 'a hit and a hit\\nquiet\\n'; exec cat"]);

    // The pane's output is asynchronous, so poll the search itself until it sees the line.
    let mut run = sprag(&sock, &["find", "a hit"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !run.stdout.contains("a hit and a hit") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        run = sprag(&sock, &["find", "a hit"]);
    }
    assert!(run.ok, "sprag find succeeded: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["0:0: a hit and a hit"],
        "one line per MATCHING line — deduped, and the quiet line is absent: {:?}",
        run.stdout,
    );

    // A needle nothing matches is not an error: it prints nothing and exits 0, so a script can
    // tell "the search ran" from "something broke".
    let empty = sprag(&sock, &["find", "zzz-no-such-text"]);
    assert!(empty.ok, "a search with no matches still succeeds");
    assert!(
        empty.stdout.is_empty(),
        "and prints nothing: {:?}",
        empty.stdout
    );

    // A missing needle is a clean local error, before any request.
    let bare = sprag(&sock, &["find"]);
    assert!(!bare.ok, "find with no needle fails");
    assert!(
        bare.stderr.contains("needle is required"),
        "with a clear message: {}",
        bare.stderr,
    );
}

/// Wait until the stand-in's recorded argv carries EVERY token in `expected`/// Wait until the stand-in's recorded argv carries EVERY token in `expected`, panicking with what it
/// did record if it never does.
///
/// Polling `argv_file.exists()` and then reading it is a race, not a wait: the stub's
/// `printf … > file` CREATES (truncates) the file at the redirect, before it writes a single byte, so
/// an existence poll can hand the assertion an empty file. The window is narrow enough to pass alone
/// and wide enough to fail under a loaded parallel run — which is how it surfaced. Waiting on the
/// condition the assertion actually reads removes the race instead of widening the timeout.
fn wait_for_recorded_argv(argv_file: &Path, expected: &[&str]) {
    let mut recorded = String::new();
    let complete = wait_for(Duration::from_secs(5), || {
        recorded = std::fs::read_to_string(argv_file).unwrap_or_default();
        expected
            .iter()
            .all(|token| recorded.lines().any(|line| line == *token))
    });
    assert!(
        complete,
        "the stand-in never recorded the argv {expected:?}; it recorded {recorded:?}",
    );
}

/// A temp directory removed on drop (including on a panicked assertion) — holds an ssh test's
/// stand-in `ssh` and the argv it records, so a failed run leaves nothing behind.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Create a temp dir holding an executable stand-in `ssh` that records its exec argv (one token per
/// line) to `<dir>/argv.txt`, then runs the shell fragment `tail(dir)` — `exec cat` to block like a
/// live pane, or a listener to simulate `ssh -L`. `tail` receives the dir so it can reference sibling
/// files by absolute path (the pane is exec'd via PATH, so `$0`-relative paths are unreliable).
/// Returns the drop guard, the dir (prepend to the daemon PATH), and the argv-record path.
fn stub_ssh(label: &str, tail: impl FnOnce(&Path) -> String) -> (TempDir, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("sprag-ssh-it-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the stand-in ssh dir");
    let argv_file = dir.join("argv.txt");
    let ssh = dir.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{}\n",
            argv_file.display(),
            tail(&dir),
        ),
    )
    .expect("write the stand-in ssh");
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
    (TempDir(dir.clone()), dir, argv_file)
}

/// `sprag ssh` end to end over the socket. The CLI parses `me@server -p 2222`, builds the `ssh -t …`
/// argv ([`sprag_host::SshTarget`]), and the daemon spawns it as the birth pane of a FRESH session —
/// no ssh-awareness anywhere on the wire, it rides the existing `new_session {cmd}` action. The
/// stand-in `ssh` records the exact argv it is exec'd with, then blocks (`exec cat`) so the pane
/// stays live and the assertion is deterministic (not racing a real ssh's connect-and-die). This
/// pins the WHOLE chain — parse → `ssh_argv` → `new_session {cmd}` → `build_command` → PTY exec —
/// reaching exec with the real arguments; the argv *shape* itself is the [`sprag_host::ssh`] unit
/// tests' job.
#[test]
fn the_cli_ssh_launches_a_remote_pane_with_the_ssh_argv() {
    let (_tmp, dir, argv_file) = stub_ssh("launch", |_| "exec cat".to_owned());

    // The DAEMON spawns the pane, so the stand-in must be first on ITS PATH.
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    // ssh me@server -p 2222: creates a session, printing the allocated name to scope a client to.
    let run = sprag(&sock, &["ssh", "me@server", "-p", "2222"]);
    assert!(run.ok, "sprag ssh succeeded: {}", run.stderr);
    let name = run.stdout.trim().to_owned();
    assert!(
        !name.is_empty() && name != "0",
        "ssh prints a fresh allocated session name: {name:?}",
    );

    // The birth pane exec'd the stand-in ssh with the real argv (async spawn — poll for the record).
    wait_for_recorded_argv(&argv_file, &["-t", "-p", "2222", "me@server"]);

    // The live remote pane keeps the session listable (panes > 0), like any other workspace.
    assert!(
        sprag(&sock, &["ls"]).stdout.contains(&name),
        "the ssh session lists as a live workspace",
    );
}

/// `sprag ssh -L` reaches exec with the forwards ssh's SSOT rendered — the one-field shorthand
/// `3000` EXPANDED to `3000:localhost:3000`, the three-field spec verbatim. Deterministic (`exec cat`
/// stand-in, no listener), so it pins the CLI→argv→exec path for forwards; the live "the forward
/// surfaces in the sidebar" composition is the next test.
#[test]
fn the_cli_ssh_passes_local_forwards_to_exec() {
    let (_tmp, dir, argv_file) = stub_ssh("fwd", |_| "exec cat".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    let run = sprag(&sock, &["ssh", "host", "-L", "3000", "-L", "8080:db:5432"]);
    assert!(run.ok, "sprag ssh -L succeeded: {}", run.stderr);

    wait_for_recorded_argv(&argv_file, &["-L", "3000:localhost:3000", "8080:db:5432"]);
}

/// The headline of Slice 2, LIVE: a remote workspace's forwarded local port shows up in the session
/// sidebar (`sprag ls`). Real `ssh -L PORT:…` opens a local listener on the ssh process; here a
/// python stand-in binds that same local port and holds it, so the daemon's per-pane `/proc` port
/// scan attributes it to the ssh session — no ssh-specific code in the scan, an ssh pane is just a
/// pane. Proves the composition end to end (`/proc` attribution itself is unit-covered in
/// `sprag_terminal::ports`). Linux-only, like the port scan; needs python3 to hold the socket.
#[cfg(target_os = "linux")]
#[test]
fn the_cli_ssh_forward_surfaces_the_local_port_in_the_sidebar() {
    let has_python = Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !has_python {
        eprintln!(
            "skipping ssh-forward-surfaces: python3 is needed to hold the forwarded listener"
        );
        return;
    }

    // A free local port: bind :0 to learn a number, then release it (SO_REUSEADDR lets python rebind).
    let free_port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral loopback port")
        .local_addr()
        .unwrap()
        .port();

    // The stand-in ssh execs a python listener on $LISTEN_PORT (what real `ssh -L` would bind).
    let (_tmp, dir, argv_file) = stub_ssh("surface", |dir| {
        format!(
            "exec python3 '{}' \"$LISTEN_PORT\"",
            dir.join("listener.py").display(),
        )
    });
    std::fs::write(
        dir.join("listener.py"),
        "import socket, sys, signal\n\
         s = socket.socket()\n\
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
         s.bind((\"127.0.0.1\", int(sys.argv[1])))\n\
         s.listen()\n\
         signal.pause()\n",
    )
    .expect("write the python listener");

    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path), ("LISTEN_PORT", &free_port.to_string())]);

    // Forward the free local port to the remote's :22 (the remote target is irrelevant to the LOCAL
    // listen this test observes).
    let spec = format!("{free_port}:localhost:22");
    let run = sprag(&sock, &["ssh", "host", "-L", &spec]);
    assert!(run.ok, "sprag ssh -L succeeded: {}", run.stderr);
    let name = run.stdout.trim().to_owned();

    // The -L reached exec (deterministic argv record).
    wait_for_recorded_argv(&argv_file, &[&spec]);

    // The live composition: the forwarded port, held by the ssh (python) process in the pane's
    // subtree, surfaces on THIS session's `sprag ls` line. Polled — the listener binds asynchronously
    // and the port scan is a live per-read.
    let badge = format!(":{free_port}");
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["ls"])
                .stdout
                .lines()
                .any(|line| line.starts_with(&format!("{name}:")) && line.contains(&badge))
        }),
        "the forwarded port {free_port} surfaces on the ssh session in `sprag ls`",
    );
}

/// The `--tmux` preset reaches exec as `tmux new-session -A -s NAME` (attach-or-create), and clashing
/// it with a `--` remote command is a clean LOCAL error surfaced before anything is sent — the parse
/// runs ahead of the connect.
#[test]
fn the_cli_ssh_tmux_preset_reaches_exec_and_rejects_a_conflict() {
    let (_tmp, dir, argv_file) = stub_ssh("tmux", |_| "exec cat".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    // --tmux=work: the birth pane execs the attach-or-create tmux command on the remote.
    let run = sprag(&sock, &["ssh", "host", "--tmux=work"]);
    assert!(run.ok, "sprag ssh --tmux succeeded: {}", run.stderr);
    wait_for_recorded_argv(&argv_file, &["tmux", "new-session", "-A", "-s", "work"]);

    // --tmux together with a -- command is a clean, non-zero local error (nothing spawned).
    let clash = sprag(&sock, &["ssh", "host", "--tmux", "--", "vim"]);
    assert!(!clash.ok, "combining --tmux and a -- command fails");
    assert!(
        clash.stderr.contains("--tmux") && clash.stderr.contains("not both"),
        "a clean conflict message: {}",
        clash.stderr,
    );
}

/// `--pane` narrows the sweep to ONE pane, and naming a pane the window does not hold is a clean
/// ERROR rather than an empty result.
///
/// The asymmetry is the point: finding no matches for a needle IS the answer, but finding no
/// matches for a pane that is not there answers a question the caller did not ask. Two panes print
/// the SAME needle so the filter has something to exclude — a one-pane fixture would pass whether
/// the filter worked or not.
#[test]
fn the_cli_find_narrows_to_one_pane_and_rejects_an_absent_one() {
    let printer = "printf 'shared marker\\n'; exec cat";
    let (_host, sock) = spawn_host_running(&["sh", "-c", printer]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let second: u64 = conn
        .call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["sh", "-c", printer] },
            }),
        )
        .expect("spawn a second pane")
        .as_u64()
        .expect("spawn returns the pane id");

    // Both panes match, once the second one has printed (its output is asynchronous).
    let mut all = sprag(&sock, &["find", "shared marker"]);
    let both = wait_for(Duration::from_secs(5), || {
        all = sprag(&sock, &["find", "shared marker"]);
        all.stdout.lines().count() == 2
    });
    assert!(both, "both panes matched: {:?}", all.stdout);

    // --pane keeps only that pane's lines.
    let only = sprag(
        &sock,
        &["find", "shared marker", "--pane", &second.to_string()],
    );
    assert!(only.ok, "find --pane succeeded: {}", only.stderr);
    assert_eq!(
        only.stdout.trim_end(),
        format!("{second}:0: shared marker"),
        "only the named pane's line: {:?}",
        only.stdout,
    );

    // An absent pane is an error that says what IS there, not an empty success.
    let absent = sprag(&sock, &["find", "shared marker", "--pane", "9999"]);
    assert!(!absent.ok, "an absent pane fails");
    assert!(
        absent.stderr.contains("no pane 9999"),
        "and names it: {}",
        absent.stderr,
    );

    // A non-numeric --pane is rejected locally, before any request reaches the daemon.
    let bad = sprag(&sock, &["find", "shared marker", "--pane", "abc"]);
    assert!(!bad.ok, "a non-numeric pane id fails");
    assert!(
        bad.stderr.contains("is not a pane id"),
        "with a clear message: {}",
        bad.stderr,
    );
}

/// `--regex` sends a DIFFERENT query, not the same one with a flag: the same argument matches
/// different things under the two languages, and an invalid pattern is an error rather than a
/// silently empty result.
///
/// The fixture prints `axb` and `a.b` on one line so the literal and pattern readings of `a.b` are
/// distinguishable — one match versus two. A fixture where both readings agree would pass whether
/// the flag reached the wire or not.
#[test]
fn the_cli_find_regex_reads_the_needle_as_a_pattern() {
    let (_host, sock) = spawn_host_running(&["sh", "-c", "printf 'axb a.b\\n'; exec cat"]);

    // Literal: only the real dot matches, so ONE line, printed once.
    let mut literal = sprag(&sock, &["find", "a.b"]);
    let printed = wait_for(Duration::from_secs(5), || {
        literal = sprag(&sock, &["find", "a.b"]);
        literal.stdout.contains("axb a.b")
    });
    assert!(printed, "the pane printed: {:?}", literal.stdout);
    assert_eq!(literal.stdout.trim_end(), "0:0: axb a.b");

    // As a pattern the dot matches any character — the same LINE, so the grep-shaped output is
    // still one line. What proves the language changed is a pattern that is not literal text.
    let pattern = sprag(&sock, &["find", "--regex", "^a.b a"]);
    assert!(pattern.ok, "find --regex succeeded: {}", pattern.stderr);
    assert_eq!(
        pattern.stdout.trim_end(),
        "0:0: axb a.b",
        "an anchored pattern matched: {:?}",
        pattern.stdout,
    );
    // …and the same string as a LITERAL matches nothing, so the flag really did change the query.
    let as_literal = sprag(&sock, &["find", "^a.b a"]);
    assert!(
        as_literal.ok && as_literal.stdout.is_empty(),
        "literally absent"
    );

    // An invalid pattern is an error naming the reason, not an empty success.
    let bad = sprag(&sock, &["find", "--regex", "a(b"]);
    assert!(!bad.ok, "an invalid pattern fails");
    assert!(
        bad.stderr.contains("invalid pattern"),
        "with the engine's reason: {}",
        bad.stderr,
    );
    // The SAME string is a perfectly good literal needle, which is exactly the point.
    let fine = sprag(&sock, &["find", "a(b"]);
    assert!(
        fine.ok,
        "the same string is a valid literal needle: {}",
        fine.stderr
    );
}

// ---------------------------------------------------------------------------
// The durability ring, end to end: a daemon that dies gives its panes back WITH
// their scrollback.
// ---------------------------------------------------------------------------

/// Kills whatever daemon is serving `sock` and removes the socket and the state directory —
/// including on a panicked assertion, so a failed run leaks neither a process nor a temp tree.
struct DaemonGuard {
    sock: PathBuf,
    state: PathBuf,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(pid) = daemon_pid(&self.sock) {
            kill_daemon(pid);
        }
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

/// The pid of the `sprag-term` daemon serving `sock`.
///
/// It has to be FOUND rather than remembered: a `--daemon` forks and the parent exits, so the
/// process the test spawned is a short-lived intermediate, not the daemon. Matching on the
/// environment rather than the command line because the socket travels as `SPRAG_HOST_RPC_SOCK`,
/// and that value is unique per test call — so this cannot pick up a sibling test's daemon the way
/// a `pkill -f sprag-term` would.
fn daemon_pid(sock: &Path) -> Option<u32> {
    let want = format!("SPRAG_HOST_RPC_SOCK={}", sock.display());
    let me = std::process::id();
    std::fs::read_dir("/proc")
        .ok()?
        .flatten()
        .find_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if pid == me {
                return None;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            if comm.trim() != "sprag-term" {
                return None;
            }
            let environ = std::fs::read(entry.path().join("environ")).ok()?;
            environ
                .split(|byte| *byte == 0)
                .any(|value| value == want.as_bytes())
                .then_some(pid)
        })
}

/// SIGKILL `pid` and wait for it to be gone — the reboot analogue. Deliberately not a graceful
/// `kill-server`: the ring exists for the case where the daemon gets NO chance to tidy up, and a
/// clean shutdown would also race its own teardown against the save loop.
fn kill_daemon(pid: u32) {
    // SAFETY: `pid` was just read from /proc for a process this test spawned.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    wait_for(Duration::from_secs(5), || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
}

/// Launch a `--daemon` on `sock` keeping its durable state under `state`. Returns once the
/// intermediate parent has forked and exited; the caller waits for the socket to answer.
fn spawn_daemon(sock: &Path, state: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg("--daemon")
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("SPRAG_HOST_RPC", "1")
        .env("XDG_STATE_HOME", state)
        .stdin(Stdio::null())
        .status()
        .expect("spawn the sprag-term daemon");
    assert!(status.success(), "the daemon's parent forked cleanly");
}

/// Whether any DURABLE saved pane history under `state` contains `needle`.
///
/// "Durable" is the whole point, and it is why the file filter is
/// [`sprag_host::history_file_pane`] rather than a hand-rolled one: the atomic write leaves
/// `<id>.hist.tmp` in the same directory while it is in flight, and an unfiltered scan matches THAT.
/// A caller waiting on this to decide the history is safe would then kill the daemon between the
/// temp write and the rename — the pane comes back with no history at all, and the successor's own
/// save loop overwrites the file with its fresh prompt, erasing the evidence.
///
/// That was a real ~15% flake in
/// `a_killed_daemon_gives_its_panes_back_with_their_scrollback`, found by dumping the bytes: the
/// needle was sitting in `0.hist.tmp`. Waiting on the file a RESTORE would actually read is the
/// condition the assertion depends on — the rule this helper had stated in its caller's comment and
/// then not implemented.
fn saved_history_contains(state: &Path, needle: &str) -> bool {
    let Ok(dirs) = std::fs::read_dir(state.join("sprag")) else {
        return false;
    };
    dirs.flatten()
        .filter(|dir| dir.path().extension().is_some_and(|e| e == "history"))
        .flat_map(|dir| {
            std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter(|file| sprag_host::history_file_pane(&file.path()).is_some())
        .any(|file| {
            std::fs::read(file.path())
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
        })
}

/// THE reboot payoff, end to end: a pane prints something, the daemon is KILLED outright, and the
/// next daemon on the same socket brings the pane back with its scrollback — provable because
/// `sprag find` still finds text the pane printed before the crash.
///
/// This is the one test that exercises the whole ring at once: the save loop's timer, the encoding,
/// the per-pane file, the restore's replay-before-the-reader seam, and the search that reads the
/// result. The pane comes back as a plain SHELL (a recorded `sh -c` is never re-run), so the text
/// it finds can only have come from the restored history, never from re-running the command.
///
/// Linux-gated: it finds the forked daemon through `/proc`.
#[test]
#[cfg(target_os = "linux")]
fn a_killed_daemon_gives_its_panes_back_with_their_scrollback() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-durability-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    // Unique per run, so a needle can never be found in another test's leftovers.
    let needle = format!("PERSISTED-SCROLLBACK-{}", std::process::id());

    // Daemon A, and a session whose pane PRINTS the needle then blocks on its pty.
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "work",
                "cmd": ["sh", "-c", format!("printf '{needle}\\n'; exec cat")],
            },
        }),
    )
    .expect("new_session answers");
    drop(conn);

    // Wait on the CONDITION the assertion reads: the needle is actually ON DISK. The save loop is
    // on a timer, so polling anything else here would be a race dressed as a wait.
    assert!(
        wait_for(Duration::from_secs(30), || saved_history_contains(
            &state, &needle
        )),
        "the daemon never persisted the pane's scrollback under {}",
        state.display(),
    );

    // The reboot: kill the daemon outright, then start its successor on the same socket + state.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    // The payoff: the restored pane is searchable over output its predecessor produced.
    let mut run = sprag(&sock, &["find", "-t", "work", &needle]);
    let found = wait_for(Duration::from_secs(15), || {
        run = sprag(&sock, &["find", "-t", "work", &needle]);
        run.stdout.contains(&needle)
    });
    assert!(
        found,
        "the restored pane lost its scrollback (stdout {:?}, stderr {:?})",
        run.stdout, run.stderr,
    );
    drop(guard);
}
