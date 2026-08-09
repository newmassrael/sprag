//! The share a pane is granted, against a real kernel.
//!
//! Everything in `sprag_terminal::share`'s own unit tests runs on a directory of ordinary files, so
//! it can prove the SEQUENCE and never the effect. This proves the effect: two panes weighted alike
//! under one window, one of them running four spinners and the other one, all pinned to a single
//! CPU. Placed, they split that CPU evenly. The same processes with no tree around them split it by
//! thread count.
//!
//! | condition | four threads : one thread |
//! |---|---|
//! | placed under the tree | 50 : 50 |
//! | no tree (measured before this existed) | 80 : 20 |
//!
//! That gap is what makes this a gate rather than a smoke test: the failure mode this feature
//! exists to remove reproduces as 80:20, which is nowhere near the band asserted here.
//!
//! # Why it can skip, and why a skip says so
//!
//! It needs a systemd user manager willing to delegate a subtree. A container, a CI runner without
//! a session bus, a non-systemd host — none of those can give one, and none of them is a failure of
//! this crate. So the test SAYS which precondition was missing and returns. A silent skip and a
//! pass are the same colour, which is how a gate stops discriminating without anybody noticing.

#![cfg(target_os = "linux")]

use std::os::fd::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sprag_terminal::registry::{SessionId, WindowId};
use sprag_terminal::resources::Cpu;
use sprag_terminal::share::{Counted, PaneLineage, Percent, Placement, Share, Tree, Waiting};
use sprag_terminal::workspace::PaneId;

/// How long the two cgroups are left to accumulate CPU time.
///
/// Long enough that scheduler noise averages out, short enough that a developer running the suite
/// does not notice one core busy. The ratio being measured is 4:1 against 1:1, so the signal is
/// enormous and the window does not have to be.
const MEASURE: Duration = Duration::from_millis(2500);

/// The CPU every spinner is pinned to.
///
/// Without pinning there is no contention to measure at all: this machine has more cores than the
/// test has threads, so both cgroups would simply get everything they asked for and the test would
/// pass while proving nothing.
const PINNED_CPU: usize = 0;

/// The band the even split has to land in.
///
/// Wide, because it is separating 50 from 80 rather than 50 from 51 — a tight band would buy no
/// discrimination and would spend it all on flakes.
const EVEN_BAND: std::ops::RangeInclusive<u64> = 35..=65;

#[test]
fn two_panes_weighted_alike_split_a_cpu_evenly_however_many_threads_each_runs() {
    let Some(scope) = DelegatedScope::acquire("split") else {
        return;
    };

    let tree = Tree::adopt(scope.root.clone()).expect("adopt the delegated root");
    let busy = address(1, 1, 1);
    let quiet = address(1, 1, 2);
    let busy_cgroup = tree.place(busy, Share::EVEN).expect("place the busy pane");
    let quiet_cgroup = tree
        .place(quiet, Share::EVEN)
        .expect("place the quiet pane");

    // Two panes, two cgroups. Without this the whole measurement is vacuous: collapse both panes
    // onto one cgroup and the two readings become the same file, which reads as a perfect 50:50 and
    // asserts nothing at all.
    assert_ne!(busy_cgroup.path(), quiet_cgroup.path());

    let mut spinners = Spinners::default();
    for _ in 0..4 {
        spinners.add(&busy_cgroup);
    }
    spinners.add(&quiet_cgroup);

    let before = (usage(busy_cgroup.path()), usage(quiet_cgroup.path()));
    std::thread::sleep(MEASURE);
    let after = (usage(busy_cgroup.path()), usage(quiet_cgroup.path()));

    spinners.kill();
    let busy_usec = after.0 - before.0;
    let quiet_usec = after.1 - before.1;
    let total = busy_usec + quiet_usec;
    assert!(
        total > 0,
        "neither cgroup accumulated any CPU time — nothing was measured"
    );
    let busy_share = busy_usec * 100 / total;

    tree.release(busy).expect("release the busy pane");
    tree.release(quiet).expect("release the quiet pane");

    assert!(
        EVEN_BAND.contains(&busy_share),
        "four threads took {busy_share}% against one thread's {}%, which is the unplaced \
         thread-count split, not an even grant",
        100 - busy_share
    );
}

/// What a pane took, measured against a real kernel — the reading, not the setting.
///
/// # What this gate can say that the unit tests cannot
///
/// `share`'s own tests read a directory of ordinary files this repository wrote, so they prove the
/// PARSE against a fixture that agrees with the parse by construction. This reads the files the
/// kernel writes: a `cpu.stat` with the keys that kernel prints in the order it prints them, a
/// `cpu.pressure` from the live PSI accounting, and counters from controllers that were really
/// delegated. A units error, a key read as a prefix, or the `full` row taken for the `some` row all
/// survive the fixture and die here.
///
/// # The two panes are not the same experiment as the split above
///
/// That one asserts the GRANT is honoured. This asserts the READING is true: a pane holding one core
/// reads as one core, an idle pane beside it reads as nothing, and the pane whose two threads are
/// fighting over one CPU reports that it WAITED. The last is the number that makes the others
/// interpretable — 1000 millicores means something different when the pane wanted 2000.
#[test]
fn a_pane_that_holds_a_core_is_measured_holding_a_core_and_says_it_waited() {
    let Some(scope) = DelegatedScope::acquire("reading") else {
        return;
    };

    let tree = Tree::adopt(scope.root.clone()).expect("adopt the delegated root");
    let (busy, quiet) = (address(2, 1, 1), address(2, 1, 2));
    let busy_cgroup = tree.place(busy, Share::EVEN).expect("place the busy pane");
    tree.place(quiet, Share::EVEN)
        .expect("place the quiet pane");

    let mut spinners = Spinners::default();
    // TWO, both pinned to one CPU: the second is what makes the first WAIT for it. One spinner
    // would take the same CPU time and report no pressure at all.
    spinners.add(&busy_cgroup);
    spinners.add(&busy_cgroup);

    let before = (
        tree.charge(busy).expect("the busy pane has a leaf"),
        tree.charge(quiet).expect("the quiet pane has a leaf"),
    );
    let spent_before = spinners.cpu_usec();
    let started = std::time::Instant::now();
    std::thread::sleep(MEASURE);
    let after = (
        tree.charge(busy).expect("the busy pane still has a leaf"),
        tree.charge(quiet).expect("the quiet pane still has a leaf"),
    );
    let window = started.elapsed();
    let spent = spinners.cpu_usec() - spent_before;
    spinners.kill();

    let held = Cpu::over(window, before.0.cpu_usec, after.0.cpu_usec);
    let idle = Cpu::over(window, before.1.cpu_usec, after.1.cpu_usec);

    let Cpu::Held { millicores, .. } = held else {
        panic!("two samples a measured window apart produced no rate: {held:?}");
    };
    // AGAINST A SECOND SOURCE, never against a band of its own.
    //
    // The first version of this asserted "two threads on one CPU hold one core", and the parallel
    // suite refuted it at 498 millicores — the spinners share that CPU with whatever else the run
    // put there, so the absolute number is a fact about the machine's load and not about this
    // reader. Widening the band would have bought a gate that passes on the defect it exists to
    // catch. What is load-invariant is that two independent accountings of the same processes AGREE:
    // the kernel's per-cgroup `cpu.stat` and its per-process `/proc/<pid>/stat`. A units error, a
    // pane charged for its neighbour's work, and a key read as a prefix all break that agreement
    // whatever else is running.
    let expected = expected_millicores(spent, window);
    assert!(
        agree(millicores, expected),
        "the cgroup reading says {millicores} millicores and the spinners' own /proc accounting \
         says {expected} over the same {} ms — two accountings of the same processes disagree",
        window.as_millis(),
    );
    assert!(
        expected > 0,
        "the spinners burned no CPU at all, so this run compared two zeroes"
    );
    assert!(
        matches!(idle, Cpu::Held { millicores, .. } if millicores < 100),
        "the pane running nothing measured as {idle:?}, so the reading is not attributed per pane"
    );

    // The WAITING half. It is what separates a pane with nothing to do from a pane being starved,
    // and the pane above is being starved on purpose: two runnable threads, one CPU.
    match after.0.waiting {
        Waiting::Measured { avg10, .. } => assert!(
            avg10 > Percent::NONE,
            "two threads fighting over one CPU reported no waiting at all ({avg10}), so the `some` \
             row is not being read"
        ),
        // A kernel without PSI is a real host and not a failure. But "the reader answered nothing"
        // and "the kernel keeps nothing" are two different facts, and letting the first wear the
        // second's colour is how a broken parser passes as an unsupported host — so the KERNEL is
        // asked directly, and only its own silence excuses this one.
        Waiting::NotAccounted => {
            assert!(
                !Path::new("/proc/pressure/cpu").exists(),
                "the kernel keeps pressure accounting and this reading says it does not, so the \
                 `some` row is not being read at all"
            );
            eprintln!(
                "SKIP (half): this kernel keeps no pressure accounting, so waiting is unread"
            );
        }
    }

    // The other two counters, from the controllers this host delegated. A pane running two shells
    // holds processes and memory, and both are read from the leaf rather than from the machine.
    assert!(
        matches!(after.0.memory, Counted::Now(bytes) if bytes > 0),
        "a pane running two shells reported {:?} memory",
        after.0.memory
    );
    assert!(
        matches!(after.0.processes, Counted::Now(count) if count >= 2),
        "a pane running two shells reported {:?} processes",
        after.0.processes
    );

    tree.release(busy).expect("release the busy pane");
    tree.release(quiet).expect("release the quiet pane");
}

/// What the spinners' own `/proc` accounting implies, in millicores over `window`.
fn expected_millicores(spent_usec: u64, window: Duration) -> u64 {
    let window_usec = u64::try_from(window.as_micros()).expect("a window of sane length");
    spent_usec * 1000 / window_usec
}

/// Whether two accountings of the same processes agree.
///
/// A tenth, which is far wider than the two sources can honestly differ (the per-process figure is
/// quantised to the kernel's tick — at 100 Hz, 10 ms per process against a window of seconds) and
/// far narrower than any error worth catching: reading seconds for microseconds is off by a million,
/// reading a percentage by ten, and charging a pane for a neighbour's work by whatever that
/// neighbour is doing.
fn agree(measured: u64, expected: u64) -> bool {
    measured.abs_diff(expected) * 10 <= expected.max(measured)
}

fn address(session: u64, window: u64, pane: u64) -> PaneLineage {
    PaneLineage {
        session: SessionId(session),
        window: WindowId(window),
        pane: PaneId(pane),
    }
}

/// A cgroup's cumulative CPU time in microseconds, off the controller this feature turns on.
///
/// Read from `cpu.stat` rather than by summing `/proc/<pid>/stat` over the members, because the
/// kernel is already keeping exactly this number per cgroup and a sum over pids would race every
/// spinner's exit.
fn usage(cgroup: &Path) -> u64 {
    let body = std::fs::read_to_string(cgroup.join("cpu.stat")).expect("cpu.stat");
    body.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
        .expect("cpu.stat reports usage_usec")
}

/// The spinners a measurement runs, killed by PID and never by pattern.
#[derive(Default)]
struct Spinners {
    running: Vec<Child>,
}

impl Spinners {
    /// Start one busy loop, pinned, and born INSIDE `cgroup` — the production placement, not a move
    /// after the fact.
    ///
    /// # Why it joins in `pre_exec` rather than moving the process in afterwards
    ///
    /// Both put the process in the cgroup, and only one of them puts its MEMORY there. cgroup v2
    /// charges a page to the cgroup that first faulted it and does not re-charge on migration, so a
    /// process moved in after it started keeps every page it already had charged where it was born.
    /// Measured: a spinner moved in this way leaves `memory.current` reading **0** for a leaf
    /// holding two live shells — and under load it reproduced every other run, because the moved
    /// shell touches nothing new.
    ///
    /// That is not a quirk to work around. It is `sprag_terminal::pty`'s whole argument for the
    /// descriptor-in-`pre_exec` design, and a gate that placed its processes the other way would be
    /// measuring a shape this product deliberately does not ship. Moving this gate onto the shipped
    /// path left `Placement::join` — the move-afterwards method, documented as "the racy half" —
    /// with no caller anywhere, and R338 deleted it: the racy way in is now not merely discouraged
    /// but absent.
    fn add(&mut self, cgroup: &Placement) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "while :; do :; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let procs = cgroup
            .open_for_join()
            .expect("open the pane's cgroup.procs");
        // SAFETY: the closure runs between `fork` and `exec` in the child and calls two
        // async-signal-safe syscalls, allocating nothing. `"0"` is what the kernel reads as "the
        // process doing the writing", which is this child.
        unsafe {
            command.pre_exec(move || {
                pin_to_one_cpu()?;
                if libc::write(procs.as_raw_fd(), c"0".as_ptr().cast(), 1) != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn a spinner");
        self.running.push(child);
    }

    /// The CPU time these processes have used, from the kernel's PER-PROCESS accounting.
    ///
    /// The second source the reading is checked against. `/proc/<pid>/stat`'s `utime` and `stime`
    /// are fields 14 and 15 — counted from AFTER the last `)`, because a process's `comm` can
    /// contain spaces and parentheses and splitting the whole line puts every later field somewhere
    /// else. A spinner that has exited contributes what it had; the cgroup keeps that time too.
    fn cpu_usec(&self) -> u64 {
        // SAFETY: `sysconf` reads a system constant and touches nothing of ours.
        let ticks_per_second = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
            .expect("a sane clock tick rate");
        self.running
            .iter()
            .filter_map(|child| {
                let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).ok()?;
                let tail = stat.rsplit_once(')')?.1;
                let mut fields = tail.split_ascii_whitespace().skip(11);
                let utime: u64 = fields.next()?.parse().ok()?;
                let stime: u64 = fields.next()?.parse().ok()?;
                Some((utime + stime) * 1_000_000 / ticks_per_second)
            })
            .sum()
    }

    /// Kill every spinner by its own pid and reap it.
    ///
    /// By pid because this machine has other sessions running load rigs built from the very same
    /// shell idiom, and a pattern kill would take theirs down with ours.
    fn kill(&mut self) {
        for child in &mut self.running {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.running.clear();
    }
}

impl Drop for Spinners {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Confine this process to one CPU. Runs in the child, after `fork`, before `exec`.
fn pin_to_one_cpu() -> std::io::Result<()> {
    // SAFETY: `set` is zeroed before use and its size is passed explicitly; `sched_setaffinity` on
    // pid 0 addresses the calling thread, which after `fork` is the only one there is.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(PINNED_CPU, &mut set);
        if libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A transient systemd scope with `Delegate=yes` — a subtree this test may build a tree in.
struct DelegatedScope {
    /// The unit name, for teardown.
    unit: String,
    /// The process holding the scope open.
    holder: Child,
    /// The scope's cgroup directory.
    root: PathBuf,
}

impl DelegatedScope {
    /// Ask systemd for one of ITS OWN, or say on stderr which precondition was missing and answer
    /// `None`.
    ///
    /// # Why the tag is not decoration
    ///
    /// The name was `sprag-share-gate-<pid>` while this file held one test. R338 added a second, the
    /// two run concurrently in one binary, and the name they derived was therefore the SAME — so
    /// `systemd-run` failed for the second, the poll below found the unit anyway (the first test had
    /// just made it), and both tests measured inside one scope that either of them would stop on the
    /// way out. Under a 4x load it reproduced every time, as `cpu.stat: No such file or directory`
    /// in whichever test was still running: **a shared name is a shared teardown.** The tag is the
    /// test's own, so two acquisitions in one process cannot name one scope.
    fn acquire(tag: &str) -> Option<Self> {
        let unit = format!("sprag-share-gate-{}-{tag}", std::process::id());
        let holder = Command::new("systemd-run")
            .args([
                "--user",
                "--scope",
                "--quiet",
                "--property=Delegate=yes",
                &format!("--unit={unit}"),
                "sleep",
                "600",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(holder) = holder else {
            eprintln!("SKIP: systemd-run is not on PATH — no delegated cgroup to test against");
            return None;
        };

        // The unit exists only once systemd has answered, so the path is polled rather than assumed.
        for _ in 0..50 {
            if let Some(path) = control_group(&unit) {
                return Some(Self {
                    unit,
                    holder,
                    root: PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/')),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        eprintln!(
            "SKIP: systemd would not create a delegated scope — no user manager, no session bus, \
             or delegation refused"
        );
        let mut holder = holder;
        let _ = holder.kill();
        let _ = holder.wait();
        None
    }
}

impl Drop for DelegatedScope {
    fn drop(&mut self) {
        let _ = self.holder.kill();
        let _ = self.holder.wait();
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{}.scope", self.unit)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The cgroup path systemd gave a unit, if the unit is up yet.
fn control_group(unit: &str) -> Option<String> {
    let out = Command::new("systemctl")
        .args([
            "--user",
            "show",
            &format!("{unit}.scope"),
            "-p",
            "ControlGroup",
            "--value",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim().to_owned();
    (path.starts_with('/')).then_some(path)
}
