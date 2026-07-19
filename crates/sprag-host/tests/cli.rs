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

/// Run the `sprag` CLI against `sock`, returning its stdout and whether it exited 0.
fn sprag(sock: &Path, args: &[&str]) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(args)
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .output()
        .expect("run the sprag CLI");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.success(),
    )
}

#[test]
fn the_cli_lists_creates_and_kills_sessions_over_the_socket() {
    let (_host, sock) = spawn_host();

    // ls: the boot session "0" is the one an unscoped request lands in. (The CLI's own connect
    // window absorbs the host's bind race.)
    let (out, ok) = sprag(&sock, &["ls"]);
    assert!(ok, "ls succeeded: {out}");
    assert!(
        out.contains("0:") && out.contains("(default)"),
        "ls shows the boot session as the default: {out}",
    );

    // new work: creates it and prints the name a client would scope to.
    let (out, ok) = sprag(&sock, &["new", "work"]);
    assert!(ok, "new succeeded: {out}");
    assert_eq!(out.trim(), "work", "new prints the created name: {out}");

    // ls now lists it, and "0" is still the default.
    let (out, _) = sprag(&sock, &["ls"]);
    assert!(out.contains("work"), "ls shows the created session: {out}");
    assert!(out.contains("(default)"), "the default is unmoved: {out}");

    // kill-session work: a non-last kill removes it.
    let (out, ok) = sprag(&sock, &["kill-session", "work"]);
    assert!(ok, "kill-session succeeded: {out}");
    assert!(out.contains("killed work"), "kill-session confirms: {out}");

    // ...and it is gone, while the default survives (it was not the last).
    let (out, _) = sprag(&sock, &["ls"]);
    assert!(!out.contains("work"), "the killed session is gone: {out}");
    assert!(
        out.contains("0:"),
        "a non-last kill leaves the default: {out}"
    );

    // Killing an unknown session fails cleanly (non-zero exit, no raw wire error).
    let (_out, ok) = sprag(&sock, &["kill-session", "ghost"]);
    assert!(!ok, "killing an unknown session fails");
}
