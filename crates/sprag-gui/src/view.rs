//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::ROOT_TAG;
use crate::dock::pane_window_index;
use crate::input::use_preedit;
use crate::slotview::SlotView;
use crate::split::{
    pane_index_of_panel, panel_id, use_dock_topology, use_drop_preview, use_split_ratio,
};
use crate::terminal::{TerminalView, pane_index_of, pane_tag, use_terminal};
use pinion_core::external::OUTER_DOCK_ZONE_TAG;
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{ColorRole, Theme, use_theme};
use pinion_core::{Frame, Scene};
use pinion_shell::{
    WINDOW_CHROME_CLOSE_TAG, WINDOW_CHROME_MAXIMIZE_TAG, WINDOW_CHROME_MINIMIZE_TAG,
};
use pinion_widget_paint::dock::{
    DockPanelChrome, DockPanelStyle, DockSplitState, WindowControlTags, dock_outer_zone_highlight,
    view_dock_panel_with_actions, view_dock_surface_chrome, view_window_controls,
};
use std::borrow::Cow;

/// Shared [`ThemeProvider`](pinion_core::ThemeProvider) cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";

/// The DISPLAY title of the pane in tile `i` (R128) — the ONE home for the fallback rule,
/// read by EVERY title surface: the docked panel header + tab label (via
/// [`view_dock_surface_chrome`]'s `DockPanelChrome::with_title`, R130), the floater header
/// (this fn's `view_for_window` arm), the torn-off placeholder label (R129), the floater's
/// OS title, and — for the focused pane — the main window's OS title (both via
/// [`crate::dock`], R130/R132). All PR52/PR53 surfaces now consume it.
///
/// Prefers the child's live `OSC 0` / `OSC 2` window title — what tmux / `gnome-terminal`
/// show (`vim README`, `coin@host:~`, an ssh remote) — and falls back to the stable
/// [`panel_id`] when the child has set none, or set a BLANK one (which must not blank the
/// header).
///
/// **Display only.** IDENTITY — the dock-leaf [`panel_id`], scene tags, focus, RPC paths —
/// never derives from this: a child sets its title freely and rewrites it on every prompt,
/// so deriving identity from it would let a pane rename its own address (R70).
pub(crate) fn pane_display_title(slots: &SlotView, i: usize) -> String {
    slots
        .pane_title(i)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| panel_id(i))
}

/// view-fn (§6.3): per-window paint. The **main** window tiles the DOCKED panes
/// (those without an undock window); an **undock window** (`pane-{i}`) paints that
/// one pane as a single [`view_dock_panel_with_actions`]
/// — a draggable header (the drag source the same per-pane `DockPanelExternal` routes
/// from, so a SETTLED floating window can be re-grabbed / dragged back onto the dock;
/// since R95 also the floater's TITLE BAR, hosting its window controls) above the
/// pane content. [`WidgetCore::view`](crate::TerminalViewer) (the windowless /
/// RPC-snapshot fallback) routes here as the main window. The producer threads (the PTY
/// readers) live in `create_extra_externals`, not here.
pub(crate) fn view_for_window(
    window_id: &str,
    state: crate::ctxmenu::MenuState,
    _frame: &Frame,
) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    match pane_window_index(window_id) {
        // An undock window paints its one pane wrapped in a dock panel (header +
        // content), mirroring pinion's `hello-dock-panels-editor` `view_floating_panel`
        // — NO outer `compose` (the panel IS the window root). The content is
        // `fill_definite_shrinkable` so the pane reflows to a window SMALLER than its boot
        // content (the floating-window reflow path, see that fn). A stale window id (pane
        // closed) falls back to the main layout, never a stranded paint.
        //
        // Controls-in-header (R95, pinion R1171/R1186/R1187 — PR-43): the floater is
        // chrome-less (`window_chrome == None`), so this dock header IS its title bar —
        // ONE strip, not a chrome bar stacked over a tab header. The header hosts the
        // window controls (min / max / close) via the lifted `view_window_controls`;
        // the shell's routing tags are supplied HERE (the binding owns the window
        // lifecycle) so `try_chrome_press` routes close → `window_close_requested`
        // (dock-back, R86), min / max → per-window `set_minimized` / `set_maximized`.
        Some(i) if tv.slots.is_pane_occupied(i) => {
            let style = DockPanelStyle::m3_default(panel_id(i));
            let controls = view_window_controls(
                &theme,
                style.header_font_size_px,
                WindowControlTags {
                    minimize: WINDOW_CHROME_MINIMIZE_TAG,
                    maximize: WINDOW_CHROME_MAXIMIZE_TAG,
                    close: WINDOW_CHROME_CLOSE_TAG,
                },
            );
            view_dock_panel_with_actions(
                &pane_display_title(&tv.slots, i),
                fill_definite_shrinkable(build_pane_scene(&tv, i, &theme)),
                &theme,
                &style,
                None,
                Some(controls),
            )
        }
        // The main window tiles the docked panes; overlay the right-click context menu
        // (R140) when it is open (a no-op when closed) — LAST so the popup paints over.
        _ => crate::ctxmenu::overlay(view_main(&tv, &theme), state, &theme),
    }
}

/// The main window: arrange the DOCKED panes with draggable dividers via pinion's
/// [`view_dock_surface_chrome`] over the [`use_dock_topology`] split-tree. Each leaf's
/// `panel_id` maps back to its tile ([`pane_index_of_panel`]) and the
/// `panel_content` callback projects that pane ([`build_pane_scene`]); each Split's
/// ratio is the shared [`use_split_ratio`] Signal a drag re-weights (the SSOT both
/// the painted splitter and its `SplitterExternal` read). The walker wraps every
/// leaf in a [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel) — a
/// header strip (the drag / tear-off handle) above the pane.
///
/// The topology holds only the TILED panes (R149): the HOST owns which panes are tiled, and
/// a floated one has no leaf here — its content is painted alone in its own undock window
/// ([`view_for_window`]). `None` is the zero-pane edge (paints an empty surface).
///
/// (R151: this used to branch on [`is_pane_floating`](crate::dock::is_pane_floating) and
/// paint a `view_floating_placeholder` for a floated pane's RETAINED leaf — R72's model,
/// where the topology held every pane's leaf and the windows-signal was the float authority.
/// R149 gave both roles to the host, which made the branch unreachable: a floated pane has no
/// leaf, so `panel_content` is never called for one. Deleted rather than left to rot behind a
/// doc that still called the windows-signal "the sole floating authority".)
fn view_main(tv: &TerminalView, theme: &Theme) -> Scene {
    // The live drag-to-dock drop-preview (P2): read once here so the closure below
    // captures one snapshot (not a per-leaf re-read), and so the view subscribes to it —
    // a dragged panel's `DockPanelExternal::drag_to` `set` repaints the target's zone.
    // `None` between drags (no panel highlights).
    let drop_preview = use_drop_preview().get();
    // (R130, pinion R1318 / PINION-PR52) The DOCKED panel's header title + its tab label
    // are DISPLAY names, not identity: the walker still owns the `panel_id` tag (it
    // PANICS on a customizer that changes it), and this provider only supplies the string
    // it PAINTS. So a docked pane shows `vim README` / `coin@host:~` — the same
    // [`pane_display_title`] the floater header and the torn-off placeholder use — while
    // its address (dock-leaf id, scene tag, RPC path, `DockPanelExternal` key) stays
    // `terminal-{i}`. Two panes may safely share a display title; only the address must
    // be unique. `Cow::Borrowed(panel_id)` = the walker's identity default, for a leaf
    // with no live pane.
    let chrome =
        DockPanelChrome::default().with_title(|panel_id: &str| {
            match pane_index_of_panel(panel_id) {
                Some(i) if tv.slots.is_pane_occupied(i) => {
                    Cow::Owned(pane_display_title(&tv.slots, i))
                }
                _ => Cow::Borrowed(panel_id),
            }
        });
    let content = match use_dock_topology().get() {
        None => Scene::Container(ContainerNode::new(Vec::new())),
        Some(topo) => view_dock_surface_chrome(
            &topo,
            |panel_id| match pane_index_of_panel(panel_id) {
                // One occupancy check per leaf, then branch on float state (was two match
                // arms each re-evaluating `is_pane_occupied`).
                // A leaf is a TILED pane (the host tiles nothing else): fill the dock
                // panel's content area — the pane grid is no longer the direct splitter
                // child (`view_dock_panel` wraps it under a header), so it needs its own
                // definite extent or its full-window intrinsic size overflows the panel (the
                // grid never gets a measured rect, the R1012 reflow never fires, and the
                // pane stays at its boot dims).
                Some(i) if tv.slots.is_pane_occupied(i) => {
                    fill_definite(build_pane_scene(tv, i, theme))
                }
                // A leaf with no live pane (out of range / stale) — defensive.
                _ => Scene::Container(ContainerNode::new(Vec::new())),
            },
            |id, ratio| DockSplitState {
                ratio_signal: use_split_ratio(id.to_string(), ratio),
                // P1: no mid-drag tint (the splitter still drags fine). P2 reads
                // SplitterExternal::is_dragging() here for the M3 dragged overlay.
                dragging: false,
            },
            // drop-zone affordance per panel (pinion R1080/R1082 `view_dock_surface_chrome`
            // arg): the panel currently under a drag (the live `DockDropPreview` target)
            // paints its zone highlight; every other panel returns None. The dragged
            // panel's `DockPanelExternal::drag_to` writes `drop_preview` each cursor move.
            |panel_id| {
                drop_preview
                    .as_ref()
                    .filter(|p| p.target == panel_id)
                    .map(|p| p.zone)
            },
            &chrome,
            theme,
        ),
    };
    // Same-window OUTER full-span preview (pinion R1167): a docked-panel drag whose cursor
    // entered the window's outer band resolves to `OUTER_DOCK_ZONE_TAG` (no panel matches
    // the per-panel callback above, so the inner panels stay un-highlighted). Overlay a
    // full-span band at the previewed edge — preview == result, the same affordance the
    // cross-window floater preview ([`TerminalViewer::dock_drop_preview`]) shows. Appended
    // as an absolute (out-of-flow) child of the surface root, so the dock layout is
    // undisturbed. Mirrors the editor's `view_main_dock`.
    let content = match drop_preview
        .as_ref()
        .filter(|p| p.target == OUTER_DOCK_ZONE_TAG)
    {
        Some(p) => match content {
            Scene::Container(mut root) => {
                root.children.push(dock_outer_zone_highlight(p.zone, theme));
                Scene::Container(root)
            }
            other => other,
        },
        None => content,
    };
    compose(content, theme)
}

/// Build ONE pane's scene from its live screen + per-pane `ScrollState` + IME
/// preedit — the single per-pane builder shared by the docked tiling
/// ([`view_main`]) and an undock window ([`view_for_window`]). Reading the pane's
/// scroll offset / preedit subscribes the paint to them (the R705.1 reactive
/// bridge), so a per-pane scroll (keyboard OR drag) or composition `set` repaints
/// live. The scroll authority is the row-unit `ScrollState`
/// ([`crate::scrollbar::use_pane_scroll`]); `offset_y == max` is the live screen and
/// a smaller `offset_y` windows into history (styled cells, R58). The preedit overlays
/// only the live view (the host seam self-gates on the cursor). On child EOF the
/// pane paints its frozen final screen.
///
/// PURE read: the scroll bound + tail-follow are reconciled OUT of this view by
/// [`TerminalViewer::reconcile_frame`](crate::TerminalViewer) (pinion R1047's
/// pre-view hook), which runs first, so `offset_y` is already current here — the
/// view fn never writes a `Signal` (the §6.3 `dry_run` purity guarantee).
fn build_pane_scene(tv: &TerminalView, i: usize, theme: &Theme) -> Scene {
    let scroll = crate::scrollbar::use_pane_scroll(i);
    let preedit = use_preedit(i).get();
    // R1012 measured pane height — the winsize SSOT the reflow Effect reads; the
    // bar's track derives from the SAME rect (§3, vertical axis), never a
    // window-side recompute. Tracked read: the view re-runs (repaints the thumb)
    // when this pane's measured rect changes (resize / splitter drag).
    let track_h = pinion_core::use_pane_viewport_size(pane_tag(i)).1;
    // The non-cell per-frame facts, read from the host (scrollback depth for the
    // offset math + scrollbar extent; visible rows for the bar). Convert the
    // (already-reconciled) top-anchored offset to the projection's "rows up from
    // the live bottom".
    let dims = tv.slots.pane_scroll_facts(i);
    let offset_lines =
        crate::scrollbar::offset_lines_from_top(scroll.offset_y(), dims.scrollback_len);
    // Topology B: the GUI is a CLIENT of the host's per-pane cell DATA query
    // (`Host::pane_cells`). The host owns the screen + scrollback projection;
    // the IME preedit is a CLIENT-local overlay (an uncommitted composition never in
    // the PTY); the node is assembled CLIENT-side (`pane_view_scene_from_cells`, the
    // Screen-free seam). In-process now — the same steps ride the wire when the
    // Workspace moves to the host process (the transport step).
    let cells = tv.slots.pane_cells(i, offset_lines);
    let cells = sprag_grid::overlay_preedit(cells, &preedit);
    // R139: invert the mouse-selected cell band (read here subscribes the paint, so a
    // drag repaints the band live — the same reactive path as the preedit overlay).
    let cells = match crate::selection::span_for(i) {
        Some((start, end)) => sprag_grid::overlay_selection(cells, start, end),
        None => cells,
    };
    let grid =
        sprag_host::pane_view_scene_from_cells(pane_tag(i), cells, tv.metric, tv.font_size_px);
    let bar = crate::scrollbar::view_pane_scrollbar(i, &scroll, dims.visible_rows, track_h, theme);
    let pane = crate::scrollbar::wrap_pane_with_bar(grid, bar);
    // R142: sprag's focus indicator = DIM THE INACTIVE panes (the iTerm2 / kitty / tmux
    // convention) — the FOCUSED pane stays full-brightness, every other pane gets a
    // translucent dark scrim so the active one stands out, with no added chrome and no
    // ring painting over the context menu. The focused pane is
    // `pinion_core::focus_state::focused()` — pinion's R1335 owner-scoped focus mirror has
    // PRODUCER PARITY (locked by pinion R1343, which refuted PINION-PR55): populated in the
    // live winit paint AND the RPC snapshot/screenshot produce path, so the dim shows on
    // screen AND in a snapshot. Reading it here auto-subscribes the paint, so the dim
    // follows a click / Tab focus move.
    if pinion_core::focus_state::focused()
        .as_deref()
        .and_then(pane_index_of)
        == Some(i)
    {
        pane
    } else {
        dim_inactive(pane)
    }
}

/// The dim-scrim alpha over an inactive pane (0 = clear .. 255 = opaque black).
const DIM_ALPHA: u8 = 120;

/// Dim an INACTIVE pane (R142) — sprag's focus indicator overlays a translucent dark
/// scrim over every pane EXCEPT the focused one, so the active pane reads brighter (the
/// iTerm2 / kitty / tmux "dim inactive split" convention), with no added chrome and no
/// ring painting over the context menu. The scrim is a `pointer_transparent` absolute
/// overlay (does NOT block click-to-focus / drag-select) appended LAST so it paints over
/// the pane content; a full-cover `Percent(100)` fill, so — unlike a thin fixed-height
/// bar — it cannot collapse on a flex axis.
fn dim_inactive(pane: Scene) -> Scene {
    let scrim = Scene::Container(
        ContainerNode::new(Vec::new())
            .with_tag("sprag_gui.pane_dim")
            .with_style(BoxStyle::filled(
                pinion_core::style::Color::rgb(0, 0, 0).with_alpha(DIM_ALPHA),
            ))
            .with_layout(
                LayoutStyle::new()
                    .with_absolute_position(0, 0)
                    .with_size(fill_size())
                    .with_pointer_transparent(true),
            ),
    );
    match pane {
        Scene::Container(mut c) => {
            c.children.push(scrim);
            Scene::Container(c)
        }
        other => other,
    }
}

/// Wrap the workspace `content` in the surface-filled paint root (tagged
/// [`ROOT_TAG`]) that fills the window, so the tiling fills it and each pane's rect
/// derives from its split share (§3, per-pane via R1012). The surface shows through
/// the inter-pane divider gap.
///
/// `compose` owns the "content must carry a definite extent" invariant: it applies
/// [`fill_definite`] to `content` so a sizeless flex child can't collapse its main
/// axis to intrinsic (the cross axis still stretches). This is the SINGLE
/// enforcement point — every paint path funnels through `compose`, so a caller
/// cannot forget it (the R55 undock bug was exactly a forgotten fill). The fill only
/// sets `size` and is idempotent. Pure composition; the unit test exercises it
/// without a PTY.
fn compose(content: Scene, theme: &Theme) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![fill_definite(content)])
            .with_tag(ROOT_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(LayoutStyle::new().with_size(fill_size())),
    )
}

/// Give a Container a definite `Percent(100)` size (via [`fill_size`]) so a sizeless
/// flex child can't collapse to its content's intrinsic size on the main axis (the
/// cross axis still stretches) — the intrinsic-collapse the splitter's own R685 fix
/// documents. Applied at TWO enforcement points, each a DIFFERENT flex layer (so it
/// is not a single-point invariant — interior nodes still get their extent from
/// their parent's flex distribution and keep `Auto` cross-axes for `AlignItems::Stretch`):
///
/// 1. [`compose`] wraps the workspace `content` (the docked split-tree OR a lone
///    undock pane) so it fills the window-sized surface root. Forgetting this was the
///    R55 undock bug (the pane reflowed only its width).
/// 2. [`view_main`]'s `panel_content` callback wraps EACH docked pane's content,
///    because [`view_dock_surface_chrome`] interposes a sizeless `flex_grow(1.0)` content
///    wrapper ([`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel))
///    between the splitter and the pane grid — without a definite extent there the
///    grid keeps its full-window intrinsic width, never gets a measured rect, and the
///    R1012 reflow never fires (R60).
///
/// Each call site is the single fill for ITS layer (the surface root; each leaf's
/// content slot), so neither layer can forget it.
pub(crate) fn fill_definite(scene: Scene) -> Scene {
    match scene {
        Scene::Container(c) => Scene::Container(c.map_layout(|l| l.with_size(fill_size()))),
        other => other,
    }
}

/// [`fill_definite`] PLUS a main-axis (height) `min_size: Px(0)` — the content for a
/// lone pane in a FLOATING window. A floating window can be sized SMALLER than the
/// pane's boot content (the user shrinks it). Since R78, pinion's `view_dock_panel`
/// `content_wrapper` carries the `view_splitter` R1086 idiom (`flex_basis:0 + flex_grow:1 +
/// min-height:0`, delivered as pinion R1109 for PINION-PR35), so the WRAPPER no longer
/// pins an automatic minimum; declaring the CONTENT's own `min_size.height = 0` (alongside a
/// definite `Percent(100)` preferred height) composes with it so the grid can shrink to the
/// panel's distributed height, gets a sub-window rect, the R1012 publish reports it, and the
/// reflow Effect fires. Both sides carry `min-height:0` and compose — verified by
/// `undock_window_reflows_its_height_below_boot_content`.
fn fill_definite_shrinkable(scene: Scene) -> Scene {
    // Literally [`fill_definite`] PLUS the main-axis `min_size: 0` — compose it so the two
    // never drift (the "fill the window" extent lives in exactly one place).
    match fill_definite(scene) {
        Scene::Container(c) => Scene::Container(
            c.map_layout(|l| l.with_min_size(Size::auto().with_height(SizeValue::Px(0)))),
        ),
        other => other,
    }
}

/// A both-axes `Percent(100)` size — fill the parent slot. The ONE definition,
/// shared by [`compose`] and [`fill_definite`] (so the "fill the window" literal
/// lives in one place).
pub(crate) fn fill_size() -> Size {
    Size::auto()
        .with_width(SizeValue::Percent(100))
        .with_height(SizeValue::Percent(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    #[test]
    fn compose_wraps_the_grid_in_a_filling_paint_root() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            // A stand-in grid (the real one is the host's
            // pane_view_scene_from_cells, tested in sprag-host) — compose only
            // owns the root wrapping.
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("grid_stub"));
            compose(grid, &theme)
        });
        match scene {
            Scene::Container(ref root) => {
                assert_eq!(root.tag.as_deref(), Some(ROOT_TAG));
                assert_eq!(root.layout.size.width, SizeValue::Percent(100));
                assert_eq!(root.layout.size.height, SizeValue::Percent(100));
            }
            other => unreachable!("compose returns a Container, got {other:?}"),
        }
        assert!(scene.contains_tag("grid_stub"), "the grid is mounted");
    }
}
