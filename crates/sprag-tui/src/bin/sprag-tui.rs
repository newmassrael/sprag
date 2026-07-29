//! `sprag-tui` — attach to a sprag session, paint a pane into this terminal, and type into it.
//!
//! The second frontend, at its third slice: the round trip is closed. Cells come out of the host
//! and onto this terminal; keystrokes go from this terminal into the pane; a window resize reaches
//! the pane's PTY. That is what "a session can be attached over ssh" means, and
//! `tests/pty_round_trip.rs` proves it against a real daemon through a real pseudoterminal.
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
//! # Keys belong to the pane, so the client needs a prefix
//!
//! Once keystrokes reach the child, every key is spoken for: `q` is a program's quit, `Ctrl-C` is
//! a program's interrupt, and raw mode means the client cannot fall back on a signal either. So
//! this client's own commands live behind a PREFIX, which is tmux's answer and the one a user
//! already has in their fingers — [`PREFIX_KEY`] then a command key.
//!
//! Slice 3 binds exactly one command, `d` for detach, because that is the one a client cannot do
//! without: something has to give the terminal back. `prefix prefix` types a literal prefix into
//! the pane (tmux's `send-prefix`), which is what makes the prefix key itself reachable by the
//! program running there. Slice 4 grows the table; H2 makes it configurable. Until then the choice
//! of `Ctrl-B` is a default, not a decision anyone can change.
//!
//! # What it deliberately does not do yet
//!
//! * **One pane.** The first pane of the session. The character-cell layouter that tiles the rest
//!   is slice 4, and it needs a wire action the daemon does not have yet.
//! * **Latest attach wins the pane's size.** Attaching resizes the pane to this terminal, and so
//!   does every later window change. With one client that is simply correct; with several it is a
//!   POLICY, and the same one tmux spells `window-size latest`. The alternatives tmux also offers
//!   (smallest attached client, or a per-client viewport over a larger pane) need a client-size
//!   registry the daemon does not have, and choosing between them is H2's, not this slice's.
//! * **No mouse, and it is turned OFF rather than left on.** The wire carries a semantic
//!   [`MouseInput`](sprag_input::MouseInput) that the host gates against the pane's tracking mode,
//!   so the path exists and slice 4 will use it. Until then this client asks termwiz NOT to enable
//!   mouse reporting on the local terminal (see [`local_capabilities`]) — because termwiz's
//!   `set_raw_mode` enables it by default, and a client that captures the mouse and then discards
//!   every report has taken click-drag selection and wheel scrolling away from the user's own
//!   terminal emulator in exchange for nothing.
//! * **Type-ahead before the client is up is lost.** `set_raw_mode` sets the termios with
//!   `TCSAFLUSH`, which purges whatever was typed before the client got there. That is what every
//!   full-screen program does and it is not this client's to change, but it is a real thing a user
//!   can see: characters typed into `ssh host sprag attach --tui` while it is still connecting do
//!   not arrive.

use std::error::Error;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pinion_core::QuitSink;
use sprag_client::WireHost;
use sprag_host::HostClient;
use sprag_terminal::PaneId;
use sprag_tui::{WireKey, grid_changes, wire_key};
use termwiz::caps::{Capabilities, ProbeHints};
use termwiz::color::ColorAttribute;
use termwiz::input::{InputEvent, KeyCode, KeyEvent, Modifiers};
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
    let mut terminal = SystemTerminal::new(local_capabilities()?)?;
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

    // The pane this client attached to was sized by whoever created it, which is this client only
    // when it created the session too. Matching it to the terminal HERE — before the first paint,
    // through the same call a window change uses — is what makes an attach over ssh show a pane
    // shaped like the window it is being shown in.
    resize_pane(&host, cols, rows);

    // The first paint clears, because the surface starts blank but the terminal underneath it does
    // not, and because the pane is almost never exactly the size of this window.
    paint(&mut screen, &host, Clear::Yes)?;

    // Where the next key goes. Starts at the pane: the prefix is a departure from the steady
    // state, not the other way round.
    let mut keys = Keys::ToPane;
    loop {
        // `None` blocks until the terminal has something OR the waker fires — the select this
        // client's whole idle cost rests on.
        match screen.terminal().poll_input(None)? {
            Some(InputEvent::Key(event)) => match command(&mut keys, &event) {
                Command::Detach => break,
                Command::Swallow => {}
                Command::ToPane(key) => send_key(&host, &key),
            },
            // A bracketed paste arrives as ONE event rather than a key per character, and this arm
            // is REACHED: termwiz's `set_raw_mode` enables DEC private mode 2004 on the local
            // terminal, so a paste into this window comes back as `Paste` and would be silently
            // dropped without it.
            //
            // Forwarded as a paste rather than as text, and the distinction is the point: the host
            // brackets it if — and only if — the pane's CHILD asked for bracketing, which is a mode
            // only the host can see. A shell that wanted to see a multi-line paste as one edit
            // still does; one that did not still runs it line by line.
            Some(InputEvent::Paste(text)) => paste(&host, &text),
            // A window change resizes both ends: the local surface, so the view is not cropped,
            // and the PANE, so the program inside it reflows. Clearing is what keeps the margin
            // honest — the region the pane does not cover holds whatever the old, differently
            // shaped screen left there.
            Some(InputEvent::Resized { .. }) => {
                // Re-read through `screen_size` rather than trusting the event's payload or
                // `BufferedTerminal::check_for_resize`: both take the terminal's raw answer, so a
                // terminal that reports 0 would undo the boot fallback and leave a 0x0 surface.
                let (cols, rows) = screen_size(screen.terminal())?;
                screen.resize(usize::from(cols), usize::from(rows));
                resize_pane(&host, cols, rows);
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

/// Send one decoded key to the pane this client shows.
///
/// A key the host declines is logged, not surfaced: the only place this client could report it is
/// the screen it is painting a pane onto, and a viewer that scribbled diagnostics over a user's
/// program would be worse than the dropped key. `false` covers a key `sprag-input` has no encoding
/// for (F13 upward) and a pane that closed between the poll and the send — neither is this
/// client's to fix.
fn send_key(host: &WireHost, key: &WireKey) {
    let Some(pane) = first_pane(host) else {
        return;
    };
    let mut scratch = [0u8; 4];
    let name = key.name(&mut scratch);
    if !host.send_key(pane, name, key.mods()) {
        tracing::debug!(target: "sprag_tui::input", key = name, "the host did not encode this key");
    }
}

/// Forward pasted text to the pane, letting the host decide whether it is bracketed.
fn paste(host: &WireHost, text: &str) {
    let Some(pane) = first_pane(host) else {
        return;
    };
    if !host.paste(pane, text) {
        tracing::debug!(target: "sprag_tui::input", bytes = text.len(), "the paste did not reach the pane");
    }
}

/// The display's `(cell_width, cell_height)` in pixels, which a character-cell client has no way
/// to know: it is told a window is 80 columns wide, never how wide a column is.
///
/// `(0, 0)` is the wire's "unknown" — the host keeps the pane's last-known cell geometry rather
/// than writing zeroes into the PTY winsize, so a pane that a GUI sized keeps truthful
/// `ws_xpixel` / `ws_ypixel` reports even while a TUI is the one resizing it.
const CELL_PX_UNKNOWN: (u16, u16) = (0, 0);

/// Resize the pane this client shows to `cols` x `rows` — the reflow the program inside it sees.
///
/// The no-op guard reads [`WireHost`]'s poll-maintained cache rather than the socket, so the
/// common case (a resize event that did not change the character grid, which is every pixel-level
/// drag in a GUI terminal emulator) costs no RPC and no reflow. It is a guard against WORK, not
/// against correctness: `RESIZE_ACTION` is idempotent.
fn resize_pane(host: &WireHost, cols: u16, rows: u16) {
    let Some(pane) = first_pane(host) else {
        return;
    };
    if host.pane_grid_size(pane) == (cols, rows) {
        return;
    }
    host.resize(pane, cols, rows, CELL_PX_UNKNOWN);
}

/// The client's prefix key: `Ctrl-B`, tmux's default (see the module docs for why a prefix exists
/// at all, and why this is a default rather than a decision).
const PREFIX_KEY: char = 'b';

/// Where the next keystroke goes.
///
/// Two states rather than a `bool` because the prefix is not a modifier — it is a mode the client
/// enters and leaves, and `after_prefix: true` at a call site says nothing about which way round
/// that is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keys {
    /// The steady state: keys are the program's.
    ToPane,
    /// The prefix was just pressed, so the next key is a command to this client.
    AfterPrefix,
}

/// What the loop should do with a key.
#[derive(PartialEq, Eq, Debug)]
enum Command {
    /// Send it to the pane.
    ToPane(WireKey),
    /// Give the terminal back and leave the session running.
    Detach,
    /// Nothing — the key was the prefix itself, an unbound command, or one the wire cannot spell.
    Swallow,
}

/// Route one key through the prefix table, advancing `keys`.
///
/// An unbound command key is SWALLOWED rather than passed through to the pane, which is tmux's
/// behaviour and the safer of the two: a user who typed the prefix meant to address the client, so
/// delivering their mistake to a shell would run something they did not ask for.
fn command(keys: &mut Keys, event: &KeyEvent) -> Command {
    match *keys {
        Keys::ToPane if is_prefix(event) => {
            *keys = Keys::AfterPrefix;
            Command::Swallow
        }
        Keys::ToPane => wire_key(event).map_or(Command::Swallow, Command::ToPane),
        Keys::AfterPrefix => {
            // One command key, whatever it turns out to be: the prefix is a one-key mode, so the
            // reset happens here rather than in each arm, where a new binding could forget it.
            *keys = Keys::ToPane;
            match event.key {
                // `prefix prefix` types a literal prefix — tmux's `send-prefix`, and what keeps
                // `Ctrl-B` reachable by the program running in the pane.
                _ if is_prefix(event) => wire_key(event).map_or(Command::Swallow, Command::ToPane),
                // `Ctrl-D` is a program's end-of-file and must not be a detach; the binding is the
                // bare letter, exactly as tmux spells it.
                KeyCode::Char('d') if !event.modifiers.contains(Modifiers::CTRL) => Command::Detach,
                _ => Command::Swallow,
            }
        }
    }
}

/// Whether `event` is the prefix.
///
/// Both cases of the letter are accepted because they are two spellings of one keystroke: a
/// terminal sends `Ctrl-B` as the C0 byte `0x02`, which termwiz reports as lowercase, while a
/// terminal using the `CSI u` encoding reports whichever case the layout produced.
fn is_prefix(event: &KeyEvent) -> bool {
    event.modifiers.contains(Modifiers::CTRL)
        && matches!(event.key, KeyCode::Char(c) if c.eq_ignore_ascii_case(&PREFIX_KEY))
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

/// What this client asks of the terminal it was started in: everything the environment advertises,
/// except the mouse.
///
/// [`Terminal::set_raw_mode`] does not only set the termios — it also turns on every input mode the
/// capabilities claim, which by default includes ANY-EVENT mouse reporting (`DECSET 1003` + the
/// SGR encoding `1006`). Enabling that is a decision, not a detail: once it is on, the user's
/// terminal emulator stops handling the mouse itself and forwards reports here instead, so
/// click-drag selection and wheel scrolling stop working in their window. A client that then
/// discarded every report — which is all this one can do until the pane-addressed
/// [`MouseInput`](sprag_input::MouseInput) path of slice 4 exists — would be taking that away for
/// nothing.
///
/// Bracketed paste is deliberately LEFT ON: this client handles the [`InputEvent::Paste`] it
/// produces, and it is what lets a paste reach the pane as one edit rather than as a burst of
/// keystrokes.
fn local_capabilities() -> Result<Capabilities, Box<dyn Error>> {
    Ok(Capabilities::new_with_hints(
        ProbeHints::new_from_env().mouse_reporting(Some(false)),
    )?)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes a terminal sends for a keystroke, as termwiz's own parser decodes them — so the
    /// events these tests route are the ones a terminal produces rather than ones this author
    /// invented. `tests/key_round_trip.rs` makes the same argument at length.
    fn typed(bytes: &[u8]) -> KeyEvent {
        let mut events = Vec::new();
        // `maybe_more = false`: the sequence is complete, so a lone `ESC` resolves as Escape
        // rather than waiting to see whether an Alt-combination follows.
        termwiz::input::InputParser::new().parse(bytes, |event| events.push(event), false);
        match events.as_slice() {
            [InputEvent::Key(event)] => event.clone(),
            other => panic!("{bytes:?} is not one key event: {other:?}"),
        }
    }

    /// The name a routed key would be sent to the pane under, or `None` if it goes nowhere.
    fn routed(keys: &mut Keys, bytes: &[u8]) -> Option<String> {
        match command(keys, &typed(bytes)) {
            Command::ToPane(key) => {
                let mut scratch = [0u8; 4];
                Some(key.name(&mut scratch).to_owned())
            }
            _ => None,
        }
    }

    const CTRL_B: &[u8] = &[0x02];

    /// The steady state: a key is the program's, not the client's.
    #[test]
    fn an_ordinary_key_reaches_the_pane() {
        let mut keys = Keys::ToPane;
        assert_eq!(routed(&mut keys, b"q").as_deref(), Some("q"));
        assert_eq!(keys, Keys::ToPane);
    }

    /// The two keys slice 2 quit on now belong to the program — which is the whole reason the
    /// prefix exists, so it is asserted rather than left to the module docs.
    #[test]
    fn the_old_quit_keys_are_the_programs_now() {
        let mut keys = Keys::ToPane;
        assert_eq!(routed(&mut keys, b"q").as_deref(), Some("q"));
        // `Ctrl-C`: an interrupt for the child, not a quit for the client.
        assert_eq!(routed(&mut keys, &[0x03]).as_deref(), Some("c"));
        assert!(routed(&mut keys, &[0x03]).is_some_and(|_| keys == Keys::ToPane));
    }

    /// The prefix is swallowed and arms the next key; `d` then detaches.
    #[test]
    fn the_prefix_then_d_detaches() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(keys, Keys::AfterPrefix, "the prefix arms the next key");
        assert_eq!(command(&mut keys, &typed(b"d")), Command::Detach);
        assert_eq!(keys, Keys::ToPane, "the mode is one key long");
    }

    /// A bare `d` with no prefix is a letter, not a detach. The revert-proof for the prefix
    /// mechanism itself: route `d` first and the client would leave before anything was typed.
    #[test]
    fn a_bare_d_is_a_letter() {
        let mut keys = Keys::ToPane;
        assert_eq!(routed(&mut keys, b"d").as_deref(), Some("d"));
    }

    /// `Ctrl-D` after the prefix is not a detach — the binding is the bare letter, and a program's
    /// end-of-file must survive a slip of the Ctrl key.
    #[test]
    fn ctrl_d_after_the_prefix_is_not_a_detach() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(command(&mut keys, &typed(&[0x04])), Command::Swallow);
        assert_eq!(keys, Keys::ToPane);
    }

    /// `prefix prefix` types a literal prefix into the pane, which is what keeps `Ctrl-B` reachable
    /// by a program that binds it (readline's backward-char, for one).
    #[test]
    fn the_prefix_twice_types_a_literal_prefix() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        let sent = command(&mut keys, &typed(CTRL_B));
        let Command::ToPane(key) = sent else {
            panic!("the second prefix reaches the pane: {sent:?}");
        };
        let mut scratch = [0u8; 4];
        assert_eq!(key.name(&mut scratch), "b");
        assert!(key.mods().ctrl, "and it is still a Ctrl-B");
        assert_eq!(keys, Keys::ToPane);
    }

    /// An unbound command key is dropped rather than delivered — a user who typed the prefix meant
    /// to address the client, so their mistake must not reach a shell.
    #[test]
    fn an_unbound_command_key_is_swallowed() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(command(&mut keys, &typed(b"z")), Command::Swallow);
        assert_eq!(keys, Keys::ToPane, "and the mode still ends");
    }
}
