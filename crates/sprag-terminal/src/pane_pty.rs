//! A pane's pseudoterminal — the producer.
//!
//! [`PanePty`] owns an OS pseudoterminal (via `portable-pty`, the
//! cross-platform abstraction the README NFR mandates), the child process
//! running on its slave, and the [`Emulator`] its output feeds. A background
//! thread reads the master and applies bytes to the emulator **directly**,
//! so the authoritative screen is always current and bounded by the grid
//! size — there is no unbounded byte backlog and no caller-side pump step.
//!
//! Screen access goes through [`with_screen`](PanePty::with_screen)
//! under the emulator lock; because the reader applies one `advance` batch at
//! a time, every observed grid is consistent (termwiz buffers any partial
//! escape across batches).

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, PtySize, native_pty_system};
use sprag_vt::{Emulator, InputModes, Notification, Screen, VtPort};

// Re-exported so callers build commands without depending on portable-pty
// directly (it is an implementation detail of the PTY seam).
pub use portable_pty::CommandBuilder;

/// The PTY master writer, shared (and interior-mutable) so both the owning
/// [`PanePty`] and any [`PanePtyHandle`] can inject input without a
/// `&mut` borrow — the pty is already concurrent (the reader thread
/// holds the emulator lock), so the writer is shared the same way.
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Upper bound on the raw-output capture buffer ([`RawCapture`]). Generous
/// enough for a large structured envelope (a `claude -p --output-format json`
/// reply with a code-block `result` is a few KiB to tens of KiB), bounded so a
/// runaway child cannot grow it without limit. A child that exceeds it marks
/// the capture `truncated`, which a structured reader treats as an unparseable
/// reply and degrades gracefully.
const RAW_CAPTURE_CAP: usize = 256 * 1024;

/// A snapshot of a pane child's raw output: the captured source bytes paired
/// with whether the capture hit its cap (so it is incomplete — a structured
/// reader treats a truncated capture as unparseable and degrades). Named, not a
/// bare `(Vec<u8>, bool)` tuple, so every seam reads the two fields by name. The
/// bytes are the child's **source** stream, before the emulator renders them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawOutput {
    /// The captured source bytes (head-anchored, bounded by `RAW_CAPTURE_CAP`).
    pub bytes: Vec<u8>,
    /// Whether the capture hit the cap and stopped: `bytes` is a prefix of the
    /// child's output, not the whole, so a structured read should degrade.
    pub truncated: bool,
}

/// A bounded, head-anchored capture of the child's raw output bytes — the
/// **source** stream, before the emulator renders it onto the grid. Structured
/// machine output (a JSON envelope) must be read from here, not reconstructed
/// from the display grid: the grid wraps a long logical line across rows,
/// trailing-trims each row, and strips control bytes, none of which is
/// reversible. The source bytes are exact.
///
/// Head-anchored (append until the cap, then stop and mark `truncated`) rather
/// than a tail ring, because a structured envelope is parsed from its **start**
/// (`{`…); evicting the head to keep the tail would corrupt every parse. An
/// over-cap child is the bounded-degradation case, not the common one.
struct RawCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

impl RawCapture {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
        }
    }

    /// Append `more`, up to [`RAW_CAPTURE_CAP`]; past the cap, keep the head
    /// already captured and latch `truncated`.
    fn push(&mut self, more: &[u8]) {
        if self.truncated {
            return;
        }
        let room = RAW_CAPTURE_CAP - self.bytes.len();
        if more.len() <= room {
            self.bytes.extend_from_slice(more);
        } else {
            self.bytes.extend_from_slice(&more[..room]);
            self.truncated = true;
        }
    }

    /// The bytes captured so far, paired with whether the cap was hit (the
    /// capture is incomplete and a structured read should degrade).
    fn snapshot(&self) -> RawOutput {
        RawOutput {
            bytes: self.bytes.clone(),
            truncated: self.truncated,
        }
    }
}

/// Shared raw-output capture: the reader thread appends to it as it reads the
/// PTY master; a [`PanePtyHandle`] snapshots it. Shared the same `Arc<Mutex>`
/// way as the emulator and the eof flag.
type SharedRawCapture = Arc<Mutex<RawCapture>>;

/// A failure setting up or driving the pseudoterminal. Wraps the underlying
/// `portable-pty` / IO error message with the operation that produced it.
#[derive(Debug)]
pub struct PanePtyError {
    context: &'static str,
    source: String,
}

impl PanePtyError {
    fn new(context: &'static str, source: &dyn std::fmt::Display) -> Self {
        Self {
            context,
            source: source.to_string(),
        }
    }
}

impl std::fmt::Display for PanePtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pane pty: {} failed: {}", self.context, self.source)
    }
}

impl std::error::Error for PanePtyError {}

/// A live terminal: a child process on a PTY, its output parsed into a
/// queryable [`Screen`] that the reader thread keeps current.
pub struct PanePty {
    // The PTY master is OWNED BY the resize coalescer thread (`resize` is its
    // sole user), so resizes apply off the caller's thread and are debounced —
    // see [`run_resize_coalescer`]. The pty hands target sizes to it via
    // `resize_tx`.
    child: Box<dyn Child + Send + Sync>,
    writer: SharedWriter,
    emulator: Arc<Mutex<Emulator>>,
    raw_output: SharedRawCapture,
    eof: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
    resize_tx: Option<Sender<(u16, u16)>>,
    resize_thread: Option<JoinHandle<()>>,
}

impl PanePty {
    /// Spawn `command` on a fresh `cols × rows` pseudoterminal.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the PTY cannot be opened, the child
    /// cannot be spawned, or the master reader/writer cannot be acquired.
    pub fn spawn(command: CommandBuilder, cols: u16, rows: u16) -> Result<Self, PanePtyError> {
        Self::spawn_with_dirty(command, cols, rows, None, None)
    }

    /// [`Self::spawn`] with two reader-thread callbacks:
    ///
    /// * `on_dirty` — invoked after each parsed PTY batch is applied to the screen,
    ///   **and once more when the child exits** — after [`is_eof`](Self::is_eof)
    ///   publishes, so a wake can never observe the pane it announces as still live.
    ///   This is the sprag side of the pinion R999 `RepaintSink` seam. A windowed host
    ///   passes `Some(Box::new(move || sink.request_repaint()))`; the headless host
    ///   passes `bump_on_dirty`, which moves the scene revision so a parked
    ///   `scene/waitFor` wakes.
    /// * `on_exit` — invoked EXACTLY ONCE, when the child has exited (the reader loop
    ///   reached EOF), after `on_dirty`'s exit wake. This is the "this child is gone"
    ///   event as distinct from "this child produced output", so a caller can act on a
    ///   pane's death without a per-output-batch check: the daemon reads it to end its
    ///   own process when the last live pane dies. Fired after `is_eof` publishes, so a
    ///   liveness scan run from it never counts the pane it announces.
    ///
    /// Both are deliberately pinion-free (`Box<dyn Fn() + Send>`), so this crate stays
    /// decoupled from the GUI shell and the host lifetime. `None` is for a caller with
    /// nothing to do (this crate's own tests, and [`spawn`](Self::spawn)).
    ///
    /// # Errors
    ///
    /// Same as [`Self::spawn`].
    pub fn spawn_with_dirty(
        command: CommandBuilder,
        cols: u16,
        rows: u16,
        on_dirty: Option<Box<dyn Fn() + Send>>,
        on_exit: Option<Box<dyn Fn() + Send>>,
    ) -> Result<Self, PanePtyError> {
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
            .map_err(|e| PanePtyError::new("open pty", &e))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| PanePtyError::new("spawn command", &e))?;
        // The child now holds the slave fd; drop ours so the master reads
        // EOF once the child exits (otherwise the reader blocks forever).
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PanePtyError::new("clone reader", &e))?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| PanePtyError::new("take writer", &e))?,
        ));

        let emulator = Arc::new(Mutex::new(Emulator::new(cols, rows)));
        let raw_output: SharedRawCapture = Arc::new(Mutex::new(RawCapture::new()));
        let eof = Arc::new(AtomicBool::new(false));
        let reader_emulator = Arc::clone(&emulator);
        let reader_raw = Arc::clone(&raw_output);
        let reader_eof = Arc::clone(&eof);
        let reader_thread = std::thread::Builder::new()
            .name("sprag-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // Tee the source bytes into the capture, then render
                            // them. Both happen in this iteration before the loop
                            // can break, so once `eof` is observed every byte is
                            // BOTH captured and applied to the screen — the same
                            // completeness guarantee `is_eof` gives the grid.
                            lock(&reader_raw).push(&buf[..n]);
                            lock(&reader_emulator).advance(&buf[..n]);
                            // R999 seam: wake the windowed host to repaint
                            // now that this batch is applied (no-op headless).
                            if let Some(ref notify) = on_dirty {
                                notify();
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                // Publish EOF BEFORE either callback, never after: both run a liveness
                // check off this flag, and a call that overtook it would count the very
                // pane that just died as live.
                reader_eof.store(true, Ordering::Release);
                // Wake the host for the child's EXIT, not just its output. A pane dying
                // changes what a client draws — but the loop above notified only on
                // `Ok(n)`, so an exit reached whoever happened to poll next and nobody else.
                if let Some(ref notify) = on_dirty {
                    notify();
                }
                // The distinct "this child is gone" event: fired once, so a liveness scan
                // (the daemon's exit-when-empty) runs per DEATH, never per output batch.
                if let Some(ref on_exit) = on_exit {
                    on_exit();
                }
            })
            .map_err(|e| PanePtyError::new("spawn reader thread", &e))?;

        // `TIOCSWINSZ` coalescer: own thread, owns the PTY master (its only
        // user). The caller's reflow (a continuous drag) resizes the emulator
        // synchronously and sends every intermediate size here; the coalescer
        // debounces the PTY ioctl to the final one — so the live shell gets one
        // `SIGWINCH` per settle, not one per cell-width boundary.
        let (resize_tx, resize_rx) = mpsc::channel::<(u16, u16)>();
        let master = pair.master;
        let resize_thread = std::thread::Builder::new()
            .name("sprag-pty-resize".to_string())
            .spawn(move || {
                run_resize_coalescer(RESIZE_DEBOUNCE, &resize_rx, |(cols, rows)| {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                });
            })
            .map_err(|e| PanePtyError::new("spawn resize thread", &e))?;

        Ok(Self {
            child,
            writer,
            emulator,
            raw_output,
            eof,
            reader_thread: Some(reader_thread),
            resize_tx: Some(resize_tx),
            resize_thread: Some(resize_thread),
        })
    }

    /// Read the current authoritative screen under the emulator lock.
    pub fn with_screen<R>(&self, f: impl FnOnce(&Screen) -> R) -> R {
        f(lock(&self.emulator).screen())
    }

    /// The child's self-reported window title (`OSC 0` / `OSC 2`), `None` until it sets
    /// one. LIVE state (a shell rewrites it every prompt), distinct from the pane's
    /// spawn [`command_label`](crate::workspace::Pane::command_label). Owned, because
    /// it is read out from under the emulator lock.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        lock(&self.emulator).title().map(str::to_owned)
    }

    /// The most recent attention notification the child raised (`OSC 9` / `OSC 777;notify`
    /// / `OSC 99`), or `None`, paired with its monotonic sequence — see
    /// [`VtPort::notification`]. Owned, read out from under the emulator lock in ONE take so
    /// the payload and its sequence are consistent (a consumer detects a new one via the
    /// sequence growing, so they must not tear).
    #[must_use]
    pub fn notification(&self) -> (Option<Notification>, u64) {
        let emu = lock(&self.emulator);
        (emu.notification().cloned(), emu.notification_seq())
    }

    /// The monotonic count of BELLs (`\a`) the child has rung — the tmux `monitor-bell` signal,
    /// kept apart from the notification (a bell carries no text). See [`VtPort::bell_seq`].
    #[must_use]
    pub fn bell_seq(&self) -> u64 {
        lock(&self.emulator).bell_seq()
    }

    /// An owned snapshot of the current screen.
    #[must_use]
    pub fn screen(&self) -> Screen {
        self.with_screen(Screen::clone)
    }

    /// The OS process id of the child on this pty's slave.
    ///
    /// The current backend (portable-pty over [`std::process::Child`]) reports the id for the whole
    /// lifetime of this `PanePty` — it does NOT clear when the child exits — so a `Some` here is NOT
    /// proof the child is still alive; a caller wanting liveness must consult
    /// [`is_eof`](Self::is_eof). `None` is only the trait's allowance for a backend that cannot
    /// report an id. Consumers that read a pid to inspect `/proc` (cwd, listening ports) rely on
    /// only ever being handed a pid whose child is unreaped — see the note on the registry's
    /// `window_pids`.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// The child's current working directory, read LIVE from the OS.
    ///
    /// This is the fact the durability ring restores: a reboot kills the PTY and the child,
    /// but a fresh shell re-spawned in this directory puts the pane back where the user was
    /// working. Read at snapshot time (a shell rewrites its cwd on every `cd`), so it is NOT
    /// stored on the pane — like [`title`](Self::title), it is live child state, not the
    /// stable `command_label`.
    ///
    /// `None` when the child has exited (no pid), the directory was removed out from under
    /// it, or the platform has no `/proc` (Linux-only for now; elsewhere a restored pane
    /// falls back to the daemon's own cwd).
    #[must_use]
    pub fn cwd(&self) -> Option<PathBuf> {
        read_cwd(self.child.process_id()?)
    }

    /// Whether the child has closed the pseudoterminal (no more output).
    /// Once set, the reader thread has applied every byte it received.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }

    /// A snapshot of the child's raw output bytes (the source stream, before
    /// emulation) paired with whether the capture was truncated at the cap.
    /// Once [`is_eof`](Self::is_eof) holds, this is the child's complete output.
    /// Use it — not the rendered screen — to read structured machine output (a
    /// JSON envelope), which the grid would corrupt by wrapping and trimming.
    #[must_use]
    pub fn raw_output(&self) -> RawOutput {
        lock(&self.raw_output).snapshot()
    }

    /// Write input bytes to the child. Keys are encoded to PTY bytes by the
    /// caller (encoding ownership is sprag's, per PINION-REQUIREMENTS R2.6).
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        write_shared(&self.writer, bytes)
    }

    /// A cloneable [`PanePtyHandle`] sharing this pty's emulator and
    /// PTY writer — the seam an input-injecting consumer (the host's pane
    /// `External`) holds to read the screen/modes and write encoded keys
    /// without owning the pty.
    #[must_use]
    pub fn handle(&self) -> PanePtyHandle {
        PanePtyHandle {
            emulator: Arc::clone(&self.emulator),
            writer: Arc::clone(&self.writer),
            raw_output: Arc::clone(&self.raw_output),
        }
    }

    /// Resize the pseudoterminal and the emulator to `cols × rows`, notifying
    /// the child via `TIOCSWINSZ` (DESIGN.md §3 winsize ownership).
    ///
    /// Takes `&self`: `MasterPty::resize` is `&self` and the emulator is behind
    /// a `Mutex`, so the size is updated through interior mutability with no
    /// exclusive borrow. This is what lets a shared `&PanePty` (e.g. a
    /// pane reached through an `Rc` in the GUI's resize Effect) reflow the PTY
    /// without owning it. The size is held only by the emulator (see
    /// [`dimensions`](Self::dimensions)), so there is no cache field to update.
    ///
    /// The PTY winsize ioctl is set BEFORE the emulator lock is taken (a blocking
    /// ioctl must not be held across the lock, which would stall the reader
    /// thread on every resize), so for one uncontended lock acquisition a
    /// `SIGWINCH`-racing child can observe the new winsize before the emulator
    /// reflects it. The disagreement is transient and self-heals within the next
    /// batch; it is visual-only, never a lost update.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the master resize fails.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PanePtyError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // Resize the emulator screen SYNCHRONOUSLY so the grid reflows the SAME
        // frame the GUI lays it out (the same-frame reflow contract the host's
        // dirty re-pass relies on). Only the PTY ioctl (`TIOCSWINSZ` →
        // `SIGWINCH`) is debounced: a continuous drag would otherwise flood the
        // live shell with `SIGWINCH`es and it redraws its prompt for every one,
        // which the emulator accumulates (the reported bug). The coalescer
        // applies one ioctl per settle.
        lock(&self.emulator).resize(cols, rows);
        if let Some(tx) = &self.resize_tx {
            // A send only fails once the pty is dropping (coalescer gone),
            // where a stale PTY size is moot.
            let _ = tx.send((cols, rows));
        }
        Ok(())
    }

    /// The current `(cols, rows)` the pty is sized to, read from the
    /// emulator screen — the single source of the size (the emulator's
    /// dimensions are authoritative; there is no duplicate cache field). The PTY
    /// winsize is updated alongside the emulator in [`resize`](Self::resize),
    /// though not atomically with it (see that method's note).
    #[must_use]
    pub fn dimensions(&self) -> (u16, u16) {
        let emulator = lock(&self.emulator);
        let screen = emulator.screen();
        (screen.cols(), screen.rows())
    }
}

/// A cloneable handle to a live pty's shared I/O: the emulator (read
/// the screen and input modes) and the PTY writer (inject input bytes).
/// Both are `Arc`-shared with the owning [`PanePty`] and its reader
/// thread, so a handle stays valid for the pty's lifetime. This is the
/// producer-side seam the host's pane `External` holds to drive input
/// without owning the pty (DESIGN.md §3 producer ownership; R2.6).
#[derive(Clone)]
pub struct PanePtyHandle {
    emulator: Arc<Mutex<Emulator>>,
    writer: SharedWriter,
    raw_output: SharedRawCapture,
}

impl PanePtyHandle {
    /// Read the current authoritative screen under the emulator lock.
    pub fn with_screen<R>(&self, f: impl FnOnce(&Screen) -> R) -> R {
        f(lock(&self.emulator).screen())
    }

    /// The current input modes (DECCKM, …) the key encoder consults.
    #[must_use]
    pub fn input_modes(&self) -> InputModes {
        lock(&self.emulator).input_modes()
    }

    /// A snapshot of the child's raw output bytes (the source stream, before
    /// emulation) paired with whether the capture was truncated at the cap —
    /// the seam a control plugin reads to parse structured output from a pane
    /// it does not own. See [`PanePty::raw_output`].
    #[must_use]
    pub fn raw_output(&self) -> RawOutput {
        lock(&self.raw_output).snapshot()
    }

    /// Write already-encoded input bytes to the child (R2.6: the caller
    /// owns key→byte encoding).
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        write_shared(&self.writer, bytes)
    }
}

/// Read a process's current working directory from the OS.
///
/// Linux: the kernel exposes it as the `/proc/<pid>/cwd` symlink, resolved to the real
/// path (which is why a restore that reads this then re-spawns a shell there survives a
/// `cd`). `None` if the process is gone or the link cannot be read (a removed directory,
/// a permission the caller lacks — though a child of this process is always readable).
#[cfg(target_os = "linux")]
fn read_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// No `/proc` off Linux: cwd-from-pid needs a platform-specific syscall not yet wired, so a
/// restored pane falls back to the daemon's own cwd. An honest `None`, not a guess.
#[cfg(not(target_os = "linux"))]
fn read_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

/// Lock a pty mutex (emulator or raw capture), recovering the guard if a
/// holder panicked (the grid stays structurally valid and the byte buffer is
/// plain data; neither `advance` nor `push` panics in practice).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Write all bytes to the shared PTY writer and flush, recovering the lock
/// if a holder panicked.
fn write_shared(writer: &SharedWriter, bytes: &[u8]) -> io::Result<()> {
    let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
    writer.write_all(bytes)?;
    writer.flush()
}

/// The quiet window the resize coalescer waits for before applying a size. A
/// continuous splitter/window drag emits a distinct `(cols, rows)` at every
/// cell-width boundary; without coalescing each one issues a `TIOCSWINSZ` →
/// `SIGWINCH`, and a live shell redraws its prompt for every one (the emulator,
/// which does not yet rewrap, then accumulates them as fragmented copies).
/// Debouncing to the LATEST size after a brief quiet collapses the storm.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(40);

/// The resize coalescer loop (runs on its own thread, which OWNS the PTY
/// master — `resize` is the master's only user, so no sharing is needed).
/// Trailing debounce: every request resets the quiet timer and overwrites the
/// pending size (last-write-wins), so only the FINAL size of a burst is applied,
/// once, after `quiet` of silence. On channel disconnect (the pty dropping)
/// it flushes the final pending size synchronously so a resize-then-quit never
/// strands the PTY at a stale size. Generic over `apply` so the debounce policy
/// is unit-tested without a real PTY.
fn run_resize_coalescer(
    quiet: Duration,
    rx: &Receiver<(u16, u16)>,
    mut apply: impl FnMut((u16, u16)),
) {
    let mut pending: Option<(u16, u16)> = None;
    loop {
        let recv = if pending.is_some() {
            rx.recv_timeout(quiet)
        } else {
            // Nothing pending — block until the next request (or disconnect).
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match recv {
            Ok(size) => pending = Some(size), // reset the timer, keep the latest
            Err(RecvTimeoutError::Timeout) => {
                if let Some(size) = pending.take() {
                    apply(size);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(size) = pending.take() {
                    apply(size); // flush the final size before exiting
                }
                break;
            }
        }
    }
}

impl Drop for PanePty {
    fn drop(&mut self) {
        // Disconnect the resize channel so the coalescer flushes its final
        // pending size and exits (it owns the PTY master, which then closes);
        // join it before reaping the child.
        drop(self.resize_tx.take());
        if let Some(handle) = self.resize_thread.take() {
            let _ = handle.join();
        }
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
    use std::sync::atomic::AtomicUsize;
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
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");

        let start = Instant::now();
        while !pty.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let row0 = pty.with_screen(|screen| {
            (0..screen.cols())
                .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.clone()))
                .collect::<String>()
        });
        assert!(row0.starts_with("hi"), "row0 = {row0:?}");
        assert_eq!(pty.dimensions(), (20, 4));
    }

    /// `pid` resolves a live child, and `cwd` reads where it is working — the fact the
    /// durability ring snapshots so a restored shell re-spawns in the same directory. The
    /// child is `cd`'d into a known dir at spawn (`CommandBuilder::cwd`), and `cwd()` reads
    /// it back through `/proc`. Linux-only: elsewhere `cwd()` is an honest `None`.
    #[cfg(target_os = "linux")]
    #[test]
    fn pid_and_cwd_report_the_live_child() {
        let dir = std::env::temp_dir();
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // long-lived: keeps the child (and its cwd) alive
        command.cwd(&dir);
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");

        assert!(pty.pid().is_some(), "a live child has a process id");
        let cwd = pty.cwd().expect("a live child's cwd is readable on Linux");
        // Canonicalize both: the spawn dir may be a symlink (e.g. /tmp), while /proc
        // resolves the link to the real path — comparing raw strings would spuriously fail.
        assert_eq!(
            cwd.canonicalize().ok(),
            dir.canonicalize().ok(),
            "cwd tracks the directory the child was spawned in",
        );
    }

    /// `resize` is `&self`: a shared `&PanePty` reflows the PTY +
    /// emulator (the capability the GUI resize Effect needs through an `Rc`),
    /// and `dimensions()` reports the new size from the emulator — the single
    /// source, with no stale cache field to drift.
    #[test]
    fn resize_through_a_shared_ref_updates_dimensions() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // long-lived: keeps the PTY open across the resize
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        assert_eq!(pty.dimensions(), (20, 4));
        // Through a SHARED borrow — proves the resize needs no `&mut`.
        let shared: &PanePty = &pty;
        shared.resize(100, 30).expect("resize through a shared ref");
        // The emulator resizes synchronously (only the PTY ioctl is debounced),
        // so `dimensions()` is current immediately.
        assert_eq!(pty.dimensions(), (100, 30), "dimensions track the emulator");
        // The floor at 1x1 holds (a zero dimension cannot reach the PTY).
        shared.resize(0, 0).expect("resize floors at 1x1");
        assert_eq!(pty.dimensions(), (1, 1));
    }

    /// Wait (bounded) until the child has exited and all its bytes are applied.
    fn wait_eof(pty: &PanePty) {
        let start = Instant::now();
        while !pty.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert!(pty.is_eof(), "child did not reach EOF in time");
    }

    /// A child that exits without ever writing a byte still wakes the host, and EOF
    /// is published by the time that wake lands.
    ///
    /// The `Ok(n)` branch is the only other notifier and it NEVER runs for a silent
    /// child, so a wake observed here can have come from the exit path alone — the
    /// guard cannot pass by accident. The wake is what the daemon ends its process on
    /// (last live pane gone) and what repaints a dead pane's final screen; before it,
    /// an exit reached only whoever happened to poll next.
    ///
    /// The second assert is a SANITY CHECK that EOF is observable once the wake lands, not a
    /// guard on the store-before-notify ORDER — that order is guaranteed by construction (the
    /// reader stores `Release` then calls the callbacks, sequentially on one thread), and a
    /// test cannot reliably pin it: reverse the two and the reader usually still wins the
    /// store before the main thread reads `is_eof`, so the swap slips past most runs. The real
    /// dependant (the daemon's liveness scan) is exercised where it lives, in
    /// `sprag-host::reap_hook_fires_only_when_the_last_pane_is_gone`.
    #[test]
    fn a_silent_childs_exit_still_wakes_the_host() {
        let (tx, rx) = mpsc::channel();
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("exec true"); // exits at once, writes nothing to the pty
        command.env("TERM", "dumb");
        let pty = PanePty::spawn_with_dirty(
            command,
            20,
            4,
            Some(Box::new(move || {
                let _ = tx.send(());
            })),
            None,
        )
        .expect("spawn a pty");

        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the child's exit woke nobody",
        );
        assert!(
            pty.is_eof(),
            "EOF is observable once the exit wake has landed (a sanity check, not an \
             ordering guard — see the fn doc)",
        );
    }

    /// `on_exit` fires exactly once, at the child's exit, and `is_eof` holds by then — the
    /// distinct death event the daemon ends its process on. A child that writes some output
    /// first proves the fire is tied to EOF, not to output: the counter is still 1.
    #[test]
    fn on_exit_fires_once_at_the_childs_exit_after_eof_publishes() {
        let exits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&exits);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf hi"); // writes output, THEN exits
        command.env("TERM", "dumb");
        let pty = PanePty::spawn_with_dirty(
            command,
            20,
            4,
            None,
            Some(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
        )
        .expect("spawn a pty");

        let start = Instant::now();
        while exits.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert_eq!(
            exits.load(Ordering::SeqCst),
            1,
            "on_exit fires once at the child's exit, not per output batch",
        );
        assert!(pty.is_eof(), "and EOF was published before it fired");
    }

    /// The raw capture is byte-faithful even when the output is a single
    /// logical line far longer than the grid width — exactly the case the
    /// rendered screen mangles (wrap `\n` injection + trailing-trim). This is
    /// why structured output is read from the source stream, not the grid.
    #[test]
    fn raw_output_captures_a_wrapping_line_byte_for_byte() {
        // A 300-char single line with no trailing newline, on a 20-col pane:
        // the grid wraps it across 15 rows; the source bytes are one line.
        let payload = "x".repeat(150) + &" spaced  words ".repeat(10);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%s' '{payload}'"));
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        wait_eof(&pty);

        let RawOutput { bytes, truncated } = pty.raw_output();
        assert!(!truncated, "small payload must not truncate");
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            payload,
            "raw capture must equal the emitted bytes exactly (no wrap, no trim)"
        );
        // The handle sees the same capture.
        assert_eq!(pty.handle().raw_output().bytes, bytes);
    }

    /// A child that emits more than the cap latches `truncated` and keeps the
    /// head it already captured — the bounded-degradation path a structured
    /// reader treats as unparseable.
    #[test]
    fn raw_output_truncates_past_the_cap() {
        // Emit one more KiB than the cap with `yes` piped through `head -c`.
        let want = RAW_CAPTURE_CAP + 1024;
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("yes a | tr -d '\\n' | head -c {want}"));
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 80, 24).expect("spawn a pty");
        wait_eof(&pty);

        let RawOutput { bytes, truncated } = pty.raw_output();
        assert!(
            truncated,
            "an over-cap child must mark the capture truncated"
        );
        assert_eq!(
            bytes.len(),
            RAW_CAPTURE_CAP,
            "capture is bounded at the cap"
        );
        assert!(bytes.iter().all(|&b| b == b'a'), "the head bytes are kept");
    }

    /// A1 resize debounce: a rapid burst of distinct sizes (a continuous drag)
    /// collapses to ONE applied size — the FINAL one — and a disconnect (the
    /// pty dropping) flushes the final pending size so it is never stranded.
    /// Pure policy test (no real PTY) via the `apply`-generic coalescer.
    #[test]
    fn resize_coalescer_debounces_to_the_final_size_and_flushes_on_drop() {
        let (tx, rx) = mpsc::channel::<(u16, u16)>();
        let applied = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&applied);
        let handle = std::thread::spawn(move || {
            run_resize_coalescer(Duration::from_millis(20), &rx, move |size| {
                recorder.lock().unwrap().push(size);
            });
        });

        // A burst with no quiet between sends -> only the final size applies.
        tx.send((10, 5)).unwrap();
        tx.send((20, 5)).unwrap();
        tx.send((30, 5)).unwrap();
        sleep(Duration::from_millis(80)); // > quiet -> the trailing flush fires
        assert_eq!(
            *applied.lock().unwrap(),
            vec![(30, 5)],
            "a drag's burst collapses to a single applied size (the final one)",
        );

        // A further size then disconnect -> flushed synchronously on shutdown.
        tx.send((40, 6)).unwrap();
        drop(tx);
        handle.join().unwrap();
        assert_eq!(
            *applied.lock().unwrap(),
            vec![(30, 5), (40, 6)],
            "disconnect (Drop) flushes the final pending size, never stranding it",
        );
    }
}
