//! **A pane the kernel will not confine is still a pane.**
//!
//! [`AttachedPty::spawn`](sprag_terminal::pty::AttachedPty::spawn) hands the child an open
//! `cgroup.procs` and the
//! child writes itself into it between `fork` and `exec` (R336). That write can be REFUSED by a
//! kernel that let the file be opened, and the two events are not the same check: opening tests the
//! permissions on one file, while the write runs cgroup v2's delegation containment rule, which
//! compares the *writer's own cgroup* against the destination and so cannot be predicted from the
//! destination alone.
//!
//! Until this gate existed a refused write came back out of `pre_exec` as an `Err`, which
//! `std::process::Command` turns into a failed spawn — so the pane never opened. Measured on
//! GitHub's Linux runner at `dc35614`, where the daemon's own cgroup is outside the subtree systemd
//! delegated to it: **11 panes across two test binaries failed to be born**, every one of them with
//! `spawn command: Permission denied (os error 13)`.
//!
//! That is the wrong trade twice over. `PaneHomes::open`'s own contract already says so — *"Never
//! fails a birth. A pane that could not be placed runs unweighted, which is what every pane did
//! before this existed; refusing to open it would trade a missing guarantee for a missing
//! terminal"* — and it enforced that contract at the OPEN, which is the check that cannot fail on
//! the hosts where the WRITE does.
//!
//! # Why the refusal is manufactured with `/dev/full` and not with a cgroup
//!
//! The claim under test is *what sprag does when the write is refused*, not *which errno a kernel
//! chooses*. A fixture that needed a host where a cgroup join is genuinely refused would be a
//! fixture that skips on every developer machine and runs only where the bug was found — the
//! opposite of what a regression gate is for. `/dev/full` is a write that always fails, on every
//! Linux, with no privileges and no cgroup tree, so this gate discriminates HERE.
//!
//! Measured, both ways: the real refusals were `EACCES` (CI) and this one is `ENOSPC`, and the code
//! under test does not branch on the number.

#![cfg(target_os = "linux")]

use std::os::fd::AsFd;

use sprag_terminal::CommandBuilder;
use sprag_terminal::pty::{AttachedPty, Joined, Pty};

/// A pty with a reader on it that drains and discards.
///
/// These gates are about the cgroup ANSWER and never read a byte of output — but a child is still
/// only born onto a terminal something is reading, because that ordering is the one thing standing
/// between a fast child and a kernel that discards what nobody has collected
/// (`Pty::attach_reader`). Saying so with a sink is honest; there is no spelling that says "nobody
/// reads this", and there should not be.
fn read_and_discarded(pty: Pty) -> AttachedPty {
    pty.attach_reader("join-refused-drain", |mut terminal| {
        let _ = std::io::copy(&mut terminal, &mut std::io::sink());
    })
    .expect("a fresh pty takes a reader")
}

/// A descriptor that opens for writing and refuses every write — what a `cgroup.procs` the kernel
/// will not migrate into looks like from the child's side.
fn a_write_that_will_be_refused() -> std::fs::File {
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full is a Linux device node, present on every host this runs on")
}

/// THE GATE: a refused join costs the pane its cgroup and NOT its existence.
#[test]
fn a_pane_whose_cgroup_the_kernel_refuses_is_still_born() {
    let mut pty = read_and_discarded(Pty::open(80, 24).expect("open a pty"));
    let refusing = a_write_that_will_be_refused();

    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    let (mut child, joined) = pty
        .spawn(&command, Some(refusing.as_fd()))
        .expect("a refused cgroup join must not cost the person their pane");

    // The pane is REALLY there — asserted by the child being alive and reapable, not by the `Ok`
    // above. A spawn that answered `Ok` with a child that had already died of the same error would
    // pass a gate that only read the result.
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "the child is running, not already dead",
    );

    // And the daemon KNOWS, because a pane running unconfined while its row claims a share is the
    // silent half of this defect: the answer says which of the three things happened, so a caller
    // can tell "this host enforces nothing" from "this pane in particular did not get in".
    assert!(
        matches!(joined, Joined::Refused(_)),
        "the join was refused and the answer says so, not {joined:?}",
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// The CONTROL, and the one that keeps the gate above from passing for the wrong reason: a pane
/// offered a cgroup it CAN join reports that it joined.
///
/// Measured, which is the only reason this says what it says: a reader hard-coded to `Refused`
/// leaves the gate above GREEN — it asks only that a refusal be reported and would then be told so
/// about every pane on every host. This is what fails. (The gate above is the one that catches the
/// join being deleted outright, also measured: with no write there is nothing to refuse.)
/// `/dev/null` accepts the write the same way a `cgroup.procs` does.
#[test]
fn a_pane_whose_cgroup_accepts_it_reports_that_it_joined() {
    let mut pty = read_and_discarded(Pty::open(80, 24).expect("open a pty"));
    let accepting = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .expect("/dev/null");

    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    let (mut child, joined) = pty.spawn(&command, Some(accepting.as_fd())).expect("spawn");

    assert!(
        matches!(joined, Joined::Joined),
        "an accepted write is an accepted join, not {joined:?}",
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A pane on a host with no cgroups at all is neither joined nor refused — the third value, and the
/// one every macOS pane and every unplaced pane has.
#[test]
fn a_pane_offered_no_cgroup_says_it_was_never_asked() {
    let mut pty = read_and_discarded(Pty::open(80, 24).expect("open a pty"));
    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    let (mut child, joined) = pty.spawn(&command, None).expect("spawn");

    assert!(
        matches!(joined, Joined::NotAsked),
        "no cgroup was offered, so there is nothing to have joined: {joined:?}",
    );

    let _ = child.kill();
    let _ = child.wait();
}
