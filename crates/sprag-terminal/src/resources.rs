//! What each pane is actually TAKING of the machine — the cores it is holding, how much of the
//! recent past it spent waiting for cores it did not get, its memory, and how many processes it has.
//!
//! # The question, and why the two rounds before this one could not answer it
//!
//! R336 gave every pane a [`Share`](crate::share::Share) and R337 gave it
//! [`Limits`](crate::share::Limits). Both are things a PERSON says. After both, a person asking
//! *which pane is eating my machine* had the instrument they had before any of it existed — none —
//! and the settings could not stand in for one:
//!
//! * a weight is not a cap. A pane weighted 10 beside an idle neighbour takes the whole machine
//!   (measured: all 8 cores it was offered).
//! * a weight is not even a ratio. A nominal 10:100 was measured at 18:82, because the kernel
//!   distributes weight per runqueue and a pane with many threads under-collects.
//!
//! So the only honest source for what a pane got is what the kernel CHARGED it, and that is a
//! reading rather than a setting.
//!
//! # Two numbers, together, or neither means anything
//!
//! Cores held cannot be interpreted alone: a pane holding a tenth of a core is either a pane with
//! nothing to do or a pane being starved of what it asked for, and those want opposite responses
//! from a person. So [`Taken::Measured`] carries [`waiting`](Taken::Measured::waiting) beside
//! [`cpu`](Taken::Measured::cpu) and there is no shape of this type that has one without the other.
//!
//! # Why this is SAMPLED, and why the rate has a window on it
//!
//! [`crate::sampled`]'s table, applied to this fact: it changes when a program somebody started
//! decides to spend a core, which is not an event the daemon performs, so it is a reading with an
//! age and not a published state. It gets its own address for [`crate::processes`]'s reason — the
//! pane list is re-read by every attached client on every poll wake, and a per-pane file read has no
//! business there.
//!
//! A RATE is the part that is not just a read. `cpu.stat` is cumulative, so cores-per-second exists
//! only between two samples, and the honest form of the answer states the window it covers: a rate
//! over 40 ms and a rate over a minute are different claims about a pane, and a reader that cannot
//! tell them apart will read a build's opening burst as its steady state. So the window travels with
//! the number, and the sampler keeps the BASELINE that defines it — not the caller, because two
//! callers keeping their own baselines would give one pane two different answers at one instant.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::registry::SessionRegistry;
use crate::sampled::Sampled;
use crate::share::{Charge, Counted, Granted, Unmeasured, Waiting};

/// How old a baseline has to be before a fresher sample replaces it.
///
/// It sets the LONGEST window a rate is stated over, not the shortest: a caller polling faster than
/// this is answered against the same baseline until it ages out, so its windows grow from its own
/// cadence up to this and then start again. Both ends are stated on every row, so neither is a
/// number a reader has to know.
///
/// Half a second because that is long enough that scheduler granularity is noise rather than the
/// signal, and short enough that a person watching a build start sees it start. It is also what a
/// one-shot client waits when it finds a pane still [`Cpu::Settling`], which is the other reason it
/// is one constant and not two: those two windows are the same window.
pub const SETTLE: Duration = Duration::from_millis(500);

/// What one pane is taking of the machine.
///
/// Keyed by pane [`id`](Self::id) — registry-unique and never reused, so a caller joining this
/// against the pane list cannot pair one read's row with another's pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneResources {
    /// The pane this row describes.
    pub id: u64,
    /// What the kernel charged it, or why there is no reading.
    pub taken: Taken,
}

/// A pane's reading, or the reason it has none.
///
/// The absence carries its reason as a VALUE for [`crate::share::Enforcement`]'s reason: every
/// caller above this one has to be able to tell a person why their pane has no numbers, and the
/// three reasons are acted on differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Taken {
    /// The kernel is charging this pane, and this is what it charged.
    Measured {
        /// The cores it is holding, over a stated window.
        cpu: Cpu,
        /// How much of the recent past it spent runnable and not running.
        waiting: Waiting,
        /// Its current memory footprint, in bytes.
        memory: Counted,
        /// How many processes it holds.
        processes: Counted,
        /// What it is ALLOWED — the grant the kernel is holding for it, read back from the same
        /// leaf the numbers above came out of.
        ///
        /// Beside the usage rather than at an address of its own, because a usage without its
        /// ceiling is a number a person cannot act on: `6 MiB` is not a fact, `6 MiB of 512 MiB` and
        /// `6 MiB, uncapped` are two different ones. It is the same argument that puts
        /// [`waiting`](Self::Measured::waiting) beside [`cpu`](Self::Measured::cpu), and it is
        /// enforced the same way — there is no shape of this arm with a usage and no grant.
        granted: Granted,
    },
    /// Nothing measures this pane, for this reason.
    Unmeasured {
        /// Which of the three states this pane is in.
        reason: Unmeasured,
    },
}

/// The CPU a pane is holding, as a RATE — and the window that rate covers.
///
/// A rate needs two samples, so a pane the daemon has seen once has no rate yet and says so rather
/// than reporting a zero that is indistinguishable from an idle pane. The same arm answers a pane
/// whose counter RESTARTED, which is a real event and not a defensive branch: a pane that moves
/// between windows is placed in a fresh cgroup whose `usage_usec` begins again at zero, so the
/// baseline from before the move describes a leaf this pane no longer lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cpu {
    /// Held, in THOUSANDTHS OF A CORE over the stated window — `1500` is one and a half cores.
    ///
    /// Thousandths rather than a fraction because this crosses the wire: an integer means the same
    /// thing to every peer, and a millicore is the unit the rest of the industry already writes CPU
    /// budgets in.
    Held {
        /// Thousandths of a core.
        millicores: u64,
        /// The window this covers, in milliseconds. See [`SETTLE`] for what bounds it.
        over_ms: u64,
    },
    /// One sample so far, so there is no rate — ask again after [`SETTLE`].
    Settling,
}

impl Cpu {
    /// The rate a cgroup's cumulative CPU time implies, moving from `from` to `to` over `window`.
    ///
    /// The whole arithmetic of this module, public because the live gate against a real kernel
    /// (`tests/pane_share_cgroup.rs`) has two readings and a stopwatch and nothing else — and a gate
    /// that re-implemented the division would be measuring its own copy of the formula rather than
    /// the one that ships.
    ///
    /// # Why a counter that went BACKWARDS is not zero
    ///
    /// The subtraction is checked, not saturating. A pane that moves between windows is placed in a
    /// fresh cgroup whose counter starts again at zero (R337), so `to < from` is an ordinary event
    /// and not a defensive branch — and "this pane is using no CPU" is the one answer that would be
    /// certainly wrong about a pane somebody just moved BECAUSE it was busy.
    #[must_use]
    pub fn over(window: Duration, from: u64, to: u64) -> Self {
        let Some(spent) = to.checked_sub(from) else {
            return Self::Settling;
        };
        let Ok(window_usec) = u64::try_from(window.as_micros()) else {
            return Self::Settling;
        };
        // A window of zero is two samples at one instant, which no arithmetic can turn into a rate.
        if window_usec == 0 {
            return Self::Settling;
        }
        Self::Held {
            // Thousandths of a core: one core held for the whole window is `window_usec` spent, so
            // the ratio is scaled by a thousand. Done in `u128` because a busy pane on a large
            // machine can spend more microseconds than the window has, and the product overflows a
            // `u64` for windows past about five minutes.
            millicores: u64::try_from(u128::from(spent) * 1000 / u128::from(window_usec))
                .unwrap_or(u64::MAX),
            over_ms: u64::try_from(window.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// A whole [`PaneResourceSampler`] reading: every pane's resources, and how old the reading is.
pub type PaneResourceReading = crate::Reading<Vec<PaneResources>>;

/// The one place a pane's [resources](PaneResources) are sampled, the one place a sample is held
/// between reads, and the one place the BASELINE a rate is measured from lives.
///
/// Shared (`Arc`) by every arm that serves the question, which is what makes the rate one fact: two
/// samplers would keep two baselines and answer one pane two different numbers at one instant, and
/// each would be right about its own history and useless for comparing panes — which is the whole
/// use of the number.
///
/// # Locking
///
/// Sampler → registry → pool, the crate's one direction, with the baseline map taken UNDER the
/// sampler's own lock (it is only ever touched inside the sampling function, which
/// [`Sampled::read`] runs while holding that lock).
#[derive(Default)]
pub struct PaneResourceSampler {
    held: Sampled<Vec<PaneResources>>,
    /// What each pane's rate is measured FROM, by pane id.
    ///
    /// Rebuilt from the panes present at each sample rather than pruned on a pane's death: a pane
    /// that is gone leaves no row, so its baseline has nothing to be measured against and keeping it
    /// would be a map that only grows. The daemon holds no other pane-to-anything table for the same
    /// reason ([`crate::share::Tree::sweep`]).
    baselines: Mutex<HashMap<u64, Baseline>>,
}

/// The sample a pane's rate is measured from.
#[derive(Clone, Copy)]
struct Baseline {
    /// When it was taken.
    at: Instant,
    /// The cumulative CPU time the pane had then.
    cpu_usec: u64,
}

impl PaneResourceSampler {
    /// A sampler holding nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every pane's resources, no older than `max_age`, with the age it actually has. See
    /// [`Sampled::read`] for what that tolerance does and does not do.
    pub fn read(
        &self,
        registry: &Arc<Mutex<SessionRegistry>>,
        max_age: Duration,
    ) -> PaneResourceReading {
        self.held.read(max_age, || self.sample(registry))
    }

    /// Take one reading of every pane.
    ///
    /// TWO PHASES, following the crate's registry-then-pool, never-nested discipline: under the
    /// registry lock, every window's pool and nothing else; then each pool locked on its own for its
    /// panes' ids and placement answers. The cgroup reads happen with the pool lock RELEASED,
    /// because they are filesystem reads and a pool lock is what a pane's output is waiting on.
    fn sample(&self, registry: &Arc<Mutex<SessionRegistry>>) -> Vec<PaneResources> {
        let pools: Vec<_> = {
            let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
            reg.window_pools().into_iter().flatten().collect()
        };
        let anchors: Vec<_> = pools
            .iter()
            .flat_map(|pool| {
                let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
                let homes = pool.pane_homes();
                pool.panes()
                    .iter()
                    .map(|pane| (pane.id().0, Arc::clone(&homes), pane.home()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let taken = Instant::now();
        // Both reads of one leaf, taken together and with the pool lock released. The grant is
        // asked for only where the charge landed: they fail for the same three reasons, so a pane
        // with no charge has no grant either and a second `Unmeasured` would be the same sentence
        // twice.
        let charges: Vec<_> = anchors
            .into_iter()
            .map(|(id, homes, home)| {
                (
                    id,
                    homes
                        .charge(home)
                        .and_then(|charge| homes.granted(home).map(|granted| (charge, granted))),
                )
            })
            .collect();
        let mut baselines = self
            .baselines
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut fresh = HashMap::with_capacity(charges.len());
        let rows = charges
            .into_iter()
            .map(|(id, charge)| PaneResources {
                id,
                taken: match charge {
                    Ok((charge, granted)) => {
                        let cpu = rate(baselines.get(&id).copied(), &charge, taken);
                        fresh.insert(id, roll(baselines.get(&id).copied(), &charge, taken));
                        Taken::Measured {
                            cpu,
                            waiting: charge.waiting,
                            memory: charge.memory,
                            processes: charge.processes,
                            granted,
                        }
                    }
                    Err(reason) => Taken::Unmeasured { reason },
                },
            })
            .collect();
        *baselines = fresh;
        rows
    }
}

/// The rate `charge` implies against `baseline`, or [`Cpu::Settling`] where there is no baseline to
/// measure from — the sampler's half of [`Cpu::over`], which owns the arithmetic.
fn rate(baseline: Option<Baseline>, charge: &Charge, taken: Instant) -> Cpu {
    baseline.map_or(Cpu::Settling, |baseline| {
        Cpu::over(
            taken.saturating_duration_since(baseline.at),
            baseline.cpu_usec,
            charge.cpu_usec,
        )
    })
}

/// The baseline to keep for the next reading — the old one until it has aged past [`SETTLE`].
///
/// Holding it rather than replacing it on every sample is what keeps a fast poller's windows
/// meaningful: replace every time and a client polling at 20 ms measures every pane over 20 ms,
/// where scheduler granularity IS the signal. A counter that went backwards takes the fresh sample
/// immediately, because the old one describes a cgroup this pane has left.
fn roll(baseline: Option<Baseline>, charge: &Charge, taken: Instant) -> Baseline {
    let fresh = Baseline {
        at: taken,
        cpu_usec: charge.cpu_usec,
    };
    match baseline {
        Some(old)
            if taken.saturating_duration_since(old.at) < SETTLE
                && charge.cpu_usec >= old.cpu_usec =>
        {
            old
        }
        _ => fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::Percent;

    /// A registry with no session at all costs nothing and reports nothing — the idle daemon,
    /// where reading four files per pane would be pure waste.
    #[test]
    fn an_empty_registry_samples_nothing() {
        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let reading = PaneResourceSampler::new().read(&registry, Duration::ZERO);
        assert!(reading.value.is_empty());
        assert_eq!(reading.age, Duration::ZERO);
    }

    /// THE JOIN, end to end: a real pane, really placed, read TWICE — no rate the first time, and
    /// the CPU the leaf's counter moved by the second.
    ///
    /// # What only this can say
    ///
    /// [`Cpu::over`] and [`roll`] are pure and tested as such, and the wire's own test runs on a host
    /// that enforces nothing, so every row there is [`Unmeasured`]. Between them nothing exercised
    /// the part that actually has to be right: pane id → the placement's ANSWER → that leaf's files →
    /// a row, with the baseline carried between two reads. A sampler that read the wrong pane's leaf,
    /// or kept no baseline, passes both of the others.
    ///
    /// The counter is written by hand rather than earned by burning CPU, because what is under test
    /// is the join and not the kernel — `tests/pane_share_cgroup.rs` is where a real `cpu.stat` is
    /// read, and a test that had to spend a core would be paid for by everyone running the suite.
    #[test]
    fn a_placed_panes_row_has_no_rate_until_the_second_reading_and_then_the_leafs_own() {
        let root = std::env::temp_dir().join(format!("sprag-resources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            for (file, body) in [
                ("cgroup.procs", ""),
                ("cgroup.subtree_control", ""),
                ("cgroup.controllers", "cpu memory pids\n"),
                ("cpu.weight", "100\n"),
                ("cpu.stat", "usage_usec 0\n"),
                ("memory.high", "max\n"),
                ("pids.max", "max\n"),
            ] {
                std::fs::write(path.join(file), body).expect("fixture file");
            }
        };
        cgroup("");

        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = {
            let reg = registry.lock().unwrap();
            let name = reg.default_session().name().to_owned();
            reg.workspace_of(&name).expect("the default session's pool")
        };
        // The INTERIOR levels, made by hand with the interface files the kernel would have put in
        // them. Without this `place` fails at `session-N` — a directory of ordinary files has no
        // `cgroup.controllers` to read — the pane comes up unplaced, and this test would measure the
        // unplaced path while looking exactly like a pass. R337 was caught by the same trap.
        let home = pool
            .lock()
            .unwrap()
            .home()
            .expect("the pool knows its window");
        let window = format!("session-{}/window-{}", home.session.0, home.window.0);
        cgroup(&format!("session-{}", home.session.0));
        cgroup(&window);
        // ...and the LEAVES, because `write_control` never creates a file: every one of these is
        // made by the kernel on a real hierarchy, so a path that has to be created is a path that is
        // not in a cgroup filesystem. The pool mints pane ids from zero and this test spawns one, so
        // a handful covers it — and the guard below fails loudly rather than silently measuring the
        // unplaced path if it ever does not.
        for id in 0..4 {
            cgroup(&format!("{window}/pane-{id}"));
        }
        pool.lock()
            .unwrap()
            .set_pane_homes(Arc::new(crate::share::PaneHomes::over(
                crate::share::Tree::adopt(root.clone()).expect("adopt a plain directory"),
            )));
        let pane = pool
            .lock()
            .unwrap()
            .spawn(
                crate::command::default_shell_command().0,
                "sh".into(),
                40,
                8,
            )
            .expect("spawn a pane");
        let home = pool
            .lock()
            .unwrap()
            .panes()
            .iter()
            .find(|held| held.id() == pane)
            .and_then(crate::workspace::Pane::home)
            .expect("the pane was placed, or this test measures the unplaced path instead");
        let leaf = root.join(home.relative());

        let sampler = PaneResourceSampler::new();
        let row = |reading: PaneResourceReading| {
            reading
                .value
                .into_iter()
                .find(|row| row.id == pane.0)
                .expect("the pane has a row")
        };

        let first = row(sampler.read(&registry, Duration::ZERO));
        assert!(
            matches!(
                first.taken,
                Taken::Measured {
                    cpu: Cpu::Settling,
                    ..
                }
            ),
            "one sample is not a rate: {first:?}",
        );

        // The leaf's counter moves by exactly one CPU-second.
        std::fs::write(leaf.join("cpu.stat"), "usage_usec 1000000\n").expect("advance the counter");
        std::thread::sleep(Duration::from_millis(20));

        let second = row(sampler.read(&registry, Duration::ZERO));
        let Taken::Measured { cpu, .. } = second.taken else {
            panic!("a placed pane is measured: {second:?}");
        };
        let Cpu::Held {
            millicores,
            over_ms,
        } = cpu
        else {
            panic!("the second reading has a baseline to measure from: {cpu:?}");
        };
        // A second of CPU inside a window of tens of milliseconds is many cores — the exact number
        // depends on how long the sleep really took, which is why the CLAIM is that the leaf's own
        // counter reached the row rather than any particular figure.
        assert!(
            millicores > 1000,
            "one CPU-second inside {over_ms} ms should read as more than one core, not {millicores}",
        );
        assert!(over_ms > 0, "the window a rate covers is stated");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn charge(cpu_usec: u64) -> Charge {
        Charge {
            cpu_usec,
            waiting: Waiting::Measured {
                avg10: Percent::from_hundredths(1234),
                avg60: Percent::NONE,
                avg300: Percent::NONE,
            },
            memory: Counted::Now(4096),
            processes: Counted::Now(3),
        }
    }

    /// One sample is not a rate, and the honest answer says so rather than reporting a zero.
    ///
    /// A zero here would be indistinguishable from an idle pane, which is the exact confusion the
    /// whole module exists to remove.
    #[test]
    fn a_pane_seen_once_has_no_rate_yet() {
        assert_eq!(rate(None, &charge(0), Instant::now()), Cpu::Settling);
    }

    /// Two samples at ONE instant are not a rate, whatever the counter says.
    ///
    /// Reachable: two reads with a zero staleness tolerance, back to back. No arithmetic turns a
    /// window of zero into a rate, and the honest answer is the same one a first reading gets.
    #[test]
    fn two_samples_at_one_instant_are_not_a_rate() {
        assert_eq!(Cpu::over(Duration::ZERO, 0, 5_000), Cpu::Settling);
    }

    /// A full core held for the whole window reads as a thousand millicores.
    #[test]
    fn a_core_held_for_the_window_is_a_thousand_millicores() {
        let then = Instant::now();
        let baseline = Baseline {
            at: then,
            cpu_usec: 1_000_000,
        };
        // One second of wall clock, one second of CPU spent in it.
        let now = then + Duration::from_secs(1);

        assert_eq!(
            rate(Some(baseline), &charge(2_000_000), now),
            Cpu::Held {
                millicores: 1000,
                over_ms: 1000,
            }
        );
    }

    /// Four cores on a four-thread build is four thousand — the reading a person is hunting when
    /// they ask which pane is eating the machine.
    #[test]
    fn a_pane_spending_more_cpu_than_wall_clock_reads_as_several_cores() {
        let then = Instant::now();
        let baseline = Baseline {
            at: then,
            cpu_usec: 0,
        };
        let now = then + Duration::from_millis(500);

        assert_eq!(
            rate(Some(baseline), &charge(2_000_000), now),
            Cpu::Held {
                millicores: 4000,
                over_ms: 500,
            }
        );
    }

    /// A counter that went BACKWARDS is a pane that moved windows, not a pane using nothing.
    ///
    /// R337 gives a moved pane a fresh leaf, so its `usage_usec` restarts at zero. A saturating
    /// subtraction would report 0 millicores for a pane somebody had just moved BECAUSE it was busy
    /// — the one answer that is certainly wrong — and the next sample would then show a spike, since
    /// the new leaf's whole usage would be attributed to one short window.
    #[test]
    fn a_pane_whose_counter_restarted_has_no_rate_rather_than_a_zero() {
        let then = Instant::now();
        let baseline = Baseline {
            at: then,
            cpu_usec: 9_000_000,
        };

        assert_eq!(
            rate(
                Some(baseline),
                &charge(12_000),
                then + Duration::from_secs(1)
            ),
            Cpu::Settling,
        );
    }

    /// ...and it takes the fresh sample as its baseline immediately, rather than waiting out
    /// [`SETTLE`] against a cgroup the pane has left.
    #[test]
    fn a_restarted_counter_rebaselines_at_once() {
        let then = Instant::now();
        let now = then + Duration::from_millis(10);
        let rolled = roll(
            Some(Baseline {
                at: then,
                cpu_usec: 9_000_000,
            }),
            &charge(12_000),
            now,
        );

        assert_eq!(rolled.cpu_usec, 12_000);
        assert_eq!(rolled.at, now);
    }

    /// A baseline is kept until it has aged past [`SETTLE`], so a fast poller's windows GROW
    /// instead of being pinned to its own cadence.
    ///
    /// Both halves are asserted, because keeping one forever and replacing one every time are both
    /// wrong and each passes half of this.
    #[test]
    fn a_baseline_is_held_until_it_ages_out_and_then_replaced() {
        let then = Instant::now();
        let old = Baseline {
            at: then,
            cpu_usec: 100,
        };

        let early = roll(Some(old), &charge(200), then + SETTLE / 2);
        assert_eq!(early.cpu_usec, 100, "a young baseline is kept");

        let late = roll(Some(old), &charge(200), then + SETTLE);
        assert_eq!(late.cpu_usec, 200, "an aged baseline is replaced");
    }

    /// The rate is stated over the window it actually covers, not over [`SETTLE`].
    ///
    /// A caller polling faster than the baseline ages out gets a real window, and the number says
    /// which one — the difference between "this pane held 4 cores for the last 40 ms" and "for the
    /// last minute" is the difference between a burst and a runaway.
    #[test]
    fn the_window_a_rate_covers_is_the_one_it_is_stated_over() {
        let then = Instant::now();
        let baseline = Baseline {
            at: then,
            cpu_usec: 0,
        };

        assert_eq!(
            rate(
                Some(baseline),
                &charge(40_000),
                then + Duration::from_millis(40)
            ),
            Cpu::Held {
                millicores: 1000,
                over_ms: 40,
            }
        );
    }
}
