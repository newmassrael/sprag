//! Stopping a pane's foreground JOB — a SIGNAL to a process group, not a byte on a tty.
//!
//! # ⚠⚠⚠ The question this exists to answer
//!
//! *Does a `Ctrl-C` written into a pane guarantee that the job stops?* **It does not, and nothing
//! in this workspace ever promised it did** — [`PanePty::write`](crate::PanePty::write) puts bytes
//! on a pseudoterminal and `sprag_plugin::Written`'s own doc already says *"success is not
//! delivery."* What turns the byte `0x03` into a `SIGINT` is the LINE DISCIPLINE, and it does so
//! only when two conditions the writer neither controls nor can observe atomically both hold:
//!
//! * the terminal has `ISIG` set — a program that took the terminal raw (an editor, a full-screen
//!   TUI, anything that ran `stty -isig`) has turned that off, and then `0x03` is ordinary input;
//! * and the group the signal would go to is the one the caller MEANT — the kernel reads the
//!   terminal's foreground group at the instant it processes the character, so a byte written while
//!   a shell is still handing the terminal over reaches the shell, which ignores it.
//!
//! **Measured, deterministically, on this machine**: a pane running `stty -isig; sleep 300`, sent
//! `C-c` through the product's own `send-keys`, echoes `^C` onto its screen and the `sleep` lives
//! on. That is the same snapshot a CI runner produced under load with `ISIG` untouched — the byte
//! arrived, the signal did not — which is why the answer had to be a mechanism rather than a
//! longer wait.
//!
//! # What this does instead
//!
//! It sends the signal itself, to the process GROUP that owns the pane's terminal, which is the
//! same address the line discipline would have used and the same one a person's keyboard reaches.
//! No byte is written, so nothing depends on the terminal's modes, on what the child does with its
//! input, or on when a grid saw anything. And because the delivery is this daemon's own syscall,
//! it can SAY what it reached ([`StoppedJob`]) or why it could not ([`Unstopped`]) — which a write
//! of `0x03` can never do, because a write succeeds either way.
//!
//! # ⚠ What it still does not promise
//!
//! **That the program obeys.** [`Stop::Interrupt`] and [`Stop::Terminate`] are catchable and
//! ignorable; a program that blocks them keeps running and this reports an honest success, because
//! the signal WAS delivered. Only [`Stop::Kill`] cannot be refused. A caller that needs the work to
//! be over rather than merely asked observes the pane afterwards — the foreground job it reads back
//! is the evidence, exactly as it is for a person at a keyboard.
//!
//! # ⚠ The residual window, stated rather than smoothed over
//!
//! The group is READ and then SIGNALLED, and a job that ends in between leaves a group id the
//! kernel may eventually give to somebody else. The window is microseconds and it is not new: it is
//! the one [`PanePty`](crate::PanePty)'s own hard kill documents, and it is the one the line
//! discipline itself has — it reads the terminal's foreground group at signal time too. A process
//! group id is not reused while the group has any member, so the hazard needs the whole job to exit
//! AND a new group leader to be born on that number inside those microseconds. It is not closed by
//! checking again, because a second read has the same window as the first.

use crate::processes::JobProcess;

sprag_vt::closed_set! {
/// WHAT A CALLER ASKS A PANE'S FOREGROUND JOB TO DO.
///
/// Three, because they are three different requests a person makes of a running job and no two of
/// them substitute: *stop what you are doing*, *shut down*, and *be gone*. An AI control loop that
/// runs out of time wants the first — the peer's current turn ends and the peer is still there for
/// the next run — and a loop that could only reach for the last would have to destroy a pane, its
/// shell and its scrollback to end one turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// END THE WORK, KEEP THE PROGRAM (`SIGINT`) — what a person's `Ctrl-C` means.
    ///
    /// The one an AI loop's cancel or deadline wants: an agent CLI aborts the turn in flight and
    /// returns to its own prompt, so the next run finds a peer that is ready rather than a pane
    /// that is gone.
    Interrupt,
    /// ASK THE PROGRAM ITSELF TO END (`SIGTERM`), giving it its own chance to finish first.
    Terminate,
    /// THE KERNEL ENDS IT AND NOTHING RUNS ON THE WAY OUT (`SIGKILL`) — for a job that has refused
    /// the other two.
    Kill,
}
}

impl Stop {
    /// The signal number this asks for.
    ///
    /// Exhaustive, so a fourth request cannot reach a syscall without somebody choosing what it
    /// sends.
    #[must_use]
    pub const fn signal(self) -> i32 {
        match self {
            Self::Interrupt => libc::SIGINT,
            Self::Terminate => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
        }
    }

    /// This request's WORD on the wire — the one place the variant → name mapping lives, so no
    /// surface spells a `Stop` variant itself ([`Cost::unit`](../../sprag_plugin/plugin/enum.Cost.html)'s
    /// rule, which this follows).
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Kill => "kill",
        }
    }

    /// The request a caller's word names, or `None` for a word no surface publishes.
    ///
    /// ⚠ DERIVED from [`ALL`](Self::ALL) rather than a second `match`, so the set a caller may say
    /// and the set a surface publishes cannot drift apart — the drift this project has paid for on
    /// three vocabularies now.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stop| stop.wire_str() == word)
    }
}

sprag_vt::wire_words!(Stop: wire_str);

impl std::fmt::Display for Stop {
    /// The verb a report uses for what was done, reading as *"the job was …"*.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Interrupt => "interrupted",
            Self::Terminate => "asked to terminate",
            Self::Kill => "killed",
        })
    }
}

/// WHAT A STOP REACHED — the job that received the signal.
///
/// Carried rather than assumed, because the whole reason this mechanism exists is that a caller
/// writing `0x03` could not find out. A cancelled run publishes this, and *"the `claude` your run
/// was driving was interrupted"* is an answer somebody can act on where *"the run was cancelled"*
/// alone is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoppedJob {
    /// Which request was delivered.
    pub stop: Stop,
    /// The process GROUP that received it — the pane terminal's foreground group, and the address
    /// a person's keyboard reaches.
    pub pgid: u32,
    /// The group's LEADER, when it is still readable.
    ///
    /// `None` is a FACT and not a failure, and it is the one absence
    /// [`foreground_leader_of`](crate::foreground_leader_of) already documents: a group whose
    /// leader has exited lives on through its other members, and there is then a job to signal and
    /// no name to print for it. The signal is still delivered — a report that refused to act
    /// because it could not narrate would be the tail wagging the dog.
    pub leader: Option<JobProcess>,
}

sprag_vt::closed_set! {
/// WHY A STOP WAS NOT DELIVERED — ⚠ and so why the work is still running.
///
/// A closed set because each arm is a DIFFERENT thing to tell a caller — your pane is finished,
/// this host cannot see process groups at all, the kernel said no — and an `Option` spelled the
/// first two the same, which is the mistake [`PaneDoing`](../../sprag_plugin/access/enum.PaneDoing.html)
/// was split three ways to correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unstopped {
    /// The pane has no live child: it exited and its status has been published, so there is
    /// nothing left of this pane to signal — and its pid is free to be reused, which is why this
    /// refuses rather than signalling a number that no longer means the pane.
    Gone,
    /// This host cannot see which group owns the pane's terminal — no process table, or a pane
    /// with no controlling terminal. The same absence
    /// [`foreground_pgid_of`](crate::pane_pty::foreground_pgid_of) answers with `None`.
    Unseen,
    /// The kernel refused the signal, with its own `errno`.
    ///
    /// `ESRCH` for a group that ended between the read and the send — the window the module doc
    /// states — and `EPERM` for one this daemon may not signal.
    Refused(i32) = (0),
    /// ⚠⚠ THE PANE'S OWN PROGRAM is what owns the terminal, and the caller asked to reach no
    /// further than [`Reach::UnderTheProgram`] — so nothing was signalled.
    ///
    /// # Why this is a refusal and not a stop
    ///
    /// Signalling the pane's own child can END THE PANE: a program with no `SIGINT` handler dies,
    /// its pty closes, and the pane goes with it. A shell survives that and `cat` does not, and
    /// nothing here can tell those apart. For a caller who asked deliberately — a person typing
    /// `stop-job` — that is a consequence they chose. For a BOUNDED RUN whose clock simply ran out
    /// it is not: a routine timeout must not be able to close somebody's pane, still less the last
    /// pane of a session. **Measured**: it closed one, and the daemon exited behind it.
    IsTheProgram,
}
}

sprag_vt::closed_set! {
/// HOW FAR A STOP MAY REACH — the choice that decides whether the PANE can end with the job.
///
/// It is the caller's and cannot be inferred, because the two cases are indistinguishable from
/// here: a pane whose own program is an agent CLI mid-turn and a pane whose own program is `cat`
/// look identical to the process table, and a signal ends the turn in the first and the pane in the
/// second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Only work the pane's own program STARTED. A foreground group that IS the pane's program is
    /// left alone and reported as [`Unstopped::IsTheProgram`].
    ///
    /// What an automatic stop wants: a run that ran out of time ends the work it caused and never
    /// the pane it was given.
    UnderTheProgram,
    /// Whatever owns the terminal, the pane's own program included — what a person's `Ctrl-C` does,
    /// and what a caller naming one pane on purpose is asking for.
    TheProgramToo,
}
}

impl std::fmt::Display for Unstopped {
    /// ⚠ THE SENTENCE A CALLER READS when their stop did not land, in the shape the rest of this
    /// workspace answers refusals in: what was not done, and why, in words that are not a Rust
    /// variant name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => f.write_str("the pane's child has already exited, so it runs nothing"),
            Self::Unseen => f.write_str(
                "this host cannot see which job owns the pane's terminal, so it has no group to \
                 signal",
            ),
            Self::Refused(errno) => write!(
                f,
                "the kernel refused to signal the pane's job (errno {errno})",
            ),
            Self::IsTheProgram => f.write_str(
                "the pane's own program is what is running, and signalling that could end the \
                 pane, so nothing was sent — stop it by name if that is what you want, or close \
                 the pane",
            ),
        }
    }
}

/// Send `stop` to the foreground job on `pane_child`'s controlling terminal.
///
/// `pane_child` is the pid of the child the daemon spawned for the pane — the pane's shell,
/// normally — and NOT the job: the whole point is that the job is whatever that terminal currently
/// belongs to, which the caller does not know and must not have to.
///
/// # Why a pid and not a pane
///
/// The same reason [`foreground_pgid_of`](crate::pane_pty::foreground_pgid_of) takes one: this is
/// I/O, and the caller that runs it must not hold the workspace lock across a syscall. A pid is not
/// a pane, and nothing here can reach the registry.
///
/// # At a prompt, the shell IS the job
///
/// A pane sitting at its prompt has its own child as the terminal's foreground group. Under
/// [`Reach::TheProgramToo`] the stop is delivered to that shell — exactly what a person's `Ctrl-C`
/// at a prompt does, and what a shell answers by redrawing the prompt. Under
/// [`Reach::UnderTheProgram`] it is refused as [`Unstopped::IsTheProgram`], because the same
/// delivery to a pane whose program is not a shell ends the pane.
///
/// # Errors
///
/// [`Unstopped`] when there is no group to signal, the group IS the pane's own program and `reach`
/// forbids that, or the kernel refused — see each arm.
pub fn stop_foreground_job(
    pane_child: u32,
    stop: Stop,
    reach: Reach,
) -> Result<StoppedJob, Unstopped> {
    let pgid = crate::pane_pty::foreground_pgid_of(pane_child).ok_or(Unstopped::Unseen)?;
    // ⚠ DECIDED BEFORE THE SIGNAL, necessarily: there is no undoing one, and the whole point of the
    // narrow reach is that the pane must still be there afterwards.
    if reach == Reach::UnderTheProgram && pgid == pane_child {
        return Err(Unstopped::IsTheProgram);
    }
    // Read the name BEFORE signalling: after it, `Stop::Kill`'s target may already be unreadable,
    // and a report that names nothing for the one request that always works would be worst where
    // it matters most.
    let leader = crate::processes::foreground_leader_of(pane_child);
    let group: libc::pid_t = pgid.try_into().map_err(|_| Unstopped::Unseen)?;
    // SAFETY: `kill` is async-signal-safe and takes no pointers. `-group` names a process GROUP —
    // the one that owns this pane's terminal, read above — and the signal number comes from
    // `Stop::signal`, which is exhaustive over the closed set.
    if unsafe { libc::kill(-group, stop.signal()) } == 0 {
        return Ok(StoppedJob { stop, pgid, leader });
    }
    Err(Unstopped::Refused(
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every request has a DISTINCT wire word, and every word round-trips back to its request.
    ///
    /// ⚠ Derived from [`Stop::ALL`], so a fourth request cannot be added with no word, with a word
    /// that collides with an existing one, or with a word [`Stop::from_wire`] does not answer for.
    /// A hand-written list of three is the shape this workspace has removed four times.
    #[test]
    fn every_stop_has_its_own_word_and_the_word_finds_it_again() {
        for stop in Stop::ALL {
            assert_eq!(
                Stop::from_wire(stop.wire_str()),
                Some(stop),
                "{stop:?}'s own word must name it again, or a caller can say a word the surface \
                 publishes and be refused",
            );
            assert_eq!(
                Stop::ALL
                    .iter()
                    .filter(|other| other.wire_str() == stop.wire_str())
                    .count(),
                1,
                "{stop:?}'s word is its alone — two requests sharing one word makes the argument \
                 undecidable",
            );
        }
        assert_eq!(
            Stop::from_wire("INTERRUPT"),
            None,
            "and the words are the published spellings, not a case-insensitive guess",
        );
    }

    /// Each refusal reads as its own sentence, and none of them leaks a variant name.
    ///
    /// ⚠ Walks [`Unstopped::ALL`] rather than a literal list, and asserts the sentences are
    /// DISTINCT — a catch-all message passes every shape check while telling three failures apart
    /// from none, which is the trap `PaneError`'s own gate was rebuilt to avoid.
    #[test]
    fn every_refusal_says_something_different_and_none_of_it_is_rust() {
        let mut said: Vec<String> = Vec::new();
        for why in Unstopped::ALL {
            let sentence = why.to_string();
            assert!(
                !sentence.is_empty(),
                "{why:?} has no sentence, so a caller reads nothing",
            );
            // ⚠ Against the type's OWN `Debug`, not against punctuation. The first spelling of this
            // check banned `(` and so banned `(errno 13)` — a legitimate English parenthesis — while
            // still passing anything that merely avoided one. What a reader must not be handed is
            // the RUST rendering, and `Debug` is exactly that rendering, so comparing to it is the
            // check rather than a proxy for it.
            let rust = format!("{why:?}");
            assert!(
                !sentence.contains(&rust),
                "{why:?} hands a reader who cannot look a variant up its Rust shape: {sentence}",
            );
            assert!(
                !said.contains(&sentence),
                "{why:?} repeats a sentence another refusal already uses, so a caller cannot tell \
                 which they got: {sentence}",
            );
            said.push(sentence);
        }
        assert_eq!(
            said.len(),
            Unstopped::ALL.len(),
            "every refusal in the type was asked, not a hand-picked few",
        );
    }

    /// A pane whose child is gone answers [`Unstopped::Unseen`] rather than signalling a number
    /// that no longer means anything.
    ///
    /// Pid 0 is used deliberately: it is not a process, and to `kill(2)` a negated 0 would mean
    /// *"my own process group"* — this test's own test runner. The reader refuses before reaching
    /// the syscall, and that ordering is the claim.
    #[test]
    fn a_pane_with_no_process_to_read_is_refused_before_any_signal_is_sent() {
        assert_eq!(
            stop_foreground_job(0, Stop::Kill, Reach::TheProgramToo),
            Err(Unstopped::Unseen),
            "an unreadable process has no foreground group, and the alternative reading of `0` is \
             this very test's process group",
        );
    }

    /// ⚠⚠⚠ **THE NARROW REACH LEAVES THE PANE'S OWN PROGRAM ALONE, AND THE WIDE ONE ENDS IT** —
    /// the same pane, the same instant, one argument different.
    ///
    /// # ⚠⚠ The measurement that put this here
    ///
    /// Before [`Reach`] existed, a run cut short signalled whatever owned the pane's terminal. On a
    /// pane whose own program was `cat` — the fake peer every loop fixture uses — that killed the
    /// child, the pty closed, the pane went, and it was a session's LAST pane, **so the daemon
    /// exited.** A routine deadline expiring must not be able to do that. A person typing a stop at
    /// one named pane may; those are different callers and the argument is how they say so.
    ///
    /// ⚠ The fixture is `exec sleep 300` with no shell in between, so the pane's own child IS the
    /// foreground group — the condition being discriminated. A pane running a shell would have the
    /// job one level down and both reaches would behave alike, which is exactly the case that would
    /// make this gate vacuous.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_stop_that_must_not_end_the_pane_leaves_the_panes_own_program_running() {
        use crate::{CommandBuilder, PanePty};
        use std::time::{Duration, Instant};

        let spawn = || {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec sleep 300");
            command.env("TERM", "dumb");
            let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
            let child = pty.pid().expect("a live child");
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                if crate::foreground_leader_of(child).is_some_and(|job| job.name == "sleep") {
                    return (pty, child);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("the fixture never reached its job, so nothing below measures anything");
        };

        // ⚠ THE CONTROL: the narrow reach refuses, by name, and the program is still there.
        let (narrow, child) = spawn();
        assert_eq!(
            stop_foreground_job(child, Stop::Interrupt, Reach::UnderTheProgram),
            Err(Unstopped::IsTheProgram),
            "a stop that may not end the pane must refuse when the pane's own program is what \
             owns the terminal",
        );
        assert_eq!(
            narrow.exit_status(),
            None,
            "⚠⚠ and NOTHING WAS SENT — the pane's program is still running, which is the whole \
             point of the narrow reach",
        );

        // ⚠ THE SUBJECT: the wide reach is the same call with the caller's own decision in it.
        let (wide, child) = spawn();
        let stopped = stop_foreground_job(child, Stop::Interrupt, Reach::TheProgramToo)
            .expect("a caller who asked for the program too reaches it");
        assert_eq!(
            stopped.pgid, child,
            "and what it reached IS the pane's program"
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) && wide.exit_status().is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            wide.exit_status().is_some_and(|exit| exit.signal.is_some()),
            "⚠⚠ THE SUBJECT: the pane's own program ended by a signal — the consequence the narrow \
             reach exists to keep away from a run that merely ran out of time",
        );
    }

    /// ⚠⚠ **THE THREE PUBLISHED WORDS DO THREE DIFFERENT THINGS**, against a job that has said no
    /// to the first two.
    ///
    /// A vocabulary whose members are all accepted proves only that the parser reads them. What
    /// makes three words worth publishing is that a caller who picks the wrong one gets a different
    /// outcome — so the fixture TRAPS `INT` and `TERM` and the gate walks the escalation: interrupt
    /// (ignored), terminate (ignored), kill (cannot be).
    ///
    /// ⚠ Without the trap this would pass with all three mapped to `SIGKILL`, which is exactly the
    /// shape of vacuous vocabulary gate this workspace has removed before.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_job_that_refuses_the_first_two_stops_are_ended_by_the_third() {
        use crate::{CommandBuilder, PanePty};
        use std::time::{Duration, Instant};

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // `trap '' INT TERM` sets both to IGNORED, which `exec` PRESERVES across the new image —
        // so the sleep itself is the process that ignores them, not a shell standing in front of
        // it. That is what makes the pgid read below the job's own.
        command.arg("trap '' INT TERM; exec sleep 300");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        let child = pty.pid().expect("a live child");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10)
            && !crate::foreground_leader_of(child).is_some_and(|job| job.name == "sleep")
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            crate::foreground_leader_of(child).is_some_and(|job| job.name == "sleep"),
            "the fixture never reached its job, so nothing below measures anything",
        );

        for ignored in [Stop::Interrupt, Stop::Terminate] {
            stop_foreground_job(child, ignored, Reach::TheProgramToo)
                .unwrap_or_else(|why| panic!("{ignored:?} is delivered: {why}"));
            // ⚠ A DELIVERED SIGNAL IS NOT OBEDIENCE, which is what this half asserts and what the
            // module doc promises rather than hides. The wait is generous in the direction that
            // makes the claim WEAKER if it is wrong — a job that was going to die has ample time.
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(500) && pty.exit_status().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                pty.exit_status(),
                None,
                "{ignored:?} was DELIVERED and the job ignored it — the report says delivered, and \
                 that is all it ever claims",
            );
        }

        stop_foreground_job(child, Stop::Kill, Reach::TheProgramToo).expect("a kill is delivered");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) && pty.exit_status().is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            pty.exit_status().is_some_and(|exit| exit.signal.is_some()),
            "⚠⚠ AND THE THIRD CANNOT BE REFUSED — which is the whole reason a caller is offered \
             three words instead of one",
        );
    }

    /// ⚠⚠⚠ **THE BYTE IS THE CONTROL AND THE SIGNAL IS THE SUBJECT**, on ONE pane, ONE job and one
    /// run of the test — the pair that answers the open product question this module exists for.
    ///
    /// The pane runs `stty -isig; exec sleep 300`, so its child IS the job and the terminal's line
    /// discipline has been told not to make signals out of input. Then:
    ///
    /// 1. `0x03` — the exact byte `send-keys C-c` writes — is written to the pty. The screen shows
    ///    `^C`, which is the line discipline ECHOING it: proof the byte was not merely queued but
    ///    PROCESSED, so what follows is not a race that a longer wait would win. **The job lives.**
    /// 2. [`stop_foreground_job`] sends `SIGINT` to the group. **The job dies, by a signal**, and
    ///    the report names it.
    ///
    /// ⚠ Without step 1's echo assertion this would be a negative claim bounded by a timer, which
    /// is the load-marginal shape this workspace has paid for repeatedly. The echo makes the
    /// control an OBSERVATION rather than a wait.
    ///
    /// ⚠ REVERT-PROOF: make [`stop_foreground_job`] write `0x03` to the pty instead of signalling
    /// the group and step 2 fails — which is the whole difference between the mechanism this module
    /// adds and the one it exists because of.
    // Linux AND macOS: the reader underneath is `procfs`, which answers on both since R343. A gate
    // for a portable fact that ran on one platform is how the last divergence here reached a push.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_byte_the_line_discipline_ignores_stops_nothing_and_a_signal_to_the_group_stops_the_job() {
        use crate::{CommandBuilder, PanePty};
        use std::time::{Duration, Instant};

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty -isig; exec sleep 300");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        let child = pty.pid().expect("a live child");

        // The pane's child EXECS the job, so the pid is stable and the name is how we know `stty`
        // has already run — a job named `sleep` is one the `exec` reached.
        let until = |deadline: Duration, mut ready: Box<dyn FnMut() -> bool>| {
            let start = Instant::now();
            while start.elapsed() < deadline {
                if ready() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(
            until(
                Duration::from_secs(10),
                Box::new(
                    || crate::foreground_leader_of(child).is_some_and(|job| job.name == "sleep")
                ),
            ),
            "the fixture never reached its job, so nothing below measures anything",
        );

        pty.write(&[0x03]).expect("write the interrupt byte");
        assert!(
            until(
                Duration::from_secs(10),
                Box::new(|| pty.with_screen(|screen| screen.full_text().contains("^C"))),
            ),
            "⚠ THE CONTROL'S PREMISE: the terminal must ECHO the byte, or the job surviving it \
             says only that the byte had not arrived yet",
        );
        assert_eq!(
            pty.exit_status(),
            None,
            "⚠⚠ THE CONTROL: the byte was processed by the line discipline and made NO signal, so \
             the job is still running — this is what `send-keys C-c` guarantees, and it is nothing",
        );

        let stopped = stop_foreground_job(child, Stop::Interrupt, Reach::TheProgramToo)
            .expect("the group is signalled");
        assert_eq!(
            stopped.pgid, child,
            "the group signalled is the one that owns the pane's terminal",
        );
        assert_eq!(
            stopped.leader.as_ref().map(|job| job.name.as_str()),
            Some("sleep"),
            "and the report NAMES the job it reached, which a write of 0x03 can never do",
        );
        assert!(
            until(
                Duration::from_secs(10),
                Box::new(|| pty.exit_status().is_some()),
            ),
            "⚠⚠ THE SUBJECT: the signal ended the job the byte could not",
        );
        assert!(
            pty.exit_status().is_some_and(|exit| exit.signal.is_some()),
            "and it ended BY A SIGNAL rather than returning — the platform's own spelling for \
             which is not asserted, because a gate that names it asserts a distribution's \
             packaging",
        );
    }
}
