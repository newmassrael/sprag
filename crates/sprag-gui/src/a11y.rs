//! The accessible-node projection: expose each tiled terminal pane as one
//! focusable text region for a screen reader / AccessKit tree (the human-AT path;
//! the AI path is `scene/snapshot`). See the crate-root module docs.

use crate::TerminalViewer;
use crate::terminal::{pane_tag, use_terminal};
use pinion_a11y::{AccessNode, AccessValue, AriaRole, WidgetA11y};

impl WidgetA11y for TerminalViewer {
    /// Expose each tiled terminal pane as one accessible text region so a screen
    /// reader / AccessKit tree can read them (the AI path is `scene/snapshot`;
    /// this is the human-AT path). It runs in the root Owner scope, so
    /// [`use_terminal`] resolves the live panes here. Each node's tag is its
    /// [`pane_tag`], so AT focus and the keyboard focus gate
    /// ([`route_key`](crate::input::route_key)) share one identity per pane and
    /// the focused pane's node reports focused.
    fn access_node(_state: &(), focused: Option<&str>) -> Vec<AccessNode> {
        let terminal = use_terminal();
        (0..terminal.pane_count())
            .map(|i| {
                let pane = terminal.pane(i);
                // `full_text` is the pane's text SSOT — the same string the RPC
                // `full_text` query and the plugin capture read, so the AT and
                // the AI see one notion of each screen (scrollback + visible).
                let text = pane.session().with_screen(|screen| screen.full_text());
                terminal_a11y_node(
                    pane_tag(i),
                    pane.command_label(),
                    text,
                    focused == Some(pane_tag(i)),
                )
            })
            .collect()
    }
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
        let nodes = owner.run(|| TerminalViewer::access_node(&(), Some(pane_tag(0))));
        let count = owner.run(|| use_terminal().pane_count());
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
}
