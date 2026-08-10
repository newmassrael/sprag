//! What a client-side RE-WRAP allocates — counted, for the reason `allocs.rs` gives.
//!
//! ## Why this claim needs its own binary and its own gate
//!
//! A `#[global_allocator]` is binary-wide, so the projection's claim and this one cannot share a
//! harness: a second test on another thread pollutes the count. That is why this file exists
//! beside `allocs.rs` rather than inside it.
//!
//! What makes it worth a gate at all is WHO pays. A re-wrapped pane is deliberately given no
//! projection token (see `sprag-tui`'s paint path), so it is rebuilt on every frame — and the
//! client doing the rebuilding is by definition the SMALL one, the phone attached beside a
//! desktop. A per-cell allocation here would land on the least able machine in the session, on
//! every keystroke. Nothing else in this suite would notice.
//!
//! ## The claim, and why it is an EQUALITY
//!
//! Stated as a threshold it would be a guess. Stated as "two buffers with the same rows IN and the
//! same rows OUT allocate the same number of times, whatever their cell count", it is exact and
//! needs no tuned constant: 100 columns cut to 50 and 200 columns cut to 100 both turn 24 rows
//! into 48, with twice the cells in the second. A per-cell allocation makes the second cost twice
//! the first, and the equality fails loudly.
//!
//! REVERT-PROOF: make the join own its cells (`cluster: Cow::Owned(cell.cluster.to_string())`) and
//! the two counts separate by exactly the difference in cells.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use sprag_grid::{project, rewrap, shares};
use sprag_vt::{Emulator, Palette, Screen, VtPort};

/// Allocation events since the process started — see `allocs.rs` for why a `realloc` counts.
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// The system allocator with a counter in front of it.
struct Counting;

// SAFETY: every method forwards to `System`, which upholds the `GlobalAlloc` contract; the
// counter is a relaxed atomic add that touches no allocator state.
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

/// Rows every screen below shares, so width is the only axis that varies.
const ROWS: u16 = 24;

/// Count the allocations `body` performs.
fn allocations(body: impl FnOnce()) -> u64 {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    body();
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

/// A screen of `cols x ROWS` whose every row is full of printable ASCII — built OUTSIDE any
/// measured window, since the emulator's own allocations are not what this prices.
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
fn a_re_wrap_allocates_per_row_and_not_per_cell() {
    let palette = Palette::xterm_default();
    let narrow = filled(100);
    let wide = filled(200);
    let (narrow_cells, narrow_shares) = (project(&narrow, &palette), shares(&narrow, 0));
    let (wide_cells, wide_shares) = (project(&wide, &palette), shares(&wide, 0));

    // Both are re-wrapped once first, so a lazily-initialised anything is warm and cannot be
    // charged to whichever went first.
    let _ = rewrap(&narrow_cells, &narrow_shares, 50, ROWS * 2).expect("100 cols cut to 50");
    let _ = rewrap(&wide_cells, &wide_shares, 100, ROWS * 2).expect("200 cols cut to 100");

    let narrow_allocations = allocations(|| {
        let cut = rewrap(&narrow_cells, &narrow_shares, 50, ROWS * 2).expect("re-wraps");
        assert_eq!(cut.cells.rows(), ROWS * 2, "24 rows of 100 become 48 of 50");
    });
    let wide_allocations = allocations(|| {
        let cut = rewrap(&wide_cells, &wide_shares, 100, ROWS * 2).expect("re-wraps");
        assert_eq!(cut.cells.rows(), ROWS * 2, "and 24 of 200 become 48 of 100");
    });

    assert_eq!(
        narrow_allocations, wide_allocations,
        "twice the cells, the same rows in and out, must cost the same number of allocations \
         ({narrow_allocations} for 100->50 vs {wide_allocations} for 200->100)",
    );
    // And the absolute scale is the ROWS, not some small multiple of the cells: one join buffer
    // per line plus one vector per output row. Generous enough not to pin an implementation
    // detail, tight enough that even the narrow case's 2400 cells could not hide inside it.
    assert!(
        narrow_allocations <= u64::from(ROWS) * 4 + 16,
        "a re-wrap should cost about one allocation per row, got {narrow_allocations}",
    );
}
