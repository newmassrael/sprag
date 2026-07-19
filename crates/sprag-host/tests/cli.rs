//! Integration test: the `sprag` management CLI drives a real `sprag-term` over the socket.
//!
//! Both binaries are the built artifacts (`CARGO_BIN_EXE_*`), so a break in the wire vocabulary
//! the CLI shares with the daemon — or in the CLI's own output — fails in CI, not by hand.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg("--")
        .arg("cat")
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
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
