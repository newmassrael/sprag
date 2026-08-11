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

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
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
    /// The answer for *"I just started it"*, and echo-proof for anything typed before the run: the
    /// barrier baselines every row's damage generation on its first look and only reads rows that
    /// moved past it. Wrap-safe — the moved rows are joined the way
    /// [`pane_collapsed`](crate::access::PaneAccess::pane_collapsed) joins the screen, so a marker
    /// the pane wrapped is still found.
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
}

impl ReadyWhen {
    /// The two words a caller may spell, in this type's own order.
    ///
    /// Published to every mouth from here rather than retyped as literals, so a third kind reaches
    /// the wire in the compile that adds it.
    pub const WIRE_WORDS: &'static [&'static str] = &["prints", "shows", "runs"];

    /// The kind named by `word`, or `None` for a word outside the closed set.
    ///
    /// ⚠ A caller who sends something else has made a MALFORMED request, not a rejected one —
    /// R353's rule, and the reason this returns an `Option` for the parser to turn into the wire's
    /// own grammar refusal rather than a friendly sentence.
    #[must_use]
    pub fn parse(word: &str, marker: String) -> Option<Self> {
        match word {
            "prints" => Some(Self::Prints(marker)),
            "shows" => Some(Self::Shows(marker)),
            "runs" => Some(Self::Runs(marker)),
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
        }
    }

    /// The text the pane must carry — or, for [`Runs`](Self::Runs), the program it must be running.
    #[must_use]
    pub fn marker(&self) -> &str {
        match self {
            Self::Prints(marker) | Self::Shows(marker) | Self::Runs(marker) => marker,
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
    /// Captured on the first look rather than at construction, because that is the moment the
    /// question is first asked and the only one a `PaneAccess` is in hand for. `None` until then.
    armed_at: Option<Vec<u64>>,
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
            ReadyWhen::Shows(marker) => panes
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(marker.as_str())),
            ReadyWhen::Prints(marker) => {
                let Some(rows) = panes.pane_rows(pane) else {
                    return false;
                };
                let armed = self.armed_at.as_deref().unwrap_or_default();
                // ⚠⚠ WHAT WAS TYPED AT THE PANE IS NOT WHAT THE PANE SAID. The pty echoes it, and
                // on the grid the echo is ordinary output — so a row carrying a piece of the
                // caller's own input is dropped before anything is read. This is the same rule
                // `Orchestrator::reaction` applies to its own stimulus, asked of input this plugin
                // did not write, which is only possible because the PANE remembers it.
                //
                // ⚠ Absent the capability the discount cannot be applied, and the fallback is the
                // generation baseline alone — weaker, and the reason `input_echo` returning `None`
                // is documented as a degradation rather than a default.
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
                // The rows that MOVED since arming, joined the way the whole screen is joined, so a
                // marker the pane wrapped across two fresh rows is still found. A row that did not
                // move is not evidence — that is the entire point of this kind.
                let printed: String = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, row)| row.generation > armed.get(*i).copied().unwrap_or(0))
                    .map(|(_, row)| row.text.trim_end())
                    .collect();
                printed.contains(marker.as_str())
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
        // ⚠ ARM BEFORE THE FIRST LOOK, never before. Everything on the screen at this instant is
        // what `Prints` refuses to count, and this is the first moment a pane is in hand to read
        // it from.
        if self.armed_at.is_none() {
            self.armed_at = Some(
                panes
                    .pane_rows(pane)
                    .map(|rows| rows.iter().map(|row| row.generation).collect())
                    .unwrap_or_default(),
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
                instead: panes
                    .foreground_job()
                    .and_then(|jobs| jobs.pane_foreground_leader(pane))
                    .map(|leader| leader.name),
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
    use sprag_terminal::JobProcess;

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
            3,
            "the three questions a readiness marker can ask: whether the pane PRINTS it after the \
             run arms, whether it SHOWS it already, or whether the pane's terminal belongs to a \
             job that RUNS it — the last of which is not a question about the screen at all",
        );
        assert!(
            ReadyWhen::parse("appears", "MARK".to_string()).is_none(),
            "and a word outside the set is refused, or the published `enum` is a false statement",
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
                instead: None,
            },
            "and it says so by having NO answer for what ran instead, rather than inventing one",
        );
        assert!(
            !failed.to_string().contains("instead"),
            "so the sentence simply omits that clause rather than reading `belonged to None`: {}",
            failed,
        );
    }
}
