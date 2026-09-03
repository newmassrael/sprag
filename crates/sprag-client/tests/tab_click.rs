//! **A TAB CLICK'S WINDOW SELECT, THROUGH A REAL CLIENT AGAINST A REAL DAEMON** — register item
//! 860.
//!
//! # ⛔⛔⛔⛔⛔ The defect this pins, and why every neighbouring gate was green
//!
//! The owner pressed a window tab dozens of times and the window did not change. Item 852 made the
//! click SAY when it does nothing; it did not make it work, and the ledger's item 860 carries the
//! table of causes disproved by reading. Two more were disproved by driving:
//!
//! * the DAEMON performs an identity-addressed select over a real socket
//!   (`sprag-host`'s `a_window_selected_by_identity_lands_over_the_real_socket`), and
//! * the GUI's own half works over the in-process host
//!   (`sprag-gui`'s `a_tab_click_selects_the_window_the_tab_was_painted_from`).
//!
//! Those two meet at exactly one place that neither of them drives: [`WireHost`], the client the
//! production GUI actually holds. A tab click reaches `HostClient::select_window` on THIS type, and
//! nothing anywhere had ever called it. The `sprag-host` wire tests say in their own words that
//! `WireHost` *"lives in `sprag-gui` (a bin crate, `WireHost` is `pub(crate)`)"* and is therefore
//! out of reach — a sentence that stopped being true when the type moved into this LIBRARY crate,
//! and which is why the gap outlived several rounds of work on the surfaces either side of it.
//!
//! # ⚠⚠ Why the assertion is the client's own MIRROR and not the daemon's answer
//!
//! The tab strip is painted from [`HostClient::windows`], which a wire client serves out of a
//! mirror it refreshes for itself. So a select that the daemon performs and the client never
//! re-reads is invisible to the person — the owner's report exactly — and a gate that stopped at
//! the answer would be green through it. Both halves are asserted here, and they are different
//! claims: the ANSWER says the daemon moved, the MIRROR says this client can tell.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pinion_core::QuitSink;
use serde_json::json;
use sprag_client::{BootSpec, WireHost};
use sprag_host::HostClient;
use sprag_host::mux_action_path;
use sprag_host::wire::{NEW_WINDOW_ACTION, WindowRef};
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

/// The shell's quit edge. A client under test that asks to QUIT has failed the claim — selecting a
/// window leaves it attached — so this says so where it happens.
struct NeverQuits;

impl QuitSink for NeverQuits {
    fn request_quit(&self) {
        panic!("the client asked the shell to quit; selecting a window leaves it on its session");
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
    // ⚠ `sprag_scratch::scratch_root()` and not `std::env::temp_dir()` — item 794. A bare
    // `temp_dir()` answers the CRATE'S OWN DIRECTORY inside this repository when `TMPDIR` is
    // set-and-empty, so a socket made that way is litter `git status` cannot see. The ratchet in
    // `sprag-gate` counts the bare call sites and refused this file's first draft.
    sprag_scratch::scratch_root().join(format!("sprag-tab-{}-{n}.sock", std::process::id()))
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

/// Boot a real display client against `sock`, on a fresh session of its own — the frontend the
/// window tab strip belongs to.
fn boot(sock: &Path) -> WireHost {
    let endpoint = HostEndpoint::given("the tab-click test", sock);
    let argv = ["cat".to_owned()];
    WireHost::boot(
        &BootSpec {
            endpoint: &endpoint,
            session: None,
            // ⚠ Its OWN session, so the windows it makes below are the ones it is looking at and no
            // other client's arrangement can move under the assertion.
            fresh: true,
            argv: Some(&argv),
            cols: 80,
            rows: 24,
            panes: 1,
            frontend: sprag_client::Frontend::Window,
        },
        Arc::new(|| {}),
        Arc::new(NeverQuits),
    )
    .expect("a client boots against the daemon")
}

/// ⛔⛔⛔⛔⛔ **THE OWNER'S GESTURE, THROUGH THE CLIENT THE GUI HOLDS.**
///
/// A tab click resolves the row it was painted from to a [`WindowRef::Picked`] and sends THAT —
/// `sprag-gui`'s `wtabs::select_painted_tab`, verbatim in shape. This drives the same call on the
/// same type against a real daemon.
///
/// # ⚠ The fixture is built so a NO-OP cannot pass
///
/// `new_window` selects what it makes, so the client ends on the last one; the target is a window
/// it is NOT on. A client that answered a name without acting, or that acted and never re-read,
/// leaves the mirror where it was and fails the second assertion.
///
/// REVERT-PROOF: drop the `refresh_view()` from `WireHost::select_window` and the ANSWER still
/// arrives while the mirror stays behind — which is the shape of the owner's report and the reason
/// the two assertions are separate.
#[test]
fn a_tab_clicks_identity_select_moves_the_client_it_was_sent_from() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let host = boot(&sock);

    // Two more windows, so "the one this client is on" is a real choice.
    for _ in 0..2 {
        let _born = host.new_window();
    }
    let rows = host.windows();
    assert_eq!(
        rows.len(),
        3,
        "three windows for the strip to paint: {rows:?}",
    );

    // WHAT A TAB IS PAINTED FROM — the identity on the row, which is the only address a pointer
    // surface may hold (a name it painted a moment ago can have moved since).
    let target = rows
        .iter()
        .find(|w| !w.current)
        .expect("a window this client is NOT on, or a no-op would pass");
    let id = target.id.expect(
        "⚠ THE PREMISE: this daemon publishes an identity on every window row, and a client that \
         is served none can offer no act at all",
    );
    let name = target.name.clone();

    // THE CLICK.
    let landed = host.select_window(&WindowRef::Picked(id));
    assert_eq!(
        landed.as_deref(),
        Some(name.as_str()),
        "⛔⛔⛔⛔⛔ THE SELECT MUST LAND. `HostClient::select_window` answers the window it landed \
         on and `None` when the address resolved to nothing — and a `None` here is exactly what a \
         tab click reports as *that window is gone* while the person is looking straight at the \
         tab",
    );

    // ⚠⚠⚠⚠⚠ AND THE CLIENT MUST BE ABLE TO TELL. The strip paints from this mirror, so a select
    // the daemon performed and this client never re-read is invisible to the person — which is the
    // owner's report and is NOT covered by the answer above.
    let after = host.windows();
    assert_eq!(
        after.iter().find(|w| w.current).map(|w| w.name.as_str()),
        Some(name.as_str()),
        "⛔⛔⛔⛔⛔ THE TAB STRIP READS THIS LIST. A select that landed at the daemon and left this \
         client's own window mirror on the old window paints the old tab highlighted, which is \
         *the tab did nothing* to everybody looking at it: {after:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A TAB FOR A WINDOW THIS CLIENT DID NOT MAKE** — register item 860, and the ONE
/// difference between every green fixture and the arrangement the owner is actually sitting in.
///
/// # ⚠⚠⚠⚠⚠ Why this is a different test and not the one above with more windows
///
/// The gate above makes its own windows, and so does `sprag-gui`'s smoke, and so does every
/// window test in this workspace. **The owner made none of theirs.** All six of that session's
/// windows were opened by the loop's launcher from other processes, so the client learned they
/// exist through its POLL thread rather than as the answer to its own act — a different road into
/// the same mirror (`store_windows` from the poll, against `refresh_view` on the UI thread), and
/// the identity a tab is painted from is whatever arrived by it.
///
/// A row that reached the strip that way and carried no `id` would paint a tab that cannot be
/// addressed and can only no-op, which is the owner's report exactly and is the one shape the
/// disproof table in item 860 could not rule out by reading: the ledger checked what the DAEMON
/// serves, and this checks what a client that was not asking ends up holding.
///
/// REVERT-PROOF: this fails the moment the poll's window list stops carrying identities, which no
/// other test in this workspace would notice — they all read a list they asked for themselves.
#[test]
fn a_tab_click_lands_on_a_window_this_client_never_opened() {
    let (_daemon, sock) = spawn_daemon();
    await_daemon(&sock);
    let host = boot(&sock);
    // ⚠ The client's OWN name for where it is — `HostClient::current_session`, the same label its
    // session rail paints. Asking it, rather than assuming the daemon's boot session, is what keeps
    // the windows below in the session this client is actually looking at.
    let session = host.current_session();

    // ⚠ MADE FROM ANOTHER CONNECTION, which is the whole point: the client is not the caller, so
    // everything it knows about these windows arrived on its own poll — the launcher's shape.
    let mut elsewhere = HostConn::connect(&sock, BOOT_WAIT).expect("a second connection");
    for _ in 0..2 {
        elsewhere
            .call(
                "scene/invoke",
                json!({
                    "session": session,
                    "path": mux_action_path(NEW_WINDOW_ACTION),
                    "args": {},
                }),
            )
            .expect("new_window answers on the second connection");
    }

    // The client has to NOTICE, on its own clock. A deadline rather than a sleep: what is being
    // waited for is the poll adopting a list nobody handed it.
    let deadline = Instant::now() + BOOT_WAIT;
    let rows = loop {
        let rows = host.windows();
        if rows.len() == 3 || Instant::now() >= deadline {
            break rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        rows.len(),
        3,
        "⚠ THE PREMISE: the client's own poll must adopt windows somebody else opened, or this \
         gate is about nothing: {rows:?}",
    );

    // ⚠⚠ AND EVERY ONE OF THEM MUST CARRY AN ADDRESS. This is the assertion the disproof table in
    // item 860 could only make of the DAEMON; here it is made of what a client actually holds.
    let unaddressed: Vec<&str> = rows
        .iter()
        .filter(|w| w.id.is_none())
        .map(|w| w.name.as_str())
        .collect();
    assert!(
        unaddressed.is_empty(),
        "⛔⛔⛔⛔⛔ A TAB IS PAINTED FROM THIS LIST AND CLICKED THROUGH ITS `id`. A row that \
         arrived on the poll with none paints a tab that can only no-op — which is what the owner \
         met, and what no other test in this workspace looks at: {unaddressed:?} of {rows:?}",
    );

    let target = rows
        .iter()
        .find(|w| !w.current)
        .expect("a window this client is NOT on");
    let (id, name) = (
        target.id.expect("filtered to the rows that have one"),
        target.name.clone(),
    );
    assert_eq!(
        host.select_window(&WindowRef::Picked(id)).as_deref(),
        Some(name.as_str()),
        "a select addressed by an identity the POLL delivered must land exactly as one the client \
         asked for itself does",
    );
    let after = host.windows();
    assert_eq!(
        after.iter().find(|w| w.current).map(|w| w.name.as_str()),
        Some(name.as_str()),
        "⛔⛔⛔⛔⛔ and the strip must follow: {after:?}",
    );
}
