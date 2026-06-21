//! Per-pane vertical scrollbar (gnome-terminal style) — PAINT-ONLY v1.
//!
//! A PURE PROJECTION of the row-offset authority [`use_scroll_offset`](crate::input)
//! (`Signal<usize>`, rows up from the live bottom). There is NO `ScrollState`, NO
//! mirror, NO back-map, NO drag — so there is exactly one authority for "where is
//! this pane scrolled" (the row offset), and the bar reads it the same frame the
//! grid is projected from it. The thumb tracks keyboard scrolling
//! (`Shift+PageUp/Down`) for free, because it is a function of the same offset the
//! `Shift+PageUp/Down` chord writes.
//!
//! ## Why row-driven (no `cell_h`, no `ScrollState`)
//!
//! pinion's [`scrollbar_thumb_rect`] is unit-agnostic: `viewport_extent` /
//! `content_extent` / `scroll_offset` are dimensionless ratios; only the track
//! length is pixels. So the thumb is driven directly in ROWS — the terminal's
//! scroll quantum — and the one pixel input, the track height, is the R1012
//! measured pane rect ([`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)`(pane_tag(i)).1`)
//! — the SAME winsize SSOT the reflow Effect reads, so the bar's track and the
//! PTY's `rows` derive from one rect (§3, vertical axis). This is the public
//! geometry primitive consumed with the terminal's own unit model; `pinion`'s
//! `view_vertical_scrollbar` is the OTHER consumer (px, `ScrollState`-coupled).
//!
//! ## DRAG is deferred to pinion PR-16
//!
//! A draggable bar needs `ScrollBarExternal`, which hard-wires writing a px
//! `ScrollState` (it READS `state.offset` for the press snapshot AND WRITES it on
//! drag), so wiring it to the row authority would force a bidirectional
//! mirror+echo — the codebase's first dual-source-of-truth, with a convergence
//! that breaks under live-output `scrollback_len` drift. `SplitterExternal` has no
//! such problem (`attach_ratio` lets the caller own the authority Signal). That
//! asymmetry is a pinion gap, reported as `claudedocs/PINION-PR16`; the drag waits
//! for that caller-owned drag-authority seam rather than shipping the mirror.

use crate::terminal::MAX_PANES;
use pinion_core::scene::{ContainerNode, Rect, Scene};
use pinion_core::style::{AlignItems, BoxStyle, FlexDirection, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widgets::scrollbar::{ScrollBarOrientation, scrollbar_thumb_rect};

/// Gutter (track) width, px — M3 desktop canonical (matches
/// `VerticalScrollbarStyle::material`'s default).
const GUTTER_W: u32 = 8;
/// Thumb extent floor, px — M3 / UIKit convention so the thumb stays grabbable
/// on very long history (load-bearing once PR-16 lands the drag).
const MIN_THUMB_PX: u32 = 24;
/// Thumb corner radius, px — matches `view_vertical_scrollbar`.
const THUMB_RADIUS: u32 = 2;

/// The per-pane scrollbar track tags (`sprag_gui.scrollbar.<i>`), one per possible
/// pane — the identity SSOT this module owns, mirroring
/// [`PANE_TAGS`](crate::terminal) / `SPLITTER_TAGS`. Static `&'static str`: the
/// paint tag (here) and the future `ScrollBarExternal` registration tag (PR-16)
/// must be the same literal. Pointer-only, never `with_focusable` — so they stay
/// out of R1020's scene-derived Tab order (the splitter-handle discipline).
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

/// Pane `i`'s scrollbar track tag (`i < `[`MAX_PANES`]).
pub(crate) fn scrollbar_tag(i: usize) -> &'static str {
    SCROLLBAR_TAGS[i]
}

/// Build pane `i`'s vertical scrollbar as a pure projection of its row offset.
///
/// * `rows` = `use_scroll_offset(i)` (rows up from the live bottom; `0` = live).
/// * `scrollback_len` / `visible_rows` = the pane's live screen (one borrow).
/// * `track_h_px` = R1012 `use_pane_viewport_size(pane_tag(i)).1` (the height SSOT).
///
/// Row-driven [`scrollbar_thumb_rect`]: `viewport = visible_rows`, `content =
/// visible_rows + scrollback_len`, and the INVERSION `scroll_offset =
/// scrollback_len - rows` — `rows = 0` (live) ⇒ `scroll_offset = max` ⇒ thumb at
/// the BOTTOM; `rows = scrollback_len` (oldest) ⇒ `scroll_offset = 0` ⇒ thumb at
/// the TOP (gnome-terminal). With no history (`content <= viewport`) pinion fills
/// the thumb to the whole track ("nothing to scroll"). `track_h_px == 0` (boot,
/// pre-layout) yields a zero-extent track the backend elides — the bar appears on
/// the first measured frame.
pub(crate) fn view_pane_scrollbar(
    i: usize,
    rows: usize,
    scrollback_len: usize,
    visible_rows: u16,
    track_h_px: u32,
    theme: &Theme,
) -> Scene {
    let track = Rect::new(0, 0, GUTTER_W, track_h_px);
    let viewport = u32::from(visible_rows);
    let content = viewport.saturating_add(u32::try_from(scrollback_len).unwrap_or(u32::MAX));
    // The inversion, clamped: a stale `rows` past history (reflow shrank it) pins
    // to the oldest (scroll_offset 0 = top).
    let rows = rows.min(scrollback_len);
    let scroll_offset = u32::try_from(scrollback_len - rows).unwrap_or(u32::MAX);
    let geom = scrollbar_thumb_rect(
        ScrollBarOrientation::Vertical,
        track,
        viewport,
        content,
        scroll_offset,
        MIN_THUMB_PX,
    );
    let thumb_y = geom.thumb.y.saturating_sub(geom.track.y);
    let thumb = Scene::Container(
        ContainerNode::new(vec![])
            .with_style(
                BoxStyle::filled(theme.resolve(ColorRole::Outline))
                    .with_corner_radius(THUMB_RADIUS),
            )
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

    fn build(rows: usize, scrollback_len: usize, visible_rows: u16, track_h: u32) -> Scene {
        Owner::new().run(|| {
            let theme = use_theme("app").theme_animated();
            view_pane_scrollbar(0, rows, scrollback_len, visible_rows, track_h, &theme)
        })
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
    fn live_offset_puts_the_thumb_at_the_bottom() {
        // rows == 0 (live) -> thumb at the bottom: its top edge is near
        // track_h - thumb_h (not 0).
        let bar = build(0, 100, 24, 600);
        let y = thumb_y_of(&bar);
        assert!(y > 0, "live -> thumb below the top, got y={y}");
    }

    #[test]
    fn oldest_offset_puts_the_thumb_at_the_top() {
        // rows == scrollback_len (oldest) -> scroll_offset 0 -> thumb top edge 0.
        let bar = build(100, 100, 24, 600);
        assert_eq!(thumb_y_of(&bar), 0, "oldest -> thumb at the very top");
    }

    #[test]
    fn scrolling_up_moves_the_thumb_up_monotonically() {
        // More rows up from the bottom -> thumb higher (smaller y). Monotone.
        let y_live = thumb_y_of(&build(0, 100, 24, 600));
        let y_mid = thumb_y_of(&build(50, 100, 24, 600));
        let y_old = thumb_y_of(&build(100, 100, 24, 600));
        assert!(
            y_live > y_mid && y_mid > y_old,
            "{y_live} > {y_mid} > {y_old}"
        );
    }

    #[test]
    fn no_history_fills_the_thumb() {
        // scrollback_len == 0 -> content == viewport -> "nothing to scroll":
        // thumb fills the track (thumb_h == track_h), pinned at the top.
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
    fn wrap_makes_the_grid_flex_grow_and_keeps_the_bar_fixed() {
        let owner = Owner::new();
        let (wrapped, grid_tag) = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("the_grid"));
            let bar = view_pane_scrollbar(0, 0, 50, 24, 600, &theme);
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
        // child 0 = grid (flex-grow 1, basis 0); child 1 = bar (fixed gutter).
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
