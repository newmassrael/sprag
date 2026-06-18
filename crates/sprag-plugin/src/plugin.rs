//! `Plugin` — the control-plugin extension point.
//!
//! A plugin owns its perceive + act + judge behaviour; the [`Driver`] owns the
//! statechart lifecycle and the guardrails around each [`step`](Plugin::step).
//! That is the SOLID seam: what is uniform (termination topology, guardrails,
//! outcome mapping) lives in the Driver; what is plugin-specific (when/how to
//! read a pane, what to inject, when to converge) lives here.
//!
//! [`Driver`]: crate::driver::Driver

use crate::access::{PaneAccess, PaneError};
use crate::run::RunContext;

/// A plugin's verdict for one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep going (subject to the Driver's guardrails).
    Continue,
    /// The plugin reached its goal; the run converges.
    Converged,
}

/// A typed cost quantity — what a [`Step`] spent, with its UNIT in the type.
///
/// A run drives exactly one plugin, and a plugin reports the SAME unit every
/// step, so a run accumulates spend in one currency and the two variants never
/// mix within a run (the [`Driver`] never sums bytes against tokens). Bytes and
/// tokens have no exchange rate — typing the unit makes that a compile-time fact
/// rather than a convention, so the cost guardrail (the platform's defence
/// against runaway spend) can never silently bind one currency with another's
/// budget.
///
/// A new cost unit (a future tool measured in dollars or API calls) is a new
/// variant here; the `Driver` / `Guardrails` / `Outcome` stay generic over
/// `Cost`.
///
/// [`Driver`]: crate::driver::Driver
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// PTY bytes injected into a pane — the natural unit of the byte-relay
    /// plugins (`Orchestrator`, `Pipe`, `Agent`).
    Bytes(u64),
    /// Real billed LLM tokens (input + output) — a conversation plugin's natural
    /// unit. A turn whose tokens cannot be measured (a print-mode endpoint, or a
    /// degraded / cancelled turn) reports `Tokens(0)`: no measured spend. For
    /// such a turn the iteration budget, not cost, is the liveness guarantee.
    Tokens(u64),
}

impl Cost {
    /// The scalar amount, dropping the unit.
    #[must_use]
    pub const fn amount(self) -> u64 {
        match self {
            Cost::Bytes(n) | Cost::Tokens(n) => n,
        }
    }

    /// Sum two costs of the SAME unit (saturating). `None` if the units differ —
    /// which a single run never produces (one plugin reports one unit), so this
    /// is a defensive guard, not an expected path.
    pub(crate) fn try_add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Cost::Bytes(a), Cost::Bytes(b)) => Some(Cost::Bytes(a.saturating_add(b))),
            (Cost::Tokens(a), Cost::Tokens(b)) => Some(Cost::Tokens(a.saturating_add(b))),
            _ => None,
        }
    }

    /// Whether this accumulated cost has reached the `bound` of the same unit. A
    /// bound of a different unit does not bind (defensive; a run's steps and its
    /// bound share a unit by construction).
    pub(crate) fn reaches(self, bound: Self) -> bool {
        match (self, bound) {
            (Cost::Bytes(a), Cost::Bytes(b)) | (Cost::Tokens(a), Cost::Tokens(b)) => a >= b,
            _ => false,
        }
    }
}

/// What a [`Plugin::step`] did and decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// What this step spent on the peer, as a typed [`Cost`]: injected/argv bytes
    /// for the byte-relay plugins, real billed tokens for an AI adapter. A run
    /// drives ONE plugin reporting ONE unit, so the Driver accumulates and bounds
    /// without ever summing across units. Cost is non-negative and may be zero (a
    /// `Tokens(0)` print-mode/degraded turn) — the iteration budget, not cost, is
    /// the liveness guarantee, so a cost-free turn cannot loop forever.
    pub cost: Cost,
    pub verdict: Verdict,
}

/// A control plugin driven over the [`PaneAccess`] extension API.
pub trait Plugin {
    /// Perceive the panes, act on them, and judge — one step.
    ///
    /// The Driver calls this each microstep, enforces the guardrails around it,
    /// and maps the result onto the statechart. An error aborts the run
    /// (mapped to the `failed` terminal state). `run` carries the run-scoped
    /// signals (cancellation): a plugin's bounded waits should consult it so a
    /// long in-flight step aborts promptly.
    ///
    /// # Errors
    ///
    /// [`PaneError`] when a pane operation fails — unknown pane, unencodable
    /// key, a write failure, or a pane spawn failure (an AI dialogue).
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError>;

    /// Content the plugin captured during its run — e.g. an AI adapter's
    /// response text — read by the host after the run completes and surfaced as
    /// scene-as-data. The [`Driver`] never touches it (it stays content-
    /// agnostic); control plugins that produce no content keep the default
    /// `None`.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn captured(&self) -> Option<String> {
        None
    }
}
