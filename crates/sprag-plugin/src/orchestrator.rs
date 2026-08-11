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
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::run::{RunContext, Waited, poll_until};

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

    /// Wait (bounded, cancellable) for the PEER to answer — a row whose damage
    /// `generation` has advanced past the pre-stimulus baseline AND that carries
    /// something other than the stimulus this step just typed.
    fn observe(&self, panes: &dyn PaneAccess, run: &RunContext) -> Waited {
        poll_until(run, OBSERVE_TIMEOUT, || {
            self.reaction(panes) == Reaction::Answered
        })
    }

    /// What the pane has done since this step's baseline.
    ///
    /// # ⚠⚠ Why the ECHO had to stop counting as a reaction
    ///
    /// A pty in cooked mode echoes what is injected before the program behind it
    /// has read a byte. Keying the wait on "any row changed" therefore ended EVERY
    /// step in microseconds against EVERY ordinary pane: the screen was judged
    /// before the peer had said anything, no sentinel was there, and the loop took
    /// another turn — re-prompting a peer that was still answering the last one. A
    /// peer replying in 200ms, well inside one step's [`OBSERVE_TIMEOUT`], was
    /// measured burning all three of a run's turns in 30 MILLISECONDS and reported
    /// `exhausted`. `max_iterations` was bounding a loop that had never once
    /// waited for a reply.
    ///
    /// ⚠ It FAILS SAFE. A real answer misread as an echo only costs the rest of
    /// the step's wait: the verdict is judged off the collapsed screen after the
    /// wait either way, so a convergence can be reached late but never lost.
    fn reaction(&self, panes: &dyn PaneAccess) -> Reaction {
        let Some(rows) = panes.pane_rows(self.pane) else {
            return Reaction::None;
        };
        let changed: Vec<&str> = rows
            .iter()
            .enumerate()
            .filter(|(i, row)| {
                row.generation > self.baseline_generations.get(*i).copied().unwrap_or(0)
            })
            .map(|(_, row)| row.text.trim())
            .collect();
        if changed.is_empty() {
            return Reaction::None;
        }
        // A changed row is the ECHO when what it holds is a piece of what was just typed — the
        // `contains` covers a stimulus the pane wrapped across rows. A blank row is no evidence of
        // an answer either.
        if changed
            .iter()
            .all(|line| line.is_empty() || self.spec.stimulus.contains(line))
        {
            return Reaction::EchoOnly;
        }
        Reaction::Answered
    }
}

/// What a pane has done since a step's baseline — the three cases a step must tell apart, because
/// two of them are the same absence of an answer with different remedies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reaction {
    /// Nothing on the pane changed at all: the peer is not listening, or is not there.
    None,
    /// Only the stimulus came back — the terminal's own echo, not the peer.
    EchoOnly,
    /// Something the peer produced.
    Answered,
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
        let cost = panes.inject(self.pane, &keys)?.bytes();

        // Perceive, then judge against the collapsed (wrap-safe) screen text.
        // If the RUN ended mid-observe — cancelled, or out of time — don't judge:
        // return Continue so the Driver's loop top decides the terminal state,
        // rather than a spurious Converged off a screen nobody finished reading.
        let seen = self.observe(panes, run);
        if seen == Waited::Stopped {
            return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                .noting("the run ended while watching for the pane to react"));
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
        // ⚠ A STIMULUS THE PANE NEVER REACTED TO IS THE FINDING, and it is invisible in the
        // outcome: the step costs the same bytes and reads `continue` either way, so a hundred
        // iterations against a pane that is not listening look exactly like a hundred against one
        // that is.
        let note = match (seen, verdict) {
            (_, Verdict::Converged) => "the sentinel appeared".to_string(),
            // The two ways a step can end with no answer are different findings with different
            // remedies: a pane showing NOTHING is one nobody is listening on, while one that
            // echoed and said no more is a peer that heard and did not reply.
            (Waited::TimedOut, _) => match self.reaction(panes) {
                Reaction::Answered => "the pane answered as the step's wait ran out".to_string(),
                Reaction::EchoOnly => {
                    "the stimulus was echoed back and THE PEER SAID NOTHING".to_string()
                }
                Reaction::None => "the pane did not react to the stimulus at all".to_string(),
            },
            _ => "the peer answered; no sentinel yet".to_string(),
        };
        Ok(Step::new(Cost::Bytes(cost), verdict).noting(note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};

    /// A workspace with one pane running `script`, wrapped as pane-access.
    fn sh_access(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    /// A workspace with one live `cat` pane, wrapped as pane-access.
    fn cat_access(cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        sh_access("cat", cols, rows)
    }

    /// What a pane that cannot react runs: echo off, a readiness marker, then a reader that
    /// discards. The marker is the load-bearing part — see [`await_ready`].
    const DEAF: &str = "stty -echo; printf DEAF-READY; exec cat >/dev/null";

    /// Block until the deaf pane has finished starting up.
    ///
    /// ⚠⚠ WITHOUT THIS THE PANE IS NOT YET DEAF WHEN THE RUN BEGINS. A pane is spawned with the
    /// pty's default echo ON, and the shell needs a moment to reach its `stty`; a run that starts
    /// driving in that window has its FIRST stimulus echoed back and reads a pane that cannot hear
    /// it as one that reacted. That is not a slow machine to be waited out — it is the run racing
    /// the pane's own startup, and the marker is how the race is settled rather than survived.
    fn await_ready(access: &WorkspacePaneAccess, pane: PaneId) {
        let waited = poll_until(
            &RunContext::uncancellable(),
            Duration::from_secs(10),
            || {
                access
                    .pane_collapsed(pane)
                    .is_some_and(|text| text.contains("DEAF-READY"))
            },
        );
        assert_eq!(waited, Waited::Ready, "the deaf pane never came up");
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
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
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
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
        assert!(
            outcome.iterations >= 1,
            "iterations: {}",
            outcome.iterations
        );
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
                max_cost: None,
                max_duration: None,
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
                max_cost: Some(Cost::Bytes(12)),
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Cost));
        assert!(
            matches!(outcome.cost, Some(Cost::Bytes(n)) if n >= 12),
            "cost: {:?}",
            outcome.cost
        );
    }

    /// ⚠⚠ **A PEER THAT ANSWERS IS WAITED FOR; ITS OWN ECHO IS NOT AN ANSWER.**
    ///
    /// A pty in cooked mode echoes what is injected before the program has read a byte of it. If
    /// that echo satisfies the observe-wait, then EVERY turn against EVERY ordinary pane ends in
    /// microseconds, the screen is judged before the peer has said anything, and the loop takes
    /// another turn — spamming a peer that was already thinking. `max_iterations` then bounds a
    /// run that never waited for one reply.
    ///
    /// The peer here answers in 200ms, comfortably inside one step's [`OBSERVE_TIMEOUT`]. A loop
    /// that waits for its peer converges on the FIRST turn. A loop that races its own echo burns
    /// all three turns before the answer lands and reports `exhausted` about a peer that replied.
    #[test]
    fn a_turn_waits_for_the_peer_and_not_for_the_echo_of_what_it_typed() {
        // Reads a line, thinks, then answers. The kernel echoes the injected line long before the
        // `sleep` is over, which is exactly the difference under test.
        let (access, pane) = sh_access(
            "while read line; do sleep 0.2; echo PEER-REPLIED; done",
            40,
            8,
        );
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-REPLIED".to_string()),
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers well inside one step's observe timeout, so a loop that waits for it \
             converges; this run gave up after {} turns against a peer that was replying",
            outcome.iterations,
        );
        assert_eq!(
            outcome.iterations, 1,
            "and it converges on the FIRST turn — a second turn means the first was judged on a \
             screen holding nothing but the echo of what it had just typed, and the peer was \
             prompted again while it was still answering",
        );
    }

    /// ⚠⚠ **A PANE THAT CANNOT REACT PUTS A FLOOR UNDER EVERY STEP**, which is the only thing that
    /// lets a gate ask WHICH ceiling stopped a run without racing the machine it runs on.
    ///
    /// Against a pane that echoes, a step ends the instant the echo lands, so a run's turn count
    /// is a function of how fast the box is: the same one-second run took 97 turns here and would
    /// take a different number anywhere else. Deaf, every step waits [`OBSERVE_TIMEOUT`] out in
    /// full, so the turns a timed run can fit are arithmetic — and a slower box only makes the
    /// floor higher, never lower.
    ///
    /// Both halves are asserted because either alone is a weaker claim than it reads as:
    ///
    /// * The pane really is DEAF — the step notes say so. Without this the run below could be
    ///   ending by the clock for the ordinary reason, and the floor this gate is about would be
    ///   absent with nothing to notice it.
    /// * The turns it fitted are FAR below the iteration ceiling it also asked for, so `duration`
    ///   is the only ceiling that was ever in reach.
    #[test]
    fn a_deaf_pane_floors_every_step_so_the_clock_is_the_only_ceiling_in_reach() {
        // `stty -echo` stops the kernel echoing the injection; the reader discards what it reads.
        // Once ready, nothing this run does can reach the screen.
        let (access, pane) = sh_access(DEAF, 20, 4);
        await_ready(&access, pane);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("A SENTINEL THIS PANE NEVER PRINTS".to_string()),
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(1_200)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut orch, &access, &crate::run::RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "a hundred turns were on offer and the clock is what ran out",
        );
        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        assert!(
            !notes.iter().any(|note| note.contains("the pane reacted")),
            "no step may have found this pane reacting, or the floor this gate rests on is not \
             there: {notes:?}; the pane shows {:?}",
            access.pane_collapsed(pane),
        );
        assert_eq!(
            notes.last().map(String::as_str),
            Some("the run ended while watching for the pane to react"),
            "AND THE LAST STEP IS ONE THE CLOCK CUT MID-OBSERVE — the deadline reaching inside a \
             step, which is the whole difference between this ceiling and the two that are decided \
             between them. A run whose final step ran its observe out in full would end by the \
             same `duration` and prove only the loop top: {notes:?}",
        );
        assert!(
            outcome.iterations <= 4,
            "a step floored at {OBSERVE_TIMEOUT:?} cannot fit more than a handful into 1.2s — \
             {} turns says the floor is missing",
            outcome.iterations,
        );
    }
}
