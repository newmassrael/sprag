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
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_host::agent::SWEEP_INTERVAL;
use sprag_host::plugins::{PluginWorld, drive_request};
use sprag_host::remote_access::{RemotePaneAccess, RemotePluginWorld};
use sprag_host::wire::events_slot_since;
use sprag_host::wire::{
    ACTION_GRAMMAR_SLOT, AGENT_MANIFESTS_SLOT, ArgGrammar, BREAK_PANE_ACTION, CLIENTS_SLOT,
    CLOSE_ACTION, CallForm, DISPLAY_MESSAGE_ACTION, DROP_FILE_ACTION, FULL_LINES_SLOT,
    FULL_TEXT_SLOT, JOIN_PANE_ACTION, KEY_ACTION, KILL_SESSION_ACTION, LAYOUT_SLOT, LINKS_SLOT,
    MOVE_WINDOW_ACTION, NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANE_EOF_SLOT, PANE_SUMMARY_ID_KEY,
    PANES_SLOT, PASTE_ACTION, PaneProcessesWire, RELEASE_AGENT_ACTION, RENAME_PANE_ACTION,
    RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION, REPORT_AGENT_ACTION, SCREEN_COLLAPSED_SLOT,
    SCREEN_ROWS_SLOT, SELECT_WINDOW_ACTION, SESSION_SLOT, SESSIONS_SLOT, SET_FLOATING_ACTION,
    SET_LAYOUT_ACTION, SPAWN_ACTION, SPAWN_CMD_KEY, SPAWN_COLS_KEY, SPAWN_CWD_KEY, SPAWN_ROWS_KEY,
    SPLIT_ACTION, TEXT_ACTION, WINDOWS_SLOT, agent_slot_for, cells_slot_at, pane_processes_at,
    project_slot_for, recent_input_has,
};
use sprag_host::{CellFrame, mux_action_path, pane_input_path};
use sprag_input::Modifiers;
use sprag_plugin::{
    Attended, Delivered, Delivery, Driver, Guardrails, Interruption, KeyStroke, OrchestrationSpec,
    Orchestrator, OutcomeState, PaneAccess, PaneError, Reached, Readiness, ReadyWhen, RunContext,
    Written, deliver,
};
use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_BUILD_PARAM, CLIENT_HELLO_METHOD, CLIENT_PARAM,
    EVENTS_WAIT_METHOD, HostConn, PROTOCOL_FIELD, PROTOCOL_PARAM, SINCE_PARAM, WIRE_PROTOCOL,
};
use sprag_terminal::{PaneEcho, PaneEndOfInput, PaneId};

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

/// **R327 over the REAL socket: a reader whose own session was destroyed can still read the SESSION
/// LIST — and still cannot read anything about a session.**
///
/// The live path, which is the point of putting it here. `sprag_host::rpc`'s unit gate drives the
/// string entry (`handle_request`); every socket client's every request goes through `dispatch_one`
/// instead, which resolves the scope EARLY and for its own reasons. A fix that landed in one and not
/// the other would leave the product exactly as broken while looking tested — R322's *"a fix is a
/// claim until the probe is re-run"*, one layer down.
///
/// # What the two halves are worth separately
///
/// R326 measured `detach-on-destroy = no-detached` walking into a session another client was sitting
/// in. Its policy turns on each session's ATTACHED count, and at the instant it decides, the reader's
/// own session is gone: scope resolution refused every method on that connection, so the list could
/// not be re-read and the decision fell to a mirror nothing bounds the staleness of.
///
/// So the first half must ANSWER — and answer TRUTHFULLY, which is why the count is driven by a real
/// second connection that really attached, and asserted to be 1 rather than merely present. A door
/// that answered a hollow list would pass a mere `is_ok`, and `no-detached` reading `attached: 0` off
/// a hollow row is the original defect with a new cause.
///
/// The second half must still REFUSE, and it carries the same weight. That refusal is the DETACH
/// signal: a display client's poll thread reads it as *"the session I was viewing is gone"*. A build
/// that opened the door wider would trade R326's defect for a client that never notices its session
/// died, so both readings are made of the SAME connection in the same breath.
///
/// Both ways a dead scope arrives are driven, because a client meets one or the other and never
/// both: a NAME the registry no longer carries, and an ATTACHED ask from a client whose attachment
/// the kill released.
#[test]
fn a_dead_scope_still_reads_the_registry_and_still_refuses_a_session_over_the_real_socket() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
    )
    .expect("new_session answers");

    // SOMEBODY IS IN THE SURVIVOR. A real second connection that really attaches, so the count the
    // dying client is about to read is one the daemon derived rather than one this test wrote.
    let mut neighbour =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the neighbour connects");
    neighbour
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "neighbour" }))
        .expect("client/hello is accepted");
    neighbour
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");
    assert!(
        wait_until(Duration::from_secs(5), || attached_of(&mut conn, "0") == 1),
        "the neighbour must be counted before the reading below means anything",
    );

    // THE DYING CLIENT: it says hello and attaches to `work`, so BOTH ways of naming a dead scope
    // are available to it once `work` goes.
    let mut dying = HostConn::connect(&sock, Duration::from_secs(5)).expect("the dying client");
    dying
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "dying" }))
        .expect("client/hello is accepted");
    dying
        .call(CLIENT_ATTACH_METHOD, json!({ "session": "work" }))
        .expect("client/attach is accepted");
    // THE CONTROL, and it runs first: while `work` lives, this connection reads both.
    for scope in [json!({ "session": "work" }), json!({ "attached": true })] {
        for slot in [SESSIONS_SLOT, PANES_SLOT] {
            let mut params = scope.clone();
            params["path"] = json!(mux_action_path(slot));
            assert!(
                dying.call("scene/query", params).is_ok(),
                "{scope} must read {slot} while its session lives, or nothing below discriminates",
            );
        }
    }

    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(KILL_SESSION_ACTION), "args": { "name": "work" } }),
    )
    .expect("kill_session of a non-last session answers Ok");

    for scope in [json!({ "session": "work" }), json!({ "attached": true })] {
        // THE REGISTRY still answers — and answers the truth about who is sitting where.
        let mut params = scope.clone();
        params["path"] = json!(mux_action_path(SESSIONS_SLOT));
        let listed = dying
            .call("scene/query", params)
            .unwrap_or_else(|error| panic!("{scope} must still read the session list: {error}"));
        let rows = listed.as_array().expect("the sessions slot answers a list");
        let survivor = rows
            .iter()
            .find(|row| row["name"] == "0")
            .unwrap_or_else(|| panic!("the survivor must be listed: {listed}"));
        assert_eq!(
            survivor["attached"], 1,
            "and the count must be the daemon's own, or a `no-detached` client reading 0 off a \
             hollow row joins an occupied session exactly as before: {listed}",
        );
        assert_eq!(
            rows.len(),
            1,
            "the killed session is gone from it: {listed}"
        );

        // ...and a read about ONE session is still refused, on the very same connection, IN THE
        // WORDS IT WAS ALWAYS REFUSED IN. This is the detach signal, and widening it away would be
        // the opposite defect — but so would answering it in the registry surface's vocabulary: a
        // client owed *"no session named work"* and told *"unknown path"* has been given a sentence
        // about a slot in place of the fact about its session, and its poll thread classifies the
        // first. The wording is the assertion for R325's reason: a refusal a caller reads is a claim.
        let mut params = scope.clone();
        params["path"] = json!(mux_action_path(PANES_SLOT));
        let why = dying
            .call("scene/query", params)
            .expect_err("a read about a session it no longer has must be refused")
            .to_string();
        let expected = if scope["attached"] == json!(true) {
            "params.attached asks for this client's session and it is attached to none"
        } else {
            r#"no session named "work""#
        };
        assert!(
            why.contains(expected),
            "{scope} must be refused with {expected:?}, and said: {why}",
        );
    }

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
    /// ⚠⚠⚠⚠ The stand-in is LINKED from a tracked file and told its exit code through a DATA file
    /// beside it — register item 467. It used to be composed here with the code substituted in, and
    /// the DAEMON then exec'd it: a file any process holds open for writing cannot be executed, and
    /// this harness runs its cases on threads, so a sibling's fork inherits the write handle and
    /// holds it until its own exec. `exit-code` is read, never executed, so it carries none of that.
    fn new(label: &str, scp_exit: i32) -> Self {
        let dir =
            std::env::temp_dir().join(format!("sprag-drop-it-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the stand-in scp dir");
        let argv_file = dir.join("argv.txt");
        std::fs::write(dir.join("exit-code"), format!("{scp_exit}\n"))
            .expect("the status the stand-in scp is to answer with");
        sprag_gate::doubles::Doubles::of(env!("CARGO_MANIFEST_DIR"))
            .set("wire")
            .link("scp", &dir.join("scp"));

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

    // CANONICALISED: the daemon derived this root from the pane's own cwd, which the OS reports
    // RESOLVED, and macOS's `TMPDIR` is a symlink (`/var/folders/…` → `/private/var/folders/…`).
    // Comparing the path this test handed over against the one the kernel handed back compares two
    // spellings of one directory.
    assert_eq!(
        answer["root"].as_str().map(std::path::PathBuf::from),
        project.canonicalize().ok(),
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

    // ⚠⚠⚠ **A MALFORMED MEMBER GETS ITS OWN REFUSAL, AND THAT IS THE WHOLE OF R372.**
    //
    // It used to answer `Null`, and that was measured (R371d) as carrying TWO facts with opposite
    // remedies: at every parametric family the same `Null` was also what a serialisation failure
    // degraded to (`encoded_answer(..).unwrap_or(Null)`), so *fix your argument* and *this daemon
    // could not encode its own reading* reached a client as one answer it could not tell apart.
    //
    // R155 chose `Null` correctly against the API it had — `query` answered an `Option` and there
    // was no third thing to say. pinion R1667/R1674 built the third thing, and `QueryTypeMismatch`
    // is this case by its own definition (*"including an argument that is empty"*).
    let refused = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path("events.zzz") }),
        )
        .expect_err("a malformed member is REFUSED, not answered with a value");
    let refused = refused.to_string();
    assert!(
        refused.contains("QueryTypeMismatch"),
        "⚠⚠⚠ `events.zzz` is a declared family's member with an argument that is not a cursor, and \
         the caller is the one who can fix it — so the refusal has to SAY so rather than hand back \
         a value that also means this daemon failed to serialise: {refused}",
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

/// A temp directory that removes itself, plus the pieces this file's agent-launch test needs in it.
struct AgentBox(PathBuf);

impl AgentBox {
    /// A box holding a `claude` that is really the stand-in agent, an empty agent config home, and
    /// an empty sprag config home.
    ///
    /// **The agent config home is set explicitly and points at nothing.** `claude`'s own
    /// `CLAUDE_CONFIG_DIR` wins over `$HOME`, so a test that set only `HOME` would read the
    /// DEVELOPER's file whenever that variable happened to be set in their shell — and this test's
    /// whole subject is what sprag does when the user's own config does NOT report. R318/R319/R331:
    /// write the config for every process the claim passes through.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sprag-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).expect("the box's bin dir");
        std::fs::create_dir_all(dir.join("cfg")).expect("the box's sprag config dir");
        // Cargo's own answer to *where is that binary*, and the reason the stand-in is a binary of
        // THIS package. The first spelling took `CARGO_BIN_EXE_sprag-term`'s directory and joined
        // the name on, which reads the same and promises nothing: cargo builds no other package's
        // binaries for this test, so that path held a file only for whoever had run `cargo build`
        // earlier. CI had not, and this line is what it failed on.
        let peer = PathBuf::from(env!("CARGO_BIN_EXE_sprag-agent-peer"));
        // Named `claude` because the decision is made on the program's BASENAME — which is the rule
        // under test, so the fixture must go through it rather than around it. A symlink rather than
        // a `claude` binary target in the workspace: nothing named `claude` then exists in
        // `target/debug`, where it could shadow the real one for anybody who puts that on `PATH`.
        std::os::unix::fs::symlink(&peer, dir.join("bin").join("claude"))
            .expect("the stand-in agent takes the name the rule reads");
        Self(dir)
    }

    /// The daemon's environment: its `PATH` finds the stand-in, and both config homes are this box's.
    fn env(&self) -> Vec<(String, String)> {
        let path = format!(
            "{}:{}",
            self.0.join("bin").display(),
            std::env::var("PATH").unwrap_or_default(),
        );
        vec![
            ("PATH".to_owned(), path),
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                self.0.join("claude-home").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                self.0.join("cfg").display().to_string(),
            ),
        ]
    }
}

impl Drop for AgentBox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// THE WHOLE LOOP: a daemon launches an agent, the agent runs the hooks that launch handed it, and
/// the daemon answers with a turn boundary nothing sampled.
///
/// This is the SCE requirement's §5, end to end and with no step faked: a real `sprag-term`, a real
/// argv instrumented at a real pane's birth, a real program that PARSES the settings document and
/// runs what it names, the real `sprag hook claude` binary, and the real `report_agent` door.
///
/// **What makes it a proof rather than a demonstration** is the pane's screen. The stand-in paints
/// no title, no spinner and no footer — nothing any detection rule can read — so before its first
/// event this pane has NO agent key at all. Every verdict after that came from inside the pane,
/// because there is nothing outside it to have come from. A daemon that failed to instrument the
/// launch does not merely miss an assertion here: the stand-in says so on the pane's own screen.
///
/// **And the turn is one no scrape could have caught even if the screen had said something**: both
/// of its events are delivered in ONE write, so it begins and ends between two of this test's looks.
/// The state at the end is `idle`, which is what it was at the start; `seq` is what carries the fact
/// that a turn happened. That is the case `plugins::tests` measures the scrape losing.
#[test]
fn an_agent_this_daemon_launched_reports_the_turn_boundaries_it_alone_knows() {
    let agent = AgentBox::new("turn");
    // Published on sight, so the assertions below are about the report and not about a settle window.
    std::fs::create_dir_all(agent.0.join("cfg").join("sprag")).expect("the sprag config dir");
    std::fs::write(
        agent
            .0
            .join("cfg")
            .join("sprag")
            .join(sprag_host::config::CONFIG_FILE),
        "[options]\nagent-settle-time = 0\n",
    )
    .expect("write the config");

    let env = agent.env();
    let (_host, sock) = spawn_host_with(
        &["claude"],
        &env.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    let text = |conn: &mut HostConn| {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(0, FULL_TEXT_SLOT) }),
        )
        .ok()
        .and_then(|v: Value| v.as_str().map(str::to_owned))
        .unwrap_or_default()
    };

    // The agent parsed the document it was launched with. A daemon that added nothing gets
    // `agent-peer: no --settings on this launch` here, which is the diagnosis rather than a silence.
    let mut screen = String::new();
    let ready = wait_until(Duration::from_secs(10), || {
        screen = text(&mut conn);
        screen.contains("agent-peer ready")
    });
    assert!(
        ready,
        "the launched agent never read its settings; the pane said: {screen:?}",
    );

    // THE CONTROL: this pane's screen tells a scrape nothing, so it has no agent state at all.
    let before = pane_entry(&mut conn, 0);
    assert!(
        before.get("agent").is_none(),
        "nothing on this screen is an agent to any rule, which is what makes the rest attributable: \
         {before}",
    );

    // A whole turn in ONE write: it begins and ends before anything looks.
    let sent: Value = conn
        .call(
            "scene/invoke",
            json!({
                "path": pane_input_path(0, TEXT_ACTION),
                "args": { "text": "UserPromptSubmit\nStop\n" },
            }),
        )
        .expect("the events reach the agent's stdin");
    assert!(
        sent.is_null() || sent.is_object(),
        "a well-formed reply: {sent}"
    );

    let mut screen = String::new();
    let done = wait_until(Duration::from_secs(10), || {
        screen = text(&mut conn);
        screen.contains("Stop done")
    });
    assert!(
        done,
        "the agent never finished the turn; the pane said: {screen:?}",
    );
    assert!(
        screen.contains("UserPromptSubmit done (1)"),
        "and the document named exactly one hook for the turn's start: {screen:?}",
    );

    // The daemon knows a turn happened, and knows it from inside the pane.
    let after = wait_until(Duration::from_secs(5), || {
        pane_entry(&mut conn, 0)["agent"]["state"].as_str() == Some("idle")
    });
    let entry = pane_entry(&mut conn, 0);
    assert!(after, "the daemon never took the agent's report: {entry}");
    assert_eq!(
        entry["agent"]["name"], "claude",
        "the report names which agent it is: {entry}",
    );
    assert!(
        entry["agent"]["source"].is_string() && entry["agent"]["rule"].is_null(),
        "a REPORTED verdict names its reporter and no rule: {entry}",
    );
    assert_eq!(
        entry["agent"]["seq"], 2,
        "both edges of the turn were published, though the state ends where it began: {entry}",
    );
}

/// Run the REAL `sprag` CLI against `sock`, feeding it `input`, with `envs` on top.
///
/// ⚠⚠ `XDG_STATE_HOME` is redirected because `hook` is the verb that WRITES: `note_hook_trouble`
/// files `sprag/hook-mute.<pane>` whenever a report could not be delivered, and without a state
/// home of its own that lands in the runner's real `~/.local/state` — the residue CI's
/// `ambient-home-guard` reported in register item 464, bisected there rather than reasoned.
///
/// ⚠ Fed through [`sprag_gate::feeding`] (register item 471): this CLI can refuse BEFORE it reads
/// its payload, and a fixture that treated the resulting `EPIPE` as fatal would report a write
/// failure where the exit status it came for is the answer.
fn sprag_cli(sock: &std::path::Path, args: &[&str], envs: &[(&str, &str)], input: &str) -> bool {
    sprag_cli_output(sock, args, envs, input).0
}

/// [`sprag_cli`] plus WHAT IT PRINTED — for the gates whose subject is the answer a person reads,
/// not the exit status.
///
/// Separate rather than folded in because the two ask different questions of the same run, and a
/// caller that only wants the status must not have to name a variable for prose it will not read.
fn sprag_cli_output(
    sock: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &str)],
    input: &str,
) -> (bool, String) {
    let state = sock.with_extension("state");
    std::fs::create_dir_all(&state).expect("a state home of this test's own");
    let mut child = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(args)
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("XDG_STATE_HOME", &state)
        // ⚠ The pane THIS suite's runner is itself in must not leak into the CLI it drives —
        // register item 226. The debt-repayment loop's own agent runs in a sprag pane, so a test
        // that inherited `SPRAG_PANE` would report to a daemon that has never heard of it. Before
        // `envs`, so a caller that WANTS a pane still sets one and wins.
        .env_remove(sprag_host::PANE_ENV_VAR)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the sprag CLI");
    sprag_gate::feeding::feed(&mut child, input.as_bytes());
    let out = child.wait_with_output().expect("wait for the sprag CLI");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// ⚠⚠⚠⚠⚠ **THE HOOK SAYS WHICH BUILD IT IS, ON THE SAME TERMS A PERSON'S `report-agent` DOES** —
/// register item 459, at the door the whole key was written for.
///
/// # ⚠⚠⚠⚠ Why this exists when two gates already carry `build`
///
/// Both of them supply the very field the product was omitting.
/// `agent::tests::a_reporter_says_which_build_it_is_and_silence_is_never_agreement` hands the
/// REGISTRY a `Report { build: Some(..) }` by hand, and
/// `workspace::tests::a_reporters_build_reaches_the_pane_row` invokes `report_agent` with the key
/// already in the params. Each proves its own layer carries a build **somebody sends** — and
/// nobody did. [`sprag_host::wire::AGENT_BUILD_KEY`]'s entire argument is about the HOOK ("a
/// `cargo build` replaces the reporter under a running daemon … the ORDINARY state after any
/// rebuild"), and the hook was the one reporter in the tree that never stated it: `deliver_hook`
/// built its params with `id`/`state`/`source`/`name`/`seq`/`asked`/`said`/`transcript`/`bind` and
/// no `build`, so `reporter_build` was `None` — *this reporter did not say* — for every report
/// production has ever made. Item 412's quiet skew had no detector at all, and a gate whose
/// fixture supplies the omitted field cannot see the omission (register item 428's shape).
///
/// So this one sends NOTHING by hand. It runs the real `sprag hook claude` binary on a real
/// daemon's socket and reads the answer off the pane row a client reads.
///
/// # The two halves, and why neither is the other's decoration
///
/// * **The hook's**, on the boot pane. Its CONTROL is that a `cat` pane has no agent key at all
///   before the hook runs, so every word of the row afterwards came from that process.
/// * **A person's `report-agent`**, on a second pane, which is the *"same terms"* half of the
///   item's done-when — and not a tautology: it is also the first gate anywhere that the VERB
///   states a build over a real socket. Without it a green above could be produced by a wire that
///   defaulted the key, and the cross-check (`the two reporters agree`) is what refutes that.
///
/// ⚠ [`sprag_rpc::BUILD`] is the oracle because every binary linking that crate is stamped with the
/// SAME value, so within one `cargo build` the hook, the daemon and this test are one image — the
/// property the const's own doc states, and the reason a mismatch here means a reporter that is
/// not this image rather than a flaky fixture.
#[test]
fn the_hook_states_which_build_reported_on_the_same_terms_a_person_does() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // THE CONTROL: `cat` paints nothing any rule reads, so this pane has no agent key whatever.
    let before = pane_entry(&mut conn, 0);
    assert!(
        before.get("agent").is_none(),
        "nothing on this screen is an agent to any rule, which is what makes the rest \
         attributable: {before}",
    );

    // A second pane for the person's half, so the two reporters never overwrite each other and a
    // `None` on one cannot be hidden by a `Some` on the other.
    let second = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a 2nd pane over the wire")
        .as_u64()
        .expect("spawn returns the new pane id");

    // ── The hook, as an agent's session runs it: the real binary, its payload on stdin. ──
    assert!(
        sprag_cli(
            &sock,
            &["hook", "claude"],
            &[(sprag_host::PANE_ENV_VAR, "0")],
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#,
        ),
        "the hook binary succeeded",
    );
    let reported = wait_until(Duration::from_secs(5), || {
        pane_entry(&mut conn, 0)["agent"]["source"].as_str() == Some("hook:claude")
    });
    let hooked = pane_entry(&mut conn, 0);
    assert!(reported, "the hook never reached the daemon: {hooked}");
    assert_eq!(
        hooked["agent"][sprag_host::wire::AGENT_BUILD_KEY],
        json!(sprag_rpc::BUILD),
        "⚠⚠⚠⚠⚠ THE HOOK MUST SAY WHICH BUILD IT IS. It is the reporter `AGENT_BUILD_KEY` was \
         written for — the one a `cargo build` replaces under a running daemon — and an absent key \
         means *this reporter did not say*, never *it matches*. With it absent the quiet skew of \
         register item 412 (reports accepted, reporter running code the daemon has never seen) is \
         undetectable by construction: {hooked}",
    );

    // ── A person at a command line, on the terms the item names. ──
    assert!(
        sprag_cli(
            &sock,
            &["report-agent", "working", "--pane", &second.to_string()],
            &[],
            "",
        ),
        "the report-agent verb succeeded",
    );
    let by_hand = pane_entry(&mut conn, second);
    assert_eq!(
        by_hand["agent"][sprag_host::wire::AGENT_BUILD_KEY],
        json!(sprag_rpc::BUILD),
        "a person's report states its build over a real socket too — the terms the hook is held \
         to, measured rather than assumed: {by_hand}",
    );
    assert_eq!(
        hooked["agent"][sprag_host::wire::AGENT_BUILD_KEY],
        by_hand["agent"][sprag_host::wire::AGENT_BUILD_KEY],
        "and the two reporters of one image agree, which is what makes a DISAGREEMENT mean \
         something: {hooked} / {by_hand}",
    );
}

/// A DAEMON WHOSE BUILD IS NOT THE ONE IT WAS BUILT FROM — a real `sprag-term` reached through a
/// relay that changes exactly one fact about it.
///
/// # ⚠⚠⚠⚠⚠ Why this is needed at all: one `cargo build` cannot produce the skew it gates
///
/// [`sprag_rpc::BUILD`] is stamped INTO each image at compile time from `HEAD`, and every binary
/// linking that crate gets the SAME stamp — which is the property that makes the field mean
/// anything, and the property that makes *"a hook that is not this daemon's image"* impossible to
/// stage inside one build. The two honest-looking ways out are both worse: compiling a second image
/// inside the suite is the defect register item 467 spent a round removing (a test that writes the
/// program it runs), and an env override on the stamp would hand every process a way to claim a
/// build it is not — the exact lie `crates/sprag-rpc/build.rs` refuses a dirty-tree flag over.
///
/// So the DAEMON is made to differ instead, and by the one means that leaves every other byte
/// alone: the peer relays the whole conversation and substitutes `build` in the daemon's own reply.
/// The daemon is real, its panes are real, the reporter is the real hook binary stating its real
/// stamp — the only invention is the fact a second compile would otherwise have to supply.
///
/// ⚠ It rewrites the DAEMON's half deliberately, never the reporter's: only the TOP LEVEL of a
/// `result` object is edited ([`sprag_peer::Missing::answering`]), and a pane listing's result is an
/// ARRAY, so the reporter's own `build` in a pane row cannot be forged by this.
///
/// ⚠⚠ **It is [`sprag_peer`]'s peer rather than a relay written here**, which is that crate's whole
/// argument: four suites had each hand-written a stand-in daemon and drifted into three different
/// meanings of *"older"*. A fifth relay living in one test file would have been the same mistake,
/// and it is what item 474 needed to borrow — the agent-facing mouth is gated in another package
/// and cannot reach a fixture private to this one.
fn skewed_daemon(real: &std::path::Path, build: &str) -> sprag_peer::OldDaemon {
    sprag_peer::OldDaemon::proxying(
        &socket_path(),
        real,
        sprag_peer::Missing::answering(&[(sprag_rpc::BUILD_FIELD, json!(build))]),
    )
}

/// ⚠⚠⚠⚠⚠ **A PERSON IS TOLD WHETHER THE REPORTER THAT ANSWERED IS THIS DAEMON'S IMAGE** — register
/// item 473, the QUIET half of the hazard whose loud half has had a sentence here since item 344.
///
/// # What was wrong, and why a passing wire made it invisible
///
/// One round earlier the hook began stating its build ([`sprag_host::wire::AGENT_BUILD_KEY`], the
/// gate above), so the fact reached the pane row. **No surface a person reads rendered it.** `sprag
/// agent <pane>` printed `state / name / origin / seq / asked / said` and stopped, so item 412's
/// skew — *the numbers agree, the reports are accepted, and the reporter is running code the daemon
/// has never seen* — was legible to a wire client and to nobody else. Its loud sibling (a reporter
/// that can no longer deliver) prints *"⚠ THAT REPORTER IS MUTE"* three lines up; this one printed
/// nothing at all, which is the asymmetry the item was filed for.
///
/// # Three answers, and each one is staged by a different party
///
/// * **IS this image** — put there by the REAL `sprag hook claude` against the real daemon, so the
///   build in the row is one process's true stamp compared against another's.
/// * **is NOT** — the SAME report, read back through a daemon whose stated build differs
///   ([`skewed_daemon`], whose doc argues why no cheaper fixture is honest). Nothing about the
///   reporter changes between this arm and the one above; only the daemon does, which is what makes
///   the two sentences attributable to the comparison rather than to the report.
/// * **DID NOT SAY** — a report that OMITS the key, which is the exact wire every hook older than
///   the round above sends. An omission is not a hand-set field: there is no value here to get
///   wrong, and `None` must never render as agreement.
///
/// ⚠ The control is the same one the gate above uses and for the same reason: before any reporter
/// speaks, a `cat` pane carries no agent key whatever, so every word read afterwards is attributable
/// to a process this test ran.
#[test]
fn a_person_is_told_whether_the_reporter_that_answered_is_this_daemons_image() {
    /// A build no image in this tree can be. Twelve hex digits, the shape `build.rs` stamps, so it
    /// is refused for what it SAYS and not for how it is spelled.
    const NOT_THIS_IMAGE: &str = "0000deadbeef";

    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    let before = pane_entry(&mut conn, 0);
    assert!(
        before.get("agent").is_none(),
        "nothing on this screen is an agent to any rule, which is what makes the rest \
         attributable: {before}",
    );

    // ── The reporter the whole key was written for, running for real. ──
    assert!(
        sprag_cli(
            &sock,
            &["hook", "claude"],
            &[(sprag_host::PANE_ENV_VAR, "0")],
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#,
        ),
        "the hook binary succeeded",
    );
    let reported = wait_until(Duration::from_secs(5), || {
        pane_entry(&mut conn, 0)["agent"]["source"].as_str() == Some("hook:claude")
    });
    let hooked = pane_entry(&mut conn, 0);
    assert!(reported, "the hook never reached the daemon: {hooked}");

    // ── ARM 1: the reporter IS this daemon's image, and the surface says so. ──
    let (ok, own) = sprag_cli_output(&sock, &["agent", "0"], &[], "");
    assert!(ok, "`sprag agent 0` succeeded: {own:?}");
    assert!(
        own.contains("this daemon's own image") && own.contains(sprag_rpc::BUILD),
        "⚠⚠⚠⚠⚠ A PERSON MUST BE ABLE TO READ THIS. The hook stated its build and the daemon holds \
         both halves, so the surface that already says whether a reporter can SPEAK must say \
         whether it is this daemon's CODE — until it did, item 412's quiet skew was visible to a \
         wire client and to no one else: {own:?}",
    );

    // ── ARM 2: the same report, read against a daemon built from other code. ──
    let skewed = skewed_daemon(&sock, NOT_THIS_IMAGE);
    let (ok, skew) = sprag_cli_output(skewed.sock(), &["agent", "0"], &[], "");
    assert!(ok, "`sprag agent 0` succeeded through the relay: {skew:?}");
    assert!(
        skew.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠⚠ THIS IS THE WHOLE HAZARD: a verdict that outranks the screen, produced by code this \
         daemon has never run. A rebuild replaces the hook under every live daemon at once, so \
         this is the ORDINARY state after one — and a surface that stays quiet here leaves the \
         reader believing the report: {skew:?}",
    );
    assert!(
        skew.contains(sprag_rpc::BUILD) && skew.contains(NOT_THIS_IMAGE),
        "⚠⚠⚠ and it names BOTH builds — one of them alone tells a reader nothing about which is \
         which: {skew:?}",
    );
    assert!(
        !skew.contains("own image"),
        "a reporter that is not this daemon's image must not also be called one: {skew:?}",
    );

    // ── ARM 3: a reporter older than the key, which says nothing at all. ──
    let silent = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a 2nd pane over the wire")
        .as_u64()
        .expect("spawn returns the new pane id");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(REPORT_AGENT_ACTION),
            "args": { "id": silent, "state": "working", "source": "hook:claude" },
        }),
    )
    .expect("a report with no build at all — every hook older than this key");
    let quiet = pane_entry(&mut conn, silent);
    assert!(
        quiet["agent"][sprag_host::wire::AGENT_BUILD_KEY].is_null(),
        "the fixture's premise: this reporter said nothing about its build: {quiet}",
    );
    let (ok, unsaid) = sprag_cli_output(&sock, &["agent", &silent.to_string()], &[], "");
    assert!(ok, "`sprag agent {silent}` succeeded: {unsaid:?}");
    assert!(
        unsaid.contains("did not say"),
        "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE, and this is the arm a tidy-looking edit \
         folds into the first. Silence here would convert *nobody knows* into *nothing is wrong* — \
         the exact inversion `AGENT_BUILD_KEY` exists to end: {unsaid:?}",
    );
    assert!(
        !unsaid.contains("own image") && !unsaid.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠ three answers stay three: a reporter that did not say is neither of the other two: \
         {unsaid:?}",
    );
}

/// ⚠⚠⚠⚠⚠ **A PERSON IS TOLD WHICH OF THE WINDOWS ON THEIR SCREEN IS THIS DAEMON'S BUILD** —
/// register item 463, end to end, over the real socket and through the real CLI.
///
/// # The companion this daemon does NOT resolve
///
/// `host.rs` resolves the hook and the MCP server as siblings of the running executable, so a
/// daemon cannot hand its agents a reporter from another build. **There is no such rule for the
/// display client**, and there cannot be a complete one: a `sprag-gui` is a process a person starts,
/// from whatever directory they point at. This repository's own promotion procedure does exactly
/// the thing that produces the skew — it copies the daemon into one directory and then launches
/// `target/debug/sprag-gui` — so *the window is a different build from the daemon it drives* is the
/// ordinary state here, and until this round nothing anywhere could say it.
///
/// The owner raised it the moment it bit, with an experimental window driving a daemon built from
/// other code. **The answer is a report and never a refusal**, which is `sprag_rpc::BUILD_FIELD`'s
/// standing ruling one direction over: a GUI thrown out of the door on a build difference would
/// take a person's windows with it, every rebuild.
///
/// # Four states, and each is staged by a different party
///
/// * **NOBODY ATTACHED** — the control, read before any client exists, so every word afterwards is
///   attributable to a connection this test made.
/// * **IS this build** — a client that went through the product's OWN seam
///   ([`HostConn::handshake`]), which is what puts [`sprag_rpc::CLIENT_BUILD_PARAM`] on the wire.
///   Nothing here hand-writes the happy case, so a fix that only ever worked in a fixture fails.
/// * **is NOT** — a hand-written hello naming another build, because **one `cargo build` cannot
///   manufacture two images** (register item 474 paid a round to learn that): every binary this
///   tree produces carries the same stamp, so the foreign one has to be SAID.
/// * **DID NOT SAY** — a hello with no build key at all, which is the exact wire every client older
///   than this round sends. An omission is not a hand-set value: there is nothing here to get
///   wrong, and it must never render as agreement.
#[test]
fn a_person_is_told_which_of_the_windows_on_their_screen_is_this_daemons_build() {
    /// A build no image in this tree can be. Twelve hex digits, the shape `build.rs` stamps, so it
    /// is judged for what it SAYS and not for how it is spelled.
    const NOT_THIS_IMAGE: &str = "0000deadbeef";

    let (_host, sock) = spawn_host();

    // ── THE CONTROL: no window exists yet, and that is its own answer ──
    let (ok, alone) = sprag_cli_output(&sock, &["doctor"], &[], "");
    assert!(
        ok,
        "`sprag doctor` succeeded against a fresh daemon: {alone:?}"
    );
    assert!(
        alone.contains("no client is attached"),
        "⚠⚠⚠ zero windows compared must not read as zero problems found — and this is the line \
         every assertion below is a change FROM: {alone:?}",
    );

    // ── THE PRODUCT'S OWN SEAM states this image; nothing here writes the happy case by hand ──
    let mut current =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the current window connects");
    current
        .handshake("gui-current")
        .expect("the real handshake is accepted");
    current
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");

    // ── THE WINDOW STARTED FROM SOMEWHERE ELSE ──
    let mut foreign =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the foreign window connects");
    foreign
        .call(
            CLIENT_HELLO_METHOD,
            json!({ CLIENT_PARAM: "gui-foreign", CLIENT_BUILD_PARAM: NOT_THIS_IMAGE }),
        )
        .expect("client/hello is accepted");
    foreign
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");

    // ── AND A CLIENT OLDER THAN THE KEY, which says nothing at all ──
    let mut quiet =
        HostConn::connect(&sock, Duration::from_secs(5)).expect("the quiet window connects");
    quiet
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "tui-quiet" }))
        .expect("a hello with no build — every client older than this key");
    quiet
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");

    let (ok, report) = sprag_cli_output(&sock, &["doctor"], &[], "");
    assert!(
        ok,
        "`sprag doctor` succeeded with three windows: {report:?}"
    );
    assert!(
        report.contains("3 attached client(s)") && report.contains("1 on the daemon's build"),
        "⚠⚠⚠⚠ SILENCE HAS TO BE EARNED: a reader cannot tell *every window was checked* from \
         *nobody looked* unless the count says so, and this surface is read precisely when \
         somebody already suspects the answer: {report:?}",
    );

    let named = |who: &str| {
        report
            .lines()
            .find(|line| line.contains(who))
            .unwrap_or_else(|| panic!("no line of the report names {who}: {report:?}"))
            .to_owned()
    };
    let skew = named("gui-foreign");
    assert!(
        skew.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠⚠⚠ THIS IS THE WHOLE HAZARD: the window a person is looking at draws from code this \
         daemon has never run, and every key they press is that build's behaviour: {skew:?}",
    );
    assert!(
        skew.contains(NOT_THIS_IMAGE) && skew.contains(sprag_rpc::BUILD),
        "⚠⚠⚠ and the finding names BOTH builds on its own line — one of them alone tells a reader \
         nothing about which is which: {skew:?}",
    );

    let unsaid = named("tui-quiet");
    assert!(
        report.contains("1 did not say") && !unsaid.contains("NOT THIS DAEMON'S IMAGE"),
        "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE, and it is not a skew either. Every client \
         older than this key answers exactly this silence, so counting it either way is the \
         inversion the key exists to end: {unsaid:?}",
    );

    assert!(
        !report.contains("gui-current"),
        "⚠⚠ the window that IS this daemon's build is counted and not named — a finding per \
         healthy row is how a report stops being read: {report:?}",
    );
}

/// **AN AGENT THIS DAEMON LAUNCHED TALKS TO THE MCP SERVER OF THE IMAGE THAT MADE ITS PANE** —
/// register item 444, end to end, with no step faked and no install anywhere.
///
/// A real `sprag-term`, a real pane birth, the real per-launch decision, the real `sprag-mcp`
/// sitting beside that daemon, and a program that actually SPAWNS what the document names and
/// speaks JSON-RPC to it. The item's whole complaint was a server nobody could date: the machine it
/// was measured on served an agent-facing roster three weeks behind the tree, from user scope, with
/// nothing anywhere able to say so.
///
/// # ⚠⚠⚠⚠ Two readers, marked in the same breath, because either alone is a complete and wrong story
///
/// * **The OPERATING SYSTEM's answer** — the pane's foreground job argv, straight out of
///   `/proc/<pid>/cmdline`. It says what the daemon actually put on that command line, which is the
///   only reading that can say the injected server is the SIBLING BY ABSOLUTE PATH rather than a
///   name `PATH` would resolve to whatever is installed. A screen cannot tell those apart.
/// * **The PANE's own screen** — what came back when the agent started that server and asked it
///   what it was. It says the document names something an agent can actually run, which an argv
///   cannot: a path to a file that does not exist, or a document nested one level wrong, produces
///   exactly the same command line.
///
/// The build is what ties the two together. [`sprag_rpc::BUILD`] is stamped into an image when it
/// is compiled, so a server whose `serverInfo` carries THIS test's build is the one built from this
/// tree — and the stale copy on the machine this was written on (which is still installed, and
/// still first on nobody's `PATH` by accident) carries another.
///
/// ⚠ [`sprag_gate::sibling_bin`] rather than joining the name onto a directory: `cargo test -p
/// sprag-host` does not build another package's binaries, and a gate that drove whatever an earlier
/// build had left there would pass while saying nothing. It refuses when it cannot tell.
#[test]
fn an_agent_this_daemon_launched_talks_to_the_mcp_server_of_the_image_that_made_its_pane() {
    let server = sprag_gate::sibling_bin(env!("CARGO_BIN_EXE_sprag-term"), "sprag-mcp");
    let agent = AgentBox::new("mcp");
    let env = agent.env();
    let (_host, sock) = spawn_host_with(
        &["claude"],
        &env.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // ⚠ THE LOGICAL LINES THE CHILD WROTE, not the rows a 40-column pane broke them into — see
    // `FULL_LINES_SLOT`. The width belongs to whoever attached, and a `contains` over wrapped rows
    // would be a gate about this harness's pane size.
    let lines = |conn: &mut HostConn| -> Vec<String> {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(0, FULL_LINES_SLOT) }),
        )
        .ok()
        .and_then(|value: Value| serde_json::from_value::<Vec<String>>(value).ok())
        .unwrap_or_default()
    };

    // ── READER ONE: the pane's screen. The agent started the server it was handed and asked it
    // what it is. A daemon that injected nothing gets `agent-peer mcp: no --mcp-config on this
    // launch` here, which is the diagnosis rather than a silence.
    let mut said = Vec::new();
    let answered = wait_until(Duration::from_secs(30), || {
        said = lines(&mut conn);
        said.iter().any(|line| line.starts_with("agent-peer mcp"))
    });
    assert!(
        answered,
        "the launched agent never reported on an MCP server; the pane said: {said:?}",
    );
    let report = said
        .iter()
        .find(|line| line.starts_with("agent-peer mcp"))
        .expect("the line the wait above found");
    // The package version is this workspace's one version, so the test's own is the server's.
    let expected = format!(
        "agent-peer mcp {} version={}+{} tools=",
        sprag_host::hooks::MCP_SERVER,
        env!("CARGO_PKG_VERSION"),
        sprag_rpc::BUILD,
    );
    assert!(
        report.starts_with(&expected),
        "⚠⚠⚠ the agent reached a server that is not this image. Wanted a line opening {expected:?}, \
         the pane said {report:?}",
    );
    // ⚠ A LOWER BOUND, never a count: what the roster holds is the product's to say (and there is a
    // gate in `sprag-mcp` holding it against the vocabulary). What this asserts is that the server
    // answered `tools/list` with a roster at all — without it, a server that completed the handshake
    // and then went silent would read as a working one.
    let tools: usize = report[expected.len()..]
        .parse()
        .unwrap_or_else(|why| panic!("the roster size is a number ({why}): {report:?}"));
    assert!(tools > 0, "the server served a roster: {report:?}");

    // ── READER TWO: the operating system. What the daemon actually put on that command line.
    let id = pane_entry(&mut conn, 0)["id"]
        .as_u64()
        .expect("the boot pane is listed with an id");
    let reading: PaneProcessesWire = serde_json::from_value(
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(&pane_processes_at(0)) }),
        )
        .expect("the processes reading"),
    )
    .expect("the processes reading parses");
    let argv = reading
        .panes
        .iter()
        .find(|row| row.id == id)
        .and_then(|row| row.foreground.as_ref())
        .map(|job| {
            job.processes
                .iter()
                .flat_map(|process| process.argv.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let at = argv
        .iter()
        .position(|arg| arg == "--mcp-config")
        .unwrap_or_else(|| panic!("the pane's own job carries the flag: {argv:?}"));
    let document: Value = serde_json::from_str(&argv[at + 1])
        .unwrap_or_else(|why| panic!("the value beside the flag is JSON ({why}): {argv:?}"));
    let named = document["mcpServers"][sprag_host::hooks::MCP_SERVER]["command"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("the entry names a command: {document}"));
    // ⚠⚠⚠ ABSOLUTE is the half a `PATH` lookup fails, and it is asserted on the STRING the daemon
    // wrote — before any resolution, because canonicalising would make a bare name absolute and
    // hide exactly the defect this exists to catch.
    assert!(
        named.is_absolute(),
        "⚠⚠⚠⚠ the server is named by absolute path — a bare name is resolved on PATH, which is how \
         an agent came to be talking to an image three weeks behind the tree: {document}",
    );
    // ⚠⚠ And THE SAME FILE, compared as a file rather than as a string: this repository's `target`
    // is a symlink to a build cache, so the daemon's own `/proc/self/exe` spells the sibling
    // resolved while cargo hands this test the link. Two spellings, one inode — a string comparison
    // here was red on a correctly injected launch.
    assert_eq!(
        std::fs::canonicalize(&named).ok(),
        std::fs::canonicalize(&server).ok(),
        "⚠⚠⚠⚠ the server named on the launch is the SIBLING OF THIS DAEMON: {document}",
    );
    assert!(
        !argv.iter().any(|arg| arg == "--strict-mcp-config"),
        "⚠⚠⚠ sprag ADDS a server; it never says «only mine», which would delete every server this \
         agent's user configured: {argv:?}",
    );
}

/// THE SAME LOOP AGAINST THE REAL `claude`, which is the only thing that can say the document sprag
/// writes is one the agent it was written for actually honours.
///
/// `#[ignore]`d, for the reason `drives_real_claude` is: it needs a logged-in `claude` on `PATH` and
/// it reaches Anthropic's API, so no CI job can run it. What it buys over the hermetic twin above is
/// the one thing a stand-in cannot: the stand-in was written from the same understanding of
/// `--settings` as the producer, and only the real agent can refute that understanding.
///
/// It runs the agent in PRINT mode with a trivial prompt — the shortest real turn available — and
/// asserts a report arrived NAMING sprag's own hook. Which state it names is deliberately not
/// asserted: print mode ends by exiting, so `Stop` is followed by `SessionEnd`, which RELEASES the
/// report, and a test that waited for one particular edge would be racing the agent's own teardown.
/// What is not racy is that a real `claude`, launched by this daemon, ran the command in the
/// document this daemon appended to its argv. The turn's two edges are asserted in the hermetic
/// twin, where the agent's lifetime is the test's to control.
///
/// **Its precondition is asserted rather than assumed.** If this machine's own `claude` config
/// already carries sprag's hooks, sprag deliberately adds nothing to the launch
/// ([`sprag_host::hooks::launch_args`]) and this test would be watching `install-hooks` work. Run
/// `sprag uninstall-hooks claude` first; the assertion says so.
#[test]
#[ignore = "needs a logged-in `claude` on PATH and reaches Anthropic's API"]
fn a_real_claude_this_daemon_launched_reports_its_own_turn() {
    let status =
        sprag_host::hooks::status(&sprag_host::hooks::CLAUDE).expect("a claude config path");
    assert!(
        !status.reporting(),
        "this machine's own claude config already reports, so sprag adds nothing to a launch and \
         this test would be watching the wrong mechanism. Run `sprag uninstall-hooks claude` \
         first: {}",
        status.path.display(),
    );

    // Its own sprag config home (`agent-settle-time = 0`), and the machine's own claude config —
    // this test needs the real agent's credentials, which is exactly what the hermetic twin does not.
    let dir = std::env::temp_dir().join(format!("sprag-real-claude-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("the config dir");
    std::fs::write(
        dir.join("sprag").join(sprag_host::config::CONFIG_FILE),
        "[options]\nagent-settle-time = 0\n",
    )
    .expect("write the config");

    let (_host, sock) = spawn_host_with(
        &["claude", "-p", "reply with the single word: ok"],
        &[("XDG_CONFIG_HOME", &dir.display().to_string())],
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // A real turn takes real time; the bound is generous because what is under test is WHETHER the
    // report arrives, not how fast.
    let mut entry = Value::Null;
    let reported = wait_until(Duration::from_secs(120), || {
        entry = pane_entry(&mut conn, 0);
        entry["agent"]["source"].is_string()
    });
    assert!(
        reported,
        "the real claude never reported through the settings sprag launched it with: {entry}",
    );
    assert_eq!(entry["agent"]["name"], "claude", "{entry}");
    assert_eq!(
        entry["agent"]["source"], "hook:claude",
        "the reporter is the hook sprag's own document named: {entry}",
    );
    assert!(
        entry["agent"]["rule"].is_null(),
        "a reported verdict carries no rule, so nothing here was read off the screen: {entry}",
    );
    let _ = std::fs::remove_dir_all(&dir);
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

/// ⚠⚠ **A CLIENT CAN DRIVE A PANE FROM THE PUBLISHED GRAMMAR AND NOTHING ELSE** — the whole feature,
/// end to end, over a real socket against a real daemon.
///
/// # Why this test exists when four in-crate gates already drive the same table
///
/// Those gates call `invoke` on the surface directly, which means they choose the `IntrospectValue`
/// themselves. **A published form says which SHAPE the `args` value is, and the mapping from JSON to
/// that shape is pinion's, not sprag's** — so a gate that builds `IntrospectValue::Text` by hand is
/// asserting about a conversion it performed. R351's rule: an instrument is a claim. Here the JSON
/// goes down a socket and pinion's `json_to_introspect_value` does the converting, so a scalar form
/// that is only reachable by a shape no client can send would fail HERE and nowhere else.
///
/// The test is written the way an agent would work: read `action_grammar` off the pane's own address,
/// pick a form, fill it from the declaration, send it, and read the pane to see the keystroke arrive.
/// Nothing in it names an argument that did not come out of the answer.
#[test]
fn a_client_can_drive_a_pane_from_its_published_grammar() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned sprag-term host");

    let panes: Value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers");
    let pane = panes[0]["id"].as_u64().expect("the boot pane has an id");

    // THE ONE READ AN AGENT STARTS FROM — on the PANE's surface, which is the address whose verbs it
    // describes. The multiplexer's own answer is a different table (its twenty-five verbs), and
    // asking the wrong surface for the other's grammar is the confusion a single global table would
    // have invited.
    let grammar: Value = conn
        .call(
            "scene/query",
            json!({ "path": pane_input_path(pane, ACTION_GRAMMAR_SLOT) }),
        )
        .expect("a pane publishes how to call its own verbs");
    let verbs = grammar.as_object().expect("the slot answers an object");
    let mut published: Vec<&str> = verbs.keys().map(String::as_str).collect();
    published.sort_unstable();
    assert_eq!(
        published,
        [
            "clipboard_answer",
            "focus",
            "inject",
            "key",
            "mouse",
            "paste",
            "text"
        ],
        "the verbs a pane's input surface serves, over the real wire — the display client's, plus \
         the run driver's own `inject` (register item 544): {grammar}",
    );

    // ── The SCALAR form, which is the half no in-crate gate can prove ────────────────────────────
    let text_forms = verbs["text"].as_array().expect("text answers its forms");
    let scalar = text_forms
        .iter()
        .find(|form| form[CallForm::FORM_KEY] == json!("scalar"))
        .expect("`text` publishes a scalar form");
    assert_eq!(
        scalar[CallForm::ARGS_KEY]
            .as_array()
            .expect("a form answers its arguments")
            .len(),
        1,
        "a scalar form's one argument IS the whole args value",
    );
    conn.call(
        "scene/invoke",
        // The `args` is the BARE VALUE, exactly as the published form says. pinion turns this JSON
        // string into the shape the surface reads; if that mapping were anything else this call would
        // be refused and the assertion below would never see the text.
        json!({ "path": pane_input_path(pane, TEXT_ACTION), "args": "from-the-scalar-form" }),
    )
    .expect("the scalar form is a call this daemon reads");

    // ── The OBJECT form of `key`, with a word taken from its published vocabulary ────────────────
    let key_object = verbs["key"]
        .as_array()
        .expect("key answers its forms")
        .iter()
        .find(|form| form[CallForm::FORM_KEY] == json!("object"))
        .expect("`key` publishes an object form")
        .clone();
    let args = key_object[CallForm::ARGS_KEY]
        .as_array()
        .expect("a form answers its arguments");
    let edge = args
        .iter()
        .find(|arg| arg[ArgGrammar::NAME_KEY] == json!("state"))
        .expect("`key`'s object form declares the edge")[ArgGrammar::ONE_OF_KEY][0]
        .as_str()
        .expect("the edge publishes its vocabulary")
        .to_owned();
    assert_eq!(
        edge, "down",
        "the FIRST published edge is the one that injects — a client filling a form takes it",
    );
    // Built from the answer: the key name argument by its published NAME, the edge by its published
    // WORD. Nothing here is a string this test knew in advance except the keystroke itself.
    let name_of_key = args[0][ArgGrammar::NAME_KEY]
        .as_str()
        .expect("the first argument is named")
        .to_owned();
    conn.call(
        "scene/invoke",
        json!({
            "path": pane_input_path(pane, KEY_ACTION),
            "args": { name_of_key: "!", "state": edge },
        }),
    )
    .expect("the object form is a call this daemon reads");

    // The pane is a `cat`, so what reached the child echoes back — the proof that a grammar-built
    // call did not merely parse but ARRIVED.
    assert!(
        wait_until(Duration::from_secs(5), || {
            conn.call(
                "scene/query",
                json!({ "path": pane_input_path(pane, FULL_TEXT_SLOT) }),
            )
            .ok()
            .and_then(|value| {
                value
                    .as_str()
                    .map(|text| text.contains("from-the-scalar-form!"))
            })
            .unwrap_or(false)
        }),
        "a call built from the published grammar never reached the child",
    );

    // ⚠ THE CONTROL: a word the vocabulary does NOT hold is refused, so the published list is a
    // constraint the daemon enforces rather than documentation beside it. Without this the test would
    // pass against a daemon that accepted any `state` at all, and the vocabulary would be decoration.
    assert!(
        conn.call(
            "scene/invoke",
            json!({
                "path": pane_input_path(pane, KEY_ACTION),
                "args": { "key": "!", "state": "sideways" },
            }),
        )
        .is_err(),
        "a `state` outside the published vocabulary must be refused",
    );
}

/// ⛔⛔⛔⛔ **A CLIENT OUTSIDE THIS PROCESS CAN ASK WHETHER A PANE'S CHILD HAS GONE** — register item
/// 544's first stage, and the read that had no address at all.
///
/// # Why this one read, out of the six a driver needs
///
/// Item 544's direction is to move the AI loop's driver OUT of the daemon, so that changing a loop
/// document stops meaning *restart the thing that owns your PTYs*. Everything else the driver reads
/// was already published — `full_text`, `full_lines`, `cells.<offset>`, the pane list, the input
/// actions, the event journal. **`PaneAccess::pane_eof` was not**: it has only ever been an
/// in-process atomic load, which nothing noticed while every reader lived in the daemon.
///
/// ⚠⚠⚠⚠⚠ **AND ITS ABSENCE IS THE EXPENSIVE ONE, MEASURED RATHER THAN RANKED.** It is what
/// `ai_loop.scxml`'s `peer_gone` stands on, and that ending exists because a pseudoterminal whose
/// child is dead takes 16,896 bytes and then blocks FOR EVER holding the pane's writer lock — a
/// driver typing its stimulus every step walks there in ~29 minutes, and **one run held a build
/// machine for 43 hours that way.** A remote driver that could not ask this would walk into the
/// same wall, so this address is the precondition for the driver being allowed to live elsewhere.
///
/// # ⚠⚠⚠ Why the pair is the claim
///
/// A slot hard-wired to `true` passes *the dead pane reads true*, and a slot hard-wired to `false`
/// passes *the live pane reads false*. **The two panes are the same host, the same connection and
/// the same instant**, differing only in whether their child has exited — which is the one thing
/// this address is about. ⚠ And the live pane is asserted FIRST and AGAIN after the other has
/// died, so a slot that answered about *some* pane rather than *this* one is red too.
#[test]
fn a_pane_whose_child_has_exited_says_so_at_an_address_a_remote_driver_can_ask() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    let eof = |conn: &mut HostConn, pane: u64| -> Option<bool> {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(pane, PANE_EOF_SLOT) }),
        )
        .ok()
        .and_then(|value| value.as_bool())
    };

    // ── A PANE WHOSE CHILD STAYS: `cat` holds its pseudoterminal open ──
    let living = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
        )
        .expect("spawn a pane whose child stays")
        .as_u64()
        .expect("spawn returns the new pane id");
    assert_eq!(
        eof(&mut conn, living),
        Some(false),
        "⚠⚠⚠ THE CONTROL: a pane running `cat` has NOT reached EOF, and the address must say so. \
         A slot answering `true` here would report every pane dead — which reads to a driver as \
         *stop typing*, i.e. a loop that refuses to work at a perfectly good peer.",
    );

    // ── AND ONE WHOSE CHILD LEAVES ──
    let dying = conn
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["true"] } }),
        )
        .expect("spawn a pane whose child exits at once")
        .as_u64()
        .expect("spawn returns the new pane id");
    assert!(
        wait_until(Duration::from_secs(5), || eof(&mut conn, dying)
            == Some(true)),
        "⛔⛔⛔⛔ REGISTER ITEM 544: a pane whose child has EXITED never said so at \
         {PANE_EOF_SLOT:?}. A driver in another process therefore cannot tell a dead peer from a \
         thinking one — the two look identical in the pane's text — and the only remedy left to it \
         is to keep typing, which is the 43-hour wedge `peer_gone` exists to prevent. Got {:?}",
        eof(&mut conn, dying),
    );

    // ⚠⚠ AND THE LIVE PANE IS STILL LIVE, asked in the same breath as the dead one. Without this
    // the assertions above are satisfied by an address that answers about the HOST rather than
    // about the pane it names — which is the failure a per-pane slot exists to make impossible.
    assert_eq!(
        eof(&mut conn, living),
        Some(false),
        "⚠⚠⚠ the address must answer about the pane it NAMES: one child exiting has been read as \
         every pane reaching EOF",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A CLIENT OUTSIDE THIS PROCESS CAN READ A PANE'S SCREEN THE TWO WAYS THE DRIVER READS
/// IT** — register item 544's stage 1b, and the pair whose absence forces a remote driver to
/// RE-DERIVE one of them and get the wrap wrong.
///
/// # ⚠⚠⚠⚠⚠ Why re-derivation is the defect, measured rather than argued
///
/// `cells.<offset>` has always been published, so a remote driver *could* rebuild the screen. What
/// it cannot rebuild is WHICH JOIN. `PaneAccess::pane_collapsed` joins each row's SHARE of its
/// logical line (`Screen::row_share_text`); `PaneAccess::pane_rows` reports each row's RENDERED
/// text (`Screen::row_text`), trailing blanks trimmed. **The obvious re-derivation — join the row
/// texts — is the one this repository has already paid for**: a pane five columns wide printing
/// `TOOL UP` wraps after the SPACE, so its rows are `"TOOL "` and `"UP"`; trimmed and joined they
/// read `"TOOLUP"`, and a barrier waiting for `TOOL UP` never clears. The width is not the driver's
/// to choose — whichever client attached decides it — so the same run, the same program and the
/// same marker hang or pass depending on somebody else's window.
///
/// # ⚠⚠⚠ Why the three reads are ONE claim, asked of one pane in one breath
///
/// * `screen_collapsed` must read `"GOTOOL UP"` — the interior space SURVIVES. A slot that joined
///   the rendered rows answers `"GOTOOLUP"` and is red.
/// * `screen_rows` must read `["GO", "TOOL", "UP"]` — the wrapped row is TRIMMED. A slot that
///   served the shares answers `"TOOL "` and is red, which is the same mutation from the other side.
/// * Neither may hold `"OLD"`, **which `full_text` at the same instant must hold** — otherwise the
///   screen-only claim is vacuous, satisfied by a pane that simply never scrolled.
///
/// ⚠⚠ The generation is deliberately NOT published. `PaneRow::generation` is a PAINT signal, and
/// this workspace has already recorded all four plugins that read it as *what did the peer produce*
/// being wrong — a resize or an OSC palette change stamps every row while no program writes a byte.
/// A remote answer carrying it would hand a driver that mistake pre-made.
///
/// ⚠ The barrier is stage 1a's own address: the pane's child prints and EXITS, and `eof` is true
/// only once every byte it wrote has been applied to the screen. No sleep, no echo, no resize race.
#[test]
fn a_pane_serves_its_screen_at_two_addresses_a_driver_cannot_derive_from_each_other() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5))
        .expect("connect to the spawned host socket");

    // Five columns wide and three rows tall, printing `OLD`, `GO`, then `TOOL UP`. The last line
    // wraps after the space, which scrolls `OLD` off the screen and into the scrollback — so one
    // pane carries the wrap claim AND the screen-versus-scrollback claim.
    let pane = conn
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SPAWN_ACTION),
                "args": {
                    "cmd": ["sh", "-c", "printf 'OLD\\nGO\\nTOOL UP'"],
                    "cols": 5,
                    "rows": 3,
                },
            }),
        )
        .expect("spawn a five-column pane that prints and exits")
        .as_u64()
        .expect("spawn returns the new pane id");

    let slot = |conn: &mut HostConn, name: &str| -> Option<Value> {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(pane, name) }),
        )
        .ok()
    };

    assert!(
        wait_until(Duration::from_secs(5), || slot(&mut conn, PANE_EOF_SLOT)
            .and_then(|value| value.as_bool())
            == Some(true)),
        "the printing child never reached EOF, so nothing below is reading a settled screen",
    );

    let collapsed = slot(&mut conn, SCREEN_COLLAPSED_SLOT)
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let rows: Vec<String> = slot(&mut conn, SCREEN_ROWS_SLOT)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let full = slot(&mut conn, FULL_TEXT_SLOT)
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();

    assert_eq!(
        collapsed, "GOTOOL UP",
        "⛔⛔⛔⛔ REGISTER ITEM 544: {SCREEN_COLLAPSED_SLOT:?} must join each row's SHARE of its \
         logical line, so a marker the terminal wrapped still matches. Joining the RENDERED rows \
         instead drops the space the wrap sat on and answers `GOTOOLUP` — the join a remote driver \
         would write for itself, and the reason this address exists rather than being derivable",
    );
    assert_eq!(
        rows,
        vec!["GO".to_owned(), "TOOL".to_owned(), "UP".to_owned()],
        "⛔⛔⛔⛔ REGISTER ITEM 544: {SCREEN_ROWS_SLOT:?} must report what each row RENDERS, \
         trailing blanks trimmed — what a person sees on that row. Serving the line shares here \
         would answer `TOOL ` and make the two addresses the same answer twice, which is the \
         re-derivation this pair exists to make impossible",
    );

    // ⚠⚠⚠ AND THE SCREEN-ONLY CLAIM, WITH ITS OWN NON-VACUITY PROOF: the scrolled-off line is
    // still readable at `full_text`, so "neither screen address holds it" is a statement about
    // SCOPE and not about a pane that happened to print little enough to fit.
    assert!(
        full.contains("OLD"),
        "the pane never scrolled, so the screen-only assertions below measure nothing. \
         {FULL_TEXT_SLOT:?} read {full:?}",
    );
    assert!(
        !collapsed.contains("OLD") && !rows.iter().any(|row| row.contains("OLD")),
        "⚠⚠⚠ both screen addresses must answer about the SCREEN: a line that scrolled off is \
         `full_text`'s to report, and a driver told otherwise re-reads output it has already \
         acted on. Got {collapsed:?} / {rows:?}",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Spawn a pane over the wire, answering its id — the setup every stage-1 gate below shares.
///
/// ⚠ On a connection of the TEST's, never the driver's. What the driver did, it did through
/// [`PaneAccess`] alone, and a setup call on its own connection would blur that.
fn spawn_pane(conn: &mut HostConn, args: Value) -> PaneId {
    PaneId(
        conn.call(
            "scene/invoke",
            json!({ "path": mux_action_path(SPAWN_ACTION), "args": args }),
        )
        .expect("spawn a pane over the socket")
        .as_u64()
        .expect("spawn returns the new pane id"),
    )
}

/// A driver's own surface over a real daemon's socket, plus a test-side connection for setup.
fn remote_driver(sock: &Path) -> (RemotePaneAccess, HostConn) {
    let setup = HostConn::connect(sock, Duration::from_secs(5)).expect("the test's own connection");
    let driving =
        HostConn::connect(sock, Duration::from_secs(5)).expect("the driver's own connection");
    (RemotePaneAccess::over(driving), setup)
}

/// The same, PARKABLE — a driver that also holds the second connection its waits park on
/// ([`RemotePaneAccess::parking_on`], register item 631).
fn parking_remote_driver(sock: &Path) -> (RemotePaneAccess, HostConn) {
    let (driver, setup) = remote_driver(sock);
    let parks = HostConn::connect(sock, Duration::from_secs(5)).expect("the driver's park socket");
    // ⚠ Both connections are unscoped and reach the same daemon, so the scope check register item
    // 641 added cannot refuse here — and `expect` is right rather than lenient: a refusal would
    // mean the check itself is wrong, which is a thing every gate below deserves to hear about.
    let driver = driver
        .parking_on(parks)
        .expect("two connections to one daemon, both unscoped, resolve to one session");
    (driver, setup)
}

/// **A [`PaneAccess`] THAT COUNTS THE READS A WAIT MAKES** — the instrument this gate's number is.
///
/// `sprag_plugin::testing::Counted` is the same idea and cannot be used here: it is
/// `#[cfg(test)]`-gated inside another crate, so an integration test cannot see it. What is counted
/// is deliberately the same thing — a question about a pane's CONTENT — because that is the
/// expensive read: over this surface it is a whole SCREEN across a socket.
///
/// ⚠⚠⚠ **A PARK IS NOT A LOOK, and that is the whole measurement.** `changes()` is forwarded
/// UNWRAPPED, so waiting costs nothing here and only looking does. An instrument that counted parks
/// too would report the same number either way and measure nothing.
struct CountingRemote {
    inner: RemotePaneAccess,
    looks: std::sync::atomic::AtomicU64,
    /// **HOW MANY OF THOSE ASKED THE SUPERVISOR** — register items 637 and 640, and the read that
    /// is a whole SOCKET ROUND TRIP for one pane's verdict.
    ///
    /// ⚠ Broken out for the reason `sprag_plugin::testing::Counted` breaks it out: a fold cannot
    /// separate *one round that asked four times* from *four rounds that asked once*, and both of
    /// those items are about which.
    supervisions: std::sync::atomic::AtomicU64,
}

impl CountingRemote {
    fn new(inner: RemotePaneAccess) -> Self {
        Self {
            inner,
            looks: std::sync::atomic::AtomicU64::new(0),
            supervisions: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn looks(&self) -> u64 {
        self.looks.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn supervisions(&self) -> u64 {
        self.supervisions.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn looked(&self) {
        self.looks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl sprag_plugin::PaneSupervision for CountingRemote {
    fn pane_agent_state(&self, id: PaneId) -> Option<sprag_plugin::AgentObservation> {
        self.looked();
        self.supervisions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.supervision()?.pane_agent_state(id)
    }
}

impl PaneAccess for CountingRemote {
    fn pane_ids(&self) -> Vec<PaneId> {
        self.inner.pane_ids()
    }

    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        self.looked();
        self.inner.pane_collapsed(id)
    }

    fn pane_rows(&self, id: PaneId) -> Option<Vec<sprag_plugin::PaneRow>> {
        self.looked();
        self.inner.pane_rows(id)
    }

    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        self.looked();
        self.inner.pane_eof(id)
    }

    /// ⚠⚠⚠ FORWARDED, for the same reason `supervision` below is — register item 555, and this
    /// wrapper is where that item's own defect would grow back.
    ///
    /// Every other method here delegates, so leaving this one out looks like nothing at all. It is
    /// not: the trait's default recomputes the answer from `pane_rows`, this wrapper forwards
    /// `pane_rows` faithfully, and the rows a REMOTE surface serves carry a damage generation of
    /// ZERO by design — so the default would answer *never painted* for every pane, through an
    /// instrument whose whole purpose is to measure what the real surface does.
    fn pane_has_painted(&self, id: PaneId) -> Option<bool> {
        self.looked();
        self.inner.pane_has_painted(id)
    }

    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        self.looked();
        self.inner.pane_full_text(id)
    }

    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
        self.inner.inject(id, keys)
    }

    /// ⚠ FORWARDED, so a wait that rests on the VERDICT can be measured at all. Left to the trait's
    /// default `None`, a completion contract driven through this instrument would find no
    /// supervisor, answer *never satisfied*, and the number below would be about a wait nobody
    /// makes.
    fn supervision(&self) -> Option<&dyn sprag_plugin::PaneSupervision> {
        self.inner
            .supervision()
            .map(|_| self as &dyn sprag_plugin::PaneSupervision)
    }

    fn changes(&self) -> Option<&dyn sprag_plugin::PaneChanges> {
        self.inner.changes()
    }
}

/// ⛔⛔⛔⛔ **A PARK CONNECTION ON ANOTHER SESSION IS REFUSED WHERE IT IS HANDED OVER** — register
/// item 641, and the obligation that used to be the caller's word.
///
/// # ⚠⚠⚠⚠⚠ What the promise cost while nothing checked it
///
/// The daemon DOES refuse a park naming a pane the scoped session does not hold, so a mis-scoped
/// connection fails loudly — **once**. What follows is the cost: to
/// [`PaneChanges::pane_moved_after`] that refusal is a transport failure, and a transport failure
/// retires the park connection (it must — a failed frame may be half-read). From that instant
/// [`PaneAccess::changes`] answers `None` **for the whole run**: the driver is back to reading a
/// screen a hundred times a second, and the one loud sentence that explained it went by in a wait
/// nobody was watching. One wrong session name buys a silent, permanent degradation.
///
/// # ⚠⚠⚠ Why the check is the DAEMON's answer and not this type's guess
///
/// [`SESSION_SLOT`] is *"the daemon's own answer to which session is this about"*, resolved by the
/// scope at the door — so both connections are simply asked. Asking the CONNECTION instead would
/// answer nothing for a client scoped to its attachment, which holds no name by design, and would
/// mis-answer a named scope whose session has since been retired.
///
/// # ⚠⚠ The three arms, and why the third is the one that would break an older daemon
///
/// * **DISAGREE ⇒ refused**, carrying both names and the surface itself, so a caller that would
///   rather degrade than stop keeps what it built.
/// * **AGREE ⇒ parkable**, which is the control: a check that refused everything would pass the
///   first arm and destroy the feature.
/// * **An ABSENCE is not a disagreement** — a daemon too old to serve the address answers nothing,
///   and refusing there would refuse every park against exactly the surface that most needs one.
///   That arm is argued rather than staged: this test's daemon is this build, so it always answers.
#[test]
fn a_park_connection_scoped_to_another_session_is_refused_where_it_is_handed_over() {
    let (_host, sock) = spawn_host();
    let mut setup = HostConn::connect(&sock, Duration::from_secs(5)).expect("the test's own");
    setup
        .call(
            "scene/invoke",
            json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": { "name": "work" } }),
        )
        .expect("a second session for the park connection to be wrong about");

    // ── THE CONTROL FIRST: two connections that agree are parkable ─────────────────────────────
    // ⚠ Before the claim, deliberately. A check that refused every park would satisfy the claim
    // below and leave item 631's whole repair unreachable — and a control placed after the defect
    // is a control that can be read as passing when the feature is gone.
    let (agreeing, _) = parking_remote_driver(&sock);
    assert!(
        agreeing.changes().is_some(),
        "⚠⚠⚠⚠ THE CONTROL: two connections to one daemon, both unscoped, must still yield a \
         parkable surface. If this refuses, the check refuses everything and register item 631's \
         repair is unreachable",
    );

    // ── THE CLAIM: a park connection on another session is refused, by name ────────────────────
    let driving = HostConn::connect(&sock, Duration::from_secs(5)).expect("the driver's own");
    let mut parks = HostConn::connect(&sock, Duration::from_secs(5)).expect("the park socket");
    parks.scope_to("work");
    let refused = RemotePaneAccess::over(driving)
        .parking_on(parks)
        .err()
        .unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔⛔⛔ REGISTER ITEM 641: a park connection scoped to a DIFFERENT session was \
                 accepted. Its first park names a pane that session does not hold, the daemon \
                 refuses it, the refusal retires the connection — and the driver polls for the \
                 rest of its run with nothing anywhere saying why. The obligation was the \
                 caller's word and nothing asked the daemon, which answers this at \
                 `SESSION_SLOT` and always could"
            )
        });
    // ⚠ The park side is asserted by NAME because this test chose it; the read side is asserted by
    // its PROPERTIES rather than by a guessed default-session name — a gate that hard-codes what it
    // did not author is a gate that goes red the day the daemon renames its boot session, which
    // would say nothing about item 641.
    assert_eq!(
        refused.park, "work",
        "⚠⚠⚠ the park side must be named, and it is the one this test scoped: {refused:?}",
    );
    assert!(
        !refused.read.is_empty() && refused.read != refused.park,
        "⚠⚠⚠ BOTH NAMES, because which one is wrong depends on which the caller meant — a refusal \
         that said only *the park connection is on the wrong session* cannot be acted on, and one \
         that carried an empty read side is that refusal wearing two fields. Got {refused:?}",
    );

    // ── AND THE SURFACE SURVIVES ITS OWN REFUSAL ──────────────────────────────────────────────
    // ⚠⚠ The builder took `self` by value, so an error carrying only the complaint would destroy
    // what two connections were spent making. A caller that chooses *park-less over stopping* is
    // making a legitimate decision, and this is what makes that decision free.
    assert!(
        refused.degraded.changes().is_none(),
        "⚠⚠⚠ the surface handed back must be the DOCUMENTED degradation — parkable would mean the \
         refusal kept the very connection it refused",
    );
    assert!(
        !refused.degraded.pane_ids().is_empty(),
        "⚠⚠ ...and it must still be a working driver, or handing it back buys nothing",
    );
}

/// Run a `git` verb in `at` and say whether it worked — enough git for a fixture, and no more.
fn git(at: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(at)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What a repository's own `git diff HEAD` reads — the exact thing register item 705's «done when»
/// asks to hold still while a check runs.
fn diff_head(at: &Path) -> Vec<u8> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(at)
        .args(["diff", "HEAD"])
        .output()
        .expect("a repository answers its own diff")
        .stdout
}

/// ⛔⛔⛔⛔⛔ **THE SURFACE A LIVE RUN USES REALLY CUTS A COPY, AND THE AGENT'S TREE DOES NOT MOVE**
/// — register item 705, at the one seam its two other gates leave between them.
///
/// # ⚠⚠⚠⚠⚠ What was unproven until this, stated exactly
///
/// Item 705 is held by three claims and they live in three places:
///
/// * **which directory the driver picks** — `sprag_plugin`'s
///   `a_check_is_sent_to_a_copy_and_the_sentence_names_the_copy`, over a STAND-IN copy;
/// * **whether a real copy carries the work and isolates a mutation** —
///   `sprag_host::checkout`'s own gate, over real `git`;
/// * **and that the surface a live run actually holds joins those two** — which nothing said, and
///   is this gate. Without it the first two can both be green while `PaneAccess::checkout` on the
///   live surface answers something that is not the mechanism at all.
///
/// ⚠⚠ **THE SURFACE IS THE REMOTE ONE ON PURPOSE.** `RUN_DRIVER_PROCESS` defaults to `on`, so a
/// live run's driver is a process outside the daemon and sees its panes through exactly this type.
/// The in-process surface cannot isolate — its crate may not spawn a `git` — and degrades, which is
/// a loss the product names rather than hides.
///
/// # ⚠⚠⚠ The premises, asserted inside
///
/// * **The repository is mid-claim**, carrying work that is not yet committed. A clean tree would
///   be satisfied by a copy of `HEAD` alone, and the arm about carrying the agent's in-flight work
///   would be about nothing.
/// * **The check really writes.** *The shared tree did not change* is satisfied trivially by a
///   check that never touched anything — register item 280's fifth lesson, and the reason the
///   mutation below is asserted to have landed before the original is asked whether it moved.
#[test]
fn the_surface_a_run_uses_cuts_a_copy_and_the_agents_tree_does_not_move() {
    let (_host, sock) = spawn_host();
    let (driver, _setup) = remote_driver(&sock);

    let repo = std::env::temp_dir().join(format!(
        "sprag-705-seam-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).expect("a directory to make a repository in");
    assert!(
        git(&repo, &["init", "-q", "."]),
        "the fixture needs a `git`"
    );
    assert!(git(&repo, &["config", "user.email", "gate@example"]));
    assert!(git(&repo, &["config", "user.name", "gate"]));
    std::fs::write(repo.join("door.txt"), "door returns 1\n").expect("the committed file");
    assert!(git(&repo, &["add", "door.txt"]));
    assert!(git(&repo, &["commit", "-qm", "base"]));
    // ⚠⚠ THE AGENT IS MID-CLAIM: this is the work the milestone is about, and it is not in `HEAD`.
    std::fs::write(repo.join("door.txt"), "door returns 2\n").expect("the uncommitted edit");

    let before = diff_head(&repo);
    assert!(
        !before.is_empty(),
        "⚠⚠⚠⚠ THE PREMISE: the agent must have work that is not yet committed, or a copy of `HEAD` \
         alone would satisfy the arm below and this gate would be about nothing",
    );

    // ── THE SEAM: the surface a live run holds hands back the real mechanism ──────────────────
    let surface = driver.checkout().expect(
        "⛔⛔⛔ ITEM 705: the surface every out-of-daemon run uses cannot isolate at all, so the \
         milestone checker is loose in the tree the agent is working in — which is what this item \
         was filed about, measured on run 0",
    );
    let cut = surface.cut(&repo).expect("a repository can be cut");
    assert_ne!(
        cut.path(),
        repo.as_path(),
        "⛔⛔⛔⛔⛔ THE «COPY» IS THE ORIGINAL. Every assertion below would pass over a surface that \
         isolates nothing, and the checker would be told to open the files in the agent's own tree",
    );
    assert_eq!(
        std::fs::read_to_string(cut.path().join("door.txt")).ok(),
        Some("door returns 2\n".to_owned()),
        "⛔⛔⛔⛔ THE CHECKER WOULD JUDGE THE WRONG TREE: the claim is about work that is not in \
         `HEAD`, and a copy without it lets a careful checker open real files, reason correctly, \
         and answer about something else",
    );

    // ── ⚠⚠ THE CHECK MUTATES — the premise that makes the claim below mean anything ───────────
    std::fs::write(cut.path().join("door.txt"), "MUTATED BY THE CHECK\n")
        .expect("the check writes into what it was given");
    assert_eq!(
        std::fs::read_to_string(cut.path().join("door.txt")).ok(),
        Some("MUTATED BY THE CHECK\n".to_owned()),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: nothing was written, so «the agent's tree did not move» below \
         is satisfied by a check that never touched anything at all",
    );

    // ── THE CLAIM ────────────────────────────────────────────────────────────────────────────
    assert_eq!(
        diff_head(&repo),
        before,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 705: a check mutated the tree the AGENT is working in. Measured \
         2026-08-26 on run 0: a watcher read exactly this as the agent's leftover, told the owner \
         one sentence of the agent's report was false — it was true — and then ran `git checkout \
         --` over it, which was a no-op only because the check had already finished",
    );

    // ── AND THE COPY GOES WHEN THE CHECK IS OVER ─────────────────────────────────────────────
    let was_at = cut.path().to_path_buf();
    drop(cut);
    assert!(
        !was_at.exists(),
        "⚠⚠⚠⚠ A COPY OUTLIVED ITS CHECK — a second tree holding a half-applied mutation, which is \
         the confusion this item exists to end, re-created by the repair",
    );
    assert_eq!(
        diff_head(&repo),
        before,
        "⚠⚠ and removing the copy is not a write to the agent's tree either",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// ⛔⛔⛔⛔⛔ **A WHOLE RUN, DRIVEN, CUTS A REAL COPY AND WAKES ITS CHECKER UP INSIDE IT** —
/// register item 705's last link, and the one its four other gates leave to the compiler and to
/// each other.
///
/// # ⚠⚠⚠⚠⚠ What was still unmeasured, stated exactly
///
/// Item 705 is one sentence — *the milestone checker must not mutate the tree the agent is working
/// in* — and it was held by four gates that each own a different piece of it:
///
/// * `sprag_plugin`'s `a_check_is_sent_to_a_copy…` drives the product's own resolution, over a
///   STAND-IN copy: no `git` anywhere in it;
/// * `sprag_host::checkout`'s own gate cuts a REAL copy, and never meets a loop;
/// * the seam gate above holds that the surface a live run holds hands back that real mechanism —
///   and cuts the copy ITSELF rather than watching a run do it;
/// * `sprag_plugin`'s `the_checker_the_product_spawns_stands_in_the_copy` calls `checked` directly,
///   which is the real spawn, over a stand-in copy again.
///
/// **The two halves could not meet inside one test, and the reason was structural**:
/// [`OuterLoop::checked`] is private to `sprag_plugin`, and `IsolatedCheckout` lives in this crate.
/// Re-implementing either of them in a fixture is measuring one's own variable — which is the
/// mistake item 705's FIRST gate made, and it stayed green under a mutation that made the product
/// name the shared tree. So the only road left was a REAL RUN: build the loop through the same door
/// the daemon builds it through, drive it with the [`Driver`] over the same remote surface a live
/// run holds, and let the PRODUCT decide, on its own, to cut a copy and spawn a checker in it.
///
/// # ⚠⚠⚠ Why the plugin is BUILT here rather than requested
///
/// `milestone_check` is a loop KIND's field and appears **zero times in `wire.rs`**: no request can
/// carry a checker, so `drive_request` cannot stage this at all — the neighbouring `ai_loop` gates
/// go through that door precisely because they do not need one. `AiLoop::new` is `pub`, and this
/// takes the same four arguments `plugins.rs` hands it.
///
/// # ⚠⚠ The peer is a ONE-SHOT, and that is a product contract rather than a convenience
///
/// [`DoneWhen::Exits`] is `claude -p`'s contract, and it is chosen over `INNER_SESSION_ENDS`
/// because this daemon supervises no `/bin/sh`: a `Settles` turn asks a supervisor whether the
/// agent came back to rest, nobody reports for a shell, and the turn would never end. Said plainly
/// rather than hidden — **the claim under test is where the checker WAKES UP, and no turn contract
/// touches that.** The barrier is [`ReadyWhen::Runs`] for the same reason: it asks the operating
/// system which program owns the pane's terminal, which is a fact about a shell.
///
/// # ⛔⛔⛔⛔⛔ The reading is taken WHILE THE CHECK IS RUNNING, and that is the item's own words
///
/// Item 705's «done when» is a sentence about a WINDOW: *while a check that inserts and reverts a
/// mutation runs, the shared tree's `git diff HEAD` keeps reading the same, and a gate asserts it*.
/// Every gate before this one read the tree AFTERWARDS — and **run 0's checker put back exactly
/// what it found**, so an afterwards-reading is green about the very defect this item was filed
/// on. The two costs it caused were both paid inside that window: a watcher attributed the
/// mutation to the agent and told the owner the agent's report was false (it was true), and then
/// ran `git checkout --` over it, a no-op only because the check had already finished.
///
/// So the checker here mutates, **holds its own mutation standing** until this gate has looked,
/// and only then puts back what it found. The reading is not a sample — it answers the checker's
/// own signal — so *the tree did not move* is measured at the one instant that can disprove it.
///
/// # What reddens it, measured rather than reasoned about
///
/// | mutation | verdict |
/// |---|---|
/// | `a_check_to_put` resolves the SHARED tree instead of the copy | **red — in the window** |
/// | `checked` spawns in the agent's tree while the sentence names the copy | **red — in the window** |
/// | `RemotePaneAccess::checkout` answers `None`, so the live run degrades | **red — in the window** |
/// | `IsolatedCheckout::of` skips the diff, so the copy holds `HEAD` alone | **red — what it was given** |
///
/// ⚠⚠ The first three land on the WINDOW reading and not on the directory arm below it, which is
/// what says the window is the load-bearing one: the checker puts back what it found, so the tree
/// reads identical afterwards in every one of them.
///
/// ⚠ The directory arm is kept beside it rather than folded in, and it is a DIFFERENT claim: a
/// checker standing in the agent's tree that happened to write nothing moves no diff and is still
/// the defect. It is second in the file because the window's failure is the more informative one.
///
/// # ⚠⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **A check was really asked** — `Checks::asked` counts the one place a claim is put to an
///   independent process. Zero means nothing was spawned and every path below is about a check that
///   never happened.
/// * **The window really opened.** A check that finished without standing a mutation up never gave
///   the reading above anything to be about.
/// * **The checker really woke up.** It writes `pwd` to a file OUTSIDE the copy, so the record
///   outlives the copy's removal; an absent file is *no checker ran*, not *it stood in the copy*.
/// * **The repository is mid-claim.** A clean tree would be satisfied by a copy of `HEAD` alone,
///   and the arm about carrying the agent's in-flight work would be about nothing.
/// * **The check really writes**, and what it wrote is carried OUT of the copy the same way — so
///   *the agent's tree did not move* cannot be satisfied by a check that touched nothing at all
///   (register item 280's fifth lesson).
#[test]
fn a_driven_run_cuts_a_real_copy_and_its_checker_wakes_up_in_it() {
    // ⚠⚠ NO PARENTHESES IN ANY OF THESE PATHS, and it is not fussiness: `milestone_check` is split
    // on WHITESPACE and its words reach a shell, where `(` is a metacharacter. Measured in
    // `sprag_plugin`'s spawn gate — a `{:?}` thread id rendered `ThreadId(2)` into the argv, and
    // the checker answered with its own path instead of a verdict.
    let under = std::env::temp_dir().join(format!("sprag-705-driven-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&under);
    std::fs::create_dir_all(&under).expect("somewhere to put the fixture");
    let repo = under.join("the-agents-tree");
    std::fs::create_dir_all(&repo).expect("the run's repository");

    // ── THE AGENT'S REPOSITORY, MID-CLAIM ────────────────────────────────────────────────────
    assert!(
        git(&repo, &["init", "-q", "."]),
        "the fixture needs a `git`"
    );
    assert!(git(&repo, &["config", "user.email", "gate@example"]));
    assert!(git(&repo, &["config", "user.name", "gate"]));
    std::fs::write(repo.join("door.txt"), "door returns 1\n").expect("the committed file");
    assert!(git(&repo, &["add", "door.txt"]));
    assert!(git(&repo, &["commit", "-qm", "base"]));
    // ⚠⚠ THE WORK THE MILESTONE IS ABOUT, and it is not in `HEAD`.
    std::fs::write(repo.join("door.txt"), "door returns 2\n").expect("the uncommitted edit");
    let before = diff_head(&repo);
    assert!(
        !before.is_empty(),
        "⚠⚠⚠⚠ THE PREMISE: the agent must hold work that is not yet committed, or a copy of `HEAD` \
         alone would satisfy the arm below and this gate would be about nothing",
    );

    // ── THE CHECKER: it says where it woke, reads what it was given, and MUTATES-AND-REVERTS ──
    //
    // ⚠⚠⚠ EVERY RECORD IT LEAVES IS OUTSIDE THE COPY, because the copy is removed the moment the
    // check ends — evidence left inside it dies with it, and this gate would then be asserting
    // about files that are gone for a reason it cannot tell from *the checker never ran*.
    //
    // ⛔⛔⛔⛔⛔ **IT PUTS BACK EXACTLY WHAT IT FOUND, WHICH IS WHAT RUN 0's CHECKER DID** — and
    // that is what makes the reading below load-bearing rather than decorative. A checker that
    // mutated and LEFT is caught by any read afterwards; the one item 705 was filed on left the
    // tree byte-identical and was still the cause of two misattributions and a `git checkout --`.
    // **So the tree has to be read WHILE the mutation is standing**, which is what the window
    // below is: the checker holds its own mutation open until this gate has looked.
    let stood_at = under.join("where-the-checker-stood");
    let read_back = under.join("what-the-checker-was-given");
    let landed = under.join("what-the-checks-write-left");
    let saved = under.join("what-the-checker-found");
    let window = under.join("the-mutation-is-standing");
    let go = under.join("the-gate-has-looked");
    let over = under.join("the-run-is-over");
    let checker = under.join("checker.sh");
    std::fs::write(
        &checker,
        format!(
            // ⚠⚠ IT ONLY WRITES WHERE THE FILE IT IS JUDGING ALREADY IS, so a build whose origin
            // regressed — the checker then lands in `$HOME`, which is item 710's measured defect —
            // fails the premise below instead of leaving a `door.txt` in somebody's home directory.
            "pwd > {stood}\n\
             cat door.txt > {given} 2>&1\n\
             if [ -f door.txt ]; then cp door.txt {saved}; \
             printf 'MUTATED BY THE CHECK\\n' > door.txt; fi\n\
             cat door.txt > {landed} 2>&1\n\
             : > {window}\n\
             i=0\n\
             while [ ! -f {go} ] && [ $i -lt 600 ]; do \
             i=$((i+1)); sleep 0.1 2>/dev/null || sleep 1; done\n\
             if [ -f {saved} ]; then cp {saved} door.txt; fi\n\
             printf 'YES the work is on disk\\n'\n",
            stood = stood_at.display(),
            given = read_back.display(),
            saved = saved.display(),
            landed = landed.display(),
            window = window.display(),
            go = go.display(),
        ),
    )
    .expect("the stand-in checker");

    // ── THE DAEMON, AND A PANE BORN IN THE REPOSITORY ────────────────────────────────────────
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["cat"]);
    let (driving, mut setup) = remote_driver(&sock);

    // ⚠⚠⚠ THE PEER PAINTS WHAT IT READS, and that line is a fix rather than decoration: with echo
    // off and nothing painted, `deliver` can never confirm the prompt arrived and RETYPES it, so a
    // peer counting prompts sees two where the driver sent one. `sprag_plugin`'s own stand-in
    // carries the same line for the same measured reason.
    // ⚠⚠ IT KEYS ON `exactly:`, which is the PRODUCT's own contract — `done_instruction` ends
    // *"reply exactly: MILESTONE REACHED"* — rather than on a private word no real agent would see.
    let peer = "stty -echo; while IFS= read -r line; do printf '%s\\n' \"$line\"; \
                case \"$line\" in *exactly:*) printf 'MILESTONE REACHED\\n'; exit 0;; esac; done";
    let pane = spawn_pane(
        &mut setup,
        json!({
            SPAWN_CMD_KEY: ["/bin/sh", "-c", peer],
            SPAWN_CWD_KEY: repo.to_string_lossy(),
            SPAWN_COLS_KEY: 80,
            SPAWN_ROWS_KEY: 16,
        }),
    );

    // ── THE RUN: the daemon's own construction, one argument at a time ───────────────────────
    let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
        std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
    let brief = sprag_plugin::Brief {
        north_star: "the checker judges a copy and leaves the agent's tree alone".to_string(),
        milestone: "the door is open".to_string(),
        reference: "register item 705".to_string(),
        closing_rules: None,
        context_ceiling: None,
        reflect_after_refusals: None,
        // ⛔⛔⛔ THE WHOLE REASON THIS RUN IS BUILT HERE RATHER THAN REQUESTED.
        milestone_check: Some(format!("/bin/sh {}", checker.display())),
        service: None,
        max_turns: Some(sprag_plugin::Counted::Of(4)),
        // ⚠ EQUAL to the turn budget, which is what keeps `reflecting` unreachable: `judging` tests
        // the turn budget first.
        reflect_every: Some(4),
        screen_rules: None,
        // ⚠ NOBODY IS ASKED and nobody is waited for — this peer raises no dialog, and inheriting
        // the shipped document's patience would hang the suite at the first one it did raise.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        hold_within_ms: Some(3_600_000),
        ready_timeout_ms: Some(10_000),
        turn_within_ms: Some(20_000),
    };
    let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
    spec.ready_when = Some(ReadyWhen::Runs("sh".to_string()));
    spec.done_when = sprag_plugin::DoneWhen::Exits;
    // ⚠ A `/bin/sh` peer paints only once it holds a whole LINE, so a delivery cannot be confirmed
    // on screen before the newline that submits it. `driving` is the real-agent shape.
    spec.shows_the_prompt = false;
    let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
        .expect("a well-briefed loop over a live pane starts");

    // ── ⚠⚠⚠⚠⚠ THE READER THAT LOOKS *WHILE THE CHECK IS RUNNING* ─────────────────────────────
    //
    // Item 705's «done when» is a sentence about a WINDOW — *the shared tree's `git diff HEAD`
    // stays put while a check that inserts and reverts a mutation runs* — and the run is driven on
    // this thread, so nothing on it can look during that window. This is not a SAMPLER either: it
    // waits for the checker's own signal and answers it, so the reading is taken with the mutation
    // provably standing rather than whenever a poll happened to land.
    let watching = {
        let repo = repo.clone();
        let window = window.clone();
        let go = go.clone();
        let over = over.clone();
        std::thread::spawn(move || -> Option<Vec<u8>> {
            loop {
                if window.exists() {
                    // ⚠⚠ READ FIRST, RELEASE SECOND. The checker is holding its mutation open
                    // until `go` exists, so this order is what makes the reading in-window.
                    let seen = diff_head(&repo);
                    let _ = std::fs::write(&go, b"looked\n");
                    return Some(seen);
                }
                // ⚠ THE RUN ENDING IS THE OTHER ANSWER, so a check that never ran ends this thread
                // instead of holding the gate for the checker's whole bound.
                if over.exists() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = Driver::new(Guardrails {
        max_iterations: 40,
        max_cost: None,
        max_duration: Some(Duration::from_secs(120)),
    })
    .reporting_to(std::sync::Arc::clone(&progress))
    .run(&mut loops, &driving, &RunContext::uncancellable());
    let _ = std::fs::write(&over, b"over\n");
    let in_window = watching.join().expect("the in-window reader");
    let walk: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|step| step.note.clone())
        .collect();

    // ── ⚠⚠ THE PREMISES ──────────────────────────────────────────────────────────────────────
    assert_eq!(
        outcome.checks.asked, 1,
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: this run never put its milestone claim to an independent \
         process, so every claim below is about a check that did not happen. Ended {:?} after {} \
         pumps; the walk: {walk:?}",
        outcome.state, outcome.iterations,
    );
    assert_eq!(
        outcome.checks.silent, 0,
        "⚠⚠⚠ THE PREMISE FAILED: the check was asked and said nothing back, which is a fixture \
         that did not start rather than a verdict. What the run says about the silence: {:?}",
        outcome.checks.why_silent,
    );
    let stood_in = std::fs::read_to_string(&stood_at).unwrap_or_default();
    assert!(
        !stood_in.trim().is_empty(),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: no checker ever woke up, so «it stood in the copy» below is \
         true of nothing. Ended {:?}; the walk: {walk:?}",
        outcome.state,
    );
    let stood_in = std::path::Path::new(stood_in.trim()).to_path_buf();
    assert_eq!(
        std::fs::read_to_string(&landed).ok().as_deref(),
        Some("MUTATED BY THE CHECK\n"),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the check never wrote anything, so «the agent's tree did not \
         move» below is satisfied by a check that touched nothing at all — register item 280's \
         fifth lesson, and the reason this fixture carries its own write out of the copy",
    );
    let Some(in_window) = in_window else {
        panic!(
            "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the check finished without ever standing a mutation up, so \
             the one reading item 705's «done when» asks for was never taken. The checker woke up \
             in {}; the walk: {walk:?}",
            stood_in.display(),
        )
    };

    // ── THE CLAIM, IN THE WINDOW ITEM 705's «DONE WHEN» NAMES ────────────────────────────────
    //
    // ⚠ COMPARED AS TEXT, which is not a weakening: `diff_head` answers a diff, and a reader
    // meeting this failure needs to see WHICH hunk moved. The byte vectors are the same values
    // and render as a hundred and forty numbers.
    assert_eq!(
        String::from_utf8_lossy(&in_window),
        String::from_utf8_lossy(&before),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 705, VERBATIM: WHILE the check was running, with its mutation \
         standing, the tree the AGENT is working in had moved. This is the reading nothing could \
         take before — the checker puts back exactly what it found, so every look AFTERWARDS is \
         green about it. Measured 2026-08-26 on run 0: a watcher read that mutation as the \
         agent's leftover, told the owner one sentence of the agent's report was false — it was \
         true — and then ran `git checkout --` over it, which was a no-op only because the check \
         had already finished. The checker stood in {}",
        stood_in.display(),
    );
    assert_ne!(
        stood_in, repo,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 705: a whole driven run spawned its milestone checker into the \
         tree the AGENT is working in. Measured 2026-08-26 on run 0: the checker mutates what it \
         judges, a watcher read that mutation as the agent's leftover, told the owner one sentence \
         of the agent's report was false — it was true — and ran `git checkout --` over it",
    );
    assert_eq!(
        std::fs::read_to_string(&read_back).ok().as_deref(),
        Some("door returns 2\n"),
        "⛔⛔⛔⛔ THE CHECKER JUDGED THE WRONG TREE: the claim is about work that is not in `HEAD`, \
         and a copy without it lets a careful checker open real files, reason correctly, and \
         answer about something else. It stood in {}",
        stood_in.display(),
    );
    // ⚠⚠ AND AFTERWARDS TOO, which is a WEAKER claim than the window above and still its own: it
    // is what catches a check that mutated and did not put back, and a «revert» that put back
    // something other than what it found — the shape that would leave the agent's uncommitted work
    // silently rewritten by the process sent to judge it.
    assert_eq!(
        String::from_utf8_lossy(&diff_head(&repo)),
        String::from_utf8_lossy(&before),
        "⛔⛔⛔⛔ REGISTER ITEM 705: the agent's tree does not read the same after the check as it \
         did before it. The checker stood in {}",
        stood_in.display(),
    );
    assert!(
        !stood_in.exists(),
        "⚠⚠⚠⚠ A COPY OUTLIVED ITS CHECK — a second tree holding a half-applied mutation, which is \
         the confusion this item exists to end, re-created by the repair. It is still at {}",
        stood_in.display(),
    );

    let _ = std::fs::remove_dir_all(&under);
}

/// **WHERE A STAND-IN PUTS ITS DECLARATION** — the axis register item 441 is about, staged.
///
/// # ⚠⚠⚠⚠⚠ Why *where* is a separate question from *whether*
///
/// `OuterLoop::said_marker` asks the PEER before it reads the pane: a statement dated to this turn
/// (`said_seq` past the turn's arming) is used and the terminal is never looked at; without one the
/// pane's lines since the prompt are all there is. **Both roads answer *the agent declared*, and
/// they are not equally trustworthy** — item 441 measured the pane road going permanently blind
/// against a full-screen agent, nine judged turns in a row.
///
/// So a fixture that BOTH prints and states proves nothing about which road answered, and every
/// host gate before this one declared by printing alone. These arms separate them.
#[derive(Clone, Copy)]
enum Declares {
    /// Never — it acknowledges every turn, which is what a run that must end on its budget wants.
    Never,
    /// On this WORK turn, PRINTED on its screen and stated nowhere. The pane is the only evidence,
    /// which is the world every gate here lived in before `--said` existed.
    ByPrinting(u32),
    /// On this WORK turn, STATED in its report and printed nowhere. ⚠⚠ The screen says `ACK`, so a
    /// reader that fell back to the pane finds nothing at all — which is what makes a convergence
    /// here attributable to the statement and to nothing else.
    ByStating(u32),
}

/// **WHAT THE FIRST BILLED REQUEST OF [`standin_that_reports_itself`]'s RECORD WRITES TO CACHE** —
/// `cold`, the toll a replacement re-pays, and the number `reviewing`'s economic door multiplies by
/// twenty.
///
/// ⚠⚠ SIZED SO THAT THE ECONOMICS ARE **EVALUATED AND FALSE**, never skipped. The guard reads
/// `cold > 0 && floor > 0 && context - floor >= 20 * cold`, so a fixture leaving `cold` at zero
/// would take the same road for a different reason — and a control that passes because a term was
/// absent proves nothing about the term that matters. At 5,000 the break-even is 100,000 and this
/// stand-in's reading never approaches it.
const STANDIN_COLD: u64 = 5_000;
/// What every one of its billed requests reports as OUTPUT — what register item 719's per-turn
/// reading is the difference of. ⚠ Not `1`: a count that could be confused with a request COUNT
/// would let a wrong reader look right.
const STANDIN_PRODUCES: u64 = 7;

/// **A STAND-IN AGENT THAT REPORTS ITS OWN TURNS THROUGH THE SHIPPED CLI** — register item 730, and
/// the fixture that makes [`INNER_SESSION_ENDS`](sprag_plugin::INNER_SESSION_ENDS) drivable from a
/// host integration test at all.
///
/// # ⚠⚠⚠⚠⚠ What it stands in for, and what it deliberately does NOT fake
///
/// [`DoneWhen::Settles`](sprag_plugin::DoneWhen::Settles) asks a SUPERVISOR whether the agent came
/// back to rest, and this daemon supervises no `/bin/sh` — its detector reads screens and titles
/// and reports no agent for a shell. Two roads were available and only one of them is honest:
///
/// * **paint the screen like a real agent** so the DETECTOR is fooled. Refused: that measures the
///   fixture's imitation of somebody else's UI, which is this workspace's own standing rule about
///   stand-ins that parse by pattern, and item 730 names it out loud;
/// * **report, as a hook-instrumented agent does.** The stand-in says what it is doing through
///   `sprag report-agent`, in the pane the daemon gave it, over the socket the daemon published
///   into that pane's environment. Nothing here is a double: the CLI is the shipped binary, the
///   action is the shipped one, the tracker counts it exactly as it counts `claude`'s hook.
///
/// # ⛔⛔⛔ `--asked` IS THE WHOLE MECHANISM, and it did not exist until item 730
///
/// `Settles` pairs the peer's rest against
/// [`asked_seq`](sprag_detect::Tracker::asked_seq) wherever the rest is the agent's own statement,
/// and that counter moves only on a report that STATES A PROMPT. The verb could not state one — the
/// key was reachable exclusively through `sprag hook` — so a reporter could say `idle` for ever and
/// the turn never ended. `states_asked: false` is that world, kept as this gate's control.
///
/// # ⚠⚠ Why it answers only the lines the DOCUMENT's own prompts end with
///
/// A prompt is one paste to a real agent and many `read` lines to a shell. Reporting per LINE would
/// end the turn on the first line of a twenty-line prompt — `Settles`' own documented hazard, met
/// from the fixture's side — so a turn is reported once, on the clause the composed prompt ends
/// with. `sprag_plugin`'s `standin_agent` keys on the same two for the same reason and is
/// unreachable from here (`#[cfg(test)]`, in another crate).
///
/// ⚠⚠⚠ **THE STOP CLAUSE IS SPELLED, AND ITS DRIFT IS CAUGHT BY AN ASSERTION RATHER THAN A COPY.**
/// A fixture cannot read a datamodel that has not been initialised. What holds it in step is the
/// ENDING: a stand-in that stops recognising what it is asked never publishes another change, so
/// the run walks to its wall clock and reports `exhausted(duration)` where the gate demands
/// `exhausted(turns)`. That is the shape four plugin gates were measured coming back wrong in
/// before `standin_agent` learned to answer both endings.
///
/// # ⚠⚠⚠⚠ AND IT ANSWERS THE REFLECTION, which is what lets a run REPLACE ITS SESSION
///
/// (see [`STANDIN_COLD`] for the numbers the record it writes carries)
///
/// `reflecting` is a `Settles` turn like any other (`OuterLoop::reflect` calls `watch` and refuses
/// anything but `turn.done`), and its answer is two labelled rows the driver reads back off the
/// pane. A stand-in that ignored it would end the turn and name no successor, so the run would take
/// `reflect.none`, keep its session, and **nothing downstream of `reviewing` would ever be walked**
/// — which is the shape `standin_agent` was given this clause for.
///
/// ⚠⚠ **THE ANSWER TEXT MUST NOT APPEAR IN THE PROMPT.** `OuterLoop::proposed` rejects a candidate
/// row the prompt just delivered contains — the echo rule — and the prompt names both labels
/// mid-sentence. So the caller's milestone and reference are what keep the two apart, and the gate
/// asserts the driver really took them.
///
/// # ⚠⚠⚠⚠ AND IT CAN DECLARE, which is the only way a stand-down or a convergence is reachable
///
/// `declares_on` is the WORK turn — a reflection and a closing account are not one — at which the
/// peer says the document's done marker instead of acknowledging. Every ending that rests on the
/// agent having finished something needs it: `judging`'s stand-down edge is
/// `_event.data.done && In('standing_down')`, its milestone edge is `_event.data.done`, and a peer
/// that only ever acknowledges can reach neither. [`None`] never declares, which is what the two
/// gates above want.
///
/// ⚠ `MILESTONE REACHED` is spelled here on the other clauses' terms, and its drift has the same
/// catcher: a run whose peer says the wrong words never converges, loudly, in the one gate that
/// asks for that ending.
fn standin_that_reports_itself(
    sprag: &Path,
    log: &Path,
    state: &Path,
    states_asked: bool,
    next: (&str, &str),
    record: Option<&Path>,
    declares: Declares,
) -> String {
    // ⚠⚠ THE ONE THING THE TWO ARMS DIFFER BY, and it is the whole of item 730 on the product's
    // side: the prompt the turn opened on, stated by the peer that was asked it.
    let asked = if states_asked {
        " --asked \"$line\""
    } else {
        ""
    };
    let (milestone, reference) = next;
    // ⚠⚠⚠⚠ **A SESSION THAT KEEPS A RECORD, AND SAYS SO** — the second half of item 730's absence.
    // `--transcript` is how a reporter makes its spend readable at all, and this peer writes the
    // file it names: one billed request per turn, appended BEFORE the `idle` that ends the turn, so
    // the reading the driver takes at `turn.done` already includes it.
    //
    // ⚠⚠⚠ THE MESSAGE ID CARRIES THE SHELL'S PID (`$$`) AND MUST. `spend_in` deduplicates by id —
    // a streamed reply repeats its envelope — and a REPLACED session re-runs this script from the
    // top with `s` back at 1, so ids without the pid would collide with the predecessor's rows and
    // the total would stop growing exactly where this gate needs it to grow.
    //
    // ⚠⚠⚠⚠ **AND THE READING GROWS, WHICH IS WHAT MAKES A CEILING REACHABLE.** `context` is the
    // LAST billed request's cache read, so a fixture that wrote a constant could never cross an
    // authored `context_ceiling` — the door would be unreachable and a gate over it would be
    // measuring the number it typed. Row *n* reads `4000 + 4000n`, and only the FIRST row writes
    // cache: `cold` is the first request's WRITE and `floor` is the SECOND request's READ, so the
    // schedule fixes both at values the economics guard is EVALUATED against and fails on the
    // arithmetic — never skipped for being zero, which is a different control.
    let (states_record, writes_row) = match record {
        Some(path) => (
            format!(" --transcript '{}'", path.display()),
            format!(
                "n=$((n+1))\n\
                 r=$((4000 + 4000 * n))\n\
                 if [ $n -eq 1 ]; then w={COLD}; else w=0; fi\n\
                 printf '{{\"type\":\"assistant\",\"message\":{{\"id\":\"m%s-%s\",\"usage\":\
                 {{\"input_tokens\":0,\"cache_read_input_tokens\":%s,\
                 \"cache_creation_input_tokens\":%s,\"output_tokens\":{PRODUCES}}}}}}}\\n' \
                 \"$$\" \"$s\" \"$r\" \"$w\" >> '{}'\n",
                path.display(),
                COLD = STANDIN_COLD,
                PRODUCES = STANDIN_PRODUCES,
            ),
        ),
        None => (String::new(), String::new()),
    };
    // ⚠⚠ THE WORK TURN IS COUNTED SEPARATELY FROM THE RECORD ROW. `n` moves on every turn a row is
    // written for — reflections and the closing account included — and *declare on the second WORK
    // turn* is not the same sentence. A peer that declared on a reflection would be answering the
    // question *where next* with the words *I am finished*.
    //
    // ⚠⚠⚠⚠⚠ **AND WHERE IT PUTS THE DECLARATION IS THE OTHER AXIS** — see [`Declares`]. Printing
    // leaves the marker only on the pane; stating leaves it only in the report, on the `idle` that
    // ENDS the turn (which is the event a real agent's hook carries `said` on). The two never
    // happen together in one arm, because a peer that did both could not say which reader answered.
    //
    // ⚠⚠⚠ **THE STATING ARM BRANCHES THE WHOLE COMMAND RATHER THAN BUILDING A FRAGMENT.** The
    // first draft put ` --said 'MILESTONE REACHED'` into a shell variable and expanded it unquoted
    // at the call — and an unquoted expansion splits on spaces WITHOUT removing the quotes, so the
    // daemon was handed three words with apostrophes in them, the report carried no `said`, and the
    // run simply never converged. Branching keeps the marker one argument.
    //
    // ⚠⚠⚠⚠ **THE STATING ARM STATES ON EVERY TURN, and only the CONTENT changes.** That is what a
    // real agent's hook does — `said` rides the event that ends any turn — and it is what makes the
    // road readable: the walk names the reader on an ORDINARY turn too, so a gate can see which
    // road a run is on without needing the declaring turn to say it. A peer that stated only when
    // it declared would leave the two arms indistinguishable until the very edge under test.
    let ends_turn = |report: &str| match declares {
        Declares::ByStating(turn) => format!(
            "if [ $t -ge {turn} ]; then a='MILESTONE REACHED'; else a='ACK'; fi\n\
             {report} --said \"$a\" >> '{log}' 2>&1\n",
            log = log.display(),
        ),
        _ => format!("{report} >> '{}' 2>&1\n", log.display()),
    };
    let answers = match declares {
        Declares::ByPrinting(turn) => format!(
            "if [ $t -ge {turn} ]; then printf 'MILESTONE REACHED\\n'; \
             else printf 'ACK\\n'; fi\n"
        ),
        // ⚠ The STATING arm's screen says `ACK` and nothing else — see [`Declares`], where that is
        // the whole point rather than a detail.
        Declares::Never | Declares::ByStating(_) => "printf 'ACK\\n'\n".to_owned(),
    };
    let work_turn_ends = ends_turn(&format!(
        "'{sprag}' report-agent idle --name sh --seq $s",
        sprag = sprag.display(),
    ));
    // ⚠ EVERY PATH IS QUOTED. They are `/tmp` names this test composes, and one of them carrying a
    // shell metacharacter killed the stand-in before its first line — see the caller's own note.
    //
    // ⚠⚠ THE FOUR CLAUSES IN THE `case`s ARE THE DOCUMENT'S OWN: `exactly:` is what every WORK
    // prompt ends with (`done_instruction`), *Summarise* is what `closing` asks a run that GOT
    // there, *where you got to* is what `stopping` asks one that did not, and `NEXT MILESTONE:` is
    // the label `reflecting` authors. See the type's doc for why they are spelled and what catches
    // each of them drifting.
    //
    // ⚠⚠⚠ `closing`'s CLAUSE IS NOT OPTIONAL ONCE A PEER CAN DECLARE. A stand-down and a
    // convergence both end by asking for an account, and a stand-in that did not recognise that
    // question would end the run on its wall clock with the milestone already banked — an
    // `exhausted(duration)` about a run that had finished.
    //
    // ⚠⚠⚠ THE REFLECTION CLAUSE IS FIRST AND ENDS IN `continue`, which is not tidiness: its prompt
    // may carry the work clause's words too, and a stand-in that fell through would answer one
    // question with the other's reply.
    format!(
        "export XDG_STATE_HOME='{state}'\n\
         stty -echo\n\
         s=1\n\
         n=0\n\
         t=0\n\
         '{sprag}' report-agent idle --name sh --seq $s >> '{log}' 2>&1\n\
         printf 'AGENT-READY\\n'\n\
         while IFS= read -r line; do\n\
         printf '%s\\n' \"$line\"\n\
         case \"$line\" in\n\
         *'NEXT MILESTONE:'*)\n\
         s=$((s+1))\n\
         '{sprag}' report-agent working --name sh --seq $s{asked}{states_record} >> '{log}' 2>&1\n\
         printf 'NEXT MILESTONE: {milestone}\\n'\n\
         printf 'NEXT REFERENCE: {reference}\\n'\n\
         s=$((s+1))\n\
         {writes_row}\
         '{sprag}' report-agent idle --name sh --seq $s >> '{log}' 2>&1\n\
         continue;;\n\
         esac\n\
         case \"$line\" in *exactly:*|*Summarise*|*'where you got to'*) ;; *) continue;; esac\n\
         t=$((t+1))\n\
         s=$((s+1))\n\
         '{sprag}' report-agent working --name sh --seq $s{asked}{states_record} >> '{log}' 2>&1\n\
         {answers}\
         s=$((s+1))\n\
         {writes_row}\
         {work_turn_ends}\
         done\n",
        sprag = sprag.display(),
        log = log.display(),
        state = state.display(),
    )
}

/// ⛔⛔⛔⛔⛔ **A HOST INTEGRATION TEST DRIVES A WHOLE RUN ON THE TURN CONTRACT THAT SHIPS** —
/// register item 730, and the wall every whole-run gate outside `sprag_plugin` had hit.
///
/// # ⚠⚠⚠⚠⚠ What could not be measured, and what that cost
///
/// [`AiLoopSpec::driving`](sprag_plugin::AiLoopSpec::driving) — the constructor the daemon builds
/// every loop through — sets [`INNER_SESSION_ENDS`](sprag_plugin::INNER_SESSION_ENDS), which is
/// [`DoneWhen::Settles`](sprag_plugin::DoneWhen::Settles). A `Settles` turn asks a supervisor, this
/// daemon supervises no `/bin/sh`, and the turn therefore never ended: the run pumped
/// `Working --Null--> Working` until a guardrail bit. So item 705's whole-run gate had to fall back
/// to [`DoneWhen::Exits`](sprag_plugin::DoneWhen::Exits) and say so, and **reflection, session
/// replacement, stand-down and the context ceiling stayed measurable only over `sprag_plugin`'s own
/// `#[cfg(test)]` supervisor double** — which is *measure the door the product calls* holding
/// everywhere in this workspace except at the layer that assembles the product.
///
/// # ⚠⚠⚠ The road, measured before it was built on — item 730's own instruction
///
/// The item said to measure whether `report-agent` can move `asked_seq` FIRST, because *"if it
/// cannot, this road is a dead end and what is MISSING is this item's answer"*. It could not: the
/// verb took four arguments and none of them was the prompt, so the key
/// [`AGENT_ASKED_KEY`](sprag_host::wire::AGENT_ASKED_KEY) — carried on the wire since register item
/// 441, accepted by the daemon since then, counted by the tracker since then — **had exactly one
/// writer in the whole tree, `sprag hook`.** The shipped turn contract was reachable only by the
/// agents whose hooks this binary ships. `--asked` is that missing door, and the control arm below
/// is the measurement kept.
///
/// # ⚠⚠ The two arms differ in ONE argument
///
/// Same daemon, same stand-in script, same brief, same spec — and one of them passes `--asked` on
/// the report that opens each turn. Everything else is identical, so what the two endings differ by
/// is the counter `Settles` pairs against and nothing else.
///
/// # ⚠⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **The contract really is the shipped one.** `spec.done_when` is read back and required to be
///   `INNER_SESSION_ENDS` — never assigned by this gate, because a gate that set the contract it is
///   measuring would be green over a constructor that had stopped choosing it.
/// * **The reports really landed.** The CLI's own answer for every report is captured and required
///   to say `accepted`; a stand-in whose `sprag` could not reach the daemon would be silent in
///   exactly the way a blocked turn is.
/// * **The ending is the DOCUMENT's budget and not the wall clock.** `exhausted(turns)` is the only
///   ending that says every turn completed; `exhausted(duration)` is what a peer that stopped
///   answering produces, and it is what the control arm produces.
#[test]
fn a_host_run_drives_its_turns_on_the_turn_contract_that_ships() {
    /// How many turns the DOCUMENT is given. Two rather than one, so *a turn ended* is measured as
    /// a repeatable thing rather than as one lucky edge.
    const TURNS: i64 = 2;
    /// The run's wall clock, and it is what the CONTROL arm spends: a run whose turns never end has
    /// no other ending. ⚠⚠ Sized off the measurement rather than guessed — the stated arm drives
    /// both its turns, the closing account and the whole ending in **~50ms** (printed below), so
    /// this is two hundred times what the claim needs and the only thing paying for it is the arm
    /// that is supposed to run out. ⚠ The control outlives it by the account window the driver
    /// grants a run whose clock has passed, which is `turn_within_ms` twice.
    const CEILING: Duration = Duration::from_secs(10);

    /// Drive one arm and hand back what the run ended as, what it walked, and what the stand-in's
    /// own reports came back as — the third value carrying the PANE's own text when the reports
    /// said nothing at all, because a stand-in that died has left its reason on the screen and
    /// nowhere else.
    fn ran(states_asked: bool) -> (sprag_plugin::Outcome, Vec<String>, String) {
        let (_host, sock) = spawn_host();
        let (driving, mut setup) = remote_driver(&sock);

        // ⚠⚠ NO THREAD ID IN THIS NAME, unlike its neighbours, and the reason is measured rather
        // than stylistic: `ThreadId` renders as `ThreadId(12)`, this path is interpolated into a
        // SHELL SCRIPT, and `dash` answered `Syntax error: "(" unexpected` — the stand-in died
        // before its first line and the pane's own screen is what said so. The pid plus the arm is
        // unique here because this test runs once per process and its two arms differ by the flag.
        let under =
            std::env::temp_dir().join(format!("sprag-730-{}-{states_asked}", std::process::id(),));
        let _ = std::fs::remove_dir_all(&under);
        std::fs::create_dir_all(&under).expect("a directory of this arm's own");
        let log = under.join("what-the-daemon-said-to-each-report");
        let state = under.join("state");
        std::fs::create_dir_all(&state).expect("a state home of this arm's own");

        // ⚠⚠ THE PANE IS BORN BY THE DAEMON, which is what puts `SPRAG_PANE` and the socket's
        // address into the stand-in's environment — so its `report-agent` takes no argument naming
        // either, exactly as a real agent's hook takes none.
        let pane = spawn_pane(
            &mut setup,
            json!({
                SPAWN_CMD_KEY: [
                    "/bin/sh",
                    "-c",
                    standin_that_reports_itself(
                        Path::new(env!("CARGO_BIN_EXE_sprag")),
                        &log,
                        &state,
                        states_asked,
                        // ⚠ This run never reflects (`reflect_every` equals the budget and the
                        // budget guard is above the cadence), so the clause is inert here — named
                        // rather than left blank so a run that DID reach it would be diagnosable.
                        ("this run does not reflect", "nor does it carry a reference"),
                        // ⚠ AND IT KEEPS NO RECORD. This gate's subject is the turn CONTRACT, and a
                        // readable spend would put a second moving part into a two-arm comparison.
                        None,
                        // ⚠ AND IT NEVER DECLARES: this run must end on its BUDGET, so a peer that
                        // said the marker would end it a different way and measure something else.
                        Declares::Never,
                    ),
                ],
                SPAWN_COLS_KEY: 80,
                SPAWN_ROWS_KEY: 16,
            }),
        );

        let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
            std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
        let brief = sprag_plugin::Brief {
            north_star: "the stand-in answers, and says so through the CLI".to_string(),
            milestone: "two turns end on the contract that ships".to_string(),
            reference: "register item 730".to_string(),
            closing_rules: None,
            context_ceiling: None,
            reflect_after_refusals: None,
            milestone_check: None,
            service: None,
            max_turns: Some(sprag_plugin::Counted::Of(TURNS)),
            // ⚠ EQUAL to the turn budget, which keeps `reflecting` unreachable: `judging` tests the
            // budget first. A reflection would replace the session and this gate's subject is the
            // TURN, not the lifecycle the turn makes measurable.
            reflect_every: Some(TURNS),
            screen_rules: None,
            may_answer: None,
            await_person_ms: Some(0),
            handback_still_ms: None,
            hold_within_ms: Some(3_600_000),
            ready_timeout_ms: Some(15_000),
            // ⚠ Sixty times the whole stated run, and it is paid TWICE by the control arm alone —
            // the account window a run whose clock has passed is granted. See `CEILING`.
            turn_within_ms: Some(3_000),
        };
        // ⚠⚠⚠⚠⚠ `done_when` IS NOT SET HERE, and that is the claim rather than a convenience: this
        // is the constructor `plugins.rs` builds every requested loop through, and what it chooses
        // is what a live run gets.
        let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
        // ⚠ A `/bin/sh` peer paints only once it holds a whole LINE, so a delivery cannot be
        // confirmed on screen before the newline that submits it — item 705's fixture, same reason.
        spec.shows_the_prompt = false;
        assert_eq!(
            spec.done_when,
            sprag_plugin::INNER_SESSION_ENDS,
            "⚠⚠⚠⚠⚠ THE PREMISE: the constructor the daemon uses must still choose the contract \
             this gate is about, or the run below is measuring something else entirely",
        );

        let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
            .expect("a well-briefed loop over a live pane starts");
        let progress = sprag_plugin::ProgressCell::default();
        let began = std::time::Instant::now();
        let outcome = Driver::new(Guardrails {
            max_iterations: 4_000,
            max_cost: None,
            max_duration: Some(CEILING),
        })
        .reporting_to(std::sync::Arc::clone(&progress))
        .run(&mut loops, &driving, &RunContext::uncancellable());
        // ⚠⚠ PRINTED RATHER THAN ASSERTED, on this file's own rule about numbers a round would
        // otherwise have to raise a bound to read: what the STATED arm costs is what says how much
        // headroom `CEILING` has, and the control's cost IS `CEILING` by construction.
        println!(
            "== item 730, arm states_asked={states_asked}: {:?}, {} iteration(s), in {:?} ==",
            outcome.state,
            outcome.iterations,
            began.elapsed(),
        );
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        // ⚠⚠ AND WHAT THE PANE ITSELF SHOWS, when the reports came back empty. A stand-in that
        // could not start leaves its reason there and in no other place this test can reach, and a
        // premise failure that could not quote it sent the first run of this gate looking at the
        // wrong layer.
        let said = if said.trim().is_empty() {
            format!(
                "(no report reached the daemon; the pane shows: {:?})",
                sprag_plugin::PaneAccess::pane_collapsed(&driving, pane).unwrap_or_default(),
            )
        } else {
            said
        };
        let _ = std::fs::remove_dir_all(&under);
        (outcome, walk, said)
    }

    let (stated, walk, said) = ran(true);

    // ── ⚠⚠ THE PREMISE: the stand-in's own reports really reached the daemon ─────────────────
    assert!(
        said.contains("accepted"),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: not one of the stand-in's reports was accepted, so the turn \
         below did not end because a peer came back to rest — it ended, or failed to, for a reason \
         this gate is not about. A stand-in whose `sprag` cannot reach the daemon looks exactly \
         like a peer that never answered. What the CLI said: {said:?}; the walk: {walk:?}",
    );
    assert!(
        !said.contains("REFUSED"),
        "⚠⚠ and none of them was refused as a replay — the stand-in's own `--seq` must rise, or \
         some of its turns were never counted at all. What the CLI said: {said:?}",
    );

    // ── ⛔⛔⛔ THE CLAIM ──────────────────────────────────────────────────────────────────────
    assert_eq!(
        stated.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 730: a whole run driven from OUTSIDE `sprag_plugin`, over a real \
         daemon, on the turn contract the shipped constructor chooses, must spend the turns its \
         document was given. An `exhausted(duration)` here is the wall clock ending a run whose \
         turns never ended — which is the state this layer was in for every gate before this one, \
         and the reason reflection, session replacement, stand-down and the context ceiling are \
         measured only over a double. The walk: {walk:?}",
    );
    assert_eq!(
        stated.banked.as_ref().map(|banked| banked.completed),
        Some(u32::try_from(TURNS).expect("a small budget fits a u32")),
        "⚠⚠⚠ and the DOCUMENT must have counted every one of them — `turns` moves on `turn.done` \
         and on nothing else, so this is the same fact the walk shows, read off the machine rather \
         than off prose. The walk: {walk:?}",
    );
    let ended: Vec<&String> = walk
        .iter()
        .filter(|note| note.contains("--TurnDone--> Judging"))
        .collect();
    assert!(
        ended.len() >= usize::try_from(TURNS).expect("a small budget fits a usize"),
        "⚠⚠⚠ and the walk must show each of them ending. Got {}: {walk:?}",
        ended.len(),
    );

    // ── ⛔⛔ THE CONTROL, WHICH IS ITEM 730's ③ KEPT AS A GATE ────────────────────────────────
    //
    // The same everything, minus the one argument. `asked_seq` then never moves, `Settles`' fourth
    // term is false for ever at a pane whose rest is its own statement, and the run walks to its
    // wall clock with its turn budget untouched. This is what the whole layer looked like before
    // `--asked` existed, and it is why the arm above is a claim about the product rather than about
    // a fixture that happened to work.
    let (silent, control_walk, control_said) = ran(false);
    assert!(
        control_said.contains("accepted"),
        "⚠⚠⚠⚠ THE CONTROL'S OWN PREMISE: its reports must land too, or the two arms differ in \
         whether the CLI ran at all rather than in what it stated. What the CLI said: \
         {control_said:?}",
    );
    assert_eq!(
        silent.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Duration),
        "⛔⛔⛔⛔ THE CONTROL: a reporter that states no prompt must NOT be able to end a `Settles` \
         turn. If this arm now spends its turn budget, `asked_seq` is being moved by something \
         other than a stated prompt — and the claim above is then true of a run that would have \
         passed without item 730's repair. The walk: {control_walk:?}",
    );
    assert_eq!(
        silent.banked.as_ref().map(|banked| banked.completed),
        Some(0),
        "⚠⚠ and it must have banked NO turn at all: the peer answered on its screen every time, \
         and the contract is what refused to call any of it a completed turn. The walk: \
         {control_walk:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A HOST RUN REPLACES ITS OWN INNER SESSION, AND KEEPS WORKING** — the first time the
/// loop's session lifecycle has been walked outside `sprag_plugin`, and what register item 730
/// unblocked.
///
/// # ⚠⚠⚠⚠⚠ Why the lifecycle had never been measured here
///
/// `reflecting → reviewing → restarting → resuming → priming` is the sequence that makes this loop
/// more than one agent's context wearing a longer life: the session is CLOSED, a fresh one is
/// spawned in its place, and the run briefs it with what the last one said should happen next. Every
/// gate over it was a `sprag_plugin` unit test against that crate's `#[cfg(test)]` supervisor
/// double, because a host test could not end a single `Settles` turn — so nothing had ever driven a
/// replacement through the daemon that actually respawns the pane
/// ([`RESPAWN_ACTION`](sprag_host::wire::RESPAWN_ACTION), over the socket).
///
/// # ⚠⚠⚠ What each leg of the sequence needs, and what would silently skip it
///
/// * **`reflecting`** is a turn, and its answer is READ BACK off the pane rather than reported. A
///   peer that ends the turn saying nothing takes `reflect.none` — a legal, quiet ending in which
///   the session is KEPT. So *the run reached `reflecting`* and *the run replaced its session* are
///   two different facts and this gate asserts the second.
/// * **`reviewing`** decides. With no `context_ceiling` authored and a peer whose spend nothing can
///   read, it takes the `nobody_could_say` door to `restarting` — the document's own *"a caller who
///   has not thought about capacity is not given a number somebody guessed for them"*.
/// * **`restarting`** is the one that needs a real daemon: `PaneLifecycle::respawn` closes the pane
///   and opens another running the same command, and the fresh pane's id is a NEW one.
/// * **`resuming`** waits for that pane to become an agent — which is the stand-in's own boot
///   report, made over the CLI in a pane it has just been born into.
///
/// # ⚠⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **The replacement is a DIFFERENT pane.** `Plugin::driving` is read before and after and the
///   two must differ — a run that walked the states while staying on one pane has replaced nothing,
///   and the walk alone cannot tell those apart.
/// * **The fresh session really worked.** Turns must be banked AFTER the replacement, not just
///   before it: a run that respawned and then stalled would still show every edge this gate names.
/// * **The successor was the AGENT's**, not the brief's. The milestone the replacement is briefed
///   with must be the one the stand-in named — `reflect.applied` carries the agent's answer and
///   `reflect.none` carries nothing, and only the first of those replaces a session for a reason.
#[test]
fn a_host_run_replaces_its_inner_session_and_the_fresh_one_works() {
    /// The document's turn budget. Four rather than three: two turns before the cadence comes
    /// round and **two after the replacement**, so *the fresh session works* is a repeated fact.
    const TURNS: i64 = 4;
    /// The cadence, deliberately BELOW the budget — which is the whole staging. Equal (item 730's
    /// gate) keeps `reflecting` unreachable, because `judging` tests the budget first.
    const EVERY: i64 = 2;
    /// What the stand-in names when it is asked where the work goes next.
    ///
    /// ⚠⚠ NEITHER STRING MAY APPEAR IN THE PROMPT THAT ASKS FOR IT. `OuterLoop::proposed` discounts
    /// a candidate row the delivered prompt contains — the echo rule, which exists because the
    /// prompt names the label mid-sentence and a terminal wraps where it likes — so an answer
    /// borrowed from the question would be read as the loop's own words and thrown away.
    const NEXT: &str = "the successor the stand-in itself named";
    /// Its other half, on the same terms.
    const CARRY: &str = "read this rather than the checkpoint it replaced";
    /// The run's wall clock. The whole of item 730's gate is ~50ms; this one buys a respawn and a
    /// second agent coming up, so it is generous — and an `exhausted(duration)` is a red here.
    const CEILING: Duration = Duration::from_secs(60);

    let (_host, sock) = spawn_host();
    let (driving, mut setup) = remote_driver(&sock);

    // ⚠ NO THREAD ID IN THIS NAME — it is interpolated into a shell script, and `ThreadId(12)`
    // killed the stand-in of item 730's gate before its first line. See that gate's own note.
    let under = std::env::temp_dir().join(format!("sprag-replace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&under);
    std::fs::create_dir_all(&under).expect("a directory of this run's own");
    let log = under.join("what-the-daemon-said-to-each-report");
    let state = under.join("state");
    let record = under.join("what-the-standin-says-it-is-writing.jsonl");
    std::fs::create_dir_all(&state).expect("a state home of this run's own");

    // ⚠⚠⚠⚠⚠ **THE PANE SET IS ASKED OF THE DAEMON, AND THIS READER IS A REPAIR.** The first draft
    // read `Plugin::driving` after the run and compared it with the pane the run began on — and
    // that is VACUOUS: `AiLoop::driving`'s own doc says the two endings that follow a completed
    // turn answer `None`, so the comparison was `None != Some(pane)` and passed for free. Measured
    // rather than reasoned about: a mutation making `replace` keep the SAME pane left this gate
    // GREEN. The daemon's own list cannot be fooled that way — `respawn` is *a pane was born and
    // another died*, in the daemon's words.
    let listed = |conn: &mut HostConn| -> Vec<u64> {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("panes query")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["id"].as_u64())
        .collect()
    };

    let pane = spawn_pane(
        &mut setup,
        json!({
            SPAWN_CMD_KEY: [
                "/bin/sh",
                "-c",
                standin_that_reports_itself(
                    Path::new(env!("CARGO_BIN_EXE_sprag")),
                    &log,
                    &state,
                    true,
                    (NEXT, CARRY),
                    // ⚠⚠⚠ THE SAME FILE ACROSS THE REPLACEMENT, and that is a stronger staging than
                    // a fresh one: it removes the alternative explanation. With a NEW record the
                    // successor's first turn could read `Unmeasured` because the total went
                    // BACKWARDS (two records compared), which is a different arm of `Made`. Sharing
                    // the file means the reading is readable, larger and still `Unmeasured` — which
                    // only the baseline having been dropped can explain.
                    Some(&record),
                    // ⚠ NEVER DECLARES — this run's reflection is brought on by the CADENCE, and a
                    // declared milestone would reach `reflecting` by a different edge.
                    Declares::Never,
                ),
            ],
            SPAWN_COLS_KEY: 80,
            SPAWN_ROWS_KEY: 16,
        }),
    );
    let before_panes = listed(&mut setup);
    assert!(
        before_panes.contains(&pane.0),
        "⚠⚠ THE PREMISE: the daemon must list the pane this run is about to drive, or the reading \
         below is about a set this test cannot see. Panes: {before_panes:?}",
    );

    let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
        std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
    let brief = sprag_plugin::Brief {
        north_star: "the stand-in keeps working across a replacement".to_string(),
        milestone: "the checkpoint this run starts on".to_string(),
        reference: "register item 730's fixture".to_string(),
        closing_rules: None,
        // ⚠⚠ NOT AUTHORED, and that is the door this run takes: the document's own answer to an
        // unauthored ceiling is that every reflection replaces the session, and `reviewing` says so
        // in the `restart_reason` it writes. A number here would make this gate about capacity.
        context_ceiling: None,
        reflect_after_refusals: None,
        milestone_check: None,
        service: None,
        max_turns: Some(sprag_plugin::Counted::Of(TURNS)),
        reflect_every: Some(EVERY),
        screen_rules: None,
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        hold_within_ms: Some(3_600_000),
        ready_timeout_ms: Some(15_000),
        turn_within_ms: Some(5_000),
    };
    let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
    spec.shows_the_prompt = false;
    assert_eq!(
        spec.done_when,
        sprag_plugin::INNER_SESSION_ENDS,
        "⚠⚠⚠⚠ THE PREMISE: the constructor the daemon uses must still choose the shipped turn \
         contract — a replacement driven on some other contract is not the lifecycle a live run has",
    );

    let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
        .expect("a well-briefed loop over a live pane starts");
    let progress = sprag_plugin::ProgressCell::default();
    let began = std::time::Instant::now();
    let outcome = Driver::new(Guardrails {
        max_iterations: 8_000,
        max_cost: None,
        max_duration: Some(CEILING),
    })
    .reporting_to(std::sync::Arc::clone(&progress))
    .run(&mut loops, &driving, &RunContext::uncancellable());
    let walk: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|step| step.note.clone())
        .collect();
    let after_panes = listed(&mut setup);
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    println!(
        "== a host run replacing its session: {:?}, {} iteration(s), in {:?} ==",
        outcome.state,
        outcome.iterations,
        began.elapsed(),
    );
    let _ = std::fs::remove_dir_all(&under);

    // ── ⚠⚠ THE PREMISES ──────────────────────────────────────────────────────────────────────
    assert!(
        said.contains("accepted") && !said.contains("REFUSED"),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the stand-in's own reports did not all land, so a turn that \
         did not end below is this fixture's fault rather than the product's. What the CLI said: \
         {said:?}; the walk: {walk:?}",
    );
    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
        "⚠⚠⚠⚠ THE PREMISE FAILED: this run did not spend the turns its document was given, so \
         everything below is about a run that stalled. An `exhausted(duration)` is a turn that \
         never ended — item 730's own subject, one layer down. The walk: {walk:?}",
    );

    // ── ⛔⛔⛔ THE CLAIM: THE SESSION WAS REPLACED ────────────────────────────────────────────
    let replaced: Vec<&String> = walk
        .iter()
        .filter(|note| note.contains("Restarting --SessionReplaced--> Resuming"))
        .collect();
    assert_eq!(
        replaced.len(),
        1,
        "⛔⛔⛔⛔⛔ A HOST RUN MUST BE ABLE TO REPLACE ITS INNER SESSION. This is the lifecycle the \
         whole loop is built around — a fresh agent, briefed with what the last one said should \
         happen next — and until item 730 no test outside `sprag_plugin` could reach it, because \
         none of them could end a single turn. The walk: {walk:?}",
    );
    for leg in [
        "Judging --Judge--> Reflecting",
        "Reflecting --ReflectApplied--> Reviewing",
        "Reviewing --",
        "Resuming --SessionReady--> Priming",
    ] {
        assert!(
            walk.iter().any(|note| note.contains(leg)),
            "⚠⚠⚠ and every leg of the sequence must be walked, not just its ends — `{leg}` is \
             missing, so the run reached a replacement by some road this gate does not describe. \
             The walk: {walk:?}",
        );
    }

    // ── ⚠⚠⚠ AND IT REALLY IS A DIFFERENT PANE, BY THE DAEMON'S OWN COUNT ────────────────────
    assert!(
        !after_panes.contains(&pane.0),
        "⛔⛔⛔⛔⛔ THE RUN WALKED THE STATES AND STAYED ON ONE PANE. `restarting` CLOSES the inner \
         session and opens another in its place — the daemon calls it *a pane was born and another \
         died* — and a walk cannot tell that from a machine that merely visited the states. The \
         pane this run began on ({}) is still listed. Panes before: {before_panes:?}; after: \
         {after_panes:?}",
        pane.0,
    );
    let fresh: Vec<&u64> = after_panes
        .iter()
        .filter(|id| !before_panes.contains(id))
        .collect();
    assert_eq!(
        fresh.len(),
        1,
        "⚠⚠⚠ and exactly one pane must have been BORN — the replacement. A run that closed its \
         session without opening another has ended the loop rather than replaced its agent, and \
         the assertion above cannot tell those apart. Panes before: {before_panes:?}; after: \
         {after_panes:?}",
    );

    // ── ⚠⚠⚠ AND THE SUCCESSOR WAS THE AGENT'S OWN ANSWER ────────────────────────────────────
    //
    // `reflect.applied` is the edge that carries what the agent named; `reflect.none` keeps the
    // session and carries nothing. Asserting the milestone MOVED is what separates a replacement
    // the agent steered from one that happened to fire.
    //
    // ⚠⚠ READ OFF THE PROMPT THE FRESH SESSION IS GREETED WITH, which is a stronger reading than
    // the datamodel slot: `priming` composes `start` out of the milestone, so this says the answer
    // reached the sentence the replacement was actually spoken to with.
    let greeting = loops
        .authored()
        .expect("a machine mid-run answers with its strings")
        .start;
    assert!(
        greeting.contains(NEXT),
        "⛔⛔⛔ THE REPLACEMENT WAS NOT BRIEFED WITH WHAT THE AGENT SAID. `reflecting` exists so the \
         party doing the work says where it goes next — a run that replaced its session and kept \
         the caller's original checkpoint is bounded by one agent's context wearing a longer life, \
         which is this state's own reason for existing. What it was greeted with: {greeting:?}; \
         the walk: {walk:?}",
    );

    // ── ⛔⛔ AND THE FRESH SESSION DID THE WORK ──────────────────────────────────────────────
    //
    // The sharp one: a run that respawned and then stalled walks every edge above and banks
    // nothing after it. `turns` is the DOCUMENT's counter and moves only on `turn.done`.
    let after: usize = walk
        .iter()
        .skip_while(|note| !note.contains("Restarting --SessionReplaced--> Resuming"))
        .filter(|note| note.contains("--TurnDone--> Judging"))
        .count();
    assert!(
        after >= 2,
        "⛔⛔⛔⛔ THE FRESH SESSION DID NOT WORK. Replacing a session is only worth anything if the \
         replacement then takes turns — and a run that respawned and stalled walks every edge \
         asserted above. Turns judged after the replacement: {after}. The walk: {walk:?}",
    );
    assert_eq!(
        outcome.banked.as_ref().map(|banked| banked.completed),
        Some(u32::try_from(TURNS).expect("a small budget fits a u32")),
        "⚠⚠⚠ and the run must have banked its whole budget across the two sessions — the count is \
         the RUN's and a replacement does not reset it. The walk: {walk:?}",
    );

    // ── ⛔⛔⛔⛔ AND REGISTER ITEM 719's BASELINE IS DROPPED WITH THE SESSION ──────────────────
    //
    // `Made` is the difference between what a session had produced when its LAST turn ended and
    // what it has produced now, and the earlier reading is a field of `Session` precisely so that
    // `Session::replacing` forgets it. Carried over, the replacement's first turn would be measured
    // against its PREDECESSOR's output — which is the failure that field's neighbours already
    // refuse in their own words (*"the spend would freeze … and look like an agent doing nothing"*).
    //
    // ⚠⚠⚠ THE PREMISE FIRST, because `Unmeasured` says nothing at all and a run whose record could
    // not be read answers it for every turn. The turns BEFORE the replacement must have produced a
    // measured amount, or the claim below is satisfied by an instrument that never worked.
    let produced: Vec<&String> = walk
        .iter()
        .filter(|note| note.contains("tokens of output"))
        .collect();
    assert!(
        produced.len() >= 2,
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: this run never measured what a single turn produced, so *the \
         baseline was dropped* below is true of an instrument that read nothing. `--transcript` is \
         what makes a reporter's spend readable; a run that states none reads zeros the document \
         defines as *do not decide on this*. The walk: {walk:?}",
    );
    // ⚠ The turn ENDINGS, in order, each labelled with whether the loop could measure what it
    // produced. `Made::Unmeasured` says nothing, which is the absence this reads.
    let measured: Vec<bool> = walk
        .iter()
        .skip_while(|note| !note.contains("Restarting --SessionReplaced--> Resuming"))
        .filter(|note| note.contains("--TurnDone--> Judging"))
        .map(|note| note.contains("tokens of output") || note.contains("PRODUCED NOTHING"))
        .collect();
    assert_eq!(
        measured.first(),
        Some(&false),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 719: THE REPLACEMENT'S FIRST TURN WAS MEASURED AGAINST ITS \
         PREDECESSOR. The record is the same file and it is readable and growing — the premise \
         above says so — and a fresh session has no earlier reading of its OWN, so the only honest \
         answer is *nothing could be compared*. A number here is the successor being credited with, \
         or accused of, what the session before it wrote. The walk: {walk:?}",
    );
    assert_eq!(
        measured.get(1),
        Some(&true),
        "⚠⚠⚠⚠ AND THE TURN AFTER IT MUST BE MEASURED, which is what stops the claim above being \
         vacuous: if the record had simply become unreadable at the replacement, EVERY later turn \
         would answer *nothing could be compared* too. Readable, growing, and unmeasured for \
         exactly one turn is a dropped baseline and nothing else. The walk: {walk:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A SESSION THAT HAS FILLED UP HANDS OVER, AND ONE THAT HAS NOT KEEPS WORKING** —
/// register item 424(b)'s door, driven end to end over a real daemon for the first time.
///
/// # ⚠⚠⚠⚠⚠ Why this door had never been crossed outside `sprag_plugin`
///
/// `context_ceiling` is decided on `context` — what the inner session's own record says its last
/// billed request was charged to read — and until register item 730 gave `report-agent` a
/// `--transcript`, **nothing but a sprag-installed hook could make that number anything but zero.**
/// A zero is the document's *do not decide on this*, so every host-layer run took the
/// `context_ceiling <= 0` doors whatever ceiling was authored, and the capacity edge was
/// unreachable by construction.
///
/// # ⚠⚠⚠ The two arms differ in ONE NUMBER, and take opposite doors
///
/// Same stand-in, same record, same growth, same budget — and one authored ceiling below the
/// reading the session reaches, one above it.
///
/// | arm | `reviewing` says | the session |
/// |---|---|---|
/// | ceiling BELOW the reading | `capacity` | is **replaced** |
/// | ceiling ABOVE it | (no restart reason at all) | is **kept**, and the run re-primes on it |
///
/// ⚠⚠ **THE CONTROL IS THE INTERESTING HALF.** Every other door out of `reviewing` restarts; this
/// one is the only edge in the state that does NOT, so it is the only thing that can show the
/// ceiling DECIDING rather than the loop replacing its session the way it always does. The pane set
/// is read off the daemon before and after: kept means *no pane was born and none died*.
///
/// # ⚠⚠⚠⚠ And the ceiling outranks the cadence, which the arms also show
///
/// `judging` tests the ceiling ABOVE `turns_since_reflect >= reflect_every`, so the filling arm
/// reflects on turn two while its cadence would not have come round until turn three. That is the
/// item's own claim — *a session must be able to hand over before the window fills, whatever the
/// cadence says* — and it falls out of the same single number.
///
/// # ⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **The reading really reached the document.** `context` is read back off the datamodel and must
///   be the record's own number, never `0` — a run that read nothing takes a `<= 0` door and would
///   satisfy *the session was replaced* for a reason that has nothing to do with capacity.
/// * **The economics were EVALUATED and false.** `reviewing`'s first door restarts on
///   `context - floor >= 20 * cold`, above the capacity edge, so a control that kept its session
///   because a term was zero would prove nothing about the term under test. The arithmetic is
///   recomputed here from the fixture's own constants.
/// * **Both arms really reached `reviewing`.** A run that never reflected takes neither door.
#[test]
fn a_session_that_has_filled_up_hands_over_and_one_that_has_not_keeps_working() {
    /// The document's turn budget. Three: two before the decision and one after it, so the run
    /// ends on its budget rather than on the clock in BOTH arms.
    const TURNS: i64 = 3;
    /// The cadence. Below the budget, so the arm that never fills up still reaches `reviewing` —
    /// which is the only way its door can be read at all.
    const EVERY: i64 = 2;
    /// [`standin_that_reports_itself`]'s own schedule, spelled once: its *n*th billed request reads
    /// `4000 + 4000n`. ⚠ A function of `n` rather than two digits, because both numbers this gate
    /// reasons with come off it — a ceiling written against a REMEMBERED reading is the shape this
    /// workspace has paid for repeatedly.
    const fn reads_at(n: i64) -> i64 {
        4_000 + 4_000 * n
    }
    /// What the SECOND billed request reads — where the schedule has got to when the decision is
    /// taken, because `context` is assigned on each judged turn and the reflection is the third.
    const READING: i64 = reads_at(2);
    /// A ceiling the session has PASSED. Below the reading, so `context >= context_ceiling`.
    const FULL: i64 = READING - 2_000;
    /// A ceiling it is nowhere near. ⚠ It also has to leave the ECONOMIC door shut, which is
    /// asserted below rather than assumed.
    const ROOMY: i64 = READING * 10;

    /// Drive one arm at `ceiling` and hand back its ending, its walk, what the document held as
    /// `context` when it decided, and the daemon's pane set before and after.
    fn ran(
        ceiling: i64,
    ) -> (
        sprag_plugin::Outcome,
        Vec<String>,
        Option<i64>,
        Vec<u64>,
        Vec<u64>,
    ) {
        let (_host, sock) = spawn_host();
        let (driving, mut setup) = remote_driver(&sock);

        // ⚠ NO THREAD ID — it is interpolated into a shell script. See the neighbouring gate.
        let under =
            std::env::temp_dir().join(format!("sprag-capacity-{}-{ceiling}", std::process::id()));
        let _ = std::fs::remove_dir_all(&under);
        std::fs::create_dir_all(&under).expect("a directory of this arm's own");
        let log = under.join("what-the-daemon-said-to-each-report");
        let state = under.join("state");
        let record = under.join("what-the-standin-says-it-is-writing.jsonl");
        std::fs::create_dir_all(&state).expect("a state home of this arm's own");

        let listed = |conn: &mut HostConn| -> Vec<u64> {
            conn.call(
                "scene/query",
                json!({ "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("panes query")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["id"].as_u64())
            .collect()
        };

        let pane = spawn_pane(
            &mut setup,
            json!({
                SPAWN_CMD_KEY: [
                    "/bin/sh",
                    "-c",
                    standin_that_reports_itself(
                        Path::new(env!("CARGO_BIN_EXE_sprag")),
                        &log,
                        &state,
                        true,
                        ("the next checkpoint this arm names", "and the line it carries"),
                        Some(&record),
                        // ⚠ NEVER DECLARES: the ceiling is what must bring this run to `reviewing`,
                        // and a milestone would take it there by an edge above the one under test.
                        Declares::Never,
                    ),
                ],
                SPAWN_COLS_KEY: 80,
                SPAWN_ROWS_KEY: 16,
            }),
        );
        let before = listed(&mut setup);

        let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
            std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
        let brief = sprag_plugin::Brief {
            north_star: "the stand-in works until its window is the question".to_string(),
            milestone: "the checkpoint this run starts on".to_string(),
            reference: "register item 424(b)".to_string(),
            closing_rules: None,
            // ⚠⚠⚠⚠⚠ THE ONE NUMBER THE TWO ARMS DIFFER BY, and it is AUTHORED THROUGH THE BRIEF —
            // never written into the datamodel by hand. A gate that reached its subject by a door
            // no caller has is measuring itself (register item 492).
            context_ceiling: Some(ceiling),
            reflect_after_refusals: None,
            milestone_check: None,
            service: None,
            max_turns: Some(sprag_plugin::Counted::Of(TURNS)),
            reflect_every: Some(EVERY),
            screen_rules: None,
            may_answer: None,
            await_person_ms: Some(0),
            handback_still_ms: None,
            hold_within_ms: Some(3_600_000),
            ready_timeout_ms: Some(15_000),
            turn_within_ms: Some(5_000),
        };
        let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
        spec.shows_the_prompt = false;

        let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
            .expect("a well-briefed loop over a live pane starts");
        let progress = sprag_plugin::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 8_000,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(std::sync::Arc::clone(&progress))
        .run(&mut loops, &driving, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let held = loops.context();
        let after = listed(&mut setup);
        println!(
            "== ceiling {ceiling}: {:?} in {} iteration(s), context {held:?}, panes {before:?} -> \
             {after:?} ==",
            outcome.state, outcome.iterations,
        );
        let _ = std::fs::remove_dir_all(&under);
        (outcome, walk, held, before, after)
    }

    // ── ⚠⚠ THE ARITHMETIC PREMISE, BEFORE EITHER RUN ────────────────────────────────────────
    //
    // `reviewing` tests the ECONOMIC door first, and it restarts too — so a control that kept its
    // session while that door was open would be measuring nothing. This is the guard's own
    // expression over the fixture's own constants.
    let floor = reads_at(1);
    let cold = i64::try_from(STANDIN_COLD).expect("a toll fits an i64");
    assert!(
        READING - floor < 20 * cold,
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: this stand-in's reading already pays for a restart on the \
         ECONOMIC door ({READING} - {floor} >= 20 * {cold}), which `reviewing` tests ABOVE the one \
         under test — so the arm that is supposed to keep its session would restart for a reason \
         this gate is not about, and the arm that is supposed to restart would prove nothing",
    );

    let (filled, full_walk, full_context, full_before, full_after) = ran(FULL);
    let (roomy, roomy_walk, roomy_context, roomy_before, roomy_after) = ran(ROOMY);

    // ── ⚠⚠ THE PREMISES ──────────────────────────────────────────────────────────────────────
    //
    // ⚠⚠⚠⚠⚠ **THE READING IS ASSERTED AS NON-ZERO, NOT AS A VALUE, and that is a repair.** The
    // first draft demanded the number the document held when it DECIDED — and `context` is a LEVEL
    // that every judged turn overwrites, so what a reader gets after the run is the LAST turn's
    // (measured: 20,000 and 24,000 against a decision taken at 12,000). That is the same trap the
    // neighbouring gate's `Plugin::driving` reading fell into, met a second time in one round:
    // **a level read after the run is not the level the run decided on.**
    //
    // What is left is enough, because the DOORS carry the rest: `capacity` fires only on
    // `context_ceiling > 0 && context > 0 && context >= context_ceiling`, and the control's edge
    // only on `context > 0 && context < context_ceiling`. The reading being real is the part the
    // door cannot say — a `0` takes a different door entirely — and that is what this asserts.
    assert!(
        full_context.is_some_and(|held| held > 0) && roomy_context.is_some_and(|held| held > 0),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the session's own record did not reach the document as \
         `context` on both arms — {full_context:?} and {roomy_context:?}. A `0` is what a run whose \
         reporter states no transcript reads, and it takes a `context_ceiling <= 0` door, which \
         restarts: the claim below would then be satisfied by a run that consulted no ceiling",
    );
    for (name, walk) in [("filled", &full_walk), ("roomy", &roomy_walk)] {
        assert!(
            walk.iter().any(|note| note.contains("--> Reviewing")),
            "⚠⚠⚠ THE PREMISE FAILED: the {name} arm never reached `reviewing`, so it took neither \
             of the doors this gate is about. The walk: {walk:?}",
        );
        assert_eq!(
            walk.iter()
                .filter(|note| note.contains("--> Reviewing"))
                .count(),
            1,
            "⚠⚠ and it must have decided EXACTLY ONCE, or *which door* is a question about several \
             decisions read as one. The walk: {walk:?}",
        );
    }

    // ── ⛔⛔⛔ THE CLAIM: A FULL SESSION HANDS OVER, NAMING CAPACITY ─────────────────────────
    assert!(
        full_walk
            .iter()
            .any(|note| note.contains("--> Restarting — capacity:")),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 424(b): a session whose reading has passed the ceiling its caller \
         authored must hand over, and `reviewing` must say WHY in that word. Every other door out \
         of that state also restarts, so a replacement alone says nothing — the reason is the \
         claim. The walk: {full_walk:?}",
    );
    assert!(
        full_after != full_before && full_after.len() == full_before.len(),
        "⚠⚠⚠ and the daemon must have seen it: ONE pane died and ONE was born, which is what a \
         respawn is and what a walk cannot distinguish from a machine visiting states. Panes \
         {full_before:?} -> {full_after:?}",
    );
    // ⚠ AND THE CEILING OUTRANKED THE CADENCE. `judging` tests it above `turns_since_reflect`, so
    // this arm reflected on the turn its window filled rather than waiting for its budget to come
    // round — which is the half of item 424(b) a cadence cannot buy.
    assert!(
        full_walk
            .iter()
            .any(|note| note.contains("--> Reflecting — capacity:")),
        "⚠⚠⚠ and it must have been the CEILING that sent it to reflect, not the cadence — the two \
         are different edges and only one of them can act before the count comes round. The walk: \
         {full_walk:?}",
    );

    // ── ⛔⛔ THE CONTROL: A SESSION WITH ROOM KEEPS WORKING ON THE SAME PANE ─────────────────
    assert!(
        !roomy_walk
            .iter()
            .any(|note| note.contains("--> Restarting")),
        "⛔⛔⛔⛔ THE CONTROL: a session that is nowhere near its ceiling must be KEPT. This is the \
         only edge in `reviewing` that does not restart, so it is the only thing that can show the \
         ceiling deciding rather than the loop doing what it always does. The walk: {roomy_walk:?}",
    );
    assert!(
        roomy_walk
            .iter()
            .any(|note| note.contains("Reviewing --") && note.contains("--> Priming")),
        "⚠⚠⚠ and it must have gone back to `priming` — re-briefed on the session it already had, \
         which is what *kept* means here. The walk: {roomy_walk:?}",
    );
    assert_eq!(
        roomy_after, roomy_before,
        "⛔⛔⛔ AND THE DAEMON MUST HAVE SEEN NOTHING HAPPEN. No pane was born and none died: this \
         is the reading that cannot be satisfied by a machine that merely visited the states, and \
         it is the same instrument the neighbouring gate needed after its first one turned out to \
         be vacuous",
    );

    // ── ⚠⚠ AND BOTH ARMS DID THE WORK THEY WERE BUDGETED ────────────────────────────────────
    assert_eq!(
        (
            filled.state,
            roomy.state,
            filled.banked.as_ref().map(|banked| banked.completed),
            roomy.banked.as_ref().map(|banked| banked.completed),
        ),
        (
            sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
            sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
            Some(u32::try_from(TURNS).expect("a small budget fits a u32")),
            Some(u32::try_from(TURNS).expect("a small budget fits a u32")),
        ),
        "⚠⚠⚠ both arms must spend their whole turn budget — a `duration` ending is a turn that \
         never ended, and a short count is a run that stopped working after it decided. Filled \
         walked {full_walk:?}; roomy walked {roomy_walk:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A RUN THAT HAS BEEN STOOD DOWN FINISHES ITS MILESTONE AND STOPS** — the last of the
/// four lifecycles register item 730 found measurable only over a double.
///
/// # ⚠⚠⚠⚠⚠ Why this ending is the one a person actually uses, and why it had no host gate
///
/// A stand-down is what somebody types at a loop they want to end WITHOUT losing the turn it is in
/// the middle of. [`STAND_DOWN_TAKES_EFFECT`](sprag_plugin::STAND_DOWN_TAKES_EFFECT) is the promise
/// the surface makes about it — *"it finishes the turn its agent is in the middle of and stops at
/// the next milestone, banking that work"* — and every gate over the edge that keeps that promise
/// ran against `sprag_plugin`'s `#[cfg(test)]` supervisor double, because no host test could end a
/// `Settles` turn until register item 730.
///
/// # ⚠⚠⚠ The two arms differ in ONE act, and the SAME declaration takes opposite doors
///
/// Same stand-in, same brief, same peer declaring the milestone on the same turn — and one of the
/// runs has been stood down before it starts. `judging` then reads
/// `_event.data.done && In('standing_down')` and goes to `closing`; without the order the same
/// declaration falls through to the plain `done` edge and goes to `reflecting`, which is the loop
/// carrying on. **Converged versus exhausted, off one call.**
///
/// ⚠⚠ **THE CADENCE AND THE CEILING ARE BOTH MADE UNREACHABLE ON PURPOSE** (`reflect_every` equals
/// the budget, no `context_ceiling` is authored). That is what makes the control's `reflecting`
/// EVIDENCE: with those two doors shut, the only edge left that reaches it is the one that requires
/// `_event.data.done` — so the control arriving there is the proof that the peer really declared,
/// and neither arm's claim rests on reading a marker off a screen in this test.
///
/// # ⚠⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **The order was really taken.** The document is asked, through
///   [`AiLoop::standing_down`](sprag_plugin::AiLoop::standing_down), and the two arms must disagree
///   — an order that never reached the machine leaves this gate comparing one run with itself.
/// * **The peer really declared, in BOTH arms.** The control's `reflecting` says so for it; the
///   stood-down arm's own `closing` edge requires the same `_event.data.done`, so a run that ended
///   for any other reason cannot carry the word this gate reads.
/// * **The work was BANKED**, which is the half of the promise a `converged` does not say by
///   itself: `banked` counts what the DOCUMENT completed and a stand-down that threw the turn away
///   would still report the same ending word.
#[test]
fn a_run_that_has_been_stood_down_finishes_its_milestone_and_stops() {
    /// The turn the peer declares on. Two rather than one, so the run is measured taking a turn it
    /// does NOT declare on first — a peer that declared immediately would leave *finishes the turn
    /// it is in the middle of* untested.
    const DECLARES_ON: u32 = 2;
    /// The document's budget, and it is what the CONTROL ends on. Reached only by an arm that keeps
    /// working after its milestone, which is the whole difference between the two.
    const TURNS: i64 = 3;

    /// Drive one arm, ordered or not, and hand back its ending, its walk, whether the document
    /// holds the order, and the daemon's pane set before and after.
    fn ran(ordered: bool) -> (sprag_plugin::Outcome, Vec<String>, bool, Vec<u64>, Vec<u64>) {
        let (_host, sock) = spawn_host();
        let (driving, mut setup) = remote_driver(&sock);

        // ⚠ NO THREAD ID — it is interpolated into a shell script. See the neighbouring gates.
        let under =
            std::env::temp_dir().join(format!("sprag-standdown-{}-{ordered}", std::process::id()));
        let _ = std::fs::remove_dir_all(&under);
        std::fs::create_dir_all(&under).expect("a directory of this arm's own");
        let log = under.join("what-the-daemon-said-to-each-report");
        let state = under.join("state");
        std::fs::create_dir_all(&state).expect("a state home of this arm's own");

        let listed = |conn: &mut HostConn| -> Vec<u64> {
            conn.call(
                "scene/query",
                json!({ "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("panes query")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["id"].as_u64())
            .collect()
        };

        let pane = spawn_pane(
            &mut setup,
            json!({
                SPAWN_CMD_KEY: [
                    "/bin/sh",
                    "-c",
                    standin_that_reports_itself(
                        Path::new(env!("CARGO_BIN_EXE_sprag")),
                        &log,
                        &state,
                        true,
                        ("what the control goes on to work toward", "and its reference"),
                        // ⚠ NO RECORD. Neither door here reads a spend, and a readable one would
                        // put the capacity edge back in play — a second moving part in a two-arm
                        // comparison that is about one act.
                        None,
                        // ⚠ BY PRINTING, which is what every host gate before `--said` could do —
                        // and it is right here: this gate's subject is the ORDER, so putting the
                        // declaration on the road the neighbouring gate is about would move two
                        // things at once.
                        Declares::ByPrinting(DECLARES_ON),
                    ),
                ],
                SPAWN_COLS_KEY: 80,
                SPAWN_ROWS_KEY: 16,
            }),
        );
        let before = listed(&mut setup);

        let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
            std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
        let brief = sprag_plugin::Brief {
            north_star: "the stand-in reaches a checkpoint and somebody has asked it to stop"
                .to_string(),
            milestone: "the checkpoint this run is working toward".to_string(),
            reference: "register item 730's fourth lifecycle".to_string(),
            closing_rules: None,
            // ⚠⚠ NOT AUTHORED, which SHUTS the capacity edge: `judging` tests it on
            // `context_ceiling > 0`. With the cadence shut too (below), the only door left into
            // `reflecting` is the one that requires a declared milestone — which is what makes the
            // control's ending evidence about the peer rather than about the loop's housekeeping.
            context_ceiling: None,
            reflect_after_refusals: None,
            milestone_check: None,
            service: None,
            max_turns: Some(sprag_plugin::Counted::Of(TURNS)),
            // ⚠ EQUAL to the budget, which is what shuts the cadence: `judging` tests the budget
            // above it, so `turns_since_reflect >= reflect_every` can never be the reason.
            reflect_every: Some(TURNS),
            screen_rules: None,
            may_answer: None,
            await_person_ms: Some(0),
            handback_still_ms: None,
            hold_within_ms: Some(3_600_000),
            ready_timeout_ms: Some(15_000),
            turn_within_ms: Some(5_000),
        };
        let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
        spec.shows_the_prompt = false;

        let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
            .expect("a well-briefed loop over a live pane starts");
        // ⚠⚠⚠⚠⚠ **THE ORDER IS GIVEN BEFORE THE RUN, and that is a constraint rather than a
        // preference.** `Driver::run` takes `&mut loops` for the whole run and
        // `AiLoop::stand_down` needs the same borrow, so there is no seam to reach in through from
        // this thread. It costs nothing the claim needs: the order is a STATE in the document's
        // orders region, and a run that carries it from its first pump meets the same guard on the
        // same edge as one that is given it half way. ⚠ What it does NOT measure is a stand-down
        // arriving mid-turn, which is the daemon's own channel and its own gate.
        if ordered {
            loops.stand_down();
        }
        let held = loops.standing_down();
        let progress = sprag_plugin::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 8_000,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(std::sync::Arc::clone(&progress))
        .run(&mut loops, &driving, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let after = listed(&mut setup);
        println!(
            "== stood down {ordered}: {:?} in {} iteration(s), banked {:?}, panes {before:?} -> \
             {after:?} ==",
            outcome.state, outcome.iterations, outcome.banked,
        );
        let _ = std::fs::remove_dir_all(&under);
        (outcome, walk, held, before, after)
    }

    let (stopped, stopped_walk, stopped_held, stopped_before, stopped_after) = ran(true);
    let (carried_on, carry_walk, carry_held, carry_before, carry_after) = ran(false);

    // ── ⚠⚠ THE PREMISES ──────────────────────────────────────────────────────────────────────
    assert_eq!(
        (stopped_held, carry_held),
        (true, false),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the two arms do not differ in whether the DOCUMENT holds the \
         order. `standing_down` reads the machine's own orders region, so this is the act having \
         landed rather than this test having intended it — and without the difference the whole \
         comparison below is one run measured against itself",
    );
    assert!(
        carry_walk
            .iter()
            .any(|note| note.contains("--> Reflecting")),
        "⚠⚠⚠⚠ THE PREMISE FAILED: the control never reflected. With no ceiling authored and the \
         cadence equal to the budget, the ONLY door left into `reflecting` is the one that requires \
         a declared milestone — so this is how the gate knows the peer really said it, in both \
         arms. Its walk: {carry_walk:?}",
    );

    // ── ⛔⛔⛔ THE CLAIM: THE ORDERED RUN CLOSED, NAMING THE ORDER ───────────────────────────
    assert!(
        stopped_walk
            .iter()
            .any(|note| note.contains("--> Closing") && note.contains("stood_down")),
        "⛔⛔⛔⛔⛔ A STOOD-DOWN RUN MUST STOP AT ITS MILESTONE, AND SAY THAT IS WHY. `judging` has \
         two edges a declared milestone can take and they differ only by `In('standing_down')` — so \
         a run that reached an ending without this word took the other one, and the order a person \
         gave did nothing. The walk: {stopped_walk:?}",
    );
    assert_eq!(
        stopped.state,
        sprag_plugin::OutcomeState::Converged,
        "⚠⚠⚠ and it must end CONVERGED — `STAND_DOWN_TAKES_EFFECT` promises a person that \
         `sprag runs` reports it converged and says somebody stood it down, and an ending of any \
         other word breaks that sentence. The walk: {stopped_walk:?}",
    );
    assert_eq!(
        stopped.banked.as_ref().map(|banked| banked.completed),
        Some(DECLARES_ON),
        "⛔⛔⛔⛔ AND THE WORK MUST BE BANKED, which is the half of the promise the ending word does \
         not carry: *it finishes the turn its agent is in the middle of and stops at the next \
         milestone, BANKING THAT WORK*. A stand-down that threw the declaring turn away reports the \
         same `converged`. The walk: {stopped_walk:?}",
    );
    assert_eq!(
        stopped_after, stopped_before,
        "⚠⚠ and no pane was born or died: a stand-down ENDS a run, where every door out of \
         `reviewing` replaces its session. Panes {stopped_before:?} -> {stopped_after:?}",
    );

    // ── ⛔⛔ THE CONTROL: THE SAME DECLARATION, UNORDERED, CARRIES ON ────────────────────────
    assert!(
        !carry_walk.iter().any(|note| note.contains("--> Closing")),
        "⛔⛔⛔⛔ THE CONTROL: without the order, the SAME declaration must not end the run. \
         `reflecting` is where a finished milestone goes when nobody has asked the loop to stop — \
         it asks what to work toward next — and a run that closed here would be ending on the \
         agent's word alone, which is the thing the order exists to authorise. The walk: \
         {carry_walk:?}",
    );
    assert_eq!(
        carried_on.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
        "⚠⚠⚠ and it must go on to spend its whole budget: *carries on* is a claim about work done \
         AFTER the milestone, and a run that stalled would satisfy *did not close* by doing \
         nothing. The walk: {carry_walk:?}",
    );
    assert_ne!(
        carry_after, carry_before,
        "⚠⚠ and its session was replaced, which is what `reflecting` leads to here — the pane set \
         moved where the stood-down arm's did not. Panes {carry_before:?} -> {carry_after:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A LOOP CONVERGES ON WHAT ITS AGENT SAYS, NOT ON WHAT ITS TERMINAL SHOWS** — register
/// item 441, and the last of the three keys `sprag hook` alone could send.
///
/// # ⚠⚠⚠⚠⚠ What the pane road costs, measured rather than argued
///
/// `OuterLoop::said_marker` asks the peer first and reads the terminal only where it has said
/// nothing. Item 441 is what the terminal is worth when it is the only road: on 2026-08-18, at
/// every judgement of a live run, the pane's whole logical-line count stood at **37 and never
/// moved** while the agent wrote reply after reply with the marker alone on a row — a full-screen
/// agent repaints instead of scrolling, so nothing is shed and, once its composer settles, nothing
/// new stands above the cursor. **Nine judged turns read identically to a peer that said nothing.**
///
/// # ⛔⛔⛔ And every host gate before this one was riding that road
///
/// `--said` did not exist, so a stand-in could declare only by PRINTING — and the four gates above
/// converge, stand down and reflect on markers scraped off a `/bin/sh` that scrolls. That is not
/// wrong, and it is not the shipped shape: it is the fallback working for a peer that happens not
/// to have the property item 441 measured. This gate is the other road, and it is the one a real
/// agent takes.
///
/// # ⚠⚠⚠ The two arms put the SAME declaration in different places
///
/// | arm | the marker is | the pane says | must converge |
/// |---|---|---|---|
/// | `ByStating` | in the report only | `ACK` | on the agent's own account |
/// | `ByPrinting` | on the pane only | `MILESTONE REACHED` | on the fallback |
///
/// ⚠⚠ **THE STATED ARM'S SCREEN IS BARE ON PURPOSE.** A peer that both printed and stated would
/// converge whichever reader answered, so it could not tell them apart — which is exactly the
/// position every gate here was in until now. With the pane holding nothing but `ACK`, a
/// convergence can only have come from the statement, and the walk says which reader was used in
/// the product's own words.
///
/// # ⚠⚠⚠⚠ The premises, asserted inside, because each one alone makes the claim vacuous
///
/// * **Both arms really converged.** An ending of any other word means no declaration was read at
///   all, and *which reader* is then a question about nothing.
/// * **The walk names the READER**, and the two arms must name different ones. Converging is the
///   weaker half: the strong claim is that the statement road was the one taken, and `Evidence` is
///   the product's own answer to that (register item 448).
/// * **The stated arm's pane really is bare.** Asserted through the walk rather than assumed — a
///   fixture that printed after all would make the claim true of the road it was meant to exclude.
#[test]
fn a_loop_converges_on_what_its_agent_says_not_on_what_its_terminal_shows() {
    /// The work turn the peer declares on — the second, so a turn it does not declare on is
    /// measured first in both arms.
    const DECLARES_ON: u32 = 2;
    /// The phrase the walk carries on an ORDINARY judged turn when the AGENT's own account was the
    /// reader.
    ///
    /// ⚠⚠ READ OFF AN ORDINARY TURN, and that is not second best. The DECLARING edge names no
    /// reader unless a `milestone_check` is authored — `Evidence` rides the check's artifact — and
    /// authoring one here would send this fixture's checker into an isolated checkout of a pane
    /// born in `$HOME`, which is register item 705's machinery pointed at somebody's home
    /// directory. The road does not change between a run's turns; only what the peer states does.
    const BY_STATEMENT: &str = "the judge read the agent's own account of this turn";
    /// And when the terminal was. ⚠ Both are the product's own words (`Evidence::named`): a gate
    /// that spelled its own would go green the day the product stopped saying either.
    const BY_PANE: &str = "the judge read the pane, since this turn's prompt";

    /// Drive one arm and hand back its ending and its walk.
    fn ran(declares: Declares, name: &str) -> (sprag_plugin::Outcome, Vec<String>) {
        let (_host, sock) = spawn_host();
        let (driving, mut setup) = remote_driver(&sock);

        // ⚠ NO THREAD ID — it is interpolated into a shell script. See the neighbouring gates.
        let under = std::env::temp_dir().join(format!("sprag-said-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&under);
        std::fs::create_dir_all(&under).expect("a directory of this arm's own");
        let log = under.join("what-the-daemon-said-to-each-report");
        let state = under.join("state");
        std::fs::create_dir_all(&state).expect("a state home of this arm's own");

        let pane = spawn_pane(
            &mut setup,
            json!({
                SPAWN_CMD_KEY: [
                    "/bin/sh",
                    "-c",
                    standin_that_reports_itself(
                        Path::new(env!("CARGO_BIN_EXE_sprag")),
                        &log,
                        &state,
                        true,
                        ("nothing this run reflects toward", "nor carries"),
                        // ⚠ NO RECORD: neither road reads a spend, and a readable one would open
                        // the capacity edge — a second thing moving in a two-arm comparison.
                        None,
                        declares,
                    ),
                ],
                SPAWN_COLS_KEY: 80,
                SPAWN_ROWS_KEY: 16,
            }),
        );

        let script: std::sync::Arc<dyn sce_rust_runtime::IScriptEngine> =
            std::sync::Arc::new(sce_rust_lua::LuaEngine::new());
        let brief = sprag_plugin::Brief {
            north_star: "the stand-in reaches a checkpoint and somebody has asked it to stop"
                .to_string(),
            milestone: "the checkpoint this run is working toward".to_string(),
            reference: "register item 441".to_string(),
            closing_rules: None,
            context_ceiling: None,
            reflect_after_refusals: None,
            milestone_check: None,
            service: None,
            // ⚠⚠ Above what either arm needs, so a run that FAILED to read the declaration does not
            // end on its budget looking like one that did — it runs on and this gate says so.
            max_turns: Some(sprag_plugin::Counted::Of(6)),
            reflect_every: Some(6),
            // (the wall clock is the caller's — see the `Guardrails` below)
            screen_rules: None,
            may_answer: None,
            await_person_ms: Some(0),
            handback_still_ms: None,
            hold_within_ms: Some(3_600_000),
            ready_timeout_ms: Some(15_000),
            turn_within_ms: Some(5_000),
        };
        let mut spec = sprag_plugin::AiLoopSpec::driving("sh");
        spec.shows_the_prompt = false;

        let mut loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
            .expect("a well-briefed loop over a live pane starts");
        // ⚠⚠ STOOD DOWN, which is what turns a declaration into an ENDING here: without the order
        // a declared milestone goes to `reflecting` and the run carries on, and this gate's subject
        // is which READER answered rather than which door the answer opened.
        loops.stand_down();
        let progress = sprag_plugin::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 8_000,
            max_cost: None,
            // ⚠⚠ TIGHT ON PURPOSE. Both arms converge in well under a second; this bound is only
            // ever paid by an arm that FAILED to read its declaration, and the first draft of this
            // gate spent two of them at sixty seconds each before the runner cut it off. A failure
            // that takes minutes to report is a failure nobody iterates against.
            max_duration: Some(Duration::from_secs(15)),
        })
        .reporting_to(std::sync::Arc::clone(&progress))
        .run(&mut loops, &driving, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        // ⚠⚠ THE MARKER IS ON THE PANE IN BOTH ARMS AND THAT IS NOT A FIXTURE FAULT: the peer
        // echoes what it reads, and the WORK PROMPT ends *reply exactly: MILESTONE REACHED*. What
        // separates the arms is whether it is there as the AGENT'S OWN ROW — which is what the echo
        // discount (`wraps_onto`, against the prompt this run typed) exists to decide, and which
        // this test cannot re-derive from outside. So the reading printed is the one that matters:
        // WHICH READER the product says it used.
        let reader = walk
            .iter()
            .find_map(|note| note.split_once("the judge read ").map(|(_, rest)| rest))
            .map(|rest| rest.split(" — ").next().unwrap_or(rest).to_owned());
        println!(
            "== declares {name}: {:?} in {} iteration(s); the judge read {reader:?} ==",
            outcome.state, outcome.iterations,
        );
        let _ = std::fs::remove_dir_all(&under);
        (outcome, walk)
    }

    let (stated, stated_walk) = ran(Declares::ByStating(DECLARES_ON), "by-stating");
    let (printed, printed_walk) = ran(Declares::ByPrinting(DECLARES_ON), "by-printing");

    // ── ⚠⚠ THE PREMISE: BOTH ARMS REALLY READ A DECLARATION ─────────────────────────────────
    assert_eq!(
        (stated.state, printed.state),
        (
            sprag_plugin::OutcomeState::Converged,
            sprag_plugin::OutcomeState::Converged,
        ),
        "⚠⚠⚠⚠⚠ THE PREMISE FAILED: an arm did not converge, so no declaration was read at all and \
         *which reader* is a question about nothing. Stated walked {stated_walk:?}; printed walked \
         {printed_walk:?}",
    );

    // ── ⛔⛔⛔ THE CLAIM: THE STATED ARM CONVERGED ON THE AGENT'S OWN ACCOUNT ─────────────────
    assert!(
        stated_walk.iter().any(|note| note.contains(BY_STATEMENT)),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 441: this peer put its declaration NOWHERE BUT ITS REPORT — its \
         screen says `ACK` — and the run still converged, so something read it. The walk must name \
         the agent's own account as the reader: a convergence that says it was shown the PANE here \
         is a reader finding a marker that is not on the pane. The walk: {stated_walk:?}",
    );
    assert!(
        !stated_walk.iter().any(|note| note.contains(BY_PANE)),
        "⚠⚠⚠ and it must NOT also name the pane — the two roads are exclusive by construction \
         (`said_marker` returns on the statement before the terminal is touched), so both would \
         mean the reading is not the one this gate thinks it is. The walk: {stated_walk:?}",
    );

    // ── ⛔⛔ THE CONTROL: THE FALLBACK STILL WORKS, AND SAYS SO ──────────────────────────────
    //
    // ⚠ This is R27's rule: a repair that made the OTHER road stop working would pass the claim
    // above and break every peer with no hooks installed — which is exactly who the pane read is
    // kept for, and its own doc says so in one line: *the fallback is not dead code*.
    assert!(
        printed_walk.iter().any(|note| note.contains(BY_PANE)),
        "⛔⛔⛔⛔ THE CONTROL: a peer that states nothing must still be read off its pane, and the \
         walk must say that is what happened. Every agent with no hook installed is this peer, and \
         a repair that took its road away would look green in the claim above. The walk: \
         {printed_walk:?}",
    );
    assert!(
        !printed_walk.iter().any(|note| note.contains(BY_STATEMENT)),
        "⚠⚠ and it must not claim a statement it never made. The walk: {printed_walk:?}",
    );

    // ── ⛔⛔⛔⛔⛔ AND WHY IT READ THE PANE, ON A RUN WHOSE REPORTER IS ALIVE ──────────────────
    //
    // Register item 486. This arm's peer is `standin_that_reports_itself` — it runs `sprag
    // report-agent`, so something IS answering for that pane — and it simply does not STATE its
    // declaration. That is `Unstated::NotYet`, which is the world sprag run 31 was measured in
    // (2026-08-25: two pane-road judgements while `source=hook:claude seq=13 asked=7 said=3` was
    // live in the same minute).
    //
    // ⛔⛔ **AND THE WALK USED TO TELL THIS RUN'S READER TO GO AND INSTALL A HOOK.** One sentence
    // was printed for every pane read, ending *"Install the agent's hook and this judgement reads
    // what it SAID instead"* — a wrong errand, at a pane whose reporter is right there in the
    // fixture. This is the end-to-end proof that the reason now survives from
    // `stated_this_turn` through `Because::Judged` to the sentence a person reads in `sprag runs`:
    // every other gate on item 486 measures one joint of that chain.
    let reason = printed_walk
        .iter()
        .find(|note| note.contains(BY_PANE))
        .cloned()
        .unwrap_or_default();
    assert!(
        !reason.contains("install the agent's hook"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 486: this pane's agent reports itself — the fixture spawns it \
         through `sprag report-agent` — and the run is telling whoever reads its walk to go and \
         install a hook. That is not a missing word, it is a WRONG ERRAND, and it is the one this \
         item was measured costing a reader on sprag run 31. The line: {reason:?}",
    );
    assert!(
        reason.contains("HOOK IS INSTALLED AND REPORTING"),
        "⚠⚠⚠⚠ AND IT MUST SAY THE TRUE THING RATHER THAN MERELY STOP SAYING THE FALSE ONE — a \
         deleted clause passes the assertion above and leaves this run's reader with no account of \
         why their reporting agent was judged off the screen. ⚠ This is also the claim that the \
         `Unstated` word REACHES A WALK at all: if it does not, *run a loop and read the walk* is \
         not a way to answer item 486's remaining question. The line: {reason:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A DRIVER OUTSIDE THE DAEMON CAN SAY WHERE ITS RUN'S WORK IS** — register item 722,
/// which is the half of register item 710 that this surface could not answer.
///
/// # ⛔⛔ What was broken, and why it was broken in the direction that matters
///
/// Item 710 pointed the independent milestone checker at the run's repository, and built it as two
/// doors: a WRITING one (a spawn may carry a cwd) and a READING one (*where was this pane born*).
/// The writing door already reached this surface — `SPAWN_CWD_KEY` was in the grammar and nobody
/// sent it. **The reading door did not exist here at all**: a pane's birth directory had no wire
/// address, so `RemotePaneAccess` could not implement `PaneOrigin`, `PaneAccess::origin` stayed at
/// its `None` default, `OuterLoop::checked` was handed nothing, and the checker was spawned with no
/// directory — landing in `$HOME`, judging a repository it could not open a file in. That is item
/// 710's own measured defect, alive in the path items 544 and 643 move every run onto.
///
/// # ⚠⚠⚠⚠⚠ The premises, asserted inside — because either one missing makes this vacuous
///
/// * **THE SURFACE IS THE REMOTE ONE.** The whole item is that the in-process surface answered and
///   this one did not, so a gate that measured `WorkspacePaneAccess` again would re-measure the
///   door that already worked. `remote_driver` is a real `RemotePaneAccess` over a real socket.
/// * ⚠⚠ **AND THE DIRECTORY IS NEITHER `$HOME` NOR THIS PROCESS'S CWD** — item 710's own argument,
///   restated here because it is what makes the answer evidence. Those two are exactly the values a
///   broken read degrades to, so a birth directory equal to either would be satisfied by a surface
///   that had learned nothing.
///
/// ⚠ The two links either side of this one are held elsewhere and are not re-measured here:
/// `sprag_plugin`'s `a_checker_is_put_where_the_work_is_and_told_what_to_consult` holds
/// *origin answers → the question says where the work is*, and the writing door is the same
/// `SPAWN_CWD_KEY` this gate spawns through.
#[test]
fn a_driver_over_the_socket_reads_where_a_pane_was_born() {
    let (_host, sock) = spawn_host();
    let (driver, mut setup) = remote_driver(&sock);

    // ⚠ A directory this daemon can really spawn in, and one nothing else in this test would
    // produce by accident — the pane is born HERE and nowhere the reader could guess.
    let repo = std::env::temp_dir().join(format!(
        "sprag-722-birth-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    std::fs::create_dir_all(&repo).expect("the run's repository stands in a real directory");

    // ── ⚠⚠ THE PREMISES ───────────────────────────────────────────────────────────────────────
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    assert_ne!(
        Some(repo.clone()),
        home,
        "⚠⚠⚠⚠⚠ THE PREMISE: `$HOME` is where a pane with no directory lands, so a repository that \
         WAS `$HOME` would be answered correctly by a surface that reads nothing at all",
    );
    assert_ne!(
        Some(repo.clone()),
        std::env::current_dir().ok(),
        "⚠⚠⚠⚠ AND THE OTHER VALUE A BROKEN READ DEGRADES TO — the process's own working directory. \
         Item 710's doc names both, and a gate that let the answer coincide with either would pass \
         against the very defect it is here to catch",
    );

    let pane = spawn_pane(
        &mut setup,
        json!({ "cmd": ["cat"], SPAWN_CWD_KEY: repo.to_string_lossy() }),
    );

    // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────────
    let origin = driver.origin().expect(
        "⛔⛔⛔ ITEM 722: a driver over this socket cannot say where ANY pane was born, so every \
         run driven outside the daemon puts its milestone checker in `$HOME` — which is exactly \
         what register item 710 was filed for, one surface along",
    );
    assert_eq!(
        origin.pane_start_dir(pane).as_deref(),
        Some(repo.as_path()),
        "⛔⛔⛔⛔⛔ ITEM 722: this pane was spawned in the run's repository through the daemon's own \
         verb, and the surface a driver reads it back through gives some other answer. The checker \
         this feeds is told «the work is HERE, open the files» — so a wrong directory is worse than \
         an absent one, and an absent one already cost item 710 a live checker answering YES about \
         a tree it never opened",
    );

    // ⚠ A PANE THIS DAEMON DOES NOT HOLD IS THE OTHER ANSWER, and it is asserted for
    // `RemotePluginWorld::has_pane`'s reason one gate below: a surface that answered the same
    // directory to everything would satisfy the claim above and be useless. `None` here is *no such
    // pane*, never `$HOME`.
    assert_eq!(
        origin.pane_start_dir(PaneId(u64::MAX)),
        None,
        "⚠⚠⚠ a pane that is not there has no birth directory, and the absence must arrive as one — \
         register item 709's discipline, which this address is the newest place to need",
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// ⛔⛔⛔⛔ **THE WORLD A RUN IS CHECKED AGAINST ANSWERS THE SAME TWO THINGS FROM OUTSIDE THE
/// DAEMON** — register items 544 and 643, and the second implementation that makes
/// [`PluginWorld`] a seam rather than a decoration.
///
/// # ⚠⚠⚠⚠⚠ Why a trait with one implementation would have been the defect
///
/// A run's plugin is built from its request map, and the builder asks the world exactly two
/// questions — measured over the whole of `build_plugin`, not assumed. When the driver moves out of
/// the daemon (item 544, whose stages 1 and 2 already stand) it must build **the same plugin from
/// the same request**, and a second builder over there would be a second answer to one question.
/// So the builder stayed one function and the world became an argument — and an argument with a
/// single implementation is a surface nobody reads, which is this register's item 492 wearing a
/// trait. **This gate is the second reader.**
///
/// # What each answer has to survive, and why it is not the obvious thing
///
/// * **`has_pane` is the PANE LIST**, so it is the same set the daemon's own pool answers from.
///   A mistyped id becomes a synchronous refusal instead of a run that dies on its first step.
/// * **`default_size` is the DAEMON'S ARBITRATED SIZE, never this process's terminal.** A driver
///   process has a terminal of its own and it is nobody's business: the rectangle a pane opens at
///   is the one every client of that session lays its arrangement over, and a size chosen
///   independently by two processes for one pane is exactly the reflow that address exists to
///   prevent.
#[test]
fn the_world_a_run_is_checked_against_answers_the_same_two_things_over_the_wire() {
    let (_host, sock) = spawn_host();
    let (driver, mut setup) = remote_driver(&sock);
    let spawned = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));
    let world = RemotePluginWorld::over(&driver);

    // ── THE PANE THE DAEMON HOLDS, AND ONE IT DOES NOT ────────────────────────────────────────
    // ⚠ The pair is the whole assertion: an implementation that answered `true` to everything, or
    // `false` to everything, satisfies half of this and is useless. Only the discrimination is
    // evidence — the shape `PaneEcho`'s own gate is built on.
    assert!(
        world.has_pane(spawned),
        "⛔⛔⛔⛔ A PANE THE DAEMON IS HOLDING READS AS ABSENT FROM OUTSIDE IT. Every run checked \
         through this world would be refused before it started, which turns item 544's driver \
         process into one that can never drive anything",
    );
    assert!(
        !world.has_pane(PaneId(spawned.0 + 1_000)),
        "⚠⚠⚠ ...and an id nothing carries must be REFUSED. A world that says yes to everything \
         turns a mistyped id into a run that dies on its first step instead of a refusal the \
         caller reads at the door",
    );

    // ── AND THE SIZE IS THE DAEMON'S ──────────────────────────────────────────────────────────
    // ⚠⚠⚠⚠⚠ **THE FIRST FORM OF THIS ARM COULD NOT GO RED, AND THE MUTATION SAID SO.** It read the
    // published slot and compared — but with no attached client that slot is `null`, so the
    // expectation fell back to the same 80x24 literal the product falls back to. Replacing the
    // whole read with `(80, 24)` PASSED. **A gate whose expectation and whose subject share a
    // fallback is measuring the fallback**, which is item 632's shape at a fixture rather than at a
    // branch.
    //
    // So a client REPORTS an area first — the only thing that makes the daemon arbitrate one — and
    // it is deliberately neither 80x24 nor the boot pane's 40x6, so no constant available to the
    // implementation can match it by luck.
    const REPORTED: (u16, u16) = (117, 41);
    let mut viewer = HostConn::connect(&sock, Duration::from_secs(5)).expect("a viewing client");
    viewer
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "sizer" }))
        .expect("client/hello is accepted");
    viewer
        .call(CLIENT_ATTACH_METHOD, json!({}))
        .expect("client/attach is accepted");
    viewer
        .call(
            sprag_rpc::CLIENT_SIZE_METHOD,
            json!({ "cols": REPORTED.0, "rows": REPORTED.1 }),
        )
        .expect("a client may say how big a window it can give");

    // ⚠ The arbitration is the daemon's own act on the report, so the ADDRESS is still what this
    // asserts against — the constant above is only what makes the two answers distinguishable.
    let published = setup
        .call(
            "scene/query",
            json!({ "path": mux_action_path(sprag_host::wire::WINDOW_SIZE_SLOT) }),
        )
        .expect("the daemon publishes the size it arbitrated");
    let want = published["cols"]
        .as_u64()
        .zip(published["rows"].as_u64())
        .map(|(c, r)| {
            (
                u16::try_from(c).expect("a width fits"),
                u16::try_from(r).expect("a height fits"),
            )
        })
        .unwrap_or_else(|| {
            panic!(
                "⚠⚠⚠⚠ THE FIXTURE MUST MAKE THE DAEMON ARBITRATE A SIZE, or this arm compares two \
                 fallbacks and cannot fail. Published {published}"
            )
        });
    assert_ne!(
        want,
        (80, 24),
        "⚠⚠⚠⚠⚠ AND IT MUST NOT BE THE FALLBACK, which is the whole reason a client reported one: \
         an expectation equal to the product's own default is an expectation no defect can miss",
    );
    let (cols, rows) = world.default_size();
    assert_eq!(
        (cols, rows),
        want,
        "⛔⛔⛔ THE SIZE A PANE WOULD BE OPENED AT IS NOT THE ONE THIS SESSION IS LAID OUT OVER. \
         A driver process has a terminal of its own, and taking it from there — or from a constant \
         — reflows every program in the pane to a number nobody in the session chose. Published \
         {published}, world answered {cols}x{rows}",
    );

    let _ = std::fs::remove_file(&sock);
}

/// Type `text` into `pane` on the TEST's connection after `after` — the pane MOVING at an instant
/// this gate chooses, which is what a wait is being measured against.
///
/// On the test's own connection and not the driver's, for [`spawn_pane`]'s reason: what the driver
/// did, it did through [`PaneAccess`] alone.
fn print_into_pane_after(sock: &Path, pane: PaneId, after: Duration, text: &'static str) {
    let sock = sock.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let mut conn =
            HostConn::connect(&sock, Duration::from_secs(5)).expect("the prodder's connection");
        let _ = conn.call(
            "scene/invoke",
            json!({
                "path": pane_input_path(pane.0, TEXT_ACTION),
                "args": { "text": text },
            }),
        );
    });
}

/// ⛔⛔⛔⛔ **A PARK THAT CANNOT BE SERVED DEGRADES TO A CLOCK RATHER THAN TO A LIE** — register
/// item 631, and the arm nothing exercised until this gate.
///
/// # ⚠⚠⚠⚠⚠ Why this is the dangerous arm, and why the whole gate is about ONE return value
///
/// [`PaneChanges::pane_moved_after`]'s two "nothing happened" answers look alike and mean opposite
/// things. `Some(seen)` is **the pane did not move — park again**; `None` is **this surface cannot
/// tell you when it moves — go back to a clock**. A repair that answered `Some(seen)` when the park
/// failed would read as a perfectly quiet pane, and `park_until` would go on parking on a signal
/// that is never coming — a wait that ends only at its own timeout, on a pane that may have moved
/// a hundred times.
///
/// The daemon's own refusal is what stages it: a park naming a pane this session does not hold is
/// answered `INVALID_PARAMS`, which is exactly the shape a daemon too old to serve the address
/// gives. So this drives the real failure rather than a mock of it.
///
/// ⚠⚠ **AND THE PARK CONNECTION IS DROPPED**, which the second half asserts through
/// [`PaneAccess::changes`]: a transport that failed may have left half a frame in the stream, so
/// keeping it would answer `None` for ever while still LOOKING like a capability. A surface that
/// says it can be waited on and never can is worse than one that never claimed it.
#[test]
fn a_park_the_daemon_refuses_degrades_the_wait_instead_of_reporting_a_still_pane() {
    let (_host, sock) = spawn_host();
    let (driver, _setup) = parking_remote_driver(&sock);
    assert!(
        driver.changes().is_some(),
        "the precondition: this driver CAN park, so what follows is about the refusal and not \
         about a surface that never offered",
    );

    // A pane this session does not hold. The daemon refuses the park by name.
    let absent = PaneId(4242);
    let answered =
        sprag_plugin::PaneChanges::pane_moved_after(&driver, absent, 0, Duration::from_millis(200));
    assert_eq!(
        answered, None,
        "a refused park is *I cannot tell you when this pane moves*, NEVER *it did not move*: \
         Some(seen) here is a driver parked for ever on a signal that is not coming",
    );
    assert!(
        driver.changes().is_none(),
        "and the park connection is gone, so the surface stops claiming a capability it no longer \
         has — a `Some` here would offer a park that answers None for ever",
    );

    // ⚠ THE CONTROL, and it is what stops this passing on a build where nothing works: a driver
    // that has NOT met a refusal still parks on a pane the session does hold, and still answers
    // the contract's *nothing happened* rather than the degradation.
    let (fresh, _fresh_setup) = parking_remote_driver(&sock);
    let quiet = sprag_plugin::PaneChanges::pane_revision(&fresh, PaneId(0))
        .expect("the boot pane has a revision");
    assert_eq!(
        sprag_plugin::PaneChanges::pane_moved_after(
            &fresh,
            PaneId(0),
            quiet,
            Duration::from_millis(200)
        ),
        Some(quiet),
        "a served park that simply timed out answers the revision UNCHANGED — the other word for \
         nothing happened, and the one that keeps the caller parked",
    );
    assert!(
        fresh.changes().is_some(),
        "and that driver keeps its park connection: a timeout is not a failure",
    );
}

/// ⛔⛔⛔⛔ **A RUN DRIVEN OVER THE WIRE WAITS ON A PANE INSTEAD OF RE-READING ITS SCREEN** —
/// register item 631, and the number it was open for.
///
/// # ⚠⚠⚠⚠⚠ Why the gate is a PAIR and neither half means anything alone
///
/// *Cheap* and *deaf* are one reading when all you count is looks: a wait that answered instantly
/// and wrongly would score best of all. So this asserts BOTH — the wait comes back
/// [`Waited::Ready`] because the pane really did print the marker, AND it paid a handful of screen
/// reads rather than one per ten milliseconds. The register records that lesson twice (items 280
/// and 630, where a gate measuring only the ENDING stayed green under a polling mutation).
///
/// # ⛔⛔⛔⛔⛔ Why TWO WINDOWS PER ARM and not two arms at one window — register item 668
///
/// This gate used to wait one 1000 ms window on each arm and demand the parked one cost a fifth of
/// the polling one. **That reading is the runner's speed, not this product's behaviour.** A look
/// here is a whole SCREEN across a socket plus a detector run over it, so it is far dearer than the
/// 10 ms slice that paces it: on the build machine the polling arm scored 96, but a runner where
/// one look costs 100 ms scores 10 over the same second, and the assertion asked for more than 10.
/// Nothing about the park would have changed. Register item 666 is the same defect measured in
/// `sprag-plugin`, where it turned a macOS lane red at a commit that touched no plugin — and where
/// the lane then went GREEN at the next commit with the defect still in place, which is why a
/// lane's colour is not evidence about a gate of this shape and the ratio is.
///
/// ⚠⚠⚠ **So each arm is now waited out over TWO windows a factor of four apart, and what is
/// asserted is the SHAPE of each arm's cost.** A slow host stretches both windows alike, so the
/// ratio survives exactly what the count could not:
///
/// * **the parked arm's cost does NOT follow the window** — the claim, and item 631's whole content;
/// * **the polling arm's cost DOES** — the staging control, which is what says the windows really
///   buy looks on this runner and that the line above was not made true by a fixture that waits for
///   nothing (register item 646).
///
/// # ⚠⚠⚠ MEASURED 2026-08-24 on the build machine (32 cores, 125 GB), and it is the HOST'S number
///
/// A one-second wait over a real `sprag-term`, ended by the pane printing a marker:
/// **parked 2 looks, polling 96.** ⚠ Both counts are that machine's; a slower one scores lower on
/// both and the ratio between the two windows is what this gate reads.
///
/// # ⚠⚠⚠ THE RIVAL, measured rather than remembered — herdr at `9a4ce5e1`, read 2026-08-24
///
/// **Every wait herdr's API serves is a poll, and there is no wake anywhere in it.**
/// `src/api/wait.rs::wait_for_output` loops: issue a full `PaneRead` of the pane's text, test the
/// match, `sleep(CONNECTION_POLL_INTERVAL)` — **100 ms** (`src/api/server.rs`). Its event wait does
/// the same against a RING (`event_hub.events_after(seq)`) on the same interval, and
/// `src/api/event_hub.rs` holds exactly three methods — `push`, `events_after`,
/// `current_sequence` — with no condvar, no observer and nothing to notify. So an hour of patience
/// over that API is 36,000 pane reads, and the cost of any wait there follows the CLOCK.
///
/// After this gate, sprag's is one request and then nothing: no wire traffic, no daemon work, and a
/// wake that is the pane's own reader thread firing an observer synchronously as it applies the
/// batch. That is the axis this repository was BEHIND on, and the number below is what changed it.
///
/// # ⚠⚠⚠ And the CONTROL is the same wait over a surface with no park connection
///
/// Same daemon, same pane, same predicate, same markers, and the only difference is whether the
/// wait could be told. A repair that broke the park would not merely miss a target; it would make
/// the parked arm's cost grow with the window exactly as the control's does.
#[test]
fn a_remote_driver_parks_on_a_pane_instead_of_re_reading_its_screen() {
    /// The short window each arm is waited out over.
    const SHORT: Duration = Duration::from_millis(250);
    /// The long one, FOUR TIMES it — the pair whose ratio is this gate's whole claim.
    const LONG: Duration = Duration::from_millis(1000);
    /// ⚠⚠ ONE MARKER PER WINDOW, because the pane keeps what it printed: a second wait looking for
    /// the first window's marker would find it already on the screen and end before it began,
    /// scoring one look and reporting the park as perfect.
    const SHORT_MARKER: &str = "PANE-MOVED-631-SHORT";
    /// ⚠ WITH ITS NEWLINE. The boot pane runs `cat`, so the line discipline echoes the text and
    /// `cat` writes it back — but only a completed line reaches `cat` at all, and a gate that
    /// depended on the echo alone would be measuring the TERMINAL rather than the program.
    const SHORT_TYPED: &str = "PANE-MOVED-631-SHORT\n";
    const LONG_MARKER: &str = "PANE-MOVED-631-LONG";
    const LONG_TYPED: &str = "PANE-MOVED-631-LONG\n";
    /// What the long window may cost the PARKED arm over the short one before this gate calls that
    /// cost a function of the window. ⚠ Generous on purpose: the claim is *flat*, and a park that
    /// woke a handful of extra times would still be a park.
    const SLACK: u64 = 8;
    /// What the long window must cost the POLLING arm over the short one before this gate believes
    /// the windows buy looks at all. ⚠⚠ Half the honest reading against a window four times longer,
    /// and that headroom is the whole of register item 668: what must not be able to turn this over
    /// is the host it runs on.
    const GROWTH: u64 = 2;

    /// Wait out one window on one driver, answering **how many looks it cost**, how long it took,
    /// and how it ended.
    fn waited_out(
        driver: RemotePaneAccess,
        sock: &Path,
        pane: PaneId,
        window: Duration,
        marker: &'static str,
        typed: &'static str,
    ) -> (u64, Duration, sprag_plugin::run::Waited) {
        let driver = CountingRemote::new(driver);
        print_into_pane_after(sock, pane, window, typed);
        let run = RunContext::uncancellable();
        let began = Instant::now();
        let ended =
            sprag_plugin::run::park_until(&run, &driver, pane, Duration::from_secs(20), || {
                let seen = driver
                    .pane_rows(pane)
                    .is_some_and(|rows| rows.iter().any(|row| row.text.contains(marker)));
                if seen {
                    sprag_plugin::run::Look::Holds
                } else {
                    // ⚠ THE ARM THIS GATE IS ABOUT. The answer is a function of the pane's bytes
                    // and of nothing else, so a surface that can be told when the pane moved owes
                    // exactly one look per move — and one that cannot owes one per slice.
                    sprag_plugin::run::Look::Steady
                }
            });
        (driver.looks(), began.elapsed(), ended)
    }

    // ── the arm this gate is about: a driver holding a park connection, over both windows ──
    let (_host, sock) = spawn_host();
    let (short_driver, _short_setup) = parking_remote_driver(&sock);
    assert!(
        short_driver.changes().is_some(),
        "a driver given a park connection publishes a change signal — without that this arm is \
         the control measured twice",
    );
    let (parked_short, parked_short_took, parked_short_end) = waited_out(
        short_driver,
        &sock,
        PaneId(0),
        SHORT,
        SHORT_MARKER,
        SHORT_TYPED,
    );
    let (long_driver, _long_setup) = parking_remote_driver(&sock);
    assert!(
        long_driver.changes().is_some(),
        "⚠⚠ AND THE SECOND WINDOW'S DRIVER PARKS TOO — the two readings are one arm, so a park \
         that failed to be granted here would report itself as a product regression below",
    );
    let (parked_long, parked_long_took, parked_long_end) =
        waited_out(long_driver, &sock, PaneId(0), LONG, LONG_MARKER, LONG_TYPED);

    // ── the staging control: the same wait over a surface that cannot be told, over both ──
    let (_control_host, control_sock) = spawn_host();
    let (control_short_driver, _control_short_setup) = remote_driver(&control_sock);
    assert!(
        control_short_driver.changes().is_none(),
        "the CONTROL is a driver that cannot be told, and its `None` is what makes it one",
    );
    let (control_short, control_short_took, control_short_end) = waited_out(
        control_short_driver,
        &control_sock,
        PaneId(0),
        SHORT,
        SHORT_MARKER,
        SHORT_TYPED,
    );
    let (control_long_driver, _control_long_setup) = remote_driver(&control_sock);
    let (control_long, control_long_took, control_long_end) = waited_out(
        control_long_driver,
        &control_sock,
        PaneId(0),
        LONG,
        LONG_MARKER,
        LONG_TYPED,
    );

    // ── the controls: all four really came back, and all four really waited ──
    assert_eq!(
        (
            parked_short_end,
            parked_long_end,
            control_short_end,
            control_long_end
        ),
        (
            sprag_plugin::run::Waited::Ready,
            sprag_plugin::run::Waited::Ready,
            sprag_plugin::run::Waited::Ready,
            sprag_plugin::run::Waited::Ready
        ),
        "⛔⛔⛔⛔⛔ EVERY WAIT MUST END BECAUSE THE PANE REALLY PRINTED ITS MARKER. *Cheap* and \
         *deaf* are one reading when all you count is looks — a park that never woke would score \
         best of all — so this is the half of the pair the numbers below mean nothing without \
         (register items 280 and 630)",
    );
    assert!(
        parked_short_took >= SHORT
            && parked_long_took >= LONG
            && control_short_took >= SHORT
            && control_long_took >= LONG,
        "⚠⚠⚠ AND EACH ARM MUST REALLY HAVE WAITED OUT ITS OWN WINDOW, or *cheap* was bought by not \
         waiting — parked {parked_short_took:?}/{parked_long_took:?}, polling \
         {control_short_took:?}/{control_long_took:?} against {SHORT:?}/{LONG:?}",
    );

    // ── the staging control's claim: A WINDOW REALLY DOES BUY LOOKS ON THIS RUNNER ──
    assert!(
        control_long >= control_short.saturating_mul(GROWTH),
        "⚠⚠⚠⚠ THE STAGING CONTROL: a surface that cannot be told MUST pay for the window, or this \
         host is not buying looks with time and the claim below is true of a fixture rather than \
         of a park. A {LONG:?} window cost {control_long} looks where a {SHORT:?} one cost \
         {control_short}",
    );

    // ── the claim: A PARKED WAIT'S COST DOES NOT FOLLOW THE WINDOW ──
    assert!(
        parked_long <= parked_short + SLACK,
        "⛔⛔⛔⛔⛔ THE PARKED WAIT IS PAYING FOR THE CLOCK AGAIN — register item 631's repair is \
         undone. A {LONG:?} window cost {parked_long} looks where a {SHORT:?} one cost \
         {parked_short}, and the control over the same two windows cost {control_short} and \
         {control_long}: this arm has taken the control's SHAPE. Each look is a whole screen \
         across a socket plus a detector run over it. ⚠ Register item 668: the absolute counts \
         are the runner's speed, so the shape is the only honest reading here",
    );
    assert!(
        parked_long <= 15,
        "the parked wait's cost follows the PANE, not the clock — a second of silence must not \
         buy looks. Got {parked_long}",
    );

    // ⚠⚠⚠ PRINTED RATHER THAN ASSERTED, and printed rather than left to be re-derived by a round
    // that raises a bound to read it: the four numbers are what say how much headroom the two
    // shapes above have on THIS host, and register item 668 exists because a past round reasoned
    // from counts it had not read. Absolute counts are the runner's; the shapes are the product's.
    println!(
        "\n== what one wire-driven wait costs, per window ==\n  parked:  {parked_short} look(s) \
         over {SHORT:?}, {parked_long} over {LONG:?}\n  polling: {control_short} look(s) over \
         {SHORT:?}, {control_long} over {LONG:?}\n  the claim is the SHAPE of each row, never the \
         numbers in it\n",
    );
}

/// Report `state` for `pane` on the TEST's connection, as an agent's own hook would.
///
/// ⚠ `asked` is what moves `asked_seq`, which is the counter a completion contract pairs a rest
/// against (register item 441). A helper that could not state one could not stage a turn ENDING at
/// all — only a verdict changing, which is a different fact.
fn report_agent(conn: &mut HostConn, pane: PaneId, state: &str, asked: Option<&str>) {
    let mut args = json!({
        "id": pane.0,
        "source": "hook:test",
        "state": state,
        "name": "claude",
    });
    if let Some(asked) = asked {
        args["asked"] = json!(asked);
    }
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(REPORT_AGENT_ACTION), "args": args }),
    )
    .expect("the agent reports its own state over the wire");
}

/// ⛔⛔⛔⛔⛔ **A REMOTE DRIVER WAITING ON A VERDICT STOPS ASKING FOR IT EVERY SLICE** — register
/// item 640, driven through a real daemon over a real socket.
///
/// # ⚠⚠⚠⚠⚠ Why item 631 did not close this, measured rather than predicted
///
/// Item 631 gave this surface a park connection, and the gate above measures what that bought: a
/// wait whose predicate rests on the pane's BYTES went from 96 looks a second to 2. The register
/// predicted the rest would follow. **It did not, and reading the product is what said so.** A wait
/// whose predicate rests on the pane's VERDICT — which is every wait an agent loop takes, because
/// [`DoneWhen::Settles`](sprag_plugin::DoneWhen::Settles) is the contract a loop runs on — asked
/// `Settling::due()` and got [`Settling::Unknown`](sprag_plugin::Settling::Unknown), whose deadline
/// is *now plus one poll interval* by construction. So the deadline fell due on every slice, the
/// park was woken on every slice, and the parkable surface polled exactly as hard as the
/// unparkable one.
///
/// `Unknown` was the honest answer while the wire carried no deadline. This gate exists because it
/// now carries one.
///
/// # ⚠⚠⚠ THE CONTROL IS A DRIVER THAT CANNOT BE TOLD, and it is a real one rather than a mutation
///
/// [`RemotePaneAccess::over`] without [`RemotePaneAccess::parking_on`] publishes no change signal
/// at all, so `park_until` takes its documented degradation and looks every slice. That is what the
/// polling rate over this wire IS, staged rather than argued — and before this repair the parked
/// arm sat on the same number, because `Unknown` bought a look every slice anyway.
///
/// # ⚠⚠⚠⚠ AND THE SECOND ARM IS THE ONE THAT COULD MAKE CHEAP INTO DEAF
///
/// `Settling::Nothing` — which is what a published, unpending verdict now crosses as — tells a
/// waiter it may park on the pane and look no more. **A verdict that then changes with the pane
/// producing nothing at all would be slept straight through**, and the run would report *the peer
/// never finished* about a peer that finished. An agent's own hook reports out of band, so that is
/// not a hypothetical: it is what every turn ending looks like at a hook-instrumented pane. The
/// second arm drives exactly that and requires the wait to come back with [`Over::Yes`].
#[test]
fn a_remote_driver_waiting_on_a_verdict_stops_asking_for_it_every_slice() {
    /// How long the measured wait may run. The peer never answers in the first arm, so this is
    /// dead time: at the poll interval it is ~200 looks, and parked it is a handful.
    const PATIENCE: Duration = Duration::from_secs(2);
    /// How many supervisor reads the parked wait may cost. Well above the 1 this fixture makes it
    /// (the pane never moves and nothing is pending) and far below what any slice-paced wait can
    /// reach, so it separates the two behaviours rather than tolerating one of them.
    const CEILING: u64 = 25;

    /// Wait out a peer that never answers, over a driver that either can or cannot park, and
    /// answer **how many times it asked the daemon for the verdict**.
    fn waited_out(parkable: bool) -> (u64, sprag_plugin::Over) {
        let (_host, sock) = spawn_host();
        let (driver, mut setup) = if parkable {
            parking_remote_driver(&sock)
        } else {
            remote_driver(&sock)
        };
        assert_eq!(
            driver.changes().is_some(),
            parkable,
            "the two arms must differ in exactly the thing being measured",
        );
        // The boot pane becomes an agent by its own report — published on sight, so nothing is
        // pending and the verdict this wait rests on can only be moved by another report.
        report_agent(&mut setup, PaneId(0), "working", Some("go"));

        let counted = CountingRemote::new(driver);
        let mut done = sprag_plugin::Completion::new(sprag_plugin::DoneWhen::Settles);
        // ⚠ ARMED BEFORE THE MEASUREMENT, and it must find an agent: an unarmed contract is never
        // satisfied, which is cheap for the wrong reason.
        done.begin(&counted, PaneId(0));
        let entered = counted.supervisions();
        let over = done.wait(
            &counted,
            PaneId(0),
            PATIENCE,
            None,
            &sprag_plugin::RunContext::uncancellable(),
        );
        (counted.supervisions() - entered, over)
    }

    let (parked_asks, parked_end) = waited_out(true);
    let (polled_asks, polled_end) = waited_out(false);

    // ── THE CONTROLS: both really waited, at a peer that really never answered ─────────────────
    assert_eq!(
        (parked_end, polled_end),
        (sprag_plugin::Over::NotYet, sprag_plugin::Over::NotYet),
        "⚠⚠⚠⚠ THE CONTROL: a wait that ended early costs nothing either, and the number below \
         would then be measuring a contract that answered rather than one that waited",
    );

    // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────────
    assert!(
        parked_asks * 5 < polled_asks,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 640: A REMOTE WAIT THAT RESTS ON THE VERDICT IS STILL ASKING FOR \
         IT EVERY SLICE. Parked {parked_asks} reads against {polled_asks} for a driver that cannot \
         be told anything — item 631 made this surface parkable and this arm did not get cheaper, \
         because `Settling::Unknown`'s deadline is *now plus a poll interval* and so falls due on \
         every one. Each of these reads is a socket round trip for one pane's verdict, on the path \
         an agent loop walks at every step of every turn",
    );
    assert!(
        parked_asks <= CEILING,
        "⚠⚠⚠ and the parked wait's cost follows the PANE and the DEADLINE, not the clock — two \
         seconds of silence at a settled pane must buy no reads at all. Got {parked_asks}",
    );

    // ── AND CHEAP MUST NOT MEAN DEAF ──────────────────────────────────────────────────────────
    let (_host, sock) = spawn_host();
    let (driver, mut setup) = parking_remote_driver(&sock);
    report_agent(&mut setup, PaneId(0), "working", Some("go"));
    let armed = driver
        .supervision()
        .expect("the daemon supervises")
        .pane_agent_state(PaneId(0))
        .expect("the pane reported itself an agent");
    let mut done = sprag_plugin::Completion::new(sprag_plugin::DoneWhen::Settles);
    done.begin(&driver, PaneId(0));
    let sock_for_peer = sock.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        let mut conn = HostConn::connect(&sock_for_peer, Duration::from_secs(5))
            .expect("the peer's own connection");
        // The turn: the peer takes a question (`asked_seq` moves) and then comes back to rest
        // (`seq` moves). Neither writes a byte to the pane, which is the whole hazard.
        report_agent(&mut conn, PaneId(0), "working", Some("the driver's prompt"));
        report_agent(&mut conn, PaneId(0), "idle", None);
    });
    let answered = done.wait(
        &driver,
        PaneId(0),
        PATIENCE,
        None,
        &sprag_plugin::RunContext::uncancellable(),
    );
    // ⚠⚠⚠⚠⚠ THE FIXTURE'S OWN CONTROL, ASKED AFTER THE WAIT AND ASSERTED BEFORE IT. *The peer's
    // turn ended* and *the wait noticed* are two facts, and a gate that only checks the second
    // cannot tell a deaf wait from a staging that never moved the peer. So the daemon is asked what
    // it holds now, and the three terms the contract pairs on are checked against what the turn was
    // ARMED at — if these hold and the wait still says `NotYet`, the wait is the defect.
    let after = driver
        .supervision()
        .expect("the daemon supervises")
        .pane_agent_state(PaneId(0))
        .expect("the pane is still an agent");
    assert_eq!(
        (
            after.state,
            after.seq > armed.seq,
            after.asked_seq > armed.asked_seq
        ),
        (sprag_detect::AgentState::Idle, true, true),
        "⚠⚠⚠⚠ THE STAGING FAILED, so the assertion below would be about a peer that never \
         answered rather than about a wait that did not hear. Armed at {armed:?}, and the daemon \
         now holds {after:?}",
    );
    assert_eq!(
        answered,
        sprag_plugin::Over::Yes,
        "⛔⛔⛔⛔⛔ A VERDICT THAT MOVED WITH THE PANE STANDING STILL WAS SLEPT THROUGH. \
         `Settling::Nothing` says *park on the pane and look no more*, and an agent's own hook \
         reports OUT OF BAND — so a turn ending at a hook-instrumented pane changes the verdict \
         without a byte reaching the screen. The control directly above proves the peer's turn DID \
         end: the daemon holds {after:?} against the {armed:?} this turn was armed at. A wait deaf \
         to that reports *the peer never finished* about a peer that finished, which is worse than \
         the polling it replaced",
    );

    println!(
        "\n== what a remote wait on a VERDICT costs, {PATIENCE:?} at a pane that says nothing \
         ==\n  parked, deadline published: {parked_asks} supervisor read(s)\n  control, a driver \
         that cannot be told: {polled_asks}\n  before item 640 the parked arm sat on the control's \
         number: `Settling::Unknown` falls due every slice\n",
    );
}

/// ⛔⛔⛔⛔ **THE FUNCTION EVERY PLUGIN TYPES THROUGH DRIVES A REAL PANE FROM ANOTHER PROCESS** —
/// register item 544, stage 1, and the claim the whole item is for.
///
/// # ⚠⚠⚠⚠⚠ Why this is the gate and not a unit test of the client
///
/// The item's defect is that **two things with different natural lifetimes share one process**: a
/// multiplexer that owns pseudoterminals for weeks, and a run supervisor whose life is the work.
/// Because the driver is compiled into the daemon, *"change how a loop reflects"* has meant
/// *"restart the thing holding your PTYs"*. Nothing about that is settled by a client that answers
/// the right JSON — it is settled by **the plugin layer's own typing function, unchanged, driving a
/// pane it cannot reach any other way.** So this test runs `sprag_plugin::deliver` in the TEST
/// process against a `sprag-term` in another one, and the shell in that pane runs what it was told.
///
/// # ⚠⚠⚠ The three things asserted, and why each is separate
///
/// * **THE SHELL RAN IT.** `ran-42` cannot appear from an echo: the text typed is
///   `echo ran-$((21+21))`, so the arithmetic is the SHELL's and nothing but a submit that landed
///   produces it. That is the delivery's whole contract — text on the screen, then Enter, then a
///   program that acted.
/// * **THE BYTE COUNT CROSSED THE WIRE.** Exactly the text plus one for Enter. A door that answered
///   no count, or a client that fabricated one, would leave a run charging its own cost from a
///   guess — and this is the only address on this wire that answers what a write actually wrote.
/// * **THE ECHO QUESTION IS UNANSWERED, AND SAID SO.** `OnScreenOnly { echo: None }` is the honest
///   verdict for a host that cannot ask a pane's terminal about its modes; a remote driver that
///   reported `Confirmed` would be claiming the PROGRAM took the text on evidence it never had.
///   The absence is the stage's own residue, named here rather than left to be discovered.
///
/// ⚠ The barrier is the pane's own prompt, read through the driver's surface. Not a sleep, and not
/// `has_painted`: that reads a per-row damage GENERATION, which this wire deliberately does not
/// publish — so a remote driver waits for what it can see, which is what the loop's own
/// readiness contract does too.
#[test]
fn the_door_every_plugin_types_through_drives_a_real_pane_from_another_process() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh"] }));

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains('$'))),
        "⚠⚠ the shell never printed a prompt through the driver's own surface, so nothing below \
         would be measuring a delivery — it would be measuring a race. Read {:?}",
        remote.pane_collapsed(pane),
    );

    let text = "echo ran-$((21+21))";
    let delivered = deliver(
        &remote,
        &RunContext::uncancellable(),
        pane,
        text,
        &Delivery::new(),
    )
    .expect("a delivery through the remote surface");

    // ⚠⚠⚠⚠⚠ THE VERDICT IS ASSERTED AGAINST THE READING, NOT AGAINST A PLATFORM'S SHELL — and CI
    // is what taught this gate the difference. It used to assert `echo.is_none()` (*"a remote
    // surface cannot ask a pane's terminal who echoes"*), which item 557 made false; the version
    // after that asserted `OnScreenOnly` with `ByTheTerminal`, on the assumption that *a shell
    // echoes*. **That is not true of a shell, it is true of `dash`.** Linux's `/bin/sh` is dash;
    // macOS's is bash, and an interactive readline shell takes its terminal RAW at the prompt and
    // echoes the characters ITSELF — so macOS answered `ByTheProgram`, `deliver` answered
    // `Confirmed`, and BOTH were right.
    //
    // ⚠⚠⚠⚠⚠ AND THE MODE IS NOT RE-READ HERE TO CHECK THE VERDICT AGAINST IT, WHICH THE FIRST FIX
    // DID. `deliver` reads the mode at one instant; a second read is a second instant, and a
    // readline shell FLIPS the discipline between its prompt and running a command — so the
    // comparison would be of two values taken at different moments, which is this workspace's own
    // recorded trap (items 368, 429). **Measured, not feared**: with the `ByTheTerminal` assertion
    // in place this gate failed on two macOS runs and passed on a third with IDENTICAL code.
    //
    // The mode-to-verdict derivation is `deliver`'s OWN gate's claim, over a fixture that declares
    // the mode instead of hosting a shell that changes it. What THIS gate owns is stage 1c's: the
    // plugin layer's typing function drives a real pane from another process. So it asserts the two
    // verdicts that are honest, the count, and — the part no mode can fake — that the shell RAN it.
    let (attempts, written) = match delivered {
        Delivered::OnScreenOnly {
            attempts, written, ..
        }
        | Delivered::Confirmed { attempts, written } => (attempts, written),
        other => panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 544: `deliver` over the wire answered {other:?}. A driver \
             outside the daemon must reach the same verdict the in-process one does — the text \
             read back off a screen it changed, then the submit. Anything else means this seam \
             cannot carry the loop."
        ),
    };
    assert_eq!(
        attempts, 1,
        "the first injection was not confirmed, so the byte count below is a sum over retries \
         rather than the write this gate is about",
    );
    assert_eq!(
        written.bytes(),
        text.len() as u64 + 1,
        "⛔⛔⛔⛔ REGISTER ITEM 544: the door must answer WHAT IT WROTE — {} bytes of text and one \
         for Enter. A remote driver cannot compute this for itself: what a stroke becomes is the \
         encoder's answer under the pane's LIVE input modes, which the program may change between \
         any read and any write, so the count is the writer's to report or it is a guess.",
        text.len(),
    );

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_full_text(pane)
            .is_some_and(|out| out.contains("ran-42"))),
        "⛔⛔⛔⛔ REGISTER ITEM 544: the shell never ran the command. `ran-42` is arithmetic only \
         the SHELL performs — the typed line reads `{text}` — so its absence means the submit did \
         not land, and a driver in another process cannot yet do the one thing a driver is for. \
         Read {:?}",
        remote.pane_full_text(pane),
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **THE REMOTE DOOR REFUSES A PANE WHOSE CHILD HAS GONE, AND THE LIVE PANE BESIDE IT TAKES
/// THE WRITE** — register item 544, stage 1, and the refusal that keeps a remote driver out of the
/// wedge that held a build machine for 43 hours.
///
/// # ⚠⚠⚠⚠⚠ Why a remote driver needs this more than an in-process one
///
/// A pseudoterminal whose child has exited is not a hole, it is a wall with a queue: it takes
/// 16,896 bytes and then blocks for ever. The in-process door reads the pane's own EOF atomic and
/// refuses before writing; a driver that reached the pane through the display client's `key` verb
/// would have no such door and would type its stimulus every step, patiently, all the way there.
///
/// # ⚠⚠⚠ The pair is the claim, and the third arm is the one a skew would break
///
/// * The DEAD pane must refuse, and refuse **as the typed cause** — `PeerGone`, not some other
///   error. A driver that read *some other error* would retry, which is the march.
/// * The LIVE pane, on the same daemon and the same connection, must take the write and say how
///   much. Without it, a door hard-wired to refuse would pass.
/// * A pane NOBODY KNOWS must answer `UnknownPane` and not the daemon-skew sentence. Both arrive as
///   one JSON-RPC fault with one payload word, and only the pane list tells them apart — so this
///   arm is what holds that discrimination in place.
#[test]
fn a_remote_door_refuses_a_pane_whose_child_has_gone_and_the_live_one_beside_it_is_written_to() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);

    let living = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));
    let dying = spawn_pane(&mut setup, json!({ "cmd": ["true"] }));

    assert_eq!(
        remote
            .inject(living, &KeyStroke::text("ping"))
            .map(Written::bytes),
        Ok(4),
        "⚠⚠⚠ THE CONTROL: a pane running `cat` takes the write, and the door says how many bytes \
         it put on the pseudoterminal. A refusal here would report every pane dead, which reads to \
         a driver as *stop typing* — a loop that refuses to work at a perfectly good peer.",
    );

    assert!(
        wait_until(Duration::from_secs(5), || remote.pane_eof(dying)
            == Some(true)),
        "the pane whose child exits at once never reported EOF through the driver's own surface, \
         so the refusal below would be measuring nothing",
    );
    assert_eq!(
        remote.inject(dying, &KeyStroke::text("x")),
        Err(PaneError::PeerGone(dying)),
        "⛔⛔⛔⛔ REGISTER ITEM 544: typing into a pane whose child has EXITED must be refused, and \
         refused as PeerGone. A remote driver handed anything else has one remedy — try again — \
         and the write it retries is the one that fills a dead pty's buffer and blocks for ever.",
    );

    assert_eq!(
        remote.inject(PaneId(4242), &KeyStroke::text("x")),
        Err(PaneError::UnknownPane(PaneId(4242))),
        "⚠⚠⚠ a pane nobody knows and a daemon too old to have this door arrive as the SAME fault \
         with the SAME payload word. Telling them apart is what the pane list is for here, and \
         getting it wrong tells an operator to restart a daemon that is perfectly current.",
    );

    assert!(
        remote.inject(living, &KeyStroke::text("g")).is_ok(),
        "⚠⚠⚠ the door must answer about the pane it NAMES: one child exiting has been read as \
         every pane being gone",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER OUTSIDE THE DAEMON READS A WRAPPED PANE THE FOUR WAYS IT READS ONE INSIDE** —
/// register item 544, stage 1, and the client half of stage 1b's argument.
///
/// # ⚠⚠⚠⚠⚠ Every one of the four is a DIFFERENT answer about the same output
///
/// One pane, five columns wide, printing `OLD`, `GO`, then `TOOL UP` — so the last line wraps after
/// the SPACE and `OLD` scrolls into the scrollback. What each read must answer, and what a client
/// that derived it from a neighbour would answer instead:
///
/// * `pane_collapsed` — each row's SHARE of its logical line, so `GOTOOL UP`. Joining the rendered
///   rows drops the space the wrap sat on and answers `GOTOOLUP`; a barrier waiting for `TOOL UP`
///   then never clears, and **the width is not the driver's to choose** — whichever display client
///   attached decides it.
/// * `pane_rows` — what each row RENDERS, trailing blanks trimmed: `GO`, `TOOL`, `UP`. Serving the
///   shares here would make the two reads one answer twice.
/// * `pane_full_lines` — the LOGICAL lines the child wrote: `OLD`, `GO`, `TOOL UP`. ⚠ The trait's
///   own default splits the RENDERED text instead, which answers FOUR lines and breaks `TOOL UP`
///   in half. That default is a documented degradation for a host that cannot answer the content
///   question; this one can, so taking the default would be a rendering published as content.
/// * `pane_full_text` — the rendered whole, scrollback included, which is the only one of the four
///   that still holds `OLD`. It is also this gate's NON-VACUITY: without it, "the screen reads have
///   scrolled past `OLD`" is satisfied by a pane that never scrolled at all.
///
/// ⚠⚠ And the rows' `generation` is ZERO, asserted rather than ignored. A damage generation is a
/// PAINT signal that a resize or a palette change stamps while no program writes a byte; four
/// plugins in this workspace read it as *what did the peer produce* and each reported something
/// false. So the wire does not carry it, and this surface says so in the value rather than
/// inventing one.
#[test]
fn a_remote_surface_reads_a_wrapped_pane_the_four_ways_a_driver_reads_it() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(
        &mut setup,
        json!({
            "cmd": ["sh", "-c", "printf 'OLD\\nGO\\nTOOL UP'"],
            "cols": 5,
            "rows": 3,
        }),
    );

    assert!(
        wait_until(Duration::from_secs(5), || remote.pane_eof(pane)
            == Some(true)),
        "the printing child never reached EOF, so nothing below is reading a settled screen",
    );

    let rows = remote.pane_rows(pane).expect("the pane's rows");
    assert_eq!(
        rows.iter().map(|row| row.text.clone()).collect::<Vec<_>>(),
        vec!["GO".to_owned(), "TOOL".to_owned(), "UP".to_owned()],
        "⛔⛔⛔⛔ REGISTER ITEM 544: the rows a remote driver reads must be what each row RENDERS",
    );
    assert!(
        rows.iter().all(|row| row.generation == 0),
        "⚠⚠⚠ the paint generation is deliberately not published, so a remote row must not carry \
         one. A number invented here hands a driver the mistake four plugins already made — \
         reading a repaint as the peer having produced something. Got {rows:?}",
    );
    assert_eq!(
        remote.pane_collapsed(pane).as_deref(),
        Some("GOTOOL UP"),
        "⛔⛔⛔⛔ REGISTER ITEM 544: the collapsed screen must join each row's SHARE of its logical \
         line, so a marker the terminal wrapped still matches. Deriving it from the rows answers \
         `GOTOOLUP`, and the barrier never clears.",
    );
    assert_eq!(
        remote.pane_full_lines(pane),
        Some(vec![
            "OLD".to_owned(),
            "GO".to_owned(),
            "TOOL UP".to_owned()
        ]),
        "⛔⛔⛔⛔ REGISTER ITEM 544: the logical lines must be READ, not derived. The trait's \
         default splits the rendered text and answers four lines with `TOOL UP` broken in half — \
         which makes every marker this driver matches depend on somebody else's window width.",
    );
    let full = remote.pane_full_text(pane).unwrap_or_default();
    assert!(
        full.contains("OLD"),
        "the pane never scrolled, so the screen-scoped assertions above measure nothing. \
         `full_text` read {full:?}",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A MODIFIED KEYSTROKE CROSSES THE WIRE AS THE CHORD IT IS** — register item 544, stage 1.
///
/// # ⚠⚠⚠ The pair is the claim, because a dropped modifier is a SUCCESSFUL write of the wrong byte
///
/// `a` and `C-a` are one byte each and both are accepted, so nothing about the answer distinguishes
/// them: a client that lost the modifier reports the same count for both. What separates them is
/// the pane's own echo — `a` for the character, `^A` for the control byte the line discipline
/// renders. So the bare stroke goes first and the chord after it, and the screen has to show both.
///
/// ⚠ Every modifier is spelled on every stroke this surface sends (`false` included), which is why
/// one chord holds the claim for the form rather than for one flag: a form assembled per-stroke
/// from whichever flags happened to be set is the shape where the fifth one is forgotten.
#[test]
fn a_modified_keystroke_reaches_a_pane_as_the_chord_it_is() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));

    assert_eq!(
        remote
            .inject(pane, &KeyStroke::text("a"))
            .map(Written::bytes),
        Ok(1),
        "the bare character is one byte on the pseudoterminal",
    );
    assert!(
        wait_until(Duration::from_secs(5), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains('a'))),
        "⚠⚠ THE CONTROL: the pane never echoed the plain character, so the chord below would be \
         measuring a pane that echoes nothing. Read {:?}",
        remote.pane_collapsed(pane),
    );

    assert_eq!(
        remote
            .inject(
                pane,
                &[KeyStroke {
                    key: "a".to_owned(),
                    mods: Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    },
                }],
            )
            .map(Written::bytes),
        Ok(1),
        "⚠⚠⚠ THE CHORD COSTS THE SAME ONE BYTE AS THE CHARACTER, which is why the count cannot \
         tell them apart and the pane's own echo is the only witness",
    );
    assert!(
        wait_until(Duration::from_secs(5), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains("^A"))),
        "⛔⛔⛔⛔ REGISTER ITEM 544: the pane never received the CONTROL byte — a client that drops \
         a modifier writes the plain character instead, successfully, and reports the same byte \
         count for both. Read {:?}",
        remote.pane_collapsed(pane),
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER OUTSIDE THIS PROCESS READS WHAT THE AGENT IN A PANE IS DOING, AND THE TWO
/// ABSENCES BESIDE IT STAY TWO** — register item 557, the supervision half of item 544's stage 1.
///
/// # ⚠⚠⚠⚠⚠ Why this read is the one that decides whether the loop may live elsewhere
///
/// `outer.rs` consults [`PaneAccess::supervision`] on five production paths: it is how a run learns
/// that its peer took the prompt, that a turn ended, and that the peer is asking for a person. A
/// remote driver that answered `None` there would report *this build cannot supervise anything*
/// about a daemon that is supervising perfectly well — so every run driven from outside would hand
/// itself to a human on its first step.
///
/// # ⚠⚠⚠⚠⚠ The two absences a single word collapses, which is the defect this gate holds shut
///
/// [`PaneAccess::supervision`] answering `None` means *ask a person, this build cannot look*.
/// `PaneSupervision::pane_agent_state` answering `None` means *this pane is not an agent*. They are
/// OPPOSITE instructions, and a surface that publishes one `unknown` word for both lets a
/// supervisor conclude "no agents here" from a host that never looked. The rival terminal cloned in
/// this tree does exactly that — one `AgentStatus::Unknown` serves both (herdr
/// `src/api/schema/common.rs:151`, read at its pin `9a4ce5e1`) — so this gate asserts the plain
/// pane's `None` **in the same breath** as the host's `Some`, which is the only arrangement in
/// which the distinction is observable at all.
///
/// # ⚠⚠⚠ And WHAT the remote answer carries, not merely that it exists
///
/// * `reports` — how many reports this pane has ACCEPTED, whatever they said. It is the one counter
///   that moves while a turn is merely working (register item 458): `seq` stands still through a
///   turn calling tool after tool, and `asked_seq`/`said_seq` stand still through a turn still in
///   flight. A remote observation without it cannot tell a thinking peer from one that stopped
///   speaking, which is the judgement a supervisor exists to make.
/// * `authority` — REPORTED (and by whom) versus SCRAPED (and by which rule). A driver handed the
///   flattened form would act on screen evidence believing it had the agent's own statement.
/// * `asking` — the menu this daemon already parsed. Its absence would make a remote driver
///   re-derive a question off a screen the daemon read in the same instant, which is the tax R367
///   was filed over, one process further out.
#[test]
fn a_remote_driver_reads_a_pane_agent_verdict_and_a_plain_pane_is_a_different_absence() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);

    // The agent's pane: a menu on the screen, then `cat` so the child stays and the screen holds.
    let agent = spawn_pane(
        &mut setup,
        json!({
            "cmd": ["sh", "-c", "printf 'Pick one:\\n\\342\\235\\257 1. first\\n  2. second\\n'; cat"],
            "cols": 40,
            "rows": 10,
        }),
    );
    // ...and a pane no manifest claims, on the same host and the same connection.
    let plain = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(agent)
            .is_some_and(|screen| screen.contains("2. second"))),
        "the menu never reached the screen, so the `asking` claim below would measure a race. \
         Read {:?}",
        remote.pane_collapsed(agent),
    );

    setup
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(REPORT_AGENT_ACTION),
                "args": {
                    "id": agent.0,
                    "source": "hook:claude",
                    "state": "blocked",
                    "name": "claude",
                    "asked": "which one?",
                    "said": "I need a person",
                    "noticed": "the menu is up",
                    "transcript": "/tmp/a-session.jsonl",
                },
            }),
        )
        .expect("the agent reports its own state over the wire");

    // ── THE HOST CAN LOOK ──────────────────────────────────────────────────────────────────────
    let supervisor = remote.supervision().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a remote driver reports that this build CANNOT SUPERVISE \
             ANYTHING — about a daemon that is supervising the two panes it was just asked about. \
             That verdict means *hand this to a person*, so every run driven from outside the \
             daemon stops on its first step. `outer.rs` asks it on five production paths."
        )
    });

    let seen = supervisor.pane_agent_state(agent).unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: the pane that just REPORTED itself an agent reads as *not \
             an agent* through a remote driver's surface. The daemon holds the verdict — it is on \
             the pane list in the same instant — and the driver has no address to ask it at."
        )
    });

    assert_eq!(
        seen.state,
        sprag_detect::AgentState::Blocked,
        "the verdict itself must cross: a driver reading the wrong state acts on the wrong turn",
    );
    assert_eq!(
        seen.agent.as_deref(),
        Some("claude"),
        "WHICH agent, as the reporter named it",
    );
    assert_eq!(
        seen.authority,
        sprag_plugin::Authority::Reported {
            source: "hook:claude".to_owned()
        },
        "⚠⚠⚠⚠ WHERE THE ANSWER CAME FROM, and so how much it is worth. A remote surface that \
         flattened this into a screen guess would have a driver act on the weaker evidence without \
         ever learning there was stronger — and one that flattened it the other way would report a \
         scrape as the agent's own statement",
    );
    assert!(
        seen.reports >= 1,
        "⛔⛔⛔⛔ REGISTER ITEM 458, one process out: `reports` is the ONLY counter that moves \
         while a turn is merely working — `seq` stands still through a turn calling tool after \
         tool, and `asked_seq`/`said_seq` stand still through a turn still in flight. Without it a \
         remote supervisor cannot tell a thinking peer from one that stopped speaking. Got {}",
        seen.reports,
    );
    assert_eq!(
        seen.asked.as_deref(),
        Some("which one?"),
        "the agent's own account of what it was asked, carried through untouched",
    );
    assert_eq!(
        seen.said.as_deref(),
        Some("I need a person"),
        "...and what it answered — the half a driver was reading off a pane that cannot be read \
         for it (register item 441)",
    );
    assert_eq!(
        seen.noticed.as_deref(),
        Some("the menu is up"),
        "...and WHY it wants a person, which is precisely the case a run has to hand to one",
    );
    assert_eq!(
        seen.transcript.as_deref(),
        Some("/tmp/a-session.jsonl"),
        "...and where it writes, which no remote reader can derive from a session id",
    );
    let asking = seen.asking.as_ref().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: the daemon PARSED this menu — it is on the pane list in \
             the same instant — and a remote driver is handed nothing, so it must re-derive the \
             question off a screen somebody has already read. Observation was {seen:?}"
        )
    });
    assert_eq!(
        asking
            .choices
            .iter()
            .map(|choice| (choice.number, choice.label.as_str(), choice.selected))
            .collect::<Vec<_>>(),
        vec![(1, "first", true), (2, "second", false)],
        "⚠⚠⚠ the menu crosses with its SELECTION intact: which option a bare Enter would take is \
         the answer a caller gets by doing nothing, and a driver that lost it cannot tell a \
         consent from an accident",
    );

    // ── ONE BUILDER, TWO READERS ───────────────────────────────────────────────────────────────
    // ⚠⚠⚠⚠ The listing has carried this object since H3 and the address is new, so *they are the
    // same object* is a claim exactly one round old — and the way it would break is the way every
    // duplicated shape in this repository has broken: a key added at one site and not the other,
    // with both looking perfectly well-formed. Asserted whole rather than key by key, so a key that
    // ARRIVES at one site is as red as a key that leaves.
    //
    // ⚠⚠⚠⚠ **ONE KEY HERE IS TIME-VARYING AND THIS COMPARISON ONLY HOLDS BECAUSE NOTHING IS
    // PENDING** — register item 640. `settles_in_ms` is a REMAINING time computed where each answer
    // is built, so two renderings of one PENDING verdict legitimately differ by however long
    // separates them, and a whole-object equality across two calls would flake. This pane's verdict
    // was REPORTED, and a report publishes on sight — so `settling` reads `nothing` at both sites
    // and no duration is written at either. A future round that stages a settling verdict here must
    // compare the keys it means rather than the object.
    let listed = setup
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list on the test's own connection");
    let entry = listed
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry[PANE_SUMMARY_ID_KEY].as_u64() == Some(agent.0))
        .cloned()
        .expect("the agent's pane is on the list");
    let addressed = setup
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&agent_slot_for(agent.0)) }),
        )
        .expect("the same verdict at its own address");
    assert_eq!(
        entry["agent"], addressed,
        "⛔⛔⛔⛔ REGISTER ITEM 557: the pane list and the pane's own address must publish the SAME \
         verdict, because they are one builder. Two literals answering one question drift first in \
         whichever key one of them forgot — and a driver reading the address while a person reads \
         the list would then be supervising a peer the screen disagrees about",
    );

    // ── AND THE PANE THAT IS SIMPLY NOT AN AGENT, IN THE SAME BREATH ───────────────────────────
    assert!(
        supervisor.pane_agent_state(plain).is_none(),
        "⚠⚠⚠⚠⚠ a pane no manifest claims must read as NOT AN AGENT. An address answering \
         something here would have a driver supervise a shell — and one answering the neighbouring \
         pane's verdict would have it supervise the wrong peer",
    );
    assert!(
        remote.supervision().is_some(),
        "⛔⛔⛔⛔ REGISTER ITEM 557, AND THE WHOLE POINT: the plain pane's `None` above must not be \
         readable as *this host cannot look*. The two are OPPOSITE instructions — *this pane is \
         not an agent* versus *ask a person, nothing here can supervise* — and a surface that \
         publishes one word for both lets a supervisor conclude «no agents here» from a host that \
         never looked. The rival terminal in this tree collapses them into one `unknown`",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER OUTSIDE THIS PROCESS ROLLS A PANE'S SESSION, AND THE REPLACEMENT IS THE SAME
/// PROGRAM IN THE SAME WORLD AND THE SAME SEAT** — register item 557's `lifecycle` surface, and the
/// act `outer.rs::replace` is.
///
/// # ⚠⚠⚠⚠⚠ Why `respawn` is a VERB and not the composition a client would obviously write
///
/// This is stage 1b's lesson on the ACTING side. A remote driver has `spawn` and `close`, so it
/// *could* assemble a replacement — and the assembly loses four things that are in neither call:
///
/// * **The WORLD.** argv, environment, working directory and SIZE are read off the outgoing pane. A
///   client that passed argv alone starts the same PROGRAM somewhere else, at somebody else's size.
/// * **The SEAT** — the name a person gave the pane, who opened it, what it may spend — handed over
///   as ONE operation. Register item 478 is what a forgotten declaration costs, and a composition
///   forgets them one at a time.
/// * **THE ORDER**: the old pane dies only once the new one exists, so a failed spawn leaves the run
///   holding the pane it had rather than having destroyed the session it was preserving.
/// * **The refusal that is a real case**: a pane with no recorded argv says so instead of being
///   handed an invented shell.
///
/// # ⚠⚠⚠ What each assertion below would catch on its own
///
/// The pane is born 33x7, in a directory that is NOT the daemon's, under a name, running a program
/// that prints its own working directory. So a replacement assembled from `spawn` answers 80x24, in
/// the daemon's directory, unnamed — three separate reds — and one that never re-ran the command
/// prints nothing at all.
#[test]
fn a_remote_driver_replaces_a_pane_and_the_new_one_is_the_same_program_in_the_same_seat() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);

    // A directory that is NOT the daemon's, so "the replacement kept the cwd" is a claim about the
    // PANE rather than about whatever the host happens to be standing in.
    let elsewhere = std::env::temp_dir();
    let home = elsewhere.canonicalize().unwrap_or(elsewhere);
    let home = home.to_string_lossy().into_owned();
    let old = spawn_pane(
        &mut setup,
        json!({
            "cmd": ["sh", "-c", "pwd; cat"],
            "cwd": home,
            "cols": 33,
            "rows": 7,
            "name": "inner-session",
        }),
    );

    let lifecycle = remote.lifecycle().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a remote driver reports that this host CANNOT OPEN PANES. \
             `outer.rs::replace` turns that into *a loop cannot replace its inner session*, which \
             ends every run driven from outside the daemon at its first session rollover."
        )
    });

    // ⚠⚠⚠⚠⚠ READ THROUGH `full_lines`, NOT `full_text`, AND THAT IS STAGE 1b'S OWN LESSON BITING
    // THIS GATE. `full_text` is the RENDERED text, so a line the terminal WRAPPED carries a newline
    // that the program never printed — and this pane is 33 columns wide on purpose. It passed on
    // Linux (`/tmp` is four characters) and failed on macOS, whose temp directory is sixty-four:
    // the path came back as `…djsxfhc17\nx95674wsm…`. The CONTENT question is `full_lines`', which
    // is why the two addresses exist.
    let printed_home = |lines: Vec<String>| lines.iter().any(|line| line.contains(&home));
    assert!(
        wait_until(Duration::from_secs(10), || printed_home(
            remote.pane_full_lines(old).unwrap_or_default()
        )),
        "the original never printed its working directory, so the comparison below would have \
         nothing to compare. Read {:?}",
        remote.pane_full_lines(old),
    );

    // ── THE REPLACEMENT ────────────────────────────────────────────────────────────────────────
    let fresh = lifecycle.respawn(old).unwrap_or_else(|error| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a driver in another process could not replace a pane — \
             {error:?}. Nothing it holds can substitute: `close` then `spawn` starts the same \
             program in a different world and drops the seat, and doing it in that order destroys \
             the session on a spawn that fails."
        )
    });
    assert_ne!(
        fresh, old,
        "a replacement is a NEW pane; answering the old id would tell a driver its rollover \
         happened when nothing had moved",
    );

    assert!(
        wait_until(Duration::from_secs(10), || printed_home(
            remote.pane_full_lines(fresh).unwrap_or_default()
        )),
        "⛔⛔⛔⛔ REGISTER ITEM 557: the replacement did not re-run the pane's OWN command in the \
         pane's OWN directory. `pwd` printing anything else means the world was rebuilt from what \
         the caller knew rather than from what the pane was — and a caller knows the argv at best, \
         never the cwd or the environment. Read {:?}",
        remote.pane_full_lines(fresh),
    );

    // ── THE SEAT, AND THE SIZE, READ OFF THE DAEMON'S OWN LISTING ──────────────────────────────
    let listed = setup
        .call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list on the test's own connection");
    let entries: Vec<Value> = listed.as_array().cloned().unwrap_or_default();
    let entry = entries
        .iter()
        .find(|entry| entry[PANE_SUMMARY_ID_KEY].as_u64() == Some(fresh.0))
        .unwrap_or_else(|| panic!("the replacement is on the daemon's list: {entries:?}"));
    assert_eq!(
        entry["name"].as_str(),
        Some("inner-session"),
        "⛔⛔⛔⛔ REGISTER ITEM 478, over the wire: the SEAT must follow the replacement. A name is \
         how everything else addresses this pane — a person, a sibling agent, the run's own \
         records — so a rollover that dropped it would leave a live session nothing could reach by \
         the only name it was ever known by. Entry: {entry}",
    );
    assert_eq!(
        (entry["cols"].as_u64(), entry["rows"].as_u64()),
        (Some(33), Some(7)),
        "⚠⚠⚠⚠ the replacement must be the same SIZE. A spawn-shaped composition answers the \
         daemon's default (80x24) here, and a program re-wrapped at a width nobody chose is the \
         defect stage 1b measured from the reading side. Entry: {entry}",
    );

    // ── AND THE OUTGOING PANE IS GONE, WHICH IS WHAT MAKES IT A REPLACEMENT ────────────────────
    assert!(
        !remote.pane_ids().contains(&old),
        "⚠⚠⚠ the pane that was replaced must be reaped: two live panes running one session is not \
         a rollover, it is a fork — and the run would go on typing into whichever it still holds",
    );

    // ── THE REFUSAL, WHICH IS A REAL CASE AND NOT A DEFENSIVE ONE ─────────────────────────────
    let ghost = lifecycle.respawn(old);
    assert!(
        matches!(ghost, Err(PaneError::Spawn(_))),
        "⚠⚠⚠⚠ replacing a pane nobody holds must REFUSE with the reason. A fabricated id here \
         would hand a driver a pane that does not exist, and its next act is to type into it. \
         Got {ghost:?}",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A BARRIER DRIVEN FROM ANOTHER PROCESS DOES NOT CONVERGE ON THE DRIVER'S OWN
/// KEYSTROKE** — register item 557's `input_echo` surface, driven through the loop's REAL consumer.
///
/// # ⚠⚠⚠⚠⚠ The defect, which is a race rather than a wrong answer
///
/// A pseudoterminal ECHOES what is written into it, and on the grid that echo is ordinary output.
/// So `ReadyWhen::Prints("MARK")` matching the screen cannot tell *the program printed MARK* from
/// *the driver typed MARK and the terminal handed it back* — and which one it sees depends on
/// whether the echo landed before the barrier armed. The same call therefore converges or feeds the
/// shell depending on scheduling. `Readiness` refuses a marker that is in the pane's ECHO TRAIL,
/// and until this address existed a remote driver read that trail as EMPTY: every marker was
/// accepted, and the refusal that makes the answer deterministic was silently not running.
///
/// # ⚠⚠⚠ Why this drives `Readiness` rather than asserting the string
///
/// The address answering the right text proves the wire. It does not prove that the LOOP's barrier
/// is the thing consuming it — and item 557's claim is about the loop. So this constructs the same
/// `Readiness` an `ai_loop` run does and calls `reached` with the REMOTE surface, twice:
///
/// * **REFUSED** where the marker is on the screen because the driver typed it — with the screen
///   read back at that instant, so the refusal is not satisfied by a pane that shows nothing.
/// * **ACCEPTED** where a second marker was PRINTED by the shell and never typed — the control that
///   keeps the first arm from passing on a barrier that simply never clears.
#[test]
fn a_remote_barrier_refuses_the_marker_the_driver_typed_and_takes_the_one_the_shell_printed() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh"], "cols": 60, "rows": 12 }));

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains('$'))),
        "the shell never printed a prompt, so nothing below is measuring a settled pane. Read {:?}",
        remote.pane_collapsed(pane),
    );

    // ── ARM FIRST. `Prints` counts occurrences AGAINST the count at arming, so a barrier armed
    // after the text is on screen is a barrier that can never clear — this workspace's own
    // longest-running flake, recorded in `Reached::RunEnded`'s doc.
    let run = RunContext::uncancellable();
    let mut typed_barrier = Readiness::new(
        Some(ReadyWhen::Prints("TYPEDMARK".to_owned())),
        Some(Duration::from_millis(1500)),
        None,
        Attended::NoOne,
    );
    let armed = typed_barrier.reached(&remote, pane, &run);
    assert!(
        matches!(
            armed,
            Err(PaneError::NeverReady {
                already_showing: false,
                ..
            })
        ),
        "the barrier must NOT be down before anything was typed, and its baseline must record that \
         the marker was NOT already showing — otherwise the refusal below is about a latch, or \
         about a count that was poisoned at arming. Got {armed:?}",
    );

    // ── THE DRIVER TYPES THE MARKER, AND THE TERMINAL HANDS IT BACK ────────────────────────────
    // No Enter: the shell never runs it, so every occurrence on that screen is the ECHO and
    // nothing else. That is what makes this arm about the trail rather than about output.
    assert_eq!(
        remote
            .inject(pane, &KeyStroke::text("TYPEDMARK"))
            .map(Written::bytes),
        Ok(9),
        "the WHOLE marker reached the pseudoterminal — a partial write would put a different \
         string on the screen and the refusal below would be about the wrong text",
    );
    assert!(
        wait_until(Duration::from_secs(5), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains("TYPEDMARK"))),
        "⚠⚠⚠ THE NON-VACUITY: the marker must actually BE on the screen, or the refusal below is \
         satisfied by a pane that shows nothing and proves nothing. Read {:?}",
        remote.pane_collapsed(pane),
    );

    let verdict = typed_barrier.reached(&remote, pane, &run);
    assert!(
        matches!(verdict, Err(PaneError::NeverReady { .. })),
        "⛔⛔⛔⛔ REGISTER ITEM 557: the barrier took the driver's OWN KEYSTROKE for the program's \
         output. The marker is on that screen only because a pty echoes what is written into it, \
         and the read that tells the two apart is the pane's echo trail — which a remote driver \
         could not ask for until this address existed, so it read EMPTY and every marker was \
         accepted. That is not a wrong answer, it is a RACE: the same call converges or feeds the \
         shell depending on whether the echo landed first. Got {verdict:?}",
    );

    // ── THE CONTROL: A MARKER THE SHELL PRINTS AND NOBODY TYPED ────────────────────────────────
    // The command is spelled so that the TYPED text does not contain the marker while the OUTPUT
    // does — `printf 'SAID%s\n' MARK` — which is the only way one pane can carry both arms.
    let mut printed_barrier = Readiness::new(
        Some(ReadyWhen::Prints("SAIDMARK".to_owned())),
        Some(Duration::from_secs(4)),
        None,
        Attended::NoOne,
    );
    // ⚠⚠⚠ ARMED BEFORE THE COMMAND RUNS, for the reason the first barrier states: the baseline is
    // taken on the FIRST `reached`, so arming after the output is on screen is a barrier that can
    // never clear. Asserting `already_showing: false` here is what proves the baseline is honest.
    let printed_arm = printed_barrier.reached(&remote, pane, &run);
    assert!(
        matches!(
            printed_arm,
            Err(PaneError::NeverReady {
                already_showing: false,
                ..
            })
        ),
        "the control's marker must not be on screen before its command runs: {printed_arm:?}",
    );
    // ⚠ The two counts below are not the claim (the byte count has its own gate), so they are bound
    // rather than asserted — but bound, never dropped: `Written` is `#[must_use]` because a write
    // whose size nobody read is a run charging itself from a guess.
    let _cleared = remote
        .inject(
            pane,
            &[KeyStroke {
                key: "u".to_owned(),
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            }],
        )
        .expect("clear the composed line so the marker above is not submitted");
    let mut printing = KeyStroke::text("printf 'SAID%s\\n' MARK");
    printing.push(KeyStroke::named("Enter"));
    let _ran = remote
        .inject(pane, &printing)
        .expect("the driver runs a command whose OUTPUT carries a marker it never typed");

    let reached = printed_barrier.reached(&remote, pane, &run);
    assert_eq!(
        reached,
        Ok(Reached::Yes),
        "⚠⚠⚠⚠ THE CONTROL: a marker the SHELL printed must clear the barrier. Without this arm \
         the refusal above passes on a barrier that never clears at all — and an `input_echo` \
         hard-wired to answer the whole SCREEN would fail exactly here, because it would report \
         the program's own output as something somebody typed. The pane was showing:\n{}",
        remote.pane_collapsed(pane).unwrap_or_default(),
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER OUTSIDE THIS PROCESS READS THE KERNEL'S TWO ANSWERS ABOUT A PANE'S TERMINAL**
/// — register item 557's `terminal_modes` surface, and the two facts every screen-based confirmation
/// silently assumes.
///
/// # ⚠⚠⚠⚠⚠ Each answer has a MEASURED failure behind it, not a hypothesis
///
/// * **`echo`** decides what a read-back is WORTH. With the terminal echoing, the line discipline
///   paints every byte the instant it reaches the device — before the program has read one, and
///   whether or not it ever will. Measured: a delivery confirmed into a pane running `sleep 60`, in
///   20 ms, over a peer that never read a byte. A driver that cannot ask this reports *the peer took
///   it* on evidence about the TERMINAL.
/// * **`end_of_input`** decides whether a `Ctrl-D` will do anything at all. It is a CONDITION the
///   line discipline raises, and only in canonical mode; a program that took its terminal raw gets
///   `0x04` as an ordinary byte. Measured on `stty raw -echo; exec cat`: the run spent its whole
///   reply timeout, converged, published the peer's ECHO of the prompt as the model's answer, and
///   blamed *"the peer had not finished"* — a sentence about the peer's speed for a cause that was
///   the terminal's mode and knowable before the wait began.
///
/// # ⚠⚠⚠ The PAIR is the claim, on one host and one connection
///
/// A pane whose program leaves the discipline alone answers `ByTheTerminal` / `EndsTheInput`; a pane
/// that has taken its terminal RAW answers `ByTheProgram` / `IsJustAByte`. Either slot hard-wired to
/// one word passes for one of them and fails for the other, and both are asked in the same breath —
/// so an address answering about the HOST rather than about the pane it names is red too.
///
/// # ⚠⚠⚠⚠⚠ The cooked pane runs `cat`, NOT a shell, and CI is what taught this gate the difference
///
/// The first version used `sh`, on the assumption that *a shell echoes*. **It is not true of a
/// shell, it is true of `dash`.** Linux's `/bin/sh` is dash and leaves the discipline alone;
/// macOS's is bash, and an interactive readline shell **takes its terminal RAW at the prompt and
/// echoes the characters itself** — which is exactly what [`PaneEcho`]'s own doc says every
/// interactive agent does. So macOS answered `ByTheProgram` for the "cooked" pane and was RIGHT,
/// and this gate was asserting one platform's shell as a fact about terminals. `cat` touches
/// nothing, so the pty's default discipline stands on both.
#[test]
fn a_remote_driver_reads_who_echoes_and_whether_ctrl_d_ends_the_input() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);

    // CANONICAL: `cat` never reconfigures its terminal, so the pty's own default discipline is what
    // the address must report — echo on, canonical on, identically on every platform.
    let cooked = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));
    // RAW: the pane takes its own terminal off echo and out of canonical mode before `cat` runs —
    // which is what every full-screen agent does on startup, done here in one line.
    let raw = spawn_pane(
        &mut setup,
        json!({ "cmd": ["sh", "-c", "stty raw -echo; exec cat"] }),
    );

    let modes = remote.terminal_modes().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a remote driver reports that NO PANE on this daemon has a \
             terminal whose modes can be read. Every screen-based confirmation it makes is then an \
             assumption, and `deliver` must answer `echo: None` for a pane whose mode the daemon \
             can read perfectly well."
        )
    });

    assert!(
        wait_until(Duration::from_secs(10), || modes.pane_echo(cooked)
            == Some(PaneEcho::ByTheTerminal)),
        "⛔⛔⛔⛔ REGISTER ITEM 557: a pane whose program leaves the discipline alone ECHOES, and a \
         driver that cannot learn that will read its own keystroke coming back as the peer's \
         output — a delivery confirmed in 20 ms over a peer that never read a byte. Got {:?}",
        modes.pane_echo(cooked),
    );
    assert_eq!(
        modes.pane_end_of_input(cooked),
        Some(PaneEndOfInput::EndsTheInput),
        "the pty's default discipline is CANONICAL, so the EOF character becomes end-of-input and \
         a caller that ends its question with Ctrl-D is asking for something that will happen",
    );

    // ── AND THE RAW PANE, IN THE SAME BREATH ───────────────────────────────────────────────────
    assert!(
        wait_until(Duration::from_secs(10), || modes.pane_echo(raw)
            == Some(PaneEcho::ByTheProgram)),
        "⚠⚠⚠⚠ THE OTHER HALF OF THE PAIR: a pane whose program took its terminal off echo must say \
         so. Without this arm a slot hard-wired to `terminal` passes — and a driver told the \
         terminal echoes would DISCOUNT output the program actually printed. Got {:?}",
        modes.pane_echo(raw),
    );
    assert_eq!(
        modes.pane_end_of_input(raw),
        Some(PaneEndOfInput::IsJustAByte),
        "⛔⛔⛔⛔ REGISTER ITEM 557: on a RAW pane a `Ctrl-D` is an ordinary byte, and a run that \
         waited for an end-of-input it never caused spent its whole timeout and then blamed the \
         peer. This is the read that lets it know before the wait rather than after it",
    );

    // ⚠⚠ AND THE COOKED PANE IS STILL COOKED, asked after the raw one — without this, both
    // assertions above are satisfied by an address that answers about whichever pane was asked
    // LAST, which is the failure a per-pane slot exists to make impossible.
    assert_eq!(
        (modes.pane_echo(cooked), modes.pane_end_of_input(cooked)),
        (
            Some(PaneEcho::ByTheTerminal),
            Some(PaneEndOfInput::EndsTheInput)
        ),
        "the addresses must answer about the pane they NAME: one pane going raw has been read as \
         every pane going raw",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A REMOTE BARRIER KNOWS WHO HAS THE PANE, AND SAYS SO WHEN IT GIVES UP** — register
/// item 557's `foreground_job` surface, driven through its two production consumers at once.
///
/// # ⚠⚠⚠⚠⚠ The two things this one address decides
///
/// * **THE PREDICATE.** `ReadyWhen::Runs(name)` asks *is the thing I launched the thing that owns
///   my pane* — the one readiness kind that does not depend on what a program chose to print. A
///   driver without this address cannot use it at all: it is false for every pane, for ever.
/// * **THE DIAGNOSIS.** When a barrier gives up, `PaneError::NeverReady` carries what the pane was
///   doing INSTEAD. With no surface that is `PaneDoing::Unknown` — *this host has no view of the
///   process table at all* — which is a sentence about the HOST offered for a failure about the
///   PANE. The person reading it goes looking for a broken platform.
///
/// ⚠⚠ The two arms are asked of ONE pane in one breath, so a slot that answered a fixed leader
/// would clear the barrier AND name the same thing in the refusal, and only the pair separates
/// them: the barrier waits for `sleep` while `sh` holds the terminal, then the refusal must NAME
/// `sh`.
#[test]
fn a_remote_barrier_asks_who_holds_the_pane_and_names_it_when_it_gives_up() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh"] }));

    let jobs = remote.foreground_job().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a remote driver has no way to ask who owns a pane's \
             terminal. `ReadyWhen::Runs` is then false for every pane for ever, and every barrier \
             that gives up blames the platform."
        )
    });

    // ── THE PREDICATE: the shell really is what holds this pane ────────────────────────────────
    assert!(
        wait_until(Duration::from_secs(10), || jobs
            .pane_foreground_leader(pane)
            .is_some_and(|leader| leader.name.contains("sh"))),
        "⛔⛔⛔⛔ REGISTER ITEM 557: the pane's foreground leader must reach a remote driver. This \
         is the fact `ReadyWhen::Runs` decides on, and it is the only readiness kind that does not \
         depend on what the program chose to print. Got {:?}",
        jobs.pane_foreground_leader(pane),
    );

    let run = RunContext::uncancellable();
    let mut runs = Readiness::new(
        Some(ReadyWhen::Runs("sh".to_owned())),
        Some(Duration::from_secs(5)),
        None,
        Attended::NoOne,
    );
    assert_eq!(
        runs.reached(&remote, pane, &run),
        Ok(Reached::Yes),
        "⚠⚠⚠⚠ the barrier the address exists for must actually CLEAR over the wire — the read \
         above proves the JSON and this proves the consumer",
    );

    // ── THE DIAGNOSIS: a barrier that gives up must name what it saw instead ───────────────────
    // Waiting for a program nobody launched, so the refusal is guaranteed — and the pane is held by
    // a shell the whole time, which is the fact the refusal has to carry.
    let mut never = Readiness::new(
        Some(ReadyWhen::Runs("no-such-program".to_owned())),
        Some(Duration::from_millis(800)),
        None,
        Attended::NoOne,
    );
    let refused = never.reached(&remote, pane, &run);
    let Err(PaneError::NeverReady { instead, .. }) = refused else {
        panic!("the barrier for a program nobody launched must refuse: {refused:?}");
    };
    let named = instead.leader().map(|leader| leader.to_string());
    assert!(
        named.as_deref().is_some_and(|name| name.contains("sh")),
        "⛔⛔⛔⛔ REGISTER ITEM 557: the refusal must say WHAT THE PANE WAS DOING INSTEAD. Without \
         this address it reads `Unknown` — *this host has no view of the process table at all* — \
         which is a sentence about the HOST handed to somebody debugging a failure about the PANE, \
         and it sends them looking for a broken platform. A gate that cannot say what it saw \
         cannot be debugged. Got {instead:?}",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER OUTSIDE THIS PROCESS FOLLOWS A PANE'S OUTPUT, AND IS TOLD WHAT IT MISSED** —
/// register item 557's `output_lines` surface, and the last of the six.
///
/// # ⚠⚠⚠⚠⚠ Why no screen address can be a relay
///
/// `full_lines` answers *everything this pane has ever said*. A reader following a running program
/// would re-read the whole history every step and could not tell what is NEW — and worse, could not
/// tell what it had MISSED. This family answers *since a cursor*, and carries three facts a re-read
/// cannot reconstruct:
///
/// * **`lost`** — complete lines evicted before this reader asked. **A silent gap in a relay is
///   indistinguishable from a quiet source**, and a reader that cannot tell them apart reports the
///   peer said nothing when it said something nobody kept.
/// * **`partial`** — the line still being written, kept OUT of `lines` and NOT counted by `next`, so
///   a reader that ignores it loses nothing and one that takes it is handed the line again, whole.
/// * **`next`** — where to resume. It is the field whose ABSENCE is most dangerous, which is why the
///   client defaults it to the cursor it passed rather than to zero: a relay that rewound to the
///   beginning of the pane every step would re-deliver the peer's whole history as if it were new.
///
/// # ⚠⚠⚠ The claim is a WALK, not one read
///
/// One read proves the JSON. Two reads in sequence, with the second using the `next` the first
/// answered, prove the thing the address is FOR: the second must carry what the pane said between
/// them and must NOT carry what the first already delivered.
#[test]
fn a_remote_driver_follows_a_panes_output_from_a_cursor_and_does_not_re_read_it() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh"], "cols": 60, "rows": 12 }));

    let output = remote.output_lines().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: a remote driver has no way to follow a pane's output. It \
             then falls back to re-reading the whole history each step, which cannot say what is \
             new and cannot say what was lost."
        )
    });

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains('$'))),
        "the shell never printed a prompt, so the walk below would start mid-boot",
    );

    // ── SOMETHING COMPLETE BEFORE THE CURSOR, so the "must not come back" arm below has something
    // to be about. ⚠⚠⚠⚠ WITHOUT THIS THE ARM IS VACUOUS AND SAYS SO TO NOBODY: a shell's prompt
    // sits on the PARTIAL line, never in `lines`, so the first version of this gate skipped that
    // assertion entirely — and the mutation that makes the address ignore its cursor passed.
    let mut before = KeyStroke::text("printf 'ZERO\\n'");
    before.push(KeyStroke::named("Enter"));
    let _first_write = remote
        .inject(pane, &before)
        .expect("the driver asks the shell to print a line before the cursor is taken");
    assert!(
        wait_until(Duration::from_secs(10), || output
            .pane_lines_since(pane, 0)
            .is_some_and(|since| since.lines.iter().any(|line| line == "ZERO"))),
        "the pane never printed the line the cursor is taken after. Read {:?}",
        output.pane_lines_since(pane, 0),
    );

    // ── THE FIRST READ SETS THE CURSOR ─────────────────────────────────────────────────────────
    let first = output
        .pane_lines_since(pane, 0)
        .expect("the pane answers its lines from the beginning");
    assert!(
        first.lines.iter().any(|line| line == "ZERO"),
        "⚠⚠⚠⚠ THE FIXTURE'S PREMISE, ASSERTED: this read must actually HOLD the line the second \
         one is forbidden to repeat. It is stated because the first draft of this gate assumed a \
         shell prompt would be here — it is not, it is on the `partial` line — and the arm below \
         silently measured nothing. Got {:?}",
        first.lines,
    );
    assert_eq!(
        first.lost, 0,
        "⚠⚠ nothing can have been evicted from a pane this young — a non-zero `lost` here would \
         mean the client is inventing the field rather than reading it. Got {first:?}",
    );

    // ── THEN THE PANE SAYS SOMETHING, AND ONLY THAT MUST COME BACK ─────────────────────────────
    let mut says = KeyStroke::text("printf 'ONE\\nTWO\\n'");
    says.push(KeyStroke::named("Enter"));
    let _typed = remote
        .inject(pane, &says)
        .expect("the driver asks the shell to print two lines");

    let resumed = first.next;
    assert!(
        wait_until(Duration::from_secs(10), || output
            .pane_lines_since(pane, resumed)
            .is_some_and(|since| since.lines.iter().any(|line| line == "TWO"))),
        "⛔⛔⛔⛔ REGISTER ITEM 557: the pane's NEW output never reached the driver at the cursor \
         the previous read handed it. A relay that cannot resume is a relay that re-reads, and a \
         re-reading relay cannot tell what the peer just said from what it said an hour ago. \
         Read {:?}",
        output.pane_lines_since(pane, resumed),
    );

    let second = output
        .pane_lines_since(pane, resumed)
        .expect("the pane answers from the resumed cursor");
    assert!(
        second.next > resumed,
        "⚠⚠⚠ the cursor must ADVANCE, or the next step re-reads these lines for ever. Got {} from \
         {resumed}",
        second.next,
    );
    // ⚠⚠⚠⚠ THE NON-VACUITY OF THE WHOLE WALK, and it is an UNCONDITIONAL assertion because a
    // conditional one is how this arm was vacuous the first time. An address that ignored the
    // cursor and answered the pane's whole history satisfies every assertion above — it contains
    // `TWO`, after all — and fails only here.
    assert!(
        !second.lines.iter().any(|line| line == "ZERO"),
        "⛔⛔⛔⛔ REGISTER ITEM 557: a line the FIRST read already delivered came back in the \
         second. The cursor is then decorative and this address is `full_lines` wearing a \
         different name — which is exactly the re-read the family exists to replace, and a relay \
         built on it would report the peer's whole history as new on every step. Got {:?}",
        second.lines,
    );

    let _ = std::fs::remove_file(&sock);
}

/// The NAME the restart gate's run knows its pane by — the one address that survives a daemon.
const DRIVEN: &str = "inner-session";

/// A test-side connection to `sock` — the first one dies with the daemon it was made to, so a gate
/// that outlives a restart needs another.
fn setup_at(sock: &Path) -> HostConn {
    HostConn::connect(sock, Duration::from_secs(5))
        .expect("connect to the daemon that is there now")
}

/// Spawn a `sprag-term` on a socket path the CALLER names — what a restart needs, because the whole
/// point is that the second daemon takes the FIRST one's address.
///
/// ⚠⚠⚠⚠⚠ IT RETURNS THE GUARD, NOT A BARE `Child`, AND THAT IS NOT TIDINESS. [`HostChild`]'s own
/// doc says why in words: *"a panicking assertion never leaks a `sprag-term`"*. The first draft of
/// this helper returned the `Child`; the gate's first run panicked at its second assertion, both
/// daemons outlived the test binary, and the REMOTE build session they were holding open looked
/// like a seventeen-minute hang. The test had already finished in 0.04 s.
///
/// ⚠ It does not unlink the path first: the caller owns the ordering, and a restart gate needs the
/// old socket gone only after the old daemon is dead.
fn spawn_host_at(sock: &Path, program_and_args: &[&str]) -> HostChild {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sprag-term"));
    command
        .arg("--size")
        .arg("40x6")
        .arg("--")
        .args(program_and_args)
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null());
    let child = command
        .spawn()
        .expect("spawn a sprag-term at a named socket");
    HostChild(child, sock.to_path_buf())
}

/// ⛔⛔⛔⛔ **A RUN DRIVEN FROM ANOTHER PROCESS MUST SEE THE PERSON WHO REACHED INTO ITS PANE** —
/// register item 653, the `hands` surface, and the one absence on item 557's list that is NOT safe.
///
/// # ⚠⚠⚠⚠⚠ Why this absence is different from the eight beside it
///
/// Item 557 measured nine optional sub-surfaces missing from `RemotePaneAccess` and paid the six
/// the loop reads for its own work, recording that *"each absence is safe by that surface's OWN
/// documentation"*. That sentence is FALSE of this one, and the reason is where the read sits:
/// [`Readiness::reached`] asks *has a person reached in* **first, ahead of every other question**,
/// and it asks it through [`PaneAccess::hands`]. It is the barrier all three injecting plugins pass
/// through on their way to a keystroke — `ai_loop` included.
///
/// So a driver whose `hands()` answers `None` does not degrade: it concludes **nobody has ever
/// touched this pane**, for every pane, for the run's whole life. A person who reaches into a pane
/// an out-of-process run is driving gets typed over, and the run's ending carries no count of what
/// they did (register item 586's other half). The collapse is the one item 557 spent a round
/// preventing for `supervision` — *I did not look* wearing the words of *I looked and there was
/// nobody* — arriving one surface along.
///
/// # ⚠⚠⚠ THE PAIR IS THE CLAIM, and the control comes FIRST
///
/// The driver's own typing goes through the same pseudoterminal door a person's does, and the pane
/// tells them apart only because the write DECLARES which hand it is (`sprag_host::pane`: the
/// display client says `person`, the wire says `program`). So:
///
/// * **The control** — the driver types twice and the barrier CLEARS. An address that counted every
///   write would report the driver interrupting itself here, and a run that stopped for its own
///   keystroke would never take a single turn.
/// * **The claim** — a person types once, and the very next look is `Interrupted`, carrying `1`.
///
/// Either half alone is passed by a wrong address: a surface hard-wired to zero passes the control,
/// a surface counting all hands passes the claim. Both are asked of ONE pane on ONE connection.
#[test]
fn a_remote_barrier_sees_the_person_who_reached_in_and_not_the_driver_that_is_typing() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh"], "cols": 60, "rows": 12 }));

    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains('$'))),
        "the shell never printed a prompt, so nothing below is measuring a settled pane. Read {:?}",
        remote.pane_collapsed(pane),
    );

    // ── ARM. The FIRST `reached` is what takes both watermarks: the hands count this barrier will
    // measure against, and `Prints`'s baseline. Neither is a fact a later call can recover.
    let run = RunContext::uncancellable();
    let mut barrier = Readiness::new(
        Some(ReadyWhen::Prints("PROGRAMMARK".to_owned())),
        Some(Duration::from_secs(5)),
        None,
        Attended::NoOne,
    );
    let armed = barrier.reached(&remote, pane, &run);
    assert!(
        matches!(
            armed,
            Err(PaneError::NeverReady {
                already_showing: false,
                ..
            })
        ),
        "the barrier must not be down before anything ran, and its baseline must record that the \
         marker was NOT already showing — otherwise the control below is about a latch rather than \
         about a pane. Got {armed:?}",
    );

    // ── THE CONTROL, FIRST: THE DRIVER'S OWN WRITES ARE NOT AN INTERRUPTION ─────────────────────
    // Every one of these crosses the socket as `Hand::AProgram`, which is what the wire's write
    // door records for a caller that did not declare otherwise.
    let _cleared = remote
        .inject(
            pane,
            &[KeyStroke {
                key: "u".to_owned(),
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            }],
        )
        .expect("the driver clears the composed line");
    let mut printing = KeyStroke::text("printf 'PROGRAM%s\\n' MARK");
    printing.push(KeyStroke::named("Enter"));
    let _ran = remote
        .inject(pane, &printing)
        .expect("the driver runs a command whose output carries the barrier's marker");

    let cleared = barrier.reached(&remote, pane, &run);
    assert_eq!(
        cleared,
        Ok(Reached::Yes),
        "⚠⚠⚠⚠ THE CONTROL: the driver wrote into this pane twice and NONE of it is a person. A \
         `hands` address that counted every write would answer `Interrupted` here, and a run that \
         stopped for its own keystroke would never take one turn. The pane was showing:\n{}",
        remote.pane_collapsed(pane).unwrap_or_default(),
    );

    // ── THE CLAIM: ONE WRITE BY A PERSON, DECLARED AS ONE ───────────────────────────────────────
    setup
        .call(
            "scene/invoke",
            json!({
                "path": pane_input_path(pane.0, TEXT_ACTION),
                "args": { "text": "PERSONHERE", "hand": "person" },
            }),
        )
        .expect("a display client puts a person's keystrokes into the pane");
    assert!(
        wait_until(Duration::from_secs(5), || remote
            .pane_collapsed(pane)
            .is_some_and(|screen| screen.contains("PERSONHERE"))),
        "⚠⚠⚠ THE NON-VACUITY: the person's write must really have reached the pane, or the verdict \
         below is about a write that never happened. Read {:?}",
        remote.pane_collapsed(pane),
    );

    let verdict = barrier.reached(&remote, pane, &run);
    assert_eq!(
        verdict,
        Ok(Reached::Interrupted(Interruption::of(1))),
        "⛔⛔⛔⛔ REGISTER ITEM 653: a run driven from another process asked *has a person reached \
         into this pane* and was told no. That is not a degradation — `Readiness::reached` asks \
         this FIRST, ahead of every other question, so the answer stands for the run's whole life \
         and the run types over whoever is there. The pane was showing:\n{}",
        remote.pane_collapsed(pane).unwrap_or_default(),
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A DRIVER DOES NOT GO ON TYPING INTO A DAEMON THAT IS NOT THE ONE IT ADOPTED** —
/// register item 544, stage 1d, and the property every later stage stands on.
///
/// # ⚠⚠⚠⚠⚠ A socket is an ADDRESS, not an identity
///
/// When a daemon dies and another takes the same path, a client that merely redialled would carry
/// its pane ids across — and **pane ids are minted from a counter that starts at zero**, so a fresh
/// daemon's own boot pane IS pane 0. A driver holding pane 0 would type its run's stimulus into a
/// stranger's shell and be told it succeeded every time. Nothing on this wire could tell the two
/// apart before stage 1d: `build` names the BINARY, and two daemons started from one binary share
/// it.
///
/// # ⚠⚠⚠ What the gate holds, and why the third arm is the one that matters
///
/// * The surface really was driving before the restart — otherwise the refusal after it proves
///   nothing.
/// * After the restart it REFUSES: reads answer `None` (*I cannot see that pane* — what every
///   consumer of this trait already stops on) and the write door refuses in words.
/// * ⚠⚠⚠⚠ And the run is NOT lost: `readopt` — the driver saying *I have looked, and it is mine* —
///   puts it back to work against the daemon that is there now. A surface that could only latch
///   would have traded a silent corruption for a permanent stop.
#[test]
fn a_driver_stops_when_the_daemon_under_it_is_replaced_and_goes_again_when_told_to() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let first = spawn_host_at(&sock, &["cat"]);
    let (remote, mut setup) = remote_driver(&sock);

    // ⚠⚠⚠⚠⚠ THE BOOT PANE, and it is the fixture's whole force. Both daemons mint their ids from a
    // counter that starts at zero, so this id exists on the REPLACEMENT too — running the same
    // `cat`, owned by nobody this driver ever met. A gate that drove a pane it SPAWNED would be
    // refused by the new daemon for the wrong reason (it never minted that id), and the danger the
    // latch exists for would be described in prose and demonstrated by nothing.
    let pane = *remote
        .pane_ids()
        .first()
        .expect("the daemon's boot pane is there to drive");
    // ...and it is NAMED, because a name is what a run re-adopts by. A restore brings the name
    // back; the id is whatever the new daemon's counter says.
    setup
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(RENAME_PANE_ACTION),
                "args": { "pane": pane.0, "name": DRIVEN },
            }),
        )
        .expect("name the pane this run drives");

    // ── IT IS DRIVING, and the surface has adopted a daemon ────────────────────────────────────
    assert_eq!(
        remote
            .inject(pane, &KeyStroke::text("live"))
            .map(Written::bytes),
        Ok(4),
        "⚠⚠ THE CONTROL: the driver must really be driving before the restart, or the refusal \
         below is about a surface that never worked",
    );
    let adopted = remote.adopted_instance();
    assert!(
        adopted.is_some(),
        "⛔⛔⛔⛔ REGISTER ITEM 544 stage 1d: the surface learned no daemon identity, so it has \
         nothing to compare a redial against and cannot tell a restored world from a stranger's",
    );
    assert!(
        !remote.world_changed(),
        "nothing has been replaced yet, so a latch here would refuse a daemon that never moved",
    );

    // ── THE DAEMON IS REPLACED AT THE SAME ADDRESS ─────────────────────────────────────────────
    // Dropping the guard kills it AND unlinks the socket, which is the ordering a replacement
    // needs: the address must be free before the next daemon binds it.
    drop(first);
    let _second = spawn_host_at(&sock, &["cat"]);
    // The new daemon has to be accepting before the driver's next read, or the read fails for
    // "nobody is listening" rather than for the reason this gate is about.
    let reachable = wait_until(Duration::from_secs(10), || {
        HostConn::connect(&sock, Duration::from_millis(200)).is_ok()
    });
    assert!(reachable, "the replacement daemon never bound {sock:?}");
    // ⚠⚠⚠ THE REPLACEMENT CARRIES THE NAME, which is what a daemon RESTORING its snapshot does —
    // panes come back under the addresses a person gave them. Done here explicitly rather than
    // through the durability ring so the gate is about the DRIVER's re-adoption and not about
    // whether a snapshot happened to be written in the second the test had.
    let mut fresh = setup_at(&sock);
    let born = *pane_ids(&mut fresh)
        .first()
        .expect("the replacement daemon has a boot pane of its own");
    fresh
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(RENAME_PANE_ACTION),
                "args": { "pane": born, "name": DRIVEN },
            }),
        )
        .expect("the restored world carries the name the run knows");

    // ── THE DRIVER NOTICES, AND STOPS ──────────────────────────────────────────────────────────
    assert!(
        wait_until(Duration::from_secs(10), || {
            let _ = remote.pane_collapsed(pane);
            remote.world_changed()
        }),
        "⛔⛔⛔⛔ REGISTER ITEM 544 stage 1d: the driver's surface reconnected to a DIFFERENT daemon \
         and said nothing. Its pane ids now name whatever the new daemon minted — a fresh daemon's \
         boot pane is pane 0 — so the next injection is a run's stimulus typed into a stranger's \
         shell, reported as a success.",
    );
    assert_eq!(
        remote.pane_collapsed(pane),
        None,
        "⚠⚠⚠ a surface whose world changed must see NOTHING: `None` is *I cannot see that pane*, \
         which is the answer every consumer of this trait already stops on",
    );
    // ⚠⚠⚠⚠⚠ AND THE NEW DAEMON REALLY DOES HOLD THIS ID — asserted, so the refusal below is
    // measured against a write that WOULD have landed. Without the latch this injection succeeds:
    // the run's stimulus goes into a `cat` nobody here started, and the door reports how many bytes
    // it wrote.
    let stranger = HostConn::connect(&sock, Duration::from_secs(5))
        .ok()
        .and_then(|mut fresh| {
            fresh
                .call(
                    "scene/query",
                    json!({ "path": mux_action_path(PANES_SLOT) }),
                )
                .ok()
        })
        .map(|panes| {
            panes
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry[PANE_SUMMARY_ID_KEY].as_u64())
                .any(|id| id == pane.0)
        });
    assert_eq!(
        stranger,
        Some(true),
        "⚠⚠⚠⚠ THE FIXTURE'S PREMISE: the replacement daemon must ALSO hold this pane id, or the \
         refusal below is about a write that would have failed anyway and the gate demonstrates \
         nothing. Both daemons mint from a counter that starts at zero, so the boot pane is the id \
         they share",
    );
    let refused = remote.inject(pane, &KeyStroke::text("no"));
    assert!(
        matches!(refused, Err(PaneError::Write(_))),
        "⛔⛔⛔⛔ REGISTER ITEM 544 stage 1d: the write door must REFUSE. *I cannot see it* and *I \
         typed into something* are not interchangeable when the something may be a stranger's \
         shell — and the assertion above has just established that this id NAMES one. Got \
         {refused:?}",
    );

    // ── AND THE DRIVER RE-ADOPTS BY NAME, WHICH IS THE ONLY ADDRESS THAT SURVIVED ──────────────
    // ⚠⚠⚠⚠⚠ THE ID IS NOT RE-USED. A driver that carried its pane NUMBER across would be doing the
    // very thing the latch just stopped. It asks the new daemon for the pane it CALLED something —
    // the address a person gave and a restore brings back — and only then says the world is its
    // own. That order is the whole re-adoption: look, recognise, then drive.
    let renamed = remote
        .pane_named(DRIVEN)
        .expect("the replacement daemon holds the pane this run was driving, by its name");
    remote.readopt();
    assert!(
        !remote.world_changed(),
        "⚠⚠⚠⚠ a latch that could not be cleared would trade a silent corruption for a permanent \
         stop — the driver has looked and said the world is its own",
    );
    assert_eq!(
        remote
            .inject(renamed, &KeyStroke::text("again"))
            .map(Written::bytes),
        Ok(5),
        "⚠⚠⚠ after re-adopting, the surface drives the pane it recognised on the daemon that is \
         there now — the WORK continues across a restart, which is what item 544 is for",
    );
    assert!(
        wait_until(Duration::from_secs(10), || remote
            .pane_collapsed(renamed)
            .is_some_and(|screen| screen.contains("again"))),
        "⚠⚠ and it really reached that pane: the pty echoed it back. Read {:?}",
        remote.pane_collapsed(renamed),
    );
}

/// ⛔⛔⛔⛔ **THE LOOP'S OWN PLUGIN IS BUILDABLE BY THE OUT-OF-PROCESS DRIVER, THROUGH THE ONE
/// BUILDER AND OVER THE REMOTE WORLD** — register item 557's second clause, at the plugin the item
/// is actually about.
///
/// # ⚠⚠⚠⚠⚠ Why the restart gate below does not already cover this
///
/// The gate under this one drives an `Orchestrator`, which is what stage 1 settled. But item 557's
/// own text is about `ai_loop`: *"`outer.rs` and `ai_loop.rs` consult supervision, input_echo,
/// foreground_job, output_lines, terminal_modes and lifecycle on paths a real run takes."* Between
/// the request and any of those reads there is a step nothing held — **the loop's plugin has to be
/// CONSTRUCTIBLE from the request, by the builder the remote driver calls, against a world whose
/// every answer comes off a socket.** `plugin_from_request`'s `ai_loop` arm builds a Lua engine,
/// resolves a loop KIND from this repository's own document and asks the world for two facts. A
/// change there that reached for anything only the daemon holds would break the out-of-process
/// driver and no gate would say so: `drive_request` is the door, and its `ai_loop` arm had no
/// caller in any test that used a REMOTE world.
///
/// # ⚠⚠⚠ The pair is the claim, because «it built» alone is cheap
///
/// A builder that ignored the world entirely would also build. So the second half asks for a pane
/// the daemon does not hold and requires a REFUSAL — which can only come from the world, and this
/// world can only answer it over the socket.
#[test]
fn the_loops_own_plugin_is_built_from_a_request_over_a_world_that_is_a_socket() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    let (remote, _setup) = remote_driver(&sock);
    let pane = *remote
        .pane_ids()
        .first()
        .expect("the daemon's boot pane is there to drive");
    let world = RemotePluginWorld::over(&remote);

    // ⚠ THE REQUEST A CALLER REALLY SENDS — the `agent` key is required, and the barrier is derived
    // from it because a loop's first prompt goes into a pane whose program may still be starting.
    // ⚠⚠ The ceiling is ONE ITERATION, because this gate is about the door and not about a
    // conversation: the run must reach a terminal state on its own rather than supervise a shell
    // that will never answer it.
    // ⚠⚠ THE KEYS ARE THE FORM'S OWN (`PluginGrammar::AI_LOOP_FORM`), not a guess: a loop is
    // steered by a north star, a milestone and a reference, and the first draft of this gate sent
    // `{plugin, pane, agent}` and was refused `TypeMismatch` — the grammar refusing an incomplete
    // request, which is the door working.
    let request = json!({
        "plugin": "ai_loop",
        "pane": pane.0,
        "agent": "claude",
        "north_star": "the door is what this gate is about",
        "milestone": "be built and stepped from outside the daemon",
        "reference": "the surfaces are served; the construction was not held",
        // ⚠⚠ BOUNDED AT THE BARRIER, not only at the ceiling. `max_iterations` decides between
        // turns, and the pane here holds a SHELL — so the loop's first readiness wait is what the
        // whole run would otherwise spend, and it did: 200 seconds on the first draft of this gate.
        "ready_timeout_ms": 300,
        "guardrails": { "max_iterations": 1 },
    })
    .as_object()
    .expect("a request is an object")
    .clone();

    // ── THE CLAIM: the loop's plugin is built AND stepped, out here ───────────────────────────
    let context = RunContext::uncancellable();
    let quiet: sprag_plugin::ProgressSink = std::sync::Arc::new(|_: &sprag_plugin::Progress| {});
    let driven = drive_request(
        &world,
        &request,
        &remote,
        &context,
        std::sync::Arc::clone(&quiet),
    );
    let driven = driven.unwrap_or_else(|why| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 557: the out-of-process driver cannot BUILD the loop it exists \
             to drive. Every surface the loop reads is served over this wire now, and none of that \
             is reachable if the plugin cannot be constructed from the request in the process that \
             will step it — this arm refuses BEFORE a byte is typed, so a failure here is the door \
             and not the conversation. Refused with {why:?}"
        )
    });
    // ⚠⚠⚠⚠⚠ WHICH ENDING IT REACHED IS DELIBERATELY NOT ASSERTED, and saying so is the honest
    // report. This pane holds a SHELL, so the loop correctly gets no agent to talk to and ends
    // `Failed` — measured. Pinning that word would make this gate about the PEER, and the day
    // somebody points it at a live agent the ending changes and a gate about the DOOR would go red
    // for a reason that has nothing to do with the door. What is asserted is that the door opened:
    // `drive_request` answers a terminal state instead of refusing to build.
    let _ = &driven.outcome.state;

    // ── THE CONTROL: the world is really consulted, and it is really this socket ──────────────
    // ⚠ THE SAME REQUEST WITH ONE FIELD MOVED, so the refusal below can only be about the PANE —
    // a differently-shaped request would be refused by the grammar and prove nothing about a world.
    let mut missing = request.clone();
    missing.insert("pane".to_owned(), json!(9_999_999));
    assert!(
        drive_request(&world, &missing, &remote, &context, quiet).is_err(),
        "⚠⚠⚠ THE CONTROL: a pane this daemon does not hold must be refused BEFORE anything is \
         typed, and only the world can know that. An accepted build would mean the arm above \
         proves construction and nothing about the socket — the same green a builder that never \
         asked the world would give.",
    );
}

/// ⛔⛔⛔⛔⛔ **A SAVED PLACE THIS BUILD CANNOT READ STOPS THE DRIVER BEFORE A BYTE IS TYPED** —
/// register item 543's fourth brick, at the door a restarted daemon puts an inherited run through.
///
/// # ⚠⚠⚠⚠⚠ Why the refusal is the claim here, and the resume is claimed next door
///
/// Between a run log and `Plugin::resume_at` there is exactly one door — `drive_request`, in the
/// process that will step the plugin — and until it READS the place, every word item 543 has
/// persisted is written and never read (register item 492's shape, in the feature whose whole
/// subject is a run surviving a restart). This gate is what says it reads it.
///
/// **What it does NOT say**, written down rather than implied: that a place this build CAN read
/// puts the machine there. That needs a loop that takes real steps, which needs a peer that
/// answers, and the pane here holds a shell — so it is measured where the stand-in agent lives:
/// `sprag_plugin`'s `a_resumed_loop_is_placed_rather_than_walked_and_does_not_re_open_with_its_prompt`.
/// The two together are the chain; neither alone is.
///
/// # ⚠⚠⚠⚠ The pair is the claim, because a refusal alone proves nothing
///
/// The SAME request without the key must be driven. Only then can the refusal below be about the
/// place rather than about anything else in a loop request — which is the neighbouring gate's own
/// rule (*one field moved*) and the reason a door that ignored the key entirely is red here: it
/// would answer `Ok` to both.
#[test]
fn a_saved_place_this_build_cannot_read_stops_the_driver_before_a_byte_is_typed() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    let (remote, _setup) = remote_driver(&sock);
    let pane = *remote
        .pane_ids()
        .first()
        .expect("the daemon's boot pane is there to drive");
    let world = RemotePluginWorld::over(&remote);
    let context = RunContext::uncancellable();

    // ⚠ The neighbouring gate's request verbatim in shape: one iteration, because what is measured
    // here happens BEFORE the first step either way.
    let request = json!({
        "plugin": "ai_loop",
        "pane": pane.0,
        "agent": "claude",
        "north_star": "a run outlives the daemon that started it",
        "milestone": "be put back where the log said, without re-typing",
        "reference": "register item 543",
        "ready_timeout_ms": 300,
        "guardrails": { "max_iterations": 1 },
    })
    .as_object()
    .expect("a request is an object")
    .clone();
    let quiet: sprag_plugin::ProgressSink = std::sync::Arc::new(|_: &sprag_plugin::Progress| {});

    // ── THE PREMISE: with no place on it, this request is DRIVEN ─────────────────────────────
    assert!(
        drive_request(
            &world,
            &request,
            &remote,
            &context,
            std::sync::Arc::clone(&quiet)
        )
        .is_ok(),
        "⚠⚠ THE PREMISE FAILED: this request is refused with no place on it at all, so the \
         refusal below would be about anything in it EXCEPT the one key this gate is measuring.",
    );

    // ── THE CLAIM: one key added, and the door shuts ──────────────────────────────────────────
    //
    // ⚠⚠⚠⚠ THE FORGERY IS SHAPED LIKE A REAL RECORD — several words, the first of which no
    // document in this build spells — for the reason item 543's own round measured twice: a single
    // junk word is refused for the WRONG reason (a head that is not among the states it names), so
    // it stays green against a reader that has stopped checking names at all.
    let mut forged = request.clone();
    forged.insert(
        sprag_host::plugins::RUN_PLACE_KEY.to_owned(),
        json!([
            "a-state-no-document-in-this-build-has",
            "nor-this-one",
            "nor-this"
        ]),
    );
    let refused = drive_request(&world, &forged, &remote, &context, quiet);
    assert!(
        refused.is_err(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a place this build cannot read was SWALLOWED, and the run \
         carried on from the top. That is not a fact quietly dropped — starting at the top fires \
         `<onentry>` all the way down, so the loop re-types its opening prompt into somebody's \
         pane and spends the peer's tokens saying what was already said. A door that never reads \
         the key answers `Ok` here exactly as it did above, which is what makes the pair the claim.",
    );
}

/// ⛔⛔⛔⛔ **A REAL RUN, DRIVEN BY THE REAL `Driver` FROM ANOTHER PROCESS, AND IT OUTLIVES THE
/// DAEMON IT DRIVES** — register item 544, stage 1's own claim.
///
/// # ⚠⚠⚠⚠⚠ What this is a gate for, in the item's own words
///
/// *"Two things with different natural lifetimes share one process."* The multiplexer owns
/// pseudoterminals for weeks; a run supervises for hours. Because the driver was compiled into the
/// daemon, *"change how a loop reflects"* meant *"restart the thing holding your PTYs"*. Nothing
/// about that is settled by a client that answers the right JSON — it is settled by **the shipped
/// `Driver`, stepping a shipped plugin, over a `PaneAccess` whose answers come off a socket.**
///
/// # ⚠⚠⚠⚠⚠ AND «THE RUN CONTINUES» IS THE WRONG BAR — the item's «done when» was wrong
///
/// A run CANNOT cross a restart and should not. `Readiness` latches facts about ONE pane (a marker
/// count taken on a screen that no longer exists, a hands watermark, a `seen` flag), and the
/// daemon's own restore marks every inherited run `INTERRUPTED` *because the process that would
/// have read it died*. What must survive is **THE WORK**: run one ends, the DRIVER re-adopts by
/// name, and run two takes the pane — which is the same ruling the item already makes about a
/// changed document (*"a changed document is a NEW run, deliberately"*).
///
/// So this gate asserts the three things that are actually the claim:
///
/// * **Run one converges** against a real pane over the wire — the shipped Driver, the shipped
///   `Orchestrator`, no in-process access anywhere.
/// * **The daemon is replaced, and the driver survives it** — the process running the Driver is
///   still there, and its surface says the world changed rather than typing into a stranger.
/// * **Run two converges too**, on the pane re-adopted BY NAME. The driver outlived its host.
#[test]
fn a_real_run_driven_from_another_process_outlives_the_daemon_it_drives() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let first = spawn_host_at(&sock, &["sh"]);
    let (remote, mut setup) = remote_driver(&sock);
    let pane = *remote
        .pane_ids()
        .first()
        .expect("the daemon's boot pane is there to drive");
    setup
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(RENAME_PANE_ACTION),
                "args": { "pane": pane.0, "name": DRIVEN },
            }),
        )
        .expect("name the pane this run drives");

    // The stimulus is arithmetic the SHELL performs, so the sentinel cannot appear from an echo —
    // `deliver`'s own discriminator, reused because it is the only one a screen cannot fake.
    let spec = || OrchestrationSpec {
        stimulus: "echo run-$((6*7))".to_owned(),
        sentinel: Some("run-42".to_owned()),
        ready_when: None,
        ready_within: None,
        may_answer: None,
        attended: Attended::NoOne,
        turn: None,
    };
    let rails = Guardrails {
        max_iterations: 4,
        max_cost: None,
        max_duration: Some(Duration::from_secs(20)),
    };

    // ── RUN ONE: the shipped Driver, over the socket ───────────────────────────────────────────
    let mut first_run = Orchestrator::new(pane, spec());
    let outcome = Driver::new(rails).run(&mut first_run, &remote, &RunContext::uncancellable());
    assert_eq!(
        outcome.state,
        OutcomeState::Converged,
        "⛔⛔⛔⛔ REGISTER ITEM 544: a real run driven from ANOTHER PROCESS did not converge \
         against a real pane. This is the claim the whole item is for — the shipped `Driver` \
         stepping a shipped plugin over a `PaneAccess` whose answers come off a socket — and \
         nothing about a client that returns the right JSON settles it. Got {outcome:?}",
    );

    // ── THE DAEMON IS REPLACED UNDER THE DRIVER ────────────────────────────────────────────────
    drop(first);
    // ⚠⚠⚠⚠⚠ THE REPLACEMENT'S BOOT PANE RUNS `cat`, NOT `sh`, AND THAT IS THE FIXTURE'S WHOLE
    // FORCE. A driver that carried its old pane NUMBER across would land on it — `cat` echoes the
    // stimulus and performs no arithmetic, so the sentinel never appears and run two cannot
    // converge. The pane the run means is a SHELL spawned after it, under a DIFFERENT id, carrying
    // the NAME. ⚠ The first version of this gate gave both daemons a `sh` boot pane, so the old id
    // and the re-adopted one were the same number and pointed at equivalent programs: the
    // re-adoption was decorative and the mutation that removed it passed.
    let _second = spawn_host_at(&sock, &["cat"]);
    assert!(
        wait_until(Duration::from_secs(10), || HostConn::connect(
            &sock,
            Duration::from_millis(200)
        )
        .is_ok()),
        "the replacement daemon never bound {sock:?}",
    );
    let mut fresh = setup_at(&sock);
    let born = spawn_pane(&mut fresh, json!({ "cmd": ["sh"], "name": DRIVEN }));
    assert_ne!(
        born, pane,
        "⚠⚠⚠⚠ THE FIXTURE'S PREMISE: the pane carrying the name on the NEW daemon must have a \
         different id from the one this run started with, or re-adopting by name is the same \
         answer as carrying the number and this gate proves nothing about either",
    );

    assert!(
        wait_until(Duration::from_secs(10), || {
            let _ = remote.pane_collapsed(pane);
            remote.world_changed()
        }),
        "⛔⛔⛔⛔ REGISTER ITEM 544: the driver did not notice its daemon being replaced, so its \
         next run would type into whatever the new daemon minted under the same id",
    );

    // ── RUN TWO: re-adopted BY NAME, on the daemon that is there now ───────────────────────────
    let readopted = remote
        .pane_named(DRIVEN)
        .expect("the replacement world carries the pane this driver knows, by name");
    remote.readopt();
    let mut second_run = Orchestrator::new(readopted, spec());
    let again = Driver::new(rails).run(&mut second_run, &remote, &RunContext::uncancellable());
    assert_eq!(
        again.state,
        OutcomeState::Converged,
        "⛔⛔⛔⛔ REGISTER ITEM 544, AND THE WHOLE POINT OF UNFUSING: the DRIVER outlived the daemon \
         it was driving. Run one converged, the host was replaced underneath it, and run two \
         converged on the pane this process re-adopted by NAME — without the driver's own process \
         ever restarting. That is what «the multiplexer and the supervisor have different \
         lifetimes» means when it is true rather than asserted. Got {again:?}",
    );
}

/// **THE JOB THAT OWNS `pane`'s TERMINAL, read on a connection the driver has never touched.**
///
/// # ⚠⚠⚠⚠ Why the instrument is deliberately NOT the surface under test
///
/// The two gates below judge whether a stop LANDED. Reading that back through the same
/// `RemotePaneAccess` that sent it would fold two failures into one look — a driver that never sent
/// the stop and a driver whose read is broken produce the same answer — and this workspace has paid
/// for that shape twice (register items 617 and 637). So this asks the daemon directly, over
/// `pane_processes`, which is the OPERATING SYSTEM's answer and not a guess from a pane's text.
///
/// ⚠⚠ The GROUP and not the leader, because a shell's job control is not a constant: an interactive
/// shell that runs `sleep 300` may put it in its own process group or keep it in the shell's, and
/// *which* decides whether the group's leader is `sleep` or the shell. The MEMBERS answer the
/// question either way — *is the work still there* — which is the only thing these gates ask.
fn foreground_job_over_the_wire(
    conn: &mut HostConn,
    pane: PaneId,
) -> Option<sprag_terminal::ForegroundJob> {
    let answer = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&pane_processes_at(0)) }),
        )
        .ok()?;
    let reading: PaneProcessesWire = serde_json::from_value(answer).ok()?;
    reading
        .panes
        .into_iter()
        .find(|row| row.id == pane.0)?
        .foreground
}

/// Whether `pane`'s foreground job still holds a process with `pid` — *is the work this run started
/// still running?*, asked of the process table.
fn job_still_holds(conn: &mut HostConn, pane: PaneId, pid: u32) -> bool {
    foreground_job_over_the_wire(conn, pane)
        .is_some_and(|job| job.processes.iter().any(|process| process.pid == pid))
}

/// ⛔⛔⛔⛔ **A RUN CANCELLED FROM OUTSIDE THE DAEMON REALLY ENDS THE TURN IT STARTED** — register
/// item 654, and the claim `Stopped::Unsupported` stood in for.
///
/// # ⚠⚠⚠⚠⚠ The defect, which was an HONEST sentence said by only one of two drivers
///
/// `Driver::stop_the_work` runs on exactly two endings — a person's cancel and a passed deadline —
/// and asks `PaneAccess::job_control`. `RemotePaneAccess` answered `None`, so **every run driven
/// from another process reported `Stopped::Unsupported`, whatever it was driving**. Nothing about
/// that word is false: a host with no job control must say it could not stop the work rather than
/// write `0x03` and hope, which is why this absence survived three rounds of item 557's list where
/// item 653's did not. What was false was the SITUATION — the same `orchestrate` request ended a
/// peer's turn in-process and left it running out of process, and `RUN_DRIVER_PROCESS`'s contract
/// is that a request means one thing on both sides.
///
/// # ⚠⚠⚠ What is measured, and in which order
///
/// * **THE CONTROL FIRST** (register item 648). A run over this same surface that ENDS ON ITS OWN
///   TERMS reaches for nothing: `stopped` is `None` and the pane's shell is left alone. Without it,
///   a surface that signalled a pane on every run end would pass the gate below and look like a
///   working cancel.
/// * **THE PREMISE, ASSERTED** (register item 25's rule: a fixture states what it built). The run's
///   stimulus starts a `sleep` and this gate does not proceed until the process table SHOWS it in
///   the pane's foreground job — so a cancel that lands before there is any work to stop cannot be
///   read as a cancel that stopped work.
/// * **THE STOP ITSELF**, as the run publishes it, and
/// * **THE WORK BEING GONE**, read off the process table afterwards — the half no outcome word can
///   prove. A `Stopped::Job` says a signal was delivered; only the pid disappearing says the turn
///   ended.
/// * **AND THE PANE STILL STANDING.** Ending a turn and ending somebody's pane are different acts,
///   and the sibling gate below is what makes that distinction load-bearing.
#[test]
fn a_cancelled_run_driven_from_another_process_really_ends_the_turn_it_started() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    let (remote, mut setup) = remote_driver(&sock);
    let pane = *remote
        .pane_ids()
        .first()
        .expect("the daemon's boot pane is there to drive");

    // ── THE CONTROL: a run that ends on its own terms reaches for nothing ──────────────────────
    let mut converging = Orchestrator::new(
        pane,
        OrchestrationSpec {
            // The arithmetic is the SHELL's, so the sentinel cannot appear from an echo — the
            // discriminator the gate above this one uses, and the only one a screen cannot fake.
            stimulus: "echo run-$((6*7))".to_owned(),
            sentinel: Some("run-42".to_owned()),
            ready_when: None,
            ready_within: None,
            may_answer: None,
            attended: Attended::NoOne,
            turn: None,
        },
    );
    let control = Driver::new(Guardrails {
        max_iterations: 4,
        max_cost: None,
        max_duration: Some(Duration::from_secs(20)),
    })
    .run(&mut converging, &remote, &RunContext::uncancellable());
    assert_eq!(
        control.state,
        OutcomeState::Converged,
        "the control run has to CONVERGE, or the gate below is measuring a surface that cannot \
         drive this pane at all rather than one that can stop it. Got {control:?}",
    );
    assert_eq!(
        control.stopped, None,
        "⚠⚠⚠ a run that ended on its OWN terms must not have reached for its peer's job — a \
         surface that signalled on every ending would make the cancel below indistinguishable from \
         no cancel at all. Got {:?}",
        control.stopped,
    );

    // ── THE RUN THAT GETS CANCELLED MID-TURN ──────────────────────────────────────────────────
    let mut working = Orchestrator::new(
        pane,
        OrchestrationSpec {
            stimulus: "sleep 300".to_owned(),
            // ⚠ NEVER PRINTED, so the step is still parked when the cancel lands. It is also not a
            // word the shell could echo into being: the stimulus does not contain it.
            sentinel: Some("this-turn-never-finishes".to_owned()),
            ready_when: None,
            ready_within: None,
            may_answer: None,
            attended: Attended::NoOne,
            // ⚠⚠ A CONTRACT WITH NO BOUND OF ITS OWN, so `patience()` is `Duration::MAX` and this is
            // ONE step parked until the run ends. Without it each step waits `OBSERVE_TIMEOUT` and
            // then TYPES THE STIMULUS AGAIN — a second `sleep 300` queued at a shell that is
            // already sleeping, which is a fixture nobody can reason about.
            turn: sprag_plugin::Turn::lasting(sprag_plugin::DoneWhen::Exits, None),
        },
    );

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let run = RunContext::new(std::sync::Arc::clone(&cancel));
    let watcher = {
        let cancel = std::sync::Arc::clone(&cancel);
        let sock = sock.clone();
        std::thread::spawn(move || {
            // ⚠ ITS OWN CONNECTION. The driver holds one and the test holds another; a watcher
            // sharing either would be measuring a wire it is also occupying.
            let mut watching = setup_at(&sock);
            let armed = Instant::now();
            let mut working_pid = None;
            while armed.elapsed() < Duration::from_secs(20) {
                if let Some(job) = foreground_job_over_the_wire(&mut watching, pane)
                    && let Some(process) =
                        job.processes.iter().find(|process| process.name == "sleep")
                {
                    working_pid = Some(process.pid);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // ⚠⚠ RAISED EVEN IF THE PREMISE NEVER HELD, so a fixture that failed to start any work
            // fails this gate with a sentence instead of hanging it until the run's own deadline.
            cancel.store(true, std::sync::atomic::Ordering::Release);
            working_pid
        })
    };

    let outcome = Driver::new(Guardrails {
        // Out of reach on purpose: the cancel must be the only ending available, or this gate
        // measures a ceiling and reports it as a stop.
        max_iterations: u32::MAX,
        max_cost: None,
        max_duration: Some(Duration::from_secs(120)),
    })
    .run(&mut working, &remote, &run);
    let working_pid = watcher.join().expect("the watcher thread").expect(
        "⚠⚠⚠ THE FIXTURE'S PREMISE: the run's stimulus never put a `sleep` in the pane's \
             foreground job, so there was no turn to stop and nothing below is about this product",
    );

    assert_eq!(
        outcome.state,
        OutcomeState::Cancelled,
        "a person's stop is the run's ending: {outcome:?}",
    );
    assert!(
        outcome.stopped.is_some(),
        "⛔⛔⛔⛔ REGISTER ITEM 654: the run driven from another process ended without REACHING for \
         the work it started — its door closing on a room its peer is still working in. Got {:?}",
        outcome.stopped,
    );
    // ⚠⚠ The exact word is asserted only where the platform can answer the question the narrow
    // reach asks — `signal_ends` reads a disposition off `/proc`, and a host without one refuses
    // rather than guessing. Same `cfg!` the in-process gate for this path carries, and for its
    // reason: a macOS runner reporting an absent CAPABILITY as a failure of the DRIVER is a red
    // that costs a day and says nothing.
    if cfg!(target_os = "linux") {
        match &outcome.stopped {
            Some(sprag_plugin::Stopped::Job(signalled)) => {
                assert!(
                    signalled.pgid != 0,
                    "⚠ a stop that named no process group is a report a person cannot verify: \
                     {signalled:?}",
                );
                assert!(
                    signalled.leader.is_some(),
                    "⚠ and it must say WHAT it reached — the group's leader was readable here, so \
                     an absence is this client dropping the name rather than the daemon lacking \
                     one: {signalled:?}",
                );
            }
            other => panic!(
                "⛔⛔⛔⛔ REGISTER ITEM 654: a run cancelled OVER THE SOCKET must report its peer's \
                 job SIGNALLED, exactly as the in-process driver does. `Unsupported` is the word \
                 this item exists to have removed — it says «this host cannot stop a pane's job» \
                 about a daemon that owns the pseudoterminal. Got {other:?}",
            ),
        }
    }
    // ⛔⛔⛔⛔ **AND THE TURN REALLY ENDED.** No outcome word can prove this: `Stopped::Job` says a
    // signal was DELIVERED, and `stop`'s own module doc is explicit that delivery is not obedience.
    // The pid leaving the pane's foreground job is the product's claim, read off the process table.
    assert!(
        wait_until(Duration::from_secs(10), || !job_still_holds(
            &mut setup,
            pane,
            working_pid
        )),
        "⛔⛔⛔⛔ REGISTER ITEM 654: the run reported its work stopped and process {working_pid} is \
         STILL IN THE PANE'S FOREGROUND JOB. That is the whole defect said out loud — a loop's door \
         closed on a room that is still occupied, and the peer goes on spending after the run that \
         started it has been reported over.",
    );
    // ⚠ ENDING A TURN AND ENDING A PANE ARE DIFFERENT ACTS. The sibling gate below is where that
    // distinction is made to carry weight; here it is the cheap half of the same claim.
    assert!(
        remote.pane_ids().contains(&pane),
        "⚠⚠⚠ the stop took the PANE with the turn — a cancelled run must leave its peer's pane, its \
         shell and its scrollback exactly where it found them",
    );
}

/// ⛔⛔⛔⛔ **A STOP THAT CROSSES THE SOCKET CARRIES THE NARROW REACH, SO A RUN THAT RAN OUT OF TIME
/// CANNOT CLOSE SOMEBODY'S PANE** — register item 654, and the reason this needed a wire ARGUMENT
/// rather than only a client.
///
/// # ⚠⚠⚠⚠⚠ The verb was already there and it was the WRONG ACT
///
/// The item's first question was whether `stop_job` could carry a driver's stop with no new
/// address. It could — and its grammar could not. That verb was written for a PERSON naming one
/// pane on purpose, so it always delivered `Reach::TheProgramToo`, and the difference is not a
/// degree: under the wide reach a stop that would KILL the pane's own program is delivered and the
/// pane goes with it. `sprag_terminal::stop`'s own measurement of that path is *it closed one, and
/// the daemon exited behind it.* A run whose clock simply ran out must never be able to do that,
/// which is what `Reach::UnderTheProgram` is for and what a remote driver had no way to say.
///
/// # ⚠⚠⚠ The fixture is the AI loop's own shape, not a contrived one
///
/// A pane opened running its peer — `open_pane`'s `cmd`, which `Unstopped::WouldEndThePane`'s doc
/// names as the preferred path — is a pane whose OWN program owns the terminal. `sleep` stands in
/// for the peer because it dies of a `SIGINT` and says so through `/proc`, which is exactly the
/// condition the narrow reach declines to act on.
///
/// So the pass is: the run is cut short, the daemon DECLINES, the refusal crosses back as the word
/// it left as, and **the pane is still there**. The failure it is built to catch is the opposite of
/// a missing feature — it is a stop that worked too well.
#[test]
fn a_stop_that_crosses_the_socket_keeps_the_narrow_reach_and_leaves_the_pane_standing() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    // ⚠ The boot pane is a SECOND pane and is never driven. Without it the daemon's last pane is
    // the one under test, and a mutation that closes it takes the daemon down too — which would red
    // this gate for the right reason by the wrong mechanism, and leave the socket unreadable at the
    // moment the assertion wants to ask about it.
    let _host = spawn_host_at(&sock, &["cat"]);
    let (remote, mut setup) = remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sleep", "300"] }));

    let mut working = Orchestrator::new(
        pane,
        OrchestrationSpec {
            stimulus: "a-word-for-the-peer".to_owned(),
            sentinel: Some("this-turn-never-finishes".to_owned()),
            ready_when: None,
            ready_within: None,
            may_answer: None,
            attended: Attended::NoOne,
            turn: sprag_plugin::Turn::lasting(sprag_plugin::DoneWhen::Exits, None),
        },
    );
    // ⚠⚠ THE DEADLINE, not a cancel — the OTHER of the two endings `stop_the_work` runs on, and the
    // one the item's sentence is about (*a routine timeout must not be able to close somebody's
    // pane*). Between the two gates both endings are driven over this socket.
    let outcome = Driver::new(Guardrails {
        max_iterations: u32::MAX,
        max_cost: None,
        max_duration: Some(Duration::from_secs(3)),
    })
    .run(&mut working, &remote, &RunContext::uncancellable());

    assert!(
        outcome.stopped.is_some(),
        "⛔⛔ a run cut short by its own clock must have REACHED for the work it started, whatever \
         the answer was: {outcome:?}",
    );
    if cfg!(target_os = "linux") {
        assert_eq!(
            outcome.stopped,
            Some(sprag_plugin::Stopped::Unreached(PaneError::NotStopped(
                sprag_terminal::Unstopped::WouldEndThePane
            ))),
            "⛔⛔⛔⛔ REGISTER ITEM 654: the daemon had to DECLINE this stop, and the refusal had to \
             cross back as the word it left as. Two different failures land here and both matter: \
             a `Job(_)` means the narrow reach never reached the daemon and this run just killed \
             the pane's own program; an `Unreachable` means the reach DID cross and the refusal did \
             not, so a remote run publishes a different sentence from an in-process one about the \
             same event. Got {:?}",
            outcome.stopped,
        );
    }
    // ⛔⛔⛔⛔ **THE CLAIM.** Nothing above this line distinguishes «declined» from «delivered and
    // the pane happened to survive», and nothing below it can be satisfied by a client that merely
    // spells the word `reach` — the pane is either still there or it is not.
    assert!(
        wait_until(Duration::from_secs(5), || remote.pane_ids().contains(&pane)),
        "⛔⛔⛔⛔ REGISTER ITEM 654: A RUN THAT RAN OUT OF TIME CLOSED THE PANE IT WAS GIVEN. That is \
         what the wide reach does to a pane whose own program is the work, it is measured in \
         `sprag_terminal::stop` («it closed one, and the daemon exited behind it»), and it is the \
         reason this stop had to carry a reach across the wire instead of borrowing the verb a \
         person uses.",
    );
    assert!(
        remote.pane_collapsed(pane).is_some(),
        "⚠⚠ and the pane is still a pane this driver can read, not merely an id still in a list",
    );
}

/// ⛔⛔⛔⛔ **A STATE WORD THIS BUILD CANNOT SPELL IS A SKEW, NOT A SHELL** — register item 564.
///
/// # ⚠⚠⚠⚠⚠ The collapse, and why it goes live on an ordinary day
///
/// `pane_agent_state` answering [`None`] means *this pane is not an agent — carry on*. Until this
/// item, a verdict carrying a state word this driver's vocabulary did not hold produced the SAME
/// `None`, so a supervisor would conclude *"a shell"* about a pane running an agent it had never
/// heard of, and drive straight past it.
///
/// It needs no exotic setup to happen: the daemon and the driver are separate processes, a
/// `cargo build` replaces one and not the other, and a newer daemon publishing a fifth state is
/// exactly the case item 412 records as the ORDINARY state after any rebuild here.
///
/// # ⚠⚠⚠ The fixture is the only one that can stage it, and this repository already owned it
///
/// No `cargo build` can produce this skew — the vocabulary has ONE definition, so this build cannot
/// make a daemon say a word it cannot read. `sprag_peer::OldDaemon::proxying` with
/// `Missing::answering` is the tool: a REAL daemon behind it, every byte its own, and one key of one
/// reply rewritten. ⚠ `agent.<pane>` is the only address on this wire whose `result` is an object
/// with a top-level `state`, so the edit reaches that verdict and nothing else.
#[test]
fn a_state_word_this_build_cannot_spell_stops_the_driver_claiming_to_supervise() {
    let (_host, upstream) = spawn_host();
    let ahead = sprag_peer::OldDaemon::proxying(
        &socket_path(),
        &upstream,
        // A word from a daemon one build ahead. It is not in `AgentState`'s vocabulary and this
        // build has no way to invent it.
        sprag_peer::Missing::answering(&[("state", json!("dreaming"))]),
    );
    let (remote, mut setup) = remote_driver(ahead.sock());
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["cat"] }));
    setup
        .call(
            "scene/invoke",
            json!({
                "path": mux_action_path(REPORT_AGENT_ACTION),
                "args": { "id": pane.0, "source": "hook:claude", "state": "working" },
            }),
        )
        .expect("a real report, so the daemon really has a verdict to rewrite");

    // ── THE CONTROL: this surface CAN supervise before it meets the word ───────────────────────
    let supervisor = remote
        .supervision()
        .expect("the daemon supervises, and nothing has gone wrong yet");
    assert!(
        remote.unspellable_state().is_none(),
        "nothing has been read yet, so a latch here would fire on a daemon that never spoke",
    );

    // ── THE READ MEETS A WORD FROM THE FUTURE ─────────────────────────────────────────────────
    let seen = supervisor.pane_agent_state(pane);
    assert!(
        seen.is_none(),
        "⚠⚠⚠ a verdict this build cannot read must not be turned into one it can — a fallback \
         variant would have a supervisor act on a state nobody published. Got {seen:?}",
    );
    assert_eq!(
        remote.unspellable_state().as_deref(),
        Some("dreaming"),
        "⛔⛔⛔⛔ REGISTER ITEM 564: the surface must KEEP the word, verbatim. It is the only thing \
         that tells a person WHICH build is ahead, and a skew reported as a shrug sends them \
         looking at the pane instead of at the two binaries",
    );

    // ── AND THE WHOLE SURFACE STOPS CLAIMING TO LOOK ──────────────────────────────────────────
    assert!(
        remote.supervision().is_none(),
        "⛔⛔⛔⛔ REGISTER ITEM 564, AND THE COLLAPSE THIS CLOSES: a driver that met a verdict it \
         could not read must answer *ask a person, nothing here can look* — NOT *this pane is a \
         shell*. The second is what it used to say, and it makes a supervisor drive straight past \
         a pane running an agent it has never heard of",
    );

    let _ = std::fs::remove_file(&upstream);
}

/// **WHAT A REMOTE DRIVER'S ROUND TRIP ACTUALLY COSTS** — register item 565, and a number with a
/// date rather than a sentence.
///
/// # ⚠⚠⚠⚠⚠ Why this exists: the claim was written and never measured
///
/// `RemotePaneAccess` asks the daemon on every `supervision()` call rather than caching, and its own
/// documentation defends that with *"a round trip is sub-millisecond, so serialising them costs
/// nothing measurable"*. **No date, no number, no instrument** — which is precisely what this
/// repository calls UNMEASURED, and it was being said about the path a run walks every step
/// (`outer.rs` asks `supervision` on five of them).
///
/// # ⚠⚠⚠ What is asserted, and why the bound is loose on purpose
///
/// The measurement is the point; the assertion is a TRIPWIRE. A unix-socket round trip on this
/// fleet is tens of microseconds, and the bound here is **20 ms per call** — three orders of
/// magnitude of headroom, so it cannot flake on a loaded box and cannot pass a regression that
/// turned a socket read into something with a sleep or a retry in it. ⚠ A tight bound would be a
/// timing assertion in a suite that runs at thirty-one threads, which is a flake with extra steps.
///
/// ⚠⚠ It PRINTS the per-call figure, because the number is what a person reads back. An assertion
/// that only says *"under the bound"* answers the tripwire's question and never the item's.
#[test]
fn a_remote_supervision_read_costs_what_its_documentation_claims() {
    let (_host, sock) = spawn_host();
    let (remote, _setup) = remote_driver(&sock);
    // Adopt first, so the ONE-OFF cost of learning the daemon's identity is not averaged into the
    // per-call figure this is about.
    let _ = remote.supervision();

    const CALLS: u32 = 200;
    let began = Instant::now();
    for _ in 0..CALLS {
        assert!(
            remote.supervision().is_some(),
            "the daemon supervises throughout, or the loop below is timing a refusal",
        );
    }
    let each = began.elapsed() / CALLS;
    println!(
        "REGISTER ITEM 565 — supervision() over a real socket: {each:?} per call, \
         {CALLS} calls in {:?}. A step that asks it five times therefore pays {:?}.",
        began.elapsed(),
        each * 5,
    );
    assert!(
        each < Duration::from_millis(20),
        "⛔⛔⛔⛔ REGISTER ITEM 565: a remote supervision read took {each:?}. The bound is a \
         TRIPWIRE at three orders of magnitude above a unix-socket round trip, so passing it is \
         not the claim — what fails it is a read that grew a sleep, a retry or a second round \
         trip. `outer.rs` asks this five times per step.",
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **A REPLACEMENT THAT CANNOT BE BORN LEAVES THE RUN HOLDING THE PANE IT HAD** — register
/// item 566, and the one property that makes a session rollover safe to ATTEMPT at all.
///
/// # ⚠⚠⚠⚠⚠ Why this was ungated, and what that cost
///
/// [`sprag_plugin::PaneLifecycle::respawn`] spawns the replacement BEFORE closing the outgoing pane,
/// so a spawn that fails leaves the caller with the pane it started with. Every other gate in this
/// workspace exercises the path where the spawn SUCCEEDS — so **reversing the two statements passed
/// all of them.** The order was a claim the documentation made and nothing could contradict, on the
/// path whose entire purpose is that nothing is lost.
///
/// # ⚠⚠⚠ The seam: a pane that is ALIVE and whose argv cannot be run again
///
/// The item filed this as unreachable, reasoning that the replacement re-runs the argv the pane is
/// *currently* running, so the program exists by construction. **It existed at spawn time. Nothing
/// says it exists now.** The fixture is a copy of `cat` under a name this test owns: the pane is
/// spawned from it, the file is unlinked, and the running pane does not notice — the kernel holds
/// the inode for as long as the process does. The pane is live, named and echoing, and its argv
/// cannot be exec'd a second time.
///
/// ⚠⚠ That is not a contrived state. It is what an upgrade, a `cargo build`, or a swept temp
/// directory does to a pane that has been open since before them — which is the ordinary condition
/// of the long-lived session a rollover is FOR.
///
/// # ⚠⚠ What each half catches on its own
///
/// * **The refusal.** An `Ok` here would mean a replacement was announced that is not running
///   anything, and the outgoing pane closed against it — a worse loss than the refusal, because the
///   caller would believe the rollover happened.
/// * **The pane is still there afterwards, by NAME and by ECHO.** The name alone can pass on a
///   listing row a closed pane left behind; the echo is what says the PROGRAM is still alive. ⚠ The
///   echo comes from the terminal because the program is `cat`, which leaves the line discipline
///   alone — item 568's fixture, for item 568's reason.
#[test]
fn a_replacement_that_cannot_be_born_leaves_the_run_holding_the_pane_it_had() {
    let (_host, sock) = spawn_host();
    let (remote, mut setup) = remote_driver(&sock);

    let source = ["/bin/cat", "/usr/bin/cat"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("every platform this suite runs on has a `cat` to copy");
    let victim = std::env::temp_dir().join(format!("sprag-566-{}", std::process::id()));
    let _ = std::fs::remove_file(&victim);
    std::fs::copy(source, &victim).expect("copy `cat` under a name this test owns");

    let pane = spawn_pane(
        &mut setup,
        json!({ "cmd": [victim.to_string_lossy()], "name": DRIVEN }),
    );

    let echoed = |what: &str| {
        remote
            .pane_full_lines(pane)
            .unwrap_or_default()
            .iter()
            .any(|line| line.contains(what))
    };
    let say = |what: &str| {
        deliver(
            &remote,
            &RunContext::uncancellable(),
            pane,
            what,
            &Delivery::new(),
        )
    };

    // ── THE CONTROL: the pane is alive and answering BEFORE anything is taken away ─────────────
    say("alive-before").expect("the fixture pane takes a delivery");
    assert!(
        wait_until(Duration::from_secs(10), || echoed("alive-before")),
        "⚠⚠ the fixture pane never echoed, so «still there» below would be a claim about a pane \
         that was never alive — which passes for the wrong reason. Read {:?}",
        remote.pane_full_lines(pane),
    );

    // The program the pane is running stops existing on disk. The pane does not notice.
    std::fs::remove_file(&victim).expect("unlink the program the pane was started from");

    let lifecycle = remote.lifecycle().expect("this host opens panes");
    let refusal = lifecycle.respawn(pane).err().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 566: `respawn` answered Ok for a pane whose argv no longer \
             exists. Either a replacement was announced that is running nothing, or the spawn \
             failure was swallowed — and in both readings the outgoing pane was closed against a \
             pane that cannot serve. A rollover that cannot happen must SAY so."
        )
    });

    println!("REGISTER ITEM 566 — the refusal a dead argv produces: {refusal}");

    // ⚠⚠⚠⚠⚠ THE REFUSAL IS ATTRIBUTED, IN THE SAME BREATH, OR THIS GATE IS ABOUT NOTHING. A
    // `respawn` that refused for a bookkeeping reason — an id nobody holds, a pane with no recorded
    // argv — would satisfy every assertion below while never reaching the spawn, so the order this
    // item is about would still be ungated. A pane running the SAME program from a path that still
    // exists must roll over, against the same daemon, moments later.
    let control = spawn_pane(&mut setup, json!({ "cmd": [source] }));
    lifecycle.respawn(control).unwrap_or_else(|error| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 566: this daemon cannot replace a pane AT ALL — {error}. The \
             refusal above is then a fact about `respawn` rather than about an argv that cannot be \
             run, and the order the gate is for was never exercised."
        )
    });

    // ── THE CLAIM: the caller still holds what it had ─────────────────────────────────────────
    assert_eq!(
        remote.pane_named(DRIVEN),
        Some(pane),
        "⛔⛔⛔⛔ REGISTER ITEM 566: the spawn failed ({refusal}) and the pane the caller named is \
         GONE. That is the loss the order exists to prevent: `respawn` must spawn before it \
         closes, so a rollover that cannot happen costs an error rather than the session it was \
         preserving. A run meeting this loses the work in that pane and has nothing to re-adopt.",
    );

    say("alive-after").expect("the pane the caller still holds takes a delivery");
    assert!(
        wait_until(Duration::from_secs(10), || echoed("alive-after")),
        "⛔⛔⛔⛔ REGISTER ITEM 566: the pane is on the daemon's listing but its PROGRAM is not \
         answering. A name that outlives the process behind it is worse than a clean refusal — the \
         caller reads «my session survived» off a row nothing is running. Read {:?}",
        remote.pane_full_lines(pane),
    );

    let _ = std::fs::remove_file(&sock);
}

/// ⛔⛔⛔⛔ **NO ADDRESS THIS DAEMON PUBLISHES FOR A PANE HANDS BACK INPUT THE TERMINAL REFUSED TO
/// ECHO** — register item 567, and the one thing a wire read can reach that a screen read cannot.
///
/// # ⚠⚠⚠⚠⚠ What the exposure is, stated exactly
///
/// A client holding this socket can already inject keys, spawn processes and read every screen —
/// **the socket is the trust boundary and no read here grants a privilege it did not have.** What
/// the echo trail adds is the one class of text that is not on the grid by construction: **input the
/// terminal was told not to echo.** A password typed at a `sudo` or `ssh` prompt is in what sprag
/// remembers writing and is nowhere on the screen, so a client that only READS can harvest it where
/// before it could not.
///
/// # ⚠⚠⚠ Why this reads the SCHEMA rather than a list of slots
///
/// A hand-written list of addresses to check decides alone: the day a seventh pane surface is
/// published, the list still says six and the gate is green about a wire it no longer describes.
/// So the population is [`sprag_host::wire::PANE_SCHEMA`] itself — every field the daemon DECLARES
/// as a read — and a new address joins this gate by existing. ⚠ The parametric families are named
/// and skipped rather than silently dropped: each takes an argument this gate has no value for, and
/// a skip nobody can see reads as coverage.
///
/// # ⚠⚠ The control is the half that makes the silence mean something
///
/// A secret that never arrived is absent from every address for an uninteresting reason. So the
/// program under the pane REPORTS what it received — its length — and the gate waits for that
/// number before it asks anything. `got:7` on the screen and `hunter2` nowhere is the claim; `got:0`
/// would mean the fixture, not the wire, is what kept the secret.
#[test]
fn no_published_pane_address_hands_back_input_the_terminal_did_not_echo() {
    const SECRET: &str = "hunter2";
    let (_host, sock) = spawn_host();
    let mut conn = setup_at(&sock);

    // ⚠ `stty -echo` FIRST, then the readiness marker: when `ready` reaches the screen the terminal
    // has already stopped echoing, so nothing typed after it can arrive on the grid by accident.
    let pane = spawn_pane(
        &mut conn,
        json!({
            "cmd": ["sh", "-c", "stty -echo; printf 'ready\\n'; read secret; printf 'got:%s\\n' \"${#secret}\""],
        }),
    );

    let screen = |conn: &mut HostConn| -> String {
        conn.call(
            "scene/query",
            json!({ "path": pane_input_path(pane.0, FULL_TEXT_SLOT) }),
        )
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
    };

    assert!(
        wait_until(Duration::from_secs(10), || screen(&mut conn)
            .contains("ready")),
        "the fixture never reached its read, so nothing below would be about a terminal that \
         refuses to echo. Read {:?}",
        screen(&mut conn),
    );

    conn.call(
        "scene/invoke",
        json!({
            "path": pane_input_path(pane.0, TEXT_ACTION),
            "args": { "text": format!("{SECRET}\n") },
        }),
    )
    .expect("type the secret into the pane");

    // ── THE CONTROL: the secret ARRIVED, and the program says so in a word that is not the secret
    assert!(
        wait_until(Duration::from_secs(10), || screen(&mut conn)
            .contains(&format!("got:{}", SECRET.len()))),
        "⚠⚠ the program never reported receiving the secret, so «it is nowhere» below would be a \
         claim about a secret that was never delivered. Read {:?}",
        screen(&mut conn),
    );
    assert!(
        !screen(&mut conn).contains(SECRET),
        "⚠⚠ the terminal ECHOED it after all, so this pane is not the fixture this gate needs — \
         the whole question is about text a screen read cannot reach. Read {:?}",
        screen(&mut conn),
    );

    // ── EVERY DECLARED READ, FROM THE SCHEMA ITSELF ────────────────────────────────────────────
    let mut asked = 0_u32;
    let mut skipped: Vec<&str> = Vec::new();
    for field in sprag_host::wire::PANE_SCHEMA {
        if field.channel != pinion_core::external::SchemaChannel::Read {
            continue;
        }
        if !field.args.is_empty() {
            skipped.push(field.path);
            continue;
        }
        let answer = conn
            .call(
                "scene/query",
                json!({ "path": pane_input_path(pane.0, field.path) }),
            )
            .unwrap_or(Value::Null);
        asked += 1;
        assert!(
            !answer.to_string().contains(SECRET),
            "⛔⛔⛔⛔ REGISTER ITEM 567: the pane address `{}` handed a read-only client input the \
             terminal REFUSED TO ECHO. The secret is not on this pane's screen and it is in this \
             answer, which is exactly the class of text a wire read must not reach — a password at \
             a `sudo` or `ssh` prompt is the shipping case. Answered {answer}",
            field.path,
        );
    }
    println!(
        "REGISTER ITEM 567 — asked {asked} declared pane read(s); parametric families skipped for \
         want of an argument: {skipped:?}"
    );
    assert!(
        asked >= 5,
        "⚠⚠⚠ only {asked} declared reads were asked, which is too few for this to be a sweep of \
         the pane surface. The population is `PANE_SCHEMA` and it does not shrink — a number this \
         small means the filter above stopped matching the schema rather than that the wire got \
         smaller",
    );

    // ── AND THE ONE FAMILY THE SWEEP CANNOT REACH IS ASKED BY HAND ────────────────────────────
    //
    // ⚠⚠⚠⚠⚠ `recent_input_has.<needle>` is skipped above for want of an argument — and it is the
    // exact address this item is about, so a sweep that only skipped it would be green about a
    // regression that served the trail here instead of a bool. It is asked with the SECRET itself,
    // which is the strongest form: the daemon still remembers, the answer is `true`, and `true` is
    // three characters that are not `hunter2`.
    let knows = conn
        .call(
            "scene/query",
            json!({ "path": pane_input_path(pane.0, &recent_input_has(SECRET)) }),
        )
        .expect("the question is served");
    assert_eq!(
        knows,
        json!(true),
        "⛔⛔⛔⛔ REGISTER ITEM 567: the address must answer the QUESTION as a bool. A `{knows}` \
         here is either a trail wearing a new name — the whole exposure, moved rather than closed \
         — or a pane that stopped recording, which silently disarms `ReadyWhen::Prints`' refusal \
         and puts the barrier back on a race",
    );
    let absent = conn
        .call(
            "scene/query",
            json!({ "path": pane_input_path(pane.0, &recent_input_has("never-typed-here")) }),
        )
        .expect("the question is served for a needle nobody typed");
    assert_eq!(
        absent,
        json!(false),
        "⚠⚠⚠ a needle nobody typed must answer `false`, or the `true` above is a constant and this \
         address answers nothing at all",
    );

    let _ = std::fs::remove_file(&sock);
}

/// A boot pane that ANSWERS rather than echoes: every line it reads comes back wearing a prefix
/// the typist never typed.
///
/// ⚠⚠⚠ Why not `cat`, which every other test here uses. A pseudoterminal echoes its own input, so
/// with `cat` the sentinel a run waits for would be satisfied by the run's own keystrokes — a
/// control that shares a word with the failure it is meant to separate. `ANSWER:` can only be on
/// that screen because the program on the far side of the PTY put it there, so a run that converges
/// on it converged because A PEER SPOKE.
const ANSWERING_PEER: &str = "while read line; do echo \"ANSWER:$line\"; done";

/// A peer that COUNTS its answers and takes its time — for the gate that must watch a run while it
/// is still going.
///
/// # ⚠⚠⚠⚠⚠ Why a run has to be slow to be watchable, measured
///
/// The first form of [`the_daemon_drives_a_run_in_a_process_of_its_own`] used [`ANSWERING_PEER`] and
/// a sentinel the peer satisfies on its FIRST answer. The run converged in one turn, before the
/// first read of the row — so the row went `0` → `done` with no observable middle, and the
/// mid-flight claim failed against a product that was working. ⚠ A gate that cannot see the state
/// it is about is not measuring the product.
///
/// So this one answers `ANSWER:1:`, `ANSWER:2:`, … and pauses before each, which makes *three turns*
/// a real interval rather than a race. The sentinel below names the third.
const COUNTING_PEER: &str =
    "n=0; while read line; do n=$((n+1)); sleep 0.4; echo \"ANSWER:$n:$line\"; done";

/// The sentinel only [`COUNTING_PEER`]'s THIRD answer carries.
///
/// ⚠ A word the typist cannot produce, for [`PEER_ANSWERED`]'s reason, and one no EARLIER answer
/// carries either — so a run that converged on turn 1 could not have satisfied it.
const PEER_ANSWERED_THRICE: &str = "ANSWER:3:";

/// The prefix [`ANSWERING_PEER`] wears, and the sentinel the runs below wait for.
const PEER_ANSWERED: &str = "ANSWER:";

/// **A RUN DRIVEN BY A SEPARATE PROCESS CONVERGES AGAINST A REAL DAEMON** — register item 643.
///
/// # ⚠⚠⚠⚠⚠ What this is the first of
///
/// `RemotePaneAccess` has been exercised by tests since it was written, and by nothing else. This
/// is its first PRODUCTION caller: `sprag-term --drive` builds the same plugin from the same
/// request the daemon would have built (`drive_request`, one builder), against a world answered
/// over the socket (`RemotePluginWorld`), typing at a pane it does not own and cannot see except
/// through the wire.
///
/// So what a red here means is not "the driver binary is broken" but "a run cannot leave this
/// process" — which is the whole of item 544's premise.
///
/// # ⚠⚠ The two claims, and why the second one is here
///
/// 1. The driver exits 0 and reports a `converged` outcome on stdout.
/// 2. **The peer's answer is on the pane.** Without it, claim 1 is satisfiable by a driver that
///    reported convergence without typing anything at all — and a driver that reports without
///    driving is the exact defect an out-of-process run makes possible.
///
/// ⚠⚠⚠⚠⚠ **CLAIM 2 IS NOT DECORATION, AND IT TOOK FOUR MUTATIONS TO SHOW IT.** Silencing
/// `RemotePaneAccess::inject` reds claim 1, never claim 2 — a run that types nothing never moves the
/// pane, and this plugin READS ONLY AFTER THE PANE MOVES, so a forged read is not even consulted.
/// The only mutation that separates the two is REAL TYPING + A SILENT PEER + A FORGED SENTINEL
/// READ, and then claim 1 stays green while this one reds. ⚠ The forgery has to target
/// `SCREEN_COLLAPSED_SLOT`: the sentinel is checked through `PaneAccess::pane_collapsed`, and a
/// draft of that mutation aimed at the pane's TEXT address and changed nothing at all.
#[test]
fn a_run_driven_by_a_separate_process_converges_against_a_real_daemon() {
    let (_host, sock) = spawn_host_running(&["sh", "-c", ANSWERING_PEER]);

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let pane = *pane_ids(&mut conn).first().expect("the boot pane");

    let request = json!({
        "plugin": "orchestrator",
        "pane": pane,
        "stimulus": "driven-from-another-process",
        "sentinel": PEER_ANSWERED,
        "guardrails": { "max_iterations": 20, "max_seconds": 30 },
    });

    let driven = drive_in_a_child(&sock, 1, &request);

    assert_eq!(
        driven["state"],
        json!("converged"),
        "⚠⚠⚠ a run driven from another process must converge on the peer's answer: {driven:?}",
    );

    // Claim 2. The daemon is asked, not the child — the pane is the daemon's, and its screen is the
    // only place a keystroke that actually crossed the socket can show up.
    let answered = wait_until(Duration::from_secs(5), || {
        pane_text(&mut conn, pane).contains(PEER_ANSWERED)
    });
    assert!(
        answered,
        "⚠⚠⚠⚠⚠ the run reported convergence but the peer never answered on the pane — a driver \
         that reports without typing is exactly what an out-of-process run makes possible, and \
         claim 1 alone cannot tell the two apart",
    );

    let _ = std::fs::remove_file(&sock);
}

/// **A REQUEST NO PLUGIN SPELLS IS REFUSED BEFORE A BYTE IS TYPED** — the control for the gate
/// above.
///
/// ⚠ Its own claim, not decoration: the driver's builder runs BEFORE its three connections do any
/// work, and a malformed request that reached the typing stage would be a run driving a pane on a
/// plan nobody could read. A non-zero exit and an empty stdout are the two halves of "it did not
/// pretend to have an outcome".
#[test]
fn a_driver_given_a_request_no_plugin_spells_reports_nothing_and_fails() {
    let (_host, sock) = spawn_host_running(&["sh", "-c", ANSWERING_PEER]);

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let pane = *pane_ids(&mut conn).first().expect("the boot pane");

    let out = drive_child(
        &sock,
        2,
        &json!({ "plugin": "no-such-plugin-lives-here", "pane": pane }),
    );

    assert!(
        !out.status.success(),
        "⚠⚠⚠ a request the builder refuses must fail the driver process, not exit 0 with nothing",
    );
    assert!(
        out.stdout.is_empty(),
        "⚠⚠⚠ a refused request has no outcome to report, and a driver that writes one is \
         manufacturing an answer: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );

    let _ = std::fs::remove_file(&sock);
}

/// **THE DAEMON PUTS A RUN IN A PROCESS OF ITS OWN, AND THE ROW STILL TELLS YOU EVERYTHING** —
/// register items 544, 643 and 650, end to end.
///
/// # ⚠⚠⚠⚠⚠ What this is the first of, and why the two claims are one gate
///
/// Every earlier gate here spawned the driver ITSELF. This one asks the DAEMON to, through the
/// ordinary `run` action with `run-driver-process` on — so the seam under test is the one a person
/// actually reaches, which is the distinction register item 373 paid a round for learning to make.
///
/// And it asks the row TWICE for a reason:
///
/// 1. **While it runs**, the row must MOVE. A run whose driver is elsewhere writes its counters into
///    a `ProgressCell` on the other side of a socket, and the shipped first driver was handed one
///    nobody reads — so this row sat at zero for a run's whole life. That is register item 492's
///    shape in the feature whose subject is a run somebody can watch.
/// 2. **When it ends**, the row must carry the OUTCOME, published as `done` like any other run's.
///    An ending that crossed a process boundary and arrived as a fifth status word would be a break
///    no client could see coming (item 342).
///
/// ⚠⚠ Nothing here says `--drive`, names a socket, or reads a child's stdout. If this passes while
/// the daemon quietly drove the run in-process, claim 1 is what notices: an in-process run's row
/// moves too. So the gate below asserts the PREMISE — that the option is what is in force — by
/// reading the run's own build/driver evidence rather than trusting the environment it set.
#[test]
fn the_daemon_drives_a_run_in_a_process_of_its_own() {
    // ⚠ THE OPTION IS SET THE WAY A PERSON SETS IT — a config file the daemon reads — because it is
    // read through `config::option_is_on` and there is no environment path to it. An env var of its
    // own would be a second way to turn this on, testable and unlike production.
    let config = config_home(&format!(
        "[options]\n{} = \"on\"\n",
        sprag_host::options::RUN_DRIVER_PROCESS
    ));
    let (_host, sock) = spawn_host_with(
        &["sh", "-c", COUNTING_PEER],
        &[("XDG_CONFIG_HOME", config.to_str().expect("a utf-8 path"))],
    );

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let pane = *pane_ids(&mut conn).first().expect("the boot pane");

    let started = conn
        .call(
            "scene/invoke",
            json!({
                "path": sprag_host::plugins_path(sprag_host::plugins::RUN_ACTION),
                "args": {
                    "plugin": "orchestrator",
                    "pane": pane,
                    "stimulus": "driven-by-the-daemon",
                    "sentinel": PEER_ANSWERED_THRICE,
                    "guardrails": { "max_iterations": 20, "max_seconds": 30 },
                },
            }),
        )
        .expect("the daemon starts a run");
    let run = started.as_u64().expect("a run answers its id");

    // Claim 1 — it moves WHILE IT RUNS.
    //
    // ⚠⚠⚠ READ OFF THE `running` ARM AND NOWHERE ELSE. Once a run is `done` its counters move under
    // `outcome`, so accepting them from there would let a run that reported NOTHING mid-flight pass
    // on the strength of its ending — which is exactly the half this claim exists to separate.
    let mut seen: Option<Value> = None;
    let moved = wait_until(Duration::from_secs(20), || {
        let row = run_row(&mut conn, run);
        let running = row["state"]["status"] == json!("running");
        let done_so_far = row["state"]["iterations"].as_u64().unwrap_or(0);
        if running && done_so_far > 0 {
            seen = Some(row);
            return true;
        }
        false
    });
    assert!(
        moved,
        "⚠⚠⚠⚠⚠ a run the daemon put in another process must show what it has done WHILE IT IS \
         STILL GOING — a row frozen at zero for the run's whole life is the black box this feature \
         must not build. Last row: {:?}",
        run_row(&mut conn, run),
    );
    // ⚠⚠ AND THE ANSWER TALLY IS THERE TOO, which is what proves the report crossed WHOLE rather
    // than one key of it: `progress_to_json` publishes four keys and the daemon stores the object
    // without reading it apart, so a row missing this one would mean somebody unpacked it.
    let mid = seen.expect("the row that satisfied the wait");
    assert!(
        mid["state"][sprag_host::plugins::RUN_ANSWERED_KEY].is_u64(),
        "⚠⚠⚠ the report a driver sent is spliced WHOLE — a mid-flight row without the answer tally \
         means a reader here unpacked it key by key and forgot one: {mid:?}",
    );

    // Claim 2 — it ends, and the ending is on the row under the same word every run uses.
    let ended = wait_until(Duration::from_secs(40), || {
        run_row(&mut conn, run)["state"]["status"] == json!("done")
    });
    let row = run_row(&mut conn, run);
    assert!(ended, "the run ends inside its own clock: {row:?}");
    assert_eq!(
        row["state"]["outcome"]["state"],
        json!("converged"),
        "⚠⚠⚠ and it converged on the peer's answer, read off the row rather than a child's pipe: \
         {row:?}",
    );
}

/// A config directory holding `text` as this daemon's `config.toml`, unique to this CALL.
///
/// ⚠ Unique per call for [`socket_path`]'s reason: these tests are parallel threads of one binary,
/// and a shared directory would have them reading each other's options.
fn config_home(text: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("sprag-wire-cfg-{}-{n}", std::process::id()));
    std::fs::create_dir_all(dir.join("sprag")).expect("a temp config dir");
    std::fs::write(
        dir.join("sprag").join(sprag_host::config::CONFIG_FILE),
        text,
    )
    .expect("write the config");
    dir
}

/// One run's row over the wire, or `Null` where the daemon does not hold it.
fn run_row(conn: &mut HostConn, run: u64) -> Value {
    conn.call(
        "scene/query",
        json!({ "path": sprag_host::plugins_path(sprag_host::plugins::RUNS_SLOT) }),
    )
    .ok()
    .and_then(|runs| {
        runs.as_array()?
            .iter()
            .find(|row| row["id"] == json!(run))
            .cloned()
    })
    .unwrap_or(Value::Null)
}

/// Everything pane `pane` has on its screen and in its scrollback, read over the wire.
fn pane_text(conn: &mut HostConn, pane: u64) -> String {
    conn.call(
        "scene/query",
        json!({ "path": pane_input_path(pane, FULL_TEXT_SLOT) }),
    )
    .ok()
    .and_then(|v| v.as_str().map(ToOwned::to_owned))
    .unwrap_or_default()
}

/// Run `sprag-term --drive <run>` against `sock` with `request` on its stdin, and return what it
/// reported — failing the test if it did not exit cleanly.
fn drive_in_a_child(sock: &Path, run: u64, request: &Value) -> Value {
    let out = drive_child(sock, run, request);
    assert!(
        out.status.success(),
        "⚠⚠⚠ the driver process failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|why| {
        panic!(
            "⚠⚠⚠ a driver reports one JSON object on stdout ({why}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The raw child, for the callers that are asking about a FAILURE.
///
/// ⚠ The socket reaches the driver the way it reaches every other client of this daemon — through
/// `SPRAG_HOST_RPC_SOCK`, which is what the daemon itself will pass when it spawns one. A flag of
/// its own here would be a second answer to "which host", testable and wrong.
fn drive_child(sock: &Path, run: u64, request: &Value) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg(sprag_host::drive::DRIVE_FLAG)
        .arg(run.to_string())
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the driver process");
    // ⚠⚠ THROUGH `feed`, NEVER a hand-rolled `stdin.take()` + `write_all` — register item 471, and
    // the caller below is exactly its case: a driver handed a request no plugin spells REFUSES
    // BEFORE IT READS, so the write meets a closed pipe and a fixture that treats that as fatal
    // reports `Broken pipe` instead of the exit status it came for. `sprag-gate`'s ratchet caught
    // this file the first time it was written the other way.
    sprag_gate::feeding::feed(&mut child, request.to_string().as_bytes());
    child.wait_with_output().expect("reap the driver process")
}

/// **A DRIVER IN ANOTHER PROCESS READS THE CHILD'S SOURCE BYTES, NOT THE GRID'S RENDERING OF
/// THEM** — register item 656, the last of `RemotePaneAccess`'s nine sub-surfaces still answering
/// the trait's default `None`.
///
/// # ⚠⚠⚠⚠⚠ Why this absence is a FALSE ANSWER and not a degradation
///
/// `dialogue::decode_claude_json` asks `raw_capture()` for the pane's source stream and
/// `unwrap_or_default()`s what it gets. A `None` therefore becomes EMPTY BYTES, which parse as no
/// envelope, which lands in the `None => raw_text()` arm — and `raw_text()` of nothing is the empty
/// string. So a `claude -p --output-format json` turn driven from another process publishes an
/// EMPTY reply, `Cost::Tokens(0)`, and no session id, while the same turn driven in-process
/// publishes the model's text, its real billed tokens, and the session to resume. Nobody is told:
/// every one of those is a value the caller reads, not an error it handles.
///
/// # ⚠⚠ The fixture makes its own premise, and asserts it
///
/// The bytes are produced by the pane's CHILD (spawned with a `cmd`, so nothing was typed and the
/// PTY echoes nothing — R27's trap), and the segment words are the SHELL's arithmetic, so no
/// screen and no echo can put `seg-77` into this pane. They are padded so a wrap boundary lands
/// inside a run of spaces: the grid trailing-trims each row, so the rendered read cannot
/// reconstruct the source — which is asserted BELOW, before the claim, because a lossless grid
/// would make the claim measure nothing.
#[test]
fn a_remote_driver_reads_the_source_bytes_the_grid_cannot_give_back() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    let (remote, mut setup) = remote_driver(&sock);

    // 12 segments of 15 columns each = 180 bytes over a 40-column pane: every row boundary but the
    // last falls inside a segment's padding, so the grid loses spaces it can never give back.
    let payload: String = (0..12).map(|i| format!("seg-{:<11}", i * 7)).collect();
    let pane = spawn_pane(
        &mut setup,
        json!({
            "cmd": ["sh", "-c", "i=0; while [ $i -lt 12 ]; do printf 'seg-%-11s' \"$((i*7))\"; i=$((i+1)); done"],
            "cols": 40,
            "rows": 6,
        }),
    );

    assert!(
        wait_until(Duration::from_secs(20), || remote.pane_eof(pane)
            == Some(true)),
        "⚠⚠ the fixture child never finished, so everything below would be a claim about a capture \
         that is still filling. Read {:?}",
        remote.pane_collapsed(pane),
    );

    // ── THE CONTROL: the bytes really were written, and this wire really can read the pane ─────
    let rendered = remote
        .pane_full_lines(pane)
        .expect("the daemon serves this pane's rendered lines")
        .join("");
    assert!(
        rendered.contains("seg-77"),
        "⚠⚠ the child never wrote its last segment, so «the grid cannot reconstruct it» below \
         would pass for the wrong reason — an empty pane cannot reconstruct anything. Read {rendered:?}",
    );

    // ── THE PREMISE, ASSERTED: the rendering is LOSSY, which is why a source read has to exist ──
    assert!(
        !rendered.contains(&payload),
        "⚠⚠⚠ the grid gave the source bytes back verbatim, so this pane cannot tell a raw-capture \
         surface from a screen scrape and the gate below measures nothing. Re-pad the fixture so a \
         wrap boundary falls inside a run of spaces. Read {rendered:?}",
    );

    // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────────
    let raw = remote.raw_capture().unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔⛔ REGISTER ITEM 656: a driver in another process is told this host has NO raw \
             capture at all. That word is about a DEPLOYMENT, and it is false here — the daemon on \
             the other end of this socket owns the pseudoterminal and captures every byte its child \
             writes. What it costs is not an error anybody sees: `decode_claude_json` \
             `unwrap_or_default()`s this absence into empty bytes, so a `--output-format json` turn \
             driven from another process publishes an EMPTY reply, `Tokens(0)` and no session id, \
             while the same turn in-process publishes the model's text, its real tokens and its \
             session. One driver, two answers, nobody told."
        )
    });
    let sprag_terminal::RawOutput { bytes, truncated } =
        raw.pane_raw_output(pane).unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔⛔ REGISTER ITEM 656: the surface answered, but this PANE has no source \
                 bytes — and it is the pane whose child just wrote 180 of them and reached EOF. A \
                 per-pane `None` here says «that pane is not capturing», which is the other \
                 absence entirely, and a caller cannot tell them apart."
            )
        });
    assert!(
        !truncated,
        "⛔⛔ 180 bytes cannot have hit `RAW_CAPTURE_CAP`, so a truncated flag crossing this wire \
         means the flag is being invented rather than carried — and a structured reader treats \
         truncated as unparseable, which would degrade every remote turn that is not over-cap.",
    );
    assert!(
        String::from_utf8_lossy(&bytes).contains(&payload),
        "⛔⛔⛔⛔ REGISTER ITEM 656: the source bytes crossed the socket CHANGED. The whole reason \
         this address exists is that the envelope is parsed from bytes the grid corrupts, so a \
         capture that arrives grid-shaped is the defect wearing the fix's name — a JSON envelope \
         wrapped across rows would still fail to parse. Wanted the child's 180 verbatim bytes \
         within, got {:?}",
        String::from_utf8_lossy(&bytes),
    );

    // ── AND THE TWO ABSENCES STAY APART (register item 641's rule, at this address) ────────────
    assert!(
        raw.pane_raw_output(PaneId(9_999_999)).is_none(),
        "⛔⛔⛔ a pane id no session holds must be `None` here. Answering a default `RawOutput` \
         would hand a caller EMPTY BYTES for a pane that does not exist — indistinguishable from a \
         live child that has printed nothing yet, which is the reading a dialogue turn degrades on.",
    );
}

/// ⛔⛔⛔⛔ **EVERY OPTIONAL SUB-SURFACE THIS DRIVER OFFERS, AND THE ONE IT REFUSES ON PURPOSE** —
/// register item 557's first clause, held by a test instead of by a comment.
///
/// # ⚠⚠⚠⚠⚠ Why the finished set is worth a gate that none of its own rounds was
///
/// Item 557 registered NINE optional sub-surfaces of [`PaneAccess`] that `RemotePaneAccess`
/// answered `None` for, and the last three were paid one per round (register items 653, 654, 656).
/// Nothing holds the finished set. Each of those rounds gated its OWN address — a `hands` read, a
/// stop, a raw capture — and a surface that quietly went back to the trait's default `None` would
/// fail none of them: every one of those gates reaches its capability THROUGH the accessor, and
/// would simply be told there is none, which is the shape `Option` was chosen to make sayable.
///
/// ⚠⚠ **AND THE REFUSAL IS THE HALF NOTHING COULD HAVE READ.** `input_trail` is `None` here
/// DELIBERATELY — register item 567: a trail carries input the terminal was told not to echo, so a
/// read-only client holding this socket could harvest a password from it, and the one question its
/// consumer asks (*is my marker in what was typed*) is served by `recent_input_has.<needle>`
/// carrying a bool instead. **That `None` and an oversight are the same value**, and this is the
/// only place that says which one it is.
///
/// # ⚠ The blind spot, stated rather than shrunk
///
/// This asserts the surfaces the trait has TODAY. A twelfth optional accessor added upstream would
/// default to `None` here and nothing would go red — a list without a glob, which is register item
/// 583's blind spot at another address. What the list does hold is every surface already paid for.
#[test]
fn a_remote_driver_offers_every_optional_sub_surface_but_the_one_it_refuses_on_purpose() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    // ⚠⚠⚠ PARKING, because that is what `crate::drive` builds — and the first draft of this gate
    // used the plain driver and went red on `changes`, which is the product being RIGHT: that
    // accessor is `Some` exactly when a park connection was supplied, and answering otherwise would
    // make every wait return "nothing moved" instantly and spin. The pair below holds both halves.
    let (remote, mut setup) = parking_remote_driver(&sock);
    let pane = spawn_pane(&mut setup, json!({ "cmd": ["sh", "-c", "exec cat"] }));

    // ── THE CONTROL: this is a LIVE surface, not a struct answering `Some(self)` into the void ──
    //
    // ⚠⚠⚠ Every accessor below is an unconditional `Some(self)`, so `is_some()` would answer the
    // same for a driver whose daemon had never existed. What makes the list mean anything is that
    // this driver is really talking to that daemon — asserted by a read only the other side can
    // answer.
    assert!(
        remote.pane_ids().contains(&pane),
        "⚠⚠ the driver cannot see the pane the daemon just made for it, so every `is_some()` below \
         would be a statement about a struct rather than about a wire. Read {:?}",
        remote.pane_ids(),
    );

    // ── THE CLAIM: the ten that are served, each named so a red says WHICH ────────────────────
    for (surface, offered, item) in [
        ("lifecycle", remote.lifecycle().is_some(), "557"),
        ("supervision", remote.supervision().is_some(), "557"),
        ("input_echo", remote.input_echo().is_some(), "557"),
        ("terminal_modes", remote.terminal_modes().is_some(), "557"),
        ("foreground_job", remote.foreground_job().is_some(), "557"),
        ("output_lines", remote.output_lines().is_some(), "557"),
        ("changes", remote.changes().is_some(), "631"),
        ("hands", remote.hands().is_some(), "653"),
        ("job_control", remote.job_control().is_some(), "654"),
        ("raw_capture", remote.raw_capture().is_some(), "656"),
    ] {
        assert!(
            offered,
            "⛔⛔⛔⛔ REGISTER ITEM 557: a run driven from another process is told this host has no \
             `{surface}` at all. That word is about a DEPLOYMENT and it is false — the daemon on \
             the other end of this socket has served that address since register item {item} paid \
             for it. A `None` here does not fail a run; it makes one quietly do without a \
             capability it has, which is the whole class this item exists to close.",
        );
    }

    // ── AND `changes` IS THE ONE THAT IS CONDITIONAL, WHICH ONLY A PAIR CAN SAY ───────────────
    //
    // ⚠⚠⚠⚠ The nine above are unconditional `Some(self)`; this one answers for a CAPABILITY THIS
    // CALLER SUPPLIED. A driver built without a park connection must be told so — `park_until`
    // reads that `None` as the instruction to fall back to a clock, where an always-`Some` would
    // hand it a wait with nowhere to park, answer *nothing moved* instantly, and spin. Asserting
    // only the `Some` half above would leave exactly that always-`Some` passing.
    let (unparked, _setup) = remote_driver(&sock);
    assert!(
        unparked.changes().is_none(),
        "⛔⛔⛔ REGISTER ITEM 631: a driver with NO park connection must answer that it cannot say \
         when a pane changed. Offering `PaneChanges` here would promise a wait this surface has \
         nowhere to make, and the caller would spin on instant «nothing moved» answers instead of \
         falling back to the clock the `None` sends it to.",
    );

    // ── AND THE ONE REFUSAL, PINNED SO IT CANNOT DECAY INTO AN OVERSIGHT ──────────────────────
    assert!(
        remote.input_trail().is_none(),
        "⛔⛔⛔ REGISTER ITEM 567: this surface must NOT offer a `PaneInputTrail`. It is the one \
         that crosses a socket, and a trail carries what the terminal was told not to echo — a \
         password typed at a `sudo` prompt is in it and is nowhere on the grid, so a client that \
         only READS could harvest it. Its one consumer asks *is my marker in what was typed*, and \
         `recent_input_has.<needle>` answers that with a bool that carries nothing home. Serving \
         the trail here would widen what this socket hands out, which is why the old address was \
         withdrawn and why `WIRE_PROTOCOL` moved for it.",
    );
}

/// ⛔⛔⛔⛔ **A PERSON'S CANCEL REACHES A DRIVER IN ANOTHER PROCESS, AND REACHES IT AS A WAKE** —
/// register item 648's remaining half.
///
/// # ⚠⚠⚠⚠⚠ What was already paid, and what this is
///
/// Item 648 measured that `EventKind`'s seventeen variants carry every PANE, WINDOW, SESSION and
/// LAYOUT change and exactly one run fact (`RunFinished`) — so a run's ORDERS were not journal
/// events at all. Half of it was paid the same round: `Event::RunOrdered` exists and the three
/// order doors publish it from their accepted arms. **The other half could not be measured then**,
/// in the item's own words — *"a driver verb has to stand first"* — because the thing that must
/// wake lives in a process this daemon had no way to start. Register item 643 stood that up.
///
/// So this is the half that was owed: not *does the order reach the journal* (gated already) but
/// **does a driver that is not in this process act on it**.
///
/// # ⚠⚠⚠ Why the pane axis is the reason this matters
///
/// Register items 629, 630, 631 and 640 spent four rounds removing exactly this shape from the PANE
/// axis — *do not let the cost of a wait follow a clock* — and a run axis with no wake would have a
/// child driver POLLING its own row. A poll is not merely slower here: an order is not *something
/// that will arrive*, it is **a person who has just spoken**, so every period of latency is spent
/// typing at a peer nobody wants answered any more, and paying for it.
///
/// # ⚠⚠ What the window is and is not
///
/// The ten seconds below is not a latency claim — register item 613 is this workspace's own scar
/// about wall-clock assertions on a shared runner. It is generous enough that a woken driver always
/// makes it.
///
/// # ⛔⛔⛔⛔⛔ It was RED, and what it found was a WRONG SURFACE — register item 660
///
/// Built red on purpose and left `#[ignore]`d for one round, because the defect it measures was
/// live: a person's cancel never reached a run driven from another process, and the same was true
/// of stand-down and hold, silently, for every such run.
///
/// **The cause was one line.** `drive::read_row` asked the MULTIPLEXER for the `runs` listing, and
/// that listing is the PLUGIN HOST's — so the watcher, woken correctly by the journal, could never
/// read the row it had been woken to re-read, and answered `None` every time. Four other causes
/// were driven out first and are recorded on `read_row` itself so nobody re-walks them.
///
/// ⚠⚠⚠ **The fixture had a defect of its own, and it hid the fix for two runs.** This drove the
/// daemon's BOOT pane. A cancel that lands makes the driver stop the work — and the work IS that
/// pane's program, so stopping it closes the pane, and a daemon whose last pane closes exits behind
/// it (register item 654's own measured sentence). The first green therefore arrived looking like a
/// failure: `Broken pipe` on the next read. The run gets a pane of its own for that reason.
///
/// # ⚠⚠⚠⚠⚠ AND IT HOLDS THE JOURNAL AS THE CARRIER — but only once the race was closed
///
/// Withholding `Event::RunOrdered` left this gate GREEN at first, which would have made register
/// item 648's half-payment look like scaffolding nobody leans on. It was a RACE, and finding it is
/// what this gate is worth: a driver subscribes with `since: 0`, so its first delivered batch is
/// the journal's HISTORY — measured, a `layout_updated` from the pane this test spawns. That stale
/// batch can flush after the cancel, wake the watcher, and have it re-read the row and find the
/// order — a cancel landing with nothing having announced it, at iteration 1.
///
/// Spending the catch-up first (the three-step wait below) makes the order's own announce the only
/// traffic that can wake the watcher. **With that in place the mutation is red**: withhold the
/// announce and the driver never hears the cancel. The journal is the carrier, and this gate now
/// says so rather than assuming it.
#[test]
fn a_persons_cancel_wakes_a_driver_in_another_process_rather_than_waiting_for_it_to_look() {
    // The option a person sets, at the door a person sets it — `the_daemon_drives_a_run_in_a
    // _process_of_its_own`'s reason: there is no environment path to it, and inventing one would be
    // a second way to turn this on that production does not have.
    let config = config_home(&format!(
        "[options]\n{} = \"on\"\n",
        sprag_host::options::RUN_DRIVER_PROCESS
    ));
    let (_host, sock) = spawn_host_with(
        &["sh", "-c", COUNTING_PEER],
        &[("XDG_CONFIG_HOME", config.to_str().expect("a utf-8 path"))],
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    // ⚠⚠⚠⚠⚠ A PANE OF ITS OWN, AND NOT THE BOOT PANE — measured, register item 660. A cancel that
    // lands makes the driver STOP THE WORK, and the work here IS the pane's own program; under the
    // wide reach that closes the pane, and a daemon whose last pane closes EXITS behind it (register
    // item 654 wrote that sentence from its own measurement). Driving the boot pane therefore turns
    // a WORKING cancel into `Broken pipe` on the next read — which is exactly what this gate saw,
    // and it read like the cancel had failed when it had in fact succeeded all the way through.
    // ⚠⚠⚠⚠⚠ A SILENT PEER, and that is what makes this gate measure register item 648 AT ALL.
    // `cat` with nothing to say produces no further output once it has echoed the stimulus, so the
    // session goes QUIET — and a quiet session is the only place the order's own journal event can
    // be told apart from the traffic around it. Measured: with a chatty peer, withholding
    // `Event::RunOrdered` left this gate GREEN, because the watcher was woken by that peer's noise
    // and re-read the row anyway. A gate that cannot fail without the mechanism is not measuring it.
    let pane = spawn_pane(
        &mut conn,
        json!({ "cmd": ["sh", "-c", "exec cat"], "name": "ordered" }),
    );

    // ⚠⚠ A SENTINEL THIS PEER NEVER SAYS, so nothing but the order can end the run inside the
    // window below. `COUNTING_PEER` answers `ANSWER:<n>:<line>` and cannot produce this word; the
    // ceilings are set far past the window for the same reason.
    let started = conn
        .call(
            "scene/invoke",
            json!({
                "path": sprag_host::plugins_path(sprag_host::plugins::RUN_ACTION),
                "args": {
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "wait-for-an-order",
                    "sentinel": "A-WORD-THIS-PEER-NEVER-SAYS",
                    "guardrails": { "max_iterations": 200, "max_seconds": 120 },
                },
            }),
        )
        .expect("the daemon starts a run");
    let run = started.as_u64().expect("a run answers its id");

    // ── THE CONTROL: it is really RUNNING when the order is given ────────────────────────────
    //
    // ⚠⚠⚠ Without this the cancel could be "ending" a run that had already stopped on its own, and
    // every claim below would be about nothing. The run must be seen in flight FIRST.
    assert!(
        wait_until(Duration::from_secs(20), || {
            run_row(&mut conn, run)["state"]["status"] == json!("running")
        }),
        "⚠⚠ the run never reached `running`, so the order below would be given to a run that was \
         not there to hear it. Read {:?}",
        run_row(&mut conn, run),
    );

    // ⚠⚠⚠⚠⚠ LET THE SUBSCRIPTION CATCH UP BEFORE THE ORDER IS GIVEN, or this gate measures a RACE
    // instead of a wake. A driver subscribes with `since: 0`, so its first delivered batch is the
    // journal's HISTORY — here a `layout_updated` from the pane this test spawned. Measured: that
    // stale batch can flush AFTER the cancel, wake the watcher, and have it re-read the row and
    // find the order — so the cancel lands with nothing having announced it, and withholding
    // `Event::RunOrdered` left this gate green. Waiting until the catch-up is spent makes the
    // order's own announce the ONLY traffic that can wake the watcher, which is the claim.
    assert!(
        wait_until(Duration::from_secs(6), || {
            run_row(&mut conn, run)["state"]["iterations"]
                .as_u64()
                .unwrap_or(0)
                >= 3
        }),
        "⚠⚠ the run never took three steps, so the catch-up window this gate needs was not spent \
         and the order below would race it. Read {:?}",
        run_row(&mut conn, run),
    );

    // ── THE ORDER, at the door a person uses ─────────────────────────────────────────────────
    conn.call(
        "scene/invoke",
        json!({
            "path": sprag_host::plugins_path(sprag_host::plugins::CANCEL_ACTION),
            "args": { "id": run },
        }),
    )
    .expect("a person cancels the run");

    // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────────
    let stopped = wait_until(Duration::from_secs(10), || {
        run_row(&mut conn, run)["state"]["status"] == json!("done")
    });
    let row = run_row(&mut conn, run);
    assert!(
        stopped,
        "⛔⛔⛔⛔ REGISTER ITEM 648: a person cancelled a run driven from another process and the \
         driver did not hear it. The order reached this daemon's registry — that half was paid — \
         but the JOURNAL is how it crosses to a driver that is not in this process, and a driver \
         that cannot be woken has to look instead. Read {row:?}",
    );
    assert_eq!(
        row["state"]["outcome"]["state"],
        json!("cancelled"),
        "⚠⚠⚠ the run ended, but not as CANCELLED — so something other than the person's order \
         stopped it and this gate would be green for the wrong reason. Read {row:?}",
    );
}

/// **A REMOTE DRIVER GETS THE SAME PAINT ANSWER AN IN-PROCESS ONE DOES** — register item 555, the
/// last of item 544 stage 1c's residues.
///
/// # ⚠⚠⚠⚠⚠ Why the absence here is a FALSE ANSWER and not a degradation
///
/// [`sprag_plugin::has_painted`] reads a row's damage generation, and `RemotePaneAccess::pane_rows`
/// serves a generation of ZERO on every row — a published decision, because a damage generation is
/// a PAINT signal that a resize or an OSC palette change stamps while no program writes a byte. So
/// the number is right to withhold and the QUESTION still has to be answerable: over this surface
/// `has_painted` returns **false for every pane in the world**, including one whose child has
/// printed and exited. Nobody is told — it is a value the caller reads, not an error it handles —
/// so a driver outside this process concludes *that pane has never painted* about a peer that is
/// plainly up.
///
/// # ⚠⚠⚠⚠ The fixture makes the two sources DISAGREE, which is the only way to measure which ran
///
/// Register item 684's rule, applied here: a fixture whose two sources happen to agree cannot say
/// which one was consulted. A pane that has painted has rows with a real generation IN THE DAEMON
/// and rows with a generation of zero ON THIS WIRE, so the painted arm stays red until the answer
/// travels as its own fact instead of being recomputed from a number that did not travel.
///
/// ⚠⚠ **AND THE UNPAINTED ARM IS THE CONTROL**, without which *always answer true* passes: a pane
/// running `cat` paints nothing until something is typed at it, and `has_painted`'s own doc names
/// that as the peer a caller must not conclude is dead.
#[test]
fn a_remote_driver_is_told_which_panes_have_painted() {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    let _host = spawn_host_at(&sock, &["sh"]);
    let (remote, mut setup) = remote_driver(&sock);

    // THE PAINTED ONE: a child that prints and exits, so the pane has certainly produced output and
    // `pane_eof` gives this test a race-free moment to ask after.
    let painted = spawn_pane(
        &mut setup,
        json!({ "cmd": ["sh", "-c", "printf 'up\\n'"], "cols": 40, "rows": 6 }),
    );
    // THE UNPAINTED ONE: `cat` opens its terminal and writes nothing until it is written to.
    let quiet = spawn_pane(&mut setup, json!({ "cmd": ["cat"], "cols": 40, "rows": 6 }));

    assert!(
        wait_until(Duration::from_secs(20), || remote.pane_eof(painted)
            == Some(true)),
        "⚠⚠ the fixture child never finished, so the claim below would be about a pane that is \
         still filling rather than one that has certainly painted. Read {:?}",
        remote.pane_collapsed(painted),
    );
    // ⚠ THE PREMISE, ASSERTED RATHER THAN ASSUMED. If the daemon never put the child's bytes on
    // that screen then the first arm below is red for a reason that has nothing to do with this
    // item, and the gate would be pointing at the wrong defect.
    assert!(
        remote
            .pane_collapsed(painted)
            .is_some_and(|shown| shown.contains("up")),
        "the painted pane must actually be showing its child's output before anything is claimed \
         about whether that fact reaches a remote driver. Read {:?}",
        remote.pane_collapsed(painted),
    );

    assert!(
        sprag_plugin::has_painted(&remote, painted),
        "⚠⚠⚠ REGISTER ITEM 555: a pane whose child PRINTED AND EXITED reads as never having \
         painted over this surface. An in-process driver answers `true` for this same pane, so the \
         two halves of one product disagree about whether a peer is up — and the remote half is \
         the one that is wrong, silently, for every pane there is.",
    );
    assert!(
        !sprag_plugin::has_painted(&remote, quiet),
        "⚠⚠⚠ and the control: a pane running `cat` has painted NOTHING, so a surface answering \
         `true` here is answering a constant rather than the question. `has_painted`'s own doc \
         names this peer as the one a caller must not conclude is dead.",
    );
}
