//! A client boot that fails after creating its session leaves NOTHING on the daemon — against a
//! real `sprag-term`, because that is the only place the claim is about.
//!
//! The defect this pins is R278's: `WireHost`'s boot creates a session and then keeps going, and
//! every later step — panes, mirrors, the poll connection — used to return its error straight out,
//! leaving that session on a daemon which outlives the client. Nine of them accumulated on a live
//! machine in one afternoon.
//!
//! # The injection is the product's own behaviour, not a test hook
//!
//! `argv` names a binary that does not exist. The host's `new_session` VALIDATES the birth spec,
//! creates the session, and then tolerates a fork/exec failure by design (an empty session is a
//! valid attach target), answering with the name. `boot_panes` then has to reach the requested
//! pane count and its `spawn` is refused — a failure strictly AFTER the creation, reached without
//! a single line of test-only code in the client.
//!
//! # Why the assertion probes BY NAME
//!
//! The `sessions` slot drops paneless, unattached sessions (`SessionInfo::is_listable`), so the
//! orphan this injection produces is invisible to a session COUNT. A request scoped to a session
//! that does not exist is refused whole instead (`ScopeError::Unknown`), so scoping a read to the
//! name the boot reports is a direct question about that one session — and it is the same question
//! whether or not the session would have listed.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pinion_core::QuitSink;
use serde_json::json;
use sprag_client::{BootError, BootSpec, WireHost};
use sprag_host::mux_action_path;
use sprag_host::wire::WINDOWS_SLOT;
use sprag_rpc::{HostConn, HostEndpoint};

/// How long the daemon gets to bind its socket before the test gives up on it.
const BOOT_WAIT: Duration = Duration::from_secs(10);

/// The reply bound for the test's own probe connection: the daemon answers from memory, so a
/// reply that has not arrived in seconds is not slow.
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// A daemon killed and unlinked when the test ends, however it ends.
struct Daemon(Child, PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// The shell's quit edge, which this test never expects to fire — a boot that fails never gets as
/// far as a poll thread.
struct NeverQuits;

impl QuitSink for NeverQuits {
    fn request_quit(&self) {
        panic!("a failed boot must not ask the shell to quit");
    }
}

/// The `sprag-term` binary beside the test executable cargo built.
///
/// `CARGO_BIN_EXE_*` covers binaries of the package under test and this package has none, so the
/// path is derived — and its ABSENCE is a loud failure rather than a skip, because a skipped gate
/// is a green tick over an untested claim.
fn sprag_term_bin() -> PathBuf {
    let path = std::env::current_exe()
        .expect("the test executable has a path")
        // …/target/<profile>/deps/<test>-<hash> -> …/target/<profile>
        .parent()
        .and_then(Path::parent)
        .expect("the test executable sits under the profile directory")
        .join("sprag-term");
    assert!(
        path.exists(),
        "{} is not built. This test drives a binary that belongs to another package, so cargo \
         does not build it for `-p sprag-client` alone — run `cargo test --workspace`, or \
         `cargo build -p sprag-host --bin sprag-term` first.",
        path.display(),
    );
    path
}

/// A socket path unique to this CALL, under the temp dir (this file's tests may run as parallel
/// threads of one binary, so a pid-only path would be one string shared by all of them).
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-boot-rb-{}-{n}.sock", std::process::id()))
}

/// A private daemon whose boot pane runs `cat` — an idle child that keeps its PTY open, so the
/// daemon's self-cleaning lifetime never ends the run out from under the test.
fn spawn_daemon() -> (Daemon, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(sprag_term_bin())
        .arg("--size")
        .arg("80x24")
        .arg("--")
        .arg("cat")
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the sprag-term daemon");
    (Daemon(child, sock.clone()), sock)
}

/// A request connection to the daemon, once it is serving.
fn connect(sock: &Path) -> HostConn {
    let mut conn = HostConn::connect(sock, BOOT_WAIT).expect("connect to the daemon socket");
    conn.set_read_deadline(Some(PROBE_DEADLINE))
        .expect("bound the probe's reads");
    conn
}

/// Whether the daemon still holds a session named `name`.
///
/// Asked by SCOPING a read to it: the host resolves the scope at the door and refuses the whole
/// request when no session carries the name, so the answer is about that one session and nothing
/// else. Any other failure (a broken socket, a malformed reply) is a broken test rather than a
/// "no", so it panics instead of being read as an absence.
fn session_exists(sock: &Path, name: &str) -> bool {
    let mut conn = connect(sock);
    conn.scope_to(name.to_owned());
    match conn.call(
        "scene/query",
        json!({ "path": mux_action_path(WINDOWS_SLOT) }),
    ) {
        Ok(_) => true,
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("no session named"),
                "the probe must fail because the session is GONE, not for another reason: \
                 {message}",
            );
            false
        }
    }
}

/// Wait until the daemon answers at all, so the boot under test is not racing the bind.
fn await_daemon(sock: &Path) {
    let deadline = Instant::now() + BOOT_WAIT;
    while Instant::now() < deadline {
        if HostConn::connect(sock, Duration::ZERO).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the daemon never bound {}", sock.display());
}

/// Boot a client whose panes cannot spawn, against `sock`, and return the boot's own report.
fn failing_boot(sock: &Path) -> BootError {
    let endpoint = HostEndpoint::given("the boot-rollback test", sock);
    let argv = ["/nonexistent/sprag-r279-probe".to_owned()];
    let error = WireHost::boot(
        &BootSpec {
            endpoint: &endpoint,
            session: None,
            argv: Some(&argv),
            cols: 80,
            rows: 24,
            panes: 1,
            // Not under test: this boot never completes, so no destroy policy is ever read.
            frontend: sprag_client::Frontend::Window,
        },
        Arc::new(|| {}),
        Arc::new(NeverQuits),
    )
    .err()
    .expect("a boot whose panes cannot spawn must fail");
    downcast(error)
}

/// The [`BootError`] inside an [`io::Error`] — the payload every boot failure carries, so a caller
/// can ask what happened instead of parsing what was said about it.
fn downcast(error: io::Error) -> BootError {
    let inner = error
        .into_inner()
        .expect("a boot failure carries its BootError payload");
    *inner
        .downcast::<BootError>()
        .expect("the payload IS a BootError")
}

/// The claim: a boot that fails after creating its session removes it again, and says so.
///
/// REVERT-PROOF: replace `Err(born.roll_back(cause).into())` in `WireHost::boot` with the plain
/// cause and this fails at the `session_exists` assertion — the session the boot created is still
/// there, which is exactly the state R278 found nine of.
#[test]
fn a_boot_that_fails_after_creating_its_session_leaves_none() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);

    let reported = failing_boot(&sock);

    let created = reported
        .created()
        .expect("the boot created a session before it failed")
        .to_owned();
    assert_eq!(
        reported.orphan(),
        None,
        "the daemon was reachable, so the rollback ran: {reported}",
    );
    assert!(
        !session_exists(&sock, &created),
        "the session `{created}` the failed boot created must not survive it",
    );
    let rendered = reported.to_string();
    assert!(
        rendered.contains("(given by the boot-rollback test)"),
        "the failure names WHICH daemon it reached: {rendered}",
    );
    assert!(
        rendered.contains(&format!(
            "the session `{created}` this boot created was removed"
        )),
        "the failure says what it did about the session it made: {rendered}",
    );
}

/// The control for the test above: the daemon it probes DOES answer "yes" for a session that is
/// really there. Without this, a `session_exists` that answered `false` for every name — a broken
/// probe — would read as a passing rollback.
#[test]
fn the_probe_sees_a_session_that_exists() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let mut conn = connect(&sock);
    let created = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(sprag_host::wire::NEW_SESSION_ACTION),
                "args": { "cols": 80, "rows": 24 },
            }),
        )
        .expect("create a session to look for")
        .as_str()
        .expect("new_session answers with the name")
        .to_owned();

    assert!(
        session_exists(&sock, &created),
        "a session that exists must probe as present, or the rollback test proves nothing",
    );
}
