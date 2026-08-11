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
//! ⚠⚠ This paragraph has now been WRONG TWICE, which is what a limitations note left to age looks
//! like. It said *"the projection has no scrollback yet"* years after `sprag-vt` retained history;
//! corrected, it then said the delta was *"still row-keyed ([`RowTrail`]), repaint-proof but not
//! scroll-proof"* while the code beside it already addressed the reply by LINE NUMBER. Both
//! readings survived because nothing drives a doc.
//!
//! What is true, and gated: the reply is the pane's LOGICAL LINES since the prompt's address, with
//! this run's own cooked-mode echo removed by exact match (`without_own_echo`) and with lines the
//! retained history evicted REPORTED as a count rather than dropped. The remaining residue is named
//! on those two items and nowhere else.

use std::time::Duration;

use sprag_input::Modifiers;
use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError, RowTrail};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness, ReadyWhen};
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
    /// What the pane must SHOW before the prompt is injected — see [`Readiness`].
    /// `None` prompts immediately, which is right for a pane already running the
    /// tool.
    ///
    /// # ⚠⚠ Why this adapter needs it MOST
    ///
    /// The pane is the CALLER'S, and this plugin types a prompt into it and then
    /// hands whatever came back to a peer as *the agent's reply*. Prompted while
    /// the pane is still a shell, the shell runs the prompt as a command AND the
    /// trailing Ctrl-D ([`eof`](Self::eof)) makes it EXIT — which is exactly the
    /// completion signal this adapter waits for. So the run CONVERGES, reports
    /// success, and publishes the shell's error as the model's answer. Measured:
    /// a prompt of *"summarise the repo"* came back as
    /// `"summarise the repo\n$ sh: 1: summarise: not found\n$"`, with nothing in
    /// the outcome, the cost or the note to say it was not a reply.
    pub ready_when: Option<ReadyWhen>,
    /// How long to wait for [`ready_when`](Self::ready_when), or `None` for
    /// [`DEFAULT_READY_TIMEOUT`](crate::readiness::DEFAULT_READY_TIMEOUT).
    pub ready_within: Option<Duration>,
}

impl AgentSpec {
    /// A spec with the default one-shot behaviour (send EOF, generous timeout).
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            eof: true,
            timeout: DEFAULT_REPLY_TIMEOUT,
            ready_when: None,
            ready_within: None,
        }
    }
}

/// Where a turn's reply starts — an ADDRESS when the host can number its lines, and a mark on the
/// rendering when it cannot. See [`Agent::capture`] for what the difference costs.
enum Baseline {
    /// The absolute line number the reply begins at.
    Line(u64),
    /// What the rows held before the prompt — the degradation.
    Rows(RowTrail),
}

/// A one-shot AI-tool adapter over one pane.
pub struct Agent {
    pane: PaneId,
    spec: AgentSpec,
    /// The reply captured this run, surfaced through [`Plugin::captured`].
    response: Option<String>,
    /// The barrier the pane must clear before it is prompted — see [`Readiness`].
    ready: Readiness,
}

impl Agent {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: AgentSpec) -> Self {
        Self {
            ready: Readiness::new(spec.ready_when.clone(), spec.ready_within),
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

    /// Capture what the pane has produced since `baseline` — the reply region — joined as the
    /// response text.
    ///
    /// # ⚠⚠ Why the reply is addressed by LINE NUMBER
    ///
    /// What this returns is published to the caller AS THE MODEL'S ANSWER, so every way of
    /// mis-reading the pane becomes a lie about what a model said. Two were measured:
    ///
    /// * keyed on each row's DAMAGE GENERATION, a resize stamped every row, so a client merely
    ///   ATTACHING mid-turn made the whole screen — banner, shell prompt and all — come back as
    ///   the reply;
    /// * keyed on row TEXT, that is fixed, but a reply longer than the pane is tall SCROLLS, and
    ///   the rows that left were never in the answer at all. **A truncated reply is worse than a
    ///   missing one, because nothing in it says it is truncated.**
    ///
    /// A LOGICAL LINE is what the tool actually wrote, and numbering those from the pane's birth
    /// makes the baseline an ADDRESS: everything after it is the reply, whether it is still on the
    /// grid or long scrolled into history.
    ///
    /// ⚠ **The unfinished last line is taken here and nowhere else**, because this adapter waits
    /// for the child to EXIT — an unterminated line at EOF is unterminated forever, and a reply
    /// need not end in a newline. On the timeout path it is taken too, which is exactly what that
    /// path's own `PARTIAL` marking already tells the caller.
    ///
    /// ⚠ [`RowTrail`] remains the fallback for a host with no output stream — repaint-proof, not
    /// scroll-proof, and named as a degradation rather than an equivalent.
    fn capture(&self, panes: &dyn PaneAccess, baseline: &Baseline) -> Captured {
        match baseline {
            Baseline::Line(cursor) => {
                let Some(since) = panes
                    .output_lines()
                    .and_then(|stream| stream.pane_lines_since(self.pane, *cursor))
                else {
                    return Captured::default();
                };
                let mut lines = since.lines;
                if !since.partial.is_empty() {
                    lines.push(since.partial);
                }
                Captured {
                    text: without_own_echo(lines, &self.spec.prompt).join("\n"),
                    lost: since.lost,
                }
            }
            Baseline::Rows(trail) => Captured {
                text: without_own_echo(trail.fresh(panes, self.pane), &self.spec.prompt).join("\n"),
                // ⚠ A rendering comparison cannot report a loss it cannot see — a scrolled-away
                // row is simply not there to be counted. `0` here means UNKNOWN, and it is the
                // degradation this fallback is already named as, not a claim of completeness.
                lost: 0,
            },
        }
    }

    /// Where a turn's reply begins.
    ///
    /// Two shapes because the precise one is a CAPABILITY: a host that can number its lines gives
    /// an address that survives a resize and a scroll, and one that cannot is read by comparing its
    /// rendering. See [`Agent::capture`].
    fn mark(&self, panes: &dyn PaneAccess) -> Baseline {
        panes
            .output_lines()
            // ⚠ `u64::MAX` MARKS WITHOUT TAKING: it is past every line, so nothing is yielded and
            // `next` is the address the reply will start at.
            .and_then(|stream| stream.pane_lines_since(self.pane, u64::MAX))
            .map_or_else(
                || Baseline::Rows(RowTrail::mark(panes, self.pane)),
                |since| Baseline::Line(since.next),
            )
    }
}

/// The reply, and what could not be in it.
///
/// Two fields because a run that answers `converged` with an *"n-character reply"* says the same
/// thing whether the pane's retained history held every line or evicted the first half of the
/// model's answer. **A truncated reply is worse than a missing one, because nothing in it says it
/// is truncated** — this adapter's own doc argued exactly that about the scrolling case and then
/// discarded the field that reports it, while [`crate::pipe`], reading the same stream, put its
/// loss in every note. One reader of a hazard is not a reader of it.
#[derive(Default)]
struct Captured {
    /// The reply as it is published to the caller.
    text: String,
    /// Complete lines the retained history evicted before this capture read them — `0` in the
    /// ordinary case. See [`sprag_vt::LinesSince::lost`].
    lost: u64,
}

/// Drop the leading lines that are exactly the prompt THIS run typed.
///
/// # ⚠⚠ Why the caller's own words came back as the model's
///
/// A pty in cooked mode echoes what is injected, and on the grid that echo is ordinary output — so
/// the first logical line after the prompt's address is the prompt itself. Measured: a run that
/// asked `"summarise the repo"` published `"summarise the repo\nREPLY[summarise the repo]"` to its
/// caller **as the model's answer**. A peer that acts on what it receives acts on a sentence sprag
/// typed.
///
/// # ⚠⚠ EXACT and LEADING, and it stops at the first line that is neither
///
/// The alternative — waiting for the echo and marking after it — is the scheduling-shaped predicate
/// R359c paid to remove: a pty echo is asynchronous, so the same call would strip it or not
/// depending on how loaded the box was. What this run TYPED is known exactly, so the comparison is
/// exact, and the failure direction is chosen: a program that renders input its own way (`> ping`,
/// a REPL's re-draw) matches nothing and keeps every line. **Deleting a line of an answer is worse
/// than leaving a line that was not one**, because only the first is unrecoverable.
///
/// ⚠ The residue, named: a program with its echo OFF whose reply's first line is byte-identical to
/// the prompt loses that line. It needs the echo to be absent AND the model to open by quoting the
/// question exactly, and the safe reading of the pair does not exist — one of them has to lose.
fn without_own_echo(lines: Vec<String>, prompt: &str) -> Vec<String> {
    let mut lines = lines.into_iter();
    let mut kept: Vec<String> = Vec::new();
    let mut echo = prompt.split('\n').peekable();
    for line in lines.by_ref() {
        if echo.peek() == Some(&line.as_str()) {
            echo.next();
            continue;
        }
        kept.push(line);
        break;
    }
    kept.extend(lines);
    kept
}

impl Plugin for Agent {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // ⚠⚠ NOT ONE BYTE UNTIL THE PANE IS THE TOOL — see [`AgentSpec::ready_when`], which is
        // where the measured failure is written down. Latched, so it costs nothing after the first
        // step (and this adapter is one-shot anyway).
        if self.ready.reached(panes, self.pane, run)? == Reached::RunEnded {
            return Ok(Step::new(Cost::Bytes(0), Verdict::Continue).noting(
                "the run ended while waiting for the pane to be ready; nothing was asked",
            ));
        }

        // Baseline before acting, so `capture` isolates this prompt's reply (and its cooked-mode
        // echo) from prior content.
        let baseline = self.mark(panes);

        let cost = panes.inject(self.pane, &self.prompt_keys())?.bytes();

        // If the RUN ended mid-wait — cancelled, or out of time — don't converge
        // or record a partial reply. Return Continue so the Driver's loop top
        // decides the terminal state, which is the only place that knows whether
        // it was a cancel or the duration ceiling.

        let waited = self.await_reply(panes, run);
        // If the RUN ended mid-wait — cancelled, or out of time — don't converge
        // or record a partial reply. Return Continue so the Driver's loop top
        // decides the terminal state, which is the only place that knows whether
        // it was a cancel or the duration ceiling.
        if waited == Waited::Stopped {
            return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                .noting("the run ended while waiting for the reply; nothing captured"));
        }
        let reply = self.capture(panes, &baseline);
        let text = reply.text;
        // ⚠ THE LENGTH IS THE DIAGNOSTIC. A peer that never answered and one that answered are the
        // same `converged` with the same cost, and an EMPTY capture is what a prompt the peer
        // swallowed looks like from out here.
        //
        // ⚠⚠ AND SO IS WHETHER THE PEER FINISHED. This adapter converges on the child EXITING,
        // which is what makes a capture complete; when the per-turn timeout runs out instead, the
        // text is whatever happened to be on screen mid-reply. Both were reported with the same
        // sentence, so a truncated capture was indistinguishable from a whole one.
        let characters = text.chars().count();
        let mut note = if waited == Waited::TimedOut {
            format!(
                "the peer had not finished after {:?}; captured the {characters} characters on \
                 screen, which may be a PARTIAL reply",
                self.spec.timeout,
            )
        } else {
            format!("captured a {characters}-character reply")
        };
        // ⚠⚠ A HOLE IN THE ANSWER IS REPORTED, NEVER SWALLOWED. The pane's retained history is
        // bounded, so a reply that outran it between the prompt and the read has lines nothing can
        // recover — and a silent gap is indistinguishable from a model that said less. This is the
        // half [`crate::pipe`] already reported and this adapter, whose text is published AS THE
        // MODEL'S ANSWER, dropped.
        if reply.lost > 0 {
            note.push_str(&format!(
                "; {} EARLIER LINES ARE MISSING FROM IT — the reply outran the pane's retained \
                 history",
                reply.lost,
            ));
        }
        self.response = Some(text);

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
    use crate::testing::STANDIN_READS_TTY;
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

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
        // ⚠ EQUALITY. This read `contains("REPLY[ping]")` and passed for as long as the capture
        // carried the prompt's own echo welded to its front: a containment check cannot see what a
        // capture has TOO MUCH of, and too much is the shape that publishes sprag's words as a
        // model's.
        assert_eq!(agent.captured().expect("a captured reply"), "REPLY[ping]",);
    }

    /// ⚠⚠⚠ **WHAT THIS RUN TYPED IS NOT WHAT THE MODEL SAID**, and it was published as if it were.
    ///
    /// A pty in cooked mode echoes an injection, and on the grid that echo is ordinary output — so
    /// the first logical line after the prompt's address is the prompt. Measured before the fix:
    /// `"summarise the repo\nREPLY[summarise the repo]"` reached the caller as the agent's answer.
    /// A relay hands that to a peer that ACTS on what it receives, so sprag's own words become an
    /// instruction somebody follows.
    ///
    /// ⚠ EQUALITY, not `contains`. The gate beside this one asserted `contains("REPLY[ping]")` and
    /// passed throughout — a capture with the prompt welded to its front contains the reply too.
    /// **A containment check cannot see what a capture has too much of.**
    #[test]
    fn a_reply_is_what_the_peer_said_and_not_the_prompt_this_run_typed() {
        let (access, pane) = sh_access("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let mut agent = Agent::new(pane, AgentSpec::new("summarise the repo"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        assert_eq!(
            agent.captured().expect("a captured reply"),
            "REPLY[summarise the repo]",
            "the whole capture is the peer's answer — the prompt's own echo is this run's, and \
             publishing it makes sprag's words a model's",
        );
    }

    /// ⚠⚠ **AND A LINE THAT IS NOT THE ECHO IS KEPT, however much it looks like one.**
    ///
    /// The other direction of the same rule, and the one that decides which way the fix fails. A
    /// program with its echo OFF that RENDERS the input its own way — `> ping`, every REPL — must
    /// keep that line: it is the peer's output, and deleting a line of an answer is unrecoverable
    /// while leaving one that was not an answer is merely noise.
    ///
    /// The fixture turns the pty's echo off, so the only text on the pane is the program's.
    #[test]
    fn a_program_that_renders_the_prompt_its_own_way_keeps_that_line() {
        let (access, pane) = sh_access(
            "stty -echo; in=$(cat); echo \"> $in\"; echo \"REPLY[$in]\"",
            40,
            6,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("ping"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        assert_eq!(
            agent.captured().expect("a captured reply"),
            "> ping\nREPLY[ping]",
            "an EXACT leading match is the echo and nothing else is — a program's own rendering \
             of the prompt is output, and stripping it would delete an answer's first line",
        );
    }

    /// ⚠⚠ **A HOLE IN THE ANSWER IS REPORTED** — the field [`crate::pipe`] reads and this adapter
    /// dropped.
    ///
    /// The pane's retained history is bounded, so a reply that outran it has lines nothing can
    /// recover. Both cases answered `converged` with the same *"captured an n-character reply"*,
    /// which makes a truncated model answer indistinguishable from a short one — the exact
    /// confusion this adapter's own `capture` doc argues against, two paragraphs above the code
    /// that discarded `lost`.
    ///
    /// Driven through a stream that REPORTS a loss, because a bounded history cannot be overrun on
    /// demand without making the gate a scrolling fixture rather than a claim about the report.
    #[test]
    fn a_reply_that_outran_the_pane_s_history_says_how_much_is_missing() {
        struct Lossy;
        impl PaneAccess for Lossy {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(true)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                Ok(crate::access::Written::of(1))
            }
            fn output_lines(&self) -> Option<&dyn crate::access::PaneOutputLines> {
                Some(self)
            }
        }
        impl crate::access::PaneOutputLines for Lossy {
            fn pane_lines_since(&self, _id: PaneId, _cursor: u64) -> Option<sprag_vt::LinesSince> {
                Some(sprag_vt::LinesSince {
                    lines: vec!["the tail of the answer".to_string()],
                    next: 10,
                    lost: 7,
                    partial: String::new(),
                })
            }
        }

        let step = Agent::new(PaneId(1), AgentSpec::new("ping"))
            .step(&Lossy, &RunContext::uncancellable())
            .expect("the turn");
        let said = step.note.unwrap_or_default();
        assert!(
            said.contains('7') && said.contains("MISSING"),
            "the turn must say HOW MANY lines of the answer it never saw, or a truncated reply is \
             published as a whole one: {said:?}",
        );
    }

    /// The three cases [`without_own_echo`] decides, without a pty in the way.
    #[test]
    fn an_echo_is_dropped_only_where_it_leads_and_only_where_it_matches() {
        let lines = |all: &[&str]| all.iter().map(|l| (*l).to_string()).collect::<Vec<_>>();

        assert_eq!(
            without_own_echo(lines(&["ask", "answer"]), "ask"),
            lines(&["answer"]),
            "the leading echo of a one-line prompt",
        );
        assert_eq!(
            without_own_echo(lines(&["one", "two", "answer"]), "one\ntwo"),
            lines(&["answer"]),
            "and of a prompt with newlines in it, line for line",
        );
        assert_eq!(
            without_own_echo(lines(&["answer", "ask"]), "ask"),
            lines(&["answer", "ask"]),
            "⚠⚠ NOT A LINE THAT MERELY EQUALS THE PROMPT — only the LEADING one is the echo, and \
             a model that quotes the question mid-answer is quoting it",
        );
        assert_eq!(
            without_own_echo(lines(&["ask"]), "ask"),
            Vec::<String>::new(),
            "a peer that answered nothing leaves an EMPTY capture, which is the diagnostic the \
             step's character count exists to publish — not a capture of this run's own prompt",
        );
        assert_eq!(
            without_own_echo(lines(&["answer"]), ""),
            lines(&["answer"]),
            "and a prompt with nothing in it consumes nothing",
        );
    }

    /// ⚠⚠ **AN AGENT RUN AGAINST A PANE THAT IS STILL A SHELL MUST NOT REPORT THE SHELL'S OUTPUT
    /// AS THE MODEL'S REPLY** — the worst shape this defect takes, because it is a WRONG ANSWER
    /// rather than a missing one.
    ///
    /// The subject half is what a caller gets today with no barrier, and it is not a hang or a
    /// failure: the shell runs the prompt as a command, the trailing Ctrl-D makes it EXIT, and
    /// exiting is precisely the completion signal this adapter converges on. So the run reports
    /// SUCCESS and hands back `"summarise the repo\n$ sh: 1: summarise: not found\n$"` as the
    /// agent's answer. Nothing in the state, the cost or the note says otherwise.
    ///
    /// With the barrier the run waits for the tool to announce itself and asks the TOOL, so the
    /// captured text is the tool's. Both halves against the same pane script, because the claim is
    /// about the barrier and not about the fixture.
    #[test]
    fn an_agent_waits_for_the_tool_rather_than_prompting_the_shell_that_is_still_there() {
        // A pane that is a shell for a moment and then becomes the "tool": it announces itself and
        // execs a one-shot that reads until EOF and answers. The stand-in shell EATS what it is
        // given (see `STANDIN_READS_TTY`) — an un-eaten prompt would sit in the pty and be read by
        // the tool anyway, and this gate would pass without a barrier.
        let script = format!(
            "while read early; do echo \"SHELL-ATE $early\"; done {STANDIN_READS_TTY} & \
             sleep 1; kill $! 2>/dev/null; printf 'TOOL-UP\\n'; \
             exec sh -c 'in=$(cat); echo \"REPLY[$in]\"'"
        );
        let (access, pane) = sh_access(&script, 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                ..AgentSpec::new("summarise the repo")
            },
        );

        let outcome = Driver::new(Guardrails {
            max_iterations: 4,
            max_cost: None,
            max_duration: Some(Duration::from_secs(20)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().expect("a captured reply");
        assert!(
            captured.contains("REPLY[summarise the repo]"),
            "the reply must be the TOOL's: {captured:?}",
        );
        assert!(
            !captured.contains("not found") && !captured.contains("SHELL-ATE"),
            "and it must carry no trace of the shell that was there first — a caller reading this \
             as the model's answer would be reading a shell error: {captured:?}",
        );
    }

    /// ⚠⚠ **A HOST WITH NO OUTPUT STREAM STILL CAPTURES A REPLY** — the degradation arm, which no
    /// gate built.
    ///
    /// [`PaneAccess::output_lines`] is optional, so a build without it falls back to comparing the
    /// RENDERING ([`RowTrail`]). That fallback is named as a degradation — it cannot see a line
    /// that scrolled away — and a degradation that returned NOTHING would not be one: this adapter
    /// publishes what it captures as the model's answer, so an empty capture is a silent failure
    /// wearing the shape of a reply.
    ///
    /// ⚠ Both halves: the reply comes back, and the text that was on the pane BEFORE the prompt
    /// does not — a fallback that returned the whole screen would pass the first alone.
    #[test]
    fn a_host_with_no_output_stream_still_captures_by_the_rendering() {
        /// A pane that answers when typed at, with every optional capability at its default.
        struct NoStream(Mutex<Vec<String>>);
        impl PaneAccess for NoStream {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(self.0.lock().unwrap().join(""))
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(
                    self.0
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|text| crate::access::PaneRow {
                            generation: 1,
                            text: text.clone(),
                        })
                        .collect(),
                )
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(true)
            }
            fn pane_full_text(&self, id: PaneId) -> Option<String> {
                self.pane_collapsed(id)
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                // ⚠ THE ECHO FIRST, because a pty in cooked mode puts it there before the program
                // has read a byte. A fake that skips it makes the degradation arm look cleaner
                // than the hosts that actually take it — and left `without_own_echo` on this path
                // built by nothing, which is how the same defect comes back through the fallback.
                let mut screen = self.0.lock().unwrap();
                screen.push("ask".to_string());
                screen.push("REPLY-BY-ROWS".to_string());
                Ok(crate::access::Written::of(4))
            }
        }

        let access = NoStream(Mutex::new(vec!["banner".to_string()]));
        let mut agent = Agent::new(PaneId(1), AgentSpec::new("ask"));
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: Some(Duration::from_secs(5)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
        assert_eq!(
            agent.captured().as_deref(),
            Some("REPLY-BY-ROWS"),
            "the fallback must return the reply — and only the reply: `banner` was on the pane \
             before the prompt and `ask` is this run's own echo, and neither is what the model \
             said",
        );
    }

    /// ⚠⚠ **A REPLY LONGER THAN THE PANE IS TALL IS CAPTURED WHOLE** — what only an addressed
    /// reply region can do, and the residue the row-keyed capture carried.
    ///
    /// A capture that compares ROWS can only ever return what is still on the grid. A model whose
    /// answer is longer than the pane pushes its own opening off the top, and the caller is handed
    /// the tail as though it were the whole reply — **a truncated answer, with nothing in it saying
    /// so**. That is worse than a missing one: it reads as complete.
    ///
    /// The fixture makes it certain rather than likely — a TEN-line reply into a FOUR-row pane —
    /// and the last line deliberately ends WITHOUT a newline, because a reply need not end in one
    /// and dropping it would lose the model's last word.
    #[test]
    fn a_reply_that_scrolled_past_the_pane_is_captured_whole() {
        let (access, pane) = sh_access(
            "exec sh -c 'in=$(cat); i=1; while [ $i -le 9 ]; do echo \"R$i[$in]\"; \
             i=$((i+1)); done; printf \"R10[$in]\"'",
            40,
            4,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("ask"));
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");

        let captured = agent.captured().expect("a captured reply");
        assert!(
            !access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .contains("R1[ask]"),
            "⚠ THE CONTROL: the reply's opening must ALREADY be off the four-row grid, or this \
             gate is about a visible reply and measures nothing new",
        );
        for i in 1..=9 {
            assert!(
                captured.contains(&format!("R{i}[ask]")),
                "line {i} of the model's answer must reach the caller — a capture that returns \
                 only what is still on screen hands back a TRUNCATED reply that reads as a \
                 complete one: {captured:?}",
            );
        }
        assert!(
            captured.contains("R10[ask]"),
            "⚠⚠ INCLUDING THE LAST LINE, WHICH HAS NO NEWLINE AFTER IT. A reply need not end in \
             one, and for a one-shot tool that unterminated line is the end of its answer — the \
             child has EXITED, so it is unfinished forever: {captured:?}",
        );
    }

    /// ⚠⚠ **A RESIZE MID-TURN IS NOT THE MODEL SPEAKING** — the worst instance of the
    /// paint-vs-content error, because what this plugin captures is published AS THE MODEL'S REPLY.
    ///
    /// The reply region was *"the rows whose DAMAGE GENERATION moved since the prompt"*, and a
    /// resize (`Screen::reflowed`) stamps every row. A client ATTACHING to the session mid-turn —
    /// the ordinary thing a person does — therefore made **the entire screen, banner and shell
    /// prompt and all, come back to the caller as what the model said**.
    ///
    /// The fixture puts text on screen that the model demonstrably did not produce (`OLD-BANNER`,
    /// printed before the prompt was ever sent), makes the peer slow enough that the resize lands
    /// inside the turn, and asserts the capture is the REPLY and not the screen.
    ///
    /// ⚠ Both halves: the reply is there (so the capture still works at all) and the banner is not
    /// (so it is a reply and not a screenshot).
    #[test]
    fn a_resize_during_a_turn_does_not_become_the_models_reply() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // OLD-BANNER is on screen BEFORE the prompt, and the peer waits a second before
            // answering — so the resize below lands between the baseline and the capture.
            command.arg(
                "printf 'OLD-BANNER\\n'; exec sh -c 'in=$(cat); sleep 1; echo \"REPLY[$in]\"'",
            );
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("OLD-BANNER"))
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut agent = Agent::new(pane, AgentSpec::new("ask"));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // Inside the turn: after the prompt's baseline, before the reply lands.
                std::thread::sleep(Duration::from_millis(300));
                workspace
                    .lock()
                    .unwrap()
                    .resize(pane, 34, 8, (0, 0))
                    .expect("a client attaches, so the pane is re-laid out");
            });
            let outcome = Driver::new(Guardrails {
                max_iterations: 2,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
        });

        let captured = agent.captured().expect("a captured reply");
        assert!(
            captured.contains("REPLY[ask]"),
            "the reply must still be captured — a fix that captured NOTHING would pass the other \
             half for the wrong reason: {captured:?}",
        );
        assert!(
            !captured.contains("OLD-BANNER"),
            "⚠⚠ AND TEXT THE MODEL NEVER PRODUCED MUST NOT BE PUBLISHED AS ITS REPLY — this was \
             on screen before the prompt was sent, and only a REPAINT could have put it in the \
             reply region: {captured:?}",
        );
    }

    /// ⚠⚠ **A PANE THAT NEVER COMES UP FAILS THE ASK AND NAMES WHAT WAS THERE** — this plugin's own
    /// `NeverReady` arm, which had no gate of its own.
    ///
    /// It was registered rather than built, on the reasoning that the other two injecting plugins
    /// build the identical `Readiness` path — which is exactly the argument R351 caught being wrong
    /// when a shared path stopped being shared. **And this plugin is the one where the arm matters
    /// most**: its failure mode is a WRONG ANSWER, not a missing one. Without the barrier it hands
    /// a shell's `command not found` back to a peer AS THE MODEL'S REPLY, so *"the pane never came
    /// up"* must reach the caller as a failure rather than as a captured reply.
    ///
    /// Three halves, and the last is the one the other plugins' gates cannot make: the run FAILED,
    /// the cause is typed and names both the question and what was running instead, and **nothing
    /// was captured** — because a captured anything here would be published as what the model said.
    #[test]
    fn an_agent_whose_pane_never_becomes_ready_fails_and_captures_nothing() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Runs("claude".to_string())),
                ready_within: Some(Duration::from_millis(200)),
                ..AgentSpec::new("summarise the repo")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // ⚠ FAR LONGER than the readiness bound, so the run's own clock provably cannot be
            // what ends this — that is the neighbouring gate, and it reaches a different arm.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane that never became the tool is a FAILURE of the ask: {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Runs("claude".to_string()),
            "cat",
            "and it names the question AND what the pane was running instead",
        );
        assert_eq!(
            agent.captured(),
            None,
            "⚠⚠ AND NOTHING WAS CAPTURED — anything here is published to the caller as THE \
             MODEL'S REPLY, which is this plugin's whole reason for taking the barrier",
        );
    }

    /// ⚠⚠ **A RUN THAT ENDS WHILE WAITING TO BE LET IN ASKS NOTHING AND CHARGES NOTHING** — and
    /// says which of the two it was doing, because "nothing was asked" and "asked, no reply" are
    /// opposite instructions to whoever reads the journal.
    #[test]
    fn an_agent_whose_run_ends_before_the_pane_is_ready_asks_nothing() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Prints(
                    "A MARKER THIS PANE NEVER PRINTS".to_string(),
                )),
                // ⚠ FAR ABOVE the run's clock, so the RUN's deadline is provably what ends the
                // wait rather than the barrier's own bound — that ending is the other arm.
                ready_within: Some(Duration::from_secs(300)),
                ..AgentSpec::new("summarise the repo")
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(200)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Duration));
        let said = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            said.contains("nothing was asked"),
            "the step must say the prompt was never sent, not that a reply never came: {said}",
        );
        assert_eq!(
            outcome.cost,
            Some(Cost::Bytes(0)),
            "nothing was injected, so nothing is charged: {outcome:?}",
        );
        assert!(
            agent.captured().is_none(),
            "and a run that asked nothing has captured no reply: {:?}",
            agent.captured(),
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "and the pane is untouched",
        );
    }

    /// ⚠⚠ **A CAPTURE TAKEN BECAUSE TIME RAN OUT SAYS SO** — it may be half a sentence.
    ///
    /// This adapter converges on the child EXITING, which is what makes a capture complete: every
    /// byte the peer produced is on the screen by then. When the per-turn timeout ends the wait
    /// instead, the text is whatever was on screen mid-reply — and both were reported with the
    /// same sentence, so a truncated capture was indistinguishable from a whole one by anything a
    /// run publishes.
    #[test]
    fn a_capture_taken_because_the_turn_ran_out_of_time_is_marked_as_possibly_partial() {
        // Answers, then stays alive — so the reply IS on screen but EOF never comes.
        let (access, pane) = sh_access("echo PARTIAL-REPLY; exec cat", 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                eof: false,
                timeout: Duration::from_millis(300),
                ..AgentSpec::new("x")
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: Some(Duration::from_secs(20)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "a per-turn timeout still converges with what it has — that behaviour is deliberate \
             and unchanged; what was missing is saying so",
        );
        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let said = notes.join(" | ");
        assert!(
            said.contains("PARTIAL"),
            "a capture the clock cut short must be marked as possibly partial: {said}",
        );
        assert!(
            agent
                .captured()
                .is_some_and(|reply| reply.contains("PARTIAL-REPLY")),
            "and it still captures what the peer did say",
        );
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
