//! The pure view-fn (§6.3): read the producer-authoritative pane screen each
//! frame and project it (live, or a scrollback window) into the surface-filled
//! paint root. The PTY producer thread lives in `create_extra_externals`, not
//! here. See the crate-root module docs.

use crate::input::{use_preedit, use_scroll_offset};
use crate::terminal::use_terminal;
use crate::ROOT_TAG;
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{use_theme, ColorRole, Theme};
use pinion_core::{Frame, Scene};

/// Shared [`ThemeProvider`](pinion_core::ThemeProvider) cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";

/// view-fn (§6.3): pure sync `() -> Scene`. Reads the producer-authoritative
/// screen of the (single) pane each frame and paints it (live, or a scrollback
/// window when [`use_scroll_offset`] is non-zero) via the host's projection
/// seam; the producer thread (the PTY reader) lives in `create_extra_externals`,
/// not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn view(_state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    let offset = use_scroll_offset().get();
    // Read the IME preedit every frame so a `set` (a composing keystroke) flips
    // the owner dirty and arms a redraw (the R705.1 reactive bridge) — the
    // half-composed syllable repaints live. Empty when not composing (no overlay).
    let preedit = use_preedit().get();
    // On child EOF the pane stays and `view` paints its frozen final screen
    // (the program exited; its last output shows) — not a loss of interactivity,
    // just no more PTY output to read.
    let pane = tv.boot_pane();
    // `offset == 0` is the live screen; a positive offset windows into history
    // (text-only, the R16 scrollback model) — one projection seam for both. The
    // preedit overlays only the live view (the host seam gates on `offset`).
    let grid = pane.session().with_screen(|screen| {
        sprag_host::pane_view_scene_scrolled_with_preedit(
            screen,
            tv.metric,
            tv.font_size_px,
            offset,
            &preedit,
        )
    });
    compose(grid, &theme)
}

/// Wrap the pane grid in the surface-filled paint root (tagged [`ROOT_TAG`])
/// that fills the window, so the single pane grid fills it and its rect = the
/// viewport (§3). Pure composition; the unit test exercises it without a PTY.
fn compose(grid: Scene, theme: &Theme) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![grid])
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
