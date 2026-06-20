//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::ROOT_TAG;
use crate::input::{use_preedit, use_scroll_offset};
use crate::terminal::{pane_tag, use_terminal};
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{ColorRole, Theme, use_theme};
use pinion_core::{Frame, Scene};
use sprag_host::PaneViewSpec;

/// Shared [`ThemeProvider`](pinion_core::ThemeProvider) cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";

/// view-fn (§6.3): pure sync `() -> Scene`. Builds each tiled pane's scene from
/// its own producer-authoritative screen (live, or a scrollback window when that
/// pane's [`use_scroll_offset`] is non-zero, with its [`use_preedit`] overlay) via
/// the host's per-pane projection seam, then tiles them ([`workspace_view_scene`])
/// into the surface-filled paint root. The producer threads (the PTY readers)
/// live in `create_extra_externals`, not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn view(_state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    // Build each pane from its live screen + per-pane scroll offset + IME
    // preedit. Reading every pane's offset/preedit Signal each frame subscribes
    // `view` to them (the R705.1 reactive bridge), so a per-pane scroll or
    // composition `set` flips the owner dirty and repaints live. On child EOF a
    // pane stays and paints its frozen final screen (no more PTY output to read).
    let panes: Vec<Scene> = (0..tv.pane_count())
        .map(|i| {
            let offset = use_scroll_offset(i).get();
            let preedit = use_preedit(i).get();
            // `offset == 0` is the live screen; a positive offset windows into
            // history (text-only, the R16 model). The preedit overlays only the
            // live view (the host seam self-gates on the cursor). The String +
            // screen borrows are confined to this `with_screen` closure.
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
        })
        .collect();
    compose(sprag_host::workspace_view_scene(panes), &theme)
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
            .with_layout(LayoutStyle::new().with_size(fill())),
    )
}

/// A both-axes `Percent(100)` size — fill the parent slot.
fn fill() -> Size {
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
