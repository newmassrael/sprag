//! **A client whose daemon cannot ACT says so** — the transport policy, driven over a real socket.
//!
//! # What this gate is for
//!
//! `WireHost::request` is the ONE place a wire failure is handled, and until R324 the whole of its
//! policy was a `tracing::debug!`. The register carried the consequence as an open surface decision
//! (item 48): *"a RUNNING display client still SWALLOWS a skew ... whether a repaint loop should say
//! so is a question about how noisy a degraded client is allowed to be."*
//!
//! The answer taken is **a person's GESTURE gets an answer, and a poll does not shout**, with the
//! METHOD as the discriminator: a `scene/invoke` happens only because somebody acted, where a
//! `scene/query` happens on every wake. Both halves are asserted here, and the second is what keeps
//! the first from meaning "report every fault from every read".
//!
//! # Why it needs a real daemon behind a proxy
//!
//! A write reaches its `scene/invoke` only after its pre-flight READS succeed, so against a peer
//! that serves nothing this client never boots, let alone acts. [`sprag_peer::OldDaemon::proxying`]
//! puts a real `sprag-term` behind it and refuses exactly one method — every read is the daemon's
//! own answer, and only the verb is missing. That IS an older build: an action is additive, so the
//! protocol number does not rise for one.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use pinion_core::QuitSink;
use sprag_client::{BootSpec, WireHost};
use sprag_host::HostClient;
use sprag_rpc::HostEndpoint;

/// How long the daemon is given to bind its socket before the boot gives up.
const BOOT_WAIT: Duration = Duration::from_secs(5);

/// A daemon killed on drop, so a failed assertion cannot leave one running.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A quit sink that records nothing: this client is never asked to quit here, and if it were, the
/// test would be measuring a teardown rather than a message.
struct NeverQuits;

impl QuitSink for NeverQuits {
    fn request_quit(&self) {}
}

/// The `sprag-term` binary beside the test executable — absent is a LOUD failure, never a skip.
fn sprag_term_bin() -> PathBuf {
    let path = std::env::current_exe()
        .expect("the test executable has a path")
        .parent()
        .and_then(Path::parent)
        .expect("the test executable sits under the profile directory")
        .join("sprag-term");
    assert!(
        path.exists(),
        "{} is not built. Run `cargo test --workspace`, or `cargo build -p sprag-host --bin \
         sprag-term` first.",
        path.display(),
    );
    path
}

/// An address a booted client reads LATER — never during its boot, which is what makes it usable
/// here: a client cannot start against a daemon missing what it starts by reading (measured — the
/// window list and the pane set are both boot reads, and a peer missing either kills the boot).
///
/// A `LazyLock` over the same builder the client addresses it with, never a hand-spelled path: a
/// literal would go on passing after the wire moved the address, while measuring nothing.
static LATE_ADDRESS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| sprag_host::mux_action_path(sprag_host::wire::TREE_SLOT));

/// A socket path unique to this CALL — these tests are threads of one binary.
fn socket_path(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-skew-{tag}-{}-{n}.sock", std::process::id()))
}

/// A private daemon whose boot pane runs `cat` — an idle child that keeps its PTY open.
fn spawn_daemon() -> (Daemon, PathBuf) {
    let sock = socket_path("up");
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
    // WAIT FOR THE BIND, bounded: the daemon binds a moment after it is spawned, and a boot that
    // raced it would fail with "no such file" — a failure about this fixture rather than about the
    // claim under test.
    let deadline = std::time::Instant::now() + BOOT_WAIT;
    while !sock.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(sock.exists(), "the daemon bound {}", sock.display());
    (Daemon(child), sock)
}

/// The name of the daemon's boot session, read off the wire — the naming rule belongs to the
/// daemon, not to this test.
fn boot_session(upstream: &Path) -> String {
    let mut conn =
        sprag_rpc::HostConn::connect(upstream, BOOT_WAIT).expect("connect to the daemon");
    let sessions = conn
        .call(
            "scene/query",
            serde_json::json!({ "path": sprag_host::mux_action_path(sprag_host::wire::SESSIONS_SLOT) }),
        )
        .expect("the sessions slot answers");
    sessions[0]["name"]
        .as_str()
        .expect("a session has a name")
        .to_owned()
}

/// Boot a client on `sock`, ATTACHED to `session`.
///
/// ⚠ **Attached, never creating** — and that is the fixture's own discovery: a boot with no session
/// CREATES one, which is itself a `scene/invoke`, so a client booting through an action-refusing
/// peer dies before it can act. What is under test is a RUNNING client, which is what a person has.
fn boot(sock: &Path, session: &str) -> WireHost {
    let endpoint = HostEndpoint::given("the skew gate", sock.to_path_buf());
    let argv = ["cat".to_owned()];
    WireHost::boot(
        &BootSpec {
            endpoint: &endpoint,
            session: Some(session),
            argv: Some(&argv),
            cols: 80,
            rows: 24,
            panes: 1,
        },
        Arc::new(|| {}),
        Arc::new(NeverQuits),
    )
    .expect("boot through the peer")
}

/// **AN ACT THE DAEMON CANNOT PERFORM REACHES THE CLIENT AS A SENTENCE.**
///
/// Measured before the fix, with this fixture: `new_window()` answered an empty name, nothing was
/// created, and the client had no way to know — which is what a person pressing `prefix c` saw.
///
/// REVERT-PROOF: drop the `store_message` in `WireHost::request` and the first assertion fails; let
/// it fire for `scene/query` as well and the CONTROL below fails.
#[test]
fn an_act_a_daemon_cannot_perform_reaches_this_client_as_a_sentence() {
    let (_daemon, upstream) = spawn_daemon();
    let sock = socket_path("aged");
    let peer = sprag_peer::OldDaemon::proxying(&sock, &upstream, sprag_peer::Missing::actions());
    let host = boot(peer.sock(), &boot_session(&upstream));

    // THE CONTROL, first: a client that has done nothing has nothing to report. Without it, a
    // mirror seeded at boot would satisfy every assertion below.
    assert!(
        host.take_skew().is_none(),
        "a client that has not acted has met no skew",
    );

    let name = host.new_window();
    let said = host
        .take_skew()
        .expect("an action this daemon does not have is reported");
    let text = said.text.as_str().to_owned();
    assert!(
        text.contains("does not perform")
            && text.contains("new_window")
            && text.contains("sprag kill-server"),
        "the sentence names the act, the cause and the remedy: {text}",
    );
    assert!(
        !text.contains("UnknownInvokePath"),
        "a Rust variant name must not reach a person: {text}",
    );
    assert_eq!(
        said.severity,
        sprag_host::report::Severity::Warn,
        "a gesture that did not happen is a warning, not an alert somebody must acknowledge",
    );
    assert!(
        name.is_empty(),
        "nothing was created, which is why the sentence is worth painting",
    );

    // TAKEN EXACTLY ONCE — it is an edge, and a second keystroke must not be answered with the
    // first one's refusal.
    assert!(host.take_skew().is_none(), "the report is taken, not read");
}

/// **THE CONTROL FOR THE POLICY: a READ this daemon cannot serve does NOT reach the row.**
///
/// Reads happen on every wake, so a client that reported each would have nothing else on its
/// surface. The discriminator is the METHOD, and this is the half that says so — without it,
/// "a person's gesture gets an answer" is satisfied by a client that shouts about everything.
///
/// The peer here is missing SLOTS ONLY, so this client boots and acts normally and the only thing
/// that fails is what it reads. A peer missing both would never let it start, which is the boot
/// path's own gate and a different claim.
#[test]
fn a_read_a_daemon_cannot_serve_says_nothing_to_the_person() {
    let (_daemon, upstream) = spawn_daemon();
    let sock = socket_path("noslots");
    // Booted against the DAEMON, then re-pointed: the boot itself reads, so a client that started
    // through this peer would fail before it could act. What is under test is a RUNNING client
    // meeting refused reads, which is the state the swallow policy was written for.
    let session = boot_session(&upstream);
    // ONE ADDRESS MISSING, which is what a client actually meets: a daemon one build behind serves
    // everything the client needs to START and lacks the address a later build added. A peer
    // refusing EVERY read cannot express this at all — the client would never boot, which is the
    // boot path's own gate and a different claim.
    let peer = sprag_peer::OldDaemon::proxying(
        &sock,
        &upstream,
        sprag_peer::Missing::addresses(std::slice::from_ref(&LATE_ADDRESS)),
    );
    let through_peer = boot(peer.sock(), &session);

    // The read FAILS, and the CONTROL is the same read against the daemon itself: without it,
    // an empty answer could mean the peer refused it OR that there was nothing to answer.
    let direct = boot(&upstream, &session);
    assert!(
        !direct.tree().is_empty(),
        "the daemon serves this address, so an EMPTY answer through the peer is the refusal and \
         not an empty daemon",
    );
    assert!(
        through_peer.tree().is_empty(),
        "the peer is missing this address, so the read cannot land",
    );
    assert!(
        through_peer.take_skew().is_none(),
        "a failing read is the poll's business and must not take a person's row",
    );

    // ...and the CONTROL for the control: this same client, acting, DOES report. Without it, a
    // client whose skew mirror was broken outright would satisfy the assertion above.
    let peer_that_cannot_act = sprag_peer::OldDaemon::proxying(
        &socket_path("both"),
        &upstream,
        sprag_peer::Missing::actions(),
    );
    let acting = boot(peer_that_cannot_act.sock(), &session);
    let _ = acting.new_window();
    assert!(
        acting.take_skew().is_some(),
        "the same seam, on the acting side, is what makes the silence above a POLICY",
    );
}
