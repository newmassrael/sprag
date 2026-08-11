//! The `Agent` adapter plugin — structured request/response over a pane running
//! a real AI CLI (adapter #1, the north star's first realization).
//!
//! This is the first plugin whose pane peer is a *real* AI tool (e.g.
//! `claude -p`) rather than a `cat`/`printf` fixture: it injects a prompt,
//! waits for the tool to finish replying, and captures the reply as structured
//! output an external peer reads back as scene-as-data. It is one-shot — one
//! prompt, one captured response, then converge; multi-turn conversation and
//! the bidirectional AI↔AI relay layer on top of this later.
//!
//! Completion is detected by the pane child *exiting*
//! ([`PaneAccess::pane_eof`]), not by output quiescence. A one-shot tool
//! (`claude -p`, or a read-until-EOF shell peer) prints its reply and exits,
//! and the producer guarantees every byte is applied to the screen once EOF is
//! observed — so the captured screen is complete and the read never tears. This
//! sidesteps the echo/think-time race a debounce would hit: the injected prompt
//! echoes back (cooked-mode tty) *before* the model has even started replying,
//! so "settle after the first change" would converge on the echo. A `timeout`
//! bounds the wait so a tool that never exits cannot hang the run.
//!
//! Known limitations (deferred, in the spirit of [`crate::pipe`]'s): the
//! captured text is the pane delta since the prompt, so it includes the
//! prompt's own cooked-mode echo; and a reply that scrolls past the screen
//! loses the scrolled-off rows (the projection has no scrollback yet).

use std::time::Duration;

use sprag_input::Modifiers;
use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::run::{DEFAULT_REPLY_TIMEOUT, RunContext, Waited, poll_until};

/// What the agent asks and how long it waits for the answer.
#[derive(Clone, Debug)]
pub struct AgentSpec {
    /// The prompt injected into the pane (followed by Enter).
    pub prompt: String,
    /// Send Ctrl-D (EOF) after the prompt, so a tool that reads stdin until
    /// end-of-input (`claude -p`, `cat`) sees EOF and replies. Default `true`;
    /// set `false` for a peer that reads line-by-line and stays alive.
    pub eof: bool,
    /// Overall bound on the reply wait. On timeout the agent converges with
    /// whatever it captured (possibly nothing) rather than hanging.
    pub timeout: Duration,
}

impl AgentSpec {
    /// A spec with the default one-shot behaviour (send EOF, generous timeout).
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            eof: true,
            timeout: DEFAULT_REPLY_TIMEOUT,
        }
    }
}

/// A one-shot AI-tool adapter over one pane.
pub struct Agent {
    pane: PaneId,
    spec: AgentSpec,
    /// The reply captured this run, surfaced through [`Plugin::captured`].
    response: Option<String>,
}

impl Agent {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: AgentSpec) -> Self {
        Self {
            pane,
            spec,
            response: None,
        }
    }

    /// The keystrokes that submit the prompt: its characters, Enter, then
    /// optionally Ctrl-D (EOF) for a read-until-EOF peer.
    fn prompt_keys(&self) -> Vec<KeyStroke> {
        let mut keys = KeyStroke::text(&self.spec.prompt);
        keys.push(KeyStroke::named("Enter"));
        if self.spec.eof {
            keys.push(KeyStroke {
                key: "d".to_string(),
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            });
        }
        keys
    }

    /// Wait (bounded by `timeout`, cancellable) for the pane child to exit —
    /// once it has, its full reply is on screen ([`PaneAccess::pane_eof`]'s
    /// contract). An unknown pane (`None`) counts as done.
    fn await_reply(&self, panes: &dyn PaneAccess, run: &RunContext) -> Waited {
        poll_until(run, self.spec.timeout, || {
            panes.pane_eof(self.pane).unwrap_or(true)
        })
    }

    /// Capture the rows damaged since `baseline` — the reply region — joined as
    /// the response text.
    fn capture(&self, panes: &dyn PaneAccess, baseline: &[u64]) -> String {
        let rows = panes.pane_rows(self.pane).unwrap_or_default();
        rows.iter()
            .enumerate()
            .filter(|(i, row)| row.generation > baseline.get(*i).copied().unwrap_or(0))
            .map(|(_, row)| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Plugin for Agent {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // Baseline the damage generations before acting, so `capture` isolates
        // this prompt's reply (and its cooked-mode echo) from prior content.
        let baseline: Vec<u64> = panes
            .pane_rows(self.pane)
            .map(|rows| rows.iter().map(|row| row.generation).collect())
            .unwrap_or_default();

        let cost = panes.inject(self.pane, &self.prompt_keys())?.bytes();

        // If the RUN ended mid-wait — cancelled, or out of time — don't converge
        // or record a partial reply. Return Continue so the Driver's loop top
        // decides the terminal state, which is the only place that knows whether
        // it was a cancel or the duration ceiling.
        if self.await_reply(panes, run) == Waited::Stopped {
            return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                .noting("the run ended while waiting for the reply; nothing captured"));
        }
        let reply = self.capture(panes, &baseline);
        // ⚠ THE LENGTH IS THE DIAGNOSTIC. A peer that never answered and one that answered are the
        // same `converged` with the same cost, and an EMPTY capture is what a prompt the peer
        // swallowed looks like from out here.
        let note = format!("captured a {}-character reply", reply.chars().count());
        self.response = Some(reply);

        // One-shot: one prompt, one captured reply, then converge. The Driver's
        // guardrails still bound it; `timeout` (above) bounds a non-exiting peer.
        Ok(Step::new(Cost::Bytes(cost), Verdict::Converged).noting(note))
    }

    fn captured(&self) -> Option<String> {
        self.response.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, Outcome, OutcomeState};
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

    fn run(access: &WorkspacePaneAccess, agent: &mut Agent) -> Outcome {
        Driver::new(Guardrails {
            max_iterations: 4,
            max_cost: None,
            max_duration: None,
        })
        .run(agent, access, &RunContext::uncancellable())
    }

    #[test]
    fn converges_and_captures_a_reply() {
        // A one-shot fake AI: read the prompt (until EOF), reply deterministically.
        let (access, pane) = sh_access("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let mut agent = Agent::new(pane, AgentSpec::new("ping"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().expect("a captured reply");
        assert!(captured.contains("REPLY[ping]"), "captured: {captured:?}");
    }

    #[test]
    fn captures_a_complete_multiline_reply() {
        // Two reply lines. Converging on child-exit (not first damage) captures
        // BOTH — a first-damage observe would stop at the prompt echo and miss
        // the reply. Pane is tall enough that the reply does not scroll.
        let (access, pane) = sh_access(
            "in=$(cat); printf 'one:%s\\ntwo:%s\\n' \"$in\" \"$in\"",
            40,
            8,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("x"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().expect("a captured reply");
        assert!(captured.contains("one:x"), "captured: {captured:?}");
        assert!(captured.contains("two:x"), "captured: {captured:?}");
    }

    /// ⚠⚠ **THE RUN'S DEADLINE REACHES INSIDE A STEP**, which is the only thing that makes it a
    /// bound at all.
    ///
    /// The peer never exits, so `await_reply` waits out `spec.timeout` in full. Both halves drive
    /// that same peer and differ only in whether the run is timed:
    ///
    /// * The CONTROL has no deadline. Its step runs its own four-second timeout to the end, the
    ///   agent captures whatever is on screen and converges — which is the behaviour a per-turn
    ///   timeout is FOR, and it is unchanged.
    /// * The SUBJECT is given three hundred milliseconds. It must come back an order of magnitude
    ///   sooner, and it must come back `Exhausted(Duration)`.
    ///
    /// ⚠ A deadline enforced only at the Driver's loop top would make both halves take four
    /// seconds and both assertions about ELAPSED time fail — which is why the timing is asserted
    /// and not just the outcome. The two are the same claim read two ways: the run stopped because
    /// of the clock, and it stopped WHEN the clock said.
    #[test]
    fn a_runs_deadline_cuts_a_step_that_is_still_inside_its_own_timeout() {
        // `exec cat` holds its pty open forever, and `eof: false` means no Ctrl-D is sent to end
        // it — so nothing but a bound can end this wait.
        let timed = |deadline: Option<Duration>| {
            let (access, pane) = sh_access("exec cat", 40, 6);
            let mut spec = AgentSpec::new("ping");
            spec.eof = false;
            spec.timeout = Duration::from_secs(4);
            let mut agent = Agent::new(pane, spec);
            let start = std::time::Instant::now();
            let outcome = Driver::new(Guardrails {
                max_iterations: 100,
                max_cost: None,
                max_duration: deadline,
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            (outcome, start.elapsed())
        };

        let (control, control_took) = timed(None);
        assert_eq!(
            control.state,
            OutcomeState::Converged,
            "an untimed run rides its own per-turn timeout out and converges on what it saw",
        );
        assert!(
            control_took >= Duration::from_secs(3),
            "the control must actually have waited its step out, or the subject below is being \
             compared against nothing; it took {control_took:?}",
        );

        let (subject, subject_took) = timed(Some(Duration::from_millis(300)));
        assert_eq!(
            subject.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "a run out of time is exhausted by the DURATION ceiling, and says so",
        );
        assert!(
            subject_took < Duration::from_secs(2),
            "the deadline must end the wait that is in flight, not merely stop the next step \
             being taken — this run took {subject_took:?} against a step timeout of 4s",
        );
    }

    /// The genuine AI↔AI proof: drive a real `claude -p` pane and capture its
    /// answer. Ignored by default — it needs the `claude` CLI, network, and
    /// auth, and is non-deterministic. Run manually:
    /// `cargo test -p sprag-plugin drives_real_claude -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the claude CLI + network + auth; run manually with --ignored"]
    fn drives_real_claude() {
        let mut command = CommandBuilder::new("claude");
        command.arg("-p");
        command.env("TERM", "dumb");
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = workspace
            .lock()
            .unwrap()
            .spawn(command, "claude".to_string(), 80, 24)
            .expect("spawn claude pane");
        let access = WorkspacePaneAccess::new(workspace);

        let mut agent = Agent::new(
            pane,
            AgentSpec::new("Reply with exactly the single word: PONG"),
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().unwrap_or_default();
        assert!(captured.contains("PONG"), "captured: {captured:?}");
    }
}
