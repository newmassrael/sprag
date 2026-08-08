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
//! What this probe does NOT prove is that a weight has been applied to anything; there is no
//! placement here yet. It answers whether placement could be enforced, which is the question that
//! has to be answered first and the one nothing in this crate could answer before.

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

/// A cgroup's ceiling on live processes.
const PIDS_MAX: &str = "pids.max";

/// What every interior level of the tree turns on for its children.
///
/// `cpu` is the point. `pids` rides along because it is what bounds one pane's fork storm from
/// taking the pid budget the person's other panes need, and enabling it costs a counter.
///
/// `memory` is deliberately NOT here: nothing reads a memory number yet, and a controller that is
/// enabled but unused buys per-cgroup accounting for no answer.
const ENABLE_CONTROLLERS: &str = "+cpu +pids";

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

    /// Move an already-running process into this pane's cgroup.
    ///
    /// # This is the racy half, and it is racy on purpose until it is not
    ///
    /// A process moved AFTER it starts can have forked already, and those children stay where they
    /// were born. For a pane's shell the window is the microseconds between `exec` and this call, so
    /// it is small — but it is not zero, and calling it small is how it survives. The fix is
    /// `clone3(CLONE_INTO_CGROUP)`, which makes the child be BORN here and closes the window by
    /// construction; this method is what the tree can offer until the spawn seam owns its own
    /// `fork`/`exec`.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the process cannot be moved. A process that exited between the
    /// caller's decision and this write is NOT an error — there is nothing left to move.
    pub fn join(&self, pid: u32) -> Result<(), TreeError> {
        move_proc(&self.path, pid)
    }

    /// Cap the number of processes this pane may have alive at once.
    ///
    /// Left uncapped by default, deliberately: a ceiling invented without a person turns somebody's
    /// working parallel build into a mysterious `fork: retry` at a number nobody chose. The
    /// mechanism is here so the option surface can hand it a number that a person did choose.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] if the cap will not take, which means the `pids` controller did not
    /// reach this level.
    pub fn cap_processes(&self, most: u32) -> Result<(), TreeError> {
        write_control(&self.path.join(PIDS_MAX), &most.to_string())
    }
}

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

/// Turn on what this cgroup's children need.
fn enable_controllers(cgroup: &Path) -> Result<(), TreeError> {
    write_control(&cgroup.join(SUBTREE_CONTROL), ENABLE_CONTROLLERS)
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
            std::fs::write(path.join(CPU_WEIGHT), "100\n").expect("fixture cpu.weight");
            std::fs::write(path.join(PIDS_MAX), "max\n").expect("fixture pids.max");
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
        assert_eq!(fs.read(SUBTREE_CONTROL), ENABLE_CONTROLLERS);
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
            ENABLE_CONTROLLERS
        );
        assert_eq!(
            fs.read(&format!("session-1/window-2/{SUBTREE_CONTROL}")),
            ENABLE_CONTROLLERS
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
