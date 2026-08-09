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
        Self { panes, subtree }
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
        }
    }
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

    /// The subtree, three levels down, with a sibling at each level — the shape a delegated
    /// scope really has (`/user.slice/user-1000.slice/user@1000.service/app.slice/sprag.scope`).
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
(/home/coin/.config/ccache/ccache.conf) depend_mode = true
(/home/coin/.config/ccache/ccache.conf) max_size = 50.0 GB
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
    #[test]
    fn the_two_ccache_probes_are_run_and_their_output_reaches_the_reading() {
        let machine = FakeMachine::new("probe");
        machine.write("shims/cc", "");
        let fake = machine.write(
            "bin/ccache",
            "#!/bin/sh\n\
             case \"$1\" in\n\
             -s) echo 'Cacheable calls:   187469 / 210800 (88.93%)'\n\
                 echo '  Hits:            149641 / 187469 (79.82%)'\n\
                 echo 'Local storage:'\n\
                 echo '  Cache size (GB):    3.9 /   50.0 ( 7.88%)'\n\
                 echo '  Cleanups:           388' ;;\n\
             -p) echo '(/home/x/.config/ccache/ccache.conf) depend_mode = true'\n\
                 echo '(/home/x/.config/ccache/ccache.conf) max_size = 50.0 GB' ;;\n\
             *) exit 3 ;;\n\
             esac\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                .expect("fixture ccache is executable");
        }
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
        let liar = machine.write(
            "bin/liar",
            "#!/bin/sh\necho '  Cleanups:           999'\nexit 1\n",
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&liar, std::fs::Permissions::from_mode(0o755))
                .expect("fixture liar is executable");
        }
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
}
