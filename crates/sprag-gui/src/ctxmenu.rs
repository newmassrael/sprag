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
//! ## What a row MEANS lives in the catalog, not here
//!
//! This module is the menu's PLUMBING — the External, the anchor, the paint, the intent, the
//! open-time captures. What a row does is [`crate::command`]'s: [`menu_rows`] builds the rows and
//! [`Command::run`](crate::command::Command::run) performs them, the same two functions the command
//! palette goes through. So an action cannot mean one thing from a right-click and another from
//! `Ctrl+Shift+P`, and adding one to the client does not mean writing it twice.
//!
//! What stays the menu's own is its editorial half: WHICH commands a pane-anchored popup offers, and
//! the short wording it offers them in. That wording travels with the row rather than living on the
//! command — [`crate::command`]'s module docs carry the reason it is deliberately not one shared
//! string.
//!
//! ## Why the row list is CAPTURED at open time
//!
//! The `Move to <window>` rows are one per OTHER window, so the menu's contents depend on the
//! live window list — which can change out from under an open popup (a second client, an agent).
//! Like the target pane, the whole row list is SNAPSHOT when the menu opens ([`captured_rows`]),
//! so [`overlay`]'s painted labels and [`handle_command`]'s index-to-row resolution read the
//! SAME list and cannot disagree: a click always runs the action the user saw, never a neighbour a
//! mid-open reflow shifted into that row. This is the wtabs "resolve the click against the list it
//! was painted from" rule, taken one step further by freezing the list for the popup's lifetime.
//!
//! Keyboard navigation (Arrow / Enter / Escape) and a11y are deferred (mouse-first).

use crate::command::{MAX_MENU_ROWS, MenuRow, menu_rows};
use crate::terminal::{focused_pane, use_terminal};
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::external::IntrospectValue;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::theme::Theme;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::context_menu::{ContextMenuExternal, read_open_state};
use pinion_core::{Intent, Scene};
use pinion_widget_paint::barrier::dismiss_barrier;
use pinion_widget_paint::menu::{ContextMenuPlacement, MenuStyle, view_context_menu};

/// The [`ContextMenuExternal`] row capacity — the MOST rows the menu can ever paint. Registered ONCE
/// at this count (pinion R689 preserves the live external by tag across the reconcile, so a per-open
/// count change would discard it); [`overlay`] paints only the rows the live list fills, exactly as
/// the tab strip paints only its live windows under a fixed button cap.
///
/// TAKEN from the row builder rather than recomputed here, so the capacity cannot drift from the
/// number of rows [`menu_rows`] is actually able to return.
const MENU_CAPACITY: usize = MAX_MENU_ROWS;

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

/// `Owner::cache` key for the [`captured_rows`] capture.
const MENU_ROWS_KEY: &str = "sprag_gui.ctxmenu.rows";

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

/// The menu's row list, CAPTURED when it opens (see the module docs) — the SSOT both the painted
/// labels and the clicked-index resolution read, so a window list that changes under an open popup
/// can never make a click run a different row than the one shown.
fn captured_rows() -> Signal<Vec<MenuRow>> {
    Owner::current()
        .expect("captured_rows() requires an active Owner scope")
        .cache(MENU_ROWS_KEY, || Signal::new(Vec::new()))
        .as_ref()
        .clone()
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
/// `apply_secondary_click` body. Snapshots the target pane AND the row list (see the
/// module docs), locates the menu external in the model scene, and invokes its `open_at`;
/// reports the External's open verdict.
pub(crate) fn open_at(scene: &mut Scene, x: f32, y: f32) -> bool {
    // Snapshot the target pane AND the row list NOW, while the pane still holds focus and the
    // window list is the one the user is about to see — a subsequent click on a menu item blurs the
    // pane and could race a window-list change.
    use_target_pane().set(focused_pane());
    captured_rows().set(menu_rows(&use_terminal().slots));
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
/// The rows are the CAPTURED list's labels (see the module docs), so the popup shows exactly what the
/// reducer will act on.
pub(crate) fn overlay(scene: Scene, menu: MenuState, theme: &Theme) -> Scene {
    let Some(anchor) = menu.open_at else {
        return scene;
    };
    let Scene::Container(mut root) = scene else {
        return scene;
    };
    // The labels are owned (a join target carries its window name); hold them so the `&[&str]` the
    // painter takes can borrow them.
    let rows = captured_rows().get();
    let label_refs: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
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

/// Run the CAPTURED row at `index`, through the one shared guarded entry
/// [`confirm::run_or_arm`](crate::confirm::run_or_arm).
///
/// No menu row is destructive today, so that door is a straight passage to
/// [`Command::run`](crate::command::Command::run) here. It is still the door this goes through, because
/// the alternative is a surface that runs commands unguarded and a rule someone has to remember when
/// the menu grows its first irreversible row — see [`crate::confirm`] on why the guard is a door and
/// not a rule.
///
/// `Copy` acts on the active selection wherever it lives; the rest act on the pane snapshotted at
/// open time (a right-click does not retarget focus, and the item click has since blurred it). Break
/// out / Move to are silent no-ops on a refusal (the sole pane of a window cannot break; a pane
/// already in the target cannot join) — the daemon is the authority.
///
/// The one `debug` line replaces the five this function used to emit, one per action. The outcome
/// each of those reported (did the copy find a selection, did the join empty the source window) is
/// deliberately no longer available: [`Command::run`](crate::command::Command::run) drops those bools
/// so that a refusal cannot be treated one way from the menu and another from the palette. What is
/// logged is what this function itself decides — which command, against which pane.
fn run_item(index: usize) {
    let Some(row) = captured_rows().get().get(index).cloned() else {
        return;
    };
    let pane = use_target_pane().get();
    tracing::debug!(
        target: "sprag_gui::input",
        command = ?row.command,
        ?pane,
        "ctxmenu command"
    );
    crate::confirm::run_or_arm(row.command, pane, &use_terminal().slots);
}

#[cfg(test)]
mod tests {
    use super::*;
    // The catalog's own type, named here rather than at module scope: outside the tests this module
    // reaches a command only through the rows `menu_rows` hands it.
    use crate::command::Command;
    use crate::terminal::{seed_terminal, use_terminal};
    use pinion_core::Clipboard;
    use sprag_host::Host;
    use sprag_terminal::CommandBuilder;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A clipboard that only RECORDS, installed through [`crate::selection::seed_clipboard`] so a
    /// copy is observable without writing to the developer's real OS clipboard. `copy_to` / `paste_from`
    /// keep the trait's defaults, which route CLIPBOARD here and no-op PRIMARY — so the PRIMARY
    /// publish `select_all` performs cannot be mistaken for the copy under test.
    #[derive(Default)]
    struct RecordingClipboard(RefCell<Option<String>>);

    impl Clipboard for RecordingClipboard {
        fn copy(&self, text: String) {
            *self.0.borrow_mut() = Some(text);
        }
        fn paste(&self) -> Option<String> {
            self.0.borrow().clone()
        }
    }

    /// A long-lived `cat` pane (holds its PTY open across the reducer drive), matching the
    /// deterministic pane the input-routing tests seed.
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// The exact `"command"` intent the [`ContextMenuExternal`] emits when the row at `index` is
    /// activated (pinion prefixes the emitting external's tag, so it arrives as
    /// [`COMMAND_INTENT_TAG`]). Constructing it here drives [`handle_command`] the way a live menu
    /// click does, without the shell / pointer round-trip.
    fn command_intent(index: usize) -> Intent {
        Intent::new_static(COMMAND_INTENT_TAG, IntrospectValue::Text(index.to_string()))
    }

    /// The index of the row running `command` in the CAPTURED list (the same list [`overlay`] paints
    /// and [`run_item`] resolves against), so a test names the row by MEANING, not a hard-coded
    /// offset.
    fn row_of(command: &Command) -> usize {
        captured_rows()
            .get()
            .iter()
            .position(|row| &row.command == command)
            .expect("the menu offers a row running this command")
    }

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

    /// The `Break out` row drives a real `break-pane` end to end: activating it routes the menu
    /// command through [`handle_command`] -> [`run_item`] -> the shared
    /// [`Command::run`](crate::command::Command::run) -> `SlotView::break_pane` into the host, which
    /// MOVES the target pane into a new window. Seeds an in-process two-pane / one-window host (the
    /// [`seed_terminal`] seam the input-routing tests use), so the reducer wiring the live GUI smoke
    /// exercises is pinned WITHOUT the shell / Xvfb.
    ///
    /// This is also the proof that the FOLD onto the catalog kept the menu working: nothing here
    /// names a menu-local action any more. REVERT-PROOF: neutering the `BreakOut` arm of
    /// [`Command::run`](crate::command::Command::run) leaves the window count at one and this fails.
    #[test]
    fn the_break_out_command_moves_the_target_pane_into_a_new_window() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None, None)
            .unwrap();
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None, None)
            .unwrap();
        Owner::new().run(|| {
            seed_terminal(host); // use_terminal() now returns these two cat panes
            let tv = use_terminal();
            // Two panes boot into one window's two slots.
            assert_eq!(tv.slots.occupied_slots(), vec![0, 1]);
            assert_eq!(tv.slots.windows().len(), 1, "both panes share one window");
            // Mirror `open_at`: capture the target pane AND the row list the reducer reads.
            use_target_pane().set(Some(0));
            captured_rows().set(menu_rows(&tv.slots));
            assert!(
                handle_command(&command_intent(row_of(&Command::BreakOut))),
                "the menu command is handled"
            );
            assert_eq!(
                tv.slots.windows().len(),
                2,
                "the target pane broke out into a new window"
            );
        });
    }

    /// The `Copy` row still copies when NO pane held focus as the menu opened — the case that makes a
    /// `needs_pane` gate at the top of [`Command::run`](crate::command::Command::run) wrong. `Copy`
    /// acts on whatever selection is active, so it needs no target; the OFFER predicate says
    /// otherwise only because offering it with no pane on screen would be pointless.
    ///
    /// Drives the whole live path: a selection exists, the captured target is `None` (what `open_at`
    /// would have recorded), and the activation goes through the real intent.
    ///
    /// REVERT-PROOF: put the `if self.needs_pane() && target.is_none() { return; }` gate back at the
    /// top of `Command::run` and this fails with nothing copied.
    #[test]
    fn the_copy_row_still_copies_with_no_pane_captured() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None, None)
            .unwrap();
        Owner::new().run(|| {
            seed_terminal(host);
            // Install the recorder BEFORE anything resolves the real clipboard.
            let recorder: Rc<dyn Clipboard> = Rc::new(RecordingClipboard::default());
            crate::selection::seed_clipboard(&recorder);
            let tv = use_terminal();

            // A selection exists in the pane (this publishes to PRIMARY, which the recorder no-ops)...
            crate::selection::select_all(0);
            assert!(
                recorder.paste().is_none(),
                "nothing is on the CLIPBOARD selection yet, so the assertion below is about the copy"
            );

            // ...while nothing holds focus, so the menu captured no target.
            use_target_pane().set(None);
            captured_rows().set(menu_rows(&tv.slots));
            assert!(
                handle_command(&command_intent(row_of(&Command::Copy))),
                "the menu command is handled"
            );

            assert!(
                recorder.paste().is_some(),
                "the copy reached the clipboard with no captured pane"
            );
        });
    }

    /// The `Move to <window>` row drives a real `join-pane` end to end: with a second window present,
    /// [`menu_rows`] offers a join target, and activating it routes through [`handle_command`] ->
    /// [`run_item`] -> the shared [`Command::run`](crate::command::Command::run) -> `join_pane`,
    /// MOVING the pane into the named window and closing the emptied source. The second window is set
    /// up by breaking the pane out first (cat panes only, no `$SHELL` spawn), so this also confirms a
    /// broke-out pane can be joined straight back.
    /// REVERT-PROOF: dropping the `JoinInto` arm of [`Command::run`](crate::command::Command::run)
    /// leaves the window count at two and this fails.
    #[test]
    fn the_move_to_command_joins_the_target_pane_into_the_named_window() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None, None)
            .unwrap();
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None, None)
            .unwrap();
        Owner::new().run(|| {
            seed_terminal(host);
            let tv = use_terminal();
            // Break a pane out to create the second window (now current, holding that pane).
            assert!(
                tv.slots.break_pane(0, None).is_some(),
                "the break sets up the second window"
            );
            let _ = tv.slots.reconcile(); // remap slots onto the new current window
            assert_eq!(tv.slots.windows().len(), 2);
            let occupied = tv.slots.occupied_slots();
            assert_eq!(
                occupied.len(),
                1,
                "the new current window holds exactly the broke-out pane"
            );
            // Mirror `open_at` for that pane: a second window now yields a `Move to` target.
            use_target_pane().set(Some(occupied[0]));
            captured_rows().set(menu_rows(&tv.slots));
            let join = captured_rows()
                .get()
                .iter()
                .find_map(|row| match &row.command {
                    Command::JoinInto(window) => Some(Command::JoinInto(window.clone())),
                    _ => None,
                })
                .expect("the second window offers a join target");
            assert!(
                handle_command(&command_intent(row_of(&join))),
                "the menu command is handled"
            );
            assert_eq!(
                tv.slots.windows().len(),
                1,
                "the pane rejoined the named window and the emptied source auto-closed"
            );
        });
    }
}
