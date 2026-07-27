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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{ChildKiller, PtySize, native_pty_system};
use sprag_vt::{
    ClipboardQuery, ClipboardWrite, Emulator, HistoryLimits, InputModes, MouseProtocol,
    Notification, Palette, Screen, ShellState, VtPort,
};

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

/// How a pane's child ENDED — the status `waitpid` yielded once the process was reaped.
///
/// A DIFFERENT fact from [`PanePty::is_eof`], not a refinement of the same one, which is why the two
/// are addressed separately everywhere they travel. EOF says the child's output stream closed;
/// this says the process terminated and with what. They normally coincide, but the kernel closes a
/// dying task's file descriptors before it becomes reapable, so there is always a window where EOF
/// holds and this is not yet known — and a child that hands its pty to a grandchild and exits, or
/// one whose grandchild outlives it, breaks the coincidence outright. So `is_eof` is the liveness
/// bit and this is the ADDITIONAL, later fact. The one invariant that does hold, and that the
/// reader thread's publication order enforces, is the implication: a known exit implies EOF.
///
/// Sprag's own type rather than `portable_pty::ExitStatus` re-exported, because it crosses the host
/// wire — the PTY backend is an implementation detail of this seam ([`CommandBuilder`] is
/// re-exported for the opposite reason: callers must build one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneExit {
    /// The process's exit code (`0` for a clean exit). When [`signal`](Self::signal) is set this is
    /// the platform's stand-in (`1`) rather than something the process chose, so a reader deciding
    /// what to SHOW should consult the signal first.
    pub code: u32,
    /// The signal that killed the child, named as the platform spells it (`Terminated`,
    /// `Killed`), or `None` for a process that returned normally. This is the difference between
    /// "your build failed" and "the OOM killer took it", which no exit code can express.
    pub signal: Option<String>,
}

/// The child's exit status, shared between the reader thread that reaps it and every reader of the
/// [`PanePty`] that outlives its child. `None` until the reap publishes — see [`PaneExit`] for why
/// that is a real state and not a transient to be papered over.
type SharedExit = Arc<Mutex<Option<PaneExit>>>;

/// A live terminal: a child process on a PTY, its output parsed into a
/// queryable [`Screen`] that the reader thread keeps current.
pub struct PanePty {
    // The PTY master is OWNED BY the resize coalescer thread (`resize` is its
    // sole user), so resizes apply off the caller's thread and are debounced —
    // see [`run_resize_coalescer`]. The pty hands target sizes to it via
    // `resize_tx`.
    // The CHILD is owned by the reader thread, which reaps it the moment its output ends — the only
    // place that can block on `wait()` without stalling anything. What stays here is the ability to
    // SIGNAL it (`clone_killer` exists for exactly this split) and the two facts a reaped child can
    // no longer be asked for: its pid and its status.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// The child's OS pid, captured at spawn because the reader thread owns the handle that could
    /// report it. Read out through [`pid`](PanePty::pid), which gates it on the child NOT having
    /// been reaped — a reaped pid may be recycled onto an unrelated process, and the `/proc` walks
    /// that consume it must never stray there.
    pid: Option<u32>,
    exit: SharedExit,
    writer: SharedWriter,
    emulator: Arc<Mutex<Emulator>>,
    raw_output: SharedRawCapture,
    eof: Arc<AtomicBool>,
    /// High-water mark of the OSC 52 clipboard READ query this pane has ANSWERED, shared with
    /// every [`PanePtyHandle`]. When several display clients race to answer the same query (each
    /// has its own system clipboard), a CAS on this admits EXACTLY ONE reply to the PTY — the
    /// child must not receive N conflicting `OSC 52` responses. See [`answer_clipboard_query`].
    clipboard_answered: Arc<AtomicU64>,
    reader_thread: Option<JoinHandle<()>>,
    // The full winsize a resize applies: `(cols, rows, pixel_width, pixel_height)`. The pixel
    // extents are derived host-side (`cols * cell_w`, `rows * cell_h`) from the display's cell
    // metric so a child reads a real `TIOCGWINSZ` `ws_xpixel` / `ws_ypixel` (0 while unknown).
    resize_tx: Option<Sender<(u16, u16, u16, u16)>>,
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
        Self::spawn_with_dirty(command, cols, rows, None, None, &[])
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
    /// `history` is a restored pane's recorded scrollback as replayable terminal bytes (empty for
    /// an ordinary spawn). It is applied to the emulator BEFORE the reader thread exists, which is
    /// the only race-free point: the child is already running by the time this returns, so seeding
    /// afterwards would interleave with its first output and could land the restored history
    /// UNDER a prompt the shell had already printed. It is applied to the emulator only, never
    /// written to the PTY — the child must not receive its predecessor's output — and it is
    /// deliberately kept out of [`raw_output`](Self::raw_output), which captures what THIS child
    /// said.
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
        history: &[u8],
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
        // Split the handle before the child moves to the reader thread: the killer signals it, the
        // pid answers `/proc` questions. `clone_killer`'s own contract is this exact split ("send it
        // signals independently from a thread that may be blocked in `.wait`").
        let killer = child.clone_killer();
        let pid = child.process_id();
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
        // Replay a restored pane's recorded scrollback into the fresh emulator while this thread is
        // still its only observer — before the reader below can apply a single byte from the child.
        if !history.is_empty() {
            lock(&emulator).advance(history);
        }
        let raw_output: SharedRawCapture = Arc::new(Mutex::new(RawCapture::new()));
        let eof = Arc::new(AtomicBool::new(false));
        let exit: SharedExit = Arc::new(Mutex::new(None));
        let reader_emulator = Arc::clone(&emulator);
        let reader_raw = Arc::clone(&raw_output);
        let reader_eof = Arc::clone(&eof);
        let reader_exit = Arc::clone(&exit);
        // The reader writes device RESPONSES (e.g. the Kitty `CSI ? u` flags reply) back to the
        // child; it shares the SAME writer the input path uses, so the two serialize on its mutex.
        let reader_writer = Arc::clone(&writer);
        let reader_thread = std::thread::Builder::new()
            .name("sprag-pty-reader".to_string())
            .spawn(move || {
                let mut child = child;
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
                            // Apply the batch, then drain any device response it produced UNDER the
                            // same lock (so a response is consistent with the state that made it),
                            // and write it back OUTSIDE the emulator lock (the writer has its own).
                            let (responses, present) = {
                                let mut emu = lock(&reader_emulator);
                                emu.advance(&buf[..n]);
                                // Synchronized output (DEC 2026): while the child holds an
                                // atomic-frame update open, DEFER the repaint so this batch's
                                // screen changes present as one tear-free frame. Device responses
                                // still flow (a query mid-update is answered at once); only the
                                // on-screen present waits. When the update closes in this same
                                // batch, `synchronized_output()` reads false and we wake normally —
                                // the consumer then re-reads the already-complete Screen once.
                                (emu.take_responses(), !emu.synchronized_output())
                            };
                            if !responses.is_empty() {
                                let _ = write_shared(&reader_writer, &responses);
                            }
                            // R999 seam: wake the windowed host to repaint now that this batch is
                            // applied (no-op headless) — UNLESS a synchronized-output update is
                            // still open, in which case the wake waits for the batch that closes it
                            // (the EOF path below still wakes unconditionally, flushing a held frame
                            // if the child dies mid-update).
                            if present && let Some(ref notify) = on_dirty {
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
                // Reap LAST, and only here. Two properties depend on the order:
                //
                // * `wait()` BLOCKS, and EOF does not prove the child has terminated (a grandchild
                //   holding the slave keeps it running, and the kernel closes a dying task's fds
                //   before it becomes reapable). Everything above must therefore already have run:
                //   a pane whose output ended reads as finished at once, whatever the process does
                //   afterwards. Nothing waits on this.
                // * This thread is the ONLY reaper, so the pid cannot be recycled while another
                //   thread still believes it owns it. `Drop` signals through the killer and joins
                //   here rather than waiting itself.
                if let Ok(status) = child.wait() {
                    *lock(&reader_exit) = Some(PaneExit {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                    });
                    // A second wake, because the status arrived after the one above: the title that
                    // said "(exited)" can now say WHICH exit. For the overwhelmingly common clean
                    // exit the rendering is unchanged, so this repaints nothing visible; it is the
                    // failing command — the one the user needs to see — that gains its code here.
                    if let Some(ref notify) = on_dirty {
                        notify();
                    }
                }
            })
            .map_err(|e| PanePtyError::new("spawn reader thread", &e))?;

        // `TIOCSWINSZ` coalescer: own thread, owns the PTY master (its only
        // user). The caller's reflow (a continuous drag) resizes the emulator
        // synchronously and sends every intermediate size here; the coalescer
        // debounces the PTY ioctl to the final one — so the live shell gets one
        // `SIGWINCH` per settle, not one per cell-width boundary.
        let (resize_tx, resize_rx) = mpsc::channel::<(u16, u16, u16, u16)>();
        let master = pair.master;
        let resize_thread = std::thread::Builder::new()
            .name("sprag-pty-resize".to_string())
            .spawn(move || {
                run_resize_coalescer(
                    RESIZE_DEBOUNCE,
                    &resize_rx,
                    |(cols, rows, pixel_width, pixel_height)| {
                        let _ = master.resize(PtySize {
                            rows,
                            cols,
                            pixel_width,
                            pixel_height,
                        });
                    },
                );
            })
            .map_err(|e| PanePtyError::new("spawn resize thread", &e))?;

        Ok(Self {
            killer,
            pid,
            exit,
            writer,
            emulator,
            raw_output,
            eof,
            clipboard_answered: Arc::new(AtomicU64::new(0)),
            reader_thread: Some(reader_thread),
            resize_tx: Some(resize_tx),
            resize_thread: Some(resize_thread),
        })
    }

    /// Read the current authoritative screen under the emulator lock.
    pub fn with_screen<R>(&self, f: impl FnOnce(&Screen) -> R) -> R {
        f(lock(&self.emulator).screen())
    }

    /// Read the current screen AND the live colour [`Palette`] together under one emulator lock —
    /// the projection (sprag-grid's `project`) needs both (the palette resolves each cell's colour).
    /// A single lock keeps them consistent (a colour change and the cells it re-colours cannot tear).
    pub fn with_screen_palette<R>(&self, f: impl FnOnce(&Screen, &Palette) -> R) -> R {
        let emu = lock(&self.emulator);
        f(emu.screen(), emu.palette())
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

    /// The pane's shell-integration state (OSC 133) + the last finished command's exit status,
    /// DERIVED from the emulator's screen marks under the lock in ONE take (so the pair is
    /// consistent). `(ShellState::Unknown, None)` when the child has no shell integration.
    #[must_use]
    pub fn shell(&self) -> (ShellState, Option<i32>) {
        let emu = lock(&self.emulator);
        (emu.screen().shell_state(), emu.screen().last_exit_status())
    }

    /// Which pointer events the child has asked the terminal to report (the DECSET mouse-tracking
    /// mode), read LIVE from the emulator's input modes. A display client reads this to decide
    /// whether to capture the pointer for reporting instead of handling it itself (selection,
    /// wheel-scroll). See [`sprag_vt::MouseProtocol`].
    #[must_use]
    pub fn mouse_protocol(&self) -> MouseProtocol {
        lock(&self.emulator).input_modes().mouse_protocol
    }

    /// Whether the child has asked the terminal to report focus changes (DECSET 1004), read LIVE
    /// from the emulator's input modes. `false` by default; a full-screen app (vim checking for
    /// external edits, a TUI dimming when inactive) sets it. A display client reads this to decide
    /// whether to emit a focus-in / focus-out edge on a pane focus change. See
    /// [`sprag_vt::InputModes::focus_tracking`].
    #[must_use]
    pub fn focus_tracking(&self) -> bool {
        lock(&self.emulator).input_modes().focus_tracking
    }

    /// The most recent OSC 52 clipboard WRITE the child requested, or `None`, paired with its
    /// monotonic sequence — read out from under the emulator lock in ONE take so the payload and
    /// its sequence stay consistent (a consumer detects a new write via the sequence growing).
    /// The payload can be large (a whole paste), so a consumer fetches it on demand off the
    /// sequence rather than shipping it every poll. See [`VtPort::clipboard_write`].
    #[must_use]
    pub fn clipboard_write(&self) -> (Option<ClipboardWrite>, u64) {
        let emu = lock(&self.emulator);
        (emu.clipboard_write().cloned(), emu.clipboard_write_seq())
    }

    /// Just the monotonic count of OSC 52 clipboard WRITES the child has requested — the CHEAP
    /// detection read (no payload clone) a consumer polls every frame to learn when to fetch the
    /// (potentially large) write via [`Self::clipboard_write`]. See [`VtPort::clipboard_write_seq`].
    #[must_use]
    pub fn clipboard_write_seq(&self) -> u64 {
        lock(&self.emulator).clipboard_write_seq()
    }

    /// The most recent OSC 52 clipboard READ query the child requested, or `None`, paired with
    /// its monotonic sequence (one lock take). Tiny (a single selection), so — unlike the write —
    /// it travels inline. See [`VtPort::clipboard_query`].
    #[must_use]
    pub fn clipboard_query(&self) -> (Option<ClipboardQuery>, u64) {
        let emu = lock(&self.emulator);
        (emu.clipboard_query(), emu.clipboard_query_seq())
    }

    /// Admit ONE display client's answer to the OSC 52 read query `seq`, writing `reply` (the
    /// framed `OSC 52` response bytes, from [`sprag_vt::osc52_reply`]) to the PTY only if no
    /// client has already answered this query or a newer one. Returns whether this call wrote.
    /// The CAS on the shared answered-query high-water mark makes the reply exactly-once across all
    /// attached clients.
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn answer_clipboard_query(&self, seq: u64, reply: &[u8]) -> io::Result<bool> {
        answer_query(&self.clipboard_answered, &self.writer, seq, reply)
    }

    /// An owned snapshot of the current screen.
    #[must_use]
    pub fn screen(&self) -> Screen {
        self.with_screen(Screen::clone)
    }

    /// This pane's retained output encoded as REPLAYABLE terminal bytes, bounded to its last
    /// `limit` logical lines — the durable form of its scrollback.
    ///
    /// The companion of [`cwd`](Self::cwd) for the durability ring: a reboot kills the PTY, and
    /// re-spawning a shell in the recorded directory puts the pane back where the user was working,
    /// but blank. These bytes are what puts the user's OUTPUT back with it. Read at save time
    /// (history grows with every scroll), so like the cwd it is live child state rather than
    /// something stored on the pane.
    ///
    /// Not routed through [`with_screen`](Self::with_screen): while a fullscreen app holds the
    /// alternate screen the ACTIVE screen is that app's furniture and carries no scrollback, so the
    /// encoding is taken from the emulator, which knows to reach past it to the main screen.
    #[must_use]
    pub fn history_bytes(&self, limits: HistoryLimits) -> Vec<u8> {
        lock(&self.emulator).history_bytes(limits)
    }

    /// The epoch of everything [`Self::history_bytes`] would encode — cheap enough to poll on a timer,
    /// which is exactly what it is for: the save loop compares it against the reading it kept and
    /// encodes only when it moved. Holds the emulator lock for a field read rather than for a walk of
    /// the whole scrollback, so it does not contend with the reader thread the way an encode does.
    #[must_use]
    pub fn history_epoch(&self) -> u64 {
        lock(&self.emulator).history_epoch()
    }

    /// The OS process id of the child on this pty's slave — `None` once it has been reaped, or for
    /// a backend that cannot report one.
    ///
    /// Gated on the reap, and that gate is a SAFETY property rather than tidiness. Every consumer
    /// of this pid inspects `/proc` with it (the durability ring's cwd, the session rail's listening
    /// ports), and a pid that has been waited for is free to be recycled onto an unrelated process —
    /// at which point those walks would read a stranger's working directory and sockets. Because the
    /// reader thread is the only reaper and publishes [`exit_status`](Self::exit_status) as it
    /// reaps, "the status is known" is exactly "the pid may now be stale", so that is the gate. See
    /// the note on the registry's `window_pids`, which asked for this the moment an in-place reap
    /// existed.
    ///
    /// A `Some` still is not proof the child is ALIVE — a zombie has a pid and answers `/proc`
    /// harmlessly — so a caller wanting liveness consults [`is_eof`](Self::is_eof).
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        if lock(&self.exit).is_some() {
            return None;
        }
        self.pid
    }

    /// How the child ENDED, or `None` while it is still running — or has stopped producing output
    /// but not yet terminated. See [`PaneExit`] for why that last state is real rather than a race
    /// to be smoothed over.
    ///
    /// Published by the reader thread as it reaps, AFTER [`is_eof`](Self::is_eof), so `Some` here
    /// implies EOF and a caller may treat the pair as a refinement in that one direction only.
    #[must_use]
    pub fn exit_status(&self) -> Option<PaneExit> {
        lock(&self.exit).clone()
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
        read_cwd(self.pid()?)
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
        write_input(&self.emulator, &self.writer, bytes)
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
            clipboard_answered: Arc::clone(&self.clipboard_answered),
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
    /// `cell_px` is the display's `(cell_width, cell_height)` in logical pixels — the GUI's font
    /// metric, the only side that knows a cell's pixel extent. `(0, 0)` means "unknown" (a headless
    /// or metric-less client) and leaves the emulator's last-known cell geometry untouched, so a
    /// plain resize never clobbers it. The PTY winsize `ws_xpixel` / `ws_ypixel` are derived
    /// (`cols * cell_w`, `rows * cell_h`) from the resulting cell geometry so a child sizes sixel /
    /// Kitty images correctly; they stay `0` until a metric has arrived.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the master resize fails.
    pub fn resize(&self, cols: u16, rows: u16, cell_px: (u16, u16)) -> Result<(), PanePtyError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // Resize the emulator screen SYNCHRONOUSLY so the grid reflows the SAME
        // frame the GUI lays it out (the same-frame reflow contract the host's
        // dirty re-pass relies on). Only the PTY ioctl (`TIOCSWINSZ` →
        // `SIGWINCH`) is debounced: a continuous drag would otherwise flood the
        // live shell with `SIGWINCH`es and it redraws its prompt for every one,
        // which the emulator accumulates (the reported bug). The coalescer
        // applies one ioctl per settle.
        let (pixel_width, pixel_height) = {
            let mut emulator = lock(&self.emulator);
            emulator.resize(cols, rows);
            // Update the display cell geometry only when the caller measured it (0 = unknown), so a
            // metric-less resize never zeroes a known cell size. Then derive the winsize pixels from
            // the resulting (possibly just-updated) cell geometry — the emulator is the SSOT.
            if cell_px != (0, 0) {
                emulator.set_cell_pixel_size(cell_px.0, cell_px.1);
            }
            let (cw, ch) = emulator.cell_pixel_size();
            (cols.saturating_mul(cw), rows.saturating_mul(ch))
        };
        if let Some(tx) = &self.resize_tx {
            // A send only fails once the pty is dropping (coalescer gone),
            // where a stale PTY size is moot.
            let _ = tx.send((cols, rows, pixel_width, pixel_height));
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

    /// The display's `(cell_width, cell_height)` in logical pixels last carried by a
    /// [`resize`](Self::resize), or `(0, 0)` while unknown — the peer of [`dimensions`](Self::dimensions)
    /// for the pixel axis. The PTY winsize `ws_xpixel` / `ws_ypixel` are `cols * cell_w`,
    /// `rows * cell_h`.
    #[must_use]
    pub fn cell_pixel_size(&self) -> (u16, u16) {
        lock(&self.emulator).cell_pixel_size()
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
    /// Shared OSC 52 answered-query high-water mark — see [`PanePty::clipboard_answered`]. The
    /// host answers a read query through this handle, so the exactly-once arbitration lives here.
    clipboard_answered: Arc<AtomicU64>,
}

impl PanePtyHandle {
    /// Read the current authoritative screen under the emulator lock.
    pub fn with_screen<R>(&self, f: impl FnOnce(&Screen) -> R) -> R {
        f(lock(&self.emulator).screen())
    }

    /// Read the current screen AND the live colour [`Palette`] together under one emulator lock —
    /// the projection needs both. See [`PanePty::with_screen_palette`].
    pub fn with_screen_palette<R>(&self, f: impl FnOnce(&Screen, &Palette) -> R) -> R {
        let emu = lock(&self.emulator);
        f(emu.screen(), emu.palette())
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

    /// The most recent OSC 52 clipboard WRITE the child requested, or `None`, with its monotonic
    /// sequence (one lock take) — the on-demand payload the host serves through this handle when a
    /// client's write seq grows. See [`PanePty::clipboard_write`].
    #[must_use]
    pub fn clipboard_write(&self) -> (Option<ClipboardWrite>, u64) {
        let emu = lock(&self.emulator);
        (emu.clipboard_write().cloned(), emu.clipboard_write_seq())
    }

    /// Write already-encoded input bytes to the child (R2.6: the caller
    /// owns key→byte encoding).
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        write_input(&self.emulator, &self.writer, bytes)
    }

    /// Admit ONE client's answer to OSC 52 read query `seq`, writing `reply` to the PTY only if
    /// no client answered this query or a newer one — the exactly-once arbitration the host runs
    /// when a display client offers its clipboard. See [`PanePty::answer_clipboard_query`].
    ///
    /// # Errors
    ///
    /// Returns an IO error if the write to the master fails.
    pub fn answer_clipboard_query(&self, seq: u64, reply: &[u8]) -> io::Result<bool> {
        answer_query(&self.clipboard_answered, &self.writer, seq, reply)
    }
}

/// The shared OSC 52 read-query arbiter: a CAS on `answered` admits `seq` (and its `reply`) only
/// if it is NEWER than every query already answered on this pane, so exactly one of the racing
/// display clients writes a reply to the PTY. `fetch_max` is a single atomic RMW, so of two
/// clients offering the same `seq`, one observes a smaller prior value (writes) and the other
/// observes `seq` already in place (drops) — no lost or duplicated reply.
fn answer_query(
    answered: &AtomicU64,
    writer: &SharedWriter,
    seq: u64,
    reply: &[u8],
) -> io::Result<bool> {
    let prev = answered.fetch_max(seq, Ordering::AcqRel);
    if seq > prev {
        write_shared(writer, reply)?;
        Ok(true)
    } else {
        Ok(false)
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

/// Write CONSUMER input to the child (a key, a paste, an injected key). First tell the emulator
/// the user acted ([`VtPort::note_input`]) so the resize-redraw reinterpretation epoch closes
/// BEFORE the child sees the bytes and responds — the child's echo / redraw / command output is
/// then emulated with the epoch already closed (no reader-vs-writer race). The emulator lock is
/// taken only for that flag flip and dropped before the PTY write, never held across it (the same
/// discipline the reader uses for its response write-back). Automated child replies (device /
/// clipboard answers) call [`write_shared`] directly and so do NOT end the epoch.
fn write_input(
    emulator: &Arc<Mutex<Emulator>>,
    writer: &SharedWriter,
    bytes: &[u8],
) -> io::Result<()> {
    lock(emulator).note_input();
    write_shared(writer, bytes)
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
fn run_resize_coalescer<T: Copy>(quiet: Duration, rx: &Receiver<T>, mut apply: impl FnMut(T)) {
    let mut pending: Option<T> = None;
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
        // Stop the child so its slave fd closes, which unblocks the reader thread's `read()` with
        // EOF; then JOIN that thread, which reaps as its last act. Signalling rather than waiting
        // here is what keeps a single reaper: two threads calling `wait()` on one child race for the
        // status, and the loser would leave `exit_status` empty for a pane that plainly had one.
        let _ = self.killer.kill();
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
                .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                .collect::<String>()
        });
        assert!(row0.starts_with("hi"), "row0 = {row0:?}");
        assert_eq!(pty.dimensions(), (20, 4));
    }

    /// Synchronized output (DEC 2026) end-to-end: a child wraps its writes in an atomic-frame
    /// update (`CSI ? 2026 h … CSI ? 2026 l`). The reader DEFERS the repaint wake while the update
    /// is open and fires it once the update closes, so the content must both (a) reach the screen
    /// and (b) trigger a present — i.e. the held frame is RELEASED, never stuck. We wait on the
    /// content CONDITION (not a timing sleep) to stay deterministic; a gate that suppressed the
    /// wake forever (or swallowed the close) would leave row0 blank and fail here.
    #[test]
    fn synchronized_output_defers_then_presents_the_frame() {
        let presents = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&presents);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Open the update, write the frame, close it, then exit.
        command.arg("printf '\\033[?2026hSYNCED\\033[?2026l'");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn_with_dirty(
            command,
            20,
            4,
            Some(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            None,
            &[],
        )
        .expect("spawn a pty");

        let read_row0 = || {
            pty.with_screen(|screen| {
                (0..screen.cols())
                    .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                    .collect::<String>()
            })
        };
        // Wait on the CONDITION the released frame produces, not on EOF timing.
        let start = Instant::now();
        while !read_row0().starts_with("SYNCED") && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        assert!(
            read_row0().starts_with("SYNCED"),
            "the frame written inside the 2026 update presented; row0 = {:?}",
            read_row0(),
        );
        assert!(
            presents.load(Ordering::SeqCst) >= 1,
            "closing the update released a repaint wake (the frame was not left stuck)",
        );
    }

    /// End-to-end proof that a child's device query is ANSWERED BACK onto its input — the intrinsic
    /// `take_responses` reverse channel drained in the reader loop (not the clipboard path). The
    /// child asks DSR status (`CSI 5 n`); the terminal must write `CSI 0 n` to the child's stdin.
    /// To OBSERVE the reply the child reads it and hex-dumps it to the screen — deterministically:
    /// `stty -echo -icanon min 4` puts the pty in raw mode with `VMIN=4`, so `head -c 4` returns the
    /// exact 4-byte reply (a newline-less terminal reply would otherwise block a cooked read). No
    /// timing sleep, no fixed byte race. `1b 5b 30 6e` = ESC `[` `0` `n`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_device_query_is_answered_back_onto_the_childs_input() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(
            "stty -echo -icanon min 4 2>/dev/null; printf '\\033[5n'; head -c 4 | od -An -tx1",
        );
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 40, 4).expect("spawn a pty");

        let start = Instant::now();
        while !pty.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let row0 = pty.with_screen(|screen| {
            (0..screen.cols())
                .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                .collect::<String>()
        });
        assert!(
            row0.contains("1b 5b 30 6e"),
            "the child read back CSI 0 n and dumped it; row0 = {row0:?}",
        );
    }

    /// End-to-end proof that an OSC colour QUERY is answered back onto the child's input — the same
    /// `take_responses` reverse channel as the DA / DSR reply, now carrying an `OSC 11 ; rgb:… ST`
    /// reply. The child asks `OSC 11 ; ?` (its background); the terminal must write back the current
    /// background (the xterm seed, black). `stty -icanon min 25` blocks the read until the exact
    /// 25-byte reply arrives (no timing race), then `od` hex-dumps it. `1b 5d 31 31 3b 72 67 62 3a`
    /// = ESC `]` `1` `1` `;` `r` `g` `b` `:` — the unmistakable OSC 11 reply opener.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_osc_color_query_is_answered_back_onto_the_childs_input() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(
            "stty -echo -icanon min 25 2>/dev/null; printf '\\033]11;?\\033\\\\'; head -c 25 | od -An -tx1",
        );
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 40, 4).expect("spawn a pty");

        let start = Instant::now();
        while !pty.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let dump = pty.with_screen(|screen| {
            (0..screen.rows())
                .flat_map(|row| {
                    (0..screen.cols()).filter_map(move |col| {
                        screen.cell(col, row).map(|cell| cell.cluster.to_string())
                    })
                })
                .collect::<String>()
        });
        assert!(
            dump.contains("1b 5d 31 31 3b 72 67 62 3a"),
            "the child read back the OSC 11 background reply and dumped it; dump = {dump:?}",
        );
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
        shared
            .resize(100, 30, (0, 0))
            .expect("resize through a shared ref");
        // The emulator resizes synchronously (only the PTY ioctl is debounced),
        // so `dimensions()` is current immediately.
        assert_eq!(pty.dimensions(), (100, 30), "dimensions track the emulator");
        // The floor at 1x1 holds (a zero dimension cannot reach the PTY).
        shared.resize(0, 0, (0, 0)).expect("resize floors at 1x1");
        assert_eq!(pty.dimensions(), (1, 1));
    }

    #[test]
    fn resize_carries_the_cell_pixel_geometry_and_a_metric_less_resize_preserves_it() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // long-lived: keeps the PTY open across the resizes
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        assert_eq!(pty.cell_pixel_size(), (0, 0), "no metric at spawn");
        // A resize carrying a 9x18 metric records it (feeds the winsize ws_xpixel/ypixel + XTWINOPS).
        pty.resize(80, 24, (9, 18)).expect("resize with a metric");
        assert_eq!(pty.cell_pixel_size(), (9, 18), "the metric is recorded");
        // A later metric-less resize (0,0 = unknown, e.g. a headless client) must NOT clobber it.
        pty.resize(100, 30, (0, 0)).expect("metric-less resize");
        assert_eq!(
            pty.cell_pixel_size(),
            (9, 18),
            "a metric-less resize preserves the last-known cell geometry"
        );
    }

    /// Wait (bounded) until the child has exited and all its bytes are applied.
    fn wait_eof(pty: &PanePty) {
        let start = Instant::now();
        while !pty.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert!(pty.is_eof(), "child did not reach EOF in time");
    }

    /// Wait (bounded) until the reader thread has REAPED, and answer the status it published.
    ///
    /// A distinct wait from [`wait_eof`], on the condition the assertion actually reads, because
    /// the two conditions are distinct: the reap happens strictly after EOF and can lag it (see
    /// [`PaneExit`]). Waiting on EOF and then reading the status would be a race dressed as a test.
    fn wait_exit(pty: &PanePty) -> PaneExit {
        let start = Instant::now();
        loop {
            if let Some(exit) = pty.exit_status() {
                return exit;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "child was not reaped in time",
            );
            sleep(Duration::from_millis(20));
        }
    }

    /// Spawn `/bin/sh -c script` on a small pty, the shape every exit-status test wants.
    fn sh(script: &str) -> PanePty {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        PanePty::spawn(command, 20, 4).expect("spawn a pty")
    }

    /// A child that FAILS reports its code, and that is the whole point of reaping: `dead` alone
    /// says a command finished, and only the status says whether it worked.
    ///
    /// REVERT-PROOF: delete the `child.wait()` publication at the end of the reader thread and the
    /// status stays `None` forever — the pane can then only ever say "(exited)", which is exactly
    /// the bound this closes.
    #[test]
    fn a_failing_childs_exit_code_reaches_the_pane() {
        let pty = sh("exit 3");
        assert_eq!(
            wait_exit(&pty),
            PaneExit {
                code: 3,
                signal: None
            },
        );
        assert!(
            pty.is_eof(),
            "and a known status always comes with EOF — the one implication that holds",
        );
    }

    /// A child KILLED by a signal names the signal, where an exit code could only say `1`. This is
    /// the difference between "your build failed" and "something killed it", which is precisely what
    /// a user staring at a stopped screen cannot otherwise tell.
    #[test]
    fn a_signalled_child_names_the_signal_rather_than_a_code() {
        // The shell signals ITSELF, so the child on the pty is the one that dies by signal (a
        // `kill` of some other pid would prove nothing about this pane's own reaping).
        let exit = wait_exit(&sh("kill -TERM $$"));
        assert!(
            exit.signal.is_some(),
            "a signalled child reports its signal: {exit:?}",
        );
    }

    /// A CLEAN exit is reported too — `code: 0`, not `None`. "Finished successfully" and "not yet
    /// reaped" are different facts and a caller must be able to tell them apart, even though the
    /// title deliberately renders both the same way.
    #[test]
    fn a_clean_exit_is_a_reported_status_not_an_absent_one() {
        assert_eq!(
            wait_exit(&sh("exit 0")),
            PaneExit {
                code: 0,
                signal: None
            },
        );
    }

    /// Once the child is REAPED its pid is withheld — the safety gate [`PanePty::pid`] documents.
    ///
    /// A waited-for pid is free to be recycled onto an unrelated process, and every consumer of this
    /// one walks `/proc` with it. Before the in-place reap existed nothing reaped a POOLED pane, so
    /// the hazard could not arise; it can now, and this is the guard. `cwd` rides the same gate, so
    /// it is asserted here rather than in a test of its own.
    ///
    /// REVERT-PROOF: return `self.pid` unconditionally and both asserts fail — the pane hands out a
    /// recyclable pid.
    #[test]
    fn a_reaped_childs_pid_is_withheld_so_no_proc_walk_can_stray() {
        let pty = sh("exit 0");
        assert!(pty.pid().is_some(), "a live child has a usable pid");
        wait_exit(&pty);
        assert_eq!(pty.pid(), None, "a reaped one does not");
        assert_eq!(pty.cwd(), None, "and neither does anything derived from it");
    }

    /// Wait (bounded) until the raw capture has latched `truncated` — the condition a truncation
    /// test actually asserts. This latches as soon as the reader has drained past the cap, which is
    /// bounded by draining ~256 KiB; it does NOT wait for the child to exit / EOF (which additionally
    /// waits on the whole pipeline collapsing via `SIGPIPE` and the shell reaping — that lag
    /// occasionally exceeds a fixed bound, and it is not what the assertions check).
    fn wait_truncated(pty: &PanePty) {
        let start = Instant::now();
        while !lock(&pty.raw_output).truncated && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(5));
        }
        assert!(
            lock(&pty.raw_output).truncated,
            "capture did not reach the cap in time",
        );
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
            &[],
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
            &[],
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
    /// The device-response path end-to-end: a child enables the Kitty disambiguate flag and
    /// queries it; the emulator replies `CSI ? 1 u`, which the READER THREAD writes back to the
    /// child's PTY. A raw (`stty raw`) `cat` echoes that reply to its output, so the reply bytes
    /// appear in the raw capture — proving the response reached the child. The reply `ESC [ ? 1 u`
    /// is distinct from the query the child printed (`ESC [ ? u`, no `1`), so its presence is the
    /// round-trip, not the echo of the query.
    #[test]
    fn the_reader_writes_a_kitty_query_response_back_to_the_child() {
        fn contains(haystack: &[u8], needle: &[u8]) -> bool {
            haystack.windows(needle.len()).any(|w| w == needle)
        }
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[>1u\\033[?u'; cat");
        command.env("TERM", "xterm");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");

        let start = Instant::now();
        let mut seen = false;
        while start.elapsed() < Duration::from_secs(5) {
            if contains(&pty.raw_output().bytes, b"\x1b[?1u") {
                seen = true;
                break;
            }
            sleep(Duration::from_millis(20));
        }
        assert!(
            seen,
            "the CSI ? 1 u reply never reached the child (the reader did not write the response)",
        );
    }

    #[test]
    fn raw_output_truncates_past_the_cap() {
        // Emit one more KiB than the cap. `head -c … /dev/zero` (NUL bytes) is DELIBERATE: this
        // exercises the reader -> capture truncation path, and the capture stores RAW bytes before
        // the emulator sees them, so the byte value is incidental. NUL parses to a no-op in the
        // emulator, which keeps the drain fast and the cap latch deterministic — an over-cap stream
        // of PRINTABLE text is dominated by the per-cell print cost (a separately tracked emulator
        // throughput debt), which raced this test's fixed timeout. What is under test is only that
        // the capture bounds at the cap and keeps the HEAD bytes.
        let want = RAW_CAPTURE_CAP + 1024;
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("head -c {want} /dev/zero"));
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 80, 24).expect("spawn a pty");
        // Wait for the CONDITION under test (the cap latched), not for the child to exit — the cap
        // latches during draining, deterministically, whereas EOF additionally waits on the pipeline
        // collapsing, which occasionally loses a fixed-timeout race.
        wait_truncated(&pty);

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
        assert!(bytes.iter().all(|&b| b == 0), "the head bytes are kept");
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
