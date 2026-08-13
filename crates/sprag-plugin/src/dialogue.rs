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
//! ([`PaneLifecycle`]); a `PaneGuard` closes the per-turn pane on every exit
//! path (no leaked PTY/child even if a step fails mid-way). It never
//! self-converges — the [`Driver`]'s `max_iterations` is the turn budget, so a
//! run ends `Exhausted` with the transcript as its payload (the [`Pipe`]
//! pattern).
//!
//! Dialogue is a TOKEN-denominated plugin: every turn's [`Cost`] is
//! [`Cost::Tokens`]. Each endpoint declares a [`ReplyFormat`] that decides how
//! its reply and tokens are read. `Text` (the default) takes the whole rendered
//! pane output as the reply and reports `Tokens(0)` (a print-mode tool has no
//! token accounting). `ClaudeJson` runs the endpoint as `claude -p
//! --output-format json` and parses the envelope off the pane's RAW output (the
//! grid would corrupt the wrapped single-line JSON) to get the real `result`
//! text, the real billed tokens (input + output) as the cost, and the
//! `session_id` used for resume. Parsing degrades gracefully: a truncated,
//! oversized, or unparsable envelope falls back to the raw text and `Tokens(0)`,
//! so a broken reply never breaks the conversation.
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

use sprag_terminal::{PaneId, RawOutput};

use crate::access::{PaneAccess, PaneError, PaneLifecycle};
use crate::completion::{Completion, DoneWhen, Over};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::reply::parse_claude_json;
use crate::run::{DEFAULT_REPLY_TIMEOUT, RunContext};
use crate::session::Session;

sprag_vt::closed_set! {
    /// How a turn's reply text and cost are decoded. Dialogue is token-denominated,
    /// so every variant's cost is [`Cost::Tokens`].
    ///
    /// A closed set because the host PUBLISHES this vocabulary on the `run` verb's `format_a` /
    /// `format_b` arguments, where it used to be two string literals inside the host's own parser.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum ReplyFormat {
        /// The endpoint prints human text: the whole rendered pane output is the
        /// reply, and the cost is `Tokens(0)` — a print-mode tool has no token
        /// accounting. The default — back-compatible with print-mode tools and the
        /// deterministic test fakes.
        #[default]
        Text,
        /// The endpoint prints a `claude -p --output-format json` envelope: the
        /// reply is its `result` and the cost is its real billed tokens
        /// (input + output). Read from the pane's RAW output, because the grid
        /// would corrupt the wrapped single-line JSON; on any parse failure it
        /// degrades to the raw text and `Tokens(0)` (never breaks the run).
        ClaudeJson,
    }
}

impl ReplyFormat {
    /// This format's word in a `run` request's `format_a` / `format_b`.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::ClaudeJson => "claude_json",
        }
    }

    /// The format a word names, or [`None`] for a word no format spells.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

sprag_vt::wire_words!(ReplyFormat: wire_str);

/// One side of the dialogue: how to launch it, what to call it in the
/// transcript, and how to decode its reply. Collapsing argv + label + format
/// into one per-side struct keeps them from drifting apart — the parity index
/// (`turn % 2`) selects ONE `Endpoint`, not three parallel field pairs. A future
/// per-endpoint knob (a model name, a temperature) is a new field here, touched
/// in one place rather than as a fourth `xxx_a`/`xxx_b` pair.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The endpoint's argv template (e.g. `["claude", "-p"]`); the turn's prompt
    /// (and a `--resume` pair when resuming) is appended before launch.
    pub argv: Vec<String>,
    /// Transcript label for this side (e.g. `"A"`).
    pub label: String,
    /// How this side's reply + cost is decoded (defaults [`ReplyFormat::Text`]).
    /// Per-endpoint because the two sides can be different tools — one a
    /// `claude --output-format json`, the other a plain print-mode peer.
    pub format: ReplyFormat,
}

/// Who talks to whom, with what seed, and how each turn is bounded.
#[derive(Clone, Debug)]
pub struct DialogueSpec {
    /// The two endpoints: index 0 = A (speaks on even turns), 1 = B (odd). The
    /// `turn % 2` parity selects one `Endpoint`, so its argv, label, and format
    /// move together and cannot desync.
    pub endpoints: [Endpoint; 2],
    /// The opening message / topic, given to both sides as context.
    pub seed: String,
    /// Size of each per-turn pane.
    pub cols: u16,
    pub rows: u16,
    /// Overall bound on one turn's reply wait; on timeout the turn captures
    /// whatever is on screen rather than hanging.
    pub timeout: Duration,
}

impl DialogueSpec {
    /// A spec from the two endpoints' argv templates, with default labels
    /// (`"A"`/`"B"`), [`ReplyFormat::Text`], pane size (80x24), and timeout; set
    /// `endpoints[n].format` / `.label` after for overrides.
    #[must_use]
    pub fn new(endpoint_a: Vec<String>, endpoint_b: Vec<String>, seed: impl Into<String>) -> Self {
        Self {
            endpoints: [
                Endpoint {
                    argv: endpoint_a,
                    label: "A".to_string(),
                    format: ReplyFormat::default(),
                },
                Endpoint {
                    argv: endpoint_b,
                    label: "B".to_string(),
                    format: ReplyFormat::default(),
                },
            ],
            seed: seed.into(),
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

/// Render one turn as a transcript block (`"<label>: <text>"`). A multi-line
/// `text` stays grouped under its label. The blank-line delimiting BETWEEN
/// blocks happens at the join sites ([`Dialogue::captured`] and
/// [`render_prompt`], which `join("\n\n")` these blocks) — not here.
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
    /// Each side's server-session lifecycle (index `turn % 2`: 0 = A, 1 = B). A
    /// side holding a resumed session sends only the new message instead of the
    /// whole transcript; the [`Session`] state machine owns the
    /// fresh/resumed/reset transitions (the resume + self-heal logic this plugin
    /// used to hand-roll inline).
    sessions: [Session; 2],
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
            sessions: [Session::new(), Session::new()],
            turn: 0,
        }
    }
}

impl Plugin for Dialogue {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        let life = panes
            .lifecycle()
            .ok_or_else(|| PaneError::Spawn("pane access has no lifecycle".to_string()))?;

        // This turn's speaker is one `Endpoint` chosen by the parity index, so
        // its launch argv, transcript label, and decode format move together
        // and cannot drift apart. Clone them out so the `&self.spec` borrow ends
        // before the later `self.history` / `self.sessions` mutations.
        let idx = self.turn % 2;
        let (argv_template, label, format) = {
            let endpoint = &self.spec.endpoints[idx];
            (
                endpoint.argv.clone(),
                endpoint.label.clone(),
                endpoint.format,
            )
        };

        // Resume when this side holds a live session: the server keeps its
        // context, so we send only the peer's new message instead of the whole
        // transcript (the O(n^2) resend). Only a structured endpoint ever opens
        // a session — a Text turn decodes to no id, so its `Session` stays
        // `fresh` forever and `resuming()` is None — so this state-based gate
        // subsumes the old explicit `format == ClaudeJson` check. Clone the id
        // out so the `&self.sessions` borrow ends before the later `record`.
        let resume_id: Option<String> = self.sessions[idx].resuming().map(str::to_string);
        let prompt = if resume_id.is_some() {
            render_resume_prompt(&self.spec.seed, &self.history, &label)
        } else {
            render_prompt(&self.spec.seed, &self.history, &label)
        };
        // argv = endpoint ++ (["--resume", id] when resuming) ++ [prompt].
        let mut argv = argv_template;
        if let Some(id) = resume_id {
            argv.push("--resume".to_string());
            argv.push(id);
        }
        argv.push(prompt);

        let id = life.spawn(&argv, self.spec.cols, self.spec.rows)?;
        // From here every exit path must close the pane — the guard does it on
        // Ok, on the cancel early-return, on a later `?`, and on a panic unwind.
        let guard = PaneGuard { life, id };

        // ⚠ The SAME contract the agent adapter waits on, from the one definition rather than from
        // a second copy of the predicate — see [`mod@crate::completion`]. Here the rule is exactly
        // right and always will be: this plugin SPAWNS a one-shot CLI per turn and passes the
        // prompt as an argv argument, so the turn really is over when that process leaves.
        //
        // ⚠ AND IT IS DELIBERATELY NOT ARMED, so [`Over::Asking`] is an ending this one plugin
        // cannot reach. Arming is what lets a turn's end report that its peer stopped to ASK, and
        // the two facts that make it worth having are both absent here: this plugin spawns its
        // own pane per turn and passes the prompt as an argv argument to a PRINT-MODE CLI, which
        // renders no dialog for anybody to answer, and the pane is destroyed before the next turn
        // begins. A supervision pull per turn to arm a claim about a program that cannot make it
        // would be ceremony. Said here rather than left to be inferred from an absent call.
        let waited = Completion::new(DoneWhen::Exits).wait(panes, id, self.spec.timeout, run);

        // If the RUN ended mid-turn — cancelled, or out of time — record nothing
        // (no junk partial turn) and return Continue with the spend committed so
        // far; the Driver's loop top decides which terminal state it was. The
        // guard closes the pane on this return.
        if waited == Over::RunEnded {
            // Token-denominated plugin: a stopped turn has no measured token spend
            // (Tokens(0)); the run's terminal state is the loop top's to decide.
            return Ok(
                Step::new(Cost::Tokens(0), Verdict::Continue).noting(format!(
                    "turn {}: the run ended before {label} replied",
                    self.turn
                )),
            );
        }

        // Decode the reply + real cost + session id while the pane is still
        // alive; the guard closes it next. A structured endpoint whose envelope
        // is unparsable degrades to the raw text and Tokens(0) (never fails the
        // turn).
        let decoded = decode_reply(format, panes, id);
        drop(guard); // close the pane now (its blocking teardown is lock-free).

        // Fold the decoded id into this side's lifecycle (the resume seam). A
        // present id opens or keeps the resumed session (idempotent; the id is
        // stable across resume, and a tool-side error on a live session is still
        // continuable). Its absence on a resumed side is a lost resume
        // (bad/expired id, or a garbled envelope) that self-heals to the fresh
        // whole-history path; on a fresh side it is a no-op. The `Session` state
        // machine owns all four transitions — this plugin no longer hand-rolls
        // the `None if resume => reset` rule.
        self.sessions[idx].record(decoded.session_id);

        // The reply is kept verbatim so the conversation payload — code blocks,
        // lists, multi-line answers — survives; turns stay delimited by
        // render_turn's blank-line blocks.
        self.history.push(Turn {
            label,
            text: decoded.text,
        });
        self.turn += 1;

        // Never self-converges; the Driver's iteration/cost budget is the cap.
        // ⚠ The note names the SPEAKER and the reply's size: a dialogue that stopped converging is
        // read by looking for the turn one side went quiet, and a run that reports only its token
        // total cannot show that.
        Ok(Step::new(decoded.cost, Verdict::Continue).noting(format!(
            "turn {}: {} replied with {} characters",
            self.turn - 1,
            self.history.last().map_or("?", |turn| turn.label.as_str()),
            self.history
                .last()
                .map_or(0, |turn| turn.text.chars().count()),
        )))
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

    /// ⚠⚠ **NOTHING — because this plugin OWNS its pane and already closes it.**
    ///
    /// A turn spawns its own pane and closes it on every exit path (the crate-private `PaneGuard`),
    /// a cancel or a passed deadline takes. Closing a pane ends the pane's whole process group, so
    /// the work is over by a stronger route than a signal to the foreground job — there is nothing
    /// left for [`Plugin::driving`] to name.
    ///
    /// Recorded rather than left to the default, because the default and this answer the same way
    /// for opposite reasons, and a reader who assumed the omission was an oversight would "fix" it
    /// by naming a pane that no longer exists.
    fn driving(&self) -> Option<PaneId> {
        None
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
    cost: Cost,
    session_id: Option<String>,
}

/// Decode this turn's reply text, cost, and session id from the finished pane,
/// per the endpoint's [`ReplyFormat`]. Dialogue is token-denominated, so every
/// turn's cost is [`Cost::Tokens`]: a `Text` endpoint has no token accounting
/// and reports `Tokens(0)` (a print-mode tool whose spend we cannot measure); a
/// `ClaudeJson` endpoint reports its real billed tokens and degrades to
/// `Tokens(0)` on any parse problem, so a parse problem never fails a turn.
fn decode_reply(format: ReplyFormat, panes: &dyn PaneAccess, id: PaneId) -> DecodedTurn {
    match format {
        ReplyFormat::Text => DecodedTurn {
            text: capture_text(panes, id),
            cost: Cost::Tokens(0),
            session_id: None,
        },
        ReplyFormat::ClaudeJson => decode_claude_json(panes, id),
    }
}

/// The full output of a fresh per-turn pane as the LOGICAL LINES the endpoint wrote, trimmed.
/// Nothing was injected, so the screen holds only the endpoint's output; a reply longer than the
/// pane is captured whole (R16).
///
/// ⚠⚠ **THIS READ THE RENDERED GRID, AND THE FUNCTION BELOW IT SAYS WHY THAT IS WRONG** — *"the
/// grid would corrupt the wrapped single-line envelope (it inserts a `\n` at every wrap and
/// trailing-trims each row)"*. That was written for the JSON path, which routed around it; the
/// PLAIN-TEXT path went on publishing a model's reply with a newline injected at every wrap and the
/// space at each break eaten. **A hazard known well enough to be worked around in one caller was
/// still live in the one beside it.**
fn capture_text(panes: &dyn PaneAccess, id: PaneId) -> String {
    panes
        .pane_full_lines(id)
        .unwrap_or_default()
        .join("\n")
        .trim()
        .to_string()
}

/// Decode a `claude -p --output-format json` reply from the pane's RAW output —
/// the grid would corrupt the wrapped single-line envelope (it inserts a `\n` at
/// every wrap and trailing-trims each row), so the source bytes are the only
/// faithful read. The cost is the reply's real billed tokens; on any failure —
/// no raw capture, a truncated/over-cap buffer, or an unparsable envelope — the
/// turn degrades to the raw text and `Tokens(0)` (no measured spend), so a
/// broken reply degrades gracefully instead of breaking the conversation.
fn decode_claude_json(panes: &dyn PaneAccess, id: PaneId) -> DecodedTurn {
    // Raw source bytes via the raw-capture capability (absent on a pane access
    // that does not provide one → degrade like an unparsable reply).
    let RawOutput {
        bytes: raw,
        truncated,
    } = panes
        .raw_capture()
        .and_then(|rc| rc.pane_raw_output(id))
        .unwrap_or_default();
    let raw_text = || String::from_utf8_lossy(&raw).trim().to_string();
    // A truncated capture is never trusted — its envelope is incomplete.
    let parsed = if truncated {
        None
    } else {
        parse_claude_json(&raw)
    };
    match parsed {
        // A cleanly parsed envelope: record its `result`. An empty or missing
        // `result` becomes an EMPTY turn (not the raw envelope) — the raw-text
        // fallback belongs ONLY to the parse-FAILED arm below, so a result-less
        // reply never leaks its usage/session_id into the transcript or the next
        // prompt's history. The real tokens are still billed (absent usage → 0).
        Some(reply) => DecodedTurn {
            text: reply.text.unwrap_or_default(),
            cost: Cost::Tokens(reply.tokens.unwrap_or(0)),
            session_id: reply.session_id,
        },
        // No parse → the raw text, no measured tokens (Tokens(0)), and no session
        // id (so a resume turn here self-heals to the fresh whole-history path).
        None => DecodedTurn {
            text: raw_text(),
            cost: Cost::Tokens(0),
            session_id: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{KeyStroke, PaneRow, WorkspacePaneAccess, Written};
    use crate::driver::{Ceiling, Driver, Guardrails, Outcome, OutcomeState};
    use sprag_terminal::Workspace;
    use std::sync::{Arc, Mutex};

    /// ⚠⚠⚠ **`Dialogue::driving()` ANSWERS `None`, AND THIS IS WHY IT IS ALLOWED TO** — driven,
    /// not read.
    ///
    /// Every other plugin names the pane it is driving so a cut-short run can stop the work. This
    /// one answers nothing, on the claim that it OWNS its per-turn pane and closes it on every exit
    /// path — so the work is ended by a stronger route than a signal. **That claim was written from
    /// reading the code**, which is the shape this workspace has been wrong about before: R335 wrote
    /// seven exemptions from reading and driving them refuted six.
    ///
    /// So it is driven. The endpoint sleeps for five minutes and would outlive any test; the run is
    /// cancelled mid-turn; and the assertion is about the PROCESS TABLE — the pid the turn spawned
    /// is gone afterwards. If the guard were removed, or if closing a pane did not end its process
    /// group, this fails with a five-minute `sleep` still live.
    ///
    /// ⚠ The pid is captured by a WATCHER, from the pane the turn opens, so the gate cannot pass by
    /// having measured a turn that never started.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_cancelled_dialogue_leaves_no_process_behind_although_it_names_no_pane() {
        use crate::access::PaneAccess;
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::time::Instant;

        // The pane's own child IS the `sleep`, so the pid the watcher reads is the work itself.
        let endpoint = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "exec sleep 300".to_string(),
            "_".to_string(),
        ];
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let cancel = Arc::new(AtomicBool::new(false));
        let spawned = Arc::new(AtomicU32::new(0));
        let watcher = {
            let (cancel, spawned) = (Arc::clone(&cancel), Arc::clone(&spawned));
            let workspace = Arc::clone(&workspace);
            std::thread::spawn(move || {
                let watching = WorkspacePaneAccess::new(workspace);
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(20) {
                    let seen = (&watching as &dyn PaneAccess)
                        .foreground_job()
                        .and_then(|jobs| {
                            (&watching as &dyn PaneAccess)
                                .pane_ids()
                                .into_iter()
                                .find_map(|id| jobs.pane_foreground_leader(id))
                        });
                    if let Some(job) = seen.filter(|job| job.name == "sleep") {
                        spawned.store(job.pid, Ordering::Release);
                        cancel.store(true, Ordering::Release);
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // ⚠ Cancel anyway so a broken fixture cannot hang the suite for five minutes; the
                // assertion below is what reports that nothing was ever seen.
                cancel.store(true, Ordering::Release);
                false
            })
        };

        let mut spec = DialogueSpec::new(endpoint.clone(), endpoint, "go");
        spec.timeout = Duration::from_secs(300);
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: None,
        })
        .run(
            &mut Dialogue::new(spec),
            &access,
            &RunContext::new(Arc::clone(&cancel)),
        );

        assert!(
            watcher.join().expect("the watcher thread"),
            "no turn ever reached a running peer, so this measured nothing",
        );
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        let pid = spawned.load(Ordering::Acquire);
        assert!(pid > 0, "the watcher recorded the turn's own process");

        let still_live = || sprag_terminal::procfs::pids_named("sleep").contains(&pid);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) && still_live() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !still_live(),
            "⚠⚠ THE CLAIM: this plugin names no pane BECAUSE it ends its own — and pid {pid} is \
             still running, so it does not",
        );
        // ⚠ AND THE OUTCOME SAYS SO in the vocabulary the other plugins share: a run that had
        // nothing of its OWN left to stop, which is true here only because the guard got there
        // first. A reader must not be told the work was signalled when it was closed.
        assert_eq!(outcome.stopped, Some(crate::driver::Stopped::Nothing));
    }

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

    /// A one-shot fake that prints a `--output-format json` envelope with `usage`
    /// (real tokens) but NO `result` field — a cleanly-parseable yet result-less
    /// reply (reachable: a turn that billed tokens but returned no message). The
    /// decoder must record an EMPTY turn here, not the raw envelope (whose
    /// `usage`/`session_id` would otherwise leak into the transcript).
    fn usage_only_fake(input: u64, output: u64) -> Vec<String> {
        let json = format!(r#"{{"usage":{{"input_tokens":{input},"output_tokens":{output}}}}}"#);
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

    /// Drive `spec` to a terminal outcome under `guardrails`, returning the
    /// workspace (to assert no leaked panes), the outcome, and the transcript.
    fn run_with(
        spec: DialogueSpec,
        guardrails: Guardrails,
    ) -> (Arc<Mutex<Workspace>>, Outcome, Option<String>) {
        let workspace = Arc::new(Mutex::new(Workspace::new((spec.cols, spec.rows))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut dialogue = Dialogue::new(spec);
        let outcome =
            Driver::new(guardrails).run(&mut dialogue, &access, &RunContext::uncancellable());
        let transcript = dialogue.captured();
        (workspace, outcome, transcript)
    }

    /// The common case: bound only by `max_turns` iterations (cost unbounded).
    fn run(spec: DialogueSpec, max_turns: u32) -> (Arc<Mutex<Workspace>>, Outcome, Option<String>) {
        run_with(
            spec,
            Guardrails {
                max_iterations: max_turns,
                max_cost: None,
                max_duration: None,
            },
        )
    }

    /// ⚠⚠⚠ **A REPLY LONGER THAN THE PANE CAME BACK WITH A NEWLINE IN THE MIDDLE OF A SENTENCE.**
    ///
    /// A per-turn pane is narrow, and an endpoint's plain-text answer is prose. Read through the
    /// RENDERED grid, every wrap became a `\n` and the space at each break was trimmed away — so
    /// what this published as the endpoint's words was not what the endpoint said, and the next
    /// turn passed the corrupted text back as history.
    ///
    /// ⚠⚠ The hazard was KNOWN: [`decode_json`]'s doc, one function below the capture, says the
    /// grid *"would corrupt the wrapped single-line envelope (it inserts a `\n` at every wrap and
    /// trailing-trims each row)"* and reads raw bytes to avoid it. **The plain-text path beside it
    /// was never moved.** A fix applied to one consumer is not a fix.
    ///
    /// The fixture's reply is 31 characters into a 16-column pane. Measured with the grid read in
    /// place, the capture was `"the build is don\ne and it passed"` — **a newline inside a word**,
    /// because a wrap falls where the width runs out and not where the language does.
    #[test]
    fn a_reply_wider_than_the_pane_is_captured_as_the_endpoint_said_it() {
        let said = "the build is done and it passed";
        let endpoint = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("printf '%s' '{said}'"),
            "_".to_string(),
        ];
        let mut spec = DialogueSpec::new(endpoint.clone(), endpoint, "go");
        spec.cols = 16;
        spec.rows = 6;

        let (_ws, _outcome, transcript) = run(spec, 1);

        let t = transcript.expect("a transcript");
        assert!(
            t.contains(said),
            "⚠⚠ THE ENDPOINT'S OWN SENTENCE, whole. Through the grid this came back broken at \
             every wrap with the space at each break eaten, and that text is what the next turn \
             was handed as history: {t:?}",
        );
    }

    #[test]
    fn accumulates_full_history_each_turn() {
        // Each turn's prompt is one line longer than the last (the accumulating
        // history), so the counts strictly increase — and the labels alternate.
        let spec = DialogueSpec::new(count_fake(), count_fake(), "count upward");
        let (_ws, outcome, transcript) = run(spec, 3);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        assert_eq!(outcome.iterations, 3);
        let t = transcript.expect("a transcript");
        // Labels alternate, and the reported line-counts strictly increase
        // (each turn's prompt carries one more transcript block) — proof the
        // whole history is passed each turn. The assertion is format-robust: it
        // checks the trend, not exact counts.
        assert!(
            t.contains("A: saw") && t.contains("B: saw"),
            "labels alternate: {t:?}"
        );
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

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
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
        // billed token count.
        let result = "the quick brown fox ".repeat(8); // 160 chars, many spaces
        let mut spec =
            DialogueSpec::new(json_fake(&result, 30, 20), json_fake(&result, 30, 20), "go");
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        spec.endpoints[1].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 2);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        // Two turns × (30 + 20) tokens — proof the Driver accumulates real
        // tokens (a byte count would differ from the render_prompt length).
        assert_eq!(
            outcome.cost,
            Some(Cost::Tokens(100)),
            "cost must be the summed real tokens"
        );
        let t = transcript.expect("a transcript");
        // The full wide result survives verbatim; a grid read would have dropped
        // interior spaces at wrap boundaries (e.g. "fox the" -> "foxthe").
        assert!(t.contains(result.trim()), "faithful reply missing: {t:?}");
    }

    #[test]
    fn claude_json_cost_binds_the_guardrail_in_tokens() {
        // One turn spends 50 tokens, hitting a 50-token budget — so a
        // 100-iteration run still stops after one turn. Proves tokens (not
        // iterations) bound the run.
        let mut spec = DialogueSpec::new(json_fake("hi", 30, 20), json_fake("hi", 30, 20), "go");
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        spec.endpoints[1].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, _t) = run_with(
            spec,
            Guardrails {
                max_iterations: 100,
                max_cost: Some(Cost::Tokens(50)),
                max_duration: None,
            },
        );

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Cost));
        assert_eq!(
            outcome.iterations, 1,
            "one turn must exhaust the 50-token budget"
        );
        assert_eq!(outcome.cost, Some(Cost::Tokens(50)));
    }

    #[test]
    fn claude_json_degrades_on_unparsable_reply() {
        // The endpoint declares ClaudeJson but emits plain text (a misconfig or
        // a truncated envelope): the turn must NOT fail — it records the raw
        // text and bills no tokens (Tokens(0)), keeping the conversation alive.
        let garbage = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf 'just plain text, not json'".to_string(),
            "_".to_string(),
        ];
        let mut spec = DialogueSpec::new(garbage, vec!["true".to_string()], "go");
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 1);

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Iterations),
            "must not Fail on bad JSON"
        );
        let t = transcript.expect("a transcript");
        assert!(
            t.contains("just plain text"),
            "raw fallback text missing: {t:?}"
        );
        // A degraded ClaudeJson turn has no measured tokens (Tokens(0)); the
        // iteration budget — not cost — is the liveness guarantee, so the run
        // still Exhausts. (The retired byte proxy used to bill bytes here.)
        assert_eq!(
            outcome.cost,
            Some(Cost::Tokens(0)),
            "a degraded turn bills no tokens"
        );
    }

    #[test]
    fn clean_empty_result_records_an_empty_turn_not_the_envelope() {
        // A's reply parses cleanly but carries `usage` and NO `result`. The turn
        // must be recorded EMPTY — so the JSON envelope's usage/session_id never
        // leak into the transcript or the next prompt — while the real tokens are
        // still billed. (Before the fix, the whole envelope was recorded as the
        // turn text.)
        let mut spec = DialogueSpec::new(usage_only_fake(7, 3), vec!["true".to_string()], "go");
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 1);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        // Real usage tokens (7 + 3) still bill the cost, even with no result text.
        assert_eq!(
            outcome.cost,
            Some(Cost::Tokens(10)),
            "real usage tokens must still bind the cost"
        );
        let t = transcript.expect("a transcript");
        assert!(
            !t.contains("usage"),
            "envelope leaked into transcript: {t:?}"
        );
        assert!(
            !t.contains("input_tokens"),
            "envelope leaked into transcript: {t:?}"
        );
        // The recorded turn is empty — just the speaker label.
        assert_eq!(
            t.trim(),
            "A:",
            "empty result should record an empty turn: {t:?}"
        );
    }

    #[test]
    fn claude_json_resumes_after_the_first_turn() {
        // Both sides self-report whether their argv carried --resume. A side's
        // FIRST turn has no session yet (fresh whole-history path, no --resume);
        // its LATER turn resumes the session id it stored — proving the resume
        // seam fires on turn 2+ of each side, never turn 1. Network-free.
        let mut spec = DialogueSpec::new(resume_probe_fake(), resume_probe_fake(), "go");
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        spec.endpoints[1].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        let t = transcript.expect("a transcript");
        let a_fresh = t.find("A: fresh").expect("A's first turn must be fresh");
        let a_resumed = t.find("A: resumed").expect("A's later turn must resume");
        let b_fresh = t.find("B: fresh").expect("B's first turn must be fresh");
        let b_resumed = t.find("B: resumed").expect("B's later turn must resume");
        assert!(
            a_fresh < a_resumed,
            "A resumed before its fresh turn: {t:?}"
        );
        assert!(
            b_fresh < b_resumed,
            "B resumed before its fresh turn: {t:?}"
        );
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
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        spec.endpoints[1].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 5);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        let t = transcript.expect("a transcript");
        assert_eq!(
            t.matches("A: ok").count(),
            2,
            "A must parse cleanly on turn 0 AND again on turn 4 (self-healed): {t:?}"
        );
        assert!(
            t.contains("A: GARBAGE"),
            "turn 2's bad resume must be recorded raw: {t:?}"
        );
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

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
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
                max_cost: None,
                max_duration: None,
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
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
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
            max_cost: None,
            max_duration: None,
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
        // cost is the real billed tokens, and that --resume
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
        spec.endpoints[0].format = ReplyFormat::ClaudeJson;
        spec.endpoints[1].format = ReplyFormat::ClaudeJson;
        let (_ws, outcome, transcript) = run(spec, 4);

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        // Real token cost accumulated over the turns (the round's whole point).
        eprintln!(
            "two_real_claudes_converse: real token cost = {:?}",
            outcome.cost
        );
        assert!(
            matches!(outcome.cost, Some(Cost::Tokens(n)) if n > 0),
            "expected real token cost, got {:?}",
            outcome.cost
        );
        let t = transcript.expect("a transcript");
        assert!(!t.trim().is_empty(), "transcript: {t:?}");
        // JSON parse succeeded (not degraded to raw): the transcript holds the
        // clean `result` text, so it must not carry the raw envelope's fields.
        assert!(
            !t.contains("input_tokens"),
            "transcript leaked the raw envelope: {t:?}"
        );
        // Stateful coherence: a counting dialogue surfaces several distinct
        // numbers (a stateless telephone game could not count past the seed).
        let distinct = ["1", "2", "3", "4"]
            .iter()
            .filter(|n| t.contains(**n))
            .count();
        assert!(distinct >= 2, "expected a coherent count, got: {t:?}");
    }
}
