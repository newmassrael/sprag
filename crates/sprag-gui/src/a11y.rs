//! The accessible-node projection: expose each tiled terminal pane as one
//! focusable text region for a screen reader / AccessKit tree (the human-AT path;
//! the AI path is `scene/snapshot`). See the crate-root module docs.

use crate::TerminalViewer;
use crate::terminal::{TerminalView, pane_tag, use_terminal};
use pinion_a11y::{AccessNode, AccessValue, AriaRole, WidgetA11y};
use sprag_host::PaneAgent;
use sprag_terminal::PaneExit;

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
        // The MAIN window: the client's MODAL surfaces first (they paint only here, and they paint
        // OVER everything — see below), then the session sidebar (main-window-only, since the rail
        // paints only there), then the docked panes.
        None => {
            let mut nodes = modal_access_nodes(focused);
            nodes.extend(crate::stabs::session_sidebar_access_nodes(
                &terminal.slots,
                focused,
            ));
            nodes.extend(docked_panes());
            nodes
        }
    }
}

/// The client's MODAL surfaces, in painting order — the destructive-command prompt
/// ([`crate::confirm`]) over the command palette ([`crate::palette`]) — or nothing when neither is up.
///
/// FIRST in the window's node list because they are last in the paint: each declares
/// [`with_modal`](AccessNode::with_modal), which lowers to AccessKit's modal flag and confines an AT's
/// virtual cursor to the dialog's own subtree. That flag is the ONE mechanism used for this. The
/// alternative — suppressing the sidebar and pane nodes while a modal is up — would be a second
/// authority over the same question, and the two would eventually disagree; the panes stay in the tree
/// and the modal flag is what keeps a screen reader out of them, exactly as the visual scrim keeps a
/// pointer out.
///
/// Both are advertised on the MAIN window only, like the session rail, because that is the only window
/// they paint on (`view::view_for_window`'s main arm mounts them). An undock window's tree is therefore
/// untouched by a modal opened over the tiling — which is honest: a floated pane keeps taking input
/// while the palette is up, since the palette's scrim covers only its own window.
///
/// Ordered prompt-then-palette to match the layering, though the two are never up together: activating
/// a destructive row CLOSES the palette before arming the prompt.
fn modal_access_nodes(focused: Option<&str>) -> Vec<AccessNode> {
    let mut nodes = crate::confirm::confirm_access_nodes(focused);
    nodes.extend(crate::palette::palette_access_nodes(focused));
    nodes.extend(crate::prompt::prompt_access_nodes(focused));
    nodes.extend(crate::keyhelp::keyhelp_access_nodes(
        focused,
        (crate::WINDOW_W, crate::WINDOW_H),
    ));
    nodes
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
        terminal
            .slots
            .pane_is_dead(i)
            .then(|| terminal.slots.pane_child_exit(i)),
        terminal.slots.pane_agent(i),
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
    // `Option<Option<_>>` because both layers are real and neither implies the other: the OUTER is
    // "this child is gone", the INNER is "and here is how" — a fact that lands later and may not
    // land at all. Flattening them would make a dead-but-unreaped pane announce as live, which is
    // precisely the thing the marker exists to prevent.
    dead: Option<Option<PaneExit>>,
    // What the AGENT in the pane is doing (H3), `None` for a pane no manifest claims. Announced
    // through `crate::view::agent_marker` — the same string the sighted title carries, for the
    // reason the exit marker is shared: a sighted user and a screen-reader user must not come to
    // different conclusions about which pane is waiting for them.
    agent: Option<PaneAgent>,
) -> AccessNode {
    // Announce an unseen attention notification as SPOKEN words in the name — never the "●"
    // display glyph, which a screen reader would read as "black circle". The AT thus hears
    // "Attention — Terminal: bash" for a pane that raised one this client has not viewed.
    let mut name = if attention {
        format!("Attention \u{2014} Terminal: {command_label}")
    } else {
        format!("Terminal: {command_label}")
    };
    // ...and the exited state on the END of the name, through the same
    // [`crate::view::dead_marker`] the sighted title renders — one function, so the AT and the
    // screen cannot come to describe one pane differently, and the exit code reaches both surfaces
    // by construction rather than by being remembered twice.
    //
    // In the NAME rather than as a description, for the reason the confirmation prompt's consequence
    // is: a description is announced at the AT's discretion, and "nothing is running here" is the
    // fact that decides whether typing into this pane will do anything at all. The CODE belongs
    // there for a sharper version of the same reason — a screen reader user cannot glance at the
    // output to guess whether the command worked.
    // The agent's state, in the same position and through the same function as the sighted title's —
    // between the pane's own name and the exit marker (and suppressed by a dead child, which is
    // `agent_marker`'s own rule rather than a second copy of it here).
    name.push_str(&crate::view::agent_marker(agent.as_ref(), dead.is_some()));
    if let Some(exit) = dead {
        name.push_str(&crate::view::dead_marker(exit.as_ref()));
    }
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
            None,
            None,
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
            !terminal_a11y_node(pane_tag(0), "sh", String::new(), false, false, None, None)
                .state
                .focused
        );
    }

    /// An unseen attention notification is announced as SPOKEN words in the accessible name
    /// (never the "●" glyph, which a screen reader would read literally). REVERT-PROOF: the
    /// non-attention node keeps the plain name, so the prefix is not unconditional.
    #[test]
    fn an_unseen_attention_pane_announces_it_in_the_accessible_name() {
        let attention =
            terminal_a11y_node(pane_tag(1), "bash", String::new(), false, true, None, None);
        assert_eq!(
            attention.name.as_deref(),
            Some("Attention \u{2014} Terminal: bash"),
            "spoken words, not a glyph",
        );
        let calm = terminal_a11y_node(pane_tag(1), "bash", String::new(), false, false, None, None);
        assert_eq!(
            calm.name.as_deref(),
            Some("Terminal: bash"),
            "no attention ⇒ the plain name",
        );
    }

    /// A pane whose child has EXITED says so in its accessible name, on the END — where the sighted
    /// title carries it — so an AT user learns that typing here will reach nothing.
    ///
    /// REVERT-PROOF: drop the `dead` push and the live and exited names become identical, which is
    /// exactly the ambiguity the marker exists to remove.
    #[test]
    fn an_exited_pane_announces_it_at_the_end_of_the_accessible_name() {
        let exited = terminal_a11y_node(
            pane_tag(1),
            "cargo",
            String::new(),
            false,
            false,
            Some(None),
            None,
        );
        assert_eq!(
            exited.name.as_deref(),
            Some("Terminal: cargo (exited)"),
            "the marker trails the name it qualifies",
        );
        let live = terminal_a11y_node(
            pane_tag(1),
            "cargo",
            String::new(),
            false,
            false,
            None,
            None,
        );
        assert_eq!(
            live.name.as_deref(),
            Some("Terminal: cargo"),
            "a live pane says nothing extra",
        );
        // The two markers COMPOSE, each at its own end — the attention prefix is a transient flag,
        // the exited suffix a permanent statement, so neither displaces the other.
        let both = terminal_a11y_node(
            pane_tag(1),
            "cargo",
            String::new(),
            false,
            true,
            Some(None),
            None,
        );
        assert_eq!(
            both.name.as_deref(),
            Some("Attention \u{2014} Terminal: cargo (exited)"),
        );
    }

    /// The exit CODE reaches the spoken name too, through the same renderer the title uses.
    ///
    /// A screen-reader user has the sharpest version of the problem this closes: they cannot glance
    /// at the output to guess whether the command worked, so "(exited)" alone leaves them with no
    /// way at all to tell a passing `cargo test` from a failing one.
    ///
    /// REVERT-PROOF: push the bare `DEAD_MARKER` instead of the rendered one and both the code and
    /// the signal disappear from the name while the sighted title keeps them — exactly the drift
    /// routing both surfaces through one function prevents.
    #[test]
    fn the_spoken_name_carries_the_exit_code_and_the_signal() {
        let failed = terminal_a11y_node(
            pane_tag(1),
            "cargo",
            String::new(),
            false,
            false,
            Some(Some(PaneExit {
                code: 101,
                signal: None,
            })),
            None,
        );
        assert_eq!(
            failed.name.as_deref(),
            Some("Terminal: cargo (exited 101)"),
            "the AT hears the code, not just that something ended",
        );

        let killed = terminal_a11y_node(
            pane_tag(1),
            "cargo",
            String::new(),
            false,
            false,
            Some(Some(PaneExit {
                code: 1,
                signal: Some("Terminated".to_owned()),
            })),
            None,
        );
        assert_eq!(
            killed.name.as_deref(),
            Some("Terminal: cargo (killed: Terminated)"),
        );
    }

    /// The AGENT's state is SPOKEN, in the same position the sighted title carries it and through the
    /// same function — so a screen-reader user learns which pane is waiting for an answer at the same
    /// moment a sighted user can see it.
    ///
    /// This is the surface with the sharpest version of the problem H3 exists for: a sighted user can
    /// glance at six panes and find the one showing a prompt, and an AT user cannot. The marker is
    /// words for exactly that reason — a glyph would be read out as its unicode name.
    ///
    /// REVERT-PROOF: drop the `agent_marker` push and the working pane and the blocked pane announce
    /// identically; render the wire token instead of the phrase and the AT hears "blocked", which
    /// reads as a fault rather than as a question waiting for an answer.
    #[test]
    fn the_spoken_name_carries_what_the_agent_is_doing() {
        let agent = |state: &str| {
            Some(PaneAgent {
                state: state.to_owned(),
                name: Some("claude".to_owned()),
                rule: Some("trust-dialog".to_owned()),
                seq: 1,
            })
        };
        let blocked = terminal_a11y_node(
            pane_tag(1),
            "claude",
            String::new(),
            false,
            false,
            None,
            agent("blocked"),
        );
        assert_eq!(
            blocked.name.as_deref(),
            Some("Terminal: claude (claude needs an answer)"),
            "the AT hears the request, not the wire's token",
        );
        let working = terminal_a11y_node(
            pane_tag(1),
            "claude",
            String::new(),
            false,
            false,
            None,
            agent("working"),
        );
        assert_eq!(
            working.name.as_deref(),
            Some("Terminal: claude (claude working)"),
            "...and the two states are distinguishable by ear",
        );
        // A pane no manifest claims announces exactly what it did before H3 — the additive rule,
        // which on this surface is what keeps a shell from being introduced as an agent at rest.
        let shell =
            terminal_a11y_node(pane_tag(1), "bash", String::new(), false, false, None, None);
        assert_eq!(shell.name.as_deref(), Some("Terminal: bash"));
        // ...and the verdict yields to the exit marker, which is `agent_marker`'s own rule: the
        // state and "(exited)" would otherwise contradict each other in one breath.
        let gone = terminal_a11y_node(
            pane_tag(1),
            "claude",
            String::new(),
            false,
            false,
            Some(None),
            agent("idle"),
        );
        assert_eq!(gone.name.as_deref(), Some("Terminal: claude (exited)"));
    }

    /// The sighted title and the spoken name use ONE marker, so the two surfaces cannot drift into
    /// describing the same pane differently.
    #[test]
    fn the_spoken_marker_is_the_same_string_the_title_paints() {
        let spoken = terminal_a11y_node(
            pane_tag(0),
            "sh",
            String::new(),
            false,
            false,
            Some(None),
            None,
        );
        assert!(
            spoken
                .name
                .as_deref()
                .is_some_and(|name| name.ends_with(crate::view::DEAD_MARKER)),
            "the accessible name ends with the very constant the title appends"
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

    /// A client modal rides the MAIN window's tree — FIRST, and flagged `aria-modal` — and is
    /// advertised on no other window, because main is the only window it paints on. The panes stay in
    /// the tree beneath it: the modal flag is what keeps an AT out of them, not their absence.
    ///
    /// REVERT-PROOF: move `modal_access_nodes` after the sidebar and the leading assertion fails; drop
    /// the call and both the dialog assertions do.
    #[test]
    fn a_modal_leads_the_main_windows_tree_and_appears_on_no_other_window() {
        let owner = Owner::new();
        owner.run(|| {
            crate::split::sync_layout(&use_terminal().slots);
            assert!(
                access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None)
                    .iter()
                    .all(|node| node.role != AriaRole::Dialog),
                "with nothing open there is no dialog to announce"
            );

            crate::palette::open(Some(0));
            let main = access_nodes_for_window(crate::dock::MAIN_WINDOW_ID, None);
            assert_eq!(
                main.first().map(|node| node.role),
                Some(AriaRole::Dialog),
                "the modal LEADS the tree, as it leads the paint"
            );
            assert!(main[0].modal, "and declares itself a modal boundary");
            assert!(
                main.iter()
                    .any(|node| crate::terminal::pane_index_of(&node.tag).is_some()),
                "the panes remain in the tree — the modal flag confines the AT, not their removal"
            );

            crate::dock::toggle_pane_floating(1);
            assert!(
                access_nodes_for_window(&crate::dock::pane_window_id(1), None)
                    .iter()
                    .all(|node| node.role != AriaRole::Dialog),
                "a modal painted on main is not advertised on a tear-off window"
            );
            crate::palette::close();
        });
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
