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
//! Pipeline (DESIGN.md §5): the producer's [`Workspace`](sprag_terminal::Workspace)
//! holds the panes (those of the current window of the session a request is SCOPED to,
//! resolved out of the [`SessionRegistry`] — see [`SessionScope`]);
//! [`workspace_scene`] assembles the tree (refreshing each grid from its live
//! screen, handing engines their `PanePtyHandle`); [`snapshot`] reads one
//! grid back as the [`TextGridSnapshot`] an AI consumer sees; [`dispatch_frames`]
//! runs the single-owner JSON-RPC dispatch loop (R104).
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

pub mod attach;
pub mod durability;
mod external;
pub mod host;
pub mod pane;
pub mod plugins;
pub mod rpc;
pub mod runs;
pub mod scope;
pub mod wire;
pub mod workspace;

pub use attach::{AttachOutcome, AttachmentRegistry, ClientId, ClientInfo};
pub use durability::{
    load_snapshot, restore_allowlist, restore_command, save_if_changed, save_snapshot,
    snapshot_path,
};
pub use host::{
    Host, HostClient, PaneClipboardQuery, PaneClipboardWrite, PaneNotification, PaneScrollFacts,
};
pub use pane::{CellFrame, SpragPaneExternal, send_key, send_text};
pub use plugins::PluginsExternal;
pub use rpc::{
    FrameIngress, HostState, SUPPORTED_METHODS, bump_on_dirty, dispatch_frames, handle_parsed,
    handle_request, pane_exit_hook, spawn_reaper, stdin_frames,
};
pub use runs::{RunId, RunRegistry, RunState};
pub use scope::{ScopeError, SessionScope};
pub use wire::{mux_action_path, pane_container_tag, pane_input_path};
pub use workspace::WorkspaceExternal;

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use pinion_core::scene::{ContainerNode, ExternalNode, TextGridNode};
use pinion_core::style::{LayoutStyle, Size, SizeValue};
use pinion_core::{CellMetric, GridBuffer, Scene, SceneRevision};
use sprag_terminal::{PaneId, PanePty, SessionRegistry};
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

/// The per-pane cell DATA a display client reads to paint a pane — the host side
/// of the topology-B wire contract. Projects the pane's live `screen` scrolled by
/// `offset_lines` rows of history into the paint-authoritative [`GridBuffer`],
/// which is serde-able since PINION-PR49 (R106), so it crosses the wire
/// byte-for-byte — proven by `wire_round_trip_preserves_the_pane_cell_buffer`.
///
/// This is the PROJECTION half of what used to be an all-in-one pane builder. The
/// ASSEMBLY half is [`pane_view_scene_from_cells`], and the IME `preedit` overlay
/// ([`sprag_grid::overlay_preedit`]) is a CLIENT-local step (an uncommitted
/// composition that never reaches the PTY). Topology B splits the three along the
/// wire boundary: the host PROJECTS (here), the client OVERLAYS its preedit and
/// ASSEMBLES the node. The in-process GUI calls this directly on its own pane pool;
/// the end-state wire client reads the serialized `GridBuffer` off the host over
/// RPC and skips this. The `with_screen` lock is released when this returns (the
/// owned buffer needs none), so the client's overlay + assembly run lock-free —
/// unlike the old builder, which held the lock across the whole assembly.
///
/// `offset_lines == 0` is the live view; `n` windows `n` rows up into scrollback
/// ([`sprag_grid::project_scrolled`], which self-clamps to the retained depth and
/// drops the cursor while scrolled — so a client `overlay_preedit` self-gates off
/// a history view with no extra check).
#[must_use]
pub fn pane_cells(pty: &PanePty, offset_lines: usize) -> GridBuffer {
    pty.with_screen(|screen| sprag_grid::project_scrolled(screen, offset_lines))
}

/// Assemble ONE pane's `Scene::Container` from an already-projected cell buffer
/// — the **Screen-free** pane-node SSOT the display client assembles a pane node
/// from (the in-process GUI today; the wire client at the transport step).
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
/// **This is the seam the topology-B display client assembles the pane node from.**
/// The in-process GUI
/// reaches it by projecting its PTY's screen via [`pane_cells`] and overlaying
/// its IME preedit ([`sprag_grid::overlay_preedit`]) first; the end-state wire
/// client feeds cells it reconstructed off the host's served data model — both
/// build the byte-identical pane node without this assembly ever touching a live
/// `Screen`. (The host's own RPC data path uses a different container —
/// `pane_container`, input `External` embedded — see [`workspace_scene`].)
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
/// composite ([`pane_view_scene_from_cells`]). It fills its pane Container (both axes
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
/// no live pty — used by [`snapshot`] and data-only consumers. The RPC
/// server assembles the full pane tree via [`workspace_scene`]; the windowed
/// host builds per-pane scenes via [`pane_view_scene_from_cells`] (from cells it
/// reads through [`pane_cells`]) and arranges them GUI-side.
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
/// [`PanePtyHandle`](sprag_terminal::PanePtyHandle) so `scene/invoke` reaches
/// that pane's PTY.
fn pane_container(id: PaneId, pty: &PanePty) -> Scene {
    let children = pty.with_screen(|screen| {
        vec![
            Scene::TextGrid(text_grid_node(screen, CellMetric::DEFAULT)),
            Scene::External(
                ExternalNode::new(Box::new(pane::SpragPaneExternal::new(pty.handle())))
                    .with_tag(INPUT_TAG),
            ),
        ]
    });
    Scene::Container(ContainerNode::new(children).with_tag(wire::pane_container_tag(id.0)))
}

/// Assemble the window of the session `scope` names as a `Scene::Container` of its panes
/// plus the pane-management [`WorkspaceExternal`] (Round 7 multiplex control core).
///
/// Each pane child is refreshed from its PTY's current screen; the
/// engines and the control surface hold shared handles (a `PanePtyHandle`
/// per pane, an `Arc<Mutex<SessionRegistry>>` for mux control and an
/// `Arc<Mutex<Workspace>>` for the plugin host), so the per-request
/// scene stays a throwaway projection (R969) while input and pane lifecycle
/// reach live state. The workspace lock is released before returning so a
/// dispatched `scene/invoke` (spawn/close/resize) can re-acquire it without
/// deadlock.
///
/// ## The scope is the whole assembly, not a filter on it
///
/// Everything here is built from [`scope`](SessionScope)'s pool, so a request sees exactly
/// the one session it named: its panes are the only `pane_<id>` nodes in the tree, and a
/// pane belonging to another session is not addressable — `scene/invoke` on it answers
/// unknown-path, because the node genuinely is not there. Scoping is therefore structural
/// rather than a check each surface has to remember to make.
///
/// The control external is handed the scope too, and that is load-bearing rather than
/// tidy: it is the one child that reaches PAST the pool to the registry (sessions, windows
/// and layout are mux concerns), so without the scope it would assemble under `work` and
/// write to the default session — pinion's R889 "wrong target for writes", exactly. The
/// plugin host and the pane children need no such care: they are built from the resolved
/// pool and cannot address anything else. That the tightest surface needs the most
/// threading, and the narrow ones none, is the Interface Segregation split paying off.
///
/// `revision` is the shared scene-version token ([`HostState`]'s): the control
/// surface wires each pane it SPAWNS with a `bump_on_dirty(&revision)` hook (so a
/// mux-spawned pane's output wakes parked `scene/waitFor`, exactly as the boot
/// pane's does) and bumps it directly on a spawn / close (so a pane-set change
/// wakes a waiter before the new pane's first output). Without it a client that
/// long-polls change-notification would never learn about mux-spawned panes.
///
/// **v1 bound:** that token is ONE for the whole registry, so a change in any session wakes
/// every attached client, which then re-reads its own scene and finds it unchanged. That is
/// waste, not error — a shared revision can only over-report (`park_if_current` answers a
/// stale baseline and parks a current one; nothing consults it as a write precondition), so
/// no session's request is ever refused or mis-answered because another was busy.
#[must_use]
pub fn workspace_scene(
    scope: &SessionScope,
    registry: &Arc<Mutex<SessionRegistry>>,
    runs: &Arc<Mutex<RunRegistry>>,
    revision: &Arc<SceneRevision>,
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    attachments: Option<Arc<Mutex<AttachmentRegistry>>>,
) -> Scene {
    // The scoped session's pool, resolved when the scope was (never re-derived here — one
    // question, one answer). The registry lock is not held, so taking the workspace lock
    // below cannot nest inside it.
    let workspace = scope.workspace();
    let mut children: Vec<Scene> = {
        let guard = lock(workspace);
        guard
            .panes()
            .iter()
            .map(|pane| pane_container(pane.id(), pane.pty()))
            .collect()
    };
    // The mux control plane speaks the REGISTRY (sessions / windows / layout are mux
    // concerns), so it carries the scope that says WHICH session it may act on...
    children.push(Scene::External(
        ExternalNode::new(Box::new(workspace::WorkspaceExternal::new(
            Arc::clone(registry),
            scope.clone(),
            Arc::clone(revision),
            on_pane_exit.clone(),
            attachments,
        )))
        .with_tag(MUX_TAG),
    ));
    // ...while the plugin host speaks only the pane POOL: a plugin has no business knowing
    // about the session tree (Interface Segregation). It DOES carry the same `on_pane_exit`
    // death-signal, but that is a registry-FREE opaque `Fn` (see [`crate::spawn_reaper`]) — a
    // plugin-spawned pane feeds the reaper exactly like a mux one without the plugin layer ever
    // learning what the hook does, so no pane category can leave a lingering daemon and the ISP
    // boundary is intact.
    children.push(Scene::External(
        ExternalNode::new(Box::new(plugins::PluginsExternal::new(
            Arc::clone(workspace),
            Arc::clone(runs),
            on_pane_exit,
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

    /// Extract the lone projected `TextGrid` from a [`pane_view_scene_from_cells`]
    /// pane Container (the pane wraps the grid in a tagged flex-share Container).
    fn pane_grid(scene: &Scene) -> &TextGridNode {
        match scene {
            Scene::Container(c) => match c.children.first() {
                Some(Scene::TextGrid(node)) => node,
                other => unreachable!("a pane Container holds one TextGrid, got {other:?}"),
            },
            other => unreachable!("pane_view_scene_from_cells returns a Container, got {other:?}"),
        }
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
}
