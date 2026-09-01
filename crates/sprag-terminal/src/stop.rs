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
    ///
    /// # ⚠⚠ Why NOTHING BUILDS THIS, measured rather than assumed
    ///
    /// It was first written down as *"needs an `EPERM` a test cannot arrange without another
    /// user's process"*, which was a guess. Driving it found the real reason, and it is worth more
    /// than the guess was:
    ///
    /// * **`EPERM` is out of reach by construction.** The only group this function can name is the
    ///   one a pane's terminal points at, and that is always a descendant of this daemon.
    /// * **`ESRCH` needs the group GONE while the terminal still points at it**, which is a race of
    ///   microseconds because the shell reclaims the terminal the moment it notices. The obvious way
    ///   to hold that window open — `SIGSTOP` the shell, then kill the job — **does not work, and
    ///   the reason is the interesting part: a stopped shell cannot REAP, so the job's leader
    ///   becomes a ZOMBIE, a zombie is still a member of its process group, and `killpg` therefore
    ///   SUCCEEDS.** Only the shell can reap it, and a shell that reaps is a shell that reclaims.
    ///
    /// So this arm is real in production — the module doc's window — and not constructible on
    /// demand here. Its SENTENCE is covered through [`ALL`](Self::ALL); the path is not, and
    /// **recorded so nobody adds a vacuous gate or a retry loop that passes by luck.**
    Refused(i32) = (0),
    /// ⚠⚠ THE PANE'S OWN PROGRAM is what owns the terminal AND THE KERNEL SAYS THE SIGNAL WOULD
    /// KILL IT, so nothing was sent — the caller asked to reach no further than
    /// [`Reach::UnderTheProgram`], and this is the case that reach exists for.
    ///
    /// # Why this is a refusal and not a stop
    ///
    /// Signalling the pane's own child can END THE PANE: its pty closes and the pane goes with it.
    /// For a caller who asked deliberately — a person typing `stop-job` — that is a consequence
    /// they chose. For a BOUNDED RUN whose clock simply ran out it is not: a routine timeout must
    /// not be able to close somebody's pane, still less the last pane of a session. **Measured**:
    /// it closed one, and the daemon exited behind it.
    ///
    /// ⚠ It is NOT *"the program is the pane's own"*, which is what this arm meant before: a peer
    /// that HANDLES the signal is signalled, because it cannot die of it. That is what makes a pane
    /// opened as its own peer — `open_pane`'s `cmd`, the preferred path — stoppable at all.
    WouldEndThePane,
    /// The pane's own program owns the terminal and **this host cannot read whether the signal
    /// would kill it**, so nothing was sent.
    ///
    /// A fact about the DEPLOYMENT rather than about this pane, which is why it is not folded into
    /// [`WouldEndThePane`](Self::WouldEndThePane): there, the kernel answered and the answer was
    /// yes; here nobody answered. See
    /// [`signal_ends`](../procfs/index.html) for which platform can say and why the other cannot.
    CannotTellIfItWouldEnd,
    /// ⚠⚠⚠⚠ **THE HOST HOLDING THE PANE COULD NOT BE ASKED AT ALL**, so nothing was sent and the
    /// work is still running.
    ///
    /// # ⚠⚠⚠ Why this arm exists in a module about a local `kill(2)`
    ///
    /// [`stop_foreground_job`] never builds it — it holds a pid and a kernel, and both of them
    /// answer. This is the vocabulary
    /// [`PaneError::NotStopped`](../../sprag_plugin/access/enum.PaneError.html) speaks, and that
    /// error is answered by every `PaneAccess` there is, INCLUDING one whose panes are held by
    /// another process. When that party cannot be reached, none of the four arms above is true:
    /// nothing was seen, refused, or declined — the request never arrived.
    ///
    /// ⚠⚠ **AND THE ARM IT WOULD OTHERWISE BORROW IS THE ONE THAT LIES.** [`Unseen`](Self::Unseen)
    /// says *this host looked at its process table and found no group*, which a caller reads as a
    /// fact about their PANE. A driver whose socket died learned nothing about the pane at all, and
    /// the two send somebody to different places — the pane's shell, or the host that is not
    /// answering.
    ///
    /// ⚠ It carries NO detail, and that is a residue rather than an oversight: this set is
    /// [`Copy`] and the whole workspace's stops are passed by value, so a payload would be paid for
    /// on every stop that succeeds. The transport's own sentence is logged where it happens, by the
    /// surface that met it.
    Unreachable,
}
}

impl Unstopped {
    /// The refusal a SENTENCE names, or [`None`] for words this vocabulary does not say.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a surface has to read a sentence back at all
    ///
    /// A host reached across a process boundary refuses in TEXT — that is all a fault carries — so
    /// a driver on the far side has the same five things to tell its caller and none of the type
    /// that separates them. Without this it would have to pick one arm for every refusal, and every
    /// choice available is a false statement about somebody's pane (see
    /// [`Unreachable`](Self::Unreachable), which is the one such statement this set now refuses to
    /// make).
    ///
    /// ⚠⚠ **DERIVED FROM [`ALL`](Self::ALL)**, on [`Stop::from_wire`]'s rule and for its reason: the
    /// set a reader can name and the set a writer can say are one list, so a sixth refusal cannot
    /// be published in a sentence nothing maps back. The `errno` arm is the one exception and it is
    /// parsed rather than enumerated, because its sentence carries a number no list can hold — the
    /// gate beside this drives that arm with several of them.
    #[must_use]
    pub fn from_sentence(sentence: &str) -> Option<Self> {
        if let Some(errno) = errno_refused(sentence) {
            return Some(Self::Refused(errno));
        }
        Self::ALL
            .into_iter()
            .find(|why| why.to_string() == sentence)
    }
}

/// The `errno` [`Unstopped::Refused`]'s sentence carries, or [`None`] for any other sentence.
///
/// ⚠ The affixes are taken from the SENTENCE ITSELF — formatted here with a number whose decimal
/// spelling cannot occur inside either side — so an edit to the wording moves the reader in the
/// same compile rather than one round later. The alternative, two string literals repeating the
/// `Display` arm, is the two-spellings-of-one-vocabulary defect this workspace keeps paying for.
fn errno_refused(sentence: &str) -> Option<i32> {
    /// A value no affix of the sentence can contain, used only to split it.
    const MARK: i32 = i32::MIN;
    let shape = Unstopped::Refused(MARK).to_string();
    let (before, after) = shape.split_once(&MARK.to_string())?;
    sentence
        .strip_prefix(before)?
        .strip_suffix(after)?
        .parse()
        .ok()
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
    /// Work the pane's own program started — **and the program ITSELF when the kernel says the
    /// signal cannot end it.**
    ///
    /// # ⛔⛔⛔⛔⛔ Not *"left alone"*, which is what this said until register item 696
    ///
    /// [`stop_foreground_job`] asks `crate::procfs::signal_ends` before refusing: a foreground
    /// group that IS the pane's program is refused as [`Unstopped::WouldEndThePane`] **only when
    /// the signal would kill it** (a `cat` under `SIGINT`), and is SIGNALLED when it would not (a
    /// shell, an agent CLI that traps it). So the two facts a cancel is about — *the turn ends* and
    /// *the program is still there for the next run* — are not divided between two kinds of pane.
    /// On the arrangement this product actually drives they hold of ONE process at once, which is
    /// what `a_cancelled_turn_reaches_the_peer_that_traps_it_and_leaves_it_running` measures.
    ///
    /// ⚠⚠ The old wording also named `Unstopped::IsTheProgram`, **a variant that does not exist** —
    /// and the rustdoc gate did not catch it. Measured 2026-08-28: a link whose leading path
    /// resolves stays quiet even when the member does not, which is register item 638's fourth
    /// blind spot. Written as plain code above for that reason.
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
            Self::WouldEndThePane => f.write_str(
                "the pane's own program is what is running and the signal would kill it, which \
                 would take the pane with it, so nothing was sent — stop it by name if that is \
                 what you want, or close the pane",
            ),
            Self::CannotTellIfItWouldEnd => f.write_str(
                "the pane's own program is what is running and this host cannot tell whether the \
                 signal would kill it, so nothing was sent — stop it by name if that is what you \
                 want",
            ),
            Self::Unreachable => f.write_str(
                "the host holding this pane could not be reached, so nothing was sent and nothing \
                 was learned about the pane — the job it was running is still running",
            ),
        }
    }
}

impl Reach {
    /// This choice's WORD on the wire — [`Stop::wire_str`]'s rule, one address over.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a REACH has to be sayable at all
    ///
    /// It is the CALLER's decision and it cannot be inferred — the type's own documentation says
    /// so, and says what the two cases look like from here (identical). A surface that offered the
    /// stop and not the reach would therefore be offering ONE of the two acts under a name that
    /// means both, and it would be the wide one: a bounded run that ran out of time would close
    /// somebody's pane, which is the outcome [`Unstopped::WouldEndThePane`] exists to have made
    /// impossible.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::UnderTheProgram => "under_the_program",
            Self::TheProgramToo => "the_program_too",
        }
    }

    /// The reach a caller's word names, or `None` for a word no surface publishes — derived from
    /// [`ALL`](Self::ALL) on [`Stop::from_wire`]'s terms.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reach| reach.wire_str() == word)
    }
}

sprag_vt::wire_words!(Reach: wire_str);

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
/// [`Reach::UnderTheProgram`] it is delivered only if the KERNEL says the program cannot die of it
/// — a shell catches `SIGINT`, so a stop at a prompt lands; a `cat` does not, so it is refused as
/// [`Unstopped::WouldEndThePane`] rather than taking the pane with it.
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
    // ⚠⚠ DECIDED BEFORE THE SIGNAL, necessarily: there is no undoing one, and the whole point of
    // the narrow reach is that the pane must still be there afterwards.
    //
    // ⚠⚠⚠ AND THE QUESTION IS THE KERNEL'S, NOT A GUESS ABOUT WHAT KIND OF PROGRAM THIS IS. The
    // first spelling of this refused whenever the group WAS the pane's own program, which left the
    // main AI-loop path — a pane opened running its peer — permanently unstoppable. What actually
    // decides whether the pane survives is whether the signal KILLS that program, and every process
    // publishes its own answer to that.
    if reach == Reach::UnderTheProgram && pgid == pane_child {
        match crate::procfs::signal_ends(pane_child, stop.signal()) {
            Some(true) => return Err(Unstopped::WouldEndThePane),
            None => return Err(Unstopped::CannotTellIfItWouldEnd),
            // It catches or ignores the signal, so it cannot die of it: the pane stays whatever
            // the program decides to do next, which is the same position a person's `Ctrl-C`
            // leaves them in.
            Some(false) => {}
        }
    }
    // Read the name BEFORE signalling: after it, `Stop::Kill`'s target may already be unreadable,
    // and a report that names nothing for the one request that always works would be worst where
    // it matters most.
    let leader = crate::processes::foreground_leader_of(pane_child);
    // SAFETY: `getpgrp` takes no arguments, touches no memory and cannot fail.
    let ours = unsafe { libc::getpgrp() };
    let Some(group) = a_group_we_may_signal(pgid, ours) else {
        // ⛔⛔⛔⛔⛔ AND SAY SO WHEN THE NUMBER WAS OURS — register item 820. Every other way this
        // returns is an ordinary answer about somebody's pane; THIS one means the process table
        // handed back a group that is this daemon's own, and a signal sent to it would have gone
        // to the daemon and everything sharing its group. That has never been observed and it is
        // exactly what nobody would notice: the refusal alone reads as `Unseen`, which is a
        // sentence about the PANE.
        if pgid == u32::try_from(ours).unwrap_or(0) {
            SIGNALLED_OURSELVES.call_once(|| {
                eprintln!(
                    "sprag: the process table named THIS PROCESS'S OWN GROUP ({ours}) as the job \
                     owning pane child {pane_child}'s terminal, so no {} was sent — sending it \
                     would have signalled this daemon and everything sharing its group. This is a \
                     defect, not a pane state (register item 820)",
                    stop.wire_str(),
                );
            });
        }
        return Err(Unstopped::Unseen);
    };
    // SAFETY: `kill` is async-signal-safe and takes no pointers. `-group` names a process GROUP —
    // the one that owns this pane's terminal, read above, and checked by `a_group_we_may_signal`
    // to be neither of `kill`'s wildcards nor our own — and the signal number comes from
    // `Stop::signal`, which is exhaustive over the closed set.
    if unsafe { libc::kill(-group, stop.signal()) } == 0 {
        return Ok(StoppedJob { stop, pgid, leader });
    }
    Err(Unstopped::Refused(
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    ))
}

/// Said once if the process table ever names our own group — see [`stop_foreground_job`].
static SIGNALLED_OURSELVES: std::sync::Once = std::sync::Once::new();

/// The group `kill` may be handed for THIS pane's job, or [`None`] for a number that would reach
/// somewhere else entirely — register item 820, and the guard `sprag_host`'s
/// `process_group_exists` has always had while the function that sends a REAL signal did not.
///
/// # ⛔⛔⛔⛔⛔ Three numbers that are not a pane's job
///
/// This value is about to be NEGATED, and `kill`'s negative arguments are not all targets:
///
/// | `pgid` | what `kill(-pgid, sig)` does |
/// | --- | --- |
/// | `0` | signals THIS PROCESS'S OWN GROUP — `-0` is `0` |
/// | `1` | signals EVERY PROCESS this user may signal — `-1` is the wildcard |
/// | `ours` | signals this daemon and everything sharing its group |
/// | above `i32::MAX` | reinterprets as a negative number, i.e. one of the above by accident |
///
/// The first two are what `process_group_exists` refuses with `pgid < 2`, in a comment that names
/// both. The third is this function's own, and it is the one no other caller could have: only the
/// code that SENDS knows which group it is sending from.
///
/// ⚠⚠ `ours` is a PARAMETER rather than a `getpgrp()` inside, so the whole decision is a pure
/// function of two numbers and the gate beside it can drive every arm — including the one this
/// process cannot be made to produce on demand.
fn a_group_we_may_signal(pgid: u32, ours: libc::pid_t) -> Option<libc::pid_t> {
    let group = libc::pid_t::try_from(pgid).ok()?;
    (group >= 2 && group != ours).then_some(group)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVERY NUMBER THAT IS NOT A PANE'S JOB IS REFUSED ONE** — register item 820, at
    /// [`a_group_we_may_signal`].
    ///
    /// # ⛔⛔⛔⛔⛔ Why each arm is here and not just the one this machine can produce
    ///
    /// The value is about to be negated into `kill`, where three of these are commands rather than
    /// targets: `0` is our own group, `1` is every process this user may signal, and `ours` is this
    /// daemon plus everything sharing its group. The fourth is the cast that manufactures one of
    /// the others out of a large `u32` — the same reinterpretation `crate::pty::one_process` closes
    /// one address over.
    ///
    /// ⚠ None of them can be produced on demand from a live process table, which is exactly why
    /// the decision is a pure function of two numbers: an arm nothing can drive is an arm nothing
    /// checks, and this workspace has paid for that four times.
    #[test]
    fn a_number_that_is_not_a_panes_job_is_never_signalled() {
        let ours: libc::pid_t = 4242;

        assert_eq!(
            a_group_we_may_signal(0, ours),
            None,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 820: `kill(-0, sig)` is `kill(0, sig)`, which signals THIS \
             PROCESS'S OWN GROUP",
        );
        assert_eq!(
            a_group_we_may_signal(1, ours),
            None,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 820: `kill(-1, sig)` signals EVERY PROCESS this user may \
             signal — the widest thing a stop request could possibly become",
        );
        assert_eq!(
            a_group_we_may_signal(u32::try_from(ours).expect("a pgid"), ours),
            None,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 820: the process table named our OWN group, and signalling it \
             would take this daemon down with the job it was aimed at",
        );
        assert_eq!(
            a_group_we_may_signal(u32::MAX, ours),
            None,
            "⛔⛔⛔ REGISTER ITEM 820: `u32::MAX` does not fit a `pid_t`, and a cast would have \
             made it `-1` — the wildcard, by accident",
        );

        assert_eq!(
            a_group_we_may_signal(2, ours),
            Some(2),
            "⚠ and the smallest group that IS one still passes, or this guard refuses the thing it \
             exists to let through",
        );
        assert_eq!(
            a_group_we_may_signal(9, ours),
            Some(9),
            "⚠ as does an ordinary one",
        );
    }

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

    /// Every REACH has a distinct wire word, and every word round-trips — the gate one address over
    /// from [`Stop`]'s, and for a sharper reason.
    ///
    /// A `Stop` that failed to round-trip is refused at the door and the caller is told. A `Reach`
    /// that failed to would be read as the OTHER reach, because a surface that cannot spell a word
    /// falls back to the wide one a person's `Ctrl-C` means — and the wide one can close somebody's
    /// pane. So the round trip is not a courtesy here; it is the only thing between an automatic
    /// stop and an ending nobody asked for.
    #[test]
    fn every_reach_has_its_own_word_and_the_word_finds_it_again() {
        for reach in Reach::ALL {
            assert_eq!(
                Reach::from_wire(reach.wire_str()),
                Some(reach),
                "{reach:?}'s own word must name it again, or a caller naming the narrow reach is \
                 silently given the wide one",
            );
            assert_eq!(
                Reach::ALL
                    .iter()
                    .filter(|other| other.wire_str() == reach.wire_str())
                    .count(),
                1,
                "{reach:?}'s word is its alone",
            );
        }
        assert_eq!(
            Reach::WIRE_WORDS.len(),
            Reach::ALL.len(),
            "every reach a caller may choose is a word a surface publishes",
        );
    }

    /// **EVERY REFUSAL A HOST WRITES DOWN IS READ BACK AS THE SAME REFUSAL** — the round trip a
    /// driver on the far side of a process boundary depends on.
    ///
    /// ⚠⚠ Walks [`Unstopped::ALL`], so a sixth refusal cannot be published in a sentence nothing
    /// maps back — and the `errno` arm is driven with SEVERAL numbers, because it is the one whose
    /// sentence carries a value and the one an enumeration cannot cover.
    #[test]
    fn every_refusal_written_as_a_sentence_reads_back_as_itself() {
        for why in Unstopped::ALL {
            assert_eq!(
                Unstopped::from_sentence(&why.to_string()),
                Some(why),
                "{why:?} is written down and read back as something else, so a run driven from \
                 another process would report the wrong reason its work is still running",
            );
        }
        for errno in [0, 1, libc::EPERM, libc::ESRCH, i32::MAX, i32::MIN] {
            let refused = Unstopped::Refused(errno);
            assert_eq!(
                Unstopped::from_sentence(&refused.to_string()),
                Some(refused),
                "the kernel's own number has to survive the trip: it is the whole content of \
                 {refused:?}",
            );
        }
        assert_eq!(
            Unstopped::from_sentence("the pane's child has already exited"),
            None,
            "⚠ and a sentence this vocabulary does not say is not GUESSED at — a near miss read as \
             a refusal would be a fact about a pane that nobody stated",
        );
    }

    /// Each DELIVERY reads as its own verb, and none of them leaks a variant name.
    ///
    /// ⚠ Written because the sweep found the `Terminate` and `Kill` arms built by NOTHING: the CLI
    /// and the agent surface both render whatever the daemon echoes back, and every gate on those
    /// mouths drives `interrupt`. So two thirds of the text a caller reads for a stop had no reader
    /// at all — the shape this workspace keeps paying for, one arm at a time.
    ///
    /// ⚠⚠ DISTINCTNESS is the half a per-arm check cannot make: three requests rendering one verb
    /// would satisfy every shape claim while telling a caller nothing about which they got.
    #[test]
    fn every_stop_reads_as_its_own_verb() {
        let mut said: Vec<String> = Vec::new();
        for stop in Stop::ALL {
            let verb = stop.to_string();
            assert!(
                !verb.is_empty() && verb.starts_with(char::is_lowercase),
                "{stop:?} must read as prose inside a report, not as {verb:?}",
            );
            assert_ne!(
                verb,
                format!("{stop:?}"),
                "{stop:?} hands a caller its Rust shape",
            );
            assert!(
                !said.contains(&verb),
                "{stop:?} repeats a verb another request already uses, so a caller cannot tell \
                 which they got: {verb}",
            );
            said.push(verb);
        }
        assert_eq!(
            said.len(),
            Stop::ALL.len(),
            "every request in the type was asked, not a hand-picked few",
        );
        // ⚠ And each reads as the END of *"the job was …"*, which is the sentence they are
        // composed into by `Signalled`'s own `Display`. A verb that does not fit there reaches a
        // person as two fragments.
        assert_eq!(Stop::Interrupt.to_string(), "interrupted");
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
    /// ⚠⚠⚠ **AND THE FIRST FIX WAS TOO BLUNT.** It refused whenever the group WAS the pane's own
    /// program, which made a pane opened running its peer — `open_pane`'s `cmd`, the preferred path
    /// — permanently unstoppable: a timed-out run could only report that its peer was still going.
    /// **What decides whether the pane survives is not what KIND of program it is; it is whether
    /// the signal KILLS it, and every process publishes its own answer.** Measured on two processes
    /// whose command line is the identical string `sleep 300`: `SigIgn: …0000` for the plain one
    /// and `SigIgn: …0002` — the `SIGINT` bit — for the one whose shell trapped it.
    ///
    /// ⚠ Every pane here has its own child as the foreground group, which is the condition being
    /// discriminated. A pane running a shell would have the job one level down and no arm would
    /// fire, which is exactly the case that would make this gate vacuous.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_stop_that_must_not_end_the_pane_refuses_only_what_the_signal_would_kill() {
        use crate::{CommandBuilder, PanePty};
        use std::time::{Duration, Instant};

        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            let pty = PanePty::spawn(command, 30, 4).expect("spawn a pty");
            let child = pty.pid().expect("a live child");
            (pty, child)
        };
        let until = |within: Duration, mut ready: Box<dyn FnMut() -> bool>| {
            let start = Instant::now();
            while start.elapsed() < within {
                if ready() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        // ⚠⚠ THE PANE'S OWN CHILD **AND** THE NAME IT EXEC'D INTO. Waiting only for the pane to own
        // its terminal is not enough and this test proved it under load: a shell leads its own
        // group from the instant it is exec'd, so the disposition would be read while the process
        // is still `/bin/sh` — **and a shell CATCHES `SIGINT`**, so the control would be delivered
        // instead of refused. Measured, as a whole-suite red:
        // `Ok(StoppedJob { leader: Some(JobProcess { name: "sh", argv: ["/bin/sh", "-c", "exec
        // sleep 300"] }) })` where `Err(WouldEndThePane)` was expected. The same race was closed in
        // the agent's own gate and left open here, which is why one fixture is not a rule.
        let became = |child: u32, name: &'static str| {
            move || {
                crate::foreground_leader_of(child)
                    .is_some_and(|job| job.pid == child && job.name == name)
            }
        };
        let owns_its_terminal =
            |child| crate::foreground_leader_of(child).is_some_and(|job| job.pid == child);

        // ⚠ THE CONTROL: nothing handles the signal, so the kernel would end it — and ending it
        // ends the pane. The narrow reach refuses BY NAME and nothing is sent.
        let (doomed, child) = spawn("exec sleep 300");
        assert!(
            until(Duration::from_secs(15), Box::new(became(child, "sleep"))),
            "the control fixture never BECAME its job, so the disposition below is the shell's",
        );
        // ⚠⚠ THE ARM IS THE PLATFORM'S OWN, and asserting one spelling on both runners is the
        // mistake R362 could only find by pushing. Linux reads the disposition and answers that the
        // signal WOULD kill it; macOS cannot read it at all (`libc` declares no `kinfo_proc` for
        // Apple) and answers that it cannot tell. **The SAFETY property is the same on both** — the
        // stop is refused and nothing is sent — and that is what the assertion below is really for.
        let refused = stop_foreground_job(child, Stop::Interrupt, Reach::UnderTheProgram);
        assert_eq!(
            refused,
            Err(if cfg!(target_os = "linux") {
                Unstopped::WouldEndThePane
            } else {
                Unstopped::CannotTellIfItWouldEnd
            }),
            "a stop that may not end the pane must refuse, in this platform's own terms",
        );
        assert_eq!(
            doomed.exit_status(),
            None,
            "⚠⚠ and NOTHING WAS SENT — the pane's program is still running, which is the whole \
             point of the narrow reach and is true on every platform",
        );

        // ⚠ AND THE WIDE REACH IS THE SAME CALL WITH THE CALLER'S OWN DECISION IN IT.
        let (wide, child) = spawn("exec sleep 300");
        assert!(
            until(Duration::from_secs(15), Box::new(became(child, "sleep"))),
            "the wide fixture never BECAME its job",
        );
        let stopped = stop_foreground_job(child, Stop::Interrupt, Reach::TheProgramToo)
            .expect("a caller who asked for the program too reaches it");
        assert_eq!(
            stopped.pgid, child,
            "and what it reached IS the pane's program"
        );
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| wide.exit_status().is_some()),
            ) && wide.exit_status().is_some_and(|exit| exit.signal.is_some()),
            "⚠⚠ the pane's own program ended by a signal — the consequence the narrow reach exists \
             to keep away from a run that merely ran out of time",
        );

        // ⚠⚠⚠ THE SUBJECT: a pane whose own program CATCHES the signal. It cannot die of it, so
        // the NARROW reach delivers — the peer's current work ends and the peer stays, which is
        // exactly what a cut-short AI turn needs and what the first fix could not do.
        //
        // ⚠⚠ IT PRINTS `READY` AFTER INSTALLING THE TRAP, AND THE GATE WAITS FOR THAT. Waiting for
        // the pane to own its terminal is not enough: a shell leads its own group from the instant
        // it is exec'd, seconds before it has parsed the script that installs the handler — so the
        // disposition would be read while the program still dies of the signal. The first spelling
        // of this gate did exactly that and reported `WouldEndThePane`, which was the honest answer
        // at that instant. ⚠ The same is true in production and is the safe direction: a peer that
        // has not yet taken its handler is one a narrow stop leaves alone.
        let (peer, child) =
            spawn("trap 'printf CAUGHT\\n' INT; printf READY\\n; while :; do sleep 300; done");
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| peer.with_screen(|screen| screen.full_text().contains("READY"))),
            ),
            "the subject fixture never installed its handler",
        );
        assert!(
            owns_its_terminal(child),
            "and it owns its own terminal, which is the condition being discriminated",
        );
        // ⚠⚠⚠ AND HERE THE TWO PLATFORMS GENUINELY DIFFER, so the gate asks each for its own
        // answer rather than baking one. **This is the capability macOS does not have**: it cannot
        // read the disposition, so it refuses and reports — exactly as it did before any of this
        // existed. Registered as owed; asserted here so the divergence is a stated fact rather than
        // a surprise on a runner.
        if !cfg!(target_os = "linux") {
            assert_eq!(
                stop_foreground_job(child, Stop::Interrupt, Reach::UnderTheProgram),
                Err(Unstopped::CannotTellIfItWouldEnd),
                "a host that cannot read a disposition must refuse rather than guess",
            );
            assert_eq!(
                peer.exit_status(),
                None,
                "and the pane is left exactly as it was found",
            );
            return;
        }
        let stopped = stop_foreground_job(child, Stop::Interrupt, Reach::UnderTheProgram)
            .expect("a program that cannot die of the signal is signalled");
        assert_eq!(
            stopped.pgid, child,
            "and what it reached IS the pane's own program",
        );
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| peer.with_screen(|screen| screen.full_text().contains("CAUGHT"))),
            ),
            "⚠⚠ THE SIGNAL LANDED AND THE PEER'S OWN HANDLER RAN — the turn ending rather than \
             the program",
        );
        assert_eq!(
            peer.exit_status(),
            None,
            "⚠⚠⚠ AND THE PANE IS STILL THERE. This is the residue the first fix could only \
             report: a pane opened running its own peer is stoppable after all.",
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

        pty.write(&[0x03], crate::pane_pty::Hand::APerson)
            .expect("write the interrupt byte");
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

    /// ⚠⚠⚠ **A GROUP WHOSE LEADER HAS GONE IS STILL A JOB, AND THE STOP STILL LANDS ON IT** —
    /// [`StoppedJob::leader`]'s `None`, built rather than registered.
    ///
    /// This was filed as *"built by nothing"* with the note that it needed a fixture killing a
    /// leader mid-job. It needs no such thing: **a shell pipeline produces it by itself.** The
    /// group's leader is the pipeline's FIRST process, so a pipeline whose head finishes first
    /// leaves a live group led by a pid the kernel has already reaped — which is exactly the state
    /// [`foreground_leader_of`](crate::foreground_leader_of) answers `None` for, while the terminal
    /// still belongs to that group.
    ///
    /// ⚠ Both halves are asserted, because either alone is consistent with the bug: the terminal
    /// belongs to a group that is NOT the pane's own child (so a job really is running), and the
    /// leader is unreadable (so this is the arm under test). Then the stop is delivered anyway —
    /// **a report that refused to act because it could not narrate would be the tail wagging the
    /// dog**, which is what this arm's doc claims and what nothing had checked.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_job_whose_leader_was_reaped_is_still_signalled_and_reported_without_a_name() {
        use crate::{CommandBuilder, PanePty};
        use std::time::{Duration, Instant};

        // ⚠ `bash -i`: JOB CONTROL is what puts the pipeline in its own process group, which is the
        // whole condition. A non-interactive shell runs it in the shell's group and the leader is
        // the shell, which never goes.
        let mut command = CommandBuilder::new("/bin/bash");
        command.arg("--norc");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");
        let child = pty.pid().expect("a live child");
        let until = |within: Duration, mut ready: Box<dyn FnMut() -> bool>| {
            let start = Instant::now();
            while start.elapsed() < within {
                if ready() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(move || crate::foreground_pgid_of(child) == Some(child)),
            ),
            "the shell must reach its own prompt first",
        );

        // The head of the pipeline leads the job and finishes first; the tail keeps the group.
        pty.write(b"sleep 0.2 | sleep 300\n", crate::pane_pty::Hand::APerson)
            .expect("write");
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(move || {
                    let pgid = crate::foreground_pgid_of(child);
                    pgid.is_some_and(|pgid| pgid != child)
                        && crate::foreground_leader_of(child).is_none()
                }),
            ),
            "the fixture never reached a live job whose leader was gone, so the arm under test \
             was never entered",
        );
        let pgid = crate::foreground_pgid_of(child).expect("the terminal still belongs to the job");

        let stopped = stop_foreground_job(child, Stop::Interrupt, Reach::TheProgramToo)
            .expect("⚠⚠ a job with no readable leader is STILL a job, and the stop must land");
        assert_eq!(stopped.pgid, pgid, "and it reached that group");
        assert_eq!(
            stopped.leader, None,
            "⚠ with NO name to give, which is the fact this arm exists to carry rather than a \
             failure to report",
        );
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(move || crate::foreground_pgid_of(child) == Some(child)),
            ),
            "⚠⚠ AND THE WORLD AGREES: the job ended and the shell took its terminal back",
        );
    }
}
