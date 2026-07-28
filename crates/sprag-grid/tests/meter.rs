//! The projection meter counts every projection exactly once.
//!
//! ## Why this is an integration test and not a unit one
//!
//! [`sprag_grid::work`] reads PROCESS-WIDE counters, so a delta assertion is only sound when
//! nothing else in the process is projecting at the same time. The crate's unit module runs a
//! dozen projection tests on the default parallel harness, so a delta taken there would be
//! measuring whichever tests happened to be in flight — green on a quiet machine, red on a busy
//! one, and meaningless either way. A test binary of its own, whose only projections are these,
//! is what makes an exact number assertable; the mutex serialises the two tests inside it.
//!
//! What the exactness is FOR: the whole point of the meter is that the cost cannot be paid without
//! being counted, and the one way it could lie is by counting the same projection twice —
//! `project_scrolled` delegates to `project` for the live view and for a stale offset, so a naive
//! count at its entry would report double for the commonest call in the system.

use std::sync::Mutex;

use sprag_grid::{project, project_scrolled, work};
use sprag_vt::{Emulator, Palette, Screen, VtPort};

/// Serialises the tests in this binary, so each one's delta is its own.
static SERIAL: Mutex<()> = Mutex::new(());

fn screen_from(bytes: &[u8], cols: u16, rows: u16) -> Screen {
    let mut emulator = Emulator::new(cols, rows);
    emulator.advance(bytes);
    emulator.screen().clone()
}

#[test]
fn a_projection_is_metered_once_and_by_area() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let screen = screen_from(b"hi", 10, 4);
    let before = work();
    let _ = project(&screen, &Palette::xterm_default());
    let after = work();
    assert_eq!(
        after.projections_total - before.projections_total,
        1,
        "one call, one projection"
    );
    assert_eq!(
        after.cells_total - before.cells_total,
        40,
        "a 10x4 screen is 40 cells, whatever it was asked to show"
    );
}

#[test]
fn the_live_view_is_not_metered_twice_through_the_delegation() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let screen = screen_from(b"hi", 10, 4);
    let palette = Palette::xterm_default();

    // Offset 0 delegates to `project`, and so does a positive offset against empty scrollback.
    // Both are the LIVE view, and both must cost exactly what the live view costs.
    let before = work();
    let _ = project_scrolled(&screen, 0, &palette);
    let live = work();
    let _ = project_scrolled(&screen, 99, &palette);
    let stale = work();

    assert_eq!(
        live.projections_total - before.projections_total,
        1,
        "the live view is one projection, not the delegation plus the delegate"
    );
    assert_eq!(live.cells_total - before.cells_total, 40);
    assert_eq!(
        stale.projections_total - live.projections_total,
        1,
        "an offset past an empty scrollback is the live view, and costs the same"
    );
    assert_eq!(stale.cells_total - live.cells_total, 40);
}
