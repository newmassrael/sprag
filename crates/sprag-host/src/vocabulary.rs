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
//! * [`Agent::Tools`] names the MCP tools an AI in a pane calls, and `sprag-mcp`'s roster is held
//!   against this table in BOTH directions — so a tool it advertises and this table does not
//!   declare fails, and a verb whose declared tool the roster does not carry fails too.
//!
//! # THE THIRD MOUTH, and why it was a fourth catalogue until R335
//!
//! R323's finding was *"three hand-written lists said what sprag can do and nothing held any two of
//! them together"*, and it joined the CLI, the keyboard and `--help`. **The agent surface was not in
//! that join.** Measured at `9727042`: `sprag-mcp` advertised 29 tools, this table forced every verb
//! to decide what a KEYSTROKE may mean and forced nothing at all about an AGENT, and the whole
//! ARRANGEMENT family was half-served — an agent could `open_pane`, `close_pane`, `swap_pane` and
//! `resize_pane`, and could not take a pane out of a window (`break-pane`), put one back
//! (`join-pane`), place one beside another (`move-pane`) or fill a window with one (`zoom-pane`).
//! Nothing anywhere stated why, and an undocumented absence is not a decision.
//!
//! So [`Entry`] carries a third answer, in the same three shapes the keyboard's has, and a verb
//! added today cannot compile without deciding all three.
//!
//! # The gaps are countable now, on every axis
//!
//! [`Keystroke::NotBuilt`] is the honest third answer, and it is what stops this table from lying
//! in the other direction: `run` is a verb a keystroke COULD mean, and sprag does not bind it
//! yet. Filing it under [`NotAKeystroke`] would be a refusal that is not true; leaving it out would
//! put it back in the "typo" sentence. Counting them is one test
//! ([`the_keyboard_gap_is_what_the_table_says_it_is`](self)), so the register's number is derived
//! from the code rather than re-measured by hand each round.
//!
//! [`Shell::NotBuilt`] and [`Agent::NotBuilt`] are the same answer on the other two mouths, and the
//! shell one had to exist the moment this table stopped being a list of the CLI's verbs: three acts
//! the product performs — a pane's last command, its hyperlinks, its inline images — reach an agent
//! and no shell, and `Option<Shell>`'s [`None`] could only say *"a shell cannot"*, which for those
//! three is false.

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
    /// `resources` — what each pane is TAKING of the machine.
    Resources,
    /// `grant` — what ONE pane is ALLOWED of it.
    Grant,
    /// `select-pane` — which pane the session is on.
    SelectPane,
    /// `swap-pane` — two panes trade places.
    SwapPane,
    /// `split-window` — divide a pane and put a shell in the half it opens.
    SplitWindow,
    /// `kill-pane` — end a pane, and whatever the cascade takes with it.
    KillPane,
    /// `stop-job` — end what a pane is RUNNING, and leave the pane standing.
    StopJob,
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
    /// `read-last-command` — the last command a pane's shell ran, its output and its status.
    ///
    /// A different act from [`CapturePane`](Self::CapturePane), which answers with a pane's WHOLE
    /// text: this reads the shell-integration marks (OSC 133) the pane's child wrote, so it can say
    /// where one command's output starts and ends where reading the screen cannot.
    ReadLastCommand,
    /// `pane-links` — the hyperlinks a pane is showing (OSC 8), with the text each sits under.
    PaneLinks,
    /// `pane-images` — the inline images a pane is showing (Kitty graphics, Sixel), and where.
    PaneImages,
    /// `agent` — what the AI in a pane is doing.
    Agent,
    /// `answer-pane` — ANSWER the question the AI in a pane has stopped to ask.
    ///
    /// The other half of [`Agent`](Self::Agent), and the half that did not exist. That verb can say
    /// a pane is `blocked` and — since R367 — what it is asking, option by option, including which
    /// one a bare Enter would take. Answering it was reachable only by declaring a consent in
    /// advance on a LOOP, which a person or an agent reading a neighbour's screen has not got.
    ///
    /// ⚠ What they had instead was `send-keys`: a digit and an Enter, with none of the guarantees
    /// [`sprag_plugin::Consent`] exists for. **The unsafe act was the reachable one and the safe
    /// act was not expressible**, on the one surface that publishes the question.
    AnswerPane,
    /// `report-agent` — an agent says what it is doing.
    ReportAgent,
    /// `release-agent` — an agent stops claiming a pane.
    ReleaseAgent,
    /// `orchestrate` — start a BOUNDED AI↔AI loop, and get its run id back.
    ///
    /// The verb the README's first line describes and that this table did not have. The platform
    /// held four plugins, an iteration ceiling, a typed cost ceiling, a cancel flag and
    /// agent-state-aware supervision, and the only way to start one was to hand-write
    /// `scene/invoke /sprag_plugins/external/run` — so the guardrails that make a loop safe were
    /// reachable by nobody the platform was built for.
    Orchestrate,
    /// `runs` — the loops this daemon is running, and how the finished ones ended.
    Runs,
    /// `cancel-run` — ask a run to stop at its next step.
    CancelRun,
    /// `stand-down` — ask a run to finish what it is doing and then stop.
    ///
    /// ⚠⚠⚠ THE SECOND THING ANYBODY CAN SAY TO A RUN, and the first that does not throw the turn in
    /// flight away. `cancel-run` stops a loop mid-turn and loses whatever the agent had done; this
    /// waits for the milestone it was working toward, takes its closing account, and converges. **A
    /// person leaving for the day wants this one**, and until it existed the only sentence available
    /// was the one that discards work.
    StandDown,
    /// `hold-run` — halt a run between turns so a person can read its pane.
    ///
    /// ⚠⚠⚠⚠⚠ THE THIRD THING ANYBODY CAN SAY TO A RUN, AND THE ONLY ONE THEY CAN TAKE BACK. Its two
    /// neighbours both END the loop and differ only in what that costs the turn in flight. Neither is
    /// *wait, let me look at this* — so a person who wanted to read a pane had to choose between
    /// losing a turn and ending the run. `ai_loop.scxml` has had the edge for this since R378 with
    /// nothing in the product able to raise it (register item 9).
    HoldRun,
    /// `resume-run` — send a held run on again.
    ///
    /// ⚠ A verb of its own rather than a flag on [`HoldRun`](Self::HoldRun), because what a person
    /// types is a sentence and *"hold this, but the other way"* is not one. They are one wire call
    /// with a direction underneath.
    ResumeRun,
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
    /// `doctor` — what is WRONG with the machine the panes run on.
    Doctor,
    /// `words` — the closed vocabularies a run's answer speaks, asked of this build.
    Words,
    /// `disposition` — what happens next to a run that ended, per ending, asked of this build.
    Disposition,
    /// `my-runs` — which runs THIS conversation is on, asked by the caller about itself.
    MyRuns,
    /// `daemons` — WHICH daemons are running and on which sockets, asked of the machine.
    Daemons,
    /// `show-grammar` — HOW TO CALL the daemon's own verbs, asked of the daemon.
    ShowGrammar,
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

/// What a SHELL can say by a verb — the same three answers the other two mouths give.
///
/// It was `Option<Shell>` until R335, where [`None`] meant *"a shell cannot say this"* and carried
/// [`Verb::DetachClient`]'s reason in prose. That conflation became false the moment this table
/// started holding the whole product rather than the CLI's own list: [`Verb::PaneLinks`] is an act
/// sprag performs, a shell could perfectly well ask for it, and nobody has built the verb. Saying
/// *cannot* about it would be a refusal that is not true — [`Keystroke::NotBuilt`]'s argument, one
/// mouth over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shell {
    /// `sprag <name>` dispatches it, with these arguments.
    ///
    /// Spelled as the usage spells them and WITHOUT the verb's own name, which [`usage`] writes
    /// from [`Entry::name`] so a synopsis cannot disagree with the name above it.
    Runs(&'static str),
    /// A shell COULD say it and sprag dispatches nothing for it yet.
    NotBuilt,
    /// A shell cannot say it, for this reason.
    Cannot(NotAShellVerb),
}

/// Why a shell cannot say a verb.
///
/// A closed set, [`NotAKeystroke`]'s rule: one rule per reason rather than a sentence per verb, so
/// a verb that fits none of them is a rule this project has not thought about yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotAShellVerb {
    /// It acts on THE CLIENT that pressed the key, and a shell has no client.
    ///
    /// The five verbs this covers would each have to name a client and then mean something
    /// different from what the key means — `sprag detach-client` is not a smaller version of
    /// pressing the binding, it is a different act with a different subject.
    NoClientOfIts,
}

impl NotAShellVerb {
    /// The clause a refusal ends with, in the user's terms — [`NotAKeystroke::why`]'s shape.
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::NoClientOfIts => {
                "it acts on the client that pressed the key, and a shell has no client"
            }
        }
    }
}

/// What an AGENT can ask for by a verb — the third mouth, and the one nothing forced until R335.
///
/// The reader here is an AI running inside a pane, talking to `sprag-mcp` over MCP. The three
/// answers are [`Keystroke`]'s three, for its reasons: a caller reading a gap has to know whether
/// the act is refused, missing, or spelled differently from what they guessed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agent {
    /// The MCP tools that mean this verb, by the names `tools/list` advertises.
    ///
    /// A SLICE and not one name, because the agent surface deliberately splits some verbs by what
    /// an agent is doing rather than by what the act is: `find` is `find_in_pane` and
    /// `regex_in_pane`, `send-keys` is `send_keys` and `write_pane`, `agent` is `agent_state` and
    /// `agent_explain`. That is the honest half of the join — the two vocabularies differ in
    /// PURPOSE and not only in coverage — and a slice says so without pretending either surface is
    /// a renaming of the other.
    Tools(&'static [&'static str]),
    /// An agent COULD ask for it and sprag serves no tool for it yet.
    NotBuilt,
    /// An agent cannot ask for it, for this reason.
    Cannot(NotAnAgents),
}

/// Why an agent cannot ask for a verb.
///
/// Four rules for twenty verbs, [`NotAKeystroke`]'s discipline: the sentence a reader gets has to
/// say which RULE they hit, because that is what tells them whether to look for another tool or to
/// stop looking. They are NOT [`NotAKeystroke`]'s five reasons re-used — *"a keystroke has nowhere
/// to put an answer"* is the opposite of true for an agent, which is a reader of answers and little
/// else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotAnAgents {
    /// It reaches OUTSIDE the session the agent works in — another session, the daemon itself, or a
    /// display process.
    ///
    /// The bound `sprag-mcp` states to its caller in its own instructions: these tools act on YOUR
    /// session. An agent that could end the daemon could end the conversation it is having.
    OutsideItsSession,
    /// It is the PERSON's own — their workspace's name, their configuration file, their keys.
    ///
    /// The authorship rule the pane and window verbs apply, at a level where there is nothing to
    /// own: a session has no `opened_by` because an agent never opens one, and a config file is the
    /// person's on every session at once.
    ThePersonsOwn,
    /// It acts on a CLIENT that is typing, and an agent has no client.
    ///
    /// [`NotAShellVerb::NoClientOfIts`]'s rule, which is what makes these five verbs the only ones
    /// refused on two mouths for the same fact: they are a person-at-a-keyboard's verbs, and
    /// neither of the other two mouths is one.
    NoClientOfIts,
    /// The agent already says it another way, and a tool would be a SECOND authority for one fact.
    ///
    /// `report-agent` and `hook` are how an agent's own state reaches the daemon, through its hook
    /// process; `help` is `tools/list`, which every MCP client reads before it calls anything. A
    /// tool for either would let one agent say two different things about itself.
    SaidAnotherWay,
}

impl NotAnAgents {
    /// The clause a refusal ends with, in the reader's terms — [`NotAKeystroke::why`]'s shape.
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::OutsideItsSession => {
                "it reaches outside the session the agent works in, and these tools act on that \
                 session"
            }
            Self::ThePersonsOwn => {
                "it is the person's own — their session's name, their config file, or their keys"
            }
            Self::NoClientOfIts => {
                "it acts on a client somebody is typing at, and an agent has no client"
            }
            Self::SaidAnotherWay => {
                "the agent already says it another way, and a tool would be a second authority for \
                 one fact"
            }
        }
    }
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

/// WHAT a verb is about — and, for the verbs a shell runs, which block of `sprag --help` prints it.
///
/// The ORDER is the declaration order and it is the reading order of the help: outward through the
/// containment the product itself uses (session, window, pane), then the surfaces that are about
/// somebody else (an agent), then the ones about sprag itself, then the client's own.
///
/// It exists because a usage text is a DOCUMENT rather than a list — the grouping is what makes the
/// verbs readable — and a group is the one piece of that document a verb has to carry for the
/// document to be derivable from the verbs — 50 of them print, and the three mouths together hold
/// 59.
///
/// **It moved out of [`Shell`] at R335**, where it had been a property of the CLI's spelling. That
/// was wrong twice: a verb no shell runs still has a subject ([`Verb::SwitchClient`] is about a
/// client), and [`subject_of`] was reading the group through the shell form and answering
/// `Client` for every verb without one — which was right for the five it then had and would have
/// been wrong for the first pane verb the shell did not run.
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
    /// A BOUNDED LOOP the platform drives — one AI against another, or one against a pane.
    ///
    /// # Why the loop is not filed under [`Agent`](Self::Agent)
    ///
    /// The agent group is about ONE pane's occupant: what it is doing, what it is blocked on, who
    /// owns it. A run is about a loop over several — and one of the four plugins (`pipe`) has no
    /// agent in it at all, while another (`dialogue`) spawns the panes it drives. Filing them
    /// together would put the product's headline feature under a heading about somebody else's
    /// process.
    ///
    /// It is also the honest reading of the README, which names *"AI↔AI 오케스트레이션 루프"* as
    /// what sprag is FOR. A heading of its own is what a person scanning `sprag --help` for that
    /// sentence finds.
    Orchestration,
    /// The keyboard.
    Keys,
    /// The settings.
    Options,
    /// sprag itself.
    Tool,
    /// The CLIENT a person is typing at — the display process, and where it is pointed.
    ///
    /// No verb of this group runs in a shell (that is what the group MEANS), so [`usage`] never
    /// prints its heading. It is still a real group rather than a `None`: these verbs have a
    /// subject, and [`subject_of`] answers from it instead of from an absence.
    Client,
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
            Self::Orchestration => "orchestration",
            Self::Keys => "keys",
            Self::Options => "options",
            Self::Tool => "sprag",
            Self::Client => "client",
        }
    }
}

/// Everything the vocabulary knows about one verb.
///
/// Answered by ONE exhaustive match ([`Verb::entry`]) rather than by a method per property, and
/// that is the load-bearing choice: five matches are five chances to forget an arm, where one match
/// makes the compiler ask for every property of a new verb at the moment it is added.
///
/// **THE THREE MOUTHS ARE THE THREE FIELDS**, and each answers the same three-shaped question — it
/// says it like this / it could and nobody built it / it cannot, for this reason. A verb is added by
/// deciding all three at once or it does not compile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    /// The verb as a user spells it — the FIRST WORD of a command line or of a binding.
    pub name: &'static str,
    /// What the verb is about, and which block of the help prints it.
    pub group: Group,
    /// What a shell can say by it.
    pub shell: Shell,
    /// What a keystroke can mean by it.
    pub key: Keystroke,
    /// What an agent can ask for by it.
    pub agent: Agent,
}

impl Verb {
    /// Every verb, in the order the help prints them.
    ///
    /// The one hand-written sequence in this module, and the only drift it can carry is an OMISSION
    /// — which [`the_table_holds_every_variant_of_the_enum`](self) catches by counting the enum's
    /// own variants out of this file's source, the instrument R322 built for the wire's methods.
    pub const ALL: [Self; 73] = [
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
        Self::Resources,
        Self::Grant,
        Self::SelectPane,
        Self::SwapPane,
        Self::SplitWindow,
        Self::KillPane,
        Self::StopJob,
        Self::ResizePane,
        Self::ZoomPane,
        Self::RenamePane,
        Self::SendKeys,
        Self::CapturePane,
        Self::ReadLastCommand,
        Self::PaneLinks,
        Self::PaneImages,
        Self::Agent,
        Self::AnswerPane,
        Self::ReportAgent,
        Self::ReleaseAgent,
        Self::DisplayMessage,
        Self::InstallHooks,
        Self::UninstallHooks,
        Self::ListHooks,
        Self::Hook,
        Self::Events,
        Self::Orchestrate,
        Self::Runs,
        Self::CancelRun,
        Self::StandDown,
        Self::HoldRun,
        Self::ResumeRun,
        Self::ListKeys,
        Self::BindKey,
        Self::UnbindKey,
        Self::ShowOptions,
        Self::SetOption,
        Self::Version,
        Self::Help,
        Self::Doctor,
        Self::Words,
        Self::Disposition,
        Self::MyRuns,
        Self::Daemons,
        Self::ShowGrammar,
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
        // A tuple so the 59 arms below read as a TABLE rather than as 59 struct literals — five
        // columns, in the order a reader asks them: what it is called, what it is about, and then
        // the three mouths.
        let (name, group, shell, key, agent) = match self {
            // ── sessions ────────────────────────────────────────────────────────────────────────
            Self::Ls => (
                "ls",
                Group::Session,
                Shell::Runs(""),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["list_sessions"]),
            ),
            // An agent COULD be told who is watching, and that is not idle: `display_message`
            // already has to say "no window is attached, so treat this as undelivered", which is
            // the same fact answered only as a side effect of sending something.
            Self::ListClients => (
                "list-clients",
                Group::Session,
                Shell::Runs("[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // BINDABLE SINCE R323, and the arm that made the keyboard reach the session level at
            // all: a person could create a window with `prefix c` and had no key that creates a
            // SESSION. The name the CLI verb takes is the daemon's to generate when a keystroke
            // does not give one, which is what makes this bindable where `ssh` is not.
            Self::New => (
                "new",
                Group::Session,
                Shell::Runs("[name] [-a]"),
                Keystroke::Means("new"),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            // A display PROCESS is launched, and the client pressing the key is one already.
            Self::Attach => (
                "attach",
                Group::Session,
                Shell::Runs("NAME [--no-wait | --tui | --remote HOST]"),
                Keystroke::Cannot(NotAKeystroke::OutsideTheClient),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            Self::Ssh => (
                "ssh",
                Group::Session,
                Shell::Runs("[user@]host [-p PORT] [-L FWD]… [--tmux[=NAME]] [-- command…]"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            // TWO tools for one verb, and the reason is [`Agent::Tools`]'s: an agent asking for a
            // literal needle and one asking for a pattern are two different calls to write, where
            // a shell flag is one more word to type.
            Self::Find => (
                "find",
                Group::Session,
                Shell::Runs("NEEDLE [-t SESSION] [--pane PANE] [--regex]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["find_in_pane", "regex_in_pane"]),
            ),
            Self::WaitForOutput => (
                "wait-for-output",
                Group::Session,
                Shell::Runs("--pane PANE NEEDLE [-t SESSION] [--regex]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["wait_for_output"]),
            ),
            // A keystroke could mean "show me this pane's project commands and type the one I
            // pick" — which is the GUI palette's own gesture, and `sprag-tui` has no palette. The
            // pick would have to carry what it is FOR (`chooser::Pick::commit` goes to a row), so
            // this is a surface this round did not build rather than an act a key cannot mean.
            Self::Run => (
                "run",
                Group::Session,
                Shell::Runs("[NAME] [-t SESSION] [--pane PANE]"),
                Keystroke::NotBuilt,
                Agent::NotBuilt,
            ),
            // The name of the workspace a PERSON is keeping, which no agent opened and which every
            // other client of the session addresses it by.
            Self::RenameSession => (
                "rename-session",
                Group::Session,
                Shell::Runs("[-t SESSION] NEW"),
                Keystroke::Means("rename-session"),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            // BINDABLE SINCE R323. A keystroke means THIS client's session, which is the one thing
            // a client always knows and the CLI never does — so the verb needs its NAME in a shell
            // and needs nothing at all from a key.
            // An agent that ended the session would end the pane it is running in, and every pane
            // it was reading. The bound is the same one `sprag-mcp` states in its instructions.
            Self::KillSession => (
                "kill-session",
                Group::Session,
                Shell::Runs("NAME"),
                Keystroke::Means("kill-session"),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            // The daemon every attached client is looking at, this one included. A key for it would
            // end the screen it was pressed on, and the sentence says which rule that is.
            Self::KillServer => (
                "kill-server",
                Group::Session,
                Shell::Runs("[--purge]"),
                Keystroke::Cannot(NotAKeystroke::OutsideTheClient),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            // ── windows ─────────────────────────────────────────────────────────────────────────
            Self::Windows => (
                "windows",
                Group::Window,
                Shell::Runs("[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["list_windows"]),
            ),
            Self::NewWindow => (
                "new-window",
                Group::Window,
                Shell::Runs("[-d] [name] [-t SESSION]"),
                Keystroke::Means("new-window"),
                Agent::Tools(&["open_window"]),
            ),
            Self::SelectWindow => (
                "select-window",
                Group::Window,
                Shell::Runs("<NAME|-n|-p> [-t SESSION]"),
                Keystroke::Means("select-window -n|-p|-t <window>"),
                Agent::Tools(&["select_window"]),
            ),
            Self::RenameWindow => (
                "rename-window",
                Group::Window,
                Shell::Runs("[window] NAME [-t SESSION]"),
                Keystroke::Means("rename-window"),
                Agent::Tools(&["rename_window"]),
            ),
            Self::KillWindow => (
                "kill-window",
                Group::Window,
                Shell::Runs("[window] [-t SESSION]"),
                Keystroke::Means("kill-window"),
                Agent::Tools(&["close_window"]),
            ),
            // WHERE a window sits in the person's own window list — an order they read left to
            // right and reach with `select-window -n`. An agent owns windows it opened and does not
            // own the ORDER, which is one list shared by every window in the session.
            Self::MoveWindow => (
                "move-window",
                Group::Window,
                Shell::Runs(
                    "[window] <--first | --last | -n | -p | --before W | --after W> [-t SESSION]",
                ),
                Keystroke::Means(
                    "move-window --first|--last|-n|-p|--before [<window>]|--after <window>",
                ),
                Agent::NotBuilt,
            ),
            // BINDABLE SINCE R331, and the SIZE travels where a window name may not: a rectangle is
            // a decision a config file can fix, which is exactly what `rename-window`'s rule asks of
            // an argument. What had to be built was not the grammar but the ANSWER — a pin stored
            // under a policy that does not read it moves nothing, so until the daemon said which
            // policy it was under, a key for this could not tell a person their resize had done
            // nothing (`wire::WindowPin`).
            Self::ResizeWindow => (
                "resize-window",
                Group::Window,
                Shell::Runs(
                    "[window] <-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u> [-t SESSION]",
                ),
                Keystroke::Means("resize-window -x COLS -y ROWS|-a|-A|-L/-R/-U/-D N|-u"),
                Agent::Tools(&["resize_window"]),
            ),
            // BINDABLE SINCE R323, on tmux's own `prefix !`. The pane is the focused one and the
            // name is optional, so a keystroke carries everything this verb needs — which is why it
            // was the loudest of the fifteen refusals R322 measured.
            // ⚠ THE AGENT ARM IS R335's, and it is the half of item 56 that was a DEFECT rather
            // than a decision: an agent could open a pane, close it, trade its place and resize it,
            // and could not move one BETWEEN WINDOWS at all. The gate is the authorship rule
            // `swap_pane` already states — a pane the agent opened is one it has a basis to move,
            // and a person's is not.
            Self::BreakPane => (
                "break-pane",
                Group::Window,
                Shell::Runs("PANE [name] [-t SESSION]"),
                Keystroke::Means("break-pane"),
                Agent::Tools(&["break_pane"]),
            ),
            // BINDABLE SINCE R329. The pane is the focused one; the WINDOW is a row the person
            // PICKS, and a binding therefore names neither — `move-pane`'s shape one level up the
            // tree. What had to be built first was not the surface but the ADDRESS: a picked row
            // carries a window IDENTITY and this verb's action took a NAME, so committing a pick
            // meant sending its label, which lands the join wherever that name has got to.
            Self::JoinPane => (
                "join-pane",
                Group::Window,
                Shell::Runs("PANE WINDOW [-t SESSION]"),
                Keystroke::Means("join-pane"),
                Agent::Tools(&["join_pane"]),
            ),
            // BINDABLE SINCE R328. The CLI form names both panes; a binding names NEITHER — the
            // mover is the focused pane and the target is a row the person PICKS, which is what
            // `chooser::Errand` was built to let a pick mean. The flags are `split-window`'s, and
            // for the same question: which half of the target the arrival takes.
            Self::MovePane => (
                "move-pane",
                Group::Window,
                Shell::Runs("PANE -h|-v [-b] TARGET [-t SESSION]"),
                Keystroke::Means("move-pane -h|-v [-b]"),
                Agent::Tools(&["move_pane"]),
            ),
            // ── panes ───────────────────────────────────────────────────────────────────────────
            // ⚠ `[PANE]` NAMES A WINDOW HERE — register item 782, and the same grammar `processes`
            // below already has. A pane is how every verb on this surface addresses a window
            // (register item 686), so these two say it the same way rather than growing a second
            // vocabulary for it.
            Self::Panes => (
                "panes",
                Group::Pane,
                Shell::Runs("[PANE] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["list_panes"]),
            ),
            Self::Layout => (
                "layout",
                Group::Pane,
                Shell::Runs("[PANE] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["pane_layout"]),
            ),
            Self::Processes => (
                "processes",
                Group::Pane,
                Shell::Runs("[PANE] [-t SESSION] [-a]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["pane_processes"]),
            ),
            Self::Resources => (
                "resources",
                Group::Pane,
                Shell::Runs("[PANE] [-t SESSION] [-a]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["pane_resources"]),
            ),
            Self::Grant => (
                "grant",
                Group::Pane,
                Shell::Runs("<PANE> [--share N] [--memory MIB] [--processes N] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Tools(&["grant_pane"]),
            ),
            Self::SelectPane => (
                "select-pane",
                Group::Pane,
                Shell::Runs("<PANE | -L|-R|-U|-D [--from PANE]> [-t SESSION]"),
                Keystroke::Means("select-pane -L|-R|-U|-D|-t :.+"),
                Agent::Tools(&["select_pane"]),
            ),
            Self::SwapPane => (
                "swap-pane",
                Group::Pane,
                Shell::Runs("[PANE] <WITH | -L|-R|-U|-D> [-t SESSION]"),
                Keystroke::Means("swap-pane -L|-R|-U|-D"),
                Agent::Tools(&["swap_pane"]),
            ),
            Self::SplitWindow => (
                "split-window",
                Group::Pane,
                Shell::Runs("[-h|-v [-b] [PANE]] [-c DIR] [-w WINDOW] [-- command…] [-t SESSION]"),
                Keystroke::Means("split-window -h|-v [-b]"),
                Agent::Tools(&["open_pane"]),
            ),
            Self::KillPane => (
                "kill-pane",
                Group::Pane,
                Shell::Runs("[PANE] [-t SESSION]"),
                Keystroke::Means("kill-pane"),
                Agent::Tools(&["close_pane"]),
            ),
            // ⚠ `NotBuilt` and NOT `Cannot`, and the distinction is the honest one here. A person
            // at the keyboard has `Ctrl-C` already — but that is a BYTE, and this verb exists
            // because a byte is not a stop: on a terminal whose `ISIG` a program turned off, the
            // keyboard cannot end the job and this can. So a binding would be worth having and
            // sprag has not built one; filing it under `Cannot` would print a reason that is false.
            Self::StopJob => (
                "stop-job",
                Group::Pane,
                Shell::Runs("PANE [--signal interrupt|terminate|kill] [-t SESSION]"),
                Keystroke::NotBuilt,
                Agent::Tools(&["stop_job"]),
            ),
            Self::ResizePane => (
                "resize-pane",
                Group::Pane,
                Shell::Runs("[PANE] <-x COLS -y ROWS | -L|-R|-U|-D [N]> [-t SESSION]"),
                Keystroke::Means("resize-pane -L|-R|-U|-D [N]"),
                Agent::Tools(&["resize_pane"]),
            ),
            Self::ZoomPane => (
                "zoom-pane",
                Group::Pane,
                Shell::Runs("[PANE] [-u] [-t SESSION]"),
                Keystroke::Means("zoom-pane [-Z|-u]"),
                Agent::Tools(&["zoom_pane"]),
            ),
            Self::RenamePane => (
                "rename-pane",
                Group::Pane,
                Shell::Runs("PANE <NAME | --clear> [-t SESSION]"),
                Keystroke::Means("rename-pane"),
                Agent::Tools(&["rename_pane"]),
            ),
            // The pane the person is on already receives their keys; what this verb adds is the
            // WORDS, and a binding cannot carry those.
            // TWO tools again: `write_pane` is the words and `send_keys` is the key names, which
            // the shell spells as one verb and a `-l` flag.
            Self::SendKeys => (
                "send-keys",
                Group::Pane,
                Shell::Runs("PANE [-l] KEY… [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Tools(&["write_pane", "send_keys"]),
            ),
            Self::CapturePane => (
                "capture-pane",
                Group::Pane,
                Shell::Runs("PANE [-p] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["read_pane"]),
            ),
            // ── THE THREE ACTS WITH NO SHELL FORM YET ───────────────────────────────────────────
            // Added to this table at R335 by the join it built, and they are the finding rather
            // than the fix: the agent surface served three things the product does that no verb
            // named, so they were invisible to `sprag --help`, to `list-keys`, and to every sweep
            // that starts from this array. `Shell::NotBuilt` is the honest answer for all three —
            // a shell could ask any of them, and nobody has written the dispatch.
            Self::ReadLastCommand => (
                "read-last-command",
                Group::Pane,
                Shell::NotBuilt,
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["read_last_command"]),
            ),
            Self::PaneLinks => (
                "pane-links",
                Group::Pane,
                Shell::NotBuilt,
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["read_pane_links"]),
            ),
            Self::PaneImages => (
                "pane-images",
                Group::Pane,
                Shell::NotBuilt,
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["read_pane_images"]),
            ),
            // ── agents ──────────────────────────────────────────────────────────────────────────
            // The verdict and the REASON for it are one verb with two tools, because an agent asks
            // the second only when the first surprised it.
            Self::Agent => (
                "agent",
                Group::Agent,
                Shell::Runs("[PANE] [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["agent_state", "agent_explain"]),
            ),
            // ⚠⚠ NOT `Keystroke::NeedsWords` BY REFLEX — it is, and for a sharper reason than
            // `send-keys`'s. A binding would fix ONE question and ONE option forever, and a
            // consent that outlives the dialog it was written from is the exact shape
            // `sprag_plugin::Consent` refuses: it authorises an answer to a question somebody read
            // once, not to whatever is on the screen when a key is pressed.
            Self::AnswerPane => (
                "answer-pane",
                Group::Agent,
                Shell::Runs("PANE --asked TEXT --answer TEXT [-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Tools(&["answer_pane"]),
            ),
            Self::ReportAgent => (
                "report-agent",
                Group::Agent,
                Shell::Runs(
                    "<working|blocked|idle> [-t SESSION] [--pane PANE] [--source S] [--name AGENT] \
                     [--seq N]",
                ),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
                Agent::Cannot(NotAnAgents::SaidAnotherWay),
            ),
            Self::ReleaseAgent => (
                "release-agent",
                Group::Agent,
                Shell::Runs("[-t SESSION] [--pane PANE]"),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
                Agent::Cannot(NotAnAgents::SaidAnotherWay),
            ),
            Self::DisplayMessage => (
                "display-message",
                Group::Agent,
                Shell::Runs("[-t SESSION] [-c CLIENT] [-s note|warn|alert] MESSAGE"),
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Tools(&["display_message"]),
            ),
            Self::InstallHooks => (
                "install-hooks",
                Group::Agent,
                Shell::Runs("[AGENT…] [--yes] [--dry-run]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            Self::UninstallHooks => (
                "uninstall-hooks",
                Group::Agent,
                Shell::Runs("[AGENT…] [--yes] [--dry-run]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            Self::ListHooks => (
                "list-hooks",
                Group::Agent,
                Shell::Runs(""),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // **NAMED IN THE HELP SINCE R323, and it was dispatched and undocumented before.** It
            // is an agent's hook process reporting on the agent's behalf, so a person will only
            // ever run it while working out why their hooks are quiet — which is exactly when a
            // verb missing from the usage costs an hour.
            Self::Hook => (
                "hook",
                Group::Agent,
                Shell::Runs("EVENT   (an agent's hook; payload on stdin)"),
                Keystroke::Cannot(NotAKeystroke::TheAgentsOwn),
                Agent::Cannot(NotAnAgents::SaidAnotherWay),
            ),
            // `wait_for_change` is the FOLLOW half (`events -f`), which is the only half an agent
            // has a use for: it parks until something moves and answers what did.
            Self::Events => (
                "events",
                Group::Agent,
                Shell::Runs("[-t SESSION] [--since N] [-f [--pane PANE] [--kind KIND]…]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["wait_for_change"]),
            ),
            // ── orchestration ───────────────────────────────────────────────────────────────────
            // THE THREE VERBS THE README'S FIRST LINE DESCRIBES, and which this table did not have
            // until R355. The loop was BUILT — four plugins, an SCXML driver, `Guardrails`, a
            // typed `Cost` that cannot bind bytes with a token budget, a cancel flag, agent-state
            // aware supervision — and reachable from no mouth at all: no CLI verb, no MCP tool, no
            // keystroke, no palette row. `sprag_host::vocabulary`, documented as the ONE list of
            // what this product does, did not mention plugins, orchestration or dialogue anywhere,
            // so the list of what the product does omitted what the README says it is FOR.
            //
            // ⚠ THE ARGUMENTS ARE NOT SPELLED IN THE SHELL FORM BELOW, and that is the round's own
            // finding rather than an omission: `orchestrate`'s arguments are FOUR forms, one per
            // plugin, and the daemon publishes them ([`crate::wire::PluginGrammar`]). A synopsis
            // here would be a fifth copy of a table that already exists and that a `show-grammar`
            // client already reads — the exact defect this module was written to end, one surface
            // along. So the usage names the discriminator and points at the door that answers.
            Self::Orchestrate => (
                "orchestrate",
                Group::Orchestration,
                Shell::Runs(
                    " PLUGIN [--ARG VALUE]… [-t SESSION] [--wait]  (--help lists each form)",
                ),
                // Its whole content is the words: a stimulus, a prompt, a seed. A binding would
                // fix one and mean it forever — `send-keys`'s rule, and `display-message`'s.
                Keystroke::Cannot(NotAKeystroke::NeedsWords),
                Agent::Tools(&["orchestrate"]),
            ),
            Self::Runs => (
                "runs",
                Group::Orchestration,
                Shell::Runs("[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["list_runs"]),
            ),
            Self::CancelRun => (
                "cancel-run",
                Group::Orchestration,
                Shell::Runs(" ID [-t SESSION]"),
                // ⚠ `NotBuilt`, not `NeedsWords`, and the difference is a real one. A binding that
                // fixed ONE run id would be useless — that is the NeedsWords shape — but "cancel
                // every run I started" is an act a key could perform and nobody has built the verb
                // for it. Filing it under a refusal would print a reason that is not true.
                Keystroke::NotBuilt,
                Agent::Tools(&["cancel_run"]),
            ),
            Self::StandDown => (
                "stand-down",
                Group::Orchestration,
                Shell::Runs(" ID [-t SESSION]"),
                // ⚠ `NotBuilt` for `cancel-run`'s reason exactly: "stand every run I started down"
                // is an act a key could perform, and nobody has built the verb for it. ⚠⚠ It is the
                // BETTER candidate of the two for a key — a person leaving their desk wants the one
                // that keeps the work, and that is the moment a keystroke is reached for.
                Keystroke::NotBuilt,
                // ⚠ NOT BUILT rather than refused: a supervising agent standing ANOTHER run down is
                // a legitimate ask and the mouth simply has no tool for it yet. ⚠⚠ What would be a
                // refusal is a run standing ITSELF down — that is the loop deciding it is finished,
                // which is `north_star`'s job and not an order's — but that is a different verb from
                // this one and does not belong in this row.
                Agent::NotBuilt,
            ),
            Self::HoldRun => (
                "hold-run",
                Group::Orchestration,
                Shell::Runs(" ID [-t SESSION]"),
                // ⚠⚠⚠ THE BEST CANDIDATE FOR A KEY OF THE THREE, and still `NotBuilt`: *stop what
                // you are doing so I can read this* is the one order somebody wants to give WITHOUT
                // leaving the pane they are looking at — which is exactly what a keystroke is for.
                // Filed here rather than built because a key names no run, and this verb needs one.
                Keystroke::NotBuilt,
                // ⚠ A supervising agent holding ANOTHER run while it reads that run's pane is the
                // same legitimate ask `stand-down`'s row records, and the mouth has no tool for it
                // yet either.
                Agent::NotBuilt,
            ),
            Self::ResumeRun => (
                "resume-run",
                Group::Orchestration,
                Shell::Runs(" ID [-t SESSION]"),
                Keystroke::NotBuilt,
                Agent::NotBuilt,
            ),
            // ── keys ────────────────────────────────────────────────────────────────────────────
            // THE ONE ANSWERING VERB THAT IS BOUND, and the reason is the whole content of
            // [`NotAKeystroke::Answers`]: this client has a VIEW for the answer
            // ([`crate::keyhelp`]). A verb refused for answering becomes bindable the day sprag
            // grows a surface for what it says.
            // An agent that could READ the bindings could tell a person which key to press — the
            // one thing on this surface it would use the keyboard for. Nobody has built it.
            Self::ListKeys => (
                "list-keys",
                Group::Keys,
                Shell::Runs("[-N]"),
                Keystroke::Means("list-keys"),
                Agent::NotBuilt,
            ),
            Self::BindKey => (
                "bind-key",
                Group::Keys,
                Shell::Runs("[-nr] [-T prefix|root] KEY ACTION…"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            Self::UnbindKey => (
                "unbind-key",
                Group::Keys,
                Shell::Runs("[-n] [-T prefix|root] KEY"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            // ── options ─────────────────────────────────────────────────────────────────────────
            Self::ShowOptions => (
                "show-options",
                Group::Options,
                Shell::Runs("[-v] [NAME] [-g]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            Self::SetOption => (
                "set-option",
                Group::Options,
                Shell::Runs("[-u] NAME [VALUE] [-g]"),
                Keystroke::Cannot(NotAKeystroke::EditsAFile),
                Agent::Cannot(NotAnAgents::ThePersonsOwn),
            ),
            // ── sprag itself ────────────────────────────────────────────────────────────────────
            // NOT built, and it is the gap with the sharpest edge: half a dozen tool answers warn
            // "the daemon is older than this tool", which is a version comparison an agent has no
            // way to make itself.
            Self::Version => (
                "version",
                Group::Tool,
                Shell::Runs("  (also --version, -V)"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // `tools/list` IS this verb on that mouth, and every MCP client reads it before it
            // calls anything — so a `help` tool would be a second usage for one surface.
            Self::Help => (
                "help",
                Group::Tool,
                Shell::Runs("     (also --help, -h)"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Cannot(NotAnAgents::SaidAnotherWay),
            ),
            // Under `sprag` and not under `panes`, though it sits beside `resources` in every
            // other way: its rows are CHECKS, not panes, and most of what it finds is not the
            // multiplexer's at all — a compiler cache the shells walk past, a kernel's swap tuning,
            // a batch workload competing at equal weight. The heading a person scans for it is the
            // one for verbs whose subject is neither a session, a window, a pane nor a key.
            //
            // NO SCOPE and no pane argument: a machine is not divided by session, and the pane
            // eating it may be in one the caller is not attached to. The narrowing `resources`
            // offers would answer a question nobody asks of a diagnosis.
            Self::Doctor => (
                "doctor",
                Group::Tool,
                Shell::Runs(""),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Tools(&["machine_health"]),
            ),
            // ⛔⛔⛔⛔⛔ **WHAT THIS BUILD'S RUN ROWS SPEAK, ASKED OF THE BUILD** — register item
            // 773. It needs NO DAEMON, `list-keys`'s reason one axis over: a closed vocabulary is
            // compiled in, so a person reading a finished run's word can be answered on a machine
            // whose daemon is down and from a directory that is not this tree.
            //
            // ⚠ NOT BOUND TO A KEY and NOT AN AGENT TOOL, for the reason every answering verb here
            // carries: this client has no view for it, and an agent reading a run gets the words in
            // the row itself rather than by asking twice.
            Self::Words => (
                "words",
                Group::Tool,
                Shell::Runs("[NAME]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // ⛔⛔⛔⛔⛔ **WHAT HAPPENS NEXT TO A RUN THAT ENDED, ASKED OF THE BUILD** — register
            // item 867, and `words`' neighbour rather than a sixth entry inside it: `words`
            // publishes VOCABULARIES (one list per name) and this publishes a PAIRING (an ending
            // and what follows it), which is a different shape and would have to be formatted
            // differently — the one thing `words` refuses to grow.
            //
            // ⚠⚠ It needs NO DAEMON, on `words`' terms exactly: the classification is
            // `OutcomeState::disposition`, compiled in, so a person holding a finished run's word
            // can be answered while the daemon that recorded it is gone. That is the whole reason
            // this exists — item 867's reader is `.githooks/loop-read.sh`, which runs at push time
            // with no daemon and reads run logs off disk.
            //
            // ⚠ NOT BOUND TO A KEY and NOT AN AGENT TOOL, for the reason every answering verb here
            // carries: this client has no view for it, and an agent reading a run already gets the
            // sentence in the row itself (`runs` prints it — item 827) rather than by asking twice.
            Self::Disposition => (
                "disposition",
                Group::Tool,
                Shell::Runs("[OUTCOME]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // ⛔⛔⛔⛔⛔ **WHICH RUNS THE CALLER IS ON, ASKED BY THE CALLER ABOUT ITSELF** —
            // register item 865's ⑷, and the one direction its other halves could not reach: ⑴⑵⑶
            // gave a RUN a mouth for its asker and ⑸ gave a PANE a mouth for its occupant, and all
            // four are answered by somebody looking from OUTSIDE.
            //
            // ⚠⚠ IT TAKES NO SUBJECT, which is item 871's safety property rather than a gap: a
            // caller may POINT (`$SPRAG_PANE` is the daemon's own stamp on this process) and may
            // not NAME. An argument here would let anybody assert somebody else's identity.
            //
            // ⚠ NOT BOUND TO A KEY, on `runs`' terms — it answers, and this client has no view for
            // it. `NotBuilt` for an agent and NOT `Refused`: an agent asking which runs it is on is
            // exactly the case this verb was measured for (a watcher that could not say whether a
            // run was its own), and it reaches it through a shell today. A tool for it is a gap
            // nobody has got to, not a decision.
            Self::MyRuns => (
                "my-runs",
                Group::Orchestration,
                Shell::Runs("[-t SESSION]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::NotBuilt,
            ),
            // ⛔⛔⛔⛔⛔ **WHICH DAEMONS ARE RUNNING, ASKED OF THE MACHINE** — register item 825.
            //
            // It needs NO DAEMON, and that is the whole point rather than a property it happens to
            // have: every other verb here asks A daemon something, and this one asks whether there
            // is one at all. A verb that had to connect first could not answer the question it
            // exists for — which is what the launcher's *"no server running at the well-known
            // socket"* was, six times, while a daemon served six windows on another socket.
            //
            // ⚠ NOT BOUND TO A KEY, on `words`' terms: a keystroke is pressed inside a client that
            // is already attached to a daemon, so a client asking which daemons exist has answered
            // its own question by being there.
            //
            // ⚠⚠ REFUSED to an agent rather than filed as a gap, and the distinction is rule 6's:
            // an agent is born in a pane and inherits that pane's daemon (`pane_env_source`), so
            // this question reaches OUTSIDE the session it works in exactly as `kill-server` does.
            // `NotBuilt` would have said *nobody got to it yet* about a decision, and invited the
            // gap to be closed by handing an agent a choice between daemons.
            Self::Daemons => (
                "daemons",
                Group::Tool,
                Shell::Runs("[--serving]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                Agent::Cannot(NotAnAgents::OutsideItsSession),
            ),
            // THE DOOR ONTO THE WIRE'S OWN GRAMMAR, and the reason it is a verb rather than a
            // document: it ASKS THE DAEMON. herdr's equivalent (`herdr api schema`) prints a JSON
            // Schema a test wrote into its docs and the binary `include_str!`'d — so it describes
            // whatever build the CLI came from, and no method among its ninety-one returns it, which
            // means a client speaking the socket cannot ask the daemon it is connected to. This
            // queries `action_grammar` on the live daemon and prints what came back, so an operator
            // and an agent read the same answer as each other and as the daemon they are talking to,
            // version skew included.
            //
            // NO SCOPE, like `doctor` beside it: a request grammar is the same on every session, and
            // narrowing it by one would answer a question nobody asks. The optional argument narrows
            // by VERB instead, which is the question a person actually has.
            Self::ShowGrammar => (
                "show-grammar",
                Group::Tool,
                Shell::Runs(" [VERB] [--pane]"),
                Keystroke::Cannot(NotAKeystroke::Answers),
                // ⚠ NO TOOL, and this is the one verb where that needs saying out loud, because an
                // AI client is exactly who the grammar is for. An MCP client learns how to call
                // ITS OWN mouth from `tools/list`, which every one of them reads before calling
                // anything — a tool here would be a second authority on the same question, and a
                // wronger one: it describes the RAW WIRE's verbs, not the tools the agent has. The
                // client that needs this is the one speaking JSON-RPC to the socket, and that client
                // reads the slot itself.
                Agent::Cannot(NotAnAgents::SaidAnotherWay),
            ),
            // ── the five a KEYSTROKE alone can say ──────────────────────────────────────────────
            // Each acts on THE CLIENT THAT PRESSED THE KEY, which neither of the other two mouths
            // has. That is a property of the verb and not a gap: `sprag detach-client` would need
            // to name a client and then mean something different from what the key means. They are
            // the only five refused on two mouths for ONE fact, which is why both reasons are
            // spelled `NoClientOfIts`.
            Self::DetachClient => (
                "detach-client",
                Group::Client,
                Shell::Cannot(NotAShellVerb::NoClientOfIts),
                Keystroke::Means("detach-client"),
                Agent::Cannot(NotAnAgents::NoClientOfIts),
            ),
            Self::SendPrefix => (
                "send-prefix",
                Group::Client,
                Shell::Cannot(NotAShellVerb::NoClientOfIts),
                Keystroke::Means("send-prefix"),
                Agent::Cannot(NotAnAgents::NoClientOfIts),
            ),
            Self::SwitchClient => (
                "switch-client",
                Group::Client,
                Shell::Cannot(NotAShellVerb::NoClientOfIts),
                Keystroke::Means("switch-client -n|-p|-l|-t [<session>]"),
                Agent::Cannot(NotAnAgents::NoClientOfIts),
            ),
            Self::ChooseTree => (
                "choose-tree",
                Group::Client,
                Shell::Cannot(NotAShellVerb::NoClientOfIts),
                Keystroke::Means("choose-tree"),
                Agent::Cannot(NotAnAgents::NoClientOfIts),
            ),
            Self::ConfirmBefore => (
                "confirm-before",
                Group::Client,
                Shell::Cannot(NotAShellVerb::NoClientOfIts),
                Keystroke::Means("confirm-before <action>"),
                Agent::Cannot(NotAnAgents::NoClientOfIts),
            ),
        };
        Entry {
            name,
            group,
            shell,
            key,
            agent,
        }
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

    /// What an agent can ask for by it.
    #[must_use]
    pub const fn agent(self) -> Agent {
        self.entry().agent
    }

    /// Whether `sprag <name>` dispatches it.
    #[must_use]
    pub const fn runs_in_shell(self) -> bool {
        matches!(self.entry().shell, Shell::Runs(_))
    }

    /// Whether a binding can name it.
    #[must_use]
    pub const fn bindable(self) -> bool {
        matches!(self.keystroke(), Keystroke::Means(_))
    }

    /// The MCP tools an agent reaches this verb through — EMPTY for a verb no tool serves.
    ///
    /// Empty covers both remaining answers on purpose: a caller asking *"can an agent do this"*
    /// wants one question answered, and a caller asking *"why not"* reads [`Agent`] itself. The
    /// same split [`bindable`](Self::bindable) makes one mouth over.
    #[must_use]
    pub const fn tools(self) -> &'static [&'static str] {
        match self.agent() {
            Agent::Tools(tools) => tools,
            Agent::NotBuilt | Agent::Cannot(_) => &[],
        }
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
        if !matches!(self.entry().shell, Shell::Cannot(_)) {
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

    /// The sentence a SHELL answers with for a verb the product performs and no CLI dispatches.
    ///
    /// [`only_a_keystroke`](Self::only_a_keystroke)'s third case, and it exists for that method's
    /// reason: a user who typed `sprag pane-links` asked for something sprag DOES, and telling them
    /// `unknown command` would be the sentence a typo gets. It names the mouth that has the act, so
    /// the answer is a place to go rather than a refusal.
    ///
    /// [`None`] unless the verb's shell answer is [`Shell::NotBuilt`], so a caller cannot print
    /// this about `sprag ls` or about `switch-client`.
    #[must_use]
    pub fn no_shell_form_yet(self) -> Option<String> {
        if !matches!(self.entry().shell, Shell::NotBuilt) {
            return None;
        }
        let tools = self.tools();
        let reached = if tools.is_empty() {
            // Unreachable through the table today — a verb no mouth can say is what
            // `every_verb_has_a_mouth` refuses — and a plainer sentence rather than a panic,
            // because this renders an error message.
            String::new()
        } else {
            format!(
                " An AI agent in a pane reaches it as the {} MCP tool{}.",
                tools
                    .iter()
                    .map(|tool| format!("`{tool}`"))
                    .collect::<Vec<_>>()
                    .join(" / "),
                if tools.len() == 1 { "" } else { "s" },
            )
        };
        Some(format!(
            "{:?} is something sprag does and has no shell command yet.{reached}",
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
        let entry = verb.entry();
        // A group whose verbs are all refused or unbuilt prints NO heading, because the heading is
        // written lazily by the first verb that has a form — which is what keeps `client` out of
        // the help without a rule saying so.
        let Shell::Runs(form) = entry.shell else {
            continue;
        };
        if group != Some(entry.group) {
            out.push_str(&format!("\n  {}\n", entry.group.heading()));
            group = Some(entry.group);
        }
        let name = verb.name();
        if form.is_empty() {
            out.push_str(&format!("    {name}\n"));
        } else {
            out.push_str(&format!("    {name} {form}\n"));
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
/// drift this module exists to remove. It is a function of [`Entry::group`], so the two axes cannot
/// disagree — and the five verbs a shell cannot say are [`Group::Client`]'s own, which is what they
/// had always been answered as through the absence of a shell form.
#[must_use]
pub fn subject_of(verb: Verb) -> ActionSubject {
    match verb.entry().group {
        Group::Window => ActionSubject::Window,
        Group::Pane => ActionSubject::Pane,
        Group::Session => ActionSubject::Session,
        // The keyboard, the settings, an agent's reports, a bounded run, and sprag itself are all
        // things a CLIENT asks about rather than parts of the containment.
        //
        // ⚠ A run is a client's rather than a pane's even though it DRIVES panes, and the reason is
        // what this axis is for: it says where a bound verb appears in the key help, and the
        // question a person asks there is about the thing they are addressing. A run's subject is
        // the run, which is not one of the three containers.
        Group::Agent
        | Group::Orchestration
        | Group::Keys
        | Group::Options
        | Group::Tool
        | Group::Client => ActionSubject::Client,
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

    /// Every verb can be SAID by somebody: a shell, a keystroke, an agent, or several.
    ///
    /// The one combination [`Entry`] can spell that means nothing — no shell form, a keystroke that
    /// cannot mean it and no tool — is a verb the product has and nobody can reach. R335 widened it
    /// to three mouths, which is what let the three tool-only acts into the table at all:
    /// [`Verb::PaneLinks`] would have failed this test before the agent axis existed.
    #[test]
    fn every_verb_has_a_mouth() {
        for verb in Verb::ALL {
            assert!(
                verb.runs_in_shell() || verb.bindable() || !verb.tools().is_empty(),
                "{} can be said by nobody",
                verb.name(),
            );
        }
        // THE CONTROL: the three mouths are not all the same mouth. Each has at least one verb the
        // other two cannot say, so a test that passed by one column alone would be reading a table
        // this project does not have.
        for (mouth, only) in [
            ("a shell", Verb::InstallHooks),
            ("a keystroke", Verb::SwitchClient),
            ("an agent", Verb::PaneLinks),
        ] {
            let mouths = usize::from(only.runs_in_shell())
                + usize::from(only.bindable())
                + usize::from(!only.tools().is_empty());
            assert_eq!(
                mouths,
                1,
                "{} is meant to be reachable by {mouth} ALONE, and {mouths} mouths have it",
                only.name(),
            );
        }
    }

    /// **WHAT AN AGENT CAN ASK FOR IS WHAT THE TABLE SAYS IT IS** — item 56's number, derived.
    ///
    /// [`the_keyboard_gap_is_what_the_table_says_it_is`]'s instrument on the third mouth, and for
    /// its reason: the register held *"a by-hand mapping suggested ~20 verbs have a tool and ~34 do
    /// not, and that is an estimate, not a measurement"*. It is a measurement now, and it moves in
    /// three directions that cannot all drift the same way.
    #[test]
    fn the_agent_gap_is_what_the_table_says_it_is() {
        let served = Verb::ALL
            .iter()
            .filter(|verb| !verb.tools().is_empty())
            .count();
        let not_built = Verb::ALL
            .iter()
            .filter(|verb| verb.agent() == Agent::NotBuilt)
            .count();
        let refused = Verb::ALL
            .iter()
            .filter(|verb| matches!(verb.agent(), Agent::Cannot(_)))
            .count();
        assert_eq!(
            (served, not_built, refused),
            // R353: `show-grammar` is the 21st refusal — an MCP client learns how to call ITS
            // OWN mouth from `tools/list`, so a tool here would be a second authority on one
            // question and a wronger one (it describes the raw wire's verbs, not the agent's tools).
            // ⚠ R369: `answer-pane` is the 38th, and this mouth is the reason it exists. An agent
            // watching a sibling can READ what it is asking (R367) and had nothing to answer it
            // with but `send_keys` — a raw digit at a menu, which is the act the consent contract
            // was built to stop a machine performing.
            // R355: three more served, and this is the mouth they matter most on — an agent that
            // wanted a bounded loop against a sibling had to hand-roll one in its own turns,
            // without the iteration ceiling, the typed cost ceiling, the wall-clock deadline or
            // the cancel flag that exist.
            // ⚠ `stand-down` is the EIGHTH not-built: a supervising agent standing another run
            // down is a legitimate ask with no tool behind it yet.
            // ⚠ `hold-run` and `resume-run` (register item 9) join the middle column: a supervising
            // agent holding another run while it reads that run's pane is the same legitimate ask
            // `stand-down`'s row records, and the mouth has no tool for it yet.
            // ⚠ REGISTER ITEM 773: `words` is the ELEVENTH not-built. An agent reading a run gets
            // the vocabulary in the row it is already holding, so asking twice would be a second
            // authority on one fact — but *what are your words* is a legitimate ask (a tool could
            // answer it once and cache), and nobody has built it.
            // ⚠ REGISTER ITEM 825: `daemons` is the 22nd REFUSAL and deliberately not a 12th gap.
            // An agent is born in a pane and inherits that pane's daemon (`pane_env_source`), so
            // *which daemons are running on this machine* reaches outside the session it works in
            // exactly as `kill-server` does. Filing it as not-built would have invited somebody to
            // close the gap by handing an agent a choice between daemons — this workspace's rule 6:
            // a category that means *nobody got to it yet* is the wrong home for a decision.
            (38, 11, 22),
            "an agent reaches {served} verbs, {not_built} are an agent's to ask and are not built, \
             and {refused} are refused with a reason",
        );
        assert_eq!(served + not_built + refused, Verb::ALL.len());
        // NAMED, so closing one is a change here and not a number that drifts — and so a reader can
        // see at a glance that none of the seven is an ARRANGEMENT verb, which is what item 56a
        // was about.
        let pending: Vec<&str> = Verb::ALL
            .iter()
            .filter(|verb| verb.agent() == Agent::NotBuilt)
            .map(|verb| verb.name())
            .collect();
        assert_eq!(
            pending,
            [
                "list-clients",
                "run",
                "move-window",
                "list-hooks",
                // ⚠ A SUPERVISING agent standing another run down is the ask with no tool behind it.
                // A run standing ITSELF down would be a different verb and a refusal, not this gap.
                "stand-down",
                // ⚠ The same gap one order over (register item 9): a supervising agent holding
                // another run while it reads that run's pane has no tool behind it either.
                "hold-run",
                "resume-run",
                "list-keys",
                "show-options",
                "version",
                // ⚠ REGISTER ITEM 773: an agent reading a run already holds the words in the row,
                // so a tool would be a second authority on one fact — but *what does this build
                // speak* is still a legitimate ask, and nothing answers it on that mouth.
                "words",
            ],
            "the agent surface's remaining gap, by name",
        );
    }

    /// **THE ARRANGEMENT FAMILY IS WHOLE ON EVERY MOUTH** — item 56a, as a claim rather than a table
    /// in a register.
    ///
    /// The measured defect was a SHAPE: six verbs that move a pane around, four of which no agent
    /// could say, so an agent could create, destroy and shuffle panes inside one window and could
    /// not move one BETWEEN windows at all. This asserts the shape rather than the count, so it
    /// stays true as the family grows and fails the moment one member loses a mouth.
    #[test]
    fn every_arrangement_verb_is_reachable_from_every_mouth() {
        for verb in [
            Verb::BreakPane,
            Verb::JoinPane,
            Verb::MovePane,
            Verb::SwapPane,
            Verb::ZoomPane,
            Verb::ResizePane,
        ] {
            assert!(verb.runs_in_shell(), "{} has no shell form", verb.name());
            assert!(verb.bindable(), "{} cannot be bound", verb.name());
            assert!(
                !verb.tools().is_empty(),
                "{} is an arrangement verb no agent can ask for — the exact half-served family \
                 item 56a measured",
                verb.name(),
            );
        }
    }

    /// A tool name names ONE verb, and every declared name is a plausible MCP tool name.
    ///
    /// The uniqueness half is what makes [`Verb::tools`] a partition rather than a tagging: two
    /// verbs claiming `read_pane` would make the roster ratchet in `sprag-mcp` pass while the table
    /// said two contradictory things about one tool.
    #[test]
    fn a_tool_name_names_one_verb() {
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for verb in Verb::ALL {
            for tool in verb.tools() {
                if let Some((_, other)) = seen.iter().find(|(name, _)| name == tool) {
                    panic!("{tool:?} is claimed by both {other} and {}", verb.name());
                }
                seen.push((tool, verb.name()));
                assert!(
                    tool.chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                    "{tool:?} is not spelled the way this surface spells a tool",
                );
            }
        }
        assert!(seen.len() > 30, "the table declares {} tools", seen.len());
    }

    /// Every agent refusal reads as a clause, no two say the same thing, and every rule is USED.
    ///
    /// [`each_refusal_reason_is_its_own_sentence`]'s test on the third mouth. The USED half is the
    /// one that matters most here: these four rules were written in one sitting, and a rule nobody
    /// applies is policy nobody decided.
    #[test]
    fn each_agent_refusal_reason_is_its_own_sentence() {
        let reasons = [
            NotAnAgents::OutsideItsSession,
            NotAnAgents::ThePersonsOwn,
            NotAnAgents::NoClientOfIts,
            NotAnAgents::SaidAnotherWay,
        ];
        let mut whys: Vec<&str> = reasons.iter().map(|reason| reason.why()).collect();
        let before = whys.len();
        whys.sort_unstable();
        whys.dedup();
        assert_eq!(before, whys.len(), "two reasons read identically");
        for why in whys {
            assert!(
                !why.ends_with('.'),
                "{why:?} is a clause a refusal ends with, so it stops without punctuation",
            );
        }
        for reason in reasons {
            assert!(
                Verb::ALL
                    .iter()
                    .any(|verb| verb.agent() == Agent::Cannot(reason)),
                "{reason:?} is a reason no verb gives",
            );
        }
        // The one reason spelled on TWO mouths is spelled so for one fact, and the verbs that give
        // it are the same five — which is the claim the two names make and nothing else checks.
        let no_client: Vec<&str> = Verb::ALL
            .iter()
            .filter(|verb| verb.agent() == Agent::Cannot(NotAnAgents::NoClientOfIts))
            .map(|verb| verb.name())
            .collect();
        let no_shell: Vec<&str> = Verb::ALL
            .iter()
            .filter(|verb| verb.entry().shell == Shell::Cannot(NotAShellVerb::NoClientOfIts))
            .map(|verb| verb.name())
            .collect();
        assert_eq!(no_client, no_shell, "one fact, two mouths, two lists");
        assert_eq!(no_client.len(), 5);
    }

    /// **THE SHELL'S GAP IS WHAT THE TABLE SAYS IT IS**, and it stopped being zero at R335.
    ///
    /// `Option<Shell>` could only say *runs* or *cannot*, and this table now holds three acts the
    /// product performs that no CLI dispatches. The count is asserted for
    /// [`the_keyboard_gap_is_what_the_table_says_it_is`]'s reason — so closing one is an edit here
    /// rather than a number nobody re-measures.
    #[test]
    fn the_shell_gap_is_what_the_table_says_it_is() {
        let runs = Verb::ALL.iter().filter(|verb| verb.runs_in_shell()).count();
        let not_built = Verb::ALL
            .iter()
            .filter(|verb| verb.entry().shell == Shell::NotBuilt)
            .count();
        let refused = Verb::ALL
            .iter()
            .filter(|verb| matches!(verb.entry().shell, Shell::Cannot(_)))
            .count();
        assert_eq!(
            (runs, not_built, refused),
            // ⚠ R369: `answer-pane` is the 58th, and the count is a CLAIM about `sprag.rs` —
            // `every_shell_verb_this_table_claims_is_one_the_binary_dispatches` is what holds it,
            // so a verb declared here and unwired there is a red rather than a boast.
            // R353: `show-grammar` is the 53rd shell verb — the door onto the wire's own grammar.
            // R355: three more, and they are the door onto the LOOP the README leads with —
            // `orchestrate`, `runs` and `cancel-run`.
            // ⚠ `stand-down` is the 59th — the second thing a person can say to a run, and the
            // first that lets it keep the turn it is in the middle of.
            // ⚠ Both of item 9's verbs are a shell's to say and are dispatched, so they land here.
            // ⚠ REGISTER ITEM 773: `words` is the 62nd, and it is the one shell verb here whose
            // whole purpose is that a shell can reach it with NO DAEMON and from any directory —
            // the two ways the answer it replaces (three `grep`s into this tree) could not be had.
            // ⚠ REGISTER ITEM 825: `daemons` is the 63rd, and the SECOND that needs no daemon —
            // for a sharper reason than `words`. Every other verb here asks A daemon something;
            // this one asks whether there is one at all, so a version of it that had to connect
            // first could not answer the question it exists for.
            (63, 3, 5),
            "the shell dispatches {runs} verbs, {not_built} are a shell's to say and are not \
             built, and {refused} are refused with a reason",
        );
        assert_eq!(runs + not_built + refused, Verb::ALL.len());
        let pending: Vec<&str> = Verb::ALL
            .iter()
            .filter(|verb| verb.entry().shell == Shell::NotBuilt)
            .map(|verb| verb.name())
            .collect();
        assert_eq!(
            pending,
            ["read-last-command", "pane-links", "pane-images"],
            "the shell's remaining gap, by name",
        );
        // Each of the three says where it CAN be reached, so a person who typed one gets a place to
        // go rather than a refusal — and the sentence names a tool the roster really carries,
        // because it is built from the same column the roster ratchet reads.
        for verb in Verb::ALL {
            let sentence = verb.no_shell_form_yet();
            assert_eq!(
                sentence.is_some(),
                verb.entry().shell == Shell::NotBuilt,
                "{} answered the wrong question",
                verb.name(),
            );
            if let Some(sentence) = sentence {
                assert!(
                    sentence.contains(verb.name())
                        && verb.tools().iter().all(|tool| sentence.contains(tool)),
                    "{sentence:?} must name the verb and every tool that has it",
                );
            }
        }
        assert_eq!(Verb::Ls.no_shell_form_yet(), None, "the shell runs `ls`");
        assert_eq!(
            Verb::SwitchClient.no_shell_form_yet(),
            None,
            "a shell CANNOT say switch-client, which is a different answer",
        );
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
            // R353: `show-grammar` ANSWERS something, so it joins the verbs a keystroke cannot mean.
            // R355: `orchestrate` needs words and `runs` answers, so both are refused with a
            // reason; `cancel-run` is the second `NotBuilt` — a key COULD mean "cancel the runs I
            // started" and nobody has built that verb.
            // ⚠ R369: `answer-pane` is the 38th refusal — its whole content is the two needles
            // a caller quotes off the dialog, and a binding would fix one question and one option
            // forever. That is `send-keys`'s rule with a sharper reason: a consent is about a
            // question somebody READ, never about whatever is on the screen when a key is pressed.
            // ⚠⚠ `stand-down` is the FOURTH `NotBuilt`, and the best candidate of them: a person
            // leaving their desk wants the verb that keeps the work, and leaving a desk is exactly
            // when a keystroke is reached for. Nobody has built "stand down every run I started".
            // ⚠ Item 9's two join the middle column for `stand-down`'s reason: a key names no run.
            // ⚠ REGISTER ITEM 773: `words` is the 39th refusal — it ANSWERS something and this
            // client has no view for the answer, which is the reason every answering verb here
            // carries. It becomes bindable the day a surface exists to show a vocabulary in.
            // ⚠ REGISTER ITEM 825: `daemons` is the 40th refusal, on `words`' terms and with one
            // more: a keystroke is pressed INSIDE a client that is already attached to a daemon,
            // so a client asking which daemons exist has answered its own question by being there.
            (25, 6, 40),
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
            // ⚠ `stand-down` sits beside `cancel-run` because they are the same shape of gap — "do
            // this to every run I started" — and it is the one a person is likeliest to want under
            // a key, since it is the one they reach for on the way out of the door.
            // ⚠⚠ `hold-run` is the BEST candidate on this list (register item 9): *stop so I can
            // read this* is the one order somebody wants to give without leaving the pane they are
            // looking at, which is what a keystroke is for. It waits here for the same reason as its
            // neighbours — a key names no run.
            [
                "run",
                "stop-job",
                "cancel-run",
                "stand-down",
                "hold-run",
                "resume-run",
            ],
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
                Group::Orchestration.heading(),
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
