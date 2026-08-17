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
//! ⚠⚠ **AND IT MAKES THE RETRY HAZARD BELOW REACHABLE AGAIN, which is the honest way round.** A
//! needle the screen already carried used to end the wait on its first poll, so a peer slower to
//! paint than [`Delivery::echo_timeout`] never got a second injection — the double-text trade was
//! being avoided by not waiting rather than by the peer being fast. It is a trade this module has
//! always declared, and it is now paid where it is owed.
//!
//! ## ⚠⚠⚠ And pressing the submit is not the same as the peer taking it
//!
//! Everything above is about the TEXT. The last act of a delivery is a keystroke — [`Delivery::
//! then_press`] — and until [`SubmittedWhen`] existed **nobody asked what became of it**. The
//! delivery pressed Enter and returned, and the answer it returned was the same one it would have
//! given for a peer that started a turn.
//!
//! Measured over a real pty, two peers that differ only in whether they ever read the submit byte —
//! one goes on to `sleep 60` after taking the prompt, the other reads one more byte and prints:
//!
//! | peer | what a delivery said | did the screen move again? |
//! |---|---|---|
//! | deaf to the submit | `Confirmed { attempts: 1, written: 18 }` in 10.22 ms | never, in 2 s |
//! | takes the submit | `Confirmed { attempts: 1, written: 18 }` in 10.22 ms | in **2.10 ms** |
//!
//! **The same answer, byte for byte, for the peer that was asked and the peer that was not.** That
//! is how a live `claude` came to sit for sixty seconds with a prompt in its composer while the run
//! that put it there waited out a turn nobody had started: the delivery path's last act had no
//! evidence behind it, so *"delivered"* was a claim about the text alone.
//!
//! So a caller may say WHAT WOULD SHOW THEM the submit landed — [`SubmittedWhen`] — and a delivery
//! that presses on a contract it cannot satisfy answers [`Delivered::Unsubmitted`] instead of
//! reporting success. ⚠ It is the caller's to name for [`ReadyWhen`](crate::readiness::ReadyWhen)'s
//! reason at the other end of the same turn — and the three readings that say so were taken in one
//! live `claude` session, in this order:
//!
//! | pressed | contract | answer | took | the turn |
//! |---|---|---|---|---|
//! | `Enter` | `Stirs` | `Confirmed` | 100.51 ms | ran, and answered |
//! | `k` | `Stirs` | **`Unsubmitted`** | the whole 2 s grace | never started |
//! | `k` | `Repaints` | `Confirmed` | 32.18 ms | never started |
//!
//! The last two rows are why the kind is a WORD and not a rule this module picked. *The screen
//! moved* is the only evidence a general observer of a pane has, and it is wrong in both
//! directions: satisfied by a key an agent's composer merely absorbed (row three), and never
//! satisfied by a peer that reads a line and prints nothing (`exec cat`, and every relay that
//! answers only when it has an answer). A type that chose for the caller would be wrong for a whole
//! class of peers in silence.
//!
//! ## Why this is not a method on `PaneAccess`
//!
//! It waits, so it is bounded, so it must be cancellable, so it needs the run-scoped
//! [`RunContext`] — and `PaneAccess` is the PANE-scoped surface. The crate already made this
//! decision once, when cancellation was bolted onto `PaneAccess` and then moved out; `poll_until`
//! lives beside `RunContext` for the same reason and this is its second caller.
//!
//! ## The retry hazard, named — and why the submit has no retry at all
//!
//! A retry can DOUBLE the text: if the pane took the first injection but echoed it more slowly than
//! [`Delivery::echo_timeout`], the second injection lands on top of the first. There is no way to
//! tell that apart from a swallowed write by looking at the screen, because both look like "not
//! there yet" — so the bound is a real trade and not an oversight. Size `echo_timeout` above the
//! pane's echo latency and the trade is bought; the default is generous for that reason, and the
//! attempt count is small.
//!
//! ⚠⚠ **AND THE SUBMIT IS NEVER RETRIED, which is the same trade answered the other way.** The
//! text can be re-injected because a second copy of a prompt nobody read is text; a second Enter
//! cannot, because the first one may have worked — and an Enter on a composer the first one emptied
//! submits an EMPTY prompt, which an agent answers. That is the failure [`Delivery::then_press`]'s
//! whole ordering exists to prevent, met from the other side, so an unsatisfied submit contract is
//! REPORTED and never re-pressed.

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

/// How long a caller who asks a [`SubmittedWhen`] should give it, unless they know better.
///
/// Two seconds, sized above what has been MEASURED of the evidence it waits for. A peer that paints
/// what it took does so in **2.10 ms** over a local pty; a live `claude` 2.1.233, asked the
/// strongest question there is ([`SubmittedWhen::Stirs`] — its supervisor publishing a change),
/// answered in **100.51 ms** of the Enter going in, and its composer repainted for a key it merely
/// absorbed in **32.18 ms**.
///
/// What the number BUYS is the difference between a slow peer and a deaf one, and the cost of being
/// generous is paid only where the submit really did not land — so it sits an order above the
/// slowest observation rather than tight to it.
///
/// ⚠ It is deliberately NOT the number a caller must use. [`Turn`](crate::completion::Turn)'s rule:
/// how long a peer may take is the caller's to say, and a delivery into a box on the far side of an
/// ssh hop is a different peer from this one.
pub const DEFAULT_SUBMIT_GRACE: Duration = Duration::from_secs(2);

/// **WHAT WOULD SHOW A CALLER THAT THEIR SUBMIT LANDED** — the contract [`deliver`] holds
/// [`Delivery::then_press`] to, and the twin of [`ReadyWhen`](crate::readiness::ReadyWhen) and
/// [`DoneWhen`](crate::completion::DoneWhen) at the two ends of the same turn.
///
/// # ⚠⚠⚠ Why the caller says, and a default could not
///
/// The evidence a general observer of a pane has is that its SCREEN MOVED, and that reading is
/// wrong in both directions for peers this workspace drives every day:
///
/// * **False negative.** A peer in raw mode that reads a line and prints nothing took the submit
///   perfectly and moved no pixel. `exec cat` is the whole class, and so is every relay that
///   answers only when it has an answer.
/// * **False positive.** A keystroke a composer merely ABSORBS repaints the screen exactly as a
///   submitted one does. Measured against `claude` 2.1.233: a printable key pressed instead of
///   Enter had the pane repainted in **32.18 ms** and started no turn, while the same session's
///   real submit was reported by its supervisor in **100.51 ms**. That is the shape register item
///   222's coalesced `…prompt…\r` took when the agent read it as a paste.
///
/// Nothing about a pane says which of those a peer is. **Only the caller knows**, which is
/// [`ReadyWhen`](crate::readiness::ReadyWhen)'s reason for existing, asked one keystroke later.
///
/// ⚠⚠ **THE CRATE ALREADY ASKED THIS ABOUT ITS OTHER KEYSTROKE.** Answering a peer's dialog is not
/// reported until the peer has LEFT the question (`readiness`' own `Arrival::LeftTheQuestion`) —
/// *"a run that reported one off its own keystroke would report success for a dialog still on the
/// screen"*. One concept, two doors, and only one of them was looking; this is the other.
///
/// ⚠ There is no wire word for these yet, deliberately: [`deliver`] is a Rust API no surface
/// publishes, and a published word nothing serves is the defect
/// `every_published_word_is_a_word_the_plugin_host_accepts` exists to catch. The round that gives a
/// wire client a delivery to configure is the round that spells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmittedWhen {
    /// **NOBODY ASKS.** The submit is pressed and the delivery answers about the TEXT alone.
    ///
    /// The honest contract for a peer whose taking of a line is invisible — a raw-mode reader that
    /// prints nothing, a tool that thinks in silence before its first byte — and the one a caller
    /// who has not thought about it gets, because the alternative is a rule that refuses those
    /// peers' every delivery.
    ///
    /// ⚠ It is a WORD and not the absence of one. A caller reading this type must meet *"nothing
    /// verifies the submit"* as a choice somebody made, since that is exactly the state the
    /// delivery path was in when a live agent sat for a minute with a prompt in its composer.
    Unchecked,
    /// The pane's SCREEN is no longer the one it was showing when the submit went in, within
    /// `within`.
    ///
    /// The rule for a peer that PAINTS what it takes and takes what it is given — a REPL that
    /// prints a result, a tool that echoes a command. It is the same *a condition already true when
    /// you started is not evidence* the text's own read-back is held to, asked about the keystroke
    /// after it.
    ///
    /// ⚠⚠ **A COMPOSER THAT MERELY ABSORBS THE KEY SATISFIES THIS**, and that is the residue rather
    /// than a defect in it: an agent's prompt box repaints for a printable character as readily as
    /// for a submit. Where the peer is an agent this host supervises, [`Stirs`](Self::Stirs) is the
    /// stronger question and it is one word away.
    Repaints {
        /// How long to wait for that, after which the delivery answers
        /// [`Delivered::Unsubmitted`].
        within: Duration,
    },
    /// The AGENT the delivery was addressed to has MOVED — the supervisor has published a change of
    /// its state since the submit went in, within `within`.
    ///
    /// The strongest of the three, and the rule for the peer this whole module was written for: an
    /// interactive agent CLI whose turn STARTING is the thing a submit is for. A prompt sitting
    /// unsent in a composer leaves the agent exactly where it was, which is why this catches what a
    /// screen predicate cannot.
    ///
    /// # ⚠⚠ The evidence is `seq`, not the state
    ///
    /// [`AgentObservation::seq`](crate::access::AgentObservation::seq) counts PUBLISHED CHANGES and
    /// never decreases, so a turn that began and ended between two polls is still visible in it —
    /// where *"the agent is working"* asked a moment too late reads `Idle` and would call a
    /// submitted prompt unsubmitted. Its own doc says it is for exactly this comparison, and
    /// [`DoneWhen::Settles`](crate::completion::DoneWhen::Settles) arms itself the same way at the
    /// other end of the turn.
    ///
    /// # ⚠ What is deliberately NOT evidence
    ///
    /// * An observation naming a DIFFERENT agent than the one that was there when the submit went
    ///   in, or naming none. A pane whose agent changed under the delivery is not a pane this can
    ///   say anything about.
    /// * A host with no supervisor at all ([`PaneAccess::supervision`] is `None`), and a pane no
    ///   manifest claims. Neither is ever satisfied, on
    ///   [`ReadyWhen::Runs`](crate::readiness::ReadyWhen::Runs)' terms: a contract that cannot be
    ///   answered says so rather than being read as a yes.
    Stirs {
        /// How long to wait for that, after which the delivery answers
        /// [`Delivered::Unsubmitted`].
        within: Duration,
    },
}

impl SubmittedWhen {
    /// How long this contract may be waited for, or [`None`] where nothing is waited for at all.
    #[must_use]
    pub const fn within(self) -> Option<Duration> {
        match self {
            Self::Unchecked => None,
            Self::Repaints { within } | Self::Stirs { within } => Some(within),
        }
    }

    /// This contract as the clause of a sentence about a submit that never satisfied it —
    /// *"repaint"*, *"stir"*.
    ///
    /// ⚠ The reason [`PaneError::NeverSubmitted`] carries the whole contract rather than a
    /// duration: its sentence is what an agent reads when a run refuses, and *"the pane did not
    /// repaint"* is false of the kind that watches the supervisor. Same rule as
    /// [`ReadyWhen::describe`](crate::readiness::ReadyWhen::describe) one door over.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            // Never reaches a refusal — nothing is waited for, so nothing can go unsatisfied — and
            // it answers rather than being unreachable, because a caller printing a spec is a
            // reader too.
            Self::Unchecked => "was not asked to show anything",
            Self::Repaints { .. } => "did not repaint",
            Self::Stirs { .. } => "did not stir",
        }
    }
}

/// How to deliver text to a pane, and what to do once it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// What must appear on the pane's screen for the text to count as arrived. `None` means the
    /// text itself.
    ///
    /// ⚠⚠ **APPEAR, not BE THERE.** A needle the screen was already carrying when the delivery
    /// began is not evidence — see the module docs' third hazard — so a caller is free to pick a
    /// fragment their peer prints on every turn without that fragment confirming their next
    /// delivery before it lands.
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
    /// **WHAT WOULD SHOW THIS CALLER THE SUBMIT LANDED** — see [`SubmittedWhen`], which is where
    /// the whole argument for a caller-chosen contract lives.
    ///
    /// [`SubmittedWhen::Unchecked`] by default, which is what this module did for its whole life
    /// before the word existed: press, and answer about the text. A caller whose peer can show them
    /// the difference says so and gets [`Delivered::Unsubmitted`] instead of a success they would
    /// have had to wait out a turn to disbelieve.
    ///
    /// ⚠ Consulted only when something is PRESSED. A delivery with an empty
    /// [`then_press`](Self::then_press) submits nothing, so there is nothing for a contract to be
    /// about — and a caller who spells both has said something that cannot be true of any pane.
    pub submitted_when: SubmittedWhen,
}

impl Delivery {
    /// The defaults: confirm on the text itself, a generous echo grace, three attempts, submit with
    /// Enter, and nothing asked about what became of it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            confirm: None,
            echo_timeout: DEFAULT_ECHO_TIMEOUT,
            attempts: DEFAULT_ATTEMPTS,
            then_press: vec![KeyStroke::named("Enter")],
            // ⚠ THE UNCHECKED SUBMIT IS THE DEFAULT, and it is a decision rather than an
            // oversight: this module cannot know whether a caller's peer shows anything at all when
            // it takes a line, and the rule that guessed would refuse every delivery to the peers
            // that show nothing. See `SubmittedWhen`.
            submitted_when: SubmittedWhen::Unchecked,
        }
    }

    /// The defaults, but the submit is held to `contract` — see [`SubmittedWhen`].
    #[must_use]
    pub fn submitted_when(mut self, contract: SubmittedWhen) -> Self {
        self.submitted_when = contract;
        self
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
/// Six outcomes and not a `bool`, because "the pane never took it" is a thing a supervisor must
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
    ///
    /// ⚠⚠ **AND THE SUBMIT SATISFIED WHATEVER THE CALLER ASKED OF IT** — see
    /// [`Delivery::submitted_when`]. Under the default ([`SubmittedWhen::Unchecked`]) that clause
    /// is empty and this answer means what it always meant: a claim about the TEXT.
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
    /// **THE TEXT ARRIVED, THE SUBMIT WAS PRESSED, AND THE CALLER'S EVIDENCE FOR IT NEVER CAME** —
    /// typed, and as far as anything here can tell not sent.
    ///
    /// # ⚠⚠⚠ Why this is a fourth answer and not a slower `Confirmed`
    ///
    /// It is the state a live `claude` sat in for sixty seconds: the prompt inside its composer's
    /// box rule, the agent idle underneath it, and the run that put it there waiting out a turn
    /// nobody had started. The delivery reported `Confirmed { attempts: 1 }` — measured, and the
    /// same answer to the digit that a peer which took the submit gives — because *delivered* was a
    /// claim about the text and nothing asked about the keystroke after it.
    ///
    /// What a caller does with it is what makes it worth a word: the prompt is IN the pane's
    /// composer, so the next delivery would concatenate onto it, and nothing here presses a second
    /// Enter (see the module docs' hazard). It is the [`Unconfirmed`](Self::Unconfirmed) of the
    /// submit — *hand this pane to a person* — and it is not an error, because a peer that ignores
    /// a keystroke has broken no contract of the pane's.
    ///
    /// `wanted` is the contract that went unsatisfied, carried for
    /// [`PaneError::NeverReady`]'s reason: the refusal built
    /// from this is a sentence somebody reads, and *"the pane did not repaint"* is false of the
    /// kind that watches the supervisor.
    Unsubmitted {
        /// How many injections carried the TEXT — the submit is pressed once and never retried.
        attempts: u32,
        /// Every byte that reached the pty, the submit's own among them. It was paid for whatever
        /// the peer did with it.
        written: Written,
        /// The contract that went unsatisfied.
        wanted: SubmittedWhen,
    },
    /// THE RUN ENDED part-way, BEFORE ANYTHING WAS SUBMITTED — cancelled, or out of time. Nothing
    /// is claimed about what the pane holds. Which of the two it was is the
    /// [`crate::run::RunContext`]'s to answer.
    ///
    /// ⚠⚠ **The prompt may be TYPED AND UNSUBMITTED**, which is what a caller acts on: a delivery
    /// writes the text and presses only once the text is on the screen, so a run that ends between
    /// those two leaves the composer holding it. The outer loop reads this answer as *no question
    /// was asked* for exactly that reason.
    Stopped { attempts: u32, written: Written },
    /// **THE SUBMIT WENT OUT AND THE RUN ENDED BEFORE ITS EVIDENCE COULD ARRIVE.** Nothing is
    /// claimed about whether it landed — and, unlike [`Stopped`](Self::Stopped), the keystroke IS
    /// on the pseudoterminal, so a question may well have been asked.
    ///
    /// # ⚠⚠⚠ Why a stop needed two words the moment the submit gained a contract
    ///
    /// `Stopped` means *nothing was asked* to every caller that reads it — the outer loop turns it
    /// into a stated *"the run ended while delivering the prompt; nothing was asked"*. That is true
    /// of every stop this module could produce before there was anything to wait for AFTER the
    /// press, and it becomes FALSE the moment there is: a run whose clock expires inside the submit
    /// wait has sent the Enter, and reporting it as *nothing was asked* would be this crate's
    /// favourite defect — a sentence about a run that is confidently the wrong way round.
    ///
    /// ⚠ A caller with no submit contract can never see this: with
    /// [`SubmittedWhen::Unchecked`] there is no wait to be stopped inside.
    Unwitnessed {
        /// How many injections carried the TEXT.
        attempts: u32,
        /// Every byte that reached the pty, the submit's own among them.
        written: Written,
        /// The contract whose evidence the run did not stay alive long enough to see.
        wanted: SubmittedWhen,
    },
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
    ///
    /// ⚠⚠ **TRUE FOR THE TWO SUBMIT ANSWERS**, and that is the point of them rather than a
    /// leniency: [`Unsubmitted`](Self::Unsubmitted) and [`Unwitnessed`](Self::Unwitnessed) are
    /// reached only through the same read-back the two above are, so the text is on that screen —
    /// which is exactly why a caller must not deliver again on top of it.
    #[must_use]
    pub const fn is_on_screen(self) -> bool {
        matches!(
            self,
            Self::Confirmed { .. }
                | Self::OnScreenOnly { .. }
                | Self::Unsubmitted { .. }
                | Self::Unwitnessed { .. }
        )
    }

    /// How many bytes reached the pty across every attempt — what a plugin charges as its
    /// [`Cost`](crate::plugin::Cost), since a swallowed write cost the same as a landed one.
    pub const fn written(self) -> Written {
        match self {
            Self::Confirmed { written, .. }
            | Self::OnScreenOnly { written, .. }
            | Self::Unconfirmed { written, .. }
            | Self::Unsubmitted { written, .. }
            | Self::Stopped { written, .. }
            | Self::Unwitnessed { written, .. } => written,
        }
    }
}

/// Inject `text` into `pane` and confirm the pane took it, re-injecting until it does.
///
/// The read-back is [`PaneAccess::pane_collapsed`] — the pane's rows joined with nothing between
/// them — so text the pane WRAPPED still matches. What it cannot see through is a border drawn
/// between the halves, which is what [`Delivery::confirm`] is for.
///
/// [`Delivery::then_press`] is sent only once the text is visible, so an Enter can never submit an
/// empty prompt — and the call returns as soon as that press has whatever evidence its caller asked
/// for ([`Delivery::submitted_when`]), which under the default is at once.
///
/// ⚠⚠⚠ **VISIBLE MEANS VISIBLE ON A SCREEN THIS DELIVERY CHANGED.** The pane's collapsed screen is
/// read once before the first injection, and a read-back that finds the needle on that same screen
/// is not evidence — see the module docs. Without it, a caller who sends the same text twice gets
/// the second delivery confirmed off the first one's echo, and the submit lands on text no program
/// has read.
///
/// ⚠⚠⚠ **AND THE SUBMIT IS HELD TO [`Delivery::submitted_when`]**, which is a SECOND baseline —
/// taken at the press, since that is the moment the evidence has to be new against. Under the
/// default nothing is asked and the answer is about the text alone; under a contract, a keystroke
/// that showed nothing comes back [`Delivered::Unsubmitted`] rather than as a success a caller
/// would need a whole turn to disbelieve.
///
/// # Errors
///
/// [`PaneError`] when the pane is unknown, a key cannot be encoded, or a write fails — the same
/// causes [`PaneAccess::inject`] has, and none of them are "the pane did not take it", which is
/// [`Delivered::Unconfirmed`], nor "the peer ignored the submit", which is
/// [`Delivered::Unsubmitted`].
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
                // Only now: the text is on a screen THIS DELIVERY CHANGED, so a submit submits the
                // text rather than an empty line — and, measured live, it is a keystroke of its own
                // rather than a byte appended to the same unread pty read as the prompt. Sent for
                // BOTH on-screen answers — see `Delivered::OnScreenOnly`.
                if !spec.then_press.is_empty() {
                    // ⚠⚠⚠ THE SECOND BASELINE, TAKEN BEFORE THE PRESS AND NOT AFTER IT — the same
                    // guarantee `before` is above and `Completion::begin` is at the turn's other
                    // end. Armed after the keystroke, the change it looks for is one it may already
                    // have missed, and a peer quick enough to answer would be reported as having
                    // ignored the submit.
                    let witness = Submission::arm(panes, pane, spec.submitted_when);
                    written += panes.inject(pane, &spec.then_press)?.bytes();
                    match witness.await_landing(panes, run, pane) {
                        Seen::No => {
                            return Ok(Delivered::Unsubmitted {
                                attempts,
                                written: Written::of(written),
                                wanted: spec.submitted_when,
                            });
                        }
                        Seen::Stopped => {
                            // ⚠ NOT `Stopped`: the keystroke is on the pseudoterminal, so *nothing
                            // was asked* is a claim this cannot make. See `Delivered::Unwitnessed`.
                            return Ok(Delivered::Unwitnessed {
                                attempts,
                                written: Written::of(written),
                                wanted: spec.submitted_when,
                            });
                        }
                        Seen::Yes => {}
                    }
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
/// hole the deadline was added to close. So EVERY wait in this crate asks
/// [`RunContext::stopped`](crate::run::RunContext::stopped) — this one, `poll_until`, and the
/// submit's own ([`Submission::await_landing`]) — which is the one definition of *the run is over*.
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

/// The ARMED evaluator of a [`SubmittedWhen`] — the contract, plus what the pane was like at the
/// moment the submit went in.
///
/// Mirrors [`Completion`](crate::completion::Completion) at the turn's other end, and for the same
/// reason: some conditions are not predicates over the present. *The screen moved* and *the agent
/// stirred* are both comparisons against a moment, and the moment is the press — so the type holds
/// what a bare `match` on the contract could not.
///
/// ⚠ Private, and not for [`Completion`](crate::completion::Completion)'s one-door reason: this is
/// one step of one function. What
/// makes it a type rather than two locals is that ARMING and ASKING must not drift apart — the
/// whole defect this closes is a question asked against the wrong moment.
struct Submission {
    /// What the caller said would show them the submit landed.
    wanted: SubmittedWhen,
    /// The pane's collapsed screen as the submit went in — [`SubmittedWhen::Repaints`]' baseline.
    ///
    /// `None` both for a contract that never reads it and for a pane that could not be read, which
    /// [`landed`](Self::landed) answers the same way: any screen it can read afterwards is a
    /// different one.
    screen: Option<String>,
    /// WHO the pane's agent was and how many published changes it had been through, as the submit
    /// went in — [`SubmittedWhen::Stirs`]' baseline.
    ///
    /// `None` where nothing could be armed: no supervisor on this host, no observation for this
    /// pane, or an observation naming no agent. That is not evidence about a keystroke, so the
    /// contract is never satisfied — see the kind's own doc.
    agent: Option<(String, u64)>,
}

impl Submission {
    /// Read what this contract will be compared against — **called before the submit is injected**.
    fn arm(panes: &dyn PaneAccess, pane: PaneId, wanted: SubmittedWhen) -> Self {
        let (screen, agent) = match wanted {
            // Nothing is asked, so nothing is read. A baseline taken for an unchecked submit would
            // be a pane read every delivery pays for and nothing consults.
            SubmittedWhen::Unchecked => (None, None),
            SubmittedWhen::Repaints { .. } => (panes.pane_collapsed(pane), None),
            SubmittedWhen::Stirs { .. } => (
                None,
                panes
                    .supervision()
                    .and_then(|supervisor| supervisor.pane_agent_state(pane))
                    .and_then(|seen| seen.agent.map(|agent| (agent, seen.seq))),
            ),
        };
        Self {
            wanted,
            screen,
            agent,
        }
    }

    /// Whether the submit's evidence is here YET — `None` where it can never come.
    ///
    /// Three answers for [`await_text`]'s reason one door up: *not yet* and *never* end the wait
    /// differently, and spending a whole grace on a question nothing can answer is a delay with no
    /// information in it.
    fn landed(&self, panes: &dyn PaneAccess, pane: PaneId) -> Option<bool> {
        match self.wanted {
            // Unreachable: `await_landing` returns before asking. Answered rather than panicking,
            // because *nobody asked* is satisfied by anything at all.
            SubmittedWhen::Unchecked => Some(true),
            // ⚠ `map`, so a pane nobody knows (`None`) stays `None` — it can never repaint, and
            // saying so at once beats spending the window on it.
            SubmittedWhen::Repaints { .. } => panes
                .pane_collapsed(pane)
                .map(|now| Some(now.as_str()) != self.screen.as_deref()),
            SubmittedWhen::Stirs { .. } => {
                // Nothing was armed — no supervisor, no observation, or no agent named — so no
                // reading taken later could be evidence about this keystroke.
                let (addressed, pressed_at) = self.agent.as_ref()?;
                Some(
                    panes
                        .supervision()
                        .and_then(|supervisor| supervisor.pane_agent_state(pane))
                        .is_some_and(|seen| {
                            // ⚠⚠ BOTH, and the name is what makes this a claim about the peer the
                            // submit went to rather than about whatever is in the pane now.
                            seen.seq > *pressed_at
                                && seen.agent.as_deref() == Some(addressed.as_str())
                        }),
                )
            }
        }
    }

    /// Wait, bounded by this contract's own window AND by the run's deadline, for the submit's
    /// evidence.
    ///
    /// ⚠⚠ **THE THIRD BOUNDED WAIT IN THIS CRATE**, held to the same stop condition as the other
    /// two: a delivery that outlived a run that is over would be typing into somebody's pane after
    /// it ended. ⚠ Where this one differs is what a stop MEANS — the keystroke is already out, so
    /// the answer is [`Delivered::Unwitnessed`] rather than [`Delivered::Stopped`].
    fn await_landing(&self, panes: &dyn PaneAccess, run: &RunContext, pane: PaneId) -> Seen {
        // Nothing to wait for: the caller asked nothing of the submit, which is this module's
        // whole behaviour before the contract existed.
        let Some(within) = self.wanted.within() else {
            return Seen::Yes;
        };
        let start = std::time::Instant::now();
        loop {
            if run.stopped() {
                return Seen::Stopped;
            }
            match self.landed(panes, pane) {
                Some(true) => return Seen::Yes,
                None => return Seen::No,
                Some(false) => {}
            }
            if start.elapsed() >= within {
                return Seen::No;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
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

    /// A peer that PAINTS a prompt of `bytes` and then reacts to the submit after it in one of
    /// three ways — the three a delivery has to tell apart.
    ///
    /// `dd bs=1 count=N` copies exactly the prompt to the screen, which is what makes the text's
    /// own read-back succeed deterministically and puts every peer below on the same footing at the
    /// moment the submit is pressed. What follows it is the whole experiment:
    ///
    /// * [`Reacts::Nothing`] — `sleep`, so the submit byte sits unread in the pty for ever. **The
    ///   peer a delivery used to report as `Confirmed`.**
    /// * [`Reacts::Paints`] — it READS the submit and prints a character. The screen moves and
    ///   nothing else happened, which is an agent's composer absorbing a keystroke.
    /// * [`Reacts::Works`] — it reads the submit and prints the marker a supervisor over this pane
    ///   reads as *the peer started working*.
    fn takes_a_prompt_of(bytes: usize, then: Reacts) -> String {
        peer(&format!(
            "dd bs=1 count={bytes} 2>/dev/null; {}",
            match then {
                Reacts::Nothing => "exec sleep 60".to_owned(),
                Reacts::Paints =>
                    "dd bs=1 count=1 of=/dev/null 2>/dev/null; printf '_'; exec sleep 60".to_owned(),
                Reacts::Works => format!(
                    "dd bs=1 count=1 of=/dev/null 2>/dev/null; printf '{WORKING}'; exec sleep 60",
                ),
            },
        ))
    }

    /// What a peer does with the submit that follows its prompt — see [`takes_a_prompt_of`].
    #[derive(Clone, Copy)]
    enum Reacts {
        Nothing,
        Paints,
        Works,
    }

    /// The marker a [`Reacts::Works`] peer prints, and the one the stand-in supervisor below reads
    /// as *this agent is working*.
    const WORKING: &str = "TOOK";

    /// HOW the stand-in supervisor publishes the turn its peer started — see [`supervised_peer`].
    ///
    /// Three shapes, and each is a claim [`SubmittedWhen::Stirs`] makes that nothing held until a
    /// fixture could stage it. **A mutation that deletes one of those clauses passes every other
    /// test in this module**, which is how the last two got here.
    #[derive(Clone, Copy)]
    enum Publishes {
        /// Working while the marker is on the screen, under the name the delivery was addressed to.
        Plainly,
        /// The same change, published about a DIFFERENT agent — a pane whose program changed under
        /// the delivery.
        AsSomebodyElse,
        /// **THE TURN BEGAN AND ENDED BETWEEN TWO POLLS**: every look reports the peer at REST, and
        /// the two changes nobody saw are in `seq`. A rule reading the STATE calls this a submit
        /// that never landed; the number says otherwise, and the number is what the real
        /// [`AgentObservation`](crate::access::AgentObservation) exists to carry.
        BetweenTwoPolls,
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

    /// The same peer, WITH A SUPERVISOR OVER IT — a stand-in for the daemon's detector that
    /// publishes [`AgentState::Working`] once the peer has printed [`WORKING`].
    ///
    /// # ⚠⚠ Its verdict is DERIVED FROM THE PANE, not set by hand
    ///
    /// A double whose observation a test moves with a `Mutex` decides its own result, and the fact
    /// under test here is whether a delivery notices a change **the peer caused**. So this reads
    /// the pane's own screen through the same [`PaneAccess::pane_collapsed`] everything else does,
    /// and the only thing it invents is the RULE — which is what a real ruleset is.
    ///
    /// ⚠ `seq` counts PUBLISHED CHANGES, which is the contract the real
    /// [`AgentObservation::seq`](crate::access::AgentObservation::seq) states and the number
    /// [`SubmittedWhen::Stirs`] compares against: bumped when the verdict differs from the last one
    /// handed out and never otherwise, so a pane repainting the same state does not move it.
    ///
    /// ⚠ [`Authority::Scraped`], honestly: this reads a screen. A stand-in claiming
    /// [`Authority::Reported`](crate::access::Authority::Reported) would be asserting that an agent
    /// hook it does not have said so.
    ///
    /// ⚠ `publishes` is HOW it publishes that turn, and each shape is there because the product
    /// makes a claim that nothing held until a fixture could stage it — see [`Publishes`].
    fn supervised_peer(script: &str, publishes: Publishes) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 6))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let pane = workspace
            .lock()
            .expect("the workspace")
            .spawn(command, "peer".to_string(), 40, 6)
            .expect("spawn the pane");

        let reader = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let published = Arc::new(Mutex::new((sprag_detect::AgentState::Idle, 0_u64)));
        let source: crate::access::AgentStateSource = Arc::new(move |id: PaneId| {
            let screen = reader.pane_collapsed(id)?;
            let working = screen.contains(WORKING);
            let now = match (working, publishes) {
                // ⚠ THE TURN THAT BEGAN AND ENDED BETWEEN TWO POLLS: this look never catches the
                // peer working, and the two published changes it missed are in `seq` — which is
                // the whole reason `Stirs` compares that number and not the state.
                (true, Publishes::BetweenTwoPolls) => sprag_detect::AgentState::Idle,
                (true, _) => sprag_detect::AgentState::Working,
                (false, _) => sprag_detect::AgentState::Idle,
            };
            let mut last = published.lock().expect("the published verdict");
            if matches!(publishes, Publishes::BetweenTwoPolls) {
                // Idle throughout, so the state cannot say the turn happened; the counter can.
                if working && last.1 == 0 {
                    *last = (now, 2);
                }
            } else if last.0 != now {
                *last = (now, last.1 + 1);
            }
            Some(crate::access::AgentObservation {
                state: last.0,
                agent: Some(
                    match (working, publishes) {
                        (true, Publishes::AsSomebodyElse) => "somebody-else",
                        _ => "peer",
                    }
                    .to_owned(),
                ),
                authority: crate::access::Authority::Scraped {
                    rule: Some("printed the marker".to_owned()),
                },
                seq: last.1,
                asking: None,
            })
        });

        let access = WorkspacePaneAccess::new(workspace).with_agent_state(Some(source));
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
        /// ⚠⚠⚠ **WHAT THE SCREEN BECOMES ONCE THE SUBMIT HAS BEEN INJECTED** — the fact
        /// [`SubmittedWhen::Repaints`] is about, and the second thing this double refused to model
        /// until the submit had a contract to satisfy.
        ///
        /// `None` is the peer that takes the keystroke and paints nothing, which is the whole
        /// hazard: a screen that stops moving is the only sign a general observer gets that a
        /// submit did nothing.
        after_submit: Option<String>,
        hidden_reads: Mutex<u32>,
        injected: Mutex<Vec<Vec<String>>>,
        /// Raised on the first read-back AFTER an injection, so a cancel lands INSIDE the wait
        /// rather than before it.
        ///
        /// ⚠ *After an injection* rather than *on the first read* since the baseline exists: the
        /// baseline read happens before the loop's first stop check, so a flag raised on it would
        /// end the delivery having written nothing — a different arm from the one this stages.
        cancel_on_read: Option<Arc<std::sync::atomic::AtomicBool>>,
        /// Raised on the first read-back AFTER THE SUBMIT, so a cancel lands inside the wait for
        /// the submit's own evidence rather than inside the wait for the text's.
        ///
        /// ⚠ A second flag rather than a reused one: the two waits are what
        /// [`Delivered::Stopped`] and [`Delivered::Unwitnessed`] tell apart, and a fixture that
        /// could only stage one of them could not measure the difference.
        cancel_on_submit: Option<Arc<std::sync::atomic::AtomicBool>>,
    }

    impl Recorder {
        /// A blank-screened double showing `text` once something has been injected, and never
        /// moving again after the submit.
        fn showing(text: &str) -> Self {
            Self {
                text: text.to_owned(),
                showing_before: String::new(),
                after_submit: None,
                hidden_reads: Mutex::new(0),
                injected: Mutex::new(Vec::new()),
                cancel_on_read: None,
                cancel_on_submit: None,
            }
        }

        /// Whether the submit has been injected — the moment this double's screen changes for the
        /// second time.
        fn submitted(&self) -> bool {
            self.injected
                .lock()
                .expect("the log")
                .iter()
                .any(|keys| keys == &vec!["Enter".to_owned()])
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
            // ⚠ THE SUBMIT'S OWN SCREEN, and it is asked FIRST: from the press onwards this pane
            // shows what the peer made of the keystroke, which for the peer that made nothing of it
            // is the same screen the text arrived on.
            if self.submitted() {
                if let Some(cancel) = &self.cancel_on_submit {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                }
                return Some(
                    self.after_submit
                        .clone()
                        .unwrap_or_else(|| self.text.clone()),
                );
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

    /// ⚠⚠⚠⚠ **A COMPOSER THAT WAS ALREADY DIRTY IS CONFIRMED AS IF IT WERE CLEAN** — register item
    /// 223, MEASURED here rather than argued about, and this gate asserts today's behaviour so that
    /// fixing it turns the gate around.
    ///
    /// # The defect, in one sentence
    ///
    /// The read-back asks *is my needle on a screen this delivery changed*, and a needle is a
    /// SUBSTRING. So a composer holding an agent's own suggestion — `claude` 2.1.233 was measured
    /// offering `what is 3 plus 3 in English?` back after two prompts differing only in a digit —
    /// takes the delivered text onto the end of it, the read-back finds the needle inside the
    /// concatenation, and the submit lands on **a prompt nobody wrote**.
    ///
    /// ⚠⚠⚠ **NOTHING ON THE SCREEN DISTINGUISHES THE TWO AUTHORS.** Text a run injected and text
    /// the agent proposed are the same pixels, so `deliver` cannot tell *my text arrived* from *my
    /// text arrived after somebody else's*.
    ///
    /// ⚠⚠⚠⚠ **AND TIGHTENING THE PREDICATE IS RULED OUT — MEASURED, NOT ASSUMED.** The obvious fix
    /// is to stop accepting a substring, so it was tried: `contains` → `ends_with`. It does not
    /// fix this at all (a concatenation ENDS WITH the delivered text, so this gate stayed green),
    /// and it reds two neighbours — including
    /// [`text_a_prompt_box_broke_in_half_is_confirmed_on_a_fragment`], which exists precisely
    /// because a prompt box may split the text, so **a needle being a fragment is a documented
    /// requirement, not an oversight**. Whatever pays this item, it is not a stricter read-back:
    /// it is clearing the composer before typing, or evidence from the PROGRAM rather than the
    /// screen — which item 224 records nothing on `PaneAccess` offers.
    ///
    /// ⚠⚠ **AND IT GETS COMMONER THE LONGER A RUN GOES**: a loop repeating one prompt is exactly
    /// the input that trains the suggestion, so the population this fires on is the loop's own.
    #[test]
    fn a_prompt_typed_onto_a_dirty_composer_is_confirmed_and_submitted_anyway() {
        // What the agent left sitting there, and what this delivery means to say.
        const OFFERED: &str = "> what is 3 plus 3 in English?";
        const SENT: &str = "what is 4 plus 4?";

        let double = Recorder {
            // ⚠ A REAL COMPOSER APPENDS. The screen after typing is the suggestion with the new
            // text on the end of it — which is precisely what makes the substring read-back pass.
            text: format!("{OFFERED}{SENT}"),
            showing_before: OFFERED.to_owned(),
            after_submit: None,
            hidden_reads: Mutex::new(0),
            injected: Mutex::new(Vec::new()),
            cancel_on_read: None,
            cancel_on_submit: None,
        };
        let spec = Delivery {
            echo_timeout: Duration::from_millis(1),
            attempts: 1,
            ..Delivery::new()
        };
        let delivered = deliver(
            &double,
            &RunContext::uncancellable(),
            PaneId(1),
            SENT,
            &spec,
        )
        .expect("no error");

        assert!(
            delivered.is_confirmed(),
            "⚠⚠⚠⚠ ITEM 223, MEASURED: the delivery reports CONFIRMED though the composer holds \
             {OFFERED:?} in front of it. When this stops holding, the item is paid and this gate \
             is to be turned around, not removed. Got {delivered:?}",
        );
        assert!(
            double.submitted(),
            "⚠⚠⚠ AND THE ENTER WENT, which is what makes it cost something: the peer is handed \
             {:?} — a prompt nobody wrote — and the run spends a turn on the answer",
            format!("{OFFERED}{SENT}"),
        );

        // ⚠⚠⚠ THE CONTROL, AND IT IS WHAT SAYS THE READ-BACK IS NOT SIMPLY BROKEN. The same
        // delivery onto a CLEAN composer confirms for the right reason — so what is measured above
        // is the substring match, not a predicate that says yes to everything.
        let clean = Recorder::showing(SENT);
        let on_clean = deliver(&clean, &RunContext::uncancellable(), PaneId(1), SENT, &spec)
            .expect("no error");
        assert!(
            on_clean.is_confirmed(),
            "a clean composer must still confirm, or this gate is measuring a broken read-back \
             rather than a dirty composer: {on_clean:?}",
        );
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

    /// ⚠⚠ **A CHANGE PUBLISHED ABOUT A DIFFERENT AGENT IS NOT THIS SUBMIT'S EVIDENCE.**
    ///
    /// The pane's supervisor here does everything the satisfied case does — it publishes a state
    /// change, the `seq` moves, and it moves because THIS keystroke was read — and it names a
    /// different agent while doing it. That is a pane whose program changed under the delivery, and
    /// the submit went to the one that was there before.
    ///
    /// ⚠ It exists because the rule had no gate: dropping the name comparison from
    /// [`SubmittedWhen::Stirs`] left every test in this module green. A claim a mutation cannot
    /// break is a claim nothing is holding.
    #[test]
    fn a_change_published_about_a_different_agent_is_not_this_submits_evidence() {
        assert!(
            delivered_watching_the_supervisor(Publishes::Plainly).is_confirmed(),
            "⚠ THE CONTROL: the same peer, the same change, named as the agent that was there when \
             the submit was pressed",
        );
        assert!(
            matches!(
                delivered_watching_the_supervisor(Publishes::AsSomebodyElse),
                Delivered::Unsubmitted { .. },
            ),
            "⚠⚠ a state change published about ANOTHER agent says nothing about the keystroke this \
             delivery sent to the one before it",
        );
    }

    /// ⚠⚠⚠ **A TURN THAT BEGAN AND ENDED BETWEEN TWO POLLS IS STILL A SUBMIT THAT LANDED** — why
    /// [`SubmittedWhen::Stirs`] compares `seq` and not the state.
    ///
    /// The supervisor here reports the peer AT REST at every look, and it is telling the truth
    /// every time: the turn was over before anybody looked. What it also carries is the two
    /// published changes nobody saw, which is exactly what
    /// [`AgentObservation::seq`](crate::access::AgentObservation::seq)'s own doc says it is for.
    ///
    /// ⚠ A rule reading *is it working?* would answer NO here and refuse a prompt that was asked
    /// and answered — and against a fast peer that is not an edge case, it is the common one. The
    /// mutation is one word (`seen.state == Working`), and until this existed nothing in the module
    /// noticed it.
    #[test]
    fn a_turn_that_began_and_ended_between_two_polls_still_counts_as_a_stir() {
        assert!(
            delivered_watching_the_supervisor(Publishes::BetweenTwoPolls).is_confirmed(),
            "the counter is the evidence; the state at a glance is not",
        );
    }

    /// One delivery over a peer that takes the submit, watched through a supervisor that
    /// [`Publishes`] its turn in the named way.
    fn delivered_watching_the_supervisor(publishes: Publishes) -> Delivered {
        const PROMPT: &str = "what is 2 plus 2?";
        let (access, pane) =
            supervised_peer(&takes_a_prompt_of(PROMPT.len(), Reacts::Works), publishes);
        let outcome = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            PROMPT,
            &Delivery {
                attempts: 1,
                submitted_when: SubmittedWhen::Stirs {
                    within: Duration::from_millis(150),
                },
                ..Delivery::new()
            },
        )
        .expect("no error");
        access.lifecycle().expect("lifecycle").close(pane);
        outcome
    }

    /// ⚠⚠⚠ **THE SUBMIT'S BASELINE IS TAKEN AT THE PRESS, AND NOT WHEN THE DELIVERY BEGAN** — the
    /// ORDER claim for the module's fourth hazard, which is the one a screen cannot show.
    ///
    /// The double's screen moves TWICE: once when the text is injected and once when the submit
    /// is. A witness armed at the wrong moment gets the wrong answer in a way no amount of waiting
    /// fixes — armed before the TEXT went in, the text's own arrival satisfies it and every peer on
    /// earth looks submitted-to; armed after the PRESS, the change it is looking for has already
    /// happened and a peer that answered instantly looks deaf.
    ///
    /// ⚠ Both halves, because either alone passes for a build that always answers the same way:
    /// the peer whose screen moves for the submit is `Confirmed`, and the peer whose screen stops
    /// moving is [`Delivered::Unsubmitted`] — one double, one field apart.
    #[test]
    fn the_submit_is_witnessed_from_the_moment_it_is_pressed() {
        let watching = SubmittedWhen::Repaints {
            within: Duration::from_millis(50),
        };
        let deliver_onto = |after_submit: Option<&str>| {
            let panes = Recorder {
                after_submit: after_submit.map(ToOwned::to_owned),
                ..Recorder::showing("hello")
            };
            let outcome = deliver(
                &panes,
                &RunContext::uncancellable(),
                PaneId(1),
                "hello",
                &Delivery {
                    echo_timeout: Duration::from_millis(1),
                    attempts: 1,
                    submitted_when: watching,
                    ..Delivery::new()
                },
            )
            .expect("no error");
            (outcome, panes.injected.lock().expect("the log").clone())
        };

        let (took_it, log) = deliver_onto(Some("hello\u{2502} thinking"));
        assert!(
            took_it.is_confirmed(),
            "a peer whose screen moved AFTER the press submitted it — and the move is only \
             visible against a baseline read at the press: {took_it:?} with {log:?}",
        );

        let (absorbed, log) = deliver_onto(None);
        assert_eq!(
            absorbed,
            Delivered::Unsubmitted {
                attempts: 1,
                written: Written::of(6),
                wanted: watching,
            },
            "a screen that stopped moving at the press is a prompt sitting in a composer: {log:?}",
        );
        assert_eq!(
            log.iter()
                .filter(|keys| keys == &&vec!["Enter".to_owned()])
                .count(),
            1,
            "⚠⚠ AND THE SUBMIT IS NEVER PRESSED TWICE. A second Enter onto a composer the first \
             one emptied asks an empty question, which an agent answers — the module's own hazard, \
             met from the other side: {log:?}",
        );
    }

    /// ⚠⚠⚠ **A RUN THAT ENDS INSIDE THE SUBMIT'S WAIT HAS ALREADY SENT THE KEYSTROKE**, and says
    /// so — [`Delivered::Unwitnessed`], which is not [`Delivered::Stopped`].
    ///
    /// The pair is the claim. Both runs are cancelled and the two answers are opposite in the one
    /// way a caller acts on: the first has typed a prompt and asked nothing, so its supervisor may
    /// deliver it again; the second has pressed Enter, so the peer may be answering right now and a
    /// second delivery would be a second question.
    #[test]
    fn a_run_cancelled_after_the_submit_says_the_keystroke_went_out() {
        let watching = SubmittedWhen::Repaints {
            within: Duration::from_secs(2),
        };
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let panes = Recorder {
            // Never moves again after the press, so only the cancel can end the second wait.
            after_submit: None,
            cancel_on_submit: Some(Arc::clone(&cancel)),
            ..Recorder::showing("hello")
        };
        let began = Instant::now();
        let outcome = deliver(
            &panes,
            &RunContext::new(cancel),
            PaneId(1),
            "hello",
            &Delivery {
                echo_timeout: Duration::from_millis(1),
                attempts: 1,
                submitted_when: watching,
                ..Delivery::new()
            },
        )
        .expect("no error");
        assert_eq!(
            outcome,
            Delivered::Unwitnessed {
                attempts: 1,
                written: Written::of(6),
                wanted: watching,
            },
            "the run ended in the submit's wait, and the Enter is on the pseudoterminal — so \
             `nothing was asked` is the one thing this may not say",
        );
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "and it stops INSIDE the wait rather than riding out the contract's window: {:?}",
            began.elapsed(),
        );
        let log = panes.injected.lock().expect("the log").clone();
        assert_eq!(
            log.last(),
            Some(&vec!["Enter".to_owned()]),
            "the submit really was the last thing sent, which is what makes the answer above \
             different from `Stopped`: {log:?}",
        );

        // ⚠ THE TWIN, one moment earlier: cancelled while waiting for the TEXT, where nothing has
        // been submitted and `Stopped` means exactly what it says.
        let earlier = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let panes = Recorder {
            cancel_on_read: Some(Arc::clone(&earlier)),
            ..Recorder::showing("")
        };
        let outcome = deliver(
            &panes,
            &RunContext::new(earlier),
            PaneId(1),
            "hello",
            &Delivery {
                submitted_when: watching,
                ..Delivery::new()
            },
        )
        .expect("no error");
        assert_eq!(
            outcome,
            Delivered::Stopped {
                attempts: 1,
                written: Written::of(5),
            },
            "a run cancelled before the press submitted nothing, and its caller may act on that",
        );
        assert!(
            !panes
                .injected
                .lock()
                .expect("the log")
                .iter()
                .any(|keys| keys == &vec!["Enter".to_owned()]),
            "no submit was sent at all",
        );
    }

    /// ⚠⚠ **A CONTRACT THIS HOST CANNOT ANSWER IS REFUSED AT ONCE, not waited out.**
    ///
    /// [`SubmittedWhen::Stirs`] over a host with no supervisor at all is never satisfiable — there
    /// is no observation to compare against, now or in two seconds — and spending the window on it
    /// would be a delay with no information in it. The BOUND is the assertion: a build that polled
    /// its way to the same answer would take the whole grace, and the number here is a fiftieth of
    /// it.
    ///
    /// ⚠ It is the same direction [`ReadyWhen::Runs`](crate::readiness::ReadyWhen::Runs) takes on a
    /// host that cannot see the process table — *a question nothing can answer is answered NO* —
    /// and a caller who meets it wanted [`SubmittedWhen::Repaints`] or nothing at all.
    #[test]
    fn a_submit_contract_no_host_can_answer_is_refused_rather_than_waited_out() {
        let unanswerable = SubmittedWhen::Stirs {
            within: Duration::from_secs(5),
        };
        // ⚠ ITS SCREEN DOES MOVE FOR THE SUBMIT, which is what makes this about the contract
        // rather than about the pane: `Repaints` would be satisfied here in a millisecond.
        let panes = Recorder {
            after_submit: Some("hello, and the peer answered".to_owned()),
            ..Recorder::showing("hello")
        };
        let began = Instant::now();
        let outcome = deliver(
            &panes,
            &RunContext::uncancellable(),
            PaneId(1),
            "hello",
            &Delivery {
                echo_timeout: Duration::from_millis(1),
                attempts: 1,
                submitted_when: unanswerable,
                ..Delivery::new()
            },
        )
        .expect("no error");
        let took = began.elapsed();
        assert_eq!(
            outcome,
            Delivered::Unsubmitted {
                attempts: 1,
                written: Written::of(6),
                wanted: unanswerable,
            },
            "a pane no supervisor can see never stirs, however loudly its screen moves",
        );
        assert!(
            took < Duration::from_millis(100),
            "and the answer is immediate: nothing arriving later could change it, so the window is \
             not spent. Took {took:?} of a five-second contract",
        );
    }

    /// ⚠⚠⚠⚠ **THE TWO WAITS DISAGREE ABOUT A DEADLINE THAT PASSED WHILE THE EVIDENCE WAS ALREADY
    /// THERE, AND NOTHING SAID SO** — a measurement, pinned here so the disagreement is visible.
    ///
    /// [`RunContext::stopped`](crate::run::RunContext::stopped) says it is *"the predicate every
    /// bounded wait consults, so neither of the two ways a run ends from outside can be honoured by
    /// one wait and missed by another"*. That holds for CANCEL. It does not hold for the DEADLINE,
    /// because the two waits order it against the evidence differently and both wrote down a reason:
    ///
    /// * [`poll_until`](crate::run::poll_until) asks cancel, then the predicate, **then** the
    ///   deadline — *"work that finished is never thrown away by a clock that ran out while it was
    ///   finishing"*.
    /// * every loop in THIS file asks `stopped()` — cancel and deadline together — **first**, so a
    ///   delivery whose evidence is on the screen reports [`Seen::Stopped`] and its caller answers
    ///   [`Delivered::Unwitnessed`]: *the keystroke went out and nobody watched*, about a keystroke
    ///   this pane could have witnessed on the very next line.
    ///
    /// ⚠⚠⚠ **NEITHER IS OBVIOUSLY WRONG, WHICH IS EXACTLY WHY IT IS PINNED RATHER THAN «FIXED».**
    /// A run out of time that discards evidence it holds is under-reporting; a run out of time that
    /// keeps gathering evidence is spending a window it does not have. The decision is the owner's;
    /// what this gate refuses to allow is that it go on being made twice, differently, by accident.
    /// It goes RED when either side moves — which is the point, and then the note above is the
    /// argument to settle rather than a surprise to rediscover.
    #[test]
    fn a_passed_deadline_beats_the_evidence_in_this_file_and_loses_to_it_in_poll_until() {
        let panes = Recorder::showing("PONG");
        let wrote = panes
            .inject(PaneId(1), &KeyStroke::text("ping"))
            .expect("the double takes what it is given");
        assert!(wrote.bytes() > 0, "the double must have taken the keys");
        assert_eq!(
            panes.pane_collapsed(PaneId(1)).as_deref(),
            Some("PONG"),
            "the fixture must stage EVIDENCE ALREADY ON THE SCREEN, or neither wait has anything \
             to weigh the clock against",
        );

        // Out of time and NOT cancelled — the one arrangement that tells the two rules apart,
        // since `stopped()` collapses them and only the deadline is ordered differently.
        let out_of_time = crate::run::RunContext::uncancellable().deadline_in(Some(Duration::ZERO));
        assert!(out_of_time.expired() && !out_of_time.cancelled());

        assert_eq!(
            crate::run::poll_until(&out_of_time, Duration::from_secs(30), || true),
            crate::run::Waited::Ready,
            "`run.rs`'s rule: a finished predicate survives a clock that ran out while it finished",
        );
        assert!(
            matches!(
                await_text(
                    &panes,
                    &out_of_time,
                    PaneId(1),
                    "PONG",
                    Duration::from_secs(30),
                    None,
                ),
                Seen::Stopped,
            ),
            "and this file's rule is the opposite one, on the same context and the same evidence",
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

    /// ⚠⚠⚠ **A SUBMIT NO PROGRAM EVER READ USED TO COME BACK AS A DELIVERY** — register item 225,
    /// and the gate that says it does not any more.
    ///
    /// Measured over this fixture's own peers, with the rule that preceded [`SubmittedWhen`]:
    ///
    /// | peer | what a delivery said | did the screen move again? |
    /// |---|---|---|
    /// | deaf to the submit | `Confirmed { attempts: 1, written: 18 }` in 10.22 ms | never, in 2 s |
    /// | takes the submit | `Confirmed { attempts: 1, written: 18 }` in 10.22 ms | in 2.10 ms |
    ///
    /// **The same answer, to the digit, for the peer that was asked and the peer that was not.**
    ///
    /// ⚠ THREE READINGS, and the third is what stops this being a gate about one arm: the SAME deaf
    /// peer, delivered to by a caller who asked nothing of the submit, must still be `Confirmed` —
    /// or the fix would have made *"deliver to a peer whose reaction you cannot see"* impossible,
    /// and `exec cat` is that peer.
    #[test]
    fn a_submit_the_peer_never_read_is_not_reported_as_a_delivery() {
        const PROMPT: &str = "what is 2 plus 2?";
        /// Short: the SUBJECT spends this whole window before answering, and it is a fixture's
        /// wait rather than a peer's — nothing here paints slowly.
        const GRACE: Duration = Duration::from_millis(150);

        let deliver_over = |reacts: Reacts, submitted_when: SubmittedWhen| {
            let (access, pane) = ready_peer(&takes_a_prompt_of(PROMPT.len(), reacts));
            let outcome = deliver(
                &access,
                &RunContext::uncancellable(),
                pane,
                PROMPT,
                &Delivery {
                    // ⚠ ONE ATTEMPT. A retry would inject the prompt a second time into a `dd`
                    // that has already counted its bytes out, which is a different experiment.
                    attempts: 1,
                    submitted_when,
                    ..Delivery::new()
                },
            )
            .expect("a peer that ignores a keystroke is not an error");
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome, screen)
        };

        let watching = SubmittedWhen::Repaints { within: GRACE };
        let (subject, subject_screen) = deliver_over(Reacts::Nothing, watching);
        assert_eq!(
            subject,
            Delivered::Unsubmitted {
                attempts: 1,
                // 17 bytes of prompt and the Enter after it: the submit is PAID FOR, whatever
                // became of it.
                written: Written::of(PROMPT.len() as u64 + 1),
                wanted: watching,
            },
            "⚠⚠⚠ THE PEER IS BLOCKED IN `sleep` WITH THE SUBMIT BYTE UNREAD, so no reading of it \
             as a delivered question is true. Screen: {subject_screen:?}",
        );
        // THE CONTROL WITHIN THE SUBJECT: the text really did arrive, so this is a gate about the
        // KEYSTROKE and not one that passes because nothing was ever delivered.
        assert!(
            subject_screen.contains(PROMPT),
            "the prompt itself must be plainly on that screen — otherwise this measures the text's \
             own read-back a second time: {subject_screen:?}",
        );
        assert!(
            subject.is_on_screen(),
            "and the answer must SAY the text is there, because that is what stops a caller \
             delivering again on top of it: {subject:?}",
        );

        let (control, control_screen) = deliver_over(Reacts::Works, watching);
        assert!(
            control.is_confirmed(),
            "⚠⚠⚠ THE CONTROL: a peer that READS the submit and paints must still be confirmed, or \
             the rule refuses every delivery there is. Got {control:?} over {control_screen:?}",
        );

        let (unasked, _) = deliver_over(Reacts::Nothing, SubmittedWhen::Unchecked);
        assert!(
            unasked.is_confirmed(),
            "⚠⚠ THE DEFAULT, over the SAME deaf peer: a caller who asks nothing of the submit gets \
             the answer this module always gave — see `SubmittedWhen::Unchecked`, which exists so \
             that a peer whose reaction is invisible can still be delivered to: {unasked:?}",
        );
    }

    /// ⚠⚠⚠ **A SCREEN THAT MOVED IS NOT A TURN THAT STARTED** — the two contracts, told apart by
    /// the peer that satisfies one and not the other.
    ///
    /// The peer here READS the submit and paints one character for it, which is what an agent's
    /// composer does with a printable key: [`SubmittedWhen::Repaints`] is satisfied and no question
    /// was asked. Measured live against `claude` before this existed — a coalesced `…prompt…\r`
    /// read as a paste repaints the composer exactly like a submitted one, which is why register
    /// item 222's prompt sat unsent under an idle agent for a minute.
    ///
    /// So the pane is put under a SUPERVISOR — a stand-in for the daemon's detector, deriving its
    /// verdict from what the peer PRINTED rather than from a value this test sets by hand — and
    /// [`SubmittedWhen::Stirs`] asks it. ⚠ The pair in the middle is the whole finding: the same
    /// delivery, the same screen, opposite answers. The CONTROL (a peer that really does start
    /// working) is what proves the strict rule is satisfiable at all.
    #[test]
    fn a_screen_that_only_repainted_is_not_an_agent_that_stirred() {
        const PROMPT: &str = "what is 2 plus 2?";
        const GRACE: Duration = Duration::from_millis(150);

        let deliver_over = |reacts: Reacts, submitted_when: SubmittedWhen| {
            let (access, pane) =
                supervised_peer(&takes_a_prompt_of(PROMPT.len(), reacts), Publishes::Plainly);
            let outcome = deliver(
                &access,
                &RunContext::uncancellable(),
                pane,
                PROMPT,
                &Delivery {
                    attempts: 1,
                    submitted_when,
                    ..Delivery::new()
                },
            )
            .expect("a peer that ignores a keystroke is not an error");
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome, screen)
        };

        let watching = SubmittedWhen::Repaints { within: GRACE };
        let supervising = SubmittedWhen::Stirs { within: GRACE };

        let (repainted, repainted_screen) = deliver_over(Reacts::Paints, watching);
        assert!(
            repainted.is_confirmed(),
            "⚠ THE RESIDUE `Repaints` DECLARES, MEASURED: a peer that merely paints a character for \
             the keystroke satisfies it. That is not a defect in the kind — it is why the kind is \
             the caller's to choose. Got {repainted:?} over {repainted_screen:?}",
        );

        let (absorbed, absorbed_screen) = deliver_over(Reacts::Paints, supervising);
        assert_eq!(
            absorbed,
            Delivered::Unsubmitted {
                attempts: 1,
                written: Written::of(PROMPT.len() as u64 + 1),
                wanted: supervising,
            },
            "⚠⚠⚠ THE SAME PEER AND THE SAME SCREEN, asked the stronger question: it took the \
             keystroke, painted for it, and never started working — which is a prompt sitting in a \
             composer. Screen: {absorbed_screen:?}",
        );

        let (stirred, stirred_screen) = deliver_over(Reacts::Works, supervising);
        assert!(
            stirred.is_confirmed(),
            "⚠⚠⚠ THE CONTROL: a peer whose supervisor publishes a change must be confirmed, or \
             `Stirs` is a contract nothing can satisfy and the gate above proves nothing. Got \
             {stirred:?} over {stirred_screen:?}",
        );

        let (deaf, deaf_screen) = deliver_over(Reacts::Nothing, supervising);
        assert!(
            matches!(deaf, Delivered::Unsubmitted { .. }),
            "and a peer that read nothing at all is refused by this kind too: {deaf:?} over \
             {deaf_screen:?}",
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
