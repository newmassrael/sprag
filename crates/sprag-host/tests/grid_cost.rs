//! What a request costs the cell-grid projection, counted exactly.
//!
//! R217 measured that ANY daemon request re-projected every pane's whole grid — a read of one
//! integer (`scene/revision`) cost the same whole-screen walk per pane that a snapshot did. The
//! projection gate (`rpc::pane_cells_for`) is the answer; this is the claim it has to keep.
//!
//! ## Why an integration binary, and why the numbers can be exact here
//!
//! [`sprag_grid::work`] reads PROCESS-WIDE counters, so a delta is only sound when nothing else
//! in the process projects at the same time. That rules out the crate's parallel unit harness and
//! buys, in exchange, something the live smoke could never have: the meter is read IN-PROCESS
//! rather than over the socket, so reading it is not itself a request and perturbs nothing. R217
//! had to argue its numbers by divisibility because its instrument cost a pane set to read; here
//! the counts are simply exact, and the sizes below are asymmetric anyway so a wrong attribution
//! could not hide behind a coincidence.
//!
//! Every pane runs `cat` on a PTY nothing is ever written to, so no child output can move a
//! counter between the reads.

use std::sync::Arc;

use sprag_host::{ChannelRegistry, Host, HostState, handle_request};
use sprag_terminal::CommandBuilder;

/// Three panes of DELIBERATELY different areas, so a count attributes to a pane set and not
/// merely to "some multiple of a pane".
const PANES: [(u16, u16); 3] = [(40, 6), (30, 5), (20, 4)];

/// Cells in one whole pane set — what a snapshot must still cost.
const SET_CELLS: u64 = 40 * 6 + 30 * 5 + 20 * 4;

/// Cells in pane 0, the one the `cells.0` reads below address.
const PANE0_CELLS: u64 = 40 * 6;

/// How many times each read is repeated, so a per-request cost is a slope and not one sample.
const READS: u64 = 8;

/// A pane that blocks on its PTY and never writes: nothing it does can move a counter.
fn quiescent() -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg("exec cat");
    command.env("TERM", "dumb");
    command
}

fn serve(state: &HostState, request: &str) {
    let response = handle_request(state, request).expect("a response");
    assert!(
        !response.contains("\"error\""),
        "request failed: {request} -> {response}",
    );
}

/// A request pays for the panes it can actually READ, and for nothing else.
///
/// One test, three phases, because the counters are process-wide: a second `#[test]` in this
/// binary would need a mutex to keep its delta its own, and the phases here are one claim.
#[test]
fn a_request_projects_only_when_its_method_can_read_a_grid() {
    let channels = Arc::new(ChannelRegistry::default());
    let host = Host::new(PANES[0]);
    for (index, (cols, rows)) in PANES.iter().enumerate() {
        host.spawn(
            quiescent(),
            format!("cat{index}"),
            *cols,
            *rows,
            None,
            None,
            None,
        )
        .expect("spawn a quiescent pane");
    }
    let state = HostState::new(host, channels, None);

    // Phase 1 — the read that reports one integer and walks no node. Was one whole pane set
    // per call (R217 measured +3 per call on three panes); must now be free.
    let before = sprag_grid::work();
    for _ in 0..READS {
        serve(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/revision","params":{}}"#,
        );
    }
    let after = sprag_grid::work();
    assert_eq!(
        after.projections_total - before.projections_total,
        0,
        "a revision read must project nothing at all",
    );
    assert_eq!(after.cells_total - before.cells_total, 0);

    // Phase 2 — the client's steady-state cell fetch. The ONE projection it costs is the one
    // that produces the reply (`SpragPaneExternal::frame_at`); the assembly adds none. The
    // cell total names WHICH pane, so this cannot pass by projecting some other pane instead.
    let before = sprag_grid::work();
    for _ in 0..READS {
        serve(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/cells.0"}}"#,
        );
    }
    let after = sprag_grid::work();
    assert_eq!(
        after.projections_total - before.projections_total,
        READS,
        "a cells read projects exactly the pane it asked for, once",
    );
    assert_eq!(
        after.cells_total - before.cells_total,
        READS * PANE0_CELLS,
        "and those projections are pane 0's area, not the pane set's",
    );

    // Phase 3 — non-vacuity. The method that genuinely reads every grid must still pay for
    // every grid, or phases 1 and 2 would also pass with the projection removed outright.
    let before = sprag_grid::work();
    serve(
        &state,
        r#"{"jsonrpc":"2.0","id":3,"method":"scene/snapshot","params":{"path":""}}"#,
    );
    let after = sprag_grid::work();
    assert_eq!(
        after.projections_total - before.projections_total,
        PANES.len() as u64,
        "a snapshot reads every pane, so it projects every pane",
    );
    assert_eq!(
        after.cells_total - before.cells_total,
        SET_CELLS,
        "one whole pane set, exactly",
    );
}
