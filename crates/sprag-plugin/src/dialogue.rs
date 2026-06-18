//! The `Dialogue` plugin — a turn-based conversation between two AI tools
//! (adapter #2, the north star's full "AI↔AI").
//!
//! Round 12's [`Agent`] made *one* side real (one prompt, one reply). This
//! drives *two* one-shot AI endpoints in alternation: each turn spawns the
//! active endpoint as a fresh pane with the other side's last message as its
//! argv prompt (`claude -p "<message>"`), waits for it to reply and exit,
//! captures the reply, closes the pane, and hands the reply to the other side.
//! The transcript of every turn is surfaced as the run's output.
//!
//! Message-as-argv (not keystroke injection) is deliberate: a one-shot tool
//! takes its prompt on the command line, so there is no stdin and no cooked-
//! mode echo to strip — the fresh pane's whole screen is the reply. Each turn
//! is a fresh process, so this needs pane *lifecycle* ([`PaneLifecycle`]); a
//! [`PaneGuard`] closes the per-turn pane on every exit path (no leaked PTY/
//! child even if a step fails mid-way). It never self-converges — the
//! [`Driver`]'s `max_iterations` is the turn budget, so a run ends `Exhausted`
//! with the transcript as its payload (the [`Pipe`] pattern).
//!
//! Known limitations (deferred): turns are **stateless** — each side sees only
//! the other's last message, not the running history (a telephone game, not a
//! memory-preserving conversation); context accumulation (`claude -p --resume`,
//! or whole-history prompts) is the next step. Capture inherits [`Agent`]'s:
//! a reply longer than the pane scrolls loses the off-screen rows (no
//! scrollback projection yet), and a peer that paints a UI frame would pollute
//! the captured text (print-mode tools like `claude -p` do not).
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

/// Who talks to whom, and how each turn is bounded.
#[derive(Clone, Debug)]
pub struct DialogueSpec {
    /// Endpoint A's argv template (e.g. `["claude", "-p"]`); the turn's message
    /// is appended as the final argument.
    pub endpoint_a: Vec<String>,
    /// Endpoint B's argv template.
    pub endpoint_b: Vec<String>,
    /// The first message, given to endpoint A on turn 0.
    pub seed: String,
    /// Size of each per-turn pane.
    pub cols: u16,
    pub rows: u16,
    /// Overall bound on one turn's reply wait; on timeout the turn captures
    /// whatever is on screen rather than hanging.
    pub timeout: Duration,
}

impl DialogueSpec {
    /// A spec with default pane size (80x24) and timeout; set the fields after
    /// for overrides.
    #[must_use]
    pub fn new(endpoint_a: Vec<String>, endpoint_b: Vec<String>, seed: impl Into<String>) -> Self {
        Self {
            endpoint_a,
            endpoint_b,
            seed: seed.into(),
            cols: 80,
            rows: 24,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// A turn-based dialogue between two one-shot AI endpoints.
pub struct Dialogue {
    spec: DialogueSpec,
    /// The message the next turn's endpoint will be prompted with.
    current_message: String,
    /// Turn counter; even turns speak as A, odd as B.
    turn: usize,
    /// Every turn's captured reply, surfaced through [`Plugin::captured`].
    transcript: Vec<String>,
}

impl Dialogue {
    /// Start a dialogue from `spec` (turn 0 prompts endpoint A with the seed).
    #[must_use]
    pub fn new(spec: DialogueSpec) -> Self {
        let current_message = spec.seed.clone();
        Self {
            spec,
            current_message,
            turn: 0,
            transcript: Vec::new(),
        }
    }
}

impl Plugin for Dialogue {
    fn step(&mut self, panes: &dyn PaneAccess) -> Result<Step, InjectError> {
        let life = panes
            .lifecycle()
            .ok_or_else(|| InjectError::Spawn("pane access has no lifecycle".to_string()))?;

        // This turn's speaker and its prompt (the other side's last message,
        // appended as the final argv element — preserved verbatim, even if it
        // spans lines, because it is one argv element, not a shell string).
        let endpoint = if self.turn.is_multiple_of(2) {
            &self.spec.endpoint_a
        } else {
            &self.spec.endpoint_b
        };
        let argv: Vec<String> = endpoint
            .iter()
            .cloned()
            .chain(std::iter::once(self.current_message.clone()))
            .collect();

        let id = life.spawn(&argv, self.spec.cols, self.spec.rows)?;
        // From here, every exit path must close the pane — the guard does it on
        // Ok, on a later `?`, and on a panic unwind. No leaked PTY or child.
        let guard = PaneGuard { life, id };

        await_reply(panes, id, self.spec.timeout);
        let reply = capture(panes, id);
        drop(guard); // close the pane now (its blocking teardown is lock-free).

        self.transcript.push(reply.clone());
        self.current_message = reply;
        self.turn += 1;

        // Never self-converges; the Driver's iteration budget is the turn cap.
        // Nothing is injected (the message rides as argv), so injected_bytes is
        // honestly 0 — the turn budget is max_iterations, not a byte count.
        Ok(Step {
            injected_bytes: 0,
            verdict: Verdict::Continue,
        })
    }

    fn captured(&self) -> Option<String> {
        if self.transcript.is_empty() {
            None
        } else {
            Some(self.transcript.join("\n----\n"))
        }
    }
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

    /// An argv template for a one-shot fake endpoint that prefixes the message
    /// (its `$1`) with `tag` and exits. `printf` avoids `echo`'s portability
    /// quirks; the `_` is the `$0` placeholder `sh -c` consumes before `$1`.
    fn endpoint(tag: &str) -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("printf '%s\\n' \"{tag}:$1\""),
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
    fn multi_turn_single_endpoint_threads_the_message() {
        // Both sides identical: a single agent talking to itself across turns.
        // The message nests deeper each turn (resp:resp:...:hi).
        let spec = DialogueSpec::new(endpoint("resp"), endpoint("resp"), "hi");
        let (_ws, outcome, transcript) = run(spec, 3);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert_eq!(outcome.iterations, 3);
        let transcript = transcript.expect("a transcript");
        assert!(transcript.contains("resp:hi"), "transcript: {transcript:?}");
        assert!(
            transcript.contains("resp:resp:resp:hi"),
            "third turn should nest thrice: {transcript:?}"
        );
    }

    #[test]
    fn bidirectional_turns_alternate_endpoints() {
        // A and B tag distinctly, so the transcript shows turns alternating and
        // the message threading through both: A:x -> B:A:x -> A:B:A:x -> ...
        let spec = DialogueSpec::new(endpoint("A"), endpoint("B"), "x");
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let transcript = transcript.expect("a transcript");
        assert!(transcript.contains("A:x"), "turn 0: {transcript:?}");
        assert!(transcript.contains("B:A:x"), "turn 1: {transcript:?}");
        assert!(transcript.contains("A:B:A:x"), "turn 2: {transcript:?}");
        assert!(transcript.contains("B:A:B:A:x"), "turn 3: {transcript:?}");
    }

    #[test]
    fn leaves_no_leftover_panes() {
        // Every turn spawns a pane; the guard must close each one.
        let spec = DialogueSpec::new(endpoint("A"), endpoint("B"), "x");
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
        // no hang. The captured reply is empty (sleep produced nothing).
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

    /// The genuine AI↔AI proof: two real `claude -p` instances converse.
    /// Ignored by default — needs the `claude` CLI + network + auth. Run with:
    /// `cargo test -p sprag-plugin two_real_claudes_converse -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the claude CLI + network + auth; run manually with --ignored"]
    fn two_real_claudes_converse() {
        let claude = vec!["claude".to_string(), "-p".to_string()];
        let mut spec = DialogueSpec::new(
            claude.clone(),
            claude,
            "Start a two-line conversation. Reply with exactly one short sentence.",
        );
        spec.cols = 80;
        spec.rows = 24;
        let (_ws, outcome, transcript) = run(spec, 2);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let transcript = transcript.expect("a transcript");
        assert!(!transcript.trim().is_empty(), "transcript: {transcript:?}");
    }
}
