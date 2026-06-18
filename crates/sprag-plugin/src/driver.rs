//! `Driver` — the shared control substrate.
//!
//! Owns the SCE/SCXML statechart (`orchestration.scxml`: idle → running →
//! converged/exhausted/failed) and the guardrails, and runs any [`Plugin`] to a
//! terminal [`Outcome`]. Each microstep is one `Plugin::step`; the Driver
//! enforces the iteration and cost budgets and maps the result onto the
//! statechart. The plugin owns the behaviour; the Driver owns the lifecycle.

use sce_rust_runtime::Engine;

use crate::access::{InjectError, PaneAccess};
use crate::plugin::{Plugin, Verdict};
use crate::sm::{OrchestrationEvent, OrchestrationPolicy, OrchestrationState};

/// The termination guardrails every plugin run is bounded by (first-class
/// safety per the README — an AI control loop must not run unbounded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guardrails {
    /// Stop after this many steps.
    pub max_iterations: u32,
    /// Stop once this many bytes have been injected (a headless proxy for
    /// token/$ spend; a real AI adapter replaces it with token cost).
    pub max_injected_bytes: u64,
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
}

/// The result of a plugin run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub state: OutcomeState,
    pub iterations: u32,
    pub injected_bytes: u64,
    /// The cause when `state` is [`OutcomeState::Failed`]; `None` otherwise.
    pub failure: Option<InjectError>,
}

/// Runs a [`Plugin`] over a [`PaneAccess`] to a terminal [`Outcome`], owning the
/// statechart + guardrail counters.
pub struct Driver {
    engine: Engine<OrchestrationPolicy>,
    guardrails: Guardrails,
    iterations: u32,
    injected_bytes: u64,
    failure: Option<InjectError>,
}

impl Driver {
    /// A driver bounded by `guardrails`.
    #[must_use]
    pub fn new(guardrails: Guardrails) -> Self {
        Self {
            engine: Engine::new(OrchestrationPolicy::new()),
            guardrails,
            iterations: 0,
            injected_bytes: 0,
            failure: None,
        }
    }

    /// Drive `plugin` over `panes` until a terminal state, reporting the
    /// [`Outcome`].
    #[must_use]
    pub fn run(mut self, plugin: &mut dyn Plugin, panes: &dyn PaneAccess) -> Outcome {
        self.engine.initialize();
        self.engine.process_event(OrchestrationEvent::Start);
        // `running` is the only non-final state in the loop.
        while !self.engine.is_in_final_state() {
            let event = match plugin.step(panes) {
                Err(error) => {
                    self.failure = Some(error);
                    OrchestrationEvent::Fail
                }
                Ok(step) => {
                    self.iterations += 1;
                    self.injected_bytes += step.injected_bytes;
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

    fn budget_exhausted(&self) -> bool {
        self.iterations >= self.guardrails.max_iterations
            || self.injected_bytes >= self.guardrails.max_injected_bytes
    }

    fn outcome(self) -> Outcome {
        let state = match self.engine.get_current_state() {
            OrchestrationState::Converged => OutcomeState::Converged,
            OrchestrationState::Exhausted => OutcomeState::Exhausted,
            // Failed, or any state the loop left unexpectedly.
            _ => OutcomeState::Failed,
        };
        Outcome {
            state,
            iterations: self.iterations,
            injected_bytes: self.injected_bytes,
            failure: self.failure,
        }
    }
}
