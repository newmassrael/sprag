//! `Driver` — the shared control substrate.
//!
//! Owns the SCE/SCXML statechart (`orchestration.scxml`: idle → running →
//! converged/exhausted/failed) and the guardrails, and runs any [`Plugin`] to a
//! terminal [`Outcome`]. Each microstep is one `Plugin::step`; the Driver
//! enforces the iteration and cost budgets and maps the result onto the
//! statechart. The plugin owns the behaviour; the Driver owns the lifecycle.

use std::sync::{Arc, Mutex};

use sce_rust_runtime::Engine;

use crate::access::{PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Verdict};
use crate::run::RunContext;
use crate::sm::orchestration::{OrchestrationEvent, OrchestrationPolicy, OrchestrationState};

/// The termination guardrails every plugin run is bounded by (first-class
/// safety per the README — an AI control loop must not run unbounded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guardrails {
    /// Stop after this many steps.
    pub max_iterations: u32,
    /// Stop once the accumulated step cost reaches this bound. `None` leaves cost
    /// unbounded (only [`max_iterations`](Self::max_iterations) applies). The
    /// bound's unit is the run's cost currency — every step the plugin reports
    /// shares it (see [`Cost`]) — so the Driver compares
    /// like with like and never sums bytes against tokens.
    pub max_cost: Option<Cost>,
}

/// Which terminal statechart state a run reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeState {
    /// A plugin step returned [`Verdict::Converged`].
    Converged,
    /// A guardrail (iteration or cost budget) stopped the run.
    Exhausted,
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    /// How many steps have completed. Zero before the first one returns.
    pub iterations: u32,
    /// What has been spent, in the run's own unit — [`None`] until the first step establishes it.
    pub cost: Option<Cost>,
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
            };
        }
    }

    /// Drive `plugin` over `panes` until a terminal state, reporting the
    /// [`Outcome`].
    #[must_use]
    pub fn run(
        mut self,
        plugin: &mut dyn Plugin,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Outcome {
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
            let event = match plugin.step(panes, run) {
                Err(error) => {
                    self.failure = Some(error);
                    OrchestrationEvent::Fail
                }
                Ok(step) => {
                    self.iterations += 1;
                    self.accumulate(step.cost);
                    self.publish();
                    match step.verdict {
                        Verdict::Converged => OrchestrationEvent::Converge,
                        Verdict::Continue if self.budget_exhausted() => OrchestrationEvent::Exhaust,
                        Verdict::Continue => OrchestrationEvent::Continue,
                    }
                }
            };
            self.engine.process_event(event);
        }
        self.outcome()
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

    fn budget_exhausted(&self) -> bool {
        if self.iterations >= self.guardrails.max_iterations {
            return true;
        }
        matches!(
            (self.cost, self.guardrails.max_cost),
            (Some(acc), Some(max)) if acc.reaches(max)
        )
    }

    fn outcome(self) -> Outcome {
        let state = match self.engine.get_current_state() {
            OrchestrationState::Converged => OutcomeState::Converged,
            OrchestrationState::Exhausted => OutcomeState::Exhausted,
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
        })
        .run(&mut FailingPlugin, &NoPanes, &RunContext::new(cancel));
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        assert_eq!(outcome.iterations, 0);
        assert!(outcome.failure.is_none());
    }
}
