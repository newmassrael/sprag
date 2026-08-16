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
//! [`AgentObservation`](crate::access::AgentObservation) — *"this agent is at rest, waiting for
//! input"* — carrying an [`Authority`](crate::access::Authority) that says how much the reading is
//! worth. **The evidence existed, it was published, and the end of the turn did not consult it.**
//! [`DoneWhen::Settles`] is that consultation.
//!
//! ⚠⚠ It arrives with its gates and not after them, because a completion rule that fires EARLY
//! truncates a model's answer and publishes the fragment as the reply — the exact failure class
//! this crate has paid for four times, reached from a new direction. The first gate written for it
//! is the one that holds a peer's PRE-TURN rest to not being an answer.

use std::time::Duration;

use sprag_detect::{AgentState, Question};
use sprag_terminal::PaneId;

use crate::access::PaneAccess;
use crate::run::{RunContext, Waited, poll_until};

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
/// the same pane, in milliseconds, since R366. One [`AgentObservation`](crate::access::AgentObservation),
/// two ends of one turn, and only one of them was looking.
///
/// # ⚠ Why a new type rather than a fourth [`Waited`] arm
///
/// R356's rule: when a new state must be handled everywhere an old one was, RENAME rather than add.
/// A fourth arm on `Waited` would have left every `== Waited::TimedOut` in this crate compiling and
/// silently reading *the peer stopped to ask* as *the peer never answered* — which for
/// [`Agent`](crate::agent::Agent) means publishing a permission dialog as the model's reply. A type
/// of its own makes each of the three call sites decide.
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
    addressed: Option<(String, u64)>,
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
            .and_then(|supervisor| supervisor.pane_agent_state(pane))
            .and_then(|seen| seen.agent.map(|agent| (agent, seen.seq)));
    }

    /// **HOW THIS TURN STANDS RIGHT NOW** — [`None`] while it is still running.
    ///
    /// ⚠⚠ VISIBLE TO THE CRATE, and that is not a second door to the question. [`wait`](Self::wait)
    /// IS `poll_until(ended)`, so a caller who needs this contract as one term of a LARGER
    /// predicate — a step that stops either when its peer's turn is over or when the sentinel it
    /// named appears — cannot express it through the wait without running two waits in sequence and
    /// making the first one's bound a lie. One predicate, composed once, is what
    /// [`Orchestrator`](crate::orchestrator::Orchestrator) does with it.
    ///
    /// ⚠ Still not public: the module doc's ONE-DOOR rule is about not offering a bare [`DoneWhen`]
    /// predicate alongside this evaluator, and that stands — an outside caller gets `wait`. What
    /// changed is that this door now answers the same RICH question the waiting one does; while it
    /// answered a `bool`, a caller composing a union could not see the ask at all.
    pub(crate) fn ended(&self, panes: &dyn PaneAccess, pane: PaneId) -> Option<Over> {
        // ⚠ THE CONTRACT IS ASKED FIRST. Where both could be true — a peer that asked and whose
        // pane then reached end-of-file — the evidence the CALLER named is the stronger answer:
        // the turn is over on the terms they chose and the capture is whole. The ask is what ends
        // a turn the contract CANNOT end, and asking it second is what keeps it to that job.
        if self.satisfied(panes, pane) {
            return Some(Over::Yes);
        }
        self.asked(panes, pane).map(Over::Asking)
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
    /// So it takes the same three-part evidence [`satisfied`](Self::satisfied) does — the pane's
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
    fn asked(&self, panes: &dyn PaneAccess, pane: PaneId) -> Option<Option<Question>> {
        let (addressed, began_at) = self.addressed.as_ref()?;
        let seen = panes.supervision()?.pane_agent_state(pane)?;
        (seen.state == AgentState::Blocked
            && seen.agent.as_deref() == Some(addressed.as_str())
            && seen.seq > *began_at)
            .then_some(seen.asking)
    }

    /// Whether `pane` satisfies this contract RIGHT NOW.
    fn satisfied(&self, panes: &dyn PaneAccess, pane: PaneId) -> bool {
        match &self.when {
            // ⚠ An UNKNOWN pane counts as over. A rule that answered "not yet" for a pane that is
            // not there would spin to the timeout on a question that can never be answered — and
            // both plugins that use this already spelled it `unwrap_or(true)`, which is the
            // behaviour this preserves exactly.
            DoneWhen::Exits => panes.pane_eof(pane).unwrap_or(true),
            DoneWhen::Settles => {
                // Never armed, no supervisor to arm from, or no agent identified in the pane the
                // prompt went to — see `begin`. None of those is evidence that a turn ended.
                let Some((addressed, began_at)) = &self.addressed else {
                    return false;
                };
                panes
                    .supervision()
                    .and_then(|supervisor| supervisor.pane_agent_state(pane))
                    .is_some_and(|seen| {
                        // ⚠⚠ ALL THREE, and the last one is what stops a peer's PRE-TURN rest from
                        // reading as its answer. See the variant's doc.
                        seen.state == AgentState::Idle
                            && seen.agent.as_deref() == Some(addressed.as_str())
                            && seen.seq > *began_at
                    })
            }
        }
    }

    /// Wait for this turn to END, bounded by `within` and by the RUN's own deadline — see
    /// [`Over`], which is what the four endings are and why they are four.
    pub fn wait(
        &self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        within: Duration,
        run: &RunContext,
    ) -> Over {
        // ⚠ SEEDED WITH THE ANSWER A WAIT THAT NEVER FIRES DESERVES, so the read after the poll
        // needs no `expect` over an invariant held somewhere else. `poll_until` answers `Ready`
        // exactly when the closure did, and the closure only says so having written an ending
        // here.
        let mut ending = Over::NotYet;
        let waited = poll_until(run, within, || match self.ended(panes, pane) {
            Some(over) => {
                ending = over;
                true
            }
            None => false,
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
    use crate::access::{AgentObservation, Authority, WorkspacePaneAccess};
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
            asking: None,
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

    /// Move the supervised pane to `state` at `seq`, reported as `agent`, asking nothing.
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
        moved(&reported, AgentState::Idle, 8, Some("claude"));
        let started = Instant::now();
        let settled = done.wait(&access, pane, BOUND, &RunContext::uncancellable());
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
        let asked = done.wait(&access, pane, BOUND, &RunContext::uncancellable());
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
                &RunContext::uncancellable(),
            ),
            Over::NotYet,
            "⚠⚠⚠ the peer is at rest and named, and it has NOT answered — it never started. A \
             contract satisfied here captures the screen from before the model wrote a word and \
             publishes it as the model's reply.",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AND IT DOES COMPLETE WHEN THE PEER ACTUALLY ANSWERS** — the other half, without which
    /// the gate above is satisfied by a rule that refuses everything.
    ///
    /// The peer is working when the turn arms, then goes to rest with a fresh `seq`, which is what
    /// a real agent's turn looks like from the supervisor's side.
    #[test]
    fn a_turn_is_over_when_the_peer_it_addressed_comes_back_to_rest() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // The peer answers and goes quiet — a published change, which is what `seq` counts.
        moved(&reported, AgentState::Idle, 8, Some("claude"));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_secs(5),
                &RunContext::uncancellable(),
            ),
            Over::Yes,
            "a peer that worked and came back to rest has finished its turn — and this is the \
             evidence the end of a turn never consulted, while the START of one has read it since \
             R359b",
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
    /// item 309's open link, which had been read and never measured.
    ///
    /// # ⚠⚠⚠ The two variants answer OPPOSITE things, and the loop runs on the one that never does
    ///
    /// [`DoneWhen::Exits`] asks `pane_eof` and a dead child says *over* at once — the gate directly
    /// above measures it. [`DoneWhen::Settles`] is
    /// [`INNER_SESSION_ENDS`](crate::outer::INNER_SESSION_ENDS), *the contract this loop makes
    /// load-bearing*, and it asks a SUPERVISOR whether the agent came back to rest. **A process
    /// that is gone is reported by nobody**, so the answer is never *yes* — not *no*, never
    /// answered at all.
    ///
    /// ⚠⚠ What that costs, and it is why the question was owed before a refusal could be designed:
    /// every pass of a loop whose agent's process died spends the whole per-turn bound waiting for
    /// evidence that cannot arrive, and only the RUN's own clock ends it. **The document has no
    /// transition for a turn that overran** (`watch`'s `Over::NotYet` arm raises nothing), so
    /// nothing about the run says *your peer is dead* — it says nothing at all, for as long as the
    /// budget lasts.
    ///
    /// ⚠⚠⚠ And it is the same pane an [`Orchestrator`](crate::orchestrator::Orchestrator) would go
    /// on typing its stimulus into every step, which is the route the 43-hour wedge was reached by
    /// (items 304, 310). **This is the fact a refusal has to be built on**: `Exits` needs no help,
    /// and `Settles` cannot tell *dead* from *thinking* without asking the eye that already knows.
    ///
    /// # ⚠⚠ Why the agent is reported ALIVE first
    ///
    /// That is the production sequence: a turn is armed against a peer that exists, and the child
    /// dies during it. Arming against a supervisor that already answers nothing would leave
    /// `addressed` unset, and `Settles` is unsatisfied for that reason too — a gate that skipped
    /// the arming would be measuring the wrong `None`.
    ///
    /// # ⚠⚠⚠⚠ MUTATED WITH THE OBVIOUS FIX, AND THE OBVIOUS FIX IS WRONG
    ///
    /// Adding `if panes.pane_eof(pane) == Some(true) { return true }` to the arm below turns this
    /// gate red at once — which is the *"repurpose it, do not delete it"* contract working. But
    /// read what it makes the product SAY: [`Over::Yes`] means *the peer answered on the evidence
    /// you named*, and this peer did not answer, it died. A loop told `Yes` goes on to `judging`
    /// and judges a turn that never happened.
    ///
    /// ⚠⚠⚠ **THERE IS NO [`Over`] VARIANT FOR *THE PEER IS GONE***, and that — not the missing
    /// `pane_eof` call — is what the fix is actually blocked on. It is a word this crate does not
    /// have and a transition `ai_loop.scxml` does not have either, which makes it the DOCUMENT's
    /// decision rather than a line in this file (register item 323).
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
                asking: None,
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
            done.wait(&access, pane, BOUND, &RunContext::uncancellable()),
            Over::NotYet,
            "⚠⚠⚠⚠ THE MEASUREMENT: at the pane `Exits` just called finished, the loop's own turn \
             contract answers NOT YET — it does not report the peer as done and it does not report \
             it as failed, so every pass burns the whole per-turn bound and the run says nothing \
             is wrong until its own clock runs out. ⚠⚠⚠ A DEAD AGENT AND A THINKING ONE ARE THE \
             SAME PICTURE to this rule, which is exactly why `pane_eof` has to be consulted here \
             for a refusal to be possible at all (register items 309, 311, 320)",
        );
        assert!(
            waited.elapsed() >= BOUND,
            "⚠ and it spent the whole bound doing it, which is the cost per pass: {:?}",
            waited.elapsed(),
        );

        // ── THE CONTROL, and without it the line above is just *"this evaluator says no"* ──
        //
        // ⚠⚠⚠ The SAME contract, the SAME dead pane, the SAME armed evaluator: report the agent at
        // rest with a published change and it answers YES. So what strands the turn is the agent's
        // disappearance and not the pane being dead, not the arming, and not the contract.
        *seen.lock().expect("the reported mutex") = Some(AgentObservation {
            state: AgentState::Idle,
            agent: Some("claude".to_string()),
            authority: Authority::Reported {
                source: "test".to_string(),
            },
            seq: 8,
            asking: None,
        });
        assert_eq!(
            done.wait(&access, pane, BOUND, &RunContext::uncancellable()),
            Over::Yes,
            "⚠⚠⚠ THE CONTROL FAILED, so the measurement above is about a contract that answers no \
             to everything rather than about a missing agent",
        );

        access.lifecycle().expect("lifecycle").close(pane);
    }
}
