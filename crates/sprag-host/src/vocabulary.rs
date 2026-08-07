//! sprag's VOCABULARY: every verb the product has, named ONCE, with which mouth can say it.
//!
//! # Why this module exists
//!
//! [`crate::keymap`]'s own module doc claims a binding is parsed from *"the same string the shell
//! takes"* — *"one vocabulary rather than three (a CLI verb, a tmux verb, and a per-frontend
//! enum)"*. Measured at `b588a41` by running the shipped binary against every verb it dispatches,
//! that was **not true**: the shell dispatched 47 verbs, `sprag bind-key F9 <verb>` accepted 8 of
//! them outright and 6 more once flags were added, and the remaining 33 came back
//! `"<verb>" is not an action (there are: …)` — the same sentence a TYPO gets. A person who had
//! just run `sprag break-pane` was told the verb does not exist.
//!
//! Three hand-written lists said what sprag can do and nothing held any two of them together:
//!
//! * `run()`'s dispatch in the `sprag` binary — the only one that was true by construction, because
//!   an arm that is not there does not run.
//! * the `USAGE` text — whose own doc said *"a second list is exactly what nothing checks"*, and
//!   which was missing `run` and `hook` when this module was written. Both are dispatched; neither
//!   appeared in `sprag --help`. Measured, not read.
//! * [`BoundAction::vocabulary`](crate::keymap::BoundAction::vocabulary) — 19 spellings, joined to
//!   the parser by a test and to the CLI's 47 verbs by nothing at all.
//!
//! So this is the one list, and it is a list of the PRODUCT's verbs rather than of one surface's:
//! [`Verb::Ls`] is a thing sprag does, and whether a shell or a keystroke can ask for it is a
//! PROPERTY of the verb rather than the reason it is here. Two mouths, one vocabulary — which is
//! what the keymap's doc always said.
//!
//! # What each surface reads
//!
//! * The CLI dispatches by [`Verb::parse`] and then matches EXHAUSTIVELY, so a verb added here
//!   without an implementation does not compile.
//! * `sprag --help` is BUILT from [`usage`], so a verb cannot be dispatched and undocumented — the
//!   drift above is unrepresentable rather than ratcheted.
//! * [`BoundAction::parse`](crate::keymap::BoundAction::parse) asks [`Verb::keystroke`] before it
//!   refuses anything, so a real verb a keystroke cannot mean is refused in ITS OWN WORDS
//!   ([`NotAKeystroke`]) and never as a typo.
//! * The keyboard's own list of forms is DERIVED here ([`bindable_forms`]), so the 19-entry array
//!   that had been stale for eight rounds cannot exist twice.
//!
//! # The gap is countable now
//!
//! [`Keystroke::NotBuilt`] is the honest third answer, and it is what stops this table from lying
//! in the other direction: `join-pane` is a verb a keystroke COULD mean, and sprag does not bind it
//! yet. Filing it under [`NotAKeystroke`] would be a refusal that is not true; leaving it out would
//! put it back in the "typo" sentence. Counting them is one test
//! ([`the_keyboard_gap_is_what_the_table_says_it_is`](self)), so the register's number is derived
//! from the code rather than re-measured by hand each round.

use crate::keymap::ActionSubject;

/// One verb of sprag's vocabulary — one thing the product can be asked to do.
///
/// **Closed on purpose, and the CLI's dispatch is exhaustive over it**: a variant added here forces
/// an [`Entry`] (one match, so every property is decided at once), a dispatch arm in the binary, and
/// a decision about the keyboard. That is the whole mechanism — a new verb enters every sweep by
/// construction instead of the day somebody remembers to add it to a list.
///
/// The spelling of each is in [`Entry::name`], never in the variant's identifier: the variant is
/// this crate's name for the act and the string is the user's, and only one of those is allowed to
/// change without a deprecation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Verb {
    /// `ls` — every session of this daemon, one line each.
    Ls,
    /// `list-clients` — who is attached, and to what.
    ListClients,
    /// `new` — create a session.
    New,
    /// `attach` — point a display client at a session, launching one.
    Attach,
    /// `ssh` — a session whose first pane is a login on another host.
    Ssh,
    /// `find` — where a needle appears in a pane's text.
    Find,
    /// `wait-for-output` — park until a pane prints something.
    WaitForOutput,
    /// `run` — a pane's declared project commands, listed or typed at its prompt.
    Run,
    /// `rename-session` — a session's name is its address; move it.
    RenameSession,
    /// `kill-session` — end a session and everything in it.
    KillSession,
    /// `kill-server` — end the daemon, and every session it holds.
    KillServer,
    /// `windows` — the windows of one session.
    Windows,
    /// `new-window` — a window, born with a shell.
    NewWindow,
    /// `select-window` — make another window of this session current.
    SelectWindow,
    /// `rename-window` — a window's name is its address inside its session.
    RenameWindow,
    /// `kill-window` — end a window and the panes in it.
    KillWindow,
    /// `move-window` — where a window sits in its session's order.
    MoveWindow,
    /// `resize-window` — force a window's cell size, or give the forcing back.
    ResizeWindow,
    /// `break-pane` — take a pane out into a window of its own.
    BreakPane,
    /// `join-pane` — put a pane into another window.
    JoinPane,
    /// `move-pane` — put a pane beside another pane, on an axis.
    MovePane,
    /// `panes` — the panes of one session, with where they are.
    Panes,
    /// `layout` — the arrangement, drawn.
    Layout,
    /// `processes` — what is running inside a pane.
    Processes,
    /// `select-pane` — which pane the session is on.
    SelectPane,
    /// `swap-pane` — two panes trade places.
    SwapPane,
    /// `split-window` — divide a pane and put a shell in the half it opens.
    SplitWindow,
    /// `kill-pane` — end a pane, and whatever the cascade takes with it.
    KillPane,
    /// `resize-pane` — move the boundary that bounds a pane.
    ResizePane,
    /// `zoom-pane` — one pane fills its window, or the arrangement comes back.
    ZoomPane,
    /// `rename-pane` — give a pane a name a person can address it by.
    RenamePane,
    /// `send-keys` — type into a pane's child.
    SendKeys,
    /// `capture-pane` — a pane's text, as text.
    CapturePane,
    /// `agent` — what the AI in a pane is doing.
    Agent,
    /// `report-agent` — an agent says what it is doing.
    ReportAgent,
    /// `release-agent` — an agent stops claiming a pane.
    ReleaseAgent,
    /// `display-message` — put a sentence on somebody's screen.
    DisplayMessage,
    /// `install-hooks` — wire an agent's hooks into its own config.
    InstallHooks,
    /// `uninstall-hooks` — take them back out.
    UninstallHooks,
    /// `list-hooks` — which agents are wired.
    ListHooks,
    /// `hook` — the entry point an agent's hook process runs.
    Hook,
    /// `events` — what happened, or what is happening.
    Events,
    /// `list-keys` — what the keys do.
    ListKeys,
    /// `bind-key` — give a key a meaning.
    BindKey,
    /// `unbind-key` — take one away.
    UnbindKey,
    /// `show-options` — the settings, resolved.
    ShowOptions,
    /// `set-option` — change one.
    SetOption,
    /// `version` — which build this is.
    Version,
    /// `help` — the usage.
    Help,
    /// `detach-client` — give the terminal back, leave the session running.
    ///
    /// The first of the five verbs with NO shell form: what it acts on is the client that pressed
    /// the key, and a shell has no client. See [`Entry::shell`].
    DetachClient,
    /// `send-prefix` — type the prefix key into the pane, so a program can still receive it.
    SendPrefix,
    /// `switch-client` — point THIS client at another session of the same daemon.
    SwitchClient,
    /// `choose-tree` — show every session, window and pane, and go to the one picked.
    ChooseTree,
    /// `confirm-before` — ask a yes/no question, then run the verb it wraps.
    ConfirmBefore,
}

/// How the SHELL spells a verb, and where `sprag --help` prints it.
///
/// [`None`] on an [`Entry`] means no CLI dispatches this verb at all — which is a fact about the
/// verb ([`Verb::DetachClient`] acts on the client that pressed the key), not a gap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shell {
    /// Which block of the usage this verb belongs to.
    pub group: Group,
    /// The arguments, spelled as the usage spells them — WITHOUT the verb's own name, which
    /// [`usage`] writes from [`Entry::name`] so a synopsis cannot disagree with the name above it.
    pub form: &'static str,
}

/// What a KEYSTROKE can mean by a verb.
///
/// The three answers are deliberately different questions, because a user reading a refusal needs
/// to know which one they got: a verb they can bind, a verb nobody has built a binding for, and a
/// verb a keystroke cannot mean at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keystroke {
    /// A binding can mean it, in these FORMS — the flag grammar
    /// [`BoundAction::parse`](crate::keymap::BoundAction::parse) accepts, verb name included, since
    /// this string is what a user is shown to type.
    ///
    /// The grammar lives here rather than in the parser because it is READ by two messages and
    /// printed to people; the parser stays the authority on which strings actually parse, and
    /// `the_bindable_forms_are_the_ones_the_parser_takes` holds the two together.
    Means(&'static str),
    /// A keystroke COULD mean it and sprag builds no binding for it yet.
    ///
    /// The honest third answer. Filing one of these under [`Cannot`](Self::Cannot) would print a
    /// reason that is not true, and leaving it out of the table would put it back in the sentence a
    /// TYPO gets — which is the defect this module exists to close. Their COUNT is the keyboard's
    /// remaining gap, asserted by a test rather than re-measured by hand.
    NotBuilt,
    /// A keystroke cannot mean it, for this reason.
    Cannot(NotAKeystroke),
}

/// Why a keystroke cannot mean a verb.
///
/// A closed set rather than a sentence per verb, and that is the difference between a vocabulary
/// with a POLICY and forty refusals somebody wrote one at a time: every verb here answers to one of
/// five rules, and a new verb that fits none of them is a rule this project has not thought about
/// yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotAKeystroke {
    /// It ANSWERS with text, and a keystroke has nowhere to put an answer.
    ///
    /// The one that covers the most verbs, and the honest half is that it is a statement about the
    /// SURFACE: `list-keys` is bound precisely because this client has a view for its answer, and a
    /// verb here would become bindable the day sprag grows a surface for what it says. See
    /// [`Verb::ListKeys`].
    Answers,
    /// Its argument is TEXT the user types, so a binding would fix one string and mean it forever.
    ///
    /// [`crate::keymap::BoundAction::RenameWindow`]'s rule, applied outwards: a `rename-window`
    /// binding carries the decision to ASK rather than a name, and a verb whose whole content is
    /// the words (`send-keys`, `display-message`) has nothing left once the words are taken out.
    NeedsWords,
    /// It edits a FILE on disk that a later start reads — including the file the keys come from.
    ///
    /// A key that rewrites the keymap is a key whose next press may not exist.
    EditsAFile,
    /// It is an AGENT's verb about the agent's OWN pane, and the person at the keyboard is not that
    /// agent.
    TheAgentsOwn,
    /// It starts or ends something AROUND the client — a new display process, or the daemon every
    /// client is attached to.
    OutsideTheClient,
}

impl NotAKeystroke {
    /// The clause a refusal ends with, in the user's terms.
    ///
    /// One per reason rather than one per verb: the sentence a person reads has to say which RULE
    /// they hit, because that is what tells them whether to look for another verb or to stop
    /// looking.
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::Answers => "it answers with text, and a keystroke has nowhere to put an answer",
            Self::NeedsWords => {
                "its argument is text you type, and a binding would fix one and mean it forever"
            }
            Self::EditsAFile => {
                "it edits a file on disk that the next start reads, including the keys themselves"
            }
            Self::TheAgentsOwn => {
                "it is an agent's own verb about the pane it is running in, and a keystroke is a \
                 person's"
            }
            Self::OutsideTheClient => {
                "it starts or ends something around the client rather than inside it"
            }
        }
    }
}

/// Which block of `sprag --help` a verb prints in, and in what order.
///
/// The ORDER is the declaration order and it is the reading order of the help: outward through the
/// containment the product itself uses (session, window, pane), then the surfaces that are about
/// somebody else (an agent), then the ones about sprag itself.
///
/// It exists because a usage text is a DOCUMENT rather than a list — the grouping is what makes 49
/// verbs readable — and a group is the one piece of that document a verb has to carry for the
/// document to be derivable from the verbs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Group {
    /// The daemon and its sessions.
    Session,
    /// One session's windows.
    Window,
    /// One window's panes.
    Pane,
    /// An AI agent working in a pane, and the messages around it.
    Agent,
    /// The keyboard.
    Keys,
    /// The settings.
    Options,
    /// sprag itself.
    Tool,
}

impl Group {
    /// What the help calls this block.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Session => "sessions",
            Self::Window => "windows",
            Self::Pane => "panes",
            Self::Agent => "agents",
            Self::Keys => "keys",
            Self::Options => "options",
            Self::Tool => "sprag",
        }
    }
}

/// Everything the vocabulary knows about one verb.
///
/// Answered by ONE exhaustive match ([`Verb::entry`]) rather than by a method per property, and
/// that is the load-bearing choice: five matches are five chances to forget an arm, where one match
/// makes the compiler ask for every property of a new verb at the moment it is added.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    /// The verb as a user spells it — the FIRST WORD of a command line or of a binding.
    pub name: &'static str,
    /// How a shell says it, or [`None`] for a verb that is only ever a keystroke.
    pub shell: Option<Shell>,
    /// What a keystroke can mean by it.
    pub key: Keystroke,
}

impl Verb {
    /// Every verb, in the order the help prints them.
    ///
    /// The one hand-written sequence in this module, and the only drift it can carry is an OMISSION
    /// — which [`the_table_holds_every_variant_of_the_enum`](self) catches by counting the enum's
    /// own variants out of this file's source, the instrument R322 built for the wire's methods.
    pub const ALL: [Self; 54] = [
        Self::Ls,
        Self::ListClients,
        Self::New,
        Self::Attach,
        Self::Ssh,
        Self::Find,
        Self::WaitForOutput,
        Self::Run,
        Self::RenameSession,
        Self::KillSession,
        Self::KillServer,
        Self::Windows,
        Self::NewWindow,
        Self::SelectWindow,
        Self::RenameWindow,
        Self::KillWindow,
        Self::MoveWindow,
        Self::ResizeWindow,
        Self::BreakPane,
        Self::JoinPane,
        Self::MovePane,
        Self::Panes,
        Self::Layout,
        Self::Processes,
        Self::SelectPane,
        Self::SwapPane,
        Self::SplitWindow,
        Self::KillPane,
        Self::ResizePane,
        Self::ZoomPane,
        Self::RenamePane,
        Self::SendKeys,
        Self::CapturePane,
        Self::Agent,
        Self::ReportAgent,
        Self::ReleaseAgent,
        Self::DisplayMessage,
        Self::InstallHooks,
        Self::UninstallHooks,
        Self::ListHooks,
        Self::Hook,
        Self::Events,
        Self::ListKeys,
        Self::BindKey,
        Self::UnbindKey,
        Self::ShowOptions,
        Self::SetOption,
        Self::Version,
        Self::Help,
        Self::DetachClient,
        Self::SendPrefix,
        Self::SwitchClient,
        Self::ChooseTree,
        Self::ConfirmBefore,
    ];

    /// This verb's whole entry — its spelling, its shell form, and what a keystroke can mean by it.
    ///
    /// # Why the keyboard's answer is DATA and not a rule computed from the shell form
    ///
    /// A rule would have to read English: `capture-pane PANE [-p]` and `break-pane PANE [name]`
    /// take the same shape of argument and only one of them is an act a keystroke can perform. So
    /// each verb states its own answer, and the five [`NotAKeystroke`] reasons are what keeps that
    /// from becoming forty opinions.
    #[must_use]
    pub const fn entry(self) -> Entry {
        // A local so the 54 arms below read as a table rather than as 54 struct literals. `shell`
        // wraps the two fields a CLI verb has; a verb with no shell form says so by name.
        const fn shell(group: Group, form: &'static str) -> Option<Shell> {
            Some(Shell { group, form })
        }
        const NO_SHELL: Option<Shell> = None;
        let (name, shell, key) = match self {
            // ── sessions ────────────────────────────────────────────────────────────────────────
            Self::Ls => (
                "ls",
                shell(Group::Session, ""),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::ListClients => (
                "list-clients",
                shell(Group::Session, "[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // BINDABLE SINCE R323, and the arm that made the keyboard reach the session level at
            // all: a person could create a window with `prefix c` and had no key that creates a
            // SESSION. The name the CLI verb takes is the daemon's to generate when a keystroke
            // does not give one, which is what makes this bindable where `ssh` is not.
            Self::New => (
                "new",
                shell(Group::Session, "[name]"),
                Keystroke::Means("new"),
            ),
            // A display PROCESS is launched, and the client pressing the key is one already.
            Self::Attach => (
                "attach",
                shell(Group::Session, "NAME [--no-wait | --tui | --remote HOST]"),
                Keystroke::Cannot(NotAKeystroke::OutsideTheClient),
            ),
            Self::Ssh => (
                "ssh",
                shell(
                    Group::Session,
                    "[user@]host [-p PORT] [-L FWD]… [--tmux[=NAME]] [-- command…]",
                ),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
            ),
            Self::Find => (
                "find",
                shell(
                    Group::Session,
                    "NEEDLE [-t SESSION] [--pane PANE] [--regex]",
                ),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::WaitForOutput => (
                "wait-for-output",
                shell(Group::Session, "--pane PANE NEEDLE [-t SESSION] [--regex]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // A keystroke could mean "show me this pane's project commands and type the one I
            // pick" — which is the GUI palette's own gesture, and `sprag-tui` has no palette. The
            // pick would have to carry what it is FOR (`chooser::Pick::commit` goes to a row), so
            // this is a surface this round did not build rather than an act a key cannot mean.
            Self::Run => (
                "run",
                shell(Group::Session, "[NAME] [-t SESSION] [--pane PANE]"),
                Keystroke::NotBuilt,
            ),
            Self::RenameSession => (
                "rename-session",
                shell(Group::Session, "[-t SESSION] NEW"),
                Keystroke::Means("rename-session"),
            ),
            // BINDABLE SINCE R323. A keystroke means THIS client's session, which is the one thing
            // a client always knows and the CLI never does — so the verb needs its NAME in a shell
            // and needs nothing at all from a key.
            Self::KillSession => (
                "kill-session",
                shell(Group::Session, "NAME"),
                Keystroke::Means("kill-session"),
            ),
            // The daemon every attached client is looking at, this one included. A key for it would
            // end the screen it was pressed on, and the sentence says which rule that is.
            Self::KillServer => (
                "kill-server",
                shell(Group::Session, "[--purge]"),
                Keystroke::Cannot(NotAKeystroke::OutsideTheClient),
            ),
            // ── windows ─────────────────────────────────────────────────────────────────────────
            Self::Windows => (
                "windows",
                shell(Group::Window, "[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::NewWindow => (
                "new-window",
                shell(Group::Window, "[-d] [name] [-t SESSION]"),
                Keystroke::Means("new-window"),
            ),
            Self::SelectWindow => (
                "select-window",
                shell(Group::Window, "<NAME|-n|-p> [-t SESSION]"),
                Keystroke::Means("select-window -n|-p|-t <window>"),
            ),
            Self::RenameWindow => (
                "rename-window",
                shell(Group::Window, "[window] NAME [-t SESSION]"),
                Keystroke::Means("rename-window"),
            ),
            Self::KillWindow => (
                "kill-window",
                shell(Group::Window, "[window] [-t SESSION]"),
                Keystroke::Means("kill-window"),
            ),
            Self::MoveWindow => (
                "move-window",
                shell(
                    Group::Window,
                    "[window] <--first | --last | -n | -p | --before W | --after W> [-t SESSION]",
                ),
                Keystroke::Means(
                    "move-window --first|--last|-n|-p|--before [<window>]|--after <window>",
                ),
            ),
            // A key could mean the FIT forms (`-a` / `-A` / `-u`), which carry no number. What
            // stops it today is that no client has a call for it: `HostClient` has `resize_toward`
            // for a pane boundary and nothing for a window's forced size.
            Self::ResizeWindow => (
                "resize-window",
                shell(
                    Group::Window,
                    "[window] <-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u> [-t SESSION]",
                ),
                Keystroke::NotBuilt,
            ),
            // BINDABLE SINCE R323, on tmux's own `prefix !`. The pane is the focused one and the
            // name is optional, so a keystroke carries everything this verb needs — which is why it
            // was the loudest of the fifteen refusals R322 measured.
            Self::BreakPane => (
                "break-pane",
                shell(Group::Window, "PANE [name] [-t SESSION]"),
                Keystroke::Means("break-pane"),
            ),
            // The pane is the focused one; the WINDOW is a row a person would PICK. The chooser
            // that shows windows exists and its pick commits to one thing (`goto`), so the surface
            // is a generalisation rather than a new view — see [`Run`](Self::Run).
            Self::JoinPane => (
                "join-pane",
                shell(Group::Window, "PANE WINDOW [-t SESSION]"),
                Keystroke::NotBuilt,
            ),
            // BINDABLE SINCE R328. The CLI form names both panes; a binding names NEITHER — the
            // mover is the focused pane and the target is a row the person PICKS, which is what
            // `chooser::Errand` was built to let a pick mean. The flags are `split-window`'s, and
            // for the same question: which half of the target the arrival takes.
            Self::MovePane => (
                "move-pane",
                shell(Group::Window, "PANE -h|-v [-b] TARGET [-t SESSION]"),
                Keystroke::Means("move-pane -h|-v [-b]"),
            ),
            // ── panes ───────────────────────────────────────────────────────────────────────────
            Self::Panes => (
                "panes",
                shell(Group::Pane, "[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::Layout => (
                "layout",
                shell(Group::Pane, "[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::Processes => (
                "processes",
                shell(Group::Pane, "[PANE] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::SelectPane => (
                "select-pane",
                shell(
                    Group::Pane,
                    "<PANE | -L|-R|-U|-D [--from PANE]> [-t SESSION]",
                ),
                Keystroke::Means("select-pane -L|-R|-U|-D|-t :.+"),
            ),
            Self::SwapPane => (
                "swap-pane",
                shell(Group::Pane, "[PANE] <WITH | -L|-R|-U|-D> [-t SESSION]"),
                Keystroke::Means("swap-pane -L|-R|-U|-D"),
            ),
            Self::SplitWindow => (
                "split-window",
                shell(
                    Group::Pane,
                    "[-h|-v [-b] [PANE]] [-- command…] [-t SESSION]",
                ),
                Keystroke::Means("split-window -h|-v [-b]"),
            ),
            Self::KillPane => (
                "kill-pane",
                shell(Group::Pane, "[PANE] [-t SESSION]"),
                Keystroke::Means("kill-pane"),
            ),
            Self::ResizePane => (
                "resize-pane",
                shell(
                    Group::Pane,
                    "[PANE] <-x COLS -y ROWS | -L|-R|-U|-D [N]> [-t SESSION]",
                ),
                Keystroke::Means("resize-pane -L|-R|-U|-D [N]"),
            ),
            Self::ZoomPane => (
                "zoom-pane",
                shell(Group::Pane, "[PANE] [-u] [-t SESSION]"),
                Keystroke::Means("zoom-pane [-Z|-u]"),
            ),
            Self::RenamePane => (
                "rename-pane",
                shell(Group::Pane, "PANE <NAME | --clear> [-t SESSION]"),
                Keystroke::Means("rename-pane"),
            ),
            // The pane the person is on already receives their keys; what this verb adds is the
            // WORDS, and a binding cannot carry those.
            Self::SendKeys => (
                "send-keys",
                shell(Group::Pane, "PANE [-l] KEY… [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
            ),
            Self::CapturePane => (
                "capture-pane",
                shell(Group::Pane, "PANE [-p] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // ── agents ──────────────────────────────────────────────────────────────────────────
            Self::Agent => (
                "agent",
                shell(Group::Agent, "[PANE] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::ReportAgent => (
                "report-agent",
                shell(
                    Group::Agent,
                    "<working|blocked|idle> [-t SESSION] [--pane PANE] [--source S] [--name AGENT] \
                     [--seq N]",
                ),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
            ),
            Self::ReleaseAgent => (
                "release-agent",
                shell(Group::Agent, "[-t SESSION] [--pane PANE]"),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
            ),
            Self::DisplayMessage => (
                "display-message",
                shell(
                    Group::Agent,
                    "[-t SESSION] [-c CLIENT] [-s note|warn|alert] MESSAGE",
                ),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
            ),
            Self::InstallHooks => (
                "install-hooks",
                shell(Group::Agent, "[AGENT…] [--yes] [--dry-run]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
            ),
            Self::UninstallHooks => (
                "uninstall-hooks",
                shell(Group::Agent, "[AGENT…] [--yes] [--dry-run]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
            ),
            Self::ListHooks => (
                "list-hooks",
                shell(Group::Agent, ""),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // **NAMED IN THE HELP SINCE R323, and it was dispatched and undocumented before.** It
            // is an agent's hook process reporting on the agent's behalf, so a person will only
            // ever run it while working out why their hooks are quiet — which is exactly when a
            // verb missing from the usage costs an hour.
            Self::Hook => (
                "hook",
                shell(Group::Agent, "EVENT   (an agent's hook; payload on stdin)"),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
            ),
            Self::Events => (
                "events",
                shell(
                    Group::Agent,
                    "[-t SESSION] [--since N] [-f [--pane PANE] [--kind KIND]…]",
                ),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // ── keys ────────────────────────────────────────────────────────────────────────────
            // THE ONE ANSWERING VERB THAT IS BOUND, and the reason is the whole content of
            // [`NotAKeystroke::Answers`]: this client has a VIEW for the answer
            // ([`crate::keyhelp`]). A verb refused for answering becomes bindable the day sprag
            // grows a surface for what it says.
            Self::ListKeys => (
                "list-keys",
                shell(Group::Keys, "[-N]"),
                Keystroke::Means("list-keys"),
            ),
            Self::BindKey => (
                "bind-key",
                shell(Group::Keys, "[-nr] [-T prefix|root] KEY ACTION…"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
            ),
            Self::UnbindKey => (
                "unbind-key",
                shell(Group::Keys, "[-n] [-T prefix|root] KEY"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
            ),
            // ── options ─────────────────────────────────────────────────────────────────────────
            Self::ShowOptions => (
                "show-options",
                shell(Group::Options, "[-v] [NAME] [-g]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::SetOption => (
                "set-option",
                shell(Group::Options, "[-u] NAME [VALUE] [-g]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
            ),
            // ── sprag itself ────────────────────────────────────────────────────────────────────
            Self::Version => (
                "version",
                shell(Group::Tool, "  (also --version, -V)"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            Self::Help => (
                "help",
                shell(Group::Tool, "     (also --help, -h)"),
                Keystroke::Cannot(NotAKeystroke::Answers),
            ),
            // ── the five with no shell form ─────────────────────────────────────────────────────
            // Each acts on THE CLIENT THAT PRESSED THE KEY, which a shell does not have. That is a
            // property of the verb and not a gap: `sprag detach-client` would need to name a client
            // and then mean something different from what the key means.
            Self::DetachClient => ("detach-client", NO_SHELL, Keystroke::Means("detach-client")),
            Self::SendPrefix => ("send-prefix", NO_SHELL, Keystroke::Means("send-prefix")),
            Self::SwitchClient => (
                "switch-client",
                NO_SHELL,
                Keystroke::Means("switch-client -n|-p|-l|-t [<session>]"),
            ),
            Self::ChooseTree => ("choose-tree", NO_SHELL, Keystroke::Means("choose-tree")),
            Self::ConfirmBefore => (
                "confirm-before",
                NO_SHELL,
                Keystroke::Means("confirm-before <action>"),
            ),
        };
        Entry { name, shell, key }
    }

    /// The verb as a user spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.entry().name
    }

    /// What a keystroke can mean by it.
    #[must_use]
    pub const fn keystroke(self) -> Keystroke {
        self.entry().key
    }

    /// Whether `sprag <name>` dispatches it.
    #[must_use]
    pub const fn runs_in_shell(self) -> bool {
        self.entry().shell.is_some()
    }

    /// Whether a binding can name it.
    #[must_use]
    pub const fn bindable(self) -> bool {
        matches!(self.keystroke(), Keystroke::Means(_))
    }

    /// The verb this word names, if any — the ONE place a command line's or a binding's first word
    /// becomes a verb.
    ///
    /// The two flag spellings of [`Version`](Self::Version) and [`Help`](Self::Help) are accepted
    /// here rather than in the caller, so `sprag -h` and `sprag help` cannot come apart.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "-V" | "--version" => return Some(Self::Version),
            "-h" | "--help" => return Some(Self::Help),
            _ => {}
        }
        Self::ALL.into_iter().find(|verb| verb.name() == word)
    }

    /// The sentence a SHELL answers with for a verb that is only ever a keystroke.
    ///
    /// It names the verb, says which surface has it, and gives the line that would bind it — which
    /// is the whole difference between this and `unknown command`: a user who typed
    /// `sprag switch-client -n` asked for something that exists.
    ///
    /// [`None`] for a verb the shell does run, so a caller cannot print this about `sprag ls`.
    #[must_use]
    pub fn only_a_keystroke(self) -> Option<String> {
        if self.runs_in_shell() {
            return None;
        }
        let form = match self.keystroke() {
            Keystroke::Means(form) => form,
            // Unreachable through the table (a verb with no shell form and no keystroke would be a
            // verb nobody can say, which `every_verb_has_a_mouth` refuses), and it must not be an
            // `expect`: this renders an error message.
            Keystroke::NotBuilt | Keystroke::Cannot(_) => self.name(),
        };
        Some(format!(
            "{:?} is a key binding, not a command: bind it with `sprag bind-key <key> {form:?}`",
            self.name(),
        ))
    }
}

/// Every FORM a binding may name, in the shell's own spelling — the keyboard's whole vocabulary.
///
/// **Derived, and that is the point.** This was a 19-entry array beside the parser, whose own doc
/// recorded that `sprag bind-key`'s copy of it had been stale for eight rounds. It is now a
/// projection of the one table, so a verb that becomes bindable appears in every message that
/// lists the vocabulary the moment its entry says [`Keystroke::Means`].
///
/// # The order is the KEYBOARD's, not the shell's
///
/// By [`ActionSubject`] and then by [`Verb::ALL`], where [`usage`] uses `ALL` alone. The two lists
/// are read in different places and are grouped by their own reader's axis: a help view lists the
/// bindings in force by subject — client first, *"because `list-keys` and `detach-client` are what
/// a user who is lost needs"* — and printing the forms underneath in the shell's containment order
/// would run the same view's two halves in opposite directions.
#[must_use]
pub fn bindable_forms() -> Vec<&'static str> {
    let mut bindable: Vec<Verb> = Verb::ALL
        .into_iter()
        .filter(|verb| verb.bindable())
        .collect();
    // STABLE, so within one subject the order is still the vocabulary's own.
    bindable.sort_by_key(|verb| subject_of(*verb));
    bindable
        .into_iter()
        .filter_map(|verb| match verb.keystroke() {
            Keystroke::Means(form) => Some(form),
            Keystroke::NotBuilt | Keystroke::Cannot(_) => None,
        })
        .collect()
}

/// What `sprag` with no verb — or with one it does not have — prints.
///
/// **BUILT from [`Verb::ALL`], not written beside it.** The text this replaced was a `const` whose
/// own doc said *"a second list is exactly what nothing checks"*, and when this module was written
/// it was missing `run` and `hook`: two verbs the binary dispatched and the help did not name,
/// measured by running it. A verb cannot be dispatched and undocumented now, because the same array
/// the dispatch is exhaustive over is what this iterates.
///
/// The GROUPING is [`Group`]'s and the layout is one verb per line, which is a change from the
/// packed `sprag <a | b | c>` shape it replaced: a run of verbs sharing one `[-t SESSION]` tail
/// cannot be assembled from entries that each spell their own arguments, and a line per verb is
/// what `tmux list-commands` prints for the same reason.
#[must_use]
pub fn usage() -> String {
    let mut out = String::from("usage: sprag <command> [arguments]\n");
    let mut group = None;
    for verb in Verb::ALL {
        let Some(shell) = verb.entry().shell else {
            continue;
        };
        if group != Some(shell.group) {
            out.push_str(&format!("\n  {}\n", shell.group.heading()));
            group = Some(shell.group);
        }
        let name = verb.name();
        if shell.form.is_empty() {
            out.push_str(&format!("    {name}\n"));
        } else {
            out.push_str(&format!("    {name} {}\n", shell.form));
        }
    }
    // Indented as deeply as the verbs, so a HEADING is the only thing at two spaces — which is
    // what lets a reader, and `the_usage_is_the_shell_half_of_the_table`, tell the two apart.
    out.push_str(
        "\n    PANE is a pane's id (see `sprag panes`) or its NAME (`sprag rename-pane`).\n    \
         Either spelling reaches any WINDOW of the session, and neither crosses a session.\n    \
         A key can run the verbs above that a keystroke can mean — `sprag list-keys` says which.",
    );
    out
}

/// Where a bindable verb belongs in [`crate::keyhelp`]'s grouping, for a caller holding a [`Verb`]
/// rather than a [`crate::keymap::BoundAction`].
///
/// **Deliberately NOT a field of [`Entry`].** `BoundAction::subject` already answers this for every
/// bound action and the help view reads it there; a second answer in this table is exactly the
/// drift this module exists to remove. It is a function of the [`Group`] the shell prints the verb
/// in, so the two axes cannot disagree — and the five verbs with no shell form are the CLIENT's own,
/// which is the group they were already in.
#[must_use]
pub fn subject_of(verb: Verb) -> ActionSubject {
    match verb.entry().shell.map(|shell| shell.group) {
        Some(Group::Window) => ActionSubject::Window,
        Some(Group::Pane) => ActionSubject::Pane,
        Some(Group::Session) => ActionSubject::Session,
        // The keyboard, the settings, an agent's reports, and sprag itself are all things a CLIENT
        // asks about rather than parts of the containment.
        Some(Group::Agent | Group::Keys | Group::Options | Group::Tool) | None => {
            ActionSubject::Client
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE TABLE IS THE ENUM: every variant declared in this file is in [`Verb::ALL`].**
    ///
    /// The one drift this module can carry, and the compiler cannot ask for it: an omitted variant
    /// still compiles, and every other test here iterates `ALL` — so a verb missing from it would
    /// be invisible to all of them. Counted out of THIS FILE'S OWN SOURCE, which is R322's
    /// instrument for the wire's methods, and for its reason: the source is the thing that ships.
    ///
    /// The needle is BUILT rather than written, so the assertion is not one of the strings it
    /// counts.
    #[test]
    fn the_table_holds_every_variant_of_the_enum() {
        let source = include_str!("vocabulary.rs");
        let head = source
            .split_once("pub enum Verb {")
            .expect("this file declares the enum")
            .1
            .split_once("\n}\n")
            .expect("the declaration ends")
            .0;
        // A variant is a line that is exactly an identifier and a comma at four-space indent —
        // built as a predicate rather than as a list of names, so this counts the DECLARATION and
        // not a copy of it.
        let declared = head
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.ends_with(',')
                    && line
                        .trim_end_matches(',')
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric())
                    && line.starts_with(|c: char| c.is_ascii_uppercase())
            })
            .count();
        assert_eq!(
            declared,
            Verb::ALL.len(),
            "the enum declares {declared} verbs and ALL holds {} — a verb missing from ALL is a \
             verb every other test in this module cannot see",
            Verb::ALL.len(),
        );
        // THE CONTROL: the count is not a constant this test could have hard-coded and still
        // passed. `Ls` is the first variant, so a parse that found nothing would also report zero.
        assert!(declared > 50, "the parse found {declared} variants");
    }

    /// Every verb can be SAID by somebody: a shell, a keystroke, or both.
    ///
    /// The one combination [`Entry`] can spell that means nothing — no shell form and a keystroke
    /// that cannot mean it — is a verb the product has and nobody can reach.
    #[test]
    fn every_verb_has_a_mouth() {
        for verb in Verb::ALL {
            assert!(
                verb.runs_in_shell() || verb.bindable(),
                "{} can be said by nobody",
                verb.name(),
            );
        }
    }

    /// Names are unique and every one round-trips through [`Verb::parse`].
    #[test]
    fn a_name_names_one_verb() {
        let mut names: Vec<&str> = Verb::ALL.iter().map(|verb| verb.name()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len(), "two verbs share a spelling");
        for verb in Verb::ALL {
            assert_eq!(Verb::parse(verb.name()), Some(verb), "{}", verb.name());
        }
        assert_eq!(Verb::parse("nonesuch"), None);
        // The flag spellings are the same two verbs, so `sprag -h` and `sprag help` cannot come
        // apart.
        assert_eq!(Verb::parse("-h"), Some(Verb::Help));
        assert_eq!(Verb::parse("--help"), Some(Verb::Help));
        assert_eq!(Verb::parse("-V"), Some(Verb::Version));
        assert_eq!(Verb::parse("--version"), Some(Verb::Version));
    }

    /// **THE KEYBOARD'S GAP IS WHAT THE TABLE SAYS IT IS** — the register's number, derived.
    ///
    /// R322 measured the gap by driving the binary once and writing the number down. This asserts
    /// it instead, in three halves that move in opposite directions: bindable verbs go UP as arms
    /// are built, [`Keystroke::NotBuilt`] goes DOWN, and a verb moved from `NotBuilt` to
    /// [`Keystroke::Cannot`] has to change a REASON here rather than quietly disappearing.
    #[test]
    fn the_keyboard_gap_is_what_the_table_says_it_is() {
        let bindable = Verb::ALL.iter().filter(|verb| verb.bindable()).count();
        let not_built = Verb::ALL
            .iter()
            .filter(|verb| verb.keystroke() == Keystroke::NotBuilt)
            .count();
        let refused = Verb::ALL
            .iter()
            .filter(|verb| matches!(verb.keystroke(), Keystroke::Cannot(_)))
            .count();
        assert_eq!(
            (bindable, not_built, refused),
            (23, 3, 28),
            "the keyboard reaches {bindable} verbs, {not_built} are a keystroke's to mean and are \
             not built, and {refused} are refused with a reason",
        );
        assert_eq!(bindable + not_built + refused, Verb::ALL.len());
        // The four are NAMED, so closing one is a change here and not a number that drifts.
        let pending: Vec<&str> = Verb::ALL
            .iter()
            .filter(|verb| verb.keystroke() == Keystroke::NotBuilt)
            .map(|verb| verb.name())
            .collect();
        assert_eq!(
            pending,
            ["run", "resize-window", "join-pane"],
            "the keyboard's remaining gap, by name",
        );
    }

    /// The forms the keyboard lists are exactly the bindable verbs, each spelling its own name
    /// first.
    #[test]
    fn the_bindable_forms_name_their_own_verbs() {
        let forms = bindable_forms();
        assert_eq!(
            forms.len(),
            Verb::ALL.iter().filter(|verb| verb.bindable()).count(),
        );
        for verb in Verb::ALL.into_iter().filter(|verb| verb.bindable()) {
            let form = forms
                .iter()
                .find(|form| form.split_whitespace().next() == Some(verb.name()))
                .unwrap_or_else(|| panic!("{} is bindable and unlisted", verb.name()));
            assert!(
                form.starts_with(verb.name()),
                "{form:?} must begin with the verb a user types",
            );
        }
    }

    /// The usage names every shell verb, and names no verb the shell does not run.
    ///
    /// The second half is what a hand-written usage could not promise: `switch-client` is a real
    /// verb of this vocabulary and telling a shell user to type it would be a lie.
    #[test]
    fn the_usage_is_the_shell_half_of_the_table() {
        let text = usage();
        for verb in Verb::ALL {
            let listed = text
                .lines()
                .any(|line| line.split_whitespace().next() == Some(verb.name()));
            assert_eq!(
                listed,
                verb.runs_in_shell(),
                "{} is {}listed in the usage and {}dispatched by the shell",
                verb.name(),
                if listed { "" } else { "not " },
                if verb.runs_in_shell() { "" } else { "not " },
            );
        }
        // Every group that has a verb prints its heading, and the order is the declaration's.
        let headings: Vec<&str> = text
            .lines()
            .filter(|line| {
                !line.trim().is_empty() && line.starts_with("  ") && !line.starts_with("    ")
            })
            .map(str::trim)
            .collect();
        assert_eq!(
            headings,
            [
                Group::Session.heading(),
                Group::Window.heading(),
                Group::Pane.heading(),
                Group::Agent.heading(),
                Group::Keys.heading(),
                Group::Options.heading(),
                Group::Tool.heading(),
            ],
        );
    }

    /// A verb with no shell form says what it IS, and one the shell runs says nothing of the kind.
    #[test]
    fn a_keystroke_only_verb_names_the_key_that_would_bind_it() {
        let sentence = Verb::SwitchClient
            .only_a_keystroke()
            .expect("switch-client has no shell form");
        assert!(
            sentence.contains("switch-client")
                && sentence.contains("bind-key")
                && sentence.contains("-n|-p|-l|-t"),
            "{sentence:?} must name the verb, the way to bind it, and the forms",
        );
        assert_eq!(Verb::Ls.only_a_keystroke(), None, "the shell runs `ls`");
    }

    /// Every reason ends in a clause that reads as one, and no two reasons say the same thing.
    #[test]
    fn each_refusal_reason_is_its_own_sentence() {
        let reasons = [
            NotAKeystroke::Answers,
            NotAKeystroke::NeedsWords,
            NotAKeystroke::EditsAFile,
            NotAKeystroke::TheAgentsOwn,
            NotAKeystroke::OutsideTheClient,
        ];
        let mut whys: Vec<&str> = reasons.iter().map(|reason| reason.why()).collect();
        let before = whys.len();
        whys.sort_unstable();
        whys.dedup();
        assert_eq!(before, whys.len(), "two reasons read identically");
        for why in whys {
            assert!(
                why.starts_with("it") && !why.ends_with('.'),
                "{why:?} is a clause a refusal ends with — it continues a sentence about the verb, \
                 so it names the verb as `it` and stops without punctuation",
            );
        }
        // Every reason is USED, so a rule nobody applies cannot sit here looking like policy.
        for reason in reasons {
            assert!(
                Verb::ALL
                    .iter()
                    .any(|verb| verb.keystroke() == Keystroke::Cannot(reason)),
                "{reason:?} is a reason no verb gives",
            );
        }
    }
}
