//! "Take me back where I was" lands on the session this client VISITED — against a real
//! `sprag-term`, because that is the only place the claim is about.
//!
//! # The defect this pins, MEASURED at `8d1de4a` before a line of R304 was written
//!
//! `WireHost` kept its own visit history as a `Vec<String>` of session NAMES, maintained by
//! nothing. Driven through this same harness: a client booted on session `1`, switched to `beta`,
//! and then — out of band — `1` was renamed to `renamed` and a NEW session took the freed name.
//! `switch_session_last()` (tmux `switch-client -l`) attached it to `1`: **the impostor**, a
//! session it had never seen, over the connection it also sends keystrokes down. The daemon
//! confirmed it, reporting `1 … attached:1` while `renamed` had no viewer.
//!
//! With no impostor the same rename produced the degraded half: the visit was silently LOST and the
//! gesture became a no-op.
//!
//! # Why the fixture keeps the impostor
//!
//! Because it is the only thing that makes this test able to fail. A history of names resolves onto
//! `work`; a history of identities resolves onto `renamed`. Both sessions are live at the moment of
//! the ask, so neither answer can be reached by accident — and a version of this test that renamed
//! without re-issuing the name would pass against a client that had simply forgotten how to go back
//! at all.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pinion_core::QuitSink;
use serde_json::{Value, json};
use sprag_client::{BootSpec, WireHost};
use sprag_host::HostClient;
use sprag_host::mux_action_path;
use sprag_host::wire::{NEW_SESSION_ACTION, RENAME_SESSION_ACTION, SESSIONS_SLOT, SPAWN_ACTION};
use sprag_rpc::{HostConn, HostEndpoint};

/// How long the daemon gets to bind its socket before the test gives up on it.
const BOOT_WAIT: Duration = Duration::from_secs(10);

/// A daemon killed and unlinked when the test ends, however it ends.
struct Daemon(Child, PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// The shell's quit edge. A client under test that asks to QUIT has failed the claim — every
/// gesture here leaves it attached to something — so this says so where it happens rather than as a
/// puzzling assertion later.
struct NeverQuits;

impl QuitSink for NeverQuits {
    fn request_quit(&self) {
        panic!("the client asked the shell to quit; every switch here leaves it on a session");
    }
}

/// The `sprag-term` binary beside the test executable cargo built. Its ABSENCE is a loud failure
/// rather than a skip, because a skipped gate is a green tick over an untested claim.
fn sprag_term_bin() -> PathBuf {
    let path = std::env::current_exe()
        .expect("the test executable has a path")
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

/// A socket path unique to this CALL (this file's tests may run as parallel threads of one binary).
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-back-{}-{n}.sock", std::process::id()))
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

fn connect(sock: &Path) -> HostConn {
    let mut conn = HostConn::connect(sock, BOOT_WAIT).expect("connect to the daemon socket");
    conn.set_read_deadline(Some(Duration::from_secs(10)))
        .expect("bound the probe's reads");
    conn
}

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

/// Boot a real display client against `sock`, on a fresh session of its own.
fn boot(sock: &Path) -> WireHost {
    let endpoint = HostEndpoint::given("the going-back test", sock);
    let argv = ["cat".to_owned()];
    WireHost::boot(
        &BootSpec {
            endpoint: &endpoint,
            session: None,
            argv: Some(&argv),
            cols: 80,
            rows: 24,
            panes: 1,
            // Not what this file measures — nothing here destroys a session, so no destroy policy is
            // consulted. `Window` because that is the frontend whose client this stands in for.
            frontend: sprag_client::Frontend::Window,
        },
        Arc::new(|| {}),
        Arc::new(NeverQuits),
    )
    .expect("a client boots against the daemon")
}

fn new_session(conn: &mut HostConn, name: &str) {
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": name, "cols": 80, "rows": 24 },
        }),
    )
    .expect("new_session answers");
}

fn rename(conn: &mut HostConn, from: &str, to: &str) {
    conn.call(
        "scene/invoke",
        json!({
            "session": from,
            "path": mux_action_path(RENAME_SESSION_ACTION),
            "args": { "name": to },
        }),
    )
    .expect("rename_session answers");
}

/// How many clients the DAEMON says are viewing `session`, and whether it holds one at all.
///
/// The client's own `current_session()` is a label it paints; this is the fact underneath it, read
/// from the other side of the socket — so the two together say the client believes it moved AND
/// that it really did.
fn attached(conn: &mut HostConn, session: &str) -> Option<u64> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )
    .expect("the sessions slot answers")
    .as_array()
    .expect("a list of sessions")
    .iter()
    .find(|row| row["name"].as_str() == Some(session))
    .map(|row| row["attached"].as_u64().unwrap_or(0))
}

/// **The claim.** A client goes back to the session it VISITED, across a rename, and never to the
/// impostor that took the name that session wore.
///
/// REVERT-PROOF: make the daemon's history hold names instead of ids — or simply have
/// `switch_session_last` re-attach by the name the client last saw — and the two assertions after
/// the ask both fail, naming `work` where the client's own visit is `renamed`.
#[test]
fn a_client_goes_back_to_the_session_it_visited_across_a_rename() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let mut admin = connect(&sock);
    let client = boot(&sock);

    let home = client.current_session();
    new_session(&mut admin, "beta");
    client.switch_session("beta");
    assert_eq!(
        client.current_session(),
        "beta",
        "the CONTROL: an ordinary switch works, so the ask below is the thing under test",
    );

    // Out of band, by another client or a person at a CLI: the visited session is renamed, and a
    // brand-new session takes the name it wore.
    rename(&mut admin, &home, "renamed");
    new_session(&mut admin, &home);
    assert_eq!(
        attached(&mut admin, &home),
        Some(0),
        "the impostor is LIVE and unattached — a history of names resolves straight onto it",
    );

    // tmux `switch-client -l`.
    let _ = client.switch_session_last();

    assert_eq!(
        client.current_session(),
        "renamed",
        "the client goes back to the session it visited, under the name that session has now",
    );
    assert_eq!(
        attached(&mut admin, "renamed"),
        Some(1),
        "and the daemon agrees: it is really viewing that session",
    );
    assert_eq!(
        attached(&mut admin, &home),
        Some(0),
        "...and never landed on the stranger wearing the name it remembered",
    );

    // Going back is itself a visit, so the gesture toggles — tmux's own `switch-client -l`.
    let _ = client.switch_session_last();
    assert_eq!(client.current_session(), "beta");
}

/// A client with nowhere to go back to stays where it is — and stays a WORKING client, which is the
/// half a no-op is easy to get wrong: the gesture stops the poll thread before it asks, so a
/// `Nowhere` answer that did not restart it would leave a live process that never notices anything
/// again.
///
/// **The last assertion is here because a revert-proof said it had to be.** Dropping
/// `switch_session_last`'s `fall_back_to` on the `Ok(None)` arm left this test GREEN with only
/// the "an ordinary switch still completes" check, because a switch does its own attach and starts
/// its own poll — so it repairs the very thing it was supposed to detect. What a stopped poll
/// really costs is the client's mirrors going deaf, so that is what is asserted: a pane opened out
/// of band must reach this client without it asking for anything.
#[test]
fn a_client_with_nowhere_to_go_back_to_stays_put_and_still_works() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let mut admin = connect(&sock);
    let client = boot(&sock);

    let home = client.current_session();
    // The CONTROL that the mirror was live BEFORE the gesture — otherwise a client that had never
    // been listening would pass the check below by way of failing it identically.
    assert!(
        follows_out_of_band(&client, &mut admin, &home),
        "the CONTROL: this client's mirror follows its session before the no-op",
    );

    let _ = client.switch_session_last();
    assert_eq!(
        client.current_session(),
        home,
        "a client that never switched has nowhere to go back to, and tmux no-ops there too",
    );
    assert_eq!(
        attached(&mut admin, &home),
        Some(1),
        "and the daemon still has it on the session it booted into",
    );
    assert!(
        follows_out_of_band(&client, &mut admin, &home),
        "and it is still LISTENING: a no-op that left the poll thread stopped would go deaf here",
    );

    // Still a working client in the other direction too: an ordinary switch after the no-op
    // completes. (On its own this proves less than it looks — see the doc above.)
    new_session(&mut admin, "beta");
    client.switch_session("beta");
    assert_eq!(client.current_session(), "beta");
    assert_eq!(attached(&mut admin, "beta"), Some(1));
}

/// Open a pane in `session` from the OUTSIDE and wait for `client`'s own mirror to show it — the
/// question "is this client's poll thread still alive", asked the only way a test outside the
/// process can ask it.
///
/// The client is never asked to refresh: a mirror that grows here grew because the poll thread woke
/// on the daemon's change and re-read. The bound is generous and the state it waits for is exact.
fn follows_out_of_band(client: &WireHost, admin: &mut HostConn, session: &str) -> bool {
    let before = client.pane_ids().len();
    admin
        .call(
            "scene/invoke",
            json!({
                "session": session,
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "argv": ["cat"], "cols": 80, "rows": 24 },
            }),
        )
        .expect("spawn answers");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if client.pane_ids().len() > before {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// The daemon answers the attach with the session the client LANDED on — the fact both arms of
/// `attach_in_place` now read, rather than assuming the name they asked with.
///
/// Driven here rather than only in `sprag-host`'s wire test because this is the client that has to
/// USE it: a daemon answering `true` (as every build before R304 did) leaves this client unable to
/// tell where it went, which is what the protocol number exists to prevent.
#[test]
fn the_daemon_names_the_session_an_attach_landed_on() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let mut conn = connect(&sock);
    conn.handshake("going-back-probe")
        .expect("the daemon speaks this build's wire");
    conn.scope_to("0".to_owned());

    assert_eq!(
        conn.call(sprag_rpc::CLIENT_ATTACH_METHOD, Value::Null)
            .expect("client/attach is accepted"),
        json!("0"),
        "a named attach answers the name it landed on",
    );

    let mut params = serde_json::Map::new();
    sprag_host::wire::AttachAsk::LastViewed { unattached: false }.write_into(&mut params);
    assert_eq!(
        conn.call(sprag_rpc::CLIENT_ATTACH_METHOD, Value::Object(params))
            .expect("client/attach is accepted"),
        Value::Null,
        "and a history ask with nowhere to go answers null, which is a state and not a refusal",
    );
}
