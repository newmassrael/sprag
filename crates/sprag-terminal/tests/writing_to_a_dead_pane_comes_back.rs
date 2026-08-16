//! **NOBODY IS EVER STUCK INSIDE A PANE'S DEVICE — A DEAD ONE REFUSES INSTEAD OF SWALLOWING.**
//! The gate that measured the 43-hour wedge, repurposed to hold the repair it asked for.
//!
//! # What it used to say, and what happened to that
//!
//! This file was `write_to_a_dead_pane_wedges.rs`, and it asserted the DEFECT on purpose: a pane
//! whose child has exited takes newline-terminated input until the slave's queue fills, and the
//! next `write(2)` never returns. Every other writer to that pane queued behind it — `write_shared`
//! held the pane's shared writer mutex across the blocking call — and one such pane held a build
//! machine for 43 hours (register items 304, 305, 309, 310, 318-321, 325).
//!
//! **Half of it was paid at the plugin door**: `PaneAccess::inject` refuses a pane whose child has
//! exited and answers `PaneError::PeerGone`. That left the route a PERSON's keystrokes take —
//! `sprag_host::pane` → `PanePtyHandle::write` → the pty — untouched, which is the half this file
//! now holds.
//!
//! # ⚠⚠⚠ What the repair had to be, and what it could NOT be
//!
//! Item 320, measured by the previous shape of this gate: from outside, *blocked on the writer
//! mutex* and *blocked on the same full queue* are the same silence, so **dropping the lock is not
//! a fix** — it moves the queue from one place to another and measures green. What had to change is
//! the WRITE. It is now BOUNDED: the blocking `write(2)` happens on the pane's own device thread,
//! callers OFFER bytes to it, and a pane whose device is not taking them refuses with a sentence
//! once its backlog is full. No caller is ever inside `write(2)`.
//!
//! ⚠⚠ **AND THE GATE HAS TO SEPARATE «BOUNDED» FROM «UNBOUNDED», WHICH «IT CAME BACK» DOES NOT.** A
//! writer thread with no backlog limit would also let every caller return at once — while the
//! offered bytes piled up in memory for ever. So a dead pane must end in a REFUSAL and not merely
//! in a return, and swallowing [`GIVE_UP_AFTER`] bytes is a failure here.
//!
//! # ⚠⚠ The three controls, and none is decoration
//!
//! * **Partial lines still go straight in.** A cooked tty will not hold a line it has no end for,
//!   so a dead pane is a HOLE for unterminated input and a WALL for whole ones — measured, a
//!   megabyte in 0.09 s. A gate that knew only the wedging arm would have called *"a dead pane
//!   refuses"* the rule and been wrong about the commoner case.
//! * **A LIVE pane takes everything.** The refusal must be a fact about a device that has stopped
//!   taking input, never about whole lines, about volume, or about writes as such.
//! * **The SECOND writer comes back too.** That is the 43 hours: not one stuck write but every
//!   other writer stuck behind it.
//!
//! # ⚠⚠ Why it MEASURES the threshold instead of naming it
//!
//! The hand-over reported a queue that filled at 16,896 bytes; this host measures 17,408 for the
//! same shape, and an earlier run of a slightly different shape reported 17,664. **That number is a
//! kernel's, not sprag's**, and a constant would be folklore on the next machine (item 310).
//!
//! # ⚠⚠⚠ Why the workspace is still deliberately LEAKED
//!
//! The repair moves the blocking `write(2)` off the caller; it does not make it cancellable,
//! because nothing can. **The pane's own device thread is still parked in there for ever**, and it
//! is never joined for exactly that reason. Dropping the workspace would close a pane whose device
//! thread cannot answer, which is the unrecoverable teardown item 305 is about — this gate would
//! become the wedge it is measuring. The leak is one pty master and one parked thread per dead
//! pane for the life of the test binary, which process exit reclaims.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use sprag_terminal::{CommandBuilder, Hand, PanePtyHandle, Workspace};

/// Bytes per write. Small enough to measure the threshold with some resolution, large enough that
/// filling a tty buffer is not thousands of syscalls.
const CHUNK: usize = 256;

/// The most any arm will write before deciding this pane never pushed back.
///
/// ⚠ For the dead pane's whole-line arm this is a **RED and not a pass**: a pane that swallows a
/// megabyte of complete lines with a dead child on the other side is holding them somewhere, and
/// *somewhere* is the memory of the daemon. For the two arms that expect it — unterminated input,
/// and a live pane — it is the expected outcome and the number is the point.
const GIVE_UP_AFTER: usize = 4096 * CHUNK;

/// No progress for this long means the writer is inside the kernel rather than merely slow. Two
/// orders of magnitude past a local pty write, which is microseconds.
const STALLED_AFTER: Duration = Duration::from_millis(750);

/// How long the SECOND writer is given to come back. It only has to be long enough that *"it never
/// returned"* is not *"it had not been scheduled yet"*.
const SECOND_WRITER_WAITS: Duration = Duration::from_secs(2);

/// How long to wait for the child to exit and the reader thread to publish it.
const CHILD_EXITS_WITHIN: Duration = Duration::from_secs(5);

/// How a writer that kept offering bytes stopped doing so — the distinction the whole gate turns
/// on, because *"it came back"* and *"it was refused"* are different repairs.
#[derive(Debug)]
enum HowTheWriterEnded {
    /// The pane pushed back, in words. `after` is what it took first — the kernel's threshold plus
    /// whatever the backlog held — and `said` is the sentence a caller is given.
    Refused { after: usize, said: String },
    /// It reached [`GIVE_UP_AFTER`] and was never refused. Right for a live pane and for
    /// unterminated input; **wrong for whole lines at a dead one**, which would mean the bytes are
    /// piling up somewhere with no ceiling.
    TookEverything,
    /// It is still inside the device and is not coming back. This is the defect: the 43 hours.
    StillInsideTheDevice { after: usize },
}

/// What one writing thread is doing, watched from the test thread. Leaked with the thread that
/// owns it — see the module doc; in the wedging arm that thread never returns.
struct Attempt {
    written: AtomicUsize,
    refused: Mutex<Option<String>>,
    ended: AtomicBool,
}

/// A pane whose child has already exited, as the handle the HOST actually drives it through.
///
/// # ⚠⚠ Why a [`PanePtyHandle`] and not the [`PanePty`](sprag_terminal::PanePty)
///
/// The pty itself is not [`Sync`] — it owns the reader thread's channel — so a borrow of it cannot
/// reach a second thread, and two threads are the whole claim. The handle is the seam that exists
/// for precisely this: *"the producer-side seam the host's pane `External` holds to drive input
/// without owning the pty"*, cloneable, sharing the SAME device. ⚠ That makes this gate closer to
/// production rather than further from it — it is the call `sprag_host::pane` makes for a keyboard.
///
/// ⚠ The workspace is leaked rather than dropped — see the module doc.
fn a_pane_nobody_is_reading() -> PanePtyHandle {
    let pane = a_pane_running("exit 0");
    let began = Instant::now();
    while !pane.0.is_eof() && began.elapsed() < CHILD_EXITS_WITHIN {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        pane.0.is_eof(),
        "⚠ THE FIXTURE FAILED, so nothing below is about a dead child: the child must have exited \
         within {CHILD_EXITS_WITHIN:?} and the pane must say so",
    );
    pane.1
}

/// A pane whose child DRAINS its input for as long as anyone offers it — the control that keeps the
/// refusal a fact about a stopped device rather than about whole lines or about volume.
///
/// ⚠ `stty -echo` so a megabyte of input does not come back as a megabyte of output for the reader
/// thread to emulate; the claim is about the input queue, and the echo is a different device.
fn a_pane_that_takes_everything() -> PanePtyHandle {
    a_pane_running("stty -echo 2>/dev/null; exec cat >/dev/null").1
}

/// Spawn `/bin/sh -c script` on a leaked workspace and hand back both views of it: the pty, for the
/// fixture's own liveness question, and the handle every writer below uses.
fn a_pane_running(script: &str) -> (&'static sprag_terminal::PanePty, PanePtyHandle) {
    let mut workspace = Workspace::new((80, 24));
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(script);
    command.env("TERM", "dumb");
    let pane = workspace
        .spawn(command, "sh".to_string(), 80, 24)
        .expect("spawn pane");
    let workspace: &'static Workspace = Box::leak(Box::new(workspace));
    let pty = workspace.pane(pane).expect("the pane just spawned").pty();
    (pty, pty.handle())
}

/// Offer `chunk` at `pane` from a thread of its own until the pane stops taking it, and answer HOW
/// that ended.
///
/// ⚠ The thread is never joined: in the arm this gate exists for it is still inside `write(2)`, and
/// a blocked write cannot be cancelled. What is read is the counter and the ending it publishes —
/// which is why *refused* and *still in there* are told apart HERE and not left to the caller. The
/// previous shape of this gate could only report a number, and a number cannot tell them apart.
fn offer_until_it_stops(pane: &PanePtyHandle, chunk: [u8; CHUNK]) -> HowTheWriterEnded {
    let attempt = &*Box::leak(Box::new(Attempt {
        written: AtomicUsize::new(0),
        refused: Mutex::new(None),
        ended: AtomicBool::new(false),
    }));
    let writer = pane.clone();
    std::thread::spawn(move || {
        while attempt.written.load(Ordering::Relaxed) < GIVE_UP_AFTER {
            if let Err(pushed_back) = writer.write(&chunk, Hand::APerson) {
                *attempt
                    .refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(pushed_back.to_string());
                break;
            }
            attempt.written.fetch_add(CHUNK, Ordering::Relaxed);
        }
        attempt.ended.store(true, Ordering::Release);
    });

    let mut at = attempt.written.load(Ordering::Relaxed);
    let mut last_moved = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(25));
        if attempt.ended.load(Ordering::Acquire) {
            let after = attempt.written.load(Ordering::Relaxed);
            let said = attempt
                .refused
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            return match said {
                Some(said) => HowTheWriterEnded::Refused { after, said },
                None => HowTheWriterEnded::TookEverything,
            };
        }
        let now = attempt.written.load(Ordering::Relaxed);
        if now != at {
            at = now;
            last_moved = Instant::now();
        } else if last_moved.elapsed() >= STALLED_AFTER {
            return HowTheWriterEnded::StillInsideTheDevice { after: at };
        }
    }
}

#[test]
fn a_dead_pane_refuses_a_writer_rather_than_keeping_it() {
    // ── CONTROL 1: a dead pane is a HOLE for unterminated input, and that is not a defect ──
    //
    // ⚠⚠⚠ With no line terminator the tty has nothing it can hold, so a megabyte goes in without
    // one write ever pushing back — at a pane whose input queue is measured below at some seventeen
    // KILObytes, so it demonstrably is not keeping what it took. This arm is what stops the repair
    // below from being read as *a dead pane refuses everything*.
    let hole = a_pane_nobody_is_reading();
    hole.write(b"x", Hand::APerson).expect(
        "⚠ THE FIRST CONTROL: a single keystroke at a pane whose child is dead must still be \
         taken. `WIRE_PROTOCOL` 36 published that a person's keystrokes are unaffected, and a \
         refusal here would make that sentence false as well as this gate meaningless",
    );
    let partial = [b'x'; CHUNK];
    assert!(
        matches!(
            offer_until_it_stops(&hole, partial),
            HowTheWriterEnded::TookEverything
        ),
        "⚠⚠⚠ THE CONTROL FAILED, so the refusal below is not about a STOPPED DEVICE: a dead pane \
         must take {GIVE_UP_AFTER} bytes of unterminated input without ever pushing back. If it \
         refused, the backlog is being reached by input the kernel was draining all along",
    );

    // ── THE ARM THAT COST 43 HOURS, and what it must do now ──
    let wall = a_pane_nobody_is_reading();
    let mut whole = [b'x'; CHUNK];
    whole[CHUNK - 1] = b'\n';
    let (threshold, said) = match offer_until_it_stops(&wall, whole) {
        HowTheWriterEnded::Refused { after, said } => (after, said),
        HowTheWriterEnded::StillInsideTheDevice { after } => panic!(
            "⚠⚠⚠⚠ THE WEDGE IS BACK. A writer offering complete lines at a pane whose child is \
             dead is still inside the device after {after} bytes and is not coming back — a \
             blocked `write(2)` cannot be cancelled, so this thread is gone for the life of the \
             process and so is every other writer to this pane. That is register items 304/305 \
             and the 43 hours."
        ),
        HowTheWriterEnded::TookEverything => panic!(
            "⚠⚠⚠⚠ IT TOOK EVERYTHING, WHICH IS NOT THE FIX EITHER. {GIVE_UP_AFTER} bytes of \
             complete lines went into a pane whose child is dead and nobody pushed back. The \
             kernel stops taking them at some seventeen KILObytes, so the rest is being held in \
             this daemon's memory with no ceiling — an unbounded backlog returns to every caller \
             at once and looks exactly like a repair from out here."
        ),
    };
    assert!(
        threshold > 0,
        "⚠ the writer must have got SOME bytes in before being refused, or control 1 and this \
         measurement disagree about what a dead pane does",
    );
    assert!(
        said.contains("not taking input") && said.contains("waiting"),
        "⚠⚠⚠ AND IT HAS TO SAY WHY. A write that stops working without a sentence is the defect \
         R396-R399 spent four rounds on: the caller must be told the DEVICE has stopped taking \
         input and how much is still owed, not handed a bare error to wrap in a retry. It said: \
         {said:?}",
    );

    // ── AND THE PART THAT COST THE 43 HOURS: every OTHER writer to that pane ──
    //
    // ⚠⚠⚠ One holder inside `write(fd=37, 1800 bytes)` and ten run workers queued behind
    // `pane_pty.rs:1489` is the shape the preserved core showed. One stuck write is a lost
    // keystroke; every writer stuck behind it is a daemon.
    let second_returned = &*Box::leak(Box::new(AtomicBool::new(false)));
    let second = wall.clone();
    std::thread::spawn(move || {
        let _ = second.write(b"y\n", Hand::APerson);
        second_returned.store(true, Ordering::SeqCst);
    });
    let began = Instant::now();
    while began.elapsed() < SECOND_WRITER_WAITS && !second_returned.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        second_returned.load(Ordering::SeqCst),
        "⚠⚠⚠⚠ THE SECOND WRITER NEVER CAME BACK. It is not the one that filled the queue — it \
         offered two bytes at a pane somebody else had already been refused at — so it is being \
         held by the first one, which is precisely the wedge: one closed pane stopping every write \
         to it, for ever.",
    );

    // ── THE EYE, which is the other half of what item 304 asked for ──
    //
    // ⚠⚠⚠ **A REPAIR THAT ONLY REFUSES LEAVES THE CAMOUFLAGE IN PLACE.** Eighty-odd kilobytes went
    // into that pane before anybody was told anything, because a pseudoterminal whose child is dead
    // ACCEPTS input until its queue fills — no error, nothing logged. So the pane has to be able to
    // say how much of what it was given its device never took, and the message parked inside
    // `write(2)` for ever is exactly the one that has to be in that number.
    //
    // ⚠⚠⚠⚠ **AND IT NEEDS A PANE OF ITS OWN, WHICH THE FIRST DRAFT OF THIS ARM DID NOT GIVE IT.**
    // Asked of `wall` it proves nothing: sixty-four kilobytes of offers are still sitting in that
    // pane's channel undelivered, so the number is that backlog whether or not the message inside
    // `write(2)` is counted. Measured — the mutation below stayed GREEN through it. What separates
    // the two is a pane given exactly ONE message and nothing behind it: the device picks it up,
    // parks inside it for ever, and the pane must go on saying it owes the whole of it.
    //
    // ⚠⚠ **THE MUTATION THIS ARM EXISTS FOR**: credit the backlog when the message is taken off the
    // channel instead of when its `write(2)` RETURNS. Everything else above stays green — the
    // writers still fill the backlog, still get refused, still come back — and this pane, holding
    // bytes it will never deliver, reports owing NOTHING.
    // ⚠⚠ COMPLETE LINES ALL THE WAY THROUGH, and the first draft of this fixture got that wrong:
    // 32 KiB of `x` with a single `\n` at the END is 32 KiB of UNTERMINATED input, which control 1
    // above has just measured a dead pane swallowing whole. The device took it, the write returned,
    // and the pane correctly reported owing nothing — a red that was the fixture's and not the
    // product's.
    let parked = a_pane_nobody_is_reading();
    let one_message: Vec<u8> = std::iter::repeat_n(whole, 128).flatten().collect();
    parked.write(&one_message, Hand::APerson).expect(
        "⚠ THE FIXTURE: one message at a pane owing nothing must be accepted — a single offer is \
         never held back for its own size",
    );
    let began = Instant::now();
    while began.elapsed() < STALLED_AFTER {
        assert_eq!(
            parked.input_backlog(),
            one_message.len(),
            "⚠⚠⚠⚠ THE PANE SAYS ITS DEVICE HAS TAKEN BYTES IT IS STILL PARKED INSIDE. Nothing is \
             ever going to drain that pseudoterminal, so this number must stay at the whole {} \
             bytes it was handed; a backlog that falls the moment the device thread PICKS a \
             message up is counting dequeues, not deliveries — and it reports a pane on its way to \
             a wall it will never get past as an idle one, which is the camouflage item 304 is \
             about",
            one_message.len(),
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let still_owed = parked.input_backlog();

    // ── CONTROL 2: a LIVE pane takes everything, so the refusal is about the DEVICE ──
    //
    // ⚠⚠ Same bytes, same call, same volume — a child that drains. Without this arm a repair that
    // simply refused whole lines, or refused past some size, would measure green above.
    let living = a_pane_that_takes_everything();
    assert!(
        matches!(
            offer_until_it_stops(&living, whole),
            HowTheWriterEnded::TookEverything
        ),
        "⚠⚠⚠ THE SECOND CONTROL FAILED: a pane whose child is READING must take {GIVE_UP_AFTER} \
         bytes of the identical complete lines. If it was refused, the bound above is a bound on \
         writing rather than on a device that has stopped taking input, and it will refuse a \
         person mid-paste at a perfectly healthy pane",
    );

    println!(
        "\n== a pane whose child is dead ==\n  unterminated input: {GIVE_UP_AFTER} bytes taken, \
         never refused\n  complete lines: refused after {threshold} bytes\n  it said: {said}\n  a \
         second writer: back within {SECOND_WRITER_WAITS:?}\n  the pane says its device has not \
         taken {still_owed} bytes, and still has not {STALLED_AFTER:?} later\n  a LIVE pane, same \
         lines: {GIVE_UP_AFTER} bytes taken\n"
    );
}
