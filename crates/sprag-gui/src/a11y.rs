//! The accessible-node projection: expose the live terminal as one focusable
//! text region for a screen reader / AccessKit tree (the human-AT path; the AI
//! path is `scene/snapshot`). See the crate-root module docs.

use crate::terminal::use_terminal;
use crate::{TerminalViewer, ROOT_TAG};
use pinion_a11y::{AccessNode, AccessValue, AriaRole, WidgetA11y};

impl WidgetA11y for TerminalViewer {
    /// Expose the live terminal as one accessible text region so a screen
    /// reader / AccessKit tree can read it (the AI path is `scene/snapshot`;
    /// this is the human-AT path). It runs in the root Owner scope, so
    /// [`use_terminal`] resolves the live pane here. The node tag is
    /// [`ROOT_TAG`], so AT focus and the keyboard focus gate ([`route_key`](crate::input::route_key))
    /// share one identity.
    fn access_node(_state: &(), focused: Option<&str>) -> Vec<AccessNode> {
        let terminal = use_terminal();
        let pane = terminal.boot_pane();
        // `full_text` is the pane's text SSOT — the same string the RPC
        // `full_text` query and the plugin capture read, so the AT and the AI
        // see one notion of the screen (scrollback + visible).
        let text = pane.session().with_screen(|screen| screen.full_text());
        vec![terminal_a11y_node(
            pane.command_label(),
            text,
            focused == Some(ROOT_TAG),
        )]
    }
}

/// Build the terminal's accessible node: a neutral focusable region
/// ([`AriaRole::Group`]) named with the pane's command, carrying the screen text
/// as its value, and the focus state from the focus manager. Pure (no Owner / no
/// live pane), so it is unit-testable; the bounds are left `None` for the shell
/// to resolve from [`ROOT_TAG`] after layout.
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
fn terminal_a11y_node(command_label: &str, text: String, focused: bool) -> AccessNode {
    AccessNode::new(ROOT_TAG, AriaRole::Group)
        .with_name(format!("Terminal: {command_label}"))
        .with_value(AccessValue::Text(text))
        .with_focused(focused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    #[test]
    fn terminal_a11y_node_exposes_role_name_value_and_focus() {
        let node = terminal_a11y_node("bash", "line one\nline two".to_owned(), true);
        assert_eq!(node.tag, ROOT_TAG);
        assert!(matches!(node.role, AriaRole::Group));
        assert_eq!(node.name.as_deref(), Some("Terminal: bash"));
        match &node.value {
            Some(AccessValue::Text(text)) => assert_eq!(text, "line one\nline two"),
            other => panic!("expected a Text value, got {other:?}"),
        }
        assert!(node.state.focused, "focused state flows through");
        // Unfocused: the AT focus follows the focus manager.
        assert!(!terminal_a11y_node("sh", String::new(), false).state.focused);
    }

    /// `access_node` reads the live pane through `use_terminal` (spawns a real
    /// shell PTY) and returns one focus-gated terminal node — the use_terminal
    /// -> screen -> node plumbing, headlessly.
    #[test]
    fn access_node_reads_the_live_pane() {
        let owner = Owner::new();
        let nodes = owner.run(|| TerminalViewer::access_node(&(), Some(ROOT_TAG)));
        assert_eq!(nodes.len(), 1, "one terminal node");
        let node = &nodes[0];
        assert_eq!(node.tag, ROOT_TAG);
        assert!(matches!(node.role, AriaRole::Group));
        assert!(node.state.focused, "ROOT_TAG focus -> focused node");
        assert!(matches!(node.value, Some(AccessValue::Text(_))), "value is the screen text");
        // Same cached pane, unfocused dispatch -> the focus flag clears.
        let unfocused = owner.run(|| TerminalViewer::access_node(&(), None));
        assert!(!unfocused[0].state.focused);
    }
}
