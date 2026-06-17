//! The PTY-driven terminal session.
//!
//! [`TerminalSession`] owns an OS pseudoterminal (via `portable-pty`, the
//! cross-platform abstraction the README NFR mandates), the child process
//! running on its slave, and the [`Emulator`] its output feeds. A
//! background thread reads the master and hands bytes to the session, which
//! [`pump`](TerminalSession::pump)s them into the emulator on demand — the
//! producer side of the walking-skeleton slice (DESIGN.md §5).

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use sprag_vt::{Emulator, Screen, VtPort};

use crate::TextGridSnapshot;

// Re-exported so callers build commands without depending on portable-pty
// directly (it is an implementation detail of the VT-input seam).
pub use portable_pty::CommandBuilder;

/// A failure setting up or driving the pseudoterminal. Wraps the
/// underlying `portable-pty` / IO error message with the operation that
/// produced it.
#[derive(Debug)]
pub struct SessionError {
    context: &'static str,
    source: String,
}

impl SessionError {
    fn new(context: &'static str, source: &dyn std::fmt::Display) -> Self {
        Self {
            context,
            source: source.to_string(),
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "terminal session: {} failed: {}", self.context, self.source)
    }
}

impl std::error::Error for SessionError {}

/// A live terminal: a child process on a PTY, its output parsed into a
/// queryable [`Screen`].
///
/// Output is read off-thread and buffered; call [`pump`](Self::pump) (or
/// [`pump_blocking`](Self::pump_blocking)) to fold buffered bytes into the
/// emulator before reading the [`screen`](Self::screen) or a
/// [`snapshot`](Self::snapshot).
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    reader_thread: Option<JoinHandle<()>>,
    emulator: Emulator,
    cols: u16,
    rows: u16,
    eof: bool,
}

impl TerminalSession {
    /// Spawn `command` on a fresh `cols × rows` pseudoterminal.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the PTY cannot be opened, the child
    /// cannot be spawned, or the master reader/writer cannot be acquired.
    pub fn spawn(command: CommandBuilder, cols: u16, rows: u16) -> Result<Self, SessionError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system
            .openpty(size)
            .map_err(|e| SessionError::new("open pty", &e))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| SessionError::new("spawn command", &e))?;
        // The child now holds the slave fd; drop ours so the master reads
        // EOF once the child exits (otherwise the reader blocks forever).
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SessionError::new("clone reader", &e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SessionError::new("take writer", &e))?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let reader_thread = std::thread::Builder::new()
            .name("sprag-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| SessionError::new("spawn reader thread", &e))?;

        Ok(Self {
            master: pair.master,
            child,
            writer,
            rx,
            reader_thread: Some(reader_thread),
            emulator: Emulator::new(cols, rows),
            cols,
            rows,
            eof: false,
        })
    }

    /// Fold all currently-buffered PTY output into the emulator without
    /// blocking. Sets [`is_eof`](Self::is_eof) once the child has closed.
    pub fn pump(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(chunk) => self.emulator.advance(&chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.eof = true;
                    break;
                }
            }
        }
    }

    /// Block up to `timeout` for at least one chunk of PTY output, fold it
    /// (and anything already buffered) into the emulator, and return
    /// whether any bytes were applied. Returns `false` on timeout or once
    /// the child has closed (see [`is_eof`](Self::is_eof)).
    pub fn pump_blocking(&mut self, timeout: Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(chunk) => {
                self.emulator.advance(&chunk);
                self.pump();
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => {
                self.eof = true;
                false
            }
        }
    }

    /// The current authoritative screen (call a `pump` first to apply any
    /// pending output).
    #[must_use]
    pub fn screen(&self) -> &Screen {
        self.emulator.screen()
    }

    /// The current screen as scene-as-data — the [`TextGridSnapshot`] an AI
    /// consumer reads over `scene/snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> TextGridSnapshot {
        crate::snapshot(self.screen())
    }

    /// Write input bytes to the child (keys are encoded to PTY bytes by the
    /// caller; encoding ownership is sprag's, per PINION-REQUIREMENTS R2.6).
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the pseudoterminal and the emulator to `cols × rows`,
    /// notifying the child via `TIOCSWINSZ` (DESIGN.md §3 winsize SSOT).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the master resize fails.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), SessionError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SessionError::new("resize pty", &e))?;
        self.emulator.resize(cols, rows);
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// Whether the child has closed the pseudoterminal (no more output).
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.eof
    }

    /// The current `(cols, rows)` the session is sized to.
    #[must_use]
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Stop the child so its slave fd closes, which unblocks the reader
        // thread's `read()` with EOF; then reap it and join the thread.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// End-to-end walking-skeleton smoke test (DESIGN.md §5,
    /// PINION-REQUIREMENTS "sprag 측 남은 작업" item 1): real PTY output
    /// travels all the way to a scene/snapshot, headlessly.
    #[test]
    fn pty_output_reaches_the_snapshot() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf hi");
        command.env("TERM", "dumb");

        let mut session = TerminalSession::spawn(command, 20, 4).expect("spawn pty session");

        let deadline = Duration::from_secs(5);
        let start = Instant::now();
        while !session.is_eof() && start.elapsed() < deadline {
            session.pump_blocking(Duration::from_millis(200));
        }
        session.pump();

        let snapshot = session.snapshot();
        assert_eq!((snapshot.cols, snapshot.rows), (20, 4));
        let joined: String = snapshot.grid_rows.iter().map(|r| r.text.as_str()).collect();
        assert!(
            joined.contains("hi"),
            "expected 'hi' in snapshot, rows: {:?}",
            snapshot
                .grid_rows
                .iter()
                .map(|r| r.text.trim_end())
                .collect::<Vec<_>>()
        );
    }
}
