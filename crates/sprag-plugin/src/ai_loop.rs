//! The OUTER loop — the machine that drives an inner agent session, and what
//! measuring it said about the datamodel it was authored with.
//!
//! [`ai_loop.scxml`] is the third control statechart in this crate, after the
//! Driver's `orchestration.scxml` and the endpoint's `session.scxml`. Until this
//! round it was the only one that was **not compiled**: `build.rs`'s `STATECHARTS`
//! listed two of the three, so 312 lines of authored control flow were enforced by
//! nothing and eight Rust doc comments cited the document as an authority no
//! compiler had ever read.
//!
//! Adding it to that list is one word. What the word bought is this module.
//!
//! # ⚠⚠⚠ And what a person can now DO with it: [`AiLoop`], the plugin
//!
//! R376 compiled the document, R377 gave a turn two endings, R378 built the driver and R379 and
//! R380 measured both against a live `claude` — and at the end of all of that **nothing in the
//! daemon constructed one and no surface started one.** The register's own words: *"after R380
//! this is the single biggest thing between this loop and a user"*.
//!
//! The door is this type, and it is a [`Plugin`] rather than a second run mechanism, which is the
//! whole design. [`OuterLoop::pump`] is ONE PASS and the register's next entry says the rest:
//! *"nothing bounds the pump — the CALLER loops, and `Driver`'s `Guardrails` solved exactly this
//! for plugins"*. Every measurement of the outer loop so far supplied its own bound out of a test
//! (`walked.len() < 16`, a five-minute wall clock) because there was nowhere else to get one.
//!
//! Making it a plugin is what makes that bound the substrate's:
//!
//! * the [`Driver`](crate::driver::Driver) loops it, so `max_iterations`, `max_cost` and
//!   `max_duration` bound a loop run exactly as they bound every other — one discipline, not two;
//! * a run gets an id, a cancel flag, a journal, a progress cell and a durable record from the
//!   registry that already holds them, so `runs` and `cancel` work on it the day it exists;
//! * [`driving`](Plugin::driving) means a cancelled or timed-out loop **stops the agent's turn**
//!   rather than closing the door on a room that is still occupied;
//! * and the pane, the guardrails and the brief are all validated at the door, so a caller's
//!   mistake is a synchronous refusal instead of a run that fails a minute later.
//!
//! [`ai_loop.scxml`]: ../../ai_loop.scxml

use std::sync::Arc;

use sce_rust_runtime::IScriptEngine;
use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::consent::Unanswered;
use crate::driver::Ceiling;
use crate::outer::{AiLoopSpec, AiLoopState, Brief, Briefed, Noticed, OuterLoop, Pumped};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::Reached;
use crate::run::RunContext;

/// **WHY A LOOP DID NOT START** — every refusal is answered at the door, before a byte is typed.
///
/// # ⚠⚠ Why these are refusals and not outcomes
///
/// A run that starts and then fails has already opened a run slot, spawned a thread and possibly
/// typed at somebody's agent. Each of these is knowable from the request alone, and the house rule
/// is that a caller's mistake is a synchronous refusal naming what to change — the same reason the
/// plugin host validates a target pane at submit time rather than reporting `failed` a minute later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotStarted {
    /// The machine's datamodel does not carry the four authored strings, so this driver cannot
    /// drive it at all — see `Authored::read`.
    Undrivable,
    /// The brief did not reach the machine, and what the machine said about it.
    ///
    /// ⚠ [`Briefed::Took`] is not representable here: this arm is only built from the other two.
    Brief(Briefed),
    /// ⚠⚠ **THE BRIEF WOULD TAKE THE RUN THROUGH A STATE THIS BUILD DOES NOT DRIVE**, and which.
    ///
    /// ⚠⚠⚠ ONE ARM REACHES THIS NOW AND IT IS `exhausted`: a brief that allows no turn at all can
    /// only end there, which is a run nobody wanted rather than a state nobody wrote. **The arm this
    /// variant was built for is gone** — `reflecting` was refused because the session-replace
    /// lifecycle behind it did not exist, and it does.
    ///
    /// Refusing here rather than mid-run is the difference between something the caller can act on
    /// before anything happens and a run that prompts a live agent and then stops somewhere with no
    /// answer for it. The variant stays a STATE rather than becoming a sentence about turn budgets
    /// for that reason: the next state this build does not serve gets the same treatment.
    Unbuilt(AiLoopState),
    /// ⚠⚠ **THE LOOP'S STANDING INSTRUCTIONS ARE NOT ONES THIS BUILD CAN CARRY OUT**, and which.
    ///
    /// The rules live in the document's authored half and reach the datamodel either from the file
    /// or through the [`Brief`]. A rule that claims every dialog would refuse every tool call the
    /// agent ever asks about, and one that says nothing leaves it turned down with nothing to do —
    /// so both are answered here, before a byte is typed, exactly as an unreachable state is.
    Screening(crate::outer::NotScreenable),
}

/// A BOUNDED, CANCELLABLE RUN of `ai_loop.scxml`'s machine against one pane — the door onto
/// [`OuterLoop`].
///
/// See the module doc for why this is a [`Plugin`] and not a second run mechanism.
pub struct AiLoop {
    /// The driver, one pass at a time.
    ///
    /// ⚠⚠⚠ IT ALSO HOLDS THE PANE, and this type deliberately keeps no copy. It used to: a `pane`
    /// field set at construction, which was correct for exactly as long as a loop could not replace
    /// its own inner session. `restarting` closes that pane and opens a fresh one, so a copy taken
    /// here names a pane that no longer exists — and [`Plugin::driving`] is what a cancelled run
    /// interrupts, so the model in the replacement pane would go on spending somebody's tokens while
    /// the run reported having stopped it. **The one field this type could hold is the one that made
    /// `driving` lie.**
    inner: OuterLoop,
}

impl std::fmt::Debug for AiLoop {
    /// The pane and where the machine is, and nothing else.
    ///
    /// ⚠ Hand-written because an [`OuterLoop`] owns a compiled statechart engine and a script
    /// interpreter, neither of which is `Debug` and neither of which anybody wants printed. What a
    /// reader meeting this in a failed assertion needs is which pane and which state.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiLoop")
            .field("pane", &self.inner.pane().0)
            .field("state", &self.inner.state())
            .finish()
    }
}

impl AiLoop {
    /// Start a loop over `pane`, evaluated by `script`, for what `brief` says it is for, driven on
    /// the contracts `spec` declares.
    ///
    /// The brief is delivered HERE and not on the first step, so *"the machine did not take what
    /// you briefed it with"* is a refusal the caller reads instead of a run that ends `failed`.
    ///
    /// # Errors
    ///
    /// [`NotStarted`], which names which of the three it was.
    pub fn new(
        script: Arc<dyn IScriptEngine>,
        pane: PaneId,
        brief: &Brief,
        spec: &AiLoopSpec,
    ) -> Result<Self, NotStarted> {
        // ⚠⚠ ASKED BEFORE THE MACHINE IS BUILT, because the answer is arithmetic on the caller's
        // own numbers and needs nothing else. A loop that may take no turn at all can only reach
        // `exhausted`, which is a run nobody wanted rather than one this build cannot drive.
        //
        // ⚠⚠⚠ THE SECOND REFUSAL THAT USED TO BE HERE IS GONE, and its going is the round's headline:
        // `reflect_every < max_turns` was refused because it reaches `reflecting`, and *"the
        // session-replace lifecycle behind it is registered debt"*. It is built. The gate that
        // measured the refusal's premise — that a run really does reach that state — is kept and
        // now measures the walk THROUGH it, which is the standing rule for a gate whose defect has
        // been paid.
        if brief.max_turns < 1 {
            return Err(NotStarted::Unbuilt(AiLoopState::Exhausted));
        }
        let mut inner = OuterLoop::new(script, pane, spec).ok_or(NotStarted::Undrivable)?;
        match inner.brief(brief) {
            Briefed::Took => {}
            refused => return Err(NotStarted::Brief(refused)),
        }
        // ⚠⚠ ASKED AFTER THE BRIEF, because the brief may REPLACE the rules — so validating first
        // would be validating a document the run is not going to use. A caller's own rules are
        // already typed and cannot be malformed; what this reaches is the author's, and the round
        // trip that just carried either of them.
        inner.screening().map_err(NotStarted::Screening)?;
        Ok(Self { inner })
    }

    /// Where the machine is — the loop's own state, for a caller that wants it beside the run's.
    #[must_use]
    pub fn state(&self) -> AiLoopState {
        self.inner.state()
    }

    /// **WHAT THIS LOOP WILL ACTUALLY SAY TO ITS AGENT**, as the document has composed it so far.
    ///
    /// ⚠⚠ A loop that has not been stepped holds three EMPTY prompts and that is correct rather
    /// than broken: the composition happens in `priming`'s `onentry`, on the way to the first
    /// thing that is spoken to. So this answers what the run is going to say only once it has
    /// started saying it — which is the honest shape, and the one a preview has to be built on.
    ///
    /// [`None`] for a machine whose datamodel has stopped answering, which is a run about to fail.
    #[must_use]
    pub fn authored(&self) -> Option<crate::outer::Authored> {
        self.inner.authored()
    }

    /// How many turns the AGENT has taken — the document's own counter, which its `max_turns`
    /// guard compares against.
    ///
    /// ⚠ Not a tally kept out here. A caller with its own would be a second authority on the one
    /// number the run's `exhausted` ending is decided by.
    #[must_use]
    pub fn turns(&self) -> Option<i64> {
        self.inner.turns()
    }

    /// **WHAT THE AGENT'S SESSION HAS BEEN CHARGED TO READ** — the document's own `context`,
    /// assigned on every judged turn.
    ///
    /// The quantity `turns` is not: measurement puts one request's addition to context between 861
    /// tokens and 633,749, so a turn count is out by 63% at p90 as a stand-in for it. See
    /// [`OuterLoop::context`](crate::outer::OuterLoop::context) for what a `Some(0)` means, which is
    /// *do not decide on this* rather than *nothing has accumulated*.
    #[must_use]
    pub fn context(&self) -> Option<i64> {
        self.inner.context()
    }

    /// How many of its peer's calls a standing instruction turned down — **the DOCUMENT's own
    /// count**, beside the run's.
    ///
    /// ⚠⚠ There are deliberately two, and the gate that drives them asserts they AGREE — see
    /// [`OuterLoop::screened`].
    #[must_use]
    pub fn screened(&self) -> Option<i64> {
        self.inner.screened()
    }

    /// Whether `state` is one of the document's five finals.
    ///
    /// ⚠ EXHAUSTIVE, so a sixth final added to the document lands here as a variant that no longer
    /// compiles rather than as a run that pumps a finished machine forever.
    const fn is_final(state: AiLoopState) -> bool {
        match state {
            AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::Blocked => true,
            AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing => false,
        }
    }

    /// What the loop was asking about when it stopped, as a verdict's [`Unanswered`].
    ///
    /// ⚠⚠ READ OFF THE DRIVER'S OWN [`Noticed`] and never off the pane a second time. The screen
    /// has moved since the decision was taken, so a fresh read is a second authority on one fact —
    /// and [`Unanswered::unreadable`] is the honest answer when the driver recorded nothing,
    /// because it says *the remedy is a person* without claiming to know a question.
    fn asking(&self) -> Unanswered {
        match self.inner.noticed() {
            Some(Noticed::Asking(unanswered)) => unanswered.clone(),
            _ => Unanswered::unreadable(),
        }
    }

    /// The verdict for a machine that has reached one of its five final states.
    ///
    /// # Errors
    ///
    /// [`PaneError::Undrivable`] for the document's `failed`, carrying the clause the driver
    /// recorded when it raised `fail`.
    fn ended(&self, state: AiLoopState, spent: u64, note: String) -> Result<Step, PaneError> {
        let verdict = match state {
            // The agent said the word, `closing` got its report, and the report landed.
            AiLoopState::Converged => Verdict::Converged,
            // ⚠⚠ THE DOCUMENT'S OWN BUDGET, which no guardrail can see: `max_turns` counts the
            // inner agent's turns and one of those is many steps of this loop. See
            // [`Ceiling::Turns`].
            AiLoopState::Exhausted => Verdict::Exhausted(Ceiling::Turns),
            // Reached from `awaiting_human --unattended-->`, which nothing produces yet (registered
            // debt), and kept exhaustive rather than folded into the arm below it.
            AiLoopState::Blocked => Verdict::Blocked(self.asking()),
            // ⚠⚠⚠ `cancel` IS RAISED ONLY WHEN THE RUN ITSELF HAS ENDED — `watch` answers it for
            // `Reached::RunEnded` and `Over::RunEnded`, both of which mean this run's context was
            // cancelled or its deadline passed. Both facts are MONOTONE, so the Driver's own
            // `ended_from_outside` is guaranteed to fire at the very next loop top and end the run
            // with the word for whichever it was. Reporting `Continue` here is therefore not a
            // stall: it hands the ending to the one authority that can tell a person's stop from a
            // clock running out, which is a distinction this plugin cannot make and must not guess.
            AiLoopState::Cancelled => Verdict::Continue,
            AiLoopState::Failed => {
                return Err(PaneError::Undrivable(match self.inner.noticed() {
                    Some(Noticed::Undrivable(variable)) => format!(
                        "its datamodel stopped answering for {variable:?}, so the prompt it owed \
                         the pane could not be read and nothing was sent"
                    ),
                    // ⚠⚠⚠ THE REPLACEMENT SESSION DID NOT COME UP, and this round's own sweep is why
                    // these two arms exist. `resuming` raises `fail` when the barrier over a FRESH
                    // pane answers anything but *ready* — a question nobody covered, or somebody
                    // typing in it — and it records the fact in the notice. Without these arms the
                    // run's only sentence said *"recorded no reason"* about a run that had recorded
                    // one, and the question the loop was holding was dropped on the floor.
                    Some(Noticed::Asking(unanswered)) => format!(
                        "the session it opened to replace the old one came up asking something \
                         nothing this run holds could answer ({unanswered:?}), so it was never \
                         prompted — a session that will not come back is a failed run"
                    ),
                    Some(Noticed::Interrupted(who)) => format!(
                        "somebody was already typing in the session it opened to replace the old \
                         one ({who:?}), so it was never prompted"
                    ),
                    // `fail` is raised from three places and the other one is `brief`, which this
                    // plugin answers at the door — so a `failed` with no notice is a path nobody
                    // wrote, and saying so beats inventing a cause.
                    _ => "it reached `failed` and recorded no reason, which is a path this driver \
                          does not write"
                        .to_owned(),
                }));
            }
            AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing => Verdict::Continue,
        };
        Ok(Step::new(Cost::Bytes(spent), verdict).noting(note))
    }

    /// The verdict for a machine sitting in a state this driver has no effect for.
    ///
    /// # ⚠⚠⚠ Why each of these ENDS the run instead of pumping again
    ///
    /// [`Pumped::Unbuilt`] is advisory — *"a caller that ignores it pumps again"* — and a caller
    /// that does is a loop watching a state nothing will ever move it out of, until a guardrail
    /// bites and reports `exhausted — iterations` about a run that took no turn. That is the
    /// registered cost of an advisory answer, and this is where it is paid: the run stops, and the
    /// word it stops on is the one whose remedy is real.
    ///
    /// ⚠ AND IT IS NOT *"unimplemented"* to whoever reads the run. `awaiting_human` is the only state
    /// left here, and a peer that stopped to ask wants an ANSWER while a person at the pane is
    /// already acting — the same two facts every other plugin reports, given the same two words.
    /// `reflecting` and `restarting` used to be here as this build's own gap; they are built, so the
    /// one thing this function is about is a run waiting for somebody who is not coming.
    ///
    /// ⚠⚠⚠ **AND IT CHARGES WHAT THE REFUSAL SPENT**, which is a hole this round opened and closed
    /// in the same breath. `Orchestrator`, `Agent` and `Answer` all report `Cost::Bytes(asking
    /// .bytes())` on a `Blocked`, and a loop reported zero — true for as long as a loop could not
    /// type at a dialog, and false the moment `screening` could press a key and give up
    /// ([`Refusal::NotDismissed`](crate::consent::Refusal::NotDismissed)). **A cost ceiling that
    /// cannot see what a run typed into somebody's dialog is a ceiling with a hole in it.**
    fn unbuilt(&self, state: AiLoopState) -> Result<Step, PaneError> {
        let spent = match self.inner.noticed() {
            Some(Noticed::Asking(unanswered)) => unanswered.bytes(),
            _ => 0,
        };
        let verdict = match (state, self.inner.noticed()) {
            // A PERSON TOOK THE PANE. `awaiting_human` is where the document waits for them, and
            // `taken_over` is this substrate's word for the same fact.
            (AiLoopState::AwaitingHuman, Some(Noticed::Interrupted(who))) => {
                Verdict::TakenOver(*who)
            }
            // THE PEER STOPPED TO ASK AND NOTHING GOT THE RUN PAST IT. `screening` is built now, so
            // reaching `awaiting_human` means it ran and answered `screen.none` — no rule claimed
            // the dialog, or one did and the refusing key did not take it. Either way the answer is
            // the one every unattended run gives: stop, and publish what is being asked, with the
            // driver's own refusal saying which of the two it was.
            (AiLoopState::AwaitingHuman, _) => Verdict::Blocked(self.asking()),
            (state, _) => {
                return Err(PaneError::Undrivable(format!(
                    "it reached {state:?}, which this build has no effect for — and the brief that \
                     could reach it is refused at the door, so this run took a path nobody wrote"
                )));
            }
        };
        Ok(Step::new(Cost::Bytes(spent), verdict).noting(format!(
            "the loop is in {state:?}, which no driver serves yet"
        )))
    }
}

impl Plugin for AiLoop {
    /// ONE PUMP of the machine, reported in the substrate's own terms.
    ///
    /// ⚠⚠ A MOVE INTO A FINAL STATE IS JUDGED IN THE SAME STEP THAT MADE IT, never on the pump
    /// after. The Driver checks its ceilings after every unconverged step, so a loop that reported
    /// `Continue` on the step that reached `converged` would be told it had run out of iterations
    /// on the very step that finished the work — *"a step that saw the goal SAW IT"*, which is the
    /// Driver's own rule read from this side.
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        match self.inner.pump(panes, run)? {
            Pumped::Moved {
                from,
                raised,
                to,
                spent,
            } => {
                // ⚠⚠⚠ AN APPROVAL IS REPORTED BEFORE ANYTHING ELSE THIS STEP DID, and TAKEN so it
                // is reported once. The barrier answered the peer's question inside this pump, on
                // a clause the caller declared — a decision taken on somebody's behalf, which the
                // substrate insists is a WORD and not a note (`Verdict::Answered`), because a run
                // whose journal spells it `continue` is a run in which approvals are indexed by
                // nothing.
                //
                // ⚠⚠ ITS BYTES JOIN THE STEP'S. They were typed by the BARRIER rather than by
                // `say`, so `Pumped::Moved`'s own `spent` cannot see them — and a cost ceiling
                // that could not see what a run typed into somebody's dialog would be a ceiling
                // with a hole in it.
                if let Some(answered) = self.inner.took_answer() {
                    let note = answered.describe();
                    return Ok(Step::new(
                        Cost::Bytes(spent + answered.bytes),
                        Verdict::Answered(answered),
                    )
                    .noting(note));
                }
                // ⚠⚠⚠ AND A REFUSAL GIVEN ON THE AUTHOR'S STANDING INSTRUCTION, reported the same
                // way and for a sharper version of the same reason: this step **stopped the
                // caller's agent doing something it had decided to do** and told it something else
                // instead. An act with that much consequence outside the loop cannot reach a
                // person's report as the word `continue`.
                //
                // ⚠⚠ ITS BYTES JOIN THE STEP'S, exactly as an answer's do — the refusing key and
                // the redirect were both typed by `screening` rather than by the transition's own
                // prompt, so `Pumped::Moved`'s `spent` cannot see either. `screen.matched` owes no
                // prompt (`Owed::on`), so that number is zero and this is the whole cost.
                if let Some(screened) = self.inner.took_screening() {
                    let note = screened.describe();
                    return Ok(Step::new(
                        Cost::Bytes(spent + screened.bytes),
                        Verdict::Screened(screened),
                    )
                    .noting(note));
                }
                let note = format!("{from:?} --{raised:?}--> {to:?}");
                if Self::is_final(to) {
                    self.ended(to, spent, note)
                } else {
                    Ok(Step::new(Cost::Bytes(spent), Verdict::Continue).noting(note))
                }
            }
            Pumped::Ended(state) => {
                self.ended(state, 0, format!("the loop is already in {state:?}"))
            }
            Pumped::Unbuilt(state) => self.unbuilt(state),
            // ⚠⚠⚠ THE PANE IS NOT THIS LOOP'S TO TYPE INTO, before a byte has been sent. Three of
            // the five answers are facts about somebody else and END the run, for
            // [`Self::unbuilt`]'s reason: a pane showing a question the loop did not provoke — a
            // fresh agent's *"do you trust this folder?"* is exactly this, measured — will go on
            // showing it, so pumping until a guardrail bites reports `exhausted` about a run that
            // never started. The other two are the pane mid-transition and the next pump asks
            // again.
            Pumped::NotReady(Reached::Asking(unanswered)) => {
                Ok(Step::new(Cost::Bytes(0), Verdict::Blocked(unanswered))
                    .noting("the pane was already asking something this run had said nothing to"))
            }
            Pumped::NotReady(Reached::Interrupted(who)) => {
                Ok(Step::new(Cost::Bytes(0), Verdict::TakenOver(who))
                    .noting("somebody was typing in the pane before this run started"))
            }
            Pumped::NotReady(seen) => Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting(format!("the pane is not ready to be driven yet: {seen:?}"))),
        }
    }

    /// THE PANE WHOSE AGENT THIS LOOP IS CAUSING TO WORK — until the machine is finished with it.
    ///
    /// ⚠⚠⚠ ANSWERED, and this is the whole reason a cancelled loop is safe. The inner session is a
    /// live agent CLI mid-turn: a run cancelled or timed out while it is thinking would otherwise
    /// stop stepping and leave the model spending somebody's tokens on a question nothing is
    /// waiting for. The Driver sends the pane's job an interrupt on exactly the two endings that
    /// can land mid-step, and it can only do that because this names the pane.
    ///
    /// ⚠⚠⚠ **AND "THE MACHINE IS FINISHED" IS NOT THE SAME QUESTION AS "THE AGENT IS FINISHED",
    /// which is what the first draft of this got wrong and its own gate said so in one line.**
    ///
    /// The obvious rule is *answer `None` once the document is in a final state*, and it makes this
    /// method useless for the one ending it exists to serve. `cancelled` is reached because
    /// `watch` saw the run end **while the agent was mid-turn** — so by the time the Driver asks
    /// what to stop, the document is finished and the agent is not. Measured: the gate below read
    /// back `Stopped::Nothing` on a cancelled run whose peer was still working.
    ///
    /// So the two endings that follow a COMPLETED turn answer `None`, and everything else answers
    /// the pane. That is the direction that fails safe: a needless interrupt costs a peer one
    /// keystroke it was waiting at anyway, and a missed one leaves a model spending somebody's
    /// tokens on a question nothing is waiting for.
    ///
    /// ⚠⚠⚠ **AND WHICH PANE IS A QUESTION WITH A MOVING ANSWER, since `restarting` was built.** It is
    /// asked of the driver on every call and never cached, because a loop that has reflected is
    /// driving the pane that REPLACED the one it was started over — see [`OuterLoop::pane`]. A copy
    /// held here would name a closed pane, and this method's whole reason is what happens to the one
    /// that is still occupied.
    fn driving(&self) -> Option<PaneId> {
        match self.inner.state() {
            // ⚠ THE PEER IS AT REST BY CONSTRUCTION in both: `converged` is entered when the
            // closing report's turn ended, and `exhausted` when a judged turn did. Signalling a
            // pane this run has finished with would interrupt whatever a person started in it next.
            AiLoopState::Converged | AiLoopState::Exhausted => None,
            // Everything else, and `cancelled` above all — see the paragraph above. `failed` and
            // `blocked` join it because neither says anything about what the peer is doing, and an
            // unknown answer must fail towards stopping.
            AiLoopState::Cancelled
            | AiLoopState::Failed
            | AiLoopState::Blocked
            | AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing => Some(self.inner.pane()),
        }
    }

    /// ⚠ NOTHING, deliberately. [`captured`](Plugin::captured) is *content the plugin produced* —
    /// an AI adapter's reply — and everything this loop learns is already published: the machine's
    /// walk is in the run's journal, its turn count is the document's own counter, and its ending
    /// is the outcome. The agent's closing REPORT is real content and is on the pane; capturing it
    /// means reading the screen after `closing`, which is a turn's worth of judgement this round
    /// did not measure and did not guess.
    fn captured(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

    use super::{AiLoop, NotStarted};
    use crate::access::PaneAccess;
    use crate::completion::Turn;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState, ProgressCell, Stopped};
    // ⚠ `OuterLoop` and `Pumped` are gone from here, and their going is a fact: the gate that used
    // them drove the layer UNDER the door in order to reach a state the door refused. The door no
    // longer refuses it, so the PLUGIN reaches it, which is the only height a caller has.
    use crate::outer::{AiLoopSpec, Brief, INNER_SESSION_ENDS};
    use crate::plugin::Plugin;
    use crate::readiness::ReadyWhen;
    use crate::run::RunContext;
    use crate::sm::ai_loop::{AiLoopEvent, AiLoopPolicy, AiLoopState};
    use crate::testing::{standin_agent, supervised};

    /// The document's own composed prompt, as a person reading the file expects it.
    const COMPOSED_START_PROMPT: &str = "North star: ";

    /// A real script engine, as the daemon's construction site builds one.
    fn engine() -> Arc<dyn IScriptEngine> {
        Arc::new(sce_rust_lua::LuaEngine::new())
    }

    /// The spec these gates drive with — the stand-in's two facts, and a per-turn bound small
    /// enough that a stalled gate fails rather than hangs.
    ///
    /// ⚠ `shows_the_prompt` is FALSE because a `/bin/sh` peer paints only once it has a whole
    /// LINE, so a delivery cannot be confirmed on screen before the newline that would submit it.
    /// [`AiLoopSpec::driving`] is the real-agent shape and sets it true.
    fn standin_spec() -> AiLoopSpec {
        AiLoopSpec {
            ready_when: Some(ReadyWhen::Settles("claude".to_string())),
            ready_within: None,
            turn: Turn::lasting(INNER_SESSION_ENDS, Some(Duration::from_secs(5)))
                .expect("a non-zero bound"),
            shows_the_prompt: false,
            may_answer: None,
            attended: crate::readiness::Attended::NoOne,
            // ⚠ NO JUDGE, so `working`'s `cond="_event.data.judged"` is always false here and
            // every blocked turn takes the `screening` edge. A stand-in gate that acquired one
            // would spawn a real agent per dialog, which is what these gates exist to avoid.
            judge: None,
        }
    }

    /// A brief that reaches a milestone in `max_turns` and never reflects.
    fn brief_for(max_turns: i64) -> Brief {
        Brief {
            north_star: "the stand-in answers prompts and then says the marker".to_string(),
            milestone: "reach it".to_string(),
            reference: "this gate".to_string(),
            max_turns,
            // ⚠ EQUAL, which is what makes `reflecting` unreachable — `judging` tests the turn
            // budget first. `AiLoop::new` refuses anything smaller, and the gate below drives that.
            reflect_every: max_turns,
            // ⚠ The document's own placeholder rule, which claims nothing — so every gate below
            // that does NOT set this measures a loop with screening available and unarmed, which
            // is the shipped shape.
            screen_rules: None,
        }
    }

    /// ⚠⚠⚠ **THE LOOP IS BOUNDED BY THE SUBSTRATE, NOT BY WHOEVER IS PUMPING IT** — register item
    /// 66, and the reason this is a [`Plugin`] at all.
    ///
    /// Every measurement of the outer loop before this round supplied its own ceiling out of a
    /// test — `assert!(walked.len() < 16)` in the driver's gate, a five-minute wall clock in the
    /// live one — because `OuterLoop::pump` is ONE PASS and nothing above it looped. A run that
    /// stalled was bounded by whatever the harness happened to write.
    ///
    /// Here the [`Driver`] does it, so the same three guardrails that bound an `orchestrator` bound
    /// a loop. The gate asserts BOTH halves, because either alone would be worth little:
    ///
    /// * a briefed loop against a real pseudoterminal reaches `converged` **through the Driver**,
    ///   with the machine's own turn count matching what the peer was asked;
    /// * and the run's JOURNAL carries the walk, which is what makes a loop that does not converge
    ///   diagnosable at all — the fact `Pumped::Moved` was carrying and nothing consumed.
    #[test]
    fn a_loop_run_converges_under_the_driver_that_bounds_it() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        // ⚠ THE CONTROL: the pane must be this run's to drive BEFORE it ends, or the `None` the
        // outcome's stop rests on below would be true for the wrong reason.
        assert_eq!(
            loops.driving(),
            Some(pane),
            "⚠⚠ a loop mid-run owns the pane its agent is working in — this is what makes a \
             cancelled run stop the turn rather than abandon it",
        );

        // ⚠ The journal is the PROGRESS cell's, not the outcome's: it is published as a run goes
        // rather than at its end, which is what lets a client tell progress from stuck.
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            // ⚠ WELL ABOVE the five passes the authored happy path takes and well below anything
            // that would take real time, so a stall is caught HERE and a converged run is the
            // machine's own doing rather than a ceiling's.
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠⚠ the whole loop, driven by the substrate: {:?} — walked {walk:?}",
            outcome.state,
        );
        assert_eq!(
            loops.state(),
            AiLoopState::Converged,
            "and the DOCUMENT agrees with the run's word, or the two are counting different things",
        );
        assert!(
            outcome.iterations > 1 && outcome.iterations < 40,
            "⚠⚠ a converged loop takes several pumps and stops well inside its ceiling; {} of 40 \
             would mean the guardrail decided rather than the machine",
            outcome.iterations,
        );
        // ⚠⚠ THE JOURNAL IS THE WALK. `Pumped::Moved` carried `from --raised--> to` and nothing
        // read it; a run that failed to converge could be diagnosed by its total alone, which is
        // what the register calls a loop nobody can debug.
        assert!(
            walk.iter().any(|note| note.contains("Priming"))
                && walk.iter().any(|note| note.contains("Judging"))
                && walk.iter().any(|note| note.contains("Converged")),
            "⚠⚠ the run's journal must carry the machine's own walk, or a loop that stalls is \
             diagnosable only by its totals: {walk:?}",
        );
        assert_eq!(
            loops.driving(),
            None,
            "⚠ and a FINISHED loop names no pane: signalling one a run is done with would \
             interrupt whatever a person started in it next",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A LOOP THAT SPENDS ITS AUTHOR'S TURNS SAYS SO, AND NAMES THE KNOB THAT WOULD BUY IT
    /// MORE** — the whole reason [`Ceiling::Turns`] exists.
    ///
    /// `max_turns` counts the inner AGENT's turns and one of those is many steps of the loop
    /// driving it, so no [`Guardrails`] number can express it. Without a fourth ceiling the two
    /// available endings were both wrong: `converged` claims a goal nobody reached, and handing the
    /// run back to the guardrails reports `exhausted — iterations` about a run whose iteration
    /// ceiling was never met, telling its reader to raise a number that would have bought them
    /// nothing.
    ///
    /// ⚠⚠ **THE ITERATION CEILING IS THE CONTROL AND IT IS DELIBERATELY GENEROUS.** The peer here
    /// never says the marker, so the run must end on the document's budget with iterations to
    /// spare — and the assertion is on WHICH ceiling, not merely that it stopped.
    #[test]
    fn a_loop_that_uses_the_turns_it_was_briefed_with_reports_that_ceiling() {
        // ⚠ A peer that answers every prompt and NEVER says the done marker, so nothing but the
        // document's own budget can end this run.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(2), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "⚠⚠⚠ the run must name the DOCUMENT's budget. `iterations` here would send its reader \
             to raise a guardrail that never bound this run: {:?}, after {} of 40 iterations",
            outcome.state,
            outcome.iterations,
        );
        assert!(
            outcome.iterations < 40,
            "⚠⚠ THE CONTROL: the iteration ceiling must not have bitten, or the assertion above is \
             about a guardrail wearing the document's word. Took {}",
            outcome.iterations,
        );
        assert_eq!(
            loops.state(),
            AiLoopState::Exhausted,
            "and the document agrees it is the one that ended the run",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A BRIEF THAT WOULD REACH A STATE NOBODY BUILT IS REFUSED BEFORE A BYTE IS TYPED** —
    /// and the document's own shipped numbers are no longer one of them.
    ///
    /// # ⚠⚠⚠ THE HALF THAT WAS REMOVED, kept here because its going is the round's headline
    ///
    /// This gate used to assert that the document's shipped pair — `reflect_every` 8 against
    /// `max_turns` 40 — was REFUSED, because the default brief walks into `reflecting` at turn eight
    /// and the session-replace lifecycle behind that state did not exist. It does now, so the same
    /// brief STARTS, and that is asserted below rather than left as an absence: a refusal that
    /// quietly stopped happening would leave a caller reading a sentence about a knob nothing
    /// enforces.
    ///
    /// What survives is the other end of the same arithmetic: a loop allowed NO turn can only reach
    /// `exhausted`, which is a run nobody wanted.
    ///
    /// ⚠⚠ **THE PANE IS THE ASSERTION**, not just the refusal: the whole value of refusing at the
    /// door is that nothing happened, and a refusal that had already typed the first prompt would
    /// be worth nothing. The screen is checked to be exactly what it was.
    #[test]
    fn a_brief_that_would_reach_an_unbuilt_state_is_refused_before_anything_is_typed() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let before = access.pane_collapsed(pane).expect("a readable pane");

        // ⚠⚠⚠ THE DOCUMENT'S OWN SHIPPED PAIR NOW STARTS. Every other assertion here is about a
        // refusal, and this one exists so that the refusal's DISAPPEARANCE is a thing a test says
        // out loud.
        AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 8,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect(
            "⚠⚠⚠ the document's shipped `reflect_every: 8` reaches `reflecting`, and a run that \
             reaches it is one this build drives — see `restarting`",
        );

        // ⚠ AND A LOOP THAT CANNOT TAKE A TURN AT ALL, at the other end of the same arithmetic.
        assert_eq!(
            AiLoop::new(engine(), pane, &brief_for(0), &standin_spec())
                .expect_err("a loop allowed no turns judges itself exhausted before it starts"),
            NotStarted::Unbuilt(AiLoopState::Exhausted),
            "the refusal must name the STATE, so the sentence a caller reads can name the knob",
        );

        assert_eq!(
            access.pane_collapsed(pane).expect("a readable pane"),
            before,
            "⚠⚠⚠ NOTHING WAS TYPED. Neither the refusal nor the loop that started has spoken to the \
             pane — a door that prompted an agent before answering would have cost exactly what \
             answering early exists to save",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A LOOP WHOSE AGENT STOPS TO ASK PERMISSION** — register items 119/120/112, measured
    /// with today's API before anything was built for it.
    ///
    /// Every live measurement of this loop picked an ARITHMETIC milestone *"so no permission dialog
    /// can fire"*, and the model itself wrote back that the next step needed real work. **Every
    /// kind of real work raises one of these.** This is what happens when one does.
    ///
    /// ⚠⚠ IT IS A PAIR, and the pair is the whole claim:
    ///
    /// * **given no consent, the run STOPS**, publishes the question, and says `no_consent` — which
    ///   is honest and is the end of the run;
    /// * **given the caller's own consent, the same peer, the same brief and the same fixture
    ///   CONVERGE** — the dialog is answered, the loop takes its next turn and the agent reaches
    ///   the milestone.
    ///
    /// Either half alone would be a fact about one arrangement. Together they say the consent is
    /// what makes the difference, and nothing else about the run changed.
    #[test]
    fn a_loop_whose_agent_asks_stops_without_a_consent_and_goes_on_with_one() {
        /// One run against a peer that raises a permission dialog on its first turn, with whatever
        /// answering contract `may_answer` declares — and what became of it.
        fn run_with(may_answer: Option<crate::consent::Consents>) -> (OutcomeState, Option<i64>) {
            let (workspace, pane) = crate::testing::standin_agent_asking();
            let access = crate::testing::supervised_asking(&workspace);
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &brief_for(40),
                &AiLoopSpec {
                    may_answer,
                    ..standin_spec()
                },
            )
            .expect("a well-briefed loop over a live pane starts");
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .run(&mut loops, &access, &RunContext::uncancellable());
            let turns = loops.turns();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome.state, turns)
        }

        // ── THE DEFECT, IN ITS OWN WORDS ──
        let (unarmed, unarmed_turns) = run_with(None);
        let OutcomeState::Blocked(Some(unanswered)) = &unarmed else {
            panic!(
                "⚠⚠⚠ a loop whose agent stops to ask must report BLOCKED with the question, not \
                 {unarmed:?} — anything else is a run that typed at a menu or died silently",
            );
        };
        // ⚠⚠⚠ THE HEAD REASON MOVED WHEN `screening` WAS BUILT, and keeping BOTH halves asserted
        // is the point rather than a repair. Screening is now the last authority to look at this
        // dialog, so the arm is its answer — but the CONSENT-level reason is what makes the second
        // half of this pair possible, and a report that lost it would send a caller to write a
        // standing rule about a dialog whose own `Yes` a clause could take. See
        // `Unanswered::unscreened`.
        assert_eq!(
            unanswered.why(),
            crate::consent::Refusal::NoRule,
            "⚠⚠ the LAST authority to look at the dialog is what the arm names, and after \
             `screening` exists that is the rules: {unanswered:?}",
        );
        assert!(
            unanswered
                .explain()
                .contains(crate::consent::Refusal::NoConsent.wire_str()),
            "⚠⚠⚠ AND THE CONSENT-LEVEL REASON MUST SURVIVE UNDERNEATH. `no_consent` is the reason \
             whose remedy is a change to the CALL — the very change the second half of this pair \
             makes — and a run that reported only `no_rule` would send its caller to write a \
             standing instruction about a dialog that offers `Yes`: {}",
            unanswered.explain(),
        );
        assert!(
            unanswered.question().is_some(),
            "⚠⚠ and it must publish the question itself, or a person coming back to this run has \
             to go and read the pane: {unanswered:?}",
        );
        assert_eq!(
            unarmed_turns,
            Some(0),
            "⚠⚠⚠ THE NUMBER THIS DEFECT COSTS: the loop stops on the FIRST dialog, with ZERO turns \
             judged. Every milestone that touches a file ends here",
        );

        // ── AND THE SAME EVERYTHING, PLUS ONE CLAUSE ──
        let consent = crate::consent::Consents::of(vec![
            crate::consent::Consent::parse(
                "Do you want to proceed?".to_string(),
                "Yes".to_string(),
            )
            .expect("both needles are non-empty"),
        ])
        .expect("a non-empty consent list");
        let (armed, armed_turns) = run_with(Some(consent));
        assert_eq!(
            armed,
            OutcomeState::Converged,
            "⚠⚠⚠ the caller's own consent must carry the loop THROUGH the dialog and on to its \
             milestone. Nothing else about this run differs from the one above",
        );
        assert!(
            armed_turns.is_some_and(|turns| turns >= 1),
            "⚠⚠ and the agent must have taken a real turn on the other side of the question, or \
             `converged` would be a word about a run that never got past it: {armed_turns:?}",
        );
    }

    /// ⚠⚠⚠ **A LOOP CARRIES OUT ITS AUTHOR'S STANDING INSTRUCTION ON A DIALOG NO CONSENT CAN
    /// ANSWER** — register items 119, 5 and 142, and the state `screening` was built for.
    ///
    /// # ⚠⚠⚠ Why the peer here asks something a consent structurally cannot reach
    ///
    /// The gate above this one arms a [`Consents`](crate::consent::Consents) clause and the run goes
    /// through, which settles the case where the answer is ON THE MENU. This peer asks *"Which way
    /// should I build this?"* and offers *"The quick one"* / *"The thorough one"* — **there is no
    /// option a caller could authorise in advance**, because the whole point of the question is that
    /// the answer is not one of the things being offered. That is the dialog `screen_rules` exist
    /// for, and until this round it ended the run.
    ///
    /// ⚠⚠ **IT IS A PAIR, and the pair is the claim:**
    ///
    /// * **with no rule that claims it**, the run stops and says `no_rule` — naming the dialog, so
    ///   the author learns what to quote;
    /// * **with one rule quoting the dialog**, the same peer and the same brief CONVERGE: the call
    ///   is turned down, the agent is told what to do instead, and it does it.
    ///
    /// ⚠⚠⚠ **AND THE RUN SAYS WHICH KIND OF DECISION IT TOOK.** `screened` is 1 and `answered` is
    /// **0** — measured, because the act refuses rather than approves and a person auditing this run
    /// must not find it counted among the things their agent was allowed to do.
    #[test]
    fn a_loop_carries_out_the_standing_instruction_its_author_wrote() {
        /// The words the stand-in's dialog carries, which a rule quotes.
        const ASKS: &str = "Which way should I build this?";
        /// What the standing instruction says instead. ⚠ It carries no marker and no `exactly:`, so
        /// a peer that converged off THIS text rather than off its own next turn could not.
        const INSTEAD: &str = "neither; do the smallest verifiable thing and report";

        /// One run against the peer that asks an unanswerable question, with whatever standing
        /// instructions the author left it — and what became of it.
        fn run_with(
            screen_rules: Option<crate::screen::ScreenRules>,
        ) -> (
            crate::driver::Outcome,
            Option<i64>,
            String,
            Vec<String>,
            Option<i64>,
        ) {
            run_against(screen_rules, true)
        }

        /// One run against a peer that asks an unanswerable question and either does or does not
        /// take the key that refuses it — and, beside the run's own account, **the DOCUMENT's
        /// count of what it screened**, so the two authorities can be compared.
        fn run_against(
            screen_rules: Option<crate::screen::ScreenRules>,
            takes_the_key: bool,
        ) -> (
            crate::driver::Outcome,
            Option<i64>,
            String,
            Vec<String>,
            Option<i64>,
        ) {
            // ⚠ ONE turn after the redirect, which is what makes the ENDING carry the claim: the
            // peer does what it was told instead and then says the marker. The gate below this one
            // drives the same peer with a number it can never reach, because *"how often is the
            // original milestone asked for again?"* is a question a converging peer cannot answer.
            let (workspace, pane) = crate::testing::standin_agent_refusing(takes_the_key, 1, None);
            let access = crate::testing::supervised_asking(&workspace);
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &Brief {
                    screen_rules,
                    ..brief_for(40)
                },
                // ⚠ NO CONSENT AT ALL, which is the control for the whole gate: nothing this run
                // holds can take an option, so whatever gets it past the dialog is `screening`.
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            // ⚠⚠ THE WALK IS CARRIED INTO EVERY FAILURE MESSAGE BELOW, R378's lesson: a loop that
            // does not reach its ending is diagnosable by its total alone only if nobody thought
            // to keep the journal, and this gate's first run stalled with `exhausted — duration`
            // saying nothing about where.
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &access, &RunContext::uncancellable());
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            let turns = loops.turns();
            let screened = loops.screened();
            // ⚠⚠⚠ THE PANE THE RUN ENDED ON, and not `pane`. A screening now triggers a reflection,
            // and a reflection that is ADOPTED replaces the inner session — so the pane this run was
            // handed is closed, and its echo trail answers `""`. Read it and every assertion about
            // what the loop said becomes an assertion about a pane nobody is driving. ⚠ In the third
            // arm nothing is adopted (the key does not land), so this is the ORIGINAL pane and the
            // negative assertion still means what it says.
            let live = access.pane_ids();
            let typed = live
                .first()
                .and_then(|last| {
                    access
                        .input_echo()
                        .and_then(|echo| echo.pane_recent_input(*last))
                })
                .unwrap_or_default();
            for pane in live {
                access.lifecycle().expect("lifecycle").close(pane);
            }
            (outcome, turns, typed, walk, screened)
        }

        // ── THE DEFECT: A DIALOG THE AUTHOR NEVER QUOTED ──
        //
        // ⚠ The document's shipped rule is an `(edit me)` placeholder that claims nothing, so this
        // is the SHIPPED loop meeting a real question.
        let (unarmed, unarmed_turns, _, unarmed_walk, unarmed_screened) = run_with(None);
        let OutcomeState::Blocked(Some(unanswered)) = &unarmed.state else {
            panic!(
                "⚠⚠⚠ a loop whose agent asks something nothing covers must report BLOCKED with the \
                 question: {:?} — walked {unarmed_walk:?}",
                unarmed.state,
            );
        };
        assert_eq!(
            unanswered.why(),
            crate::consent::Refusal::NoRule,
            "⚠⚠⚠ and the reason must be the one whose remedy is a RULE. `no_consent` here would \
             send an author to write a clause about a menu that offers nothing they could have \
             authorised — the exact dead end this state exists to end: {unanswered:?}",
        );
        assert!(
            unanswered.question().is_some(),
            "⚠⚠ and it must publish the dialog, or the author cannot see what to quote",
        );
        assert_eq!(
            unarmed_turns,
            Some(0),
            "⚠⚠⚠ THE NUMBER THIS COSTS: the loop stops on the FIRST such question, with ZERO turns \
             judged",
        );
        assert_eq!(
            (unarmed.screened, unarmed.answered),
            (0, 0),
            "⚠ the control: a run that got past nothing decided nothing",
        );
        assert_eq!(
            unarmed_screened,
            Some(0),
            "⚠⚠ and the DOCUMENT counts none either — its `screened` is incremented on \
             `screen.matched` and NOT on entering the state, so a loop that only LOOKED at a \
             dialog must not have counted it. Walked {unarmed_walk:?}",
        );

        // ── AND THE SAME EVERYTHING, PLUS ONE STANDING INSTRUCTION ──
        let rules = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let (armed, armed_turns, typed, walk, armed_screened) = run_with(Some(rules));
        assert_eq!(
            armed.state,
            OutcomeState::Converged,
            "⚠⚠⚠ the author's own standing instruction must carry the loop THROUGH the dialog and \
             on to its milestone. Nothing else about this run differs from the one above: {:?} — \
             walked {walk:?}",
            armed.state,
        );
        assert_eq!(
            (armed.screened, armed.answered),
            (2, 0),
            "⚠⚠⚠ AND THE RUN MUST SAY WHICH DECISION IT TOOK. This act REFUSED a call; a run that \
             counted it among the things it approved would answer *what did my agent get to do?* \
             with the opposite fact. ⚠⚠ TWO, not one, since `reflecting` was built: the first \
             screening is ADOPTED, which replaces the session, and this fixture asks on its first \
             prompt whatever it has been told — so the fresh agent asks and the rule fires again. \
             What the adoption buys is visible in the walk rather than in this number: the second \
             reflection is `ReflectNone` and costs no restart. Walked {walk:?}",
        );
        assert_eq!(
            armed_screened.map(|count| u32::try_from(count).unwrap_or(u32::MAX)),
            Some(armed.screened),
            "⚠⚠⚠ AND THE DOCUMENT'S OWN COUNT MUST AGREE WITH THE RUN'S. There are two authorities \
             over this one fact — `screened` in the datamodel, incremented on `screen.matched`, and \
             the Driver's tally of `Verdict::Screened` steps — and two numbers nobody compares is \
             how one of them quietly becomes folklore. Walked {walk:?}",
        );
        assert!(
            armed_turns.is_some_and(|turns| turns >= 1),
            "⚠⚠ and the agent must have taken a real turn on the other side of the question, or \
             `converged` would be a word about a run that never got past it: {armed_turns:?}",
        );
        assert!(
            typed.contains(INSTEAD),
            "⚠⚠⚠ and the REDIRECT must have reached the pane. A run that refused the call and said \
             nothing would leave the agent turned down with no next thing to do, which is the \
             failure `Malformed::SaysNothing` refuses at construction. Typed: {typed:?}",
        );

        // ── ⚠⚠⚠ AND THE THIRD ARM: A DIALOG THAT WILL NOT GO ──
        //
        // The same rule, the same brief, and a peer whose dialog ignores the refusing key. This is
        // the `Tab` arm of the live probe rebuilt where it can be run for free, and what it asserts
        // is a NEGATIVE: **nothing was typed**. A dialog still on the screen reads an Enter as an
        // answer to ITSELF — the probe measured a file being written by exactly this — so a
        // screening act that cannot prove the question is gone must say its piece to nobody.
        let same_rule = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let (stuck, stuck_turns, stuck_typed, stuck_walk, stuck_screened) =
            run_against(Some(same_rule), false);
        let OutcomeState::Blocked(Some(unmoved)) = &stuck.state else {
            panic!(
                "⚠⚠⚠ a rule that fired against a dialog that will not go must end the run BLOCKED, \
                 not carry on as though it had been carried out: {:?} — walked {stuck_walk:?}",
                stuck.state,
            );
        };
        assert_eq!(
            unmoved.why(),
            crate::consent::Refusal::NotDismissed,
            "⚠⚠ and the reason must separate *nothing claimed this* from *something did and the \
             key did not land* — the second is a fact about the AGENT, since the key is the \
             product's: {unmoved:?}",
        );
        assert!(
            !stuck_typed.contains(INSTEAD),
            "⚠⚠⚠ **THE ONE ASSERTION THIS WHOLE ACT IS ORDERED FOR.** The redirect must NOT have \
             reached a pane whose dialog is still up. A live probe typed into exactly this and the \
             Enter behind it APPROVED THE FILE WRITE the agent had asked permission for — and \
             `deliver` reported the text confirmed on screen while it happened. Typed: \
             {stuck_typed:?}",
        );
        assert!(
            unmoved.bytes() > 0,
            "⚠⚠ THE CONTROL: the refusing key really was pressed, or the assertion above is about \
             an act that never began: {unmoved:?}",
        );
        assert_eq!(
            (stuck.screened, stuck_turns, stuck_screened),
            (0, Some(0), Some(0)),
            "⚠⚠ and NEITHER authority counted it, and no turn was judged — a run that counted this \
             among the calls it got past would be reporting a decision it did not manage to take. \
             Walked {stuck_walk:?}",
        );
        // ⚠⚠⚠ AND THE KEY WAS CHARGED FOR. Every other plugin reports `Cost::Bytes(asking.bytes())`
        // on a `Blocked`, and a loop reported zero — true for as long as a loop could not type at a
        // dialog, and false the moment `screening` could press a key and give up. A ceiling that
        // cannot see what a run typed into somebody's dialog is a ceiling with a hole in it.
        assert!(
            matches!(stuck.cost, Some(crate::plugin::Cost::Bytes(spent)) if spent >= unmoved.bytes()),
            "⚠⚠ the run must charge for the refusing key it really pressed. The refusal says it \
             cost {} and the run reports {:?}",
            unmoved.bytes(),
            stuck.cost,
        );
    }

    /// ⚠⚠⚠ **A STANDING INSTRUCTION REACHES EVERY PROMPT ONCE IT HAS BEEN ADOPTED** — register item
    /// 148, paid; and the LIVE AGENT of R384 is what reported the defect, mid-gate, in its own words:
    ///
    /// > *"the loop is re-issuing the original milestone, but your last direct instruction was to
    /// > not create that file … 루프가 매 턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절하게
    /// > 되므로, 진전이 없습니다."*
    ///
    /// # ⚠⚠⚠ THE NUMBERS, BEFORE AND AFTER, on the same six-turn run
    ///
    /// **BEFORE** — this gate's first form, and what it asserted: the instruction reached the pane
    /// exactly **ONCE**, `turn_prompt` asked for the original milestone **SIX** times, and
    /// `authored().turn` did not carry the instruction at all. The loop spent its budget asking for
    /// something its author had already forbidden, and a peer that keeps refusing makes no progress.
    ///
    /// **AFTER** — the same peer and the same rule, with `reflecting` and `restarting` built: the
    /// screening triggers a reflection at the very next judgement, the session is replaced, and from
    /// then on **every prompt that names the milestone carries the instruction with it**. So the two
    /// counts no longer diverge — that is the whole claim, and it is asserted as a relation between
    /// them rather than as one number, because what matters is that no prompt asks for the milestone
    /// without saying what overrides it.
    ///
    /// ⚠⚠ **THE `authored()` HALF IS THE SHARPER ONE.** What went into the pane is history; what the
    /// loop is GOING to say next is `turn_prompt`, and after a reflection it carries the instruction —
    /// including `start_prompt`, which is what a FRESH agent that remembers nothing is greeted with.
    ///
    /// ⚠⚠⚠ **AND THE PANE IT READS IS THE ONE THE RUN ENDED ON**, not the one it started over. A
    /// reflection closes the inner session, so the echo trail of the original pane holds only the
    /// first session's prompts — a gate that kept reading it would report the BEFORE numbers for ever
    /// and call them a pass.
    #[test]
    fn a_standing_instruction_reaches_every_prompt_once_it_has_been_adopted() {
        /// The dialog the peer raises, quoted by the rule below.
        const ASKS: &str = "Which way should I build this?";
        /// What the author's standing instruction says instead. ⚠ Distinctive, and carrying neither
        /// `exactly:` nor `Summarise`, so counting it counts only the screening act.
        const INSTEAD: &str = "not that way; do the smallest verifiable thing and report";
        /// The brief's milestone. ⚠ It must share no substring with `done_instruction` — which
        /// carries the words `MILESTONE REACHED` into every single prompt — or the count below
        /// would be counting the marker.
        const AIM: &str = "the-original-thing";
        /// The turn budget. ⚠ Small deliberately: the pane's echo trail is
        /// [`ECHO_TRAIL_CAP`](sprag_terminal::pane_pty::ECHO_TRAIL_CAP) bytes and a longer run would
        /// make the counts a question about that bound instead.
        const TURNS: i64 = 6;

        // ⚠ A peer that NEVER converges after the redirect — the shape the live agent took. It does
        // what it was redirected to, comes back, and is asked for the original milestone again.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, u32::MAX, None);
        let access = crate::testing::supervised_asking(&workspace);
        let rules = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                milestone: AIM.to_owned(),
                screen_rules: Some(rules),
                ..brief_for(TURNS)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 200,
            max_cost: None,
            max_duration: Some(Duration::from_secs(120)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        // ⚠⚠⚠ THE PANE THE RUN ENDED ON, which is not the one it started over: a reflection replaced
        // the inner session. The original is closed, so `pane_ids` holds exactly the survivor — and
        // reading the OLD pane's echo trail is how this gate would go on reporting the BEFORE numbers
        // for ever. `driving()` cannot be asked, and correctly so: an `exhausted` loop answers `None`
        // because its peer is at rest.
        let live = access.pane_ids();
        let typed = access
            .input_echo()
            .and_then(|echo| echo.pane_recent_input(*live.first().expect("a surviving pane")))
            .unwrap_or_default();
        let authored = loops.authored().expect("the datamodel still answers");
        let turns = loops.turns();
        for pane in &live {
            access.lifecycle().expect("lifecycle").close(*pane);
        }

        // ⚠ THE CONTROLS FIRST. Without these the counts below could both be about a run that never
        // got past the dialog, or one whose echo trail had wrapped.
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "the peer never says the marker, so this run must end on the DOCUMENT's turn budget — \
             anything else and the counts below are about a different run: walked {walk:?}",
        );
        assert_eq!(
            live.len(),
            1,
            "⚠⚠ and exactly ONE pane must survive the run: the replacement, with the session it \
             replaced closed. Two would mean an agent was left running",
        );
        assert_eq!(
            (turns, outcome.screened),
            (Some(TURNS), 2),
            "⚠⚠ every turn spent, and the rule fired TWICE — once per session. The replacement is a \
             FRESH agent that has never been turned down, so this fixture (which asks on its first \
             prompt whatever it is told) asks again; what the adoption buys is that the second time \
             costs no restart. Walked {walk:?}",
        );
        assert!(
            typed.contains("North star: "),
            "⚠ and the echo trail must still hold the FIRST prompt of that session, or these counts \
             are a question about `ECHO_TRAIL_CAP` rather than about the loop: {typed:?}",
        );

        // ── ⚠⚠⚠ THE CLAIM, AS A RELATION BETWEEN THE TWO COUNTS ──
        let said = typed.matches(INSTEAD).count();
        let asked = typed.matches(AIM).count();
        assert!(
            asked > 0 && said >= asked,
            "⚠⚠⚠ ITEM 148: no prompt may ask for the milestone without carrying what overrides it. \
             Before `reflecting` existed these were ONE and SIX. The instruction is asked-for {asked} \
             times and said {said} times. Walked {walk:?}",
        );
        assert!(
            authored.turn.contains(AIM) && authored.turn.contains(INSTEAD),
            "⚠⚠⚠ AND THE SHARPER HALF: what the loop will say NEXT carries both, so the relation \
             above is not a history — it is the loop's standing behaviour: {:?}",
            authored.turn,
        );
        assert!(
            authored.start.contains(INSTEAD),
            "⚠⚠⚠ AND `start_prompt` MOST OF ALL: it is what the next replacement session is greeted \
             with, and that agent remembers nothing of having been redirected: {:?}",
            authored.start,
        );
    }

    /// ⚠⚠⚠ **THE STATE THE DOOR USED TO REFUSE IS ONE A RUN NOW WALKS THROUGH** — register item 6,
    /// and item 148's answer end to end.
    ///
    /// # ⚠⚠⚠ What this gate was, and why it is the same gate
    ///
    /// It measured a REFUSAL's premise: [`AiLoop::new`] turned away `reflect_every < max_turns`
    /// because such a brief reaches `reflecting`, and this drove the loop through [`OuterLoop`] — the
    /// layer under the door — to prove the state was genuinely reachable rather than argued for from
    /// reading the document. The premise held. **Now the state is built, so the same drive is kept
    /// and the assertion moves from *it arrives* to *it gets through*** — the standing rule that a
    /// gate which measured a defect is repurposed, never deleted.
    ///
    /// # ⚠⚠⚠ The five things a session replacement has to get right, all asserted here
    ///
    /// 1. **THE WALK**, as four separate states and not one: `reflecting` decides, `restarting`
    ///    replaces, `resuming` waits for the fresh agent, `priming` recomposes. A driver that held
    ///    the phase itself would replace the pane once per pump.
    /// 2. **A DIFFERENT PANE**, and the old one GONE. A replacement that left the previous session
    ///    running would leave two agents on one milestone, both spending tokens.
    /// 3. **THE SAME COMMAND, DIRECTORY AND SIZE** — read off the pane rather than supplied, so a
    ///    caller cannot be given a `claude` where they launched `claude --resume`, or the daemon's
    ///    directory where the work is.
    /// 4. ⚠⚠⚠ **THE PROMPTS CARRY THE STANDING INSTRUCTION**, which is what the whole restart is
    ///    FOR: the measurement gate above counts one delivery against six re-issues of the milestone
    ///    it overrides, and after a reflection the instruction is in `start_prompt` — the one a FRESH
    ///    agent, which remembers nothing, is greeted with.
    /// 5. **`driving()` NAMES THE NEW PANE.** A cancelled run signals what `driving` answers, so a
    ///    stale answer means the model in the live pane goes on working while the run reports having
    ///    stopped it.
    #[test]
    fn a_reflection_replaces_the_session_and_the_new_one_is_told_what_was_learned() {
        /// The dialog the peer raises, which the rule below quotes.
        const ASKS: &str = "Which way should I build this?";
        /// The standing instruction. ⚠ Distinctive enough to find in a composed prompt.
        const INSTEAD: &str = "not that way; do the smallest verifiable thing and report";

        let (workspace, pane) = crate::testing::standin_agent_refusing(true, u32::MAX, None);
        let access = crate::testing::supervised_asking(&workspace);
        // ⚠ Read BEFORE the replacement: these are what the fresh pane has to match, and the pane
        // that carries them is about to be closed.
        let (argv, cwd, size) = {
            let guard = workspace.lock().expect("the workspace mutex");
            let old = guard.pane(pane).expect("the pane the loop was given");
            (old.argv().to_vec(), old.pty().cwd(), old.pty().dimensions())
        };

        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                screen_rules: Some(
                    crate::screen::ScreenRules::of(vec![
                        crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                            .expect("both halves are non-empty"),
                    ])
                    .expect("a non-empty list"),
                ),
                // ⚠ THE BUDGET IS OFF (equal pair), so the reflection below is caused by the
                // STANDING INSTRUCTION and by nothing else. A gate that left `reflect_every` small
                // could not tell the correctness edge from the budget one.
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        assert_eq!(
            loops.driving(),
            Some(pane),
            "⚠ the control: the loop must be driving the pane it was GIVEN before it replaces it, or \
             the assertion at the end is true for the wrong reason",
        );

        // ⚠⚠ PUMPED THROUGH THE PLUGIN, one step at a time, so the gate can STOP at the far side of
        // the restart. Driven to convergence this fixture would ask again from its fresh session and
        // reflect again for ever, which is honest behaviour and a different claim.
        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let mut replaced = None;
        while replaced.is_none() {
            assert!(
                walked.len() < 40,
                "⚠ this gate's own bound. Walked: {walked:?}",
            );
            let before = loops.state();
            let step = loops
                .step(&access, &run)
                .expect("every step of a replacement must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
            // The far side: the machine is composing again, on a pane that is not the one it started
            // over.
            if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                replaced = Some(loops.driving().expect("a loop mid-run drives its pane"));
            }
        }
        let fresh = replaced.expect("the loop reached `priming` through a replacement");
        let authored = loops.authored().expect("the datamodel still answers");
        let held = {
            let guard = workspace.lock().expect("the workspace mutex");
            (
                guard.pane(pane).is_some(),
                guard
                    .pane(fresh)
                    .map(|new| (new.argv().to_vec(), new.pty().cwd(), new.pty().dimensions())),
            )
        };
        access.lifecycle().expect("lifecycle").close(fresh);

        // ── 1. THE WALK, as four states ──
        for edge in [
            "Judging --Judge--> Reflecting",
            "Reflecting --ReflectApplied--> Restarting",
            "Restarting --SessionReplaced--> Resuming",
            "Resuming --SessionReady--> Priming",
        ] {
            assert!(
                walked.iter().any(|note| note == edge),
                "⚠⚠⚠ the replacement must be these FOUR acts and the run's journal must say so — \
                 `{edge}` is missing. Walked {walked:?}",
            );
        }

        // ── 2. A DIFFERENT PANE, and the old one gone ──
        assert_ne!(
            fresh, pane,
            "⚠⚠⚠ a restart must open a NEW session; the same pane back means nothing was replaced",
        );
        assert!(
            !held.0,
            "⚠⚠⚠ and the OLD one must be gone. A replacement that left it running leaves two agents \
             working on one milestone, both spending somebody's tokens",
        );

        // ── 3. THE SAME COMMAND, DIRECTORY AND SIZE ──
        assert_eq!(
            held.1,
            Some((argv, cwd, size)),
            "⚠⚠⚠ the fresh session must be the SAME program in the SAME directory at the SAME size. \
             The pane is the only authority on what it was running, which is why `respawn` takes no \
             argv — a loop that supplied an agent NAME would restart `claude` where somebody had \
             launched `claude --resume`, in the daemon's directory rather than the repository",
        );

        // ── 4. ⚠⚠⚠ THE PROMPTS NOW CARRY WHAT THE RUN LEARNED ──
        assert!(
            authored.start.contains(INSTEAD),
            "⚠⚠⚠ THE WHOLE POINT. `start_prompt` is what the FRESH agent is greeted with, and it \
             remembers nothing of having been redirected — so a loop that carried its standing \
             instructions only in `turn_prompt` would hand its replacement a clean slate and the \
             original milestone: {:?}",
            authored.start,
        );
        assert!(
            authored.turn.contains(INSTEAD),
            "⚠⚠ and every later turn carries it too, which is what makes it STANDING rather than \
             said once: {:?}",
            authored.turn,
        );

        // ── 5. `driving()` NAMES THE LIVE PANE ──
        assert_eq!(
            loops.driving(),
            Some(fresh),
            "⚠⚠⚠ and a cancelled run must interrupt the session that EXISTS. This type held a `pane` \
             field until this round, which was correct for exactly as long as a loop could not \
             replace its own",
        );
    }

    /// ⚠⚠⚠ **A REFLECTION WITH NOTHING TO CARRY DOES NOT PAY FOR A RESTART** — the other exit of
    /// `reflecting`, and the one that stops the feature eating every run that uses it.
    ///
    /// # ⚠⚠⚠ Why this is the sharper half of the pair
    ///
    /// Closing an agent's pane and opening a fresh one throws away that session's whole context. It
    /// is worth it to make a standing instruction stick, and it is worth NOTHING when there is no
    /// instruction to make stick — so a driver that answered `reflect.applied` whenever it was asked
    /// would replace the session on every multiple of `reflect_every`, for ever, having changed
    /// nothing. The document already has the word for that (*"Nothing worth changing: carry on
    /// without paying for a restart"*); this is the gate that says the driver uses it.
    ///
    /// ⚠⚠ **THE PANE IS THE ASSERTION.** A run that reflected and restarted still converges and still
    /// reports the same turn count, so the outcome cannot tell the two apart — only the pane can, and
    /// only by being the SAME one it started with.
    ///
    /// ⚠ `reflect_every: 1` is the sharpest arrangement of the budget: due after the very first turn,
    /// and again after every one. This brief is what the door refused until this round.
    #[test]
    fn a_reflection_with_nothing_to_carry_does_not_pay_for_a_restart() {
        let (workspace, pane) = standin_agent(3);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 1,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a brief that reflects after every turn is one this build drives");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let live = access.pane_ids();
        for pane in &live {
            access.lifecycle().expect("lifecycle").close(*pane);
        }

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the control: a loop reflecting after every turn must still reach its milestone: \
             walked {walk:?}",
        );
        assert!(
            walk.iter()
                .any(|note| note == "Reflecting --ReflectNone--> Working"),
            "⚠⚠⚠ it must really have REFLECTED and found nothing — an assertion about no restart is \
             worth nothing if the loop never reached `reflecting` at all. Walked {walk:?}",
        );
        assert!(
            !walk.iter().any(|note| note.contains("Restarting")),
            "⚠⚠⚠ AND NOTHING WAS REPLACED. A reflection that restarts whenever it is asked throws \
             away an agent's whole context on every multiple of `reflect_every`, having changed \
             nothing. Walked {walk:?}",
        );
        assert_eq!(
            live,
            vec![pane],
            "⚠⚠⚠ THE PANE IS THE ASSERTION: the outcome and the turn count are identical whether or \
             not a session was replaced, so the only witness is that this is the pane the run \
             started with",
        );
    }

    /// ⚠⚠⚠ **THE RUN OUTLIVES ITS AGENT'S CONTEXT: A REFLECTION ASKS THE AGENT WHAT COMES NEXT,
    /// AND THE REPLACEMENT SESSION IS BRIEFED ON THE ANSWER.**
    ///
    /// # ⚠⚠⚠ What this gate measured before the feature existed, and why it was kept
    ///
    /// It was written as a MEASUREMENT of register item 167 and its drive has not changed. What it
    /// asserted then was the defect, in three parts:
    ///
    /// * the loop reflected (`Reflecting --ReflectNone--> Working`) and **replaced nothing**, because
    ///   the only thing a reflection could carry was a standing instruction — so a run whose agent
    ///   was never screened could not get a fresh session at all;
    /// * the milestone label never reached the pane: **the agent was never asked**;
    /// * and every later prompt still named the milestone the CALLER wrote, for as long as the run
    ///   lasted.
    ///
    /// That is what makes a bounded run bounded by one agent's context: whatever the work turned out
    /// to need, the loop could only ever ask again for the checkpoint somebody wrote before it
    /// started. A run meant to carry on across sessions — pay this debt, then the next one — would
    /// re-issue the first one for ever.
    ///
    /// ⚠⚠ **THE PEER IS THE INSTRUMENT AND IT WAS WILLING THE WHOLE TIME.**
    /// [`standin_agent_reflecting`](crate::testing::standin_agent_reflecting) answers the milestone
    /// label with a next milestone and a next reference. Before the reflection turn it was never
    /// spoken to, which is the honest shape of the defect: nothing was broken, something was never
    /// asked.
    ///
    /// ⚠⚠⚠ **AND THE ECHO IS THE TRAP, DELIBERATELY.** The prompt that asks for the label carries the
    /// label, the peer paints every line it is handed, and the pane is 80 columns — so the driver's
    /// reader meets its own instruction on screen before the answer. The assertions name the peer's
    /// words rather than *"something was adopted"*, so a reader that took the first match would
    /// adopt a milestone made of the prompt and fail here.
    #[test]
    fn a_reflection_asks_the_agent_what_comes_next_and_the_replacement_is_told() {
        /// The milestone the CALLER wrote. ⚠ Distinctive, and it must not survive the reflection.
        const ORIGINAL: &str = "the checkpoint whoever started this run wrote down";
        /// What the agent decides instead, once it is asked.
        const PROPOSED: &str = "the debt the agent picked after doing the work";
        /// And what it says the next session should read first.
        const READ_NEXT: &str = "the register entry it had just been reading";

        let (workspace, pane) = crate::testing::standin_agent_reflecting(9, PROPOSED, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                milestone: ORIGINAL.to_string(),
                // ⚠ The peer needs NINE prompts to say the marker and the budget reflects after
                // TWO, so the reflection is reached by the turn budget alone — no screening, no
                // standing instruction, which is exactly the run the defect was about.
                reflect_every: 2,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let mut replaced = None;
        // ⚠ THE FIRST SESSION'S SCREEN, KEPT AS THE RUN GOES. `restarting` CLOSES this pane, so a
        // read taken after the assertion loop would be about a pane that no longer exists — and the
        // claim is that the label reached the session that was ASKED.
        let mut asked = String::new();
        while replaced.is_none() && walked.len() < 60 {
            let before = loops.state();
            let step = loops
                .step(&access, &run)
                .expect("every step of a reflection must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
            let showing = access.pane_full_lines(pane).unwrap_or_default().join("\n");
            if !showing.trim().is_empty() {
                asked = showing;
            }
            if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                replaced = Some(loops.driving().expect("a loop mid-run drives its pane"));
            }
            if matches!(
                loops.state(),
                AiLoopState::Converged
                    | AiLoopState::Exhausted
                    | AiLoopState::Failed
                    | AiLoopState::Cancelled
                    | AiLoopState::Blocked
            ) {
                break;
            }
        }
        let authored = loops.authored().expect("the datamodel still answers");
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        // ── THE CONTROL: it really did reflect, and it really did replace ──
        assert!(
            walked
                .iter()
                .any(|note| note == "Reflecting --ReflectApplied--> Restarting"),
            "⚠⚠⚠ the reflection must have been ADOPTED — every assertion below is about what a \
             replacement session was told, and is worth nothing if the run never reached one. \
             Walked {walked:?}",
        );
        assert!(
            replaced.is_some_and(|fresh| fresh != pane),
            "⚠⚠⚠ and a NEW session must exist to be told: a reflection that decided a milestone \
             and kept the old context has improved nothing a fresh agent will read. Walked \
             {walked:?}",
        );

        // ── 1. THE AGENT WAS ASKED, IN THE DOCUMENT'S OWN WORDS ──
        assert!(
            authored
                .reflect
                .contains(crate::testing::REFLECTION_MILESTONE_LABEL),
            "⚠⚠⚠ the composed `reflect_prompt` must carry the label the peer answers, or this gate \
             is measuring a fixture that agrees with a private word: {:?}",
            authored.reflect,
        );
        assert!(
            authored
                .reflect
                .contains(crate::testing::REFLECTION_ECHO_SLICE),
            "⚠⚠⚠ and it must still carry the words the peer paints back behind the label. That row \
             is this gate's ECHO — the shape a wrapped prompt has on a pane of another width — and \
             it only tests the reader's echo rule for as long as it really is a slice of what the \
             loop said. Prompt: {:?}",
            authored.reflect,
        );
        assert!(
            asked.contains(crate::testing::REFLECTION_MILESTONE_LABEL),
            "⚠⚠⚠ and it must have REACHED THE PANE. This is the half that was missing: a loop that \
             reflects without asking can only ever carry what its author wrote",
        );

        // ── 2. THE ANSWER IS WHAT THE REPLACEMENT IS BRIEFED WITH ──
        assert!(
            authored.start.contains(PROPOSED),
            "⚠⚠⚠ THE WHOLE POINT. `start_prompt` is what the FRESH agent is greeted with, and it \
             must name the milestone THE AGENT chose — not the one the caller wrote before any of \
             the work had been done: {:?}",
            authored.start,
        );
        assert!(
            authored.start.contains(READ_NEXT),
            "⚠⚠ and what to read first, which is the other half of what a fresh context needs: {:?}",
            authored.start,
        );
        assert!(
            !authored.start.contains(ORIGINAL),
            "⚠⚠⚠ and the caller's original milestone must be GONE. A run that carried both would \
             hand a fresh agent two checkpoints and no way to tell which is current: {:?}",
            authored.start,
        );
        assert!(
            authored.turn.contains(PROPOSED),
            "⚠⚠ every later turn of the replacement session works toward the same new milestone: \
             {:?}",
            authored.turn,
        );

        // ── 3. AND IT IS THE AGENT'S ANSWER, NOT THE TWO THINGS THAT LOOK LIKE ONE ──
        assert!(
            !authored
                .start
                .contains(crate::testing::REFLECTION_ECHO_SLICE),
            "⚠⚠⚠ the reader must DISCOUNT WHAT THIS LOOP ITSELF SAID. The peer paints a row that \
             opens with the label and carries a verbatim slice of the prompt — which is exactly \
             what a wrapped echo looks like — and adopting it would make the run's own goal out of \
             the instruction it had just sent: {:?}",
            authored.start,
        );
        assert!(
            !authored
                .start
                .contains(crate::testing::REFLECTION_PROVISIONAL),
            "⚠⚠ and where the agent named more than one, the LAST is its answer: an agent asked \
             for two lines writes a paragraph first, and the row it settled on is the one below: \
             {:?}",
            authored.start,
        );
    }

    /// ⚠⚠⚠ **A QUESTION NOBODY WROTE A RULE FOR PAUSES THE RUN, AND A PERSON'S ANSWER RESUMES IT** —
    /// `awaiting_human`, the last state of `ai_loop.scxml` this driver had not built.
    ///
    /// # ⚠⚠⚠ What was there before, and why it was wrong rather than incomplete
    ///
    /// The driver answered `Pumped::Unbuilt` for this state and the [`Driver`] stopped the run. The
    /// scope note that shipped with it read *"a rule that claims nothing ends the run exactly as an
    /// unanswered dialog always has"* — a true sentence about the DRIVER and a false one about the
    /// machine. `awaiting_human` has seven edges and six of them are ways to carry on; the document
    /// says the loop *"stops prompting and waits"*, and that *"if the person answers the dialog, the
    /// agent goes back to work and the loop resumes where it was."*
    ///
    /// **A state machine with no input stays in the state.** Ending the run instead was the driver
    /// deciding something the document does not say — which is the one thing this whole arrangement
    /// exists to prevent.
    ///
    /// # ⚠⚠ The two halves, and why the first one needs a control
    ///
    /// 1. **IT WAITS.** Pumped repeatedly with the dialog up and nobody there, the machine stays in
    ///    `awaiting_human` and the run does not end. ⚠ The control is that it is pumped MANY times:
    ///    a single pump would pass for a driver that ends the run on its second look.
    /// 2. **A PERSON'S KEYSTROKE MOVES IT ON.** The person presses the key the peer is waiting for,
    ///    the agent finishes the turn it was blocked in, and `awaiting_human --turn.done--> judging`
    ///    carries the run to its milestone. ⚠ The keystroke is written as
    ///    [`Hand::APerson`](sprag_terminal::Hand::APerson), which is what a person's hand at a real
    ///    pane looks like to this product.
    #[test]
    fn a_question_no_rule_claims_pauses_the_run_and_a_person_resumes_it() {
        /// How many pumps the loop must sit through before anybody touches the pane. ⚠ Large enough
        /// that a driver ending the run *eventually* fails here rather than passing.
        const WAITED: usize = 12;

        // ⚠ The peer asks ONE question and takes Escape for an answer — so the "person" below is
        // pressing the key this dialog is actually waiting for, not a key the fixture invented.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, 2, None);
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠⚠ NO SCREEN RULES AT ALL — this is the shipped shape, whose placeholder claims
            // nothing. The dialog therefore reaches `screening`, no rule takes it, and
            // `screen.none` leads here. That is the commonest way a real run meets this state.
            &brief_for(40),
            // ⚠ A SHORTER TURN BOUND THAN THE OTHER GATES', and it is about this gate's COST rather
            // than its claim: a pump that finds nothing blocks for the turn's whole patience, and
            // this one deliberately pumps many times with nothing happening. ⚠ It stays above
            // `supervised_asking`'s 300 ms settle, or no turn could ever be seen to end.
            &AiLoopSpec {
                turn: Turn::lasting(INNER_SESSION_ENDS, Some(Duration::from_secs(1)))
                    .expect("a non-zero bound"),
                // ⚠⚠⚠ A PERSON IS DECLARED, AND THIS GATE IS WHERE THAT STOPPED BEING OPTIONAL.
                // With `Attended::NoOne` — every other gate's value, and the default — a person's
                // hand at the pane is a TAKEOVER for ever after: measured here as
                // `AwaitingHuman --TurnDone--> Judging --Judge--> Working
                // --TurnInterrupted--> AwaitingHuman`, round and round, because the barrier went on
                // reporting the keystroke that unblocked the dialog. That is the honest reading of
                // `NoOne` (*nobody is watching, so a hand means somebody took the pane*) and the
                // wrong contract for a run whose whole point is that a person may answer it.
                // `WhenStill` is what says the pane is the run's again once they have finished.
                attended: crate::readiness::Attended::of(
                    Duration::from_secs(30),
                    crate::readiness::Handback::of(Duration::from_millis(300))
                        .expect("a non-zero stillness"),
                )
                .expect("a non-zero patience"),
                ..standin_spec()
            },
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let step = |loops: &mut AiLoop, walked: &mut Vec<String>| {
            let step = loops
                .step(&access, &run)
                .expect("every step of a paused run must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
        };

        // ── 1. IT REACHES THE PAUSE ──
        let mut reached = false;
        for _ in 0..40 {
            step(&mut loops, &mut walked);
            if loops.state() == AiLoopState::AwaitingHuman {
                reached = true;
                break;
            }
        }
        assert!(
            reached,
            "⚠ the control: a dialog no rule claims must reach `awaiting_human`, or what follows is \
             about a state the run never entered. Walked {walked:?}",
        );

        // ── 2. AND IT STAYS THERE, FOR AS LONG AS NOBODY COMES ──
        for look in 0..WAITED {
            step(&mut loops, &mut walked);
            assert_eq!(
                loops.state(),
                AiLoopState::AwaitingHuman,
                "⚠⚠⚠ A STATE MACHINE WITH NO INPUT STAYS IN THE STATE. Nobody has touched this pane \
                 and the document's every exit from `awaiting_human` is something that HAPPENS — so \
                 look {look} of {WAITED} must find the loop exactly where the last one left it. \
                 This driver used to answer `Unbuilt` here and the run was over. Walked {walked:?}",
            );
        }

        // ── 3. THE PERSON ANSWERS, AND THE RUN CARRIES ON ──
        crate::testing::person_types(&access, pane, &[27]);
        // ⚠⚠⚠ WALL CLOCK, AND IT IS LOAD-BEARING RATHER THAN A SLEEP TO BE TIDIED AWAY. A real
        // supervisor's verdict SETTLES — `supervised_asking` models that with a 300 ms window — and
        // a gate that pumps in a tight loop polls FASTER than the window it is waiting out: sixty
        // pumps went by in under 300 ms, every one of them reading the answered dialog as still
        // blocked, and the run looked stuck when it was merely being asked too quickly. A real
        // [`Driver`] paces itself; a `while` loop does not.
        //
        // ⚠ The samples are kept because they are what the failure message needs: *what did the
        // supervisor actually say* is the first question of any turn that does not end.
        let mut samples: Vec<String> = Vec::new();
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(200));
            let seen = access
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(pane));
            samples.push(format!(
                "{:?}/{:?}",
                seen.as_ref().map(|s| s.state),
                seen.as_ref().map(|s| s.seq)
            ));
        }
        for _ in 0..60 {
            step(&mut loops, &mut walked);
            if matches!(
                loops.state(),
                AiLoopState::Converged
                    | AiLoopState::Exhausted
                    | AiLoopState::Failed
                    | AiLoopState::Cancelled
                    | AiLoopState::Blocked
            ) {
                break;
            }
        }
        let reached = loops.state();
        // ⚠ WHAT THE ASSERTION ACTUALLY SAW, printed rather than theorised about — the recorded
        // rule that a green (or red) mutation is re-read from the screen before it is diagnosed.
        let showing = access.pane_full_lines(pane).unwrap_or_default().join("\n");
        let seen = access
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(pane));
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        assert!(
            walked
                .iter()
                .any(|note| note == "AwaitingHuman --TurnDone--> Judging"),
            "⚠⚠⚠ THE DOCUMENT'S OWN EDGE: *\"the person answered (or typed a turn themselves) and it \
             completed\"*. Without it the run either never noticed the keystroke or had already been \
             ended by the driver. The pane was showing:\n{showing}\nThe supervisor said \
             {seen:?}\nSamples over the second after the keystroke: {samples:?}\nWalked {walked:?}",
        );
        assert_eq!(
            reached,
            AiLoopState::Converged,
            "⚠⚠ and the run must go on to REACH ITS MILESTONE. A pause that resumes into anything \
             else has moved the loop somewhere the person's answer did not ask for. Walked \
             {walked:?}",
        );
    }

    /// ⚠⚠⚠ **A REPLACEMENT SESSION THAT COMES UP ASKING ENDS THE RUN, AND THE RUN SAYS SO** — the
    /// `resuming` edge a person meeting this feature is likeliest to hit.
    ///
    /// # ⚠⚠⚠ Why this is the arm worth a fixture of its own
    ///
    /// A fresh agent CLI does not always come up ready to be typed at. A trust prompt, a model picker,
    /// a sign-in — `sprag-detect` holds captures of five such screens from two real agents, and every
    /// one of them is shown BEFORE the agent works. So the pane a `restarting` opens can be a pane
    /// nothing this run holds may answer, and the document is explicit about what that means:
    /// *"a session that will not come back is a failed run, not a stuck one."*
    ///
    /// ⚠⚠ **AND THE SENTENCE IS THE ASSERTION.** This round's own sweep found the defect: `resuming`
    /// recorded the question it was refused by, and `AiLoop::ended`'s `failed` arm knew only about a
    /// datamodel that had stopped answering — so the run's single sentence said *"it reached `failed`
    /// and recorded no reason"* about a run holding the reason, and the question was dropped. A caller
    /// would have been told a path nobody wrote had been taken.
    ///
    /// ⚠ The peer cannot know which of its lives it is in, because a replacement runs the same argv on
    /// purpose. It asks the FILESYSTEM — see [`standin_agent_refusing`](crate::testing).
    #[test]
    fn a_replacement_session_that_comes_up_asking_ends_the_run_saying_what_it_asked() {
        /// The dialog the peer raises on its FIRST life, which the rule below claims.
        const ASKS: &str = "Which way should I build this?";
        /// The redirect. ⚠ It leaves the milestone unmet, so the judgement after it reflects.
        const INSTEAD: &str = "not that way; report and wait";

        // ⚠ ONE FILE, not a directory, so the cleanup below is a single removal and a panic leaks
        // nothing a person has to go and find. The name carries the pid and a nanosecond count for the
        // reason every fixture path in this workspace does: two runs of this suite must not share it.
        let marker = std::env::temp_dir().join(format!(
            "sprag-second-life-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos()),
        ));
        let _ = std::fs::remove_file(&marker);
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, 1, Some(&marker));
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                screen_rules: Some(
                    crate::screen::ScreenRules::of(vec![
                        crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                            .expect("both halves are non-empty"),
                    ])
                    .expect("a non-empty list"),
                ),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(90)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        for pane in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(pane);
        }
        let _ = std::fs::remove_file(&marker);

        assert!(
            walk.iter()
                .any(|note| note == "Restarting --SessionReplaced--> Resuming"),
            "⚠ THE CONTROL: the run must have got as far as opening a replacement, or what follows is \
             about a session that was never replaced. Walked {walk:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⚠⚠⚠ a replacement that came up asking something nothing could answer must END the run. \
             Carrying on would mean typing a prompt at a pane showing somebody else's question, which \
             is the one thing this crate's barrier exists to prevent. Walked {walk:?}",
        );
        let said = format!("{:?}", outcome.failure);
        assert!(
            said.contains("came up asking") && said.contains("Which way"),
            "⚠⚠⚠ AND THE SENTENCE MUST CARRY THE QUESTION. Until this round it said *it reached \
             `failed` and recorded no reason* — about a run that had recorded one, holding the very \
             dialog its caller needs to see: {said:?}",
        );
    }

    /// ⚠⚠⚠ **A CANCELLED LOOP STOPS THE AGENT'S TURN IT SET GOING** — [`Plugin::driving`]'s whole
    /// reason, on the plugin whose peer is the most expensive one in this tree.
    ///
    /// A run has two endings that can land while a step is blocked — somebody cancels it, or its
    /// deadline passes — and both can arrive while a real agent is mid-turn. A plugin that answered
    /// `None` here would have the Driver report `cancelled` while the model it prompted went on
    /// spending somebody's tokens on a question nothing is waiting for.
    ///
    /// ⚠⚠ **THE STOP IS THE ASSERTION**, not the word `cancelled`: a run reports `cancelled` with
    /// or without this, and [`Stopped`] is the only thing that says what became of the work.
    #[test]
    fn a_cancelled_loop_stops_the_turn_its_agent_was_taking() {
        /// ⚠⚠⚠ A BUDGET NO 600-MILLISECOND RUN CAN SPEND, and it has to be said why it is this large.
        ///
        /// This gate asked for FORTY turns and passed for two rounds — because the stand-in's
        /// supervisor counted words on a screen, so `seq` stopped growing at the first scroll and
        /// every turn after the third burned the whole five-second turn patience. The run was still
        /// inside turn one when the cancel landed. With the counter read off the peer instead
        /// ([`peer_seq`](crate::testing)) a turn takes milliseconds, forty of them fit inside the
        /// cancel window, and the run reported `exhausted — turns`: **the correct answer to a
        /// question this gate is not asking.**
        ///
        /// So the ceilings are put out of reach and the cancel is the only ending available, which is
        /// what *"a person's stop is the run's ending, above every ceiling"* needs in order to mean
        /// anything.
        const UNSPENDABLE: i64 = 1_000_000;

        // ⚠ NEVER says the marker, so the run is still mid-loop when the cancel lands.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(UNSPENDABLE), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        let cancel = Arc::new(AtomicBool::new(false));
        let run = RunContext::new(Arc::clone(&cancel));
        let raiser = {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || {
                let armed = Instant::now();
                // Long enough that the loop is past `idle` and actually driving the peer, short
                // enough that the gate stays cheap.
                while armed.elapsed() < Duration::from_millis(600) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                cancel.store(true, Ordering::Release);
            })
        };
        let outcome = Driver::new(Guardrails {
            // ⚠ The substrate's ceilings are put out of reach for `UNSPENDABLE`'s reason: the
            // iteration count is the one a fast peer reaches first, and this gate is about neither.
            max_iterations: u32::try_from(UNSPENDABLE).expect("a positive budget"),
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &run);
        raiser.join().expect("the canceller");

        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "a person's stop is the run's ending, above every ceiling: {:?}",
            outcome.state,
        );
        assert!(
            matches!(outcome.stopped, Some(Stopped::Job(_))),
            "⚠⚠⚠ the pane's job must have been SIGNALLED. Anything else means the loop's door \
             closed on a room its agent is still working in: {:?}",
            outcome.stopped,
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// A machine plus the engine its datamodel lives in, and the session id that
    /// engine files those variables under.
    ///
    /// Both halves are handed back because they answer different questions: the
    /// ENGINE holds `<data>` a script datamodel evaluates, and the POLICY holds the
    /// data SCE was able to lower into typed Rust fields. A gate that reads only one
    /// of them cannot tell those two apart, which is the whole subject below.
    fn started() -> (Engine<AiLoopPolicy>, Arc<dyn IScriptEngine>, String) {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(AiLoopPolicy::new(Arc::clone(&lua)));
        engine.initialize();
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel must have opened a script session");
        (engine, lua, session)
    }

    /// Raise `event` at `engine` carrying `standing` as `_event.data.standing`, then step.
    ///
    /// ⚠⚠ THE TWO REFLECTION EDGES BOTH ADOPT THAT LIST, so a gate driving the machine by hand has to
    /// send it: `process_event` carries no data at all, and a document-level gate that used it would
    /// assign nil over the variable `priming` composes and then assert about the states anyway. **A
    /// fixture must reach a state by the door the product uses** — the driver's `Raise::carrying` is
    /// this, one layer up.
    fn reflected(engine: &mut Engine<AiLoopPolicy>, event: AiLoopEvent, standing: &str) {
        engine.raise_external(
            event,
            &serde_json::json!({"standing": standing}).to_string(),
            "",
        );
        engine.step();
    }

    /// ⚠⚠⚠ **HOW THE MACHINE TELLS ITS DRIVER WHAT TO DO — asked of the ENGINE, because the
    /// answer decides the driver's whole shape and the document cannot settle it.**
    ///
    /// `ai_loop.scxml` reads as though it were giving instructions: `priming` does
    /// `<send event="prompt.start"/>`, `restarting` does `<send event="session.replace"/>`, and
    /// seven such sends between them name every effect an outer driver has to perform. So the
    /// obvious driver is EVENT-DRIVEN: subscribe to the machine's sends, do what each one says.
    ///
    /// **That driver cannot be written, and this gate is where that was established rather than
    /// assumed.** A targetless `<send>` is W3C SCXML 6.2's *external event to SELF*: the generated
    /// code calls `raise_external_with_meta` on the machine's OWN queue, and no transition in this
    /// document listens for any of the seven — so they are raised and dropped. The one handle that
    /// looks like a subscription, `Engine::get_external_queue_handle`, is for `#_parent` sends out
    /// of `<invoke>`d CHILD machines and **mints a fresh empty queue on every call**.
    ///
    /// So the driver is **STATE-DRIVEN**: it reads `get_current_state()` and acts on where the
    /// machine IS, and the machine's own published ingress partition is what says this is the
    /// intended shape — `prompt.sent` (the driver's ANSWER) is externally drivable, while
    /// `prompt.start` (the supposed instruction) is not. The sends are documentation of intent
    /// that the compiler carries; the STATE is the contract.
    ///
    /// ⚠ Written as an assertion rather than as a comment because R376 paid for exactly this
    /// distinction one round ago: reading SCE's generated source said the opposite of what running
    /// it says. Whatever this gate reports is the thing to build against.
    #[test]
    fn the_machine_instructs_its_driver_through_its_state_not_through_its_sends() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: `start` must land in the state whose onentry sends `prompt.start`",
        );

        // ── the door that looks like a subscription ──
        let drained = engine.get_external_queue_handle();
        let seen = drained.lock().expect("the queue mutex").len();
        assert_eq!(
            seen, 0,
            "⚠⚠⚠ `prompt.start` WAS just sent, and this handle shows {seen} events. If it ever \
             shows one, the driver below is the wrong shape — it should subscribe rather than \
             read state, and every effect it performs should be keyed on a send",
        );

        // ── what the machine says a driver may tell it ──
        let ingress = AiLoopEvent::EXTERNALLY_DRIVABLE_EVENTS;
        assert!(
            ingress.contains(&AiLoopEvent::PromptSent),
            "the driver's ANSWER — *I have sent it* — must be something the machine accepts from \
             outside, or a driver cannot report having acted at all: {ingress:?}",
        );
        assert!(
            ingress.contains(&AiLoopEvent::Brief),
            "⚠⚠ and so must the one thing a caller TELLS the machine rather than reports to it. \
             `brief` is how somebody who did not edit this file says what the run is for; a \
             machine that does not accept it from outside is one whose template nothing can fill \
             in, which is exactly the state this round found it in: {ingress:?}",
        );
        assert!(
            !ingress.contains(&AiLoopEvent::PromptStart),
            "⚠⚠ and the supposed INSTRUCTION is not an ingress event, which is the machine saying \
             the same thing from the other side: nobody outside sends `prompt.start`, so nothing \
             outside is meant to receive it either. It is the STATE that instructs: {ingress:?}",
        );

        // ── and the state is a complete instruction on its own ──
        //
        // Every effect the seven sends name is recoverable from where the machine is, which is
        // what makes the state-driven driver whole rather than a degradation of the other one.
        for (state, effect) in [
            (AiLoopState::Priming, "deliver the start prompt"),
            (AiLoopState::Screening, "match the dialog against the rules"),
            (AiLoopState::AwaitingHuman, "raise a pane attention"),
            (AiLoopState::Reflecting, "write the improvements"),
            (
                AiLoopState::Restarting,
                "close the pane and open a fresh one",
            ),
        ] {
            assert_ne!(
                state,
                AiLoopState::Working,
                "each effect state must be distinguishable from the one where the driver only \
                 watches, or *{effect}* would have to be inferred from something else",
            );
        }
    }

    /// ⚠⚠⚠ **HOW `judging`'s GOAL-MET GUARD ACTUALLY READS ITS DATA** — the one fact an outer
    /// driver must send with an event, asked of the engine because getting it wrong is silent.
    ///
    /// `judging`'s first transition is `<transition event="judge" cond="_event.data.done"
    /// target="closing"/>`, so *did the agent say it was done* travels as event DATA rather than
    /// as a datamodel variable. Every other event on this machine's ingress surface is bare.
    ///
    /// The driver's first attempt sent `{"done": false}` as the event data and the machine went to
    /// `closing` anyway — a loop that converges on the turn its agent has NOT finished, reporting
    /// success and asking for a closing summary of work that did not happen. **The screen said the
    /// marker was absent and the machine converged regardless**, which is exactly the class of
    /// silent wrongness this project keeps paying for.
    ///
    /// So the two readings are pinned side by side here, in the machine's own terms, and whatever
    /// this gate reports is what the driver is built against.
    #[test]
    fn the_goal_met_guard_separates_a_finished_agent_from_an_unfinished_one() {
        /// Walk a fresh machine to `judging` and raise `judge` carrying `data`.
        fn judged(data: &str) -> AiLoopState {
            let (mut engine, _lua, _session) = started();
            engine.process_event(AiLoopEvent::Start);
            engine.process_event(AiLoopEvent::PromptSent);
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "the control: one completed turn is judged",
            );
            engine.raise_external(AiLoopEvent::Judge, data, "");
            engine.step();
            engine.get_current_state()
        }

        assert_eq!(
            judged("{\"done\": true}"),
            AiLoopState::Closing,
            "an agent that said the milestone was reached sends the loop to its closing report",
        );
        assert_eq!(
            judged("{\"done\": false}"),
            AiLoopState::Working,
            "⚠⚠⚠ AND AN AGENT THAT DID NOT MUST TAKE ANOTHER TURN. Converging here reports a \
             milestone reached on the strength of a screen that does not say so — the driver \
             measured exactly this and the machine converged on turn one",
        );
    }

    /// ⚠⚠⚠ **THE OUTER LOOP IS A MACHINE NOW, AND THIS IS WHAT THAT BUYS.**
    ///
    /// The topology the document draws is the topology the compiler enforces. This
    /// drives the two edges the last two rounds spent themselves on — R372's *a
    /// person took the pane* and R373's *they gave it back* — through the OUTER
    /// machine rather than through prose about it.
    ///
    /// The point is not that the transitions work; SCE's own W3C suite covers that.
    /// It is that these transitions EXIST TO BE DRIVEN AT ALL. Before this round
    /// `working --turn.interrupted--> awaiting_human` was a sentence in an XML
    /// comment, and the Rust that implements the same idea was gated against its own
    /// hand-written vocabulary with nothing joining the two.
    #[test]
    fn the_outer_loop_runs_the_edges_the_last_two_rounds_built() {
        let (mut engine, _lua, _session) = started();
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Idle,
            "the document's `initial`",
        );

        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "a started loop primes a session before it prompts it",
        );

        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // R372: a person reached into the pane. The loop stops driving.
        engine.process_event(AiLoopEvent::TurnInterrupted);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::AwaitingHuman,
            "⚠ the edge R372 built the product half of",
        );

        // R373: they let go. The loop takes the pane back and prompts again.
        engine.process_event(AiLoopEvent::Resume);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠ and the edge R373 built the product half of. `orchestration.scxml` \
             says this one was left out because *when has somebody stopped typing* \
             had no measured answer; `Handback::WhenStill` is that answer, and this \
             is the machine that was waiting for it",
        );
    }

    /// ⚠⚠⚠ **THE OUTER BUDGET IS ENFORCED BY THE MACHINE — debt 60's third item,
    /// and the first of the three that could be paid at all.**
    ///
    /// `max_turns` and `reflect_every` have sat in the document's datamodel since
    /// `95207ad` with nothing reading them: the register recorded them as *"already
    /// in the datamodel"*, which was true and meant only that the numbers were
    /// written down. A number nothing compares against is a comment.
    ///
    /// Now `judging` is a state a compiler emitted, so the three guards in it are
    /// three branches with a priority order the DOCUMENT fixed, and this walks the
    /// whole authored budget through them:
    ///
    /// * `reflect_every` (8) fires at turns 8, 16, 24 and 32 — the loop stops to
    ///   improve itself and `reflecting` resets the counter on entry;
    /// * `max_turns` (40) ends the run — and it wins at turn 40 even though
    ///   `turns_since_reflect` has also come round, because the document orders the
    ///   `max_turns` transition FIRST. **That precedence is the assertion**: a run at
    ///   its ceiling must end rather than pay for one more restart it has no turns
    ///   left to use.
    ///
    /// ⚠ THE SEQUENCE IS COLLECTED, NOT SPOT-CHECKED. Asserting only the ending
    /// would pass for a machine that reflected on every turn, or never; asserting
    /// only a reflect point would pass for one that never stopped.
    #[test]
    fn the_outer_budget_the_document_authors_is_the_one_the_machine_enforces() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // Where the loop went after each completed turn, in order.
        let mut decisions: Vec<(u32, AiLoopState)> = Vec::new();
        let mut turn = 0_u32;
        while engine.get_current_state() == AiLoopState::Working {
            turn += 1;
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "a completed turn is judged, always: turn {turn}",
            );
            // No `_event.data.done`, so the goal-met guard is falsy and the budget
            // guards are what decide. The peer saying the done marker is a
            // different gate; this one is about the two NUMBERS.
            engine.process_event(AiLoopEvent::Judge);
            decisions.push((turn, engine.get_current_state()));

            // A reflection that finds nothing to change returns to `working`
            // without paying for a restart — the document's `reflect.none` edge,
            // and what keeps this walk going to the ceiling.
            //
            // ⚠⚠ RAISED WITH DATA, because that edge adopts the normalised
            // standing list (`_event.data.standing`) and the driver always
            // carries it. `process_event` sends none, which would assign nil over
            // the very variable `priming` composes — a fixture reaching the state
            // by a door production does not use.
            if engine.get_current_state() == AiLoopState::Reflecting {
                reflected(&mut engine, AiLoopEvent::ReflectNone, "");
            }
            assert!(turn <= 100, "the ceiling must be reachable: {decisions:?}");
        }

        let reflected: Vec<u32> = decisions
            .iter()
            .filter(|(_, state)| *state == AiLoopState::Reflecting)
            .map(|(turn, _)| *turn)
            .collect();
        assert_eq!(
            reflected,
            vec![8, 16, 24, 32],
            "`reflect_every` is 8 and the counter resets on entry to `reflecting`, \
             so the loop stops to improve itself on exactly these turns — and NOT \
             at 40, where the ceiling takes precedence: {decisions:?}",
        );
        assert_eq!(
            decisions.last(),
            Some(&(40, AiLoopState::Exhausted)),
            "⚠⚠⚠ `max_turns` is 40 and its transition is written BEFORE the \
             reflect one, so the fortieth turn ends the run instead of restarting \
             a session that has no turns left to spend: {decisions:?}",
        );
        assert!(
            engine.is_in_final_state(),
            "and `exhausted` is a final state, not a pause",
        );
    }

    /// ⚠⚠⚠ **A STANDING INSTRUCTION IS REFLECTED ON AT THE NEXT JUDGEMENT, AND ONCE** — the
    /// `screened > screened_carried` guard, asked of the MACHINE.
    ///
    /// # ⚠⚠⚠ The two halves, and why neither alone would do
    ///
    /// * **AT THE NEXT JUDGEMENT.** Without this guard a screened loop waits out `reflect_every`
    ///   before adopting what it was told — the document ships 8 — and every turn in between asks for
    ///   the original milestone again. That is register item 148, whose live agent wrote *"루프가 매
    ///   턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절 … 진전이 없습니다"*.
    /// * ⚠⚠⚠ **AND ONCE.** `screened_carried` is set on ENTRY to `reflecting`, not on the way out, so
    ///   a reflection that changes nothing does not send the run straight back. Set on the exit
    ///   instead, `reflect.none` would return to `working`, the very next judgement would see the same
    ///   inequality, and the loop would judge no further turn for the rest of its budget. **That is a
    ///   livelock the state names cannot show**, which is why the second half is asserted here rather
    ///   than left to read as obvious.
    ///
    /// ⚠ Driven with `raise_external`, because both edges carry data — see [`reflected`].
    #[test]
    fn a_standing_instruction_is_reflected_on_at_the_next_judgement_and_once() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // ⚠ THE CONTROL: with nothing screened, a judged turn goes straight back to work. The
        // document ships `reflect_every: 8`, so nothing else can send this turn to `reflecting`.
        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "the control: an unscreened turn is not a reason to reflect, or the assertion below is \
             about the turn budget",
        );

        // The peer asks, a rule claims it, and the driver reports what it said.
        engine.process_event(AiLoopEvent::TurnBlocked);
        assert_eq!(engine.get_current_state(), AiLoopState::Screening);
        engine.raise_external(
            AiLoopEvent::ScreenMatched,
            &serde_json::json!({"text": "do it another way"}).to_string(),
            "",
        );
        engine.step();
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "a claimed dialog returns to work — the peer has just been handed its answer",
        );

        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Reflecting,
            "⚠⚠⚠ THE FIRST HALF: the judgement straight after a standing instruction fired must \
             reflect, at turn TWO of a document whose `reflect_every` is 8",
        );

        // Nothing worth changing — back to work without a restart.
        reflected(&mut engine, AiLoopEvent::ReflectNone, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);
        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠⚠⚠ THE SECOND HALF: the SAME instruction must not send the run back to `reflecting` \
             for ever. `screened_carried` is set on entry, so this judgement sees the two counts \
             equal — set on the way out instead, this loop would never judge another turn and no \
             state name would show it",
        );
    }

    /// ⚠⚠⚠ **THE AUTHORED HALF OF THE DOCUMENT SURVIVES — AND THE ROUND HAD TO RUN
    /// IT TO FIND THAT OUT, BECAUSE READING SAID THE OPPOSITE.**
    ///
    /// `ai_loop.scxml` declares `datamodel="ecmascript"`. At the pinned SCE rev
    /// there is exactly ONE [`IScriptEngine`] — `LuaEngine`; `sce-rust-runtime`'s
    /// own manifest calls QuickJS *"future"*, and SCE's build special-cases only the
    /// datamodel string `"null"`, routing every other value to whatever engine the
    /// consumer supplies. So the document's ECMAScript is evaluated by **Lua**, and
    /// the generated init strings show a PARTIAL rewrite: the object/array literal
    /// in `screen_rules` is turned into Lua table syntax (`[…]` → `{…}`, `key:` →
    /// `key =`), while `start_prompt`'s `'…' + north_star + '\n' + …` is passed
    /// through verbatim — and in Lua `+` is arithmetic, not concatenation.
    ///
    /// From that reading this gate was written to assert the prompts DO NOT arrive.
    /// **It failed, and the failure is the finding**: the composed prompt comes back
    /// whole. The engine handles the concatenation; the mismatch visible in the
    /// generated source is not a defect a caller can reach.
    ///
    /// So this asserts what is true, over the three shapes the authored half is made
    /// of, each of which a different part of the loop depends on:
    ///
    /// * `north_star` — a bare literal. The control: if this one fails, the gate is
    ///   not reading the datamodel at all and nothing below means anything.
    /// * `start_prompt` — a COMPOSED string, and **as of this round the composition
    ///   is an `<assign>` in `priming`'s `onentry` rather than a `<data expr>`**. That
    ///   is a shape this gate had never driven, and the document's own caveat says a
    ///   shape not driven here is a shape nobody has measured — so the walk to
    ///   `priming` below is the point, not a detour around it.
    /// * `screen_rules` — a LIST OF OBJECTS, the shape debt 60's `screening` is
    ///   built out of, and the only one whose syntax the codegen rewrote.
    /// * `max_turns` — a scalar, which the outer `judging` budget compares against.
    #[test]
    fn the_whole_authored_surface_crosses_into_the_datamodel() {
        let (mut engine, lua, session) = started();

        // ── the control: a bare literal crosses unharmed ──
        let north_star = lua.get_variable(&session, "north_star");
        assert!(
            matches!(&north_star, Ok(ScriptValue::String(text)) if text.contains("edit me")),
            "⚠ THE CONTROL FAILED, so nothing below means anything: a bare string \
             literal must reach the datamodel. Got {north_star:?}",
        );

        // ── the SECOND control, and it is what makes the composition below a claim
        //    about `priming` rather than about `<data>`: nothing is composed yet.
        let unprimed = lua.get_variable(&session, "start_prompt");
        assert!(
            matches!(&unprimed, Ok(ScriptValue::String(text)) if text.is_empty()),
            "⚠ a machine that has not primed must hold no composed prompt, or the walk \
             below proves nothing about where the composition happens: {unprimed:?}",
        );

        // ── a COMPOSED string, built by an `<assign expr>` on the way into `priming` ──
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: the composition runs on entry to `priming`",
        );
        let start_prompt = lua.get_variable(&session, "start_prompt");
        let Ok(ScriptValue::String(start_prompt)) = &start_prompt else {
            panic!("the prompt `priming` sends must be a composed string: {start_prompt:?}");
        };
        assert!(
            start_prompt.starts_with(COMPOSED_START_PROMPT),
            "the `+` chain must have concatenated, not added: {start_prompt:?}",
        );
        assert!(
            start_prompt.contains("Report what you did and what is left."),
            "and every clause of it must be there, not just the first: \
             {start_prompt:?}",
        );
        // ⚠⚠ AND ONE `<assign>` MUST HAVE SEEN THE ONE BEFORE IT. `done_instruction` is
        // composed first and the two working prompts end with it, so executable content
        // running out of document order would append the PREVIOUS entry's instruction —
        // correct for every entry but the first, and silent.
        assert!(
            start_prompt.trim_end().ends_with("MILESTONE REACHED"),
            "⚠⚠ `done_instruction` must have been composed BEFORE the prompt that ends \
             with it: {start_prompt:?}",
        );

        // ── a LIST OF OBJECTS: the shape `screening` reads its rules out of, and
        //    the one whose SYNTAX the codegen rewrote on the way in ──
        let rules = lua.get_variable(&session, "screen_rules");
        let rules = match &rules {
            Ok(ScriptValue::Array(rules)) => rules,
            other => panic!(
                "⚠⚠ `screening` cannot be built on a datamodel that cannot hold its \
                 rules. The engine answered {other:?}",
            ),
        };
        // ⚠⚠ ONE, and it is the document's `(edit me)` PLACEHOLDER — this used to be three, matched
        // by dialog KIND. R383 measured that quoting the agent covers what a taxonomy would, and
        // R384 measured that a shipped needle would have to be INVENTED for the one dialog family
        // nobody has captured. So the file ships the shape and claims nothing with it.
        assert_eq!(rules.len(), 1, "the document declares one rule: {rules:?}");
        let first = match &rules[0] {
            ScriptValue::Object(fields) => fields,
            other => panic!("a rule is an object of `when`/`text`: {other:?}"),
        };
        assert!(
            matches!(first.get("when"), Some(ScriptValue::String(w)) if w.contains("(edit me)")),
            "⚠ and its FIELDS must survive the `key:` → `key =` rewrite, not just \
             its shape: {first:?}",
        );
        // ⚠ AND `keys` MUST BE GONE. It is asserted as an ABSENCE because its presence would be a
        // rule able to name the key that APPROVES — a live probe pressed `Tab` at a real permission
        // dialog, typed into what was left, and the agent's file was written. See
        // [`crate::screen::REFUSES`].
        assert!(
            first.get("keys").is_none(),
            "⚠⚠⚠ a screen rule must NOT author its own key. The key that refuses is the product's, \
             measured, and that is what stops a standing rule granting a permission: {first:?}",
        );

        // ── a scalar: what the outer `judging` budget compares against ──
        //
        // ⚠ Read through the SCRIPT SESSION rather than off the policy, and not by
        // choice. SCE lowered every scalar `<data>` into a typed Rust field
        // (`max_turns: i64`, initialised to 40) AND emitted no accessor for any of
        // them — only `session_id` is `pub`. So a consumer cannot ask the machine
        // what its own budget is; the interpreter's copy is the only readable one.
        // That is what makes the guard below the ONLY way to observe the budget.
        let max_turns = lua.get_variable(&session, "max_turns");
        assert!(
            matches!(&max_turns, Ok(ScriptValue::Int(40))),
            "the authored budget must cross as a number: {max_turns:?}",
        );
    }

    /// ⚠⚠⚠ **A PERSON'S OWN LANGUAGE REACHES THE DATAMODEL BY BOTH ROUTES INTO IT** — the gate
    /// that found SCE PR-87, repurposed onto the fix rather than deleted with it.
    ///
    /// # What it measured before, and why the two-seam shape is kept
    ///
    /// `OuterLoop::brief` sends a person's prose in as event data and reads it back out. An em
    /// dash went in and `â\u{80}\u{94}` came out — the three UTF-8 bytes of U+2014, each widened
    /// into its own `char`. Read off either end that is a guess: the widening could have been the
    /// payload becoming `_event.data`, or [`IScriptEngine::get_variable`] converting a Lua value
    /// back. **Asking the SAME engine, session and variable through the TWO DIFFERENT ARRIVAL
    /// ROUTES is what made it arithmetic**: a document literal was clean and event data was not,
    /// so the converter was the only thing left. `sce-rust-lua`'s `json_to_lua_table` walked the
    /// payload with `bytes[i] as char`.
    ///
    /// Upstream's fix went further than the report asked — the Lua-source rewrite was replaced by
    /// a real JSON decoder, which also restored escapes and arrays (valid JSON containing either
    /// had been silently demoting `_event.data` to a string).
    ///
    /// So the structure stays and the verdicts flip. It is still the only place that asks both
    /// routes, and the routes are not interchangeable: **`screen_rules` in the shipped template is
    /// Korean**, so seam one is about text `screening` will read the day it is built, while seam
    /// two is about every brief a caller will ever supply.
    #[test]
    fn a_non_ascii_string_reaches_the_datamodel_by_either_route() {
        let (_engine, lua, session) = started();

        // ── seam one: a literal in the DOCUMENT, initialised by `<data expr>` ──
        //
        // The template's own third rule, which is Korean prose a person wrote into this file.
        let rules = lua.get_variable(&session, "screen_rules");
        let Ok(ScriptValue::Array(rules)) = &rules else {
            panic!("the control: the rules must cross as a list at all: {rules:?}");
        };
        let ScriptValue::Object(first) = &rules[0] else {
            panic!("the control: a rule is an object: {:?}", rules[0]);
        };
        let Some(ScriptValue::String(text)) = first.get("text") else {
            panic!("the control: a rule carries a reply text: {first:?}");
        };
        assert!(
            text.starts_with("비용 무시하고"),
            "⚠⚠⚠ SEAM ONE: a non-ASCII literal AUTHORED IN THE DOCUMENT does not survive the \
             datamodel. Every `screening` rule this template ships is Korean, so the day that \
             state is built it would send an agent bytes nobody wrote. Got {text:?}",
        );

        // ── seam two: a string arriving as EVENT DATA, assigned by a transition ──
        //
        // `idle`'s `brief` transition is the one place this document takes a string from outside,
        // and it is the path `OuterLoop::brief` uses. Same engine, same session, same variable
        // kind — the ONLY difference from seam one is how the value got there.
        let mut engine = _engine;
        let sent = "북극성 — ship it";
        engine.raise_external(
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": sent,
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
            "",
        );
        engine.step();
        let held = lua.get_variable(&session, "north_star");
        let Ok(ScriptValue::String(held)) = &held else {
            panic!("the control: the brief must have assigned something at all: {held:?}");
        };
        assert_eq!(
            held, sent,
            "⚠⚠⚠ SEAM TWO: a non-ASCII string does not survive EVENT DATA, where the same \
             characters authored in the document (asserted above) do. So the mangling is in the \
             payload -> `_event.data` conversion and NOT in `get_variable`. Every brief a caller \
             supplies crosses this seam. This is SCE PR-87 returning",
        );
        // ⚠ THE OLD DAMAGE, DERIVED rather than pasted, kept as the NEGATIVE control. A regression
        // would not merely differ from what was sent — it would be exactly a Latin-1 widening of
        // the UTF-8 bytes, because that is what `bytes[i] as char` does. Naming the shape is what
        // stops a DIFFERENT breakage being read as this one having come back.
        let latin1_widened: String = sent.bytes().map(char::from).collect();
        assert_ne!(
            held, &latin1_widened,
            "the exact shape of PR-87 must not be reachable again: every byte its own char",
        );
        assert_ne!(
            sent,
            latin1_widened.as_str(),
            "⚠ THE CONTROL FOR THE CONTROL: the probe text must actually CONTAIN non-ASCII, or the \
             assertion above holds for a string nothing could have damaged",
        );

        // ── and the two things upstream's fix restored beyond what was reported ──
        //
        // The rewrite required JSON's grammar to coincide with Lua's. Escapes and arrays do not,
        // so valid JSON carrying either had been demoting `_event.data` to a STRING — the field
        // read back nil and no error was raised anywhere. Neither shape is used by this document
        // today, which is exactly why they get a gate: nothing else here would notice them break.
        engine.raise_external(
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": "a \"quoted\" line\nand a second one",
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
            "",
        );
        engine.step();
        assert!(
            matches!(
                lua.get_variable(&session, "north_star"),
                Ok(ScriptValue::String(ref held))
                    if held == "a \"quoted\" line\nand a second one",
            ),
            "a JSON escape must decode rather than be handed to a Lua parser: {:?}",
            lua.get_variable(&session, "north_star"),
        );
    }
}
