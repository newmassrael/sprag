//! The OS pseudoterminal, and the child born on the far side of it.
//!
//! This is the platform boundary sprag owns. Everything OS-shaped about starting a pane lives here
//! and nowhere else: allocating the device, resizing it, naming it, and creating the child with a
//! controlling terminal.
//!
//! # Why sprag owns this
//!
//! It was `portable-pty` until R336. The thing that moved it is one requirement that dependency
//! cannot express: **a pane's child must join its cgroup before it execs.** Join it afterwards and
//! the child has already had time to fork — measured, on a pane running `sh -c 'sleep 60 & sleep
//! 60'`, BOTH children landed in the daemon's own cgroup instead of the pane's, so work a person
//! started in their pane was charged to the daemon.
//!
//! The only moment that can be fixed is between `fork` and `exec`, and `portable-pty` keeps that
//! moment to itself — `as_command` is `pub(crate)`, `SlavePty` returns no descriptor, and its
//! `pre_exec` closure is not extensible. The lesson generalises past cgroups: **a platform boundary
//! you cannot reach into is not a boundary you own**, and the next thing that has to happen before
//! `exec` would have hit the same wall.
//!
//! Rust's own `std::process::Command` still does the heavy lifting — `fork`, `PATH` search, `exec`,
//! and reaping — and it lends out the pre-exec seam. So what is written here is the part std does
//! not do: the device, and the controlling-terminal handshake. Ghostty, which owns its spawn
//! outright, has to write the `PATH` walk and the reaping too (`src/Command.zig`).
//!
//! # Where a second platform goes
//!
//! Unix only, and honestly so — `README.md` says the same. A Windows arm belongs beside
//! `Pty::open` and `AttachedPty::spawn` as a `#[cfg(windows)]` sibling built on ConPTY; nothing above
//! this module names a file descriptor, so it would not have to change. What still blocks Windows
//! is not here: the wire is a Unix domain socket in three other crates.

#![cfg(unix)]

use std::ffi::{CStr, OsStr};
use std::fs::File;
use std::io;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use crate::command::CommandBuilder;

/// Whether a pane's child reached the cgroup that was opened for it — [`AttachedPty::spawn`]'s second
/// answer.
///
/// # Why the birth answers this instead of failing
///
/// The join is done by the CHILD, between `fork` and `exec`, which is what makes it race-free
/// (R336). The only channel out of that moment that `std::process::Command` offers is the
/// `pre_exec` closure's own `Err`, and that one is FATAL by construction: std turns it into a
/// failed spawn, so reporting a refused join through it costs the person their pane. A terminal
/// that will not open a pane because a resource nicety was declined has the trade exactly
/// backwards, and `PaneHomes::open`'s contract says as much in its own words — *never fails a
/// birth*.
///
/// So the refusal comes back beside the child rather than instead of it, and the caller decides.
/// Every arm here is ORDINARY on some host this product supports, which is why this is a value and
/// not an error type: [`NotAsked`](Self::NotAsked) is every macOS pane and every pane on a daemon
/// systemd would not delegate to, and [`Refused`](Self::Refused) is every pane on a host whose
/// daemon sits outside the subtree it was given.
#[derive(Debug)]
pub enum Joined {
    /// No cgroup was offered, so there was nothing to join — this host enforces nothing, or this
    /// pane was never placed. The state every pane was in before R336.
    NotAsked,
    /// The child was in its pane's cgroup before its first instruction.
    Joined,
    /// A cgroup was opened for this child and the kernel refused to admit it, carrying the reason
    /// the kernel gave.
    ///
    /// Opening the file and writing to it are two different checks and the second is the one that
    /// fails here: cgroup v2's delegation containment rule compares the WRITER's own cgroup against
    /// the destination, so no inspection of the destination can predict it. Measured on GitHub's
    /// Linux runner, where the answer is `EACCES` for every pane.
    Refused(io::Error),
}

sprag_vt::closed_set! {
/// Who puts a pane's own input back on its screen.
///
/// # ⚠⚠⚠ Why a caller must be able to ask
///
/// **A pseudoterminal echoes what is written to it, and on the grid that echo is ordinary output.**
/// Everything above this crate that confirms an injection by LOOKING AT THE SCREEN is therefore
/// making a claim it cannot support unless it knows which of these two it has — and until this
/// existed, none of them could:
///
/// * With [`ByTheTerminal`](Self::ByTheTerminal), the line discipline paints every byte the instant
///   it reaches the device, **before the program has read one and whether or not it ever will**. A
///   read-back that finds the text has learned that the TERMINAL is alive. Measured: a confirmed
///   delivery into a pane running `sleep 60`, in 20 ms, over a peer that never read a byte.
/// * With [`ByTheProgram`](Self::ByTheProgram), the program has taken its terminal off echo, so
///   anything on that screen was PRINTED BY IT — which is exactly the evidence the read-back wanted.
///
/// This is read from the kernel (`termios`' `ECHO`, through the pane's own device) rather than
/// assumed, because it is the program's to change at any moment and every interactive agent does
/// change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEcho {
    /// The terminal echoes: what is written into this pane appears on it without the program's
    /// involvement.
    ByTheTerminal,
    /// Echo is off: what appears on this pane was printed by the program running in it.
    ByTheProgram,
}
}

impl PaneEcho {
    /// This answer's WORD on the wire — the one place the variant → name mapping lives, so no
    /// surface spells a variant itself ([`SignalKey::wire_str`]'s rule).
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::ByTheTerminal => "terminal",
            Self::ByTheProgram => "program",
        }
    }

    /// The answer a word names, or `None` for a word no surface publishes. DERIVED from
    /// [`ALL`](Self::ALL), so what a reader may parse and what a surface publishes cannot drift.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|echo| echo.wire_str() == word)
    }
}

sprag_vt::closed_set! {
/// Whether a `Ctrl-D` written into a pane will END the program's input, or arrive as a byte.
///
/// # ⚠⚠⚠ Why a caller must be able to ask
///
/// An end-of-input is not a character — it is a CONDITION the line discipline raises when it sees
/// the EOF character, **and only while the terminal is in canonical mode.** A program that took its
/// terminal raw (every full-screen agent, every editor) gets `0x04` as an ordinary byte and decides
/// for itself what it means.
///
/// So a caller that ends a question with `Ctrl-D` and then waits for the peer to finish is, on a
/// raw pane, waiting for something it did not ask for. Measured, on `stty raw -echo; exec cat` with
/// the agent adapter's DEFAULT `eof`: the run spent its whole reply timeout, converged, published
/// the peer's echo of the prompt as the model's answer, and explained itself with *"the peer had
/// not finished"* — a sentence about the PEER's speed, for a cause that is the TERMINAL's mode and
/// was knowable before the wait began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneEndOfInput {
    /// Canonical mode: the line discipline turns the EOF character into end-of-input, so a peer
    /// reading until end-of-input is told the input is over.
    EndsTheInput,
    /// Raw mode: it is delivered as an ordinary byte, and what it means is the program's business.
    IsJustAByte,
}
}

impl PaneEndOfInput {
    /// This answer's WORD on the wire — [`PaneEcho::wire_str`]'s rule, one enum along.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::EndsTheInput => "ends_the_input",
            Self::IsJustAByte => "just_a_byte",
        }
    }

    /// The answer a word names, or `None` for a word no surface publishes — DERIVED from
    /// [`ALL`](Self::ALL).
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|end| end.wire_str() == word)
    }
}

sprag_vt::closed_set! {
/// WHICH SIGNAL a terminal's line discipline raises for one of its signal characters.
///
/// Three, because `termios` has three such characters and they ask a job for three different
/// things — the same distinction [`Stop`](crate::Stop) draws for the signal a caller sends
/// itself, arrived at from the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKey {
    /// The INTR character — `Ctrl-C` on a terminal nobody has reconfigured (`SIGINT`).
    Interrupt,
    /// The QUIT character — `Ctrl-\` (`SIGQUIT`).
    Quit,
    /// The SUSP character — `Ctrl-Z` (`SIGTSTP`), which STOPS the job rather than ending it.
    Suspend,
}
}

impl SignalKey {
    /// This key's WORD on the wire — the one place the variant → name mapping lives, so no
    /// surface spells a variant itself ([`Stop::wire_str`](crate::Stop::wire_str)'s rule).
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Quit => "quit",
            Self::Suspend => "suspend",
        }
    }

    /// The key a caller's word names, or `None` for a word no surface publishes. DERIVED from
    /// [`ALL`](Self::ALL), so the set a caller may say and the set a surface publishes cannot
    /// drift apart.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.wire_str() == word)
    }

    /// How a person names the chord that means this, for a sentence a human reads.
    #[must_use]
    pub const fn chord(self) -> &'static str {
        match self {
            Self::Interrupt => "Ctrl-C",
            Self::Quit => "Ctrl-\\",
            Self::Suspend => "Ctrl-Z",
        }
    }

    /// The character this key CONVENTIONALLY is — what a person pressing that chord produces, and
    /// what every surface offering it means by it.
    ///
    /// ⚠ This is a fact about the CALLER'S INTENT, not about the device. What the device does with
    /// the byte is [`PaneSignalKeys::raises`], read from the kernel — and the whole point of asking
    /// both is that they can DISAGREE.
    #[must_use]
    pub const fn conventional_byte(self) -> u8 {
        match self {
            Self::Interrupt => 0x03,
            Self::Quit => 0x1c,
            Self::Suspend => 0x1a,
        }
    }
}

sprag_vt::wire_words!(SignalKey: wire_str);

/// Whether the characters that MEAN a signal raise one in a pane, and which characters those are.
///
/// # ⚠⚠⚠ Why a caller must be able to ask
///
/// **Writing `0x03` into a pane is not interrupting its job**, and the write reports success
/// either way. What turns the byte into a `SIGINT` is the line discipline, and it does so only
/// while `ISIG` is set — every full-screen program, every editor and every interactive agent CLI
/// clears it on startup. Measured (R363): a pane running `stty -isig; sleep 300`, sent `C-c`
/// through this product's own `send-keys`, echoes `^C` and the `sleep` lives on.
///
/// So a caller that sends `Ctrl-C` and then waits for the job to end is, on such a pane, waiting
/// for something it never asked for — [`PaneEndOfInput`]'s failure exactly, one character over.
/// This is the reading that lets the surface SAY so at the moment of the write, and
/// [`Stop`](crate::Stop) is what it points the caller at instead.
///
/// # ⚠⚠ Why the CHARACTERS and not just the flag
///
/// `ISIG` alone answers *"does this terminal raise signals?"*, and a surface that stays quiet
/// whenever it is set is asserting *"your `Ctrl-C` became a signal"* — a claim it has not checked.
/// The characters are the device's to rebind (`stty intr ^X`) or to disable outright, and they
/// come out of the SAME `tcgetattr`, so reading them costs nothing and closes a second way for the
/// silence to be false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSignalKeys {
    /// `ISIG` is set: the line discipline turns each character it names into that signal. A field
    /// is `None` where the device has DISABLED that character, which raises nothing at all.
    RaisedByTheTerminal {
        /// The INTR character, or `None` where it is disabled.
        interrupt: Option<u8>,
        /// The QUIT character, or `None` where it is disabled.
        quit: Option<u8>,
        /// The SUSP character, or `None` where it is disabled.
        suspend: Option<u8>,
    },
    /// `ISIG` is clear — the program took its terminal raw, so NO character raises a signal and
    /// every one of them reaches the program as an ordinary byte.
    DeliveredAsBytes,
}

sprag_vt::closed_set! {
/// WHY a byte a caller sent MEANING a signal did not become one.
///
/// Two, because they are two different states of the pane and a caller acts on them differently —
/// and because a surface that reported only *"no signal"* would leave a reader to guess which,
/// when one of them says the program is full-screen and the other says the terminal was
/// reconfigured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unraised {
    /// The program took its terminal RAW (`ISIG` off), so no input character raises a signal here
    /// — the state every editor, every full-screen TUI and every interactive agent CLI is in.
    TerminalRaisesNone,
    /// Signals ARE raised here, but not for this byte: the terminal's own character for it is
    /// another one, or that character is disabled.
    NotItsCharacter,
}
}

impl Unraised {
    /// This cause's WORD on the wire — [`SignalKey::wire_str`]'s rule.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::TerminalRaisesNone => "raw",
            Self::NotItsCharacter => "unbound",
        }
    }

    /// The cause a caller's word names, or `None` for a word no surface publishes. DERIVED from
    /// [`ALL`](Self::ALL), so the published set and the readable set cannot drift apart.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|why| why.wire_str() == word)
    }
}

sprag_vt::wire_words!(Unraised: wire_str);

impl std::fmt::Display for Unraised {
    /// The clause a report uses, reading as *"nothing was raised, because …"*.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TerminalRaisesNone => {
                "the program running there has taken its terminal raw, so nothing typed at it \
                 raises a signal"
            }
            Self::NotItsCharacter => {
                "that terminal's character for it is a different one, or is disabled"
            }
        })
    }
}

impl PaneSignalKeys {
    /// WHY writing `byte` into this pane raises no signal, or `None` when it DOES raise one.
    ///
    /// ⚠ DERIVED from [`raises`](Self::raises) rather than deciding a second time, so the answer
    /// *"nothing was raised"* and the answer *"here is why"* cannot disagree.
    #[must_use]
    pub fn unraised(&self, byte: u8) -> Option<Unraised> {
        if self.raises(byte).is_some() {
            return None;
        }
        Some(match self {
            Self::DeliveredAsBytes => Unraised::TerminalRaisesNone,
            Self::RaisedByTheTerminal { .. } => Unraised::NotItsCharacter,
        })
    }

    /// Which signal writing `byte` into this pane RAISES, or `None` when it raises none.
    ///
    /// ⚠ `None` is the answer that matters: it means the byte reaches the program as input. A
    /// caller that sent it meaning to stop a job has not stopped one.
    #[must_use]
    pub fn raises(&self, byte: u8) -> Option<SignalKey> {
        let Self::RaisedByTheTerminal {
            interrupt,
            quit,
            suspend,
        } = self
        else {
            return None;
        };
        [
            (*interrupt, SignalKey::Interrupt),
            (*quit, SignalKey::Quit),
            (*suspend, SignalKey::Suspend),
        ]
        .into_iter()
        .find_map(|(character, key)| (character == Some(byte)).then_some(key))
    }
}

/// A `c_cc` slot's character, or `None` where the device has no character in it.
///
/// # ⚠⚠ Why a SHAPE test and not `_POSIX_VDISABLE`
///
/// The disabled value is not the same number everywhere — `0` on Linux, `0xff` on the BSDs and
/// macOS — and `libc` publishes `_POSIX_VDISABLE` for the BSDs and Android but **not for
/// linux-gnu**, so neither number can be named portably and picking one would be a defect on the
/// other of this project's two targets. Both fall outside the test that actually matters: a signal
/// character is one a keyboard produces with `Ctrl`, so it is a control character — `0x01..=0x1F`,
/// or `0x7F` for the terminals that bind one to `Ctrl-?`. `0x00` and `0xff` are excluded by that
/// test whichever platform meant which by them.
const fn signal_character(raw: libc::cc_t) -> Option<u8> {
    match raw {
        0x01..=0x1f | 0x7f => Some(raw),
        _ => None,
    }
}

/// A read-only handle on a pane's terminal — for ASKING the kernel about the device, never for
/// reading or writing it.
///
/// A separate type rather than a bare descriptor because the distinction is the whole point: the
/// pane's other two handles on this device are a reader thread blocked in `read` and a coalescer
/// that resizes it, and a third one that consumed bytes would silently steal a program's input.
/// Nothing here can: the only operations are `ioctl`s that ask.
#[derive(Debug)]
pub struct TerminalQuery(OwnedFd);

impl TerminalQuery {
    /// Who echoes this pane's input — [`PaneEcho`] — or `None` where the kernel will not say.
    ///
    /// ⚠ `None` is not "the terminal does not echo": it is *this platform's device would not answer
    /// the question*, and a caller that reads it as the negative would draw exactly the false
    /// confidence this type exists to prevent. Both readings are asserted in the gate, in each
    /// platform's own terms.
    #[must_use]
    pub fn echo(&self) -> Option<PaneEcho> {
        Some(if self.local_modes()? & libc::ECHO == 0 {
            PaneEcho::ByTheProgram
        } else {
            PaneEcho::ByTheTerminal
        })
    }

    /// Whether a `Ctrl-D` written into this pane ends its program's input — [`PaneEndOfInput`] —
    /// or `None` where the kernel will not say.
    ///
    /// ⚠ `None` carries the same warning [`echo`](Self::echo) does: it is *this platform's device
    /// would not answer*, never the negative.
    #[must_use]
    pub fn end_of_input(&self) -> Option<PaneEndOfInput> {
        Some(if self.local_modes()? & libc::ICANON == 0 {
            PaneEndOfInput::IsJustAByte
        } else {
            PaneEndOfInput::EndsTheInput
        })
    }

    /// Whether the characters that mean a signal RAISE one in this pane — [`PaneSignalKeys`] — or
    /// `None` where the kernel will not say.
    ///
    /// ⚠ `None` carries the same warning [`echo`](Self::echo) does: it is *this platform's device
    /// would not answer*, never the negative. Reading it as *"no signals"* would put the false
    /// confidence back that this exists to remove.
    #[must_use]
    pub fn signal_keys(&self) -> Option<PaneSignalKeys> {
        let attributes = self.attributes()?;
        if attributes.c_lflag & libc::ISIG == 0 {
            return Some(PaneSignalKeys::DeliveredAsBytes);
        }
        Some(PaneSignalKeys::RaisedByTheTerminal {
            interrupt: signal_character(attributes.c_cc[libc::VINTR]),
            quit: signal_character(attributes.c_cc[libc::VQUIT]),
            suspend: signal_character(attributes.c_cc[libc::VSUSP]),
        })
    }

    /// The device's local mode flags (`c_lflag`), or `None` where it will not answer.
    fn local_modes(&self) -> Option<libc::tcflag_t> {
        Some(self.attributes()?.c_lflag)
    }

    /// The device's whole `termios`, or `None` where it will not answer.
    ///
    /// One `tcgetattr` behind every question above, so a reading is never assembled from two calls
    /// that could straddle a program changing its terminal — and so a new question costs a flag
    /// test rather than another syscall. ⚠ That is why [`signal_keys`](Self::signal_keys) takes the
    /// WHOLE structure rather than calling [`local_modes`](Self::local_modes) and then reading
    /// `c_cc`: the flag and the characters it governs must come from one reading, or a program
    /// re-configuring its terminal between them yields an answer that was never true.
    fn attributes(&self) -> Option<libc::termios> {
        let mut attributes: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is open for the life of `self`, and `tcgetattr` only fills in the
        // fully-owned `termios` handed to it.
        if unsafe { libc::tcgetattr(self.0.as_raw_fd(), &raw mut attributes) } != 0 {
            return None;
        }
        Some(attributes)
    }
}

/// ⛔⛔⛔⛔⛔ **WHAT THE HOST SAYS ABOUT ITS PSEUDOTERMINAL POOL, FOR THE ONE READER WHO MEETS A
/// REFUSAL** — register item 776, arm (d), and the sentence [`Pty::open`]'s error carries.
///
/// # ⚠⚠⚠⚠⚠ Three sentences, because *unknown* is not *zero* and must not read as one
///
/// A pool that answers `62 of 4096` tells a reader the refusal was not exhaustion. One that answers
/// only a ceiling tells them the ceiling and, explicitly, that the in-use half is **not being
/// withheld by this code but not published by their kernel**. One that answers neither says that
/// too, rather than leaving a bare `Device not configured` for somebody to complete from memory —
/// which is exactly what happened: it was completed as *the pool was exhausted*, and re-measuring
/// found nothing supporting it.
#[must_use]
fn pool_sentence(pool: crate::procfs::PtyPool) -> String {
    let host = match (pool.in_use, pool.max) {
        (Some(in_use), Some(max)) => {
            format!("this host's pty pool was {in_use} of {max} in use when it refused")
        }
        (Some(in_use), None) => format!(
            "this host had {in_use} pty(s) in use when it refused and does not publish how many it \
             allows"
        ),
        (None, Some(max)) => format!(
            "this host allows at most {max} pty(s) and does not publish how many were in use when \
             it refused, so the host's own total cannot be read here"
        ),
        (None, None) => "this host publishes neither the size of its pty pool nor how much of it \
             was in use, so the host's own total cannot be read here"
            .to_owned(),
    };
    format!(
        "{host}; {}",
        ours_sentence(OPEN_HERE.live(), OPEN_HERE.opened(), pool.max)
    )
}

/// **HOW MANY PSEUDOTERMINALS THIS PROCESS ITSELF WAS HOLDING** — register item 776, arm (d), and
/// the half a Darwin host does not publish.
///
/// # ⛔⛔⛔⛔⛔ Why this exists, and why it is not the stand-in that was refused
///
/// The host half above answers *how full the pool was* on Linux and, on macOS, only *how big it
/// is*. So the sentence a macOS reader got was a ceiling and an apology — **and that is not enough
/// to tell exhaustion from anything else**, which is the whole of this arm's remaining debt: the
/// round that built the host half wrote *"the next macOS failure brings the answer"*, and on macOS
/// it does not.
///
/// The refused stand-in was counting `/dev/ttys*`, which is a claim about devfs that nothing here
/// can check. **This is not that.** It is this process's own ledger — incremented where a
/// pseudoterminal is opened and decremented where it is dropped — so it is checkable by
/// construction and true on every platform.
///
/// # ⚠⚠⚠⚠ It cuts ONE WAY, and the sentence says so
///
/// A process holding 3 of a 127-place pool did not exhaust it **by itself**; a process holding 120
/// very likely did. But nothing here can see other processes, so a small number does not RULE OUT
/// exhaustion — somebody else may hold the rest. The sentence states the count and that limit
/// rather than letting a reader complete it from memory, which is exactly the failure this arm was
/// filed for.
/// # ⛔⛔⛔⛔⛔ AND HOLDING IS NOT THE SAME QUESTION AS HAVING OPENED — register item 814
///
/// Measured on `30cb15d`'s macOS job: the refusal said **46 of at most 511**, and 46 is far too few
/// to have used the pool up. That reads as *somebody else held the rest* — and it is only one of
/// two worlds. The other is that this host does not hand a CLOSED pseudoterminal straight back, in
/// which case what matters is not how many we were holding but how many we had ever taken, and a
/// suite that opens and closes hundreds would exhaust a 511-name space while holding 46.
///
/// The live count alone cannot tell those apart, and they want opposite repairs — one is somebody
/// else's process, the other is this suite's own churn. So both numbers are said.
///
/// ⚠ The sentence STATES the two questions rather than answering them: this code cannot see the
/// host's reuse policy, and a sentence that picked a side would be the completing-from-memory this
/// whole family of arms exists to stop.
///
/// # ⛔⛔⛔⛔⛔ AND IT DOES THE ONE PIECE OF ARITHMETIC A READER OTHERWISE DOES BY HAND
///
/// Register item 814's second reading. The two numbers above were said and the reader still had to
/// compare `opened` against the ceiling in the CLAUSE BEFORE THIS ONE to learn anything — and
/// **measured 2026-09-01, that is exactly what happened**: `6633099`'s macOS job printed the
/// balanced sentence at `holding 30 / opened 422 / at most 511`, and settling it took an `strace`
/// of the whole suite on another machine (**847 opens in one process**, which is 1.66× that
/// namespace). The sentence had every number it needed and made the reader do the subtraction.
///
/// So `ceiling` comes in and there are THREE readings, not one:
///
/// | what is true | what it means |
/// | --- | --- |
/// | `opened >= ceiling` | this process has taken the whole namespace at least once over, so on a host that does not return a closed pseudoterminal promptly **we are sufficient on our own** |
/// | `opened < ceiling` | **settles nothing** — said as a comparison, with both reasons it settles nothing |
/// | ceiling unknown | said outright, because *not published* is not *not exceeded* |
///
/// ⚠⚠ IT STILL PICKS NO SIDE ABOUT THE HOST'S REUSE POLICY, which is the thing this code cannot
/// see. What it says is the CONDITIONAL — *if this host does not recycle, this was enough* — which
/// is a fact about our own demand and is checkable here.
///
/// # ⛔⛔⛔⛔⛔ AND THE UNDER-THE-CEILING ARM MAY NOT CONCLUDE, WHICH IS THIS FAMILY'S WHOLE RULE
///
/// The first version of this arm read *"our own churn cannot have used the namespace up whatever
/// its reuse policy: something else was holding it"* — and that is
/// [`crate::procfs::PtyPool`]'s own recorded defect wearing the opposite sign. That doc says it in
/// as many words: an `ENXIO` on macOS was filed as *"the runner's pty pool was exhausted"* and
/// **that was an inference, not a reading**. Concluding *somebody else held it* from
/// `opened < kern.tty.ptmx_max` is the same move: the published ceiling has never been shown to be
/// the limit that produced the refusal.
///
/// ⚠⚠⚠⚠ **AND IT WAS WRONG ON THE ONE TRIPLE THIS ITEM WAS FILED OVER.** `6633099`'s macOS job
/// refused at `holding 30 / opened 422 / at most 511`, and 422 < 511 — so that sentence would have
/// printed *something else was holding it* over the exact failure whose cause the same round
/// measured as this suite's own churn (**847 opens in one process**, of which 422 is barely half:
/// the count is a RUNNING TOTAL and the process had not finished). A reading that denies the
/// finding at the data point that produced it is worse than no reading.
///
/// ⚠ The unknown arm is a THIRD sentence rather than a fall-through, this workspace's rule that an
/// unclassified case is stated and never glossed.
#[must_use]
fn ours_sentence(live: u64, opened: u64, ceiling: Option<u64>) -> String {
    let reading = match ceiling {
        Some(ceiling) if opened >= ceiling => format!(
            " — and {opened} is the whole of this host's {ceiling}-place namespace or more, so if \
             it does not give a closed pseudoterminal straight back THIS PROCESS ALONE was enough \
             and no other process need be involved"
        ),
        Some(ceiling) => format!(
            " — and {opened} is under the {ceiling} this host PUBLISHES, which settles nothing \
             either way: that number is a ceiling the kernel advertises rather than the limit this \
             refusal came from, and the count beside it is a running total of a process that has \
             not finished"
        ),
        None => " — and this host does not publish how many it allows, so whether that history \
                 could have used its namespace up cannot be said here"
            .to_owned(),
    };
    format!(
        "this process was holding {live} of them itself and had opened {opened} since it started, \
         which cannot rule exhaustion out (another process may hold the rest) but does say how \
         much of any of it was ours — and the two ask different things: the first is what we were \
         using when it refused, the second is what we could have used up if this host does not \
         give a closed pseudoterminal straight back{reading}"
    )
}

/// This process's live pseudoterminal count — see [`ours_sentence`].
static OPEN_HERE: OpenHere = OpenHere {
    live: std::sync::atomic::AtomicU64::new(0),
    opened: std::sync::atomic::AtomicU64::new(0),
};

/// The ledger behind [`OPEN_HERE`], kept as a type so the only way to add to it is to take a
/// [`Place`] that gives the count back when it drops.
///
/// ⚠⚠ **The count is `live()` and the guard is `Place`, deliberately not `held()`/`Held`** — the
/// first spelling of this collided with the vocabulary of a HOLD a person puts on a run, and
/// `the_only_plugin_that_can_be_held_is_the_one_that_reads_a_hold` went red on it. That gate was
/// right: *held* is that wire word's, and a second meaning for it in another crate is how one
/// sentence starts covering two facts.
/// ⚠⚠ TWO COUNTERS AND NOT ONE — register item 814. `live` goes back down when a pair is dropped;
/// `opened` never does. They answer the two worlds a refusal at 46-of-511 leaves open, and a reader
/// handed only the first cannot tell somebody else's process from this suite's own churn.
#[derive(Debug)]
struct OpenHere {
    /// How many pairs this process is holding right now.
    live: std::sync::atomic::AtomicU64,
    /// How many it has taken since it started. ⚠ MONOTONIC by construction — [`Place`] does not
    /// touch it, which is the whole difference between the two questions.
    opened: std::sync::atomic::AtomicU64,
}

impl OpenHere {
    /// How many are live right now.
    fn live(&'static self) -> u64 {
        self.live.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many have EVER been taken here — see the type's own note for why both are reported.
    fn opened(&'static self) -> u64 {
        self.opened.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Count one more, until the returned guard drops — and one more for good, which no guard
    /// gives back.
    fn take(&'static self) -> Place {
        self.live.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.opened
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Place(self)
    }
}

/// One pseudoterminal's place in [`OPEN_HERE`], returned on drop.
///
/// ⚠ A guard rather than a pair of bare `fetch_add`/`fetch_sub` calls, because the decrement has to
/// happen on EVERY path a `Pty` leaves by — including a panic between opening and spawning, which
/// is precisely when a leaked count would make the next refusal's sentence lie.
#[derive(Debug)]
struct Place(&'static OpenHere);

impl Drop for Place {
    /// ⚠ `live` ONLY. `opened` is what this process has ever taken, so giving it back here would
    /// collapse the two questions register item 814 exists to keep apart.
    fn drop(&mut self) {
        self.0
            .live
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A pseudoterminal pair: the master this process reads and writes, and the slave the child gets.
///
/// The slave is held only until a child is spawned onto it. Holding it any longer would keep the
/// device open after the child dies, and the master would never see EOF — the reader thread would
/// block forever on a pane whose program has already exited.
#[derive(Debug)]
pub struct Pty {
    /// The controlling side. Cloned for the reader and the writer; resized through.
    master: OwnedFd,
    /// The child's side, taken by [`AttachedPty::spawn`] and dropped there.
    slave: Option<OwnedFd>,
    /// This pair's place in this process's own ledger — see [`ours_sentence`]. Kept for exactly as
    /// long as the pair is, so the count a refusal reports is the count that was live.
    _place: Place,
}

impl Pty {
    /// Allocate a pseudoterminal sized `cols` x `rows`.
    ///
    /// # Errors
    ///
    /// Returns the OS error if no device could be allocated.
    pub fn open(cols: u16, rows: u16) -> io::Result<Self> {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // `mut`, and the last two arguments are `*mut`, because the two platforms declare this
        // differently: glibc takes `termp: *const termios, winp: *const winsize` and Apple's libc
        // takes both as `*mut`. A `*mut` coerces to a `*const` and not the other way round, so the
        // mutable spelling is the one that compiles on both — and it costs nothing, since `openpty`
        // does not write through either pointer on any platform.
        let mut size = winsize(cols, rows);
        // SAFETY: both descriptors are out-parameters the call fills in, `size` is a fully
        // initialised `winsize`, and the terminal-settings argument is deliberately null (the
        // child's shell sets its own).
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
            )
        };
        if opened != 0 {
            // ⛔⛔⛔⛔⛔ **AND HOW FULL THE POOL WAS WHEN IT REFUSED** — register item 776, arm (d).
            // The bare errno sent a reader guessing: `ENXIO` on one macOS CI job was filed as
            // *the runner's pty pool was exhausted*, which was an inference nothing in the message
            // supported. `pool_sentence` says what this host publishes and says so when it
            // publishes nothing, which is the difference between a reading and a guess.
            //
            // ⚠ The KIND is preserved and only the sentence grows: `PanePtyError` stores its source
            // as a `Display` string, so nothing downstream reads the errno off this — but a caller
            // that later wants to MATCH on it still can.
            let raw = io::Error::last_os_error();
            return Err(io::Error::new(
                raw.kind(),
                format!("{raw} — {}", pool_sentence(crate::procfs::pty_pool())),
            ));
        }
        // SAFETY: both are freshly opened descriptors this process now owns.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // BOTH sides close on exec, and the slave is not the obvious one.
        //
        // The master is easy to argue: a child holding the read end of its own terminal keeps it
        // open forever, so the reader here would never see EOF.
        //
        // The slave matters because this daemon spawns panes from MANY THREADS. Between this
        // `openpty` and the `spawn` below, another thread's fork inherits every descriptor that is
        // not close-on-exec — including this slave. That child then holds this pane's terminal open
        // for as long as it lives, and when THIS pane's program exits its reader blocks forever on
        // a device somebody else is still holding.
        //
        // Measured, not reasoned: with only the master marked, the crate's own suite passed 302/302
        // single-threaded and HUNG under the default parallel run. `Stdio::from` re-duplicates onto
        // the child's fds 0/1/2, which clears the flag where it must be clear, so marking it here
        // costs the child nothing.
        set_cloexec(&master)?;
        set_cloexec(&slave)?;
        Ok(Self {
            master,
            slave: Some(slave),
            // ⚠ TAKEN LAST, after every fallible step, so a pair that failed to be set up is not
            // counted as one this process is holding.
            _place: OPEN_HERE.take(),
        })
    }

    /// The device name of the child's side (`/dev/pts/N`) — a pane's terminal, as a person's `tty`
    /// would report it.
    ///
    /// Resolved from the master rather than from the child's fd 0, which the child is free to
    /// redirect.
    ///
    /// # Why the call underneath is per-platform
    ///
    /// There is no portable thread-safe spelling of this question, and thread-safety is not a
    /// nicety here: this daemon opens and names panes from many threads at once. POSIX `ptsname`
    /// returns a pointer into ONE static buffer, so two panes opening together can each be handed
    /// the other's device name. glibc's answer is `ptsname_r`, which writes into the caller's
    /// buffer; Apple has never shipped `ptsname_r` and its answer is the `TIOCPTYGNAME` ioctl,
    /// which does the same thing by another name. Both are asked for through
    /// `ptsname_into` (crate-private), so this function has one body and the difference is
    /// stated once.
    #[must_use]
    pub fn tty_name(&self) -> Option<PathBuf> {
        // `c_char` and not `i8`: the two are the same type on x86-64 Linux and on Apple, and
        // DIFFERENT on aarch64 Linux, where `c_char` is unsigned. Spelling the element type as the
        // C one is what keeps this compiling on a target neither CI job builds today.
        let mut buf = [0 as libc::c_char; TTY_NAME_MAX];
        if !ptsname_into(self.master.as_raw_fd(), &mut buf) {
            return None;
        }
        // SAFETY: on success the call above wrote a NUL-terminated string into `buf`.
        let name = unsafe { CStr::from_ptr(buf.as_ptr()) };
        Some(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
    }

    /// Tell the device its new size, so the child's `SIGWINCH` and `TIOCGWINSZ` agree with what the
    /// user sees.
    ///
    /// The PIXEL metrics travel too, and they are not decoration: a child drawing Sixel or Kitty
    /// graphics sizes its image from `ws_xpixel`/`ws_ypixel`, so dropping them would silently give
    /// every inline image the wrong scale.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the device rejected the size.
    pub fn resize(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> io::Result<()> {
        let mut size = winsize(cols, rows);
        size.ws_xpixel = pixel_width;
        size.ws_ypixel = pixel_height;
        // SAFETY: the master is open and `size` is a fully initialised `winsize`.
        let sized = unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as _,
                &raw const size,
            )
        };
        if sized != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// A handle for reading the child's output.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the descriptor could not be duplicated.
    pub fn reader(&self) -> io::Result<File> {
        Ok(File::from(self.master.try_clone()?))
    }

    /// A handle for writing to the child's input.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the descriptor could not be duplicated.
    pub fn writer(&self) -> io::Result<File> {
        Ok(File::from(self.master.try_clone()?))
    }

    /// A handle that only ASKS this terminal questions — see [`TerminalQuery`].
    ///
    /// A third duplicate of the controlling side, taken because the other two are spoken for: the
    /// reader thread is blocked in `read` on one and the resize coalescer owns the device itself.
    /// Duplicating the master keeps neither of them waiting and cannot affect when the reader sees
    /// EOF, which depends on the SLAVE side being let go.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the descriptor could not be duplicated.
    pub fn query(&self) -> io::Result<TerminalQuery> {
        Ok(TerminalQuery(self.master.try_clone()?))
    }

    /// Put `body` on its own thread reading this terminal, and answer only once it IS reading it.
    ///
    /// This is the only way to reach [`AttachedPty::spawn`], and that is the point: **a child must
    /// never be able to write to a terminal nobody is draining.**
    ///
    /// # The defect this shape exists to make unspellable
    ///
    /// A pane used to be built the other way round — spawn the child, then create the reader
    /// thread — which leaves a window where the child is running and nothing is reading. What is in
    /// that window is not the same on every kernel:
    ///
    /// * Linux keeps the bytes. Measured: a child that writes 25 bytes and is fully reaped, read
    ///   100 ms later, still hands over all 25.
    /// * macOS does not. The same shape — a restored pane re-running `echo`, which writes once and
    ///   exits — came back with an EMPTY screen on the macOS runner while passing 30 of 30 times on
    ///   Linux, and that is the difference this call removes.
    ///
    /// So the old ordering was not slow, it was *right only on one of the two platforms this
    /// product builds for*, and it read as correct on the one its author ran.
    ///
    /// # Why an answer, and not just a thread
    ///
    /// Creating a thread proves nothing: it may not be scheduled for milliseconds, and under load
    /// that is exactly when it will not be. So a single byte is written into the device and this
    /// call blocks until the reader has READ it — after which the reader is running, hot, and one
    /// loop back-edge away from its next `read`. The byte is consumed here and never reaches
    /// `body`, so nothing downstream can see it.
    ///
    /// It is honest about what remains: a reader that has read is not provably a reader that is
    /// blocked in `read`. What is gone is the part that was unbounded — thread creation and first
    /// scheduling, both of which now happen before the child exists rather than in a race with it.
    ///
    /// # The order, as something the compiler holds
    ///
    /// ```
    /// use sprag_terminal::CommandBuilder;
    /// use sprag_terminal::pty::Pty;
    ///
    /// let mut pane = Pty::open(80, 24)
    ///     .expect("open a pty")
    ///     .attach_reader("doc", |mut terminal| {
    ///         let _ = std::io::copy(&mut terminal, &mut std::io::sink());
    ///     })
    ///     .expect("a fresh pty takes a reader");
    /// // ⚠ `/bin/sh`, not `/bin/true`: macOS keeps `true` at `/usr/bin/true` and has no
    /// // `/bin/true` at all, so the first version of this example failed the macOS job with
    /// // `NotFound` while every Linux run passed. A doctest is a test, and it runs where the
    /// // suite runs. `/bin/sh` is the portable child this workspace's pty fixtures already use.
    /// let mut trivial = CommandBuilder::new("/bin/sh");
    /// trivial.arg("-c");
    /// trivial.arg("exit 0");
    /// let (mut child, _joined) = pane
    ///     .spawn(&trivial, None)
    ///     .expect("spawn onto the pty");
    /// child.wait().expect("the child is reapable");
    /// ```
    ///
    /// The ordering this replaced is not a bug that can be reintroduced by editing a line: there is
    /// no `spawn` to call before attaching, so it does not build.
    ///
    /// ```compile_fail
    /// use sprag_terminal::CommandBuilder;
    /// use sprag_terminal::pty::Pty;
    ///
    /// let mut pty = Pty::open(80, 24).expect("open a pty");
    /// // no method named `spawn` found for struct `Pty` — a child is born onto an `AttachedPty`.
    /// let _ = pty.spawn(&CommandBuilder::new("/bin/sh"), None);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the OS error if the reader could not be cloned, the thread could not be created, or
    /// the probe could not be written — and [`io::ErrorKind::BrokenPipe`] if the reader reached the
    /// end of the terminal instead of the probe, which means it is not there to read anything.
    pub fn attach_reader<F>(self, name: &str, body: F) -> io::Result<AttachedPty>
    where
        F: FnOnce(File) + Send + 'static,
    {
        let mut reader = self.reader()?;
        let (ready, reading) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                // `read_exact` of exactly the probe, and it cannot read anything else: no child
                // exists yet, and this is the last instant that is true.
                let mut probe = [0u8; 1];
                if reader.read_exact(&mut probe).is_err() {
                    return;
                }
                if ready.send(()).is_err() {
                    return;
                }
                body(reader);
            })?;
        // Written after the thread exists so that, in the ordinary case, it is already blocked in
        // `read` and the byte wakes it rather than sitting in a queue.
        self.write_probe()?;
        reading.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the reader reached the end of the terminal before it could read from it",
            )
        })?;
        Ok(AttachedPty {
            pty: self,
            reader: thread,
        })
    }

    /// Put [`ATTACH_PROBE`] into the device, from the child's side, so a reader on the master has
    /// something to read before any child can write.
    ///
    /// ⚠ THE MISSING-DEVICE ARM IS UNREACHABLE FROM THE ONE CALLER, and is here rather than as an
    /// `expect` so that it stays unreachable. Only [`AttachedPty::spawn`] takes the device and it
    /// cannot run first — reaching it means going through [`Pty::attach_reader`], which consumes the
    /// `Pty`. A second caller added later would not have that guarantee, and a library that panicked
    /// on it would answer a programming mistake with a dead daemon.
    fn write_probe(&self) -> io::Result<()> {
        let slave = self.slave.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "this pty has already given its device away",
            )
        })?;
        // A CLONE, so the write closing its handle leaves the device itself open for the child.
        File::from(slave.try_clone()?).write_all(&[ATTACH_PROBE])
    }
}

/// One byte, written into a fresh device by [`Pty::attach_reader`] and consumed by the reader it
/// attaches — the difference between *a thread was created* and *a thread is reading*.
///
/// NUL because it is the one byte that survives the line discipline unaltered and means nothing to
/// the emulator downstream if it ever did arrive.
const ATTACH_PROBE: u8 = 0;

/// A pseudoterminal that is already being read — the only thing a child can be born onto.
///
/// Reachable only through [`Pty::attach_reader`], which is what makes *spawn a child, then start
/// reading* a program that does not compile rather than a bug that reproduces on one platform.
#[derive(Debug)]
pub struct AttachedPty {
    pty: Pty,
    reader: std::thread::JoinHandle<()>,
}

impl AttachedPty {
    /// The device and the reader's handle, for a caller that owns them separately from here on —
    /// the pane hands the device to its resize thread and joins the reader on drop.
    #[must_use]
    pub fn into_parts(self) -> (Pty, std::thread::JoinHandle<()>) {
        (self.pty, self.reader)
    }

    /// Start `command` on this pseudoterminal, in `cgroup` if one is given.
    ///
    /// `cgroup` is an open `cgroup.procs` file. The child writes `"0"` to it — which the kernel
    /// reads as *the calling process* — **after** `fork` and **before** `exec`, so it is inside its
    /// cgroup from its first instruction. Nothing it forks afterwards can be born outside. Opening
    /// the file here rather than there is what makes the child's half allocation-free and
    /// async-signal-safe: it writes one byte to a descriptor it already has.
    ///
    /// Consumes the slave: after this the child owns the device, and this process must not.
    ///
    /// Answers with the child AND with whether it reached that cgroup — see [`Joined`] for why a
    /// refused join is an answer here rather than a failed birth.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the pty was already spawned onto, or if the child could not start.
    /// A cgroup the kernel would not admit the child to is NOT among them.
    pub fn spawn(
        &mut self,
        command: &CommandBuilder,
        cgroup: Option<BorrowedFd<'_>>,
    ) -> io::Result<(Child, Joined)> {
        let slave = self.pty.slave.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::AlreadyExists, "this pty already has a child")
        })?;
        let (program, args) = command.parts().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "a command with no program")
        })?;

        let mut spawn = Command::new(program);
        spawn.args(args);
        for (key, value) in command.env_pairs() {
            spawn.env(key, value);
        }
        // Always set, never left to inheritance: see `CommandBuilder::start_dir` for why a pane
        // with no directory of its own opens in HOME rather than wherever the daemon was started.
        spawn.current_dir(command.start_dir());
        // Three separate descriptors, because `std` closes each after `dup2` and one shared handle
        // would be closed three times.
        spawn
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));

        // Duplicated OUT of the borrow so the closure owns a plain descriptor: a `pre_exec` closure
        // must be `'static`, and it must not touch anything that could allocate or lock.
        let cgroup = cgroup.map(|fd| fd.try_clone_to_owned()).transpose()?;
        // The child's channel for saying it did NOT get in. Only when a cgroup was offered: with
        // nothing to join there is nothing to report, and a pipe per pane birth on a host that
        // enforces nothing would be two descriptors bought for no answer.
        //
        // `std::io::pipe` rather than `libc::pipe2`, because the write end MUST be close-on-exec —
        // it is how the parent learns the child got as far as `exec` — and `pipe2(O_CLOEXEC)` does
        // not exist on Apple, where the two-call spelling would leak the descriptor into any child
        // another thread spawned in between. Both ends are `CLOEXEC` here on both platforms, which
        // is R340's rule applied before the platform can bite: put the gate on the syscall by
        // picking one that has no divergence.
        let (hear, tell) = match cgroup {
            Some(_) => {
                let (hear, tell) = io::pipe()?;
                (Some(hear), Some(tell))
            }
            None => (None, None),
        };
        // SAFETY: the closure runs in the child between `fork` and `exec`. Every call in it is
        // async-signal-safe and none of them allocates: two `ioctl`-class syscalls and at most two
        // `write`s of fixed-size buffers to descriptors opened before the fork.
        unsafe {
            spawn.pre_exec(move || {
                // A session of its own, so this pty can become a controlling terminal at all.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                // `std` has already put the slave on fd 0, so THIS is the device to claim. Claiming
                // it is what makes Ctrl-C reach the child's foreground job instead of the daemon.
                //
                // `as _` because an ioctl request constant does NOT have one type across the targets
                // this builds for, and neither does the parameter it goes into: glibc declares the
                // constants as `Ioctl` (`c_ulong` on gnu, `c_int` on musl), and Apple's libc
                // declares `TIOCSCTTY` as `c_uint` while declaring `TIOCSWINSZ` beside it as
                // `c_ulong`. Inferring the target type from the signature is the only spelling that
                // is right on all of them; naming any one of them would be right on that one.
                if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if let Some(cgroup) = &cgroup {
                    // "0" means the calling process. The whole point of R336: the child is in its
                    // pane's cgroup before it becomes the pane's program.
                    let wrote = libc::write(cgroup.as_raw_fd(), c"0".as_ptr().cast(), 1);
                    // REPORTED, NEVER RETURNED. Returning here is what made a host that refuses the
                    // migration a host with no panes at all; see `Joined`. The errno is captured
                    // before anything else can overwrite it, and the report's own failure is
                    // ignored on purpose — there is no one left to tell, and the pane is still the
                    // thing worth having.
                    if wrote < 0
                        && let (Some(tell), Some(code)) =
                            (&tell, io::Error::last_os_error().raw_os_error())
                    {
                        let code = code.to_ne_bytes();
                        libc::write(tell.as_raw_fd(), code.as_ptr().cast(), code.len());
                    }
                }
                Ok(())
            });
        }

        let child = spawn.spawn()?;
        // The child holds the device now. Dropping ours is what lets the master read EOF when it
        // exits, and it must happen whether or not anything else here succeeds.
        drop(slave);
        // And dropping the COMMAND is what closes this process's only copy of the report pipe's
        // write end — the closure above owns it, and the closure lives in here. Without this the
        // read below would wait for a writer that is this very thread. It is safe now and not
        // before: `spawn` has forked, so the closure has already been handed to the child.
        drop(spawn);
        Ok((child, Self::heard(hear)))
    }

    /// Read the child's verdict on its own cgroup join.
    ///
    /// Called only after `spawn` has returned, which is what makes this a read and not a wait:
    /// `Command::spawn` does not answer until the child has reached `exec`, and `exec` closes the
    /// child's `CLOEXEC` copy of the write end. So the pipe is already at end-of-file when this
    /// looks, holding four bytes if the join was refused and nothing at all if it was not.
    fn heard(hear: Option<io::PipeReader>) -> Joined {
        let Some(mut hear) = hear else {
            return Joined::NotAsked;
        };
        let mut code = [0u8; size_of::<i32>()];
        match hear.read(&mut code) {
            // The child wrote its errno and then execed anyway: it is running, outside its cgroup.
            Ok(n) if n == code.len() => {
                Joined::Refused(io::Error::from_raw_os_error(i32::from_ne_bytes(code)))
            }
            // End of file with nothing in it — the write landed and the child said nothing. A short
            // read cannot happen for four bytes through a pipe, and reading the parent's own end
            // cannot fail once the writer is closed; both fall here because the pane IS in its
            // cgroup in the only ordering that produces them.
            _ => Joined::Joined,
        }
    }
}

/// How long a pty device name this crate is willing to read.
///
/// 128 because that is the size `TIOCPTYGNAME` is DECLARED with — Apple's request code
/// (`0x40807453`) encodes its argument length in the middle two bytes, `0x080` — so a shorter buffer
/// would be a buffer the kernel writes past. `/dev/pts/N` and `/dev/ttysNNN` are an order of
/// magnitude shorter than that on both platforms.
const TTY_NAME_MAX: usize = 128;

/// Write the device name of `master`'s slave side into `buf`, thread-safely. `false` if the
/// platform's call refused.
///
/// See [`Pty::tty_name`] for why this is per-platform at all. Both arms write a NUL-terminated
/// string into the caller's buffer and neither touches a static one.
#[cfg(not(target_vendor = "apple"))]
fn ptsname_into(master: RawFd, buf: &mut [libc::c_char; TTY_NAME_MAX]) -> bool {
    // SAFETY: `buf` is a valid writable buffer of the length passed, and `master` is open.
    unsafe { libc::ptsname_r(master, buf.as_mut_ptr(), buf.len()) == 0 }
}

/// Apple's spelling: an ioctl that fills the caller's buffer, because `ptsname_r` does not exist
/// here — verified against `libc`'s own `unix/bsd/apple/mod.rs`, which declares `TIOCPTYGNAME` and
/// no `ptsname_r` at all.
#[cfg(target_vendor = "apple")]
fn ptsname_into(master: RawFd, buf: &mut [libc::c_char; TTY_NAME_MAX]) -> bool {
    // SAFETY: `buf` is exactly the 128 bytes `TIOCPTYGNAME`'s request code declares, and `master`
    // is open.
    unsafe { libc::ioctl(master, libc::TIOCPTYGNAME as _, buf.as_mut_ptr()) == 0 }
}

/// The kernel's window size, with the pixel metrics left at zero — sprag reports cells, and a
/// child that wants pixels asks the emulator, not the device.
fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Mark `fd` close-on-exec.
fn set_cloexec(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: `fd` is open and owned; both calls are the documented `F_GETFD`/`F_SETFD` pair.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The two facts a reaped child leaves: its code, and the signal that took it if one did.
///
/// Spelled exactly as the previous backend spelled them, because they reach the wire — `strsignal`
/// for the name ("Hangup", "Killed"), and a signalled child reported as code 1, which is what
/// `std` leaves as `None`.
#[must_use]
pub fn exit_facts(status: std::process::ExitStatus) -> (u32, Option<String>) {
    use std::os::unix::process::ExitStatusExt as _;

    let code = status.code().map_or(1, |code| code as u32);
    let signal = status.signal().map(|signal| {
        // SAFETY: `strsignal` takes a signal number and returns a static NUL-terminated string or
        // null; both are handled.
        let named = unsafe { libc::strsignal(signal) };
        if named.is_null() {
            format!("Signal {signal}")
        } else {
            // SAFETY: non-null means `strsignal` returned a NUL-terminated string.
            unsafe { CStr::from_ptr(named) }
                .to_string_lossy()
                .into_owned()
        }
    });
    (code, signal)
}

/// Signal a pane's child by pid.
///
/// By pid rather than through the [`Child`] handle because the reader thread owns that handle and
/// is blocked in `wait` on it; `std`'s `kill` needs `&mut`. The window this opens — a pid reused
/// after the child is reaped — is the same one the previous backend's cloned killer had, and it is
/// closed the same way: the caller stops signalling once it has seen the child exit.
pub fn signal_child(pid: u32, signal: libc::c_int) {
    if pid == 0 {
        return;
    }
    // SAFETY: `kill` on a pid that may be gone is defined; it answers `ESRCH`, which is ignored
    // here because a child that has already exited needs no signal.
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// ⛔⛔⛔⛔⛔ **A REFUSAL SAYS WHAT THE HOST PUBLISHES, AND SAYS WHEN IT PUBLISHES NOTHING** —
    /// register item 776, arm (d), at [`pool_sentence`].
    ///
    /// # ⚠⚠⚠⚠⚠ Why all four combinations have a reader here and not just the one this host makes
    ///
    /// The failure this exists for happened on macOS, where `in_use` is `None` — the arm a Linux
    /// suite can never reach through [`Pty::open`]. An assertion only over the pair this kernel
    /// produces would leave the arm that matters unread, which is this workspace's rule that an
    /// unclassified path is RED rather than a pass.
    ///
    /// ⚠⚠ AND THE *unknown* ARMS ARE ASSERTED TO SAY SO IN WORDS. The whole finding is that a bare
    /// `Device not configured` was completed from memory as *the pool was exhausted*; a sentence
    /// that merely omitted the missing half would be completed the same way.
    #[test]
    fn a_pty_refusal_says_how_full_the_pool_was_or_that_it_cannot() {
        use crate::procfs::PtyPool;

        let both = pool_sentence(PtyPool {
            in_use: Some(62),
            max: Some(4096),
        });
        assert!(
            both.contains("62") && both.contains("4096"),
            "⛔⛔⛔ REGISTER ITEM 776: a host that publishes both halves is not quoting them, so a \
             reader still cannot tell exhaustion from anything else. Got: {both:?}",
        );

        // ── THE ARM THE macOS FAILURE LANDS ON, and the one no Linux run can produce ──────────
        let ceiling_only = pool_sentence(PtyPool {
            in_use: None,
            max: Some(127),
        });
        assert!(
            ceiling_only.contains("127") && ceiling_only.contains("cannot be read here"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 776: this host publishes the ceiling and NOT the count, and the \
             sentence does not say the second half is missing — so a reader completes it from \
             memory, which is how *the runner's pty pool was exhausted* got written down as a \
             cause with nothing behind it. Got: {ceiling_only:?}",
        );

        let count_only = pool_sentence(PtyPool {
            in_use: Some(9),
            max: None,
        });
        assert!(
            count_only.contains('9') && count_only.contains("does not publish how many it allows"),
            "⛔⛔⛔ REGISTER ITEM 776: a host that knows the count and not the ceiling must say \
             which half is missing, or `9` reads as `9 of 9`. Got: {count_only:?}",
        );

        let neither = pool_sentence(PtyPool {
            in_use: None,
            max: None,
        });
        assert!(
            neither.contains("neither") && neither.contains("cannot be read here"),
            "⛔⛔⛔ REGISTER ITEM 776: a host that publishes nothing must say so. Silence here is \
             what the bare errno already was. Got: {neither:?}",
        );

        // ── ⛔⛔⛔⛔⛔ AND THIS PROCESS'S OWN SHARE, ON EVERY ARM ────────────────────────────
        //
        // ⚠⚠⚠⚠⚠ The round that built the four arms above wrote *"the next macOS failure brings
        // the answer"*. **On macOS it does not**: the arm that lands there is `ceiling_only`, which
        // is a ceiling and an apology — a reader still cannot tell exhaustion from anything else,
        // which was the debt. The half Darwin will not publish is added here from the one ledger
        // that is checkable: this process's own.
        //
        // ⚠ It cuts ONE WAY and the sentence has to say so, for the reason the arms above exist:
        // a small number does not RULE OUT exhaustion, because another process may hold the rest.
        // Left unqualified it would be completed from memory as *the pool was not full*, which is
        // the same defect pointed the other way.
        for (label, sentence) in [
            ("both", &both),
            ("ceiling only", &ceiling_only),
            ("count only", &count_only),
            ("neither", &neither),
        ] {
            assert!(
                sentence.contains("this process was holding"),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 776 arm (d): the {label} arm drops this process's own \
                 share, and on the arm macOS lands on that share is the ONLY half a reader gets. \
                 Got: {sentence:?}",
            );
        }
        let ours = ours_sentence(3, 3, Some(100));
        assert!(
            ours.contains('3') && ours.contains("cannot rule exhaustion out"),
            "⛔⛔⛔⛔ a count without its limit is worse than none: three of a hundred reads as \
             *the pool was not full* unless the sentence says another process may hold the rest. \
             Got: {ours:?}",
        );
        assert_ne!(
            ours_sentence(0, 0, Some(100)),
            ours_sentence(120, 120, Some(100)),
            "⚠ and the count must actually be IN the sentence, not a constant beside it",
        );

        // ── ⛔⛔⛔⛔⛔ AND THE TWO NUMBERS MUST BE TOLD APART — register item 814 ─────────────
        //
        // `30cb15d`'s macOS job refused at **46 of at most 511**, which is far too few to have
        // used the pool up. Two worlds fit that: somebody else's process held the rest, or this
        // host does not give a CLOSED pseudoterminal straight back and what mattered was the
        // hundreds this suite had opened and dropped. They want opposite repairs, and the live
        // count alone reads the same in both.
        let churned = ours_sentence(46, 800, Some(511));
        assert!(
            churned.contains("46") && churned.contains("800"),
            "⛔⛔⛔⛔⛔ ITEM 814: a process holding 46 and having opened 800 is the whole finding, \
             and a sentence that quotes only one of them sends the next reader down one of two \
             roads at random. Got: {churned:?}",
        );
        assert_ne!(
            ours_sentence(46, 46, Some(511)),
            ours_sentence(46, 800, Some(511)),
            "⛔⛔⛔ ITEM 814: the same live count with a different history must not read the same. \
             *46 and only ever 46* says the pool was somebody else's; *46 of 800 taken* says this \
             suite's own churn could have used a 511-name space up on its own.",
        );

        // ── ⛔⛔⛔⛔⛔ AND THE SENTENCE DOES THE SUBTRACTION — register item 814's second reading ──
        //
        // ⚠⚠⚠⚠⚠ MEASURED, AND THAT MEASUREMENT IS WHY THIS ARM EXISTS. `6633099`'s macOS job
        // printed the balanced sentence at **holding 30 / opened 422 / at most 511** and settling
        // it took an `strace` of the whole suite on another machine: **847 opens in one process**,
        // 1.66× that namespace. Every number was already in the sentence and the reader still had
        // to compare two of them by hand.
        //
        // ⚠ THREE READINGS DRIVEN AS VALUES (register item 803): over the ceiling, under it, and
        // no ceiling published — which is the arm macOS's own answer lands on for the IN-USE half
        // and must not be folded into either of the other two.
        let over = ours_sentence(30, 847, Some(511));
        assert!(
            over.contains("THIS PROCESS ALONE was enough"),
            "⛔⛔⛔⛔⛔ ITEM 814: this process had taken 847 of a 511-place namespace and the \
             sentence still reads as though the two worlds were open. They are not: on a host that \
             does not recycle, our own history is sufficient and no other process need be \
             involved. Got: {over:?}",
        );
        // ⛔⛔⛔⛔⛔ **AND THE UNDER-THE-CEILING ARM MAY NOT CONCLUDE** — this is the exact triple
        // `6633099`'s macOS job refused at, and the first version of this arm printed *something
        // else was holding it* over it: the opposite of what the same round measured (847 opens in
        // one process, of which this 422 is barely half). `procfs::PtyPool`'s own doc records that
        // family of defect — an `ENXIO` filed as *the pool was exhausted*, an inference and not a
        // reading — and inferring the other way from a PUBLISHED ceiling is the same move.
        let under = ours_sentence(30, 422, Some(511));
        assert!(
            under.contains("settles nothing"),
            "⛔⛔⛔⛔⛔ ITEM 814: this is the refusal this item was filed over, and the sentence \
             draws a conclusion from `422 < 511`. It may not: that ceiling is what the kernel \
             ADVERTISES rather than the limit that refused, and 422 is a running total of a \
             process that went on to open 847. Got: {under:?}",
        );
        assert!(
            !under.contains("THIS PROCESS ALONE") && !under.contains("something else was holding"),
            "⚠⚠⚠ AND IT MAY NOT CONCLUDE IN EITHER DIRECTION. A reader handed a verdict here goes \
             looking for another process, or stops looking at ours, on arithmetic over a number \
             nobody has shown to be the binding one. Got: {under:?}",
        );
        let unknown = ours_sentence(30, 847, None);
        assert!(
            unknown.contains("does not publish how many it allows")
                && !unknown.contains("THIS PROCESS ALONE")
                && !unknown.contains("settles nothing"),
            "⛔⛔ ITEM 814 / rule 6: a host that publishes no ceiling is a THIRD state, and folding \
             it into either answer tells a reader something nobody knows. Got: {unknown:?}",
        );
        // ⚠⚠⚠⚠⚠ AND THE BOUNDARY IS THE CEILING ITSELF — asserted by which ARM each side takes,
        // never by the two sentences DIFFERING. Measured in this round: an `assert_ne!` here was
        // satisfied by a mutation that moved the boundary, because the two sentences quote
        // different numbers and so differ whichever arm they came from. That is register item 775's
        // dead line, and it went green on the mutation it was written to catch.
        let exactly = ours_sentence(30, 511, Some(511));
        let one_short = ours_sentence(30, 510, Some(511));
        assert!(
            exactly.contains("THIS PROCESS ALONE was enough")
                && one_short.contains("settles nothing"),
            "⚠⚠ TAKING EXACTLY THE WHOLE NAMESPACE is the first count that is enough on its own — \
             a boundary one place out reports the run that used every name as though the question \
             were still open. exactly={exactly:?} one_short={one_short:?}",
        );

        // ── ⛔⛔⛔ AND THE LEDGER ITSELF, which the sentence is only worth what it is ──────────
        //
        // ⚠⚠ Driven on a ledger of its OWN rather than the process-wide one, because this crate's
        // suite opens pseudoterminals on many threads at once: a reading of the shared counter is
        // not a fact about this test. Leaked so it is `'static`, which costs one `u64` for the
        // life of a test binary.
        let ledger: &'static OpenHere = Box::leak(Box::new(OpenHere {
            live: std::sync::atomic::AtomicU64::new(0),
            opened: std::sync::atomic::AtomicU64::new(0),
        }));
        assert_eq!(
            (ledger.live(), ledger.opened()),
            (0, 0),
            "a fresh ledger counts nothing, on either question",
        );
        let first = ledger.take();
        let second = ledger.take();
        assert_eq!(
            (ledger.live(), ledger.opened()),
            (2, 2),
            "two places taken must read as two, and as two ever taken",
        );
        drop(first);
        assert_eq!(
            ledger.live(),
            1,
            "⛔⛔⛔⛔⛔ A PLACE THAT IS NOT GIVEN BACK MAKES EVERY LATER SENTENCE LIE, and it lies \
             in the direction that invents exhaustion — the very cause this arm was filed for \
             having no evidence behind it",
        );
        drop(second);
        assert_eq!(ledger.live(), 0, "and the last one too");
        // ⛔⛔⛔⛔⛔ AND THE HISTORY DOES NOT COME BACK WITH THEM — register item 814. This is the
        // whole of what the second counter is: a `Drop` that touched it would make *held 46* and
        // *took 800* the same number again, and the two worlds that refusal leaves open would
        // collapse back into one.
        assert_eq!(
            ledger.opened(),
            2,
            "⛔⛔⛔⛔⛔ ITEM 814: the ledger gave back a pseudoterminal this process HAD opened, so \
             the churn half of every later refusal is understated — in the direction that says \
             *this suite cannot have used the pool up*, which is the reassuring one",
        );

        // ── ⚠ AND THE DOOR IS WIRED TO IT: a live pseudoterminal cannot read as none of ours ──
        let open_now = Pty::open(20, 5).expect("open a pseudoterminal");
        let while_open = pool_sentence(crate::procfs::pty_pool());
        assert!(
            !while_open.contains("holding 0 of them"),
            "⛔⛔⛔ `Pty::open` is not taking a place in the ledger, so the half added for macOS \
             would report zero however many this process held. Got: {while_open:?}",
        );
        drop(open_now);

        // ── AND THE FOUR ARE FOUR SENTENCES, not one wearing different numbers ────────────────
        let said = [&both, &ceiling_only, &count_only, &neither];
        for (i, one) in said.iter().enumerate() {
            for other in said.iter().skip(i + 1) {
                assert_ne!(
                    one, other,
                    "⛔⛔⛔ REGISTER ITEM 776: two of the four pool states compose the SAME \
                     sentence, so the distinction this type carries reaches no reader — which is \
                     the same thing as not carrying it.",
                );
            }
        }
    }

    /// Throw away everything the device is holding that nobody has read — what a kernel discarding
    /// a dead child's output does on its own, done deliberately so it can be OBSERVED on a kernel
    /// that does not.
    fn discard_unread(pty: &Pty) {
        // SAFETY: the master is open; `TCIFLUSH` names the queue holding what the child's side
        // wrote and this side has not read.
        let flushed = unsafe { libc::tcflush(pty.master.as_raw_fd(), libc::TCIFLUSH) };
        assert_eq!(flushed, 0, "tcflush: {}", io::Error::last_os_error());
    }

    /// Put `bytes` into the device from the child's side, without a child.
    fn write_from_the_childs_side(pty: &Pty, bytes: &[u8]) {
        let slave = pty.slave.as_ref().expect("a fresh pty holds its device");
        File::from(slave.try_clone().expect("clone the device"))
            .write_all(bytes)
            .expect("write into the device");
    }

    /// What the device is holding RIGHT NOW, read without waiting for the child's side to go.
    ///
    /// ⚠⚠ **THE CONTROL USED TO OBSERVE BY DROPPING THE SLAVE, AND THAT IS A PLATFORM CLAIM.** It
    /// asserted that a device nobody flushed *still holds* what was written to it — true on Linux,
    /// and false on macOS for exactly the mechanism this test exists to document, so the macOS job
    /// failed on the control rather than on the subject (`left: []`, `right: b"recorded"`). A
    /// control that encodes one kernel's behaviour cannot vouch for a test about the difference
    /// between two.
    ///
    /// So this reads NON-BLOCKING with the slave still open: bytes mean the device accepted them,
    /// and nothing means it has not yet — which is a state worth waiting out rather than concluding
    /// from, because a write from the child's side reaches the line discipline through a work queue
    /// (R351 measured `FIONREAD` answering 0 immediately after an 8-byte write and 8 a millisecond
    /// later). Hence the poll: it is an OBSERVATION with a deadline, not a sleep sized to a margin.
    ///
    /// The flag is set on a clone, which `dup` makes share the file description — so the master is
    /// non-blocking afterwards too. That is fine and deliberate: this pty belongs to the test.
    fn holding_now(pty: &Pty) -> Vec<u8> {
        let mut reader = pty.reader().expect("clone the reader");
        // SAFETY: the cloned descriptor is open; both calls only read and set its status flags.
        unsafe {
            let fd = reader.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            assert_ne!(flags, -1, "F_GETFL: {}", io::Error::last_os_error());
            assert_ne!(
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK),
                -1,
                "F_SETFL: {}",
                io::Error::last_os_error(),
            );
        }
        until(
            || {
                let mut buf = [0u8; 256];
                match reader.read(&mut buf) {
                    Ok(n) => buf[..n].to_vec(),
                    // Nothing in the queue yet — the only other answer a non-blocking read gives
                    // here, and the one the poll exists for.
                    Err(_) => Vec::new(),
                }
            },
            |seen| !seen.is_empty(),
        )
    }

    /// Everything left in the device, read to the end.
    fn drain_to_end(pty: &Pty) -> Vec<u8> {
        let mut reader = pty.reader().expect("clone the reader");
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        got
    }

    /// Wait for `settled` to hold, then answer what it saw — never a bare sleep, and the value
    /// comes back so the assertion is made on what the deadline actually found.
    fn until<T>(mut look: impl FnMut() -> T, settled: impl Fn(&T) -> bool) -> T {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = look();
            if settled(&seen) || std::time::Instant::now() >= deadline {
                return seen;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// **THE MECHANISM, FORCED.** Output nobody has collected is gone the moment the device drops
    /// it — so *who is reading, and since when* is not a performance question about this module.
    ///
    /// This is R351's macOS half made reproducible here. On that platform a pane's child that wrote
    /// once and exited lost what it wrote, because nothing had read it yet; on Linux the same bytes
    /// survive a fully reaped child and a 100 ms wait, so Linux can only be made to say this by
    /// emptying the queue deliberately.
    ///
    /// The CONTROL is the same write read back with no flush, in the same test: without it this
    /// would pass against a device that had never accepted the bytes at all.
    ///
    /// ⚠⚠ **WHICH PLATFORM JUDGES WHICH HALF, because the CI red was the control and not the
    /// subject.** The CONTROL is now kernel-independent — it never drops the child's side, so it
    /// asks only *did the device take the bytes*, which both answer the same way. The SUBJECT is
    /// judged on **Linux**: there the flush is the only thing that can empty the queue, so an
    /// assertion of emptiness is an assertion about `tcflush`. On macOS the drop that follows would
    /// have emptied it anyway, so the subject passes there for a second reason and proves less —
    /// said here rather than left for a later round to read this as covering both.
    #[test]
    fn output_no_one_has_collected_is_lost_when_the_device_drops_it() {
        let kept = {
            let pty = Pty::open(80, 24).expect("open a pty");
            write_from_the_childs_side(&pty, b"recorded");
            // ⚠ THE SLAVE STAYS OPEN. Dropping it is what the SUBJECT does, and on macOS dropping
            // it discards the queue — so a control that dropped it was asserting Linux's behaviour
            // in a test about the difference between two kernels. See `holding_now`.
            holding_now(&pty)
        };
        assert_eq!(
            kept, b"recorded",
            "the control: a device nobody flushed hands over what was written to it",
        );

        let mut pty = Pty::open(80, 24).expect("open a pty");
        write_from_the_childs_side(&pty, b"recorded");
        discard_unread(&pty);
        drop(pty.slave.take());
        assert!(
            drain_to_end(&pty).is_empty(),
            "what nobody had read is not there to be read afterwards",
        );
    }

    /// **What the reader is handed starts at the pane's first byte.** The attach probe is the
    /// ATTACH's business and reaches nothing downstream — a leaked one would land in a real pane's
    /// emulator, at the top of the screen, on every pane this daemon opens.
    ///
    /// REVERT-PROOF: stop consuming the probe in `attach_reader` and the body's first byte is a NUL.
    ///
    /// ⚠ THE OTHER HALF OF THE CONTRACT — *the reader has read before this call answers* — HAS NO
    /// INSTRUMENT HERE, and it is worth saying why rather than leaving a gap that looks like an
    /// oversight. `FIONREAD` on the master looks like the one and is not: measured on this box, it
    /// answers 0 immediately after a write of 8 bytes and 8 a millisecond later, because the bytes
    /// reach the line discipline's buffer through a work queue. An instrument that cannot tell
    /// *already consumed* from *not yet arrived* cannot carry this claim, and a gate built on it
    /// would have passed either way. What holds the ordering instead is that
    /// [`AttachedPty::spawn`] is unreachable without this call, and macOS is where the behaviour
    /// is judged.
    #[test]
    fn the_attach_probe_reaches_the_reader_it_attaches_and_nothing_after_it() {
        let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let attached = Pty::open(80, 24)
            .expect("open a pty")
            .attach_reader("attach-gate", {
                let seen = Arc::clone(&seen);
                move |mut terminal| {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = terminal.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.lock()
                            .expect("the seen mutex")
                            .extend_from_slice(&buf[..n]);
                    }
                }
            })
            .expect("a fresh pty takes a reader");

        write_from_the_childs_side(&attached.pty, b"hi");
        let got = until(
            || seen.lock().expect("the seen mutex").clone(),
            |seen| !seen.is_empty(),
        );
        assert_eq!(
            got, b"hi",
            "the body reads from the pane's first byte, with the probe already spent",
        );
    }

    /// **A child is born onto a terminal something is already reading.**
    ///
    /// `/bin/echo` and not a shell on purpose: a shell takes milliseconds to start, which is long
    /// enough to hide the window this is about. This child writes once and is gone.
    ///
    /// ⚠ SAY WHICH HALF THIS RUNS. On Linux it passes with the reader attached late as well —
    /// measured, 30 runs of the pane-level twin and a C probe that reads a fully reaped child's
    /// output 100 ms afterwards. It discriminates on a kernel that discards, which is macOS, and CI
    /// runs it there. What it is here is the regression gate that keeps `/bin/echo` — the shape
    /// that found this — driven at all.
    #[test]
    fn a_child_is_born_onto_a_terminal_something_is_already_reading() {
        let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut attached = Pty::open(80, 24)
            .expect("open a pty")
            .attach_reader("born-gate", {
                let seen = Arc::clone(&seen);
                move |mut terminal| {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = terminal.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.lock()
                            .expect("the seen mutex")
                            .extend_from_slice(&buf[..n]);
                    }
                }
            })
            .expect("a fresh pty takes a reader");

        let mut command = CommandBuilder::new("/bin/echo");
        command.arg("recorded");
        let (mut child, _joined) = attached.spawn(&command, None).expect("spawn onto the pty");
        child.wait().expect("the child is reapable");

        let got = until(
            || seen.lock().expect("the seen mutex").clone(),
            |seen| seen.starts_with(b"recorded"),
        );
        assert!(
            got.starts_with(b"recorded"),
            "a child that writes once and exits is still read: {:?}",
            String::from_utf8_lossy(&got),
        );
    }

    /// ⚠⚠⚠ **THE KERNEL SAYS WHO WILL PAINT THIS PANE'S INPUT**, and it changes when the program
    /// changes it.
    ///
    /// The claim is a DISCRIMINATOR, not a value: a device that answers the same thing before and
    /// after the child's `stty -echo` would be reporting a constant, and every caller that acts on
    /// it would be acting on nothing. So one pane is read twice, with the child's own `stty`
    /// between the readings, and the two must DISAGREE.
    ///
    /// ⚠ Written to hold on a platform whose master will not answer: `None` on both readings is
    /// accepted, and any platform that answers at all must answer correctly. That is R362's rule —
    /// a refusal asserted in its own platform's terms — and it is the shape that stops this gate
    /// going red on macOS for the one reason that would not be a defect.
    #[test]
    fn a_pane_says_whether_its_own_terminal_paints_what_is_typed_at_it() {
        let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut attached = Pty::open(80, 24)
            .expect("open a pty")
            .attach_reader("echo-gate", {
                let seen = Arc::clone(&seen);
                move |mut terminal| {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = terminal.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.lock()
                            .expect("the seen mutex")
                            .extend_from_slice(&buf[..n]);
                    }
                }
            })
            .expect("a fresh pty takes a reader");
        let query = attached.pty.query().expect("a query handle");

        // BEFORE: a device nobody has configured. This is the state every pane is born in, and the
        // one in which a screen read-back proves nothing about the program.
        let born = query.echo();

        // The child announces AFTER its `stty`, so the second reading cannot race it.
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty -echo; printf 'OFF'; sleep 30");
        let (mut child, _joined) = attached.spawn(&command, None).expect("spawn onto the pty");
        let _ = until(
            || seen.lock().expect("the seen mutex").clone(),
            |seen| seen.starts_with(b"OFF"),
        );
        let after = query.echo();
        let _ = child.kill();

        match (born, after) {
            (None, None) => { /* this platform's master does not answer — see the doc. */ }
            (Some(born), Some(after)) => {
                assert_eq!(
                    born,
                    PaneEcho::ByTheTerminal,
                    "a pane is born echoing, which is why a fresh one can never confirm an \
                     injection by reading its own screen",
                );
                assert_eq!(
                    after,
                    PaneEcho::ByTheProgram,
                    "and the program's own `stty -echo` is visible here — a reading that did not \
                     move is a constant, and a constant is not evidence",
                );
            }
            mixed => panic!(
                "a device that answers must go on answering — half an answer is neither reading: \
                 {mixed:?}",
            ),
        }
    }

    /// ⚠⚠⚠ **THE READING MOVES WHEN THE CHILD TAKES ITS SIGNALS AWAY** — and it is a
    /// DISCRIMINATOR, on the same terms as the echo gate above: one pty, read twice, with the
    /// child's own `stty -isig` in between, and the two readings must DISAGREE.
    ///
    /// This is the fact that makes `Ctrl-C` answerable. Before the `stty`, `0x03` is this device's
    /// interrupt character and the kernel says so; after it, the SAME byte raises nothing, and a
    /// caller who sent it to stop a job has written a byte a program will read as text.
    ///
    /// ⚠ The CHARACTER is asserted too, not only the flag. `raises` is what the surface consults,
    /// and a reading that answered `RaisedByTheTerminal` with no interrupt character in it would
    /// pass a flag-only assertion while telling every caller the opposite of the truth.
    ///
    /// ⚠ Written to hold where the master will not answer (`None` on both readings), for the echo
    /// gate's reason.
    #[test]
    fn a_pane_says_whether_a_ctrl_c_written_into_it_can_become_a_signal() {
        let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut attached = Pty::open(80, 24)
            .expect("open a pty")
            .attach_reader("signal-gate", {
                let seen = Arc::clone(&seen);
                move |mut terminal| {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = terminal.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.lock()
                            .expect("the seen mutex")
                            .extend_from_slice(&buf[..n]);
                    }
                }
            })
            .expect("a fresh pty takes a reader");
        let query = attached.pty.query().expect("a query handle");

        // BEFORE: the state every pane is born in, and the one in which a written `0x03` really is
        // an interrupt — which is exactly why the silence after the `stty` is so misleading.
        let born = query.signal_keys();

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty -isig; printf 'OFF'; sleep 30");
        let (mut child, _joined) = attached.spawn(&command, None).expect("spawn onto the pty");
        let _ = until(
            || seen.lock().expect("the seen mutex").clone(),
            |seen| seen.starts_with(b"OFF"),
        );
        let after = query.signal_keys();
        let _ = child.kill();

        let interrupt = SignalKey::Interrupt.conventional_byte();
        match (born, after) {
            (None, None) => { /* this platform's master does not answer — see the doc. */ }
            (Some(born), Some(after)) => {
                assert_eq!(
                    born.raises(interrupt),
                    Some(SignalKey::Interrupt),
                    "a pane is born turning `0x03` into a SIGINT, and the reading has to name \
                     WHICH signal — a bare yes could not tell a caller what it just did",
                );
                assert_eq!(
                    after,
                    PaneSignalKeys::DeliveredAsBytes,
                    "and the child's own `stty -isig` is visible here: this is the pane every \
                     full-screen agent CLI presents, where a written Ctrl-C is input",
                );
                assert_eq!(
                    after.raises(interrupt),
                    None,
                    "⚠⚠⚠ the same byte, the same pane, and no signal — the difference a write \
                     cannot report because a write succeeds either way",
                );
            }
            mixed => panic!(
                "a device that answers must go on answering — half an answer is neither reading: \
                 {mixed:?}",
            ),
        }
    }

    /// ⚠⚠ **A REBOUND INTERRUPT CHARACTER IS THE SECOND WAY THE SILENCE WOULD BE FALSE**, and the
    /// reason this reads `c_cc` rather than trusting `ISIG` alone.
    ///
    /// `stty intr ^X` leaves signals ON — a flag-only reading calls this pane healthy — while the
    /// `0x03` a caller sends for `Ctrl-C` becomes an ordinary byte. The device's own answer is
    /// asserted BOTH ways here: nothing is raised for `0x03`, and the interrupt is raised for the
    /// character the child actually bound, so the reading is a discriminator rather than a
    /// blanket refusal that would happen to look right.
    #[test]
    fn a_terminal_that_rebound_its_interrupt_does_not_raise_one_for_ctrl_c() {
        let seen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut attached = Pty::open(80, 24)
            .expect("open a pty")
            .attach_reader("rebind-gate", {
                let seen = Arc::clone(&seen);
                move |mut terminal| {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = terminal.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        seen.lock()
                            .expect("the seen mutex")
                            .extend_from_slice(&buf[..n]);
                    }
                }
            })
            .expect("a fresh pty takes a reader");
        let query = attached.pty.query().expect("a query handle");

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty intr ^X; printf 'MOVED'; sleep 30");
        let (mut child, _joined) = attached.spawn(&command, None).expect("spawn onto the pty");
        let _ = until(
            || seen.lock().expect("the seen mutex").clone(),
            |seen| seen.starts_with(b"MOVED"),
        );
        let after = query.signal_keys();
        let _ = child.kill();

        let Some(after) = after else {
            return; // this platform's master does not answer — see the doc.
        };
        assert!(
            matches!(after, PaneSignalKeys::RaisedByTheTerminal { .. }),
            "signals are still ON here — this pane is exactly the one a flag-only reading calls \
             healthy: {after:?}",
        );
        assert_eq!(
            after.raises(SignalKey::Interrupt.conventional_byte()),
            None,
            "⚠⚠ and yet the `0x03` a caller sends for Ctrl-C raises nothing, because this \
             terminal's interrupt character is somewhere else",
        );
        assert_eq!(
            after.raises(0x18),
            Some(SignalKey::Interrupt),
            "the character the child DID bind still raises it — so the answer discriminates \
             rather than refusing everything, which would have passed the assertion above while \
             being useless",
        );
    }
}
