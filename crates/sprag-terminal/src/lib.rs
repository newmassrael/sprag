//! sprag-terminal — the **headless** pinion host wiring.
//!
//! This crate closes the walking-skeleton vertical slice (DESIGN.md §5):
//!
//! ```text
//! PTY ─▶ termwiz parser ─▶ sprag emulator ─▶ GridBuffer projection
//!     ─▶ pinion Scene::TextGrid ─▶ scene/snapshot
//! ```
//!
//! [`session`] owns the OS pseudoterminal and the [`sprag_vt::Emulator`]
//! it feeds; this module projects the emulator's [`Screen`] into a
//! single-pane `Scene::TextGrid` and reads it back as the
//! [`TextGridSnapshot`] an AI consumer sees over `scene/snapshot`.
//!
//! ## Why this is GPU-free (DESIGN.md §5 host-viability risk)
//!
//! `pinion_shell::run` opens a winit/Vello window; the platform's reason
//! to exist, though, is that AI reads the screen as *data*, not pixels
//! (DESIGN.md §1). The scene-as-data path needs no display: [`snapshot`]
//! builds the scene and calls `pinion_rpc`'s snapshot serializer directly
//! — `pinion-rpc` carries no GPU dependency. Only the (deferred,
//! paint-opaque) human rendering would need a GPU.
//!
//! ## Winsize ownership in headless mode (DESIGN.md §3)
//!
//! §3 makes the layout-resolved pixel `rect` the winsize SSOT, from which
//! pinion derives `(cols, rows)`. With no window there is no layout pass,
//! so the producer is the natural winsize source: [`scene`] sets the grid
//! node's `rect` directly from the screen dimensions and the cell metric,
//! and pinion's derivation (`cols = rect.w / cell_w`) round-trips it. A
//! windowed host would instead let `pinion_runtime::compute_layout` fill
//! the rect from a `LayoutStyle`.

pub mod rpc;
pub mod session;

pub use rpc::{handle_request, serve};
pub use session::{CommandBuilder, SessionError, TerminalSession};

use pinion_core::scene::{Rect, TextGridNode};
use pinion_core::{CellMetric, Scene};
use sprag_vt::Screen;

// Re-export the snapshot shapes a consumer reads, so downstream code need
// not depend on pinion-rpc's module layout directly.
pub use pinion_rpc::snapshot::{GridRowSnapshot, GridStyleRun, TextGridSnapshot};

/// The intent tag the single terminal pane carries in the scene tree.
/// `scene/snapshot` path routing and future input injection address the
/// grid through this tag.
pub const PANE_TAG: &str = "sprag_pane";

/// Project a [`Screen`] into a single-pane `Scene::TextGrid`, using the
/// behaviour-preserving 8×16 baseline cell metric ([`CellMetric::DEFAULT`]).
#[must_use]
pub fn scene(screen: &Screen) -> Scene {
    scene_with_metric(screen, CellMetric::DEFAULT)
}

/// Project a [`Screen`] into a single-pane `Scene::TextGrid` using a chosen
/// cell metric.
///
/// The node's `rect` is sized to `cols × cell_w` by `rows × cell_h` so
/// pinion's layout-derived `(cols, rows)` round-trips the screen dimensions
/// (the headless winsize SSOT — see the module docs).
#[must_use]
pub fn scene_with_metric(screen: &Screen, metric: CellMetric) -> Scene {
    let buffer = sprag_grid::project(screen);
    let mut node = TextGridNode::new(metric)
        .with_tag(PANE_TAG)
        .with_cells(buffer);
    node.rect = Rect::new(
        0,
        0,
        u32::from(screen.cols()) * metric.cell_w(),
        u32::from(screen.rows()) * metric.cell_h(),
    );
    Scene::TextGrid(node)
}

/// Snapshot a [`Screen`] as scene-as-data: the [`TextGridSnapshot`] an AI
/// consumer reads over `scene/snapshot`, produced headlessly (no GPU, no
/// shell event loop).
///
/// # Panics
///
/// Panics only on an internal invariant break: the scene root is a
/// `Scene::TextGrid` by construction, and the empty path is always a
/// supported `scene/snapshot` query, so neither failure can occur for a
/// scene built by [`scene`].
#[must_use]
pub fn snapshot(screen: &Screen) -> TextGridSnapshot {
    snapshot_with_metric(screen, CellMetric::DEFAULT)
}

/// [`snapshot`] with a chosen cell metric.
///
/// # Panics
///
/// See [`snapshot`].
#[must_use]
pub fn snapshot_with_metric(screen: &Screen, metric: CellMetric) -> TextGridSnapshot {
    let scene = scene_with_metric(screen, metric);
    match pinion_rpc::snapshot::snapshot(&scene, "")
        .expect("the empty path is a supported scene/snapshot query")
    {
        pinion_rpc::snapshot::SnapshotNode::TextGrid(grid) => grid,
        other => unreachable!("scene root is a TextGrid by construction, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    fn snapshot_of(bytes: &[u8], cols: u16, rows: u16) -> TextGridSnapshot {
        let mut em = Emulator::new(cols, rows);
        em.advance(bytes);
        snapshot(em.screen())
    }

    #[test]
    fn snapshot_round_trips_text_and_dimensions() {
        let snap = snapshot_of(b"hi", 20, 3);
        // Winsize round-trip: rect-derived dims match the screen (DESIGN.md §3).
        assert_eq!((snap.cols, snap.rows), (20, 3));
        // Buffer dims are the projection's own count.
        assert_eq!((snap.buffer_cols, snap.buffer_rows), (20, 3));
        assert_eq!(snap.grid_rows.len(), 3);
        assert!(snap.grid_rows[0].text.starts_with("hi"));
        // The cursor advanced past the two printed cells.
        assert_eq!((snap.cursor.col, snap.cursor.row), (2, 0));
        assert_eq!(snap.screen, "main");
    }

    #[test]
    fn snapshot_carries_cursor_and_alt_screen() {
        // Enter the alternate screen (vim/htop), then print.
        let snap = snapshot_of(b"\x1b[?1049hX", 10, 2);
        assert_eq!(snap.screen, "alternate");
        assert!(snap.grid_rows[0].text.starts_with("X"));
    }

    #[test]
    fn snapshot_marks_wide_cluster_runs() {
        // A wide CJK ideograph occupies two columns: a head run + trailer.
        let snap = snapshot_of("\u{4e16}".as_bytes(), 6, 1);
        let runs = &snap.grid_rows[0].runs;
        assert_eq!(runs[0].width, "wide");
        assert_eq!(runs[1].width, "trailer");
    }

    #[test]
    fn snapshot_reports_per_row_damage_generation() {
        // A fresh row is untouched (generation 0); a written row is stamped.
        let snap = snapshot_of(b"a", 4, 2);
        assert!(snap.grid_rows[0].generation > 0);
        assert_eq!(snap.grid_rows[1].generation, 0);
    }
}
