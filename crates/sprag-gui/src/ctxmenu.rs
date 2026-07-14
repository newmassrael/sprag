//! Right-click context menu (R140): `Copy` / `Paste` / `Select all` on a pane.
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
//!   [`handle_command`] -> the [`selection`](crate::selection) copy / paste / select-all
//!   actions.
//!
//! Keyboard navigation (Arrow / Enter / Escape) and a11y are deferred (mouse-first).

use crate::terminal::pane_index_of;
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::external::IntrospectValue;
use pinion_core::theme::Theme;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::context_menu::{ContextMenuExternal, read_open_state};
use pinion_core::{Intent, Scene};
use pinion_widget_paint::barrier::dismiss_barrier;
use pinion_widget_paint::menu::{ContextMenuPlacement, MenuStyle, view_context_menu};

/// The menu's command items (index `i` becomes the row tagged `{CTXMENU_TAG}#i<i>`).
pub(crate) const MENU_ITEMS: [&str; 3] = ["Copy", "Paste", "Select all"];

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
        Box::new(ContextMenuExternal::new(MENU_ITEMS.len())),
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
/// `apply_secondary_click` body. Locates the menu external in the model scene and
/// invokes its `open_at`; reports the External's open verdict.
pub(crate) fn open_at(scene: &mut Scene, x: f32, y: f32) -> bool {
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
pub(crate) fn overlay(scene: Scene, menu: MenuState, theme: &Theme) -> Scene {
    let Some(anchor) = menu.open_at else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    root.children.push(dismiss_barrier(
        CTXMENU_BARRIER_TAG,
        (0, 0),
        (WINDOW_W, WINDOW_H),
    ));
    root.children.push(view_context_menu(
        CTXMENU_TAG,
        CTXMENU_TAG,
        &MENU_ITEMS,
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

/// Run item `index`'s action. Copy uses the active selection (wherever it is); Paste /
/// Select all act on the FOCUSED pane (a right-click does not retarget focus, and the
/// `apply_secondary_click` scene has no pane rects to hit-test).
fn run_item(index: usize) {
    match MENU_ITEMS.get(index).copied() {
        Some("Copy") => {
            crate::selection::copy_selection();
        }
        Some("Paste") => {
            if let Some(pane) = focused_pane() {
                crate::selection::paste_clipboard(pane);
            }
        }
        Some("Select all") => {
            if let Some(pane) = focused_pane() {
                crate::selection::select_all(pane);
            }
        }
        _ => {}
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
    fn menu_items_are_the_three_chosen_commands() {
        assert_eq!(MENU_ITEMS, ["Copy", "Paste", "Select all"]);
    }
}
