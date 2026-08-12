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

    /// The device's local mode flags (`c_lflag`), or `None` where it will not answer.
    ///
    /// One `tcgetattr` behind every question above, so a reading is never assembled from two calls
    /// that could straddle a program changing its terminal — and so a new question costs a flag
    /// test rather than another syscall.
    fn local_modes(&self) -> Option<libc::tcflag_t> {
        let mut modes: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is open for the life of `self`, and `tcgetattr` only fills in the
        // fully-owned `termios` handed to it.
        if unsafe { libc::tcgetattr(self.0.as_raw_fd(), &raw mut modes) } != 0 {
            return None;
        }
        Some(modes.c_lflag)
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
            return Err(io::Error::last_os_error());
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
}
