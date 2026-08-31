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
//! ## ⚠⚠⚠⚠⚠ And a screen can be showing something ELSE for what it took
//!
//! Every hazard above is a way of being wrong about who painted the text, or when. This one is a way
//! of the text not being paintable at all: **an agent's composer FOLDS a long paste.** `claude`
//! 2.1.233 shows `[Pasted text #2 +5 lines]` and the prompt's own characters are nowhere on the
//! pane — at any width, in any alphabet — so a read-back over that screen is asking a question the
//! screen has thrown the answer away for.
//!
//! Measured on three live runs in one evening (register item 421): the delivery re-injected until it
//! gave up, leaving 4,002 bytes of ONE prompt in the composer, and refused. The prompt it died on was
//! the driver's own reflection prompt, so no caller's wording could shorten it, and a loop that
//! cannot deliver that never replaces a session or chooses a next milestone.
//!
//! Two things came out of it, and the first is the one that generalises. **The wait's *not there* was
//! two situations wearing one word** — the screen never moved (nothing took the bytes; inject again)
//! and the screen moved without showing the text (something took them; injecting again puts a SECOND
//! COPY in that composer). The wait answers those separately now — `OnScreen`, private to this
//! module, where the argument for the split is written out. And on the second of the two, where the
//! caller's peer can name the question it received ([`SubmittedWhen::Took`]), the submit goes in and
//! **the agent's own account is the verdict**: [`Delivered::Reported`].
//!
//! ⚠ The screen path stays and stays needed — a peer with no hooks has nothing else, and for it
//! nothing has changed. What changed is that where a program CAN speak, a rendering no longer gets to
//! refuse what the program itself confirms.
//!
//! ## ⚠⚠⚠⚠⚠ And the peer can be unable to take a question AT ALL — the front of the same door
//!
//! Every hazard above is about what became of bytes that went in. This one is about whether they
//! should go in yet: **an agent that is inside a tool call is not an agent at rest**, and a prompt
//! typed at one opens no turn this run can hold to a contract.
//!
//! The cost of getting it wrong was measured (register item 745) and it is not a prompt: a live
//! loop met a peer that did not turn its Enter into a question, replaced its whole session over the
//! refusal, and — because a run that folds a DIFFERENT prompt each time never trips the *same bytes
//! twice* guard — recovered for ever and called nobody.
//!
//! ⛔⛔ **THE SAMPLE THAT MOTIVATED THIS DOOR DID NOT SHOW WHAT IT WAS READ AS SHOWING**, and the
//! correction is here rather than in a register nobody compiles. That pane's status line said
//! `1 shell still running` where its four neighbours said `esc to interrupt`, and the sentence
//! written from it — *a `claude` running a child does not turn an Enter into a question* — was
//! about a BACKGROUNDED shell. Driven on 2026-08-29 against a live `claude` with exactly one such
//! shell running, four deliveries of 48, 359, 939 and 2,369 bytes all submitted and were answered,
//! the last of them through this module's own shape (inject, wait for the echo, press the key
//! afterwards) at a size the composer folded. **A background shell does not refuse an Enter, and
//! what refused that one is not yet known.** See
//! `tests::a_background_shell_on_the_screen_is_not_a_child_this_door_holds_for`, which is where
//! that refutation is a gate instead of a paragraph.
//!
//! So there is a hold in front of the typing — [`hold_while_a_child_runs`] — and its answer is
//! [`Held`]. What it consults is the AGENT'S OWN WORD about a tool call in flight, which is a fact
//! about this turn rather than about the screen. ⚠⚠ It WAITS rather than refusing on sight, because
//! a tool call ends; refusing on sight would be a loop that sends nothing, which is why the gate on
//! it stages a busy peer AND a free one and would be passed by neither alone.
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
/// What the number BUYS is the difference between a slow peer and a deaf one, and the cost of being
/// generous is paid only where the submit really did not land — so it sits above the slowest
/// observation rather than tight to it.
///
/// # ⚠⚠⚠⚠⚠ Six seconds, and the number is READ OFF THE HOOK rather than chosen — item 669
///
/// This was **two seconds**, sized on readings taken against `claude` **2.1.233**: a peer that
/// paints what it took does so in **2.10 ms** over a local pty, and that build answered
/// [`SubmittedWhen::Stirs`] in **100.51 ms** of the Enter going in.
///
/// Those readings are about a PAINT. The contract the loop actually runs is
/// [`SubmittedWhen::Took`], whose evidence is the agent's own account — and that account travels a
/// different path: `claude` fires `UserPromptSubmit`, which SPAWNS `sprag hook claude`, which opens
/// a socket, handshakes, and reports. **Measured 2026-08-25 on an idle host: the spawn alone is
/// 2-3 ms, and spawn + socket + handshake is 514-541 ms.** A hundred milliseconds was never the
/// figure this contract had to clear.
///
/// ⛔⛔ **AND TWO NUMBERS INSIDE ONE PRODUCT DISAGREED.** The hook `sprag` writes into every agent's
/// `--settings` document declares `"timeout": 5` — **five seconds** — so this waited less than a
/// third of the time it permits its own evidence channel to take. Six seconds is that threshold and
/// not a guess: past it, *no report* is a fact ABOUT THE HOOK (which has been killed) rather than a
/// guess about timing.
///
/// ⚠⚠ **WHAT A SHORT GRACE COSTS IS PERMANENT, WHICH IS WHY IT MATTERED.**
/// [`Delivered::Unsubmitted`] is refused and never retried — correctly, since the composer is
/// holding the prompt and a second delivery would concatenate onto it. So a submit that LANDED and
/// reported late is written off for good: the run counts it as never asked and cannot learn
/// otherwise. Live runs were reporting up to 13 such prompts each.
///
/// ⚠ **The residue, stated**: five seconds bounds the hook PROCESS. When `claude` fires it after an
/// Enter is not bounded from here at all, so this closes the leg sprag can see and no more. The
/// design that removes the race rather than widening it — asking what the composer HOLDS, which is
/// a property rather than an event — is register item 669's, and it is not this constant.
///
/// ⚠ It is deliberately NOT the number a caller must use. [`Turn`](crate::completion::Turn)'s rule:
/// how long a peer may take is the caller's to say, and a delivery into a box on the far side of an
/// ssh hop is a different peer from this one.
pub const DEFAULT_SUBMIT_GRACE: Duration = Duration::from_secs(6);

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
    /// **THE COMPOSER HAS LET GO OF IT** — the pane was holding an unsubmitted prompt when the
    /// submit went in ([`AgentObservation::holding`](crate::access::AgentObservation::holding)) and
    /// is not holding one now, within `within`.
    ///
    /// # ⚠⚠⚠⚠⚠ The evidence is a PROPERTY, and every other kind here is an EVENT
    ///
    /// The three above wait for something to HAPPEN — a repaint, a published change, a report. That
    /// shape has a failure mode none of them can escape: when the thing does not happen, there is
    /// nothing to distinguish *it has not happened yet* from *it is never going to*, so the wait
    /// runs out and the answer is a guess dressed as a timeout. Register item 669 measured what
    /// that costs — of five live runs, four had prompts that were never asked, and the run could
    /// not tell, because the ONLY channel for *not submitted* is silence on the channels for
    /// *submitted*.
    ///
    /// A composer is not an event. It is holding, or it is not, and both readings are STABLE — so
    /// this contract converges where the others merely expire. The bound moves off *how long a
    /// third party takes to speak* (hundreds of milliseconds, with an unbounded tail) and onto *how
    /// long this pane takes to repaint*, which this module's own measurement puts at **2.10 ms**.
    ///
    /// # ⚠⚠ Both ends are required, and the baseline is what makes it evidence
    ///
    /// * **It must have been holding when the submit went in.** *A condition already true when you
    ///   started is not evidence* is the rule the text's own read-back and
    ///   [`Repaints`](Self::Repaints) are both held to; here it is sharper, because *not holding*
    ///   is the SATISFIED reading. Armed against a pane that was not holding, this would answer yes
    ///   to a submit that was never pressed. So a pane not holding at arming can never satisfy it,
    ///   and says so at once rather than spending the window — [`Stirs`](Self::Stirs)' rule for a
    ///   host with no supervisor, for the same reason.
    /// * **The pane must still be the same agent's**, which is what makes this a claim about the
    ///   peer the submit went to rather than about whatever is in the pane now.
    ///
    /// ⚠ Deliberately NOT part of it: that the observation's
    /// [`seq`](crate::access::AgentObservation::seq) moved. That is the EVENT discipline
    /// [`Stirs`](Self::Stirs) and [`Took`](Self::Took) need, and requiring it here would put back
    /// exactly the dependency this kind exists to drop.
    ///
    /// # ⚠⚠⚠ WHAT IT CANNOT SEE TODAY, measured rather than supposed
    ///
    /// * **A composer holding a prompt too short to FOLD.** The state is read off the placeholder
    ///   an agent paints for a long paste; a short prompt sits in the composer as itself and the
    ///   pane reads `Idle`. Right for a supervisor, whose prompts are long enough to fold — which
    ///   is how the fold was found — and wrong for a person typing a word.
    /// * ⛔ **A pane whose agent REPORTS.** `sprag_detect`'s tracker does not run the manifest's
    ///   rules on a pane a hook is reporting, so `Holding` is never published for one — pinned by
    ///   `a_reported_pane_holding_a_paste_is_not_read_as_holding` in that crate. That is the
    ///   population a supervisor drives, so this contract REFUSES there rather than passing: the
    ///   baseline cannot be armed, and an unanswerable contract says so. Lifting it is the same
    ///   change register item 524 already made for one other screen fact, and it is not made here.
    Released {
        /// How long to wait for that, after which the delivery answers
        /// [`Delivered::Unsubmitted`].
        within: Duration,
    },
    /// **THE AGENT HAS NAMED THE QUESTION IT RECEIVED, AND IT IS THIS ONE** — its own submit hook
    /// reported the prompt, within `within`.
    ///
    /// # ⚠⚠⚠⚠ The strongest of the four, and the only one that is about the TEXT
    ///
    /// The three above answer *did something happen after the keystroke*. This answers *was my
    /// question the one that was asked* — which is the question a delivery has always been trying
    /// to answer and could not, because everything else it could reach is a rendering.
    ///
    /// **A SCREEN CANNOT SETTLE IT, AND THAT IS MEASURED RATHER THAN ARGUED.** Text a run delivered
    /// and text a composer was already holding are the same pixels, so the read-back's `contains`
    /// says *something like mine is on the pane*: the gate at
    /// `a_prompt_typed_onto_a_dirty_composer_is_confirmed_and_submitted_anyway` drives exactly that
    /// and records that tightening the predicate is RULED OUT — `ends_with` was tried, it did not
    /// fix the case and it reddened a neighbour whose whole existence is that a needle may be a
    /// fragment. Its conclusion names this: *"evidence from the PROGRAM rather than the screen"*.
    ///
    /// ⚠⚠⚠ **AND THE SCREEN ORACLE HAS BEEN PATCHED THREE TIMES WITHOUT BECOMING SOUND** — 40
    /// characters became 40 COLUMNS when a Korean prompt asked for twice the pane's width; the HEAD
    /// became the TAIL when a composer was found to scroll it away; an exact match became a
    /// whitespace-insensitive one when the box was found to re-wrap what it was given. Three live
    /// runs died at that predicate in one evening, the last on a prompt no caller could shorten.
    ///
    /// # ⚠⚠ Both halves are required, and each rules out a different false yes
    ///
    /// * **The reported prompt must match this delivery's text**, whitespace aside — so a composer
    ///   that appended this text to somebody else's reports a longer question and is refused.
    /// * **The observation must have MOVED since the submit went in** ([`Stirs`](Self::Stirs)'
    ///   `seq` discipline) — so a pane still holding the report of an IDENTICAL earlier turn cannot
    ///   satisfy it. A loop repeating one prompt is exactly the population that would.
    ///
    /// ⚠ Never satisfied where nothing can answer: a host with no supervisor, a pane no manifest
    /// claims, or an agent with no hooks installed — which reports no prompt at all. That is
    /// [`Stirs`](Self::Stirs)' rule and [`ReadyWhen::Runs`](crate::readiness::ReadyWhen::Runs)': a
    /// contract that cannot be answered says so rather than being read as a yes, and the caller
    /// falls back to a weaker contract deliberately rather than by accident.
    Took {
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
            Self::Repaints { within }
            | Self::Stirs { within }
            | Self::Released { within }
            | Self::Took { within } => Some(within),
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
            // ⚠ It names the COMPOSER, because that is the thing that did not move — and a pane
            // that could not be armed reaches this sentence too, which is why it says what is still
            // true of the pane rather than claiming the wait was spent.
            Self::Released { .. } => "is still holding the prompt in its composer",
            // ⚠ It names the QUESTION rather than the pane, because that is what went unanswered:
            // the peer may well have stirred, and what did not arrive is any account of having been
            // asked THIS. A caller reading *"did not stir"* here would go looking at the wrong end.
            Self::Took { .. } => "never reported the question it was asked",
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
    /// **THE PROGRAM NAMED THIS QUESTION AS THE ONE IT RECEIVED, AND ITS SCREEN NEVER SHOWED THE
    /// TEXT AT ALL** — the strongest answer this module has, and the only one that is not about a
    /// rendering.
    ///
    /// # ⚠⚠⚠⚠ Why a screen had to stop being able to refuse a delivery — register item 421
    ///
    /// An agent's composer FOLDS a long paste. `claude` 2.1.233 shows `[Pasted text #2 +5 lines]`
    /// and the prompt's own characters are nowhere on the pane, so a read-back over that screen is
    /// asking a question the screen has thrown away the answer to. Measured on three live runs in
    /// one evening: the delivery re-injected until it gave up, leaving three copies of the prompt in
    /// the composer, and refused with [`Unconfirmed`](Self::Unconfirmed) — whose sentence blames a
    /// narrow pane and a peer that painted nothing, and **both were false**. The prompt it died on
    /// was composed by the driver itself, so no caller could shorten it, and a loop that cannot
    /// deliver a reflection prompt never replaces a session or chooses a next milestone.
    ///
    /// So where the caller's peer can name the question it took ([`SubmittedWhen::Took`]), a screen
    /// that MOVED without showing the text no longer ends the delivery: the submit is pressed and
    /// **the agent's own account is the verdict.**
    ///
    /// ⚠⚠ **IT IS A SEPARATE WORD FROM [`Confirmed`](Self::Confirmed) BECAUSE THE SCREEN CLAIM IS
    /// GONE.** `Confirmed` says *the text is on the pane and the program painted it*, which a caller
    /// may act on — this says *the program has it*, which is strictly stronger about the delivery and
    /// says nothing at all about what a person looking at that pane would see. Collapsing the two
    /// would make [`is_on_screen`](Self::is_on_screen) a guess.
    ///
    /// ⚠ Unreachable for a peer with no hooks, and that is what keeps the old rule for it: an agent
    /// that reports no question can never satisfy the contract that produces this, so its deliveries
    /// stay on the screen predicate rather than being pressed blind.
    Reported { attempts: u32, written: Written },
    /// ⛔⛔⛔⛔⛔ **THE COMPOSER LET GO OF THE FOLDED PROMPT, AND THE AGENT NEVER NAMED IT** —
    /// register item 762, and the answer that used to be [`Unreported`](Self::Unreported).
    ///
    /// # ⛔⛔⛔⛔ Why the two had to be separated, measured
    ///
    /// The fold road had ONE channel: the agent's own account ([`SubmittedWhen::Took`]). That
    /// contract EXPIRES — when the account does not come there is nothing to tell *not yet* from
    /// *never* — so its silence was reported as *the peer would not take the question*, which the
    /// driver answers with a session replacement. Register item 669 measured the cost of a single
    /// expiring channel (four of five live runs held prompts that were never asked and could not
    /// say so); register item 762 watched `run110` die of it at 187 iterations, with the pane it
    /// had just been driving reading `working seq=26 said=12`.
    ///
    /// **Those are two situations with opposite remedies.** A prompt still sitting in the composer
    /// is a wedged session and replacing it is right (register item 446). A prompt that has LEFT
    /// the composer was asked — the peer is working on it — and replacing that session throws away
    /// a turn already paid for. [`SubmittedWhen::Released`] CONVERGES where `Took` expires, so
    /// arming it beside the account is what makes the two distinguishable at all.
    ///
    /// # ⚠⚠⚠ What it does NOT claim, and the care is the point
    ///
    /// * **Not which question was asked.** `Reported` compares the agent's account against this
    ///   delivery's text; this knows only that the box is empty. A composer that was already dirty
    ///   submits this text appended to somebody else's, and this answer cannot tell — the residue
    ///   [`SubmittedWhen::Released`]'s own doc records, not a new one.
    /// * **Not that the text is on the screen.** It never was: this road is
    ///   the road where the screen MOVED WITHOUT THE TEXT, a folded paste, so
    ///   [`is_on_screen`](Self::is_on_screen) stays false.
    ///
    /// ⚠ Unreachable where nothing can read a composer — a host with no supervisor, a pane no
    /// manifest claims, a manifest authoring no `Holding` rule, or a daemon too old to send the
    /// key. All of those keep the old answer, which is [`Unreported`](Self::Unreported): an absence
    /// of the instrument is not evidence that anything was asked.
    Released { attempts: u32, written: Written },
    /// Every attempt was written and none of them ever appeared. The bytes went to the pty; the
    /// program behind it did not show them.
    ///
    /// ⚠⚠ **AND FOR A PEER THAT CAN NAME WHAT IT WAS ASKED, *NEVER APPEARED* NOW MEANS THE SCREEN
    /// NEVER MOVED** — a pane that moved without showing the text is
    /// [`Reported`](Self::Reported)'s, or a refusal that names the agent's silence. The distinction
    /// is what stopped a folded paste from being read as a swallowed one; see that answer's doc.
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
    /// **THE COMPOSER SWALLOWED THE PASTE AND THE AGENT NEVER NAMED THE QUESTION** — the refusal
    /// that pairs with [`Reported`](Self::Reported), on the one road where the pane cannot answer.
    /// Register item 762.
    ///
    /// # ⛔⛔⛔⛔⛔ It was [`Unsubmitted`](Self::Unsubmitted), and that answer's remedy is the
    /// opposite of this one's
    ///
    /// Both refusals used to be spelled `Unsubmitted`, so both reached a supervisor as *"the text
    /// was read back off a screen this delivery changed … the prompt is therefore sitting in the
    /// pane"*. On THIS road the screen moved **without** the text (`OnScreen::MovedWithoutIt`) —
    /// that is what a composer folding a long paste does — so neither half is true: nothing was
    /// read back, and what is sitting in the pane is `[Pasted text +N lines]`.
    ///
    /// **Measured, at the cost of a session.** `run110` (2026-08-31) ended here, its record said
    /// *go and look at the pane*, and the round reading it went and looked, found a healthy pane,
    /// and spent its round on brief SIZE instead. [`crate::plugin::Deliveries::unsubmitted`]'s own
    /// doc had already
    /// forbidden exactly this — *"counting them as one number would be counting two remedies as
    /// one"* — and the fold road was simply missed when that call was made.
    ///
    /// ⚠⚠ The remedy this one carries: **do not go to the pane, go to the agent's own record.** The
    /// only evidence a folded paste can ever produce is the peer naming what it was asked, and it
    /// did not — so what is worth knowing is whether the peer's hooks are reporting at all, and
    /// what this run's session-replacement budget has left ([`crate::outer::Retyped`]).
    ///
    /// ⚠ `attempts` is 1 by construction here and that is not a measurement of the peer: the fold
    /// road returns out of the injection loop, so the two spare attempts
    /// ([`DEFAULT_ATTEMPTS`]) are unreachable. A second injection would land on the first one's
    /// text, which is why — but a reader who takes `attempts: 1` for *the pane was tried once and
    /// refused once* is reading this module's control flow as a fact about the world.
    Unreported {
        /// How many injections carried the TEXT. Always 1 on this road — see the type's doc.
        attempts: u32,
        /// Every byte that reached the pty, the submit's own among them.
        written: Written,
        /// The contract that went unsatisfied — always a [`SubmittedWhen::Took`], because this road
        /// is only taken for a peer that can name what it was asked.
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
    ///
    /// ⚠⚠ **TRUE FOR [`Reported`](Self::Reported), WHICH IS THE SAME QUESTION ANSWERED BY THE
    /// PROGRAM ITSELF.** A peer that names the question it received has said it is holding the text,
    /// which is what this asks — and a stronger source than the paint `Confirmed` is read off.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed { .. } | Self::Reported { .. })
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
    ///
    /// ⚠⚠⚠ **AND FALSE FOR [`Reported`](Self::Reported), WHICH IS THE ONE ANSWER WHERE THE TWO
    /// QUESTIONS COME APART.** That delivery is confirmed by the program and its text was never seen
    /// on the pane at all — a composer had folded the paste away — so a caller reading this to mean
    /// *there is text a person can see* must be told no. It is the reason the two answers are
    /// separate words.
    ///
    /// ⚠ **AND FALSE FOR [`Unreported`](Self::Unreported), WHICH IS THAT SAME ROAD'S REFUSAL** —
    /// register item 762. It is `matches!`, so a new variant is false by default here, and for this
    /// one the default is the right answer rather than a lucky one: the screen moved WITHOUT the
    /// text, which is the whole meaning of the answer.
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
            | Self::Reported { written, .. }
            | Self::OnScreenOnly { written, .. }
            | Self::Unconfirmed { written, .. }
            | Self::Unsubmitted { written, .. }
            | Self::Released { written, .. }
            | Self::Unreported { written, .. }
            | Self::Stopped { written, .. }
            | Self::Unwitnessed { written, .. } => written,
        }
    }
}

/// **WHAT PROVED THE PROMPT ARRIVED** — the evidence one delivery was accepted on, kept so a run's
/// walk can say it and a person reading one knows what they will find on the pane.
///
/// # ⚠⚠⚠⚠ Why a delivery that SUCCEEDED still owes a word — register item 434
///
/// [`Delivered::Confirmed`] and [`Delivered::Reported`] are both successes and they say OPPOSITE
/// things about the pane: one has the prompt painted on it and the other has nothing of the prompt
/// on it at all, because a composer folded the paste away. Every caller reduced the two to *how
/// many bytes went in* — `OuterLoop::say` answered `Ok(bytes)` — so **a supervisor sent to look at
/// the pane for a prompt that was folded away had nothing telling them it will not be there.** It
/// is register item 423's disease in a fourth place: the result is written down and the grounds are
/// not.
///
/// # ⚠⚠⚠⚠⚠ AND IT IS WHAT MAKES ITEM 421'S ROAD READABLE OFF A LIVE RUN — register item 433
///
/// That fix's claim is that a prompt a composer folded away is delivered on the AGENT'S OWN
/// ACCOUNT, and its own entry says no gate can schedule the proof: only a live run retires it.
/// [`Account`](Self::Account) in a walk **is** that proof, read where it happened.
///
/// The first attempt at taking it had to go the other way round and the difference is the argument
/// for this type. On 2026-08-18 a live run reached `reflecting`, delivered the driver's own
/// reflection prompt and replaced its session — and NOTHING in the run said which road the delivery
/// took. It had to be reconstructed afterwards from arithmetic on the walk's byte counts against the
/// agent's transcript (2 × 1,314 + 1 = the 2,629 the walk reported, so two injections and one
/// submit, so the first was swallowed and the screen carried the second) — a reading nobody
/// supervising a run will do, and one that is only possible while the transcript still exists.
///
/// # ⚠⚠ Why a word rather than the two `bool`s that already answer it
///
/// [`Delivered::is_confirmed`] and [`Delivered::is_on_screen`] answer this between them, and a
/// caller carrying the pair would be publishing a two-bit code with four readings, of which no
/// delivery produces two. A closed vocabulary is what a journal LINE can render and a reader can
/// scan for.
///
/// ⚠ **NO WORD IN FRONT OF THE SENTENCE**, deliberately, and for [`crate::outer::Heard`]'s reason:
/// nothing in this product spells any of these as a wire value or a datamodel word, so inventing
/// one here would publish a spelling nothing serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Witnessed {
    /// The pane painted the prompt back and nothing but the program could have painted it —
    /// [`Delivered::Confirmed`]. The ordinary road, and the one a person can check by eye.
    Painted,
    /// The prompt is on the screen and the TERMINAL may be what put it there — the pane's echo is
    /// on, so this is not evidence the program read a byte ([`Delivered::OnScreenOnly`]).
    Echoed,
    /// ⚠⚠⚠⚠⚠ **THE AGENT NAMED THIS QUESTION AS THE ONE IT RECEIVED, AND ITS SCREEN NEVER CARRIED
    /// THE TEXT** — [`Delivered::Reported`], register item 421's road.
    ///
    /// The strongest evidence a delivery has and the one where LOOKING AT THE PANE ANSWERS NOTHING:
    /// the composer folded the paste away, so a supervisor sent there finds `[Pasted text +N lines]`
    /// where they were told to expect a prompt.
    Account,
    /// ⛔⛔⛔⛔⛔ **THE COMPOSER LET GO OF IT AND NOBODY NAMED IT** — [`Delivered::Released`],
    /// register item 762.
    ///
    /// Weaker than [`Account`](Self::Account) by exactly one thing and the gap is worth a word: the
    /// account names the TEXT, and this names only that the box the prompt was folded into is empty
    /// now. A question was asked; which one is the peer's to say and it has not said.
    ///
    /// ⚠ It is still EVIDENCE OF A DELIVERY, which is what separates it from the refusals below: a
    /// prompt that left the composer is not a prompt a session replacement would rescue.
    LetGo,
    /// Nothing was asked of the screen, because this peer paints nothing until its prompt is
    /// submitted — the caller's `shows_the_prompt` is false, so the bytes went in and the submit
    /// was pressed on trust.
    ///
    /// ⚠ Not a failure and not a weaker [`Painted`](Self::Painted): it is the honest answer for a
    /// peer whose screen could never have carried the evidence.
    Unchecked,
    /// The run ended between the typing and the submit — [`Delivered::Stopped`]. Nothing was asked,
    /// and the composer may be holding the text.
    Unasked,
    /// The submit went out and the run ended before its evidence could arrive —
    /// [`Delivered::Unwitnessed`]. A question may well have been asked; nothing here saw it land.
    Unproven,
}

impl Witnessed {
    /// The evidence `delivered` was accepted on, or [`None`] for the two answers that are REFUSALS
    /// rather than deliveries ([`Delivered::Unconfirmed`] and [`Delivered::Unsubmitted`]) — whose
    /// callers turn them into a named [`PaneError`] and never reach a walk's evidence channel at
    /// all.
    ///
    /// ⚠⚠ EXHAUSTIVE over [`Delivered`], so an eighth answer arrives here as a variant that no
    /// longer compiles rather than as a delivery a walk silently says nothing about.
    #[must_use]
    pub const fn of(delivered: Delivered) -> Option<Self> {
        match delivered {
            Delivered::Confirmed { .. } => Some(Self::Painted),
            Delivered::OnScreenOnly { .. } => Some(Self::Echoed),
            Delivered::Reported { .. } => Some(Self::Account),
            // ⚠⚠ A DELIVERY, and that is the decision — register item 762. Its evidence is weaker
            // than `Account` and it is evidence all the same: the prompt left the composer, so a
            // walk has something true to publish and the run has no wedged session to replace.
            Delivered::Released { .. } => Some(Self::LetGo),
            Delivered::Stopped { .. } => Some(Self::Unasked),
            Delivered::Unwitnessed { .. } => Some(Self::Unproven),
            // ⚠ `Unreported` joins them for the same reason, register item 762: a folded paste the
            // peer never named is not a delivery, so a walk has no evidence to publish for it. What
            // its caller must NOT do is read that shared `None` as *these are one fact* — see
            // `OuterLoop`'s classification, where the two are counted apart.
            Delivered::Unconfirmed { .. }
            | Delivered::Unsubmitted { .. }
            | Delivered::Unreported { .. } => None,
        }
    }

    /// ⛔⛔⛔⛔⛔ **WHETHER THE PANE CAN ANSWER FOR THIS PROMPT AT ALL** — what
    /// [`crate::plugin::Deliveries::folded`] counts, and register item 762's second road joining
    /// the first.
    ///
    /// `true` says: the composer swallowed the paste, the prompt is on no screen, and **sending a
    /// person to look at that pane is the wrong instruction**. That is the whole remedy `folded`
    /// carries, and it is why the two roads share one number: `Account` is the agent naming the
    /// question, [`LetGo`](Self::LetGo) is the composer emptying without anybody naming it, and a
    /// reader acts identically on both.
    ///
    /// # ⚠⚠ EXHAUSTIVE, WITH NO `_` ARM, AND THAT IS THE POINT
    ///
    /// A seventh witness cannot be added without somebody deciding which side of this line it falls
    /// on — the compiler asks. The classification used to live as a `==` against one variant at the
    /// counter's own call site, where a new road would have joined the majority in silence: not
    /// flagged, and therefore reported as *this run's prompts are visible*, which is the
    /// reassuring answer and register item 453's shape.
    #[must_use]
    pub const fn folded_away(self) -> bool {
        match self {
            // The two roads through a composer that ate the paste. Nothing of the prompt is on
            // that screen on either.
            Self::Account | Self::LetGo => true,
            // Every other road leaves the text somewhere a person can find it — painted by the
            // program, echoed by the terminal — or leaves nothing established at all, and neither
            // is a fold.
            Self::Painted | Self::Echoed | Self::Unchecked | Self::Unasked | Self::Unproven => {
                false
            }
        }
    }

    /// **WHAT A READER OF THE RUN SHOULD DO ABOUT IT** — the sentence a walk carries for the
    /// delivery this pass made.
    #[must_use]
    pub const fn noted(self) -> &'static str {
        match self {
            Self::Painted => "the pane painted the prompt back, so it is on that screen",
            Self::Echoed => {
                "the prompt is on that screen and the pane's own echo could have put it there, so \
                 nothing here proves the program has read it"
            }
            Self::Account => {
                "the agent itself named this question as the one it received and the prompt is \
                 NOWHERE ON THAT SCREEN — its composer folded the paste away, so a person sent to \
                 look at the pane for it will find a fold and not the text"
            }
            Self::LetGo => {
                "its composer folded the paste away and then LET GO of it, so the question was \
                 asked — but the agent never named it, so which question is not known here. ⚠ Do \
                 NOT replace this session: the prompt is not sitting in that box, and the peer has \
                 it. What is worth knowing is why the agent's hooks reported no question, which is \
                 the run's own journal and not the pane"
            }
            Self::Unchecked => {
                "nothing was asked of the screen: this peer paints nothing before a submit, so the \
                 prompt went in and the Enter was pressed on trust"
            }
            Self::Unasked => {
                "the run ended between the typing and the Enter, so nothing was asked — and the \
                 composer may still be holding the text"
            }
            Self::Unproven => {
                "the Enter went out and the run ended before anything could see it land, so a \
                 question may well have been asked"
            }
        }
    }
}

/// **WHAT A HOLD AT THE TYPING DOOR FOUND** — [`hold_while_a_child_runs`]'s answer, and register
/// item 745's cause side.
///
/// Four words rather than a `bool`, because the three that permit a delivery do not permit it for
/// the same reason and the one that refuses is a fact a person acts on. A caller that folded them
/// would be unable to say whether it waited, which is the difference between *the peer was free*
/// and *the peer was busy and this run stood there for it*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    /// **NOBODY NAMED A CHILD**, so nothing was held and the caller may type at once.
    ///
    /// ⚠⚠⚠ It is the answer for every pane that has no supervisor, every pane read off its screen
    /// and every daemon older than the key — *nobody said* rather than *nothing is running* — and
    /// that is deliberate: this door must behave exactly as it did before this word existed for the
    /// panes that cannot answer, or the commonest case silently stops being deliverable. The
    /// inversion is the one [`crate::completion::Quiet`] refuses one door over, met here from the
    /// other side: there, an absence must not switch a safety net OFF; here, it must not switch a
    /// delivery off.
    Free,
    /// **A CHILD WAS NAMED AND IT ENDED INSIDE THE BOUND** — `was` is the last tool the agent named
    /// and `after` is how long this door stood there. The caller may type.
    Ended {
        /// The last tool the agent named before the naming stopped.
        was: String,
        /// How long the hold lasted, measured rather than the bound it was permitted.
        after: Duration,
    },
    /// **THE BOUND RAN OUT WITH A CHILD STILL NAMED. NOTHING MAY BE TYPED.**
    ///
    /// ⚠ Not an error of the pane's: an agent inside a long tool call has broken no contract. What
    /// makes it a refusal is what typing anyway would cost — see [`hold_while_a_child_runs`].
    Still {
        /// The tool the agent still names.
        running: String,
        /// The bound this door was given, so a refusal can say how long it stood there.
        within: Duration,
    },
    /// **THE RUN ENDED WHILE HOLDING** — cancelled, or past its own deadline. Nothing was typed and
    /// nothing is claimed about the peer.
    ///
    /// ⚠ A caller falls THROUGH this into its ordinary delivery rather than branching on it, and
    /// that is not laziness: [`deliver`] answers [`Delivered::Stopped`] at its own first look for a
    /// run that has ended, before a byte goes in, so the existing road already says *nothing was
    /// asked* in the words every reader of this crate already knows. A second sentence for the same
    /// ending would be a second authority on it.
    Stopped {
        /// The tool the agent named when the run ended under this door.
        running: String,
    },
}

/// **WHAT THIS PANE'S AGENT SAYS IT IS RUNNING RIGHT NOW**, or [`None`] where nothing said.
///
/// One read, in one place, so the hold and anything else that asks cannot disagree about which
/// surface answers it. See [`crate::access::AgentObservation::running`], which is where the fact's
/// whole argument lives.
fn running_at(panes: &dyn PaneAccess, pane: PaneId) -> Option<String> {
    panes
        .supervision()?
        .pane_agent_state(pane)
        .seen()
        .and_then(|seen| seen.running)
}

/// **HOLD UNTIL THIS PANE'S AGENT IS NOT RUNNING A CHILD** — bounded by `within` and by the RUN's
/// own deadline, and answering [`Held`].
///
/// # ⚠⚠⚠⚠⚠ What a prompt typed at a peer that was not at rest cost, measured
///
/// Register item 745. An unattended run's prompt was found **sitting in a live `claude`'s composer,
/// unsubmitted**, and what that cost is not the prompt: the run replaced its whole session over it,
/// and a run that folds a DIFFERENT prompt each time never trips the *same bytes twice* guard
/// either, so it recovers for ever and calls nobody.
///
/// ⛔ **The product's own prescription for that failure is wrong for this sample.** It says *what is
/// left is the PROMPT — shorten it, or split it*, and the text that was refused was **363 bytes**.
/// There is no size to shorten to. What is left is not to type it yet.
///
/// # ⛔⛔⛔⛔⛔ What that pane's status line said, and why this door does NOT read it
///
/// The one thing that pane said which its four neighbours did not was `1 shell still running`, and
/// the item's remedy followed the correlation: give this door a second information source, the
/// clause the peer paints while it holds a BACKGROUNDED shell. **Driven 2026-08-29, it is false.**
/// Four deliveries into a live `claude` 2.1.251 with exactly one background shell running
/// throughout — 48, 359, 939 and 2,369 bytes, the 939 folded by the composer into
/// `[Pasted text #2]` and submitted by a SEPARATE Enter, which is this module's own shape — every
/// one submitted and was answered. The captures and the table are on
/// `testing::CLAUDE_FOOTER_BY_BACKGROUND_SHELLS` — named rather than linked, because that module
/// is this crate's own and a public item may not link one. The gate that holds the refutation is
/// named in the module docs above.
///
/// ⚠⚠ So the door's blind spot below is not a hole to be closed: a door that read that clause would
/// stand for a whole bound and then stop a run for a person over a peer that was never busy. **What
/// actually refused that one prompt is not known**, and this door does not claim to be its remedy.
///
/// # ⚠⚠⚠⚠ WHAT THIS DOOR CAN SEE, AND WHAT IT CANNOT — measured 2026-08-29, stated rather than hidden
///
/// The fact consulted is the AGENT'S OWN WORD: the tool named by the event that opens a tool call,
/// which register item 721 carried from the hook to a waiter. **It is retired by the very next
/// report of any kind**, and `Stop` — the turn's own rest — is one of those
/// (`sprag_host::hooks::CLAUDE`'s table, `("Stop", Report(Idle))`). So a child this door holds for
/// is **a tool call in flight**, and a shell the agent BACKGROUNDED outlives its tool call's end and
/// is invisible here — which the section above measured to be the right thing to be blind to.
///
/// ⚠⚠ The other place it could have looked is worse rather than merely different, and that was
/// measured rather than argued: `sprag processes <pane>` lists the pane's foreground process group,
/// which for a `claude` pane is the agent and its MCP server and never the shells it starts — the
/// two backgrounded `/bin/bash -c` children of a live agent were in `ps --ppid` and **not** in that
/// listing, while `sprag-mcp` sits in it for ever, so a *has a child* predicate read from there
/// would hold permanently and this door would never let a prompt through at all.
///
/// # ⚠⚠⚠ Why it WAITS instead of refusing at once, and why it is bounded anyway
///
/// A tool call ends. Refusing on sight would turn every ordinary build into a refused delivery,
/// which is the *loop that sends nothing* this door must not become — so the answer to a busy peer
/// is to stand there, and the fixtures that hold this door are a pair for exactly that reason.
///
/// It is bounded because the alternative is a run that waits out a child nothing will ever end. The
/// number is the caller's, and the caller this was built for hands it the DOCUMENT's own
/// `turn_within_ms` — *how long one of this agent's turns may take* — which is the same quantity
/// register item 721's silence bound ends a frozen tool call on, from the other side of the same
/// fact.
///
/// # ⚠⚠ It POLLS, and the reason is that this fact has no due time
///
/// [`crate::run::park_until`] is cheaper wherever a predicate rests on the pane's bytes or on a
/// deadline somebody published (register items 629 and 630). Neither is true here: a tool ends when
/// it ends, the event that says so is a REPORT rather than a paint, and nothing anywhere knows when
/// it is due. So this is [`crate::run::poll_until`] by name and not by degradation — a look every
/// [`POLL_INTERVAL`] at a supervision read, for as long as the child runs.
pub fn hold_while_a_child_runs(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    within: Duration,
) -> Held {
    // ⚠ ASKED ONCE BEFORE ANY WAIT IS SET UP. The overwhelmingly common answer is *nobody named
    // anything*, and for that answer this door must cost one supervision read and no clock at all.
    let Some(mut running) = running_at(panes, pane) else {
        return Held::Free;
    };
    let began = std::time::Instant::now();
    // ⚠⚠ THE NAME IS CARRIED FORWARD ON EVERY LOOK, not read again at the end. A refusal has to say
    // WHICH tool held it, and by the time the bound has run out the report in force may name a
    // different one — a second read would be a sentence about a later look than the one that
    // decided.
    let waited = crate::run::poll_until(run, within, || match running_at(panes, pane) {
        Some(tool) => {
            running = tool;
            false
        }
        None => true,
    });
    match waited {
        crate::run::Waited::Ready => Held::Ended {
            was: running,
            after: began.elapsed(),
        },
        crate::run::Waited::TimedOut => Held::Still { running, within },
        crate::run::Waited::Stopped => Held::Stopped { running },
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
            OnScreen::Stopped => {
                return Ok(Delivered::Stopped {
                    attempts,
                    written: Written::of(written),
                });
            }
            OnScreen::Shown => {
                // Only now: the text is on a screen THIS DELIVERY CHANGED, so a submit submits the
                // text rather than an empty line — and, measured live, it is a keystroke of its own
                // rather than a byte appended to the same unread pty read as the prompt. Sent for
                // BOTH on-screen answers — see `Delivered::OnScreenOnly`.
                if !spec.then_press.is_empty() {
                    // ⚠ NO SECOND WITNESS ON THIS ROAD — register item 762's is the fold's alone.
                    // The text is ON the screen here, so the caller's own contract is answerable by
                    // construction; arming a composer read beside it would buy a channel nothing
                    // needs and pay a supervisor read for every ordinary delivery there is.
                    match submit(panes, run, pane, text, spec, &mut written, None)? {
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
                        // ⚠⚠ UNREACHABLE BY CONSTRUCTION, AND ANSWERED RATHER THAN IGNORED —
                        // register item 762. `Seen::LetGo` is the second witness speaking, and this
                        // road arms none (`also` is `None` four lines up). It is spelled out so
                        // that a future caller which DOES arm one here has to decide what this road
                        // means by it, rather than inheriting `Yes`'s answer by a wildcard.
                        Seen::Yes | Seen::LetGo => {}
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
            // ⚠⚠⚠⚠⚠ **THE PANE TOOK THE BYTES AND IS SHOWING SOMETHING ELSE FOR THEM** — register
            // item 421, and the one road where a screen stops being able to refuse a delivery.
            //
            // A composer that FOLDS a long paste is displaying a placeholder where the text should
            // be, so no needle can be found and no retry can help: a second injection lands on the
            // first one's text. Where the caller's peer can NAME the question it received, the
            // submit goes in and that account is the verdict — which is the evidence a screen was
            // always standing in for, taken from the only party that read the bytes.
            //
            // ⚠⚠ **AND ONLY THERE.** For every other contract this is `Nothing`'s road, unchanged:
            // an agent with no hooks reports no question ever, so a press over a screen that never
            // showed the text would be exactly the blind submit this module exists to prevent, with
            // nothing able to tell afterwards whether it asked the prompt or an empty line.
            //
            // ⚠⚠ **AND ONLY WHERE THE SCREEN MOVED, WHICH LEAVES A RESIDUE — STATED, UNMEASURED.** A
            // peer that takes the text and paints NOTHING AT ALL is `Nothing`'s road too, and for it
            // this press would have been right: its own account would have settled the delivery. That
            // is the raw-mode reader the submit contract's docs name, and no agent CLI has been
            // observed behaving that way — they all paint a composer. The trade is deliberate: a
            // screen that has not moved is a screen on which nothing is known to have taken the
            // bytes, and the retry serves that case at no risk of asking an empty question.
            OnScreen::MovedWithoutIt
                if !spec.then_press.is_empty()
                    && matches!(spec.submitted_when, SubmittedWhen::Took { .. }) =>
            {
                // ⛔⛔⛔⛔⛔ **AND THE COMPOSER IS ARMED BESIDE THE ACCOUNT** — register item 762,
                // and this line is what turns one channel into two on the ONE road that had only
                // one. `Took` expires: when the account does not come there is nothing to tell
                // *not yet* from *never*, so the answer is a timeout wearing the clothes of a fact.
                // `Released` CONVERGES — a composer is holding or it is not, and both readings are
                // stable — so the same wait now also learns *the prompt left the box*.
                //
                // ⚠⚠ THE SAME WINDOW, because it is the same wait: a second duration here would be
                // a bound nobody authored, and the caller sized this one for its peer.
                //
                // ⚠ The account still WINS where both could speak (`Submission::landed`), because
                // it names the TEXT and this names only that a question was asked.
                let also = spec
                    .submitted_when
                    .within()
                    .map(|within| SubmittedWhen::Released { within });
                let landed = submit(panes, run, pane, text, spec, &mut written, also)?;
                return Ok(match landed {
                    Seen::Yes => Delivered::Reported {
                        attempts,
                        written: Written::of(written),
                    },
                    // ⛔⛔⛔⛔⛔ **THE COMPOSER LET GO AND NOBODY NAMED IT** — register item 762's
                    // second half. The prompt is no longer in that box, so a question WAS asked;
                    // which one is not known, and this word is careful not to claim it.
                    Seen::LetGo => Delivered::Released {
                        attempts,
                        written: Written::of(written),
                    },
                    // The screen never showed it and the agent never named it, so nothing places
                    // this prompt in that pane — and the keystroke IS out, which is what makes this
                    // the submit's refusal rather than the text's.
                    //
                    // ⛔⛔⛔⛔⛔ IT WAS `Unsubmitted`, WHICH SAYS THE OPPOSITE — register item 762.
                    // That answer's sentence is *the prompt is sitting in the pane*, and the whole
                    // meaning of the arm we are inside is that the screen moved WITHOUT the text.
                    // See `Delivered::Unreported`, which is this road's own word.
                    Seen::No => Delivered::Unreported {
                        attempts,
                        written: Written::of(written),
                        wanted: spec.submitted_when,
                    },
                    Seen::Stopped => Delivered::Unwitnessed {
                        attempts,
                        written: Written::of(written),
                        wanted: spec.submitted_when,
                    },
                });
            }
            OnScreen::MovedWithoutIt | OnScreen::Nothing => {}
        }
    }
    Ok(Delivered::Unconfirmed {
        attempts,
        written: Written::of(written),
    })
}

/// Send [`Delivery::then_press`] and wait for whatever the caller asked as evidence that it landed,
/// charging its bytes to `written`.
///
/// # ⚠⚠⚠ One function because the ORDERING is the same on both roads to a submit
///
/// A delivery reaches its keystroke two ways now — the read-back that SAW the text, and (for a peer
/// that names what it was asked) a screen that moved without showing it — and the guarantee they
/// share is what a second copy of this block would be free to lose: **the witness is armed BEFORE the
/// injection.** Armed after, the change it looks for is one it may already have missed, and a peer
/// quick enough to answer at once would be reported as having ignored the submit. It is the same
/// discipline as [`deliver`]'s own `before` baseline at the text's end and
/// [`Completion::begin`](crate::completion::Completion::begin) at the turn's.
///
/// ⚠ It presses ONCE and has no retry of its own — see the module docs' hazard. A second Enter onto a
/// composer the first one emptied asks an EMPTY question, which an agent answers.
///
/// # Errors
///
/// [`PaneError`] from the injection itself — an unknown pane, or a key with no bytes.
fn submit(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    text: &str,
    spec: &Delivery,
    written: &mut u64,
    also: Option<SubmittedWhen>,
) -> Result<Seen, PaneError> {
    let witness = Submission::arm(panes, pane, spec.submitted_when, also, text);
    *written += panes.inject(pane, &spec.then_press)?.bytes();
    Ok(witness.await_landing(panes, run, pane))
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
///
/// # ⚠⚠⚠⚠ It asks the SURFACE, and that is register item 555's whole repayment
///
/// This read the rows' damage generation itself. A host whose rows carry no generations — which is
/// what a host on the far side of a socket must serve, because a damage number is a paint signal
/// that a resize stamps while no program writes a byte — therefore answered `false` here for every
/// pane there is, including one whose child had printed and exited. The question now travels as its
/// own fact ([`PaneAccess::pane_has_painted`]), whose default is still exactly this rows read, so a
/// host with only rows answers as it always did and one that knows better can say so.
///
/// ⚠⚠ **`None` COLLAPSES TO `false`, which is safe in one direction only and is why the surface
/// keeps the third answer.** *No such pane* and *this build cannot say* both arrive here as *not
/// ready yet*, and a caller that waits is doing the harmless thing — but a caller that needs to
/// tell *nobody has painted* from *nobody could look* must ask
/// [`PaneAccess::pane_has_painted`] itself rather than read a `bool` that has already lost the
/// distinction.
#[must_use]
pub fn has_painted(panes: &dyn PaneAccess, pane: PaneId) -> bool {
    panes.pane_has_painted(pane).unwrap_or(false)
}

/// What a bounded wait for a pane's own evidence saw — the submit's ([`Submission::await_landing`]).
enum Seen {
    Yes,
    /// ⛔⛔⛔⛔⛔ **THE SECOND WITNESS SETTLED IT: the composer let go of the prompt, and the agent
    /// never named it** — register item 762, and only ever answered on the FOLD road.
    ///
    /// It is a separate word from [`Yes`](Self::Yes) because the two know different things. `Yes`
    /// is the agent's own account and names the TEXT; this is a property of the pane and names
    /// nothing — *a question was asked* without *which one*. Collapsing them would let the weaker
    /// evidence be reported as the stronger, which is the whole disease register item 421 and its
    /// neighbours keep paying for.
    LetGo,
    No,
    /// The run ended under it — cancelled, or past its deadline.
    Stopped,
}

/// What ONE poll of a submit's witnesses saw — [`Submission::landed`]'s answer.
///
/// # ⚠⚠⚠ Why *never* is its own arm rather than an absent yes
///
/// A contract nothing can answer must end the wait AT ONCE rather than spend the window
/// discovering it — `Released` armed against a pane that was not holding, `Took` against a host
/// with no supervisor. With two witnesses that judgement stops being a property of one contract:
/// the wait may only be abandoned when **every armed witness** can never speak, which is what
/// `Never` means here and what a bare `Option<bool>` could not express.
enum Poll {
    /// One of them spoke, and this is which — [`Seen::Yes`] for the primary contract,
    /// [`Seen::LetGo`] for the composer.
    Landed(Seen),
    /// Nobody has spoken yet, and at least one of them still could.
    NotYet,
    /// No armed witness can ever speak. Waiting buys nothing.
    Never,
}

/// What a bounded wait for the delivered TEXT saw — [`Seen`]'s three answers plus the ONE
/// ATTRIBUTION the submit's wait has no use for.
///
/// # ⚠⚠⚠⚠ Why *not there* had to become two answers — register item 421
///
/// `Seen::No` was two situations wearing one word, and they call for opposite acts:
///
/// * **the screen never moved** — nothing took the bytes, which is the swallowed-input window this
///   module was written for, and the answer is to inject again;
/// * **the screen moved and the text is not on it** — something took the bytes and will not show
///   them. An agent's composer FOLDS a long paste (`[Pasted text #2 +5 lines]`, measured on `claude`
///   2.1.233), so the prompt is in the pane and no choice of needle can find it. Injecting again
///   puts a SECOND COPY into that composer, which is how three live runs came to hold 4,002 bytes of
///   the same prompt and then be refused.
///
/// ⚠⚠⚠ **THE DISCIPLINE IS THE REGISTER'S OWN** (item 422): where a check waits on driven output,
/// attribute *unpainted* against *not yet* rather than only failing. One word for both is what made
/// a folded paste indistinguishable from a peer that read nothing.
enum OnScreen {
    /// The needle is on a screen this delivery changed.
    Shown,
    /// The screen is no longer the one this delivery began on, and the needle is not on it.
    ///
    /// ⚠ Only ever answered where the baseline was READABLE: *it moved* is a comparison, and a
    /// delivery that could not read the pane before it started has nothing to compare against —
    /// that case is [`Nothing`](Self::Nothing), which is the answer that claims least.
    MovedWithoutIt,
    /// Nothing happened: the screen is exactly the one this delivery began on, or the pane could not
    /// be read at all.
    Nothing,
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
/// `text` with every run of whitespace flattened to one space, and the ends trimmed.
///
/// **The one place a needle and a screen are made comparable** — see the hazard at its only call
/// site. A peer that re-wraps what it was typed produces the same words with different breaks, and
/// a comparison that reads a newline as content asks the peer not to have a text box.
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn await_text(
    panes: &dyn PaneAccess,
    run: &RunContext,
    pane: PaneId,
    needle: &str,
    timeout: Duration,
    before: Option<&str>,
) -> OnScreen {
    let start = std::time::Instant::now();
    // ⚠⚠⚠ RAISED INSIDE THE WAIT, NOT ASKED AFTER IT. *Did this delivery move the screen* is a
    // comparison against a moment, so it has to be taken while the poll that observed the change is
    // the poll doing the comparing — a second read taken once the grace expired would be a different
    // moment, and a peer that repainted its box back is a peer that moved.
    let mut moved = false;
    loop {
        if run.stopped() {
            return OnScreen::Stopped;
        }
        // An unknown pane can never show anything, and saying so at once beats spending the whole
        // grace on it — the caller's next `inject` will report `UnknownPane` properly.
        match panes.pane_collapsed(pane) {
            // ⚠⚠⚠⚠ THE MATCH IS WHITESPACE-INSENSITIVE AND THE CHANGE TEST IS NOT — item 421, and
            // the two halves are deliberately different questions. *Did this delivery move the
            // screen* is about the screen exactly as it is; *is my text on it* must survive the
            // peer RE-FLOWING it, because an agent's prompt box re-wraps what it was given onto
            // lines of its own choosing and indents them. Those are logical lines the CHILD wrote,
            // so `pane_collapsed` cannot rejoin them — it undoes the TERMINAL's wrapping, which is
            // a different thing and the only thing it can know about.
            //
            // ⚠⚠⚠ MEASURED, run 11: the pane was holding the prompt three times over and the tail
            // was plainly on screen as `…make the last line of your\n  reply exactly: MILESTONE
            // REACHED`. No contiguous run of the delivered text existed anywhere on that screen, so
            // NO choice of needle — head, tail or middle — could have matched it.
            //
            // ⚠ What this gives up, stated: two texts differing only in whitespace now confirm each
            // other. That is the same class as the substring match item 223 records, and narrower
            // than it — nothing on a screen distinguishes a space from a wrap, so demanding one
            // would be demanding evidence a terminal does not carry.
            Some(text)
                if squeezed(&text).contains(&squeezed(needle)) && Some(text.as_str()) != before =>
            {
                return OnScreen::Shown;
            }
            // An unknown pane can never show anything, and it can never be said to have MOVED
            // either — there is no screen to compare.
            None => return OnScreen::Nothing,
            // ⚠⚠⚠⚠ THE ATTRIBUTION, and it is taken on a screen that does NOT carry the needle: the
            // pane is showing something it was not showing before this delivery began, so the bytes
            // reached a program that is displaying something else for them. `before` of `None` is a
            // baseline that could not be read, and a change is a claim that needs one.
            Some(text) => {
                moved |= before.is_some_and(|was| was != text);
            }
        }
        if start.elapsed() >= timeout {
            return if moved {
                OnScreen::MovedWithoutIt
            } else {
                OnScreen::Nothing
            };
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
    /// ⛔⛔⛔⛔⛔ **A SECOND WITNESS, ARMED FROM THE SAME OBSERVATION** — register item 762, and
    /// [`None`] everywhere but the fold road.
    ///
    /// # ⛔⛔⛔⛔ What one witness cost
    ///
    /// A composer that FOLDS a paste leaves the delivery with the agent's own account and nothing
    /// else ([`SubmittedWhen::Took`]), and that channel EXPIRES: when the account does not come
    /// there is nothing to tell *not yet* from *never*, so the answer is a timeout wearing the
    /// clothes of a fact. Register item 669 measured it as four of five live runs holding prompts
    /// that were never asked, unable to say so, and register item 762 watched a run die of it —
    /// 187 iterations in, with the pane it had been driving reading `working seq=26 said=12`.
    ///
    /// [`SubmittedWhen::Released`] is the contract that CONVERGES, because a composer is holding or
    /// it is not and both readings are stable. Arming it beside the account gives the fold road two
    /// channels where it had one, and the stronger one still wins — see [`landed`](Self::landed).
    ///
    /// ⚠⚠ **ARMED OUT OF THE SAME `pane_agent_state` READ, WHICH IS WHY IT IS A FIELD HERE AND NOT
    /// A SECOND `Submission`.** Two arms would take two observations, and a change landing between
    /// them would date the pair to different moments — the drift this whole type exists to prevent,
    /// written out one field down.
    also: Option<SubmittedWhen>,
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
    /// WHAT THIS DELIVERY IS ASKING, for [`SubmittedWhen::Took`] to compare the agent's own account
    /// against.
    ///
    /// ⚠ The delivery's TEXT, not its needle: the needle is a fragment chosen to be findable on a
    /// screen, and the agent reports the whole question. Comparing against a fragment would accept
    /// a peer that was asked something this text is only part of, which is the dirty-composer case
    /// this contract exists to catch.
    asked: Option<String>,
    /// WHOSE composer was holding an unsubmitted prompt as the submit went in —
    /// [`SubmittedWhen::Released`]'s baseline.
    ///
    /// `None` where the pane was NOT holding, as well as where nothing could be read at all, and
    /// the two are deliberately the same answer: neither can produce evidence that this keystroke
    /// released anything. ⚠ This is the one baseline whose absence matters most, because the
    /// contract is satisfied by an ABSENCE — armed on a pane that was not holding, *it is not
    /// holding now* would be true before the submit was ever pressed.
    holding: Option<String>,
}

impl Submission {
    /// Read what these contracts will be compared against — **called before the submit is
    /// injected**.
    ///
    /// ⚠⚠ `also` is a SECOND witness armed from the same reading; see the field. Everything below
    /// asks *does EITHER contract need this baseline*, because a baseline the second one needs and
    /// the first does not is exactly the fold road.
    fn arm(
        panes: &dyn PaneAccess,
        pane: PaneId,
        wanted: SubmittedWhen,
        also: Option<SubmittedWhen>,
        text: &str,
    ) -> Self {
        /// Whether either armed contract is one of the kinds `needs` names.
        fn either(
            wanted: SubmittedWhen,
            also: Option<SubmittedWhen>,
            needs: fn(SubmittedWhen) -> bool,
        ) -> bool {
            needs(wanted) || also.is_some_and(needs)
        }
        // Nothing is asked, so nothing is read. A baseline taken for an unchecked submit would be
        // a pane read every delivery pays for and nothing consults.
        let screen = either(wanted, also, |kind| {
            matches!(kind, SubmittedWhen::Repaints { .. })
        })
        .then(|| panes.pane_collapsed(pane))
        .flatten();
        // ⚠⚠ ONE READING for every kind that asks about the agent, because `Released` draws TWO
        // baselines out of it — who the peer is, and whether its composer was holding. Two reads
        // could straddle a change and arm the pair against different moments, which is the drift
        // this whole type exists to prevent. ⚠⚠⚠ AND IT IS ONE READING ACROSS BOTH WITNESSES too
        // (register item 762): the fold road arms `Took` and `Released` together, and a second
        // `pane_agent_state` for the second contract would date them to different moments.
        let seen = either(wanted, also, |kind| {
            matches!(
                kind,
                SubmittedWhen::Stirs { .. }
                    | SubmittedWhen::Released { .. }
                    | SubmittedWhen::Took { .. }
            )
        })
        .then(|| {
            panes
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(pane).seen())
        })
        .flatten();
        Self {
            wanted,
            also,
            screen,
            // Only the contract that compares it keeps it, for `asked`'s reason below: a pane that
            // was NOT holding arms nothing, so the contract can never be satisfied and says so.
            // ⛔⛔⛔⛔⛔ READ OFF THE FACT AND NOT OFF THE STATE — register item 762. It was
            // `seen.state == AgentState::Holding`, and that state is ARBITRATED: a hook outranks
            // the screen, so `Holding` is never the state of a pane whose agent reports — which is
            // every pane a supervisor drives. This contract therefore refused on exactly the
            // population it was built for. `AgentObservation::holding` is the same screen reading
            // in a slot the arbitration does not touch.
            holding: either(wanted, also, |kind| {
                matches!(kind, SubmittedWhen::Released { .. })
            })
            .then(|| {
                seen.as_ref()
                    .filter(|seen| seen.holding == Some(true))
                    .and_then(|seen| seen.agent.clone())
            })
            .flatten(),
            agent: seen.and_then(|seen| seen.agent.map(|agent| (agent, seen.seq))),
            // Only the contract that compares it keeps it: a delivery that asks nothing of the
            // agent's account has no business holding a copy of its own prompt.
            asked: either(wanted, also, |kind| {
                matches!(kind, SubmittedWhen::Took { .. })
            })
            .then(|| text.to_owned()),
        }
    }

    /// ⛔⛔⛔⛔⛔ **WHAT ONE POLL OF EVERY ARMED WITNESS SAW** — register item 762, and the
    /// combination rule is the whole of it.
    ///
    /// * **The primary contract is asked FIRST and wins**, because it is the stronger claim. On the
    ///   fold road that is the agent's own account, which names the TEXT; the composer names only
    ///   that a question was asked. Letting the weaker one answer where the stronger could have
    ///   would report less than was known.
    /// * **The wait is abandoned only when EVERY armed witness can never speak.** With one contract
    ///   that judgement was the contract's own; with two it is not, and a `Never` taken off the
    ///   primary alone would throw away the channel this item exists to add — a host with no
    ///   supervisor refuses `Took` at once, and on the fold road the composer would never be asked.
    ///
    /// # ⛔⛔⛔⛔⛔ The second half of that rule is UNREACHABLE TODAY, and saying so is the honest
    /// report
    ///
    /// Mutating it away — `Never` off the primary alone — leaves every gate in this file GREEN.
    /// Measured, not assumed. The reason is a COUPLING in [`arm`](Self::arm): both baselines hang
    /// off one `pane_agent_state` read and both need the observation to NAME AN AGENT, so a
    /// `Submission` that can never satisfy `Took` can never satisfy `Released` either.
    /// `a_composer_baseline_is_never_armed_without_an_agent_baseline` pins that implication, which
    /// is what makes the simpler code equivalent rather than merely untested.
    ///
    /// ⚠⚠ **IT IS KEPT ANYWAY, AND DELIBERATELY.** The coupling is a property of how the baselines
    /// are read today, not of what these witnesses mean; the day a contract is armed off something
    /// other than an agent's name, the primary-only rule silently stops waiting on a witness that
    /// could still speak. A rule that is right for the reason it is written costs nothing here, and
    /// the gate above is what will notice when its premise moves.
    fn landed(&self, panes: &dyn PaneAccess, pane: PaneId) -> Poll {
        let primary = self.judged(self.wanted, panes, pane);
        if primary == Some(true) {
            return Poll::Landed(Seen::Yes);
        }
        let second = self.also.map(|kind| self.judged(kind, panes, pane));
        if second == Some(Some(true)) {
            return Poll::Landed(Seen::LetGo);
        }
        // ⚠ `None` at either level is *this one can never speak*: the outer for a witness that was
        // never armed, the inner for one that was and has nothing to compare against.
        if primary.is_none() && !matches!(second, Some(Some(_))) {
            return Poll::Never;
        }
        Poll::NotYet
    }

    /// Whether ONE contract's evidence is here YET — `None` where it can never come.
    ///
    /// Three answers for [`await_text`]'s reason one door up: *not yet* and *never* end the wait
    /// differently, and spending a whole grace on a question nothing can answer is a delay with no
    /// information in it.
    ///
    /// ⚠ It takes the kind as an ARGUMENT rather than reading `self.wanted`, because register item
    /// 762 arms two of them and both are judged against the one set of baselines above.
    fn judged(&self, wanted: SubmittedWhen, panes: &dyn PaneAccess, pane: PaneId) -> Option<bool> {
        match wanted {
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
                        .and_then(|supervisor| supervisor.pane_agent_state(pane).seen())
                        .is_some_and(|seen| {
                            // ⚠⚠ BOTH, and the name is what makes this a claim about the peer the
                            // submit went to rather than about whatever is in the pane now.
                            seen.seq > *pressed_at
                                && seen.agent.as_deref() == Some(addressed.as_str())
                        }),
                )
            }
            // ⚠⚠⚠ THE ABSENCE IS THE ANSWER, which is why the baseline is not optional here in the
            // way the others' are: `None` means the pane was not holding when the submit went in,
            // and then *it is not holding now* is a sentence that was already true. Refusing at
            // once is both the honest answer and the cheap one.
            SubmittedWhen::Released { .. } => {
                let addressed = self.holding.as_deref()?;
                Some(
                    panes
                        .supervision()
                        .and_then(|supervisor| supervisor.pane_agent_state(pane).seen())
                        .is_some_and(|seen| {
                            // ⚠ NO `seq` COMPARISON, deliberately — see the kind's own doc. What is
                            // asked is a PROPERTY of the pane now, and requiring a published change
                            // as well would reinstate the event dependency this contract drops.
                            // ⚠⚠ `== Some(false)` AND NOT `!= Some(true)` — register item 762. The
                            // third answer is *nothing could say*, and reading it as *it let go*
                            // would satisfy this contract off a daemon that had simply stopped
                            // answering, which is the inversion every absence in this crate is
                            // written to avoid.
                            seen.agent.as_deref() == Some(addressed) && seen.holding == Some(false)
                        }),
                )
            }
            SubmittedWhen::Took { .. } => {
                let (addressed, pressed_at) = self.agent.as_ref()?;
                let asked = self.asked.as_deref()?;
                Some(
                    panes
                        .supervision()
                        .and_then(|supervisor| supervisor.pane_agent_state(pane).seen())
                        .is_some_and(|seen| {
                            // ⚠⚠⚠ THREE THINGS, and dropping any one of them admits a false yes:
                            // the peer is the one addressed, its state has MOVED since the press
                            // (so an identical earlier turn's report cannot stand in), and the
                            // question it names is THIS one rather than one this text is merely
                            // part of.
                            seen.seq > *pressed_at
                                && seen.agent.as_deref() == Some(addressed.as_str())
                                && seen
                                    .asked
                                    .as_deref()
                                    .is_some_and(|said| squeezed(said) == squeezed(asked))
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
                Poll::Landed(seen) => return seen,
                Poll::Never => return Seen::No,
                Poll::NotYet => {}
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
    // ⚠ THE FIXTURES' VOCABULARY, and no longer the module's — register item 762. The delivery path
    // stopped reading the arbitrated state when the composer reading got a slot of its own, so the
    // only code left that spells a state is the doubles that stand in for a supervisor.
    use crate::access::{PaneRow, PaneTerminalModes, WorkspacePaneAccess};
    use sprag_detect::AgentState;
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
        access_sized(script, 40, 6)
    }

    /// [`access`] at a stated size, for a fixture whose evidence is a LINE rather than a word.
    ///
    /// ⚠ A pane 40 columns wide wraps anything longer, and [`PaneAccess::pane_collapsed`] joins the
    /// rows with nothing between them — which recovers a wrapped SENTENCE but not the blanks a row
    /// may have been trimmed of at the fold. A gate whose premise is *this pane is painting exactly
    /// this status line* therefore has to be given a pane the line fits on, or it would be asserting
    /// the emulator's wrapping rather than the peer's screen.
    fn access_sized(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .expect("the workspace")
            .spawn(command, "peer".to_string(), cols, rows)
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
    /// publishes [`sprag_detect::AgentState::Working`] once the peer has printed [`WORKING`].
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
                holding: None,
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
                asked_seq: last.1,
                reports: 0,
                asking: None,
                asked: None,
                said: None,
                said_seq: 0,
                noticed: None,
                running: None,
                transcript: None,
                settling: crate::access::Settling::Nothing,
                reporter: crate::access::ReporterVoice::Speaking,
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

    /// **A HOST WITH ONLY ROWS STILL ANSWERS AS IT ALWAYS DID** — register item 555's degradation
    /// clause, which the gate above cannot make because it drives the real
    /// [`WorkspacePaneAccess`](crate::access::WorkspacePaneAccess) and that one OVERRIDES the
    /// default.
    ///
    /// # ⚠⚠⚠⚠ Why this needs its own fixture rather than one more assertion up there
    ///
    /// Register item 684's rule: a fixture whose two sources agree cannot say which was consulted.
    /// The real host answers from `Screen::has_painted` and its rows carry the same generations, so
    /// every claim over it is true under both readings and measures neither. What is unmeasured
    /// there is precisely the arm a remote surface and every test double in this workspace take —
    /// **the trait's default** — and the item's own done-when names keeping it as a requirement,
    /// not as an accident.
    #[test]
    fn a_host_that_serves_only_rows_answers_the_paint_question_from_them() {
        /// Rows and nothing else: no screen, no override — the shape every double here inherits.
        struct RowsOnly(Vec<PaneRow>);

        impl PaneAccess for RowsOnly {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                None
            }
            fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
                (id == PaneId(1)).then(|| self.0.clone())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                None
            }
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
                Ok(Written::of(0))
            }
        }

        let row = |generation: u64| PaneRow {
            generation,
            text: "same text either way".to_owned(),
        };

        // ⚠ THE TEXT IS IDENTICAL IN BOTH FIXTURES AND ONLY THE GENERATION MOVES, so nothing but
        // the number under test can decide these two apart.
        let painted = RowsOnly(vec![row(0), row(7)]);
        let never = RowsOnly(vec![row(0), row(0)]);

        assert!(
            has_painted(&painted, PaneId(1)),
            "a rows-only host whose rows carry a damage generation must still answer `true` — this \
             is the reading every caller had before the question got an address of its own, and \
             the default exists to keep it",
        );
        assert!(
            !has_painted(&never, PaneId(1)),
            "and a host whose rows have never been stamped answers `false`, which is the half that \
             would pass under an always-true default",
        );
        assert!(
            !has_painted(&painted, PaneId(9999)),
            "⚠⚠ and a pane this host does not serve is NOT painted rather than a panic or a `true`: \
             `pane_has_painted` answers `None` there and `has_painted` collapses it in the safe \
             direction, which is the one behaviour its own doc promises a caller",
        );
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

        /// One delivery against this double under [`SubmittedWhen::Took`], with the agent reporting
        /// `said` as the question it received once the submit has gone in.
        ///
        /// ⚠⚠ THE REPORT IS TIED TO THE SUBMIT, which is the product's own timing: an agent's hook
        /// fires when a prompt is SUBMITTED, so a contract that could be satisfied before the
        /// keystroke would be satisfied by the previous turn's report. The `seq` moves with it for
        /// the same reason.
        fn deliver_asking(self, text: &str, said: Option<&str>) -> Delivered {
            self.reporting(said).deliver_under(text, &asking_once())
        }

        /// This double, wrapped in a supervisor whose agent names `said` as the question it
        /// received once the submit has gone in.
        ///
        /// ⚠ Separate from [`deliver_asking`](Self::deliver_asking) so a gate can vary the SPEC and
        /// read the injection log afterwards — the two facts register item 421 is about are *how
        /// many copies of the prompt went in* and *whether the Enter went at all*, and a helper
        /// that returns only the verdict can state neither.
        fn reporting(self, said: Option<&str>) -> Reporting {
            Reporting {
                inner: self,
                said: said.map(str::to_owned),
                // ⚠ NOTHING CAN SAY, which is what every gate predating register item 762 means by
                // saying nothing about a composer — and it keeps their answers exactly as measured.
                composer: (None, None),
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

    /// A [`Recorder`] whose AGENT names the question it received, once the submit has gone in.
    ///
    /// ⚠⚠ THE REPORT IS TIED TO THE SUBMIT, which is the product's own timing: an agent's hook fires
    /// when a prompt is SUBMITTED, so a contract that could be satisfied before the keystroke would
    /// be satisfied by the previous turn's report. The `seq` moves with it for the same reason.
    struct Reporting {
        inner: Recorder,
        /// What the agent says it was asked, or `None` for a peer with no hooks — which reports
        /// nothing, ever, and is the population the screen predicate stays for.
        said: Option<String>,
        /// ⛔⛔⛔⛔⛔ **WHAT THIS PANE'S COMPOSER SAYS BEFORE AND AFTER THE SUBMIT** — register item
        /// 762's second witness, `(before, after)`.
        ///
        /// `(None, None)` is a supervisor that cannot say, which is what every gate written before
        /// that item stages and is why they keep their old answers: with no composer reading there
        /// is no second channel, and the fold road is back to the account alone.
        ///
        /// ⚠⚠ The pair is separate from [`said`](Self::said) ON PURPOSE, because the whole claim is
        /// that the two channels are independent: a peer whose hooks are silent can still have a
        /// composer that empties, and that is the run this item was filed for.
        composer: (Option<bool>, Option<bool>),
    }

    impl Reporting {
        /// One delivery against this double under `spec`.
        fn deliver_under(&self, text: &str, spec: &Delivery) -> Delivered {
            deliver(self, &RunContext::uncancellable(), PaneId(1), text, spec).expect("no error")
        }

        /// Every injection this double was handed, in order — one entry per `inject` call, each the
        /// key names it carried.
        fn log(&self) -> Vec<Vec<String>> {
            self.inner.injected.lock().expect("the log").clone()
        }

        /// How many injections carried TEXT rather than the submit.
        fn text_injections(&self) -> usize {
            self.log()
                .iter()
                .filter(|keys| keys != &&vec!["Enter".to_owned()])
                .count()
        }

        /// How many submits went out. ⚠ The count and not a `bool`: *never pressed* and *pressed
        /// twice* are opposite defects and a boolean answers the same for one and for two.
        fn submits(&self) -> usize {
            self.log()
                .iter()
                .filter(|keys| keys == &&vec!["Enter".to_owned()])
                .count()
        }
    }

    impl PaneAccess for Reporting {
        fn pane_ids(&self) -> Vec<PaneId> {
            self.inner.pane_ids()
        }
        fn pane_collapsed(&self, id: PaneId) -> Option<String> {
            self.inner.pane_collapsed(id)
        }
        fn pane_rows(&self, id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
            self.inner.pane_rows(id)
        }
        fn pane_eof(&self, id: PaneId) -> Option<bool> {
            self.inner.pane_eof(id)
        }
        fn pane_full_text(&self, id: PaneId) -> Option<String> {
            self.inner.pane_full_text(id)
        }
        fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
            self.inner.inject(id, keys)
        }
        fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
            Some(self)
        }
        fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
            Some(&self.inner)
        }
    }

    impl crate::access::PaneSupervision for Reporting {
        fn pane_agent_state(&self, _id: PaneId) -> crate::access::Supervised {
            let submitted = self.inner.submitted();
            crate::access::Supervised::Seen(Box::new(crate::access::AgentObservation {
                state: sprag_detect::AgentState::Working,
                // ⚠ THE COMPOSER, BEFORE AND AFTER THE KEYSTROKE — register item 762. The state
                // beside it stays `Working` on both, which is the product's shape: this pane's
                // agent REPORTS, so a manifest rule never decides its state.
                holding: if submitted {
                    self.composer.1
                } else {
                    self.composer.0
                },
                agent: Some("claude".to_owned()),
                authority: crate::access::Authority::Reported {
                    source: "hook:claude".to_owned(),
                },
                seq: u64::from(submitted),
                asked_seq: u64::from(submitted),
                reports: 0,
                asking: None,
                asked: submitted.then(|| self.said.clone()).flatten(),
                said: None,
                said_seq: 0,
                noticed: None,
                running: None,
                transcript: None,
                settling: crate::access::Settling::Nothing,
                reporter: crate::access::ReporterVoice::Speaking,
            }))
        }
    }

    /// One text injection, a grace too short to wait out, and the submit held to the agent's own
    /// account — the spec every [`SubmittedWhen::Took`] gate here delivers under.
    fn asking_once() -> Delivery {
        Delivery {
            echo_timeout: Duration::from_millis(1),
            attempts: 1,
            submitted_when: SubmittedWhen::Took {
                within: Duration::from_millis(50),
            },
            ..Delivery::new()
        }
    }

    /// **A PANE WHOSE COMPOSER IS THE THING BEING WATCHED** — the double
    /// [`SubmittedWhen::Released`] is measured over.
    ///
    /// # ⚠⚠⚠ Its `seq` NEVER MOVES, and that is the fixture's whole point
    ///
    /// A double whose published sequence advanced with the submit would satisfy
    /// [`SubmittedWhen::Stirs`] as well, and then a green `Released` gate would prove nothing about
    /// `Released` — the neighbouring contract would have carried it. Freezing `seq` at zero makes
    /// the two contracts give OPPOSITE answers over one fixture, which is the only way to show that
    /// this one rests on a property rather than on an event.
    ///
    /// # ⛔⛔⛔⛔⛔ It was SCRAPED-ONLY, and that was the defect — register item 762
    ///
    /// This paragraph read: *"[`AgentState::Holding`] is a conclusion a manifest rule reaches by
    /// reading the composer. `sprag_detect`'s tracker does not run those rules on a pane a hook is
    /// reporting, so a fixture that published `Holding` under `Authority::Reported` would be staging
    /// a shape the product cannot produce."* Every word of it was true, and it meant this contract
    /// **could only ever be exercised on a population a supervisor never drives** — a supervisor's
    /// agents all report.
    ///
    /// The fact now travels in its own slot ([`crate::access::AgentObservation::holding`]) instead
    /// of in the arbitrated state, so a reported pane answers it and the shape below is one the
    /// product produces. [`reported`](Self::reported) is what stages it, and
    /// `a_reported_pane_settles_the_composer_contract` is the gate that would have been
    /// unwritable.
    struct Composing {
        inner: Recorder,
        /// Whether this pane's agent is REPORTING — the population a supervisor actually drives.
        ///
        /// ⚠⚠ It changes the `state` and the `authority` and NOT the composer reading, which is
        /// exactly the split register item 762 made: a hook says what the agent is doing, the
        /// screen says what the composer is holding, and the two no longer share a slot.
        reported: bool,
        /// Whether this supervisor CANNOT SAY what the composer is holding — a pane no manifest
        /// claims, a manifest with no `Holding` rule, or a daemon too old to send the key.
        ///
        /// ⚠⚠ The third answer, staged, because it is the one a contract must refuse on rather than
        /// read as *it let go*: reading an absence as the satisfied side would confirm a submit off
        /// a daemon that had merely stopped answering.
        blind: bool,
        /// ⛔⛔⛔⛔⛔ **CANNOT SAY ONLY AFTER THE SUBMIT** — the supervisor answered at arming and
        /// stopped answering afterwards, which is a daemon going away, a manifest reload, or a pane
        /// whose agent changed under the delivery.
        ///
        /// # ⚠⚠⚠⚠⚠ Why this is a separate field, and it is a DEAD CONTROL that made it one
        ///
        /// [`blind`](Self::blind) cannot exercise the JUDGING half at all: a supervisor blind from
        /// the start fails to arm the baseline, so the contract refuses before it ever asks the
        /// second question. Mutating *the absence means it let go* into the judging arm therefore
        /// left every gate GREEN — measured, on this file, and it is the shape this workspace keeps
        /// meeting: a control that fires on the road the mutation does not travel.
        ///
        /// Armed and THEN blind is the only staging where the absence reaches the comparison, and
        /// it is where reading it as satisfied would confirm a submit that never landed.
        blind_after: bool,
        /// Whether the composer is holding an unsubmitted prompt BEFORE the submit goes in — the
        /// baseline this contract arms against.
        holding_before: bool,
        /// Whether it is STILL holding once the submit has been injected. `true` is THE JAM: the
        /// keystroke went out and the prompt is sitting there.
        holding_after: bool,
        /// How many times the supervisor has been asked, so a gate can assert that a contract
        /// nothing could answer was refused AT ONCE rather than after its window.
        looks: Mutex<u32>,
    }

    impl Composing {
        /// A pane showing `text`, holding it before the submit and letting go after — the ordinary
        /// success.
        fn releasing(text: &str) -> Self {
            Self {
                inner: Recorder::showing(text),
                reported: false,
                blind: false,
                blind_after: false,
                holding_before: true,
                holding_after: false,
                looks: Mutex::new(0),
            }
        }

        /// How many times the supervisor was asked.
        fn looks(&self) -> u32 {
            *self.looks.lock().expect("the counter")
        }

        /// One delivery against this double under `spec`.
        fn deliver_under(&self, text: &str, spec: &Delivery) -> Delivered {
            deliver(self, &RunContext::uncancellable(), PaneId(1), text, spec).expect("no error")
        }

        /// How many submits went out — the count and not a `bool`, for `Reporting::submits`' reason.
        fn submits(&self) -> usize {
            self.inner
                .injected
                .lock()
                .expect("the log")
                .iter()
                .filter(|keys| keys == &&vec!["Enter".to_owned()])
                .count()
        }
    }

    impl PaneAccess for Composing {
        fn pane_ids(&self) -> Vec<PaneId> {
            self.inner.pane_ids()
        }
        fn pane_collapsed(&self, id: PaneId) -> Option<String> {
            self.inner.pane_collapsed(id)
        }
        fn pane_rows(&self, id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
            self.inner.pane_rows(id)
        }
        fn pane_eof(&self, id: PaneId) -> Option<bool> {
            self.inner.pane_eof(id)
        }
        fn pane_full_text(&self, id: PaneId) -> Option<String> {
            self.inner.pane_full_text(id)
        }
        fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
            self.inner.inject(id, keys)
        }
        fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
            Some(self)
        }
        fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
            Some(&self.inner)
        }
    }

    impl crate::access::PaneSupervision for Composing {
        fn pane_agent_state(&self, _id: PaneId) -> crate::access::Supervised {
            *self.looks.lock().expect("the counter") += 1;
            let submitted = self.inner.submitted();
            let holding = if submitted {
                self.holding_after
            } else {
                self.holding_before
            };
            // ⚠ *Nothing could say*, either from the start or only once the submit has gone out —
            // see `blind_after` for why the second staging had to exist.
            let cannot_say = self.blind || (self.blind_after && submitted);
            crate::access::Supervised::Seen(Box::new(crate::access::AgentObservation {
                // ⛔⛔⛔⛔⛔ THE STATE A REPORTED PANE PUBLISHES IS THE HOOK'S, AND IT IS NOT
                // `Holding` — register item 762, and this arm is the product's shape rather than
                // the fixture's convenience. `sprag_detect`'s tracker still refuses to let a
                // manifest rule decide the state of a pane whose agent reports (register item 524's
                // carve-out is untouched), so a supervisor's pane reads `working` while its
                // composer holds a prompt nobody submitted.
                state: if self.reported {
                    AgentState::Working
                } else if holding {
                    AgentState::Holding
                } else {
                    AgentState::Idle
                },
                // ⚠⚠ AND THE COMPOSER READING IS THE SAME EITHER WAY, which is the whole of the
                // repair: the fact lives in a slot the arbitration does not touch, so it survives
                // a report standing over it.
                holding: (!cannot_say).then_some(holding),
                agent: Some("claude".to_owned()),
                authority: if self.reported {
                    crate::access::Authority::Reported {
                        source: "hook:claude".to_owned(),
                    }
                } else {
                    crate::access::Authority::Scraped {
                        rule: Some(
                            if holding {
                                "composer-holds-paste"
                            } else {
                                "idle-glyph"
                            }
                            .to_owned(),
                        ),
                    }
                },
                // ⚠ FROZEN — see the type's doc. Nothing here is an event.
                seq: 0,
                asked_seq: 0,
                reports: 0,
                asking: None,
                asked: None,
                said: None,
                said_seq: 0,
                noticed: None,
                running: None,
                transcript: None,
                settling: crate::access::Settling::Nothing,
                reporter: crate::access::ReporterVoice::Speaking,
            }))
        }
    }

    /// One text injection, a grace too short to wait out, and the submit held to the COMPOSER —
    /// the spec every [`SubmittedWhen::Released`] gate here delivers under.
    fn releasing_once() -> Delivery {
        Delivery {
            echo_timeout: Duration::from_millis(1),
            attempts: 1,
            submitted_when: SubmittedWhen::Released {
                within: Duration::from_millis(50),
            },
            ..Delivery::new()
        }
    }

    /// ⚠⚠⚠⚠⚠ **A COMPOSER THAT LET GO IS A SUBMIT, AND ONE STILL HOLDING IS NOT** — register item
    /// 669's second stage, and the pair of screens that are one Enter apart.
    ///
    /// # The defect this answers, measured on five live runs
    ///
    /// Every other submit contract waits for something to HAPPEN. When it does not, *not yet* and
    /// *never* are the same silence, so the wait expires and the answer is a guess. Item 669
    /// measured the cost: four of five running loops had prompts that were never asked, and no run
    /// could tell — because the only channel for *not submitted* is the absence of the channels for
    /// *submitted*.
    ///
    /// # ⚠⚠⚠ THE CONTROL IS THE SAME FIXTURE UNDER THE NEIGHBOURING CONTRACT
    ///
    /// [`Composing`]'s `seq` is frozen, so [`SubmittedWhen::Stirs`] can never be satisfied over it.
    /// The two contracts therefore answer OPPOSITELY on one pane, and that difference is the claim:
    /// this contract reads a PROPERTY, and an event-shaped one has nothing to read.
    #[test]
    fn a_composer_that_let_go_of_the_prompt_is_a_submit_and_one_still_holding_is_not() {
        let released = Composing::releasing("ORTHOGONAL-669");
        let landed = released.deliver_under("ORTHOGONAL-669", &releasing_once());
        assert!(
            landed.is_confirmed(),
            "⚠⚠⚠ the composer was holding this prompt when the Enter went in and is not holding \
             one now: nothing else on this pane moved, and that absence IS the evidence. \
             Got {landed:?}",
        );
        assert_eq!(
            released.submits(),
            1,
            "and exactly one Enter — a second would land on whatever the composer holds next",
        );

        // ⚠⚠⚠ THE CONTROL, one bit different: the same delivery over a composer that never let go.
        // This is the JAM item 669 exists for — the keystroke is out and the prompt is still
        // sitting in the box — and it must be a refusal rather than a slower success.
        let jammed = Composing {
            holding_after: true,
            ..Composing::releasing("ORTHOGONAL-669")
        };
        let stuck = jammed.deliver_under("ORTHOGONAL-669", &releasing_once());
        assert!(
            matches!(
                stuck,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Released { .. },
                    ..
                },
            ),
            "⚠⚠⚠⚠ the prompt is STILL IN THE COMPOSER after the press. Answering anything else \
             here is the sixty seconds a live `claude` sat in with a prompt nobody had asked. \
             Got {stuck:?}",
        );

        // ⚠⚠⚠⚠⚠ AND THE EVENT-SHAPED CONTRACT CANNOT SEE THE SUCCESS AT ALL. Same double, same
        // Enter, same release — `Stirs` waits for a published change and this pane publishes none,
        // so it refuses the very delivery `Released` confirmed. Without this the gate above could
        // be passing on evidence `Released` never used.
        let stirring = Composing::releasing("ORTHOGONAL-669");
        let unseen = stirring.deliver_under(
            "ORTHOGONAL-669",
            &Delivery {
                submitted_when: SubmittedWhen::Stirs {
                    within: Duration::from_millis(50),
                },
                ..releasing_once()
            },
        );
        assert!(
            matches!(
                unseen,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Stirs { .. },
                    ..
                },
            ),
            "⚠ the property is there to be read and the EVENT is not, which is the whole \
             difference between the two contracts: {unseen:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A PANE WHOSE AGENT IS REPORTING SETTLES THIS CONTRACT, AND UNTIL NOW COULD NOT**
    /// — register item 762, and the gate the population this module exists for was missing.
    ///
    /// # ⛔⛔⛔⛔ What was wrong, and how far it reached
    ///
    /// [`SubmittedWhen::Released`] is the ONE submit contract that CONVERGES: a composer is holding
    /// or it is not, both readings are stable, so it answers instead of expiring. Every other kind
    /// waits for an EVENT and, when the event does not come, cannot tell *not yet* from *never* —
    /// register item 669 measured that as four of five live runs holding prompts that were never
    /// asked, with the run unable to say so.
    ///
    /// It read the composer off `AgentState::Holding`, which is a manifest rule — and
    /// `sprag_detect`'s tracker does not run manifest rules on a pane a hook is reporting. **A
    /// supervisor's agents all report.** So the convergent contract refused on the whole population
    /// it was built for, every delivery there fell back to the agent's own account, and a prompt
    /// the composer folded away left the driver with one channel and a timeout. Register item 762
    /// then watched a live run die of exactly that at 187 iterations, with the pane it had been
    /// driving reading `working seq=26 said=12`.
    ///
    /// # ⚠⚠⚠ What is asserted, and why the state assertion is half of it
    ///
    /// The pane below publishes `working` — the hook's word, standing exactly as register item 524
    /// arranged — while its composer holds a prompt nobody submitted. The delivery must still land,
    /// which is only possible because the two facts stopped sharing a slot. **The arbitration is
    /// asserted UNCHANGED in the same breath**, because a repair that had simply let the composer
    /// overrule the report would pass the first half and be the change register item 524 refused.
    #[test]
    fn a_reported_pane_settles_the_composer_contract() {
        let reported = Composing {
            reported: true,
            ..Composing::releasing("ORTHOGONAL-762")
        };
        let landed = reported.deliver_under("ORTHOGONAL-762", &releasing_once());
        assert!(
            matches!(
                landed,
                Delivered::Confirmed { .. } | Delivered::OnScreenOnly { .. }
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: this pane's agent is REPORTING and its composer let go of \
             the prompt, which is the whole of what this contract asks. Until the composer reading \
             got a slot of its own it was read off the arbitrated state, where a hook outranks the \
             screen — so `Holding` was never published here, the baseline could not be armed, and \
             the one convergent submit contract refused on every pane a supervisor drives. Got \
             {landed:?}",
        );

        // ── AND THE ARBITRATION IS UNTOUCHED, which is what says this is a second SLOT ──
        //
        // ⚠⚠⚠ Without this the arm above would also be green for the repair register item 524
        // REFUSED — letting a second screen fact overrule a report, which would decide, unmeasured,
        // what a pane says when its agent is genuinely working and a paste is queued.
        let watching = Composing {
            reported: true,
            ..Composing::releasing("ORTHOGONAL-762")
        };
        let seen = crate::access::PaneSupervision::pane_agent_state(&watching, PaneId(1))
            .seen()
            .expect("this double always answers");
        assert_eq!(
            (seen.state, seen.holding),
            (AgentState::Working, Some(true)),
            "⚠⚠⚠⚠⚠ REGISTER ITEM 524, HELD: a reported pane still publishes its REPORTER's word as \
             its state. The composer answers beside it and never instead of it — that is the split \
             this item made, and a `Holding` state here would be the change 524 declined",
        );
        assert!(
            matches!(seen.authority, crate::access::Authority::Reported { .. }),
            "⚠⚠ and the authority says who spoke, or the assertion above is about a scraped pane \
             wearing the wrong state",
        );

        // ── AND THE JAM IS STILL A JAM ON THIS POPULATION ──
        //
        // ⚠⚠⚠⚠ A contract that answered YES for every reported pane would pass both arms above and
        // be worse than the refusal it replaced: it would confirm a submit that never landed, on
        // the one population that matters. This is the same pane with the composer STILL holding
        // after the Enter, which is what a fold that did not submit looks like.
        let jammed = Composing {
            reported: true,
            holding_after: true,
            ..Composing::releasing("ORTHOGONAL-762")
        };
        let stuck = jammed.deliver_under("ORTHOGONAL-762", &releasing_once());
        assert!(
            matches!(
                stuck,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Released { .. },
                    ..
                },
            ),
            "⛔⛔⛔⛔ REGISTER ITEM 762: the keystroke went out and the composer is STILL holding \
             the prompt, so nothing was asked and this delivery must say so. A `Confirmed` here \
             means the contract stopped reading the composer and started answering yes to a \
             reported pane on sight. Got {stuck:?}",
        );

        // ── AND THE THIRD ANSWER IS REFUSED, NOT READ AS THE SATISFIED ONE ──
        //
        // ⛔⛔⛔⛔⛔ *Nothing could say* is a pane no manifest claims, a manifest with no `Holding`
        // rule, or a daemon too old to send the key — and on the last of those it is the COMMON
        // case during a rollout. Reading it as *the composer let go* would confirm a submit off a
        // supervisor that had merely stopped answering, which is the inversion every absence in
        // this crate is written against. It must refuse on the arming read alone, like the empty
        // composer one gate down.
        let blind = Composing {
            reported: true,
            blind: true,
            ..Composing::releasing("ORTHOGONAL-762")
        };
        let unanswerable = blind.deliver_under("ORTHOGONAL-762", &releasing_once());
        assert!(
            matches!(
                unanswerable,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Released { .. },
                    ..
                },
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: this supervisor cannot say what the composer holds, so \
             nothing it reports later is evidence about this keystroke. A `Confirmed` here means \
             the absence is being read as *it let go*. Got {unanswerable:?}",
        );
        assert_eq!(
            blind.looks(),
            1,
            "and refused on the ARMING read alone — a contract that cannot be answered must not \
             spend its window finding that out",
        );

        // ── AND THE ABSENCE THAT ARRIVES *AFTER* THE KEYSTROKE IS REFUSED TOO ──
        //
        // ⛔⛔⛔⛔⛔ THIS ARM EXISTS BECAUSE THE ONE ABOVE WAS A DEAD CONTROL. Mutating the judging
        // comparison from *the composer says it is empty* to *the composer does not say it is
        // holding* left every gate in this file GREEN: a supervisor blind from the start never arms
        // the baseline, so the mutated line is never reached. Armed and THEN blind is the only
        // staging that travels that road — a daemon that went away mid-delivery, a manifest reload,
        // a pane whose agent changed under the submit.
        //
        // ⚠⚠ And it is exactly where the mutation would be a FALSE CONFIRMATION: the prompt may be
        // sitting in that composer untouched, and the only thing that changed is that nobody can
        // look any more.
        let went_away = Composing {
            reported: true,
            blind_after: true,
            holding_after: true,
            ..Composing::releasing("ORTHOGONAL-762")
        };
        let lost = went_away.deliver_under("ORTHOGONAL-762", &releasing_once());
        assert!(
            matches!(
                lost,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Released { .. },
                    ..
                },
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: this composer was holding when the Enter went in and the \
             supervisor stopped answering afterwards, so nothing is known about where that prompt \
             is. A `Confirmed` here reads *nobody could look* as *it let go* and settles a submit \
             on the disappearance of the instrument. Got {lost:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A COMPOSER BASELINE IS NEVER ARMED WITHOUT AN AGENT BASELINE** — register item
    /// 762, and the predicate that explains why one half of [`Submission::landed`]'s rule cannot be
    /// mutated red.
    ///
    /// # ⚠⚠⚠ What this is standing in for
    ///
    /// `landed` abandons a wait only when EVERY armed witness can never speak. That is the right
    /// rule and its second half is unreachable today: mutating it to *abandon when the PRIMARY
    /// cannot speak* leaves every gate green. The reason is here rather than in prose — both
    /// baselines are drawn from ONE `pane_agent_state` reading and both require it to NAME AN
    /// AGENT, so *`Took` can never speak* implies *`Released` can never speak*.
    ///
    /// **A gate for an implication, because that implication is what makes an untested branch
    /// safe.** The day a contract is armed off something that is not an agent's name, this goes red
    /// and the rule up there stops being decoration.
    #[test]
    fn a_composer_baseline_is_never_armed_without_an_agent_baseline() {
        /// A supervisor whose pane is holding a paste and whose observation names NOBODY — the one
        /// shape that could arm a composer baseline while leaving the agent one empty.
        struct Nameless;

        impl crate::access::PaneSupervision for Nameless {
            fn pane_agent_state(&self, _id: PaneId) -> crate::access::Supervised {
                crate::access::Supervised::Seen(Box::new(crate::access::AgentObservation {
                    state: AgentState::Working,
                    // ⚠ THE COMPOSER SAYS YES and the identity says nothing, which is the pair
                    // this gate exists to hold apart.
                    holding: Some(true),
                    agent: None,
                    authority: crate::access::Authority::Scraped { rule: None },
                    seq: 0,
                    asked_seq: 0,
                    reports: 0,
                    asking: None,
                    asked: None,
                    said: None,
                    said_seq: 0,
                    noticed: None,
                    running: None,
                    transcript: None,
                    settling: crate::access::Settling::Nothing,
                    reporter: crate::access::ReporterVoice::Speaking,
                }))
            }
        }

        struct Watched(Recorder, Nameless);

        impl PaneAccess for Watched {
            fn pane_ids(&self) -> Vec<PaneId> {
                self.0.pane_ids()
            }
            fn pane_collapsed(&self, id: PaneId) -> Option<String> {
                self.0.pane_collapsed(id)
            }
            fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
                self.0.pane_rows(id)
            }
            fn pane_eof(&self, id: PaneId) -> Option<bool> {
                self.0.pane_eof(id)
            }
            fn pane_full_text(&self, id: PaneId) -> Option<String> {
                self.0.pane_full_text(id)
            }
            fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
                self.0.inject(id, keys)
            }
            fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
                Some(&self.1)
            }
        }

        let watched = Watched(Recorder::showing("ORTHOGONAL-762"), Nameless);
        let armed = Submission::arm(
            &watched,
            PaneId(1),
            SubmittedWhen::Took {
                within: Duration::from_millis(1),
            },
            Some(SubmittedWhen::Released {
                within: Duration::from_millis(1),
            }),
            "ORTHOGONAL-762",
        );
        assert!(
            armed.agent.is_none(),
            "the staging: this observation must name NOBODY, or the implication below is being \
             checked on a pane that has an identity after all",
        );
        assert!(
            armed.holding.is_none(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: an observation that names no agent must arm NO composer \
             baseline, because a claim about *this peer's* composer needs to know which peer that \
             is. It is also what makes `landed`'s untested half safe — the day this holds a value, \
             `Never` taken off the primary contract alone starts abandoning waits the composer \
             could have answered",
        );
    }

    /// ⛔⛔⛔⛔⛔ **WHICH WITNESSES MEAN *THE PANE CANNOT ANSWER FOR THIS PROMPT*** — register item
    /// 762, and the gate that keeps [`Witnessed::folded_away`] from being a classification only one
    /// call site knows.
    ///
    /// # ⚠⚠⚠ Why both directions are asserted, and by NAME
    ///
    /// [`crate::plugin::Deliveries::folded`] carries one remedy — *do not go and look at that pane*
    /// — and it is read by a person deciding where to look. A witness wrongly IN it sends them
    /// away from a prompt that is sitting on a screen; a witness wrongly OUT of it sends them to a
    /// pane showing `[Pasted text +N lines]`. Both halves are the number being wrong, so both are
    /// pinned, and each variant is named rather than counted — a test asserting *two of them* would
    /// stay green when a road swapped sides.
    ///
    /// ⚠ The classification itself is exhaustive in the type, so a SEVENTH witness fails to
    /// compile rather than failing here. What this adds is the decision for the six that exist.
    #[test]
    fn the_witnesses_that_mean_the_pane_cannot_answer_are_named() {
        for witness in [Witnessed::Account, Witnessed::LetGo] {
            assert!(
                witness.folded_away(),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 762: {witness:?} is a composer that ATE the paste — the \
                 prompt is on no screen — so a person sent to that pane finds a placeholder. \
                 `Deliveries::folded` is the number that says so, and leaving this road out of it \
                 publishes the reassuring answer: *this run's prompts are visible*",
            );
        }
        for witness in [
            Witnessed::Painted,
            Witnessed::Echoed,
            Witnessed::Unchecked,
            Witnessed::Unasked,
            Witnessed::Unproven,
        ] {
            assert!(
                !witness.folded_away(),
                "⚠⚠⚠⚠ AND THE OTHER DIRECTION: {witness:?} did not go through a folded composer, \
                 so counting it would tell a person NOT to look at a pane their prompt may well be \
                 sitting on. A predicate that says yes to everything carries no remedy at all",
            );
        }
    }

    /// ⚠⚠⚠⚠ **A PANE THAT WAS NOT HOLDING ANYTHING CANNOT SATISFY THIS, AND SAYS SO AT ONCE.**
    ///
    /// The baseline is what makes an ABSENCE into evidence. Without it, *the composer is not
    /// holding* is a sentence that was already true before the Enter went in — so the contract
    /// would confirm a submit that was never pressed, over any pane at all. It is the same *a
    /// condition already true when you started is not evidence* rule the text's own read-back and
    /// [`SubmittedWhen::Repaints`] are held to, and it is sharper here because the satisfied
    /// reading is the negative one.
    ///
    /// ⚠⚠ **AND THE REFUSAL MUST BE IMMEDIATE**, which is asserted rather than assumed: a contract
    /// nothing can answer spends its whole window discovering that, and the run pays the window for
    /// no information. `looks` is the instrument — one reading, at arming, and none afterwards.
    #[test]
    fn a_pane_that_was_holding_nothing_can_never_satisfy_the_composer_contract() {
        let clean = Composing {
            holding_before: false,
            ..Composing::releasing("ORTHOGONAL-669")
        };
        let refused = clean.deliver_under("ORTHOGONAL-669", &releasing_once());
        assert!(
            matches!(
                refused,
                Delivered::Unsubmitted {
                    wanted: SubmittedWhen::Released { .. },
                    ..
                },
            ),
            "⚠⚠⚠⚠ this pane's composer was empty BEFORE the Enter, so *it is empty now* says \
             nothing about the keystroke. A yes here would be a yes for every pane ever \
             delivered to. Got {refused:?}",
        );
        assert_eq!(
            clean.looks(),
            1,
            "and it was refused on the ARMING read alone — a contract that cannot be answered must \
             not spend its window finding that out",
        );
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
    /// screen.
    ///
    /// ⚠⚠⚠ **THAT SECOND ROAD EXISTS NOW, AND IT IS WHY THIS GATE STILL PASSES RATHER THAN BEING
    /// STALE**: `PaneAccess::supervision` carries the agent's own account of the question it
    /// received ([`SubmittedWhen::Took`]), and its neighbour
    /// [`a_prompt_the_agent_never_reports_receiving_is_refused_however_the_screen_reads`] drives
    /// THIS fixture through it and gets the opposite verdict. What keeps this one open is that the
    /// road is the CALLER's to ask for: a delivery under the default contract still reads the
    /// screen, so a dirty composer is still confirmed for every peer whose caller has not said
    /// otherwise. ⚠ The `PaneAccess` half of item 224 is paid; the DEFAULT is not.
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

    /// **THE GATE FOR EVIDENCE FROM THE PROGRAM** — register items 421, 223 and 224, which are one
    /// defect seen from three sides.
    ///
    /// The screen cannot answer *did MY question arrive*: text a run delivered and text a composer
    /// already held are the same pixels, so the read-back's `contains` says only *something like
    /// mine is on the pane*. Its neighbour
    /// [`a_prompt_typed_onto_a_dirty_composer_is_confirmed_and_submitted_anyway`] drives exactly
    /// that and records that tightening the predicate is RULED OUT — measured, not assumed.
    ///
    /// ⚠⚠⚠ **THIS ASKS THE AGENT INSTEAD.** Its own submit hook names the question it received, so
    /// the concatenation the composer produced is visible as a different string. The first case is
    /// the same fixture the neighbour passes with, and the difference between the two gates is the
    /// whole point: same screen, same delivery, opposite verdicts.
    #[test]
    fn a_prompt_the_agent_never_reports_receiving_is_refused_however_the_screen_reads() {
        const OFFERED: &str = "> what is 3 plus 3 in English?";
        const SENT: &str = "what is 4 plus 4?";

        // The composer already holds a suggestion and appends the delivery to it — the screen the
        // neighbouring gate confirms on. The agent reports the WHOLE line it was handed.
        let dirty = Recorder {
            text: format!("{OFFERED}{SENT}"),
            showing_before: OFFERED.to_owned(),
            ..Recorder::showing(SENT)
        };
        let delivered = dirty.deliver_asking(SENT, Some(&format!("{OFFERED}{SENT}")));
        assert!(
            matches!(delivered, Delivered::Unsubmitted { .. }),
            "⚠⚠⚠⚠ ITEM 223, SETTLED BY THE PROGRAM: the screen says the delivery arrived — it \
             does contain {SENT:?} — and the agent says it was asked {OFFERED:?} with this text on \
             the end of it. Those are different questions, and only the agent can say so. Got \
             {delivered:?}",
        );

        // The control: the same delivery onto a CLEAN composer, where the agent reports exactly
        // what was sent.
        let clean = Recorder::showing(SENT);
        let confirmed = clean.deliver_asking(SENT, Some(SENT));
        assert!(
            !matches!(
                confirmed,
                Delivered::Unsubmitted { .. } | Delivered::Unconfirmed { .. }
            ),
            "⚠ the control: an agent reporting the question that was sent must confirm, or the \
             claim above is about a contract that refuses everything. Got {confirmed:?}",
        );

        // ⚠⚠ AND A PEER THAT REPORTS NOTHING IS REFUSED RATHER THAN ASSUMED — an agent with no
        // hooks says nothing, and a delivery that read silence as success would be the old oracle
        // with extra steps. The loop asks this contract only of a pane whose verdict is REPORTED,
        // which is what keeps a scraped peer on the screen predicate instead.
        let silent = Recorder::showing(SENT);
        let unheard = silent.deliver_asking(SENT, None);
        assert!(
            matches!(unheard, Delivered::Unsubmitted { .. }),
            "⚠⚠ silence is not evidence: a peer that never names the question it took cannot \
             satisfy a contract about the question it took. Got {unheard:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE GATE FOR REGISTER ITEM 421 — A PROMPT THE COMPOSER FOLDED AWAY IS STILL A
    /// PROMPT, AND THE AGENT IS WHAT SAYS SO.**
    ///
    /// # The defect, measured live on three runs running (2026-08-17)
    ///
    /// `claude` 2.1.233 COLLAPSES a long paste: the composer shows `[Pasted text #2 +5 lines]` and
    /// the prompt's own leading characters are not on the screen at all. The read-back could
    /// therefore never confirm — the supervisor was sent at the pane's WIDTH twice by the refusal's
    /// own sentence, and widening it changed nothing, in Korean or in ASCII — the submit was
    /// withheld, and the retry put THREE copies of the prompt (4,002 bytes) into the composer before
    /// the delivery gave up. Every run died at 0 iterations.
    ///
    /// ⚠⚠⚠⚠ **AND THE PROMPT IT DIED ON WAS THE DOCUMENT'S OWN**: run 10 was given a one-sentence
    /// north star and failed at its FIRST REFLECTION, where `reflecting` composes the prompt itself.
    /// No caller could shorten it, so **the loop could not complete one cycle** — no reflection, no
    /// session replacement, no next milestone ever chosen.
    ///
    /// # What this gate holds, and why it is not a fourth patch of the screen predicate
    ///
    /// The three patches are on the record: 40 characters became 40 COLUMNS, the head became the
    /// TAIL, an exact match became a whitespace-insensitive one. Each was a different way of asking
    /// a RENDERING whether a program had read something. Here the screen moved (the placeholder
    /// appeared), it never carried the text, and **the agent's own submit hook names the question it
    /// received** — evidence from the only party that knows, which
    /// [`SubmittedWhen::Took`] already gets for the keystroke and which this
    /// gate extends to the text it was pressed over.
    ///
    /// ⚠ The two COUNTS are asserted beside the verdict, because the live failure was as much about
    /// them: one text injection (a second copy lands on a composer that already holds the first) and
    /// one submit.
    #[test]
    fn a_prompt_the_composer_folded_away_is_delivered_on_the_agents_own_account() {
        /// What the delivery is asking — longer than the composer will show inline.
        const SENT: &str = "NEXT MILESTONE: pay the top open item, gate it, mutate it";
        /// What the composer shows instead: the paste, folded, with the text nowhere on screen.
        const FOLDED: &str = "> [Pasted text #2 +5 lines]";

        let folded = || Recorder {
            text: FOLDED.to_owned(),
            showing_before: String::new(),
            ..Recorder::showing(FOLDED)
        };

        let double = folded().reporting(Some(SENT));
        let delivered = double.deliver_under(SENT, &asking_once());
        assert!(
            matches!(delivered, Delivered::Reported { attempts: 1, .. }),
            "⚠⚠⚠⚠⚠ ITEM 421: the composer folded the paste away, so the screen can never carry \
             {SENT:?} — and the agent says that is the question it was asked. A delivery whose \
             verdict is the screen's cannot ever reach this pane. Got {delivered:?}, log {:?}",
            double.log(),
        );
        assert_eq!(
            (double.text_injections(), double.submits()),
            (1, 1),
            "⚠⚠⚠ ONE COPY AND ONE PRESS. The live failure re-injected until it gave up — three \
             copies of the reflection prompt in one composer — because a screen that had taken the \
             text looked exactly like one that had swallowed it. Log: {:?}",
            double.log(),
        );
        // ⚠⚠⚠⚠ AND THE TWO QUESTIONS COME APART HERE, WHICH IS THE ONLY PLACE THEY DO. The program
        // is holding the text — the strongest answer this module has — and NOTHING a person could
        // look at on that pane says so. A caller that reads one of these for the other is reading a
        // folded paste as a visible prompt, or a proven delivery as an unproven one.
        assert!(
            delivered.is_confirmed() && !delivered.is_on_screen(),
            "confirmed by the program, and not on the screen: {delivered:?}",
        );

        // ── ⛔⛔⛔⛔⛔ AND THE SAME ROAD WHEN THE ACCOUNT NEVER COMES — register item 762 ──────────
        //
        // The arm above is this road's SUCCESS. Its failure — the composer swallowed the paste and
        // the peer never named the question — used to answer `Unsubmitted`, whose whole sentence is
        // *the prompt is sitting in the pane*. On this road the screen moved WITHOUT the text, so
        // that instruction points at a placeholder. `run110` died here twice and the round reading
        // its record went and looked at a healthy pane.
        //
        // ⚠ Same fixture, one knob: the peer reports NOTHING. That is what makes it this road's
        // refusal rather than the swallow control below — the screen still moved, so the bytes are
        // accounted for and a retry would land on top of them.
        let unnamed = folded().reporting(None);
        let lost = unnamed.deliver_under(SENT, &asking_once());
        assert!(
            matches!(lost, Delivered::Unreported { attempts: 1, .. }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: a folded paste the peer never named must be its own answer. \
             As `Unsubmitted` it reaches a supervisor as *the prompt is sitting in the pane* — and \
             the pane is showing {FOLDED:?}. Got {lost:?}, log {:?}",
            unnamed.log(),
        );
        assert!(
            !lost.is_on_screen() && !lost.is_confirmed(),
            "⛔⛔⛔ REGISTER ITEM 762: this answer is neither on the screen nor confirmed, and both \
             halves matter — `is_on_screen` is what a caller asks before it decides whether a \
             person can go and read the prompt: {lost:?}",
        );
        assert_eq!(
            (unnamed.text_injections(), unnamed.submits()),
            (1, 1),
            "⚠⚠ ONE COPY AND ONE PRESS on this road too. The `attempts: 1` in the answer above is \
             this module's control flow and not a measurement of the peer — the fold arm returns \
             out of the injection loop, so the spare attempts are unreachable. Log: {:?}",
            unnamed.log(),
        );

        // ── ⛔⛔⛔⛔⛔ AND THE SECOND CHANNEL, WHICH IS WHAT THAT DEATH WAS MISSING ─────────────
        //
        // Everything above this line has ONE witness: the agent's own account. That contract
        // EXPIRES — when the account does not come there is nothing to tell *not yet* from *never*
        // — so its silence was answered *the peer would not take the question*, which the driver
        // pays for with a session. `SubmittedWhen::Released` CONVERGES instead, because a composer
        // is holding or it is not; arming it beside the account is register item 762's repair, and
        // these four arms are the four things the pair can say.
        //
        // ⚠⚠ THE FIXTURE'S ONLY CHANGE IS THE COMPOSER PAIR. Same folded screen, same silent peer,
        // same spec — so what separates these answers from `lost` above is the second witness and
        // nothing else.
        let let_go = Reporting {
            composer: (Some(true), Some(false)),
            ..folded().reporting(None)
        };
        let asked = let_go.deliver_under(SENT, &asking_once());
        assert!(
            matches!(asked, Delivered::Released { attempts: 1, .. }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: this composer was holding the folded paste when the Enter \
             went in and is empty afterwards, so THE QUESTION WAS ASKED — the peer has it. An \
             `Unreported` here is the answer that killed `run110`: it reaches the driver as *the \
             peer would not take the question*, which buys a session replacement for a session that \
             is working. Got {asked:?}, log {:?}",
            let_go.log(),
        );
        assert!(
            !asked.is_on_screen(),
            "⚠⚠ and it is still not on any screen — the paste was folded, which is the road we are \
             on. A caller asking whether a person can go and read the prompt must still hear no: \
             {asked:?}",
        );

        // ⚠⚠⚠⚠ THE STRONGER WITNESS WINS WHERE BOTH COULD SPEAK. The account names the TEXT and
        // the composer names only that something was submitted, so a delivery that reported the
        // weaker one where the stronger was available would publish less than it knew.
        let both = Reporting {
            composer: (Some(true), Some(false)),
            ..folded().reporting(Some(SENT))
        };
        let named = both.deliver_under(SENT, &asking_once());
        assert!(
            matches!(named, Delivered::Reported { attempts: 1, .. }),
            "⛔⛔⛔ REGISTER ITEM 762: the agent named this question AND the composer let go. The \
             answer must be the account's, because it is the one that says WHICH question was \
             asked. Got {named:?}",
        );

        // ⚠⚠⚠⚠⚠ AND A COMPOSER THAT IS STILL HOLDING IS STILL THE OLD REFUSAL. This is the arm
        // that keeps the repair from being *call every fold a delivery*: the prompt is in that box,
        // nobody was asked, and a session replacement is exactly right (register item 446).
        let stuck = Reporting {
            composer: (Some(true), Some(true)),
            ..folded().reporting(None)
        };
        let held = stuck.deliver_under(SENT, &asking_once());
        assert!(
            matches!(held, Delivered::Unreported { attempts: 1, .. }),
            "⛔⛔⛔⛔ REGISTER ITEM 762: the Enter went out and the composer is STILL holding the \
             paste, so nothing was asked. A `Released` here means the second witness stopped \
             reading the composer and started answering yes on sight. Got {held:?}",
        );

        // ⛔⛔⛔⛔⛔ AND THE ABSENCE THAT ARRIVES *AFTER* THE KEYSTROKE IS NOT *IT LET GO*. Armed,
        // then blind — a daemon that went away, a manifest reload, a pane whose agent changed under
        // the submit. The prompt may be sitting in that box untouched and the only thing that
        // changed is that nobody can look.
        //
        // ⚠⚠ IT IS THE ARM A DEAD CONTROL TAUGHT: a supervisor blind from the START never arms the
        // baseline, so it is refused before the judging comparison is ever reached and a mutation
        // there stays green. Measured, on this file, one round earlier.
        let went_away = Reporting {
            composer: (Some(true), None),
            ..folded().reporting(None)
        };
        let unknown = went_away.deliver_under(SENT, &asking_once());
        assert!(
            matches!(unknown, Delivered::Unreported { attempts: 1, .. }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: the composer could be read when the Enter went in and not \
             afterwards, so nothing is known about where that prompt is. A `Released` here settles \
             a delivery on the disappearance of the instrument. Got {unknown:?}",
        );

        // ⚠⚠⚠⚠⚠ AND A COMPOSER THAT WAS EMPTY WHEN THE ENTER WENT IN CANNOT SATISFY IT EITHER,
        // however empty it is afterwards. *It is not holding now* was true before the keystroke, so
        // it is not evidence about the keystroke — the rule `SubmittedWhen::Released`'s baseline
        // exists for, met on the road that arms it beside another contract.
        //
        // ⚠⚠ IT IS A DIFFERENT ARM FROM THE ONE ABOVE and both are needed: that one goes blind
        // AFTER arming (the judging comparison sees an absence), this one never arms (the baseline
        // is absent). They are read by different code.
        let never_held = Reporting {
            composer: (Some(false), Some(false)),
            ..folded().reporting(None)
        };
        let vacuous = never_held.deliver_under(SENT, &asking_once());
        assert!(
            matches!(vacuous, Delivered::Unreported { attempts: 1, .. }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: this composer was empty BEFORE the Enter, so *it is empty \
             now* says nothing about the submit — a `Released` here would be a yes for every \
             delivery ever made down this road. Got {vacuous:?}",
        );

        // ⚠⚠ AND WITH NO COMPOSER READING AT ALL, THE ROAD IS EXACTLY AS IT WAS — which is what
        // `lost` above already asserts, and is why every gate written before this item keeps its
        // answer: an absence of the instrument is not a second channel.

        // ⚠⚠⚠ THE CONTROL THAT SAYS WHAT THE NEW ARM KEYS ON: a screen that never MOVED at all. The
        // bytes are unaccounted for — nothing took them, nothing painted them — so this is the
        // swallowed-input window the retry exists for, and it is not pressed over. Same contract,
        // same silence from the screen, opposite decision.
        //
        // ⚠⚠ **AND THE AGENT REPORTS NOTHING HERE, WHICH IS WHAT MAKES IT A SWALLOW.** A double that
        // named the question while its screen showed no sign of the text would be modelling a peer
        // that took the bytes silently — for which pressing is RIGHT — so the gate would be asserting
        // the opposite of what it says. A fixture has to stage the danger it claims.
        let swallowed = Recorder {
            text: "$ ".to_owned(),
            showing_before: "$ ".to_owned(),
            ..Recorder::showing("$ ")
        }
        .reporting(None);
        let never = swallowed.deliver_under(
            SENT,
            &Delivery {
                attempts: 2,
                ..asking_once()
            },
        );
        assert!(
            matches!(never, Delivered::Unconfirmed { attempts: 2, .. }),
            "a pane whose screen never moved took nothing: the retry is what serves it, and \
             pressing over it would submit an empty line an agent answers. Got {never:?}",
        );
        assert_eq!(
            (swallowed.text_injections(), swallowed.submits()),
            (2, 0),
            "⚠⚠⚠ AND NO SUBMIT WENT OUT. Log: {:?}",
            swallowed.log(),
        );

        // ⚠⚠⚠⚠ AND THE CONTROL FOR THE PEER THAT CANNOT ANSWER: the same folded composer under the
        // contract a scraped peer gets. Nothing can name the question it took, so the screen is all
        // there is and the old rule stands unchanged — this arm is reached by the authority the
        // supervisor publishes, not by a delivery deciding a press is worth the risk.
        let unhooked = folded();
        let by_the_screen = deliver(
            &unhooked,
            &RunContext::uncancellable(),
            PaneId(1),
            SENT,
            &Delivery {
                echo_timeout: Duration::from_millis(1),
                attempts: 1,
                ..Delivery::new()
            },
        )
        .expect("no error");
        assert!(
            matches!(by_the_screen, Delivered::Unconfirmed { .. }) && !unhooked.submitted(),
            "⚠⚠ an agent with no hooks reports no question, so a press over a screen that never \
             showed the text would be exactly the blind submit this module exists to prevent. Got \
             {by_the_screen:?}",
        );
    }

    /// ⚠⚠⚠⚠ **EVERY ANSWER THIS MODULE GIVES SAYS WHAT PROVED IT, AND THE TWO REFUSALS SAY
    /// NOTHING** — register item 434, held over the mapping itself rather than through a driver.
    ///
    /// # ⚠⚠⚠ Why a pure gate here as well as the loop's two
    ///
    /// The gates that drive a loop reach exactly the answers their fixtures can produce —
    /// `Unchecked` for the end-to-end peer, `Account` for the folded one — and the four remaining
    /// arms are reachable only through a pty race (a run's clock expiring INSIDE a delivery) or a
    /// pane in cooked mode. A timing fixture for those would be flaky, and **a flaky gate is worse
    /// than no gate**; this one is neither, because the mapping is a total function over a closed
    /// set and can simply be enumerated.
    ///
    /// ⚠⚠⚠⚠⚠ **THE TWO FOLD-ROAD REFUSALS SEND A READER TO OPPOSITE PLACES** — register item 762,
    /// and the assertion that keeps them from being merged back.
    ///
    /// `PaneError::NeverSubmitted` and `PaneError::NeverReported` carry the same three numbers, so
    /// nothing about their DATA says they are different facts. What differs is the instruction, and
    /// an instruction is a string — which is exactly the shape this workspace's rule 10 is about, a
    /// reason written in prose that nothing measures. It was prose until now, and what it cost is
    /// on the record: one round read *go and look at that pane* off a run whose prompt was on no
    /// pane, went and looked, found a healthy agent, and spent itself on the wrong quantity.
    #[test]
    fn the_two_fold_road_refusals_do_not_send_a_reader_to_the_same_place() {
        let wanted = SubmittedWhen::Took {
            within: Duration::from_millis(1),
        };
        let visible = crate::access::PaneError::NeverSubmitted {
            attempts: 1,
            written: 2782,
            wanted,
        }
        .to_string();
        let swallowed = crate::access::PaneError::NeverReported {
            attempts: 1,
            written: 2782,
            wanted,
        }
        .to_string();

        assert!(
            visible.contains("sitting in the pane"),
            "⚠⚠⚠ THIS GATE'S OWN PREMISE: the visible refusal must still send a reader to the pane, \
             or the assertions below separate nothing: {visible:?}",
        );
        assert!(
            swallowed.contains("Do NOT go and look at that pane"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: a prompt a composer swallowed is on NO pane, and this \
             sentence does not say so. It is the one instruction a reader acts on, and the round \
             that read the old one went to a pane holding `[Pasted text +N lines]` and diagnosed \
             the run's brief instead: {swallowed:?}",
        );
        assert!(
            !swallowed.contains("sitting in the pane"),
            "⛔⛔⛔⛔ REGISTER ITEM 762: the swallowed refusal still carries its sibling's clause, so \
             a reader gets both instructions at once and follows the wrong one: {swallowed:?}",
        );
        assert_ne!(
            visible, swallowed,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 762: the two refusals read identically, so splitting the variant \
             bought nothing — which is the disease this item is, one type earlier",
        );
    }

    /// ⚠⚠ **IT IS THE MUTATION THE DRIVER GATES LET THROUGH THAT ARGUES FOR IT.** Narrowing
    /// [`Witnessed::of`] to answer [`None`] for the two stop answers is green against every fixture
    /// that drives a loop — and it would put a run's clock expiring mid-delivery into the SAME
    /// reading as a pass that delivered nothing at all, which is this register's oldest disease.
    #[test]
    fn every_delivery_answer_says_what_proved_it_and_the_two_refusals_say_nothing() {
        let bytes = Written::of(7);
        let answers = [
            (
                Delivered::Confirmed {
                    attempts: 1,
                    written: bytes,
                },
                Some(Witnessed::Painted),
            ),
            (
                Delivered::OnScreenOnly {
                    attempts: 1,
                    written: bytes,
                    echo: Some(PaneEcho::ByTheTerminal),
                },
                Some(Witnessed::Echoed),
            ),
            (
                Delivered::Reported {
                    attempts: 1,
                    written: bytes,
                },
                Some(Witnessed::Account),
            ),
            (
                Delivered::Stopped {
                    attempts: 1,
                    written: bytes,
                },
                Some(Witnessed::Unasked),
            ),
            (
                Delivered::Unwitnessed {
                    attempts: 1,
                    written: bytes,
                    wanted: SubmittedWhen::Unchecked,
                },
                Some(Witnessed::Unproven),
            ),
            // ⚠ The two that are REFUSALS. Their callers turn them into a named `PaneError`, so a
            // walk never sees the delivery at all — and answering something here would put a
            // sentence about arrived evidence on a prompt that did not arrive.
            (
                Delivered::Unconfirmed {
                    attempts: 3,
                    written: bytes,
                },
                None,
            ),
            (
                Delivered::Unsubmitted {
                    attempts: 1,
                    written: bytes,
                    wanted: SubmittedWhen::Unchecked,
                },
                None,
            ),
        ];
        for (delivered, expected) in answers {
            assert_eq!(
                Witnessed::of(delivered),
                expected,
                "⚠⚠⚠ ITEM 434: {delivered:?} must report {expected:?}. A success whose grounds are \
                 dropped reaches a supervisor as the same `Ok(bytes)` every other success does",
            );
        }

        // ⚠⚠⚠⚠ AND NO TWO ANSWERS SHARE A SENTENCE, which is the property a reader actually uses:
        // a vocabulary whose arms rendered alike would pass every assertion above and still leave
        // a walk unable to say which road it walked.
        let sentences: std::collections::BTreeSet<&str> = [
            Witnessed::Painted,
            Witnessed::Echoed,
            Witnessed::Account,
            Witnessed::Unchecked,
            Witnessed::Unasked,
            Witnessed::Unproven,
        ]
        .into_iter()
        .map(Witnessed::noted)
        .collect();
        assert_eq!(
            sentences.len(),
            6,
            "⚠⚠⚠ six answers and {} distinct sentences — two roads rendering alike is the defect \
             this vocabulary was written to end: {sentences:?}",
            sentences.len(),
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
                OnScreen::Stopped,
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

    /// **A PANE WHOSE AGENT'S `running` THIS TEST MOVES**, and the counter beside it.
    ///
    /// ⚠⚠⚠ [`supervised_peer`] derives its verdict FROM THE PANE, deliberately, and this one cannot
    /// and must not: the fact under test is the tool named by a HOOK, which no screen states and no
    /// rule can scrape. `completion.rs`'s own fixture for the same field is this shape for the same
    /// reason. What that costs is stated — this double decides its own answer — and what keeps the
    /// gate honest is that both arms below use it and differ in one field.
    ///
    /// ⚠ [`Authority::Reported`](crate::access::Authority::Reported), because a tool name only ever
    /// arrives on a report; a scraped pane would be claiming a hook it does not have.
    fn peer_naming_a_tool(
        script: &str,
    ) -> (WorkspacePaneAccess, PaneId, Arc<Mutex<Option<String>>>) {
        peer_naming_a_tool_sized(script, 40, 6)
    }

    /// [`peer_naming_a_tool`] on a pane of a stated size — see [`access_sized`] for why one gate
    /// here needs a pane its evidence fits on.
    fn peer_naming_a_tool_sized(
        script: &str,
        cols: u16,
        rows: u16,
    ) -> (WorkspacePaneAccess, PaneId, Arc<Mutex<Option<String>>>) {
        let (access, pane) = access_sized(script, cols, rows);
        let running: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let source: crate::access::AgentStateSource = {
            let running = Arc::clone(&running);
            Arc::new(move |_id: PaneId| {
                Some(crate::access::AgentObservation {
                    state: sprag_detect::AgentState::Working,
                    holding: None,
                    agent: Some("claude".to_owned()),
                    authority: crate::access::Authority::Reported {
                        source: "test".to_owned(),
                    },
                    seq: 1,
                    asked_seq: 1,
                    // Not zero: nothing below may be read off a default.
                    reports: 6,
                    asking: None,
                    asked: None,
                    said: None,
                    said_seq: 0,
                    noticed: None,
                    running: running.lock().expect("the running mutex").clone(),
                    transcript: None,
                    settling: crate::access::Settling::Nothing,
                    reporter: crate::access::ReporterVoice::Speaking,
                })
            })
        };
        let access = access.with_agent_state(Some(source));
        assert!(
            shows(&access, pane, GO, Duration::from_secs(10)),
            "the peer never configured its terminal",
        );
        (access, pane, running)
    }

    /// ⛔⛔⛔⛔⛔ **A RUN HOLDS ITS PROMPT WHILE ITS PEER IS RUNNING A CHILD, AND TYPES AT ONCE WHEN
    /// IT IS NOT** — register item 745's cause side.
    ///
    /// # ⚠⚠⚠⚠⚠ THE TWO FIXTURES ARE THE GATE, and one of them alone proves nothing
    ///
    /// A rule that never let a prompt through would pass a *the busy peer was not typed at* arm
    /// perfectly, and that rule is a loop which sends nothing — the exact failure this door is
    /// wedged between. So the free peer is staged beside the busy one, on the same pane double,
    /// differing in ONE field: whether the agent has named a tool.
    ///
    /// * **A CHILD IS RUNNING** — [`Held::Still`], after standing there for the whole bound, and
    ///   **not one byte on the pane**.
    /// * **NOTHING IS RUNNING** — [`Held::Free`] at once, and the delivery goes through: the text
    ///   is on the screen and the answer is a delivery rather than a refusal.
    /// * ⚠⚠⚠ **AND THE CHILD ENDS MID-HOLD** — the arm that makes the first one MEAN something. A
    ///   passing `Still` cannot on its own tell *the hold was live* from *the hold never released
    ///   for any reason*; here the same peer's tool ends while the door stands there, the answer
    ///   is [`Held::Ended`] naming that tool, and the wait demonstrably outlasted the tool's life.
    #[test]
    fn a_run_holds_its_prompt_while_its_peer_is_running_a_child() {
        /// What the agent says it is running. A real tool name: the field carries the agent's own
        /// word, and a fixture that invented one would be describing nothing.
        const TOOL: &str = "Bash";
        /// The hold's bound — far above the poll cadence, so neither reading is an artefact of it.
        const WITHIN: Duration = Duration::from_millis(400);
        /// How long the third arm holds its tool open. Well inside `WITHIN`, so a hold that had
        /// simply timed out could not be mistaken for this one.
        const HELD: Duration = Duration::from_millis(150);

        // ── ARM ONE: a child is running for the whole bound ──
        let (access, pane, running) = peer_naming_a_tool(&peer("exec cat"));
        *running.lock().expect("the running mutex") = Some(TOOL.to_owned());
        let before = access.pane_collapsed(pane).unwrap_or_default();
        let started = Instant::now();
        let held = hold_while_a_child_runs(&access, &RunContext::uncancellable(), pane, WITHIN);
        let cost = started.elapsed();
        assert_eq!(
            held,
            Held::Still {
                running: TOOL.to_owned(),
                within: WITHIN,
            },
            "⛔⛔⛔⛔⛔ A PEER THAT IS INSIDE A TOOL CALL MUST NOT BE TYPED AT: it is not at rest, \
             so the turn a prompt opens there is not one this run can hold to a contract. Measured \
             cost (item 745): a live `claude` took 363 bytes into its composer, never turned the \
             Enter into a question, and the loop that met it spent a whole session recovering. \
             ⚠ The status-line clause that pane also showed is NOT what this arm is about — see \
             `a_background_shell_on_the_screen_is_not_a_child_this_door_holds_for`. Got {held:?}",
        );
        assert!(
            cost >= WITHIN,
            "and the premise has to hold inside the gate: this hold must actually have STOOD THERE \
             for the bound, or the answer above is about a wait that never happened. {cost:?} \
             against {WITHIN:?}",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default(),
            before,
            "⚠⚠⚠⚠⚠ AND NOT ONE BYTE REACHED THE PANE. The whole cost of this defect is text \
             sitting in a composer; a hold that refused and typed anyway would have bought nothing",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // ── ARM TWO: the identical peer with nothing running ──
        //
        // ⚠⚠⚠⚠⚠ ONE FIELD DIFFERENT. Without it the repair is passed by a door that never lets a
        // prompt through, which is a loop that does no work at all — and no arm above can see that.
        let (access, pane, _running) = peer_naming_a_tool(&peer("exec cat"));
        let started = Instant::now();
        let free = hold_while_a_child_runs(&access, &RunContext::uncancellable(), pane, WITHIN);
        let cost = started.elapsed();
        assert_eq!(
            free,
            Held::Free,
            "⚠⚠⚠⚠⚠ AND A PEER WITH NOTHING RUNNING IS TYPED AT AS IT ALWAYS WAS. This pane is the \
             one above with a single field cleared, so a build that holds here has not learned to \
             discriminate — it has stopped delivering. Got {free:?}",
        );
        assert!(
            cost < WITHIN,
            "and it must answer WITHOUT waiting: {cost:?} against a bound of {WITHIN:?}",
        );
        let delivered = deliver(
            &access,
            &RunContext::uncancellable(),
            pane,
            "hello",
            &Delivery::new().without_submitting(),
        )
        .expect("the pane takes text");
        assert!(
            delivered.is_on_screen(),
            "⚠⚠ AND THE PROMPT ACTUALLY LANDS, which is the half that says the free arm is a \
             DELIVERY and not merely a fast refusal. Got {delivered:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // ── ARM THREE: the tool ends while the door is still holding ──
        //
        // ⚠⚠⚠ THE ARM THAT MAKES ARM ONE MEAN SOMETHING. Arm one's `Still` is also what a door
        // whose predicate can never release would answer, and no assertion inside that arm can see
        // the difference. Here the hold is demonstrably live: the same peer, held by the tool
        // alone, is released the moment the agent stops naming it.
        let (access, pane, running) = peer_naming_a_tool(&peer("exec cat"));
        *running.lock().expect("the running mutex") = Some(TOOL.to_owned());
        let ending = Arc::clone(&running);
        let tool = std::thread::spawn(move || {
            std::thread::sleep(HELD);
            *ending.lock().expect("the running mutex") = None;
        });
        let started = Instant::now();
        let after = hold_while_a_child_runs(&access, &RunContext::uncancellable(), pane, WITHIN);
        let cost = started.elapsed();
        tool.join().expect("the tool thread");
        assert!(
            matches!(&after, Held::Ended { was, .. } if was == TOOL),
            "⚠⚠⚠ A CHILD THAT ENDS RELEASES THE DOOR, AND THE ANSWER NAMES IT. The tool the agent \
             was last seen running is what a person reading a held delivery needs. Got {after:?}",
        );
        assert!(
            (HELD..WITHIN).contains(&cost),
            "and the hold lasted the CHILD's life rather than the bound's: {cost:?} against a tool \
             held open for {HELD:?} inside a bound of {WITHIN:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔⛔ **A BACKGROUND SHELL ON THE SCREEN IS NOT A CHILD THIS DOOR HOLDS FOR** — register
    /// item 745's residue (A′), and a gate that holds a REFUTATION rather than a repair.
    ///
    /// # ⚠⚠⚠⚠⚠ What was prescribed, and what driving it found
    ///
    /// The register read this door's blind spot as a hole: the fact it consults is the tool named
    /// by a hook, which is retired at the turn's own `Stop`, so a shell the agent BACKGROUNDED is
    /// invisible to it — while the peer's status line says `1 shell` for as long as that shell runs.
    /// The prescription that followed was to give the door that clause as a second information
    /// source.
    ///
    /// **It was driven on 2026-08-29 and it does not hold.** Four deliveries into a live `claude`
    /// 2.1.251 with exactly one background shell running throughout, and every one of them was
    /// submitted and answered — including the last, which used the shape [`deliver`] itself uses
    /// (inject, wait for the echo, press the key as a SEPARATE write) at a size the composer folded
    /// into `[Pasted text #2]`. The whole table is on
    /// [`CLAUDE_FOOTER_BY_BACKGROUND_SHELLS`](crate::testing::CLAUDE_FOOTER_BY_BACKGROUND_SHELLS).
    ///
    /// So a door that read this clause would refuse deliveries that demonstrably land, and the cost
    /// of that is not a lost prompt: it is a whole `turn_within_ms` of standing there and then a
    /// [`PaneError::PeerBusy`](crate::access::PaneError::PeerBusy) that stops the run for a person
    /// — for a peer that was never busy.
    ///
    /// # ⚠⚠⚠ Why the refutation needs a GATE and not a paragraph
    ///
    /// The register's own sentence outlives the measurement that killed it. A later round reading
    /// *the status line says it and this door cannot see it* will reach for the same wire, and the
    /// only thing that can answer at the moment of the edit is a test. So the captured screens are
    /// staged on real panes and the claim is stated in both directions:
    ///
    /// * **THREE SCREENS, ONE ANSWER** — zero, one and two background shells, the same door, the
    ///   same [`Held::Free`], and a delivery that lands on each. Wiring the clause in turns the
    ///   second and third red while the first stays green.
    /// * **ONE SCREEN, TWO ANSWERS** — the `1 shell` pane again, this time with the agent naming a
    ///   tool: [`Held::Still`]. What decides this door is the agent's word, and the arm is here so
    ///   the gate cannot be passed by a door that has simply stopped holding at all.
    ///
    /// ⚠ The premises are asserted INSIDE the gate, both of them: that each capture names the count
    /// its row claims (so an edited fixture cannot quietly become three copies of one screen), and
    /// that the pane is really painting it (so a peer that died at `printf` cannot pass as a peer
    /// whose clause the door ignored).
    #[test]
    fn a_background_shell_on_the_screen_is_not_a_child_this_door_holds_for() {
        /// Wide enough for the captured status line to sit on ONE row — see [`access_sized`].
        const COLS: u16 = 100;
        /// The hold's bound. Every arm below must answer well inside it, except the last.
        const WITHIN: Duration = Duration::from_millis(400);
        /// The tool the last arm's agent names, as in the gate above: the agent's own word.
        const TOOL: &str = "Bash";

        for (shells, footer) in crate::testing::CLAUDE_FOOTER_BY_BACKGROUND_SHELLS {
            assert_eq!(
                footer.contains(" shell"),
                *shells > 0,
                "⚠⚠ THE CAPTURE MUST NAME WHAT ITS ROW CLAIMS. This table is three photographs of \
                 one status line, and a row whose clause disagrees with its count would stage the \
                 same screen three times while reading as three cases: {footer:?} against \
                 {shells} shell(s)",
            );
            if *shells > 0 {
                assert!(
                    footer.contains(&format!("{shells} shell")),
                    "and the peer's own plural is part of the capture — `1 shell`, `2 shells` — \
                     which is why no literal could ever have read this clause: {footer:?}",
                );
            }
            let (access, pane, _running) = peer_naming_a_tool_sized(
                &peer(&format!("printf '%s\\n' '{footer}'; exec cat")),
                COLS,
                8,
            );
            assert!(
                shows(&access, pane, footer, Duration::from_secs(10)),
                "⚠⚠⚠ AND THE PANE MUST ACTUALLY BE PAINTING IT. Without this the arm below is a \
                 door answering `Free` at a blank screen, which it would do for any reason at all",
            );
            let started = Instant::now();
            let free = hold_while_a_child_runs(&access, &RunContext::uncancellable(), pane, WITHIN);
            let cost = started.elapsed();
            assert_eq!(
                free,
                Held::Free,
                "⛔⛔⛔⛔⛔ A PEER WITH {shells} BACKGROUND SHELL(S) ON ITS STATUS LINE AND NO TOOL \
                 NAMED IS TYPED AT. Measured 2026-08-29: 48, 939 and 2,369 bytes all submitted and \
                 were answered at a live `claude` with this exact clause on the screen. A door \
                 that held here would stand for the whole bound and then stop the run for a person \
                 over a peer that was never busy. Got {free:?}",
            );
            assert!(
                cost < WITHIN,
                "and it must answer WITHOUT waiting, or the door is holding and merely releasing: \
                 {cost:?} against a bound of {WITHIN:?}",
            );
            let delivered = deliver(
                &access,
                &RunContext::uncancellable(),
                pane,
                "hello",
                &Delivery::new().without_submitting(),
            )
            .expect("the pane takes text");
            assert!(
                delivered.is_on_screen(),
                "⚠⚠ AND THE PROMPT LANDS, which is the half that makes `Free` a DELIVERY rather \
                 than a fast refusal. Got {delivered:?}",
            );
            access.lifecycle().expect("lifecycle").close(pane);
        }

        // ── THE OTHER DIRECTION: the same screen, and this time the agent names a tool ──
        //
        // ⚠⚠⚠⚠⚠ Without this the whole gate above is passed by a door that has stopped holding for
        // anything, which is register item 745's cause side deleted rather than corrected. What
        // decides here is the hook's fact, and the screen — identical to the arm two rows up — has
        // no vote either way.
        let (_, busy_footer) = crate::testing::CLAUDE_FOOTER_BY_BACKGROUND_SHELLS[1];
        let (access, pane, running) = peer_naming_a_tool_sized(
            &peer(&format!("printf '%s\\n' '{busy_footer}'; exec cat")),
            COLS,
            8,
        );
        *running.lock().expect("the running mutex") = Some(TOOL.to_owned());
        assert!(
            shows(&access, pane, busy_footer, Duration::from_secs(10)),
            "the same premise as above: this pane must be painting the clause, or the arm says \
             nothing about a screen at all",
        );
        let held = hold_while_a_child_runs(&access, &RunContext::uncancellable(), pane, WITHIN);
        assert_eq!(
            held,
            Held::Still {
                running: TOOL.to_owned(),
                within: WITHIN,
            },
            "⚠⚠⚠ AND THE HOLD IS STILL THERE. One screen, two answers, decided by the agent's own \
             word — which is the fact this door was built on and the one the refutation above does \
             not touch. Got {held:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }
}
