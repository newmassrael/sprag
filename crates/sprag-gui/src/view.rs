//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::ROOT_TAG;
use crate::attention;
use crate::dock::pane_window_index;
use crate::input::use_preedit;
use crate::slotview::SlotView;
use crate::split::{
    pane_index_of_panel, panel_id, use_dock_topology, use_drop_preview, use_split_ratio,
};
use crate::terminal::{TerminalView, pane_cache_key, pane_index_of, pane_tag, use_terminal};
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::external::OUTER_DOCK_ZONE_TAG;
use pinion_core::reactive::Owner;
use pinion_core::scene::{ContainerNode, ImageNode, Rect};
use pinion_core::style::{
    Border, BoxStyle, Fit, FlexDirection, ImageStyle, LayoutStyle, Size, SizeValue,
};
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
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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
    let title = slots
        .pane_title(i)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| panel_id(i));
    // Prefix the attention marker (R-PR67 follow-on) when the pane's child raised a notification
    // this client has not yet VIEWED — the tmux bell flag, shown on every title surface (tab,
    // dock header, floater, taskbar) so an unattended pane is visible without opening it. The
    // focused pane is acked before this runs, so it never wears its own marker.
    if attention::pane_has_unseen_attention(slots, i) {
        format!("{}{title}", attention::ATTENTION_MARKER)
    } else {
        title
    }
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
/// The binding's cached per-frame `State`: the `Copy` snapshots the shell reads out of the MODEL
/// scene and hands to the pure paint.
///
/// It grew from `()` to the context menu's posture, and now carries the find field's too — both are
/// External-owned interaction states that only the model scene knows, so both must cross this seam
/// rather than be re-derived in the view. A struct rather than a tuple so a third surface adds a
/// named field instead of a positional one.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct ViewState {
    /// The context menu's open anchor + active item.
    pub(crate) menu: crate::ctxmenu::MenuState,
    /// The find field's interaction state + caret.
    pub(crate) find: crate::find::FindFieldState,
    /// The command palette's query field, on the same terms as the find field's.
    pub(crate) palette: crate::palette::PaletteFieldState,
}

/// Overlay the find bar on `scene` when it is open (a no-op when closed), pushed LAST so the
/// absolutely-positioned bar paints over the tiling below it — the placement the context menu's own
/// overlay documents.
fn with_find_bar(scene: Scene, field: crate::find::FindFieldState, theme: &Theme) -> Scene {
    let Some(bar) = crate::find::view_bar(field, theme, (WINDOW_W, WINDOW_H)) else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    root.children.push(bar);
    Scene::Container(root)
}

/// Overlay the command palette on `scene` when it is open (a no-op when closed), pushed after
/// everything else so it paints — and HIT-TESTS — above all of it.
///
/// Topmost is not a preference here but what MODAL means: the palette's scrim is the click target
/// everywhere except over its own panel, so a click beside it must not reach a pane, a menu row or
/// the find bar underneath. That is also why it is layered above the context menu, the reverse of
/// the find bar's relationship to that menu: a menu can be opened over the bar, but nothing can be
/// opened over the palette while its focus trap is up.
fn with_palette(scene: Scene, field: crate::palette::PaletteFieldState, theme: &Theme) -> Scene {
    let Some(panel) = crate::palette::view_palette(field, theme, (WINDOW_W, WINDOW_H)) else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    root.children.push(panel);
    Scene::Container(root)
}

/// Overlay the destructive-command prompt on `scene` when one is armed (a no-op otherwise), pushed
/// after the palette — above EVERYTHING, including the surface that armed it.
///
/// Innermost is what this modal means: it is the last question before something irreversible happens,
/// so nothing may be clicked or typed while it is up, and least of all the palette row that armed it.
/// (The palette closes before arming, so the two are never up together; the layering is the guarantee
/// rather than the mechanism.)
fn with_confirm(scene: Scene, theme: &Theme) -> Scene {
    let Some(panel) = crate::confirm::view_confirm(theme, (WINDOW_W, WINDOW_H)) else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    root.children.push(panel);
    Scene::Container(root)
}

pub(crate) fn view_for_window(window_id: &str, state: ViewState, _frame: &Frame) -> Scene {
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
        // The find bar rides ABOVE the tiling and BELOW the context menu: a menu opened over the
        // bar must still paint on top, and the menu's own dismiss barrier must not sit over it.
        _ => with_confirm(
            with_palette(
                crate::ctxmenu::overlay(
                    with_find_bar(view_main(&tv, &theme), state.find, &theme),
                    state.menu,
                    &theme,
                ),
                state.palette,
                &theme,
            ),
            &theme,
        ),
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
    // The window TAB STRIP (tmux "windows") above the pane area + the session SIDEBAR (tmux
    // sessions / cmux workspaces) down the left — main window only, since `compose` is reached only
    // from here. Both read off the SlotView mirror (no socket call on the paint path).
    let strip = crate::wtabs::view_window_strip(&tv.slots, theme);
    let sidebar = crate::stabs::view_session_sidebar(&tv.slots, theme);
    compose(sidebar, strip, content, theme)
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
/// The `memory://` store key + `Scene::Image` tag suffix for pane `i`'s image `id`.
fn image_store_key(i: usize, id: u32) -> String {
    format!("pane{i}.img{id}")
}

/// Pane `i`'s client-local record of which image `(id -> seq)` is CURRENTLY registered in the root
/// image store — so [`reconcile_pane_images`] fetches + registers an image's RGBA exactly ONCE per
/// content change (R1404 Stage 5 on-demand), reusing the store entry on every later frame. The
/// `Owner::cache` per-pane pattern (like the scrollbar / hyperlink state).
fn use_pane_image_registry(i: usize) -> Rc<RefCell<HashMap<u32, u64>>> {
    Owner::current()
        .expect("use_pane_image_registry requires an active Owner scope")
        .cache(pane_cache_key("image_registry", i), || {
            Rc::new(RefCell::new(HashMap::new()))
        })
        .as_ref()
        .clone()
}

/// Fetch + register pane `i`'s NEW / CHANGED inline images into the root image store, and evict the
/// store entries of images the pane no longer shows — the off-thread-fact → UI seam, run from
/// [`reconcile_frame`](crate::TerminalViewer), NOT the pure view (the RGBA fetch is a blocking wire
/// round-trip, like the clipboard-write payload fetch). Keyed on `(id, seq)` from the panes-slot
/// summary, so a given image's megabyte raster crosses the wire ONCE per transmit, not per poll; the
/// pure [`compose_pane_images`] then just references the already-registered `memory://` key.
pub(crate) fn reconcile_pane_images(slots: &SlotView, i: usize) {
    let images = slots.pane_images(i);
    let registry = use_pane_image_registry(i);
    let store = pinion_runtime::use_image_store();
    // Evict a store key whose image left the pane (delete / clear / scrolled off), pruning the record.
    registry.borrow_mut().retain(|id, _| {
        let present = images.iter().any(|img| img.id == *id);
        if !present {
            store.remove(&image_store_key(i, *id));
        }
        present
    });
    // Fetch + register a new id, or a re-transmit (seq changed), exactly once.
    for img in &images {
        if registry.borrow().get(&img.id).copied() == Some(img.seq) {
            continue; // already registered at this content generation — reuse the store entry
        }
        let Some(rgba) = slots.pane_image_rgba(i, img.id) else {
            continue; // the host no longer has it (a transmit/clear race) — try again next frame
        };
        let Some(decoded) = pinion_runtime::DecodedImage::from_rgba8(img.width, img.height, rgba)
        else {
            continue; // a byte count that does not match the raster — skip, never register torn
        };
        store.insert(image_store_key(i, img.id), &decoded);
        registry.borrow_mut().insert(img.id, img.seq);
    }
}

/// Reset pane slot `i`'s image registry when the slot FREES — evict its store keys and clear the
/// record, so a reused slot inherits no stale image. Called from
/// [`reset_freed_slot`](crate::reset_freed_slot).
pub(crate) fn reset_pane_images(i: usize) {
    let registry = use_pane_image_registry(i);
    let store = pinion_runtime::use_image_store();
    for id in registry.borrow().keys() {
        store.remove(&image_store_key(i, *id));
    }
    registry.borrow_mut().clear();
}

/// Composite pane `i`'s inline images (Kitty / Sixel, R1404) over its text grid: push a
/// `Scene::Image` node (`memory://pane{i}.img{id}`) as a child of the grid `Container`, absolutely
/// positioned at the image's anchor cell × the cell metric and sized to its pixel raster, so
/// pinion's `ImageCache` paints it over the `TextGrid`. PURE — the RGBA was fetched + registered by
/// [`reconcile_pane_images`] (the pre-view hook); this only references the registered key, and skips
/// an image not yet registered (a first-frame race resolves next frame). A no-op when the pane has
/// no images (returns `grid` unchanged, so the common case allocates nothing).
fn compose_pane_images(grid: Scene, tv: &TerminalView, i: usize) -> Scene {
    let images = tv.slots.pane_images(i);
    if images.is_empty() {
        return grid;
    }
    let registry = use_pane_image_registry(i);
    let Scene::Container(mut container) = grid else {
        return grid;
    };
    let (cell_w, cell_h) = (tv.metric.cell_w(), tv.metric.cell_h());
    for img in &images {
        // Only paint an image the reconcile already fetched + registered at THIS content generation.
        if registry.borrow().get(&img.id).copied() != Some(img.seq) {
            continue;
        }
        let key = image_store_key(i, img.id);
        container.children.push(Scene::Image(
            ImageNode::styled(
                format!("memory://{key}"),
                Rect::default(),
                ImageStyle::default().with_fit(Fit::Fill),
            )
            .with_tag(format!("{}#img{}", pane_tag(i), img.id))
            .with_layout(
                LayoutStyle::new()
                    .with_absolute_position(
                        u32::from(img.anchor.0) * cell_w,
                        u32::from(img.anchor.1) * cell_h,
                    )
                    .with_size(Size::px(img.width, img.height)),
            ),
        ));
    }
    Scene::Container(container)
}

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
    // R-71.2 (pinion R1405): light the hovered OSC-8 link's whole id-group. The hover
    // is tracked by the pane's hover-oracle External (fed the link map in
    // `reconcile_frame`); reading its `hovered` Signal here subscribes the paint, so a
    // hover move repaints the highlight.
    let hovered = crate::hyperlink::hovered_link(i);
    let cells = sprag_grid::overlay_hyperlink_hover(cells, hovered);
    // Find-in-scrollback: recolour this pane's visible matches (and, distinctly, the current one).
    // Laid AFTER the selection / hover inversions on purpose — a match must stay legible inside a
    // selected band, which two stacked inversions would cancel. Reading the find Signals here
    // subscribes the paint, so typing in the bar repaints the highlight.
    let cells = crate::find::overlay_matches(cells, i, scroll.offset_y(), dims.visible_rows);
    let grid =
        sprag_host::pane_view_scene_from_cells(pane_tag(i), cells, tv.metric, tv.font_size_px);
    // R-71.1: the hand cursor while hovering a link (the grid's whole rect resolves to
    // the pointer cursor exactly when the current hover is a link, since the oracle
    // only sets `hovered` over a link cell).
    let grid = match grid {
        Scene::Container(mut c) if hovered.is_some() => {
            c.layout.cursor = Some(pinion_core::style::CursorHint::Pointer);
            Scene::Container(c)
        }
        other => other,
    };
    // R1404: composite the pane's inline images (Kitty graphics) over the text grid — register each
    // image's RGBA into the shell's root MemoryImageStore and add a `Scene::Image` node at the
    // image's anchor cell (× the cell metric). Stage 1: single-chunk RGBA/RGB, cleared only on
    // screen-clear/alt-screen (scroll/reflow eviction is a documented later bound). tmux cannot
    // show inline images at all.
    let grid = compose_pane_images(grid, tv, i);
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
    let pane = if pinion_core::focus_state::focused()
        .as_deref()
        .and_then(pane_index_of)
        == Some(i)
    {
        pane
    } else {
        dim_inactive(pane)
    };
    // cmux ATTENTION RING — outline an inactive pane whose child raised a notification this
    // client has not yet VIEWED (OSC 9 / 99 / 777, or a bell), so the panel wanting attention
    // is glanceable at a distance without reading titles (cmux's ring-around-the-panel
    // affordance; tmux offers only the title bell FLAG, no per-panel outline). Keyed on the
    // SAME `pane_has_unseen_attention` the title marker uses, so it appears on a notification
    // and CLEARS when the pane is focused (the focus ack bumps the seen-`seq`). A focused pane
    // is acked before this runs, so the ring only ever wears an inactive (already-dimmed) pane;
    // painting it AFTER `dim_inactive` puts it over the scrim. Reading the predicate here
    // subscribes the paint, so the ring follows a notification arriving / the pane being viewed.
    let pane = if attention::pane_has_unseen_attention(&tv.slots, i) {
        pane_ring(
            pane,
            "sprag_gui.pane_attention",
            theme.resolve(ColorRole::Accent),
        )
    } else {
        pane
    };
    // DROP TARGET — outline the pane a dragged file would land on, while a drag is over this binding
    // (pinion R1437 gave the hover hooks the window id, which is what makes the target knowable).
    // Painted LAST so it wears over the attention ring: during a drag, WHERE THE FILE GOES is the
    // question on screen, and a pane can legitimately be both. A distinct role colour keeps the two
    // readable — one says "look at me", the other "drop here" — and reading the signal subscribes the
    // paint, so the outline follows the drag and vanishes on cancel or drop.
    if crate::dock::use_drop_hover().get() == Some(i) {
        // `InversePrimary`, not `Accent`: the palette's SECOND emphasis role, so the two rings stay in
        // one family while remaining tellable apart on a pane that wears both. No colour is invented
        // outside the theme, so a re-theme moves them together.
        pane_ring(
            pane,
            "sprag_gui.pane_drop_target",
            theme.resolve(ColorRole::InversePrimary),
        )
    } else {
        pane
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

/// The width (px) of a [`pane_ring`] — the sidebar cursor-row border convention ([`crate::stabs`]),
/// shared by the attention and drop-target rings so the two read as one vocabulary.
const PANE_RING_WIDTH: u32 = 2;

/// Outline a pane in `color` under `tag` — the ONE ring primitive, drawn for two reasons that must
/// look like siblings rather than two hand-rolled frames: the cmux-parity ATTENTION ring (a child
/// raised a notification this client has not viewed) and the DROP-TARGET ring (a dragged file would
/// land here).
///
/// Built exactly like [`dim_inactive`] — a `pointer_transparent` absolute overlay (never blocks
/// click-to-focus / drag-select) at `Percent(100)` full cover, appended LAST so it paints over the
/// pane content AND the dim scrim — but with a TRANSPARENT fill carrying a [`Border`], so only the
/// frame draws. The border rides the OVERLAY, not the pane container, so it adds no layout: a
/// notification arriving (or a drag passing over) never shifts or resizes the pane. Snapshot-visible
/// like the dim scrim (same absolute-overlay path the RPC produce walks), and the distinct `tag` is
/// what lets a snapshot consumer — or a headless test — tell WHICH ring it is looking at.
fn pane_ring(pane: Scene, tag: &'static str, color: pinion_core::style::Color) -> Scene {
    let ring = Scene::Container(
        ContainerNode::new(Vec::new())
            .with_tag(tag)
            .with_style(
                BoxStyle::filled(pinion_core::style::Color::TRANSPARENT)
                    .with_border(Border::new(color, PANE_RING_WIDTH)),
            )
            .with_layout(
                LayoutStyle::new()
                    .with_absolute_position(0, 0)
                    .with_size(fill_size())
                    .with_pointer_transparent(true),
            ),
    );
    match pane {
        Scene::Container(mut c) => {
            c.children.push(ring);
            Scene::Container(c)
        }
        other => other,
    }
}

/// Wrap the workspace `content` in the surface-filled paint root (tagged [`ROOT_TAG`]) that fills
/// the window. The root is a ROW: the session `sidebar` down the left (a fixed-width band), and to
/// its right a COLUMN of the window tab `strip` above the tiled `content` — so the panes fill the
/// window MINUS the sidebar width and the strip height, and each pane's rect derives from its split
/// share (§3, per-pane via R1012). The surface shows through the inter-pane divider gap.
///
/// `compose` owns the "content must carry a definite extent" invariant: [`fill_remaining`] lets the
/// content take the height the fixed strip leaves while still handing the pane grid a measured rect
/// (so the R1012 reflow fires), and the content COLUMN takes the width the fixed sidebar leaves
/// (`flex_grow` + `min-width: 0`). This is the SINGLE enforcement point — every paint path funnels
/// through `compose`, so a caller cannot forget it (the R55 undock bug was exactly a forgotten
/// fill). Pure composition; the unit test exercises it without a PTY.
fn compose(sidebar: Scene, strip: Scene, content: Scene, theme: &Theme) -> Scene {
    // The content COLUMN (tab strip above the tiled panes), filling the width the sidebar leaves:
    // `flex_grow` takes the remaining main-axis (Row) width, `min-width: 0` lets it shrink below
    // intrinsic, and `height: 100%` fills the Row's cross axis.
    let column = Scene::Container(
        ContainerNode::new(vec![strip, fill_remaining(content)]).with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Column)
                .with_flex_grow(1.0)
                .with_min_size(Size::auto().with_width(SizeValue::Px(0)))
                .with_size(Size::auto().with_height(SizeValue::Percent(100))),
        ),
    );
    // The surface-filled paint root: the fixed-width sidebar beside the content column.
    Scene::Container(
        ContainerNode::new(vec![sidebar, column])
            .with_tag(ROOT_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_size(fill_size()),
            ),
    )
}

/// The main content BELOW the tab strip in the surface-root Column: it must take the height LEFT
/// by the fixed-height strip, so it GROWS to fill (`flex_grow`) and may shrink below its intrinsic
/// size (`min-height: 0`) — NOT the `Percent(100)` height [`fill_definite`] gives, which beside a
/// fixed strip would over-constrain the Column (strip + 100% > 100%). The cross axis still fills
/// (width `Percent(100)`), and `flex_grow` still hands the pane grid a measured rect, so the R1012
/// reflow fires exactly as it did under `fill_definite`.
fn fill_remaining(scene: Scene) -> Scene {
    match scene {
        Scene::Container(c) => Scene::Container(c.map_layout(|l| {
            l.with_flex_grow(1.0)
                .with_size(Size::auto().with_width(SizeValue::Percent(100)))
                .with_min_size(Size::auto().with_height(SizeValue::Px(0)))
        })),
        other => other,
    }
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

    /// THE display half of the inline-image feature, end to end over a REAL pane: a child transmits a
    /// Kitty image, the reconcile fetches its raster and registers it in the root image store, and the
    /// pure compose puts a `Scene::Image` over the grid AT THE ANCHOR CELL x the cell metric.
    ///
    /// Worth its own test because the two halves fail apart. `reconcile_pane_images` does the blocking
    /// wire fetch and is the only writer of the store; `compose_pane_images` is pure and paints nothing
    /// it has not seen registered. So a broken fetch yields a silently image-LESS pane rather than an
    /// error, which is the failure mode a data-side test cannot see — the host would still report the
    /// image in its panes slot and every wire assertion would pass.
    ///
    /// This is also what makes "a restored pane comes back with its images" true for a HUMAN and not
    /// just for the emulator: a restored image reaches this seam exactly as a live one does.
    ///
    /// REVERT-PROOF: skipping the store insert leaves the registry unpopulated, so compose paints
    /// nothing and the `Scene::Image` assertion fails; painting at `(0,0)` instead of the anchor fails
    /// the position assertion, which is why the fixture puts the image at a NON-zero cell.
    #[test]
    fn a_panes_inline_image_is_fetched_registered_and_composed_at_its_anchor() {
        use crate::terminal::{seed_terminal, use_terminal};
        use sprag_host::Host;
        use sprag_terminal::CommandBuilder;
        use std::time::{Duration, Instant};

        // A 2x2 RGBA image transmitted at cell (3, 1) — a non-zero anchor on both axes, so a paint
        // that ignored the anchor could not accidentally match.
        let pixels: Vec<u8> = (1..=16u8).collect();
        let b64 = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&pixels)
        };
        let script =
            format!("printf '\\033[2;4H\\033_Ga=T,f=32,s=2,v=2,i=5;{b64}\\033\\\\'; exec cat");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg(&script);
        cmd.env("TERM", "dumb");

        let host = Host::new((40, 6));
        host.spawn(cmd, "img".to_owned(), 40, 6, None, None)
            .unwrap();

        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            let terminal = use_terminal();

            // Wait on the CONDITION the assertions read — the image reaching the host — not a timer.
            let deadline = Instant::now() + Duration::from_secs(5);
            while terminal.slots.pane_images(0).is_empty() {
                assert!(
                    Instant::now() < deadline,
                    "the child's image never reached the host",
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let summary = terminal.slots.pane_images(0);
            assert_eq!(summary.len(), 1);
            assert_eq!(summary[0].anchor, (3, 1), "the fixture's non-zero anchor");
            assert_eq!(summary[0].id, 5, "and its own image id");
            // NOTE the two clients differ here and the reconcile is written to the narrower contract:
            // the IN-PROCESS `Host` hands back the raster inline (it reads `Screen::images` directly),
            // while `WireHost` sends a `{id,width,height,anchor,seq}` SUMMARY and fetches the megabyte
            // raster on demand. `reconcile_pane_images` fetches by id either way, so it never depends
            // on the summary having brought the bytes along.

            // The reconcile fetches the raster and registers it.
            reconcile_pane_images(&terminal.slots, 0);
            let store = pinion_runtime::use_image_store();
            let key = image_store_key(0, 5);
            assert!(
                store.contains(&key),
                "the reconcile registered the raster under the pane's image key",
            );

            // ...and the pure compose references it, positioned at anchor x cell metric.
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("grid_stub"));
            let composed = compose_pane_images(grid, &terminal, 0);
            let Scene::Container(container) = composed else {
                unreachable!("compose returns the grid Container");
            };
            let image = container
                .children
                .iter()
                .find_map(|child| match child {
                    Scene::Image(node) => Some(node),
                    _ => None,
                })
                .expect("the image is composed over the grid");
            assert_eq!(
                image.source,
                format!("memory://{key}"),
                "it references the REGISTERED key, not a re-fetch",
            );
            let (cell_w, cell_h) = (terminal.metric.cell_w(), terminal.metric.cell_h());
            assert_eq!(
                image.layout.absolute_position,
                Some((3 * cell_w, cell_h)),
                "positioned at the anchor CELL times the cell metric",
            );
        });
    }

    #[test]
    fn compose_wraps_the_grid_in_a_filling_paint_root() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            // Stand-in sidebar + strip + grid (the real ones are `stabs::view_session_sidebar`,
            // `wtabs::view_window_strip` and the host's pane_view_scene_from_cells, tested
            // elsewhere) — compose only owns the root wrapping: the sidebar beside a
            // strip-over-content column.
            let sidebar = Scene::Container(ContainerNode::new(Vec::new()).with_tag("sidebar_stub"));
            let strip = Scene::Container(ContainerNode::new(Vec::new()).with_tag("strip_stub"));
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("grid_stub"));
            compose(sidebar, strip, grid, &theme)
        });
        // Assert the STRUCTURE, not just "all three tags appear somewhere" (which a flat
        // `Row[sidebar, strip, content]` — strip beside content, a broken layout — would also
        // satisfy, since `contains_tag` is recursive). The root must be a Row of exactly
        // [sidebar, column], and that column a Column of [strip, content]: so the sidebar is a
        // DIRECT child (beside), and the strip stacks ABOVE the content inside the column.
        let Scene::Container(ref root) = scene else {
            unreachable!("compose returns a Container, got {scene:?}");
        };
        assert_eq!(root.tag.as_deref(), Some(ROOT_TAG));
        assert_eq!(root.layout.size.width, SizeValue::Percent(100));
        assert_eq!(root.layout.size.height, SizeValue::Percent(100));
        assert_eq!(
            root.layout.flex_direction,
            FlexDirection::Row,
            "the sidebar sits beside the content column",
        );
        assert_eq!(root.children.len(), 2, "root = [sidebar, content column]");
        let Scene::Container(ref sidebar) = root.children[0] else {
            unreachable!("the sidebar is the root's first child");
        };
        assert_eq!(
            sidebar.tag.as_deref(),
            Some("sidebar_stub"),
            "the sidebar is a DIRECT child of the Row (beside the content), not nested in the column",
        );
        let Scene::Container(ref column) = root.children[1] else {
            unreachable!("the content column is the root's second child");
        };
        assert_eq!(
            column.layout.flex_direction,
            FlexDirection::Column,
            "the content column stacks its children vertically",
        );
        // The strip stacks ABOVE the content INSIDE the column (order proves "above", not "beside").
        assert_eq!(
            column.children.len(),
            2,
            "content column = [strip, content]"
        );
        let child_tag = |i: usize| match &column.children[i] {
            Scene::Container(c) => c.tag.as_deref(),
            _ => None,
        };
        assert_eq!(
            child_tag(0),
            Some("strip_stub"),
            "the tab strip is the FIRST child of the column (stacks above the content)",
        );
        assert_eq!(
            child_tag(1),
            Some("grid_stub"),
            "the content is the second child of the column (below the strip)",
        );
    }

    /// The cmux ATTENTION RING: [`attention_ring`] frames a pane with a BORDER-only overlay — a
    /// `pane_attention`-tagged child appended LAST (paints over the pane content + dim scrim),
    /// carrying a [`Border`] and NO visible fill, full-cover so the frame sits at the pane's edges,
    /// and pointer-transparent so it never blocks click-to-focus / drag-select (like the dim
    /// scrim). REVERT-PROOF: drop the `with_border` and the border assertion FAILs; drop the append
    /// and the last-child tag assertion FAILs.
    /// The two rings are DISTINCT overlays: same primitive, different tag and different colour, so a
    /// pane wearing both is still readable and a snapshot consumer can tell which is which.
    ///
    /// Tags rather than pixels are what this asserts, because the tag is the thing a headless client
    /// (or an AI reading `scene/snapshot`) actually resolves — the colour is checked only for being
    /// DIFFERENT, which is the property that matters and the one a careless re-theme would break.
    ///
    /// REVERT-PROOF: giving the drop ring the attention ring's tag fails the distinct-tag assertion;
    /// giving it `Accent` fails the distinct-colour one.
    #[test]
    fn the_drop_target_ring_is_distinct_from_the_attention_ring() {
        let owner = Owner::new();
        let (attention, drop) = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            let ring = |tag: &'static str, role| {
                let pane = Scene::Container(ContainerNode::new(Vec::new()).with_tag("pane_stub"));
                let Scene::Container(mut framed) = pane_ring(pane, tag, theme.resolve(role)) else {
                    unreachable!("pane_ring returns the pane Container");
                };
                match framed.children.pop() {
                    Some(Scene::Container(c)) => c,
                    other => unreachable!("the ring is the pane's LAST child, got {other:?}"),
                }
            };
            (
                ring("sprag_gui.pane_attention", ColorRole::Accent),
                ring("sprag_gui.pane_drop_target", ColorRole::InversePrimary),
            )
        });
        assert_ne!(
            attention.tag, drop.tag,
            "each ring carries its OWN tag, so a snapshot says which one is showing",
        );
        assert_eq!(drop.tag.as_deref(), Some("sprag_gui.pane_drop_target"));
        let colour = |c: &ContainerNode| c.style.border.as_ref().map(|b| b.color);
        assert!(colour(&drop).is_some(), "the drop ring draws a border");
        assert_ne!(
            colour(&attention),
            colour(&drop),
            "and a DIFFERENT colour, so a pane wearing both is still readable",
        );
    }

    #[test]
    fn attention_ring_frames_the_pane_with_a_pointer_transparent_border() {
        let owner = Owner::new();
        let ring = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            let pane = Scene::Container(ContainerNode::new(Vec::new()).with_tag("pane_stub"));
            let Scene::Container(mut framed) = pane_ring(
                pane,
                "sprag_gui.pane_attention",
                theme.resolve(ColorRole::Accent),
            ) else {
                unreachable!("pane_ring returns the pane Container");
            };
            match framed.children.pop() {
                Some(Scene::Container(c)) => c,
                other => unreachable!("the ring is the pane's LAST child, got {other:?}"),
            }
        });
        assert_eq!(
            ring.tag.as_deref(),
            Some("sprag_gui.pane_attention"),
            "the ring is a distinct tagged overlay (queryable in a snapshot smoke)",
        );
        assert!(
            ring.style.border.is_some(),
            "the ring draws a Border — the outline, not a scrim (REVERT-PROOF: drop `with_border`)",
        );
        assert_eq!(
            ring.layout.size.width,
            SizeValue::Percent(100),
            "full-cover width so the frame lands at the pane's edges",
        );
        assert_eq!(
            ring.layout.size.height,
            SizeValue::Percent(100),
            "full-cover height so the frame lands at the pane's edges",
        );
        assert!(
            ring.layout.pointer_transparent,
            "the ring must not block click-to-focus / drag-select (like the dim scrim)",
        );
    }
}
