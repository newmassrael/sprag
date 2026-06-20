//! Draggable pane dividers (R38): arrange the docked panes left-to-right with
//! pinion `view_splitter` handles you can DRAG to resize, instead of the former
//! even flex split. Orthogonal to `dock` (which window) and `terminal` (which
//! panes) — this owns the divider RATIOS and the splitter-handle tags.
//!
//! ## The seam (pinion-widget-paint)
//!
//! `view_splitter(left, right, &ratio, theme, style, dragging)` emits a flex
//! Container with `left` (flex_grow = ratio), a tagged 4px handle, `right`
//! (flex_grow = 1 - ratio). A [`SplitterExternal`](pinion_widget_paint::splitter::SplitterExternal)
//! registered at the handle tag (in [`create_extra_externals`](crate::TerminalViewer))
//! mutates the SAME ratio `Signal` on a pointer drag — the shell's pointer router
//! delivers press/move/release to it automatically (no `WidgetCore` pointer
//! method). The ratio is `Owner::cache`-shared between the read side
//! ([`view_split_row`]) and the write side (the External), so a drag re-weights
//! the painted panes.
//!
//! ## N panes -> N-1 nested splitters (position-keyed ratios)
//!
//! `view_splitter` is binary, so N panes nest N-1 splitters ([`split_fold`]):
//! divider `j` separates docked tile `j` from everything to its right (the
//! proportional split-tree model hello-dock-panels ships). Ratios are keyed by
//! docked POSITION, not pane identity — so undock/dock (which re-compacts the
//! docked set) reshuffles which panes a remembered divider position separates.
//! That is the honest splitter-tree behavior, documented, not a per-pane sticky
//! layout (which would need a real split-tree — premature at `MAX_PANES` = 8).
//!
//! ## Reflow-on-drag is automatic (no new code)
//!
//! A drag mutates the ratio `Signal` -> `view` re-renders with new flex weights
//! -> `compute_layout` moves each `pane_tag(i)` Container's rect -> pinion R1012
//! publishes it -> the per-pane reflow Effect ([`reflow`](crate::reflow)) resizes
//! the PTY. The reflow's equality-skip bounds it to cell-boundary crossings. The
//! `reflow.rs` seam was designed for this (its doc names "a future splitter
//! drags").

use crate::terminal::MAX_PANES;
use pinion_core::Scene;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::ContainerNode;
use pinion_core::style::{Size, SizeValue};
use pinion_core::theme::Theme;
use pinion_widget_paint::splitter::{SplitterOrientation, SplitterStyle, view_splitter};
use std::rc::Rc;

/// The number of dividers for N panes is N-1, so the handle table is one shorter
/// than the pane table.
const SPLITTER_COUNT: usize = MAX_PANES - 1;

/// The splitter-handle tags (`sprag_gui.split.<j>`), one per possible divider
/// (docked position `j`). Static — the `&'static str` discipline of `PANE_TAGS`;
/// the `SplitterExternal` registered at each tag ([`create_extra_externals`](crate::TerminalViewer))
/// shares the ratio Signal [`use_splitter_ratio`]`(j)` the fold reads.
pub(crate) const SPLITTER_TAGS: [&str; SPLITTER_COUNT] = [
    "sprag_gui.split.0",
    "sprag_gui.split.1",
    "sprag_gui.split.2",
    "sprag_gui.split.3",
    "sprag_gui.split.4",
    "sprag_gui.split.5",
    "sprag_gui.split.6",
];

/// The handle tag of divider `j` (`j < `[`SPLITTER_COUNT`]).
pub(crate) fn splitter_tag(j: usize) -> &'static str {
    SPLITTER_TAGS[j]
}

/// The default divider ratio — even (left share `0.5`), matching the former even
/// tiling so the boot layout is unchanged.
const SPLIT_RATIO_DEFAULT: f32 = 0.5;

/// `Owner::cache` key for divider `j`'s ratio.
fn ratio_key(j: usize) -> String {
    format!("sprag_gui.split.ratio.{j}")
}

/// Divider `j`'s ratio `Signal` (left share, `[0, 1]`), an `Owner::cache`-backed
/// `Rc<Signal<f32>>` SHARED between the read side ([`view_split_row`] ->
/// `view_splitter`) and the write side (`SplitterExternal::attach_ratio`) — both
/// resolve the same root-owner slot, so a drag (`set`) re-weights the painted
/// panes. Per-divider (keyed by position).
pub(crate) fn use_splitter_ratio(j: usize) -> Rc<Signal<f32>> {
    Owner::current()
        .expect("use_splitter_ratio() requires an active Owner scope")
        .cache(ratio_key(j), || Signal::new(SPLIT_RATIO_DEFAULT))
}

/// Arrange the docked `panes` left-to-right with draggable dividers, filling the
/// window. `0` panes -> an empty root; `1` -> the lone pane; `N` -> `N-1` nested
/// `view_splitter`s ([`split_fold`]). The result is sized `Percent(100)` so
/// compose's surface root fills it to the viewport (the splitter/pane sets no
/// size of its own; this gives its flex layout a definite extent).
pub(crate) fn view_split_row(panes: Vec<Scene>, theme: &Theme) -> Scene {
    let content = match panes.len() {
        0 => Scene::Container(ContainerNode::new(Vec::new())),
        1 => panes.into_iter().next().expect("len == 1"),
        _ => split_fold(panes, theme),
    };
    fill(content)
}

/// Nest `view_splitter` right-to-left over `panes` (`>= 2`): the rightmost pane
/// seeds the accumulator; each step to its left wraps
/// `view_splitter(left, acc, ratio[j], …)`, so divider `j` (docked position)
/// governs tile `j` vs everything to its right.
fn split_fold(panes: Vec<Scene>, theme: &Theme) -> Scene {
    let mut iter = panes.into_iter().enumerate().rev();
    let (_, mut acc) = iter.next().expect("split_fold needs >= 2 panes");
    for (j, left) in iter {
        acc = view_splitter(
            left,
            acc,
            &use_splitter_ratio(j),
            theme,
            &SplitterStyle::m3_default(SplitterOrientation::Horizontal, splitter_tag(j)),
            false, // dragging: no visual handle-highlight (cosmetic; drag still works)
        );
    }
    acc
}

/// Size a Container `Percent(100)` on both axes so it fills compose's surface root
/// (itself a `Percent(100)` block). The `view_splitter` / lone-pane root sets no
/// size; this gives it the definite extent its flex layout needs (avoiding the
/// intrinsic-collapse the splitter's own R685 fix documents).
fn fill(scene: Scene) -> Scene {
    match scene {
        Scene::Container(c) => Scene::Container(c.map_layout(|l| {
            l.with_size(
                Size::auto()
                    .with_width(SizeValue::Percent(100))
                    .with_height(SizeValue::Percent(100)),
            )
        })),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;
    use pinion_core::theme::use_theme;

    fn stub_pane(tag: &'static str) -> Scene {
        Scene::Container(ContainerNode::new(Vec::new()).with_tag(tag))
    }

    #[test]
    fn two_panes_get_one_draggable_divider() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(vec![stub_pane("p0"), stub_pane("p1")], &theme)
        });
        assert!(
            scene.contains_tag("p0") && scene.contains_tag("p1"),
            "both panes present"
        );
        assert!(
            scene.contains_tag(splitter_tag(0)),
            "divider 0 handle present"
        );
        assert!(
            !scene.contains_tag(splitter_tag(1)),
            "only N-1 = 1 divider for 2 panes"
        );
    }

    #[test]
    fn three_panes_nest_two_dividers() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(
                vec![stub_pane("p0"), stub_pane("p1"), stub_pane("p2")],
                &theme,
            )
        });
        assert!(
            scene.contains_tag(splitter_tag(0)) && scene.contains_tag(splitter_tag(1)),
            "two dividers"
        );
        assert!(
            scene.contains_tag("p0") && scene.contains_tag("p1") && scene.contains_tag("p2"),
            "all three panes present",
        );
    }

    #[test]
    fn one_pane_has_no_divider() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme("app").theme_animated();
            view_split_row(vec![stub_pane("solo")], &theme)
        });
        assert!(scene.contains_tag("solo"));
        assert!(
            !scene.contains_tag(splitter_tag(0)),
            "a lone pane has no divider"
        );
    }

    #[test]
    fn ratio_signal_is_per_divider_and_defaults_even() {
        let owner = Owner::new();
        owner.run(|| {
            assert!((use_splitter_ratio(0).get() - SPLIT_RATIO_DEFAULT).abs() < f32::EPSILON);
            use_splitter_ratio(0).set(0.7);
            assert!(
                (use_splitter_ratio(0).get() - 0.7).abs() < f32::EPSILON,
                "drag re-weights"
            );
            assert!(
                (use_splitter_ratio(1).get() - SPLIT_RATIO_DEFAULT).abs() < f32::EPSILON,
                "divider 1 is independent",
            );
        });
    }
}
