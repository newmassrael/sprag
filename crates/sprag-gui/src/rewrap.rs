//! Re-wrap a pane the daemon sized wider than this client's tile — the GUI's half of R349.
//!
//! ## Why a pixel client has the same problem as a cell one
//!
//! A pane's size is the DAEMON's: `tile(tree, window)` over a window folded from every attached
//! client's report, so a client that lost the arbitration is drawing a pane with more columns than
//! its own tile has room for. `reflow.rs`'s `owns_its_own_size` is where that is decided, and it is
//! right — a shared pane's winsize is not this client's to take. What is left is the same residue
//! `sprag-tui` measured: the columns past the tile's edge are simply not on screen, and there is no
//! gesture that reaches them.
//!
//! The GUI's version is NOT the same in one way that matters, and it is why this is three lines
//! rather than a front: `sprag-tui` needed a VIEWPORT first (R348) because its window is a
//! character grid it must place panes into by hand. pinion measures each pane's tile and clips it,
//! so the "which part of the window am I showing" question never arises here — only "which part of
//! this PANE", which is exactly what [`sprag_grid::rewrap`] answers.
//!
//! ## What it does not do
//!
//! Nothing on the alternate screen (`rewrap` refuses it), nothing when the tile is wide enough,
//! and nothing to the pane: the pty is untouched, so the other clients and the child see no change.

use pinion_core::{CellMetric, GridBuffer};

use crate::terminal::{TerminalView, grid_dims, pane_tag};

/// Re-wrap slot `i`'s cells into the columns its tile can show, or hand them back unchanged.
///
/// The measured rect is read through pinion's per-pane viewport signal, which SUBSCRIBES the
/// paint: a window resize re-divides the tiles and this repaints through here, the same way the
/// hover and selection overlays beside it do.
///
/// Unchanged is the answer for every case the caller need not distinguish — an unmeasured tile at
/// boot, a tile wide enough for the pane, the alternate screen, and a host that cannot say where
/// the lines end. `sprag_grid::rewrap` owns the last three.
pub(crate) fn for_tile(tv: &TerminalView, i: usize, cells: GridBuffer) -> GridBuffer {
    into_tile(
        pinion_core::use_pane_viewport_size(pane_tag(i)),
        tv.metric,
        &tv.slots.pane_scroll_facts(i).shares,
        cells,
    )
}

/// [`for_tile`]'s decision, with the measured rect as a PARAMETER — the whole of it except the
/// tracked read.
///
/// Split out because pinion publishes a pane's measured size through a `pub(crate)` registry, so
/// nothing downstream can set one: a test of [`for_tile`] could only ever reach the unmeasured
/// arm. Narrowing the role is the recorded answer to a dependency a fixture cannot fake (R326,
/// R334) — what is left above is one tracked read, and what is testable is everything that decides.
fn into_tile(
    measured: (u32, u32),
    metric: CellMetric,
    shares: &sprag_grid::RowShares,
    cells: GridBuffer,
) -> GridBuffer {
    if measured.0 == 0 || measured.1 == 0 {
        return cells; // unmeasured at boot — the pre-layout sentinel
    }
    let (cols, rows) = grid_dims(measured, metric);
    sprag_grid::rewrap(&cells, shares, cols, rows).map_or(cells, |cut| cut.cells)
}

#[cfg(test)]
mod tests {
    use pinion_core::CellMetric;
    use sprag_vt::{Emulator, Palette, VtPort};

    use super::*;

    /// A projected pane of `cols` columns holding one line that overruns them, with the shares
    /// that describe it — the pair a client always holds together.
    fn pane(cols: u16) -> (GridBuffer, sprag_grid::RowShares) {
        let mut em = Emulator::new(cols, 6);
        em.advance(format!("START{}END", "-".repeat(usize::from(cols) - 10)).as_bytes());
        let screen = em.screen().clone();
        (
            sprag_grid::project(&screen, &Palette::xterm_default()),
            sprag_grid::shares(&screen, 0),
        )
    }

    /// The pixels a tile of `cols` x `rows` cells occupies at the default metric — the inverse of
    /// the derivation under test, so the fixture cannot agree with it by accident.
    fn tile_px(cols: u32, rows: u32) -> (u32, u32) {
        let metric = CellMetric::DEFAULT;
        (cols * metric.cell_w(), rows * metric.cell_h())
    }

    /// **A PANE WIDER THAN THIS CLIENT'S TILE IS RE-WRAPPED INTO IT** — the GUI's half of R349,
    /// and the answer to whether it had the same defect: it did.
    ///
    /// REVERT-PROOF: return `cells` unchanged and the buffer comes back 100 columns wide, of which
    /// the tile shows 50 and the person can reach none of the rest.
    #[test]
    fn a_pane_wider_than_the_tile_is_re_wrapped_into_the_columns_it_can_show() {
        let (cells, shares) = pane(100);
        let cut = into_tile(tile_px(50, 6), CellMetric::DEFAULT, &shares, cells.clone());
        assert_eq!(cut.cols(), 50, "the tile's columns, not the pane's");
        let row = |r: u16| {
            (0..cut.cols())
                .map(|c| {
                    cut.cell(c, r)
                        .map_or(' ', |x| x.cluster.chars().next().unwrap_or(' '))
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        assert!(row(0).starts_with("START"), "row 0 is {:?}", row(0));
        assert!(
            row(1).ends_with("END"),
            "and the line's second half is on its second row: {:?}",
            row(1),
        );
    }

    /// The three ways nothing happens, which a caller must not have to tell apart: a tile wide
    /// enough, a tile not yet measured, and a host that cannot say where the lines end.
    #[test]
    fn a_tile_that_fits_an_unmeasured_one_and_a_host_that_cannot_say_all_leave_the_pane_alone() {
        let (cells, shares) = pane(100);
        let metric = CellMetric::DEFAULT;
        assert_eq!(
            into_tile(tile_px(120, 6), metric, &shares, cells.clone()).cols(),
            100,
            "a tile wider than the pane",
        );
        assert_eq!(
            into_tile((0, 0), metric, &shares, cells.clone()).cols(),
            100,
            "the pre-layout sentinel",
        );
        assert_eq!(
            into_tile(
                tile_px(50, 6),
                metric,
                &sprag_grid::RowShares::default(),
                cells,
            )
            .cols(),
            100,
            "a host that said nothing must not have its rows cut at the grid's width",
        );
    }
}
