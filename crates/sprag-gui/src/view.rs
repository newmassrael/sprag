//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::ROOT_TAG;
use crate::dock::{is_pane_floating, pane_window_index, use_windows_topology};
use crate::input::use_preedit;
use crate::terminal::{TerminalView, pane_tag, use_terminal};
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{ColorRole, Theme, use_theme};
use pinion_core::{Frame, Scene};
use sprag_host::PaneViewSpec;

/// Shared [`ThemeProvider`](pinion_core::ThemeProvider) cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";

/// view-fn (§6.3): per-window paint. The **main** window tiles the DOCKED panes
/// (those without an undock window); an **undock window** (`pane-{i}`) paints that
/// one pane alone, full-window. Both reuse [`build_pane_scene`], so a docked and a
/// floated pane are pixel-identical projections. [`WidgetCore::view`](crate::TerminalViewer)
/// (the windowless / RPC-snapshot fallback) routes here as the main window. The
/// producer threads (the PTY readers) live in `create_extra_externals`, not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn view_for_window(window_id: &str, _state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    match pane_window_index(window_id) {
        // An undock window paints its one pane (if still present); a stale window
        // id (pane closed) falls back to the main layout, never a stranded paint.
        Some(i) if i < tv.pane_count() => compose(build_pane_scene(&tv, i, &theme), &theme),
        _ => view_main(&tv, &theme),
    }
}

/// The main window: arrange the panes with draggable dividers ([`is_pane_floating`]
/// is the docked/floated partition SSOT — a floated pane is painted in its own
/// undock window). The arrangement follows [`crate::split::layout_mode`]
/// (`SPRAG_GUI_LAYOUT`), and the two modes differ ONLY in how a floated pane is
/// handled — the one place the row<->grid asymmetry lives (see [`crate::split`]):
///
/// * **Row** (default, R38) — COMPACT: tile only the docked panes left-to-right
///   ([`crate::split::view_split_row`]); a floated pane leaves the row and the rest
///   slide over (safe — every row divider is Horizontal regardless of count, so the
///   boot Externals still match). An all-floated workspace -> an empty root.
/// * **Grid** (R40) — HOLD-SLOT: arrange ALL panes by the fixed boot shape
///   ([`crate::split::view_grid`]); a floated pane's slot is an [`empty_cell`]
///   (UNTAGGED) so the grid scaffold — divider tags + orientations — never changes
///   and the boot Externals always match the painted dividers (they cannot be
///   re-registered; boot-only, PR-9). Dock-back refills the slot in place.
fn view_main(tv: &TerminalView, theme: &Theme) -> Scene {
    let windows = use_windows_topology().get();
    let content = match crate::split::layout_mode() {
        crate::split::LayoutMode::Row => {
            let panes: Vec<Scene> = (0..tv.pane_count())
                .filter(|&i| !is_pane_floating(&windows, i))
                .map(|i| build_pane_scene(tv, i, theme))
                .collect();
            crate::split::view_split_row(panes, theme)
        }
        crate::split::LayoutMode::Grid => {
            let cells: Vec<Scene> = (0..tv.pane_count())
                .map(|i| {
                    if is_pane_floating(&windows, i) {
                        empty_cell(theme)
                    } else {
                        build_pane_scene(tv, i, theme)
                    }
                })
                .collect();
            crate::split::view_grid(cells, theme)
        }
    };
    compose(content, theme)
}

/// A held grid slot for a floated pane (grid mode): a surface-filled cell that
/// takes the slot's flex share but carries NO tag. Untagged is LOAD-BEARING — a
/// [`pane_tag`] here would make the slot publish an R1012 rect for the floated
/// pane, reflowing it to the empty slot (it must keep its undock-window size). The
/// floated pane is painted in its own window; this just reserves its place until
/// dock-back.
fn empty_cell(theme: &Theme) -> Scene {
    Scene::Container(
        ContainerNode::new(Vec::new())
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface))),
    )
}

/// Build ONE pane's scene from its live screen + per-pane [`ScrollState`] + IME
/// preedit — the single per-pane builder shared by the docked tiling
/// ([`view_main`]) and an undock window ([`view_for_window`]). Reading the pane's
/// scroll offset / preedit subscribes the paint to them (the R705.1 reactive
/// bridge), so a per-pane scroll (keyboard OR drag) or composition `set` repaints
/// live. The scroll authority is the row-unit `ScrollState`
/// ([`crate::scrollbar::use_pane_scroll`]); `offset_y == max` is the live screen and
/// a smaller `offset_y` windows into history (text-only, R16). The preedit overlays
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
    // Convert the (already-reconciled) top-anchored offset to the projection's
    // "rows up from the live bottom".
    let (scrollback_len, visible_rows) = tv
        .pane(i)
        .session()
        .with_screen(|screen| (screen.scrollback_len(), screen.rows()));
    let offset_lines = crate::scrollbar::offset_lines_from_top(scroll.offset_y(), scrollback_len);
    let grid = tv.pane(i).session().with_screen(|screen| {
        sprag_host::pane_view_scene(
            pane_tag(i),
            PaneViewSpec {
                screen,
                metric: tv.metric,
                font_size_px: tv.font_size_px,
                offset_lines,
                preedit: &preedit,
            },
        )
    });
    let bar = crate::scrollbar::view_pane_scrollbar(i, &scroll, visible_rows, track_h, theme);
    crate::scrollbar::wrap_pane_with_bar(grid, bar)
}

/// Wrap the tiled workspace in the surface-filled paint root (tagged [`ROOT_TAG`])
/// that fills the window, so the flex-Row tiling fills it and each pane's rect
/// derives from its split share (§3, per-pane via R1012). The surface shows
/// through the inter-pane divider gap. Pure composition; the unit test exercises
/// it without a PTY.
fn compose(content: Scene, theme: &Theme) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![content])
            .with_tag(ROOT_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(LayoutStyle::new().with_size(fill_size())),
    )
}

/// A both-axes `Percent(100)` size — fill the parent slot. The ONE definition,
/// shared by [`compose`] and [`crate::split::view_split_row`]'s fill (so the
/// "fill the window" literal lives in one place).
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
            // A stand-in grid (the real one is the host's pane_view_scene,
            // tested in sprag-host) — compose only owns the root wrapping.
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
