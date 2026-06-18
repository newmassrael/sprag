//! The `Orchestrator` plugin — a fixed-stimulus drive loop (plugin #1).
//!
//! Each step injects a fixed stimulus into one pane, waits for the pane to
//! react (via the producer's damage `generation`s), and converges when a
//! sentinel appears in the pane's output. It is the first [`Plugin`] consumer
//! of the [`PaneAccess`] extension API; the guardrails live in the [`Driver`].
//!
//! [`Driver`]: crate::driver::Driver

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::plugin::{Plugin, Step, Verdict};
use crate::run::{poll_until, RunContext, Waited};

/// How long a step waits for the pane to react before judging on the current
/// screen.
const OBSERVE_TIMEOUT: Duration = Duration::from_millis(500);

/// What the orchestrator drives toward (the guardrails live in [`Guardrails`]).
///
/// [`Guardrails`]: crate::driver::Guardrails
#[derive(Clone, Debug)]
pub struct OrchestrationSpec {
    /// Text injected into the pane each step (followed by Enter).
    pub stimulus: String,
    /// Convergence condition: succeed once the pane's collapsed text contains
    /// this. `None` runs until a guardrail.
    pub sentinel: Option<String>,
}

/// A fixed-stimulus drive plugin over one pane.
pub struct Orchestrator {
    pane: PaneId,
    spec: OrchestrationSpec,
    /// Per-row damage generations captured before the last stimulus, so the
    /// observe-wait keys on *this* step's echo.
    baseline_generations: Vec<u64>,
}

impl Orchestrator {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: OrchestrationSpec) -> Self {
        Self {
            pane,
            spec,
            baseline_generations: Vec::new(),
        }
    }

    /// Wait (bounded, cancellable) for any row's damage `generation` to advance
    /// past the pre-stimulus baseline.
    fn observe(&self, panes: &dyn PaneAccess, run: &RunContext) -> Waited {
        poll_until(run, OBSERVE_TIMEOUT, || {
            panes.pane_rows(self.pane).is_some_and(|rows| {
                rows.iter().enumerate().any(|(i, row)| {
                    row.generation > self.baseline_generations.get(i).copied().unwrap_or(0)
                })
            })
        })
    }
}

impl Plugin for Orchestrator {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // Baseline before acting, so observe() waits for this step's echo.
        self.baseline_generations = panes
            .pane_rows(self.pane)
            .map(|rows| rows.iter().map(|row| row.generation).collect())
            .unwrap_or_default();

        // Act: inject the stimulus + Enter.
        let mut keys = KeyStroke::text(&self.spec.stimulus);
        keys.push(KeyStroke::named("Enter"));
        let cost = panes.inject(self.pane, &keys)?;

        // Perceive, then judge against the collapsed (wrap-safe) screen text.
        // If cancelled mid-observe, don't judge — return Continue so the
        // Driver's loop-top ends the run Cancelled (not a spurious Converged).
        if self.observe(panes, run) == Waited::Cancelled {
            return Ok(Step {
                cost,
                verdict: Verdict::Continue,
            });
        }
        let observed = panes.pane_collapsed(self.pane).unwrap_or_default();
        let verdict = if self
            .spec
            .sentinel
            .as_ref()
            .is_some_and(|sentinel| observed.contains(sentinel.as_str()))
        {
            Verdict::Converged
        } else {
            Verdict::Continue
        };
        Ok(Step {
            cost,
            verdict,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Driver, Guardrails, OutcomeState};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};

    /// A workspace with one live `cat` pane, wrapped as pane-access.
    fn cat_access(cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .unwrap()
            .spawn(command, "cat".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    fn run(
        access: &WorkspacePaneAccess,
        plugin: &mut Orchestrator,
        guardrails: Guardrails,
    ) -> crate::driver::Outcome {
        Driver::new(guardrails).run(plugin, access, &crate::run::RunContext::uncancellable())
    }

    #[test]
    fn exhausts_after_max_iterations() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: u64::MAX,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert_eq!(outcome.iterations, 3);
        assert!(outcome.failure.is_none());
    }

    #[test]
    fn converges_on_sentinel() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("ping".to_string()),
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 10,
                max_cost: u64::MAX,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
        assert!(outcome.iterations >= 1, "iterations: {}", outcome.iterations);
    }

    #[test]
    fn converges_on_a_wrapped_sentinel() {
        // A 4-column pane wraps the 6-char echo across rows; the collapsed
        // match still finds "abcdef".
        let (access, pane) = cat_access(4, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "abcdef".to_string(),
                sentinel: Some("abcdef".to_string()),
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 10,
                max_cost: u64::MAX,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
    }

    #[test]
    fn cost_budget_also_terminates() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(), // "ping" + Enter = 5 bytes/step
                sentinel: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: u32::MAX,
                max_cost: 12,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert!(outcome.cost >= 12, "bytes: {}", outcome.cost);
    }
}
