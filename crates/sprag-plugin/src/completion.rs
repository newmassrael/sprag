//! The completion contract — *what makes a peer's TURN OVER*.
//!
//! One home, for [`readiness`](crate::readiness)' reason: the plugins that wait for a peer decide
//! it for the same reason and none of them is the reason.
//!
//! # ⚠⚠⚠ The asymmetry this exists to close
//!
//! The START of a turn has had a declared, caller-chosen contract since R359b —
//! [`ReadyWhen`](crate::readiness::ReadyWhen), four kinds, and the caller says which question they
//! are asking because **only they know**. Answering the wrong one is silent, which is what earned
//! that a protocol number.
//!
//! The END of a turn had nothing. Each plugin hard-coded a rule of its own:
//!
//! * [`Agent`](crate::agent::Agent) waits for the pane's CHILD TO EXIT.
//! * [`Dialogue`](crate::dialogue::Dialogue) waits for the pane's CHILD TO EXIT — **the same rule,
//!   spelled a second time, in a second module**, with nothing comparing the two.
//! * [`Orchestrator`](crate::orchestrator::Orchestrator) waits for the pane to produce output that
//!   is not the echo of what was typed.
//!
//! One concept, three spellings, none of them nameable by a caller. That is the shape this project
//! has paid for repeatedly — a mouse button encoded in two crates, `full_text` serving two
//! contracts, ten addresses served and never declared — and the remedy is always the same: **give
//! the concept ONE definition and make the caller say which one they mean.**
//!
//! # ⚠ Why the orchestrator's rule is named here and not yet MOVED here
//!
//! The first two are the same predicate over the same evidence, so collapsing them was a rename and
//! nothing else. The third is not: *"the pane produced output that is not the echo"* needs context
//! a turn has to carry — where this turn's output starts (a
//! [`RowTrail`](crate::access::RowTrail) marked BEFORE the injection) and what was typed, so the
//! terminal's echo of it can be discounted. [`Exits`](DoneWhen::Exits) needs neither.
//!
//! Whether that context belongs in each variant's PAYLOAD or in a uniform per-turn evaluator was a
//! decision deliberately left until a second context-needing rule existed, so the API would be read
//! off more than its least typical case. [`Settles`](DoneWhen::Settles) is that second rule and it
//! ANSWERED: it needs to remember the peer's state at the turn's start, which is not a payload the
//! caller can supply — so the evaluator is [`Completion`], armed by
//! [`begin`](Completion::begin), exactly the shape [`Readiness`](crate::readiness::Readiness) uses
//! at the other end.
//!
//! ⚠ The orchestrator's rule now has somewhere to go: its baseline is armed at the same moment
//! `begin` is, so it becomes a third variant holding a [`RowTrail`](crate::access::RowTrail) rather
//! than a fourth spelling. It has not moved yet because that is a behaviour change in the loop
//! whose echo-discounting rule is the subtlest here, and it is owed its own gate rather than a ride
//! on this one.
//!
//! # ⚠⚠ Why the hard-coded rule is not merely untidy
//!
//! *"The child exited"* is a ONE-SHOT TOOL's completion. A long-lived interactive peer — the whole
//! class [`deliver`](mod@crate::deliver), `shows_the_prompt` and
//! [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles) were built for — never exits, so
//! the wait can only end on the CLOCK. The turn is then as long as the timeout (two minutes by
//! default), every time, and the capture is whatever was on screen when it ran out.
//!
//! ⚠ Note what the barrier at the other end can already ask and this one cannot:
//! [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles) reads the peer's own
//! [`AgentObservation`] — *"this agent is at rest, waiting for
//! input"* — carrying an [`Authority`](crate::access::Authority) that says how much the reading is
//! worth. **The evidence existed, it was published, and the end of the turn did not consult it.**
//! [`DoneWhen::Settles`] is that consultation.
//!
//! ⚠⚠ It arrives with its gates and not after them, because a completion rule that fires EARLY
//! truncates a model's answer and publishes the fragment as the reply — the exact failure class
//! this crate has paid for four times, reached from a new direction. The first gate written for it
//! is the one that holds a peer's PRE-TURN rest to not being an answer.

use std::time::{Duration, Instant};

use sprag_detect::{AgentState, Question};
use sprag_terminal::PaneId;

use crate::access::{AgentObservation, PaneAccess};
use crate::run::{Look, RunContext, Waited, park_until};

/// **HOW A TURN ENDED** — the answer [`Completion::wait`] gives, and the twin of
/// [`Reached`](crate::readiness::Reached) at the other end of the same turn.
///
/// # ⚠⚠⚠ Why a turn's end could not be a `bool`, measured
///
/// It used to be [`Waited`]: ready, timed out, or the run stopped. Three answers, and a peer that
/// stops to ASK is none of them — its turn IS over, it will not write another word until somebody
/// decides something, and the contract had no way to say so. So the wait ran to its bound and
/// answered [`Waited::TimedOut`], which means *the peer did not finish* about a peer that finished.
///
/// The cost is the bound, and [`Turn`]'s own doc tells a caller to size that to their peer — *"a
/// shell command is a second and an agent asked to read a repository is minutes"*. So the better a
/// caller sized it the more each dialog cost them, and with no bound at all (the legal spelling
/// that means *wait for my peer*) a single question spent the run's entire remaining clock.
/// `the_end_of_a_turn_waits_out_an_ask_that_the_start_of_one_reads_at_once` is that measurement,
/// kept as this type's control.
///
/// ⚠⚠ **And the evidence was never hard to come by**, which is what made it a defect rather than a
/// limit: the barrier at the START of the same turn has read it out of the same supervisor, about
/// the same pane, in milliseconds, since R366. One [`AgentObservation`],
/// two ends of one turn, and only one of them was looking.
///
/// # ⚠ Why a new type rather than a fourth [`Waited`] arm
///
/// R356's rule: when a new state must be handled everywhere an old one was, RENAME rather than add.
/// A fourth arm on `Waited` would have left every `== Waited::TimedOut` in this crate compiling and
/// silently reading *the peer stopped to ask* as *the peer never answered* — which for
/// [`Agent`](crate::agent::Agent) means publishing a permission dialog as the model's reply. A type
/// of its own makes each of the three call sites decide.
///
/// # ⚠⚠⚠ AND [`PeerGone`](Self::PeerGone) IS AN ADDED ARM, WHICH THE PARAGRAPH ABOVE ARGUES AGAINST
///
/// So the sites that compare this type with `==` rather than matching it were COUNTED before it was
/// written, and each one's new answer decided rather than inherited:
///
/// * [`Agent`](crate::agent::Agent) tested `== NotYet` to head its note *the peer never finished*.
///   A gone peer now takes an arm of its own there and the run STOPS, which is the change: it used
///   to publish whatever was on the screen of a pane whose program had exited.
/// * [`judge`](crate::judge) tests `!= Yes` and treats everything else as *no verdict came back*.
///   Correct unchanged — a judge whose peer died gave no verdict.
/// * [`Dialogue`](crate::dialogue::Dialogue) tests `== RunEnded` and is deliberately unarmed, so
///   this arm cannot reach it.
///
/// ⚠ A RENAME was not available: this is not an ending the old vocabulary covered under another
/// name, it is the one that was reported as *the peer did not finish* about a peer that will never
/// finish anything again.
///
/// # ⚠⚠⚠⚠ AND [`Silent`](Self::Silent) IS THE SECOND ADDED ARM, COUNTED THE SAME WAY
///
/// It is UNREACHABLE unless the caller hands [`wait`](Completion::wait) a [`Quiet`] bound, which is
/// what makes the count short rather than what excuses skipping it:
///
/// * [`Agent`](crate::agent::Agent) passes no bound, so its `== NotYet` heading — *the peer never
///   finished* — cannot meet this. Decided rather than inherited: a one-shot `claude -p` has no
///   reporter to fall silent, and calling it silent would be this crate inventing a fact.
/// * [`judge`](crate::judge) passes none and tests `!= Yes`; correct unchanged, since a judge whose
///   peer went quiet gave no verdict.
/// * [`Dialogue`](crate::dialogue::Dialogue) passes none and tests `== RunEnded`, so it is unarmed
///   here exactly as it is for [`PeerGone`](Self::PeerGone).
/// * [`OuterLoop`](crate::outer::OuterLoop) is the one caller that asks, and it MATCHES — both of
///   its two waits, each with an answer of its own. See `watch` and `attend`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Over {
    /// **YES** — on the evidence the caller's [`DoneWhen`] named.
    Yes,
    /// **THE PEER STOPPED TO ASK**, carrying the question when this host can read it.
    ///
    /// The turn is over: an agent showing a dialog is waiting on a DECISION, and nothing a caller
    /// does to the pane short of answering will produce another word. What comes next is therefore
    /// not a stimulus and not a capture — it is the barrier, which is where this crate keeps the
    /// one door onto a blocked pane.
    ///
    /// ⚠⚠ **A [`Question`], NOT an [`Unanswered`](crate::consent::Unanswered).** `Unanswered` is a
    /// REFUSAL — it says which consent failed to cover this dialog and what that cost — and
    /// refusing is the barrier's act, taken with the caller's clauses in hand. A turn's END has
    /// consulted no clause and typed nothing, so a refusal built here would be a sentence about a
    /// decision nobody made. It reports the question; the barrier decides about it.
    ///
    /// ⚠ The inner [`None`] is [`AgentObservation::asking`](crate::access::AgentObservation::asking)'s
    /// own — *this host cannot read the question as a menu* — and it does not collapse into the
    /// outer absence: one says the turn ended in a question nobody can parse, the other says the
    /// turn did not end in a question at all.
    Asking(Option<Question>),
    /// ⚠⚠⚠⚠ **THE PEER'S PROGRAM HAS EXITED, SO THIS TURN HAS NOBODY LEFT TO END IT.**
    ///
    /// # ⚠⚠⚠ Why a turn's end needed this word, measured
    ///
    /// At one pane, at one instant, the two contracts answered opposite things.
    /// [`DoneWhen::Exits`] asks [`PaneAccess::pane_eof`] and
    /// calls a dead child's pane **over**. [`DoneWhen::Settles`] — the contract an agent loop makes
    /// load-bearing — asks a SUPERVISOR whether the agent came back to rest, and **a process that
    /// is gone is reported by nobody**, so the answer was never *yes*: it was never given. **A dead
    /// agent and a thinking one were the same picture to that rule** (register item 323), which
    /// meant every pass burnt the whole per-turn bound waiting for evidence that cannot arrive and
    /// the run said nothing was wrong until its own clock ended it. Shipped, that bound is half an
    /// hour.
    ///
    /// ⚠⚠⚠ **AND THE ONE-LINE FIX WAS WRONG, WHICH IS WHAT MADE THIS A WORD RATHER THAN A GUARD.**
    /// Teaching the `Settles` arm to read `pane_eof` makes the product answer [`Yes`](Self::Yes) —
    /// *the peer answered on the evidence you named* — about a peer that died, and a loop told
    /// `Yes` walks on to judge a turn that never happened. There was no arm for the truth, so the
    /// mutation could only choose between two lies.
    ///
    /// # ⚠⚠ Why it is asked BEFORE [`Asking`](Self::Asking) and AFTER the caller's contract
    ///
    /// After the contract, because [`DoneWhen::Exits`] names an exit as the very evidence its turn
    /// ends on — a one-shot tool that answered and left is [`Yes`](Self::Yes), not this.
    ///
    /// Before the ask, because a process that is gone did not stop to ask, and a supervisor that
    /// says otherwise is reporting a cached reading of something that no longer exists. That is
    /// register item 329's hazard met at the one place this crate can see it.
    PeerGone(PaneId),
    /// ⚠⚠⚠⚠ **NOTHING HAS SPOKEN FOR THIS PANE FOR THE WHOLE OF THE CALLER'S [`Quiet`] BOUND** —
    /// the peer's process is still there and its reporter has stopped saying anything at all.
    ///
    /// # ⚠⚠⚠ Why this is not [`NotYet`](Self::NotYet), measured (register item 458)
    ///
    /// `NotYet` is *the bound ran out and the peer is still working*, and a caller acts on it by
    /// waiting again. That reading was applied to a turn a person had stopped with Escape, and it
    /// was wrong in a way nothing could see: the agent restores the prompt INTO ITS COMPOSER and
    /// suppresses its own idle nag while the composer holds text, so **no payload of any kind ever
    /// arrives again**. Measured 2026-08-19: the pane read `working seq=6 asked=2 said=0` for
    /// fourteen minutes, and the loop watching it would have gone on re-waiting toward a
    /// `max_seconds` the shipped kind authors at TWENTY-FOUR HOURS. Both incidents that day were
    /// ended by a person typing at the pane, never by the product.
    ///
    /// ⚠⚠ **AND IT IS NOT [`PeerGone`](Self::PeerGone) EITHER, which is the distinction worth the
    /// arm.** A gone peer is a fact that cannot change back, so its state is terminal. A silent one
    /// may speak on its very next tool call — the reporter may simply have been REPLACED under a
    /// running daemon, which the wire's own `build` key calls *"the ORDINARY state after any
    /// rebuild"*. So this ending's remedy is a PERSON, not an ending: `ai_loop.scxml` routes it to
    /// `awaiting_human`, which a returning turn leaves by `turn.done` as if nothing had happened.
    ///
    /// ⚠ **A PANE NOBODY REPORTS FOR CAN NEVER ANSWER THIS**, by construction: the count is read
    /// only where [`Authority::is_exact`](crate::access::Authority::is_exact) says the answer came
    /// from inside the pane. *This pane has no reporter to be silent* is not silence, and a rule
    /// that read it as silence would call every screen-inferred pane dead.
    Silent(Silence),
    /// The bound ran out and neither happened — the peer is still working, or was never listening.
    ///
    /// ⚠ [`Waited::TimedOut`] under its old name, and it now means what it says. Every ending it
    /// used to cover that was NOT *the peer did not finish* has a word of its own above.
    NotYet,
    /// **THE RUN ended underneath** — cancelled, or out of time.
    ///
    /// Not this wait's business to interpret: every caller hands it back to the driver's loop top,
    /// because only that knows whether it was a cancel or the duration ceiling.
    RunEnded,
}

/// WHICH EVIDENCE says a peer's turn is over.
///
/// Two kinds, and they are not degrees of one another: a ONE-SHOT tool's turn ends when the process
/// does, and a LONG-LIVED peer's ends when it goes back to waiting. Nothing about a pane says which
/// it is — **only the caller knows**, which is [`ReadyWhen`](crate::readiness::ReadyWhen)'s reason
/// for existing, met at the other end of the same turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoneWhen {
    /// Over once the pane's CHILD HAS EXITED — the pseudoterminal reached end-of-file.
    ///
    /// The right rule for a ONE-SHOT tool (`claude -p`), which answers and leaves: its exit is
    /// what makes the capture complete, so nothing on screen can be a half-written reply.
    ///
    /// ⚠ It is the WRONG rule for a long-lived peer, which never exits — see the module doc. Use
    /// [`Settles`](Self::Settles) for those.
    Exits,
    /// Over once the AGENT THIS TURN WAS ADDRESSED TO is at rest again, **having been seen to move
    /// first**.
    ///
    /// The rule for a long-lived interactive peer — an agent CLI that answers and goes on waiting.
    /// It is the same observation [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles)
    /// clears the barrier on, asked at the other end of the turn.
    ///
    /// # ⚠⚠⚠ Why stillness alone is NOT the answer
    ///
    /// A peer that is waiting for a prompt is *at rest*. Ask *"is it at rest?"* right after typing
    /// one and the honest answer is YES — it has not started yet. A rule built on the state alone
    /// therefore reports every turn complete in milliseconds, and what gets captured and published
    /// AS THE MODEL'S ANSWER is the screen before the model wrote a word.
    ///
    /// That is [`ReadyWhen::Prints`](crate::readiness::ReadyWhen::Prints)' hazard exactly — a
    /// condition satisfied by what was already true when the turn began — and it is answered the
    /// same way: by ARMING. [`AgentObservation::seq`](crate::access::AgentObservation::seq) counts
    /// published changes and never decreases, so this contract holds only once the pane's state has
    /// MOVED past what it was when the turn started. Its own doc says it is for exactly this
    /// comparison.
    ///
    /// # ⚠⚠⚠⚠ And why moving is not enough either — the rest must belong to THIS question
    ///
    /// A prompt typed at a peer that is still working on the last one is reported `working` into a
    /// pane that is already `working`: same verdict, nothing published, `seq` unchanged. The
    /// EARLIER work's rest then arrives with a fresh `seq` and satisfies every term above, so the
    /// turn ends before the peer has written a word of its answer — measured live at thirty-three
    /// turns and 6,604 bytes, deaf to a marker the agent printed every time (register item 441).
    /// [`AgentObservation::asked_seq`](crate::access::AgentObservation::asked_seq) is the fact that
    /// separates them, and this rule requires it to have MOVED.
    ///
    /// ⚠⚠⚠⚠⚠ **BUT ONLY OF A PANE THAT CAN STATE ONE.** That counter advances where the agent
    /// REPORTS being asked; a pane recognised from its SCREEN reports nothing, so demanding it
    /// there refuses that pane for ever — a live turn answered in one second was still `NotYet`
    /// at its 183-second bound. So the pairing is asked only where the ending is the agent's own
    /// statement ([`Authority::is_exact`](crate::access::Authority::is_exact)), and a scraped rest
    /// is judged on the terms above. ⚠ Named as the degradation it is: a screen-read rest cannot
    /// be told from one belonging to earlier work.
    ///
    /// # ⚠ What it deliberately does NOT complete on
    ///
    /// * [`AgentState::Blocked`] — the peer stopped because it ASKED something. The turn did end,
    ///   but *"here is your answer"* and *"answer my question first"* are opposite instructions,
    ///   and a loop that read them the same way would submit its next stimulus INTO a menu. Waiting
    ///   the timeout out is the honest answer until a run can report the question, which is a
    ///   decision about the step vocabulary and not about this rule.
    /// * An observation naming a DIFFERENT agent than the one the turn was addressed to, or naming
    ///   none — that is not evidence about this peer.
    /// * A host with no supervisor at all, which simply never satisfies this and times out. Every
    ///   one of these fails in the direction that WAITS, because the other direction publishes a
    ///   fragment as a model's reply.
    ///
    /// # ⚠⚠ Why it does NOT name the agent, where [`ReadyWhen::Settles`] must
    ///
    /// The barrier at the other end is deciding *is the right program up yet*, so the caller has to
    /// say which program they meant. By the time a turn ENDS that question is settled: the prompt
    /// went to whatever was in the pane, and [`Completion::begin`] read WHICH AGENT that was. So
    /// the name is taken from the observation the turn was actually addressed to rather than from
    /// the caller — which is both one less argument and a stricter check, because a caller's name
    /// can be right about the pane while being the wrong pane.
    ///
    /// [`ReadyWhen::Settles`]: crate::readiness::ReadyWhen::Settles
    Settles,
}

impl DoneWhen {
    /// The words a caller may spell, in this type's own order.
    ///
    /// Published to every mouth from here rather than retyped as literals, so a third kind reaches
    /// the wire in the compile that adds it — [`ReadyWhen::WIRE_WORDS`](crate::readiness::ReadyWhen::WIRE_WORDS)'
    /// rule, which this vocabulary is the twin of.
    pub const WIRE_WORDS: &'static [&'static str] = &["exits", "settles"];

    /// The word this kind is spelled as on the wire.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Exits => "exits",
            Self::Settles => "settles",
        }
    }

    /// The contract named by `word`, or `None` for a word outside the closed set.
    ///
    /// ⚠ A caller who sends something else has made a MALFORMED request, not a rejected one —
    /// R353's rule, and the reason this returns an `Option` for the parser to turn into the wire's
    /// own grammar refusal rather than a friendly sentence.
    ///
    /// ⚠ A BARE WORD, and that is what the published vocabulary and the parser agreeing looks
    /// like: every word here is one this surface accepts ALONE. A kind that needed a companion
    /// argument would be a word the wire advertises and the daemon refuses — which is precisely
    /// what `every_published_word_is_a_word_the_plugin_host_accepts` caught on this argument's
    /// first draft.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Every kind, so the published vocabulary and the parser are read off ONE list.
    const ALL: [Self; 2] = [Self::Exits, Self::Settles];
}

/// **HOW A LOOPING RUN'S PEER FINISHES A TURN, AND HOW LONG IT MAY TAKE** — the two together,
/// because they are one decision and neither half means anything alone.
///
/// # ⚠⚠⚠ What this is for, measured
///
/// A step that has typed its stimulus has to decide when to stop waiting, and
/// [`Orchestrator`](crate::orchestrator::Orchestrator) decided it with a 500 ms constant. Against a
/// peer that answers inside that, fine — and every fixture in this crate was one. **A real agent
/// session is not.** Measured against a peer that thinks for three seconds: the run spent **six
/// turns on one question**, typing the stimulus again twice a second at a peer that was still
/// answering the first. Scaled to a `claude` turn of half a minute that is sixty prompts, and each
/// one is a turn of that agent's own bounded budget spent re-answering.
///
/// [`Agent`](crate::agent::Agent) never had the defect, because it asks a [`DoneWhen`] instead of a
/// clock. This is that contract offered to the looping plugins, which are the ones the MCP
/// `orchestrate` verb and the outer AI loop drive.
///
/// # ⚠⚠⚠ Why the bound lives INSIDE the contract, and why it is OPTIONAL
///
/// [`Attended::APerson`](crate::readiness::Attended::APerson)'s shape: a bound with no contract is
/// meaningless — *"wait this long and then type at it anyway"* is the timer this type exists to
/// replace, with a bigger number — so it cannot be spelled alone.
///
/// ⚠⚠⚠ **THE OTHER DIRECTION IS DIFFERENT, AND A GATE HAD TO TEACH IT.** The first draft required
/// both, and `every_published_word_is_a_word_the_plugin_host_accepts` refused it: the wire
/// publishes `done_when`'s two words, so a caller who enumerates the vocabulary sends the word
/// ALONE and must get a run rather than a refusal. **This is the second time that gate has caught
/// this exact argument** — the first was `done_when`'s own companion, at version 25. So a contract
/// with no bound is legal and means what it says: *wait for my peer to finish*, bounded by the
/// run's own clock and its cancel, which are the bounds every run already has.
///
/// ⚠ It does NOT stamp a number. How long a turn may take is the caller's to say for
/// [`Handback`](crate::readiness::Handback)'s reason one door over: a shell command is a second and
/// an agent asked to read a repository is minutes, and only the caller knows which they have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// Which evidence ends the turn.
    when: DoneWhen,
    /// A per-turn bound tighter than the run's own, or `None` for the run's alone.
    within: Option<Duration>,
}

impl Turn {
    /// The argument the BOUND is declared with, in ONE place — [`Attended::WIRE_KEY`]'s rule, so
    /// the daemon's parser, the published grammar and both mouths cannot drift apart.
    ///
    /// ⚠ Its companion is `done_when`, which is [`DoneWhen`]'s own word and was already on this
    /// wire for the `agent` form. This adds the bound, not the vocabulary.
    ///
    /// [`Attended::WIRE_KEY`]: crate::readiness::Attended::WIRE_KEY
    pub const WIRE_KEY: &'static str = "turn_within_ms";

    /// A turn that ends on `when`, waited for at most `within` — or for the RUN's own clock when
    /// that is [`None`]. Answers [`None`] itself for a bound of zero.
    ///
    /// ⚠ Zero is REFUSED for [`Attended::of`](crate::readiness::Attended::of)'s reason: *"wait no
    /// time at all for my peer to finish"* is not a thing a caller can mean, and the one who
    /// reaches zero by arithmetic — a config default, a deadline already spent — is exactly the one
    /// who needs telling rather than a run that goes straight back to typing.
    #[must_use]
    pub const fn lasting(when: DoneWhen, within: Option<Duration>) -> Option<Self> {
        if let Some(bound) = within
            && bound.is_zero()
        {
            return None;
        }
        Some(Self { when, within })
    }

    /// Which evidence ends the turn.
    #[must_use]
    pub const fn when(&self) -> DoneWhen {
        self.when
    }

    /// The per-turn bound, or [`None`] when the run's own clock is the only one.
    #[must_use]
    pub const fn within(&self) -> Option<Duration> {
        self.within
    }
}

/// **HOW LONG NOTHING MAY SPEAK FOR A PANE before a wait stops calling its peer *thinking*** —
/// register item 458's ceiling, and the answer to *which of the two was it*.
///
/// # ⚠⚠⚠⚠⚠ Why a bound of its own, beside [`Turn`]'s and not folded into it
///
/// They measure different things and a caller wrong about one is not wrong about the other.
/// `turn_within_ms` bounds HOW LONG A TURN MAY TAKE — thirty minutes, shipped — and its own doc says
/// the honest direction to be wrong in is LONG, because a bound too small makes the loop judge a
/// turn that had not finished. This bounds HOW LONG A TURN MAY BE SILENT, which a turn that is
/// genuinely working breaks every time it calls a tool. **A turn can legitimately run for the whole
/// half hour and must never be silent for ten minutes of it.** One number cannot say both.
///
/// # ⚠⚠⚠ Why it is the DOCUMENT's and not a constant here
///
/// How patient to be with a quiet peer is a judgement about the work — `reflect_after_refusals`'
/// argument exactly, and item 314's line between a binding and a judgement. Nothing about `claude`
/// says ten minutes. So the number is a `<data>` in `ai_loop.scxml`, a kind may override it in its
/// own document, and this type carries only the caller's answer to the wait.
///
/// ⚠⚠ **A BOUND AT OR ABOVE THE TURN'S DECIDES NOTHING**, and that is a real authoring rather than a
/// defect to guard against: the wait ends at [`Over::NotYet`] first, so a document whose quiet bound
/// is the larger has said *silence is not a thing I want noticed*. The template's two numbers are
/// ten minutes inside thirty, and the gate below holds both readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quiet(Duration);

impl Quiet {
    /// The `<data>` this bound is authored under, in ONE place — [`Turn::WIRE_KEY`]'s rule, so the
    /// document, the driver's reader and this type cannot drift apart.
    ///
    /// ⚠ **`DOCUMENT_KEY` AND NOT `WIRE_KEY`, WHICH IS THE HONEST HALF OF THE NAME.** Its four
    /// neighbours are spelled `WIRE_KEY` because a caller can send them; this one is authored in the
    /// `.scxml` and nowhere else, so a name promising a wire argument would send a reader looking
    /// for a key no form serves. The day a caller needs to override it, the `Brief` gains a field
    /// and this constant gains the second reader — not before.
    pub const DOCUMENT_KEY: &'static str = "quiet_within_ms";

    /// A silence bound of `within`, or [`None`] for a bound of zero.
    ///
    /// ⚠ Zero is REFUSED on [`Turn::lasting`]'s reason, one door over: *"call my peer silent the
    /// instant I stop looking"* is not a thing a caller can mean, and the author who reaches zero by
    /// editing — a document that meant to decline — is told through the [`None`] rather than given a
    /// loop that hands every turn to a person on its first poll.
    #[must_use]
    pub const fn of(within: Duration) -> Option<Self> {
        if within.is_zero() {
            return None;
        }
        Some(Self(within))
    }

    /// How long nothing may speak.
    #[must_use]
    pub const fn within(&self) -> Duration {
        self.0
    }
}

/// **WHAT NOTHING SPEAKING FOR A PANE LOOKED LIKE** — the evidence [`Over::Silent`] carries.
///
/// Both numbers, because either alone is unreadable to whoever has to act on it: *nothing has
/// spoken for ten minutes* wants to be read beside *and this pane HAS a reporter, which had
/// spoken six times* — the second is what separates a stalled agent from a pane nobody was ever
/// instrumented for, and a reader given only the duration would have to go and ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Silence {
    /// The report count that stood still for the whole bound — see
    /// [`AgentObservation::reports`](crate::access::AgentObservation::reports).
    pub reports: u64,
    /// How long nothing spoke — the caller's [`Quiet`] bound, restated so a consumer needs neither
    /// the document nor a clock of its own to say the sentence.
    pub within: Duration,
}

/// The poll-loop state behind [`Over::Silent`]: the bound, the last count anything spoke, and WHEN
/// it did.
///
/// # ⚠⚠⚠ Why a watermark of its own rather than an age read off the pane
///
/// [`AgentObservation::reports`](crate::access::AgentObservation::reports) is deliberately a COUNT
/// and not an instant — *"the tracker keeps no reader state, so several waiters can each ask since I
/// last looked without coordinating"*. So the clock belongs to the waiter, which is this, and two
/// waits over one pane cannot spoil each other's answer.
#[derive(Debug)]
struct Listening {
    /// How long nothing may speak.
    within: Duration,
    /// The count at the last look that CHANGED it, or [`None`] while nothing has answered at all.
    heard: Option<u64>,
    /// When that look happened — the anchor the bound is measured from.
    since: Instant,
}

impl Listening {
    /// A listener for `bound`, anchored now — which is the top of the wait, and the right anchor:
    /// the prompt that opened this turn is itself a report, so a turn begins having just been
    /// spoken for.
    fn for_(bound: Quiet) -> Self {
        Self {
            within: bound.within(),
            heard: None,
            since: Instant::now(),
        }
    }

    /// **HAS NOTHING SPOKEN FOR THE WHOLE BOUND?** — fed whatever [`Stands::spoken`] carried.
    ///
    /// ⚠⚠ [`None`] RESTARTS THE ANCHOR AND ANSWERS NO, which is the arm that keeps a scraped pane
    /// out of this for ever. It is not *nothing spoke*; it is *nothing here can be silent*, and the
    /// two must not collapse — a pane whose reporter was RELEASED mid-turn goes back to being one of
    /// those, and calling that silence would publish a fact about a reporter that no longer exists.
    fn silent(&mut self, spoken: Option<u64>) -> bool {
        let Some(count) = spoken else {
            self.heard = None;
            self.since = Instant::now();
            return false;
        };
        if self.heard != Some(count) {
            self.heard = Some(count);
            self.since = Instant::now();
        }
        self.since.elapsed() >= self.within
    }

    /// **WHEN THIS LISTENER'S SILENCE FALLS DUE** — the instant [`silent`](Self::silent) turns true
    /// if nothing speaks before it, and register item 629's whole repair.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a wait with a silence bound could not park at all
    ///
    /// [`Over::Silent`] is a CLOCK predicate: it becomes true with no output whatever — that is
    /// what silence IS — so a wait that parked on the pane would never wake to notice it, and
    /// [`Over::Silent`] would be unreachable for exactly the peers it exists to catch. The wait
    /// therefore handed its WHOLE bound over as a lag and degraded to
    /// [`poll_until`](crate::run::poll_until) by name: a `quiet_within_ms` of ten minutes cost
    /// ~60,000 screen reads at a pane nobody was going to hear from.
    ///
    /// The fix is not a cleverer cadence. **This listener has always known the instant** — it is
    /// [`since`](Self::since) plus [`within`](Self::within), and nothing else — so it says so, and
    /// [`park_until`] parks to it. Same shape as the supervisor's
    /// [`AgentObservation::settling`](crate::access::AgentObservation::settling) (register item
    /// 630), different clock, which is why they are two entries and one mechanism.
    ///
    /// ⚠ It MOVES whenever [`silent`](Self::silent) hears something, because the anchor does. A
    /// caller that cached this would be waiting on a bound that expired for a peer which has since
    /// spoken — which is why it is asked at every look rather than once.
    fn due(&self) -> Instant {
        self.since + self.within
    }

    /// What this listener heard, for the ending it is about to publish.
    fn silence(&self) -> Silence {
        Silence {
            reports: self.heard.unwrap_or(0),
            within: self.within,
        }
    }
}

/// The per-turn evaluator of a [`DoneWhen`] — the contract plus whatever the turn has to REMEMBER
/// for it.
///
/// Mirrors [`Readiness`](crate::readiness::Readiness), which exists for the same reason at the
/// other end: some conditions are not predicates over the present. [`DoneWhen::Settles`] has to
/// know what the peer's state was when the turn began, and a bare `match` on the enum could not.
///
/// ⚠ ONE door, even though [`DoneWhen::Exits`] needs none of this. Two entry points — a plain
/// predicate for the stateless rules and an evaluator for the rest — would be two ways to ask one
/// question, which is the shape this crate keeps finding defects in; and a caller that reached for
/// the cheap door with a rule that needed the other would get a contract that is silently never
/// satisfied.
#[derive(Clone, Debug)]
pub struct Completion {
    /// Which evidence ends the turn.
    when: DoneWhen,
    /// WHO the turn was addressed to and how many published changes their pane had been through,
    /// read when the turn BEGAN.
    ///
    /// `None` until [`begin`](Self::begin) arms it, and `None` after it where the host has no
    /// supervisor to ask or no agent was identified — which is why [`DoneWhen::Settles`] treats an
    /// unarmed evaluator as never satisfied rather than as immediately satisfied.
    ///
    /// ⚠ The NAME is armed, not taken from the caller: see [`DoneWhen::Settles`]. It is what makes
    /// *"the agent came back to rest"* a claim about the peer this turn was actually given to.
    ///
    /// ⚠⚠ The two numbers are the pane's [`seq`](crate::access::AgentObservation::seq) and
    /// [`asked_seq`](crate::access::AgentObservation::asked_seq) at arming time —
    /// see [`DoneWhen::Settles`] for why the second one had to be added and what the first alone
    /// let through.
    addressed: Option<Addressed>,
}

/// **WHAT ONE LOOK AT A PANE SAID ABOUT THE TURN RUNNING IN IT** — the answer
/// [`Completion::stands`] gives, and the reason a round of this contract reads its pane ONCE.
///
/// # ⚠⚠⚠⚠⚠ Three answers from one reading, and why they had to travel together — item 637
///
/// A waiting round needs all three: *has the turn ended*, *when could that answer change with the
/// pane producing nothing*, and *has anything spoken for this pane*. Each used to have a door of
/// its own and each door read the supervisor, so one round cost four reads — a round trip each over
/// the wire, a workspace lock and a detector run each in-process.
///
/// ⚠⚠ And they were four MOMENTS, which is the part a cost gate cannot see. The deadline could
/// belong to an observation older than the ending it is paired with, and the ending to one older
/// than the silence; nothing said which instant the round was about. Carried together they are one
/// instant by construction.
///
/// ⚠ It is deliberately not [`Copy`]: [`Over::Asking`] carries a parsed question, and a type a
/// caller can cheaply duplicate is one a caller will read twice believing it is fresh.
#[derive(Debug)]
pub(crate) struct Stands {
    /// **HOW THE TURN ENDED**, or [`None`] while it is still running.
    pub(crate) over: Option<Over>,
    /// **WHEN THIS ANSWER CAN CHANGE WITH THE PANE PRODUCING NOTHING AT ALL** — the supervisor's
    /// own deadline, and [`Settling::Nothing`](crate::access::Settling::Nothing) where there is no
    /// supervisor to ask.
    ///
    /// # ⚠⚠⚠ Why an ABSENT supervisor is `Nothing` here and not `Unknown`
    ///
    /// [`Settling`](crate::access::Settling)'s whole argument is that *nothing is pending* and
    /// *this build cannot say* must not share a word — and this fold is the one place the two are
    /// genuinely the same, for a reason about the CALLER rather than about the surface. `Nothing`
    /// means [`Look::Steady`], *park on the pane*, and a host with no
    /// supervisor has no settling verdict for that park to sleep through: everything
    /// [`over`](Self::over) can then answer rests on the screen and on end-of-file, both of which
    /// move the pane.
    ///
    /// ⚠⚠ EVERYTHING THIS CONTRACT WAITS ON RESTS ON EITHER THE VERDICT OR THE PANE —
    /// `satisfied_of` pairs published counters, `asked_of` reads the published question, the
    /// gone-peer check reads the child. So one deadline covers the contract, and a term added to
    /// [`over`](Self::over) with a clock of its own would have to be answered here in the same
    /// edit.
    pub(crate) settles: crate::access::Settling,
    /// **HOW MANY TIMES ANYTHING HAS SPOKEN FOR THIS PANE**, or [`None`] where nothing here CAN be
    /// silent — no supervisor, no observation, or an observation nothing reported.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the absence is a third answer and not a zero
    ///
    /// [`AgentObservation::reports`](crate::access::AgentObservation::reports) answers `0` for a
    /// pane read off its SCREEN, and its own doc says what that means: *"nothing reported it, which
    /// is not the same as nothing speaking — what it means is this pane has no reporter to be
    /// silent"*. A rule that read `0` as silence would hand every screen-inferred pane to a person
    /// ten minutes into its first turn.
    ///
    /// ⚠⚠ So the arming is [`Authority::is_exact`](crate::access::Authority::is_exact) — the same
    /// published question the settle arm's counter pairing is armed on, for the same reason at the
    /// other end: *did this answer come from the pane itself*. Register item 441 is the round that
    /// learned the cost of asking a scraped pane for a number only a reported one has, and this is
    /// that lesson applied before the fact rather than after.
    pub(crate) spoken: Option<u64>,
}

/// WHO this turn was given to, and what the pane's two counters read when it was.
///
/// A named struct rather than a widening tuple: the third field is a SECOND counter beside the
/// first, and a reader at the comparison site has no way to tell two adjacent `u64`s apart by
/// position — which is exactly the confusion the one this fixes was made of.
#[derive(Debug, Clone)]
struct Addressed {
    /// The agent the prompt was typed at, by the name the pane published.
    agent: String,
    /// The pane's published-verdict counter when the turn was armed.
    seq: u64,
    /// The pane's QUESTION counter when the turn was armed — see
    /// [`AgentObservation::asked_seq`](crate::access::AgentObservation::asked_seq).
    asked_seq: u64,
}

impl Completion {
    /// An evaluator for `when`, not yet armed.
    #[must_use]
    pub const fn new(when: DoneWhen) -> Self {
        Self {
            when,
            addressed: None,
        }
    }

    /// Arm it — **called where the turn BEGINS, before a byte is injected**.
    ///
    /// ⚠ The ordering is the whole guarantee. Armed after the injection, the peer may already have
    /// started and stopped, and the change this looks for would be one the turn missed; armed
    /// before, the comparison is against the state the turn was addressed to.
    pub fn begin(&mut self, panes: &dyn PaneAccess, pane: PaneId) {
        self.addressed = panes
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(pane).seen())
            .and_then(|seen| {
                seen.agent.map(|agent| Addressed {
                    agent,
                    seq: seen.seq,
                    asked_seq: seen.asked_seq,
                })
            });
    }

    /// **HOW THIS TURN STANDS AFTER ONE LOOK AT ITS PANE** — the door a round of this contract goes
    /// through, and the ONE reading everything it answers is derived from.
    ///
    /// # ⚠⚠⚠⚠⚠ Why one call answers three questions — register item 637
    ///
    /// The three used to be three doors, and a round that asked all of them read the supervisor
    /// FOUR times: once for the deadline, once for the contract, once for the ask, once for the
    /// silence. Over
    /// [`RemotePaneAccess`](../../sprag_host/remote_access/struct.RemotePaneAccess.html) each of
    /// those is a ROUND TRIP; in-process each takes the workspace lock and runs the detector. So a
    /// wait that item 630 had already made park to a published instant still paid four reads for
    /// every look it took, and the constant was invisible to every gate here because the gates
    /// measure the SLOPE — *does the cost follow the clock* — and four is not a slope.
    ///
    /// ⚠⚠ **THE COUNT WAS THE CHEAP HALF.** The one that could go wrong silently is COHERENCE:
    /// four reads are four moments, so a round could hold a deadline belonging to one observation,
    /// an ending belonging to a second and a silence belonging to a third, with nothing saying
    /// which moment the round was about. The old code leaned on that — it read the deadline FIRST
    /// and argued that *a candidate publishing between the two reads leaves a deadline already
    /// past, which buys one more look*. That argument existed only because there were two reads.
    /// **With one there is no between**, and the ordering it defended is not a thing a future edit
    /// can get wrong.
    ///
    /// ⚠ VISIBLE TO THE CRATE, and that is not a second door. [`wait`](Self::wait) IS
    /// `park_until(this)`, so a caller who needs this contract as one term of a LARGER predicate —
    /// a step that stops either when its peer's turn is over or when the sentinel it named appears
    /// — cannot express it through the wait without running two waits in sequence and making the
    /// first one's bound a lie. One predicate, composed once, is what
    /// [`Orchestrator`](crate::orchestrator::Orchestrator) does with it.
    pub(crate) fn stands(&self, panes: &dyn PaneAccess, pane: PaneId) -> Stands {
        // ⚠⚠⚠⚠⚠ THE ONE READING. Every term below is a function of these two values and of the
        // caller's contract — nothing under this line asks the pane anything.
        let seen = panes
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(pane).seen());
        // ⚠⚠ READ UNCONDITIONALLY, where the old code reached it only after `satisfied` said no.
        // That saved one call on the round a turn ENDS — one per wait, not one per look — and cost
        // the `DoneWhen::Exits` arm a second read on every other round, because `satisfied` and the
        // gone-peer check each asked. Read once, both arms are one call and both see one moment.
        let eof = panes.pane_eof(pane);
        Stands {
            over: self.ended_of(pane, seen.as_ref(), eof),
            settles: Self::settles_of(seen.as_ref()),
            spoken: Self::spoken_of(seen.as_ref()),
        }
    }

    /// **HOW THIS TURN STANDS**, decided from the reading [`stands`](Self::stands) took — [`None`]
    /// while it is still running.
    ///
    /// ⚠ `pane` is carried for [`Over::PeerGone`] to NAME, and is not a second address to ask
    /// anything at: nothing below this line reads a pane.
    fn ended_of(
        &self,
        pane: PaneId,
        seen: Option<&AgentObservation>,
        eof: Option<bool>,
    ) -> Option<Over> {
        // ⚠ THE CONTRACT IS ASKED FIRST. Where both could be true — a peer that asked and whose
        // pane then reached end-of-file — the evidence the CALLER named is the stronger answer:
        // the turn is over on the terms they chose and the capture is whole. The ask is what ends
        // a turn the contract CANNOT end, and asking it second is what keeps it to that job.
        //
        if self.satisfied_of(seen, eof) {
            return Some(Over::Yes);
        }
        // ⚠⚠⚠⚠ AND THEN: THE PEER MAY BE GONE, which no contract left can end a turn on. See
        // [`Over::PeerGone`], which carries what it cost not to have this word.
        //
        // ⚠⚠⚠ **SECOND, AND THE ORDER IS THE DECISION.** A peer that ANSWERED AND THEN LEFT is one
        // instant where both readings are true, and putting this first loses the answer: measured
        // on the first run of exactly that ordering, `Agent`'s own gate over a peer which prints
        // its reply and exits went from `converged` with the reply to `failed` with the pane. Under
        // [`DoneWhen::Exits`] the caller NAMED the exit as their evidence, so it is `Yes` and a
        // whole capture; under [`Settles`](DoneWhen::Settles) the agent came back to rest and
        // published a change, which is a completed turn whatever happened to the process next.
        //
        // ⚠⚠⚠ **WHAT THAT ORDER GIVES UP, AND WHY IT IS AFFORDABLE NOW.** `Settles` asks a
        // SUPERVISOR, and this host's own detector derives rest from the SCREEN — which a dead pane
        // still shows, frozen. A supervisor that kept publishing rising `seq` values about a gone
        // process could therefore end a turn nothing ran (register item 329, found by mutating a
        // fixture into exactly that). Today's detector cannot: a frozen screen publishes no change,
        // so `seq` does not rise and `satisfied` is false. And the residue is BOUNDED rather than
        // argued away — a loop fooled that way judges one turn and then tries to type, and
        // [`PaneAccess::inject`] refuses at a pane whose child has exited. **It is the door that
        // holds item 329, not this ordering.**
        //
        // ⚠⚠ BEFORE THE ASK, because a process that has left did not stop to ask: a supervisor
        // still reporting a dialog at a dead pane is describing a screen rather than a peer, and
        // `Asking` sends a reader to answer a question nobody is waiting on.
        //
        // ⚠⚠ `Some(true)` STRICTLY, where `satisfied`'s `Exits` arm reads an unknown pane as over.
        // The two absences are different sentences: *this pane is not mine to ask about* is not
        // *this pane's program has exited*, and only the second is a fact about a peer. It is the
        // same reading [`PaneAccess::inject`] refuses on, at the other end of the same turn.
        if eof == Some(true) {
            return Some(Over::PeerGone(pane));
        }
        self.asked_of(seen).map(Over::Asking)
    }

    /// The question THIS TURN's peer raised, or [`None`] where it raised none.
    ///
    /// # ⚠⚠⚠ Why the ask is ARMED, exactly as [`DoneWhen::Settles`] is
    ///
    /// A supervisor's verdict SETTLES: a real detector goes on calling a pane blocked for its
    /// hysteresis window after the dialog has left the screen — [`Arrival`] measured that end to
    /// end through a live daemon, and no fixture here models it, because a fixture derives its
    /// state from the screen and so has no lag.
    ///
    /// Read as a bare predicate, then, *"is it blocked?"* asked right after a stimulus can be
    /// answered YES by a question that was already gone before this turn started — and the turn
    /// would end on a dialog nobody is looking at. That is precisely the hazard this module exists
    /// for, one door along: **a state left over from before the turn is not this turn's answer.**
    ///
    /// So it takes the same three-part evidence the settle arm does — the pane's
    /// agent is the one the turn was ADDRESSED to, and its state has MOVED past what it was when
    /// the turn began. An unarmed evaluator can claim nothing, which is why this answers `None`
    /// there rather than reading the pane fresh.
    ///
    /// ⚠ Asked WHATEVER THE CONTRACT IS, and not only under [`Settles`](DoneWhen::Settles). A
    /// one-shot peer that has stopped to ask will not exit either, so *wait for the child to
    /// leave* is exactly as futile there; the rule is about what the peer is doing, not about
    /// which evidence the caller chose to end on.
    ///
    /// [`Arrival`]: crate::consent
    /// ⚠⚠⚠⚠ **AND IT DELIBERATELY DOES NOT REQUIRE THE QUESTION COUNTER**, where
    /// the settle arm does — register item 441, and the asymmetry is a
    /// decision rather than an oversight.
    ///
    /// A peer that is BLOCKED may be blocked on something it was asked BEFORE this turn — indeed a
    /// peer sitting at a dialog cannot take the prompt just typed at it, so its question counter
    /// will never move. Demanding that it move would mean a blocked peer is never noticed, and the
    /// run would wait out its whole patience on a pane whose screen is asking a person something.
    /// **That is the fail-DANGEROUS direction**: the settle arm's guard exists to stop a rest being
    /// mistaken for an answer, and there is no equivalent harm in noticing a block early.
    fn asked_of(&self, seen: Option<&AgentObservation>) -> Option<Option<Question>> {
        let addressed = self.addressed.as_ref()?;
        let seen = seen?;
        (seen.state == AgentState::Blocked
            && seen.agent.as_deref() == Some(addressed.agent.as_str())
            && seen.seq > addressed.seq)
            .then(|| seen.asking.clone())
    }

    /// Whether the reading [`stands`](Self::stands) took satisfies this contract.
    fn satisfied_of(&self, seen: Option<&AgentObservation>, eof: Option<bool>) -> bool {
        match &self.when {
            // ⚠ An UNKNOWN pane counts as over. A rule that answered "not yet" for a pane that is
            // not there would spin to the timeout on a question that can never be answered — and
            // both plugins that use this already spelled it `unwrap_or(true)`, which is the
            // behaviour this preserves exactly.
            DoneWhen::Exits => eof.unwrap_or(true),
            DoneWhen::Settles => {
                // Never armed, no supervisor to arm from, or no agent identified in the pane the
                // prompt went to — see `begin`. None of those is evidence that a turn ended.
                let Some(addressed) = &self.addressed else {
                    return false;
                };
                seen.is_some_and(|seen| {
                    // ⚠⚠ ALL FOUR, and the last one is what stops a peer's rest from reading as
                    // its answer to THIS question. See the variant's doc.
                    //
                    // ⚠⚠⚠⚠⚠ THE PAIRING IS ASKED ONLY OF A PANE THAT CAN ANSWER IT — register
                    // item 441, and this condition is the SECOND half of that item's cost.
                    // `asked_seq` advances where a REPORT states an `asked`; a pane read from
                    // its SCREEN states nothing, so the term is false there for ever and the
                    // turn never ends. Measured against a live agent: three lines, answered in
                    // a second, `Over::NotYet` still at the 183-second bound. `is_exact` is the
                    // published question for exactly this — *did this answer come from the pane
                    // itself* — and a scraped rest is judged on the three terms a scraped rest
                    // can support. ⚠ That is a DEGRADATION, named as one: a screen-read rest
                    // cannot be told from one belonging to earlier work. The alternative is not
                    // a stricter loop but one that never judges anything.
                    seen.state == AgentState::Idle
                        && seen.agent.as_deref() == Some(addressed.agent.as_str())
                        && seen.seq > addressed.seq
                        && (!seen.authority.is_exact() || seen.asked_seq > addressed.asked_seq)
                })
            }
        }
    }

    /// [`Stands::settles`], folded from the one reading — the ARGUMENT for the fold, including why
    /// an absent supervisor is `Nothing` and not `Unknown`, lives on that field.
    fn settles_of(seen: Option<&AgentObservation>) -> crate::access::Settling {
        seen.map_or(crate::access::Settling::Nothing, |seen| seen.settling)
    }

    /// [`Stands::spoken`], folded from the one reading — the ARGUMENT for the arming, including why
    /// the absence is a third answer rather than a zero, lives on that field.
    fn spoken_of(seen: Option<&AgentObservation>) -> Option<u64> {
        let seen = seen?;
        seen.authority.is_exact().then_some(seen.reports)
    }

    /// Wait for this turn to END, bounded by `within`, by `quiet` and by the RUN's own deadline —
    /// see [`Over`], which is what the endings are and why they are separate.
    ///
    /// # ⚠⚠⚠ Why `quiet` is a type and not a second [`Duration`]
    ///
    /// It would sit directly beside `within` at every call, and two adjacent `Duration`s at one call
    /// say nothing about which is which — the confusion [`AiLoopSpec`](crate::outer::AiLoopSpec)'s
    /// own doc records paying for (*"`OuterLoop::new(lua, pane, None, None, turn, false)` says
    /// nothing at all about which `None` is the barrier"*). **The type is the gate**: a caller
    /// cannot swap them, and a caller that asks nothing about silence says so in the type.
    ///
    /// ⚠ [`None`] is *do not ask*, and it is what three of this crate's four callers pass. It is not
    /// the same as a bound so large it never fires: this way [`Over::Silent`] is unreachable by
    /// construction rather than by arithmetic.
    pub fn wait(
        &self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        within: Duration,
        quiet: Option<Quiet>,
        run: &RunContext,
    ) -> Over {
        // ⚠ SEEDED WITH THE ANSWER A WAIT THAT NEVER FIRES DESERVES, so the read after the poll
        // needs no `expect` over an invariant held somewhere else. `poll_until` answers `Ready`
        // exactly when the closure did, and the closure only says so having written an ending
        // here.
        let mut ending = Over::NotYet;
        let mut listening = quiet.map(Listening::for_);
        // ⚠⚠⚠⚠⚠ **PARKED ON THE PANE, NOT POLLED AT IT** — register item 280. The predicate below
        // renders a screen and runs a detector over it, and asking it every
        // [`POLL_INTERVAL`](crate::run::POLL_INTERVAL) made the cost of a wait a function of the
        // CLOCK: measured at 98 screen reads a second, which over the loop's half-hour turn bound
        // is ~180,000 of them at a pane that said nothing.
        //
        // ⚠⚠⚠ **THE LAG IS THE SUPERVISOR'S AND IT IS NOT ZERO**, which is the whole reason
        // [`park_until`](crate::run::park_until) takes one. Everything this closure asks rests on a
        // published verdict — [`satisfied`](Self::satisfied)'s counter pairing,
        // [`asked`](Self::asked), [`spoken`](Self::spoken) — and a verdict SETTLES: the tracker
        // goes on reporting the old state for [`DEFAULT_SETTLE`](sprag_detect::DEFAULT_SETTLE)
        // after the screen stopped changing, then changes its answer with no further output. A wait
        // that parked on the bytes alone would sleep through exactly that.
        //
        // ⚠⚠⚠⚠⚠ **AND A SILENCE BOUND IS A CLOCK INSIDE THE PREDICATE — WHICH IS NOW A DEADLINE
        // THIS WAIT PARKS TO, NOT A REASON IT CANNOT.** Register items 629 and 630, paid together
        // because they are one shape. `Listening::silent` turns true `quiet` after the last report
        // with NO OUTPUT WHATEVER — that is what silence IS — so a wait that only watched the pane
        // would make [`Over::Silent`] unreachable for exactly the peers it exists to catch. The old
        // answer was to hand the whole bound over as a lag and degrade to
        // [`poll_until`](crate::run::poll_until) by name: a ten-minute `quiet_within_ms` cost
        // ~60,000 screen reads at a pane nobody was going to hear from.
        //
        // Both clocks now PUBLISH instead: `Listening::due` says when the silence falls due, and
        // the supervisor's `AgentObservation::settling` says when a
        // pending verdict changes with no further output. The look below hands
        // [`park_until`](crate::run::park_until) the EARLIER of the two, which is the one thing a
        // scalar lag could never express — it had to take the larger and poll the difference.
        let waited = park_until(run, panes, pane, within, || {
            // ⚠⚠⚠ THE CONTRACT IS ASKED FIRST. A turn that ENDED this poll ended — whatever its
            // reporter has been doing — and answering `Silent` about a peer that just came back to
            // rest would hand a finished turn to a person. The silence bound exists for the case
            // where NOTHING is going to answer, so it is consulted having found that nothing has.
            //
            // ⚠⚠⚠⚠⚠ THIS ORDER IS ARGUED AND NOT GATED, WHICH IS SAID HERE RATHER THAN LEFT TO BE
            // ASSUMED. A gate for it was written and DELETED, because writing it is what showed the
            // two conditions cannot be staged true together: the listener needs a prior look to
            // anchor from, so silence can only fall due on a poll where the last one found the turn
            // still running — and this wait RETURNS the moment either is true. The window in which
            // the order decides anything is one 10 ms poll wide, and a gate that has to hit it is a
            // flake rather than a measurement. ⚠ The residue, stated: a peer that goes quiet for the
            // whole bound and answers a second later is called silent, and the DOCUMENT is where
            // that costs nothing — `awaiting_human` leaves by `turn.done`, which is the gate that
            // was written instead.
            //
            // ⚠⚠ A test encoding a claim the product does not make is worse than none, so it is
            // gone rather than weakened until green.
            //
            // ⚠⚠⚠⚠⚠ **ONE READING OF THE PANE, AND EVERY TERM BELOW IS A FUNCTION OF IT** —
            // register item 637. This used to be four reads in this order: the deadline, the
            // contract, the ask, the silence. Four reads are four moments, and the code defended
            // the order with a lost-wakeup argument — *a candidate publishing between the deadline
            // read and the verdict read leaves a deadline already past, which buys one more look*.
            // The argument was sound and it existed only because there were two reads. **There is
            // no between now**, so the round's deadline, its ending and its silence are three
            // answers about one instant rather than three instants agreeing by luck.
            let stands = self.stands(panes, pane);
            if let Some(over) = stands.over {
                ending = over;
                return Look::Holds;
            }
            let Some(listening) = listening.as_mut() else {
                // ⚠ No silence bound: the only clock inside this predicate is the supervisor's.
                return stands.settles.not_yet();
            };
            if listening.silent(stands.spoken) {
                ending = Over::Silent(listening.silence());
                return Look::Holds;
            }
            // ⚠⚠⚠ TWO CLOCKS, AND THE EARLIER ONE IS THE DEADLINE — a wait woken by either still
            // re-asks both, so taking the minimum can only cost a look that finds nothing, while
            // taking the later one would sleep through the answer the earlier was about.
            let due = listening.due();
            Look::Settles(stands.settles.due().map_or(due, |verdict| verdict.min(due)))
        });
        match waited {
            Waited::Ready => ending,
            Waited::TimedOut => Over::NotYet,
            Waited::Stopped => Over::RunEnded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{Authority, WorkspacePaneAccess};
    use crate::readiness::{Attended, Reached, Readiness};
    use sprag_detect::{Choice, Question};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A workspace with one pane running `script`, wrapped as pane-access.
    fn sh_access(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .expect("the workspace mutex")
            .spawn(command, "sh".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    /// ⚠⚠ **THE RULE DISCRIMINATES, which is the only thing worth asserting about one variant.**
    ///
    /// A contract that answered the same thing for a peer that ended and one that is still running
    /// would be a constant, and a constant is not evidence — the shape `PaneEcho`'s gate is built
    /// on, applied here.
    ///
    /// A pane whose agent state this test DRIVES, plus a handle to move it — the question here is
    /// what the contract does with an observation, not how one is derived.
    fn supervised(state: AgentState, seq: u64) -> (WorkspacePaneAccess, PaneId, Reported) {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let reported: Reported = Arc::new(Mutex::new(AgentObservation {
            state,
            agent: Some("claude".to_string()),
            authority: Authority::Reported {
                source: "test".to_string(),
            },
            seq,
            asked_seq: seq,
            reports: 0,
            asking: None,
            asked: None,
            said: None,
            said_seq: 0,
            noticed: None,
            transcript: None,
            settling: crate::access::Settling::Nothing,
            reporter: crate::access::ReporterVoice::Speaking,
        }));
        let source = {
            let reported = Arc::clone(&reported);
            Arc::new(move |_id: PaneId| Some(reported.lock().expect("the reported mutex").clone()))
        };
        (access.with_agent_state(Some(source)), pane, reported)
    }

    /// **THE SAME PANE, READ ONLY FROM ITS SCREEN** — no agent reports anything about it.
    ///
    /// ⚠⚠⚠⚠⚠ [`supervised`] hard-codes [`Authority::Reported`] AND an `asked_seq` that keeps step
    /// with `seq`, so every gate written against it describes a hook-instrumented pane and nothing
    /// else. That is the fixture trap [`moved`]'s own doc names — *a fixture that hard-codes a
    /// field is a fixture that has decided the question nobody asked yet* — and register item 441
    /// walked into it: a pane whose state is SCRAPED can never state a question, so a rule that
    /// requires one refuses that pane for ever, and no double in this module could say so.
    ///
    /// ⚠ `asked_seq` is `0` and nothing here can move it. That is not a value chosen for the
    /// fixture; it is the only value a scraped pane has, because the counter advances where a
    /// REPORT states an `asked` and a screen states nothing.
    fn scraped(state: AgentState, seq: u64) -> (WorkspacePaneAccess, PaneId, Reported) {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let reported: Reported = Arc::new(Mutex::new(AgentObservation {
            state,
            agent: Some("claude".to_string()),
            authority: Authority::Scraped {
                rule: Some("idle-glyph".to_string()),
            },
            seq,
            asked_seq: 0,
            reports: 0,
            asking: None,
            asked: None,
            said: None,
            said_seq: 0,
            noticed: None,
            transcript: None,
            settling: crate::access::Settling::Nothing,
            reporter: crate::access::ReporterVoice::Speaking,
        }));
        let source = {
            let reported = Arc::clone(&reported);
            Arc::new(move |_id: PaneId| Some(reported.lock().expect("the reported mutex").clone()))
        };
        (access.with_agent_state(Some(source)), pane, reported)
    }

    /// What a test moves the supervised pane's observation with — the WHOLE observation.
    ///
    /// ⚠ It used to be `(state, seq, agent)`, with `asking` hard-coded to `None` inside the
    /// fixture. That triple could not express the one state this module's rule is about — a peer
    /// that stopped to ASK — so the gate below could not have been written against it. **A fixture
    /// that hard-codes a field is a fixture that has decided the question nobody asked yet.**
    type Reported = Arc<Mutex<AgentObservation>>;

    /// **THE PEER TOOK A QUESTION** — the fact `moved` cannot express and the one
    /// [`DoneWhen::Settles`] now requires before a rest counts as an answer (register item 441).
    ///
    /// ⚠⚠⚠ It is SEPARATE from [`moved`] because the two move for two reasons, and the gap between
    /// them is the whole defect: a prompt typed at a pane that is already `working` is reported
    /// `working` again, so the verdict does not change, `seq` stands still, and only this counter
    /// records that anything was asked. A helper that advanced both together could not stage the
    /// case that mattered — a rest belonging to the PREVIOUS question — which is the one a live loop
    /// spent thirty-three turns inside.
    fn took(reported: &Reported) {
        let mut seen = reported.lock().expect("the reported mutex");
        seen.asked_seq += 1;
    }

    /// Move the supervised pane to `state` at `seq`, reported as `agent`, asking nothing.
    ///
    /// ⚠ It moves the VERDICT only. Whether the peer was asked anything is [`took`]'s to say.
    fn moved(reported: &Reported, state: AgentState, seq: u64, agent: Option<&str>) {
        let mut seen = reported.lock().expect("the reported mutex");
        seen.state = state;
        seen.seq = seq;
        seen.agent = agent.map(str::to_string);
        seen.asking = None;
    }

    /// Move the supervised pane to BLOCKED at `seq`, showing a question a host can read.
    ///
    /// The shape a real agent's tool-permission dialog has: a sentence and a numbered list with a
    /// marker on one option, which is where a bare Enter would land.
    fn asks(reported: &Reported, seq: u64) {
        let mut seen = reported.lock().expect("the reported mutex");
        seen.state = AgentState::Blocked;
        seen.seq = seq;
        seen.asking = Some(Question {
            asked: vec!["Do you want to make this edit to lib.rs?".to_string()],
            choices: vec![
                Choice {
                    number: 1,
                    label: "Yes".to_string(),
                    selected: true,
                },
                Choice {
                    number: 2,
                    label: "No, and tell Claude what to do differently".to_string(),
                    selected: false,
                },
            ],
        });
    }

    /// **SOMETHING SPOKE FOR THE PANE** — one accepted report, whatever it said.
    ///
    /// ⚠⚠⚠ SEPARATE FROM [`moved`], [`took`] AND [`asks`], and the separation IS register item
    /// 458: a turn calling tool after tool reports `working` every time, so the verdict does not
    /// move, no question is stated and no answer is stated — **all three of the other helpers stand
    /// still while this one runs**. A helper that advanced them together could not stage the case
    /// that mattered, which is a peer whose only remaining sign of life is that its reporter is
    /// still there.
    ///
    /// ⚠ It is what `Tracker::report`'s own `self.reports += 1` does, one accepted report at a
    /// time, and it is AFTER that tracker's staleness refusal for the reason the tracker states: a
    /// replayed report is not a heartbeat.
    fn spoke(reported: &Reported) {
        let mut seen = reported.lock().expect("the reported mutex");
        seen.reports += 1;
    }

    /// ⚠⚠⚠⚠⚠ **A PEER THAT HAS STOPPED SPEAKING IS TOLD APART FROM ONE THAT IS STILL WORKING** —
    /// register item 458, and the ceiling its *"done when"* asked for.
    ///
    /// # ⚠⚠⚠⚠ What was measured, and why no counter beside this one can answer it
    ///
    /// A turn a person stopped with Escape emitted **no payload of any kind for fourteen minutes**:
    /// the agent restores the prompt into its composer and suppresses its own idle nag while the
    /// composer holds text, so nothing was ever going to speak again. The pane read `working seq=6
    /// asked=2 said=0` throughout — which is exactly what a long turn reads — and the wait answered
    /// [`Over::NotYet`], the same word it gives a peer that is thinking. A loop acting on that
    /// re-waited its whole per-turn bound, pass after pass, toward a `max_seconds` the shipped kind
    /// authors at twenty-four hours. **Both of that day's incidents were ended by a person.**
    ///
    /// # ⚠⚠⚠⚠⚠ The three controls, and what each of them alone would let through
    ///
    /// The headline on its own is passed by a contract that answers `Silent` for EVERYTHING, which
    /// would be strictly worse than the defect — so each control is a different way of being
    /// wrong:
    ///
    /// * **A PEER THAT IS STILL WORKING.** Its reporter speaks all the way through the bound while
    ///   nothing else about the pane moves — the tool-calling turn the counter exists for. A wait
    ///   that answers `Silent` here hands a healthy turn to a person.
    /// * **A PANE NOBODY REPORTS FOR.** A scraped observation answers `reports: 0` and always will,
    ///   which its own doc calls *"this pane has no reporter to be silent"*. A rule that read that
    ///   zero as silence would call every screen-inferred pane dead ten minutes into its first turn.
    /// * **A CALLER THAT ASKED NOTHING.** The same silent pane with no [`Quiet`] bound must answer
    ///   exactly what it answered before this existed, or three plugins in this crate change
    ///   behaviour without anybody deciding they should.
    #[test]
    fn a_pane_nothing_speaks_for_is_told_apart_from_a_peer_that_is_still_working() {
        /// The TURN's bound. Long enough that an answer arriving at it is unmistakably *the wait
        /// ran out* rather than *silence decided*.
        const BOUND: Duration = Duration::from_millis(1_500);
        /// The SILENCE bound — well inside `BOUND`, so the two are distinguishable by the clock,
        /// and far outside the 10 ms poll so neither reading is an artefact of the cadence.
        const QUIET: Duration = Duration::from_millis(300);
        /// What the silent answer has to beat to be *silence decided* rather than *the bound ran
        /// out*: halfway between the two.
        const WELL_INSIDE: Duration = Duration::from_millis(900);
        /// The count the pane's reporter had reached before it went quiet — the measured `seq=6`
        /// pane's own shape, and a number that is NOT zero so the evidence cannot be a default.
        const SPOKEN: u64 = 6;

        // ── THE HEADLINE: a working peer whose reporter falls silent ──
        //
        // Nothing else about this pane moves, which is the whole difficulty: `working` is what it
        // said, `working` is what it goes on saying, and the three counters beside `reports` are
        // exactly where they were when the turn began.
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        for _ in 0..SPOKEN {
            spoke(&reported);
        }
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        let started = Instant::now();
        let quiet = done.wait(
            &access,
            pane,
            BOUND,
            Quiet::of(QUIET),
            &RunContext::uncancellable(),
        );
        let cost = started.elapsed();
        assert_eq!(
            quiet,
            Over::Silent(Silence {
                reports: SPOKEN,
                within: QUIET,
            }),
            "⚠⚠⚠⚠⚠ NOTHING HAS SPOKEN FOR THIS PANE FOR THE WHOLE BOUND, AND THE WAIT HAS TO SAY \
             SO. `NotYet` here is the answer the product gave for fourteen measured minutes, and \
             it means *the peer is still working* about a peer that will never speak again. It \
             carries BOTH numbers because a reader given only the duration cannot tell a stalled \
             agent from a pane whose reporter never existed. Got {quiet:?}",
        );
        assert!(
            cost < WELL_INSIDE,
            "and it must decide on the SILENCE bound rather than on the turn's, or the ceiling is \
             the thing it was built to replace with a smaller number: {cost:?} against a silence \
             bound of {QUIET:?} inside a turn bound of {BOUND:?}",
        );

        // ── CONTROL ONE: the peer is working, and its reporter says so all the way through ──
        //
        // ⚠⚠⚠ THE ONE THE HEADLINE CANNOT DO WITHOUT. Without it *"it answered Silent"* is passed
        // by a rule that answers `Silent` for every wait, which would take every healthy
        // tool-calling turn away from the loop and give it to a person.
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        let talking = Arc::clone(&reported);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let halt = Arc::clone(&stop);
        let reporter = std::thread::spawn(move || {
            while !halt.load(std::sync::atomic::Ordering::Acquire) {
                spoke(&talking);
                std::thread::sleep(QUIET / 4);
            }
        });
        let working = done.wait(
            &access,
            pane,
            BOUND,
            Quiet::of(QUIET),
            &RunContext::uncancellable(),
        );
        stop.store(true, std::sync::atomic::Ordering::Release);
        reporter.join().expect("the reporter thread");
        assert_eq!(
            working,
            Over::NotYet,
            "⚠⚠⚠⚠ THE CONTROL FAILED. This peer's turn has not ended and its reporter has spoken \
             {} times while the wait ran — which is precisely a turn calling tool after tool, the \
             case `reports` was added for. Answering `Silent` here would hand a healthy turn to a \
             person, and answering anything but `NotYet` would mean the bound decided nothing",
            reported.lock().expect("the reported mutex").reports,
        );

        // ── CONTROL TWO: a pane nobody reports for cannot be silent ──
        //
        // ⚠⚠⚠⚠⚠ `reports` answers `0` here and always will, and that is not silence — it is
        // *this pane has no reporter*. Register item 441 is the round that paid for asking a
        // scraped pane for a number only a reported one has; this is that lesson applied before the
        // fact.
        let (screen_only, screen_pane, _screen_reported) = scraped(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&screen_only, screen_pane);
        let inferred = done.wait(
            &screen_only,
            screen_pane,
            BOUND,
            Quiet::of(QUIET),
            &RunContext::uncancellable(),
        );
        assert_eq!(
            inferred,
            Over::NotYet,
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, and this is the expensive way to be wrong: every pane read \
             from its SCREEN reports zero for ever, so a rule that reads that zero as silence \
             declares every un-instrumented peer dead one bound into its first turn. Got \
             {inferred:?}",
        );

        // ── CONTROL THREE: a caller that asked nothing gets what it always got ──
        //
        // Three of this crate's four callers pass no bound. If the answer moved for them, this
        // round changed `Agent`, `judge` and `Dialogue` without anybody deciding it should.
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        spoke(&reported);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        let unasked = done.wait(&access, pane, BOUND, None, &RunContext::uncancellable());
        assert_eq!(
            unasked,
            Over::NotYet,
            "⚠⚠ THE CONTROL FAILED — `Over::Silent` must be unreachable for a caller that declared \
             no silence bound, by construction and not by arithmetic. Got {unasked:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A WAIT THAT LISTENS FOR SILENCE STILL PARKS ON THE PANE** — register item 629, and
    /// the number nothing in this crate could see until now.
    ///
    /// # ⚠⚠⚠⚠ What it cost, and why the old code said so in its own comment
    ///
    /// Silence is a CLOCK predicate: it becomes true with no output whatever. So a wait handed a
    /// [`Quiet`] bound could not park on the pane — it would never wake to notice — and
    /// [`wait`](Completion::wait) said so by handing its WHOLE bound over as a lag, degrading to
    /// [`poll_until`](crate::run::poll_until) by name. Its comment named the repair and left it
    /// undone: *"the repair is for a listener to publish WHEN its silence falls due"*.
    ///
    /// [`Listening::due`] is that publication. The wait now parks to the earlier of the listener's
    /// own deadline and the supervisor's, and this is what says so in looks rather than in prose.
    ///
    /// # ⚠⚠⚠⚠⚠ TWO ARMS AND A RATIO, because a ceiling alone can be met by a slower clock
    ///
    /// The same wait at two bounds a factor of four apart. **A polling wait costs four times as
    /// many looks in the long arm however slowly it polls**; a parked one costs the same in both.
    /// Register item 280's own gate makes this argument and this is it applied one caller along —
    /// the caller that was explicitly excluded from that repair.
    ///
    /// ⚠ The absolute ceiling is asserted too and is the weaker of the two on purpose: two
    /// enormous arms can satisfy a ratio.
    ///
    /// # ⚠⚠⚠⚠⚠ MEASURED 2026-08-24, BOTH SIDES OF THE REPAIR
    ///
    /// | turn bound | looks POLLED | looks PARKED |
    /// |---|---|---|
    /// | 400 ms | 205 | **5** |
    /// | 1,600 ms | 795 | **5** |
    ///
    /// The polled column is this file with [`Listening::due`] mutated to answer `Instant::now()`,
    /// which is what handing the whole bound over as a lag amounted to. On the ten-minute
    /// `quiet_within_ms` a document may declare, that rate is ~300,000 screen reads.
    ///
    /// # ⛔⛔⛔⛔⛔ AND THE THIRD ARM IS THE ONE THAT MAKES THE OTHER TWO MEAN ANYTHING
    ///
    /// **Cheap and DEAF are the same reading when all you count is looks.** A wait that parked for
    /// ever and never woke would post the best numbers this file can print — and it would make
    /// [`Over::Silent`] unreachable, which is the exact defect the old lag existed to avoid and the
    /// reason the repair was left undone for a round.
    ///
    /// So the third arm gives the same silent pane a bound INSIDE the wait and demands
    /// [`Over::Silent`]. Nothing on that pane ever moves — `cat` waits on its input and prints
    /// nothing — so the ONLY thing that can end it is the listener's own published deadline being
    /// honoured. A mutant that answers [`Look::Steady`] where the code answers
    /// [`Look::Settles`](crate::run::Look::Settles) parks straight past it and answers
    /// [`Over::NotYet`].
    #[test]
    fn a_wait_that_listens_for_silence_parks_and_is_still_woken_by_its_own_bound() {
        /// The short arm's turn bound.
        const SHORT: Duration = Duration::from_millis(400);
        /// The long arm's, four times it. ⚠ The FACTOR is what this gate reads, not either number.
        const LONG: Duration = Duration::from_millis(1_600);
        /// How many looks a listening wait may cost however long it lasts — **measured 5 in both
        /// arms**, so this is set well above the reading rather than at it.
        const CEILING: u64 = 16;
        /// What the long arm may exceed the short one by — not zero, because two live panes are not
        /// bit-identical. At the poll interval the gap between these arms is ~120 looks, so nothing
        /// this size can hide a poll.
        const SLACK: u64 = 8;

        /// Wait out a silent peer with a silence bound FAR OUTSIDE `within`, so the wait ends on
        /// the turn's clock and what is measured is the cost of getting there.
        fn listened(within: Duration) -> (u64, Duration, Over) {
            let (access, pane, reported) = supervised(AgentState::Working, 7);
            // ⚠ Spoken once, so `reports` is not zero: a pane with no reporter cannot be silent at
            // all (control two of the gate above), and this gate must be about one that can.
            spoke(&reported);
            let counted = crate::testing::Counted::new(access);
            let mut done = Completion::new(DoneWhen::Settles);
            done.begin(&counted, pane);
            // ⚠ Counted from AFTER the arming look, so what the arming spent is somebody else's
            // number.
            let entered = counted.looks();
            let began = Instant::now();
            let over = done.wait(
                &counted,
                pane,
                within,
                Quiet::of(within * 8),
                &RunContext::uncancellable(),
            );
            let took = began.elapsed();
            let looked = counted.looks() - entered;
            counted.lifecycle().expect("lifecycle").close(pane);
            (looked, took, over)
        }

        let (short_looks, short_took, short_over) = listened(SHORT);
        let (long_looks, long_took, long_over) = listened(LONG);

        // ── the control: both arms really waited, so there is a wait to have looked during ──
        assert_eq!(
            (short_over, long_over),
            (Over::NotYet, Over::NotYet),
            "⚠⚠⚠ THE CONTROL: with the silence bound eight times the turn's, both waits must end \
             on the TURN's clock. Anything else means this measured a different wait",
        );
        assert!(
            short_took >= SHORT && long_took >= LONG,
            "⚠⚠⚠ AND NEITHER ARM MAY SKIP THE WAIT — a wait that returns at once costs no looks \
             either and would pass every assertion below. short {short_took:?} of {SHORT:?}; long \
             {long_took:?} of {LONG:?}",
        );

        // ── the claim: LOOKING DOES NOT FOLLOW THE CLOCK ──
        assert!(
            long_looks <= short_looks + SLACK,
            "⚠⚠⚠⚠⚠ A LISTENING WAIT IS STILL POLLING THE PANE. A {LONG:?} wait cost {long_looks} \
             looks where a {SHORT:?} one cost {short_looks} — the count follows the CLOCK, so the \
             silence bound is still being spent as a lag. On the ten-minute `quiet_within_ms` a \
             document may declare, that rate is ~60,000 screen reads at a pane nobody is going to \
             hear from. Register item 629",
        );
        assert!(
            long_looks <= CEILING && short_looks <= CEILING,
            "⚠⚠⚠ AND A LISTENING WAIT COSTS A HANDFUL OF LOOKS, NOT HUNDREDS. short \
             {short_looks}, long {long_looks}, ceiling {CEILING}",
        );

        // ── ⛔ THE ARM THAT SEPARATES CHEAP FROM DEAF ──
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        spoke(&reported);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        let began = Instant::now();
        let heard = done.wait(
            &access,
            pane,
            LONG,
            Quiet::of(SHORT),
            &RunContext::uncancellable(),
        );
        let took = began.elapsed();
        access.lifecycle().expect("lifecycle").close(pane);
        assert_eq!(
            heard,
            Over::Silent(Silence {
                reports: 1,
                within: SHORT,
            }),
            "⛔⛔⛔⛔⛔ THE WAIT WENT DEAF INSTEAD OF CHEAP. This pane produces nothing at all, so \
             the ONLY thing that can end this wait is the listener's own deadline being parked to \
             and honoured — which is exactly what item 629 bought. A `NotYet` here means the wait \
             parked straight past its silence bound, and the two arms above would still read as a \
             triumph. Got {heard:?} after {took:?}",
        );
        assert!(
            took < LONG,
            "⚠⚠ and it must leave on the SILENCE bound rather than on the turn's — {took:?} \
             against a turn bound of {LONG:?}",
        );
    }

    /// ⚠⚠⚠ **A ZERO SILENCE BOUND IS AN AUTHOR DECLINING, NOT A WAIT THAT DECIDES INSTANTLY** —
    /// [`Turn::lasting`]'s rule at the other bound, and the reading `ai_loop.scxml` relies on.
    ///
    /// A document spells *no bound of my own* as `0`, because a `<data>` always holds a number.
    /// If [`Quiet::of`] took that as a bound, every run of a document that declined would hand its
    /// first turn to a person on the first poll — the loudest possible way to be wrong about a
    /// decision nobody made.
    #[test]
    fn a_silence_bound_of_zero_is_a_document_declining_rather_than_an_instant_verdict() {
        assert_eq!(
            Quiet::of(Duration::ZERO),
            None,
            "⚠⚠⚠ zero must not construct. *Call my peer silent the instant I stop looking* is not \
             a thing a caller can mean, and an author who reaches zero by editing is told through \
             the absence rather than given a loop that stops on its first poll",
        );
        assert_eq!(
            Quiet::of(Duration::from_millis(1)).map(|bound| bound.within()),
            Some(Duration::from_millis(1)),
            "and the control: every bound that is not zero survives, carrying exactly what it was \
             given — or `of` is a refusal wearing a constructor",
        );
    }

    /// ⚠⚠⚠ **THE DEFECT, MEASURED WITH TODAY'S API — three readings of ONE pane, ONE peer, ONE
    /// moment, and they do not agree.**
    ///
    /// An agent that stops to ASK has finished its turn. It will not write another word until
    /// somebody decides something, so every second spent waiting for it after that buys nothing.
    ///
    /// Both ends of a turn can see it. It is one
    /// [`AgentObservation`](crate::access::AgentObservation), pulled from one supervisor, about one
    /// pane:
    ///
    /// | asked at | reads the ask? | costs |
    /// |---|---|---|
    /// | the START of a turn — [`Readiness::reached`] | yes, since R366 | milliseconds |
    /// | the END of a turn — [`Completion::wait`] | **no** | **the whole bound** |
    ///
    /// [`DoneWhen::Settles`] holds out for [`AgentState::Idle`], which a blocked peer never
    /// reaches, so the wait runs to its bound and answers [`Waited::TimedOut`] — *the peer did not
    /// finish*, about a peer that finished.
    ///
    /// # ⚠⚠⚠ Why the bound IS the number
    ///
    /// This is expensive rather than untidy because of what a caller is told to put in that bound.
    /// [`Turn`]'s own doc says to size it to the peer — *"a shell command is a second and an agent
    /// asked to read a repository is minutes"* — so a CORRECTLY configured agent run pays minutes
    /// of dead wait for every permission dialog, and the better the caller sized it the more it
    /// costs. With no bound at all — the legal spelling that means *wait for my peer*, which is the
    /// one an outer loop wants — the step waits out the RUN's entire remaining clock.
    ///
    /// ⚠ [`DoneWhen::Settles`]'s own doc names this and calls waiting *"the honest answer until a
    /// run can report the question, which is a decision about the step vocabulary and not about
    /// this rule."* That decision was already made, one door over and BEFORE this:
    /// [`Verdict::Blocked`](crate::plugin::Verdict::Blocked) carries exactly the
    /// [`Unanswered`](crate::consent::Unanswered) the barrier builds. **Both halves existed and
    /// nothing joined them.**
    ///
    /// ⚠⚠ R375's shape a second time: the finding is not *"a blocked peer is hard"* — it is an
    /// ASYMMETRY between the two ends of one turn in one crate, and it is only a finding as two
    /// numbers about one pane. Reading the source says one end is missing a check; it does not say
    /// what the omission costs, and the cost is the whole argument.
    ///
    /// # ⚠⚠⚠ THE INSTRUMENT IS KEPT AND RE-POINTED, NOT DELETED
    ///
    /// The measurement above went red the moment [`Over::Asking`] existed, which is what a gate
    /// that measures a defect does when the defect is fixed. Deleting it would throw away the only
    /// thing that says what the fix was worth, so it reads the OTHER way now: the same three
    /// readings of the same pane, with the middle one asserting that the ask ends the wait at once
    /// instead of at the bound. **The bound is still the discriminator, and it is still the
    /// number** — a wait that has to run out to answer cannot pass this, whichever way the
    /// assertion points.
    #[test]
    fn a_turn_that_ends_in_a_question_says_so_instead_of_waiting_out_its_bound() {
        /// Long enough that a wait which runs to it is unmistakable, short enough to pay in a
        /// suite. A REAL caller's is minutes — see the doc above.
        const BOUND: Duration = Duration::from_millis(1_200);
        /// What "at once" has to beat. A quarter of the bound is far outside any polling
        /// interval and far inside the bound, so neither reading can be an artefact of the clock.
        const AT_ONCE: Duration = Duration::from_millis(300);

        // ── THE CONTROL: the contract works, and it is fast when the peer settles ──
        //
        // Without this the two readings below could both be explained by a contract that never
        // fires at all, and the measurement would be about nothing.
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        // ⚠ The peer TOOK the question before answering it — item 441's half of what "answered"
        // means. A control that skipped it would be staging the defect and calling it the control.
        took(&reported);
        moved(&reported, AgentState::Idle, 8, Some("claude"));
        let started = Instant::now();
        let settled = done.wait(&access, pane, BOUND, None, &RunContext::uncancellable());
        let settling_cost = started.elapsed();
        assert_eq!(
            settled,
            Over::Yes,
            "⚠ THE CONTROL FAILED — a peer that worked and came back to rest must end its turn, \
             or neither number below means anything",
        );
        assert!(
            settling_cost < AT_ONCE,
            "the control must be fast, or `the whole bound` is not the discriminator it looks \
             like: {settling_cost:?}",
        );

        // ── WHAT THE DEFECT WAS, NOW THE OTHER WAY UP: the same peer, the same pane, one state
        //    further on ──
        //
        // It did not go quiet. It stopped and asked — which is the OTHER way a real agent's turn
        // ends, and the one that happens whenever it wants to touch anything.
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        asks(&reported, 9);
        let started = Instant::now();
        let asked = done.wait(&access, pane, BOUND, None, &RunContext::uncancellable());
        let asking_cost = started.elapsed();
        let Over::Asking(Some(question)) = &asked else {
            panic!(
                "⚠⚠⚠ the turn ended in a QUESTION and the contract has to say which ending that \
                 was. `NotYet` here is what this gate measured before [`Over`] existed, and it is \
                 wrong twice over: it means *the peer did not finish its turn* about a peer that \
                 finished it, and it hands the caller no way at all to learn that a question is \
                 on the screen. Got {asked:?}",
            );
        };
        assert!(
            question.asked.iter().any(|line| line.contains("lib.rs")),
            "and it carries WHAT is being asked, so the barrier that decides next has the dialog \
             rather than a word about one: {question:?}",
        );
        assert!(
            asking_cost < AT_ONCE,
            "⚠⚠⚠ THE NUMBER THIS GATE WAS BUILT FOR, READ THE OTHER WAY: the wait used to run to \
             its FULL bound here ({BOUND:?}) against a peer that had already stopped and could \
             not have said another word — minutes per dialog once a caller sizes the bound the \
             way this contract's doc tells them to, and the run's whole remaining clock when they \
             decline a bound at all. It now costs {asking_cost:?}",
        );

        // ── THE SIBLING THAT DOES NOT HAVE IT: the same pane, still asking, asked by the other
        //    end of the same turn ──
        //
        // ⚠ This is what makes the reading above a defect rather than a limit. The evidence is
        // not hard to get, not slow to get, and not missing from this host: the barrier this very
        // crate puts in front of every injection reads it out of the same supervisor in
        // milliseconds and says WHAT is being asked.
        let mut barrier = Readiness::new(None, None, None, Attended::NoOne);
        let started = Instant::now();
        let reached = barrier
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("the barrier must answer about a pane that is asking");
        let barrier_cost = started.elapsed();
        let Reached::Asking(unanswered) = &reached else {
            panic!(
                "⚠ THE SIBLING FAILED, so the asymmetry this gate measures is not the one it \
                 names: the START of a turn must read the ask. Got {reached:?}",
            );
        };
        // ⚠ Through `question()`, not through `explain()`. `explain` is the sentence about WHY
        // nothing was answered ("no consent … so it stopped"), and asserting on it would have made
        // this a gate about the refusal rather than about the question — which the first run of
        // this gate said, by failing.
        let asked = unanswered
            .question()
            .expect("the barrier read a question this host can parse");
        assert!(
            asked.asked.iter().any(|line| line.contains("lib.rs")),
            "and it reads WHAT is being asked, down to the option a bare Enter would land on — \
             which is the thing the end of the turn cannot even represent: {asked:?}",
        );
        assert_eq!(
            asked.selected().map(|choice| choice.number),
            Some(1),
            "including WHERE a bare Enter would land, which is what makes typing into it unsafe",
        );
        assert!(
            barrier_cost < AT_ONCE,
            "⚠⚠⚠ THE ASYMMETRY, as the two numbers side by side: the START of a turn answers the \
             ask in {barrier_cost:?} and the END of the same turn spends {asking_cost:?} failing \
             to. Same pane, same peer, same supervisor, same instant — the only difference is \
             which end of the turn is asking",
        );

        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PEER'S REST FROM BEFORE THE TURN IS NOT ITS ANSWER** — the failure this rule would
    /// otherwise introduce, and the reason it was gated before it was wired to anything.
    ///
    /// An interactive agent waiting for a prompt is AT REST. Ask *"is it at rest?"* the instant
    /// after typing one and the honest answer is yes: it has not started. A contract built on the
    /// state alone therefore calls every turn complete in milliseconds, and the capture published
    /// **as the model's answer** is the screen from before the model wrote a word — this crate's
    /// most expensive failure class, arrived at from a new direction.
    ///
    /// So the peer is left EXACTLY as a real one is at that moment — `Idle`, named, and not having
    /// moved — and the contract must refuse it.
    #[test]
    fn a_peer_that_was_already_at_rest_has_not_answered_this_turn() {
        let (access, pane, _reported) = supervised(AgentState::Idle, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠⚠ the peer is at rest and named, and it has NOT answered — it never started. A \
             contract satisfied here captures the screen from before the model wrote a word and \
             publishes it as the model's reply.",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠⚠ **A REST THAT BELONGS TO THE PREVIOUS QUESTION DOES NOT END THIS TURN** — register
    /// item 441, and the defect that cost a live loop thirty-three turns.
    ///
    /// # What it looked like, and why nothing here could see it before
    ///
    /// A loop typed a prompt at an agent that was still working on the last one. The agent's `Stop`
    /// from that EARLIER work arrived, the pane went `idle` with a fresh `seq`, and the contract —
    /// which asked only *is it idle, is it my agent, has `seq` moved?* — called the new turn over.
    /// The judge then read a window the peer had not written a word in, found no marker, and the
    /// loop prompted again. **Nine judged turns heard nothing while the pane plainly showed the
    /// marker**, because every judgement happened before the reply it was judging.
    ///
    /// ⚠⚠⚠⚠ **AND `seq` COULD NOT HAVE FIXED IT, WHICH IS WHY A SECOND COUNTER EXISTS.** The
    /// prompt that would have distinguished the two turns was reported `working` into a pane that
    /// was already `working` — an identical verdict, so nothing published and `seq` never moved.
    /// The submission was invisible. `asked_seq` is that submission, counted where the report
    /// arrives rather than where the verdict changes.
    ///
    /// ⚠⚠ The staging is one line different from its neighbour below — `took` is not called — which
    /// is what makes the pair a measurement of THAT fact rather than of the fixture.
    #[test]
    fn a_rest_left_over_from_the_previous_question_does_not_end_this_turn() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // The peer comes to rest — but it never took THIS question. This is the agent finishing
        // what it was already doing when the prompt landed in its composer.
        moved(&reported, AgentState::Idle, 8, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(300),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠⚠⚠⚠ AN IDLE THE PEER OWES TO AN EARLIER QUESTION IS NOT AN ANSWER TO THIS ONE. \
             Every other term is satisfied — it is idle, it is the addressed agent, and `seq` has \
             moved — which is exactly why this went unnoticed: the contract had no way to ask the \
             only question that separates them. Delete the `asked_seq` term in `satisfied` and this \
             goes green while the loop goes deaf again",
        );

        // ── AND IT ENDS THE MOMENT THE PEER DOES TAKE IT, so the rule refuses a stale rest rather
        //    than refusing everything. ──
        took(&reported);
        moved(&reported, AgentState::Idle, 9, Some("claude"));
        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_secs(5),
                None,
                &RunContext::uncancellable(),
            ),
            Over::Yes,
            "the same pane, one question later — a contract that refused here would have replaced \
             a loop that judges too early with one that never judges at all",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AND IT DOES COMPLETE WHEN THE PEER ACTUALLY ANSWERS** — the other half, without which
    /// the gate above is satisfied by a rule that refuses everything.
    ///
    /// The peer is working when the turn arms, then TAKES the question and goes to rest with a
    /// fresh `seq`, which is what a real agent's turn looks like from the supervisor's side.
    ///
    /// ⚠⚠⚠ **THE `took` LINE IS NOT CEREMONY, AND ITS ABSENCE IS WHAT THIS GATE USED TO MISS**
    /// (register item 441). Without it the double says *the peer came back to rest* while saying
    /// nothing about whether it was ever asked — which is exactly the live case where a loop read an
    /// `idle` belonging to earlier work as its own turn ending. The fixture now models both halves,
    /// so the neighbour below can stage the half that is missing and mean something.
    #[test]
    fn a_turn_is_over_when_the_peer_it_addressed_comes_back_to_rest() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // The peer takes THIS question, answers it, and goes quiet.
        took(&reported);
        moved(&reported, AgentState::Idle, 8, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_secs(5),
                None,
                &RunContext::uncancellable(),
            ),
            Over::Yes,
            "a peer that worked and came back to rest has finished its turn — and this is the \
             evidence the end of a turn never consulted, while the START of one has read it since \
             R359b",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠⚠ **A PEER NOBODY REPORTS STILL FINISHES ITS TURN** — the half the gate above cannot
    /// see, and the one a live run found REGRESSED.
    ///
    /// # What went wrong, measured against a real agent
    ///
    /// The rest-pairing term the two gates above are about — *has this pane been asked something
    /// since the turn armed?* — is answered by [`AgentObservation::asked_seq`], which advances only
    /// where a REPORT states an `asked`. A hook states one. **A screen cannot state anything.** So
    /// for every pane whose agent is recognised by its rendering — an agent with no hook installed,
    /// an agent whose hook has gone mute (register item 344), and every live gate in this
    /// workspace, none of which installs one — that term is false for ever and the turn NEVER ends.
    ///
    /// Measured before this gate was written: a live `claude` was asked for three lines, answered
    /// in about a second, and `DoneWhen::Settles` was still saying `Over::NotYet` **183 seconds
    /// later**, at the bound. The reply was on the pane the whole time.
    ///
    /// ⚠⚠⚠⚠ **AND EVERY FIXTURE IN THIS MODULE AGREED IT WAS FINE**, because [`supervised`] hands
    /// out `Authority::Reported` with an `asked_seq` that keeps step with `seq`. The world where
    /// the counter does not exist had no double, so the round that added the term gated both of its
    /// halves and shipped a pane class that can never end a turn. **A term is only as measured as
    /// the fixture's least-varied field.**
    ///
    /// # The rule this asserts
    ///
    /// A pane's ending is paired with its question only where the ending is the AGENT'S OWN
    /// statement — [`Authority::is_exact`], whose doc has said since it was written that this is
    /// *"the one question a supervisor must ask before treating a state as a turn BOUNDARY"*. A
    /// scraped rest is judged on what a scraped rest can support, which is the three terms that
    /// were there before.
    ///
    /// ⚠ That is a DEGRADATION and is named as one: a screen-read rest cannot be told from one
    /// belonging to earlier work, exactly as this module's other fallbacks admit what they cannot
    /// see. The alternative is not a stricter loop but a loop that never judges anything, and this
    /// crate has already paid for that shape.
    #[test]
    fn a_peer_read_only_from_its_screen_still_ends_its_turn() {
        let (access, pane, reported) = scraped(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // The peer comes to rest. Nothing reports a question, because nothing on this pane can.
        moved(&reported, AgentState::Idle, 8, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_secs(5),
                None,
                &RunContext::uncancellable(),
            ),
            Over::Yes,
            "⚠⚠⚠⚠⚠ A PANE READ FROM ITS SCREEN CAN NEVER STATE A QUESTION, so a rule that demands \
             one refuses it for ever. Measured live at 183 s for a turn that took 1 s. Restore the \
             unconditional `asked_seq` term in `satisfied` and this goes red while every live gate \
             in the workspace waits out its bound",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠ **AND THE DEGRADATION IS NOT A LOOPHOLE — A REPORTED PANE IS STILL HELD TO THE
    /// PAIRING**, which is what stops the gate above from being paid for by weakening the rule for
    /// everybody.
    ///
    /// The distinction is the AUTHORITY of the ending and nothing else: same states, same `seq`
    /// move, same absent question. One is the agent's own statement and is refused; the other is a
    /// rendering and is accepted. A rule that read the counter alone would answer identically for
    /// both, which is why the pair is written as a pair.
    #[test]
    fn a_reported_rest_is_still_paired_with_the_question_it_answers() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // Identical to the gate above in every term except where the answer came from.
        moved(&reported, AgentState::Idle, 8, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(300),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠⚠⚠ AN AGENT THAT SPEAKS FOR ITSELF IS HELD TO WHAT IT SAID. Widen the scraped \
             degradation to every pane and this goes green — and the loop is deaf again, because \
             the rest it accepts belongs to the question before this one",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A QUESTION FROM BEFORE THE TURN IS NOT THIS TURN'S ENDING** — the same discipline the
    /// gate two above holds `Idle` to, applied to the ending that was just added.
    ///
    /// A supervisor's verdict SETTLES: a real detector goes on calling a pane blocked for its
    /// hysteresis window after the dialog has left the screen, which
    /// [`Consent`](crate::consent)'s own answering path measured end to end through a live daemon
    /// and no fixture here can model, because a fixture derives its state from the screen and so
    /// has no lag. Read as a bare predicate, *"is it blocked?"* asked right after a stimulus can
    /// therefore be answered YES by a question that was already gone — and the turn would end on a
    /// dialog nobody is looking at, having captured nothing.
    ///
    /// So the peer is left EXACTLY as one is in that window: blocked, named, showing a readable
    /// question, and **not having moved since the turn armed**. The contract must refuse it.
    ///
    /// ⚠ Then it MOVES, and the same peer's same question does end the turn — without which this
    /// gate is satisfied by a rule that never reports an ask at all, which is precisely the state
    /// the code was in before this round.
    #[test]
    fn a_question_that_was_already_up_is_not_this_turns_ending() {
        let (access, pane, reported) = supervised(AgentState::Idle, 7);
        // Up BEFORE the contract arms, and still up after — the hysteresis window.
        asks(&reported, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠⚠ the question on that screen is one this turn never provoked — it was there when \
             the turn began. A contract satisfied here ends a turn the peer has not started, and \
             hands a caller a dialog to answer that may already have been answered",
        );

        // And now the peer really does raise one: same state, same question, a `seq` that moved.
        asks(&reported, 8);
        assert!(
            matches!(
                done.wait(
                    &access,
                    pane,
                    Duration::from_secs(5),
                    None,
                    &RunContext::uncancellable(),
                ),
                Over::Asking(Some(_)),
            ),
            "and a question this turn DID provoke ends it — without this half the gate above \
             passes for a rule that can never report an ask, which is what the code did before",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A PEER STILL WORKING DOES NOT END THE TURN, even though its `seq` has moved.**
    ///
    /// The arming comparison alone is not enough: a state that CHANGED is not a state at REST, and
    /// an agent prints, calls a tool and thinks its way through several published changes before
    /// it answers. Both halves are required, and this is the half the `seq` test cannot cover.
    #[test]
    fn a_peer_that_is_still_working_has_not_finished_however_much_it_has_moved() {
        let (access, pane, reported) = supervised(AgentState::Idle, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // It started, and has been busy through several published changes.
        moved(&reported, AgentState::Working, 11, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "a peer mid-answer is not a peer that answered — capturing here truncates it",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **THE PEER THAT WENT QUIET MUST BE THE PEER THAT WAS ASKED**, and every way this
    /// contract can fail to KNOW leaves it waiting.
    ///
    /// Three ways, one discipline — the other direction publishes a fragment as a model's answer:
    ///
    /// * The pane's agent CHANGED under the turn. The prompt went to one program and a different
    ///   one is at rest there now; its stillness says nothing about the question that was asked.
    ///   ⚠ This is the arm that the caller-supplied name could not have covered, and the reason
    ///   the name is ARMED from the observation instead: a caller's name can be right about the
    ///   agent while being wrong about the moment.
    /// * The observation names NOBODY — not evidence about anybody.
    /// * The host has no supervisor at all, so the evaluator cannot even arm.
    #[test]
    fn a_contract_that_cannot_know_waits_rather_than_guessing() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        // At rest, and moved — but it is not the program the prompt was given to.
        moved(&reported, AgentState::Idle, 8, Some("codex"));
        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠ the turn was addressed to `claude` and `codex` is what is at rest there now — a \
             contract satisfied here reports another program's quiet as this question's answer",
        );

        // Named nobody: the same absence of evidence, spelled the other way.
        moved(&reported, AgentState::Idle, 9, None);
        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "an observation naming no agent is not evidence about the one that was asked",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // No supervisor at all: the evaluator cannot even ARM, and must not read that as done.
        let (bare, pane) = sh_access("exec cat", 20, 4);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&bare, pane);
        assert_eq!(
            done.wait(
                &bare,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "a host that cannot see agents must not report every turn instantly complete",
        );
        bare.lifecycle().expect("lifecycle").close(pane);
    }

    /// The SUBJECT is a child that exits on its own; the CONTROL is `cat`, which holds its
    /// pseudoterminal open forever and is exactly the long-lived peer the module doc is about.
    #[test]
    fn a_turn_is_over_when_its_one_shot_peer_has_exited_and_not_before() {
        let (ended, pane) = sh_access("exit 0", 20, 4);
        assert_eq!(
            Completion::new(DoneWhen::Exits).wait(
                &ended,
                pane,
                Duration::from_secs(10),
                None,
                &RunContext::uncancellable(),
            ),
            Over::Yes,
            "a one-shot peer's exit is what makes its capture complete",
        );
        ended.lifecycle().expect("lifecycle").close(pane);

        let (running, pane) = sh_access("exec cat", 20, 4);
        assert_eq!(
            Completion::new(DoneWhen::Exits).wait(
                &running,
                pane,
                Duration::from_millis(200),
                None,
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠ and a peer that never exits can only end this wait on the CLOCK — the whole \
             reason a second kind of evidence is owed, spelled here as a measurement rather than \
             as a claim in a comment",
        );
        running.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠ **WHAT A DEAD CHILD'S PANE PRESENTS TO THE CONTRACT THE AI LOOP RUNS ON** — register
    /// item 309's open link, and **this gate is a REPURPOSING rather than a new one**.
    ///
    /// # What it used to hold, and why it is kept rather than deleted
    ///
    /// It measured the defect, because nothing in this workspace had. At one pane, at one instant,
    /// the two contracts answered OPPOSITE things: [`DoneWhen::Exits`] asks `pane_eof` and a dead
    /// child says *over* at once, while [`DoneWhen::Settles`] —
    /// [`INNER_SESSION_ENDS`](crate::outer::INNER_SESSION_ENDS), *the contract this loop makes
    /// load-bearing* — asks a SUPERVISOR whether the agent came back to rest, and **a process that
    /// is gone is reported by nobody**. So the answer was never *yes*; it was never given.
    /// **A dead agent and a thinking one were the same picture** (register item 323), every pass
    /// burnt the whole per-turn bound, and the run said nothing was wrong until its own clock
    /// ended it. Shipped, that bound is half an hour.
    ///
    /// A gate that measures a defect goes red when you fix it — repurpose it, do not delete it —
    /// so the same fixture now holds the opposite claim.
    ///
    /// # ⚠⚠⚠ What it asserts now, and the two halves are separate claims
    ///
    /// [`Over::PeerGone`] naming this pane, **and answered without spending the bound**. The word
    /// alone would not be the repair: an evaluator that took the whole half hour to say it would
    /// leave the run sitting exactly as long as before, and *how fast* is what the cost was made
    /// of. So the elapsed time is asserted against a fraction of the bound rather than printed.
    ///
    /// # ⚠⚠⚠⚠ Why `Yes` WOULD HAVE BEEN A LIE, which is what made this a WORD and not a guard
    ///
    /// The obvious fix was `if panes.pane_eof(pane) == Some(true) { return true }` in the `Settles`
    /// arm. Mutated in, it turns this gate red — the repurpose contract working — but read what it
    /// makes the product SAY: [`Over::Yes`] means *the peer answered on the evidence you named*,
    /// and this peer did not answer, it died. A loop told `Yes` walks on to `judging` and judges a
    /// turn that never happened. There was no arm for the truth, so the one-line fix could only
    /// choose between two lies, and the round that followed spent itself adding the word — to this
    /// type, to [`Verdict`](crate::plugin::Verdict), and to `ai_loop.scxml`.
    ///
    /// # ⚠⚠⚠ The control is UNCHANGED, and that is itself a claim about the fix
    ///
    /// The same dead pane with the agent reported back at rest still answers `Yes`. It is what
    /// keeps the measurement from being *"this evaluator says `PeerGone` to everything at a dead
    /// pane"* — and it is the ORDERING decision made visible: the caller's contract is asked before
    /// `pane_eof`, because *the peer answered and then left* is one instant where both readings are
    /// true and the other order throws the answer away. Register item 329's hypothetical
    /// supervisor is what that gives up, and the refusal at [`PaneAccess::inject`] is what bounds
    /// it.
    ///
    /// # ⚠⚠ Why the agent is reported ALIVE first
    ///
    /// That is the production sequence: a turn is armed against a peer that exists, and the child
    /// dies during it. Arming against a supervisor that already answers nothing would leave
    /// `addressed` unset, and `Settles` is unsatisfied for that reason too — a gate that skipped
    /// the arming would be measuring the wrong `None`.
    #[test]
    fn the_contract_a_loop_runs_on_is_never_satisfied_once_its_agent_is_gone() {
        /// Long enough that "never" is not "not yet scheduled", short enough to pay twice.
        const BOUND: Duration = Duration::from_millis(600);

        let (access, pane) = sh_access("exit 0", 20, 4);
        // ⚠ WHAT THE SUPERVISOR SAYS, and the test moves it: `None` is *there is no such process*,
        // which is what a real supervisor answers for a pane whose child has been reaped.
        let seen: Arc<Mutex<Option<AgentObservation>>> =
            Arc::new(Mutex::new(Some(AgentObservation {
                state: AgentState::Working,
                agent: Some("claude".to_string()),
                authority: Authority::Reported {
                    source: "test".to_string(),
                },
                seq: 7,
                asked_seq: 7,
                reports: 0,
                asking: None,
                asked: None,
                said: None,
                said_seq: 0,
                noticed: None,
                transcript: None,
                settling: crate::access::Settling::Nothing,
                reporter: crate::access::ReporterVoice::Speaking,
            })));
        let source = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_id: PaneId| seen.lock().expect("the reported mutex").clone())
        };
        let access = access.with_agent_state(Some(source));

        let began = Instant::now();
        while access.pane_eof(pane) != Some(true) && began.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "⚠ THE FIXTURE: the child must be gone, or neither answer below is about a dead pane",
        );

        // ⚠⚠⚠ THE EYE IS OPEN AND ONE CONTRACT ALREADY LOOKS THROUGH IT. Same pane, same instant:
        // `Exits` asks `pane_eof` and gets its answer immediately. So the knowledge a refusal would
        // need is not missing from the product, and everything below is about a contract that does
        // not consult it rather than about a fact nobody has.
        assert_eq!(
            Completion::new(DoneWhen::Exits).wait(
                &access,
                pane,
                BOUND,
                None,
                &RunContext::uncancellable()
            ),
            Over::Yes,
            "⚠ THE SECOND CONTROL: the OTHER contract must answer at once at this very pane, or \
             the comparison below is between a working rule and a broken fixture",
        );

        // ⚠⚠ ARMED WHILE THE AGENT IS STILL REPORTED — the production order, see the doc above.
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // ── the process is reaped: nobody reports it any more ──
        *seen.lock().expect("the reported mutex") = None;
        let waited = Instant::now();
        assert_eq!(
            done.wait(&access, pane, BOUND, None, &RunContext::uncancellable()),
            Over::PeerGone(pane),
            "⚠⚠⚠⚠ THE REPAIR: at the pane `Exits` just called finished, the loop's own turn \
             contract must now say THE PEER IS GONE and name it. It used to answer `NotYet` — *the \
             peer did not finish* about a peer that will never finish anything again — so a dead \
             agent and a thinking one were the same picture, every pass burnt the whole per-turn \
             bound, and the run reported nothing wrong until its own clock ran out (register items \
             309, 311, 320, 323)",
        );
        let took = waited.elapsed();
        assert!(
            took * 4 < BOUND,
            "⚠⚠⚠ AND IT MUST SAY SO AT ONCE, which is the half of the repair the word alone does \
             not carry. The cost this gate was written to measure was the WAIT: an evaluator that \
             spent {BOUND:?} arriving at the right answer would leave the run sitting exactly as \
             long as it did before. It took {took:?}",
        );

        // ── THE CONTROL, and without it the line above is just *"this evaluator says PeerGone"* ──
        //
        // ⚠⚠⚠ The SAME contract, the SAME dead pane, the SAME armed evaluator: report the agent at
        // rest with a published change and it answers YES. So what ends the turn above is the
        // agent's disappearance and not the pane being dead, not the arming, and not the contract.
        //
        // ⚠⚠⚠⚠ **AND THIS ARM IS ALSO THE ORDERING DECISION, ASSERTED RATHER THAN COMMENTED.** The
        // contract is asked BEFORE `pane_eof`, so a report of rest still wins at a dead pane — and
        // it has to, because *the peer answered and then left* is one instant where both readings
        // are true, and the other order loses the answer (`Agent`'s own gate measured it going from
        // `converged` with the reply to `failed` with the pane). What that gives up is register
        // item 329's hypothetical supervisor, and the DOOR is what bounds that now: a loop fooled
        // here judges one turn and its next prompt is refused at `PaneAccess::inject`.
        *seen.lock().expect("the reported mutex") = Some(AgentObservation {
            state: AgentState::Idle,
            agent: Some("claude".to_string()),
            authority: Authority::Reported {
                source: "test".to_string(),
            },
            seq: 8,
            asked_seq: 8,
            reports: 0,
            asking: None,
            asked: None,
            said: None,
            said_seq: 0,
            noticed: None,
            transcript: None,
            settling: crate::access::Settling::Nothing,
            reporter: crate::access::ReporterVoice::Speaking,
        });
        assert_eq!(
            done.wait(&access, pane, BOUND, None, &RunContext::uncancellable()),
            Over::Yes,
            "⚠⚠⚠ THE CONTROL FAILED, so the measurement above is about a contract that answers \
             `PeerGone` to everything at a dead pane rather than about a missing agent",
        );

        println!(
            "\n== a turn's end at a pane whose child is dead ==\n  Settles: PeerGone(pane {}) in \
             {took:?}, where the bound is {BOUND:?}\n  it used to answer NotYet after the whole \
             bound, every pass, for as long as the run's own clock lasted\n  control, same dead \
             pane with the agent reported back at rest: Yes\n",
            pane.0,
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔ **ONE ROUND OF THIS CONTRACT ASKS ITS PANE'S SUPERVISOR EXACTLY ONCE** — register
    /// item 637, and the CONSTANT every gate beside it is blind to.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the cost gates this crate already has cannot see this
    ///
    /// Items 280, 630 and 632 all measure a SLOPE — *does the cost of a wait follow the clock, or
    /// the settle window, or the patience* — and each was paid by making the answer *no*. A wait
    /// that looks three times where it could look once has the same slope as one that looks once.
    /// So four reads per round survived every one of those repairs, and `Counted::looks` could not
    /// have said so either: it folds every question about a pane into one number, and *one round
    /// that asked four times* and *four rounds that asked once* are the same fold.
    /// `Counted::supervisions` is the instrument this item needed.
    ///
    /// # ⚠⚠⚠⚠ Why the number is worth four times more than it looks
    ///
    /// In-process each read takes the workspace lock and runs a detector. Over
    /// [`RemotePaneAccess`](../../sprag_host/remote_access/struct.RemotePaneAccess.html) — the
    /// surface a driver outside the daemon walks, and the one register item 544 is moving the loop
    /// onto — each is a **socket round trip**. Four per look is four times the latency of every
    /// wait an agent loop takes.
    ///
    /// # ⚠⚠ AND THE COHERENCE IS THE HALF THAT COULD GO WRONG SILENTLY
    ///
    /// Four reads are four MOMENTS. A round could hold a settling deadline from one observation, an
    /// ending from a second and a silence from a third, and nothing said which instant it was
    /// about. That is not a cost, it is an answer nobody can name — and it cannot be gated
    /// directly, because staging three observations inside one round is a race. **What CAN be
    /// gated is the read count, and at one read the incoherence is unrepresentable.**
    ///
    /// # The fixture, and why the count is exactly one rather than a ceiling
    ///
    /// The peer's pane runs `cat` and says nothing, its verdict is
    /// [`Settling::Nothing`](crate::access::Settling::Nothing), and the silence bound is far above
    /// the patience. So [`park_until`](crate::run::park_until) parks on the pane and is woken by
    /// nothing at all: the whole wait is ONE evaluation of the predicate, and the supervisor read
    /// count IS the per-round constant with no arithmetic in between. **Before this repair the
    /// same wait read four times** — the deadline, the contract, the ask and the silence.
    ///
    /// ⚠ Both arms of [`DoneWhen`] are measured, because they consult different evidence and the
    /// `Exits` arm folded a second read of a different surface: `satisfied` asked
    /// [`PaneAccess::pane_eof`](crate::access::PaneAccess::pane_eof) and the gone-peer check asked
    /// it again, one line apart.
    #[test]
    fn a_round_of_this_contract_reads_its_pane_supervisor_once() {
        /// How long the wait may take. Everything here parks, so this is dead time and not looks.
        const PATIENCE: Duration = Duration::from_millis(300);
        /// The silence bound, far above the patience — so [`Stands::spoken`] is a LIVE term (it is
        /// consulted every round) that can never end the wait. A gate that left it unarmed would
        /// not be measuring the fourth read at all.
        const QUIET: Duration = Duration::from_secs(60);

        /// Arm a contract at a supervised pane, wait it out, and answer **how many times the
        /// supervisor was asked**, how many looks in total, and how the turn ended.
        fn cost_of(when: DoneWhen, rest: bool) -> (u64, u64, Over) {
            let (access, pane, reported) = supervised(AgentState::Working, 1);
            let counted = crate::testing::Counted::new(access);
            let mut done = Completion::new(when);
            // ⚠ ARMED BEFORE THE MEASUREMENT, exactly as a turn arms before injecting — and the
            // baselines are taken AFTER, so `begin`'s own read is not charged to the round.
            done.begin(&counted, pane);
            if rest {
                // The peer answered: it took the question and came back to rest past the counter
                // the turn was armed on. One reading has to be enough to SEE that.
                took(&reported);
                moved(&reported, AgentState::Idle, 2, Some("claude"));
            }
            let asked = counted.supervisions();
            let looked = counted.looks();
            let over = done.wait(
                &counted,
                pane,
                PATIENCE,
                Quiet::of(QUIET),
                &RunContext::uncancellable(),
            );
            let cost = (
                counted.supervisions() - asked,
                counted.looks() - looked,
                over,
            );
            counted.lifecycle().expect("lifecycle").close(pane);
            cost
        }

        let (settles_asked, settles_looks, settles_end) = cost_of(DoneWhen::Settles, false);
        let (exits_asked, exits_looks, exits_end) = cost_of(DoneWhen::Exits, false);
        let (answered_asked, _, answered_end) = cost_of(DoneWhen::Settles, true);

        // ── THE CONTROLS: the wait really ran, and one reading really answers ──────────────────
        assert_eq!(
            (settles_end.clone(), exits_end.clone()),
            (Over::NotYet, Over::NotYet),
            "⚠⚠⚠⚠⚠ THE CONTROL: a contract that ended early costs one read too, and would make \
             the claim below true by not waiting. Both arms must have run out their patience at a \
             peer that never answered. Settles {settles_end:?}, Exits {exits_end:?}",
        );
        assert_eq!(
            answered_end,
            Over::Yes,
            "⚠⚠⚠⚠ AND THE SECOND CONTROL: one reading has to be ENOUGH. A round that read once \
             and could no longer tell that its peer took the question and came back to rest would \
             be cheap and blind — the worse defect, and the one a read-count gate on its own would \
             call a success. Got {answered_end:?}",
        );

        // ── THE CLAIM ─────────────────────────────────────────────────────────────────────────
        assert_eq!(
            (settles_asked, exits_asked, answered_asked),
            (1, 1, 1),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 637: ONE ROUND OF THIS CONTRACT IS ASKING ITS SUPERVISOR MORE \
             THAN ONCE. It was four — the settling deadline, the contract, the ask and the silence, \
             each reading for itself — and over a remote surface each of those is a socket round \
             trip on the path an agent loop walks every step. It is also four MOMENTS folded into \
             one answer, so the round's deadline need not belong to the round's verdict. Settles \
             {settles_asked}, Exits {exits_asked}, answered {answered_asked}",
        );
        assert_eq!(
            (settles_looks, exits_looks),
            (2, 2),
            "⚠⚠⚠⚠ AND THE WHOLE ROUND IS TWO READS — the supervisor and the child, once each. \
             The `Exits` arm used to ask `pane_eof` TWICE one line apart (`satisfied` and the \
             gone-peer check), which is the same defect on the other surface and the reason both \
             arms are measured. Settles {settles_looks}, Exits {exits_looks}",
        );

        println!(
            "\n== what one round of a completion contract costs ==\n  Settles: {settles_asked} \
             supervisor read(s), {settles_looks} look(s)\n  Exits:   {exits_asked} supervisor \
             read(s), {exits_looks} look(s)\n  before item 637 both were 4 supervisor reads, and \
             Exits read the child twice\n",
        );
    }
}
