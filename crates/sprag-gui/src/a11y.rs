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
    fn access_node(_state: &crate::ctxmenu::MenuState, focused: Option<&str>) -> Vec<AccessNode> {
        let terminal = use_terminal();
        terminal
            .slots
            .occupied_slots()
            .into_iter()
            .map(|i| pane_node(&terminal, i, focused))
            .collect()
    }
}

/// Per-window accessible nodes: the **main** window advertises the DOCKED panes (a
/// floated pane is announced by its own undock window, not here); an **undock**
/// window (`pane-{i}`) advertises only pane i. So a sibling window's AT tree never
/// carries ghost pane nodes (the per-window-host discipline). Runs in the root
/// Owner scope (the shell calls it there). Called from
/// [`WidgetView::access_node_for_window`](crate::TerminalViewer).
pub(crate) fn access_nodes_for_window(window_id: &str, focused: Option<&str>) -> Vec<AccessNode> {
    let terminal = use_terminal();
    match crate::dock::pane_window_index(window_id) {
        // An undock window: just its one pane (if still present).
        Some(i) if terminal.slots.is_pane_occupied(i) => vec![pane_node(&terminal, i, focused)],
        // The main window (or any non-pane id): the docked panes only. Read the
        // docked set from the dock split-tree ([`crate::split::docked_pane_indices`])
        // — the SAME authority `view_main` paints from — so a11y and the paint can
        // never announce/show a different set (pre-R61 this read the windows signal's
        // float state, a second source for the same fact).
        _ => crate::split::docked_pane_indices()
            .into_iter()
            .filter(|&i| terminal.slots.is_pane_occupied(i))
            .map(|i| pane_node(&terminal, i, focused))
            .collect(),
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
) -> AccessNode {
    AccessNode::new(tag, AriaRole::Group)
        .with_name(format!("Terminal: {command_label}"))
        .with_value(AccessValue::Text(text))
        .with_focused(focused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    #[test]
    fn terminal_a11y_node_exposes_tag_role_name_value_and_focus() {
        let node = terminal_a11y_node(pane_tag(1), "bash", "line one\nline two".to_owned(), true);
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
            !terminal_a11y_node(pane_tag(0), "sh", String::new(), false)
                .state
                .focused
        );
    }

    /// `access_node` reads the live panes through `use_terminal` (spawns real
    /// shell PTYs) and returns one focus-gated terminal node PER pane, the focused
    /// pane's node reporting focused.
    #[test]
    fn access_node_reads_each_live_pane() {
        let owner = Owner::new();
        let nodes = owner.run(|| {
            TerminalViewer::access_node(&crate::ctxmenu::MenuState::default(), Some(pane_tag(0)))
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

    /// `access_nodes_for_window` partitions by window: the main window advertises
    /// only the docked panes (a floated pane drops out); an undock window
    /// advertises exactly its one pane — no cross-window ghost nodes.
    #[test]
    fn access_nodes_partition_by_window() {
        let owner = Owner::new();
        owner.run(|| {
            // Project the host's arrangement, as the pre-view reconcile does each frame —
            // "docked" IS that projection, so a test that skips it advertises nothing.
            crate::split::sync_layout(&use_terminal().slots);
            let n = use_terminal().slots.occupied_slots().len();
            // Boot: all docked -> main advertises every pane.
            assert_eq!(
                access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None).len(),
                n
            );

            // Undock pane 1: main drops it; the undock window advertises only it.
            crate::dock::toggle_pane_floating(1);
            let main = access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None);
            assert_eq!(main.len(), n - 1, "main drops the floated pane");
            assert!(
                main.iter().all(|node| node.tag != pane_tag(1)),
                "no floated-pane node in main"
            );
            let undock = access_nodes_for_window(&crate::dock::pane_window_id(1), None);
            assert_eq!(undock.len(), 1, "the undock window advertises one pane");
            assert_eq!(undock[0].tag, pane_tag(1), "exactly pane 1");
        });
    }
}
