//! **ONE CLOSED PANE STOPS EVERY WRITE TO IT, FOR EVER — BUT ONLY IF THE BYTES END A LINE.** The
//! defect that held a build machine for 43 hours, measured here rather than read off a backtrace.
//!
//! `write_shared` (`pane_pty.rs`) takes the pane's shared writer mutex and holds it across a
//! BLOCKING `write(2)` to the pty master. Bytes written to a master land in the SLAVE's input
//! queue; with the child dead nobody drains that queue, so once it is full the write blocks — and a
//! master *read* gets `EIO` where a *write* does not, which is why nothing here ever errors.
//!
//! ⚠ **This gate measures the WEDGE, not its mechanism.** Whether a second writer is held by the
//! mutex or by the same full queue is a distinction it cannot draw from outside, and the assertion
//! below says so, because the answer decides whether releasing the lock would be a fix at all.
//!
//! # ⚠⚠⚠ THE DIAGNOSIS IS A CONJUNCTION, and the first fixture written here proved it by failing
//!
//! *"The child is dead"* is not enough. The first draft wrote chunks of `x` with no line
//! terminator and **took a megabyte in 0.09 s without ever blocking** — because a cooked tty in
//! canonical mode will not hold a line it has no end for, so a dead pane is a hole rather than a
//! wall. Add the newline and the same pane wedges after ~17 KiB. Both arms are below, because a
//! gate that only knew the wedging one would have called *"a dead pane blocks"* the rule and been
//! wrong about the commoner case.
//!
//! ⚠⚠ **AND THE CONJUNCTION IS WHY AN AGENT LOOP IS THE VICTIM.** Everything a loop injects is a
//! prompt followed by Enter. The arm that wedges is the arm this product is made of.
//!
//! # ⚠⚠⚠ What this gate is FOR, and what it will mean when it goes red
//!
//! It asserts the DEFECT, on purpose, because nothing in this workspace had measured it: the whole
//! record of it was one peer session's gdb backtraces and a preserved core (register items 304,
//! 309, 310). **When the wedge is fixed this gate goes red, and the answer is to repurpose it** —
//! the threshold it measures and the second writer it strands are exactly the two facts a fix has
//! to change, and both are printed rather than spelled.
//!
//! # ⚠⚠ Why it MEASURES the threshold instead of naming it
//!
//! The hand-over reported ten workers queued behind a holder stuck in `write(fd=37, 1800 bytes)`,
//! at a queue that filled at 16,896 bytes. **That number is a kernel's, not sprag's** — this host
//! measures 17,664 for the same shape — and a constant would be folklore on the next machine, which
//! is item 310's own instruction.
//!
//! # ⚠⚠⚠ Why the workspace is deliberately LEAKED
//!
//! A blocked `write(2)` cannot be cancelled, so the two threads of the wedging arm never come back.
//! Dropping the workspace would close the pane, and closing a pane whose writer mutex is held for
//! ever is the unrecoverable join item 305 is about — **this gate would become the wedge it is
//! measuring.** The leak is one pty master and two parked threads for the life of the test binary,
//! which process exit reclaims; nothing joins them, and no other test can reach that pane.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use sprag_terminal::{CommandBuilder, Hand, PanePtyHandle, Workspace};

/// Bytes per write. Small enough to measure the threshold with some resolution, large enough that
/// filling a tty buffer is not thousands of syscalls.
const CHUNK: usize = 256;

/// The most either arm will write before deciding no wedge happened.
///
/// ⚠ For the wedging arm this is a **RED and not a pass**: a pane that swallows a megabyte of whole
/// lines with a dead child on the other side has either been fixed or has a reader nobody declared.
/// For the control arm it is the expected outcome, and the number is the point — see the module doc.
const GIVE_UP_AFTER: usize = 4096 * CHUNK;

/// No progress for this long means the writer is inside the kernel rather than merely slow. Two
/// orders of magnitude past a local pty write, which is microseconds.
const STALLED_AFTER: Duration = Duration::from_millis(750);

/// How long the SECOND writer is given to come back. It only has to be long enough that *"it never
/// returned"* is not *"it had not been scheduled yet"*.
const SECOND_WRITER_WAITS: Duration = Duration::from_secs(2);

/// How long to wait for the child to exit and the reader thread to publish it.
const CHILD_EXITS_WITHIN: Duration = Duration::from_secs(5);

/// A pane whose child has already exited, as the handle the HOST actually drives it through.
///
/// # ⚠⚠ Why a [`PanePtyHandle`] and not the [`PanePty`](sprag_terminal::PanePty)
///
/// The pty itself is not [`Sync`] — it owns the reader thread's channel — so a borrow of it cannot
/// reach a second thread, and two threads are the whole claim. The handle is the seam that exists
/// for precisely this: *"the producer-side seam the host's pane `External` holds to drive input
/// without owning the pty"*, cloneable, `Arc`-sharing the SAME writer mutex. ⚠ That makes this gate
/// closer to production rather than further from it: `PaneAccess::inject` reaches `write_shared`
/// through this door, which is the route the preserved backtrace shows.
///
/// ⚠ The workspace is leaked rather than dropped — see the module doc.
fn a_pane_nobody_is_reading() -> PanePtyHandle {
    let mut workspace = Workspace::new((80, 24));
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg("exit 0");
    command.env("TERM", "dumb");
    let pane = workspace
        .spawn(command, "sh".to_string(), 80, 24)
        .expect("spawn pane");
    let workspace: &'static Workspace = Box::leak(Box::new(workspace));
    let pty = workspace.pane(pane).expect("the pane just spawned").pty();

    let began = Instant::now();
    while !pty.is_eof() && began.elapsed() < CHILD_EXITS_WITHIN {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        pty.is_eof(),
        "⚠ THE FIXTURE FAILED, so nothing below is about a dead child: the child must have exited \
         within {CHILD_EXITS_WITHIN:?} and the pane must say so",
    );
    pty.handle()
}

/// Write `chunk` at `pane` from a thread of its own until it stops being taken, and answer HOW MANY
/// BYTES went in — `None` if [`GIVE_UP_AFTER`] was reached, which is *this pane never blocked*.
///
/// ⚠ The thread is never joined, because in the wedging arm it never finishes. What is read is the
/// counter, and a write that ERRORED would leave the counter still — so the two are told apart by
/// the caller, not here.
fn bytes_taken_before_it_blocked(pane: &PanePtyHandle, chunk: [u8; CHUNK]) -> Option<usize> {
    let written = &*Box::leak(Box::new(AtomicUsize::new(0)));
    let writer = pane.clone();
    std::thread::spawn(move || {
        while written.load(Ordering::Relaxed) < GIVE_UP_AFTER {
            if writer.write(&chunk, Hand::AProgram).is_err() {
                return;
            }
            written.fetch_add(CHUNK, Ordering::Relaxed);
        }
    });

    let mut at = written.load(Ordering::Relaxed);
    let mut last_moved = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(25));
        let now = written.load(Ordering::Relaxed);
        if now != at {
            at = now;
            last_moved = Instant::now();
            if at >= GIVE_UP_AFTER {
                return None;
            }
        } else if last_moved.elapsed() >= STALLED_AFTER {
            return Some(at);
        }
    }
}

#[test]
fn a_dead_pane_swallows_partial_lines_and_wedges_on_whole_ones() {
    // ── THE CONTROL, and it is the half the first draft of this gate got wrong ──
    //
    // ⚠⚠⚠ A dead pane is not a wall. With no line terminator the tty has nothing it can hold, so a
    // megabyte goes in without one write ever blocking — at a pane whose input buffer is measured
    // below at some seventeen KILObytes, so it demonstrably is not keeping what it took.
    let hole = a_pane_nobody_is_reading();
    hole.write(b"x", Hand::AProgram).expect(
        "⚠ THE FIRST CONTROL: a write to a pane whose child is dead must SUCCEED. An error here \
         would tell every caller at once, and there would be no defect to measure",
    );
    let partial = [b'x'; CHUNK];
    assert_eq!(
        bytes_taken_before_it_blocked(&hole, partial),
        None,
        "⚠⚠⚠ THE CONTROL FAILED, so the wedge below is not about LINES: a dead pane must take \
         {GIVE_UP_AFTER} bytes of unterminated input without blocking. If this one blocked, then \
         *the child is dead* is the whole condition after all, and the conjunction this gate is \
         built on is wrong",
    );

    // ── AND THE ARM THAT COST 43 HOURS: the same pane shape, one byte different ──
    let wall = a_pane_nobody_is_reading();
    let mut whole = [b'x'; CHUNK];
    whole[CHUNK - 1] = b'\n';
    let threshold = bytes_taken_before_it_blocked(&wall, whole).unwrap_or_else(|| {
        panic!(
            "⚠⚠⚠ NO WEDGE: this pane took {GIVE_UP_AFTER} bytes of COMPLETE LINES with its child \
             dead and never blocked. Either the defect this gate measures is fixed — in which case \
             REPURPOSE this gate, do not delete it — or something is draining the slave that \
             nobody declared."
        )
    });
    assert!(
        threshold > 0,
        "⚠ the writer must have got SOME bytes in before blocking, or the first control and this \
         measurement disagree about what a dead pane does",
    );

    // ⚠⚠⚠ THE PART THAT COSTS 43 HOURS: every OTHER writer to this pane is stuck too. That is the
    // shape the hand-over saw — one holder inside `write(fd=37, 1800 bytes)` and ten run workers
    // queued behind `pane_pty.rs:1489`.
    //
    // ⚠⚠⚠ **WHAT THIS GATE DELIBERATELY DOES NOT CLAIM, AND WHY IT MATTERS TO THE FIX.** It does
    // not separate *blocked on the writer mutex* from *blocked on the same full buffer*, because
    // from out here the two are the same silence — and a second writer to a queue with no room
    // would block in the kernel even if the mutex had been released the instant the first write
    // began. **So `write_shared` releasing its lock is not by itself a fix**, and a round that
    // shipped only that would move the queue from one place to another and measure green here.
    // What a fix has to change is the WRITE: bounded, or refused at a pane the product already
    // knows is dead (`PaneAccess::pane_eof`, register item 311). Registered rather than assumed.
    //
    // ⚠⚠⚠ **HALF OF THAT IS NOW PAID, AND THIS GATE IS STILL GREEN, WHICH IS THE POINT.**
    // `PaneAccess::inject` — the one door every PLUGIN types through — refuses at a pane whose
    // child has exited and answers `PaneError::PeerGone`, so no run can walk here any more.
    // **`write_shared` is untouched**: this gate drives `PanePtyHandle::write` directly, which is
    // also the route a PERSON's keystrokes take (`sprag_host::pane`), and that route still wedges
    // exactly as measured below. So items 304 and 305 are OPEN for the human door, and the green
    // here is the honest report of it rather than an oversight — a gate that had been written one
    // layer up would have gone red and been read as *the wedge is gone*.
    let second_returned = &*Box::leak(Box::new(AtomicBool::new(false)));
    let second = wall.clone();
    std::thread::spawn(move || {
        let _ = second.write(b"y\n", Hand::AProgram);
        second_returned.store(true, Ordering::SeqCst);
    });
    let began = Instant::now();
    while began.elapsed() < SECOND_WRITER_WAITS && !second_returned.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !second_returned.load(Ordering::SeqCst),
        "⚠⚠⚠ THE SECOND WRITER CAME BACK, which means one closed pane no longer stops every write \
         to it. That is the fix items 304 and 309 are owed — REPURPOSE this gate rather than \
         deleting it: the threshold it measured ({threshold} bytes) is still what a bounded write \
         has to be bounded against.",
    );

    println!(
        "\n== a pane whose child is dead ==\n  unterminated input: {GIVE_UP_AFTER} bytes taken, \
         never blocked\n  complete lines: blocked after {threshold} bytes\n  a second writer, \
         {SECOND_WRITER_WAITS:?} later: still inside `write_shared`\n"
    );
}
