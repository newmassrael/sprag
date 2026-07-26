//! The command CATALOG: the things this client can be asked to do BY NAME.
//!
//! ## Why a catalog rather than one list per surface
//!
//! Every action sprag can perform was, until now, named where it is INVOKED — the context menu's
//! [`MenuAction`](crate::ctxmenu) rows, the chord table in [`crate::input`], the buttons of the
//! session rail and the window strip, the `sprag` CLI's verb `match`. That is fine while each
//! surface offers a handful of actions the user reaches by pointing at it. A PALETTE is the first
//! surface whose whole purpose is to name them ALL, so writing its rows out by hand would have
//! created one more list to drift from the others: a renamed action would keep working from the
//! menu and silently mis-describe itself in the palette.
//!
//! So a command is a VALUE here — its title, its keyboard equivalent, and what it does live
//! together in one place, and [`crate::palette`] is a VIEW over that value rather than a parallel
//! list of strings. The same shape the context menu already uses for its own rows ("a semantic
//! action, so paint and the reducer name the SAME thing rather than agreeing on a stringly-typed
//! item order") — lifted out of that one menu so a second surface can share it.
//!
//! **Honest residual:** the context menu's rows are still defined in [`crate::ctxmenu`], so today
//! this catalog is the SSOT for what the PALETTE offers, not yet for every named action in the
//! client. Folding the menu onto it is a follow-up; until that lands, an action offered by both
//! surfaces is defined twice and this doc must not claim otherwise.
//!
//! ## What is NOT in the catalog, and why
//!
//! * **Destructive commands** (`kill-window`, `kill-session`). Both are reachable today only
//!   through a guarded path: the session rail ARMS a kill and a second, separate click confirms it,
//!   precisely so no single activation on a moving list can destroy the wrong thing. A palette row
//!   is the opposite — a fuzzy query plus `Enter`, where one keystroke too many would end a
//!   session. tmux draws the same line (`kill-window` is bound through `confirm-before`), so these
//!   stay out until the palette has a confirm step of its own to offer them through.
//! * **Commands that need an ARGUMENT** (`rename-window`). The palette's field holds a query, and
//!   a second field for a value is a mode this surface does not have yet.
//! * **Creating and closing PANES.** Not a policy call — the client genuinely cannot do it yet:
//!   [`HostClient`](sprag_host::HostClient) exposes no `spawn` / `close`, so there is no live path
//!   to offer. The wire actions exist and the boot path already drives them, so this is an additive
//!   host-client capability, filed as its own increment rather than faked here.
//!
//! ## The pane a command acts on
//!
//! Most of these act on ONE pane, and the palette cannot read the focused pane when a row is
//! activated — opening the palette moves focus to its query field. So the target pane is captured
//! when the palette OPENS and threaded in here as `target`; a command that needs a pane is simply
//! not offered when there is none ([`catalog`]), which is why [`Command::run`] can take the target
//! it was built for and act.

use crate::slotview::SlotView;

/// One thing the client can be asked to do by name.
///
/// Two kinds live in one enum: the FIXED commands, which mean the same thing in every session, and
/// the DYNAMIC ones, which carry the name of a live window or session ([`catalog`] builds one per
/// window / session, the way the context menu builds one `Move to <window>` row per window). The
/// dynamic ones own a `String` rather than an index because a name survives a list that moves under
/// the open palette, and an index does not.
/// (Serde-derived because it is held in a reactive `Signal`, whose value type carries pinion's
/// serialization bound — the same reason the context menu's own action enum derives them.)
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Command {
    /// Open the find bar on the target pane (`Ctrl+Shift+F`).
    Find,
    /// Copy the active selection to the clipboard (`Ctrl+Shift+C`).
    Copy,
    /// Paste the clipboard into the target pane (`Ctrl+Shift+V`).
    Paste,
    /// Select the whole of the target pane.
    SelectAll,
    /// Float the target pane out of the dock, or dock it back (`Ctrl+Shift+Enter`).
    ToggleFloat,
    /// Move the target pane into a new window of its own (tmux `break-pane`).
    BreakOut,
    /// Create a window in the current session and select it.
    NewWindow,
    /// Select the named window of the current session.
    SelectWindow(String),
    /// Create a session and switch this client to it.
    NewSession,
    /// Switch this client to the named session.
    SwitchSession(String),
    /// Switch back to the most recently used other session (`Ctrl+Shift+L`).
    LastSession,
}

impl Command {
    /// The row's title — what the palette PAINTS and what a query is matched against.
    ///
    /// Written as an imperative phrase ("Find in scrollback", not "Find"), because a palette row is
    /// read out of context: the noun is what makes a fuzzy query like `wind` land on something the
    /// user recognizes.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Find => "Find in scrollback".to_owned(),
            Self::Copy => "Copy selection".to_owned(),
            Self::Paste => "Paste into pane".to_owned(),
            Self::SelectAll => "Select all in pane".to_owned(),
            Self::ToggleFloat => "Toggle floating pane".to_owned(),
            Self::BreakOut => "Break pane out to a new window".to_owned(),
            Self::NewWindow => "New window".to_owned(),
            Self::SelectWindow(name) => format!("Go to window {name}"),
            Self::NewSession => "New session".to_owned(),
            Self::SwitchSession(name) => format!("Switch to session {name}"),
            Self::LastSession => "Switch to the last session".to_owned(),
        }
    }

    /// The keyboard chord that runs this command without the palette, or `None` for one the palette
    /// is the only way to reach.
    ///
    /// Shown at the end of the row: a palette that lists a command it shares with a chord should
    /// teach that chord, or it trains the user to keep opening the palette for something one
    /// keystroke already does. (The strings are the DISPLAY form of the bindings
    /// [`crate::input`] recognizes — there is no chord table to derive them from, so a renamed
    /// binding must be renamed here too.)
    pub(crate) fn chord(&self) -> Option<&'static str> {
        match self {
            Self::Find => Some("Ctrl+Shift+F"),
            Self::Copy => Some("Ctrl+Shift+C"),
            Self::Paste => Some("Ctrl+Shift+V"),
            Self::ToggleFloat => Some("Ctrl+Shift+Enter"),
            Self::LastSession => Some("Ctrl+Shift+L"),
            Self::SelectAll
            | Self::BreakOut
            | Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_) => None,
        }
    }

    /// Whether this command acts on a pane, and so is only offered when one is targetable.
    ///
    /// This is the ONLY gate on a pane command. In particular neither `ToggleFloat` nor `BreakOut`
    /// is additionally gated on the pane being movable: floating the last docked pane is REFUSED by
    /// the dock primitive itself ([`crate::dock::toggle_pane_floating`], where the invariant lives),
    /// and breaking out the last pane of a window is perfectly legal — the emptied source window
    /// closes behind it. A movability gate here would have been the context menu's float predicate
    /// applied to a session operation it does not describe.
    fn needs_pane(&self) -> bool {
        match self {
            Self::Find
            | Self::Copy
            | Self::Paste
            | Self::SelectAll
            | Self::ToggleFloat
            | Self::BreakOut => true,
            Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_)
            | Self::LastSession => false,
        }
    }

    /// Run the command against the pane the palette captured when it opened.
    ///
    /// Each arm drives the SAME authority the equivalent chord or button drives — the find bar's
    /// own `open`, the selection module's copy / paste, the dock's float toggle, the `SlotView`
    /// window and session actions the tab strip and rail use — so a command cannot mean one thing
    /// from the palette and another from the surface it already had. Nothing is re-implemented here.
    ///
    /// A `bool` an underlying call returns (a copy with no selection, a paste into a gone pane) is
    /// deliberately dropped: those are already the action's own tolerated no-ops, and the palette
    /// has no place to report one that the surface itself does not.
    pub(crate) fn run(&self, target: Option<usize>, slots: &SlotView) {
        // A pane command with no target cannot act. `catalog` does not offer one in that state, so
        // this is the belt to that braces — reached only if a caller builds a command by hand.
        if self.needs_pane() && target.is_none() {
            return;
        }
        match self {
            Self::Find => {
                if let Some(pane) = target {
                    crate::find::open(pane);
                }
            }
            Self::Copy => {
                let _ = crate::selection::copy_selection();
            }
            Self::Paste => {
                if let Some(pane) = target {
                    let _ = crate::selection::paste_clipboard(pane);
                }
            }
            Self::SelectAll => {
                if let Some(pane) = target {
                    crate::selection::select_all(pane);
                }
            }
            Self::ToggleFloat => {
                if let Some(pane) = target {
                    crate::dock::toggle_pane_floating(pane);
                }
            }
            Self::BreakOut => {
                if let Some(pane) = target {
                    slots.break_pane(pane, None);
                }
            }
            Self::NewWindow => {
                // Creates AND selects (the host action does both), like the strip's "+".
                slots.new_window();
            }
            Self::SelectWindow(name) => slots.select_window(name),
            Self::NewSession => {
                // Creates AND switches, like the rail's "+".
                let _ = slots.new_session();
            }
            Self::SwitchSession(name) => slots.switch_session(name),
            Self::LastSession => slots.switch_to_last_session(),
        }
    }
}

/// The cap on `Go to window <name>` rows, matching the tab strip's own practical ceiling
/// ([`MAX_WINDOW_TABS`](crate::wtabs::MAX_WINDOW_TABS)) so the palette offers exactly the windows
/// the strip can show. A session with more windows than this reaches the overflow through
/// `sprag select-window`, the same honest bound the strip and the context menu state.
const MAX_WINDOW_ROWS: usize = crate::wtabs::MAX_WINDOW_TABS;

/// The cap on `Switch to session <name>` rows, for the same reason, against the session rail.
const MAX_SESSION_ROWS: usize = 16;

/// Every command offered RIGHT NOW, in the order a palette with an empty query lists them.
///
/// Built from live state (the window and session lists) and from `target`, so it is a snapshot:
/// the palette FREEZES the result when it opens and filters that frozen list as the query changes.
/// This is the context menu's rule, and it is load-bearing for the same reason — a list rebuilt
/// under an open palette could move a row between the frame the user read and the `Enter` that
/// runs it, so an activation would run a neighbour of the command they chose.
///
/// Order is by kind, most-local first: the pane commands (which act on what the user is looking
/// at), then the window commands, then the session ones. Within a kind, declaration order. An
/// empty query therefore reads as a menu of the client's capabilities rather than an arbitrary
/// permutation, and the fuzzy ranking takes over the moment anything is typed.
pub(crate) fn catalog(target: Option<usize>, slots: &SlotView) -> Vec<Command> {
    let mut out = vec![
        Command::Find,
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::ToggleFloat,
        Command::BreakOut,
        Command::NewWindow,
    ];
    // A pane command with no pane to act on is not offered at all: a row that is guaranteed to do
    // nothing is worse than a shorter list, because the user cannot tell the two apart.
    out.retain(|command| !command.needs_pane() || target.is_some());

    // One row per OTHER window: going to the window you are already in is not an action.
    let windows = slots.windows();
    out.extend(
        windows
            .iter()
            .filter(|window| !window.current)
            .take(MAX_WINDOW_ROWS)
            .map(|window| Command::SelectWindow(window.name.clone())),
    );

    // ...and one per OTHER session, on the same terms.
    let current = slots.current_session();
    let sessions = slots.sessions();
    out.push(Command::NewSession);
    out.extend(
        sessions
            .iter()
            .filter(|session| session.name != current)
            .take(MAX_SESSION_ROWS)
            .map(|session| Command::SwitchSession(session.name.clone())),
    );
    out.push(Command::LastSession);
    out
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use pinion_core::GridBuffer;
    use sprag_host::{HostClient, PaneScrollFacts};
    use sprag_input::Modifiers;
    use sprag_terminal::{LayoutSnapshot, LayoutWire, PaneId, SessionInfo, WindowInfo};

    use super::*;

    /// What a run recorded, so a test reads the ACTION the command drove rather than a screen.
    #[derive(Default)]
    struct Log {
        selected_windows: Vec<String>,
        new_windows: usize,
        switched_sessions: Vec<String>,
        new_sessions: usize,
        broken_panes: Vec<PaneId>,
        last_session: usize,
    }

    /// A [`HostClient`] serving fixed window / session lists and RECORDING the actions
    /// [`Command::run`] drives — the same recording-fake shape the session rail's own reducer test
    /// uses, and for the same reason: the in-process `Host` no-ops the session actions, so a fake is
    /// the only way to observe which one a command routed to. Every other method is inert.
    struct CatalogHost {
        windows: Vec<WindowInfo>,
        sessions: Vec<String>,
        current: String,
        log: Rc<RefCell<Log>>,
    }

    impl HostClient for CatalogHost {
        fn windows(&self) -> Vec<WindowInfo> {
            self.windows.clone()
        }
        fn select_window(&self, name: &str) {
            self.log.borrow_mut().selected_windows.push(name.to_owned());
        }
        fn new_window(&self) -> String {
            self.log.borrow_mut().new_windows += 1;
            "w".to_owned()
        }
        fn kill_window(&self, _name: &str) {}
        fn break_pane(&self, id: PaneId, _name: Option<&str>) -> Option<String> {
            self.log.borrow_mut().broken_panes.push(id);
            Some("w".to_owned())
        }
        fn sessions(&self) -> Vec<SessionInfo> {
            self.sessions
                .iter()
                .map(|name| SessionInfo {
                    name: name.clone(),
                    windows: 1,
                    panes: 1,
                    default: false,
                    cwd: None,
                    branch: None,
                    ports: Vec::new(),
                    attached: 0,
                })
                .collect()
        }
        fn current_session(&self) -> String {
            self.current.clone()
        }
        fn switch_session(&self, name: &str) {
            self.log
                .borrow_mut()
                .switched_sessions
                .push(name.to_owned());
        }
        fn new_session(&self) -> String {
            self.log.borrow_mut().new_sessions += 1;
            "s".to_owned()
        }
        fn kill_session(&self, _name: &str) {}
        fn switch_to_last_session(&self) {
            self.log.borrow_mut().last_session += 1;
        }
        fn pane_ids(&self) -> Vec<PaneId> {
            // A pane whose id is NOT its slot number, so a test asserting on the id it recorded
            // proves the slot→id mapping was applied rather than an accidental identity.
            vec![PaneId(7)]
        }
        fn pane_cells(&self, _id: PaneId, _off: usize) -> GridBuffer {
            GridBuffer::new(1, 1)
        }
        fn pane_scroll_facts(&self, _id: PaneId) -> PaneScrollFacts {
            PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            }
        }
        fn pane_prompt_positions(&self, _id: PaneId) -> Vec<usize> {
            Vec::new()
        }
        fn pane_grid_size(&self, _id: PaneId) -> (u16, u16) {
            (1, 1)
        }
        fn resize(&self, _id: PaneId, _cols: u16, _rows: u16, _cell_px: (u16, u16)) {}
        fn send_key(&self, _id: PaneId, _key: &str, _mods: Modifiers) -> bool {
            false
        }
        fn send_text(&self, _id: PaneId, _text: &str) -> bool {
            false
        }
        fn pane_full_text(&self, _id: PaneId) -> String {
            String::new()
        }
        fn pane_command_label(&self, _id: PaneId) -> String {
            String::new()
        }
        fn pane_title(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn layout(&self) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_layout(&self, _tree: LayoutWire, _expected: u64) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_floating(&self, _id: PaneId, _floating: bool) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
    }

    /// A `SlotView` over a host with `windows` (one marked current) and `sessions` (`current` is
    /// this client's), plus the log every action lands in.
    ///
    /// RECONCILED before it is handed back, because a `SlotView` maps a display slot onto a
    /// [`PaneId`] only once it has adopted the host's pane set — an un-reconciled view answers every
    /// slot-addressed call with `None`, so a command would look like it had run and touched nothing.
    fn slots_with(
        windows: &[(&str, bool)],
        sessions: &[&str],
        current: &str,
    ) -> (SlotView, Rc<RefCell<Log>>) {
        let log: Rc<RefCell<Log>> = Rc::default();
        let host = CatalogHost {
            windows: windows
                .iter()
                .map(|(name, current)| WindowInfo {
                    name: (*name).to_owned(),
                    current: *current,
                })
                .collect(),
            sessions: sessions.iter().map(|s| (*s).to_owned()).collect(),
            current: current.to_owned(),
            log: Rc::clone(&log),
        };
        let slots = SlotView::new(Box::new(host));
        slots.reconcile();
        (slots, log)
    }

    #[test]
    fn a_pane_command_is_not_offered_without_a_pane_to_act_on() {
        // Built twice over the SAME host state, differing only in whether a pane was captured — so
        // the delta is exactly the pane commands.
        let (slots, _log) = slots_with(&[("0", true)], &["0"], "0");
        let with_pane = catalog(Some(0), &slots);
        let without = catalog(None, &slots);

        assert!(
            with_pane.contains(&Command::Find),
            "a captured pane offers the pane commands"
        );
        assert!(
            !without.contains(&Command::Find),
            "with no pane, a command that can only act on one is not offered at all"
        );
        assert!(
            without.contains(&Command::NewSession),
            "the session commands need no pane and stay"
        );
    }

    #[test]
    fn the_catalog_offers_every_other_window_and_session_but_never_the_current_one() {
        let (slots, _log) = slots_with(
            &[("main", true), ("build", false), ("logs", false)],
            &["0", "work"],
            "0",
        );
        let titles: Vec<String> = catalog(Some(0), &slots)
            .iter()
            .map(Command::title)
            .collect();

        assert!(titles.contains(&"Go to window build".to_owned()));
        assert!(titles.contains(&"Go to window logs".to_owned()));
        assert!(
            !titles.contains(&"Go to window main".to_owned()),
            "going to the window you are already in is not an action"
        );
        assert!(titles.contains(&"Switch to session work".to_owned()));
        assert!(
            !titles.contains(&"Switch to session 0".to_owned()),
            "nor is switching to the session you are already attached to"
        );
    }

    #[test]
    fn the_dynamic_rows_are_capped() {
        // One more window and session than each cap; the catalog must stop at the cap rather than
        // grow a palette taller than its list can address.
        let windows: Vec<(String, bool)> = (0..=MAX_WINDOW_ROWS)
            .map(|i| (format!("w{i}"), i == 0))
            .collect();
        let window_refs: Vec<(&str, bool)> = windows
            .iter()
            .map(|(name, current)| (name.as_str(), *current))
            .collect();
        let sessions: Vec<String> = (0..=MAX_SESSION_ROWS + 1)
            .map(|i| format!("s{i}"))
            .collect();
        let session_refs: Vec<&str> = sessions.iter().map(String::as_str).collect();
        let (slots, _log) = slots_with(&window_refs, &session_refs, "s0");

        let built = catalog(Some(0), &slots);
        let windows_offered = built
            .iter()
            .filter(|c| matches!(c, Command::SelectWindow(_)))
            .count();
        let sessions_offered = built
            .iter()
            .filter(|c| matches!(c, Command::SwitchSession(_)))
            .count();
        assert_eq!(windows_offered, MAX_WINDOW_ROWS);
        assert_eq!(sessions_offered, MAX_SESSION_ROWS);
    }

    #[test]
    fn running_a_command_drives_the_action_it_names() {
        // The routing itself: each command reaches the ONE host action it describes, with the
        // captured pane / the named window. REVERT-PROOF: swap any two arms of `run` and the
        // matching assertion below fails.
        let (slots, log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");

        Command::SelectWindow("build".to_owned()).run(Some(0), &slots);
        Command::NewWindow.run(Some(0), &slots);
        Command::BreakOut.run(Some(0), &slots);
        Command::SwitchSession("work".to_owned()).run(None, &slots);
        Command::NewSession.run(None, &slots);
        Command::LastSession.run(None, &slots);

        let log = log.borrow();
        assert_eq!(log.selected_windows, vec!["build".to_owned()]);
        assert_eq!(log.new_windows, 1);
        assert_eq!(
            log.broken_panes,
            vec![PaneId(7)],
            "break-out acts on the pane the captured SLOT maps to, by its host id"
        );
        assert_eq!(log.switched_sessions, vec!["work".to_owned()]);
        assert_eq!(log.new_sessions, 1);
        assert_eq!(log.last_session, 1);
    }

    #[test]
    fn a_pane_command_with_no_captured_pane_does_nothing() {
        // The belt to `catalog`'s braces: a pane command built by hand with no target must not act
        // on some other pane. REVERT-PROOF: drop the `needs_pane` guard in `run` and `break_pane`
        // is reached with whatever id the fallback picked.
        let (slots, log) = slots_with(&[("main", true)], &["0"], "0");
        Command::BreakOut.run(None, &slots);
        assert!(log.borrow().broken_panes.is_empty());
    }

    #[test]
    fn every_command_carries_a_non_empty_title() {
        // The title is what a query matches and what the row paints; an empty one would be an
        // unreachable, invisible row.
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");
        for command in catalog(Some(0), &slots) {
            assert!(
                !command.title().trim().is_empty(),
                "{command:?} paints and matches on its title"
            );
        }
    }
}
