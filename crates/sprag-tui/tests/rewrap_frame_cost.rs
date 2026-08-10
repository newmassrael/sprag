//! What a RE-WRAPPED pane costs this client per frame — the measurement R349's registration owed.
//!
//! A re-wrapped pane is given no `ProjectionToken`, so `PaintCache` cannot vouch for any of its
//! rows and rebuilds the whole change list on every frame. Registering that as "unmeasured" is the
//! shape this project has been wrong about twenty times, so it is measured here instead: the three
//! pieces of one frame for one pane, in allocations, which is the instrument R344 established
//! because wall-clock on this box drifts more than 20% run to run.
//!
//! **The measurement refuted the guess that prompted it.** "Rebuilt every frame" was registered as
//! roughly a doubling — the re-wrap plus a change list of the same order. Driven, on a 100x24 pane
//! at 50 columns: **the re-wrap is 80 allocations and the change list is 1350**, seventeen times
//! it. So the token the cache needs is not a tidiness item that saves half a frame; it removes
//! ~94% of what a re-wrapped pane costs, on the client least able to pay it. That is why one is
//! derived (`rewrapped_token`) rather than the pane being marked "cannot say".
//!
//! The assertion is therefore about the SHAPE this conclusion rests on — the change list dominates
//! — rather than a tuned number, and both numbers are printed so the next round reads them.
//!
//! Its own binary because `#[global_allocator]` is binary-wide; a second test on another harness
//! thread would pollute the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use sprag_grid::{project, rewrap, shares};
use sprag_tui::{Rect, pane_changes};
use sprag_vt::{Emulator, Palette, Screen, VtPort};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

struct Counting;

// SAFETY: every method forwards to `System`, which upholds the `GlobalAlloc` contract; the counter
// is a relaxed atomic add that touches no allocator state.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System.alloc` with this same `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const ROWS: u16 = 24;

fn allocations(body: impl FnOnce()) -> u64 {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    body();
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

/// A `cols x ROWS` screen full of printable ASCII, built outside any measured window.
fn filled(cols: u16) -> Screen {
    let mut emulator = Emulator::new(cols, ROWS);
    let mut line = String::new();
    while line.chars().count() < usize::from(cols) {
        line.push_str("abc 123 xyz ");
    }
    line.truncate(usize::from(cols));
    for row in 0..ROWS {
        emulator.advance(line.as_bytes());
        if row + 1 < ROWS {
            emulator.advance(b"\r\n");
        }
    }
    emulator.screen().clone()
}

#[test]
fn a_re_wrapped_pane_pays_the_re_wrap_and_the_change_list_on_every_frame() {
    let palette = Palette::xterm_default();
    let screen = filled(100);
    let (cells, cuts) = (project(&screen, &palette), shares(&screen, 0));
    let area = Rect::screen(50, ROWS);

    // Warm both paths so a lazily-initialised anything is charged to neither.
    let warm = rewrap(&cells, &cuts, 50, ROWS).expect("re-wraps").cells;
    let _ = pane_changes(&warm, area, (0, 0));

    let re_wrap = allocations(|| {
        let _ = rewrap(&cells, &cuts, 50, ROWS).expect("re-wraps");
    });
    let cut = rewrap(&cells, &cuts, 50, ROWS).expect("re-wraps").cells;
    let change_list = allocations(|| {
        let _ = pane_changes(&cut, area, (0, 0));
    });

    eprintln!(
        "R349: one frame of a re-wrapped {}x{} pane at {} columns — re-wrap {re_wrap} \
         allocations, change list {change_list}, and a cache HIT would be 0 of both",
        cells.cols(),
        cells.rows(),
        area.cols,
    );
    assert!(
        change_list > re_wrap * 4,
        "the change list ({change_list}) must DOMINATE the re-wrap ({re_wrap}) — that is what \
         makes the derived token worth its own soundness argument. If this ever fails, the \
         conclusion it supports has to be re-taken, not the number relaxed.",
    );
}
