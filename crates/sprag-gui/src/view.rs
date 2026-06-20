//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::ROOT_TAG;
use crate::dock::{is_pane_floating, pane_window_index, use_windows_topology};
use crate::input::{use_preedit, use_scroll_offset};
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
        Some(i) if i < tv.pane_count() => compose(build_pane_scene(&tv, i), &theme),
        _ => view_main(&tv, &theme),
    }
}

/// The main window: arrange the DOCKED panes with draggable dividers (a floated
/// pane is painted in its own undock window, not here — [`is_pane_floating`] is
/// the partition SSOT). The docked panes go through [`crate::split::view_split_row`]
/// (R38): even at boot, draggable to resize, nested `view_splitter`s for N>2. An
/// all-floated workspace yields zero panes -> an empty surface-filled root (dock
/// any pane back to recover).
fn view_main(tv: &TerminalView, theme: &Theme) -> Scene {
    let windows = use_windows_topology().get();
    let panes: Vec<Scene> = (0..tv.pane_count())
        .filter(|&i| !is_pane_floating(&windows, i))
        .map(|i| build_pane_scene(tv, i))
        .collect();
    compose(crate::split::view_split_row(panes, theme), theme)
}

/// Build ONE pane's scene from its live screen + per-pane scroll offset + IME
/// preedit — the single per-pane builder shared by the docked tiling
/// ([`view_main`]) and an undock window ([`view_for_window`]). Reading the pane's
/// offset/preedit Signal subscribes the paint to them (the R705.1 reactive
/// bridge), so a per-pane scroll or composition `set` repaints live. `offset == 0`
/// is the live screen; a positive offset windows into history (text-only, R16);
/// the preedit overlays only the live view (the host seam self-gates on the
/// cursor). The String + screen borrows are confined to the `with_screen` closure.
/// On child EOF the pane paints its frozen final screen.
fn build_pane_scene(tv: &TerminalView, i: usize) -> Scene {
    let offset = use_scroll_offset(i).get();
    let preedit = use_preedit(i).get();
    tv.pane(i).session().with_screen(|screen| {
        sprag_host::pane_view_scene(
            pane_tag(i),
            PaneViewSpec {
                screen,
                metric: tv.metric,
                font_size_px: tv.font_size_px,
                offset_lines: offset,
                preedit: &preedit,
            },
        )
    })
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
