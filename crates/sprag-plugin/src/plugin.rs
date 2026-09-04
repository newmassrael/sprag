//! `Plugin` — the control-plugin extension point.
//!
//! A plugin owns its perceive + act + judge behaviour; the [`Driver`] owns the
//! statechart lifecycle and the guardrails around each [`step`](Plugin::step).
//! That is the SOLID seam: what is uniform (termination topology, guardrails,
//! outcome mapping) lives in the Driver; what is plugin-specific (when/how to
//! read a pane, what to inject, when to converge) lives here.
//!
//! [`Driver`]: crate::driver::Driver

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::run::RunContext;

/// A plugin's verdict for one step.
///
/// ⚠ NOT `Copy`, because [`Blocked`](Self::Blocked) carries the question. That is
/// [`OutcomeState::Exhausted`](crate::driver::OutcomeState::Exhausted)'s rule — the reason is
/// carried INSIDE, so a verdict that does not say what it is blocked on cannot be constructed —
/// and the convenience of `Copy` is not worth a second field somebody can forget to set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep going (subject to the Driver's guardrails).
    Continue,
    /// The plugin reached its goal; the run converges.
    Converged,
    /// **The peer stopped to ASK, so the run stops rather than typing at it.**
    ///
    /// Carries what it is asking, when this host can read the question. `None` is a real answer
    /// and not a gap — an agent can block on something that is not a numbered list, and
    /// [`AgentObservation::asking`](crate::access::AgentObservation::asking) records the remedy:
    /// hand the pane to a person.
    ///
    /// # ⚠⚠⚠ Why a verdict of its own rather than a `Continue` with a note
    ///
    /// An agent that stops to ask shows a bottom-anchored NUMBERED CHOICE LIST, and a numbered
    /// list consumes keystrokes: what a loop types into one is not text, it is a SELECTION. Every
    /// injection these plugins make ends with Enter, and `Question::selected` is *"where a bare
    /// Enter would land, and so the answer a caller gets by doing nothing"* — so a loop that kept
    /// going would confirm whatever option the agent had highlighted, which on a tool-permission
    /// dialog is an approval nobody read.
    ///
    /// Measured before this existed: an orchestrator whose peer blocked after the first step typed
    /// its stimulus three more times and reported `Exhausted(Iterations)` — the answer that tells
    /// a reader to raise a budget.
    Blocked(crate::consent::Unanswered),
    /// **THE PEER ASKED AND THIS STEP ANSWERED IT**, on a [`Consent`](crate::consent::Consent) the
    /// caller declared before the run started. The run continues.
    ///
    /// # ⚠⚠⚠ Why an approval given by a machine gets a WORD OF ITS OWN
    ///
    /// For control flow this is a `Continue` — the run goes on, guardrails and all — and it would
    /// have been one line shorter to say so. But the step DECIDED SOMETHING ON A PERSON'S BEHALF,
    /// and a run whose journal spells that `continue` is a run in which approvals are indexed by
    /// nothing. The word is what makes *"which of my runs answered a dialog, and what did they
    /// say?"* a question the journal can be asked, and it costs one arm of an exhaustive match.
    ///
    /// This is [`Stopped`](crate::driver::Stopped)'s argument at the other end of the run: an act
    /// with consequences outside the loop must be reportable in the loop's own vocabulary, or the
    /// only record of it is prose.
    Answered(crate::consent::Answered),
    /// **A PERSON TOOK THIS PANE**, so the run stopped driving it. See
    /// [`Reached::Interrupted`](crate::readiness::Reached::Interrupted).
    ///
    /// # ⚠⚠⚠ Why not a flavour of [`Blocked`](Self::Blocked)
    ///
    /// They are opposite facts wearing a similar shape. `Blocked` is *the PEER stopped and wants an
    /// answer nobody has given*; this is *a PERSON is here and already acting*. A reader told the
    /// first goes looking for a dialog to answer; a reader told the second must do nothing at all,
    /// because the one thing the pane does not need is another party typing into it.
    ///
    /// Collapsing them would also make the run's own report false in the direction that matters:
    /// `blocked` says nobody came, and somebody did.
    ///
    /// ⚠ It is a verdict rather than a note on a `Continue` for [`Answered`](Self::Answered)'s
    /// reason — something happened to this run that its journal has to be askable about — and a
    /// TERMINAL one for [`Blocked`](Self::Blocked)'s: the pane now belongs to somebody else.
    TakenOver(crate::readiness::Interruption),
    /// **THE PLUGIN'S OWN DECLARED BUDGET IS SPENT**, and which one — see
    /// [`Ceiling::Turns`](crate::driver::Ceiling::Turns).
    ///
    /// # ⚠⚠⚠ Why a plugin may say `exhausted` at all, when the Driver owns exhaustion
    ///
    /// The [`Driver`](crate::driver::Driver)'s three [`Guardrails`](crate::driver::Guardrails)
    /// bound a run in the substrate's own units — steps, spend, seconds — and they are the only
    /// budgets it can see. A plugin whose DOCUMENT carries a budget of its own counts something
    /// the substrate has no name for: `ai_loop.scxml`'s `max_turns` counts the inner agent's
    /// TURNS, and one turn is many steps of the loop driving it.
    ///
    /// Before this arm the only endings available to such a plugin were a lie in one direction or
    /// the other. [`Converged`](Self::Converged) says the goal was reached and it was not.
    /// `Continue` hands the run back to the guardrails, so a loop that spent its author's eight
    /// turns would go on pumping a machine that is already in a final state until the iteration
    /// ceiling bit — reporting `exhausted — iterations`, which tells the reader to raise a number
    /// that would have bought them nothing. The remedy for THIS exhaustion is in the brief.
    ///
    /// ⚠ It is TERMINAL, and it does not outrank the two verdicts above it: a peer that stopped to
    /// ask, or a person at the pane, is a fact about somebody else and this is a fact about
    /// arithmetic. The Driver's ordering already says so — a step reports one verdict, and a
    /// plugin that can see both reports the other one.
    Exhausted(crate::driver::Ceiling),
    /// ⚠⚠⚠ **THE PEER ASKED, THIS STEP REFUSED THE CALL AND TOLD IT WHAT TO DO INSTEAD**, on a
    /// standing instruction the loop's author wrote — see [`ScreenRules`](crate::screen::ScreenRules).
    /// The run continues.
    ///
    /// # ⚠⚠⚠ Why this is not [`Answered`](Self::Answered), when both are decisions
    ///
    /// They are two different decisions and a reader has to be able to count them apart. `answered`
    /// TAKES AN OFFERED OPTION — usually an approval, which is the thing a person auditing a run
    /// most needs indexed. This one **turns the tool call down**: measured against a live `claude`
    /// 2.1.232, the key it presses produces `User rejected write to PROBE.txt` and the file is
    /// never created.
    ///
    /// A run that spelled both `answered` would answer *"what did this run approve?"* with a number
    /// that includes every refusal, which is the opposite fact.
    ///
    /// ⚠ It is also not a `Continue` with a note, for `Answered`'s reason exactly: an act with
    /// consequences outside the loop must be reportable in the loop's own vocabulary, or the only
    /// record of it is prose. **This one has more consequence than an approval, not less** — the
    /// caller's agent was stopped from doing something it had decided to do.
    Screened(crate::screen::Screened),
    /// ⚠⚠⚠⚠ **THE PANE'S PROGRAM HAS EXITED, SO THIS RUN HAS NO PEER LEFT TO DRIVE** — and which
    /// pane, because a run stopped without naming one is the defect R396-R399 spent four rounds on.
    ///
    /// # ⚠⚠⚠ Why the word had to exist before the guard could be written
    ///
    /// The fix for the 43-hour wedge (register items 304, 309, 310) is one reading: do not type at
    /// a pane the product can already report as dead. Writing it stopped at the RETURN VALUE.
    /// [`Blocked`](Self::Blocked) carries an [`Unanswered`](crate::consent::Unanswered) — *a consent
    /// failed to cover a dialog* — and a dead child asked nothing. [`TakenOver`](Self::TakenOver) is
    /// *a person took this pane*, and nobody did. [`Exhausted`](Self::Exhausted) sends a reader to
    /// raise a budget that would buy them nothing. Substituting `Continue` compiles and leaves the
    /// run stepping for ever over a peer that cannot answer. **The compiler refused the mutation,
    /// which is how this crate learns that what is missing is a word** (register item 326).
    ///
    /// # ⚠⚠ What a reader is being told, which is why it is not a flavour of the four above
    ///
    /// Not *fix your run* — nothing here is broken. Not *answer a question* — none was asked. Not
    /// *do nothing* — the work has stopped. It is **the program you asked this run to drive is no
    /// longer running**, and the remedy is outside the run entirely: find out why the agent left,
    /// and start it again.
    ///
    /// ⚠⚠⚠ TERMINAL, and that is the whole of the repair. A run that goes on stepping puts its
    /// stimulus in at the start of every step — measured at 5 bytes and 509 ms a step, so **3,380
    /// steps, about 29 minutes, from a dead peer to a pseudoterminal that blocks for ever** (item
    /// 325). Not a burst; a patient march, which is why nobody saw the 43 hours being spent.
    ///
    /// ⚠ It arrives two ways and both are one fact: [`PaneAccess::inject`]
    /// refuses a write at such a pane ([`PaneError::PeerGone`]),
    /// and [`Over::PeerGone`](crate::completion::Over::PeerGone) ends a turn nobody is left to
    /// finish. The first is about a run that was about to type; the second about one already
    /// waiting.
    PeerGone(PaneId),
    /// **SOMEBODY HELD THIS RUN AND DID NOT COME BACK** inside the loop document's own
    /// `hold_within_ms`. Register item 534.
    ///
    /// ⚠⚠⚠ NOT [`Blocked`](Self::Blocked), which is the verdict a reader would otherwise be handed
    /// for *a person is what this run needs*. `blocked` says a question went unanswered and sends
    /// them hunting for a dialog there is none of. What happened here is that a person came, said
    /// *wait, let me look*, and then stopped looking — and until this word existed the run said
    /// `exhausted — iterations`, which sent its reader to raise a step budget that would have bought
    /// it nothing (register item 9, measured on run 18).
    ///
    /// ⚠⚠ **AND IT CARRIES NOTHING, WHICH IS A DECISION.** The obvious payload is the pane, on
    /// [`PeerGone`](Self::PeerGone)'s precedent — and the [`Driver`](crate::driver::Driver) has
    /// nowhere to put it: this verdict's ending is `exhausted` against
    /// [`Ceiling::Hold`](crate::driver::Ceiling::Hold), whose word travels to the wire through
    /// `Ceiling` itself and survives a restart, where `PeerGone`'s pane admittedly does not. A
    /// `PaneId` nobody reads is how two spellings of one fact come to differ. ⚠ The residue is
    /// stated rather than hidden: nothing published names the pane a loop is driving NOW, which a
    /// run that has replaced its session makes a real gap — and it is a gap for every ending, not
    /// this one, so it is registered rather than half-closed here.
    Abandoned,
    /// **THE STEP COULD NOT BE TAKEN AT ALL** — the pass returned an error instead of a verdict,
    /// so the run ends [`OutcomeState::Failed`](crate::driver::OutcomeState::Failed).
    ///
    /// # ⚠⚠⚠⚠⚠ Why a run needed a word for this, measured — register item 680
    ///
    /// Until it existed the driver's failing arm wrote NOTHING to the journal: it set
    /// `Outcome::failure` and moved on, so a run's own walk ended at the last step that SUCCEEDED.
    /// **2026-08-25: three live runs died `there is no pane N`, each with
    /// `Working --TurnDone--> Judging` as its last entry and nothing after it, each naming a pane
    /// that was alive at the time.** The defect behind them (register item 682) could be narrowed
    /// by elimination and never confirmed, because nothing said which call raised the error.
    ///
    /// # ⚠⚠ Why not one of the nine words already here
    ///
    /// Each of them is a CONCLUSION a pass reached, and this pass reached none.
    /// [`Abandoned`](Self::Abandoned) is the closest shape and means something specific and
    /// different — *a person said wait and then stopped looking* — so a reader meeting it on a pane
    /// that vanished would go looking for the person. [`Exhausted`](Self::Exhausted) names a ceiling
    /// that did not fall. Reusing either is a sentence the product does not mean.
    ///
    /// # ⚠⚠ It carries NOTHING, on [`Abandoned`](Self::Abandoned)'s precedent
    ///
    /// The obvious payload is the [`PaneError`], and it is already in two
    /// places a reader has: `Outcome::failure`, and the journal line's own `note` — which is where
    /// the sentence belongs, because the note is what carries the PLACE beside it
    /// ([`Plugin::at`], register item 543). A third spelling is how two copies of one fact come to
    /// differ.
    Failed,
}

impl Verdict {
    /// This verdict's word in a run's published journal — the ONE place the variant → name mapping
    /// lives, so the host never spells a `Verdict` variant ([`Cost::unit`]'s rule).
    ///
    /// Exhaustive, so a further verdict cannot reach the wire without a word.
    #[must_use]
    pub const fn wire_str(&self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Converged => "converged",
            Self::Blocked(_) => "blocked",
            Self::Answered(_) => "answered",
            Self::TakenOver(_) => "taken_over",
            Self::Exhausted(_) => "exhausted",
            Self::Screened(_) => "screened",
            // ⚠⚠ AN EIGHTH WORD, and it earned a `WIRE_PROTOCOL` bump for version 27's stated
            // reason: a journal reader decodes this closed set WHOLE. It is reachable by a client
            // older than this build, because `orchestrator` — a form every version has been able to
            // select — is the plugin the 43 hours were actually spent inside.
            Self::PeerGone(_) => "peer_gone",
            Self::Abandoned => "abandoned",
            // ⚠⚠⚠ A TENTH, on the eighth's terms exactly — register item 680. A journal reader
            // decodes this set whole, and this word reaches every plugin (any step may fail), so
            // it earns the same bump `peer_gone` did.
            Self::Failed => "failed",
        }
    }

    /// Every word this vocabulary publishes, so a reader of a run's journal can be told the closed
    /// set rather than discovering it.
    ///
    /// ⚠ Hand-ordered because the variants CARRY DATA and so have no `ALL` to walk — the residue
    /// [`OutcomeState`](crate::driver::OutcomeState) states for the same reason, and the gate below
    /// this holds the list to [`wire_str`](Self::wire_str) rather than trusting it.
    pub const WIRE_WORDS: &'static [&'static str] = &[
        "continue",
        "converged",
        "blocked",
        "answered",
        "taken_over",
        "exhausted",
        "screened",
        "peer_gone",
        "abandoned",
        "failed",
    ];
}

/// A typed cost quantity — what a [`Step`] spent, with its UNIT in the type.
///
/// A run drives exactly one plugin, and a plugin reports the SAME unit every
/// step, so a run accumulates spend in one currency and the two variants never
/// mix within a run (the [`Driver`] never sums bytes against tokens). Bytes and
/// tokens have no exchange rate — typing the unit makes that a compile-time fact
/// rather than a convention, so the cost guardrail (the platform's defence
/// against runaway spend) can never silently bind one currency with another's
/// budget.
///
/// A new cost unit (a future tool measured in dollars or API calls) is a new
/// variant here; the `Driver` / `Guardrails` / `Outcome` stay generic over
/// `Cost`.
///
/// [`Driver`]: crate::driver::Driver
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// PTY bytes injected into a pane — the natural unit of the byte-relay
    /// plugins (`Orchestrator`, `Pipe`, `Agent`).
    Bytes(u64),
    /// Real billed LLM tokens (input + output) — a conversation plugin's natural
    /// unit. A turn whose tokens cannot be measured (a print-mode endpoint, or a
    /// degraded / cancelled turn) reports `Tokens(0)`: no measured spend. For
    /// such a turn the iteration budget, not cost, is the liveness guarantee.
    Tokens(u64),
}

impl Cost {
    /// The scalar amount, dropping the unit.
    #[must_use]
    pub const fn amount(self) -> u64 {
        match self {
            Cost::Bytes(n) | Cost::Tokens(n) => n,
        }
    }

    /// The unit label — the self-describing tag the host emits on the wire and
    /// validates a guardrail's unit against. The ONE place the variant→name
    /// mapping lives, so the host never names a `Cost` variant.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Cost::Bytes(_) => "bytes",
            Cost::Tokens(_) => "tokens",
        }
    }

    /// **THE GUARDRAIL KEY THAT SETS A BOUND IN THIS UNIT** — `max_bytes` for a byte-spending run,
    /// `max_tokens` for a token-spending one.
    ///
    /// # ⚠⚠ Why it lives beside [`unit`](Self::unit) rather than in the wire grammar
    ///
    /// [`unit`](Self::unit)'s own reason, at the other name this type has: **the ONE place a
    /// variant → name mapping lives**, so nothing outside spells a `Cost` variant. It earned the
    /// move when a THIRD party started reading this key — register item 738 gave a loop KIND a
    /// guardrail clause of its own, so `max_bytes` is now spelled by a wire grammar, by the
    /// request parser and by a document's reader, and three hand-written copies of one key is how
    /// a published shape and an admitted one come apart.
    ///
    /// ⚠ Exhaustive, so a third cost unit cannot reach the wire without a key: there is no
    /// hand-written list for it to be left out of.
    #[must_use]
    pub const fn bound_key(self) -> &'static str {
        match self {
            Cost::Bytes(_) => "max_bytes",
            Cost::Tokens(_) => "max_tokens",
        }
    }

    /// This cost's UNIT carrying `amount` — the way to build a bound in a run's own currency
    /// without naming the variant.
    ///
    /// ⚠ `none_of`'s shape at a number that is not zero, and for the same reason: a bound built by
    /// naming a variant is a bound that can be built in the wrong one.
    #[must_use]
    pub const fn sized(self, amount: u64) -> Self {
        match self {
            Self::Bytes(_) => Self::Bytes(amount),
            Self::Tokens(_) => Self::Tokens(amount),
        }
    }

    /// THIS COST'S UNIT AT NOTHING SPENT — what a record the Driver writes about ITSELF costs.
    ///
    /// ⚠ Derived from an existing cost rather than defaulted to bytes, because a run's unit is
    /// established by its first step and a zero in the wrong currency is a lie in a journal a
    /// reader sums.
    pub(crate) const fn none_of(self) -> Self {
        match self {
            Self::Bytes(_) => Self::Bytes(0),
            Self::Tokens(_) => Self::Tokens(0),
        }
    }

    /// Sum two costs of the SAME unit (saturating). `None` if the units differ —
    /// which a single run never produces (one plugin reports one unit), so this
    /// is a defensive guard, not an expected path.
    pub(crate) fn try_add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Cost::Bytes(a), Cost::Bytes(b)) => Some(Cost::Bytes(a.saturating_add(b))),
            (Cost::Tokens(a), Cost::Tokens(b)) => Some(Cost::Tokens(a.saturating_add(b))),
            _ => None,
        }
    }

    /// Whether this accumulated cost has reached the `bound` of the same unit. A
    /// bound of a different unit does not bind (defensive; a run's steps and its
    /// bound share a unit by construction).
    pub(crate) fn reaches(self, bound: Self) -> bool {
        match (self, bound) {
            (Cost::Bytes(a), Cost::Bytes(b)) | (Cost::Tokens(a), Cost::Tokens(b)) => a >= b,
            // A bound of a different unit than the accumulator cannot occur (one
            // plugin = one unit; the host sizes max_cost in that unit). If it
            // ever did, the guardrail must NOT silently fail open — scream in
            // debug (like `accumulate`'s `try_add` guard); in release report
            // "not reached" so the iteration budget still bounds the run.
            _ => {
                debug_assert!(
                    false,
                    "cost guardrail unit mismatch: {self:?} reaches {bound:?}"
                );
                false
            }
        }
    }
}

/// **WHAT A RUN HAS PUT INTO ITS PANE, AND HOW MUCH OF IT NOBODY CAN SEE THERE** — register item
/// 591, and the answer to [`Plugin::deliveries`].
///
/// # ⚠⚠⚠⚠⚠ Why the two numbers travel together and neither is useful alone
///
/// `folded` alone is a count with no scale: *three folds* is a run whose every prompt is invisible
/// if it made three deliveries, and a rounding error if it made two hundred. `made` alone says
/// nothing about visibility. **The question a person actually asks is a RATIO** — *can I go and
/// look at that pane for my prompt?* — and it has one honest answer only when both are published.
///
/// ⚠⚠ It is deliberately not a tally over every [`Witnessed`](crate::deliver::Witnessed) road.
/// Six counters would publish five numbers nobody has asked a question about, and the wire rule
/// this repository follows is that a number the product does not read is folklore. The two here
/// are the two register item 591 was filed on; a third road that turns out to matter earns its own
/// field with its own measurement.
///
/// ⚠ `made` counts deliveries that were ACCEPTED — a refused one never reached a pane at all, and
/// counting it would make the ratio read as *this run's prompts are visible* on a run whose
/// prompts never arrived.
/// ⚠⚠ **NO `serde` DERIVES, THOUGH `sprag-host` DOES PERSIST THIS PAIR** — register item 606, and
/// this crate's stated contract (see its `Cargo.toml`): the host owns every mapping to a stored or
/// wire shape. `crate::runs::PersistedDeliveries` one crate over is that mapping, and it keeps the
/// pair as ONE value for the reason this struct exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Deliveries {
    /// How many prompts this run has put into its pane and had accepted.
    pub made: u32,
    /// How many of [`made`](Self::made) were accepted although the peer's composer FOLDED THE PASTE
    /// AWAY — [`Witnessed::Account`](crate::deliver::Witnessed) and
    /// [`Witnessed::LetGo`](crate::deliver::Witnessed).
    ///
    /// ⚠ **THE ROADS WHERE LOOKING AT THE PANE ANSWERS NOTHING.** Every other road leaves the text
    /// somewhere a person can find it; these leave `[Pasted text +N lines]` where they were told to
    /// expect their prompt.
    ///
    /// ⚠⚠ **TWO WITNESSES, ONE NUMBER, AND THAT IS THIS STRUCT'S OWN RULE APPLIED** — register item
    /// 762. `Account` is the agent naming the question; `LetGo` is the composer emptying without
    /// anybody naming it. They carry the SAME remedy — *do not go to that pane* — and
    /// [`unsubmitted`](Self::unsubmitted)'s doc is explicit that a split is earned by remedies
    /// differing, not by evidence differing.
    ///
    /// ⚠ The residue this used to state rather than hide — *a run cannot tell here how many of its
    /// folds the agent actually named* — **is paid by [`released`](Self::released), and the day it
    /// named arrived.**
    pub folded: u32,
    /// ⛔⛔⛔⛔⛔ **HOW MANY OF [`folded`](Self::folded) THE COMPOSER SETTLED RATHER THAN THE
    /// AGENT** — [`Witnessed::LetGo`](crate::deliver::Witnessed), and register item 669.
    ///
    /// # ⛔⛔⛔⛔⛔ A SUB-COUNT, not a fifth sibling, and the difference is this struct's own rule
    ///
    /// [`unsubmitted`](Self::unsubmitted) states the rule in as many words: **a split is earned by
    /// remedies differing, not by evidence differing.** The remedies here do NOT differ — neither
    /// prompt is on that pane, and *do not go and look* is the answer either way — so this is not a
    /// number beside `folded`. It is a number INSIDE it: `released <= folded` always, and a reader
    /// who wants the fold ratio still reads `folded` of `made` and is unaffected.
    ///
    /// # ⛔⛔⛔⛔⛔ What earns it is register item 669's rule, and the argument that withheld it was
    /// circular
    ///
    /// This field was declined on the ground that *a road earns its own number with its own
    /// measurement, and this one has never run in production*. **That measurement was impossible
    /// while the number did not exist**: `Account` and `LetGo` reach the same `folded`, so no run,
    /// row or log could say which witness settled a fold. Measured 2026-09-04 over this
    /// repository's own loop log — **151 live runs, 2,940 delivery attempts, 1,030+ folds, and
    /// nothing anywhere could say whether [`crate::deliver::SubmittedWhen::Released`] had ever
    /// answered once.** That contract was built FOR item 669 and shipped 2026-08-31.
    ///
    /// ⇒ A contract nobody can observe firing is a contract nobody can say is working, which is the
    /// position item 669 exists to end — and it is the same disease one level up: item 669's
    /// finding was *NO has no channel*, and this is *the channel has no counter*.
    ///
    /// # ⚠⚠ What it diagnoses, which is why it is read and not folklore
    ///
    /// A run whose folds are **all** `released` has a peer whose hooks report nothing (register
    /// item 709's territory), and one whose folds are **never** `released` is a run where the
    /// composer reading is absent — no manifest claim, no composer rule, or a daemon too old to
    /// send the key. Those are different faults with different remedies, and until this field
    /// existed they published the same number.
    pub released: u32,
    /// How many prompts this run typed onto a pane, saw painted there, and **never got asked** —
    /// [`crate::deliver::Delivered::Unsubmitted`], the composer holding a question nobody
    /// submitted.
    ///
    /// ⚠⚠⚠⚠⚠ **NOT PART OF [`made`](Self::made), AND THAT IS THE POINT.** It is not a delivery: no
    /// question was asked, so putting it in the denominator would dilute the folded ratio with
    /// prompts nobody was ever asked. But it is not NOTHING either, which is what it was until
    /// register item 617 — `Witnessed::of` maps both refusals to `None`, so a run whose prompt sat
    /// in a composer published the same `0 of 0` as a run that never typed a byte, and the host's
    /// sentence (which returns early on a zero denominator) said nothing at all about the one run
    /// register item 591 built these counters for.
    ///
    /// ⚠⚠ **THE REMEDY IT CARRIES IS THE OPPOSITE OF [`folded`](Self::folded)'s.** A folded prompt
    /// means *do not go and look at that pane*; this one means **go and look — your prompt is
    /// sitting there**, which is a different instruction to a different person. Counting them as
    /// one number would be counting two remedies as one.
    pub unsubmitted: u32,
    /// How many prompts this run typed into a composer that **swallowed them**, and which the peer
    /// then never named — [`crate::deliver::Delivered::Unreported`]. Register item 762.
    ///
    /// ⚠⚠⚠⚠⚠ **THE FOURTH NUMBER, AND [`unsubmitted`](Self::unsubmitted)'S OWN DOC IS WHY.** That
    /// field argues, in as many words, that a folded prompt and a visible one *"would be counting
    /// two remedies as one"* — and then counted this road as its own, because the fold road reaches
    /// the same `Delivered` answer. So the number that said *go and look at the pane* was being
    /// raised for prompts that are not on any pane.
    ///
    /// ⚠⚠ **IT IS NOT [`folded`](Self::folded) EITHER, and the two are easy to mistake.** `folded`
    /// counts folds that RECOVERED: the paste was swallowed and the agent named the question
    /// anyway, so a delivery happened. This counts the ones that did not — no delivery, no
    /// question, and nothing on the screen. `folded` is a sub-count of [`made`](Self::made) and
    /// this one is outside it, for `unsubmitted`'s reason exactly.
    ///
    /// ⚠ **MEASURED**: `run110` (2026-08-31) ended on this road twice, eighteen hours apart, and
    /// this workspace had no number for it — the register entry that diagnosed the run said so
    /// itself (*"안 쟀다 … 이 항목의 수는 「닿았는데 기록 못 된 것」"*).
    pub unreported: u32,
}

impl Deliveries {
    /// **NOTHING DELIVERED** — the answer for a plugin that puts no composed prompt into a pane.
    ///
    /// ⚠ A named constant rather than `Default::default()` at the call site, for the reason
    /// `crate::runs`-style absences are named everywhere in this workspace: `Deliveries::default()`
    /// reads as *I have not filled this in*, and this is a positive claim — **this plugin has no
    /// prompts for a composer to fold.** `pipe` relays bytes somebody else composed, `orchestrator`
    /// drives a peer it did not write the words for, and neither has a delivery in this sense.
    pub const NONE: Self = Self {
        made: 0,
        folded: 0,
        released: 0,
        unsubmitted: 0,
        unreported: 0,
    };

    /// Whether EVERY prompt this run delivered was folded away — the reading that says *do not go
    /// and look at that pane*, and the one register item 591 was measured on.
    ///
    /// ⚠ False for a run that has delivered nothing, which is the honest answer: a run with no
    /// deliveries has no invisible ones, and a predicate answering `true` there would tell a person
    /// to distrust a pane nothing has been typed into.
    #[must_use]
    pub const fn all_folded(self) -> bool {
        self.made > 0 && self.folded == self.made
    }
}

/// **WHAT BECAME OF THIS RUN'S INDEPENDENT CHECKS** — register item 601, and the answer to
/// [`Plugin::checks`].
///
/// # ⛔⛔⛔ *Checked* and *the checker died* were the same `converged`
///
/// Register item 428 built the independent check because **a milestone certified by the agent that
/// worked on it is not certified** — the literature it cites measured 92 of 100 runs recording
/// success where success meant *the branch was pushed*. Register item 593 then made a silent check
/// say WHICH silence. Neither reached the place a person actually looks: the run's own answer says
/// `converged` whether an independent process agreed or whether the checker never started, and
/// those are opposite facts about how much the ending is worth.
///
/// ⚠⚠⚠⚠⚠ **THIS IS THE THIRD TIME THE SAME SHAPE HAS BEEN PAID** — items 591 and 594 are the other
/// two. A fact the driver knows flows into the WALK, which is a stream of changes bounded to the
/// last `JOURNAL_LIMIT` steps and not persisted; the run's ANSWER only carries it if somebody
/// deliberately carries it. **Three instances is a tendency, not a coincidence**, and it is written
/// down here rather than in a round summary because the next person adding a driver-side fact needs
/// to meet it.
///
/// ⚠⚠ `asked` is the denominator and it separates the two absences a reader must not confuse:
/// `asked: 0` is a run whose document **authored no checker** — a decision its author took, which
/// [`crate::outer::Checked::NotAsked`] names — while `asked: 3, silent: 3` is a checker that was
/// declared and never worked. Item 593 was filed on the second and the first is not a fault at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    /// How many milestone claims this run put to an independent checker.
    pub asked: u32,
    /// How many of [`asked`](Self::asked) answered nothing this run could read.
    pub silent: u32,
    /// **WHY THE LAST SILENT ONE SAID NOTHING** — `crate::judge::Unheard::describe`, or [`None`]
    /// when no check has been silent.
    ///
    /// ⚠⚠ THE LAST rather than a list, and that is a decision with a reason: a run's answer is read
    /// to decide what to do NEXT, and the remedy for the most recent failure is the one still
    /// standing. A list would grow with the run — the property `Step::note` is capped for — and a
    /// FIRST would name a checker that may since have been fixed.
    ///
    /// ⚠ The sentence is `judge`'s, not composed here: one authority on what a silence means.
    pub why_silent: Option<String>,
    /// **HOW MANY MILESTONE CLAIMS COULD NOT BE PUT TO A CHECKER AT ALL** — register item 674, and
    /// the claims that used to leave the tally with no mark whatever.
    ///
    /// # ⛔⛔⛔⛔⛔ It is NOT part of [`asked`](Self::asked), and it is not zero for the same reason
    ///
    /// `asked` counts questions really put to an independent process. A claim counted here never
    /// got that far: the loop could not READ its own `milestone_check` out of the datamodel, so
    /// nothing was spawned and nothing could answer. Putting it in `asked` would claim a question
    /// nobody asked; leaving it out of the record entirely — which is what happened before this
    /// field — makes the run's own numbers **flatter its checker**: every claim it could not put is
    /// a claim absent from the denominator, so *the checker answered all of them* stays true by
    /// losing the ones it never saw.
    ///
    /// ⚠⚠ **THE DISTINCTION `asked`'s DOC ALREADY CLAIMED AND COULD NOT KEEP.** That doc says
    /// `asked: 0` separates *nobody was meant to check this* from *the checker is broken* — true
    /// only once a datamodel that cannot answer is told apart from an author who declared no
    /// checker. Both used to arrive as `asked: 0`, and only one of them is a decision.
    ///
    /// ⚠ Non-zero means the loop's own instrument failed, so the remedy is this run's document and
    /// script session, never the checker or the working agent.
    pub unasked: u32,
    /// How many of [`asked`](Self::asked) the checker TURNED DOWN — register item 499.
    ///
    /// ⚠⚠⚠ THE THIRD VERDICT, AND IT WAS THE ONE WITH NO TALLY. A check answers agreed, refused or
    /// nothing; `asked` counted the question and `silent` counted one of the three answers, so the
    /// arm that publishes [`crate::outer::Checked::Failed`] incremented nothing. **A refusal was
    /// readable in a run's walk and nowhere in its answer**, which is this type's own opening
    /// argument — a fact the driver knows reaching a bounded stream and not the level.
    pub refused: u32,
    /// **THE MOST REFUSALS ONE CLAIM-CHAIN TOOK IN A ROW** — how close this run ever came to
    /// `reflect_after_refusals`, in the units that ceiling is written in.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the total above cannot answer the question this one is for
    ///
    /// Register item 499. `reflect_after_refusals` bounds CONSECUTIVE refusals, and thirty-two
    /// refusals spread one apiece over thirty-two claims is a run that never approached a ceiling
    /// of two, while sixteen pairs is a run that hit it sixteen times. [`refused`](Self::refused)
    /// cannot tell those apart, so a number authored against the ceiling could never be defended
    /// or withdrawn from it.
    ///
    /// ⚠⚠⚠ **THE DOCUMENT IS THE ONE AUTHOR OF *CONSECUTIVE*, AND THIS ONLY MARKS ITS HIGH WATER.**
    /// `ai_loop.scxml` holds `refusals` and decides where it resets — on entry to `reflecting`, by
    /// ANY road, a cadence reflection included. A driver that re-derived the streak would be a
    /// second author of that reset rule and free to disagree with the guard it is measuring. What
    /// is recorded here is the value the document's own ceiling guard compares (`refusals + 1`),
    /// read at the one moment a refusal is known — the watermark idiom that document already uses
    /// for `milestone_at`, `screened_carried` and `service_retried_carried`.
    ///
    /// ⚠ `0` for a run whose checks all agreed, and that is the reading the item was registered
    /// for: *the ceiling was never approached*, which is a measurement and not an absence.
    pub refused_in_a_row: u32,
}

impl Checks {
    /// **NOTHING CHECKED** — the answer for a plugin that puts no claim to an independent checker.
    ///
    /// ⚠ Named for [`Deliveries::NONE`]'s reason: this is a positive claim — *this plugin makes no
    /// milestone claim anybody could check* — and not an unfilled field.
    pub const NONE: Self = Self {
        asked: 0,
        silent: 0,
        why_silent: None,
        refused: 0,
        refused_in_a_row: 0,
        unasked: 0,
    };

    /// Whether EVERY check this run asked said nothing — the reading that says *this run's endings
    /// rest on the working agent's own word*, and the one register item 593 was measured on.
    ///
    /// ⚠ False for a run that asked none, which is the honest answer and the important one: a run
    /// whose author declared no checker has nothing broken, and saying otherwise would send
    /// somebody to fix a checker that was never meant to exist.
    #[must_use]
    pub const fn none_answered(&self) -> bool {
        self.asked > 0 && self.silent == self.asked
    }
}

/// **WHAT A RUN COMPLETED AND KEPT, WHATEVER WORD IT ENDED WITH**, in the plugin's own unit.
///
/// # ⚠⚠⚠⚠⚠ The report this exists to stop being backwards
///
/// `sprag stand-down` promises *"it stops at its next milestone, and its work is kept"*, and the
/// sentence a person reads afterwards asserted, of EVERY ending that was not a convergence, that
/// *the turn it had going was NOT banked*. Register item 604 measured the ordinary case that makes
/// it false: an agent finishes a turn under a standing order and then exits, so the run ends
/// `peer_gone` with its work safely recorded and the person is told they lost it. **The alarming
/// answer and the relieved one were swapped**, which is the one direction a report must never be
/// wrong in.
///
/// ⚠⚠⚠ **THE RENDERER COULD NOT HAVE KNOWN.** It holds a `RunState` and cannot see which plugin
/// produced it — the same reason its own comments refuse the word *milestone*. So the repair is not
/// a softer sentence, it is a FACT: the plugin that counted the work says how much there was.
///
/// ⚠⚠ **THE UNIT TRAVELS WITH THE COUNT**, exactly as [`Plugin::at`] carries a state's name and
/// [`Edge`] carries a document's: a number whose noun lives in the reader is a number the reader
/// has to already know the plugin for. `ai_loop` answers `"turn"`; a plugin with no unit of
/// completed work answers [`None`] and the sentence says nothing about work rather than guessing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banked {
    /// How many complete units of work this run recorded before it ended.
    ///
    /// ⚠ `0` is a real answer and not an absence — *this plugin counts work and there was none* is
    /// what a run that never finished a turn should say, and it is a different sentence from *this
    /// plugin does not count work at all*, which is [`None`] one level up.
    pub completed: u32,
    /// What the plugin calls ONE of them, singular and lower case — `"turn"` for the loop.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is a `Cow` where [`Plugin::at`] is a bare `&'static str`
    ///
    /// A live plugin hands over a literal and pays nothing. A run READ AFTER A RESTART hands over a
    /// word decoded from the daemon's log, and there is no `'static` to borrow it from — so the
    /// type has to admit both, and saying so is better than an intern table or a leak.
    ///
    /// ⚠⚠⚠ **AND RESTORING IT IS SOUND, WHERE RESTORING A POSITION IS NOT.** `sprag-host`'s
    /// `PersistedRun::at` is a STATE NAME: a symbol whose meaning lives in a `.scxml`, so a word
    /// from a dead daemon and this binary's vocabulary are only the same fact when the document
    /// fingerprints agree — which is why that one is deliberately not restored into the live cell.
    /// This is a plain noun for a unit of work. Three completed turns are three completed turns
    /// whatever the document said, so the pair survives a restart with nothing to check it against.
    pub unit: std::borrow::Cow<'static, str>,
}

/// ONE TRANSITION A STEP'S MACHINE ACTUALLY TOOK, in that machine's own words.
///
/// # ⚠⚠⚠⚠⚠ Why the walk stopped being a sentence
///
/// A step used to say where it went in [`Step::note`] and nowhere else — `"judging --judge-->
/// working"` — so every question about a run's path was a substring match on prose. Register item
/// 611 wrote the rule (*a substring of prose is not a declaration; ask the line that decides*) and
/// register item 605 paid for breaking it: a walk line that read `Judging --PeerGone--> PeerGone`
/// was believed for four rounds, and the machine had answered that event from `working`.
///
/// ⚠⚠ **THE WORDS ARE THE DOCUMENT'S, NOT A COPY OF THEM.** All three come from the generated
/// `StatePolicy::get_state_name` / `get_event_name`, exactly as [`crate::driver::Progress::at`]
/// does since item 543 — so a state renamed in the `.scxml` renames itself here, and no
/// hand-written table can drift out of step with the machine it describes.
///
/// ⚠ A plugin with no machine publishes none of these, which is what keeps the substrate
/// content-agnostic: this is a fact ABOUT a step, in the plugin's vocabulary, not a type the
/// driver has to understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The state the machine was in **at the moment this event was raised** — not at the top of
    /// the pass. Item 605's whole cost was the difference between those two.
    pub from: &'static str,
    /// The event that was raised into the machine.
    pub raised: &'static str,
    /// Where the machine was once it had answered.
    pub to: &'static str,
}

/// What a [`Plugin::step`] did and decided.
///
/// ⚠ NOT `Copy`, because of [`note`](Self::note) — and the field is worth that. A run reported its
/// total spend and its terminal state and NOTHING about the steps in between, so a loop that failed
/// to converge could not be diagnosed at all: an agent or a person looking at
/// `exhausted after 100 iterations` had no way to ask what the seventh one did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// What this step spent on the peer, as a typed [`Cost`]: injected/argv bytes
    /// for the byte-relay plugins, real billed tokens for an AI adapter. A run
    /// drives ONE plugin reporting ONE unit, so the Driver accumulates and bounds
    /// without ever summing across units. Cost is non-negative and may be zero (a
    /// `Tokens(0)` print-mode/degraded turn) — the iteration budget, not cost, is
    /// the liveness guarantee, so a cost-free turn cannot loop forever.
    pub cost: Cost,
    pub verdict: Verdict,
    /// ONE LINE ABOUT WHAT THIS STEP DID, in the plugin's own terms, or [`None`] when it has
    /// nothing to add beyond its cost and verdict.
    ///
    /// The [`Driver`] never reads it — it records it into the run's journal and the host publishes
    /// it. That keeps the Driver content-agnostic exactly as [`Plugin::captured`] does, and it is
    /// why this is a plain string rather than a type the substrate would have to understand: what
    /// is worth saying about a step is the plugin's business, and a `pipe` that relayed into a pane
    /// which never showed the text is the case that proves it — nothing in `cost` or `verdict` can
    /// carry that fact.
    ///
    /// ⚠ Keep it SHORT and per-step. It is retained for the last [`JOURNAL_LIMIT`] steps of a live
    /// run, so a note that grew with the transcript would make a long run's memory grow with it.
    ///
    /// [`Driver`]: crate::driver::Driver
    /// [`JOURNAL_LIMIT`]: crate::driver::JOURNAL_LIMIT
    pub note: Option<String>,
}

impl Step {
    /// A step that spent `cost`, decided `verdict`, and has nothing to add.
    #[must_use]
    pub const fn new(cost: Cost, verdict: Verdict) -> Self {
        Self {
            cost,
            verdict,
            note: None,
        }
    }

    /// The same step, with one line for the run's journal.
    #[must_use]
    pub fn noting(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// **WHAT A PLUGIN CAN STILL SAY ONCE THE RUN'S BUDGET IS SPENT** — the answer to
/// [`Plugin::ask_for_an_account`].
///
/// # ⚠⚠⚠ Why the plugin names the window and the Driver does not
///
/// Every other bound in this substrate is the [`Driver`](crate::driver::Driver)'s, and this one
/// looks like it should be too — *"what is uniform lives in the Driver"*. It cannot be, and the
/// reason is that an account is **one turn of the plugin's own peer**, whose length is a fact
/// about that peer and about what its caller declared. The Driver's three ceilings are spent by
/// the time this is asked, so there is nothing left in them to carve a window out of, and a
/// number invented here would end somebody's account on a duration nobody chose — the objection
/// [`OuterLoop::attend`](crate::outer::OuterLoop) records against inventing a patience.
///
/// So the plugin answers with a bound derived from what its CALLER already gave it (`ai_loop`
/// counts TWO of its `turn_within_ms`, or of the substrate's published
/// [`DEFAULT_REPLY_TIMEOUT`](crate::run::DEFAULT_REPLY_TIMEOUT) for a caller who declared none —
/// see its own `ask_for_an_account`, and the live run that priced the second turn), and the Driver
/// grants exactly that and no more. What stops this being a hole in the guardrails is the shape:
/// it is granted ONCE, at the end of a run whose ending is already decided and cannot be changed by
/// anything that happens inside it, and a plugin with no bound to name answers
/// [`Cannot`](Self::Cannot) rather than an open window.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Accounting {
    /// **THIS PLUGIN HAS NO ACCOUNT TO GIVE** — the answer of every plugin that relays bytes.
    ///
    /// A run of `orchestrator`, `pipe` or `agent` has already published everything it knows: what
    /// it typed, what came back, and which ceiling stopped it. There is nobody to ask *where did
    /// you get to* and no second party who would know the answer.
    Nothing,
    /// **THE PLUGIN WILL ACCOUNT FOR THE RUN, AND NEEDS THIS LONG.** The Driver keeps stepping it
    /// until it reaches a terminal verdict or the window passes.
    Within(Duration),
    /// **IT WOULD, AND HERE IS WHAT STOPS IT.** The run ends at once, and the sentence goes into
    /// the run's journal — see [`Driver::run`](crate::driver::Driver::run).
    ///
    /// ⚠ Prose rather than a typed cause, on [`Step::note`]'s exact terms: what makes an account
    /// unaskable is the plugin's own business (a pane somebody else is typing in, a session that
    /// has been closed, a machine that never started), and a vocabulary out here would be the
    /// substrate guessing at the shapes of plugins nobody has written yet.
    Cannot(String),
}

/// **WHAT BECAME OF AN ATTEMPT TO PUT A PLUGIN BACK WHERE IT WAS** — [`Plugin::resume_at`]'s
/// answer, register item 543.
///
/// # ⚠⚠⚠⚠⚠ Four answers, because three of them are different problems for a different person
///
/// The register's rule (item 641) is that an absence written as ONE word is a trap the next round
/// walks into, and *"the resume did not happen"* is exactly such a word. Collapsed, it hides the
/// only three questions a daemon reading a run log actually has:
///
/// | answer | whose problem it is | what a boot should do |
/// | --- | --- | --- |
/// | [`NoMachine`](Self::NoMachine) | nobody's — a `pipe` has no place | carry on; the run was never resumable |
/// | [`NotThisDocument`](Self::NotThisDocument) | the DOCUMENT changed under the log | leave the run interrupted, and say so |
/// | [`Refused`](Self::Refused) | the RECORD is malformed for a document it does name | leave it interrupted, and log the sentence |
/// | [`Placed`](Self::Placed) | — | drive on from here, entering nothing |
///
/// ⚠⚠ The middle two are both *"no"* and must not be one word: the first is the ordinary cost of a
/// promotion (item 544's *a changed document is a NEW run*) and is not a defect, while the second
/// means this build wrote a place its own engine will not accept — a bug in the writer, and the
/// only thing that could ever report it is the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Resumption {
    /// **THIS PLUGIN WALKS NO STATECHART**, so there is nothing to put back — the default, and the
    /// answer of every plugin but the loop. ⚠ NOT a failure: a run of `pipe` was never resumable,
    /// and a caller that read this as one would report a defect on every restart.
    NoMachine,
    /// **THESE WORDS ARE NOT THIS DOCUMENT'S** — refused rather than defaulted, on
    /// `sprag_plugin::LoopPlace::from_words`'s rule: a run placed where nobody chose spends a
    /// peer's tokens doing the wrong work, which is worse than the honest `interrupted` a person
    /// is told today.
    NotThisDocument,
    /// **THE WORDS DECODED AND THE MACHINE WOULD NOT TAKE THEM**, with the engine's reason rendered
    /// in the vocabulary the record is written in — see `sprag_plugin::refusal_in_words` for why it
    /// is a sentence and not the rejection's `Debug`.
    Refused(String),
    /// **PLACED, AND NOTHING WAS ENTERED.** The next [`Plugin::step`] continues from here; no
    /// `<onentry>` re-fired, which is the whole difference between a resume and a second run
    /// wearing the first one's id.
    ///
    /// ⚠ It carries no words. Where the plugin now IS is [`Plugin::place`]'s answer and stays that
    /// one function's — a copy here would be a second authority on one fact (item 445), agreeing
    /// with the first until the day something placed a machine and reported a different place.
    Placed,
}

/// ⛔⛔⛔⛔⛔ **WHAT WAS REPLACED UNDER A RUN THAT IS BEING PUT BACK** — [`Plugin::resume_at`]'s
/// second argument, register item 774.
///
/// # ⚠⚠⚠⚠⚠ Why a resume cannot be one word
///
/// A run is put back for two different reasons and they say opposite things about the PEER:
///
/// | who resumed it | what happened to the pane | what the peer is doing |
/// | --- | --- | --- |
/// | [`Driver`](Self::Driver) | nothing — it was never touched | still mid-turn, and the answer is coming |
/// | [`Boot`](Self::Boot) | the same boot re-spawned its program | nothing; no turn was ever started |
///
/// Told apart nowhere, a machine put back at *the inner agent is running* waits for a turn that
/// cannot end. **Measured 2026-08-30**: three loops came back from one promotion and made zero
/// deliveries between them in two hours while four runs started in the same window made one each,
/// and every one of the seven rows read `running`. Re-measured in a fixture on 2026-09-01 with the
/// competing explanation eliminated — the rescued run's pane came back RUNNING ITS PROGRAM and
/// showing the marker the run waits for, and the driver still typed nothing.
///
/// ⚠⚠ **IT IS A PARAMETER AND NOT A METHOD SOMEBODY MAY FORGET TO CALL.** Both facts are known for
/// certain by the caller — the two call sites are different functions in the daemon — and putting
/// the answer in the resume's own signature is what makes *nobody said* impossible rather than
/// merely unlikely. A plugin with no machine ignores it, as it ignores the place.
///
/// ⚠ NO `_` ARM anywhere that reads this: a third way a run comes back must fail to compile rather
/// than inherit whichever of these two happens to be written first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resumed {
    /// **A DRIVER PROCESS WAS REPLACED UNDER A LIVE PANE** — register item 671's door. The pane and
    /// whatever is running in it are the ones this run left, so a peer that was mid-turn still is.
    Driver,
    /// **A DAEMON BOOT PUT THIS RUN BACK, AND THE SAME BOOT RE-SPAWNED ITS PANE'S PROGRAM** —
    /// register item 543's door. Whatever the peer was doing is gone with the process that was
    /// doing it, however faithfully the pane itself came back.
    Boot,
}

/// A control plugin driven over the [`PaneAccess`] extension API.
pub trait Plugin {
    /// Perceive the panes, act on them, and judge — one step.
    ///
    /// The Driver calls this each microstep, enforces the guardrails around it,
    /// and maps the result onto the statechart. An error aborts the run
    /// (mapped to the `failed` terminal state). `run` carries the run-scoped
    /// signals (cancellation): a plugin's bounded waits should consult it so a
    /// long in-flight step aborts promptly.
    ///
    /// # Errors
    ///
    /// [`PaneError`] when a pane operation fails — unknown pane, unencodable
    /// key, a write failure, or a pane spawn failure (an AI dialogue).
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError>;

    /// Content the plugin captured during its run — e.g. an AI adapter's
    /// response text — read by the host after the run completes and surfaced as
    /// scene-as-data. The [`Driver`] never touches it (it stays content-
    /// agnostic); control plugins that produce no content keep the default
    /// `None`.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn captured(&self) -> Option<String> {
        None
    }

    /// ⚠⚠⚠⚠⚠ **WHERE THIS PLUGIN'S OWN MACHINE IS RIGHT NOW** — a state name from the document it
    /// was built with, or [`None`] for a plugin that walks no statechart.
    ///
    /// # Why the Driver ASKS instead of each step CARRYING it — register item 543
    ///
    /// Where a run has got to exists today only as PROSE: `ai_loop` writes `working --judged-->
    /// judging` into a step's [`note`](Step::note), which is a human sentence in a journal that is
    /// **bounded to the last [`JOURNAL_LIMIT`] steps and is not persisted at all**. So the one fact
    /// a person needs after a restart — *where was it?* — is unreadable by anything, and gone.
    ///
    /// ⚠⚠⚠⚠ **AND A FIELD ON [`Step`] WOULD HAVE BEEN THE WRONG SHAPE**, measured rather than
    /// preferred: this crate builds a `Step` at more than twenty sites, so a per-step field is a
    /// fact that can be FORGOTTEN at any one of them — and a forgotten one does not read as absent,
    /// it reads as *still where it last said*, which is the worst answer available. Asked here, it
    /// has ONE call site in the Driver and cannot be missed.
    ///
    /// ⚠⚠ `'static` because these names come from a document COMPILED INTO THIS BINARY. That is
    /// what makes them safe to persist: the fingerprint recorded beside them
    /// ([`STATECHARTS_FINGERPRINT`](crate::STATECHARTS_FINGERPRINT)) says which documents they are
    /// names from, so a successor daemon can never read a state word against a document that no
    /// longer has it.
    ///
    /// ⚠ The [`Driver`] records it and never reads it, exactly as it treats
    /// [`captured`](Self::captured) and [`Step::note`] — where a plugin is, is the plugin's
    /// business.
    ///
    /// [`Driver`]: crate::driver::Driver
    /// [`JOURNAL_LIMIT`]: crate::driver::JOURNAL_LIMIT
    fn at(&self) -> Option<&'static str> {
        None
    }

    /// **WHETHER THIS RUN IS WAITING FOR A PERSON RIGHT NOW, AND WHY** — register item 755.
    /// [`None`] for a plugin nobody can be waiting on, which is the default and every plugin but
    /// the loop.
    ///
    /// # ⛔⛔⛔⛔⛔ The sixty-two minutes this exists to end
    ///
    /// 2026-08-29, read off a live daemon after the owner asked why nobody was watching a run that
    /// had stopped an hour earlier: `sprag runs` said `running — 46 iterations`, the driver was
    /// alive so no kernel wait fired, and the fact — the agent was `blocked` on a dialog the loop
    /// had correctly put to a person — was in a third place nothing was reading. **The loop's
    /// judgement was right the whole hour and there was no channel.**
    ///
    /// # ⚠⚠⚠⚠ Why an EVENT could not have carried it, which is the whole shape
    ///
    /// Waiting is a state a machine STAYS IN, and every channel a run has reports TRANSITIONS: a
    /// stream has no line to write while a state stands, and a kernel wait fires on an ending that
    /// is not happening. Re-measured the same day — after the wait expired, `AwaitingHuman` appears
    /// in the WALK while the row still reads `running`, so a watcher on the row learns it **never**
    /// rather than late. A fact that persists has to be readable where a reader looks, which is the
    /// run's own status line.
    ///
    /// # ⚠⚠⚠ It STATES, and nothing acts on it
    ///
    /// Register item 715 refused the neighbouring repair — shortening the wait — because that hides
    /// the defect rather than reporting it. This changes no timing, ends no run and answers no
    /// dialog; it is a mouth, in the shape register item 706 gave `done_reason`: the fact was
    /// already inside the daemon and had nowhere to be said.
    ///
    /// ⚠⚠ A SENTENCE and not a word, `Outcome`'s `stop_said` rule: what a
    /// reader does about *the agent is asking you something* and *the machine handed this to you*
    /// is the same walk to the same pane, but only a sentence can say which one it is when they
    /// disagree.
    ///
    /// ⚠ Asked on every step beside [`at`](Self::at), so a plugin that starts waiting between two
    /// steps says so at the next one and never has to be polled.
    fn waiting_for_a_person(&self) -> Option<String> {
        None
    }

    /// **THE WHOLE PLACE THIS PLUGIN'S MACHINE IS IN**, in the document's own words — register item
    /// 543. [`None`] for a plugin that walks no statechart, which is every one but the loop.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is not [`at`](Self::at) with more words in it
    ///
    /// `at` answers a PERSON: *was my run mid-turn, or waiting on me?* — one name, and the whole of
    /// what a row renders. This answers an ENGINE: `enter_at` takes the active set **and** the
    /// current state, and refuses a current that is not a member of that set. A record holding only
    /// `at` is therefore a run that can be reported and never resumed, which is exactly item 543.
    ///
    /// ⚠⚠ **OWNED `String`s, where `at` is `&'static str`.** Both come from documents compiled into
    /// this binary, so both COULD be static — but a configuration is assembled per call from a live
    /// machine, and handing back borrowed names would tie this answer's lifetime to the plugin at
    /// the one moment a caller wants to put it somewhere and let the plugin go. `Cow` would carry
    /// the distinction and nothing here reads it; the allocation happens once per persisted run.
    ///
    /// ⚠ The [`Driver`](crate::driver::Driver) records it and never reads it — `at`'s own rule.
    fn place(&self) -> Option<Vec<String>> {
        None
    }

    /// **PUT THIS PLUGIN'S MACHINE BACK AT `place`, ENTERING NOTHING** — [`place`](Self::place)'s
    /// inverse, and the door a daemon reads a run log through. Register item 543.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the trait carries it, when only one plugin can answer
    ///
    /// The words come out of a run log as `Vec<String>` and the machine that takes them is
    /// `sprag_plugin::OuterLoop`'s. Without this door the daemon that holds the log would have to
    /// know WHICH plugin it is holding, decode the words into a type from this crate's insides, and
    /// call a method the plugin vocabulary does not publish — three couplings for one call, at the
    /// one layer whose whole design (`plugin_from_request`, `PluginKind`) is *the host names a
    /// plugin and never holds one*. Asked here, a place is a thing you may offer to any plugin, and
    /// the ones with no machine say so.
    ///
    /// ⚠⚠ **`&mut self`, which is the honest shape rather than a convenience: this MOVES the
    /// plugin**, as [`step`](Self::step) and [`ask_for_an_account`](Self::ask_for_an_account) do.
    /// It is called before the first step and never during a run — putting a machine somewhere
    /// while a driver is stepping it would be two writers on one configuration.
    ///
    /// ⚠⚠ **A REFUSAL IS AN ANSWER, NOT AN ERROR**, so this returns [`Resumption`] rather than
    /// `Result`: the common refusal (*the document changed*) is what item 544 says a promotion
    /// SHOULD do, and a caller that met it as `Err` would be tempted to fail a boot over the most
    /// ordinary thing that can happen to a saved place.
    ///
    /// ⚠ The default is [`NoMachine`](Resumption::NoMachine) — the answer of every plugin that
    /// relays bytes, and a `place` handed to one is dropped rather than half-honoured.
    ///
    /// ⛔⛔⛔ **`by` SAYS WHAT WAS REPLACED UNDERNEATH** — [`Resumed`], register item 774. A place
    /// alone cannot say whether the peer this run was watching is still there, and a machine put
    /// back at *the inner agent is running* after a BOOT waits for a turn nobody started.
    fn resume_at(&mut self, place: &[String], by: Resumed) -> Resumption {
        let _ = (place, by);
        Resumption::NoMachine
    }

    /// **EVERY TRANSITION THE LAST [`step`](Self::step) TOOK**, in order — see [`Edge`].
    ///
    /// # ⚠⚠⚠⚠⚠ Why a step's path stopped being a sentence
    ///
    /// [`at`](Self::at) says where a plugin IS and [`Step::note`] said, in prose, how it got there.
    /// Register item 611 wrote the rule that a substring of prose is not a declaration, and item
    /// 605 paid for breaking it: a journal line reading `Judging --PeerGone--> PeerGone` was
    /// believed for four rounds while the machine had answered that event from `working`.
    ///
    /// ⚠⚠⚠ **A LIST, BECAUSE A PASS CAN RAISE MORE THAN ONE EVENT.** Measured 2026-08-23: a pass
    /// in `judging` raised `judge`, landed in `working`, then could not deliver that turn's prompt
    /// and raised `peer.gone`. Two transitions, and the journal carried one — so a reader could see
    /// where the run ended and had no way to learn when it left `judging`, which is the question
    /// item 605 could not answer. Register item 614.
    ///
    /// ⚠⚠ Empty by default, and that is the honest answer for a plugin with no machine rather than
    /// a gap: a `pipe` relaying bytes takes no transitions. It stays plugin-agnostic for
    /// [`at`](Self::at)'s reason — the words are the plugin's own, and the [`Driver`] records them
    /// without understanding them.
    ///
    /// ⚠ Read by the driver immediately after `step` returns, so a plugin may clear it at the top
    /// of the next pass and must not clear it on the way out.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn walked(&self) -> Vec<Edge> {
        Vec::new()
    }

    /// **HOW MUCH OF THIS RUN'S WORK IS COMPLETE AND KEPT** — see [`Banked`], or [`None`] for a
    /// plugin with no unit of completed work.
    ///
    /// ⚠⚠⚠ Asked in the same breath as [`at`](Self::at), [`deliveries`](Self::deliveries) and
    /// [`checks`](Self::checks), at the one place a step completes, and for their reason: totals
    /// read at different moments are facts about different moments, and a person weighing *was my
    /// work kept* needs one that describes the run they are reading.
    ///
    /// ⚠⚠ [`None`] is not *nothing was banked* — that is `Some(Banked { completed: 0, .. })`. The
    /// difference is what lets the sentence a person reads say nothing about work when nothing
    /// counted it, rather than reporting a zero the plugin never claimed (register item 539's rule:
    /// ask the plugin, and let a plugin that cannot answer say so).
    fn banked(&self) -> Option<Banked> {
        None
    }

    /// ⛔⛔⛔⛔⛔ **WHY THIS RUN ENDED ON ITS OWN TERMS, AS ONE WORD** — register item 706's third
    /// requirement, and [`None`] for every run that has not ended on its own terms yet.
    ///
    /// # ⚠⚠⚠⚠⚠ The word existed and only a SENTENCE carried it
    ///
    /// A loop that converges knows which of its endings it took, and said so — inside the walk's
    /// note, as prose composed for a person. So a consumer asking *did my stand-down land, or did
    /// this run simply run out of things to propose?* had a string to parse and a vocabulary to
    /// re-spell, which is register item 594's cost arriving one field over. **The repair is not to
    /// build a fact that was missing — it is to lift the word out of the sentence and give it a
    /// key.**
    ///
    /// ⚠⚠⚠ **A WORD AND NOT A TYPE, BECAUSE THE VOCABULARY IS THE PLUGIN'S.** A driver that owned
    /// a closed set here would be a second authority on a distinction it cannot check: the endings
    /// a loop closes under are facts about that loop's own document, and the next plugin's are its
    /// own. This is [`at`](Self::at)'s arrangement exactly — the state name is the plugin's word,
    /// carried and never interpreted — and it is why the gate that keeps the vocabulary honest
    /// belongs beside the plugin that spells it, not here.
    ///
    /// ⚠⚠ **IT IS NOT [`OutcomeState`](crate::driver::OutcomeState) AND MUST NOT BE READ AS ONE.**
    /// That word says what BECAME of the run — `converged`, `cancelled` — and all three endings a
    /// loop can close under converge. Item 594 measured what collapsing the two costs: a
    /// stood-down run printed `converged`, byte-identical to one that finished on its own.
    ///
    /// ⚠ `'static`, so it cannot be a value assembled per call: an ending is chosen from a closed
    /// set the plugin already holds, and anything a `format!` could produce here would be prose
    /// again.
    fn ended_because(&self) -> Option<&'static str> {
        None
    }

    /// ⚠⚠⚠⚠⚠ **HOW BIG THE BRIEF THIS RUN WAS STARTED WITH IS** — register item 719's second
    /// direction, and [`None`] for a plugin nobody briefs.
    ///
    /// # The door took any size and said nothing, and the caller could not find out
    ///
    /// A loop's brief is composed into the prompt every session it opens is greeted with, and
    /// **re-typed in full into every replacement**. Item 719 measured one at 9,025 bytes being
    /// refused by a peer's composer and retyped every turn — and the person who wrote it had no
    /// channel that would have told them it was large. `orchestrate` answers a run id; nothing on
    /// the row said what it had accepted.
    ///
    /// ⚠⚠⚠ **IT REPORTS AND DOES NOT REFUSE**, and [`Briefing`](crate::Briefing) holds the
    /// measurement that settles why: briefs at 9,025, 8,271, 4,532 **and 2,816** bytes were all
    /// folded away, so there is no threshold to reject on and any constant would be invented.
    ///
    /// ⚠⚠ **A LEVEL THAT DOES NOT MOVE, asked every step anyway.** It is fixed at the door, unlike
    /// [`deliveries`](Self::deliveries) and [`checks`](Self::checks) beside it — but asking it the
    /// same way they are asked is what keeps *one place a step reports what it is* true, and a
    /// second road for the one constant would be the shape [`at`](Self::at)'s doc argues against.
    fn briefed(&self) -> Option<crate::Briefing> {
        None
    }

    /// ⚠⚠⚠⚠⚠ **HOW MANY PROMPTS THIS PLUGIN HAS PUT INTO ITS PANE, AND HOW MANY OF THEM THE PEER'S
    /// COMPOSER FOLDED AWAY** — register item 591.
    ///
    /// # The fact that existed only as a CHANGE — and a supervisor arrives mid-run
    ///
    /// `ai_loop` already says which road a delivery took, but it says it as a DIFF:
    /// `crate::outer::Told` publishes the evidence once and then only when it CHANGES, and that
    /// type's own doc states the trade — *"a reader who joins a walk part-way sees no evidence line
    /// until the road changes"*. So *are my run's prompts visible on that pane?* was answerable only
    /// by having watched from the start, and a person who came back to a running loop could not ask
    /// it at all.
    ///
    /// ⚠⚠⚠⚠ **A FOLDED PROMPT IS INVISIBLE EXACTLY WHERE PEOPLE ARE SENT TO LOOK.** Measured
    /// 2026-08-22: a live run carried *"the prompt is NOWHERE ON THAT SCREEN — its composer folded
    /// the paste away"* on every one of its reflections, and delivery confirmation is the axis this
    /// project has spent the most rounds on. A count is what turns that from an anecdote into a
    /// number somebody can act on — and the DENOMINATOR is half of it: *3 of 3* and *3 of 200* are
    /// different runs, and only the first says every prompt is a fold.
    ///
    /// # ⚠⚠ Why the Driver ASKS, rather than [`Step`] carrying it
    ///
    /// [`at`](Self::at)'s argument verbatim, and it applies harder here because these are TOTALS: a
    /// per-step field forgotten at one of the twenty-odd sites that build a `Step` does not read as
    /// absent, it reads as *nothing was delivered on that step* — a fold silently uncounted, which
    /// is the exact failure this exists to end. Asked here, it has one call site.
    ///
    /// ⚠ A LEVEL and never a delta: answer the run's totals so far. The Driver records what it is
    /// told and never adds to it, which is what keeps one authority on the count.
    ///
    /// ⚠ The default is [`Deliveries::NONE`], which is the honest answer for the three bundled
    /// plugins that put no composed prompt into a pane at all — see that constant.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn deliveries(&self) -> Deliveries {
        Deliveries::NONE
    }

    /// ⛔⛔⛔⛔⛔ **THOSE SAME FOLDS, SPLIT BY WHY THE LOOP WAS REFLECTING** — register item 856(1).
    ///
    /// # ⛔⛔⛔⛔ A total can confirm an axis and can never refute one
    ///
    /// [`deliveries`](Self::deliveries) above answers *how much of this run is invisible*, which is
    /// the question register item 591 asked. Item 856 asks a different one — **what does folding
    /// depend on** — and its discriminator is the `capacity` reflection, the one moment the loop
    /// knows the receiving session is full because that is what put it there. The register's own
    /// refutation is *one capacity reflection whose prompt lands*, and a single total has no shape
    /// in which that fact can appear.
    ///
    /// ⚠ The default is [`crate::outer::FoldsByReason::NONE`], which is the honest answer for every
    /// plugin that does not reflect: no reason ever fired, so no row has a denominator. It is not
    /// *nothing folded* — the rows say `0 of 0`, which is a population and not a clean bill.
    ///
    /// ⚠⚠ Asked here for [`deliveries`](Self::deliveries)' reason exactly: a level with one call
    /// site, never a per-[`Step`] delta that twenty-odd construction sites could each forget.
    fn folds_by_reason(&self) -> crate::outer::FoldsByReason {
        crate::outer::FoldsByReason::NONE
    }

    /// ⛔⛔⛔⛔⛔ **AND EVERY DELIVERY, SPLIT BY WHAT PROVED IT ARRIVED** — register item 856, and
    /// the one place a prompt LANDING is counted exhaustively.
    ///
    /// # ⛔⛔⛔⛔⛔ Both answers above count every fold and only some landings
    ///
    /// [`deliveries`](Self::deliveries) offers `made - folded` as the landing count and it is not
    /// one: two of the five roads inside that subtraction are a run ending mid-delivery and a peer
    /// that paints nothing, neither of which is a question anybody asked.
    /// [`folds_by_reason`](Self::folds_by_reason) counts only what was asked during a reflection.
    /// Measured over runs 194-198 and 201 of this repository, the third instrument — the run log —
    /// agreed with the persisted fold count **12 of 12** and reached **16 of 127** landings.
    ///
    /// ⚠ The default is [`crate::outer::DeliveredByRoad::NONE`]: every road present and zero,
    /// which is the honest answer for a plugin that has typed nothing. It is not *nothing landed*.
    ///
    /// ⚠⚠ Asked here for [`deliveries`](Self::deliveries)' reason exactly: a level with one call
    /// site, never a per-[`Step`] delta that twenty-odd construction sites could each forget.
    fn delivered_by_road(&self) -> crate::outer::DeliveredByRoad {
        crate::outer::DeliveredByRoad::NONE
    }

    /// ⚠⚠⚠⚠⚠ **WHAT BECAME OF THIS PLUGIN'S INDEPENDENT CHECKS** — register item 601, and
    /// [`deliveries`](Self::deliveries)' argument one fact over: asked here, at the one site a step
    /// completes, so it cannot be forgotten at any of the twenty-odd places a [`Step`] is built —
    /// and a forgotten check does not read as absent, it reads as **a run whose milestone was
    /// verified**, which is the reassuring wrong answer.
    ///
    /// ⚠ [`Checks::NONE`] by default: three of the four bundled plugins make no milestone claim,
    /// so there is nothing for an independent process to be shown.
    fn checks(&self) -> Checks {
        Checks::NONE
    }

    /// 🎯🎯🎯🎯🎯 **HOW MANY PROPOSALS THIS RUN SET ASIDE BECAUSE IT HAD SPENT ITS RE-AIMING
    /// BUDGET** — the owner's decision of 2026-09-02, register item 833(2), and the half of that
    /// decision that keeps a cap from becoming a quiet way of losing findings.
    ///
    /// # ⚠⚠⚠⚠⚠ A cap without this number is indistinguishable from a loop that found nothing
    ///
    /// The VALUE of bounding how far a run may re-aim itself is that it goes on toward the thing a
    /// person asked for. The DANGER, in the item's own words, is that what it set aside is counted
    /// nowhere: **a run that deferred eight proposals and a run that never had an idea publish the
    /// same row.** So the count rides on the outcome and on the live progress both, and the mouth a
    /// person reads prints it — because a fact that reaches the wire and dies at the mouth is the
    /// shape this workspace keeps paying for.
    ///
    /// ⚠⚠ [`None`] is **this plugin does not set proposals aside**, which is every bundled plugin
    /// but the loop, and it is also what a loop answers when its datamodel has stopped speaking.
    /// Never `Some(0)` for either — [`banked`](Self::banked)'s distinction exactly, and the wire
    /// omits the key rather than publishing a zero somebody would read as *it deferred nothing*.
    ///
    /// ⚠ Asked by the [`Driver`] each step, on [`deliveries`](Self::deliveries)' argument: a TOTAL
    /// carried on a [`Step`] can be forgotten at one of the twenty-odd sites that build one, and a
    /// forgotten deferral reads as a run that never had to make the choice.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn deferred(&self) -> Option<u32> {
        None
    }

    /// 🎯🎯🎯🎯🎯 **HOW MANY TIMES THIS RUN CHANGED DIRECTION WITH NOBODY CHECKING** — the owner's
    /// decision of 2026-09-03, register item 847, and [`deferred`](Self::deferred)'s twin at the
    /// other end of the same bound.
    ///
    /// # ⛔⛔⛔⛔⛔ A bound that ships switched off, and says nothing about it
    ///
    /// The loop's document names the program that decides whether a proposed next checkpoint may be
    /// taken, and the TEMPLATE ships that slot empty — deliberately, so a repository copying the
    /// file gets the machinery before it has a judgement to put in it. What was wrong is that it was
    /// inert **quietly**: *a run with the bound switched off and a run with it satisfied published
    /// the same row.* Measured 2026-09-03: no gate in this workspace named that slot, and no
    /// sentence a person reads mentioned it.
    ///
    /// So this is the sound an empty socket makes. `0` is *every direction this run took was
    /// checked*; anything above it is *this run re-aimed itself on its agent's word alone, this
    /// many times*.
    ///
    /// ⚠⚠ [`None`] is **this plugin does not re-aim itself**, which is every bundled plugin but the
    /// loop, and it is also what a loop answers when its datamodel has stopped speaking. Never
    /// `Some(0)` for either — [`deferred`](Self::deferred)'s distinction exactly, and the wire omits
    /// the key rather than publishing a zero somebody would read as *nothing went unchecked*.
    ///
    /// ⚠ It counts ADOPTIONS and not reflections: a reflection that kept the checkpoint it had
    /// owed nobody a verdict.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn unchecked(&self) -> Option<u32> {
        None
    }

    /// 🎯🎯🎯🎯🎯 **HOW MANY OF [`deferred`](Self::deferred) THE CLASSIFIER REFUSED** — a SUBSET of
    /// that count and never a second total. Register item 833.
    ///
    /// # ⛔⛔⛔⛔⛔ The count was right and the sentence beside it was not
    ///
    /// Two different things set a proposal aside: the run spent its own re-aiming budget, and the
    /// kind's `successor_check` refused what its agent named. The document keeps ONE total on
    /// purpose — two would make every reader add them up — and puts the reason in the ending's word
    /// (`capped` against `unadmitted`). **A run that is still going has no ending word.** Measured
    /// 2026-09-03 on this repository's own loop: three proposals set aside, every one of them a
    /// refusal, and the row said *"at its depth cap"*.
    ///
    /// ⚠⚠ The two want opposite things next, which is why the split is worth a number: a proposal
    /// set aside at the cap is registered and the next run may take it; one the classifier refused
    /// will be refused again.
    ///
    /// ⚠ [`None`] is **this plugin sets nothing aside**, on [`deferred`](Self::deferred)'s exact
    /// terms — never `Some(0)` for a plugin with no such choice to make.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn unadmitted(&self) -> Option<u32> {
        None
    }

    /// ⚠⚠⚠ **THE RUN'S BUDGET IS SPENT — CAN YOU SAY WHERE IT GOT TO, AND HOW LONG DO YOU NEED?**
    ///
    /// Called by the [`Driver`] the moment one of ITS ceilings binds, before the run is ended, and
    /// exactly once. A plugin that answers [`Accounting::Within`] is stepped on until it reaches a
    /// terminal verdict or that window passes; whatever it publishes through
    /// [`captured`](Self::captured) by then is the run's own account of itself.
    ///
    /// # ⚠⚠⚠ Why this method has to exist at all, and what its absence cost
    ///
    /// A plugin's own budget reaches the Driver as [`Verdict::Exhausted`], so the plugin sees it
    /// coming and can spend a last turn on it — which is how `ai_loop.scxml`'s `stopping` came to
    /// ask a run that had spent its `max_turns` where it got to. **The Driver's own three ceilings
    /// have no such door.** They are decided out here, between steps, and the run is over before
    /// the plugin is asked anything: measured, a loop stopped by `max_iterations` was left standing
    /// in `working` or `judging` with its agent at rest and its account never requested, and a run
    /// stopped by `max_duration` the same. **Three ways to run out, and only one of them explained
    /// itself** — with a wall clock expiring at least as common as a turn budget.
    ///
    /// # ⚠⚠ Why it has a DEFAULT, where [`driving`](Self::driving) deliberately has none
    ///
    /// `driving`'s wrong answer is harmful: a plugin that inherits `None` leaves a peer running and
    /// says it stopped one. This one's is not. A plugin that inherits [`Accounting::Nothing`] ends
    /// exactly as every run ended before this existed — the account is a courtesy a plugin either
    /// has something to put in or has not, and three of the four bundled plugins genuinely have
    /// not. That is [`captured`](Self::captured)'s reasoning, and this is its other half.
    ///
    /// ⚠ `&mut self`, because answering is a DECISION the plugin then has to remember: `ai_loop`
    /// records that its next judgement must route to `stopping` rather than back to work.
    ///
    /// ⚠⚠ AND `ceiling` IS PART OF WHAT IT REMEMBERS, not merely of how it decides. A plugin that
    /// kept only *some ceiling fell* can route correctly and still say the wrong thing: `ai_loop`
    /// asks its agent where the run got to, and with a flag alone the document's question told a
    /// run stopped by a wall clock that it had spent its turn budget — false, and typed into that
    /// agent's pane (register item 264). Whoever answers this owns the only copy of the fact.
    ///
    /// [`Driver`]: crate::driver::Driver
    fn ask_for_an_account(&mut self, _ceiling: crate::driver::Ceiling) -> Accounting {
        Accounting::Nothing
    }

    /// THE PANE WHOSE JOB THIS PLUGIN IS CAUSING TO WORK — what a run that is cut short must stop.
    /// `None`, the default, for a plugin that sets nothing running of its own.
    ///
    /// # ⚠⚠⚠ Why the Driver cannot answer this itself
    ///
    /// A run has two ways to end from OUTSIDE its own logic — somebody cancels it, or its
    /// [`max_duration`](crate::driver::Guardrails::max_duration) passes — and both can land while a
    /// step is blocked waiting for a peer that this run set going. Before this existed the Driver
    /// stopped STEPPING and returned, and the peer kept working: **the loop's door closed on a room
    /// that was still occupied.** A cancelled run reported `cancelled` while the model it had
    /// prompted went on spending somebody's tokens.
    ///
    /// The Driver holds the lifecycle and cannot fix that alone, because *which* pane is running
    /// *this run's* work is the plugin's own knowledge and nothing else's. The
    /// [`PaneAccess`] it is handed lists every pane in the workspace, and
    /// guessing among them is exactly the wrong answer: a relay reads a pane a PERSON is working
    /// in, and stopping that would be sprag interrupting somebody's editor because an unrelated run
    /// timed out.
    ///
    /// So the plugin names it and the Driver acts on it — the same seam as everywhere else here:
    /// what is plugin-specific is the plugin's, what is uniform is the substrate's.
    ///
    /// # ⚠ It is a question about NOW, not a configuration
    ///
    /// Asked when the run ends, so a plugin whose work is finished may answer `None` and one that
    /// is mid-turn answers its pane. A plugin that OWNS its pane outright and closes it on every
    /// exit path (as [`Dialogue`](crate::dialogue::Dialogue) does) has nothing to add here: closing
    /// a pane already ends everything in it.
    ///
    /// # ⚠⚠⚠ Why this has NO DEFAULT, alone among the methods here
    ///
    /// [`captured`](Self::captured) defaults to `None` because that default is HARMLESS — a plugin
    /// with nothing to publish publishes nothing. This one's would not be. A plugin that drives a
    /// pane and inherits `None` makes a cut-short run report *the run had no job of its own
    /// running* — a true-sounding sentence about a peer that is still working and still spending,
    /// which is exactly the defect this method exists to close, reintroduced silently by an author
    /// who never saw the question.
    ///
    /// So the compiler asks instead. All four bundled plugins answer it explicitly, two of them
    /// with `None` and their reasons written out; a fifth cannot be written without deciding. It is
    /// the reasoning that renamed `Waited::Cancelled` — **make the mistake fail to compile rather
    /// than fail quietly.**
    fn driving(&self) -> Option<PaneId>;

    /// ⚠⚠⚠⚠⚠ **DO YOU READ THIS STANDING ORDER?** — register items 539 and 597, asked of the
    /// plugin so that nothing anywhere else has to keep a list of which plugins do.
    ///
    /// # What this closes
    ///
    /// [`RunContext::held`] and [`RunContext::stood_down`] have exactly ONE reader each in this
    /// workspace, and two standing ratchets count them. Every other plugin is handed the same
    /// order and drives straight on — so `sprag hold-run` and `sprag stand-down` against an
    /// orchestrator, a pipe, a dialogue or an agent run **answered as though they worked and
    /// changed nothing**, while the CLI printed *"it parks at its next pass"* and *"its work is
    /// kept"*. A person who held a run to read its pane was told the pane was now still.
    ///
    /// ⚠⚠⚠ **ASKED, NOT LOOKED UP.** The host refuses the order when this answers `false`, and the
    /// day a second plugin grows a reader it says so HERE and its own refusal lifts — there is no
    /// table of plugin names to remember to update, which is the shape a list would have taken and
    /// the shape that rots. It is [`driving`](Self::driving)'s argument for the same reason.
    ///
    /// ⚠⚠ **THE DEFAULT IS `false`, AND THAT IS THE HONEST ONE**, unlike `driving`'s. A plugin that
    /// has not been written to read an order does not read it, so inheriting *no* describes the
    /// author's silence exactly. The failure mode `driving` guards against — a default that makes a
    /// FALSE claim — is reversed here: this default under-claims, and an under-claim costs a
    /// refusal a person can see and act on rather than a promise they cannot.
    ///
    /// ⚠ [`StandingOrder`] has no `_` arm at any reader, so an order ADDED to it makes every plugin
    /// that answers this fail to compile until its author decides. `Cancel` is deliberately absent:
    /// the DRIVER acts on a cancel, not the plugin, so it is honoured by every run alike.
    ///
    /// [`RunContext::held`]: crate::run::RunContext::held
    /// [`RunContext::stood_down`]: crate::run::RunContext::stood_down
    fn honours(&self, order: StandingOrder) -> bool {
        let _ = order;
        false
    }
}

/// **AN ORDER A PERSON RAISES OVER A RUNNING RUN THAT ITS PLUGIN HAS TO READ** — register items
/// 539 and 597, and the question [`Plugin::honours`] is asked.
///
/// # ⚠⚠⚠ Why a cancel is not one of these
///
/// A cancel is acted on by the DRIVER: it aborts the turn in flight whatever the plugin is, so
/// every run honours one and there is nothing to ask. These two are different in kind — they are
/// carried into the plugin's own document and take effect at a moment only that document can
/// name (*its next pass*, *its next milestone*), so a plugin that has no such moment cannot obey
/// them at all.
///
/// ⚠ A closed set with no `_` arm at its readers, so adding a third order is a compile error at
/// every plugin rather than a silent `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandingOrder {
    /// **PARK BETWEEN TURNS UNTIL A PERSON SAYS GO** — `sprag hold-run`, read via
    /// [`RunContext::held`](crate::run::RunContext::held).
    Hold,
    /// **FINISH WHAT YOU ARE DOING AND THEN STOP** — `sprag stand-down`, read via
    /// [`RunContext::stood_down`](crate::run::RunContext::stood_down).
    StandDown,
}

impl StandingOrder {
    /// Every order there is, so a gate can walk them rather than name the ones its author
    /// remembered — the rule this repository spells *a list with no glob decides alone*.
    pub const ALL: [Self; 2] = [Self::Hold, Self::StandDown];

    /// **WHAT A PERSON ASKED FOR**, in the words the refusal prints — never the variant's name.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Hold => "hold it between turns",
            Self::StandDown => "stop at its next milestone",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **THE PUBLISHED VERDICT LIST IS HELD TO THE ONE THAT IS SERVED.**
    ///
    /// [`Verdict::wire_str`] is what a run's journal is rendered through and
    /// [`Verdict::WIRE_WORDS`] is what the wire's value-space pin walks. They are two spellings of
    /// one vocabulary, and this type CARRIES DATA so there is no `ALL` to derive the second from —
    /// which is exactly the weakness `OutcomeState` states one level up, and the reason it is
    /// checked rather than trusted.
    ///
    /// Built by constructing every variant and asking it for its word, so a further verdict added
    /// to the type fails here until it is published — the failure mode being that a journal reaches
    /// a peer carrying a word no pin has ever seen.
    ///
    /// ⚠⚠ IT HAS NOW CAUGHT ONE, which is worth recording because a gate that has never bitten is
    /// a gate nobody knows the strength of: [`Verdict::PeerGone`] was added to the type and this
    /// went red naming the missing word before the wire's own pin was touched.
    #[test]
    fn every_verdict_the_type_can_spell_is_a_word_the_wire_publishes() {
        let question = sprag_detect::Question {
            asked: vec!["Do you want to proceed?".to_owned()],
            choices: vec![sprag_detect::Choice {
                number: 1,
                label: "Yes".to_owned(),
                selected: true,
            }],
        };
        let served: Vec<&'static str> = [
            Verdict::Continue,
            Verdict::Converged,
            Verdict::Blocked(crate::consent::Unanswered::unreadable()),
            Verdict::Answered(crate::consent::Answered {
                question: question.clone(),
                chose: question.choices[0].clone(),
                how: crate::consent::Taken::Selected,
                bytes: 1,
            }),
            Verdict::TakenOver(crate::readiness::Interruption::of(1)),
            Verdict::Exhausted(crate::driver::Ceiling::Turns),
            Verdict::Screened(crate::screen::Screened {
                question: question.clone(),
                when: "Do you want to".to_owned(),
                said: "think again".to_owned(),
                bytes: 1,
            }),
            Verdict::PeerGone(PaneId(3)),
            // ⚠⚠ THE NINTH, and it is the only one on this list a reader can construct without
            // reaching for anything: it carries nothing (register item 534). The `Driver` has one
            // typed slot for the ending and this verdict's is `Ceiling::Hold`, which travels
            // through the closed set and survives a restart — where a `PaneId` here would have
            // been a value nobody reads.
            Verdict::Abandoned,
            // ⚠⚠⚠ THE TENTH, and the only one on this list NO PLUGIN can produce — register item
            // 680. The `Driver` composes it for the journal line a run leaves when a pass returned
            // an ERROR instead of a verdict, so it reaches a reader through exactly the same closed
            // set every other word does and has to be declared here like every other word.
            Verdict::Failed,
        ]
        .iter()
        .map(Verdict::wire_str)
        .collect();
        assert_eq!(
            served,
            Verdict::WIRE_WORDS,
            "⚠⚠ a verdict the type can spell and the wire does not publish is a word a journal \
             reader meets with no warning — and the pin that would have caught it walks the \
             PUBLISHED list, so it cannot see one that was never added",
        );
        // ⚠ Distinct, or the mapping is not a vocabulary. Two verdicts sharing a word would make
        // `answered` and `continue` indistinguishable in a journal, which is the whole reason the
        // fifth word exists.
        let mut unique = Verdict::WIRE_WORDS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), Verdict::WIRE_WORDS.len());
    }
}
