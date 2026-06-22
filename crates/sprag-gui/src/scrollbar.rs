//! Per-pane vertical scrollbar (gnome-terminal style) — DRAGGABLE + wheel (R49/R50).
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
//! bottom (newest) — the gnome-terminal sense the `ScrollBarExternal` drag
//! expects (top edge -> smallest offset, bottom edge -> `scroll_max`). `max_y` is
//! the retained scrollback depth in rows, reconciled to the live screen each frame
//! by [`reconcile_scroll`]. The projection wants "rows up from the live bottom",
//! so the boundary conversion [`offset_lines_from_top`] (`scrollback_len -
//! offset_y`) is computed fresh per frame — never a second stored value.
//!
//! ## Row-unit scrollbar (R1032 §5.45 seam)
//!
//! pinion's [`ScrollState`] is unit-neutral (an `i32` offset/max holder), so the
//! row IS the scroll quantum end-to-end: keyboard scroll, drag, and wheel all move
//! `offset_y` in rows. [`view_pane_scrollbar`] paints via pinion
//! [`view_vertical_scrollbar`] with `.with_viewport_extent(visible_rows)` (the
//! R1032 row-unit override — the px track height cancels against the row extent in
//! `scrollbar_thumb_rect`), so the track's px height — the R1012 measured pane rect
//! ([`use_pane_viewport_size`](pinion_core::use_pane_viewport_size)`(pane_tag(i)).1`),
//! the SAME winsize SSOT the reflow Effect reads (§3, vertical axis) — and its row
//! content stay consistent.
//!
//! ## Thumb contrast + wheel (R1046 / R1045 seams)
//!
//! The thumb is pinion's [`view_vertical_scrollbar`] with
//! `.with_thumb_role(OnSurfaceMuted)` (R1046) for the always-visible gnome-terminal
//! idle thumb (the faint-`Outline` R48 complaint, now a one-line override) — sprag
//! no longer hand-paints it. Drag / geometry / interaction are fully pinion
//! ([`scrollbar_extra_external`], [`use_scrollbar_interaction`]). Wheel / touchpad
//! scroll lives GUI-side in [`apply_wheel`](crate::TerminalViewer) (pinion R1045)
//! driving [`wheel_scroll_pane`] on the SAME `ScrollState` — never the AI-facing
//! pane engine (the R1.7 boundary the session review enforced).

use crate::terminal::{pane_cache_key, pane_scroll_key, pane_scrollbar_tag};
use pinion_core::reactive::Owner;
use pinion_core::scene::{ContainerNode, Scene};
use pinion_core::style::{AlignItems, FlexDirection, LayoutStyle, SizeValue};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::scroll::{ScrollState, use_scroll_state};
use pinion_core::widgets::scrollbar::{scrollbar_extra_external, use_scrollbar_interaction};
use pinion_core::widgets::virtual_list::{at_bottom, follow_tail};
use pinion_widget_paint::scrollbar::{VerticalScrollbarStyle, view_vertical_scrollbar};
use std::cell::Cell;
use std::rc::Rc;

/// Rows scrolled per wheel LINE (a notched-wheel `Lines.dy`, or a touchpad
/// `Pixels.dy / LINE_HEIGHT_PX`) — the gnome-terminal ~3-lines-per-notch feel.
const WHEEL_ROWS_PER_LINE: f32 = 3.0;

/// Pane `i`'s scroll authority — the row-unit [`ScrollState`] shared by the
/// keyboard chords ([`crate::input`]), the projection + reconcile
/// ([`crate::view`]), the paint ([`view_pane_scrollbar`]), and the drag External
/// ([`pane_scrollbar_external`]). `Owner::cache`-backed (keyed by
/// [`pane_scroll_key`], the per-pane identity
/// SSOT) so every site resolves the one slot. `offset_y` = rows from the oldest
/// line (`0` = top, `max_y` = live).
pub(crate) fn use_pane_scroll(i: usize) -> Rc<ScrollState> {
    use_scroll_state(pane_scroll_key(i))
}

/// Register pane `i`'s draggable scrollbar peer — a `ScrollBarExternal` over the
/// pane's [`ScrollState`], tagged [`pane_scrollbar_tag`]`(i)` so the painted track
/// routes pointer drags to it. Wired in
/// [`create_extra_externals`](crate::TerminalViewer); the shell's pointer router
/// (capture-locked drag) translates the cursor to [`ScrollState::scroll_to`] in
/// rows — no `WidgetCore` pointer method, mirroring the splitter.
pub(crate) fn pane_scrollbar_external(i: usize) -> ExtraExternal {
    scrollbar_extra_external(use_pane_scroll(i), pane_scrollbar_tag(i))
}

/// Reconcile pane `i`'s scroll bound to the live scrollback depth and follow the
/// tail by delegating to pinion's [`follow_tail`] reducer in the terminal's ROW
/// unit (`row_pitch = 1`, `viewport_h = 0`, so the content extent IS the row count
/// and the bound becomes `scrollback_len`). Run from
/// [`TerminalViewer::reconcile_frame`](crate::TerminalViewer) — pinion R1047's
/// pre-view binding-reconcile hook, the **sanctioned non-view-fn place** for this
/// reactive `Signal` write (the view fn stays pure). Why here and not the runtime's
/// own reducer / an `Effect`: the canonical post-layout reducer
/// `update_scroll_state_bounds` only walks `Scene::Scroll` clip nodes, but the
/// terminal grid is a `Scene::TextGrid` that windows history via `offset_lines` (no
/// clip node), and `scrollback_len` lives in an off-thread PTY producer with no
/// reactive `Signal` for an `Effect` to subscribe — a substrate-integration gap
/// pinion closed with `reconcile_frame` (R1047 / PR-20).
///
/// `follow_tail` grows the bound to `scrollback_len`; if the view WAS at the live
/// bottom it pins to the new bottom (live output keeps following), otherwise the
/// offset holds (a paused history view stays on its content as the extent grows
/// beneath it). Loop-safe: [`ScrollState::set_max`] / [`ScrollState::scroll_to`]
/// equality-skip, so once the scrollback stops growing this is a no-op.
pub(crate) fn reconcile_scroll(scroll: &ScrollState, scrollback_len: usize) {
    let was_following = at_bottom(scroll.offset_y(), scroll.max().1);
    follow_tail(scroll, scrollback_len, 1, 0, was_following);
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

/// Scroll pane `i` by a wheel/touchpad delta expressed in LINES — the GUI-side
/// wheel handler ([`TerminalViewer::apply_wheel`](crate::TerminalViewer), pinion
/// R1045/PR-18) drives this on the **GUI's own** [`ScrollState`] (no coupling into
/// the AI-facing pane engine — the R1.7 boundary the session review enforced).
///
/// Converts lines -> rows at [`WHEEL_ROWS_PER_LINE`], carrying the sub-row
/// remainder in a per-pane `Owner::cache` `Cell` (keyed via [`pane_cache_key`]) so a
/// fine touchpad pan accumulates to whole rows instead of rounding to zero each
/// event. `+lines` (W3C scroll-down) maps to `+offset_y` (toward the live bottom) —
/// no inversion. The remainder is DISCARDED when [`ScrollState::scroll_by`] clamps
/// at a `[0, max]` boundary (offset unchanged), so a reversal never starts with a
/// stale wrong-side fraction — the accumulator never outruns the authority. Must run
/// inside the binding root `Owner` scope (the shell wraps `apply_wheel` in it).
pub(crate) fn wheel_scroll_pane(i: usize, lines: f32) {
    let owner = Owner::current().expect("wheel_scroll_pane() requires an active Owner scope");
    let accum = owner.cache(pane_cache_key("wheel_accum", i), || Cell::new(0.0_f32));
    let total = accum.get() + lines * WHEEL_ROWS_PER_LINE;
    let rows = total.trunc();
    if rows == 0.0 {
        accum.set(total); // sub-row pan: carry the fraction for the next event
        return;
    }
    let scroll = use_pane_scroll(i);
    let before = scroll.offset_y();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "trunc()'d row count is small and bounded by the wheel delta"
    )]
    scroll.scroll_by(0, rows as i32);
    // Carry the remainder only if we actually moved; a clamped boundary discards it.
    accum.set(if scroll.offset_y() == before {
        0.0
    } else {
        total - rows
    });
}

/// Build pane `i`'s vertical scrollbar from its [`ScrollState`] — pinion's
/// [`view_vertical_scrollbar`] in the terminal's ROW unit (R1046 collapse: sprag no
/// longer hand-paints the thumb).
///
/// * `scroll` = [`use_pane_scroll`]`(i)` — `offset_y` (rows from top) is the thumb
///   position; `max().1` (== `scrollback_len`, reconciled) is the extent.
/// * `visible_rows` = the pane's live screen rows, the thumb's `viewport_extent`
///   (row unit — the px track height cancels against it in `scrollbar_thumb_rect`,
///   R1032).
/// * `track_h_px` = the measured track height in px (the caller passes the pane's
///   R1012 [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) height).
///
/// [`with_thumb_role`](VerticalScrollbarStyle::with_thumb_role)`(OnSurfaceMuted)`
/// (pinion R1046) gives the always-visible gnome-terminal thumb (idle
/// `OnSurfaceMuted`, brightening to `OnSurface` on hover/drag) instead of the faint
/// default `Outline` — the R48 contrast complaint, now a one-line style override.
/// No history fills the thumb; `track_h_px == 0` (boot) yields a zero-extent track
/// the backend elides; the interaction state comes from [`use_scrollbar_interaction`]
/// (the signal the `ScrollBarExternal`(pinion_core::widgets::scrollbar) drag writes).
pub(crate) fn view_pane_scrollbar(
    i: usize,
    scroll: &Rc<ScrollState>,
    visible_rows: u16,
    track_h_px: u32,
    theme: &Theme,
) -> Scene {
    let style = VerticalScrollbarStyle::material(track_h_px, pane_scrollbar_tag(i))
        .with_viewport_extent(u32::from(visible_rows))
        .with_thumb_role(ColorRole::OnSurfaceMuted);
    view_vertical_scrollbar(
        scroll,
        theme,
        &style,
        use_scrollbar_interaction(pane_scrollbar_tag(i)).get(),
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
    fn wheel_scroll_pane_moves_rows_with_sign_and_accumulates_subrows() {
        Owner::new().run(|| {
            let scroll = use_pane_scroll(0);
            scroll.set_max(0, 100);
            scroll.scroll_to(0, 100); // live bottom
            // Wheel UP 1 line -> 3 rows toward history (offset_y decreases). W3C
            // -dy = up; no inversion.
            wheel_scroll_pane(0, -1.0);
            assert_eq!(scroll.offset_y(), 97, "1 line up = 3 rows toward history");
            // Wheel DOWN 1 line -> back toward the live bottom.
            wheel_scroll_pane(0, 1.0);
            assert_eq!(scroll.offset_y(), 100, "1 line down = 3 rows back to live");
            // Sub-line touchpad pan accumulates: 0.1 line (<1 row) scrolls nothing
            // yet, but the remainder carries so 0.1 + 0.3 line = 1.2 rows -> 1 row.
            scroll.scroll_to(0, 50);
            wheel_scroll_pane(0, -0.1);
            assert_eq!(scroll.offset_y(), 50, "sub-row pan scrolls nothing yet");
            wheel_scroll_pane(0, -0.3);
            assert_eq!(
                scroll.offset_y(),
                49,
                "accumulated remainder crosses one row"
            );
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
        assert!(bar.contains_tag(pane_scrollbar_tag(0)), "track tagged");
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
            assert_eq!(ext.tag.as_ref(), pane_scrollbar_tag(1));
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
            SizeValue::Px(8),
            "bar is the fixed gutter (VerticalScrollbarStyle::material default 8px)"
        );
        assert_eq!(
            bar.tag.as_deref(),
            Some(pane_scrollbar_tag(0)),
            "bar carries its track tag"
        );
    }
}
