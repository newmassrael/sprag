//! What a pane is granted of the machine, and whether this host can ENFORCE that grant.
//!
//! A pane's share of the machine belongs to the person who opened it, not to whichever child
//! happened to spawn the most runnable threads. Without a grant that is enforced, one pane running
//! `make -j32` takes 32 times what its neighbour running one process takes, and nothing in this
//! product could say otherwise: measured on this machine at 4 threads against 1, the split was
//! **80:20**.
//!
//! # Why a grant and its enforcement are two types
//!
//! Every pane has a [`Share`] — that is a fact of the product, true on macOS, true in a container,
//! true on a kernel with no cgroups at all. Whether the host can make the kernel honour it is a
//! separate fact about the machine, and conflating the two is what produces the silent skip: a
//! daemon that cannot enforce quietly behaving as though it had. So the grant is unconditional and
//! [`Enforcement`] is probed, carries its reason as a VALUE, and is never an `Option` or a `bool` —
//! the shape R335 tore out of [`crate::pane_pty`]'s neighbourhood for the same reason, where one
//! `None` had come to mean two different things.
//!
//! # Why the probe reads OUR OWN cgroup
//!
//! `cgroup.controllers` in a cgroup lists what its PARENT enabled for it, which is exactly the set
//! this process may enable for children it creates. That makes it the honest answer to the only
//! question worth asking — *can I weight the children I am about to make?* — and it discriminates
//! on real machines rather than in principle. Measured here, same host, same moment:
//!
//! ```text
//! app-ghostty-surface-transient-7240.scope   memory pids       <- cannot weight children
//! agents.slice/agent-claude-...scope         cpu memory pids   <- can
//! ```
//!
//! A terminal multiplexer launched from a desktop terminal typically lands in the first one. That
//! is not a hypothetical: it is where this daemon's own panes were living when this was written,
//! and it is why a patch that merely creates a cgroup per pane changes nothing. The controller has
//! to be available before a weight means anything, and the same probe answers again — correctly —
//! after the daemon has been given a subtree of its own.
//!
//! What the probe does NOT prove is that a weight has been applied to anything. It answers whether
//! placement COULD be enforced, which is the question that has to be answered first; [`Tree`] is
//! what then makes the cgroups and [`PaneHomes`] is what puts panes in them.
//!
//! # Where a pane's cgroup is decided
//!
//! In [`PaneHomes`], which a [`Workspace`](crate::Workspace) holds — never at a call site. R336
//! placed panes from one of the five doors a pane arrives through and the other four placed
//! nothing, with the gate written for the first green throughout. A pool is what every door goes
//! through, so the pool is what carries this.
//!
//! The tree is a PROJECTION of the identity tree, and a projection has to track its source: a pane
//! that moves between windows takes its processes with it
//! ([`PaneHomes::relocate`](PaneHomes::relocate)), because otherwise the window a person pulled a
//! runaway build OUT of goes on being charged for it.
//!
//! # Measured against ghostty at `2602886`, honest trade first
//!
//! ghostty has a real cgroup layer and R336's note that it "still has the post-fork race" was
//! WRONG — re-read at source, `src/apprt/gtk/pre_exec.zig` makes the CHILD wait up to 250 ms for
//! the parent's D-Bus move to land, with `linux-cgroup-hard-fail` to refuse the exec if it never
//! does. That refusal is a knob sprag does not have.
//!
//! | | ghostty | sprag |
//! |---|---|---|
//! | ceilings | `memory.high`, `pids.max` | the same pair |
//! | CPU weight | none (`cpu.weight` appears nowhere) | `cpu.weight` per leaf |
//! | shape | one flat scope per surface | `session/window/pane`, weighted per LEVEL |
//! | D-Bus calls | one per surface | one per DAEMON, then plain `mkdir` |
//! | the fork/exec race | child POLLS, ≤250 ms, may time out | closed by construction |
//! | refuse the exec if placement failed | `linux-cgroup-hard-fail` | **no such knob** |
//! | what a person can SEE of it | nothing | [`Charge`], on all three mouths (R338) |
//!
//! The honest trade first: ghostty's hard-fail knob is a real thing sprag does not have, and
//! herdr — which has no cgroup layer at all — supports Windows, which sprag cannot compile on.
//!
//! The race is the difference that is structural rather than a feature gap: ghostty asks systemd
//! for a scope the child must then wait to be moved into, so there is a window and a timeout.
//! sprag holds a DELEGATED subtree, so the pane's cgroup can be made *before the child exists* and
//! the child joins itself with one write — nothing to wait for and no way to time out.
//!
//! # The measurement half, and why neither rival has it (checked at source, R338)
//!
//! A grant that cannot be read back is half a feature: the person who set it has no way to learn
//! whether it did anything, and the pane being starved has no way to say so.
//!
//! * **ghostty** (`2602886`): `src/os/cgroup.zig` is **27 lines with one function**,
//!   `current(buf, pid)`, which reads `/proc/<pid>/cgroup`. It WRITES `MemoryHigh`
//!   (`src/apprt/gtk/cgroup.zig:57`) and a process cap, and reads back **nothing** — `cpu.stat`,
//!   `cpu.pressure`, `memory.current` and `pids.current` appear nowhere in its source.
//! * **herdr** (`9a4ce5e1`): no cgroup layer at all — no file under `src/` mentions `cgroup`,
//!   `cpu.stat`, `CPUWeight` or `systemd-run` — so every agent it runs shares one cgroup, which is
//!   the defect its own report describes. Driving the shipped binary, none of its **twelve**
//!   subcommand help texts names a resource, and its API schema declares **no** resource method.
//!
//! Both were measured by RUNNING the rival or reading the file cited, never by assuming.

use std::path::{Path, PathBuf};

use crate::registry::{SessionId, WindowId};
use crate::workspace::PaneId;

/// Where the unified cgroup hierarchy is mounted on every Linux this runs on.
///
/// Not configurable: cgroup v2's mount point is fixed by systemd and by the kernel documentation
/// alike, and a host that has moved it is a host whose probe should say it found nothing rather
/// than one this crate should hunt for.
#[cfg(target_os = "linux")]
const UNIFIED_ROOT: &str = "/sys/fs/cgroup";

/// The CPU controller's name in `cgroup.controllers` and `cgroup.subtree_control`.
#[cfg(target_os = "linux")]
const CPU_CONTROLLER: &str = "cpu";

/// The share of its level a pane is granted — the `cpu.weight` a placed pane would carry.
///
/// A weight, not a cap. Two facts follow that a cap would not give, and both are wanted: a pane
/// granted a small share still takes the WHOLE machine when nothing else wants it (measured: a
/// cgroup weighted 10 against an idle sibling took all 8 cores it was offered), and the grants of
/// live panes always sum to the machine rather than to some budget a person has to keep balanced.
///
/// The range is the kernel's, 1..=10000, and it is checked in the constructor so that an
/// out-of-range weight cannot be built and then fail at the write — the failure would surface at
/// pane birth, which is the worst place to learn that a setting was invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Share(u32);

impl Share {
    /// The smallest grant the kernel accepts.
    pub const MIN: u32 = 1;
    /// The largest grant the kernel accepts.
    pub const MAX: u32 = 10_000;

    /// What every pane gets unless a person says otherwise — the kernel's own default, so an
    /// unplaced pane and a placed-but-unadjusted one are weighted alike.
    ///
    /// Equal grants at every level are the whole policy: because cgroup v2 distributes weight PER
    /// LEVEL, "everyone even" already means two sessions split the machine evenly regardless of how
    /// many panes each holds. There is no tuning to do for the default case, which is the case.
    pub const EVEN: Self = Self(100);

    /// Build a grant, or say why the number is not one.
    ///
    /// # Errors
    ///
    /// Returns [`ShareError::OutOfRange`] outside 1..=10000.
    pub fn new(weight: u32) -> Result<Self, ShareError> {
        if (Self::MIN..=Self::MAX).contains(&weight) {
            Ok(Self(weight))
        } else {
            Err(ShareError::OutOfRange { weight })
        }
    }

    /// The weight, as the kernel spells it.
    #[must_use]
    pub const fn weight(self) -> u32 {
        self.0
    }
}

impl Default for Share {
    fn default() -> Self {
        Self::EVEN
    }
}

impl std::fmt::Display for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a number could not become a [`Share`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareError {
    /// Outside the kernel's 1..=10000.
    OutOfRange {
        /// What was asked for.
        weight: u32,
    },
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfRange { weight } => write!(
                f,
                "share {weight} is outside {}..={}",
                Share::MIN,
                Share::MAX
            ),
        }
    }
}

impl std::error::Error for ShareError {}

/// Whether this host can make the kernel honour a [`Share`], and if not, why not.
///
/// Probed rather than assumed, and carrying its reason rather than collapsing to absence, because
/// every caller above this one has to be able to TELL A PERSON why their setting is not taking
/// effect. A daemon that refuses states why (R325); a daemon that silently does not enforce is
/// worse than one that cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enforcement {
    /// Shares can be enforced for children created under `home`.
    Available {
        /// This process's own cgroup — the directory whose `cgroup.subtree_control` governs what
        /// its children may be weighted by, and therefore where a delegated subtree would be built.
        home: PathBuf,
    },
    /// Shares cannot be enforced here.
    Unenforceable {
        /// What stands in the way, in terms a person can act on.
        reason: Unenforceable,
    },
}

impl Enforcement {
    /// Ask this machine, now.
    ///
    /// Reads two files and decides; it changes nothing. Safe to call at startup on any platform —
    /// off Linux it answers [`Unenforceable::NotLinux`] without touching a filesystem.
    #[must_use]
    pub fn probe() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::probe_under(
                Path::new(UNIFIED_ROOT),
                &std::fs::read_to_string("/proc/self/cgroup"),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::Unenforceable {
                reason: Unenforceable::NotLinux,
            }
        }
    }

    /// The whole decision, with its two inputs handed in — the seam the tests drive.
    ///
    /// Split out so the decision is exercised against fixtures rather than against the developer's
    /// own machine: a test that reads the real `/sys/fs/cgroup` asserts whatever that host happens
    /// to be, which passes everywhere and discriminates nowhere.
    #[cfg(target_os = "linux")]
    fn probe_under(root: &Path, self_cgroup: &std::io::Result<String>) -> Self {
        let Ok(contents) = self_cgroup else {
            return Self::Unenforceable {
                reason: Unenforceable::NoUnifiedHierarchy,
            };
        };
        let Some(path) = unified_path(contents) else {
            return Self::Unenforceable {
                reason: Unenforceable::NoUnifiedHierarchy,
            };
        };
        // `path` is absolute in the hierarchy's own terms ("/user.slice/..."), so it is joined by
        // trimming rather than by `Path::join`, which would discard `root` entirely for an absolute
        // argument and silently probe the host root instead of the mount point.
        let home = root.join(path.trim_start_matches('/'));
        let Ok(controllers) = std::fs::read_to_string(home.join("cgroup.controllers")) else {
            return Self::Unenforceable {
                reason: Unenforceable::NoUnifiedHierarchy,
            };
        };
        if lists_controller(&controllers, CPU_CONTROLLER) {
            Self::Available { home }
        } else {
            Self::Unenforceable {
                reason: Unenforceable::CpuControllerUnavailable,
            }
        }
    }
}

/// What stands between this host and an enforced share.
///
/// Each arm is something a person or an operator can act on, which is the bar for being a separate
/// arm at all: an arm nobody could respond to differently belongs merged with its neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unenforceable {
    /// Not Linux. There is no cgroup equivalent to reach for, and saying so is the answer.
    NotLinux,
    /// No cgroup v2 hierarchy this process can read itself into — v1-only, an unusual mount, or a
    /// container that hid it.
    NoUnifiedHierarchy,
    /// The hierarchy is there, but this process's own cgroup was not given the CPU controller, so
    /// nothing it creates below itself can be weighted.
    ///
    /// The common cause is a desktop terminal's transient scope: systemd enables `memory` and
    /// `pids` for an app's children and stops there. Delegating a subtree with `cpu` to this daemon
    /// is what answers it.
    CpuControllerUnavailable,
}

impl std::fmt::Display for Unenforceable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLinux => f.write_str("this platform has no cgroups"),
            Self::NoUnifiedHierarchy => f.write_str("no readable cgroup v2 hierarchy"),
            Self::CpuControllerUnavailable => {
                f.write_str("this process's cgroup was not given the cpu controller")
            }
        }
    }
}

/// The leaf the daemon's own processes are moved into so the delegated root can stay an interior
/// node.
///
/// Not a `session-` name, so it can never collide with one.
const DAEMON_LEAF: &str = "daemon";

/// The file that decides which controllers a cgroup's CHILDREN get.
const SUBTREE_CONTROL: &str = "cgroup.subtree_control";

/// The file that lists — and, written to, moves — a cgroup's member processes.
const PROCS: &str = "cgroup.procs";

/// A cgroup's share of its level.
const CPU_WEIGHT: &str = "cpu.weight";

/// The CPU time a cgroup and everything under it has consumed, cumulative.
///
/// Present in EVERY cgroup v2 directory, controller or no controller: it is part of the base stat
/// the kernel keeps for the hierarchy itself. So a pane whose host could not give it the `cpu`
/// controller — no weight, no ceiling — is still measurable, which is the case worth having.
const CPU_STAT: &str = "cpu.stat";

/// The key inside [`CPU_STAT`] that carries the cumulative CPU time, in microseconds.
const USAGE_USEC: &str = "usage_usec";

/// How long the tasks in a cgroup were RUNNABLE and not running — the kernel's pressure stall
/// information, per cgroup.
///
/// The other half of the only question worth asking about a pane's CPU. [`CPU_STAT`] says what a
/// pane GOT, and by itself that number cannot be read: a pane holding 0.1 cores is either a pane
/// with nothing to do or a pane being starved, and those want opposite responses from a person. This
/// file is what separates them.
const CPU_PRESSURE: &str = "cpu.pressure";

/// The row of a pressure file that carries the time when SOME task was stalled.
///
/// The row a PANE is read through ([`Tree::charge`]): a pane is normally one job, so `some` is the
/// reading that moves when that job waits, and [`PRESSURE_FULL`] on a cgroup running one thread
/// would say the same thing less often.
const PRESSURE_SOME: &str = "some";

/// The row of a pressure file that carries the time when EVERY task was stalled — nothing in this
/// scope made progress at all.
///
/// The row a MACHINE is judged on, and the reason [`Pressure`] carries both. At `/proc/pressure/io`
/// a non-zero `full` is not a slow disk, it is every runnable task on the box parked waiting for
/// one; the same number under a pane would only say the pane is one job wide.
const PRESSURE_FULL: &str = "full";

/// A cgroup's current memory footprint, in bytes — the memory controller's counter.
const MEMORY_CURRENT: &str = "memory.current";

/// How many processes a cgroup holds right now — the pids controller's counter, and the number a
/// [`Limits`] process ceiling is measured against.
const PIDS_CURRENT: &str = "pids.current";

/// A cgroup's ceiling on live processes.
const PIDS_MAX: &str = "pids.max";

/// The memory level above which a cgroup is throttled and reclaimed from.
///
/// `memory.high` and NOT `memory.max`, and the difference is what happens to the person's work:
/// `max` invokes the OOM killer, so a pane that touches the ceiling loses whatever it was doing;
/// `high` puts the cgroup under reclaim pressure and throttles it, so a build that overshoots gets
/// slow instead of dead. A ceiling a person set to protect their other panes should not be a way to
/// lose the pane they set it on.
const MEMORY_HIGH: &str = "memory.high";

/// What the kernel reads and writes for "no ceiling here" in both limit files.
const UNCAPPED: &str = "max";

/// What every interior level of the tree turns on for its children, IF the level has it to give.
///
/// `cpu` is the point. `pids` bounds one pane's fork storm from taking the pid budget the person's
/// other panes need. `memory` joined them when [`Limits`] gave a person a number to set — before
/// that it was deliberately absent, because a controller enabled but unread buys per-cgroup
/// accounting for no answer.
///
/// A WISH LIST and not a command, because it is written as one write and the kernel takes that write
/// **all or nothing**. Measured on a real delegated scope: `+cpu +nosuchctrl +pids` fails entirely
/// and leaves `cgroup.subtree_control` EMPTY — not `cpu pids`, nothing. So a host whose delegation
/// offers `cpu pids` and no `memory` would have every level fail to enable and every pane lose the
/// share it used to get. That host is not hypothetical: systemd delegates whatever the parent slice
/// enabled, and R336 measured a scope on this very machine listing `memory pids` with no `cpu`.
/// [`available_controllers`] narrows this to what a level actually has.
const WANTED_CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];

/// The file listing what a cgroup's PARENT enabled for it — which is exactly the set this cgroup may
/// enable for its own children.
const CONTROLLERS: &str = "cgroup.controllers";

/// The three identities that already name a pane, which are also where its cgroup goes.
///
/// The resource tree is a PROJECTION of the identity tree, so this type invents nothing: the ids
/// are the ones the registry mints and the wire already carries. Because cgroup v2 distributes
/// weight per LEVEL, mirroring the identities is what makes two sessions split the machine evenly
/// regardless of how many panes each holds — the policy is the shape, and there is nothing to tune.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneLineage {
    /// The session the pane's window belongs to.
    pub session: SessionId,
    /// The window the pane belongs to.
    pub window: WindowId,
    /// The pane.
    pub pane: PaneId,
}

/// The identities a pane POOL descends from — the window it is, and that window's session.
///
/// [`PaneLineage`] is this plus a pane, and [`pane`](Self::pane) is the only way to build one, which
/// is the point: a pool knows its own two ids at the moment a window is made and learns the third
/// only when it mints it. Keeping the pair as a value the pool holds is what lets EVERY birth path
/// place a pane without being told where — the asymmetry R336 shipped, where one door of four
/// carried the lineage in its arguments and the other three had nothing to carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PoolLineage {
    /// The session the pool's window belongs to.
    pub session: SessionId,
    /// The window the pool IS.
    pub window: WindowId,
}

impl PoolLineage {
    /// The full lineage of `pane`, born into or moved into this pool.
    #[must_use]
    pub const fn pane(self, pane: PaneId) -> PaneLineage {
        PaneLineage {
            session: self.session,
            window: self.window,
            pane,
        }
    }
}

impl PaneLineage {
    /// This pane's cgroup, relative to the tree root — `session-<n>/window-<n>/pane-<n>`.
    #[must_use]
    pub fn relative(self) -> PathBuf {
        let mut path = PathBuf::from(format!("session-{}", self.session.0));
        path.push(format!("window-{}", self.window.0));
        path.push(format!("pane-{}", self.pane.0));
        path
    }

    /// The interior levels, outermost first — the ones that must exist and have their controllers
    /// enabled before the leaf can be weighted.
    fn interiors(self) -> [String; 2] {
        [
            format!("session-{}", self.session.0),
            format!("window-{}", self.window.0),
        ]
    }
}

/// The delegated cgroup subtree this daemon owns and builds panes into.
///
/// # Why adopting a root is not just remembering a path
///
/// A delegated scope arrives holding the daemon's own processes, and cgroup v2 forbids a cgroup
/// from both holding processes and enabling controllers for its children. Enable them anyway and
/// the kernel does not refuse the write — it lets it through and every child is then born
/// `cgroup.type = domain invalid`, where enabling anything fails with **`ENOTSUP`**. That reads as
/// "the kernel does not support this" and means "you left a process in an interior node", which is
/// why [`adopt`](Self::adopt) moves the daemon into a leaf of its own BEFORE it enables anything,
/// and why failing to make that leaf aborts before a single controller is turned on.
///
/// Measured on a real delegated scope rather than reasoned about; the sequence is in
/// `claudedocs/DESIGN-R336-PANE-SHARE.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    /// The delegated root — every path this type touches is under it.
    root: PathBuf,
}

impl Tree {
    /// Take `root` as this daemon's subtree: vacate it, then turn on what its children need.
    ///
    /// `root` is a cgroup this process may write, which in practice is one systemd delegated after
    /// [`Enforcement::probe`] said a share could be enforced at all. Idempotent: adopting a root
    /// that is already vacated and already enabled does the same thing again and succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the daemon leaf cannot be made, if a process cannot be moved into
    /// it, or if the root will not take its controllers. In every one of those cases the root is
    /// left as it was found rather than half-enabled.
    pub fn adopt(root: PathBuf) -> Result<Self, TreeError> {
        let leaf = root.join(DAEMON_LEAF);
        make_cgroup(&leaf)?;
        // Every process, not just this one: a delegated root holds only what was put there for this
        // daemon, and one straggler left behind invalidates the whole tree beneath it.
        for pid in read_procs(&root)? {
            move_proc(&leaf, pid)?;
        }
        // ONLY now. Above this line the root may still hold a process; below it, it does not.
        enable_controllers(&root)?;
        Ok(Self { root })
    }

    /// The root this tree was adopted at.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Make the cgroup a pane belongs in, creating and enabling the levels above it as needed.
    ///
    /// Writing `cpu.weight` is also the check that the tree is really enforcing: the file exists
    /// only where the CPU controller reached, so a level that was never enabled fails HERE, loudly,
    /// instead of yielding a pane that looks placed and is weighted by nothing.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if a level cannot be created, cannot take its controllers, or if the
    /// leaf will not take its weight.
    pub fn place(&self, at: PaneLineage, share: Share) -> Result<Placement, TreeError> {
        let mut path = self.root.clone();
        for level in at.interiors() {
            path.push(level);
            make_cgroup(&path)?;
            enable_controllers(&path)?;
        }
        path.push(format!("pane-{}", at.pane.0));
        make_cgroup(&path)?;
        write_control(&path.join(CPU_WEIGHT), &share.weight().to_string())?;
        Ok(Placement { path, share })
    }

    /// What the kernel has charged the pane at `at` — one read of its leaf, no rate in it.
    ///
    /// # Why a missing counter is not a missing reading
    ///
    /// Only `cpu.stat` is required, because it is the one file cgroup v2 puts in every directory
    /// whatever controllers reached it. The other three are absent exactly when their controller
    /// never arrived, and each says so as a VALUE ([`Counted::NoController`],
    /// [`Waiting::NotAccounted`]) instead of failing the whole reading — a host that can measure CPU
    /// and cannot measure memory should answer the half it has, which is the same per-controller
    /// degradation this module applies when it ENABLES them on the way in.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError::Read`] when the pane has no leaf to read: it ended, or it was never
    /// placed here. That is the ONE failure, and it is a fact about the pane rather than about the
    /// machine, which is why it is an error and the three absences above are not.
    pub fn charge(&self, at: PaneLineage) -> Result<Charge, TreeError> {
        let leaf = self.root.join(at.relative());
        let stat = {
            let path = leaf.join(CPU_STAT);
            std::fs::read_to_string(&path).map_err(|source| TreeError::Read { path, source })?
        };
        Ok(Charge {
            cpu_usec: keyed_value(&stat, USAGE_USEC).unwrap_or_default(),
            waiting: Pressure::read(&leaf.join(CPU_PRESSURE)).some,
            memory: read_counted(&leaf.join(MEMORY_CURRENT)),
            processes: read_counted(&leaf.join(PIDS_CURRENT)),
        })
    }

    /// Write `grant` onto the leaf a pane is ALREADY running in — a person changing their mind
    /// about a pane that exists, as distinct from [`place`](Self::place), which grants a pane that
    /// does not exist yet.
    ///
    /// # Why this is a separate door and not a re-placement
    ///
    /// [`place`](Self::place) creates. Re-placing a live pane to change one number would `mkdir` an
    /// existing directory and re-enable controllers that are already on — work whose only visible
    /// effect on a good day is nothing, and whose effect on a bad one is a failure in the interior
    /// levels reported as a failure to grant the leaf. This writes the three files a grant IS and
    /// touches nothing else, so what can go wrong is exactly what the person asked for.
    ///
    /// The weight goes first for [`PaneHomes::open`]'s reason: a ceiling that will not take must not
    /// cost the pane the share that already took.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the leaf is not there (the pane ended, or was never placed here) or
    /// if a control will not take, which means the controller behind it never reached this level.
    pub fn grant(&self, at: PaneLineage, grant: Grant) -> Result<(), TreeError> {
        let leaf = self.root.join(at.relative());
        write_control(&leaf.join(CPU_WEIGHT), &grant.share.weight().to_string())?;
        Placement {
            path: leaf,
            share: grant.share,
        }
        .limit(grant.limits)
    }

    /// What the kernel is HOLDING as this pane's grant — the person's words read back out of the
    /// leaf they were written into.
    ///
    /// # Why a grant is READ rather than remembered
    ///
    /// The daemon could keep what it last wrote and serve that. It would be wrong in the one case
    /// worth reporting: a write the kernel refused. The design document this feature comes from
    /// makes exactly that point about its own worked example — a wrapper that was syntactically
    /// perfect and never executed — and states the rule that follows, *근거 없는 권고는 사용자가
    /// 검증할 수 없다*. So the answer a person gets back is the file's contents and not the request's,
    /// and a ceiling that did not take reads as absent rather than as set.
    ///
    /// It is also what makes this need no per-pane table. The kernel is already keeping the fact,
    /// exactly as it keeps *is this pane still running* for [`sweep`](Self::sweep) — and a grant
    /// read back from the leaf survives a daemon that was killed outright, which no table could.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError::Read`] when the pane has no leaf, which is [`charge`](Self::charge)'s
    /// one failure and means the same thing. A control the leaf HAS but a controller never reached
    /// is a value ([`Ceiling::NoController`], [`Counted::NoController`]), not a failure — the same
    /// per-controller degradation the rest of this module applies.
    pub fn granted(&self, at: PaneLineage) -> Result<Granted, TreeError> {
        let leaf = self.root.join(at.relative());
        // The leaf's EXISTENCE is checked against a file cgroup v2 puts in every directory, for
        // `charge`'s reason: `cpu.weight` is absent on a host whose CPU controller never arrived, and
        // reading its absence as "this pane is gone" would answer `Gone` for every pane on a machine
        // that simply cannot weight them.
        let path = leaf.join(CPU_STAT);
        std::fs::read_to_string(&path).map_err(|source| TreeError::Read { path, source })?;
        Ok(Granted {
            share: read_counted(&leaf.join(CPU_WEIGHT)),
            memory: read_ceiling(&leaf.join(MEMORY_HIGH)),
            processes: read_ceiling(&leaf.join(PIDS_MAX)),
        })
    }

    /// Move every process a pane has into `into`, and report how many distinct ones made the trip.
    ///
    /// # Why it loops, and why it stops on NEW work rather than on an empty source
    ///
    /// A live pane is a shell, and a shell forks. Reading `cgroup.procs` once and writing that list
    /// across leaves a child born during the walk behind in a cgroup the pane no longer owns — so
    /// the source is re-read.
    ///
    /// What it must NOT do is loop until the source is empty. A pid the kernel refuses to move
    /// stays in the source forever, and "until empty" is then a spin that re-writes the same
    /// refusal a bounded number of times and reports it as that many successes. Stopping when a pass
    /// finds no pid it has not already tried terminates on the real condition — *there is nothing
    /// left that I have not attempted* — and makes the count honest.
    ///
    /// Moving a process is not the same as moving its threads: cgroup v2's `cgroup.procs` migrates
    /// the whole thread group, which is exactly the unit a pane is made of.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the source cannot be read or a pid cannot be written across. A
    /// process that EXITED mid-migration is not an error — moving one already treats a vanished
    /// pid as nothing to do, which is the common case when a pane is moved just as its command
    /// finishes.
    pub fn migrate(&self, from: PaneLineage, into: &Placement) -> Result<usize, TreeError> {
        let source = self.root.join(from.relative());
        let mut tried = std::collections::HashSet::new();
        for _ in 0..MIGRATE_PASSES {
            let fresh: Vec<u32> = read_procs(&source)?
                .into_iter()
                .filter(|pid| tried.insert(*pid))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for pid in fresh {
                move_proc(&into.path, pid)?;
            }
        }
        Ok(tried.len())
    }

    /// Remove a pane's cgroup, and any level it leaves empty.
    ///
    /// A level that still holds a sibling refuses to go, and that refusal is the ANSWER rather than
    /// an error: it is how the tree learns a window still has panes without asking.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the pane's own cgroup will not go away — which means something is
    /// still running in it, and the caller has a child it thinks it killed.
    pub fn release(&self, at: PaneLineage) -> Result<(), TreeError> {
        let leaf = self.root.join(at.relative());
        remove_cgroup(&leaf)?;
        // Upward, best effort: `ENOTEMPTY` here is a sibling, not a fault.
        let mut level = leaf;
        for _ in 0..at.interiors().len() {
            level.pop();
            if std::fs::remove_dir(&level).is_err() {
                break;
            }
        }
        Ok(())
    }

    /// Remove every cgroup in the tree that nothing is running in, and report how many went.
    ///
    /// # Why a sweep, and not a note of which pane died
    ///
    /// The obvious alternative is a table from pane to cgroup, written at birth and read at death.
    /// It has to be right in three places — birth, death, and every path that ends a pane without
    /// going through the usual death — and a table that is wrong is invisible: the leak it leaves
    /// is an empty directory nobody looks at.
    ///
    /// The kernel is already keeping the fact. `rmdir` on a cgroup with a live process in it fails
    /// with `EBUSY`, so "is this pane still running?" needs no bookkeeping at all — asking IS the
    /// answer. That also makes this self-healing: a tree left behind by a daemon that was killed
    /// outright is cleaned by the next one to adopt the same root, which no side table could do.
    ///
    /// Never returns an error. A level that refuses to go is a level that is still in use, which is
    /// the normal case for every sweep but the last.
    pub fn sweep(&self) -> usize {
        let mut removed = 0;
        // Deepest first, so a window emptied by its last pane goes in the same pass rather than
        // waiting for the next one.
        for session in interior_children(&self.root) {
            for window in interior_children(&session) {
                for pane in interior_children(&window) {
                    removed += usize::from(std::fs::remove_dir(&pane).is_ok());
                }
            }
        }
        for session in interior_children(&self.root) {
            for window in interior_children(&session) {
                removed += usize::from(std::fs::remove_dir(&window).is_ok());
            }
            removed += usize::from(std::fs::remove_dir(&session).is_ok());
        }
        removed
    }
}

/// How many times [`Tree::migrate`] re-reads a source cgroup before giving up on its stragglers.
///
/// Three, because the second pass exists to catch what forked during the first and the third to say
/// the second was not a fluke. A pane forking faster than three passes can drain is a pane whose
/// resource use is not going to be fixed by a fourth: the stragglers stay where they are, charged to
/// the window the pane left, which is bad accounting and not a broken terminal.
const MIGRATE_PASSES: usize = 3;

/// Where a pane's processes live, and the ONE thing that puts them there.
///
/// # Why this is a type and not two calls at each spawn
///
/// R336 placed panes from `Host::spawn` — one of FIVE doors a pane arrives through (the daemon's
/// wire, a restore, an in-process client's `new_pane`, and a plugin's spawn, plus a sixth arrival
/// that is not a birth: another window's `break-pane`). The other four placed nothing, and the gate
/// written for the first passed the whole time. That is what a policy carried in a caller's
/// arguments buys: it is correct exactly where somebody remembered it.
///
/// So the policy lives here — sweep the dead, place the newborn, write its ceilings, open its
/// `cgroup.procs` for the child to join itself — and a [`Workspace`](crate::Workspace) holds one.
/// Every door then goes through the pool, because a pool is what a pane is born into and moved
/// into, and no door can forget what it never had to say.
///
/// [`none`](Self::none) is the honest spelling of "this host enforces nothing": a GUI's in-process
/// host, a test, a machine [`Enforcement::probe`] found nothing on. Such a pane opens exactly as
/// every pane did before any of this existed.
#[derive(Default)]
pub struct PaneHomes {
    /// The subtree, or nothing to place into.
    ///
    /// An `Option` rather than two types because the ABSENCE is a designed state that every method
    /// here answers the same way — do nothing, successfully — and a caller that had to branch on
    /// which kind of homes it held would be a caller writing that branch four times.
    tree: Option<Tree>,
    /// Serialises placing a pane against sweeping away the panes that ended.
    ///
    /// A freshly placed cgroup is EMPTY until its child is moved in, and empty is exactly what the
    /// sweep collects. Without this, one thread's birth could be swept out from under it by
    /// another's, and the pane would come up unweighted for no reason anybody could reproduce.
    placing: std::sync::Mutex<()>,
    /// What ceilings each pane is born with, asked at every birth — `None` for uncapped, which is
    /// what a host that has never been given a source answers.
    limits: Option<LimitSource>,
}

impl std::fmt::Debug for PaneHomes {
    /// Hand-written because a [`LimitSource`] is a closure and closures are not [`Debug`]. What a
    /// reader of a log wants anyway is the two facts a derive could not have shown: whether there is
    /// a tree at all, and what the ceilings say RIGHT NOW.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneHomes")
            .field("tree", &self.tree)
            .field("limits", &self.limits())
            .finish()
    }
}

impl PaneHomes {
    /// A host that enforces nothing — see the type docs for who those are.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Place panes into `tree`, with no ceilings on them.
    #[must_use]
    pub fn over(tree: Tree) -> Self {
        Self {
            tree: Some(tree),
            placing: std::sync::Mutex::new(()),
            limits: None,
        }
    }

    /// Ask `source` at each birth what ceilings the pane should carry — the seam `sprag-host` uses
    /// to put the user's `pane-memory-limit` and `pane-process-limit` behind every pane without
    /// this crate learning what a config file is.
    ///
    /// A builder rather than a second constructor: a host with a tree and no ceilings is the
    /// ordinary case, and making every call site name its absence would be making them say
    /// something they have no opinion about.
    #[must_use]
    pub fn limited_by(mut self, source: LimitSource) -> Self {
        self.limits = Some(source);
        self
    }

    /// The delegated subtree panes are placed into, or `None` on a host that enforces nothing.
    ///
    /// The one way out of this type to a PATH, and it hands back a read-only borrow rather than the
    /// [`Tree`]: a caller that wanted to place a pane has the pool's own doors for it, and a caller
    /// that wants to walk the hierarchy above this daemon has [`CgroupNode`]. Handing out the tree
    /// would hand out `place` and `charge` to anybody who asked where the daemon lives.
    #[must_use]
    pub fn tree_root(&self) -> Option<&Path> {
        self.tree.as_ref().map(Tree::root)
    }

    /// What a pane born now should be capped at.
    fn limits(&self) -> Limits {
        self.limits.as_ref().map_or(Limits::UNCAPPED, |ask| ask())
    }

    /// What a pane nobody has said anything about gets: an even share, and the MACHINE's ceilings as
    /// they read right now.
    ///
    /// Asked at every use rather than held, which is `limits`' rule (crate-private) and its reason: a
    /// person who raises a ceiling in their config gets it on their next pane with nothing to
    /// restart.
    ///
    /// Public because a caller EDITING one pane's grant has to start from what that pane currently
    /// follows, and for a pane nobody has singled out that is this — see
    /// [`Workspace::pane_grant_or_default`](crate::Workspace::pane_grant_or_default).
    #[must_use]
    pub fn default_grant(&self) -> Grant {
        Grant::even(self.limits())
    }

    /// Make `at`'s cgroup and hand back its open `cgroup.procs`, for the child to write itself into
    /// between `fork` and `exec`.
    ///
    /// # Why an open descriptor and not a call after the spawn
    ///
    /// Placing a child AFTER it execs is what R336 measured: a pane running `sh -c 'sleep 60 & sleep
    /// 60'` had BOTH of its children born in the daemon's own cgroup, because the shell forked while
    /// the parent was still creating directories. Handing the spawn an already-open descriptor
    /// closes that window by construction — the cgroup exists before the child does, and the child
    /// joins itself with one write and no allocation.
    ///
    /// **Never fails a birth.** A pane that could not be placed runs unweighted, which is what every
    /// pane did before this existed; refusing to open it would trade a missing guarantee for a
    /// missing terminal. What went wrong is logged, once, at the moment it is known.
    ///
    /// This end of the promise was kept and the OTHER end was not, for two rounds: a descriptor
    /// that opens is not a migration the kernel will allow, and the child's write is where the
    /// second check happens. That half now answers with
    /// [`Joined`](crate::pty::Joined) instead of failing the spawn — so the sentence above is true
    /// of a pane on any host, rather than true of every pane except the ones it was written for.
    #[cfg(unix)]
    pub fn open(&self, at: PaneLineage) -> Option<std::os::fd::OwnedFd> {
        let tree = self.tree.as_ref()?;
        let _placing = self
            .placing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The cgroups of panes that have ended, collected here rather than from a death signal.
        //
        // `rmdir` refuses a cgroup with a live process in it, so the kernel already holds the fact a
        // pane-to-cgroup table would duplicate — and asking it is what makes this self-healing
        // across a daemon that was killed outright, which no table could be. The honest cost, stated
        // rather than hidden: a dead pane's cgroup lingers until the NEXT pane is born. They are
        // empty directories, births and deaths alternate in a terminal, and a daemon whose last pane
        // died is on its way out anyway.
        tree.sweep();
        // The MACHINE's grant, always: nobody has said anything about a pane that does not exist
        // yet, which is what makes a birth the one door with no override to consult.
        let grant = self.default_grant();
        let placed = match tree.place(at, grant.share) {
            Ok(placed) => placed,
            Err(error) => {
                tracing::warn!(%error, pane = at.pane.0, "pane opened without an enforced share");
                return None;
            }
        };
        // A ceiling that will not take does NOT cost the pane its share, and the order says so: the
        // weight is already written above, and a pane weighted but uncapped is strictly better than
        // one that got neither. A person who set a ceiling the kernel refused needs to be told, so
        // this warns rather than passing silently — and then opens the pane.
        if let Err(error) = placed.limit(grant.limits) {
            tracing::warn!(%error, pane = at.pane.0, "pane opened without the ceilings it was given");
        }
        match placed.open_for_join() {
            Ok(fd) => Some(fd),
            Err(error) => {
                tracing::warn!(%error, pane = at.pane.0, "pane opened without an enforced share");
                None
            }
        }
    }

    /// What the kernel has charged the pane whose placement ANSWER is `at`, or why there is nothing
    /// to read.
    ///
    /// # Why it takes the answer and not an address
    ///
    /// `at` is [`crate::workspace::Pane`]'s own `home`, which R337 made the ANSWER to the placement
    /// rather than the address the placement would have used. Measuring has to follow the same rule
    /// for a stronger reason than moving did: a reading taken at an address the pane was never put
    /// at would either fail, or — after that address was later re-used — report SOMEBODY ELSE'S
    /// numbers under this pane's id. So the absence is passed in rather than resolved here, and the
    /// ways there can be no reading are values a caller shows a person.
    ///
    /// # Errors
    ///
    /// Returns [`Unmeasured`], which is a state and not a fault: three of its four arms are
    /// ordinary on hosts this product supports.
    pub fn charge(&self, at: Landing) -> Result<Charge, Unmeasured> {
        // The TREE first, then the pane: with no subtree every pane is unplaced, and answering
        // "this pane's placement failed" for all of them would send a person hunting a fault in
        // their pane instead of reading the one sentence that is true of their whole machine.
        let tree = self.tree.as_ref().ok_or(Unmeasured::NothingEnforced)?;
        let at = at.measurable()?;
        tree.charge(at).map_err(|error| {
            tracing::debug!(%error, pane = at.pane.0, "a placed pane had no cgroup to read");
            Unmeasured::Gone
        })
    }

    /// Give the LIVE pane whose placement answer is `at` the grant a person just asked for, and
    /// answer with what the kernel holds afterwards.
    ///
    /// # Why it answers with a reading and not with an acknowledgement
    ///
    /// Returning `Ok(())` would tell a person their ceiling is in force on a host that has no
    /// `memory` controller to put it in, which is the exact failure the design document behind this
    /// feature records: a setting that is syntactically perfect and never executes. So the write is
    /// followed by [`granted`](Self::granted) and the ANSWER is the file's contents — a person who
    /// asked for 512 MiB on a host without the controller reads [`Ceiling::NoController`] and knows
    /// to go and change their delegation rather than their command line.
    ///
    /// # Errors
    ///
    /// Returns [`Unmeasured`] for the reasons a pane has no leaf, which are the same
    /// [`charge`](Self::charge) answers and mean the same things. A write the kernel refused is NOT
    /// among them: it comes back in the reading, per-control, which is finer than a failure —
    /// [`Unmeasured::Refused`] is the kernel refusing the pane's CHILD at its birth, which is a
    /// different event and leaves no leaf to write to at all.
    pub fn grant(&self, at: Landing, grant: Grant) -> Result<Granted, Unmeasured> {
        let tree = self.tree.as_ref().ok_or(Unmeasured::NothingEnforced)?;
        let at = at.measurable()?;
        // Logged and NOT propagated, for the reason above: `grant` fails per-control, and each of
        // those failures is a value the reading below carries. Failing here would replace three
        // specific answers with one vague one.
        if let Err(error) = tree.grant(at, grant) {
            tracing::debug!(%error, pane = at.pane.0, "part of a pane's grant did not take");
        }
        tree.granted(at).map_err(|error| {
            tracing::debug!(%error, pane = at.pane.0, "a granted pane had no cgroup to read");
            Unmeasured::Gone
        })
    }

    /// What the kernel is holding as the grant of the pane whose placement answer is `at`, or why
    /// there is nothing to read.
    ///
    /// [`charge`](Self::charge)'s twin, taking the placement ANSWER for the same reason and failing
    /// in the same ways: a grant read at an address a pane was never put at would report somebody
    /// else's ceilings under this pane's id.
    ///
    /// # Errors
    ///
    /// Returns [`Unmeasured`], which is a state and not a fault — three of its four arms are
    /// ordinary on hosts this product supports.
    pub fn granted(&self, at: Landing) -> Result<Granted, Unmeasured> {
        let tree = self.tree.as_ref().ok_or(Unmeasured::NothingEnforced)?;
        let at = at.measurable()?;
        tree.granted(at).map_err(|error| {
            tracing::debug!(%error, pane = at.pane.0, "a placed pane had no grant to read");
            Unmeasured::Gone
        })
    }

    /// Move an already-running pane's processes from the cgroup `from` names into the one `to` does
    /// — what a `break-pane`, a `join-pane`, a `move-pane` or a `swap` owes the projection.
    ///
    /// # Why a move has to touch the kernel at all
    ///
    /// The resource tree is a PROJECTION of the identity tree. A pane's identity changes when it
    /// moves between windows, so a projection computed only at birth stops being one at the first
    /// move: a person who pulls a runaway build into its own window to contain it would find it
    /// still eating the share of the window they pulled it out of. Re-placing the DIRECTORY is not
    /// enough either — an empty new leaf beside a full old one changes nothing about who the kernel
    /// charges.
    ///
    /// # Why the pane's OWN grant has to be carried across
    ///
    /// `own` is what a person said about THIS pane, or `None` for a pane nobody has singled out.
    /// The distinction cannot be recovered from the kernel: a leaf holding 512 MiB holds the same
    /// bytes whether that came from the machine's config or from somebody typing it at this pane, so
    /// a move that read the old leaf back would freeze the machine's setting onto every pane that
    /// ever moved, and a move that re-asked the config would silently undo the person's override at
    /// the moment they most expect it to hold — pulling a runaway build into its own window is when
    /// somebody has just capped it.
    ///
    /// So each half follows the authority it came from: an overridden pane carries its own grant,
    /// and an ordinary one re-asks the machine (which is what shipped, and why a person who raises a
    /// ceiling in their config sees it on the next pane that moves).
    ///
    /// Like [`open`](Self::open) this never fails the operation it serves. A move whose cgroup half
    /// did not work leaves a pane weighted where it used to be, which is worse accounting and not a
    /// broken terminal, so it is logged and the pane still moves.
    pub fn relocate(&self, from: PaneLineage, to: PaneLineage, own: Option<Grant>) {
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        if from == to {
            return;
        }
        let grant = own.unwrap_or_else(|| self.default_grant());
        // NOT under a sweep, unlike a birth: the source leaf is occupied by the very processes about
        // to be moved, and the destination is made and filled in one breath below.
        let _placing = self
            .placing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let placed = match tree.place(to, grant.share) {
            Ok(placed) => placed,
            Err(error) => {
                tracing::warn!(%error, pane = to.pane.0, "moved pane kept its old share");
                return;
            }
        };
        // A moved pane keeps its ceilings, because they are a fact about the pane and not about the
        // window it happens to be in — see the note above on which authority each half follows.
        if let Err(error) = placed.limit(grant.limits) {
            tracing::warn!(%error, pane = to.pane.0, "moved pane lost the ceilings it was given");
        }
        match tree.migrate(from, &placed) {
            // Read rather than discarded, and at `debug` rather than dropped: ZERO is the reading
            // that says the move did nothing, and it is indistinguishable from success in every
            // other signal this function produces.
            Ok(moved) => {
                tracing::debug!(
                    moved,
                    pane = to.pane.0,
                    "a moved pane took its processes with it"
                );
            }
            Err(error) => {
                tracing::warn!(%error, pane = to.pane.0, "moved pane left processes behind");
                return;
            }
        }
        // Best effort, and last: a source that will not go is one something is still running in,
        // which the next sweep collects once it is not.
        if let Err(error) = tree.release(from) {
            tracing::debug!(%error, pane = from.pane.0, "the pane's old cgroup outlived the move");
        }
    }
}

/// The cgroup directories this tree made under `parent`, and nothing else.
///
/// Filtered to the tree's own `session-` / `window-` / `pane-` spelling so a sweep can never reach
/// the daemon's own leaf, or anything a person put in the subtree by hand.
fn interior_children(parent: &Path) -> Vec<PathBuf> {
    // Collected rather than lazy: the listing is a handful of entries, and the sweep REMOVES
    // directories as it walks — reading a directory while unlinking out of it is the kind of thing
    // that works until the day it does not.
    std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("session-")
                            || name.starts_with("window-")
                            || name.starts_with("pane-")
                    })
        })
        .collect()
}

/// One pane's cgroup, made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The leaf itself.
    path: PathBuf,
    /// What it was weighted with.
    share: Share,
}

impl Placement {
    /// The cgroup directory this pane's processes belong in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The grant this pane was placed with.
    #[must_use]
    pub fn share(&self) -> Share {
        self.share
    }

    /// Open this pane's `cgroup.procs`, for a child to write itself into before it execs.
    ///
    /// Handed to the spawn rather than used here: what makes the placement race-free is that the
    /// CHILD does the write, after `fork` and before `exec`, so nothing it forks afterwards can be
    /// born outside. See `sprag_terminal::pty::Pty::spawn`.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the file cannot be opened, which means this cgroup is not there.
    #[cfg(unix)]
    pub fn open_for_join(&self) -> Result<std::os::fd::OwnedFd, TreeError> {
        let path = self.path.join(PROCS);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map(std::os::fd::OwnedFd::from)
            .map_err(|source| TreeError::Write { path, source })
    }

    /// Write this pane's ceilings, whatever they are — including "none", which is a value.
    ///
    /// Both files are written on EVERY placement rather than only when a number is set, and that is
    /// what makes a ceiling a person REMOVED take effect on their next pane instead of surviving
    /// until their next daemon. It is the same rule `history-limit` keeps, for the same reason.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if a ceiling will not take, which means the controller behind it did
    /// not reach this level. The caller decides what that is worth: a pane with no ceiling is a
    /// working pane, so [`PaneHomes`] logs it and opens the pane anyway.
    pub fn limit(&self, limits: Limits) -> Result<(), TreeError> {
        write_control(&self.path.join(MEMORY_HIGH), &limits.memory_high())?;
        write_control(&self.path.join(PIDS_MAX), &limits.processes())
    }
}

/// The CEILINGS a pane may not cross — as distinct from the [`Share`] it is granted, which is a
/// weight and has no ceiling in it at all.
///
/// # Why a share and a ceiling are different things
///
/// A [`Share`] cannot starve anybody: a pane weighted 10 beside an idle neighbour still takes the
/// whole machine, and live grants always sum to what there is. A ceiling can, and that is the point
/// of it — a person who says *this pane may not go past 4 GiB* is buying protection for everything
/// else on the machine, and accepts that the pane pays. So the two are set separately, and the
/// default here is NONE of them: a ceiling invented without a person turns somebody's working
/// parallel build into a mysterious `fork: retry`, or an OOM, at a number nobody chose.
///
/// ghostty ships the same pair (`linux-cgroup-memory-limit`, `linux-cgroup-processes-limit`,
/// measured at `2602886`) and no CPU weight; sprag now has both halves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// Bytes, at [`MEMORY_HIGH`]. `None` is uncapped.
    memory_high: Option<u64>,
    /// Live processes, at [`PIDS_MAX`]. `None` is uncapped.
    processes: Option<u32>,
}

impl Limits {
    /// No ceilings — what a pane gets unless a person has said otherwise.
    pub const UNCAPPED: Self = Self {
        memory_high: None,
        processes: None,
    };

    /// A ceiling on memory, in BYTES. `None` removes it.
    #[must_use]
    pub const fn with_memory(mut self, bytes: Option<u64>) -> Self {
        self.memory_high = bytes;
        self
    }

    /// A ceiling on live processes. `None` removes it.
    #[must_use]
    pub const fn with_processes(mut self, most: Option<u32>) -> Self {
        self.processes = most;
        self
    }

    /// The memory ceiling as the kernel spells it.
    #[must_use]
    pub fn memory_high(self) -> String {
        self.memory_high
            .map_or_else(|| UNCAPPED.to_owned(), |bytes| bytes.to_string())
    }

    /// The process ceiling as the kernel spells it.
    #[must_use]
    pub fn processes(self) -> String {
        self.processes
            .map_or_else(|| UNCAPPED.to_owned(), |most| most.to_string())
    }
}

/// EVERYTHING A PERSON SAYS about one pane's resources — the [`Share`] of its level it is granted
/// and the [`Limits`] it may not cross.
///
/// # Why the two travel together
///
/// They are set in one breath and written to one leaf, and until this type existed they were
/// carried separately at every door: [`Tree::place`] took a share and [`Placement::limit`] took the
/// ceilings, so a caller that had both had to remember both and a caller that forgot one wrote a
/// pane half of what somebody asked for. R336 shipped exactly that asymmetry one level up and R337
/// measured what it costs — a policy carried in a caller's arguments is correct exactly where
/// somebody remembered it.
///
/// # Why a person needs to say it per PANE
///
/// The two settings behind this ([`crate::share::Share`] and [`Limits`]) reach the kernel from the
/// user's config file, which has one value for the whole machine. That is the right DEFAULT and a
/// useless override: the pane a person wants to hold back is the one running the parallel build,
/// and they know which one that is only once it is running. The design document this comes from asks
/// for the override in `config.toml`; a pane's id is minted at runtime and no config file can name
/// it, so the same ask lands here — the global option is what every pane is BORN with, and this is
/// what a person says about ONE of them afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grant {
    /// The share of its level this pane is granted.
    pub share: Share,
    /// The ceilings it may not cross.
    pub limits: Limits,
}

impl Grant {
    /// What a pane gets when nobody has said anything about it in particular: an even share of its
    /// level, and whatever ceilings the machine's own settings carry.
    #[must_use]
    pub const fn even(limits: Limits) -> Self {
        Self {
            share: Share::EVEN,
            limits,
        }
    }
}

/// One ceiling as the kernel is HOLDING it — three states, because *no ceiling* and *nothing here
/// can hold a ceiling* are different answers to the person who set one.
///
/// [`Counted`]'s argument, applied to a setting rather than a counter, and the distinction is
/// sharper here: a person who typed a memory ceiling and reads [`Uncapped`](Self::Uncapped) learns
/// their write was undone, while one who reads [`NoController`](Self::NoController) learns their
/// host never delegated `memory` and no write was ever going to take. Those send a reader to two
/// different places, and collapsing them to `None` would send them to neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ceiling {
    /// The kernel holds this ceiling, at this value. Bytes for memory, processes for pids.
    At(u64),
    /// The kernel holds `max` — this pane may go as far as the machine allows.
    Uncapped,
    /// The controller behind this ceiling never reached this pane's level, so there is no file to
    /// hold one. See the type docs for why this is not [`Uncapped`](Self::Uncapped).
    NoController,
}

/// WHAT THE KERNEL IS HOLDING as one pane's grant — as distinct from the [`Grant`] a person asked
/// for, which is a request, and from the [`Charge`] the pane ran up, which is what it then did with
/// it.
///
/// # Why the answer is the file and not the request
///
/// A write to a control file can be refused, and the refusal is exactly the case a person needs
/// reported: a ceiling on a host whose `memory` controller was never delegated is a number that
/// went nowhere. Answering with what was ASKED would make this product agree with itself about a
/// setting that is not in force — which is the failure mode the design document behind this feature
/// records twice, once for a weight that was set and did nothing and once for a wrapper that parsed
/// and never ran.
///
/// So every field here is read out of the leaf after the fact. It is also the discriminator against
/// both rivals measured at this project's pins: ghostty's `src/os/cgroup.zig` writes a memory
/// ceiling and a process ceiling and reads **nothing** back (27 lines, one function, `2602886`), and
/// herdr has no cgroup layer at all (`9a4ce5e1`).
///
/// # Why it sits beside the usage and not on its own
///
/// `6 MiB` is not a fact a person can act on; `6 MiB of 512 MiB` is, and so is `6 MiB, uncapped`.
/// R338 put the two CHARGE numbers together for this reason — cores held beside time spent waiting,
/// because either alone has two readings — and a ceiling is the third member of that set: it is what
/// turns a usage into a fraction. So it arrives in the same row, by construction rather than by a
/// caller remembering to ask twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Granted {
    /// `cpu.weight` — the share of its level this pane is holding, or no CPU controller here.
    ///
    /// A [`Counted`] rather than a [`Share`] precisely because it is a READING: a `Share` refuses a
    /// weight outside the kernel's range in its constructor, which is right for a person's request
    /// and wrong for a file. A host whose kernel one day prints something this crate's range does
    /// not allow should report what it read, not fail to answer.
    pub share: Counted,
    /// `memory.high`, in bytes.
    pub memory: Ceiling,
    /// `pids.max`, in live processes.
    pub processes: Ceiling,
}

/// WHAT THE KERNEL ACTUALLY CHARGED one pane — as distinct from the [`Share`] it was granted and
/// the [`Limits`] it may not cross, both of which are things a person SAID.
///
/// # Why a grant that cannot be seen is only half a grant
///
/// R336 gave a pane a weight and R337 gave it a ceiling, and after both a person asking *which pane
/// is eating my machine* had exactly the instrument they had before any of it existed: none. A
/// weight is not a promise about cores — a pane weighted 10 beside an idle neighbour takes the whole
/// machine, and a nominal 10:100 was MEASURED at 18:82 under load, because the kernel distributes
/// weight per runqueue and a pane with many threads under-collects. So the setting cannot be read as
/// a prediction, and the only honest source for what a pane got is what the kernel charged it.
///
/// # The two numbers that have to arrive together
///
/// [`cpu_usec`](Self::cpu_usec) alone cannot be interpreted. A pane holding a tenth of a core is
/// either a pane with nothing to do or a pane being starved of what it asked for, and those want
/// opposite responses. [`waiting`](Self::waiting) is what separates them, it is per-cgroup, and the
/// kernel keeps it for free. Serving one without the other would be serving a number whose reader
/// must guess which of two worlds they are in.
///
/// # Raw on purpose
///
/// No rate lives here. A rate needs two of these and a clock, and the second one belongs to whoever
/// keeps the baseline — [`crate::resources`], which therefore also owns the WINDOW a rate is stated
/// over. What is here is everything one read of the leaf can answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Charge {
    /// Cumulative CPU time charged to this pane and everything under it, in microseconds.
    ///
    /// Monotonic within one cgroup and NOT across a pane's life: a pane that moves between windows
    /// is placed in a fresh leaf whose counter starts at zero (see [`PaneHomes::relocate`]), so a
    /// reader differencing two samples must be prepared for the second to be smaller than the first.
    pub cpu_usec: u64,
    /// How much of the recent past this pane spent runnable and not running.
    pub waiting: Waiting,
    /// This pane's current memory footprint, in bytes.
    pub memory: Counted,
    /// How many processes this pane holds right now.
    pub processes: Counted,
}

/// How much of the recent past a pane spent RUNNABLE AND NOT RUNNING — the kernel's pressure stall
/// information for this cgroup's tasks.
///
/// The starvation half of [`Charge`], and the reason it is an enum rather than three numbers: a
/// kernel built without `CONFIG_PSI`, or booted with `psi=0`, keeps no such accounting at all, and
/// answering `0` for that host would say *this pane never waited* about a pane that may have waited
/// for everything. Absence is a different fact from zero, so it is a different value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Waiting {
    /// Measured, over the kernel's own three windows.
    Measured {
        /// The last ten seconds.
        avg10: Percent,
        /// The last minute.
        avg60: Percent,
        /// The last five minutes.
        avg300: Percent,
    },
    /// This kernel keeps no pressure accounting, so there is no number to give — which is not the
    /// same as a pane that never waited.
    NotAccounted,
}

impl Waiting {
    /// The longest window the kernel keeps, when there is a reading at all.
    ///
    /// Five minutes is the window a DIAGNOSIS is read over: a person runs a check because something
    /// has felt slow for a while, and `avg10` answers about the ten seconds their own command spent
    /// starting. The minute in between is [`avg60`](Self::avg60), which is what a threshold is set
    /// on; this is the one that says whether the minute was typical.
    #[must_use]
    pub const fn avg300(self) -> Option<Percent> {
        match self {
            Self::Measured { avg300, .. } => Some(avg300),
            Self::NotAccounted => None,
        }
    }

    /// The minute — the window a stall threshold is judged on.
    ///
    /// Long enough that one compile's burst does not trip it and short enough that a person still
    /// recognises the machine they are sitting at.
    #[must_use]
    pub const fn avg60(self) -> Option<Percent> {
        match self {
            Self::Measured { avg60, .. } => Some(avg60),
            Self::NotAccounted => None,
        }
    }
}

/// Both rows of one kernel pressure file — the stall accounting for one resource, at one scope.
///
/// # Why both rows, when [`Charge`] wanted only one
///
/// `some` and `full` answer different questions and a reader that has only one of them cannot ask
/// the other. `some` is *something here waited*, which under a pane is the starvation signal
/// [`Tree::charge`] exists for. `full` is *nothing here ran at all*, which under a pane is nearly
/// the same statement and across a whole MACHINE is the difference between a busy box and a stopped
/// one. A diagnosis of the machine is judged on `full`; a pane is judged on `some`.
///
/// Absence is per-row for [`Waiting`]'s reason: the kernel prints no `full` row for system-wide CPU
/// on some versions, and a zero there would claim the machine never stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pressure {
    /// Time when SOME task in this scope was stalled on the resource.
    pub some: Waiting,
    /// Time when EVERY task in this scope was stalled — nothing in it made progress.
    pub full: Waiting,
}

impl Pressure {
    /// What a host that keeps no pressure accounting reads as, in both rows.
    pub const NONE: Self = Self {
        some: Waiting::NotAccounted,
        full: Waiting::NotAccounted,
    };

    /// Read one pressure file — `/proc/pressure/<resource>`, or a cgroup's `<resource>.pressure`.
    ///
    /// One parser for both scopes, deliberately: the file format is the kernel's and it is the same
    /// in both places, and two readers of one format is how a machine row and a pane row come to be
    /// parsed differently. An unreadable file is [`NONE`](Self::NONE) rather than an error for
    /// [`Tree::charge`]'s reason.
    #[must_use]
    pub fn read(path: &Path) -> Self {
        let Ok(body) = std::fs::read_to_string(path) else {
            return Self::NONE;
        };
        Self::parse(&body)
    }

    /// The whole decision with the bytes handed in — the seam a fixture drives, so a kernel format
    /// this crate has never seen can be replayed instead of installed.
    #[must_use]
    pub fn parse(body: &str) -> Self {
        Self {
            some: pressure_row(body, PRESSURE_SOME),
            full: pressure_row(body, PRESSURE_FULL),
        }
    }
}

/// A cgroup counter whose CONTROLLER may never have reached this level.
///
/// `memory.current` and `pids.current` exist only where their controllers were enabled by the level
/// above, and R337 measured why that is a live case rather than a theoretical one: a
/// `cgroup.subtree_control` write is all-or-nothing, systemd delegates only what the parent slice
/// had, and a host that hands down `cpu pids` and no `memory` is an ordinary host. On such a host
/// the memory counter is not zero — it does not exist — and a zero would read as *this pane is using
/// no memory*, which is the one answer that is certainly wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Counted {
    /// The number the kernel has.
    Now(u64),
    /// The controller behind this counter never reached this pane's level, so there is nothing to
    /// read. See the type docs for why this is not a zero.
    NoController,
}

crate::closed_set! {
/// Why a pane has no reading — four states, each of which a person acts on differently.
///
/// Not an absence and not an error: three of the four are ordinary on hosts this product supports,
/// and the whole point of separating them is that *nothing on this machine is measured* and *this
/// one pane is not* send a reader in opposite directions. [`Enforcement`]'s rule, one layer out.
///
/// # Why it is a CLOSED SET and what that fixed
///
/// A surface answering this type must answer all of it: a mouth that renders three reasons and
/// falls silent on the fourth is how a person learns to distrust the row. Every such surface used
/// to hold its own hand-written list of the arms, and when R342 added
/// [`Refused`](Self::Refused) the agent mouth's list simply did not mention it — the arm reached a
/// screen with no test naming what it said. `ALL` is generated from the variant list below, so
/// there is nowhere for a fifth reason to hide from the surfaces that must speak it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unmeasured {
    /// This daemon holds no delegated subtree, so NO pane on it is placed and none can be measured.
    ///
    /// A fact about the machine rather than about the pane — the state [`Enforcement::probe`]
    /// describes, and the state every rival multiplexer measured so far is in permanently.
    NothingEnforced,
    /// The daemon does place panes, and this one is not placed: nothing was opened for it, and it
    /// has been running unweighted ever since.
    ///
    /// Distinct from the arm above precisely because it is actionable — one pane out of eight
    /// answering this is a fault to look at, and the daemon logged it when it happened.
    NotPlaced,
    /// A cgroup WAS opened for this pane and the kernel would not let its child in, for the reason
    /// carried here.
    ///
    /// # Why this is not [`NotPlaced`](Self::NotPlaced), which is what it used to answer
    ///
    /// The two are different people's problems and the difference is not visible from the pane.
    /// `NotPlaced` says *this daemon did not try*, and the reader's next move is to look at the
    /// daemon — a window with no home, a leaf that could not be created. This says *this daemon
    /// tried and the KERNEL said no*, and the reader's next move is not in this daemon at all: on
    /// the host where it was first measured (GitHub's Linux runner) the answer is `EACCES` for
    /// every pane, and no amount of configuring sprag changes it.
    ///
    /// Collapsing them cost this project a whole round. `PaneHomes::open` succeeded, so eleven
    /// panes were recorded as placed and were not; when R341 made the birth answer the JOIN, they
    /// became honest and became indistinguishable from a pane nobody had tried to place. This is
    /// the arm that was missing, and it is the project's own rule that an absence carries its
    /// reason.
    ///
    /// The sample `ALL` carries is `EACCES`, which is the errno every host that does this in
    /// practice answers with.
    Refused(Refusal) = (Refusal::from_errno(13)),
    /// It has a cgroup and the read did not land, which in practice means the pane ended between
    /// the walk that listed it and the read that would have measured it.
    Gone,
}
}

impl std::fmt::Display for Unmeasured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingEnforced => {
                f.write_str("this daemon holds no cgroup subtree, so no pane is measured")
            }
            Self::NotPlaced => f.write_str("this pane was never placed in a cgroup of its own"),
            Self::Refused(why) => {
                write!(
                    f,
                    "the kernel would not admit this pane to its cgroup: {why}"
                )
            }
            Self::Gone => f.write_str("this pane's cgroup is gone"),
        }
    }
}

/// The reason the kernel gave for refusing to admit a pane's child to its cgroup.
///
/// The errno and nothing else. It is not an [`std::io::Error`] because this crosses the wire, is
/// compared in tests and is stored on a pane — none of which that type does — and because the
/// errno is the whole of what the kernel said: the write of `"0"` to `cgroup.procs` either lands or
/// comes back with a number, and there is no message beneath it to lose.
///
/// # What the number usually is, and why the check that produces it surprises people
///
/// `EACCES`. cgroup v2's delegation containment rule is checked at the WRITE and compares the
/// writer's own cgroup against the destination's — a non-root writer needs write access to the
/// `cgroup.procs` of the common ancestor of where the process IS and where it is going. Opening the
/// destination tests the permissions on one file and says nothing about that, so a descriptor opens
/// cleanly on hosts where every migration is then refused. Rendered through
/// [`Display`](std::fmt::Display) it reads as the kernel's own sentence, which is what a person
/// needs in order to recognise it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Refusal(i32);

impl Refusal {
    /// The refusal the kernel signalled with `errno`.
    #[must_use]
    pub const fn from_errno(errno: i32) -> Self {
        Self(errno)
    }

    /// The raw platform error number, for a caller that wants to compare it rather than print it.
    #[must_use]
    pub const fn errno(self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::io::Error::from_raw_os_error(self.0).fmt(f)
    }
}

/// Where a pane's processes ACTUALLY are — the answer its birth gave, and the one thing every
/// resource reading has to be taken through.
///
/// # Why this is an enum and not an `Option<PaneLineage>`
///
/// It carries the same lineage the option did plus the reason there is none, and the reason is not
/// recoverable afterwards: the kernel refuses the migration at the instant of the birth, and a
/// process that is running in the daemon's cgroup looks identical whether it was never offered a
/// leaf or was turned away from one. Holding the refusal beside the lineage in two fields would let
/// them disagree — a pane both placed and refused is a state nothing should be able to spell — so
/// they are one value with three arms.
///
/// Every arm is ORDINARY on some host this product supports, which is why this is not an error:
/// [`Unplaced`](Self::Unplaced) is every pane on macOS and on a daemon systemd would not delegate
/// to, and [`Refused`](Self::Refused) is every pane on a host whose daemon sits outside the subtree
/// it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Landing {
    /// No cgroup was opened for this pane, so its processes are in the daemon's own.
    #[default]
    Unplaced,
    /// The child was in this leaf before its first instruction.
    At(PaneLineage),
    /// A leaf was opened and the kernel would not admit the child to it.
    Refused(Refusal),
}

impl Landing {
    /// The leaf to read this pane through, or the reason there is not one.
    ///
    /// ONE match, and every reader of a placement goes through it — the reason
    /// [`PaneHomes::charge`], [`grant`](PaneHomes::grant) and [`granted`](PaneHomes::granted) all
    /// take the placement rather than an address. Three matches would be three chances to answer
    /// [`Unmeasured::NotPlaced`] about a pane the kernel actually refused, which is the sentence
    /// this type exists to stop being printed.
    ///
    /// # Errors
    ///
    /// [`Unmeasured::NotPlaced`] or [`Unmeasured::Refused`], which are states and not faults.
    pub const fn measurable(self) -> Result<PaneLineage, Unmeasured> {
        match self {
            Self::At(lineage) => Ok(lineage),
            Self::Unplaced => Err(Unmeasured::NotPlaced),
            Self::Refused(why) => Err(Unmeasured::Refused(why)),
        }
    }

    /// The leaf a pane is in, for the callers that MOVE a pane rather than measure it.
    ///
    /// A refusal and an absence are one answer here on purpose: [`Workspace::adopt`] migrates
    /// processes OUT of a source cgroup, and there is nothing to migrate out of in either case.
    ///
    /// [`Workspace::adopt`]: crate::workspace::Workspace::adopt
    #[must_use]
    pub const fn leaf(self) -> Option<PaneLineage> {
        match self {
            Self::At(lineage) => Some(lineage),
            Self::Unplaced | Self::Refused(_) => None,
        }
    }
}

/// A percentage the kernel prints with two decimals, held as HUNDREDTHS so it stays exact.
///
/// `8869` is 88.69%. An integer rather than a float because this crosses the wire, where a float is
/// a formatting decision every peer makes differently, and because the two decimals the kernel
/// prints are the whole of the precision there is — nothing is lost by keeping them as they came.
///
/// It is READ through [`Display`](std::fmt::Display) and through [`Ord`] (`avg10 > Percent::NONE`),
/// which is everything its callers do with it, and off the wire as a plain integer — the derive is
/// `transparent`. There is deliberately no accessor: one was written this round, nothing called it,
/// and an answer no caller reads is the shape this project sweeps for.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Percent(u32);

impl Percent {
    /// Nothing at all.
    pub const NONE: Self = Self(0);

    /// From the kernel's own unit.
    #[must_use]
    pub const fn from_hundredths(hundredths: u32) -> Self {
        Self(hundredths)
    }

    /// `88.69` — a percentage written with a decimal point, from wherever it was printed.
    ///
    /// Public because the kernel is not the only thing on a machine that prints this shape: a cache
    /// reports its hit rate the same way, and a second parser for one format is how two readings of
    /// the same two decimals come to round differently. See the type docs for why the digits are
    /// taken as integers.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        parse_percent(text.trim())
    }
}

impl std::fmt::Display for Percent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{:02}%", self.0 / 100, self.0 % 100)
    }
}

/// Asked, at each pane's BIRTH, what ceilings that pane should carry.
///
/// A source rather than a value for [`crate::HistoryLimitSource`]'s reason: the answer is the
/// user's and it can change, so `sprag-host` installs one that reads `config.toml` and raising a
/// ceiling reaches the NEXT pane rather than the next daemon. A stored [`Limits`] would freeze the
/// setting at the moment the daemon was given its subtree.
pub type LimitSource = std::sync::Arc<dyn Fn() -> Limits + Send + Sync>;

/// What went wrong building or tearing down the tree, and where.
#[derive(Debug)]
pub enum TreeError {
    /// A cgroup directory could not be made.
    Create {
        /// The cgroup that could not be made.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// A cgroup's control file could not be read.
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// A cgroup's control file could not be written.
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// A cgroup directory could not be removed.
    Remove {
        /// The cgroup that would not go away.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create { path, source } => {
                write!(f, "cannot make cgroup {}: {source}", path.display())
            }
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Write { path, source } => {
                write!(f, "cannot write {}: {source}", path.display())?;
                // The one errno whose text sends a reader in the wrong direction. It does not mean
                // the kernel lacks the feature; it means an ancestor of this cgroup still holds a
                // process, which makes this one an invalid domain.
                if source.raw_os_error() == Some(NOT_SUPPORTED) {
                    f.write_str(
                        " — an ancestor cgroup still holds a process, so this one is an invalid domain",
                    )?;
                }
                Ok(())
            }
            Self::Remove { path, source } => {
                write!(f, "cannot remove cgroup {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for TreeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Create { source, .. }
            | Self::Read { source, .. }
            | Self::Write { source, .. }
            | Self::Remove { source, .. } => Some(source),
        }
    }
}

/// Where cgroup v2 is mounted on this host, when it is mounted at all.
///
/// `None` off Linux, so a caller that wants to walk the hierarchy asks once and is TOLD there is
/// none, instead of joining paths under a root that will never exist and reading every answer as an
/// absence. It is fixed rather than hunted for, for the reason recorded on the constant it returns:
/// cgroup v2's mount point is settled by systemd and by the kernel documentation alike.
#[must_use]
pub fn mount_point() -> Option<&'static Path> {
    #[cfg(target_os = "linux")]
    {
        Some(Path::new(UNIFIED_ROOT))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Any cgroup in the hierarchy, read-only — including ones this daemon did not make and may not
/// write.
///
/// # Why the tree is not enough
///
/// [`Tree`] is the subtree this daemon OWNS: it creates levels, enables controllers and writes
/// weights, and every path it touches is under its own root. That is the right bound for placing a
/// pane and the wrong one for explaining why a pane is slow, because the competition that starves a
/// pane usually sits ABOVE the tree — a batch workload in `system.slice` at the same weight as the
/// whole user session. Answering that needs a reader that can look at a cgroup this daemon has no
/// business writing.
///
/// So the split is by CAPABILITY and not by location: this type has no `place`, no `write`, and no
/// constructor that takes a [`Share`]. Reaching a stranger's cgroup through it cannot modify one,
/// and every cgroup filename in this crate still lives in this module — the single reader R338
/// established, widened to a scope it did not have rather than duplicated somewhere else.
///
/// Every reader answers an absence rather than an error, because up here absence is the normal
/// case: a level the CPU controller never reached has no `cpu.weight`, and that is a fact about the
/// host to be reported, not a failure that should cost the caller the rest of the walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgroupNode {
    /// The directory this node is.
    at: PathBuf,
}

impl CgroupNode {
    /// Read the cgroup at `at`. Nothing is opened until a reader is called.
    #[must_use]
    pub fn at(at: PathBuf) -> Self {
        Self { at }
    }

    /// The directory this node is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.at
    }

    /// What a person calls this level — its last path component.
    ///
    /// The cgroup root itself has none, and answers `/`: it is the only node with no name, and a
    /// walk that reached it has reached the top.
    #[must_use]
    pub fn name(&self) -> String {
        self.at.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// This cgroup's share of its own level, or `None` where the CPU controller never reached it.
    ///
    /// `None` is the load-bearing answer and not a default: a level with no `cpu.weight` is one
    /// where the kernel is not arbitrating CPU between the children at all, which is a different
    /// state from every child carrying the same number.
    #[must_use]
    pub fn weight(&self) -> Option<u32> {
        std::fs::read_to_string(self.at.join(CPU_WEIGHT))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// The CPU this cgroup and everything under it has consumed since it was made, in microseconds.
    ///
    /// Cumulative, so it is a rate only against a second reading — which is the whole reason the
    /// diagnosis takes two. Present in every cgroup v2 directory whatever controllers reached it
    /// — it is part of the base stat the kernel keeps for the hierarchy itself — so a level that was
    /// never given the CPU controller is still measurable, which is the case worth having.
    #[must_use]
    pub fn cpu_usec(&self) -> Option<u64> {
        keyed_value(
            &std::fs::read_to_string(self.at.join(CPU_STAT)).ok()?,
            USAGE_USEC,
        )
    }

    /// This cgroup's CPU stall accounting, both rows.
    #[must_use]
    pub fn pressure(&self) -> Pressure {
        Pressure::read(&self.at.join(CPU_PRESSURE))
    }

    /// The controllers this cgroup's PARENT enabled for it — everything it could turn on for its
    /// own children.
    #[must_use]
    pub fn controllers(&self) -> Vec<String> {
        tokens_of(&self.at.join(CONTROLLERS))
    }

    /// The controllers this cgroup HAS turned on for its children.
    ///
    /// The pair with [`controllers`](Self::controllers) is what makes a delegation answerable: a
    /// controller in the first list and not in the second was available and left off, and one in
    /// neither was never delegated to this level at all. Those are different people's problems.
    #[must_use]
    pub fn subtree_control(&self) -> Vec<String> {
        tokens_of(&self.at.join(SUBTREE_CONTROL))
    }

    /// The process ids this cgroup holds right now.
    ///
    /// A pane's own process set, exactly — which is the answer a `/proc` walk can only approximate,
    /// because a build's children reparent and a pid tree read a moment late has lost them. The
    /// membership is the kernel's and it is not a heuristic.
    #[must_use]
    pub fn procs(&self) -> Vec<u32> {
        std::fs::read_to_string(self.at.join(PROCS))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }

    /// The child cgroups of this one, in the order the filesystem lists them.
    ///
    /// Directories only: every other entry in a cgroup directory is a control file.
    #[must_use]
    pub fn children(&self) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(&self.at) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                entry
                    .file_type()
                    .ok()?
                    .is_dir()
                    .then(|| Self::at(entry.path()))
            })
            .collect()
    }
}

/// The whitespace-separated tokens of a one-line cgroup control file, or nothing where the file is
/// not there.
///
/// Tokens and never a substring search, for the reason the controller probe records: `cpu` is a
/// prefix of `cpuset`, and a substring test reports the CPU controller present on a host that
/// offers only `cpuset`.
fn tokens_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect()
}

/// `ENOTSUP` — what the kernel answers when a cgroup's ancestor still holds a process.
const NOT_SUPPORTED: i32 = 95;

/// `ESRCH` — the process named is already gone.
const NO_SUCH_PROCESS: i32 = 3;

/// Make a cgroup, treating one that already exists as made.
fn make_cgroup(path: &Path) -> Result<(), TreeError> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(TreeError::Create {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Remove a cgroup.
fn remove_cgroup(path: &Path) -> Result<(), TreeError> {
    std::fs::remove_dir(path).map_err(|source| TreeError::Remove {
        path: path.to_path_buf(),
        source,
    })
}

/// The pids a cgroup holds directly.
fn read_procs(cgroup: &Path) -> Result<Vec<u32>, TreeError> {
    let path = cgroup.join(PROCS);
    let body = std::fs::read_to_string(&path).map_err(|source| TreeError::Read { path, source })?;
    Ok(body
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect())
}

/// Move one process into `cgroup`, treating an already-exited process as nothing to do.
fn move_proc(cgroup: &Path, pid: u32) -> Result<(), TreeError> {
    match write_control(&cgroup.join(PROCS), &pid.to_string()) {
        Err(TreeError::Write { source, .. }) if source.raw_os_error() == Some(NO_SUCH_PROCESS) => {
            Ok(())
        }
        other => other,
    }
}

/// Turn on as much of [`WANTED_CONTROLLERS`] as this cgroup actually has to give.
///
/// # Why it asks first instead of writing the whole list
///
/// The write is atomic in the worst way: naming one controller a level does not have fails the
/// ENTIRE write and enables nothing (measured on a real delegated scope — see
/// [`WANTED_CONTROLLERS`]). Asking narrows the failure to the controller that is missing, so a host
/// that can weight panes but not cap their memory gets its weights, and only the ceilings are
/// unenforced — which is the degradation [`Enforcement`] already models for the whole feature,
/// applied per controller.
///
/// A level that has NONE of them writes nothing and succeeds. That is honest rather than lenient:
/// the placement below it will still fail at `cpu.weight`, which is the file that does not exist
/// when the CPU controller never reached this level, and that is where the caller is told.
fn enable_controllers(cgroup: &Path) -> Result<(), TreeError> {
    let wanted: String = available_controllers(cgroup)?
        .iter()
        .map(|controller| format!("+{controller}"))
        .collect::<Vec<_>>()
        .join(" ");
    if wanted.is_empty() {
        return Ok(());
    }
    write_control(&cgroup.join(SUBTREE_CONTROL), &wanted)
}

/// Which of [`WANTED_CONTROLLERS`] this cgroup's parent enabled for it, in the wish list's order.
///
/// Matched as whole TOKENS, never as substrings: `cpuset` contains `cpu`, and a substring test would
/// read a host offering only `cpuset` as one that can weight children. The same trap
/// [`lists_controller`] exists for on the probe side.
fn available_controllers(cgroup: &Path) -> Result<Vec<&'static str>, TreeError> {
    let path = cgroup.join(CONTROLLERS);
    let body = std::fs::read_to_string(&path).map_err(|source| TreeError::Read { path, source })?;
    Ok(WANTED_CONTROLLERS
        .into_iter()
        .filter(|wanted| body.split_whitespace().any(|have| have == *wanted))
        .collect())
}

/// Write one value to a cgroup control file.
///
/// Opened WITHOUT create: every one of these files is made by the kernel, so a path that has to be
/// created is a path that is not in a cgroup filesystem at all. Letting the write create it would
/// turn "this root was never delegated" into a silent success with a stray regular file — the
/// failure mode that makes a resource-control feature look like it works.
///
/// Truncating, which on cgroupfs is what `echo value > file` already does and which the kernel
/// ignores — it parses each write as a whole command. It matters anywhere the path is NOT cgroupfs:
/// without it a short value written over a longer one leaves the old tail behind, so the file reads
/// back as something nobody wrote.
fn write_control(path: &Path, value: &str) -> Result<(), TreeError> {
    use std::io::Write as _;

    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .and_then(|mut file| file.write_all(value.as_bytes()))
        .map_err(|source| TreeError::Write {
            path: path.to_path_buf(),
            source,
        })
}

/// One `key value` line out of a cgroup stat file, as a number.
///
/// Matched on the whole first TOKEN, never on a prefix: `cpu.stat` carries `usage_usec`,
/// `user_usec` and `system_usec`, and a `starts_with` test for the first would answer with whichever
/// of the three the kernel happened to print first on some future version.
fn keyed_value(body: &str, key: &str) -> Option<u64> {
    body.lines().find_map(|line| {
        let mut fields = line.split_ascii_whitespace();
        (fields.next()? == key).then(|| fields.next()?.parse().ok())?
    })
}

/// One named row of a pressure file's body, or the honest absence.
///
/// A row the kernel did not print is [`Waiting::NotAccounted`] and never a zero, for the reason
/// stated on [`Tree::charge`]: a kernel without `CONFIG_PSI` has no such file, and a kernel that
/// prints only `some` for a resource is not saying that `full` was never reached.
fn pressure_row(body: &str, row: &str) -> Waiting {
    let Some(fields) = body
        .lines()
        .find(|line| line.split_ascii_whitespace().next() == Some(row))
    else {
        return Waiting::NotAccounted;
    };
    Waiting::Measured {
        avg10: pressure_average(fields, "avg10"),
        avg60: pressure_average(fields, "avg60"),
        avg300: pressure_average(fields, "avg300"),
    }
}

/// One `avgN=NN.NN` field of a pressure row.
///
/// Missing reads as zero and NOT as an absence: the row was there, so this kernel does keep the
/// accounting, and a field it did not print is a window it has nothing to report for.
fn pressure_average(row: &str, window: &str) -> Percent {
    row.split_ascii_whitespace()
        .find_map(|field| field.strip_prefix(window)?.strip_prefix('='))
        .and_then(parse_percent)
        .unwrap_or(Percent::NONE)
}

/// `88.69` as hundredths of a percent.
///
/// Parsed as two integers rather than through a float: the kernel prints exactly two decimals, and
/// a float round-trip would turn an exact quantity into one whose last digit depends on the
/// rounding mode. A value with fewer decimals is padded and one with more is truncated, so a future
/// kernel that changes its precision is read rather than refused.
fn parse_percent(text: &str) -> Option<Percent> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let mut hundredths: u32 = whole.parse().ok()?;
    hundredths = hundredths.checked_mul(100)?;
    let mut digits = fraction.chars().filter(char::is_ascii_digit);
    let tens = digits.next().and_then(|digit| digit.to_digit(10))?;
    let units = digits
        .next()
        .and_then(|digit| digit.to_digit(10))
        .unwrap_or(0);
    Some(Percent::from_hundredths(
        hundredths.checked_add(tens * 10 + units)?,
    ))
}

/// A cgroup counter file, or the honest absence of the controller behind it.
fn read_counted(path: &Path) -> Counted {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| body.trim().parse().ok())
        .map_or(Counted::NoController, Counted::Now)
}

/// One ceiling file, as the three states a person acts on differently.
///
/// The word `max` is the kernel's own spelling of "no ceiling" and is what both limit files hold
/// when nobody set one — so it is read as [`Ceiling::Uncapped`] and never as a missing file. An
/// ABSENT file is the controller never arriving, which is the other answer entirely. A body that is
/// neither a number nor `max` is a file this crate does not understand, and reporting no controller
/// for it says the one true thing: nothing here is holding a ceiling anybody can rely on.
fn read_ceiling(path: &Path) -> Ceiling {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Ceiling::NoController;
    };
    let body = body.trim();
    if body == UNCAPPED {
        return Ceiling::Uncapped;
    }
    body.parse().map_or(Ceiling::NoController, Ceiling::At)
}

/// The cgroup a process is in, as a directory under the unified hierarchy.
///
/// The ONE reader of `/proc/<pid>/cgroup` in this workspace, for the reason this crate keeps one
/// reader of `/proc/<pid>/stat`: the parse has a trap in it — a hybrid host lists v1 controllers
/// beside the v2 line, so taking the first line yields a path that exists under the v2 mount often
/// enough to look like it worked — and a second copy is a second chance to fall in. A caller
/// outside this crate, the daemon watching for its own move into a freshly delegated scope, asks
/// here rather than splitting the line again.
///
/// `None` where there is no v2 hierarchy to read, or no such process.
#[cfg(target_os = "linux")]
#[must_use]
pub fn cgroup_of(pid: u32) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = unified_path(&contents)?;
    Some(Path::new(UNIFIED_ROOT).join(path.trim_start_matches('/')))
}

/// The unified-hierarchy path out of a `/proc/<pid>/cgroup` file, if it has one.
///
/// The v2 line is `0::<path>` and it is the ONLY line on a pure-v2 host, but a hybrid host lists v1
/// controllers alongside it (`1:name=systemd:/...`), so the line has to be found by its `0::`
/// prefix rather than taken as the first or the last. Reading the wrong line yields a v1 path that
/// exists under the v2 mount often enough to look like it worked.
#[cfg(target_os = "linux")]
fn unified_path(contents: &str) -> Option<&str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
}

/// Whether a `cgroup.controllers` / `cgroup.subtree_control` body lists `name`.
///
/// Space-separated on one line, but matched token-wise rather than by substring: `cpu` is a prefix
/// of `cpuset`, and a substring test would report the CPU controller present on a host that offers
/// only `cpuset` — the false positive that ends in a write to a file that is not there.
#[cfg(target_os = "linux")]
fn lists_controller(body: &str, name: &str) -> bool {
    body.split_ascii_whitespace().any(|token| token == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_outside_the_kernels_range_cannot_be_built() {
        assert_eq!(Share::new(0), Err(ShareError::OutOfRange { weight: 0 }));
        assert_eq!(
            Share::new(10_001),
            Err(ShareError::OutOfRange { weight: 10_001 })
        );
        assert_eq!(Share::new(1).map(Share::weight), Ok(1));
        assert_eq!(Share::new(10_000).map(Share::weight), Ok(10_000));
    }

    #[test]
    fn the_default_grant_is_the_kernels_own_default() {
        assert_eq!(Share::default(), Share::EVEN);
        assert_eq!(Share::EVEN.weight(), 100);
    }

    /// A stand-in cgroup filesystem: real directories, with the kernel's interface files
    /// pre-created because the kernel is what makes them.
    ///
    /// It cannot prove the KERNEL accepts anything — only `tests/pane_share_cgroup.rs`, against a
    /// real delegated scope, does that. What it proves is the sequencing, which is where the one
    /// measured failure lives: enable a controller while a process is still in an interior cgroup
    /// and the whole subtree below is silently invalid.
    struct FakeCgroupFs {
        root: PathBuf,
    }

    impl FakeCgroupFs {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("sprag-share-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let fs = Self { root };
            fs.cgroup("", "");
            fs
        }

        /// Make one cgroup under the root, holding `procs`, with the interface files the kernel
        /// would have put there.
        fn cgroup(&self, relative: &str, procs: &str) -> PathBuf {
            let path = if relative.is_empty() {
                self.root.clone()
            } else {
                self.root.join(relative)
            };
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            std::fs::write(path.join(PROCS), procs).expect("fixture procs");
            std::fs::write(path.join(SUBTREE_CONTROL), "").expect("fixture subtree_control");
            // What the PARENT enabled for this cgroup, which is what `enable_controllers` may
            // enable below it. The fixture offers all three; `a_level_enables_only_what_it_has`
            // makes one that does not.
            std::fs::write(path.join(CONTROLLERS), WANTED_CONTROLLERS.join(" "))
                .expect("fixture controllers");
            std::fs::write(path.join(CPU_WEIGHT), "100\n").expect("fixture cpu.weight");
            std::fs::write(path.join(PIDS_MAX), "max\n").expect("fixture pids.max");
            std::fs::write(path.join(MEMORY_HIGH), "max\n").expect("fixture memory.high");
            // The COUNTERS the kernel keeps in every cgroup it makes. They are here rather than in
            // the one test that reads them because R337 measured what a fixture that omits a file
            // the kernel always makes does: it pins the code to today's reads, and the next change
            // breaks every test built on it at once.
            std::fs::write(
                path.join(CPU_STAT),
                "usage_usec 0\nuser_usec 0\nsystem_usec 0\n",
            )
            .expect("fixture cpu.stat");
            std::fs::write(
                path.join(CPU_PRESSURE),
                "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
                 full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
            )
            .expect("fixture cpu.pressure");
            std::fs::write(path.join(MEMORY_CURRENT), "0\n").expect("fixture memory.current");
            std::fs::write(path.join(PIDS_CURRENT), "0\n").expect("fixture pids.current");
            path
        }

        /// Make one cgroup with NO interface files — the shape the teardown path needs.
        ///
        /// Real cgroupfs lets a cgroup be `rmdir`ed while it still holds the kernel's own interface
        /// files; a directory of ordinary files cannot be. So the fixture that exercises removal
        /// leaves them out, and says here that this is a limit of the fake rather than of the tree:
        /// `release` opens no control file, it only unlinks directories.
        fn bare(&self, relative: &str) {
            std::fs::create_dir_all(self.root.join(relative)).expect("fixture bare cgroup");
        }

        fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.root.join(relative)).expect("fixture read")
        }
    }

    impl Drop for FakeCgroupFs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The `cgroup.subtree_control` write that turns `controllers` on, as the kernel takes it.
    fn enabled(controllers: &[&str]) -> String {
        controllers
            .iter()
            .map(|controller| format!("+{controller}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn address(session: u64, window: u64, pane: u64) -> PaneLineage {
        PaneLineage {
            session: SessionId(session),
            window: WindowId(window),
            pane: PaneId(pane),
        }
    }

    #[test]
    fn a_panes_cgroup_is_spelled_from_the_ids_that_already_name_it() {
        assert_eq!(
            address(3, 7, 12).relative(),
            PathBuf::from("session-3/window-7/pane-12")
        );
    }

    #[test]
    fn adopting_vacates_the_root_before_it_enables_anything() {
        let fs = FakeCgroupFs::new("adopt-order");
        std::fs::write(fs.root.join(PROCS), "111\n222\n").expect("fixture procs");
        fs.cgroup(DAEMON_LEAF, "");

        Tree::adopt(fs.root.clone()).expect("adopt");

        // The fake keeps only the last write to a file, so the LAST pid standing is what proves the
        // move ran through every process the root held rather than stopping at the first.
        assert_eq!(fs.read(&format!("{DAEMON_LEAF}/{PROCS}")), "222");
        assert_eq!(
            fs.read(SUBTREE_CONTROL),
            enabled(WANTED_CONTROLLERS.as_slice())
        );
    }

    #[test]
    fn a_root_that_cannot_be_vacated_is_left_with_no_controllers_on() {
        let fs = FakeCgroupFs::new("adopt-abort");
        std::fs::write(fs.root.join(PROCS), "111\n").expect("fixture procs");
        // The daemon leaf's name is taken by a FILE, so the move into it cannot land.
        std::fs::write(fs.root.join(DAEMON_LEAF), "in the way").expect("fixture blocker");

        assert!(Tree::adopt(fs.root.clone()).is_err());

        // The measured defect, asserted: enabling a controller over a root that still holds a
        // process makes every child an invalid domain. Reordering the two lines turns this RED.
        assert_eq!(fs.read(SUBTREE_CONTROL), "");
    }

    #[test]
    fn placing_a_pane_enables_every_level_above_it_and_weights_the_leaf() {
        let fs = FakeCgroupFs::new("place");
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-2", "");
        fs.cgroup("session-1/window-2/pane-3", "");
        let tree = Tree {
            root: fs.root.clone(),
        };

        let placed = tree
            .place(address(1, 2, 3), Share::new(250).expect("a valid share"))
            .expect("place");

        assert_eq!(placed.path(), fs.root.join("session-1/window-2/pane-3"));
        assert_eq!(
            fs.read(&format!("session-1/{SUBTREE_CONTROL}")),
            enabled(WANTED_CONTROLLERS.as_slice())
        );
        assert_eq!(
            fs.read(&format!("session-1/window-2/{SUBTREE_CONTROL}")),
            enabled(WANTED_CONTROLLERS.as_slice())
        );
        assert_eq!(
            fs.read(&format!("session-1/window-2/pane-3/{CPU_WEIGHT}")),
            "250"
        );
        // The leaf is a leaf: nothing turned controllers on below it.
        assert_eq!(
            fs.read(&format!("session-1/window-2/pane-3/{SUBTREE_CONTROL}")),
            ""
        );
    }

    #[test]
    fn releasing_a_pane_stops_at_a_level_that_still_holds_a_sibling() {
        let fs = FakeCgroupFs::new("release");
        fs.bare("session-1/window-2/pane-3");
        fs.bare("session-1/window-2/pane-4");
        let tree = Tree {
            root: fs.root.clone(),
        };

        tree.release(address(1, 2, 3)).expect("release");

        assert!(!fs.root.join("session-1/window-2/pane-3").exists());
        // The sibling keeps its window, and the window keeps its session.
        assert!(fs.root.join("session-1/window-2/pane-4").exists());
        assert!(fs.root.join("session-1").exists());
    }

    #[test]
    fn a_sweep_takes_the_empty_levels_and_leaves_the_daemons_own_home_alone() {
        let fs = FakeCgroupFs::new("sweep");
        fs.bare("session-1/window-2/pane-3");
        fs.bare("session-1/window-2/pane-4");
        fs.bare("session-9/window-9/pane-9");
        fs.bare(DAEMON_LEAF);
        let tree = Tree {
            root: fs.root.clone(),
        };

        // Nothing is running in a fixture, so every pane level is sweepable and the levels that
        // held only those panes go with them, deepest first, in ONE pass.
        let removed = tree.sweep();

        // Three panes, the two windows they emptied, and the two sessions those emptied.
        assert_eq!(removed, 7);
        assert!(!fs.root.join("session-1").exists());
        assert!(!fs.root.join("session-9").exists());
        // The daemon's own leaf is not spelled like a level of the tree, so a sweep cannot reach
        // it — taking it would put the daemon's threads back in an interior cgroup and invalidate
        // everything below.
        assert!(fs.root.join(DAEMON_LEAF).exists());
    }

    /// A pane that moves takes its PROCESSES with it, not just its directory.
    ///
    /// The claim that is easy to satisfy without meaning it: re-placing the directory under the new
    /// window would give a new leaf and an absent old one while the kernel went on charging the old
    /// window for every process. So the pids are read out of the destination.
    ///
    /// The RELEASE of the old leaf is not asserted here and cannot be — this fixture is ordinary
    /// files, and `rmdir` refuses a directory holding the kernel's interface files that real
    /// cgroupfs lets go (see [`FakeCgroupFs::bare`], which exists for exactly that and cannot be
    /// used here because the migration has to read the source's `cgroup.procs` first). It is gated
    /// on a real kernel instead, in `sprag-host/tests/pane_placement.rs`, by counting how many
    /// leaves carry the moved pane's name.
    #[test]
    fn a_pane_that_moves_windows_takes_its_processes_into_the_new_windows_cgroup() {
        let fs = FakeCgroupFs::new("relocate");
        let homes = PaneHomes::over(Tree {
            root: fs.root.clone(),
        });
        let (from, to) = (address(1, 1, 7), address(1, 2, 7));
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-1", "");
        fs.cgroup("session-1/window-1/pane-7", "4242\n");
        // The levels the pane is moving INTO, with the interface files the kernel would have made:
        // a directory of ordinary files has none, so `place` would fail here for a reason the real
        // hierarchy does not have. See `FakeCgroupFs`.
        fs.cgroup("session-1/window-2", "");
        fs.cgroup("session-1/window-2/pane-7", "");

        homes.relocate(from, to, None);

        assert!(
            fs.root.join(to.relative()).is_dir(),
            "the pane has a leaf under the window it moved to"
        );
        assert_eq!(
            fs.read(&format!("{}/{PROCS}", to.relative().display())),
            "4242",
            "the pane's processes are what moved, not just its directory"
        );
    }

    /// A pane that moves windows keeps the ceilings it was given.
    ///
    /// A ceiling is a fact about the PANE, not about the window it happens to be in, so a `break-`
    /// or `join-pane` that dropped it would silently un-cap the very pane a person capped —
    /// and the way they would find out is the machine going down. The new leaf is a NEW cgroup, so
    /// nothing carries across on its own; this is what makes it.
    #[test]
    fn a_pane_that_moves_windows_keeps_the_ceilings_it_was_given() {
        let fs = FakeCgroupFs::new("relocate-limits");
        let homes = PaneHomes::over(Tree {
            root: fs.root.clone(),
        })
        .limited_by(std::sync::Arc::new(|| {
            Limits::UNCAPPED.with_processes(Some(77))
        }));
        let (from, to) = (address(1, 1, 7), address(1, 2, 7));
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-1", "");
        fs.cgroup("session-1/window-1/pane-7", "4242\n");
        fs.cgroup("session-1/window-2", "");
        fs.cgroup("session-1/window-2/pane-7", "");

        homes.relocate(from, to, None);

        assert_eq!(
            fs.read(&format!("{}/{PIDS_MAX}", to.relative().display())),
            "77",
            "the pane arrived in its new window uncapped"
        );
    }

    /// A move within one window is not a move.
    ///
    /// It is reachable: `swap_panes` adopts each pane into the other's pool, and two panes of ONE
    /// window swap into the pool they were already in. Without the guard the pane's own cgroup would
    /// be placed, migrated into itself, and then RELEASED — which is a live pane's cgroup deleted
    /// out from under it. The fixture makes that visible: the leaf still holds its process.
    #[test]
    fn a_pane_adopted_back_into_the_window_it_is_already_in_keeps_its_cgroup() {
        let fs = FakeCgroupFs::new("relocate-same");
        let homes = PaneHomes::over(Tree {
            root: fs.root.clone(),
        });
        let at = address(1, 1, 7);
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-1", "");
        fs.cgroup("session-1/window-1/pane-7", "4242\n");

        homes.relocate(at, at, None);

        assert_eq!(
            fs.read(&format!("{}/{PROCS}", at.relative().display())),
            "4242\n",
            "untouched — the fixture's own trailing newline is still there, so nothing rewrote it"
        );
    }

    /// A pane is born carrying the ceilings its source names, and "none" is written as a VALUE.
    ///
    /// Both halves matter. Writing `max` when a person has cleared the setting is what makes the
    /// clearing take effect on their next pane — a placement that skipped the write would leave the
    /// ceiling in whatever state the cgroup's parent had, which for a re-used leaf is the OLD
    /// number. Measured on the fixture, which keeps the last write to each file.
    #[test]
    fn a_pane_is_born_with_the_ceilings_its_source_names_and_none_is_one() {
        let fs = FakeCgroupFs::new("limits");
        let at = address(1, 1, 7);
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-1", "");
        fs.cgroup("session-1/window-1/pane-7", "");

        let capped = PaneHomes::over(Tree {
            root: fs.root.clone(),
        })
        .limited_by(std::sync::Arc::new(|| {
            Limits::UNCAPPED
                .with_memory(Some(64 * 1024 * 1024))
                .with_processes(Some(512))
        }));
        capped.open(at);
        assert_eq!(
            fs.read(&format!("{}/{MEMORY_HIGH}", at.relative().display())),
            "67108864",
            "the kernel is told BYTES, whatever unit the person typed"
        );
        assert_eq!(
            fs.read(&format!("{}/{PIDS_MAX}", at.relative().display())),
            "512"
        );

        // The person clears both. The next pane is uncapped, and it is uncapped because the files
        // were WRITTEN, not because they were left alone.
        let cleared = PaneHomes::over(Tree {
            root: fs.root.clone(),
        });
        cleared.open(at);
        assert_eq!(
            fs.read(&format!("{}/{MEMORY_HIGH}", at.relative().display())),
            UNCAPPED
        );
        assert_eq!(
            fs.read(&format!("{}/{PIDS_MAX}", at.relative().display())),
            UNCAPPED
        );
    }

    /// A ceiling of zero is not a ceiling of zero.
    ///
    /// The option surface spells "no ceiling" as `0`, and letting that through would be a pane
    /// allowed to run no processes at all — a terminal that cannot start a shell. The mapping is
    /// `sprag-host`'s (`config::pane_limits`), and this is the type's half: `None` is the only way
    /// to say uncapped here, so the wrong thing cannot be built.
    #[test]
    fn uncapped_is_absence_and_never_the_number_zero() {
        assert_eq!(Limits::UNCAPPED.processes(), UNCAPPED);
        assert_eq!(Limits::UNCAPPED.memory_high(), UNCAPPED);
        assert_eq!(Limits::UNCAPPED.with_processes(Some(0)).processes(), "0");
    }

    /// A host with nothing to enforce touches nothing at all.
    ///
    /// The designed state — a GUI's in-process host, a test, a machine with no delegated subtree —
    /// and it has to be a no-op rather than a best effort: a `PaneHomes::none()` that created
    /// directories would put a pane's cgroup somewhere no daemon owns.
    /// A grant written onto a LIVE pane reaches all three files, and the answer is READ BACK rather
    /// than echoed — the whole argument of the feature.
    ///
    /// The two ceilings are asserted at the KERNEL's spelling (bytes, a bare count) rather than at
    /// the person's (mebibytes), because the unit conversion happens above this layer and a gate
    /// that asserted the typed number would pass against a daemon that never converted.
    #[test]
    fn a_grant_reaches_the_leaf_and_the_answer_is_read_back_from_it() {
        let fs = FakeCgroupFs::new("grant-live");
        let tree = Tree::adopt(fs.root.clone()).expect("adopt");
        let at = address(1, 1, 7);
        // The fixture makes the kernel's interface files; `place` only makes directories, so every
        // level it will walk has to exist here first — the fake's standing limit.
        for level in [
            "session-1",
            "session-1/window-1",
            "session-1/window-1/pane-7",
        ] {
            fs.cgroup(level, "");
        }
        tree.place(at, Share::EVEN).expect("place the pane");

        let grant = Grant {
            share: Share::new(10).expect("a legal weight"),
            limits: Limits::UNCAPPED
                .with_memory(Some(512 * 1024 * 1024))
                .with_processes(Some(64)),
        };
        tree.grant(at, grant).expect("grant the live pane");

        let leaf = at.relative().display().to_string();
        assert_eq!(fs.read(&format!("{leaf}/{CPU_WEIGHT}")), "10");
        assert_eq!(fs.read(&format!("{leaf}/{MEMORY_HIGH}")), "536870912");
        assert_eq!(fs.read(&format!("{leaf}/{PIDS_MAX}")), "64");

        assert_eq!(
            tree.granted(at).expect("read the grant back"),
            Granted {
                share: Counted::Now(10),
                memory: Ceiling::At(536_870_912),
                processes: Ceiling::At(64),
            },
        );
    }

    /// `max` is the kernel's own word for "no ceiling", and an ABSENT file is the controller never
    /// arriving. Reading them the same way would tell a person who set a ceiling on a host without
    /// the controller that they had chosen not to set one.
    #[test]
    fn an_uncapped_ceiling_and_a_missing_controller_are_different_answers() {
        let fs = FakeCgroupFs::new("grant-absent");
        let tree = Tree::adopt(fs.root.clone()).expect("adopt");
        let at = address(1, 1, 7);
        // The fixture makes the kernel's interface files; `place` only makes directories, so every
        // level it will walk has to exist here first — the fake's standing limit.
        for level in [
            "session-1",
            "session-1/window-1",
            "session-1/window-1/pane-7",
        ] {
            fs.cgroup(level, "");
        }
        tree.place(at, Share::EVEN).expect("place the pane");
        let leaf = fs.root.join(at.relative());

        // As placed: both files exist and say `max`.
        let uncapped = tree.granted(at).expect("read a fresh leaf");
        assert_eq!(uncapped.memory, Ceiling::Uncapped);
        assert_eq!(uncapped.processes, Ceiling::Uncapped);

        // A host whose `memory` controller never reached this level has no such file at all.
        std::fs::remove_file(leaf.join(MEMORY_HIGH)).expect("take the controller away");
        std::fs::remove_file(leaf.join(CPU_WEIGHT)).expect("take the cpu controller away");
        let without = tree
            .granted(at)
            .expect("still readable without the controller");
        assert_eq!(
            without.memory,
            Ceiling::NoController,
            "an absent file is not an uncapped pane",
        );
        assert_eq!(
            without.share,
            Counted::NoController,
            "and a weight nothing holds is not a weight of zero",
        );
        assert_eq!(
            without.processes,
            Ceiling::Uncapped,
            "the controller that IS there still answers — the degradation is per-control",
        );
    }

    /// A pane whose grant a PERSON set carries it across a move; a pane nobody singled out re-asks
    /// the machine.
    ///
    /// The two halves are one test because the claim is the DIFFERENCE between them: a `relocate`
    /// that always carried the old leaf, and one that always re-asked the config, each pass half of
    /// this and fail the other.
    #[test]
    fn a_moved_pane_carries_its_own_grant_and_an_ordinary_one_re_asks_the_machine() {
        let fs = FakeCgroupFs::new("grant-move");
        let tree = Tree::adopt(fs.root.clone()).expect("adopt");
        // The MACHINE's ceilings, as a config would answer them.
        let homes = PaneHomes::over(tree).limited_by(std::sync::Arc::new(|| {
            Limits::UNCAPPED.with_processes(Some(77))
        }));
        let from = address(1, 1, 7);
        let to = address(1, 2, 7);
        for level in ["session-1", "session-1/window-1", "session-1/window-2"] {
            fs.cgroup(level, "");
        }
        fs.cgroup(
            "session-1/window-1/pane-7",
            "4242
",
        );
        fs.cgroup("session-1/window-2/pane-7", "");

        // A person's own grant on THIS pane.
        let own = Grant {
            share: Share::new(10).expect("a legal weight"),
            limits: Limits::UNCAPPED.with_processes(Some(5)),
        };
        homes.relocate(from, to, Some(own));
        let moved = to.relative().display().to_string();
        assert_eq!(
            fs.read(&format!("{moved}/{CPU_WEIGHT}")),
            "10",
            "the weight a person set followed the pane into its new window",
        );
        assert_eq!(
            fs.read(&format!("{moved}/{PIDS_MAX}")),
            "5",
            "and so did the ceiling — not the machine's 77",
        );

        // A pane nobody singled out, moving the other way, takes the machine's ceilings.
        let back = address(1, 3, 7);
        fs.cgroup("session-1/window-3", "");
        fs.cgroup("session-1/window-3/pane-7", "");
        homes.relocate(to, back, None);
        let ordinary = back.relative().display().to_string();
        assert_eq!(
            fs.read(&format!("{ordinary}/{PIDS_MAX}")),
            "77",
            "an un-granted pane re-asks the machine rather than inheriting the leaf it left",
        );
        assert_eq!(
            fs.read(&format!("{ordinary}/{CPU_WEIGHT}")),
            "100",
            "and gets an even share, not the 10 the other pane was carrying",
        );
    }

    /// A grant asked of a pane the daemon never placed is the pane's own absence, and a grant on a
    /// host with no subtree is the MACHINE's — the same two-level distinction `charge` makes, and
    /// for its reason: they send a person to different places.
    ///
    /// **Both doors, not one.** `granted` is asked by the resource reading on every sample and
    /// `grant` by the verb; they are separate functions with separate `ok_or`s, so a gate on one
    /// says nothing about the other. ⚠ Written when the debt question found `granted` had exactly
    /// one caller and no test of its own.
    #[test]
    fn a_grant_with_nothing_to_write_says_which_absence_it_is() {
        let fs = FakeCgroupFs::new("grant-absences");
        let tree = Tree::adopt(fs.root.clone()).expect("adopt");
        let homes = PaneHomes::over(tree);
        assert_eq!(
            homes.grant(Landing::Unplaced, Grant::even(Limits::UNCAPPED)),
            Err(Unmeasured::NotPlaced),
        );
        assert_eq!(
            PaneHomes::none().grant(Landing::At(address(1, 1, 7)), Grant::even(Limits::UNCAPPED)),
            Err(Unmeasured::NothingEnforced),
            "a machine that enforces nothing outranks any one pane's placement",
        );
        // The READING door answers the same ones, which is what lets one row of `sprag resources`
        // say why it has no grant without the caller knowing which door failed.
        assert_eq!(homes.granted(Landing::Unplaced), Err(Unmeasured::NotPlaced));
        assert_eq!(
            PaneHomes::none().granted(Landing::At(address(1, 1, 7))),
            Err(Unmeasured::NothingEnforced),
        );
        // And a pane that WAS placed and whose leaf is gone is another, distinct from both — the
        // ordinary race where a pane ends between the walk that listed it and the read.
        let at = address(9, 9, 9);
        assert_eq!(homes.granted(Landing::At(at)), Err(Unmeasured::Gone));
        // ⚠ AND BOTH DOORS CARRY THE KERNEL'S REFUSAL THROUGH. A person who asks a refused pane
        // for a ceiling is told the kernel would not admit it, not that nobody tried: it is the
        // one absence whose remedy is outside this daemon entirely, so it is the one a person
        // most needs spelled. Both doors, because they have separate `?`s.
        let refused = Landing::Refused(Refusal::from_errno(13));
        assert_eq!(
            homes.grant(refused, Grant::even(Limits::UNCAPPED)),
            Err(Unmeasured::Refused(Refusal::from_errno(13))),
        );
        assert_eq!(
            homes.granted(refused),
            Err(Unmeasured::Refused(Refusal::from_errno(13))),
        );
    }

    #[test]
    fn a_host_with_nothing_to_enforce_places_and_moves_nothing() {
        let fs = FakeCgroupFs::new("no-homes");
        let homes = PaneHomes::none();

        homes.relocate(address(1, 1, 7), address(1, 2, 7), None);

        assert!(!fs.root.join("session-1").exists());
    }

    /// Migration tries each process ONCE, however many passes it takes to see them all.
    ///
    /// The fixture is the real kernel's opposite: writing a pid across does NOT remove it from the
    /// source here, so a loop that ran "until the source is empty" would move the same pid three
    /// times and report three. That is exactly the shape a pid the kernel REFUSES to move produces
    /// on a real host, which is why the count is what is asserted.
    #[test]
    fn migration_counts_processes_and_not_passes() {
        let fs = FakeCgroupFs::new("migrate-once");
        let tree = Tree {
            root: fs.root.clone(),
        };
        let (from, to) = (address(1, 1, 7), address(1, 2, 7));
        fs.cgroup("session-1", "");
        fs.cgroup("session-1/window-1", "");
        fs.cgroup("session-1/window-1/pane-7", "11\n22\n");
        fs.cgroup("session-1/window-2", "");
        fs.cgroup("session-1/window-2/pane-7", "");
        let into = tree.place(to, Share::EVEN).expect("place the destination");

        assert_eq!(tree.migrate(from, &into).expect("migrate"), 2);
    }

    /// A level enables only what it HAS, and a missing controller costs only itself.
    ///
    /// # The live regression this exists for
    ///
    /// R337 added `memory` to the wish list as a bare string, and a `cgroup.subtree_control` write
    /// is **all or nothing** — measured against a real delegated scope: `+cpu +nosuchctrl +pids`
    /// left the file EMPTY, not `cpu pids`. So on a host whose delegation offers `cpu pids` and no
    /// `memory` — systemd hands down whatever the parent slice enabled, and R336 measured a scope on
    /// this machine listing `memory pids` with no `cpu` — `Tree::adopt` would have failed outright
    /// and EVERY pane would have lost the share R336 gave it. A feature regressed to nothing by a
    /// ceiling nobody had set.
    ///
    /// So the claim is two-sided and both sides are asserted: what IS available gets enabled, and
    /// what is not is simply absent rather than fatal.
    #[test]
    fn a_level_enables_only_the_controllers_it_has_and_a_missing_one_is_not_fatal() {
        let fs = FakeCgroupFs::new("narrow");
        let level = fs.cgroup("session-1", "");
        // The host that would have broken: no `memory` on offer.
        std::fs::write(level.join(CONTROLLERS), "cpu pids\n").expect("fixture controllers");

        enable_controllers(&level).expect("a level missing one controller still enables the rest");

        assert_eq!(
            fs.read(&format!("session-1/{SUBTREE_CONTROL}")),
            enabled(&["cpu", "pids"]),
            "the share survives a host that cannot cap memory",
        );
    }

    /// A level with NOTHING on offer writes nothing and does not fail here.
    ///
    /// Where it fails is the leaf's `cpu.weight`, which is the file that is absent exactly when the
    /// CPU controller never arrived — so the caller is told at the point the fact is true, rather
    /// than by a write that could equally mean five other things.
    #[test]
    fn a_level_with_no_controllers_on_offer_writes_nothing() {
        let fs = FakeCgroupFs::new("nothing-on-offer");
        let level = fs.cgroup("session-1", "");
        std::fs::write(level.join(CONTROLLERS), "\n").expect("fixture controllers");

        enable_controllers(&level).expect("nothing to enable is not a failure");

        assert_eq!(fs.read(&format!("session-1/{SUBTREE_CONTROL}")), "");
    }

    /// `cpuset` is not `cpu`, on the enabling side as well as the probing side.
    ///
    /// A substring test passes this and would write `+cpu` to a level that has no CPU controller —
    /// which is the all-or-nothing write failing again, for a controller we asked for and could not
    /// have had.
    #[test]
    fn enabling_matches_whole_controller_names_not_substrings() {
        let fs = FakeCgroupFs::new("cpuset");
        let level = fs.cgroup("session-1", "");
        std::fs::write(level.join(CONTROLLERS), "cpuset memory\n").expect("fixture controllers");

        assert_eq!(
            available_controllers(&level).expect("read the offer"),
            vec!["memory"]
        );
    }

    /// What the kernel charged a pane comes back whole, from the leaf that pane is actually in.
    ///
    /// The fixture writes numbers that are all different, because a reading that put the memory
    /// figure in the process field would pass any assertion made against a fixture of zeroes.
    #[test]
    fn a_charge_reads_every_counter_out_of_the_panes_own_leaf() {
        let fs = FakeCgroupFs::new("charge");
        let at = address(1, 2, 3);
        let leaf = fs.cgroup(&at.relative().display().to_string(), "");
        std::fs::write(
            leaf.join(CPU_STAT),
            "usage_usec 8123456\nuser_usec 6000000\nsystem_usec 2123456\n",
        )
        .expect("fixture cpu.stat");
        std::fs::write(
            leaf.join(CPU_PRESSURE),
            "some avg10=88.69 avg60=49.96 avg300=7.55 total=123456789\n\
             full avg10=1.00 avg60=0.50 avg300=0.10 total=1234\n",
        )
        .expect("fixture cpu.pressure");
        std::fs::write(leaf.join(MEMORY_CURRENT), "734003200\n").expect("fixture memory.current");
        std::fs::write(leaf.join(PIDS_CURRENT), "42\n").expect("fixture pids.current");
        let tree = Tree {
            root: fs.root.clone(),
        };

        let charge = tree.charge(at).expect("a placed pane has a leaf to read");

        assert_eq!(
            charge,
            Charge {
                cpu_usec: 8_123_456,
                // The `some` row, never `full`: a pane is normally one job, and `full` on one thread
                // says the same thing far less often.
                waiting: Waiting::Measured {
                    avg10: Percent::from_hundredths(8869),
                    avg60: Percent::from_hundredths(4996),
                    avg300: Percent::from_hundredths(755),
                },
                memory: Counted::Now(734_003_200),
                processes: Counted::Now(42),
            }
        );
    }

    /// A counter whose controller never reached this level is ABSENT, and absent is not zero.
    ///
    /// The host is not hypothetical: R337 measured that a `cgroup.subtree_control` write is
    /// all-or-nothing and that systemd hands down only what the parent slice had, so a machine
    /// offering `cpu pids` and no `memory` is ordinary. Reporting `0 B` for every pane on it would be
    /// the one answer that is certainly wrong.
    #[test]
    fn a_counter_whose_controller_never_arrived_is_absent_rather_than_zero() {
        let fs = FakeCgroupFs::new("charge-narrow");
        let at = address(1, 1, 1);
        let leaf = fs.cgroup(&at.relative().display().to_string(), "");
        std::fs::remove_file(leaf.join(MEMORY_CURRENT))
            .expect("a host without the memory controller");
        let tree = Tree {
            root: fs.root.clone(),
        };

        let charge = tree.charge(at).expect("the CPU half is still readable");

        assert_eq!(charge.memory, Counted::NoController);
        // ...and losing one controller costs only itself. The whole reading failing would take the
        // CPU numbers away from a host that has them, which is the degradation this crate already
        // refuses on the way IN.
        assert_eq!(charge.processes, Counted::Now(0));
    }

    /// A kernel that keeps no pressure accounting says so instead of reporting a calm pane.
    ///
    /// `CONFIG_PSI` off, or `psi=0` on the command line, and the file simply is not there. Zero
    /// would claim this pane never waited for a core, about a pane that may have waited for
    /// everything.
    #[test]
    fn a_kernel_without_pressure_accounting_says_so_rather_than_reporting_calm() {
        let fs = FakeCgroupFs::new("charge-nopsi");
        let at = address(1, 1, 1);
        let leaf = fs.cgroup(&at.relative().display().to_string(), "");
        std::fs::remove_file(leaf.join(CPU_PRESSURE)).expect("a kernel without PSI");
        let tree = Tree {
            root: fs.root.clone(),
        };

        assert_eq!(
            tree.charge(at).expect("charge").waiting,
            Waiting::NotAccounted
        );
    }

    /// The three ways a pane has no reading are three answers, because they are acted on
    /// differently: a whole machine that enforces nothing, one pane that failed to be placed, and a
    /// pane that ended while it was being read.
    #[test]
    fn a_pane_with_no_reading_says_which_of_the_four_reasons_it_is() {
        let fs = FakeCgroupFs::new("charge-absent");
        let at = address(1, 1, 1);

        assert_eq!(
            PaneHomes::none().charge(Landing::At(at)),
            Err(Unmeasured::NothingEnforced),
            "a daemon with no subtree measures nothing, and that is about the machine"
        );
        // ⚠ THE CASE THAT ACTUALLY SHIPS, and the one the line above cannot discriminate: on a host
        // with no subtree, nothing was ever placed, so every pane arrives here with NO home either.
        // Both facts are true at once and only one of them is the answer a person can act on —
        // measured, when reversing the two questions left the test above GREEN while every pane on
        // such a host started reporting a placement fault of its own.
        assert_eq!(
            PaneHomes::none().charge(Landing::Unplaced),
            Err(Unmeasured::NothingEnforced),
            "the machine's reason outranks the pane's: a host that places nothing must not report \
             every pane as one that failed to be placed"
        );

        let homes = PaneHomes::over(Tree {
            root: fs.root.clone(),
        });
        assert_eq!(
            homes.charge(Landing::Unplaced),
            Err(Unmeasured::NotPlaced),
            "a daemon that DOES place panes and did not place this one is a fault to look at"
        );
        assert_eq!(
            homes.charge(Landing::At(at)),
            Err(Unmeasured::Gone),
            "a pane whose leaf is not there ended between the walk and the read"
        );
        // ⚠ THE FOURTH, and the one that had no spelling until this round: the daemon placed
        // panes, it tried to place THIS one, and the kernel turned its child away. Every pane on
        // GitHub's Linux runner is in this state, where the three above would have called it a
        // placement this daemon never attempted and sent the reader hunting the wrong fault.
        assert_eq!(
            homes.charge(Landing::Refused(Refusal::from_errno(13))),
            Err(Unmeasured::Refused(Refusal::from_errno(13))),
            "a refusal is the kernel's answer and it is carried, not flattened into an absence"
        );
    }

    /// Every reason says something, and something DIFFERENT — the one source both mouths render.
    ///
    /// `sprag resources` prints `{reason}` and nothing else, and the MCP tool renders the same
    /// `Display`, so this is where a silent or duplicated arm would reach a person. Driven from
    /// `ALL` rather than a list retyped here: the hand-written version of this claim on the agent
    /// surface is what let the fourth reason ship untested.
    #[test]
    fn every_reason_a_pane_has_no_reading_says_its_own_sentence() {
        let said: Vec<String> = Unmeasured::ALL
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        for (reason, sentence) in Unmeasured::ALL.iter().zip(&said) {
            assert!(
                !sentence.is_empty(),
                "{reason:?} renders as nothing, and a blank row is what these arms exist to \
                 prevent",
            );
            assert_eq!(
                said.iter().filter(|other| *other == sentence).count(),
                1,
                "{reason:?} says the same thing as another reason ({sentence:?}), so a person \
                 cannot act on the difference the arms were split for",
            );
        }
        // And the refusal carries the KERNEL'S words, which is the whole of what a person on such
        // a host has to go on. Not merely "different from its neighbours" — the errno itself.
        assert!(
            Unmeasured::Refused(Refusal::from_errno(13))
                .to_string()
                .contains("Permission denied"),
            "a refusal prints what the kernel said: {}",
            Unmeasured::Refused(Refusal::from_errno(13)),
        );
    }

    #[test]
    fn a_pressure_percentage_keeps_the_two_decimals_the_kernel_prints() {
        assert_eq!(parse_percent("88.69"), Some(Percent::from_hundredths(8869)));
        assert_eq!(parse_percent("0.00"), Some(Percent::NONE));
        assert_eq!(
            parse_percent("100.00"),
            Some(Percent::from_hundredths(10_000))
        );
        // A float round-trip is what this avoids: the exact quantity survives, and so does its
        // rendering.
        assert_eq!(Percent::from_hundredths(8869).to_string(), "88.69%");
        assert_eq!(Percent::from_hundredths(705).to_string(), "7.05%");
        assert_eq!(parse_percent("what"), None);
    }

    /// `cpu.stat` carries `usage_usec`, `user_usec` and `system_usec`, and a prefix test would
    /// answer with whichever the kernel printed first.
    #[test]
    fn a_stat_key_is_matched_whole_and_never_as_a_prefix() {
        let body = "usage_usec 900\nuser_usec 700\nsystem_usec 200\n";
        assert_eq!(keyed_value(body, USAGE_USEC), Some(900));
        assert_eq!(keyed_value(body, "user_usec"), Some(700));
        assert_eq!(keyed_value(body, "nosuch"), None);
        // ⚠ THE PREFIX ITSELF, which is the case the three lines above cannot express: no key in a
        // `cpu.stat` is a prefix of another, so a `starts_with` match passes all of them. Asking for
        // a prefix is what tells the two apart — measured, when swapping the comparison for
        // `starts_with` left this test GREEN until this line existed.
        assert_eq!(keyed_value(body, "usage"), None);
        assert_eq!(keyed_value(body, "user"), None);
    }

    #[test]
    fn a_control_file_that_does_not_exist_is_never_created() {
        let fs = FakeCgroupFs::new("no-create");
        let absent = fs.root.join("not-a-cgroup").join(CPU_WEIGHT);

        let failed = write_control(&absent, "100");

        // A root that was never delegated must fail loudly, not accumulate stray regular files that
        // make an unenforced tree look like an enforced one.
        assert!(matches!(failed, Err(TreeError::Write { .. })));
        assert!(!absent.exists());
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;

        /// The two bodies measured on one host at one moment: a desktop terminal's transient scope,
        /// and a scope under a slice that was given the CPU controller. The probe has to separate
        /// them, because that difference is the entire defect this module exists for.
        const GHOSTTY_SCOPE: &str = "memory pids\n";
        const DELEGATED_SCOPE: &str = "cpu memory pids\n";

        #[test]
        fn a_scope_without_the_cpu_controller_cannot_weight_its_children() {
            assert!(!lists_controller(GHOSTTY_SCOPE, CPU_CONTROLLER));
            assert!(lists_controller(DELEGATED_SCOPE, CPU_CONTROLLER));
        }

        #[test]
        fn cpuset_alone_is_not_the_cpu_controller() {
            // A substring test passes this and is wrong; the token test is the point.
            assert!(!lists_controller("cpuset memory pids\n", CPU_CONTROLLER));
            assert!(lists_controller(
                "cpuset cpu io memory pids\n",
                CPU_CONTROLLER
            ));
        }

        #[test]
        fn the_unified_line_is_found_by_its_prefix_not_its_position() {
            assert_eq!(
                unified_path("0::/user.slice/user-1000.slice\n"),
                Some("/user.slice/user-1000.slice")
            );
            // A hybrid host: the v2 line is last, and taking the first would yield a v1 path.
            assert_eq!(
                unified_path("1:name=systemd:/init.scope\n0::/user.slice\n"),
                Some("/user.slice")
            );
            // v1-only: there is no unified path to find, and inventing one is the bug.
            assert_eq!(unified_path("1:cpu,cpuacct:/user.slice\n"), None);
            assert_eq!(unified_path(""), None);
            // A `0::` line that is not a path is not a path.
            assert_eq!(unified_path("0::\n"), None);
        }

        #[test]
        fn an_unreadable_self_cgroup_is_reported_not_guessed() {
            let denied = Err(std::io::Error::other("denied"));
            assert_eq!(
                Enforcement::probe_under(Path::new("/sys/fs/cgroup"), &denied),
                Enforcement::Unenforceable {
                    reason: Unenforceable::NoUnifiedHierarchy,
                }
            );
        }

        #[test]
        fn a_hierarchy_without_our_own_cgroup_in_it_is_not_available() {
            // The path parses, but nothing is mounted under this root, so no controller list can be
            // read — which must answer "no hierarchy I can read myself into", never "available".
            let missing = Ok("0::/user.slice/nothing-here\n".to_owned());
            assert_eq!(
                Enforcement::probe_under(Path::new("/nonexistent-cgroup-root"), &missing),
                Enforcement::Unenforceable {
                    reason: Unenforceable::NoUnifiedHierarchy,
                }
            );
        }

        #[test]
        fn probing_this_machine_answers_without_panicking() {
            // Deliberately asserts no particular verdict: which one is right depends on where the
            // test runner's own cgroup sits, and a test that pinned it would be asserting the
            // developer's desktop. What is asserted is that the probe reaches an answer at all.
            let _ = Enforcement::probe();
        }
    }
}
