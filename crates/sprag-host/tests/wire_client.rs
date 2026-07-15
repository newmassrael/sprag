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
use sprag_host::wire::{
    CLOSE_ACTION, FULL_TEXT_SLOT, LAYOUT_SLOT, PANES_SLOT, SET_FLOATING_ACTION, SET_LAYOUT_ACTION,
    SPAWN_ACTION, TEXT_ACTION, cells_slot_at,
};
use sprag_host::{mux_action_path, pane_input_path};
use sprag_rpc::HostConn;

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
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let child = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg("--size")
        .arg("40x6")
        .arg("--")
        .arg("cat")
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn the sprag-term host binary");
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
    let cells: pinion_core::GridBuffer =
        serde_json::from_value(frame["cells"].clone()).expect("cells deserialize to a GridBuffer");
    assert_eq!((cells.cols(), cells.rows()), (40, 6));

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
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(CLOSE_ACTION), "args": { "id": victim } }),
    )
    .expect("close the 2nd pane over the wire");
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
