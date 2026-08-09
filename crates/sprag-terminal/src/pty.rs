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
//! `Pty::open` and `Pty::spawn` as a `#[cfg(windows)]` sibling built on ConPTY; nothing above
//! this module names a file descriptor, so it would not have to change. What still blocks Windows
//! is not here: the wire is a Unix domain socket in three other crates.

#![cfg(unix)]

use std::ffi::{CStr, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::command::CommandBuilder;

/// A pseudoterminal pair: the master this process reads and writes, and the slave the child gets.
///
/// The slave is held only until a child is spawned onto it. Holding it any longer would keep the
/// device open after the child dies, and the master would never see EOF — the reader thread would
/// block forever on a pane whose program has already exited.
#[derive(Debug)]
pub struct Pty {
    /// The controlling side. Cloned for the reader and the writer; resized through.
    master: OwnedFd,
    /// The child's side, taken by [`Pty::spawn`] and dropped there.
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
        let size = winsize(cols, rows);
        // SAFETY: both descriptors are out-parameters the call fills in, `size` is a fully
        // initialised `winsize`, and the terminal-settings argument is deliberately null (the
        // child's shell sets its own).
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &raw const size,
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
    #[must_use]
    pub fn tty_name(&self) -> Option<PathBuf> {
        let mut buf = [0i8; 128];
        // SAFETY: `buf` is a valid writable buffer of the length passed, and the master is open.
        let named =
            unsafe { libc::ptsname_r(self.master.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
        if named != 0 {
            return None;
        }
        // SAFETY: on success `ptsname_r` wrote a NUL-terminated string into `buf`.
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
        let sized =
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
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
    /// # Errors
    ///
    /// Returns the OS error if the pty was already spawned onto, or if the child could not start.
    pub fn spawn(
        &mut self,
        command: &CommandBuilder,
        cgroup: Option<BorrowedFd<'_>>,
    ) -> io::Result<Child> {
        let slave = self.slave.take().ok_or_else(|| {
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
        // SAFETY: the closure runs in the child between `fork` and `exec`. Every call in it is
        // async-signal-safe and none of them allocates: two `ioctl`-class syscalls and one `write`
        // of a literal byte to a descriptor opened before the fork.
        unsafe {
            spawn.pre_exec(move || {
                // A session of its own, so this pty can become a controlling terminal at all.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                // `std` has already put the slave on fd 0, so THIS is the device to claim. Claiming
                // it is what makes Ctrl-C reach the child's foreground job instead of the daemon.
                if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if let Some(cgroup) = &cgroup {
                    // "0" means the calling process. The whole point of R336: the child is in its
                    // pane's cgroup before it becomes the pane's program.
                    let wrote = libc::write(cgroup.as_raw_fd(), c"0".as_ptr().cast(), 1);
                    if wrote < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        let child = spawn.spawn()?;
        // The child holds the device now. Dropping ours is what lets the master read EOF when it
        // exits, and it must happen whether or not anything else here succeeds.
        drop(slave);
        Ok(child)
    }
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
