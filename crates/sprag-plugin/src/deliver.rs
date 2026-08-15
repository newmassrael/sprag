//! Getting text INTO a pane and knowing it arrived.
//!
//! [`PaneAccess::inject`] writes to a pseudoterminal, and a pty takes bytes whether or not the
//! program behind it is ready to read them meaningfully. A long-lived interactive agent has a
//! window during which it does exactly that: it is up, it has a tty, it reports itself idle, and it
//! discards what you type because its own input layer has not finished starting. The write returns
//! success and the text is gone.
//!
//! That is not a hypothesis. It was measured while supervising a real agent session against a rival
//! multiplexer: text sent the instant the agent reported `idle` vanished with no error, the prompt
//! stayed empty, and the supervising machine then waited forever for a turn it had never actually
//! started. The prescription that worked — inject, read the screen back, re-inject until the text
//! is visible, and only THEN press Enter — is what this module is, written once so that every
//! plugin author does not discover it separately.
//!
//! ## ⚠⚠⚠ And the screen is where the pty puts them too
//!
//! That prescription has a hole the paragraph above walks straight past. **A pseudoterminal echoes
//! what is written to it**, so on a pane whose line discipline is echoing, the text appears the
//! instant it reaches the device — before the program has read a byte, and whether or not it ever
//! will. A read-back that finds it has learned that the TERMINAL is alive.
//!
//! Measured, over a pane running `sleep 60`: `Confirmed { attempts: 1 }`, in 20 ms. The peer had
//! read nothing and was going to read nothing. **Every fixture in this module's own tests began
//! with `stty raw -echo`**, which takes the kernel out of the picture, and that is why nothing here
//! ever asked the question.
//!
//! So a delivery now says which evidence it has, by asking the kernel who echoes
//! ([`PaneEcho`], through
//! [`PaneTerminalModes::pane_echo`](crate::access::PaneTerminalModes::pane_echo)):
//! [`Delivered::Confirmed`] where the program painted the text, and
//! [`Delivered::OnScreenOnly`] where it is on the screen and nothing here can say who put it there.
//! ⚠ The weaker answer is not a failure — for a cooked one-shot peer (`claude -p`) it is the best
//! any observer of a screen can honestly claim, and the delivery still proceeds. What changed is
//! that a caller is no longer told it was proved.
//!
//! ## ⚠⚠⚠ And a screen can be carrying the needle before a byte goes in
//!
//! Both paragraphs above are about WHO painted the text. There is a third question underneath them
//! that neither asks: **was it painted by THIS delivery?** A read-back is a predicate over the
//! present, and *"the needle is on the screen"* is satisfied by a screen that was already carrying
//! it — a supervisor sending the SAME prompt twice, an agent whose transcript still shows the last
//! one, a marker a program prints on every turn.
//!
//! It is not a corner. Measured live, an outer loop's `turn_prompt` is a fixed sentence, so from the
//! second turn on the confirmation needle is a string the agent's own transcript is still showing —
//! and the delivery came back `Confirmed` **in one poll, before the program had read a byte**. The
//! [`Delivery::then_press`] then went in on top of the unread text, which is a pty read of
//! `…prompt…\r` rather than a prompt followed by a keystroke, and a live `claude` kept the whole
//! thing in its composer and started no turn. Three live runs, three times.
//!
//! So the wait is against a BASELINE: the pane's collapsed screen is read once before the first
//! injection, and the needle counts as arrived only when the screen carrying it is **not the screen
//! that was there before**. That is [`ReadyWhen::Prints`](crate::readiness::ReadyWhen::Prints)'
//! argument — *a condition satisfied by what was already true when you started is not evidence* —
//! applied at the other end of the same turn.
//!
//! ⚠ It is a CHANGE and not `Prints`' occurrence COUNT, deliberately. A count's residue is that
//! text scrolling off lowers it, and the thing being delivered here is often long enough to scroll
//! the old copy away as it lands — which would be a false NEGATIVE whose price is a retry that
//! doubles the text and then a refusal. The residue this takes instead is stated: a screen that
//! changes for an unrelated reason inside the grace (a peer still printing, a footer with a clock)
//! is a change this cannot tell from the program taking the text. It narrows the hole rather than
//! closing it, and what closes it is a peer that paints what it read — which every agent CLI does.
//!
//! ## Why this is not a method on `PaneAccess`
//!
//! It waits, so it is bounded, so it must be cancellable, so it needs the run-scoped
//! [`RunContext`] — and `PaneAccess` is the PANE-scoped surface. The crate already made this
//! decision once, when cancellation was bolted onto `PaneAccess` and then moved out; `poll_until`
//! lives beside `RunContext` for the same reason and this is its second caller.
//!
//! ## The one hazard, named
//!
//! A retry can DOUBLE the text: if the pane took the first injection but echoed it more slowly than
//! [`Delivery::echo_timeout`], the second injection lands on top of the first. There is no way to
//! tell that apart from a swallowed write by looking at the screen, because both look like "not
//! there yet" — so the bound is a real trade and not an oversight. Size `echo_timeout` above the
//! pane's echo latency and the trade is bought; the default is generous for that reason, and the
//! attempt count is small.

use std::time::Duration;

use sprag_terminal::{PaneEcho, PaneId};

use crate::access::{KeyStroke, PaneAccess, PaneError, Written};
use crate::run::{POLL_INTERVAL, RunContext};

/// How long to wait for a pane to show text that was injected into it, before deciding the pane
/// never took it.
///
/// Two seconds: an echo is a round trip through a pty and a program's input layer, which is
/// microseconds when the program is reading and unbounded when it is starting up. The number is
/// sized for the RETRY hazard rather than for the echo — see the module docs — so it is
/// deliberately far above any echo this project has measured.
pub const DEFAULT_ECHO_TIMEOUT: Duration = Duration::from_secs(2);

/// How many times [`deliver`] injects before giving up.
///
/// Three. The measured window a starting agent swallows input in closed within 500 ms in every
/// observation, so one retry would very likely do; two spare attempts against a
/// [`DEFAULT_ECHO_TIMEOUT`]-long grace each is the cheap side of a bound whose other side is
/// waiting forever for a turn that never started.
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// How to deliver text to a pane, and what to do once it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// What must appear on the pane's screen for the text to count as arrived. `None` means the
    /// text itself.
    ///
    /// Overridable because an agent's prompt box is a BOX: a long line wraps inside it and the
    /// border characters land between the halves, so the pane's text contains the prompt in pieces
    /// and not as one run. A caller delivering something longer than a pane is wide should confirm
    /// on a leading fragment of it, and this is where that is said rather than in each caller.
    pub confirm: Option<String>,
    /// How long to wait for it to appear after one injection. See [`DEFAULT_ECHO_TIMEOUT`].
    pub echo_timeout: Duration,
    /// How many injections to make in total (at least one). See [`DEFAULT_ATTEMPTS`].
    pub attempts: u32,
    /// Keys to send once — and only once — the text is CONFIRMED on the screen.
    ///
    /// The submit is here rather than left to the caller because the ordering is the whole point:
    /// an Enter sent beside a swallowed prompt submits an empty line, which an agent answers, which
    /// is worse than sending nothing at all. Defaults to Enter; give an empty list to deliver text
    /// without submitting it.
    pub then_press: Vec<KeyStroke>,
}

impl Delivery {
    /// The defaults: confirm on the text itself, a generous echo grace, three attempts, submit with
    /// Enter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            confirm: None,
            echo_timeout: DEFAULT_ECHO_TIMEOUT,
            attempts: DEFAULT_ATTEMPTS,
            then_press: vec![KeyStroke::named("Enter")],
        }
    }

    /// The defaults, but confirmed on `needle` instead of on the whole text.
    #[must_use]
    pub fn confirmed_on(needle: impl Into<String>) -> Self {
        Self {
            confirm: Some(needle.into()),
            ..Self::new()
        }
    }

    /// The defaults, but nothing is pressed after the text lands.
    #[must_use]
    pub fn without_submitting(mut self) -> Self {
        self.then_press.clear();
        self
    }
}

impl Default for Delivery {
    fn default() -> Self {
        Self::new()
    }
}

/// How a [`deliver`] ended.
///
/// Three outcomes and not a `bool`, because "the pane never took it" is a thing a supervisor must
/// be able to act on — hand the pane to a person — and is not the same as an error. An unknown
/// pane or an unencodable key IS an error and comes back as [`PaneError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// The text is on the pane's screen AND THE PROGRAM IS WHAT PUT IT THERE — the pane's echo is
    /// off, so nothing but the program could have painted it. `attempts` is how many injections it
    /// took, so a caller that wants to know whether this pane swallows input can find out.
    ///
    /// ⚠⚠ **AND THE SCREEN IS ONE THIS DELIVERY CHANGED**, which is the other half of *put it
    /// there*: a needle the pane was already carrying says nothing about the bytes just written,
    /// whoever painted the old copy. See the module docs' third hazard.
    Confirmed { attempts: u32, written: Written },
    /// The text is on the pane's screen and **nothing here can say the program is what put it
    /// there**, so it is not evidence the program read a byte.
    ///
    /// # ⚠⚠⚠ Why this had to become a separate answer
    ///
    /// This module exists to not be fooled by a pseudoterminal, and it was: with the pane's echo
    /// ON, the line discipline paints every byte the instant it reaches the device — before the
    /// program has read one and whether or not it ever will. Measured, over a pane running
    /// `sleep 60`: `Confirmed { attempts: 1 }`, in 20 ms, with the peer having read nothing and
    /// about to read nothing. **Every fixture in this module's own tests disabled the echo**
    /// (`stty raw -echo`), which is why nothing ever asked.
    ///
    /// `echo` carries the reading that decided it — [`PaneEcho::ByTheTerminal`] where the terminal
    /// is what echoes, and `None` where the host offers no such capability or the platform's device
    /// would not say. Two different reasons for one epistemic state, kept apart because only the
    /// first also tells the caller their pane is in COOKED mode.
    ///
    /// ⚠ The submit ([`Delivery::then_press`]) is still sent for this answer, and deliberately: in
    /// cooked mode the newline is what makes the line readable at all, so withholding it would
    /// guarantee the non-delivery it is meant to prevent. The press is withheld only where the text
    /// is demonstrably ABSENT — see [`Unconfirmed`](Self::Unconfirmed).
    OnScreenOnly {
        attempts: u32,
        written: Written,
        echo: Option<PaneEcho>,
    },
    /// Every attempt was written and none of them ever appeared. The bytes went to the pty; the
    /// program behind it did not show them.
    Unconfirmed { attempts: u32, written: Written },
    /// THE RUN ENDED part-way — cancelled, or out of time. Nothing is claimed about what the pane
    /// holds. Which of the two it was is the [`crate::run::RunContext`]'s to answer.
    Stopped { attempts: u32, written: Written },
}

impl Delivered {
    /// Whether the PROGRAM is known to be holding the text.
    ///
    /// ⚠ False for [`OnScreenOnly`](Self::OnScreenOnly), and that is the whole point of the
    /// distinction: a caller that treats text on a cooked pane's screen as delivery is reading the
    /// terminal's own echo as the program's acknowledgement.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }

    /// Whether the text is on the pane's screen at all, however it got there.
    ///
    /// The weaker question, named so that a caller that genuinely wants it does not reach for
    /// [`is_confirmed`](Self::is_confirmed) and get the strong claim by accident.
    #[must_use]
    pub const fn is_on_screen(self) -> bool {
        matches!(self, Self::Confirmed { .. } | Self::OnScreenOnly { .. })
    }

    /// How many bytes reached the pty across every attempt — what a plugin charges as its
    /// [`Cost`](crate::plugin::Cost), since a swallowed write cost the same as a landed one.
    pub const fn written(self) -> Written {
        match self {
            Self::Confirmed { written, .. }
            | Self::OnScreenOnly { written, .. }
            | Self::Unconfirmed { written, .. }
            | Self::Stopped { written, .. } => written,
        }
    }
}

/// Inject `text` into `pane` and confirm the pane took it, re-injecting until it does.
///
/// The read-back is [`PaneAccess::pane_collapsed`] — the pane's rows joined with nothing between
/// them — so text the pane WRAPPED still matches. What it cannot see through is a border drawn
/// between the halves, which is what [`Delivery::confirm`] is for.
///
/// Returns as soon as the text is visible; [`Delivery::then_press`] is sent only after that, so an
/// Enter can never submit an empty prompt.
///
/// ⚠⚠⚠ **VISIBLE MEANS VISIBLE ON A SCREEN THIS DELIVERY CHANGED.** The pane's collapsed screen is
/// read once before the first injection, and a read-back that finds the needle on that same screen
/// is not evidence — see the module docs. Without it, a caller who sends the same text twice gets
/// the second delivery confirmed off the first one's echo, and the submit lands on text no program
/// has read.
///
/// # Errors
///
/// [`PaneError`] when the pane is unknown, a key cannot be encoded, or a write fails — the same
/// causes [`PaneAccess::inject`] has, and none of them are "the pane did not take it", which is
/// [`Delivered::Unconfirmed`].
pub fn deliver(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    text: &str,
    spec: &Delivery,
) -> Result<Delivered, PaneError> {
    let needle = spec.confirm.as_deref().unwrap_or(text);
    let keys = KeyStroke::text(text);
    let mut written = 0_u64;
    let mut attempts = 0_u32;
    // ⚠⚠⚠ THE BASELINE, taken before a byte goes in — see the module docs. Read ONCE for the whole
    // delivery rather than per attempt, so a paint that arrives late (the first injection landing
    // while the second is being made) still confirms instead of being compared against a screen it
    // had already moved.
    let before = panes.pane_collapsed(pane);

    for _ in 0..spec.attempts.max(1) {
        if run.stopped() {
            return Ok(Delivered::Stopped {
                attempts,
                written: Written::of(written),
            });
        }
        attempts += 1;
        written += panes.inject(pane, &keys)?.bytes();
        match await_text(
            panes,
            run,
            pane,
            needle,
            spec.echo_timeout,
            before.as_deref(),
        ) {
            Seen::Stopped => {
                return Ok(Delivered::Stopped {
                    attempts,
                    written: Written::of(written),
                });
            }
            Seen::Yes => {
                // Only now: the text is on the screen, so a submit submits the text rather than an
                // empty line. Sent for BOTH on-screen answers — see `Delivered::OnScreenOnly`.
                if !spec.then_press.is_empty() {
                    written += panes.inject(pane, &spec.then_press)?.bytes();
                }
                let written = Written::of(written);
                // ⚠⚠ THE READING IS TAKEN HERE, not at the top: a program that takes its terminal
                // off echo does it during the same startup this call is racing, so an answer read
                // before the injection would be about the terminal the pane USED to have.
                return Ok(match painter(panes, pane) {
                    Some(PaneEcho::ByTheProgram) => Delivered::Confirmed { attempts, written },
                    echo => Delivered::OnScreenOnly {
                        attempts,
                        written,
                        echo,
                    },
                });
            }
            Seen::No => {}
        }
    }
    Ok(Delivered::Unconfirmed {
        attempts,
        written: Written::of(written),
    })
}

/// Who paints what is written into `pane`, or `None` where nothing can say.
///
/// `None` covers two hosts that are the same to a caller and different to a reader of this code: a
/// [`PaneAccess`] that offers no [`PaneInputEcho`](crate::access::PaneInputEcho) at all, and one
/// whose platform device would not
/// answer. Both mean the same thing here — **no evidence** — which is why they collapse to one
/// value rather than to a `Confirmed` that would be a guess.
fn painter(panes: &dyn PaneAccess, pane: PaneId) -> Option<PaneEcho> {
    panes.terminal_modes()?.pane_echo(pane)
}

/// Whether a pane's child has produced ANY output yet — the cheapest honest readiness signal there
/// is.
///
/// A program that has painted has certainly opened its terminal and set its modes, which is the
/// thing a pane fresh out of [`PaneLifecycle::spawn`](crate::access::PaneLifecycle::spawn) has not
/// necessarily done. It is a sufficient condition and NOT a necessary one, which is why [`deliver`]
/// does not gate on it: a pane running `cat` never paints until you type, so waiting for paint
/// before injecting would hang on the simplest peer there is.
///
/// It is here, named, because the alternative is every plugin inventing a readiness heuristic of
/// its own — and the one heuristic that was tried against a rival ("is the foreground process a
/// lone shell?") passed while the pane still refused, which is what a plausible predicate measuring
/// an ADJACENT fact looks like from the inside.
#[must_use]
pub fn has_painted(panes: &dyn PaneAccess, pane: PaneId) -> bool {
    panes
        .pane_rows(pane)
        .is_some_and(|rows| rows.iter().any(|row| row.generation > 0))
}

/// What a bounded wait for text on a pane saw.
enum Seen {
    Yes,
    No,
    /// The run ended under it — cancelled, or past its deadline.
    Stopped,
}

/// Wait, bounded by `timeout` AND by the run's own deadline, for `needle` to appear on a pane whose
/// screen is **no longer the one `before` recorded**.
///
/// ⚠⚠⚠ `before` is the whole claim and not a refinement of it. `Some(screen)` is what the pane was
/// showing when the delivery began; a read-back equal to it has learned that nothing has happened
/// yet, however many times the needle occurs in it. `None` means the pane could not be read at the
/// baseline, which the loop below answers the same way it answers a pane that has gone away.
///
/// ⚠⚠ **THE SECOND BOUNDED WAIT IN THIS CRATE**, and it is here rather than routed through
/// [`poll_until`](crate::run::poll_until) because it needs a THREE-way predicate: a pane that has
/// gone away can never show anything, and saying so at once is not the same answer as "not yet".
/// What it must not have of its own is the STOP condition — a wait that knew about cancellation and
/// not about the deadline would let a delivery outlive a run that is over, which is exactly the
/// hole the deadline was added to close. So both waits ask
/// [`RunContext::stopped`](crate::run::RunContext::stopped), which is the one definition of
/// *the run is over*.
fn await_text(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    needle: &str,
    timeout: Duration,
    before: Option<&str>,
) -> Seen {
    let start = std::time::Instant::now();
    loop {
        if run.stopped() {
            return Seen::Stopped;
        }
        // An unknown pane can never show anything, and saying so at once beats spending the whole
        // grace on it — the caller's next `inject` will report `UnknownPane` properly.
        match panes.pane_collapsed(pane) {
            Some(text) if text.contains(needle) && Some(text.as_str()) != before => {
                return Seen::Yes;
            }
            None => return Seen::No,
            Some(_) => {}
        }
        if start.elapsed() >= timeout {
            return Seen::No;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{PaneRow, PaneTerminalModes, WorkspacePaneAccess};
    use sprag_terminal::PaneEndOfInput;
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// The peer's own "I have configured my terminal" marker.
    ///
    /// Without it every test here would be a race: a `sh -c` peer takes milliseconds to reach its
    /// `stty`, and an injection that arrives first is echoed by the LINE DISCIPLINE — so the pane
    /// shows text the child never took, and a test about swallowed input silently becomes a test
    /// about the kernel's echo. Found by running it: the first version of
    /// `a_swallowed_injection_reports_success_and_a_confirmed_delivery_does_not` failed with the
    /// text plainly on the screen.
    const GO: &str = "GO";

    /// A peer in RAW mode with echo off, so what reaches the pane's screen is only what the CHILD
    /// chose to print — and so a byte reaches the child the instant it is written, with no line
    /// discipline holding it back for a newline that a confirmed delivery deliberately has not sent
    /// yet.
    fn peer(after_go: &str) -> String {
        format!("stty raw -echo; printf '{GO}'; {after_go}")
    }

    /// A peer that SWALLOWS its first five bytes — one `hello` — and echoes everything after them.
    ///
    /// `dd` reads and discards exactly five bytes, which makes the measured failure deterministic
    /// rather than something to wait for: the first injection is always lost and the second is
    /// always seen. This is a test about the retry, not about a race.
    fn swallows_five() -> String {
        peer("dd bs=1 count=5 of=/dev/null 2>/dev/null; exec cat")
    }

    fn access(script: &str) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .expect("the workspace")
            .spawn(command, "peer".to_string(), 40, 6)
            .expect("spawn the pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    /// Wait (bounded) for `needle` on the pane, answering whether it arrived.
    fn shows(access: &WorkspacePaneAccess, pane: PaneId, needle: &str, within: Duration) -> bool {
        let start = Instant::now();
        loop {
            if access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(needle))
            {
                return true;
            }
            if start.elapsed() >= within {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A peer that has said [`GO`], so nothing below is racing its `stty`.
    fn ready_peer(script: &str) -> (WorkspacePaneAccess, PaneId) {
        let (access, pane) = access(script);
        assert!(
            shows(&access, pane, GO, Duration::from_secs(10)),
            "the peer never configured its terminal",
        );
        (access, pane)
    }

    /// A bare inject reports success over a pane that threw the text away; a confirmed delivery
    /// does not.
    ///
    /// Both halves in one test on purpose. The control is what makes the claim: `inject` returns a
    /// `Written` for five bytes that never arrive, and a caller reading that as delivery would wait
    /// forever for a reply to a prompt it never sent.
    #[test]
    fn a_swallowed_injection_reports_success_and_a_confirmed_delivery_does_not() {
        // THE CONTROL: one bare injection into a pane that discards it.
        let (control, pane) = ready_peer(&swallows_five());
        let receipt = control
            .inject(pane, &KeyStroke::text("hello"))
            .expect("write");
        assert_eq!(receipt.bytes(), 5, "the pty took every byte");
        assert!(
            !shows(&control, pane, "hello", Duration::from_millis(750)),
            "the write succeeded and the text is nowhere: {:?}",
            control.pane_collapsed(pane),
        );
        control.lifecycle().expect("lifecycle").close(pane);

        // THE SUBJECT: the same pane, delivered to.
        let (access, pane) = ready_peer(&swallows_five());
        let outcome = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            "hello",
            &Delivery::new().without_submitting(),
        )
        .expect("no error");
        match outcome {
            Delivered::Confirmed { attempts, written } => {
                assert_eq!(attempts, 2, "the first attempt is the swallowed one");
                assert_eq!(written.bytes(), 10, "both injections were paid for");
            }
            other => panic!("the retry must land it: {other:?}"),
        }
        assert!(shows(&access, pane, "hello", Duration::from_millis(1)));
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// A pane that takes the text first time costs exactly one injection — the retry is a fallback,
    /// not a tax on every delivery.
    #[test]
    fn a_pane_that_is_ready_takes_it_on_the_first_attempt() {
        let (access, pane) = ready_peer(&peer("exec cat"));
        let outcome = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            "ping",
            &Delivery::new().without_submitting(),
        )
        .expect("no error");
        assert_eq!(
            outcome,
            Delivered::Confirmed {
                attempts: 1,
                written: Written::of(4),
            },
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// A pane that never shows the text is reported as UNCONFIRMED rather than as an error or a
    /// success — the answer a supervisor turns into "hand this one to a person".
    #[test]
    fn a_pane_that_never_shows_it_is_unconfirmed_and_says_how_hard_it_tried() {
        let (access, pane) = ready_peer(&peer("exec cat > /dev/null"));
        let outcome = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            "hello",
            &Delivery {
                echo_timeout: Duration::from_millis(120),
                attempts: 2,
                ..Delivery::new()
            },
        )
        .expect("a pane that ignores input is not an error");
        assert_eq!(
            outcome,
            Delivered::Unconfirmed {
                attempts: 2,
                written: Written::of(10),
            },
        );
        assert!(!outcome.is_confirmed());
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PANE THAT WILL NEVER READ A BYTE USED TO COME BACK CONFIRMED**, in 20 ms, and this
    /// is the gate that says it does not any more.
    ///
    /// The peer is `sleep 60`. It has a terminal, it has printed, and it will not read for as long
    /// as this test can wait — so there is no reading of "the program took it" that is true of it.
    /// The text appears on its screen anyway, because the pane's line discipline paints every byte
    /// written to the device the instant it arrives.
    ///
    /// **This is the module's own premise turned against it.** Its docs open with a pty taking
    /// bytes "whether or not the program behind it is ready to read them meaningfully"; its answer
    /// was to read the screen back; and the screen is where the pty puts them. Every fixture around
    /// this one says `stty raw -echo` first, which removed the kernel from the picture and is why
    /// the hole survived.
    ///
    /// ⚠ THE CONTROL is the assertion that the text IS on the screen. Without it a passing gate
    /// would be indistinguishable from one where the injection simply failed, and the claim —
    /// *on the screen is not the same as taken* — needs both halves to mean anything.
    #[test]
    fn a_peer_that_never_reads_is_not_confirmed_by_its_terminals_own_echo() {
        // No `stty`: the pty's default discipline, which is what a pane running a program that
        // does not touch its terminal has. `printf` first so the peer is up before the delivery.
        let (access, pane) = access("printf 'UP\\n'; sleep 60");
        assert!(
            shows(&access, pane, "UP", Duration::from_secs(10)),
            "the peer never started",
        );

        let outcome = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            "hello",
            &Delivery::new().without_submitting(),
        )
        .expect("no error");

        assert!(
            !outcome.is_confirmed(),
            "a peer blocked in `sleep` has read nothing, so nothing may report it as holding the \
             text: {outcome:?}",
        );
        assert!(
            matches!(
                outcome,
                Delivered::OnScreenOnly {
                    attempts: 1,
                    echo: Some(PaneEcho::ByTheTerminal),
                    ..
                },
            ),
            "and the reason is the pane's own terminal, named: {outcome:?}",
        );
        // THE CONTROL: the text really is on the screen, put there by the line discipline. A gate
        // that passed because nothing arrived would prove the opposite of what this claims.
        assert!(
            shows(&access, pane, "hello", Duration::from_millis(1)),
            "the terminal painted it — that is the whole difficulty: {:?}",
            access.pane_collapsed(pane),
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A NEEDLE THE SCREEN WAS ALREADY CARRYING IS NOT THIS DELIVERY'S EVIDENCE** — the third
    /// hazard in the module docs, and the one that reached a live agent.
    ///
    /// Both peers are shown the needle BEFORE anything is written to them, which is the ordinary
    /// case rather than an exotic one: an outer loop's turn prompt is a fixed sentence, so from the
    /// second turn on the confirmation needle is a string the agent's own transcript is still
    /// showing.
    ///
    /// * **THE SUBJECT** never reads a byte (`sleep`), so nothing about the delivery can be true.
    ///   The old rule — *is the needle on the screen?* — answered YES on the first poll and
    ///   returned `Confirmed`, the answer whose own doc says THE PROGRAM PUT IT THERE, about a peer
    ///   that was going to read nothing. The submit then went in on top of unread text.
    /// * **THE CONTROL** is the same screen with a peer that DOES read. It must still confirm, or
    ///   the fix would have made *"deliver the same text twice"* impossible — which is the thing an
    ///   outer loop does on every turn.
    ///
    /// ⚠ The pair is the whole test. Without the control this passes for a build that never
    /// confirms anything; without the subject it passes for the defect.
    #[test]
    fn a_needle_the_screen_already_carried_is_not_evidence_that_this_delivery_landed() {
        /// What both peers print before a byte is written to them — the previous turn's prompt,
        /// still on the transcript.
        const ALREADY: &str = "Continue toward: pay the debt";
        /// What is delivered. Longer than the needle so a peer that reads it changes the screen.
        const PROMPT: &str = "Continue toward: pay the debt, next smallest thing";

        let deliver_over = |after_go: &str| {
            let (access, pane) = ready_peer(&peer(&format!("printf '{ALREADY}'; {after_go}")));
            // THE STAGING, asserted rather than assumed: the needle really is on the screen before
            // the delivery begins. A fixture whose `printf` had not landed yet would be measuring
            // the ordinary case and calling it the hazard.
            assert!(
                shows(&access, pane, ALREADY, Duration::from_secs(10)),
                "the peer must be showing the needle before anything is written: {:?}",
                access.pane_collapsed(pane),
            );
            let outcome = deliver(
                &access,
                &RunContext::uncancellable(),
                pane,
                PROMPT,
                &Delivery {
                    confirm: Some(ALREADY.to_owned()),
                    echo_timeout: Duration::from_millis(150),
                    attempts: 2,
                    ..Delivery::new()
                },
            )
            .expect("a peer that ignores input is not an error");
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome, screen)
        };

        let (subject, subject_screen) = deliver_over("exec sleep 60");
        assert!(
            matches!(subject, Delivered::Unconfirmed { attempts: 2, .. }),
            "⚠⚠⚠ A PEER BLOCKED IN `sleep` HAS READ NOTHING, so no reading of this delivery is \
             true — and the needle being on its screen is a fact about the previous turn. Reported \
             {subject:?} over a screen that never changed: {subject_screen:?}",
        );
        assert!(
            !subject.is_on_screen(),
            "and not the weaker answer either: `OnScreenOnly` would still send the submit, which \
             is exactly what put an Enter on top of text no program had read: {subject:?}",
        );

        let (control, control_screen) = deliver_over("exec cat");
        assert!(
            control.is_confirmed(),
            "⚠⚠⚠ THE CONTROL: a peer that READS the same text on the same screen must still be \
             confirmed. An outer loop delivers the same turn prompt every turn, so a rule that \
             refused a repeat would refuse every turn after the first. Got {control:?} over \
             {control_screen:?}",
        );
    }

    /// Readiness, in both directions — and the pane that is NOT ready still takes a delivery, which
    /// is why [`deliver`] consults this and does not gate on it.
    #[test]
    fn a_pane_that_has_painted_is_ready_and_one_that_has_not_is_still_deliverable() {
        // Nothing printed and no `stty`: the line discipline's own echo is what will show the text,
        // which is exactly the case a paint-gated delivery would have hung on.
        let (quiet, quiet_pane) = access("exec cat");
        let (loud, loud_pane) = ready_peer(&peer("exec cat"));

        assert!(
            has_painted(&loud, loud_pane),
            "a pane whose child printed has painted",
        );
        assert!(
            !has_painted(&quiet, quiet_pane),
            "a pane whose child has printed nothing has not painted",
        );
        assert!(
            !has_painted(&quiet, PaneId(9999)),
            "a pane nobody knows has not painted",
        );

        // ⚠⚠⚠ AND THE WEAKER CLAIM IS THE HONEST ONE. This fixture's own comment above says the
        // line discipline is what will show the text — and it asserted `is_confirmed()` anyway,
        // for as long as `Confirmed` covered both. It does not: a cooked pane's screen is the
        // TERMINAL's answer, so what is proved here is that the delivery went through, not that
        // `cat` read it.
        let onto_a_cooked_pane = deliver(
            &quiet,
            &RunContext::uncancellable(),
            quiet_pane,
            "x",
            &Delivery::new().without_submitting(),
        )
        .expect("no error");
        assert!(
            onto_a_cooked_pane.is_on_screen(),
            "a pane that has painted nothing is still a pane you can deliver to: \
             {onto_a_cooked_pane:?}",
        );
        assert!(
            !onto_a_cooked_pane.is_confirmed(),
            "and the terminal's own echo must never be read as the program's acknowledgement: \
             {onto_a_cooked_pane:?}",
        );
        assert!(
            matches!(
                onto_a_cooked_pane,
                Delivered::OnScreenOnly {
                    echo: Some(PaneEcho::ByTheTerminal),
                    ..
                },
            ),
            "and it says WHICH of the two reasons it cannot confirm: {onto_a_cooked_pane:?}",
        );

        quiet.lifecycle().expect("lifecycle").close(quiet_pane);
        loud.lifecycle().expect("lifecycle").close(loud_pane);
    }

    /// A `PaneAccess` that records every injection and shows the text only after `hidden_reads`
    /// read-backs — the swallowed-input window, made exact.
    struct Recorder {
        text: String,
        /// ⚠⚠⚠ **WHAT THE SCREEN IS CARRYING BEFORE A BYTE IS WRITTEN**, which is the fact
        /// [`deliver`]'s baseline is about and the one this double used to refuse to model: it
        /// answered `text` from the first read, so every gate over it confirmed a delivery on a
        /// screen that had never moved, and the defect the module's third hazard describes was
        /// invisible here by construction.
        ///
        /// Empty for a pane that starts blank; equal to [`text`](Self::text) for the pane that
        /// stages the hazard — a screen already showing the needle and never changing again.
        showing_before: String,
        hidden_reads: Mutex<u32>,
        injected: Mutex<Vec<Vec<String>>>,
        /// Raised on the first read-back AFTER an injection, so a cancel lands INSIDE the wait
        /// rather than before it.
        ///
        /// ⚠ *After an injection* rather than *on the first read* since the baseline exists: the
        /// baseline read happens before the loop's first stop check, so a flag raised on it would
        /// end the delivery having written nothing — a different arm from the one this stages.
        cancel_on_read: Option<Arc<std::sync::atomic::AtomicBool>>,
    }

    impl Recorder {
        /// A blank-screened double showing `text` once something has been injected.
        fn showing(text: &str) -> Self {
            Self {
                text: text.to_owned(),
                showing_before: String::new(),
                hidden_reads: Mutex::new(0),
                injected: Mutex::new(Vec::new()),
                cancel_on_read: None,
            }
        }

        /// One delivery against this double, with a short grace and no retries.
        fn deliver_once(self, text: &str, confirm: Option<&str>) -> Delivered {
            let spec = Delivery {
                echo_timeout: Duration::from_millis(1),
                attempts: 1,
                ..confirm.map_or_else(Delivery::new, Delivery::confirmed_on)
            };
            deliver(&self, &RunContext::uncancellable(), PaneId(1), text, &spec).expect("no error")
        }
    }

    impl PaneAccess for Recorder {
        fn pane_ids(&self) -> Vec<PaneId> {
            vec![PaneId(1)]
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            // ⚠ THE BASELINE READ IS NOT A READ-BACK. Nothing has been written yet, so what the
            // screen holds is whatever was there before this delivery — and neither the cancel nor
            // the swallowed-input window is about that moment.
            if self.injected.lock().expect("the log").is_empty() {
                return Some(self.showing_before.clone());
            }
            if let Some(cancel) = &self.cancel_on_read {
                cancel.store(true, std::sync::atomic::Ordering::Release);
            }
            let mut left = self.hidden_reads.lock().expect("the counter");
            if *left > 0 {
                *left -= 1;
                return Some(self.showing_before.clone());
            }
            Some(self.text.clone())
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            None
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            Some(false)
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn inject(&self, _id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
            self.injected
                .lock()
                .expect("the log")
                .push(keys.iter().map(|k| k.key.clone()).collect());
            Ok(Written::of(keys.len() as u64))
        }
        fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
            Some(self)
        }
    }

    /// ⚠⚠ **A DOUBLE THAT SHOWS TEXT MUST SAY WHO SHOWED IT.** This one models a PROGRAM painting
    /// its own prompt box — that is the whole reason it withholds the text for `hidden_reads` and
    /// then produces it — so it declares [`PaneEcho::ByTheProgram`] and the confirmations below are
    /// about the program.
    ///
    /// A double that left this out would be answering `None`, which collapses to
    /// [`Delivered::OnScreenOnly`]: honest for a host that cannot say, and wrong here, because this
    /// one can. **A fixture that will not state its own premise makes every gate over it weaker
    /// than the product.**
    impl PaneTerminalModes for Recorder {
        fn pane_echo(&self, _id: PaneId) -> Option<PaneEcho> {
            Some(PaneEcho::ByTheProgram)
        }
        fn pane_end_of_input(&self, _id: PaneId) -> Option<PaneEndOfInput> {
            // Not what this double is for — nothing here waits for a peer to finish — and the
            // honest answer for a stand-in with no device is that it cannot say.
            None
        }
    }

    /// The submit is sent ONCE, and only after the text is confirmed.
    ///
    /// Driven against a recording double rather than a pty, because the claim is about the ORDER of
    /// calls and a screen can only show their result. An Enter beside the swallowed first injection
    /// submits an empty prompt, which an agent answers — worse than sending nothing — and the pty
    /// tests above cannot see that it did not happen.
    #[test]
    fn the_submit_is_sent_once_and_only_after_the_text_is_confirmed() {
        let panes = Recorder {
            // Two read-backs come up empty, so the first injection's whole grace expires and a
            // second injection is made — the retry path, with the submit still pending.
            hidden_reads: Mutex::new(2),
            ..Recorder::showing("hello")
        };
        let outcome = deliver(
            &panes,
            &RunContext::uncancellable(),
            PaneId(1),
            "hello",
            &Delivery {
                echo_timeout: Duration::from_millis(1),
                ..Delivery::new()
            },
        )
        .expect("no error");
        assert!(outcome.is_confirmed());

        let log = panes.injected.lock().expect("the log").clone();
        let enters: Vec<usize> = log
            .iter()
            .enumerate()
            .filter(|(_, keys)| keys == &&vec!["Enter".to_owned()])
            .map(|(index, _)| index)
            .collect();
        assert_eq!(enters.len(), 1, "exactly one submit: {log:?}");
        assert_eq!(
            enters[0],
            log.len() - 1,
            "the submit is the LAST thing sent, after the text: {log:?}",
        );
        assert!(log.len() >= 2, "the retry really happened: {log:?}");
    }

    /// ⚠⚠⚠ **AND THE SUBMIT IS NOT SENT AT ALL WHEN THE ONLY EVIDENCE IS TEXT THAT WAS ALREADY
    /// THERE** — the ORDER claim for the module's third hazard, which a screen cannot show.
    ///
    /// This is the live symptom staged: `deliver` returned success, `then_press` went in behind an
    /// injection the program had not read, and the pty handed the peer `…prompt…\r` as ONE read
    /// rather than a prompt and then a keystroke. A live `claude` kept the whole thing in its
    /// composer and started no turn — three runs, three times.
    ///
    /// ⚠ Its twin above (`the_submit_is_sent_once_and_only_after_the_text_is_confirmed`) is the
    /// positive half: the same double, with the screen blank until something is injected, sends
    /// exactly one Enter and sends it last. The two differ ONLY in what the screen was carrying
    /// beforehand, which is the fact under test.
    #[test]
    fn a_submit_is_never_sent_over_a_screen_this_delivery_did_not_change() {
        /// The needle, on the screen before a byte goes in and for ever after — a peer that takes
        /// nothing and repaints nothing.
        const ALREADY: &str = "Continue toward: pay the debt";

        let panes = Recorder {
            showing_before: ALREADY.to_owned(),
            ..Recorder::showing(ALREADY)
        };
        let outcome = deliver(
            &panes,
            &RunContext::uncancellable(),
            PaneId(1),
            ALREADY,
            &Delivery {
                echo_timeout: Duration::from_millis(20),
                attempts: 2,
                ..Delivery::new()
            },
        )
        .expect("no error");

        let log = panes.injected.lock().expect("the log").clone();
        assert!(
            matches!(outcome, Delivered::Unconfirmed { attempts: 2, .. }),
            "a screen that never moved confirms nothing, however many times it carries the \
             needle: {outcome:?}",
        );
        assert!(
            !log.iter().any(|keys| keys == &vec!["Enter".to_owned()]),
            "⚠⚠⚠ AND NO SUBMIT. An Enter behind text the program has not read is not a submitted \
             prompt — it is a byte the pty appends to the same unread run, and the turn never \
             starts. Injected: {log:?}",
        );
        assert_eq!(
            log.len(),
            2,
            "both attempts wrote the text, and only the text: {log:?}"
        );
    }

    /// A run cancelled while WAITING for the echo stops there, having paid for what it wrote.
    ///
    /// The other cancel arm, and the one a real supervisor hits: the wait is where a delivery spends
    /// its time, so a run told to stop is nearly always inside it. Forced rather than raced — the
    /// double raises the flag on the read-back that follows the injection.
    #[test]
    fn a_run_cancelled_while_waiting_for_the_echo_stops_there() {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let panes = Recorder {
            // never shows the text, so only the cancel can end the wait
            cancel_on_read: Some(Arc::clone(&cancel)),
            ..Recorder::showing("")
        };
        let outcome = deliver(
            &panes,
            &RunContext::new(cancel),
            PaneId(1),
            "hello",
            &Delivery::new(),
        )
        .expect("no error");
        assert_eq!(
            outcome,
            Delivered::Stopped {
                attempts: 1,
                written: Written::of(5),
            },
            "the injection it had already made is still charged",
        );
        assert_eq!(
            outcome.written(),
            Written::of(5),
            "and the accessor a plugin charges its Cost from agrees",
        );
        assert_eq!(
            panes.injected.lock().expect("the log").len(),
            1,
            "no submit and no retry after the cancel",
        );
    }

    /// ⚠⚠ **A RUN OUT OF TIME ENDS THIS WAIT TOO** — the second bounded wait in this crate, held to
    /// the same stop condition as the first.
    ///
    /// A delivery's `echo_timeout` is its own affair and this fixture's peer never echoes, so
    /// without the run's deadline the two attempts would each ride that timeout out in full. The
    /// timings are the claim: the deadline is a tenth of one attempt's grace, so a delivery that
    /// consulted only the cancel flag would take some multiple of `grace` and this would fail on
    /// elapsed time even though the outcome looked right.
    ///
    /// ⚠ THE CONTROL comes first and must be SLOW: an untimed run really does spend both attempts,
    /// so the subject below is being compared against a wait that genuinely happens.
    #[test]
    fn a_run_out_of_time_ends_a_delivery_that_is_still_waiting_for_its_echo() {
        let grace = Duration::from_millis(400);
        let attempt = |deadline: Option<Duration>| {
            // never shows the text, so only a bound can end the wait
            let panes = Recorder::showing("");
            let mut spec = Delivery::new();
            spec.echo_timeout = grace;
            spec.attempts = 2;
            let run = RunContext::uncancellable().deadline_in(deadline);
            let start = std::time::Instant::now();
            let outcome = deliver(&panes, &run, PaneId(1), "hello", &spec).expect("no error");
            (outcome, start.elapsed())
        };

        let (control, control_took) = attempt(None);
        assert!(
            matches!(control, Delivered::Unconfirmed { attempts: 2, .. }),
            "an untimed delivery spends every attempt it was given: {control:?}",
        );
        assert!(
            control_took >= grace,
            "and it really waited — otherwise the subject below is compared against nothing: \
             {control_took:?}",
        );

        let (subject, subject_took) = attempt(Some(Duration::from_millis(40)));
        assert!(
            matches!(subject, Delivered::Stopped { attempts: 1, .. }),
            "a run out of time stops the delivery where it stands, charged for the one injection \
             it had already made: {subject:?}",
        );
        assert!(
            subject_took < grace,
            "and it stops INSIDE the wait rather than after it: {subject_took:?} against a \
             per-attempt grace of {grace:?}",
        );

        // ⚠ THE OTHER STOP CHECK — the one at the RETRY loop's top, which the two readings above
        // never reach because their deadline expires inside the first wait. A run already out of
        // time when the delivery is asked for must write NOTHING: an expired run that still gets
        // one injection in is a run writing to somebody's pane after it was over.
        let (already_over, _) = attempt(Some(Duration::ZERO));
        assert!(
            matches!(already_over, Delivered::Stopped { attempts: 0, .. }),
            "a delivery asked for by a run that is already over makes no attempt at all: \
             {already_over:?}",
        );
    }

    /// A prompt box that BREAKS the text across its border is still confirmable — on a fragment.
    ///
    /// The case `Delivery::confirm` exists for. An agent's composer draws a frame, so a long line
    /// wrapped inside it reaches the screen with border characters between the halves and the whole
    /// text is nowhere to be found as one run. Confirming on a leading fragment is what a caller
    /// does instead, and the default (the whole text) would have waited out every attempt.
    #[test]
    fn text_a_prompt_box_broke_in_half_is_confirmed_on_a_fragment() {
        let bordered = |confirm: Option<&str>| {
            Recorder::showing("> the quick brown \u{2502}\u{2502} fox jumps")
                .deliver_once("the quick brown fox jumps", confirm)
        };

        assert!(
            !bordered(None).is_confirmed(),
            "the whole text is not on that screen, and saying it is would be a lie",
        );
        assert!(
            bordered(Some("the quick brown")).is_confirmed(),
            "a fragment the box did not break is what a caller confirms on",
        );
    }

    /// A cancelled run stops delivering and claims nothing about what the pane holds.
    #[test]
    fn a_cancelled_run_stops_and_claims_nothing() {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let panes = Recorder::showing("hello");
        let outcome = deliver(
            &panes,
            &RunContext::new(cancel),
            PaneId(1),
            "hello",
            &Delivery::new(),
        )
        .expect("no error");
        assert_eq!(
            outcome,
            Delivered::Stopped {
                attempts: 0,
                written: Written::of(0),
            },
        );
        assert!(
            panes.injected.lock().expect("the log").is_empty(),
            "a run cancelled before it began writes nothing",
        );
    }

    /// **REQ §3, measured**: a pane [`PaneLifecycle::spawn`] returns is one you can type into at
    /// once — the CHILD reads what is injected at t+0, over and over, with nothing lost.
    ///
    /// The requirement this answers came from a rival, where creating a pane and starting a program
    /// in it are two calls: three of five attempts to use the pane at t+0 were refused, all clearing
    /// within 500 ms, and an attempt to PREDICT readiness ("is the foreground process a lone
    /// shell?") passed while the pane still refused — a predicate measuring an adjacent fact. sprag
    /// has no such gap by construction (one call creates the pane WITH its process), and a claim
    /// about construction is worth exactly what a measurement of it is worth, so this measures it.
    ///
    /// The probe is confirmed by the CHILD's own echo and not the line discipline's: the peer runs
    /// with `-echo`, so the only way `PROBE` reaches the screen is `cat` having read it and written
    /// it back. Twenty spawns, because the failure it looks for was intermittent where it was
    /// observed — one spawn would say nothing about a three-in-five.
    ///
    /// Measured on this box at 20/20 delivered, the child's echo landing 1.2 ms after `spawn`
    /// returned. What is ASSERTED is only that nothing is lost: a time bound here would be a gate
    /// that fails under load the same way it fails under a defect, which this project has paid for.
    #[test]
    fn every_injection_into_a_freshly_spawned_pane_reaches_its_child() {
        const TRIALS: usize = 20;
        let mut lost = Vec::new();
        for trial in 0..TRIALS {
            let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
            let access = WorkspacePaneAccess::new(workspace);
            let life = access.lifecycle().expect("lifecycle");
            let pane = life
                .spawn(
                    &[
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        // `-echo` so the line discipline shows nothing; `cat` in canonical mode
                        // then writes back the line it read, which is the child having taken it.
                        "stty -echo; exec cat".to_owned(),
                    ],
                    40,
                    6,
                )
                .expect("spawn");
            // t+0 — the instant `spawn` returned, with no wait of any kind.
            let mut keys = KeyStroke::text("PROBE");
            keys.push(KeyStroke::named("Enter"));
            let _receipt = access.inject(pane, &keys).expect("write");
            if !shows(&access, pane, "PROBE", Duration::from_secs(5)) {
                lost.push(trial);
            }
            life.close(pane);
        }
        assert!(
            lost.is_empty(),
            "{} of {TRIALS} injections at t+0 never reached the child (trials {lost:?}) — a pane \
             this API hands out must be usable, or every plugin needs a readiness heuristic of its \
             own",
            lost.len(),
        );
    }
}
