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
//! Each endpoint declares a [`ReplyFormat`] that decides how its reply and cost
//! are read. `Text` (the default) takes the whole rendered pane output as the
//! reply and the prompt-byte count as a proxy cost. `ClaudeJson` runs the
//! endpoint as `claude -p --output-format json` and parses the envelope off the
//! pane's RAW output (the grid would corrupt the wrapped single-line JSON) to
//! get the real `result` text, the real billed tokens (input + output) as the
//! cost, and the `session_id` used for resume. Parsing degrades gracefully: a
//! truncated, oversized, or unparsable envelope falls back to the raw text and
//! the byte proxy, so a broken reply never breaks the conversation.
//!
//! Prompting has two modes. A side's FIRST turn (and every `Text` endpoint)
//! sends the whole transcript so far (instruction + seed + labelled history) so
//! the peer has full context. A `ClaudeJson` side that has already spoken
//! instead RESUMES its server session (`claude -p --resume <session_id>`) and
//! sends only the peer's last message — the server holds this side's prior
//! context, so the per-turn prompt is O(1) instead of resending the growing
//! transcript (the old O(n²) whole-history cost, which also hit the OS arg-size
//! cap on a long run). Each side keeps its own session; a resume that fails to
//! return a session id resets that side to the fresh whole-history path on its
//! next turn (self-healing). Each captured reply is kept verbatim (trimmed only
//! of surrounding blanks) and stored as a structured turn, so the conversation
//! payload — code, lists, multi-line answers — survives; turns stay delimited
//! by blank-line blocks (`render_turn`). Capture reads the pane's full output
//! (scrollback + visible, R16), so a reply longer than the pane is captured
//! whole — unlike [`Agent`], which still reads the visible screen only (it
//! injects into a non-fresh pane, so its reply region is the damage delta).
//!
//! [`Agent`]: crate::agent::Agent
//! [`Driver`]: crate::driver::Driver
//! [`Pipe`]: crate::pipe::Pipe

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError, PaneLifecycle};
use crate::plugin::{Plugin, Step, Verdict};
use crate::reply::parse_claude_json;
use crate::run::{poll_until, RunContext, Waited, DEFAULT_REPLY_TIMEOUT};

/// How a turn's reply text and cost are decoded from the endpoint's output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReplyFormat {
    /// The endpoint prints human text: the whole rendered pane output is the
    /// reply, and the cost is the prompt-byte proxy. The default — back-
    /// compatible with print-mode tools and the deterministic test fakes.
    #[default]
    Text,
    /// The endpoint prints a `claude -p --output-format json` envelope: the
    /// reply is its `result` and the cost is its real billed tokens
    /// (input + output). Read from the pane's RAW output, because the grid
    /// would corrupt the wrapped single-line JSON; on any parse failure it
    /// degrades to the raw text and the byte proxy (never breaks the run).
    ClaudeJson,
}

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
    /// How each side's reply + cost is decoded (defaults [`ReplyFormat::Text`]).
    /// Per-endpoint because the two sides can be different tools — one a
    /// `claude --output-format json`, the other a plain print-mode peer.
    pub format_a: ReplyFormat,
    pub format_b: ReplyFormat,
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
            format_a: ReplyFormat::default(),
            format_b: ReplyFormat::default(),
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
    /// The running conversation, in order — surfaced as the output transcript,
    /// and replayed into a turn's prompt on the fresh / Text path. Replies are
    /// kept verbatim (lossless).
    history: Vec<Turn>,
    /// Each side's server-session id (index `turn % 2`: 0 = A, 1 = B), set from
    /// a `ClaudeJson` reply's `session_id`. Once a side has one, its later turns
    /// `--resume` it and send only the new message instead of the whole
    /// transcript (server holds the context); `None` until the side has spoken,
    /// or after a resume failure (reset to re-establish context fresh).
    sessions: [Option<String>; 2],
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
            sessions: [None, None],
            turn: 0,
        }
    }
}

impl Plugin for Dialogue {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        let life = panes
            .lifecycle()
            .ok_or_else(|| PaneError::Spawn("pane access has no lifecycle".to_string()))?;

        // This turn's speaker — endpoint, label, decode format, and its session
        // — all selected by one parity index so they cannot drift apart.
        let idx = self.turn % 2;
        let (endpoint, label, format) = if idx == 0 {
            (&self.spec.endpoint_a, self.spec.label_a.clone(), self.spec.format_a)
        } else {
            (&self.spec.endpoint_b, self.spec.label_b.clone(), self.spec.format_b)
        };
        let session = self.sessions[idx].clone();

        // Resume only a claude endpoint that already holds a session: the server
        // keeps this side's context, so we send only the peer's new message
        // instead of the whole transcript (the O(n^2) resend). A Text endpoint
        // has no session, and a side that has not spoken yet has no id — both
        // take the fresh whole-history path.
        let resume = format == ReplyFormat::ClaudeJson && session.is_some();
        let prompt = if resume {
            render_resume_prompt(&self.spec.seed, &self.history, &label)
        } else {
            render_prompt(&self.spec.seed, &self.history, &label)
        };
        // The prompt-byte proxy: the honest fallback when real token cost is
        // unavailable, and the spend already committed by spawning the peer (so
        // it is the cost reported on the cancel path).
        let prompt_cost = prompt.len() as u64;
        // argv = endpoint ++ (["--resume", id] when resuming) ++ [prompt].
        let mut argv: Vec<String> = endpoint.to_vec();
        if resume {
            argv.push("--resume".to_string());
            argv.push(session.expect("resume implies a session"));
        }
        argv.push(prompt);

        let id = life.spawn(&argv, self.spec.cols, self.spec.rows)?;
        // From here every exit path must close the pane — the guard does it on
        // Ok, on the cancel early-return, on a later `?`, and on a panic unwind.
        let guard = PaneGuard { life, id };

        let waited = poll_until(run, self.spec.timeout, || panes.pane_eof(id).unwrap_or(true));

        // If cancelled mid-turn, record nothing (no junk partial turn) and
        // return Continue with the spend committed so far; the Driver's loop-top
        // ends the run Cancelled. The guard closes the pane on this return.
        if waited == Waited::Cancelled {
            return Ok(Step {
                cost: prompt_cost,
                verdict: Verdict::Continue,
            });
        }

        // Decode the reply + real cost + session id while the pane is still
        // alive; the guard closes it next. A structured endpoint whose envelope
        // is unparsable degrades to the raw text and the byte proxy (never fails
        // the turn).
        let decoded = decode_reply(format, panes, id, prompt_cost);
        drop(guard); // close the pane now (its blocking teardown is lock-free).

        // Session bookkeeping (the resume seam). A reply that carries a session
        // id is a healthy, continuable session — store it (idempotent; the id is
        // stable across resume, and an `is_error` reply on a live session is
        // still continuable). A RESUME turn that came back with NO id means the
        // resume did not produce a usable session (bad/expired id, or a garbled
        // envelope) — reset this side so the next same-side turn re-establishes
        // context via the whole-history fresh path (self-healing over one turn).
        match decoded.session_id {
            Some(sid) => self.sessions[idx] = Some(sid),
            None if resume => self.sessions[idx] = None,
            None => {}
        }

        // The reply is kept verbatim so the conversation payload — code blocks,
        // lists, multi-line answers — survives; turns stay delimited by
        // render_turn's blank-line blocks.
        self.history.push(Turn { label, text: decoded.text });
        self.turn += 1;

        // Never self-converges; the Driver's iteration/cost budget is the cap.
        Ok(Step {
            cost: decoded.cost,
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

/// The minimal prompt for a RESUME turn: just the peer's last message, in the
/// same labelled-block form (`render_turn`) the fresh prompt used for history,
/// so the model sees a consistent transcript line across the fresh→resume seam.
/// The instruction + seed + earlier turns already live in this side's server
/// session (`--resume`), so only the new message is sent. A resume turn always
/// follows the peer's turn, so `history` is non-empty; the fallback to the full
/// prompt is purely defensive (never panics on an unexpected empty history).
fn render_resume_prompt(seed: &str, history: &[Turn], speaker: &str) -> String {
    match history.last() {
        Some(turn) => render_turn(turn),
        None => render_prompt(seed, history, speaker),
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

/// What a turn decoded to: the reply text, its cost, and the tool's session id
/// (when it reports one — the id a later same-side turn `--resume`s). The
/// session id is `None` for a `Text` endpoint, and for a structured reply that
/// did not parse or carried none — the latter being the cue to reset a resume.
struct DecodedTurn {
    text: String,
    cost: u64,
    session_id: Option<String>,
}

/// Decode this turn's reply text, cost, and session id from the finished pane,
/// per the endpoint's [`ReplyFormat`]. Always succeeds: a structured endpoint
/// whose envelope is missing or garbled degrades to the raw text and the
/// byte-count `fallback_cost` (and no session id), so a parse problem never
/// fails a turn.
fn decode_reply(
    format: ReplyFormat,
    panes: &dyn PaneAccess,
    id: PaneId,
    fallback_cost: u64,
) -> DecodedTurn {
    match format {
        ReplyFormat::Text => DecodedTurn {
            text: capture_text(panes, id),
            cost: fallback_cost,
            session_id: None,
        },
        ReplyFormat::ClaudeJson => decode_claude_json(panes, id, fallback_cost),
    }
}

/// The rendered full-output text of a fresh per-turn pane (scrollback +
/// visible), trimmed. Nothing was injected, so the screen holds only the
/// endpoint's output; a reply longer than the pane is captured whole (R16).
fn capture_text(panes: &dyn PaneAccess, id: PaneId) -> String {
    panes.pane_full_text(id).unwrap_or_default().trim().to_string()
}

/// Decode a `claude -p --output-format json` reply from the pane's RAW output —
/// the grid would corrupt the wrapped single-line envelope (it inserts a `\n` at
/// every wrap and trailing-trims each row), so the source bytes are the only
/// faithful read. On any failure — no raw capture, a truncated/over-cap buffer,
/// or an unparsable envelope — fall back to the raw text and the byte-count
/// `fallback_cost`, so a broken reply degrades gracefully instead of breaking
/// the conversation.
fn decode_claude_json(panes: &dyn PaneAccess, id: PaneId, fallback_cost: u64) -> DecodedTurn {
    let (raw, truncated) = panes.pane_raw_output(id).unwrap_or_default();
    let raw_text = || String::from_utf8_lossy(&raw).trim().to_string();
    // A truncated capture is never trusted — its envelope is incomplete.
    let parsed = if truncated { None } else { parse_claude_json(&raw) };
    match parsed {
        Some(reply) => DecodedTurn {
            text: reply.text.filter(|t| !t.is_empty()).unwrap_or_else(raw_text),
            cost: reply.tokens.unwrap_or(fallback_cost),
            session_id: reply.session_id,
        },
        // No parse → no session id, so a resume turn here self-heals to fresh.
        None => DecodedTurn {
            text: raw_text(),
            cost: fallback_cost,
            session_id: None,
        },
    }
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

    /// A one-shot fake that prints a single-line `--output-format json`
    /// envelope: a fixed `result` and `usage` token counts, independent of the
    /// prompt (the `_` is the `$0` placeholder before the appended prompt `$1`).
    /// `result` must be free of single quotes (it rides in a single-quoted shell
    /// string). The JSON is one physical line, so on a normal pane it wraps —
    /// which is exactly why the reply must be read from the raw source, not the
    /// grid.
    fn json_fake(result: &str, input: u64, output: u64) -> Vec<String> {
        let json = format!(
            r#"{{"result":"{result}","usage":{{"input_tokens":{input},"output_tokens":{output}}}}}"#
        );
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("printf '%s' '{json}'"),
            "_".to_string(),
        ]
    }

    /// A fake that self-reports whether its argv carried `--resume`: it prints a
    /// JSON envelope whose `result` is `"resumed"` if `$*` contains `--resume`,
    /// else `"fresh"`, and always a stable `session_id` `"S"` (like real
    /// claude). So a side's first turn (no session yet) reads `fresh` and a
    /// later turn (resuming the stored `"S"`) reads `resumed`.
    fn resume_probe_fake() -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "case \"$*\" in *--resume*) r=resumed ;; *) r=fresh ;; esac; \
             printf '{\"result\":\"%s\",\"session_id\":\"S\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}' \"$r\""
                .to_string(),
            "_".to_string(),
        ]
    }

    /// A fake that emits a valid envelope (session `"S"`, result `"ok"`) on a
    /// fresh call but NON-JSON on a `--resume` call — simulating a bad/expired
    /// session id. The unparsable resume reply degrades and yields no session
    /// id, so the side resets to the fresh path on its next turn.
    fn selfheal_fake() -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "case \"$*\" in *--resume*) printf 'GARBAGE not json' ;; \
             *) printf '{\"result\":\"ok\",\"session_id\":\"S\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}' ;; esac"
                .to_string(),
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
    fn claude_json_reply_reports_real_token_cost() {
        // A wide reply with interior spaces that the grid would wrap and trim;
        // reading the raw source captures it verbatim, and the cost is the real
        // tokens — not the prompt-byte proxy.
        let result = "the quick brown fox ".repeat(8); // 160 chars, many spaces
        let mut spec = DialogueSpec::new(
            json_fake(&result, 30, 20),
            json_fake(&result, 30, 20),
            "go",
        );
        spec.format_a = ReplyFormat::ClaudeJson;
        spec.format_b = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 2);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        // Two turns × (30 + 20) tokens — proof the Driver accumulates real
        // tokens, not the byte proxy (the render_prompt length would differ).
        assert_eq!(outcome.cost, 100, "cost must be the summed real tokens");
        let t = transcript.expect("a transcript");
        // The full wide result survives verbatim; a grid read would have dropped
        // interior spaces at wrap boundaries (e.g. "fox the" -> "foxthe").
        assert!(t.contains(result.trim()), "faithful reply missing: {t:?}");
    }

    #[test]
    fn claude_json_cost_binds_the_guardrail_in_tokens() {
        // One turn spends 50 tokens, hitting a 50-token budget — so a
        // 100-iteration run still stops after one turn. Proves tokens (not the
        // byte proxy, not iterations) bound the run.
        let mut spec = DialogueSpec::new(json_fake("hi", 30, 20), json_fake("hi", 30, 20), "go");
        spec.format_a = ReplyFormat::ClaudeJson;
        spec.format_b = ReplyFormat::ClaudeJson;

        let workspace = Arc::new(Mutex::new(Workspace::new((spec.cols, spec.rows))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut dialogue = Dialogue::new(spec);
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: 50,
        })
        .run(&mut dialogue, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert_eq!(outcome.iterations, 1, "one turn must exhaust the 50-token budget");
        assert_eq!(outcome.cost, 50);
    }

    #[test]
    fn claude_json_degrades_on_unparsable_reply() {
        // The endpoint declares ClaudeJson but emits plain text (a misconfig or
        // a truncated envelope): the turn must NOT fail — it records the raw
        // text and bills the byte proxy, keeping the conversation alive.
        let garbage = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf 'just plain text, not json'".to_string(),
            "_".to_string(),
        ];
        let mut spec = DialogueSpec::new(garbage, vec!["true".to_string()], "go");
        spec.format_a = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 1);

        assert_eq!(outcome.state, OutcomeState::Exhausted, "must not Fail on bad JSON");
        let t = transcript.expect("a transcript");
        assert!(t.contains("just plain text"), "raw fallback text missing: {t:?}");
        // The byte proxy still bound the run (a positive fallback cost).
        assert!(outcome.cost > 0, "fallback cost must be the positive byte proxy");
    }

    #[test]
    fn claude_json_resumes_after_the_first_turn() {
        // Both sides self-report whether their argv carried --resume. A side's
        // FIRST turn has no session yet (fresh whole-history path, no --resume);
        // its LATER turn resumes the session id it stored — proving the resume
        // seam fires on turn 2+ of each side, never turn 1. Network-free.
        let mut spec = DialogueSpec::new(resume_probe_fake(), resume_probe_fake(), "go");
        spec.format_a = ReplyFormat::ClaudeJson;
        spec.format_b = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let t = transcript.expect("a transcript");
        let a_fresh = t.find("A: fresh").expect("A's first turn must be fresh");
        let a_resumed = t.find("A: resumed").expect("A's later turn must resume");
        let b_fresh = t.find("B: fresh").expect("B's first turn must be fresh");
        let b_resumed = t.find("B: resumed").expect("B's later turn must resume");
        assert!(a_fresh < a_resumed, "A resumed before its fresh turn: {t:?}");
        assert!(b_fresh < b_resumed, "B resumed before its fresh turn: {t:?}");
    }

    #[test]
    fn resume_failure_self_heals_to_fresh() {
        // The fake returns a session id on a fresh call but non-JSON on a
        // --resume call (a bad/expired id). Turn 0 (A) stores the session; turn
        // 2 (A) resumes and gets garbage -> decode yields no session id -> A
        // resets; turn 4 (A) is fresh again and parses cleanly. So "A: ok"
        // appears on BOTH turn 0 and turn 4 — without self-heal turn 4 would
        // still resume and re-garble. Network-free.
        let mut spec = DialogueSpec::new(selfheal_fake(), selfheal_fake(), "go");
        spec.format_a = ReplyFormat::ClaudeJson;
        spec.format_b = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 5);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        let t = transcript.expect("a transcript");
        assert_eq!(
            t.matches("A: ok").count(),
            2,
            "A must parse cleanly on turn 0 AND again on turn 4 (self-healed): {t:?}"
        );
        assert!(t.contains("A: GARBAGE"), "turn 2's bad resume must be recorded raw: {t:?}");
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
    ///
    /// This also exercises `--resume` end-to-end: turns 0-1 are fresh (each side
    /// first speaks), turns 2-3 RESUME each side's real session and send only the
    /// peer's last message. A coherent count across turns 2-3 proves the server
    /// preserved context via `--resume` — a fresh-but-contextless turn could not.
    #[test]
    #[ignore = "needs the claude CLI + network + auth; run manually with --ignored"]
    fn two_real_claudes_converse() {
        // Run both sides as `claude -p --output-format json` and decode each
        // turn as a JSON envelope — the real-PTY check that the envelope parses
        // off the raw source (no wrap corruption, no stderr pollution), that the
        // cost is the real billed tokens (not the byte proxy), and that --resume
        // carries context across turns.
        let claude = vec![
            "claude".to_string(),
            "-p".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ];
        let mut spec = DialogueSpec::new(
            claude.clone(),
            claude,
            "Let's count upward together, one integer per turn. Start at 1.",
        );
        spec.cols = 80;
        spec.rows = 24;
        spec.format_a = ReplyFormat::ClaudeJson;
        spec.format_b = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted);
        // Real token cost accumulated over the turns (the round's whole point).
        eprintln!("two_real_claudes_converse: real token cost = {}", outcome.cost);
        assert!(outcome.cost > 0, "expected real token cost, got {}", outcome.cost);
        let t = transcript.expect("a transcript");
        assert!(!t.trim().is_empty(), "transcript: {t:?}");
        // JSON parse succeeded (not degraded to raw): the transcript holds the
        // clean `result` text, so it must not carry the raw envelope's fields.
        assert!(!t.contains("input_tokens"), "transcript leaked the raw envelope: {t:?}");
        // Stateful coherence: a counting dialogue surfaces several distinct
        // numbers (a stateless telephone game could not count past the seed).
        let distinct = ["1", "2", "3", "4"].iter().filter(|n| t.contains(**n)).count();
        assert!(distinct >= 2, "expected a coherent count, got: {t:?}");
    }
}
