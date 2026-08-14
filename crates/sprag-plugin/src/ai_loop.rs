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
    /// `reflecting` is reached from `judging` when `turns_since_reflect >= reflect_every`, so a
    /// brief whose `reflect_every` is under its `max_turns` reaches it — and the session-replace
    /// lifecycle behind it (close the pane, write the improvements, open a fresh one that reads
    /// them) is registered debt with named prerequisites.
    ///
    /// Refusing here rather than mid-run is the difference between *"raise `reflect_every` to
    /// `max_turns` or above"*, which the caller can act on before anything happens, and a run that
    /// prompts a live agent eight times and then stops somewhere with no answer for it.
    Unbuilt(AiLoopState),
}

/// A BOUNDED, CANCELLABLE RUN of `ai_loop.scxml`'s machine against one pane — the door onto
/// [`OuterLoop`].
///
/// See the module doc for why this is a [`Plugin`] and not a second run mechanism.
pub struct AiLoop {
    /// The driver, one pass at a time.
    inner: OuterLoop,
    /// The pane whose agent this loop is causing to work — [`Plugin::driving`]'s answer.
    pane: PaneId,
}

impl std::fmt::Debug for AiLoop {
    /// The pane and where the machine is, and nothing else.
    ///
    /// ⚠ Hand-written because an [`OuterLoop`] owns a compiled statechart engine and a script
    /// interpreter, neither of which is `Debug` and neither of which anybody wants printed. What a
    /// reader meeting this in a failed assertion needs is which pane and which state.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiLoop")
            .field("pane", &self.pane.0)
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
        // own numbers and needs nothing else. `judging` checks `turns >= max_turns` BEFORE
        // `turns_since_reflect >= reflect_every`, and `turns_since_reflect` only resets in
        // `reflecting` — so `exhausted` is reached first exactly when `reflect_every >= max_turns`,
        // and that is the whole condition for a run that never meets an unbuilt state.
        if brief.max_turns < 1 {
            return Err(NotStarted::Unbuilt(AiLoopState::Exhausted));
        }
        if brief.reflect_every < brief.max_turns {
            return Err(NotStarted::Unbuilt(AiLoopState::Reflecting));
        }
        let mut inner = OuterLoop::new(script, pane, spec).ok_or(NotStarted::Undrivable)?;
        match inner.brief(brief) {
            Briefed::Took => Ok(Self { inner, pane }),
            refused => Err(NotStarted::Brief(refused)),
        }
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
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
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
                    // `fail` is raised from exactly two places and the other one is `brief`, which
                    // this plugin answers at the door — so a `failed` with no notice is a path
                    // nobody wrote, and saying so beats inventing a cause.
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
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
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
    /// ⚠ Two of the three are not *"unimplemented"* to whoever reads the run. A peer that stopped
    /// to ask wants an ANSWER and a person at the pane is already acting — those are the same two
    /// facts every other plugin here reports, and they get the same two words. Only
    /// `reflecting`/`restarting` are this build's own gap, and they are refused at the door
    /// ([`NotStarted::Unbuilt`]) so a run cannot reach them.
    fn unbuilt(&self, state: AiLoopState) -> Result<Step, PaneError> {
        let verdict = match (state, self.inner.noticed()) {
            // A PERSON TOOK THE PANE. `awaiting_human` is where the document waits for them, and
            // `taken_over` is this substrate's word for the same fact.
            (AiLoopState::AwaitingHuman, Some(Noticed::Interrupted(who))) => {
                Verdict::TakenOver(*who)
            }
            // THE PEER STOPPED TO ASK. `screening` is where the document answers such a question
            // from a person's standing rules — two owner decisions in front of it — so until it is
            // built the answer is the one every unattended run gives: stop, and publish what is
            // being asked.
            (AiLoopState::Screening | AiLoopState::AwaitingHuman, _) => {
                Verdict::Blocked(self.asking())
            }
            (state, _) => {
                return Err(PaneError::Undrivable(format!(
                    "it reached {state:?}, which this build has no effect for — and the brief that \
                     could reach it is refused at the door, so this run took a path nobody wrote"
                )));
            }
        };
        Ok(Step::new(Cost::Bytes(0), verdict).noting(format!(
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
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
            | AiLoopState::Closing => Some(self.pane),
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
    use crate::outer::{AiLoopSpec, Brief, INNER_SESSION_ENDS, OuterLoop, Pumped};
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

    /// ⚠⚠⚠ **A BRIEF THAT WOULD REACH A STATE NOBODY BUILT IS REFUSED BEFORE A BYTE IS TYPED.**
    ///
    /// The document's shipped `reflect_every` is 8 against a `max_turns` of 40, so the DEFAULT
    /// brief walks into `reflecting` at turn eight — a state whose session-replace lifecycle is
    /// registered debt. A run that discovered that eight turns in would have prompted a live agent
    /// eight times, spent eight turns of somebody's quota, and then stopped somewhere with no
    /// answer for it.
    ///
    /// ⚠⚠ **THE PANE IS THE ASSERTION**, not just the refusal: the whole value of refusing at the
    /// door is that nothing happened, and a refusal that had already typed the first prompt would
    /// be worth nothing. The screen is checked to be exactly what it was.
    #[test]
    fn a_brief_that_would_reach_an_unbuilt_state_is_refused_before_anything_is_typed() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let before = access.pane_collapsed(pane).expect("a readable pane");

        let refused = AiLoop::new(
            engine(),
            pane,
            &Brief {
                // The document's own shipped pair, which is the case that matters.
                reflect_every: 8,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect_err("a brief that reaches `reflecting` cannot start a run this build can finish");
        assert_eq!(
            refused,
            NotStarted::Unbuilt(AiLoopState::Reflecting),
            "the refusal must name the STATE, so the sentence a caller reads can name the knob",
        );

        // ⚠ AND A LOOP THAT CANNOT TAKE A TURN AT ALL, at the other end of the same arithmetic.
        assert_eq!(
            AiLoop::new(engine(), pane, &brief_for(0), &standin_spec())
                .expect_err("a loop allowed no turns judges itself exhausted before it starts"),
            NotStarted::Unbuilt(AiLoopState::Exhausted),
        );

        assert_eq!(
            access.pane_collapsed(pane).expect("a readable pane"),
            before,
            "⚠⚠⚠ NOTHING WAS TYPED. A refusal that had already prompted the agent would have cost \
             exactly what refusing early exists to save",
        );
        // ⚠ THE CONTROL: the same pane and the same numbers, one of them changed, starts.
        AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("⚠ the control: an equal pair is the brief this build can drive to the end");
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE STATE THE DOOR REFUSES IS ONE A RUN REALLY REACHES** — the refusal's premise,
    /// measured rather than argued from reading the document.
    ///
    /// [`AiLoop::new`] refuses `reflect_every < max_turns` on an arithmetic claim about
    /// `judging`'s guard order, and a claim like that is exactly the kind this workspace has been
    /// wrong about before: R370b filed four premises read off the source and all four were wrong.
    /// So the loop is driven with the numbers the door refuses, through [`OuterLoop`] — the layer
    /// UNDER the refusal, which is the only way to reach a case the door exists to prevent — and
    /// the machine is asked where it actually goes.
    ///
    /// ⚠⚠ **IT IS THE OTHER HALF OF A PAIR.** `a_loop_that_uses_the_turns_it_was_briefed_with…`
    /// drives the EQUAL pair (`reflect_every == max_turns`) all the way to `exhausted` without
    /// meeting an unbuilt state, so between them the two gates measure both sides of the one
    /// inequality the door is made of. Either alone would be a fact about one arrangement of
    /// numbers.
    #[test]
    fn the_state_the_door_refuses_is_one_a_run_would_really_reach() {
        // ⚠ NEVER says the marker, so nothing but the document's own guards decides where it goes.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let script = engine();
        let mut loops = OuterLoop::new(Arc::clone(&script), pane, &standin_spec())
            .expect("the document's datamodel must carry its four authored strings");
        assert_eq!(
            loops.brief(&Brief {
                // The refused shape, at its sharpest: reflection due after the FIRST turn.
                reflect_every: 1,
                ..brief_for(40)
            }),
            crate::outer::Briefed::Took,
            "the control: the parts must reach the datamodel, or what follows is about a brief \
             that never arrived",
        );

        let run = RunContext::uncancellable();
        let mut walked = Vec::new();
        let reached = loop {
            assert!(
                walked.len() < 24,
                "⚠ this gate's own bound, which is exactly what item 66 says a caller has to \
                 supply when nothing else does — the plugin above it is bounded by the Driver. \
                 Walked: {walked:?}",
            );
            match loops
                .pump(&access, &run)
                .expect("the pane must stay readable")
            {
                Pumped::Moved {
                    from, raised, to, ..
                } => walked.push((from, raised, to)),
                other => break other,
            }
        };
        assert!(
            matches!(reached, Pumped::Unbuilt(AiLoopState::Reflecting)),
            "⚠⚠⚠ a loop briefed the way the door refuses must really arrive at `reflecting` — if \
             it does not, the refusal is turning away callers for a reason that is not true. Got \
             {reached:?} after {walked:?}",
        );
        assert_eq!(
            loops.turns(),
            Some(1),
            "and it arrives after the FIRST judged turn, which is what `reflect_every: 1` says",
        );
        access.lifecycle().expect("lifecycle").close(pane);
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
        // ⚠ NEVER says the marker, so the run is still mid-loop when the cancel lands.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
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
            max_iterations: 10_000,
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
            if engine.get_current_state() == AiLoopState::Reflecting {
                engine.process_event(AiLoopEvent::ReflectNone);
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
                 rules. The document writes three; the engine answered {other:?}",
            ),
        };
        assert_eq!(
            rules.len(),
            3,
            "the document declares three rules: {rules:?}"
        );
        let first = match &rules[0] {
            ScriptValue::Object(fields) => fields,
            other => panic!("a rule is an object of `when`/`keys`/`text`: {other:?}"),
        };
        assert!(
            matches!(first.get("when"), Some(ScriptValue::String(w)) if w == "design-decision"),
            "⚠ and its FIELDS must survive the `key:` → `key =` rewrite, not just \
             its shape: {first:?}",
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
