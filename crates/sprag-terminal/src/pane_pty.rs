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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::pty::Pty;
use sprag_vt::{
    ClipboardQuery, ClipboardWrite, Emulator, HistoryLimits, InputModes, LinesSince, MouseProtocol,
    Notification, Palette, Screen, ShellState, VtPort,
};

// Re-exported so callers build commands without depending on portable-pty
// directly (it is an implementation detail of the PTY seam).
pub use crate::command::CommandBuilder;

/// The PTY master writer, shared (and interior-mutable) so both the owning
/// [`PanePty`] and any [`PanePtyHandle`] can inject input without a
/// `&mut` borrow — the pty is already concurrent (the reader thread
/// holds the emulator lock), so the writer is shared the same way.
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// The bytes most recently WRITTEN INTO a pane, shared so every writer records into one trail.
///
/// See [`ECHO_TRAIL_CAP`] for why a pane keeps this at all.
type SharedEchoTrail = Arc<Mutex<Vec<u8>>>;

/// A pane's ask-only handle on its own device, shared with every [`PanePtyHandle`].
///
/// No `Mutex`: every operation on it is an `ioctl` that reads, so there is nothing to serialise —
/// which is the point of the type being ask-only rather than a third writer.
type SharedQuery = Arc<crate::pty::TerminalQuery>;

/// How much recently-written input a pane remembers, for telling its own ECHO apart from what the
/// program in it actually said.
///
/// Public because the two `echo_trail` readers are, and a bound a caller cannot see is a bound
/// they cannot reason about: a command line longer than this is remembered only in part.
///
/// # ⚠⚠ Why a pane has to remember what was typed at it
///
/// **A pseudoterminal echoes what is written to it, and that echo is indistinguishable from
/// program output once it reaches the grid.** Everything downstream that has to know *did the
/// program say this, or is this my own input coming back?* has so far answered it by comparing
/// against what THAT CALLER had just written — which works only for a caller that did the writing
/// (`Orchestrator::reaction`, `Pipe::shown`). A run that must wait for a program somebody ELSE
/// started has nothing to compare against, and the answer it reached depended on whether the echo
/// happened to land before or after it started looking. **A predicate whose answer depends on
/// scheduling is not a predicate.**
///
/// So the pane keeps the trail, every writer feeds it, and the question becomes answerable by
/// anyone. Bounded because it is unbounded input otherwise; generous against the thing it exists to
/// recognise, which is a command line somebody typed or pasted.
pub const ECHO_TRAIL_CAP: usize = 8 * 1024;

/// Upper bound on the raw-output capture buffer ([`RawCapture`]). Generous
/// enough for a large structured envelope (a `claude -p --output-format json`
/// reply with a code-block `result` is a few KiB to tens of KiB), bounded so a
/// runaway child cannot grow it without limit. A child that exceeds it marks
/// the capture `truncated`, which a structured reader treats as an unparseable
/// reply and degrades gracefully.
const RAW_CAPTURE_CAP: usize = 256 * 1024;

/// The three things a pane's READER THREAD can tell its host: something changed, the child is gone,
/// the child is asking for a person.
///
/// # Why a type and not three parameters
///
/// Because they are one decision made once, at a birth, by whoever owns the pane's display and
/// lifetime — and because they arrived one at a time. `on_dirty` came with the repaint seam,
/// `on_exit` with the self-cleaning daemon, `on_attention` with the routed notification; the third
/// one pushed [`PanePty::spawn_with_dirty`] to eight parameters, which is where a positional list
/// stops being readable and a caller starts passing `None, None, None` and hoping. Named fields make
/// a site that wires two of three say which two.
///
/// [`Default`] is every hook absent — a pane whose output nobody is waiting for, which is what this
/// crate's own tests and [`PanePty::spawn`] want.
#[derive(Default)]
pub struct PaneHooks {
    /// Invoked after each parsed PTY batch is applied to the screen, **and once more when the child
    /// exits** — after [`is_eof`](PanePty::is_eof) publishes, so a wake can never observe the pane it
    /// announces as still live. This is the sprag side of the pinion R999 `RepaintSink` seam: a
    /// windowed host passes `Some(Box::new(move || sink.request_repaint()))`, the headless host
    /// passes `bump_on_dirty`, which moves the scene revision so a parked `scene/waitFor` wakes.
    pub on_dirty: Option<Box<dyn Fn() + Send>>,
    /// Invoked EXACTLY ONCE, when the child has exited (the reader loop reached EOF), after
    /// `on_dirty`'s exit wake. This is the "this child is gone" event as distinct from "this child
    /// produced output", so a caller can act on a pane's death without a per-output-batch check: the
    /// daemon reads it to end its own process when the last live pane dies. Fired after `is_eof`
    /// publishes, so a liveness scan run from it never counts the pane it announces.
    pub on_exit: Option<Box<dyn Fn() + Send>>,
    /// The pane's own `cgroup.procs`, ALREADY OPEN, for the child to write itself into between
    /// `fork` and `exec` (R336).
    ///
    /// Open rather than a path, because the child's half must not allocate: it writes one byte to a
    /// descriptor it already has. `None` on a host with no cgroup tree — the GUI's in-process host,
    /// every test, and any machine that cannot enforce a share — and such a pane opens exactly as
    /// it always did.
    #[cfg(unix)]
    pub home: Option<std::os::fd::OwnedFd>,
    /// Invoked when a batch RAISED an [`Attention`]: a notification OSC or a bell. Distinct from
    /// [`Self::on_dirty`] because it is a different question — that one says *something changed,
    /// repaint*, and this says *the child is asking for a person* — and because a notification
    /// stamps no cells, so the two are not even the same event about the same bytes.
    ///
    /// **It runs ON THE READER THREAD, so it must not take a lock this daemon holds across a pane
    /// drop.** `PanePty::Drop` JOINS this thread, so a hook that waited on a workspace lock would
    /// deadlock the moment a pane-drop site held one — the hazard the daemon's reaper seam already
    /// documents and solves the same way: the hook SENDS on a channel and a dedicated thread does
    /// the work that needs the registry.
    pub on_attention: Option<Box<dyn Fn(Attention) + Send>>,
}

/// A pane's child asking for a PERSON — the fact the reader thread reports through
/// [`PanePty::spawn_with_dirty`]'s `on_attention` hook the moment the batch carrying it is applied.
///
/// # Why an EVENT and not another poll field
///
/// The emulator has latched both of these since R-PR67, and the pane list has published them since:
/// a display client polls `notification` / `bell_seq` and paints a dot. Measured at `3114923` by
/// running the shipped binaries, that is where the whole feature ended — a child raising
/// `OSC 9 build finished: 3 errors` in an unfocused pane left a live `sprag-tui` **byte-for-byte
/// unchanged**, and `sprag-gui` showed a dot with the WORDS dropped. A latch nobody is obliged to
/// read is a fact with no delivery; the hook is what turns the moment it happened into something the
/// daemon can route while it is still true.
///
/// # The two arms are the two sources, kept apart because they carry different things
///
/// A notification carries WORDS a person can act on; a bell carries only that something happened.
/// The emulator keeps their sequences separate for exactly that reason, and folding them here would
/// force every consumer to invent a sentence for the bell or drop the notification's text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Attention {
    /// A desktop-style notification OSC (`OSC 9` / `OSC 777;notify` / `OSC 99`), as latched.
    ///
    /// **What it does NOT claim**: that every notification the batch carried is here. The emulator
    /// latches ONE, so a batch that raised two reports the second — the same bound the pane list
    /// has always had, stated where a caller can read it rather than implied by a counter. A queue
    /// would need a bound and a bound needs a discard rule, which is the silent drop this seam
    /// exists to remove.
    Raised(Notification),
    /// A `BEL` — tmux's `monitor-bell` signal, with no words to carry.
    ///
    /// Reported once per BATCH rather than once per byte: a shell that rings three times while the
    /// reader is between wakeups has asked for a person once.
    Bell,
}

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
    // The child is SIGNALLED by pid, because the reader thread owns the handle and is blocked in
    // `wait` on it — `std`'s `kill` needs `&mut`. See `crate::pty::signal_child` for the pid-reuse
    // window this leaves and why it is the same one the previous backend had.
    /// The child's OS pid, captured at spawn because the reader thread owns the handle that could
    /// report it. Read out through [`pid`](PanePty::pid), which gates it on the child NOT having
    /// been reaped — a reaped pid may be recycled onto an unrelated process, and the `/proc` walks
    /// that consume it must never stray there.
    pid: Option<u32>,
    /// The pane's terminal DEVICE (`/dev/pts/7`), captured at spawn — see [`tty`](PanePty::tty).
    tty: Option<PathBuf>,
    exit: SharedExit,
    writer: SharedWriter,
    /// A handle that only ASKS this pane's device questions — see
    /// [`TerminalQuery`](crate::pty::TerminalQuery).
    ///
    /// Shared with every [`PanePtyHandle`] because the question it answers is asked from the
    /// plugin surface, which is handle-shaped, and because a descriptor per handle would be a
    /// descriptor per reader of a fact that does not change per reader.
    query: SharedQuery,
    /// What has recently been written INTO this pane — see [`ECHO_TRAIL_CAP`].
    echo_trail: SharedEchoTrail,
    emulator: Arc<Mutex<Emulator>>,
    raw_output: SharedRawCapture,
    eof: Arc<AtomicBool>,
    /// High-water mark of the OSC 52 clipboard READ query this pane has ANSWERED, shared with
    /// every [`PanePtyHandle`]. When several display clients race to answer the same query (each
    /// has its own system clipboard), a CAS on this admits EXACTLY ONE reply to the PTY — the
    /// child must not receive N conflicting `OSC 52` responses. See
    /// [`answer_clipboard_query`](PanePty::answer_clipboard_query).
    clipboard_answered: Arc<AtomicU64>,
    reader_thread: Option<JoinHandle<()>>,
    /// Disconnects when the reader thread has run its whole tail — apply, EOF, `on_exit`, reap,
    /// publish. [`Drop`](PanePty::drop) waits on THIS to decide whether the hangup worked, because
    /// a join cannot be asked "are you done yet". See [`HANGUP_GRACE`].
    reader_done: Receiver<()>,
    // The full winsize a resize applies: `(cols, rows, pixel_width, pixel_height)`. The pixel
    // extents are derived host-side (`cols * cell_w`, `rows * cell_h`) from the display's cell
    // metric so a child reads a real `TIOCGWINSZ` `ws_xpixel` / `ws_ypixel` (0 while unknown).
    resize_tx: Option<Sender<(u16, u16, u16, u16)>>,
    resize_thread: Option<JoinHandle<()>>,
    /// Whether this pane's child reached the cgroup that was opened for it, answered by the birth
    /// itself — see [`Joined`](crate::pty::Joined).
    ///
    /// Kept here because the birth is the only moment it is knowable and the POOL is what needs it:
    /// a pane whose join was refused has no cgroup of its own, so it must not record one, or every
    /// later read and every later move would be aimed at a leaf its processes are not in. Captured
    /// rather than acted on here for this module's usual reason — this layer starts children, it
    /// does not decide what a pane's resources mean.
    #[cfg(unix)]
    joined: crate::pty::Joined,
}

impl PanePty {
    /// Spawn `command` on a fresh `cols × rows` pseudoterminal retaining the default depth of
    /// scrollback.
    ///
    /// The callback-free, history-free convenience. A DAEMON birthing a user's pane goes through
    /// [`Workspace::spawn`](crate::Workspace::spawn), which asks that pool's
    /// [`HistoryLimitSource`](crate::HistoryLimitSource) instead of taking this default.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the PTY cannot be opened, the child
    /// cannot be spawned, or the master reader/writer cannot be acquired.
    pub fn spawn(command: CommandBuilder, cols: u16, rows: u16) -> Result<Self, PanePtyError> {
        Self::spawn_with_dirty(
            command,
            cols,
            rows,
            PaneHooks::default(),
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
        )
    }

    /// [`Self::spawn`] with the pane's reader-thread callbacks — see [`PaneHooks`], which documents
    /// each one and the reason they travel together.
    ///
    /// All three are deliberately pinion-free (`Box<dyn Fn() + Send>`), so this crate stays decoupled
    /// from the GUI shell and the host lifetime. [`PaneHooks::default`] is for a caller with nothing
    /// to do (this crate's own tests, and [`spawn`](Self::spawn)).
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
        hooks: PaneHooks,
        history: &[u8],
        history_limit: usize,
    ) -> Result<Self, PanePtyError> {
        let PaneHooks {
            on_dirty,
            on_exit,
            on_attention,
            #[cfg(unix)]
            home,
        } = hooks;
        let cols = cols.max(1);
        let rows = rows.max(1);
        let pty = Pty::open(cols, rows).map_err(|e| PanePtyError::new("open pty", &e))?;
        // The pane's terminal DEVICE, taken here because this is the only moment it is reachable:
        // the master moves to the resize coalescer thread below and the slave is dropped before
        // that. It is `ttyname_r` on the slave fd, resolved by the PTY backend at `openpty` — so
        // this daemon does not have to DISCOVER what a caller could only guess at from the child's
        // fd 0 (which the child is free to redirect).
        let tty = pty.tty_name();
        let writer: SharedWriter = Arc::new(Mutex::new(Box::new(
            pty.writer()
                .map_err(|e| PanePtyError::new("take writer", &e))?,
        )));

        let emulator = Arc::new(Mutex::new(Emulator::with_history_limit(
            cols,
            rows,
            history_limit,
        )));
        // Replay a restored pane's recorded scrollback into the fresh emulator while this thread is
        // still its only observer — before the reader below can apply a single byte from the child.
        if !history.is_empty() {
            lock(&emulator).advance(history);
        }
        let raw_output: SharedRawCapture = Arc::new(Mutex::new(RawCapture::new()));
        let eof = Arc::new(AtomicBool::new(false));
        let exit: SharedExit = Arc::new(Mutex::new(None));
        // WHERE THE ATTENTION COUNTERS START, and it is load-bearing. A restored pane replays its
        // recorded scrollback into the emulator above, and that replay runs the same OSC handling
        // live output does — so a pane whose child once raised `OSC 9 build finished` comes back
        // with `notification_seq` already at 1. Reading the marks HERE, after the replay and before
        // the reader thread exists, is what makes a restore silent: the hook fires on what this
        // child says, never on what its predecessor said. Zero for an ordinary spawn.
        let mut attention_marks = {
            let emu = lock(&emulator);
            (emu.notification_seq(), emu.bell_seq())
        };
        let reader_emulator = Arc::clone(&emulator);
        let reader_raw = Arc::clone(&raw_output);
        let reader_eof = Arc::clone(&eof);
        let reader_exit = Arc::clone(&exit);
        // The reader writes device RESPONSES (e.g. the Kitty `CSI ? u` flags reply) back to the
        // child; it shares the SAME writer the input path uses, so the two serialize on its mutex.
        let reader_writer = Arc::clone(&writer);
        // The reader's "I am finished" edge. Moved IN and never sent on: the disconnect its drop
        // causes is the signal, so it fires however the closure ends — normal return or panic — and
        // cannot be forgotten on a future early-exit path.
        let (reader_done_tx, reader_done) = mpsc::channel::<()>();
        // The child arrives AFTER the thread that reaps it, because the reader is attached before
        // any child exists — see `Pty::attach_reader` for the platform difference that ordering is
        // there to remove. This channel is how the one handle reaches its one reaper: `wait` is
        // still called on this thread and nowhere else, so the pid cannot be recycled under another
        // thread that still believes it owns it.
        let (child_tx, child_rx) = mpsc::channel::<std::process::Child>();
        let mut attached = pty
            .attach_reader("sprag-pty-reader", move |mut reader| {
                let _reader_done_tx = reader_done_tx;
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
                            let (responses, present, attention) = {
                                let mut emu = lock(&reader_emulator);
                                emu.advance(&buf[..n]);
                                // Synchronized output (DEC 2026): while the child holds an
                                // atomic-frame update open, DEFER the repaint so this batch's
                                // screen changes present as one tear-free frame. Device responses
                                // still flow (a query mid-update is answered at once); only the
                                // on-screen present waits. When the update closes in this same
                                // batch, `synchronized_output()` reads false and we wake normally —
                                // the consumer then re-reads the already-complete Screen once.
                                //
                                // The attention is read under the SAME lock as the `advance` that
                                // may have raised it — two field reads — and acted on outside it,
                                // which is the discipline the response drain above already follows.
                                (
                                    emu.take_responses(),
                                    !emu.synchronized_output(),
                                    take_attention(&emu, &mut attention_marks),
                                )
                            };
                            if !responses.is_empty() {
                                let _ = write_shared(&reader_writer, &responses);
                            }
                            // ATTENTION, outside the emulator lock and BEFORE the repaint wake: a
                            // pane's child asking for a person is not a repaint, and the two are
                            // ordered so that a consumer woken by the bump below can already see
                            // whatever the attention hook published. Deliberately NOT gated on
                            // `present` — a child that raises a notification inside a synchronized
                            // update is still asking for a person, and the frame it is composing
                            // has nothing to do with that.
                            for raised in attention {
                                if let Some(ref tell) = on_attention {
                                    tell(raised);
                                }
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
                //
                // The handle is RECEIVED rather than owned from the start: this thread was reading
                // the terminal before the child was created, which is the whole point of the
                // ordering. A `recv` that fails means the spawn failed and there is nothing to reap.
                if let Ok(mut child) = child_rx.recv()
                    && let Ok(status) = child.wait()
                {
                    let (code, signal) = crate::pty::exit_facts(status);
                    *lock(&reader_exit) = Some(PaneExit { code, signal });
                    // A second wake, because the status arrived after the one above: the title that
                    // said "(exited)" can now say WHICH exit. For the overwhelmingly common clean
                    // exit the rendering is unchanged, so this repaints nothing visible; it is the
                    // failing command — the one the user needs to see — that gains its code here.
                    if let Some(ref notify) = on_dirty {
                        notify();
                    }
                }
            })
            .map_err(|e| PanePtyError::new("attach reader", &e))?;

        // The cgroup join's outcome comes back BESIDE the child, never instead of it: a kernel that
        // refuses the migration costs this pane its share and not its existence.
        let (child, joined) = attached
            .spawn(&command, home.as_ref().map(std::os::fd::AsFd::as_fd))
            .map_err(|e| PanePtyError::new("spawn command", &e))?;
        if let crate::pty::Joined::Refused(error) = &joined {
            // Once, at the moment it is known, and in the same words `PaneHomes::open` uses for the
            // failures it can see — this is the one it cannot, because it happens after the fork.
            tracing::warn!(%error, "pane opened without an enforced share");
        }
        // Split the handle before it goes to its reaper: the killer signals it, the pid answers
        // `/proc` questions. `clone_killer`'s own contract is this exact split ("send it signals
        // independently from a thread that may be blocked in `.wait`").
        let pid = child.id();
        // The send cannot fail while the reader lives, and if it has already died the child is
        // reaped by the drop of the handle instead — either way there is nothing for a caller here
        // to decide.
        let _ = child_tx.send(child);
        let (master, reader_thread) = attached.into_parts();
        // ⚠ TAKEN BEFORE THE DEVICE MOVES. The coalescer below owns the master from here on, so
        // this is the last moment a handle on it can be made — and a question about the device
        // must not have to be routed through a thread whose job is to sleep between resizes.
        let query = Arc::new(
            master
                .query()
                .map_err(|e| PanePtyError::new("query handle", &e))?,
        );

        // `TIOCSWINSZ` coalescer: own thread, owns the PTY master (its only
        // user). The caller's reflow (a continuous drag) resizes the emulator
        // synchronously and sends every intermediate size here; the coalescer
        // debounces the PTY ioctl to the final one — so the live shell gets one
        // `SIGWINCH` per settle, not one per cell-width boundary.
        let (resize_tx, resize_rx) = mpsc::channel::<(u16, u16, u16, u16)>();
        let resize_thread = std::thread::Builder::new()
            .name("sprag-pty-resize".to_string())
            .spawn(move || {
                run_resize_coalescer(
                    RESIZE_DEBOUNCE,
                    &resize_rx,
                    |(cols, rows, pixel_width, pixel_height)| {
                        let _ = master.resize(cols, rows, pixel_width, pixel_height);
                    },
                );
            })
            .map_err(|e| PanePtyError::new("spawn resize thread", &e))?;

        Ok(Self {
            pid: Some(pid),
            tty,
            exit,
            writer,
            query,
            echo_trail: Arc::new(Mutex::new(Vec::new())),
            emulator,
            raw_output,
            eof,
            clipboard_answered: Arc::new(AtomicU64::new(0)),
            reader_thread: Some(reader_thread),
            reader_done,
            resize_tx: Some(resize_tx),
            resize_thread: Some(resize_thread),
            #[cfg(unix)]
            joined,
        })
    }

    /// Whether this pane's child reached the cgroup opened for it at its birth.
    ///
    /// The pool asks this to decide what the pane's [`home`](crate::Workspace) is: a refused join
    /// means the processes are in the DAEMON's cgroup, so the pane has none of its own to be read,
    /// weighted or moved out of.
    #[cfg(unix)]
    #[must_use]
    pub fn joined(&self) -> &crate::pty::Joined {
        &self.joined
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

    /// How many logical lines of SCROLLBACK this pane retains — the `history-limit` it was born
    /// with.
    ///
    /// Introspection, and deliberately NOT the persistence budget: [`Self::history_bytes`] encodes
    /// the visible screen as well as the scrollback, so bounding a save by this number would cut the
    /// live screen out of a full pane's history and leave a pane set to keep no scrollback with
    /// nothing saved at all. The save path passes the operator's ceiling whole and lets the encoder
    /// saturate at whatever the screen actually holds.
    ///
    /// Read from the emulator rather than stored beside it, so it is the value the eviction path
    /// actually enforces. The alt screen carries the same limit, so this answers identically while a
    /// fullscreen app is up.
    #[must_use]
    pub fn history_limit(&self) -> usize {
        lock(&self.emulator).screen().history_limit()
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

    /// The TERMINAL DEVICE this pane is — `/dev/pts/7` — or `None` on a platform whose PTY backend
    /// does not name one.
    ///
    /// Fixed at the pane's BIRTH and never changing, which is what separates it from every other
    /// fact about what is running here: it survives the child exiting, and it is the daemon's OWN
    /// (the backend resolves it from the slave fd at `openpty`) rather than something inferred from
    /// the child afterwards. That matters because the obvious inference — read the child's
    /// `/proc/<pid>/fd/0` — is a guess: a process may redirect its own standard input and go on
    /// owning the terminal.
    ///
    /// It is the name a person and every OS tool outside sprag call this pane by: `ps -t pts/7`,
    /// `who`, `write`, a debugger's `--tty`. A pane list gives an id nothing outside this daemon
    /// knows; this is the address the rest of the machine agrees on.
    #[must_use]
    pub fn tty(&self) -> Option<&Path> {
        self.tty.as_deref()
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

    /// The process group that currently owns this pane's terminal, read LIVE from the OS.
    ///
    /// This is the pane's FOREGROUND JOB, which is what a shell hands the terminal to when the user
    /// runs something and takes back when that thing ends. It is the only fact available here that
    /// identifies a process the pane is running but did not spawn — [`pid`](Self::pid) names the
    /// child, and an agent typed at that child's prompt is one level further down.
    ///
    /// A caller can hold onto the answer as a claim about WHICH process is doing something in this
    /// pane, and later ask the OS whether that group still exists. That is the whole reason it is
    /// published: it outlives the read, where [`is_eof`](Self::is_eof) only ever answers about the
    /// child.
    ///
    /// `None` when the child has exited or been reaped (same guard and same reason as
    /// [`cwd`](Self::cwd) — a recycled pid must not be walked), when nothing owns the terminal, or
    /// on a platform with no `/proc`.
    /// It is [`foreground_pgid_of`] applied to this pane's [`pid`](Self::pid), and it is one
    /// function rather than two so that a caller holding only a pid — a sweep that has released its
    /// workspace lock before doing I/O — cannot end up with a second answer.
    #[must_use]
    pub fn foreground_pgid(&self) -> Option<u32> {
        foreground_pgid_of(self.pid()?)
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
        write_input(&self.emulator, &self.writer, &self.echo_trail, bytes)
    }

    /// What has recently been written INTO this pane — the trail that lets a reader tell the
    /// pane's own ECHO from what the program in it said. See [`ECHO_TRAIL_CAP`].
    ///
    /// Lossy UTF-8, because it is compared against SCREEN TEXT and the screen is text.
    #[must_use]
    pub fn echo_trail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.echo_trail)).into_owned()
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
            query: Arc::clone(&self.query),
            echo_trail: Arc::clone(&self.echo_trail),
            raw_output: Arc::clone(&self.raw_output),
            clipboard_answered: Arc::clone(&self.clipboard_answered),
        }
    }

    /// Who paints what is typed into this pane — see [`PaneEcho`](crate::pty::PaneEcho).
    #[must_use]
    pub fn echo(&self) -> Option<crate::pty::PaneEcho> {
        self.query.echo()
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
    /// Shared with the owning [`PanePty`] — see [`SharedQuery`].
    query: SharedQuery,
    /// Shared with the owning [`PanePty`] — see [`ECHO_TRAIL_CAP`].
    echo_trail: SharedEchoTrail,
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

    /// Who paints what is typed into this pane — see [`PaneEcho`](crate::pty::PaneEcho).
    ///
    /// ⚠ Read at the moment it is ASKED, never cached. It is the program's to change and every
    /// interactive agent changes it on startup, so a value taken at the pane's birth would be a
    /// claim about a terminal that no longer exists.
    #[must_use]
    pub fn echo(&self) -> Option<crate::pty::PaneEcho> {
        self.query.echo()
    }

    /// A snapshot of the child's raw output bytes (the source stream, before
    /// emulation) paired with whether the capture was truncated at the cap —
    /// The COMPLETE logical lines this pane has produced after absolute line `cursor`, and how
    /// many were lost before the caller asked — see
    /// [`Screen::lines_since`](sprag_vt::Screen::lines_since).
    ///
    /// ⚠ The reader for *what has this pane printed since I last looked*. Every consumer here
    /// reached first for a per-ROW comparison, and a row is not a unit the child produced — it is
    /// where the terminal happened to break a line at the width it happened to have, so a resize
    /// renumbers all of them and a repaint changes none of them.
    #[must_use]
    pub fn lines_since(&self, cursor: u64) -> LinesSince {
        self.with_screen(|screen| screen.lines_since(cursor))
    }

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
        write_input(&self.emulator, &self.writer, &self.echo_trail, bytes)
    }

    /// What has recently been written INTO this pane — see [`PanePty::echo_trail`].
    #[must_use]
    pub fn echo_trail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.echo_trail)).into_owned()
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

/// macOS: `proc_pidinfo(PROC_PIDVNODEPATHINFO)`, the same call `lsof` and `ps` make — the kernel
/// answers with the process's current vnode directory and its path.
///
/// # This is the ONE fact off Linux that a terminal cannot ask the shell for
///
/// The obvious alternative is OSC 7: the shell emits an escape sequence naming its directory after
/// every `cd`, and the terminal remembers the last one. **ghostty does exactly that and only that**
/// (`src/terminal/osc.zig`'s `report_pwd`, at `260288614`; nothing in its tree calls `libproc`,
/// `proc_pidinfo` or `F_GETPATH`), and herdr reads no process's directory at all — every `cwd` in
/// its tree is one it was handed.
///
/// ⚠ **The honest trade OSC 7 has, stated first**: it survives SSH. A remote shell emits its own
/// directory and the local terminal learns it, where this call can only ever answer about a process
/// on THIS machine — which is why [`crate::PanePty::cwd`]'s caller returns nothing for a remote
/// pane rather than reporting the daemon's own filesystem.
///
/// What asking the kernel buys is everything OSC 7 needs cooperation for: a pane running `vim`, a
/// build, `cat`, or a shell with no integration installed still answers, and the answer cannot be
/// stale — a sequence that was never sent is a directory the terminal still believes in.
///
/// # Reading it
///
/// `proc_pidinfo` answers with the number of bytes it filled, so a short write is a refusal and is
/// treated as one. `vip_path` is a NUL-terminated `[c_char; MAXPATHLEN]`, which libc declares as
/// `[[c_char; 32]; 32]` to stay buildable on old compilers — hence the flatten. Reading another
/// user's process needs root; this only ever reads a child THIS daemon forked, so the uid matches
/// by construction.
#[cfg(target_os = "macos")]
fn read_cwd(pid: u32) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let want = std::mem::size_of::<libc::proc_vnodepathinfo>();
    // SAFETY: `info` is a live, correctly sized allocation of exactly the type this flavour writes,
    // and the size handed over is its own. `pid` naming a process that is gone is an ordinary
    // refusal (a short answer), not unsound.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            libc::c_int::try_from(want).ok()?,
        )
    };
    if usize::try_from(got).ok()? != want {
        return None;
    }
    let path: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    // An empty path is the kernel saying it has no answer, which is an absence rather than the
    // root directory — and `PathBuf::from("")` would be neither.
    (!path.is_empty()).then(|| PathBuf::from(OsString::from_vec(path)))
}

/// Neither `/proc` nor `libproc`: an honest `None`, so a restored pane falls back to the daemon's
/// own cwd rather than to a guess.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

/// Read the foreground process group of a process's controlling terminal.
///
/// Linux: `/proc/<pid>/stat` field 8, `tpgid`, through the crate's one parse of that line
/// (the crate-private `procfs::Stat`) — which is where the `comm`-in-parentheses hazard, the
/// byte-rather-than-`&str` decision and the `-1`-is-an-absence rule are all stated. It is the same
/// number `tcgetpgrp` would return for that terminal (measured equal at a prompt, under a foreground
/// job, and after that job was killed) and it is reachable without a master fd, which this crate hands
/// to the resize coalescer thread.
///
/// **This used to be a second parser of that line, and it read the file as a `String`** — so a
/// child whose `comm` the kernel truncated mid-codepoint reported no foreground group at all, while
/// the sibling parser in the crate-private `ports` module had been written on bytes to survive that.
///
/// # Why this is public, and takes a PID rather than a pane
///
/// It is I/O, and the caller that runs it most often — the daemon's settle sweep, once per pane per
/// sweep interval — must not perform I/O while holding the workspace lock
/// every client wake wants. **Measured (R291): doing so turned a concurrent reader's median from
/// +0.8 us to +687 us and its p99 from +5.8 us to +41.8 ms**, because the sweeper released the lock
/// and immediately re-took it around 4 us of syscalls, which is a convoy.
///
/// So the sweep reads each pane's [`PanePty::pid`] under the lock, releases it, and calls this. A
/// pid is not a pane, and that is the point: nothing here can touch the registry.
///
/// **The residual, stated rather than smoothed over:** `PanePty::pid` stops answering once the exit
/// is published, which is what stops a recycled pid being read — and between that check and this
/// call the window is now microseconds wide instead of nanoseconds. It is the same window
/// [`crate::PaneProcesses`] already documents for the same reason, not a new class, and closing it
/// needs the exit to publish before the reap is observable.
/// ⚠ **NO LONGER PLATFORM-GATED, and the gate was on the wrong thing.** This body reads one field
/// off one struct; the platform question is entirely `procfs::stat`'s, which answers it on Linux and
/// on macOS now. A `cfg` here made a portable one-liner have two bodies and told every caller that
/// *what job is this pane running* is a Linux question — the shape R340 removed from `procfs` and
/// left behind here.
#[must_use]
pub fn foreground_pgid_of(pid: u32) -> Option<u32> {
    crate::procfs::stat(pid)?.tpgid
}

/// What the batch just applied to `emu` ASKED FOR — read against the counters this reader thread
/// last saw, which it then advances.
///
/// Called with the emulator lock held (two field reads) and its answer acted on outside it. The
/// marks live on the reader thread's own stack, so there is no shared "last seen" for a second
/// observer to race on and no bookkeeping to reset: this thread is the only caller by construction.
///
/// A batch that raised BOTH reports both, notification first — the words are the part a person can
/// act on, and a surface holding one message at a time should have them rather than the bell that
/// arrived in the same 8 KiB.
fn take_attention(emu: &Emulator, marks: &mut (u64, u64)) -> Vec<Attention> {
    let mut raised = Vec::new();
    let (notification_seq, bell_seq) = (emu.notification_seq(), emu.bell_seq());
    if notification_seq > marks.0
        && let Some(notification) = emu.notification()
    {
        raised.push(Attention::Raised(notification.clone()));
    }
    if bell_seq > marks.1 {
        raised.push(Attention::Bell);
    }
    *marks = (notification_seq, bell_seq);
    raised
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
    trail: &SharedEchoTrail,
    bytes: &[u8],
) -> io::Result<()> {
    lock(emulator).note_input();
    // ⚠ RECORDED BEFORE THE WRITE, so the trail can never be behind an echo that has already come
    // back. This is the ONE place a pane's input is written, which is what makes the trail complete
    // — a second write path would be a second answer that can drift.
    {
        let mut trail = lock(trail);
        trail.extend_from_slice(bytes);
        let over = trail.len().saturating_sub(ECHO_TRAIL_CAP);
        if over > 0 {
            trail.drain(..over);
        }
    }
    write_shared(writer, bytes)
}

/// The quiet window the resize coalescer waits for before applying a size. A
/// continuous splitter/window drag emits a distinct `(cols, rows)` at every
/// cell-width boundary; without coalescing each one issues a `TIOCSWINSZ` →
/// `SIGWINCH`, and a live shell redraws its prompt for every one (the emulator,
/// which does not yet rewrap, then accumulates them as fragmented copies).
/// Debouncing to the LATEST size after a brief quiet collapses the storm.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(40);

/// How long [`PanePty::drop`] lets the polite hangup work before it stops asking and kills.
///
/// A pane close hangs its child up ([`crate::pty::signal_child`], one `SIGHUP`) and then has to wait for the
/// reader thread, because that thread is the only reaper. The wait is the problem: `SIGHUP` is a
/// REQUEST. `kill(2)` reporting success means the signal was raised, not that it did anything — a
/// signal whose disposition is `SIG_IGN` at the moment of delivery is DISCARDED outright, leaving
/// no pending bit and no trace, and a shell passes through windows during its own startup where
/// that is true. When the request is lost the child keeps the slave open, the master read never
/// ends, the reader never returns, and an unbounded join here never returns either.
///
/// That is not theoretical. Under concurrent pane teardown a `cargo test` run of the GUI suite hung
/// in roughly one run in five, always inside this join; the stuck pane's shell was still alive
/// holding its own pty, with NO pending signal and `SIGHUP` by then neither blocked nor ignored —
/// the request had been discarded — and a `SIGHUP` sent by hand killed it instantly and released
/// the join. So the close cannot rest on a signal the child is free to drop.
///
/// Long enough that an ordinary exit is never hurried (a child that honours the hangup is gone in
/// milliseconds and never reaches this), short enough that a lost one costs a closing window a
/// pause instead of the process. A child still running when it expires is one that did not take the
/// hint, and closing a pane is not a negotiation.
const HANGUP_GRACE: Duration = Duration::from_secs(2);

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
        // Hang the child up so its slave fd closes, which unblocks the reader thread's `read()`
        // with EOF; then wait for that thread, which reaps as its last act. Signalling rather than
        // waiting here is what keeps a single reaper: two threads calling `wait()` on one child race
        // for the status, and the loser would leave `exit_status` empty for a pane that plainly had
        // one.
        crate::pty::signal_child(self.pid.unwrap_or(0), libc::SIGHUP);
        let Some(handle) = self.reader_thread.take() else {
            return;
        };
        // Then CHECK, because the hangup is a request the child may never have received (see
        // [`HANGUP_GRACE`]). `Disconnected` is the reader's own end hanging up — it is finished, so
        // the join below returns at once. Only `Timeout` means the request did not land, and the
        // answer to that is a signal nothing can refuse.
        if self.reader_done.recv_timeout(HANGUP_GRACE) == Err(RecvTimeoutError::Timeout) {
            self.kill_hard();
        }
        let _ = handle.join();
    }
}

impl PanePty {
    /// `SIGKILL` the pane's whole process GROUP, for the one case [`Drop`](Self::drop) cannot talk
    /// its way out of.
    ///
    /// The GROUP, not the child, and that distinction is the difference between working and not.
    /// EOF on the master needs EVERY holder of the slave to let go, and the child's descendants hold
    /// it too — its stdio is theirs by inheritance. Killing only the child leaves a foreground job
    /// running on the pty and the close is exactly as stuck as before (measured: with a `sleep`
    /// still on the pty, the reader returned only when that `sleep` did). Taking the group is also
    /// the right MEANING: closing a pane ends the pane's job, not just the shell that launched it.
    ///
    /// Addressing the group by the child's pid is exact rather than lucky: `portable-pty` puts the
    /// child through `setsid` before `exec`, so it leads its own session AND its own group — the
    /// negated pid names precisely this pane's processes and nothing else.
    ///
    /// Gated on the child NOT having been reaped, for the same reason [`pid`](Self::pid) is: the
    /// instant a status publishes, that pid is free to be recycled, and a group id built from a
    /// recycled pid would name a stranger's processes. `exit` is that gate — the reader publishes it
    /// as it reaps — so `None` here means the id is still this pane's to signal. (A reaped child is
    /// also one whose reader has finished, so this is unreachable in that state; the check makes it
    /// unreachable by construction rather than by argument.)
    fn kill_hard(&self) {
        if lock(&self.exit).is_some() {
            return;
        }
        let Some(pid) = self.pid else {
            return;
        };
        let Ok(pid): Result<libc::pid_t, _> = pid.try_into() else {
            return;
        };
        tracing::warn!(
            target: "sprag_terminal::pane_pty",
            pid,
            grace = ?HANGUP_GRACE,
            "the pane's child ignored its hangup; killing its process group so the pty can close",
        );
        // SAFETY: `kill` is async-signal-safe and takes no pointers. `-pid` is this pane's own
        // process group (the child leads it — see above), and the child is unreaped, checked above.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// [`read_cwd`] answers about a process this one can name — asked of THIS process, whose
    /// directory the test already knows.
    ///
    /// # Why the reader and not a pane
    ///
    /// Every other gate on this fact goes through a spawned child, so it exercises the reader only
    /// as far as the pane machinery reaches — and it says nothing at all on a platform where the
    /// reader answers `None`, because "the child has exited" and "this build cannot ask" arrive as
    /// the same value. That is not hypothetical: `read_cwd` returned `None` on every non-Linux
    /// target for the whole life of this file, and the first macOS run of this suite (R343) failed
    /// FOUR tests on it, in `workspace` and in `host`, none of which named the reader.
    ///
    /// So this one is deliberately platform-blind: it asks for a directory it already knows, and
    /// therefore fails on whichever platform's implementation is wrong. On Linux it drives the
    /// `/proc` symlink; on macOS, `proc_pidinfo`; on anything else it asserts the honest absence,
    /// which is a claim too — a build that starts guessing would fail here.
    #[test]
    fn a_process_this_one_can_name_reports_the_directory_it_is_working_in() {
        let mine = std::env::current_dir().expect("this process is somewhere");
        let read = read_cwd(std::process::id());

        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let read = read.expect("a platform with a reader must answer about its own process");
            // CANONICALISED on both sides: macOS resolves `/var` to `/private/var` and the two
            // spellings name one directory, so comparing the strings would fail on a difference
            // that is not one.
            assert_eq!(
                read.canonicalize().ok(),
                mine.canonicalize().ok(),
                "the reader must answer with the directory this process is actually in",
            );
        } else {
            assert_eq!(
                read, None,
                "a platform with no reader answers an honest absence, never a guess",
            );
        }
    }

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

    /// Closing a pane whose child IGNORES the hangup still completes.
    ///
    /// This is the wild failure made deterministic. A pane close asks its child to hang up with a
    /// single `SIGHUP`, and a signal that is ignored at the moment of delivery is discarded — no
    /// pending bit, no effect, and `kill(2)` still reports success. `trap "" HUP` puts the child in
    /// exactly that state on purpose; in the wild it was a shell that happened to be there while
    /// starting up, which is why it struck about one `cargo test` run in five and never with
    /// `--test-threads=1`.
    ///
    /// Before the close escalated, this did not FAIL here, it HUNG: the child kept the slave open,
    /// so the master read never ended, so the reader thread never returned, so the join never did.
    /// The child also leaves a `sleep` of its own on the pty, which is why the escalation has to
    /// take the process GROUP — killing the child alone leaves that `sleep` holding the slave and
    /// the close stays stuck until it finishes on its own.
    ///
    /// The LOWER bound matters as much as the upper one: a close that returned immediately would
    /// mean the child took the hint after all and the escalation path was never exercised.
    #[test]
    fn a_pane_whose_child_ignores_the_hangup_still_closes() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Announce readiness ON the pty, so the close is driven only once the trap is demonstrably
        // installed — waiting on that condition rather than on a timer.
        command.arg(r#"trap "" HUP; printf DEAF; sleep 60"#);
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let row0 = pty.with_screen(|screen| {
                (0..screen.cols())
                    .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                    .collect::<String>()
            });
            if row0.starts_with("DEAF") {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        assert!(
            !pty.is_eof(),
            "the child is alive with the hangup trapped, so there can be no EOF yet",
        );

        let closing = Instant::now();
        drop(pty);
        let took = closing.elapsed();
        assert!(
            took >= HANGUP_GRACE,
            "the close should have gone through the grace and escalated (took {took:?}) — \
             returning early would mean the child honoured the hangup and this proves nothing",
        );
        assert!(
            took < HANGUP_GRACE * 3,
            "the close must be bounded by the grace, not by the child (took {took:?})",
        );
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
            PaneHooks {
                on_dirty: Some(Box::new(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                })),
                ..PaneHooks::default()
            },
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
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
    // Linux AND macOS: this drives a reader that stopped being Linux-only when `procfs` and
    // `read_cwd` learned their macOS syscalls (R343). The gate that used to be here is the
    // reason the absence went five rounds unnoticed — the tests for the fact were on the one
    // platform that had it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
        let cwd = pty.cwd().expect("a live child's cwd is readable here");
        // Canonicalize both: the spawn dir may be a symlink (e.g. /tmp), while /proc
        // resolves the link to the real path — comparing raw strings would spuriously fail.
        assert_eq!(
            cwd.canonicalize().ok(),
            dir.canonicalize().ok(),
            "cwd tracks the directory the child was spawned in",
        );
    }

    /// `foreground_pgid` names the job the pane's terminal currently belongs to — which is what
    /// makes it able to see a process the pane did not spawn.
    ///
    /// Read TWICE with the input changed, because one reading cannot tell "the foreground job" from
    /// "the child": at a prompt those are the same number, and the whole point of this accessor is
    /// the case where they are not. So the shell is asked at rest (where it owns its own terminal)
    /// and again while a job it started holds the terminal — and the child is asserted ALIVE across
    /// both, since a caller that could use `is_eof` instead would not need this at all.
    ///
    /// The job is `sleep`, given a here-string of nothing to do, because what is being observed is
    /// terminal ownership rather than anything the job prints.
    // Linux AND macOS: this drives a reader that stopped being Linux-only when `procfs` and
    // `read_cwd` learned their macOS syscalls (R343). The gate that used to be here is the
    // reason the absence went five rounds unnoticed — the tests for the fact were on the one
    // platform that had it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn foreground_pgid_names_the_job_that_owns_the_terminal() {
        let mut command = CommandBuilder::new("/bin/bash");
        command.arg("--norc");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        let child = pty.pid().expect("a live child");

        let at_rest =
            wait_for(&pty, |pgid| pgid == Some(child)).expect("the shell owns its own tty");
        assert_eq!(
            at_rest, child,
            "at a prompt the foreground job IS the child"
        );

        pty.write(b"sleep 300\n").expect("write to the pty");
        let running = wait_for(&pty, |pgid| pgid.is_some_and(|pgid| pgid != child))
            .expect("a foreground job takes the terminal");
        assert_ne!(
            running, child,
            "a job the child started owns the terminal while it runs",
        );
        assert!(
            !pty.is_eof(),
            "and the child is alive throughout — this is exactly the case `is_eof` cannot see",
        );
    }

    /// `tty()` names the REAL device, checked against a source that has nothing to do with how it
    /// was obtained: the child's own `/proc/<pid>/fd/0`.
    ///
    /// Two independent answers are what make this an assertion rather than a restatement — the
    /// accessor reads a `ttyname_r` the PTY backend took on the SLAVE fd at `openpty`, and the
    /// control reads a symlink the KERNEL maintains for the child. Asserting the shape (`/dev/pts/`
    /// and a number) alone would pass for any pty on the box, including one belonging to somebody
    /// else's pane.
    ///
    /// It is deliberately not the other way round: `/proc/<pid>/fd/0` is the CONTROL here and not
    /// the implementation, because a child may redirect its own standard input and go on owning the
    /// terminal — which is exactly why the accessor does not read it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_pane_names_the_terminal_device_the_kernel_gave_its_child() {
        let mut command = CommandBuilder::new("/bin/sleep");
        command.arg("300");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        let child = pty.pid().expect("a live child");

        let named = pty.tty().expect("a unix pty names its device").to_owned();
        let kernels = std::fs::read_link(format!("/proc/{child}/fd/0"))
            .expect("the child's stdin is its terminal");
        assert_eq!(
            named, kernels,
            "the device the pane reports and the one the kernel gave the child are one device",
        );
        assert!(
            named.starts_with("/dev/pts/"),
            "and it is an addressable path, not a label: {}",
            named.display(),
        );
    }

    /// `/proc/<pid>/stat`'s second field is the executable name in parentheses and may contain BOTH
    /// spaces and parentheses, so the fields after it are found from the LAST `)` — never by
    /// splitting the line, and never from the first `)`.
    ///
    /// The fixture carries both hazards at once (`sl) eep`) because they break different parses and
    /// one name that only had a space would leave half the claim untested: a whitespace split from
    /// the start of the line misses on the space, and anchoring on the FIRST `)` misses on the
    /// paren. Either way the number read belongs to something else entirely, and a pane's child is
    /// whatever the user configured as their shell — so this is a rename away from being real.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_whose_name_breaks_a_naive_stat_parse_is_still_read_correctly() {
        let dir = std::env::temp_dir().join(format!("sprag-pty-space-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let spaced = dir.join("sl) eep");
        std::fs::copy("/bin/sleep", &spaced).expect("a copy of sleep under an awkward name");

        let mut command = CommandBuilder::new(&spaced);
        command.arg("300");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        let child = pty.pid().expect("a live child");
        let pgid = wait_for(&pty, |pgid| pgid.is_some()).expect("a foreground group");

        assert_eq!(
            pgid, child,
            "the child is its own foreground job, read past a comm field with a space in it",
        );
        drop(pty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Poll `foreground_pgid` until `want` accepts it, or give up. Job control settles on the
    /// child's own schedule, so a fixed sleep would either be flaky or slow.
    ///
    /// Gated with its callers, and that is the point rather than bookkeeping: ungating the test
    /// above without this one left `wait_for` undefined on macOS, and the local
    /// `--target aarch64-apple-darwin` check said so in seconds. A helper's platform is its
    /// callers' platform, always.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn wait_for(pty: &PanePty, want: impl Fn(Option<u32>) -> bool) -> Option<u32> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let pgid = pty.foreground_pgid();
            if want(pgid) {
                return pgid;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
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
    ///
    /// The child BLOCKS until this test tells it to end, and that is load-bearing: a child whose
    /// whole script is `exit 0` may already have run, exited and been reaped in place before the
    /// first assertion executes, and then the live-pid half fails — on a loaded machine, sometimes,
    /// while proving nothing about the withholding it is there to guard. Waiting on the condition
    /// each half needs (alive for the first, reaped for the second) is the fix; a longer-running
    /// script would only have widened the window it raced in.
    #[test]
    fn a_reaped_childs_pid_is_withheld_so_no_proc_walk_can_stray() {
        let pty = sh("read line");
        assert!(pty.pid().is_some(), "a live child has a usable pid");
        pty.write(b"\n").expect("end the child's read");
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
            PaneHooks {
                on_dirty: Some(Box::new(move || {
                    let _ = tx.send(());
                })),
                ..PaneHooks::default()
            },
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
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

    /// **A pane whose program does not exist fails, and strands nothing behind it.**
    ///
    /// R351 put the reader on the terminal BEFORE the child, so a failed birth is now a state with
    /// two things already waiting on a child that will never arrive: a reader blocked on the device,
    /// and, behind it, the reap blocked on the handle. Neither ends by being asked to. The device's
    /// last slave goes with the failed spawn, which is the reader's EOF; the handle's sender is a
    /// local dropped with the error, which is what turns the reap's wait into an answer.
    ///
    /// What is asserted is the WHOLE thread ending, not the exit hook — the hook fires on the way
    /// out, before the reap, so a gate built on it would pass over a reaper that waits forever. The
    /// hook's own captures are dropped when the reader's body returns, so a value that signals from
    /// its `Drop` reports the one moment that matters. With a deadline, because the failure being
    /// tested for is a hang, and a hang has no output.
    ///
    /// REVERT-PROOF: keep any second sender alive past the error return and the reap never wakes,
    /// the body never returns, and this deadline expires. Measured that way, not argued.
    #[test]
    fn a_pane_whose_program_does_not_exist_fails_without_stranding_its_reader() {
        /// Sends when it is dropped — captured by the exit hook, so it reports the reader's body
        /// RETURNING rather than the hook being called.
        struct SignalOnDrop(mpsc::Sender<()>);
        impl Drop for SignalOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let (tell, reader_finished) = mpsc::channel::<()>();
        let guard = SignalOnDrop(tell);
        let mut command = CommandBuilder::new("/nonexistent/sprag-has-no-program-here");
        command.env("TERM", "dumb");
        let birth = PanePty::spawn_with_dirty(
            command,
            20,
            4,
            PaneHooks {
                on_exit: Some(Box::new(move || {
                    let _ = &guard;
                })),
                ..PaneHooks::default()
            },
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
        );
        let error = birth
            .err()
            .expect("a program that is not there cannot be a pane");
        assert!(
            error.to_string().contains("spawn command"),
            "the birth names the step that failed: {error}",
        );
        reader_finished
            .recv_timeout(Duration::from_secs(10))
            .expect("the reader attached before the child runs to the end when the birth fails");
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
            PaneHooks {
                on_exit: Some(Box::new(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                })),
                ..PaneHooks::default()
            },
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
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
    /// ⚠⚠ **A PANE'S ECHO TRAIL IS THE SAME TRAIL THROUGH THE OWNER AND THROUGH A HANDLE.**
    ///
    /// The two are one `Arc`, and the gate is here because forgetting to clone it into
    /// [`PanePty::handle`] compiles: the handle would start its own empty trail, every writer that
    /// reaches a pane through a handle — which is every host-side writer — would record into a
    /// trail nobody reads, and the readiness barrier downstream would silently stop recognising
    /// echo. Same shape as the `raw_output` pair below, for the same reason.
    #[test]
    fn the_echo_trail_is_one_trail_through_the_owner_and_through_a_handle() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        let pty = PanePty::spawn(command, 20, 4).expect("spawn a pty");
        assert_eq!(
            pty.echo_trail(),
            "",
            "a pane nobody wrote to has an empty trail"
        );

        pty.write(b"TYPED-AT-THE-OWNER\r").expect("write");
        assert!(
            pty.echo_trail().contains("TYPED-AT-THE-OWNER"),
            "the owner records what is written through it: {:?}",
            pty.echo_trail(),
        );
        assert_eq!(
            pty.handle().echo_trail(),
            pty.echo_trail(),
            "and a handle reads the SAME trail, not one of its own",
        );

        pty.handle().write(b"TYPED-AT-A-HANDLE\r").expect("write");
        let trail = pty.echo_trail();
        assert!(
            trail.contains("TYPED-AT-THE-OWNER") && trail.contains("TYPED-AT-A-HANDLE"),
            "and a write through EITHER seam lands in it: {trail:?}",
        );
    }

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

        // ⚠ WAITED FOR, NOT SLEPT THROUGH. This was `sleep(80ms)` against a 20 ms window — a 4x
        // margin on another thread's scheduling, which is not a bound this test can read. R343 added
        // two live-PTY tests to this binary and the extra contention was enough: the macOS runner
        // reported `left: []`, the coalescer simply not having run yet.
        //
        // The claim is *the burst collapses to ONE size, the final one* — it says nothing about how
        // soon. So this waits for the first application and then asserts the WHOLE vector: a broken
        // debounce applies `(10, 5)` first, so the wait ends on a snapshot that is not `[(30, 5)]`
        // and the assertion fails with what it saw. Lengthening the sleep would have hidden the
        // flake and changed nothing about what the gate can discriminate.
        let deadline = Instant::now() + Duration::from_secs(10);
        while applied.lock().unwrap().is_empty() && Instant::now() < deadline {
            sleep(Duration::from_millis(5));
        }
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
    /// **A child asking for a person reaches the hook the moment it asks** — driven through a real
    /// pseudoterminal, because the whole point of this seam is that it fires from the reader thread
    /// and no polling consumer is involved.
    ///
    /// Three claims:
    ///
    /// * the WORDS arrive, with the urgency the child claimed — a hook that only said "something
    ///   happened" would leave the sentence to a poller, which is the state this replaces;
    /// * a BELL arrives as its own arm, so a surface is never asked to invent a sentence for it;
    /// * and the CONTROL: ordinary output that raises neither fires NOTHING. Without it a hook that
    ///   ran on every batch would pass the two above.
    #[test]
    fn a_childs_attention_reaches_the_hook_from_the_reader_thread() {
        let raised: Arc<Mutex<Vec<Attention>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&raised);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Plain text FIRST (the control's subject), then the notification, then the bell.
        command.arg("printf 'working\\n'; sleep 0.2; printf '\\033]99;u=2;the build needs you\\007'; sleep 0.2; printf '\\007'; cat");
        command.env("TERM", "xterm");
        let _pty = PanePty::spawn_with_dirty(
            command,
            40,
            6,
            PaneHooks {
                on_attention: Some(Box::new(move |attention| {
                    lock(&sink).push(attention);
                })),
                ..PaneHooks::default()
            },
            &[],
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
        )
        .expect("spawn a pty");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) && lock(&raised).len() < 2 {
            sleep(Duration::from_millis(20));
        }
        let seen = lock(&raised).clone();
        assert_eq!(
            seen.len(),
            2,
            "exactly the two things the child ASKED with — the `working` line is output, not a \
             request for a person: {seen:?}",
        );
        match &seen[0] {
            Attention::Raised(notification) => {
                assert_eq!(notification.title.as_deref(), Some("the build needs you"));
                assert_eq!(
                    notification.urgency,
                    sprag_vt::Urgency::Critical,
                    "the child's own claim survives the whole path to the hook",
                );
            }
            other => panic!("the notification arrives first: {other:?}"),
        }
        assert_eq!(seen[1], Attention::Bell);
    }

    /// **A RESTORED pane does not raise its predecessor's notification.**
    ///
    /// The replayed scrollback runs the same OSC handling live output does, so the emulator comes
    /// back with `notification_seq` at 1 before the child has written a byte. The reader reads its
    /// starting marks AFTER that replay, which is what makes the restore silent.
    ///
    /// **THE FIXTURE IS THE WHOLE POINT, and the first one was VACUOUS** — found by a revert-proof
    /// that came back GREEN with the marks forced to zero. That version's child raised a live
    /// notification of its own, and the emulator LATCHES only the most recent one: the replayed
    /// notification was already overwritten by the time the reader looked, so the stale seq fired
    /// the LIVE words and every assertion still held. The child here raises NOTHING and merely
    /// PRINTS, so a reader that started its marks at zero reports the predecessor's sentence — which
    /// is the defect, and is now the only thing that can happen.
    ///
    /// The CONTROL is the second pane below: same replay, same hook, and a child that DOES raise
    /// one. Without it a build where the hook never fired at all would pass the silence above.
    #[test]
    fn a_restored_panes_recorded_notification_is_not_raised_again() {
        let recorded: &[u8] = b"\x1b]9;the one from before the reboot\x07";
        let raised: Arc<Mutex<Vec<Attention>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&raised);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Ordinary output ONLY. Nothing here asks for a person, so anything the hook reports came
        // from the replay above.
        command.arg("printf 'working\\n'; cat");
        command.env("TERM", "xterm");
        let _quiet = PanePty::spawn_with_dirty(
            command,
            40,
            6,
            PaneHooks {
                on_attention: Some(Box::new(move |attention| {
                    lock(&sink).push(attention);
                })),
                ..PaneHooks::default()
            },
            recorded,
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
        )
        .expect("spawn a pty");

        // THE CONTROL, on its own pane through the same hook: a restored pane whose child DOES raise
        // one. Waiting for it is what gives the quiet pane above time to have spoken.
        let control: Arc<Mutex<Vec<Attention>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&control);
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf 'working\\n'; sleep 0.2; printf '\\033]9;this one is live\\007'; cat");
        command.env("TERM", "xterm");
        let _live = PanePty::spawn_with_dirty(
            command,
            40,
            6,
            PaneHooks {
                on_attention: Some(Box::new(move |attention| {
                    lock(&sink).push(attention);
                })),
                ..PaneHooks::default()
            },
            recorded,
            sprag_vt::DEFAULT_SCROLLBACK_LINES,
        )
        .expect("spawn a pty");

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) && lock(&control).is_empty() {
            sleep(Duration::from_millis(20));
        }
        let seen_control = lock(&control).clone();
        assert_eq!(
            seen_control.len(),
            1,
            "the control's live notification reached the hook: {seen_control:?}",
        );
        match &seen_control[0] {
            Attention::Raised(notification) => assert_eq!(notification.body, "this one is live"),
            other => panic!("the control raises a notification: {other:?}"),
        }

        let seen_quiet = lock(&raised).clone();
        assert!(
            seen_quiet.is_empty(),
            "a restored pane whose child asked for nothing must ask for nothing: {seen_quiet:?}",
        );
    }
}
