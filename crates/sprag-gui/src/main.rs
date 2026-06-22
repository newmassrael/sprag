//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R38).
//!
//! A window that tiles N terminal panes, paints each one's **live** screen (and
//! its scrollback history), and types into the focused one. It is the human
//! observation/interaction path; the north star (an AI reading/driving the
//! terminal as *data*) is the headless `sprag-host` RPC path, which needs none of
//! this. This binding is a faithful pixel projection of the *same* cell data the
//! AI reads — it reuses the host's per-pane projection seam
//! ([`sprag_host::pane_view_scene`]) and arranges the panes itself (the
//! interactive [`split`] layout — reactive ratios the headless host has no use
//! for) — and routes keystrokes through the *same* [`SpragPaneExternal`]
//! `invoke("key", ...)` wire the AI drives (§2 #2).
//!
//! ## Multi-pane (R36): N tiled panes, one focused
//!
//! [`pane_count`](terminal::pane_count) panes (default 2, `SPRAG_GUI_PANES=<n>`,
//! capped at [`MAX_PANES`](terminal::MAX_PANES)) are spawned at boot and tiled
//! left-to-right. Each pane has a single identity [`pane_tag`](terminal::pane_tag)
//! that is its model-scene input External tag (input routing), its focus tag, its
//! paint-scene Container tag (the pinion R1012
//! [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect target +
//! framework focus ring + click-focus anchor), and its per-pane reflow Effect tag
//! — one string so input / focus / measure / paint can never address different
//! panes. Keyboard focusability is **scene-derived** (pinion R1020 §5.39): the
//! pane's paint Container is marked `with_focusable(true)` in
//! [`sprag_host::pane_view_scene`] and the shell collects the Tab order each frame
//! via [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
//! — there is no binding-side `focusable_tags()` list. The model scene is `Container([pane0, ...panesN])`
//! ([`WidgetCore::create_external`] is pane 0; [`WidgetCore::create_extra_externals`]
//! the rest), so [`WidgetCore::apply_key`] reaches the focused pane by
//! `find_external_with_tag_mut(focused)`. `Ctrl+PageUp/Down` cycles focus; the
//! framework draws the focus ring around the active pane. Per-pane GUI state
//! (scroll offset, IME preedit) is keyed by tile index ([`input`]).
//!
//! ## Dock / undock (R37): a pane in its own OS window
//!
//! `Ctrl+Shift+Enter` toggles the focused pane between **docked** (tiled in the
//! main window) and **undocked** (painted alone in its own OS window), via
//! pinion's multi-window seam: [`WidgetView::windows_signal`] returns the
//! [`dock`] topology `Signal<Vec<WindowSpec>>` (the floating SSOT — a pane floats
//! iff its `pane-{i}` window exists), and [`WidgetView::view_for_window`]
//! dispatches per window (main tiles the docked panes; `pane-{i}` paints that
//! pane). **Input is unchanged**: the model scene + focus are global, so a
//! keystroke reaches the focused pane's PTY regardless of which window paints it
//! — dock/undock changes only *where* a pane is painted, never its tag /
//! External / focusability (so it does NOT need runtime-focusable changes).
//! **Honest v1 bound**: the undock window opens fixed to the pane's intrinsic
//! `(cols, rows) × cell` and does NOT reflow on OS resize — pinion's R1006/R1012
//! viewport publishes are gated to the default window (the per-window publish is
//! a reported gap; see `dock` docs). Per-window a11y partitions nodes
//! ([`a11y::access_nodes_for_window`]).
//!
//! ## Draggable dividers (R38): drag to resize panes
//!
//! The docked panes are arranged by [`split::view_split_row`] with pinion
//! `view_splitter` dividers you can DRAG to resize (even at boot; N>2 nests N-1
//! splitters). Each divider's ratio is an `Owner::cache`-shared `Signal<f32>`: the
//! view reads it, and a [`SplitterExternal`](pinion_widget_paint::splitter::SplitterExternal)
//! registered at the handle tag ([`create_extra_externals`](TerminalViewer))
//! writes it on a pointer drag (the shell's pointer router delivers the drag — no
//! `WidgetCore` pointer method). A drag re-weights the flex layout -> the pane
//! rects change -> the R1012 reflow Effects resize the PTYs (automatic; the
//! `reflow` seam was built for it). The interactive layout lives GUI-side (not in
//! the host) because it needs reactive ratios + a registered External.
//!
//! ## Layout: row or grid (R40)
//!
//! `SPRAG_GUI_LAYOUT=grid` arranges the panes in a balanced 2D grid (rows of
//! columns) instead of the default 1D row ([`split::layout_mode`]). The grid reuses
//! the SAME `view_splitter` primitive — an outer Vertical splitter stack of rows,
//! each row an inner Horizontal stack of panes ([`split::view_grid`] over
//! [`split::grid_plan`]) — and the SAME per-pane R1012 reflow (layout-agnostic, so
//! no reflow change). A divider's drag-axis (Horizontal vs Vertical) is welded at
//! boot when its `SplitterExternal` is registered, and a balanced grid's
//! orientations depend on the pane count, so grid mode does NOT reshape on undock
//! (which would change a count): a floated pane HOLDS its grid slot (an untagged
//! empty cell) rather than compacting like row mode. Both modes draw their N-1
//! dividers from the one `SPLITTER_TAGS` table; no pinion change (`SplitterOrientation::Vertical`
//! already ships). See [`split`] for the row<->grid asymmetry rationale.
//!
//! ## Module map (R32, R36, R37, R38)
//!
//! The binding is split by concern so each axis grows in one place:
//!
//! - [`terminal`] — the booted [`TerminalView`](terminal::TerminalView) model
//!   (N panes), the [`pane_tag`](terminal::pane_tag) / [`pane_count`](terminal::pane_count)
//!   identity SSOT, font/command config, and the [`grid_dims`](terminal::grid_dims)
//!   winsize SSOT; [`use_terminal`] self-creates it.
//! - [`reflow`] — the per-pane resize -> PTY reflow [`Effect`](pinion_core::reactive::Effect)s
//!   ([`install_reflow`]).
//! - [`input`] — focused-pane keystroke / IME commit -> PTY routing
//!   ([`route_key`] / [`route_composition`]), the focus-cycle + dock-toggle
//!   chords, and the per-pane scrollback-view offset / preedit.
//! - [`dock`] — which OS window paints each pane: the topology
//!   [`Signal`](pinion_core::reactive::Signal) (floating SSOT) +
//!   [`toggle_pane_floating`](dock::toggle_pane_floating).
//! - [`split`] — the draggable-divider layout: the per-divider ratio Signals, the
//!   shared [`view_split_row`](split::view_split_row) / [`view_grid`](split::view_grid)
//!   splitter fold, and the `SPRAG_GUI_LAYOUT` row/grid mode ([`layout_mode`](split::layout_mode)).
//! - [`a11y`] — the per-pane (per-window) accessible-node projection (human-AT).
//! - [`view`] — the per-window paint ([`view::view_for_window`]: main tiling /
//!   single undocked pane) + the surface-filled paint root.
//!
//! ## How it stays on the substrate (no hacks)
//!
//! A child process writes its PTY from a **separate OS thread**, so the static
//! `view` must read changing, cross-thread (`Send`) data and the window must
//! repaint on change without owning the event loop. The seams (all pinion):
//!
//! - [`use_terminal`] self-creates the
//!   [`Workspace`](sprag_terminal::Workspace) + the N panes once in an
//!   `Owner::cache` hook (the `use_storage` pattern — nothing flows through
//!   `main`), spawned in [`WidgetCore::create_extra_externals`] at boot.
//! - Each pane is spawned via [`Workspace::spawn_with_dirty`](sprag_terminal::Workspace::spawn_with_dirty)
//!   (the sprag R23 hook) with an `on_dirty` that calls
//!   [`RepaintSink::request_repaint`](pinion_core::RepaintSink::request_repaint)
//!   (pinion R999) — any pane's PTY batch wakes the shell; the next frame's `view`
//!   re-reads the screens. Event-driven, no polling. `State` stays `()` (`Copy`).
//! - The cell is **measured once** at boot from the resolved monospace via
//!   pinion R1003 [`measured_monospace_cell`](pinion_core::measured_monospace_cell)
//!   (the shell seeds the provider before factories) — no private `FontContext`,
//!   no double measurement. `pane_view_scene` pins that size on each node so the
//!   painted advance equals `cell_w` (R1002).
//!
//! ## Winsize (§3): the window drives the size, resize reflows each PTY (R26, R36)
//!
//! The window opens at [`WINDOW_W`] x [`WINDOW_H`]; the tiling fills it, so each
//! pane's resolved sub-rect IS its viewport and its `(cols, rows)` derive from
//! `sub-rect / cell` ([`grid_dims`](terminal::grid_dims), the single derivation
//! SSOT). Each pane's PTY boots at the full-window dims (the honest pre-layout
//! value) and a **per-pane** reflow Effect ([`install_reflow`]) keeps it live: it
//! subscribes to pinion's R1012 per-pane viewport-size
//! [`Signal`](pinion_core::reactive::Signal) (`use_pane_viewport_size(pane_tag)`)
//! and, whenever that pane's measured rect changes (an OS resize re-divides the
//! tiles), re-derives `(cols, rows)` and reflows that pane (`TIOCSWINSZ`). The
//! reflow is a real side-effect (an ioctl on a live fd), so it lives in an
//! [`Effect`](pinion_core::reactive::Effect) — gated out of `dry_run` / snapshot
//! paint by the seam — never the pure `view`. No window-side split math: the
//! authoritative sub-rect comes from pinion's post-layout measure, not a
//! consumer-replicated flex calc (the SSOT trap).
//!
//! ## Input (R27, R36): keystroke -> the focused pane's PTY
//!
//! The model scene is `Container([pane0, ...panesN])`, each pane an
//! [`SpragPaneExternal`] tagged its [`pane_tag`](terminal::pane_tag) (built in
//! [`WidgetCore::create_external`] / [`WidgetCore::create_extra_externals`] over each
//! pane's `SessionHandle`). Each pane is a focusable tab stop (its paint Container
//! is `with_focusable(true)`, R1020 scene-derived focus — above), so
//! [`WidgetCore::apply_key`] routes a focused keystroke + W3C modifiers to the
//! **focused** pane's External (`find_external_with_tag_mut(focused)`) via
//! `invoke("key", {key, ctrl, alt, shift, super})` — the *same* `scene/invoke`
//! channel the RPC client uses, where the sprag-owned encoder
//! ([`sprag_input`](https://docs.rs/sprag-input)) turns the key into PTY bytes
//! (R2.6). Returning `true` swallows Escape/Tab from the shell's quit/traverse
//! defaults so a full-screen TUI (vim) receives them. `Ctrl+PageUp/Down` is
//! reserved to cycle focus between tiles (a pinion `focus_request`, not the PTY).
//!
//! IME-composed input (R31, R34) — Hangul, CJK — arrives not as keystrokes but
//! as [`WidgetCore::apply_composition`] events targeting the focused pane. The
//! in-progress preedit is mirrored into that pane's
//! [`use_preedit`](input::use_preedit) overlay Signal and drawn underlined at its
//! cursor by `view` (R34 — see [`route_composition`] and `sprag_grid::overlay_preedit`
//! for why a terminal renders the preedit itself); on
//! [`CompositionEvent::Commit`] the overlay clears and the finished text is
//! written *literally* (no key-encoding) through the focused pane's sibling
//! `invoke("text", …)` wire.
//!
//! ## Scrollback (R29, R36): scroll a pane's history view
//!
//! `Shift+PageUp` / `Shift+PageDown` scroll a view over the **focused pane's**
//! history (a per-pane [`Signal`](pinion_core::reactive::Signal)-backed offset in
//! lines from the live bottom; [`route_key`] writes it, `view` reads it, so a
//! scroll re-renders that pane). The view reuses the host's per-pane projection
//! seam at its scrolled entry, so history and the live screen share one
//! authority. The scroll keys do NOT reach the PTY; any other key snaps that pane
//! back to the live bottom (you type at the prompt). Scrolled history is
//! **text-only** (the R16 scrollback model keeps text, not cells) — it renders in
//! default colors; the live screen is exact. `offset == 0` follows the bottom
//! with no drift; scrolling *during* active output may shift (the offset is
//! relative to the live bottom) — a v1 limit.

mod a11y;
mod dock;
mod input;
mod reflow;
mod scrollbar;
mod split;
mod terminal;
mod view;

use pinion_a11y::AccessNode;
use pinion_core::external::External;
use pinion_core::reactive::Signal;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{SizeStrategy, WidgetView, WindowSpec, vello_renderer_impl};
use pinion_widget_paint::splitter::SplitterExternal;
use sprag_host::SpragPaneExternal;
use std::rc::Rc;

use crate::input::{route_composition, route_key};
use crate::reflow::install_reflow;
use crate::terminal::{pane_tag, use_terminal};

include!(concat!(env!("OUT_DIR"), "/app.rs"));
vello_renderer_impl!(SpragGuiRenderer, SpragGuiRendererError);

/// The window's initial logical-pixel size. The grid fills it and the terminal
/// `(cols, rows)` derive from `WINDOW / measured-cell`
/// ([`grid_dims`](terminal::grid_dims); §3 — the window drives the winsize, the
/// inverse of the headless host). The window stays resizable; live resize -> PTY
/// reflow keeps the dims current via the [`install_reflow`]
/// Effect.
const WINDOW_W: u32 = 960;
const WINDOW_H: u32 = 600;

/// The paint-root / window-background `Scene::Container` tag (the surface fill
/// behind the tiled panes; see [`view::view`]). NOT a pane or a focus stop — the
/// per-pane identity + focus tags are [`pane_tag`] (`PANE_TAGS`), and the primary
/// input External is tagged `pane_tag(0)` via [`TerminalViewer::tag`].
const ROOT_TAG: &str = "sprag_gui";

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    type State = ();
    type Event = ();

    /// The primary input External is **pane 0**'s [`SpragPaneExternal`] (tagged
    /// [`Self::tag`] == `pane_tag(0)`) over its live `SessionHandle`; panes 1.. are
    /// the [`Self::create_extra_externals`] extras, so the shell's model scene is
    /// `Container([pane0, ...panesN])`. The shell runs this inside the root Owner
    /// scope, so [`use_terminal`] resolves the (already-booted) panes here.
    /// [`Self::apply_key`] routes a keystroke to the FOCUSED pane's External
    /// (`find_external_with_tag_mut(pane_tag)` -> `invoke("key", ...)`) — the
    /// **same** wire the headless RPC path drives (one input substrate, §2 #2;
    /// key->PTY-byte encoding is sprag's, R2.6).
    fn create_external() -> Box<dyn External> {
        Box::new(SpragPaneExternal::new(use_terminal().pane_handle(0)))
    }

    /// Boot the live terminal (spawn the N pane PTYs + wire each `on_dirty` ->
    /// repaint) AND install the per-pane resize -> reflow
    /// [`Effect`](pinion_core::reactive::Effect)s (pinion R1012), before the first
    /// paint and off the pure `view`. [`install_reflow`] resolves [`use_terminal`]
    /// (booting the panes) and wires every pane's reflow in one call. Then focus
    /// pane 0 so keystrokes reach a pane without a click — the shell drains this
    /// focus request before the first paint. Returns panes 1.. as the extra input
    /// Externals (pane 0 is the primary), each tagged its [`pane_tag`].
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let terminal = use_terminal();
        install_reflow();
        pinion_core::focus_request::request(pane_tag(0));
        // Panes 1.. are the extra input Externals (pane 0 is the primary).
        let mut externals: Vec<ExtraExternal> = (1..terminal.pane_count())
            .map(|i| {
                ExtraExternal::new(
                    pane_tag(i),
                    Box::new(SpragPaneExternal::new(terminal.pane_handle(i))),
                )
            })
            .collect();
        // The draggable dividers: one `SplitterExternal` per divider, its
        // orientation chosen by [`split::divider_orientations`] for the current
        // `SPRAG_GUI_LAYOUT` + `pane_count` (row mode: all Horizontal; grid mode:
        // the `grid_plan` column/row mix — same SSOT the view fold reads, so a
        // divider's drag-axis matches the painted divider). The count is
        // `pane_count - 1` in both modes (a binary split-tree of N leaves has N-1
        // dividers — the max the view ever paints, all panes docked). Each shares
        // the ratio `Signal` the view reads; the shell's pointer router drives them
        // (no `WidgetCore` pointer method). NOT focusable tab stops (pointer-only —
        // their handle nodes carry no `with_focusable`, so R1020's per-frame
        // `collect_focusable_tags` never enumerates them). Boot-only: a grid's
        // divider orientations
        // are welded here, so grid mode holds floated slots rather than reshaping
        // (see [`split`] / [`view::view_main`]).
        externals.extend(
            split::divider_orientations(terminal.pane_count())
                .into_iter()
                .enumerate()
                .map(|(j, orientation)| {
                    ExtraExternal::new(
                        split::splitter_tag(j),
                        Box::new(
                            SplitterExternal::new(orientation)
                                .attach_ratio(split::use_splitter_ratio(j)),
                        ),
                    )
                }),
        );
        // One draggable scrollbar peer per pane (R49): a `ScrollBarExternal` over
        // the pane's row-unit `ScrollState`, tagged its `scrollbar.{i}` so the
        // shell's pointer router routes a press on the painted track to it. Like
        // the splitters, pointer-only (never focusable) and boot-registered; the
        // caller-owned `ScrollState` authority (pinion R1032) needs no mirror.
        externals.extend((0..terminal.pane_count()).map(scrollbar::pane_scrollbar_external));
        externals
    }

    /// Route a focused keystroke to the focused pane's PTY — delegates to
    /// [`route_key`] (the roving-tabindex focus gate + the focused pane's
    /// `invoke("key", ...)` wire + the focus-cycle / scrollback chords).
    fn apply_key(
        scene: &mut Scene,
        focused: Option<&str>,
        key: &str,
        modifiers: Modifiers,
    ) -> bool {
        route_key(scene, focused, key, modifiers)
    }

    /// Route committed IME text to the focused pane's PTY — delegates to
    /// [`route_composition`] (the focus gate + the focused pane's literal
    /// `invoke("text", ...)` wire + its preedit overlay).
    fn apply_composition(
        scene: &mut Scene,
        focused: Option<&str>,
        event: &CompositionEvent,
    ) -> bool {
        route_composition(scene, focused, event)
    }

    /// Pane 0's identity tag — the primary input External
    /// ([`Self::create_external`]) is tagged with it (the model-scene primary's
    /// tag is `V::tag()`), so it joins panes 1.. (the extras) under one uniform
    /// [`pane_tag`] addressing.
    fn tag() -> &'static str {
        pane_tag(0)
    }

    fn read_state(_scene: &Scene) {}

    /// The windowless / RPC-snapshot fallback paints the **main** window (the
    /// docked panes). The live multi-window paint goes through
    /// [`WidgetView::view_for_window`]; this keeps the no-window-context path
    /// (an RPC `scene/snapshot` without a window) showing what the human sees.
    fn view(state: (), frame: &Frame) -> Scene {
        view::view_for_window(dock::MAIN_WINDOW_ID, state, frame)
    }

    fn event_name(_event: ()) -> &'static str {
        "__internal__"
    }

    // Keyboard focus enumeration is no longer a binding-side method: pinion R1020
    // §5.39 removed `WidgetCore::focusable_tags()` and DERIVES the Tab order each
    // frame from the paint scene via `Scene::collect_focusable_tags` — the
    // `focusable`-marked, tagged nodes. Each pane declares itself a Tab stop where
    // it is painted: `sprag_host::pane_view_scene` marks the pane Container
    // `with_focusable(true)` (one Tab stop per pane; its inner grid is not focused).
    // Focusing a pane gates [`Self::apply_key`] to it, the framework draws its focus
    // ring, and a click on its rect re-focuses it; `Ctrl+PageUp/Down` cycles between
    // them ([`route_key`]). [`Self::create_extra_externals`] requests focus on pane 0
    // at boot so typing works without a click — the shell re-derives that request
    // against the first paint scene's collected tags (pane 0 is among them).

    fn title() -> &'static str {
        "sprag terminal (interactive)"
    }
}

impl WidgetView for TerminalViewer {
    type Renderer = SpragGuiRenderer;

    fn initial_size_strategy() -> SizeStrategy {
        SizeStrategy::Fixed {
            width: WINDOW_W,
            height: WINDOW_H,
        }
    }

    /// Opt into runtime windows: the dock topology Signal (the floating SSOT,
    /// [`dock::use_windows_topology`]). The shell subscribes it and reconciles
    /// winit windows on each `set` — the `Ctrl+Shift+Enter` chord
    /// ([`dock::toggle_pane_floating`] via [`route_key`]) undocks/docks the
    /// focused pane.
    fn windows_signal() -> Option<Rc<Signal<Vec<WindowSpec>>>> {
        Some(dock::use_windows_topology())
    }

    /// Per-window paint: the main window tiles the DOCKED panes; an undock window
    /// (`pane-{i}`) paints that pane alone. Delegates to [`view::view_for_window`].
    fn view_for_window(window_id: &str, state: (), frame: &Frame) -> Scene {
        view::view_for_window(window_id, state, frame)
    }

    /// Per-window a11y: each window advertises only the panes IT paints (the main
    /// window the docked panes; an undock window its one pane), so a sibling
    /// window's AT tree carries no ghost pane nodes. Delegates to
    /// [`a11y::access_nodes_for_window`].
    fn access_node_for_window(
        window_id: &str,
        _state: &(),
        focused: Option<&str>,
    ) -> Vec<AccessNode> {
        a11y::access_nodes_for_window(window_id, focused)
    }
}

fn main() {
    pinion_shell::run::<TerminalViewer>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_shell::ShellCore;

    /// Collect every painted `TextGrid`'s `(cols, rows)` by walking the scene
    /// tree (the pane grids sit inside the tiling Containers; the focus-ring
    /// overlay is a `Box`, not a grid, so it is ignored).
    fn painted_grid_dims(scene: &Scene) -> Vec<(u16, u16)> {
        match scene {
            Scene::TextGrid(node) => vec![(node.cells().cols(), node.cells().rows())],
            Scene::Container(c) => c.children.iter().flat_map(painted_grid_dims).collect(),
            Scene::Scroll(s) => painted_grid_dims(&s.content),
            _ => Vec::new(),
        }
    }

    /// End-to-end multi-pane reflow through the REAL `TerminalViewer` driven by
    /// the shell's live paint path: build the shell (boots 2 panes, installs the
    /// per-pane R1012 reflow Effects), drive one `compute_paint_scene`, and assert
    /// each pane's painted grid reflowed to ITS measured half-rect (not the full
    /// window) — same-frame (the publish dirty-bit re-pass). pinion's
    /// `pane_viewport_seam.rs` proves the seam with a mock widget; this proves
    /// sprag tags/registers/derives correctly so the real viewer tiles and
    /// reflows. The shell seeds the monospace font provider, so the cell is the
    /// real measured metric (read back from the booted terminal), not
    /// `CellMetric::DEFAULT`.
    #[test]
    fn tiles_two_panes_each_reflowed_to_its_half_rect() {
        // Default env: 2 panes. (No SPRAG_GUI_PANES set in the test process.)
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let dims = painted_grid_dims(&scene);
        // The actual measured cell the shell sized the panes with (its monospace
        // provider is seeded, so this is NOT CellMetric::DEFAULT) — read from the
        // booted terminal in the shell's own root owner, then derive the
        // full-window dims at that cell to compare against.
        let metric = core.root_owner().run(|| use_terminal().metric);
        let (full_cols, full_rows) = crate::terminal::grid_dims((WINDOW_W, WINDOW_H), metric);

        assert_eq!(
            dims.len(),
            2,
            "two tiled pane grids are painted, got {dims:?}"
        );
        for (cols, rows) in &dims {
            // Horizontal split: each pane keeps the full height...
            assert_eq!(*rows, full_rows, "a horizontal split preserves pane height");
            // ...but is reflowed to roughly half the width — strictly narrower
            // than a full-window pane (the per-pane R1012 reflow shrank it from
            // its full-window boot size), same-frame.
            assert!(
                *cols < full_cols && *cols >= full_cols / 2 - 3,
                "pane reflowed to ~half ({cols} cols vs full {full_cols})",
            );
        }
        // The even split makes the two panes within one cell of each other.
        assert!(
            dims[0].0.abs_diff(dims[1].0) <= 1,
            "the two panes split the window evenly, got {dims:?}",
        );
    }

    /// R1020 §5.39 scene-derived focus: the REAL viewer's paint scene marks every
    /// tiled pane Container `with_focusable(true)`, so the shell's per-frame
    /// [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
    /// walk enumerates both pane tags as Tab stops — and ONLY those (the surface
    /// root and the splitter handle are not focus stops). This is the contract that
    /// replaced the removed binding-side `focusable_tags()`; a regression here would
    /// drop focus to `None` every frame (no focus ring, dead keyboard input), so it
    /// is pinned end-to-end through the live paint path.
    #[test]
    fn panes_are_scene_derived_focus_stops() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let focusable = scene.collect_focusable_tags();
        assert!(
            focusable.iter().any(|t| t == pane_tag(0)),
            "pane 0 is a scene-derived Tab stop, got {focusable:?}",
        );
        assert!(
            focusable.iter().any(|t| t == pane_tag(1)),
            "pane 1 is a scene-derived Tab stop, got {focusable:?}",
        );
        // Exactly the two panes — the surface root and the divider handle are not
        // focus stops (no `with_focusable`), so they never enter the Tab order.
        assert_eq!(
            focusable.len(),
            2,
            "only the two panes are focusable, got {focusable:?}",
        );
        assert!(
            !focusable.iter().any(|t| t == ROOT_TAG),
            "the surface root is not a Tab stop",
        );
    }

    /// R1020 §5.39 boot focus: [`create_extra_externals`](TerminalViewer::create_extra_externals)
    /// requests focus on pane 0 at boot; once the dispatch tail drains
    /// ([`ShellCore::finalize_frame`]), that request lands on `pane_tag(0)` — the
    /// drain re-derives the focusable set from the painted scene (where the pane is
    /// `with_focusable`), so the request resolves and the framework frames pane 0
    /// with its focus ring. Pre-migration (the removed `focusable_tags()` list) this
    /// was a binding-side seed; this proves the scene-derived path keeps typing
    /// working without a click.
    #[test]
    fn boot_focus_lands_on_pane_zero() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // First paint seeds the scene-derived enumeration (both panes focusable).
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        // Drive the dispatch tail so the boot focus_request drains and resolves.
        core.finalize_frame(scene);
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "boot focus lands on pane 0 -> the framework frames it with the focus ring",
        );
    }

    /// Click-to-focus: a mouse press inside a pane focuses THAT pane, so typing
    /// then reaches it. The pane's grid is tagged `{pane_tag}#grid` (a pinion
    /// composite sub-tag), so a press on the grid — the deepest tagged node under
    /// the pointer — resolves through `resolve_focusable` (split on `#`) to the
    /// focusable pane tag. With a plain, non-composite grid tag the press resolved
    /// to nothing and focus never moved — the live bug where clicking the right
    /// pane kept typing in the left. Driven through the REAL shell pointer path
    /// (`cursor_moved` -> `mouse_pressed` -> `click_to_focus`).
    #[test]
    fn clicking_a_pane_focuses_it() {
        use pinion_runtime::PointerId;
        let mut core = ShellCore::<TerminalViewer>::new();
        // Boot-focus pane 0 (the dispatch tail drains the boot focus_request).
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        assert_eq!(core.focus().focused(), Some(pane_tag(0)), "boots on pane 0");

        // The router hit-tests against the last painted scene; take pane centers.
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let center = |tag: &str| {
            let r = scene
                .rect_for_tag_absolute(tag)
                .unwrap_or_else(|| panic!("{tag} painted"));
            (
                f64::from(r.x) + f64::from(r.w) / 2.0,
                f64::from(r.y) + f64::from(r.h) / 2.0,
            )
        };
        let click = |core: &mut ShellCore<TerminalViewer>, (x, y): (f64, f64)| {
            core.cursor_moved_for_window(dock::MAIN_WINDOW_ID, PointerId::MOUSE, x, y);
            core.mouse_pressed_for_window(dock::MAIN_WINDOW_ID, PointerId::MOUSE);
        };

        // Click pane 1 -> focus moves to pane 1 (the bug: it stayed on pane 0).
        click(&mut core, center(pane_tag(1)));
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(1)),
            "clicking inside pane 1 focuses pane 1",
        );
        // Click back into pane 0 -> focus returns.
        click(&mut core, center(pane_tag(0)));
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "clicking inside pane 0 focuses pane 0",
        );
    }

    /// End-to-end dock/undock through the REAL `TerminalViewer` + the shell's
    /// per-window paint dispatch: undock pane 1 and assert the main window drops
    /// it while the `pane-1` undock window paints exactly it — the docked/floating
    /// partition as scene DATA. The secondary-window paint runs pure geometry
    /// (R1006/R1012 publishes are default-window-gated — the documented gap), so
    /// this asserts the partition, not a secondary reflow; the undock window is
    /// driven at the pane's own size.
    #[test]
    fn undock_partitions_panes_across_windows() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // Boot: both panes docked -> the main window tiles both.
        let main0 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(main0.contains_tag(pane_tag(0)), "main tiles pane 0");
        assert!(main0.contains_tag(pane_tag(1)), "main tiles pane 1");

        // Undock pane 1 (the chord runs in the shell root owner scope).
        core.root_owner().run(|| dock::toggle_pane_floating(1));

        // The main window now drops the floated pane 1, keeps the docked pane 0.
        let main1 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(
            main1.contains_tag(pane_tag(0)),
            "main keeps the docked pane 0"
        );
        assert!(
            !main1.contains_tag(pane_tag(1)),
            "main drops the floated pane 1"
        );

        // The undock window paints exactly pane 1 (not pane 0).
        let undock = core.compute_paint_scene_for_window(&dock::pane_window_id(1), 400, 300);
        assert!(
            undock.contains_tag(pane_tag(1)),
            "the undock window paints pane 1"
        );
        assert!(
            !undock.contains_tag(pane_tag(0)),
            "the undock window does not paint pane 0"
        );

        // Dock back: the main window tiles pane 1 again.
        core.root_owner().run(|| dock::toggle_pane_floating(1));
        let main2 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(main2.contains_tag(pane_tag(1)), "dock-back re-tiles pane 1");
    }

    /// End-to-end divider drag through the REAL viewer: setting divider 0's ratio
    /// Signal (the exact write a pointer drag performs via `SplitterExternal`)
    /// re-weights the two panes — the left pane reflows wider, tracking ~0.7. Proves
    /// the read side (`view_split_row` -> `view_splitter` -> layout -> R1012 reflow)
    /// sprag wires; the pointer->Signal write side is pinion's `SplitterExternal`
    /// (its own tests) + the live drag smoke.
    #[test]
    fn dragging_a_divider_reweights_the_panes() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // Even split at boot — the two panes are within a cell of each other.
        let even = painted_grid_dims(&core.compute_paint_scene(WINDOW_W, WINDOW_H));
        assert_eq!(even.len(), 2, "two panes, got {even:?}");
        assert!(
            even[0].0.abs_diff(even[1].0) <= 1,
            "boots even, got {even:?}"
        );

        // Drag divider 0 to a 0.7 left-share (the same Signal a pointer drag sets).
        core.root_owner()
            .run(|| split::use_splitter_ratio(0).set(0.7));
        let weighted = painted_grid_dims(&core.compute_paint_scene(WINDOW_W, WINDOW_H));
        assert_eq!(weighted.len(), 2);
        let (left, right) = (weighted[0].0, weighted[1].0);
        assert!(
            left > right,
            "left pane reflowed wider after ratio 0.7, got {weighted:?}"
        );
        let left_frac = f32::from(left) / f32::from(left + right);
        assert!(
            (left_frac - 0.7).abs() < 0.06,
            "the split tracks ~0.7 (got {left_frac:.2}, {weighted:?})",
        );
    }

    /// End-to-end drag wiring (R49): the real viewer paints a scrollbar track
    /// tagged its `scrollbar.{i}` for each pane — the hit target the shell's pointer
    /// router routes a press to, for the `ScrollBarExternal` registered at the same
    /// tag ([`create_extra_externals`] -> [`scrollbar::pane_scrollbar_external`]).
    /// This is the integration claim sprag owns (paint a tagged track + register a
    /// peer at that tag); the pointer->`scroll_to` write is pinion's
    /// `ScrollBarExternal` (its own tests), and the `offset_y`->thumb read is the
    /// [`scrollbar`] unit tests. Mirrors how the splitter trusts pinion for its
    /// write side.
    #[test]
    fn each_pane_paints_a_tagged_scrollbar_track() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        assert!(
            scene.contains_tag(scrollbar::scrollbar_tag(0)),
            "pane 0 paints a tagged scrollbar track (the drag hit target)",
        );
        assert!(
            scene.contains_tag(scrollbar::scrollbar_tag(1)),
            "pane 1 paints a tagged scrollbar track",
        );
    }

    /// Undocking every pane leaves the main window empty — it must not panic
    /// (`view_split_row(vec![])` is a childless Container) and paints no pane.
    #[test]
    fn undock_all_panes_yields_an_empty_main_without_panic() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let n = core.root_owner().run(|| use_terminal().pane_count());
        core.root_owner().run(|| {
            for i in 0..n {
                dock::toggle_pane_floating(i);
            }
        });
        let main = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        for i in 0..n {
            assert!(
                !main.contains_tag(pane_tag(i)),
                "pane {i} is floated, not in main"
            );
        }
    }
}
