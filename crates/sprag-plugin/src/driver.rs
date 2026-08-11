//! `Driver` — the shared control substrate.
//!
//! Owns the SCE/SCXML statechart (`orchestration.scxml`: idle → running →
//! converged/exhausted/failed) and the guardrails, and runs any [`Plugin`] to a
//! terminal [`Outcome`]. Each microstep is one `Plugin::step`; the Driver
//! enforces the iteration, cost and DURATION budgets and maps the result onto
//! the statechart. The plugin owns the behaviour; the Driver owns the lifecycle.
//!
//! Two of the three ceilings are decided between steps, from counters the Driver
//! keeps. The third cannot be: a run out of time may be INSIDE a step that will
//! not return for minutes, so the deadline is armed into the [`RunContext`] and
//! every bounded wait a plugin makes consults it.
//!
//! [`RunContext`]: crate::run::RunContext

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sce_rust_runtime::Engine;

use crate::access::{PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::run::RunContext;
use crate::sm::orchestration::{OrchestrationEvent, OrchestrationPolicy, OrchestrationState};

/// The termination guardrails every plugin run is bounded by (first-class
/// safety per the README — an AI control loop must not run unbounded).
///
/// THREE independent ceilings, ANDed: whichever binds first ends the run, and
/// the outcome names which one it was ([`Ceiling`]). They are independent
/// because they answer three different questions a person asks about a loop —
/// *how many times?*, *how much?*, and *for how long?* — and no two of them can
/// stand in for each other. An iteration ceiling cannot bound a run whose single
/// step blocks; a cost ceiling cannot bound a run whose steps are free (a
/// print-mode dialogue reports `Tokens(0)` every turn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guardrails {
    /// Stop after this many steps.
    pub max_iterations: u32,
    /// Stop once the accumulated step cost reaches this bound. `None` leaves cost
    /// unbounded (the other two ceilings still apply). The
    /// bound's unit is the run's cost currency — every step the plugin reports
    /// shares it (see [`Cost`]) — so the Driver compares
    /// like with like and never sums bytes against tokens.
    pub max_cost: Option<Cost>,
    /// STOP ONCE THIS MUCH WALL-CLOCK TIME HAS PASSED since the run began.
    /// `None` leaves the run untimed.
    ///
    /// # ⚠⚠ Why the other two ceilings do not already cover this
    ///
    /// Both of them bound the run per STEP: `max_iterations` counts steps that
    /// COMPLETED, and `max_cost` sums what completed steps spent. Neither can see
    /// a step that is still running — so a run of 100 iterations against a peer
    /// that answers slowly is bounded at 100 × that peer's own reply timeout,
    /// a number nobody chose and no caller can read off the request. The only
    /// bound expressible in the units a person actually reasons in — *"not more
    /// than five minutes on this"* — was missing.
    ///
    /// ⚠ It is DISTINCT from the per-turn `timeout_ms` an `agent` or `dialogue`
    /// takes: that bounds ONE reply and then the loop takes another turn. This
    /// bounds the loop.
    ///
    /// The [`Driver`] arms it into the [`RunContext`] as an instant
    /// ([`RunContext::deadline_in`]), which is what carries it into the waits
    /// inside a step; a ceiling checked only between steps would be no ceiling
    /// at all for a run stuck inside one.
    ///
    /// [`RunContext`]: crate::run::RunContext
    /// [`RunContext::deadline_in`]: crate::run::RunContext::deadline_in
    pub max_duration: Option<Duration>,
}

/// WHICH GUARDRAIL STOPPED A RUN — the reason an [`OutcomeState::Exhausted`] carries.
///
/// # ⚠⚠ Why an exhausted run that does not say which ceiling is barely an answer
///
/// `exhausted` alone tells a caller to change something without telling it WHAT. The three
/// ceilings have three different remedies — give it more turns, give it more budget, give it more
/// time — and with a third one added, guessing gets a third harder. Worse, a caller cannot even
/// infer it by comparing the counters against what it asked for: the ceilings it did not name came
/// from the DAEMON's defaults, so a run stopped by a default is stopped by a number the caller
/// never saw.
///
/// # ⚠ Why it names the CONCEPT and not the argument that sets it
///
/// `duration`, not `max_seconds`. The obvious alternative — answer the knob the caller would turn —
/// breaks on `Cost`, which is set by `max_bytes` on a byte-relay run and `max_tokens` on a
/// dialogue: one ceiling, two argument names, chosen by the plugin. A concept per ceiling is the
/// only naming that is one-to-one, and the `unit` key already beside it says which knob a `cost`
/// answer means.
///
/// # ⚠ Why this is deliberately NOT a published vocabulary
///
/// The other closed sets on this wire are ARGUMENT vocabularies: a client picks a word from them,
/// so publishing what is admissible is the difference between a call it can build and one it has
/// to guess. This is an ANSWER word, and no peer decodes it into a closed type — both renderers
/// take the string and print it. So a fourth ceiling is additive here in the strongest sense: an
/// old reader shows a word it has never seen and is not wrong. Giving it a `WIRE_WORDS` nobody
/// reads would be a constant the product does not enforce, which this project has removed twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceiling {
    /// [`Guardrails::max_iterations`] — the run took all the steps it was allowed.
    Iterations,
    /// [`Guardrails::max_cost`] — the run spent all it was allowed, in its own unit.
    Cost,
    /// [`Guardrails::max_duration`] — the run ran out of wall-clock time.
    Duration,
}

impl Ceiling {
    /// This ceiling's word on the wire — the ONE place the variant → name mapping lives, so the
    /// host never spells a `Ceiling` variant ([`Cost::unit`]'s rule, one level up).
    ///
    /// Exhaustive, so a ceiling added to the type cannot reach the wire without a word: there is no
    /// hand-written list for it to be left out of.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Iterations => "iterations",
            Self::Cost => "cost",
            Self::Duration => "duration",
        }
    }
}

/// Which terminal statechart state a run reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeState {
    /// A plugin step returned [`Verdict::Converged`].
    Converged,
    /// A GUARDRAIL stopped the run, and which one.
    ///
    /// The [`Ceiling`] is carried rather than reported beside it so that an exhausted outcome
    /// which does not say what exhausted it cannot be constructed — the same reasoning that put
    /// the unit inside [`Cost`] instead of next to it.
    Exhausted(Ceiling),
    /// A plugin step failed.
    Failed,
    /// The run was cancelled (the host raised the cancel signal).
    Cancelled,
}

/// The result of a plugin run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub state: OutcomeState,
    pub iterations: u32,
    /// The accumulated typed cost, or `None` if the run took no measured step
    /// (e.g. cancelled at the loop top before any step ran).
    pub cost: Option<Cost>,
    /// The cause when `state` is [`OutcomeState::Failed`]; `None` otherwise.
    pub failure: Option<PaneError>,
}

/// Runs a [`Plugin`] over a [`PaneAccess`] to a terminal [`Outcome`], owning the
/// statechart + guardrail counters.
pub struct Driver {
    engine: Engine<OrchestrationPolicy>,
    guardrails: Guardrails,
    iterations: u32,
    cost: Option<Cost>,
    failure: Option<PaneError>,
    progress: Option<ProgressCell>,
    /// What each step did, bounded to the last [`JOURNAL_LIMIT`].
    journal: Vec<StepRecord>,
    /// Which ceiling raised [`OrchestrationEvent::Exhaust`], recorded AT the raise.
    ///
    /// Recorded rather than re-derived at the end, because a deadline that passed one instant
    /// after the decision would make a re-derivation disagree with the decision that was actually
    /// taken. [`Driver::exhaust`] is the only writer, so the statechart cannot reach its
    /// `exhausted` state without one.
    exhausted_by: Option<Ceiling>,
}

/// WHAT A RUN HAS SPENT SO FAR — the counters the [`Driver`] keeps, readable while it is still
/// keeping them.
///
/// # ⚠⚠ Why a running run reporting nothing was a defect and not an omission
///
/// The driver counts `iterations` and accumulates [`Cost`] from the first step, and published both
/// ONLY in the terminal [`Outcome`]. So a client watching a long run could not tell PROGRESS from
/// STUCK, and **could not see spend until the spending was over** — a strange property for a
/// feature whose selling point is a typed cost ceiling. The counters existed; they were simply not
/// readable mid-flight.
///
/// The same two facts under the same names as `Outcome`'s, deliberately: a reader that polls this
/// and then reads the outcome meets one vocabulary, and the last progress a run reports agrees with
/// the outcome it ends on.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    /// How many steps have completed. Zero before the first one returns.
    pub iterations: u32,
    /// What has been spent, in the run's own unit — [`None`] until the first step establishes it.
    pub cost: Option<Cost>,
    /// THE LAST [`JOURNAL_LIMIT`] STEPS, oldest first.
    ///
    /// See [`StepRecord`] for why a run that reports only its total is not diagnosable.
    pub journal: Vec<StepRecord>,
}

/// HOW MANY STEPS A RUN REMEMBERS.
///
/// A bound rather than the whole history, because a run may take as many steps as its iteration
/// ceiling allows and this is held in memory for the life of the daemon. The LAST ones are kept
/// because the question a journal is read to answer — *why did it not converge?* — is asked about
/// the end of a run. ⚠ The TOTAL is never lost: `iterations` counts every step, so a reader can
/// always tell a truncated journal from a complete one by comparing the two.
pub const JOURNAL_LIMIT: usize = 64;

/// WHAT ONE STEP DID — one entry of a run's journal.
///
/// # ⚠⚠ Why a run's total was not enough
///
/// A finished run said how many steps it took, what it spent, and which ceiling stopped it. For the
/// one question a bounded loop is actually debugged with — *what happened in there?* — it said
/// nothing at all, so `exhausted after 100 iterations` was the whole account of a hundred acts on
/// somebody's pane. The counters existed per step and were summed away.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepRecord {
    /// Which step this was, counting from one.
    pub iteration: u32,
    /// What this step alone spent (not the running total).
    pub cost: Cost,
    /// What it decided.
    pub verdict: Verdict,
    /// The plugin's own line about it, if it had one — see [`Step::note`](crate::plugin::Step::note).
    pub note: Option<String>,
}

/// Where a [`Driver`] publishes its [`Progress`], shared with whoever is watching.
///
/// A cell rather than a callback, on the same reasoning `runs` is a slot rather than a stream: this
/// is a LEVEL. A reader that looks twice and sees the same numbers has learned that nothing moved,
/// which is exactly the question "progress or stuck?" asks — and a missed edge would cost it
/// nothing.
pub type ProgressCell = Arc<Mutex<Progress>>;

impl Driver {
    /// A driver bounded by `guardrails`.
    #[must_use]
    pub fn new(guardrails: Guardrails) -> Self {
        Self {
            engine: Engine::new(OrchestrationPolicy::new()),
            guardrails,
            iterations: 0,
            cost: None,
            failure: None,
            progress: None,
            journal: Vec::new(),
            exhausted_by: None,
        }
    }

    /// Publish this run's progress into `cell` as it goes.
    ///
    /// Optional because the driver is used without a host — a test, or a fire-and-forget run — and
    /// a driver that REQUIRED somewhere to report to would make every such caller invent one.
    #[must_use]
    pub fn reporting_to(mut self, cell: ProgressCell) -> Self {
        self.progress = Some(cell);
        self
    }

    /// Write the counters out, if anybody is watching.
    ///
    /// ⚠ Called after the accumulate and NOT before the step: a reader must never see an iteration
    /// counted before the work it counts has happened, which is the same ordering rule the run-end
    /// announcement follows one layer up.
    fn publish(&self) {
        if let Some(cell) = &self.progress {
            let mut held = cell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *held = Progress {
                iterations: self.iterations,
                cost: self.cost,
                journal: self.journal.clone(),
            };
        }
    }

    /// Record what a step did, keeping the last [`JOURNAL_LIMIT`].
    ///
    /// ⚠ The step's OWN cost, not the running total: a journal of totals could not answer *"which
    /// step was the expensive one?"*, which is the question a cost ceiling makes people ask.
    fn record(&mut self, step: &Step) {
        if self.journal.len() == JOURNAL_LIMIT {
            self.journal.remove(0);
        }
        self.journal.push(StepRecord {
            iteration: self.iterations,
            cost: step.cost,
            verdict: step.verdict,
            note: step.note.clone(),
        });
    }

    /// Drive `plugin` over `panes` until a terminal state, reporting the
    /// [`Outcome`].
    ///
    /// ⚠ The context the PLUGIN sees is not the one handed in: the run's deadline
    /// is armed here, at the one moment "when does this run end?" has a single
    /// answer every wait underneath can share.
    #[must_use]
    pub fn run(
        mut self,
        plugin: &mut dyn Plugin,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Outcome {
        let run = &run.deadline_in(self.guardrails.max_duration);
        self.engine.initialize();
        self.engine.process_event(OrchestrationEvent::Start);
        // `running` is the only non-final state in the loop.
        while !self.engine.is_in_final_state() {
            // Cancel is checked before each step (and again by the plugin's own
            // wait loops mid-step), so a cancel ends the run promptly without
            // running another step.
            if run.cancelled() {
                self.engine.process_event(OrchestrationEvent::Cancel);
                continue;
            }
            // ⚠ THE DEADLINE IS CHECKED BEFORE THE STEP, NOT AFTER IT. Checked
            // after, a run whose remaining time is a millisecond would still
            // start a step that may take minutes, and the ceiling would be
            // advisory. Checked before, the run's LAST step is the last one that
            // began in time — and the waits inside it are bounded by the same
            // deadline, so it cannot outlive it by more than a poll interval.
            if run.expired() {
                let event = self.exhaust(Ceiling::Duration);
                self.engine.process_event(event);
                continue;
            }
            let event = match plugin.step(panes, run) {
                Err(error) => {
                    self.failure = Some(error);
                    OrchestrationEvent::Fail
                }
                Ok(step) => {
                    self.iterations += 1;
                    self.accumulate(step.cost);
                    self.record(&step);
                    self.publish();
                    match (step.verdict, self.budget_exhausted()) {
                        (Verdict::Converged, _) => OrchestrationEvent::Converge,
                        (Verdict::Continue, Some(ceiling)) => self.exhaust(ceiling),
                        (Verdict::Continue, None) => OrchestrationEvent::Continue,
                    }
                }
            };
            self.engine.process_event(event);
        }
        self.outcome()
    }

    /// Record WHICH ceiling ended the run and answer the event that ends it.
    ///
    /// The single writer of [`exhausted_by`](Self::exhausted_by), so the recorded reason and the
    /// statechart transition cannot be raised apart from one another.
    fn exhaust(&mut self, ceiling: Ceiling) -> OrchestrationEvent {
        self.exhausted_by = Some(ceiling);
        OrchestrationEvent::Exhaust
    }

    /// Add this step's cost to the running total, establishing the run's unit on
    /// the first step. Later steps share that unit (one plugin reports one unit),
    /// so the add always succeeds; a mismatch is a plugin bug (debug-asserted,
    /// release-ignored so a stray step can never corrupt the accumulator).
    fn accumulate(&mut self, cost: Cost) {
        self.cost = Some(match self.cost {
            None => cost,
            Some(acc) => acc.try_add(cost).unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "a plugin changed cost unit mid-run: {acc:?} + {cost:?}"
                );
                acc
            }),
        });
    }

    /// Which per-step ceiling this run has reached, if any.
    ///
    /// ⚠ Answers a [`Ceiling`] rather than a bool so the caller cannot raise the exhaustion without
    /// also saying what caused it. The DURATION ceiling is not asked here: it is not a property of
    /// a completed step, and the loop top is where a run out of time must stop.
    fn budget_exhausted(&self) -> Option<Ceiling> {
        if self.iterations >= self.guardrails.max_iterations {
            return Some(Ceiling::Iterations);
        }
        match (self.cost, self.guardrails.max_cost) {
            (Some(acc), Some(max)) if acc.reaches(max) => Some(Ceiling::Cost),
            _ => None,
        }
    }

    fn outcome(self) -> Outcome {
        let state = match self.engine.get_current_state() {
            OrchestrationState::Converged => OutcomeState::Converged,
            OrchestrationState::Exhausted => OutcomeState::Exhausted(
                // `exhaust` is the only producer of the event that reaches this state, and it
                // always records. A miss would mean the statechart reached `exhausted` by some
                // path nobody wrote — scream in debug; in release name the ceiling that bounds
                // EVERY run (`max_iterations` is never optional), which is the least wrong answer
                // available and still true of any run that got here.
                self.exhausted_by.unwrap_or_else(|| {
                    debug_assert!(false, "a run exhausted without recording which ceiling");
                    Ceiling::Iterations
                }),
            ),
            OrchestrationState::Cancelled => OutcomeState::Cancelled,
            // Failed, or any state the loop left unexpectedly.
            _ => OutcomeState::Failed,
        };
        Outcome {
            state,
            iterations: self.iterations,
            cost: self.cost,
            failure: self.failure,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{KeyStroke, PaneRow, Written};
    use crate::plugin::Step;
    use sprag_terminal::PaneId;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// A plugin whose step always fails — to pin the Driver's Err -> Failed
    /// mapping deterministically, no threads or PTY.
    struct FailingPlugin;
    impl Plugin for FailingPlugin {
        fn step(&mut self, _panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
    }

    /// An empty pane access (the failing plugin ignores it).
    struct NoPanes;
    impl PaneAccess for NoPanes {
        fn pane_ids(&self) -> Vec<PaneId> {
            Vec::new()
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            None
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            None
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
    }

    #[test]
    fn driver_maps_a_step_error_to_failed_with_the_cause() {
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut FailingPlugin, &NoPanes, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Failed);
        assert_eq!(outcome.iterations, 0);
        assert_eq!(outcome.failure, Some(PaneError::UnknownPane(PaneId(0))));
    }

    #[test]
    fn driver_ends_cancelled_without_running_a_step() {
        // The plugin would fail if stepped, but a pre-raised cancel pre-empts
        // it at the loop top: Cancelled, zero iterations, no failure recorded.
        let cancel = Arc::new(AtomicBool::new(true));
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut FailingPlugin, &NoPanes, &RunContext::new(cancel));
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        assert_eq!(outcome.iterations, 0);
        assert!(outcome.failure.is_none());
    }
}
