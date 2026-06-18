//! `Plugin` — the control-plugin extension point.
//!
//! A plugin owns its perceive + act + judge behaviour; the [`Driver`] owns the
//! statechart lifecycle and the guardrails around each [`step`](Plugin::step).
//! That is the SOLID seam: what is uniform (termination topology, guardrails,
//! outcome mapping) lives in the Driver; what is plugin-specific (when/how to
//! read a pane, what to inject, when to converge) lives here.
//!
//! [`Driver`]: crate::driver::Driver

use crate::access::{InjectError, PaneAccess};

/// A plugin's verdict for one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep going (subject to the Driver's guardrails).
    Continue,
    /// The plugin reached its goal; the run converges.
    Converged,
}

/// What a [`Plugin::step`] did and decided. `injected_bytes` feeds the Driver's
/// cost guardrail — the budget lives in one place (the Driver), the inject
/// happens in the plugin, and this reports the count across the seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub injected_bytes: u64,
    pub verdict: Verdict,
}

/// A control plugin driven over the [`PaneAccess`] extension API.
pub trait Plugin {
    /// Perceive the panes, act on them, and judge — one step.
    ///
    /// The Driver calls this each microstep, enforces the guardrails around it,
    /// and maps the result onto the statechart. An error aborts the run
    /// (mapped to the `failed` terminal state).
    ///
    /// # Errors
    ///
    /// [`InjectError`] when injecting input fails (unknown pane, unencodable
    /// key, or write failure).
    fn step(&mut self, panes: &dyn PaneAccess) -> Result<Step, InjectError>;
}
