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
//! the caller's [`Consent`](crate::consent::Consent) lives here too, and the reason is the reason
//! this module exists at all: **this is the one place all three injecting plugins pass through on
//! their way to a keystroke.** A second door to a blocked pane — a plugin answering a dialog on its
//! own — would be two readers of one question, which is the shape R344 spent a round on and R365
//! found again. There is one, and what it may type is what the caller wrote down.

use std::time::{Duration, Instant};

use sprag_detect::{AgentState, Choice, Question};
use sprag_terminal::PaneId;

use crate::access::{JobLeader, KeyStroke, PaneAccess, PaneDoing, PaneError};
use crate::consent::{Answered, Consents, Refusal, Taken, Unanswered};
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
pub(crate) fn peer_asking(panes: &dyn PaneAccess, pane: PaneId) -> Option<Option<Question>> {
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

/// **HAS THE PEER MOVED OFF `question`** — the one condition that ends a wait for a person.
///
/// [`marker_arrived`]'s [`Arrival::LeftTheQuestion`] test with the authorised choice taken out of
/// it, because a run waiting for a HUMAN has no authorised choice: it is not watching for a marker
/// to land somewhere, only for the dialog it could not answer to stop being the one on the pane.
///
/// ⚠ The `None` question — a peer blocked on something this host cannot read — is the one case that
/// cannot be answered by comparing sentences, so it asks the weaker question it can answer: is this
/// pane still blocked at all. That is strictly more conservative. A person who replaces one
/// unreadable dialog with another keeps the run waiting, which is right, since nothing here can
/// tell the two apart and resuming would type into whichever it is.
fn moved_on(panes: &dyn PaneAccess, pane: PaneId, question: Option<&Question>) -> bool {
    match question {
        Some(asked) => left_the_question(panes, pane, asked),
        // ⚠ A peer blocked on something this host cannot read is the one case that cannot be
        // answered by comparing sentences, so it asks the weaker question it can answer: is this
        // pane still blocked at all.
        None => peer_asking(panes, pane).is_none(),
    }
}

/// **IS `question` OFF THE PANE** — [`Arrival::LeftTheQuestion`]'s test, with no authorised choice
/// in it.
///
/// # ⚠⚠⚠ THE QUESTION, NOT THE STATE, and this crate has paid for the difference
///
/// A supervisor's verdict SETTLES: a real detector goes on calling a pane blocked for its hysteresis
/// window after the dialog has left the screen. Asked as *"is the pane still blocked"*, a wait for a
/// peer to move outlived its own bound and a run that HAD been answered reported
/// [`Refusal::NotTaken`] — measured end to end through a real daemon, and invisible to every fixture
/// here because a fixture derives its state from the screen and so has no lag to model.
///
/// ⚠ A DIFFERENT question is also *gone*: the one being watched for is no longer up, and whatever
/// the peer asked next is the next step's business — where it meets the whole contract again rather
/// than a decision already in flight.
///
/// ⚠⚠ **THREE ACTS SHARE IT**, and that is why it is a function: the answering wait, the wait for a
/// person, and — since the round that built `screening` — the proof a refused dialog is really gone
/// before anything is typed at the pane. A live probe measured what happens when that last one is
/// skipped: text typed into a dialog that was still up **approved the file write it was asking
/// about**.
pub(crate) fn left_the_question(panes: &dyn PaneAccess, pane: PaneId, question: &Question) -> bool {
    // ⚠⚠ FLATTENED, and the two `None`s it merges are both this question being over. Not blocked
    // at all is obvious; blocked with NO READABLE MENU is the settle window — a detector goes on
    // calling a pane blocked after the dialog left its screen — and it is also a peer that moved on
    // to something this host cannot parse.
    peer_asking(panes, pane)
        .flatten()
        .is_none_or(|now| now.asked != question.asked)
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

/// **WHETHER ANYBODY IS EXPECTED TO COME** when the peer asks something this run may not answer —
/// the run's fourth declared-in-advance contract, beside [`ReadyWhen`],
/// [`DoneWhen`](crate::completion::DoneWhen) and [`Consents`].
///
/// # ⚠⚠⚠ Why a blocked run had exactly one behaviour, and why that was wrong for half its callers
///
/// Every refusal this contract can reach leaves a QUESTION on a pane with nothing this run may do
/// about it, and most of them say so outright: **hand the pane to a person**
/// ([`Refusal::describe`]). Until this existed the run acted on that by STOPPING, which is the only
/// honest thing to do when the pane is on a screen nobody is looking at — and the wrong thing when
/// somebody is looking at it right now.
///
/// The two cases are not degrees of one another, and no run could tell them apart:
///
/// * **An unattended run** — a nightly sweep, an agent driving a peer in a detached session. A
///   question is the end of it, because the alternative is a machine deciding something nobody
///   authorised. This stays the default and the behaviour is unchanged.
/// * **A supervised run** — the inner session of a loop a person is watching. The pane is on their
///   screen, they read every turn as it happens, and they can answer the dialog with their own
///   hands. Here stopping throws away a turn for no reason but the absence of a way to say *"wait
///   for me"*. **Measured before this existed: a run whose supervisor answered the dialog a moment
///   later had already reported `blocked` — in forty milliseconds.**
///
/// # ⚠⚠ Why waiting is not the same as answering, and does not weaken anything
///
/// A run that waits still types nothing. [`Consents`] remains the only thing that can put a byte
/// into a dialog, and the wait ends when the PERSON has moved the peer off the question. So this
/// widens what a run can WAIT for and not one thing that it may DECIDE — which is the distinction
/// the whole answering contract is built on, kept intact at the one door all four injecting
/// plugins pass through.
///
/// # ⚠ Why a DURATION rather than a flag
///
/// A bare *"somebody is watching"* would need a patience from somewhere, and the only somewhere is
/// a default nobody chose — the shape this crate has removed twice. How long a person may take is
/// exactly the kind of question only the caller can answer (a supervisor at the keyboard is
/// seconds; one who checks between meetings is an hour), so they say it, and the absence of the
/// argument is the absence of the person.
///
/// [`Consents`]: crate::consent::Consents
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attended {
    /// **NOBODY IS WATCHING.** A question this run cannot answer ends it — the behaviour every run
    /// had before this contract existed, and the default.
    NoOne,
    /// A person is at this pane, with `patience` to spare for a question this run may not answer.
    /// `handback` says what becomes of a pane they TAKE — see [`Handback`].
    APerson {
        /// How long to wait for them to deal with a dialog, before carrying on from wherever they
        /// left the peer.
        patience: Duration,
        /// Whether a pane this person takes for themselves ever becomes this run's again.
        handback: Handback,
    },
}

/// **WHEN IS A PANE A PERSON TOOK THIS RUN'S AGAIN?** — the other half of
/// [`Reached::Interrupted`], and the half `ai_loop.scxml` has always asked for.
///
/// # ⚠⚠⚠ Why this lives INSIDE [`Attended::APerson`] rather than beside it
///
/// A handback is only meaningful where somebody is expected: it is the question *"the person you
/// told me to wait for has taken the pane — do I wait for that too?"*. Declared as its own
/// top-level argument it would have a value nobody could act on whenever
/// [`Attended::NoOne`] was chosen, and a caller who arrived at that pair by arithmetic — a config
/// that set one and not the other — would be silently given a run that ends on the first keystroke
/// while their request plainly asked otherwise. That is the shape [`Attended::of`] already refuses
/// one level down for a patience of zero. Nested, the combination cannot be spelt.
///
/// # ⚠⚠⚠ Why a STILLNESS and not a resume signal
///
/// `ai_loop.scxml`'s `awaiting_human` leaves for `working` on an event — *"the person looked and
/// waved it on"*. An SCXML document may name an event and leave who raises it to the driver; the
/// driver has to answer it. The only thing this product can HONESTLY see about a person is what
/// [`sprag_terminal::Hands`] counts: writes at the pane's own door, whose hand each came from. So
/// the question a run can actually ask is *has their hand been still long enough*, and how long
/// that is nobody but the caller knows — a supervisor answering one dialog is a second, one editing
/// a file by hand is a minute. **R372 refused to invent this number and it was right to; this makes
/// the caller say it**, which is [`Attended`]'s own reasoning about patience, one door over.
///
/// ⚠ The stillness is measured from the last write this barrier OBSERVED, not from the keystroke
/// itself — a poll cannot see between its own looks. That rounds the wait UP, never down, so a run
/// resumes later than a person let go and never earlier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handback {
    /// **THE PANE IS THEIRS NOW.** The run reports [`Verdict::TakenOver`] and ends, which is what
    /// every run did before this existed and is still the right answer for most of them: a person
    /// who reaches into a pane a machine is driving has usually done so to STOP it.
    ///
    /// [`Verdict::TakenOver`]: crate::plugin::Verdict::TakenOver
    Never,
    /// **WAIT FOR THEM TO FINISH.** The pane is this run's again once the person's hand has been
    /// still for this long, and the run carries on from wherever they left the peer.
    ///
    /// ⚠ Bounded by the [`patience`](Attended::APerson::patience) beside it: a pane still theirs
    /// when that runs out ends the run exactly as [`Never`](Self::Never) would have, with the same
    /// word and the whole episode's writes counted. The bound is deliberately the SAME number the
    /// caller already gave for a dialog, because `ai_loop.scxml` gives `awaiting_human` ONE
    /// `unattended` exit no matter which door the run came in by.
    WhenStill(Duration),
}

impl Handback {
    /// The argument this is declared with, in ONE place — [`Attended::WIRE_KEY`]'s rule.
    ///
    /// ⚠ It names the ACT and its UNIT, like its neighbour: *"the pane is mine again once their
    /// hand has been still this many milliseconds"*.
    pub const WIRE_KEY: &'static str = "handback_still_ms";

    /// A handback after `still`, or [`None`] for a stillness of zero.
    ///
    /// ⚠ Zero is REFUSED for [`Attended::of`]'s reason exactly: *"the pane is mine again the
    /// instant they pause"* is not a thing a caller can mean — every person pauses between
    /// keystrokes — and a caller who reached zero by arithmetic needs telling rather than a run
    /// that types into the gap between their words.
    #[must_use]
    pub const fn of(still: Duration) -> Option<Self> {
        if still.is_zero() {
            return None;
        }
        Some(Self::WhenStill(still))
    }

    /// How long a still hand means they are done, or [`None`] when the pane stays theirs.
    #[must_use]
    pub const fn stillness(self) -> Option<Duration> {
        match self {
            Self::Never => None,
            Self::WhenStill(still) => Some(still),
        }
    }
}

impl Attended {
    /// The argument this contract is declared with, in ONE place, so the daemon's parser, the
    /// published grammar and both mouths spell it identically — [`Consents::WIRE_KEY`]'s rule.
    ///
    /// ⚠ It names the ACT and its UNIT rather than the state, because the value IS the patience:
    /// *"await a person for this many milliseconds"*. A `attended: true` would have needed a
    /// patience from somewhere, and the only somewhere is a default nobody chose.
    ///
    /// [`Consents::WIRE_KEY`]: crate::consent::Consents::WIRE_KEY
    pub const WIRE_KEY: &'static str = "await_person_ms";

    /// A run watched by somebody with `patience` to spare, who gets the pane back on `handback`'s
    /// terms — or [`None`] for a patience of zero.
    ///
    /// ⚠ Zero is REFUSED rather than accepted as a no-op, for [`Consents::of`]'s reason one level
    /// up: *"wait for a person for no time at all"* and *"nobody is watching"* would be two
    /// spellings of one behaviour, and the caller who arrives at the first by arithmetic — a
    /// deadline already passed, a config that defaulted to 0 — is precisely the one who needs to be
    /// told rather than silently given the other.
    ///
    /// ⚠⚠ BOTH FACTS AT ONCE, and deliberately: a watching person cannot be declared without
    /// deciding what becomes of a pane they take. The alternative was a second constructor with a
    /// default, and the default would have been *"the run ends"* for every caller who never read
    /// this far — which is the answer `ai_loop.scxml` spent a whole state saying is wrong for a
    /// supervised loop.
    ///
    /// [`Consents::of`]: crate::consent::Consents::of
    #[must_use]
    pub const fn of(patience: Duration, handback: Handback) -> Option<Self> {
        if patience.is_zero() {
            return None;
        }
        Some(Self::APerson { patience, handback })
    }

    /// How long a person is given, or [`None`] when nobody is expected.
    #[must_use]
    pub const fn patience(self) -> Option<Duration> {
        match self {
            Self::NoOne => None,
            Self::APerson { patience, .. } => Some(patience),
        }
    }

    /// What becomes of a pane this run's person takes — [`Handback::Never`] when nobody is
    /// expected, because a run nobody is watching has nobody to give it back.
    #[must_use]
    pub const fn handback(self) -> Handback {
        match self {
            Self::NoOne => Handback::Never,
            Self::APerson { handback, .. } => handback,
        }
    }
}

/// **A PERSON DEALT WITH THE DIALOG THIS RUN COULD NOT** — what [`Reached::Attended`] carries.
///
/// It holds everything an unattended run would have ENDED with, because the facts are worth the
/// same words either way: a supervised run that needed a human four times is a run whose consents
/// are missing four clauses, and a journal that only said *"continued"* would hide that.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attention {
    /// The question, and why this run could not answer it itself.
    asked: Unanswered,
    /// How long the person took to come.
    waited: Duration,
}

impl Attention {
    /// The question that stopped the run, and the reason no clause covered it.
    #[must_use]
    pub const fn asked(&self) -> &Unanswered {
        &self.asked
    }

    /// How long the run waited before the person moved the peer off it.
    #[must_use]
    pub const fn waited(&self) -> Duration {
        self.waited
    }

    /// What this run had already spent on the question before it waited — non-zero only where a
    /// consent was typed and the peer went on asking ([`Refusal::NotTaken`]).
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.asked.bytes()
    }

    /// The line a run's journal carries for the turn a person took over.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "a person answered what this run could not ({}) after {:.1}s",
            self.asked.why().wire_str(),
            self.waited.as_secs_f32(),
        )
    }
}

/// **SOMEBODY REACHED INTO THIS PANE WITH THEIR OWN HANDS** — what [`Reached::Interrupted`]
/// carries.
///
/// # ⚠⚠⚠ Why this is not a kind of [`Attention`]
///
/// [`Attention`] is a person answering a question THE RUN STOPPED ON: the run asked for help and
/// help came, so the pane is handed back and the loop goes on. This is the opposite direction —
/// **nothing was asking**, and a person started typing into a pane a run was driving. The run was
/// not waiting for them and has no idea what they are doing.
///
/// `ai_loop.scxml` names them as two different events for that reason: a dialog sends the loop to
/// `screening`, and `turn.interrupted` goes straight to `awaiting_human` — *"they are plainly here
/// and doing something, so the loop stops driving; screening would be answering a question the
/// person is already handling"*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Interruption {
    /// How many times a person wrote into this pane after this barrier armed.
    writes: u64,
}

impl Interruption {
    /// An interruption of `writes` — for the gates that must construct one without a pane.
    ///
    /// ⚠ Not `pub(crate)`: the vocabulary gate in [`Verdict`](crate::plugin::Verdict) builds every
    /// variant it can spell, and a variant whose payload no caller outside this module can make is
    /// one that gate cannot walk.
    #[must_use]
    pub const fn of(writes: u64) -> Self {
        Self { writes }
    }

    /// How many separate times a person put input into this pane while the run was driving it.
    ///
    /// ⚠ WRITES, not keystrokes and not bytes. One write is one act at the door — a keystroke, an
    /// IME commit, a paste of eighty lines — and the pane counts acts because that is what it can
    /// honestly see. A report that said *"eighty keystrokes"* for one paste would be inventing
    /// detail the device never had.
    #[must_use]
    pub const fn writes(self) -> u64 {
        self.writes
    }

    /// The line a run's journal carries for the turn a person took the pane.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "a person typed into this pane {} time(s) while the run was driving it",
            self.writes,
        )
    }
}

/// **A PERSON TOOK THIS PANE AND GAVE IT BACK** — what [`Reached::HandedBack`] carries.
///
/// It holds what an ending run would have reported ([`Interruption`]) plus how long the pane was
/// theirs, for [`Attention`]'s reason: a loop that was interrupted six times is one whose prompts
/// keep asking for something its supervisor would rather do by hand, and a journal that only said
/// *"continued"* would hide the whole pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handover {
    /// What they did while they had it — the same fact a [`Reached::Interrupted`] run ends on,
    /// counted over the WHOLE episode rather than only the write that stopped the run.
    took: Interruption,
    /// How long the run waited between noticing them and getting the pane back.
    held: Duration,
}

impl Handover {
    /// What the person did while the pane was theirs.
    #[must_use]
    pub const fn took(&self) -> Interruption {
        self.took
    }

    /// How long this run waited for the pane to come back.
    #[must_use]
    pub const fn held(&self) -> Duration {
        self.held
    }

    /// The line a run's journal carries for the turn it spent waiting for a person to finish.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "a person had this pane for {:.1}s ({} write(s)) and their hand went still; it is this \
             run's again",
            self.held.as_secs_f32(),
            self.took.writes(),
        )
    }
}

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
    ///
    /// # ⚠⚠⚠ Why it carries the refusal it is NOT
    ///
    /// It carries the very [`PaneError::NeverReady`] the barrier's OWN bound would have refused
    /// with. The two endings stay different — one is *this pane never came up*, the other is *the
    /// run was out of time* — and which of them a caller gets depends on nothing they chose: it is
    /// whichever of `ready_within` and the run's clock happens to be shorter. So the ENDING may
    /// differ and the DIAGNOSIS may not, or a caller learns what they were waiting for only when
    /// the bounds fall one way round.
    ///
    /// ⚠⚠⚠ **AND THIS IS THIS WORKSPACE'S OWN LONGEST-RUNNING FLAKE, TOLD BACK BY THE PRODUCT.**
    /// A suite at thirty threads deschedules the gap between a fixture's spawn and the driver's
    /// first look for longer than a peer takes to print its banner; the marker is then in
    /// [`Prints`](ReadyWhen::Prints)' baseline, the barrier can never clear, and the run waits out
    /// its whole clock. What it reported was `Exhausted(Duration)` with `Bytes(0)` and not one word
    /// about a barrier — a signature that sat in `.claude/remote-build.toml` for rounds as a
    /// hypothesis nobody had instrumented. A run that says what it was still waiting for is the
    /// instrument, and it is the product's to say rather than a note in a file.
    RunEnded(PaneError),
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
    /// **THE PEER ASKED AND THE RUN ANSWERED IT**, on a [`Consent`](crate::consent::Consent) the caller declared in advance.
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
    /// **THE PEER ASKED SOMETHING THIS RUN MAY NOT ANSWER, AND THE PERSON WATCHING ANSWERED IT** —
    /// see [`Attended`].
    ///
    /// # ⚠⚠ Not `Yes`, for [`Answered`](Self::Answered)'s reason exactly
    ///
    /// The peer has just been handed a decision by a human and is acting on it, so a barrier that
    /// said `Yes` here would let the same step type its stimulus into a pane mid-transition. The
    /// wait ENDS the step; the next one asks this barrier again, by which time the peer is working,
    /// at rest, or asking something else — each of which this type already has an answer for.
    ///
    /// ⚠ That is also what makes a person's answer safe to resume from without inspecting it. This
    /// run never learns what they chose and has no business knowing: it asks the barrier again and
    /// meets whatever the pane became.
    Attended(Attention),
    /// **A PERSON HAS TAKEN THIS PANE** — they typed into it themselves while the run was driving,
    /// and the run stops rather than typing over them. See [`Interruption`].
    ///
    /// # ⚠⚠⚠ Why this outranks the question below it
    ///
    /// It is asked BEFORE *is the peer waiting on a dialog*, which inverts the order every other
    /// answer here follows. `ai_loop.scxml` is explicit about why: a person who is typing is
    /// already dealing with whatever is on that screen, so consulting a run's standing consents
    /// would be *"answering a question the person is already handling"* — and the run would type a
    /// stored reply into a dialog somebody was mid-way through answering by hand.
    ///
    /// ⚠ It is also the safer order under the one thing this cannot see. A pane whose person is
    /// typing is a pane whose screen is changing under the dialog parser, so the question read
    /// there is the least trustworthy reading this barrier ever takes.
    Interrupted(Interruption),
    /// **A PERSON TOOK THIS PANE AND HAS FINISHED WITH IT** — they went still for as long as the
    /// caller's [`Handback::WhenStill`] says means *done*, and the pane is this run's again.
    ///
    /// # ⚠⚠ Not `Yes`, for [`Attended`](Self::Attended)'s reason exactly
    ///
    /// The pane a person hands back is not the pane they took: they may have answered a dialog,
    /// started a different program, or left one of their own up. So the wait ENDS the step and the
    /// NEXT one meets this barrier again — where *is it asking*, *has it started* and *has somebody
    /// reached in* are all asked afresh against whatever the pane became. This run never learns what
    /// they did and has no business knowing.
    HandedBack(Handover),
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
///
/// # ⚠⚠⚠ NOT `Clone`, and that absence is a safety property
///
/// A barrier is three facts about ONE PANE — the latch, the marker baseline, the hands watermark —
/// and the only caller that ever wanted a second one is `ai_loop`'s `restarting`, which closes its
/// inner pane and opens a fresh one. A copy is precisely the WRONG value there: `seen` is latched, so
/// the replacement session reads as *already ready* and its first prompt goes into a program that has
/// existed for ten milliseconds. **That is R379's measured defect, and a mutation proved no stand-in
/// in this workspace catches it** — a `sh` peer's pseudoterminal buffers the early prompt and the run
/// converges either way, which is the same blindness that let the defect ship in the first place.
///
/// So the copy is not forbidden by a comment or caught by a gate: [`rearmed`](Self::rearmed) is the
/// only way to get a second barrier from an existing one, and it forgets what it must. Deriving
/// `Clone` here would make the dangerous version writable again, and nothing would fail.
#[derive(Debug)]
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
    ///
    /// ⚠⚠ A LIST since R370, because one turn asks more than one question — see [`Consents`],
    /// which owns what several clauses say about one dialog.
    consent: Option<Consents>,
    /// WHO IS EXPECTED TO COME when the consent above does not cover what the peer asked — see
    /// [`Attended`].
    ///
    /// ⚠ It lives HERE, beside the consent and for the same reason: this is the one door to a
    /// blocked pane, and *"may this run answer it"* and *"will somebody else"* are the two halves
    /// of one question. Split across two places, a plugin could be written that consults one of
    /// them.
    attended: Attended,
    /// How many times a PERSON had written into this pane when this barrier first looked — the
    /// watermark [`Reached::Interrupted`] is measured against. `None` until the first look, for
    /// [`armed_at`](Self::armed_at)'s reason exactly: it is the first moment a pane is in hand.
    ///
    /// ⚠ `None` ALSO after a first look at a host with no [`PaneHands`] capability, and the two are
    /// deliberately the same value: both mean *this run has no watermark to compare against*, and
    /// the consequence — carry on driving — is the same and is the safe one. See
    /// [`PaneAccess::hands`].
    ///
    /// [`PaneHands`]: crate::access::PaneHands
    /// [`PaneAccess::hands`]: crate::access::PaneAccess::hands
    hands_at: Option<u64>,
}

impl Readiness {
    /// A barrier for `when`, waiting `within` (defaulting to [`DEFAULT_READY_TIMEOUT`]), answering
    /// its peer's questions under `consent`, and waiting for whoever `attended` says is watching.
    ///
    /// A `None` condition is a barrier that is already down: the caller is saying the pane is
    /// running what they mean to drive. A `None` consent is a run that answers nothing, and
    /// [`Attended::NoOne`] is a run nobody will come to.
    ///
    /// ⚠ Both contracts are PARAMETERS and not builders, so a plugin that injects cannot be written
    /// without deciding what it does about a blocked peer — [`Plugin::driving`]'s reasoning, which
    /// is the other question in this crate whose harmless-looking default was a wrong answer.
    ///
    /// [`Plugin::driving`]: crate::plugin::Plugin::driving
    #[must_use]
    pub fn new(
        when: Option<ReadyWhen>,
        within: Option<Duration>,
        consent: Option<Consents>,
        attended: Attended,
    ) -> Self {
        Self {
            seen: when.is_none(),
            when,
            within: within.unwrap_or(DEFAULT_READY_TIMEOUT),
            armed_at: None,
            consent,
            attended,
            hands_at: None,
        }
    }

    /// **THE SAME BARRIER OVER A PANE THAT HAS BEEN REPLACED** — same terms, nothing latched.
    ///
    /// # ⚠⚠⚠ Why a run's barrier cannot simply be reused across a session replacement
    ///
    /// Every one of the three pieces of state in here is a fact ABOUT ONE PANE, and a loop that
    /// closes its inner session and opens a fresh one keeps none of them:
    ///
    /// * `seen` LATCHES, and it is what makes the barrier cost one look per pump after the first.
    ///   Carried over, a fresh pane whose agent has existed for ten milliseconds is *already ready*
    ///   — R379's defect exactly, reintroduced by a struct field rather than by a missing call.
    /// * `armed_at` is a marker count taken on the old pane's screen.
    /// * `hands_at` is a watermark of how often a PERSON had written into the old pane, so a
    ///   replacement pane's own startup writes would read as an interruption.
    ///
    /// What is kept is everything the CALLER declared — the condition, the patience, the consents and
    /// who is expected — because a replacement session is the same run under the same contracts.
    ///
    /// ⚠ It answers a NEW barrier rather than resetting this one, so the caller has to put it
    /// somewhere: a `reset(&mut self)` could be called and its answer dropped, and this cannot.
    #[must_use]
    pub fn rearmed(&self) -> Self {
        Self::new(
            self.when.clone(),
            Some(self.within),
            self.consent.clone(),
            self.attended,
        )
    }

    /// **WHO THIS RUN'S CALLER SAID WOULD BE WATCHING** — read back rather than kept twice.
    ///
    /// ⚠⚠ The barrier is where this contract lives, because the barrier is what a person's hand
    /// arrives at. A second copy on the loop beside it would be two authorities on one caller
    /// declaration, and the failure of letting two copies drift is silent — so
    /// `awaiting_human`'s wait asks the barrier what the caller declared instead of being handed
    /// its own.
    #[must_use]
    pub const fn attended(&self) -> Attended {
        self.attended
    }

    /// **HAS A PERSON TAKEN THIS PANE SINCE THIS RUN STARTED WATCHING IT?**
    ///
    /// Arms on the first look and compares on every one after — the watermark discipline
    /// [`PaneHands`] is built for, so the pane holds no state on this run's behalf.
    ///
    /// # ⚠⚠ Why the first look can never report an interruption
    ///
    /// A pane a run is handed has usually been typed into already: somebody launched the program in
    /// it. Reading the count as a delta from the first look is what separates *"a person is typing
    /// at this run"* from *"a person once typed here"*, and it is why a watermark exists at all
    /// rather than a flag.
    ///
    /// ⚠ A host with no [`PaneHands`] answers `None` and this returns `None` — *carry on*. An
    /// absence of the capability is not evidence that somebody is present, and reading it as one
    /// would stop every run on every host that has not implemented it.
    ///
    /// [`PaneHands`]: crate::access::PaneHands
    fn interrupted(&mut self, panes: &dyn PaneAccess, pane: PaneId) -> Option<Interruption> {
        let now = panes.hands()?.pane_hands(pane)?.by_a_person();
        let Some(armed) = self.hands_at else {
            self.hands_at = Some(now);
            return None;
        };
        let writes = now.checked_sub(armed).filter(|writes| *writes > 0)?;
        Some(Interruption { writes })
    }

    /// **WAIT FOR THE PERSON TO FINISH, AND TAKE THE PANE BACK WHEN THEY DO** — what a
    /// [`Handback::WhenStill`] run does with an interruption instead of ending on it.
    ///
    /// # ⚠⚠⚠ What ends the wait, and why it is the HAND and not the SCREEN
    ///
    /// A screen that has stopped changing is not a person who has stopped: an agent thinking about
    /// what they just typed paints nothing for a minute, and a pane running a clock paints for
    /// ever. The only thing that answers *is this person still working here* is the thing that
    /// counts THEIR acts — [`sprag_terminal::Hands`], the same watermark that noticed them — so the
    /// wait watches that and nothing else.
    ///
    /// ⚠⚠ AND THE BARRIER'S OWN ORDER DOES THE REST. Whatever they left behind — a dialog they
    /// opened and did not answer, a program they started — is met by the NEXT step's pass through
    /// [`reached`](Self::reached), which asks *is it asking* and *has it started* in the order it
    /// always did. Folding those questions in here would be a second door to a blocked pane, which
    /// is the shape this crate keeps finding defects in.
    ///
    /// # ⚠ Why the watermark is re-armed on the way out, and only there
    ///
    /// [`interrupted`](Self::interrupted) reports a DELTA against `hands_at`. Left where it was,
    /// every look after a handback would report the same writes again and the run would hand the
    /// pane back for ever without driving it once. Re-arming is what makes the answer *an episode
    /// that is over* rather than *a person who once typed*, and it happens only on the arm where
    /// the episode really did end.
    fn await_the_handback(
        &mut self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        took: Interruption,
        run: &RunContext,
    ) -> Reached {
        let (Some(patience), Some(still)) = (
            self.attended.patience(),
            self.attended.handback().stillness(),
        ) else {
            return Reached::Interrupted(took);
        };
        let began = Instant::now();
        // The count the interruption was read from, and the moment this barrier last saw it move. A
        // poll cannot see between its own looks, so `since` is the last time the run OBSERVED
        // stillness begin — later than the keystroke, never earlier. See [`Handback`].
        let mut seen = took.writes();
        let mut since = Instant::now();
        let waited = poll_until(run, patience, || {
            let now = panes
                .hands()
                .and_then(|hands| hands.pane_hands(pane))
                .map_or(seen, |hands| {
                    hands
                        .by_a_person()
                        .saturating_sub(self.hands_at.unwrap_or_default())
                });
            if now != seen {
                seen = now;
                since = Instant::now();
                return false;
            }
            since.elapsed() >= still
        });
        let took = Interruption::of(seen);
        match waited {
            Waited::Ready => {
                // ⚠ The whole episode is now behind the watermark, so the next look starts clean.
                self.hands_at = self.hands_at.map(|armed| armed + seen);
                Reached::HandedBack(Handover {
                    took,
                    held: began.elapsed(),
                })
            }
            // ⚠⚠⚠ THE PANE IS STILL THEIRS, so the run ends exactly as `Handback::Never` would —
            // with the word that says a person has it and the writes of the whole episode, rather
            // than `RunEnded`. `await_the_person`'s rule, for its reason: a run's clock running out
            // underneath does not put the pane back in this run's hands, and reporting the ending
            // instead of the finding would tell a supervisor to raise a budget.
            Waited::TimedOut | Waited::Stopped => Reached::Interrupted(took),
        }
    }

    /// **HAND THE PANE TO THE PERSON WHO IS WATCHING IT, AND WAIT** — what an
    /// [`Attended::APerson`] run does with a refusal instead of ending on it.
    ///
    /// # ⚠⚠⚠ Why EVERY refusal reaches this, and not a chosen few
    ///
    /// The first draft waited only on the arms that read like a missing clause. That was a rule
    /// with no author: every refusal this act can build leaves a QUESTION on a pane with nothing
    /// left to do about it — which is the caller's cue, and this is the caller stating that a
    /// person is there to take it. Picking a subset would mean deciding that some of those
    /// sentences meant it less, which nothing in the type says and no caller could predict.
    ///
    /// ⚠ [`Refusal::NotTaken`] is the arm that proves it rather than the exception it looks like: a
    /// consent was typed and the peer went on asking, so the pane is sitting on a dialog in a state
    /// nobody understands — the case where a human is most obviously wanted, and the one a subset
    /// rule would most likely have left out.
    ///
    /// ⚠⚠ **AND THE RULE IS NOT *"they all ask for a person"***, which is how it read while every
    /// arm happened to. [`Refusal::Unwitnessed`]'s remedy is to READ THE PANE and give the run
    /// longer, and it reaches this door like the rest — where the wait is over before it starts,
    /// since a run stopped is exactly what that arm reports. A subset rule keyed on the sentence
    /// would have had to grow a case for it; keyed on *there is a question and this run is done
    /// with it*, nothing here moved.
    ///
    /// # ⚠⚠ What ends the wait, and why it is the QUESTION and not the STATE
    ///
    /// [`Arrival::LeftTheQuestion`]'s rule, for [`Arrival::LeftTheQuestion`]'s measured reason: a
    /// supervisor's verdict settles, so a pane whose dialog has just been answered goes on reading
    /// `Blocked` for its hysteresis window. Keyed on the state, this would wait out the person's
    /// answer and report that nobody came.
    ///
    /// ⚠⚠ And a DIFFERENT question also ends it. The run does not inspect what the person chose and
    /// has no business doing so — it hands back a spent step, and the next one meets the barrier
    /// again with whatever the pane has become. That is [`Reached::Answered`]'s discipline, applied
    /// to a decision this run did not make at all.
    fn await_the_person(
        &self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        unanswered: Unanswered,
        run: &RunContext,
    ) -> Reached {
        let Some(patience) = self.attended.patience() else {
            return Reached::Asking(unanswered);
        };
        let began = Instant::now();
        let asked = unanswered.question().cloned();
        let waited = poll_until(run, patience, || moved_on(panes, pane, asked.as_ref()));
        match waited {
            Waited::Ready => Reached::Attended(Attention {
                asked: unanswered,
                waited: began.elapsed(),
            }),
            // ⚠⚠⚠ A RUN ENDING UNDERNEATH REPORTS THE QUESTION, not `RunEnded`. The Driver's own
            // rule — *"a peer that stopped to ASK outranks everything below, including the run's
            // own end"* — and the same reasoning: a cancel arriving mid-wait does not make the
            // dialog go away, and `RunEnded` promises nothing was charged, which a `not_taken`
            // refusal would make false.
            Waited::Stopped => Reached::Asking(unanswered),
            // Nobody came. The arm says so and the original reason rides along — see
            // [`Unanswered::unattended`].
            Waited::TimedOut => Reached::Asking(Unanswered::unattended(unanswered, patience)),
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
            // ⚠⚠ THE ONE ARM THAT OWES MORE THAN ITS WORD. Every other refusal is about ONE thing
            // the caller wrote and its standing sentence names the fix; `contradicted` is about
            // several, and a caller holding ten clauses would be sent to find two of them by hand
            // against a dialog they are not looking at. The clauses are gathered from the same
            // `covers` that decided, so the report cannot disagree with the verdict.
            Err(Refusal::Contradicted) => {
                let collided = consent.clauses_about(&question);
                return Ok(Reached::Asking(Unanswered::contradicted(
                    question, &collided,
                )));
            }
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
        // ⚠⚠⚠ A run ending underneath does not un-type the key, and it does not learn what became
        // of it either. The two endings are NOT the same sentence, and saying they were is the
        // defect this arm was split out of: `not_taken` is a fact about the PEER — it went on
        // asking while this run watched — and a run stopped inside the wait watched nothing. See
        // [`Refusal::Unwitnessed`]. Both carry what was spent.
        if settled == Waited::Stopped {
            return Ok(Reached::Asking(Unanswered::unwitnessed(question, bytes)));
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
            // ⚠⚠ THE TWO UNSATISFIED ENDINGS SAY DIFFERENT THINGS, and this is the wait where that
            // matters most: the keys are all sent, so the only question left is what the PEER did,
            // and a stopped run is the one reader that cannot answer it. Measured — the fixture
            // peer had already committed the authorised option (`TOOK 2 VIA 10`) when the run this
            // arm reports on was cancelled.
            Waited::TimedOut => Ok(Reached::Asking(Unanswered::not_taken(question, bytes))),
            Waited::Stopped => Ok(Reached::Asking(Unanswered::unwitnessed(question, bytes))),
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
        // ⚠⚠⚠ FIRST, AND AHEAD OF THE QUESTION BELOW. A person typing into this pane is already
        // dealing with whatever is on it, so a run that consulted its consents here would answer a
        // dialog somebody is part-way through answering by hand — see [`Reached::Interrupted`].
        // Asked every time and before the latch, for the question below's reason: *has somebody
        // reached in* is not answered once and for all, it is answered again on every step.
        if let Some(interruption) = self.interrupted(panes, pane) {
            // ⚠⚠ THE WAIT WRAPS THE ENDING RATHER THAN REPLACING IT — `await_the_person`'s shape.
            // A caller who declared no handback gets `Interrupted` straight back out of this call,
            // so the arm above stays exactly the run-ending answer R372 built.
            return Ok(self.await_the_handback(panes, pane, interruption, run));
        }
        // ⚠⚠⚠ ASKED EVERY TIME, BEFORE THE LATCH AND BEFORE THE CONDITION. *Has it started* is
        // answered once; *is it waiting on a question of its own* is not, and this is the only
        // place all three injecting plugins pass through on their way to a keystroke. Put after
        // the latch it would never run again after the first step, which is exactly the window the
        // defect lived in.
        if let Some(asking) = settled_question(panes, pane, run) {
            // ⚠⚠ THE WAIT WRAPS THE ANSWER RATHER THAN PRECEDING IT. A run that waited for a person
            // FIRST would sit on a dialog its own consents authorise, which is a supervisor being
            // fetched for a decision they already wrote down. Every path that ends in a refusal
            // reaches the wait; the two that do not — an answer given, a run ended — never should.
            return match self.answer(panes, pane, asking, run)? {
                Reached::Asking(unanswered) => {
                    Ok(self.await_the_person(panes, pane, unanswered, run))
                }
                decided => Ok(decided),
            };
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
        // ⚠⚠⚠ THE WAIT WATCHES FOR A QUESTION AS WELL AS FOR READINESS, and until this round it did
        // not — it asked *is the peer asking?* once, above, and then polled the readiness condition
        // alone until its bound.
        //
        // **A PEER THAT IS STARTING UP RAISES ITS FIRST DIALOG DURING EXACTLY THIS WAIT.** That is not
        // a corner: a fresh agent CLI takes seconds to come up and a *"do you trust the files in this
        // folder?"* is the first thing it paints — `sprag-detect` holds captures of five such screens
        // from two real agents. Asked only beforehand, the barrier saw a blank pane, waited out the
        // whole bound and reported `NeverReady`: **the run was told the session never came up, about a
        // session that came up and asked something.** A caller's consent could not reach it either,
        // which is the one thing `may_answer` exists for.
        //
        // ⚠ MEASURED, on the pane an `ai_loop`'s `restarting` opens: the loop's own `resuming` arm for
        // a question was structurally unreachable, so the gate written to drive it ended
        // `exhausted — duration` instead. That is how this was found.
        let mut arrived = None;
        let waited = poll_until(run, self.within, || {
            if let Some(question) = settled_question(panes, pane, run) {
                arrived = Some(question);
                return true;
            }
            self.satisfied(&when, panes, pane)
        });
        match waited {
            Waited::Ready => {
                if let Some(question) = arrived {
                    // ⚠ THE SAME THREE-STEP ANSWER the pre-poll check makes, and deliberately not a
                    // fourth spelling of it: what a run may answer, and who is expected when it may
                    // not, are one decision — see the arm above.
                    return match self.answer(panes, pane, question, run)? {
                        Reached::Asking(unanswered) => {
                            Ok(self.await_the_person(panes, pane, unanswered, run))
                        }
                        decided => Ok(decided),
                    };
                }
                self.seen = true;
                Ok(Reached::Yes)
            }
            // ⚠⚠⚠ THE SAME DIAGNOSIS AS THE ARM BELOW, and that is the point — see
            // [`Reached::RunEnded`]. Which of these two arms a caller lands in is decided by
            // whichever of `ready_within` and the RUN's clock is shorter, which is not a thing they
            // chose; a diagnosis that existed on only one of the paths was therefore handed out by
            // arithmetic nobody wrote down.
            Waited::Stopped => Ok(Reached::RunEnded(self.not_ready(panes, pane, when))),
            Waited::TimedOut => Err(self.not_ready(panes, pane, when)),
        }
    }

    /// **WHAT THIS BARRIER WAS STILL WAITING FOR, AND WHAT THE PANE WAS DOING INSTEAD** — built in
    /// ONE place because both of [`reached`](Self::reached)'s unsatisfied endings hand it back and
    /// they must not be able to say different things.
    ///
    /// ⚠ THE DIAGNOSTIC IS READ AT THE MOMENT THE WAIT ENDS, not carried from arming: what a caller
    /// needs is what the pane was doing when the wait gave up. One read, on the way out of a run
    /// that is already over.
    fn not_ready(&self, panes: &dyn PaneAccess, pane: PaneId, when: ReadyWhen) -> PaneError {
        PaneError::NeverReady {
            // ⚠⚠⚠ READ HERE, WHERE THE MARKER IS STILL IN HAND. `wanted` is moved into the error
            // below, and this is the only fact of the three that has to be asked of the SCREEN
            // rather than of the process table — see `already_showing`.
            already_showing: matches!(when, ReadyWhen::Prints(_))
                && panes
                    .pane_collapsed(pane)
                    .is_some_and(|text| text.contains(when.marker())),
            wanted: when,
            // ⚠ THE ABSENCE OF THE CAPABILITY AND THE ABSENCE OF A JOB ARE DIFFERENT ANSWERS —
            // one is about this build, the other about this pane. See [`PaneDoing`].
            instead: panes.foreground_job().map_or(PaneDoing::Unknown, |jobs| {
                jobs.pane_foreground_leader(pane).map_or(
                    PaneDoing::Nothing,
                    // ⚠⚠ THE SAME TYPE THE PREDICATE DECIDED WITH. Reporting one of the two names
                    // it accepts is what named `"bash"` at a caller who launched `/bin/sh`, and it
                    // differed by platform. See [`JobLeader`].
                    |leader| PaneDoing::Job(JobLeader::of(&leader)),
                )
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
    use crate::consent::Consent;
    use crate::testing::{ENTER_BYTE, asking_peer, screen_showing};
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
                Attended::NoOne,
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
                Attended::NoOne,
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
                None,
                Attended::NoOne
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
            Attended::NoOne,
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

    /// ⚠⚠⚠ **A BARRIER OVER A REPLACED PANE HAS FORGOTTEN THE PANE IT LATCHED ON** —
    /// [`Readiness::rearmed`], and the one thing that makes a loop's session replacement safe.
    ///
    /// # ⚠⚠⚠ Why this cannot be left to the caller remembering
    ///
    /// `seen` LATCHES, deliberately: it is what makes the barrier cost one look per pump after the
    /// first, on a run that pumps hundreds of times. So a driver that closed its pane, opened a fresh
    /// one and kept its barrier would be told *already ready* about a program that had existed for ten
    /// milliseconds — and would type its first prompt into it. That is R379's measured defect (the
    /// pty's own line discipline echoes the text, the delivery reads back as confirmed, Enter goes to
    /// a booting program, and the run sits in `working` for as long as anyone lets it), reintroduced
    /// not by a missing call but by a struct field that was still true about a pane that is gone.
    ///
    /// ⚠⚠ **THE SECOND HALF IS THE CONTROL, and it is the half that makes this a gate rather than an
    /// assertion about a `bool`**: the SAME barrier, unrearmed, answers `Yes` about the same silent
    /// pane. Without it a `rearmed` that returned a barrier which refuses everything would pass.
    #[test]
    fn a_barrier_over_a_replaced_pane_has_forgotten_the_pane_it_latched_on() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        // The pane the barrier clears on: it prints the marker and then waits.
        let first = spawn("printf 'BANNER\\n'; exec cat");
        // ⚠⚠⚠ WAIT FOR THE MARKER, THEN ASK `Shows` — the recorded remedy for the recorded trap, which
        // the first run of this gate walked straight into on the build machine. `Prints` means *MORE
        // occurrences than when the barrier armed*, so on a fast box the fixture's own `printf` lands
        // BEFORE the arming look, the marker goes into the baseline and the barrier can never be
        // satisfied: it refused with `already_showing: true`, which is the correction naming itself.
        // `Shows` is the question that reads a marker already on the screen, and it is the right one
        // here anyway — this gate's subject is the LATCH, not who printed what when.
        crate::testing::started(&access, first, "BANNER");
        let mut ready = Readiness::new(
            Some(ReadyWhen::Shows("BANNER".to_string())),
            Some(Duration::from_millis(1500)),
            None,
            Attended::NoOne,
        );
        assert_eq!(
            ready
                .reached(&access, first, &RunContext::uncancellable())
                .expect("a pane that prints the marker clears this barrier"),
            Reached::Yes,
            "the control: this barrier must really have LATCHED, or what follows is about a barrier \
             that never cleared",
        );

        // ⚠ The replacement, standing in for what `restarting` opens: a pane that never prints the
        // marker at all. Whatever the barrier says about it is a statement about the LATCH.
        let replacement = spawn("exec cat");
        assert_eq!(
            ready
                .reached(&access, replacement, &RunContext::uncancellable())
                .expect("the latched barrier answers without looking"),
            Reached::Yes,
            "⚠⚠ THE CONTROL FOR THE CLAIM BELOW: carried over, the barrier says a pane it has never \
             looked at is ready — which is exactly the answer a loop must not get about a session it \
             has just opened",
        );

        let mut afresh = ready.rearmed();
        crate::testing::refused_naming(
            afresh
                .reached(&access, replacement, &RunContext::uncancellable())
                .as_ref()
                .err(),
            &ReadyWhen::Shows("BANNER".to_string()),
            "cat",
            "⚠⚠⚠ a re-armed barrier must ASK AGAIN on the pane that replaced the old one — a loop \
             that inherits `seen` types its first prompt into a program that is still starting",
        );
        let lifecycle = <WorkspacePaneAccess as PaneAccess>::lifecycle(&access).expect("lifecycle");
        lifecycle.close(first);
        lifecycle.close(replacement);
    }

    /// ⚠⚠⚠ **AND THE REFUSAL SAYS THE ONE THING THAT FIXES IT: THE MARKER IS ALREADY THERE.**
    ///
    /// The gate above proves the barrier is RIGHT to refuse. This one is about whether the caller
    /// can act on the refusal, and until it existed they could not. What they were handed was:
    ///
    /// > *the pane never printed `"BANNER"`, which this run was told to wait for before driving
    /// > it, so nothing was injected; its terminal belonged to `"cat"` instead*
    ///
    /// — every word of it true, and the correction is not in it. [`PaneDoing`] answers *what owns
    /// the terminal*, which diagnoses [`ReadyWhen::Runs`] and [`ReadyWhen::Settles`] precisely and
    /// says nothing at all about a MARKER. So the commonest readiness mistake there is — naming
    /// `prints` for a banner the pane had already printed — reports a job name the caller never
    /// asked about and stays invisible.
    ///
    /// # ⚠⚠⚠ Why this is the mistake that had to be named
    ///
    /// A caller opens a pane, the program in it announces itself once, and some time later they ask
    /// for a run. **Every one of those is a separate call**, so by the time the barrier arms the
    /// announcement is long past — and `prints`, whose whole contract is *more occurrences than
    /// when I armed*, can then never clear. It is not an exotic case: it is what happens whenever
    /// the pane was opened before the run was asked for, which is the normal order.
    ///
    /// ⚠⚠ It is also this workspace's own recorded flake. A suite running at thirty threads
    /// deschedules the gap between a fixture's `spawn` and the driver's first look for longer than
    /// the peer takes to print its banner, and the run then waits out its whole clock having typed
    /// nothing (`Exhausted(Duration)`, `Bytes(0)`). That signature has been in
    /// `.claude/remote-build.toml` for rounds as a HYPOTHESIS nobody had instrumented. This is the
    /// instrument, and the product is where the answer belongs.
    ///
    /// ⚠ **THE FACT, NOT THE INTENT.** It says the marker is on the screen and which question would
    /// have read it; it does not say the caller meant that one. A peer that re-announces every turn
    /// is a caller who meant `prints` exactly, and whose real finding is that the peer went quiet.
    #[test]
    fn a_marker_that_is_already_on_the_screen_is_named_as_such_by_the_refusal() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("printf 'BANNER\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        // The window a loaded machine opens by accident, opened deliberately: the marker is on the
        // screen BEFORE the barrier's first look, which is the only state this gate is about.
        crate::testing::screen_showing(&access, pane, "BANNER");

        let failed = Readiness::new(
            Some(ReadyWhen::Prints("BANNER".to_string())),
            Some(Duration::from_millis(200)),
            None,
            Attended::NoOne,
        )
        .reached(&access, pane, &RunContext::uncancellable())
        .expect_err("a marker that was on screen at arming can never be exceeded");

        let said = failed.to_string();
        assert!(
            said.contains("already on its screen"),
            "⚠⚠⚠ the refusal must say the marker IS THERE. Without it the caller is told what owns \
             the terminal — true, and about a question they did not ask — and the one fact that \
             corrects their call is the one the barrier had in its hand and did not pass on: \
             {said}",
        );
        assert!(
            said.contains("shows"),
            "⚠⚠ and it names the question that WOULD have read it, in the caller's own wire word, \
             or the correction is one they have to already know to act on: {said}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
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
            Attended::NoOne,
        )
        .reached(&access, pane, &RunContext::uncancellable())
        .expect_err("a pane with no child can never come to run anything");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Nothing,
                // ⚠ A PROGRAM NAME IS NOT SCREEN TEXT. `Runs` fails about the process table, so the
                // screen answer is not asked for — see `already_showing`.
                already_showing: false,
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
                None,
                Attended::NoOne
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
                Attended::NoOne,
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
            Readiness::new(
                Some(ReadyWhen::Runs(name.to_string())),
                Some(within),
                None,
                Attended::NoOne,
            )
            .reached(&access, pane, &RunContext::uncancellable())
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
                None,
                Attended::NoOne
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
            Attended::NoOne,
        );
        let failed = ready
            .reached(&NoProcessView, PaneId(1), &RunContext::uncancellable())
            .expect_err("a host that cannot see the process table can never confirm the program");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Unknown,
                already_showing: false,
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
                None,
                Attended::NoOne
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
    // ⚠⚠⚠ EVERY GATE BELOW DRIVES A REAL PSEUDOTERMINAL RUNNING A REAL MENU, through
    // [`crate::testing::asking_peer`] — which is where the reasoning about that fixture lives, and
    // which is SHARED with the plugin that answers a pane on its own ([`crate::answer::Answer`]).
    // One peer, because two spellings of *"what a dialog does to a keystroke"* is two products.

    /// A barrier with no readiness condition and the given consent — the shape every gate below
    /// wants, since what is under test is the ANSWERING contract and not the starting one.
    fn answering(consent: Option<Consents>) -> Readiness {
        Readiness::new(
            None,
            Some(Duration::from_millis(200)),
            consent,
            Attended::NoOne,
        )
    }

    /// The same barrier with somebody watching the pane — the control's twin, differing in exactly
    /// the one contract these gates are about.
    fn watched(consent: Option<Consents>, patience: Duration) -> Readiness {
        Readiness::new(
            None,
            Some(Duration::from_millis(200)),
            consent,
            // ⚠ `Handback::Never` and not the round's new arm: these gates are about waiting for a
            // person to ANSWER, and a handback would add a second wait to every one of them.
            Attended::of(patience, Handback::Never).expect("a positive patience"),
        )
    }

    /// A one-clause consent for the measured permission question, authorising the option carrying
    /// `answer` — the shape every gate in this section is about, since what they measure is the
    /// KEYSTROKE a single authorised option produces. What SEVERAL clauses say about one question
    /// is `Consents::covers`'s own business and is gated there.
    fn consent_to(answer: &str) -> Consents {
        Consents::of(vec![
            Consent::parse("Do you want to proceed?".to_string(), answer.to_string())
                .expect("two needles"),
        ])
        .expect("a non-empty list")
    }

    /// ⚠⚠⚠ **NOBODY CAME, AND THE RUN SAYS BOTH THINGS THAT ARE TRUE.**
    ///
    /// The other end of [`Attended`]: a caller declared a person was watching, the peer asked
    /// something no clause covered, and the patience ran out with the dialog still up. Two facts
    /// with two different remedies, and a report that carried only one of them would send the
    /// caller the wrong way — so the ARM says nobody came and the DETAIL says what they would have
    /// been answering.
    ///
    /// ⚠ It also pins the direction of the wait: the run must still have typed NOTHING. Waiting is
    /// a widening of what a run may wait FOR and not of what it may decide, and the pane is the
    /// witness for that.
    #[test]
    fn a_person_who_never_comes_is_reported_as_such_and_keeps_the_reason_underneath() {
        let (access, pane) = asking_peer("either");
        // ⚠ A consent about a DIFFERENT question, so the refusal underneath is `other_question`
        // rather than `no_consent` — the arm a caller acts on by writing a clause, and the one
        // that would be silently lost if `unattended` overwrote it.
        let elsewhere = Consents::of(vec![
            Consent::parse(
                "Do you want to make this edit?".to_string(),
                "Yes".to_string(),
            )
            .expect("two needles"),
        ]);
        let began = Instant::now();
        let waited_out = watched(elsewhere, Duration::from_millis(400))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a blocked peer is not an error");
        let took = began.elapsed();

        let Reached::Asking(unanswered) = waited_out else {
            panic!("⚠⚠⚠ nobody came, so the run must still hand the question over: {waited_out:?}");
        };
        assert_eq!(
            unanswered.why(),
            Refusal::Unattended,
            "⚠⚠⚠ the arm must say that the PERSON did not come. Reported as `other_question` the \
             caller goes looking for a clause to write, when what actually happened is that the \
             human they promised was not there: {unanswered:?}",
        );
        let said = unanswered.explain();
        assert!(
            said.contains("other_question"),
            "⚠⚠ and the reason underneath SURVIVES — a caller whose own consents could have \
             answered this must not be told only to wait longer: {said:?}",
        );
        assert_eq!(
            unanswered.bytes(),
            0,
            "a run that waited typed nothing, and waiting is not a licence to decide",
        );
        assert!(
            took >= Duration::from_millis(400),
            "⚠⚠ and it actually WAITED the patience it was given. A gate that passes without the \
             clock moving is measuring the refusal, not the wait: {took:?}",
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("SAW") && !screen.contains("TOOK"),
            "⚠⚠⚠ NOT ONE KEY, over the whole wait — the pane is the witness: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A RUN DOES NOT FETCH A PERSON FOR A DECISION THEY ALREADY WROTE DOWN.**
    ///
    /// The ordering claim, and it is the one a plausible implementation gets backwards: waiting
    /// BEFORE consulting the consent would leave a supervisor staring at a dialog their own
    /// standing rule authorises, for the whole patience, on every turn. The wait wraps the answer;
    /// it does not precede it.
    ///
    /// ⚠ The clock is the assertion. Both orderings end with the peer answered, so only the time
    /// taken tells them apart.
    #[test]
    fn a_question_the_consent_covers_is_answered_at_once_and_nobody_is_fetched() {
        let (access, pane) = asking_peer("either");
        let began = Instant::now();
        let reached = watched(Some(consent_to("Yes")), Duration::from_secs(30))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a blocked peer is not an error");
        let took = began.elapsed();
        assert!(
            matches!(reached, Reached::Answered(_)),
            "⚠⚠⚠ the clause covers this question, so the run answers it — a `Attended` here is a \
             run that went to find a human for a decision it was holding: {reached:?}",
        );
        assert!(
            took < Duration::from_secs(5),
            "⚠⚠⚠ AND IT DID NOT WAIT FIRST. The patience is thirty seconds and an answered dialog \
             must not pay any of it: {took:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE SETTLE WINDOW ENDS THE WAIT, BECAUSE IT IS WHAT AN ANSWERED DIALOG LOOKS LIKE.**
    ///
    /// A supervisor's verdict SETTLES: a real detector goes on calling a pane `blocked` for its
    /// hysteresis window after the dialog has left the screen, with nothing readable on it. That
    /// state — blocked, no menu — is therefore two things at once, and `marker_arrived` already
    /// merges them for a reason MEASURED end to end: read as *"still asking"*, the wait outlives
    /// the very answer it is waiting for, and a person who answered inside their patience is
    /// reported as never having come.
    ///
    /// # ⚠⚠⚠ Why this is a unit gate on the predicate and not an end-to-end run
    ///
    /// **It was mutated both ways first.** Flipping this arm to `false` left BOTH the plugin's
    /// end-to-end gate and the live daemon's green: in each of them the person's answer takes the
    /// peer all the way out of `blocked`, so the merged arm is never the one that decides, and the
    /// mutation costs only the settle's own length against a patience many times larger. The arm
    /// is reachable and the defect is real — a patience SHORTER than the detector's settle turns an
    /// answered dialog into `unattended` — but reproducing THAT through a pane means racing a
    /// fixture's clock against a product's, which is the shape every load-marginal red in this
    /// crate has had.
    ///
    /// ⚠⚠ **AND THE RESIDUE IS NOW CLOSED FROM THE OTHER SIDE.**
    /// [`a_person_who_answers_is_not_waited_out_by_the_supervisors_own_hysteresis`] drives the same
    /// arm through the BARRIER by measuring LATENCY instead of expiry — the mutation below costs
    /// the supervisor's whole hysteresis (301 ms measured against the fixture's 300) and no clock
    /// of the fixture's has to beat a clock of the product's. This one stays because it is the
    /// exact statement, and because it also gates the `None` control the timed one cannot reach.
    #[test]
    fn a_pane_whose_menu_has_gone_but_whose_verdict_has_not_counts_as_answered() {
        /// A pane its supervisor still calls blocked, with NO question readable on it — the
        /// settle window, and the state a peer is in for `DEFAULT_SETTLE` after being answered.
        struct StillBlockedNothingReadable;
        impl crate::access::PaneSupervision for StillBlockedNothingReadable {
            fn pane_agent_state(&self, _id: PaneId) -> Option<crate::access::AgentObservation> {
                Some(crate::access::AgentObservation {
                    state: AgentState::Blocked,
                    agent: Some("claude".to_owned()),
                    // ⚠ The SCREEN-read authority, deliberately: the settle window is a property
                    // of a detector sampling a pane, and a `Reported` verdict comes from inside
                    // the agent and has no hysteresis to model.
                    authority: crate::access::Authority::Scraped {
                        rule: Some("permission-menu".to_owned()),
                    },
                    seq: 1,
                    asking: None,
                })
            }
        }
        impl PaneAccess for StillBlockedNothingReadable {
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
            fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
                Some(self)
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[crate::access::KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                panic!("⚠⚠ a run waiting for a person types NOTHING");
            }
        }

        let was_asked = sprag_detect::Question {
            asked: vec!["Do you want to proceed?".to_owned()],
            choices: vec![sprag_detect::Choice {
                number: 1,
                label: "Yes".to_owned(),
                selected: true,
            }],
        };
        assert!(
            moved_on(&StillBlockedNothingReadable, PaneId(1), Some(&was_asked)),
            "⚠⚠⚠ the dialog this run stopped on is off the screen, and only the supervisor's own \
             hysteresis still says otherwise. Read as still-asking, a person who answered inside \
             their patience is reported as never having come",
        );
        // ⚠ THE CONTROL, and it is what keeps the arm above from being *"anything blocked counts
        // as answered"*: the SAME unreadable state, for a run that stopped on an unreadable dialog
        // in the first place. There is no sentence to compare, so nothing here says a person acted.
        assert!(
            !moved_on(&StillBlockedNothingReadable, PaneId(1), None),
            "⚠⚠⚠ a run that stopped on a dialog it could NOT read has no sentence to compare \
             against, so only the pane ceasing to be blocked can say a person dealt with it — \
             resuming here would type into whatever is still up",
        );
    }

    /// ⚠⚠⚠ **A RUN RESUMES THE MOMENT THE PERSON ANSWERS, NOT WHEN THE SUPERVISOR CATCHES UP** —
    /// the settle-window arm measured THROUGH THE BARRIER, which R371 left open as a residue.
    ///
    /// # ⚠⚠⚠ How it escapes the race that made the predicate gate the honest answer first
    ///
    /// The obvious construction is a patience SHORTER than the settle, so a mutated arm runs out of
    /// it and reports `unattended`. That pits the fixture's clock against the product's and is the
    /// shape every load-marginal red in this crate has had.
    ///
    /// **The discriminator is LATENCY, and it needs no expiry at all.** The person's answer takes
    /// the menu off the screen at a moment this test KNOWS, because this test is the one that
    /// typed it. From there:
    ///
    /// * the shipping arm reads `blocked, nothing readable` as *the question is over* and returns
    ///   within a [`poll_until`] tick — 10 ms;
    /// * an arm that waits for the STATE instead cannot return until the supervisor's hysteresis
    ///   expires, which is the fixture's 300 ms and the real detector's `DEFAULT_SETTLE` seconds.
    ///
    /// So the assertion is that the run came back FAST, with a patience so generous it can never
    /// be the thing that ended the wait. ⚠ Thirty times the poll interval and a third of the
    /// settle: wide enough that a loaded box does not fail it, and nowhere near what the mutant
    /// must pay.
    ///
    /// ⚠ The person waits before answering, so the barrier is provably inside its wait rather than
    /// still reading the screen — and if that ordering ever loses, the outcome is `Yes` and the
    /// message below says so rather than a latency number nobody can interpret.
    #[test]
    fn a_person_who_answers_is_not_waited_out_by_the_supervisors_own_hysteresis() {
        let (access, pane) = asking_peer("either");
        let answered_at = std::sync::Mutex::new(None::<Instant>);

        let reached = std::thread::scope(|watching| {
            watching.spawn(|| {
                // ⚠ Long enough that `reached` is provably past its screen read and inside
                // `await_the_person`, which it enters within a millisecond of being called.
                std::thread::sleep(Duration::from_millis(150));
                *answered_at.lock().expect("the clock") = Some(Instant::now());
                let _typed = access
                    .inject(pane, &KeyStroke::text("1"))
                    .expect("the person types");
            });
            // ⚠ TEN SECONDS of patience. Whatever ends this wait, it is not the bound.
            watched(None, Duration::from_secs(10))
                .reached(&access, pane, &RunContext::uncancellable())
                .expect("a blocked peer is not an error")
        });
        let back_at = Instant::now();

        let Reached::Attended(attention) = reached else {
            panic!(
                "⚠⚠ the person answered and the barrier must report that a person did: {reached:?} \
                 — a `Yes` here means the fixture's own ordering lost and the wait never started",
            );
        };
        assert_eq!(
            attention.asked().why(),
            Refusal::NoConsent,
            "and it carries what this run could not answer for itself",
        );
        let after_the_answer =
            back_at.duration_since(answered_at.lock().expect("the clock").expect("they typed"));
        assert!(
            after_the_answer < Duration::from_millis(100),
            "⚠⚠⚠ THE RUN WAITED OUT THE SUPERVISOR'S HYSTERESIS INSTEAD OF THE PERSON. It came \
             back {after_the_answer:?} after the dialog was answered, and a verdict that settles \
             is the one thing this wait must not key on — the real detector's window is SECONDS, \
             so a person who answered inside their patience would be reported as never having come",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A PATIENCE OF ZERO IS NOT A SPELLING OF `NoOne`.**
    ///
    /// [`Consents::of`]'s rule one level up: two spellings of one behaviour make the caller who
    /// arrived at the first by arithmetic — a deadline already past, a config defaulting to 0 —
    /// silently get the other. They are told instead.
    #[test]
    fn a_watch_with_no_patience_cannot_be_built() {
        assert!(
            Attended::of(Duration::ZERO, Handback::Never).is_none(),
            "⚠⚠ zero patience must be unrepresentable, not a quiet `NoOne`",
        );
        assert_eq!(
            Attended::of(Duration::from_millis(1), Handback::Never),
            Some(Attended::APerson {
                patience: Duration::from_millis(1),
                handback: Handback::Never,
            }),
            "and any positive patience is a person",
        );
        assert_eq!(
            Attended::NoOne.patience(),
            None,
            "nobody watching has no patience to spend",
        );
        // ⚠⚠⚠ AND THE SAME RULE ONE LEVEL IN, for the argument that joined this one. A stillness of
        // zero is *"the pane is mine again the instant they pause"*, and every person pauses between
        // keystrokes — so it is not a spelling of `Never`, it is a request nobody can have meant.
        assert!(
            Handback::of(Duration::ZERO).is_none(),
            "⚠⚠ zero stillness must be unrepresentable, not a quiet `Never`",
        );
        assert_eq!(
            Handback::of(Duration::from_millis(1)),
            Some(Handback::WhenStill(Duration::from_millis(1))),
            "and any positive stillness is a handback",
        );
        // ⚠⚠⚠ THE STRUCTURAL CLAIM THIS TYPE EXISTS FOR: a handback cannot be declared for a run
        // nobody is watching. There is no `Attended` value carrying one without a patience, so the
        // combination is not refused at runtime — it cannot be SPELT. This asserts the consequence
        // callers see, which is that the absent person's handback is `Never` and nothing else.
        assert_eq!(
            Attended::NoOne.handback(),
            Handback::Never,
            "⚠⚠⚠ a run nobody is watching has nobody to give the pane back to, and the type is why: \
             `Handback` lives inside `APerson`, so `NoOne` has no room to carry one",
        );
        assert_eq!(
            Attended::of(
                Duration::from_secs(1),
                Handback::WhenStill(Duration::from_secs(2))
            )
            .map(Attended::handback),
            Some(Handback::WhenStill(Duration::from_secs(2))),
            "and a declared one comes back out of the pair it was declared with",
        );
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

    /// ⚠⚠⚠ **A RUN THAT WAS STOPPED INSIDE THE ANSWER'S WAIT SAYS NOBODY LOOKED**, and must not
    /// say that the peer refused the key.
    ///
    /// The measurement is the pair, and the second half is what makes it a defect rather than a
    /// wording preference: this peer TAKES the option — its own screen says `TOOK 2` — and the
    /// answering act was cancelled before it could read that. [`Refusal::NotTaken`]'s own sentence
    /// is *"the run typed the option the consent authorised and did not see the peer take it"*, so
    /// reporting it here publishes a claim about the AGENT that is not merely unestablished but
    /// false, and sends a reader to hand a pane to a person over a dialog that is already gone.
    ///
    /// ⚠ [`Delivered::Unwitnessed`](crate::deliver::Delivered::Unwitnessed) is the same finding one
    /// keystroke earlier, and this is the sweep that asked *where else does this crate press a key
    /// and then report on it*.
    ///
    /// ⚠ REVERT-PROOF: return `not_taken` from either stopped arm of `answer` and this reads the
    /// word the pane disproves.
    #[test]
    fn a_run_stopped_inside_the_answers_wait_does_not_blame_the_peer_for_it() {
        // ⚠ THE ESCALATION'S WAIT, which is the later of the two places this act gives up: the
        // marker peer moves its highlight on the digit and commits on the Enter, so the run types
        // the number, sees the marker arrive, and sends the Enter — and THAT is the key this
        // double's cancel rides in on.
        let stopping = crate::testing::StopsAtTheKey::nth(asking_peer("marker").0, 2);
        let pane = stopping.pane.pane_ids()[0];
        let began = Instant::now();
        let reached = answering(Some(consent_to("do not ask again")))
            .reached(&stopping, pane, &stopping.run())
            .expect("a run ending underneath is not an error");
        let took = began.elapsed();

        let Reached::Asking(unanswered) = reached else {
            panic!("the run ended before anything could confirm the answer: {reached:?}");
        };
        assert_eq!(
            unanswered.why(),
            Refusal::Unwitnessed,
            "⚠⚠⚠ the run was cancelled with two keys already on the pseudoterminal, so what it \
             knows is that NOBODY WATCHED — `not_taken` is a claim about the peer that this run \
             never made an observation about",
        );
        assert_eq!(
            unanswered.bytes(),
            2,
            "⚠ and both keys are still charged: a run under a cost ceiling that dropped what it \
             spent because it was cancelled would under-report its own spend",
        );
        assert!(
            unanswered.question().is_some(),
            "and the question travels, so a reader knows WHICH dialog is in an unknown state",
        );
        assert!(
            took < ANSWER_WITHIN,
            "⚠ it gives up INSIDE the wait rather than riding out the answering bound: {took:?}",
        );

        let screen = screen_showing(&stopping.pane, pane, "TOOK");
        assert!(
            screen.contains("TOOK 2 VIA 10"),
            "⚠⚠⚠ AND THE PEER TOOK IT. That is what makes `not_taken` false here rather than \
             merely unproven — the option the caller authorised was chosen, by the Enter, and the \
             run that had just sent it was stopped before it could look: {screen:?}",
        );
        stopping.pane.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AND AT THE FIRST KEY TOO** — the earlier of the two places the answering act gives up,
    /// staged against a peer that really does ignore everything.
    ///
    /// The pair with [`an_answer_the_peer_ignores_is_reported_rather_than_confirmed_anyway`]:
    /// identical peer, identical consent, one key typed in both — and the two runs differ only in
    /// whether the run survived the wait that followed. A product that answered `not_taken` to both
    /// would be telling a reader the same thing about a peer it watched for four seconds and about
    /// a peer it never looked at once.
    #[test]
    fn the_first_keys_wait_reports_the_stop_as_its_own_ending() {
        let stopping = crate::testing::StopsAtTheKey::nth(asking_peer("deaf").0, 1);
        let pane = stopping.pane.pane_ids()[0];
        let reached = answering(Some(consent_to("do not ask again")))
            .reached(&stopping, pane, &stopping.run())
            .expect("a run ending underneath is not an error");
        let Reached::Asking(unanswered) = reached else {
            panic!("nothing was answered: {reached:?}");
        };
        assert_eq!(unanswered.why(), Refusal::Unwitnessed);
        assert_eq!(unanswered.bytes(), 1, "the digit, and nothing after it");
        let screen = screen_showing(&stopping.pane, pane, "SAW");
        assert!(
            screen.contains("SAW 50"),
            "⚠ the digit really did reach the peer, which is what separates this from a run that \
             stopped before it typed — that one is `Reached::RunEnded` and charges nothing: \
             {screen:?}",
        );
        stopping.pane.lifecycle().expect("lifecycle").close(pane);
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
            let consent = Consents::of(vec![
                Consent::parse(asked.to_string(), answer.to_string()).expect("two needles"),
            ])
            .expect("a non-empty list");
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

    /// ⚠⚠⚠ **CONSENTS THAT DISAGREE TYPE NOTHING AT THE PANE**, driven at the one door a keystroke
    /// can come out of.
    ///
    /// [`Consents::covers`](crate::consent::Consents::covers) decides the precedence and is gated
    /// there over values; this is the claim that matters — that the decision reaches the PANE as
    /// silence. The caller has authorised `Yes` for a question about proceeding and `No, and tell`
    /// for one about `rm -rf`, and the dialog on screen carries both phrases: a run that resolved
    /// that would be applying a precedence rule nobody wrote down, and it would be applying it to
    /// two options that mean opposite things.
    ///
    /// ⚠ THE PANE IS THE WITNESS, not the refusal word. This peer prints `SAW <byte>` for any key
    /// it ignores and `TOOK` when it acts, so a run that typed anything at all is visible even if
    /// the dialog swallowed it.
    #[test]
    fn consents_that_disagree_about_the_question_on_screen_leave_it_untouched() {
        let (access, pane) = asking_peer("either");
        let disagreeing = Consents::of(vec![
            Consent::parse("proceed".to_string(), "Yes".to_string()).expect("two needles"),
            Consent::parse("Bash command".to_string(), "No, and tell".to_string())
                .expect("two needles"),
        ])
        .expect("a non-empty list");
        let reached = answering(Some(disagreeing))
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a refusal is not an error");
        let Reached::Asking(unanswered) = reached else {
            panic!("⚠⚠⚠ two clauses naming opposite options must answer NEITHER: {reached:?}");
        };
        assert_eq!(
            unanswered.why(),
            Refusal::Contradicted,
            "and the reason is the one the caller can act on — narrow one of their OWN rules, \
             which is a different remedy from every other arm here",
        );
        assert_eq!(unanswered.bytes(), 0);
        assert!(
            unanswered.why().describe().contains("caller"),
            "the remedy names whose decision it is: {}",
            unanswered.why().describe(),
        );
        // ⚠⚠⚠ AND THE BARRIER MUST NAME THE CLAUSES, not merely reach the right arm. The
        // constructor that carries them is gated over values in `consent`; this is the claim that
        // THIS caller uses it — the recorded rule that a unit test on a method is not a test that
        // anybody calls it. Built through `Unanswered::refused` instead, the arm and the remedy
        // stay exactly right and only the caller's own words go missing.
        let said = unanswered.explain();
        for needle in ["proceed", "Bash command", "No, and tell"] {
            assert!(
                said.contains(needle),
                "⚠⚠⚠ the report must quote {needle:?} — the caller wrote it, and with several \
                 clauses in hand `contradicted` alone sends them hunting: {said}",
            );
        }
        std::thread::sleep(Duration::from_millis(80));
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("TOOK") && !screen.contains("SAW"),
            "⚠⚠⚠ NOT ONE KEY reaches a dialog the caller's own consents disagree about: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
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
