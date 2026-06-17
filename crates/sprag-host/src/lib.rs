//! sprag-host — the headless host layer.
//!
//! Assembles the producer's terminal [`Screen`] into a pinion
//! `Scene::TextGrid` and exposes it as scene-as-data. This is the layer that
//! owns the pinion-rpc dependency (the RPC dispatch + `scene/snapshot` wire),
//! keeping the producer ([`sprag_terminal`]) and projection ([`sprag_grid`])
//! crates free of its heavy transitive deps.
//!
//! Pipeline (DESIGN.md §5): the producer's [`TerminalSession`] holds the
//! emulator; [`scene`] wraps `sprag_grid::project(screen)` into a
//! `Scene::TextGrid`; [`snapshot`] reads it back as the [`TextGridSnapshot`]
//! an AI consumer sees; [`serve`] runs the JSON-RPC loop.
//!
//! ## Why this is GPU-free (DESIGN.md §5 host-viability risk)
//!
//! `pinion_shell::run` opens a winit/Vello window; the platform's reason to
//! exist, though, is that AI reads the screen as *data*, not pixels
//! (DESIGN.md §1). `pinion-rpc` carries no GPU dependency, so this whole
//! path runs headlessly. Only the (deferred, paint-opaque) human rendering
//! would need a GPU.
//!
//! ## Winsize ownership (DESIGN.md §3)
//!
//! §3 makes the layout-resolved pixel `rect` the winsize SSOT for a *GUI*
//! host: layout fills the rect, pinion derives `(cols, rows)`, the producer
//! adopts them. A headless host has no layout pass, so that derivation does
//! not apply: the producer/consumer specifies the size, the emulator and PTY
//! are sized to it, and the authoritative `(cols, rows)` live in the
//! GridBuffer — reported by [`TextGridSnapshot::buffer_cols`] /
//! [`TextGridSnapshot::buffer_rows`]. [`scene`] therefore leaves the node's
//! GUI `rect` unset; its rect-derived `cols` / `rows` stay `0` here by
//! design (not faked to mirror the buffer). A future windowed host fills the
//! rect via `pinion_runtime::compute_layout`.

pub mod rpc;

pub use rpc::{handle_request, serve, READ_METHODS};

use pinion_core::scene::TextGridNode;
use pinion_core::{CellMetric, Scene};
use sprag_vt::Screen;

// Re-export the snapshot shapes a consumer reads, so downstream code need
// not depend on pinion-rpc's module layout directly.
pub use pinion_rpc::snapshot::{GridRowSnapshot, GridStyleRun, TextGridSnapshot};

/// The intent tag the single terminal pane carries in the scene tree.
/// `scene/snapshot` path routing (and future input addressing) reach the
/// grid through this tag.
pub const PANE_TAG: &str = "sprag_pane";

/// Assemble a [`Screen`] into a single-pane `Scene::TextGrid`, using the
/// behaviour-preserving 8×16 baseline cell metric ([`CellMetric::DEFAULT`]).
#[must_use]
pub fn scene(screen: &Screen) -> Scene {
    scene_with_metric(screen, CellMetric::DEFAULT)
}

/// Assemble a [`Screen`] into a single-pane `Scene::TextGrid` using a chosen
/// cell metric.
///
/// The node's GUI `rect` is intentionally left unset (see the module docs on
/// headless winsize ownership): the authoritative terminal size is the
/// projected `GridBuffer`, read via [`TextGridSnapshot::buffer_cols`] /
/// `buffer_rows`.
#[must_use]
pub fn scene_with_metric(screen: &Screen, metric: CellMetric) -> Scene {
    Scene::TextGrid(
        TextGridNode::new(metric)
            .with_tag(PANE_TAG)
            .with_cells(sprag_grid::project(screen)),
    )
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
    let scene = scene(screen);
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
    fn snapshot_round_trips_content_and_buffer_dims() {
        let snap = snapshot_of(b"hi", 20, 3);
        // The authoritative terminal size is the producer's GridBuffer.
        assert_eq!((snap.buffer_cols, snap.buffer_rows), (20, 3));
        // Headless: no layout pass, so the GUI rect-derived winsize is unset
        // (the documented contract — not faked to mirror the buffer).
        assert_eq!((snap.cols, snap.rows), (0, 0));
        assert_eq!(snap.grid_rows.len(), 3);
        assert!(snap.grid_rows[0].text.starts_with("hi"));
        assert_eq!((snap.cursor.col, snap.cursor.row), (2, 0));
        assert_eq!(snap.screen, "main");
    }

    #[test]
    fn snapshot_carries_alt_screen() {
        let snap = snapshot_of(b"\x1b[?1049hX", 10, 2);
        assert_eq!(snap.screen, "alternate");
        assert!(snap.grid_rows[0].text.starts_with("X"));
    }

    #[test]
    fn snapshot_marks_wide_cluster_runs() {
        let snap = snapshot_of("\u{4e16}".as_bytes(), 6, 1);
        let runs = &snap.grid_rows[0].runs;
        assert_eq!(runs[0].width, "wide");
        assert_eq!(runs[1].width, "trailer");
    }

    #[test]
    fn snapshot_reports_per_row_damage_generation() {
        let snap = snapshot_of(b"a", 4, 2);
        assert!(snap.grid_rows[0].generation > 0);
        assert_eq!(snap.grid_rows[1].generation, 0);
    }
}
