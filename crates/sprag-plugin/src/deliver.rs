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

use sprag_terminal::PaneId;

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
    /// The text is on the pane's screen. `attempts` is how many injections it took, so a caller
    /// that wants to know whether this pane swallows input can find out.
    Confirmed { attempts: u32, written: Written },
    /// Every attempt was written and none of them ever appeared. The bytes went to the pty; the
    /// program behind it did not show them.
    Unconfirmed { attempts: u32, written: Written },
    /// The run was cancelled part-way. Nothing is claimed about what the pane holds.
    Cancelled { attempts: u32, written: Written },
}

impl Delivered {
    /// Whether the pane is known to be holding the text.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }

    /// How many bytes reached the pty across every attempt — what a plugin charges as its
    /// [`Cost`](crate::plugin::Cost), since a swallowed write cost the same as a landed one.
    pub const fn written(self) -> Written {
        match self {
            Self::Confirmed { written, .. }
            | Self::Unconfirmed { written, .. }
            | Self::Cancelled { written, .. } => written,
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

    for _ in 0..spec.attempts.max(1) {
        if run.cancelled() {
            return Ok(Delivered::Cancelled {
                attempts,
                written: Written::of(written),
            });
        }
        attempts += 1;
        written += panes.inject(pane, &keys)?.bytes();
        match await_text(panes, run, pane, needle, spec.echo_timeout) {
            Seen::Cancelled => {
                return Ok(Delivered::Cancelled {
                    attempts,
                    written: Written::of(written),
                });
            }
            Seen::Yes => {
                // Only now: the prompt is holding the text, so a submit submits the text.
                if !spec.then_press.is_empty() {
                    written += panes.inject(pane, &spec.then_press)?.bytes();
                }
                return Ok(Delivered::Confirmed {
                    attempts,
                    written: Written::of(written),
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
    Cancelled,
}

/// Wait, bounded and cancellable, for `needle` to appear on the pane.
fn await_text(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    needle: &str,
    timeout: Duration,
) -> Seen {
    let start = std::time::Instant::now();
    loop {
        if run.cancelled() {
            return Seen::Cancelled;
        }
        // An unknown pane can never show anything, and saying so at once beats spending the whole
        // grace on it — the caller's next `inject` will report `UnknownPane` properly.
        match panes.pane_collapsed(pane) {
            Some(text) if text.contains(needle) => return Seen::Yes,
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
    use crate::access::{PaneRow, WorkspacePaneAccess};
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

        assert!(
            deliver(
                &quiet,
                &RunContext::uncancellable(),
                quiet_pane,
                "x",
                &Delivery::new().without_submitting(),
            )
            .expect("no error")
            .is_confirmed(),
            "a pane that has painted nothing is still a pane you can deliver to",
        );

        quiet.lifecycle().expect("lifecycle").close(quiet_pane);
        loud.lifecycle().expect("lifecycle").close(loud_pane);
    }

    /// A `PaneAccess` that records every injection and shows the text only after `hidden_reads`
    /// read-backs — the swallowed-input window, made exact.
    struct Recorder {
        text: String,
        hidden_reads: Mutex<u32>,
        injected: Mutex<Vec<Vec<String>>>,
    }

    impl PaneAccess for Recorder {
        fn pane_ids(&self) -> Vec<PaneId> {
            vec![PaneId(1)]
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            let mut left = self.hidden_reads.lock().expect("the counter");
            if *left > 0 {
                *left -= 1;
                return Some(String::new());
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
            text: "hello".to_owned(),
            // Two read-backs come up empty, so the first injection's whole grace expires and a
            // second injection is made — the retry path, with the submit still pending.
            hidden_reads: Mutex::new(2),
            injected: Mutex::new(Vec::new()),
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

    /// A cancelled run stops delivering and claims nothing about what the pane holds.
    #[test]
    fn a_cancelled_run_stops_and_claims_nothing() {
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let panes = Recorder {
            text: "hello".to_owned(),
            hidden_reads: Mutex::new(0),
            injected: Mutex::new(Vec::new()),
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
            Delivered::Cancelled {
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
