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
//! fails loudly (`InjectError::Spawn`) rather than corrupting — `claude -p
//! --resume` (server-side session state) is the future optimization. Each
//! captured reply is collapsed to a single line so it stays one labelled
//! transcript entry; intentional line breaks in a reply are lost. Capture
//! inherits [`Agent`]'s: a reply longer than the pane scrolls loses the
//! off-screen rows (no scrollback projection yet).
//!
//! [`Agent`]: crate::agent::Agent
//! [`Driver`]: crate::driver::Driver
//! [`Pipe`]: crate::pipe::Pipe

use std::thread::sleep;
use std::time::{Duration, Instant};

use sprag_terminal::PaneId;

use crate::access::{InjectError, PaneAccess, PaneLifecycle};
use crate::plugin::{Plugin, Step, Verdict};

/// Poll interval while waiting for a turn's pane child to reply and exit.
const REPLY_POLL: Duration = Duration::from_millis(10);

/// Default overall bound on one turn's reply — generous, since a real model
/// thinks for seconds. Overridable per run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

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
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// A stateful turn-based dialogue between two one-shot AI endpoints.
pub struct Dialogue {
    spec: DialogueSpec,
    /// Every turn's reply as `"<label>: <reply>"`, in order — the running
    /// conversation, replayed into each turn's prompt and surfaced as output.
    history: Vec<String>,
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
    fn step(&mut self, panes: &dyn PaneAccess) -> Result<Step, InjectError> {
        let life = panes
            .lifecycle()
            .ok_or_else(|| InjectError::Spawn("pane access has no lifecycle".to_string()))?;

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
        let argv: Vec<String> = endpoint
            .iter()
            .cloned()
            .chain(std::iter::once(prompt))
            .collect();

        let id = life.spawn(&argv, self.spec.cols, self.spec.rows)?;
        // From here every exit path must close the pane — the guard does it on
        // Ok, on a later `?`, and on a panic unwind. No leaked PTY or child.
        let guard = PaneGuard { life, id };

        await_reply(panes, id, self.spec.timeout);
        let reply = capture(panes, id);
        drop(guard); // close the pane now (its blocking teardown is lock-free).

        // Collapse to a single line so the turn is one labelled transcript
        // entry; otherwise a multi-line reply would inject unlabelled lines
        // into the next turn's prompt.
        let reply = reply.split_whitespace().collect::<Vec<_>>().join(" ");
        self.history.push(format!("{label}: {reply}"));
        self.turn += 1;

        // Never self-converges; the Driver's iteration budget is the turn cap.
        // Nothing is injected (the prompt rides as argv), so injected_bytes is
        // honestly 0 — the turn budget is max_iterations, not a byte count.
        Ok(Step {
            injected_bytes: 0,
            verdict: Verdict::Continue,
        })
    }

    fn captured(&self) -> Option<String> {
        if self.history.is_empty() {
            None
        } else {
            Some(self.history.join("\n"))
        }
    }
}

/// Render one turn's prompt: a one-line instruction naming `speaker`, then the
/// seed, then the labelled transcript so far. Joined by `\n` with **no trailing
/// newline** — the offline count-fake test keys on this exact shape (one `\n`
/// per logical line), so changing the line structure will shift those counts.
fn render_prompt(seed: &str, history: &[String], speaker: &str) -> String {
    let mut lines = Vec::with_capacity(history.len() + 2);
    lines.push(format!(
        "You are {speaker} in this two-party dialogue. Reply with only {speaker}'s next message."
    ));
    lines.push(seed.to_string());
    lines.extend(history.iter().cloned());
    lines.join("\n")
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

/// Wait (bounded by `timeout`) for the turn's pane child to reply and exit —
/// once it has, its full reply is on screen ([`PaneAccess::pane_eof`]).
fn await_reply(panes: &dyn PaneAccess, id: PaneId, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if panes.pane_eof(id).unwrap_or(true) {
            return;
        }
        sleep(REPLY_POLL);
    }
}

/// The reply on a fresh per-turn pane: its non-empty rows, joined. Nothing was
/// injected, so the screen holds only the endpoint's output.
fn capture(panes: &dyn PaneAccess, id: PaneId) -> String {
    panes
        .pane_rows(id)
        .map(|rows| {
            rows.iter()
                .filter(|row| !row.text.is_empty())
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
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
            max_injected_bytes: u64::MAX,
        })
        .run(&mut dialogue, &access);
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
        assert!(t.contains("A: saw1"), "turn 0 (1-line prompt): {t:?}");
        assert!(t.contains("B: saw2"), "turn 1 (2-line prompt): {t:?}");
        assert!(t.contains("A: saw3"), "turn 2 (3-line prompt): {t:?}");
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
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<u64, InjectError> {
                Err(InjectError::UnknownPane(PaneId(0)))
            }
        }

        let mut dialogue = Dialogue::new(DialogueSpec::new(
            vec!["claude".to_string()],
            vec!["claude".to_string()],
            "hi",
        ));
        let outcome = Driver::new(Guardrails {
            max_iterations: 3,
            max_injected_bytes: u64::MAX,
        })
        .run(&mut dialogue, &NoPanes);
        assert_eq!(outcome.state, OutcomeState::Failed);
        assert!(matches!(outcome.failure, Some(InjectError::Spawn(_))));
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
