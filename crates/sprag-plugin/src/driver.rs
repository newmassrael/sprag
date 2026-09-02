//! `Driver` — the shared control substrate.
//!
//! Owns the SCE/SCXML statechart (`orchestration.scxml`: idle → running →
//! converged/exhausted/failed) and the guardrails, and runs any [`Plugin`] to a
//! terminal [`Outcome`]. Each microstep is one `Plugin::step`; the Driver
//! enforces the iteration, cost and DURATION budgets and maps the result onto
//! the statechart. The plugin owns the behaviour; the Driver owns the lifecycle.
//!
//! Two of the three ceilings are decided between steps, from counters the Driver
//! keeps. The third cannot be: a run out of time may be INSIDE a step that will
//! not return for minutes, so the deadline is armed into the [`RunContext`] and
//! every bounded wait a plugin makes consults it.
//!
//! [`RunContext`]: crate::run::RunContext

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sce_rust_runtime::Engine;
use sprag_terminal::{Reach, Stop, Unstopped};

use crate::access::{PaneAccess, PaneError, Signalled};
use crate::plugin::{Accounting, Checks, Cost, Deliveries, Plugin, Step, Verdict};
use crate::run::RunContext;
use crate::sm::orchestration::{OrchestrationEvent, OrchestrationPolicy, OrchestrationState};

/// The termination guardrails every plugin run is bounded by (first-class
/// safety per the README — an AI control loop must not run unbounded).
///
/// THREE independent ceilings, ANDed: whichever binds first ends the run, and
/// the outcome names which one it was ([`Ceiling`]). They are independent
/// because they answer three different questions a person asks about a loop —
/// *how many times?*, *how much?*, and *for how long?* — and no two of them can
/// stand in for each other. An iteration ceiling cannot bound a run whose single
/// step blocks; a cost ceiling cannot bound a run whose steps are free (a
/// print-mode dialogue reports `Tokens(0)` every turn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guardrails {
    /// Stop after this many steps.
    pub max_iterations: u32,
    /// Stop once the accumulated step cost reaches this bound. `None` leaves cost
    /// unbounded (the other two ceilings still apply). The
    /// bound's unit is the run's cost currency — every step the plugin reports
    /// shares it (see [`Cost`]) — so the Driver compares
    /// like with like and never sums bytes against tokens.
    pub max_cost: Option<Cost>,
    /// STOP ONCE THIS MUCH WALL-CLOCK TIME HAS PASSED since the run began.
    /// `None` leaves the run untimed.
    ///
    /// # ⚠⚠ Why the other two ceilings do not already cover this
    ///
    /// Both of them bound the run per STEP: `max_iterations` counts steps that
    /// COMPLETED, and `max_cost` sums what completed steps spent. Neither can see
    /// a step that is still running — so a run of 100 iterations against a peer
    /// that answers slowly is bounded at 100 × that peer's own reply timeout,
    /// a number nobody chose and no caller can read off the request. The only
    /// bound expressible in the units a person actually reasons in — *"not more
    /// than five minutes on this"* — was missing.
    ///
    /// ⚠ It is DISTINCT from the per-turn `timeout_ms` an `agent` or `dialogue`
    /// takes: that bounds ONE reply and then the loop takes another turn. This
    /// bounds the loop.
    ///
    /// The [`Driver`] arms it into the [`RunContext`] as an instant
    /// ([`RunContext::deadline_in`]), which is what carries it into the waits
    /// inside a step; a ceiling checked only between steps would be no ceiling
    /// at all for a run stuck inside one.
    ///
    /// # ⚠⚠⚠ A RUN MAY OUTLIVE THIS BY ONE ACCOUNT TURN, and that is stated rather than hidden
    ///
    /// When this passes the plugin is asked whether it can say where the run got to
    /// ([`Plugin::ask_for_an_account`]), and one that can is given a window of its own to do it in.
    /// So a run declaring five minutes may take five minutes plus that window — for `ai_loop`, TWO
    /// of its peer's declared turns, because the account cannot be asked until the turn in flight
    /// ends. Two alternatives were available and both are worse: ending in silence is the defect
    /// this answers, and carving the window OUT of this number would quietly spend work the caller
    /// asked for on a report they did not.
    ///
    /// ⚠ The window is finite, derived from a bound whoever set the plugin's per-turn ceiling
    /// already declared, and granted exactly once — see [`crate::plugin::Accounting`], which is
    /// where the reasoning about *whose number it is* lives.
    ///
    /// [`Plugin::ask_for_an_account`]: crate::plugin::Plugin::ask_for_an_account
    ///
    /// [`RunContext`]: crate::run::RunContext
    /// [`RunContext::deadline_in`]: crate::run::RunContext::deadline_in
    pub max_duration: Option<Duration>,
}

sprag_vt::closed_set! {
/// WHICH CEILING STOPPED A RUN — the reason an [`OutcomeState::Exhausted`] carries.
///
/// # ⚠⚠ Why an exhausted run that does not say which ceiling is barely an answer
///
/// `exhausted` alone tells a caller to change something without telling it WHAT. The three
/// ceilings have three different remedies — give it more turns, give it more budget, give it more
/// time — and with a third one added, guessing gets a third harder. Worse, a caller cannot even
/// infer it by comparing the counters against what it asked for: the ceilings it did not name came
/// from the DAEMON's defaults, so a run stopped by a default is stopped by a number the caller
/// never saw.
///
/// # ⚠ Why it names the CONCEPT and not the argument that sets it
///
/// `duration`, not `max_seconds`. The obvious alternative — answer the knob the caller would turn —
/// breaks on `Cost`, which is set by `max_bytes` on a byte-relay run and `max_tokens` on a
/// dialogue: one ceiling, two argument names, chosen by the plugin. A concept per ceiling is the
/// only naming that is one-to-one, and the `unit` key already beside it says which knob a `cost`
/// answer means.
///
/// # ⚠ Why this is deliberately NOT a published vocabulary
///
/// The other closed sets on this wire are ARGUMENT vocabularies: a client picks a word from them,
/// so publishing what is admissible is the difference between a call it can build and one it has
/// to guess. This is an ANSWER word, and no peer decodes it into a closed type — both renderers
/// take the string and print it. So a fourth ceiling is additive here in the strongest sense: an
/// old reader shows a word it has never seen and is not wrong. Giving it a `WIRE_WORDS` nobody
/// reads would be a constant the product does not enforce, which this project has removed twice.
///
/// # ⚠⚠⚠ Why it IS a `closed_set!`, where the paragraph above says it is not a vocabulary
///
/// Those are different claims. Nothing publishes an admissible LIST of these to a client — that is
/// still true, and is why there is no `WIRE_WORDS`. What a `closed_set!` buys is [`ALL`](Self::ALL),
/// and the reader that needs it is the DURABLE LOG: a run's ceiling is written out as its word and
/// read back by `outcome_from_words`, which matched two of the three by hand and answered
/// `Iterations` for anything else. The fourth arm added below would have come back from a restart
/// as *"you ran out of steps"* — a false sentence pointing at a guardrail the run never met.
/// **A list with no glob decides alone**, and this one decided against a variant that did not
/// exist when it was written.
///
/// # ⚠⚠⚠⚠⚠ A SIXTH ARM REDDENS A PIN — register item 545, and the second reader of [`ALL`](Self::ALL)
///
/// The two paragraphs above are why there is no `WIRE_WORDS` and they are unchanged. What was
/// wrong is what they were taken to settle: **no published list meant no pin, and these words went
/// out on the wire with nothing walking them for as long as the type existed.** `sprag-host`'s
/// `an_answers_value_space_cannot_widen_under_the_protocol_number` walks them now — through `ALL`
/// and [`wire_str`](Self::wire_str), not through a list published to anybody — so adding an arm
/// here is a RED there rather than a silence.
///
/// ⚠⚠⚠ It is worth knowing which break that pin is holding, because it is not serde's. Nothing
/// decodes this key whole; both renderers print the string. The reader that decodes it is
/// `outcome_from_words`, and an unknown word there is not a parse error but `Iterations` — the
/// false sentence the paragraph above describes, arriving from a restart where nobody is looking
/// for it. **The louder break is the one that gets found; this is the other kind.**
///
/// ⚠⚠ So the obligation an added arm inherits is stated once, here: say out loud why an older
/// reader cannot meet the new word, or move `WIRE_PROTOCOL`. What may not happen again is the
/// thing that happened to `hold` — reaching the wire while every gate stayed green and the pin
/// recorded its own blindness in a comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceiling {
    /// [`Guardrails::max_iterations`] — the run took all the steps it was allowed.
    Iterations,
    /// [`Guardrails::max_cost`] — the run spent all it was allowed, in its own unit.
    Cost,
    /// [`Guardrails::max_duration`] — the run ran out of wall-clock time.
    Duration,
    /// ⚠⚠ **THE PLUGIN'S OWN DECLARED BUDGET** — the one ceiling the [`Driver`] does not own.
    ///
    /// The three above are [`Guardrails`], which bound a run in the substrate's units: steps,
    /// spend, seconds. A plugin whose document counts something else has a fourth budget the
    /// Driver structurally cannot see — [`ai_loop.scxml`]'s `max_turns` counts the inner AGENT's
    /// turns, and one of those is many steps of the loop that drives it, so no iteration ceiling
    /// expresses *"give this agent eight turns"*.
    ///
    /// It reaches the Driver through [`Verdict::Exhausted`], which is why this is the same type
    /// rather than a second word beside it: a caller reading `ceiling` is asking *what do I raise
    /// to buy this run more room*, and the answer is one question whoever set the number can act
    /// on. The remedy differs (raise `max_turns` in the brief, not a guardrail), and the word says
    /// which knob it is exactly as `cost` does for a bound spelled `max_bytes` on one form and
    /// `max_tokens` on another.
    ///
    /// [`ai_loop.scxml`]: ../../ai_loop.scxml
    Turns,
    /// ⚠⚠⚠⚠ **HOW LONG A PERSON MAY HOLD A RUN** — [`ai_loop.scxml`]'s `hold_within_ms`, and the
    /// SECOND ceiling the [`Driver`] does not own. Register item 534.
    ///
    /// [`Turns`](Self::Turns) is a budget the plugin COUNTS; this is one it WAITS OUT, and the two
    /// are the same type for the reason written there: a caller reading `ceiling` is asking *what
    /// do I raise to buy this run more room*, and here the answer is a real knob — hold it longer,
    /// or come back sooner.
    ///
    /// ⚠⚠⚠ IT IS WHY THIS RUN'S ENDING IS `exhausted` AND NOT A SEVENTH OUTCOME WORD, on
    /// `peer_gone`'s test one layer up: does its reader DO something different? A held run's reader
    /// does what an exhausted run's reader does — decide whether to start it again, with a number
    /// raised or not. What they must NOT be told is `iterations`, which is what a held run reported
    /// before this variant existed: a step budget that would have bought it nothing (item 9,
    /// measured on run 18 — dead in 24 minutes under a document declaring an hour of patience).
    ///
    /// ⚠⚠ AND IT SURVIVES A RESTART, which is the payoff of being in this closed set rather than a
    /// typed pane on the verdict. The durable log writes the word and `outcome_from_words` reads it
    /// back, so a run abandoned before a daemon died still says so afterwards.
    Hold,
}
}

impl Ceiling {
    /// The ceiling a `word` names, or [`None`] for one no ceiling spells.
    ///
    /// ⚠ Derived from [`ALL`](Self::ALL) rather than matched by hand, which is the whole reason
    /// this type is a closed set — see the type's own doc.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }

    /// This ceiling's word on the wire — the ONE place the variant → name mapping lives, so the
    /// host never spells a `Ceiling` variant ([`Cost::unit`]'s rule, one level up).
    ///
    /// Exhaustive, so a ceiling added to the type cannot reach the wire without a word: there is no
    /// hand-written list for it to be left out of.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Iterations => "iterations",
            Self::Cost => "cost",
            Self::Duration => "duration",
            Self::Turns => "turns",
            Self::Hold => "hold",
        }
    }

    /// **WHETHER A RUN THIS CEILING ENDS IS ASKED WHERE IT GOT TO** — the class `Ceiling::Hold`
    /// created, made explicit rather than left for a reader to infer. Register item 534.
    ///
    /// # ⚠⚠⚠⚠ Why this is a method and not a comment
    ///
    /// Four of these five stop a run **mid-work**: the agent is at its pane, the budget ran out
    /// around it, and `ai_loop.scxml`'s `stopping` state asks it for an account before the run
    /// ends — which is what `stop_said` composes a per-ceiling sentence for (register item 264).
    /// [`Hold`](Self::Hold) cannot: the run reached its ending through the `orders` region with
    /// nobody at the pane, and typing a question at it would break the one promise a hold makes
    /// (`HOLD_TAKES_EFFECT`: *"nothing further is typed at the pane while it waits"*).
    ///
    /// ⚠⚠⚠ **AND THE RATCHET THAT READS `stop_said` NEEDED A POPULATION.** It asserts the document
    /// authors one clause per ceiling AND no others, so before this method existed a fifth ceiling
    /// left exactly two options: a clause nothing can ever select (the *"prose with no ceiling"*
    /// half of that same assertion), or a red nobody could fix without weakening the gate. The
    /// class is the answer, and putting it in the type means a SIXTH ceiling is asked the question
    /// instead of inheriting an answer.
    ///
    /// ⚠⚠ IT IS ALSO WHY THE MISTAKE CANNOT BE MADE QUIETLY IN EITHER DIRECTION: flip `Hold` to
    /// `true` and the ratchet demands a clause the document does not have; author that clause and
    /// the ratchet rejects a key no live ceiling selects.
    #[must_use]
    pub const fn asks_for_an_account(self) -> bool {
        match self {
            Self::Iterations | Self::Cost | Self::Duration | Self::Turns => true,
            Self::Hold => false,
        }
    }

    /// **WHAT A READER OF THE RUN SHOULD DO ABOUT IT** — which knob this ceiling is, in prose, and
    /// deliberately not the word itself so a caller that needs to match gets it from
    /// [`wire_str`](Self::wire_str).
    ///
    /// ⚠ Prose for a PERSON reading a journal, which is why it lives out here and not in any
    /// document: what a plugin's agent is told is that plugin's own authored business
    /// (`ai_loop.scxml`'s `stop_said`), and a driver that wrote sentences into somebody's prompt
    /// would be a second author. These are the two audiences and they get two texts.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Iterations => {
                "the run took every step it was allowed — raise `max_iterations` to buy it more"
            }
            Self::Cost => {
                "the run spent everything it was allowed, in its own unit — raise `max_cost` to buy \
                 it more"
            }
            Self::Duration => {
                "the run ran out of wall-clock time — raise `max_duration` to buy it more"
            }
            // ⚠ NAMES NO KNOB, alone among the four, and cannot: this ceiling is the PLUGIN's, so
            // the number lives in whatever that plugin was briefed with. Naming one plugin's
            // variable here would be the substrate guessing at documents nobody has written yet —
            // `Accounting::Cannot`'s reason, one type over.
            Self::Turns => {
                "the plugin's own declared budget is spent — no guardrail bounds it, so raising \
                 one buys nothing; the number is in the brief the plugin was given"
            }
            // ⚠⚠⚠ IT NAMES NO KNOB EITHER, for `Turns`' reason, AND IT NAMES A SECOND REMEDY THAT
            // IS NOT A NUMBER AT ALL. This is the only ceiling in the set that a person can spend
            // by doing nothing, so *come back sooner* is as much the answer as *raise it* — and a
            // sentence that offered only the number would tell somebody to configure their way out
            // of having walked away. ⚠ It says what the run DID NOT do, because that is the fact a
            // reader is most likely to get wrong here: no turn was taken, so nothing was lost to a
            // budget and the work is exactly where the hold found it.
            Self::Hold => {
                "somebody held this run and did not come back inside the plugin's own hold \
                 ceiling — no guardrail bounds it, so raising one buys nothing; hold it longer in \
                 the brief the plugin was given, or come back sooner. Nothing was typed at the \
                 pane while it waited and no turn was spent, so the work stands where the hold \
                 found it"
            }
        }
    }

    /// **THE WORD AND THE WHOLE SENTENCE**, for a reader who has only this one journal line.
    ///
    /// ⚠⚠ [`ReflectReason::noted`](crate::outer::ReflectReason::noted)'s reason, one state over: the
    /// step that walks `judge` into `stopping` answers [`Verdict::Continue`] — the machine is
    /// mid-run — so the LINE is the publication, and a reader scanning a walk for *which ceiling
    /// ended this* has nothing to scan for unless the word is in it.
    #[must_use]
    pub fn noted(self) -> String {
        format!("{}: {}", self.wire_str(), self.describe())
    }
}

// **THE WORDS THIS ANSWER CAN CARRY, PROJECTED FROM `Ceiling::ALL`** — register item 821.
//
// ⛔⛔⛔⛔⛔ WHY THIS IS NOT THE "ADMISSIBLE LIST" THE TYPE DECLINES TO PUBLISH.
//
// The value-space pin says, beside its own `Ceiling::ALL` chain, that this type *"deliberately
// publishes no admissible list, because no client picks a ceiling out of one, and a constant
// nothing enforces is what this project has removed twice."* Both halves of that stay true and
// neither is what this array is:
//
//   * **An admissible list answers *what may I SEND*.** Nothing here changes that — no request
//     argument takes a `Ceiling`, and `from_wire` still exists only to read a durable log back.
//     This answers the other question, *what may I be TOLD*, which a caller meets whether or not
//     it ever names a ceiling: `outcome_ceiling` puts one of these words in front of it.
//   * **"A constant nothing enforces" is a HAND-WRITTEN list.** This one is projected from `ALL`
//     by `wire_words!`, so a sixth ceiling joins it in the compile that adds the variant and
//     cannot be left out of it — the same mechanism the four vocabularies `sprag words` already
//     publishes are built by.
//
// ⚠ What it buys: `sprag_host::plugins::RUN_VOCABULARIES` is a `const` table of `&[&str]`, so a
// vocabulary with no such array could not be published at all. This was the one run answer a
// person could not ask this build about, and item 816's gate filed it as a debt rather than as a
// difference in kind — which is what brought somebody back to it.
//
// ⚠⚠ A LINE COMMENT and not a doc comment: `wire_words!` expands to an `impl` block, and rustc
// refuses to attach documentation to a macro invocation ("unused doc comment"). The words' own
// documentation is on `Ceiling` and on `wire_str`, where a reader looking for them will be.
sprag_vt::wire_words!(Ceiling: wire_str);

/// Which terminal statechart state a run reached.
///
/// ⚠ NOT `Copy` since [`Blocked`](Self::Blocked) carries the question — [`Verdict`]'s reason
/// exactly, one level up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutcomeState {
    /// A plugin step returned [`Verdict::Converged`].
    Converged,
    /// A GUARDRAIL stopped the run, and which one.
    ///
    /// The [`Ceiling`] is carried rather than reported beside it so that an exhausted outcome
    /// which does not say what exhausted it cannot be constructed — the same reasoning that put
    /// the unit inside [`Cost`] instead of next to it.
    Exhausted(Ceiling),
    /// A plugin step failed.
    Failed,
    /// The run was cancelled (the host raised the cancel signal).
    Cancelled,
    /// **THE PEER STOPPED TO ASK**, and the run ended rather than answering for somebody.
    ///
    /// Carries the question when this host can read it AND the reason nothing was answered — see
    /// [`Verdict::Blocked`], whose doc holds the reason this is a terminal outcome at all, and
    /// [`Unanswered`](crate::consent::Unanswered) for why the reason travels with it.
    ///
    /// ⚠ The `Option` is *does this record carry the detail at all*: a run RESTORED from a previous
    /// daemon's log has only its word (the host's `outcome_from_words`), and a re-published
    /// question would be a claim about a screen nobody has looked at since.
    ///
    /// # ⚠⚠ Why not a flavour of [`Failed`](Self::Failed)
    ///
    /// They are different instructions to whoever reads the run. A failed run wants something
    /// FIXED; this one wants an ANSWER, and one that is not the run's to give. R357 settled this
    /// shape for `interrupted`: reporting a run that did not finish as though it had would have
    /// been a lie, and *"the honest word is what costs the number"*.
    Blocked(Option<crate::consent::Unanswered>),
    /// **A PERSON TOOK THE PANE**, and the run stopped driving rather than typing over them.
    ///
    /// Carries how much they typed when this record holds the detail — see
    /// [`Verdict::TakenOver`] for why this is not a flavour of
    /// [`Blocked`](Self::Blocked), and [`Interruption`](crate::readiness::Interruption) for what it
    /// counts.
    ///
    /// ⚠ The [`Option`] is [`Blocked`](Self::Blocked)'s exactly: a run RESTORED from a previous
    /// daemon's log has only its word, and a re-published count would be a claim about writes
    /// nobody has watched since.
    ///
    /// # ⚠⚠ What this outcome does NOT say
    ///
    /// It does not say the run may resume, and nothing here decides when a person is finished.
    /// `ai_loop.scxml` has an edge back (`awaiting_human` → `resume`) that a supervisor takes by
    /// hand; automating it would need a measured answer to *"when has somebody stopped typing"*,
    /// which this round does not have and did not guess.
    TakenOver(Option<crate::readiness::Interruption>),
}

impl OutcomeState {
    /// This outcome's terminal word — the ONE place the variant → name mapping lives.
    ///
    /// # ⚠⚠ Why this belongs on the type and lived in the host for five rounds
    ///
    /// [`Cost::unit`], [`Ceiling::wire_str`] and [`Verdict::wire_str`] all follow this rule for the
    /// same stated reason — *the host never spells a variant* — and this, the outcome a run is
    /// actually reported under, was the one that did not. The host's `outcome_word` matched on the
    /// variants itself, so the mapping and the type could drift, and no list existed for a pin to
    /// walk. R365 found the second half of that (a fifth word reached the wire with every ratchet
    /// green) and filed the first half; this is the first half.
    ///
    /// ⚠ THE WORD IS UNCHANGED BY WHAT THE VARIANT CARRIES, deliberately: folding the ceiling into
    /// the word (`exhausted_duration`) would change the value space of a key old readers decode
    /// whole, which is a wire break no address or shape pin can see (R342). The ceiling travels
    /// beside it.
    #[must_use]
    pub const fn wire_str(&self) -> &'static str {
        match self {
            Self::Converged => "converged",
            Self::Exhausted(_) => "exhausted",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            // ⚠⚠ A WORD OF ITS OWN, not a flavour of `failed`. A blocked run wants an ANSWER and a
            // failed one wants a FIX — R357's rule for `interrupted`, and the same honesty test.
            Self::Blocked(_) => "blocked",
            // ⚠⚠ AND A SIXTH, on the same test: a taken-over run wants NOTHING done, where a
            // blocked one wants an answer and a failed one wants a fix.
            Self::TakenOver(_) => "taken_over",
        }
    }

    /// Every word this vocabulary publishes, so the answers pin walks a LIST rather than an array
    /// somebody has to remember to extend.
    ///
    /// ⚠ Hand-ordered, because these variants CARRY DATA and so have no `ALL` to project from —
    /// [`Verdict::WIRE_WORDS`](crate::plugin::Verdict::WIRE_WORDS)' residue, closed the same way:
    /// the gate beside this type constructs every variant and holds the list to
    /// [`wire_str`](Self::wire_str), so a sixth outcome cannot be published without a word or
    /// spelled without being published.
    pub const WIRE_WORDS: &'static [&'static str] = &[
        "converged",
        "exhausted",
        "failed",
        "cancelled",
        "blocked",
        "taken_over",
    ];
}

sprag_vt::closed_set! {
/// WHAT BECAME OF THE WORK a run had going, once the run was cut short.
///
/// # ⚠⚠⚠ Why a cancelled run that says only *"cancelled"* is half an answer
///
/// The two ceilings a run can be cut short by — a person's cancel and
/// [`max_duration`](Guardrails::max_duration) — both land while a step may be BLOCKED on a peer
/// this run set going. Ending the loop does not end that peer. So *"cancelled"* on its own is
/// consistent with two opposite states of the world: the work stopped, or the work is still
/// running and still spending. A caller cannot tell which, and the one they need to act on is the
/// second.
///
/// Each arm is a different thing to tell them, and two of the four say **the work is still
/// running** — which is why this is a closed set and not an `Option<Signalled>` whose `None` would
/// have meant all three of *nothing was running*, *this host cannot stop things*, and *the stop
/// failed*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The job the run had working was signalled, and this is what received it.
    Job(Signalled) = (Signalled {
        stop: Stop::Interrupt,
        pgid: 0,
        leader: None,
    }),
    /// The plugin had no pane's job of its own to stop — see
    /// [`Plugin::driving`](crate::plugin::Plugin::driving). A relay starts nothing, and a plugin
    /// that owns its pane outright has already closed it.
    Nothing,
    /// ⚠ THE WORK IS STILL RUNNING: the stop was attempted and did not land, and this is why.
    Unreached(PaneError) = (PaneError::NotStopped(Unstopped::Unseen)),
    /// ⚠ THE WORK IS STILL RUNNING: this host offers no way to stop a pane's job at all, so none
    /// was attempted.
    ///
    /// Distinct from [`Unreached`](Self::Unreached) because it is a fact about the DEPLOYMENT and
    /// not about this pane — the same distinction
    /// [`PaneDoing::Unknown`](crate::access::PaneDoing::Unknown) is a separate arm for.
    Unsupported,
}
}

impl std::fmt::Display for Stopped {
    /// ⚠ THE SENTENCE A CALLER READS beside their run's state. Each is a clause about the WORK,
    /// because the run's own fate is already published next to it and repeating it here would tell
    /// a reader nothing twice.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Job(signalled) => write!(f, "the run's own job was {signalled}"),
            Self::Nothing => f.write_str("the run had no job of its own running"),
            Self::Unreached(why) => write!(f, "the run's own job is still running: {why}"),
            // ⚠ The SAME phrase as the arm above, deliberately: the two reach a caller for
            // different reasons and leave them in the same position, and a reader scanning for
            // *is my work still going?* must not have to know two spellings of yes.
            Self::Unsupported => f.write_str(
                "the run's own job is still running for all anybody here can tell: this host \
                 cannot stop a pane's job",
            ),
        }
    }
}

/// The result of a plugin run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub state: OutcomeState,
    pub iterations: u32,
    /// The accumulated typed cost, or `None` if the run took no measured step
    /// (e.g. cancelled at the loop top before any step ran).
    pub cost: Option<Cost>,
    /// The cause when `state` is [`OutcomeState::Failed`]; `None` otherwise.
    pub failure: Option<PaneError>,
    /// WHAT BECAME OF THE WORK, for a run that was CUT SHORT — cancelled, or out of time.
    ///
    /// `None` for a run that ended on its own terms: a [`Converged`](OutcomeState::Converged) one
    /// reached its goal, a [`Failed`](OutcomeState::Failed) step never got to block on a peer, and
    /// the per-step ceilings ([`Ceiling::Iterations`], [`Ceiling::Cost`]) are decided BETWEEN steps
    /// — at which point the last step has returned and there is nothing in flight to stop. Only the
    /// two outside endings can land mid-step, and they are exactly the two this answers for.
    ///
    /// ⚠⚠ **A PER-STEP CEILING CAN NOW REACH THIS TOO, through the one door it did not have
    /// before**: a run given a window to account for itself
    /// ([`crate::plugin::Plugin::ask_for_an_account`]) is stepped
    /// again, and a window that runs out is a run cut short with a peer mid-answer. The rule is
    /// unchanged — *what became of the work, for a run that could still have had some in flight* —
    /// and the set of endings that satisfy it grew by one.
    ///
    /// ⚠ `None` ALSO for a run RESTORED from a previous daemon's log, which carries a run's summary
    /// and not its whole outcome — the same lossiness `failure` already has there, and harmless for
    /// the same reason: the daemon that had work running is the one that died.
    pub stopped: Option<Stopped>,
    /// HOW MANY OF ITS PEER'S QUESTIONS THIS RUN ANSWERED on the caller's consent — `0` for the
    /// runs that answered none, which is every run that was given no consent.
    ///
    /// ⚠ Reported for EVERY terminal state and not only the happy one. A run that answered a
    /// tool-permission dialog and then hit its iteration ceiling still answered it, and an outcome
    /// that mentioned the approval only on convergence would hide it in exactly the cases somebody
    /// is reading the outcome to understand.
    pub answered: u32,
    /// ⚠⚠⚠ **HOW MANY OF ITS PEER'S TOOL CALLS THIS RUN REFUSED AND REDIRECTED**, on a standing
    /// instruction the loop's author wrote — see [`ScreenRules`](crate::screen::ScreenRules).
    ///
    /// # ⚠⚠ Why this is not folded into [`answered`](Self::answered), when both count decisions
    ///
    /// They are opposite decisions, and the question a person reads a tally to ask is *"what did my
    /// run let it do?"*. `answered` takes an offered option — an approval, usually — and this one
    /// **turns a call down**: measured, the key it presses makes the agent report `User rejected`
    /// and the file is never written. One number covering both would answer that question with a
    /// count that includes every refusal.
    pub screened: u32,
    /// 🎯🎯🎯🎯🎯 **HOW MANY NEXT CHECKPOINTS THIS RUN'S AGENT NAMED AND THIS RUN COUNTED RATHER
    /// THAN TOOK** — the owner's decision of 2026-09-02, register item 833(2). See
    /// [`Plugin::deferred`], where the argument lives.
    ///
    /// ⚠ Reported for EVERY terminal state, on [`answered`](Self::answered)'s argument: a run that
    /// set three proposals aside and then hit its iteration ceiling is exactly the run somebody
    /// reads an outcome to understand, and a tally present only on convergence would be missing
    /// from the endings that most need explaining.
    ///
    /// ⚠⚠ [`None`] is *this plugin does not set proposals aside* — every bundled plugin but the
    /// loop, and a loop whose datamodel has stopped answering. Never `Some(0)`, which is *it had
    /// the choice and never had to make it*.
    pub deferred: Option<u32>,
    /// 🎯🎯🎯🎯🎯 **HOW MANY TIMES THIS RUN CHANGED DIRECTION WITH NOBODY CHECKING** — the owner's
    /// decision of 2026-09-03, register item 847. See [`Plugin::unchecked`], where the argument is.
    ///
    /// ⚠ Reported for EVERY terminal state on [`deferred`](Self::deferred)'s argument, and it
    /// matters more here than there: a run that ended anyhow having re-aimed itself four times on
    /// nobody's say-so is precisely the run whose account must not read like a bounded one.
    ///
    /// ⚠⚠ [`None`] is *this plugin does not re-aim itself*; `Some(0)` is *every direction it took
    /// was checked*, which is a real and different claim.
    pub unchecked: Option<u32>,
    /// ⚠⚠⚠ **WHAT THE RUN PUT INTO ITS PANE AND HOW MUCH OF IT WAS NEVER VISIBLE THERE** —
    /// register item 591, [`Progress::deliveries`] read at the end.
    ///
    /// ⚠ Reported for EVERY terminal state, on [`answered`](Self::answered)'s argument: a run whose
    /// every prompt was folded away and which then hit its iteration ceiling is precisely the run
    /// somebody is reading the outcome to understand, and a tally present only on convergence would
    /// be absent in exactly those cases.
    pub deliveries: Deliveries,
    /// ⚠⚠⚠ **WHETHER ANYTHING INDEPENDENT VERIFIED THE MILESTONES THIS RUN CONVERGED ON** —
    /// register item 601, [`Progress::checks`] read at the end.
    ///
    /// ⚠ Reported for EVERY terminal state on [`answered`](Self::answered)'s argument, and here it
    /// matters most on the HAPPY one: a `converged` whose every check was silent is exactly the
    /// ending somebody would otherwise take at face value.
    pub checks: Checks,
    /// ⚠⚠⚠⚠⚠ **HOW MUCH OF THIS RUN'S WORK IS COMPLETE AND KEPT, WHATEVER WORD IT ENDED WITH** —
    /// register item 604. See [`crate::plugin::Banked`].
    ///
    /// It exists because a reader of an ending could not tell *the work is safe* from *the work is
    /// gone*, and the sentence `sprag stand-down` prints asserted the second of every ending that
    /// was not a convergence. Measured 2026-08-23: an agent that finished a turn and then exited —
    /// the ordinary case — had its person told the turn was lost.
    ///
    /// ⚠⚠ [`None`] means the plugin does not count completed work, which is a different answer
    /// from `Some(Banked { completed: 0, .. })` and has to stay different: one is *nothing was
    /// finished* and the other is *nobody was counting*.
    pub banked: Option<crate::plugin::Banked>,
    /// ⚠⚠⚠⚠⚠ **HOW BIG THE BRIEF THIS RUN WAS STARTED WITH IS** — register item 719's second
    /// direction, and [`None`] for a plugin nobody briefs.
    ///
    /// It is on the ENDING as well as on [`Progress::briefed`] because the question it answers is
    /// asked most often about a run that is already over: *why did that one churn?* — and item 719's
    /// own diagnosis had to be written by hand, after the fact, because nothing on the row said what
    /// the run had been handed.
    pub briefed: Option<crate::Briefing>,
    /// ⛔⛔⛔⛔⛔ **WHY THIS RUN ENDED ON ITS OWN TERMS, AS ONE WORD** — register item 706's third
    /// requirement. See [`crate::plugin::Plugin::ended_because`], which is where the word comes
    /// from and where the argument for its being a word lives.
    ///
    /// ⚠⚠⚠ **IT ANSWERS A QUESTION [`state`](Self::state) CANNOT.** All three endings a loop closes
    /// under are [`OutcomeState::Converged`], so a reader asking *did the stand-down I gave land?*
    /// got one word for three different facts — register item 594's cost — and until this field the
    /// word existed only inside a walk note's prose, where every consumer had to parse for it.
    ///
    /// ⚠⚠ [`None`] is **the plugin named no ending**: every run that did not end on its own terms
    /// (cancelled, failed, out of iterations) and every plugin that has no endings to name. Never
    /// *it ended for no reason* — [`banked`](Self::banked)'s distinction, and the reason the wire
    /// omits the key rather than publishing `null`.
    ///
    /// ⚠ A [`Cow`](std::borrow::Cow) for [`crate::plugin::Banked::unit`]'s reason, and the case it
    /// documents is exactly this one: a LIVE plugin names an ending out of a closed set it already
    /// holds and pays nothing, and only a word read back from a daemon's durable log arrives owned.
    pub done_reason: Option<std::borrow::Cow<'static, str>>,
}

/// Runs a [`Plugin`] over a [`PaneAccess`] to a terminal [`Outcome`], owning the
/// statechart + guardrail counters.
pub struct Driver {
    engine: Engine<OrchestrationPolicy>,
    guardrails: Guardrails,
    iterations: u32,
    cost: Option<Cost>,
    failure: Option<PaneError>,
    progress: Option<ProgressCell>,
    /// **WHERE THIS RUN'S PROGRESS GOES WHEN THE READER IS NOT IN THIS PROCESS**, or [`None`] for
    /// every run whose watcher shares [`progress`](Self::progress) above.
    ///
    /// # ⚠⚠⚠ Why a cell was not enough, when [`ProgressCell`]'s own doc argues for one
    ///
    /// That argument is about a READER: progress is a level, so a watcher that looks twice and sees
    /// the same numbers has learned something, and a missed edge costs it nothing. It holds. What it
    /// assumes is that the reader can SEE the cell — and a run driven by a process of its own
    /// (sprag's register items 544 / 650) has a reader on the other side of a socket, for whom a
    /// level in this process's memory is not a level at all.
    ///
    /// So this does not replace the cell; it carries the same value to somebody who cannot read it.
    /// A forwarder that drops a call loses nothing for the same reason a missed edge costs a local
    /// watcher nothing: the next publish carries the level.
    forward: Option<ProgressSink>,
    /// What each step did, bounded to the last [`JOURNAL_LIMIT`].
    journal: Vec<StepRecord>,
    /// **WHERE THE PLUGIN'S OWN MACHINE IS** — [`Plugin::at`]'s last answer, asked once per
    /// completed step. [`None`] before the first step and for a plugin that walks no statechart.
    at: Option<&'static str>,
    /// **WHETHER A PERSON IS BEING WAITED ON** — [`Plugin::waiting_for_a_person`]'s last answer,
    /// asked beside [`at`](Self::at) and for the same steps. Register item 755.
    waiting: Option<String>,
    /// **THE WHOLE PLACE THAT MACHINE IS IN** — [`Plugin::place`]'s last answer, asked beside
    /// [`at`](Self::at) and for the same steps. Register item 543.
    place: Option<Vec<String>>,
    /// WHETHER THE RUN WAS CUT SHORT — ended by a cancel or a passed deadline rather than by its
    /// own logic. [`Driver::ended_from_outside`] is the only writer, so this cannot disagree with
    /// the decision that was actually taken.
    ///
    /// ⚠ Recorded rather than re-derived from the terminal state, and the difference is real: a
    /// cancel raised in the same instant a plugin converged leaves the statechart in `converged`,
    /// and a re-derivation would then conclude nothing was cut short while a step had in fact been
    /// pre-empted. The decision is the fact; the state is its consequence.
    cut_short: bool,
    /// What became of the work, once [`Driver::stop_the_work`] has answered. `None` for a run that
    /// ended on its own terms and never asked.
    stopped: Option<Stopped>,
    /// Which ceiling raised [`OrchestrationEvent::Exhaust`], recorded AT the raise.
    ///
    /// Recorded rather than re-derived at the end, because a deadline that passed one instant
    /// after the decision would make a re-derivation disagree with the decision that was actually
    /// taken. [`Driver::exhaust`] is the only writer, so the statechart cannot reach its
    /// `exhausted` state without one.
    exhausted_by: Option<Ceiling>,
    /// WHAT THE PEER WAS ASKING when a step returned [`Verdict::Blocked`], recorded at the moment
    /// the verdict is read.
    ///
    /// Recorded rather than re-read at the end for [`exhausted_by`](Self::exhausted_by)'s reason
    /// and one more: the question was read off a PANE, and by the time the outcome is assembled
    /// that pane may have been answered by a person, scrolled, or closed. The outcome must report
    /// what stopped the run, not what the screen says afterwards.
    ///
    /// ⚠ The `Option` is *was there a blocked verdict at all*. Whether the QUESTION could be read
    /// is [`Unanswered`](crate::consent::Unanswered)'s own business, and it carries the reason
    /// beside it — which is a real answer with its own remedy rather than a second `Option`.
    asking: Option<crate::consent::Unanswered>,
    /// WHAT A PERSON DID TO THE PANE, when a step reported they had taken it — recorded at the
    /// verdict for [`asking`](Self::asking)'s reason exactly: the count is a delta against a
    /// watermark this run armed, and nothing at the end of the run can re-derive it.
    taken_over: Option<crate::readiness::Interruption>,
    /// HOW MANY OF ITS PEER'S QUESTIONS THIS RUN ANSWERED, on the caller's consent.
    ///
    /// # ⚠⚠ Why a run that answered has to say so in its OUTCOME and not only in its journal
    ///
    /// The journal keeps the last [`JOURNAL_LIMIT`] steps, so an approval given on step 3 of a
    /// three-hundred-step run is not in it. A tally is unbounded and cheap, and it is the
    /// difference between *"this run answered nothing"* and *"this run answered something you can
    /// no longer see"* — which for an act taken on a person's behalf is the one distinction that
    /// must survive.
    ///
    /// ⚠ A COUNT and not a list, deliberately: a list grows with the run, which is the property
    /// [`Step::note`](crate::plugin::Step::note) is capped for. The count says how many to go
    /// looking for; the journal says what they were, for as far back as it reaches.
    answered: u32,
    /// HOW MANY OF ITS PEER'S TOOL CALLS THIS RUN REFUSED AND REDIRECTED, on the loop author's
    /// standing instructions — [`Self::answered`]'s argument, for the other decision.
    screened: u32,
    /// 🎯 **WHAT THE PLUGIN LAST SAID ITS SET-ASIDE PROPOSALS CAME TO** — register item 833(2),
    /// held for [`deliveries`](Self::deliveries)' reason and read from the plugin at the same one
    /// place.
    ///
    /// ⚠ NEVER incremented here. The rule that sets a proposal aside is a guard in the plugin's own
    /// document, so a second counter out here would be a number that agrees until the day it does
    /// not — `deliveries`' own words, one field down.
    deferred: Option<u32>,
    /// 🎯 **WHAT THE PLUGIN LAST SAID ITS UNCHECKED RE-AIMS CAME TO** — register item 847, held on
    /// the line above's terms and read from the plugin at the same one place. ⚠ NEVER incremented
    /// here, for that field's reason exactly.
    unchecked: Option<u32>,
    /// ⚠⚠⚠ **WHAT THE PLUGIN LAST SAID ITS DELIVERIES CAME TO** — register item 591, held for
    /// [`at`](Self::at)'s reason and read from the plugin at the same one place.
    ///
    /// ⚠ NEVER incremented here. The Driver records the plugin's own totals and adds nothing: the
    /// evidence a delivery was accepted on is visible only inside the thing that delivered, so a
    /// second counter out here would be a number that agrees until the day it does not.
    deliveries: Deliveries,
    /// ⚠⚠⚠ **WHAT THE PLUGIN LAST SAID ITS INDEPENDENT CHECKS CAME TO** — register item 601, held
    /// and read exactly as [`deliveries`](Self::deliveries) is, and never added to here.
    checks: Checks,
    /// ⚠⚠⚠ **WHAT THE PLUGIN LAST SAID ITS COMPLETED WORK CAME TO** — register item 604, held and
    /// read exactly as the two totals above are, and never added to here.
    ///
    /// ⚠⚠ Kept as the last answer that EXISTED rather than the last answer given: a plugin whose
    /// machine has reached a final state may stop being able to count, and a run's ending is
    /// exactly when the sentence a person reads is composed.
    banked: Option<crate::plugin::Banked>,
    /// ⚠⚠⚠ **HOW BIG THE BRIEF THIS RUN WAS STARTED WITH IS** — register item 719's second
    /// direction, held and read exactly as the totals above are.
    ///
    /// ⚠⚠ It is the one of these that does NOT move: a brief is taken once, at the door. It is
    /// asked per step anyway, on the rule the three above it are asked under — *one place a step
    /// reports what a run is* — because a constant fetched by a second road is a constant two
    /// roads can disagree about, and the run that would expose the disagreement is the one nobody
    /// is watching.
    briefed: Option<crate::Briefing>,
    /// ⛔⛔⛔ **WHICH ENDING THE PLUGIN LAST NAMED** — register item 706's third requirement, held
    /// and read exactly as the totals above are, and never invented here.
    ///
    /// ⚠⚠ Kept as the last answer that EXISTED, on [`banked`](Self::banked)'s rule and for a
    /// sharper reason: the ending is named by the very step that reaches the machine's final
    /// state, and a plugin asked again afterwards may have no datamodel left to read it out of.
    /// A read that replaced an answer with [`None`] would lose the one word this field is for, at
    /// the exact moment it becomes true.
    done_reason: Option<&'static str>,
    /// ⚠⚠⚠ **WHICH PANE THE PLUGIN LAST SAID IT WAS DRIVING** — register items 540 and 595, held
    /// and read exactly as the two totals above are.
    driving: Option<sprag_terminal::PaneId>,
    /// ⚠⚠⚠ **WHETHER THE RUN IS SPENDING ITS LAST MOMENTS SAYING WHERE IT GOT TO** — set once, by
    /// the single site that asks a plugin for an account, and never cleared.
    ///
    /// While it is set the run is OVER in every sense that matters to a reader: which ceiling
    /// stopped it is already recorded, no further ceiling is consulted, and whatever the plugin
    /// decides in this window can add an ACCOUNT but never change the ending — see
    /// [`only_an_account`](Self::only_an_account).
    winding: bool,
}

/// WHAT A RUN HAS SPENT SO FAR — the counters the [`Driver`] keeps, readable while it is still
/// keeping them.
///
/// # ⚠⚠ Why a running run reporting nothing was a defect and not an omission
///
/// The driver counts `iterations` and accumulates [`Cost`] from the first step, and published both
/// ONLY in the terminal [`Outcome`]. So a client watching a long run could not tell PROGRESS from
/// STUCK, and **could not see spend until the spending was over** — a strange property for a
/// feature whose selling point is a typed cost ceiling. The counters existed; they were simply not
/// readable mid-flight.
///
/// The same two facts under the same names as `Outcome`'s, deliberately: a reader that polls this
/// and then reads the outcome meets one vocabulary, and the last progress a run reports agrees with
/// the outcome it ends on.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    /// How many steps have completed. Zero before the first one returns.
    pub iterations: u32,
    /// What has been spent, in the run's own unit — [`None`] until the first step establishes it.
    pub cost: Option<Cost>,
    /// THE LAST [`JOURNAL_LIMIT`] STEPS, oldest first.
    ///
    /// See [`StepRecord`] for why a run that reports only its total is not diagnosable.
    pub journal: Vec<StepRecord>,
    /// HOW MANY OF ITS PEER'S QUESTIONS THIS RUN HAS ANSWERED SO FAR, on the caller's consent —
    /// [`Outcome::answered`] read mid-flight, under the same name for this type's stated reason.
    ///
    /// ⚠⚠ **A RUNNING RUN IS EXACTLY WHERE THIS MATTERS MOST.** The other two counters are watched
    /// to tell progress from stuck; this one is watched to see a decision being taken on your
    /// behalf while there is still time to stop it. Available only in the outcome, it would reach
    /// the reader after every approval had already been given.
    pub answered: u32,
    /// HOW MANY OF ITS PEER'S TOOL CALLS THIS RUN HAS REFUSED AND REDIRECTED SO FAR —
    /// [`Outcome::screened`] read mid-flight, under the same name for this type's stated reason.
    ///
    /// ⚠⚠ Watched for `answered`'s reason and with more urgency: a run refusing calls in a cycle is
    /// one whose standing rule is too broad, and this count is the only thing that says so while
    /// there is still time to stop it.
    pub screened: u32,
    /// 🎯🎯🎯🎯🎯 **HOW MANY PROPOSALS THIS RUN HAS SET ASIDE AT ITS RE-AIMING CAP, SO FAR** —
    /// register item 833(2).
    ///
    /// ⚠⚠ Watched WHILE THE RUN IS GOING for `screened`'s reason exactly: a run deferring proposal
    /// after proposal is one whose cap is too tight for the work it was pointed at, and the person
    /// who can widen it is the one reading this row now. An ending-only tally answers that question
    /// after the chance to act on it has gone.
    ///
    /// ⚠ [`None`] is *nobody was counting* — see [`Outcome::deferred`].
    pub deferred: Option<u32>,
    /// 🎯 **AND HOW MANY OF ITS DIRECTIONS NOBODY CHECKED** — register item 847, published WHILE
    /// THE RUN IS STILL GOING on the line above's argument: a person watching a loop re-aim itself
    /// unchecked is the one who can still go and name a classifier for it.
    pub unchecked: Option<u32>,
    /// ⚠⚠⚠⚠⚠ **WHERE THE RUN IS** — the plugin's own machine position, from [`Plugin::at`].
    ///
    /// # The fact that existed only as prose — register item 543
    ///
    /// Every other field here is a NUMBER, and numbers answer *how much* while nobody could ask
    /// *where*. The loop does write its position down — `working --judged--> judging` — but into a
    /// step's [`note`](crate::plugin::Step::note), which is **a human sentence inside a journal
    /// bounded to the last [`JOURNAL_LIMIT`] steps and not persisted at all**. So the answer was
    /// unreadable by any program, truncated for a long run, and gone at the next restart, which is
    /// exactly when it is wanted: a run that comes back `Interrupted` could say how far it got and
    /// never where it stopped.
    ///
    /// ⚠⚠ **A LEVEL, like everything else in this cell** ([`ProgressCell`]'s own rule): the CURRENT
    /// position, not a history. A reader that looks twice and sees the same word has learned that
    /// the run has not moved — which is what *"progress or stuck?"* actually asks, and which the
    /// counters answer only for a run that is spending something.
    ///
    /// ⚠ The words belong to a document, so anything that RECORDS one must record
    /// [`STATECHARTS_FINGERPRINT`](crate::STATECHARTS_FINGERPRINT) beside it. `None` before the
    /// first step completes, and for a plugin that walks no statechart.
    pub at: Option<&'static str>,
    /// ⛔⛔⛔⛔⛔ **WHETHER THIS RUN IS WAITING FOR A PERSON RIGHT NOW, AND WHY** —
    /// [`crate::Plugin::waiting_for_a_person`], register item 755. `None` for a run nobody is
    /// waiting on, and before the first step.
    ///
    /// # ⚠⚠⚠⚠ Why it is a LEVEL here and not a journal line
    ///
    /// Every other channel a run has reports TRANSITIONS: the journal records steps, and a state a
    /// machine STAYS IN writes no line while it stands. Measured 2026-08-29 — a run sat waiting for
    /// sixty-two minutes while `sprag runs` read `running`, the driver stayed alive so no kernel
    /// wait fired, and the walk only recorded `AwaitingHuman` after the wait had already expired.
    /// **A persisting fact has to be readable as a level**, which is what this field is and what
    /// `iterations` beside it already is for a different question.
    ///
    /// ⚠ A SENTENCE rather than a state word: the plugin knows which of its states mean this and
    /// why, and a reader is told what to DO. The document owns the words (`needs_a_person`), the
    /// plugin composes the sentence, and this carries it — nobody in between reads it.
    pub waiting: Option<String>,
    /// **THE WHOLE PLACE THE MACHINE IS IN**, in the document's own words — [`crate::Plugin::place`],
    /// register item 543. `None` for a plugin that walks no statechart and before the first step.
    ///
    /// ⚠ It travels beside [`at`](Self::at) rather than replacing it: that one answers a person and
    /// this one is what `Engine::enter_at` can be handed. Anything that RECORDS it must record
    /// [`STATECHARTS_FINGERPRINT`](crate::STATECHARTS_FINGERPRINT) beside it, `at`'s own rule —
    /// more so, because a configuration read against a foreign document decodes cleanly and lies.
    pub place: Option<Vec<String>>,
    /// ⚠⚠⚠⚠⚠ **WHAT THIS RUN HAS PUT INTO ITS PANE AND HOW MUCH OF IT NOBODY CAN SEE THERE** —
    /// [`crate::plugin::Plugin::deliveries`], register item 591.
    ///
    /// ⚠⚠ **A LEVEL, like everything else in this cell**, and that is the whole reason it is here.
    /// The same fact reaches a reader through the walk as a CHANGE — `crate::outer::Told` publishes
    /// the evidence once and again only when the road moves — so *are my prompts visible on that
    /// pane?* was answerable only by having watched from the start. A person who comes back to a
    /// running loop reads a level or reads nothing.
    ///
    /// ⚠ [`Deliveries::NONE`] for a plugin that delivers no composed prompt, which is three of the
    /// four bundled ones; see that constant for why the absence is a claim rather than a default.
    pub deliveries: Deliveries,
    /// ⚠⚠⚠⚠⚠ **WHICH PANE THIS RUN IS DRIVING RIGHT NOW** — [`Plugin::driving`], register item 540.
    ///
    /// # The fact that was published only as prose inside a name
    ///
    /// It was never absent — a run's LABEL reads `ai_loop pane=3`, and every reader that wanted the
    /// pane had to parse a human sentence to get it. That is R431's *derive it from a name* one
    /// surface over, and the label is free to change wording at any time.
    ///
    /// ⚠⚠ **AND IT IS THE OTHER HALF OF A QUESTION NOBODY COULD ASK** — register item 595. A daemon
    /// restart re-runs an allowlisted agent's argv, so a `claude` pane comes back **holding its old
    /// conversation and driven by nothing**, and a person looking at the screen cannot tell that
    /// from a loop that is working: both are a `claude` prompt. Answering *which pane is a run
    /// driving* is what lets the other surface answer *is anybody driving this pane*.
    ///
    /// ⚠ [`None`] while a run has completed no step, and for a plugin driving no pane of its own —
    /// see [`Plugin::driving`]'s own doc for why that is a real answer rather than an omission.
    pub driving: Option<sprag_terminal::PaneId>,
    /// ⚠⚠⚠⚠ **WHETHER ANYTHING INDEPENDENT VERIFIED THIS RUN'S MILESTONES** —
    /// [`crate::plugin::Plugin::checks`], register item 601.
    ///
    /// ⚠⚠ **A LEVEL, and the third fact to need one.** *Checked* and *the checker never started*
    /// were the same `converged` in a run's answer, because the verdict reached the WALK and
    /// nothing else — items 591 and 594 are the same shape, and the pattern is written out in
    /// [`Checks`]' own doc.
    pub checks: Checks,
    /// ⚠⚠⚠⚠⚠ **HOW MUCH OF THIS RUN'S WORK IS COMPLETE AND KEPT SO FAR** — register item 604's
    /// fact, published as a LEVEL beside the three above and for their reason.
    ///
    /// ⚠⚠ It is here as well as on [`Outcome::banked`] because *is my work safe?* is a question
    /// about a RUNNING run too — a person watching a long loop and reaching for `sprag stand-down`
    /// is asking exactly it — and because this is the cell the daemon's run log is written from
    /// (register item 616: the answer has to survive the restart, since a run is read after it
    /// ends and its daemon is usually gone by then).
    pub banked: Option<crate::plugin::Banked>,
    /// ⚠⚠⚠⚠⚠ **HOW BIG THE BRIEF THIS RUN WAS STARTED WITH IS** — register item 719's second
    /// direction, published as a LEVEL beside the four above and for their reason.
    ///
    /// ⚠⚠ **IT HAS TO BE A LEVEL AND NOT A LINE IN THE WALK**, which is the whole reason it is
    /// here rather than in a step's note. The journal is BOUNDED ([`JOURNAL_LIMIT`]) and a brief is
    /// said once, at the start — so on exactly the runs this exists for, the long ones that churn,
    /// the line would have been evicted by the time anybody read the row. [`Progress::deliveries`]
    /// exists for the same reason, and says so in its own words: a supervisor who arrives mid-run
    /// can only read a level.
    pub briefed: Option<crate::Briefing>,
    /// ⛔⛔⛔⛔⛔ **THESE COUNTERS ARE A PREDECESSOR'S AND NO DRIVER HAS WRITTEN THEM SINCE** —
    /// register item 815, and the only field here that is about the CELL rather than about a run.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the counters need a word for whose they are
    ///
    /// A daemon that reads a predecessor's run log fills this cell from the FILE — deliberately, so
    /// a person meets what the run managed rather than a row of zeros (register items 606 and 616).
    /// A boot may then put that run back on a driver of its own, and until that driver's first step
    /// **every number here is the dead daemon's**, with nothing saying so.
    ///
    /// **Measured 2026-09-01**: a rescued run whose pane came back a plain shell published
    /// `running — 2 iterations` and `1 prompt(s) delivered, all of them on that pane` while its new
    /// driver had taken no step and typed nothing. Item 774's clause — *this run came back and has
    /// typed nothing since* — reads the ABSENCE of a delivery count as its evidence, so a stale
    /// count silences it on exactly the run it was built for.
    ///
    /// ⚠⚠ **IT CLEARS ITSELF, WHICH IS THE WHOLE REASON IT LIVES IN THIS STRUCT.** [`Driver`]
    /// republishes the cell WHOLE on every step from fields of its own, so the first step a driver
    /// takes overwrites this with `false` by construction — there is no clearing site anybody could
    /// forget, and no second authority on *has this driver spoken yet*.
    ///
    /// ⚠ FALSE for every run a driver started, which is nearly every run ever: only
    /// `sprag_host::runs::RunRegistry::restore` sets it, and only for the rows it builds out of a
    /// file.
    pub inherited: bool,
}

/// HOW MANY STEPS A RUN REMEMBERS.
///
/// A bound rather than the whole history, because a run may take as many steps as its iteration
/// ceiling allows and this is held in memory for the life of the daemon. The LAST ones are kept
/// because the question a journal is read to answer — *why did it not converge?* — is asked about
/// the end of a run. ⚠ The TOTAL is never lost: `iterations` counts every step, so a reader can
/// always tell a truncated journal from a complete one by comparing the two.
pub const JOURNAL_LIMIT: usize = 64;

/// WHAT ONE STEP DID — one entry of a run's journal.
///
/// # ⚠⚠ Why a run's total was not enough
///
/// A finished run said how many steps it took, what it spent, and which ceiling stopped it. For the
/// one question a bounded loop is actually debugged with — *what happened in there?* — it said
/// nothing at all, so `exhausted after 100 iterations` was the whole account of a hundred acts on
/// somebody's pane. The counters existed per step and were summed away.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepRecord {
    /// Which step this was, counting from one.
    pub iteration: u32,
    /// What this step alone spent (not the running total).
    pub cost: Cost,
    /// What it decided.
    pub verdict: Verdict,
    /// The plugin's own line about it, if it had one — see [`Step::note`](crate::plugin::Step::note).
    pub note: Option<String>,
    /// **EVERY TRANSITION THIS STEP'S MACHINE TOOK** — see [`crate::plugin::Plugin::walked`].
    ///
    /// ⚠⚠⚠ The journal is where a run is DEBUGGED, and until 2026-08-23 the only account of its
    /// path was [`note`](Self::note) — prose, one edge per step whatever the step actually did.
    /// Register item 611 wrote the rule and item 605 paid for it: four rounds spent on a guard at
    /// a state the machine had already left, because the sentence said otherwise.
    pub walked: Vec<crate::plugin::Edge>,
}

/// Where a [`Driver`] publishes its [`Progress`], shared with whoever is watching.
///
/// A cell rather than a callback, on the same reasoning `runs` is a slot rather than a stream: this
/// is a LEVEL. A reader that looks twice and sees the same numbers has learned that nothing moved,
/// which is exactly the question "progress or stuck?" asks — and a missed edge would cost it
/// nothing.
pub type ProgressCell = Arc<Mutex<Progress>>;

/// **WHERE A RUN'S PROGRESS GOES WHEN THE WATCHER CANNOT READ A CELL** — see
/// [`Driver::forwarding_to`].
///
/// A sibling of [`ProgressCell`] rather than a replacement: that one is right for a watcher sharing
/// this process's memory, and this one is what a watcher across a socket needs. Both carry the same
/// value from the same call.
pub type ProgressSink = Arc<dyn Fn(&Progress) + Send + Sync>;

impl Driver {
    /// A driver bounded by `guardrails`.
    #[must_use]
    pub fn new(guardrails: Guardrails) -> Self {
        Self {
            engine: Engine::new(OrchestrationPolicy::new()),
            guardrails,
            iterations: 0,
            cost: None,
            failure: None,
            progress: None,
            forward: None,
            journal: Vec::new(),
            at: None,
            waiting: None,
            place: None,
            cut_short: false,
            stopped: None,
            exhausted_by: None,
            asking: None,
            taken_over: None,
            answered: 0,
            screened: 0,
            // ⚠ `None` AND NOT `Some(0)` — a run that has not been stepped has not been asked, and
            // *nobody was counting* is a different answer from *it set nothing aside*. See
            // `Plugin::deferred`.
            deferred: None,
            // ⚠ `None` FOR `deferred`'s REASON ONE LINE UP — a run nobody has stepped has not been
            // asked, and *nobody was counting* is not *every direction it took was checked*.
            unchecked: None,
            deliveries: Deliveries::NONE,
            checks: Checks::NONE,
            banked: None,
            briefed: None,
            // ⚠ A RUN THAT HAS NOT ENDED HAS NAMED NO ENDING — register item 706. Never a word
            // standing in for *not yet*; see `Outcome::done_reason` for what `None` promises.
            done_reason: None,
            driving: None,
            winding: false,
        }
    }

    /// Publish this run's progress into `cell` as it goes.
    ///
    /// Optional because the driver is used without a host — a test, or a fire-and-forget run — and
    /// a driver that REQUIRED somewhere to report to would make every such caller invent one.
    #[must_use]
    pub fn reporting_to(mut self, cell: ProgressCell) -> Self {
        self.progress = Some(cell);
        self
    }

    /// Also hand this run's progress to `forward` as it goes — for a watcher that cannot read the
    /// cell because it is in another process. See [`ProgressSink`] for why a call rather than a
    /// second cell.
    ///
    /// ⚠ Called from the same place and at the same moment as the cell write, so the two can never
    /// disagree about WHEN a number became true.
    #[must_use]
    pub fn forwarding_to(mut self, forward: ProgressSink) -> Self {
        self.forward = Some(forward);
        self
    }

    /// Write the counters out, if anybody is watching.
    ///
    /// ⚠ Called after the accumulate and NOT before the step: a reader must never see an iteration
    /// counted before the work it counts has happened, which is the same ordering rule the run-end
    /// announcement follows one layer up.
    fn publish(&self) {
        if self.progress.is_none() && self.forward.is_none() {
            return;
        }
        let progress = Progress {
            iterations: self.iterations,
            cost: self.cost,
            journal: self.journal.clone(),
            answered: self.answered,
            screened: self.screened,
            // 🎯 AND WHAT IT HAS SET ASIDE SO FAR — register item 833(2), published WHILE THE RUN
            // IS STILL GOING for `answered`'s reason on the row below: a person watching a loop
            // defer proposal after proposal is the one who can still change its brief.
            deferred: self.deferred,
            // 🎯 AND HOW MANY OF THEM NOBODY CHECKED — register item 847, live for the reason
            // above it: the person watching is the one who can still go and name a classifier.
            unchecked: self.unchecked,
            at: self.at,
            waiting: self.waiting.clone(),
            place: self.place.clone(),
            deliveries: self.deliveries,
            checks: self.checks.clone(),
            banked: self.banked.clone(),
            briefed: self.briefed,
            driving: self.driving,
            // ⛔⛔⛔ **A DRIVER'S OWN COUNTERS ARE NEVER A PREDECESSOR'S** — register item 815, and
            // this line is the whole of how that flag clears: the cell is republished WHOLE from
            // this driver's fields, so the first step after a boot puts back a run overwrites the
            // restored `true` with what is true now. There is no clearing site to forget.
            inherited: false,
        };
        if let Some(cell) = &self.progress {
            let mut held = cell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *held = progress.clone();
        }
        // ⚠⚠ THE SAME VALUE THE CELL GOT, AND FROM THE SAME CALL. A forwarder that built its own
        // `Progress` a line later could differ from what a local reader sees — and the two readers
        // are the same person looking at one run through two windows.
        if let Some(forward) = &self.forward {
            forward(&progress);
        }
    }

    /// Record what a step did, keeping the last [`JOURNAL_LIMIT`].
    ///
    /// ⚠ The step's OWN cost, not the running total: a journal of totals could not answer *"which
    /// step was the expensive one?"*, which is the question a cost ceiling makes people ask.
    /// ⚠⚠⚠ `walked` is passed IN rather than read off the step, because it is the PLUGIN's answer
    /// and a `Step` is content the plugin composed — see [`crate::plugin::Plugin::walked`].
    /// One call site, taken the moment the pass returns, is what makes *this walk belongs to this
    /// step* true by construction rather than by every producer remembering.
    fn record(&mut self, step: &Step, walked: Vec<crate::plugin::Edge>) {
        self.remember(StepRecord {
            iteration: self.iterations,
            cost: step.cost,
            verdict: step.verdict.clone(),
            note: step.note.clone(),
            walked,
        });
    }

    /// **THE LINE A RUN LEAVES WHEN A STEP COULD NOT BE TAKEN** — register item 680, and the twin
    /// of [`note_to_itself`](Self::note_to_itself) one failure over.
    ///
    /// # ⚠⚠⚠ Why the note carries the PLACE and not only the error
    ///
    /// The error says *what* — `there is no pane 7` — and a reader of a dead run needs *where*: a
    /// machine has a dozen states and only one of them was taking the call. [`Plugin::at`] is that
    /// word (register item 543 built it, in the DOCUMENT's own spelling), and this is the one place
    /// a failure can ask for it, because the plugin is about to be dropped.
    ///
    /// ⚠ `at` is [`None`] for a plugin that does not publish a place — a `pipe`, an `answer` — and
    /// the sentence then says only what happened. That is honest rather than degraded: those
    /// plugins have no state a reader could be sent to.
    ///
    /// # ⚠⚠ Why the verdict is a NEW word and not one of the nine already published
    ///
    /// The first draft of this took [`Verdict::Abandoned`] to avoid widening a published
    /// vocabulary, and that was the exact mistake this function's own reasoning rejects one
    /// paragraph up: `Abandoned` means *a person said wait and then stopped looking*, so a reader
    /// meeting it on a pane that vanished goes hunting for the person. Every one of the nine is a
    /// CONCLUSION a pass reached, and this pass reached none —
    /// see [`Verdict::Failed`], which states the cost it earns.
    fn note_failure(&mut self, error: &PaneError, plugin: &dyn Plugin) {
        let note = plugin.at().map_or_else(
            || format!("the step could not be taken: {error}"),
            |place| format!("the step could not be taken at `{place}`: {error}"),
        );
        // ⛔⛔⛔⛔⛔ **AND WHAT THAT MEANS FOR THE PANE THIS RUN WAS DRIVING** — register item 682,
        // and the sentence that would have saved a day.
        //
        // # ⚠⚠⚠⚠⚠ Why the error's own words are not enough HERE, when they are everywhere else
        //
        // `there is no pane 5` is [`PaneError::UnknownPane`]'s rendering, and for the caller it was
        // written for — somebody who mistyped an id — it is exactly right. **For a RUN it reads as
        // a lie.** A run's pane is checked into its pool before the run starts
        // (`sprag_host`'s `require_pane_in`), so the id was real; and a pool is ONE WINDOW's, so
        // the pane can be alive in the next window along while this sentence is true.
        //
        // Measured 2026-08-25: pane `5` was running in window `pinion` — its child had been up for
        // 2h40m — while the run that had been driving it ended `failed: there is no pane 5`. The
        // reader went looking, found the pane, and concluded the run was wrong about its own death.
        // **It was not.** The pane had left the pool the run holds, which is a different fact from
        // the pane not existing, and nothing said so.
        //
        // # ⚠⚠ What it does NOT claim, and why the hedge is the honest part
        //
        // *Where* the pane went is a SESSION fact, and this layer sees one pool by design (the ISP
        // boundary `sprag_host::plugin_host` keeps: a plugin has no business knowing about the
        // window tree). A move and a close are indistinguishable from here — `sprag_plugin`'s
        // `a_run_ends_the_same_way_whether_its_pane_moved_or_was_closed` is the gate that measures
        // it. So the clause says *may still be open*, which is the true shape of what is known:
        // it names the DIRECTION to look rather than pretending to a fact this side cannot have.
        //
        // ⚠⚠⚠ ADDED TO the error's sentence rather than replacing it. The error's own words are
        // what tie a dead run to the incident that produced it, and this crate's standing rule is
        // that a failure travels in the vocabulary the layer that raised it used — a second
        // spelling would be a second authority on one fact.
        //
        // ⚠⚠⚠ Guarded on the pane being the one the plugin is DRIVING, and that guard is what makes
        // the clause mean anything: a run dying while reaching for a pane of its own making — a
        // milestone checker's, say — has not lost ITS pane, and a clause there would send a reader
        // to the wrong window. A plugin driving nothing at all gets none either.
        //
        // ⚠⚠⚠⚠⚠ **ASKED OF THE PLUGIN HERE RATHER THAN READ OFF [`driving`](Self::driving), AND A
        // GATE MEASURED WHY.** That field is refreshed only after a step COMPLETES, so a driver
        // handed a plugin already mid-flight — which is what
        // `ai_loop::tests::a_judging_run_whose_pane_left_its_pool_dies_where_the_live_runs_died`
        // hands it — has never set it, and the clause silently did not appear on the one gate that
        // reconstructs the live incident. The plugin can still answer at the moment it fails, and
        // that answer is about NOW rather than about the last step that worked.
        //
        // ⚠⚠ It therefore claims nothing about HISTORY. *This is the pane the run is driving and
        // this workspace has not got it* is true whether the pane left or was never here, and both
        // send a reader the same useful way — which is the honest scope of what one pool can know.
        let note = match error {
            PaneError::UnknownPane(gone) if plugin.driving() == Some(*gone) => format!(
                "{note} — that is the pane this run is driving, and a workspace is one window's \
                 pool: it may still be open in another window of this session",
            ),
            _ => note,
        };
        self.remember(StepRecord {
            iteration: self.iterations,
            // ⚠ ZERO OF WHATEVER THIS RUN COUNTS, on [`note_to_itself`]'s precedent one function
            // up: a step that never ran spent nothing, and a line that carried some other unit
            // would be the only one in the journal a reader could not add up.
            cost: self.cost.map_or(Cost::Bytes(0), Cost::none_of),
            verdict: Verdict::Failed,
            note: Some(note),
            walked: plugin.walked(),
        });
        // ⚠⚠⚠⚠⚠ **AND PUBLISHED, WHICH IS HALF THE FIX AND WAS ALMOST MISSED.** `remember` writes
        // into the driver's OWN journal; the run's readers hold a [`ProgressCell`]. Without this
        // line the failure entry exists and nobody outside can read it — register item 492's shape,
        // in the very function written to close a legibility gap. [`note_to_itself`] publishes for
        // exactly this reason, one function up.
        //
        // ⚠⚠ It is the LAST line a run publishes: the loop breaks on `Fail` immediately after, so
        // this is the state a watcher's final poll — and the daemon's stored row — will hold.
        self.publish();
    }

    /// Keep `record`, dropping the oldest once the journal is full.
    fn remember(&mut self, record: StepRecord) {
        if self.journal.len() == JOURNAL_LIMIT {
            self.journal.remove(0);
        }
        self.journal.push(record);
    }

    /// ⚠⚠⚠ **THE LINES THE DRIVER WRITES INTO A RUN'S JOURNAL ABOUT ITSELF** — what it did when a
    /// ceiling fell due, which is the one thing in a run's walk that no step can report.
    ///
    /// # ⚠⚠ Why this is published here and not as a field of [`Outcome`]
    ///
    /// A journal is *what happened in there*, it is already published as a run goes
    /// ([`Progress::journal`]), and *"the budget ran out, so the loop was asked where it got to"*
    /// is exactly that kind of fact. An outcome field would have been a second place a consumer has
    /// to know to look, published on every run to say nothing on almost all of them — and the one
    /// thing a reader wants beside `exhausted` is the WALK, which is here.
    ///
    /// ⚠⚠ **AND WITHOUT IT THE WALK CANNOT SAY WHICH BUDGET SENT A LOOP TO ITS ACCOUNT.** A run
    /// stopped by a wall clock and one that spent the document's own `max_turns` produce the same
    /// two lines — `Judging --Judge--> Stopping`, `Stopping --TurnDone--> Exhausted` — because both
    /// budgets reach that state by the same edge. This is where they differ.
    ///
    /// ⚠ It borrows the run's own [`Cost`] unit at nothing spent
    /// ([`Cost::none_of`]) rather than defaulting to bytes: this record cost the peer nothing, and
    /// a zero in the wrong currency is a lie in a column somebody sums. A run that never took a
    /// step has no unit at all, and bytes is then the substrate's own — nothing was typed, and
    /// saying so in the unit every relay uses is the least wrong answer available.
    fn note_to_itself(&mut self, ceiling: Ceiling, note: String) {
        self.remember(StepRecord {
            iteration: self.iterations,
            cost: self.cost.map_or(Cost::Bytes(0), Cost::none_of),
            verdict: Verdict::Exhausted(ceiling),
            note: Some(note),
            // ⚠⚠ EMPTY, AND NOT FOR WANT OF KNOWING: this record is the DRIVER's own line about a
            // ceiling IT enforced, and the driver takes no transitions. The machine's walk for the
            // step that reached the ceiling is on that step's record, one entry back — a copy here
            // would be a second authority on one path, which is item 445's whole argument.
            walked: Vec::new(),
        });
        self.publish();
    }

    /// Drive `plugin` over `panes` until a terminal state, reporting the
    /// [`Outcome`].
    ///
    /// ⚠ The context the PLUGIN sees is not the one handed in: the run's deadline
    /// is armed here, at the one moment "when does this run end?" has a single
    /// answer every wait underneath can share.
    ///
    /// # ⚠⚠⚠ Panics
    ///
    /// A panicking plugin's panic is RE-RAISED, after the work it had going is stopped. Swallowing
    /// it would trade a leak for a lie — the host turns the worker's join failure into
    /// `RunState::Panicked` and a caller is entitled to that — but letting it unwind unattended was
    /// **the one ending that skipped the stop**. Measured: a plugin that typed `sleep 300` into a
    /// pane and then panicked left the `sleep` running, with the run reported as panicked and
    /// nothing said about the peer. That is precisely the unbounded loop the guardrails exist to
    /// prevent, reached by the one path around them.
    #[must_use]
    pub fn run(
        mut self,
        plugin: &mut dyn Plugin,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Outcome {
        let run = run.deadline_in(self.guardrails.max_duration);
        // ⚠⚠ THE STEPPING IS GUARDED AGAINST AN UNWIND, and `AssertUnwindSafe` is an assertion this
        // makes rather than an escape: what the closure borrows is this Driver's own counters, a
        // `Vec`, and the statechart engine, none of which a `plugin.step` panic can leave
        // half-mutated — the panic happens INSIDE the plugin, before any of them is touched for
        // that step. The plugin itself may well be inconsistent, which is why it is never stepped
        // again and why the panic is re-raised rather than absorbed.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.step_to_a_terminal_state(plugin, panes, run);
        }));
        if let Err(payload) = unwound {
            // ⚠ The same NARROW reach a cut-short run uses: a plugin bug must not close somebody's
            // pane. And the report is DISCARDED here rather than published, because there is no
            // outcome to publish it in — the honest channel for a panicking run is the panic.
            let _ = Self::stop_the_work(&*plugin, panes);
            std::panic::resume_unwind(payload);
        }
        // ⚠⚠ THE WORK OUTLIVES THE LOOP UNLESS SOMEBODY ENDS IT. After the statechart has reached a
        // terminal state and before the outcome is assembled, because the answer is part of the
        // outcome — a run that reports `cancelled` without saying what became of its work is
        // telling half of what happened.
        if self.cut_short {
            self.stopped = Some(Self::stop_the_work(&*plugin, panes));
        }
        self.outcome()
    }

    /// Step the plugin until the statechart is final — [`run`](Self::run)'s loop, split out so the
    /// unwind guard has one call to wrap rather than a closure holding the whole body.
    ///
    /// ⚠⚠ THE CONTEXT IS OWNED HERE rather than borrowed, and that is what the account turn needs:
    /// a run granted a window to say where it got to is a run whose deadline MOVES, once, and the
    /// waits inside the plugin have to see the new one. Re-arming a borrowed context would have
    /// meant a second authority on *when is this run over*.
    fn step_to_a_terminal_state(
        &mut self,
        plugin: &mut dyn Plugin,
        panes: &dyn PaneAccess,
        mut run: RunContext,
    ) {
        self.engine.initialize();
        self.engine.process_event(OrchestrationEvent::Start);
        // ⚠⚠⚠⚠⚠ **ASKED ONCE BEFORE ANY STEP, so the answer does not depend on a step completing.**
        // Register item 604's fact is read by the sentence a person is shown at the END, and a run
        // cancelled before its first pass finished would otherwise report *this plugin does not
        // count completed work* — which is false of a loop that counts perfectly well and had
        // simply not started. Measured 2026-08-23 on exactly that run.
        //
        // ⚠⚠ It is `None` for a plugin with no unit of completed work either way, so the two
        // answers stay distinguishable: *nobody counts* and *counted, and it was nothing*.
        self.banked = plugin.banked();
        // ⚠⚠⚠⚠ **AND HOW BIG ITS BRIEF IS, ASKED IN THE SAME BREATH AND FOR A SHARPER VERSION OF
        // THE SAME REASON** — register item 719. This one is already true before the first step:
        // the door took the brief, and a run cancelled or refused before a step completed is
        // exactly the run whose reader most wants to know what it was handed. Read here, that row
        // says so; read only per-step, it would say nothing about the run that never stepped.
        self.briefed = plugin.briefed();
        // `running` is the only non-final state in the loop.
        while !self.engine.is_in_final_state() {
            // Cancel is checked before each step (and again by the plugin's own
            // wait loops mid-step), so a cancel ends the run promptly without
            // running another step.
            // ⚠ THE DEADLINE IS CHECKED BEFORE THE STEP, NOT AFTER IT. Checked
            // after, a run whose remaining time is a millisecond would still
            // start a step that may take minutes, and the ceiling would be
            // advisory. Checked before, the run's LAST step is the last one that
            // began in time — and the waits inside it are bounded by the same
            // deadline, so it cannot outlive it by more than a poll interval.
            if let Some(event) = self.ended_from_outside(plugin, &mut run) {
                self.engine.process_event(event);
                continue;
            }
            let event = match plugin.step(panes, &run) {
                Err(error) => {
                    // ⛔⛔⛔⛔⛔ **THE JOURNAL GETS THE LINE TOO — register item 680.** Until this
                    // arm wrote one, a step that failed set `failure` and NOTHING else: the walk
                    // recorded only steps that SUCCEEDED, so a run's own record ended at the last
                    // thing that worked and the failure was a summary sentence with no place in it.
                    //
                    // Measured 2026-08-25: three live runs died `there is no pane N`, all three
                    // with `Working --TurnDone--> Judging` as their last walk entry and nothing
                    // after it — and the pane each named was alive. **The defect could be narrowed
                    // by elimination and never confirmed**, because nothing said which call raised
                    // it (register item 682). Item 505 built exactly this legibility for a failure
                    // the DOCUMENT answers; an I/O failure never reaches the document, so it never
                    // got one.
                    //
                    // ⚠⚠ Written BEFORE `failure` is taken, because the sentence is composed from
                    // the error and this is the only place that holds it, the plugin's place and
                    // the plugin's walk at the same instant — the rule item 637 states, one axis
                    // over: two values that are compared are read at the same moment.
                    self.note_failure(&error, plugin);
                    self.failure = Some(error);
                    OrchestrationEvent::Fail
                }
                Ok(step) => {
                    self.iterations += 1;
                    self.accumulate(step.cost);
                    // ⚠ Counted with the loop's other per-step totals rather than inside the
                    // verdict match below, so an answer is tallied by the same code that tallies
                    // iterations and spend — one place a step is measured, not two.
                    if matches!(step.verdict, Verdict::Answered(_)) {
                        self.answered += 1;
                    }
                    // ⚠ Counted BESIDE the approval tally and never inside it: the two are opposite
                    // decisions, and a reader asking what a run let its agent DO must not be handed
                    // a number that also counts what it stopped.
                    if matches!(step.verdict, Verdict::Screened(_)) {
                        self.screened += 1;
                    }
                    self.record(&step, plugin.walked());
                    // ⚠⚠⚠⚠⚠ WHERE THE PLUGIN IS, ASKED AT THE ONE PLACE A STEP COMPLETES — register
                    // item 543. This is why `Plugin::at` is a question rather than a field on
                    // `Step`: a per-step field can be forgotten at any of the twenty-odd sites that
                    // build one, and a forgotten one does not read as absent, it reads as *still
                    // where it last said*. Asked here, after the step and before the publish, it
                    // cannot be missed and cannot be stale.
                    self.at = plugin.at();
                    // ⛔⛔⛔⛔⛔ AND WHETHER ANYBODY IS WAITED ON — register item 755, in the same
                    // breath as `at` above and for its reason exactly: two readings of one machine
                    // taken a step apart describe two moments, and this one is the fact a watcher
                    // acts on. Asked here rather than derived downstream, because which states mean
                    // *a person is needed* is the DOCUMENT's answer and only the plugin holds it.
                    self.waiting = plugin.waiting_for_a_person();
                    // ⚠ BESIDE `at` AND AT THE SAME MOMENT — register item 543. Two readings of one
                    // machine taken a step apart would be a record whose word and whose
                    // configuration describe different places, and nothing downstream could tell.
                    self.place = plugin.place();
                    // ⚠⚠⚠ AND WHAT ITS DELIVERIES CAME TO, asked in the same breath and for the
                    // same reason — register item 591. Two totals read a moment apart from one
                    // plugin are two facts about two moments, which is this repository's *비교하는
                    // 두 값은 같은 순간에* rule; and asking here rather than carrying it on a
                    // `Step` means it cannot be forgotten at one of the twenty-odd sites that
                    // build one, where a forgotten fold reads as a prompt somebody can go and see.
                    self.deliveries = plugin.deliveries();
                    // ⚠⚠⚠ AND WHAT ITS CHECKS CAME TO — register item 601, in the same breath as
                    // the two above for the same reason: three totals read at three moments are
                    // three facts about three moments, and a reader weighing a `converged` needs
                    // them to describe one run.
                    self.checks = plugin.checks();
                    // 🎯🎯🎯 AND HOW MANY PROPOSALS IT HAS SET ASIDE AT ITS RE-AIMING CAP —
                    // register item 833(2), in the same breath as the three above and for their
                    // reason. ⚠ It is KEPT where the plugin stops being able to answer, on
                    // `banked`'s terms one line down: the last step is the one that ends the run,
                    // and a machine in a final state may no longer read its own datamodel — what a
                    // reader wants is the last answer it COULD give, not the silence after it.
                    self.deferred = plugin.deferred().or_else(|| self.deferred.take());
                    // 🎯 AND HOW MANY OF ITS DIRECTIONS NOBODY CHECKED — register item 847, read in
                    // the same breath and KEPT on the same terms: the step that ends a run is the
                    // one whose machine may no longer read its own datamodel, and what a reader
                    // wants is the last answer it COULD give rather than the silence after it.
                    self.unchecked = plugin.unchecked().or_else(|| self.unchecked.take());
                    // ⚠⚠⚠⚠⚠ AND HOW MUCH OF ITS WORK IS COMPLETE AND KEPT — register item 604, in
                    // the same breath as the three above and for their reason. It is asked EVERY
                    // step rather than once at the end because the last step is the one that ends
                    // the run, and a plugin whose machine has reached a final state may no longer
                    // be able to answer: what is wanted is the last answer it COULD give, which is
                    // what a per-step read leaves behind.
                    self.banked = plugin.banked().or_else(|| self.banked.take());
                    // ⚠⚠ AND THE BRIEF'S SIZE, on the same terms — kept where the plugin stops
                    // being able to answer, for `banked`'s reason one line up. A brief is taken
                    // once and never re-read, so this can only ever confirm what the pre-step read
                    // already had; asking anyway is what keeps every level on one road.
                    self.briefed = plugin.briefed().or(self.briefed);
                    // ⛔⛔⛔⛔⛔ AND WHICH ENDING IT NAMED, IF IT HAS NAMED ONE — register item
                    // 706's third requirement, in the same breath as the totals above and for
                    // `banked`'s reason at its sharpest. The step that names an ending is the step
                    // that reaches the machine's final state, so this read is the ONLY one that
                    // can see it: a plugin asked once more afterwards may have no datamodel left
                    // to read the word out of, and `or_else` is what keeps the answer it could
                    // still give.
                    //
                    // ⚠⚠ It is a WORD the plugin owns and this driver never interprets — see
                    // `Plugin::ended_because`. Nothing here compares it to `OutcomeState`, because
                    // all three of a loop's endings converge and a driver that tried to check one
                    // against the other would be inventing a second authority on the distinction.
                    self.done_reason = plugin.ended_because().or_else(|| self.done_reason.take());
                    // ⚠⚠⚠ AND WHICH PANE IT IS DRIVING — register items 540 and 595, in the same
                    // breath as the three above. A pane read a moment apart from the counters is a
                    // fact about a different moment, and this one is compared against the LIVE
                    // workspace by a mouth that has to say *is anybody driving this pane*.
                    self.driving = plugin.driving();
                    self.publish();
                    let decided = match &step.verdict {
                        // A step that saw the goal SAW IT. A stop or a deadline arriving in the
                        // same instant does not un-reach it, and the plugins hand back `Continue`
                        // rather than a verdict off a screen nobody finished reading precisely so
                        // that a `Converged` reaching here is a real one.
                        Verdict::Converged => OrchestrationEvent::Converge,
                        // ⚠⚠⚠ AND A PEER THAT STOPPED TO ASK OUTRANKS EVERYTHING BELOW, including
                        // the run's own end. A cancel arriving in the same instant does not make
                        // the question go away, and reporting `cancelled` would lose the one fact
                        // a person has to act on. Recorded here rather than re-derived at the
                        // final state, because the pane it was read from may be gone by then.
                        Verdict::Blocked(asking) => {
                            self.asking = Some(asking.clone());
                            OrchestrationEvent::Block
                        }
                        // ⚠⚠⚠ AND A PERSON AT THE PANE OUTRANKS EVERYTHING BELOW TOO, for the arm
                        // above's reason turned around: a cancel or a spent budget arriving in the
                        // same instant does not un-take the pane, and the run must not be recorded
                        // as having merely run out while somebody was typing into it.
                        Verdict::TakenOver(interruption) => {
                            self.taken_over = Some(*interruption);
                            OrchestrationEvent::Taken
                        }
                        // ⚠⚠⚠⚠ AND A PEER THAT IS GONE OUTRANKS THE TALLY FOR THE TWO ARMS ABOVE'S
                        // REASON: it is a fact about the world outside this run, and a budget
                        // arriving in the same instant does not bring the program back. A run told
                        // `exhausted` here sends its reader to raise a number; the thing to do is
                        // find out why the agent left.
                        //
                        // ⚠⚠⚠ **RECORDED AS THE RUN'S FAILURE, AND THAT IS A DECISION RATHER THAN
                        // A SHORTCUT.** A seventh [`OutcomeState`] word was the alternative, and
                        // what it would buy over `failed` + this cause is nothing a caller can act
                        // on differently: [`Outcome::failure`] carries the typed
                        // [`PaneError::PeerGone`] with the PANE in it, and its sentence is the one
                        // that says why typing anyway is the worse answer. What separated `blocked`
                        // and `taken_over` from `failed` was that their readers must do something
                        // ELSE — answer, or nothing at all; this reader does what a failed run's
                        // reader does, which is go and look at what broke. ⚠ The residue is
                        // written down rather than hidden: a run RESTORED from a previous daemon's
                        // log has only the word `failed`, so the pane and the sentence do not
                        // survive a restart — the same residue `Blocked`'s own `Option` states.
                        Verdict::PeerGone(pane) => {
                            self.failure = Some(PaneError::PeerGone(*pane));
                            OrchestrationEvent::Fail
                        }
                        // ⚠⚠ THE RUN'S OWN END OUTRANKS THE TALLY, and asking in the other order
                        // was two defects: a person's stop mid-turn reported as `exhausted —
                        // iterations`, and a deadline that curtailed the last permitted turn
                        // reported the same. Both told the reader to raise a guardrail that would
                        // have bought the run nothing, about work that never finished.
                        //
                        // ⚠⚠ AN ANSWERED STEP TAKES THIS ARM, and sharing it is the claim: a step
                        // that answered its peer's dialog continues under exactly the same
                        // guardrails as any other, because an approval is not a licence to exceed
                        // a ceiling. What makes it different is a RECORD and not a control path —
                        // the fifth verdict word, and the tally kept beside the loop's own
                        // counters — so a second arm here would be one concept spelled twice.
                        // ⚠⚠ THE PLUGIN'S OWN BUDGET, recorded through the SAME writer the three
                        // guardrails go through — `exhaust` is the single site that pairs the
                        // recorded ceiling with the transition, so a fourth source of exhaustion
                        // cannot reach `exhausted` without saying which one it was.
                        //
                        // ⚠ Below `blocked` and `taken_over` and above the run's own end, and each
                        // half of that is a decision. A peer's question and a person's hand are
                        // facts about somebody else and outrank arithmetic. A cancel arriving in
                        // the same instant does NOT: the plugin has already looked and its budget
                        // is already spent, so reporting `cancelled` would tell the reader to
                        // start it again when the answer is to give it more turns.
                        Verdict::Exhausted(ceiling) => self.exhaust(*ceiling),
                        // ⚠⚠⚠⚠ A HOLD THAT OUTLIVED ITS CEILING TAKES THE SAME ARM AS ANY OTHER
                        // SPENT BUDGET, and going through `exhaust` rather than beside it is the
                        // claim — register item 534. `exhaust` is the single site that pairs the
                        // recorded ceiling with the transition, which is exactly why a fifth source
                        // of exhaustion must not reach `exhausted` any other way.
                        //
                        // ⚠⚠⚠ AND IT IS BELOW `blocked` AND `taken_over` FOR THEIR OWN REASON,
                        // WHICH BITES HARDEST HERE: a person's hand on the pane outranks this, and
                        // a hold is precisely the order a person gives before putting it there. A
                        // run whose reader is told *nobody came back* when somebody is typing in it
                        // right now is the one sentence this ending must never produce.
                        Verdict::Abandoned => self.exhaust(Ceiling::Hold),
                        // ⚠⚠⚠ **A WORD ONLY THIS DRIVER WRITES, AND NEVER INTO A `Step`** —
                        // register item 680. [`Verdict::Failed`] is composed by
                        // [`note_failure`](Self::note_failure) for the journal line a run leaves
                        // when a pass returned an ERROR instead of a verdict; a plugin has no way
                        // to hand one back, because a plugin that failed returned `Err` and never
                        // reached this match at all.
                        //
                        // ⚠⚠ It is answered rather than left to a catch-all, on `outcome()`'s rule
                        // in this same file: a wildcard here would silently absorb the NEXT word
                        // this vocabulary grows. `Fail` is the honest event — a step that could not
                        // be taken has failed the run — and the `debug_assert` says loudly, in the
                        // build that can afford to, that arriving here means a producer this
                        // comment does not know about.
                        Verdict::Failed => {
                            debug_assert!(
                                false,
                                "a plugin handed back Verdict::Failed, which only the driver writes",
                            );
                            OrchestrationEvent::Fail
                        }
                        // ⚠⚠ A SCREENED STEP TAKES THE SAME ARM AS AN ANSWERED ONE, and sharing it
                        // is the claim: a step that refused its peer's call and redirected it
                        // continues under exactly the same guardrails as any other, because a
                        // refusal is no more a licence to exceed a ceiling than an approval is.
                        // What makes it different is a RECORD and a tally, not a control path.
                        Verdict::Continue | Verdict::Answered(_) | Verdict::Screened(_) => {
                            match self.ended_from_outside(plugin, &mut run) {
                                Some(event) => event,
                                // ⚠⚠ THE PER-STEP CEILINGS ARE NOT RE-ASKED ONCE THE RUN IS
                                // ACCOUNTING FOR ITSELF. They are already spent — that is what got
                                // us here — and the counters go on rising, so consulting them would
                                // end the account turn on the very step after it began.
                                None if self.winding => OrchestrationEvent::Continue,
                                None => match self.budget_exhausted() {
                                    Some(ceiling) => self
                                        .spend_or_account(plugin, &mut run, ceiling)
                                        .unwrap_or(OrchestrationEvent::Continue),
                                    None => OrchestrationEvent::Continue,
                                },
                            }
                        }
                    };
                    self.only_an_account(decided)
                }
            };
            self.engine.process_event(event);
        }
    }

    /// ⚠⚠⚠ **A RUN SAYING WHERE IT GOT TO CANNOT BE SENT SOMEWHERE ELSE BY WHAT HAPPENS WHILE IT
    /// SAYS IT** — the endings that are facts about the ACCOUNT collapse into the ceiling that
    /// stopped the run.
    ///
    /// This is [`ai_loop.scxml`]'s own rule about `stopping`, one layer out and for the same
    /// reason. A peer that stops to ask in the middle of writing its account, or a person who takes
    /// the pane while it is being written, would end the run `blocked` or `taken_over` — **sending
    /// its reader off to answer a dialog when the knob they need is the ceiling they set
    /// themselves.** Neither fact is about the work; both are about a courtesy turn granted after
    /// the run was already over.
    ///
    /// # ⚠⚠⚠ Why `Converge` is NOT one of them, which is the arm worth arguing
    ///
    /// It looks like the same shape and it is the opposite fact. A plugin only converges by
    /// reaching the goal it was given, and a window cannot manufacture one: the loop routes a run
    /// that is stopping short to its account state ahead of every other judgement, so the only way
    /// a `Converge` arrives inside a window is that the plugin was ALREADY writing its closing
    /// report when the ceiling fell due. Collapsing it would tell a caller their finished work ran
    /// out of time, and they would run it again. **The word is about the work; the window changed
    /// only when the work stopped being paid for.**
    ///
    /// ⚠ `Fail` passes for a nearby reason: a plugin whose step returned an error has recorded a
    /// [`PaneError`] in [`Outcome::failure`], and an outcome that said `exhausted` while carrying a
    /// failure would be two answers to one question.
    ///
    /// ⚠ `Cancel` passes because a person's stop is somebody's decision and outranks arithmetic
    /// everywhere else in this file; a grace window is not where that changes.
    ///
    /// ⚠⚠ EXHAUSTIVE, so a further orchestration event has to be classified here rather than
    /// silently inheriting *changes the ending*.
    ///
    /// [`ai_loop.scxml`]: ../../ai_loop.scxml
    const fn only_an_account(&self, event: OrchestrationEvent) -> OrchestrationEvent {
        if !self.winding {
            return event;
        }
        match event {
            OrchestrationEvent::Block | OrchestrationEvent::Taken | OrchestrationEvent::Exhaust => {
                OrchestrationEvent::Exhaust
            }
            // ⚠ `Null` is W3C SCXML 3.13's eventless sentinel: this Driver never raises it, and an
            // arm that mapped it to anything would be inventing a transition for a non-event.
            OrchestrationEvent::Start
            | OrchestrationEvent::Continue
            | OrchestrationEvent::Converge
            | OrchestrationEvent::Fail
            | OrchestrationEvent::Cancel
            | OrchestrationEvent::Null => event,
        }
    }

    /// ⚠⚠⚠ **A CEILING HAS BOUND — END THE RUN, OR GIVE IT ONE WINDOW TO SAY WHERE IT GOT TO.**
    ///
    /// Answers [`None`] when the plugin took the window and the loop should carry on stepping it,
    /// and `Some(Exhaust)` when there is nothing more to do.
    ///
    /// # ⚠⚠⚠ Why the ceiling is recorded BEFORE the plugin is asked anything
    ///
    /// The account is one more turn of the plugin's own machine, and that machine has budgets of
    /// its own: `ai_loop.scxml` reaches its `exhausted` through `stopping`, so the very step that
    /// hands back the account reports [`Verdict::Exhausted`] with [`Ceiling::Turns`]. Recorded
    /// afterwards, that would overwrite the ceiling that actually stopped the run — and a caller
    /// told to raise `max_turns` about a run their WALL CLOCK ended is being sent to a knob that
    /// would have bought them nothing. [`exhaust`](Self::exhaust) keeps the first answer for
    /// exactly this reason.
    fn spend_or_account(
        &mut self,
        plugin: &mut dyn Plugin,
        run: &mut RunContext,
        ceiling: Ceiling,
    ) -> Option<OrchestrationEvent> {
        let ceiling = *self.exhausted_by.get_or_insert(ceiling);
        // ⚠ ASKED ONCE. A second window would be a second courtesy, and the first one's own clock
        // running out is the answer to whether the plugin could use it.
        //
        // ⚠⚠⚠ AND REACHING HERE A SECOND TIME MEANS THE ACCOUNT NEVER ARRIVED, which the Driver can
        // say without reading a word of it: a plugin that finished its account reported a terminal
        // verdict, and the run ended on that. So the only way the window's own clock runs out is
        // that the turn asking for the account did not end — and a run that says nothing about that
        // is back to the silence this whole method exists to remove, one step later.
        if self.winding {
            self.note_to_itself(
                ceiling,
                "no account: the window it was given to say so ran out first".to_owned(),
            );
            return Some(OrchestrationEvent::Exhaust);
        }
        match plugin.ask_for_an_account(ceiling) {
            Accounting::Nothing => Some(OrchestrationEvent::Exhaust),
            Accounting::Cannot(why) => {
                self.note_to_itself(ceiling, format!("no account: {why}"));
                Some(OrchestrationEvent::Exhaust)
            }
            Accounting::Within(window) => {
                self.note_to_itself(
                    ceiling,
                    format!(
                        "the run's own {} ceiling fell due; the plugin was given {window:?} to say \
                         where it got to",
                        ceiling.wire_str(),
                    ),
                );
                self.winding = true;
                // ⚠⚠⚠ THE DEADLINE MOVES, AND THIS IS THE ONLY PLACE IT EVER DOES. Every bounded
                // wait under the plugin consults this context, so a run whose clock has just run
                // out would otherwise abandon the account turn at its first poll — the account
                // would be asked for and never waited for.
                //
                // ⚠⚠ IT IS `window` FROM NOW rather than an extension of the old deadline, which
                // for a run stopped by `max_duration` means the run may last that much longer than
                // the caller asked. Stated rather than hidden: the window is the plugin's caller's
                // own number for ONE TURN of their peer, and the alternative — carving the account
                // out of the ceiling — silently spends work the caller asked for on a report they
                // did not.
                *run = run.deadline_in(Some(window));
                None
            }
        }
    }

    /// How the run ended FROM OUTSIDE its own logic — a person raised the cancel, or the clock ran
    /// out — or [`None`] while it is still allowed to run.
    ///
    /// ⚠⚠ THE ONE AUTHORITY on that question, consulted at the loop top AND after every
    /// unconverged step. Split across the two sites it was answered at only one of them, and the
    /// other decided the run's fate from step counters that have never heard of a cancel: the two
    /// ways a run ends from outside were invisible to the arithmetic that got to answer first.
    ///
    /// ⚠ Cancel is asked before the deadline at both sites, so a person's stop beats a clock that
    /// ran out in the same instant — a cancel is somebody's decision and an exhaustion is nobody's.
    ///
    /// ⚠⚠⚠ A PASSED DEADLINE NO LONGER ENDS THE RUN ON THE SPOT: it asks the plugin whether it can
    /// account for itself first, and [`None`] here then means *carry on into that window* rather
    /// than *still working* — see [`spend_or_account`](Self::spend_or_account). Once the window is
    /// armed this reads the NEW deadline, so the second expiry is the account's own and ends the
    /// run for good.
    fn ended_from_outside(
        &mut self,
        plugin: &mut dyn Plugin,
        run: &mut RunContext,
    ) -> Option<OrchestrationEvent> {
        if run.cancelled() {
            self.cut_short = true;
            return Some(OrchestrationEvent::Cancel);
        }
        if run.expired() {
            self.cut_short = true;
            return self.spend_or_account(plugin, run, Ceiling::Duration);
        }
        None
    }

    /// Record WHICH ceiling ended the run and answer the event that ends it.
    ///
    /// The single writer of [`exhausted_by`](Self::exhausted_by), so the recorded reason and the
    /// statechart transition cannot be raised apart from one another.
    ///
    /// ⚠⚠⚠ **THE FIRST CEILING WINS**, and that is a claim rather than tidiness. A run given a
    /// window to account for itself is STEPPED AGAIN, and the plugin's own budget falls due inside
    /// that window as a matter of course — `ai_loop.scxml`'s account turn ends in its `exhausted`,
    /// which reports [`Ceiling::Turns`]. Overwriting here would report the courtesy's ceiling
    /// instead of the one that stopped the run, and every remedy this word exists to name would be
    /// the wrong one.
    fn exhaust(&mut self, ceiling: Ceiling) -> OrchestrationEvent {
        self.exhausted_by.get_or_insert(ceiling);
        OrchestrationEvent::Exhaust
    }

    /// Add this step's cost to the running total, establishing the run's unit on
    /// the first step. Later steps share that unit (one plugin reports one unit),
    /// so the add always succeeds; a mismatch is a plugin bug (debug-asserted,
    /// release-ignored so a stray step can never corrupt the accumulator).
    fn accumulate(&mut self, cost: Cost) {
        self.cost = Some(match self.cost {
            None => cost,
            Some(acc) => acc.try_add(cost).unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "a plugin changed cost unit mid-run: {acc:?} + {cost:?}"
                );
                acc
            }),
        });
    }

    /// Which per-step ceiling this run has reached, if any.
    ///
    /// ⚠ Answers a [`Ceiling`] rather than a bool so the caller cannot raise the exhaustion without
    /// also saying what caused it. The DURATION ceiling is not asked here: it is not a property of
    /// a completed step, and the loop top is where a run out of time must stop.
    fn budget_exhausted(&self) -> Option<Ceiling> {
        if self.iterations >= self.guardrails.max_iterations {
            return Some(Ceiling::Iterations);
        }
        match (self.cost, self.guardrails.max_cost) {
            (Some(acc), Some(max)) if acc.reaches(max) => Some(Ceiling::Cost),
            _ => None,
        }
    }

    /// END THE WORK THIS RUN SET GOING, now that the run itself is over.
    ///
    /// Called on exactly the two endings that can land while a step is blocked — a cancel and a
    /// passed deadline — and answers what became of the work so the run can publish it.
    ///
    /// ⚠ NOT called for a run that ended on its own terms. A converged run's peer answered, a
    /// failed step never got to block on one, and the per-step ceilings are decided between steps
    /// with the last one already returned. Signalling there would interrupt work nobody asked to
    /// interrupt — a run that finished normally must leave the pane exactly as it found it.
    ///
    /// ⚠⚠ [`Stop::Interrupt`] and not one of the harder two. What is being ended is a TURN, and the
    /// program that was taking it — an agent CLI, a shell's job — is meant to still be there for
    /// the next run. A run that reached for `SIGKILL` because its clock ran out would leave the
    /// caller a dead peer to restart, which is a far larger consequence than the one they asked
    /// for.
    fn stop_the_work(plugin: &dyn Plugin, panes: &dyn PaneAccess) -> Stopped {
        let Some(pane) = plugin.driving() else {
            return Stopped::Nothing;
        };
        let Some(control) = panes.job_control() else {
            return Stopped::Unsupported;
        };
        match control.pane_stop_job(pane, Stop::Interrupt, Reach::UnderTheProgram) {
            Ok(signalled) => Stopped::Job(signalled),
            Err(why) => Stopped::Unreached(why),
        }
    }

    fn outcome(self) -> Outcome {
        let state = match self.engine.get_current_state() {
            OrchestrationState::Converged => OutcomeState::Converged,
            OrchestrationState::Exhausted => OutcomeState::Exhausted(
                // `exhaust` is the only producer of the event that reaches this state, and it
                // always records. A miss would mean the statechart reached `exhausted` by some
                // path nobody wrote — scream in debug; in release name the ceiling that bounds
                // EVERY run (`max_iterations` is never optional), which is the least wrong answer
                // available and still true of any run that got here.
                self.exhausted_by.unwrap_or_else(|| {
                    debug_assert!(false, "a run exhausted without recording which ceiling");
                    Ceiling::Iterations
                }),
            ),
            OrchestrationState::Cancelled => OutcomeState::Cancelled,
            // `Verdict::Blocked` is the only producer of the event that reaches this state, and it
            // always records — the same construction `exhausted` uses one arm up, for the same
            // reason: an outcome that cannot say what it is blocked on is not worth reaching.
            OrchestrationState::Blocked => OutcomeState::Blocked(self.asking),
            // `Verdict::TakenOver` is the only producer of the event that reaches this state, and
            // it always records — `blocked`'s construction one arm up, for its reason.
            OrchestrationState::TakenOver => OutcomeState::TakenOver(self.taken_over),
            // Failed, or any state the loop left unexpectedly.
            _ => OutcomeState::Failed,
        };
        Outcome {
            state,
            iterations: self.iterations,
            cost: self.cost,
            failure: self.failure,
            stopped: self.stopped,
            answered: self.answered,
            screened: self.screened,
            deferred: self.deferred,
            unchecked: self.unchecked,
            deliveries: self.deliveries,
            checks: self.checks.clone(),
            banked: self.banked,
            briefed: self.briefed,
            // ⛔⛔⛔ AND THE WORD THE PLUGIN CLOSED UNDER — register item 706. Carried, never
            // derived from `state` above: three endings share `Converged`, which is the collapse
            // this field exists to undo.
            //
            // ⚠ Borrowed, which is the half of the `Cow` a live run always takes: the word came
            // out of the plugin's own closed set a moment ago. Only a restore owns one.
            done_reason: self.done_reason.map(std::borrow::Cow::Borrowed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **THE PUBLISHED OUTCOME LIST IS HELD TO THE ONE THAT IS SERVED** — the residue R365 named
    /// on the answers pin, and the reason `OutcomeState` finally spells its own words.
    ///
    /// The pin that stops a run's outcome vocabulary from widening unnoticed used to walk a
    /// HAND-WRITTEN array of five variants, because this type carries data and has no `ALL`. So a
    /// sixth outcome added here would have reached the wire with the pin green — the exact way the
    /// FIFTH one (`blocked`) got in, one level up, before that pin existed at all.
    ///
    /// Building every variant and asking it for its word is what closes it: a sixth fails here
    /// until it is published, and the pin now walks the published list.
    #[test]
    fn every_outcome_the_type_can_spell_is_a_word_the_wire_publishes() {
        let served: Vec<&'static str> = [
            OutcomeState::Converged,
            OutcomeState::Exhausted(Ceiling::Iterations),
            OutcomeState::Failed,
            OutcomeState::Cancelled,
            OutcomeState::Blocked(None),
            OutcomeState::TakenOver(None),
        ]
        .iter()
        .map(OutcomeState::wire_str)
        .collect();
        assert_eq!(
            served,
            OutcomeState::WIRE_WORDS,
            "⚠⚠ an outcome the type can spell and the wire does not publish is a word a client \
             decoding this set WHOLE meets with no warning — and the pin that would have caught it \
             walks the PUBLISHED list, so it cannot see one that was never added",
        );
        // ⚠ THE CEILING DOES NOT CHANGE THE WORD. Folding it in (`exhausted_duration`) would widen
        // a value space old readers decode whole, which is a break no address pin can see.
        for ceiling in [Ceiling::Iterations, Ceiling::Cost, Ceiling::Duration] {
            assert_eq!(OutcomeState::Exhausted(ceiling).wire_str(), "exhausted");
        }
        let mut unique = OutcomeState::WIRE_WORDS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), OutcomeState::WIRE_WORDS.len());
    }
    use crate::access::{KeyStroke, PaneRow, Written};
    use crate::plugin::Step;
    use crate::run::poll_until;
    use sprag_terminal::PaneId;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A pane surface that RECORDS every stop asked of it and answers `Ok` — for the claims about
    /// WHEN the Driver stops work, which need no pseudoterminal to settle.
    ///
    /// ⚠ It honours the pane id in its answer, so a gate can tell *the Driver stopped the pane the
    /// plugin named* from *the Driver stopped something*.
    struct RecordingPanes {
        asked: Mutex<Vec<(PaneId, Stop)>>,
    }

    impl RecordingPanes {
        fn new() -> Self {
            Self {
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<(PaneId, Stop)> {
            self.asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl PaneAccess for RecordingPanes {
        fn pane_ids(&self) -> Vec<PaneId> {
            vec![PaneId(1)]
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            Some(String::new())
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            Some(Vec::new())
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            Some(false)
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            Some(String::new())
        }
        fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
            Ok(Written::of(0))
        }
        fn job_control(&self) -> Option<&dyn crate::access::PaneJobControl> {
            Some(self)
        }
    }

    impl crate::access::PaneJobControl for RecordingPanes {
        fn pane_stop_job(
            &self,
            id: PaneId,
            stop: Stop,
            _reach: Reach,
        ) -> Result<Signalled, PaneError> {
            self.asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((id, stop));
            Ok(Signalled {
                stop,
                pgid: 4711,
                leader: None,
            })
        }
    }

    /// ⛔⛔⛔⛔⛔ **A DRIVER FORWARDS THE SAME PROGRESS ITS CELL GOT, AND THAT INCLUDES WHERE THE
    /// MACHINE IS** — `sprag-host`'s register item 662, first link.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this link had to be measured and was not
    ///
    /// [`Driver::forwarding_to`] is how a run driven in ANOTHER PROCESS says anything at all: its
    /// daemon shares no cell with it, so the sink is the only channel, and what does not go down it
    /// is a fact the daemon can never learn. Item 662 measured the far end of that channel (the
    /// daemon dropped `at` and `place` on the floor) and closed it — and this end, *the driver puts
    /// them on the channel in the first place*, was held by a comment and nothing else. Nothing
    /// anywhere called `forwarding_to` outside the product.
    ///
    /// ⚠⚠ **THE CLAIM IS THAT THE TWO READERS AGREE**, not merely that the sink saw something: the
    /// cell and the sink are the same person looking at one run through two windows, so the
    /// assertion compares them rather than checking each against a literal. A forwarder that built
    /// its own `Progress` a line later is the defect this shape exists to prevent, and it would
    /// pass a gate that only asked *did the sink get a place*.
    #[test]
    fn a_driver_forwards_the_very_progress_its_cell_got() {
        /// A plugin that says where its machine is, the way `AiLoop` does.
        struct Placed;
        impl Plugin for Placed {
            fn step(&mut self, _: &dyn PaneAccess, _: &RunContext) -> Result<Step, PaneError> {
                Ok(Step::new(Cost::Bytes(3), Verdict::Continue))
            }
            fn at(&self) -> Option<&'static str> {
                Some("judging")
            }
            fn place(&self) -> Option<Vec<String>> {
                Some(vec!["judging".to_owned(), "running".to_owned()])
            }
            fn driving(&self) -> Option<PaneId> {
                None
            }
        }

        let cell: ProgressCell = ProgressCell::default();
        let heard: Arc<Mutex<Vec<Progress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let heard = Arc::clone(&heard);
            Arc::new(move |progress: &Progress| {
                heard
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(progress.clone());
            }) as ProgressSink
        };
        // ⚠ The OUTCOME is not what this measures — what the two watchers were told is.
        let _ = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&cell))
        .forwarding_to(sink)
        .run(
            &mut Placed,
            &RecordingPanes::new(),
            &RunContext::uncancellable(),
        );

        let heard = heard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let last = heard
            .last()
            .expect("⚠⚠ THE PREMISE: a run that took steps must have published at least once");
        assert_eq!(
            last.place.as_deref(),
            Some(["judging".to_owned(), "running".to_owned()].as_slice()),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 662: a driver in another process forwarded its counters and \
             NOT where its machine was. That sink is the only channel such a run has, so a place \
             left off it is a place its daemon can never learn — and item 543's whole resume ends \
             at exactly that driver. Heard: {last:?}",
        );
        assert_eq!(
            last.at,
            Some("judging"),
            "⚠⚠ and the word a person reads beside it, which is the other half of the pair a run \
             log keeps",
        );
        assert_eq!(
            *last,
            *cell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "⚠⚠⚠ AND THE TWO READERS MUST AGREE. A forwarder that assembles its own `Progress` \
             is free to drift from the cell one field at a time, and the two readers are the same \
             person looking at one run through two windows.",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE FIRST STEP TAKES THE CELL OVER FROM A DEAD DAEMON** — register item 815, and
    /// the whole reason [`Progress::inherited`] lives in this struct rather than beside it.
    ///
    /// # What it is protecting
    ///
    /// `sprag_host::runs::RunRegistry::restore` fills a run's cell from a predecessor's LOG and
    /// marks it. A boot may then put that run back on a driver of its own, and from that driver's
    /// first step the numbers are its. **Nothing clears the mark explicitly**: the cell is
    /// republished WHOLE from this driver's own fields, so the flag goes false by construction.
    ///
    /// ⚠⚠ **THAT IS A CLAIM ABOUT `publish` AND NOT ABOUT A CALLER**, which is why it is driven
    /// here. A driver in another process never touches the restored cell at all — for that arm the
    /// row reads `RunSummary::reported` instead, and `sprag-host`'s own gate holds it. This one is
    /// the arm a daemon told `run-driver-process = off` takes, where the cell IS the answer.
    ///
    /// ⚠ The cell is handed in ALREADY MARKED, which is exactly the shape `put_back` produces: it
    /// gives the thread driver the restored run's own cell rather than a fresh one, so that the row
    /// keeps showing what the predecessor managed until this driver has something to say.
    #[test]
    fn a_drivers_first_step_takes_over_a_cell_a_restore_marked_as_a_predecessors() {
        struct Stepping;
        impl Plugin for Stepping {
            fn step(&mut self, _: &dyn PaneAccess, _: &RunContext) -> Result<Step, PaneError> {
                Ok(Step::new(Cost::Bytes(1), Verdict::Continue))
            }
            fn driving(&self) -> Option<PaneId> {
                None
            }
        }

        // The cell as a restore hands it over: a predecessor's counters, marked as such.
        let cell: ProgressCell = Arc::new(Mutex::new(Progress {
            iterations: 7,
            deliveries: Deliveries {
                made: 3,
                folded: 0,
                unsubmitted: 0,
                unreported: 0,
            },
            inherited: true,
            ..Progress::default()
        }));
        assert!(
            cell.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .inherited,
            "⚠⚠ THE PREMISE: the cell must arrive marked, or what follows is a statement about a \
             flag that was never set and would hold of any writer at all",
        );

        let _ = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&cell))
        .run(
            &mut Stepping,
            &RecordingPanes::new(),
            &RunContext::uncancellable(),
        );

        let held = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert!(
            !held.inherited,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 815: this driver has taken a step and the cell still says its \
             numbers are a predecessor's. A row reading that publishes *nothing has driven this \
             run since* about a run that is working — a warning that never clears, which a reader \
             learns to skip, on the one row item 774 exists to make legible. Held: {held:?}",
        );
        assert_eq!(
            held.iterations, 1,
            "⚠⚠ AND THE COUNTERS ARE THIS DRIVER'S NOW, which is the fact the flag was describing: \
             a cell that kept the inherited 7 while clearing the flag would be worse than either.",
        );
    }

    /// A plugin that never converges, DRIVES pane 1, and can be told to block for a while — the
    /// stand-in for "a step is in flight when the run ends".
    struct Driving {
        blocks_for: Duration,
    }

    impl Plugin for Driving {
        fn step(&mut self, _panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
            poll_until(run, self.blocks_for, || false);
            Ok(Step::new(Cost::Bytes(1), Verdict::Continue))
        }
        fn driving(&self) -> Option<PaneId> {
            Some(PaneId(1))
        }
    }

    /// ⚠⚠⚠ **A RUN CUT SHORT STOPS ITS WORK; A RUN THAT ENDED ON ITS OWN TERMS TOUCHES NOTHING.**
    ///
    /// Both halves in one gate, because either alone is half a claim. Stopping on every ending
    /// would make a converged run interrupt the peer that had just answered it, and stopping on
    /// none is the defect this exists to close — a `cancelled` outcome over a model still
    /// answering.
    ///
    /// The four endings are driven through the Driver's own ceilings rather than asserted about its
    /// internals: a passed deadline and a cancel are the two that can land INSIDE a step, and the
    /// iteration ceiling and a plugin's own convergence are the two that cannot.
    #[test]
    fn only_a_run_that_was_cut_short_stops_the_work_it_had_going() {
        let bounded = |max_iterations, max_duration| Guardrails {
            max_iterations,
            max_cost: None,
            max_duration,
        };

        // 1. OUT OF TIME, inside a step that would have blocked for a minute.
        let panes = RecordingPanes::new();
        let outcome = Driver::new(bounded(100, Some(Duration::from_millis(50)))).run(
            &mut Driving {
                blocks_for: Duration::from_secs(60),
            },
            &panes,
            &RunContext::uncancellable(),
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Duration));
        assert_eq!(
            panes.asked(),
            vec![(PaneId(1), Stop::Interrupt)],
            "a run out of time stops the pane its plugin named, and asks for an INTERRUPT — the \
             turn ends and the peer stays",
        );
        assert!(
            matches!(outcome.stopped, Some(Stopped::Job(_))),
            "and the outcome SAYS so, or a caller cannot tell this from the defect: {:?}",
            outcome.stopped,
        );

        // 2. CANCELLED at the loop top, before any step.
        let panes = RecordingPanes::new();
        let cancel = Arc::new(AtomicBool::new(true));
        let outcome = Driver::new(bounded(100, None)).run(
            &mut Driving {
                blocks_for: Duration::from_millis(0),
            },
            &panes,
            &RunContext::new(cancel),
        );
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        assert_eq!(
            panes.asked().len(),
            1,
            "a cancel stops the work too — it is the other way a run ends from outside",
        );

        // 3. THE ITERATION CEILING, which is decided BETWEEN steps with nothing in flight.
        let panes = RecordingPanes::new();
        let outcome = Driver::new(bounded(1, None)).run(
            &mut Driving {
                blocks_for: Duration::from_millis(0),
            },
            &panes,
            &RunContext::uncancellable(),
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        assert_eq!(
            panes.asked(),
            Vec::new(),
            "⚠⚠ a run that spent its own budget interrupts NOTHING: its last step returned, so \
             there is no work of its to end, and signalling here would interrupt whatever the pane \
             went on to do",
        );
        assert_eq!(
            outcome.stopped, None,
            "and it has no answer to give about work it never cut short",
        );

        // 4. CONVERGED — the plugin reached its goal.
        struct Converging;
        impl Plugin for Converging {
            fn step(
                &mut self,
                _panes: &dyn PaneAccess,
                _run: &RunContext,
            ) -> Result<Step, PaneError> {
                Ok(Step::new(Cost::Bytes(1), Verdict::Converged))
            }
            fn driving(&self) -> Option<PaneId> {
                Some(PaneId(1))
            }
        }
        let panes = RecordingPanes::new();
        let outcome = Driver::new(bounded(100, None)).run(
            &mut Converging,
            &panes,
            &RunContext::uncancellable(),
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
        assert_eq!(
            panes.asked(),
            Vec::new(),
            "⚠⚠ AND A RUN THAT SUCCEEDED LEAVES THE PANE EXACTLY AS IT FOUND IT — a peer that has \
             just answered must not be interrupted for having answered",
        );
    }

    /// A run cut short by a host that CANNOT stop a pane's job says so, rather than reporting a
    /// stop it never made.
    ///
    /// ⚠ The arm every other gate here skips, because [`WorkspacePaneAccess`] offers the
    /// capability — so this is product behaviour nothing else in the crate builds, which is the
    /// shape this workspace has paid for five times.
    ///
    /// [`WorkspacePaneAccess`]: crate::access::WorkspacePaneAccess
    #[test]
    fn a_host_that_cannot_stop_a_job_reports_that_and_not_a_stop() {
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(50)),
        })
        .run(
            &mut Driving {
                blocks_for: Duration::from_secs(60),
            },
            // `NoPanes` implements the trait's minimum, so `job_control` is the default `None`.
            &NoPanes,
            &RunContext::uncancellable(),
        );
        assert_eq!(
            outcome.stopped,
            Some(Stopped::Unsupported),
            "a caller must be able to tell 'nothing was running' from 'this host cannot stop \
             things, so your work may well be'",
        );
        assert!(
            outcome
                .stopped
                .as_ref()
                .is_some_and(|stopped| stopped.to_string().contains("still running")),
            "and the sentence must say the work is still running, which is the part they act on",
        );
    }

    /// A plugin whose whole purpose is to leave panes alone gets its `None` honoured, and the
    /// outcome says the run had nothing of its own going.
    #[test]
    fn a_run_that_drove_no_job_of_its_own_says_so_rather_than_stopping_something() {
        struct DrivingNothing;
        impl Plugin for DrivingNothing {
            fn step(
                &mut self,
                _panes: &dyn PaneAccess,
                run: &RunContext,
            ) -> Result<Step, PaneError> {
                poll_until(run, Duration::from_secs(60), || false);
                Ok(Step::new(Cost::Bytes(1), Verdict::Continue))
            }
            /// NOTHING, and that is this stand-in's whole subject.
            fn driving(&self) -> Option<PaneId> {
                None
            }
        }
        let panes = RecordingPanes::new();
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(50)),
        })
        .run(&mut DrivingNothing, &panes, &RunContext::uncancellable());
        assert_eq!(
            panes.asked(),
            Vec::new(),
            "⚠⚠ NOTHING IS SIGNALLED FOR A PLUGIN THAT NAMED NO PANE — a relay reads panes a \
             person is working in, and an unrelated timeout must not reach into them",
        );
        assert_eq!(outcome.stopped, Some(Stopped::Nothing));
    }

    /// A plugin that takes the account window and then reports **its own** budget as spent — the
    /// shape `ai_loop` has, written out here so the substrate's guarantee is measured without it.
    ///
    /// ⚠ `steps_before` is what makes the ceiling bite at a known step; everything after
    /// [`ask_for_an_account`](Plugin::ask_for_an_account) is the account turn.
    struct Accounts {
        asked: Option<Ceiling>,
        after: u32,
        /// How it ends the last step of its account — a terminal verdict of its OWN, deliberately
        /// not the one it was told about: a plugin's own budget falls due inside the window as a
        /// matter of course, and the run's word must not follow it.
        ends_with: Verdict,
    }

    impl Accounts {
        /// A plugin that will take a window and then end its account with `ends_with`.
        const fn ending_with(ends_with: Verdict) -> Self {
            Self {
                asked: None,
                after: 0,
                ends_with,
            }
        }
    }

    impl Plugin for Accounts {
        fn step(&mut self, _panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
            let Some(_) = self.asked else {
                return Ok(Step::new(Cost::Bytes(1), Verdict::Continue).noting("working"));
            };
            self.after += 1;
            // ⚠ Two steps of account, so the gate can tell *the window was granted* from *one more
            // step happened to run*.
            if self.after < 2 {
                return Ok(Step::new(Cost::Bytes(1), Verdict::Continue).noting("accounting"));
            }
            Ok(Step::new(Cost::Bytes(1), self.ends_with.clone()).noting("accounted"))
        }

        fn captured(&self) -> Option<String> {
            self.asked.map(|_| "where it got to".to_owned())
        }

        fn ask_for_an_account(&mut self, ceiling: Ceiling) -> Accounting {
            self.asked = Some(ceiling);
            Accounting::Within(Duration::from_secs(30))
        }

        fn driving(&self) -> Option<PaneId> {
            None
        }
    }

    /// ⚠⚠⚠ **A RUN GIVEN A WINDOW TO ACCOUNT FOR ITSELF STILL ENDS ON THE CEILING THAT STOPPED
    /// IT** — the substrate half of register item 208, measured without a statechart in sight.
    ///
    /// # ⚠⚠⚠ Why the ceiling has to be frozen, and what would happen if it were not
    ///
    /// The account is one more turn of the plugin's OWN machine, and that machine has budgets of
    /// its own — `ai_loop.scxml` reaches its `exhausted` through the state that writes the account,
    /// so the very step handing it back reports [`Verdict::Exhausted`]. Recorded in the ordinary
    /// way that would overwrite the ceiling that actually stopped the run, and **a caller whose
    /// wall clock ran out would be told to raise a turn budget they never came near.** The
    /// stand-in claims exactly that, on purpose.
    ///
    /// ⚠⚠ THE CONTROL IS THE DEFAULT, driven in the same gate: a plugin that inherits
    /// [`Accounting::Nothing`] must end at its ceiling and not one step later, which is what every
    /// run did before this method existed and what three of the four bundled plugins still do.
    #[test]
    fn a_run_given_a_window_to_account_for_itself_still_ends_on_the_ceiling_that_stopped_it() {
        const CEILING: u32 = 3;
        let bounded = Guardrails {
            max_iterations: CEILING,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        };

        /// A plugin that inherits every default this trait has — the shape three of the four
        /// bundled ones have, and the control for the window below.
        struct NeverAsked;
        impl Plugin for NeverAsked {
            fn step(
                &mut self,
                _panes: &dyn PaneAccess,
                _run: &RunContext,
            ) -> Result<Step, PaneError> {
                Ok(Step::new(Cost::Bytes(1), Verdict::Continue))
            }
            fn driving(&self) -> Option<PaneId> {
                None
            }
        }

        let control = Driver::new(bounded).run(
            &mut NeverAsked,
            &RecordingPanes::new(),
            &RunContext::uncancellable(),
        );
        assert_eq!(
            (control.state, control.iterations),
            (OutcomeState::Exhausted(Ceiling::Iterations), CEILING),
            "⚠⚠ THE CONTROL: a plugin that inherits the default is stepped exactly as far as its \
             ceiling allows. A window granted to a plugin that never asked for one would move \
             every existing run's bound",
        );

        let mut accounting = Accounts::ending_with(Verdict::Exhausted(Ceiling::Turns));
        let outcome = Driver::new(bounded).run(
            &mut accounting,
            &RecordingPanes::new(),
            &RunContext::uncancellable(),
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Iterations),
            "⚠⚠⚠ THE RUN'S CEILING IS THE FIRST ONE THAT BOUND IT. This plugin reported `turns` on \
             the way out — as `ai_loop` genuinely does — and a Driver that took the later answer \
             would send its caller to a knob that never bound this run",
        );
        assert!(
            outcome.iterations > CEILING,
            "⚠⚠ and the window really was granted: {} of a ceiling of {CEILING}",
            outcome.iterations,
        );
        assert_eq!(
            accounting.asked,
            Some(Ceiling::Iterations),
            "⚠⚠ the plugin is TOLD which ceiling stopped it, because what it says to its peer — and \
             what it reports back — depends on which one it was",
        );
        assert_eq!(
            accounting.captured().as_deref(),
            Some("where it got to"),
            "⚠⚠⚠ and the account exists at all, which is the whole of item 208",
        );
    }

    /// ⚠⚠⚠ **AN ACCOUNT THAT GOES WRONG DOES NOT CHANGE THE ENDING** — `ai_loop.scxml`'s own rule
    /// about its `stopping` state, one layer out and enforced for every plugin.
    ///
    /// The ceiling was reached before the question was asked, and nothing that happens inside a
    /// courtesy window can un-reach it. A peer that stops to ask while writing its account, or a
    /// person who takes the pane in the middle of it, would otherwise end the run `blocked` or
    /// `taken_over` — **sending its reader off to answer a dialog when the knob they need is the
    /// ceiling they set themselves.**
    ///
    /// ⚠ Driven for both, because they are two different facts that arrive by two different
    /// verdicts, and a collapse written for one of them is a collapse that misses the other.
    ///
    /// ⚠⚠⚠ **AND `converged` IS DRIVEN AS THE COUNTER-CASE IN THE SAME GATE.** A collapse that
    /// swallowed it would tell a caller whose plugin was already writing its closing report when
    /// the ceiling fell due that their finished work ran out of time — and they would run it
    /// again. The two halves are asserted together because the rule is *which facts are about the
    /// account*, and a gate that only drove the collapsing ones would pass for a Driver that
    /// collapsed everything.
    #[test]
    fn a_run_that_stumbles_while_accounting_still_reports_the_ceiling() {
        for (ends_with, expected) in [
            (
                Verdict::Blocked(crate::consent::Unanswered::unreadable()),
                OutcomeState::Exhausted(Ceiling::Iterations),
            ),
            (
                Verdict::TakenOver(crate::readiness::Interruption::of(1)),
                OutcomeState::Exhausted(Ceiling::Iterations),
            ),
            (Verdict::Converged, OutcomeState::Converged),
        ] {
            let mut accounting = Accounts::ending_with(ends_with.clone());
            let outcome = Driver::new(Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .run(
                &mut accounting,
                &RecordingPanes::new(),
                &RunContext::uncancellable(),
            );
            assert_eq!(
                outcome.state,
                expected,
                "⚠⚠⚠ a {} verdict inside the account window sent the run to the wrong ending. A \
                 question or a hand is a fact about the ACCOUNT and cannot change a verdict already \
                 reached; reaching the GOAL is a fact about the work and must not be swallowed by a \
                 window granted after it",
                ends_with.wire_str(),
            );
        }
    }

    /// ⚠⚠ **EVERY WAY A RUN'S WORK CAN END READS AS ITS OWN SENTENCE**, and the two that mean the
    /// work is STILL RUNNING say so in words.
    ///
    /// Driven from [`Stopped::ALL`], so a fifth arm is covered the day it is declared — the reason
    /// this type is a `closed_set!` at all, and the difference between a vocabulary with a ratchet
    /// and one with a hand-written list of four beside it.
    ///
    /// ⚠ The DISTINCTNESS is the half a per-arm check cannot make. A single polite sentence would
    /// satisfy every shape claim here while telling four outcomes apart from none — and two of these
    /// four are the ones a caller has to act on.
    #[test]
    fn every_end_of_a_runs_work_reads_as_its_own_sentence() {
        let every = Stopped::ALL;
        let distinct: std::collections::BTreeSet<String> =
            every.iter().map(ToString::to_string).collect();
        assert_eq!(
            distinct.len(),
            every.len(),
            "two endings read as the SAME sentence, so a caller cannot tell them apart: \
             {distinct:?}",
        );
        for stopped in &every {
            let said = stopped.to_string();
            assert_ne!(
                said,
                format!("{stopped:?}"),
                "the published text is the DEBUG form, which is the leak itself",
            );
            assert!(
                said.contains(' ') && said.starts_with(char::is_lowercase),
                "an answer an agent reads must be prose, not {said:?}",
            );
        }
        // ⚠⚠ AND THE TWO THAT LEAVE WORK BEHIND MUST SAY SO. This is the whole point of the type:
        // a caller whose stop did not land is the one who has a peer still spending their money,
        // and a sentence that reported the failure without that fact would be a diagnostic about
        // the past rather than a warning about the present.
        for leftover in [
            Stopped::Unreached(PaneError::NotStopped(Unstopped::Unseen)),
            Stopped::Unsupported,
        ] {
            let said = leftover.to_string();
            assert!(
                said.contains("still running"),
                "{leftover:?} must tell the caller their work is still going: {said:?}",
            );
        }
        // And the two that do NOT must not scare somebody into looking for work that is over.
        for settled in [
            Stopped::Job(Signalled {
                stop: Stop::Interrupt,
                pgid: 4711,
                leader: None,
            }),
            Stopped::Nothing,
        ] {
            assert!(
                !settled.to_string().contains("still running"),
                "{settled:?} must not read as a warning: {}",
                settled,
            );
        }
    }

    /// ⚠⚠⚠ **A RUN THAT PANICS IS THE FOURTH WAY ONE ENDS, AND IT LEFT ITS WORK RUNNING.**
    ///
    /// The host has a whole state for it — `RunState::Panicked`, raised when a worker thread's
    /// join comes back `Err` — so this is not a hypothetical: it is an ending the product already
    /// names and reports. And a panic UNWINDS past the end of [`Driver::run`], so the stop that
    /// every other cut-short ending performs never ran.
    ///
    /// ⚠ The three endings that leave nothing behind were checked rather than assumed:
    /// `Converged` and the per-step ceilings are decided with the last step returned, and a
    /// `Failed` one cannot have work in flight because **every plugin's only fallible call after
    /// its readiness barrier is the inject itself** — read across all four, and everything after it
    /// is `Ok`. A panic is the one ending where a step is abandoned MID-FLIGHT.
    ///
    /// ⚠ REVERT-PROOF: remove the unwind guard from `Driver::run` and the `sleep` outlives the run.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_run_that_panics_mid_step_still_stops_the_work_it_had_going() {
        use crate::access::WorkspacePaneAccess;
        use sprag_terminal::{CommandBuilder, Workspace};
        use std::time::Instant;

        let workspace = Arc::new(Mutex::new(Workspace::new((60, 8))));
        let mut command = CommandBuilder::new("/bin/bash");
        command.arg("--norc");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        let pane = workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .spawn(command, "bash".to_string(), 60, 8)
            .expect("spawn pane");
        let child = workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pane(pane)
            .and_then(|held| held.pty().pid())
            .expect("a live child");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let until = |within: Duration, mut ready: Box<dyn FnMut() -> bool>| {
            let start = Instant::now();
            while start.elapsed() < within {
                if ready() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| sprag_terminal::foreground_leader_of(child)
                    .is_some_and(|job| job.pid == child)),
            ),
            "the shell must reach its own prompt first",
        );

        /// Types a job into the pane, waits for it to own the terminal, then PANICS.
        struct PanicsMidStep(PaneId);
        impl Plugin for PanicsMidStep {
            fn step(
                &mut self,
                panes: &dyn PaneAccess,
                _run: &RunContext,
            ) -> Result<Step, PaneError> {
                let mut keys = crate::access::KeyStroke::text("sleep 300");
                keys.push(crate::access::KeyStroke::named("Enter"));
                let _typed = panes.inject(self.0, &keys)?;
                let jobs = panes
                    .foreground_job()
                    .expect("this host reads the job table");
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(15) {
                    if jobs
                        .pane_foreground_leader(self.0)
                        .is_some_and(|job| job.name == "sleep")
                    {
                        // ⚠ THE PANIC IS RAISED WITH THE JOB PROVABLY RUNNING, so a gate that
                        // passes cannot be passing because nothing had started.
                        panic!("a plugin bug, raised while its peer is working");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                panic!("the job never started, so this measured nothing");
            }
            fn driving(&self) -> Option<PaneId> {
                Some(self.0)
            }
        }

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Driver::new(Guardrails {
                max_iterations: 4,
                max_cost: None,
                max_duration: None,
            })
            .run(
                &mut PanicsMidStep(pane),
                &access,
                &RunContext::uncancellable(),
            )
        }));
        assert!(
            unwound.is_err(),
            "the panic must still reach the caller — the host turns it into RunState::Panicked, \
             and swallowing it here would trade a leak for a lie",
        );
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| !sprag_terminal::foreground_leader_of(child)
                    .is_some_and(|job| job.name == "sleep")),
            ),
            "⚠⚠ A PANICKED RUN MUST NOT LEAVE ITS PEER WORKING — that is the unbounded loop the \
             whole guardrail apparatus exists to prevent, reached by the one path that skips it",
        );
    }

    /// A plugin whose step always fails — to pin the Driver's Err -> Failed
    /// mapping deterministically, no threads or PTY.
    struct FailingPlugin;
    impl Plugin for FailingPlugin {
        fn step(&mut self, _panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
        /// A step that fails before it acts has nothing running.
        fn driving(&self) -> Option<PaneId> {
            None
        }
    }

    /// A plugin that fails **at a named place, after walking** — the shape a real run has when it
    /// dies, which [`FailingPlugin`] deliberately does not.
    ///
    /// ⚠ `at` and `walked` are what a live `ai_loop` carries into its failure and what a reader
    /// needs from it; a fixture with neither could not tell a journal that records the place from
    /// one that records the word `failed` and nothing else.
    struct FailingSomewhere;
    impl Plugin for FailingSomewhere {
        fn step(&mut self, _panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
            Err(PaneError::UnknownPane(PaneId(7)))
        }
        fn driving(&self) -> Option<PaneId> {
            Some(PaneId(7))
        }
        fn at(&self) -> Option<&'static str> {
            Some("judging")
        }
        fn walked(&self) -> Vec<crate::plugin::Edge> {
            vec![crate::plugin::Edge {
                from: "working",
                raised: "TurnDone",
                to: "judging",
            }]
        }
    }

    /// A plugin that takes ONE step driving `pane`, then cannot take another because `raises` is
    /// not a pane its workspace holds — **the shape a live run has when its pane goes missing
    /// mid-flight**, which [`FailingSomewhere`] deliberately is not (that one dies on its first
    /// step, so nothing was ever being driven).
    struct DrivesThenLosesIt {
        pane: PaneId,
        raises: PaneId,
        stepped: bool,
    }
    impl Plugin for DrivesThenLosesIt {
        fn step(&mut self, _panes: &dyn PaneAccess, _run: &RunContext) -> Result<Step, PaneError> {
            if self.stepped {
                return Err(PaneError::UnknownPane(self.raises));
            }
            self.stepped = true;
            Ok(Step::new(Cost::Bytes(1), Verdict::Continue))
        }
        fn driving(&self) -> Option<PaneId> {
            Some(self.pane)
        }
        fn at(&self) -> Option<&'static str> {
            Some("working")
        }
    }

    /// Run `plugin` to its ending and answer the last line it wrote in its own journal.
    fn last_line(plugin: &mut dyn Plugin) -> String {
        let cell: ProgressCell = Arc::new(Mutex::new(Progress::default()));
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&cell))
        .run(plugin, &NoPanes, &RunContext::uncancellable());
        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "these fixtures all end in a failed step: {outcome:?}",
        );
        cell.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .journal
            .last()
            .and_then(|step| step.note.clone())
            .expect("register item 680's failure line carries a sentence")
    }

    /// The clause register item 682 added — quoted once, so the gate below and the product cannot
    /// drift about what is being asserted.
    const ONE_WINDOWS_POOL: &str = "one window's pool";

    /// ⛔⛔⛔⛔⛔ **A RUN WHOSE PANE WENT MISSING SAYS THE PANE LEFT, NOT THAT IT NEVER EXISTED** —
    /// register item 682's repair (a).
    ///
    /// # ⚠⚠⚠⚠⚠ The day this sentence would have saved
    ///
    /// 2026-08-25: three runs ended `failed: there is no pane N`. Somebody went and looked, and
    /// **pane `5` was right there in window `pinion`, its child up for 2h40m.** The obvious reading
    /// — *the run is wrong about its own death* — is the wrong one, and it cost a day: a run's pool
    /// is ONE WINDOW's, so *this workspace has no such pane* and *the session has no such pane* are
    /// different sentences, and only the first was ever true.
    ///
    /// [`PaneError::UnknownPane`]'s own words are right for the caller it was written for, who
    /// mistyped an id. This is the reading a RUN has to add, because a run's pane was checked into
    /// its pool before it started (`sprag_host`'s `require_pane_in`, gated there) — so for a run
    /// the id was never in doubt and the only thing that can have happened is that it LEFT.
    ///
    /// # ⚠⚠⚠ The two controls, and neither is decoration
    ///
    /// * **A run driving NO pane says nothing extra.** There is no pane to point a reader at, and
    ///   this is the arm that stops the clause becoming a suffix every `UnknownPane` carries.
    /// * **A run that dies reaching for a DIFFERENT pane says nothing extra.** A judging pass opens
    ///   a checker's pane of its own; a failure naming THAT is not the run's pane going missing,
    ///   and a clause that could not tell them apart would send a reader to the wrong window.
    ///
    /// ⚠ The clause is ADDED to the error's own sentence, never instead of it — asserted here, so
    /// the tie between a dead run and the incident that produced it survives this repair.
    #[test]
    fn a_run_whose_pane_went_missing_says_it_left_rather_than_that_it_never_was() {
        // ── 1. THE CONTROLS, AND THEY COME FIRST ──
        //
        // ⚠⚠ `FailingPlugin` drives NOTHING, so there is no pane a reader could be sent to look for.
        let drives_nothing = last_line(&mut FailingPlugin);
        assert!(
            drives_nothing.contains("there is no pane 0"),
            "⚠⚠ THE CONTROL'S OWN PRECONDITION: this fixture must still reach item 680's line, or \
             the absence below is about a journal that says nothing at all: {drives_nothing:?}",
        );
        assert!(
            !drives_nothing.contains(ONE_WINDOWS_POOL),
            "⚠⚠⚠⚠ A RUN DRIVING NO PANE HAS NOWHERE TO SEND ANYBODY. Saying it anyway would make \
             the clause a suffix every unknown-pane failure carries, which is a sentence that has \
             stopped distinguishing anything: {drives_nothing:?}",
        );

        // ⚠⚠⚠ AND A RUN THAT DIES REACHING FOR SOMEBODY ELSE'S PANE — the milestone checker opens
        // one of its own inside the judging pass, and a failure naming that is not this.
        let another_pane = last_line(&mut DrivesThenLosesIt {
            pane: PaneId(7),
            raises: PaneId(9),
            stepped: false,
        });
        assert!(
            another_pane.contains("there is no pane 9"),
            "⚠⚠ the control's precondition again: {another_pane:?}",
        );
        assert!(
            !another_pane.contains(ONE_WINDOWS_POOL),
            "⚠⚠⚠⚠⚠ **THE DISCRIMINATION.** This run drives pane 7 and the failure names 9, so \
             nothing of THIS RUN'S is missing — a clause here would send a person looking for a \
             pane 7 that never moved: {another_pane:?}",
        );

        // ── 2. THE MEASUREMENT: the pane it IS driving, gone from under it ──
        let lost = last_line(&mut DrivesThenLosesIt {
            pane: PaneId(7),
            raises: PaneId(7),
            stepped: false,
        });
        assert!(
            lost.contains("there is no pane 7"),
            "⚠⚠⚠⚠⚠ **THE ERROR'S OWN WORDS MUST SURVIVE THE REPAIR.** They are what ties a dead \
             run to the incident that produced it; a clause that REPLACED them would make every \
             record written before today unmatchable: {lost:?}",
        );
        assert!(
            lost.contains(ONE_WINDOWS_POOL),
            "⚠⚠⚠⚠⚠ **AND THE READING A RUN OWES ITS READER.** Without it a person who goes and \
             finds the pane alive concludes the run is lying about its own death — measured, and \
             it cost a day: {lost:?}",
        );
        assert!(
            lost.contains("another window"),
            "⚠⚠⚠ **AND THE DIRECTION TO LOOK, which is the whole value.** A pool is one WINDOW's, \
             so a pane this one has not got may be alive in the next — that is the fact a reader \
             needs and the one this layer can honestly hedge at: {lost:?}",
        );

        // ── 3. AND A RUN THAT NEVER COMPLETED A STEP STILL SAYS IT ──
        //
        // ⚠⚠⚠⚠⚠ **THIS ARM IS THE ONE THAT WAS MEASURED INTO EXISTENCE.** The first draft guarded
        // on `Driver::driving`, which is refreshed only after a step COMPLETES — so a driver handed
        // a plugin already mid-flight never set it and the clause vanished on the one gate that
        // reconstructs the live incident. The rule is about the pane the run IS DRIVING, not about
        // whether this driver has banked a pass, and `FailingSomewhere` fails on its very first
        // step while driving pane 7.
        let first_step = last_line(&mut FailingSomewhere);
        assert!(
            first_step.contains(ONE_WINDOWS_POOL),
            "⚠⚠⚠⚠ a run that dies on its FIRST step is still driving a pane, and its reader still \
             needs telling which window to look in: {first_step:?}",
        );
    }

    /// ⛔⛔⛔⛔ **A RUN THAT DIES SAYS WHERE IT WAS AND WHAT IT WAS DOING** — register item 680.
    ///
    /// # ⚠⚠⚠⚠⚠ The gap this closes, measured on three live runs in one morning
    ///
    /// 2026-08-25: runs `0`, `1` and `3` all ended `failed: there is no pane N`, all three with
    /// `Working --TurnDone--> Judging` as their LAST walk entry and nothing after it. The pane each
    /// named was alive at the time and still is. **Nothing anywhere said which call raised it**, so
    /// a deterministic defect (register item 682) could be narrowed by elimination and never
    /// confirmed — the daemon log was silent for the whole 42 minutes around it.
    ///
    /// The cause is three lines: the driver's loop calls [`Driver::record`] only on the `Ok` arm.
    /// A step that returns `Err` sets `failure` — which becomes the run's one-line summary — and
    /// **writes no journal entry at all**. Register item 505 built exactly this legibility for a
    /// failure the DOCUMENT answers (*"The pass that ended the run: `<state> --<event>--> Failed`"*)
    /// and an I/O failure never reaches the document, so it never got one.
    ///
    /// # ⚠⚠ What the claim is, and why the fixture carries a place and a walk
    ///
    /// Not *"the run failed"* — [`driver_maps_a_step_error_to_failed_with_the_cause`] already holds
    /// that, and held it while every one of those three runs died unreadably. The claim is that the
    /// JOURNAL — the thing a person reads — carries **where** and **what**. So the fixture fails at
    /// a named place (`judging`) after a named edge, and the assertions are about both.
    #[test]
    fn a_run_that_dies_leaves_the_place_and_the_reason_in_its_own_journal() {
        // ⚠ The journal is the RUNNING run's record ([`Progress`]), not the outcome's — which is
        // itself part of what makes this gap easy to miss: a reader who has only the `Outcome` has
        // only the summary line, and the place where the "where" belongs is a cell somebody else
        // holds.
        let cell: ProgressCell = Arc::new(Mutex::new(Progress::default()));
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&cell))
        .run(
            &mut FailingSomewhere,
            &NoPanes,
            &RunContext::uncancellable(),
        );

        assert_eq!(outcome.state, OutcomeState::Failed, "{outcome:?}");

        let progress = cell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let last = progress.journal.last().unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔⛔ REGISTER ITEM 680: a run that died wrote NOTHING to its journal. Three \
                 live runs ended this way on 2026-08-25 and the defect that killed them could not \
                 be diagnosed, because the walk stops at the last step that SUCCEEDED and the \
                 failure is a summary line with no place in it. Outcome: {outcome:?}",
            )
        });

        assert!(
            last.note
                .as_deref()
                .is_some_and(|note| note.contains("there is no pane 7")),
            "⚠⚠⚠ the journal's last line must carry the REASON in the error's own words — a line \
             that says only `failed` sends a reader to the daemon log, which was silent: {last:?}",
        );
        assert!(
            last.note
                .as_deref()
                .is_some_and(|note| note.contains("judging")),
            "⚠⚠⚠⚠ and WHERE it was, in the document's own word (`Plugin::at`, register item 543). \
             Without it a reader knows a call failed and not which of a machine's states was \
             taking it — which is exactly how item 682 stayed a hypothesis: {last:?}",
        );
        assert_eq!(
            last.walked,
            vec![crate::plugin::Edge {
                from: "working",
                raised: "TurnDone",
                to: "judging",
            }],
            "⚠⚠ and the edges the plugin walked getting there, which the `Ok` arm already records \
             — a failure that dropped them would be the only step in the journal that cannot say \
             how it arrived: {last:?}",
        );
    }

    /// An empty pane access (the failing plugin ignores it).
    struct NoPanes;
    impl PaneAccess for NoPanes {
        fn pane_ids(&self) -> Vec<PaneId> {
            Vec::new()
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            None
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            None
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
    }

    #[test]
    fn driver_maps_a_step_error_to_failed_with_the_cause() {
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut FailingPlugin, &NoPanes, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Failed);
        assert_eq!(outcome.iterations, 0);
        assert_eq!(outcome.failure, Some(PaneError::UnknownPane(PaneId(0))));
    }

    /// ⚠⚠ **A LONG RUN'S JOURNAL IS BOUNDED, AND ITS TOTAL IS NOT** — the branch every run under
    /// sixty-four steps skips, which is every run any other gate in this workspace drives.
    ///
    /// A journal that grew with the run would make a hundred-thousand-iteration loop's memory grow
    /// with it, and one that silently truncated would leave a reader unable to tell a complete
    /// account from a clipped one. Both halves are asserted: the LAST steps are what is kept (the
    /// end of a run is what *"why did it not converge?"* is asked about), and `iterations` still
    /// counts every step, which is how the two are told apart.
    #[test]
    fn a_journal_keeps_its_last_steps_and_never_loses_the_count() {
        struct Counting(u32);
        impl Plugin for Counting {
            fn step(
                &mut self,
                _panes: &dyn PaneAccess,
                _run: &RunContext,
            ) -> Result<Step, PaneError> {
                self.0 += 1;
                Ok(Step::new(Cost::Bytes(1), Verdict::Continue).noting(format!("step {}", self.0)))
            }
            /// It touches no pane at all — the journal is its whole subject.
            fn driving(&self) -> Option<PaneId> {
                None
            }
        }

        let steps = u32::try_from(JOURNAL_LIMIT).expect("the limit fits a step count") + 10;
        let cell = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: steps,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut Counting(0), &NoPanes, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        assert_eq!(
            outcome.iterations, steps,
            "the TOTAL counts every step, which is what makes a clipped journal detectable",
        );
        let held = cell.lock().expect("the progress cell");
        assert_eq!(
            held.journal.len(),
            JOURNAL_LIMIT,
            "the journal is bounded, or a long run's memory grows with it",
        );
        assert_eq!(
            held.journal.last().and_then(|last| last.note.clone()),
            Some(format!("step {steps}")),
            "and it keeps the LAST steps: the end of a run is what its journal is read for",
        );
        assert_eq!(
            held.journal.first().map(|first| first.iteration),
            Some(steps - u32::try_from(JOURNAL_LIMIT).expect("fits") + 1),
            "the oldest kept step is exactly the limit back from the newest",
        );
    }

    /// A plugin whose step CANNOT end on its own — it waits on a predicate that never holds,
    /// under a local bound far past any deadline a test arms. The shape of a real turn a model is
    /// still thinking about when the clock runs out, with none of a PTY's timing.
    ///
    /// The only way out is [`RunContext::stopped`], so every gate built on it measures the
    /// deadline reaching INSIDE a step and nothing else — no machine-speed race can end it early.
    struct Blocking {
        /// Raised by the step itself on the iteration named, standing in for a person hitting
        /// stop while a turn is in flight.
        cancel_on: Option<(u32, Arc<AtomicBool>)>,
        stepped: u32,
    }
    impl Plugin for Blocking {
        fn step(&mut self, _panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
            self.stepped += 1;
            if let Some((at, flag)) = &self.cancel_on
                && self.stepped == *at
            {
                flag.store(true, Ordering::Release);
            }
            let waited = poll_until(run, Duration::from_secs(600), || false);
            Ok(Step::new(Cost::Bytes(1), Verdict::Continue).noting(format!("{waited:?}")))
        }
        /// It blocks against no pane at all — its subject is the clock.
        fn driving(&self) -> Option<PaneId> {
            None
        }
    }

    /// ⚠⚠ **THE DEADLINE REACHES INSIDE A STEP** — the load-bearing half of the duration ceiling,
    /// and until now NO test armed `max_duration` at all.
    ///
    /// The step here can only end by the run's own clock: its local bound is ten minutes and its
    /// predicate never holds. So a run that ends at all proves the deadline reached the wait, and
    /// the iteration ceiling is left far away so that `Duration` is the only ceiling in reach.
    #[test]
    fn a_run_out_of_time_inside_a_step_ends_by_the_clock_and_says_so() {
        let outcome = Driver::new(Guardrails {
            max_iterations: 1_000,
            max_cost: None,
            max_duration: Some(Duration::from_millis(50)),
        })
        .run(
            &mut Blocking {
                cancel_on: None,
                stepped: 0,
            },
            &NoPanes,
            &RunContext::uncancellable(),
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "a step that cannot return on its own ended, so the deadline reached the wait inside \
             it — and the ceiling the run names is the clock",
        );
    }

    /// ⚠⚠ **THE CLOCK THAT CURTAILED A STEP OUTRANKS THE TALLY THAT TOPPED OUT** — when the
    /// deadline passes inside the run's LAST permitted turn, both ceilings are true at once and
    /// only one of them is a useful thing to tell a caller.
    ///
    /// The step here is cut off mid-flight by the clock, and returns as the iteration count
    /// reaches its max. Answering `iterations` says *"you got your turn, ask for more"* about a
    /// turn that never finished — and raising `max_iterations` would not buy the run one more
    /// second. The ceiling that stopped WORK IN FLIGHT is the one that stopped the run; a tally
    /// reached on the way out is a coincidence of arithmetic.
    #[test]
    fn the_clock_that_curtailed_a_step_outranks_the_tally_that_topped_out() {
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: Some(Duration::from_millis(50)),
        })
        .run(
            &mut Blocking {
                cancel_on: None,
                stepped: 0,
            },
            &NoPanes,
            &RunContext::uncancellable(),
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "the deadline is what ended the only step this run was allowed, so it is what the run \
             ran out of — `iterations` here is a remedy that would buy nothing",
        );
    }

    /// ⚠⚠ **A PERSON'S STOP IS NEVER REPORTED AS A BUDGET** — a cancel raised while the run's last
    /// permitted turn is in flight.
    ///
    /// The loop top asks about cancel BEFORE the ceilings, so every cancel that lands between
    /// steps is answered `cancelled`. The one that lands INSIDE a step was decided somewhere else
    /// entirely — by the post-step tally, which had never been told cancel exists. A person who
    /// hit stop being told the run ran out of turns is a lie about who ended it, and it points the
    /// reader at a guardrail to raise when nothing was exhausted at all.
    #[test]
    fn a_person_who_stops_the_last_permitted_turn_is_not_told_it_ran_out() {
        let cancel = Arc::new(AtomicBool::new(false));
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: None,
        })
        .run(
            &mut Blocking {
                cancel_on: Some((1, Arc::clone(&cancel))),
                stepped: 0,
            },
            &NoPanes,
            &RunContext::new(cancel),
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "the run was stopped by a person mid-turn; the iteration count reaching its max on the \
             way out does not make that an exhaustion",
        );
    }

    /// ⚠⚠⚠⚠ **A PERSON'S STOP OUTRANKS THE CLOCK WHEN BOTH ARE TRUE AT ONCE — AND THE CANCELLED
    /// RUN IS NOT GRANTED A WINDOW TO ACCOUNT FOR ITSELF.**
    ///
    /// Its neighbour above holds cancel against the TALLY. This holds it against the DEADLINE, and
    /// the two are not the same claim: `ended_from_outside` asks both, in one order, and swapping
    /// them passed every gate in this crate.
    ///
    /// # ⚠⚠⚠ Why the order is worth a gate rather than a comment
    ///
    /// An expired run is not merely renamed — it is offered an ACCOUNT WINDOW
    /// ([`spend_or_account`](Driver::spend_or_account)), which moves the run's deadline and steps
    /// the plugin again. So with the wrong order a person's stop would (1) be reported as
    /// `exhausted (duration)`, pointing the reader at a `max_seconds` that would have bought
    /// nothing about a run somebody deliberately ended, and (2) let that run **go on working after
    /// they said stop**, for as long as its own account takes.
    ///
    /// ⚠⚠ AND THAT SECOND HALF IS A SHUTDOWN'S PROBLEM, which is how this was found: `cancel_all`
    /// raises the flag on EVERY run, so at a shutdown any run already past its `max_duration` would
    /// take an account window while `RunRegistry`'s join waited on it.
    #[test]
    fn a_run_cancelled_after_its_clock_ran_out_is_not_granted_an_account_window() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut plugin = Accounts::ending_with(Verdict::Converged);
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            // Already past at the first loop top, so BOTH endings are true in the same instant —
            // which is the only arrangement that can tell the two orders apart.
            max_duration: Some(Duration::ZERO),
        })
        .run(&mut plugin, &NoPanes, &RunContext::new(cancel));

        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "a person stopped this run; `exhausted (duration)` names a knob that would have bought \
             them nothing",
        );
        assert!(
            plugin.asked.is_none(),
            "⚠⚠⚠⚠ THE CANCELLED RUN WAS ASKED FOR AN ACCOUNT, so it was given a fresh deadline and \
             kept working after a person said stop — and a shutdown waiting on it waits for that \
             too. It was asked about: {:?}",
            plugin.asked,
        );
        assert_eq!(
            outcome.iterations, 0,
            "and no step ran: the account window is the only thing that could have produced one",
        );
    }

    #[test]
    fn driver_ends_cancelled_without_running_a_step() {
        // The plugin would fail if stepped, but a pre-raised cancel pre-empts
        // it at the loop top: Cancelled, zero iterations, no failure recorded.
        let cancel = Arc::new(AtomicBool::new(true));
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut FailingPlugin, &NoPanes, &RunContext::new(cancel));
        assert_eq!(outcome.state, OutcomeState::Cancelled);
        assert_eq!(outcome.iterations, 0);
        assert!(outcome.failure.is_none());
    }
}
