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
//! The table is `d` to detach, `%` and `"` to split, `o` to move between panes, and
//! `prefix prefix` to type a literal prefix into the pane (tmux's `send-prefix`, which is what
//! keeps the prefix key itself reachable by the program running there). Every one of those is
//! tmux's own spelling, because the point of a prefix table is that a user already has it in their
//! fingers. H2 makes it configurable; until then the choice of `Ctrl-B` is a default, not a
//! decision anyone can change.
//!
//! # Which pane the keys go to is THIS CLIENT's question
//!
//! The daemon has no active-pane concept — that is the same fact that makes tmux's `select-pane`
//! unbuilt here — so focus is client state, and it has to be. Two clients attached to one session
//! are looking at two terminals, and a user typing into the left pane of one has said nothing about
//! where the other's keystrokes should land. What the daemon IS told is the EDGE
//! ([`HostClient::focus`]), because a program that enabled DEC 1004 asked to know when it gained or
//! lost the user's attention, and that is exactly what moving focus here means.
//!
//! # What it deliberately does not do yet
//!
//! * **Latest attach wins the pane's size.** Attaching resizes each pane to the rectangle this
//!   terminal gives it, and so does every later window change. With one client that is simply
//!   correct; with several it is a POLICY, and the same one tmux spells `window-size latest`. The
//!   alternatives tmux also offers (smallest attached client, or a per-client viewport over a
//!   larger pane) need a client-size registry the daemon does not have, and choosing between them
//!   is H2's, not this slice's.
//! * **No mouse, and it is turned OFF rather than left on.** The wire carries a semantic
//!   [`MouseInput`](sprag_input::MouseInput) that the host gates against the pane's tracking mode,
//!   and [`Tiling`](sprag_tui::Tiling) now answers which pane a cell belongs to — so both halves of
//!   a click path exist and neither is wired to the other. Until they are, this client asks termwiz
//!   NOT to enable mouse reporting on the local terminal (see [`local_capabilities`]), because
//!   termwiz's `set_raw_mode` enables it by default, and a client that captures the mouse and then
//!   discards every report has taken click-drag selection and wheel scrolling away from the user's
//!   own terminal emulator in exchange for nothing.
//! * **No pane is closed from here.** `exit` in the shell does it, and the destructive verb is the
//!   one that would want a confirmation prompt this client has nowhere to draw.
//! * **No divider is dragged.** A split opens at an even share and keeps it; resizing one needs a
//!   relative `resize-pane`, which the daemon does not have either.
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
use sprag_terminal::{PaneId, SplitDir};
use sprag_tui::{
    Rect, Tiling, WireKey, cursor_changes, divider_changes, pane_changes, tile, wire_key,
};
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
    // The rectangle every pane's is carved out of. Mutable because a window change replaces it, and
    // kept as ONE value rather than a pair so that every reader of "how big is the screen" — the
    // layouter, the surface, the pane resizes — is reading the same fact.
    let mut screen_area = {
        let (cols, rows) = screen_size(&mut terminal)?;
        Rect::screen(cols, rows)
    };

    // The two edges the client is woken by, each a flag plus a wake of the one blocking poll.
    // The flags carry WHICH edge fired; the wake only says that one did.
    let repaint = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let waker = terminal.waker();

    let host = WireHost::spawn_or_attach(
        // No argv: the host's own `$SHELL`, the same default `sprag attach` gives the GUI.
        None,
        screen_area.cols,
        screen_area.rows,
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
    screen.resize(usize::from(screen_area.cols), usize::from(screen_area.rows));

    // Which pane the user is typing into. `None` until the arrangement is read, which is the
    // honest starting value: the client cannot name a pane before it has been told of one.
    let mut focus = None;
    // The panes this client attached to were sized by whoever created them, which is this client
    // only when it created the session too. Matching each to the rectangle it was given HERE —
    // before the first paint, through the same call a window change uses — is what makes an attach
    // over ssh show panes shaped like the window they are being shown in.
    let mut tiling = reconcile(&host, screen_area, &mut focus);

    // The first paint clears, because the surface starts blank but the terminal underneath it does
    // not. Later ones do not need to: the tiling PARTITIONS the screen, so every cell has an author
    // and a repaint cannot leave a hole for the previous frame to show through.
    paint(&mut screen, &host, &tiling, focus, Clear::Yes)?;

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
                Command::ToPane(key) => send_key(&host, focus, &key),
                // A split and a focus move both change what is on screen without the host
                // necessarily waking this loop, so each repaints on the spot rather than waiting
                // for a notification that may only arrive with the new shell's first prompt.
                Command::Split(dir, before) => {
                    if let Some(pane) = focus.and_then(|pane| host.split(pane, dir, before)) {
                        // tmux puts a new pane in the foreground, and so does this: the user asked
                        // for a shell and would otherwise have to ask again to reach it.
                        set_focus(&host, &mut focus, Some(pane));
                    }
                    tiling = reconcile(&host, screen_area, &mut focus);
                    paint(&mut screen, &host, &tiling, focus, Clear::No)?;
                }
                Command::NextPane => {
                    let next = focus.and_then(|pane| tiling.next_after(pane));
                    set_focus(&host, &mut focus, next.or_else(|| tiling.first_pane()));
                    // Only the CURSOR moved, and it is painted from the tiling this loop already
                    // holds — so the repaint is the whole point and the reconcile is not needed.
                    paint(&mut screen, &host, &tiling, focus, Clear::No)?;
                }
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
            Some(InputEvent::Paste(text)) => paste(&host, focus, &text),
            // A window change resizes both ends: the local surface, so the view is not cropped, and
            // every PANE, so the programs inside them reflow into their new rectangles. Clearing is
            // what keeps a shrunken screen honest — a partition of the OLD size says nothing about
            // cells the new one does not have.
            Some(InputEvent::Resized { .. }) => {
                // Re-read through `screen_size` rather than trusting the event's payload or
                // `BufferedTerminal::check_for_resize`: both take the terminal's raw answer, so a
                // terminal that reports 0 would undo the boot fallback and leave a 0x0 surface.
                let (cols, rows) = screen_size(screen.terminal())?;
                screen_area = Rect::screen(cols, rows);
                screen.resize(usize::from(cols), usize::from(rows));
                tiling = reconcile(&host, screen_area, &mut focus);
                paint(&mut screen, &host, &tiling, focus, Clear::Yes)?;
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
            // Reconciled, not merely repainted: the host's notification covers the ARRANGEMENT as
            // well as the cells, so a split made from another client — or a pane whose shell just
            // exited and was closed — changes which rectangles exist. Painting the old tiling would
            // put the new pane nowhere and leave the closed one's cells on screen.
            tiling = reconcile(&host, screen_area, &mut focus);
            paint(&mut screen, &host, &tiling, focus, Clear::No)?;
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

/// Paint every tiled pane, the lines between them, and the focused pane's cursor onto `screen`,
/// then flush the difference to the terminal.
///
/// **The cursor is emitted LAST and by ONE pane**, which is not an ordering preference: a terminal
/// has a single cursor, [`Change::Text`] moves it as it writes, and the pane that should own it is
/// the one the user is typing into rather than the one that happened to paint last. The other panes
/// show no cursor at all — the same thing tmux does, and what makes the focused pane identifiable
/// without a coloured border.
///
/// Reading the cells every frame rather than caching them is deliberate and free: a live
/// [`HostClient::pane_cells`] reads [`WireHost`](sprag_client::WireHost)'s poll-maintained cache
/// with no socket call.
fn paint(
    screen: &mut BufferedTerminal<SystemTerminal>,
    host: &WireHost,
    tiling: &Tiling,
    focus: Option<PaneId>,
    clear: Clear,
) -> Result<(), Box<dyn Error>> {
    if tiling.panes.is_empty() {
        // No panes is a legitimate transient state (the last one just closed), not an error: the
        // host will either grow one or go away, and both wake this loop. The last frame stays on
        // screen rather than being blanked, because a user whose shell just exited is owed the
        // output it exited with.
        return Ok(());
    }
    if clear == Clear::Yes {
        screen.add_change(Change::ClearScreen(ColorAttribute::Default));
    }
    let mut cursor = Vec::new();
    for held in &tiling.panes {
        let cells = host.pane_cells(held.pane, 0);
        screen.add_changes(pane_changes(&cells, held.area));
        if focus == Some(held.pane) {
            cursor = cursor_changes(&cells, held.area);
        }
    }
    for divider in &tiling.dividers {
        screen.add_changes(divider_changes(divider));
    }
    screen.add_changes(cursor);
    screen.flush()?;
    Ok(())
}

/// Lay the host's arrangement out over `area`, keep `focus` on a pane that is actually shown, and
/// match every pane's PTY to the rectangle it was given.
///
/// The three belong together because each depends on the tiling the other two would otherwise
/// recompute — and because getting them out of step is what a partial update looks like: a focus on
/// a pane that no longer has a rectangle sends keys into a program nobody can see, and a pane whose
/// PTY still holds the old rectangle's size reflows to the wrong width.
fn reconcile(host: &WireHost, area: Rect, focus: &mut Option<PaneId>) -> Tiling {
    let tiling = tile(&host.layout().tree, area);
    // Keep the pane the user chose if it is still shown; fall back to the first otherwise. The
    // fallback is reached by a pane exiting, by another client closing one, and by this terminal
    // shrinking below what the arrangement needs — all of which leave a focus naming nothing.
    let held = focus.filter(|pane| tiling.area_of(*pane).is_some());
    set_focus(host, focus, held.or_else(|| tiling.first_pane()));
    for pane in &tiling.panes {
        resize_pane(host, pane.pane, pane.area);
    }
    tiling
}

/// Move focus to `next`, telling the panes on both ends of the move.
///
/// The host is told because a child that enabled DEC 1004 asked to be: an editor that reloads a
/// changed file when it regains attention is reacting to exactly this edge, and a client that
/// moved focus silently would leave it reacting to nothing. A no-op when focus does not move, so
/// the callers can be blunt about calling it.
fn set_focus(host: &WireHost, focus: &mut Option<PaneId>, next: Option<PaneId>) {
    if *focus == next {
        return;
    }
    // The leaving edge first, so no program is ever told it has focus while another still believes
    // it does. A refused edge (the pane is gone, 1004 is off) is not this client's to report.
    if let Some(leaving) = *focus {
        let _ = host.focus(leaving, false);
    }
    if let Some(arriving) = next {
        let _ = host.focus(arriving, true);
    }
    *focus = next;
}

/// Send one decoded key to the focused pane.
///
/// A key the host declines is logged, not surfaced: the only place this client could report it is
/// the screen it is painting a pane onto, and a viewer that scribbled diagnostics over a user's
/// program would be worse than the dropped key. `false` covers a key `sprag-input` has no encoding
/// for (F13 upward) and a pane that closed between the poll and the send — neither is this
/// client's to fix.
fn send_key(host: &WireHost, focus: Option<PaneId>, key: &WireKey) {
    let Some(pane) = focus else {
        return;
    };
    let mut scratch = [0u8; 4];
    let name = key.name(&mut scratch);
    if !host.send_key(pane, name, key.mods()) {
        tracing::debug!(target: "sprag_tui::input", key = name, "the host did not encode this key");
    }
}

/// Forward pasted text to the focused pane, letting the host decide whether it is bracketed.
fn paste(host: &WireHost, focus: Option<PaneId>, text: &str) {
    let Some(pane) = focus else {
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

/// Resize `pane` to the rectangle the layouter gave it — the reflow the program inside it sees.
///
/// The rectangle, not the terminal: with more than one pane on screen those are different numbers,
/// and a program told it has the whole window would wrap its lines at a column the user cannot see.
///
/// The no-op guard reads [`WireHost`]'s poll-maintained cache rather than the socket, so the common
/// case (a repaint that did not move any boundary, which is every one of them in the steady state)
/// costs no RPC and no reflow. It is a guard against WORK, not against correctness: `RESIZE_ACTION`
/// is idempotent, and the layouter never hands out an empty rectangle for the host to refuse.
fn resize_pane(host: &WireHost, pane: PaneId, area: Rect) {
    if host.pane_grid_size(pane) == (area.cols, area.rows) {
        return;
    }
    host.resize(pane, area.cols, area.rows, CELL_PX_UNKNOWN);
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
    /// Divide the focused pane and put a new shell in the half it opens. The `bool` is tmux's
    /// `-b`: put it on the near side (left of, or above) instead of the far one.
    Split(SplitDir, bool),
    /// Move focus to the next pane in paint order.
    NextPane,
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
            // Every binding is the BARE letter or symbol: a modifier on a command key means the
            // user's finger slipped, and tmux's own table reads the same way. `Ctrl-D` in
            // particular is a program's end-of-file and must never be a detach.
            if is_prefix(event) {
                // `prefix prefix` types a literal prefix — tmux's `send-prefix`, and what keeps
                // `Ctrl-B` reachable by the program running in the pane.
                return wire_key(event).map_or(Command::Swallow, Command::ToPane);
            }
            if event.modifiers.intersects(Modifiers::CTRL | Modifiers::ALT) {
                return Command::Swallow;
            }
            match event.key {
                KeyCode::Char('d') => Command::Detach,
                // tmux's two split keys, and its inversion with them: `%` runs `split-window -h`,
                // which lays the panes side by SIDE. The flag names the layout, not the line drawn
                // between them, and this is the one place in the client where the two could be
                // confused — so the mapping is spelled against tmux's verb rather than against
                // what the divider looks like.
                KeyCode::Char('%') => Command::Split(SplitDir::Horizontal, false),
                KeyCode::Char('"') => Command::Split(SplitDir::Vertical, false),
                KeyCode::Char('o') => Command::NextPane,
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

    /// **THE INVERSION, pinned at the keyboard.** tmux's `%` is `split-window -h`, which lays the
    /// panes side by SIDE — so it must reach the wire as `Horizontal`, and `"` as `Vertical`.
    ///
    /// Asserted as a pair in one test because the failure this guards against is the two being
    /// SWAPPED, which either assertion alone would let through: a client that mapped both keys to
    /// the same direction, or exchanged them, still splits and still shows two panes. R227 recorded
    /// exactly this — a test that ran each form and counted panes would have passed a CLI that
    /// mapped `-v` to horizontal.
    #[test]
    fn the_two_split_keys_carry_tmuxs_directions_and_not_each_others() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(
            command(&mut keys, &typed(b"%")),
            Command::Split(SplitDir::Horizontal, false),
        );
        assert_eq!(keys, Keys::ToPane, "and the mode is one key long");
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(
            command(&mut keys, &typed(b"\"")),
            Command::Split(SplitDir::Vertical, false),
        );
    }

    /// `prefix o` moves to the next pane — tmux's `select-pane -t :.+`, and the only way to reach a
    /// pane this client has just made.
    #[test]
    fn the_prefix_then_o_moves_to_the_next_pane() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        assert_eq!(command(&mut keys, &typed(b"o")), Command::NextPane);
    }

    /// The split keys are the client's only BEHIND the prefix. Typed bare they are ordinary
    /// characters, and they are characters a shell sees constantly — `%` in a prompt, `"` around
    /// every quoted string.
    ///
    /// REVERT-PROOF for the prefix gate itself: route these without it and typing a quoted argument
    /// would split the window mid-word.
    #[test]
    fn the_split_keys_are_ordinary_characters_without_the_prefix() {
        let mut keys = Keys::ToPane;
        assert_eq!(routed(&mut keys, b"%").as_deref(), Some("%"));
        assert_eq!(routed(&mut keys, b"\"").as_deref(), Some("\""));
        assert_eq!(routed(&mut keys, b"o").as_deref(), Some("o"));
    }

    /// A command key with a modifier on it is a slip, not a command — the rule `Ctrl-D` already
    /// forced, applied to the whole table rather than to the one binding that noticed it.
    ///
    /// `Ctrl-O` is the case that makes it more than tidiness: it is readline's `operate-and-get-
    /// next`, so a user running through a history with it would find their focus moving instead.
    #[test]
    fn a_modified_command_key_is_swallowed() {
        let mut keys = Keys::ToPane;
        assert_eq!(command(&mut keys, &typed(CTRL_B)), Command::Swallow);
        // Ctrl-O, the C0 byte.
        assert_eq!(command(&mut keys, &typed(&[0x0f])), Command::Swallow);
        assert_eq!(keys, Keys::ToPane);
    }
}
