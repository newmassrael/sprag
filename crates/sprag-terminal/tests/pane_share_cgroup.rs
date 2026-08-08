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

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sprag_terminal::registry::{SessionId, WindowId};
use sprag_terminal::share::{PaneLineage, Placement, Share, Tree};
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
    let Some(scope) = DelegatedScope::acquire() else {
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
    /// Start one busy loop, pinned, and move it into `cgroup`.
    fn add(&mut self, cgroup: &Placement) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "while :; do :; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: the closure runs between `fork` and `exec` in the child and calls one
        // async-signal-safe syscall, allocating nothing.
        unsafe {
            command.pre_exec(pin_to_one_cpu);
        }
        let child = command.spawn().expect("spawn a spinner");
        cgroup.join(child.id()).expect("join the spinner's cgroup");
        self.running.push(child);
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
    /// Ask systemd for one, or say on stderr which precondition was missing and answer `None`.
    fn acquire() -> Option<Self> {
        let unit = format!("sprag-share-gate-{}", std::process::id());
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
