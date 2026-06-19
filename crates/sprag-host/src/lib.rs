//! sprag-host — the headless host layer.
//!
//! Assembles the producer's terminal panes into a pinion scene and exposes
//! them as scene-as-data. The scene is a workspace `Scene::Container` of N
//! pane children plus one control child:
//!
//! * each **pane** is a `Scene::Container` of the R1.7 data/engine split — a
//!   `Scene::TextGrid` (the cell grid an AI reads via `scene/snapshot` /
//!   `scene/query`) and a `Scene::External` ([`SpragPaneExternal`]) whose
//!   `scene/invoke` action channel injects input (key→PTY-byte encoding is
//!   sprag-owned — PINION-REQUIREMENTS R2.6); and
//! * the **workspace control** `Scene::External` ([`WorkspaceExternal`]),
//!   whose `scene/invoke` actions spawn / close / resize panes (the Round 7
//!   headless multiplex control core).
//!
//! This is the layer that owns the pinion-rpc dependency (the RPC dispatch
//! and the `scene/snapshot` wire), keeping the producer ([`sprag_terminal`])
//! and projection ([`sprag_grid`]) crates free of its heavy transitive deps.
//!
//! Pipeline (DESIGN.md §5): the producer's [`Workspace`] holds the panes;
//! [`workspace_scene`] assembles the tree (refreshing each grid from its live
//! screen, handing engines their `SessionHandle`); [`snapshot`] reads one
//! grid back as the [`TextGridSnapshot`] an AI consumer sees; [`serve`] runs
//! the JSON-RPC loop.
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

mod external;
pub mod pane;
pub mod plugins;
pub mod rpc;
pub mod runs;
pub mod workspace;

pub use pane::SpragPaneExternal;
pub use plugins::PluginsExternal;
pub use rpc::{handle_request, serve, HostState, SUPPORTED_METHODS};
pub use runs::{RunId, RunRegistry, RunState};
pub use workspace::WorkspaceExternal;

use std::sync::{Arc, Mutex};

use pinion_core::scene::{ContainerNode, ExternalNode, TextGridNode};
use pinion_core::{CellMetric, Scene};
use sprag_terminal::{PaneId, TerminalSession, Workspace};
use sprag_vt::Screen;

use crate::external::lock;

// Re-export the snapshot shapes a consumer reads, so downstream code need
// not depend on pinion-rpc's module layout directly.
pub use pinion_rpc::snapshot::{GridRowSnapshot, GridStyleRun, TextGridSnapshot};

/// The intent tag on the workspace's root `Scene::Container`.
pub const WORKSPACE_TAG: &str = "sprag_workspace";

/// The tag on the workspace-control `External` (pane management). The
/// `scene/invoke` path addresses it as `/sprag_mux/external/<action>`.
pub const MUX_TAG: &str = "sprag_mux";

/// The tag on the plugin-host `External`. The `scene/invoke` path addresses it
/// as `/sprag_plugins/external/run`.
pub const PLUGINS_TAG: &str = "sprag_plugins";

/// The tag on each pane's `TextGrid` child (the cell-data projection).
pub const GRID_TAG: &str = "sprag_grid";

/// The tag on each pane's input `External` child. The `scene/invoke` input
/// path addresses pane `<id>` as `/pane_<id>/sprag_input/external/key` (R2.6).
pub const INPUT_TAG: &str = "sprag_input";

/// Project a [`Screen`] into the `TextGrid` node carrying the cell data.
///
/// This is the **single** `Screen` → `TextGridNode` projection — the one call
/// site of [`sprag_grid::project`]. Both the headless data path ([`scene`] /
/// [`pane_container`]) and the GUI windowed host (`sprag-gui`) read it, so the
/// cell projection has exactly one authority (no GUI-side duplicate).
///
/// The node carries **no layout** here: the headless path leaves the GUI
/// `rect` unset (the authoritative terminal size is the projected
/// `GridBuffer`, read via [`TextGridSnapshot::buffer_cols`] / `buffer_rows`),
/// while a windowed host attaches its own [`LayoutStyle`](pinion_core::style::LayoutStyle)
/// via [`TextGridNode::with_layout`] so `pinion_runtime::compute_layout`
/// derives the cell `(cols, rows)` from the resolved rect (the §3 GUI winsize
/// SSOT, inverse of the headless contract). Layout is therefore the host's
/// concern, kept out of this projection so this crate stays presentation-free.
#[must_use]
pub fn text_grid_node(screen: &Screen, metric: CellMetric) -> TextGridNode {
    TextGridNode::new(metric)
        .with_tag(GRID_TAG)
        .with_cells(sprag_grid::project(screen))
}

/// Assemble a [`Screen`] into a bare `Scene::TextGrid` (the cell-data view),
/// using the 8×16 baseline cell metric ([`CellMetric::DEFAULT`]).
///
/// This is the pure data projection — a function of the screen alone, with
/// no live session — used by [`snapshot`] and data-only consumers. The RPC
/// server assembles the full pane via [`pane_scene`].
#[must_use]
pub fn scene(screen: &Screen) -> Scene {
    scene_with_metric(screen, CellMetric::DEFAULT)
}

/// [`scene`] with an explicit cell metric.
#[must_use]
pub fn scene_with_metric(screen: &Screen, metric: CellMetric) -> Scene {
    Scene::TextGrid(text_grid_node(screen, metric))
}

/// Build one pane's `Scene::Container` (tagged `pane_<id>`) — the R1.7
/// data/engine split: a `TextGrid` projected from the pane's live screen,
/// and a [`SpragPaneExternal`] input engine holding the pane's
/// [`SessionHandle`](sprag_terminal::SessionHandle) so `scene/invoke` reaches
/// that pane's PTY.
fn pane_container(id: PaneId, session: &TerminalSession) -> Scene {
    let children = session.with_screen(|screen| {
        vec![
            Scene::TextGrid(text_grid_node(screen, CellMetric::DEFAULT)),
            Scene::External(
                ExternalNode::new(Box::new(pane::SpragPaneExternal::new(session.handle())))
                    .with_tag(INPUT_TAG),
            ),
        ]
    });
    Scene::Container(ContainerNode::new(children).with_tag(format!("pane_{id}")))
}

/// Assemble the live workspace as a `Scene::Container` of its panes plus the
/// pane-management [`WorkspaceExternal`] (Round 7 multiplex control core).
///
/// Each pane child is refreshed from its session's current screen; the
/// engines and the control surface hold shared handles (a `SessionHandle`
/// per pane, an `Arc<Mutex<Workspace>>` for control), so the per-request
/// scene stays a throwaway projection (R969) while input and pane lifecycle
/// reach live state. The workspace lock is released before returning so a
/// dispatched `scene/invoke` (spawn/close/resize) can re-acquire it without
/// deadlock.
#[must_use]
pub fn workspace_scene(
    workspace: &Arc<Mutex<Workspace>>,
    runs: &Arc<Mutex<RunRegistry>>,
) -> Scene {
    let mut children: Vec<Scene> = {
        let guard = lock(workspace);
        guard
            .panes()
            .iter()
            .map(|pane| pane_container(pane.id(), pane.session()))
            .collect()
    };
    children.push(Scene::External(
        ExternalNode::new(Box::new(workspace::WorkspaceExternal::new(Arc::clone(workspace))))
            .with_tag(MUX_TAG),
    ));
    children.push(Scene::External(
        ExternalNode::new(Box::new(plugins::PluginsExternal::new(
            Arc::clone(workspace),
            Arc::clone(runs),
        )))
        .with_tag(PLUGINS_TAG),
    ));
    Scene::Container(ContainerNode::new(children).with_tag(WORKSPACE_TAG))
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
