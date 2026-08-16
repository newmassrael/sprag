//! `PaneAccess` — the plugin extension API.
//!
//! A plugin's whole view of the core: enumerate panes, read a pane's screen as
//! scene-as-data, and inject input — all addressed by [`PaneId`], never by
//! reaching into a `PanePtyHandle` or `Screen` directly. This is the single
//! read+inject path: every plugin (and any future control consumer) goes
//! through it, so reads and injections are consistent and the input-encoding
//! lives in one place.
//!
//! [`WorkspacePaneAccess`] is the production implementation over a shared
//! [`Workspace`]; it stays pinion-free (the producer/control layer).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sprag_detect::{AgentState, Question};
use sprag_input::{Modifiers, encode};
use sprag_terminal::{
    Attention, CommandBuilder, Hands, JobProcess, Pane, PaneBirthHooks, PaneEcho, PaneEndOfInput,
    PaneId, PanePtyHandle, RawOutput, Reach, Stop, StoppedJob, Unstopped, Workspace,
    foreground_leader_of,
};
use sprag_vt::LinesSince;
use sprag_vt::Screen;

use crate::readiness::ReadyWhen;

/// One screen row: its damage `generation` paired with its (trailing-trimmed)
/// text, read in a single locked snapshot so the two never tear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub generation: u64,
    pub text: String,
}

/// A mark of what a pane's rows HELD, so a later look can say which of them carry new CONTENT.
///
/// # ⚠⚠ Why this is not a damage generation, which is what all four callers used
///
/// Every plugin in this crate asks one question — *what has this pane produced since I last
/// looked?* — and all four answered it with [`PaneRow::generation`]. **A damage generation is a
/// PAINT signal**: it exists so a renderer knows which rows to redraw. Two ordinary events stamp
/// every row with a fresh one while no program produces a byte:
///
/// * a **RESIZE** (`Emulator::resize` → `Screen::reflowed`) — which is what a client ATTACHING to
///   the session does, in a terminal multiplexer of all products;
/// * an **OSC PALETTE change** (`repaint_for_palette_change` → `mark_all_dirty`), which many
///   programs send on startup.
///
/// Each caller then reported something false, and the severity ran from cosmetic to dangerous:
/// [`Pipe`](crate::pipe::Pipe) **RE-RELAYED THE SOURCE'S WHOLE SCREEN** into a peer that acts on
/// what it receives (measured: 16 bytes sent for a resize that printed nothing);
/// [`Agent`](crate::agent::Agent) captured the whole screen and published it **AS THE MODEL'S
/// REPLY**; [`Orchestrator`](crate::orchestrator::Orchestrator) read it as *the peer answered* and
/// took another turn.
///
/// So the question is asked of the CONTENT. A row is fresh when its text differs from what this
/// mark recorded — which no repaint, palette change or re-render can fake.
///
/// ⚠ [`PaneRow::generation`] is still the right answer to a PAINT question, and
/// [`has_painted`](crate::deliver::has_painted) still asks one. The rule is not *never use it* — it
/// is **use it for what it is**.
#[derive(Clone, Debug, Default)]
pub struct RowTrail(Vec<String>);

impl RowTrail {
    /// Mark what `pane`'s rows hold right now.
    #[must_use]
    pub fn mark(panes: &dyn PaneAccess, pane: PaneId) -> Self {
        Self(
            panes
                .pane_rows(pane)
                .unwrap_or_default()
                .into_iter()
                .map(|row| row.text)
                .collect(),
        )
    }

    /// The rows whose text has CHANGED since this mark, trailing-trimmed, in screen order.
    ///
    /// ⚠ A row that changed and changed BACK is not fresh, and neither is a row that reprinted the
    /// text it already held. Both are indistinguishable from *nothing happened* by any measure a
    /// screen can offer, and reporting them would be guessing — the safe direction is the one that
    /// costs a caller a wait rather than a wrong answer.
    #[must_use]
    pub fn fresh(&self, panes: &dyn PaneAccess, pane: PaneId) -> Vec<String> {
        panes
            .pane_rows(pane)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .filter(|(i, row)| self.0.get(*i).map(String::as_str) != Some(row.text.as_str()))
            .map(|(_, row)| row.text.trim_end().to_string())
            .collect()
    }

    /// The fresh rows, and ADVANCE this mark past them — for a consumer that must not deliver the
    /// same row twice. See [`fresh`](Self::fresh) for what counts.
    #[must_use]
    pub fn take_fresh(&mut self, panes: &dyn PaneAccess, pane: PaneId) -> Vec<String> {
        let fresh = self.fresh(panes, pane);
        *self = Self::mark(panes, pane);
        fresh
    }
}

/// A key to inject: a W3C `KeyboardEvent.key` string plus modifiers, encoded
/// to PTY bytes by [`PaneAccess::inject`] (the sprag-owned encoder, R2.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: String,
    pub mods: Modifiers,
}

impl KeyStroke {
    /// A single unmodified named key (e.g. `KeyStroke::named("Enter")`).
    #[must_use]
    pub fn named(key: &str) -> Self {
        Self {
            key: key.to_string(),
            mods: Modifiers::default(),
        }
    }

    /// Expand text into one unmodified character keystroke per `char`.
    #[must_use]
    pub fn text(s: &str) -> Vec<Self> {
        s.chars()
            .map(|ch| Self {
                key: ch.to_string(),
                mods: Modifiers::default(),
            })
            .collect()
    }
}

/// What [`PaneAccess::inject`] returns: bytes WRITTEN to the pane's pseudoterminal.
///
/// A count with a name, and the name is the contract. Writing to a pty succeeds the moment the
/// kernel takes the bytes, which says nothing about the program on the other end having taken
/// them — a TUI that has not finished starting reads its input and throws it away, and the write
/// that vanished reports exactly the same success as the one that landed. Measured against a rival
/// while supervising a real agent session: text injected the instant the agent reported itself idle
/// disappeared with no error, leaving an empty prompt and a supervisor waiting forever for work it
/// had never actually asked for.
///
/// So this type is the API saying what it knows. A caller that wants *the pane took it* wants
/// [`deliver`](crate::deliver::deliver), which returns a [`Delivered`](crate::deliver::Delivered)
/// and cannot be reached from here — the distinction is in the types rather than in a doc comment
/// somebody has to have read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[must_use]
pub struct Written(u64);

impl Written {
    /// A receipt for `bytes` handed to a pty. Public so a test double can implement
    /// [`PaneAccess`]; nothing about constructing one makes it a delivery.
    pub const fn of(bytes: u64) -> Self {
        Self(bytes)
    }

    /// How many bytes reached the pseudoterminal.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }
}

sprag_vt::closed_set! {
/// Why [`PaneAccess::inject`] failed — a typed cause, not a discarded error.
///
/// ⚠⚠ **A CLOSED SET, because the sentence gate over it was a hand-written list of five.** A run's
/// failure reaches its caller as this type's [`Display`](std::fmt::Display), and that gate is the
/// only thing standing between an agent and a `format!("{e:?}")` leak. It walked a literal array,
/// so a SIXTH variant would have been ungated the day it was added — and the list could not be
/// derived, because [`NeverReady`](Self::NeverReady) has named fields and `closed_set!` could not
/// express one. The macro grew the form rather than this type losing its field names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneError {
    /// No pane has the given id.
    UnknownPane(PaneId) = (PaneId(0)),
    /// A keystroke had no PTY-byte encoding (the offending key).
    Encode(String) = (String::new()),
    /// Writing the encoded bytes to the pane failed (the IO error message).
    Write(String) = (String::new()),
    /// ⚠⚠⚠⚠ **THE PANE'S PROGRAM HAS EXITED, SO NOTHING WAS TYPED INTO IT.**
    ///
    /// # ⚠⚠⚠ Why this is a refusal and not a write that happens to go nowhere
    ///
    /// A pty master whose slave nobody reads is not a hole, it is a wall with a queue in front of
    /// it. Measured on this workstation
    /// (`sprag-terminal/tests/write_to_a_dead_pane_wedges.rs`): a dead pane takes **16,896 bytes**
    /// of newline-terminated input and then `write(2)` **blocks for ever**, holding the pane's
    /// shared writer mutex — so every other writer to that pane is stranded behind it, and a
    /// blocked write cannot be cancelled. That is the defect that held a build machine for 43
    /// hours (register items 304, 309, 318, 319).
    ///
    /// ⚠⚠ **AND NOTHING ABOUT THE WALK TO IT LOOKS WRONG.** An `Orchestrator` types its stimulus at
    /// the start of EVERY step: measured at **5 bytes and 509 ms a step, so 3,380 steps — about 29
    /// minutes — from a dead peer to a wedged machine** (item 325). Not a burst; a patient march.
    ///
    /// ⚠⚠⚠ **THE EVIDENCE WAS ALREADY HERE.** [`PaneAccess::pane_eof`] answers `Some(true)` before
    /// the first of those bytes and after every one of them, and two other readers already consult
    /// it ([`DoneWhen::Exits`](crate::completion::DoneWhen::Exits), [`Pipe`](crate::pipe::Pipe)).
    /// **The hand on the keyboard did not** (item 324). This variant is that reading, at the one
    /// door a plugin types through.
    ///
    /// ⚠ It NAMES THE PANE, because a refusal that does not say which one is the other defect this
    /// workspace has paid for four rounds running (R396-R399, and item 311's own warning).
    PeerGone(PaneId) = (PaneId(0)),
    /// Spawning a pane failed: no [`PaneLifecycle`] support, an empty argv, or
    /// the pseudoterminal/child could not start (the cause message).
    Spawn(String) = (String::new()),
    /// A run's readiness barrier gave up: the pane never answered the question the caller asked,
    /// so nothing was injected.
    NeverReady {
        /// The whole question, not just its marker.
        ///
        /// ⚠ The four [`ReadyWhen`] kinds fail for four DIFFERENT reasons — a marker that was never
        /// printed, a screen that never carried it, a program that never took the terminal, an
        /// agent that never came to rest — and a caller handed only the marker back cannot tell
        /// which of the four they got wrong.
        wanted: ReadyWhen,
        /// What the pane was doing instead.
        ///
        /// The diagnostic half, and the reason a wrong guess costs one run rather than an
        /// afternoon: *"you waited for `claude` and this pane's terminal belonged to `sh`"* is
        /// the whole correction, in the sentence that reports the failure. Answered for every
        /// kind, because *what was actually running* diagnoses a marker that never printed just as
        /// well as a name that never matched.
        instead: PaneDoing,
        /// ⚠⚠⚠ WHETHER WHAT THE CALLER NAMED IS ALREADY ON THE PANE'S SCREEN — the correction
        /// [`instead`](Self::NeverReady::instead) structurally cannot carry.
        ///
        /// [`ReadyWhen::Prints`] means *more occurrences than when this run started watching*, so a
        /// pane that printed the marker BEFORE the run was asked for can never satisfy it. That is
        /// not an exotic case: opening a pane and asking for a run are separate calls, and a
        /// program announces itself once, on the way up. The barrier is right to refuse and
        /// `instead` then reports the JOB — true, and about a question the caller did not ask.
        ///
        /// ⚠ It states the FACT and not the INTENT. A peer that re-announces every turn is a caller
        /// who meant `prints` exactly, and whose real finding is that their peer went quiet.
        ///
        /// ⚠ `false` for the other three kinds rather than *unknown*: [`Shows`](ReadyWhen::Shows)
        /// failing already means the text is not there, and the markers of
        /// [`Runs`](ReadyWhen::Runs) and [`Settles`](ReadyWhen::Settles) name a program and an
        /// agent — neither is screen text, so a screen answer about them would be a coincidence
        /// reported as a diagnosis.
        already_showing: bool,
    } = {
        wanted: ReadyWhen::Shows(String::new()),
        instead: PaneDoing::Unknown,
        already_showing: false,
    },
    /// ⚠ A PROMPT WAS WRITTEN INTO A READY PANE AND THE PANE NEVER SHOWED IT, so nothing was
    /// submitted and the peer was never asked.
    ///
    /// The sibling of [`NeverReady`](Self::NeverReady) one step later: that one is *the pane never
    /// became the thing you wanted to talk to*, this one is *it was, and it did not take what you
    /// said*. Both end with nothing asked, and the distinction is what a caller acts on — the first
    /// is a wrong `ready_when`, the second is a peer swallowing its input.
    ///
    /// A separate refusal rather than an empty reply because **an empty reply is a sentence about
    /// the model** and this is a sentence about the pane. A run that converged with nothing
    /// captured told its caller the model had said nothing, which is the worst reading available:
    /// it is the only one that is actionable and wrong.
    NeverTook {
        /// How many injections were written before giving up — [`Delivery::attempts`] worth.
        ///
        /// [`Delivery::attempts`]: crate::deliver::Delivery::attempts
        attempts: u32,
        /// How many bytes reached the pseudoterminal across all of them. Paid for, and gone.
        written: u64,
    } = { attempts: 0, written: 0 },
    /// ⚠⚠ THE PROMPT ARRIVED AND THE SUBMIT AFTER IT SHOWED NOTHING, so the text is sitting in the
    /// pane and the peer was never asked.
    ///
    /// The sibling of [`NeverTook`](Self::NeverTook) ONE KEYSTROKE later, and the two are a pair
    /// for the reason that one is a pair with [`NeverReady`](Self::NeverReady): each is the same
    /// run failing at a further point along, and which one a caller meets is what they act on. The
    /// text never appeared — a peer swallowing input. The text appeared and the submit did nothing
    /// — a peer that took the keystroke and started nothing, which is what a live agent looks like
    /// when its composer treats a coalesced `…prompt…\r` as a paste.
    ///
    /// ⚠ It carries the CONTRACT that went unsatisfied ([`SubmittedWhen`](crate::deliver::SubmittedWhen))
    /// for [`NeverReady`](Self::NeverReady)'s reason: *"the pane did not repaint"* is a false
    /// sentence about a run that was watching the supervisor instead, and the failure text is what
    /// an agent reads.
    NeverSubmitted {
        /// How many injections carried the TEXT before it was read back.
        attempts: u32,
        /// How many bytes reached the pseudoterminal, the submit's own among them.
        written: u64,
        /// What the caller said would show them the submit had landed.
        wanted: crate::deliver::SubmittedWhen,
    } = {
        attempts: 0,
        written: 0,
        wanted: crate::deliver::SubmittedWhen::Repaints { within: std::time::Duration::ZERO },
    },
    /// ⚠⚠ **THE MACHINE DRIVING THIS RUN COULD NOT BE DRIVEN ON**, and the clause saying why.
    ///
    /// The one arm that is not about the pane, and it is here because this type is what
    /// [`Plugin::step`](crate::plugin::Plugin::step) returns — the substrate's single channel for
    /// *this step could not be taken*. A statechart plugin's step is a transition of a document,
    /// and a document can stop being drivable in ways no pane operation describes: its datamodel
    /// stops holding the prompt the transition owes a peer
    /// ([`OuterLoop::authored`](crate::outer::OuterLoop::authored) says why that is read at the
    /// moment of delivery rather than cached), or it reaches a state this build has no effect for.
    ///
    /// ⚠ It carries the CLAUSE and not just the fact, because the alternative is a failed run
    /// whose only sentence is that it failed. Every producer names what it was and what it wanted:
    /// the remedy here is a fix — the document, the engine, or the pin under it — which is exactly
    /// what separates a `failed` outcome from a `blocked` one that wants an answer.
    Undrivable(String) = (String::new()),
    /// ⚠ A STOP WAS NOT DELIVERED, so the pane's job is STILL RUNNING — and why.
    ///
    /// Distinct from every other arm here because the others describe something that did not
    /// happen; this one also describes something that is still happening. A run cancelled at its
    /// deadline whose stop failed has left work on somebody's machine, and *"cancelled"* on its own
    /// would tell them the opposite.
    NotStopped(Unstopped) = (Unstopped::Unseen),
}
}

/// What a pane was doing when a readiness barrier gave up — the diagnostic half of
/// [`PaneError::NeverReady`].
///
/// # ⚠⚠ Three states, because an `Option<String>` spelled two of them the same
///
/// *"This build cannot see the process table"* and *"this pane's child has exited"* are opposite
/// things to tell a caller: the first is about their DEPLOYMENT and the second about their PANE.
/// Carried as one `None`, a pane that died mid-wait reported the first — a false statement about a
/// build that was working perfectly.
/// The names a foreground job's LEADER answers to, and the one place that decides whether it
/// answers to a given one.
///
/// # ⚠⚠ Two names, because the two sources honestly disagree
///
/// The KERNEL's name for a process is the basename of the file it exec'd, capped (15 bytes on
/// Linux, `MAXCOMLEN` on macOS) and rewritable by the process itself. `argv[0]` is what its PARENT
/// called it. They are different facts and they diverge in ordinary cases, not exotic ones:
///
/// * `exec awk` where `/usr/bin/awk` is `mawk` gives a leader the kernel calls `mawk` whose
///   `argv[0]` is `awk`, and a caller who wrote `awk` is not wrong;
/// * `/bin/sh` on macOS is `bash`, so the SAME pane, spawned the same way, has a leader the kernel
///   calls `bash` there and `sh` on Linux.
///
/// # ⚠⚠ Why this is a TYPE and not two comparisons
///
/// [`ReadyWhen::Runs`] accepted EITHER name — it has to, or a caller would
/// have to know which spelling their platform packages — while [`PaneDoing::Job`] reported only the
/// kernel's. So the refusal named a program the caller never launched (`"bash"` for a pane they
/// opened as `/bin/sh`) and named a DIFFERENT one on each platform, which is how a gate over it came
/// to assert a shell's spelling and fail on the other runner. Matching and reporting read different
/// halves of one fact because they were different code. Here they are one type, so they cannot
/// disagree again.
///
/// ⚠ EXACT, never a prefix. A prefix match is a silent merge — `claude` accepting `claude-relay` is
/// a run that drives the wrong program and reports success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobLeader {
    /// What the KERNEL calls it. Always present: every process has one.
    kernel: String,
    /// The basename of `argv[0]` — what the leader's parent called it — when it has one.
    ///
    /// `None` is a FACT and not a failure: a zombie's argv is released while its entry lives on,
    /// and a kernel thread never had one. The basename rather than the whole word so that
    /// `/usr/local/bin/claude` and `claude` are one answer, which is the same rule
    /// [`launch_args`](../../sprag_host/hooks/fn.launch_args.html) decides an agent by.
    invoked: Option<String>,
}

impl JobLeader {
    /// Read both names off a job's leader.
    #[must_use]
    pub fn of(process: &JobProcess) -> Self {
        Self {
            kernel: process.name.clone(),
            invoked: process.argv.first().and_then(|arg0| {
                std::path::Path::new(arg0)
                    .file_name()
                    .map(|base| base.to_string_lossy().into_owned())
            }),
        }
    }

    /// A leader that answers to exactly ONE name.
    ///
    /// For a caller holding a name with no process table behind it — and it is deliberately the
    /// impoverished case rather than the normal one: [`of`](Self::of) is how a real job is read, and
    /// a leader built here cannot answer to the OTHER spelling because nothing told it one.
    #[must_use]
    pub const fn known_as(kernel: String) -> Self {
        Self {
            kernel,
            invoked: None,
        }
    }

    /// Whether this leader answers to `want` — **the whole of what `Runs` decides**.
    #[must_use]
    pub fn answers_to(&self, want: &str) -> bool {
        self.kernel == want || self.invoked.as_deref() == Some(want)
    }

    /// The spelling to LEAD a report with: what the leader was invoked as, else the kernel's name.
    ///
    /// `argv[0]` first because it is the caller's own vocabulary — the word they typed or the path
    /// they launched — and a correction phrased in a word they never wrote is a correction they
    /// cannot act on. The kernel's name is not dropped; [`Display`](std::fmt::Display) carries it
    /// whenever the two disagree.
    #[must_use]
    pub fn named(&self) -> &str {
        self.invoked.as_deref().unwrap_or(&self.kernel)
    }
}

impl std::fmt::Display for JobLeader {
    /// The leader as a person reads it inside a failure sentence: `"sh"`, or `"sh" (which the
    /// kernel calls "bash")` when the two sources disagree.
    ///
    /// Both names when they differ, because either is a spelling
    /// [`ReadyWhen::Runs`] accepts, and a reader handed one of them cannot
    /// tell whether the other would have worked.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.named())?;
        match &self.invoked {
            Some(invoked) if invoked != &self.kernel => {
                write!(f, " (which the kernel calls {:?})", self.kernel)
            }
            _ => Ok(()),
        }
    }
}

sprag_vt::closed_set! {
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneDoing {
    /// A job owns the pane's terminal; this is the leader it is led by.
    Job(JobLeader) = (JobLeader::known_as(String::new())),
    /// The host CAN see the process table and nothing owns this pane's terminal — its child has
    /// exited, or the pane never had one.
    Nothing,
    /// This host has no view of the process table at all, so it cannot say.
    Unknown,
}
}

impl PaneDoing {
    /// The job's leader, when a job owns the terminal at all.
    ///
    /// The accessor a caller needs to ask the diagnostic a QUESTION — *"is the thing that owns my
    /// pane the thing I launched?"* — rather than compare it to a spelling. The two other arms are
    /// absences with no leader to hand back, and they are why this is an `Option` rather than a
    /// panic.
    #[must_use]
    pub const fn leader(&self) -> Option<&JobLeader> {
        match self {
            Self::Job(leader) => Some(leader),
            Self::Nothing | Self::Unknown => None,
        }
    }
}

impl std::fmt::Display for PaneDoing {
    /// The clause this becomes inside a [`PaneError::NeverReady`] sentence. ⚠ Each reads as the
    /// END of *"…, so nothing was injected"*, so each starts mid-sentence by design.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Job(leader) => write!(f, "; its terminal belonged to {leader} instead"),
            Self::Nothing => write!(
                f,
                "; nothing owned its terminal — the pane's child had gone"
            ),
            Self::Unknown => Ok(()),
        }
    }
}

impl std::fmt::Display for PaneError {
    /// ⚠⚠ THE SENTENCE AN AGENT READS. A run's failure is published as this text, and it was
    /// `format!("{e:?}")` — a Rust variant name and its debug payload, `Write("Broken pipe (os
    /// error 32)")`, reaching the one reader who cannot look up what a variant means. That is the
    /// leak R283 measured on the CLI, standing on the loop's own answer.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPane(id) => write!(f, "there is no pane {}", id.0),
            Self::Encode(key) => write!(f, "the key {key:?} has no bytes to send to a terminal"),
            Self::Write(why) => write!(f, "writing to the pane failed: {why}"),
            // ⚠⚠ IT SAYS WHY REFUSING IS THE SERVICE. A caller told only *"the program has
            // exited"* reads a run that gave up on a technicality; the wall is the part that makes
            // typing anyway the worse answer, and it is what stops them adding a retry.
            Self::PeerGone(id) => write!(
                f,
                "pane {}'s program has exited, so nothing was typed into it: a terminal nobody is \
                 reading takes about 16 KB and then blocks for ever, and the write cannot be \
                 cancelled — one more line here would strand every other writer to that pane",
                id.0,
            ),
            Self::Spawn(why) => write!(f, "the pane could not be started: {why}"),
            Self::NeverReady {
                wanted,
                instead,
                already_showing,
            } => {
                write!(
                    f,
                    "the pane never {}, which this run was told to wait for before driving it, so \
                     nothing was injected",
                    wanted.describe(),
                )?;
                write!(f, "{instead}")?;
                // ⚠⚠⚠ LAST, AND ONLY WHEN IT IS TRUE. This is the clause that ends the caller's
                // search, so it goes where a sentence puts its point — and a `prints` that failed
                // for any other reason must not be handed a correction that does not apply to it.
                if *already_showing {
                    write!(
                        f,
                        "; but {:?} is already on its screen — {:?} counts only what a pane prints \
                         AFTER a run starts watching it, so a caller who meant the text that is \
                         there wants {:?}",
                        wanted.marker(),
                        wanted.word(),
                        ReadyWhen::Shows(String::new()).word(),
                    )?;
                }
                Ok(())
            }
            Self::NeverTook { attempts, written } => {
                write!(
                    f,
                    "the prompt could not be read back off the pane: {attempts} injections put \
                     {written} bytes on its pseudoterminal and none of them CHANGED it into a \
                     screen carrying the confirmation, so nothing was submitted and no reply is \
                     this run's. Two panes answer this with the text plainly arrived: one too \
                     narrow to carry the confirmation on one row, and one whose screen never moved \
                     at all — a peer that took the bytes and painted nothing"
                )
            }
            Self::NeverSubmitted {
                attempts,
                written,
                wanted,
            } => {
                write!(
                    f,
                    "the prompt reached the pane and the submit after it showed nothing: \
                     {attempts} injections put {written} bytes on its pseudoterminal, the text was \
                     read back off a screen this delivery changed, and then the pane {} inside the \
                     window this run allowed. The prompt is therefore sitting in the pane — a \
                     composer holding an unsent question — and nothing pressed again, because a \
                     second submit onto a composer the first one emptied asks an empty one",
                    wanted.describe(),
                )
            }
            Self::Undrivable(why) => {
                write!(f, "this run's machine could not be driven on: {why}")
            }
            Self::NotStopped(why) => {
                write!(
                    f,
                    "the pane's job was not stopped, and is still running: {why}"
                )
            }
        }
    }
}

/// The plugin extension API: a plugin's view of the core's panes.
pub trait PaneAccess {
    /// The ids of the live panes, in order.
    fn pane_ids(&self) -> Vec<PaneId>;

    /// The pane's collapsed screen text (each row trailing-trimmed, rows joined
    /// without separators) — the read for substring/sentinel matching across
    /// wrapped lines. `None` if no pane has that id.
    fn pane_collapsed(&self, id: PaneId) -> Option<String>;

    /// The pane's screen as per-row `(generation, text)`, read in one snapshot.
    /// `None` if no pane has that id.
    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>>;

    /// Whether the pane's child has closed its PTY (exited): no more output is
    /// coming and every byte it produced has already been applied to the
    /// screen. `None` if no pane has that id. This is the race-free completion
    /// signal a one-shot adapter (a tool that replies then exits) converges on.
    fn pane_eof(&self, id: PaneId) -> Option<bool>;

    /// The pane's full output text: scrolled-off lines (scrollback) then the
    /// visible rows, trailing blank lines stripped, joined by `"\n"`. `None` if
    /// no pane has that id. Unlike `pane_rows`/`pane_collapsed` (visible screen
    /// only), this captures output longer than the grid — a scrolled AI reply.
    fn pane_full_text(&self, id: PaneId) -> Option<String>;

    /// The pane's full output as the LOGICAL LINES THE CHILD WROTE — one entry per line however
    /// the width broke it. `None` if no pane has that id.
    ///
    /// # ⚠⚠ The CONTENT question, where [`pane_full_text`](Self::pane_full_text) is the RENDERED one
    ///
    /// Same pane, same output, two answers, and which one a caller wants is decided by what the
    /// caller PROMISES its own reader. `read_pane` promises *"what a human sees in that pane"* and
    /// takes the rendered one. Anything that publishes a model's words, matches a marker or relays
    /// to a peer is asking about CONTENT — and **the width belongs to whichever client attached**,
    /// so a rendered answer makes those depend on somebody else's window size.
    ///
    /// Defaults to the rendered text SPLIT BACK into lines, so a host that has not implemented it
    /// degrades to the old answer rather than to nothing — named as a degradation, exactly as the
    /// no-stream fallbacks in this crate are.
    fn pane_full_lines(&self, id: PaneId) -> Option<Vec<String>> {
        Some(
            self.pane_full_text(id)?
                .lines()
                .map(ToOwned::to_owned)
                .collect(),
        )
    }

    /// Inject `keys` into the pane, returning what was WRITTEN to its pseudoterminal.
    ///
    /// **Success is not delivery.** [`Written`] says so in its name and its docs say why; a caller
    /// that needs the pane to have taken the input wants [`deliver`](crate::deliver::deliver),
    /// which is this call plus the read-back that confirms it.
    ///
    /// # Errors
    ///
    /// [`PaneError`] when the pane is unknown, a key cannot be encoded, or
    /// the write fails.
    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError>;

    /// The pane *lifecycle* surface (spawn/close), if this implementation
    /// supports it. `None` by default — read/inject plugins never need it, so
    /// they (and test doubles) pay nothing; a plugin that manages panes (e.g.
    /// an AI dialogue spawning one pane per turn) asks for it and fails cleanly
    /// when it is absent. Kept a separate sub-trait so [`PaneAccess`] stays the
    /// read/inject surface (interface segregation).
    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        None
    }

    /// The pane *raw-output capture* surface, if this implementation supports it.
    /// `None` by default — only a plugin that parses structured machine output (a
    /// `claude --output-format json` envelope the grid would corrupt) needs the
    /// source bytes, so read/inject plugins and test doubles pay nothing AND
    /// cannot reach raw bytes at all: the scene-as-data invariant ("a plugin
    /// reads structured screen data, never raw bytes") is then enforced by the
    /// type, not a doc comment. Mirrors [`lifecycle`](PaneAccess::lifecycle).
    fn raw_capture(&self) -> Option<&dyn PaneRawCapture> {
        None
    }

    /// The pane *supervision* surface — what the AGENT in a pane is doing — if this host has a
    /// detector. `None` by default, and the absence is an answer: see [`PaneSupervision`].
    fn supervision(&self) -> Option<&dyn PaneSupervision> {
        None
    }

    /// The pane's *echo trail* — what has recently been WRITTEN INTO it — if this host records
    /// one. `None` by default, on the same terms as the other three sub-surfaces: only a consumer
    /// that must tell a pane's own echo from what the program said asks for it.
    ///
    /// ⚠ A `None` here is *"this build cannot tell echo from output"*, and a consumer of it must
    /// degrade in the SAFE direction rather than assume everything it sees is the program's.
    fn input_echo(&self) -> Option<&dyn PaneInputEcho> {
        None
    }

    /// What this pane's TERMINAL does with what is written into it — see [`PaneTerminalModes`].
    /// `None` by default, on the same terms as the other sub-surfaces: a host that cannot ask its
    /// device says so rather than guessing.
    fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
        None
    }

    /// The pane's *foreground job* — WHICH PROGRAM owns its terminal — if this host can see the
    /// process table. `None` by default, on the same terms as the other four sub-surfaces.
    ///
    /// ⚠ A `None` here is *"this build cannot say what a pane is running"*, and a consumer of it
    /// must fail in the SAFE direction: [`ReadyWhen::Runs`] treats it as not-ready rather than as
    /// ready, because the alternative is typing at whatever is there.
    fn foreground_job(&self) -> Option<&dyn PaneForegroundJob> {
        None
    }

    /// The pane's *output stream* — the COMPLETE logical lines it has produced, by absolute line
    /// number. `None` by default, on the same terms as the other five sub-surfaces.
    ///
    /// ⚠ A consumer without it must fall back to comparing the RENDERING, which is what
    /// [`RowTrail`] does and what its documentation is about. That fallback is a degradation, not
    /// an equivalent: it cannot see a line that scrolled away, and a genuine RE-WRAP changes the
    /// rows under it.
    fn output_lines(&self) -> Option<&dyn PaneOutputLines> {
        None
    }

    /// The pane's *hands* — WHO has written into it, and how often — if this host records them.
    /// `None` by default, on the same terms as the other sub-surfaces.
    ///
    /// ⚠ A `None` here is *"this build cannot say whose keystrokes these were"*, and a consumer
    /// must degrade in the SAFE direction — which for this surface means **carrying on**, not
    /// stopping. An absence is not evidence that somebody is present, and a run that read it as one
    /// would refuse to drive any pane on a host without the capability.
    fn hands(&self) -> Option<&dyn PaneHands> {
        None
    }

    /// The pane's *job control* — the ability to STOP what its terminal belongs to. `None` by
    /// default, on the same terms as the other six sub-surfaces.
    ///
    /// ⚠ A consumer without it has NO safe fallback, and that asymmetry is why this is its own
    /// capability rather than an assumed one. Every other absence here degrades to reading
    /// something less exact; this one degrades to writing `0x03` and hoping, which is not a
    /// degradation of stopping a job — it is a different act with a different outcome. A run that
    /// cannot reach this must report that it could not stop its work rather than pretend it did.
    fn job_control(&self) -> Option<&dyn PaneJobControl> {
        None
    }
}

/// Pane *output stream*: the complete logical lines a pane has produced, addressed by a number
/// that survives a resize. Reached via [`PaneAccess::output_lines`].
///
/// # ⚠⚠ Why a relay cannot be built on the grid
///
/// A pane's grid is a RENDERING of its output at the width it currently has. Reading *what did this
/// pane produce* off it means reading rows, and a row is not something the child made: a **resize**
/// re-wraps and renumbers every one, a **repaint** changes none of them, and **scrolling** drops
/// the ones nobody came back for. Each of those was measured here as a live defect — a relay that
/// re-injected a whole screen because a client attached, an agent adapter that published that
/// screen as the model's reply.
///
/// A LOGICAL line IS what the child produced, and reflow is defined as preserving it. Numbering
/// those lines from the pane's birth turns a cursor into an ADDRESS, so a consumer can say *"I have
/// had everything up to line N"* and be given the rest EXACTLY ONCE — however many times the rows
/// carrying it are re-wrapped or repainted in between.
pub trait PaneOutputLines {
    /// The complete logical lines `id` has produced after absolute line `cursor`, with how many
    /// were lost first. `None` for a pane nobody knows.
    ///
    /// ⚠ A loss is COUNTED rather than hidden: retained history is bounded, and a silent gap is
    /// indistinguishable from a quiet source.
    fn pane_lines_since(&self, id: PaneId, cursor: u64) -> Option<LinesSince>;
}

/// Pane *foreground job*: which program owns a pane's terminal right now. Reached via
/// [`PaneAccess::foreground_job`].
///
/// # ⚠⚠ Why a readiness question had to leave the screen entirely
///
/// Three rounds of this crate's history are one predicate being narrowed: *has the program in this
/// pane started yet?*, asked of the SCREEN. The screen cannot answer it, and each fix moved the
/// failure rather than removing it.
///
/// * A whole-screen match said yes to **the echo of the command line that started the program** —
///   text that is on screen before the program exists.
/// * A damage-generation baseline made that echo count only if it landed after the barrier armed,
///   and a pty's echo is ASYNCHRONOUS: the same call converged or fed the shell depending on how
///   loaded the machine was.
/// * Refusing any marker found in the pane's own echo trail made the answer deterministic — and
///   left a caller with **no way at all to wait for a program that prints nothing on startup**,
///   which is most REPLs, most relays, and any tool that speaks only when spoken to.
///
/// The last one is not a gap in the fix; it is the shape of the question. **A silent program has
/// no marker, so no predicate over its output can ever fire.** Meanwhile the operating system has
/// known the answer the whole time: a shell hands its terminal to the job it runs
/// (`tcsetpgrp`) and takes it back when that job ends. That fact is not text, cannot be echoed,
/// cannot be printed by a program pretending to be another, and does not depend on when a byte
/// reached a grid.
///
/// So this asks it. [`ReadyWhen::Runs`] is the only readiness kind that never reads the screen, and
/// it is the one to prefer.
pub trait PaneForegroundJob {
    /// The LEADER of `id`'s foreground job, or `None` for a pane nobody knows, a pane whose child
    /// has exited, or a terminal no job owns.
    ///
    /// The leader rather than every member, because this is polled: see
    /// [`foreground_leader_of`] for what that costs and what it therefore cannot answer.
    fn pane_foreground_leader(&self, id: PaneId) -> Option<JobProcess>;
}

/// STOPPING a pane's foreground job — the WRITE half of what [`PaneForegroundJob`] reads. Reached
/// via [`PaneAccess::job_control`].
///
/// # ⚠⚠⚠ Why [`inject`](PaneAccess::inject) was not already this
///
/// A plugin CAN write `0x03` into a pane, and until this existed that was the only stop it had. It
/// is not one. The byte becomes a `SIGINT` only if the terminal's line discipline is willing —
/// `stty -isig`, a full-screen editor, any program that took the terminal raw, and the byte is
/// ordinary input — and it goes to whichever group owns the terminal at the instant the kernel
/// processes it, which is not necessarily the one the plugin meant. **Measured: a pane running
/// `stty -isig; sleep 300` echoes `^C` and keeps sleeping.** See
/// [`sprag_terminal::stop`](../../sprag_terminal/stop/index.html) for the whole measurement.
///
/// So a bounded run could not keep its own promise. `max_duration` stopped the DRIVER on time and
/// left the peer's turn running — the loop's door closed on a room that was still occupied — and no
/// amount of care inside the loop could fix it, because the missing thing was an address, not a
/// policy. This is that address.
///
/// # Why it is a separate capability and not a method on [`PaneAccess`]
///
/// Interface segregation, the same reason [`RunContext`](crate::run::RunContext) is not bolted onto
/// this trait: a plugin that only READS panes must not depend on the ability to signal the programs
/// in them, and a host that cannot signal (a projection, a replay, a remote view) must be able to
/// say so by not offering the capability rather than by returning a lie.
pub trait PaneJobControl {
    /// Send `stop` to the job that owns `id`'s terminal, and say what received it.
    ///
    /// # Errors
    ///
    /// [`PaneError::UnknownPane`] for a pane nobody knows, and [`PaneError::NotStopped`] when
    /// there was no group to signal or the kernel refused — ⚠ in which case the work is STILL
    /// RUNNING, which is the whole reason this answers instead of returning nothing.
    fn pane_stop_job(&self, id: PaneId, stop: Stop, reach: Reach) -> Result<Signalled, PaneError>;
}

/// WHAT A STOP REACHED, as a plugin's caller reads it.
///
/// [`sprag_terminal::StoppedJob`] with its leader read through [`JobLeader`] — the same two-name
/// reading every other report on this surface uses, so a stop and a readiness refusal name one
/// program the same way. A report that spelled the leader differently from the barrier that had
/// been waiting for it would be two vocabularies for one fact, which is the defect `JobLeader`
/// exists to have removed once.
///
/// [`sprag_terminal::StoppedJob`]: ../../sprag_terminal/stop/struct.StoppedJob.html
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signalled {
    /// Which request was delivered.
    pub stop: Stop,
    /// The process GROUP that received it.
    pub pgid: u32,
    /// The names its leader answers to, or `None` when the group's leader has already gone and its
    /// other members keep the group alive — an absence with its own meaning, not a failure.
    pub leader: Option<JobLeader>,
}

impl Signalled {
    /// Read a terminal-layer answer as this one.
    #[must_use]
    pub fn of(job: &StoppedJob) -> Self {
        Self {
            stop: job.stop,
            pgid: job.pgid,
            leader: job.leader.as_ref().map(JobLeader::of),
        }
    }
}

impl std::fmt::Display for Signalled {
    /// The clause a run's outcome carries: *`interrupted "claude" (process group 4711)`*.
    ///
    /// ⚠ The GROUP is printed and not only the name, because the name is a courtesy and the group
    /// is the address — it is what a person types into `kill` when they want to check, and a report
    /// that named only a program leaves them nothing to verify with.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.stop)?;
        match &self.leader {
            Some(leader) => write!(f, " {leader} (process group {})", self.pgid),
            None => write!(f, " process group {}", self.pgid),
        }
    }
}

/// Pane *echo trail*: the text recently written INTO a pane, for telling the pane's own echo from
/// what the program in it actually said. Reached via [`PaneAccess::input_echo`].
///
/// # ⚠⚠ Why this is a capability and not a caller's own bookkeeping
///
/// **A pseudoterminal echoes what is written to it, and on the grid that echo is
/// indistinguishable from program output.** Two plugins already answer *"is this my own input
/// coming back?"* by comparing against what THEY just wrote — [`Orchestrator::reaction`] and
/// [`Pipe::shown`] — and that works because they did the writing.
///
/// A readiness barrier cannot do that. It waits for a program SOMEBODY ELSE started, and the text
/// it must not be fooled by is the command line that caller typed. Without a record of it, the
/// barrier's answer depended on whether the echo happened to reach the grid before or after the
/// barrier started looking: **the same call converged or drove into a shell depending on
/// scheduling.** Measured, and no amount of waiting closes it — which is why the fact had to
/// become a capability the pane offers rather than a caller's private note.
///
/// [`Orchestrator::reaction`]: crate::orchestrator::Orchestrator
/// [`Pipe::shown`]: crate::pipe::Pipe
pub trait PaneInputEcho {
    /// The text recently written into `id`, or `None` for a pane nobody knows.
    ///
    /// Bounded and lossy-UTF-8 by construction — it is compared against SCREEN TEXT, and the
    /// screen is text.
    fn pane_recent_input(&self, id: PaneId) -> Option<String>;
}

/// Pane *hands*: WHO has written into a pane and how many times each. Reached via
/// [`PaneAccess::hands`].
///
/// # ⚠⚠⚠ The question the echo trail beside it cannot answer
///
/// [`PaneInputEcho`] records WHAT was written and is deliberately one stream, because its consumers
/// are matching text against a screen. That made it useless for a different question the product
/// could not otherwise ask: **has a PERSON reached into this pane?**
///
/// The two hands are indistinguishable in the trail by construction — `sprag_host::pane::send_key`
/// is one encoder shared by a display client's keyboard and the `scene/invoke` wire, on purpose. So
/// the fact had to be recorded at the write rather than recovered afterwards, which is what
/// [`Hands`] is.
///
/// # ⚠⚠ Why a caller keeps its own watermark instead of asking *"since when"*
///
/// The counts are monotonic and the pane holds no reader state, so several readers can each ask
/// their own *"since I last looked"* without coordinating — and a reader that never looks costs
/// nothing. A `since(instant)` API would have made the pane remember on everyone's behalf and
/// answer a different question depending on how many were asking.
pub trait PaneHands {
    /// How many times each hand has written into `id`, or `None` for a pane nobody knows.
    fn pane_hands(&self, id: PaneId) -> Option<Hands>;
}

/// What a pane's TERMINAL does with what is written into it — the kernel's answers, not guesses.
/// Reached via [`PaneAccess::terminal_modes`].
///
/// # ⚠⚠ Why this is not on [`PaneInputEcho`], which it was for one round
///
/// The trail beside it is what SPRAG REMEMBERS writing; these are what THE KERNEL will do with it.
/// Merging them read well while there was one of them — both were about an echo — and stopped the
/// moment a second question arrived, because *"will a `Ctrl-D` end this program's input?"* is not a
/// fact about an echo trail under any reading. **A trait whose name stops describing its methods is
/// the defect this workspace's own doc gate refuses one layer down**, so the two subjects were
/// split rather than the name stretched.
///
/// Every answer is `Option`, and `None` is always *this platform's device would not say* — never
/// the negative. A caller that reads an absence as a NO manufactures the exact confidence these
/// exist to withhold.
pub trait PaneTerminalModes {
    /// WHO will put a pane's own input back on its screen.
    ///
    /// # ⚠⚠⚠ The question a read-back cannot answer for itself
    ///
    /// With [`PaneEcho::ByTheTerminal`] the line discipline paints the text the instant it reaches
    /// the device, **before the program has read a byte and whether or not it ever does** — so
    /// finding it there is evidence about the terminal. Measured:
    /// [`deliver`](crate::deliver::deliver) reported a CONFIRMED delivery into a pane running
    /// `sleep 60`, in 20 ms.
    fn pane_echo(&self, id: PaneId) -> Option<PaneEcho>;

    /// Whether a `Ctrl-D` written into the pane ENDS its program's input, or arrives as a byte.
    ///
    /// # ⚠⚠⚠ The wait this answers before it is spent
    ///
    /// An end-of-input is a CONDITION the line discipline raises, and only in canonical mode. A
    /// caller that ends a question with `Ctrl-D` and then waits for the peer to finish is, on a raw
    /// pane, waiting for something it never asked for. Measured on `stty raw -echo; exec cat`: the
    /// run spent its whole reply timeout and explained itself with *"the peer had not finished"* —
    /// a sentence about the PEER's speed for a cause that was the TERMINAL's mode, and knowable
    /// before the wait began.
    fn pane_end_of_input(&self, id: PaneId) -> Option<PaneEndOfInput>;
}

/// Which authority a pane's [`AgentState`] came from, and so how much it is worth.
///
/// A supervisor that cannot tell these apart is using an approximation as if it were exact. The
/// two are not degrees of the same evidence — they are different KINDS, reached by different
/// machinery, and a consumer choosing a poll interval or deciding whether to trust a turn boundary
/// needs to know which it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    /// A process INSIDE the pane said so — the agent's own hook, reporting the turn boundary it
    /// alone knows. Exact: nothing was sampled and nothing can have been missed between samples.
    /// The string is who said it.
    Reported { source: String },
    /// A rule read it off the pane's screen and title. Approximate by construction: the working
    /// signal is an ANIMATION, so a sample can land in its gap, and a state that flips twice
    /// between two looks is a state neither look saw. The string is which rule fired.
    ///
    /// `rule` is an `Option` because the field it is built from is one, and it is not reachable
    /// today: a pane whose manifest claims it but whose rules all miss reads `Unknown`, and an
    /// observation is never produced for a pane with no state. Stated rather than made
    /// unrepresentable because the alternative — a second shape for "scraped, and I cannot say
    /// which rule" — would be a state a future publisher could reach with nowhere to put it.
    Scraped { rule: Option<String> },
}

impl Authority {
    /// Whether this answer came from the pane itself, and so has no sampling gap in it.
    ///
    /// The one question a supervisor must ask before treating a state as a turn BOUNDARY rather
    /// than as a description of right now.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Reported { .. })
    }
}

/// What the agent in one pane is doing, read as a LEVEL.
///
/// Everything here is answered by a pull, deliberately. A supervisor driven by state-change EVENTS
/// loses any turn shorter than the gap between two of them — measured against a rival, where a
/// one-second agent turn produced no event at all and the supervising machine waited forever for a
/// turn that had already finished. A level cannot be lost that way: whatever the pane is doing when
/// you ask is what you are told.
///
/// [`seq`](Self::seq) is what recovers the part of an edge stream that is worth having. It counts
/// PUBLISHED CHANGES, so two pulls that both read `Idle` while the number moved by two say a turn
/// began and ended in between — the transition a poll could not see, carried as a level and
/// therefore not lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentObservation {
    /// What the pane is doing now.
    pub state: AgentState,
    /// Which agent it is, `None` when a rule fired without one being identified.
    pub agent: Option<String>,
    /// Where the answer came from, and so whether it is exact — see [`Authority`].
    pub authority: Authority,
    /// How many published CHANGES this pane's state has been through. Never decreases while the
    /// pane lives; compare it across two pulls to learn that something happened between them.
    pub seq: u64,
    /// What the pane is blocked ON, when it is blocked and the question is on its screen.
    ///
    /// Populated only for [`AgentState::Blocked`], because that is the only state in which the
    /// menu on the screen is the thing the pane is waiting on — a menu still painted behind a
    /// working agent is scenery, and reporting it would invite a supervisor to answer a question
    /// nobody is asking.
    ///
    /// `None` on a blocked pane is a real case and not a defect: an agent can block on something
    /// that is not a numbered list, and a report can say `blocked` about a pane whose screen shows
    /// no menu at all. A supervisor that finds `None` here has to hand the pane to a person, which
    /// is the correct answer to a question it cannot read.
    pub asking: Option<Question>,
}

/// Pane *supervision*: what the AGENT in a pane is doing. Reached via
/// [`PaneAccess::supervision`], on the same terms as [`PaneLifecycle`] and [`PaneRawCapture`] —
/// only a plugin that supervises asks for it, so nothing else carries the dependency.
///
/// # Why the absence of the whole surface is an answer
///
/// [`PaneAccess::supervision`] returns `None` for a host with no detector at all, and
/// [`pane_agent_state`](Self::pane_agent_state) returns `None` for a pane no manifest claims. Those
/// are opposite instructions: the first says *ask a person, this build cannot supervise anything*,
/// and the second says *this pane is not an agent*. Collapsing them into one `None` would let a
/// supervisor conclude "no agents here" from a host that simply never looked.
pub trait PaneSupervision {
    /// What the agent in `id` is doing right now, or `None` for a pane no manifest claims (and for
    /// a pane id nobody knows).
    ///
    /// A LEVEL: safe to call as often as a plugin steps, and each answer stands on its own. The
    /// read is arbitrated by the host's one detector, so two plugins watching one pane can never
    /// disagree about it, and the host's quiescence gate means a pane whose screen has not moved
    /// costs no rule evaluation.
    fn pane_agent_state(&self, id: PaneId) -> Option<AgentObservation>;
}

/// Pane *lifecycle* control: spawn and close panes. The capability a plugin
/// needs to orchestrate one-shot tools across turns (each turn a fresh pane).
/// Reached via [`PaneAccess::lifecycle`] so it does not fatten the read/inject
/// surface every plugin depends on.
pub trait PaneLifecycle {
    /// Spawn a pane running `argv` (`[program, args…]`) at `cols × rows`,
    /// returning its [`PaneId`].
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when `argv` is empty or the pane cannot start.
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, PaneError>;

    /// Close (reap) the pane with `id`, returning whether it existed. The
    /// pane's blocking teardown runs outside any shared lock.
    fn close(&self, id: PaneId) -> bool;

    /// **REPLACE `id` WITH A FRESH PANE RUNNING THE SAME THING, IN THE SAME PLACE** — the same argv,
    /// the same environment, the same working directory and the same size — answering the new pane's
    /// [`PaneId`].
    ///
    /// # ⚠⚠⚠ Why this takes no argv, where [`spawn`](Self::spawn) does
    ///
    /// The caller that needs this is [`AiLoop`](crate::ai_loop::AiLoop)'s `restarting`: the things a
    /// loop wants to improve about its inner session — the agent's base context, which MCP servers
    /// load, `CLAUDE.md`, a memory index — are all read when a session STARTS, so a live session
    /// cannot be asked to re-read them and the loop closes it and opens a fresh one.
    ///
    /// A `respawn(id, argv)` would make the loop the authority on what its pane runs, and it is not:
    /// the pane was opened by somebody else, possibly with arguments nobody passed this run
    /// (`claude --model …`), in a working directory that is the whole point of the work. **The pane
    /// itself is the only authority on what it is running**, and this asks it. A loop that supplied
    /// an agent NAME instead would silently restart `claude` where a person had launched
    /// `claude --resume`, or in the daemon's directory rather than the repository.
    ///
    /// # ⚠⚠ The order, which is a promise: the new pane exists BEFORE the old one is closed
    ///
    /// A spawn can fail — a program that has been uninstalled, a full process table. Closing first
    /// would leave a run with no pane at all and nothing to report it against; this way a failed
    /// replacement leaves the caller exactly where it was, holding a pane it can still read.
    ///
    /// # ⚠⚠ What it does NOT carry, stated because each one matters to somebody
    ///
    /// * the new pane's POSITION in a window's layout — it arrives wherever a newly spawned pane
    ///   arrives, so a person watching sees the session they were reading appear somewhere else;
    /// * the pane's resource `grant`, its `name`, and its `opened_by` PROVENANCE. The last is the
    ///   sharpest: a replacement is a pane NOBODY CLAIMS, so an agent surface that refuses to close
    ///   panes its caller did not open will refuse the run's own inner session to the agent that
    ///   started it.
    ///
    /// The four things it does carry are what make it *the same command*; these are what would make it
    /// *the same pane to everybody else*, and they are registered rather than guessed at.
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when there is no such pane, when it has no argv to re-run (a pane
    /// restored from a snapshot older than argv capture), or when the fresh pane cannot start.
    fn respawn(&self, id: PaneId) -> Result<PaneId, PaneError>;
}

/// Pane *raw-output* capture: the child's **source** bytes, before the emulator
/// renders them onto the grid. Reached via [`PaneAccess::raw_capture`].
///
/// Kept a separate sub-trait (like [`PaneLifecycle`]) so [`PaneAccess`] stays
/// the structured scene-as-data surface. The `pane_*_text` family returns the
/// *rendered grid* (wrapped to the pane width, trailing-trimmed, control-
/// stripped — a lossy projection for display); this returns the *exact bytes*
/// the child emitted, the read for **structured machine output** (a single-line
/// JSON envelope a long reply wraps across rows, which the grid's wrap-`\n`
/// insertion and trailing-trim would corrupt). Only a plugin that parses such
/// output asks for it, so the "structured data, never raw bytes" invariant is
/// enforced by the type rather than by convention.
pub trait PaneRawCapture {
    /// The pane child's raw output bytes (the source stream, before emulation),
    /// or `None` if no pane has that `id`. A truncated [`RawOutput`] is an
    /// incomplete capture, and a structured read should degrade.
    fn pane_raw_output(&self, id: PaneId) -> Option<RawOutput>;
}

/// A MINTER for one pane's attention hook: called per birth, answering a hook that pane owns.
///
/// A named type because the shape is genuinely three-deep (`Arc<dyn Fn() -> Box<dyn Fn(..)>>`) and
/// reads as noise inline — and because the field it fills and the builder
/// ([`WorkspacePaneAccess::with_attention`]) that sets it must say the same thing.
///
/// **Why a minter and not one shared closure**: the hook the daemon hands out owns a channel sender
/// per pane, and asking for it per birth is what keeps the PTY reader thread that runs it from taking
/// a lock. This layer expresses that as *"give me a hook"* without knowing why — the same opaque
/// discipline the pane-exit death signal follows.
pub type AttentionMinter = Arc<dyn Fn() -> Box<dyn Fn(PaneId, Attention) + Send> + Send + Sync>;

/// A reader for one pane's agent state — the daemon's detector, handed in as an opaque `Fn`.
///
/// The same discipline [`AttentionMinter`] and the pane-exit signal follow, and here it carries one
/// more argument. The memory a verdict comes out of is per-DAEMON and lives beside the session
/// tree; this layer is session-tree-free by decision (R144). An `Fn(PaneId) -> Option<_>` lets a
/// plugin read the daemon's ONE arbitration without this crate learning that a registry, a settle
/// window or a manifest file exists — and it keeps the alternative unavailable, which matters: a
/// plugin holding its own detector would be a second authority answering the same question about
/// the same pane, free to disagree with the pane list a person is looking at.
pub type AgentStateSource = Arc<dyn Fn(PaneId) -> Option<AgentObservation> + Send + Sync>;

/// [`PaneAccess`] over a shared [`Workspace`] — the production implementation.
pub struct WorkspacePaneAccess {
    workspace: Arc<Mutex<Workspace>>,
    /// An OPAQUE hook run once when a pane this surface [`spawn`](PaneLifecycle::spawn)ed
    /// exits (the daemon's reaper death-signal), or `None`. Deliberately a bare
    /// `Fn` and NOT the registry: the plugin layer stays session-tree-free (Interface
    /// Segregation, the R144 decision) while a plugin-spawned pane still feeds the daemon's
    /// self-cleaning exactly like a mux one — this layer never learns what the hook does.
    /// Set only by the host's plugin surface via [`with_pane_exit`](Self::with_pane_exit); the
    /// default is `None`, so nothing but the daemon wires it.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// A MINTER for the daemon's attention hook: called once per pane this surface spawns to get
    /// that pane its own `on_attention`. Opaque, exactly as [`Self::on_pane_exit`] is — this layer
    /// never learns what the hook does, so a plugin-spawned pane whose child asks for a person
    /// reaches that person like a mux-spawned one. Wired by the host's plugin surface, `None`
    /// everywhere else.
    ///
    /// **A minter and not one shared closure**, because the hook the daemon hands out owns a channel
    /// sender per pane; asking for it per birth is what keeps the PTY reader thread that runs it from
    /// taking a lock. This layer expresses that as *"give me a hook"* without knowing why.
    ///
    /// **A separate signal and not a second use of the exit hook**, because they answer different
    /// questions about different moments — and because a pane category quietly left out of one of
    /// them is exactly the shape the notification path was in: every layer carrying the fact and one
    /// surface obliged to read it.
    on_attention: Option<AttentionMinter>,
    /// The daemon's agent-state reader ([`AgentStateSource`]), or `None` for a host with no
    /// detector — a GUI's in-process host, a test double. Opaque exactly as its two neighbours
    /// are.
    ///
    /// Its absence is what [`PaneAccess::supervision`] reports, so "this build cannot supervise"
    /// and "this pane is not an agent" stay different answers all the way out to the plugin.
    agent_state: Option<AgentStateSource>,
}

impl WorkspacePaneAccess {
    /// Wrap a shared workspace as the plugin pane-access surface (no pane-exit hook — see
    /// [`with_pane_exit`](Self::with_pane_exit)).
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        Self {
            workspace,
            on_pane_exit: None,
            on_attention: None,
            agent_state: None,
        }
    }

    /// Attach the daemon's opaque pane-exit death-signal, so a pane this surface spawns feeds
    /// the reaper on its death. A builder (not a `new` parameter) so the many non-daemon
    /// constructors — plugin machinery, tests — stay untouched and pass nothing.
    #[must_use]
    pub fn with_pane_exit(mut self, hook: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        self.on_pane_exit = hook;
        self
    }

    /// Attach the daemon's opaque ATTENTION signal, so a pane this surface spawns can ask for a
    /// person. A builder for [`with_pane_exit`](Self::with_pane_exit)'s reason.
    #[must_use]
    pub fn with_attention(mut self, mint: Option<AttentionMinter>) -> Self {
        self.on_attention = mint;
        self
    }

    /// Attach the daemon's agent-state reader, so a plugin can supervise what the agents in its
    /// panes are doing. A builder for [`with_pane_exit`](Self::with_pane_exit)'s reason, and
    /// passing `None` leaves [`PaneAccess::supervision`] answering that this host cannot.
    #[must_use]
    pub fn with_agent_state(mut self, source: Option<AgentStateSource>) -> Self {
        self.agent_state = source;
        self
    }

    /// Clone the pane's I/O handle under the workspace lock (released before
    /// the handle is used), so screen reads / writes never hold the workspace
    /// lock.
    ///
    /// ⚠ `pub(crate)` for the fixtures' sake: a gate about a PERSON typing into a pane has to
    /// reach the door a display client writes through ([`PanePtyHandle::write`]), because the
    /// injection API above is the door the RUN writes through and a fixture that uses it is
    /// staging the person out of the very distinction under test.
    pub(crate) fn handle(&self, id: PaneId) -> Option<PanePtyHandle> {
        lock(&self.workspace).pane(id).map(Pane::handle)
    }
}

impl PaneAccess for WorkspacePaneAccess {
    fn pane_ids(&self) -> Vec<PaneId> {
        lock(&self.workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.with_screen(read_collapsed))
    }

    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
        Some(self.handle(id)?.with_screen(read_rows))
    }

    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        // A quick atomic load; reading it under the workspace lock (rather than
        // cloning the handle) is negligible and needs no producer change.
        lock(&self.workspace)
            .pane(id)
            .map(|pane| pane.pty().is_eof())
    }

    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.with_screen(Screen::full_text))
    }

    fn pane_full_lines(&self, id: PaneId) -> Option<Vec<String>> {
        Some(self.handle(id)?.with_screen(Screen::full_lines))
    }

    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
        // ⚠⚠⚠⚠ THE ONE DOOR, WHICH IS WHY THE REFUSAL IS HERE AND NOT IN A PLUGIN. Every plugin
        // that types reaches a pane through this function — the comment below has said so for as
        // long as it has existed — so one reading protects all of them, where a guard per plugin
        // would be four copies of a decision and a fifth plugin arriving unprotected.
        // ⚠⚠ It costs one `pane_eof` per injection: an atomic load under the workspace lock this
        // call already takes, which `PanePty::is_eof`'s own doc calls negligible.
        // ⚠ AFTER `UnknownPane` would be wrong-footed — a pane nobody knows is a different
        // sentence — so the handle is resolved first and the liveness asked second.
        if self.handle(id).is_some() && self.pane_eof(id) == Some(true) {
            return Err(PaneError::PeerGone(id));
        }
        let handle = self.handle(id).ok_or(PaneError::UnknownPane(id))?;
        let modes = handle.input_modes();
        let mut bytes = Vec::new();
        for stroke in keys {
            let encoded = encode(&stroke.key, stroke.mods, modes)
                .ok_or_else(|| PaneError::Encode(stroke.key.clone()))?;
            bytes.extend_from_slice(&encoded);
        }
        // ⚠⚠ A PROGRAM, always — this is the door a PLUGIN types through, and every caller of it is
        // a run driving a pane rather than somebody at a keyboard. The distinction is what lets a
        // run ask whether a PERSON has reached in ([`sprag_terminal::Hand`]); a plugin that stamped
        // its own injections as a person's would trip its own interruption check on the next step.
        handle
            .write(&bytes, sprag_terminal::Hand::AProgram)
            .map_err(|e| PaneError::Write(e.to_string()))?;
        Ok(Written::of(bytes.len() as u64))
    }

    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        Some(self)
    }

    fn raw_capture(&self) -> Option<&dyn PaneRawCapture> {
        Some(self)
    }

    fn input_echo(&self) -> Option<&dyn PaneInputEcho> {
        Some(self)
    }

    fn hands(&self) -> Option<&dyn PaneHands> {
        Some(self)
    }

    fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
        Some(self)
    }

    fn foreground_job(&self) -> Option<&dyn PaneForegroundJob> {
        Some(self)
    }

    fn output_lines(&self) -> Option<&dyn PaneOutputLines> {
        Some(self)
    }

    fn job_control(&self) -> Option<&dyn PaneJobControl> {
        Some(self)
    }

    fn supervision(&self) -> Option<&dyn PaneSupervision> {
        // Gated on the reader rather than answered unconditionally: a surface with no detector
        // behind it must say so, or every pane on a host that never looked reads as "not an agent".
        self.agent_state
            .is_some()
            .then_some(self as &dyn PaneSupervision)
    }
}

impl PaneSupervision for WorkspacePaneAccess {
    fn pane_agent_state(&self, id: PaneId) -> Option<AgentObservation> {
        (self.agent_state.as_ref()?)(id)
    }
}

impl PaneInputEcho for WorkspacePaneAccess {
    fn pane_recent_input(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.echo_trail())
    }
}

impl PaneHands for WorkspacePaneAccess {
    fn pane_hands(&self, id: PaneId) -> Option<Hands> {
        Some(self.handle(id)?.hands())
    }
}

impl PaneTerminalModes for WorkspacePaneAccess {
    fn pane_echo(&self, id: PaneId) -> Option<PaneEcho> {
        self.handle(id)?.echo()
    }

    fn pane_end_of_input(&self, id: PaneId) -> Option<PaneEndOfInput> {
        self.handle(id)?.end_of_input()
    }
}

impl PaneOutputLines for WorkspacePaneAccess {
    fn pane_lines_since(&self, id: PaneId, cursor: u64) -> Option<LinesSince> {
        Some(self.handle(id)?.lines_since(cursor))
    }
}

impl PaneJobControl for WorkspacePaneAccess {
    /// ⚠ THE PID IS TAKEN UNDER THE LOCK AND THE SIGNAL IS SENT OUTSIDE IT, for the reason the
    /// reader below states — and here it matters twice, because this is called from a run that is
    /// being cancelled, which is exactly when a client is also asking the workspace for the run's
    /// state.
    ///
    /// ⚠⚠ A pane whose child has been REAPED reads `None` for its pid — the gate
    /// [`PanePty::pid`](../../sprag_terminal/pane_pty/struct.PanePty.html#method.pid) already
    /// applies — and that is reported as [`Unstopped::Gone`] rather than as an unknown pane: the
    /// pane is still there, its program is not, and telling a caller their pane does not exist when
    /// it does would send them looking in the wrong place.
    fn pane_stop_job(&self, id: PaneId, stop: Stop, reach: Reach) -> Result<Signalled, PaneError> {
        let pid = {
            let workspace = lock(&self.workspace);
            let pane = workspace.pane(id).ok_or(PaneError::UnknownPane(id))?;
            pane.pty().pid()
        };
        let pid = pid.ok_or(PaneError::NotStopped(Unstopped::Gone))?;
        sprag_terminal::stop_foreground_job(pid, stop, reach)
            .map(|job| Signalled::of(&job))
            .map_err(PaneError::NotStopped)
    }
}

impl PaneForegroundJob for WorkspacePaneAccess {
    /// ⚠ THE PID IS TAKEN UNDER THE LOCK AND THE SYSCALLS RUN OUTSIDE IT — the `let` binding ends
    /// the guard's temporary before the read. R291 measured the other order on the settle sweep: a
    /// concurrent reader's median went from +0.8 us to +687 us and its p99 to +41.8 ms, because a
    /// holder doing I/O under the lock every client wake wants is a convoy. This is polled every
    /// [`POLL_INTERVAL`](crate::run::POLL_INTERVAL), which is exactly that shape.
    fn pane_foreground_leader(&self, id: PaneId) -> Option<JobProcess> {
        let pid = lock(&self.workspace)
            .pane(id)
            .and_then(|pane| pane.pty().pid())?;
        foreground_leader_of(pid)
    }
}

impl PaneRawCapture for WorkspacePaneAccess {
    fn pane_raw_output(&self, id: PaneId) -> Option<RawOutput> {
        Some(self.handle(id)?.raw_output())
    }
}

impl WorkspacePaneAccess {
    /// Spawn `argv` at `cols × rows`, in `cwd` when one is given — the body both
    /// [`spawn`](PaneLifecycle::spawn) and [`respawn`](PaneLifecycle::respawn) are, so a replacement
    /// pane is wired to the daemon's hooks exactly as a first one is.
    ///
    /// ⚠ ONE BODY AND NOT TWO, for the reason this crate keeps re-deriving: the interesting content
    /// here is the three opaque birth hooks, and a second copy of them is a second place for a
    /// plugin-spawned pane to stop feeding the reaper.
    fn spawn_in(
        &self,
        argv: &[String],
        cwd: Option<&std::path::Path>,
        env: &[(std::ffi::OsString, std::ffi::OsString)],
        cols: u16,
        rows: u16,
    ) -> Result<PaneId, PaneError> {
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| PaneError::Spawn("empty argv".to_string()))?;
        let mut command = CommandBuilder::new(program.as_str());
        for arg in rest {
            command.arg(arg.as_str());
        }
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        // The emulator parses (and strips) escape sequences, so captured cell
        // text stays clean regardless of TERM; match the host's spawn default.
        command.env("TERM", "xterm-256color");
        // ⚠⚠⚠ AND WHATEVER THE PANE BEING REPLACED WAS LAUNCHED WITH — after the default above, so a
        // recorded `TERM` wins over it and a caller who set none still gets one.
        //
        // A replacement that carried only the argv would be the same PROGRAM in a different WORLD, and
        // the live measurement in `sprag_host::live_agent` is what makes that concrete: it blanks nine
        // `CLAUDE_CODE_*` variables so the child is what a person gets from a terminal rather than a
        // NESTED agent session. A restart that dropped them would hand the replacement a mode only
        // that harness can produce, silently, and every reading after it would be of a different
        // program.
        for (key, value) in env {
            command.env(key, value);
        }
        // Carry the daemon's death-signal (if any) so a plugin-spawned pane feeds the reaper
        // exactly like a boot/mux one — the opaque hook is just a channel send, so this
        // registry-free layer wires it without learning what it does.
        let hooks = PaneBirthHooks {
            on_dirty: None,
            on_exit: self.on_pane_exit.as_ref().map(|hook| {
                let hook = Arc::clone(hook);
                Box::new(move || hook()) as Box<dyn Fn() + Send>
            }),
            // ...and its ATTENTION, on the same terms: opaque, registry-free, and minted for THIS
            // pane rather than shared with every other one.
            on_attention: self.on_attention.as_ref().map(|mint| mint()),
        };
        // Nothing here says where the pane's cgroup goes, and that is the point: the pool this
        // spawns into carries its window's lineage and the daemon's subtree, so a plugin-spawned
        // pane is weighted exactly like every other one (R337). It used to carry a `home: None` over
        // a comment saying "the host fills this in when it has a tree" — the host did no such thing
        // for this door, and the comment was the only thing that said otherwise.
        lock(&self.workspace)
            .spawn_with_dirty(command, program.clone(), cols, rows, hooks)
            .map_err(|e| PaneError::Spawn(e.to_string()))
    }
}

impl PaneLifecycle for WorkspacePaneAccess {
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, PaneError> {
        // ⚠ NO WORKING DIRECTORY AND NO ENVIRONMENT, which is what this door has always meant: a
        // plugin spawning a one-shot tool has no opinion about either, and takes the daemon's.
        // `respawn` is the caller that does have one, and it reads both off the pane rather than being
        // told.
        self.spawn_in(argv, None, &[], cols, rows)
    }

    fn respawn(&self, id: PaneId) -> Result<PaneId, PaneError> {
        // ⚠⚠ READ, THEN RELEASE, THEN SPAWN. `spawn_in` takes the workspace lock itself, and this
        // crate's standing rule is that no lock is held across a syscall — a pty spawn most of all.
        let (argv, env, cwd, (cols, rows)) = {
            let guard = lock(&self.workspace);
            let pane = guard
                .pane(id)
                .ok_or_else(|| PaneError::Spawn(format!("no pane {} to replace", id.0)))?;
            (
                pane.argv().to_vec(),
                pane.env().to_vec(),
                pane.pty().cwd(),
                pane.pty().dimensions(),
            )
        };
        if argv.is_empty() {
            // ⚠ A REAL CASE AND NOT A DEFENSIVE ONE: `Pane::argv` is empty for a pane restored from
            // a snapshot taken before argv capture existed. Saying so beats spawning something
            // invented, which is what a fallback to a shell would be for a caller that is trying to
            // replace an AGENT session.
            return Err(PaneError::Spawn(format!(
                "pane {} has no recorded command to re-run, so it cannot be replaced",
                id.0
            )));
        }
        let fresh = self.spawn_in(&argv, cwd.as_deref(), &env, cols, rows)?;
        // ⚠ The old pane goes only once the new one is up — see the trait's doc. Its answer is
        // deliberately ignored: it existed a moment ago (it was read above), and a caller holding a
        // fresh pane has nothing to do differently if the reap raced somebody else's close.
        let _ = <Self as PaneLifecycle>::close(self, id);
        Ok(fresh)
    }

    fn close(&self, id: PaneId) -> bool {
        // Bind the removed Pane so the workspace guard (the temporary) drops
        // first; the Pane's blocking Drop (kill/wait/join) then runs OUTSIDE
        // the workspace lock (R11 lesson).
        let removed = lock(&self.workspace).close(id);
        removed.is_some()
    }
}

/// Lock the workspace, recovering the guard if a holder panicked.
fn lock(workspace: &Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    workspace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Per-row `(generation, text)` for the whole screen. Text via the canonical
/// [`Screen::row_text`] so capture and the emulator's scrollback never drift.
fn read_rows(screen: &Screen) -> Vec<PaneRow> {
    (0..screen.rows())
        .map(|row| PaneRow {
            generation: screen.row_generation(row).unwrap_or(0),
            text: screen.row_text(row),
        })
        .collect()
}

/// Collapsed screen text: each row's SHARE OF ITS LOGICAL LINE, joined without separators, so a
/// sentinel the terminal wrapped across rows still matches.
///
/// # ⚠⚠⚠ Why the share and not the row's text
///
/// This joined `Screen::row_text`, and that reader's own doc says it cannot be joined that way:
/// *"it trims a continuing row's trailing blanks, which are interior to the line, and it keeps the
/// pad a wide cluster left at the margin, which is not in the line at all. Both halves have cost
/// this project a defect."* The warning was written and the wrong reader stayed here — under
/// [`ReadyWhen::Prints`], whose own doc promises the join is *"wrap-safe … at any width."*
///
/// Measured: a pane five columns wide printing `TOOL UP` wraps after the SPACE, so the rows are
/// `"TOOL "` and `"UP"`; trimmed and joined they read **`"TOOLUP"`**, and a barrier waiting for
/// `TOOL UP` never clears. The width is not the caller's to choose — **a client attaching at
/// another size decides it** — so the same run, the same program and the same marker succeed or
/// hang depending on somebody else's window.
///
/// [`Screen::row_share_text`] is the reader that exists for exactly this, and it is what
/// [`Screen::lines_since`] already joins. One rule, one place.
fn read_collapsed(screen: &Screen) -> String {
    (0..screen.rows())
        .map(|row| screen.row_share_text(row))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::CommandBuilder;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn cat_workspace(cols: u16, rows: u16) -> Arc<Mutex<Workspace>> {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        lock(&workspace)
            .spawn(command, "cat".to_string(), cols, rows)
            .expect("spawn pane");
        workspace
    }

    /// Poll `ready` until it holds or `within` elapses, answering whether it ever did.
    fn until(within: Duration, mut ready: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < within {
            if ready() {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    /// ⚠⚠⚠ **THE STOP REACHES THROUGH THE EXTENSION API, AND WHAT AN INJECTION CANNOT REACH.**
    ///
    /// The pane runs `stty -isig; exec sleep 300`, so its terminal has been told to make no signals
    /// out of input — the condition a plugin can neither see nor prevent, and the one that makes a
    /// written `0x03` a stop in name only.
    ///
    /// The CONTROL is the extension API's own [`PaneAccess::inject`] carrying `Ctrl-C`: the pane
    /// echoes `^C`, so the byte was processed, and the job runs on. The SUBJECT is
    /// [`PaneJobControl::pane_stop_job`] on the same pane in the same test, which ends it — and
    /// **names it**, which the injection could not have done even had it worked, because a write
    /// reports bytes and not consequences.
    ///
    /// ⚠ Driven through `&dyn PaneAccess` rather than the concrete type: what a plugin holds is the
    /// trait object, so a capability reachable only from `WorkspacePaneAccess` would be one no
    /// plugin can use — the exact shape [`PaneAccess::job_control`] exists to make impossible.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_plugin_can_stop_a_job_its_own_ctrl_c_would_only_have_been_echoed_at() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty -isig; exec sleep 300");
        command.env("TERM", "dumb");
        lock(&workspace)
            .spawn(command, "sleep".to_string(), 40, 6)
            .expect("spawn pane");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let panes: &dyn PaneAccess = &access;
        let pane = panes.pane_ids()[0];
        let jobs = panes
            .foreground_job()
            .expect("this host reads the job table");

        assert!(
            until(Duration::from_secs(10), || jobs
                .pane_foreground_leader(pane)
                .is_some_and(|job| job.name == "sleep")),
            "the fixture never reached its job, so nothing below measures anything",
        );

        // ⚠ The W3C key is `"c"` with CONTROL held, which is what every surface's `C-c` becomes —
        // the encoder turns that pair into the byte 0x03. Spelling it `"Ctrl+c"` is not a key name
        // and is refused at the encoder, which is the API working as designed.
        let written = panes
            .inject(
                pane,
                &[KeyStroke {
                    key: "c".to_string(),
                    mods: Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    },
                }],
            )
            .expect("the write itself succeeds — that is the point");
        assert_eq!(
            written,
            Written::of(1),
            "and it wrote the one byte a Ctrl-C is, so what follows is about that byte's fate \
             rather than about an encoder that sent nothing",
        );
        assert!(
            until(Duration::from_secs(10), || panes
                .pane_collapsed(pane)
                .is_some_and(|screen| screen.contains("^C"))),
            "⚠ THE CONTROL'S PREMISE: the terminal must ECHO the byte, or the job surviving says \
             only that the byte had not arrived",
        );
        assert!(
            jobs.pane_foreground_leader(pane)
                .is_some_and(|job| job.name == "sleep"),
            "⚠⚠ THE CONTROL: a plugin's own Ctrl-C was written, echoed, and stopped nothing",
        );

        let control = panes
            .job_control()
            .expect("this host can signal a pane's job");
        let signalled = control
            .pane_stop_job(pane, Stop::Interrupt, Reach::TheProgramToo)
            .expect("the group is signalled");
        assert!(
            signalled
                .leader
                .as_ref()
                .is_some_and(|leader| leader.answers_to("sleep")),
            "⚠ the report names the job the way a readiness refusal would — through the SAME \
             two-name reading, or a stop and a barrier would spell one program two ways: \
             {signalled}",
        );
        assert!(
            until(Duration::from_secs(10), || jobs
                .pane_foreground_leader(pane)
                .is_none()),
            "⚠⚠ THE SUBJECT: the signal ended the job the injection could not",
        );
    }

    /// A pane that is REAL and whose program has GONE is refused for that reason, and a pane that
    /// does not exist is refused for the other.
    ///
    /// ⚠ The two are different corrections — *your program finished* sends somebody to their
    /// scrollback, *there is no such pane* sends them to their pane list — and collapsing them was
    /// the temptation here, because both arrive at the same `?`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_finished_program_and_a_pane_that_never_existed_are_refused_differently() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("exit 0");
        command.env("TERM", "dumb");
        lock(&workspace)
            .spawn(command, "gone".to_string(), 40, 6)
            .expect("spawn pane");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let panes: &dyn PaneAccess = &access;
        let pane = panes.pane_ids()[0];
        let control = panes
            .job_control()
            .expect("this host can signal a pane's job");

        assert!(
            until(Duration::from_secs(10), || matches!(
                control.pane_stop_job(pane, Stop::Interrupt, Reach::TheProgramToo),
                Err(PaneError::NotStopped(Unstopped::Gone)),
            )),
            "a pane whose child was reaped has nothing to stop, and says so about the PROGRAM",
        );
        assert_eq!(
            control.pane_stop_job(PaneId(4242), Stop::Interrupt, Reach::TheProgramToo),
            Err(PaneError::UnknownPane(PaneId(4242))),
            "and a pane nobody knows is a different correction entirely",
        );
    }

    /// ⚠⚠ **THE DEGRADATION ARM OF [`PaneAccess::pane_full_lines`], which no production host takes.**
    ///
    /// The trait's default splits the RENDERED text back into lines, so a host that has not
    /// implemented the reader answers the old way rather than answering nothing — and a caller
    /// asking for content on such a host gets the width baked in, which is the degradation and not
    /// an equivalent.
    ///
    /// Gated here because [`WorkspacePaneAccess`] overrides it, so the default is product code
    /// **nothing else in this crate builds** — the shape this workspace has now paid for four times
    /// (`RowTrail`'s no-stream fallback, two plugin degradation arms, and the echo strip on the
    /// same path).
    #[test]
    fn a_host_that_implements_only_the_rendered_text_still_answers_for_lines() {
        struct RenderedOnly;
        impl PaneAccess for RenderedOnly {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(true)
            }
            // ⚠ HONOURS THE ID, because the absence half below is exactly what the default's `?`
            // is for — a fake that answers for every id cannot measure it.
            fn pane_full_text(&self, id: PaneId) -> Option<String> {
                (id == PaneId(1)).then(|| "first\nsecond".to_string())
            }
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
                Ok(Written::of(0))
            }
        }

        assert_eq!(
            RenderedOnly.pane_full_lines(PaneId(1)),
            Some(vec!["first".to_string(), "second".to_string()]),
            "the default answers from what the host does implement, rather than refusing",
        );
        assert_eq!(
            RenderedOnly.pane_full_lines(PaneId(9)),
            None,
            "and an unknown pane is still an absence, not an empty list — a caller cannot tell \
             `there is no such pane` from `it said nothing` if those collapse",
        );
    }

    /// ⚠⚠ **A LEADER READS AS ONE NAME WHEN ITS TWO AGREE, AND AS BOTH WHEN THEY DO NOT.**
    ///
    /// The half of the fix that is about not breaking anything: a leader whose kernel name and
    /// `argv[0]` say the same thing — which is nearly every process — must read exactly as it did
    /// when [`PaneDoing::Job`] carried a bare `String`, or every failure sentence an agent has ever
    /// been shown changes shape for a fact that did not change.
    ///
    /// The other half is the correction itself: when they DISAGREE the reader is handed both, in
    /// the order that puts the caller's own word first. Reading `"cat"` alone was what let a macOS
    /// caller be told their `/bin/sh` pane belonged to `"bash"`.
    ///
    /// ⚠ Driven through [`PaneDoing`]'s own sentence rather than [`JobLeader`]'s, because the
    /// sentence is the published surface and a `Display` that is right in isolation proves nothing
    /// about the text that reaches an agent.
    #[test]
    fn a_job_reads_as_one_name_when_its_two_agree_and_as_both_when_they_do_not() {
        let leader = |name: &str, argv: &[&str]| {
            JobLeader::of(&JobProcess {
                pid: 11,
                name: name.to_string(),
                argv: argv.iter().map(|arg| (*arg).to_string()).collect(),
            })
        };

        assert_eq!(
            PaneDoing::Job(leader("cat", &["cat"])).to_string(),
            "; its terminal belonged to \"cat\" instead",
            "the ordinary case is UNCHANGED — one name, spelled once",
        );
        assert_eq!(
            PaneDoing::Job(leader("cat", &[])).to_string(),
            "; its terminal belonged to \"cat\" instead",
            "and a job with no argv at all has one name to give, not an empty second one",
        );
        assert_eq!(
            PaneDoing::Job(leader("bash", &["/bin/sh", "-c", "cat | tr a-z A-Z"])).to_string(),
            "; its terminal belonged to \"sh\" (which the kernel calls \"bash\") instead",
            "⚠⚠ THE macOS CASE, VERBATIM: the caller launched `/bin/sh` and must be told `sh` \
             first — and `bash` too, because that is the other spelling `Runs` accepts",
        );
    }

    /// A pane a PLUGIN opens lands in the cgroup its pool's window names.
    ///
    /// The fifth door onto pane birth, and the one whose comment used to claim "the host fills this
    /// in when it has a tree" while the host did no such thing for this path (R337). It is gated
    /// here rather than trusted to the structure, because that comment is exactly what trusting the
    /// structure looks like from the inside.
    ///
    /// A stand-in cgroup root of ordinary files: this asserts the pool PLACED the pane, which is
    /// this layer's whole responsibility. That the kernel then honours the weight is
    /// `sprag-terminal/tests/pane_share_cgroup.rs`, against a real delegated scope.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_a_plugin_opens_lands_in_the_cgroup_its_window_names() {
        use sprag_terminal::share::{PaneHomes, PoolLineage, Tree};
        use sprag_terminal::{SessionId, WindowId};

        let root = std::env::temp_dir().join(format!("sprag-plugin-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            std::fs::write(path.join("cgroup.procs"), "").expect("fixture procs");
            std::fs::write(path.join("cgroup.subtree_control"), "").expect("fixture subtree");
            // What the parent enabled here — read by every level's `enable_controllers`, and
            // present on every real cgroup.
            std::fs::write(path.join("cgroup.controllers"), "cpu memory pids\n")
                .expect("fixture controllers");
            std::fs::write(path.join("cpu.weight"), "100\n").expect("fixture weight");
        };
        cgroup("");
        cgroup("session-3");
        cgroup("session-3/window-4");

        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        {
            let mut pool = lock(&workspace);
            pool.set_home(PoolLineage {
                session: SessionId(3),
                window: WindowId(4),
            });
            pool.set_pane_homes(Arc::new(PaneHomes::over(
                Tree::adopt(root.clone()).expect("adopt the stand-in root"),
            )));
        }

        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let pane = access
            .spawn(
                &["/bin/sh".to_owned(), "-c".to_owned(), "cat".to_owned()],
                20,
                4,
            )
            .expect("a plugin opens a pane");

        assert!(
            root.join(format!("session-3/window-4/pane-{}", pane.0))
                .is_dir(),
            "a plugin's pane was born outside the share tree its window owns",
        );

        let _ = access.close(pane);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn injects_and_reads_back_through_the_api() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let pane = access.pane_ids()[0];

        let mut keys = KeyStroke::text("hi");
        keys.push(KeyStroke::named("Enter"));
        let written = access.inject(pane, &keys).expect("inject");
        assert!(written.bytes() >= 3, "wrote {} bytes", written.bytes());

        // The echo is async; poll the collapsed text until it lands.
        let start = Instant::now();
        let mut echoed = false;
        while !echoed && start.elapsed() < Duration::from_secs(5) {
            echoed = access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains("hi"));
            if !echoed {
                sleep(Duration::from_millis(20));
            }
        }
        assert!(echoed, "injected 'hi' never echoed back");

        // pane_rows snapshots generation+text together.
        let rows = access.pane_rows(pane).expect("rows");
        assert_eq!(rows.len(), 4);
        assert!(
            rows.iter()
                .any(|r| r.text.contains("hi") && r.generation > 0)
        );
    }

    #[test]
    fn inject_into_unknown_pane_is_typed() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let err = access
            .inject(PaneId(999), &KeyStroke::text("x"))
            .unwrap_err();
        assert_eq!(err, PaneError::UnknownPane(PaneId(999)));
    }

    /// ⚠⚠ **A PANE REMEMBERS WHAT WAS WRITTEN INTO IT**, which is the only thing that lets a
    /// reader tell the pane's own echo from what the program in it said.
    #[test]
    fn a_pane_remembers_what_was_written_into_it() {
        let workspace = cat_workspace(40, 8);
        let pane = lock(&workspace).panes()[0].id();
        let access = WorkspacePaneAccess::new(workspace);
        assert_eq!(
            access
                .input_echo()
                .expect("this host records a trail")
                .pane_recent_input(pane)
                .as_deref(),
            Some(""),
            "a pane nobody has written to has an empty trail, not a missing one",
        );
        let mut typed = KeyStroke::text("HELLO-TRAIL");
        typed.push(KeyStroke::named("Enter"));
        let _written = access.inject(pane, &typed).expect("write");
        let trail = access
            .input_echo()
            .expect("this host records a trail")
            .pane_recent_input(pane)
            .expect("a live pane has a trail");
        assert!(
            trail.contains("HELLO-TRAIL"),
            "what was typed has to be in the trail, or nothing downstream can recognise its \
             echo: {trail:?}",
        );
        assert!(
            access
                .input_echo()
                .expect("recorded")
                .pane_recent_input(PaneId(9999))
                .is_none(),
            "and a pane nobody knows has no trail at all",
        );
    }

    #[test]
    fn lifecycle_spawn_and_close_roundtrip() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let life = access
            .lifecycle()
            .expect("workspace access exposes lifecycle");

        let id = life
            .spawn(
                &["/bin/sh".to_string(), "-c".to_string(), "cat".to_string()],
                20,
                4,
            )
            .expect("spawn");
        assert!(lock(&workspace).pane(id).is_some(), "pane should be live");

        assert!(life.close(id), "close reports the pane existed");
        assert!(lock(&workspace).pane(id).is_none(), "pane should be gone");
        assert!(!life.close(id), "closing again reports absence");
    }

    /// ⚠⚠⚠ **A REPLACEMENT PANE IS THE SAME COMMAND IN THE SAME WORLD** —
    /// [`PaneLifecycle::respawn`], and the four facts a session replacement has to carry.
    ///
    /// # ⚠⚠⚠ Why the ENVIRONMENT is asserted through the child rather than by reading the pane back
    ///
    /// Comparing `Pane::env` before and after would compare two copies of the same `Vec` and pass on a
    /// `respawn` that recorded the entries and never PASSED them to the spawn. So the peer prints what
    /// it was given, and the assertion is what a program inside the fresh pane can see.
    ///
    /// It matters because of what it is for: `sprag_host::live_agent` blanks nine `CLAUDE_CODE_*`
    /// variables so its child is what a person gets from a terminal rather than a NESTED agent
    /// session, and a restart that dropped them would hand the replacement a mode only that harness
    /// can produce — silently, with every later reading being of a different program.
    ///
    /// ⚠ The cwd is asserted the same way and for the same reason, and it is the one this test can
    /// choose freely: `/` is never where a test runner stands, so *carried* and *defaulted* are
    /// different answers. See the same rule in `sprag_plugin::testing::standin_agent_refusing`, where
    /// a mutation went green until the fixture stopped agreeing with the default by accident.
    #[test]
    fn a_respawned_pane_is_the_same_command_in_the_same_world() {
        let workspace = Arc::new(Mutex::new(Workspace::new((37, 9))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // Everything the replacement has to reproduce, printed by the child itself.
            command.arg("printf 'MARK %s %s\\n' \"$SPRAG_RESPAWN_PROBE\" \"$(pwd)\"; exec cat");
            command.env("SPRAG_RESPAWN_PROBE", "carried");
            command.env("TERM", "dumb");
            command.cwd("/");
            lock(&workspace)
                .spawn(command, "sh".to_string(), 37, 9)
                .expect("spawn the pane to be replaced")
        };
        let settled = |id: PaneId| {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5)
                && !WorkspacePaneAccess::new(Arc::clone(&workspace))
                    .pane_collapsed(id)
                    .is_some_and(|text| text.contains("MARK "))
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            WorkspacePaneAccess::new(Arc::clone(&workspace))
                .pane_collapsed(id)
                .unwrap_or_default()
        };
        assert!(
            settled(pane).contains("MARK carried /"),
            "⚠ the control: the ORIGINAL pane must show what it was launched with, or the comparison \
             below is against a fixture that never worked: {:?}",
            settled(pane),
        );

        let life = access
            .lifecycle()
            .expect("workspace access exposes lifecycle");
        let fresh = life.respawn(pane).expect("a live pane can be replaced");
        let shown = settled(fresh);
        let (argv, size) = {
            let guard = lock(&workspace);
            let new = guard
                .pane(fresh)
                .expect("the replacement is in the workspace");
            (new.argv().to_vec(), new.pty().dimensions())
        };

        assert_ne!(fresh, pane, "a replacement is a NEW pane");
        assert!(
            lock(&workspace).pane(pane).is_none(),
            "⚠⚠⚠ and the pane it replaced is CLOSED — a replacement that left the old child running \
             leaves two programs where the caller asked for one",
        );
        assert!(
            shown.contains("MARK carried /"),
            "⚠⚠⚠ THE CHILD ITSELF MUST SEE the launcher's variable and the launcher's directory. \
             `MARK  /home/…` means the environment was dropped; `MARK carried /home/…` means the cwd \
             was. Shown: {shown:?}",
        );
        assert_eq!(
            argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf 'MARK %s %s\\n' \"$SPRAG_RESPAWN_PROBE\" \"$(pwd)\"; exec cat".to_string(),
            ],
            "and the same argv, which is the only one of the four the pane can be asked for directly",
        );
        assert_eq!(size, (37, 9), "and the same size");
        assert!(
            life.close(fresh),
            "the pane this gate opened was there to close"
        );
    }

    /// A pane that has gone cannot be replaced, and the refusal names it — the arm a loop meets when
    /// somebody closed its inner session by hand between two pumps.
    #[test]
    fn a_pane_that_is_gone_cannot_be_replaced() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let life = access.lifecycle().expect("lifecycle");
        match life.respawn(PaneId(4242)) {
            Err(PaneError::Spawn(why)) => assert!(
                why.contains("4242") && why.contains("replace"),
                "the refusal must name the pane and what was being attempted: {why:?}",
            ),
            other => panic!("a pane nobody has cannot be replaced: {other:?}"),
        }
    }

    #[test]
    fn lifecycle_spawn_rejects_empty_argv() {
        let access = WorkspacePaneAccess::new(Arc::new(Mutex::new(Workspace::new((20, 4)))));
        let life = access.lifecycle().unwrap();
        assert!(matches!(life.spawn(&[], 20, 4), Err(PaneError::Spawn(_))));
    }

    #[test]
    fn pane_full_text_includes_scrolled_off_lines() {
        // 30 numbered lines on a 4-row pane: the early ones scroll off. Full
        // text must include a line the visible-only read has lost.
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("seq 1 30");
        command.env("TERM", "dumb");
        let id = lock(&workspace)
            .spawn(command, "seq".to_string(), 20, 4)
            .expect("spawn");
        let access = WorkspacePaneAccess::new(workspace);

        // Wait until the child has finished (all output applied at EOF).
        let start = Instant::now();
        while access.pane_eof(id) != Some(true) && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let full = access.pane_full_text(id).expect("full text");
        // "\n5\n": line 5 as a standalone line — deep in the scrolled-off region.
        assert!(
            full.contains("\n5\n"),
            "scrolled-off line 5 missing: {full:?}"
        );
        assert!(
            full.contains("30"),
            "last line missing from full text: {full:?}"
        );
        // The visible-only read lost it — proving scrollback was needed (the
        // last visible rows are ~27..30, none containing '5').
        let visible = access.pane_collapsed(id).expect("visible");
        assert!(
            !visible.contains('5'),
            "line 5 should have scrolled off: {visible:?}"
        );
    }

    #[test]
    fn pane_raw_output_is_byte_exact_for_a_wrapping_line() {
        // A single logical line wider than the pane: the grid wraps and trims
        // it, but the raw source read returns the emitted bytes verbatim — the
        // capture path structured output (a wrapped JSON envelope) relies on.
        let payload = "abc def  ghi   ".repeat(12); // 180 chars, embedded runs of spaces
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%s' '{payload}'"));
        command.env("TERM", "dumb");
        let id = lock(&workspace)
            .spawn(command, "printf".to_string(), 20, 4)
            .expect("spawn");
        let access = WorkspacePaneAccess::new(workspace);

        let start = Instant::now();
        while access.pane_eof(id) != Some(true) && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let raw = access
            .raw_capture()
            .expect("workspace access exposes raw capture");
        let RawOutput { bytes, truncated } = raw.pane_raw_output(id).expect("raw output");
        assert!(!truncated);
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            payload,
            "raw bytes must be verbatim"
        );
        // The grid lost interior spaces to trailing-trim at wrap boundaries, so
        // it cannot reconstruct the source — exactly why raw capture exists.
        assert!(
            raw.pane_raw_output(PaneId(999)).is_none(),
            "unknown pane is None"
        );
    }

    /// **A pane a PLUGIN spawned can ask for a person** — the wiring
    /// [`WorkspacePaneAccess::with_attention`] exists for, driven end to end rather than merely
    /// present.
    ///
    /// It was present and undriven: every live caller of the attention path went through the mux
    /// surface, so a minter that was never called — or one called once and shared, which is the
    /// defect the type exists to prevent — would have left every test green while a dialogue
    /// plugin's own pane told nobody its build had finished.
    ///
    /// Three claims, and the third is the one a shared closure would fail:
    ///
    /// * the hook fires at all, carrying the CHILD's own words;
    /// * it names the pane THIS surface spawned, so the router can find who holds it;
    /// * each pane gets its OWN hook — two births, two mints — which is what keeps the sender the
    ///   hook owns per-pane and the PTY reader thread free of a lock.
    #[test]
    fn a_pane_a_plugin_spawned_can_ask_for_a_person() {
        let raised: Arc<Mutex<Vec<(PaneId, Attention)>>> = Arc::new(Mutex::new(Vec::new()));
        let mints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let access = WorkspacePaneAccess::new(Arc::new(Mutex::new(Workspace::new((40, 6)))))
            .with_attention(Some({
                let (raised, mints) = (Arc::clone(&raised), Arc::clone(&mints));
                Arc::new(move || {
                    mints.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let raised = Arc::clone(&raised);
                    Box::new(move |pane, attention| {
                        raised
                            .lock()
                            .expect("the raised log")
                            .push((pane, attention));
                    }) as Box<dyn Fn(PaneId, Attention) + Send>
                }) as AttentionMinter
            }));

        // The CHILD raises it, exactly as a build script inside a plugin's pane would.
        let pane = access
            .spawn(
                &[
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf '\\033]9;the plugin pane needs you\\007'; exec cat".to_owned(),
                ],
                40,
                6,
            )
            .expect("a plugin spawns a pane");
        // A second pane, so the mint count below is a claim about PER-BIRTH minting rather than
        // about the one call every wiring would make.
        let other = access
            .spawn(
                &["/bin/sh".to_owned(), "-c".to_owned(), "exec cat".to_owned()],
                40,
                6,
            )
            .expect("a plugin spawns a second pane");
        assert_ne!(pane, other);

        let start = Instant::now();
        while raised.lock().expect("the raised log").is_empty()
            && start.elapsed() < Duration::from_secs(10)
        {
            sleep(Duration::from_millis(20));
        }
        let seen = raised.lock().expect("the raised log").clone();
        let (told, attention) = seen.first().unwrap_or_else(|| {
            panic!("the plugin pane's child asked for a person and the hook never fired: {seen:?}")
        });
        assert_eq!(*told, pane, "the hook must name the pane that raised it");
        match attention {
            Attention::Raised(notification) => assert_eq!(
                notification.body, "the plugin pane needs you",
                "the child's own words must arrive",
            ),
            other => panic!("an OSC 9 is a raised notification, not {other:?}"),
        }
        assert_eq!(
            mints.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "a hook is minted PER BIRTH — one shared closure would be minted once",
        );
    }
}
