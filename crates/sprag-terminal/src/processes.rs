//! What a pane is RUNNING — its terminal device, the child the daemon spawned, and the FOREGROUND
//! JOB that currently owns its terminal, with every process in that job.
//!
//! # The question, and why nothing already answered it
//!
//! A pane's listing carries a `command` and it is the SPAWN label: the argv the daemon started the
//! pane with, stored at birth and never touched again. A pane opened as `bash` three hours ago and
//! now half way through a `cargo build` still lists as `bash`, because from the daemon's side
//! nothing happened — a shell running a command is bytes on a pty, not an event. The screen shows
//! it, until the output scrolls past; a silent build and an idle prompt look identical.
//!
//! What the OS knows, and the pane list cannot, is which process GROUP owns the pane's terminal.
//! That is a shell's answer to "what did the user start": it hands the terminal to the job it runs
//! and takes it back when the job ends. [`PanePty::foreground_pgid`](crate::PanePty::foreground_pgid)
//! has been deriving that number all along for one internal purpose (binding an agent report's
//! lifetime to the job that was running), and no reader — no client, no CLI verb, no agent tool —
//! could ask for it.
//!
//! # Why this is SAMPLED and not published
//!
//! The R282 table, applied to this fact:
//!
//! | | the registry's structure | this |
//! |---|---|---|
//! | what | name, windows, panes, the arrangement | which job owns each pane's terminal |
//! | changes on | an event this daemon performs | a user typing at a shell |
//! | so it is | published, and the scene revision carries it | SAMPLED, at some time, with some age |
//!
//! So it must not ride the pane list, which every attached client re-reads on every poll wake — the
//! exact mistake R282 measured at 3478 us a read and removed. It gets its own address with a
//! caller-declared staleness tolerance, through the same [`Sampled`] machinery.
//!
//! # One walk, every pane
//!
//! `/proc` has no index by process group, so enumerating ONE job costs a full pass over
//! `/proc/*/stat` — which is the same pass that answers every other pane. So a reading covers every
//! pane in the registry and a caller filters. That is a straight consequence of the medium, not an
//! optimisation: asking about one pane and asking about eight cost the same, and pretending
//! otherwise would mean paying eight times.
//!
//! Two of the three facts on each row do NOT age — the terminal device is fixed at the pane's birth
//! and the child pid until it is reaped — and they are carried here anyway rather than on the pane
//! list, because their only consumer is this question and the hot slot every client re-reads should
//! not grow a string per pane for it. Each says so in its own doc.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::registry::SessionRegistry;
use crate::sampled::Sampled;

/// What ONE pane is running.
///
/// Keyed by pane [`id`](Self::id) — registry-unique and never reused, so a caller joining this
/// against the pane list cannot pair one read's row with another's pane.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneProcesses {
    /// The pane this row describes.
    pub id: u64,
    /// The pane's TERMINAL DEVICE (`/dev/pts/7`), or `None` on a platform whose PTY backend names
    /// none. See [`PanePty::tty`](crate::PanePty::tty).
    ///
    /// It does NOT age with the rest of this row: it is fixed when the pane is born and the daemon
    /// holds it, so no sample is involved. It rides here because this is the question that wants it
    /// — it is the address every tool outside sprag calls this pane by (`ps -t pts/7`, a debugger's
    /// `--tty`), and a pane id is a name only this daemon knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The pid of the child the daemon SPAWNED for this pane — the shell, normally — or `None` once
    /// it has exited and been reaped.
    ///
    /// Also not a sampled fact. It is here as the anchor the rest of the row is derived from, and
    /// because it is what a person needs to signal the pane's own process rather than its job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_pid: Option<u32>,
    /// The job that currently OWNS this pane's terminal, or `None` when the pane has no live child,
    /// nothing owns the terminal, or the platform exposes no `/proc`.
    ///
    /// This is the sampled half, and the reading's age describes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ForegroundJob>,
}

/// The process group that owns a pane's terminal, and every process in it.
///
/// A GROUP rather than a process because that is what a shell hands the terminal to: `cargo build |
/// less` is one job of two processes, and naming either one alone would be a choice made for the
/// wrong reason. The group id is also the thing a caller can act on — it is what a `SIGINT` from the
/// keyboard goes to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForegroundJob {
    /// The process group id — the pane's `tpgid`, and the address of the whole job.
    pub pgid: u32,
    /// Every process in the group, by ascending pid.
    ///
    /// Found by INDEXING the whole process table by group, never by walking the pane child's
    /// descendants and filtering: a job's process that has been reparented away (its parent died,
    /// it kept the group) is no longer a descendant of anything the pane spawned, and a descendant
    /// walk simply loses it. Ascending pid because a job's members have no other order the OS
    /// offers, and a stable one keeps two readings comparable.
    pub processes: Vec<JobProcess>,
}

/// One process of a pane's foreground job.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JobProcess {
    /// The process id.
    pub pid: u32,
    /// The KERNEL's name for it (`/proc/<pid>/stat`'s `comm`) — always present, capped at 15 bytes,
    /// and settable by the process itself. A name to show a person, never an identity to match on.
    ///
    /// Carried BESIDE [`argv`](Self::argv) rather than derived from it because the two have
    /// different sources and can honestly disagree: a process that rewrote its own argv still has
    /// its kernel name, and a process with no argv at all still has one.
    pub name: String,
    /// The process's own ARGUMENTS, from `/proc/<pid>/cmdline`.
    ///
    /// EMPTY IS A FACT, not a failure: a kernel thread has no argv, and neither does a zombie (the
    /// kernel releases that memory at exit while the entry lives on until it is reaped). A reader
    /// that wants a display line joins these itself and knows it is doing so; this carries no
    /// pre-joined string, because joining argv with spaces makes an argument that contains a space
    /// indistinguishable from two arguments, and publishing both forms invites a consumer to read
    /// the same fact two ways and get two answers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
}

/// The LEADER of the foreground job on `pid`'s controlling terminal — the process a shell hands
/// the terminal to when it runs something, and takes back from when that thing ends.
///
/// # Why this exists BESIDE the sampler, rather than as a call into it
///
/// [`PaneProcessSampler`] answers *what is every pane running* and pays one full pass over the
/// process table to do it, because a job's members can only be found by indexing the whole table by
/// group. That is the right cost for a question asked once per client poll about eight panes.
///
/// It is the wrong cost for a PREDICATE. A plugin's readiness barrier (`ReadyWhen::Runs`, in
/// `sprag-plugin` — named rather than linked, because this crate is BELOW that one and a link
/// upward would invert the dependency it documents) asks *does this one pane's terminal belong to
/// `claude` yet* every 10 ms for up to two minutes; answering that with a table walk would be some
/// thousands of passes over every process on the box to learn one name.
///
/// So this reads the LEADER only, in two `stat`-sized reads and no walk at all: a process group's
/// id IS its leader's pid, which is the whole reason the number is addressable. A shell running
/// `claude` makes `claude` that leader; `cargo build | less` makes `cargo` it.
///
/// # What it therefore does NOT answer
///
/// * **Not every process in the job** — `less` in that pipeline is a member, not the leader, and
///   nothing here will name it. A caller who needs the membership wants the sampler, which is why
///   both exist and why this returns the same [`JobProcess`] type rather than a second one.
/// * **`None` when the leader has already exited but the group lives on** (its other members keep
///   it), and when the platform exposes no process table at all. Both are absences a caller must
///   already handle — the same honest `None` [`crate::foreground_pgid_of`] answers.
///
/// ⚠ **NOT PLATFORM-GATED, and that is load-bearing rather than incidental.** Everything here goes
/// through [`crate::procfs`], which reads `/proc` on Linux and `proc_pidinfo`/`KERN_PROCARGS2` on
/// macOS. A readiness condition that silently never fired on one of the two platforms sprag builds
/// for would be worse than the defect it was written to remove.
#[must_use]
pub fn foreground_leader_of(pid: u32) -> Option<JobProcess> {
    let pgid = crate::pane_pty::foreground_pgid_of(pid)?;
    let stat = crate::procfs::stat(pgid)?;
    Some(JobProcess {
        pid: pgid,
        name: stat.comm,
        argv: argv(pgid),
    })
}

/// A whole [`PaneProcessSampler`] reading: every pane's processes, and how old the reading is.
///
/// One age for the whole reading because one `/proc` pass produces it all — see
/// [`Reading`](crate::Reading).
pub type PaneProcessReading = crate::Reading<Vec<PaneProcesses>>;

/// The one place a pane's [processes](PaneProcesses) are sampled, and the one place a sample is held
/// between reads.
///
/// Shared (`Arc`) by every arm that serves the question, so two readers can neither disagree about
/// what a field means nor pay twice for the same walk. A named wrapper over [`Sampled`] for the
/// reason [`ActivitySampler`](crate::ActivitySampler) is one: the generic owns the cache and the
/// coalescing, this owns the question.
///
/// # Locking
///
/// Sampler → registry → pool, the crate's one direction. The sampler's lock is held across the
/// sample (that IS the coalescing); the registry lock is taken and RELEASED before any pool lock,
/// and the pools are locked one at a time, never nested.
#[derive(Default)]
pub struct PaneProcessSampler {
    held: Sampled<Vec<PaneProcesses>>,
}

impl PaneProcessSampler {
    /// A sampler holding nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every pane's processes, no older than `max_age`, with the age it actually has. See
    /// [`Sampled::read`] for what that tolerance does and does not do.
    pub fn read(
        &self,
        registry: &Arc<Mutex<SessionRegistry>>,
        max_age: Duration,
    ) -> PaneProcessReading {
        self.held.read(max_age, || sample(registry))
    }
}

/// One pane's identity as the pool knows it, gathered under the pool lock so the expensive half can
/// run with every lock released.
struct PaneAnchor {
    id: u64,
    pid: Option<u32>,
    tty: Option<String>,
}

/// Take one reading of every pane's processes.
///
/// THREE PHASES, following the crate's registry-then-pool, never-nested discipline:
///  1. under the registry lock: every window's pool, and nothing else;
///  2. lock RELEASED — each pool locked on its own for its panes' ids, child pids and devices;
///  3. no lock at all — ONE `/proc` pass, indexed by process group, joined onto those anchors.
///
/// Each anchor's pid comes from [`PanePty::pid`](crate::PanePty::pid), so it belongs to a child that
/// has not been reaped as of that read — the same gate every other `/proc` consumer in this crate
/// uses, and with the same narrow residue: the reader thread publishes the exit AFTER waiting, so a
/// pid read a moment before that publication can in principle be recycled before the walk reaches
/// it. Re-checking the gate after the walk would narrow the window without closing it (the
/// publication still trails the reap), so it is stated here rather than papered over with a check
/// that cannot make the claim it looks like it makes.
fn sample(registry: &Arc<Mutex<SessionRegistry>>) -> Vec<PaneProcesses> {
    let pools: Vec<_> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.window_pools().into_iter().flatten().collect()
    };
    let anchors: Vec<PaneAnchor> = pools
        .iter()
        .flat_map(|pool| {
            let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            pool.panes()
                .iter()
                .map(|pane| PaneAnchor {
                    id: pane.id().0,
                    pid: pane.pty().pid(),
                    tty: pane
                        .pty()
                        .tty()
                        .map(|tty| tty.to_string_lossy().into_owned()),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // ONLY when some pane actually holds a live child. An idle daemon then pays no `/proc` walk even
    // for a caller that demanded a fresh sample; with no pid to anchor on the walk could not
    // attribute anything anyway, so the skip changes the cost and not the answer.
    let table = if anchors.iter().any(|anchor| anchor.pid.is_some()) {
        ProcessTable::read()
    } else {
        ProcessTable::default()
    };
    anchors
        .into_iter()
        .map(|anchor| PaneProcesses {
            foreground: anchor.pid.and_then(|pid| table.foreground_job(pid)),
            id: anchor.id,
            tty: anchor.tty,
            shell_pid: anchor.pid,
        })
        .collect()
}

/// One `/proc` pass, indexed for this question: every process's stat row, plus the members of every
/// process GROUP.
///
/// Built once per reading and shared across every pane, so a box with eight panes costs one pass
/// rather than eight. The group index is the whole point — it is what makes a job's REPARENTED
/// member (its parent died, it kept the group) still a member, which no walk of the pane child's
/// descendants can say.
#[derive(Default)]
struct ProcessTable {
    /// Every process's stat row, by pid — read to find a pane child's `tpgid`.
    by_pid: HashMap<u32, crate::procfs::Stat>,
    /// Process group id → its members' pids, ascending.
    by_group: HashMap<u32, Vec<u32>>,
}

impl ProcessTable {
    /// Read and index `/proc`. Empty off Linux, where the honest answer to every question below is
    /// an absence rather than a guess — the same choice [`crate::ports`] and
    /// [`crate::pane_pty`] make.
    ///
    /// ONE body for every platform, because the platform difference is one level down:
    /// [`crate::procfs::walk`] answers with no processes off Linux, so this indexes nothing and
    /// [`foreground_job`](Self::foreground_job) reports nothing, which is exactly what the
    /// hand-written non-Linux arm here used to do. A `cfg` that only restates what its callee
    /// already guarantees is a second place for the two platforms to drift apart.
    fn read() -> Self {
        let mut by_pid = HashMap::new();
        let mut by_group: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, stat) in crate::procfs::walk() {
            by_group.entry(stat.pgrp).or_default().push(pid);
            by_pid.insert(pid, stat);
        }
        for members in by_group.values_mut() {
            members.sort_unstable();
        }
        Self { by_pid, by_group }
    }

    /// The job owning the terminal of the process `pid` — its `tpgid`, and every process in that
    /// group. `None` when the process is gone, has no controlling terminal, or the group has no
    /// live member left.
    fn foreground_job(&self, pid: u32) -> Option<ForegroundJob> {
        let pgid = self.by_pid.get(&pid)?.tpgid?;
        let processes: Vec<JobProcess> = self
            .by_group
            .get(&pgid)?
            .iter()
            .filter_map(|&member| {
                Some(JobProcess {
                    pid: member,
                    name: self.by_pid.get(&member)?.comm.clone(),
                    argv: argv(member),
                })
            })
            .collect();
        // A group with no member left is an absence, not an empty job: the job ENDED between the
        // walk and now, and reporting `pgid` with nothing in it would state that something is
        // running there.
        (!processes.is_empty()).then_some(ForegroundJob { pgid, processes })
    }
}

/// A process's ARGUMENTS as it reports them, from `/proc/<pid>/cmdline`.
///
/// Read per group MEMBER rather than in the shared walk: a foreground job is a handful of processes
/// and the box is thousands, so reading argv for every process on the machine to serve a few panes
/// would be paying a thousandfold for nothing.
#[cfg(target_os = "linux")]
fn argv(pid: u32) -> Vec<String> {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    split_cmdline(&raw)
}

/// macOS: the same arguments, from `sysctl(KERN_PROCARGS2)`.
///
/// # The payload's shape, which is not `/proc/<pid>/cmdline`'s
///
/// The kernel answers one buffer: a 4-byte `argc`, then the **executable path**, then a run of
/// padding NULs, then `argc` NUL-terminated arguments, then the environment. So unlike Linux —
/// where the file IS the argument vector — two things have to be stepped over before the arguments
/// begin, and the count is what says where they end. Taking the first `argc` strings after the
/// padding is therefore not a heuristic: the environment that follows is excluded BY the count.
///
/// `argv[0]` is the program as the process was invoked with it, which is what
/// [`JobProcess::argv`]'s Linux reader also yields — the executable path before it is the kernel's
/// own resolution and is deliberately dropped, or a person would read the same program twice.
///
/// Lossy-decoded and short-answer-tolerant for [`split_cmdline`]'s reasons: an argument is
/// arbitrary bytes, and a process that exits mid-read must produce an empty argv rather than
/// delete the process from its job.
#[cfg(target_os = "macos")]
fn argv(pid: u32) -> Vec<String> {
    crate::procfs::argv(pid)
}

/// Neither `/proc` nor `sysctl`, so no arguments — and the empty here means the same thing it means
/// for a zombie, which is why [`JobProcess::argv`] documents empty as a fact.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn argv(_pid: u32) -> Vec<String> {
    Vec::new()
}

/// Split a `/proc/<pid>/cmdline` payload into arguments.
///
/// The file is NUL-SEPARATED and NUL-TERMINATED, so only the terminator is stripped — an argument
/// that is itself the empty string is KEPT. Filtering empties instead would silently re-write
/// somebody's command line (`sh -c '' foo` has three arguments, not two), which is the same class of
/// lie as reporting argv pre-joined by spaces.
///
/// Lossy-decoded: an argument is arbitrary bytes, and one that will not decode must still be shown
/// rather than delete the process from the job.
///
/// ⚠ COMPILED AND TESTED EVERYWHERE, CALLED ON LINUX — the `allow` says so rather than a `cfg`
/// hiding it. Gating the parser by platform is the mistake `procfs`'s module docs describe: a byte
/// layout is not a syscall, and a parser only one runner builds is one only that runner can catch a
/// mistake in. Its macOS peer lives in [`crate::procfs`], beside the sysctl that fetches what it
/// parses — one read there answers both the arguments and the environment.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn split_cmdline(raw: &[u8]) -> Vec<String> {
    let body = raw.strip_suffix(b"\0").unwrap_or(raw);
    if body.is_empty() {
        return Vec::new();
    }
    body.split(|&b| b == 0)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandBuilder;

    /// The command line keeps every argument the process actually has, INCLUDING an empty one, and
    /// drops only the file's own terminator.
    #[test]
    fn a_command_line_keeps_its_empty_arguments_and_drops_only_the_terminator() {
        assert_eq!(
            split_cmdline(b"sh\0-c\0\0echo hi\0"),
            vec!["sh", "-c", "", "echo hi"],
            "the empty argument in the middle survives",
        );
        assert_eq!(
            split_cmdline(b"cargo\0build\0"),
            vec!["cargo", "build"],
            "no trailing empty from the terminator",
        );
        assert_eq!(
            split_cmdline(b""),
            Vec::<String>::new(),
            "a kernel thread or a zombie has no arguments at all",
        );
        assert_eq!(split_cmdline(b"\0"), Vec::<String>::new());
    }

    /// A registry with no session at all costs nothing and reports nothing — the idle daemon case,
    /// where a `/proc` walk would be pure waste.
    #[test]
    fn an_empty_registry_samples_nothing() {
        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let reading = PaneProcessSampler::new().read(&registry, Duration::ZERO);
        assert!(reading.value.is_empty());
        assert_eq!(reading.age, Duration::ZERO);
    }

    /// THE ANSWER, end to end against a real shell: the row names the pane, carries its device and
    /// its child, and its foreground job is the COMMAND the user ran — not the shell the pane was
    /// spawned as.
    ///
    /// Read TWICE with the input changed, because one reading cannot tell "the foreground job" from
    /// "the child": at a prompt they are the same group, and the entire reason this exists is the
    /// case where they are not. So the shell is sampled at rest first, and then while a job it
    /// started holds the terminal.
    // Linux AND macOS now: these drive the foreground-job reader end to end against a real
    // shell, and that reader stopped being Linux-only when `procfs` learned `proc_pidinfo`
    // (R343). A gate left on a test after its subject became portable is a claim that the
    // subject is not — and the platform this was hiding from is the one it had never run on.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_pane_reports_the_command_its_shell_is_running() {
        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = {
            let reg = registry.lock().unwrap();
            let name = reg.default_session().name().to_owned();
            reg.workspace_of(&name).expect("the default session's pool")
        };
        let pane = {
            let mut command = CommandBuilder::new("/bin/bash");
            command.arg("--norc");
            command.arg("-i");
            command.env("TERM", "dumb");
            command.env("PS1", "$ ");
            pool.lock()
                .unwrap()
                .spawn(command, "bash".into(), 40, 8)
                .expect("spawn a pane")
        };
        let sampler = PaneProcessSampler::new();
        let row = |reading: PaneProcessReading| {
            reading
                .value
                .into_iter()
                .find(|row| row.id == pane.0)
                .expect("the pane has a row")
        };

        let at_rest = settle(&sampler, &registry, |row| {
            row.foreground
                .as_ref()
                .is_some_and(|job| job.processes.iter().any(|p| p.name == "bash"))
        })
        .map(row)
        .expect("the shell owns its own terminal at a prompt");
        assert!(
            // `/dev/` and not `/dev/pts/`: the SPELLING of a pty slave is the platform's
            // (Linux `/dev/pts/7`, macOS `/dev/ttys007`), so pinning one made this a claim
            // about a kernel's naming while reading as a claim about the row. R343 fixed the
            // same assertion in `cli.rs` and did NOT grep for its siblings — this is one.
            at_rest.tty.is_some_and(|tty| tty.starts_with("/dev/")),
            "the row carries the pane's device",
        );
        assert_eq!(
            at_rest.shell_pid,
            at_rest.foreground.as_ref().map(|job| job.pgid),
            "at a prompt the foreground job IS the pane's own child",
        );

        pool.lock()
            .unwrap()
            .pane(pane)
            .expect("the pane is still pooled")
            .pty()
            .write(b"sleep 300\n")
            .expect("type a command into the pane");
        let running = settle(&sampler, &registry, |row| {
            row.foreground
                .as_ref()
                .is_some_and(|job| job.processes.iter().any(|p| p.name == "sleep"))
        })
        .map(row)
        .expect("the job the user started takes the terminal");
        let job = running.foreground.expect("a job owns the terminal");
        assert_ne!(
            Some(job.pgid),
            running.shell_pid,
            "and it is a DIFFERENT group from the pane's child — the case a pane list cannot see",
        );
        let sleep = job
            .processes
            .iter()
            .find(|p| p.name == "sleep")
            .expect("the job holds the command");
        assert_eq!(
            sleep.argv,
            vec!["sleep", "300"],
            "with the arguments the user typed, unjoined",
        );

        // ⚠⚠ **TWO READERS OF ONE FACT, AND THEY MUST AGREE.** [`foreground_leader_of`] answers
        // *what owns this pane's terminal* in two `stat`-sized reads because a readiness barrier
        // polls it every 10 ms; the sampler above answers the same question with a full pass over
        // the process table because it also wants the job's MEMBERSHIP. Two routes to one fact is
        // the R347 shape — the one where nothing compares them and they drift until a caller gets
        // two answers from the same daemon about the same pane.
        //
        // Asserted HERE rather than in a gate of its own, because this fixture has already paid to
        // build the state that makes the question interesting: a job that is NOT the pane's child.
        let leader = foreground_leader_of(running.shell_pid.expect("a live child"))
            .expect("the cheap reader answers wherever the sampler does");
        assert_eq!(
            leader.pid, job.pgid,
            "the leader the barrier reads IS the group the sampler reports — a process group's id \
             is its leader's pid, which is the whole reason one `stat` can stand in for the walk",
        );
        assert!(
            job.processes
                .iter()
                .any(|p| p.pid == leader.pid && p.name == leader.name && p.argv == leader.argv),
            "and it is one of the members the sampler found, named and argv'd identically: \
             {leader:?} is not in {:?}",
            job.processes,
        );
    }

    /// THE DISCRIMINATOR between indexing the process table by GROUP and walking the pane child's
    /// DESCENDANTS — and it is checked against BOTH answers, in this one test.
    ///
    /// A job's member can stop being a descendant of anything the pane spawned while staying in the
    /// job: its parent exits, it is reparented to init, and it keeps its process group. That is not
    /// exotic — `cmd &` inside a `sh -c` that then returns produces it — and the terminal still
    /// belongs to that group, so the process is still what the pane is running.
    ///
    /// The fixture builds exactly that state and then computes what a descendant walk WOULD have
    /// found, so the assertion is a comparison rather than a claim: the descendant set is a strict
    /// subset, missing the reparented member, and the group index has it.
    ///
    /// `sh -c 'sh -c "sleep 300 &"; exec sleep 400'` in the pane:
    ///  * the outer `sh` is the job's group leader, and `exec` keeps that pid as `sleep 400`;
    ///  * the inner `sh` starts `sleep 300` in the same group (no job control inside `sh -c`) and
    ///    exits, so `sleep 300` is reparented away while the group lives on through the leader.
    // Linux AND macOS now: these drive the foreground-job reader end to end against a real
    // shell, and that reader stopped being Linux-only when `procfs` learned `proc_pidinfo`
    // (R343). A gate left on a test after its subject became portable is a claim that the
    // subject is not — and the platform this was hiding from is the one it had never run on.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_reparented_member_of_the_job_is_still_in_it() {
        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = {
            let reg = registry.lock().unwrap();
            let name = reg.default_session().name().to_owned();
            reg.workspace_of(&name).expect("the default session's pool")
        };
        let pane = {
            let mut command = CommandBuilder::new("/bin/bash");
            command.arg("--norc");
            command.arg("-i");
            command.env("TERM", "dumb");
            command.env("PS1", "$ ");
            pool.lock()
                .unwrap()
                .spawn(command, "bash".into(), 40, 8)
                .expect("spawn a pane")
        };
        let sampler = PaneProcessSampler::new();
        // Wait for the shell's own prompt first, so the command is typed at one rather than raced
        // against bash's startup.
        settle(&sampler, &registry, |row| {
            row.id == pane.0 && row.foreground.is_some()
        })
        .expect("the shell reaches a prompt");
        pool.lock()
            .unwrap()
            .pane(pane)
            .expect("the pane is still pooled")
            .pty()
            .write(b"sh -c 'sh -c \"sleep 300 &\"; exec sleep 400'\n")
            .expect("type the job into the pane");

        let reading = settle(&sampler, &registry, |row| {
            row.id == pane.0
                && row.foreground.as_ref().is_some_and(|job| {
                    job.processes.iter().filter(|p| p.name == "sleep").count() == 2
                })
        })
        .expect("both sleeps join the pane's foreground job");
        let row = reading
            .value
            .into_iter()
            .find(|row| row.id == pane.0)
            .expect("the pane has a row");
        let shell = row.shell_pid.expect("a live child");
        let job = row.foreground.expect("a job owns the terminal");

        // What a DESCENDANT walk would have found: the pane child's subtree, intersected with the
        // group — herdr's shape (`process_tree_pids` from the child, filtered by pgid) computed
        // here from the same `/proc` the row came from, so the two answers are comparable.
        let table: HashMap<u32, u32> = crate::procfs::walk()
            .into_iter()
            .map(|(pid, stat)| (pid, stat.ppid))
            .collect();
        let descends_from_shell = |mut pid: u32| {
            for _ in 0..64 {
                if pid == shell {
                    return true;
                }
                match table.get(&pid) {
                    Some(&parent) if parent > 1 => pid = parent,
                    _ => return false,
                }
            }
            false
        };
        let by_descent: Vec<u32> = job
            .processes
            .iter()
            .map(|p| p.pid)
            .filter(|&pid| descends_from_shell(pid))
            .collect();
        let by_group: Vec<u32> = job.processes.iter().map(|p| p.pid).collect();

        assert_eq!(by_group.len(), 2, "the group holds both sleeps: {job:?}");
        assert_eq!(
            by_descent.len(),
            1,
            "and exactly one of them is still a descendant of the pane's child — \
             a descendant walk would report a one-process job here: {job:?}",
        );
        let reparented = job
            .processes
            .iter()
            .find(|p| !by_descent.contains(&p.pid))
            .expect("the reparented member");
        assert_eq!(
            reparented.argv,
            vec!["sleep", "300"],
            "the member the descendant walk loses is the backgrounded one",
        );
    }

    /// Poll a fresh sample until `ready`, or give up — a real shell reaches its prompt and starts a
    /// job on its own schedule, and `Duration::ZERO` is what makes each read a genuinely new walk.
    // Linux AND macOS now: these drive the foreground-job reader end to end against a real
    // shell, and that reader stopped being Linux-only when `procfs` learned `proc_pidinfo`
    // (R343). A gate left on a test after its subject became portable is a claim that the
    // subject is not — and the platform this was hiding from is the one it had never run on.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn settle(
        sampler: &PaneProcessSampler,
        registry: &Arc<Mutex<SessionRegistry>>,
        ready: impl Fn(&PaneProcesses) -> bool,
    ) -> Option<PaneProcessReading> {
        for _ in 0..200 {
            let reading = sampler.read(registry, Duration::ZERO);
            if reading.value.iter().any(&ready) {
                return Some(reading);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }
}
