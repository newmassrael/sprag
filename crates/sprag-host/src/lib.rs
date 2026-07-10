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
pub use rpc::{
    FrameIngress, HostState, SUPPORTED_METHODS, dispatch_frames, handle_request, stdin_frames,
};
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
/// node-shape SSOT (metric + cells), shared by the headless data path
/// ([`text_grid_node`], tagged [`GRID_TAG`]) and the GUI view path
/// ([`view_text_grid`], tagged a per-pane `{pane}#grid` composite so a click
/// resolves to the focusable pane). The `tag` is the caller's because it differs by
/// path; the projection itself is [`sprag_grid::project`] /
/// [`sprag_grid::project_scrolled`]; this assembles the node around whatever cells
/// the caller projected.
fn grid_node(
    tag: impl Into<Cow<'static, str>>,
    metric: CellMetric,
    cells: GridBuffer,
) -> TextGridNode {
    TextGridNode::new(metric).with_tag(tag).with_cells(cells)
}

/// Project a [`Screen`] into the bare `TextGrid` node carrying the cell data
/// (the headless data path: [`scene`] / [`pane_container`]). The one call site
/// of [`sprag_grid::project`] for the data path, so the cell projection has one
/// authority. The node carries no layout: the headless path leaves the GUI
/// `rect` unset (the authoritative terminal size is the projected `GridBuffer`,
/// read via [`TextGridSnapshot::buffer_cols`] / `buffer_rows`); the GUI seam
/// adds layout + font size via [`view_text_grid`].
pub(crate) fn text_grid_node(screen: &Screen, metric: CellMetric) -> TextGridNode {
    grid_node(GRID_TAG, metric, sprag_grid::project(screen))
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

/// Project ONE pane (`spec`) into a tagged `Scene::Container` — the live-`Screen`
/// convenience over [`pane_view_scene_from_cells`] (which holds the node shape).
/// The same single projection
/// ([`sprag_grid::project_scrolled`] + [`sprag_grid::overlay_preedit`]) wrapped in
/// the [`view_text_grid`] presentation (R1002 font-size pin + fill), inside a
/// tagged, **focus-stop** Container. The Container's ONLY layout of its own is the
/// pinion R1020 `focusable` flag ([`LayoutStyle::with_focusable`](pinion_core::style::LayoutStyle::with_focusable)),
/// which declares this pane a keyboard Tab stop where it is painted (§5.39
/// scene-derived focus — see below). Its SIZE/FLEX still come from the GUI's
/// arrangement — `sprag-gui`'s `view_splitter` overwrites `flex_basis`/`flex_grow`
/// with the drag ratio for a tiled pane, the lone-pane / undock-window paths size
/// it `Percent(100)` — and those layout mutators (the splitter's `apply_flex_main`,
/// the fill's `map_layout`) edit the size/flex FIELDS in place, so they preserve
/// the `focusable` flag set here. (R38 removed the host's even-tiling, so the old
/// `flex_basis 0 + flex_grow 1` here was dead; R1020 brings the layout back, but
/// now carrying exactly the live `focusable` flag, not a dead flex share.)
///
/// The Container `tag` is the per-pane identity the windowed host keys these
/// things on, so the GUI passes the SAME tag it registers as that pane's input
/// External:
/// * pinion R1012 [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)
///   — the post-layout pane rect that sizes this pane's PTY (`TIOCSWINSZ`);
/// * keyboard focus: under pinion R1020 §5.39 the shell derives the Tab order each
///   frame from [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
///   (the `focusable`-marked, tagged nodes), so marking THIS node focusable makes
///   the pane a Tab stop — there is no binding-side `focusable_tags()` list anymore;
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
    // The live-screen projection + IME overlay — the two things the node needs
    // MORE than the cells for. Both stay on the projecting side of the topology-B
    // seam: the host owns the scrollback screen (`project_scrolled`), and the
    // preedit is a client-local overlay on received cells. The resulting cells
    // feed the shared Screen-free assembly, which holds the node shape.
    let cells = sprag_grid::overlay_preedit(
        sprag_grid::project_scrolled(spec.screen, spec.offset_lines),
        spec.preedit,
    );
    pane_view_scene_from_cells(tag, cells, spec.metric, spec.font_size_px)
}

/// Assemble ONE pane's `Scene::Container` from an already-projected cell buffer
/// — the **Screen-free** pane-node SSOT the two frontends share.
///
/// The node is a tagged, **focus-stop** Container wrapping the pane's grid. The
/// Container's only own layout is the pinion R1020 `focusable` flag
/// ([`LayoutStyle::with_focusable`](pinion_core::style::LayoutStyle::with_focusable)),
/// declaring the pane a scene-derived keyboard Tab stop (§5.39). Its SIZE/FLEX
/// come from the GUI's arrangement (`view_splitter` drag ratio for a tiled pane;
/// `Percent(100)` for a lone / undocked pane); those mutators edit the size/flex
/// fields in place, preserving the `focusable` flag set here. The inner grid
/// stays NON-focusable, so each pane is exactly one Tab stop.
///
/// The grid child carries the COMPOSITE `{tag}#grid` sub-tag: a pointer press
/// lands on the grid (the deepest tagged node under the cursor) and pinion's
/// click-to-focus resolves a `primary#sub` tag back to `primary` (via
/// `resolve_focusable` / `composite_tag::split_subindex`) — the focusable pane.
/// A plain shared tag (e.g. the headless [`GRID_TAG`]) resolves to nothing, so
/// the click would NOT move focus (the live bug: clicking a pane kept typing in
/// the previously-focused one) and is ambiguous across panes.
///
/// **This is the seam the topology-B display client shares.** The in-process GUI
/// reaches it via [`pane_view_scene`] (projecting a live [`Screen`] first); the
/// end-state wire client feeds cells it reconstructed off the host's served data
/// model, building the byte-identical pane node without ever touching a live
/// `Screen`. (The host's own RPC data path uses a different container —
/// [`pane_container`], input `External` embedded — see [`workspace_scene`].)
#[must_use]
pub fn pane_view_scene_from_cells(
    tag: impl Into<Cow<'static, str>>,
    cells: GridBuffer,
    metric: CellMetric,
    font_size_px: u32,
) -> Scene {
    let tag = tag.into();
    let grid_tag = format!("{tag}#grid");
    Scene::Container(
        ContainerNode::new(vec![view_text_grid(cells, grid_tag, metric, font_size_px)])
            .with_tag(tag)
            .with_layout(LayoutStyle::new().with_focusable(true)),
    )
}

/// Assemble the windowed-host `Scene::TextGrid` from a pre-projected cell buffer
/// — the GUI presentation (R1002 font-size pin + fill layout) shared by every pane,
/// so the node shape lives in one place. `tag` is the per-pane `{pane}#grid`
/// composite ([`pane_view_scene`]). It fills its pane Container (both axes
/// `Percent(100)`), so the grid spans whatever sub-rect the flex split resolved for
/// that pane.
fn view_text_grid(
    cells: GridBuffer,
    tag: impl Into<Cow<'static, str>>,
    metric: CellMetric,
    font_size_px: u32,
) -> Scene {
    Scene::TextGrid(
        grid_node(tag, metric, cells)
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
    fn pane_view_scene_is_a_focusable_tagged_container_around_the_grid() {
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
                // Its only own layout is the R1020 focus-stop flag — size/flex
                // come from the GUI arrangement (splitter / fill).
                assert_eq!(c.layout, LayoutStyle::new().with_focusable(true));
            }
            other => unreachable!("pane_view_scene returns a Container, got {other:?}"),
        }
        // R1020 §5.39 contract: the pane is exactly one scene-derived Tab stop
        // (its tag), and the inner grid is NOT a focus stop — so a future pinion
        // focus-enumeration regression fails HERE, not silently in the GUI.
        assert_eq!(
            scene.collect_focusable_tags(),
            vec!["pane.test".to_owned()],
            "the pane Container is the one focusable node; its grid child is not",
        );
        // The inner grid: the per-pane `{pane}#grid` composite tag, the single
        // projection (cells present), the R1002 font-size pin, and a fill layout so
        // it spans the pane sub-rect.
        let node = pane_grid(&scene);
        assert_eq!(node.tag.as_deref(), Some("pane.test#grid"));
        // The composite splits back to the focusable pane tag — the exact
        // resolution pinion's click-to-focus (`resolve_focusable`) performs, so a
        // pointer press on the grid focuses the pane (not nothing).
        assert_eq!(
            pinion_core::composite_tag::split_subindex("pane.test#grid").0,
            "pane.test",
            "the grid tag resolves to the focusable pane (click-to-focus)",
        );
        assert!(!node.cells().is_empty());
        assert_eq!(node.font_size_px(), Some(18));
        assert_eq!(node.layout.size.width, SizeValue::Percent(100));
        assert_eq!(node.layout.size.height, SizeValue::Percent(100));
    }

    /// The topology-B seam: `pane_view_scene_from_cells` assembles the identical
    /// focusable pane node from a hand-built cell buffer — no live `Screen`. This
    /// is the exact call a wire display client makes with cells it reconstructed
    /// off the host's data model, so a regression in the shared node shape (the
    /// focus-stop Container / composite `{tag}#grid` click anchor / R1002 font pin)
    /// fails HERE, in the crate that owns the shape, not silently in the GUI.
    #[test]
    fn pane_view_scene_from_cells_assembles_the_pane_node_without_a_screen() {
        let cells = GridBuffer::new(4, 1);
        let scene = super::pane_view_scene_from_cells("pane.wire", cells, CellMetric::DEFAULT, 18);
        match &scene {
            Scene::Container(c) => {
                assert_eq!(c.tag.as_deref(), Some("pane.wire"));
                assert_eq!(c.layout, LayoutStyle::new().with_focusable(true));
            }
            other => unreachable!("pane_view_scene_from_cells returns a Container, got {other:?}"),
        }
        // The one scene-derived Tab stop is the pane; its grid child is not.
        assert_eq!(scene.collect_focusable_tags(), vec!["pane.wire".to_owned()]);
        let node = pane_grid(&scene);
        assert_eq!(node.tag.as_deref(), Some("pane.wire#grid"));
        // The composite splits back to the focusable pane tag — the exact
        // resolution pinion's click-to-focus performs on a pointer press.
        assert_eq!(
            pinion_core::composite_tag::split_subindex("pane.wire#grid").0,
            "pane.wire",
            "the grid tag resolves to the focusable pane (click-to-focus)",
        );
        assert_eq!(node.cells().cols(), 4);
        assert_eq!(node.font_size_px(), Some(18));
        assert_eq!(node.layout.size.width, SizeValue::Percent(100));
        assert_eq!(node.layout.size.height, SizeValue::Percent(100));
    }

    /// The topology-B wire fidelity guarantee (PINION-PR49 / R106): the
    /// paint-authoritative [`GridBuffer`] survives a JSON round-trip byte-for-byte,
    /// so a wire display client reconstructs the EXACT buffer the host projected —
    /// every colour (indexed / truecolor / default), attribute, wide-cluster
    /// head+trailer split, cursor, screen kind, and per-row damage generation. This
    /// is the load-bearing assumption of the whole GUI-as-client-of-host arc: if
    /// serde dropped a field, the human (GUI) path would render differently from the
    /// host's served data and the "read-data-not-pixels" invariant would break
    /// SILENTLY in the GUI. It fails HERE — in the crate that owns the wire seam —
    /// instead. Proven on REAL projected production data (not a synthetic buffer),
    /// so it also guards `sprag_grid::project` against emitting a non-round-trippable
    /// cell.
    #[test]
    fn wire_round_trip_preserves_the_pane_cell_buffer() {
        use sprag_vt::{Emulator, VtPort};
        // A rich MAIN screen written at the top-left: bold indexed-red, truecolor,
        // reverse, a wide (CJK) cluster (head+trailer). Writing row 0 also stamps it
        // with a nonzero damage generation. Everything a real terminal frame carries.
        let mut em = Emulator::new(12, 2);
        em.advance(b"\x1b[1;31mred\x1b[0m "); // bold indexed-red "red"
        em.advance(b"\x1b[38;2;10;20;30mR\x1b[0m "); // truecolor "R"
        em.advance(b"\x1b[7mV\x1b[0m"); // reverse "V"
        em.advance("世".as_bytes()); // a wide cluster (head + trailer)
        let buf = sprag_grid::project_scrolled(em.screen(), 0);

        // The wire: serialize -> deserialize the paint-authoritative buffer.
        let json = serde_json::to_string(&buf).expect("GridBuffer serializes (PR-49)");
        let back: GridBuffer = serde_json::from_str(&json).expect("GridBuffer round-trips (PR-49)");

        // The whole buffer is byte-identical — the strongest fidelity claim
        // (GridBuffer: Eq covers cells + cursor + screen + row generations).
        assert_eq!(
            buf, back,
            "serde round-trip is lossless for the paint buffer"
        );

        // Position-independent spot-checks so a dropped field type names itself (not
        // just "Eq failed"): search the round-tripped buffer for a cell of each kind.
        let find = |pred: &dyn Fn(&pinion_core::TermCell) -> bool| {
            (0..back.cols())
                .flat_map(|c| (0..back.rows()).map(move |r| (c, r)))
                .find_map(|(c, r)| back.cell(c, r).filter(|x| pred(x)))
        };
        // bold + indexed fg (the "red").
        let red = find(&|x| x.fg == pinion_core::TermColor::Indexed(1))
            .expect("an indexed-fg cell survived");
        assert!(
            red.attrs.bold,
            "bold attr survived alongside the indexed fg"
        );
        // truecolor fg (the "R").
        assert!(
            find(&|x| matches!(x.fg, pinion_core::TermColor::Rgb(_))).is_some(),
            "a truecolor (rgb) fg survived",
        );
        // reverse attr (the "V").
        assert!(
            find(&|x| x.attrs.reverse).is_some(),
            "the reverse attr survived",
        );
        // the wide cluster: head carries the cluster + Wide, its trailer is Trailer.
        let (wide_col, wide_row) = (0..back.cols())
            .flat_map(|c| (0..back.rows()).map(move |r| (c, r)))
            .find(|&(c, r)| {
                back.cell(c, r)
                    .is_some_and(|x| x.width == pinion_core::CellWidth::Wide)
            })
            .expect("the wide cluster survived the round-trip");
        assert_eq!(back.cell(wide_col, wide_row).unwrap().cluster, "世");
        assert_eq!(
            back.cell(wide_col + 1, wide_row).unwrap().width,
            pinion_core::CellWidth::Trailer,
        );
        // cursor (position / shape / visibility) and a nonzero damage generation.
        assert_eq!(back.cursor(), buf.cursor(), "cursor survived");
        assert!(back.cursor().visible, "the live cursor is visible");
        assert!(
            back.row_generation(0).is_some_and(|g| g > 0),
            "per-row damage generation survived",
        );

        // And the CLIENT node assembled from the round-tripped buffer carries the
        // round-tripped cells — a wire client paints byte-identically to the host's
        // projection (the buffers are Eq, so the deterministic assembly matches).
        let node_wire =
            super::pane_view_scene_from_cells("sprag_gui.pane.0", back, CellMetric::DEFAULT, 18);
        let node_direct =
            super::pane_view_scene_from_cells("sprag_gui.pane.0", buf, CellMetric::DEFAULT, 18);
        assert_eq!(
            pane_grid(&node_wire).cells(),
            pane_grid(&node_direct).cells(),
            "the client node built off the wire matches the direct projection",
        );

        // The screen-kind field round-trips as Alternate too (not just the Main
        // default above): a fullscreen app's alt screen must reach the client.
        let mut alt = Emulator::new(4, 2);
        alt.advance(b"\x1b[?1049hA");
        let altbuf = sprag_grid::project(alt.screen());
        assert_eq!(altbuf.screen(), pinion_core::ScreenKind::Alternate);
        let altback: GridBuffer =
            serde_json::from_str(&serde_json::to_string(&altbuf).unwrap()).unwrap();
        assert_eq!(altback, altbuf, "alt-screen buffer round-trips");
        assert_eq!(
            altback.screen(),
            pinion_core::ScreenKind::Alternate,
            "the Alternate screen kind survived the wire",
        );
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
