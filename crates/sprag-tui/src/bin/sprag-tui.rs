//! `sprag-tui` — attach to a sprag session and paint a pane into this terminal.
//!
//! The second frontend, at its second slice: it attaches, it paints, and it quits. **It does not
//! yet send anything to the pane** — the local-input-to-wire-key decoder and its PTY harness are
//! the next slice, and they are the gate for the front. What this binary proves is the half that
//! has to be right first: that a `sprag-term` session's cells can be rendered onto a real terminal
//! by a process with no GPU in its dependency closure.
//!
//! # The loop is a real select, not a poll
//!
//! Two things wake this client: the host's change notification (the poll thread inside
//! [`WireHost`](sprag_client::WireHost), driven by `scene/waitFor`) and the local terminal. Both
//! arrive at ONE blocking [`Terminal::poll_input`] — the host side through termwiz's
//! [`TerminalWaker`](termwiz::terminal::TerminalWaker), which writes to a pipe the same `select`
//! is already watching. So an idle client costs nothing: no tick, no timeout, no periodic re-fetch.
//! That is the event loop the H1 design called for, and it needed no new machinery because
//! termwiz's waker exists for exactly this.
//!
//! # What it deliberately does not do yet
//!
//! * **One pane.** The first pane of the session. The character-cell layouter that tiles the rest
//!   is slice 4, and it needs a wire action the daemon does not have yet.
//! * **No resize of the PANE.** A local resize resizes the local SURFACE (so the view is cropped
//!   or margined, never corrupted) but does not tell the host, because a client that resizes a
//!   pane it cannot type into would be reshaping other people's windows for a read-only view.
//!   `RESIZE_ACTION` lands with input, in the slice where both halves can be proven together.
//! * **No keys to the pane.** `q` and `Ctrl-C` quit this viewer; every other key is discarded.
//!   Both bindings are PROVISIONAL: once keys reach the pane, `q` belongs to the program running
//!   in it and the client's own commands move behind the prefix table of slice 4.

use std::error::Error;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pinion_core::QuitSink;
use sprag_client::WireHost;
use sprag_host::HostClient;
use sprag_terminal::PaneId;
use sprag_tui::grid_changes;
use termwiz::caps::Capabilities;
use termwiz::color::ColorAttribute;
use termwiz::input::{InputEvent, KeyCode, Modifiers};
use termwiz::surface::Change;
use termwiz::terminal::buffered::BufferedTerminal;
use termwiz::terminal::{SystemTerminal, Terminal, TerminalWaker};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Deliberately not `tracing`: this is the ONE message a user who cannot attach must
            // see, and it is printed after the terminal has been restored (or before it was ever
            // taken), so it lands on a working screen.
            eprintln!("sprag-tui: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Attach, paint, and run until the user quits or the host is gone.
///
/// The order here is load-bearing. The session is attached BEFORE the terminal is taken, so a
/// failure to reach the daemon prints an ordinary error on an ordinary screen instead of a
/// diagnostic nobody can read inside an alternate screen that is about to be torn down.
fn run() -> Result<(), Box<dyn Error>> {
    let mut terminal = SystemTerminal::new(Capabilities::new_from_env()?)?;
    let (cols, rows) = screen_size(&mut terminal)?;

    // The two edges the client is woken by, each a flag plus a wake of the one blocking poll.
    // The flags carry WHICH edge fired; the wake only says that one did.
    let repaint = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let waker = terminal.waker();

    let host = WireHost::spawn_or_attach(
        // No argv: the host's own `$SHELL`, the same default `sprag attach` gives the GUI.
        None,
        cols,
        rows,
        1,
        Arc::new({
            let (repaint, waker) = (Arc::clone(&repaint), waker.clone());
            move || {
                repaint.store(true, Ordering::Release);
                // A failed wake is not worth ending the client over: the flag is already set, so
                // the next event of any kind repaints. Losing a wake costs latency, not state.
                let _ = waker.wake();
            }
        }),
        Arc::new(HostGone {
            quit: Arc::clone(&quit),
            waker: waker.clone(),
        }),
    )?;

    // Only now is the terminal taken. The hook goes in FIRST so that a panic between here and the
    // end of the loop still leaves a usable shell behind.
    install_restore_hook();
    terminal.set_raw_mode()?;
    terminal.enter_alternate_screen()?;
    let mut screen = BufferedTerminal::new(terminal)?;
    // `BufferedTerminal::new` sizes its surface from the terminal's raw answer, so the fallback
    // has to be applied here too or a terminal that reports nothing paints into a 0x0 surface.
    screen.resize(usize::from(cols), usize::from(rows));

    // The first paint clears, because the surface starts blank but the terminal underneath it does
    // not, and because the pane is almost never exactly the size of this window.
    paint(&mut screen, &host, Clear::Yes)?;

    loop {
        // `None` blocks until the terminal has something OR the waker fires — the select this
        // client's whole idle cost rests on.
        match screen.terminal().poll_input(None)? {
            // A key of ours ends the loop; every other key is dropped on the floor, because a
            // read-only viewer that forwarded keys would be typing into a pane it cannot show the
            // consequences of until the next poll.
            Some(InputEvent::Key(key)) if is_quit(&key.key, key.modifiers) => break,
            // A local resize changes only the local surface (see the module docs). Clearing is
            // what keeps the margin honest: the region the pane does not cover holds whatever the
            // old, differently-shaped screen left there.
            Some(InputEvent::Resized { .. }) => {
                // Re-read through `screen_size` rather than trusting the event's payload or
                // `BufferedTerminal::check_for_resize`: both take the terminal's raw answer, so a
                // terminal that reports 0 would undo the boot fallback and leave a 0x0 surface.
                let (cols, rows) = screen_size(screen.terminal())?;
                screen.resize(usize::from(cols), usize::from(rows));
                paint(&mut screen, &host, Clear::Yes)?;
            }
            // `Wake` carries no payload by design — which edge fired is in the flags below.
            Some(_) | None => {}
        }
        if quit.load(Ordering::Acquire) {
            break;
        }
        // `swap` rather than `load` + `store`: a change landing DURING the paint must leave the
        // flag set, so the next iteration repaints instead of showing a frame one behind.
        if repaint.swap(false, Ordering::AcqRel) {
            paint(&mut screen, &host, Clear::No)?;
        }
    }

    // `BufferedTerminal`'s inner terminal restores the termios, the cursor and the alternate
    // screen on drop, so the normal exit path needs nothing here beyond letting it drop.
    Ok(())
}

/// Whether a repaint should blank the surface first.
///
/// A named pair rather than a `bool` because the two callers are far apart and `paint(.., true)`
/// at a call site says nothing about what is true.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clear {
    /// Blank the surface first: the pane does not cover it, or what is under it is unknown.
    Yes,
    /// Leave the surface alone and let the diff do its work — the steady state.
    No,
}

/// Paint the session's first pane onto `screen` and flush the difference to the terminal.
///
/// Reading the pane list every frame rather than caching an id is deliberate and free: both
/// [`HostClient::pane_ids`] and a live [`HostClient::pane_cells`] read
/// [`WireHost`](sprag_client::WireHost)'s poll-maintained cache with no socket call, and a cached
/// id would go stale the moment the pane it names is closed from another client.
fn paint(
    screen: &mut BufferedTerminal<SystemTerminal>,
    host: &WireHost,
    clear: Clear,
) -> Result<(), Box<dyn Error>> {
    let Some(pane) = first_pane(host) else {
        // No panes is a legitimate transient state (the last one just closed), not an error: the
        // host will either grow one or go away, and both wake this loop.
        return Ok(());
    };
    if clear == Clear::Yes {
        screen.add_change(Change::ClearScreen(ColorAttribute::Default));
    }
    screen.add_changes(grid_changes(&host.pane_cells(pane, 0)));
    screen.flush()?;
    Ok(())
}

/// The pane this client shows: the session's first, in host order.
///
/// One pane is slice 2's whole scope, so "which one" has exactly one defensible answer until the
/// layouter exists — and picking the first keeps it the same pane across repaints, which a
/// most-recently-changed rule would not.
fn first_pane(host: &WireHost) -> Option<PaneId> {
    host.pane_ids().first().copied()
}

/// The client's own quit keys, which are NOT the pane's.
///
/// `Ctrl-C` and `q`, both provisional (see the module docs). Raw mode is what makes this a
/// decision at all: with the line discipline off, `Ctrl-C` is a byte rather than a signal, so a
/// viewer that bound nothing could only be left by killing it from another terminal.
fn is_quit(key: &KeyCode, mods: Modifiers) -> bool {
    match key {
        KeyCode::Char('c' | 'C') => mods.contains(Modifiers::CTRL),
        KeyCode::Char('q') => !mods.contains(Modifiers::CTRL),
        _ => false,
    }
}

/// The [`QuitSink`] the wire client pulls when the daemon is definitively gone — the tmux
/// convention that a client detaches when its server dies.
///
/// It cannot end the process itself: the loop owns the terminal, and tearing that down from the
/// poll thread would race the paint. So it does what every other edge here does — set a flag and
/// wake the select.
struct HostGone {
    /// Read by the loop right after each wake.
    quit: Arc<AtomicBool>,
    /// Unblocks the loop's `poll_input` so the flag is read now rather than at the next keystroke.
    waker: TerminalWaker,
}

impl QuitSink for HostGone {
    fn request_quit(&self) {
        self.quit.store(true, Ordering::Release);
        let _ = self.waker.wake();
    }
}

/// Restore the terminal from a panic hook, BEFORE the panic message is printed.
///
/// `UnixTerminal`'s own `Drop` already restores the termios, the cursor and the alternate screen,
/// and it runs during unwinding — so the terminal is never left broken. What `Drop` cannot fix is
/// ORDER: the default hook prints the panic while the alternate screen is still up, and leaving it
/// afterwards takes the message with it. A user then sees a client vanish with no explanation.
///
/// This is the one place this crate writes escape sequences by hand, and the exception is
/// principled: a panic hook has no working object to ask, `ESC [ ? 1049 l` and `ESC [ ? 25 h` are
/// the two sequences every emulator that supports the alternate screen at all implements, and the
/// cost of being wrong is a stray six characters against the certainty of a lost diagnostic.
fn install_restore_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = std::io::stdout();
        // Leave the alternate screen and show the cursor. Errors are unrecoverable here by
        // definition — we are already panicking — and swallowing them keeps the real message.
        let _ = out.write_all(b"\x1b[?1049l\x1b[?25h");
        let _ = out.flush();
        previous(info);
    }));
}

/// The size to paint at when the terminal reports none — tmux's `default-size`, and the same
/// 80x24 `sprag-term` itself falls back to for a dimension-less boot.
const FALLBACK_SIZE: (u16, u16) = (80, 24);

/// This terminal's `(cols, rows)`, with a size it can actually be used at.
///
/// **A terminal that reports ZERO is the case this function exists for, and it is not
/// hypothetical.** A PTY nobody has set a winsize on answers `0x0` — which is what a PTY allocated
/// by a harness, and an ssh session before its first window-change, both look like. Forwarding
/// that zero was a real defect, found by running this binary rather than by reading it: the host's
/// `opt_dim` rejects a non-positive dimension, so the whole attach failed with `InvalidParams` and
/// a message that named nothing about a size. A zero-column pane is not a pane, and the host is
/// right to refuse it.
///
/// So an unknown size becomes [`FALLBACK_SIZE`] rather than an error: every terminal program makes
/// the same substitution (tmux names it `default-size`), a wrong guess is corrected by the first
/// resize, and the alternative — refusing to start — would turn a recoverable unknown into a
/// failure to attach at all.
///
/// The upper clamp is separate and dull: `ScreenSize` counts in `usize` while a pane's grid is
/// `u16` end to end, and saturating rather than wrapping keeps an implausibly wide terminal from
/// becoming a one-column pane.
fn screen_size(terminal: &mut SystemTerminal) -> Result<(u16, u16), Box<dyn Error>> {
    let size = terminal.get_screen_size()?;
    let dim = |value: usize, fallback: u16| match u16::try_from(value).unwrap_or(u16::MAX) {
        0 => fallback,
        measured => measured,
    };
    Ok((
        dim(size.cols, FALLBACK_SIZE.0),
        dim(size.rows, FALLBACK_SIZE.1),
    ))
}
