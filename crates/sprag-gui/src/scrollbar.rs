//! Per-pane vertical scrollbar (gnome-terminal style) — DRAGGABLE (R49).
//!
//! The scroll authority is a per-pane pinion [`ScrollState`] (row-unit): a single
//! source of truth that the keyboard chords ([`crate::input`]), the projection
//! ([`crate::view`]), and the bar's own paint + drag all read/write. There is NO
//! separate `Signal<usize>` offset mirror — the paint-only R16-era model (a
//! one-way projection of a sprag-owned offset Signal, no drag) is gone.
//!
//! ## Row-unit `ScrollState` (the single authority)
//!
//! `offset_y` is the row offset **from the oldest retained line (the top)**, so
//! `offset_y == 0` is the top of history and `offset_y == max_y` is the live
//! bottom (newest) — the gnome-terminal sense the [`ScrollBarExternal`] drag
//! expects (top edge -> smallest offset, bottom edge -> `scroll_max`). `max_y` is
//! the retained scrollback depth in rows, reconciled to the live screen each frame
//! by [`reconcile_scroll`]. The projection wants "rows up from the live bottom",
//! so the boundary conversion [`offset_lines_from_top`] (`scrollback_len -
//! offset_y`) is computed fresh per frame — never a second stored value.
//!
//! ## Why row units, not pixels (R1032 §5.45 seam)
//!
//! pinion's [`ScrollState`] is unit-neutral (an `i32` offset/max holder), so the
//! row IS the scroll quantum end-to-end: keyboard scroll, the drag's
//! `scroll_to`, and the thumb geometry all stay in rows. The one pixel input is
//! the track height — the R1012 measured pane rect
//! ([`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)`(pane_tag(i)).1`),
//! the SAME winsize SSOT the reflow Effect reads (§3, vertical axis). pinion's
//! [`view_vertical_scrollbar`](pinion_widget_paint::scrollbar::view_vertical_scrollbar)
//! is the px-unit consumer; this is the row-unit consumer of the same
//! [`scrollbar_thumb_rect`] geometry primitive.
//!
//! ## Why sprag paints the thumb (not `view_vertical_scrollbar`)
//!
//! The drag substrate is fully pinion ([`ScrollState`], [`scrollbar_extra_external`],
//! [`use_scrollbar_interaction`]); only the thumb/track FILL is sprag's, because a
//! terminal wants an always-visible bar (gnome-terminal) whereas
//! `view_vertical_scrollbar` hard-wires an `Outline` idle thumb with no color
//! override — the low-contrast fill the R48 bar showed. So the bar reads the SAME
//! pinion interaction signal the drag external writes (idle/hover/dragging) and
//! resolves a higher-contrast role per state ([`thumb_fill`]). If pinion later adds
//! a thumb-style override, this collapses to `view_vertical_scrollbar`.

use crate::terminal::MAX_PANES;
use pinion_core::scene::{ContainerNode, Rect, Scene};
use pinion_core::style::{
    AlignItems, BoxStyle, Color, FlexDirection, LayoutStyle, Size, SizeValue,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::scroll::{ScrollState, use_scroll_state};
use pinion_core::widgets::scrollbar::{
    ScrollBarOrientation, ScrollBarState, scrollbar_extra_external, scrollbar_thumb_rect,
    use_scrollbar_interaction,
};
use pinion_core::widgets::virtual_list::at_bottom;
use std::rc::Rc;

/// Gutter (track) width, px — M3 desktop canonical (matches
/// `VerticalScrollbarStyle::material`'s default).
const GUTTER_W: u32 = 8;
/// Thumb extent floor, px — M3 / UIKit convention so the thumb stays grabbable
/// on very long history.
const MIN_THUMB_PX: u32 = 24;
/// Thumb corner radius, px — matches `view_vertical_scrollbar`.
const THUMB_RADIUS: u32 = 2;

/// The per-pane scrollbar track + drag-External tags (`sprag_gui.scrollbar.<i>`),
/// one per possible pane — the identity SSOT this module owns, mirroring
/// [`PANE_TAGS`](crate::terminal). Static `&'static str`: the paint tag (here) and
/// the [`ScrollBarExternal`] registration tag ([`pane_scrollbar_external`]) MUST be
/// the same literal so the shell's pointer router hit-tests the painted track and
/// routes the drag to the matching External. Pointer-only, never `with_focusable`
/// — so they stay out of R1020's scene-derived Tab order (the splitter-handle
/// discipline).
const SCROLLBAR_TAGS: [&str; MAX_PANES] = [
    "sprag_gui.scrollbar.0",
    "sprag_gui.scrollbar.1",
    "sprag_gui.scrollbar.2",
    "sprag_gui.scrollbar.3",
    "sprag_gui.scrollbar.4",
    "sprag_gui.scrollbar.5",
    "sprag_gui.scrollbar.6",
    "sprag_gui.scrollbar.7",
];

/// The per-pane [`ScrollState`] cache keys (`sprag_gui.scroll.<i>`), DISTINCT from
/// [`SCROLLBAR_TAGS`]: [`use_scroll_state`] caches the `ScrollState` under this key
/// while [`use_scrollbar_interaction`] caches the interaction signal under the tag,
/// so the two `Owner::cache` slots must not collide. Static for the same
/// `&'static str` reason as the tags.
const SCROLL_STATE_KEYS: [&str; MAX_PANES] = [
    "sprag_gui.scroll.0",
    "sprag_gui.scroll.1",
    "sprag_gui.scroll.2",
    "sprag_gui.scroll.3",
    "sprag_gui.scroll.4",
    "sprag_gui.scroll.5",
    "sprag_gui.scroll.6",
    "sprag_gui.scroll.7",
];

/// Pane `i`'s scrollbar track / drag-External tag (`i < `[`MAX_PANES`]).
pub(crate) fn scrollbar_tag(i: usize) -> &'static str {
    SCROLLBAR_TAGS[i]
}

/// Pane `i`'s [`ScrollState`] cache key (`i < `[`MAX_PANES`]).
fn scroll_state_key(i: usize) -> &'static str {
    SCROLL_STATE_KEYS[i]
}

/// Pane `i`'s scroll authority — the row-unit [`ScrollState`] shared by the
/// keyboard chords ([`crate::input`]), the projection + reconcile
/// ([`crate::view`]), the paint ([`view_pane_scrollbar`]), and the drag External
/// ([`pane_scrollbar_external`]). `Owner::cache`-backed so every site resolves the
/// one slot. `offset_y` = rows from the oldest line (`0` = top, `max_y` = live).
pub(crate) fn use_pane_scroll(i: usize) -> Rc<ScrollState> {
    use_scroll_state(scroll_state_key(i))
}

/// Register pane `i`'s draggable scrollbar peer — a [`ScrollBarExternal`] over the
/// pane's [`ScrollState`], tagged [`scrollbar_tag`]`(i)` so the painted track
/// routes pointer drags to it. Wired in
/// [`create_extra_externals`](crate::TerminalViewer); the shell's pointer router
/// (capture-locked drag) translates the cursor to [`ScrollState::scroll_to`] in
/// rows — no `WidgetCore` pointer method, mirroring the splitter.
pub(crate) fn pane_scrollbar_external(i: usize) -> ExtraExternal {
    scrollbar_extra_external(use_pane_scroll(i), scrollbar_tag(i))
}

/// Reconcile pane `i`'s scroll bound to the live scrollback depth and follow the
/// tail — the row-unit shape of [`follow_tail`](pinion_core::widgets::virtual_list::follow_tail),
/// run once per frame from [`build_pane_scene`](crate::view) (the one main-thread
/// hook per PTY batch — the producer's `on_dirty` runs off-thread and only
/// `request_repaint`s, so no reactive dep flips an `Effect`).
///
/// `max_y` becomes `scrollback_len` (rows); if the view WAS at the live bottom it
/// is pinned to the new bottom (so live output keeps following), otherwise the
/// offset holds (a paused history view stays on its content as the extent grows
/// beneath it). Loop-safe: [`ScrollState::set_max`] / [`ScrollState::scroll_to`]
/// equality-skip, so once the scrollback stops growing this is a no-op (no repaint
/// cascade); a growing scrollback already requested a repaint via R999.
pub(crate) fn reconcile_scroll(scroll: &ScrollState, scrollback_len: usize) {
    let max_y = i32::try_from(scrollback_len).unwrap_or(i32::MAX);
    let was_following = at_bottom(scroll.offset_y(), scroll.max().1);
    scroll.set_max(0, max_y);
    if was_following {
        scroll.scroll_to(0, max_y);
    }
}

/// Convert the authority's top-anchored `offset_y` (rows from the oldest line)
/// into the projection's "rows up from the live bottom"
/// ([`PaneViewSpec::offset_lines`](sprag_host::PaneViewSpec)): `scrollback_len -
/// offset_y`. `offset_y == max_y == scrollback_len` (live) -> `0` (live bottom);
/// `offset_y == 0` (oldest) -> `scrollback_len` (top of history). A boundary
/// conversion computed fresh per frame, NOT a second stored offset. Pure /
/// unit-tested.
pub(crate) fn offset_lines_from_top(offset_y: i32, scrollback_len: usize) -> usize {
    let from_top = usize::try_from(offset_y.max(0)).unwrap_or(0);
    scrollback_len.saturating_sub(from_top)
}

/// The thumb fill for the drag interaction `state` — a terminal-visible
/// (gnome-terminal) palette: a readable [`OnSurfaceMuted`](ColorRole::OnSurfaceMuted)
/// at idle (NOT pinion's faint `Outline`, the R48 low-contrast complaint) that
/// brightens to [`OnSurface`](ColorRole::OnSurface) on hover / drag so the grabbed
/// thumb is unmistakable against the [`SurfaceContainerHighest`](ColorRole::SurfaceContainerHighest)
/// track.
fn thumb_fill(theme: &Theme, state: ScrollBarState) -> Color {
    match state {
        ScrollBarState::Hover | ScrollBarState::Dragging => theme.resolve(ColorRole::OnSurface),
        ScrollBarState::Idle | ScrollBarState::Disabled => theme.resolve(ColorRole::OnSurfaceMuted),
    }
}

/// Build pane `i`'s vertical scrollbar from its [`ScrollState`] — a draggable bar
/// painted in the terminal's row unit.
///
/// * `scroll` = [`use_pane_scroll`]`(i)` — `offset_y` (rows from top) is the thumb
///   position directly; `max().1` (== `scrollback_len`, reconciled) gives the
///   content extent.
/// * `visible_rows` = the pane's live screen rows (the thumb's `viewport_extent`).
/// * `track_h_px` = R1012 [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)`(pane_tag(i)).1`.
///
/// Row-driven [`scrollbar_thumb_rect`]: `viewport = visible_rows`, `content =
/// visible_rows + max_y`, `scroll_offset = offset_y`. No history (`content <=
/// viewport`) fills the thumb to the whole track ("nothing to scroll"). `track_h_px
/// == 0` (boot, pre-layout) yields a zero-extent track the backend elides — the bar
/// appears on the first measured frame. The fill tracks the drag interaction
/// ([`use_scrollbar_interaction`], the signal the [`ScrollBarExternal`] writes).
pub(crate) fn view_pane_scrollbar(
    i: usize,
    scroll: &ScrollState,
    visible_rows: u16,
    track_h_px: u32,
    theme: &Theme,
) -> Scene {
    let track = Rect::new(0, 0, GUTTER_W, track_h_px);
    let viewport = u32::from(visible_rows);
    let max_y = u32::try_from(scroll.max().1.max(0)).unwrap_or(0);
    let content = viewport.saturating_add(max_y);
    let scroll_offset = u32::try_from(scroll.offset_y().max(0)).unwrap_or(0);
    let geom = scrollbar_thumb_rect(
        ScrollBarOrientation::Vertical,
        track,
        viewport,
        content,
        scroll_offset,
        MIN_THUMB_PX,
    );
    let thumb_y = geom.thumb.y.saturating_sub(geom.track.y);
    let state = use_scrollbar_interaction(scrollbar_tag(i)).get();
    let thumb = Scene::Container(
        ContainerNode::new(vec![])
            .with_style(BoxStyle::filled(thumb_fill(theme, state)).with_corner_radius(THUMB_RADIUS))
            .with_layout(
                LayoutStyle::new()
                    .with_size(Size::px(GUTTER_W, geom.thumb.h))
                    .with_absolute_position(0, thumb_y),
            ),
    );
    Scene::Container(
        ContainerNode::new(vec![thumb])
            .with_tag(scrollbar_tag(i))
            .with_style(BoxStyle::filled(
                theme.resolve(ColorRole::SurfaceContainerHighest),
            ))
            .with_layout(LayoutStyle::new().with_size(Size::px(GUTTER_W, track_h_px))),
    )
}

/// Pair the pane's grid Container with its scrollbar in a horizontal flex Row:
/// `[grid (flex-grow 1), bar (fixed gutter)]`. The grid keeps `pane_tag(i)` and
/// stays the SOLE measured (R1012) + focusable node — the bar is a SIBLING, NOT a
/// wrapper that steals the tag — so the measured rect excludes the gutter and
/// `cols` shrink by it (the gnome-terminal width). The Row is UNTAGGED; the
/// splitter folds IT (its `apply_flex_main` sets the Row's outer flex share),
/// while this inner Row splits that share into grid + gutter.
pub(crate) fn wrap_pane_with_bar(grid: Scene, bar: Scene) -> Scene {
    let grid = match grid {
        // The grid fills the Row minus the gutter (CSS `flex-basis:0; flex-grow:1`).
        // map in place so `pane_tag` + `with_focusable` are preserved.
        Scene::Container(mut c) => {
            c.layout.flex_basis = Some(SizeValue::Px(0));
            c.layout.flex_grow = 1.0;
            Scene::Container(c)
        }
        other => other,
    };
    Scene::Container(
        ContainerNode::new(vec![grid, bar]).with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_align_items(AlignItems::Stretch),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;
    use pinion_core::theme::use_theme;

    /// The thumb's absolute-y offset within the track, for inspecting position.
    fn thumb_y_of(bar: &Scene) -> u32 {
        let Scene::Container(track) = bar else {
            panic!("bar is a Container")
        };
        let Scene::Container(thumb) = &track.children[0] else {
            panic!("thumb is the track's child")
        };
        thumb.layout.absolute_position.map_or(0, |p| p.1)
    }

    /// Build the bar over a fresh `ScrollState` with `offset_y`/`max_y` set, in an
    /// Owner scope (the interaction signal + theme are cache-backed).
    fn build(offset_y: i32, scrollback_len: usize, visible_rows: u16, track_h: u32) -> Scene {
        Owner::new().run(|| {
            let theme = use_theme("app").theme_animated();
            let scroll = use_pane_scroll(0);
            scroll.set_max(0, i32::try_from(scrollback_len).unwrap());
            scroll.scroll_to(0, offset_y);
            view_pane_scrollbar(0, &scroll, visible_rows, track_h, &theme)
        })
    }

    #[test]
    fn thumb_contrasts_with_the_track_and_brightens_on_interaction() {
        // The R48 complaint was a faint `Outline` thumb on the `SurfaceContainerHighest`
        // track. The thumb fill must now be distinct from the track at rest AND
        // brighten on hover/drag (so the grabbed thumb is unmistakable).
        Owner::new().run(|| {
            let theme = use_theme("app").theme_animated();
            let track = theme.resolve(ColorRole::SurfaceContainerHighest);
            let idle = thumb_fill(&theme, ScrollBarState::Idle);
            let active = thumb_fill(&theme, ScrollBarState::Dragging);
            assert_ne!(
                idle, track,
                "the idle thumb is distinct from the track (visible at rest)"
            );
            assert_ne!(active, idle, "the thumb brightens on hover / drag");
            assert_eq!(
                thumb_fill(&theme, ScrollBarState::Hover),
                active,
                "hover and drag share the bright fill",
            );
        });
    }

    #[test]
    fn offset_lines_from_top_converts_to_rows_up_from_bottom() {
        // Live (offset_y == max == len) -> 0 rows up (the live bottom).
        assert_eq!(offset_lines_from_top(100, 100), 0);
        // Oldest (offset_y == 0) -> the full depth up from the bottom.
        assert_eq!(offset_lines_from_top(0, 100), 100);
        // Mid.
        assert_eq!(offset_lines_from_top(60, 100), 40);
        // No history.
        assert_eq!(offset_lines_from_top(0, 0), 0);
    }

    #[test]
    fn reconcile_follows_the_tail_when_at_bottom() {
        Owner::new().run(|| {
            let scroll = use_pane_scroll(0);
            // Boot at the live bottom (offset 0 == max 0).
            reconcile_scroll(&scroll, 0);
            assert_eq!(scroll.offset_y(), 0);
            // Scrollback grows to 50: following -> pinned to the new bottom.
            reconcile_scroll(&scroll, 50);
            assert_eq!(scroll.max().1, 50, "bound grows to the depth");
            assert_eq!(scroll.offset_y(), 50, "followed to the live bottom");
        });
    }

    #[test]
    fn reconcile_holds_a_paused_history_view() {
        Owner::new().run(|| {
            let scroll = use_pane_scroll(0);
            reconcile_scroll(&scroll, 100);
            // Pause: scroll up to the oldest line (offset 0, not the bottom).
            scroll.scroll_to(0, 0);
            // More output: the extent grows but the paused offset holds.
            reconcile_scroll(&scroll, 150);
            assert_eq!(scroll.max().1, 150, "the extent grew underneath");
            assert_eq!(scroll.offset_y(), 0, "the paused view stays put");
        });
    }

    #[test]
    fn live_offset_puts_the_thumb_at_the_bottom() {
        // offset_y == max (live) -> thumb at the bottom: top edge near track_h - h.
        let bar = build(100, 100, 24, 600);
        assert!(thumb_y_of(&bar) > 0, "live -> thumb below the top");
    }

    #[test]
    fn oldest_offset_puts_the_thumb_at_the_top() {
        // offset_y == 0 (oldest) -> thumb top edge 0.
        let bar = build(0, 100, 24, 600);
        assert_eq!(thumb_y_of(&bar), 0, "oldest -> thumb at the very top");
    }

    #[test]
    fn scrolling_down_moves_the_thumb_down_monotonically() {
        // Larger offset_y (toward the live bottom) -> thumb lower (larger y).
        let y_old = thumb_y_of(&build(0, 100, 24, 600));
        let y_mid = thumb_y_of(&build(50, 100, 24, 600));
        let y_live = thumb_y_of(&build(100, 100, 24, 600));
        assert!(
            y_old < y_mid && y_mid < y_live,
            "{y_old} < {y_mid} < {y_live}"
        );
    }

    #[test]
    fn no_history_fills_the_thumb() {
        // scrollback_len == 0 -> content == viewport -> "nothing to scroll":
        // thumb fills the track, pinned at the top.
        let bar = build(0, 0, 24, 600);
        let Scene::Container(track) = &bar else {
            panic!()
        };
        let Scene::Container(thumb) = &track.children[0] else {
            panic!()
        };
        assert_eq!(
            thumb.layout.size.height,
            SizeValue::Px(600),
            "thumb fills track"
        );
        assert_eq!(thumb_y_of(&bar), 0);
    }

    #[test]
    fn unmeasured_track_is_zero_extent() {
        // Boot (track_h == 0) -> the backend elides a zero-area track; no panic.
        let bar = build(0, 100, 24, 0);
        let Scene::Container(track) = &bar else {
            panic!()
        };
        assert_eq!(track.layout.size.height, SizeValue::Px(0));
    }

    #[test]
    fn bar_carries_its_tag_and_one_thumb() {
        let bar = build(0, 100, 24, 600);
        assert!(bar.contains_tag(scrollbar_tag(0)), "track tagged");
        let Scene::Container(track) = &bar else {
            panic!()
        };
        assert_eq!(track.children.len(), 1, "exactly the thumb");
    }

    #[test]
    fn drag_external_carries_the_track_tag() {
        // The drag peer registers at the SAME tag the paint uses, so the pointer
        // router can route a press on the painted track to it.
        Owner::new().run(|| {
            let ext = pane_scrollbar_external(1);
            assert_eq!(ext.tag.as_ref(), scrollbar_tag(1));
        });
    }

    #[test]
    fn wrap_makes_the_grid_flex_grow_and_keeps_the_bar_fixed() {
        let owner = Owner::new();
        let (wrapped, grid_tag) = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("the_grid"));
            let scroll = use_pane_scroll(0);
            let bar = view_pane_scrollbar(0, &scroll, 24, 600, &theme);
            (wrap_pane_with_bar(grid, bar), "the_grid")
        });
        let Scene::Container(row) = &wrapped else {
            panic!("wrap returns a Row Container")
        };
        assert_eq!(row.layout.flex_direction, FlexDirection::Row);
        assert!(
            row.tag.is_none(),
            "the Row is untagged (the splitter folds it)"
        );
        let Scene::Container(grid) = &row.children[0] else {
            panic!()
        };
        assert_eq!(grid.tag.as_deref(), Some(grid_tag), "grid keeps its tag");
        assert!((grid.layout.flex_grow - 1.0).abs() < f32::EPSILON);
        assert_eq!(grid.layout.flex_basis, Some(SizeValue::Px(0)));
        let Scene::Container(bar) = &row.children[1] else {
            panic!()
        };
        assert_eq!(
            bar.layout.size.width,
            SizeValue::Px(GUTTER_W),
            "bar is the fixed gutter"
        );
        assert_eq!(
            bar.tag.as_deref(),
            Some(scrollbar_tag(0)),
            "bar carries its track tag"
        );
    }
}
