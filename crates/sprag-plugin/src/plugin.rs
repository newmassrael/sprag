//! `Plugin` — the control-plugin extension point.
//!
//! A plugin owns its perceive + act + judge behaviour; the [`Driver`] owns the
//! statechart lifecycle and the guardrails around each [`step`](Plugin::step).
//! That is the SOLID seam: what is uniform (termination topology, guardrails,
//! outcome mapping) lives in the Driver; what is plugin-specific (when/how to
//! read a pane, what to inject, when to converge) lives here.
//!
//! [`Driver`]: crate::driver::Driver

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
}

impl Verdict {
    /// This verdict's word in a run's published journal — the ONE place the variant → name mapping
    /// lives, so the host never spells a `Verdict` variant ([`Cost::unit`]'s rule).
    ///
    /// Exhaustive, so a fifth verdict cannot reach the wire without a word.
    #[must_use]
    pub const fn wire_str(&self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Converged => "converged",
            Self::Blocked(_) => "blocked",
            Self::Answered(_) => "answered",
            Self::TakenOver(_) => "taken_over",
            Self::Exhausted(_) => "exhausted",
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
    /// Built by constructing every variant and asking it for its word, so a fifth verdict added to
    /// the type fails here until it is published — the failure mode being that a journal reaches a
    /// peer carrying a word no pin has ever seen.
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
