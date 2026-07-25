//! The accessible-node projection: expose each tiled terminal pane as one
//! focusable text region for a screen reader / AccessKit tree (the human-AT path;
//! the AI path is `scene/snapshot`). See the crate-root module docs.

use crate::TerminalViewer;
use crate::terminal::{TerminalView, pane_tag, use_terminal};
use pinion_a11y::{AccessNode, AccessValue, AriaRole, WidgetA11y};

impl WidgetA11y for TerminalViewer {
    /// Expose each terminal pane as one accessible text region so a screen reader
    /// / AccessKit tree can read them (the AI path is `scene/snapshot`; this is the
    /// human-AT path). This is the **windowless / RPC** path — it advertises every
    /// pane with no window partition; the live multi-window path is
    /// [`access_nodes_for_window`] (via
    /// [`WidgetView::access_node_for_window`](crate::TerminalViewer)). Runs in the
    /// root Owner scope, so [`use_terminal`] resolves the live panes. Each node's
    /// tag is its [`pane_tag`], so AT focus and the keyboard focus gate
    /// ([`route_key`](crate::input::route_key)) share one identity per pane.
    fn access_node(_state: &crate::view::ViewState, focused: Option<&str>) -> Vec<AccessNode> {
        let terminal = use_terminal();
        terminal
            .slots
            .occupied_slots()
            .into_iter()
            .map(|i| pane_node(&terminal, i, focused))
            .collect()
    }
}

/// Per-window accessible nodes: the **main** window advertises the session SIDEBAR (the WAI-ARIA
/// `tablist` of sessions, R179 — [`crate::stabs::session_sidebar_access_nodes`]) THEN the DOCKED
/// panes (a floated pane is announced by its own undock window, not here); an **undock** window
/// (`pane-{i}`) advertises only pane i (no sidebar — it paints none). So a sibling window's AT tree
/// never carries ghost pane / sidebar nodes (the per-window-host discipline). Runs in the root Owner
/// scope (the shell calls it there). Called from
/// [`WidgetView::access_node_for_window`](crate::TerminalViewer).
pub(crate) fn access_nodes_for_window(window_id: &str, focused: Option<&str>) -> Vec<AccessNode> {
    let terminal = use_terminal();
    // The DOCKED pane nodes — the set the main window tiles, and the defensive fallback for a stale
    // undock id. Read the docked set from the dock split-tree ([`crate::split::docked_pane_indices`])
    // — the SAME authority `view_main` paints from — so a11y and the paint can never announce/show a
    // different set (pre-R61 this read the windows signal's float state, a second source).
    let docked_panes = || {
        crate::split::docked_pane_indices()
            .into_iter()
            .filter(|&i| terminal.slots.is_pane_occupied(i))
            .map(|i| pane_node(&terminal, i, focused))
            .collect::<Vec<AccessNode>>()
    };
    match crate::dock::pane_window_index(window_id) {
        // An undock window: just its one pane (if still present).
        Some(i) if terminal.slots.is_pane_occupied(i) => vec![pane_node(&terminal, i, focused)],
        // A stale undock id (its pane is gone): the docked set defensively — but NO sidebar, this is
        // not the main window.
        Some(_) => docked_panes(),
        // The MAIN window: the session sidebar (main-window-only, since the rail paints only there)
        // FIRST, then the docked panes.
        None => {
            let mut nodes = crate::stabs::session_sidebar_access_nodes(&terminal.slots, focused);
            nodes.extend(docked_panes());
            nodes
        }
    }
}

/// Build pane `i`'s accessible node from its live screen — the per-pane node
/// shared by the windowless [`WidgetA11y::access_node`] and the per-window
/// [`access_nodes_for_window`], so both announce a pane identically.
fn pane_node(terminal: &TerminalView, i: usize, focused: Option<&str>) -> AccessNode {
    // `full_text` is the pane's text SSOT — the same string the RPC `full_text`
    // query and the plugin capture read, so the AT and the AI see one notion of
    // each screen (scrollback + visible). Read through the host client (no direct
    // session touch), like every other pane access.
    let text = terminal.slots.pane_full_text(i);
    terminal_a11y_node(
        pane_tag(i),
        &terminal.slots.pane_command_label(i),
        text,
        focused == Some(pane_tag(i)),
        crate::attention::pane_has_unseen_attention(&terminal.slots, i),
    )
}

/// Build one pane's accessible node: a neutral focusable region
/// ([`AriaRole::Group`]) tagged its [`pane_tag`] (so AT focus + the keyboard focus
/// gate share one identity), named with the pane's command, carrying the screen
/// text as its value, and the focus state from the focus manager. Pure (no Owner
/// / no live pane), so it is unit-testable; the bounds are left `None` for the
/// shell to resolve from the tag after layout.
///
/// `Group` (not `TextInput`): pinion has no `terminal` / `log` role, and a
/// textbox role advertises caret + place-click + set-value edit affordances that
/// this widget does not implement (input is raw keystrokes funneled to the PTY
/// by `apply_key`, not textbox editing). A neutral region carrying a read value
/// is the honest shape — it does not promise an edit contract the terminal
/// cannot honor. `Group` over `Generic` because a generic container drops the
/// accessible name, whereas a `Group` announces the named region ("Terminal:
/// bash"). The cell data the AI reads stays the `scene/snapshot` path; this node
/// is the human-AT label + text.
fn terminal_a11y_node(
    tag: &'static str,
    command_label: &str,
    text: String,
    focused: bool,
    attention: bool,
) -> AccessNode {
    // Announce an unseen attention notification as SPOKEN words in the name — never the "●"
    // display glyph, which a screen reader would read as "black circle". The AT thus hears
    // "Attention — Terminal: bash" for a pane that raised one this client has not viewed.
    let name = if attention {
        format!("Attention \u{2014} Terminal: {command_label}")
    } else {
        format!("Terminal: {command_label}")
    };
    AccessNode::new(tag, AriaRole::Group)
        .with_name(name)
        .with_value(AccessValue::Text(text))
        .with_focused(focused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    #[test]
    fn terminal_a11y_node_exposes_tag_role_name_value_and_focus() {
        let node = terminal_a11y_node(
            pane_tag(1),
            "bash",
            "line one\nline two".to_owned(),
            true,
            false,
        );
        assert_eq!(
            node.tag,
            pane_tag(1),
            "the node carries its pane identity tag"
        );
        assert!(matches!(node.role, AriaRole::Group));
        assert_eq!(node.name.as_deref(), Some("Terminal: bash"));
        match &node.value {
            Some(AccessValue::Text(text)) => assert_eq!(text, "line one\nline two"),
            other => panic!("expected a Text value, got {other:?}"),
        }
        assert!(node.state.focused, "focused state flows through");
        // Unfocused: the AT focus follows the focus manager.
        assert!(
            !terminal_a11y_node(pane_tag(0), "sh", String::new(), false, false)
                .state
                .focused
        );
    }

    /// An unseen attention notification is announced as SPOKEN words in the accessible name
    /// (never the "●" glyph, which a screen reader would read literally). REVERT-PROOF: the
    /// non-attention node keeps the plain name, so the prefix is not unconditional.
    #[test]
    fn an_unseen_attention_pane_announces_it_in_the_accessible_name() {
        let attention = terminal_a11y_node(pane_tag(1), "bash", String::new(), false, true);
        assert_eq!(
            attention.name.as_deref(),
            Some("Attention \u{2014} Terminal: bash"),
            "spoken words, not a glyph",
        );
        let calm = terminal_a11y_node(pane_tag(1), "bash", String::new(), false, false);
        assert_eq!(
            calm.name.as_deref(),
            Some("Terminal: bash"),
            "no attention ⇒ the plain name",
        );
    }

    /// `access_node` reads the live panes through `use_terminal` (spawns real
    /// shell PTYs) and returns one focus-gated terminal node PER pane, the focused
    /// pane's node reporting focused.
    #[test]
    fn access_node_reads_each_live_pane() {
        let owner = Owner::new();
        let nodes = owner.run(|| {
            TerminalViewer::access_node(&crate::view::ViewState::default(), Some(pane_tag(0)))
        });
        let count = owner.run(|| use_terminal().slots.occupied_slots().len());
        assert_eq!(nodes.len(), count, "one terminal node per pane");
        for (i, node) in nodes.iter().enumerate() {
            assert_eq!(node.tag, pane_tag(i), "node {i} carries pane {i}'s tag");
            assert!(matches!(node.role, AriaRole::Group));
            assert!(
                matches!(node.value, Some(AccessValue::Text(_))),
                "value is the screen text"
            );
        }
        assert!(
            nodes[0].state.focused,
            "the focused pane's node reports focused"
        );
        assert!(
            !nodes[1].state.focused,
            "an unfocused pane's node is not focused"
        );
    }

    /// `access_nodes_for_window` partitions by window: the main window advertises the session
    /// SIDEBAR (the R179 `tablist`) alongside the docked PANES (a floated pane drops out); an undock
    /// window advertises exactly its one pane, with NO sidebar — no cross-window ghost nodes.
    #[test]
    fn access_nodes_partition_by_window() {
        let owner = Owner::new();
        owner.run(|| {
            // Project the host's arrangement, as the pre-view reconcile does each frame —
            // "docked" IS that projection, so a test that skips it advertises nothing.
            crate::split::sync_layout(&use_terminal().slots);
            let n = use_terminal().slots.occupied_slots().len();
            // Count only the PANE nodes — the sidebar's tablist / tab / button nodes ride alongside
            // on the main window (R179), asserted separately.
            let main_pane_nodes = || {
                access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None)
                    .into_iter()
                    .filter(|node| crate::terminal::pane_index_of(&node.tag).is_some())
                    .collect::<Vec<_>>()
            };
            // Boot: all docked -> main advertises every pane.
            assert_eq!(main_pane_nodes().len(), n);
            // ...and the session sidebar rides alongside — a `tablist` — on the MAIN window only.
            assert!(
                access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None)
                    .iter()
                    .any(|node| node.role == AriaRole::TabList),
                "the main window advertises the session tablist",
            );

            // Undock pane 1: main drops it; the undock window advertises only it (no sidebar).
            crate::dock::toggle_pane_floating(1);
            assert_eq!(
                main_pane_nodes().len(),
                n - 1,
                "main drops the floated pane"
            );
            assert!(
                main_pane_nodes().iter().all(|node| node.tag != pane_tag(1)),
                "no floated-pane node in main"
            );
            let undock = access_nodes_for_window(&crate::dock::pane_window_id(1), None);
            assert_eq!(
                undock.len(),
                1,
                "the undock window advertises one pane, no rail"
            );
            assert_eq!(undock[0].tag, pane_tag(1), "exactly pane 1");
            assert!(
                undock.iter().all(|node| node.role != AriaRole::TabList),
                "an undock window carries no session tablist",
            );
        });
    }
}
