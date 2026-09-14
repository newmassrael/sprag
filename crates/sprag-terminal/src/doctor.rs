//! What is WRONG with the machine the panes run on — layer 2 of the resource design.
//!
//! [`crate::resources`] is layer 1: it says what each pane is TAKING, every ten seconds, for a few
//! file reads. It answers *which pane is eating the machine* and it cannot answer *why is this
//! machine slow*, because most of the reasons are not the multiplexer's. The design this implements
//! records the measurement that settles that: of seven causes found in a real investigation, **one**
//! belonged to the terminal and the rest were a compiler cache bypassed by a `PATH`, kernel swap
//! tuning, a systemd delegation policy and a CI runner competing at equal weight. A diagnosis
//! scoped to sprag's own state would have found one seventh of the problem.
//!
//! # The three rules this module is built on, each of which cost that investigation something
//!
//! * **Print the measured value beside the verdict.** [`Evidence`] cannot be empty — not by
//!   convention, by construction — so a [`Finding`] that says *degraded* and cannot say what it
//!   read does not exist. Advice a person cannot check is advice they have to take on faith.
//! * **Detect; never prescribe by acting.** Nothing here writes a file, and the one check that runs
//!   a program runs it through [`Probe`], whose whole set is a private const table of read-only
//!   invocations. A remedy is a sentence, and the person types it.
//! * **A setting is not a state.** Two worked examples in the design are both settings that read
//!   correct and did nothing: a `CPUWeight=10` that changed no allocation because cgroup weights
//!   compare only among SIBLINGS and the siblings were idle, and a wrapper that was syntactically
//!   fine and never executed. So [`Check::CompetingWeight`] walks the LEVELS between this daemon's
//!   subtree and the top of the hierarchy and reads what each level's children actually took over a
//!   window; and [`Check::CcacheOnPath`] reads the `PATH` a pane's child was really started with
//!   rather than any file that claims to set one.
//!
//! # Why the judging is pure and the reading is not
//!
//! [`Readings`] is a value: every file this module opens and every command it runs lands in one,
//! and [`Check::judge`] is a function from that value to a [`Finding`] with no clock, no filesystem
//! and no host in it. `sprag-detect` made the same split for the same reason — a verdict about
//! somebody else's machine is only honest if a captured machine can be replayed against it, and no
//! test suite can arrange a box that is swapping, oversubscribed and missing a controller at once.
//!
//! # What this cannot see
//!
//! A `PATH` read from `/proc/<pid>/environ` is the one the process was EXECUTED with. A shell that
//! edits `PATH` in its own rc file has a different one and the kernel does not publish it. That
//! bound is stated in the criterion the check prints, because a reader who does not know it would
//! read a clean verdict as a promise it is not.
//!
//! [`Check::StrayDaemon`] recognises a daemon by the FILE NAME of its program, so a fork of this
//! daemon renamed on disk is not one of *these* daemons however alike it behaves. That is
//! deliberate and is the only spelling that catches the case the check exists for — a second daemon
//! running a DIFFERENT BUILD of the same program, whose full path is by definition not ours.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::closed_set;
use crate::resources::Cpu;
use crate::share::{CgroupNode, Landing, Percent, Pressure, Waiting};
use crate::workspace::PaneId;

closed_set! {
    /// One thing that can be wrong with the environment a pane runs in.
    ///
    /// A closed set, and every one of them is answered on every diagnosis — a check whose source is
    /// missing reports [`Verdict::Blind`] rather than dropping out of the list. A person reading a
    /// report has to be able to tell *this was fine* from *nobody looked*, and a check that
    /// disappears when it cannot run is indistinguishable from one that passed.
    ///
    /// The set comes from an investigation rather than from a taxonomy: each arm is something that
    /// was actually measured as a cause, and the two that the design lists as separate rows here
    /// arrive as one arm ([`Swapping`](Self::Swapping)) because a swap SETTING without the swap it
    /// caused is not a verdict — the design's own worked output prints them in one sentence.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum Check {
        /// Do two panes share one cgroup, so that the kernel cannot tell their CPU apart?
        PaneIsolation,
        /// Did the panes this daemon placed actually get INTO the cgroups it opened for them?
        PaneAdmission,
        /// Which controllers reached this daemon's subtree, and which arbitration is therefore
        /// impossible here whatever anybody sets?
        ControllerDelegation,
        /// Above this daemon's subtree, is something taking CPU at a weight equal to or better than
        /// the whole terminal's?
        CompetingWeight,
        /// Is the machine as a whole waiting for CPU?
        CpuStall,
        /// Is the machine as a whole stopped on disk?
        IoStall,
        /// Is the machine as a whole stopped on memory?
        MemoryStall,
        /// Are the panes' pages on disk, and how eagerly will the kernel put them there?
        Swapping,
        /// Are there far more runnable tasks than cores to run them?
        BuildSaturation,
        /// Is the compiler cache installed and bypassed?
        CcacheOnPath,
        /// Is the compiler cache big enough for what is being built through it?
        CcacheSizing,
        /// Is there a fast linker for the panes' builds to use?
        FastLinker,
        /// Is another daemon of this program still running on this machine with nobody attached?
        ///
        /// The one arm here that is not about a resource setting, and it is on this list rather
        /// than on a per-pane surface for the reason the whole module exists: a daemon nobody is
        /// attached to is in NO session, so no tool divided by session can reach it. The
        /// investigation behind it found an agent's probe daemon that had outlived the turn that
        /// spawned it, been reparented to `systemd --user`, and gone on holding its socket with no
        /// client on it — invisible to every other reading here.
        StrayDaemon,
    }
}

/// Everything the vocabulary knows about one [`Check`].
///
/// ONE exhaustive match ([`Check::entry`]) rather than a method per property, for
/// [`crate::share::Share`]'s neighbour's reason and for `sprag_host`'s: four matches are four
/// chances to forget an arm, and one match makes the compiler ask for every property of a new check
/// at the moment it is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The check's name, as a report spells it and as it goes over the wire.
    pub name: &'static str,
    /// The question, in the words of the person who would ask it.
    pub asks: &'static str,
    /// The file or the command this check READ. Named so a person can go and read it themselves —
    /// which is the difference between a diagnosis and an opinion.
    pub source: &'static str,
    /// What makes this check say [`Verdict::Degraded`], including any bound on what it can see.
    pub criterion: &'static str,
    /// What a person could DO about it. A sentence for them to act on, never a command this
    /// process runs: see the module docs.
    pub remedy: &'static str,
}

impl Check {
    /// Everything known about this check.
    #[must_use]
    pub const fn entry(self) -> Entry {
        match self {
            Self::PaneIsolation => Entry {
                name: "pane-isolation",
                asks: "does each pane have a cgroup of its own?",
                source: "/proc/<pid>/cgroup, per pane",
                criterion: "two panes reading the same cgroup path. Read from where each pane's \
                            child IS, not from where it was placed, so a pane that escaped its leaf \
                            is caught too",
                remedy: "a pane sharing a cgroup was not placed; the pane-admission row below \
                         says whether the kernel refused it, which is the usual reason and is not \
                         one any sprag setting changes",
            },
            Self::PaneAdmission => Entry {
                name: "pane-admission",
                asks: "did the panes actually get into the cgroups opened for them?",
                source: "what the kernel answered each pane's child at its birth, remembered per \
                         pane — the one reading here that /proc cannot supply afterwards",
                criterion: "any pane whose child the kernel refused to admit. Refused is not the \
                            same as unplaced and the difference is the whole row: unplaced means \
                            this daemon did not try, refused means it tried and was turned away, \
                            and only the second is a fault outside this daemon. A host with no \
                            delegated subtree is blind here rather than clean, because a pane \
                            nobody offered a cgroup was never refused one",
                remedy: "cgroup v2 checks delegation containment at the WRITE, against the common \
                         ancestor of where the process IS and where it is going — so a daemon \
                         running outside the subtree it was given cannot move anything into it, \
                         however that subtree is configured. Start the daemon inside its own \
                         scope, which is what a systemd unit does and what a bare process in a CI \
                         runner does not",
            },
            Self::ControllerDelegation => Entry {
                name: "controller-delegation",
                asks: "which resources can be arbitrated between panes at all?",
                source: "cgroup.controllers and cgroup.subtree_control of the delegated subtree",
                criterion: "cpu missing from what the subtree turned on — no pane can carry a \
                            weight, so every share setting is inert. io missing is reported and is \
                            not a fault here: this daemon delegates cpu, memory and pids, so an \
                            absent io controller costs nothing unless the machine is stalling on \
                            disk, which the io-stall row answers",
                remedy: "systemd delegates only what the parent slice enabled; widening it is a \
                         system-level `Delegate=` change, not this daemon's",
            },
            Self::CompetingWeight => Entry {
                name: "competing-weight",
                asks: "is something outside the terminal taking the machine from it?",
                source: "cpu.weight and cpu.stat of every sibling at each level between this \
                         daemon's subtree and the cgroup root, sampled twice",
                criterion: "a sibling that is not on this daemon's path took CPU over the window \
                            while carrying a weight at least equal to ours. A weight is compared \
                            only among siblings, so a level where nothing else ran is not \
                            competition however the weights read",
                remedy: "raise this daemon's slice against its siblings, or lower the batch \
                         workload's — the numbers beside each name say which level to set it at",
            },
            Self::CpuStall => Entry {
                name: "cpu-stall",
                asks: "is the machine waiting for CPU?",
                source: "/proc/pressure/cpu, and each pane's own cpu.pressure",
                criterion: "the machine's `some avg60` at or above the limit printed beside it. \
                            The five-minute figure is printed beside the minute — a minute far \
                            above it is a burst, a minute equal to it is how this machine lives — \
                            and so is the worst pane's own, because a machine stalling while ONE \
                            pane holds all of it is a different problem from one stalling evenly",
                remedy: "the build-saturation row says whether there is simply more work than \
                         cores; the competing-weight row says whether somebody else is taking them",
            },
            Self::IoStall => Entry {
                name: "io-stall",
                asks: "is the machine stopped on disk?",
                source: "/proc/pressure/io",
                criterion: "`full avg60` at or above the limit printed beside it — not a slow \
                            disk, but time when EVERY runnable task on the box was parked waiting \
                            for one. A limit and not simply above-zero: an idle machine measures a \
                            few hundredths of a percent here, and a row that is red on a healthy \
                            box is a row nobody reads on the day it matters",
                remedy: "io cannot be arbitrated between panes unless the io controller is \
                         delegated; the controller-delegation row says whether it is",
            },
            Self::MemoryStall => Entry {
                name: "memory-stall",
                asks: "is the machine stopped reclaiming memory?",
                source: "/proc/pressure/memory",
                criterion: "`full avg60` at or above the limit printed beside it — every runnable \
                            task parked while the kernel reclaims. The same limit the disk row \
                            uses, and for the same measured reason",
                remedy: "a per-pane memory ceiling bounds one pane's share of this; the swapping \
                         row says whether pages are already going to disk",
            },
            Self::Swapping => Entry {
                name: "swapping",
                asks: "are the panes' pages on disk?",
                source: "VmSwap in /proc/<pid>/status for every process in every pane's cgroup, and \
                         /proc/sys/vm/swappiness",
                criterion: "any pane holding pages in swap. The swappiness setting is printed \
                            beside it and is not the verdict: a setting with no swapped page behind \
                            it is a number, not a fault",
                remedy: "an agent that has been swapped out pays the fault on its next keystroke; \
                         lowering vm.swappiness changes how eagerly the kernel does it again",
            },
            Self::BuildSaturation => Entry {
                name: "build-saturation",
                asks: "is more work runnable than there are cores to run it?",
                source: "/proc/loadavg, /proc/stat, and the process count of every pane's cgroup",
                criterion: "runnable tasks at or above twice the core count. Twice, because at \
                            parity a scheduler is busy and at twice it is queueing: every task's \
                            wait is then longer than its run",
                remedy: "the panes' own process count is printed beside the machine's — a parallel \
                         build inside one pane is bounded by that pane's job flag, not by the \
                         terminal",
            },
            Self::CcacheOnPath => Entry {
                name: "ccache-on-path",
                asks: "is the compiler cache installed and being walked past?",
                source: "the ccache compiler shim directory, and the PATH each pane's child was \
                         executed with",
                criterion: "shims present and no pane started with the shim directory on its PATH. \
                            A shell that edits PATH in its own rc file has one the kernel does not \
                            publish, so a clean verdict here means the panes were STARTED with it, \
                            not that every command finds it",
                remedy: "put the shim directory ahead of the compilers on the PATH the panes are \
                         started with, so a build reaches the cache without opting in",
            },
            Self::CcacheSizing => Entry {
                name: "ccache-sizing",
                asks: "is the compiler cache big enough for what goes through it?",
                source: "ccache -s and ccache -p, run as the DAEMON would run them",
                criterion: "any cleanup at all. A cleanup is the cache evicting to stay under its \
                            ceiling, so a non-zero count means the working set does not fit and \
                            some of what was paid for has already been thrown away. The \
                            configuration read is the one this DAEMON'S environment selects, which \
                            is what its panes inherit — a shell that points itself at another \
                            config in its own rc file has one nothing outside it can read",
                remedy: "raise max_size past the working set; the hit rate printed beside it says \
                         what the cache is currently worth",
            },
            Self::FastLinker => Entry {
                name: "fast-linker",
                asks: "is there a fast linker for the panes' builds to use?",
                source: "the PATH each pane's child was executed with",
                criterion: "neither mold nor lld resolvable on any pane's PATH. The default linker \
                            is single-threaded, and a link is the one build step that cannot be \
                            parallelised away",
                remedy: "install mold or lld and select it in the build's link flags — finding it \
                         on PATH is not the same as a build choosing it",
            },
            Self::StrayDaemon => Entry {
                name: "stray-daemon",
                asks: "is another daemon of this program still running with nobody attached?",
                source: "/proc/<pid>/exe for every process on this machine, and the socket each \
                         listening one serves in /proc/net/unix",
                criterion: "a daemon of this program, OTHER than the one this report came from, \
                            with no client attached. Attachment is counted from the kernel's \
                            socket table rather than asked of the daemon, so one that has stopped \
                            answering is still counted; the reporting daemon is never its own \
                            fault, because the report is itself an attachment. A second daemon \
                            somebody IS attached to is the supported per-socket model and is \
                            listed, not flagged",
                remedy: "read the pid's own tree before ending it — a daemon with nobody attached \
                         is either an unattended run whose panes are still working or a leftover \
                         whose only child is its boot shell, and only the second is waste. The \
                         image beside each pid says which build it is running, which is the other \
                         thing a second daemon costs",
            },
        }
    }

    /// This check's answer for one captured machine.
    ///
    /// Pure: it opens nothing and it reads no clock, so every arm is driven from a [`Readings`]
    /// literal. See the module docs.
    #[must_use]
    pub fn judge(self, readings: &Readings) -> Finding {
        match self {
            Self::PaneIsolation => judge_pane_isolation(readings),
            Self::PaneAdmission => judge_pane_admission(readings),
            Self::ControllerDelegation => judge_controller_delegation(readings),
            Self::CompetingWeight => judge_competing_weight(readings),
            Self::CpuStall => judge_cpu_stall(readings),
            Self::IoStall => judge_io_stall(readings),
            Self::MemoryStall => judge_memory_stall(readings),
            Self::Swapping => judge_swapping(readings),
            Self::BuildSaturation => judge_build_saturation(readings),
            Self::CcacheOnPath => judge_ccache_on_path(readings),
            Self::CcacheSizing => judge_ccache_sizing(readings),
            Self::FastLinker => judge_fast_linker(readings),
            Self::StrayDaemon => judge_stray_daemon(readings),
        }
    }

    /// A [`Finding`] for this check, spelled at the call sites above.
    fn found(self, verdict: Verdict, evidence: Evidence) -> Finding {
        Finding {
            check: self,
            verdict,
            evidence,
        }
    }
}

/// One named quantity a check measured, in the words a person reads it in.
///
/// Text rather than a number because eleven checks measure eleven different shapes — a percentage,
/// a byte count, a controller list, a cgroup path — and a union of those shapes would be a type
/// every reader has to switch on to print. What every reader DOES do with it is print it beside its
/// name, so that is what it is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Measurement {
    /// What was measured.
    pub of: String,
    /// What it read.
    pub is: String,
}

impl Measurement {
    /// One reading.
    fn new(of: impl Into<String>, is: impl Into<String>) -> Self {
        Self {
            of: of.into(),
            is: is.into(),
        }
    }
}

/// Why a check answered what it answered — one measurement at least, always.
///
/// # Why the emptiness is a type and not a habit
///
/// The design's rule is that a verdict without its measured value cannot be checked by the person
/// receiving it, and that rule survives exactly as long as somebody remembers it. Held as a head
/// and a tail it is not a rule at all: a `Finding` with nothing behind it cannot be constructed,
/// and cannot arrive off the wire either — the deserialiser goes through the same fallible
/// conversion a caller does.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Vec<Measurement>", into = "Vec<Measurement>")]
pub struct Evidence {
    /// The reading the verdict turns on.
    head: Measurement,
    /// Everything else worth printing beside it.
    tail: Vec<Measurement>,
}

impl Evidence {
    /// The reading a verdict turns on.
    #[must_use]
    pub fn of(what: impl Into<String>, is: impl Into<String>) -> Self {
        Self {
            head: Measurement::new(what, is),
            tail: Vec::new(),
        }
    }

    /// One more reading, printed beside the first.
    #[must_use]
    pub fn and(mut self, what: impl Into<String>, is: impl Into<String>) -> Self {
        self.tail.push(Measurement::new(what, is));
        self
    }

    /// Every reading, the head first.
    pub fn rows(&self) -> impl Iterator<Item = &Measurement> {
        std::iter::once(&self.head).chain(&self.tail)
    }
}

/// What an [`Evidence`] that arrived with nothing in it is refused with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoEvidence;

impl std::fmt::Display for NoEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a verdict arrived with nothing measured behind it")
    }
}

impl std::error::Error for NoEvidence {}

impl TryFrom<Vec<Measurement>> for Evidence {
    type Error = NoEvidence;

    fn try_from(rows: Vec<Measurement>) -> Result<Self, Self::Error> {
        let mut rows = rows.into_iter();
        Ok(Self {
            head: rows.next().ok_or(NoEvidence)?,
            tail: rows.collect(),
        })
    }
}

impl From<Evidence> for Vec<Measurement> {
    fn from(evidence: Evidence) -> Self {
        std::iter::once(evidence.head)
            .chain(evidence.tail)
            .collect()
    }
}

/// What one check concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Measured, and inside what this check calls healthy.
    Healthy,
    /// Measured, and outside it. The evidence carries the number that says so.
    Degraded,
    /// Not measured, because the source is not on this host — which is a different fact from
    /// healthy, and the one a report that dropped the row would destroy.
    Blind(Blind),
}

/// Why a check could not look.
///
/// Each arm is a different thing to tell a person, which is the bar for being an arm: an absence
/// they would respond to identically belongs merged with its neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blind {
    /// This kernel keeps no pressure accounting — built without `CONFIG_PSI`, or booted `psi=0`.
    NoAccounting,
    /// There is no cgroup v2 hierarchy this daemon could read itself into, so nothing below it is
    /// measurable per pane.
    NoHierarchy,
    /// The hierarchy is there and this daemon was never given a subtree of its own, so there is no
    /// level at which it could be arbitrating anything.
    NoSubtree,
    /// The daemon holds no panes, so there is nothing to compare or to read a `PATH` from.
    NoPanes,
    /// The tool this check is about is not installed here, so there is no configuration to judge.
    NotInstalled,
    /// The tool IS here — its files are on this machine — and the program did not answer where this
    /// daemon runs.
    ///
    /// A separate arm from [`NotInstalled`](Self::NotInstalled) because the two are different
    /// people's problems: one is *install it*, the other is *put it on the PATH this daemon was
    /// started with*. Measured rather than imagined — a daemon launched from a stripped `PATH`
    /// reported `33 shims in /usr/lib/ccache` and `not installed on this host` in ONE report, which
    /// is a sentence the reader can see is false.
    Unanswered,
    /// The kernel's own tables — which processes exist, and which sockets they hold — were not
    /// there to read, so no process outside this one could be named at all.
    ///
    /// Its own arm rather than [`NoHierarchy`](Self::NoHierarchy)'s: a machine with no cgroup v2
    /// is an ordinary machine that simply cannot arbitrate, and a machine that publishes no
    /// process table is one where every reading here about OTHER processes is a guess. A reader
    /// responds differently — the first is a configuration, the second means look somewhere else
    /// entirely.
    NoProcessTable,
}

impl std::fmt::Display for Blind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAccounting => f.write_str("this kernel keeps no pressure accounting"),
            Self::NoHierarchy => f.write_str("no readable cgroup v2 hierarchy"),
            Self::NoSubtree => f.write_str("this daemon was given no cgroup subtree"),
            Self::NoPanes => f.write_str("no panes to read"),
            Self::NotInstalled => f.write_str("not installed on this host"),
            Self::Unanswered => {
                f.write_str("installed, but the program did not answer where this daemon runs")
            }
            Self::NoProcessTable => {
                f.write_str("this host publishes no process table for anything but ourselves")
            }
        }
    }
}

/// One check, its verdict, and what it read to get there.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    /// Which check.
    pub check: Check,
    /// What it concluded.
    pub verdict: Verdict,
    /// What it measured. Never empty — see [`Evidence`].
    pub evidence: Evidence,
}

/// Every check's answer for one machine, in [`Check::ALL`]'s order.
///
/// Total by construction: it is built by mapping the closed set, so a check added to the enum is in
/// every report the day it compiles and cannot be forgotten by a hand-written list — the ratchet
/// failure this project has hit three times.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnosis {
    /// One finding per check.
    pub findings: Vec<Finding>,
}

impl Diagnosis {
    /// Judge a captured machine.
    #[must_use]
    pub fn of(readings: &Readings) -> Self {
        Self {
            findings: Check::ALL
                .iter()
                .map(|check| check.judge(readings))
                .collect(),
        }
    }

    /// The findings that came back [`Verdict::Degraded`], in the same order.
    ///
    /// What a caller printing a summary counts, and what an agent asking *is anything wrong* reads.
    pub fn degraded(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.verdict == Verdict::Degraded)
    }
}

// ── what was read ───────────────────────────────────────────────────────────────────────────────

/// Everything the checks read, captured once.
///
/// A plain value with no absences hidden as zeroes: each field that can be missing says so in its
/// own type, because the difference between *the machine is not swapping* and *this host does not
/// publish swap* is the difference between a verdict and a guess.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Readings {
    /// `/proc/pressure/cpu`.
    pub cpu: Option<Pressure>,
    /// `/proc/pressure/io`.
    pub io: Option<Pressure>,
    /// `/proc/pressure/memory`.
    pub memory: Option<Pressure>,
    /// `/proc/sys/vm/swappiness` — how eagerly this kernel puts anonymous pages on disk.
    pub swappiness: Option<u32>,
    /// What the machine as a whole is being asked to run.
    pub load: Option<Load>,
    /// One row per live pane.
    pub panes: Vec<PaneReading>,
    /// This daemon's own delegated subtree, and the levels above it.
    pub subtree: Option<SubtreeReading>,
    /// The compiler cache, when it is installed.
    pub ccache: Option<Ccache>,
    /// The fast linkers found on the panes' own `PATH`s.
    pub linkers: Vec<String>,
    /// How many distinct `PATH`s the panes were started with — what the two `PATH` checks searched.
    pub paths: usize,
    /// Whether a cgroup v2 hierarchy was found at all. `false` makes the per-pane rows blind rather
    /// than clean.
    pub hierarchy: bool,
    /// Every daemon of this program the machine is running, and what is attached to each.
    ///
    /// [`None`] is the one absence this cannot recover from — either this process could not name
    /// its own program, or the kernel published no socket table — and it makes
    /// [`Check::StrayDaemon`] blind rather than clean.
    pub daemons: Option<Daemons>,
}

/// Every daemon of one program on this machine, as the kernel's tables show them.
///
/// # Why the reporting daemon is IN the list rather than filtered out of it
///
/// A report that quietly dropped the reader's own daemon would answer *how many daemons are there*
/// with a number that is one short of the truth, and the reader has no way to tell which. It is
/// here, marked, and the judgement excludes it — the exclusion is a property of the verdict, not
/// of the reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Daemons {
    /// The program every row here was recognised by — this process's own executable, by file name.
    pub program: String,
    /// This process. A row with this pid is the daemon the reader is talking to.
    pub mine: u32,
    /// One row per daemon found, in pid order.
    pub found: Vec<DaemonReading>,
}

/// One daemon of this program, and what the kernel says is attached to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonReading {
    /// Which process.
    pub pid: u32,
    /// The executable the kernel resolves for it, `(deleted)` and all.
    ///
    /// Kept whole rather than reduced to a file name, because the fault this check is looking for
    /// is a daemon running a different BUILD of the same program, and the build is the part of the
    /// path that a file name throws away. A `(deleted)` suffix is the kernel saying the image it is
    /// running has since been replaced on disk, which is the same fault one step further along.
    pub image: String,
    /// The socket it is listening on.
    pub socket: String,
    /// How many clients the kernel shows connected to that socket.
    pub attached: usize,
}

/// What the machine as a whole is being asked to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Load {
    /// Tasks in the runnable state right now — `/proc/loadavg`'s fourth field, before the slash.
    ///
    /// The instantaneous count and NOT a load average, deliberately: a one-minute average of a
    /// machine that has just been given a build says what the machine was doing before the build.
    pub runnable: u32,
    /// Every task on the box, runnable or not — the same field, after the slash.
    pub threads: u32,
    /// How many cores there are to run them on, counted from `/proc/stat`'s per-CPU rows.
    pub cores: u32,
    /// How many processes the panes hold between them, from their cgroups.
    pub pane_procs: u32,
}

/// One pane, as `/proc` and its cgroup describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneReading {
    /// Which pane.
    pub id: PaneId,
    /// The unified-hierarchy path this pane's child is actually in, read from `/proc/<pid>/cgroup`.
    ///
    /// Where the pane IS and not where the daemon meant to put it — the distinction R337 was built
    /// on. A pane that was never placed, or that escaped, reads its ancestor's path here and that
    /// is precisely what [`Check::PaneIsolation`] is looking for.
    pub cgroup: Option<String>,
    /// Bytes of this pane's processes that the kernel has put in swap, summed.
    pub swapped: Option<u64>,
    /// This pane's own CPU stall accounting.
    pub waiting: Waiting,
    /// Whether the ccache shim directory is on the `PATH` this pane's child was executed with.
    pub ccache_on_path: Option<bool>,
    /// What happened when this pane's child was asked to join its cgroup. See [`PaneSite::landing`]
    /// for why this is carried from the birth rather than read from the machine.
    pub landing: Landing,
}

/// This daemon's delegated subtree, and the competition above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtreeReading {
    /// Where the subtree is.
    pub root: String,
    /// What the level above handed it — everything it COULD turn on.
    pub available: Vec<String>,
    /// What it HAS turned on for the levels below.
    pub enabled: Vec<String>,
    /// Every level between the subtree and the top of the hierarchy, nearest first.
    ///
    /// Only above: below the subtree this daemon is the arbiter, every pane carries the same weight
    /// by design, and [`crate::resources`] already reports what each took. Above it, the terminal is
    /// one cgroup among strangers, and that is the half no amount of internal policy can fix.
    pub above: Vec<Level>,
}

/// One level of the hierarchy — an interior cgroup whose children divide its CPU between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    /// The interior cgroup itself.
    pub at: String,
    /// The name of the child this daemon descends through.
    pub ours: String,
    /// Every child of [`at`](Self::at), ours among them.
    pub children: Vec<Sibling>,
}

impl Level {
    /// The child this daemon descends through, when it is still there.
    #[must_use]
    pub fn us(&self) -> Option<&Sibling> {
        self.children.iter().find(|child| child.name == self.ours)
    }

    /// The child that took the most CPU over the window and is not ours.
    #[must_use]
    pub fn rival(&self) -> Option<&Sibling> {
        self.children
            .iter()
            .filter(|child| child.name != self.ours)
            .max_by_key(|child| child.cores())
    }
}

/// One child of a [`Level`] — a name, what it was granted, and what it took.
///
/// Both halves, for the reason a share is never rendered as a predicted split: a nominal 10:100
/// measured 18:82 on a real machine, because the kernel distributes weight per runqueue and a
/// cgroup with many threads falls short of its nominal share. The weight is what somebody SET and
/// the rate is what HAPPENED, and only the second is a fact about this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sibling {
    /// The cgroup's name at this level.
    pub name: String,
    /// Its `cpu.weight`, or `None` where the CPU controller never reached this level — which means
    /// the kernel is not arbitrating between these children at all.
    pub weight: Option<u32>,
    /// What it took over the window, as a rate.
    pub took: Cpu,
}

impl Sibling {
    /// What it took, in thousandths of a core — `0` where there is no rate yet.
    #[must_use]
    fn cores(&self) -> u64 {
        match self.took {
            Cpu::Held { millicores, .. } => millicores,
            Cpu::Settling => 0,
        }
    }
}

/// The compiler cache, as this host has it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ccache {
    /// The shim directory and how many compiler shims are in it, when it exists.
    pub shims: Option<(String, usize)>,
    /// `max_size` from `ccache -p`.
    pub max_size: Option<String>,
    /// `depend_mode` from `ccache -p`.
    pub depend_mode: Option<bool>,
    /// The cache hit rate from `ccache -s`.
    pub hit_rate: Option<Percent>,
    /// How many times the cache has evicted to stay under its ceiling, from `ccache -s`.
    pub cleanups: Option<u64>,
    /// How full the cache is against its own ceiling, from `ccache -s`.
    ///
    /// The other half of a cleanup count. 388 evictions against a cache sitting at 8% means the
    /// ceiling was raised AFTER the thrashing and the count is history; the same 388 against a
    /// cache at 99% means it is happening now. One number without the other cannot tell those
    /// apart, and they want opposite responses.
    pub occupancy: Option<Percent>,
}

// ── the verdicts ────────────────────────────────────────────────────────────────────────────────

/// The machine's `some avg60` at or above which [`Check::CpuStall`] calls the machine stalled.
///
/// Half the minute. `some` counts time when at least one task was runnable and not running, which
/// on a machine doing anything at all is never zero, so a low bar would fire on every busy box. At
/// half, the machine has spent more of the minute with somebody queued than without — which is the
/// point where a person waiting on a keystroke feels it.
const CPU_STALL_LIMIT: Percent = Percent::from_hundredths(5_000);

/// The `full avg60` at or above which a whole-machine stall is called a fault.
///
/// # Why not "above zero", which is what the design says
///
/// The design's criterion for the disk and memory rows is `full > 0`, on the reasoning that any
/// full time at all is the whole machine stopped. Driving the shipped command against an ordinary
/// idle box measured `/proc/pressure/memory` `full avg60` at **0.09%** — 54 milliseconds spread
/// across a minute, which nobody felt and nothing can be attributed to. A check that reports
/// degraded on a healthy machine is worse than no check: it teaches the reader to skip the row, and
/// the row is then not there on the day it matters.
///
/// One percent is a little over half a second of a minute in which nothing on the box ran. That is
/// small enough to be a real complaint and large enough not to be sampling noise. The measured
/// value AND this limit are printed on every row, so a reader who wants to act on 0.09% can see
/// both — which is the design's actual rule, of which the threshold is only the summary.
const STALL_LIMIT: Percent = Percent::from_hundredths(100);

/// How many runnable tasks per core [`Check::BuildSaturation`] calls oversubscribed.
///
/// Twice. At parity the scheduler is busy; at twice, every task waits longer than it runs, and the
/// design's own case was 151 runnable against 32 cores — well past this and unmistakable.
const RUNNABLE_PER_CORE: u32 = 2;

fn judge_pane_isolation(readings: &Readings) -> Finding {
    let check = Check::PaneIsolation;
    if readings.panes.is_empty() {
        return check.found(Verdict::Blind(Blind::NoPanes), Evidence::of("panes", "0"));
    }
    let mut sharers: BTreeMap<&str, Vec<PaneId>> = BTreeMap::new();
    for pane in &readings.panes {
        if let Some(cgroup) = pane.cgroup.as_deref() {
            sharers.entry(cgroup).or_default().push(pane.id);
        }
    }
    if sharers.is_empty() {
        return check.found(
            Verdict::Blind(Blind::NoHierarchy),
            Evidence::of("panes", readings.panes.len().to_string())
                .and("panes with a cgroup path", "0"),
        );
    }
    let evidence = Evidence::of("panes", readings.panes.len().to_string())
        .and("distinct cgroups", sharers.len().to_string());
    match sharers.iter().find(|(_, panes)| panes.len() > 1) {
        Some((cgroup, panes)) => check.found(
            Verdict::Degraded,
            evidence
                .and("shared cgroup", (*cgroup).to_owned())
                .and("panes sharing it", pane_list(panes)),
        ),
        None => check.found(Verdict::Healthy, evidence),
    }
}

/// The check the design document's own second worked example asks for, and the one this module's
/// third rule — *a setting is not a state* — was violated by for two rounds.
///
/// [`Check::ControllerDelegation`] reads `cgroup.subtree_control` and reports what the subtree
/// turned on. On GitHub's Linux runner it reads `cpu memory pids` and says HEALTHY, the delegation
/// succeeds, the enforcement probe answers `Available` — and every single pane's child is refused
/// admission to its leaf, so nothing is weighted and no setting on the machine can change it. That
/// is a configuration that parses perfectly and never executes, which is exactly what the design
/// says a diagnosis must catch, written by the module that says so.
///
/// It reads no file. The refusal happens at the instant of a birth and leaves no trace afterwards —
/// a refused pane's child and an unplaced pane's child are the same process in the same cgroup —
/// so the only honest source is what the kernel said at the time, which the pane has carried since
/// R342. That also keeps this check inside the module's SECOND rule: detecting admission by
/// admitting a throwaway process would be a diagnosis that mutates the machine it is diagnosing.
fn judge_pane_admission(readings: &Readings) -> Finding {
    let check = Check::PaneAdmission;
    // The MACHINE's reason outranks the pane's, which is `PaneHomes::charge`'s own precedence and
    // is load-bearing for the same measured reason: where nothing is delegated, EVERY pane is
    // unplaced, and a row that read that as an admission failure would report a fault per pane on
    // a host whose single true sentence is that it enforces nothing.
    if !readings.hierarchy {
        return check.found(
            Verdict::Blind(Blind::NoHierarchy),
            Evidence::of("panes", readings.panes.len().to_string()),
        );
    }
    let Some(subtree) = &readings.subtree else {
        return check.found(
            Verdict::Blind(Blind::NoSubtree),
            Evidence::of("panes", readings.panes.len().to_string()),
        );
    };
    if readings.panes.is_empty() {
        return check.found(
            Verdict::Blind(Blind::NoPanes),
            Evidence::of("subtree", subtree.root.clone()).and("panes", "0"),
        );
    }
    let refused: Vec<_> = readings
        .panes
        .iter()
        .filter_map(|pane| match pane.landing {
            Landing::Refused(why) => Some((pane.id, why)),
            Landing::At(_) | Landing::Unplaced => None,
        })
        .collect();
    let admitted = readings
        .panes
        .iter()
        .filter(|pane| matches!(pane.landing, Landing::At(_)))
        .count();
    let evidence = Evidence::of("panes", readings.panes.len().to_string())
        .and("in a cgroup of their own", admitted.to_string())
        .and("refused by the kernel", refused.len().to_string());
    match refused.first() {
        // The kernel's own sentence, verbatim, beside the pane it was said about. A person who has
        // never met cgroup delegation containment will not recognise the RULE, but they will
        // recognise `Permission denied` — and the remedy is written for the reader who arrives
        // holding exactly that.
        Some((id, why)) => check.found(
            Verdict::Degraded,
            evidence
                .and(
                    "panes refused",
                    pane_list(&refused.iter().map(|(id, _)| *id).collect::<Vec<_>>()),
                )
                .and("what the kernel said", format!("pane {} — {why}", id.0)),
        ),
        None => check.found(Verdict::Healthy, evidence),
    }
}

fn judge_controller_delegation(readings: &Readings) -> Finding {
    let check = Check::ControllerDelegation;
    let Some(subtree) = &readings.subtree else {
        return check.found(
            Verdict::Blind(if readings.hierarchy {
                Blind::NoSubtree
            } else {
                Blind::NoHierarchy
            }),
            Evidence::of("delegated subtree", "none"),
        );
    };
    let evidence = Evidence::of("subtree", subtree.root.clone())
        .and("available", list(&subtree.available))
        .and("enabled", list(&subtree.enabled))
        .and(
            "io",
            if subtree.enabled.iter().any(|name| name == "io") {
                "delegated"
            } else {
                "not delegated — disk time cannot be weighted between panes here"
            },
        );
    if subtree.enabled.iter().any(|name| name == "cpu") {
        check.found(Verdict::Healthy, evidence)
    } else {
        check.found(Verdict::Degraded, evidence)
    }
}

fn judge_competing_weight(readings: &Readings) -> Finding {
    let check = Check::CompetingWeight;
    let Some(subtree) = &readings.subtree else {
        return check.found(
            Verdict::Blind(if readings.hierarchy {
                Blind::NoSubtree
            } else {
                Blind::NoHierarchy
            }),
            Evidence::of("levels above this daemon", "0"),
        );
    };
    let evidence = Evidence::of("levels above this daemon", subtree.above.len().to_string());
    // The WORST level, not the first: a person can only act at one, and the one that matters is
    // wherever the biggest competitor is. A level whose rival took nothing is not competition
    // however its weights read, which is the whole of the design's first worked example.
    let contested = subtree
        .above
        .iter()
        .filter_map(|level| {
            let rival = level.rival()?;
            let ours = level.us()?;
            (rival.cores() > 0 && at_least(rival.weight, ours.weight))
                .then_some((level, ours, rival))
        })
        .max_by_key(|(_, _, rival)| rival.cores());
    match contested {
        Some((level, ours, rival)) => check.found(
            Verdict::Degraded,
            evidence
                .and("level", level.at.clone())
                .and("ours", sibling_row(ours))
                .and("competing", sibling_row(rival)),
        ),
        None => check.found(
            Verdict::Healthy,
            match subtree.above.first().and_then(Level::rival) {
                Some(rival) => evidence.and("busiest neighbour", sibling_row(rival)),
                None => evidence.and("neighbours", "none at any level"),
            },
        ),
    }
}

fn judge_cpu_stall(readings: &Readings) -> Finding {
    let check = Check::CpuStall;
    let Some(some) = readings.cpu.and_then(|cpu| cpu.some.avg60()) else {
        return check.found(
            Verdict::Blind(Blind::NoAccounting),
            Evidence::of("/proc/pressure/cpu", "absent"),
        );
    };
    let worst = readings
        .panes
        .iter()
        .filter_map(|pane| Some((pane.id, pane.waiting.avg60()?)))
        .max_by_key(|(_, avg60)| *avg60);
    let evidence = Evidence::of("machine waiting (some, 60s)", some.to_string())
        .and("limit", CPU_STALL_LIMIT.to_string())
        // The five-minute window beside the minute, because a person runs a diagnosis about
        // something that has felt slow FOR A WHILE: a minute far above the five is a burst they
        // caught, and a minute equal to it is how this machine has been living.
        .and(
            "and over 5 minutes",
            readings
                .cpu
                .and_then(|cpu| cpu.some.avg300())
                .map_or_else(|| "not accounted".to_owned(), |long| long.to_string()),
        )
        .and(
            "worst pane",
            match worst {
                Some((id, avg60)) => format!("pane {id} at {avg60}"),
                None => "no pane reports pressure".to_owned(),
            },
        );
    if some >= CPU_STALL_LIMIT {
        check.found(Verdict::Degraded, evidence)
    } else {
        check.found(Verdict::Healthy, evidence)
    }
}

fn judge_io_stall(readings: &Readings) -> Finding {
    stall_of(
        Check::IoStall,
        "/proc/pressure/io",
        readings.io,
        readings
            .subtree
            .as_ref()
            .is_some_and(|subtree| subtree.enabled.iter().any(|name| name == "io")),
    )
}

fn judge_memory_stall(readings: &Readings) -> Finding {
    stall_of(
        Check::MemoryStall,
        "/proc/pressure/memory",
        readings.memory,
        readings
            .subtree
            .as_ref()
            .is_some_and(|subtree| subtree.enabled.iter().any(|name| name == "memory")),
    )
}

/// The `full`-row verdict both whole-machine stall checks share.
///
/// One function because the two differ only in which file they read and which controller would let
/// a person do something about it — and two copies of "is `full` above zero" is how two rows of one
/// report come to disagree about what zero means.
fn stall_of(check: Check, source: &str, pressure: Option<Pressure>, arbitrable: bool) -> Finding {
    let Some(full) = pressure.and_then(|pressure| pressure.full.avg60()) else {
        return check.found(
            Verdict::Blind(Blind::NoAccounting),
            Evidence::of(source.to_owned(), "no full row"),
        );
    };
    let evidence = Evidence::of("stopped (full, 60s)", full.to_string())
        .and("limit", STALL_LIMIT.to_string())
        .and(
            "waiting (some, 60s)",
            pressure
                .and_then(|pressure| pressure.some.avg60())
                .map_or_else(|| "not accounted".to_owned(), |some| some.to_string()),
        )
        .and(
            "arbitrable between panes",
            if arbitrable {
                "yes — the controller is delegated"
            } else {
                "no — the controller is not delegated here"
            },
        );
    if full >= STALL_LIMIT {
        check.found(Verdict::Degraded, evidence)
    } else {
        check.found(Verdict::Healthy, evidence)
    }
}

fn judge_swapping(readings: &Readings) -> Finding {
    let check = Check::Swapping;
    let swapped: Vec<(PaneId, u64)> = readings
        .panes
        .iter()
        .filter_map(|pane| Some((pane.id, pane.swapped?)))
        .collect();
    if swapped.is_empty() {
        return check.found(
            Verdict::Blind(if readings.panes.is_empty() {
                Blind::NoPanes
            } else {
                Blind::NoHierarchy
            }),
            Evidence::of("panes with a readable swap figure", "0"),
        );
    }
    let total: u64 = swapped.iter().map(|(_, bytes)| bytes).sum();
    let on_disk: Vec<PaneId> = swapped
        .iter()
        .filter(|(_, bytes)| *bytes > 0)
        .map(|(id, _)| *id)
        .collect();
    let evidence = Evidence::of("panes with pages in swap", on_disk.len().to_string())
        .and("total swapped", bytes(total))
        .and(
            "vm.swappiness",
            readings
                .swappiness
                .map_or_else(|| "unreadable".to_owned(), |value| value.to_string()),
        );
    if on_disk.is_empty() {
        check.found(Verdict::Healthy, evidence)
    } else {
        check.found(
            Verdict::Degraded,
            evidence.and("which", pane_list(&on_disk)),
        )
    }
}

fn judge_build_saturation(readings: &Readings) -> Finding {
    let check = Check::BuildSaturation;
    let Some(load) = readings.load else {
        return check.found(
            Verdict::Blind(Blind::NoAccounting),
            Evidence::of("/proc/loadavg", "unreadable"),
        );
    };
    let evidence = Evidence::of("runnable", load.runnable.to_string())
        .and("cores", load.cores.to_string())
        .and("all tasks", load.threads.to_string())
        .and("processes in panes", load.pane_procs.to_string());
    // `cores == 0` is a host whose `/proc/stat` this reader did not understand, and multiplying it
    // out would make every machine oversubscribed. It reports the numbers and judges nothing.
    if load.cores > 0 && load.runnable >= load.cores.saturating_mul(RUNNABLE_PER_CORE) {
        check.found(Verdict::Degraded, evidence)
    } else {
        check.found(Verdict::Healthy, evidence)
    }
}

fn judge_ccache_on_path(readings: &Readings) -> Finding {
    let check = Check::CcacheOnPath;
    let Some((dir, shims)) = readings.ccache.as_ref().and_then(|c| c.shims.clone()) else {
        return check.found(
            Verdict::Blind(Blind::NotInstalled),
            Evidence::of("ccache compiler shims", "none found"),
        );
    };
    let reached: Vec<PaneId> = readings
        .panes
        .iter()
        .filter(|pane| pane.ccache_on_path == Some(true))
        .map(|pane| pane.id)
        .collect();
    let readable = readings
        .panes
        .iter()
        .filter(|pane| pane.ccache_on_path.is_some())
        .count();
    if readable == 0 {
        return check.found(
            Verdict::Blind(Blind::NoPanes),
            Evidence::of("shims", format!("{shims} in {dir}"))
                .and("panes whose PATH could be read", "0"),
        );
    }
    let evidence = Evidence::of("shims", format!("{shims} in {dir}")).and(
        "panes started with it on PATH",
        format!("{}/{readable}", reached.len()),
    );
    if reached.is_empty() {
        check.found(Verdict::Degraded, evidence)
    } else {
        check.found(Verdict::Healthy, evidence.and("which", pane_list(&reached)))
    }
}

fn judge_ccache_sizing(readings: &Readings) -> Finding {
    let check = Check::CcacheSizing;
    let Some(cleanups) = readings.ccache.as_ref().and_then(|c| c.cleanups) else {
        // WHICH absence, from the half of the reading that does not need the program: shims on the
        // filesystem mean it is installed and something stopped it answering HERE. Collapsing the
        // two put `33 shims in /usr/lib/ccache` and `not installed on this host` in one report.
        let shims = readings
            .ccache
            .as_ref()
            .and_then(|ccache| ccache.shims.as_ref());
        return check.found(
            Verdict::Blind(if shims.is_some() {
                Blind::Unanswered
            } else {
                Blind::NotInstalled
            }),
            match shims {
                Some((dir, count)) => Evidence::of("ccache -s", "no cleanup count")
                    .and("but its shims are here", format!("{count} in {dir}")),
                None => Evidence::of("ccache -s", "no cleanup count"),
            },
        );
    };
    let ccache = readings
        .ccache
        .as_ref()
        .expect("the cleanup count came from it");
    let evidence = Evidence::of("cleanups", cleanups.to_string())
        .and(
            "max_size",
            ccache
                .max_size
                .clone()
                .unwrap_or_else(|| "unset".to_owned()),
        )
        .and(
            "hit rate",
            ccache
                .hit_rate
                .map_or_else(|| "unreported".to_owned(), |rate| rate.to_string()),
        )
        .and(
            "cache is full",
            ccache
                .occupancy
                .map_or_else(|| "unreported".to_owned(), |full| full.to_string()),
        )
        .and(
            "depend_mode",
            match ccache.depend_mode {
                Some(true) => "on",
                Some(false) => "off",
                None => "unreported",
            },
        );
    if cleanups > 0 {
        check.found(Verdict::Degraded, evidence)
    } else {
        check.found(Verdict::Healthy, evidence)
    }
}

fn judge_fast_linker(readings: &Readings) -> Finding {
    let check = Check::FastLinker;
    if readings.paths == 0 {
        return check.found(
            Verdict::Blind(Blind::NoPanes),
            Evidence::of("PATHs searched", "0"),
        );
    }
    let evidence = Evidence::of("PATHs searched", readings.paths.to_string());
    if readings.linkers.is_empty() {
        check.found(
            Verdict::Degraded,
            evidence.and("fast linkers found", "none"),
        )
    } else {
        check.found(
            Verdict::Healthy,
            evidence.and("fast linkers found", list(&readings.linkers)),
        )
    }
}

/// # Why *nobody attached* is the criterion and *more than one daemon* is not
///
/// Two daemons on two sockets is this project's supported shape — `crate::durability` keys a
/// snapshot on the socket precisely so they can coexist — so a count would flag the design. What
/// the investigation actually found was a daemon that no client had been on since the turn that
/// spawned it died, and *nobody is attached* is the reading that separates it from every legitimate
/// second daemon.
///
/// # Why the reporting daemon is excluded rather than counted
///
/// Not a convenience: the report reached the reader over an attachment to that daemon, so calling
/// it abandoned would be a sentence the reader can see is false — [`Blind::Unanswered`]'s lesson,
/// which was paid for by a report that printed two contradictory rows about ccache. An in-process
/// host reaches this with no client at all, and it is the same exclusion that keeps that honest.
fn judge_stray_daemon(readings: &Readings) -> Finding {
    let check = Check::StrayDaemon;
    let Some(daemons) = readings.daemons.as_ref() else {
        return check.found(
            Verdict::Blind(Blind::NoProcessTable),
            Evidence::of("daemons of this program", "not countable on this host"),
        );
    };
    let stray: Vec<&DaemonReading> = daemons
        .found
        .iter()
        .filter(|daemon| daemon.pid != daemons.mine && daemon.attached == 0)
        .collect();
    let evidence = daemons.found.iter().fold(
        Evidence::of(
            format!("daemons running `{}`", daemons.program),
            format!(
                "{}, {}",
                daemons.found.len(),
                match stray.len() {
                    0 => "and somebody is attached to every one but this report's own".to_owned(),
                    count => format!("{count} with nobody attached"),
                },
            ),
        ),
        |evidence, daemon| evidence.and(format!("pid {}", daemon.pid), daemon_row(daemon, daemons)),
    );
    if stray.is_empty() {
        check.found(Verdict::Healthy, evidence)
    } else {
        check.found(Verdict::Degraded, evidence)
    }
}

/// `/usr/bin/sprag-term on /run/user/1000/sprag.sock, 3 attached`.
fn daemon_row(daemon: &DaemonReading, daemons: &Daemons) -> String {
    let attached = match daemon.attached {
        0 => "nobody attached".to_owned(),
        count => format!("{count} attached"),
    };
    // Marked rather than left to the reader to work out from a pid they did not choose: the row
    // that is exempt from the verdict has to say so where the verdict is read, or the report and
    // its own criterion read as contradicting each other.
    let mine = if daemon.pid == daemons.mine {
        " — this report's own"
    } else {
        ""
    };
    format!("{} on {}, {attached}{mine}", daemon.image, daemon.socket)
}

/// Whether `weight` is at least `ours`, where an ABSENT weight means the CPU controller never
/// reached that level.
///
/// A level with no weights is one the kernel is not arbitrating at all, so a busy neighbour there
/// is taking whatever it can run — which is the worst case, not the exempt one. That is why absence
/// answers `true` on both sides rather than dropping the level.
fn at_least(weight: Option<u32>, ours: Option<u32>) -> bool {
    match (weight, ours) {
        (Some(theirs), Some(ours)) => theirs >= ours,
        _ => true,
    }
}

/// `pane 1, pane 4`.
fn pane_list(panes: &[PaneId]) -> String {
    panes
        .iter()
        .map(|id| format!("pane {id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `cpu memory pids`, or the honest word for an empty list.
fn list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(" ")
    }
}

/// `system.slice weight 100, took 6.41 cores over 0.5s`.
fn sibling_row(sibling: &Sibling) -> String {
    let weight = sibling.weight.map_or_else(
        || "no weight".to_owned(),
        |weight| format!("weight {weight}"),
    );
    match sibling.took {
        Cpu::Held {
            millicores,
            over_ms,
        } => format!(
            "{} {weight}, took {}.{:02} cores over {}.{:01}s",
            sibling.name,
            millicores / 1000,
            (millicores % 1000) / 10,
            over_ms / 1000,
            (over_ms % 1000) / 100,
        ),
        Cpu::Settling => format!("{} {weight}, no rate yet", sibling.name),
    }
}

/// Bytes as a person reads them.
fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ── the reading ─────────────────────────────────────────────────────────────────────────────────

/// Where a [`Readings`] comes from — the real machine, or a directory standing in for one.
///
/// Every path this module opens is joined onto one of these, so the whole capture can be pointed at
/// a fixture. That is the same seam `Enforcement::probe` opened for the same reason: a test that
/// reads the real `/proc` asserts whatever the developer's box happens to be, which passes
/// everywhere and discriminates nowhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    /// The `/proc` filesystem.
    pub proc: PathBuf,
    /// The cgroup v2 mount point, when this host has one.
    pub cgroup: Option<PathBuf>,
    /// Where this distribution keeps the compiler shims that route a build through ccache.
    pub shims: PathBuf,
    /// The compiler-cache program to ask for its configuration, if it is on the daemon's own
    /// `PATH`. `None` skips the two ccache checks entirely, which is what a fixture wants.
    pub ccache: Option<PathBuf>,
}

impl Default for Sources {
    /// This machine.
    fn default() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            cgroup: crate::share::mount_point().map(Path::to_path_buf),
            shims: PathBuf::from(CCACHE_SHIMS),
            ccache: Some(PathBuf::from("ccache")),
        }
    }
}

/// Where Debian and Ubuntu put the compiler shims. Named as a constant because it is a
/// DISTRIBUTION fact with an expiry date, not a property of ccache.
const CCACHE_SHIMS: &str = "/usr/lib/ccache";

/// The fast linkers a build could be pointed at, in the order a report lists them.
const FAST_LINKERS: [&str; 3] = ["mold", "ld.lld", "lld"];

/// What the daemon knows that the machine does not — which panes are alive, and where its own
/// subtree is.
///
/// Handed in rather than reached for, because this module has no registry: the caller that has one
/// builds this from it, and every path below is then a plain file read that a fixture can stand in
/// for.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Subject {
    /// One entry per live pane.
    pub panes: Vec<PaneSite>,
    /// The delegated subtree this daemon builds panes into, when it has one.
    pub subtree: Option<PathBuf>,
    /// This process, for the one check whose subject is other processes like it.
    ///
    /// [`None`] where the process could not name itself, which makes [`Check::StrayDaemon`] blind:
    /// a peer can only be recognised against something, and inventing a program name here would
    /// make every daemon on the machine either invisible or a stranger.
    pub daemon: Option<DaemonSelf>,
}

/// The reporting process, in the two terms [`Check::StrayDaemon`] needs to recognise its peers.
///
/// Read from the process rather than handed in as a constant, so a fork, a rename or a second
/// build cannot leave the check hunting for a program that is not the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonSelf {
    /// This process's pid, so its own row can be excluded from its own verdict.
    pub pid: u32,
    /// The FILE NAME of this process's executable — see the module docs for why a name and not a
    /// path.
    pub program: String,
}

impl DaemonSelf {
    /// This process, when it can name itself.
    #[must_use]
    pub fn here() -> Option<Self> {
        Some(Self {
            pid: std::process::id(),
            program: std::env::current_exe()
                .ok()?
                .file_name()?
                .to_str()?
                .to_owned(),
        })
    }
}

/// One live pane, as the daemon knows it before `/proc` is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneSite {
    /// Which pane.
    pub id: PaneId,
    /// The process on the far side of its pty.
    pub pid: u32,
    /// Whether this pane's child reached the cgroup opened for it, and the kernel's reason if not.
    ///
    /// The one reading in this module that `/proc` cannot supply. A refused pane and a pane nobody
    /// tried to place are the SAME process in the SAME cgroup afterwards — the daemon's own — so
    /// the difference exists only at the instant of the birth, and only the daemon was there. Every
    /// other field here is re-read on each capture; this one is remembered, because there is
    /// nothing left to re-read it from.
    pub landing: Landing,
}

impl Subject {
    /// Every live pane in `registry`, and the subtree its windows place panes into.
    ///
    /// # Locking
    ///
    /// Registry → pool, this crate's one direction, never nested: the window pools are taken under
    /// the registry lock and then each is locked alone. Nothing here opens a file, so no pool lock
    /// is held across I/O — the whole point of building this value before the capture rather than
    /// reading the machine with a pool in hand.
    ///
    /// A pane whose child has been reaped has no pid and is left out, deliberately: its `/proc`
    /// entries are gone and a recycled pid would read a stranger's cgroup and swap into this
    /// report. That is `PanePty::pid`'s own gate, honoured rather than worked around.
    #[must_use]
    pub fn of(registry: &std::sync::Arc<std::sync::Mutex<crate::SessionRegistry>>) -> Self {
        use std::sync::PoisonError;

        let pools: Vec<_> = {
            let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
            reg.window_pools().into_iter().flatten().collect()
        };
        let mut subtree = None;
        let mut panes = Vec::new();
        for pool in &pools {
            let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            subtree = subtree.or_else(|| {
                pool.pane_homes()
                    .tree_root()
                    .map(std::path::Path::to_path_buf)
            });
            panes.extend(pool.panes().iter().filter_map(|pane| {
                Some(PaneSite {
                    id: pane.id(),
                    pid: pane.pty().pid()?,
                    landing: pane.home(),
                })
            }));
        }
        Self {
            panes,
            subtree,
            daemon: DaemonSelf::here(),
        }
    }
}

impl Readings {
    /// Read this machine.
    ///
    /// # Why it takes a window rather than a single pass
    ///
    /// [`Check::CompetingWeight`] is the one check that cannot be answered by a snapshot. A
    /// cumulative `cpu.stat` says a neighbour used CPU at some point since boot, and the question
    /// is whether it is using it NOW; the difference between a batch job that finished this morning
    /// and one that is running is the whole verdict. So the levels above the subtree are read
    /// twice, `window` apart, and every rate states the window it covers.
    ///
    /// Everything else is a snapshot taken on the second pass, so the report describes one moment
    /// rather than the beginning and end of the window.
    #[must_use]
    pub fn capture(subject: &Subject, sources: &Sources, window: Duration) -> Self {
        let before = baseline(subject, sources);
        // The one sleep in this module, and the reason the whole capture is not on a hot path.
        std::thread::sleep(window);
        Self::capture_after(subject, sources, &before, window)
    }

    /// The whole capture with the window's opening reading handed in — the seam the fixtures drive.
    ///
    /// Split out so the two-sample half is exercised without a test sleeping through a window and
    /// racing a background writer against it. A fixture takes the [`baseline`], edits the counters
    /// by hand, and calls this with the window it means: the arithmetic, the pairing of levels and
    /// the verdict all run, and nothing about the result depends on when a thread woke up.
    fn capture_after(
        subject: &Subject,
        sources: &Sources,
        before: &BTreeMap<PathBuf, u64>,
        window: Duration,
    ) -> Self {
        let ccache = read_ccache(sources);
        let shims = ccache
            .as_ref()
            .and_then(|ccache| ccache.shims.as_ref())
            .map(|(dir, _)| dir.clone());
        let paths = distinct_paths(&subject.panes, sources);
        Self {
            cpu: pressure(sources, "cpu"),
            io: pressure(sources, "io"),
            memory: pressure(sources, "memory"),
            swappiness: read_number(&sources.proc.join("sys/vm/swappiness")),
            load: read_load(sources, &subject.panes),
            panes: subject
                .panes
                .iter()
                .map(|site| read_pane(site, sources, shims.as_deref()))
                .collect(),
            subtree: subject
                .subtree
                .as_deref()
                .zip(sources.cgroup.as_deref())
                .map(|(subtree, mount)| read_subtree(mount, subtree, before, window)),
            ccache,
            linkers: FAST_LINKERS
                .into_iter()
                .filter(|linker| {
                    paths
                        .iter()
                        .any(|dir| Path::new(dir).join(linker).is_file())
                })
                .map(str::to_owned)
                .collect(),
            paths: paths.len(),
            hierarchy: sources.cgroup.as_deref().is_some_and(Path::is_dir),
            daemons: read_daemons(subject, sources),
        }
    }
}

/// Every process on this machine running the same program as this one, that is LISTENING.
///
/// # Why listening, and not the program alone
///
/// The daemon and its clients are one binary here — `sprag-term` serves when it is given
/// `--daemon` and drives a terminal otherwise — so a program name alone would count every client
/// as a daemon. Holding a listening socket is the thing a daemon does that a client does not, and
/// it is also the thing that makes an abandoned one COST something: the socket is what a later
/// process finds and connects to.
///
/// # Why the count of attached clients comes from the kernel and not from the daemon
///
/// Asking a daemon how many clients it has means connecting to it, which makes this reading one of
/// them, and means a daemon that has wedged answers nothing — the exact daemon most worth finding.
/// `/proc/net/unix` carries the listener's path on every SERVER-side end of a connection to it,
/// because only a bound socket has an address to print and a client's own end is unbound. Counting
/// those rows is therefore counting the clients, from outside, with nothing connected and nothing
/// written.
///
/// # ⚠⚠⚠⚠⚠ And a connection that has ARRIVED is not yet a connection that was ACCEPTED
///
/// Measured on 2026-09-14 against a real listener, one client at a time, because the first draft of
/// this reader assumed otherwise and a real-kernel test refuted it in one run:
///
/// ```text
/// bound only :  [('01', 4098871)]                            the door
/// 1 pending  :  [('01', 4098871), ('02', 0)]                 connected, NOT yet accepted
/// 1 accepted :  [('01', 4098871), ('03', 4117604)]           after accept(), and a new inode
/// ```
///
/// A pending end has no `struct socket` yet, so the kernel prints it `02` with inode `0` — and a
/// reader that counted only `03` would call a daemon whose accept loop has stopped *abandoned*,
/// which is the one daemon on the machine people are actively failing to reach. Both states are a
/// client on the door and both are counted; the listener itself is `01` and is neither.
fn read_daemons(subject: &Subject, sources: &Sources) -> Option<Daemons> {
    let me = subject.daemon.as_ref()?;
    let sockets = SocketTable::read(sources)?;
    let mut found: Vec<DaemonReading> = process_ids(sources)
        .into_iter()
        .filter_map(|pid| {
            let image = image_of(pid, sources)?;
            (program_of(&image) == me.program).then_some(())?;
            let socket = sockets.listening_of(pid, sources)?;
            Some(DaemonReading {
                attached: sockets.clients_on(&socket),
                pid,
                image,
                socket,
            })
        })
        .collect();
    found.sort_by_key(|daemon| daemon.pid);
    Some(Daemons {
        program: me.program.clone(),
        mine: me.pid,
        found,
    })
}

/// Every pid `/proc` names, unsorted.
///
/// An unreadable `/proc` answers an empty list rather than an absence: the caller has already had
/// [`SocketTable::read`] fail on the same filesystem by then, so there is one place that decides
/// this host cannot be read and it is not here.
fn process_ids(sources: &Sources) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(&sources.proc) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}

/// What `/proc/<pid>/exe` resolves to, with the kernel's `(deleted)` suffix left on.
///
/// [`std::fs::read_link`] and not [`std::fs::canonicalize`]: the second would follow the link to a
/// real file, which fails for exactly the deleted image this most wants to report, and would
/// resolve a symlinked install directory into a path the reader does not recognise.
fn image_of(pid: u32, sources: &Sources) -> Option<String> {
    Some(
        std::fs::read_link(sources.proc.join(pid.to_string()).join("exe"))
            .ok()?
            .to_str()?
            .to_owned(),
    )
}

/// The program a resolved image names, with the kernel's `(deleted)` suffix taken back off.
///
/// The suffix is part of the link's TEXT, not of the file name, so a daemon whose binary has been
/// replaced under it would otherwise be running a program called `sprag-term (deleted)` and match
/// nothing. That daemon is the most interesting one this check can find.
fn program_of(image: &str) -> &str {
    let path = image.strip_suffix(DELETED_IMAGE).unwrap_or(image);
    Path::new(path)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(path)
}

/// What the kernel appends to `/proc/<pid>/exe` once the image behind it is unlinked.
const DELETED_IMAGE: &str = " (deleted)";

/// `/proc/net/unix`, parsed once per capture.
///
/// One read for the whole machine rather than one per daemon: the file is a snapshot of the
/// kernel's socket table, and two reads of it are two different moments — a client that connected
/// between them would be counted against one daemon and not the other.
#[derive(Debug)]
struct SocketTable {
    rows: Vec<SocketRow>,
}

/// One row of `/proc/net/unix`, in the three fields this module reads.
#[derive(Debug)]
struct SocketRow {
    /// The socket's inode, which is how `/proc/<pid>/fd` names it.
    inode: u64,
    /// Whether the kernel has it listening for connections, or already connected to one.
    state: SocketState,
    /// The filesystem path it is bound to. Only a bound socket has one, which is why a connected
    /// row carrying a path is a SERVER-side connection and not a client's.
    path: String,
}

/// The three socket states this check tells apart, in the kernel's own numbering.
///
/// Three and not two: see [`read_daemons`] for the measurement that separates the middle one, which
/// a reader is most likely to leave out and which is the one a wedged daemon's clients are all in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketState {
    /// `01` — bound and accepting. A daemon's door.
    Listening,
    /// `02` — a connection the kernel has put on a door that has not accepted it yet. Its inode is
    /// `0`, because there is no `struct socket` behind it to have one.
    Arriving,
    /// `03` — one end of an accepted connection.
    Connected,
}

impl SocketState {
    /// Whether this end is a CLIENT on somebody's door, as opposed to the door itself.
    const fn is_a_client(self) -> bool {
        match self {
            Self::Arriving | Self::Connected => true,
            Self::Listening => false,
        }
    }
}

impl SocketTable {
    /// The kernel's unix socket table, or the absence of one.
    fn read(sources: &Sources) -> Option<Self> {
        let table = std::fs::read_to_string(sources.proc.join("net/unix")).ok()?;
        Some(Self {
            rows: table.lines().filter_map(SocketRow::parse).collect(),
        })
    }

    /// The path `pid` is listening on, if it is listening on one.
    ///
    /// A process may hold several sockets, and the first LISTENING one with a path is its door —
    /// this daemon opens exactly one. A pid whose `/proc/<pid>/fd` cannot be read (another user's,
    /// or one that exited between the two reads) is simply not a daemon this can see, which is the
    /// same answer as not being one.
    fn listening_of(&self, pid: u32, sources: &Sources) -> Option<String> {
        let fds = std::fs::read_dir(sources.proc.join(pid.to_string()).join("fd")).ok()?;
        let held: BTreeSet<u64> = fds
            .flatten()
            .filter_map(|entry| socket_inode(&std::fs::read_link(entry.path()).ok()?))
            .collect();
        self.rows
            .iter()
            .find(|row| {
                row.state == SocketState::Listening
                    && !row.path.is_empty()
                    && held.contains(&row.inode)
            })
            .map(|row| row.path.clone())
    }

    /// How many clients the kernel shows on the door at `path`, accepted or still arriving.
    ///
    /// See [`read_daemons`] for why this counts the clients and why it counts both states: the
    /// listening row is neither, and a client's OWN end carries no path, so every remaining row
    /// bearing this path is one connection the daemon is holding or has been handed.
    fn clients_on(&self, path: &str) -> usize {
        self.rows
            .iter()
            .filter(|row| row.state.is_a_client() && row.path == path)
            .count()
    }
}

impl SocketRow {
    /// One line of `/proc/net/unix`, or [`None`] for its header and for anything this does not
    /// understand.
    ///
    /// The columns are `Num RefCount Protocol Flags Type St Inode Path`, and the path is LAST —
    /// so it is taken as the remainder of the line rather than as a field, because a socket path
    /// may contain a space and splitting on whitespace would truncate it to its first word.
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split_whitespace();
        let state = match fields.nth(5)? {
            "01" => SocketState::Listening,
            "02" => SocketState::Arriving,
            "03" => SocketState::Connected,
            _ => return None,
        };
        let inode = fields.next()?.parse().ok()?;
        Some(Self {
            inode,
            state,
            path: rest_after(line, 7).trim_end().to_owned(),
        })
    }
}

/// Everything after the first `fields` whitespace-separated fields of `line`, verbatim.
///
/// Written as an offset walk rather than as `split_whitespace().skip(n).collect::<Vec<_>>().join(" ")`
/// because that spelling REBUILDS the tail with single spaces, and the tail here is a filesystem
/// path: one written with two spaces in it would come back as a path that does not exist.
fn rest_after(line: &str, fields: usize) -> &str {
    let mut rest = line;
    for _ in 0..fields {
        rest = rest.trim_start();
        let Some(end) = rest.find(char::is_whitespace) else {
            return "";
        };
        rest = &rest[end..];
    }
    rest.trim_start()
}

/// The inode in a `socket:[12345]` symlink target, which is how `/proc/<pid>/fd` spells a socket.
fn socket_inode(target: &Path) -> Option<u64> {
    target
        .to_str()?
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// `/proc/pressure/<resource>`, or the absence.
fn pressure(sources: &Sources, resource: &str) -> Option<Pressure> {
    let path = sources.proc.join("pressure").join(resource);
    path.exists().then(|| Pressure::read(&path))
}

/// One whole-number control file.
fn read_number(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The runnable/total pair, the core count, and how many processes the panes hold.
fn read_load(sources: &Sources, panes: &[PaneSite]) -> Option<Load> {
    let loadavg = std::fs::read_to_string(sources.proc.join("loadavg")).ok()?;
    // `0.42 0.31 0.28 3/2295 1940976` — the fourth field is runnable/total.
    let (runnable, threads) = loadavg.split_ascii_whitespace().nth(3)?.split_once('/')?;
    Some(Load {
        runnable: runnable.parse().ok()?,
        threads: threads.parse().ok()?,
        cores: cores(sources),
        pane_procs: u32::try_from(
            panes
                .iter()
                .filter_map(|site| pane_node(site, sources))
                .map(|node| node.procs().len())
                .sum::<usize>(),
        )
        .unwrap_or(u32::MAX),
    })
}

/// How many cores `/proc/stat` reports, counted from its per-CPU rows.
///
/// From `/proc/stat` rather than from the runtime's parallelism hint because that hint is not a
/// file: it cannot be pointed at a fixture, so a check built on it could never be shown to
/// discriminate. The rows are `cpu` (the total) then `cpu0`, `cpu1`, …, and only the numbered ones
/// are counted.
fn cores(sources: &Sources) -> u32 {
    let Ok(body) = std::fs::read_to_string(sources.proc.join("stat")) else {
        return 0;
    };
    u32::try_from(
        body.lines()
            .filter(|line| {
                line.split_ascii_whitespace()
                    .next()
                    .and_then(|name| name.strip_prefix("cpu"))
                    .is_some_and(|rest| {
                        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                    })
            })
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Where a pane's child actually is in the hierarchy, and the node that is.
fn pane_node(site: &PaneSite, sources: &Sources) -> Option<CgroupNode> {
    let mount = sources.cgroup.as_deref()?;
    let relative = pane_cgroup(site, sources)?;
    Some(CgroupNode::at(mount.join(relative.trim_start_matches('/'))))
}

/// The unified-hierarchy path from a pane's `/proc/<pid>/cgroup`.
fn pane_cgroup(site: &PaneSite, sources: &Sources) -> Option<String> {
    let body =
        std::fs::read_to_string(sources.proc.join(site.pid.to_string()).join("cgroup")).ok()?;
    body.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
        .map(str::to_owned)
}

/// One pane's row. `shims` is the compiler-shim directory, when this host has one.
fn read_pane(site: &PaneSite, sources: &Sources, shims: Option<&str>) -> PaneReading {
    let cgroup = pane_cgroup(site, sources);
    let node = pane_node(site, sources);
    PaneReading {
        id: site.id,
        cgroup,
        swapped: node.as_ref().map(|node| {
            node.procs()
                .into_iter()
                .filter_map(|pid| swapped_kib(pid, sources))
                .sum::<u64>()
                * 1024
        }),
        waiting: node.map_or(Waiting::NotAccounted, |node| node.pressure().some),
        // `None` where the pane's own `PATH` could not be read, which is counted as UNREAD rather
        // than as clean: a pane whose environ is gone says nothing about whether the cache is
        // reached, and a `false` there would be a verdict nobody measured.
        ccache_on_path: shims
            .and_then(|shims| Some(lists_dir(&path_of_pid(site.pid, sources)?, shims))),
        landing: site.landing,
    }
}

/// `VmSwap` from one process's status, in kibibytes.
fn swapped_kib(pid: u32, sources: &Sources) -> Option<u64> {
    let body = std::fs::read_to_string(sources.proc.join(pid.to_string()).join("status")).ok()?;
    body.lines()
        .find_map(|line| line.strip_prefix("VmSwap:"))
        .and_then(|value| value.split_ascii_whitespace().next())
        .and_then(|kib| kib.parse().ok())
}

/// The `PATH` one process was EXECUTED with — see the module docs for what that cannot see.
fn path_of_pid(pid: u32, sources: &Sources) -> Option<String> {
    let environ = std::fs::read(sources.proc.join(pid.to_string()).join("environ")).ok()?;
    environ
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .find_map(|entry| entry.strip_prefix("PATH="))
        .map(str::to_owned)
}

/// Whether a `PATH` names `dir` as one of its entries — whole entries, never a substring, because
/// `/usr/lib/ccache-old` contains `/usr/lib/ccache`.
fn lists_dir(path: &str, dir: &str) -> bool {
    path.split(':').any(|entry| entry == dir)
}

/// Every distinct `PATH` the panes were started with, as directories.
fn distinct_paths(panes: &[PaneSite], sources: &Sources) -> BTreeSet<String> {
    panes
        .iter()
        .filter_map(|site| path_of_pid(site.pid, sources))
        .flat_map(|path| {
            path.split(':')
                .filter(|entry| !entry.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The cumulative CPU of every cgroup at every level above the subtree, keyed by path — the
/// window's opening reading.
///
/// Empty where there is no hierarchy or no subtree, which makes every rate [`Cpu::Settling`]: a
/// neighbour whose baseline is missing has no window to be measured over, and reporting it as zero
/// cores would say *this one is idle* about the one nobody sampled.
fn baseline(subject: &Subject, sources: &Sources) -> BTreeMap<PathBuf, u64> {
    let mut seen = BTreeMap::new();
    let Some((mount, subtree)) = sources.cgroup.as_deref().zip(subject.subtree.as_deref()) else {
        return seen;
    };
    for level in levels_above(mount, subtree) {
        for child in CgroupNode::at(level).children() {
            if let Some(usec) = child.cpu_usec() {
                seen.insert(child.path().to_path_buf(), usec);
            }
        }
    }
    seen
}

/// Every interior cgroup between `subtree`'s parent and the mount point, nearest first.
fn levels_above(mount: &Path, subtree: &Path) -> Vec<PathBuf> {
    let mut levels = Vec::new();
    let mut at = subtree.parent();
    while let Some(level) = at {
        levels.push(level.to_path_buf());
        if level == mount {
            break;
        }
        at = level.parent();
    }
    levels
}

/// The subtree's own delegation, and every level above it with what each child took.
fn read_subtree(
    mount: &Path,
    subtree: &Path,
    before: &BTreeMap<PathBuf, u64>,
    window: Duration,
) -> SubtreeReading {
    let node = CgroupNode::at(subtree.to_path_buf());
    SubtreeReading {
        root: subtree.display().to_string(),
        available: node.controllers(),
        enabled: node.subtree_control(),
        above: levels_above(mount, subtree)
            .into_iter()
            .filter_map(|level| {
                // `ours` is the child of THIS level that the subtree descends through — the next
                // component of the subtree's path below it.
                let ours = subtree
                    .strip_prefix(&level)
                    .ok()?
                    .components()
                    .next()?
                    .as_os_str()
                    .to_string_lossy()
                    .into_owned();
                Some(Level {
                    at: level.display().to_string(),
                    ours,
                    children: CgroupNode::at(level)
                        .children()
                        .into_iter()
                        .map(|child| Sibling {
                            name: child.name(),
                            weight: child.weight(),
                            took: match (before.get(child.path()), child.cpu_usec()) {
                                (Some(from), Some(to)) => Cpu::over(window, *from, to),
                                _ => Cpu::Settling,
                            },
                        })
                        .collect(),
                })
            })
            .collect(),
    }
}

/// The compiler cache, when this host has one.
fn read_ccache(sources: &Sources) -> Option<Ccache> {
    let shims =
        shim_count(&sources.shims).map(|count| (sources.shims.display().to_string(), count));
    let stats = sources.ccache.as_deref().and_then(|program| {
        Some((
            Probe::STATISTICS.run(program)?,
            Probe::CONFIGURATION.run(program)?,
        ))
    });
    let (statistics, configuration) = match (shims.as_ref(), stats) {
        (None, None) => return None,
        (_, stats) => stats.unzip(),
    };
    let statistics = statistics.unwrap_or_default();
    let configuration = configuration.unwrap_or_default();
    Some(Ccache {
        shims,
        max_size: keyed(&configuration, "max_size"),
        depend_mode: keyed(&configuration, "depend_mode").map(|value| value == "true"),
        hit_rate: keyed_after(&statistics, "Hits:").and_then(parenthesised_percent),
        cleanups: keyed_after(&statistics, "Cleanups:").and_then(|value| value.parse().ok()),
        occupancy: statistics
            .lines()
            .find(|line| line.trim_start().starts_with(CACHE_SIZE))
            .and_then(parenthesised_percent),
    })
}

/// How many compiler shims a directory holds, or `None` where there is no such directory.
fn shim_count(dir: &Path) -> Option<usize> {
    Some(std::fs::read_dir(dir).ok()?.flatten().count())
}

/// One `key = value` line of `ccache -p`.
///
/// The key is the LAST token before the `=`, not the first, because the tool prints where each
/// setting came from ahead of it: `(/home/…/ccache.conf) max_size = 50.0 GB`. Read from the front
/// this file answers nothing at all — measured against the shipped tool rather than assumed, which
/// is the whole reason the check reads a program's output instead of a config file.
fn keyed(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.split_ascii_whitespace().last()? == key).then(|| value.trim().to_owned())
    })
}

/// What follows a label on one line of `ccache -s`.
///
/// The FIRST such line, and the statistics are nested: `Hits:` appears once for the whole cache and
/// again indented under `Local storage:` for the local tier only. The first is the overall figure,
/// which is the one a person means by *is this cache working*.
fn keyed_after<'a>(body: &'a str, label: &str) -> Option<&'a str> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .map(str::trim)
}

/// The label of the `ccache -s` line carrying how full the cache is. The UNIT follows it and is
/// not part of it — the tool prints `Cache size (GB)` or `Cache size (GiB)` depending on its
/// configuration, so matching the whole label would read one host and not the other.
const CACHE_SIZE: &str = "Cache size";

/// The percentage in the parentheses at the end of a statistics line — `149641 / 187469 (79.82%)`.
///
/// From the RIGHT, and that is load-bearing rather than defensive: the occupancy line is
/// `Cache size (GB):    3.9 /   50.0 ( 7.88%)`, whose FIRST parenthesis opens the unit. A parse
/// from the left reads `GB):    3.9 /   50.0 ( 7.88%)` and answers nothing at all, and on the hit
/// line — which has one parenthesis and cannot tell the two apart — it silently agrees.
fn parenthesised_percent(line: &str) -> Option<Percent> {
    let (_, tail) = line.rsplit_once('(')?;
    Percent::parse(tail.trim_end_matches([')', '%', ' ']))
}

/// A program this module is allowed to run, and the arguments it may run it with.
///
/// # Why this is a type and not a string
///
/// The design's boundary is *detect and show the evidence; do not apply the prescription*, and a
/// [`Entry::remedy`] is a sentence written for a person. Nothing stops a later round from passing
/// one to a process spawner except that it cannot: this is the only thing in this module that
/// spawns, its constructor is private, and the whole set of them is the two constants below. A
/// remedy is a `&str` and there is no way to turn one into a `Probe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The arguments — each one a request for something the program already knows.
    args: &'static [&'static str],
}

impl Probe {
    /// `ccache -s` — what the cache has done.
    const STATISTICS: Self = Self { args: &["-s"] };

    /// `ccache -p` — what the cache is configured to do.
    const CONFIGURATION: Self = Self { args: &["-p"] };

    /// Run `program` with these arguments and take its standard output.
    ///
    /// `None` when the program is not there or would not run, which is the ordinary case on a host
    /// without it and never an error: a diagnosis that fails because a tool it was asking ABOUT is
    /// missing has confused its subject for its instrument.
    fn run(self, program: &Path) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(self.args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::{Counted, Waiting};

    /// A stand-in machine: a `/proc` and a cgroup hierarchy made of real directories.
    ///
    /// Every file this module reads is written here by hand, because the point of the
    /// [`Sources`] seam is that no test asserts whatever the developer's own box happens to be —
    /// a check driven against the real `/proc` passes everywhere and discriminates nowhere.
    struct FakeMachine {
        root: PathBuf,
    }

    impl FakeMachine {
        fn new(tag: &str) -> Self {
            // Two tests in one binary derive one name unless the name carries the test too — R338
            // lost a whole gate to exactly that, where the first to finish tore down the other's
            // fixture. The `::` in a test's thread name is stripped rather than kept: this
            // directory ends up INSIDE a fixture `PATH`, which is colon-separated, and a colon in
            // it split one entry into two and read as a bypassed cache.
            let owner: String = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            let root = std::env::temp_dir()
                .join(format!("sprag-doctor-{}-{owner}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let machine = Self { root };
            machine.write("proc/loadavg", "0.42 0.31 0.28 3/2295 1940976\n");
            machine.write("proc/stat", "cpu  1 2 3\ncpu0 1 2 3\ncpu1 1 2 3\nintr 9\n");
            machine.write("proc/sys/vm/swappiness", "60\n");
            machine.write(
                "proc/pressure/cpu",
                "some avg10=1.00 avg60=2.00 avg300=3.00 total=1\n\
                 full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
            );
            machine.write(
                "proc/pressure/io",
                "some avg10=1.00 avg60=1.00 avg300=1.00 total=1\n\
                 full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
            );
            machine.write(
                "proc/pressure/memory",
                "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
                 full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
            );
            machine
        }

        fn write(&self, relative: &str, body: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("fixture directory");
            std::fs::write(&path, body).expect("fixture file");
            path
        }

        /// One process, as `/proc` shows it.
        fn process(&self, pid: u32, cgroup: &str, swap_kib: u64, path: &str) {
            self.write(&format!("proc/{pid}/cgroup"), &format!("0::{cgroup}\n"));
            self.write(
                &format!("proc/{pid}/status"),
                &format!("Name:\tbash\nVmSwap:\t{swap_kib} kB\n"),
            );
            self.write(
                &format!("proc/{pid}/environ"),
                &format!("HOME=/h\0PATH={path}\0"),
            );
        }

        /// One process's `/proc/<pid>/exe` and the socket inodes its `/proc/<pid>/fd` holds.
        ///
        /// Symlinks and not files, because that is what the reader dereferences: a fixture that
        /// wrote the exe as a regular file would pass a `read_to_string` implementation and fail
        /// the `read_link` one, which is the reverse of what a fixture is for. `socket:[N]` targets
        /// point at nothing, exactly as the kernel's do.
        fn running(&self, pid: u32, exe: &str, socket_inodes: &[u64]) {
            let dir = self.root.join(format!("proc/{pid}"));
            std::fs::create_dir_all(dir.join("fd")).expect("fixture pid directory");
            std::os::unix::fs::symlink(exe, dir.join("exe")).expect("fixture exe link");
            for (fd, inode) in socket_inodes.iter().enumerate() {
                std::os::unix::fs::symlink(
                    format!("socket:[{inode}]"),
                    dir.join("fd").join(fd.to_string()),
                )
                .expect("fixture fd link");
            }
        }

        /// `/proc/net/unix`, header and all, from `(inode, state, path)` rows.
        ///
        /// The header is written because the real file has one and dropping it would leave the
        /// parser's first-line handling untested — the column layout is the only thing standing
        /// between this reader and a machine's whole socket table read one field out.
        fn sockets(&self, rows: &[(u64, &str, &str)]) {
            let mut table =
                String::from("Num       RefCount Protocol Flags    Type St Inode Path\n");
            for (inode, state, path) in rows {
                let flags = if *state == "01" {
                    "00010000"
                } else {
                    "00000000"
                };
                table.push_str(&format!(
                    "0000000000000000: 00000002 00000000 {flags} 0001 {state} {inode} {path}\n"
                ));
            }
            self.write("proc/net/unix", &table);
        }

        /// One cgroup, with the interface files the kernel would have made.
        fn cgroup(&self, relative: &str, weight: &str, usage_usec: u64, procs: &str) -> PathBuf {
            let at = format!("cgroup/{relative}");
            self.write(&format!("{at}/cpu.weight"), &format!("{weight}\n"));
            self.write(
                &format!("{at}/cpu.stat"),
                &format!("usage_usec {usage_usec}\nuser_usec 0\nsystem_usec 0\n"),
            );
            self.write(&format!("{at}/cgroup.procs"), procs);
            self.write(&format!("{at}/cgroup.controllers"), "cpu memory pids\n");
            self.write(&format!("{at}/cgroup.subtree_control"), "cpu memory pids\n");
            self.write(
                &format!("{at}/cpu.pressure"),
                "some avg10=0.00 avg60=4.00 avg300=0.00 total=0\n",
            );
            self.root.join(at)
        }

        fn sources(&self) -> Sources {
            Sources {
                proc: self.root.join("proc"),
                cgroup: Some(self.root.join("cgroup")),
                shims: self.root.join("shims"),
                // No probe: a test that ran the developer's real ccache would report their cache.
                ccache: None,
            }
        }
    }

    impl Drop for FakeMachine {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A pane the daemon placed successfully, for the fixtures that are not about admission.
    fn admitted(id: u64, pid: u32) -> PaneSite {
        PaneSite {
            id: PaneId(id),
            pid,
            landing: Landing::At(crate::PaneLineage {
                session: crate::SessionId(1),
                window: crate::WindowId(1),
                pane: PaneId(id),
            }),
        }
    }

    /// The subtree, three levels down, with a sibling at each level — the shape a delegated
    /// scope really has (`/user.slice/user-1000.slice/user@1000.service/app.slice/sprag.scope`).
    fn machine_with_a_subtree(tag: &str) -> (FakeMachine, Subject) {
        let machine = FakeMachine::new(tag);
        machine.cgroup("", "100", 0, "");
        machine.cgroup("user.slice", "100", 500_000, "");
        machine.cgroup("system.slice", "100", 500_000, "");
        machine.cgroup("user.slice/sprag.scope", "100", 100_000, "");
        machine.cgroup("user.slice/other.scope", "100", 100_000, "");
        machine.cgroup("user.slice/sprag.scope/pane-1", "100", 10_000, "111\n");
        machine.process(111, "/user.slice/sprag.scope/pane-1", 0, "/usr/bin:/bin");
        let subject = Subject {
            panes: vec![admitted(1, 111)],
            subtree: Some(machine.root.join("cgroup/user.slice/sprag.scope")),
            // Spelled rather than defaulted: this is the one field on a subject that names a
            // PROCESS, and a fixture that quietly inherited the test runner's own would count the
            // developer's real machine into a reading that is supposed to be replayable.
            daemon: None,
        };
        (machine, subject)
    }

    /// ⚠ THE DAEMON-SIDE SEAM: what the registry knows reaches the subject, refusal and all.
    ///
    /// Every other test in this module hands `Readings::capture_after` a `Subject` LITERAL, so the
    /// one line that builds a real one from live panes — `Subject::of` — was covered by nothing:
    /// replacing `pane.home()` there with a constant left this whole module, the verdict suite and
    /// the host's own doctor test GREEN, and the admission row would have read clean on every host
    /// forever. Measured, which is the only reason it is known.
    ///
    /// The refusal is `/dev/full` for `workspace`'s reason: it opens for writing, fails every
    /// write, on every Linux, with no cgroup tree and no privileges.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_subject_carries_what_the_kernel_answered_each_pane() {
        use std::sync::{Arc, Mutex};

        let root = std::env::temp_dir().join(format!("sprag-subject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            for (file, body) in [
                ("cgroup.procs", ""),
                ("cgroup.subtree_control", ""),
                ("cgroup.controllers", "cpu memory pids\n"),
                ("cpu.weight", "100\n"),
            ] {
                std::fs::write(path.join(file), body).expect("fixture file");
            }
        };
        cgroup("");

        let registry = Arc::new(Mutex::new(crate::SessionRegistry::new((80, 24))));
        let pool = {
            let reg = registry.lock().expect("registry");
            let name = reg.default_session().name().to_owned();
            reg.workspace_of(&name).expect("the default session's pool")
        };
        let home = pool.lock().expect("pool").home().expect("a window");
        let window = format!("session-{}/window-{}", home.session.0, home.window.0);
        cgroup(&format!("session-{}", home.session.0));
        cgroup(&window);
        // Pane 0 gets a leaf that TAKES it and pane 1 a leaf that refuses, so the two arms differ
        // inside one subject: a seam that answered the same thing for everything would pass a
        // fixture where every pane was refused just as happily as one where none was.
        cgroup(&format!("{window}/pane-0"));
        cgroup(&format!("{window}/pane-1"));
        std::fs::remove_file(root.join(format!("{window}/pane-1/cgroup.procs")))
            .expect("replace the leaf's procs file");
        std::os::unix::fs::symlink(
            "/dev/full",
            root.join(format!("{window}/pane-1/cgroup.procs")),
        )
        .expect("a leaf that refuses every write");

        pool.lock()
            .expect("pool")
            .set_pane_homes(Arc::new(crate::share::PaneHomes::over(
                crate::share::Tree::adopt(root.clone()).expect("adopt a plain directory"),
            )));
        for _ in 0..2 {
            pool.lock()
                .expect("pool")
                .spawn(
                    crate::command::default_shell_command().0,
                    "sh".into(),
                    40,
                    8,
                )
                .expect("a refused join must never cost the person their pane");
        }

        let subject = Subject::of(&registry);
        let landings: Vec<Landing> = subject.panes.iter().map(|site| site.landing).collect();
        assert!(
            matches!(landings.as_slice(), [Landing::At(_), Landing::Refused(_)]),
            "the subject carries each pane's own answer, not one answer for all of them: \
             {landings:?}",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The levels a walk reports are the interior cgroups between the subtree and the mount point,
    /// nearest first, and it STOPS at the mount rather than climbing out of the hierarchy.
    ///
    /// The stop is the claim: `Path::parent` does not know where a cgroup filesystem ends, so a
    /// walk without the bound reports `/tmp` and `/` as levels of the machine's CPU hierarchy.
    #[test]
    fn the_walk_stops_at_the_mount_point() {
        let mount = Path::new("/sys/fs/cgroup");
        assert_eq!(
            levels_above(mount, Path::new("/sys/fs/cgroup/a/b/c")),
            vec![
                PathBuf::from("/sys/fs/cgroup/a/b"),
                PathBuf::from("/sys/fs/cgroup/a"),
                PathBuf::from("/sys/fs/cgroup"),
            ],
        );
        assert_eq!(
            levels_above(mount, Path::new("/sys/fs/cgroup/a")),
            vec![PathBuf::from("/sys/fs/cgroup")],
            "a subtree one level down has exactly one level above it",
        );
    }

    /// A capture over a fixture reads every source, and the rate a level's child is credited with
    /// is the DELTA over the stated window, not the counter.
    ///
    /// Driven through the two-reading seam rather than through a sleep, so the sibling's burst is
    /// placed exactly between the two samples instead of being raced against a timer.
    #[test]
    fn a_capture_credits_a_sibling_with_what_it_took_over_the_window() {
        let (machine, subject) = machine_with_a_subtree("window");
        let sources = machine.sources();
        let before = baseline(&subject, &sources);
        assert!(
            before.contains_key(&machine.root.join("cgroup/system.slice")),
            "the baseline covers the siblings at every level: {before:?}",
        );
        // Two seconds of CPU inside a one-second window: two cores.
        machine.write(
            "cgroup/system.slice/cpu.stat",
            "usage_usec 2500000\nuser_usec 0\nsystem_usec 0\n",
        );
        let readings = Readings::capture_after(&subject, &sources, &before, Duration::from_secs(1));
        let subtree = readings.subtree.as_ref().expect("a subtree was read");
        let top = subtree
            .above
            .iter()
            .find(|level| level.at.ends_with("cgroup"))
            .expect("the mount point is a level");
        assert_eq!(
            top.ours, "user.slice",
            "the child the subtree descends through"
        );
        let rival = top.rival().expect("system.slice is a sibling");
        assert_eq!(rival.name, "system.slice");
        assert_eq!(
            rival.took,
            Cpu::Held {
                millicores: 2000,
                over_ms: 1000,
            },
            "2.0 s of CPU over a 1.0 s window is two cores",
        );
        assert_eq!(
            top.us().and_then(|us| us.weight),
            Some(100),
            "and the weight beside it comes from the file, not from the setting",
        );
    }

    /// Every non-cgroup source lands in the reading, and each absence stays an absence.
    #[test]
    fn a_capture_reads_the_machine_and_the_panes() {
        let machine = FakeMachine::new("sources");
        machine.cgroup("", "100", 0, "");
        machine.cgroup("scope", "100", 0, "");
        machine.cgroup("scope/pane-1", "100", 0, "111\n222\n");
        // The shim directory is the fixture's own, so the PATH the pane was started with has to
        // name THAT — the check compares against the directory it discovered, never a literal.
        let shims = machine.root.join("shims");
        let with_shims = format!("{}:/usr/bin", shims.display());
        machine.process(111, "/scope/pane-1", 100, &with_shims);
        machine.process(222, "/scope/pane-1", 28, &with_shims);
        machine.write("shims/gcc", "");
        machine.write("shims/g++", "");
        let subject = Subject {
            panes: vec![admitted(1, 111)],
            subtree: Some(machine.root.join("cgroup/scope")),
            daemon: None,
        };
        let sources = Sources {
            shims: machine.root.join("shims"),
            ..machine.sources()
        };
        let readings =
            Readings::capture_after(&subject, &sources, &BTreeMap::new(), Duration::from_secs(1));
        assert_eq!(readings.swappiness, Some(60));
        let load = readings.load.expect("loadavg parsed");
        assert_eq!(
            (load.runnable, load.threads, load.cores, load.pane_procs),
            (3, 2295, 2, 2),
            "runnable/total from loadavg, cores from /proc/stat's numbered rows, and the pane's \
             process count from ITS OWN CGROUP rather than from a /proc walk",
        );
        let pane = readings.panes.first().expect("one pane");
        assert_eq!(pane.cgroup.as_deref(), Some("/scope/pane-1"));
        assert_eq!(
            pane.swapped,
            Some(128 * 1024),
            "every process in the pane's cgroup, summed, in bytes",
        );
        assert_eq!(pane.ccache_on_path, Some(true));
        assert_eq!(
            pane.waiting.avg60(),
            Some(Percent::from_hundredths(400)),
            "the pane's own pressure, from the cgroup it is actually in",
        );
        assert_eq!(
            readings
                .ccache
                .and_then(|ccache| ccache.shims)
                .map(|(_, count)| count),
            Some(2),
        );
    }

    /// A pane whose `PATH` does not name the shim directory is not on it, and one whose environ is
    /// unreadable is UNREAD — which the check counts differently from clean.
    #[test]
    fn a_pane_with_no_environ_is_unread_rather_than_clean() {
        let machine = FakeMachine::new("environ");
        machine.cgroup("", "100", 0, "");
        machine.cgroup("scope", "100", 0, "");
        machine.process(111, "/scope", 0, "/usr/bin:/bin");
        machine.write("shims/cc", "");
        let subject = Subject {
            panes: vec![
                admitted(1, 111),
                // ⚠ AND THE SECOND ONE WAS REFUSED, so this fixture also proves the birth's
                // answer survives a capture. Every other field here is re-read from the fake
                // `/proc`; a `landing` dropped on the way through would be invisible to a test
                // that only ever built admitted panes, which is what every other fixture does.
                PaneSite {
                    id: PaneId(2),
                    pid: 999,
                    landing: Landing::Refused(crate::Refusal::from_errno(13)),
                },
            ],
            subtree: Some(machine.root.join("cgroup/scope")),
            daemon: None,
        };
        let sources = Sources {
            shims: machine.root.join("shims"),
            ..machine.sources()
        };
        let readings =
            Readings::capture_after(&subject, &sources, &BTreeMap::new(), Duration::from_secs(1));
        assert_eq!(readings.panes[0].ccache_on_path, Some(false));
        assert_eq!(
            readings.panes[1].ccache_on_path, None,
            "a pane with no readable environ makes no claim either way",
        );
        // ⚠ AND THE BIRTH'S ANSWER SURVIVES THE CAPTURE. Every other field on a `PaneReading` is
        // re-read from `/proc` here; this one is carried, because after the birth there is nothing
        // left to read it from — a refused pane's child and an unplaced pane's child are the same
        // process in the same cgroup. So a capture that dropped it would be invisible to every
        // other assertion in this module, and the whole admission row would silently read clean.
        // Measured: zeroing this field left all of `doctor::tests` GREEN until these two lines.
        assert_eq!(
            readings.panes[0].landing, subject.panes[0].landing,
            "an admitted pane arrives in the reading as one",
        );
        assert_eq!(
            readings.panes[1].landing,
            Landing::Refused(crate::Refusal::from_errno(13)),
            "and a refused one arrives carrying the kernel's own number, not an absence",
        );
    }

    /// A `PATH` entry is matched whole. `/usr/lib/ccache-old` is not `/usr/lib/ccache`, and a
    /// substring test would call a machine that bypasses the cache clean.
    #[test]
    fn a_path_entry_is_matched_whole() {
        assert!(lists_dir("/a:/usr/lib/ccache:/b", "/usr/lib/ccache"));
        assert!(!lists_dir("/a:/usr/lib/ccache-old:/b", "/usr/lib/ccache"));
        assert!(!lists_dir("/usr/lib/ccachex", "/usr/lib/ccache"));
    }

    /// `/proc/stat`'s numbered rows are the cores; the unnumbered `cpu` total is not one of them,
    /// and neither is any other row that happens to start with those three letters.
    #[test]
    fn the_core_count_comes_from_the_numbered_rows_only() {
        let machine = FakeMachine::new("cores");
        machine.write(
            "proc/stat",
            "cpu  0 0\ncpu0 0 0\ncpu1 0 0\ncpu2 0 0\ncpufreq 0\nctxt 1\n",
        );
        assert_eq!(cores(&machine.sources()), 3);
    }

    /// The two ccache probes, parsed from the bytes the shipped tool really printed.
    ///
    /// Captured from `ccache -s` and `ccache -p` on this machine (ccache 4.9.1) rather than
    /// written from the shape the parser expects, which is the only way a fixture can falsify one:
    /// both real forms defeated the first parse written for them. `-p` prints WHERE each setting
    /// came from ahead of the key, and `-s` puts each percentage at the END of its line behind two
    /// counts and nests a second `Hits:` under the local tier.
    #[test]
    fn the_ccache_output_is_read_for_the_four_facts_that_matter() {
        let statistics = "\
Cacheable calls:   187469 / 210800 (88.93%)
  Hits:            149641 / 187469 (79.82%)
    Direct:         17236 / 149641 (11.52%)
  Misses:           37828 / 187469 (20.18%)
Local storage:
  Cache size (GB):    3.9 /   50.0 ( 7.88%)
  Cleanups:           388
  Hits:             11789 /  19594 (60.17%)
";
        let configuration = "\
(/home/somebody/.config/ccache/ccache.conf) depend_mode = true
(/home/somebody/.config/ccache/ccache.conf) max_size = 50.0 GB
(default) compression = true
";
        assert_eq!(keyed(configuration, "max_size").as_deref(), Some("50.0 GB"));
        assert_eq!(keyed(configuration, "depend_mode").as_deref(), Some("true"));
        assert_eq!(
            keyed(configuration, "ccache.conf"),
            None,
            "the origin in front of the key is not a key, however much it looks like a token",
        );
        assert_eq!(
            keyed(configuration, "compress"),
            None,
            "a key is a whole token: `compress` is a prefix of `compression`",
        );
        assert_eq!(keyed_after(statistics, "Cleanups:"), Some("388"));
        assert_eq!(
            keyed_after(statistics, "Hits:").and_then(parenthesised_percent),
            Some(Percent::from_hundredths(7982)),
            "the OVERALL hit rate, from the end of the first `Hits:` line — not the local tier's \
             60.17% one line further down, and not the hit COUNT a left-to-right parse finds",
        );
        // The line that makes reading from the RIGHT load-bearing rather than defensive: its FIRST
        // parenthesis opens the UNIT, so a left-to-right parse answers nothing here while agreeing
        // on every other line in the file.
        assert_eq!(
            statistics
                .lines()
                .find(|line| line.trim_start().starts_with(CACHE_SIZE))
                .and_then(parenthesised_percent),
            Some(Percent::from_hundredths(788)),
            "how full the cache is, past the `(GB)` in its own label",
        );
    }

    /// The one path in this module that RUNS something, driven end to end.
    ///
    /// # Why this test exists at all
    ///
    /// Every other piece of the ccache reading is a pure parser with its own test, and that left
    /// the spawn itself — the `Probe` table, the argument list, the success check, the stdout
    /// decode — reachable only from a live host with ccache installed. A branch no test builds is
    /// one of the three shapes this project's debt sweep hunts for, and it found this one.
    ///
    /// The stand-in prints the bytes the real tool printed on this machine, so what is proved is
    /// that the two probes are RUN, that their output reaches the parsers, and that a
    /// non-zero exit is not read as an answer.
    ///
    /// # ⚠⚠⚠⚠⚠ Both stand-ins are TRACKED files, and that is a fix rather than a tidy-up
    ///
    /// This case used to WRITE them and the product then EXECUTED them, which is `ETXTBSY` waiting
    /// to happen: the LINUX kernel refuses to execute a file any process holds open for writing
    /// (macOS does not — `sprag_gate::doubles::exec_of_a_held_writer` is where the tree says so),
    /// and this harness runs its cases on THREADS of one process — so a sibling forking to spawn a program
    /// inherits this case's write handle and holds it until its own exec. `O_CLOEXEC` does not
    /// close that window, it ends it one exec too late. Register item 465 measured the same shape
    /// on `sprag-gate` at **10 failures in 30 runs, 0 in 30 after**; item 467 is the class, and
    /// this was two of its ten sites. A file nobody writes cannot be busy.
    ///
    /// ⚠ **The product is what execs here**, so an interpreter is not available as the remedy: it
    /// spawns whatever path `Sources::ccache` names, which is the behaviour under test. Tracking
    /// the file is the only fix that does not change the subject.
    #[cfg(unix)]
    #[test]
    fn the_two_ccache_probes_are_run_and_their_output_reaches_the_reading() {
        let doubles = sprag_gate::doubles::Doubles::of(env!("CARGO_MANIFEST_DIR")).set("doctor");
        let machine = FakeMachine::new("probe");
        machine.write("shims/cc", "");
        let fake = doubles.program("ccache");
        let sources = Sources {
            shims: machine.root.join("shims"),
            ccache: Some(fake.clone()),
            ..machine.sources()
        };
        let ccache = read_ccache(&sources).expect("ccache is installed here");
        assert_eq!(ccache.max_size.as_deref(), Some("50.0 GB"));
        assert_eq!(ccache.depend_mode, Some(true));
        assert_eq!(ccache.cleanups, Some(388));
        assert_eq!(ccache.hit_rate, Some(Percent::from_hundredths(7982)));
        assert_eq!(ccache.occupancy, Some(Percent::from_hundredths(788)));
        assert_eq!(ccache.shims.map(|(_, count)| count), Some(1));

        // A program that is not there is not an error: a diagnosis that failed because a tool it
        // was asking ABOUT is missing has confused its subject for its instrument. The shims are
        // still reported, because that half was read from the filesystem.
        let absent = Sources {
            ccache: Some(machine.root.join("bin/nosuchthing")),
            ..sources.clone()
        };
        let ccache = read_ccache(&absent).expect("the shims are still a reading");
        assert_eq!(ccache.cleanups, None);
        assert!(ccache.shims.is_some());

        // ⚠ AND A PROGRAM THAT RAN AND FAILED, which is a DIFFERENT case and the one the success
        // check exists for. An absent program fails at the spawn and never reaches it, so a
        // fixture with only the absent case cannot express the failure at all — the mutation that
        // takes a failed program's stdout anyway came back green against exactly that fixture.
        // Here the program runs, prints something that would parse, and exits non-zero.
        let liar = doubles.program("liar");
        let ccache = read_ccache(&Sources {
            ccache: Some(liar),
            ..sources
        })
        .expect("the shims are still a reading");
        assert_eq!(
            ccache.cleanups, None,
            "a program that exited non-zero told us nothing, whatever it printed",
        );
    }

    /// A counter is not a rate, and a reading with no baseline says so.
    #[test]
    fn a_sibling_with_no_baseline_has_no_rate() {
        let (machine, subject) = machine_with_a_subtree("nobaseline");
        let readings = Readings::capture_after(
            &subject,
            &machine.sources(),
            &BTreeMap::new(),
            Duration::from_secs(1),
        );
        let subtree = readings.subtree.expect("a subtree");
        assert!(
            subtree
                .above
                .iter()
                .flat_map(|level| &level.children)
                .all(|child| child.took == Cpu::Settling),
            "every child of every level, with no opening reading, is settling and not zero",
        );
    }

    /// The counters this module reads are the ones the neighbour reads, so a pane's own figures
    /// still come back through the type that carries an absent controller as a value.
    #[test]
    fn an_absent_counter_is_still_a_value() {
        assert_eq!(Counted::NoController, Counted::NoController);
        assert_eq!(Waiting::NotAccounted.avg60(), None);
    }

    // ── the daemons on this machine ─────────────────────────────────────────────────────────

    /// A subject naming a program, for the daemon fixtures.
    fn looking_for(program: &str) -> Subject {
        Subject {
            panes: Vec::new(),
            subtree: None,
            daemon: Some(DaemonSelf {
                pid: 100,
                program: program.to_owned(),
            }),
        }
    }

    /// ⚠ THE WHOLE READING, and the three ways a process can fail to be a daemon of this program.
    ///
    /// One fixture rather than four, because the discriminations only mean anything against each
    /// other: a reader that returned every process, or every process of this program, or every
    /// listening process, passes a test built on any ONE of them.
    #[test]
    fn a_daemon_is_this_program_listening_and_its_clients_are_counted_from_the_table() {
        let machine = FakeMachine::new("daemons");
        // Us: listening, two clients on it.
        machine.running(100, "/usr/bin/sprag-term", &[10, 11, 12]);
        // Another daemon of this program with nobody on it — the fault this check exists for.
        machine.running(200, "/home/dev/target/debug/sprag-term", &[20]);
        // A CLIENT of this program: same executable, no listening socket.
        machine.running(300, "/usr/bin/sprag-term", &[11]);
        // A daemon of some OTHER program, listening, abandoned. Not ours to report.
        machine.running(400, "/usr/bin/some-other-daemon", &[40]);
        machine.sockets(&[
            (10, "01", "/run/a.sock"),
            (11, "03", "/run/a.sock"),
            (12, "03", "/run/a.sock"),
            (20, "01", "/tmp/probe.sock"),
            (40, "01", "/run/other.sock"),
        ]);
        assert_eq!(
            SocketTable::read(&machine.sources())
                .expect("the fixture publishes a table")
                .clients_on("/run/other.sock"),
            0,
            "the other program's daemon is abandoned too — this check is bounded to OURS, and \
             that bound has to be a measurement rather than a sentence",
        );
        let daemons = read_daemons(&looking_for("sprag-term"), &machine.sources())
            .expect("a machine that publishes both tables is readable");
        assert_eq!(daemons.program, "sprag-term");
        assert_eq!(daemons.mine, 100);
        assert_eq!(
            daemons
                .found
                .iter()
                .map(|daemon| (
                    daemon.pid,
                    daemon.image.as_str(),
                    daemon.socket.as_str(),
                    daemon.attached
                ))
                .collect::<Vec<_>>(),
            vec![
                (100, "/usr/bin/sprag-term", "/run/a.sock", 2),
                (
                    200,
                    "/home/dev/target/debug/sprag-term",
                    "/tmp/probe.sock",
                    0
                ),
            ],
            "the client of this program, and the daemon of another, are not daemons of ours",
        );
    }

    /// ⚠⚠ A DAEMON WHOSE BINARY WAS REPLACED UNDER IT IS THE MOST INTERESTING ONE HERE, and the
    /// kernel spells it `… (deleted)`.
    ///
    /// Measured, not assumed: `/proc/<pid>/exe` keeps resolving after the file is unlinked and the
    /// suffix is part of the LINK TEXT. A reader that took the file name straight off it would look
    /// for a program called `sprag-term (deleted)`, match nothing, and report a clean machine — so
    /// the stalest daemon on the box would be the one guaranteed invisible.
    #[test]
    fn a_daemon_running_a_deleted_image_is_still_a_daemon_of_this_program() {
        let machine = FakeMachine::new("deleted");
        machine.running(100, "/usr/bin/sprag-term", &[10]);
        machine.running(200, "/home/dev/target/debug/sprag-term (deleted)", &[20]);
        machine.sockets(&[(10, "01", "/run/a.sock"), (20, "01", "/tmp/probe.sock")]);
        let daemons = read_daemons(&looking_for("sprag-term"), &machine.sources()).expect("read");
        assert_eq!(
            daemons
                .found
                .iter()
                .map(|daemon| daemon.image.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/usr/bin/sprag-term",
                "/home/dev/target/debug/sprag-term (deleted)",
            ],
            "matched despite the suffix, and the suffix KEPT in what the reader is shown",
        );
    }

    /// A socket path with a space in it is one path, not two.
    ///
    /// `/proc/net/unix` puts the path last precisely because it can contain anything a filename
    /// can. A reader that took field eight would truncate it, and would then count zero clients on
    /// a daemon that has them — a false clean, which is the direction that costs.
    #[test]
    fn a_socket_path_with_a_space_is_read_whole() {
        let machine = FakeMachine::new("spaced");
        machine.running(100, "/usr/bin/sprag-term", &[10, 11]);
        machine.sockets(&[
            (10, "01", "/run/two words.sock"),
            (11, "03", "/run/two words.sock"),
        ]);
        let daemons = read_daemons(&looking_for("sprag-term"), &machine.sources()).expect("read");
        assert_eq!(
            daemons
                .found
                .iter()
                .map(|daemon| (daemon.socket.as_str(), daemon.attached))
                .collect::<Vec<_>>(),
            vec![("/run/two words.sock", 1)],
        );
    }

    /// ⚠⚠ A DAEMON THAT HAS STOPPED ACCEPTING IS THE ONE PEOPLE ARE FAILING TO REACH, and its
    /// clients are all in the state a two-state reader would drop.
    ///
    /// Its socket carries only `02` rows — connections the kernel has queued on a door nobody is
    /// taking them off. Counting those as nobody would tell a person to go and kill the daemon
    /// their own terminal is blocked on, which is the worst answer this check could give.
    #[test]
    fn clients_queued_on_a_door_nobody_is_accepting_are_still_clients() {
        let machine = FakeMachine::new("wedged");
        machine.running(100, "/usr/bin/sprag-term", &[10]);
        machine.running(200, "/usr/bin/sprag-term", &[20]);
        machine.sockets(&[
            (10, "01", "/run/a.sock"),
            (20, "01", "/run/wedged.sock"),
            // Inode 0 twice, as the kernel really writes it — two clients, not one row seen twice.
            (0, "02", "/run/wedged.sock"),
            (0, "02", "/run/wedged.sock"),
        ]);
        let daemons = read_daemons(&looking_for("sprag-term"), &machine.sources()).expect("read");
        assert_eq!(
            daemons
                .found
                .iter()
                .map(|daemon| (daemon.pid, daemon.attached))
                .collect::<Vec<_>>(),
            vec![(100, 0), (200, 2)],
            "the wedged daemon has two people on it and the idle one has nobody",
        );
    }

    /// Two absences, and both make the check BLIND rather than clean.
    ///
    /// A machine with no socket table cannot be asked, and a process that cannot name its own
    /// program has nothing to recognise a peer against. Either one reported as *no daemons found*
    /// would be a clean verdict about a question nobody asked.
    #[test]
    fn a_machine_that_cannot_be_asked_answers_nothing_rather_than_none() {
        let machine = FakeMachine::new("blind");
        machine.running(100, "/usr/bin/sprag-term", &[10]);
        machine.sockets(&[(10, "01", "/run/a.sock")]);
        assert_eq!(
            read_daemons(
                &Subject {
                    panes: Vec::new(),
                    subtree: None,
                    daemon: None,
                },
                &machine.sources(),
            ),
            None,
            "a process that cannot name itself recognises no peer",
        );
        let nowhere = Sources {
            proc: machine.root.join("proc-that-is-not-there"),
            ..machine.sources()
        };
        assert_eq!(
            read_daemons(&looking_for("sprag-term"), &nowhere),
            None,
            "and neither does a host with no socket table",
        );
    }

    /// The header line, and every row this reader does not understand, are skipped rather than
    /// parsed into a zero.
    #[test]
    fn only_the_three_states_this_check_names_are_rows() {
        assert!(
            SocketRow::parse("Num       RefCount Protocol Flags    Type St Inode Path").is_none()
        );
        assert!(
            SocketRow::parse("0000000000000000: 00000002 00000000 00000000 0001 07 7 /a.sock")
                .is_none(),
            "state 07 is a socket on its way out, which is neither a door nor a client on one",
        );
        let door =
            SocketRow::parse("0000000000000000: 00000002 00000000 00010000 0001 01 7 /a.sock")
                .expect("a listening row");
        assert_eq!(
            (door.inode, door.state, door.path.as_str()),
            (7, SocketState::Listening, "/a.sock"),
        );
        // ⚠ Inode ZERO, and that is the kernel's real answer for a connection nobody has accepted
        // yet — a parser that treated a zero inode as a failure would drop exactly the clients of
        // the daemon that has stopped accepting them.
        let arriving =
            SocketRow::parse("0000000000000000: 00000002 00000000 00000000 0001 02 0 /a.sock")
                .expect("an arriving row");
        assert_eq!(
            (arriving.inode, arriving.state, arriving.path.as_str()),
            (0, SocketState::Arriving, "/a.sock"),
        );
        assert!(arriving.state.is_a_client() && !door.state.is_a_client());
    }

    /// ⚠⚠⚠⚠⚠ THE REAL KERNEL, because every fixture above is a file this module also WROTE.
    ///
    /// The fixtures prove the arithmetic and prove nothing about the layout: `/proc/net/unix`'s
    /// column order, the `socket:[N]` spelling of an fd, and the path `net/unix` itself are all
    /// facts about Linux that a fake `/proc` written by this same module cannot disagree with. A
    /// reader that looked for `net/unix.txt` would pass all six and report a clean machine forever.
    ///
    /// So this one binds a REAL listening socket, finds ITSELF through the kernel's own tables, and
    /// then watches the attached count move when a real client connects — which is the exact number
    /// the check's verdict turns on, measured end to end with nothing stubbed.
    ///
    /// ⚠⚠⚠⚠⚠ It has already earned its place once: it FAILED on its first run against a reader
    /// that counted only accepted connections, which is how the `02` state in [`SocketState`] came
    /// to be known at all. Every fixture in this module passed that same reader.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_kernel_shows_this_process_listening_and_counts_a_real_client() {
        use std::os::unix::net::UnixStream;

        // Both halves through `sprag_scratch`, and both because a gate in this workspace says so:
        // the NAME because a per-run path built on the root alone is one nothing sweeps when the
        // run that made it is killed (item 795), and the BIND because macOS gives 104 bytes of
        // `sun_path` against a 48-byte scratch root, so a name that binds here is refused there
        // (item 957). `scratch_for` mints the pid where the reaper reads it and sweeps this
        // prefix's dead owners in the same call.
        let at = sprag_scratch::scratch_for("sprag-doctor", "sock");
        let _ = std::fs::remove_file(&at);
        let door = sprag_scratch::bind_socket(&at).expect("a real listening socket");
        // Our own program, whatever cargo named this test binary — the check recognises peers by
        // the running program and never by a constant, so the fixture must not supply one either.
        let me = DaemonSelf::here().expect("this process can name itself");
        let subject = Subject {
            panes: Vec::new(),
            subtree: None,
            daemon: Some(me.clone()),
        };
        let ours = |readings: Option<Daemons>| {
            readings
                .expect("this host publishes a process table")
                .found
                .into_iter()
                .find(|daemon| daemon.pid == me.pid)
                .expect("this process is listening, so it is a daemon of its own program")
        };

        let alone = ours(read_daemons(&subject, &Sources::default()));
        assert_eq!(
            alone.socket,
            at.to_str().expect("a utf-8 fixture path"),
            "the door found through /proc is the one this test opened",
        );
        assert_eq!(alone.attached, 0, "nobody has connected to it yet");
        assert!(
            alone.image.ends_with(&me.program),
            "the image the kernel resolves ends in the program it was recognised by: {}",
            alone.image,
        );

        // ⚠ NOT ACCEPTED YET. This is the state the first draft of this reader could not see, and
        // the run that found it is the only reason the `02` arm exists.
        let client = UnixStream::connect(&at).expect("a real client");
        let arrived = ours(read_daemons(&subject, &Sources::default()));
        assert_eq!(
            arrived.attached, 1,
            "a client the daemon has not taken off the door is still a client on it",
        );

        let taken = door.accept().expect("the door accepts").0;
        let accepted = ours(read_daemons(&subject, &Sources::default()));
        assert_eq!(
            accepted.attached, 1,
            "and it is the SAME client after accept(), not a second one — the kernel moves the row \
             from 02 to 03 and gives it an inode, which a reader counting both states must not \
             double",
        );

        drop(taken);
        drop(client);
        drop(door);
        std::fs::remove_file(&at).expect("the fixture socket is removed");
    }

    /// An abstract socket, and a socket bound to no path at all, are both read without a panic —
    /// and the pathless one is never mistaken for a daemon's door.
    #[test]
    fn a_socket_with_no_path_is_not_a_door() {
        let machine = FakeMachine::new("pathless");
        machine.running(100, "/usr/bin/sprag-term", &[10, 11]);
        machine.sockets(&[(10, "01", ""), (11, "01", "@abstract")]);
        let daemons = read_daemons(&looking_for("sprag-term"), &machine.sources()).expect("read");
        assert_eq!(
            daemons
                .found
                .iter()
                .map(|daemon| daemon.socket.as_str())
                .collect::<Vec<_>>(),
            vec!["@abstract"],
            "the unnamed listening socket is skipped and the abstract one is the door",
        );
    }
}
