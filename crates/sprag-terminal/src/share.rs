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

#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;

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
