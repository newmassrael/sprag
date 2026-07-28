//! What a projection ALLOCATES — counted, because the wall clock cannot answer it.
//!
//! ## Why allocations and not time
//!
//! sprag has measured this exact question before. The round that repaid the emulator's throughput
//! debt found wall-clock drifting more than 20% run to run on this machine — even for an
//! allocation-free control — and switched to counting the global allocator, which is reproducible,
//! is unaffected by what else the box is doing, and answers the question actually being asked.
//! This binary is that instrument, kept rather than thrown away, so the claim has a permanent
//! guard instead of a remembered number.
//!
//! ## The claim, and why it is stated as an EQUALITY
//!
//! A projection allocates once per ROW (the row's cell vector) and a fixed handful for the buffer
//! itself. It must not allocate once per CELL. Stated as a threshold that would be a guess; stated
//! as "two screens of the same height and very different widths allocate the SAME number of
//! times", it is exact, needs no tuned constant, and fails loudly the moment a per-cell allocation
//! returns — the wide screen simply costs four times the narrow one.
//!
//! A `#[global_allocator]` is binary-wide, so this file holds ONE test: a second would have its
//! count polluted by whatever the first was doing on another harness thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use sprag_grid::project;
use sprag_vt::{Emulator, Palette, Screen, VtPort};

/// Allocation events since the process started. A `realloc` counts too: the default
/// [`GlobalAlloc`] forwarding routes it through [`Counting::alloc`], and a reallocation IS an
/// allocation event for the purpose of this claim.
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

/// A screen of `cols x ROWS` filled with `fill`, repeated to the end of every row. Built OUTSIDE
/// any measured window — the emulator's own allocations are not what this prices.
fn filled(cols: u16, fill: &str) -> Screen {
    let mut emulator = Emulator::new(cols, ROWS);
    let mut line = String::new();
    while line.chars().count() < usize::from(cols) {
        line.push_str(fill);
    }
    for row in 0..ROWS {
        emulator.advance(line.as_bytes());
        if row + 1 < ROWS {
            emulator.advance(b"\r\n");
        }
    }
    emulator.screen().clone()
}

#[test]
fn a_projection_allocates_per_row_and_not_per_cell() {
    let palette = Palette::xterm_default();

    // Printable ASCII, which is what a terminal overwhelmingly holds. Four times the cells...
    let narrow = filled(40, "abc 123 xyz ");
    let wide = filled(160, "abc 123 xyz ");
    // ...and each is projected once first, so a lazily-initialised anything is warm and cannot be
    // charged to whichever screen happened to go first.
    let _ = project(&narrow, &palette);
    let _ = project(&wide, &palette);

    let narrow_allocations = allocations(|| {
        let _ = project(&narrow, &palette);
    });
    let wide_allocations = allocations(|| {
        let _ = project(&wide, &palette);
    });

    assert_eq!(
        narrow_allocations, wide_allocations,
        "four times the cells must cost the same number of allocations \
         ({narrow_allocations} for 40 cols vs {wide_allocations} for 160)",
    );
    // And the absolute scale is the rows, not some small multiple of the cells: the buffer plus
    // one vector per row. Generous enough not to pin an implementation detail, tight enough that
    // even the NARROW screen's 960 cells could not hide inside it.
    assert!(
        narrow_allocations <= u64::from(ROWS) + 8,
        "a projection should cost about one allocation per row, got {narrow_allocations}",
    );

    // The half that reaches PAST this crate. A display client does not project — it clones a
    // buffer it already holds, per pane, per painted frame (`WireHost::live_cells`). A borrowed
    // cluster clones by copying a pointer, so that copy is now two allocations (the cell vector
    // and the row generations) whatever the screen's width.
    let narrow_buffer = project(&narrow, &palette);
    let wide_buffer = project(&wide, &palette);
    let narrow_clone = allocations(|| {
        let _ = narrow_buffer.clone();
    });
    let wide_clone = allocations(|| {
        let _ = wide_buffer.clone();
    });
    assert_eq!(
        narrow_clone, wide_clone,
        "cloning a projected buffer must not scale with its cells \
         ({narrow_clone} for 40 cols vs {wide_clone} for 160)",
    );
    assert!(
        narrow_clone <= 4,
        "a clone should be the buffer's own vectors and nothing per cell, got {narrow_clone}",
    );

    // The boundary, which is also the non-vacuity: a cluster this crate cannot name still owns its
    // string, so a screen of wide CJK glyphs pays per cell again. Without this the equality above
    // would read the same on a counter that had simply stopped.
    let cjk = filled(40, "世界");
    let _ = project(&cjk, &palette);
    let cjk_allocations = allocations(|| {
        let _ = project(&cjk, &palette);
    });
    assert!(
        cjk_allocations > narrow_allocations,
        "an unnameable cluster still allocates, so the counter is alive \
         ({cjk_allocations} for CJK vs {narrow_allocations} for ASCII)",
    );
}
