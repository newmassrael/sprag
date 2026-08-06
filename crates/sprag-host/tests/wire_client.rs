//! Integration test: the wire CLIENT protocol against a REAL `sprag-term` host
//! process, over the always-on Unix socket.
//!
//! The GUI's `WireHost` (in `sprag-gui`) and an AI peer both drive the host through
//! exactly this contract — `HostConn` request/response over the socket, the
//! `/sprag_mux/…` pane list, the `/pane_<id>/sprag_input/external/cells.<offset>` frame
//! read, the `key`/`text` input actions, and the async `scene/revision` + `scene/waitFor`
//! change-notification. R115c's default (wire) boot path is otherwise exercised only
//! by a manual live drive; this gives it automated coverage against the real binary
//! (`CARGO_BIN_EXE_sprag-term`), so a break in the wire ABI fails in CI, not by hand.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_host::agent::SWEEP_INTERVAL;
use sprag_host::wire::events_slot_since;
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION, DISPLAY_MESSAGE_ACTION,
    DROP_FILE_ACTION, FULL_TEXT_SLOT, JOIN_PANE_ACTION, KILL_SESSION_ACTION, LAYOUT_SLOT,
    LINKS_SLOT, MOVE_WINDOW_ACTION, NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT,
    PASTE_ACTION, RELEASE_AGENT_ACTION, RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION,
    REPORT_AGENT_ACTION, SELECT_WINDOW_ACTION, SESSION_SLOT, SESSIONS_SLOT, SET_FLOATING_ACTION,
    SET_LAYOUT_ACTION, SPAWN_ACTION, SPLIT_ACTION, TEXT_ACTION, WINDOWS_SLOT, cells_slot_at,
    project_slot_for,
};
use sprag_host::{CellFrame, mux_action_path, pane_input_path};
use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, EVENTS_WAIT_METHOD, HostConn,
    PROTOCOL_FIELD, PROTOCOL_PARAM, SINCE_PARAM, WIRE_PROTOCOL,
};

/// Kills + reaps the spawned host on scope exit (including a test panic), so a failed
/// assertion never leaks a `sprag-term` — and unlinks its socket, so it leaks no file either.
///
/// Owning the PATH (not just the `Child`) is the point: [`socket_path`] mints a fresh name per
/// call, so without this every run would strew one dead socket per test under the temp dir,
/// forever. The kill must come first — the host holds the socket open until it exits.
struct HostChild(Child, PathBuf);
impl Drop for HostChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// Spawn a `sprag-term` whose initial pane runs `cat` (a deterministic echo pane that
/// keeps its PTY open), on a socket path of its OWN — see [`socket_path`].
///
/// Returns the guard and the path, so a caller never names a socket itself: hand-rolling one
/// is exactly how three of these tests used to collide (below).
fn spawn_host() -> (HostChild, PathBuf) {
    spawn_host_running(&["cat"])
}

/// Like [`spawn_host`] but the boot pane runs `program [args…]` instead of `cat` — for the tests
/// that need the child to EMIT something (an OSC notification), not just echo. `sprag-term`'s
/// `-- <program> [args…]` contract sets the boot command.
fn spawn_host_running(program_and_args: &[&str]) -> (HostChild, PathBuf) {
    spawn_host_with(program_and_args, &[])
}

/// The one spawn: `program_and_args` as the boot pane, plus `env` overrides on the DAEMON's own
/// environment — which is what a test needs when it stands in for a program the HOST spawns (a
/// stand-in `scp` reached through the daemon's `PATH`), not one the test spawns itself.
fn spawn_host_with(program_and_args: &[&str], env: &[(&str, &str)]) -> (HostChild, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let mut command = Command::new(env!("CARGO_BIN_EXE_sprag-term"));
    command
        .arg("--size")
        .arg("40x6")
        .arg("--")
        .args(program_and_args)
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn the sprag-term host binary");
    (HostChild(child, sock.clone()), sock)
}

/// A socket path unique to this CALL, under the temp dir.
///
/// The counter is load-bearing, not decoration. `cargo test` runs this file's tests as
/// PARALLEL THREADS OF ONE BINARY, so a path keyed only on `process::id()` is the SAME string
/// in every test that asks for one — and each test opens by unlinking that path before
/// spawning its host, i.e. it removes the socket a concurrently-running sibling is serving on.
/// The loser's next call dies with `BrokenPipe`. A live race the whole time; it only surfaced
/// when R152 lengthened `wire_client_drives_a_real_sprag_term_host` enough to widen the
/// overlap. "Passes today" and "is isolated" are different claims.
///
/// **R154:** the race was among the THREE tests that shared `sprag-wire-it-{pid}` — the R153
/// fix and its commit message both overstated it as "all 6". The other three hand-rolled their
/// own names (`spawn` / `close` / `absent` infixes) and so avoided collision by NAMING LUCK,
/// one copy-pasted infix away from re-creating it. They now all come through here, and
/// [`spawn_host`] owns the minting so no test can name a socket at all.
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-wire-it-{}-{n}.sock", std::process::id()))
}

#[test]
fn wire_client_drives_a_real_sprag_term_host() {
    let (_host, sock) = spawn_host();

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // The mux pane list — exactly one boot pane (the `cat`).
    let panes = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("panes query");
    assert_eq!(
        panes.as_array().map(Vec::len),
        Some(1),
        "one boot pane: {panes}"
    );

    // The LIVE cell FRAME is a QUERY — `cells.0`, the live member of the host's
    // `cells.<offset>` family — the exact wire contract WireHost's poll loop reads each
    // wake. It deserializes as the shared CellFrame shape (cells + flattened facts).
    let frame = conn
        .call(
            "scene/query",
            json!({ "path": pane_input_path(0, &cells_slot_at(0)) }),
        )
        .expect("live frame query");
    assert!(frame.get("cells").is_some(), "frame carries cells: {frame}");
    assert_eq!(
        frame["visible_rows"], 6,
        "visible rows = the boot size: {frame}"
    );
    // Read back through the ONE shared type rather than by reaching into `cells` with pinion's own
    // deserializer: the grid's wire shape is `sprag_grid::wire`'s since R222, and a test that
    // spells it a second time is testing a shape nothing on the real path uses.
    let frame: CellFrame =
        serde_json::from_value(frame).expect("the frame deserializes through the one wire type");
    assert_eq!((frame.cells.cols(), frame.cells.rows()), (40, 6));

    // The livelock regression guard (R152): reading the live frame must NOT advance the
    // scene revision. The poll loop re-reads this frame on every `scene/waitFor` wake, so
    // a read that bumped would wake the very waiter it answered — a ~30Hz idle spin. A
    // query is a `MethodOcc::Read`; the retired `cells`-invoke live path was a Mutate.
    //
    // Nothing else can move the revision across this window, which is what makes it a FACT
    // rather than a coin flip: the boot pane is `cat`, which emits not one byte until it is
    // written to, and the queries above already proved it is up and sized. So any bump here
    // is the read's own doing. (R153 wrapped this in a "quiesce" loop that R154 deleted: it
    // polled for two equal reads, but `wait_until` evaluates before its first sleep, so both
    // reads landed microseconds apart and it returned true immediately — mid-transient
    // included. It waited for nothing while its comment claimed rigor, which is worse than
    // not being there.)
    let before_read = read_revision(&mut conn);
    for _ in 0..5 {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(0, &cells_slot_at(0)) }),
        )
        .expect("live frame re-query");
    }
    assert_eq!(
        read_revision(&mut conn),
        before_read,
        "reading the live frame must not bump the revision (else the poll loop livelocks)",
    );

    // A SCROLLBACK read does not bump either — and it needs no loop of its own here, which
    // is worth stating because R155's first draft wrote one and it was theatre twice over.
    // `MethodOcc` is a STATIC TABLE KEYED BY METHOD NAME (`("scene/query", Read)` /
    // `("scene/invoke", Mutate)`), consulted before the handler ever reads `params.path` —
    // so the classification cannot vary by offset, and the loop above already pins it for
    // every member of the family. Worse, the extra loop read no history at all: this pane is
    // `cat` and has emitted nothing, so `project_scrolled` clamps every offset back to the
    // live view. It re-ran the same read five more times while its comment claimed to prove
    // a second property.
    //
    // The real property PR-61 bought — that history is a READ rather than the revision-
    // bumping invoke it used to be — is pinned where it can be observed: the offset-carrying
    // path is exercised over real scrollback by `rpc::tests::the_cells_family_honors_the_
    // scrollback_offset` (a `seq 1 40` pane), and routing it back through `scene/invoke`
    // fails that test outright, since the query would answer nothing.
    //
    // The two requirements genuinely fight, which is why they live in two tests: real
    // history needs output, and output bumps the revision — the very thing measured here.

    // The retired doors answer nothing. `cells` is the bare stem: it carries no argument, so
    // it is not a MEMBER of the family and is correctly absent. (A member whose argument is
    // malformed — `cells.zzz` — is a different case, and answers `Null` rather than absence;
    // that taxonomy is pinned in `rpc::tests::the_cells_family_answers_the_paths_it_declares`,
    // where a live pane can be driven.)
    for absent in ["cells", "frame"] {
        assert!(
            conn.call("scene/query", json!({ "path": pane_input_path(0, absent) }))
                .is_err(),
            "`{absent}` addresses no frame and must not answer one",
        );
    }

    // Async change-notification: read the baseline, send input (cat echoes it → pane
    // output → revision bump), and confirm a waitFor{baseline} reports the advance.
    let since = read_revision(&mut conn);
    conn.call(
        "scene/invoke",
        json!({ "path": pane_input_path(0, TEXT_ACTION), "args": { "text": "wire_marker_42\n" } }),
    )
    .expect("send text over the wire");
    let woken = conn
        .call("scene/waitFor", json!({ "since": since }))
        .expect("waitFor returns on the echo");
    assert_eq!(woken["changed"], true, "the pane's echo advanced the scene");
    assert!(
        woken["revision"].as_u64().unwrap_or(0) > since,
        "woke past the baseline: {woken}"
    );

    // The input reached the PTY: full_text (scrollback + visible) shows the echo.
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(0, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.contains("wire_marker_42")))
            .unwrap_or(false)
        }),
        "the sent text never echoed back through the wire",
    );

    // The PASTE action routes over the same socket: this `cat` never enabled bracketed paste
    // (mode 2004 off), so the paste is written raw and its marker echoes back — proving
    // `PASTE_ACTION` reaches `inject_paste` -> `pane::paste`. (The bracketing bytes themselves,
    // when 2004 IS on, are pinned by `pane::tests::paste_brackets_when_the_child_enabled_2004`,
    // which reads the raw capture the emulator consumes before it reaches `full_text`.)
    conn.call(
        "scene/invoke",
        json!({ "path": pane_input_path(0, PASTE_ACTION), "args": { "text": "wire_paste_43\n" } }),
    )
    .expect("paste over the wire");
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(0, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.contains("wire_paste_43")))
            .unwrap_or(false)
        }),
        "the pasted text never echoed back through the wire (PASTE_ACTION dispatch)",
    );

    let _ = std::fs::remove_file(&sock);
}

#[test]
fn a_mux_spawn_and_the_new_panes_output_both_advance_the_wire_notification() {
    // Round 1's rail over the REAL socket, end to end. Two new behaviors:
    //
    //   (a) a mux `spawn` (a pane-SET change, not output) grows the set AND advances
    //       the scene revision, so a client long-polling change-notification learns
    //       the host gained a pane;
    //   (b) that mux-spawned pane's OWN output advances the notification too — the
    //       latent-bug closure: before Round 1 only the boot pane was wired to bump,
    //       so a 2nd pane's independent output never woke a waiter.
    //
    // Single connection, no parked blocking read (the park->wake mechanism is covered
    // deterministically by the rpc-level unit tests); here we cross the OS socket +
    // the real `/sprag_mux` dispatch and read the non-blocking `scene/revision`.
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // One boot pane to start.
    assert_eq!(pane_count(&mut conn), 1, "one boot pane");

    // (a) Spawn a 2nd pane over the wire; the set grows and the revision advances.
    let before_spawn = read_revision(&mut conn);
    let spawned = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a 2nd pane over the wire");
    let new_id = spawned.as_u64().expect("spawn returns the new pane id");
    assert_eq!(pane_count(&mut conn), 2, "the mux spawn grew the set");
    // The revision already advanced (spawn bumped), so waitFor{baseline} takes the
    // catch-up path and returns at once — no blocking park.
    let after_spawn = conn
        .call("scene/waitFor", json!({ "since": before_spawn }))
        .expect("waitFor reports the spawn advance");
    assert_eq!(after_spawn["changed"], true, "the spawn advanced the scene");
    assert!(
        after_spawn["revision"].as_u64().unwrap_or(0) > before_spawn,
        "woke past the pre-spawn baseline: {after_spawn}"
    );

    // (b) The NEW pane's own output advances the notification. Send text to pane
    // `new_id` (NOT pane 0); `cat` echoes it → the mux-spawned pane's on_dirty bumps
    // the shared revision. Poll the non-blocking `scene/revision` (no parked waitFor).
    let before_output = read_revision(&mut conn);
    conn.call(
        "scene/invoke",
        json!({ "path": pane_input_path(new_id, TEXT_ACTION), "args": { "text": "pane1_marker_42\n" } }),
    )
    .expect("send text to the mux-spawned pane");
    assert!(
        wait_until(Duration::from_secs(5), || {
            read_revision(&mut conn) > before_output
        }),
        "the mux-spawned pane's own output never advanced the revision (latent bug)",
    );
    // And the bytes reached that pane specifically (not pane 0).
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(new_id, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.contains("pane1_marker_42")))
            .unwrap_or(false)
        }),
        "the text never echoed back through the mux-spawned pane",
    );

    let _ = std::fs::remove_file(&sock);
}

#[test]
fn a_mux_close_shrinks_the_set_and_advances_the_wire_notification() {
    // Round 2b's host-side trigger over the REAL socket: a mux `close` REMOVES a pane from
    // the served list AND advances the scene revision (the R118 set-SHRINK rail), so a
    // client long-polling change-notification learns the host lost a pane — exactly what
    // the GUI wire poll re-queries and mirrors as a freed slot.
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // Grow to two panes, capturing the 2nd pane's id.
    assert_eq!(pane_count(&mut conn), 1, "one boot pane");
    let spawned = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a 2nd pane over the wire");
    let victim = spawned.as_u64().expect("spawn returns the new pane id");
    assert_eq!(pane_count(&mut conn), 2, "the mux spawn grew the set");

    // Close the 2nd pane: the served set shrinks AND the revision advances.
    let before_close = read_revision(&mut conn);
    let closed = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(CLOSE_ACTION), "args": { "id": victim } }),
        )
        .expect("close the 2nd pane over the wire");
    // THE ANSWER, over a real socket (R309). The CLI test pins the SENTENCE a person reads and the
    // registry tests pin the cascade; this is the only place the BYTES a client parses are checked
    // end to end. This pane has a sibling, so the honest word is the cheapest one — which also
    // makes it the control for the escalations: a `close` that always claimed the window would pass
    // every cascade test and fail here.
    assert_eq!(
        closed["ended"].as_str(),
        Some("pane"),
        "the close says how far it reached, and it reached exactly the pane: {closed}",
    );
    assert_eq!(pane_count(&mut conn), 1, "the mux close shrank the set");
    assert!(
        !pane_ids(&mut conn).contains(&victim),
        "the closed pane's id is no longer served (the mirror would drop its slot)",
    );
    // waitFor{baseline} reports the close advanced the scene (the set-shrink rail bump), so
    // a parked poll wakes on the removal just as it does on output.
    let after_close = conn
        .call("scene/waitFor", json!({ "since": before_close }))
        .expect("waitFor reports the close advance");
    assert_eq!(after_close["changed"], true, "the close advanced the scene");
    assert!(
        after_close["revision"].as_u64().unwrap_or(0) > before_close,
        "woke past the pre-close baseline: {after_close}",
    );

    let _ = std::fs::remove_file(&sock);
}

#[test]
fn connect_fails_cleanly_when_no_host_is_listening() {
    // A short timeout against a path nothing bound: connect must error (not hang past
    // the timeout, not panic) — the boot-failure path WireHost turns into a reaped
    // child + a clean error.
    let sock = socket_path(); // never bound: no host is spawned for it
    let start = Instant::now();
    let result = HostConn::connect(&sock, Duration::from_millis(300));
    assert!(result.is_err(), "connect to an unbound socket must fail");
    assert!(
        matches!(
            result.err().map(|e| e.kind()),
            Some(ErrorKind::NotFound | ErrorKind::ConnectionRefused)
        ),
        "a clean connect error kind",
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "connect returned within the timeout window, no hang",
    );
}

/// Spawn a `cat` pane into the session named `session`, over the real socket — the pane-set
/// grows in THAT session and nowhere else. Returns the new pane's id.
fn spawn_in(conn: &mut HostConn, session: &str) -> u64 {
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
    .expect("spawn returns the new pane id")
}

/// The pane ids the session named `session` holds, over the mux `panes` slot.
fn pane_ids_in(conn: &mut HostConn, session: &str) -> Vec<u64> {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array()
            .map(|arr| arr.iter().filter_map(|p| p["id"].as_u64()).collect())
    })
    .unwrap_or_default()
}

/// The `(name, current)` of each window of the session named `session`, over the mux `windows`
/// slot — what a tabbed client draws from.
fn windows_in(conn: &mut HostConn, session: &str) -> Vec<(String, bool)> {
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .map(|w| {
                    (
                        w["name"].as_str().unwrap_or_default().to_owned(),
                        w["current"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
    })
    .unwrap_or_default()
}

/// Make `window` current in the session named `session`, over the mux `select_window` action.
fn select_window(conn: &mut HostConn, session: &str, window: &str) {
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": mux_action_path(SELECT_WINDOW_ACTION),
            "args": { "window": window },
        }),
    )
    .expect("select_window answers");
}

/// Read the host's current scene revision.
fn read_revision(conn: &mut HostConn) -> u64 {
    conn.call("scene/revision", json!({}))
        .ok()
        .and_then(|v: Value| v["revision"].as_u64())
        .unwrap_or(0)
}

/// The host's live pane count over the `/sprag_mux` control surface.
fn pane_count(conn: &mut HostConn) -> usize {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()
    .and_then(|v| v.as_array().map(Vec::len))
    .unwrap_or(0)
}

/// The host's live pane ids over the `/sprag_mux` control surface.
fn pane_ids(conn: &mut HostConn) -> Vec<u64> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array()
            .map(|arr| arr.iter().filter_map(|p| p["id"].as_u64()).collect())
    })
    .unwrap_or_default()
}

/// Poll `predicate` until true or `timeout` elapses.
fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// The detach/reattach arc's core wire claim, over a REAL host process: a window's
/// LOGICAL arrangement crosses the socket and deserialises back into the exact
/// [`LayoutTree`] the host holds. This is what will let a reattaching client restore the
/// user's layout instead of re-evening it — so it is proven against a real `sprag-term`,
/// not an in-process fake.
#[test]
fn the_window_layout_crosses_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // The boot pane alone arranges as a bare leaf — no split to divide.
    let (revision, layout) = read_layout(&mut conn);
    let boot = layout.panes();
    assert_eq!(boot.len(), 1, "the boot pane is arranged: {boot:?}");

    // Spawn a second pane: the arrangement grows a split over BOTH panes, in order.
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
    )
    .expect("spawn a second pane");

    let (grown, layout) = read_layout(&mut conn);
    assert_eq!(
        layout.panes().len(),
        2,
        "the spawned pane joined the arrangement: {:?}",
        layout.panes(),
    );
    assert!(grown > revision, "arranging a new pane moved the revision");
    // The tree is a real split (not two orphan roots), and carries no pixels.
    assert!(
        matches!(
            layout.root(),
            Some(sprag_terminal::LayoutNode::Split { .. })
        ),
        "two panes arrange as a split, got {:?}",
        layout.root(),
    );

    let _ = std::fs::remove_file(&sock);
}

/// A floated pane docks back into ITS OWN PLACE, across a real socket, against a real host.
///
/// THREE panes, and the middle one floated — because with two, "home" and "the end" are the
/// same position and a test cannot tell the mechanism from the coincidence. The pane's place
/// is session state the host captured; nothing the client says is involved, which is the
/// whole point: it survives the client that floated it going away.
#[test]
fn a_floated_pane_docks_back_at_its_home_across_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // Boot pane + two more: `0 | 1 | 2`.
    for _ in 0..2 {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a pane");
    }
    let (_, layout) = read_layout(&mut conn);
    let panes = layout.panes();
    assert_eq!(panes.len(), 3, "three panes tiled: {panes:?}");

    // Float the MIDDLE one out of the tiling.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(SET_FLOATING_ACTION),
            "args": { "id": panes[1].0, "floating": true },
        }),
    )
    .expect("the float write answers");
    let (_, layout) = read_layout(&mut conn);
    assert_eq!(
        layout.panes(),
        vec![panes[0], panes[2]],
        "the floated pane left the tiling",
    );

    // Dock it back with NO gesture to say where. Pre-anchor this answered `0 | 2 | 1`.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(SET_FLOATING_ACTION),
            "args": { "id": panes[1].0, "floating": false },
        }),
    )
    .expect("the dock-back write answers");
    let (_, layout) = read_layout(&mut conn);
    assert_eq!(
        layout.panes(),
        panes,
        "the pane came home to the middle, not to the end",
    );

    let _ = std::fs::remove_file(&sock);
}

/// A DIRECTIONAL split reaches a real daemon over a real socket and puts the pane where the
/// caller named — the op that makes a split expressible by a client that draws nothing.
///
/// THREE panes, and the FIRST one divided, for the same reason the float test floats the middle:
/// against a two-pane window, or against the last pane, "beside the target" and "at the end" are
/// the same position, and the test could not tell the split from the append it replaces. The
/// placement is asserted through [`LayoutTree::leaf_home`], the tree's own reciprocal reader, so
/// what is checked is the daemon agreeing with the request rather than a shape this test drew.
#[test]
fn a_directional_split_lands_where_the_caller_named_across_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // Boot pane + two more: `0 | 1 | 2`.
    for _ in 0..2 {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a pane");
    }
    let (_, layout) = read_layout(&mut conn);
    let panes = layout.panes();
    assert_eq!(panes.len(), 3, "three panes tiled: {panes:?}");

    // Divide the FIRST pane, vertically. An append would put the new pane last.
    let answer = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SPLIT_ACTION),
                "args": { "pane": panes[0].0, "dir": "vertical", "cmd": ["cat"] },
            }),
        )
        .expect("the split answers");
    let fresh = sprag_terminal::PaneId(answer.as_u64().expect("a pane id"));

    let (_, layout) = read_layout(&mut conn);
    assert_eq!(
        layout.panes(),
        vec![panes[0], fresh, panes[1], panes[2]],
        "the new pane landed under pane 0, not appended after pane 2",
    );
    assert_eq!(
        layout.leaf_home(fresh),
        Some(sprag_terminal::LeafHome::beside(
            panes[0],
            sprag_terminal::SplitSide::Second,
            sprag_terminal::SplitDir::Vertical,
        )),
        "on the axis and the side the request named",
    );

    // A target the window does not tile is REFUSED, and refused before anything is forked.
    let refused = conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(SPLIT_ACTION),
            "args": { "pane": 9999, "dir": "horizontal", "cmd": ["cat"] },
        }),
    );
    assert!(refused.is_err(), "an unreachable target is refused");
    let (_, layout) = read_layout(&mut conn);
    assert_eq!(
        layout.panes().len(),
        4,
        "and a refused split spawns no pane: {:?}",
        layout.panes(),
    );

    let _ = std::fs::remove_file(&sock);
}

/// Two sessions under ONE daemon are independent, over a REAL socket — the shape the owner's
/// several-windows workflow needs once one process holds every session (the point of C1a).
///
/// A single connection creates `work`, then drives BOTH sessions by naming each on the wire.
/// Every assertion is paired with its complement: `work`'s panes are `work`'s AND not the
/// default's, its arrangement is its own AND leaves the default's untouched. A daemon that
/// merged the two — or answered whichever session happened to be first — fails the second
/// half of each pair, which the first half alone could not catch.
#[test]
fn two_sessions_under_one_daemon_are_independent_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // The daemon boots with one session, "0", holding its boot `cat` pane (id 0).
    let sessions = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(SESSIONS_SLOT) }),
        )
        .expect("the sessions slot answers");
    assert_eq!(
        sessions.as_array().map(Vec::len),
        Some(1),
        "one session at boot: {sessions}",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "the default's boot pane"
    );

    // Create a second session by name, over the wire. It is BORN with one pane (tmux's
    // new-session) — landed in `work`, not the default.
    let created = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    assert_eq!(
        created, "work",
        "the answer is the name to scope with: {created}"
    );
    let born = pane_ids_in(&mut conn, "work");
    assert_eq!(
        born.len(),
        1,
        "work is born with its shell, never empty: {born:?}",
    );
    let birth = born[0];
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "...and the birth pane landed in work, not the default",
    );

    // Spawn a second pane into `work`. Ids come from ONE registry-wide counter, so the birth
    // pane and this spawn are both distinct from the default's 0 — which is what lets the sets
    // be told apart.
    let w = spawn_in(&mut conn, "work");
    assert_eq!(
        pane_ids_in(&mut conn, "work"),
        vec![birth, w],
        "work holds its birth pane and the one spawned into it",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "...and the default still holds only its boot pane — the two do not merge",
    );

    // Arrange `work`'s two panes as a vertical split at a dragged ratio, naming `work`.
    let (rev, _) = read_layout_in(&mut conn, "work");
    conn.call(
        "scene/invoke",
        json!({
            "session": "work",
            "path": mux_action_path(SET_LAYOUT_ACTION),
            "args": { "expected_revision": rev, "tree": { "root": { "split": {
                "dir": "vertical",
                "ratio": 0.8,
                "first": { "leaf": birth },
                "second": { "leaf": w },
            } } } },
        }),
    )
    .expect("work's arrangement write answers");

    // work's window carries the split; the default's window is still a lone boot leaf.
    let (_, work_tree) = read_layout_in(&mut conn, "work");
    assert!(
        matches!(
            work_tree.root(),
            Some(sprag_terminal::LayoutNode::Split {
                dir: sprag_terminal::SplitDir::Vertical,
                ..
            })
        ),
        "work holds the vertical split it was given: {:?}",
        work_tree.root(),
    );
    let (_, default_tree) = read_layout_in(&mut conn, "0");
    assert_eq!(
        default_tree.root(),
        Some(&sprag_terminal::LayoutNode::Leaf(sprag_terminal::PaneId(0))),
        "the default session's arrangement was never touched by work's gesture",
    );

    // Closing work's spawned pane leaves the default's set alone — lifecycle is scoped too.
    conn.call(
        "scene/invoke",
        json!({ "session": "work", "path": mux_action_path(CLOSE_ACTION), "args": { "id": w } }),
    )
    .expect("close in work answers");
    assert_eq!(
        pane_ids_in(&mut conn, "work"),
        vec![birth],
        "work lost its spawned pane, keeping its birth pane",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "...and the default is untouched by a close in another session",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Killing a session over the real socket, the wire MECHANISM the GUI's `WireHost::kill_session`
/// rests on: a NON-last kill REMOVES that session and the daemon keeps serving the rest, and a read
/// naming the killed session is REFUSED (an error, not an empty-but-ok set). That refusal is exactly
/// the `Other` error the GUI's poll-thread `detach_reason` converts into a DETACH — so this proves,
/// at the wire level, both halves the client relies on: kill-ANOTHER keeps this client serving, and
/// kill-OWN (a client scoped to the killed session) has its next read refused → the detach.
///
/// Each claim is paired with its complement (the survivor's set still answers), so a daemon that
/// ignored the kill, or one that answered a dead scope with an empty set instead of an error, fails
/// the pair. REVERT-PROOF: if the kill did not remove the session, `work`'s read would still succeed
/// and the sessions slot would still list two.
///
/// SCOPE (as with [`re_scoping_one_connection_switches_which_session_it_serves_over_the_real_socket`]):
/// only the wire semantics — NOT `WireHost::kill_session`'s own→`request_quit` / other→
/// `refresh_sessions` branch, which lives in `sprag-gui` (a bin crate, `WireHost` is `pub(crate)`)
/// and is covered by the reducer routing test + the live smoke, not reachable from here. The
/// last-session kill ending the daemon is proven by the CLI/workspace tests, not repeated here.
#[test]
fn killing_a_session_over_the_real_socket_refuses_its_reads_and_keeps_the_others() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // Boot session "0" (pane 0); create a second, "work", born with its own pane.
    let created = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    assert_eq!(created, "work");
    assert_eq!(session_names(&mut conn), vec!["0", "work"], "two sessions");
    assert_eq!(
        pane_ids_in(&mut conn, "work").len(),
        1,
        "work answers a scoped read while it lives",
    );

    // Kill "work" — a NON-last kill: the daemon keeps serving. The reply comes back Ok (only the
    // LAST kill severs it), so this is a plain successful call.
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(KILL_SESSION_ACTION), "args": { "name": "work" } }),
    )
    .expect("kill_session of a non-last session answers Ok");

    // The killed session's scoped read is now REFUSED (an error) — the detach trigger. NOT merely an
    // empty set: a client scoped to `work` must be TOLD its session is gone, so it detaches rather
    // than paint a blank window over nothing.
    let refused = conn.call(
        "scene/query",
        json!({ "session": "work", "path": mux_action_path(PANES_SLOT) }),
    );
    assert!(
        refused.is_err(),
        "a read of the killed session is refused, not answered empty: {refused:?}",
    );

    // ...and the daemon kept serving the survivor: only "0" remains, and it still answers.
    assert_eq!(
        session_names(&mut conn),
        vec!["0"],
        "kill-another dropped only work; the daemon lives on",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "the survivor's boot pane still answers",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The names of every session on the host, in list order — off the registry-wide `sessions` slot.
fn session_names(conn: &mut HostConn) -> Vec<String> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_owned))
                .collect()
        })
    })
    .unwrap_or_default()
}

/// How many clients the daemon reports as ATTACHED to `session`, off the registry-wide `sessions`
/// slot. An unattached session serialises the field away (`skip_serializing_if`), so its absence
/// reads back as 0 — the exact additive contract [`SessionInfo::attached`] promises.
fn attached_of(conn: &mut HostConn, session: &str) -> u64 {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().and_then(|arr| {
            arr.iter()
                .find(|s| s["name"].as_str() == Some(session))
                .map(|s| s["attached"].as_u64().unwrap_or(0))
        })
    })
    .unwrap_or(0)
}

/// R-PR67 Stage 1 end to end over the REAL socket: a client that announces itself (`client/hello`)
/// and attaches (`client/attach`) is COUNTED on the session's `attached` badge, and — the
/// crash-safe property — that count is RELEASED when the client's connection CLOSES, with no
/// explicit detach. The release rides the transport's `on_disconnect`, so however the client goes
/// away (here a dropped `HostConn`; in the field a crash) the daemon never leaks the session as
/// "attached". This is the deliverable that could not be built before pinion R1393 — the whole
/// motivation of PINION-PR67 — so it is pinned over the real transport, not just the unit registry.
///
/// An OBSERVER connection (which never attaches, so never counts) reads the badge throughout, so
/// the count it sees is the ATTACHER's alone — and its own steady 0 proves that merely connecting
/// is not attaching. The post-close assertion POLLS, because `on_disconnect` is delivered
/// asynchronously on the daemon's per-connection reader thread; the bound is generous and the
/// steady state is exact (0), so it is a fact, not a race.
#[test]
fn client_attachment_is_counted_and_released_on_disconnect_over_the_real_socket() {
    let (_host, sock) = spawn_host();

    // The observer only READS the badge — it never sends client/hello or client/attach.
    let mut observer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("observer connects to the host");
    let session = session_names(&mut observer)
        .into_iter()
        .next()
        .expect("the daemon boots with its default session");
    assert_eq!(
        attached_of(&mut observer, &session),
        0,
        "a bare connection is not an attachment",
    );

    {
        // The attacher announces a client id, then attaches to the default session (unscoped).
        let mut attacher = HostConn::connect(&sock, Duration::from_secs(5))
            .expect("attacher connects to the host");
        attacher
            .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "test-client" }))
            .expect("client/hello is accepted");
        attacher
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach is accepted");

        assert!(
            wait_until(Duration::from_secs(5), || attached_of(
                &mut observer,
                &session
            ) == 1),
            "the attach must show on the badge the observer reads",
        );
        // `attacher` drops at the end of this block: its socket closes with no detach sent.
    }

    assert!(
        wait_until(Duration::from_secs(5), || attached_of(
            &mut observer,
            &session
        ) == 0),
        "the closed connection's attachment must be released (on_disconnect), not leaked",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The `clients` slot (tmux `list-clients`, behind `sprag list-clients`) over the REAL socket: it
/// lists one `{client, session}` row per ATTACHED client, and releases it when the connection
/// closes — the same crash-safe lifecycle as the `attached` count, read as a per-client list.
///
/// This pins the slot's WIRE SHAPE (`[{client, session}]`) directly, independent of the CLI that
/// consumes it: an OBSERVER connection (never attached) reads the slot throughout, so it sees
/// exactly the attacher's row and its own absence proves a bare connection is not listed.
#[test]
fn the_clients_slot_lists_attached_clients_and_releases_them_over_the_real_socket() {
    let (_host, sock) = spawn_host();

    let mut observer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("observer connects to the host");
    let session = session_names(&mut observer)
        .into_iter()
        .next()
        .expect("the daemon boots with its default session");
    assert!(
        clients_of(&mut observer).is_empty(),
        "no client has attached yet, so the clients slot is empty",
    );

    {
        let mut attacher = HostConn::connect(&sock, Duration::from_secs(5))
            .expect("attacher connects to the host");
        attacher
            .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "wire-client" }))
            .expect("client/hello is accepted");
        attacher
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach is accepted");

        assert!(
            wait_until(Duration::from_secs(5), || {
                clients_of(&mut observer) == vec![("wire-client".to_owned(), session.clone())]
            }),
            "the slot lists the attached client and the session it views",
        );
        // attacher drops here: its socket closes with no detach sent.
    }

    assert!(
        wait_until(Duration::from_secs(5), || clients_of(&mut observer)
            .is_empty()),
        "the closed connection's client leaves the slot (on_disconnect), not leaks",
    );

    let _ = std::fs::remove_file(&sock);
}

/// An attention notification (`OSC 9`) a CHILD raises reaches the wire `panes` slot as
/// `{notification: {title, body, seq}}` — the deliverable behind the pane attention badge.
///
/// The boot pane runs a shell that emits `OSC 9 ; from-child BEL` once, then sleeps to hold the
/// session open. The bytes flow through the REAL pipeline (child stdout -> PTY -> emulator OSC
/// parse -> latched notification -> panes slot), so this pins the whole vertical, not the unit
/// parse. Polled, because the child's first write and the reader thread applying it are async.
#[test]
fn a_child_raised_osc_9_notification_reaches_the_panes_slot() {
    let (_host, sock) =
        spawn_host_running(&["sh", "-c", "printf '\\033]9;from-child\\007'; sleep 30"]);
    let mut conn =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the spawned host");

    assert!(
        wait_until(Duration::from_secs(5), || {
            notification_of(&mut conn).is_some_and(|(title, body, seq)| {
                title.is_none() && body == "from-child" && seq >= 1
            })
        }),
        "the child's OSC 9 must surface on the panes slot as a body-only notification",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The boot pane's `(title, body, seq)` notification off the `panes` slot, or `None` when it has
/// none — the wire shape a client's attention badge reads.
fn notification_of(conn: &mut HostConn) -> Option<(Option<String>, String, u64)> {
    let panes = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .ok()?;
    let note = panes.as_array()?.first()?.get("notification")?;
    Some((
        note["title"].as_str().map(str::to_owned),
        note["body"].as_str()?.to_owned(),
        note["seq"].as_u64()?,
    ))
}

/// A BELL (`\a`) a CHILD rings reaches the wire `panes` slot as `bell_seq` — the deliverable behind
/// the tmux monitor-bell attention marker, kept SEPARATE from the notification (a bell carries no
/// text). The boot pane rings the bell twice, then sleeps to hold the session open; the bytes flow
/// through the REAL pipeline (child stdout -> PTY -> emulator control parse -> bell_seq -> panes
/// slot), so this pins the whole vertical, not the unit count. Polled, since the child's write and
/// the reader thread applying it are async.
#[test]
fn a_child_rung_bell_reaches_the_panes_slot() {
    let (_host, sock) = spawn_host_running(&["sh", "-c", "printf '\\007\\007'; sleep 30"]);
    let mut conn =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the spawned host");

    assert!(
        wait_until(Duration::from_secs(5), || {
            bell_seq_of(&mut conn).is_some_and(|seq| seq >= 2)
        }),
        "the child's two bells must surface on the panes slot as bell_seq >= 2",
    );
    // A bell is NOT a notification — it carries no text, so the notification key stays absent.
    assert!(
        notification_of(&mut conn).is_none(),
        "a bell must not masquerade as a notification",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The boot pane's `bell_seq` off the `panes` slot, or `None` when the key is absent (rang none).
fn bell_seq_of(conn: &mut HostConn) -> Option<u64> {
    let panes = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .ok()?;
    panes.as_array()?.first()?.get("bell_seq")?.as_u64()
}

/// A CHILD's OSC 133 (FinalTerm) shell-integration cycle reaches the wire `panes` slot as
/// `{shell, exit_status}` — the deliverable behind the "idle at a prompt vs running a command"
/// summary an AI sibling reads. The boot pane emits a full cycle (A prompt, C output, D exit 3)
/// through the REAL pipeline (child stdout -> PTY -> emulator OSC 133 parse -> screen marks ->
/// derived summary -> panes slot), then sleeps to hold the session open. Polled, since the write
/// and the reader thread applying it are async.
#[test]
fn a_child_osc_133_cycle_reaches_the_panes_slot() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]133;A\\007$ \\033]133;C\\007out\\033]133;D;3\\007'; sleep 30",
    ]);
    let mut conn =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the spawned host");

    assert!(
        wait_until(Duration::from_secs(5), || {
            shell_of(&mut conn) == (Some("at_prompt".to_owned()), Some(3))
        }),
        "the child's OSC 133 cycle must surface as shell=at_prompt + exit_status=3 on the panes slot",
    );

    let _ = std::fs::remove_file(&sock);
}

/// A child that emits an OSC 8 hyperlink surfaces on the `links` slot as a `{text, uri}` run — the
/// link's DESTINATION as data, end-to-end over the real socket. The tmux-superior surface: tmux's
/// `capture-pane` flattens OSC 8 to plain text and drops the URI, so an agent there cannot read a
/// link's target at all.
#[test]
fn a_child_osc_8_hyperlink_reaches_the_links_slot() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]8;;https://example.com/spec\\007docs\\033]8;;\\007'; sleep 30",
    ]);
    let mut conn =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the spawned host");

    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(0, LINKS_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_array().cloned())
            .is_some_and(|arr| {
                arr.iter().any(|run| {
                    run["text"].as_str() == Some("docs")
                        && run["uri"].as_str() == Some("https://example.com/spec")
                })
            })
        }),
        "the child's OSC 8 link must surface as a {{text: docs, uri: ...}} run on the links slot",
    );

    let _ = std::fs::remove_file(&sock);
}

/// A child that transmits a Kitty RGBA image surfaces a SUMMARY on the panes slot
/// (`{id,width,height,anchor,seq}`, NO rgba), and the RGBA is served ON DEMAND via `image_data.<id>`
/// (R1404 Stage 5) — the raster does not ride the per-poll panes slot. tmux shows no inline images.
#[test]
fn a_child_kitty_image_summarises_on_the_panes_slot_and_serves_rgba_on_demand() {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    let rgba: Vec<u8> = [255u8, 0, 0, 255].repeat(4); // 2x2 opaque red = 16 bytes
    let b64 = STANDARD.encode(&rgba);
    let cmd = format!("printf '\\033_Ga=T,f=32,s=2,v=2,i=1;{b64}\\033\\\\'; sleep 30");
    let (_host, sock) = spawn_host_running(&["sh", "-c", &cmd]);
    let mut conn =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the spawned host");

    // The panes slot carries the SUMMARY only — id/size/anchor/seq, and NO rgba on the poll.
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": mux_action_path(PANES_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_array().and_then(|a| a.first().cloned()))
            .and_then(|p| p["images"].as_array().cloned())
            .is_some_and(|imgs| {
                imgs.iter().any(|im| {
                    im["id"].as_u64() == Some(1)
                        && im["width"].as_u64() == Some(2)
                        && im["height"].as_u64() == Some(2)
                        && im["seq"].is_u64()
                        && im.get("rgba_b64").is_none() // the raster does NOT ride the poll
                })
            })
        }),
        "the image summary (id/size/anchor/seq, NO rgba) must reach the panes slot",
    );

    // The RGBA is fetched ON DEMAND via image_data.<id> and matches the transmit.
    let fetched = conn
        .call(
            "scene/query",
            json!({ "path": pane_input_path(0, "image_data.1") }),
        )
        .expect("image_data.1 query")
        .as_str()
        .and_then(|s| STANDARD.decode(s).ok());
    assert_eq!(
        fetched.as_deref(),
        Some(rgba.as_slice()),
        "image_data.1 serves the transmitted RGBA on demand",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The boot pane's `(shell, exit_status)` off the `panes` slot — the wire shape an agent's
/// idle/running + exit summary reads. Either is `None` when its key is absent (no shell
/// integration / no finished command).
fn shell_of(conn: &mut HostConn) -> (Option<String>, Option<i64>) {
    let panes = match conn.call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    ) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let Some(pane) = panes.as_array().and_then(|arr| arr.first()) else {
        return (None, None);
    };
    (
        pane["shell"].as_str().map(str::to_owned),
        pane["exit_status"].as_i64(),
    )
}

/// The `clients` slot as `(client, session)` pairs — the wire shape `sprag list-clients` parses.
fn clients_of(conn: &mut HostConn) -> Vec<(String, String)> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(CLIENTS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    Some((
                        c["client"].as_str()?.to_owned(),
                        c["session"].as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
    })
    .unwrap_or_default()
}

/// Re-scoping ONE persistent connection from session to session — the wire MECHANISM the GUI's
/// `WireHost::switch_session` rests on for its REQUEST connection. `HostConn::scope_to` makes the
/// session sticky, so after a re-scope every unscoped read serves the NEW session's panes, and
/// re-scoping back serves the first again. Distinct from
/// [`two_sessions_under_one_daemon_are_independent_over_the_real_socket`], which scopes each read
/// per-request; this proves the STICKY re-point a switch relies on. Each claim is paired with its
/// complement (the OTHER session's set), so a daemon that ignored the re-scope fails the pair.
///
/// SCOPE OF THIS TEST: only the `scope_to` primitive over the real socket — NOT `WireHost`'s
/// orchestration around it (poll-thread teardown + respawn, the transactional mirror re-boot, the
/// fallback-to-previous). Those live in `sprag-gui` (a bin crate; `WireHost` is `pub(crate)`) and
/// need a real `sprag-term`, which isn't reachable from here — so, as with the other `WireHost`
/// methods, they are proven by the live pixel smoke (session sidebar: switch A→B→A, a marker
/// returns; "+" creates + switches), not a unit test.
#[test]
fn re_scoping_one_connection_switches_which_session_it_serves_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // The daemon boots session "0" holding its boot pane (id 0). Create "work", born with its own
    // pane — a DISTINCT id from one registry-wide counter, which is what lets the two sets be told
    // apart after a switch.
    let created = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    assert_eq!(created, "work", "the answer is the name to scope with");
    let work_set = pane_ids_in(&mut conn, "work");
    assert_eq!(
        work_set.len(),
        1,
        "work is born with one pane: {work_set:?}"
    );
    assert_ne!(work_set, vec![0], "...distinct from the default's pane 0");

    // STICK the connection to "0" (scope_to) and read WITHOUT a per-request session — the sticky
    // scope answers. It serves the default's pane.
    conn.scope_to("0".to_owned());
    assert_eq!(
        pane_ids(&mut conn),
        vec![0],
        "scoped to 0 -> the default's pane"
    );

    // Re-scope the SAME connection to "work" (the switch). The same unscoped read now serves work's
    // pane, not the default's — exactly switch_session's request-conn re-point.
    conn.scope_to("work".to_owned());
    assert_eq!(
        pane_ids(&mut conn),
        work_set,
        "re-scoped to work -> work's pane"
    );
    assert_ne!(
        pane_ids(&mut conn),
        vec![0],
        "...and no longer the default's"
    );

    // The switch is reversible: re-scope back to "0" and the default is intact.
    conn.scope_to("0".to_owned());
    assert_eq!(
        pane_ids(&mut conn),
        vec![0],
        "switched back -> the default again"
    );

    let _ = std::fs::remove_file(&sock);
}

/// A client scoped to its ATTACHMENT follows a `rename-session`, over the REAL socket — and the
/// same client scoped by NAME does not. The whole of R303's thesis, driven through the daemon.
///
/// The three readings are the three the instrument took against a live daemon before a line was
/// written, in order:
///
/// 1. **Before**: both clients read their session. (The control that makes the rest mean
///    something — a `scope_to_attached` that never worked would fail here.)
/// 2. **After the rename**: the ATTACHED client still reads the same session under its new name;
///    the NAME-scoped one is refused, which is what its poll thread reads as "my session is gone"
///    and leaves on.
/// 3. **After a NEW session takes the retired name**: the name-scoped client is served — by the
///    IMPOSTOR. This is the sharp half: not a refusal but a success, against a session it never
///    named, on the connection it also sends keystrokes down. The attached client is untouched.
///
/// The two sessions are told apart by their WINDOW names rather than by the session name, because
/// the session name is the very thing under test: `orig` and `impostor` are facts the rename cannot
/// move, so a reply naming one of them says which registry entry answered.
#[test]
fn an_attached_client_follows_a_rename_where_a_name_scoped_one_is_captured_by_an_impostor() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    let rename_window = |conn: &mut HostConn, session: &str, to: &str| {
        conn.call(
            "scene/invoke",
            json!({
                "session": session,
                "path": mux_action_path(RENAME_WINDOW_ACTION),
                "args": { "name": to },
            }),
        )
        .expect("rename_window answers");
    };
    rename_window(&mut admin, "work", "orig");

    // The DISPLAY client: hello, attach by name, then off the name and onto the attachment — the
    // exact sequence `sprag-client`'s `attach_and_follow` performs at boot.
    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    viewer.scope_to("work".to_owned());
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");
    viewer.scope_to_attached();

    // The client this round is fixing: identical, except that it keeps re-sending the name.
    let mut by_name =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the name-scoped client connects");
    by_name
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "by-name" }))
        .expect("client/hello is accepted");
    by_name.scope_to("work".to_owned());
    by_name
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");

    // What each connection's requests are ABOUT, in the daemon's own words. Read here rather than
    // only in the live client test because THIS fixture can discriminate: `work` is not the
    // daemon's default session, so a slot that answered "the default" instead of "the scope" gives
    // a different string. (It was written the other way round first, in a fixture where the
    // client's session WAS the default — and a revert-proof that broke the slot passed.)
    let scope_name = |conn: &mut HostConn| -> Option<String> {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(SESSION_SLOT) }),
        )
        .ok()?
        .as_str()
        .map(str::to_owned)
    };
    let default_session = scope_name(&mut admin).expect("an unscoped read names the default");
    assert_ne!(
        default_session, "work",
        "the fixture only discriminates if the viewed session is NOT the default",
    );
    assert_eq!(
        scope_name(&mut viewer).as_deref(),
        Some("work"),
        "an attached-scoped read is about the session this client is viewing",
    );

    // 1. BEFORE — the control.
    let window_of = |conn: &mut HostConn| -> Option<String> {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(WINDOWS_SLOT) }),
        )
        .ok()?
        .as_array()?
        .iter()
        .find(|w| w["current"].as_bool().unwrap_or(false))?["name"]
            .as_str()
            .map(str::to_owned)
    };
    assert_eq!(window_of(&mut viewer).as_deref(), Some("orig"));
    assert_eq!(window_of(&mut by_name).as_deref(), Some("orig"));

    // The rename, on a THIRD connection — a person typing `sprag rename-session`.
    admin
        .call(
            "scene/invoke",
            json!({
                "session": "work",
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": "prod" },
            }),
        )
        .expect("rename_session answers");

    // 2. AFTER — the attached client is still on its own session; the name-scoped one is refused.
    assert_eq!(
        window_of(&mut viewer).as_deref(),
        Some("orig"),
        "the attachment moved with the rename, so this client never noticed it",
    );
    assert!(
        by_name
            .call(
                "scene/query",
                json!({ "path": mux_action_path(WINDOWS_SLOT) })
            )
            .is_err(),
        "a client re-sending the retired name is refused — the detach measured at R303",
    );

    // 3. THE IMPOSTOR — a new session takes the freed name and captures the name-scoped client.
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    rename_window(&mut admin, "work", "impostor");
    assert_eq!(
        window_of(&mut by_name).as_deref(),
        Some("impostor"),
        "the name-scoped client is SERVED, by a session it never named — the silent half",
    );
    assert_eq!(
        window_of(&mut viewer).as_deref(),
        Some("orig"),
        "while the attached client is on the session it has been viewing all along",
    );
    // ...and the daemon tells it the NEW name, which is what a client whose scope is no longer a
    // name has to be told in order to label itself.
    assert_eq!(
        scope_name(&mut viewer).as_deref(),
        Some("prod"),
        "the attached scope's name follows the rename",
    );
    assert_eq!(
        scope_name(&mut admin).as_deref(),
        Some(default_session.as_str()),
        "control: an unscoped read still names the default, which the rename did not touch",
    );

    let _ = std::fs::remove_file(&sock);
}

/// **R304 over the REAL socket**: a client asks to go back where it was, and the daemon answers the
/// session it actually VISITED — across a rename, and never the impostor wearing the freed name.
///
/// The one-verb-over twin of the test above, and the same three readings in the same order. What
/// makes it a different claim: R303 fixed the session a client is VIEWING, where a hook at the
/// rename can keep a name true; this is a session it HAS VIEWED, which no hook can reach — so the
/// answer has to come from an identity the daemon kept.
///
/// The fixture is built so the two answers DISAGREE at every step: the visited session is renamed
/// (so a remembered name resolves to nothing) AND a live impostor takes its name (so the remembered
/// name resolves to a stranger). A history of names lands on `impostor`; a history of identities
/// lands on `renamed`. Both are live, so nothing here can pass by accident.
#[test]
fn a_client_goes_back_to_the_session_it_visited_not_to_the_name_it_wore() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    let new_session = |conn: &mut HostConn, name: &str| {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": name } }),
        )
        .expect("new_session answers");
    };
    new_session(&mut admin, "work");
    new_session(&mut admin, "here");

    // A display client that visits `work` and then moves to `here` — `attach_and_follow`'s exact
    // sequence, twice, which is what a switch is.
    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    let attach_to = |conn: &mut HostConn, session: &str| -> Value {
        conn.scope_to(session.to_owned());
        let landed = conn
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach is accepted");
        conn.scope_to_attached();
        landed
    };
    assert_eq!(
        attach_to(&mut viewer, "work"),
        json!("work"),
        "an attach answers the session it landed on, which is what makes both arms one path",
    );
    attach_to(&mut viewer, "here");

    // The visited session is renamed, and a NEW session takes the name it wore.
    admin
        .call(
            "scene/invoke",
            json!({
                "session": "work",
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": "renamed" },
            }),
        )
        .expect("rename_session answers");
    new_session(&mut admin, "work");
    assert!(
        session_names(&mut admin).contains(&"work".to_owned()),
        "the impostor is LIVE — a history of names would resolve straight onto it",
    );

    // The gesture: tmux `switch-client -l`, on the connection that is scoped to the attachment.
    let go_back = |conn: &mut HostConn, unattached: bool| -> Value {
        let mut params = serde_json::Map::new();
        sprag_host::wire::AttachAsk::LastViewed { unattached }.write_into(&mut params);
        conn.call(CLIENT_ATTACH_METHOD, Value::Object(params))
            .expect("client/attach is accepted")
    };
    assert_eq!(
        go_back(&mut viewer, false),
        json!("renamed"),
        "the session it VISITED, under the name that session has now — never the impostor",
    );
    assert_eq!(
        attached_of(&mut admin, "renamed"),
        1,
        "and it is really there: the daemon counts it on that session's badge",
    );
    assert_eq!(
        attached_of(&mut admin, "work"),
        0,
        "...and not on the stranger's",
    );

    // Going back is itself a visit, so the answer toggles — tmux's own `switch-client -l`.
    assert_eq!(go_back(&mut viewer, false), json!("here"));

    // A client with nowhere to go back to is ANSWERED, not refused: `null`, and it stays put.
    let mut fresh =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("a fresh client connects");
    fresh
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "fresh" }))
        .expect("client/hello is accepted");
    attach_to(&mut fresh, "here");
    assert_eq!(
        go_back(&mut fresh, false),
        Value::Null,
        "a client that never switched has no last session, and that is an answer",
    );
    assert_eq!(
        attached_of(&mut admin, "here"),
        2,
        "and a null answer moved nothing: it is still where it was",
    );

    // The `unattached` narrowing (tmux `detach-on-destroy no-detached`), answered from the daemon's
    // own attachment map: `here` is where `viewer` went back to, and `fresh` is sitting on it, so
    // the narrowed ask skips it for the next session in that client's history.
    assert_eq!(
        go_back(&mut viewer, true),
        json!("renamed"),
        "the narrowed ask skips the session another client is viewing",
    );

    let _ = std::fs::remove_file(&sock);
}

/// **R314 over the REAL socket**: a client asks for *the next session* and the DAEMON walks the
/// ring — from where that client actually is, answering the name it landed on.
///
/// THREE sessions and the client starts in the MIDDLE, deliberately: from there `next` and
/// `previous` name different rows, so a walk that ignored the direction — or one that always
/// answered the first or the last — could not pass. The origin is never sent: the client says
/// only which way, and the daemon reads its own attachment map for where.
///
/// ⚠ **What this test deliberately does NOT claim.** The walk is over
/// `sprag_host::host::listable_sessions`, the same builder the `sessions` slot paints from, so a
/// step can only land where a list would show. That is unobservable HERE and it was measured
/// rather than assumed: a daemon gives its boot session a pane whether or not `--` names one, and
/// R309 ends a session whose last pane goes — so every session a live daemon holds is listable and
/// the two orders coincide. The distinction is kept because the two must not be able to come
/// apart, and it is driven where the state can actually be built (`rpc::tests::step_along`, and
/// `sessions_hides_the_empty_anchor_and_lists_a_worked_session` for the paneless anchor).
#[test]
fn a_client_steps_along_the_session_ring_the_daemon_walks_from_where_it_is() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    let new_session = |conn: &mut HostConn, name: &str| {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": name } }),
        )
        .expect("new_session answers");
    };
    for name in ["alpha", "beta"] {
        new_session(&mut admin, name);
    }
    // The ring the daemon will walk, read the way a user reads it. Named here so the assertions
    // below are about THIS order rather than about an order the test assumed.
    assert_eq!(
        session_names(&mut admin),
        vec!["0".to_owned(), "alpha".to_owned(), "beta".to_owned()],
        "the boot session, then the two created ones, in the registry's own order",
    );

    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    let attach_to = |conn: &mut HostConn, session: &str| -> Value {
        conn.scope_to(session.to_owned());
        let landed = conn
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach is accepted");
        conn.scope_to_attached();
        landed
    };
    let step = |conn: &mut HostConn, step: sprag_terminal::OrderStep| -> Value {
        let mut params = serde_json::Map::new();
        sprag_host::wire::AttachAsk::Step(step).write_into(&mut params);
        conn.call(CLIENT_ATTACH_METHOD, Value::Object(params))
            .expect("client/attach is accepted")
    };
    attach_to(&mut viewer, "alpha");

    // From the MIDDLE of three the two directions DISAGREE, which is what makes these two lines
    // discriminate rather than merely pass.
    assert_eq!(
        step(&mut viewer, sprag_terminal::OrderStep::Next),
        json!("beta"),
    );
    assert_eq!(
        step(&mut viewer, sprag_terminal::OrderStep::Previous),
        json!("alpha"),
        "and back: the ring is walked from where the client NOW is, never from a fixed origin",
    );
    assert_eq!(
        attached_of(&mut admin, "alpha"),
        1,
        "the client really moved — the daemon counts it on that session's badge",
    );
    assert_eq!(
        attached_of(&mut admin, "beta"),
        0,
        "...and not on the one it left",
    );

    // BOTH WRAPS, from the two ends.
    attach_to(&mut viewer, "beta");
    assert_eq!(
        step(&mut viewer, sprag_terminal::OrderStep::Next),
        json!("0"),
        "past the last is the first",
    );
    assert_eq!(
        step(&mut viewer, sprag_terminal::OrderStep::Previous),
        json!("beta"),
        "before the first is the last",
    );

    // A SECOND CLIENT steps from ITS OWN attachment, not from the first one's — the fact that
    // makes the origin the attachment map rather than anything the request carries.
    let mut other =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("a second client connects");
    other
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "other" }))
        .expect("client/hello is accepted");
    attach_to(&mut other, "alpha");
    // Each connection is scoped to its OWN attachment (`scope_to_attached`), so this slot reads
    // where that client is — the disagreement the assertion after it rests on.
    let where_it_is = |conn: &mut HostConn| -> Option<String> {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(SESSION_SLOT) }),
        )
        .ok()?
        .as_str()
        .map(str::to_owned)
    };
    assert_ne!(
        where_it_is(&mut viewer),
        where_it_is(&mut other),
        "the two clients are on DIFFERENT sessions, so the next assertion discriminates",
    );
    assert_eq!(
        step(&mut other, sprag_terminal::OrderStep::Next),
        json!("beta"),
        "the second client steps from alpha, where IT is, not from beta where the first one is",
    );

    // Every malformed target is refused rather than resolved — over the socket, where a caller
    // learns it from a sentence.
    let refused = |conn: &mut HostConn, params: Value| -> String {
        match conn.call(CLIENT_ATTACH_METHOD, params) {
            Err(error) => error.to_string(),
            Ok(answer) => panic!("expected a refusal, got {answer}"),
        }
    };
    let sentence = refused(&mut viewer, json!({ "step": "sideways" }));
    assert!(
        sentence.contains("next") && sentence.contains("previous"),
        "the refusal names the two words a caller may use: {sentence}",
    );
    let both = refused(&mut viewer, json!({ "step": "next", "last": true }));
    assert!(
        both.contains("ask for one"),
        "two targets is no target: {both}",
    );
    assert_eq!(
        attached_of(&mut admin, "beta"),
        2,
        "and neither refusal moved anybody: both clients stepped onto beta and are still there",
    );

    // A CONNECTION THAT NEVER ATTACHED steps from its SCOPE — the one arm of the walk that is about
    // the ATTACHMENT MAP rather than about the list, and the branch `step_along`'s unit test cannot
    // reach. It said hello (so the host knows its client) and never sent `client/attach`, so
    // `session_of` answers nothing and the scope is where a plain attach would have put it.
    //
    // Scoped to `alpha` and stepping forward, so the answer (`beta`) differs from BOTH the scope
    // itself and the session the other two clients are on — a fallback to the wrong thing could not
    // produce it.
    let mut fresh =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("a fresh client connects");
    fresh
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "fresh" }))
        .expect("client/hello is accepted");
    fresh.scope_to("alpha".to_owned());
    assert_eq!(
        step(&mut fresh, sprag_terminal::OrderStep::Next),
        json!("beta"),
        "a client with no attachment steps from the session its requests are scoped to",
    );

    let _ = std::fs::remove_file(&sock);
}

/// A client can re-attach to WHERE IT ALREADY IS without naming it, and is told what that session
/// is called now — the `{"attached": true}` attach, over the socket, across a rename.
///
/// R303 registered this spelling as "accepted and means nothing": it resolved to the client's own
/// attachment and re-attached it to itself. It has a meaning now, and this is it — the client that
/// has to RESUME (a gesture stopped its poll thread and then found nowhere to go) must start
/// serving again over the session it never left, and naming that session is precisely the mistake
/// this round exists to remove. A rename in the instant before would make a named resume fail and
/// take the client down with it.
///
/// The rename is what makes the fixture discriminate: `work` no longer resolves, so a resume that
/// went by the name the client last saw is refused here, and one that goes by the attachment is
/// answered `renamed`.
#[test]
fn a_client_can_resume_where_it_already_is_and_is_told_the_current_name() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");

    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "resumer" }))
        .expect("client/hello is accepted");
    viewer.scope_to("work".to_owned());
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");
    viewer.scope_to_attached();

    admin
        .call(
            "scene/invoke",
            json!({
                "session": "work",
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": "renamed" },
            }),
        )
        .expect("rename_session answers");

    assert_eq!(
        viewer
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach is accepted"),
        json!("renamed"),
        "an attach scoped to the attachment resumes where the client is and names it",
    );
    assert_eq!(
        attached_of(&mut admin, "renamed"),
        1,
        "and it did not move: one viewer, on the session it never left",
    );

    // The CONTROL that makes the claim mean something: the NAME it attached with is refused now.
    let mut by_name =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("a name-scoped client connects");
    by_name
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "by-name" }))
        .expect("client/hello is accepted");
    by_name.scope_to("work".to_owned());
    assert!(
        by_name.call(CLIENT_ATTACH_METHOD, json!({})).is_err(),
        "a resume that went by the remembered name would be refused outright",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Every way an attach can name a target this daemon does not admit, as the SENTENCE an operator
/// reads — over the socket, not through the grammar's own unit tests.
///
/// R303 registered exactly this gap for the scope's three new refusals: the wording was pinned by a
/// unit test on `Display` and the DELIVERY of it to a reader was pinned by nothing. These are that
/// item's lesson applied on the round that would otherwise repeat it.
///
/// The CONTROL is the last case: a well-typed `false` is not a fault, so a daemon that refused
/// everything it did not understand would fail here rather than pass this test whole.
#[test]
fn a_malformed_attach_target_is_refused_with_the_sentence_that_says_which() {
    let (_host, sock) = spawn_host();
    let mut client = HostConn::connect(&sock, Duration::from_secs(5)).expect("the client connects");
    client
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "malformed" }))
        .expect("client/hello is accepted");

    for (params, sentence) in [
        (json!({ "last": 1 }), "params.last must be a boolean"),
        (json!({ "last": null }), "params.last must be a boolean"),
        (
            json!({ "last": true, "unattached": "yes" }),
            "params.unattached must be a boolean",
        ),
        (
            json!({ "unattached": true }),
            "params.unattached narrows params.last, which this request does not ask for",
        ),
    ] {
        let refusal = client
            .call(CLIENT_ATTACH_METHOD, params.clone())
            .expect_err("a malformed target is refused");
        assert!(
            refusal.to_string().contains(sentence),
            "{params} must be refused as {sentence:?}, not as {refusal}",
        );
    }

    assert_eq!(
        client
            .call(CLIENT_ATTACH_METHOD, json!({ "last": false }))
            .expect("the CONTROL: a well-typed no is an absent key"),
        json!("0"),
        "and it attaches to the connection's scope, which is the daemon's default session",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The window RING walked over the REAL socket — `select_window {relative}`, the verb behind
/// `prefix n` / `prefix p` and `sprag select-window -n|-p`.
///
/// The walk is the DAEMON's, so this is where it is judged: a client that resolved the step from
/// its own window mirror would be a second answer to this question, and the mirror can be a
/// revision behind. Each step is asserted through the `windows` slot rather than only through the
/// answer, so an action that ANSWERED a name without moving the session fails.
#[test]
fn the_window_ring_is_walked_by_the_daemon_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    for _ in 0..2 {
        conn.call(
            "scene/invoke",
            json!({ "session": "0", "path": mux_action_path(NEW_WINDOW_ACTION), "args": {} }),
        )
        .expect("new_window answers");
    }
    // Three windows — "0", "1", "2" — and `new_window` selected the last.
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![
            ("0".to_owned(), false),
            ("1".to_owned(), false),
            ("2".to_owned(), true),
        ],
    );

    let step = |conn: &mut HostConn, relative: &str| -> Value {
        conn.call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(SELECT_WINDOW_ACTION),
                "args": { "relative": relative },
            }),
        )
        .expect("select_window answers")
    };

    // Forward from the last WRAPS onto the first — what makes a window list a ring rather than a
    // row, and the half a clamping walk would get wrong while looking right in the middle.
    assert_eq!(step(&mut conn, "next"), json!("0"));
    assert_eq!(
        windows_in(&mut conn, "0").into_iter().find(|w| w.1),
        Some(("0".to_owned(), true)),
        "the answer names the window the SESSION is on, not just a string",
    );
    assert_eq!(step(&mut conn, "next"), json!("1"));
    // ...and backward from the first wraps onto the last.
    assert_eq!(step(&mut conn, "previous"), json!("0"));
    assert_eq!(step(&mut conn, "previous"), json!("2"));
    assert_eq!(
        windows_in(&mut conn, "0").into_iter().find(|w| w.1),
        Some(("2".to_owned(), true)),
    );

    // The NAMED arm answers the same shape — one verb, one answer, whichever way it was asked.
    assert_eq!(
        conn.call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(SELECT_WINDOW_ACTION),
                "args": { "window": "1" },
            }),
        )
        .expect("select_window answers"),
        json!("1"),
    );

    // Every reading this grammar does not admit is refused, and the CONTROL above is that the two
    // well-formed ones are not: a name AND a step, neither, and a step that is not a word this
    // vocabulary has.
    for bad in [
        json!({ "window": "1", "relative": "next" }),
        json!({}),
        json!({ "relative": "sideways" }),
        json!({ "relative": 1 }),
    ] {
        assert!(
            conn.call(
                "scene/invoke",
                json!({
                    "session": "0",
                    "path": mux_action_path(SELECT_WINDOW_ACTION),
                    "args": bad,
                }),
            )
            .is_err(),
            "{bad} names no window this grammar admits, so it must be refused",
        );
    }
    // ...and the refusals moved nothing.
    assert_eq!(
        windows_in(&mut conn, "0").into_iter().find(|w| w.1),
        Some(("1".to_owned(), true)),
    );

    let _ = std::fs::remove_file(&sock);
}

/// `move_window`'s ANSWER BYTES over a real socket — the shape a client parses, which the CLI's
/// sentence test and the registry's order tests both leave unchecked (R309's finding on `close`,
/// applied on the round that could have repeated it).
///
/// The CONTROL is built into the fixture: the FIRST assertion moves a window that is not the
/// scope's current one and reads `{window: "2", how: "moved"}`, so a daemon that always answered
/// about the current window, or always answered `moved`, fails here while passing every test that
/// only reads the order back.
#[test]
fn the_move_answers_which_window_and_how_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    for _ in 0..2 {
        conn.call(
            "scene/invoke",
            json!({ "session": "0", "path": mux_action_path(NEW_WINDOW_ACTION), "args": {} }),
        )
        .expect("new_window answers");
    }
    // "0", "1", "2" — and `new_window` selected the last, so "2" IS current. Put the scope back on
    // "0", which is what makes the omitted-window arm below discriminate.
    select_window(&mut conn, "0", "0");

    let mv = |conn: &mut HostConn, args: Value| -> Value {
        conn.call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(MOVE_WINDOW_ACTION),
                "args": args,
            }),
        )
        .expect("move_window answers")
    };

    assert_eq!(
        mv(&mut conn, json!({ "window": "2", "place": "first" })),
        json!({ "window": "2", "how": "moved" }),
        "a NAMED move answers the window it was given and the outcome word",
    );
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![
            ("2".to_owned(), false),
            ("0".to_owned(), true),
            ("1".to_owned(), false),
        ],
    );
    assert_eq!(
        mv(&mut conn, json!({ "place": "last" })),
        json!({ "window": "0", "how": "moved" }),
        "an OMITTED window resolves to the scope's current one and is NAMED BACK",
    );
    assert_eq!(
        mv(&mut conn, json!({ "place": "last" })),
        json!({ "window": "0", "how": "already_there" }),
        "and the same request again says which nothing happened",
    );
    assert_eq!(
        mv(&mut conn, json!({ "before": "0" })),
        json!({ "window": "0", "how": "itself" }),
    );
    assert_eq!(
        mv(&mut conn, json!({ "after": "2" })),
        json!({ "window": "0", "how": "moved" }),
        "the anchored arm crosses the wire under its own key",
    );
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![
            ("2".to_owned(), false),
            ("0".to_owned(), true),
            ("1".to_owned(), false),
        ],
    );

    // Every reading this grammar does not admit is refused, with the two well-formed arms above as
    // the control that the refusals are about the SHAPE and not about the verb.
    for bad in [
        json!({ "place": "first", "before": "1" }),
        json!({}),
        json!({ "place": "sideways" }),
        json!({ "place": 1 }),
        json!({ "before": 1 }),
    ] {
        assert!(
            conn.call(
                "scene/invoke",
                json!({
                    "session": "0",
                    "path": mux_action_path(MOVE_WINDOW_ACTION),
                    "args": bad,
                }),
            )
            .is_err(),
            "{bad} names no place this grammar admits, so it must be refused",
        );
    }
    // A window and an anchor that do NOT exist are refused too — not answered with an outcome.
    for bad in [
        json!({ "window": "nosuch", "place": "first" }),
        json!({ "before": "nosuch" }),
    ] {
        assert!(
            conn.call(
                "scene/invoke",
                json!({
                    "session": "0",
                    "path": mux_action_path(MOVE_WINDOW_ACTION),
                    "args": bad,
                }),
            )
            .is_err(),
            "{bad} names a window that does not exist",
        );
    }
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![
            ("2".to_owned(), false),
            ("0".to_owned(), true),
            ("1".to_owned(), false),
        ],
        "and not one refusal moved anything",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Two WINDOWS in ONE session are independent, over a REAL socket — the tmux "windows" shape.
///
/// A `new_window` is born with its own shell and BECOMES current, so the session's reads answer
/// about it; selecting a window re-scopes what those reads see; each window's pane set is its
/// own. As with the two-sessions test, every claim is paired with its complement, so a daemon
/// that merged windows or ignored the selection fails the second half of each pair.
#[test]
fn two_windows_in_one_session_are_independent_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // The default session boots with one window "0" holding its boot `cat` pane (id 0).
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![("0".to_owned(), true)],
        "one boot window, and it is current",
    );
    assert_eq!(pane_ids_in(&mut conn, "0"), vec![0], "the boot pane");

    // Create a second window: born with a shell and SELECTED, so the session's reads now answer
    // about it — and its birth pane's id is fresh (the ONE global counter), never window 0's.
    let created = conn
        .call(
            "scene/invoke",
            json!({ "session": "0", "path": mux_action_path(NEW_WINDOW_ACTION), "args": {} }),
        )
        .expect("new_window answers");
    assert_eq!(created, "1", "the lowest free window name");
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![("0".to_owned(), false), ("1".to_owned(), true)],
        "two windows, the new one current",
    );
    let win1 = pane_ids_in(&mut conn, "0");
    assert_eq!(
        win1.len(),
        1,
        "the new window is born with exactly its shell"
    );
    let birth = win1[0];
    assert_ne!(birth, 0, "a fresh global id, not window 0's boot pane");

    // Select back to window "0": the session's reads answer about window 0 again — its boot pane,
    // and NOT window 1's birth pane.
    select_window(&mut conn, "0", "0");
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![("0".to_owned(), true), ("1".to_owned(), false)],
        "the selection moved back",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "window 0's boot pane — and not window 1's birth pane",
    );

    // A pane spawned NOW lands in the current window (0), leaving window 1's set alone.
    let w0_extra = spawn_in(&mut conn, "0");
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0, w0_extra],
        "the spawn grew the current window",
    );
    select_window(&mut conn, "0", "1");
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![birth],
        "...and window 1 is untouched by the spawn into window 0 — the two do not merge",
    );

    let _ = std::fs::remove_file(&sock);
}

/// `break-pane` and `join-pane` MOVE a pane between windows over the REAL socket — the tmux
/// pane-migration shape. The pane keeps its id across the move (relocated, not re-spawned), break
/// births a new SELECTED window, and a join that empties the source CLOSES it. Every claim is
/// paired with its complement (the source loses exactly what the destination gains), so a daemon
/// that dropped, duplicated, or mis-scoped a pane fails a half.
#[test]
fn break_and_join_move_a_pane_between_windows_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // Window "0" boots with pane 0; add a second pane so window 0 has one to break out.
    let extra = spawn_in(&mut conn, "0");
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0, extra],
        "two panes in window 0"
    );

    // break-pane: move `extra` out into a NEW window, born current, KEEPING its id.
    let broke = conn
        .call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(BREAK_PANE_ACTION),
                "args": { "pane": extra },
            }),
        )
        .expect("break_pane answers")
        .as_str()
        .expect("break_pane returns the new window's name")
        .to_owned();
    assert_eq!(broke, "1", "the new window's lowest free name");
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![("0".to_owned(), false), ("1".to_owned(), true)],
        "two windows now, the broken-out one current",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![extra],
        "the new (current) window holds the moved pane — same id, not a re-spawn",
    );
    select_window(&mut conn, "0", "0");
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![0],
        "the source window kept only its boot pane",
    );

    // join-pane: move pane 0 into window "1" — the source ("0", derived from the pane id) empties
    // and CLOSES. The wire names only the destination.
    let joined = conn
        .call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(JOIN_PANE_ACTION),
                "args": { "pane": 0, "window": "1" },
            }),
        )
        .expect("join_pane answers");
    assert_eq!(
        joined["closed_source"].as_bool(),
        Some(true),
        "the emptied source window was closed",
    );
    assert_eq!(
        windows_in(&mut conn, "0"),
        vec![("1".to_owned(), true)],
        "only the survivor window remains, and it is current",
    );
    assert_eq!(
        pane_ids_in(&mut conn, "0"),
        vec![extra, 0],
        "both panes now live in window 1 (the joined one appended)",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Bound d over the REAL socket: a `set_layout` whose `expected_window` names a window that is
/// NOT the session's current one is REFUSED — the host does not mis-apply it to whatever is
/// current. This is the end-to-end wire proof of the guard the GUI's `WireHost` now invokes (it
/// sends the window its gesture was drawn on). Revert-proof: with the guard removed, the mistagged
/// write below applies (it names the current window's own panes) and the 0.5 assertion fails.
#[test]
fn a_layout_write_tagged_with_the_wrong_window_is_refused_over_the_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // The current window (boot "0") gets a second pane, so it has a two-pane even split to author
    // a gesture against, at a known revision.
    let w0b = spawn_in(&mut conn, "0");
    let (rev, layout) = read_layout_in(&mut conn, "0");
    let ids = layout.panes();
    assert_eq!(ids.len(), 2, "window 0 has two panes: {ids:?}");
    assert_eq!(
        ids,
        vec![sprag_terminal::PaneId(0), sprag_terminal::PaneId(w0b)]
    );

    // A gesture for THIS window's OWN panes, but TAGGED with a window name that is not current, is
    // refused — the even split stands, NOT the 0.75 the gesture asked for.
    let refused = write_layout_tagged(&mut conn, "0", rev, "not-the-current-window", w0b, 0);
    assert!(
        (split_ratio(&refused) - 0.5).abs() < f32::EPSILON,
        "a mistagged write was refused; window 0 kept its even split ({})",
        split_ratio(&refused),
    );

    // CONTROL: the SAME gesture tagged with the ACTUAL current window ("0") applies.
    let (rev2, _) = read_layout_in(&mut conn, "0");
    let applied = write_layout_tagged(&mut conn, "0", rev2, "0", w0b, 0);
    assert!(
        (split_ratio(&applied) - 0.75).abs() < f32::EPSILON,
        "tagged with the current window, the gesture applied ({})",
        split_ratio(&applied),
    );

    let _ = std::fs::remove_file(&sock);
}

/// Send a `set_layout` (a vertical split of `first | second` at 0.75) tagged with `expected_window`
/// and `expected_revision`, over the session named `session`; deserialise + install the answer.
///
/// Deliberately spelled the LEGACY nested way. R264 flattened what a client serialises and kept the
/// nested spelling readable so a snapshot written by an older build still restores; this is that
/// path's only wire-level coverage, and it is here by choice rather than because nobody updated it.
/// The flat spelling a current client sends is exercised by
/// [`an_arrangement_far_deeper_than_the_old_ceiling_crosses_the_socket`].
fn write_layout_tagged(
    conn: &mut HostConn,
    session: &str,
    expected_revision: u64,
    expected_window: &str,
    first: u64,
    second: u64,
) -> sprag_terminal::LayoutTree {
    let value = conn
        .call(
            "scene/invoke",
            json!({
                "session": session,
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": {
                    "expected_revision": expected_revision,
                    "expected_window": expected_window,
                    "tree": { "root": { "split": {
                        "dir": "vertical", "ratio": 0.75,
                        "first": { "leaf": first }, "second": { "leaf": second },
                    } } },
                },
            }),
        )
        .expect("a set_layout write answers");
    let snapshot: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the write answers with a snapshot");
    let mut tree = sprag_terminal::LayoutTree::new();
    tree.set_from_wire(snapshot.tree)
        .expect("a served arrangement is well-formed");
    tree
}

/// The root split's ratio, panicking if the arrangement is not a split.
fn split_ratio(tree: &sprag_terminal::LayoutTree) -> f32 {
    match tree.root() {
        Some(sprag_terminal::LayoutNode::Split { ratio, .. }) => *ratio,
        other => panic!("expected a split at the root, got {other:?}"),
    }
}

/// Read the current arrangement off the wire, exactly as a display client does: query the
/// mux `layout` slot, deserialise the snapshot, and install its tree — which VALIDATES what
/// the host sent and yields definite divider ids to key per-split state on.
fn read_layout(conn: &mut HostConn) -> (u64, sprag_terminal::LayoutTree) {
    let value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(LAYOUT_SLOT) }),
        )
        .expect("the layout query answers");
    let snapshot: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the layout deserialises off the wire");
    let mut tree = sprag_terminal::LayoutTree::new();
    tree.set_from_wire(snapshot.tree)
        .expect("a served arrangement is well-formed");
    (snapshot.revision, tree)
}

/// [`read_layout`] scoped to the session named `session` — the arrangement of THAT session's
/// current window.
fn read_layout_in(conn: &mut HostConn, session: &str) -> (u64, sprag_terminal::LayoutTree) {
    let value = conn
        .call(
            "scene/query",
            json!({ "session": session, "path": mux_action_path(LAYOUT_SLOT) }),
        )
        .expect("the scoped layout query answers");
    let snapshot: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the layout deserialises off the wire");
    let mut tree = sprag_terminal::LayoutTree::new();
    tree.set_from_wire(snapshot.tree)
        .expect("a served arrangement is well-formed");
    (snapshot.revision, tree)
}

/// A DEEP arrangement crosses the real socket — the boundary R264 moved, gated where it bit.
///
/// R264 removed a ceiling nobody designed: a window's arrangement used to serialise as a nested
/// chain, so its JSON depth tracked its pane count and a session of more than 62 panes could not be
/// read by any client. That was proved at the unit level (the parse) and confirmed by hand with an
/// example that spawns real panes. Neither is a gate: the unit test cannot see the daemon, and a
/// suite does not spawn sixty PTYs on every commit.
///
/// It does not have to. The subject is what the WIRE carries, not what a pool holds, so this writes
/// an arrangement of two hundred leaves — nested, that is four hundred levels deep, three times past
/// the limit that used to bite — over a real socket to a real daemon holding TWO panes. The
/// synthetic leaves are dropped by the reconcile against the live pool, which is the honest outcome
/// and is asserted: what is proved is that the daemon PARSED, VALIDATED and APPLIED an arrangement
/// no client could have sent before.
#[test]
fn an_arrangement_far_deeper_than_the_old_ceiling_crosses_the_socket() {
    use sprag_terminal::{LayoutNodeWire, LayoutWire, SplitDir};

    /// Leaves in the written arrangement. Nested, this is `2 * 200 + 2` levels of JSON.
    const LEAVES: u64 = 200;

    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
    )
    .expect("spawn a second pane");
    let (revision, layout) = read_layout(&mut conn);
    let live = layout.panes();
    assert_eq!(live.len(), 2, "the daemon holds two panes, not two hundred");

    // The two live panes sit deepest, so a reconcile that keeps them has to have walked the
    // whole chain. Every id above them is synthetic and exists only to make the tree deep.
    let mut root = LayoutNodeWire::Split {
        id: None,
        dir: SplitDir::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNodeWire::Leaf(live[0])),
        second: Box::new(LayoutNodeWire::Leaf(live[1])),
    };
    let synthetic = live.iter().map(|pane| pane.0).max().unwrap_or(0) + 1;
    for step in 0..LEAVES - 2 {
        root = LayoutNodeWire::Split {
            id: None,
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNodeWire::Leaf(sprag_terminal::PaneId(
                synthetic + step,
            ))),
            second: Box::new(root),
        };
    }
    let deep = serde_json::to_value(LayoutWire { root: Some(root) })
        .expect("a client serialises its arrangement");

    let value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": { "expected_revision": revision, "tree": deep },
            }),
        )
        .expect("a two-hundred-leaf arrangement reaches the daemon and is answered");
    let answer: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the write answers with a snapshot a client can read");
    assert!(
        answer.revision > revision,
        "the deep write was APPLIED, not merely parsed and discarded",
    );

    let (_, served) = read_layout(&mut conn);
    assert_eq!(
        served.panes(),
        live,
        "the reconcile kept the live panes and dropped the synthetic ones",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The WRITE half over a REAL socket: a client's settled arrangement — a divider it minted
/// itself, dragged off-centre — reaches the host, comes back NAMED, and is what the host
/// then serves. This is the claim the whole detach/reattach arc rests on: the user's layout
/// is session state, not something the client is merely holding.
#[test]
fn a_clients_settled_arrangement_crosses_the_real_socket_and_is_named() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
    )
    .expect("spawn a second pane");
    let (revision, layout) = read_layout(&mut conn);
    let panes = layout.panes();
    assert_eq!(panes.len(), 2);

    // The client drops the two panes into a VERTICAL split at a ratio the user dragged,
    // through a divider of its own minting (`id` omitted — it has no authority to name one).
    let value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": { "expected_revision": revision, "tree": { "root": { "split": {
                    "dir": "vertical",
                    "ratio": 0.75,
                    "first": { "leaf": panes[0].0 },
                    "second": { "leaf": panes[1].0 },
                } } } },
            }),
        )
        .expect("the arrangement write answers");
    let answer: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the write answers with a snapshot");
    assert!(answer.revision > revision, "the write moved the revision");

    // The host NAMED the client's divider, and a fresh read serves the same arrangement —
    // the user's intent is now the session's, not the client's.
    let (served, layout) = read_layout(&mut conn);
    assert_eq!(
        served, answer.revision,
        "the write's answer IS what is served"
    );
    let Some(sprag_terminal::LayoutNode::Split { id, dir, ratio, .. }) = layout.root() else {
        panic!("the written split survived, got {:?}", layout.root());
    };
    assert_eq!(*dir, sprag_terminal::SplitDir::Vertical, "direction stuck");
    assert!((*ratio - 0.75).abs() < f32::EPSILON, "the ratio stuck");
    assert_eq!(
        answer.tree,
        sprag_terminal::LayoutWire::from(&layout),
        "the answer is the canonical tree, with the divider named {id:?}",
    );

    // A pane floated OUT of the tiling loses its leaf host-side, so the client's tree stays
    // an exact projection with no filter of its own.
    let value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SET_FLOATING_ACTION),
                "args": { "id": panes[1].0, "floating": true },
            }),
        )
        .expect("the float write answers");
    let floated: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the float answers with a snapshot");
    assert_eq!(
        floated.tree.root,
        Some(sprag_terminal::LayoutNodeWire::Leaf(panes[0])),
        "the floated pane's leaf collapsed; its sibling reclaimed the space",
    );
    // ...and WHICH pane is floated crosses the wire too. Without it a reattaching client
    // would see a pane that is neither tiled nor floated, and simply not draw it.
    assert_eq!(floated.floating, vec![panes[1]], "the float set is served");
    let (_, _) = read_layout(&mut conn);

    // ...and the host REJECTS an arrangement that would corrupt the session, keeping the
    // one in force rather than absorbing it.
    let value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": { "expected_revision": floated.revision, "tree": { "root": { "split": {
                    "dir": "horizontal",
                    "ratio": 4.2, // not a share
                    "first": { "leaf": panes[0].0 },
                    "second": { "leaf": panes[0].0 }, // and the same pane twice
                } } } },
            }),
        )
        .expect("a rejected write still answers");
    let kept: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the rejection answers with a snapshot");
    assert_eq!(
        kept.tree, floated.tree,
        "a rejected write left the session's arrangement exactly as it was",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Drag-to-upload, end to end against a real host process: a file dropped on a REMOTE workspace pane
/// is `scp`-uploaded and the pane is handed the file's REMOTE path.
///
/// The three links this pins, none of which any unit test can reach together:
/// 1. `drop_file` reaches the daemon's action dispatch and answers the planned remote path;
/// 2. the host actually EXECS an upload — with the argv [`sprag_host::SshTarget::scp_argv`] builds,
///    which the stand-in `scp` records verbatim;
/// 3. the remote path is pasted into the pane only AFTER the transfer succeeds (the upload runs on a
///    background thread, so this is the ordering that could regress silently).
///
/// The pane is a `cat` MARKED remote (`spawn {cmd, remote}`) rather than a real `ssh`: the drop
/// policy keys off the pane's recorded endpoint, and `cat` echoes what is pasted into it, which is
/// how the paste becomes observable. The file name carries a SPACE, so the answer also proves the
/// shell quoting survives the wire (`~/'drop me.txt'`, tilde outside the quotes).
#[test]
fn a_dropped_file_on_a_remote_pane_uploads_and_pastes_the_remote_path() {
    let fixture = DropFixture::new("upload", 0);
    let dropped = fixture.dropped.clone();
    let argv_file = fixture.argv_file.clone();
    let (_host, sock) = spawn_host_with(&["cat"], &[("PATH", &fixture.path_env())]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    // A pane MARKED as a remote workspace — the same `{cmd, remote}` birth spec `sprag ssh` sends.
    let pane = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["cat"], "remote": { "host": "server", "user": "me" } },
            }),
        )
        .expect("spawn a remote-marked pane")
        .as_u64()
        .expect("spawn returns the new pane id");

    let answer = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(DROP_FILE_ACTION),
                "args": { "pane": pane, "path": dropped.to_str().unwrap() },
            }),
        )
        .expect("drop_file answers");
    assert_eq!(
        answer["path"].as_str(),
        Some("~/'drop me.txt'"),
        "the pane is promised the REMOTE path, tilde-expanded and name-quoted: {answer}",
    );

    // The upload runs on a background thread, so the paste is what proves it finished.
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(pane, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.contains("~/'drop me.txt'")))
            .unwrap_or(false)
        }),
        "the remote path never reached the pane after a successful upload",
    );

    let recorded = std::fs::read_to_string(&argv_file).expect("the stand-in scp ran and recorded");
    let argv: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        argv,
        vec!["-B", "--", dropped.to_str().unwrap(), "me@server:"],
        "the host exec'd the batch-mode upload to the remote HOME with the local file verbatim",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The other half of the upload contract: a FAILED transfer leaves the pane untouched.
///
/// Without this, "paste the remote path" and "paste it only if the file got there" are
/// indistinguishable — every assertion in the success test passes just as well for an unconditional
/// paste. Here the stand-in `scp` exits non-zero, so a pane that receives the path is being told a
/// file is on the remote when it is not.
///
/// The negative is bounded, not merely awaited: the recorded argv proves the upload RAN and finished
/// before the window in which the paste is checked for, so this is "the paste did not happen after
/// the failure", not "the paste had not happened yet".
#[test]
fn a_failed_upload_leaves_the_pane_untouched() {
    let fixture = DropFixture::new("failed", 1);
    let dropped = fixture.dropped.clone();
    let argv_file = fixture.argv_file.clone();
    let (_host, sock) = spawn_host_with(&["cat"], &[("PATH", &fixture.path_env())]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    let pane = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["cat"], "remote": { "host": "server" } },
            }),
        )
        .expect("spawn a remote-marked pane")
        .as_u64()
        .expect("spawn returns the new pane id");

    let answer = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(DROP_FILE_ACTION),
                "args": { "pane": pane, "path": dropped.to_str().unwrap() },
            }),
        )
        .expect("drop_file answers");
    assert_eq!(
        answer["path"].as_str(),
        Some("~/'drop me.txt'"),
        "the request itself was valid — only the transfer fails, and it fails LATER: {answer}",
    );

    assert!(
        wait_until(Duration::from_secs(5), || argv_file.exists()),
        "the stand-in scp never ran",
    );
    assert!(
        !wait_until(Duration::from_secs(1), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(pane, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v| v.as_str().map(|s| s.contains("~/'drop me.txt'")))
            .unwrap_or(false)
        }),
        "a FAILED upload must not paste a remote path for a file that never landed",
    );

    let _ = std::fs::remove_file(&sock);
}

/// The stand-in `scp` + the file to drop, for the drag-to-upload tests.
///
/// The stub goes on the DAEMON's `PATH` (the daemon is what spawns the upload — stubbing the test
/// process's own PATH would prove nothing), records the argv it was exec'd with, and exits with the
/// requested code so both the success and the failure arm are drivable. Cleans up on drop, including
/// on a panic, so a failed assertion leaks no temp tree.
struct DropFixture {
    dir: PathBuf,
    argv_file: PathBuf,
    dropped: PathBuf,
}

impl DropFixture {
    fn new(label: &str, scp_exit: i32) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("sprag-drop-it-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the stand-in scp dir");
        let argv_file = dir.join("argv.txt");
        let scp = dir.join("scp");
        std::fs::write(
            &scp,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit {scp_exit}\n",
                argv_file.display()
            ),
        )
        .expect("write the stand-in scp");
        std::fs::set_permissions(&scp, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");

        // A REAL file: the host canonicalizes the drop before delivering it. The space in the name
        // is load-bearing — it is what makes the answer prove the shell quoting.
        let dropped = dir.join("drop me.txt");
        std::fs::write(&dropped, b"payload").expect("write the file to drop");
        let dropped = std::fs::canonicalize(&dropped).expect("canonicalize the dropped file");
        Self {
            dir,
            argv_file,
            dropped,
        }
    }

    /// A `PATH` with the stand-in first, for the daemon's environment.
    fn path_env(&self) -> String {
        format!(
            "{}:{}",
            self.dir.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }
}

impl Drop for DropFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A pane's PROJECT reaches a wire client: the daemon walks up from that pane's LIVE cwd, parses the
/// `.sprag.toml` it finds, and serves the declared commands on the mux `project.<pane>` slot.
///
/// The daemon's `HOME` is pointed at the temporary project, because a birth pane with no explicit
/// cwd starts in the home directory (portable-pty's default) — so this puts the boot pane INSIDE the
/// project without driving a `cd` through its shell, which would be a race to wait on.
///
/// REVERT-PROOF: drop the `project.<pane>` arm from the mux `query` and the slot answers
/// `UnknownPath` instead of the actions; make `project_value` ignore the pane's remote flag and the
/// remote case below stops being `null`.
#[test]
fn a_panes_project_commands_reach_a_wire_client() {
    let project = std::env::temp_dir().join(format!("sprag-wire-project-{}", std::process::id()));
    std::fs::create_dir_all(project.join("sub")).expect("create the temp project");
    std::fs::write(
        project.join(sprag_host::PROJECT_FILE),
        "[[command]]\nname = \"test\"\ntitle = \"Run the suite\"\nrun = [\"cargo\", \"test\"]\n",
    )
    .expect("write the project config");

    let (_host, sock) = spawn_host_with(&["cat"], &[("HOME", &project.display().to_string())]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term");

    let answer = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&project_slot_for(0)) }),
        )
        .expect("project query");

    assert_eq!(
        answer["root"].as_str(),
        Some(project.to_str().expect("utf-8 temp path")),
        "the root is the directory holding the config: {answer}"
    );
    assert_eq!(
        answer["actions"][0]["name"].as_str(),
        Some("test"),
        "the declared command's name reaches the client: {answer}"
    );
    assert_eq!(
        answer["actions"][0]["title"].as_str(),
        Some("Run the suite"),
        "...and its title: {answer}"
    );
    assert_eq!(
        answer["actions"][0]["run"],
        json!(["cargo", "test"]),
        "...and the ARGV it would run, so a client can show it before running it: {answer}"
    );

    // A pane that does not exist is `null`, not an error — the same "no project here" answer a pane
    // outside any project gets, because neither is a fault to report.
    let absent = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&project_slot_for(9999)) }),
        )
        .expect("a query for an absent pane still answers");
    assert!(absent.is_null(), "an unknown pane has no project: {absent}");

    std::fs::remove_dir_all(&project).ok();
}

/// A project whose config is BROKEN is reported as an error, never as an empty command list — the
/// author of a committed config needs to hear about their typo.
///
/// REVERT-PROOF: make `project_value` answer `Null` for a parse failure and this fails.
#[test]
fn a_broken_project_config_is_reported_rather_than_read_as_empty() {
    let project = std::env::temp_dir().join(format!("sprag-wire-badproj-{}", std::process::id()));
    std::fs::create_dir_all(&project).expect("create the temp project");
    std::fs::write(
        project.join(sprag_host::PROJECT_FILE),
        "[[command]]\nname = \"test\"\nrun = [\n",
    )
    .expect("write a broken config");

    let (_host, sock) = spawn_host_with(&["cat"], &[("HOME", &project.display().to_string())]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term");
    let answer = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&project_slot_for(0)) }),
        )
        .expect("project query");

    let error = answer["error"]
        .as_str()
        .unwrap_or_else(|| panic!("a broken config reports an error: {answer}"));
    assert!(
        error.contains(sprag_host::PROJECT_FILE),
        "the report names the file: {error}"
    );
    assert!(
        answer.get("actions").is_none(),
        "and offers no actions to run: {answer}"
    );

    std::fs::remove_dir_all(&project).ok();
}

/// H3's D9, against a REAL `sprag-term`: a verdict resting on an ABSENCE confirms itself with no
/// client activity and no pane output.
///
/// # Why this is the gate the slice exists to earn
///
/// The pane list evaluates a pane when a client asks, and a client asks when the session's scene
/// revision moves — which pane OUTPUT advances. That drives `Blocked` and `Working` for free, because
/// the output that paints a dialog is the same event that wakes the reader. It does not drive `Idle`:
/// that verdict has to hold for the settle window, and the last thing to move the revision was **the
/// output that stopped**. Without the settle waker this pane sits at its previous state until
/// something unrelated wakes a client.
///
/// So the observation cannot be "read the pane list after two seconds and see `idle`" — a read drives
/// an evaluation, so that assertion passes with no waker at all. What distinguishes them is the
/// REVISION: only the waker can advance it here, because the boot pane paints once and then holds its
/// pty open with `cat`. Parking on `scene/waitFor` and being woken is therefore proof that the daemon
/// acted on its own clock.
///
/// **What this test alone does NOT prove, measured rather than assumed.** A waker that bumped the
/// session WITHOUT observing also passes everything above: the wake arrives, the woken client
/// re-queries, and that query publishes. So this test pins "the daemon wakes a reader on its own
/// clock"; `one_query_on_a_never_queried_daemon_answers_a_settled_verdict` is what pins "the daemon
/// CONFIRMS the verdict itself", because there no client ever asks. The pair was found by running that
/// mutation and reading which test died — the docstring said more than the assertions did.
///
/// The tail closes the other half of that mutation: a waker that bumps without publishing leaves the
/// pane due forever and bumps again every sweep, which is the R152 livelock shape. So the revision is
/// read again after two sweep intervals and must not have moved.
///
/// The wait runs on its own thread so a daemon that never publishes fails with this message instead of
/// hanging until CI's timeout — a hang reports nothing about which claim broke.
#[test]
fn an_idle_agent_pane_settles_with_no_client_activity_and_no_output() {
    // A `claude` pane at REST: the resting glyph in the title (OSC 2) and the footer its fingerprint
    // reads, painted once — then `cat`, which emits nothing further, so the pane goes quiet exactly as
    // an agent that has finished does. The rules' fidelity to a real agent screen is slice 1's
    // business, proven there against captured screens; this is a screen those rules answer for.
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \
         \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
    ]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // Wait for the paint to land, so the baseline below is taken after the pane's own output has
    // stopped. The condition is the pane's TEXT, not a timer.
    let painted = wait_until(Duration::from_secs(5), || {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(0, FULL_TEXT_SLOT) }),
        )
        .ok()
        .and_then(|v: Value| v.as_str().map(|s| s.contains("? for shortcuts")))
        .unwrap_or(false)
    });
    assert!(painted, "the agent-shaped screen never painted");

    // The first look has started the settle window; nothing is published yet, because a resting
    // verdict has to hold. This assertion is what makes the wake below meaningful — without it the
    // test could not tell "confirmed on the daemon's clock" from "was already published".
    let entry = pane_entry(&mut conn, 0);
    assert!(
        entry.get("agent").is_none(),
        "a resting verdict must not publish on sight: {entry}",
    );

    // Now go quiet. The only thing that can advance this session's revision from here is the waker
    // publishing the settled verdict: the pane is `cat` with nothing to say, and this test sends no
    // input and invokes no action.
    let since = read_revision(&mut conn);
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let mut parked =
            HostConn::connect(&sock, Duration::from_secs(5)).expect("second connection");
        let woken = parked.call("scene/waitFor", json!({ "since": since }));
        let _ = tx.send(woken.map(|v: Value| v["revision"].as_u64().unwrap_or(0)));
    });

    let woken = rx.recv_timeout(Duration::from_secs(15)).expect(
        "the daemon never advanced the revision on its own — nothing confirmed the verdict",
    );
    let revision = woken.expect("waitFor answered an error");
    assert!(
        revision > since,
        "the wake carried a newer revision: {revision} vs {since}",
    );
    waiter.join().expect("the waiter thread");

    // ONE wake per transition, not a tick — and this must be measured BEFORE the pane list is read
    // again, which is the correction a mutation forced. A waker that bumps without publishing leaves
    // the pane due forever and bumps every sweep (the R152 livelock shape), but a pane-list read
    // publishes the verdict itself and so CURES the pane of being due. Asserting stability after such
    // a read therefore proves nothing: the first draft of this check passed under exactly that
    // mutation. `scene/revision` is not a pane-list query and drives no evaluation, so the window
    // below is genuinely untouched. Two sweep intervals, so a per-sweep bump cannot hide inside it.
    let after_settle = read_revision(&mut conn);
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(
        read_revision(&mut conn),
        after_settle,
        "a settled workspace must stop advancing the revision — a waker that woke a client without \
         publishing would still find this pane due and bump again every sweep",
    );

    // Only NOW read the pane list, so the answer the wake was about is reported by the daemon rather
    // than produced by this query.
    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"], "idle",
        "the settled verdict reached the pane list: {entry}",
    );
    assert_eq!(
        entry["agent"]["name"], "claude",
        "and it says whose: {entry}"
    );
}

/// H3 slice 4 against a REAL `sprag-term`: a manifest edited on DISK reaches a pane that is not
/// moving, with no client activity and no pane output.
///
/// # Why this cannot be a unit test, which is the lesson slice 3 paid for
///
/// Every part of the reload is unit-tested one layer down: the file format, the layering, the holder
/// that notices an edit, the ruleset revision in the quiescence key, and `AgentRegistry::stale`. That
/// is exactly the state slice 3's waker was in when the shipped daemon published nothing at all — the
/// COMPOSITION is what fails, and only the binary runs it. The rule R253 wrote down is the one this
/// test exists to honour: when a subsystem's whole purpose is to act without being asked, no test that
/// asks can confirm it.
///
/// The composition here has three joints, and a break in any of them leaves every individual piece
/// green: the waker must re-read the file, it must hand the new list to the registry, and its `ask`
/// must have a reason to look at a pane that is neither due nor unknown. A pane that has painted once
/// and gone quiet has nothing else coming.
///
/// # What makes the observation about the DAEMON
///
/// The same separation the idle test needed. A pane-list read drives an evaluation, so "edit the file,
/// then read the pane list" would pass with no waker reload at all — the reading client would do the
/// work. So this parks on `scene/waitFor` BEFORE the edit and requires the wake: only the daemon can
/// advance this session's revision here, because the boot pane is `cat` with nothing left to say.
#[test]
fn an_edited_manifest_reaches_a_pane_that_is_not_moving() {
    let dir = std::env::temp_dir().join(format!("sprag-wire-agentcfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    let config = dir.join("sprag").join(sprag_host::config::CONFIG_FILE);
    // A file that says nothing about agents: the built-ins are in force, so the pane below settles
    // exactly as it does with no config at all. Written rather than left absent so that the EDIT is a
    // change of content and not a creation — the harder case, and the one a user actually performs.
    std::fs::write(&config, "[options]\n").expect("write the initial config");

    let (_host, sock) = spawn_host_with(
        &[
            "sh",
            "-c",
            "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \
             \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
        ],
        &[("XDG_CONFIG_HOME", &dir.display().to_string())],
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // Settled under the BUILT-IN rules first, so what the edit changes is an answer this daemon has
    // already given rather than one it never reached.
    let settled = wait_until(Duration::from_secs(20), || {
        pane_entry(&mut conn, 0)["agent"]["state"] == "idle"
    });
    assert!(
        settled,
        "the agent-shaped pane never settled under the built-ins"
    );

    // Park BEFORE the edit. From here the pane emits nothing and this test invokes no action, so the
    // only thing that can move the revision is the daemon acting on a file it re-read by itself.
    let since = read_revision(&mut conn);
    let (tx, rx) = std::sync::mpsc::channel();
    let parked_sock = sock.clone();
    let waiter = std::thread::spawn(move || {
        let mut parked =
            HostConn::connect(&parked_sock, Duration::from_secs(5)).expect("second connection");
        let woken = parked.call("scene/waitFor", json!({ "since": since }));
        let _ = tx.send(woken.map(|v: Value| v["revision"].as_u64().unwrap_or(0)));
    });

    // The user corrects a built-in rule: the same screen now reads as WORKING. `working` is evidence
    // a rule SAW, so it publishes on sight — the edit's arrival is not itself delayed by a settle
    // window, and the timeout below is about the sweep alone.
    let edited = Instant::now();
    std::fs::write(
        &config,
        "[options]\n\n[[agent]]\nname = \"claude\"\n\n[[agent.rule]]\nid = \"idle-glyph\"\n\
         state = \"working\"\npriority = 10\nall = [ { region = \"title\", starts_with = \"✳\" } ]\n",
    )
    .expect("edit the config");

    let woken = rx.recv_timeout(Duration::from_secs(30)).expect(
        "the daemon never advanced the revision after the edit — nothing re-read the file, or \
         nothing looked at a pane that was neither due nor unknown",
    );
    // THE LATENCY CONTRACT, asserted rather than described. `AgentManifests`' docs state that an
    // edit takes effect "within one sweep", and `spawn_agent_waker` states WHY that is one and not
    // two: the re-read runs BEFORE the walk, so the panes the edit invalidates are served by the
    // very pass that invalidated them. Both were durable comments with nothing behind them.
    //
    // The bound separates the two mechanisms rather than fitting the measurement. Serving the stale
    // panes on the pass AFTER the reload — the shape the ordering exists to avoid — costs a second
    // interval, so anything under one-and-a-half intervals is the correct ordering and anything at
    // two is the wrong one. Measured 4.997-5.039 s over five runs at a 5 s interval, so the slack
    // here is 2.5 s against an observed overshoot of 40 ms.
    //
    // That tight clustering at the TOP of the range is not luck and is not an average: this test
    // reaches the WORST case by construction, because `settled` above is itself published by the
    // pass that precedes the edit. A 1.3 s sleep inserted before the write moved the reading to
    // 3.698-3.710 s, which is what proves the arrival is the next sweep BOUNDARY rather than a
    // fixed interval — and which is why the assertion below is a real ceiling rather than a
    // description of where this fixture happens to sit.
    let latency = edited.elapsed();
    assert!(
        latency < SWEEP_INTERVAL + SWEEP_INTERVAL / 2,
        "the edit took {latency:?}, which is a second sweep — the re-read is no longer running \
         before the walk it invalidates",
    );
    let revision = woken.expect("waitFor answered an error");
    assert!(
        revision > since,
        "the wake carried a newer revision: {revision} vs {since}",
    );
    waiter.join().expect("the waiter thread");

    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"], "working",
        "the corrected rule is what the pane now reads as: {entry}",
    );
    assert_eq!(
        entry["agent"]["rule"], "idle-glyph",
        "and the verdict still names the rule that fired, which is what `explain` reads: {entry}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A BROKEN `[[agent]]` block is reported by the daemon that refused it, and the report clears when
/// the file is fixed — over the real socket, against the real binary.
///
/// # Why this cannot be a unit test
///
/// Because the interesting transition is the one where NOTHING ELSE MOVES. A broken edit replaces no
/// ruleset — `AgentManifests::refresh` answers `false` and keeps the last list that worked — so every
/// pane keeps its verdict, the revision does not advance, and the only thing in the process that
/// changed is a sentence nobody had a way to read until now. A test that asked the library would be
/// asking the very function whose result the daemon is free to ignore; what has to be proved is that
/// the daemon publishes it OUTSIDE the branch it would be natural to publish it inside.
///
/// # The three readings, and why the middle one is the point
///
/// Boot is clean, so the slot answers `null`: a report that appeared here would mean the daemon
/// reported a file it had accepted. Then the file is BROKEN, and the sentence appears from a sweep
/// this test never asks for — the daemon's own wake. Then it is FIXED, and the sentence goes away,
/// which is the half a "report once and remember it" implementation gets wrong: a user who has
/// corrected their config must stop being told it is broken.
///
/// REVERT-PROOF: move the publish inside `if replaced` in `sprag-term`'s `adopt_manifests` and the
/// middle reading stays `null` forever — the daemon detects with the last good list and says nothing,
/// which is exactly the state this round was opened to end.
///
/// The waits are sweep-length because a sweep is what does the work; they poll the CONDITION rather
/// than sleeping a fixed span, so they cost the sweep and not the timeout.
#[test]
fn a_broken_agent_manifest_is_reported_and_the_report_clears_when_it_is_fixed() {
    let dir = std::env::temp_dir().join(format!("sprag-wire-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    let config = dir.join("sprag").join(sprag_host::config::CONFIG_FILE);
    // A file that declares a USABLE manifest, so the daemon boots holding the user's own list and the
    // edit below is a change to something it accepted — not a first read that happens to fail.
    let good = "[[agent]]\nname = \"claude\"\ndisable = [\"idle-glyph\"]\n";
    std::fs::write(&config, good).expect("write the initial config");

    let (_host, sock) =
        spawn_host_with(&["cat"], &[("XDG_CONFIG_HOME", &dir.display().to_string())]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    assert_eq!(
        manifest_report(&mut conn),
        None,
        "a daemon whose manifests ARE the user's reports nothing"
    );

    // The typo: a `disable` naming a rule that does not exist. Valid TOML, so nothing else in the
    // file stops working — which is what makes this silent.
    std::fs::write(
        &config,
        "[[agent]]\nname = \"claude\"\ndisable = [\"nope\"]\n",
    )
    .expect("break the config");
    let reported = wait_until(Duration::from_secs(30), || {
        manifest_report(&mut conn).is_some()
    });
    assert!(
        reported,
        "the daemon never reported the broken manifests — nothing published outside the reload branch"
    );
    let message = manifest_report(&mut conn).expect("just observed");
    assert!(
        message.contains("nope"),
        "and the report names what is wrong, rendered by the end that read the file: {message}"
    );

    // Fixed. The report has to go: a user who corrected the file must stop being told it is broken.
    std::fs::write(&config, good).expect("fix the config");
    let cleared = wait_until(Duration::from_secs(30), || {
        manifest_report(&mut conn).is_none()
    });
    assert!(cleared, "the report survived the fix: {message}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The daemon's verdict on the user's agent manifests, or `None` when it has none to give.
fn manifest_report(conn: &mut HostConn) -> Option<String> {
    let value: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(AGENT_MANIFESTS_SLOT) }),
        )
        .expect("agent manifests query");
    value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// One pane's entry from the `/sprag_mux` pane list.
fn pane_entry(conn: &mut HostConn, id: u64) -> Value {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    )
    .expect("panes query")
    .as_array()
    .and_then(|panes| panes.iter().find(|p| p["id"].as_u64() == Some(id)).cloned())
    .expect("the pane is listed")
}

/// D8's additive rule over the REAL wire, plus the one-shot reader D9 buys.
///
/// Two claims that both need a live daemon:
///
/// * **A workspace with no agents is byte-identical to the pre-H3 shape.** The unit tests assert the
///   key's absence on a synthetic screen; this asserts it on the wire a client actually parses, where
///   an accidental `"agent": null` would be a shape change rather than an absence.
/// * **A reader that asks ONCE gets the settled answer.** Under a poll-driven site alone, a verdict
///   resting on an absence needs a second observation a window later, so a single-shot caller — `sprag`
///   on the CLI, an MCP call — could never see `Idle` at all. Here the daemon has already confirmed it
///   on its own clock, so ONE query answers, and the connection is a FRESH one that has never driven an
///   evaluation.
#[test]
fn a_shell_pane_carries_no_agent_key_and_one_query_answers_a_settled_one() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // The boot pane is `cat`: a blank screen no manifest claims.
    let shell = pane_entry(&mut conn, 0);
    assert!(
        shell.get("agent").is_none(),
        "a shell pane carries no agent key at all — not a null one: {shell}",
    );
    assert!(
        !shell.to_string().contains("agent"),
        "and the word does not appear anywhere in its entry: {shell}",
    );

    // Spawn an agent-shaped pane beside it, then let the daemon settle it with nobody watching.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(SPAWN_ACTION),
            "args": { "cmd": ["sh", "-c",
                "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \
                 \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat"] },
        }),
    )
    .expect("spawn the agent-shaped pane");

    let settled = wait_until(Duration::from_secs(10), || {
        pane_entry(&mut conn, 1)["agent"]["state"] == "idle"
    });
    assert!(
        settled,
        "the agent pane never settled: {}",
        pane_entry(&mut conn, 1)
    );

    // A FRESH connection asking exactly once. It has driven no evaluation of its own, so anything it
    // sees was confirmed by the daemon.
    let mut fresh =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("a second, naive connection");
    let entry = pane_entry(&mut fresh, 1);
    assert_eq!(
        entry["agent"]["state"], "idle",
        "one query answers a settled verdict: {entry}",
    );
    assert_eq!(
        entry["agent"]["rule"], "idle-glyph",
        "with the rule that said so"
    );

    // And the shell beside it is still bare, so the presence above is not the detector answering for
    // everything.
    let shell = pane_entry(&mut fresh, 0);
    assert!(
        shell.get("agent").is_none(),
        "the shell is still unclaimed: {shell}",
    );
}

/// The one-shot reader contract, measured rather than assumed: a caller that asks EXACTLY ONCE, on a
/// daemon no client has ever queried, gets a settled verdict.
///
/// # Why this test exists in this shape
///
/// The first version of D9 claimed a one-shot reader was served, and a live drive showed it was not. A
/// candidate is created by an OBSERVATION, and until slice 3 the only observer was the pane-list query
/// — so a first-ever read WAS that pane's first observation, and a resting verdict has to hold before
/// it publishes. The answer was `None`, twice, on a daemon that had been up for five seconds. That is
/// false for exactly the caller slice 5 is for: an MCP or CLI peer with no frontend attached.
///
/// So the sweep discovers unknown panes, and this test is the assertion that it does. It must not poll:
/// polling would drive the very evaluation being tested, which is how the earlier version of this claim
/// passed while being wrong. It therefore waits blind, then asks once.
///
/// The wait is sweep + settle with room to spare. It is the slowest test in this file, and the reason
/// is the thing being proven: nothing may touch the daemon in between.
#[test]
fn one_query_on_a_never_queried_daemon_answers_a_settled_verdict() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \
         \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
    ]);

    // Blind. No connection, no query, no input — the daemon is alone with its own clock.
    std::thread::sleep(Duration::from_secs(12));

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");
    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"], "idle",
        "the daemon settled this pane with nobody asking: {entry}",
    );
    assert_eq!(
        entry["agent"]["name"], "claude",
        "and knows whose pane it is: {entry}"
    );
    assert_eq!(
        entry["agent"]["seq"], 1,
        "published exactly once, so the sweep is not re-publishing on a loop: {entry}",
    );
}

/// **THE slice-3 proof, over a real socket against the shipped daemon.** A wake tells a client that
/// something moved; this asks WHAT, and the asking must be free.
///
/// Three claims, and the third is the one that needed a daemon rather than a unit test:
///
/// 1. a structural change is READABLE by cursor — spawn a pane, and the batch names it;
/// 2. the pair composes — the revision `scene/waitFor` answers with is the cursor this reads at, so
///    a client parks and reads with one number and no counter of its own;
/// 3. **reading does not BUMP.** `events.<since>` is a `scene/query`, so it is classified
///    `MethodOcc::Read` and the revision is untouched. Served as an invoke it would be `Mutate`,
///    and a reader parked on `scene/waitFor` would wake its own waiter by reading — for an event
///    stream, reading events would generate events. That is the R152 livelock in its worst form,
///    and `cells.<offset>` records sprag having already met it once.
#[test]
fn the_events_family_reads_a_change_by_cursor_and_reading_does_not_bump() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // A read before anything has moved: the daemon has observed a shape but recorded no change, and
    // the answer must say so rather than being absent.
    let baseline = read_revision(&mut conn);
    let first: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(baseline)) }),
        )
        .expect("the events family answers");
    assert_eq!(
        first["events"].as_array().map(Vec::len),
        Some(0),
        "nothing has changed yet: {first}",
    );
    assert_eq!(
        first["lost"], false,
        "and `lost` travels even when false — absent must not be able to mean fine: {first}",
    );

    // Claim 3, measured across the read that follows as well as this one.
    let before_read = read_revision(&mut conn);
    let _: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(0)) }),
        )
        .expect("a second read answers");
    assert_eq!(
        read_revision(&mut conn),
        before_read,
        "reading the change log must not advance the token the log is keyed by",
    );

    // Claim 1: a real mutation, over the real dispatch, with nothing asking for an event.
    let since = read_revision(&mut conn);
    let _: Value = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
        )
        .expect("spawn a pane over the wire");

    // Claim 2: the wake's own number is the cursor. `waitFor` has already been satisfied by the
    // spawn's bump, so this returns immediately with the revision to read at.
    let woken: Value = conn
        .call("scene/waitFor", json!({ "since": since }))
        .expect("the spawn advanced the scene");
    assert_eq!(woken["changed"], true, "the spawn moved the scene: {woken}");

    let batch: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(since)) }),
        )
        .expect("the events family answers after a change");
    let events = batch["events"].as_array().expect("an events array");
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "pane_created" && event["pane"].is_u64()),
        "the spawn is readable as a typed change naming its subject: {batch}",
    );
    assert_eq!(
        batch["lost"], false,
        "and nothing was dropped on the way: {batch}",
    );

    // The cursor advances: reading at what the batch says to read from next reports no repeat.
    let next = batch["next"].as_u64().expect("a next cursor");
    let after: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(next)) }),
        )
        .expect("the events family answers at the new cursor");
    assert_eq!(
        after["events"].as_array().map(Vec::len),
        Some(0),
        "a change is delivered ONCE, to the cursor that had not seen it: {after}",
    );

    // A malformed member of an ADVERTISED family is present-but-empty, never absent: `None` becomes
    // `UnknownIntrospectPath`, meaning "not in its schema", and `events.zzz` IS in the schema. The
    // taxonomy `cells.<offset>` was corrected into by R155's review.
    let malformed: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path("events.zzz") }),
        )
        .expect("a malformed member is answered, not refused");
    assert!(
        malformed.is_null() || malformed["value"].is_null(),
        "`events.zzz` belongs to a declared family and is malformed, not unknown: {malformed}",
    );
}

/// **THE R292 proof, over a real socket against the shipped daemon: a filtered wait sleeps through
/// OUTPUT, and the old pair does not.**
///
/// The control is what makes this a measurement rather than an assertion. Both waits are issued
/// against the SAME daemon with the SAME chatty pane running, on their own connections, with the same
/// deadline:
///
/// * `scene/waitFor {since}` returns almost immediately, and the batch that follows it is EMPTY —
///   the defect, which cost the agent surface a tool call and an LLM turn per output batch. Measured
///   at 22 431 returns a second against a build-rate pane, all empty — a rate `sprag-latency`'s
///   poll-pair row reproduces (17 152/s on another box) and explains: the follower's cursor is the
///   journal's, the scene runs away from it, and `waitFor` therefore takes the catch-up path every
///   time instead of parking.
/// * `events/waitFor {since, match}` trips its deadline instead, because output appends no record
///   and this wait's condition is a record.
///
/// Then the change the caller DID ask about is made, and the same wait returns it — so the deadline
/// above is a wait that works rather than a wait that is broken in the other direction.
#[test]
fn a_filtered_wait_sleeps_through_output_where_the_scene_wait_does_not() {
    // A pane that writes forever, which is what a build looks like to the daemon.
    let (_host, sock) = spawn_host_running(&[
        "bash",
        "-c",
        "while :; do echo building a thing; sleep 0.02; done",
    ]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");
    // Let the pane actually start writing, so the control below is measuring output and not a race
    // with the boot.
    std::thread::sleep(Duration::from_millis(500));

    // THE CONTROL: the pair the MCP tool used to be built from. It returns at once, and says nothing.
    let since = read_revision(&mut conn);
    conn.set_read_deadline(Some(Duration::from_secs(2)))
        .expect("a deadline for the control");
    let woken: Value = conn
        .call("scene/waitFor", json!({ "since": since }))
        .expect("output releases a scene wait");
    assert_eq!(
        woken["changed"], true,
        "the control: OUTPUT alone releases a scene wait: {woken}",
    );
    conn.set_read_deadline(None).expect("clear the deadline");
    let empty: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(since)) }),
        )
        .expect("the events slot answers");
    assert_eq!(
        empty["events"].as_array().map(Vec::len),
        Some(0),
        "and the batch behind that wake is EMPTY — the whole defect in one assertion: {empty}",
    );

    // THE SUBJECT: the same daemon, the same output, a wait that named what it cares about.
    let mut waiter = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("a second connection for the filtered wait");
    let since = read_revision(&mut waiter);
    waiter
        .set_read_deadline(Some(Duration::from_secs(2)))
        .expect("the same deadline the control had");
    let timed_out = waiter.call(
        EVENTS_WAIT_METHOD,
        json!({ SINCE_PARAM: since, "match": [{ "kind": "pane_created" }] }),
    );
    let error = timed_out.expect_err("a filtered wait must NOT be woken by output");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        "it must sleep until ITS deadline, not fail some other way: {error}",
    );

    // And it is not merely broken: the change it asked for wakes it. A fresh connection, because the
    // one above tripped its deadline and is finished — which is also what releases the parked wait.
    let mut waiter = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("a third connection for the wait that gets its answer");
    let since = read_revision(&mut waiter);
    waiter
        .set_read_deadline(Some(Duration::from_secs(10)))
        .expect("a deadline generous enough to be an answer, not a race");
    let waiting = std::thread::spawn(move || {
        waiter.call(
            EVENTS_WAIT_METHOD,
            json!({ SINCE_PARAM: since, "match": [{ "kind": "pane_created" }] }),
        )
    });
    // Give the park time to be registered, then make the change on ANOTHER connection.
    std::thread::sleep(Duration::from_millis(300));
    let _: Value = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": {} }),
        )
        .expect("spawn a pane over the wire");

    let batch = waiting
        .join()
        .expect("the waiting thread")
        .expect("the filtered wait is answered by the change it named");
    let events = batch["events"].as_array().expect("an events array");
    assert_eq!(
        events.len(),
        1,
        "exactly the change asked for, with the session's output nowhere in it: {batch}",
    );
    assert_eq!(events[0]["type"], "pane_created");
    assert!(events[0]["pane"].is_u64(), "naming its subject: {batch}");
    assert_eq!(batch["lost"], false, "and nothing was dropped: {batch}");
    assert!(
        batch["next"].as_u64().is_some_and(|next| next > since),
        "with a cursor to resume from: {batch}",
    );
}

/// A client PARKED on the session it holds is woken BY THE RENAME ITSELF, and told the new name.
///
/// # Why this test exists, and what passed without it
///
/// Everything else about a rename is observable from the outside afterwards — the listing, the
/// journal, the refusal on the retired name — and all of it passes even when the change is derived
/// one request TOO LATE. The derive site runs after every dispatch and reads the shape at the
/// address the REQUEST carried; a rename retires exactly that address, so reading it there records
/// nothing and the rename lands only when some later request happens to be dispatched on the
/// session. A client parked at the moment of the rename would sleep through it, and the very next
/// `sprag events` would still show the record — which is why reverting the fix left the CLI test
/// green, and why this is the assertion that had to be BUILT rather than registered.
///
/// It also pins the CHANNEL move: the wait below is parked in the channel keyed by the OLD name, and
/// a rename that minted a fresh one instead of carrying this one across would leave it parked
/// forever.
#[test]
fn a_rename_wakes_the_client_parked_on_the_name_it_moved() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    let mut waiter = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("a second connection to park the wait on");
    let since = read_revision(&mut waiter);
    waiter
        .set_read_deadline(Some(Duration::from_secs(10)))
        .expect("a deadline generous enough to be an answer, not a race");
    // The filter is the question this event exists to answer: *tell me when the address I hold
    // stops resolving*. It names the OLD name, which is the only one this client can know.
    let waiting = std::thread::spawn(move || {
        waiter.call(
            EVENTS_WAIT_METHOD,
            json!({
                SINCE_PARAM: since,
                "match": [{ "kind": "session_renamed", "session": "0" }],
            }),
        )
    });
    // Give the park time to register, then rename on ANOTHER connection.
    std::thread::sleep(Duration::from_millis(300));
    let _: Value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": "prod" },
            }),
        )
        .expect("rename the session over the wire");

    let batch = waiting
        .join()
        .expect("the waiting thread")
        .expect("the rename itself wakes the client parked on the name it moved");
    let events = batch["events"].as_array().expect("an events array");
    assert_eq!(events.len(), 1, "exactly the change asked for: {batch}");
    assert_eq!(events[0]["type"], "session_renamed");
    assert_eq!(
        events[0]["session"], "0",
        "the SUBJECT is the address this client held — filtering on the new name would leave \
         exactly the client that needs this event unwoken: {batch}",
    );
    assert_eq!(
        events[0]["name"], "prod",
        "and the detail is the one fact no later read could recover: {batch}",
    );
    assert_eq!(batch["lost"], false, "nothing was dropped: {batch}");
}

/// A filter that cannot ever match is a caller MISTAKE, and the daemon says so at once rather than
/// parking it forever. The sentence is asserted, not just the failure: a refusal an agent cannot act
/// on is barely better than the silence it replaces.
#[test]
fn a_filter_a_daemon_cannot_honour_is_refused_with_a_sentence() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    let error = conn
        .call(
            EVENTS_WAIT_METHOD,
            json!({ SINCE_PARAM: 0, "match": [{ "kind": "pane_output_matched" }] }),
        )
        .expect_err("a kind this daemon does not report is refused");
    let sentence = error.to_string();
    assert!(
        sentence.contains("is not a change this terminal reports"),
        "the refusal must say what is wrong: {sentence}",
    );
    assert!(
        sentence.contains("pane_job_changed"),
        "and offer the vocabulary it could have asked for: {sentence}",
    );

    let error = conn
        .call(EVENTS_WAIT_METHOD, json!({ "match": [{ "pane": 1 }] }))
        .expect_err("a wait with no cursor is refused");
    assert!(
        error.to_string().contains("since"),
        "naming the missing parameter: {error}",
    );

    // The connection still works afterwards: a refusal is an answer, not a broken pipe.
    let revision = read_revision(&mut conn);
    assert!(
        revision < u64::MAX,
        "the connection survived both refusals and still answers reads",
    );
}

/// **THE slice-4 proof, over a real socket.** The agent transition is the event this niche exists
/// for, and it is the one the dispatch funnel cannot derive — so this asks whether the SWEEP's own
/// verdict reaches a reader as a typed change.
///
/// Blind on purpose, like `one_query_on_a_never_queried_daemon_answers_a_settled_verdict`: nothing
/// connects while the settle window runs. A verdict resting on an ABSENCE ("the agent stopped
/// working") is confirmed by a clock nothing else in the daemon runs, so a test that kept a client
/// attached would be measuring the client's wakes rather than the waker's own pass.
#[test]
fn the_sweeps_own_verdict_reaches_a_reader_as_a_typed_change() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \
         \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
    ]);

    // The daemon alone with its clock, long enough for the candidate to settle and publish.
    std::thread::sleep(Duration::from_secs(12));

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // From the beginning of time, because this reader was not here when it happened — which is the
    // whole point: the record outlives the wake, so a client that arrives late still learns.
    let batch: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&events_slot_since(0)) }),
        )
        .expect("the events family answers");
    let events = batch["events"].as_array().expect("an events array");
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "pane_agent_state_changed" && event["pane"] == 0),
        "the settle waker's own transition is readable as a typed change: {batch}",
    );
    assert_eq!(
        batch["lost"], false,
        "and nothing was evicted before this reader arrived: {batch}",
    );

    // The record names its subject and nothing else — the reader turns it into ONE targeted read of
    // the slot where a verdict is defined, rather than being handed a second copy of it here.
    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"], "idle",
        "and the subject it names answers: {entry}",
    );
}

/// A pane born under a REAL `sprag-term` carries its own identity in its child's environment — the
/// end of the chain `sprag_host::pane_env_source` starts, driven the only way that proves the daemon
/// installs it at all.
///
/// The wiring is one call in `sprag-term`'s `main`, and no unit test can reach it: a `Host` built
/// without `with_pane_env` satisfies every other test in this file, and a pane spawned by such a
/// daemon looks identical from the outside. R258's lesson — a wiring nothing drives is untested.
///
/// **What this asserts and what it does not.** `SPRAG_PANE` is a proof of INJECTION: nothing in the
/// environment of the process running this test sets it, so a value in the pane's child can only have
/// come from the birth. The address half is asserted for agreement, not injection — this harness
/// hands the daemon `SPRAG_HOST_RPC_SOCK` to control its socket path, so the child would inherit the
/// same string either way. That the source publishes it under the variable a client overrides is
/// pinned where a control exists, in `host::tests`.
#[test]
fn a_daemon_born_pane_is_told_which_pane_it_is_and_where_to_report() {
    // The boot pane prints both variables and then holds its PTY open, so the screen stays readable.
    // `-` between them so an empty value cannot masquerade as the other's.
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf 'PANEENV %s-%s\\n' \"$SPRAG_PANE\" \"$SPRAG_HOST_RPC_SOCK\"; cat",
    ]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // The boot pane's id, read from the wire rather than assumed to be 0 — the assertion below is
    // about the child being told ITS OWN id, so taking that id from the same list a client reads is
    // what keeps the two from agreeing by coincidence.
    let panes = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("panes query");
    let id = panes[0]["id"].as_u64().expect("the boot pane has an id");

    let expected = format!("PANEENV {id}-{}", sock.display());
    let mut seen = String::new();
    let printed = wait_until(Duration::from_secs(5), || {
        seen = conn
            .call(
                "scene/query",
                json!({ "path": pane_input_path(id, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|v: Value| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        // Row breaks removed before matching: this harness boots a 40-COLUMN pane and a socket path
        // under the temp dir is longer than that, so the emulator wraps the line mid-path — the
        // first run of this test read `…/sprag-wire-it-0.so\nck`. The wrap is the terminal's, not
        // the child's, and the assertion is about what the child was TOLD.
        seen.replace('\n', "").contains(&expected)
    });
    assert!(
        printed,
        "the boot pane's child never printed {expected:?}; its screen was {seen:?}",
    );
}

/// The PUSH path end to end against a REAL `sprag-term`: a report outranks the daemon's own scrape,
/// and a release hands the pane back — with the correction served by the daemon's waker rather than by
/// anything this test does.
///
/// Nothing else in the suite reaches that second half. The release only sets a flag; the SCREEN it
/// must be re-read from is reachable only from the waker's pass, so a daemon whose waker did not learn
/// the new reason to work (`AgentRegistry::any_owes_look` in its guard) would answer this test's
/// release with a pane frozen in its reported state. The unit tests prove the guard's predicate and
/// the signal in isolation; this is the only place the two run in the daemon that owns them.
///
/// **What the timing can and cannot separate.** The bound below is well under
/// `SWEEP_INTERVAL`, so a daemon that served the release only on its next scheduled sweep would
/// usually fail it — but not always, since a release can land shortly before a sweep that was coming
/// anyway. The bound is therefore a real contract and a partial proof of the mechanism; the mechanism
/// itself is pinned where it can be, in `agent::tests`.
#[test]
fn a_report_outranks_the_daemons_scrape_and_a_release_gives_the_pane_back() {
    // `agent-settle-time = 0` is what makes the bound below an instrument rather than a stopwatch. With
    // the default window a released pane's RESTING verdict has to hold two seconds before it can be
    // published, so the correction is wake + window and no bound separates a signalled wake from a
    // sweep that was coming anyway. At zero the window is gone and the only thing left between the
    // release and the new verdict is the wake itself.
    let dir = std::env::temp_dir().join(format!("sprag-wire-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    std::fs::write(
        dir.join("sprag").join(sprag_host::config::CONFIG_FILE),
        "[options]\nagent-settle-time = 0\n",
    )
    .expect("write the config");

    // A `claude` pane at rest: the resting title and the footer its fingerprint reads, then `cat` so
    // the pane stays put.
    let (_host, sock) = spawn_host_with(
        &[
            "sh",
            "-c",
            "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\\342\\235\\257\\n  \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
        ],
        &[("XDG_CONFIG_HOME", &dir.display().to_string())],
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // The daemon's own reading first, so what the report overrides is a real scraped verdict rather
    // than an absence.
    let scraped = wait_until(Duration::from_secs(10), || {
        pane_entry(&mut conn, 0)["agent"]["state"].as_str() == Some("idle")
    });
    assert!(scraped, "the daemon never scraped this pane as idle");
    let entry = pane_entry(&mut conn, 0);
    assert!(
        entry["agent"]["rule"].is_string() && entry["agent"]["source"].is_null(),
        "a scraped verdict names its rule and no reporter: {entry}",
    );

    // The report. `working` is not what the screen says, and it needs no settle window.
    let answer: Value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(REPORT_AGENT_ACTION),
                "args": {
                    "id": 0,
                    "source": "wire-it",
                    "state": "working",
                    "name": "claude",
                    "seq": 1,
                },
            }),
        )
        .expect("the report crosses the socket");
    assert_eq!(answer["accepted"], json!(true), "accepted: {answer}");
    assert_eq!(answer["changed"], json!(true), "and it moved the verdict");

    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"],
        json!("working"),
        "the report outranks the daemon's own reading of the screen: {entry}",
    );
    assert_eq!(entry["agent"]["source"], json!("wire-it"));
    assert!(
        entry["agent"]["rule"].is_null(),
        "and it carries no rule, because none fired: {entry}",
    );

    // A replay of the same sequence number is refused, over the same socket the hook would use.
    let replay: Value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(REPORT_AGENT_ACTION),
                "args": {"id": 0, "source": "wire-it", "state": "idle", "seq": 1},
            }),
        )
        .expect("a refused report is still a well-formed call");
    assert_eq!(
        replay["accepted"],
        json!(false),
        "a replay is refused: {replay}"
    );
    assert_eq!(
        pane_entry(&mut conn, 0)["agent"]["state"],
        json!("working"),
        "and changed nothing",
    );

    // WHAT COUNTS AS EVIDENCE HERE, after a first draft that had none. Polling the pane list would
    // SERVE the release — a pane-list query observes every pane it describes — and parking on
    // `waitFor` proves nothing either, because pinion bumps the scene revision inside its own
    // dispatcher: the release's own invoke wakes the park whatever the daemon does with it. The
    // revert-proof caught both.
    //
    // The one observable that only the daemon can produce is the JOURNAL: `agent_state_changed` is
    // recorded when a verdict MOVES, and after the release the only thing that can move this pane's
    // verdict is the waker's own pass. `events.<since>` is a QUERY — it reads the log and observes no
    // pane — so watching it cannot serve what it is watching for.
    let cursor = read_revision(&mut conn);
    let released: Value = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(RELEASE_AGENT_ACTION),
                "args": {"id": 0},
            }),
        )
        .expect("the release crosses the socket");
    assert_eq!(
        released["released"],
        json!(true),
        "a report was in force: {released}"
    );

    // With `agent-settle-time = 0` the re-derived verdict publishes on sight, so the only thing between
    // the release and the record below is the waker's trip round its loop — which needs both the signal
    // (`AgentClock::release` notifies) and a reason to act on a wake that is not a sweep
    // (`any_owes_look` in the guard). One second against a sweep interval of five.
    let corrected = wait_until(Duration::from_secs(1), || {
        let batch: Value = conn
            .call(
                "scene/query",
                json!({ "path": mux_action_path(&events_slot_since(cursor)) }),
            )
            .expect("the events family answers");
        batch["events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| event["type"] == "pane_agent_state_changed" && event["pane"] == 0)
    });
    assert!(
        corrected,
        "the daemon never re-derived the released pane's verdict on its own",
    );

    // Only NOW read the pane list, so what it reports was published by the daemon rather than produced
    // by this query.
    let entry = pane_entry(&mut conn, 0);
    assert_eq!(
        entry["agent"]["state"],
        json!("idle"),
        "released, the pane is the screen's again: {entry}",
    );
    assert!(
        entry["agent"]["source"].is_null(),
        "and it names no reporter: {entry}",
    );
    assert!(
        entry["agent"]["rule"].is_string(),
        "the verdict is a rule's again: {entry}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A request written the way a client from BEFORE the shape agreement wrote one — no `protocol`
/// key — put on the wire BY HAND, because the point is a request this build can no longer produce
/// (`HostConn` adds the key at its one seam).
///
/// Returns the daemon's whole reply line, so the test reads exactly what an old client would have.
/// The connect retries like [`HostConn::connect`] does, for the same reason: the daemon binds
/// asynchronously and a bare connect would race it.
fn raw_request(sock: &std::path::Path, line: &str) -> Value {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match UnixStream::connect(sock) {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the daemon never bound {}: {error}",
                    sock.display(),
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    writeln!(stream, "{line}").expect("write the request");
    stream.flush().expect("flush the request");
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .expect("read the reply");
    serde_json::from_str(reply.trim()).expect("the daemon answers JSON")
}

/// A daemon refuses a request written against a wire shape it does not speak — the half of the
/// agreement a CLIENT cannot perform, because an old client contains no check to run.
///
/// This is the direction that bit. R264 flattened the layout wire; a `sprag-tui` left over from
/// before it created a session and then died decoding the ninth reply with a serde message about
/// an integer. With this, its FIRST request is refused with a sentence naming both numbers, and
/// nothing is created at all.
///
/// REVERT-PROOF: delete the `protocol_refused` call in `dispatch_one` and this reads `result`
/// where it expects `error`.
#[test]
fn a_request_without_the_wire_protocol_is_refused_by_name() {
    let (_host, sock) = spawn_host();

    let old = raw_request(
        &sock,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "scene/query",
            "params": { "path": mux_action_path(PANES_SLOT) },
        })
        .to_string(),
    );

    let message = old["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("a client with no protocol must be refused, got: {old}"));
    assert!(
        message.contains(&format!("speaks wire protocol {WIRE_PROTOCOL}")),
        "the refusal names what THIS daemon speaks: {message}",
    );
    assert!(
        message.contains("a client older than this check"),
        "and what the caller spoke, which is nothing: {message}",
    );
    assert!(
        message.contains("sprag kill-server"),
        "and the action that resolves it: {message}",
    );
}

/// The CONTROL for the refusal above: the same request, carrying this build's protocol, is served.
/// Without it, a daemon that refused everything would look like a passing test.
#[test]
fn the_same_request_carrying_this_protocol_is_served() {
    let (_host, sock) = spawn_host();

    let current = raw_request(
        &sock,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "scene/query",
            "params": { "path": mux_action_path(PANES_SLOT), PROTOCOL_PARAM: WIRE_PROTOCOL },
        })
        .to_string(),
    );

    assert!(
        current["result"].is_array(),
        "a request that declares this build's shape is answered: {current}",
    );
    assert!(current["error"].is_null(), "and is not refused: {current}");
}

/// The daemon ANSWERS with its own protocol, which is the other direction's only evidence: a
/// daemon older than its client ignores the request param and serves happily, so a client can only
/// learn the truth from a reply. `HostConn::handshake` is what reads it.
#[test]
fn the_hello_reply_carries_the_daemons_protocol() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    let reply = conn
        .call(
            CLIENT_HELLO_METHOD,
            json!({ CLIENT_PARAM: "cli-protocol-test" }),
        )
        .expect("the daemon answers a hello");

    assert_eq!(
        reply[PROTOCOL_FIELD],
        json!(WIRE_PROTOCOL),
        "a client learns the daemon's shape from the announcement it was already sending: {reply}",
    );
    conn.handshake("cli-protocol-test")
        .expect("and the handshake over the same method agrees");
}

/// **R315 over the REAL socket: a CHOOSER'S PICK is an identity, and it lands on the row the person
/// read even after that row has been renamed and a stranger has taken its name.**
///
/// The round's central claim, driven end to end, and **the fixture is R304's own — built so a NAME
/// and an IDENTITY land on DIFFERENT LIVE SESSIONS**: the picked session is renamed (so the label
/// the chooser painted resolves to nothing) and a new session takes the freed name (so it resolves
/// to a stranger). Both are live. A chooser that committed the label lands on `work`; one that
/// commits the identity lands on `renamed`.
///
/// It is not R304's claim, though: there the daemon holds the past and the client cannot name it,
/// and here the CLIENT holds the past — it has the label on the screen — and must not send it. That
/// is why the identity had to reach the wire at all.
///
/// The list is READ FROM THE TREE SLOT rather than assumed, so the ids under test are the ones a
/// real chooser would have painted from.
///
/// REVERT-PROOF: commit the label instead (send `{"session": <name>}` as a scope and a plain
/// attach) and the first assertion lands on the impostor with both readings still "succeeding".
#[test]
fn a_pick_lands_on_the_session_it_named_after_a_stranger_takes_its_name() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    let new_session = |conn: &mut HostConn, name: &str| {
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": name } }),
        )
        .expect("new_session answers");
    };
    new_session(&mut admin, "work");

    // WHAT A CHOOSER WOULD HAVE PAINTED. Read off the slot, so the id below is the one a person's
    // screen was built from rather than a number this test invented.
    let picked = tree_of(&mut admin);
    let work = picked
        .iter()
        .find(|session| session["name"] == json!("work"))
        .expect("the tree lists the session that was just made");
    let work_id = work["id"]
        .as_u64()
        .expect("a tree row carries its identity");

    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("it starts on the boot session");

    // ...AND THEN THE WORLD MOVES UNDER THE OPEN LIST, which is what a chooser is for and what
    // makes its pick a fact about the past.
    admin
        .call(
            "scene/invoke",
            json!({
                "session": "work",
                "path": mux_action_path(RENAME_SESSION_ACTION),
                "args": { "name": "renamed" },
            }),
        )
        .expect("rename_session answers");
    new_session(&mut admin, "work");
    assert!(
        session_names(&mut admin).contains(&"work".to_owned()),
        "the impostor is LIVE — a pick that carried the label would resolve straight onto it",
    );

    let goto = |conn: &mut HostConn, ask: sprag_host::wire::AttachAsk| -> Result<Value, _> {
        let mut params = serde_json::Map::new();
        ask.write_into(&mut params);
        conn.call(CLIENT_ATTACH_METHOD, Value::Object(params))
    };
    assert_eq!(
        goto(
            &mut viewer,
            sprag_host::wire::AttachAsk::Goto {
                session: sprag_terminal::SessionId(work_id),
                window: None,
                pane: None,
            },
        )
        .expect("a well-formed pick is accepted"),
        json!("renamed"),
        "the row that was PICKED, under the name it has now — never the stranger wearing its old \
         one",
    );
    assert_eq!(
        attached_of(&mut admin, "renamed"),
        1,
        "and it really went: the daemon counts it on that session's badge",
    );
    assert_eq!(
        attached_of(&mut admin, "work"),
        0,
        "...and not on the impostor's",
    );

    // A PICK THAT IS GONE IS REFUSED, which is the answer a label could never give — a label that
    // something else has taken RESOLVES. The id is one no session carries.
    let missing = goto(
        &mut viewer,
        sprag_host::wire::AttachAsk::Goto {
            session: sprag_terminal::SessionId(9999),
            window: None,
            pane: None,
        },
    )
    .expect_err("a pick naming nothing is refused");
    assert!(
        missing.to_string().contains("is gone"),
        "the refusal says the row went, rather than falling back to somewhere: {missing}",
    );
    assert_eq!(
        attached_of(&mut admin, "renamed"),
        1,
        "...and the refused pick left the client exactly where it was",
    );
}

/// **A pick naming a WINDOW takes the client there AND selects it — and a path with a dead level is
/// refused WHOLE, leaving the client where it was.**
///
/// The half a session-only chooser cannot have, and the reason the wire carries a path rather than
/// a leaf: attaching and selecting are two acts, and a person who picked a window row asked for
/// both. The refusal is what says they get both or neither.
///
/// THE FIXTURE MAKES THE TWO OUTCOMES DISAGREE: the window picked is NOT the session's current one,
/// so "went to the session" and "went to the window" are different observations, and the second is
/// what is read.
///
/// REVERT-PROOF: perform the window selection before checking it exists (drop `resolve_goto`'s
/// window arm) and the dead-window assertion below finds the client attached to `work` instead of
/// still on the boot session.
#[test]
fn a_pick_naming_a_window_selects_it_and_a_dead_one_refuses_the_whole_path() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    // Two more windows, so the one picked is neither the first nor the current.
    for name in ["build", "logs"] {
        admin
            .call(
                "scene/invoke",
                json!({
                    "session": "work",
                    "path": mux_action_path(NEW_WINDOW_ACTION),
                    "args": { "name": name },
                }),
            )
            .expect("new_window answers");
    }
    let boot = session_names(&mut admin)
        .into_iter()
        .next()
        .expect("a boot session");

    let tree = tree_of(&mut admin);
    let work = tree
        .iter()
        .find(|session| session["name"] == json!("work"))
        .expect("the tree lists work");
    let work_id = work["id"].as_u64().expect("an identity");
    let windows = work["windows"].as_array().expect("a session's windows");
    assert_eq!(windows.len(), 3, "three windows, so a pick can be specific");
    let build = &windows[1];
    assert_eq!(build["name"], json!("build"));
    assert_eq!(
        build["current"],
        json!(false),
        "and it is NOT the current one, so selecting it is observable",
    );
    let build_id = build["id"].as_u64().expect("a window carries its identity");

    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("it starts on the boot session");

    let goto = |conn: &mut HostConn, session: u64, window: Option<u64>| -> Result<Value, _> {
        let mut params = serde_json::Map::new();
        sprag_host::wire::AttachAsk::Goto {
            session: sprag_terminal::SessionId(session),
            window: window.map(sprag_terminal::WindowId),
            pane: None,
        }
        .write_into(&mut params);
        conn.call(CLIENT_ATTACH_METHOD, Value::Object(params))
    };

    // A DEAD WINDOW FIRST, so a handler that attached before checking fails here rather than
    // passing the happy path below.
    let missing = goto(&mut viewer, work_id, Some(9999))
        .expect_err("a path naming a window that is not there is refused");
    assert!(missing.to_string().contains("is gone"), "{missing}");
    assert_eq!(
        attached_of(&mut admin, "work"),
        0,
        "and the refused path did NOT attach the client on its way to failing",
    );
    assert_eq!(
        attached_of(&mut admin, &boot),
        1,
        "...it is still exactly where it was",
    );

    // ...and the live one takes it there and selects the window.
    assert_eq!(
        goto(&mut viewer, work_id, Some(build_id)).expect("a live path is accepted"),
        json!("work"),
    );
    assert_eq!(attached_of(&mut admin, "work"), 1);
    assert_eq!(
        windows_in(&mut admin, "work"),
        vec![
            ("0".to_owned(), false),
            ("build".to_owned(), true),
            ("logs".to_owned(), false),
        ],
        "the picked window is the session's current one now — which is what a WINDOW row means",
    );
}

/// The registry-wide TREE as the daemon publishes it — every level, with its identity, in ONE read.
fn tree_of(conn: &mut HostConn) -> Vec<Value> {
    conn.call(
        "scene/query",
        json!({ "path": mux_action_path(sprag_host::wire::TREE_SLOT) }),
    )
    .expect("the tree slot answers")
    .as_array()
    .expect("the tree is a list")
    .clone()
}

/// **The tree carries an identity at every level, and a RENAME does not move any of them** — which
/// is the property the whole pick rests on and the one a name cannot have.
///
/// REVERT-PROOF: mint a fresh id on rename (or key the tree by name) and the second half fails with
/// two different numbers for one session.
#[test]
fn the_tree_publishes_an_identity_at_every_level_and_a_rename_does_not_move_it() {
    let (_host, sock) = spawn_host_running(&["cat"]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    let boot = session_names(&mut conn)
        .into_iter()
        .next()
        .expect("a boot session");

    let before = tree_of(&mut conn);
    assert_eq!(before.len(), 1, "one session: {before:?}");
    let session = &before[0];
    let session_id = session["id"]
        .as_u64()
        .expect("a session carries an identity");
    let window = &session["windows"].as_array().expect("windows")[0];
    let window_id = window["id"].as_u64().expect("a window carries an identity");
    let pane = &window["panes"].as_array().expect("panes")[0];
    assert_eq!(
        pane["command"],
        json!("cat"),
        "a pane row says what it is running, which is what names an UNNAMED pane",
    );
    assert_eq!(pane["active"], json!(true));
    assert!(
        pane["id"].is_u64(),
        "and the pane's own id, which has been on the wire all along",
    );

    // THE RENAME — the act that moves a NAME and must not move an identity.
    conn.call(
        "scene/invoke",
        json!({
            "session": &boot,
            "path": mux_action_path(RENAME_SESSION_ACTION),
            "args": { "name": "renamed" },
        }),
    )
    .expect("rename_session answers");
    conn.call(
        "scene/invoke",
        json!({
            "session": "renamed",
            "path": mux_action_path(RENAME_WINDOW_ACTION),
            "args": { "name": "build" },
        }),
    )
    .expect("rename_window answers");

    let after = tree_of(&mut conn);
    let session = &after[0];
    assert_eq!(
        session["name"],
        json!("renamed"),
        "the LABEL moved, which is what makes this a discriminating fixture",
    );
    assert_eq!(
        session["id"].as_u64(),
        Some(session_id),
        "...and the IDENTITY did not",
    );
    let window = &session["windows"].as_array().expect("windows")[0];
    assert_eq!(window["name"], json!("build"));
    assert_eq!(window["id"].as_u64(), Some(window_id), "one level down too");
}

/// **The tree and the `panes` slot agree about which pane is HERE** — two surfaces, one fact.
///
/// ⚠ **THIS ASSERTION EXISTS BECAUSE THE FIRST VERSION OF THE TREE FAILED IT, on a live daemon.**
/// `SessionRegistry::tree` read `Window::active_pane()` raw, and a window whose layout has never
/// been reconciled answers `None` — which is every freshly booted session. So the tree said no pane
/// was active while `mux.panes` reported the same pane as active in the same instant. Nothing in
/// the type system connects the two; this does.
///
/// It also drives the case the bug lived in — a session NOBODY has selected a pane in — and then a
/// second one where a select HAS happened, so the agreement is checked in both states rather than
/// in the one that happens to be easy.
///
/// REVERT-PROOF: drop the reconcile from the tree's third phase and the FIRST comparison fails with
/// `None` against `Some(0)`.
#[test]
fn the_tree_and_the_pane_list_name_the_same_active_pane() {
    let (_host, sock) = spawn_host_running(&["cat"]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    let boot = session_names(&mut conn)
        .into_iter()
        .next()
        .expect("a boot session");
    conn.scope_to(boot.clone());

    /// The pane the `panes` slot marks — the daemon's older answer to "here".
    fn active_in_panes(conn: &mut HostConn) -> Option<u64> {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(sprag_host::wire::PANES_SLOT) }),
        )
        .expect("the panes slot answers")
        .as_array()
        .expect("a list")
        .iter()
        .find(|row| row["active"] == json!(true))
        .map(|row| row["id"].as_u64().expect("a pane carries its id"))
    }
    let active_in_tree = |conn: &mut HostConn| -> Option<u64> {
        tree_of(conn)[0]["windows"].as_array().expect("windows")[0]["panes"]
            .as_array()
            .expect("panes")
            .iter()
            .find(|pane| pane["active"] == json!(true))
            .map(|pane| pane["id"].as_u64().expect("a pane carries its id"))
    };

    // NOBODY HAS SELECTED ANYTHING — the state the defect lived in.
    //
    // ⚠ **THE TREE IS READ FIRST, AND THE ORDER IS THE TEST.** Reading the pane list first passed
    // with the fix REVERTED, because that slot's own read reconciles the window on its way past —
    // so the tree behind it saw a state somebody else had already repaired. The control caught it;
    // the assertion had been vacuous. Whichever surface is asked first must be the one under test.
    let in_tree = active_in_tree(&mut conn);
    let listed = active_in_panes(&mut conn);
    assert!(
        listed.is_some(),
        "the pane list marks the boot pane active, which is what makes this a discriminator",
    );
    assert_eq!(
        in_tree, listed,
        "and the tree said the same thing about the same pane, asked FIRST",
    );

    // ...and again after a SELECT has moved it, so the agreement is not an accident of the boot
    // state. The second pane is the one the split leaves the session on.
    let born = spawn_in(&mut conn, &boot);
    assert!(born > 0, "a second pane");
    let listed = active_in_panes(&mut conn);
    assert_eq!(
        active_in_tree(&mut conn),
        listed,
        "after a select, still one answer",
    );
    assert_ne!(
        listed, None,
        "...and it is a real pane, so the comparison above is not two Nones",
    );
}

/// **A pick naming a PANE puts the person on that pane, and a pane that has gone refuses the whole
/// path** — the deepest level, and the one the two tests above leave undriven.
///
/// ⚠ **THIS EXISTS BECAUSE THE AUDIT FOUND THE BRANCH DRIVEN BY NOTHING.** `resolve_goto` checks
/// the pane against its window's live POOL — a second lock acquisition, after the registry has
/// named the window — and the session and window arms cannot reach it: a path with a dead SESSION
/// or a dead WINDOW is refused before the pool is ever opened. So the one level whose check lives
/// in a different lock had no test at all.
///
/// THE FIXTURE MAKES THE READINGS DISAGREE: the pane picked is NOT the window's active one, so
/// "went to the window" and "went to the pane" are different observations and the second is what is
/// read.
///
/// REVERT-PROOF: drop the pool membership check from `resolve_goto` and the dead-pane assertion
/// finds the client attached to `work` with the window selected — a half-landing, which is the
/// state the whole-path rule exists to make unrepresentable.
#[test]
fn a_pick_naming_a_pane_selects_it_and_a_dead_one_refuses_the_whole_path() {
    let (_host, sock) = spawn_host();
    let mut admin = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    admin
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("new_session answers");
    admin.scope_to("work".to_owned());
    // A SECOND pane, so the pick can name one that is not the one the window is already on.
    let born = spawn_in(&mut admin, "work");
    assert!(born > 0, "a second pane exists to pick");
    let boot = session_names(&mut admin)
        .into_iter()
        .find(|name| name != "work")
        .expect("the boot session");

    let tree = tree_of(&mut admin);
    let work = tree
        .iter()
        .find(|session| session["name"] == json!("work"))
        .expect("the tree lists work");
    let work_id = work["id"].as_u64().expect("an identity");
    let window = &work["windows"].as_array().expect("windows")[0];
    let window_id = window["id"].as_u64().expect("an identity");
    let panes = window["panes"].as_array().expect("panes");
    assert_eq!(panes.len(), 2, "two panes: {panes:?}");
    // The one that is NOT active, so selecting it is observable.
    let wanted = panes
        .iter()
        .find(|pane| pane["active"] != json!(true))
        .expect("a pane the window is not on");
    let pane_id = wanted["id"].as_u64().expect("a pane carries its id");

    let mut viewer =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the display client connects");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "display" }))
        .expect("client/hello is accepted");
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("it starts on the boot session");

    let goto = |conn: &mut HostConn, pane: u64| -> Result<Value, _> {
        let mut params = serde_json::Map::new();
        sprag_host::wire::AttachAsk::Goto {
            session: sprag_terminal::SessionId(work_id),
            window: Some(sprag_terminal::WindowId(window_id)),
            pane: Some(sprag_terminal::PaneId(pane)),
        }
        .write_into(&mut params);
        conn.call(CLIENT_ATTACH_METHOD, Value::Object(params))
    };

    // A DEAD PANE FIRST, so a handler that acted before checking the deepest level fails here.
    let missing =
        goto(&mut viewer, 9999).expect_err("a path naming a pane that is gone is refused");
    assert!(missing.to_string().contains("is gone"), "{missing}");
    assert_eq!(
        attached_of(&mut admin, "work"),
        0,
        "and the refused path did NOT attach the client on its way to failing",
    );
    assert_eq!(attached_of(&mut admin, &boot), 1, "...it never moved");

    // ...and the live one takes it there and makes that pane active.
    assert_eq!(
        goto(&mut viewer, pane_id).expect("a live path is accepted"),
        json!("work"),
    );
    assert_eq!(attached_of(&mut admin, "work"), 1);
    let after = tree_of(&mut admin);
    let active = after
        .iter()
        .find(|session| session["name"] == json!("work"))
        .expect("work")["windows"]
        .as_array()
        .expect("windows")[0]["panes"]
        .as_array()
        .expect("panes")
        .iter()
        .find(|pane| pane["active"] == json!(true))
        .and_then(|pane| pane["id"].as_u64());
    assert_eq!(
        active,
        Some(pane_id),
        "the picked PANE is the one the window is on now — which is what a pane row means",
    );
}

/// **The `display_message` GRAMMAR, pinned at the wire** — the surface R317 added and the file that
/// pins every other action's grammar had no case for.
///
/// Found by the round's second debt sweep asking which arms had no driver. The CLI and the MCP tool
/// both check the grammar before sending, so a daemon that stopped enforcing it would leave every
/// higher gate green — which is exactly why the refusals belong HERE, one layer under both readers.
///
/// Four claims, and the last two are the arms nothing else can reach:
///
/// * A well-formed message is accepted and answers a `clients` LIST (empty here — no display client
///   is attached to this daemon, which is the honest state of a wire test).
/// * Each RULE is refused at the daemon: a control character, a blank, an over-long line, and a
///   severity this build does not know. A caller that skipped the client-side check gets no further.
/// * `client/messages` on a connection that never said hello answers `null` rather than refusing —
///   deliberate, so a probe's first read is not a protocol error, and reachable from no CLI.
/// * A `client` naming nobody is REJECTED, which is what keeps "you got the name wrong" distinct
///   from "nobody is watching".
#[test]
fn the_display_message_grammar_is_enforced_by_the_daemon_itself() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");

    let say = |conn: &mut HostConn, args: Value| -> Result<Value, sprag_rpc::CallError> {
        conn.try_call(
            "scene/invoke",
            json!({ "path": mux_action_path(DISPLAY_MESSAGE_ACTION), "args": args }),
        )
    };

    // A well-formed message: accepted, and the answer is a LIST. Nobody is attached to a daemon
    // nothing has displayed on, so it is empty — which is the fact `Delivery` exists to state.
    let answer = say(&mut conn, json!({ "text": "the build finished" }))
        .expect("a well-formed message crosses the socket");
    assert_eq!(
        answer["clients"],
        json!([]),
        "the answer is a delivery LIST, empty because no display client is attached: {answer}",
    );

    // EVERY RULE, refused at the daemon rather than only at the two clients that also check.
    for (name, args) in [
        ("a control character", json!({ "text": "two\nrows" })),
        ("a blank line", json!({ "text": "   " })),
        (
            "an over-long line",
            json!({ "text": "x".repeat(sprag_host::report::MessageText::MAX_BYTES + 1) }),
        ),
        (
            "an unknown severity",
            json!({ "text": "fine", "severity": "shout" }),
        ),
        (
            "a severity that is not a string",
            json!({ "text": "fine", "severity": 3 }),
        ),
        (
            "a client that is not a string",
            json!({ "text": "fine", "client": 7 }),
        ),
        ("no text at all", json!({ "severity": "note" })),
    ] {
        assert!(
            say(&mut conn, args.clone()).is_err(),
            "{name} must be refused by the DAEMON, not only by its callers: {args}",
        );
    }

    // A named client that is not attached is a REFUSAL, not an empty delivery — the distinction that
    // stops an agent hunting for a person who is right there.
    assert!(
        say(
            &mut conn,
            json!({ "text": "fine", "client": "gui-nobody-0" })
        )
        .is_err(),
        "a client that is not attached is a caller's mistake, not an empty audience",
    );

    // ...and the READ half: a connection that never said hello has no mailbox and is answered
    // `null`. Deliberate — a refusal here would make a probe's first read look like a skew.
    let collected: Value = conn
        .call(sprag_rpc::CLIENT_MESSAGES_METHOD, json!({}))
        .expect("collecting is a well-formed call with no hello");
    assert_eq!(
        collected[sprag_rpc::MESSAGE_FIELD],
        Value::Null,
        "a connection with no client has nothing waiting: {collected}",
    );
}
