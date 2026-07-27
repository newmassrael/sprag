//! The command CATALOG: the things this client can be asked to do BY NAME.
//!
//! ## Why a catalog rather than one list per surface
//!
//! Every action sprag can perform was, until this module, named where it is INVOKED — the context
//! menu's own action enum, the chord table in [`crate::input`], the buttons of the session rail and
//! the window strip, the `sprag` CLI's verb `match`. That is fine while each surface offers a
//! handful of actions the user reaches by pointing at it. A PALETTE is the first surface whose whole
//! purpose is to name them ALL, so writing its rows out by hand would have created one more list to
//! drift from the others: a renamed action would keep working from the menu and silently
//! mis-describe itself in the palette.
//!
//! So a command is a VALUE here — its title, its keyboard equivalent, and what it does live
//! together in one place, and a surface is a VIEW over that value rather than a parallel list of
//! strings. The shape came from the context menu ("a semantic action, so paint and the reducer name
//! the SAME thing rather than agreeing on a stringly-typed item order"), lifted out of that one menu
//! so a second surface could share it — and the menu now reads it back: [`crate::ctxmenu`] paints
//! the rows [`menu_rows`] builds and its reducer runs them through the same [`Command::run`] the
//! palette calls. The client has ONE definition of what a named command DOES.
//!
//! ## What is deliberately NOT shared: the wording, and the row policy
//!
//! Two surfaces offering the same command still differ in two ways, and neither is drift:
//!
//! * **The wording.** A context-menu row is read ANCHORED on the pane it was opened over, so it can
//!   drop the object — `Copy`, `Break out`. A palette row is read out of context, one line in a
//!   ranked list, so it has to carry the noun a fuzzy query lands on — `Copy selection`, `Break pane
//!   out to a new window`. One shared string would have to pick one of those, and either choice
//!   damages the other surface. So [`Command::title`] is the PALETTE's phrasing, and a menu row
//!   carries its own short [`label`](MenuRow::label) written beside the command it names in
//!   [`menu_rows`] — where the menu's editorial decision belongs, and where no command the menu does
//!   not offer needs a label at all.
//! * **The row policy.** The palette does not offer a command it cannot run ([`catalog`] drops the
//!   pane commands when no pane was captured), because in a long searched list a dead row is
//!   indistinguishable from a live one. The menu keeps its fixed rows either way: it is a short
//!   popup opened ON something, its refusals are already silent no-ops, and a menu that opened EMPTY
//!   would be the worse failure.
//!
//! ## The destructive commands, and the third thing that is shared
//!
//! `kill-window` and `kill-session` were held OUT of the catalog while the palette had no way to
//! ask before acting: a fuzzy query plus `Enter` is exactly the shape where one keystroke too many
//! ends a session, and tmux draws the same line (`kill-window` is bound through `confirm-before`).
//! They are in now, because the confirmation is no longer a property of a surface. Which commands
//! are destructive — and the words a prompt must show to describe one — is [`Command::confirmation`],
//! decided HERE beside what the command does, and [`crate::confirm`] is the one surface that asks.
//! No caller chooses: every surface activates a command through
//! [`confirm::run_or_arm`](crate::confirm::run_or_arm), which routes a destructive command to the
//! prompt and everything else straight to [`Command::run`]. A surface added later cannot forget the
//! guard, because it never reaches `run` itself.
//!
//! That is the same lesson the wording split teaches, applied to policy instead of prose: the thing
//! that must not differ between surfaces belongs on the command; the thing that must differ belongs
//! to the surface.
//!
//! ## What is still NOT in the catalog, and why
//!
//! * **Commands that need an ARGUMENT** (`rename-window`). The palette's field holds a query, and
//!   a second field for a value is a mode this surface does not have yet.
//!
//! Creating and closing PANES used to be listed here too — not as a policy call but because the
//! client genuinely could not do it: [`HostClient`](sprag_host::HostClient) exposed no pane
//! create / close, so there was no live path to offer. It does now
//! ([`new_pane`](sprag_host::HostClient::new_pane) / [`kill_pane`](sprag_host::HostClient::kill_pane)),
//! and both rows are in the catalog — the kill among the destructive ones, since it ends a running
//! program.
//!
//! ## The pane a command acts on
//!
//! Most of these act on ONE pane, and neither surface can read the focused pane at the moment a row
//! is activated: opening the palette moves focus to its query field, and clicking a menu item blurs
//! the pane the menu was opened over. So both surfaces capture the target pane when they OPEN and
//! thread it in here as `target`. [`Command::run`] therefore acts on the pane it was handed, and
//! each arm that needs one is total over its absence — see the note on [`Command::run`] for why that
//! is the arm's job and not a gate at the top.

use sprag_host::ProjectAction;

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
    /// Move the target pane into the named window (tmux `join-pane`) — `BreakOut`'s inverse.
    JoinInto(String),
    /// Create a pane in the current window (tmux `split-window`).
    NewPane,
    /// Close the target pane (tmux `kill-pane`). DESTRUCTIVE — see [`Command::confirmation`].
    KillPane,
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
    /// Kill the named window of the current session (tmux `kill-window`). DESTRUCTIVE — see
    /// [`Command::confirmation`].
    KillWindow(String),
    /// Kill the named session (tmux `kill-session`). DESTRUCTIVE — see [`Command::confirmation`].
    KillSession(String),
    /// A command DECLARED in a config file — the target pane's project (`.sprag.toml`) or the user's
    /// own (`config.toml`).
    ///
    /// ONE variant for both sources on purpose. What a declared command IS, and what activating it
    /// does, are identical: it is pasted at the pane's prompt for the user's own `Enter`. A second
    /// variant would duplicate every arm to record a distinction nothing acts on — the trust
    /// difference between the two files (one arrives with a repository, one the user wrote) is
    /// answered by that shared treatment, not by branching on origin. If a future affordance ever
    /// runs one WITHOUT the user's keystroke, that is when the origin starts to matter and this
    /// splits.
    ///
    /// Carries the whole action rather than its name, for the reason the dynamic window / session
    /// rows carry names: the palette freezes its list at open time, and an index or a name would
    /// have to be re-resolved against a config that may have been edited since.
    Declared(ProjectAction),
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
            Self::JoinInto(name) => format!("Move pane to window {name}"),
            // "Split" is the word every terminal multiplexer uses for this and the one a user will
            // type; "new pane" is what it actually produces and what a query of "new" should also
            // reach. The title carries both rather than choosing.
            Self::NewPane => "Split into a new pane".to_owned(),
            Self::NewWindow => "New window".to_owned(),
            Self::SelectWindow(name) => format!("Go to window {name}"),
            Self::NewSession => "New session".to_owned(),
            Self::SwitchSession(name) => format!("Switch to session {name}"),
            Self::LastSession => "Switch to the last session".to_owned(),
            // The verb LEADS, so a query of "kill" collects every destructive row in one place —
            // the one search a user makes when they are about to do something irreversible. Which
            // is also why the pane's row is `Kill pane` and not `Close pane`: it belongs in that
            // one search, and what it does to a running program is a kill by any honest name.
            Self::KillPane => "Kill pane".to_owned(),
            Self::KillWindow(name) => format!("Kill window {name}"),
            Self::KillSession(name) => format!("Kill session {name}"),
            // The project's own words. NOT prefixed with "Run" — a project titles its commands, and
            // wrapping them would make a fuzzy query match sprag's phrasing instead of theirs.
            Self::Declared(action) => action.title.clone(),
        }
    }

    /// What the row shows on its right — the SECOND column of a palette row, and two different
    /// facts share it because they answer the same question ("what will this actually do?").
    ///
    /// * For a built-in, the keyboard CHORD that runs it without the palette. A palette that lists a
    ///   command it shares with a chord should teach that chord, or it trains the user to keep
    ///   opening the palette for something one keystroke already does. (These strings are the DISPLAY
    ///   form of the bindings [`crate::input`] recognizes — there is no chord table to derive them
    ///   from, so a renamed binding must be renamed here too.)
    /// * For a PROJECT action, the COMMAND LINE it would run. That is not decoration: a project's
    ///   config arrives with a repository, so a row saying only "Run the tests" would be asking the
    ///   user to trust a label. Showing `cargo test` is what makes the offer honest.
    ///
    /// `None` for a command with neither.
    pub(crate) fn hint(&self) -> Option<String> {
        match self {
            Self::Find => Some("Ctrl+Shift+F".to_owned()),
            Self::Copy => Some("Ctrl+Shift+C".to_owned()),
            Self::Paste => Some("Ctrl+Shift+V".to_owned()),
            Self::ToggleFloat => Some("Ctrl+Shift+Enter".to_owned()),
            Self::LastSession => Some("Ctrl+Shift+L".to_owned()),
            Self::Declared(action) => Some(action.command_line()),
            Self::SelectAll
            | Self::BreakOut
            | Self::JoinInto(_)
            | Self::NewPane
            | Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_)
            // A kill deliberately advertises no chord: there is none, and this column is also where
            // the eye looks for one. What it WOULD say is on the confirmation prompt instead, which
            // is the only place a consequence can be read in time to change the outcome.
            | Self::KillPane
            | Self::KillWindow(_)
            | Self::KillSession(_) => None,
        }
    }

    /// What asking about this command must SAY, or `None` when it needs no asking.
    ///
    /// This is the single answer to "is this destructive?", and it lives here rather than on a
    /// surface for the reason the module docs give: a second surface must not be able to hold a
    /// second opinion about whether a command can be run without a question, and a surface added
    /// later must inherit the answer rather than re-decide it.
    ///
    /// The match is written out rather than defaulted with a `_` arm ON PURPOSE. A new command has
    /// to state that it is safe; the compiler asks. A wildcard would make "not destructive" the
    /// silent default, which is the one direction where forgetting is unrecoverable.
    ///
    /// Reads `slots`, because the honest prompt for a kill is not always the same sentence: killing
    /// a session's LAST window ends the session, and killing the ATTACHED session detaches this
    /// client. Those are the two facts that change what the user is actually agreeing to, and they
    /// are knowable only from live state. The caller CAPTURES the result at arm time (see
    /// [`confirm::arm`](crate::confirm::run_or_arm)) so the prompt cannot be re-derived out from
    /// under the person reading it — the same discipline the session rail's captured-name kill
    /// keeps, extended from the name to the whole sentence.
    /// Takes `target` for the same reason [`run`](Self::run) does, and it is not symmetry for its own
    /// sake: a destructive command addressed by the CAPTURED pane rather than by a name can only
    /// describe what it is about to destroy if it is told which pane that is. [`Self::KillPane`] is
    /// the case — its escalation (this is the window's last pane) is a fact about the target, and
    /// without the argument the prompt would have to fall back to a sentence that is true of every
    /// pane and therefore useful about none.
    pub(crate) fn confirmation(
        &self,
        target: Option<usize>,
        slots: &SlotView,
    ) -> Option<Confirmation> {
        match self {
            // Named by what it RUNS, not by an index: "pane 2" is a display slot the user never
            // chose and cannot see, whereas the program in it is what they are looking at. Falls
            // back to the bare question when the label is empty (a pane whose command is unknown),
            // rather than painting an empty quotation.
            Self::KillPane => Some(Confirmation {
                prompt: match target.map(|pane| slots.pane_command_label(pane)) {
                    Some(label) if !label.is_empty() => format!("Kill pane running '{label}'?"),
                    _ => "Kill this pane?".to_owned(),
                },
                // The two escalations compose, and the WINDOW one only escalates further when the
                // window is also the session's last — the same chain `KillWindow` states one link
                // further along, arrived at from the pane end.
                consequence: match (
                    slots.occupied_slots().len() <= 1,
                    slots.windows().len() <= 1,
                ) {
                    (true, true) => Some(
                        "It is this window's last pane and this session's last window.".to_owned(),
                    ),
                    (true, false) => Some("It is this window's last pane.".to_owned()),
                    (false, _) => None,
                },
                verb: KILL_VERB.to_owned(),
            }),
            Self::KillWindow(name) => Some(Confirmation {
                prompt: format!("Kill window '{name}'?"),
                // The escalation the name alone does not carry: `kill-window` on the last window is
                // `kill-session` by another route (`SlotView::kill_window` documents it).
                consequence: (slots.windows().len() <= 1).then(|| {
                    "It is this session's last window, so the session ends with it.".to_owned()
                }),
                verb: KILL_VERB.to_owned(),
            }),
            Self::KillSession(name) => Some(Confirmation {
                prompt: format!("Kill session '{name}'?"),
                consequence: (*name == slots.current_session())
                    .then(|| "It is the attached session, so this client detaches.".to_owned()),
                verb: KILL_VERB.to_owned(),
            }),
            Self::Find
            | Self::Copy
            | Self::Paste
            | Self::SelectAll
            | Self::ToggleFloat
            | Self::BreakOut
            | Self::JoinInto(_)
            | Self::NewPane
            | Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_)
            | Self::LastSession
            // A project command is NOT destructive as an activation: it is PASTED at the pane's
            // prompt without a newline, so the user's own Enter is already the confirmation, and
            // the line they are agreeing to is the one the row showed them.
            | Self::Declared(_) => None,
        }
    }

    /// Whether the thing this command NAMES is still there — asked of an already-armed command, so
    /// a prompt cannot linger over a window or session that has since gone.
    ///
    /// Only the commands that name a destroyable object can answer `false`. Everything else names
    /// nothing that can vanish between the question and the answer (`NewWindow` creates, `Copy` acts
    /// on a selection), so it is trivially still targetable.
    ///
    /// The residual this does NOT close is name REUSE, and it is inherent: a name is the only window
    /// / session identity on the wire, so a captured name whose bearer was killed and replaced reads
    /// as live. That is the same bound the session rail's own auto-disarm states, unchanged here.
    pub(crate) fn target_still_exists(&self, target: Option<usize>, slots: &SlotView) -> bool {
        match self {
            // The pane's own case is the one this check is STRONGEST for: a pane is the thing most
            // likely to vanish under an open prompt, because its child can simply exit. And unlike a
            // window or session name, a slot cannot be reused out from under the capture within a
            // frame — the reconcile that frees a slot is the same one this runs beside.
            Self::KillPane => target.is_some_and(|pane| slots.is_pane_occupied(pane)),
            Self::KillWindow(name) => slots.windows().iter().any(|window| &window.name == name),
            Self::KillSession(name) => slots.sessions().iter().any(|session| &session.name == name),
            Self::Find
            | Self::Copy
            | Self::Paste
            | Self::SelectAll
            | Self::ToggleFloat
            | Self::BreakOut
            | Self::JoinInto(_)
            | Self::NewPane
            | Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_)
            | Self::LastSession
            | Self::Declared(_) => true,
        }
    }

    /// Whether this command acts on a pane, and so is only OFFERED where one was captured.
    ///
    /// An OFFER predicate, not a precondition. It decides what [`catalog`] puts in front of the user;
    /// [`Command::run`] deliberately does not consult it, because the two are different questions and
    /// `Copy` is the proof — it is pointless to offer with no pane (there is nothing on screen to
    /// have selected) and yet perfectly runnable without one, since it copies whatever selection is
    /// active wherever it lives.
    ///
    /// Within offering, this is the ONLY gate. In particular neither `ToggleFloat` nor `BreakOut`
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
            | Self::BreakOut
            // A join needs the pane it MOVES, exactly as the break it inverts does.
            | Self::JoinInto(_)
            // A project command is DELIVERED to a pane's prompt, so it needs one as much as a paste
            // does — and the pane it needs is the one whose project declared it.
            | Self::Declared(_)
            // A kill of THE pane needs the pane, unlike the two name-addressed kills below it.
            | Self::KillPane => true,
            // Creating a pane needs no pane: it goes into the CURRENT WINDOW, which exists whether
            // or not anything in it holds focus — the same reason `NewWindow` beside it needs none.
            Self::NewPane
            | Self::NewWindow
            | Self::SelectWindow(_)
            | Self::NewSession
            | Self::SwitchSession(_)
            | Self::LastSession
            // A kill is addressed by the NAME it carries, like the select / switch rows above it —
            // what it destroys has nothing to do with which pane the user was looking at.
            | Self::KillWindow(_)
            | Self::KillSession(_) => false,
        }
    }

    /// Run the command against the pane the palette captured when it opened.
    ///
    /// PERFORMS, unconditionally — including a destructive command. The question is asked one level
    /// out, by [`confirm::run_or_arm`](crate::confirm::run_or_arm), which is what every surface calls;
    /// this is the performer it eventually reaches, and it has to be reachable with no further
    /// asking, because the confirmation surface is itself a caller. Guarding here as well would
    /// either be dead (the prompt already asked) or unsatisfiable (nothing left to ask with).
    ///
    /// Each arm drives the SAME authority the equivalent chord or button drives — the find bar's
    /// own `open`, the selection module's copy / paste, the dock's float toggle, the `SlotView`
    /// window and session actions the tab strip and rail use — so a command cannot mean one thing
    /// from the palette and another from the surface it already had. Nothing is re-implemented here.
    ///
    /// A `bool` an underlying call returns (a copy with no selection, a paste into a gone pane) is
    /// deliberately dropped: those are already the action's own tolerated no-ops, and neither surface
    /// has a place to report one that the surface itself does not.
    ///
    /// ## Why there is no `needs_pane` gate at the top
    ///
    /// There was one, as a belt to [`catalog`]'s braces, while the palette was the only caller and so
    /// the only way to reach `run` with a missing target was to build a command by hand. The menu
    /// builds them by hand — from [`menu_rows`], whose row set is deliberately not filtered by
    /// [`needs_pane`](Self::needs_pane) — so that gate would now be LIVE, and wrong: it would refuse
    /// `Copy`, which the menu has always run against the active selection whether or not a pane held
    /// focus at open time.
    ///
    /// Nothing is lost by removing it, because every arm below that needs a pane destructures
    /// `target` itself and is therefore already total over its absence. The gate was never the guard
    /// — the arms were.
    pub(crate) fn run(&self, target: Option<usize>, slots: &SlotView) {
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
            Self::JoinInto(name) => {
                if let Some(pane) = target {
                    slots.join_pane(pane, name);
                }
            }
            Self::NewPane => {
                // No target: the pane joins the CURRENT WINDOW, and the arrangement places it (the
                // layout reconciles against the live pane set, so nothing here says where).
                slots.new_pane();
            }
            Self::KillPane => {
                if let Some(pane) = target {
                    slots.close_pane(pane);
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
            // Addressed by NAME, so what is killed is what the prompt named — never a row index
            // re-resolved against a list that moved in the meantime. Killing the last window ends
            // the session and killing the attached session detaches this client; both are stated on
            // the prompt (see [`Command::confirmation`]) rather than refused here.
            Self::KillWindow(name) => slots.kill_window(name),
            Self::KillSession(name) => slots.kill_session(name),
            // PASTED at the pane's prompt, without a trailing newline: the user presses Enter. The
            // whole rationale (their shell runs it, so output/history/Ctrl-C behave; and a command
            // named by a file in a repository must not execute on a repository's say-so) lives on
            // `ProjectAction::command_line`. Bracketed paste keeps the whole line one inert unit.
            Self::Declared(action) => {
                if let Some(pane) = target {
                    let _ = slots.paste(pane, &action.command_line());
                }
            }
        }
    }
}

/// The imperative on the affirmative button of a kill's prompt. tmux's own word for the operation,
/// and the same one the session rail's confirm strip already paints — a user who has confirmed a kill
/// once should read the identical word the second time, from whichever surface asked.
const KILL_VERB: &str = "Kill";

/// What a destructive command's prompt must say — the whole sentence, owned by the command.
///
/// A value rather than three methods, and captured rather than re-read, because these strings are
/// what the user is agreeing to: the surface that paints them must not be able to compose its own
/// question, and the sentence must not change between being read and being answered.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Confirmation {
    /// The question, naming exactly what is about to be destroyed (`Kill session 'work'?`).
    pub(crate) prompt: String,
    /// The consequence the name does not already imply — killing a session's last window, or the
    /// session this client is attached to. `None` when the prompt says everything.
    pub(crate) consequence: Option<String>,
    /// The affirmative button's word: the destructive act in the imperative ([`KILL_VERB`]), never a
    /// bare "OK". A button that names the act cannot be clicked past without reading it.
    pub(crate) verb: String,
}

/// The cap on the rows named after a window — `Go to window <name>` and `Move pane to window <name>`
/// alike — matching the tab strip's own practical ceiling
/// ([`MAX_WINDOW_TABS`](crate::wtabs::MAX_WINDOW_TABS)) so a surface offers exactly the windows the
/// strip can show. A session with more windows than this reaches the overflow through `sprag
/// select-window` / `sprag join-pane`, the same honest bound the strip states.
const MAX_WINDOW_ROWS: usize = crate::wtabs::MAX_WINDOW_TABS;

/// The cap on `Switch to session <name>` rows, for the same reason, against the session rail.
const MAX_SESSION_ROWS: usize = 16;

/// The cap on rows one PROJECT may contribute. The host already refuses a config declaring more than
/// its own cap, so this is the client's independent bound on what it will paint — the file is
/// untrusted input, and a client should not depend on the other end having checked.
const MAX_PROJECT_ROWS: usize = sprag_host::project::MAX_ACTIONS;

/// Every command offered RIGHT NOW, in the order a palette with an empty query lists them.
///
/// Built from live state (the window and session lists) and from `target`, so it is a snapshot:
/// the palette FREEZES the result when it opens and filters that frozen list as the query changes.
/// This is the context menu's rule, and it is load-bearing for the same reason — a list rebuilt
/// under an open palette could move a row between the frame the user read and the `Enter` that
/// runs it, so an activation would run a neighbour of the command they chose.
///
/// Order is by kind, MOST SPECIFIC FIRST: the target pane's PROJECT commands, then the pane
/// commands (which act on what the user is looking at), then the window commands, then the session
/// ones, and LAST the destructive ones (see the note beside them for why last is right for those and
/// wrong for everything else). Within a kind, declaration order — the project's own file order for
/// its own commands.
///
/// The project going first is not a preference. The palette paints a bounded number of rows
/// ([`MAX_VISIBLE_ROWS`](crate::palette)), so whatever sits at the END of an unfiltered list is
/// INVISIBLE until something is typed — and the rows that must not be invisible are the ones a
/// project deliberately declared, which exist nowhere else and which nothing else can reach. A
/// built-in pushed past the cut still has its chord and its menu. (Learned from the live screenshot:
/// with the project last, a config's second command could not be seen at all.)
///
/// The fuzzy ranking takes over the moment anything is typed, so this order only governs the
/// just-opened palette.
pub(crate) fn catalog(target: Option<usize>, slots: &SlotView) -> Catalog {
    // The target pane's PROJECT commands first (see the fn docs). The read is the reason `catalog`
    // is only ever called at OPEN time: it costs the host a filesystem walk (and, off a wire client,
    // a socket round trip).
    let mut errors: Vec<String> = Vec::new();
    let mut out: Vec<Command> = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    if let Some(pane) = target {
        match slots.project(pane) {
            None => {}
            Some(Ok(project)) => {
                for action in project.actions.into_iter().take(MAX_PROJECT_ROWS) {
                    declared.push(action.name.clone());
                    out.push(Command::Declared(action));
                }
            }
            // A project whose config is unusable contributes NO rows and one message. Reporting it
            // is the point: a client that showed an empty list would leave the config's author
            // believing their file works (`sprag_host::project` carries the whole rationale).
            Some(Err(error)) => errors.push(error.to_string()),
        }
    }
    // ...then the USER's own commands, which are offered wherever the pane is — including in no
    // project at all, the case the block above cannot serve.
    //
    // SECOND, and shadowed by name: a project's command wins over a global one it collides with,
    // which is the same nearest-wins rule that makes an inner `.sprag.toml` beat an outer one. A
    // name is an address (`sprag run <name>`), so two rows answering to one name would be a palette
    // the user cannot read and a CLI call nobody can resolve.
    match slots.global_commands() {
        None => {}
        Some(Ok(config)) => out.extend(
            config
                .commands
                .into_iter()
                .filter(|action| !declared.contains(&action.name))
                .take(MAX_PROJECT_ROWS)
                .map(Command::Declared),
        ),
        // Reported the same way, and SEPARATELY: both configs can be broken at once, and each
        // message names its own file, so the user learns which to open.
        Some(Err(message)) => errors.push(message),
    }
    out.extend([
        Command::Find,
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::ToggleFloat,
        Command::BreakOut,
        // Between the pane rows and the window ones because that is what it is: a pane command that
        // needs no pane. It sits AFTER the rows that act on the pane you are looking at and BEFORE
        // the ones that leave it.
        Command::NewPane,
        Command::NewWindow,
    ]);
    // A pane command with no pane to act on is not offered at all: a row that is guaranteed to do
    // nothing is worse than a shorter list, because the user cannot tell the two apart. (A project
    // command is only ever added WITH a target above, so this cannot strip one.)
    out.retain(|command| !command.needs_pane() || target.is_some());

    // One row per OTHER window: going to the window you are already in is not an action.
    let windows = slots.windows();
    let elsewhere: Vec<&str> = windows
        .iter()
        .filter(|window| !window.current)
        .take(MAX_WINDOW_ROWS)
        .map(|window| window.name.as_str())
        .collect();
    out.extend(
        elsewhere
            .iter()
            .map(|name| Command::SelectWindow((*name).to_owned())),
    );
    // ...and, where a pane was captured, one row per window to MOVE it into. The SAME list serves
    // both: a join's destination is any window except the one the pane already lives in, which is
    // the current one (a slot addresses a pane of the current window), and the host refuses a join
    // into the window the pane is already in anyway.
    if target.is_some() {
        out.extend(
            elsewhere
                .iter()
                .map(|name| Command::JoinInto((*name).to_owned())),
        );
    }

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

    // ...and LAST, the DESTRUCTIVE rows — one kill per window and per session.
    //
    // Two differences from every row above, both deliberate:
    //
    // Their targets INCLUDE the current window and the attached session, where `Go to window` and
    // `Switch to session` exclude them. "Go where you already are" is not an action; "kill what you
    // are looking at" is the commonest kill there is, and both existing "×" affordances do exactly
    // that. Excluding them would have made the palette the one surface that cannot close the window
    // in front of you.
    //
    // And being pushed past the visible cut is, here alone, a FEATURE. The project rows lead this
    // list because nothing else can reach them; these trail it because everything about them should
    // be deliberate — a palette opened by accident must not have `Kill session 0` under the cursor.
    // Typing "kill" collects them all (see [`Command::title`]), which is the only way they are meant
    // to be found, and the confirmation is still asked afterwards regardless.
    // Narrowest first WITHIN the tail, mirroring the catalog's own most-specific-first rule: the one
    // pane, then its window, then the session. Added HERE rather than in the block above — where its
    // `needs_pane` would have been honoured by the `retain` — because a destructive row belongs past
    // the cut with its siblings, so the guard is spelled out instead.
    if target.is_some() {
        out.push(Command::KillPane);
    }
    out.extend(
        windows
            .iter()
            .take(MAX_WINDOW_ROWS)
            .map(|window| Command::KillWindow(window.name.clone())),
    );
    out.extend(
        sessions
            .iter()
            .take(MAX_SESSION_ROWS)
            .map(|session| Command::KillSession(session.name.clone())),
    );
    Catalog {
        commands: out,
        config_errors: errors,
    }
}

/// What one [`catalog`] call answers: the commands to offer, and any report about the project that
/// could not contribute to them.
///
/// A struct rather than a bare `Vec` because the error is NOT a command — it is something to SHOW,
/// and folding it into the list as an unrunnable row would put a thing that cannot be run where
/// every other row can be.
pub(crate) struct Catalog {
    /// The commands to offer, in the order an empty query lists them.
    pub(crate) commands: Vec<Command>,
    /// Why a config contributed nothing, one message per broken file (the pane's project, the
    /// user's own, or both). A `Vec` rather than an `Option` because the two are independent: one
    /// being broken says nothing about the other, and a user with two problems needs to see two.
    pub(crate) config_errors: Vec<String>,
}

/// One row of the pane context menu: the command it runs, and the SHORT wording it paints with.
///
/// The label is DATA here rather than a method on [`Command`] for the reason the module docs give.
/// Only a handful of commands are ever offered in a menu, so a `menu_label()` on the enum would be
/// mostly arms returning nothing, and the wording is the MENU's editorial decision — it belongs where
/// the menu's row set is decided, which is [`menu_rows`]. Pairing the two in one value preserves the
/// property the menu already depended on: [`crate::ctxmenu`] paints `label` and its reducer runs
/// `command` out of the SAME captured row, so an activation cannot run a neighbour of what was read.
/// (Serde-derived for the same reason [`Command`] is — the captured list lives in a reactive
/// `Signal`, whose value type carries pinion's serialization bound.)
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MenuRow {
    /// What activating this row does — the shared definition, performed by [`Command::run`].
    pub(crate) command: Command,
    /// What the row paints: the anchored, object-dropping phrasing (`Copy`, not `Copy selection`).
    pub(crate) label: String,
}

impl MenuRow {
    /// A row pairing `command` with the wording the menu paints for it.
    fn new(command: Command, label: &str) -> Self {
        Self {
            command,
            label: label.to_owned(),
        }
    }
}

/// The rows the menu offers regardless of live state, and so the floor of every menu.
const FIXED_MENU_ROWS: usize = 4;

/// The MOST rows [`menu_rows`] can ever return — the fixed rows plus one join target per window the
/// cap allows.
///
/// Stated here, beside the builder, because the menu's `ContextMenuExternal` is registered ONCE at
/// this capacity (pinion preserves the live handle by tag across the reconcile, so a per-open count
/// would discard it). Re-deriving the same arithmetic at the registration site is exactly the kind of
/// second definition this module exists to remove.
pub(crate) const MAX_MENU_ROWS: usize = FIXED_MENU_ROWS + MAX_WINDOW_ROWS;

/// The rows the pane context menu offers RIGHT NOW, in the order it paints them.
///
/// A snapshot, frozen when the menu opens, for the reason [`catalog`] is frozen when the palette
/// opens: the join targets depend on the live window list, which a second client or an agent can
/// change under an open popup.
///
/// Unlike [`catalog`] this takes no target and filters nothing — see the module docs on the row
/// policy. The join targets are the same "every window but the current one" list, capped the same
/// way ([`MAX_WINDOW_ROWS`]), that the palette's own `Move pane to window` rows are built from.
pub(crate) fn menu_rows(slots: &SlotView) -> Vec<MenuRow> {
    let mut rows = vec![
        MenuRow::new(Command::Copy, "Copy"),
        MenuRow::new(Command::Paste, "Paste"),
        MenuRow::new(Command::SelectAll, "Select all"),
        MenuRow::new(Command::BreakOut, "Break out"),
    ];
    for window in slots
        .windows()
        .into_iter()
        .filter(|window| !window.current)
        .take(MAX_WINDOW_ROWS)
    {
        let label = format!("Move to {}", window.name);
        rows.push(MenuRow::new(Command::JoinInto(window.name), &label));
    }
    rows
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
        /// `(pane, destination window)` per join — `broken_panes`' inverse.
        joined: Vec<(PaneId, String)>,
        /// How many panes were created (tmux `split-window`).
        new_panes: usize,
        /// The panes a kill removed — recorded, not no-op'd, for the reason the killed WINDOWS are:
        /// a mis-addressed destructive command cannot be walked back.
        killed_panes: Vec<PaneId>,
        last_session: usize,
        /// `(pane, text)` per paste — how a project command reaches a pane.
        pasted: Vec<(PaneId, String)>,
        /// The windows a kill named. Recorded rather than no-op'd because the destructive routing is
        /// the one place a mis-addressed command cannot be walked back.
        killed_windows: Vec<String>,
        /// The sessions a kill named. The in-process `Host` deliberately no-ops `kill_session` (it
        /// renders only the default session), so a recording fake is the ONLY way to observe that this
        /// arm addresses the right one at all.
        killed_sessions: Vec<String>,
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
        /// What this host answers for a pane's project — the three outcomes the real host has.
        project: Option<Result<sprag_host::Project, sprag_host::ProjectError>>,
        /// What this host answers for the USER's config — the same three outcomes, independently.
        global: Option<Result<sprag_host::UserConfig, String>>,
        /// The live pane set. Ids are deliberately NOT their slot numbers, so a test asserting on a
        /// recorded id proves the slot→id mapping was applied rather than an accidental identity.
        panes: Vec<PaneId>,
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
        fn kill_window(&self, name: &str) {
            self.log.borrow_mut().killed_windows.push(name.to_owned());
        }
        fn break_pane(&self, id: PaneId, _name: Option<&str>) -> Option<String> {
            self.log.borrow_mut().broken_panes.push(id);
            Some("w".to_owned())
        }
        fn join_pane(&self, id: PaneId, dst: &str) -> Option<bool> {
            self.log.borrow_mut().joined.push((id, dst.to_owned()));
            Some(true)
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
        fn kill_session(&self, name: &str) {
            self.log.borrow_mut().killed_sessions.push(name.to_owned());
        }
        fn switch_to_last_session(&self) {
            self.log.borrow_mut().last_session += 1;
        }
        fn project(
            &self,
            _id: PaneId,
        ) -> Option<Result<sprag_host::Project, sprag_host::ProjectError>> {
            self.project.clone()
        }
        fn global_commands(&self) -> Option<Result<sprag_host::UserConfig, String>> {
            self.global.clone()
        }
        fn paste(&self, id: PaneId, text: &str) -> bool {
            self.log.borrow_mut().pasted.push((id, text.to_owned()));
            true
        }
        fn pane_ids(&self) -> Vec<PaneId> {
            self.panes.clone()
        }
        fn new_pane(&self) -> Option<PaneId> {
            self.log.borrow_mut().new_panes += 1;
            Some(PaneId(99))
        }
        fn kill_pane(&self, id: PaneId) -> bool {
            self.log.borrow_mut().killed_panes.push(id);
            true
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
        slots_with_panes(windows, sessions, current, 1)
    }

    /// The same, over `panes` live panes — the dimension a pane-addressed destructive command reads
    /// (killing the LAST pane of a window says something extra), so a test can drive both branches.
    fn slots_with_panes(
        windows: &[(&str, bool)],
        sessions: &[&str],
        current: &str,
        panes: usize,
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
            project: None,
            global: None,
            // Offset so no id equals its slot (see the field's own note).
            panes: (0..panes).map(|i| PaneId(7 + i as u64)).collect(),
        };
        let slots = SlotView::new(Box::new(host));
        slots.reconcile();
        (slots, log)
    }

    /// A `SlotView` whose host answers `project` with `answer`.
    fn slots_with_project(
        answer: Option<Result<sprag_host::Project, sprag_host::ProjectError>>,
    ) -> (SlotView, Rc<RefCell<Log>>) {
        let log: Rc<RefCell<Log>> = Rc::default();
        let host = CatalogHost {
            windows: vec![WindowInfo {
                name: "main".to_owned(),
                current: true,
            }],
            sessions: vec!["0".to_owned()],
            current: "0".to_owned(),
            log: Rc::clone(&log),
            project: answer,
            global: None,
            panes: vec![PaneId(7)],
        };
        let slots = SlotView::new(Box::new(host));
        slots.reconcile();
        (slots, log)
    }

    /// A `SlotView` over a host answering `project` for the pane AND `global` for the user, so a
    /// test can drive the two config sources independently — which is the whole point of their being
    /// two.
    fn slots_with_configs(
        project: Option<Result<sprag_host::Project, sprag_host::ProjectError>>,
        global: Option<Result<sprag_host::UserConfig, String>>,
    ) -> SlotView {
        let host = CatalogHost {
            windows: vec![WindowInfo {
                name: "main".to_owned(),
                current: true,
            }],
            sessions: vec!["0".to_owned()],
            current: "0".to_owned(),
            log: Rc::default(),
            project,
            global,
            panes: vec![PaneId(7)],
        };
        let slots = SlotView::new(Box::new(host));
        slots.reconcile();
        slots
    }

    /// A user config declaring `names`, each running a program of the same name.
    fn user_config(names: &[&str]) -> sprag_host::UserConfig {
        sprag_host::UserConfig {
            path: std::path::PathBuf::from("/home/u/.config/sprag/config.toml"),
            commands: names
                .iter()
                .map(|name| sprag_host::ProjectAction {
                    name: (*name).to_owned(),
                    title: format!("User: {name}"),
                    run: vec![(*name).to_owned()],
                })
                .collect(),
        }
    }

    /// The user's own commands are offered in EVERY pane — including one in no project at all, which
    /// is exactly the case a project config cannot serve — and they sit after the project's rows but
    /// before the built-ins, since neither has a chord to be reached by once past the visible cut.
    ///
    /// REVERT-PROOF: drop the `global_commands` block and the rows vanish; move it after the
    /// built-ins and the ordering assertion fails.
    #[test]
    fn the_users_own_commands_are_offered_wherever_the_pane_is() {
        let slots = slots_with_configs(None, Some(Ok(user_config(&["top"]))));

        let commands = catalog(Some(0), &slots).commands;
        let user = commands
            .iter()
            .position(|c| matches!(c, Command::Declared(a) if a.name == "top"))
            .expect("the user's command is offered with no project in sight");
        let built_in = commands
            .iter()
            .position(|c| *c == Command::Find)
            .expect("the built-ins are there too");
        assert!(
            user < built_in,
            "a user command has no chord, so it must not be the row pushed past the cut"
        );

        // It still NEEDS a pane, though — for the same reason a project command does, and this is
        // the one thing the two share that the wording "available everywhere" could mislead about: a
        // declared command is DELIVERED by pasting at a prompt, so with no pane captured there is
        // nowhere to deliver it and the row is not offered at all.
        assert!(
            !catalog(None, &slots)
                .commands
                .iter()
                .any(|c| matches!(c, Command::Declared(_))),
            "a declared command with no pane to paste into is not a row"
        );
    }

    /// A project's command SHADOWS a user command of the same name — the nearest-wins rule that
    /// already makes an inner `.sprag.toml` beat an outer one, applied one level further out.
    ///
    /// REVERT-PROOF: drop the `declared.contains` filter and `test` is offered twice, which is a
    /// palette the user cannot read and a `sprag run test` nobody can resolve.
    #[test]
    fn a_project_command_shadows_the_users_command_of_the_same_name() {
        let slots = slots_with_configs(
            Some(Ok(one_action_project())), // declares "test", titled "Run the suite"
            Some(Ok(user_config(&["test", "top"]))),
        );

        let commands = catalog(Some(0), &slots).commands;
        let declared: Vec<&sprag_host::ProjectAction> = commands
            .iter()
            .filter_map(|c| match c {
                Command::Declared(action) => Some(action),
                _ => None,
            })
            .collect();

        let named_test: Vec<&&sprag_host::ProjectAction> =
            declared.iter().filter(|a| a.name == "test").collect();
        assert_eq!(named_test.len(), 1, "a name addresses exactly one command");
        assert_eq!(
            named_test[0].title, "Run the suite",
            "and the PROJECT's is the one that survives — the nearer config wins"
        );
        assert!(
            declared.iter().any(|a| a.name == "top"),
            "a user command the project does not shadow is still offered"
        );
    }

    /// Both configs can be broken at once, and each is reported SEPARATELY — a user with two
    /// problems must see two, each naming the file to open.
    ///
    /// REVERT-PROOF: collapse the reports back into one `Option` and the second is lost, sending the
    /// user to fix one file while the other stays broken and unexplained.
    #[test]
    fn a_broken_project_and_a_broken_user_config_are_reported_independently() {
        let slots = slots_with_configs(
            Some(Err(sprag_host::ProjectError::Malformed(
                "expected `]` at line 3".to_owned(),
            ))),
            Some(Err(
                "config.toml: the command \"a\" has an empty `run`".to_owned()
            )),
        );

        let built = catalog(Some(0), &slots);
        assert!(
            !built
                .commands
                .iter()
                .any(|c| matches!(c, Command::Declared(_))),
            "neither broken config offers anything to run"
        );
        assert_eq!(built.config_errors.len(), 2, "{:?}", built.config_errors);
        assert!(
            built
                .config_errors
                .iter()
                .any(|e| e.contains("expected `]`")),
            "the project's parser message survives: {:?}",
            built.config_errors
        );
        assert!(
            built
                .config_errors
                .iter()
                .any(|e| e.contains("config.toml") && e.contains("empty `run`")),
            "and the user config's report names ITS file: {:?}",
            built.config_errors
        );
    }

    /// One declared command, as the host would answer it.
    fn one_action_project() -> sprag_host::Project {
        sprag_host::Project {
            root: std::path::PathBuf::from("/tmp/demo"),
            actions: vec![sprag_host::ProjectAction {
                name: "test".to_owned(),
                title: "Run the suite".to_owned(),
                run: vec!["cargo".to_owned(), "test".to_owned()],
            }],
        }
    }

    /// A project's declared commands join the catalog, titled in the project's OWN words, and each
    /// row shows the command line it would run — the "show it before you run it" rule.
    ///
    /// REVERT-PROOF: drop the project arm from `catalog` and no row carries the title; drop
    /// `hint()`'s `Project` arm and the command line stops being shown.
    #[test]
    fn a_projects_commands_join_the_catalog_showing_what_they_would_run() {
        let (slots, _log) = slots_with_project(Some(Ok(one_action_project())));
        let built = catalog(Some(0), &slots);

        let action = built
            .commands
            .iter()
            .find(|command| matches!(command, Command::Declared(_)))
            .expect("the project's command is offered");
        assert_eq!(action.title(), "Run the suite", "the project's own title");
        assert_eq!(
            action.hint().as_deref(),
            Some("cargo test"),
            "the row shows the command line, so the offer is not a label to be trusted blindly"
        );
        assert!(built.config_errors.is_empty());
    }

    /// A project's commands come FIRST in an unfiltered catalog — the palette paints a bounded
    /// number of rows, so the ones that exist nowhere else must not be the ones pushed past the cut.
    ///
    /// REVERT-PROOF: move the project block back after the built-ins and this fails.
    #[test]
    fn a_projects_commands_lead_the_unfiltered_catalog() {
        let (slots, _log) = slots_with_project(Some(Ok(one_action_project())));
        let built = catalog(Some(0), &slots);
        assert!(
            matches!(built.commands.first(), Some(Command::Declared(_))),
            "the project's own commands lead: {:?}",
            built.commands.first()
        );
    }

    /// A project command is DELIVERED to the pane's prompt as a pasted line, with NO trailing
    /// newline — the user still presses Enter.
    ///
    /// REVERT-PROOF: make the `Project` arm of `run` send text with a `\n` and the assertion on the
    /// exact payload fails; drop the arm and nothing is pasted at all.
    #[test]
    fn running_a_project_command_pastes_the_line_without_executing_it() {
        let (slots, log) = slots_with_project(Some(Ok(one_action_project())));
        let built = catalog(Some(0), &slots);
        let action = built
            .commands
            .into_iter()
            .find(|command| matches!(command, Command::Declared(_)))
            .expect("the project's command is offered");

        action.run(Some(0), &slots);

        let pasted = log.borrow().pasted.clone();
        assert_eq!(
            pasted,
            vec![(PaneId(7), "cargo test".to_owned())],
            "the line reaches the captured pane verbatim and WITHOUT a newline: {pasted:?}"
        );
    }

    /// A project whose config is broken contributes NO rows and one report — never an empty list
    /// that would leave the config's author thinking it worked.
    ///
    /// REVERT-PROOF: fold the error into the command list (or drop it) and this fails.
    #[test]
    fn a_broken_project_config_contributes_a_report_and_no_rows() {
        let (slots, _log) = slots_with_project(Some(Err(sprag_host::ProjectError::Malformed(
            "expected `]` at line 3".to_owned(),
        ))));
        let built = catalog(Some(0), &slots);

        assert!(
            !built
                .commands
                .iter()
                .any(|command| matches!(command, Command::Declared(_))),
            "a broken config offers nothing to run"
        );
        assert_eq!(built.config_errors.len(), 1, "...but it does report why");
        assert!(
            built.config_errors[0].contains("expected `]`"),
            "the report carries the parser's message: {:?}",
            built.config_errors
        );
    }

    #[test]
    fn a_pane_command_is_not_offered_without_a_pane_to_act_on() {
        // Built twice over the SAME host state, differing only in whether a pane was captured — so
        // the delta is exactly the pane commands.
        let (slots, _log) = slots_with(&[("0", true)], &["0"], "0");
        let with_pane = catalog(Some(0), &slots).commands;
        let without = catalog(None, &slots).commands;

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
            .commands
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

        let built = catalog(Some(0), &slots).commands;
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
        Command::JoinInto("build".to_owned()).run(Some(0), &slots);
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
        assert_eq!(
            log.joined,
            vec![(PaneId(7), "build".to_owned())],
            "a join carries BOTH the captured pane and the window it names"
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

    /// The palette offers `Move pane to window <name>` per other window — but only where a pane was
    /// captured, since a join has nothing to move otherwise.
    ///
    /// REVERT-PROOF: drop the `JoinInto` block from `catalog` and the first assertion fails; drop its
    /// `target.is_some()` guard and the last one does.
    #[test]
    fn a_move_row_is_offered_per_other_window_and_only_with_a_captured_pane() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0"], "0");

        let titles: Vec<String> = catalog(Some(0), &slots)
            .commands
            .iter()
            .map(Command::title)
            .collect();
        assert!(titles.contains(&"Move pane to window build".to_owned()));
        assert!(
            !titles.contains(&"Move pane to window main".to_owned()),
            "the pane already lives in the current window"
        );

        assert!(
            !catalog(None, &slots)
                .commands
                .iter()
                .any(|command| matches!(command, Command::JoinInto(_))),
            "with no pane captured there is nothing to move"
        );
    }

    /// THE point of this module: every row the context menu offers runs a command the palette also
    /// knows. The two surfaces hold no separate definition of what an action does.
    ///
    /// Asserted with a pane captured, because that is the only state in which the palette offers the
    /// pane commands at all (see the row-policy test below).
    ///
    /// REVERT-PROOF: give the menu back an action of its own — anything `catalog` does not build —
    /// and this fails.
    #[test]
    fn every_menu_row_runs_a_command_the_palette_also_offers() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0"], "0");
        let offered = catalog(Some(0), &slots).commands;

        let rows = menu_rows(&slots);
        assert!(!rows.is_empty(), "the menu offers something to compare");
        for row in &rows {
            assert!(
                offered.contains(&row.command),
                "the menu's {:?} row is not a command the palette offers: {offered:?}",
                row.command
            );
        }
    }

    /// The wording is deliberately NOT shared: a menu row is read anchored on a pane, so it drops the
    /// object the palette's row has to carry. Every shared row is strictly shorter in the menu.
    ///
    /// REVERT-PROOF: paint the menu with `Command::title` (the "simplification" this test exists to
    /// refuse) and every assertion here fails at once.
    #[test]
    fn the_menu_words_a_shared_command_shorter_than_the_palette_does() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0"], "0");

        for row in menu_rows(&slots) {
            let title = row.command.title();
            assert!(
                row.label.len() < title.len(),
                "the anchored wording must be the shorter one: {:?} vs {title:?}",
                row.label
            );
        }

        // The two exemplars, spelled out, so this test also documents the phrasings it protects.
        let rows = menu_rows(&slots);
        let copy = rows
            .iter()
            .find(|row| row.command == Command::Copy)
            .expect("the menu offers copy");
        assert_eq!(copy.label, "Copy");
        assert_eq!(copy.command.title(), "Copy selection");
        let break_out = rows
            .iter()
            .find(|row| row.command == Command::BreakOut)
            .expect("the menu offers break-out");
        assert_eq!(break_out.label, "Break out");
        assert_eq!(
            break_out.command.title(),
            "Break pane out to a new window",
            "a palette row is read out of context, so it names the whole gesture"
        );
    }

    /// The other surviving difference: the palette hides a row it cannot run, the menu keeps its
    /// fixed rows regardless.
    ///
    /// `Copy` is the case that matters, and the reason [`Command::run`] carries no `needs_pane` gate:
    /// the palette will not offer it without a pane (nothing on screen to have selected), while the
    /// menu offers it always — so a gate keyed on the OFFER predicate would refuse a row the menu
    /// legitimately paints and has always run.
    #[test]
    fn the_menu_keeps_its_fixed_rows_where_the_palette_hides_a_row_it_cannot_run() {
        let (slots, _log) = slots_with(&[("main", true)], &["0"], "0");

        assert!(
            !catalog(None, &slots).commands.contains(&Command::Copy),
            "with no pane captured the palette does not offer a pane command"
        );
        assert!(
            menu_rows(&slots)
                .iter()
                .any(|row| row.command == Command::Copy),
            "the menu offers copy whether or not a pane held focus when it opened"
        );
    }

    #[test]
    fn the_menu_rows_stay_within_the_capacity_the_external_is_registered_at() {
        // A single window contributes no join target, so the menu is exactly its fixed rows...
        let (single, _log) = slots_with(&[("main", true)], &["0"], "0");
        assert_eq!(menu_rows(&single).len(), FIXED_MENU_ROWS);

        // ...and one more window than the cap allows saturates it without exceeding MAX_MENU_ROWS,
        // which is the count the `ContextMenuExternal` is registered at — a row past it could not be
        // painted, and its click index could not be resolved.
        let windows: Vec<(String, bool)> = (0..=MAX_WINDOW_ROWS + 1)
            .map(|i| (format!("w{i}"), i == 0))
            .collect();
        let window_refs: Vec<(&str, bool)> = windows
            .iter()
            .map(|(name, current)| (name.as_str(), *current))
            .collect();
        let (saturated, _log) = slots_with(&window_refs, &["0"], "0");
        assert_eq!(menu_rows(&saturated).len(), MAX_MENU_ROWS);
    }

    /// A kill reaches the host addressed by the NAME it carries — the whole reason the destructive
    /// variants hold a `String` instead of an index.
    ///
    /// REVERT-PROOF: swap the two kill arms of [`Command::run`] and each assertion below catches it;
    /// drop either arm and its list is empty.
    #[test]
    fn a_kill_addresses_the_window_or_session_it_names() {
        let (slots, log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");

        Command::KillWindow("build".to_owned()).run(Some(0), &slots);
        Command::KillSession("work".to_owned()).run(Some(0), &slots);

        let log = log.borrow();
        assert_eq!(
            log.killed_windows,
            vec!["build".to_owned()],
            "the window kill names the window, and only it"
        );
        assert_eq!(
            log.killed_sessions,
            vec!["work".to_owned()],
            "the session kill names the session, and only it"
        );
    }

    /// The catalog offers a kill for EVERY window and session, including the current window and the
    /// attached session — unlike the `Go to` / `Switch to` rows, which exclude them.
    ///
    /// "Kill what I am looking at" is the commonest kill there is, and both existing "×" affordances do
    /// exactly that; excluding it would leave the palette the one surface unable to close the window in
    /// front of you.
    ///
    /// REVERT-PROOF: build the kill rows from the `elsewhere` list (the one the select rows use) and
    /// the two current/attached assertions fail.
    #[test]
    fn the_catalog_offers_a_kill_for_every_window_and_session_including_the_current_one() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");
        let titles: Vec<String> = catalog(Some(0), &slots)
            .commands
            .iter()
            .map(Command::title)
            .collect();

        assert!(
            titles.contains(&"Kill window main".to_owned()),
            "{titles:?}"
        );
        assert!(titles.contains(&"Kill window build".to_owned()));
        assert!(titles.contains(&"Kill session 0".to_owned()));
        assert!(titles.contains(&"Kill session work".to_owned()));
    }

    /// The destructive rows come LAST, so a palette opened by accident never has one under the cursor.
    ///
    /// REVERT-PROOF: move the kill block anywhere above the session rows and this fails.
    #[test]
    fn the_destructive_rows_trail_the_unfiltered_catalog() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");
        let commands = catalog(Some(0), &slots).commands;

        let first_kill = commands
            .iter()
            .position(|command| command.confirmation(Some(0), &slots).is_some())
            .expect("the catalog offers destructive commands");
        let last_safe = commands
            .iter()
            .rposition(|command| command.confirmation(Some(0), &slots).is_none())
            .expect("...and safe ones");
        assert!(
            first_kill > last_safe,
            "every destructive row sits after every safe one: kill at {first_kill}, safe at {last_safe}"
        );
        assert!(
            !matches!(commands.first(), Some(command) if command.confirmation(Some(0), &slots).is_some()),
            "and never at the cursor's opening position"
        );
    }

    /// Exactly the kills need asking about, and each carries the words for it — the answer no surface
    /// is allowed to hold a second opinion about.
    ///
    /// REVERT-PROOF: return `None` from either kill arm of [`Command::confirmation`] and this fails;
    /// return `Some` from any other and it fails too.
    #[test]
    fn a_confirmation_is_carried_by_exactly_the_destructive_commands() {
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");

        for command in catalog(Some(0), &slots).commands {
            let destructive = matches!(
                command,
                Command::KillPane | Command::KillWindow(_) | Command::KillSession(_)
            );
            let asks = command.confirmation(Some(0), &slots);
            assert_eq!(
                asks.is_some(),
                destructive,
                "{command:?} asks: {}, destructive: {destructive}",
                asks.is_some()
            );
            if let Some(words) = asks {
                assert!(
                    words.prompt.contains('?'),
                    "the prompt is a QUESTION: {:?}",
                    words.prompt
                );
                assert_eq!(words.verb, "Kill", "the button names the act");
            }
        }
    }

    /// The consequence line appears exactly when the name does not already imply it: the session's last
    /// window, and the attached session.
    ///
    /// REVERT-PROOF: drop either `consequence` condition and its assertion here fails.
    #[test]
    fn the_prompt_states_the_escalation_only_when_there_is_one() {
        // One window: killing it ends the session. Attached session "0": killing it detaches.
        let (single, _log) = slots_with(&[("main", true)], &["0", "work"], "0");
        let last_window = Command::KillWindow("main".to_owned())
            .confirmation(Some(0), &single)
            .expect("a kill asks");
        assert!(
            last_window
                .consequence
                .as_deref()
                .is_some_and(|line| line.contains("session")),
            "the last window's prompt names the escalation: {last_window:?}"
        );
        let attached = Command::KillSession("0".to_owned())
            .confirmation(Some(0), &single)
            .expect("a kill asks");
        assert!(
            attached
                .consequence
                .as_deref()
                .is_some_and(|line| line.contains("detach")),
            "the attached session's prompt says the client detaches: {attached:?}"
        );

        // Two windows, and a session this client is NOT on: nothing extra to say.
        let (several, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");
        assert!(
            Command::KillWindow("build".to_owned())
                .confirmation(Some(0), &several)
                .expect("a kill asks")
                .consequence
                .is_none(),
            "a window that is not the last one carries no extra warning"
        );
        assert!(
            Command::KillSession("work".to_owned())
                .confirmation(Some(0), &several)
                .expect("a kill asks")
                .consequence
                .is_none(),
            "nor does a session this client is not attached to"
        );
    }

    /// A command that names something destroyable knows whether it is still there; everything else is
    /// trivially still targetable.
    ///
    /// REVERT-PROOF: make `target_still_exists` return `true` unconditionally and the two "gone"
    /// assertions fail — which is what would let a prompt outlive its target.
    #[test]
    fn only_a_command_naming_a_destroyable_target_can_report_it_gone() {
        let (slots, _log) = slots_with(&[("main", true)], &["0"], "0");

        assert!(Command::KillWindow("main".to_owned()).target_still_exists(None, &slots));
        assert!(!Command::KillWindow("gone".to_owned()).target_still_exists(None, &slots));
        assert!(Command::KillSession("0".to_owned()).target_still_exists(None, &slots));
        assert!(!Command::KillSession("gone".to_owned()).target_still_exists(None, &slots));
        assert!(
            Command::NewWindow.target_still_exists(None, &slots),
            "a command that names nothing destroyable is always targetable"
        );
        // The pane's case is target-addressed, so its answer moves with the ARGUMENT, not with a
        // name — the whole reason `target_still_exists` gained one.
        assert!(Command::KillPane.target_still_exists(Some(0), &slots));
        assert!(
            !Command::KillPane.target_still_exists(Some(3), &slots),
            "an empty slot is a pane that has gone"
        );
        assert!(
            !Command::KillPane.target_still_exists(None, &slots),
            "and a kill armed with no pane at all can never still be targetable"
        );
    }

    /// The two pane commands reach the HOST — a split creates, a kill removes THE captured pane —
    /// and the kill is addressed by the pane's host id, not by its display slot.
    ///
    /// REVERT-PROOF: drop either arm of [`Command::run`] and its assertion fails; address the kill
    /// by the slot number instead of `SlotView`'s mapping and the recorded id is 0, not 8.
    #[test]
    fn the_pane_commands_reach_the_host_and_the_kill_is_slot_mapped() {
        let (slots, log) = slots_with_panes(&[("main", true)], &["0"], "0", 2);

        Command::NewPane.run(None, &slots);
        assert_eq!(
            log.borrow().new_panes,
            1,
            "a split reaches the host with no target — it goes into the current window"
        );

        Command::KillPane.run(Some(1), &slots);
        assert_eq!(
            log.borrow().killed_panes,
            vec![PaneId(8)],
            "slot 1 is host pane 8 — the kill is addressed by id, never by the display slot"
        );

        // Total over an absent target, like every other pane arm: nothing runs, nothing panics.
        Command::KillPane.run(None, &slots);
        assert_eq!(
            log.borrow().killed_panes.len(),
            1,
            "a kill armed with no pane touches nothing"
        );
    }

    /// A pane kill names the PROGRAM it is about to end, and escalates only when the pane is the
    /// last one — composing with the window escalation when it is also the last window.
    ///
    /// REVERT-PROOF: drop the `occupied_slots` condition and the two-pane case grows a consequence;
    /// drop the `windows` condition and the last-pane case stops mentioning the session.
    #[test]
    fn a_pane_kill_states_only_the_escalation_it_actually_has() {
        // Two panes: nothing extra to say, whatever the window count.
        let (roomy, _log) = slots_with_panes(&[("main", true), ("build", false)], &["0"], "0", 2);
        assert!(
            Command::KillPane
                .confirmation(Some(0), &roomy)
                .expect("a pane kill asks")
                .consequence
                .is_none(),
            "a pane with a sibling takes nothing else down with it"
        );

        // The last pane of a window that is NOT the last window: the window goes, the session stays.
        let (last_pane, _log) =
            slots_with_panes(&[("main", true), ("build", false)], &["0"], "0", 1);
        let words = Command::KillPane
            .confirmation(Some(0), &last_pane)
            .expect("a pane kill asks");
        let line = words
            .consequence
            .as_deref()
            .expect("the escalation is said");
        assert!(
            line.contains("last pane"),
            "it names the pane escalation: {line}"
        );
        assert!(
            !line.contains("session"),
            "and does NOT claim the session ends when another window survives: {line}"
        );

        // The last pane of the last window: both escalations, in one sentence.
        let (last_of_all, _log) = slots_with_panes(&[("main", true)], &["0"], "0", 1);
        let line = Command::KillPane
            .confirmation(Some(0), &last_of_all)
            .expect("a pane kill asks")
            .consequence
            .expect("the escalation is said");
        assert!(
            line.contains("last pane") && line.contains("last window"),
            "the two escalations compose: {line}"
        );
    }

    /// The two pane rows are OFFERED on the terms their kinds set: the split needs no pane (it
    /// creates one in the current window), the kill needs the pane it destroys — and the kill trails
    /// the list with the other destructive rows while the split sits among the safe ones.
    ///
    /// REVERT-PROOF: make `NewPane::needs_pane` true and it vanishes from the pane-less catalog;
    /// drop the `target.is_some()` guard on the kill and it is offered with nothing to kill.
    #[test]
    fn the_pane_rows_are_offered_on_the_terms_their_kinds_set() {
        let (slots, _log) = slots_with(&[("main", true)], &["0"], "0");

        let with_pane = catalog(Some(0), &slots).commands;
        assert!(
            with_pane.contains(&Command::NewPane) && with_pane.contains(&Command::KillPane),
            "a captured pane offers both: {with_pane:?}"
        );
        let split = with_pane
            .iter()
            .position(|c| *c == Command::NewPane)
            .expect("the split is offered");
        let kill = with_pane
            .iter()
            .position(|c| *c == Command::KillPane)
            .expect("the kill is offered");
        assert!(
            split < kill,
            "the split sits with the safe rows and the kill trails with the destructive ones"
        );

        let without = catalog(None, &slots).commands;
        assert!(
            without.contains(&Command::NewPane),
            "a split is still offered with no pane captured — it needs none: {without:?}"
        );
        assert!(
            !without.contains(&Command::KillPane),
            "but a kill with nothing to kill is not a row: {without:?}"
        );
    }

    #[test]
    fn every_command_carries_a_non_empty_title() {
        // The title is what a query matches and what the row paints; an empty one would be an
        // unreachable, invisible row.
        let (slots, _log) = slots_with(&[("main", true), ("build", false)], &["0", "work"], "0");
        for command in catalog(Some(0), &slots).commands {
            assert!(
                !command.title().trim().is_empty(),
                "{command:?} paints and matches on its title"
            );
        }
    }
}
