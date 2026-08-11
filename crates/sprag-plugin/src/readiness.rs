//! The readiness barrier — *do not type into a pane whose program has not started yet*.
//!
//! One home, because the three plugins that INJECT need it for the same reason and none of them is
//! the reason.
//!
//! # ⚠⚠ Why a loop needs a barrier at all
//!
//! A pane is not ready when it is OPEN. It is born running a shell with the pseudoterminal's
//! default echo on, and the program a caller means to drive — `claude`, a REPL, a test runner —
//! starts some time after that. Anything injected in that window goes to whatever IS there: the
//! shell EXECUTES it as a command, or a reader that is not the peer eats it. Either way the turns
//! are spent, the guardrails count them, and the peer never saw a word.
//!
//! The remedy cannot be a sleep — how long a program takes to come up is not a number a caller
//! knows, and it is the one thing that varies most between the programs this drives. It is a thing
//! the pane SAYS: a prompt, a banner, a marker the caller printed on purpose. So the caller names
//! it and the run waits for it, bounded by the caller's own `ready_within` and by the run's
//! deadline like every other wait here.
//!
//! # Who takes one, and why the last one needed it most
//!
//! Every plugin that TYPES INTO a pane it did not start takes this barrier; the one that does not
//! type takes none. That is the whole rule, and each half of it was measured rather than argued.
//!
//! * [`Orchestrator`](crate::orchestrator::Orchestrator) drives a pane a caller pointed it at. It
//!   got the barrier first.
//! * [`Pipe`](crate::pipe::Pipe) relays into a pane **somebody else prepared**, which is the whole
//!   shape of a relay — and it had no barrier at all. A relay into a pane that was still a shell
//!   was eaten by that shell (`SHELL-ATE relayme`, twice) while the peer that came up a second
//!   later saw nothing, and every number the run reported matched a working relay's.
//! * [`Agent`](crate::agent::Agent) is the worst case, because its failure is a WRONG ANSWER and
//!   not a missing one. It prompts the caller's pane and hands what comes back to a peer as *the
//!   model's reply*; against a shell, the prompt is run as a command and the trailing Ctrl-D makes
//!   the shell EXIT — which is exactly the completion signal it converges on. Measured:
//!   `"summarise the repo"` came back as `"…$ sh: 1: summarise: not found\n$"`, reported
//!   `converged`.
//! * [`Dialogue`](crate::dialogue::Dialogue) takes NONE, and the absence is a finding too: it
//!   passes each turn's prompt as an ARGV ARGUMENT of the pane it spawns for that turn and never
//!   injects a byte, so there is no window for a shell to be typed into.

use std::time::Duration;

use sprag_detect::AgentState;
use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneDoing, PaneError};
use crate::run::{RunContext, Waited, poll_until};

/// The bound a readiness wait is given when the caller names none.
///
/// ⚠ Deliberately the same number as [`DEFAULT_REPLY_TIMEOUT`], and deliberately its OWN name.
/// The value is shared because two minutes is this crate's one answer to *"long enough for
/// something on the other side to happen"* and inventing a second number nobody chose would be
/// worse. The name is separate because the QUESTIONS differ — one is how long a model may think,
/// the other is how long a program may take to start — and a caller who wants to change one of
/// them almost never means the other.
///
/// [`DEFAULT_REPLY_TIMEOUT`]: crate::run::DEFAULT_REPLY_TIMEOUT
pub const DEFAULT_READY_TIMEOUT: Duration = crate::run::DEFAULT_REPLY_TIMEOUT;

/// How a [`Readiness`] wait ended, for the two endings that are not an error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reached {
    /// The pane is ready. Drive it.
    Yes,
    /// THE RUN ended while waiting — cancelled, or out of time. **Nothing was injected**, so
    /// nothing is charged; which of the two it was is the [`RunContext`]'s to answer.
    RunEnded,
}

/// WHICH QUESTION a readiness marker is asking — the distinction the argument used to hide.
///
/// # ⚠⚠ Why one needle cannot answer both
///
/// A marker match over the pane's text answers *"is this text here?"*, and that same observation
/// means two opposite things depending on what the caller is doing:
///
/// * **They just started the program.** Text that was ALREADY on the screen when the run began is
///   no evidence at all — and the most likely such text is the ECHO OF THE COMMAND LINE THAT
///   STARTED THE PROGRAM, which a pty puts on screen before the program exists. Measured: a pane
///   told to wait for `TOOL-UP` passed the barrier in 50 MILLISECONDS against
///   `…printf "TOOL-UP\n"; exec cat'$ ping ATE ping…` — the run spent both its turns on the shell
///   and the peer never saw a word.
/// * **They are pointing at a program already running.** A REPL sitting at its prompt has that
///   prompt on screen and will print nothing further until it is fed. Here the text already being
///   there is exactly the evidence, and demanding NEW output would wait forever.
///
/// These are not degrees of the same evidence — they are different KINDS, in the sense
/// [`Authority`](crate::access::Authority) means it, and nothing in the marker itself says which.
/// **Only the caller knows**, so the type makes them say. A single string could not, which is why
/// the wire value changed shape rather than gaining a default nobody chose.
///
/// ⚠ Whichever they pick, a marker that cannot appear in a command line (a prompt, `>>> `) is
/// safer than a word they typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyWhen {
    /// Ready once the marker appears in output the pane produces **after the barrier arms**.
    ///
    /// The answer for *"I just started it"*: the barrier counts how many times the marker is
    /// already on the collapsed screen when it arms, and clears only once there are MORE. Wrap-safe
    /// — the screen is joined the way [`pane_collapsed`](crate::access::PaneAccess::pane_collapsed)
    /// joins it, so a marker the pane wrapped is one occurrence at any width.
    ///
    /// ⚠⚠ **A COUNT, and not each row's DAMAGE GENERATION, which is what this was.** A damage
    /// generation is a PAINT signal — it tells a renderer which rows to redraw — and a RESIZE
    /// (`Screen::reflowed`) or an OSC PALETTE change (`mark_all_dirty`) stamps every row with a
    /// fresh one while no program prints anything. Measured: a pane whose screen already carried
    /// the marker cleared this barrier **the instant anybody resized**, which in a terminal
    /// multiplexer is what every attaching client does.
    ///
    /// ⚠ The residue: text scrolling OFF lowers the count, so a marker that was on screen, scrolled
    /// away and was then printed afresh may not exceed its baseline. A false NEGATIVE — the safe
    /// direction — and [`Runs`](Self::Runs) has no such arithmetic.
    ///
    /// # ⚠⚠ AND THE PANE'S OWN ECHO IS DISCOUNTED, whenever it lands
    ///
    /// The generation baseline alone left a RACE, and it was measured rather than reasoned about: a
    /// pty echo is asynchronous, so a caller who writes the starting command and begins the run in
    /// the same breath can have the echo reach the grid AFTER the barrier armed. It is then new
    /// output by every test a screen can apply — and the barrier cleared on it, spending the run's
    /// turns on the shell that was still there. **The same call converged or drove into a shell
    /// depending on scheduling.**
    ///
    /// So the pane REMEMBERS what was written into it
    /// ([`PaneInputEcho`](crate::access::PaneInputEcho)), and a marker found in that text is
    /// refused as evidence outright. The race is not narrowed, it is REMOVED: the answer is the
    /// same whichever side of the arming the echo falls on, and it no longer depends on how loaded
    /// the machine is.
    ///
    /// ⚠ **A MARKER THAT APPEARS IN WHAT YOU TYPED CAN THEREFORE NEVER BE EVIDENCE**, and the run
    /// ends [`NeverReady`](crate::access::PaneError::NeverReady) naming it rather than driving
    /// something that was never listening. That is the honest answer to an ambiguous marker, and it
    /// is deterministic — which the alternative was not. Pick a marker the program prints, or open
    /// the pane running the program (`open_pane`'s `cmd`) so there is no echo at all.
    Prints(String),
    /// Ready once the marker is on the screen **now**, wherever it came from.
    ///
    /// The answer for *"it is already running"* — a REPL at its prompt, which will print nothing
    /// more until it is fed. This is the older behaviour, and it is the one that can be satisfied
    /// by an echo, so it is opt-in rather than the default.
    Shows(String),
    /// Ready once the program named here OWNS THE PANE'S TERMINAL. **Prefer this one.**
    ///
    /// # ⚠⚠ The only kind that does not read the screen, and the only one a silent program has
    ///
    /// The two kinds above are predicates over TEXT, and a program that prints nothing on startup
    /// emits no text to predicate over. There is no marker to name for `cat`, for a REPL launched
    /// `--quiet`, for a relay that speaks only when spoken to — so for that whole class the barrier
    /// had no usable answer at all, and a caller's realistic options were to guess a sleep or to
    /// drive a pane that might still be a shell.
    ///
    /// This asks the operating system instead. A shell hands its terminal to the job it runs and
    /// takes it back when that job ends; the pane's terminal therefore NAMES what is running in it,
    /// with no screen involved. That fact:
    ///
    /// * **cannot be echoed** — it is not text, so no amount of typing the word can satisfy it, and
    ///   the whole echo hazard the two kinds above spend their documentation on does not arise;
    /// * **does not depend on scheduling** — it is a state, not an event that may or may not have
    ///   landed on a grid before the barrier armed;
    /// * **works for a program that never prints**, which is the case that has no other answer;
    /// * **is the same value either way a pane was made** — a pane opened running the program
    ///   (`open_pane`'s `cmd`) matches from birth, and a shell typed into matches when the program
    ///   starts. One spelling, both shapes.
    ///
    /// # What the name is matched against
    ///
    /// The job LEADER's kernel name, or the basename of its `argv[0]` — either, because the two
    /// honestly disagree and a caller should not have to know which. `exec awk …` on a Debian box
    /// is a leader whose kernel name is `mawk` and whose `argv[0]` is `awk`; a program that rewrote
    /// its own argv still has its kernel name; a name longer than 15 bytes is truncated in the
    /// kernel's copy and intact in `argv[0]`.
    ///
    /// Matched EXACTLY, never as a prefix: a prefix match is a silent merge, and `claude` matching
    /// `claude-relay` is a run driving the wrong program while reporting success.
    ///
    /// ⚠ A leader is not every process of the job — `cargo build | less` is led by `cargo`. See
    /// [`foreground_leader_of`](sprag_terminal::foreground_leader_of).
    ///
    /// ⚠ On a host that cannot see the process table
    /// ([`PaneAccess::foreground_job`] is `None`) this
    /// is never satisfied, and the run ends [`NeverReady`](crate::access::PaneError::NeverReady)
    /// rather than typing into whatever is there.
    Runs(String),
    /// Ready once the AGENT named here is **at rest and waiting for input**. The strongest of the
    /// four, and the one to prefer when the pane runs an agent this host can supervise.
    ///
    /// # ⚠⚠ Owning the terminal is not the same as listening
    ///
    /// [`Runs`](Self::Runs) clears the moment a program takes the pane's terminal, which for a
    /// cold agent is seconds before it will answer anything: the model is still starting, and a
    /// prompt sent into that window is typed at something that has not finished reading its own
    /// configuration. `Runs` is the honest answer to *has it started*, and callers kept needing the
    /// next question — *has it started AND stopped again, waiting for me*.
    ///
    /// One condition answers both halves, because an
    /// [`AgentObservation`](crate::access::AgentObservation) carries WHICH agent alongside what it
    /// is doing. So this is not a composition of two barriers; it is the barrier the supervisor was
    /// already able to answer and nothing asked it for.
    ///
    /// # What counts as ready, and what deliberately does not
    ///
    /// [`Idle`](sprag_detect::AgentState::Idle) — *at rest, waiting for input it has not asked
    /// for* — and nothing else.
    ///
    /// * **`Working` is not ready**, which is the entire point: it is the state `Runs` cannot
    ///   distinguish from readiness.
    /// * ⚠ **`Blocked` is not ready either, and that is a decision rather than an omission.** A
    ///   blocked agent is waiting for an ANSWER TO ITS OWN QUESTION, and a fresh prompt sent there
    ///   answers the wrong thing — often into a numbered menu, where it selects. A caller who means
    ///   to answer a blocked agent is supervising it, not waiting to start.
    /// * **An observation that names no agent is not ready**, however idle it looks: two panes at
    ///   rest are not evidence about WHICH program is at rest, and this kind's whole value is that
    ///   it names one.
    ///
    /// ⚠ **The evidence may be a SCREEN RULE rather than the agent's own report**
    /// ([`Authority`](crate::access::Authority)), and this accepts either. That is a real trade and
    /// not an oversight: a scraped answer is APPROXIMATE, but it is deterministic — it is not the
    /// scheduling-dependent ambiguity the other kinds' documentation is about — and refusing it
    /// would leave a caller whose agent does not self-report with no way to ask this question at
    /// all. [`Runs`](Self::Runs) is the exact-but-weaker alternative, and it is one word away.
    ///
    /// ⚠ On a host with no detector at all ([`PaneAccess::supervision`] is `None`) this is never
    /// satisfied, on the same terms as [`Runs`](Self::Runs).
    Settles(String),
}

impl ReadyWhen {
    /// The two words a caller may spell, in this type's own order.
    ///
    /// Published to every mouth from here rather than retyped as literals, so a third kind reaches
    /// the wire in the compile that adds it.
    pub const WIRE_WORDS: &'static [&'static str] = &["prints", "shows", "runs", "settles"];

    /// The kind named by `word`, or `None` for a word outside the closed set.
    ///
    /// ⚠ A caller who sends something else has made a MALFORMED request, not a rejected one —
    /// R353's rule, and the reason this returns an `Option` for the parser to turn into the wire's
    /// own grammar refusal rather than a friendly sentence.
    ///
    /// ⚠⚠ **AN EMPTY MARKER IS REFUSED**, because it is a different wrong answer in each kind and
    /// none of them is what a caller meant: `""` is on every screen, so [`Shows`](Self::Shows)
    /// clears instantly and the barrier is a no-op that LOOKS like a barrier; no process is named
    /// `""`, so [`Runs`](Self::Runs) can never clear; and counting occurrences of `""` is
    /// arithmetic on nothing. The type is `String` and the argument admits fewer values than the
    /// type — R352's shape, and the fix is one predicate the parser and the publication share.
    #[must_use]
    pub fn parse(word: &str, marker: String) -> Option<Self> {
        if marker.is_empty() {
            return None;
        }
        match word {
            "prints" => Some(Self::Prints(marker)),
            "shows" => Some(Self::Shows(marker)),
            "runs" => Some(Self::Runs(marker)),
            "settles" => Some(Self::Settles(marker)),
            _ => None,
        }
    }

    /// The word this kind is spelled as on the wire.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Prints(_) => "prints",
            Self::Shows(_) => "shows",
            Self::Runs(_) => "runs",
            Self::Settles(_) => "settles",
        }
    }

    /// The text the pane must carry — or, for [`Runs`](Self::Runs), the program it must be running.
    #[must_use]
    pub fn marker(&self) -> &str {
        match self {
            Self::Prints(marker)
            | Self::Shows(marker)
            | Self::Runs(marker)
            | Self::Settles(marker) => marker,
        }
    }

    /// This question as the PAST-TENSE clause of a sentence about a pane that never answered it —
    /// `printed "UP"`, `showed ">>> "`, `ran "claude"`.
    ///
    /// ⚠ The reason [`PaneError::NeverReady`] carries the
    /// whole kind rather than the marker alone. Its sentence is what an agent reads when a run
    /// fails, and *"the pane never showed `claude`"* is false of two of the three kinds — one waits
    /// for text to be PRINTED and one for a program to be RUNNING, neither of which is showing.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Prints(marker) => format!("printed {marker:?}"),
            Self::Shows(marker) => format!("showed {marker:?}"),
            Self::Runs(name) => format!("ran {name:?}"),
            Self::Settles(agent) => format!("settled as {agent:?}, at rest and waiting for input"),
        }
    }
}

/// A pane's readiness barrier: what it must show before a plugin types into it, which question
/// that is asking ([`ReadyWhen`]), and how long the plugin will wait.
///
/// Latched — the wait happens once and every later step drives straight away, so a run pays for
/// this on its first step only.
#[derive(Clone, Debug)]
pub struct Readiness {
    /// What the pane must show, and which question it answers. `None` starts driving immediately,
    /// which is right for a pane the caller knows is already running the program.
    when: Option<ReadyWhen>,
    /// How long to wait for it. See [`DEFAULT_READY_TIMEOUT`] for why this is the caller's.
    within: Duration,
    /// Every row's damage generation when this barrier ARMED, for [`ReadyWhen::Prints`].
    ///
    /// How many times the marker was ALREADY on the collapsed screen when this barrier armed, for
    /// [`ReadyWhen::Prints`].
    ///
    /// Captured on the first look rather than at construction, because that is the moment the
    /// question is first asked and the only one a `PaneAccess` is in hand for. `None` until then.
    ///
    /// ⚠ A COUNT, and not each row's damage generation, which is what this was. See the comment in
    /// [`satisfied`](Self::satisfied): a generation says a row was REPAINTED, and a resize repaints
    /// every one of them.
    armed_at: Option<usize>,
    /// Whether the marker has been seen. Latched.
    seen: bool,
}

impl Readiness {
    /// A barrier for `when`, waiting `within` (defaulting to [`DEFAULT_READY_TIMEOUT`]).
    ///
    /// A `None` condition is a barrier that is already down: the caller is saying the pane is
    /// running what they mean to drive.
    #[must_use]
    pub fn new(when: Option<ReadyWhen>, within: Option<Duration>) -> Self {
        Self {
            seen: when.is_none(),
            when,
            within: within.unwrap_or(DEFAULT_READY_TIMEOUT),
            armed_at: None,
        }
    }

    /// Whether `pane` satisfies `when` right now.
    fn satisfied(&self, when: &ReadyWhen, panes: &dyn PaneAccess, pane: PaneId) -> bool {
        match when {
            ReadyWhen::Runs(name) => panes
                .foreground_job()
                .and_then(|jobs| jobs.pane_foreground_leader(pane))
                .is_some_and(|leader| leader_is_named(&leader, name)),
            // ⚠⚠ IDLE AND NAMED, both halves from ONE observation. `Working` is the state `Runs`
            // cannot tell from readiness, which is why this kind exists; `Blocked` is waiting for
            // an answer to its OWN question, where a fresh prompt answers the wrong thing; and an
            // observation naming no agent is not evidence about WHICH program is at rest.
            ReadyWhen::Settles(agent) => panes
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(pane))
                .is_some_and(|seen| {
                    seen.state == AgentState::Idle && seen.agent.as_deref() == Some(agent.as_str())
                }),
            ReadyWhen::Shows(marker) => panes
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(marker.as_str())),
            ReadyWhen::Prints(marker) => {
                let Some(text) = panes.pane_collapsed(pane) else {
                    return false;
                };
                // ⚠⚠ WHAT WAS TYPED AT THE PANE IS NOT WHAT THE PANE SAID. The pty echoes it, and
                // on the grid the echo is ordinary output — so a row carrying a piece of the
                // caller's own input is dropped before anything is read. This is the same rule
                // `Orchestrator::reaction` applies to its own stimulus, asked of input this plugin
                // did not write, which is only possible because the PANE remembers it.
                //
                // ⚠ Absent the capability the discount cannot be applied, and the fallback is the
                // arming count alone — weaker, and the reason `input_echo` returning `None` is
                // documented as a degradation rather than a default.
                let typed = panes
                    .input_echo()
                    .and_then(|echo| echo.pane_recent_input(pane))
                    .unwrap_or_default();
                // ⚠⚠ A MARKER THAT IS IN WHAT WAS TYPED AT THE PANE IS NOT EVIDENCE, EVER. The pty
                // echoes input, and on the grid that echo is ordinary output — so the marker has
                // two possible authors and nothing on the screen says which. Refusing it here is
                // what makes the answer DETERMINISTIC: the alternative was to hope the echo had
                // already landed before the barrier armed, and the same call then converged or fed
                // the shell depending on scheduling.
                //
                // ⚠ Checked against the MARKER rather than by dropping echoing ROWS. Dropping rows
                // depends on where the terminal happened to wrap and on whether a prompt shares
                // the line, which is the same non-determinism one step further in.
                if typed.contains(marker.as_str()) {
                    return false;
                }
                // ⚠⚠ MORE OCCURRENCES THAN WHEN THE BARRIER ARMED — counted over the whole
                // collapsed screen, never over rows a DAMAGE GENERATION says were repainted.
                //
                // A damage generation is a PAINT signal: it exists so a renderer knows what to
                // redraw. Two ordinary events stamp every row with a fresh one while no program
                // prints anything — a RESIZE (`Screen::reflowed`, which is what every attaching
                // client causes, in a terminal multiplexer of all products) and an OSC PALETTE
                // change (`mark_all_dirty`, which many programs send on startup). Answering *did
                // the pane print this* with it cleared the barrier on text that was already there.
                //
                // A count is immune to both, and to the re-wrap between them: the screen is
                // collapsed the way [`PaneAccess::pane_collapsed`] joins it, so a marker the pane
                // wrapped is ONE occurrence at either width.
                //
                // ⚠ The residue, stated rather than smoothed over: text scrolling off LOWERS the
                // count, so a marker that was on screen twice, scrolled away and was then printed
                // afresh does not exceed its baseline. That is a false NEGATIVE — the safe
                // direction — and [`ReadyWhen::Runs`] is the kind that has no such arithmetic.
                text.matches(marker.as_str()).count() > self.armed_at.unwrap_or(0)
            }
        }
    }

    /// Wait (once, then latched) for `pane` to satisfy the barrier.
    ///
    /// # Errors
    ///
    /// [`PaneError::NeverReady`] when the caller's bound elapses with the marker unseen and the
    /// run still has time. Driving on would inject into whatever IS there and report turns
    /// against a peer that was never listening, so the caller's run stops and says exactly what it
    /// was waiting for.
    ///
    /// ⚠ The RUN's own deadline is the other bound, and it is consulted by `poll_until`: a run
    /// whose clock is shorter ends as [`Reached::RunEnded`] instead. Both are real endings and
    /// they are different findings — one is *this pane never came up*, the other is *the run was
    /// out of time*, and only the first is about the pane.
    pub fn reached(
        &mut self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        run: &RunContext,
    ) -> Result<Reached, PaneError> {
        if self.seen {
            return Ok(Reached::Yes);
        }
        // ⚠ A BARRIER WITH NO CONDITION IS ALREADY DOWN, and saying so HERE is what keeps the
        // failure below honest. `seen` is set from `when.is_none()` at construction, so this arm is
        // unreachable in practice — but taking the condition out of the `Option` now means the
        // `NeverReady` error cannot be constructed without one. The alternative was a fabricated
        // empty marker for a case that cannot happen, which is a false sentence waiting for a
        // refactor to make it reachable.
        let Some(when) = self.when.clone() else {
            self.seen = true;
            return Ok(Reached::Yes);
        };
        // ⚠ ARM BEFORE THE FIRST LOOK, never before. Every occurrence of the marker on the screen
        // at this instant is one `Prints` refuses to count, and this is the first moment a pane is
        // in hand to read them from.
        if self.armed_at.is_none() {
            self.armed_at = Some(
                panes
                    .pane_collapsed(pane)
                    .map_or(0, |text| text.matches(when.marker()).count()),
            );
        }
        match poll_until(run, self.within, || self.satisfied(&when, panes, pane)) {
            Waited::Ready => {
                self.seen = true;
                Ok(Reached::Yes)
            }
            Waited::Stopped => Ok(Reached::RunEnded),
            // ⚠ THE DIAGNOSTIC IS READ AT THE MOMENT OF FAILURE, not carried from arming: what a
            // caller needs is what the pane was doing when the wait gave up. One read, on the way
            // out of a run that is already over.
            Waited::TimedOut => Err(PaneError::NeverReady {
                wanted: when,
                // ⚠ THE ABSENCE OF THE CAPABILITY AND THE ABSENCE OF A JOB ARE DIFFERENT ANSWERS —
                // one is about this build, the other about this pane. See [`PaneDoing`].
                instead: panes.foreground_job().map_or(PaneDoing::Unknown, |jobs| {
                    jobs.pane_foreground_leader(pane)
                        .map_or(PaneDoing::Nothing, |leader| PaneDoing::Job(leader.name))
                }),
            }),
        }
    }
}

/// Whether a foreground job's leader answers to `want`.
///
/// # ⚠ Two names, because the two sources honestly disagree
///
/// The kernel's name for a process is the basename of the FILE it exec'd, capped at 15 bytes and
/// rewritable by the process itself; `argv[0]` is what its parent called it. `exec awk` on a box
/// where `/usr/bin/awk` is `mawk` produces a leader named `mawk` whose `argv[0]` is `awk`, and a
/// caller who wrote `awk` is not wrong. Accepting either is the answer that does not require them
/// to know which spelling their platform packages.
///
/// ⚠ EXACT, never a prefix. A prefix match is a silent merge — `claude` accepting `claude-relay`
/// is a run that drives the wrong program and reports success.
fn leader_is_named(leader: &sprag_terminal::JobProcess, want: &str) -> bool {
    leader.name == want
        || leader.argv.first().is_some_and(|arg0| {
            std::path::Path::new(arg0)
                .file_name()
                .is_some_and(|base| base == want)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use sprag_terminal::{CommandBuilder, JobProcess, Workspace};
    use std::sync::{Arc, Mutex};

    /// ⚠⚠ **EVERY WORD THE VOCABULARY PUBLISHES IS ONE THE PARSER READS, AND BACK.**
    ///
    /// [`ReadyWhen::WIRE_WORDS`] is what the wire advertises as this argument's closed set, and
    /// [`ReadyWhen::parse`] is what the daemon reads it with. They are two spellings of one
    /// vocabulary, and R353 measured that shape going wrong in this workspace already — a mouse
    /// encoder and its decoder, each documented as the other's twin, with nothing comparing the
    /// lists. Driven from `WIRE_WORDS` rather than from a literal here, so a third kind added to
    /// the type fails this the moment its word is published without a parser arm.
    ///
    /// ⚠ [`word`](ReadyWhen::word) is the reader this gate exists to give the vocabulary: without
    /// it the type could publish a word it cannot spell back, which is how the two lists drift.
    #[test]
    fn every_published_readiness_word_round_trips_through_the_parser() {
        for word in ReadyWhen::WIRE_WORDS {
            let parsed = ReadyWhen::parse(word, "MARK".to_string())
                .unwrap_or_else(|| panic!("{word:?} is published and the parser refuses it"));
            assert_eq!(
                parsed.word(),
                *word,
                "and it must spell back the word it was read from",
            );
            assert_eq!(parsed.marker(), "MARK", "carrying the caller's marker");
        }
        assert_eq!(
            ReadyWhen::WIRE_WORDS.len(),
            4,
            "the four questions a readiness marker can ask, in increasing strength: whether the \
             pane PRINTS it after the run arms, whether it SHOWS it already, whether the pane's \
             terminal belongs to a job that RUNS it, or whether the agent SETTLES as it and waits \
             — and only the first two are questions about the screen",
        );
        assert!(
            ReadyWhen::parse("appears", "MARK".to_string()).is_none(),
            "and a word outside the set is refused, or the published `enum` is a false statement",
        );
        // ⚠⚠ AND AN EMPTY MARKER, IN EVERY KIND — driven from the published words rather than
        // spot-checked on one, because the reason differs per kind and the refusal must not.
        // `Shows("")` is the dangerous one: every screen contains `""`, so the barrier clears
        // instantly while LOOKING like a barrier the caller asked for.
        for word in ReadyWhen::WIRE_WORDS {
            assert!(
                ReadyWhen::parse(word, String::new()).is_none(),
                "{word:?} with an empty marker is a MALFORMED request, not a barrier: the \
                 argument admits fewer values than its `String` type",
            );
            // ⚠⚠ AND EVERY KIND CAN SAY ITS OWN FAILURE. `describe` is what
            // `PaneError::NeverReady`'s sentence is built from, and it is an exhaustive match —
            // so a fifth kind compiles the moment it is added and would reach an agent with
            // whatever clause its author wrote. Driven from the published words so each arm is
            // BUILT here rather than trusted.
            let said = ReadyWhen::parse(word, "MARK".to_string())
                .expect("published")
                .describe();
            assert!(
                said.contains("MARK") && said.starts_with(char::is_lowercase),
                "{word:?} must describe itself as a past-tense clause naming the marker — it is \
                 read after \"the pane never \": {said:?}",
            );
        }
    }

    /// ⚠⚠ **OWNING THE TERMINAL IS NOT LISTENING** — the gap [`ReadyWhen::Runs`] cannot close, and
    /// the reason [`ReadyWhen::Settles`] exists.
    ///
    /// A cold agent takes its pane's terminal seconds before it will answer anything. `Runs` clears
    /// at the first instant — correctly, because *has it started* is the question it answers — and a
    /// caller who meant *is it waiting for me* has been driving a starting program ever since.
    ///
    /// **The discriminator is the SAME PANE AT THE SAME MOMENT**, asked both ways. The supervisor
    /// reports `Working` (the agent is up, thinking) and the fixture asserts:
    ///
    /// 1. `Runs("tr")` IS satisfied — the program owns the terminal, which is true and not enough;
    /// 2. `Settles("claude")` is NOT — it is working, not waiting;
    /// 3. and once the same supervisor reports `Idle`, `Settles` clears.
    ///
    /// Half 1 is what makes this a discriminator rather than a restatement: without it the gate
    /// would prove only that a made-up condition is unsatisfied.
    ///
    /// ⚠ `Blocked` gets its own half, because it is the arm most likely to be "fixed" into
    /// readiness by someone reading only the state name. A blocked agent is waiting for an answer
    /// to ITS OWN question — a prompt sent there answers the wrong thing, and into a numbered menu
    /// it SELECTS.
    #[test]
    fn a_program_that_owns_the_terminal_is_not_yet_an_agent_that_is_listening() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec tr a-z A-Z");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        // The supervisor this host installs, driven by the test rather than by a screen rule — the
        // question here is what the BARRIER does with an observation, not how one is derived.
        let reported = Arc::new(Mutex::new((
            AgentState::Working,
            Some("claude".to_string()),
        )));
        let source = {
            let reported = Arc::clone(&reported);
            Arc::new(move |_id: PaneId| {
                let (state, agent) = reported.lock().unwrap().clone();
                Some(crate::access::AgentObservation {
                    state,
                    agent,
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq: 1,
                    asking: None,
                })
            })
        };
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));

        let settled = |access: &WorkspacePaneAccess| {
            Readiness::new(
                Some(ReadyWhen::Settles("claude".to_string())),
                Some(Duration::from_millis(150)),
            )
            .reached(access, pane, &RunContext::uncancellable())
        };

        // ⚠ HALF 1, THE CONTROL: the weaker question is ALREADY satisfied at this instant.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(50)),
            )
            .reached(&access, pane, &RunContext::uncancellable())
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(200)),
            )
            .reached(&access, pane, &RunContext::uncancellable()),
            Ok(Reached::Yes),
            "the program owns the terminal — so this gate is about the DIFFERENCE between the two \
             questions, not about a pane that never came up",
        );

        // HALF 2: working is not waiting.
        assert_eq!(
            settled(&access),
            Err(PaneError::NeverReady {
                wanted: ReadyWhen::Settles("claude".to_string()),
                instead: PaneDoing::Job("tr".to_string()),
            }),
            "an agent that is WORKING is not ready to be typed at, however firmly it owns the \
             terminal",
        );

        // HALF 3: blocked is waiting for an answer to its own question, which is not this.
        *reported.lock().unwrap() = (AgentState::Blocked, Some("claude".to_string()));
        assert!(
            settled(&access).is_err(),
            "a BLOCKED agent is waiting for an answer to its own question — a fresh prompt sent \
             there answers the wrong thing, and into a numbered menu it selects",
        );

        // HALF 4: an idle observation that names no agent says nothing about WHICH is at rest.
        *reported.lock().unwrap() = (AgentState::Idle, None);
        assert!(
            settled(&access).is_err(),
            "an observation that names no agent is not evidence about which program is at rest",
        );

        // HALF 5: named and idle.
        *reported.lock().unwrap() = (AgentState::Idle, Some("claude".to_string()));
        assert_eq!(
            settled(&access),
            Ok(Reached::Yes),
            "the agent the caller named is at rest and waiting for input — NOW drive it",
        );
    }

    /// ⚠⚠ **A REPAINT IS NOT A PRINT, AND A RESIZE REPAINTS EVERY ROW.**
    ///
    /// [`ReadyWhen::Prints`] baselined each row's DAMAGE GENERATION and counted a row as evidence
    /// once that number moved past the baseline. A damage generation is a PAINT signal — it exists
    /// so a renderer knows which rows to redraw — and answering a CONTENT question with it is the
    /// category error this gate names. Two ordinary things stamp every row with a fresh generation
    /// without a program printing anything:
    ///
    /// * **a RESIZE** (`Emulator::resize` → `Screen::reflowed(cols, rows, g)`), which is what every
    ///   client attaching to a session does — in a terminal multiplexer, of all products;
    /// * **an OSC palette change** (`repaint_for_palette_change` → `mark_all_dirty`), which many
    ///   programs send on startup.
    ///
    /// So a pane whose screen ALREADY carried the marker — a word from an earlier command, a
    /// banner, anything the caller did not type and the echo trail therefore never saw — cleared the
    /// barrier the instant anybody resized. The run then drove whatever was there.
    ///
    /// ⚠ This fixture uses a REAL pty and a REAL resize rather than a hand-built row list, because
    /// the claim is about what the emulator does, and a double asserting my own belief about that
    /// would be the gate believing itself.
    #[test]
    fn a_repaint_of_text_that_was_already_there_is_not_the_pane_printing_it() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // ⚠ The marker is printed BY THE PROGRAM and never typed at the pane, so the echo trail
            // does not cover it — this is the half R359c's fix cannot reach.
            command.arg("printf 'BANNER\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("BANNER"))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("BANNER")),
            "the fixture must get the marker on screen BEFORE the barrier arms, or it is not \
             asking this question at all",
        );

        let mut ready = Readiness::new(
            Some(ReadyWhen::Prints("BANNER".to_string())),
            Some(Duration::from_millis(600)),
        );
        let outcome = std::thread::scope(|scope| {
            let waiting =
                scope.spawn(|| ready.reached(&access, pane, &RunContext::uncancellable()));
            // Let the barrier ARM (it baselines on its first look), then resize — which is what an
            // attaching client does, and what re-stamps every row.
            std::thread::sleep(Duration::from_millis(120));
            workspace
                .lock()
                .unwrap()
                .resize(pane, 30, 8, (0, 0))
                .expect("resize the pane");
            waiting.join().expect("the barrier thread")
        });

        assert_eq!(
            outcome,
            Err(PaneError::NeverReady {
                wanted: ReadyWhen::Prints("BANNER".to_string()),
                instead: PaneDoing::Job("cat".to_string()),
            }),
            "text that was on screen when the barrier armed is NOT the pane printing it, however \
             many times the screen is re-laid out under it",
        );
    }

    /// ⚠⚠ **A PANE WHOSE CHILD HAS GONE SAYS SO, RATHER THAN BLAMING THE BUILD** — the third
    /// [`PaneDoing`] arm, and the reason that field stopped being an `Option`.
    ///
    /// A host that CAN see the process table and finds no job owning the terminal has learned
    /// something about the PANE. Spelled as the same `None` that means *this build has no process
    /// view*, it told the caller their deployment was blind when it was working perfectly.
    #[test]
    fn a_pane_whose_child_has_exited_is_reported_as_nothing_owning_its_terminal() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exit 0");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) && access.pane_eof(pane) != Some(true) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "the fixture's child must be GONE, or this is not the arm being built",
        );

        let failed = Readiness::new(
            Some(ReadyWhen::Runs("claude".to_string())),
            Some(Duration::from_millis(120)),
        )
        .reached(&access, pane, &RunContext::uncancellable())
        .expect_err("a pane with no child can never come to run anything");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Nothing,
            },
            "the host CAN see the process table and there is no job — that is a fact about the \
             PANE, and it must not read as a blind build",
        );
        assert!(
            failed.to_string().contains("the pane's child had gone"),
            "and the sentence says which: {failed}",
        );
    }

    /// ⚠⚠ **A HOST WITH NO ECHO TRAIL LOSES ONE DISCOUNT, NOT THE WHOLE BARRIER** — the degradation
    /// [`PaneAccess::input_echo`] documents, and which no gate built.
    ///
    /// `input_echo` returning `None` costs [`ReadyWhen::Prints`] its refusal of a marker the caller
    /// TYPED. It must not also cost the arming count, or a pane that already showed the marker
    /// would clear the barrier on a host that merely cannot see its own echo — the two protections
    /// are independent and only one of them is a capability.
    #[test]
    fn a_host_with_no_echo_trail_still_refuses_a_marker_that_was_already_on_screen() {
        /// Every optional capability at its default, and a screen that never changes.
        struct NoEchoTrail;
        impl PaneAccess for NoEchoTrail {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some("BANNER at rest".to_string())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some("BANNER at rest".to_string())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[crate::access::KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                panic!("⚠⚠ NOT ONE BYTE — the marker was on screen before the barrier armed");
            }
        }
        assert!(
            Readiness::new(
                Some(ReadyWhen::Prints("BANNER".to_string())),
                Some(Duration::from_millis(120)),
            )
            .reached(&NoEchoTrail, PaneId(1), &RunContext::uncancellable())
            .is_err(),
            "the arming COUNT is not a capability and must protect a host that has no echo trail",
        );
    }

    /// ⚠⚠ **A JOB'S LEADER IS NOT EVERY PROCESS IN IT** — the documented limit of
    /// [`ReadyWhen::Runs`], measured rather than left as prose.
    ///
    /// `cat | tr` is ONE job of two processes led by the shell that started them, so a caller who
    /// names `tr` is naming a member and not the leader. The honest answer is that the barrier does
    /// not clear — and the failure NAMES the leader, which is exactly what tells the caller they
    /// asked about the wrong end of a pipeline.
    #[test]
    fn a_pipeline_is_led_by_its_shell_and_naming_a_member_does_not_clear() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("cat | tr a-z A-Z");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 20, 4)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        assert_eq!(
            Readiness::new(
                Some(ReadyWhen::Runs("tr".to_string())),
                Some(Duration::from_millis(400)),
            )
            .reached(&access, pane, &RunContext::uncancellable()),
            Err(PaneError::NeverReady {
                wanted: ReadyWhen::Runs("tr".to_string()),
                instead: PaneDoing::Job("sh".to_string()),
            }),
            "`tr` is a MEMBER of the job, not its leader — and the failure names the leader, which \
             is what tells a caller they named the wrong end of their pipeline",
        );
    }

    /// ⚠⚠ **THE TWO NAMES A LEADER HAS, AND THE MERGE THAT MUST NOT HAPPEN.**
    ///
    /// [`leader_is_named`] is where [`ReadyWhen::Runs`] decides, and two of its claims are reachable
    /// from no pty fixture in this crate: every program the fixtures start (`tr`, `cat`) has a
    /// kernel name equal to its `argv[0]`, so the second arm is never taken and a prefix match would
    /// pass every one of them. Both are ordinary on a real box — `exec awk` where `/usr/bin/awk` is
    /// `mawk` is exactly the disagreement, and it is a packaging decision the caller cannot see.
    ///
    /// Four claims, each a different way to get this wrong:
    ///
    /// 1. the KERNEL name answers;
    /// 2. `argv[0]`'s BASENAME answers when the kernel name disagrees — the `mawk`/`awk` case, and
    ///    the reason a caller is not asked which spelling their distribution chose;
    /// 3. a PREFIX does not answer, in either direction. `claude` accepting `claude-relay` is a run
    ///    driving the wrong program and reporting success, which is worse than never starting;
    /// 4. a job with NO argv at all — a kernel thread, a zombie whose argv the kernel has already
    ///    released — still answers by its kernel name rather than panicking on an empty vector.
    #[test]
    fn a_leader_answers_to_its_kernel_name_or_its_argv_basename_and_to_neither_by_prefix() {
        let leader = |name: &str, argv: &[&str]| JobProcess {
            pid: 4242,
            name: name.to_string(),
            argv: argv.iter().map(|arg| (*arg).to_string()).collect(),
        };

        let awk = leader("mawk", &["awk", "{print}"]);
        assert!(
            leader_is_named(&awk, "mawk"),
            "the kernel's name for the process answers",
        );
        assert!(
            leader_is_named(&awk, "awk"),
            "and so does what its parent called it — a caller who wrote `awk` on a box that \
             packages `mawk` is not wrong, and cannot be expected to know",
        );

        let absolute = leader("claude", &["/usr/local/bin/claude", "--print"]);
        assert!(
            leader_is_named(&absolute, "claude"),
            "an absolute `argv[0]` is matched by its BASENAME, or naming a program would mean \
             knowing where it was installed",
        );

        let relay = leader("claude-relay", &["claude-relay"]);
        assert!(
            !leader_is_named(&relay, "claude"),
            "⚠⚠ A PREFIX IS NOT A MATCH: `claude` accepting `claude-relay` is a run that drives \
             the wrong program and reports success",
        );
        assert!(
            !leader_is_named(&leader("cl", &["cl"]), "claude"),
            "and neither is the other direction",
        );

        assert!(
            leader_is_named(&leader("cat", &[]), "cat"),
            "a job with no argv at all — a zombie's is released at exit — still answers by the \
             name the kernel keeps",
        );
    }

    /// ⚠⚠ **A HOST THAT CANNOT SEE THE PROCESS TABLE FAILS THE RUN RATHER THAN DRIVING IT** — the
    /// arm every pty gate in this crate skips, because the production access implements the
    /// capability and therefore never builds the absence.
    ///
    /// [`PaneAccess::foreground_job`] defaults to `None`, and that default is what a port to a
    /// platform with no process table would land on. A capability that is missing has exactly two
    /// possible readings and only one of them is safe: *"cannot tell, so assume ready"* types into
    /// whatever is there, which is the whole failure the barrier exists to prevent. **The safe
    /// direction is documented on the trait and, until this gate, was asserted nowhere** — the
    /// same shape the echo trail's own absence still owes, and not one worth owing twice.
    ///
    /// Two halves: the run FAILS (not converges), and the failure's `instead` is `None` — the arm
    /// of the sentence that says *this build cannot say what the pane was running*, which is a
    /// different message from naming a program and is otherwise built by nothing.
    #[test]
    fn a_host_that_cannot_see_the_process_table_is_never_ready_to_run_a_program() {
        /// A pane that exists, shows nothing, and whose host has NO process view — every
        /// capability left at its default, which is the point.
        struct NoProcessView;
        impl PaneAccess for NoProcessView {
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
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[crate::access::KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                panic!("⚠⚠ NOT ONE BYTE may be injected into a pane this host cannot vouch for");
            }
        }

        let mut ready = Readiness::new(
            Some(ReadyWhen::Runs("claude".to_string())),
            Some(Duration::from_millis(120)),
        );
        let failed = ready
            .reached(&NoProcessView, PaneId(1), &RunContext::uncancellable())
            .expect_err("a host that cannot see the process table can never confirm the program");
        assert_eq!(
            failed,
            PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Unknown,
            },
            "and it says so by having NO answer for what ran instead, rather than inventing one",
        );
        // ⚠ THE SIBLING CAPABILITY, SAME RULE. `supervision()` is `None` on this double too, and a
        // host that cannot supervise must not conclude that an agent has settled — the safe
        // direction is the same one, and it is the arm a second capability makes easy to forget.
        assert!(
            Readiness::new(
                Some(ReadyWhen::Settles("claude".to_string())),
                Some(Duration::from_millis(120)),
            )
            .reached(&NoProcessView, PaneId(1), &RunContext::uncancellable())
            .is_err(),
            "a host with no detector cannot say an agent is at rest, so it must not type at one",
        );
        assert!(
            !failed.to_string().contains("instead"),
            "so the sentence simply omits that clause rather than reading `belonged to None`: {}",
            failed,
        );
    }
}
