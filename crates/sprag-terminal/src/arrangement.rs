//! The ONE drawing of an arrangement — a [`LayoutSnapshot`] as text, for every surface that shows
//! one.
//!
//! # Why this is a library function and not each surface's own
//!
//! The drawing states this crate's conventions, and a second writer of them is a second thing that
//! can come to disagree with the daemon about what an arrangement means:
//!
//! * a divider reads `RATIO SIDE|SIDE`, where the ratio is the **first** side's share and the two
//!   branches below are in that order — [`SplitDir`]'s own first/second convention, made legible;
//! * the pane filling the window is marked in the words `zoom-pane` prints when it sets one, so the
//!   verb and the reading cannot drift into two vocabularies;
//! * a floated pane is listed rather than drawn, because [`LayoutSnapshot::floating`] is exactly the
//!   set with no leaf in the tree.
//!
//! If any of those changed, this drawing would have to change in the same commit — which is the
//! whole reason it lives beside them rather than in whichever binary drew one first.
//!
//! # The label, and why it is a parameter rather than a constant
//!
//! Surfaces name a pane differently and always have. The `sprag` CLI speaks the host id an operator
//! passes to `select-pane`; the agent-facing MCP server speaks the 1-based number ITS tools take,
//! which is a different integer for the same pane. Rendering both as a bare `pane N` from one
//! function would print the same words for two different panes — so the caller supplies the naming
//! and the drawing supplies the shape.

use crate::layout::{LayoutNodeWire, LayoutSnapshot, SplitDir};
use crate::workspace::PaneId;

/// Draw `snapshot`'s arrangement, naming each pane with `label`.
///
/// Ends in a newline; never empty. What it draws, in order: the tiling as a tree, the floated panes
/// if any, and — only when the zoom names a pane the tree does not hold — that fact.
///
/// The snapshot's REVISION is deliberately not here. It frames the drawing rather than being part of
/// it, and the two surfaces frame it differently: the CLI heads its output with it, the MCP tool
/// puts it in a sentence. Rendering it here would force one of them to strip it back off.
///
/// ```text
/// 50% left|right
/// ├─ pane 1
/// └─ 60% top|bottom
///    ├─ pane 2  (fills the window)
///    └─ pane 3
/// floating: pane 4
/// ```
pub fn render(snapshot: &LayoutSnapshot, label: &dyn Fn(PaneId) -> String) -> String {
    let mut out = String::new();
    let mut zoom_shown = false;
    match snapshot.tree.root.as_ref() {
        Some(root) => render_node(
            root,
            snapshot.zoomed,
            label,
            &mut zoom_shown,
            "",
            "",
            &mut out,
        ),
        // A window every pane of which has been floated out still has an arrangement to report,
        // and reporting it as nothing at all would read as a failed call.
        None => out.push_str("no panes tiled\n"),
    }
    if !snapshot.floating.is_empty() {
        let names: Vec<String> = snapshot.floating.iter().map(|pane| label(*pane)).collect();
        // Comma-separated, not space-separated: a label is a PHRASE (the MCP surface's carries two
        // integers), and spaces alone would run two panes' names together.
        out.push_str(&format!("floating: {}\n", names.join(", ")));
    }
    // The daemon's own invariant (`Window::heal_zoom`) is that a zoom names a pane the window is ON
    // and TILES, so the marker above always lands on a leaf. This says so anyway when it did not,
    // because that invariant is held on the OTHER side of a socket whose whole reason for carrying
    // a wire-protocol number is that the process there can be a different build (R280). A reader
    // that assumed the writer would drop, in silence, the fact that one pane covers all of this.
    if let Some(pane) = snapshot.zoomed.filter(|_| !zoom_shown) {
        out.push_str(&format!(
            "zoomed: {} (not in this arrangement)\n",
            label(pane)
        ));
    }
    out
}

/// One node of [`render`]'s drawing: `head` prefixes this node's own line and `tail` prefixes its
/// descendants', which is what keeps a deep arrangement's guides connected.
fn render_node(
    node: &LayoutNodeWire,
    zoomed: Option<PaneId>,
    label: &dyn Fn(PaneId) -> String,
    zoom_shown: &mut bool,
    head: &str,
    tail: &str,
    out: &mut String,
) {
    match node {
        LayoutNodeWire::Leaf(pane) => {
            let mark = if zoomed == Some(*pane) {
                *zoom_shown = true;
                "  (fills the window)"
            } else {
                ""
            };
            out.push_str(&format!("{head}{}{mark}\n", label(*pane)));
        }
        LayoutNodeWire::Split {
            dir,
            ratio,
            first,
            second,
            ..
        } => {
            // `Horizontal` lays `first` LEFT and `second` RIGHT; `Vertical` lays it TOP and BOTTOM
            // (`SplitDir`'s own convention) — named here in the reading order the branches below
            // are drawn in, so the two cannot be read the wrong way round.
            let sides = match dir {
                SplitDir::Horizontal => "left|right",
                SplitDir::Vertical => "top|bottom",
            };
            out.push_str(&format!("{head}{:.0}% {sides}\n", ratio * 100.0));
            render_node(
                first,
                zoomed,
                label,
                zoom_shown,
                &format!("{tail}├─ "),
                &format!("{tail}│  "),
                out,
            );
            render_node(
                second,
                zoomed,
                label,
                zoom_shown,
                &format!("{tail}└─ "),
                &format!("{tail}   "),
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutWire;

    /// A leaf, spelled once so the arrangement literals below read as the shapes they are.
    fn leaf(pane: u64) -> LayoutNodeWire {
        LayoutNodeWire::Leaf(PaneId(pane))
    }

    /// A division at `ratio` — `id: None`, because the drawing never reads a divider's identity and
    /// a test that supplied one would suggest it did.
    fn split(
        dir: SplitDir,
        ratio: f32,
        first: LayoutNodeWire,
        second: LayoutNodeWire,
    ) -> LayoutNodeWire {
        LayoutNodeWire::Split {
            id: None,
            dir,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn snapshot(root: Option<LayoutNodeWire>) -> LayoutSnapshot {
        LayoutSnapshot {
            revision: 12,
            tree: LayoutWire { root },
            floating: Vec::new(),
            zoomed: None,
        }
    }

    /// The naming the `sprag` CLI passes: a pane by the host id an operator hands to `select-pane`.
    fn by_id(pane: PaneId) -> String {
        format!("pane {pane}")
    }

    /// The whole drawing, pinned as one string rather than probed for fragments — the arrangement
    /// is what this exists to show, so a test that only asked whether the ids appeared would pass on
    /// a rendering that put them in the wrong places.
    ///
    /// Nesting on the SECOND child is the case worth pinning: it is the only one where the parent's
    /// continuation prefix matters, and getting it wrong (indenting the grandchildren under the
    /// first branch's guide) draws a tree that is connected but false.
    #[test]
    fn an_arrangement_draws_as_a_tree_whose_dividers_name_their_sides() {
        let tree = split(
            SplitDir::Horizontal,
            0.5,
            leaf(1),
            split(SplitDir::Vertical, 0.6, leaf(2), leaf(3)),
        );
        assert_eq!(
            render(&snapshot(Some(tree)), &by_id),
            "50% left|right\n\
             ├─ pane 1\n\
             └─ 60% top|bottom\n\
             \x20  ├─ pane 2\n\
             \x20  └─ pane 3\n",
        );
    }

    /// The first child's subtree gets the CONNECTED guide (`│`) and the second's gets blank space,
    /// which is the other half of the prefix rule and cannot be seen in a right-nested tree.
    #[test]
    fn a_nested_first_child_keeps_its_parents_guide() {
        let tree = split(
            SplitDir::Vertical,
            0.25,
            split(SplitDir::Horizontal, 0.75, leaf(4), leaf(5)),
            leaf(6),
        );
        assert_eq!(
            render(&snapshot(Some(tree)), &by_id),
            "25% top|bottom\n\
             ├─ 75% left|right\n\
             │  ├─ pane 4\n\
             │  └─ pane 5\n\
             └─ pane 6\n",
        );
    }

    /// A one-pane window roots at a LEAF, so it draws with no guides at all — and a window whose
    /// panes have all been floated out still reports, rather than answering with silence.
    #[test]
    fn the_degenerate_arrangements_still_say_what_they_are() {
        assert_eq!(render(&snapshot(Some(leaf(0))), &by_id), "pane 0\n");
        let mut emptied = snapshot(None);
        emptied.floating = vec![PaneId(2), PaneId(7)];
        assert_eq!(
            render(&emptied, &by_id),
            "no panes tiled\nfloating: pane 2, pane 7\n",
        );
    }

    /// The zoom is marked on the pane it names, in the words `zoom-pane` itself prints — and the
    /// float list stays ABSENT when nothing floats rather than printing an empty one.
    #[test]
    fn the_zoomed_pane_is_marked_where_it_sits() {
        let mut zoomed = snapshot(Some(split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2))));
        zoomed.zoomed = Some(PaneId(2));
        assert_eq!(
            render(&zoomed, &by_id),
            "50% left|right\n\
             ├─ pane 1\n\
             └─ pane 2  (fills the window)\n",
        );
    }

    /// A zoom naming a pane this arrangement does not tile is REPORTED, not swallowed.
    ///
    /// The daemon's invariant forbids it, so this can only arrive from a daemon of another build —
    /// which is the case the wire-protocol number exists for (R280). The failure it prevents is the
    /// silent one: an operator reading a tidy three-pane drawing while one pane covers all of it.
    #[test]
    fn a_zoom_the_tree_does_not_hold_is_reported_rather_than_dropped() {
        let mut skewed = snapshot(Some(split(SplitDir::Vertical, 0.5, leaf(1), leaf(2))));
        skewed.zoomed = Some(PaneId(8));
        assert_eq!(
            render(&skewed, &by_id),
            "50% top|bottom\n\
             ├─ pane 1\n\
             └─ pane 2\n\
             zoomed: pane 8 (not in this arrangement)\n",
        );
    }

    /// **The label reaches every place a pane is named**, which is what makes one drawing serveable
    /// to two surfaces that number panes differently.
    ///
    /// A label applied to the leaves but not to the float list or the unheld-zoom arm would leave a
    /// caller's own vocabulary silently mixed with the host's ids in one answer — the failure this
    /// parameter exists to prevent, and one no single-surface test can see.
    #[test]
    fn every_pane_in_the_drawing_is_named_by_the_caller() {
        let mut all = snapshot(Some(split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2))));
        all.floating = vec![PaneId(3)];
        all.zoomed = Some(PaneId(9));
        assert_eq!(
            render(&all, &|pane| format!("<{pane}>")),
            "50% left|right\n\
             ├─ <1>\n\
             └─ <2>\n\
             floating: <3>\n\
             zoomed: <9> (not in this arrangement)\n",
            "no `pane N` survives a caller that names panes another way",
        );
    }
}
