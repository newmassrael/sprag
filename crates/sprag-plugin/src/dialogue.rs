//! The `Dialogue` plugin — a stateful turn-based conversation between two AI
//! tools (adapter #2, the north star's full "AI↔AI").
//!
//! Round 12's [`Agent`] made *one* side real (one prompt, one reply). This
//! drives *two* one-shot AI endpoints in alternation, and — unlike its earlier
//! telephone-game form — gives each side the **whole accumulated transcript**
//! every turn, so the two AIs actually converse coherently rather than each
//! replying to a single decontextualized line.
//!
//! Each turn renders a prompt (a short instruction naming the current speaker,
//! then the seed, then the labelled transcript so far), spawns the active
//! endpoint as a fresh pane with that prompt as its argv (`claude -p
//! "<prompt>"` — argv, not stdin, so there is no cooked-mode echo to strip),
//! waits for it to reply and exit, captures the reply, closes the pane, and
//! appends `"<label>: <reply>"` to the history. Instruction framing (verified:
//! `claude -p` returns a bare next message, not a `"A: 3"`-style line) lets the
//! plugin own the labelling.
//!
//! Each turn is a fresh process, so this needs pane *lifecycle*
//! ([`PaneLifecycle`]); a [`PaneGuard`] closes the per-turn pane on every exit
//! path (no leaked PTY/child even if a step fails mid-way). It never
//! self-converges — the [`Driver`]'s `max_iterations` is the turn budget, so a
//! run ends `Exhausted` with the transcript as its payload (the [`Pipe`]
//! pattern).
//!
//! Known limitations (deferred): whole-history prompting resends the growing
//! transcript every turn (O(n²) tokens over a run) and passes it as one argv
//! element, so a very long conversation eventually hits the OS arg-size cap and
//! fails loudly (`PaneError::Spawn`) rather than corrupting — `claude -p
//! --resume` (server-side session state) is the future optimization. Each
//! captured reply is kept verbatim (trimmed only of surrounding blanks) and
//! stored as a structured turn, so the conversation payload — code, lists,
//! multi-line answers — survives; turns stay delimited by blank-line blocks
//! (`render_turn`). Capture reads the pane's full output (scrollback + visible,
//! R16), so a reply longer than the pane is captured whole — unlike [`Agent`],
//! which still reads the visible screen only (it injects into a non-fresh pane,
//! so its reply region is the damage delta, where scrollback is ambiguous).
//!
//! [`Agent`]: crate::agent::Agent
//! [`Driver`]: crate::driver::Driver
//! [`Pipe`]: crate::pipe::Pipe

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError, PaneLifecycle};
use crate::plugin::{Plugin, Step, Verdict};
use crate::run::{poll_until, RunContext, Waited, DEFAULT_REPLY_TIMEOUT};

/// Who talks to whom, with what labels, and how each turn is bounded.
#[derive(Clone, Debug)]
pub struct DialogueSpec {
    /// Endpoint A's argv template (e.g. `["claude", "-p"]`); the turn's prompt
    /// is appended as the final argument.
    pub endpoint_a: Vec<String>,
    /// Endpoint B's argv template.
    pub endpoint_b: Vec<String>,
    /// The opening message / topic, given to both sides as context.
    pub seed: String,
    /// Transcript label for each side (defaults `"A"`/`"B"`).
    pub label_a: String,
    pub label_b: String,
    /// Size of each per-turn pane.
    pub cols: u16,
    pub rows: u16,
    /// Overall bound on one turn's reply wait; on timeout the turn captures
    /// whatever is on screen rather than hanging.
    pub timeout: Duration,
}

impl DialogueSpec {
    /// A spec with default labels (`"A"`/`"B"`), pane size (80x24), and timeout;
    /// set the fields after for overrides.
    #[must_use]
    pub fn new(endpoint_a: Vec<String>, endpoint_b: Vec<String>, seed: impl Into<String>) -> Self {
        Self {
            endpoint_a,
            endpoint_b,
            seed: seed.into(),
            label_a: "A".to_string(),
            label_b: "B".to_string(),
            cols: 80,
            rows: 24,
            timeout: DEFAULT_REPLY_TIMEOUT,
        }
    }
}

/// One completed turn: who spoke and their verbatim reply (may be multi-line).
#[derive(Clone, Debug)]
struct Turn {
    label: String,
    text: String,
}

/// Render a turn as a transcript block (`"<label>: <text>"`). A multi-line
/// `text` stays grouped under its label; blocks are blank-line-delimited where
/// joined, so turn boundaries are clear without flattening the reply.
fn render_turn(turn: &Turn) -> String {
    format!("{}: {}", turn.label, turn.text)
}

/// A stateful turn-based dialogue between two one-shot AI endpoints.
pub struct Dialogue {
    spec: DialogueSpec,
    /// The running conversation, in order — replayed into each turn's prompt and
    /// surfaced as output. Replies are kept verbatim (lossless).
    history: Vec<Turn>,
    /// Turn counter; even turns speak as A, odd as B.
    turn: usize,
}

impl Dialogue {
    /// Start a dialogue from `spec` (turn 0 prompts endpoint A with the seed).
    #[must_use]
    pub fn new(spec: DialogueSpec) -> Self {
        Self {
            spec,
            history: Vec::new(),
            turn: 0,
        }
    }
}

impl Plugin for Dialogue {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        let life = panes
            .lifecycle()
            .ok_or_else(|| PaneError::Spawn("pane access has no lifecycle".to_string()))?;

        // This turn's speaker (endpoint + label).
        let (endpoint, label) = if self.turn.is_multiple_of(2) {
            (&self.spec.endpoint_a, self.spec.label_a.clone())
        } else {
            (&self.spec.endpoint_b, self.spec.label_b.clone())
        };

        // The prompt is the whole conversation so far (instruction + seed +
        // labelled history), appended as the final argv element — preserved
        // verbatim, newlines and all, because it is one argv element.
        let prompt = render_prompt(&self.spec.seed, &self.history, &label);
        // Cost is the prompt bytes the peer ingests (argv, not injected) — so
        // the Driver's cost guardrail binds this plugin too, not just 0.
        let cost = prompt.len() as u64;
        let argv: Vec<String> = endpoint
            .iter()
            .cloned()
            .chain(std::iter::once(prompt))
            .collect();

        let id = life.spawn(&argv, self.spec.cols, self.spec.rows)?;
        // From here every exit path must close the pane — the guard does it on
        // Ok, on a later `?`, and on a panic unwind. No leaked PTY or child.
        let guard = PaneGuard { life, id };

        let waited = poll_until(run, self.spec.timeout, || panes.pane_eof(id).unwrap_or(true));
        let reply = capture(panes, id);
        drop(guard); // close the pane now (its blocking teardown is lock-free).

        // If cancelled mid-turn, record nothing (no junk partial turn) and
        // return Continue; the Driver's loop-top ends the run Cancelled. The
        // guard above already closed the spawned pane.
        if waited == Waited::Cancelled {
            return Ok(Step {
                cost,
                verdict: Verdict::Continue,
            });
        }

        // Keep the reply verbatim (trimmed only of surrounding blank lines) so
        // the conversation payload — code blocks, lists, multi-line answers — is
        // preserved; turns stay delimited by render_turn's blank-line blocks.
        let text = reply.trim().to_string();
        self.history.push(Turn { label, text });
        self.turn += 1;

        // Never self-converges; the Driver's iteration/cost budget is the cap.
        Ok(Step {
            cost,
            verdict: Verdict::Continue,
        })
    }

    fn captured(&self) -> Option<String> {
        if self.history.is_empty() {
            None
        } else {
            Some(
                self.history
                    .iter()
                    .map(render_turn)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        }
    }
}

/// Render one turn's prompt: a one-line instruction naming `speaker`, the seed,
/// then the labelled transcript so far — blocks joined by a blank line so a
/// multi-line reply stays grouped under its speaker and turns stay delimited.
fn render_prompt(seed: &str, history: &[Turn], speaker: &str) -> String {
    let mut blocks = Vec::with_capacity(history.len() + 2);
    blocks.push(format!(
        "You are {speaker} in this two-party dialogue. Reply with only {speaker}'s next message."
    ));
    blocks.push(seed.to_string());
    blocks.extend(history.iter().map(render_turn));
    blocks.join("\n\n")
}

/// Closes a per-turn pane on drop — so a turn never leaks a PTY or child,
/// whatever exit path `step` takes.
struct PaneGuard<'a> {
    life: &'a dyn PaneLifecycle,
    id: PaneId,
}

impl Drop for PaneGuard<'_> {
    fn drop(&mut self) {
        self.life.close(self.id);
    }
}

/// The reply on a fresh per-turn pane: its full output text (scrollback +
/// visible), so a reply longer than the pane is captured whole. Nothing was
/// injected, so the screen holds only the endpoint's output.
fn capture(panes: &dyn PaneAccess, id: PaneId) -> String {
    panes.pane_full_text(id).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{KeyStroke, PaneRow, WorkspacePaneAccess};
    use crate::driver::{Driver, Guardrails, Outcome, OutcomeState};
    use std::sync::{Arc, Mutex};
    use sprag_terminal::Workspace;

    /// A one-shot fake endpoint that replies with the newline-count of its
    /// prompt (`$1`): as the transcript accumulates, the prompt gains a line per
    /// turn, so the count strictly increases — a deterministic proof that the
    /// whole history is passed each turn. `tr -d ' '` strips wc's padding; the
    /// `_` is the `$0` placeholder `sh -c` consumes before `$1`.
    fn count_fake() -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "n=$(printf '%s' \"$1\" | wc -l | tr -d ' '); printf 'saw%s\\n' \"$n\"".to_string(),
            "_".to_string(),
        ]
    }

    fn run(spec: DialogueSpec, max_turns: u32) -> (Arc<Mutex<Workspace>>, Outcome, Option<String>) {
        let workspace = Arc::new(Mutex::new(Workspace::new((spec.cols, spec.rows))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut dialogue = Dialogue::new(spec);
        let outcome = Driver::new(Guardrails {
            max_iterations: max_turns,
            max_cost: u64::MAX,
        })
        .run(&mut dialogue, &access, &RunContext::uncancellable());
        let transcript = dialogue.captured();
        (workspace, outcome, transcript)
    }

    #[test]
    fn accumulates_full_history_each_turn() {
        // Each turn's prompt is one line longer than the last (the accumulating
        // history), so the counts strictly increase — and the labels alternate.
        let spec = DialogueSpec::new(count_fake(), count_fake(), "count upward");
        let (_ws, outcome, transcript) = run(spec, 3);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert_eq!(outcome.iterations, 3);
        let t = transcript.expect("a transcript");
        // Labels alternate, and the reported line-counts strictly increase
        // (each turn's prompt carries one more transcript block) — proof the
        // whole history is passed each turn. The assertion is format-robust: it
        // checks the trend, not exact counts.
        assert!(t.contains("A: saw") && t.contains("B: saw"), "labels alternate: {t:?}");
        let counts: Vec<u32> = t
            .match_indices("saw")
            .map(|(i, _)| {
                t[i + 3..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .expect("a saw count")
            })
            .collect();
        assert_eq!(counts.len(), 3, "three turns: {counts:?}");
        assert!(
            counts.windows(2).all(|w| w[0] < w[1]),
            "history must accumulate (strictly increasing): {counts:?}"
        );
    }

    #[test]
    fn long_reply_captured_in_full_including_scrolled_off_rows() {
        // An endpoint that emits 30 lines onto a 4-row pane: 26 scroll off.
        // Full-output capture keeps them, so the transcript has the early lines
        // a visible-only read would have lost. (Endpoint B is a quick no-op.)
        let mut spec = DialogueSpec::new(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "seq 1 30".to_string(),
            ],
            vec!["true".to_string()],
            "go",
        );
        spec.cols = 20;
        spec.rows = 4;
        let (_ws, outcome, transcript) = run(spec, 1);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let t = transcript.expect("a transcript");
        // The reply is kept verbatim (newline-separated), so the scrolled-off
        // line 5 appears as its own line; "\n5\n" can't match "15"/"25"/"50".
        assert!(t.contains("\n5\n"), "scrolled-off line 5 missing: {t:?}");
        assert!(t.contains("\n30"), "last line missing: {t:?}");
    }

    #[test]
    fn leaves_no_leftover_panes() {
        // Every turn spawns a pane; the guard must close each one.
        let spec = DialogueSpec::new(count_fake(), count_fake(), "x");
        let (workspace, _outcome, _transcript) = run(spec, 4);
        assert!(
            workspace.lock().unwrap().panes().is_empty(),
            "dialogue leaked a pane"
        );
    }

    #[test]
    fn closes_the_pane_when_a_turn_times_out() {
        // A non-exiting endpoint (sleep) never reaches EOF; the turn bails at
        // its (tiny) timeout and the guard still closes the pane — no leak,
        // no hang.
        let mut spec = DialogueSpec::new(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ],
            vec!["true".to_string()],
            "x",
        );
        spec.cols = 20;
        spec.rows = 4;
        spec.timeout = Duration::from_millis(60);
        let (workspace, outcome, _transcript) = run(spec, 1);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert!(
            workspace.lock().unwrap().panes().is_empty(),
            "a timed-out turn leaked its pane"
        );
    }

    #[test]
    fn cancel_mid_turn_ends_cancelled_with_no_leak_or_junk() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Instant;

        // A non-exiting endpoint: the turn blocks in poll_until until cancelled.
        let spec = DialogueSpec::new(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ],
            vec!["true".to_string()],
            "x",
        );
        let workspace = Arc::new(Mutex::new(Workspace::new((spec.cols, spec.rows))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let cancel = Arc::new(AtomicBool::new(false));
        let run_ctx = RunContext::new(Arc::clone(&cancel));

        let worker = thread::spawn(move || {
            let mut dialogue = Dialogue::new(spec);
            let outcome = Driver::new(Guardrails {
                max_iterations: 100,
                max_cost: u64::MAX,
            })
            .run(&mut dialogue, &access, &run_ctx);
            (outcome, dialogue.captured())
        });

        // Let the first turn spawn its pane and enter the wait, then cancel.
        thread::sleep(Duration::from_millis(80));
        cancel.store(true, Ordering::Release);

        let start = Instant::now();
        let (outcome, transcript) = worker.join().expect("worker");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "cancel did not abort the in-flight turn promptly: {:?}",
            start.elapsed()
        );
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        // No junk partial turn recorded, and the per-turn pane was reaped.
        assert!(transcript.is_none(), "recorded a junk turn: {transcript:?}");
        assert!(
            workspace.lock().unwrap().panes().is_empty(),
            "cancel leaked the per-turn pane"
        );
    }

    #[test]
    fn no_lifecycle_fails_cleanly() {
        // Without pane lifecycle, the first turn fails (mapped to Failed) rather
        // than panicking — deterministic, no PTY.
        struct NoPanes;
        impl PaneAccess for NoPanes {
            fn pane_ids(&self) -> Vec<PaneId> {
                Vec::new()
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                None
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
                None
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                None
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                None
            }
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<u64, PaneError> {
                Err(PaneError::UnknownPane(PaneId(0)))
            }
        }

        let mut dialogue = Dialogue::new(DialogueSpec::new(
            vec!["claude".to_string()],
            vec!["claude".to_string()],
            "hi",
        ));
        let outcome = Driver::new(Guardrails {
            max_iterations: 3,
            max_cost: u64::MAX,
        })
        .run(&mut dialogue, &NoPanes, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Failed);
        assert!(matches!(outcome.failure, Some(PaneError::Spawn(_))));
    }

    /// The genuine stateful AI↔AI proof: two real `claude -p` instances hold a
    /// counting conversation. Ignored by default — needs the `claude` CLI +
    /// network + auth. Run with:
    /// `cargo test -p sprag-plugin two_real_claudes_converse -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the claude CLI + network + auth; run manually with --ignored"]
    fn two_real_claudes_converse() {
        let claude = vec!["claude".to_string(), "-p".to_string()];
        let mut spec = DialogueSpec::new(
            claude.clone(),
            claude,
            "Let's count upward together, one integer per turn. Start at 1.",
        );
        spec.cols = 80;
        spec.rows = 24;
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let t = transcript.expect("a transcript");
        assert!(!t.trim().is_empty(), "transcript: {t:?}");
        // Stateful coherence: a counting dialogue surfaces several distinct
        // numbers (a stateless telephone game could not count past the seed).
        let distinct = ["1", "2", "3", "4"].iter().filter(|n| t.contains(**n)).count();
        assert!(distinct >= 2, "expected a coherent count, got: {t:?}");
    }
}
