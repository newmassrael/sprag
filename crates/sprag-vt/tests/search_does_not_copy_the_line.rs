//! ⚠⚠ **A SEARCH MUST NOT COPY THE LINE IT IS SEARCHING.**
//!
//! # The regression this exists for, and it shipped
//!
//! R344 made the search walk LOGICAL lines, and its first version built each one into a
//! `Vec<Cell>` before scanning it. That is fine for a line that fits on a row and ruinous for the
//! case the round exists for: a program that prints a megabyte with no newline makes ONE logical
//! line out of the whole scrollback, so every keystroke in a find bar memcpy'd every cell in the
//! pane's history. Measured at 200x5000 (release, best of five):
//!
//! | build | one enormous logical line | the same bytes as 5000 lines |
//! |---|---|---|
//! | before R344 | 4.35 ms | 4.27 ms |
//! | R344's first version | **25.9 ms** | **11.1 ms** |
//! | borrowed slices | 3.87 ms | 3.79 ms |
//!
//! Note the second column: the copy made the ORDINARY search three times slower too. **The whole
//! suite was green for it**, because nothing in it asks what a read costs.
//!
//! # Why allocation and not time
//!
//! A timing gate on a shared runner is a flake, and a flake is worse than no gate. What went wrong
//! here was not "slow", it was "copied", and a copy is exactly measurable: this counts the bytes
//! the search asks the allocator for and compares them to the pane's own size. That number does not
//! move with the machine, the load, or the build profile — a debug run and a release run allocate
//! the same.
//!
//! # Why its own test binary
//!
//! `#[global_allocator]` is process-wide: a counting allocator installed for one test would count
//! every other test in the same binary, running concurrently. So the gate has to be a binary of its
//! own, which an integration test is. (It is NOT a `sprag-gate` gate — that crate takes no
//! dependencies by design, and this one needs the emulator.)

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use sprag_vt::{Emulator, VtPort};

/// Bytes handed out since the process started. Monotonic, never decreased on free: the question is
/// how much the search ASKED FOR, and a peak would miss a copy that is promptly dropped.
static HANDED_OUT: AtomicUsize = AtomicUsize::new(0);

/// The system allocator, counting. `realloc` is not overridden deliberately — the default forwards
/// to `alloc`, so a `Vec` growing through its doubling is counted at every step, which is what
/// makes the growth of a copied line visible rather than only its final size.
struct Counting;

// SAFETY: every method forwards to `System`, which is a correct allocator, and the counter is
// atomic. Nothing here allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HANDED_OUT.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Columns per row and rows in the fixture — 100 000 cells, big enough that a per-cell copy dwarfs
/// every fixed cost and small enough to build in a debug test.
const COLS: u16 = 200;
const ROWS: usize = 500;

/// A pane whose whole retained output is ONE logical line: `COLS * ROWS` cells, no newline
/// anywhere, so every row soft-wraps onto the next.
fn one_enormous_logical_line() -> Emulator {
    let mut em = Emulator::with_history_limit(COLS, 24, 10_000);
    let row = "x".repeat(COLS as usize);
    for _ in 0..ROWS {
        em.advance(row.as_bytes());
    }
    em
}

/// What the search may allocate per cell of the line it scans.
///
/// MEASURED, both sides. Borrowing the rows costs **23** bytes per cell: the byte-offset map is one
/// `usize` (8) and the text one byte, each roughly doubled by `Vec`/`String` growth. Copying the
/// line costs **138** — `size_of::<Cell>()` on top, doubling too. The bound sits between them with
/// room for the map to change shape, and the failure message prints the real number so a
/// legitimate change is a one-line edit rather than a mystery.
const BYTES_PER_CELL: usize = 48;

/// ⚠ **ONE TEST, BOTH SEARCHES, AND THAT IS NOT TIDINESS.** The first version of this gate was two
/// tests, and `cargo test` runs the tests in a binary CONCURRENTLY: the counter is global, so each
/// one measured the other's fixture being built. It read 52 bytes per cell against a budget of 48
/// and failed — over borrowed code that copies nothing — while its twin passed the same instant.
/// A shared counter admits no parallel readers; the two measurements are taken in sequence here.
#[test]
fn neither_search_copies_the_line_it_scans() {
    let em = one_enormous_logical_line();
    let screen = em.screen();
    let cells = COLS as usize * ROWS;

    // Non-vacuity: the fixture really is ONE line, not `ROWS` of them. If a change made these rows
    // separate lines the gate below would pass for the wrong reason — the copy it exists to catch
    // is per LINE, so a fixture of short lines cannot express it.
    assert!(
        screen.wrapped(0),
        "the fixture's rows must soft-wrap into one logical line",
    );
    assert_eq!(
        screen.scrollback_len(),
        ROWS - 24,
        "the line must fill the retained region: every row but the visible 24 scrolled off",
    );

    let before = HANDED_OUT.load(Ordering::Relaxed);
    let found = screen.find("a-needle-this-pane-does-not-contain");
    let literal = HANDED_OUT.load(Ordering::Relaxed) - before;
    assert!(found.matches.is_empty(), "the needle is not there");

    // The REGEX search shares the traversal, and would not share a regression if somebody gave it
    // a buffer of its own. Measured after the literal one, never beside it.
    let before = HANDED_OUT.load(Ordering::Relaxed);
    let found = screen
        .find_regex("a-needle-this-pane-does-not-contain")
        .expect("a literal pattern compiles");
    let pattern = HANDED_OUT.load(Ordering::Relaxed) - before;
    assert!(found.matches.is_empty(), "the pattern matches nothing");

    for (which, asked_for) in [("find", literal), ("find_regex", pattern)] {
        assert!(
            asked_for < cells * BYTES_PER_CELL,
            "{which} asked for {asked_for} bytes to scan {cells} cells ({} per cell, budget {}) — \
             it is COPYING the line rather than borrowing its rows, which costs a memcpy of the \
             whole scrollback on every keystroke a find bar types",
            asked_for / cells,
            BYTES_PER_CELL,
        );
    }
}
