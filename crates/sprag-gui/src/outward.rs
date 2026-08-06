//! A message FOLLOWING the person out of the WINDOW — the copy that reaches their desktop once the
//! strip it would have been read on is behind something else.
//!
//! # The defect this module removes, measured before it was written
//!
//! R319 gave `sprag-tui` an outward copy: a message arriving while the person is not looking at
//! their terminal is also written to that terminal as an `OSC 9`. `sprag-gui` got nothing, and
//! `notify-outward` became a setting one client obeyed and the other silently ignored. Driven
//! through the shipped binary under the smoke's recorder
//! (`check_a_message_follows_the_person_out_of_the_window`): with the window blurred, an ALERT was
//! delivered, the strip painted it, and the desktop received **nothing at all**. That is R318's
//! *"every layer carried it and nothing was obliged to read it"* one front further along — and the
//! layer that was missing here is the one the person was actually in a position to see.
//!
//! # What a WINDOW can do that a terminal cannot
//!
//! [`sprag_host::outward`] holds the policy and `sprag-tui` holds the terminal's transport; this is
//! the window's. The two answers differ because the two clients are in different places:
//!
//! * **Where the person is** comes from the WINDOW MANAGER, not from a terminal that may or may not
//!   implement DEC 1004. `pinion_core::window_focus_state::os_focused_window()` is the same read
//!   [`crate::focus_report`] already intersects a pane's focus with, so this client pays nothing new
//!   for it and inherits its per-WINDOW accuracy: a tear-off floating pane and the main tiling
//!   window are distinct, and the person is away only when the WM has activated neither.
//! * **The notification is the DESKTOP's**, because this client is not the one people run over ssh.
//!   That was the stated reason `sprag-tui` sends an OSC instead — its machine is a server with
//!   nobody in front of it — and it does not apply to a process that has opened a window on
//!   somebody's screen. A window is proof of a display in a way an environment variable is not.
//!
//! # The rival
//!
//! herdr (`9a4ce5e1`) had the OS-native half sprag did not, and this module exists because of them.
//! `platform::show_desktop_notification` shells out to `notify-send`, which is the same mechanism
//! chosen here for the same reason (every desktop ships it; a D-Bus client would put an async stack
//! inside a synchronous renderer). Where this goes past them, each read off their source rather than
//! assumed:
//!
//! * **The URGENCY survives.** Their argv is `notify-send -- <title> [body]` and carries no `-u` at
//!   all, so an agent that says *a person is needed* and a routine "done" arrive on the desktop
//!   identical. Here [`sprag_host::outward::urgency_of`] — the SAME projection the terminal client
//!   renders as kitty's `u=` digit — becomes `-u critical`, and sprag's alert (the one message that
//!   waits for a keystroke rather than a clock) is the one that asks for it.
//! * **Nothing blocks the loop that draws.** `run_notification_command` calls `Command::status()`,
//!   which waits for the notifier to EXIT, from inside `run_client_loop` — the same task that writes
//!   their frames to stdout. A notification daemon that is slow to answer therefore stalls their
//!   rendering. Here the frame hook does a channel send and returns; one thread owns the spawn and
//!   the reap, which is the arrangement R318 arrived at for the attention router and for its reason.
//! * **The person is asked about on EVERY path.** Their focus suppression
//!   (`active_tab_suppresses_notifications`) is applied by the agent-state path and not by
//!   `handle_notification_show_api`, so the one surface a script drives toasts a person looking
//!   straight at it. Here there is one seam — every message this client shows goes through
//!   [`follow`] — so the policy cannot be bypassed by adding a producer.
//! * **Terminal and desktop are not an either/or.** Their `ToastDelivery` picks one: a user who
//!   chooses `System` gives up the in-terminal toast. sprag has no such setting to get wrong,
//!   because each front does what its own medium can under one policy word.
//!
//! And the honest trade the other way, stated first in the register and repeated here: their
//! notifier is written for three platforms and this one is `notify-send`, so a Windows or macOS
//! `sprag-gui` copies nothing out yet. The seam for that is [`Desk::argv`] and the thread behind it,
//! not this module's shape.

use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};

use pinion_core::reactive::Owner;
use sprag_host::options::Options;
use sprag_host::outward::{Forward, Person, follows, urgency_of};
use sprag_host::report::Announcement;

/// The desktop's notification program.
///
/// Named rather than configurable, and that is a decision. A `notify-command` option would be a
/// second way to spell something the desktop already standardises, and the thing it would buy — a
/// test seam — is better had by putting a recorder on `PATH`, which is what the smoke does and which
/// exercises the argv the shipped binary really builds rather than one an injection point supplied.
const NOTIFIER: &str = "notify-send";

/// The `Owner::cache` key holding this client's notifier thread.
const DESK_KEY: &str = "sprag_gui.outward_desk";

/// This client's route to the desktop: a channel to the one thread allowed to run a notifier.
///
/// # Why a thread and not a call
///
/// Because the caller is the frame hook. Spawning a process is a `fork`/`exec` and WAITING for one
/// is unbounded — the notification daemon decides when it answers — and both would be paid on the
/// loop that draws somebody's panes. That is the rival's actual arrangement (see the module docs)
/// and it is the class of mistake this codebase already has two rules about: nothing expensive on
/// the PTY reader thread, and never a lock across I/O.
///
/// So the hook does a channel send, which is lock-free per sender, and one thread does the spawn and
/// the REAP. The reap is not tidiness: a notifier nobody waits for becomes a zombie, one per message,
/// for the life of a window that may be open for days.
pub(crate) struct Desk {
    /// The queue of ARGUMENTS — the notifier's own name is [`NOTIFIER`] and is spelled by the thread
    /// that runs it, so a queued message cannot be one this client has no program for. The first
    /// draft queued the whole argv and the thread split the program back off it, which left a
    /// "queued an empty argv" arm that nothing could ever produce and no test could ever drive.
    ///
    /// `None` when this client could not start its notifier thread — a state nothing else in the
    /// client should have to think about, so [`Desk::send`] drops the message exactly as a failed
    /// write does in the terminal client.
    outbox: Option<Sender<Vec<String>>>,
}

impl Desk {
    /// Start the notifier thread.
    ///
    /// The thread ends when this `Desk` drops, because the receiver's loop ends when the last sender
    /// goes — no shutdown flag, no join, nothing to forget.
    fn start() -> Self {
        let (outbox, inbox) = channel::<Vec<String>>();
        let spawned = std::thread::Builder::new()
            .name("sprag-outward".to_owned())
            .spawn(move || {
                for arguments in inbox {
                    // Both halves here, and the WAIT is why this is a thread: an unwaited child is a
                    // zombie and a waited one is an unbounded block.
                    if let Ok(mut child) = Command::new(NOTIFIER)
                        .args(arguments)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        let _ = child.wait();
                    }
                }
            });
        Self {
            outbox: spawned.ok().map(|_| outbox),
        }
    }

    /// Hand one message's arguments to the notifier thread, dropping them if there is nobody to
    /// take them.
    ///
    /// A failure is dropped for the terminal client's reason: the only place this client could
    /// report it is the strip it is painting the message onto, and a person whose desktop will not
    /// take a notification is not helped by a second sentence in the window they are not looking at.
    fn send(&self, arguments: Vec<String>) {
        if let Some(outbox) = &self.outbox {
            let _ = outbox.send(arguments);
        }
    }

    /// The ARGUMENTS that ask the desktop to show `announcement`, as a pure function of it.
    ///
    /// These are exactly what the smoke's recorder reads as `"$@"`, which is deliberate: a test and
    /// a running client should be looking at the same list, not at one that differs by a leading
    /// program name only one of them can see.
    ///
    /// Pure so the whole shape is testable without a desktop, a window or a subprocess — the seam
    /// the rival's `detect_backend` does not have and the reason their equivalent can only be tested
    /// through an injected `Command` builder.
    ///
    /// The SUMMARY names the session, exactly as the terminal client's sentence does and as the
    /// session rail beside the strip does, so a person with four sessions is told which one wants
    /// them. `--` separates it from the flags: a session name is the user's own string and could
    /// begin with a dash, which without the terminator `notify-send` would read as an option and
    /// refuse — the same class of input [`sprag_host::report::MessageText`] exists for, handled at
    /// the boundary that cares about it.
    fn argv(session: &str, announcement: &Announcement) -> Vec<String> {
        vec![
            // The application this came from, so a desktop that groups or themes by app can, and so
            // the person reads sprag's name rather than the notifier's.
            "-a".to_owned(),
            "sprag".to_owned(),
            "-u".to_owned(),
            urgency_of(announcement.severity).word().to_owned(),
            "--".to_owned(),
            format!("sprag: {session}"),
            announcement.text.as_str().to_owned(),
        ]
    }
}

/// This client's [`Desk`], started on first use and kept for the life of the client.
fn use_desk() -> std::rc::Rc<Desk> {
    Owner::current()
        .expect("use_desk() requires an active Owner scope")
        .cache(DESK_KEY, Desk::start)
}

/// Where the person is, as the window manager reports it.
///
/// `None` when the POLICY does not need an answer, which is the distinction
/// [`sprag_host::outward::follows`] rests on: a client that did not ask must not be readable as one
/// that asked and was told the person is here.
///
/// When it does need one, the answer is *is ANY window of this client activated* — because a person
/// who tore a pane off into its own window and is reading that one has not left. That is the
/// per-window accuracy [`crate::focus_report`]'s own docs argue for, arrived at here from the same
/// read; a single app-wide bool could not express it.
fn person(policy: Forward, os_focused: Option<&str>) -> Option<Person> {
    policy.needs_focus().then(|| {
        if os_focused.is_some() {
            Person::Here
        } else {
            Person::Away
        }
    })
}

/// Copy `announcement` out to this machine's desktop if the policy and the window manager call for
/// it.
///
/// # Nothing is held between calls, and that is the shape of this function
///
/// Not the policy, not the session, not where the person was. R319's own finding, applied before it
/// could be repeated: this client re-reads `config.toml` through [`crate::keys::ClientKeys`] and it
/// can change which session it is viewing without exiting, so anything cached here would go stale
/// against the strip painted beside it. Both are read at the message, from the same sources the
/// frame is drawn from.
///
/// # What does NOT come through here, deliberately
///
/// A message this client builds for its OWN keyboard — [`crate::message::show`], which every bound
/// action's [`Report`](sprag_host::report::Report) ends at. Only a message that ARRIVED
/// ([`crate::slotview::SlotView::take_message`]: somebody else's `display-message`, a pane child's
/// own notification) is copied out. The terminal client draws the same line for the same reason and
/// this front makes it sharper: a keystroke reached this window, so the window manager had given it
/// focus, so the person was here to read the answer. Forwarding it would be telling somebody about
/// the key they just pressed.
pub(crate) fn follow(session: &str, announcement: &Announcement, options: &Options) {
    let policy = Forward::of(options);
    let os_focused = pinion_core::window_focus_state::os_focused_window();
    if !follows(policy, person(policy, os_focused.as_deref())) {
        return;
    }
    use_desk().send(Desk::argv(session, announcement));
}

#[cfg(test)]
mod tests {
    use sprag_host::outward::{Forward, Person};
    use sprag_host::report::{Announcement, MessageText, Severity};

    use super::{Desk, person};

    fn said(text: &str, severity: Severity) -> Announcement {
        Announcement {
            text: MessageText::parse(text).expect("a legal message"),
            severity,
        }
    }

    /// **The person is away exactly when the window manager has activated no window of this
    /// client** — and a policy that does not ask gets no answer rather than a guess.
    ///
    /// The `None` row is the load-bearing one: it is what keeps a client on `off` or `always` from
    /// being read as a client that asked and was told the person is here.
    #[test]
    fn the_window_manager_answers_only_the_policy_that_asked() {
        assert_eq!(
            person(Forward::Unfocused, Some("main")),
            Some(Person::Here),
            "an activated window means somebody is looking",
        );
        assert_eq!(
            person(Forward::Unfocused, Some("pane-2")),
            Some(Person::Here),
            "and a TORN-OFF window is this client's too — the person has not left",
        );
        assert_eq!(
            person(Forward::Unfocused, None),
            Some(Person::Away),
            "no window of this client is activated: the person is elsewhere",
        );
        for policy in [Forward::Off, Forward::Always] {
            assert_eq!(
                person(policy, None),
                None,
                "{policy:?} does not ask, so it has no answer to be stale",
            );
            assert_eq!(person(policy, Some("main")), None, "{policy:?}");
        }
    }

    /// The whole argv, pinned — because it is the thing the person's desktop actually receives, and
    /// every element of it is a decision this module made.
    #[test]
    fn the_notifier_is_asked_for_the_words_the_session_and_an_urgency() {
        assert_eq!(
            Desk::argv("work", &said("pane 3: done", Severity::Note)),
            [
                "-a",
                "sprag",
                "-u",
                "normal",
                "--",
                "sprag: work",
                "pane 3: done",
            ],
        );
    }

    /// **An ALERT asks the desktop for the critical urgency**, which is the property the rival's
    /// argv cannot express at all.
    ///
    /// Asserted over the whole severity set rather than on the interesting arm, so a fourth severity
    /// could not be added without deciding what a person's desktop should be told.
    #[test]
    fn the_severity_reaches_the_desktop_as_an_urgency() {
        for severity in Severity::ALL {
            let want = match severity {
                Severity::Note | Severity::Warn => "normal",
                Severity::Alert => "critical",
            };
            let argv = Desk::argv("work", &said("something happened", severity));
            let urgency = argv
                .windows(2)
                .find(|pair| pair[0] == "-u")
                .map(|pair| pair[1].clone())
                .unwrap_or_default();
            assert_eq!(urgency, want, "{severity:?}");
        }
    }

    /// A session name that begins with a dash reaches the notifier as a SUMMARY and not as a flag —
    /// the terminator is what makes that true, and a user may name a session anything.
    #[test]
    fn a_session_named_like_a_flag_is_still_a_summary() {
        let argv = Desk::argv("--help", &said("done", Severity::Note));
        let terminator = argv
            .iter()
            .position(|argument| argument == "--")
            .expect("the argv separates flags from text");
        assert!(
            argv[terminator + 1].contains("--help"),
            "the name is on the text side of the terminator: {argv:?}",
        );
    }
}
