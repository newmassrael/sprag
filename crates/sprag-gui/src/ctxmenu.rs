//! Right-click context menu (R140): `Copy` / `Paste` / `Select all`, plus the pane-migration
//! gestures `Break out` (tmux `break-pane`) and `Move to <window>` (tmux `join-pane`).
//!
//! Built entirely on pinion's context-menu substrate (NO pinion change), the
//! composite pattern of `hello-grid-header-menu`:
//!
//! * a [`ContextMenuExternal`] is registered as one of the GUI's EXTRA externals
//!   ([`create_menu_external`], tag [`CTXMENU_TAG`]) — its open / active state is
//!   preserved across the dynamic-external reconcile by tag (pinion R689), like the
//!   splitters and dock panels;
//! * a secondary-button press ([`WidgetCore::apply_secondary_click`](crate::TerminalViewer))
//!   forwards the window-space point to [`open_at`], anchoring the popup (the universal
//!   AI peer is `scene/click {button:"right"}`, reaching the same override);
//! * the pure `view` reads the menu's open / active projection into the binding
//!   [`State`](MenuState) via [`read_menu_state`] (the `read_state` seam) and, when open,
//!   overlays [`view_context_menu`] + a click-outside [`dismiss_barrier`] on the main
//!   window ([`overlay`]);
//! * activating an item emits a `"command"` intent that the binding reducer routes to
//!   [`handle_command`] -> the matching action.
//!
//! ## Why the item list is CAPTURED at open time
//!
//! The `Move to <window>` items are one per OTHER window, so the menu's contents depend on the
//! live window list — which can change out from under an open popup (a second client, an agent).
//! Like the target pane, the whole action list is SNAPSHOT when the menu opens ([`menu_actions`]),
//! so [`overlay`]'s painted labels and [`handle_command`]'s index-to-action resolution read the
//! SAME list and cannot disagree: a click always runs the action the user saw, never a neighbour a
//! mid-open reflow shifted into that row. This is the wtabs "resolve the click against the list it
//! was painted from" rule, taken one step further by freezing the list for the popup's lifetime.
//!
//! Keyboard navigation (Arrow / Enter / Escape) and a11y are deferred (mouse-first).

use crate::terminal::{pane_index_of, use_terminal};
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::external::IntrospectValue;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::theme::Theme;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::context_menu::{ContextMenuExternal, read_open_state};
use pinion_core::{Intent, Scene};
use pinion_widget_paint::barrier::dismiss_barrier;
use pinion_widget_paint::menu::{ContextMenuPlacement, MenuStyle, view_context_menu};

/// One row of the pane context menu — a semantic action, so paint (its [`label`](MenuAction::label))
/// and the reducer (its effect in [`run_item`]) name the SAME thing rather than agreeing on a
/// stringly-typed item order.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum MenuAction {
    /// Copy the active selection (wherever it is).
    Copy,
    /// Paste into the target pane.
    Paste,
    /// Select the whole target pane.
    SelectAll,
    /// Break the target pane out into a new window (tmux `break-pane`).
    BreakOut,
    /// Move the target pane into the named window (tmux `join-pane`).
    JoinInto(String),
}

impl MenuAction {
    /// The row label painted for this action.
    fn label(&self) -> String {
        match self {
            Self::Copy => "Copy".to_owned(),
            Self::Paste => "Paste".to_owned(),
            Self::SelectAll => "Select all".to_owned(),
            Self::BreakOut => "Break out".to_owned(),
            Self::JoinInto(window) => format!("Move to {window}"),
        }
    }
}

/// The fixed leading actions, always present in order; the `Move to <window>` items follow.
const FIXED_ACTION_COUNT: usize = 4;

/// The cap on `Move to <window>` items — one per window past the current, matching the tab strip's
/// [`MAX_WINDOW_TABS`](crate::wtabs::MAX_WINDOW_TABS) practical ceiling. A session with more windows
/// than this offers the CLI's `join-pane` for the overflow (an honest bound, like the tab strip's).
const MAX_JOIN_TARGETS: usize = 16;

/// The [`ContextMenuExternal`] row capacity — the MOST rows the menu can ever paint (the fixed
/// actions plus the join-target cap). Registered ONCE at this count (pinion R689 preserves the live
/// external by tag across the reconcile, so a per-open count change would discard it); [`overlay`]
/// paints only the rows the live action list fills, exactly as the tab strip paints only its live
/// windows under a fixed button cap.
const MENU_CAPACITY: usize = FIXED_ACTION_COUNT + MAX_JOIN_TARGETS;

/// The [`ContextMenuExternal`] scope tag — the External handle, the painted popup
/// panel, and the snapshot anchor share it; item rows paint as the composite
/// `{CTXMENU_TAG}#i<i>`, routing clicks back to this one handle (pinion R51.42).
const CTXMENU_TAG: &str = "sprag_gui.ctxmenu";

/// The click-outside dismiss barrier tag (the composite `{CTXMENU_TAG}#barrier` routes
/// its outside `PointerUp` to the menu handle, which closes the popup — pinion R715).
const CTXMENU_BARRIER_TAG: &str = "sprag_gui.ctxmenu#barrier";

/// The scoped intent the [`ContextMenuExternal`] emits on activation — pinion prefixes
/// the emitting external's tag, so `"command"` arrives as `{CTXMENU_TAG}.command`.
const COMMAND_INTENT_TAG: &str = "sprag_gui.ctxmenu.command";

/// `Owner::cache` key for the [`use_target_pane`] capture.
const TARGET_PANE_KEY: &str = "sprag_gui.ctxmenu.target_pane";

/// `Owner::cache` key for the [`menu_actions`] capture.
const MENU_ACTIONS_KEY: &str = "sprag_gui.ctxmenu.actions";

/// The pane the menu's Paste / Select-all / Break out / Move to act on, CAPTURED when the menu
/// opens (right-click time). Clicking a menu item afterwards blurs the pane focus, so the reducer
/// cannot read the focused pane then — it reads this snapshot instead.
fn use_target_pane() -> Signal<Option<usize>> {
    Owner::current()
        .expect("use_target_pane() requires an active Owner scope")
        .cache(TARGET_PANE_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The menu's action list, CAPTURED when it opens (see the module docs) — the SSOT both the painted
/// labels and the clicked-index resolution read, so a window list that changes under an open popup
/// can never make a click run a different row than the one shown.
fn menu_actions() -> Signal<Vec<MenuAction>> {
    Owner::current()
        .expect("menu_actions() requires an active Owner scope")
        .cache(MENU_ACTIONS_KEY, || Signal::new(Vec::new()))
        .as_ref()
        .clone()
}

/// The action list for a menu opening NOW: the fixed actions, then a `Move to <window>` per window
/// that is NOT the current one (the pane lives in the current window; a join moves it elsewhere).
/// A single-window session offers only the fixed actions.
fn build_actions() -> Vec<MenuAction> {
    let mut actions = vec![
        MenuAction::Copy,
        MenuAction::Paste,
        MenuAction::SelectAll,
        MenuAction::BreakOut,
    ];
    for window in use_terminal()
        .slots
        .windows()
        .into_iter()
        .filter(|window| !window.current)
        .take(MAX_JOIN_TARGETS)
    {
        actions.push(MenuAction::JoinInto(window.name));
    }
    actions
}

/// The binding [`State`](crate::TerminalViewer): the context menu's open anchor +
/// active item, read from the [`ContextMenuExternal`] each frame ([`read_menu_state`]).
/// `Copy` so the shell hands the snapshot into the pure `view` (the sole reason sprag's
/// `State` grew from `()` — the menu is its first stateful surface).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct MenuState {
    /// Window-space anchor when the popup is open, `None` when closed.
    pub(crate) open_at: Option<(f32, f32)>,
    /// The highlighted (active-descendant) item, or `None`.
    pub(crate) active: Option<usize>,
}

/// The [`ContextMenuExternal`] as an extra external (registered every reconcile at the
/// constant [`CTXMENU_TAG`]; pinion R689 preserves its live open state by tag).
pub(crate) fn create_menu_external() -> ExtraExternal {
    ExtraExternal::new(
        CTXMENU_TAG.to_owned(),
        Box::new(ContextMenuExternal::new(MENU_CAPACITY)),
    )
}

/// Project the menu's open / active state out of the model scene (the `read_state`
/// seam). Returns the default (closed) when the external is absent or has no handle.
pub(crate) fn read_menu_state(scene: &Scene) -> MenuState {
    scene
        .find_external_with_tag(CTXMENU_TAG)
        .and_then(|node| node.handle.introspect())
        .map(|intro| {
            let (open_at, active) = read_open_state(intro);
            MenuState { open_at, active }
        })
        .unwrap_or_default()
}

/// Open (or re-anchor) the popup at the window-space press point — the
/// `apply_secondary_click` body. Snapshots the target pane AND the action list (see the
/// module docs), locates the menu external in the model scene, and invokes its `open_at`;
/// reports the External's open verdict.
pub(crate) fn open_at(scene: &mut Scene, x: f32, y: f32) -> bool {
    // Snapshot the target pane AND the action list NOW, while the pane still holds focus and the
    // window list is the one the user is about to see — a subsequent click on a menu item blurs the
    // pane and could race a window-list change.
    use_target_pane().set(focused_pane());
    menu_actions().set(build_actions());
    let Some(node) = scene.find_external_with_tag_mut(CTXMENU_TAG) else {
        return false;
    };
    let Some(intro) = node.handle.introspect_mut() else {
        return false;
    };
    matches!(
        intro.invoke("open_at", ContextMenuExternal::open_at_args(x, y)),
        Ok(IntrospectValue::Bool(true))
    )
}

/// Overlay the open popup + click-outside barrier on the main-window `scene` (a
/// no-op when the menu is closed). The barrier is pushed first, then the popup, both
/// LAST in the root child list so the absolutely-positioned popup paints over
/// everything below it (pinion's documented placement). Uses the boot window size for
/// the barrier extent + placement clamp — the live size is not on the `Frame` (a
/// resized-larger window under-covers the barrier; a v1 limit).
///
/// The rows are the CAPTURED action list's labels (see the module docs), so the popup shows exactly
/// what the reducer will act on.
pub(crate) fn overlay(scene: Scene, menu: MenuState, theme: &Theme) -> Scene {
    let Some(anchor) = menu.open_at else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    // The labels are owned (a join target carries its window name); hold them so the `&[&str]` the
    // painter takes can borrow them.
    let labels: Vec<String> = menu_actions().get().iter().map(MenuAction::label).collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    root.children.push(dismiss_barrier(
        CTXMENU_BARRIER_TAG,
        (0, 0),
        (WINDOW_W, WINDOW_H),
    ));
    root.children.push(view_context_menu(
        CTXMENU_TAG,
        CTXMENU_TAG,
        &label_refs,
        menu.active,
        ContextMenuPlacement {
            anchor,
            window: (WINDOW_W, WINDOW_H),
        },
        theme,
        &MenuStyle::m3_default(),
    ));
    Scene::Container(root)
}

/// Route a drained intent: if it is the menu's `"command"` (an item activated), run the
/// item's action and report handled. Any other intent is left for the caller's own
/// reducer arms (the dock tear-off family).
pub(crate) fn handle_command(intent: &Intent) -> bool {
    if intent.tag_str() != COMMAND_INTENT_TAG {
        return false;
    }
    if let Some(index) = command_index(&intent.payload) {
        run_item(index);
    }
    true
}

/// The activated item index from the `"command"` payload (the External emits the index
/// as text, matching the menu's item order).
fn command_index(payload: &IntrospectValue) -> Option<usize> {
    match payload {
        IntrospectValue::Text(s) => s.parse().ok(),
        _ => None,
    }
}

/// Run the CAPTURED action at row `index`. Copy uses the active selection (wherever it is); the rest
/// act on the pane snapshotted at open time (a right-click does not retarget focus, and the item
/// click has since blurred it). Break out / Move to are silent no-ops on a refusal (the sole pane of
/// a window cannot break; a pane already in the target cannot join) — the daemon is the authority.
fn run_item(index: usize) {
    let Some(action) = menu_actions().get().get(index).cloned() else {
        return;
    };
    match action {
        MenuAction::Copy => {
            let copied = crate::selection::copy_selection();
            tracing::debug!(target: "sprag_gui::input", copied, "ctxmenu copy");
        }
        MenuAction::Paste => {
            let pane = use_target_pane().get();
            let pasted = pane.is_some_and(crate::selection::paste_clipboard);
            tracing::debug!(target: "sprag_gui::input", ?pane, pasted, "ctxmenu paste");
        }
        MenuAction::SelectAll => {
            let pane = use_target_pane().get();
            if let Some(p) = pane {
                crate::selection::select_all(p);
            }
            tracing::debug!(target: "sprag_gui::input", ?pane, "ctxmenu select all");
        }
        MenuAction::BreakOut => {
            let pane = use_target_pane().get();
            let created = pane.and_then(|p| use_terminal().slots.break_pane(p, None));
            tracing::debug!(target: "sprag_gui::input", ?pane, ?created, "ctxmenu break out");
        }
        MenuAction::JoinInto(window) => {
            let pane = use_target_pane().get();
            let closed = pane.and_then(|p| use_terminal().slots.join_pane(p, &window));
            tracing::debug!(target: "sprag_gui::input", ?pane, window, ?closed, "ctxmenu join into");
        }
    }
}

/// The focused pane's tile index (`focus_state::focused()` -> pane), or `None` when
/// focus is off a pane.
fn focused_pane() -> Option<usize> {
    pinion_core::focus_state::focused()
        .as_deref()
        .and_then(pane_index_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_index_parses_the_text_payload() {
        assert_eq!(
            command_index(&IntrospectValue::Text("2".to_owned())),
            Some(2)
        );
        assert_eq!(command_index(&IntrospectValue::Text("x".to_owned())), None);
        assert_eq!(command_index(&IntrospectValue::Bool(true)), None);
    }

    #[test]
    fn handle_command_ignores_a_foreign_intent() {
        // A dock tear-off intent is NOT the menu command — left for the caller's arms.
        let tear = Intent::new_static("terminal-0.tear_off", IntrospectValue::Text("x".to_owned()));
        assert!(!handle_command(&tear));
    }

    #[test]
    fn each_action_labels_itself() {
        assert_eq!(MenuAction::Copy.label(), "Copy");
        assert_eq!(MenuAction::Paste.label(), "Paste");
        assert_eq!(MenuAction::SelectAll.label(), "Select all");
        assert_eq!(MenuAction::BreakOut.label(), "Break out");
        // A join target carries its destination window's name in the label.
        assert_eq!(
            MenuAction::JoinInto("logs".to_owned()).label(),
            "Move to logs"
        );
    }

    #[test]
    fn the_menu_capacity_covers_the_fixed_actions_plus_every_join_target() {
        // The external is registered ONCE at this capacity; a live menu never paints more rows than
        // the fixed actions plus one per join target, so a click index always lands in range.
        assert_eq!(MENU_CAPACITY, FIXED_ACTION_COUNT + MAX_JOIN_TARGETS);
        assert_eq!(
            FIXED_ACTION_COUNT, 4,
            "Copy / Paste / Select all / Break out"
        );
    }
}
