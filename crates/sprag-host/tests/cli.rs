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

/// `--no-wait` returns on the window ATTACHING, not on merely having been spawned — so a window
/// that dies on startup is still this command's failure. That is the whole reason the flag waits
/// for the daemon to witness the client rather than exiting 0 the moment `spawn` succeeds.
///
/// `/usr/bin/env` is the stand-in for exactly that failure: it starts, prints, and exits without
/// ever speaking to the daemon — indistinguishable, from the CLI's side, from a real window that
/// cannot open a display. The pass condition is that the CLI NOTICES.
///
/// The success path needs a window that really attaches, so it is proven live rather than here
/// (a stand-in that speaks the client handshake would be asserting against a mock of the thing
/// under test); measured 4/4 attaching in 0.13-0.22s.
#[test]
fn attach_no_wait_reports_a_window_that_exits_before_attaching() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag_env(
        &sock,
        &["attach", "work", "--no-wait"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(
        !run.ok,
        "a window that never attached is a FAILURE, not a silent exit 0: {}",
        run.stdout,
    );
    assert!(
        run.stderr.contains("before its window attached"),
        "and it is diagnosed as such, not as a timeout: {}",
        run.stderr,
    );
}

/// An unknown flag is refused rather than swallowed as the session name — otherwise a typo
/// (`--nowait`) would attach to a session by that name, or fail with a confusing "no session".
#[test]
fn attach_refuses_an_unknown_argument() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag(&sock, &["attach", "work", "--nowait"]);
    assert!(!run.ok, "a misspelled flag fails");
    assert!(
        run.stderr.contains("unexpected argument"),
        "named as the argument error it is: {}",
        run.stderr,
    );
}

/// `attach --tui` PRE-FLIGHTS like the window client, then launches the TERMINAL client with the
/// same session + socket env. `/usr/bin/env` stands in for `sprag-tui` (prints its env, exits 0),
/// which is what makes the launch provable without a terminal to take.
///
/// `SPRAG_GUI_BIN` is pointed at `/bin/false` throughout, and that is the half of this test that
/// distinguishes "the flag chose the terminal client" from "a client was launched" — a pass here
/// cannot be produced by launching the wrong one. MEASURED, by making the launch resolve
/// `SPRAG_GUI_BIN` whatever the flag says: exit 1 with NOTHING on either stream, because `exec`
/// leaves no CLI behind to say what it launched. The silence is the point — the flag is not
/// something a diagnostic could recover from being wrong about.
#[test]
fn the_cli_attach_tui_launches_the_terminal_client_scoped_to_the_session() {
    let (_host, sock) = spawn_host();
    assert!(
        sprag(&sock, &["new", "work"]).ok,
        "created a session to attach to"
    );
    let clients = [
        ("SPRAG_TUI_BIN", "/usr/bin/env"),
        ("SPRAG_GUI_BIN", "/bin/false"),
    ];

    // The pre-flight is the terminal client's too: a missing session is the "no session" error,
    // NOT a launch failure — which is what proves nothing was exec'd on a bad name.
    let bad = sprag_env(&sock, &["attach", "ghost", "--tui"], &clients);
    assert!(!bad.ok, "attach --tui to a missing session fails");
    assert!(
        bad.stderr.contains("no session named"),
        "clean pre-flight error, no client launched: {}",
        bad.stderr,
    );

    let ok = sprag_env(&sock, &["attach", "work", "--tui"], &clients);
    assert!(
        ok.ok,
        "attach --tui to a real session launches the terminal client: {} / {}",
        ok.stdout, ok.stderr,
    );
    assert!(
        ok.stdout.contains("SPRAG_GUI_SESSION=work"),
        "the terminal client is handed the session to adopt: {}",
        ok.stdout,
    );
    assert!(
        ok.stdout
            .contains(&format!("SPRAG_GUI_HOST_SOCK={}", sock.display())),
        "and pinned to THIS daemon's socket, not a default: {}",
        ok.stdout,
    );
}

/// `--no-wait` is refused with `--tui`, by NAME, rather than accepted and ignored.
///
/// The flag exists to hand a shell back once a window is up. A terminal client holds this terminal
/// until it detaches, so there is no such moment — and accepting it silently would promise a
/// prompt that never returns.
#[test]
fn attach_tui_refuses_no_wait() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag_env(
        &sock,
        &["attach", "work", "--tui", "--no-wait"],
        &[("SPRAG_TUI_BIN", "/usr/bin/env")],
    );
    assert!(!run.ok, "--no-wait with a terminal client fails");
    assert!(
        run.stderr
            .contains("--no-wait belongs to the window client"),
        "refused by name, with the reason: {}",
        run.stderr,
    );
}

/// `attach --remote HOST NAME` runs the terminal client ON HOST: it execs
/// `ssh -t HOST sprag attach --tui NAME` and never opens a local connection.
///
/// Both halves are asserted by ONE arrangement — no daemon is spawned at all, and the session
/// named exists nowhere. If the local pre-flight ran, this could only fail (there is no daemon on
/// that socket to answer, and no session by that name if there were), so reaching the recorded
/// argv IS the proof that `--remote` is answered before any of it.
///
/// The argv is compared as an exact SEQUENCE rather than a set of tokens: `sprag attach --tui
/// ghost` and `sprag --tui attach ghost` carry the same tokens and only one of them is a command.
/// It needs no polling either — `exec` makes the stand-in this process, so the CLI run returning
/// IS the stand-in having finished writing.
#[test]
fn the_cli_attach_remote_execs_ssh_and_never_touches_a_local_daemon() {
    let (_tmp, dir, argv_file) = stub_ssh("attach-remote", |_| "exit 0".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // A socket nothing serves: the local daemon is not merely unused here, it does not exist.
    let sock = socket_path();

    let run = sprag_env(
        &sock,
        &["attach", "ghost", "--remote", "me@server"],
        &[("PATH", path.as_str())],
    );
    assert!(
        run.ok,
        "attach --remote needs no local daemon and no local session: {}",
        run.stderr,
    );

    let recorded: Vec<String> = std::fs::read_to_string(&argv_file)
        .expect("the stand-in ssh recorded its argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        recorded,
        vec!["-t", "me@server", "sprag", "attach", "--tui", "ghost"],
        "the remote command is the terminal client, named, under a forced pty",
    );
    assert!(
        !sock.exists(),
        "no daemon was connected to or spawned on the local socket",
    );
}

/// `--remote` with no host is a clean argument error, not a host silently read off the next thing
/// in the line (which would be the session name, producing an ssh to a machine named for it).
#[test]
fn attach_remote_needs_a_host() {
    let (_host, sock) = spawn_host();

    let run = sprag(&sock, &["attach", "work", "--remote"]);
    assert!(!run.ok, "a valueless --remote fails");
    assert!(
        run.stderr.contains("--remote needs a host"),
        "with a message naming the missing value: {}",
        run.stderr,
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

/// THE ghostty-parity payoff: a killed daemon gives a pane back WITH ITS INLINE IMAGE — same id, same
/// extent, same anchor cell — not merely with the text around it.
///
/// This is the axis no other multiplexer covers. tmux and cmux persist no image; ghostty renders
/// images but has no session persistence at all, so it has nothing to restore them into. sprag stores
/// the image as Kitty transmit bytes in the same `.hist` stream as the text, which is why it costs no
/// second file, no second lifecycle and — the load-bearing part — no second decoder: the emulator that
/// replays the text replays the image.
///
/// The child PRINTS the sequence itself rather than having it written to the pty: a raw `ESC` written as
/// input would be echoed back in caret notation by the line discipline and never reach the parser.
///
/// Linux-gated: it finds the forked daemon through `/proc`, like its text sibling.
#[test]
#[cfg(target_os = "linux")]
fn a_killed_daemon_gives_its_panes_back_with_their_inline_images() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-image-durability-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    // A 2x2 RGBA raster with distinctive pixels, transmitted at cell (0,0) under a distinctive id.
    let pixels: Vec<u8> = (1..=16u8).collect();
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&pixels)
    };
    let printf = format!("\\033_Ga=T,f=32,s=2,v=2,i=42;{b64}\\033\\\\");

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
                "name": "art",
                "cmd": ["sh", "-c", format!("printf '{printf}'; exec cat")],
            },
        }),
    )
    .expect("new_session answers");

    // The pane really is showing the image before anything is killed — otherwise a restore of nothing
    // would pass vacuously.
    assert!(
        wait_for(Duration::from_secs(10), || {
            image_summaries(&mut conn, "art")
                .iter()
                .any(|img| img["id"] == 42 && img["anchor"] == json!([0, 0]) && img["width"] == 2)
        }),
        "the child's image never reached the pane",
    );
    drop(conn);

    // Wait on the DURABLE condition: the transmit is in a committed `.hist`, not in the atomic
    // write's temp (see `saved_history_contains`).
    assert!(
        wait_for(Duration::from_secs(30), || saved_history_contains(
            &state, "_Ga=T"
        )),
        "the daemon never persisted the pane's image under {}",
        state.display(),
    );

    // The reboot.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("reconnect");
    let mut seen = Vec::new();
    let restored = wait_for(Duration::from_secs(15), || {
        seen = image_summaries(&mut conn, "art");
        !seen.is_empty()
    });
    assert!(
        restored,
        "the restored pane came back with no image at all (summaries: {seen:?})",
    );
    assert_eq!(seen.len(), 1, "exactly the one image: {seen:?}");
    assert_eq!(seen[0]["id"], 42, "the image kept its OWN id: {seen:?}");
    assert_eq!(
        (seen[0]["width"].as_u64(), seen[0]["height"].as_u64()),
        (Some(2), Some(2)),
        "and its extent: {seen:?}",
    );
    assert_eq!(
        seen[0]["anchor"],
        json!([0, 0]),
        "and the CELL it was anchored at: {seen:?}",
    );
    drop(guard);
}

/// Every pane image summary in session `session`, flattened across its panes — `{id,width,height,
/// anchor,seq}` as the panes slot reports it. Empty when no pane is showing one (the field is additive,
/// so an image-less pane simply omits it).
#[cfg(target_os = "linux")]
fn image_summaries(conn: &mut HostConn, session: &str) -> Vec<serde_json::Value> {
    let listed: serde_json::Value = match conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    ) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["images"].as_array())
        .flatten()
        .cloned()
        .collect()
}

/// `sprag run` LISTS a project's declared commands, and TYPES one at the pane's prompt without
/// running it — the whole vertical, over the real socket, with the daemon's `HOME` pointed at a
/// temporary project so its boot pane sits inside one.
///
/// The pane runs `cat`, so what is "typed" at it comes straight back as its own output: that is how
/// this proves the paste ARRIVED without needing a shell. The absence of a trailing newline is the
/// load-bearing assertion — a project's config names the command, but the user presses Enter.
///
/// REVERT-PROOF: append a `\n` to the pasted line and the "no newline" assertion fails; drop the
/// `run` verb's paste and the pane echoes nothing.
#[test]
fn the_cli_run_lists_a_projects_commands_and_types_one_at_the_prompt() {
    let project = std::env::temp_dir().join(format!("sprag-cli-project-{}", std::process::id()));
    std::fs::create_dir_all(&project).expect("create the temp project");
    std::fs::write(
        project.join(sprag_host::PROJECT_FILE),
        "[[command]]\nname = \"greet\"\nrun = [\"echo\", \"two words\"]\n",
    )
    .expect("write the project config");

    let (_host, sock) = spawn_host_env(&[("HOME", &project.display().to_string())]);

    // The listing: `name<TAB>command line`, with the multi-word argument quoted back into one word.
    let listed = sprag(&sock, &["run"]);
    assert!(listed.ok, "run listed: {}", listed.stderr);
    assert_eq!(
        listed.stdout.trim(),
        "greet\techo 'two words'",
        "one line per command, name then the line it would run: {:?}",
        listed.stdout
    );

    // An unknown name is a clean error naming what IS declared, never a silent no-op.
    let unknown = sprag(&sock, &["run", "nope"]);
    assert!(!unknown.ok, "an unknown command fails");
    assert!(
        unknown.stderr.contains("nope") && unknown.stderr.contains("greet"),
        "the error names the miss and the alternatives: {}",
        unknown.stderr
    );

    // The delivery: typed at the pane, which is `cat`, so it echoes back.
    let typed = sprag(&sock, &["run", "greet"]);
    assert!(typed.ok, "run greet succeeded: {}", typed.stderr);
    let echoed = wait_for_pane_text(&sock, "echo 'two words'");
    // The line appears EXACTLY ONCE: that one copy is the terminal's echo of what was typed. A
    // trailing newline would have completed `cat`'s line-buffered read, so `cat` would write the
    // line back and it would appear TWICE — which is precisely the "it ran" signal that must not
    // happen here. (Measured both ways before being written down.)
    assert_eq!(
        echoed.matches("echo 'two words'").count(),
        1,
        "the line was typed but NOT executed — the Enter is the user's: {echoed:?}"
    );

    std::fs::remove_dir_all(&project).ok();
}

/// Poll pane 0's full text until it contains `needle`, returning it. Waits on the CONDITION the
/// assertion reads rather than on a timer.
fn wait_for_pane_text(sock: &Path, needle: &str) -> String {
    let mut conn = HostConn::connect(sock, Duration::from_secs(5)).expect("connect");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = String::new();
    while Instant::now() < deadline {
        let answer = conn
            .call(
                "scene/query",
                json!({ "path": sprag_host::pane_input_path(0, sprag_host::wire::FULL_TEXT_SLOT) }),
            )
            .expect("full_text query");
        last = answer.as_str().unwrap_or_default().to_owned();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("pane 0 never showed {needle:?}; last text was {last:?}");
}

/// The pane lifecycle over the CLI: list, split, list again, kill, and the refusals on either side
/// of it. Drives the real daemon, so a break in the wire vocabulary these verbs share with it —
/// `spawn`, `close`, the `panes` slot — fails here rather than in someone's shell.
///
/// The listing is checked for the SHAPE a script slices (`ID: COLSxROWS  COMMAND`), not just for
/// the id being present: `cut -d: -f1` is the documented way to feed these ids to the other verbs,
/// so the colon is part of the contract.
#[test]
fn the_cli_splits_lists_and_kills_panes_over_the_socket() {
    let (_host, sock) = spawn_host();

    // The boot window holds exactly its one pane, listed id-first.
    let listed = sprag(&sock, &["panes"]);
    assert!(listed.ok, "panes succeeded: {}", listed.stderr);
    let first: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(first.len(), 1, "one boot pane: {:?}", listed.stdout);
    assert!(
        first[0].starts_with("0: ") && first[0].contains('x') && first[0].contains("cat"),
        "ID: COLSxROWS  COMMAND: {:?}",
        first[0],
    );
    // The scope is OPTIONAL for a pane command, and naming it explicitly means the same thing.
    assert_eq!(
        sprag(&sock, &["panes", "-t", "0"]).stdout,
        listed.stdout,
        "-t 0 and the default scope are the same session here",
    );

    // split-window: a new pane in the same window, its id printed for a script to capture.
    let split = sprag(&sock, &["split-window", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let new_pane = split.stdout.trim().to_owned();
    assert!(
        new_pane.parse::<u64>().is_ok(),
        "it prints the new pane id: {new_pane:?}",
    );
    let listed = sprag(&sock, &["panes"]);
    assert_eq!(
        listed.stdout.lines().count(),
        2,
        "the window now holds two panes: {:?}",
        listed.stdout,
    );
    assert!(
        listed
            .stdout
            .lines()
            .any(|line| line.starts_with(&format!("{new_pane}: "))),
        "including the one just made: {:?}",
        listed.stdout,
    );

    // kill-pane: it goes, and the boot pane stays.
    let killed = sprag(&sock, &["kill-pane", &new_pane]);
    assert!(killed.ok, "kill-pane succeeded: {}", killed.stderr);
    assert!(
        killed.stdout.contains(&format!("killed pane {new_pane}")),
        "it confirms which: {:?}",
        killed.stdout,
    );
    let listed = sprag(&sock, &["panes"]);
    assert_eq!(
        listed.stdout.lines().count(),
        1,
        "back to the boot pane: {:?}",
        listed.stdout,
    );

    // Killing it again is a clean "no such pane", not a silent success.
    let again = sprag(&sock, &["kill-pane", &new_pane]);
    assert!(!again.ok, "a second kill fails");
    assert!(
        again.stderr.contains(&format!("no pane {new_pane}")),
        "and names the miss: {}",
        again.stderr,
    );

    // Argument errors are local, before any request goes out.
    let noid = sprag(&sock, &["kill-pane"]);
    assert!(
        !noid.ok && noid.stderr.contains("needs a pane id"),
        "arg error: {}",
        noid.stderr,
    );
}

/// `split-window -h` / `-v` divide the pane the caller names, from the shell.
///
/// The verb's FOUR forms are exercised because they are four different requests and only running
/// each proves the dispatch: bare (append — the `spawn` action), `-h PANE` and `-v PANE` (divide —
/// the `split` action), and `-b` (the other side).
///
/// Each one's ARRANGEMENT is then read off the daemon, which is the assertion that could not be
/// skipped: a CLI that mapped `-v` to `"horizontal"`, or dropped `-b`, would spawn a pane and
/// print an id exactly like a correct one. Counting panes proves the request arrived; only the
/// layout proves it arrived meaning what the user typed.
#[test]
fn split_window_divides_the_pane_it_is_given_from_the_shell() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");

    // Bare: no direction, no target — the append tmux's bare `split-window` gives.
    let appended = split_id(&sock, &["split-window", "--", "cat"]);
    assert_eq!(
        tiled(&mut conn),
        vec![0, appended],
        "a bare split appends, as it always has",
    );

    // Each directional form, against the SAME target, read back through the daemon's own layout.
    for (args, side, dir) in [
        (
            vec!["split-window", "-h", "0", "--", "cat"],
            sprag_terminal::SplitSide::Second,
            sprag_terminal::SplitDir::Horizontal,
        ),
        (
            vec!["split-window", "-v", "0", "--", "cat"],
            sprag_terminal::SplitSide::Second,
            sprag_terminal::SplitDir::Vertical,
        ),
        (
            vec!["split-window", "-v", "-b", "0", "--", "cat"],
            sprag_terminal::SplitSide::First,
            sprag_terminal::SplitDir::Vertical,
        ),
    ] {
        let fresh = split_id(&sock, &args);
        assert_eq!(
            layout_of(&mut conn).leaf_home(sprag_terminal::PaneId(fresh)),
            Some(sprag_terminal::LeafHome::beside(
                sprag_terminal::PaneId(0),
                side,
                dir
            )),
            "{args:?} put pane {fresh} beside pane 0 on the axis and side it named",
        );
    }
}

/// Run a `split-window` form and return the pane id it printed.
fn split_id(sock: &Path, args: &[&str]) -> u64 {
    let run = sprag(sock, args);
    assert!(run.ok, "{args:?} succeeded: {}", run.stderr);
    run.stdout
        .trim()
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("{args:?} prints the new pane id: {:?}", run.stdout))
}

/// The scoped window's arrangement, as the daemon serves it.
fn layout_of(conn: &mut HostConn) -> sprag_terminal::LayoutTree {
    let value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(sprag_host::wire::LAYOUT_SLOT) }),
        )
        .expect("the layout query answers");
    let snapshot: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the layout deserialises off the wire");
    let mut tree = sprag_terminal::LayoutTree::new();
    tree.set_from_wire(snapshot.tree)
        .expect("a served arrangement is well-formed");
    tree
}

/// The tiled pane ids, in paint order.
fn tiled(conn: &mut HostConn) -> Vec<u64> {
    layout_of(conn)
        .panes()
        .into_iter()
        .map(|pane| pane.0)
        .collect()
}

/// The verb's two halves arrive together or not at all, and each refusal NAMES what is missing.
///
/// This is the honesty guard the old direction-flag refusal became. sprag's daemon has no current
/// pane, so tmux's bare `-h` cannot be honoured — and the two wrong answers are the ones this
/// checks against: guessing a pane (the user's shell would land somewhere they never named) and
/// accepting a pane with no axis (nothing to ask for). A refused request must also cost nothing,
/// so each case re-counts the panes.
#[test]
fn split_window_refuses_a_direction_without_a_pane_and_a_pane_without_a_direction() {
    let (_host, sock) = spawn_host();

    for (args, expected) in [
        (vec!["split-window", "-h"], "needs the pane to divide"),
        (vec!["split-window", "-v"], "needs the pane to divide"),
        (vec!["split-window", "0"], "needs an axis"),
        (vec!["split-window", "-b"], "needs -h or -v"),
        (vec!["split-window", "-h", "-v", "0"], "only one"),
        (vec!["split-window", "nope"], "neither a flag nor a pane id"),
    ] {
        let run = sprag(&sock, &args);
        assert!(!run.ok, "{args:?} is refused, not guessed at");
        assert!(
            run.stderr.contains(expected),
            "{args:?} names what is missing (want {expected:?}): {}",
            run.stderr,
        );
        assert_eq!(
            sprag(&sock, &["panes"]).stdout.lines().count(),
            1,
            "{args:?} spawned nothing",
        );
    }

    // A pane the window does not hold is the daemon's refusal, not the parser's — and it too
    // must leave nothing behind.
    let missing = sprag(&sock, &["split-window", "-v", "9999"]);
    assert!(!missing.ok, "an unreachable target is refused");
    assert!(
        missing.stderr.contains("9999") && missing.stderr.contains("tiling"),
        "and the refusal names the pane and the reason: {}",
        missing.stderr,
    );
    assert_eq!(
        sprag(&sock, &["panes"]).stdout.lines().count(),
        1,
        "a refused split spawns nothing",
    );
}

/// `resize-pane -x -y` reaches the pane's PTY: the daemon reports the new geometry back through
/// the same `panes` slot the listing reads.
///
/// Both dimensions are required, and a zero is refused — the two argument rules that keep this
/// verb from sending a size no terminal can hold.
#[test]
fn the_cli_resizes_a_pane_and_the_daemon_reports_the_new_size() {
    let (_host, sock) = spawn_host();

    let resized = sprag(&sock, &["resize-pane", "0", "-x", "100", "-y", "30"]);
    assert!(resized.ok, "resize-pane succeeded: {}", resized.stderr);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("100x30"),
        "the daemon reports the new size: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // One dimension is not enough: there is no honest "the other one, unchanged" to send.
    let half = sprag(&sock, &["resize-pane", "0", "-x", "80"]);
    assert!(!half.ok, "one dimension is refused");
    assert!(
        half.stderr.contains("both dimensions"),
        "and says so: {}",
        half.stderr,
    );

    // A zero column count is not a resize; it is rejected locally.
    let zero = sprag(&sock, &["resize-pane", "0", "-x", "0", "-y", "30"]);
    assert!(!zero.ok, "a zero dimension is refused");
    assert!(
        zero.stderr.contains("positive"),
        "with a clear reason: {}",
        zero.stderr,
    );

    // An absent pane names what IS there rather than failing obscurely.
    let ghost = sprag(&sock, &["resize-pane", "9999", "-x", "80", "-y", "24"]);
    assert!(
        !ghost.ok && ghost.stderr.contains("9999"),
        "an absent pane fails, naming it: {}",
        ghost.stderr,
    );
}

/// `resize-window` PINS a window's size, `-u` un-pins it, and the size takes effect only while
/// `window-size` is `manual` — with the gap REPORTED rather than left to be discovered.
///
/// The daemon here has no attached client, which is what makes the first claim discriminating: every
/// other policy answers "no window" in that state, so a pane that moves at all proves the pin is a
/// rule the daemon performs. The `largest` half then proves the other direction — the size is stored
/// but inert — and that a user is TOLD, which is the whole reason storing an inert value is allowed.
#[test]
fn the_cli_pins_a_window_size_and_reports_when_the_policy_ignores_it() {
    // The DAEMON reads `window-size` from the user's file itself — no option crosses the wire — so
    // the config home has to reach the daemon as well as the CLI. Giving it to the CLI alone is a
    // real mistake this gate made first: the verb stored the pin and printed no note (its own view of
    // the file said `manual`) while the daemon went on arbitrating under the default.
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let (_host, sock) = spawn_host_env(&env);

    let pinned = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "111", "-y", "33"],
        &env,
    );
    assert!(pinned.ok, "resize-window succeeded: {}", pinned.stderr);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("111x33"),
        "the pin reached the pane with nobody attached: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
    assert!(
        pinned.stderr.is_empty(),
        "a pin the policy USES needs no note: {}",
        pinned.stderr,
    );

    // A pane forced elsewhere is pulled back by the next action boundary — the pin is the authority
    // for as long as it stands, not a one-shot write.
    sprag(&sock, &["resize-pane", "0", "-x", "40", "-y", "10"]);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("111x33"),
        "a pinned window re-derives over a direct pane resize: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // -u hands it back: with nothing pinned and no client reporting, there is no window, so the
    // panes hold where they are rather than reflowing to a number nobody chose.
    let freed = sprag_env(&sock, &["resize-window", "-t", "0", "-u"], &env);
    assert!(freed.ok, "-u succeeded: {}", freed.stderr);
    sprag(&sock, &["resize-pane", "0", "-x", "40", "-y", "10"]);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("40x10"),
        "an un-pinned window stops overriding: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // The same verb under a policy that does not read the pin: it STORES and says so. The policy is
    // flipped through the real `set-option`, which edits the one file both processes read — so this
    // also shows the daemon picking the change up with nothing restarted.
    let flipped = sprag_env(&sock, &["set-option", "window-size", "largest"], &env);
    assert!(flipped.ok, "set-option succeeded: {}", flipped.stderr);
    let noted = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "100", "-y", "30"],
        &env,
    );
    assert!(
        noted.ok,
        "a pin is stored whatever the policy: {}",
        noted.stderr
    );
    assert!(
        noted.stderr.contains("largest") && noted.stderr.contains("manual"),
        "the note names the policy in force AND the way out: {:?}",
        noted.stderr,
    );
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("40x10"),
        "and it stayed inert: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // ...and the SAME stored size becomes live the moment the policy names it. Nothing is re-sent.
    let back = sprag_env(&sock, &["set-option", "window-size", "manual"], &env);
    assert!(back.ok, "set-option succeeded: {}", back.stderr);
    sprag_env(&sock, &["resize-pane", "0", "-x", "50", "-y", "12"], &env);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("100x30"),
        "a size stored under `largest` is what `manual` then uses: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
}

/// A RELATIVE resize reads the window's current size on the DAEMON side and answers the rectangle it
/// produced — the one form a CLI could almost have computed for itself, and deliberately does not.
///
/// The direction convention is what a reader gets wrong, so it is asserted rather than described:
/// each flag names an EDGE and pushes it, so `-R` widens and `-U` SHORTENS. A gate written to "up
/// means taller" fails here.
#[test]
fn the_cli_resizes_a_window_relative_to_the_size_it_has() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let (_host, sock) = spawn_host_env(&env);

    sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "100", "-y", "30"],
        &env,
    );
    let moved = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-R", "20", "-U", "5"],
        &env,
    );
    assert!(moved.ok, "{}", moved.stderr);
    assert!(
        moved.stdout.contains("120x25"),
        "the daemon answered the rectangle it worked out: {:?}",
        moved.stdout,
    );
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("120x25"),
        "and the panes took it: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // The clamp: past the bottom is the smallest window, never a wrapped one.
    let floored = sprag_env(&sock, &["resize-window", "-t", "0", "-L", "9999"], &env);
    assert!(
        floored.ok && floored.stdout.contains("1x25"),
        "a huge narrowing saturates: {:?} {}",
        floored.stdout,
        floored.stderr,
    );
}

/// `resize-window`'s argument rules: both dimensions or neither, no zero, the five spellings mutually
/// exclusive, opposing edges on one axis refused, a relative count required, an unresolvable fold
/// refused, and an unknown window named. Each refusal happens with nothing written.
#[test]
fn the_cli_refuses_a_half_window_a_zero_and_a_contradiction() {
    let (_host, sock) = spawn_host();

    let half = sprag(&sock, &["resize-window", "-t", "0", "-x", "80"]);
    assert!(
        !half.ok && half.stderr.contains("both dimensions"),
        "{}",
        half.stderr
    );

    let zero = sprag(&sock, &["resize-window", "-t", "0", "-x", "0", "-y", "30"]);
    assert!(
        !zero.ok && zero.stderr.contains("positive"),
        "{}",
        zero.stderr
    );

    let ghost = sprag(
        &sock,
        &["resize-window", "-t", "0", "ghost", "-x", "80", "-y", "24"],
    );
    assert!(
        !ghost.ok && ghost.stderr.contains("ghost"),
        "{}",
        ghost.stderr
    );

    // The five spellings name ONE rectangle, so two of them is a caller who has not decided. Every
    // pair is refused rather than ordered by a precedence rule the user never stated.
    for pair in [
        vec!["-x", "80", "-y", "24", "-A"],
        vec!["-x", "80", "-y", "24", "-u"],
        vec!["-A", "-R", "4"],
        vec!["-u", "-R", "4"],
    ] {
        let mut args = vec!["resize-window", "-t", "0"];
        args.extend(pair.iter().copied());
        let mixed = sprag(&sock, &args);
        assert!(
            !mixed.ok && mixed.stderr.contains("five ways to name one size"),
            "{pair:?}: {}",
            mixed.stderr,
        );
    }

    // `-a -A` is the one contradiction the mode count CANNOT see, because both spellings land in one
    // slot — and this assertion is why it is caught at all: it first failed with the daemon's
    // no-basis refusal, which is the shape of `-A` having silently won.
    let folds = sprag(&sock, &["resize-window", "-t", "0", "-a", "-A"]);
    assert!(
        !folds.ok && folds.stderr.contains("opposite folds"),
        "{}",
        folds.stderr,
    );

    // Both directions on ONE axis is a contradiction, not a net movement: `-L 5 -R 3` could only be
    // read as "2 narrower" by arithmetic nobody asked for.
    for axis in [vec!["-L", "5", "-R", "3"], vec!["-U", "2", "-D", "9"]] {
        let mut args = vec!["resize-window", "-t", "0"];
        args.extend(axis.iter().copied());
        let opposed = sprag(&sock, &args);
        assert!(
            !opposed.ok && opposed.stderr.contains("opposite ways"),
            "{axis:?}: {}",
            opposed.stderr,
        );
    }

    // A call naming NOTHING is refused rather than read as `-u`: an empty command line has stated no
    // intent, and treating it as an un-pin would throw a decision away.
    let silent = sprag(&sock, &["resize-window", "-t", "0"]);
    assert!(
        !silent.ok && silent.stderr.contains("needs a size"),
        "{}",
        silent.stderr,
    );

    // A relative flag's count is REQUIRED and positive — the direction carries the sign, so there is
    // no negative adjustment to spell and no default to guess (see the verb's docs on why sprag
    // cannot copy tmux's bare `-U`).
    let bare = sprag(&sock, &["resize-window", "-t", "0", "-R"]);
    assert!(
        !bare.ok && bare.stderr.contains("needs a column count"),
        "{}",
        bare.stderr
    );
    let nought = sprag(&sock, &["resize-window", "-t", "0", "-D", "0"]);
    assert!(
        !nought.ok && nought.stderr.contains("positive"),
        "{}",
        nought.stderr
    );

    // `-A` with no client that has reported an area names no rectangle. The refusal must NOT be read
    // as "no size", which would un-pin the window the user was trying to resize.
    let unfoldable = sprag(&sock, &["resize-window", "-t", "0", "-A"]);
    assert!(
        !unfoldable.ok && unfoldable.stderr.contains("reported an area"),
        "{}",
        unfoldable.stderr,
    );

    // A window command's -t is required, so a missing one is refused before any connection.
    let unscoped = sprag(&sock, &["resize-window", "-x", "80", "-y", "24"]);
    assert!(
        !unscoped.ok && unscoped.stderr.contains("-t SESSION"),
        "{}",
        unscoped.stderr,
    );

    // Nothing above wrote anything: the boot pane is untouched.
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("80x24"),
        "a refused resize-window pins nothing: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
}

/// The strongest chain the CLI can prove without a display: `send-keys` reaches the pane's CHILD
/// through the real PTY, and `capture-pane` reads back what the child did with it.
///
/// The fixture is `cat`, which makes the two languages distinguishable. Literal text is ECHOED by
/// the terminal once, and stays one copy while the line is unfinished; the `Enter` KEY completes
/// `cat`'s line-buffered read, so it writes the line back and a SECOND copy appears. One assertion
/// therefore separates "the text arrived" from "the keystroke arrived", which a single combined
/// send could not. (The same mechanism `sprag run`'s test relies on, read in the other direction.)
#[test]
fn send_keys_reaches_the_child_and_capture_pane_reads_it_back() {
    let (_host, sock) = spawn_host();

    // -l: literal text, typed, no Enter appended.
    let typed = sprag(&sock, &["send-keys", "0", "-l", "marker-one"]);
    assert!(typed.ok, "send-keys -l succeeded: {}", typed.stderr);
    let echoed = wait_for_pane_text(&sock, "marker-one");
    assert_eq!(
        echoed.matches("marker-one").count(),
        1,
        "the terminal echoed it once and `cat` has not seen a line yet: {echoed:?}",
    );

    // capture-pane reads the same text the daemon holds, through the CLI.
    let captured = sprag(&sock, &["capture-pane", "0"]);
    assert!(captured.ok, "capture-pane succeeded: {}", captured.stderr);
    assert!(
        captured.stdout.contains("marker-one"),
        "capture-pane prints the pane's output: {:?}",
        captured.stdout,
    );
    // tmux's `-p` says "to stdout", which is the only thing this can do — same output.
    assert_eq!(
        sprag(&sock, &["capture-pane", "0", "-p"]).stdout,
        captured.stdout,
        "-p is accepted and means what it says",
    );

    // The KEY language: Enter finishes the line, so `cat` writes it back and a second copy lands.
    let entered = sprag(&sock, &["send-keys", "0", "Enter"]);
    assert!(entered.ok, "send-keys Enter succeeded: {}", entered.stderr);
    let doubled = wait_for(Duration::from_secs(5), || {
        sprag(&sock, &["capture-pane", "0"])
            .stdout
            .matches("marker-one")
            .count()
            == 2
    });
    assert!(
        doubled,
        "the Enter reached the child, which echoed the line back: {:?}",
        sprag(&sock, &["capture-pane", "0"]).stdout,
    );

    // A key name the encoder does not know is a clean refusal naming the vocabulary — never a
    // keystroke that silently vanished, which is the one outcome a script cannot detect.
    let unknown = sprag(&sock, &["send-keys", "0", "NotAKey"]);
    assert!(!unknown.ok, "an unknown key name fails");
    assert!(
        unknown.stderr.contains("W3C key name"),
        "and names the vocabulary: {}",
        unknown.stderr,
    );

    // Both pane-addressed verbs pre-flight the id, so a wrong one is about PANES, not addresses.
    for args in [
        vec!["send-keys", "9999", "Enter"],
        vec!["capture-pane", "9999"],
    ] {
        let ghost = sprag(&sock, &args);
        assert!(!ghost.ok, "{args:?} fails on an absent pane");
        assert!(
            ghost.stderr.contains("no pane 9999"),
            "naming it: {}",
            ghost.stderr,
        );
    }
}

/// A temporary `$XDG_CONFIG_HOME` holding `text` as the user config, cleaned up on drop —
/// including on a panicked assertion, so a failed run leaks no directory.
struct ConfigHome(PathBuf);
impl Drop for ConfigHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ConfigHome {
    /// Unique per CALL, like [`socket_path`]: these tests run in parallel threads of one binary and
    /// a shared directory would have them reading each other's config.
    fn new(text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sprag-cli-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("sprag")).expect("temp config dir");
        std::fs::write(dir.join("sprag").join("config.toml"), text).expect("write config");
        Self(dir)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("a utf-8 temp path")
    }
}

/// `list-keys` answers from the user's config with **NO DAEMON RUNNING**, and a file's declarations
/// reach the table a client would use.
///
/// The no-daemon half is the point of the verb, not a convenience: a keybinding is what a CLIENT
/// does with a keyboard, so it lives in the config file rather than in the server — which is why
/// this test names a socket that was never bound and still expects success. tmux's `list-keys`
/// starts a server to answer the same question.
///
/// REVERT-PROOF for the no-daemon claim: route this through `connect()` like every other verb and it
/// fails with "no server running", i.e. a user could not read their own keymap while editing it.
#[test]
fn list_keys_reads_the_users_config_with_no_daemon() {
    let config = ConfigHome::new(
        "[options]\nprefix = \"C-a\"\n\n\
         [[bind]]\nkey = \"|\"\naction = \"split-window -h\"\n\n\
         [[unbind]]\nkey = \"%\"\n",
    );
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let run = sprag_env(
        &absent,
        &["list-keys"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("prefix C-a"));
    let binds: Vec<String> = lines[1..]
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(
        binds.contains(&"bind-key -T prefix | split-window -h".to_owned()),
        "the declared bind is there: {binds:?}",
    );
    assert!(
        !binds.iter().any(|line| line.contains(" % ")),
        "the unbound default is gone: {binds:?}",
    );
    assert!(
        binds.contains(&"bind-key -T prefix d detach-client".to_owned()),
        "and every default the file did not mention survives: {binds:?}",
    );
    // The self-send followed the prefix, so `prefix prefix` still types it.
    assert!(
        binds.contains(&"bind-key -T prefix C-a send-prefix".to_owned()),
        "the self-send follows the prefix: {binds:?}",
    );
}

/// A broken config is a clean refusal that names the file and what is wrong — never a silently
/// default table, which would leave a user believing their config was accepted.
#[test]
fn list_keys_refuses_a_broken_config_and_names_it() {
    let config = ConfigHome::new("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
    let run = sprag_env(
        &socket_path(),
        &["list-keys"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(!run.ok, "a broken config fails");
    assert!(
        run.stderr.contains("config.toml") && run.stderr.contains("is not an action"),
        "naming the file and the fault: {}",
        run.stderr,
    );
}

/// The text `config` currently holds, for the editing verbs' tests.
fn config_text(config: &ConfigHome) -> String {
    std::fs::read_to_string(std::path::Path::new(config.as_str()).join("sprag/config.toml"))
        .expect("the config file")
}

/// **`bind-key` and `unbind-key` need NO DAEMON either, and they WRITE the file.**
///
/// The write is the whole of slice 2's design and the one thing tmux's `bind-key` does not do:
/// tmux's config is an imperative script a runtime fact cannot be written back into, so its binds
/// are transient and the user has to remember to record them. sprag's is declarative TOML, so the
/// file simply IS the live table — which it also has to be, because `list-keys` reads that file
/// with no server and a binding living anywhere else would make it print a table nobody uses.
///
/// Asserted through `list-keys` rather than by reading the file, because the claim is about what a
/// CLIENT would do, and `list-keys` is the same reader a client uses.
///
/// REVERT-PROOF for the no-daemon claim: route either verb through `connect()` and it fails against
/// this never-bound socket — a user could not bind a key while no session was running, which is
/// exactly when they are setting one up.
#[test]
fn bind_key_writes_the_users_config_with_no_daemon() {
    let config = ConfigHome::new("# mine\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    // tmux's own unquoted spelling: the action is the rest of the line.
    let run = sprag_env(&absent, &["bind-key", "c", "split-window", "-h"], &env);
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "stdout stays clean for a script: {:?}",
        run.stdout
    );

    let run = sprag_env(&absent, &["unbind-key", "o"], &env);
    assert!(run.ok, "{}", run.stderr);

    let listed = sprag_env(&absent, &["list-keys"], &env);
    assert!(listed.ok, "{}", listed.stderr);
    let binds: Vec<String> = listed
        .stdout
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(
        binds.contains(&"bind-key -T prefix c split-window -h".to_owned()),
        "the bound key is in the table a client would read: {binds:?}",
    );
    assert!(
        !binds.iter().any(|line| line.contains(" o ")),
        "and the unbound default is gone: {binds:?}",
    );
    // The user's own file is still theirs.
    let text = config_text(&config);
    assert!(text.contains("# mine"), "the comment survived: {text:?}");
    assert!(text.contains("[[command]]"), "and the commands: {text:?}");
}

/// A key or action the USER TYPED is reported as an argument, naming NO file — while a broken FILE
/// still names `config.toml`. Both messages exist to send someone to the right place, and a
/// `config.toml:` prefix on a command-line typo sends them to a file that is fine.
///
/// REVERT-PROOF: parse the argument inside `config::bind_key` and render the failure through
/// `ConfigError`, and the first assertion's `config.toml` check fires.
#[test]
fn a_mistyped_argument_names_no_file_and_a_broken_file_still_does() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(&socket_path(), &["bind-key", "Up", "detach-client"], &env);
    assert!(!run.ok, "a key nothing can produce is refused");
    assert!(
        run.stderr.contains("is not a key") && !run.stderr.contains("config.toml"),
        "the argument is the fault, not the file: {}",
        run.stderr,
    );
    assert_eq!(config_text(&config), "", "and nothing was written");

    let broken = ConfigHome::new("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "c", "detach-client"],
        &[("XDG_CONFIG_HOME", broken.as_str())],
    );
    assert!(!run.ok, "an unusable file is refused");
    assert!(
        run.stderr.contains("config.toml"),
        "the file IS the fault here: {}",
        run.stderr,
    );
}

/// `-T prefix` is accepted so a tmux user's spelling works; any other table is refused BY NAME.
///
/// `-T root` is the one that matters: a binding with no prefix competes with the pane for every
/// keystroke, and accepting the flag would promise arbitration nothing performs. Refusing it names
/// what sprag has instead of silently binding into the only table there is.
#[test]
fn only_the_prefix_key_table_is_accepted() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-T", "prefix", "c", "detach-client"],
        &env,
    );
    assert!(run.ok, "tmux's own spelling works: {}", run.stderr);

    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-T", "root", "c", "detach-client"],
        &env,
    );
    assert!(!run.ok, "the root table is not built");
    assert!(
        run.stderr.contains("root") && run.stderr.contains("prefix"),
        "naming what was asked for and what exists: {}",
        run.stderr,
    );
}

/// `show-options` answers from the user's config with **NO DAEMON RUNNING**, and prints EVERY option
/// — the one the file sets and the one it does not.
///
/// The no-daemon half is the point of the verb, for `list-keys`'s reason: every option here is what
/// one CLIENT does with one attachment, so it lives in the user's file rather than in the server.
///
/// Printing an option the file never mentions is the other half, and it is not padding: a user who
/// does not already know an option's name cannot discover it in a file that does not name it, which
/// is the whole reason this table exists rather than a struct. tmux answers the same way.
#[test]
fn show_options_prints_every_option_with_no_daemon() {
    let config = ConfigHome::new("[options]\ndetach-on-destroy = \"off\"\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let run = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert!(
        lines.contains(&"detach-on-destroy off"),
        "the option the file set: {lines:?}",
    );
    assert!(
        lines.contains(&"prefix C-b"),
        "and the one it did not, at its default: {lines:?}",
    );
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    assert_eq!(lines, sorted, "sorted by name, like tmux's: {lines:?}");
}

/// `set-option` WRITES the user's config, needs no daemon, and canonicalises the value.
///
/// It writes for `bind-key`'s reason: the file IS the live table, so `show-options`, an attached
/// client and the next attach cannot give three different answers. Asserted THROUGH `show-options`
/// rather than by reading the file, because the claim is about what a client would read.
///
/// The canonical half is what keeps one setting one string: `^a` and `C-a` are one keystroke, so a
/// file that recorded the spelling it was handed would make `show-options` and `list-keys` disagree
/// about a prefix they both read from it.
#[test]
fn set_option_writes_the_users_config_with_no_daemon() {
    let config = ConfigHome::new("# mine\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let run = sprag_env(&absent, &["set-option", "prefix", "^a"], &env);
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "stdout stays clean for a script: {:?}",
        run.stdout
    );

    let listed = sprag_env(&absent, &["show-options"], &env);
    assert!(listed.ok, "{}", listed.stderr);
    assert!(
        listed.stdout.lines().any(|line| line == "prefix C-a"),
        "stored as the keymap spells it: {:?}",
        listed.stdout,
    );

    // The user's own file is still theirs.
    let text = config_text(&config);
    assert!(
        text.contains("# mine") && text.contains("htop"),
        "the comment and the unrelated table survive an option edit: {text:?}",
    );
}

/// `set-option -u` puts an option back to its default by REMOVING it, so the file says only what the
/// user chose — and it is idempotent, like `unbind-key`.
#[test]
fn set_option_u_removes_the_option_and_restores_the_default() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let run = sprag_env(&absent, &["set-option", "-u", "prefix"], &env);
    assert!(run.ok, "{}", run.stderr);
    assert!(
        !config_text(&config).contains("C-a"),
        "the value is gone from the file: {:?}",
        config_text(&config),
    );
    let listed = sprag_env(&absent, &["show-options"], &env);
    assert!(
        listed.stdout.lines().any(|line| line == "prefix C-b"),
        "and the default is in force: {:?}",
        listed.stdout,
    );

    // Unsetting what is already unset rewrites the same file rather than failing.
    let before = config_text(&config);
    let again = sprag_env(&absent, &["set-option", "-u", "prefix"], &env);
    assert!(again.ok, "{}", again.stderr);
    assert_eq!(before, config_text(&config), "idempotent");
}

/// A mistyped option NAME or VALUE is reported as an ARGUMENT — it must never name `config.toml`.
///
/// The rule this pins is the one `ConfigError` exists for, one level in: a user who typed
/// `set-option prefixx` has a fine config file, and a message prefixed with `config.toml` would send
/// them to read it. Both messages instead say what the alternatives are, which is the only thing that
/// makes a name-keyed table usable.
#[test]
fn set_option_reports_a_bad_argument_without_naming_the_file() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let unknown = sprag_env(&absent, &["set-option", "prefixx", "C-a"], &env);
    assert!(!unknown.ok, "a mistyped option is refused");
    assert!(
        !unknown.stderr.contains("config.toml"),
        "an argument mistake names no file: {}",
        unknown.stderr,
    );
    assert!(
        unknown.stderr.contains("prefix") && unknown.stderr.contains("detach-on-destroy"),
        "it lists the real options: {}",
        unknown.stderr,
    );

    let bad = sprag_env(&absent, &["set-option", "detach-on-destroy", "maybe"], &env);
    assert!(!bad.ok, "a value outside the vocabulary is refused");
    assert!(
        !bad.stderr.contains("config.toml"),
        "also an argument mistake: {}",
        bad.stderr,
    );
    assert!(
        bad.stderr.contains("no-detached"),
        "and it lists the values: {}",
        bad.stderr,
    );
    assert!(
        config_text(&config).is_empty(),
        "neither refusal touched the file: {:?}",
        config_text(&config),
    );
}

/// tmux's `-g` is accepted and a per-window / per-pane scope is refused BY NAME.
///
/// `-g` because every sprag option is global, so the flag a tmux user's fingers produce carries no
/// information and must simply work. `-w` / `-p` because there is no per-window or per-pane table:
/// accepting them would promise an overlay nothing holds, and silently ignoring them would leave a
/// user believing they had set something narrower than they had. The `-T root` treatment, one verb
/// over.
#[test]
fn an_option_scope_with_no_members_is_refused_by_name() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let global = sprag_env(&absent, &["set-option", "-g", "prefix", "C-a"], &env);
    assert!(global.ok, "tmux's own flag works: {}", global.stderr);
    assert!(sprag_env(&absent, &["show-options", "-g"], &env).ok);

    for scope in ["-w", "-p"] {
        let run = sprag_env(&absent, &["set-option", scope, "prefix", "C-a"], &env);
        assert!(!run.ok, "{scope} is refused rather than ignored");
        assert!(
            run.stderr.contains(scope) && run.stderr.contains("-g"),
            "naming what was asked for and what exists: {}",
            run.stderr,
        );
    }
}

/// `show-options` and `list-keys` can never disagree about the PREFIX.
///
/// Two verbs print it, which is safe only because they read ONE fact: the keymap's prefix is built
/// FROM the option rather than beside it. This is the drift guard that claim needs — a second home
/// for the prefix would show up here as two answers, and R235's defect was exactly a keymap and its
/// prefix disagreeing.
#[test]
fn the_prefix_reads_the_same_through_both_verbs() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    for spelling in ["C-a", "^o", "F1"] {
        assert!(sprag_env(&absent, &["set-option", "prefix", spelling], &env).ok);
        let shown = sprag_env(&absent, &["show-options"], &env);
        let listed = sprag_env(&absent, &["list-keys"], &env);
        let from_options = shown
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("prefix "))
            .map(str::to_owned);
        let from_keys = listed
            .stdout
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("prefix "))
            .map(str::to_owned);
        assert_eq!(
            from_options, from_keys,
            "{spelling:?}: show-options and list-keys must name one prefix",
        );
        assert!(from_options.is_some(), "and both printed one");
    }
}

/// `show-options NAME` prints one option, and `-v` prints its value ALONE — tmux's `show-options -v`,
/// and the singular read herdr spells `show-option`.
///
/// `-v` is what a script wants: `$(sprag show-options -v prefix)` needs no `cut`, and a value on a
/// line of its own cannot be mis-split by one.
#[test]
fn show_options_reads_one_option_by_name() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let named = sprag_env(&absent, &["show-options", "prefix"], &env);
    assert!(named.ok, "{}", named.stderr);
    assert_eq!(named.stdout.trim(), "prefix C-a", "the tmux shape");

    let bare = sprag_env(&absent, &["show-options", "-v", "prefix"], &env);
    assert!(bare.ok, "{}", bare.stderr);
    assert_eq!(bare.stdout.trim(), "C-a", "the value alone");

    // An option the file never mentions still answers, with the default in force — the whole reason
    // the registry holds a default rather than the file.
    let unset = sprag_env(&absent, &["show-options", "-v", "detach-on-destroy"], &env);
    assert_eq!(unset.stdout.trim(), "on");

    // A mistyped name is refused with the list, like `set-option`'s.
    let unknown = sprag_env(&absent, &["show-options", "prefixx"], &env);
    assert!(!unknown.ok, "a mistyped name is refused");
    assert!(
        unknown.stderr.contains("gui-font") && !unknown.stderr.contains("config.toml"),
        "listing the real options and naming no file: {}",
        unknown.stderr,
    );

    // `-v` with nothing to read is refused rather than answered with every value: the flag exists so a
    // caller can read ONE without parsing, and a list would hand back the ambiguity it removes.
    let vague = sprag_env(&absent, &["show-options", "-v"], &env);
    assert!(!vague.ok, "-v needs a name");
    assert!(vague.stderr.contains("needs a name"), "{}", vague.stderr);
}

/// A NUMBER option is written unquoted and read back from either spelling.
///
/// `gui-font = 20` is how a person writes a size in a file they maintain by hand; demanding `"20"`
/// would be the parser's convenience imposed on the user. Both spellings are accepted BECAUSE the
/// writer emits one of them — a file this CLI edited and a file a human wrote must not differ.
#[test]
fn a_number_option_is_written_as_a_number() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    assert!(sprag_env(&absent, &["set-option", "gui-font", "28"], &env).ok);
    let text = config_text(&config);
    assert!(
        text.contains("gui-font = 28") && !text.contains("\"28\""),
        "written unquoted, as a person writes a size: {text:?}",
    );

    // The QUOTED spelling a user may have written by hand reads back the same.
    let quoted = ConfigHome::new("[options]\ngui-font = \"28\"\n");
    let read = sprag_env(
        &absent,
        &["show-options", "-v", "gui-font"],
        &[("XDG_CONFIG_HOME", quoted.as_str())],
    );
    assert_eq!(read.stdout.trim(), "28", "either spelling is one value");

    // A TOML type no option takes names the option — not just the type serde wanted.
    let wrong = ConfigHome::new("[options]\ngui-font = 1.5\n");
    let refused = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", wrong.as_str())],
    );
    assert!(!refused.ok, "a float is not a value an option takes");
    assert!(
        refused.stderr.contains("gui-font") && refused.stderr.contains("config.toml"),
        "naming the option AND the file: {}",
        refused.stderr,
    );
}

/// **THE GATE for the first DAEMON-side option**: a pane the daemon births with no command runs the
/// user's `default-command`.
///
/// Every other option in this table is one client's, and the daemon reads none of them. This one it
/// must, because a pane is born THERE — so the claim can only be made against a real daemon reading a
/// real `config.toml`, which is what this does. It covers TWO birth paths, and they are separate code:
/// the standalone boot pane (`sprag-term` with no `--` command) and a pane created through the wire
/// `spawn` action (`split-window`, which every other daemon-side birth shares a spec parser with).
///
/// The claim discriminates: with `default-command = "cat"` the label is `cat`, where a daemon ignoring
/// the option labels the pane with `$SHELL`'s own name. Read through `sprag panes`, so it is the label
/// the daemon actually recorded.
///
/// REVERT-PROOF: put `default_shell_command()` back in `parse_spawn` and the split's line fails while
/// the boot pane's still passes — the two paths are asserted separately for exactly that reason.
#[test]
fn a_daemon_born_pane_runs_the_users_default_command() {
    let config = ConfigHome::new("[options]\ndefault-command = \"cat\"\n");
    // NO `--` command, so the boot pane has none of its own to prefer. `cat` blocks on its PTY, which
    // is what keeps the session alive for the rest of the test.
    let (_host, sock) = spawn_host_with(&[], &[("XDG_CONFIG_HOME", config.as_str())]);

    // Polled, because the boot pane is spawned after the bind: an unpopulated first read would be a
    // false failure rather than a claim about the option.
    let mut boot = String::new();
    let born = wait_for(Duration::from_secs(5), || {
        boot = sprag(&sock, &["panes"])
            .stdout
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        !boot.is_empty()
    });
    assert!(born, "the boot pane appears");
    assert!(
        boot.ends_with("cat"),
        "the boot pane runs the user's default-command, not a shell: {boot:?}",
    );

    // ...and so does a pane born through the wire spawn action.
    let split = sprag(&sock, &["split-window", "-h", "0"]);
    assert!(split.ok, "split: {}", split.stderr);
    let listed = sprag(&sock, &["panes"]);
    assert!(listed.ok, "{}", listed.stderr);
    let labels: Vec<&str> = listed
        .stdout
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    assert_eq!(
        labels,
        vec!["cat", "cat"],
        "both the boot pane and the split's run it: {:?}",
        listed.stdout,
    );
}

/// `show-options` prints a COMMAND shell-quoted, and `-v` prints it raw.
///
/// Quoted because it is the only way an empty one can be seen at all — `default-command ''` says
/// something, a line ending in a space says nothing — and because a command with spaces on an
/// unquoted line is ambiguous to a reader. tmux prints a string option the same way. `-v` stays raw:
/// a script asked for the value, not a rendering of it.
#[test]
fn a_command_option_is_printed_quoted_and_read_raw() {
    let absent = socket_path();
    let empty = ConfigHome::new("");
    let shown = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", empty.as_str())],
    );
    assert!(shown.ok, "{}", shown.stderr);
    assert!(
        shown
            .stdout
            .lines()
            .any(|line| line == "default-command ''"),
        "an unset command is VISIBLE as empty: {:?}",
        shown.stdout,
    );

    let set = ConfigHome::new("[options]\ndefault-command = \"exec top -d 2\"\n");
    let env = [("XDG_CONFIG_HOME", set.as_str())];
    let shown = sprag_env(&absent, &["show-options"], &env);
    assert!(
        shown
            .stdout
            .lines()
            .any(|line| line == "default-command 'exec top -d 2'"),
        "a command with spaces is one quoted word: {:?}",
        shown.stdout,
    );
    let bare = sprag_env(&absent, &["show-options", "-v", "default-command"], &env);
    assert_eq!(
        bare.stdout.trim(),
        "exec top -d 2",
        "and -v hands a script the value itself",
    );
}
