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

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::run::{RunContext, Waited, poll_until};

/// How long a relay waits for its destination to show ANY change before reporting that it showed
/// nothing.
///
/// # ⚠ Why a reaction and not a confirmation of the TEXT
///
/// [`deliver`](crate::deliver::deliver) confirms that specific text appeared, which is what a
/// prompt needs and what a relay cannot use: relayed output is arbitrary and often longer than the
/// destination is wide, so it arrives WRAPPED and no `contains` over the rendered screen can find
/// it whole. A damage-generation bump is wrap-proof, needle-free, and answers the question a relay
/// actually has — *did anything happen over there?*
///
/// Sized like the orchestrator's own observe window: an echo is a round trip through a pty, which
/// is microseconds when the peer is reading.
const REACTION_TIMEOUT: Duration = Duration::from_millis(500);

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

impl Pipe {
    /// Every row's damage generation on `pane`, or empty for a pane that is gone.
    fn generations(panes: &dyn PaneAccess, pane: PaneId) -> Vec<u64> {
        panes
            .pane_rows(pane)
            .map(|rows| rows.iter().map(|row| row.generation).collect())
            .unwrap_or_default()
    }
}

impl Plugin for Pipe {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
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
        if relayed.is_empty() {
            return Ok(
                Step::new(Cost::Bytes(0), Verdict::Continue).noting("nothing new on the source")
            );
        }

        // ⚠⚠ WATCH THE DESTINATION REACT. This plugin's whole job is delivery and it was the one
        // that never looked at where it delivered: it read the source, wrote the destination,
        // charged the bytes, and answered `continue` forever. A pipe relaying into a pane that
        // swallows its input is indistinguishable, in every number a run reports, from one that is
        // working — so the failure this plugin exists to have is the failure it could not report.
        let before = Self::generations(panes, self.dst);
        let cost = panes.inject(self.dst, &KeyStroke::text(&relayed))?.bytes();
        let reacted = poll_until(run, REACTION_TIMEOUT, || {
            Self::generations(panes, self.dst)
                .iter()
                .enumerate()
                .any(|(i, now)| *now > before.get(i).copied().unwrap_or(0))
        });

        // The pipe never self-terminates; the Driver's guardrails bind it.
        Ok(
            Step::new(Cost::Bytes(cost), Verdict::Continue).noting(match reacted {
                Waited::Ready => format!("relayed {cost} bytes; the destination reacted"),
                Waited::TimedOut => {
                    format!("relayed {cost} bytes and THE DESTINATION SHOWED NOTHING")
                }
                Waited::Stopped => format!("relayed {cost} bytes; the run ended"),
            }),
        )
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

    /// ⚠⚠ **A RELAY INTO A PANE THAT SWALLOWS IT SAYS SO** — the one plugin whose whole job is
    /// delivery was the one that never looked at where it delivered.
    ///
    /// Both halves against the same fixture, because every NUMBER a run reports is identical
    /// either way: same bytes charged, same `continue`, same `exhausted`. The only thing that can
    /// tell the two apart is the journal, which is exactly why this defect survived.
    ///
    /// The deaf destination is a child that reads its input and prints nothing (`cat >/dev/null`
    /// with echo off) — a real program doing a reasonable thing, not a broken pane. The pane is
    /// alive, the write succeeds, the bytes are charged, and nothing appears.
    #[test]
    fn a_relay_into_a_destination_that_shows_nothing_reports_that_it_showed_nothing() {
        let relay_into = |dst_script: &str| {
            let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
            let spawn = |script: &str| {
                let mut command = CommandBuilder::new("/bin/sh");
                command.arg("-c");
                command.arg(script);
                command.env("TERM", "dumb");
                workspace
                    .lock()
                    .unwrap()
                    .spawn(command, "peer".to_string(), 20, 4)
                    .expect("spawn")
            };
            let src = spawn("cat");
            let dst = spawn(dst_script);
            let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

            let mut seed = KeyStroke::text("relayme");
            seed.push(KeyStroke::named("Enter"));
            let _seeded = access.inject(src, &seed).expect("seed src");
            assert!(wait_until(&access, src, "relayme"), "source never echoed");

            let journal = Arc::new(Mutex::new(sprag_plugin_progress()));
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: None,
            })
            .reporting_to(Arc::clone(&journal))
            .run(
                &mut Pipe::new(src, dst),
                &access,
                &RunContext::uncancellable(),
            );
            assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
            let notes: Vec<String> = journal
                .lock()
                .unwrap()
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            notes.join(" | ")
        };

        // THE CONTROL FIRST — a destination that echoes. If this did not say "reacted", the
        // subject below would be measuring a check that never passes for anyone.
        let heard = relay_into("cat");
        assert!(
            heard.contains("the destination reacted"),
            "a live echoing destination must read as reacting: {heard}",
        );

        // THE SUBJECT — a destination that consumes its input and prints nothing.
        let deaf = relay_into("stty -echo; cat >/dev/null");
        assert!(
            deaf.contains("THE DESTINATION SHOWED NOTHING"),
            "a relay into a pane that shows nothing must say so — no number in the outcome can: \
             {deaf}",
        );
    }

    /// The journal cell a `Driver` reports into, spelled once for the test above.
    fn sprag_plugin_progress() -> crate::driver::Progress {
        crate::driver::Progress::default()
    }
}
