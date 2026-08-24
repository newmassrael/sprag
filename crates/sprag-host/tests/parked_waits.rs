//! **WHAT A REMOTE DRIVER LEAVES PARKED IN THE DAEMON, COUNTED IN THE DAEMON THAT HOLDS IT** —
//! register item 642.
//!
//! # ⚠⚠⚠⚠⚠ Why this file exists at all, and why the register said it could not
//!
//! [`RevisionChannel::parked_count`](sprag_host::notify::RevisionChannel::parked_count) carries a
//! bound that was only ever an ARGUMENT: *one entry per pane the connection has waited on*. It
//! rests on the driver abandoning a park when it changes its question, on that park firing the next
//! time its pane moves, and on `release` clearing the rest at close. Nothing had ever walked a
//! driver and read the number back, and the register recorded three reasons why not — the sharpest
//! being that *`sprag_rpc::mount` is the only door that binds a socket and it is a process
//! singleton, so an in-process harness cannot stand one*.
//!
//! **That is false, and it was false when it was written.** `mount`'s `OnceLock` owns the SIGUSR
//! control and nothing else; the bind itself is `pinion_rpc_transport::UnixSocketTransport::serve`,
//! which `sprag-rpc`'s own tests have called directly for as long as they have existed, and
//! `tests/grid_cost.rs` has stood a real [`HostState`] in a test process for just as long. This
//! file is the two of them in one place. What it skips is `mount`'s exposure policy and its signal
//! control — neither of which a parked wait can see — and what it keeps is every part that can:
//! the daemon's own [`FrameIngress`], its own dispatch owner, its own `HostState`, and the real
//! [`RemotePaneAccess`] as the driver.
//!
//! ⚠⚠ That matters because the alternative was register item 617's trap. Issuing the frames by
//! hand reaches the number and re-implements the driver's question-changing rule — and measures a
//! shape the driver cannot produce (two identical parks in a row, which the driver's own memory
//! makes unspellable). Here the driver IS the product, so the walk is the product's walk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use pinion_rpc_transport::UnixSocketTransport;
use serde_json::json;
use sprag_host::remote_access::RemotePaneAccess;
use sprag_host::wire::{SESSION_SLOT, SPAWN_ACTION, TEXT_ACTION};
use sprag_host::{
    ChannelRegistry, FrameIngress, Host, HostState, dispatch_channel, dispatch_frames,
    mux_action_path, pane_input_path,
};
use sprag_plugin::PaneChanges;
use sprag_rpc::HostConn;
use sprag_terminal::PaneId;

/// Panes the walk alternates between. THREE rather than one: a single-pane wait re-asks the same
/// question and the driver's memory answers it without touching the wire, so one pane could never
/// show an abandoned park at all.
const PANES: usize = 3;

/// Laps of the walk. MORE THAN ONE, deliberately: one lap parks once per pane and cannot tell *one
/// per pane* from *one per call*. The second lap is what makes the two numbers differ.
const LAPS: usize = 3;

/// The slice each wait gives the daemon — the same shape `park_until` uses, small because every
/// call here is expected to time out.
const SLICE: Duration = Duration::from_millis(60);

/// **THE DAEMON, IN THIS PROCESS** — a real [`HostState`], the host's own [`FrameIngress`], the
/// host's own dispatch owner, and pinion's own Unix transport, bound at a path of this test's own.
///
/// Returns the [`ChannelRegistry`] the daemon was BUILT WITH, which is how the count is reachable:
/// the registry is an argument to `HostState::new`, so a clone of the same `Arc` answers from out
/// here about the very channels the dispatch thread is parking into.
fn daemon_in_this_process(path: &Path) -> Arc<ChannelRegistry> {
    let channels = Arc::new(ChannelRegistry::default());
    let theirs = Arc::clone(&channels);
    let mounted = path.to_path_buf();
    let (ready, is_ready) = mpsc::channel();
    std::thread::Builder::new()
        .name("parked-waits-daemon".to_owned())
        .spawn(move || {
            let host = Host::new((40, 6));
            let state = HostState::new(host, theirs, None);
            // Through `dispatch_channel`, not a bare `mpsc::channel`, for the reason `sprag-term`
            // gives at its own call site: the output signal is wired into `state` BY CONSTRUCTION.
            let (tx, rx) = dispatch_channel(&state);
            let control = UnixSocketTransport::serve(&mounted, Arc::new(FrameIngress::new(tx)))
                .expect("bind this test's own socket");
            control.set_enabled(true);
            ready
                .send(())
                .expect("the test is still waiting for its daemon");
            // The accept threads hold the ingress (and so the sender), which is what keeps this
            // owner alive; it ends when the test binary does.
            dispatch_frames(&state, rx);
            drop(control);
        })
        .expect("spawn the in-process daemon");
    is_ready
        .recv_timeout(Duration::from_secs(10))
        .expect("the in-process daemon bound its socket");
    channels
}

/// Ask the daemon for a pane, over the wire — the same door `wire_client`'s tests spawn through, so
/// the panes this walk waits on are panes the daemon made rather than ones the test reached in and
/// planted.
///
/// ⚠⚠ **`cat` WITH NOTHING ON ITS STDIN, and that is the fixture's whole premise**: it prints
/// nothing, so the pane's revision stands still for the walk. A pane that never moves again is the
/// ONLY case the bound is about — an abandoned park at a pane that still moves is answered and
/// cleared by that pane's own next move, which is exactly why the leak went unnoticed. The premise
/// is not left to trust: every call in the walk asserts the pane did not move.
fn spawn_pane(conn: &mut HostConn, name: &str) -> PaneId {
    PaneId(
        conn.call(
            "scene/invoke",
            json!({
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["/bin/sh", "-c", "exec cat"], "name": name, "cols": 40, "rows": 6 },
            }),
        )
        .expect("spawn a pane over the socket")
        .as_u64()
        .expect("spawn returns the new pane id"),
    )
}

/// ⛔⛔⛔⛔ **A DRIVER THAT CHANGES ITS QUESTION MUST NOT LEAVE A PARK BEHIND PER CALL** — register
/// item 642, and the walk its own number's documentation says is owed.
///
/// # ⚠⚠⚠ The control comes first, because the claim is an upper bound
///
/// Every assertion below is *the count is no bigger than N*, and the cheapest way to satisfy one of
/// those is to park nothing at all — a driver whose socket died, an address the daemon does not
/// serve, a `world_changed` latch. So the first thing asserted is that the walk really does park,
/// and the second is that the panes really did not move (a park answered by its own pane's movement
/// is cleared for a reason that has nothing to do with this bound). Only then is the bound worth
/// reading.
#[test]
fn a_driver_that_walks_three_panes_leaves_at_most_one_park_per_pane() {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "sprag-parked-waits-{}-{:?}.sock",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_file(&path);
    let channels = daemon_in_this_process(&path);

    let mut setup =
        HostConn::connect(&path, Duration::from_secs(5)).expect("the test's own connection");
    let session = setup
        .call(
            "scene/query",
            json!({ "path": mux_action_path(SESSION_SLOT) }),
        )
        .expect("the daemon says which session this is about")
        .as_str()
        .expect("a session name is a string")
        .to_owned();
    let panes: Vec<PaneId> = (0..PANES)
        .map(|index| spawn_pane(&mut setup, &format!("cat{index}")))
        .collect();

    // The driver, exactly as `crate::drive` builds it: its own reading connection and its own park
    // connection, both unscoped and both reaching this daemon.
    let driving =
        HostConn::connect(&path, Duration::from_secs(5)).expect("the driver's connection");
    let parking =
        HostConn::connect(&path, Duration::from_secs(5)).expect("the driver's park socket");
    let driver = RemotePaneAccess::over(driving)
        .parking_on(parking)
        .expect("two connections to one daemon, both unscoped, resolve to one session");

    let seen: Vec<u64> = panes
        .iter()
        .map(|pane| {
            driver
                .pane_revision(*pane)
                .expect("the daemon serves this pane's revision")
        })
        .collect();

    let parked = || channels.revisions(&session).parked_count();

    // ── THE CONTROL: the walk really reaches the daemon and really parks ──────────────────────
    assert_eq!(
        driver.pane_moved_after(panes[0], seen[0], SLICE),
        Some(seen[0]),
        "⚠⚠ the first wait did not time out with the pane where it was, so either the pane moved \
         or the park never happened — and every bound below would then be about a walk that did \
         not take place",
    );
    assert_eq!(
        parked(),
        1,
        "⚠⚠⚠ THE CONTROL: one slice of one wait must leave exactly one park in this daemon. A \
         zero here passes every upper bound below while measuring nothing at all, which is the \
         shape a driver with a dead socket, an unserved address or a tripped `world_changed` latch \
         would produce.",
    );

    // ── THE WALK: three panes, three laps, every call changing the question ───────────────────
    for lap in 0..LAPS {
        for (index, pane) in panes.iter().enumerate() {
            assert_eq!(
                driver.pane_moved_after(*pane, seen[index], SLICE),
                Some(seen[index]),
                "⚠⚠⚠ PREMISE: pane {index} moved during lap {lap}, so its park was answered by its \
                 own movement rather than abandoned. This walk is only about panes that never move \
                 again — a `cat` with nothing on its stdin — so a moving one means the fixture is \
                 no longer making the case the bound is about.",
            );
        }
    }

    // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────────
    //
    // ⚠⚠ EXACTLY, not «at most». An upper bound alone cannot tell the fix from an over-correction:
    // a `park` that dropped every wait this CONNECTION holds — rather than the one it holds for
    // THIS PANE — would answer 1 here and pass any `<=`, while having thrown away a question a
    // pipelining client could still be waiting on. The pair of numbers is the claim.
    assert_eq!(
        parked(),
        PANES,
        "⛔⛔⛔⛔ REGISTER ITEM 642: this daemon holds {} parked revision waits after a driver \
         walked {PANES} panes over {LAPS} laps. TOO MANY means the leak is back — a driver \
         alternating panes abandons a park at every call, an abandoned park is released only when \
         its pane MOVES or its connection CLOSES, so a relay between panes that have gone quiet \
         accumulates one daemon-side entry per step for as long as it runs (measured at NINE \
         before `park` learned to replace). TOO FEW means the replacement reaches past the pane it \
         was asked about and is dropping questions somebody may still want answered.",
        parked(),
    );

    // ── AND THE PARK THAT REPLACED ONE STILL WAKES, WHICH NO COUNT CAN SAY ────────────────────
    //
    // ⚠⚠⚠⚠⚠ THE ORDER HERE IS THE WHOLE ASSERTION, and the first draft had it backwards. Writing
    // into the pane FIRST and asking afterwards proves nothing about waking: a wait parked at a
    // pane that has ALREADY moved is answered by the pass that runs at the park site, so the
    // wake path — the revision channel's armed edge, which is what fires the signal at all — is
    // never asked. Measured: a `park` that published `parked_any` as FALSE, disarming every
    // revision wake in the daemon, passed that draft.
    //
    // So the pane moves while the wait is OUTSTANDING. The first call below parks pane 0 afresh
    // (replacing the park the walk left — the very act under test) and times out; the second
    // RESUMES that same outstanding request without putting a byte on the wire, so the only thing
    // that can answer it is the daemon deciding, on its own, that this pane moved.
    assert_eq!(
        driver.pane_moved_after(panes[0], seen[0], SLICE),
        Some(seen[0]),
        "the replacing park must be outstanding before the pane is made to move",
    );
    assert_eq!(
        parked(),
        PANES,
        "⚠⚠ re-parking pane 0 must REPLACE its own wait rather than add one — the count is the \
         same claim as above, re-read here because everything below rests on this park being the \
         only one pane 0 has",
    );
    setup
        .call(
            "scene/invoke",
            json!({
                "path": pane_input_path(panes[0].0, TEXT_ACTION),
                "args": { "text": "parked_waits_marker\n" },
            }),
        )
        .expect("write into the first pane over the wire");
    let moved = driver
        .pane_moved_after(panes[0], seen[0], Duration::from_secs(10))
        .expect("the park answers rather than the surface degrading");
    assert!(
        moved > seen[0],
        "⛔⛔⛔ THE PARK THAT REPLACED ONE NEVER WOKE: it answered {moved} against the revision \
         {} it was parked past, which is this surface's word for «the slice elapsed and nothing \
         happened» — and the pane demonstrably moved. A replacement that leaves a DEAD entry \
         behind counts exactly one per pane and wakes nobody, so the count above would report the \
         repair while every wait in the daemon had stopped working.",
        seen[0],
    );

    let _ = std::fs::remove_file(&path);
}
