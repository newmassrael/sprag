//! The `Pipe` plugin — relay one pane's output into another (plugin #2).
//!
//! The second consumer that validates the extension API. Each step reads the
//! *source* pane and injects the text of whatever rows are newly damaged since
//! its last relay into the *destination* pane. It never self-converges — the
//! [`Driver`]'s guardrails bind it (first-class safety per the README). This
//! proves the substrate generalizes beyond the orchestrator: the same
//! damage-`generation` primitive, but the cursor is "after last relay" rather
//! than "before last stimulus", and it lives in the plugin (not the Driver).
//!
//! Known limitation (for the AI↔AI relay layer, not now): a row is relayed
//! whole whenever its generation bumps, not as a character-level delta — over-
//! relays for in-place redraws, correct for an append/echo source.
//!
//! [`Driver`]: crate::driver::Driver

use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::run::RunContext;

/// Relays the source pane's new output into the destination pane.
pub struct Pipe {
    src: PaneId,
    dst: PaneId,
    /// Last-relayed damage generation per source row.
    consumed: Vec<u64>,
}

impl Pipe {
    /// Relay `src`'s output into `dst`.
    #[must_use]
    pub fn new(src: PaneId, dst: PaneId) -> Self {
        Self {
            src,
            dst,
            consumed: Vec::new(),
        }
    }
}

impl Plugin for Pipe {
    fn step(&mut self, panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
        let rows = panes.pane_rows(self.src).unwrap_or_default();
        if self.consumed.len() < rows.len() {
            self.consumed.resize(rows.len(), 0);
        }
        // Collect the text of rows newly damaged since the last relay.
        let mut relayed = String::new();
        for (i, row) in rows.iter().enumerate() {
            if row.generation > self.consumed[i] {
                relayed.push_str(&row.text);
                self.consumed[i] = row.generation;
            }
        }
        let cost = if relayed.is_empty() {
            0
        } else {
            panes.inject(self.dst, &KeyStroke::text(&relayed))?.bytes()
        };
        // The pipe never self-terminates; the Driver's guardrails bind it.
        Ok(Step {
            cost: Cost::Bytes(cost),
            verdict: Verdict::Continue,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// A workspace with two live `cat` panes, wrapped as pane-access.
    fn two_cat_panes() -> (WorkspacePaneAccess, PaneId, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let spawn = |ws: &Arc<Mutex<Workspace>>| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("cat");
            command.env("TERM", "dumb");
            ws.lock()
                .unwrap()
                .spawn(command, "cat".to_string(), 20, 4)
                .expect("spawn")
        };
        let src = spawn(&workspace);
        let dst = spawn(&workspace);
        (WorkspacePaneAccess::new(workspace), src, dst)
    }

    fn wait_until(access: &WorkspacePaneAccess, pane: PaneId, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains(needle))
            {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn relays_source_output_into_destination() {
        let (access, src, dst) = two_cat_panes();

        // Seed the source out-of-band and wait for it to echo, so the pipe's
        // first step has real output to relay.
        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(
            wait_until(&access, src, "relayme"),
            "source never echoed the seed"
        );

        // The pipe never converges; the iteration budget binds it.
        let mut pipe = Pipe::new(src, dst);
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut pipe, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));

        // The destination received the relayed text (its echo is async).
        assert!(
            wait_until(&access, dst, "relayme"),
            "destination never received the relay"
        );
    }
}
