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
pub use rpc::{HostState, SUPPORTED_METHODS, handle_request, serve};
pub use runs::{RunId, RunRegistry, RunState};
pub use workspace::WorkspaceExternal;

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use pinion_core::scene::{ContainerNode, ExternalNode, TextGridNode};
use pinion_core::style::{LayoutStyle, Size, SizeValue};
use pinion_core::{CellMetric, GridBuffer, Scene};
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

/// The tagged `TextGrid` node carrying a pre-projected cell buffer — the
/// node-shape SSOT (tag + metric + cells), shared by the headless data path
/// ([`text_grid_node`]) and the GUI view path ([`view_text_grid`]). The
/// projection itself is [`sprag_grid::project`] / [`sprag_grid::project_scrolled`];
/// this assembles the node around whatever cells the caller projected.
fn grid_node(metric: CellMetric, cells: GridBuffer) -> TextGridNode {
    TextGridNode::new(metric)
        .with_tag(GRID_TAG)
        .with_cells(cells)
}

/// Project a [`Screen`] into the bare `TextGrid` node carrying the cell data
/// (the headless data path: [`scene`] / [`pane_container`]). The one call site
/// of [`sprag_grid::project`] for the data path, so the cell projection has one
/// authority. The node carries no layout: the headless path leaves the GUI
/// `rect` unset (the authoritative terminal size is the projected `GridBuffer`,
/// read via [`TextGridSnapshot::buffer_cols`] / `buffer_rows`); the GUI seam
/// adds layout + font size via [`view_text_grid`].
pub(crate) fn text_grid_node(screen: &Screen, metric: CellMetric) -> TextGridNode {
    grid_node(metric, sprag_grid::project(screen))
}

/// A per-pane view request: everything the windowed-host projection needs to
/// render ONE pane — its live `screen`, the measured cell `metric` and the glyph
/// `font_size_px` it was measured at (pinion R1002), the scrollback
/// `offset_lines` window, and the IME `preedit` overlay.
///
/// This is the **D1 resolution**: the single-pane seam grew a positional
/// `(offset, preedit)` tail; multi-pane gives every pane its own `(offset,
/// preedit)`, so the tail crosses the threshold from "premature struct" to "N
/// real consumers" and becomes a named spec rather than a 5th/6th positional arg.
/// `'a` borrows the live screen (held only inside the producer's
/// `with_screen` lock) and the preedit string for the call.
pub struct PaneViewSpec<'a> {
    /// The pane's live producer screen (read under `TerminalSession::with_screen`).
    pub screen: &'a Screen,
    /// The once-measured monospace cell (pinion R1003).
    pub metric: CellMetric,
    /// The glyph size the cell was measured at (pinion R1002 `with_font_size_px`).
    pub font_size_px: u32,
    /// Scrollback window: `0` is the live screen, `n` scrolls up `n` rows.
    pub offset_lines: usize,
    /// In-progress IME composition overlaid at the cursor (empty = none).
    pub preedit: &'a str,
}

/// Project ONE pane (`spec`) into a tagged `Scene::Container` — the multi-pane
/// building block. The same single projection
/// ([`sprag_grid::project_scrolled`] + [`sprag_grid::overlay_preedit`]) wrapped in
/// the [`view_text_grid`] presentation (R1002 font-size pin + fill), inside a
/// Container that takes an **even flex share** of its row
/// (`flex_basis 0 + flex_grow 1`, the CSS proportional-split idiom).
///
/// The Container `tag` is the per-pane identity the windowed host keys three
/// things on, so the GUI passes the SAME tag it registers as that pane's
/// focusable input External:
/// * pinion R1012 [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)
///   — the post-layout pane rect that sizes this pane's PTY (`TIOCSWINSZ`);
/// * the framework focus ring (drawn around the focused tag's rect); and
/// * click-to-focus (the rect a pointer press hit-tests).
///
/// The GUI arranges N of these into the window (an even split, or draggable
/// `view_splitter` dividers — R38; `sprag-gui` owns that interactive layout since
/// it needs reactive ratio `Signal`s the headless host has no use for). The
/// overlay self-gates to the live view (a scrolled `project_scrolled` drops the
/// cursor, and `overlay_preedit` no-ops without one), so an empty `preedit` is
/// identical to the bare scrolled projection and no `offset_lines` check is needed.
#[must_use]
pub fn pane_view_scene(tag: impl Into<Cow<'static, str>>, spec: PaneViewSpec<'_>) -> Scene {
    let cells = sprag_grid::overlay_preedit(
        sprag_grid::project_scrolled(spec.screen, spec.offset_lines),
        spec.preedit,
    );
    Scene::Container(
        ContainerNode::new(vec![view_text_grid(cells, spec.metric, spec.font_size_px)])
            .with_tag(tag)
            .with_layout(
                LayoutStyle::new()
                    .with_flex_basis(SizeValue::Px(0))
                    .with_flex_grow(1.0),
            ),
    )
}

/// Assemble the windowed-host `Scene::TextGrid` from a pre-projected cell buffer
/// — the GUI presentation (tag + R1002 font-size pin + fill layout) shared by
/// every pane, so the node shape lives in one place. It fills its pane Container
/// (both axes `Percent(100)`), so the grid spans whatever sub-rect the flex split
/// resolved for that pane.
fn view_text_grid(cells: GridBuffer, metric: CellMetric, font_size_px: u32) -> Scene {
    Scene::TextGrid(
        grid_node(metric, cells)
            .with_font_size_px(font_size_px)
            .with_layout(
                LayoutStyle::new().with_size(
                    Size::auto()
                        .with_width(SizeValue::Percent(100))
                        .with_height(SizeValue::Percent(100)),
                ),
            ),
    )
}

/// Assemble a [`Screen`] into a bare `Scene::TextGrid` (the cell-data view),
/// using the 8×16 baseline cell metric ([`CellMetric::DEFAULT`]).
///
/// This is the pure data projection — a function of the screen alone, with
/// no live session — used by [`snapshot`] and data-only consumers. The RPC
/// server assembles the full pane tree via [`workspace_scene`]; the windowed
/// host builds per-pane scenes via [`pane_view_scene`] and arranges them GUI-side.
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
pub fn workspace_scene(workspace: &Arc<Mutex<Workspace>>, runs: &Arc<Mutex<RunRegistry>>) -> Scene {
    let mut children: Vec<Scene> = {
        let guard = lock(workspace);
        guard
            .panes()
            .iter()
            .map(|pane| pane_container(pane.id(), pane.session()))
            .collect()
    };
    children.push(Scene::External(
        ExternalNode::new(Box::new(workspace::WorkspaceExternal::new(Arc::clone(
            workspace,
        ))))
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

    /// Extract the lone projected `TextGrid` from a [`pane_view_scene`] pane
    /// Container (the pane wraps the grid in a tagged flex-share Container).
    fn pane_grid(scene: &Scene) -> &TextGridNode {
        match scene {
            Scene::Container(c) => match c.children.first() {
                Some(Scene::TextGrid(node)) => node,
                other => unreachable!("a pane Container holds one TextGrid, got {other:?}"),
            },
            other => unreachable!("pane_view_scene returns a Container, got {other:?}"),
        }
    }

    #[test]
    fn pane_view_scene_wraps_projection_in_a_tagged_flex_share_container() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"hi");
        let scene = super::pane_view_scene(
            "pane.test",
            PaneViewSpec {
                screen: em.screen(),
                metric: CellMetric::DEFAULT,
                font_size_px: 18,
                offset_lines: 0,
                preedit: "",
            },
        );
        match &scene {
            Scene::Container(c) => {
                // The per-pane identity tag (the use_pane_viewport_size rect
                // target / focus-ring / click-focus anchor).
                assert_eq!(c.tag.as_deref(), Some("pane.test"));
                // An even flex share of its row (the proportional-split idiom).
                assert_eq!(c.layout.flex_basis, Some(SizeValue::Px(0)));
                assert!((c.layout.flex_grow - 1.0).abs() < f32::EPSILON);
            }
            other => unreachable!("pane_view_scene returns a Container, got {other:?}"),
        }
        // The inner grid: the single projection (tagged + cells present), the
        // R1002 font-size pin, and a fill layout so it spans the pane sub-rect.
        let node = pane_grid(&scene);
        assert_eq!(node.tag.as_deref(), Some(GRID_TAG));
        assert!(!node.cells().is_empty());
        assert_eq!(node.font_size_px(), Some(18));
        assert_eq!(node.layout.size.width, SizeValue::Percent(100));
        assert_eq!(node.layout.size.height, SizeValue::Percent(100));
    }

    /// The live view (`offset 0`) overlays the IME preedit at the cursor (after
    /// "hi", col 2); a scrolled history window drops it (the cursor — the compose
    /// anchor — lives only in the live view, so `overlay_preedit` self-gates off).
    #[test]
    fn pane_view_scene_overlays_preedit_only_on_the_live_view() {
        let cell_cluster = |scene: &Scene, col: u16, row: u16| {
            pane_grid(scene)
                .cells()
                .cell(col, row)
                .map(|c| c.cluster.to_string())
        };
        let contains_han = |scene: &Scene| {
            let g = pane_grid(scene).cells();
            (0..g.cols())
                .any(|c| (0..g.rows()).any(|r| g.cell(c, r).is_some_and(|x| x.cluster == "한")))
        };
        // Live view: the preedit shows at the cursor; an empty preedit does not.
        let mut em = Emulator::new(8, 2);
        em.advance(b"hi");
        let live = super::pane_view_scene(
            "pane.test",
            PaneViewSpec {
                screen: em.screen(),
                metric: CellMetric::DEFAULT,
                font_size_px: 18,
                offset_lines: 0,
                preedit: "한",
            },
        );
        assert_eq!(
            cell_cluster(&live, 2, 0).as_deref(),
            Some("한"),
            "preedit at the cursor on the live view"
        );
        let bare = super::pane_view_scene(
            "pane.test",
            PaneViewSpec {
                screen: em.screen(),
                metric: CellMetric::DEFAULT,
                font_size_px: 18,
                offset_lines: 0,
                preedit: "",
            },
        );
        assert!(!contains_han(&bare), "no composition -> no overlay");
        // Scrolled view: the preedit must appear NOWHERE (the half the test name
        // promises). Fails if the overlay's cursor-visible self-gate regresses.
        let mut sc = Emulator::new(4, 2);
        sc.advance(b"a\r\nb\r\nc\r\nd\r\ne"); // 3 rows scroll into history
        assert_eq!(sc.screen().scrollback_len(), 3);
        let scrolled = super::pane_view_scene(
            "pane.test",
            PaneViewSpec {
                screen: sc.screen(),
                metric: CellMetric::DEFAULT,
                font_size_px: 18,
                offset_lines: 1,
                preedit: "한",
            },
        );
        assert!(
            !contains_han(&scrolled),
            "a scrolled history window shows no preedit"
        );
    }
}
