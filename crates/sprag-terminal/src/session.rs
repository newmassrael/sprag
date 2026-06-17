//! The PTY-driven terminal session — the producer.
//!
//! [`TerminalSession`] owns an OS pseudoterminal (via `portable-pty`, the
//! cross-platform abstraction the README NFR mandates), the child process
//! running on its slave, and the [`Emulator`] its output feeds. A background
//! thread reads the master and applies bytes to the emulator **directly**,
//! so the authoritative screen is always current and bounded by the grid
//! size — there is no unbounded byte backlog and no caller-side pump step.
//!
//! Screen access goes through [`with_screen`](TerminalSession::with_screen)
//! under the emulator lock; because the reader applies one `advance` batch at
//! a time, every observed grid is consistent (termwiz buffers any partial
//! escape across batches).

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use sprag_vt::{Emulator, Screen, VtPort};

// Re-exported so callers build commands without depending on portable-pty
// directly (it is an implementation detail of the PTY seam).
pub use portable_pty::CommandBuilder;

/// A failure setting up or driving the pseudoterminal. Wraps the underlying
/// `portable-pty` / IO error message with the operation that produced it.
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
/// queryable [`Screen`] that the reader thread keeps current.
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    emulator: Arc<Mutex<Emulator>>,
    eof: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
    cols: u16,
    rows: u16,
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

        let emulator = Arc::new(Mutex::new(Emulator::new(cols, rows)));
        let eof = Arc::new(AtomicBool::new(false));
        let reader_emulator = Arc::clone(&emulator);
        let reader_eof = Arc::clone(&eof);
        let reader_thread = std::thread::Builder::new()
            .name("sprag-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => lock(&reader_emulator).advance(&buf[..n]),
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                reader_eof.store(true, Ordering::Release);
            })
            .map_err(|e| SessionError::new("spawn reader thread", &e))?;

        Ok(Self {
            master: pair.master,
            child,
            writer,
            emulator,
            eof,
            reader_thread: Some(reader_thread),
            cols,
            rows,
        })
    }

    /// Read the current authoritative screen under the emulator lock.
    pub fn with_screen<R>(&self, f: impl FnOnce(&Screen) -> R) -> R {
        f(lock(&self.emulator).screen())
    }

    /// An owned snapshot of the current screen.
    #[must_use]
    pub fn screen(&self) -> Screen {
        self.with_screen(Screen::clone)
    }

    /// Whether the child has closed the pseudoterminal (no more output).
    /// Once set, the reader thread has applied every byte it received.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }

    /// Write input bytes to the child. Keys are encoded to PTY bytes by the
    /// caller (encoding ownership is sprag's, per PINION-REQUIREMENTS R2.6).
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the pseudoterminal and the emulator to `cols × rows`, notifying
    /// the child via `TIOCSWINSZ` (DESIGN.md §3 winsize ownership).
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
        lock(&self.emulator).resize(cols, rows);
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// The current `(cols, rows)` the session is sized to.
    #[must_use]
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
}

/// Lock the emulator, recovering the guard if a holder panicked (the screen
/// grid stays structurally valid; `advance` does not panic in practice).
fn lock(emulator: &Mutex<Emulator>) -> MutexGuard<'_, Emulator> {
    emulator.lock().unwrap_or_else(PoisonError::into_inner)
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
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// End-to-end producer smoke test (DESIGN.md §5): real PTY output is
    /// parsed into the queryable screen, with no caller-side pump.
    #[test]
    fn pty_output_reaches_the_screen() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf hi");
        command.env("TERM", "dumb");
        let session = TerminalSession::spawn(command, 20, 4).expect("spawn pty session");

        let start = Instant::now();
        while !session.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let row0 = session.with_screen(|screen| {
            (0..screen.cols())
                .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.clone()))
                .collect::<String>()
        });
        assert!(row0.starts_with("hi"), "row0 = {row0:?}");
        assert_eq!(session.dimensions(), (20, 4));
    }
}
