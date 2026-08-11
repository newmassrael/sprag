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
    /// Per-row damage generations captured before the last stimulus, so the
    /// observe-wait keys on *this* step's echo.
    baseline_generations: Vec<u64>,
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

    /// ⚠⚠ **THE ECHO OF THE COMMAND THAT STARTED THE PROGRAM IS NOT THE PROGRAM COMING UP.**
    ///
    /// A pane echoes what is typed at it, so the command line a caller used to START the tool is on
    /// screen before the tool exists — and a marker matched against the whole screen finds it
    /// there. Measured against this fixture with the old whole-screen match: the barrier cleared in
    /// 50 MILLISECONDS and the run spent both turns on the shell, screen
    /// `…printf "TOOL-UP\n"; exec cat'$ pingATE pingpingATE ping` — the stand-in ate both and the
    /// peer never saw a word.
    ///
    /// [`ReadyWhen::Prints`] is what refuses it: the barrier baselines the pane's damage
    /// generations on its first look and only reads rows that moved past it, so text that was
    /// already there is not evidence. Both halves, because a barrier that never let go would
    /// satisfy the first: nothing may be eaten, AND the peer must receive the stimulus afterwards.
    #[test]
    fn the_echo_of_the_command_that_started_the_program_is_not_the_program_coming_up() {
        let (access, pane) = sh_access("exec sh", 80, 10);
        // The caller starts the tool by TYPING it — the ordinary shape, and the shape R358 measured
        // eight gates driving. Its command line MENTIONS the banner, because the caller wrote both.
        let started = format!(
            "sh -c 'while read e; do echo \"ATE $e\"; done {STANDIN_READS_TTY} & sleep 2; \
             kill $! 2>/dev/null; printf \"TOOL-UP\\n\"; \
             exec sh -c \"while read l; do echo PEER-SAW \\$l; done\"'"
        );
        let mut typed = KeyStroke::text(&started);
        typed.push(KeyStroke::named("Enter"));
        let _typed = access.inject(pane, &typed).expect("start the tool");
        // ⚠ WAIT FOR THE ECHO TO LAND, which is what makes this the case under test rather than a
        // race. A pty echo is asynchronous: the caller's own command line reaches the grid some
        // moments after the write, and a barrier that armed in between would see it as output
        // produced AFTER arming — see the note on the residue in [`ReadyWhen::Prints`].
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("TOOL-UP"))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("TOOL-UP")),
            "the fixture needs the caller's command line — marker and all — ON SCREEN before the \
             run starts, or this gate is not measuring the echo at all",
        );

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-SAW ping".to_string()),
                ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
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
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the run waited for the tool to PRINT its banner and then drove it: {screen:?}",
        );
        assert!(
            !screen.contains("ATE ping"),
            "the barrier passed on the ECHO of the command line rather than on the program: \
             {screen:?}",
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
        assert_eq!(
            outcome.failure,
            Some(PaneError::NeverReady("NEVER-PRINTED".to_string())),
            "and the cause is typed and carries the marker the caller named",
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
            PaneError::NeverReady("PEER-UP".to_string()),
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
        assert!(
            PaneError::NeverReady("PEER-UP".to_string())
                .to_string()
                .contains("PEER-UP"),
            "a readiness that never came must name what it waited for, or the caller cannot tell \
             which marker they got wrong",
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
