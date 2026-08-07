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
//! # Keys belong to the pane, so the client needs a prefix — and the prefix is the USER's
//!
//! Once keystrokes reach the child, every key is spoken for: `q` is a program's quit, `Ctrl-C` is
//! a program's interrupt, and raw mode means the client cannot fall back on a signal either. So
//! this client's own commands live behind a PREFIX, which is tmux's answer and the one a user
//! already has in their fingers — the prefix, then a command key.
//!
//! Both halves are read from the user's [`Keymap`] rather than written here. `Ctrl-B` and the table
//! `d` / `%` / `"` / `o` are still what a user who has said nothing gets, because those are tmux's
//! own defaults — but they are now [`Keymap::default`]'s, layered over by `config.toml`, and this
//! binary spells none of them.
//!
//! The keymap is loaded FIRST, before the daemon is reached and long before the terminal is taken:
//! a config with a typo in it is a message a user has to be able to read, and the only screen that
//! can show one is the one this client has not yet replaced.
//!
//! It is then re-read WHENEVER THE FILE MOVES ([`refreshed`]), which is what makes `sprag bind-key`
//! a runtime command and hands `source-file` to anyone who edits their config in an editor. The
//! file is the live table rather than some runtime copy of it, because `sprag list-keys` reads that
//! file with no daemon — a binding living anywhere else would make that verb print a table nobody
//! is using.
//!
//! Exact-modifier matching is what makes the table a table. The hardcoded version needed a rule of
//! its own — "a command key with a modifier on it is a slip" — so that `Ctrl-D` could not detach and
//! `Ctrl-O` could not move focus; a keymap gets that for free, because `Ctrl-D` is simply not the
//! key `d` is bound to. It also makes `C-o` bindable, which the special case could not express.
//!
//! # Which pane the keys go to is the SESSION's answer, projected here
//!
//! This used to be client state on the argument that two attached terminals are two independent
//! views. tmux says otherwise and so does the rest of this client: the current WINDOW is session
//! state and every attached client already follows it, so keeping the current PANE private made one
//! fact have two authorities — and left nothing that draws no pixels, an agent or a shell running
//! `sprag`, able to say "here" at all.
//!
//! So the daemon holds it ([`HostClient::active_pane`]), this client PROJECTS it, and a move the
//! user makes here is published ([`HostClient::select_pane`]). Two facts stay separate underneath:
//! the daemon is also told the focus EDGE ([`HostClient::focus`]), because a program that enabled
//! DEC 1004 asked to know when it gained or lost the user's attention — that is about a client's
//! attention, this is about where the session is.
//!
//! The one place they part company is a pane this terminal cannot SHOW: a floated pane has no leaf
//! in the arrangement a terminal client tiles. Then the cursor falls back locally and publishes
//! nothing, so the GUI showing that pane keeps the session where the user put it.
//!
//! # The pointer goes where it IS, and the terminal reports it only when a child asked
//!
//! A keystroke belongs to the focused pane; a mouse report belongs to the pane under the pointer,
//! because that is the only reading a program can make sense of. A press also MOVES the focus
//! there, so the two never drift into two answers to "where am I". The cell is translated into the
//! pane's own coordinates on the way ([`Tiling::pane_at`](sprag_tui::Tiling::pane_at)), since a
//! child knows only its own grid.
//!
//! Whether this terminal reports the mouse at all is not this client's preference — it is a MIRROR
//! of what the panes' children have asked for ([`MouseMirror`]). Capturing the pointer takes
//! click-drag selection and wheel scrolling away from the user's own emulator, which is a cost
//! worth paying exactly when there is a program to hand the reports to, and never otherwise.
//!
//! A press on a DIVIDER is the client's own gesture rather than any child's: it claims the drag,
//! and the moves that follow rewrite that split's ratio on the host. Claimed on the press because
//! the pointer leaves the divider the instant it starts moving — recognising the line on every
//! event instead would resize once and then swallow every click that followed.
//!
//! # The pane's size is the SESSION's, not this terminal's
//!
//! This client reports its own area to the daemon ([`report_size`]) and lays the arrangement out
//! over the WINDOW the daemon arbitrates from every attached client's report
//! ([`window_area`], tmux's `window-size`) — not over its own screen. With one client the two are
//! the same rectangle and nothing has changed. With several they need not be, and that is the
//! point: one pane cannot have two sizes.
//!
//! So a terminal larger than the window paints into part of itself, and one smaller shows the part
//! that fits ([`Rect::intersect`], applied at PAINT time only — the pane is resized to its share of
//! the window regardless of what this terminal can display). The window change path still reports
//! first and tiles second, because under the default `latest` policy this terminal's new area IS
//! the new window.
//!
//! # What it deliberately does not do yet
//!
//! * **No pane is closed from here.** `exit` in the shell does it, and the destructive verb is the
//!   one that would want a confirmation prompt this client has nowhere to draw.
//! * **Type-ahead before the client is up is lost.** `set_raw_mode` sets the termios with
//!   `TCSAFLUSH`, which purges whatever was typed before the client got there. That is what every
//!   full-screen program does and it is not this client's to change, but it is a real thing a user
//!   can see: characters typed into `ssh host sprag attach --tui` while it is still connecting do
//!   not arrive.

use std::error::Error;
use std::fmt::Write as _;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pinion_core::QuitSink;
use sprag_client::WireHost;
use sprag_host::HostClient;
use sprag_host::chooser::Pick;
use sprag_host::keyhelp::{KeyHelp, Pressed, Scroll};
use sprag_host::keymap::{BoundAction, Keymap, PrefixMode, Routed, SwitchClientAsk};
use sprag_host::prompt::{Ask, Line, Subject, Typed};
use sprag_host::report::{Message, Report, display_time, now};
use sprag_host::status::Status;
use sprag_host::wire::SelectWindowAsk;
use sprag_input::{Modifiers, MouseEventKind, MouseInput};
use sprag_terminal::{Ended, PaneId, PlaceHow, SplitId};
use sprag_tui::focus::{self, Person};
use sprag_tui::outward::Outward;
use sprag_tui::{
    Divider, MouseEdges, PaintCache, PanePaint, Rect, Split, Tiling, WireKey, agent_window_title,
    chooser_changes, cursor_changes, divider_changes, help_changes, help_viewport, prompt_changes,
    status_changes, tile, title_change, wire_key, with_ratio,
};
use sprag_vt::MouseProtocol;
use termwiz::caps::{Capabilities, ProbeHints};
use termwiz::color::ColorAttribute;
use termwiz::escape::csi::{CSI, DecPrivateMode, DecPrivateModeCode, Mode};
use termwiz::input::{InputEvent, KeyEvent};
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
    // FIRST, before the daemon is reached and long before the terminal is taken. A config error is
    // a message the user has to read, and every later step either replaces the screen it would be
    // printed on or gives them something else to think about. It also costs one file read, so
    // there is nothing to gain by deferring it.
    let mut keymap = sprag_host::config::ClientConfig::load()?;
    // This client OPENS the controlling terminal rather than letting termwiz do it, and keeps the
    // handle: the mouse modes have to be turned on and off as the panes' children ask for them
    // ([`MouseMirror`]), and `Terminal` offers no way to say so — `set_raw_mode` decides once, from
    // the capabilities, and `Change::Text` renders control characters inert by contract. The one
    // seam that does exist is `new_with`, which is documented for exactly this. `/dev/tty` is the
    // same file `SystemTerminal::new` would have opened, so nothing about which terminal is taken
    // changes — including the failure when there is no controlling terminal to take.
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    let mut terminal = SystemTerminal::new_with(local_capabilities()?, &tty, &tty)?;
    // A SECOND handle on the same terminal, for the OTHER conversation this client has with it: the
    // mouse mirror owns the modes, this one carries a notification out to the person (R319). Two
    // handles rather than one shared writer because they are two concerns with two lifetimes, and it
    // is sound here for a reason rather than by luck — this client is single-threaded, so the two
    // never write at once, and each sequence goes out as one `write_all`.
    let mut outward_tty = tty.try_clone()?;
    let mut mouse = MouseMirror::new(tty);
    // The terminal, cut into the rectangle every pane's is carved out of and the row this client
    // speaks in. Mutable because a window change replaces it, and kept as ONE value rather than a
    // pair so that every reader of "how big is the screen" — the layouter, the surface, the pane
    // resizes, the size REPORTED to the daemon — is reading the same fact. That last one is why the
    // cut is a type: reporting the whole terminal while tiling one row less would arbitrate a
    // window a row taller than any pane can ever be given.
    let mut split = {
        let (cols, rows) = screen_size(&mut terminal)?;
        Split::of(cols, rows)
    };

    // The two edges the client is woken by, each a flag plus a wake of the one blocking poll.
    // The flags carry WHICH edge fired; the wake only says that one did.
    let repaint = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let waker = terminal.waker();

    let host = WireHost::spawn_or_attach(
        // No argv: the host's own `$SHELL`, the same default `sprag attach` gives the GUI.
        None,
        split.panes.cols,
        split.panes.rows,
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

    // WHERE A MESSAGE GOES WHEN THE PERSON IS NOT HERE (R319). This holds only what cannot change
    // under a running client — what its terminal IS — because the policy is re-read from the user's
    // file on every keystroke and the session this client is viewing can change without it exiting.
    let mut outward = Outward::of(|name| std::env::var(name).ok());
    // Where the person is, or `None` while this client is not asking its terminal to say — which is
    // NOT the same as their being here, and is why this is an `Option` rather than a `Person` with a
    // third arm. `Outward::follow` owns this and the mode together, so the two cannot disagree.
    let mut person: Option<Person> = None;

    // Only now is the terminal taken. The hook goes in FIRST so that a panic between here and the
    // end of the loop still leaves a usable shell behind.
    install_restore_hook();
    terminal.set_raw_mode()?;
    terminal.enter_alternate_screen()?;
    // AFTER `set_raw_mode`, which calls `tcsetattr` with `TCSAFLUSH` and purges the input queue: a
    // report answering a mode asked for before that would be discarded, and the first thing this
    // client would learn about the person is a change from a state it never saw.
    outward.follow(keymap.options(), &mut person, &mut outward_tty);
    let mut screen = BufferedTerminal::new(terminal)?;
    // `BufferedTerminal::new` sizes its surface from the terminal's raw answer, so the fallback
    // has to be applied here too or a terminal that reports nothing paints into a 0x0 surface.
    //
    // **THE WHOLE TERMINAL, not the pane rectangle**, and a live test caught the difference: a
    // surface one row short CLAMPS the status row's absolute cursor move, so the row painted a line
    // above where it belongs and the terminal's real bottom row was never written. The surface is
    // what this client can draw ON; `split.panes` is what it gives the panes.
    screen.resize(
        usize::from(split.terminal().cols),
        usize::from(split.terminal().rows),
    );

    // Which pane the user is typing into. `None` until the arrangement is read, which is the
    // honest starting value: the client cannot name a pane before it has been told of one.
    // This client's focus, and the DAEMON's answer as this client last saw it. The second is what
    // makes following an EDGE rather than a level: the pane mirror lags a moment behind a publish,
    // so a client that adopted it unconditionally would yank the cursor back to where the user was
    // before their own keypress. Compared against, it moves focus exactly when the daemon's answer
    // CHANGES — another client selecting, a close handing off, this client's own move landing —
    // and says nothing while the mirror is merely catching up.
    let mut focus = None;
    let mut seen_active = None;
    // The panes this client attached to were sized by whoever created them, which is this client
    // only when it created the session too. Matching each to the rectangle it was given HERE —
    // before the first paint, through the same call a window change uses — is what makes an attach
    // over ssh show panes shaped like the window they are being shown in.
    // BEFORE the first reconcile: the arbitration cannot include a client that has not spoken, so
    // reporting after it would tile the first frame over a window this terminal was not counted in.
    report_size(&host, split.panes);
    let mut tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
    mouse.follow(&host, &tiling);
    // The pointer's state, kept across events because the wire wants EDGES and this terminal
    // reports a state (see [`MouseEdges`]).
    let mut pointer = MouseEdges::default();
    // The divider a press claimed, held until the release that ends the drag. `None` is the steady
    // state: the pointer is over panes, not between them.
    let mut dragging: Option<(SplitId, Divider)> = None;

    // What this client has already put on the terminal, so a frame writes only what differs from it.
    // The first paint finds it empty and writes everything, which is also what the `Clear::Yes` below
    // means — see [`Painted`].
    let mut held = Painted::default();

    // The first paint clears, because the surface starts blank but the terminal underneath it does
    // not. Later ones do not need to: the tiling PARTITIONS the screen, so every cell has an author
    // and a repaint cannot leave a hole for the previous frame to show through.
    paint(
        &mut screen,
        &host,
        &tiling,
        split.panes,
        Frame {
            focus,
            clear: Clear::Yes,
            // Nothing is over the panes at boot, and the state that would say so is
            // declared below — where it belongs, one line before the loop that owns it.
            overlay: &Overlay::None,
            status: split.status,
            // Nothing has been pressed yet, so the row says where this client is — which is the
            // first thing a person attaching over ssh wants to know anyway.
            message: None,
        },
        &mut held,
    )?;

    // Where the next key goes. Starts at the pane: the prefix is a departure from the steady
    // state, not the other way round.
    let mut keys = PrefixMode::ToPane;
    // What this client has put over the panes — see [`Overlay`]. `None` is the steady state, and
    // while it is anything else every keystroke belongs to that surface rather than to the pane.
    let mut overlay = Overlay::None;
    // WHAT THE LAST KEY DID, while it is still worth reading — see [`sprag_host::report::Message`].
    // `None` is the steady state, and the status row then says where this client is instead.
    let mut message: Option<Message> = None;
    // The one event this loop read AHEAD of itself and has not routed yet — see [`read_input`]. Empty
    // in the steady state: only a possible focus report puts anything here, and only for the one turn
    // it takes to find out that it was not one.
    let mut pending: Option<InputEvent> = None;
    loop {
        // The poll blocks until the terminal has something OR the waker fires — the select this
        // client's whole idle cost rests on. It blocks FOREVER unless a message is up, in which
        // case it blocks only until that message expires: the row has to clear on its own deadline
        // rather than at the next keystroke, and a timeout that exists only while a sentence is on
        // screen leaves the idle cost exactly where it was. This is the one timer in this loop and
        // it is bounded by `display-time`.
        // `None` for a message with no deadline (a `Severity::Alert`, R317) as well as for no
        // message at all — and the two are the same instruction to this poll: block forever. An
        // alert is cleared by the person, in the key arm below, so there is nothing here to wake up
        // for; giving it a timeout would be a client waking to re-decide that it should still be
        // showing what it is showing.
        let waiting = message
            .as_ref()
            .and_then(|said| said.until())
            .map(|until| until.saturating_sub(now()));
        match read_input(&mut screen, waiting, &mut pending, person.as_mut())? {
            // The message's own deadline came and went with nothing else happening: take the row
            // back. `poll_input` answers `None` on a timeout, which is the same answer it gives for
            // a spurious wake — so the state is re-derived rather than assumed, and a `None` with
            // no message up costs one comparison.
            Input::Nothing
                if message
                    .as_ref()
                    .is_some_and(|said| said.showing(now()).is_none()) =>
            {
                message = None;
                paint_status(&mut screen, &host, split.status, None)?;
            }
            // The person left this terminal, or came back to it. Nothing to route and nothing to
            // repaint: what it changes is where the NEXT message goes, and it deliberately does not
            // touch the one that is up — an alert waits for a KEY because a key is what proves
            // somebody read it, and a window regaining focus proves only that it has focus.
            Input::Focus => {}
            // The table is re-read HERE, and only here, because this is the one moment its answer
            // is used: a repaint cannot change what a key means. Routed in a `let` before the match
            // so the borrow the re-read takes ends before an arm reads the prefix back out.
            Input::Event(InputEvent::Key(event)) => {
                // A message that waits to be ACKNOWLEDGED is cleared by this keystroke (R317). It
                // is the only thing that can clear one — an alert has no deadline precisely because
                // a timer is a bet that somebody is looking, and a key is the one event that proves
                // somebody is.
                //
                // ⚠ IT REPAINTS THE ROW ITSELF, and a live test is what settled that: the first
                // draft cleared the state with a comment claiming every path out of this arm paints
                // anyway. It does not — a key bound to nothing is SWALLOWED, and a report with
                // nothing to say paints nothing — so the sentence stayed on the terminal after the
                // client had forgotten it. The one keystroke that must clear the row is exactly the
                // one least likely to be doing anything else.
                if message
                    .as_ref()
                    .is_some_and(Message::waits_to_be_acknowledged)
                {
                    message = None;
                    paint_status(&mut screen, &host, split.status, None)?;
                }
                // AN OVERLAY OWNS THE KEYBOARD while it is up, and this is checked before the key
                // is looked at rather than after: a user answering a question — or reading the key
                // table — is not addressing the keymap, the prefix or the pane, and an unhandled
                // key is swallowed rather than leaked to a shell behind a surface the user has not
                // finished with. `sprag-gui` states the same rule at the top of its own routing.
                //
                // TAKEN rather than borrowed, and put back only if it is still up: that is what
                // lets an arm close the overlay while it is still holding the value, and it says in
                // the types that closing is the default — a path that forgets to restore it gives
                // the panes back rather than stranding the keyboard.
                let command = match std::mem::replace(&mut overlay, Overlay::None) {
                    Overlay::Asking(mut open) => match open.answered(&host, &event) {
                        // Still asking: only the row changed, so only the row is painted.
                        Answered::Asking => {
                            paint_prompt(&mut screen, split.panes, &open)?;
                            overlay = Overlay::Asking(open);
                            continue;
                        }
                        // Closed with nothing to do — repaint the FRAME, which is what puts the
                        // panes back under the row the overlay borrowed.
                        Answered::Closed => {
                            paint(
                                &mut screen,
                                &host,
                                &tiling,
                                split.panes,
                                Frame {
                                    focus,
                                    clear: Clear::Yes,
                                    overlay: &overlay,
                                    status: split.status,
                                    message: showing(&message).as_deref(),
                                },
                                &mut held,
                            )?;
                            continue;
                        }
                        // Answered yes: the row is given back first, then the guarded action runs
                        // through the very same arms a bare binding reaches.
                        Answered::Perform(action) => {
                            paint(
                                &mut screen,
                                &host,
                                &tiling,
                                split.panes,
                                Frame {
                                    focus,
                                    clear: Clear::Yes,
                                    overlay: &overlay,
                                    status: split.status,
                                    message: showing(&message).as_deref(),
                                },
                                &mut held,
                            )?;
                            Command::Act(action)
                        }
                    },
                    // The help view answers every key itself — scroll, or leave. What each key MEANS
                    // is `KeyHelp::pressed`'s, shared with the other frontend; what this arm decides
                    // is only what to repaint, which is the surface's half of that split.
                    Overlay::Showing(mut open) => {
                        if open.pressed(&event, help_viewport(split.panes)) == Shown::Open {
                            paint_help(&mut screen, split.panes, &open)?;
                            overlay = Overlay::Showing(open);
                        } else {
                            paint(
                                &mut screen,
                                &host,
                                &tiling,
                                split.panes,
                                Frame {
                                    focus,
                                    clear: Clear::Yes,
                                    overlay: &overlay,
                                    status: split.status,
                                    message: showing(&message).as_deref(),
                                },
                                &mut held,
                            )?;
                        }
                        continue;
                    }
                    Overlay::None => command(&mut keys, refreshed(&mut keymap), &event),
                };
                // The keystroke above RE-READ the user's file, so this is where a changed
                // `notify-outward` takes effect — the same edge that makes an edited BINDING live,
                // and the only one that can: a person editing their config is a person typing.
                outward.follow(keymap.options(), &mut person, &mut outward_tty);
                // An action that cannot be carried out without an ANSWER opens the prompt instead
                // of acting. Asked here for EVERY action rather than per arm, so the decision is
                // `Ask::of`'s alone — the same discipline `Routed::next` applies to the prefix
                // mode, and for the same reason: two frontends cannot each hold half a rule.
                if let Command::Act(action) = &command
                    && let Some(ask) = Ask::of(action, &host, focus)
                {
                    let open = Asking::open(ask);
                    paint_prompt(&mut screen, split.panes, &open)?;
                    overlay = Overlay::Asking(open);
                    continue;
                }
                // EVERY ARM ANSWERS. The match is an expression whose value is what this
                // keystroke DID, so an action that changed nothing cannot leave without saying so —
                // the defect R316 measured (a key bound to a session that does not exist left a
                // live client's screen byte-for-byte unchanged) is unrepresentable here, because
                // there is no arm that can simply return.
                let report: Report = match command {
                    Command::Act(BoundAction::DetachClient) => break,
                    // THE VIEW IS BUILT FROM THE TABLE IN FORCE, at the instant it opens: the same
                    // `refreshed` re-read the keystroke above went through, so a user who edits
                    // their config and presses `?` is shown what they just wrote. It is then a
                    // photograph — see `KeyHelp` — because a table that changed while it was on
                    // screen would scroll under the reader.
                    Command::Act(BoundAction::ListKeys) => {
                        let open = Showing::open(refreshed(&mut keymap));
                        paint_help(&mut screen, split.panes, &open)?;
                        overlay = Overlay::Showing(open);
                        continue;
                    }
                    // The prefix itself, an unbound key, a key the wire cannot spell: each is a
                    // keystroke that was never a command, so there is no outcome to report. Not
                    // silence by omission — the arm SAYS so, which is the whole difference.
                    Command::Swallow => Report::on_screen(),
                    // A key that reached the pane is answered by the pane: whatever the program
                    // inside it does about the keystroke is what appears, and a client sentence
                    // over the top would be this client talking about somebody else's program.
                    Command::ToPane(key) => {
                        let mut scratch = [0u8; 4];
                        send_key(&host, focus, key.name(&mut scratch), key.mods());
                        Report::on_screen()
                    }
                    // The PREFIX, not the key that was pressed: a user who binds `send-prefix` to some
                    // other key means that key to send the prefix, not to send itself.
                    Command::Act(BoundAction::SendPrefix) => {
                        let prefix = keymap.keymap().prefix();
                        send_key(&host, focus, prefix.name(), prefix.mods());
                        // The pane answers, exactly as it does for `ToPane` above.
                        Report::on_screen()
                    }
                    // A split and a focus move both change what is on screen without the host
                    // necessarily waking this loop, so each repaints on the spot rather than waiting
                    // for a notification that may only arrive with the new shell's first prompt.
                    Command::Act(BoundAction::SplitWindow { dir, before }) => {
                        if let Some(pane) = focus.and_then(|pane| host.split(pane, dir, before)) {
                            // The DAEMON has already made the new pane active (tmux's rule, applied
                            // where the split happens), so this only moves the cursor to where the
                            // session already is — locally, with no publish to race the next one.
                            set_focus(&host, &mut focus, Some(pane));
                        }
                        // A pane either appeared or it did not, and either way the screen below
                        // says which — this arm's own repaint is the answer.
                        let report = Report::on_screen();
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // A zoom changes what this client DRAWS without changing the pane set, so it
                    // needs the same on-the-spot reconcile a split does: the projection the next
                    // paint tiles is the arrangement filtered by the zoomed pane, and waiting for a
                    // host notification would leave the user's own keystroke unanswered until
                    // something else moved. `Clear::No` for the standing reason — a projection
                    // partitions the screen exactly as an arrangement does, so every cell still has
                    // an author.
                    Command::Act(BoundAction::ZoomPane { on }) => {
                        // A zoom is a change to what is DRAWN, so the drawing is the report — for
                        // the three outcomes that HAVE a drawing. [`None`] is the fourth: the
                        // daemon REFUSED, because the pane is floating and a floating pane is in no
                        // arrangement to fill a window with. That case draws nothing at all, which
                        // is the shape this round exists to remove — and the answer was dropped
                        // here until the `#[must_use]` sweep asked what read it.
                        //
                        // `changed: false` is NOT a refusal: it is a zoom re-asserting the state
                        // already in force, which is a well-formed request whose drawing is
                        // already on the screen.
                        let report = match focus.map(|pane| host.zoom_pane(pane, on)) {
                            Some(None) => Report::nowhere(&BoundAction::ZoomPane { on }),
                            Some(Some(_)) | None => Report::on_screen(),
                        };
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // THE WINDOW LEVEL, reached from a key (R305), and the pane KILL that can
                    // reach it (R309). All four arms share one shape and one repaint, because all
                    // four can change WHICH WINDOW this client projects: the pane set is replaced
                    // wholesale, so the screen is cleared rather than differenced — every cell has
                    // a new author, which is the one case `Clear::No` (a projection partitioning
                    // the same window) does not cover. `kill-pane` belongs here rather than beside
                    // the zoom precisely because it MAY do that: closing a window's last pane ends
                    // the window, and the client is then projecting a different one.
                    //
                    // None of them publishes a focus: the daemon selects the window it created or
                    // walked to, and `reconcile` follows the session's active pane onto this
                    // client's ring on the next pass — one authority for "which pane the session is
                    // on", the discipline the directional arm below leans on.
                    Command::Act(
                        action @ (BoundAction::NewWindow
                        | BoundAction::SelectWindow { .. }
                        | BoundAction::MoveWindow { .. }
                        | BoundAction::KillPane
                        | BoundAction::KillWindow
                        | BoundAction::BreakPane
                        | BoundAction::KillSession
                        | BoundAction::NewSession
                        | BoundAction::SwitchClient { .. }),
                    ) => {
                        // The GROUP'S OWN REPORT, computed before the repaint below rather than
                        // after it, because two of these six can move this client somewhere that
                        // does not exist — and the repaint would then be painting a screen that
                        // never changed while the sentence explaining why sat unbuilt.
                        let report = match &action {
                            BoundAction::NewWindow => {
                                // A window is born and selected, so the whole screen below is the
                                // answer — and the status row now names it.
                                let _ = host.new_window();
                                Report::on_screen()
                            }
                            // The pane the user is ON, which is the only one a keystroke can mean —
                            // the zoom arm's rule. How far the kill cascaded is the daemon's answer
                            // and this client does not read it: the reconcile below re-derives the
                            // whole projection, which is the honest way for a display client to
                            // learn that its window is gone.
                            BoundAction::KillPane => {
                                if let Some(pane) = focus {
                                    let _ = host.kill_pane(pane);
                                }
                                Report::on_screen()
                            }
                            BoundAction::SelectWindow { ask } => match ask {
                                // THE NAME IS THE HALF THAT CAN BE WRONG. A window called what the
                                // config says may simply not be there, and until R316 the daemon's
                                // refusal stopped at a `()` return.
                                SelectWindowAsk::Named(window) => {
                                    match host.select_window(window) {
                                        Some(_) => Report::on_screen(),
                                        None => Report::no_such(&action),
                                    }
                                }
                                // The step cannot miss: a session always holds a window, so the
                                // walk always lands. `None` is a host that could not be asked.
                                SelectWindowAsk::Step(step) => {
                                    let _ = host.select_window_toward(*step);
                                    Report::on_screen()
                                }
                            },
                            // THE SESSION LEVEL (R314), and the first way this front has ever had
                            // to reach another session — before it, a `sprag-tui` user had to
                            // detach and run `sprag attach` in a shell. It belongs in THIS group
                            // because it changes the whole projection: the reconcile below re-reads
                            // the pane set, the tiling and the focus, which is exactly what a
                            // client that is now looking at a different session needs.
                            //
                            // Nothing here resolves a neighbour. The ring is the daemon's, walked
                            // from this client's own attachment — the authority split the window
                            // arm above states, one level up and with a sharper cost, since a
                            // client's session mirror is refreshed by a poll.
                            //
                            // The ASKING arm is unreachable here: `Ask::of` consumes it above.
                            BoundAction::SwitchClient { ask } => match ask {
                                // A step that landed is shown by the status row naming the session
                                // it landed on; `None` is a ring this client could not be walked
                                // along at all, which nothing else says.
                                SwitchClientAsk::Step(step) => {
                                    match host.switch_session_toward(*step) {
                                        Some(_) => Report::on_screen(),
                                        None => Report::nowhere(&action),
                                    }
                                }
                                // `None` here is the degraded half R304 measured: this client has
                                // viewed nothing else that is still alive, so "take me back" has
                                // nowhere to take anybody.
                                SwitchClientAsk::LastViewed => match host.switch_session_last() {
                                    Some(_) => Report::on_screen(),
                                    None => Report::nowhere(&action),
                                },
                                // **THE MEASURED DEFECT.** `switch-client -t <a name nothing
                                // carries>` was a silent no-op in both frontends: a live client's
                                // screen came back byte-for-byte identical to an UNBOUND key's.
                                SwitchClientAsk::Named(session) => {
                                    match host.switch_session_named(session) {
                                        Some(_) => Report::on_screen(),
                                        None => Report::no_such(&action),
                                    }
                                }
                                // Unreachable: `Ask::of` consumes the asking form above and opens a
                                // chooser with it. Kept as an arm because the vocabulary has one.
                                SwitchClientAsk::Ask => Report::on_screen(),
                            },
                            // NO window named, which is the same rule the kill below breaks
                            // deliberately: the daemon resolves "the one I am on" under its own
                            // lock, where this client's mirror can be a revision behind.
                            //
                            // **The outcome word IS read now, and this front is the reason the
                            // note beside it changed.** It used to say the move "changes nothing
                            // this front draws" because `sprag-tui` painted no window strip — and
                            // this round gave it one, so a reorder moves the status row. What no
                            // repaint can show is a move that did NOT happen: three of
                            // `PlaceHow`'s four words leave the order untouched, and the two that
                            // are a user's mistake — already at that end, or anchored to the
                            // window itself — are exactly the ones a person needs told.
                            BoundAction::MoveWindow { place } => {
                                match host.move_window(None, place) {
                                    Some((_, PlaceHow::Moved)) => Report::on_screen(),
                                    Some((_, _)) | None => Report::nowhere(&action),
                                }
                            }
                            // The CURRENT window, which is the only one a keystroke can mean. Its
                            // name comes off the client's own window mirror, where `current` is the
                            // fact the daemon publishes for exactly this.
                            // ...and it REPORTS THE CASCADE (R325): the session's last window
                            // takes the session, which the confirm prompt only PREDICTS off this
                            // client's mirror. The prompt's own doc says it can over-state; this is
                            // what the daemon actually did.
                            BoundAction::KillWindow => match current_window_name(&host) {
                                Some(window) => match host.kill_window(&window) {
                                    Some(ended) => Report::cascaded(ended, Ended::Window),
                                    None => Report::nowhere(&action),
                                },
                                None => Report::nowhere(&action),
                            },
                            // R323's THREE, and they belong in THIS group for the group's own
                            // reason: each can change which window — or which SESSION — this
                            // client projects, so the pane set is replaced wholesale and the
                            // screen is cleared rather than differenced.
                            //
                            // The pane is the FOCUSED one, which is the only one a keystroke can
                            // mean; `None` is a client with no pane focused, and the daemon is not
                            // asked to break a pane nobody named.
                            BoundAction::BreakPane => match focus.and_then(|pane| {
                                host.break_pane(pane, None)
                            }) {
                                Some(_) => Report::on_screen(),
                                None => Report::nowhere(&action),
                            },
                            // This client's OWN session. What becomes of the client afterwards is
                            // the daemon's `detach-on-destroy` policy, applied by the wire client.
                            BoundAction::KillSession => {
                                match host.kill_session(&host.current_session()) {
                                    // `Ended::Server` is the one a person cannot find out any other
                                    // way: the daemon they were talking to is gone, so there is
                                    // nothing left to re-read. A severed reply answers `None` and
                                    // this client is leaving anyway.
                                    Some(ended) => Report::cascaded(ended, Ended::Session),
                                    None => Report::on_screen(),
                                }
                            }
                            // CREATE AND FOLLOW — see the arm's own doc for why it is two acts.
                            // The name read BEFORE is the only way to tell a birth from a refusal:
                            // `new_session` answers the current session's name when the daemon
                            // would not make one, which is R316's defect waiting to be rebuilt.
                            BoundAction::NewSession => {
                                let before = host.current_session();
                                if host.new_session() == before {
                                    Report::nowhere(&action)
                                } else {
                                    Report::on_screen()
                                }
                            }
                            _ => unreachable!("the match above admits only the nine arms here"),
                        };
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::Yes,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // A DIRECTIONAL move is the daemon's to resolve, so this arm publishes nothing
                    // and adopts nothing: it asks, and then re-reads through the same [`reconcile`]
                    // that already follows the session's active pane. That is what keeps the one
                    // case where this client's ring is NOT the session's pane — the active pane is
                    // floating, which a terminal cannot show — from being yanked by an answer that
                    // did not move. `SelectNextPane` below cannot share this shape: its target is
                    // this client's own paint order, so the client is the one that knows it.
                    Command::Act(BoundAction::SelectPaneToward { dir }) => {
                        // The daemon's answer is the report: `None` is the EDGE, which no repaint
                        // can show because nothing moved. This is the fact
                        // `HostClient::select_toward`'s own doc said stopped at its signature —
                        // *"the day one wants \"you are at the edge\", this signature is where the
                        // fact stops"* — and it stops here instead now.
                        let report = if host.select_toward(dir) {
                            Report::on_screen()
                        } else {
                            Report::nowhere(&BoundAction::SelectPaneToward { dir })
                        };
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // The SWAP's twin of the arm above, and identical for the same reason: the
                    // daemon resolves the direction and this re-reads. What it does NOT need is a
                    // focus move — the active pane is a PANE, so a user typing into it goes on
                    // typing into it in its new cell, and the reconcile below just re-tiles.
                    Command::Act(BoundAction::SwapPaneToward { dir }) => {
                        // The select's rule one verb over: `false` is the edge, and a swap that
                        // moved nothing looks exactly like a key that is not bound.
                        let report = if host.swap_toward(dir) {
                            Report::on_screen()
                        } else {
                            Report::nowhere(&BoundAction::SwapPaneToward { dir })
                        };
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // THE BOUNDARY (R307), and the same shape a third time. The reconcile is what
                    // makes it visible: a resize changes no pane's identity and no window's pane
                    // set, so the ONLY thing that moved is the rectangle each pane is tiled into —
                    // which is exactly what `reconcile` re-derives. `Clear::No` for the standing
                    // reason: a tiling partitions the screen, so every cell still has an author.
                    //
                    // **This is the arm that makes `sprag-tui` able to resize at all.** Before it
                    // the only gesture in this product that could move a split's share was a
                    // pointer drag in `sprag-gui`, and a terminal client had no pointer to drag
                    // with — so a TUI user's arrangement was whatever the even splits gave them.
                    Command::Act(BoundAction::ResizePaneToward { dir, cells }) => {
                        // The same rule a third time. `false` here is a boundary that had nowhere
                        // to go — the window's own edge, or a lone pane with no boundary at all —
                        // and it is the arm a user is likeliest to hold down without noticing.
                        let report = if host.resize_toward(dir, cells) {
                            Report::on_screen()
                        } else {
                            Report::nowhere(&BoundAction::ResizePaneToward { dir, cells })
                        };
                        tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                        mouse.follow(&host, &tiling);
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        report
                    }
                    // The ASKING actions are consumed above, where `Ask::of` turns them into a
                    // question. This arm is reached only when it answered `None` — a `rename-pane`
                    // pressed with no pane focused, which has no subject and so nothing to ask
                    // about. Doing nothing is the honest outcome: inventing a subject would rename
                    // a pane the user is not looking at.
                    Command::Act(
                        BoundAction::RenameWindow
                        | BoundAction::RenameSession
                        | BoundAction::RenamePane
                        | BoundAction::MoveWindowBefore
                        // ...and a `choose-tree` whose daemon answered an empty tree, which is the
                        // same shape one level up: a question with nothing to ask about.
                        | BoundAction::ChooseTree
                        | BoundAction::ConfirmBefore { .. },
                    ) => Report::on_screen(),
                    Command::Act(BoundAction::SelectNextPane) => {
                        let next = focus.and_then(|pane| tiling.next_after(pane));
                        select_pane(&host, &mut focus, next.or_else(|| tiling.first_pane()));
                        // Only the CURSOR moved, and it is painted from the tiling this loop already
                        // holds — so the repaint is the whole point and the reconcile is not needed.
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                        // The ring is this client's own paint order, so a window holding one pane
                        // cycles onto itself — which is not a refusal, it is the answer.
                        Report::on_screen()
                    }
                };
                // ⚠ A SKEW OUTRANKS WHAT THE KEY THOUGHT IT DID (R324). Every arm above reports
                // what it ASKED for, and against a daemon too old to perform it the honest answer
                // is the one the transport saw: `HostClient::take_gesture_refusal` holds it, taken here by the
                // path that caused it, because a skewed daemon performs nothing and therefore wakes
                // nobody — the mailbox's own drain is on a wake that never comes.
                let report = host
                    .take_gesture_refusal()
                    .map_or(report, |said| Report::said(&said));
                // WHAT THE KEY DID, put where a person can read it. A report with nothing to say
                // leaves the message that is already up alone rather than clearing it: a user who
                // pressed a key that spoke and then typed into a pane is still owed the sentence
                // for its remaining lifetime, and the deadline is what takes it away.
                if let Some(said) = Message::of(&report, now(), display_time(keymap.options())) {
                    // `over` and not a plain assignment: a key that says something must not take the
                    // row from a live ALERT somebody sent this client (R317). One rule, in
                    // `sprag_host::report`, so this front and the windowed one cannot rank two
                    // messages differently.
                    message = Some(said.over(message.take(), now()));
                    // The row is repainted HERE and not by the arm above, because the arm painted
                    // before this message existed. One row, diffed by the surface, so a keystroke
                    // that says nothing costs nothing.
                    paint_status(
                        &mut screen,
                        &host,
                        split.status,
                        showing(&message).as_deref(),
                    )?;
                }
            }
            // A bracketed paste arrives as ONE event rather than a key per character, and this arm
            // is REACHED: termwiz's `set_raw_mode` enables DEC private mode 2004 on the local
            // terminal, so a paste into this window comes back as `Paste` and would be silently
            // dropped without it.
            //
            // Forwarded as a paste rather than as text, and the distinction is the point: the host
            // brackets it if — and only if — the pane's CHILD asked for bracketing, which is a mode
            // only the host can see. A shell that wanted to see a multi-line paste as one edit
            // still does; one that did not still runs it line by line.
            // A PASTE, and the prompt gets first refusal on it for the same reason it gets first
            // refusal on a keystroke: a name pasted into an open question must not land in the
            // shell behind it. Found by the debt audit — the key path was closed and this was not.
            Input::Event(InputEvent::Paste(text)) => match &mut overlay {
                Overlay::Asking(open) => {
                    // A yes/no has nowhere to put text, so only the line takes it — and the row is
                    // redrawn either way, which costs one idempotent repaint and means this arm has
                    // no second opinion about which questions are on screen.
                    match open {
                        Asking::Line { line, refusal, .. } => {
                            if line.pasted(&text) == Typed::Edited {
                                *refusal = None;
                            }
                        }
                        // The chooser's QUERY takes it, because a pasted session name is exactly
                        // what somebody would paste into a chooser — and because the alternative is
                        // the leak this whole arm exists to close.
                        Asking::Choose { pick, refusal } => {
                            if pick.pasted(&text) == Typed::Edited {
                                *refusal = None;
                            }
                        }
                        Asking::Confirm { .. } => {}
                    }
                    paint_prompt(&mut screen, split.panes, open)?;
                }
                // The key table has nowhere to put text either. Swallowed rather than forwarded,
                // which is what the keystroke path does with everything it does not understand —
                // and the point of the whole arm: a paste must not reach the shell behind a surface
                // the user is still using. R306 found exactly that leak on the prompt one round ago,
                // and an overlay added without this arm would have re-opened it.
                Overlay::Showing(_) => {}
                Overlay::None => paste(&host, focus, &text),
            },
            // The pointer is addressed by WHERE IT IS, not by what has the keyboard: a report
            // belongs to the pane under it, which is the only reading a program can make sense of.
            // A press ALSO moves the keyboard there, so the two never drift apart — a client that
            // clicked into one pane while typing into another would hold two answers to "where am
            // I", which is the split-authority shape the layouter already had to settle once.
            //
            // ⚠ EXCEPT WHERE THE USER CANNOT SEE WHAT THEY WOULD BE CLICKING. That rule is what
            // decides the two overlays differently, and it is the rule rather than a compromise:
            //
            // * The KEY TABLE covers the whole screen, so every cell under the pointer belongs to
            //   something invisible. A press on a cell that happens to be a divider would start a
            //   DRAG and resize the arrangement while the user is reading a table — a change with
            //   no gesture behind it and nothing on screen to explain it. Swallowed entirely.
            // * The PROMPT borrows ONE row, so everything the pointer can reach except that row is
            //   visible and is exactly what the user means by clicking it. Passed through, which is
            //   also what leaving R306's surface alone means.
            //
            // Found by the debt audit, not by a test: the arm below had no idea an overlay existed,
            // and the round that added a full-screen one is the round that had to notice.
            Input::Event(InputEvent::Mouse(event))
                if !matches!(
                    overlay,
                    Overlay::Showing(_) | Overlay::Asking(Asking::Choose { .. })
                ) =>
            {
                for edge in pointer.edges(&event) {
                    // A divider DRAG outranks everything below, and it is claimed on the PRESS
                    // rather than recognised on each move: once a drag is under way the pointer
                    // leaves the divider immediately (that is what moving it means), so a client
                    // that asked "is this cell a divider" every event would resize once and then
                    // start clicking into whichever pane the pointer had entered.
                    if edge.kind == MouseEventKind::Press
                        && let Some(divider) = tiling.divider_at(edge.col, edge.row)
                    {
                        dragging = divider.id.map(|id| (id, divider));
                        continue;
                    }
                    if let Some((id, divider)) = dragging {
                        match edge.kind {
                            MouseEventKind::Release => dragging = None,
                            MouseEventKind::Drag => {
                                if let Some(ratio) = divider.ratio_at(edge.col, edge.row) {
                                    tiling = drag_divider(
                                        &host,
                                        split.panes,
                                        &mut focus,
                                        &mut seen_active,
                                        id,
                                        ratio,
                                    );
                                    // Repainted here rather than left to the host's notification:
                                    // a divider that lags the pointer by a round trip is what a
                                    // user reads as a heavy client.
                                    paint(
                                        &mut screen,
                                        &host,
                                        &tiling,
                                        split.panes,
                                        Frame {
                                            focus,
                                            clear: Clear::Yes,
                                            overlay: &overlay,
                                            status: split.status,
                                            message: showing(&message).as_deref(),
                                        },
                                        &mut held,
                                    )?;
                                }
                            }
                            // A press cannot reach here (it is claimed above) and a bare motion
                            // while a button is held is reported as a drag, so nothing else can.
                            _ => {}
                        }
                        continue;
                    }
                    let Some((pane, col, row)) = tiling.pane_at(edge.col, edge.row) else {
                        // A divider column nobody is dragging by, or a cell outside every
                        // rectangle. Not forwarded anywhere: there is no child whose grid holds it.
                        continue;
                    };
                    if edge.kind == MouseEventKind::Press && focus != Some(pane) {
                        select_pane(&host, &mut focus, Some(pane));
                        paint(
                            &mut screen,
                            &host,
                            &tiling,
                            split.panes,
                            Frame {
                                focus,
                                clear: Clear::No,
                                overlay: &overlay,
                                status: split.status,
                                message: showing(&message).as_deref(),
                            },
                            &mut held,
                        )?;
                    }
                    // Pane-LOCAL cells: `pane_at` has already subtracted the rectangle's origin.
                    // The host re-gates this against the pane's own tracking mode, so a report the
                    // child did not ask for costs a message and reaches nothing.
                    let _ = host.mouse(pane, MouseInput { col, row, ..edge });
                }
            }
            // A window change resizes both ends: the local surface, so the view is not cropped, and
            // every PANE, so the programs inside them reflow into their new rectangles. Clearing is
            // what keeps a shrunken screen honest — a partition of the OLD size says nothing about
            // cells the new one does not have.
            Input::Event(InputEvent::Resized { .. }) => {
                // Re-read through `screen_size` rather than trusting the event's payload or
                // `BufferedTerminal::check_for_resize`: both take the terminal's raw answer, so a
                // terminal that reports 0 would undo the boot fallback and leave a 0x0 surface.
                let (cols, rows) = screen_size(screen.terminal())?;
                split = Split::of(cols, rows);
                screen.resize(usize::from(cols), usize::from(rows));
                // The window is arbitrated over what the clients report, so this terminal's new
                // area has to reach the daemon BEFORE the tiling that depends on it — under the
                // default `latest` policy this report IS the new window.
                report_size(&host, split.panes);
                tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
                mouse.follow(&host, &tiling);
                paint(
                    &mut screen,
                    &host,
                    &tiling,
                    split.panes,
                    Frame {
                        focus,
                        clear: Clear::Yes,
                        overlay: &overlay,
                        status: split.status,
                        message: showing(&message).as_deref(),
                    },
                    &mut held,
                )?;
            }
            // `Wake` carries no payload by design — which edge fired is in the flags below.
            Input::Event(_) | Input::Nothing => {}
        }
        if quit.load(Ordering::Acquire) {
            break;
        }
        // `swap` rather than `load` + `store`: a change landing DURING the paint must leave the
        // flag set, so the next iteration repaints instead of showing a frame one behind.
        if repaint.swap(false, Ordering::AcqRel) {
            // ⚠ AN OPEN CHOOSER IS RE-READ ON THE SAME WAKE, and it was NOT until the debt sweep
            // asked. `Pick::refresh` had exactly one caller — the GUI's per-frame reconcile — so
            // this front's list was a PHOTOGRAPH while the other's was live, and `Pick`'s own doc
            // ("refreshed from the daemon so the list is LIVE while a person reads it") was false
            // for half the product. The wake that tells this client a session appeared is the same
            // wake that must put it in the list somebody is reading.
            //
            // The cursor is `Pick::refresh`'s to move and it moves only when its OWN row goes —
            // which is the whole reason the cursor is an identity, and why re-reading under an open
            // list is safe rather than disruptive.
            if let Overlay::Asking(Asking::Choose { pick, .. }) = &mut overlay {
                pick.refresh(&host.tree(), &host.current_session());
            }
            // WHAT SOMEBODY ELSE ASKED THIS CLIENT TO SAY (R317), taken on the same wake. It becomes
            // a `Report` — the very type this client's own keys produce — so there is no second path
            // by which a message reaches the row, and `over` decides between it and whatever is
            // already up by the one rule in `sprag_host::report`.
            //
            // The row is not painted here: the frame below paints it, and it reads `message` through
            // the same `showing` the whole loop does.
            if let Some(announcement) = host.take_message() {
                // ⚠ THE COPY GOES OUT FIRST, and independently of whether a row is built below
                // (R319). A person who set `display-time 0` has asked for no ROW — a decision about
                // this screen — and a message that cannot be shown to somebody who is not here
                // either would be the silence this whole front exists to remove. The two deliveries
                // answer different questions and neither gates the other.
                //
                // Only what the DAEMON routed is copied out. A `Report` this client builds for its
                // own keys is not: it answers a keystroke, and a keystroke is proof somebody is
                // sitting here to read the answer.
                outward.forward(
                    keymap.options(),
                    person,
                    // The session is read HERE and not held: this client can be viewing a different
                    // one than it attached to (`switch-client`, the chooser), and a notification
                    // naming the session the person LEFT would be worse than one naming none.
                    &host.current_session(),
                    &announcement,
                    &mut outward_tty,
                );
                let said = Message::of(
                    &Report::said(&announcement),
                    now(),
                    display_time(keymap.options()),
                );
                if let Some(said) = said {
                    message = Some(said.over(message.take(), now()));
                }
            }
            // Reconciled, not merely repainted: the host's notification covers the ARRANGEMENT as
            // well as the cells, so a split made from another client — or a pane whose shell just
            // exited and was closed — changes which rectangles exist. Painting the old tiling would
            // put the new pane nowhere and leave the closed one's cells on screen.
            tiling = reconcile(&host, split.panes, &mut focus, &mut seen_active);
            // A child that just enabled tracking woke this client the same way its output would,
            // so the mirror is re-read here and not on a timer.
            mouse.follow(&host, &tiling);
            paint(
                &mut screen,
                &host,
                &tiling,
                split.panes,
                Frame {
                    focus,
                    clear: Clear::No,
                    overlay: &overlay,
                    status: split.status,
                    message: showing(&message).as_deref(),
                },
                &mut held,
            )?;
        }
    }
    // Before the terminal is given back: see [`MouseMirror::release`], and — for the same reason,
    // one mode along — stop asking it about focus. termwiz restores what IT set and it set neither.
    mouse.release();
    if outward.watching() {
        Outward::watch_focus(false, &mut outward_tty);
    }

    // `BufferedTerminal`'s inner terminal restores the termios, the cursor and the alternate
    // screen on drop, so the normal exit path needs nothing here beyond letting it drop.
    Ok(())
}

/// What one turn of the loop has to act on.
///
/// A named type rather than the `Option<InputEvent>` [`Terminal::poll_input`] answers, because there
/// are now THREE outcomes and two of them are that answer's `None`: a timeout (which may expire a
/// message) and a focus change (which must not). Collapsing them would make the row's own deadline
/// depend on whether somebody switched windows.
enum Input {
    /// The terminal delivered something that has to be routed.
    Event(InputEvent),
    /// The person left this terminal or came back to it — already recorded, nothing to route.
    Focus,
    /// A timeout, or a wake with nothing behind it.
    Nothing,
}

/// The next thing the loop must act on, with any FOCUS REPORT taken out of the stream (R319).
///
/// # Why the read-ahead, and why it is bounded to one event
///
/// `termwiz 0.23.3` has no focus event and no seam at the bytes, so a terminal's `CSI I` arrives as
/// the two keystrokes `Alt-[` and `I` (measured — see [`sprag_tui::focus`]). Routing those would type
/// them into whatever the person left running. What separates a report from somebody typing those two
/// keys is that the report is ONE WRITE and therefore lands in ONE read: termwiz parses a read whole
/// and queues every event it found, and `poll_input` drains that queue before it polls, so a second
/// event available with NO WAIT came from the same read.
///
/// So a bracket — and only a bracket — costs one zero-wait poll. What comes back is either the other
/// half of a report, or an event that has to be routed on the NEXT turn, which is what `pending`
/// holds. Nothing is dropped and nothing is delayed by more than the turn it takes to decide.
///
/// `person` is [`None`] when this client never asked its terminal to report focus, and then this is
/// exactly `poll_input` with a pushback: no bracket is read ahead of, so no binding can be swallowed
/// for a person who switched the feature off.
fn read_input(
    screen: &mut BufferedTerminal<SystemTerminal>,
    waiting: Option<Duration>,
    pending: &mut Option<InputEvent>,
    person: Option<&mut Person>,
) -> Result<Input, Box<dyn Error>> {
    // The read-ahead's leftover comes first and WITHOUT polling: it was read before this call, so a
    // poll here could park on a timeout while an event this loop already holds waits behind it.
    let event = match pending.take() {
        Some(held) => Some(held),
        None => screen.terminal().poll_input(waiting)?,
    };
    let Some(event) = event else {
        return Ok(Input::Nothing);
    };
    let Some(person) = person else {
        return Ok(Input::Event(event));
    };
    if !focus::opens_report(&event) {
        return Ok(Input::Event(event));
    }
    // ZERO, not a small wait: the whole discriminator is that a report's second half is ALREADY
    // queued. A wait — any wait — would start catching people's keystrokes instead.
    let next = screen.terminal().poll_input(Some(Duration::ZERO))?;
    match focus::edge(&event, next.as_ref()) {
        Some(seen) => {
            *person = seen;
            Ok(Input::Focus)
        }
        // Not a report: the bracket is the person's, and so is whatever came back behind it.
        None => {
            *pending = next;
            Ok(Input::Event(event))
        }
    }
}

/// What this client has already put on the terminal — the baseline every frame is a difference
/// against, and the reason a repaint is cheap.
///
/// The fields are one concept and not a bag: none is state the client HAS, each is a record of what
/// the terminal was last told, and each exists so that telling it again can be skipped. They are
/// also written in exactly one place ([`paint`]), which is what makes the records trustworthy — a
/// second writer would leave the terminal and the record disagreeing with no way to notice.
#[derive(Default)]
struct Painted {
    /// The rows the surface holds, so only the rows whose stamps moved are rebuilt (R246).
    cache: PaintCache,
    /// The window title as last SET, so an unchanged digest costs no escape sequence at all — see
    /// [`title_change`], which owns that decision and is tested on it.
    title: Option<String>,
    /// What that title was DERIVED from: the host's pane-agent token
    /// ([`HostClient::pane_agents_token`]) and the session name, together the whole input to
    /// [`agent_window_title`]. Held so the digest is rebuilt when its source moved rather than on
    /// every keystroke — see [`retitle`].
    title_from: Option<(u64, String)>,
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
/// [`HostClient::pane_cells_and_token`] reads [`WireHost`](sprag_client::WireHost)'s
/// poll-maintained cache with no socket call.
///
/// What is NOT free is turning those cells into changes, which is `O(cells)` and was being done in
/// full every frame — once per keystroke on the input path, and measured at 17.9 ms for a 240x64
/// pane. So the cells are read whole and the CHANGES are built only for the rows the producer
/// stamped, through [`PaintCache`]. The cursor is unaffected: it is `O(1)` and a bare cursor move
/// stamps no row, which is exactly why it is painted separately.
fn paint(
    screen: &mut BufferedTerminal<SystemTerminal>,
    host: &WireHost,
    tiling: &Tiling,
    screen_area: Rect,
    frame: Frame<'_>,
    held: &mut Painted,
) -> Result<(), Box<dyn Error>> {
    let Frame {
        focus,
        clear,
        overlay,
        status,
        message,
    } = frame;
    // The outer terminal's title, refreshed HERE because this is the one function that writes the
    // terminal at all — so every path that repaints also re-titles, and no future caller can add a
    // repaint that forgets to. Before the empty-tiling return: a client whose last pane just closed
    // still owes the title the truth, and "no panes" is not "an agent is still waiting".
    retitle(screen, host, held);
    if tiling.panes.is_empty() {
        // No panes is a legitimate transient state (the last one just closed), not an error: the
        // host will either grow one or go away, and both wake this loop. The last frame stays on
        // screen rather than being blanked, because a user whose shell just exited is owed the
        // output it exited with. Flushed all the same, so a title the retitle above queued is not
        // held back by a frame there is nothing to draw for; with no cell changes it paints nothing.
        //
        // THE STATUS ROW IS STILL DRAWN, before the return: a client whose last pane just closed is
        // still attached to a session and can still be pressed at, so the one surface able to say
        // so must not be the thing that disappears with the panes.
        screen.add_changes(status_changes(status, &Status::of(host), message));
        screen.flush()?;
        return Ok(());
    }
    if clear == Clear::Yes {
        screen.add_change(Change::ClearScreen(ColorAttribute::Default));
        // The surface no longer holds anything the cache could let a row skip.
        held.cache.forget();
    }
    let mut drawn = Vec::with_capacity(tiling.panes.len());
    for held in &tiling.panes {
        // The tiling is in WINDOW coordinates, which this terminal need not be big enough to hold:
        // a rectangle past the edge is painted as far as the screen goes and no further. Skipped
        // entirely when it does not reach the screen at all — a pane of a window larger than this
        // terminal, which is visible on the clients that do have room for it.
        let Some(area) = held.area.intersect(screen_area) else {
            continue;
        };
        let (cells, token) = host.pane_cells_and_token(held.pane, 0);
        drawn.push(PanePaint {
            pane: held.pane,
            area,
            cells,
            token,
        });
    }
    // The cursor is read from the SAME buffers the changes were built from, and before them in the
    // source only because the cache consumes the list: a second fetch could land a frame apart and
    // put the cursor where the cells are not.
    let cursor = drawn
        .iter()
        .find(|held| focus == Some(held.pane))
        .map_or_else(Vec::new, |held| cursor_changes(&held.cells, held.area));
    screen.add_changes(held.cache.changes(&drawn));
    for divider in &tiling.dividers {
        let Some(area) = divider.area.intersect(screen_area) else {
            continue;
        };
        screen.add_changes(divider_changes(&Divider { area, ..*divider }));
    }
    // THE STATUS ROW, after the panes and BEFORE the cursor. It is outside the tiling's rectangle
    // by construction ([`Split`]), so nothing above can have painted over it — and it is drawn HERE
    // rather than at the call sites for the same reason the prompt below is: this is the one
    // function that writes the terminal, so no future repaint can be added that forgets the row.
    // `Status` is DERIVED from the host on every frame, so there is no cached location to go stale.
    //
    // **Before the cursor, and that ordering is load-bearing**: this row is the last thing painted
    // in the terminal's own bottom-left corner, so drawing it after the cursor would leave the
    // terminal's one cursor sitting on the status line instead of in the pane the user is typing
    // into. The overlays below can follow the cursor because each of them HIDES it.
    screen.add_changes(status_changes(status, &Status::of(host), message));
    screen.add_changes(cursor);
    // THE PROMPT LAST, and inside this function rather than at the call sites, for the reason the
    // retitle above is here: this is the one place that writes the terminal, so every path that
    // repaints also re-draws the question, and no future caller can add a repaint that drops it.
    // Found by the audit rather than by a test — a resize or a host wake while the prompt was up
    // wiped the row off the screen while this client went on eating every keystroke.
    match overlay {
        Overlay::None => {}
        Overlay::Asking(asking) => {
            screen.add_changes(asking_changes(screen_area, asking));
        }
        Overlay::Showing(showing) => {
            screen.add_changes(help_changes(screen_area, showing.help(), showing.scroll()));
        }
    }
    screen.flush()?;
    Ok(())
}

/// Set the outer terminal's window title to what the session's agents are doing
/// ([`agent_window_title`]), and only when that has CHANGED since this client last set one.
///
/// The equality skip is not an optimisation, it is the whole reason a title is cheap enough to
/// refresh from the paint path: `Surface::add_change` records every change it is handed and the flush
/// renders them, so a title re-added on each frame would put one OSC on the wire per repaint — per
/// keystroke, on the input path this client's cost model is built around (R246).
///
/// The facts are read from the poll-maintained cache ([`HostClient::pane_agents`]), so this makes no
/// socket call and cannot block a frame. Which pane the user is looking at plays no part: unlike the
/// GUI — whose OS title follows the FOCUSED pane, because its background panes have tabs and dock
/// headers of their own to wear a marker on — this client has no other chrome, so its title has to
/// answer for every pane at once.
///
/// # Two skips, at two different costs, and they are not the same skip
///
/// [`title_change`] skips the OSC when the digest came out the same — it is what keeps an unchanged
/// title off the wire, and it is downstream of building the digest. This one skips BUILDING it, by
/// keying the answer on everything it is derived from ([`HostClient::pane_agents_token`] plus the
/// session name). Until R265 only the first existed, so the walk, the sort and the string build ran
/// on every keystroke and were thrown away; the argument for leaving it that way was that the cost
/// was small at a pane count the wire could not exceed, and R264 removed that ceiling.
///
/// The key is complete because it is not a LIST of inputs: the token counts changes to the cache
/// every one of those inputs lives in, so a verdict moving, a pane opening and a pane closing are
/// the same event to it. A host that will not promise a token answers `None` and this skips
/// nothing, which is the direction a mistake here has to fall.
fn retitle(screen: &mut BufferedTerminal<SystemTerminal>, host: &WireHost, held: &mut Painted) {
    let session = host.current_session();
    // The whole input to the digest, in one value: the host's token for the pane verdicts plus the
    // session the baseline names. `None` from the host means it will not promise a token, and the
    // walk runs unconditionally — the safe direction, and the one every impl but this client's is
    // on (see `HostClient::pane_agents_token`).
    let from = host
        .pane_agents_token()
        .map(|token| (token, session.clone()));
    if from.is_some() && from == held.title_from {
        return;
    }

    let wanted = agent_window_title(&session, &host.pane_agents());
    if let Some(change) = title_change(&mut held.title, wanted) {
        screen.add_change(change);
    }
    held.title_from = from;
}

/// Lay the host's arrangement out over `area`, keep `focus` on a pane that is actually shown, and
/// match every pane's PTY to the rectangle it was given.
///
/// The three belong together because each depends on the tiling the other two would otherwise
/// recompute — and because getting them out of step is what a partial update looks like: a focus on
/// a pane that no longer has a rectangle sends keys into a program nobody can see, and a pane whose
/// PTY still holds the old rectangle's size reflows to the wrong width.
fn reconcile(
    host: &WireHost,
    screen: Rect,
    focus: &mut Option<PaneId>,
    seen_active: &mut Option<PaneId>,
) -> Tiling {
    // The PROJECTION, so a zoomed session shows its one pane here as it does in the GUI and as the
    // daemon has already sized its PTY. The `shown` fallbacks below then cover the zoom's hidden
    // panes with no arm of their own — a pane the tiling does not name is a pane this terminal
    // cannot display, which is the same sentence a float already made true.
    let layout = host.layout();
    let tiling = tile(&layout.projection(), window_area(host, screen));
    // FOLLOW the daemon's active pane WHEN IT MOVES, then this client's own, then the first pane
    // shown — each step used only when the one before it names a pane this terminal is showing.
    //
    // The daemon leads because which pane the session is on is session state: another client's
    // `select-pane`, a `sprag select-pane` from a shell, and a close handing off all move it with
    // nothing local having happened, and a client that ignored it would be a second authority.
    //
    // It can still name a pane this terminal cannot show — the user FLOATED it, and a terminal
    // client tiles the arrangement only. That is the one case the fallbacks below exist for, and
    // they move the cursor LOCALLY without publishing: "I cannot display that" is not the user
    // choosing something else, and telling the daemon otherwise would fight the client that can.
    let shown = |pane: PaneId| tiling.area_of(pane).is_some();
    let active = host.active_pane();
    let moved = active != *seen_active;
    *seen_active = active;
    let wanted = active
        .filter(|_| moved)
        .filter(|pane| shown(*pane))
        .or_else(|| focus.filter(|pane| shown(*pane)))
        .or_else(|| tiling.first_pane());
    set_focus(host, focus, wanted);
    // A pane of a session with an arbitrated window is sized by the DAEMON, which holds both inputs
    // (`tile(tree, window)`) and re-derives whenever either moves. This client writes a size only
    // when the host has no window to derive from — an older daemon, or one nothing has reported an
    // area to — which is the same fallback `window_area` above states for its own screen, and what
    // this did before either existed.
    if host.window_size().is_none() {
        for pane in &tiling.panes {
            resize_pane(host, pane.pane, pane.area);
        }
    }
    tiling
}

/// Write `ratio` onto the split `id` and re-tile from what the host answers.
///
/// The host's reply is used rather than the tree this client just sent, and that is the whole
/// discipline of a layout write: the arrangement is the HOST's, `set_layout` takes the epoch this
/// client last saw, and a write made against a stale one is refused. Re-tiling from the answer is
/// therefore correct whether the write landed or was declined — in both cases it is what the
/// arrangement now IS.
///
/// The panes are resized through the same [`reconcile`] every other path uses, so a dragged
/// boundary reaches the children's PTYs exactly as a split or a window change does.
fn drag_divider(
    host: &WireHost,
    area: Rect,
    focus: &mut Option<PaneId>,
    seen_active: &mut Option<PaneId>,
    id: SplitId,
    ratio: f32,
) -> Tiling {
    let snapshot = host.layout();
    if let Some(tree) = with_ratio(&snapshot.tree, id, ratio) {
        // The canonical arrangement is DROPPED and the reconcile below re-reads it, which is the
        // one place in this file that is right to drop an answer: a refused write (the layout moved
        // under the drag) answers with the arrangement actually in force, and this function's whole
        // job is to project whatever that is. Adopting it here would be a second path to the fact
        // the next line already fetches.
        let _ = host.set_layout(tree, snapshot.revision);
    }
    reconcile(host, area, focus, seen_active)
}

/// Move THIS CLIENT's focus to `next`, telling the panes on both ends of the move.
///
/// The host is told because a child that enabled DEC 1004 asked to be: an editor that reloads a
/// changed file when it regains attention is reacting to exactly this edge, and a client that
/// moved focus silently would leave it reacting to nothing. A no-op when focus does not move, so
/// the callers can be blunt about calling it.
///
/// LOCAL: it reports an edge and moves the cursor, and says nothing about which pane the SESSION is
/// on. [`select_pane`] is the half that does, and the split is deliberate — a client following the
/// daemon, or falling back because it cannot show the active pane, must not publish a correction
/// The name of the session's CURRENT window, off this client's own window mirror — what
/// `kill-window` from a key means by "this window".
///
/// The mirror rather than a fresh read: `current` is the fact the daemon publishes on the `windows`
/// slot for exactly this question, and the poll thread refreshes it. `None` when the mirror holds no
/// current window, which is a client that has not finished booting rather than a session without one.
fn current_window_name(host: &WireHost) -> Option<String> {
    host.windows()
        .into_iter()
        .find(|window| window.current)
        .map(|window| window.name)
}

/// nobody asked for (see [`reconcile`]).
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

/// The USER moved to `next`: this client's focus AND the session's active pane.
///
/// Every path where a person chose a pane goes through here — a click, `select-pane` on the keymap,
/// the pane a split just opened. The daemon is told because the choice is SESSION state: another
/// attached client follows it, a reattaching one inherits it, and `sprag split-window -h` with no
/// pane divides it. The publish is synchronous, so the next poll reads back what was just sent and
/// [`reconcile`] cannot yank the cursor to where the user was a moment ago.
fn select_pane(host: &WireHost, focus: &mut Option<PaneId>, next: Option<PaneId>) {
    set_focus(host, focus, next);
    if let Some(pane) = next {
        // A refusal is not this client's to repair: the daemon has the authoritative pane set, so a
        // pane it will not select is one that has left, and the next reconcile answers with what IS.
        let _ = host.select_pane(pane);
    }
}

/// Send one key, named in the wire's vocabulary, to the focused pane.
///
/// Takes the name and modifiers rather than a [`WireKey`] because two callers reach it with the same
/// pair spelled differently: a keystroke this terminal decoded, and the
/// [`KeySpec`](sprag_host::keymap::KeySpec) a `send-prefix` binding has to deliver. Both are "a key
/// the wire can address", and the host's own
/// [`HostClient::send_key`] takes exactly this pair.
///
/// A key the host declines is logged, not surfaced: the only place this client could report it is
/// the screen it is painting a pane onto, and a viewer that scribbled diagnostics over a user's
/// program would be worse than the dropped key. `false` covers a key `sprag-input` has no encoding
/// for (F13 upward) and a pane that closed between the poll and the send — neither is this
/// client's to fix.
fn send_key(host: &WireHost, focus: Option<PaneId>, name: &str, mods: Modifiers) {
    let Some(pane) = focus else {
        return;
    };
    if !host.send_key(pane, name, mods) {
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

/// The rectangle the arrangement is laid out over: the session's ARBITRATED window if the host has
/// one, else this terminal's own screen.
///
/// The fallback is not a default, it is the honest answer to a different question. A host with no
/// window (an older daemon, or one no client has reported an area to) is saying nothing about what
/// the panes should be, and this client's own screen is then the only fact available — which is
/// exactly what it used before `window-size` existed.
fn window_area(host: &WireHost, screen: Rect) -> Rect {
    match host.window_size() {
        Some((cols, rows)) => Rect::screen(cols, rows),
        None => screen,
    }
}

/// Tell the host how big this terminal is — the input its `window-size` policy arbitrates over.
///
/// Called at boot and on every window change, which are exactly the moments the answer can move.
/// The daemon ignores a repeat of the same numbers, so a resize that ends where it started costs
/// one call and wakes nobody.
/// The status row's content for THIS instant: the message while one is live, and nothing once its
/// deadline has passed.
///
/// One reader, called at every paint, so a row cannot show a sentence past its lifetime because one
/// call site forgot to check the clock. The clock is read HERE rather than passed in for the reason
/// [`Frame::message`] states: the loop and the paint must agree, and they agree by there being one
/// function.
fn showing(message: &Option<Message>) -> Option<String> {
    let said = message.as_ref()?;
    let line = said.showing(now())?;
    // MARKED, from `Message::mark` — the shared derivation, so this row and the windowed strip put
    // the same word in front of the same message. Only an ALERT carries one; see there for why.
    Some(
        said.mark()
            .map_or_else(|| line.to_owned(), |mark| format!("{mark}: {line}")),
    )
}

/// Repaint the status row ALONE and flush it.
///
/// The row-only fast path, exactly as [`paint_prompt`] is for the question: a keystroke that
/// produced a sentence has already repainted whatever else it changed, and re-tiling the panes to
/// put one line on the screen would cost a frame for a row the surface diffs to a few cells.
///
/// [`Status`] is read from the host here rather than passed in, so this and [`paint`] cannot come to
/// disagree about where the client is.
fn paint_status(
    screen: &mut BufferedTerminal<SystemTerminal>,
    host: &WireHost,
    area: Rect,
    message: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    screen.add_changes(status_changes(area, &Status::of(host), message));
    screen.flush()?;
    Ok(())
}

fn report_size(host: &WireHost, screen: Rect) {
    host.report_client_size(screen.cols, screen.rows);
}

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

/// The user's keymap, re-read first if [`CONFIG_FILE`](sprag_host::CONFIG_FILE) has changed since
/// it was last looked at.
///
/// **This is what makes `sprag bind-key` a RUNTIME command.** The file is the live table — it has
/// to be, because `sprag list-keys` reads it with no daemon — so a client acts on an edit by
/// noticing the file moved, and the same mechanism hands the user who edits their config in an
/// EDITOR the reload tmux spells `source-file`, with nothing to invoke.
///
/// Called from the KEY arm alone, which is both the cheapest place and the only correct one: the
/// table decides nothing else, so a check anywhere earlier would be work the answer never uses.
/// **No timer, no thread, no watch** — the loop stays the pure `select` R226 measured.
///
/// The cost is one `read_to_string` per keystroke — the whole file, not a `metadata` stat, because
/// [`ClientConfig::refresh`](sprag_host::config::ClientConfig::refresh) compares CONTENT rather than
/// an mtime (an editor that rewrites a file unchanged must not count as an edit). MEASURED in R246
/// at **4.8 us per key**, against a repaint that was three orders of magnitude dearer before the
/// same round made it row-gated. This sentence said `metadata` until R246 read the function it was
/// describing.
///
/// A broken save keeps the last good table and is LOGGED, because the only screen this client could
/// print on is the one it is painting a user's panes onto — and taking their bindings away over a
/// typo in an editor would be a worse answer than carrying on with the table they had.
fn refreshed(keymap: &mut sprag_host::config::ClientConfig) -> &Keymap {
    if let Err(error) = keymap.refresh() {
        tracing::warn!(target: "sprag_tui::keys", %error, "the edited config was not usable; keeping the loaded keymap");
    }
    keymap.keymap()
}

/// What this client has put over the panes, and therefore what owns the keyboard.
///
/// ONE value rather than an `Option` per surface, and that is a correctness decision rather than a
/// tidiness one: two `Option`s make "a question and the key table are both up" representable, and
/// the client would then be painting one surface while routing keys to the other. There is nothing
/// to keep in step here because there is only one thing.
///
/// It also gives every repaint one field to carry ([`Frame::overlay`]) instead of one per surface,
/// so the next overlay this client grows is an arm and not another thirteen call sites.
enum Overlay {
    /// Nothing: keys route through the keymap to the pane, which is the steady state.
    None,
    /// A question is up. Every key belongs to it — see [`Asking`].
    Asking(Asking),
    /// The key table is up. Every key belongs to it — see [`Showing`].
    Showing(Showing),
}

/// The help view this client is showing, and where the reader has scrolled to.
///
/// The SURFACE half of [`sprag_host::keyhelp`], exactly as [`Asking`] is the surface half of
/// [`sprag_host::prompt`]: the shared module decides which rows, in what order, with what text, and
/// what each key MEANS; this holds the photograph and the position, and knows how many rows fit.
struct Showing {
    /// The table as it was when `?` was pressed.
    help: KeyHelp,
    /// Where the reader is in it.
    scroll: Scroll,
}

impl Showing {
    /// Open the view on `keymap`.
    fn open(keymap: &Keymap) -> Self {
        Self {
            help: KeyHelp::of(keymap),
            scroll: Scroll::default(),
        }
    }

    /// The table being shown.
    fn help(&self) -> &KeyHelp {
        &self.help
    }

    /// Where the reader is.
    fn scroll(&self) -> Scroll {
        self.scroll
    }

    /// Feed one keystroke to the open view.
    ///
    /// `viewport` is how many rows of the table are on screen, which the caller knows and this does
    /// not: the same value the painter uses, so a page here and a page there are the same distance.
    fn pressed(&mut self, event: &KeyEvent, viewport: usize) -> Shown {
        let Some(key) = wire_key(event) else {
            // A key the wire cannot spell is still this view's — swallowed, not passed on, which is
            // the rule the prompt states one surface over.
            return Shown::Open;
        };
        let mut scratch = [0u8; 4];
        match self
            .help
            .pressed(self.scroll, key.name(&mut scratch), key.mods(), viewport)
        {
            Pressed::Open(scroll) => {
                self.scroll = scroll;
                Shown::Open
            }
            Pressed::Closed => Shown::Closed,
        }
    }
}

/// Whether the help view survived a keystroke.
#[derive(PartialEq, Eq, Debug)]
enum Shown {
    /// Still up.
    Open,
    /// The reader left; give the panes back.
    Closed,
}

/// The question this client is asking, and everything it needs to finish asking it.
///
/// The SURFACE half of [`sprag_host::prompt`]: the shared module decides which actions ask, what
/// they ask and what an answer does; this holds the live editor and the sentence to paint. A GUI
/// holds a field and a modal instead, which is the split the shared module's own docs draw.
enum Asking {
    /// A name is being typed.
    Line {
        /// What the answer will name — carried, not re-derived, so the commit cannot land on a
        /// different subject than the question named.
        subject: Subject,
        /// The editor.
        line: Line,
        /// What the daemon (or the grammar) refused last, painted after the question and cleared by
        /// the next edit: a refusal is about the text that was sent, so it stops being true the
        /// moment that text changes.
        refusal: Option<String>,
    },
    /// A yes/no is being answered.
    Confirm {
        /// The whole sentence, as it was when the prompt was armed.
        question: String,
        /// What to do on `y`.
        action: BoundAction,
    },
    /// A LIST is being picked from (R315) — the third kind of question, and the only one that
    /// takes the whole screen.
    ///
    /// It sits in [`Asking`] rather than beside [`Showing`] even though it LOOKS like the help
    /// view, and the reason is what the two are: the help view is a thing to read and this is a
    /// question to answer. Everything the loop does with a question — route every key to it, give
    /// the keyboard back when it closes, run what it authorised — is already written once here.
    Choose {
        /// The open chooser: rows, query and the picked row, all `sprag_host::chooser`'s.
        pick: Box<Pick>,
        /// The daemon's refusal, standing until the next keystroke — the picked row is gone.
        refusal: Option<String>,
    },
}

/// What answering did.
enum Answered {
    /// Still asking — repaint the row.
    Asking,
    /// The prompt is over and there is nothing to do.
    Closed,
    /// The prompt is over and this action was authorised.
    Perform(BoundAction),
}

impl Asking {
    /// Open a prompt for `ask`.
    fn open(ask: Ask) -> Self {
        match ask {
            Ask::Line { subject, seed } => Self::Line {
                subject,
                line: Line::new(&seed),
                refusal: None,
            },
            // The two sentences are joined HERE and the answer hint is added here too: the shared
            // ask names the act and its consequence, and how one answers is the surface's — this
            // client has no buttons, so `(y/n)` is what tells a user which keys mean what.
            Ask::Confirm {
                question,
                consequence,
                action,
                ..
            } => Self::Confirm {
                question: match consequence {
                    Some(also) => format!("{question} {also} (y/n)"),
                    None => format!("{question} (y/n)"),
                },
                action: *action,
            },
            Ask::Choose { pick } => Self::Choose {
                pick,
                refusal: None,
            },
        }
    }

    /// Feed one keystroke to the open prompt.
    ///
    /// The two arms answer different questions and so read keys differently, and neither gesture is
    /// invented here: a line is edited with the readline chords the pane behind it would have used
    /// ([`Line::typed`]), and a yes/no takes `y` and treats EVERYTHING ELSE as no — tmux's own
    /// `confirm-before` rule, and the safe direction for a question whose yes destroys something.
    /// `Enter` is deliberately not a yes for that reason: the key that arms a prompt must not also
    /// be the key that answers it.
    fn answered(&mut self, host: &WireHost, event: &KeyEvent) -> Answered {
        let Some(key) = wire_key(event) else {
            // A key the wire cannot spell is still the prompt's — swallowed, not passed on.
            return Answered::Asking;
        };
        let mut scratch = [0u8; 4];
        let name = key.name(&mut scratch).to_owned();
        match self {
            Self::Confirm { action, .. } => match name.as_str() {
                "y" | "Y" => Answered::Perform(action.clone()),
                _ => Answered::Closed,
            },
            // A PICK. The keys are `Pick::typed`'s — shared with the other frontend for the same
            // reason the line editor's are — and what an answer DOES is `Pick::commit`'s. What this
            // arm decides is only what happens to the surface afterwards, which is the surface's
            // half of that split.
            Self::Choose { pick, refusal } => match pick.typed(&name, key.mods()) {
                Typed::Ignored => Answered::Asking,
                Typed::Edited => {
                    // The standing refusal was about a row the cursor has left.
                    *refusal = None;
                    Answered::Asking
                }
                Typed::Cancel => Answered::Closed,
                // The chooser STAYS OPEN on a refusal, exactly as the name prompt does and for its
                // stated reason: a person whose row went while they were reading has lost nothing
                // but that row, and closing the list would make them press the key again to find
                // out what else is there.
                Typed::Commit => match pick.commit(host) {
                    Ok(()) => Answered::Closed,
                    Err(why) => {
                        *refusal = Some(why);
                        Answered::Asking
                    }
                },
            },
            Self::Line {
                subject,
                line,
                refusal,
            } => match line.typed(&name, key.mods()) {
                Typed::Ignored => Answered::Asking,
                Typed::Edited => {
                    // The standing refusal was about text that no longer exists.
                    *refusal = None;
                    Answered::Asking
                }
                Typed::Cancel => Answered::Closed,
                Typed::Commit => {
                    let answer = line.text().to_owned();
                    // The grammar first, with the daemon's own function, so the sentence names the
                    // rule; then the daemon, whose refusal now has one cause left. Either way the
                    // prompt STAYS OPEN with what was typed still in it — a user who has to retype
                    // a name they just typed has been told off rather than helped.
                    match subject
                        .check(&answer)
                        .and_then(|()| subject.commit(host, &answer))
                    {
                        Ok(_recorded) => Answered::Closed,
                        Err(why) => {
                            *refusal = Some(why);
                            Answered::Asking
                        }
                    }
                }
            },
        }
    }
}

/// Paint the prompt row over the bottom of the screen, and nothing else.
///
/// The row-only path, used while a name is being TYPED: a keystroke changes one row, and repainting
/// every pane for it would put the whole arrangement through the diff cache at typing rate. Every
/// other path goes through [`paint`], which draws this same row last so a repaint cannot lose it —
/// see [`prompt_changes`] for why the row is an overlay and not a reserved line.
fn paint_prompt(
    screen: &mut BufferedTerminal<SystemTerminal>,
    screen_area: Rect,
    asking: &Asking,
) -> Result<(), Box<dyn Error>> {
    screen.add_changes(asking_changes(screen_area, asking));
    screen.flush()?;
    Ok(())
}

/// What an open question LOOKS like — the ONE place the three kinds pick their surface.
///
/// Two callers must agree: the row-only fast path above, and [`paint`]'s own last act. They held
/// one `prompt_changes` call each until a question arrived that is not a row, and two call sites
/// each deciding would be two answers to "what is on the screen right now" — the exact shape that
/// wiped the prompt off a repainting screen once already.
fn asking_changes(screen_area: Rect, asking: &Asking) -> Vec<termwiz::surface::Change> {
    match asking {
        Asking::Choose { pick, refusal } => chooser_changes(screen_area, pick, refusal.as_deref()),
        // The three ROW pieces are read straight off the arm that has them, and that is the audit
        // correcting a shape it had just introduced: these were three `Asking` methods, each with a
        // `Choose` arm returning an empty string or a `None` that nothing could ever ask for. A
        // total function whose totality is a FICTION is the thing this project spells "make the
        // wrong thing unrepresentable" — the arms are gone, and so is the only caller that needed
        // them to be total.
        Asking::Line {
            subject,
            line,
            refusal,
        } => prompt_changes(
            screen_area,
            &match refusal {
                // Two spaces, so the refusal reads as a second clause rather than as part of the
                // name being typed — the row has no colour of its own to separate them with.
                Some(why) => format!("{}  {why}", subject.question()),
                None => subject.question().to_owned(),
            },
            line.text(),
            Some(line.cursor()),
        ),
        // A yes/no has nothing to type into, so no text and no caret.
        Asking::Confirm { question, .. } => prompt_changes(screen_area, question, "", None),
    }
}

/// Paint the help view over the screen, and nothing else.
///
/// [`paint_prompt`]'s peer and used for the same reason: while a reader is scrolling, every
/// keystroke changes only what this draws, and putting the whole arrangement through the diff cache
/// for a page-down would be paying for panes nobody can see. Every other path goes through
/// [`paint`], which draws this last so a repaint cannot lose it.
fn paint_help(
    screen: &mut BufferedTerminal<SystemTerminal>,
    screen_area: Rect,
    showing: &Showing,
) -> Result<(), Box<dyn Error>> {
    screen.add_changes(help_changes(screen_area, showing.help(), showing.scroll()));
    screen.flush()?;
    Ok(())
}

/// What one frame shows beyond the panes themselves — the three facts that are about THIS paint
/// rather than about the arrangement.
///
/// A record because [`paint`] took eight arguments once the prompt row joined it, three of which
/// were a bare `Option`, a two-state enum and another `Option` in a row: a call site that swapped
/// two of them would still compile. Named fields cost nothing at eleven call sites and make the
/// swap unrepresentable.
struct Frame<'a> {
    /// The pane the cursor belongs to.
    focus: Option<PaneId>,
    /// Whether the screen is cleared first — see [`Clear`].
    clear: Clear,
    /// What is over the panes, drawn last so no repaint can lose it.
    overlay: &'a Overlay,
    /// The bottom row this client speaks in — [`Split::status`]. Empty on a terminal with no room
    /// for one, which every painter here already treats as "draw nothing".
    status: Rect,
    /// What that row says INSTEAD of where the client is, while a message is live.
    ///
    /// Carried on the frame rather than read inside [`paint`] for the reason the whole record
    /// exists: the message has a DEADLINE, so a paint that read it would be reading a different
    /// clock than the loop that set it — and the row would clear one frame late, on a client that
    /// only repaints when something else happens.
    message: Option<&'a str>,
}

/// What the loop should do with a key.
#[derive(PartialEq, Eq, Debug)]
enum Command {
    /// Send it to the pane.
    ToPane(WireKey),
    /// Carry out a bound command of this client's own.
    Act(BoundAction),
    /// Nothing — the key was the prefix itself, an unbound command, or one the wire cannot spell.
    Swallow,
}

/// Route one key through `keymap`, advancing `keys`.
///
/// The keystroke is decoded into the wire's vocabulary ONCE, and everything downstream asks about
/// that: whether it is the prefix, what it is bound to, and what reaches the pane. A key the wire
/// has no spelling for is therefore not a key a binding could have named either, which is why the
/// one decode can serve all three.
///
/// The DECODE is all this adds. Which keystroke is the prefix, what an armed key means, and when the
/// mode ends are [`Keymap::route`]'s — shared with `sprag-gui`, whose keys arrive already spelled
/// that way, so the two frontends cannot come to disagree about what a user's table says.
fn command(keys: &mut PrefixMode, keymap: &Keymap, event: &KeyEvent) -> Command {
    // The prefix is a ONE-KEY mode, so it ends here — before anything looks at what the key is —
    // rather than in each outcome, where a new binding could forget it. Taking the old mode out in
    // the same move is what leaves exactly one place that can put it back.
    let mode = std::mem::replace(keys, PrefixMode::ToPane);
    let Some(key) = wire_key(event) else {
        // A key the wire cannot spell reaches neither a pane nor the table. It still ENDS the
        // prefix mode, because the mode is one key long whatever that key turns out to be.
        return Command::Swallow;
    };
    let mut scratch = [0u8; 4];
    // The clock is read HERE and passed in, which is all a repeat window (`-r`) costs this loop:
    // nothing observes a window closing except the next keystroke, so there is still no timer, no
    // tick and no timeout on the `select` — the property this client's whole idle cost rests on.
    let routed = keymap.route(mode, Instant::now(), key.name(&mut scratch), key.mods());
    *keys = routed.next();
    match routed {
        Routed::ToPane => Command::ToPane(key),
        // `again` is consumed by `Routed::next` above, which is the one place the mode is decided.
        Routed::Act { action, .. } => Command::Act(action),
        Routed::Prefix | Routed::Swallow => Command::Swallow,
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

/// This terminal's mouse reporting, kept a MIRROR of what the panes' children have asked for.
///
/// # Why mirror rather than simply capture
///
/// Turning mouse reporting on takes the pointer away from the user's own emulator: click-drag
/// selection and wheel scrolling stop working in their window and arrive here instead. That is a
/// real cost, and one every full-screen program already imposes — running `vim` with `set mouse=a`
/// in a bare terminal does exactly the same thing. What would be unreasonable is imposing it when
/// nothing wants it, which is what a client that captured for its whole life would do: in a plain
/// shell there is no program to give the reports to, and the host's encoder would drop every one.
///
/// So the rule is that this terminal reports what a pane's child would have made it report had the
/// child been running here directly. A user in a shell keeps their selection; the moment an editor
/// asks for tracking, tracking is on; when it exits, it is off again. The LEVEL is mirrored too
/// (1000 / 1002 / 1003), not just the on-off: capturing at any-event when a pane asked for
/// button-event would put a report on the wire for every pointer movement across the window, all
/// of which the host would then discard — over ssh, for nothing.
///
/// The maximum over the panes is what is set, because there is one pointer and one terminal: two
/// panes cannot be tracked at different levels, and the pane wanting more would otherwise be
/// starved by the one wanting less. The host still gates each report against the pane it is
/// addressed to, so the extra events a lower-wanting pane sees are dropped THERE — the mirror
/// widens what arrives, never what a child is told.
///
/// # Why this writes the sequences itself
///
/// `Terminal::set_raw_mode` enables mouse reporting from the CAPABILITIES, once, and there is no
/// method to change it afterwards. `Change::Text` cannot carry an escape sequence — it renders
/// control characters inert by contract. So the client holds its own handle on the terminal (see
/// [`run`]) and writes `CSI` values built with termwiz's own escape vocabulary rather than
/// hand-spelled bytes; the modes are named, not numbered, at every point in this file.
struct MouseMirror {
    /// The controlling terminal, the same file the [`SystemTerminal`] was built over.
    tty: std::fs::File,
    /// The level currently enabled on it — what a DECRST would have to undo.
    active: MouseProtocol,
}

impl MouseMirror {
    /// A mirror over `tty`, reflecting nothing: a client that has not yet read an arrangement has
    /// been told of no pane that wants the pointer.
    fn new(tty: std::fs::File) -> Self {
        Self {
            tty,
            active: MouseProtocol::None,
        }
    }

    /// Set this terminal's reporting to the highest level any pane of `tiling` is asking for.
    ///
    /// Called from [`reconcile`]'s callers rather than from `reconcile` itself, because the tracking
    /// mode is a fact about the PANES that arrives on the same host notification as everything else:
    /// a child enabling tracking wakes this client exactly as its output does.
    fn follow(&mut self, host: &WireHost, tiling: &Tiling) {
        let wanted = tiling
            .panes
            .iter()
            .map(|held| host.pane_mouse_protocol(held.pane))
            .max_by_key(|protocol| tracking_rank(*protocol))
            .unwrap_or(MouseProtocol::None);
        self.set(wanted);
    }

    /// Turn reporting off, whatever it was — the exit path.
    ///
    /// Not left to `Drop` on the terminal: termwiz restores only what IT set, and it did not set
    /// this. A client that exited with 1003 still on would leave the user's shell forwarding mouse
    /// reports to a prompt, which prints them as garbage on every movement.
    fn release(&mut self) {
        self.set(MouseProtocol::None);
    }

    /// Move the terminal to `wanted`, writing only when something actually changes.
    ///
    /// Every level is DECRST first and the wanted one DECSET after, rather than toggling only the
    /// difference: the three tracking modes are independent DEC private modes, not an enum, so a
    /// terminal left with both 1002 and 1003 set reports at the higher of them. Clearing all three
    /// makes the terminal's state a function of `wanted` alone.
    ///
    /// A write that fails is dropped: the only place this client could report it is the screen it
    /// is painting a user's panes onto, and a terminal that will not take a mode sequence is not
    /// going to take a diagnostic either.
    fn set(&mut self, wanted: MouseProtocol) {
        if self.active == wanted {
            return;
        }
        self.active = wanted;
        let mut out = String::new();
        for code in [
            DecPrivateModeCode::MouseTracking,
            DecPrivateModeCode::ButtonEventMouse,
            DecPrivateModeCode::AnyEventMouse,
            DecPrivateModeCode::SGRMouse,
        ] {
            let _ = write!(
                out,
                "{}",
                CSI::Mode(Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)))
            );
        }
        if let Some(code) = tracking_code(wanted) {
            // SGR (1006) LAST and only alongside a tracking mode: it selects the ENCODING of the
            // reports, and it is what keeps a coordinate past column 223 reportable at all — the
            // legacy form runs out of byte there.
            for code in [code, DecPrivateModeCode::SGRMouse] {
                let _ = write!(
                    out,
                    "{}",
                    CSI::Mode(Mode::SetDecPrivateMode(DecPrivateMode::Code(code)))
                );
            }
        }
        let _ = self.tty.write_all(out.as_bytes());
        let _ = self.tty.flush();
    }
}

/// The DEC private mode that turns `protocol` on, or `None` for no tracking at all.
fn tracking_code(protocol: MouseProtocol) -> Option<DecPrivateModeCode> {
    match protocol {
        MouseProtocol::None => None,
        MouseProtocol::Click => Some(DecPrivateModeCode::MouseTracking),
        MouseProtocol::ButtonEvent => Some(DecPrivateModeCode::ButtonEventMouse),
        MouseProtocol::AnyEvent => Some(DecPrivateModeCode::AnyEventMouse),
    }
}

/// How much a protocol asks for, so the panes' levels can be compared. `MouseProtocol` carries no
/// ordering of its own, and the containment IS total: any-event reports everything button-event
/// does, which reports everything click does.
fn tracking_rank(protocol: MouseProtocol) -> u8 {
    match protocol {
        MouseProtocol::None => 0,
        MouseProtocol::Click => 1,
        MouseProtocol::ButtonEvent => 2,
        MouseProtocol::AnyEvent => 3,
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
/// SGR encoding `1006`), for the client's whole life and at a level nothing here chose. The mouse
/// IS wanted now, but as a mirror of what the panes' children ask for ([`MouseMirror`]) — so what
/// is declined here is termwiz's one-shot decision, not the reporting.
///
/// Declining it does NOT stop the reports being understood: termwiz's `InputParser` is built with
/// no capabilities at all, so it decodes a mouse report whenever one arrives. Only the ENABLING is
/// this client's to place, which is exactly what the mirror needs.
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
    use sprag_host::keymap::KeyTable;

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
    fn routed(keys: &mut PrefixMode, bytes: &[u8]) -> Option<String> {
        routed_with(&Keymap::default(), keys, bytes)
    }

    /// [`routed`] against a keymap other than the default.
    fn routed_with(keymap: &Keymap, keys: &mut PrefixMode, bytes: &[u8]) -> Option<String> {
        match command(keys, keymap, &typed(bytes)) {
            Command::ToPane(key) => {
                let mut scratch = [0u8; 4];
                Some(key.name(&mut scratch).to_owned())
            }
            _ => None,
        }
    }

    /// Route one keystroke through the DEFAULT keymap — what a user who has written no config gets.
    fn acted(keys: &mut PrefixMode, bytes: &[u8]) -> Command {
        command(keys, &Keymap::default(), &typed(bytes))
    }

    const CTRL_B: &[u8] = &[0x02];

    /// The steady state: a key is the program's, not the client's.
    #[test]
    fn an_ordinary_key_reaches_the_pane() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(routed(&mut keys, b"q").as_deref(), Some("q"));
        assert_eq!(keys, PrefixMode::ToPane);
    }

    /// The two keys slice 2 quit on now belong to the program — which is the whole reason the
    /// prefix exists, so it is asserted rather than left to the module docs.
    #[test]
    fn the_old_quit_keys_are_the_programs_now() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(routed(&mut keys, b"q").as_deref(), Some("q"));
        // `Ctrl-C`: an interrupt for the child, not a quit for the client.
        assert_eq!(routed(&mut keys, &[0x03]).as_deref(), Some("c"));
        assert!(routed(&mut keys, &[0x03]).is_some_and(|_| keys == PrefixMode::ToPane));
    }

    /// The prefix is swallowed and arms the next key; `d` then detaches.
    #[test]
    fn the_prefix_then_d_detaches() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(
            keys,
            PrefixMode::AfterPrefix,
            "the prefix arms the next key"
        );
        assert_eq!(
            acted(&mut keys, b"d"),
            Command::Act(BoundAction::DetachClient)
        );
        assert_eq!(keys, PrefixMode::ToPane, "the mode is one key long");
    }

    /// A bare `d` with no prefix is a letter, not a detach. The revert-proof for the prefix
    /// mechanism itself: route `d` first and the client would leave before anything was typed.
    #[test]
    fn a_bare_d_is_a_letter() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(routed(&mut keys, b"d").as_deref(), Some("d"));
    }

    /// `Ctrl-D` after the prefix is not a detach — the binding is the bare letter, and a program's
    /// end-of-file must survive a slip of the Ctrl key.
    ///
    /// Under a keymap this is no longer a rule but a CONSEQUENCE of matching modifiers exactly, so
    /// the assertion is unchanged while the mechanism under it lost a special case.
    #[test]
    fn ctrl_d_after_the_prefix_is_not_a_detach() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(acted(&mut keys, &[0x04]), Command::Swallow);
        assert_eq!(keys, PrefixMode::ToPane);
    }

    /// `prefix prefix` types a literal prefix into the pane, which is what keeps `Ctrl-B` reachable
    /// by a program that binds it (readline's backward-char, for one).
    #[test]
    fn the_prefix_twice_types_a_literal_prefix() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(
            acted(&mut keys, CTRL_B),
            Command::Act(BoundAction::SendPrefix),
            "the second prefix is the send-prefix binding",
        );
        assert_eq!(keys, PrefixMode::ToPane);
        // ...and what that binding sends is the PREFIX itself, read from the keymap rather than
        // from the key that triggered it.
        let keymap = Keymap::default();
        assert_eq!(keymap.prefix().name(), "b");
        assert!(keymap.prefix().mods().ctrl, "and it is still a Ctrl-B");
    }

    /// **`send-prefix` sends the PREFIX, not the key that was pressed.** Bound to `a`, `prefix a`
    /// must type `Ctrl-B` into the pane — the distinction only a rebindable table can even have.
    ///
    /// REVERT-PROOF: forward the triggering event instead (which is what the hardcoded version did,
    /// correctly, because there the only key that could trigger it WAS the prefix) and this sends
    /// `a` — a letter into the user's shell instead of the control byte their program is waiting on.
    #[test]
    fn send_prefix_bound_elsewhere_still_sends_the_prefix() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "a", "send-prefix", false)
            .expect("binds");
        let mut keys = PrefixMode::ToPane;
        assert_eq!(
            command(&mut keys, &keymap, &typed(CTRL_B)),
            Command::Swallow
        );
        assert_eq!(
            command(&mut keys, &keymap, &typed(b"a")),
            Command::Act(BoundAction::SendPrefix),
        );
        // The loop reads the prefix off the keymap for this action; assert the pair it would send.
        assert_eq!(keymap.prefix().name(), "b");
        assert!(keymap.prefix().mods().ctrl);
    }

    /// An unbound command key is dropped rather than delivered — a user who typed the prefix meant
    /// to address the client, so their mistake must not reach a shell.
    #[test]
    fn an_unbound_command_key_is_swallowed() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        // `k` because the default table does not bind it — and this assertion IS that claim, so
        // binding it later fails here with a reason instead of quietly testing a bound key. It used
        // to be `z`, which R289 gave to the zoom.
        assert_eq!(acted(&mut keys, b"k"), Command::Swallow);
        assert_eq!(keys, PrefixMode::ToPane, "and the mode still ends");
    }

    /// A rebound PREFIX moves the gate, and the old prefix becomes the program's again.
    ///
    /// This is the whole point of the round, stated at the keyboard: `Ctrl-A` opens the table and
    /// `Ctrl-B` is now just a keystroke — which is what a user who lives in `screen`'s bindings, or
    /// who needs `Ctrl-B` for their editor, actually asked for.
    #[test]
    fn a_rebound_prefix_moves_the_gate_and_frees_the_old_one() {
        let mut keymap = Keymap::default();
        keymap.set_prefix("C-a").expect("sets");
        const CTRL_A: &[u8] = &[0x01];
        let mut keys = PrefixMode::ToPane;
        assert_eq!(
            routed_with(&keymap, &mut keys, CTRL_B).as_deref(),
            Some("b")
        );
        assert_eq!(keys, PrefixMode::ToPane, "the old prefix arms nothing");
        assert_eq!(
            command(&mut keys, &keymap, &typed(CTRL_A)),
            Command::Swallow
        );
        assert_eq!(keys, PrefixMode::AfterPrefix, "the new one does");
        assert_eq!(
            command(&mut keys, &keymap, &typed(b"d")),
            Command::Act(BoundAction::DetachClient),
        );
    }

    /// A user's own binding reaches the same routing the defaults do, and an unbound DEFAULT stops
    /// meaning anything — the two directions a config has to work in.
    #[test]
    fn a_users_binding_is_routed_and_an_unbound_default_is_not() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "C-o", "detach-client", false)
            .expect("binds");
        keymap.unbind(KeyTable::Prefix, "o").expect("unbinds");
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        // Ctrl-O, the C0 byte — unreachable under the hardcoded table's "a modified command key is
        // a slip" rule, and bindable now.
        assert_eq!(
            command(&mut keys, &keymap, &typed(&[0x0f])),
            Command::Act(BoundAction::DetachClient),
        );
        assert_eq!(
            command(&mut keys, &keymap, &typed(CTRL_B)),
            Command::Swallow
        );
        assert_eq!(
            command(&mut keys, &keymap, &typed(b"o")),
            Command::Swallow,
            "the unbound default is swallowed, not passed to the pane",
        );
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
        use sprag_terminal::SplitDir;

        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(
            acted(&mut keys, b"%"),
            Command::Act(BoundAction::SplitWindow {
                dir: SplitDir::Horizontal,
                before: false
            }),
        );
        assert_eq!(keys, PrefixMode::ToPane, "and the mode is one key long");
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(
            acted(&mut keys, b"\""),
            Command::Act(BoundAction::SplitWindow {
                dir: SplitDir::Vertical,
                before: false
            }),
        );
    }

    /// `prefix o` moves to the next pane — tmux's `select-pane -t :.+`, and the only way to reach a
    /// pane this client has just made.
    #[test]
    fn the_prefix_then_o_moves_to_the_next_pane() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        assert_eq!(
            acted(&mut keys, b"o"),
            Command::Act(BoundAction::SelectNextPane)
        );
    }

    /// The split keys are the client's only BEHIND the prefix. Typed bare they are ordinary
    /// characters, and they are characters a shell sees constantly — `%` in a prompt, `"` around
    /// every quoted string.
    ///
    /// REVERT-PROOF for the prefix gate itself: route these without it and typing a quoted argument
    /// would split the window mid-word.
    #[test]
    fn the_split_keys_are_ordinary_characters_without_the_prefix() {
        let mut keys = PrefixMode::ToPane;
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
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        // Ctrl-O, the C0 byte.
        assert_eq!(acted(&mut keys, &[0x0f]), Command::Swallow);
        assert_eq!(keys, PrefixMode::ToPane);
    }

    /// A key the wire cannot spell still ENDS the prefix mode — it is one key long whatever that
    /// key turns out to be.
    ///
    /// Reached with a hand-built event because no terminal sends a bare modifier; `wire_key` drops
    /// it, and a client that left the mode armed would treat the user's NEXT keystroke as a command.
    #[test]
    fn an_unspellable_key_after_the_prefix_still_ends_the_mode() {
        let mut keys = PrefixMode::ToPane;
        assert_eq!(acted(&mut keys, CTRL_B), Command::Swallow);
        let bare_shift = KeyEvent {
            key: termwiz::input::KeyCode::Shift,
            modifiers: termwiz::input::Modifiers::NONE,
        };
        assert_eq!(
            command(&mut keys, &Keymap::default(), &bare_shift),
            Command::Swallow
        );
        assert_eq!(keys, PrefixMode::ToPane, "the mode ended");
        // ...and the very next ordinary key is the program's again, not a command.
        assert_eq!(routed(&mut keys, b"d").as_deref(), Some("d"));
    }
}
