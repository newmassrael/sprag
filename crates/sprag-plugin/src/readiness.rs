//! The readiness barrier — *do not type into a pane whose program has not started yet*.
//!
//! One home, because the three plugins that INJECT need it for the same reason and none of them is
//! the reason.
//!
//! # ⚠⚠ Why a loop needs a barrier at all
//!
//! A pane is not ready when it is OPEN. It is born running a shell with the pseudoterminal's
//! default echo on, and the program a caller means to drive — `claude`, a REPL, a test runner —
//! starts some time after that. Anything injected in that window goes to whatever IS there: the
//! shell EXECUTES it as a command, or a reader that is not the peer eats it. Either way the turns
//! are spent, the guardrails count them, and the peer never saw a word.
//!
//! The remedy cannot be a sleep — how long a program takes to come up is not a number a caller
//! knows, and it is the one thing that varies most between the programs this drives. It is a thing
//! the pane SAYS: a prompt, a banner, a marker the caller printed on purpose. So the caller names
//! it and the run waits for it, bounded by the caller's own `ready_within` and by the run's
//! deadline like every other wait here.
//!
//! # Who takes one, and why the last one needed it most
//!
//! Every plugin that TYPES INTO a pane it did not start takes this barrier; the one that does not
//! type takes none. That is the whole rule, and each half of it was measured rather than argued.
//!
//! * [`Orchestrator`](crate::orchestrator::Orchestrator) drives a pane a caller pointed it at. It
//!   got the barrier first.
//! * [`Pipe`](crate::pipe::Pipe) relays into a pane **somebody else prepared**, which is the whole
//!   shape of a relay — and it had no barrier at all. A relay into a pane that was still a shell
//!   was eaten by that shell (`SHELL-ATE relayme`, twice) while the peer that came up a second
//!   later saw nothing, and every number the run reported matched a working relay's.
//! * [`Agent`](crate::agent::Agent) is the worst case, because its failure is a WRONG ANSWER and
//!   not a missing one. It prompts the caller's pane and hands what comes back to a peer as *the
//!   model's reply*; against a shell, the prompt is run as a command and the trailing Ctrl-D makes
//!   the shell EXIT — which is exactly the completion signal it converges on. Measured:
//!   `"summarise the repo"` came back as `"…$ sh: 1: summarise: not found\n$"`, reported
//!   `converged`.
//! * [`Dialogue`](crate::dialogue::Dialogue) takes NONE, and the absence is a finding too: it
//!   passes each turn's prompt as an ARGV ARGUMENT of the pane it spawns for that turn and never
//!   injects a byte, so there is no window for a shell to be typed into.
//!
//! # ⚠⚠ And the same door decides what may be typed at a peer that is ASKING
//!
//! A barrier that only ever refuses would leave a run with one answer to a peer's question, and
//! *"stop and fetch a person"* is the right answer only until somebody has decided in advance. So
//! the caller's [`Consent`] lives here too, and the reason is the reason
//! this module exists at all: **this is the one place all three injecting plugins pass through on
//! their way to a keystroke.** A second door to a blocked pane — a plugin answering a dialog on its
//! own — would be two readers of one question, which is the shape R344 spent a round on and R365
//! found again. There is one, and what it may type is what the caller wrote down.

use std::time::Duration;

use sprag_detect::{AgentState, Choice, Question};
use sprag_terminal::PaneId;

use crate::access::{JobLeader, KeyStroke, PaneAccess, PaneDoing, PaneError};
use crate::consent::{Answered, Consent, Refusal, Taken, Unanswered};
use crate::run::{RunContext, Waited, poll_until};

/// Whether the peer in `pane` has stopped to ASK, and what it is asking when this host can read it.
///
/// The OUTER `Option` is *is it blocked*; the inner one is
/// [`AgentObservation::asking`](crate::access::AgentObservation::asking)'s own — *this host cannot
/// read the question* — and the two must not collapse: one says nothing is wrong and the other says
/// a person is needed.
///
/// ⚠ A host with no supervisor answers `None` and the run proceeds, which is this crate's rule
/// everywhere: an absence of evidence is never read as the negative. It is also the honest cost of
/// the guard — a build that cannot see agents cannot protect their dialogs either, and pretending
/// otherwise would block every run on every host that has no detector.
fn peer_asking(panes: &dyn PaneAccess, pane: PaneId) -> Option<Option<Question>> {
    let seen = panes.supervision()?.pane_agent_state(pane)?;
    (seen.state == AgentState::Blocked).then_some(seen.asking)
}

/// What a peer did with the number that was typed at it — the three states an answer can be in
/// while it is being given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arrival {
    /// The peer is no longer showing the question this answer was given to — it took the key.
    ///
    /// ⚠⚠ **THE QUESTION, not the STATE.** A supervisor's verdict SETTLES: a real detector goes on
    /// calling a pane blocked for its hysteresis window after the dialog has left the screen. Keyed
    /// on the state, the wait for a peer to move outlived the answering bound and a run that HAD
    /// been answered reported [`Refusal::NotTaken`] — measured end to end through a real daemon,
    /// and invisible to every fixture here because a fixture derives its state from the screen and
    /// so has no lag to model.
    ///
    /// ⚠ A DIFFERENT question is also this arm. The one this run answered is gone, and whatever the
    /// peer asked next is the next step's business — where it meets the whole contract again rather
    /// than an answer already in flight.
    LeftTheQuestion,
    /// The peer is still asking the SAME question and its marker is now on the authorised option —
    /// so it read the key, and an Enter can land nowhere else.
    OnTheOption,
    /// Neither yet. ⚠ Also the answer when the peer moved to a DIFFERENT question, which is a
    /// dialog nobody consented to and must never be confirmed.
    NotYet,
}

/// Where the peer stands right now, relative to the question `chose` was authorised for.
///
/// ⚠ Read FRESH each time, and the freshness is load-bearing: `asking` is derived from the pane's
/// screen at the moment it is asked, so a peer that has taken the key no longer shows the menu even
/// while a settle window still calls its STATE blocked. That is why a stale approval cannot be
/// confirmed here — the screen is the thing that moved, and the screen is what this reads.
fn marker_arrived(
    panes: &dyn PaneAccess,
    pane: PaneId,
    question: &Question,
    chose: &Choice,
) -> Arrival {
    // ⚠⚠ FLATTENED, and the two `None`s it merges are both this question being over. Not blocked
    // at all is obvious; blocked with NO READABLE MENU is the settle window — a detector goes on
    // calling a pane blocked after the dialog left its screen — and it is also a peer that moved on
    // to something this host cannot parse. In every one of those the question this answer was given
    // to is no longer on the pane, and the next step meets whatever is.
    let Some(now) = peer_asking(panes, pane).flatten() else {
        return Arrival::LeftTheQuestion;
    };
    // ⚠ Compared by the question's own SENTENCE, never the whole value: the marker has moved by
    // now, so the choices differ from the ones this answer was authorised against and comparing
    // them whole would be a condition that can never hold.
    if now.asked != question.asked {
        return Arrival::LeftTheQuestion;
    }
    if now
        .selected()
        .is_some_and(|marked| marked.number == chose.number)
    {
        Arrival::OnTheOption
    } else {
        Arrival::NotYet
    }
}

/// How long the whole answering act may take — the keystroke, the peer moving off the question, and
/// its supervisor catching up with both.
///
/// # ⚠⚠ A MECHANISM bound, which is why it is a constant and not an argument
///
/// Every other bound in this crate is the caller's, because every other one asks a question only
/// the caller can answer: how long a program takes to start, how long a model may think. This one
/// asks how long a terminal program takes to process a keystroke it was already sitting waiting
/// for, and nobody has a better answer for that than the product does. A peer that has not moved
/// off its own dialog in this long is not slow — it did not take the key, which is
/// [`Refusal::NotTaken`] and a fact worth reporting rather than a bound worth raising.
///
/// # ⚠⚠⚠ Why it is SIZED FROM THE SETTLE WINDOW and not from a repaint
///
/// A keystroke's repaint is milliseconds, and the first draft of this was two seconds on that
/// reasoning. It was the wrong quantity: the slowest thing this waits for is not the PANE, it is
/// the SUPERVISOR — a verdict is published only once a candidate has held for
/// [`sprag_detect::DEFAULT_SETTLE`], so a pane whose dialog has just gone on being answered still
/// reads `Blocked` for that whole window. Two seconds against a two-second settle is a bound that
/// races the thing it is waiting for, and an end-to-end run measured it losing: a run whose answer
/// had plainly landed reported [`Refusal::NotTaken`].
///
/// ⚠ The residue, stated: a host may CONFIGURE a longer settle than the default this is anchored
/// to, and there the race returns. The anchor is what makes that a known quantity rather than a
/// coincidence — and the failure stays in the safe direction, since a run that gives up reports the
/// question rather than typing at it.
const ANSWER_WITHIN: Duration = Duration::from_secs(sprag_detect::DEFAULT_SETTLE.as_secs() + 2);

/// What the peer is asking, having given a pane that reads BLOCKED WITH NOTHING READABLE the chance
/// to catch up first.
///
/// # ⚠⚠⚠ Why the immediate reading is not trustworthy for this one case
///
/// `Blocked` with no question is a real state with a real remedy — a person — and R366 gave it a
/// word for that reason. It is ALSO what a pane looks like for [`sprag_detect::DEFAULT_SETTLE`]
/// after its dialog has been answered and gone: the screen has no menu, and the supervisor's
/// verdict has not yet caught up with that. Read on sight, the step after a successful answer
/// reported *"the peer is blocked on something this host cannot read — fetch a person"* about a
/// pane that had just done exactly what it was asked. Measured end to end through a real daemon.
///
/// So the ambiguous reading is waited out. If it was the settle window, the verdict moves and the
/// run goes on; if the peer really is blocked on something unreadable, the same answer comes back
/// and the remedy is reported one bound later — which costs a run that is stopping anyway nothing
/// it can use.
///
/// ⚠ Only the AMBIGUOUS reading waits. A pane that is not blocked, and a pane blocked on a menu
/// this host CAN read, are both answered on sight.
fn settled_question(
    panes: &dyn PaneAccess,
    pane: PaneId,
    run: &RunContext,
) -> Option<Option<Question>> {
    let asking = peer_asking(panes, pane)?;
    if asking.is_some() {
        return Some(asking);
    }
    let _ = poll_until(run, ANSWER_WITHIN, || {
        !matches!(peer_asking(panes, pane), Some(None))
    });
    peer_asking(panes, pane)
}

/// The bound a readiness wait is given when the caller names none.
///
/// ⚠ Deliberately the same number as [`DEFAULT_REPLY_TIMEOUT`], and deliberately its OWN name.
/// The value is shared because two minutes is this crate's one answer to *"long enough for
/// something on the other side to happen"* and inventing a second number nobody chose would be
/// worse. The name is separate because the QUESTIONS differ — one is how long a model may think,
/// the other is how long a program may take to start — and a caller who wants to change one of
/// them almost never means the other.
///
/// [`DEFAULT_REPLY_TIMEOUT`]: crate::run::DEFAULT_REPLY_TIMEOUT
pub const DEFAULT_READY_TIMEOUT: Duration = crate::run::DEFAULT_REPLY_TIMEOUT;

/// How a [`Readiness`] wait ended, for the endings that are not an error.
/// ⚠ NOT `Copy` since two arms carry the peer's question — [`Verdict`]'s reason, and the same one:
/// an answer that cannot say WHAT the peer is asking is not worth returning.
///
/// [`Verdict`]: crate::plugin::Verdict
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reached {
    /// The pane is ready. Drive it.
    Yes,
    /// THE RUN ended while waiting — cancelled, or out of time. **Nothing was injected**, so
    /// nothing is charged; which of the two it was is the [`RunContext`]'s to answer.
    RunEnded,
    /// **THE PEER HAS STOPPED TO ASK, and the run is not going to type its own text at it** —
    /// carrying the question when this host can read it, and WHY nothing was answered.
    ///
    /// # ⚠⚠⚠ The one answer here that is not about STARTING
    ///
    /// Every other state of this barrier is about *has the program come up*, which is answered
    /// once and stays answered — that is why it latches. This one is about a fact that CHANGES
    /// under a running loop: an agent that was at rest can pop a tool-permission dialog at any
    /// moment, and from then on the pane is a numbered menu.
    ///
    /// A menu consumes keystrokes, so a stimulus typed into one is not text — it is a SELECTION,
    /// and every injection these plugins make ends with Enter, which lands on whatever option the
    /// agent had highlighted. Measured before this variant existed: an orchestrator whose peer
    /// blocked after its first step typed the stimulus three more times and reported
    /// `Exhausted(Iterations)`.
    Asking(Unanswered),
    /// **THE PEER ASKED AND THE RUN ANSWERED IT**, on a [`Consent`] the caller declared in advance.
    ///
    /// # ⚠⚠ This is not `Yes`, and the difference is the whole safety of it
    ///
    /// The peer has just been handed a decision and is acting on it. A barrier that answered `Yes`
    /// here would let the same step go on to type its stimulus into a pane that is mid-transition,
    /// which is the defect one step to the right of the one [`Asking`](Self::Asking) closed. So an
    /// answer ENDS the step: the plugin charges what the keystrokes cost, records what was
    /// answered, and the NEXT step asks this barrier again — by which time the peer is working, at
    /// rest, or asking something else, and each of those is already an answer this type has.
    Answered(Answered),
}

/// WHICH QUESTION a readiness marker is asking — the distinction the argument used to hide.
///
/// # ⚠⚠ Why one needle cannot answer both
///
/// A marker match over the pane's text answers *"is this text here?"*, and that same observation
/// means two opposite things depending on what the caller is doing:
///
/// * **They just started the program.** Text that was ALREADY on the screen when the run began is
///   no evidence at all — and the most likely such text is the ECHO OF THE COMMAND LINE THAT
///   STARTED THE PROGRAM, which a pty puts on screen before the program exists. Measured: a pane
///   told to wait for `TOOL-UP` passed the barrier in 50 MILLISECONDS against
///   `…printf "TOOL-UP\n"; exec cat'$ ping ATE ping…` — the run spent both its turns on the shell
///   and the peer never saw a word.
/// * **They are pointing at a program already running.** A REPL sitting at its prompt has that
///   prompt on screen and will print nothing further until it is fed. Here the text already being
///   there is exactly the evidence, and demanding NEW output would wait forever.
///
/// These are not degrees of the same evidence — they are different KINDS, in the sense
/// [`Authority`](crate::access::Authority) means it, and nothing in the marker itself says which.
/// **Only the caller knows**, so the type makes them say. A single string could not, which is why
/// the wire value changed shape rather than gaining a default nobody chose.
///
/// ⚠ Whichever they pick, a marker that cannot appear in a command line (a prompt, `>>> `) is
/// safer than a word they typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyWhen {
    /// Ready once the marker appears in output the pane produces **after the barrier arms**.
    ///
    /// The answer for *"I just started it"*: the barrier counts how many times the marker is
    /// already on the collapsed screen when it arms, and clears only once there are MORE. Wrap-safe
    /// — the screen is joined the way [`pane_collapsed`](crate::access::PaneAccess::pane_collapsed)
    /// joins it, so a marker the pane wrapped is one occurrence at any width.
    ///
    /// ⚠⚠ **A COUNT, and not each row's DAMAGE GENERATION, which is what this was.** A damage
    /// generation is a PAINT signal — it tells a renderer which rows to redraw — and a RESIZE
    /// (`Screen::reflowed`) or an OSC PALETTE change (`mark_all_dirty`) stamps every row with a
    /// fresh one while no program prints anything. Measured: a pane whose screen already carried
    /// the marker cleared this barrier **the instant anybody resized**, which in a terminal
    /// multiplexer is what every attaching client does.
    ///
    /// ⚠ The residue: text scrolling OFF lowers the count, so a marker that was on screen, scrolled
    /// away and was then printed afresh may not exceed its baseline. A false NEGATIVE — the safe
    /// direction — and [`Runs`](Self::Runs) has no such arithmetic.
    ///
    /// # ⚠⚠ AND THE PANE'S OWN ECHO IS DISCOUNTED, whenever it lands
    ///
    /// The generation baseline alone left a RACE, and it was measured rather than reasoned about: a
    /// pty echo is asynchronous, so a caller who writes the starting command and begins the run in
    /// the same breath can have the echo reach the grid AFTER the barrier armed. It is then new
    /// output by every test a screen can apply — and the barrier cleared on it, spending the run's
    /// turns on the shell that was still there. **The same call converged or drove into a shell
    /// depending on scheduling.**
    ///
    /// So the pane REMEMBERS what was written into it
    /// ([`PaneInputEcho`](crate::access::PaneInputEcho)), and a marker found in that text is
    /// refused as evidence outright. The race is not narrowed, it is REMOVED: the answer is the
    /// same whichever side of the arming the echo falls on, and it no longer depends on how loaded
    /// the machine is.
    ///
    /// ⚠ **A MARKER THAT APPEARS IN WHAT YOU TYPED CAN THEREFORE NEVER BE EVIDENCE**, and the run
    /// ends [`NeverReady`](crate::access::PaneError::NeverReady) naming it rather than driving
    /// something that was never listening. That is the honest answer to an ambiguous marker, and it
    /// is deterministic — which the alternative was not. Pick a marker the program prints, or open
    /// the pane running the program (`open_pane`'s `cmd`) so there is no echo at all.
    Prints(String),
    /// Ready once the marker is on the screen **now**, wherever it came from.
    ///
    /// The answer for *"it is already running"* — a REPL at its prompt, which will print nothing
    /// more until it is fed. This is the older behaviour, and it is the one that can be satisfied
    /// by an echo, so it is opt-in rather than the default.
    Shows(String),
    /// Ready once the program named here OWNS THE PANE'S TERMINAL. **Prefer this one.**
    ///
    /// # ⚠⚠ The only kind that does not read the screen, and the only one a silent program has
    ///
    /// The two kinds above are predicates over TEXT, and a program that prints nothing on startup
    /// emits no text to predicate over. There is no marker to name for `cat`, for a REPL launched
    /// `--quiet`, for a relay that speaks only when spoken to — so for that whole class the barrier
    /// had no usable answer at all, and a caller's realistic options were to guess a sleep or to
    /// drive a pane that might still be a shell.
    ///
    /// This asks the operating system instead. A shell hands its terminal to the job it runs and
    /// takes it back when that job ends; the pane's terminal therefore NAMES what is running in it,
    /// with no screen involved. That fact:
    ///
    /// * **cannot be echoed** — it is not text, so no amount of typing the word can satisfy it, and
    ///   the whole echo hazard the two kinds above spend their documentation on does not arise;
    /// * **does not depend on scheduling** — it is a state, not an event that may or may not have
    ///   landed on a grid before the barrier armed;
    /// * **works for a program that never prints**, which is the case that has no other answer;
    /// * **is the same value either way a pane was made** — a pane opened running the program
    ///   (`open_pane`'s `cmd`) matches from birth, and a shell typed into matches when the program
    ///   starts. One spelling, both shapes.
    ///
    /// # What the name is matched against
    ///
    /// The job LEADER's kernel name, or the basename of its `argv[0]` — either, because the two
    /// honestly disagree and a caller should not have to know which. `exec awk …` on a Debian box
    /// is a leader whose kernel name is `mawk` and whose `argv[0]` is `awk`; a program that rewrote
    /// its own argv still has its kernel name; a name longer than 15 bytes is truncated in the
    /// kernel's copy and intact in `argv[0]`.
    ///
    /// Matched EXACTLY, never as a prefix: a prefix match is a silent merge, and `claude` matching
    /// `claude-relay` is a run driving the wrong program while reporting success.
    ///
    /// ⚠ A leader is not every process of the job — `cargo build | less` is led by `cargo`. See
    /// [`foreground_leader_of`](sprag_terminal::foreground_leader_of).
    ///
    /// ⚠ On a host that cannot see the process table
    /// ([`PaneAccess::foreground_job`] is `None`) this
    /// is never satisfied, and the run ends [`NeverReady`](crate::access::PaneError::NeverReady)
    /// rather than typing into whatever is there.
    Runs(String),
    /// Ready once the AGENT named here is **at rest and waiting for input**. The strongest of the
    /// four, and the one to prefer when the pane runs an agent this host can supervise.
    ///
    /// # ⚠⚠ Owning the terminal is not the same as listening
    ///
    /// [`Runs`](Self::Runs) clears the moment a program takes the pane's terminal, which for a
    /// cold agent is seconds before it will answer anything: the model is still starting, and a
    /// prompt sent into that window is typed at something that has not finished reading its own
    /// configuration. `Runs` is the honest answer to *has it started*, and callers kept needing the
    /// next question — *has it started AND stopped again, waiting for me*.
    ///
    /// One condition answers both halves, because an
    /// [`AgentObservation`](crate::access::AgentObservation) carries WHICH agent alongside what it
    /// is doing. So this is not a composition of two barriers; it is the barrier the supervisor was
    /// already able to answer and nothing asked it for.
    ///
    /// # What counts as ready, and what deliberately does not
    ///
    /// [`Idle`](sprag_detect::AgentState::Idle) — *at rest, waiting for input it has not asked
    /// for* — and nothing else.
    ///
    /// * **`Working` is not ready**, which is the entire point: it is the state `Runs` cannot
    ///   distinguish from readiness.
    /// * ⚠ **`Blocked` is not ready either, and that is a decision rather than an omission.** A
    ///   blocked agent is waiting for an ANSWER TO ITS OWN QUESTION, and a fresh prompt sent there
    ///   answers the wrong thing — often into a numbered menu, where it selects. A caller who means
    ///   to answer a blocked agent is supervising it, not waiting to start.
    /// * **An observation that names no agent is not ready**, however idle it looks: two panes at
    ///   rest are not evidence about WHICH program is at rest, and this kind's whole value is that
    ///   it names one.
    ///
    /// ⚠ **The evidence may be a SCREEN RULE rather than the agent's own report**
    /// ([`Authority`](crate::access::Authority)), and this accepts either. That is a real trade and
    /// not an oversight: a scraped answer is APPROXIMATE, but it is deterministic — it is not the
    /// scheduling-dependent ambiguity the other kinds' documentation is about — and refusing it
    /// would leave a caller whose agent does not self-report with no way to ask this question at
    /// all. [`Runs`](Self::Runs) is the exact-but-weaker alternative, and it is one word away.
    ///
    /// ⚠ On a host with no detector at all ([`PaneAccess::supervision`] is `None`) this is never
    /// satisfied, on the same terms as [`Runs`](Self::Runs).
    Settles(String),
}

impl ReadyWhen {
    /// The two words a caller may spell, in this type's own order.
    ///
    /// Published to every mouth from here rather than retyped as literals, so a third kind reaches
    /// the wire in the compile that adds it.
    pub const WIRE_WORDS: &'static [&'static str] = &["prints", "shows", "runs", "settles"];

    /// The kind named by `word`, or `None` for a word outside the closed set.
    ///
    /// ⚠ A caller who sends something else has made a MALFORMED request, not a rejected one —
    /// R353's rule, and the reason this returns an `Option` for the parser to turn into the wire's
    /// own grammar refusal rather than a friendly sentence.
    ///
    /// ⚠⚠ **AN EMPTY MARKER IS REFUSED**, because it is a different wrong answer in each kind and
    /// none of them is what a caller meant: `""` is on every screen, so [`Shows`](Self::Shows)
    /// clears instantly and the barrier is a no-op that LOOKS like a barrier; no process is named
    /// `""`, so [`Runs`](Self::Runs) can never clear; and counting occurrences of `""` is
    /// arithmetic on nothing. The type is `String` and the argument admits fewer values than the
    /// type — R352's shape, and the fix is one predicate the parser and the publication share.
    #[must_use]
    pub fn parse(word: &str, marker: String) -> Option<Self> {
        if marker.is_empty() {
            return None;
        }
        match word {
            "prints" => Some(Self::Prints(marker)),
            "shows" => Some(Self::Shows(marker)),
            "runs" => Some(Self::Runs(marker)),
            "settles" => Some(Self::Settles(marker)),
            _ => None,
        }
    }

    /// The word this kind is spelled as on the wire.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Prints(_) => "prints",
            Self::Shows(_) => "shows",
            Self::Runs(_) => "runs",
            Self::Settles(_) => "settles",
        }
    }

    /// The text the pane must carry — or, for [`Runs`](Self::Runs), the program it must be running.
    #[must_use]
    pub fn marker(&self) -> &str {
        match self {
            Self::Prints(marker)
            | Self::Shows(marker)
            | Self::Runs(marker)
            | Self::Settles(marker) => marker,
        }
    }

    /// This question as the PAST-TENSE clause of a sentence about a pane that never answered it —
    /// `printed "UP"`, `showed ">>> "`, `ran "claude"`.
    ///
    /// ⚠ The reason [`PaneError::NeverReady`] carries the
    /// whole kind rather than the marker alone. Its sentence is what an agent reads when a run
    /// fails, and *"the pane never showed `claude`"* is false of two of the three kinds — one waits
    /// for text to be PRINTED and one for a program to be RUNNING, neither of which is showing.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Prints(marker) => format!("printed {marker:?}"),
            Self::Shows(marker) => format!("showed {marker:?}"),
            Self::Runs(name) => format!("ran {name:?}"),
            Self::Settles(agent) => format!("settled as {agent:?}, at rest and waiting for input"),
        }
    }
}

/// A pane's readiness barrier: what it must show before a plugin types into it, which question
/// that is asking ([`ReadyWhen`]), and how long the plugin will wait.
///
/// Latched — the wait happens once and every later step drives straight away, so a run pays for
/// this on its first step only.
#[derive(Clone, Debug)]
pub struct Readiness {
    /// What the pane must show, and which question it answers. `None` starts driving immediately,
    /// which is right for a pane the caller knows is already running the program.
    when: Option<ReadyWhen>,
    /// How long to wait for it. See [`DEFAULT_READY_TIMEOUT`] for why this is the caller's.
    within: Duration,
    /// Every row's damage generation when this barrier ARMED, for [`ReadyWhen::Prints`].
    ///
    /// How many times the marker was ALREADY on the collapsed screen when this barrier armed, for
    /// [`ReadyWhen::Prints`].
    ///
    /// Captured on the first look rather than at construction, because that is the moment the
    /// question is first asked and the only one a `PaneAccess` is in hand for. `None` until then.
    ///
    /// ⚠ A COUNT, and not each row's damage generation, which is what this was. See the comment in
    /// [`satisfied`](Self::satisfied): a generation says a row was REPAINTED, and a resize repaints
    /// every one of them.
    armed_at: Option<usize>,
    /// Whether the marker has been seen. Latched.
    seen: bool,
    /// WHAT THIS RUN MAY ANSWER when the peer stops to ask — `None` for a run that may answer
    /// nothing, which is the default and what every run did before the contract existed.
    ///
    /// ⚠ It lives on the BARRIER because this is the one place all three injecting plugins pass
    /// through on their way to a keystroke, and a second door to a blocked pane is the shape this
    /// crate keeps finding defects in. The consent is the only thing that may be typed at a peer
    /// that is asking, and it is typed here or nowhere.
    consent: Option<Consent>,
}

impl Readiness {
    /// A barrier for `when`, waiting `within` (defaulting to [`DEFAULT_READY_TIMEOUT`]), answering
    /// its peer's questions under `consent`.
    ///
    /// A `None` condition is a barrier that is already down: the caller is saying the pane is
    /// running what they mean to drive. A `None` consent is a run that answers nothing.
    ///
    /// ⚠ The consent is a PARAMETER and not a builder, so a plugin that injects cannot be written
    /// without deciding what it does about a blocked peer — [`Plugin::driving`]'s reasoning, which
    /// is the other question in this crate whose harmless-looking default was a wrong answer.
    ///
    /// [`Plugin::driving`]: crate::plugin::Plugin::driving
    #[must_use]
    pub fn new(
        when: Option<ReadyWhen>,
        within: Option<Duration>,
        consent: Option<Consent>,
    ) -> Self {
        Self {
            seen: when.is_none(),
            when,
            within: within.unwrap_or(DEFAULT_READY_TIMEOUT),
            armed_at: None,
            consent,
        }
    }

    /// The peer stopped to ask. Answer it if — and only if — the caller consented to exactly this
    /// question and exactly one of its options; otherwise say why nothing was typed.
    ///
    /// # ⚠⚠⚠ Every keystroke sent from here is justified by the peer's OWN marker
    ///
    /// The three shapes are [`Taken`]'s, and the reasoning is worth having in one place because it
    /// is the difference between a supervisor and a machine that clicks approvals:
    ///
    /// * **The marker is already on the authorised option.** Then a bare Enter takes THAT option
    ///   and cannot take another — [`Question::selected`] is exactly this fact — so no number is
    ///   typed at all. Typing one would be a keystroke with no purpose and, in a dialog whose
    ///   numbers submit outright, a second act nobody authorised.
    /// * **The marker is elsewhere.** The number is typed, and then one of two things is TRUE
    ///   rather than assumed: the peer left the question (it took the number), or the peer's marker
    ///   moved onto the authorised option (it processed the number and wants an Enter). The Enter
    ///   is sent only in the second case, where the marker having moved is both the proof the key
    ///   was read AND the guarantee of where the Enter will land.
    ///
    /// ⚠ **NO ENTER IS EVER SENT ON A GUESS**, which is the rule the measured hazard demands: the
    /// agents this reads submit on the number, so a reflexive `number + Enter` would put a stray
    /// Enter into whatever the peer showed NEXT — a second dialog, most dangerously.
    ///
    /// ⚠ The residue, stated: a peer whose marker never moves and which never leaves the question
    /// keeps a typed digit. That is [`Refusal::NotTaken`], the run stops, and the remedy is a
    /// person — the same direction every other unknown in this module fails in.
    ///
    /// # ⚠⚠ A consent is STANDING, and what makes that safe is the tally
    ///
    /// Nothing here latches, so a run whose peer asks the same question on every turn answers it on
    /// every turn — which is the point: a fifty-turn loop that stopped after the first
    /// tool-permission dialog would be a loop somebody has to sit with. The failure mode it opens
    /// is a peer that asks in a cycle, and that is bounded twice and hidden neither time: the
    /// run's own [`max_iterations`](crate::driver::Guardrails::max_iterations) ends it, and
    /// [`Outcome::answered`](crate::driver::Outcome::answered) says how many decisions were taken
    /// on the caller's behalf getting there. **A count of approvals is what makes an unlatched
    /// consent auditable rather than merely convenient.**
    ///
    /// # Errors
    ///
    /// [`PaneError`] from the injection itself — an unencodable key or a write failure, which is a
    /// failure of the run and not a refusal of the answer.
    fn answer(
        &self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        asking: Option<Question>,
        run: &RunContext,
    ) -> Result<Reached, PaneError> {
        let Some(question) = asking else {
            return Ok(Reached::Asking(Unanswered::unreadable()));
        };
        let Some(consent) = self.consent.as_ref() else {
            return Ok(Reached::Asking(Unanswered::refused(
                question,
                Refusal::NoConsent,
            )));
        };
        let chose = match consent.covers(&question) {
            Ok(choice) => choice.clone(),
            Err(why) => return Ok(Reached::Asking(Unanswered::refused(question, why))),
        };

        let standing_on_it = question.selected() == Some(&chose);
        // ⚠ THE KEY WHOSE LANDING PLACE IS PROVEN GOES FIRST. Where the peer's own marker is
        // already on the authorised option, that key is Enter — `Question::selected` says exactly
        // where it lands. Everywhere else it is the number, whose effect is inferred and whose
        // proof arrives afterwards, as the marker.
        let mut bytes = if standing_on_it {
            panes.inject(pane, &[KeyStroke::named("Enter")])?.bytes()
        } else {
            panes
                .inject(pane, &KeyStroke::text(&chose.number.to_string()))?
                .bytes()
        };
        let mut how = if standing_on_it {
            Taken::Selected
        } else {
            Taken::Numbered
        };

        // What would END the wait differs by which key went in, and conflating them was a defect
        // an end-to-end run measured. For a NUMBER, the marker arriving on the option is news. For
        // an ENTER it is not — the marker was already there, so a wait that treated it as a signal
        // would return before the peer had touched the key.
        let settled = poll_until(run, ANSWER_WITHIN, || {
            match marker_arrived(panes, pane, &question, &chose) {
                Arrival::LeftTheQuestion => true,
                Arrival::OnTheOption => !standing_on_it,
                Arrival::NotYet => false,
            }
        });
        // ⚠ A run ending underneath does not un-type the key, and it does not answer the question
        // either. Both endings are the same sentence to whoever reads the run — see
        // [`Refusal::NotTaken`] — and both carry what was spent.
        if settled == Waited::Stopped {
            return Ok(Reached::Asking(Unanswered::not_taken(question, bytes)));
        }

        // ⚠⚠⚠ THE SECOND KEY, and it is the OTHER one — sent only where the same question is still
        // up with the marker still on the authorised option, which is the same evidence the first
        // key was justified by.
        //
        // Both directions are real dialogs and neither can be told from a screen. A NUMBER that
        // moved the marker wants an Enter to commit it. An ENTER the peer ignored — a menu with
        // number hotkeys and no Enter handling — wants the number, and until an end-to-end run
        // MEASURED one, the commonest consent there was (`Yes`, on the pre-selected option) could
        // not be answered at all: the run pressed the one key that dialog does not read and
        // reported `not_taken`.
        //
        // ⚠ The escalation cannot confirm something else by accident. It happens only while the
        // screen still shows THIS question with the marker on THIS option, so a first key that had
        // in fact landed would have taken the dialog away and there would be nothing to escalate
        // into.
        if marker_arrived(panes, pane, &question, &chose) == Arrival::OnTheOption {
            if standing_on_it {
                bytes += panes
                    .inject(pane, &KeyStroke::text(&chose.number.to_string()))?
                    .bytes();
                how = Taken::SelectedThenNumbered;
            } else {
                bytes += panes.inject(pane, &[KeyStroke::named("Enter")])?.bytes();
                how = Taken::NumberedThenConfirmed;
            }
        } else if settled == Waited::TimedOut {
            // The peer neither left the question nor put its marker where this run could act on
            // it. Nothing further is justified, and re-typing is what this contract exists to not
            // do.
            return Ok(Reached::Asking(Unanswered::not_taken(question, bytes)));
        }

        // ⚠⚠ AN ANSWER IS NOT GIVEN UNTIL THE PEER LEAVES THE QUESTION. A run that reported one off
        // its own keystroke would report success for a dialog still on the screen, which is
        // precisely the claim `Reached::Asking` was built to stop being made silently.
        //
        // ⚠⚠⚠ THROUGH THE SAME PREDICATE THE REST OF THIS ACT USES, and that is a fix rather than
        // tidiness. Asked as *"is the pane still blocked"* this outlived the answering bound on
        // every real daemon: a detector's verdict SETTLES, so the state says blocked for its
        // hysteresis window after the menu has gone. A run whose answer had plainly landed reported
        // `not_taken`. See [`Arrival::LeftTheQuestion`].
        match poll_until(run, ANSWER_WITHIN, || {
            marker_arrived(panes, pane, &question, &chose) == Arrival::LeftTheQuestion
        }) {
            Waited::Ready => Ok(Reached::Answered(Answered {
                question,
                chose,
                how,
                bytes,
            })),
            Waited::Stopped | Waited::TimedOut => {
                Ok(Reached::Asking(Unanswered::not_taken(question, bytes)))
            }
        }
    }

    /// Whether `pane` satisfies `when` right now.
    fn satisfied(&self, when: &ReadyWhen, panes: &dyn PaneAccess, pane: PaneId) -> bool {
        match when {
            ReadyWhen::Runs(name) => panes
                .foreground_job()
                .and_then(|jobs| jobs.pane_foreground_leader(pane))
                .is_some_and(|leader| JobLeader::of(&leader).answers_to(name)),
            // ⚠⚠ IDLE AND NAMED, both halves from ONE observation. `Working` is the state `Runs`
            // cannot tell from readiness, which is why this kind exists; `Blocked` is waiting for
            // an answer to its OWN question, where a fresh prompt answers the wrong thing; and an
            // observation naming no agent is not evidence about WHICH program is at rest.
            ReadyWhen::Settles(agent) => panes
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(pane))
                .is_some_and(|seen| {
                    seen.state == AgentState::Idle && seen.agent.as_deref() == Some(agent.as_str())
                }),
            ReadyWhen::Shows(marker) => panes
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(marker.as_str())),
            ReadyWhen::Prints(marker) => {
                let Some(text) = panes.pane_collapsed(pane) else {
                    return false;
                };
                // ⚠⚠ WHAT WAS TYPED AT THE PANE IS NOT WHAT THE PANE SAID. The pty echoes it, and
                // on the grid the echo is ordinary output — so a row carrying a piece of the
                // caller's own input is dropped before anything is read. This is the same rule
                // `Orchestrator::reaction` applies to its own stimulus, asked of input this plugin
                // did not write, which is only possible because the PANE remembers it.
                //
                // ⚠ Absent the capability the discount cannot be applied, and the fallback is the
                // arming count alone — weaker, and the reason `input_echo` returning `None` is
                // documented as a degradation rather than a default.
                let typed = panes
                    .input_echo()
                    .and_then(|echo| echo.pane_recent_input(pane))
                    .unwrap_or_default();
                // ⚠⚠ A MARKER THAT IS IN WHAT WAS TYPED AT THE PANE IS NOT EVIDENCE, EVER. The pty
                // echoes input, and on the grid that echo is ordinary output — so the marker has
                // two possible authors and nothing on the screen says which. Refusing it here is
                // what makes the answer DETERMINISTIC: the alternative was to hope the echo had
                // already landed before the barrier armed, and the same call then converged or fed
                // the shell depending on scheduling.
                //
                // ⚠ Checked against the MARKER rather than by dropping echoing ROWS. Dropping rows
                // depends on where the terminal happened to wrap and on whether a prompt shares
                // the line, which is the same non-determinism one step further in.
                if typed.contains(marker.as_str()) {
                    return false;
                }
                // ⚠⚠ MORE OCCURRENCES THAN WHEN THE BARRIER ARMED — counted over the whole
                // collapsed screen, never over rows a DAMAGE GENERATION says were repainted.
                //
                // A damage generation is a PAINT signal: it exists so a renderer knows what to
                // redraw. Two ordinary events stamp every row with a fresh one while no program
                // prints anything — a RESIZE (`Screen::reflowed`, which is what every attaching
                // client causes, in a terminal multiplexer of all products) and an OSC PALETTE
                // change (`mark_all_dirty`, which many programs send on startup). Answering *did
                // the pane print this* with it cleared the barrier on text that was already there.
                //
                // A count is immune to both, and to the re-wrap between them: the screen is
                // collapsed the way [`PaneAccess::pane_collapsed`] joins it, so a marker the pane
                // wrapped is ONE occurrence at either width.
                //
                // ⚠ The residue, stated rather than smoothed over: text scrolling off LOWERS the
                // count, so a marker that was on screen twice, scrolled away and was then printed
                // afresh does not exceed its baseline. That is a false NEGATIVE — the safe
                // direction — and [`ReadyWhen::Runs`] is the kind that has no such arithmetic.
                text.matches(marker.as_str()).count() > self.armed_at.unwrap_or(0)
            }
        }
    }

    /// Wait (once, then latched) for `pane` to satisfy the barrier.
    ///
    /// # Errors
    ///
    /// [`PaneError::NeverReady`] when the caller's bound elapses with the marker unseen and the
    /// run still has time. Driving on would inject into whatever IS there and report turns
    /// against a peer that was never listening, so the caller's run stops and says exactly what it
    /// was waiting for.
    ///
    /// ⚠ The RUN's own deadline is the other bound, and it is consulted by `poll_until`: a run
    /// whose clock is shorter ends as [`Reached::RunEnded`] instead. Both are real endings and
    /// they are different findings — one is *this pane never came up*, the other is *the run was
    /// out of time*, and only the first is about the pane.
    pub fn reached(
        &mut self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        run: &RunContext,
    ) -> Result<Reached, PaneError> {
        // ⚠⚠⚠ ASKED EVERY TIME, BEFORE THE LATCH AND BEFORE THE CONDITION. *Has it started* is
        // answered once; *is it waiting on a question of its own* is not, and this is the only
        // place all three injecting plugins pass through on their way to a keystroke. Put after
        // the latch it would never run again after the first step, which is exactly the window the
        // defect lived in.
        if let Some(asking) = settled_question(panes, pane, run) {
            return self.answer(panes, pane, asking, run);
        }
        if self.seen {
            return Ok(Reached::Yes);
        }
        // ⚠ A BARRIER WITH NO CONDITION IS ALREADY DOWN, and saying so HERE is what keeps the
        // failure below honest. `seen` is set from `when.is_none()` at construction, so this arm is
        // unreachable in practice — but taking the condition out of the `Option` now means the
        // `NeverReady` error cannot be constructed without one. The alternative was a fabricated
        // empty marker for a case that cannot happen, which is a false sentence waiting for a
        // refactor to make it reachable.
        let Some(when) = self.when.clone() else {
            self.seen = true;
            return Ok(Reached::Yes);
        };
        // ⚠ ARM BEFORE THE FIRST LOOK, never before. Every occurrence of the marker on the screen
        // at this instant is one `Prints` refuses to count, and this is the first moment a pane is
        // in hand to read them from.
        if self.armed_at.is_none() {
            self.armed_at = Some(
                panes
                    .pane_collapsed(pane)
                    .map_or(0, |text| text.matches(when.marker()).count()),
            );
        }
        match poll_until(run, self.within, || self.satisfied(&when, panes, pane)) {
            Waited::Ready => {
                self.seen = true;
                Ok(Reached::Yes)
            }
            Waited::Stopped => Ok(Reached::RunEnded),
            // ⚠ THE DIAGNOSTIC IS READ AT THE MOMENT OF FAILURE, not carried from arming: what a
            // caller needs is what the pane was doing when the wait gave up. One read, on the way
            // out of a run that is already over.
            Waited::TimedOut => Err(PaneError::NeverReady {
                wanted: when,
                // ⚠ THE ABSENCE OF THE CAPABILITY AND THE ABSENCE OF A JOB ARE DIFFERENT ANSWERS —
                // one is about this build, the other about this pane. See [`PaneDoing`].
                instead: panes.foreground_job().map_or(PaneDoing::Unknown, |jobs| {
                    jobs.pane_foreground_leader(pane).map_or(
                        PaneDoing::Nothing,
                        // ⚠⚠ THE SAME TYPE THE PREDICATE DECIDED WITH. Reporting one of the two
                        // names it accepts is what named `"bash"` at a caller who launched
                        // `/bin/sh`, and it differed by platform. See [`JobLeader`].
                        |leader| PaneDoing::Job(JobLeader::of(&leader)),
                    )
                }),
            }),
        }
    }
}

// ⚠ WHAT `Runs` DECIDES WITH LIVES ON [`JobLeader`], beside the report that has to agree with it.
// It was a private function here, and the report was a field read in `access.rs`, which is how the
// two came to answer differently for the same job.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use sprag_terminal::{CommandBuilder, JobProcess, Workspace};
    use std::sync::{Arc, Mutex};

    /// ⚠⚠ **EVERY WORD THE VOCABULARY PUBLISHES IS ONE THE PARSER READS, AND BACK.**
    ///
    /// [`ReadyWhen::WIRE_WORDS`] is what the wire advertises as this argument's closed set, and
    /// [`ReadyWhen::parse`] is what the daemon reads it with. They are two spellings of one
    /// vocabulary, and R353 measured that shape going wrong in this workspace already — a mouse
    /// encoder and its decoder, each documented as the other's twin, with nothing comparing the
    /// lists. Driven from `WIRE_WORDS` rather than from a literal here, so a third kind added to
    /// the type fails this the moment its word is published without a parser arm.
    ///
    /// ⚠ [`word`](ReadyWhen::word) is the reader this gate exists to give the vocabulary: without
    /// it the type could publish a word it cannot spell back, which is how the two lists drift.
    #[test]
    fn every_published_readiness_word_round_trips_through_the_parser() {
        for word in ReadyWhen::WIRE_WORDS {
            let parsed = ReadyWhen::parse(word, "MARK".to_string())
                .unwrap_or_else(|| panic!("{word:?} is published and the parser refuses it"));
            assert_eq!(
                parsed.word(),
                *word,
                "and it must spell back the word it was read from",
            );
            assert_eq!(parsed.marker(), "MARK", "carrying the caller's marker");
        }
        assert_eq!(
            ReadyWhen::WIRE_WORDS.len(),
            4,
            "the four questions a readiness marker can ask, in increasing strength: whether the \
             pane PRINTS it after the run arms, whether it SHOWS it already, whether the pane's \
             terminal belongs to a job that RUNS it, or whether the agent SETTLES as it and waits \
             — and only the first two are questions about the screen",
        );
        assert!(
            ReadyWhen::parse("appears", "MARK".to_string()).is_none(),
            "and a word outside the set is refused, or the published `enum` is a false statement",
        );
        // ⚠⚠ AND AN EMPTY MARKER, IN EVERY KIND — driven from the published words rather than
        // spot-checked on one, because the reason differs per kind and the refusal must not.
        // `Shows("")` is the dangerous one: every screen contains `""`, so the barrier clears
        // instantly while LOOKING like a barrier the caller asked for.
        for word in ReadyWhen::WIRE_WORDS {
            assert!(
                ReadyWhen::parse(word, String::new()).is_none(),
                "{word:?} with an empty marker is a MALFORMED request, not a barrier: the \
                 argument admits fewer values than its `String` type",
            );
            // ⚠⚠ AND EVERY KIND CAN SAY ITS OWN FAILURE. `describe` is what
            // `PaneError::NeverReady`'s sentence is built from, and it is an exhaustive match —
            // so a fifth kind compiles the moment it is added and would reach an agent with
            // whatever clause its author wrote. Driven from the published words so each arm is
            // BUILT here rather than trusted.
            let said = ReadyWhen::parse(word, "MARK".to_string())
                .expect("published")
                .describe();
            assert!(
                said.contains("MARK") && said.starts_with(char::is_lowercase),
                "{word:?} must describe itself as a past-tense clause naming the marker — it is \
                 read after \"the pane never \": {said:?}",
            );
        }
    }

    /// ⚠⚠ **OWNING THE TERMINAL IS NOT LISTENING** — the gap [`ReadyWhen::Runs`] cannot close, and
    /// the reason [`ReadyWhen::Settles`] exists.
    ///
    /// A cold agent takes its pane's terminal seconds before it will answer anything. `Runs` clears
    /// at the first instant — correctly, because *has it started* is the question it answers — and a
    /// caller who meant *is it waiting for me* has been driving a starting program ever since.
    ///
    /// **The discriminator is the SAME PANE AT THE SAME MOMENT**, asked both ways. The supervisor
    /// reports `Working` (the agent is up, thinking) and the fixture asserts:
    ///
    /// 1. `Runs("tr")` IS satisfied — the program owns the terminal, which is true and not enough;
    /// 2. `Settles("claude")` is NOT — it is working, not waiting;
    /// 3. and once the same supervisor reports `Idle`, `Settles` clears.
    ///
    /// Half 1 is what makes this a discriminator rather than a restatement: without it the gate
    /// would prove only that a made-up condition is unsatisfied.
    ///
    /// ⚠ `Blocked` gets its own half, because it is the arm most likely to be "fixed" into
    /// readiness by someone reading only the state name. A blocked agent is waiting for an answer
    /// to ITS OWN question — a prompt sent there answers the wrong thing, and into a numbered menu
    /// it SELECTS.
    #[test]
    fn a_program_that_owns_the_terminal_is_not_yet_an_agent_that_is_listening() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec tr a-z A-Z");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        // The supervisor this host installs, driven by the test rather than by a screen rule — the
        // question here is what the BARRIER does with an observation, not how one is derived.
        let reported = Arc::new(Mutex::new((
            AgentState::Working,
            Some("claude".to_string()),
        )));
        let source = {
            let reported = Arc::clone(&reported);
            Arc::new(move |_id: PaneId| {
                let (state, agent) = reported.lock().unwrap().clone();
                Some(crate::access::AgentObservation {
                    state,
                    agent,
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq: 1,
                    asking: None,
                })
            })
        };
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));

        let settled = |access: &WorkspacePaneAccess| {
            Readiness::new(
                Some(ReadyWhen::Settles("claude".to_string())),
                Some(Duration::from_millis(150)),
                None,
            )
            .reached(access, pane, &RunContext::uncancellable())
        };

        // ⚠ HALF 1, THE CONTROL: the weaker question is ALREADY satisfied at this instant.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(50)),
                None,
            )
            .reached(&access, pane, &RunContext::uncancellable())
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(200)),
                None
            )
            .reached(&access, pane, &RunContext::uncancellable()),
            Ok(Reached::Yes),
            "the program owns the terminal — so this gate is about the DIFFERENCE between the two \
             questions, not about a pane that never came up",
        );

        // HALF 2: working is not waiting.
        crate::testing::refused_naming(
            settled(&access).as_ref().err(),
            &ReadyWhen::Settles("claude".to_string()),
            "tr",
            "an agent that is WORKING is not ready to be typed at, however firmly it owns the \
             terminal",
        );

        // HALF 3: blocked is waiting for an answer to its own question, which is not this.
        //
        // ⚠⚠ THE CLAIM IS UNCHANGED AND THE ANSWER MOVED. This asserted `is_err()` — the barrier
        // simply never cleared, so the caller waited out `ready_within` and got `NeverReady`,
        // which protected the pane while saying nothing about WHY. `Reached::Asking` is the same
        // protection reached IMMEDIATELY and carrying the question, so a run can report what its
        // peer wants instead of a generic refusal. Asserting the variant rather than "not Yes"
        // because *typed into anyway* and *told the peer is asking* are different products.
        *reported.lock().unwrap() = (AgentState::Blocked, Some("claude".to_string()));
        assert!(
            matches!(settled(&access), Ok(Reached::Asking(_))),
            "a BLOCKED agent is waiting for an answer to its own question — a fresh prompt sent \
             there answers the wrong thing, and into a numbered menu it selects. The barrier must \
             say so rather than merely failing to open: {:?}",
            settled(&access),
        );

        // HALF 4: an idle observation that names no agent says nothing about WHICH is at rest.
        *reported.lock().unwrap() = (AgentState::Idle, None);
        assert!(
            settled(&access).is_err(),
            "an observation that names no agent is not evidence about which program is at rest",
        );

        // HALF 5: named and idle.
        *reported.lock().unwrap() = (AgentState::Idle, Some("claude".to_string()));
        assert_eq!(
            settled(&access),
            Ok(Reached::Yes),
            "the agent the caller named is at rest and waiting for input — NOW drive it",
        );
    }

    /// ⚠⚠ **A REPAINT IS NOT A PRINT, AND A RESIZE REPAINTS EVERY ROW.**
    ///
    /// [`ReadyWhen::Prints`] baselined each row's DAMAGE GENERATION and counted a row as evidence
    /// once that number moved past the baseline. A damage generation is a PAINT signal — it exists
    /// so a renderer knows which rows to redraw — and answering a CONTENT question with it is the
    /// category error this gate names. Two ordinary things stamp every row with a fresh generation
    /// without a program printing anything:
    ///
    /// * **a RESIZE** (`Emulator::resize` → `Screen::reflowed(cols, rows, g)`), which is what every
    ///   client attaching to a session does — in a terminal multiplexer, of all products;
    /// * **an OSC palette change** (`repaint_for_palette_change` → `mark_all_dirty`), which many
    ///   programs send on startup.
    ///
    /// So a pane whose screen ALREADY carried the marker — a word from an earlier command, a
    /// banner, anything the caller did not type and the echo trail therefore never saw — cleared the
    /// barrier the instant anybody resized. The run then drove whatever was there.
    ///
    /// ⚠ This fixture uses a REAL pty and a REAL resize rather than a hand-built row list, because
    /// the claim is about what the emulator does, and a double asserting my own belief about that
    /// would be the gate believing itself.
    #[test]
    fn a_repaint_of_text_that_was_already_there_is_not_the_pane_printing_it() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // ⚠ The marker is printed BY THE PROGRAM and never typed at the pane, so the echo trail
            // does not cover it — this is the half R359c's fix cannot reach.
            command.arg("printf 'BANNER\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("BANNER"))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("BANNER")),
            "the fixture must get the marker on screen BEFORE the barrier arms, or it is not \
             asking this question at all",
        );

        let mut ready = Readiness::new(
            Some(ReadyWhen::Prints("BANNER".to_string())),
            Some(Duration::from_millis(600)),
            None,
        );
        let outcome = std::thread::scope(|scope| {
            let waiting =
                scope.spawn(|| ready.reached(&access, pane, &RunContext::uncancellable()));
            // Let the barrier ARM (it baselines on its first look), then resize — which is what an
            // attaching client does, and what re-stamps every row.
            std::thread::sleep(Duration::from_millis(120));
            workspace
                .lock()
                .unwrap()
                .resize(pane, 30, 8, (0, 0))
                .expect("resize the pane");
            waiting.join().expect("the barrier thread")
        });

        crate::testing::refused_naming(
            outcome.as_ref().err(),
            &ReadyWhen::Prints("BANNER".to_string()),
            "cat",
            "text that was on screen when the barrier armed is NOT the pane printing it, however \
             many times the screen is re-laid out under it",
        );
    }

    /// ⚠⚠ **A PANE WHOSE CHILD HAS GONE SAYS SO, RATHER THAN BLAMING THE BUILD** — the third
    /// [`PaneDoing`] arm, and the reason that field stopped being an `Option`.
    ///
    /// A host that CAN see the process table and finds no job owning the terminal has learned
    /// something about the PANE. Spelled as the same `None` that means *this build has no process
    /// view*, it told the caller their deployment was blind when it was working perfectly.
    #[test]
    fn a_pane_whose_child_has_exited_is_reported_as_nothing_owning_its_terminal() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exit 0");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) && access.pane_eof(pane) != Some(true) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "the fixture's child must be GONE, or this is not the arm being built",
        );

        let failed = Readiness::new(
            Some(ReadyWhen::Runs("claude".to_string())),
            Some(Duration::from_millis(120)),
            None,
        )
        .reached(&access, pane, &RunContext::uncancellable())
        .expect_err("a pane with no child can never come to run anything");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Nothing,
            },
            "the host CAN see the process table and there is no job — that is a fact about the \
             PANE, and it must not read as a blind build",
        );
        assert!(
            failed.to_string().contains("the pane's child had gone"),
            "and the sentence says which: {failed}",
        );
    }

    /// ⚠⚠ **A HOST WITH NO ECHO TRAIL LOSES ONE DISCOUNT, NOT THE WHOLE BARRIER** — the degradation
    /// [`PaneAccess::input_echo`] documents, and which no gate built.
    ///
    /// `input_echo` returning `None` costs [`ReadyWhen::Prints`] its refusal of a marker the caller
    /// TYPED. It must not also cost the arming count, or a pane that already showed the marker
    /// would clear the barrier on a host that merely cannot see its own echo — the two protections
    /// are independent and only one of them is a capability.
    #[test]
    fn a_host_with_no_echo_trail_still_refuses_a_marker_that_was_already_on_screen() {
        /// Every optional capability at its default, and a screen that never changes.
        struct NoEchoTrail;
        impl PaneAccess for NoEchoTrail {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some("BANNER at rest".to_string())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some("BANNER at rest".to_string())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[crate::access::KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                panic!("⚠⚠ NOT ONE BYTE — the marker was on screen before the barrier armed");
            }
        }
        assert!(
            Readiness::new(
                Some(ReadyWhen::Prints("BANNER".to_string())),
                Some(Duration::from_millis(120)),
                None
            )
            .reached(&NoEchoTrail, PaneId(1), &RunContext::uncancellable())
            .is_err(),
            "the arming COUNT is not a capability and must protect a host that has no echo trail",
        );
    }

    /// ⚠⚠ **A JOB'S LEADER IS NOT EVERY PROCESS IN IT** — the documented limit of
    /// [`ReadyWhen::Runs`], measured rather than left as prose.
    ///
    /// `cat | tr` is ONE job of two processes led by the shell that started them, so a caller who
    /// names `tr` is naming a member and not the leader. The honest answer is that the barrier does
    /// not clear — and the failure NAMES the leader, which is exactly what tells the caller they
    /// asked about the wrong end of a pipeline.
    #[test]
    fn a_pipeline_is_led_by_its_shell_and_naming_a_member_does_not_clear() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("cat | tr a-z A-Z");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        crate::testing::refused_naming(
            Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(400)),
                None,
            )
            .reached(&access, pane, &RunContext::uncancellable())
            .as_ref()
            .err(),
            &ReadyWhen::Runs("tr".to_string()),
            "sh",
            "`tr` is a MEMBER of the job, not its leader — and the failure names the leader, which \
             is what tells a caller they named the wrong end of their pipeline",
        );
    }

    /// ⚠⚠⚠ **THE TWO NAMES DISAGREE ON A REAL PROCESS, AND THE REFUSAL HAS TO CARRY BOTH** — the
    /// macOS red, forced on the runner that was green.
    ///
    /// # What was wrong, and why no gate here could see it
    ///
    /// [`ReadyWhen::Runs`] accepts EITHER the kernel's name for the leader or the basename of its
    /// `argv[0]`, because the two sources honestly disagree and a caller cannot be asked which
    /// spelling their platform packages. [`PaneDoing::Job`] then reported only the KERNEL's. So a
    /// refusal named a program the caller never launched, and named a different one per platform:
    /// `/bin/sh` is `bash` on macOS, so a pane spawned identically was blamed on `"sh"` here and
    /// `"bash"` there. Every gate in this crate compared the whole error against a literal, so each
    /// of them had a platform's shell spelling baked in, and the divergence could only ever be
    /// discovered by pushing.
    ///
    /// # Forcing it, rather than waiting for the other runner
    ///
    /// `exec -a` gives a process an `argv[0]` unrelated to the file it runs — measured: comm
    /// `sleep`, `argv[0]` `shim` — which is macOS's `/bin/sh` case exactly, reproduced on Linux and
    /// true on both. So this gate is not about macOS: it is about a leader whose two names differ,
    /// which any wrapper, symlink or `busybox`-style multi-call binary produces.
    ///
    /// ⚠ REVERT-PROOF: make [`JobLeader::named`] answer the kernel's name and the last assertion
    /// fails, which is the state the product was in.
    #[test]
    fn a_leader_whose_two_names_differ_answers_to_both_and_the_refusal_carries_both() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            // `exec -a` is bash's, and `/bin/bash` is present on both runners. A shell that lacks
            // it must FAIL here rather than skip: a gate that quietly does not run is the thing
            // this round is paying off.
            let mut command = CommandBuilder::new("/bin/bash");
            command.arg("-c");
            command.arg("exec -a shim /bin/cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "shim".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let asks = |name: &str, within: Duration| {
            Readiness::new(Some(ReadyWhen::Runs(name.to_string())), Some(within), None).reached(
                &access,
                pane,
                &RunContext::uncancellable(),
            )
        };

        assert_eq!(
            asks("shim", Duration::from_secs(5)),
            Ok(Reached::Yes),
            "the name the caller LAUNCHED it under answers — this is the arm that carries macOS, \
             where the shell a pane is spawned as is not the file the kernel names",
        );
        assert_eq!(
            asks("cat", Duration::from_millis(400)),
            Ok(Reached::Yes),
            "and so does the kernel's, for a caller who read it off `ps` — accepting only one of \
             the two would make the answer depend on which they had looked at",
        );

        let refused = asks("tr", Duration::from_millis(400));
        crate::testing::refused_naming(
            refused.as_ref().err(),
            &ReadyWhen::Runs("tr".to_string()),
            "shim",
            "the refusal is about a leader that DOES answer to what the pane was launched as",
        );
        let sentence = refused.expect_err("the barrier refused").to_string();
        assert!(
            sentence.contains("\"shim\""),
            "⚠⚠ THE CALLER'S OWN WORD LEADS THE CORRECTION. Reporting only the kernel's name is \
             what told a macOS caller their `/bin/sh` pane belonged to \"bash\": {sentence:?}",
        );
        assert!(
            sentence.contains("\"cat\""),
            "and the kernel's name is not DROPPED either — it is the other spelling `Runs` would \
             have accepted, and a reader handed one of them cannot tell the other exists: \
             {sentence:?}",
        );
    }

    /// ⚠⚠⚠ **A MARKER THE PANE'S WIDTH WRAPPED AT A SPACE STOPPED MATCHING** — and the width is
    /// not the caller's to choose.
    ///
    /// [`ReadyWhen::Prints`] promises the join is *"wrap-safe … so a marker the pane wrapped is one
    /// occurrence at any width"*, and the join was `Screen::row_text`, whose own doc says it cannot
    /// be joined that way — it trims a continuing row's trailing blanks, and those blanks are
    /// INTERIOR to the line. Measured: five columns, `TOOL UP` wraps after the SPACE, the rows are
    /// `"TOOL "` and `"UP"`, and the barrier matched against `"TOOLUP"`.
    ///
    /// So a run against the same program with the same marker succeeded or hung **on the width of
    /// somebody else's window** — a client attaching at another size is what sets it, which is the
    /// same trigger as every other defect this front has paid.
    ///
    /// ⚠ Both halves: what the barrier READS, and what the barrier DOES. A join that is right in
    /// isolation proves nothing about a wait that has to end.
    ///
    /// ⚠ `row_text`'s doc names a SECOND hazard — the pad a wide cluster leaves at the margin — and
    /// it is deliberately NOT asserted here: measured through this surface, both readers answer
    /// `"ABCD한EF"`, because a trailing pad is trimmed away either way. **An assertion that passes
    /// with the defect in place is not a gate**, and writing one would have made this look like it
    /// covered a half it cannot reach.
    #[test]
    fn a_marker_the_width_wrapped_at_a_space_still_matches() {
        let workspace = Arc::new(Mutex::new(Workspace::new((5, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // Five columns: `TOOL ` fills the row and `UP` lands on the next, so the space is a
            // continuing row's trailing blank — the exact cell `row_text` throws away.
            command.arg("printf 'TOOL UP\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 5, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        assert_eq!(
            Readiness::new(
                Some(ReadyWhen::Shows("TOOL UP".to_string())),
                Some(Duration::from_secs(5)),
                None
            )
            .reached(&access, pane, &RunContext::uncancellable()),
            Ok(Reached::Yes),
            "the marker is on the pane; that the terminal broke the line inside it is the \
             terminal's business and not the caller's",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim_end(),
            "TOOL UP",
            "and the collapsed read is what the child WROTE — a row's share of its logical line, \
             not a row's rendering with its interior blanks trimmed away",
        );
    }

    /// ⚠⚠ **THE TWO NAMES A LEADER HAS, AND THE MERGE THAT MUST NOT HAPPEN.**
    ///
    /// [`JobLeader::answers_to`] is where [`ReadyWhen::Runs`] decides, and two of its claims are reachable
    /// from no pty fixture in this crate: every program the fixtures start (`tr`, `cat`) has a
    /// kernel name equal to its `argv[0]`, so the second arm is never taken and a prefix match would
    /// pass every one of them. Both are ordinary on a real box — `exec awk` where `/usr/bin/awk` is
    /// `mawk` is exactly the disagreement, and it is a packaging decision the caller cannot see.
    ///
    /// Four claims, each a different way to get this wrong:
    ///
    /// 1. the KERNEL name answers;
    /// 2. `argv[0]`'s BASENAME answers when the kernel name disagrees — the `mawk`/`awk` case, and
    ///    the reason a caller is not asked which spelling their distribution chose;
    /// 3. a PREFIX does not answer, in either direction. `claude` accepting `claude-relay` is a run
    ///    driving the wrong program and reporting success, which is worse than never starting;
    /// 4. a job with NO argv at all — a kernel thread, a zombie whose argv the kernel has already
    ///    released — still answers by its kernel name rather than panicking on an empty vector.
    #[test]
    fn a_leader_answers_to_its_kernel_name_or_its_argv_basename_and_to_neither_by_prefix() {
        let leader = |name: &str, argv: &[&str]| {
            JobLeader::of(&JobProcess {
                pid: 4242,
                name: name.to_string(),
                argv: argv.iter().map(|arg| (*arg).to_string()).collect(),
            })
        };

        let awk = leader("mawk", &["awk", "{print}"]);
        assert!(
            awk.answers_to("mawk"),
            "the kernel's name for the process answers",
        );
        assert!(
            awk.answers_to("awk"),
            "and so does what its parent called it — a caller who wrote `awk` on a box that \
             packages `mawk` is not wrong, and cannot be expected to know",
        );

        let absolute = leader("claude", &["/usr/local/bin/claude", "--print"]);
        assert!(
            absolute.answers_to("claude"),
            "an absolute `argv[0]` is matched by its BASENAME, or naming a program would mean \
             knowing where it was installed",
        );

        let relay = leader("claude-relay", &["claude-relay"]);
        assert!(
            !relay.answers_to("claude"),
            "⚠⚠ A PREFIX IS NOT A MATCH: `claude` accepting `claude-relay` is a run that drives \
             the wrong program and reports success",
        );
        assert!(
            !leader("cl", &["cl"]).answers_to("claude"),
            "and neither is the other direction",
        );

        assert!(
            leader("cat", &[]).answers_to("cat"),
            "a job with no argv at all — a zombie's is released at exit — still answers by the \
             name the kernel keeps",
        );
    }

    /// ⚠⚠ **A HOST THAT CANNOT SEE THE PROCESS TABLE FAILS THE RUN RATHER THAN DRIVING IT** — the
    /// arm every pty gate in this crate skips, because the production access implements the
    /// capability and therefore never builds the absence.
    ///
    /// [`PaneAccess::foreground_job`] defaults to `None`, and that default is what a port to a
    /// platform with no process table would land on. A capability that is missing has exactly two
    /// possible readings and only one of them is safe: *"cannot tell, so assume ready"* types into
    /// whatever is there, which is the whole failure the barrier exists to prevent. **The safe
    /// direction is documented on the trait and, until this gate, was asserted nowhere** — the
    /// same shape the echo trail's own absence still owes, and not one worth owing twice.
    ///
    /// Two halves: the run FAILS (not converges), and the failure's `instead` is `None` — the arm
    /// of the sentence that says *this build cannot say what the pane was running*, which is a
    /// different message from naming a program and is otherwise built by nothing.
    #[test]
    fn a_host_that_cannot_see_the_process_table_is_never_ready_to_run_a_program() {
        /// A pane that exists, shows nothing, and whose host has NO process view — every
        /// capability left at its default, which is the point.
        struct NoProcessView;
        impl PaneAccess for NoProcessView {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[crate::access::KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                panic!("⚠⚠ NOT ONE BYTE may be injected into a pane this host cannot vouch for");
            }
        }

        let mut ready = Readiness::new(
            Some(ReadyWhen::Runs("claude".to_string())),
            Some(Duration::from_millis(120)),
            None,
        );
        let failed = ready
            .reached(&NoProcessView, PaneId(1), &RunContext::uncancellable())
            .expect_err("a host that cannot see the process table can never confirm the program");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Unknown,
            },
            "and it says so by having NO answer for what ran instead, rather than inventing one",
        );
        // ⚠ THE SIBLING CAPABILITY, SAME RULE. `supervision()` is `None` on this double too, and a
        // host that cannot supervise must not conclude that an agent has settled — the safe
        // direction is the same one, and it is the arm a second capability makes easy to forget.
        assert!(
            Readiness::new(
                Some(ReadyWhen::Settles("claude".to_string())),
                Some(Duration::from_millis(120)),
                None
            )
            .reached(&NoProcessView, PaneId(1), &RunContext::uncancellable())
            .is_err(),
            "a host with no detector cannot say an agent is at rest, so it must not type at one",
        );
        assert!(
            !failed.to_string().contains("instead"),
            "so the sentence simply omits that clause rather than reading `belonged to None`: {}",
            failed,
        );
    }

    // ── THE ANSWERING CONTRACT ────────────────────────────────────────────────────────────────
    //
    // ⚠⚠⚠ EVERY GATE BELOW DRIVES A REAL PSEUDOTERMINAL RUNNING A REAL MENU, and the observation
    // the barrier reads is derived by the SHIPPING parser (`sprag_detect::question`) from that
    // pane's actual screen — the same derivation the daemon's own `agent_state_source` makes. A
    // double reporting a hand-built `Question` would have asserted my belief about dialogs; this
    // asserts what the product does to one.
    //
    // ⚠⚠ AND THE PEER SAYS WHICH KEY IT ACTED ON. Every claim here is about which keystrokes a run
    // sends, so a fixture that only reported the OUTCOME would pass for a run that typed a digit
    // it did not need — the exact over-typing this contract is about. The peer prints
    // `TOOK <option> VIA <byte> AFTER <the bytes it ignored>` when it acts, `SAW <byte>` for a key
    // it ignores, and `EXTRA <byte>` for anything that arrives after it is done. The pane is the
    // witness.
    //
    // ⚠ `AFTER` exists because ACTING CLEARS THE SCREEN, so the `SAW` lines that led up to it are
    // wiped by the redraw — and the ORDER two keys went in is exactly what an escalation gate has
    // to assert. Carried in a variable, it survives the clear.

    /// A peer that draws a bottom-anchored numbered menu and reacts to single keystrokes, in one of
    /// the four dialog behaviours a run has to survive.
    ///
    /// * `numbers` — the DIGIT selects outright and Enter does nothing. The measured behaviour of
    ///   the agents this reads, and the one where a reflexive trailing Enter lands on whatever the
    ///   peer shows next.
    /// * `marker` — the digit only MOVES the highlight; Enter commits whatever it is on. The
    ///   behaviour where an Enter is required, and may only be sent once the marker can be SEEN on
    ///   the authorised option.
    /// * `either` — both, which is what a real agent's permission dialog does. The kind that makes
    ///   *"do not type a key you do not need"* a claim with consequences.
    /// * `deaf` — nothing works. The peer that makes [`Refusal::NotTaken`] reachable.
    fn menu_peer(kind: &str) -> String {
        format!(
            r#"
stty -icanon -echo 2>/dev/null
kind={kind}
sel=1
seen=''
readbyte() {{ dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \n'; }}
draw() {{
  printf '\033[2J\033[H'
  printf 'Bash command\r\n'
  printf 'Do you want to proceed?\r\n'
  i=1
  for label in 'Yes' 'Yes, and do not ask again' 'No, and tell me what to do'; do
    if [ "$i" = "$sel" ]; then printf '\342\235\257 '; else printf '  '; fi
    printf '%s. %s\r\n' "$i" "$label"
    i=$((i+1))
  done
}}
took() {{
  printf '\033[2J\033[H'
  printf 'TOOK %s VIA %s AFTER%s\r\n' "$sel" "$1" "$seen"
  while :; do
    e=$(readbyte)
    [ -n "$e" ] || exit 0
    printf 'EXTRA %s\r\n' "$e"
  done
}}
draw
while :; do
  k=$(readbyte)
  [ -n "$k" ] || exit 0
  case "$k" in
    49|50|51)
      case "$kind" in
        numbers|either) sel=$((k-48)); took "$k" ;;
        marker) sel=$((k-48)); draw ;;
        *) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
      esac ;;
    13|10)
      case "$kind" in
        marker|either) took "$k" ;;
        *) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
      esac ;;
    *) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
  esac
done
"#
        )
    }

    /// The byte the peer reports for Enter, so a gate names a KEY and not a number nobody can read.
    /// `VIA 10` is Enter; `VIA 50` is the digit `2`.
    ///
    /// ⚠ TEN, not thirteen — [`KeyStroke::named("Enter")`](crate::access::KeyStroke::named) encodes
    /// LF and not CR, which the fixture MEASURED rather than assumed (the first draft of these
    /// gates asserted `13` and the pane said `10`). The peer accepts both, so the gates are about
    /// which key the RUN chose to send and not about which byte a terminal calls Enter.
    const ENTER_BYTE: &str = "10";

    /// A pane running [`menu_peer`], wrapped in a pane-access whose SUPERVISOR is derived from that
    /// pane's own screen by the shipping choice-list parser.
    ///
    /// ⚠ `Blocked` exactly when the screen carries a menu. That is the daemon's own rule for the
    /// `asking` field (`agent_state_source`), reproduced here rather than mocked, so a gate cannot
    /// pass against a question the product would not have parsed.
    fn asking_peer(kind: &str) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((60, 12))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(menu_peer(kind));
            command.env("TERM", "dumb");
            workspace
                .lock()
                .expect("the workspace mutex")
                .spawn(command, "peer".to_string(), 60, 12)
                .expect("spawn the peer")
        };
        // ⚠⚠⚠ IT SETTLES, like the real one. A supervisor publishes a resting verdict only once a
        // candidate has held for its window, so a pane whose dialog has just been answered goes on
        // reading `Blocked` with NOTHING readable on it for that long. A source derived straight
        // from the screen has no such lag — and its absence hid a live defect from every gate in
        // this file until an end-to-end run through a real daemon met it: the step after a
        // successful answer read the stale verdict as a fresh one and reported that a person was
        // needed. **A double that cannot be wrong in the way the real thing is wrong is a double
        // that asserts your belief.**
        //
        // ⚠ Far shorter than `sprag_detect::DEFAULT_SETTLE`, because what is under test is that the
        // product tolerates a lag AT ALL rather than any particular length of one — and a gate that
        // paid two seconds per answer would be bought with wall-clock nobody gets back.
        const FIXTURE_SETTLE: Duration = Duration::from_millis(300);
        let source = {
            let workspace = Arc::clone(&workspace);
            let last_menu: Mutex<Option<std::time::Instant>> = Mutex::new(None);
            Arc::new(move |id: PaneId| {
                let guard = workspace.lock().expect("the workspace mutex");
                guard.pane(id)?.pty().with_screen(|screen| {
                    let asking = sprag_detect::question(screen, sprag_detect::DIALOG_WINDOW);
                    let mut seen = last_menu.lock().expect("the settle mutex");
                    if asking.is_some() {
                        *seen = Some(std::time::Instant::now());
                    }
                    let settling = seen.is_some_and(|at| at.elapsed() < FIXTURE_SETTLE);
                    Some(crate::access::AgentObservation {
                        state: if asking.is_some() || settling {
                            AgentState::Blocked
                        } else {
                            AgentState::Idle
                        },
                        agent: Some("claude".to_string()),
                        authority: crate::access::Authority::Scraped {
                            rule: Some("dialog-choice-list".to_string()),
                        },
                        seq: 1,
                        asking,
                    })
                })
            }) as crate::access::AgentStateSource
        };
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));
        // The menu must be UP before anything asks the barrier, or the gate is about a pane that
        // was never blocked.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10)
            && peer_asking(&access, pane).flatten().is_none()
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            peer_asking(&access, pane).flatten().is_some(),
            "the fixture's peer must be showing a menu the shipping parser reads, or this gate is \
             about nothing: {:?}",
            access.pane_collapsed(pane),
        );
        (access, pane)
    }

    /// Wait (bounded) for `pane` to show `needle`, then hand back the whole collapsed screen —
    /// which is what every assertion below reads, including the ones about what is NOT there.
    fn screen_showing(access: &WorkspacePaneAccess, pane: PaneId, needle: &str) -> String {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(needle))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        access.pane_collapsed(pane).unwrap_or_default()
    }

    /// A barrier with no readiness condition and the given consent — the shape every gate below
    /// wants, since what is under test is the ANSWERING contract and not the starting one.
    fn answering(consent: Option<Consent>) -> Readiness {
        Readiness::new(None, Some(Duration::from_millis(200)), consent)
    }

    /// A consent for the measured permission question, authorising the option carrying `answer`.
    fn consent_to(answer: &str) -> Consent {
        Consent::parse("Do you want to proceed?".to_string(), answer.to_string())
            .expect("two needles")
    }

    /// ⚠⚠⚠ **A RUN GIVEN NO CONSENT STILL TYPES NOTHING** — the behaviour R365 shipped, held here
    /// against the round that made answering possible at all.
    ///
    /// This is the arm that must not move. Everything else in this section is about how a run
    /// answers; this one is about the DEFAULT, and the default is the product's whole position on
    /// clicking approvals nobody read.
    ///
    /// ⚠ And the run must SAY WHY. `no_consent` is what tells a reader the run behaved as
    /// configured rather than that its consent failed to fire — the two look identical without it.
    #[test]
    fn a_run_with_no_consent_does_not_type_at_a_peer_that_is_asking() {
        let (access, pane) = asking_peer("either");
        let refused = answering(None)
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a blocked peer is not an error");
        let Reached::Asking(unanswered) = refused else {
            panic!("⚠⚠⚠ a peer showing a menu must never be reported ready: {refused:?}");
        };
        assert_eq!(
            unanswered.why(),
            Refusal::NoConsent,
            "the run was configured to answer nothing and it must say so — a reader who cannot \
             tell `I gave no consent` from `my consent did not fire` cannot fix either",
        );
        assert_eq!(unanswered.bytes(), 0, "and nothing was typed at the peer");
        let question = unanswered.question().expect("the question was read");
        assert_eq!(question.choices.len(), 3);
        assert_eq!(
            question.selected().map(|c| c.number),
            Some(1),
            "and the report says where a bare Enter would land, which is what a person answering \
             this by hand needs to know",
        );
        std::thread::sleep(Duration::from_millis(80));
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("SAW") && !screen.contains("TOOK"),
            "⚠⚠⚠ NOT ONE KEY, and the pane is the witness: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A KEY THAT IS NOT NEEDED IS NOT SENT.**
    ///
    /// The commonest real case: the agent's own marker is already on the option the caller
    /// authorised. A bare Enter takes THAT option and can take no other
    /// ([`Question::selected`]), so there is nothing for a digit to do — and a digit sent anyway is
    /// a second act nobody authorised, which against a dialog whose numbers submit outright is a
    /// second submission.
    ///
    /// The peer accepts BOTH keys and reports which one moved it, so `VIA 13` is the whole claim:
    /// the run pressed Enter and never typed a number.
    ///
    /// ⚠ REVERT-PROOF: make the [`Taken::Selected`] arm type the number first and the peer reports
    /// `VIA 49`.
    #[test]
    fn an_option_the_peer_is_already_standing_on_is_taken_without_typing_its_number() {
        let (access, pane) = asking_peer("either");
        let reached = answering(Some(consent_to("Yes")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the answer is not an error");
        let Reached::Answered(answered) = reached else {
            panic!(
                "`Yes` is option 1's whole label, so exactly one option carries it: {reached:?}"
            );
        };
        assert_eq!(answered.chose.number, 1);
        assert_eq!(answered.how, Taken::Selected);
        assert_eq!(answered.bytes, 1, "one keystroke, and it is the Enter");
        let screen = screen_showing(&access, pane, "TOOK");
        assert!(
            screen.contains(&format!("TOOK 1 VIA {ENTER_BYTE}")),
            "⚠⚠⚠ the peer must report being moved by the ENTER — a run that typed the number \
             first would show `VIA 49`, which is a keystroke nobody needed sent into a dialog \
             whose numbers submit: {screen:?}",
        );
        assert!(
            !screen.contains("EXTRA"),
            "and nothing followed it: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE NUMBER IS THE WHOLE ANSWER, AND NO ENTER FOLLOWS IT.**
    ///
    /// The peer here selects on the digit and ignores Enter, which is what the measured agents do.
    /// A run that reflexively sent `number + Enter` would put a stray Enter into whatever the peer
    /// showed NEXT — and what an agent shows after a tool approval is frequently ANOTHER dialog,
    /// where that Enter confirms the highlighted option.
    ///
    /// The consent names option 2, so the marker (on 1) is NOT already where it needs to be and the
    /// digit is genuinely required. The peer prints `EXTRA <byte>` for anything arriving after it
    /// is done, so the absence of one is the assertion.
    ///
    /// ⚠ REVERT-PROOF: append an Enter to the [`Taken::Numbered`] arm and the peer reports
    /// `EXTRA 13`.
    #[test]
    fn a_peer_that_takes_the_number_is_never_sent_an_enter_after_it() {
        let (access, pane) = asking_peer("numbers");
        let reached = answering(Some(consent_to("do not ask again")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the answer is not an error");
        let Reached::Answered(answered) = reached else {
            panic!("one option carries the authorised words: {reached:?}");
        };
        assert_eq!(answered.chose.number, 2);
        assert_eq!(
            answered.how,
            Taken::Numbered,
            "the peer left the question on the digit alone",
        );
        assert_eq!(answered.bytes, 1, "one keystroke: the digit");
        let screen = screen_showing(&access, pane, "TOOK");
        assert!(
            screen.contains("TOOK 2 VIA 50"),
            "the peer took the option the consent authorised, moved by its number: {screen:?}",
        );
        assert!(
            !screen.contains("EXTRA"),
            "⚠⚠⚠ and NOTHING followed it. An Enter sent here goes to whatever the peer shows \
             next, which for an agent is often a second dialog: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **AN ENTER IS SENT ONLY ONCE THE PEER'S OWN MARKER IS ON THE AUTHORISED OPTION.**
    ///
    /// The other dialog behaviour: the digit moves the highlight and Enter commits it. Here the
    /// Enter is REQUIRED, and it is safe for a reason the run can check rather than assume — the
    /// marker having arrived on the option is simultaneously the proof the peer read the digit and
    /// the guarantee of where the Enter will land.
    ///
    /// The consent names option 3, deliberately: the marker starts on 1, so a run that pressed
    /// Enter before checking would take `Yes` when the caller authorised `No`. The peer prints
    /// which option it took, so this gate can tell those apart.
    #[test]
    fn a_peer_whose_marker_must_move_first_is_confirmed_only_after_it_has() {
        let (access, pane) = asking_peer("marker");
        let reached = answering(Some(consent_to("No, and tell me")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the answer is not an error");
        let Reached::Answered(answered) = reached else {
            panic!("one option carries the authorised words: {reached:?}");
        };
        assert_eq!(answered.chose.number, 3);
        assert_eq!(answered.how, Taken::NumberedThenConfirmed);
        assert_eq!(answered.bytes, 2, "the digit and the Enter");
        let screen = screen_showing(&access, pane, "TOOK");
        assert!(
            screen.contains(&format!("TOOK 3 VIA {ENTER_BYTE}")),
            "⚠⚠⚠ the peer must have taken the option the CONSENT named, not the one its marker \
             happened to start on — an Enter sent before the marker moved reads `TOOK 1`, which is \
             `Yes` to a caller who authorised `No`: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PEER THAT IGNORES THE NUMBER IS NEVER SENT THE ENTER** — and the run reports it
    /// rather than typing again.
    ///
    /// The deaf peer's marker never moves, so the confirming Enter is never justified. Three
    /// things must follow, and the middle one is the sharpest: the run stops with
    /// [`Refusal::NotTaken`] rather than escalating; **no Enter is sent**, because an Enter here
    /// would commit option 1 when the caller authorised option 2; and the step charges for the key
    /// it did type, or a run under a cost ceiling under-reports its own spend.
    ///
    /// ⚠ REVERT-PROOF: send the Enter unconditionally after the number and the peer reports
    /// `SAW 13`.
    #[test]
    fn an_answer_the_peer_ignores_is_reported_rather_than_confirmed_anyway() {
        let (access, pane) = asking_peer("deaf");
        let reached = answering(Some(consent_to("do not ask again")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a peer that ignores a key is not an error");
        let Reached::Asking(unanswered) = reached else {
            panic!("the peer never left the question, so nothing was answered: {reached:?}");
        };
        assert_eq!(unanswered.why(), Refusal::NotTaken);
        assert_eq!(
            unanswered.bytes(),
            1,
            "⚠ the digit WAS typed, and a refusal that charged nothing for it would under-report \
             the run's spend against the caller's ceiling",
        );
        assert!(
            unanswered.question().is_some(),
            "and the question is still what a person has to answer",
        );
        let screen = screen_showing(&access, pane, "SAW");
        assert!(
            screen.contains("SAW 50"),
            "the digit reached the peer, which is what makes this the NotTaken case rather than a \
             refusal: {screen:?}",
        );
        assert!(
            !screen.contains(&format!("SAW {ENTER_BYTE}")),
            "⚠⚠⚠ and the Enter was NOT sent. The marker never moved onto option 2, so an Enter \
             here would have committed option 1 — the caller authorised the other one: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PEER THAT IGNORES THE ENTER ITS OWN MARKER JUSTIFIED IS STILL ANSWERED** — the arm an
    /// end-to-end run through a real daemon had to find, because every unit gate here supplied the
    /// other side of the conversation.
    ///
    /// The commonest consent by far names the option the agent has already highlighted (`Yes`, on a
    /// permission dialog). Against the `numbers` peer — number hotkeys, no Enter handling, which is
    /// a real menu shape — the run pressed the one key that dialog does not read and reported
    /// `not_taken`. The measurement came from `cli.rs`'s live-daemon gate; this is the same finding
    /// reduced to a fixture that runs in milliseconds.
    ///
    /// Both halves are asserted. The Enter goes FIRST, because it is the key whose landing place
    /// the peer's own marker proves; the number follows ONLY because the same question is still up
    /// with the marker still on the authorised option.
    ///
    /// ⚠ REVERT-PROOF: drop the `SelectedThenNumbered` escalation and this reads `not_taken`.
    #[test]
    fn a_peer_that_ignores_the_enter_gets_the_number_it_does_read() {
        let (access, pane) = asking_peer("numbers");
        let reached = answering(Some(consent_to("Yes")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the answer is not an error");
        let Reached::Answered(answered) = reached else {
            panic!(
                "⚠⚠⚠ the peer offers the option the consent named and reads a key that takes it — \
                 a run that gave up here cannot answer the commonest dialog there is: {reached:?}",
            );
        };
        assert_eq!(answered.chose.number, 1);
        assert_eq!(
            answered.how,
            Taken::SelectedThenNumbered,
            "the Enter went first and the peer ignored it; the number followed",
        );
        assert_eq!(answered.bytes, 2, "the Enter and then the number");
        let screen = screen_showing(&access, pane, "TOOK");
        assert!(
            screen.contains("TOOK 1 VIA 49"),
            "and the peer took the authorised option, moved by the NUMBER — `VIA 10` here would \
             mean this fixture accepts Enter and proves nothing: {screen:?}",
        );
        assert!(
            screen.contains(&format!("AFTER {ENTER_BYTE}")),
            "⚠⚠ and the ORDER: the peer received the Enter FIRST and did nothing with it, which is \
             what makes the second key justified rather than a guess. A run that reached for the \
             number first would read `AFTER` with nothing behind it: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AND THE ESCALATION DOES NOT FIRE WHERE THE FIRST KEY WORKED.** The `either` peer takes
    /// the Enter, so the number must never be typed — otherwise every ordinary answer would put a
    /// stray digit into whatever the peer showed next, which is the hazard the whole `Taken`
    /// vocabulary exists to avoid.
    ///
    /// The control for the gate above: without it, an escalation that fired unconditionally would
    /// pass there and be a defect everywhere else.
    #[test]
    fn a_peer_that_takes_the_enter_is_never_sent_the_number_as_well() {
        let (access, pane) = asking_peer("either");
        let reached = answering(Some(consent_to("Yes")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the answer is not an error");
        let Reached::Answered(answered) = reached else {
            panic!("the peer takes the Enter its marker justifies: {reached:?}");
        };
        assert_eq!(answered.how, Taken::Selected);
        assert_eq!(answered.bytes, 1, "one key, and it is the Enter");
        let screen = screen_showing(&access, pane, "TOOK");
        assert!(
            !screen.contains("EXTRA"),
            "⚠⚠ NOTHING followed the Enter that worked: {screen:?}",
        );
    }

    /// ⚠⚠⚠ **A CONSENT THAT DOES NOT NAME EXACTLY ONE OPTION TYPES NOTHING**, and each way of not
    /// naming one is reported as itself.
    ///
    /// Driven against the SAME live dialog, so the three answers differ only by what the caller
    /// consented to. The ambiguity arm is the one that matters most: `"and"` sits on the two
    /// options that mean opposite things — grant a standing permission, or refuse — and a
    /// first-match policy would take one of them.
    #[test]
    fn a_consent_that_names_no_single_option_leaves_the_dialog_untouched() {
        for (asked, answer, expected) in [
            ("Do you want to proceed?", "and", Refusal::Ambiguous),
            ("Do you want to proceed?", "Maybe", Refusal::NotOffered),
            ("delete the database?", "Yes", Refusal::OtherQuestion),
        ] {
            let (access, pane) = asking_peer("either");
            let consent =
                Consent::parse(asked.to_string(), answer.to_string()).expect("two needles");
            let reached = answering(Some(consent))
                .reached(&access, pane, &RunContext::uncancellable())
                .expect("a refusal is not an error");
            let Reached::Asking(unanswered) = reached else {
                panic!("{asked:?}/{answer:?} must not have answered anything: {reached:?}");
            };
            assert_eq!(unanswered.why(), expected, "for {asked:?} / {answer:?}");
            assert_eq!(unanswered.bytes(), 0, "for {asked:?} / {answer:?}");
            std::thread::sleep(Duration::from_millis(80));
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            assert!(
                !screen.contains("TOOK") && !screen.contains("SAW"),
                "⚠⚠⚠ NOT ONE KEY may reach a dialog no consent covers, and the peer says whether \
                 one did ({asked:?} / {answer:?}): {screen:?}",
            );
            access.lifecycle().expect("lifecycle").close(pane);
        }
    }

    /// ⚠⚠ **A BLOCKED PANE WHOSE QUESTION THIS HOST CANNOT READ NOW SAYS WHAT TO DO ABOUT IT.**
    ///
    /// `asking: None` on a blocked pane was published as an ABSENCE and explained nowhere: the
    /// remedy — hand the pane to a person — lived in a doc comment and no surface said it. An agent
    /// can block on something that is not a numbered list, and no consent can name an option a
    /// screen does not offer, so this is a real state and not a gap.
    #[test]
    fn a_blocked_pane_with_no_readable_question_is_handed_to_a_person() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .expect("the workspace mutex")
                .spawn(command, "peer".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let source = Arc::new(|_id: PaneId| {
            Some(crate::access::AgentObservation {
                state: AgentState::Blocked,
                agent: Some("claude".to_string()),
                authority: crate::access::Authority::Reported {
                    source: "hook".to_string(),
                },
                seq: 3,
                asking: None,
            })
        }) as crate::access::AgentStateSource;
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));

        // ⚠ Even WITH a consent — the consent is not the thing missing, and a run that reported
        // `no_consent` here would send its caller to fix an argument that would not have helped.
        let reached = answering(Some(consent_to("Yes")))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a blocked peer is not an error");
        let Reached::Asking(unanswered) = reached else {
            panic!("a blocked pane must never be reported ready: {reached:?}");
        };
        assert_eq!(unanswered.why(), Refusal::Unreadable);
        assert!(unanswered.question().is_none());
        assert!(
            unanswered.why().describe().contains("person"),
            "and the remedy is a PERSON, said out loud rather than left in a doc comment: {}",
            unanswered.why().describe(),
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AN ANSWERED PEER IS NOT A READY PANE**, and the barrier says so by ending the step.
    ///
    /// The step that answers must not go on to type its stimulus: the peer has just been handed a
    /// decision and is acting on it. This gate takes the barrier round TWICE against the same live
    /// pane — the first call answers, the second finds the peer no longer asking and clears — which
    /// is the sequence a driven loop actually makes.
    ///
    /// ⚠ It also holds the LATCH honest: both answers are computed BEFORE the readiness latch, so a
    /// run that has already cleared its barrier still cannot type into a dialog that opened later.
    #[test]
    fn an_answer_ends_the_step_and_the_next_one_finds_the_pane_ready() {
        let (access, pane) = asking_peer("either");
        let mut barrier = answering(Some(consent_to("Yes")));
        assert!(
            matches!(
                barrier.reached(&access, pane, &RunContext::uncancellable()),
                Ok(Reached::Answered(_)),
            ),
            "the first step spends itself on the answer",
        );
        let start = std::time::Instant::now();
        let mut second = barrier.reached(&access, pane, &RunContext::uncancellable());
        while start.elapsed() < Duration::from_secs(5) && second != Ok(Reached::Yes) {
            std::thread::sleep(Duration::from_millis(20));
            second = barrier.reached(&access, pane, &RunContext::uncancellable());
        }
        assert_eq!(
            second,
            Ok(Reached::Yes),
            "and the NEXT one drives the pane the answer freed — an answer that left the barrier \
             shut would be a loop that stops on every dialog it is allowed to answer",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }
}
