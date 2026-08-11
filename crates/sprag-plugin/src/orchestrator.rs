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

#[cfg(test)]
use crate::access::{JobLeader, PaneDoing};
use crate::access::{KeyStroke, PaneAccess, PaneError, RowTrail};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness, ReadyWhen};
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
    /// What the pane must SHOW before the first stimulus is injected — see
    /// [`Readiness`], which is where this barrier lives and why it exists. `None`
    /// starts driving immediately, which is right for a pane already running the
    /// program.
    pub ready_when: Option<ReadyWhen>,
    /// How long to wait for [`ready_when`](Self::ready_when), or `None` for
    /// [`DEFAULT_READY_TIMEOUT`](crate::readiness::DEFAULT_READY_TIMEOUT).
    ///
    /// The caller's, because how long a program takes to start is the thing that
    /// varies most between the programs this drives — `cat` is instant, an agent
    /// takes seconds, a cold test runner minutes — and the caller who names the
    /// marker is exactly the one who knows.
    pub ready_within: Option<Duration>,
}

/// A fixed-stimulus drive plugin over one pane.
pub struct Orchestrator {
    pane: PaneId,
    spec: OrchestrationSpec,
    /// What the pane's rows HELD before the last stimulus, so the observe-wait keys on *this*
    /// step's reply. ⚠ Text and not damage generations — see [`RowTrail`].
    baseline: RowTrail,
    /// The barrier this run must clear before it types anything — see [`Readiness`].
    ready: Readiness,
}

impl Orchestrator {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: OrchestrationSpec) -> Self {
        Self {
            ready: Readiness::new(spec.ready_when.clone(), spec.ready_within),
            pane,
            spec,
            baseline: RowTrail::default(),
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
        let changed = self.baseline.fresh(panes, self.pane);
        if changed.is_empty() {
            return Reaction::None;
        }
        // A changed row is the ECHO when what it holds is a piece of what was just typed — the
        // `contains` covers a stimulus the pane wrapped across rows. A blank row is no evidence of
        // an answer either.
        if changed
            .iter()
            .all(|line| line.trim().is_empty() || self.spec.stimulus.contains(line.trim()))
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
        // ⚠⚠ NOT ONE BYTE UNTIL THE PANE IS READY. Injecting into a pane whose program has not
        // started is spending a turn on the shell that is still there — see [`Readiness`], which
        // owns this barrier and the `NeverReady` failure. Latched, so it costs nothing after the
        // first step.
        if self.ready.reached(panes, self.pane, run)? == Reached::RunEnded {
            // Nothing was injected, so nothing is charged; the Driver's loop top says which of the
            // two ways the run ended it was.
            return Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting("the run ended while waiting for the pane to be ready"));
        }

        // Baseline before acting, so observe() waits for this step's reply.
        self.baseline = RowTrail::mark(panes, self.pane);

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
    use crate::testing::STANDIN_READS_TTY;
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

    /// A silent program's argv, and the ONE readiness spec both ways of reaching it are driven
    /// with.
    ///
    /// ⚠⚠ **THE SYMMETRY IS THE CLAIM, SO IT IS A SHARED FUNCTION RATHER THAN A SENTENCE.** Two
    /// gates below start this program two entirely different ways — a shell that is typed at until
    /// it `exec`s, and a pane OPENED running it (`open_pane`'s `cmd`, reached here through
    /// [`PaneLifecycle::spawn`]) — and both converge on this identical value. A prose claim that
    /// one value serves both shapes is a claim; a value neither gate can vary is the fact.
    ///
    /// `tr` is the fixture because it is `cat` with a witness: silent until fed, then provably
    /// itself, because `PING` is not a spelling the pty's echo of `ping` can produce.
    const SILENT_PROGRAM: [&str; 3] = ["tr", "a-z", "A-Z"];

    fn drive_the_silent_program() -> OrchestrationSpec {
        OrchestrationSpec {
            stimulus: "ping".to_string(),
            sentinel: Some("PING".to_string()),
            // No marker at all: the pane's TERMINAL says what is running in it.
            ready_when: Some(ReadyWhen::Runs(SILENT_PROGRAM[0].to_string())),
            // ⚠ BOUNDED, though both fixtures are ready well inside it. Left unbounded this
            // inherits the two-MINUTE default, so a mutation that makes the barrier never clear
            // costs the suite two minutes to report what it can report in fifteen seconds. A
            // gate's failing path has a running time too.
            ready_within: Some(Duration::from_secs(15)),
        }
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
                ready_when: None,
                ready_within: None,
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
                ready_when: None,
                ready_within: None,
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
                ready_when: None,
                ready_within: None,
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
                ready_when: None,
                ready_within: None,
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

    /// ⚠⚠ **A PANE IS NOT READY WHEN IT IS OPEN**, and a run told what ready looks like spends no
    /// turn before it.
    ///
    /// The pane here is born a shell and becomes the peer a second later — the ordinary shape of
    /// *open a pane, start `claude` in it, drive it*. A run that starts immediately injects into
    /// the SHELL, which executes the stimulus as a command; by the time the peer exists its turns
    /// are gone and the guardrails have counted them.
    ///
    /// Both halves, because either alone is weak: the run must CONVERGE (so the wait ended and the
    /// driving worked), and the STAND-IN SHELL must never have been fed — it says so itself.
    ///
    /// ⚠⚠ THE FIXTURE HAD TO BE REBUILT TWICE, AND THE SECOND TIME IS WHY IT MEASURES ANYTHING.
    /// Its first form let the pane merely SLEEP before becoming the peer, and a mutation that
    /// ignored `ready_when` entirely still passed: nothing consumed the early stimulus, so it sat
    /// in the pty buffer and the peer read it when it started. A pane that is not ready has to be
    /// one that EATS what it is given, which is what a real shell does with a stimulus meant for
    /// something else.
    ///
    /// The second form said `while read early; …&` and **still ate nothing**, for a reason no
    /// reading of it shows: a background job of a NON-INTERACTIVE shell gets its stdin from
    /// `/dev/null`, so the stand-in was reading end-of-file while the injection sat in the pty
    /// exactly as before. Both halves passed for the same reason they had passed before the first
    /// rebuild. [`STANDIN_READS_TTY`] is what fixes it — reopening the controlling terminal is the
    /// only way a background reader here can be given the pane's own input.
    #[test]
    fn a_run_told_what_ready_looks_like_injects_nothing_before_it() {
        // A stand-in shell that consumes and NAMES anything typed at it, for two seconds — longer
        // than this run can take unaided (two turns, each floored by the 500ms observe). Then it
        // is killed, the peer announces itself and `exec`s, so what answers afterwards is
        // unambiguously the peer.
        let (access, pane) = sh_access(
            &format!(
                "while read early; do echo \"SHELL-ATE $early\"; done {STANDIN_READS_TTY} & \
                 sleep 2; kill $! 2>/dev/null; printf 'PEER-UP\\n'; \
                 exec sh -c 'while read l; do echo \"PEER-SAW $l\"; done'"
            ),
            40,
            8,
        );
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-SAW ping".to_string()),
                ready_when: Some(ReadyWhen::Prints("PEER-UP".to_string())),
                ready_within: None,
            },
        );
        // ⚠ WHICH OF THE TWO ASSERTIONS BELOW FIRES DEPENDS ON THE STAND-IN, and both were
        // measured against a run with the barrier removed. A stand-in that ANSWERS (this one names
        // what it ate) ends each observe at once, so a barrier-less run burns every turn in
        // milliseconds and never reaches the peer — the CONVERGED half fails, in 70ms. A stand-in
        // that merely swallows would floor each step instead, the run would outlive it, and the
        // SHELL-ATE half is what catches it. Keep both: they are the same defect seen from the two
        // ends, and neither covers the other.
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the run waited for the peer to come up and then drove it",
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("SHELL-ATE"),
            "NOTHING may have been injected while the pane was still the stand-in shell — every \
             `SHELL-ATE` is a turn the run spent on a peer that did not exist yet: {screen:?}",
        );
    }

    /// ⚠⚠ **A MARKER YOU TYPED IS NEVER EVIDENCE, AND THE ANSWER DOES NOT DEPEND ON TIMING.**
    ///
    /// A pane echoes the command line that started the program, so a marker appearing in that line
    /// is on screen before the program exists — and the echo is ordinary output once it reaches the
    /// grid. Under the whole-screen match the barrier cleared on it in 50 MILLISECONDS and the run
    /// spent every turn on the shell (`…exec cat'$ pingATE pingpingATE ping`).
    ///
    /// The generation baseline alone did not close it, it only moved the failure: the echo arrives
    /// ASYNCHRONOUSLY, so whether it counted as "produced after arming" depended on scheduling.
    /// **The same call converged or fed the shell depending on how the machine was loaded.**
    ///
    /// So the pane remembers what was written into it and such a marker is refused outright. The
    /// run ends `NeverReady` NAMING it — an ambiguous marker answered honestly and identically
    /// every time — instead of driving something that was never listening.
    ///
    /// ⚠ BOTH HALVES START THE RUN THE SAME WAY AND DIFFER ONLY IN THE WAIT, which is the point:
    /// the echo having landed or not must not change the answer.
    #[test]
    fn a_marker_that_is_in_what_the_caller_typed_is_never_evidence() {
        // The command line MENTIONS the banner, because the caller wrote both.
        let started = format!(
            "sh -c 'while read e; do echo \"ATE $e\"; done {STANDIN_READS_TTY} & sleep 2; \
             kill $! 2>/dev/null; printf \"TOOL-UP\\n\"; exec cat'"
        );
        let drive = |wait_for_the_echo: bool| {
            let (access, pane) = sh_access("exec sh", 80, 10);
            let mut typed = KeyStroke::text(&started);
            typed.push(KeyStroke::named("Enter"));
            let _typed = access.inject(pane, &typed).expect("start the tool");
            if wait_for_the_echo {
                let start = std::time::Instant::now();
                while start.elapsed() < Duration::from_secs(5)
                    && !access
                        .pane_collapsed(pane)
                        .is_some_and(|text| text.contains("TOOL-UP"))
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let mut orch = Orchestrator::new(
                pane,
                OrchestrationSpec {
                    stimulus: "ping".to_string(),
                    sentinel: None,
                    ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                    ready_within: Some(Duration::from_millis(400)),
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut orch, &access, &RunContext::uncancellable());
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            (outcome, screen)
        };

        for waited in [true, false] {
            let (outcome, screen) = drive(waited);
            // ⚠ `instead` is deliberately NOT asserted here: this fixture's pane is mid-`exec` at
            // the moment the barrier gives up, so the job that owns its terminal is the starting
            // shell or the program it became depending on the clock. Pinning it would make a
            // diagnostic field decide a gate about REFUSING AN ECHO, which is a different claim.
            assert!(
                matches!(
                    &outcome.failure,
                    Some(PaneError::NeverReady { wanted, .. })
                        if wanted == &ReadyWhen::Prints("TOOL-UP".to_string()),
                ),
                "an ambiguous marker is refused and NAMED, whether or not its echo had landed \
                 (waited: {waited}): {outcome:?} {screen:?}",
            );
            assert!(
                !screen.contains("ATE ping"),
                "and NOTHING was typed at the stand-in that was still there (waited: {waited}): \
                 {screen:?}",
            );
        }
    }

    /// ⚠⚠ **AND A MARKER THE PROGRAM COMPOSES CONVERGES WITH NO WAIT AT ALL** — the other half,
    /// and the one that says the remedy above is not simply "refuse everything".
    ///
    /// The banner here is assembled by the program (`printf "PEER-%s" UP`), so it cannot appear in
    /// the line the caller typed and the echo cannot be mistaken for it — by CONSTRUCTION, not by
    /// timing. The run starts in the same breath as the write, with no quiescing and no sleep, and
    /// still waits for the peer rather than for its own echo.
    #[test]
    fn a_marker_the_program_composes_needs_no_wait_before_the_run() {
        let (access, pane) = sh_access("exec sh", 80, 10);
        let started = format!(
            "sh -c 'while read e; do echo \"ATE $e\"; done {STANDIN_READS_TTY} & sleep 2; \
             kill $! 2>/dev/null; printf \"PEER-%s\\n\" UP; \
             exec sh -c \"while read l; do echo SAW \\$l; done\"'"
        );
        let mut typed = KeyStroke::text(&started);
        typed.push(KeyStroke::named("Enter"));
        let _typed = access.inject(pane, &typed).expect("start the tool");
        // ⚠ NO WAIT, deliberately — the echo of the line above is still in flight.

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("SAW ping".to_string()),
                ready_when: Some(ReadyWhen::Prints("PEER-UP".to_string())),
                ready_within: Some(Duration::from_secs(10)),
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(20)),
            },
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("ATE ping"),
            "no turn may be spent on the stand-in that was still there: {screen:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "and the peer is driven once it exists: {screen:?}",
        );
    }

    /// ⚠⚠ **AND `shows` IS NOT THE SAME BUG KEPT AROUND** — it is the answer to the other question,
    /// and it is the ONLY answer there.
    ///
    /// A program already running has already said everything it is going to say until it is fed.
    /// This pane prints its prompt and then goes quiet; a barrier demanding NEW output would wait
    /// for ever against it, which is why the whole-screen match had to stay reachable rather than
    /// be tightened away.
    ///
    /// The pause is what makes this measure anything: the banner is over and done with before the
    /// run looks, so a `Prints` barrier here would have nothing to find.
    #[test]
    fn a_program_already_at_its_prompt_is_ready_by_what_it_shows() {
        let (access, pane) = sh_access(
            "printf 'REPL-READY\n'; exec sh -c 'while read l; do echo \"GOT $l\"; done'",
            40,
            8,
        );
        // Wait for the banner to be OVER, so nothing new arrives after the barrier arms.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains("REPL-READY"))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("GOT ping".to_string()),
                ready_when: Some(ReadyWhen::Shows("REPL-READY".to_string())),
                ready_within: Some(Duration::from_millis(500)),
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(20)),
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "a pane whose program is already at its prompt is ready by what it SHOWS — demanding \
             new output would wait for a line this program will never print unasked: {outcome:?}",
        );
    }

    /// ⚠⚠ **A PROGRAM THAT PRINTS NOTHING IS WAITED FOR BY WHAT OWNS THE TERMINAL** — the case the
    /// two screen kinds cannot answer at all, and the reason [`ReadyWhen::Runs`] exists.
    ///
    /// The fixture is the ordinary AI-loop shape with one change that removes every marker: the
    /// program that finally comes up **says nothing when it starts**. `tr` is `cat` with a witness
    /// — silent until fed, then provably itself, because `PING` is not a spelling the pty's echo of
    /// `ping` can produce. Most things this drives are in that class (a REPL launched quiet, a
    /// relay, any tool that speaks only when spoken to), and for all of them `Prints` waits for a
    /// line that will never come and `Shows` has nothing to look for but the caller's own echo.
    ///
    /// **THREE HALVES, and the first is the control that makes the other two mean anything:**
    ///
    /// 1. the pane produces NOTHING between the stand-in dying and the program being ready — so the
    ///    set of markers a caller could have named is empty, and this is a gap in the QUESTION
    ///    rather than a marker chosen badly;
    /// 2. the stand-in was never fed — every `ATE` is a turn spent on a shell;
    /// 3. the run CONVERGED, so the wait ended and the driving worked.
    ///
    /// ⚠ MUTATION-MEASURED, and the order of the last two is what the measurement bought. With the
    /// `Runs` arm answering `true` unconditionally the run drives the stand-in, which is half 2;
    /// with it answering `false` the pane is never ready, nothing is ever injected and only half 3
    /// can see it. **Asserted screen-first for that reason** — the reverse order was tried and half
    /// 3 fired on BOTH mutations, hiding the more specific diagnosis behind a generic one.
    #[test]
    fn a_program_that_prints_nothing_is_ready_when_it_owns_the_terminal() {
        // The stand-in eats for two seconds — longer than this run takes unaided — then `exec`s a
        // program that prints NOT ONE BYTE until it is spoken to.
        let (access, pane) = sh_access(
            &format!(
                "while read early; do echo \"ATE $early\"; done {STANDIN_READS_TTY} & \
                 sleep 2; kill $! 2>/dev/null; exec tr a-z A-Z"
            ),
            40,
            8,
        );
        // ⚠ HALF 1, THE CONTROL — read BEFORE the run, while the stand-in is still there. A silent
        // program cannot be waited for by anything that reads the screen, and this is that claim
        // measured rather than asserted: the pane is blank now and stays blank until it is driven.
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "the fixture's program must print NOTHING on startup, or this gate is about a marker \
             the caller chose badly rather than about a program that has none",
        );
        let mut orch = Orchestrator::new(pane, drive_the_silent_program());
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("ATE"),
            "NOTHING may have been injected while the pane's terminal still belonged to the \
             stand-in shell — every `ATE` is a turn the run spent on a program that had not \
             started: {screen:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the run waited for the program to take the terminal and then drove it: {outcome:?}",
        );
    }

    /// ⚠⚠ **A PANE OPENED RUNNING THE PROGRAM IS READY BY THE SAME VALUE, WITH NO WINDOW AT ALL.**
    ///
    /// `open_pane` has taken a `cmd` since the daemon's argv path was fixed, and NOTHING PREFERRED
    /// IT: every loop gate in this workspace opened a shell and typed into it, which is how the
    /// echo hazard got three rounds of attention. This is the other shape, and the point is that it
    /// needs no new spelling — [`drive_the_silent_program`] is shared with the gate above verbatim,
    /// so the two cannot drift into two answers.
    ///
    /// **Opening the pane running the program is the shape to prefer**, and the reason is visible
    /// here rather than argued: there is no shell to be typed at, so there is no window in which an
    /// injection can be eaten, and no echo of a starting command line for a marker to be confused
    /// with. The barrier is not a wait — it is a confirmation that the pane is what the caller
    /// asked for. A `Prints` marker could not make that claim at all: the program says nothing, so
    /// on this shape it would wait out its bound and fail.
    ///
    /// ⚠ THE THIRD HALF IS THE ONE THAT MAKES THIS MORE THAN A CONVERGENCE TEST. A run that
    /// converged might still have waited seconds for a barrier it should have cleared at once, so
    /// the elapsed time is asserted too — well under the 500ms floor one observe step costs, which
    /// is the cheapest bound that could not be met by accident.
    #[test]
    fn a_pane_opened_running_the_program_is_ready_by_the_same_value() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let access = WorkspacePaneAccess::new(workspace);
        let argv: Vec<String> = SILENT_PROGRAM.iter().map(|a| (*a).to_string()).collect();
        let pane = access
            .lifecycle()
            .expect("this access spawns panes")
            .spawn(&argv, 40, 8)
            .expect("open a pane RUNNING the program, rather than a shell to type it into");

        let mut ready = crate::readiness::Readiness::new(
            drive_the_silent_program().ready_when,
            drive_the_silent_program().ready_within,
        );
        let started = std::time::Instant::now();
        let reached = ready
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a pane opened running the program is ready for it");
        assert_eq!(
            reached,
            crate::readiness::Reached::Yes,
            "the pane IS the program — the barrier confirms it rather than waiting for it",
        );

        let mut orch = Orchestrator::new(pane, drive_the_silent_program());
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "and driving it works, off the identical spec the shell-and-type gate uses: \
             {outcome:?}",
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a pane that is ALREADY the program must not be waited for — this shape has no \
             starting window, and paying one would mean the barrier is watching the wrong thing",
        );
    }

    /// ⚠⚠ **NO AMOUNT OF TYPING THE NAME MAKES A PANE READY** — the discriminator against both
    /// screen kinds, and the structural claim [`ReadyWhen::Runs`] is worth having for.
    ///
    /// `Shows` is satisfied by any text on the pane, and the pty puts the caller's own command line
    /// there before the program exists; `Prints` had to grow an echo trail, a damage baseline and a
    /// refusal rule to survive the same input, and still answers only for programs that speak.
    /// This kind is not a better predicate over the screen — it does not read the screen, so the
    /// hazard is not narrowed, it is ABSENT.
    ///
    /// The fixture types the name as LOUDLY as a pane can carry it: as a command that echoes it
    /// back, so it is both in what was typed AND in what the program printed, freshly, after the
    /// barrier armed. `Shows` would clear on it and so would `Prints`.
    ///
    /// ⚠ The pane runs `cat`, so the barrier's answer is *"a job named `tr` never owned this
    /// terminal"* — and the failure NAMES what did, which is the correction a caller who guessed
    /// the wrong program name needs.
    #[test]
    fn typing_a_program_name_at_a_pane_never_makes_it_ready() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        // `cat` echoes: after this the word is in the echo trail AND on the screen as fresh output.
        let mut typed = KeyStroke::text("tr");
        typed.push(KeyStroke::named("Enter"));
        let _typed = access.inject(pane, &typed).expect("type the name");
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .matches("tr")
                .count()
                < 2
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .matches("tr")
                .count()
                >= 2,
            "the fixture must get the name onto the screen TWICE — the pty's echo and the \
             program's copy — or it has not put the screen kinds in a position to be fooled",
        );

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Runs("tr".to_string())),
                ready_within: Some(Duration::from_millis(300)),
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // Far longer than the readiness bound, so the RUN's clock provably cannot end this.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane running `cat` is not ready for `tr`, however many times the word is on its \
             screen: {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Runs("tr".to_string()),
            "cat",
            "and the failure NAMES what owned the terminal instead, which is the whole correction \
             for a caller who guessed the program's name wrong",
        );
    }

    /// ⚠⚠ **A READINESS THAT NEVER COMES STOPS THE RUN AND SAYS WHAT IT WAITED FOR** — the other
    /// half, and the one that decides whether the argument is a bound or a hope.
    ///
    /// Driving on would inject into whatever IS there and report turns against a peer that was
    /// never listening. The run fails instead, naming the text, in a sentence rather than a Rust
    /// variant.
    #[test]
    fn a_readiness_that_never_comes_ends_the_run_naming_what_it_waited_for() {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Prints("NEVER-PRINTED".to_string())),
                ready_within: None,
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: Some(Duration::from_millis(300)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "the run's own clock is what bounds waiting to be ready — not a number the plugin \
             invented, and not the turn ceiling",
        );
        assert_eq!(
            outcome.iterations, 1,
            "and it spent ONE step doing it rather than burning the turn budget against a pane \
             that was never ready",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "not one byte was injected into a pane that never became ready",
        );
    }

    /// ⚠⚠ **A PANE THAT NEVER COMES UP FAILS THE RUN AND NAMES WHAT IT WAITED FOR** — the arm that
    /// had no test at all, and could not have had one until the wait's bound became the caller's.
    ///
    /// The gate above ends by the RUN's clock, which is a different finding: *the run was out of
    /// time* says nothing about the pane. This one is *this pane never came up*, and it is the
    /// answer a caller needs, because it is the one that names the marker they got wrong.
    ///
    /// ⚠ It was unreachable rather than untested. `ready_within` was hard-wired to two minutes, so
    /// any gate short enough to run had a run deadline shorter than the readiness bound, and
    /// [`Waited::Stopped`] won every time — `NeverReady` was constructed in one place and read by
    /// nothing. A bound the CALLER names is what makes the arm reachable in 200ms, and it is also
    /// the right product answer: how long a program takes to start is the caller's knowledge.
    ///
    /// The three halves are three different claims: the run FAILED (not exhausted), it carries the
    /// TYPED cause naming the marker, and NOTHING was injected into the pane that never came up.
    #[test]
    fn a_pane_that_never_becomes_ready_fails_the_run_and_names_the_marker() {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Prints("NEVER-PRINTED".to_string())),
                ready_within: Some(Duration::from_millis(200)),
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // ⚠ FAR LONGER than the readiness bound, so the run's own clock provably cannot be
            // what ends this — that is the other gate, and it reaches a different arm.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane that never becomes ready is a FAILURE of the run, not a ceiling it reached: \
             {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Prints("NEVER-PRINTED".to_string()),
            // ⚠ The pane runs `exec cat`, so `cat` IS the job that owns its terminal — a caller
            // reading this learns the pane was never going to print, which is the correction, and
            // it arrives without them reading the screen.
            "cat",
            "and the cause is typed, carries the QUESTION the caller asked, and names what the \
             pane was running instead",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "not one byte was injected into a pane that never became ready",
        );
    }

    /// ⚠⚠ **THE FAILURE AN AGENT READS IS A SENTENCE**, and not one of these five had a gate.
    ///
    /// A run's `failure` is published to its caller as this text
    /// ([`plugins.rs`](../../sprag_host/plugins/index.html) does `.map(ToString::to_string)`), and
    /// it was `format!("{e:?}")` until R358 — `Write("Broken pipe (os error 32)")`, a Rust variant
    /// name and its debug payload, reaching the one reader who cannot look up what a variant means.
    ///
    /// The fix had no test, so a reverted `to_string()` would have broken nothing and the leak
    /// would have come back unnoticed. Derived from a list of every variant rather than spot-
    /// checked, so a SIXTH variant added with a debug-shaped sentence fails here.
    #[test]
    fn every_pane_failure_reads_as_a_sentence_rather_than_a_rust_variant() {
        let every = [
            PaneError::UnknownPane(PaneId(7)),
            PaneError::Encode("F13".to_string()),
            PaneError::Write("Broken pipe (os error 32)".to_string()),
            PaneError::Spawn("No such file or directory".to_string()),
            PaneError::NeverReady {
                wanted: ReadyWhen::Prints("PEER-UP".to_string()),
                instead: PaneDoing::Job(JobLeader::known_as("sh".to_string())),
            },
        ];
        for error in &every {
            let said = error.to_string();
            let debug = format!("{error:?}");
            assert_ne!(
                said, debug,
                "the published text is the DEBUG form, which is the leak itself",
            );
            // A variant name is `CamelCase` with no space; a sentence has spaces and starts lower.
            assert!(
                said.contains(' ') && said.starts_with(char::is_lowercase),
                "a failure an agent reads must be prose, not {said:?}",
            );
            assert!(
                !said.contains('(') || !said.contains("::"),
                "and must not carry a Rust path: {said:?}",
            );
        }
        // The PAYLOAD has to survive into the sentence, or the prose is prose about nothing — this
        // is the half that a "polite" catch-all message would silently fail.
        assert!(
            PaneError::Write("Broken pipe (os error 32)".to_string())
                .to_string()
                .contains("Broken pipe (os error 32)"),
            "the cause the operating system gave must reach the reader",
        );
        let never_ready = PaneError::NeverReady {
            wanted: ReadyWhen::Runs("claude".to_string()),
            instead: PaneDoing::Job(JobLeader::known_as("sh".to_string())),
        }
        .to_string();
        assert!(
            never_ready.contains("claude"),
            "a readiness that never came must name what it waited for, or the caller cannot tell \
             which marker they got wrong: {never_ready:?}",
        );
        // ⚠⚠ AND WHAT THE PANE WAS DOING INSTEAD, which is the half that turns a two-minute
        // mystery into a correction. A caller who waited for `claude` against a pane still sitting
        // at a shell learns BOTH facts from one sentence.
        assert!(
            never_ready.contains("sh"),
            "and what owned the terminal instead: {never_ready:?}",
        );
        assert!(
            PaneError::UnknownPane(PaneId(7)).to_string().contains('7'),
            "and an unknown pane must name the id that was asked for",
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
                ready_when: None,
                ready_within: None,
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
        // ⚠ The readiness barrier is the PRODUCT's now (`ready_when`, below) rather than a helper
        // this test kept to itself — so the gate drives the same wait a caller gets.
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("A SENTINEL THIS PANE NEVER PRINTS".to_string()),
                ready_when: Some(ReadyWhen::Prints("DEAF-READY".to_string())),
                ready_within: None,
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
