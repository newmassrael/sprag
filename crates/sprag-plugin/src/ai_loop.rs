//! The OUTER loop — the machine that drives an inner agent session, and what
//! measuring it said about the datamodel it was authored with.
//!
//! [`ai_loop.scxml`] is the third control statechart in this crate, after the
//! Driver's `orchestration.scxml` and the endpoint's `session.scxml`. Until this
//! round it was the only one that was **not compiled**: `build.rs`'s `STATECHARTS`
//! listed two of the three, so 312 lines of authored control flow were enforced by
//! nothing and eight Rust doc comments cited the document as an authority no
//! compiler had ever read.
//!
//! Adding it to that list is one word. What the word bought is this module.
//!
//! # ⚠⚠⚠ And what a person can now DO with it: [`AiLoop`], the plugin
//!
//! R376 compiled the document, R377 gave a turn two endings, R378 built the driver and R379 and
//! R380 measured both against a live `claude` — and at the end of all of that **nothing in the
//! daemon constructed one and no surface started one.** The register's own words: *"after R380
//! this is the single biggest thing between this loop and a user"*.
//!
//! The door is this type, and it is a [`Plugin`] rather than a second run mechanism, which is the
//! whole design. [`OuterLoop::pump`] is ONE PASS and the register's next entry says the rest:
//! *"nothing bounds the pump — the CALLER loops, and `Driver`'s `Guardrails` solved exactly this
//! for plugins"*. Every measurement of the outer loop so far supplied its own bound out of a test
//! (`walked.len() < 16`, a five-minute wall clock) because there was nowhere else to get one.
//!
//! Making it a plugin is what makes that bound the substrate's:
//!
//! * the [`Driver`](crate::driver::Driver) loops it, so `max_iterations`, `max_cost` and
//!   `max_duration` bound a loop run exactly as they bound every other — one discipline, not two;
//! * a run gets an id, a cancel flag, a journal, a progress cell and a durable record from the
//!   registry that already holds them, so `runs` and `cancel` work on it the day it exists;
//! * [`driving`](Plugin::driving) means a cancelled or timed-out loop **stops the agent's turn**
//!   rather than closing the door on a room that is still occupied;
//! * and the pane, the guardrails and the brief are all validated at the door, so a caller's
//!   mistake is a synchronous refusal instead of a run that fails a minute later.
//!
//! [`ai_loop.scxml`]: ../../ai_loop.scxml

use std::sync::Arc;

use sce_rust_runtime::IScriptEngine;
use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::consent::Unanswered;
use crate::driver::Ceiling;
use crate::outer::{
    AiLoopEvent, AiLoopSpec, AiLoopState, Brief, Briefed, Noticed, OuterLoop, Pumped,
};
use crate::plugin::{Accounting, Cost, Plugin, Step, Verdict};
use crate::readiness::Reached;
use crate::run::{DEFAULT_REPLY_TIMEOUT, RunContext};

/// **WHY A LOOP DID NOT START** — every refusal is answered at the door, before a byte is typed.
///
/// # ⚠⚠ Why these are refusals and not outcomes
///
/// A run that starts and then fails has already opened a run slot, spawned a thread and possibly
/// typed at somebody's agent. Each of these is knowable from the request alone, and the house rule
/// is that a caller's mistake is a synchronous refusal naming what to change — the same reason the
/// plugin host validates a target pane at submit time rather than reporting `failed` a minute later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotStarted {
    /// The machine's datamodel does not carry the four authored strings, so this driver cannot
    /// drive it at all — see `Authored::read`.
    Undrivable,
    /// The brief did not reach the machine, and what the machine said about it.
    ///
    /// ⚠ [`Briefed::Took`] is not representable here: this arm is only built from the other two.
    Brief(Briefed),
    /// ⚠⚠ **THE BRIEF WOULD TAKE THE RUN THROUGH A STATE THIS BUILD DOES NOT DRIVE**, and which.
    ///
    /// ⚠⚠⚠ ONE ARM REACHES THIS NOW AND IT IS `exhausted`: a brief that allows no turn at all can
    /// only end there, which is a run nobody wanted rather than a state nobody wrote. **The arm this
    /// variant was built for is gone** — `reflecting` was refused because the session-replace
    /// lifecycle behind it did not exist, and it does.
    ///
    /// Refusing here rather than mid-run is the difference between something the caller can act on
    /// before anything happens and a run that prompts a live agent and then stops somewhere with no
    /// answer for it. The variant stays a STATE rather than becoming a sentence about turn budgets
    /// for that reason: the next state this build does not serve gets the same treatment.
    Unbuilt(AiLoopState),
    /// ⚠⚠ **THE LOOP'S STANDING INSTRUCTIONS ARE NOT ONES THIS BUILD CAN CARRY OUT**, and which.
    ///
    /// The rules live in the document's authored half and reach the datamodel either from the file
    /// or through the [`Brief`]. A rule that claims every dialog would refuse every tool call the
    /// agent ever asks about, and one that says nothing leaves it turned down with nothing to do —
    /// so both are answered here, before a byte is typed, exactly as an unreachable state is.
    Screening(crate::outer::NotScreenable),
}

/// A BOUNDED, CANCELLABLE RUN of `ai_loop.scxml`'s machine against one pane — the door onto
/// [`OuterLoop`].
///
/// See the module doc for why this is a [`Plugin`] and not a second run mechanism.
pub struct AiLoop {
    /// The driver, one pass at a time.
    ///
    /// ⚠⚠⚠ IT ALSO HOLDS THE PANE, and this type deliberately keeps no copy. It used to: a `pane`
    /// field set at construction, which was correct for exactly as long as a loop could not replace
    /// its own inner session. `restarting` closes that pane and opens a fresh one, so a copy taken
    /// here names a pane that no longer exists — and [`Plugin::driving`] is what a cancelled run
    /// interrupts, so the model in the replacement pane would go on spending somebody's tokens while
    /// the run reported having stopped it. **The one field this type could hold is the one that made
    /// `driving` lie.**
    inner: OuterLoop,
}

impl std::fmt::Debug for AiLoop {
    /// The pane and where the machine is, and nothing else.
    ///
    /// ⚠ Hand-written because an [`OuterLoop`] owns a compiled statechart engine and a script
    /// interpreter, neither of which is `Debug` and neither of which anybody wants printed. What a
    /// reader meeting this in a failed assertion needs is which pane and which state.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiLoop")
            .field("pane", &self.inner.pane().0)
            .field("state", &self.inner.state())
            .finish()
    }
}

impl AiLoop {
    /// Start a loop over `pane`, evaluated by `script`, for what `brief` says it is for, driven on
    /// the contracts `spec` declares.
    ///
    /// The brief is delivered HERE and not on the first step, so *"the machine did not take what
    /// you briefed it with"* is a refusal the caller reads instead of a run that ends `failed`.
    ///
    /// # Errors
    ///
    /// [`NotStarted`], which names which of the three it was.
    pub fn new(
        script: Arc<dyn IScriptEngine>,
        pane: PaneId,
        brief: &Brief,
        spec: &AiLoopSpec,
    ) -> Result<Self, NotStarted> {
        // ⚠⚠ ASKED BEFORE THE MACHINE IS BUILT, because the answer is arithmetic on the caller's
        // own numbers and needs nothing else. A loop that may take no turn at all can only reach
        // `exhausted`, which is a run nobody wanted rather than one this build cannot drive.
        //
        // ⚠⚠⚠ THE SECOND REFUSAL THAT USED TO BE HERE IS GONE, and its going is the round's headline:
        // `reflect_every < max_turns` was refused because it reaches `reflecting`, and *"the
        // session-replace lifecycle behind it is registered debt"*. It is built. The gate that
        // measured the refusal's premise — that a run really does reach that state — is kept and
        // now measures the walk THROUGH it, which is the standing rule for a gate whose defect has
        // been paid.
        if brief.max_turns < 1 {
            return Err(NotStarted::Unbuilt(AiLoopState::Exhausted));
        }
        let mut inner = OuterLoop::new(script, pane, spec).ok_or(NotStarted::Undrivable)?;
        match inner.brief(brief) {
            Briefed::Took => {}
            refused => return Err(NotStarted::Brief(refused)),
        }
        // ⚠⚠ ASKED AFTER THE BRIEF, because the brief may REPLACE the rules — so validating first
        // would be validating a document the run is not going to use. A caller's own rules are
        // already typed and cannot be malformed; what this reaches is the author's, and the round
        // trip that just carried either of them.
        inner.screening().map_err(NotStarted::Screening)?;
        Ok(Self { inner })
    }

    /// Where the machine is — the loop's own state, for a caller that wants it beside the run's.
    #[must_use]
    pub fn state(&self) -> AiLoopState {
        self.inner.state()
    }

    /// **WHAT THIS LOOP WILL ACTUALLY SAY TO ITS AGENT**, as the document has composed it so far.
    ///
    /// ⚠⚠ A loop that has not been stepped holds three EMPTY prompts and that is correct rather
    /// than broken: the composition happens in `priming`'s `onentry`, on the way to the first
    /// thing that is spoken to. So this answers what the run is going to say only once it has
    /// started saying it — which is the honest shape, and the one a preview has to be built on.
    ///
    /// [`None`] for a machine whose datamodel has stopped answering, which is a run about to fail.
    #[must_use]
    pub fn authored(&self) -> Option<crate::outer::Authored> {
        self.inner.authored()
    }

    /// **WHICH DIALOGS THIS RUN MAY ANSWER**, read live off its datamodel — or [`None`] for a run
    /// that answers none.
    ///
    /// ⚠⚠⚠ It exists so the CARRYING can be gated. The clauses no longer come from this loop's own
    /// document: the template stopped deciding for the repositories that copy it, a KIND document
    /// holds them, and something has to hand them across at `start`. **A carrier nothing can observe
    /// is a carrier that can quietly drop what it carries** — a run would come up looking configured
    /// and stop at the first dialog, which is exactly the failure a live run already paid for once.
    ///
    /// # Errors
    ///
    /// [`crate::outer::NotScreenable`] when the datamodel holds something unreadable as a clause
    /// list — the same refusal the barrier makes, rather than a second reading of it.
    pub fn consenting(
        &self,
    ) -> Result<Option<crate::consent::Consents>, crate::outer::NotScreenable> {
        self.inner.consenting()
    }

    /// **TELL THIS RUN TO STAND DOWN** — finish the milestone it is on, then stop. See
    /// [`OuterLoop::stand_down`] for why this is not `cancel` and why the order is a state.
    pub fn stand_down(&mut self) {
        self.inner.stand_down();
    }

    /// How many turns the AGENT has taken — the document's own counter, which its `max_turns`
    /// guard compares against.
    ///
    /// ⚠ Not a tally kept out here. A caller with its own would be a second authority on the one
    /// number the run's `exhausted` ending is decided by.
    #[must_use]
    pub fn turns(&self) -> Option<i64> {
        self.inner.turns()
    }

    /// **WHAT THE AGENT'S SESSION HAS BEEN CHARGED TO READ** — the document's own `context`,
    /// assigned on every judged turn.
    ///
    /// The quantity `turns` is not: measurement puts one request's addition to context between 861
    /// tokens and 633,749, so a turn count is out by 63% at p90 as a stand-in for it. See
    /// [`OuterLoop::context`](crate::outer::OuterLoop::context) for what a `Some(0)` means, which is
    /// *do not decide on this* rather than *nothing has accumulated*.
    #[must_use]
    pub fn context(&self) -> Option<i64> {
        self.inner.context()
    }

    /// How many of its peer's calls a standing instruction turned down — **the DOCUMENT's own
    /// count**, beside the run's.
    ///
    /// ⚠⚠ There are deliberately two, and the gate that drives them asserts they AGREE — see
    /// [`OuterLoop::screened`].
    #[must_use]
    pub fn screened(&self) -> Option<i64> {
        self.inner.screened()
    }

    /// **WHAT THE MACHINE ACTUALLY DID**, in the journal's own words — and nothing it did not do.
    ///
    /// # ⚠⚠⚠ A LOOK IS NOT A STEP, AND THIS USED TO WRITE IT DOWN AS ONE
    ///
    /// Every pass through the driver produced `{from} --{raised}--> {to}`. That is the right
    /// sentence for a transition and a FALSE one for `Null`, which is the sentinel
    /// [`OuterLoop`]'s `watch` answers when nothing happened — `advance`
    /// returns on it **before touching the machine**, so `from` and `to` are the same state and no
    /// transition exists. The journal printed it in the SHAPE OF A TRANSITION anyway, so a walk of
    /// thirteen entries read as thirteen steps when the machine had moved NINE times and been
    /// LOOKED AT four.
    ///
    /// ⚠⚠⚠ **IT MISLED A READER WHO WAS PAYING ATTENTION.** This round's supervisor read those
    /// lines as progress and reported them to the owner as progress; the owner asked whether that
    /// was a real state or an invention, and the honest answer was that the product had invented
    /// it. **A journal that renders a non-event as an event is not a record — it is a second,
    /// wrong account of what the machine is.**
    ///
    /// ⚠⚠ The DOCUMENT is the single source of truth for which states and transitions exist.
    /// `null` is not one of its events — there is no `<transition event="null">` anywhere in
    /// `ai_loop.scxml` — it is W3C SCXML's eventless sentinel, which the driver uses to say *I
    /// looked and there was nothing to tell you*. So that is what gets written down.
    ///
    /// # ⚠⚠⚠ AND AN EDGE IS NOT ITS OWN REASON — register item 240's journal half
    ///
    /// `screen.none` is ONE edge with several causes behind it, and this used to write the same six
    /// words for every one of them. Three runs whose remedies are three different things — *quote
    /// this dialog in `screen_rules`*, *your agent did not take the key that refuses a call*, *this
    /// run ended holding a key nobody watched land* — left walks that were byte-for-byte identical
    /// on the line that mattered, **and the step's own verdict could not make up for it**: the
    /// machine lands in `awaiting_human`, which is not final, so that step answers `Continue` and
    /// publishes nothing structural at all.
    ///
    /// So a pass that ARRIVED AT a refusal says which — [`Pumped::Moved`]'s `found`, whose doc
    /// holds why *arrived at* rather than *is holding* is the question, and why the pump answers it
    /// rather than a list of states out here.
    ///
    /// ⚠ It is appended to BOTH shapes above rather than to the transition alone. Nothing today
    /// reaches a refusal on a `Null` pass — every producer of one raises `turn.blocked` — and an
    /// arm asserting that would be an arm no production path can reach (R373 turned round, register
    /// item 247's argument). If one ever does, a reader sees a look that says it found something,
    /// which is a visible contradiction rather than a silent loss.
    ///
    /// # ⚠⚠⚠ AND AN EDGE IS NOT ITS OWN CAUSE EITHER — register item 261
    ///
    /// The same finding one state over, and the document says it about itself: `judging` has THREE
    /// transitions into `reflecting` — the milestone was reached, a standing instruction fired, the
    /// budget came round — and every one of them wrote `Judging --Judge--> Reflecting`. Three
    /// causes, three remedies, one arrow; *"which one fired is not published anywhere"* was a
    /// comment in `ai_loop.scxml` for as long as the edges existed, while the fact itself sat in
    /// the datamodel under `reflect_reason` with nothing but a livelock guard reading it.
    ///
    /// So a pass that ENTERED `reflecting` says why — [`Pumped::Moved`]'s `because`, which is a
    /// [`Because`](crate::outer::Because) and not a level, and whose doc holds why an
    /// ENTRY test rather than 240's diff is the right reader for a variable each edge rewrites.
    ///
    /// # ⚠⚠⚠ AND THE SAME AGAIN AT THE OTHER MANY-DOORED STATE — register item 265
    ///
    /// `stopping` is reached by two transitions carrying FOUR ceilings between them — this
    /// document's `max_turns`, and the run's `iterations`, `cost` and `duration` through
    /// [`Plugin::ask_for_an_account`] — and every one of them wrote one arrow. The only thing that
    /// separated them in a walk was whether the Driver's own `note_to_itself` line PRECEDED the
    /// edge, which is telling two facts apart BY THE ABSENCE OF A KEY: the reading this workspace
    /// has burned wire numbers over, arrived at again one state along. It is the same `because`
    /// slot and not a second field, for the reason [`Because`](crate::outer::Because) holds.
    ///
    /// ⚠⚠ **THE TWO FACTS ARE APPENDED SEPARATELY AND IN A FIXED ORDER**, cause before finding: the
    /// first says why the arrow was drawn and the second what the pass ran into on the way. They
    /// are disjoint today — the transition into `reflecting` delivers a prompt, which CLEARS the
    /// notice, so nothing can be arrived at on it — and this composes both anyway rather than
    /// choosing, because a line that silently dropped one of two true things is the failure this
    /// whole function keeps being about.
    fn walked(
        from: AiLoopState,
        raised: AiLoopEvent,
        to: AiLoopState,
        found: Option<&crate::consent::Unanswered>,
        because: Option<crate::outer::Because>,
    ) -> String {
        let mut note = if raised == AiLoopEvent::Null {
            format!("{from:?}: looked, nothing had happened")
        } else {
            format!("{from:?} --{raised:?}--> {to:?}")
        };
        if let Some(reason) = because {
            note = format!("{note} — {}", reason.noted());
        }
        if let Some(unanswered) = found {
            note = format!("{note} — {}", unanswered.noted());
        }
        note
    }

    /// Whether `state` is one of the document's six finals.
    ///
    /// ⚠ EXHAUSTIVE, so a seventh final added to the document lands here as a variant that no
    /// longer compiles rather than as a run that pumps a finished machine forever. ⚠⚠ The sixth
    /// arrived that way: `peer_gone` broke this match on the compile that added it to the file.
    const fn is_final(state: AiLoopState) -> bool {
        match state {
            AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::PeerGone
            | AiLoopState::Blocked => true,
            AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Reviewing
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing
            | AiLoopState::Stopping
            // ⚠⚠⚠ THE FIVE THE REGIONS ADDED, AND NONE OF THEM IS AN ENDING. Four are structural —
            // the parallel root, the two region roots — and one is an ORDER a person gave. They are
            // here because the match is exhaustive on purpose, and the compiler is what made them
            // arrive rather than a reader noticing later.
            //
            // ⚠⚠ A DRIVER SHOULD NEVER SEE ANY OF THEM. `OuterLoop::state` reads the WORK region by
            // name, so what it answers is always one of the thirteen above it. Answering `false`
            // here is the honest reading of the question asked (*is this an ending*) rather than a
            // guess: a run that somehow reported `Standing` has a reader bug, and treating it as
            // finished would end somebody's run over it.
            | AiLoopState::Running
            | AiLoopState::Work
            | AiLoopState::Orders
            | AiLoopState::Standing
            | AiLoopState::StandingDown => false,
        }
    }

    /// What the loop was asking about when it stopped, as a verdict's [`Unanswered`].
    ///
    /// ⚠⚠ READ OFF THE DRIVER'S OWN [`Noticed`] and never off the pane a second time. The screen
    /// has moved since the decision was taken, so a fresh read is a second authority on one fact —
    /// and [`Unanswered::unreadable`] is the honest answer when the driver recorded nothing,
    /// because it says *the remedy is a person* without claiming to know a question.
    fn asking(&self) -> Unanswered {
        match self.inner.noticed() {
            Some(Noticed::Asking(unanswered)) => unanswered.clone(),
            _ => Unanswered::unreadable(),
        }
    }

    /// **WHAT THE RUN IS WALKING AWAY FROM**, where its last turn ended on somebody else — or
    /// [`None`] where it ended cleanly and there is nothing to say.
    ///
    /// ⚠ READ OFF THE DRIVER'S OWN [`Noticed`], which is cleared at every prompt and set only by
    /// the barrier: after a turn that ENDED, it is `None` unless the peer stopped to ask or a person
    /// took the pane. So this is exactly *"the account's turn did not finish, and here is who has
    /// the pane"* — the two facts nothing downstream can recover once the run is over.
    fn left_behind(&self) -> Option<String> {
        match self.inner.noticed() {
            Some(Noticed::Asking(unanswered)) => Some(format!(
                " — no account: the agent stopped to ask ({unanswered:?}) and the question is still \
                 on the pane, unanswered by this run"
            )),
            Some(Noticed::Interrupted(who)) => Some(format!(
                " — no account: somebody took the pane ({who:?}) before the agent answered"
            )),
            _ => None,
        }
    }

    /// The verdict for a machine that has reached one of its five final states.
    ///
    /// # Errors
    ///
    /// [`PaneError::Undrivable`] for the document's `failed`, carrying the clause the driver
    /// recorded when it raised `fail`.
    fn ended(&self, state: AiLoopState, spent: u64, mut note: String) -> Result<Step, PaneError> {
        let verdict = match state {
            // The agent said the word, `closing` got its report, and the report landed.
            AiLoopState::Converged => Verdict::Converged,
            // ⚠⚠ THE DOCUMENT'S OWN BUDGET, which no guardrail can see: `max_turns` counts the
            // inner agent's turns and one of those is many steps of this loop. See
            // [`Ceiling::Turns`].
            // ⚠⚠⚠ AND IT NOW ARRIVES THROUGH `stopping`, which asked the agent where it got to —
            // so an exhausted run's [`Plugin::captured`] can be `Some`. The VERDICT is what tells a
            // caller the two accounts apart: this word and `converged` are the same shape of answer
            // about opposite outcomes, and nothing is written into the agent's own text to say so.
            //
            // ⚠⚠⚠ **AND WHERE THERE IS NO ACCOUNT, THE NOTE SAYS WHY AND WHAT WAS LEFT BEHIND.**
            // The account's turn can end blocked or interrupted, and both still end `exhausted` —
            // the ending is the budget's, and no last question can change it. But the run then hands
            // back `exhausted` and `None`, and a person who asked for a report has no way to tell
            // *the agent wrote nothing* from *this build does not capture it*. Worse, the pane is
            // not left tidy: a dialog raised in this turn is answered by nobody and OUTLIVES THE
            // RUN, on a pane the run has just let go of. So the ONE authority that saw it says so.
            // ⚠ `Verdict::Blocked` is deliberately not used: the run's ending is the budget, and a
            // reader sent to raise `max_turns` is being sent to the right knob.
            //
            // ⚠⚠⚠ **AND WHICH BUDGET IT WAS IS NOT ALWAYS THIS DOCUMENT'S.** Since a ceiling of the
            // RUN's can route the machine here too ([`Plugin::ask_for_an_account`]), naming
            // `turns` unconditionally would tell a caller whose wall clock ran out to raise a
            // number they never came near. The Driver said which when it asked, the loop latched
            // it, and this reports it back — see [`OuterLoop::stopped_short_by`].
            // ⚠ READ RATHER THAN COPIED. This plugin used to keep a `stopped_by` of its own beside
            // the loop's latch: two records of one fact, written in the same breath, and the one
            // that went stale would send a caller to the wrong knob.
            //
            // ⚠⚠ THE VERDICT AND THE WALK'S ARROW ANSWER TWO DIFFERENT QUESTIONS AND MAY DIFFER,
            // stated rather than hidden. The arrow says WHICH CEILING SENT THE MACHINE TO
            // `stopping`; this says WHICH ENDED THE RUN. A loop that spends `max_turns` and then
            // hangs on the account question runs its own clock out during it, so the arrow reads
            // `turns` and this reads `duration` — the document already accepts that trade in
            // `stopping`'s own comment (*"the same ending, reported against a different ceiling"*),
            // and both lines are true. ⚠ A reader is not left to guess between them: the Driver's
            // `note_to_itself` sits between the two, saying which ceiling fell due and when.
            AiLoopState::Exhausted => {
                if let Some(unfinished) = self.left_behind() {
                    note.push_str(&unfinished);
                }
                Verdict::Exhausted(self.inner.stopped_short_by().unwrap_or(Ceiling::Turns))
            }
            // Reached from `awaiting_human --unattended-->`, which nothing produces yet (registered
            // debt), and kept exhaustive rather than folded into the arm below it.
            AiLoopState::Blocked => Verdict::Blocked(self.asking()),
            // ⚠⚠⚠ `cancel` IS RAISED ONLY WHEN THE RUN ITSELF HAS ENDED — `watch` answers it for
            // `Reached::RunEnded` and `Over::RunEnded`, both of which mean this run's context was
            // cancelled or its deadline passed. Both facts are MONOTONE, so the Driver's own
            // `ended_from_outside` is guaranteed to fire at the very next loop top and end the run
            // with the word for whichever it was. Reporting `Continue` here is therefore not a
            // stall: it hands the ending to the one authority that can tell a person's stop from a
            // clock running out, which is a distinction this plugin cannot make and must not guess.
            AiLoopState::Cancelled => Verdict::Continue,
            AiLoopState::Failed => {
                return Err(PaneError::Undrivable(match self.inner.noticed() {
                    Some(Noticed::Undrivable(variable)) => format!(
                        "its datamodel stopped answering for {variable:?}, so the prompt it owed \
                         the pane could not be read and nothing was sent"
                    ),
                    // ⚠⚠⚠ THE REPLACEMENT SESSION DID NOT COME UP, and this round's own sweep is why
                    // these two arms exist. `resuming` raises `fail` when the barrier over a FRESH
                    // pane answers anything but *ready* — a question nobody covered, or somebody
                    // typing in it — and it records the fact in the notice. Without these arms the
                    // run's only sentence said *"recorded no reason"* about a run that had recorded
                    // one, and the question the loop was holding was dropped on the floor.
                    Some(Noticed::Asking(unanswered)) => format!(
                        "the session it opened to replace the old one came up asking something \
                         nothing this run holds could answer ({unanswered:?}), so it was never \
                         prompted — a session that will not come back is a failed run"
                    ),
                    Some(Noticed::Interrupted(who)) => format!(
                        "somebody was already typing in the session it opened to replace the old \
                         one ({who:?}), so it was never prompted"
                    ),
                    // `fail` is raised from three places and the other one is `brief`, which this
                    // plugin answers at the door — so a `failed` with no notice is a path nobody
                    // wrote, and saying so beats inventing a cause.
                    _ => "it reached `failed` and recorded no reason, which is a path this driver \
                          does not write"
                        .to_owned(),
                }));
            }
            // ⚠⚠⚠⚠ THE DOCUMENT'S SIXTH ENDING, AND THE ONE THAT IS NOT AN ERROR. Every other way
            // this loop can stop without finishing reaches its caller either as a word
            // (`exhausted`, `blocked`, `cancelled`) or, for `failed`, as a `PaneError` — and the
            // whole point of the round that built this is that *the peer's program has exited* is
            // NEITHER a fault of the run nor a question anybody asked. It is a fact about the world
            // outside the run, so it is reported as a verdict with the pane in it.
            //
            // ⚠⚠ THE PANE IS READ OFF THE LOOP AND NOT OFF THE NOTICE, even though the typing route
            // records one. A run REPLACES its inner session as it goes, so the pane a reader must
            // go and look at is whichever one this loop is driving NOW — the same reason
            // [`Self::driving`] asks the loop every time instead of holding a copy. The notice is
            // still set on the typing route, because it is the only evidence that the prompt this
            // transition owed was never sent.
            AiLoopState::PeerGone => Verdict::PeerGone(self.inner.pane()),
            AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Reviewing
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing
            | AiLoopState::Stopping
            // ⚠⚠ THE REGIONS' STATES REACH HERE ONLY IF SOMETHING IS WRONG, and `Continue` is the
            // right answer to being wrong: this method is called for a state `is_final` called
            // final, none of these is, and a driver that ended somebody's run on a structural
            // state would be turning a reader bug into a lost run.
            | AiLoopState::Running
            | AiLoopState::Work
            | AiLoopState::Orders
            | AiLoopState::Standing
            | AiLoopState::StandingDown => Verdict::Continue,
        };
        Ok(Step::new(Cost::Bytes(spent), verdict).noting(note))
    }

    /// The verdict for a machine sitting in a state this driver has no effect for.
    ///
    /// # ⚠⚠⚠ Why each of these ENDS the run instead of pumping again
    ///
    /// [`Pumped::Unbuilt`] is advisory — *"a caller that ignores it pumps again"* — and a caller
    /// that does is a loop watching a state nothing will ever move it out of, until a guardrail
    /// bites and reports `exhausted — iterations` about a run that took no turn. That is the
    /// registered cost of an advisory answer, and this is where it is paid: the run stops, and the
    /// word it stops on is the one whose remedy is real.
    ///
    /// ⚠ AND IT IS NOT *"unimplemented"* to whoever reads the run. `awaiting_human` is the only state
    /// left here, and a peer that stopped to ask wants an ANSWER while a person at the pane is
    /// already acting — the same two facts every other plugin reports, given the same two words.
    /// `reflecting` and `restarting` used to be here as this build's own gap; they are built, so the
    /// one thing this function is about is a run waiting for somebody who is not coming.
    ///
    /// ⚠⚠⚠ **AND IT CHARGES WHAT THE REFUSAL SPENT**, which is a hole this round opened and closed
    /// in the same breath. `Orchestrator`, `Agent` and `Answer` all report `Cost::Bytes(asking
    /// .bytes())` on a `Blocked`, and a loop reported zero — true for as long as a loop could not
    /// type at a dialog, and false the moment `screening` could press a key and give up
    /// ([`Refusal::NotDismissed`](crate::consent::Refusal::NotDismissed)). **A cost ceiling that
    /// cannot see what a run typed into somebody's dialog is a ceiling with a hole in it.**
    fn unbuilt(&self, state: AiLoopState) -> Result<Step, PaneError> {
        let spent = match self.inner.noticed() {
            Some(Noticed::Asking(unanswered)) => unanswered.bytes(),
            _ => 0,
        };
        let verdict = match (state, self.inner.noticed()) {
            // A PERSON TOOK THE PANE. `awaiting_human` is where the document waits for them, and
            // `taken_over` is this substrate's word for the same fact.
            (AiLoopState::AwaitingHuman, Some(Noticed::Interrupted(who))) => {
                Verdict::TakenOver(*who)
            }
            // THE PEER STOPPED TO ASK AND NOTHING GOT THE RUN PAST IT. `screening` is built now, so
            // reaching `awaiting_human` means it ran and answered `screen.none` — no rule claimed
            // the dialog, or one did and the refusing key did not take it. Either way the answer is
            // the one every unattended run gives: stop, and publish what is being asked, with the
            // driver's own refusal saying which of the two it was.
            (AiLoopState::AwaitingHuman, _) => Verdict::Blocked(self.asking()),
            (state, _) => {
                return Err(PaneError::Undrivable(format!(
                    "it reached {state:?}, which this build has no effect for — and the brief that \
                     could reach it is refused at the door, so this run took a path nobody wrote"
                )));
            }
        };
        Ok(Step::new(Cost::Bytes(spent), verdict).noting(format!(
            "the loop is in {state:?}, which no driver serves yet"
        )))
    }
}

impl Plugin for AiLoop {
    /// ONE PUMP of the machine, reported in the substrate's own terms.
    ///
    /// ⚠⚠ A MOVE INTO A FINAL STATE IS JUDGED IN THE SAME STEP THAT MADE IT, never on the pump
    /// after. The Driver checks its ceilings after every unconverged step, so a loop that reported
    /// `Continue` on the step that reached `converged` would be told it had run out of iterations
    /// on the very step that finished the work — *"a step that saw the goal SAW IT"*, which is the
    /// Driver's own rule read from this side.
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        match self.inner.pump(panes, run)? {
            Pumped::Moved {
                from,
                raised,
                to,
                spent,
                found,
                because,
            } => {
                // ⚠⚠⚠ AN APPROVAL IS REPORTED BEFORE ANYTHING ELSE THIS STEP DID, and TAKEN so it
                // is reported once. The barrier answered the peer's question inside this pump, on
                // a clause the caller declared — a decision taken on somebody's behalf, which the
                // substrate insists is a WORD and not a note (`Verdict::Answered`), because a run
                // whose journal spells it `continue` is a run in which approvals are indexed by
                // nothing.
                //
                // ⚠⚠ ITS BYTES JOIN THE STEP'S. They were typed by the BARRIER rather than by
                // `say`, so `Pumped::Moved`'s own `spent` cannot see them — and a cost ceiling
                // that could not see what a run typed into somebody's dialog would be a ceiling
                // with a hole in it.
                if let Some(answered) = self.inner.took_answer() {
                    let note = answered.describe();
                    return Ok(Step::new(
                        Cost::Bytes(spent + answered.bytes),
                        Verdict::Answered(answered),
                    )
                    .noting(note));
                }
                // ⚠⚠⚠ AND A REFUSAL GIVEN ON THE AUTHOR'S STANDING INSTRUCTION, reported the same
                // way and for a sharper version of the same reason: this step **stopped the
                // caller's agent doing something it had decided to do** and told it something else
                // instead. An act with that much consequence outside the loop cannot reach a
                // person's report as the word `continue`.
                //
                // ⚠⚠ ITS BYTES JOIN THE STEP'S, exactly as an answer's do — the refusing key and
                // the redirect were both typed by `screening` rather than by the transition's own
                // prompt, so `Pumped::Moved`'s `spent` cannot see either. `screen.matched` owes no
                // prompt (`Owed::on`), so that number is zero and this is the whole cost.
                if let Some(screened) = self.inner.took_screening() {
                    let note = screened.describe();
                    return Ok(Step::new(
                        Cost::Bytes(spent + screened.bytes),
                        Verdict::Screened(screened),
                    )
                    .noting(note));
                }
                let note = Self::walked(from, raised, to, found.as_ref(), because);
                if Self::is_final(to) {
                    self.ended(to, spent, note)
                } else {
                    Ok(Step::new(Cost::Bytes(spent), Verdict::Continue).noting(note))
                }
            }
            Pumped::Ended(state) => {
                self.ended(state, 0, format!("the loop is already in {state:?}"))
            }
            Pumped::Unbuilt(state) => self.unbuilt(state),
            // ⚠⚠⚠ THE PANE IS NOT THIS LOOP'S TO TYPE INTO, before a byte has been sent. Three of
            // the five answers are facts about somebody else and END the run, for
            // [`Self::unbuilt`]'s reason: a pane showing a question the loop did not provoke — a
            // fresh agent's *"do you trust this folder?"* is exactly this, measured — will go on
            // showing it, so pumping until a guardrail bites reports `exhausted` about a run that
            // never started. The other two are the pane mid-transition and the next pump asks
            // again.
            Pumped::NotReady(Reached::Asking(unanswered)) => {
                Ok(Step::new(Cost::Bytes(0), Verdict::Blocked(unanswered))
                    .noting("the pane was already asking something this run had said nothing to"))
            }
            Pumped::NotReady(Reached::Interrupted(who)) => {
                Ok(Step::new(Cost::Bytes(0), Verdict::TakenOver(who))
                    .noting("somebody was typing in the pane before this run started"))
            }
            Pumped::NotReady(seen) => Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting(format!("the pane is not ready to be driven yet: {seen:?}"))),
        }
    }

    /// THE PANE WHOSE AGENT THIS LOOP IS CAUSING TO WORK — until the machine is finished with it.
    ///
    /// ⚠⚠⚠ ANSWERED, and this is the whole reason a cancelled loop is safe. The inner session is a
    /// live agent CLI mid-turn: a run cancelled or timed out while it is thinking would otherwise
    /// stop stepping and leave the model spending somebody's tokens on a question nothing is
    /// waiting for. The Driver sends the pane's job an interrupt on exactly the two endings that
    /// can land mid-step, and it can only do that because this names the pane.
    ///
    /// ⚠⚠⚠ **AND "THE MACHINE IS FINISHED" IS NOT THE SAME QUESTION AS "THE AGENT IS FINISHED",
    /// which is what the first draft of this got wrong and its own gate said so in one line.**
    ///
    /// The obvious rule is *answer `None` once the document is in a final state*, and it makes this
    /// method useless for the one ending it exists to serve. `cancelled` is reached because
    /// `watch` saw the run end **while the agent was mid-turn** — so by the time the Driver asks
    /// what to stop, the document is finished and the agent is not. Measured: the gate below read
    /// back `Stopped::Nothing` on a cancelled run whose peer was still working.
    ///
    /// So the two endings that follow a COMPLETED turn answer `None`, and everything else answers
    /// the pane. That is the direction that fails safe: a needless interrupt costs a peer one
    /// keystroke it was waiting at anyway, and a missed one leaves a model spending somebody's
    /// tokens on a question nothing is waiting for.
    ///
    /// ⚠⚠⚠ **AND WHICH PANE IS A QUESTION WITH A MOVING ANSWER, since `restarting` was built.** It is
    /// asked of the driver on every call and never cached, because a loop that has reflected is
    /// driving the pane that REPLACED the one it was started over — see [`OuterLoop::pane`]. A copy
    /// held here would name a closed pane, and this method's whole reason is what happens to the one
    /// that is still occupied.
    fn driving(&self) -> Option<PaneId> {
        match self.inner.state() {
            // ⚠ NO MODEL IS MID-TURN BY CONSTRUCTION in either, and signalling a pane this run has
            // finished with would interrupt whatever a person started in it next.
            //
            // `converged` is entered when the closing report's turn ended, so the peer is at rest.
            //
            // ⚠⚠⚠ `exhausted` USED TO BE THE SAME ONE-LINE CLAIM — *"entered when a judged turn
            // did"* — AND SINCE `stopping` IT HAS THREE DOORS, so the claim is re-derived rather
            // than carried over. The stopping turn can end three ways and none of them leaves a
            // model spending tokens: `turn.done` is the account written and the peer at rest;
            // `turn.blocked` is the peer PARKED AT A DIALOG, waiting for input rather than
            // producing; `turn.interrupted` is a person who has already taken the pane, and
            // signalling underneath them is the one thing this driver must never do. The other
            // three doors out of `stopping` are `fail`, `cancel` and `peer.gone`; the first two
            // land in states below that DO answer the pane, and the third is beside this arm for a
            // reason of its own.
            //
            // ⚠⚠⚠ `peer_gone` ANSWERS `None` ON EVIDENCE RATHER THAN ON THE FAIL-SAFE. Every other
            // `None` here is an argument that no model can be mid-turn; this one is the one case
            // where the product has LOOKED — the state is only reached because `pane_eof` said the
            // pane's child has exited, which is the same reading the refusal at the door stands on.
            // There is no job left to signal, and `Stopped::Nothing` is the true sentence.
            AiLoopState::Converged | AiLoopState::Exhausted | AiLoopState::PeerGone => None,
            // Everything else, and `cancelled` above all — see the paragraph above. `failed` and
            // `blocked` join it because neither says anything about what the peer is doing, and an
            // unknown answer must fail towards stopping.
            AiLoopState::Cancelled
            | AiLoopState::Failed
            | AiLoopState::Blocked
            | AiLoopState::Idle
            | AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Reviewing
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Closing
            // ⚠ `stopping` IS A TURN LIKE ANY OTHER while it is running: the agent is writing the
            // account, and a run cancelled or timed out underneath it must stop that model exactly
            // as it stops one mid-work. The cost of asking for a last report is a last turn, and
            // this is what bounds it.
            | AiLoopState::Stopping
            // ⚠⚠ A RUN REPORTING ONE OF THESE IS A RUN WHOSE READER IS WRONG, and the safe answer
            // is still the pane: this asks WHICH PANE a stop would have to reach, and a run in
            // flight has one whatever state a misreading named. Answering `None` would leave a live
            // model running after a cancel.
            | AiLoopState::Running
            | AiLoopState::Work
            | AiLoopState::Orders
            | AiLoopState::Standing
            | AiLoopState::StandingDown => Some(self.inner.pane()),
        }
    }

    /// **THE ACCOUNT THE AGENT WROTE WHEN THE RUN ASKED FOR ONE** — `closing`'s turn or
    /// `stopping`'s, read off the pane and handed to whoever started the loop.
    ///
    /// # ⚠⚠⚠ What this answered `None` for, and what that cost
    ///
    /// It read: *"everything this loop learns is already published — the walk is in the journal,
    /// the turn count is the document's counter, the ending is the outcome"*. Every clause is true
    /// and the conclusion was wrong, because none of those says WHAT HAPPENED. **A person who
    /// started a run that edited a dozen files got back the single word `converged`.**
    ///
    /// The content was never hypothetical. R384's live gate ended with its agent auditing the run
    /// in its own words — *"The Write call … was rejected at the tool layer — the rejection came
    /// back as a tool error, not as me declining. That is the north star claim, and it held up"* —
    /// plus two caveats nobody had asked for. That was on the pane, and it reached nobody.
    ///
    /// ⚠⚠⚠ **AND SINCE `restarting`, SCROLLING BACK IS NOT AVAILABLE EITHER.** A run spans several
    /// agent sessions; the pane the report lands on is not the pane the run started with, and the
    /// earlier ones have been closed. This is now the only place a run's own account can survive
    /// its sessions.
    ///
    /// # ⚠⚠⚠ AND THE ENDING THAT MOST NEEDED ONE IS THE ONE THAT HAD NONE
    ///
    /// This used to answer `None` for every unfinished ending, on the argument that publishing the
    /// last WORK turn's output would answer *what did it say last* to a caller who asked *what did
    /// it do*. That argument is still right, and it was an argument for **asking a different
    /// question**, not for staying silent: a run that reached its north star is the one ending a
    /// person can already read from the word alone, and it was the only one being explained.
    ///
    /// So `stopping` asks a run that spent its turn budget where it got to, and this hands that
    /// back too. The other three unfinished endings still answer `None`, and now by MECHANISM
    /// rather than by decision — see the document, which spells out for each of `cancelled`,
    /// `failed` and `blocked` what makes the question unaskable.
    ///
    /// ⚠⚠ THE TWO ACCOUNTS ARE TOLD APART BY THE VERDICT, not by anything written into the text.
    /// A caller reads `converged` beside one and `exhausted` beside the other; the capture puts
    /// words of its own into an agent's account exactly once — to say an opening was evicted — and
    /// inventing a second such line here would be this module editing somebody's report to say what
    /// the run's own outcome already says.
    ///
    /// [`OuterLoop::report`]: crate::outer::OuterLoop::report
    fn captured(&self) -> Option<String> {
        self.inner.report().map(ToOwned::to_owned)
    }

    /// ⚠⚠⚠ **A RUN THE DRIVER'S OWN CEILING STOPPED IS ASKED WHERE IT GOT TO, TOO** — register item
    /// 208, and the half of `stopping` that the document could not reach on its own.
    ///
    /// # ⚠⚠⚠ Why this is not the same as `max_turns`, and why it needed a second door
    ///
    /// `judging`'s `turns >= max_turns` is the DOCUMENT's budget: the loop can see it coming, so it
    /// routes itself into `stopping` and asks. The [`Guardrails`](crate::driver::Guardrails) cannot
    /// be seen from in here at all — they are counted outside, between steps — so a run stopped by
    /// `max_iterations` or `max_duration` was simply not stepped again. **Measured before this
    /// existed: the loop was left standing in `working` or `judging`, its agent at rest, and the
    /// run handed back `exhausted` and nothing** — the same silence `stopping` had just removed
    /// from the ending next door.
    ///
    /// So the answer is the SAME ROUTE, entered by a different door: this records that the run is
    /// stopping short, `judging` reads it off the very next `judge` and goes to `stopping`, and
    /// everything downstream — the question, the echo discount, the capture — is the one that was
    /// already built and gated. **A second way to ask would have been a second account to keep
    /// right.**
    ///
    /// # ⚠⚠⚠ The states that cannot be asked, and the mechanism in each
    ///
    /// Exhaustive, and each group refuses for a fact about the PANE rather than for a policy —
    /// `stopping`'s own table, one layer out:
    ///
    /// * a machine that never started has an agent that was never asked anything, so there is no
    ///   account to give and no turn to give it in;
    /// * `awaiting_human` means somebody else has that pane — a question this run may not answer is
    ///   on it, or a person is typing in it — and typing a question there ANSWERS THE DIALOG or
    ///   types under a hand. That is the refusal `screen.none` and `turn.interrupted` already make;
    /// * a run between sessions has closed the one that did the work, and its replacement has
    ///   nothing to account for: the account would be written by an agent that has done none of it;
    /// * a machine already in a final state has ended, and the run's word for it is published.
    ///
    /// ⚠⚠ AND ONE MORE, which is a bound rather than a pane: a run whose caller put NO limit on a
    /// turn would be asking for an account inside a window with no end. The window is the caller's
    /// own `turn_within_ms` where they gave one, and where they gave none the substrate's published
    /// [`DEFAULT_REPLY_TIMEOUT`] stands in, because *how long one AI reply may take* is exactly the
    /// quantity being bounded and it is the number two other plugins in this crate already run on.
    /// Neither is invented here.
    ///
    /// # ⚠⚠⚠ Why it is TWO of those turns, which a live run priced and no stand-in could
    ///
    /// The account cannot be asked until the turn in flight ENDS. Nothing here may type at a peer
    /// that is mid-reply — that is the whole reason [`OuterLoop::stop_short`] sets a latch instead
    /// of pushing the machine — so a ceiling that falls due while the agent is working buys a wait
    /// before it buys a question. **Measured against a real `claude`, first run: the clock fell due
    /// one step after a turn prompt went in, the window was spent entirely on
    /// `Working --Null--> Working`, and the account was never asked at all.** Every stand-in in this
    /// tree answers in microseconds, so every offline gate passed with a window of one.
    ///
    /// ⚠ So the window covers both turns, and both are the same declared number: one for the peer
    /// to finish what nobody will now read — the run's ending is already decided, and the pane is
    /// simply not this run's until it stops — and one for the answer. A turn already part-spent
    /// leaves the remainder as slack, which is the honest direction to be wrong in.
    ///
    /// [`OuterLoop::stop_short`]: crate::outer::OuterLoop::stop_short
    fn ask_for_an_account(&mut self, ceiling: Ceiling) -> Accounting {
        let state = self.inner.state();
        match state {
            // ⚠⚠⚠ FIRST, BECAUSE IT CUTS ACROSS THE STATES BELOW, and a LIVE run is what found it.
            // A prompt the run's clock cut short between the typing and the Enter is a turn that
            // NEVER STARTED — so it can never end, `judging` is never reached, and the account
            // would be waited for until the window ran out. Worse, the composer still holds that
            // text, so a question typed now would be submitted WITH it (register item 197).
            // Measured twice against a real `claude`; see [`OuterLoop::say`].
            state if self.inner.asked_nothing() => Accounting::Cannot(format!(
                "the run's clock landed between typing its last prompt and submitting it, so in \
                 {state:?} its agent was never asked anything and the composer still holds text \
                 nobody sent"
            )),
            // A turn is in flight or one has just landed: the flag reaches `judging` on the very
            // next judgement, which is the door `stopping` is already entered by.
            //
            // ⚠ `closing` and `stopping` are in this list and are already asking their own
            // question. The flag changes nothing for them, and leaving them out would have been an
            // exception nobody could state a reason for.
            AiLoopState::Priming
            | AiLoopState::Working
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::Redirecting
            | AiLoopState::Closing
            | AiLoopState::Stopping => {
                self.inner.stop_short(ceiling);
                // ⚠⚠⚠ TWO TURNS, AND A LIVE RUN IS WHAT PRICED THE SECOND ONE — see the doc above.
                Accounting::Within(
                    self.inner
                        .turn_within()
                        .unwrap_or(DEFAULT_REPLY_TIMEOUT)
                        .saturating_mul(2),
                )
            }
            AiLoopState::Idle => Accounting::Cannot(
                "the loop never got its pane, so its agent was never asked anything and has \
                 nothing to account for"
                    .to_owned(),
            ),
            AiLoopState::AwaitingHuman => Accounting::Cannot(
                "the pane is not this run's to type in: it is showing a question nothing here \
                 could answer, or somebody is typing in it — asking where the run got to would \
                 answer that dialog or type under their hand"
                    .to_owned(),
            ),
            AiLoopState::Reflecting
            | AiLoopState::Reviewing
            | AiLoopState::Restarting
            | AiLoopState::Resuming => Accounting::Cannot(format!(
                "the run is between sessions ({state:?}): the agent that did the work is being \
                 replaced, and its successor has done none of it"
            )),
            AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::PeerGone
            // ⚠ Structure and orders, which a ceiling can no more account for than an ending can.
            | AiLoopState::Running
            | AiLoopState::Work
            | AiLoopState::Orders
            | AiLoopState::Standing
            | AiLoopState::StandingDown
            | AiLoopState::Blocked => Accounting::Cannot(format!(
                "the loop had already ended in {state:?} when its {} ceiling fell due",
                ceiling.wire_str(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

    use super::{AiLoop, NotStarted};
    use crate::access::PaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState, ProgressCell, Stopped};
    // ⚠ `OuterLoop` and `Pumped` are gone from here, and their going is a fact: the gate that used
    // them drove the layer UNDER the door in order to reach a state the door refused. The door no
    // longer refuses it, so the PLUGIN reaches it, which is the only height a caller has.
    use crate::outer::{AiLoopSpec, Brief, INNER_SESSION_ENDS};
    use crate::plugin::{Accounting, Cost, Plugin, Verdict};
    use crate::readiness::ReadyWhen;
    use crate::run::RunContext;
    use crate::sm::ai_loop::{AiLoopEvent, AiLoopPolicy, AiLoopState};
    use crate::testing::{standin_agent, supervised};

    /// The document's own composed prompt, as a person reading the file expects it.
    const COMPOSED_START_PROMPT: &str = "North star: ";

    /// A peer that answers a working prompt at once — every gate here but the one measuring what
    /// happens when a run's clock expires INSIDE a turn. See `standin_agent_reporting`.
    const NO_THINKING: Duration = Duration::ZERO;

    /// **THE PER-TURN BOUND EVERY GATE HERE DECLARES**, in milliseconds — what `standin_spec`
    /// carried before register item 300 moved the number into the document.
    ///
    /// ⚠⚠⚠ It is DECLARED and not inherited, and the shipped value is why: the document authors
    /// half an hour, for a live `claude` that thinks in minutes. A gate that let it default would
    /// wait that out against a stand-in answering in microseconds — 307's lesson, one field over.
    const GATE_TURN_MS: i64 = 5_000;

    /// **THE BARRIER'S BOUND EVERY GATE HERE DECLARES** — the substrate's own published default,
    /// which is exactly what `AiLoopSpec::ready_within: None` gave these gates before the move.
    ///
    /// ⚠ Not a short number: these gates want the barrier to CLEAR, and `testing::started` is what
    /// clears it. Naming a small bound would be racing the fixture rather than declaring anything.
    /// ⚠⚠ And not ZERO, which here means a bound of zero — one look — rather than *decline*: the
    /// key is a plain duration and a caller has always been able to send it verbatim.
    /// ⚠⚠⚠ ASKED OF THE PRODUCT rather than typed, so a substrate that changes its own default
    /// moves these gates with it instead of leaving a number here that used to be true.
    const GATE_READY_MS: i64 = crate::readiness::DEFAULT_READY_TIMEOUT.as_millis() as i64;

    /// A real script engine, as the daemon's construction site builds one.
    fn engine() -> Arc<dyn IScriptEngine> {
        Arc::new(sce_rust_lua::LuaEngine::new())
    }

    /// The spec these gates drive with — the stand-in's two facts, and nothing else, because
    /// nothing else is a fact about a peer.
    ///
    /// ⚠⚠ THE PER-TURN BOUND IS NOT HERE ANY MORE and cannot be: it is the document's since
    /// register item 300, and each gate declares it on its BRIEF ([`GATE_TURN_MS`]) — small enough
    /// that a stalled gate fails rather than waiting out the shipped half hour.
    ///
    /// ⚠ `shows_the_prompt` is FALSE because a `/bin/sh` peer paints only once it has a whole
    /// LINE, so a delivery cannot be confirmed on screen before the newline that would submit it.
    /// [`AiLoopSpec::driving`] is the real-agent shape and sets it true.
    fn standin_spec() -> AiLoopSpec {
        AiLoopSpec {
            ready_when: Some(ReadyWhen::Settles("claude".to_string())),
            done_when: INNER_SESSION_ENDS,
            shows_the_prompt: false,
            // ⚠ NO JUDGE, so `working`'s `cond="_event.data.judged"` is always false here and
            // every blocked turn takes the `screening` edge. A stand-in gate that acquired one
            // would spawn a real agent per dialog, which is what these gates exist to avoid.
            judge: None,
        }
    }

    /// **A CONSENT ABOUT A QUESTION THE STAND-INS DO NOT ASK** — how a gate says *nobody here
    /// answers a dialog* now that the empty list is unsayable.
    ///
    /// # ⚠⚠⚠ Why a gate cannot simply decline to arm one
    ///
    /// [`Consents::of`](crate::consent::Consents::of) refuses the empty list, and an absent
    /// `may_answer` means **the document decides** — and the shipped document authors the two
    /// dialogs a working loop meets, one of which is the COMMAND question
    /// [`standin_agent_asking`](crate::testing::standin_agent_asking) raises. So a gate that briefs
    /// nothing gets its dialog ANSWERED, and every gate measuring a run that STOPS at one silently
    /// stopped measuring it: three did, in the round that authored the clauses, and the walks said
    /// `answered the peer with 2. "Yes, and do not ask again"`.
    ///
    /// ⚠⚠ **THIS IS `await_person_ms: Some(0)`'s LESSON, ONE FIELD OVER** — a gate that wants a
    /// value SAYS it rather than inheriting one, because the thing it would inherit from is the
    /// shipped document and the shipped document is allowed to change. What this clause proves is
    /// *"none of my consents is about this"* — `other_question` — which is the honest sentence for
    /// a run holding a list that does not reach the dialog on screen.
    /// ⚠⚠ The needle is deliberately a question **no stand-in in this workspace raises**. The four
    /// they do raise are *"Bash command … Do you want to proceed?"*, *"Do you want to make this
    /// edit…"*, *"Do you want to create PROBE.txt?"* and *"Which way should I build this?"* — and
    /// the first draft of this helper picked the fourth, which collided: the screening peer's own
    /// dialog was suddenly claimed, the clause pressed a key at it, and `the_walk_says_which_
    /// refusal_left_the_run_waiting_for_a_person` reported `unwitnessed` instead of the edge it
    /// gates. **A control that matches something is not a control.**
    fn a_consent_about_something_else() -> crate::consent::Consents {
        crate::consent::Consents::of(vec![
            crate::consent::Consent::parse(
                "Shall I publish this release?".to_string(),
                "Publish it".to_string(),
            )
            .expect("both needles are non-empty"),
        ])
        .expect("a non-empty consent list")
    }

    /// A brief that reaches a milestone in `max_turns` and never reflects.
    fn brief_for(max_turns: i64) -> Brief {
        Brief {
            north_star: "the stand-in answers prompts and then says the marker".to_string(),
            milestone: "reach it".to_string(),
            reference: "this gate".to_string(),
            max_turns,
            // ⚠ EQUAL, which is what makes `reflecting` unreachable — `judging` tests the turn
            // budget first. `AiLoop::new` refuses anything smaller, and the gate below drives that.
            reflect_every: max_turns,
            // ⚠ The document's own placeholder rule, which claims nothing — so every gate below
            // that does NOT set this measures a loop with screening available and unarmed, which
            // is the shipped shape.
            screen_rules: None,
            // ⚠⚠⚠ ANSWER NOTHING **THESE STAND-INS ASK**, written rather than inherited — and the
            // comment here used to read *"the shipped document's own value"*, which stopped being
            // true the day that document authored its consents. `None` now means the run may take
            // the command dialog outright; see [`a_consent_about_something_else`]. A gate that arms
            // a consent for real says so in its own brief.
            may_answer: Some(a_consent_about_something_else()),
            // ⚠⚠⚠ NOBODY IS WATCHING, WRITTEN RATHER THAN INHERITED. This was `AiLoopSpec`'s
            // default until the patience moved into the document, and the gates below were written
            // against it: a run that ends at the first dialog it cannot answer. Leaving these
            // `None` would hand every gate the SHIPPED document's hour instead — measured, it hung
            // this suite 59 tests in. A gate that wants a person says so, three lines down.
            await_person_ms: Some(0),
            handback_still_ms: None,
            // ⚠⚠⚠ AND THE TWO DURATIONS, WRITTEN RATHER THAN INHERITED, for the reason directly
            // above and one more: the shipped document authors THREE MINUTES and HALF AN HOUR,
            // which are a person's allowances for a live `claude` and are a hang in a suite whose
            // whole run is 74 seconds. `standin_spec` used to carry the turn bound; it cannot now,
            // so the brief is where a gate says it.
            ready_timeout_ms: Some(GATE_READY_MS),
            turn_within_ms: Some(GATE_TURN_MS),
        }
    }

    /// ⚠⚠⚠ **THE LOOP IS BOUNDED BY THE SUBSTRATE, NOT BY WHOEVER IS PUMPING IT** — register item
    /// 66, and the reason this is a [`Plugin`] at all.
    ///
    /// Every measurement of the outer loop before this round supplied its own ceiling out of a
    /// test — `assert!(walked.len() < 16)` in the driver's gate, a five-minute wall clock in the
    /// live one — because `OuterLoop::pump` is ONE PASS and nothing above it looped. A run that
    /// stalled was bounded by whatever the harness happened to write.
    ///
    /// Here the [`Driver`] does it, so the same three guardrails that bound an `orchestrator` bound
    /// a loop. The gate asserts BOTH halves, because either alone would be worth little:
    ///
    /// * a briefed loop against a real pseudoterminal reaches `converged` **through the Driver**,
    ///   with the machine's own turn count matching what the peer was asked;
    /// * and the run's JOURNAL carries the walk, which is what makes a loop that does not converge
    ///   diagnosable at all — the fact `Pumped::Moved` was carrying and nothing consumed.
    #[test]
    fn a_loop_run_converges_under_the_driver_that_bounds_it() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        // ⚠ THE CONTROL: the pane must be this run's to drive BEFORE it ends, or the `None` the
        // outcome's stop rests on below would be true for the wrong reason.
        assert_eq!(
            loops.driving(),
            Some(pane),
            "⚠⚠ a loop mid-run owns the pane its agent is working in — this is what makes a \
             cancelled run stop the turn rather than abandon it",
        );

        // ⚠ The journal is the PROGRESS cell's, not the outcome's: it is published as a run goes
        // rather than at its end, which is what lets a client tell progress from stuck.
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            // ⚠ WELL ABOVE the five passes the authored happy path takes and well below anything
            // that would take real time, so a stall is caught HERE and a converged run is the
            // machine's own doing rather than a ceiling's.
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠⚠ the whole loop, driven by the substrate: {:?} — walked {walk:?}",
            outcome.state,
        );
        assert_eq!(
            loops.state(),
            AiLoopState::Converged,
            "and the DOCUMENT agrees with the run's word, or the two are counting different things",
        );
        assert!(
            outcome.iterations > 1 && outcome.iterations < 40,
            "⚠⚠ a converged loop takes several pumps and stops well inside its ceiling; {} of 40 \
             would mean the guardrail decided rather than the machine",
            outcome.iterations,
        );
        // ⚠⚠ THE JOURNAL IS THE WALK. `Pumped::Moved` carried `from --raised--> to` and nothing
        // read it; a run that failed to converge could be diagnosed by its total alone, which is
        // what the register calls a loop nobody can debug.
        assert!(
            walk.iter().any(|note| note.contains("Priming"))
                && walk.iter().any(|note| note.contains("Judging"))
                && walk.iter().any(|note| note.contains("Converged")),
            "⚠⚠ the run's journal must carry the machine's own walk, or a loop that stalls is \
             diagnosable only by its totals: {walk:?}",
        );
        assert_eq!(
            loops.driving(),
            None,
            "⚠ and a FINISHED loop names no pane: signalling one a run is done with would \
             interrupt whatever a person started in it next",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE RUN HANDS BACK THE ACCOUNT ITS AGENT WROTE** — register item 121, and the thing a
    /// person who starts a loop actually wants.
    ///
    /// # ⚠⚠⚠ What a caller got before this, in full
    ///
    /// The word `converged`. `AiLoop::captured` answered `None` with a reason that was true clause
    /// by clause — the walk is in the journal, the turns are the document's counter, the ending is
    /// the outcome — and wrong as a conclusion, because not one of those says WHAT HAPPENED. The
    /// agent's own account did, it was on the pane, and nothing read it. R384's live gate priced it
    /// by accident: its agent closed by AUDITING the run in its own words, including two caveats
    /// nobody had asked for, and every one of them was discarded.
    ///
    /// # What this asserts, and why each half is a different way of publishing something false
    ///
    /// * **THE ACCOUNT ARRIVES WHOLE** — its first line as well as its last. The peer's report is
    ///   taller than its pane, so it has scrolled by the time anyone reads: through a [`RowTrail`],
    ///   the reader every other question in this driver is asked through, the opening is **simply
    ///   not there**. Measured against a live `claude` at sixty lines on a forty-row pane, where the
    ///   rendering came back opening at `LINE-29`.
    /// * **AND IT IS THE AGENT'S, NOT THE LOOP'S.** The closing prompt is on that pane too — whole,
    ///   because the peer echoes it, and again as a FRAGMENT, because a real composer re-wraps. A
    ///   capture that returned either would answer *what did the agent report?* with the caller's
    ///   own instruction.
    /// * **AND WHAT IS BETWEEN THE ENDS SURVIVES**, blank line and all: an interior blank is a
    ///   paragraph break in somebody's report, and trimming blanks everywhere would re-flow it.
    ///
    /// ⚠ The scroll is ASSERTED rather than assumed, because a fixture that fits on its pane makes
    /// the two readers agree and the first assertion above would pass through either one.
    #[test]
    fn a_converged_run_hands_back_the_report_its_agent_wrote() {
        let (workspace, pane) = crate::testing::standin_agent_reporting(
            crate::testing::Accounts::ForARunThatGotThere,
            NO_THINKING,
        );
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &RunContext::uncancellable());
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠ the control: an account is only asked for by a run that closed",
        );

        // ⚠⚠ THE PREMISE OF THE FIRST ASSERTION, CHECKED FIRST. A report that still fits on its
        // pane is one both readers can see, so the gate would pass without the address doing any
        // work at all — R385's rule about a fixture that agrees with the default by accident.
        let rendered = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !rendered.contains(crate::testing::REPORT_OPENS),
            "⚠⚠⚠ THE REPORT MUST HAVE SCROLLED OFF THE PANE, or this gate cannot tell the line \
             address from the rendering and its verdict about the reader is worthless. The screen \
             still holds {:?}",
            crate::testing::REPORT_OPENS,
        );

        let report = loops.captured().expect(
            "⚠⚠⚠ A CONVERGED RUN MUST HAND BACK THE ACCOUNT ITS AGENT WROTE. This is register item \
             121: the closing report is real content, it is on the pane, and a caller who started \
             the run gets a word without it",
        );
        assert!(
            report.contains(crate::testing::REPORT_OPENS),
            "⚠⚠⚠ THE ACCOUNT ARRIVED WITHOUT ITS OPENING — which is what reading the RENDERING \
             gives you once a report is taller than its pane, and a truncated account is worse \
             than a missing one because nothing in it says it is truncated. Got: {report:?}",
        );
        assert!(
            report.contains(crate::testing::REPORT_CLOSES),
            "⚠⚠ and without its ending either: {report:?}",
        );
        assert!(
            !report.contains("Summarise what changed"),
            "⚠⚠⚠ THE CALLER'S OWN CLOSING PROMPT CAME BACK AS THE AGENT'S REPORT. The peer echoes \
             what it is asked, exactly as a real agent paints it into its composer: {report:?}",
        );
        assert!(
            !report.contains(crate::testing::REPORT_ECHO_SLICE),
            "⚠⚠⚠ AND THE WRAPPED HALF OF THAT ECHO IS THE ONE AN EXACT MATCH CANNOT SEE. A live \
             composer re-wraps the prompt to the pane's width, so the line store holds a FRAGMENT \
             of it — the discount has to ask `does what I said contain this line?`: {report:?}",
        );
        assert!(
            !report.starts_with(crate::testing::REPORT_RULE)
                && !report.ends_with(crate::testing::REPORT_RULE),
            "⚠⚠ an account must not open or close in the terminal's furniture: {report:?}",
        );
        assert!(
            report.contains("\n\n"),
            "⚠⚠ and the blank line INSIDE it is the agent's paragraph break — trimming blanks \
             everywhere would re-flow somebody's report and nothing would say so: {report:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A RUN THAT RAN OUT OF TURNS IS ASKED WHERE IT GOT TO, AND HANDS THAT BACK** — register
    /// item 201, and the ending a person most wants an account of.
    ///
    /// # ⚠⚠⚠ What this gate used to assert, and why the assertion was right and the design was not
    ///
    /// It was `a_run_that_was_never_asked_for_an_account_publishes_none`, and it MEASURED THE
    /// DEFECT: a peer that never says the marker spends the document's turn budget, its pane ends up
    /// covered in what the agent said, and [`Plugin::captured`] answered `None`. The argument
    /// underneath was sound — *publishing the last WORK turn's output answers "what did it say
    /// last" to a caller who asked "what did it do"* — and it was an argument for **asking a
    /// different question**, which nothing did. So the one ending that could already be read from
    /// its own word (`converged`) was the only one explained, and the one nobody can read
    /// (`exhausted`) was the one that said nothing.
    ///
    /// The old assertion survives INSIDE this one, and it is the third below: the account handed
    /// back must be the STOPPING turn's, not the work turn that came before it. That is what the
    /// `since` mark on every prompt buys, and a reader that took the mark anywhere else would return
    /// `ACK 2` as a run's report.
    ///
    /// # What each assertion is a different way of publishing something false
    ///
    /// * **THE RUN STILL ENDS ON THE DOCUMENT'S BUDGET.** `exhausted — turns` and not
    ///   `exhausted — duration`: the extra turn is a report, not a reprieve, and a ceiling that
    ///   moved would send its reader to raise a guardrail that never bound this run.
    /// * **THE ACCOUNT ARRIVES WHOLE**, opening and ending both — the report is taller than the
    ///   pane, so a rendering reader loses its first lines. Asserted after the scroll is asserted.
    /// * **AND IT IS THE ACCOUNT, NOT THE WORK.** No `ACK` from the turns before it.
    /// * **AND IT IS THE AGENT'S, NOT THE CALLER'S** — neither the stopping question whole nor the
    ///   wrapped fragment of it a real composer would paint.
    #[test]
    fn a_run_that_ran_out_of_turns_hands_back_the_account_it_was_asked_for() {
        let (workspace, pane) = crate::testing::standin_agent_reporting(
            crate::testing::Accounts::ForARunThatRanOutOfTurns,
            NO_THINKING,
        );
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(2), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &RunContext::uncancellable());
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "⚠⚠ THE CONTROL, AND IT IS ALSO A CLAIM. This peer never finishes, so the run must end \
             on the DOCUMENT's turn budget — the account it is asked for on the way out is one more \
             TURN, and a ceiling that moved to `duration` would mean the loop had sat in `stopping` \
             waiting for an answer that never came",
        );

        // ⚠⚠ THE PREMISE OF THE NEXT ASSERTION, CHECKED FIRST — the converged gate's rule, and for
        // its reason: an account that still fits on its pane is one both readers can see, so the
        // verdict about the reader would be worthless.
        let rendered = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !rendered.contains(crate::testing::REPORT_OPENS),
            "⚠⚠⚠ THE ACCOUNT MUST HAVE SCROLLED OFF THE PANE, or this gate cannot tell the line \
             address from the rendering. The screen still holds {:?}",
            crate::testing::REPORT_OPENS,
        );

        let report = loops.captured().expect(
            "⚠⚠⚠ A RUN THAT SPENT ITS TURN BUDGET MUST HAND BACK THE ACCOUNT IT WAS ASKED FOR. \
             This is register item 201: `closing` explained the ending a person can already read \
             from its own word, and the ending nobody can read explained nothing",
        );
        assert!(
            report.contains(crate::testing::REPORT_OPENS)
                && report.contains(crate::testing::REPORT_CLOSES),
            "⚠⚠⚠ THE ACCOUNT ARRIVED WITHOUT ONE OF ITS ENDS — which is what reading the RENDERING \
             gives you once a report is taller than its pane, and a truncated account is worse than \
             a missing one because nothing in it says it is truncated. Got: {report:?}",
        );
        assert!(
            !report.contains("ACK"),
            "⚠⚠⚠ THE LAST WORK TURN'S OUTPUT CAME BACK AS THE RUN'S ACCOUNT. This is the assertion \
             the gate that measured this defect was built on, and it survives the fix: a caller who \
             asked *what did it do* is being answered *what did it say last*. The mark is taken on \
             every prompt for exactly this: {report:?}",
        );
        assert!(
            !report.contains(crate::testing::STOP_QUESTION)
                && !report.contains(crate::testing::STOP_ECHO_SLICE),
            "⚠⚠⚠ THE CALLER'S OWN STOPPING QUESTION CAME BACK AS THE AGENT'S ACCOUNT — whole, or as \
             the wrapped FRAGMENT a real composer paints, which is the half an exact match cannot \
             see. The echo discounted is the question this state ASKED, and a driver holding \
             `end_prompt` as a constant would discount the wrong one: {report:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **AN ACCOUNT NOBODY COULD GIVE STILL ENDS THE RUN — AND THE RUN SAYS WHAT IT LEFT ON THE
    /// PANE** — the driver's half of `stopping`'s shape, and the sweep item this round's own build
    /// produced.
    ///
    /// # ⚠⚠⚠ What the document proves and what only a real pane can
    ///
    /// `an_account_that_cannot_be_had_does_not_change_the_ending` drives the DOCUMENT: every ending
    /// of the stopping turn targets `exhausted`. What it cannot say is what a person is told, and
    /// that is where the loss was: a run out of turns whose last question the agent never answered
    /// hands back `exhausted` and no account, which is indistinguishable from a build that does not
    /// capture one — **and it walks away from a dialog that is still on the pane**.
    ///
    /// ⚠⚠ THE PEER IS THE SAME PROGRAM AS THE ONE IN THE CONSENT GATE, asking at a different
    /// moment ([`Asks`](crate::testing::Asks)). That is the point: a dialog in a WORKING turn is a
    /// run that can still be helped — `screening` looks for a rule, a person is woken — and the
    /// identical dialog in the ACCOUNT's turn can be helped by nobody, because the ending is already
    /// decided. Two situations, one peer, and what separates them is which turn it is.
    ///
    /// ⚠ THE ENDING IS ASSERTED FIRST AND IT IS THE SAFETY PROPERTY: `blocked` here would mean a
    /// reader sent to answer a question instead of to raise `max_turns`, and any route back into the
    /// working cycle would re-take the budget guard and ask for another account for ever.
    #[test]
    fn a_run_whose_account_was_blocked_still_ends_and_says_what_it_left_on_the_pane() {
        let (workspace, pane) =
            crate::testing::standin_agent_asking(crate::testing::Asks::WhenTheRunStopsShort);
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(2), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walked: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|entry| entry.note.clone())
            .collect();

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "⚠⚠⚠ A RUN OUT OF TURNS ENDS ON ITS BUDGET EVEN WHERE THE LAST QUESTION WENT UNANSWERED. \
             `blocked` would send a reader to answer a dialog when the knob they need is \
             `max_turns`; anything that carried on would re-take the budget guard at the next \
             judgement and ask for another account, for ever. Walked: {walked:?}",
        );
        // ⚠ THE CONTROL: the run must actually have got as far as asking. A peer whose dialog never
        // fired would end `exhausted` too, and every assertion below would be about nothing.
        assert!(
            walked.iter().any(|note| note.contains("Stopping")),
            "⚠ the run must have reached `stopping` and been blocked in it: {walked:?}",
        );
        assert_eq!(
            loops.captured(),
            None,
            "⚠⚠ and there is no account, because the agent never gave one — publishing the work \
             turns' text here would answer *what did it say last* to somebody who asked *what did \
             it do*, which is the distinction the whole capture rests on",
        );
        let last = walked.last().expect("a run writes a journal");
        assert!(
            last.contains("no account")
                && last.contains("still on the pane")
                && last.contains("Do you want to proceed?"),
            "⚠⚠⚠ THE RUN MUST SAY WHAT IT WALKED AWAY FROM. `exhausted` with an empty report is \
             indistinguishable from a build that captures nothing — and the dialog this run \
             provoked OUTLIVES it, on a pane nobody is now driving. This driver is the only \
             authority that saw either fact: {last:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A LOOP THAT SPENDS ITS AUTHOR'S TURNS SAYS SO, AND NAMES THE KNOB THAT WOULD BUY IT
    /// MORE** — the whole reason [`Ceiling::Turns`] exists.
    ///
    /// `max_turns` counts the inner AGENT's turns and one of those is many steps of the loop
    /// driving it, so no [`Guardrails`] number can express it. Without a fourth ceiling the two
    /// available endings were both wrong: `converged` claims a goal nobody reached, and handing the
    /// run back to the guardrails reports `exhausted — iterations` about a run whose iteration
    /// ceiling was never met, telling its reader to raise a number that would have bought them
    /// nothing.
    ///
    /// ⚠⚠ **THE ITERATION CEILING IS THE CONTROL AND IT IS DELIBERATELY GENEROUS.** The peer here
    /// never says the marker, so the run must end on the document's budget with iterations to
    /// spare — and the assertion is on WHICH ceiling, not merely that it stopped.
    #[test]
    fn a_loop_that_uses_the_turns_it_was_briefed_with_reports_that_ceiling() {
        // ⚠ A peer that answers every prompt and NEVER says the done marker, so nothing but the
        // document's own budget can end this run.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(2), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "⚠⚠⚠ the run must name the DOCUMENT's budget. `iterations` here would send its reader \
             to raise a guardrail that never bound this run: {:?}, after {} of 40 iterations",
            outcome.state,
            outcome.iterations,
        );
        assert!(
            outcome.iterations < 40,
            "⚠⚠ THE CONTROL: the iteration ceiling must not have bitten, or the assertion above is \
             about a guardrail wearing the document's word. Took {}",
            outcome.iterations,
        );
        assert_eq!(
            loops.state(),
            AiLoopState::Exhausted,
            "and the document agrees it is the one that ended the run",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A RUN THE *DRIVER'S* OWN CEILING STOPPED SAYS WHERE IT GOT TO** — register item 208,
    /// and the two thirds of *"a run that stops short accounts for itself"* that the document could
    /// not reach on its own.
    ///
    /// # ⚠⚠⚠ What was measured here before it was built
    ///
    /// `stopping` is entered from `judging`'s `turns >= max_turns` — this DOCUMENT's budget. A run
    /// meets the [`Guardrails`] instead at least as often, and those are counted OUTSIDE the plugin,
    /// between steps. Measured with today's API at four iteration ceilings and five wall clocks:
    /// **every one ended `exhausted` with the machine standing in `working` or `judging`, its agent
    /// at rest, and [`Plugin::captured`] answering `None`.** Three ways to run out, one of them
    /// explained — and the two that explained nothing are the two a caller sets by hand.
    ///
    /// # ⚠⚠ Why ALL THREE ceilings, in one gate, over one peer
    ///
    /// They are the same claim reached by three arithmetics, and the register names two of them.
    /// Running them as a table rather than as three gates is what keeps them from drifting: the
    /// assertions below are written once, so a fix that satisfies the step count and not the clock
    /// cannot pass.
    ///
    /// ⚠ THE CEILING IS ASSERTED, not merely the ending. `exhausted — turns` here would mean the run
    /// had reported the DOCUMENT's budget for a run that never came near it — telling its reader to
    /// raise `max_turns` when the knob they set is the one that bound. That is the same assertion
    /// `a_loop_that_uses_the_turns_it_was_briefed_with_reports_that_ceiling` makes in the other
    /// direction, and one of them is worth nothing without the other.
    #[test]
    fn a_run_stopped_by_the_runs_own_ceiling_says_where_it_got_to() {
        // ⚠ A ceiling that bites AFTER at least one whole turn and well before the peer could
        // finish: measured, this fixture's turns take three steps each and it never says the
        // marker, so eight steps is two turns of work and no possibility of converging.
        for (guardrails, ceiling) in [
            (
                Guardrails {
                    max_iterations: 8,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
                Ceiling::Iterations,
            ),
            (
                Guardrails {
                    max_iterations: 4_000,
                    max_cost: None,
                    max_duration: Some(Duration::from_millis(120)),
                },
                Ceiling::Duration,
            ),
            // ⚠ THE THIRD GUARDRAIL, and it binds on the very first thing this loop spends: the
            // start prompt. So this row also drives the account being asked for from `priming` —
            // a run stopped before its agent has answered anything once — which the other two do
            // not reach.
            (
                Guardrails {
                    max_iterations: 4_000,
                    max_cost: Some(Cost::Bytes(1)),
                    max_duration: Some(Duration::from_secs(60)),
                },
                Ceiling::Cost,
            ),
        ] {
            let (workspace, pane) = crate::testing::standin_agent_reporting(
                crate::testing::Accounts::ForARunThatRanOutOfTurns,
                NO_THINKING,
            );
            let access = supervised(&workspace);
            // ⚠⚠ THE DOCUMENT'S OWN BUDGET IS PUT OUT OF REACH, which is what makes this gate about
            // the guardrails at all: a run that spent `max_turns` would reach `stopping` by the
            // door that already existed and prove nothing about the new one.
            let mut loops = AiLoop::new(engine(), pane, &brief_for(1_000_000), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let progress = ProgressCell::default();
            let outcome = Driver::new(guardrails)
                .reporting_to(Arc::clone(&progress))
                .run(&mut loops, &access, &RunContext::uncancellable());
            let journal = progress.lock().expect("the progress cell").journal.clone();
            let walked: Vec<String> = journal
                .iter()
                .filter_map(|entry| entry.note.clone())
                .collect();

            assert_eq!(
                outcome.state,
                OutcomeState::Exhausted(ceiling),
                "⚠⚠⚠ THE RUN MUST STILL END ON THE CEILING THAT STOPPED IT. The account is one \
                 more TURN, and the machine reaches its own `exhausted` through `stopping` — so a \
                 ceiling that moved to `turns` would send a caller to raise a budget their run \
                 never came near. Walked: {walked:?}",
            );
            assert!(
                walked.iter().any(|note| note.contains("Stopping")),
                "⚠⚠⚠ THE RUN MUST HAVE BEEN ASKED. Without the outside door into `stopping` the \
                 machine is simply never stepped again — measured, it was left standing in \
                 `working` or `judging` with its agent at rest: {walked:?}",
            );
            assert!(
                walked
                    .iter()
                    .any(|note| note
                        .contains(&format!("own {} ceiling fell due", ceiling.wire_str()))),
                "⚠⚠ AND THE WALK MUST SAY WHICH BUDGET SENT IT THERE. Both budgets reach `stopping` \
                 by the same edge, so `Judging --Judge--> Stopping` reads identically for a run \
                 that spent its `max_turns` and one a wall clock stopped — and only the Driver \
                 knows which: {walked:?}",
            );
            // ⚠⚠ THE PREMISE OF THE ASSERTION BELOW, CHECKED FIRST — the converged gate's rule: an
            // account that still fits on its pane is one both readers can see.
            let rendered = access.pane_collapsed(pane).unwrap_or_default();
            assert!(
                !rendered.contains(crate::testing::REPORT_OPENS),
                "⚠⚠ the account must have scrolled off the pane, or this proves nothing about the \
                 reader; the screen still holds {:?}",
                crate::testing::REPORT_OPENS,
            );
            let report = loops.captured().unwrap_or_default();
            assert!(
                report.contains(crate::testing::REPORT_OPENS)
                    && report.contains(crate::testing::REPORT_CLOSES),
                "⚠⚠⚠ A RUN STOPPED BY ITS OWN {ceiling:?} CEILING HANDED BACK NO ACCOUNT. This is \
                 register item 208: `exhausted` is the ending a person can least read from its own \
                 word, and it was the one that said nothing. Got {report:?}, walked {walked:?}",
            );
            assert!(
                !report.contains("ACK"),
                "⚠⚠⚠ THE LAST WORK TURN'S OUTPUT CAME BACK AS THE RUN'S ACCOUNT — *what did it say \
                 last* answered to somebody who asked *what did it do*: {report:?}",
            );
            // ⚠⚠⚠ AND THE JOURNAL'S OWN LAST WORD, which the outcome cannot stand in for.
            // `Verdict::Exhausted` carries the ceiling INSIDE it and means *the plugin's own
            // declared budget is spent*: a run the wall clock stopped that spelled `turns` there
            // would be false in the one record a reader diagnoses a loop from.
            assert!(
                matches!(
                    journal.last().map(|entry| &entry.verdict),
                    Some(Verdict::Exhausted(named)) if *named == ceiling,
                ),
                "⚠⚠⚠ the terminal step must name the ceiling that stopped the run, not the \
                 document's own: {:?}",
                journal.last(),
            );
            access.lifecycle().expect("lifecycle").close(pane);
        }
    }

    /// ⚠⚠⚠ **THE QUESTION A STOPPED RUN IS ASKED NAMES THE CEILING THAT STOPPED IT** — register
    /// item 264, and its walk half, item 265.
    ///
    /// # ⚠⚠⚠ What was measured here before it was built
    ///
    /// `stop_prompt` was ONE authored sentence — *"This run has spent its whole turn budget and is
    /// ending here, short of what it was asked for"* — and `stopping` is reached by FOUR ceilings:
    /// the document's own `max_turns`, and the run's `iterations`, `cost` and `duration` through
    /// [`Plugin::ask_for_an_account`]. **So for three of the four it was false.** Driven at all four
    /// with today's API before anything was changed, the identical sentence came back every time.
    ///
    /// ⚠⚠⚠ AND IT IS NOT A JOURNAL LINE A READER CAN WEIGH — IT IS TYPED INTO THE AGENT'S PANE, in
    /// the one turn that asks that agent *what a run picking this up should do first*. A run a wall
    /// clock stopped was told it had run out of turns, and everything it wrote back was reasoned
    /// from that. **Register item 261's class, one state over and one layer worse**: 261 misled a
    /// reader of a journal; this misled the WITNESS.
    ///
    /// # ⚠⚠ Why all four ceilings, in one gate, and why each asserts the OTHER THREE are absent
    ///
    /// The defect was a document that said the same true-for-one thing to all four, so a gate
    /// asserting only that SOME clause is present would have passed it unchanged the moment the
    /// clause happened to be `turns`'. What separates a fixed document from the broken one is that
    /// the wrong three clauses are gone — so that is the assertion, and the four needles are checked
    /// mutually exclusive first or it tests nothing.
    ///
    /// ⚠ The prompt is read back through [`AiLoop::authored`] rather than off the pane. It is the
    /// same string [`OuterLoop::advance`] delivered — read out of the same variable, which nothing
    /// assigns after `stopping` composes it — and the pane cannot answer: this peer's account is
    /// twenty-eight lines and scrolls the question off a sixteen-row screen, which the gate above
    /// asserts of the same fixture.
    ///
    /// ⚠⚠ THE THREE READERS MUST AGREE. The agent's question, the walk's word and the run's terminal
    /// `Verdict::Exhausted` are three publications of one fact, and until this round two of them
    /// were fed by two different fields. They are asserted together so a fix that satisfies one and
    /// drifts on another cannot pass.
    #[test]
    fn the_question_a_stopped_run_is_asked_names_the_ceiling_that_stopped_it() {
        // ⚠ THE PREMISE OF THE ABSENCE ASSERTIONS BELOW, CHECKED FIRST: a needle that appeared in
        // another ceiling's clause would make *the wrong three are gone* unfalsifiable.
        for ceiling in Ceiling::ALL {
            for other in Ceiling::ALL {
                assert!(
                    ceiling == other
                        || !crate::testing::stop_said(ceiling)
                            .contains(crate::testing::stop_said(other)),
                    "⚠⚠ the fixture's needles must be mutually exclusive, or the assertions below \
                     cannot tell one ceiling's clause from another: {:?} contains {:?}",
                    crate::testing::stop_said(ceiling),
                    crate::testing::stop_said(other),
                );
            }
        }

        // ⚠ The two doors, driven as four runs. The document's own budget is reached by briefing a
        // SMALL `max_turns` under guardrails that cannot bite; the run's three by putting
        // `max_turns` out of reach and letting each guardrail bind in turn — the gate above's own
        // table, and its comments hold why each number is the number.
        for (guardrails, max_turns, ceiling) in [
            (
                Guardrails {
                    max_iterations: 4_000,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
                2,
                Ceiling::Turns,
            ),
            (
                Guardrails {
                    max_iterations: 8,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
                1_000_000,
                Ceiling::Iterations,
            ),
            (
                Guardrails {
                    max_iterations: 4_000,
                    max_cost: None,
                    max_duration: Some(Duration::from_millis(120)),
                },
                1_000_000,
                Ceiling::Duration,
            ),
            (
                Guardrails {
                    max_iterations: 4_000,
                    max_cost: Some(Cost::Bytes(1)),
                    max_duration: Some(Duration::from_secs(60)),
                },
                1_000_000,
                Ceiling::Cost,
            ),
        ] {
            let (workspace, pane) = crate::testing::standin_agent_reporting(
                crate::testing::Accounts::ForARunThatRanOutOfTurns,
                NO_THINKING,
            );
            let access = supervised(&workspace);
            let mut loops = AiLoop::new(engine(), pane, &brief_for(max_turns), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let progress = ProgressCell::default();
            let outcome = Driver::new(guardrails)
                .reporting_to(Arc::clone(&progress))
                .run(&mut loops, &access, &RunContext::uncancellable());
            let walked: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|entry| entry.note.clone())
                .collect();

            // ── THE CONTROL: this run really did stop on the ceiling the row is about ──
            assert_eq!(
                outcome.state,
                OutcomeState::Exhausted(ceiling),
                "⚠⚠ the control: the row must have driven the ceiling it names, or the prompt \
                 asserted below is a claim about some other run. Walked {walked:?}",
            );

            // ── 1. THE AGENT WAS TOLD THE TRUTH (item 264) ──
            let asked = loops
                .authored()
                .expect("the datamodel must still answer for its prompts")
                .stop;
            assert!(
                asked.contains(crate::testing::stop_said(ceiling)),
                "⚠⚠⚠ REGISTER ITEM 264: a run stopped by its {ceiling:?} ceiling was asked \
                 {asked:?}, which does not name that ceiling. This sentence is TYPED INTO THE \
                 AGENT'S PANE in the turn that asks it what a run picking this up should do first, \
                 so the agent reasons from whatever it says. Expected the clause \
                 {:?}",
                crate::testing::stop_said(ceiling),
            );
            for other in Ceiling::ALL {
                assert!(
                    other == ceiling || !asked.contains(crate::testing::stop_said(other)),
                    "⚠⚠⚠ AND IT NAMED A CEILING THAT DID NOT STOP IT. A run stopped by \
                     {ceiling:?} was asked {asked:?}, which carries {other:?}'s clause \
                     ({:?}) — the exact defect item 264 is about, since the agent cannot check.",
                    crate::testing::stop_said(other),
                );
            }

            // ── 2. AND SO WAS THE READER OF THE WALK (item 265) ──
            //
            // ⚠⚠ The arrow itself has to carry it. Before this, the only thing separating the two
            // doors in a walk was whether the Driver's own `note_to_itself` line PRECEDED the edge
            // — telling them apart by the ABSENCE of a key, and for `turns` there is no such line
            // at all, so that reading had nothing to work with in a quarter of the cases.
            let arrows: Vec<&String> = walked
                .iter()
                .filter(|note| note.contains("--> Stopping"))
                .collect();
            assert!(
                arrows
                    .iter()
                    .all(|note| note.contains(&format!("— {}:", ceiling.wire_str())))
                    && !arrows.is_empty(),
                "⚠⚠⚠ REGISTER ITEM 265: the edge into `stopping` must say WHICH ceiling took it \
                 there. Four ceilings arrive on two transitions, so an arrow that names none reads \
                 identically for a run that spent `max_turns` and one a wall clock stopped. \
                 Arrows: {arrows:?}, walked {walked:?}",
            );
            for other in Ceiling::ALL {
                assert!(
                    other == ceiling
                        || arrows
                            .iter()
                            .all(|note| !note.contains(&format!("— {}:", other.wire_str()))),
                    "⚠⚠⚠ AND THE WALK NAMED A CEILING THAT DID NOT STOP IT: the run ended on \
                     {ceiling:?} and an arrow into `stopping` says {other:?}. Arrows: {arrows:?}",
                );
            }
            access.lifecycle().expect("lifecycle").close(pane);
        }
    }

    /// ⚠⚠⚠ **A CLOCK THAT RUNS OUT *INSIDE* A TURN IS NOT A PERSON'S CANCEL** — the one shape a
    /// fast stand-in cannot stage, and the one a real agent meets every time.
    ///
    /// # ⚠⚠⚠ Why the gate above is not enough
    ///
    /// Measured at five wall clocks against a peer that answers in microseconds, the deadline
    /// always passed BETWEEN two of the driver's steps, so the loop was never inside a wait when it
    /// expired. A real agent's turn is tens of seconds, so a real run's clock expires **inside
    /// `Completion::wait`** — which answers `Over::RunEnded`, and which this driver used to
    /// translate to the machine's `cancel`. That puts the document into a FINAL state, and a
    /// machine already in one has no turn left to give: the account would be asked for on the very
    /// path where it is most wanted and refused every time.
    ///
    /// So the peer here takes a whole second over a working prompt and the run's clock is a
    /// fraction of that. What the gate asserts is the CONSEQUENCE rather than the translation —
    /// the run still ends on its clock, and it still comes back with its agent's account.
    ///
    /// ⚠ The account is asked for AFTER the turn in flight ends, never on top of it. That is the
    /// whole reason `stop_short` sets a flag instead of pushing the machine: the peer is mid-reply,
    /// and a question typed there is typed over a working agent.
    #[test]
    fn a_clock_that_runs_out_while_the_agent_is_working_still_gets_an_account() {
        let (workspace, pane) = crate::testing::standin_agent_reporting(
            crate::testing::Accounts::ForARunThatRanOutOfTurns,
            Duration::from_secs(1),
        );
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(1_000_000), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let started = Instant::now();
        let outcome = Driver::new(Guardrails {
            max_iterations: 4_000,
            max_cost: None,
            // ⚠ A FRACTION OF ONE TURN, so the deadline lands inside the wait rather than between
            // two steps — which is the whole hazard being staged.
            max_duration: Some(Duration::from_millis(200)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let took = started.elapsed();
        let walked: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|entry| entry.note.clone())
            .collect();

        // ⚠ THE CONTROL: the clock has to have expired while the peer was still thinking, or this
        // gate is the one above with a slower fixture.
        assert!(
            took > Duration::from_millis(600),
            "⚠ the run must have been inside a turn when its clock ran out; it took {took:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "⚠⚠⚠ A CLOCK RUNNING OUT IS THE RUN'S OWN CEILING, NOT SOMEBODY'S STOP. `cancelled` \
             here means the loop reported a person's decision about a clock nobody watched — and \
             it is a FINAL state, so the account can never be asked for. Walked: {walked:?}",
        );
        assert!(
            loops
                .captured()
                .is_some_and(|report| report.contains(crate::testing::REPORT_CLOSES)),
            "⚠⚠⚠ AND THE ACCOUNT STILL ARRIVES. This is the path a live run takes every time: a \
             turn of tens of seconds and a clock that expires inside it. Walked: {walked:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A LOOP THAT CANNOT BE ASKED FOR AN ACCOUNT SAYS SO, AND THE REASON IS ABOUT THE
    /// PANE** — the mechanism table item 208's answer rests on, driven at the door itself.
    ///
    /// # ⚠⚠⚠ Why this is asked directly rather than through a run
    ///
    /// The two states below are ones a ceiling has to fall due *while the loop is in them*, and
    /// arranging that through a wall clock is a race: measured, a clock set to 400 ms landed in
    /// `working` on one pass and `screening` on the next, and a gate written that way asserts
    /// whatever the machine happened to be doing. **The claim is about the door, so the door is
    /// what is asked** — and the exhaustive match behind it is what covers the states this gate
    /// does not name.
    ///
    /// ⚠⚠ THE TWO REASONS MUST DIFFER, and that assertion is the half a per-state check cannot
    /// make. One polite sentence would satisfy every *"it refused"* claim while telling a reader
    /// the two situations apart from none — [`Stopped`]'s own distinctness argument, one type over.
    #[test]
    fn a_loop_that_cannot_be_asked_for_an_account_says_what_stops_it() {
        // ── a machine nobody has stepped: its agent was never asked anything ──
        let (idle_workspace, idle_pane) = standin_agent(u32::MAX);
        let idle_access = supervised(&idle_workspace);
        let mut never_started = AiLoop::new(engine(), idle_pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        assert_eq!(
            never_started.state(),
            AiLoopState::Idle,
            "⚠ the control: a loop that has not been stepped has not spoken to anybody",
        );
        let never = never_started.ask_for_an_account(Ceiling::Duration);
        let Accounting::Cannot(never) = never else {
            panic!(
                "⚠⚠⚠ a loop whose agent was never asked anything has nothing to account for, and \
                 taking a window for it would spend a caller's ceiling on a turn that cannot \
                 happen: {never:?}"
            );
        };
        idle_access.lifecycle().expect("lifecycle").close(idle_pane);

        // ── a machine waiting for a person: the pane is not this run's to type in ──
        //
        // ⚠ THE SAME PEER AND SPEC `a_question_no_rule_claims_pauses_the_run_and_a_person_resumes_
        // it` uses, deliberately: reaching this state is that gate's subject, and a second
        // arrangement for reaching it would be a second thing to keep working.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, 2, None);
        let access = crate::testing::supervised_asking(&workspace);
        // ⚠⚠ SOMEBODY IS EXPECTED, which is what makes the loop STAY in `awaiting_human` rather
        // than ending on the first dialog — `Attended::NoOne`, this file's usual value, is the
        // honest answer for a pane nobody is looking at and passes through the state in one pump.
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠⚠ SOMEBODY IS EXPECTED, and it is the BRIEF that says so since the patience became
            // the document's — see `Brief::await_person_ms`.
            &Brief {
                await_person_ms: Some(30_000),
                handback_still_ms: Some(300),
                // ⚠ A SHORT TURN BOUND, about this gate's COST rather than its claim: a pump that
                // finds nothing blocks for the turn's whole patience, and this one pumps until the
                // dialog has been met and screened. It stays above `supervised_asking`'s settle.
                // ⚠⚠ IT IS ON THE BRIEF because the bound is the document's — item 300.
                turn_within_ms: Some(1_000),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        // ⚠ NO CONSENT AND NO RULE, so the first dialog is one nothing here can answer, and
        // `screen.none` leads to the wait. Stepped by hand for this gate's stated reason.
        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        for _ in 0..40 {
            if loops.state() == AiLoopState::AwaitingHuman {
                break;
            }
            let step = loops
                .step(&access, &run)
                .expect("every step of a paused run must be readable");
            if let Some(note) = step.note {
                walked.push(note);
            }
        }
        assert_eq!(
            loops.state(),
            AiLoopState::AwaitingHuman,
            "⚠ the control: the loop must be waiting for the person before the door is asked: \
             {walked:?}",
        );
        let waiting = loops.ask_for_an_account(Ceiling::Duration);
        let Accounting::Cannot(waiting) = waiting else {
            panic!(
                "⚠⚠⚠ THE PANE IS NOT THIS RUN'S TO TYPE IN. A dialog nothing here may answer is on \
                 it, or somebody is typing — and asking *where did you get to* would ANSWER THAT \
                 DIALOG or type under a hand. That is the same refusal `screen.none` and \
                 `turn.interrupted` already make: {waiting:?}"
            );
        };
        assert_ne!(
            never, waiting,
            "⚠⚠⚠ TWO SITUATIONS, ONE SENTENCE. A reader told only *there is no account* cannot act \
             on it; the whole value of the refusal is that it names what would have to change \
             first",
        );
        assert!(
            waiting.contains("not this run's to type in"),
            "⚠⚠ and the sentence has to be about the PANE rather than about a policy anybody \
             chose: {waiting:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **WAITING FOR A PERSON COSTS ONE STEP, NOT ONE PER LOOK** — register items 279 and 280,
    /// and the run they ended.
    ///
    /// # ⚠⚠⚠ What went wrong, so the number below is read as a claim and not as a tolerance
    ///
    /// `awaiting_human` asked [`Completion::wait`] whether the TURN had ended. **A dialog is an
    /// ending**, so that wait answered `Asking` on its first look and never waited at all; the arm
    /// turned it into `Null`, and the Driver — which pauses between steps for nothing — asked
    /// again. Measured on a live run: **~100,000 steps in the hour it sat at one permission
    /// dialog**, against 64 steps for a nine-hour run that never sat at one. The iteration ceiling,
    /// which the document cannot see, then ended the run and reported *exhausted (iterations)* —
    /// a sentence about a hundred thousand steps of work, for thirteen transitions and a question
    /// nobody answered.
    ///
    /// # ⚠⚠ Why STEPS and not elapsed time is the assertion
    ///
    /// The defect is not that the wait was short — it is that the wait was **re-derived**. A gate
    /// on duration passes for a driver that spins for exactly as long; only counting the steps says
    /// the wait was ONE wait. ⚠ Both are asserted anyway: a step count of one with no elapsed time
    /// would mean the patience was skipped, which is the opposite defect and just as wrong.
    ///
    /// # ⚠⚠⚠ WHAT THIS GATE DOES **NOT** HOLD, AND THE ARM IT CANNOT REACH
    ///
    /// **It does not reach `Over::Asking` — the arm the fix is in.** Restoring
    /// `Over::Asking(_) => return Null` leaves this GREEN, which was measured, not assumed.
    ///
    /// ⚠⚠⚠⚠ **AND THE REASON IS A DIFFERENT ROUTE, NOT A WEAK FIXTURE** — which took three
    /// arrangements to learn. Register item 297 concluded *"the fixture is what is missing, not the
    /// path"*, and a peer that WORKS BEFORE ASKING was built for it
    /// ([`Asks::OnItsFirstPromptAfterWorking`], since a peer that blocks having painted nothing
    /// leaves [`Completion::asked`]'s `seen.seq > began_at` false). It changed nothing here, and
    /// the mutation that says why is decisive: killing `Reached::Asking => TurnBlocked` — the
    /// READINESS barrier's route — makes this gate fail to enter `awaiting_human` at all, walking
    /// forty steps of *"looked, nothing had happened"* instead.
    ///
    /// So this loop meets a dialog at its BARRIER, before `Completion::wait` is ever consulted, and
    /// no fixture on the completion side can change that. A gate for `Over::Asking` needs a peer
    /// that goes quiet asking **after the barrier has already passed** — a different arrangement
    /// altogether, and the one item 297 is really asking for.
    ///
    /// ⚠⚠⚠ **THE ARM IS REACHED IN PRODUCTION AND THE ARITHMETIC IS THE PROOF.** On `NotYet` the
    /// wait parks for the turn's bound — 30 minutes on the live run — which is about two steps an
    /// hour. The live run spent **~100,000 steps in one hour** at one dialog. Only the immediate
    /// return of the `Asking` arm produces that number, so the fixture is what is missing, not the
    /// path. **Registered rather than papered over** — a fixture that cannot stage the hazard makes
    /// every gate standing on it weaker than the product.
    ///
    /// What this DOES hold: the wait for a person ends in one step and lasts at least the declared
    /// patience, on the path these fixtures can reach.
    #[test]
    fn a_run_waiting_for_a_person_spends_one_step_on_the_whole_wait() {
        /// Short enough to keep the gate cheap, long enough that a spinning driver needs hundreds
        /// of steps to cross it. ⚠ It is the PERSON's patience, not the turn's.
        const PATIENCE: Duration = Duration::from_millis(400);
        /// ⚠ Far above [`PATIENCE`], so *left on the person's clock* and *left on the turn's* are
        /// distinguishable by how long it took — see the spec below.
        const TURN_BOUND: Duration = Duration::from_secs(8);

        // ⚠⚠⚠ THE PEER MUST ASK *DURING A TURN THIS LOOP ARMED*, and the first draft of this gate
        // did not get that. `Completion::asked` answers only for an ARMED evaluator whose peer has
        // MOVED since the prompt — so a fixture that arrives at `awaiting_human` some other way
        // leaves `ended()` answering `None`, the wait falls to `Over::NotYet`, and that arm was
        // never the broken one. Measured: the earlier arrangement left after **16.4s** — twice the
        // TURN's bound — where the arm under test leaves after the PERSON's patience.
        let (workspace, pane) = crate::testing::standin_agent_asking(
            crate::testing::Asks::OnItsFirstPromptAfterWorking,
        );
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠ The PERSON's clock is the document's now, so the gate authors it through the brief.
            &Brief {
                await_person_ms: Some(PATIENCE.as_millis() as i64),
                handback_still_ms: Some(50),
                // ⚠⚠⚠ THE TURN'S BOUND IS DELIBERATELY MUCH LONGER THAN THE PERSON'S PATIENCE, and
                // that is what makes this gate able to tell the two waits apart. `attend` hands the
                // TURN's bound to `Completion::wait` and the PERSON's to its own arms, so a run
                // that leaves after roughly the patience left on the person's clock and one that
                // leaves after the turn's bound are different code paths with the same ending.
                // Equal numbers hid that, and the first draft of this gate passed under its own
                // mutation because of it.
                // ⚠⚠ BOTH NUMBERS ARE NOW ON ONE SURFACE, which is the shape item 300 argued for:
                // they are the same KIND of value and were in different worlds.
                turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        for _ in 0..40 {
            if loops.state() == AiLoopState::AwaitingHuman {
                break;
            }
            let step = loops
                .step(&access, &run)
                .expect("every step of a paused run must be readable");
            if let Some(note) = step.note {
                walked.push(note);
            }
        }
        assert_eq!(
            loops.state(),
            AiLoopState::AwaitingHuman,
            "⚠ the control: the loop must be WAITING for a person before the wait can be \
             measured. Walked {walked:?}",
        );

        // ── the measurement: how much does the wait itself cost the run? ──
        let began = std::time::Instant::now();
        let mut spent = 0_u32;
        while loops.state() == AiLoopState::AwaitingHuman && spent < 2_000 {
            loops
                .step(&access, &run)
                .expect("a waiting run is still readable");
            spent += 1;
        }
        let took = began.elapsed();

        assert!(
            spent <= 2,
            "⚠⚠⚠ THE WAIT MUST BE ONE WAIT. Leaving `awaiting_human` took {spent} steps over \
             {took:?}, so the driver is re-deriving the same unchanged screen instead of parking \
             on the one condition that ends a wait for a person (`readiness::moved_on`). Every one \
             of those steps is an iteration charged against a ceiling the document cannot see, and \
             on a live run that arithmetic ended it: ~100,000 steps in one hour at one dialog.",
        );
        assert!(
            took >= PATIENCE,
            "⚠⚠⚠ AND IT MUST ACTUALLY HAVE WAITED. Leaving took only {took:?} of a {PATIENCE:?} \
             patience, which is the opposite defect: a person who was promised that long did not \
             get it, and the run gave up while they were still on their way.",
        );
        // ⚠⚠⚠⚠ **AND IT MUST HAVE LEFT ON THE PERSON'S CLOCK, NOT THE TURN'S** — the assertion
        // register item 297 was missing, and without which this gate passed under its own mutation.
        // `Over::NotYet` parks for the TURN's bound; only `Over::Asking` returns on the patience
        // authored above. With both bounds equal the two are one number, which is why the spec sets
        // them an order of magnitude apart — see `TURN_BOUND`.
        assert!(
            took < TURN_BOUND,
            "⚠⚠⚠⚠ THE RUN LEFT ON THE TURN'S CLOCK, NOT THE PERSON'S. {took:?} is past the \
             {TURN_BOUND:?} turn bound and far past the {PATIENCE:?} a person was promised, so \
             this wait ended on `Over::NotYet` and the `Over::Asking` arm — the one that notices a \
             peer has STOPPED TO ASK — was never entered. On the live run that difference was \
             ~100,000 steps in one hour at a single dialog.",
        );
        assert_ne!(
            loops.state(),
            AiLoopState::AwaitingHuman,
            "⚠⚠ and the wait must END — a run that waits for ever and a run that is dead are the \
             same thing to every reader",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A WINDOW THAT RAN OUT BEFORE THE ACCOUNT ARRIVED SAYS SO, IN THE RUN'S JOURNAL** —
    /// the silence item 208 removes, put back one step later and removed again.
    ///
    /// # ⚠⚠⚠ Why the Driver can say this without reading a word of the account
    ///
    /// A plugin that FINISHES its account reports a terminal verdict, and the run ends there. So
    /// the only way the account window's own clock can run out is that the turn asking for it never
    /// ended — which the Driver knows from having reached its own loop top a second time, and which
    /// it says without ever touching [`Plugin::captured`]. **`exhausted` with an empty report is
    /// otherwise indistinguishable from a build that captures nothing at all**, which is the exact
    /// confusion the whole capture exists to remove.
    ///
    /// ⚠ The staging is deterministic rather than timed: the clock is short enough to fall due
    /// while the loop is still working, and the peer then meets a dialog nothing may answer — so
    /// the account turn provably cannot end, whatever else the machine is doing when the ceiling
    /// arrives.
    #[test]
    fn an_account_that_never_arrives_says_that_its_window_ran_out() {
        let (workspace, pane) =
            crate::testing::standin_agent_asking(crate::testing::Asks::OnItsFirstPrompt);
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠ A person is expected and the pane is never taken back — `handback_still_ms` of zero
            // is `Handback::Never`, which is what this gate held before the two moved here.
            &Brief {
                await_person_ms: Some(60_000),
                handback_still_ms: Some(0),
                // ⚠⚠ THIS IS ALSO THE ACCOUNT'S WINDOW — the plugin sizes the turn it is granted
                // from the bound declared for a turn, so a short one here is what keeps this gate
                // cheap AND what it is measuring. ⚠ The declaration is the DOCUMENT's now, which
                // is why the plugin reads it back through `OuterLoop::turn_within` at act time.
                turn_within_ms: Some(1_000),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 4_000,
            max_cost: None,
            max_duration: Some(Duration::from_millis(400)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let journal = progress.lock().expect("the progress cell").journal.clone();
        let walked: Vec<String> = journal
            .iter()
            .filter_map(|entry| entry.note.clone())
            .collect();

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "⚠ the control: the run's own CLOCK must be what ended it. Walked {walked:?}",
        );
        assert_eq!(
            loops.captured(),
            None,
            "⚠ and the control's other half: there is no account, which is what makes the line \
             below the only thing a reader has",
        );
        let last = walked.last().expect("a run writes a journal");
        assert!(
            last.contains("no account") && last.contains("ran out first"),
            "⚠⚠⚠ THE RUN MUST SAY THAT ITS ACCOUNT WAS CUT SHORT. Without it a caller cannot tell \
             *the agent was never asked*, *the agent said nothing*, and *this build captures \
             nothing* apart — and only the first two are things they can do anything about: \
             {last:?}",
        );
        assert!(
            matches!(
                journal.last().map(|entry| &entry.verdict),
                Some(Verdict::Exhausted(Ceiling::Duration)),
            ),
            "⚠⚠ and the line names the ceiling that fell due, so a reader is sent to the knob they \
             set: {:?}",
            journal.last(),
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A BRIEF THAT WOULD REACH A STATE NOBODY BUILT IS REFUSED BEFORE A BYTE IS TYPED** —
    /// and the document's own shipped numbers are no longer one of them.
    ///
    /// # ⚠⚠⚠ THE HALF THAT WAS REMOVED, kept here because its going is the round's headline
    ///
    /// This gate used to assert that the document's shipped pair — `reflect_every` 8 against
    /// `max_turns` 40 — was REFUSED, because the default brief walks into `reflecting` at turn eight
    /// and the session-replace lifecycle behind that state did not exist. It does now, so the same
    /// brief STARTS, and that is asserted below rather than left as an absence: a refusal that
    /// quietly stopped happening would leave a caller reading a sentence about a knob nothing
    /// enforces.
    ///
    /// What survives is the other end of the same arithmetic: a loop allowed NO turn can only reach
    /// `exhausted`, which is a run nobody wanted.
    ///
    /// ⚠⚠ **THE PANE IS THE ASSERTION**, not just the refusal: the whole value of refusing at the
    /// door is that nothing happened, and a refusal that had already typed the first prompt would
    /// be worth nothing. The screen is checked to be exactly what it was.
    #[test]
    fn a_brief_that_would_reach_an_unbuilt_state_is_refused_before_anything_is_typed() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let before = access.pane_collapsed(pane).expect("a readable pane");

        // ⚠⚠⚠ THE DOCUMENT'S OWN SHIPPED PAIR NOW STARTS. Every other assertion here is about a
        // refusal, and this one exists so that the refusal's DISAPPEARANCE is a thing a test says
        // out loud.
        AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 8,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect(
            "⚠⚠⚠ the document's shipped `reflect_every: 8` reaches `reflecting`, and a run that \
             reaches it is one this build drives — see `restarting`",
        );

        // ⚠ AND A LOOP THAT CANNOT TAKE A TURN AT ALL, at the other end of the same arithmetic.
        assert_eq!(
            AiLoop::new(engine(), pane, &brief_for(0), &standin_spec())
                .expect_err("a loop allowed no turns judges itself exhausted before it starts"),
            NotStarted::Unbuilt(AiLoopState::Exhausted),
            "the refusal must name the STATE, so the sentence a caller reads can name the knob",
        );

        assert_eq!(
            access.pane_collapsed(pane).expect("a readable pane"),
            before,
            "⚠⚠⚠ NOTHING WAS TYPED. Neither the refusal nor the loop that started has spoken to the \
             pane — a door that prompted an agent before answering would have cost exactly what \
             answering early exists to save",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A LOOP WHOSE AGENT STOPS TO ASK PERMISSION** — register items 119/120/112, measured
    /// with today's API before anything was built for it.
    ///
    /// Every live measurement of this loop picked an ARITHMETIC milestone *"so no permission dialog
    /// can fire"*, and the model itself wrote back that the next step needed real work. **Every
    /// kind of real work raises one of these.** This is what happens when one does.
    ///
    /// ⚠⚠ IT IS A PAIR, and the pair is the whole claim:
    ///
    /// * **given no consent that is ABOUT this dialog, the run STOPS**, publishes the question, and
    ///   keeps the consent-level reason underneath — which is honest and is the end of the run;
    /// * **given a consent that IS about it, the same peer, the same brief and the same fixture
    ///   CONVERGE** — the dialog is answered, the loop takes its next turn and the agent reaches
    ///   the milestone.
    ///
    /// Either half alone would be a fact about one arrangement. Together they say the consent is
    /// what makes the difference, and nothing else about the run changed.
    ///
    /// # ⚠⚠⚠ Both halves BRIEF a consent now, and the unarmed one is the interesting change
    ///
    /// Briefing nothing no longer means *answer nothing* — it means **the document decides**, and
    /// the shipped document authors the two dialogs a working loop meets: the EDIT question and the
    /// COMMAND question. This peer raises the command one, so an unbriefed run would be ANSWERED
    /// and this pair would have no unarmed half at all.
    ///
    /// ⚠⚠ So the unarmed half briefs a consent that is deliberately **about a different question**,
    /// which REPLACES the document's clauses (`Brief::may_answer`'s rule: `Some` overrides, `None`
    /// echoes). That stages the hazard the name claims — a run holding consents, none of them about
    /// the dialog on screen — and it no longer depends on what the shipped document happens to say.
    ///
    /// ⚠ **WHAT NO GATE HERE CAN STAGE ANY MORE IS `no_consent`** — a run with no clauses at all.
    /// A caller cannot spell it ([`Consents::of`](crate::consent::Consents::of) refuses the empty
    /// list, and an absent `may_answer` defers to the document), so it is reachable only by
    /// authoring `[]` in the file. Registered rather than papered over.
    #[test]
    fn a_loop_whose_agent_asks_stops_unless_a_consent_is_about_the_question() {
        /// One run against a peer that raises a permission dialog on its first turn, with whatever
        /// answering contract `may_answer` declares — and what became of it.
        fn run_with(may_answer: Option<crate::consent::Consents>) -> (OutcomeState, Option<i64>) {
            let (workspace, pane) =
                crate::testing::standin_agent_asking(crate::testing::Asks::OnItsFirstPrompt);
            let access = crate::testing::supervised_asking(&workspace);
            let mut loops = AiLoop::new(
                engine(),
                pane,
                // ⚠ The consent is the DOCUMENT's now, so the gate authors it through the brief.
                &Brief {
                    may_answer,
                    ..brief_for(40)
                },
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .run(&mut loops, &access, &RunContext::uncancellable());
            let turns = loops.turns();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome.state, turns)
        }

        // ── THE DEFECT, IN ITS OWN WORDS ──
        let (unarmed, unarmed_turns) = run_with(Some(a_consent_about_something_else()));
        let OutcomeState::Blocked(Some(unanswered)) = &unarmed else {
            panic!(
                "⚠⚠⚠ a loop whose agent stops to ask must report BLOCKED with the question, not \
                 {unarmed:?} — anything else is a run that typed at a menu or died silently",
            );
        };
        // ⚠⚠⚠ THE HEAD REASON MOVED WHEN `screening` WAS BUILT, and keeping BOTH halves asserted
        // is the point rather than a repair. Screening is now the last authority to look at this
        // dialog, so the arm is its answer — but the CONSENT-level reason is what makes the second
        // half of this pair possible, and a report that lost it would send a caller to write a
        // standing rule about a dialog whose own `Yes` a clause could take. See
        // `Unanswered::unscreened`.
        assert_eq!(
            unanswered.why(),
            crate::consent::Refusal::NoRule,
            "⚠⚠ the LAST authority to look at the dialog is what the arm names, and after \
             `screening` exists that is the rules: {unanswered:?}",
        );
        assert!(
            unanswered
                .explain()
                .contains(crate::consent::Refusal::OtherQuestion.wire_str()),
            "⚠⚠⚠ AND THE CONSENT-LEVEL REASON MUST SURVIVE UNDERNEATH. It is the reason whose \
             remedy is a change to the CONSENTS — the very change the second half of this pair \
             makes — and a run that reported only `no_rule` would send its author to write a \
             standing instruction about a dialog that offers `Yes`. ⚠ It reads `other_question` \
             and not `no_consent` because the document authors a clause now: this run HAS \
             consents and none of them is about this question, which is a different sentence with \
             a different remedy — widen a needle, do not write one: {}",
            unanswered.explain(),
        );
        assert!(
            unanswered.question().is_some(),
            "⚠⚠ and it must publish the question itself, or a person coming back to this run has \
             to go and read the pane: {unanswered:?}",
        );
        assert_eq!(
            unarmed_turns,
            Some(0),
            "⚠⚠⚠ THE NUMBER THIS DEFECT COSTS: the loop stops on the FIRST dialog, with ZERO turns \
             judged. Every milestone that touches a file ends here",
        );

        // ── AND THE SAME EVERYTHING, PLUS ONE CLAUSE ──
        let consent = crate::consent::Consents::of(vec![
            crate::consent::Consent::parse(
                "Do you want to proceed?".to_string(),
                "Yes".to_string(),
            )
            .expect("both needles are non-empty"),
        ])
        .expect("a non-empty consent list");
        let (armed, armed_turns) = run_with(Some(consent));
        assert_eq!(
            armed,
            OutcomeState::Converged,
            "⚠⚠⚠ the caller's own consent must carry the loop THROUGH the dialog and on to its \
             milestone. Nothing else about this run differs from the one above",
        );
        assert!(
            armed_turns.is_some_and(|turns| turns >= 1),
            "⚠⚠ and the agent must have taken a real turn on the other side of the question, or \
             `converged` would be a word about a run that never got past it: {armed_turns:?}",
        );
    }

    /// ⚠⚠⚠ **A LOOP CARRIES OUT ITS AUTHOR'S STANDING INSTRUCTION ON A DIALOG NO CONSENT CAN
    /// ANSWER** — register items 119, 5 and 142, and the state `screening` was built for.
    ///
    /// # ⚠⚠⚠ Why the peer here asks something a consent structurally cannot reach
    ///
    /// The gate above this one arms a [`Consents`](crate::consent::Consents) clause and the run goes
    /// through, which settles the case where the answer is ON THE MENU. This peer asks *"Which way
    /// should I build this?"* and offers *"The quick one"* / *"The thorough one"* — **there is no
    /// option a caller could authorise in advance**, because the whole point of the question is that
    /// the answer is not one of the things being offered. That is the dialog `screen_rules` exist
    /// for, and until this round it ended the run.
    ///
    /// ⚠⚠ **IT IS A PAIR, and the pair is the claim:**
    ///
    /// * **with no rule that claims it**, the run stops and says `no_rule` — naming the dialog, so
    ///   the author learns what to quote;
    /// * **with one rule quoting the dialog**, the same peer and the same brief CONVERGE: the call
    ///   is turned down, the agent is told what to do instead, and it does it.
    ///
    /// ⚠⚠⚠ **AND THE RUN SAYS WHICH KIND OF DECISION IT TOOK.** `screened` is 1 and `answered` is
    /// **0** — measured, because the act refuses rather than approves and a person auditing this run
    /// must not find it counted among the things their agent was allowed to do.
    #[test]
    fn a_loop_carries_out_the_standing_instruction_its_author_wrote() {
        /// The words the stand-in's dialog carries, which a rule quotes.
        const ASKS: &str = "Which way should I build this?";
        /// What the standing instruction says instead. ⚠ It carries no marker and no `exactly:`, so
        /// a peer that converged off THIS text rather than off its own next turn could not.
        const INSTEAD: &str = "neither; do the smallest verifiable thing and report";

        /// One run against the peer that asks an unanswerable question, with whatever standing
        /// instructions the author left it — and what became of it.
        fn run_with(
            screen_rules: Option<crate::screen::ScreenRules>,
        ) -> (
            crate::driver::Outcome,
            Option<i64>,
            String,
            Vec<String>,
            Option<i64>,
        ) {
            run_against(screen_rules, true)
        }

        /// One run against a peer that asks an unanswerable question and either does or does not
        /// take the key that refuses it — and, beside the run's own account, **the DOCUMENT's
        /// count of what it screened**, so the two authorities can be compared.
        fn run_against(
            screen_rules: Option<crate::screen::ScreenRules>,
            takes_the_key: bool,
        ) -> (
            crate::driver::Outcome,
            Option<i64>,
            String,
            Vec<String>,
            Option<i64>,
        ) {
            // ⚠ ONE turn after the redirect, which is what makes the ENDING carry the claim: the
            // peer does what it was told instead and then says the marker. The gate below this one
            // drives the same peer with a number it can never reach, because *"how often is the
            // original milestone asked for again?"* is a question a converging peer cannot answer.
            let (workspace, pane) = crate::testing::standin_agent_refusing(takes_the_key, 1, None);
            let access = crate::testing::supervised_asking(&workspace);
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &Brief {
                    screen_rules,
                    ..brief_for(40)
                },
                // ⚠ NO CONSENT AT ALL, which is the control for the whole gate: nothing this run
                // holds can take an option, so whatever gets it past the dialog is `screening`.
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            // ⚠⚠ THE WALK IS CARRIED INTO EVERY FAILURE MESSAGE BELOW, R378's lesson: a loop that
            // does not reach its ending is diagnosable by its total alone only if nobody thought
            // to keep the journal, and this gate's first run stalled with `exhausted — duration`
            // saying nothing about where.
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &access, &RunContext::uncancellable());
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            let turns = loops.turns();
            let screened = loops.screened();
            // ⚠⚠⚠ THE PANE THE RUN ENDED ON, and not `pane`. A screening now triggers a reflection,
            // and a reflection that is ADOPTED replaces the inner session — so the pane this run was
            // handed is closed, and its echo trail answers `""`. Read it and every assertion about
            // what the loop said becomes an assertion about a pane nobody is driving. ⚠ In the third
            // arm nothing is adopted (the key does not land), so this is the ORIGINAL pane and the
            // negative assertion still means what it says.
            let live = access.pane_ids();
            let typed = live
                .first()
                .and_then(|last| {
                    access
                        .input_echo()
                        .and_then(|echo| echo.pane_recent_input(*last))
                })
                .unwrap_or_default();
            for pane in live {
                access.lifecycle().expect("lifecycle").close(pane);
            }
            (outcome, turns, typed, walk, screened)
        }

        // ── THE DEFECT: A DIALOG THE AUTHOR NEVER QUOTED ──
        //
        // ⚠ The document's shipped rule is an `(edit me)` placeholder that claims nothing, so this
        // is the SHIPPED loop meeting a real question.
        let (unarmed, unarmed_turns, _, unarmed_walk, unarmed_screened) = run_with(None);
        let OutcomeState::Blocked(Some(unanswered)) = &unarmed.state else {
            panic!(
                "⚠⚠⚠ a loop whose agent asks something nothing covers must report BLOCKED with the \
                 question: {:?} — walked {unarmed_walk:?}",
                unarmed.state,
            );
        };
        assert_eq!(
            unanswered.why(),
            crate::consent::Refusal::NoRule,
            "⚠⚠⚠ and the reason must be the one whose remedy is a RULE. `no_consent` here would \
             send an author to write a clause about a menu that offers nothing they could have \
             authorised — the exact dead end this state exists to end: {unanswered:?}",
        );
        assert!(
            unanswered.question().is_some(),
            "⚠⚠ and it must publish the dialog, or the author cannot see what to quote",
        );
        assert_eq!(
            unarmed_turns,
            Some(0),
            "⚠⚠⚠ THE NUMBER THIS COSTS: the loop stops on the FIRST such question, with ZERO turns \
             judged",
        );
        assert_eq!(
            (unarmed.screened, unarmed.answered),
            (0, 0),
            "⚠ the control: a run that got past nothing decided nothing",
        );
        assert_eq!(
            unarmed_screened,
            Some(0),
            "⚠⚠ and the DOCUMENT counts none either — its `screened` is incremented on \
             `screen.matched` and NOT on entering the state, so a loop that only LOOKED at a \
             dialog must not have counted it. Walked {unarmed_walk:?}",
        );

        // ── AND THE SAME EVERYTHING, PLUS ONE STANDING INSTRUCTION ──
        let rules = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let (armed, armed_turns, typed, walk, armed_screened) = run_with(Some(rules));
        assert_eq!(
            armed.state,
            OutcomeState::Converged,
            "⚠⚠⚠ the author's own standing instruction must carry the loop THROUGH the dialog and \
             on to its milestone. Nothing else about this run differs from the one above: {:?} — \
             walked {walk:?}",
            armed.state,
        );
        assert_eq!(
            (armed.screened, armed.answered),
            (2, 0),
            "⚠⚠⚠ AND THE RUN MUST SAY WHICH DECISION IT TOOK. This act REFUSED a call; a run that \
             counted it among the things it approved would answer *what did my agent get to do?* \
             with the opposite fact. ⚠⚠ TWO, not one, since `reflecting` was built: the first \
             screening is ADOPTED, which replaces the session, and this fixture asks on its first \
             prompt whatever it has been told — so the fresh agent asks and the rule fires again. \
             What the adoption buys is visible in the walk rather than in this number: the second \
             reflection is `ReflectNone` and costs no restart. Walked {walk:?}",
        );
        assert_eq!(
            armed_screened.map(|count| u32::try_from(count).unwrap_or(u32::MAX)),
            Some(armed.screened),
            "⚠⚠⚠ AND THE DOCUMENT'S OWN COUNT MUST AGREE WITH THE RUN'S. There are two authorities \
             over this one fact — `screened` in the datamodel, incremented on `screen.matched`, and \
             the Driver's tally of `Verdict::Screened` steps — and two numbers nobody compares is \
             how one of them quietly becomes folklore. Walked {walk:?}",
        );
        assert!(
            armed_turns.is_some_and(|turns| turns >= 1),
            "⚠⚠ and the agent must have taken a real turn on the other side of the question, or \
             `converged` would be a word about a run that never got past it: {armed_turns:?}",
        );
        assert!(
            typed.contains(INSTEAD),
            "⚠⚠⚠ and the REDIRECT must have reached the pane. A run that refused the call and said \
             nothing would leave the agent turned down with no next thing to do, which is the \
             failure `Malformed::SaysNothing` refuses at construction. Typed: {typed:?}",
        );

        // ── ⚠⚠⚠ AND THE THIRD ARM: A DIALOG THAT WILL NOT GO ──
        //
        // The same rule, the same brief, and a peer whose dialog ignores the refusing key. This is
        // the `Tab` arm of the live probe rebuilt where it can be run for free, and what it asserts
        // is a NEGATIVE: **nothing was typed**. A dialog still on the screen reads an Enter as an
        // answer to ITSELF — the probe measured a file being written by exactly this — so a
        // screening act that cannot prove the question is gone must say its piece to nobody.
        let same_rule = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let (stuck, stuck_turns, stuck_typed, stuck_walk, stuck_screened) =
            run_against(Some(same_rule), false);
        let OutcomeState::Blocked(Some(unmoved)) = &stuck.state else {
            panic!(
                "⚠⚠⚠ a rule that fired against a dialog that will not go must end the run BLOCKED, \
                 not carry on as though it had been carried out: {:?} — walked {stuck_walk:?}",
                stuck.state,
            );
        };
        assert_eq!(
            unmoved.why(),
            crate::consent::Refusal::NotDismissed,
            "⚠⚠ and the reason must separate *nothing claimed this* from *something did and the \
             key did not land* — the second is a fact about the AGENT, since the key is the \
             product's: {unmoved:?}",
        );
        assert!(
            !stuck_typed.contains(INSTEAD),
            "⚠⚠⚠ **THE ONE ASSERTION THIS WHOLE ACT IS ORDERED FOR.** The redirect must NOT have \
             reached a pane whose dialog is still up. A live probe typed into exactly this and the \
             Enter behind it APPROVED THE FILE WRITE the agent had asked permission for — and \
             `deliver` reported the text confirmed on screen while it happened. Typed: \
             {stuck_typed:?}",
        );
        assert!(
            unmoved.bytes() > 0,
            "⚠⚠ THE CONTROL: the refusing key really was pressed, or the assertion above is about \
             an act that never began: {unmoved:?}",
        );
        assert_eq!(
            (stuck.screened, stuck_turns, stuck_screened),
            (0, Some(0), Some(0)),
            "⚠⚠ and NEITHER authority counted it, and no turn was judged — a run that counted this \
             among the calls it got past would be reporting a decision it did not manage to take. \
             Walked {stuck_walk:?}",
        );
        // ⚠⚠⚠ AND THE KEY WAS CHARGED FOR. Every other plugin reports `Cost::Bytes(asking.bytes())`
        // on a `Blocked`, and a loop reported zero — true for as long as a loop could not type at a
        // dialog, and false the moment `screening` could press a key and give up. A ceiling that
        // cannot see what a run typed into somebody's dialog is a ceiling with a hole in it.
        assert!(
            matches!(stuck.cost, Some(crate::plugin::Cost::Bytes(spent)) if spent >= unmoved.bytes()),
            "⚠⚠ the run must charge for the refusing key it really pressed. The refusal says it \
             cost {} and the run reports {:?}",
            unmoved.bytes(),
            stuck.cost,
        );
    }

    /// ⚠⚠⚠ **A STANDING INSTRUCTION REACHES EVERY PROMPT ONCE IT HAS BEEN ADOPTED** — register item
    /// 148, paid; and the LIVE AGENT of R384 is what reported the defect, mid-gate, in its own words:
    ///
    /// > *"the loop is re-issuing the original milestone, but your last direct instruction was to
    /// > not create that file … 루프가 매 턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절하게
    /// > 되므로, 진전이 없습니다."*
    ///
    /// # ⚠⚠⚠ THE NUMBERS, BEFORE AND AFTER, on the same six-turn run
    ///
    /// **BEFORE** — this gate's first form, and what it asserted: the instruction reached the pane
    /// exactly **ONCE**, `turn_prompt` asked for the original milestone **SIX** times, and
    /// `authored().turn` did not carry the instruction at all. The loop spent its budget asking for
    /// something its author had already forbidden, and a peer that keeps refusing makes no progress.
    ///
    /// **AFTER** — the same peer and the same rule, with `reflecting` and `restarting` built: the
    /// screening triggers a reflection at the very next judgement, the session is replaced, and from
    /// then on **every prompt that names the milestone carries the instruction with it**. So the two
    /// counts no longer diverge — that is the whole claim, and it is asserted as a relation between
    /// them rather than as one number, because what matters is that no prompt asks for the milestone
    /// without saying what overrides it.
    ///
    /// ⚠⚠ **THE `authored()` HALF IS THE SHARPER ONE.** What went into the pane is history; what the
    /// loop is GOING to say next is `turn_prompt`, and after a reflection it carries the instruction —
    /// including `start_prompt`, which is what a FRESH agent that remembers nothing is greeted with.
    ///
    /// ⚠⚠⚠ **AND THE PANE IT READS IS THE ONE THE RUN ENDED ON**, not the one it started over. A
    /// reflection closes the inner session, so the echo trail of the original pane holds only the
    /// first session's prompts — a gate that kept reading it would report the BEFORE numbers for ever
    /// and call them a pass.
    #[test]
    fn a_standing_instruction_reaches_every_prompt_once_it_has_been_adopted() {
        /// The dialog the peer raises, quoted by the rule below.
        const ASKS: &str = "Which way should I build this?";
        /// What the author's standing instruction says instead. ⚠ Distinctive, and carrying neither
        /// `exactly:` nor `Summarise`, so counting it counts only the screening act.
        const INSTEAD: &str = "not that way; do the smallest verifiable thing and report";
        /// The brief's milestone. ⚠ It must share no substring with `done_instruction` — which
        /// carries the words `MILESTONE REACHED` into every single prompt — or the count below
        /// would be counting the marker.
        const AIM: &str = "the-original-thing";
        /// The turn budget. ⚠ Small deliberately: the pane's echo trail is
        /// [`ECHO_TRAIL_CAP`](sprag_terminal::pane_pty::ECHO_TRAIL_CAP) bytes and a longer run would
        /// make the counts a question about that bound instead.
        const TURNS: i64 = 6;

        // ⚠ A peer that NEVER converges after the redirect — the shape the live agent took. It does
        // what it was redirected to, comes back, and is asked for the original milestone again.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, u32::MAX, None);
        let access = crate::testing::supervised_asking(&workspace);
        let rules = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                milestone: AIM.to_owned(),
                screen_rules: Some(rules),
                ..brief_for(TURNS)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 200,
            max_cost: None,
            max_duration: Some(Duration::from_secs(120)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        // ⚠⚠⚠ THE PANE THE RUN ENDED ON, which is not the one it started over: a reflection replaced
        // the inner session. The original is closed, so `pane_ids` holds exactly the survivor — and
        // reading the OLD pane's echo trail is how this gate would go on reporting the BEFORE numbers
        // for ever. `driving()` cannot be asked, and correctly so: an `exhausted` loop answers `None`
        // because its peer is at rest.
        let live = access.pane_ids();
        let typed = access
            .input_echo()
            .and_then(|echo| echo.pane_recent_input(*live.first().expect("a surviving pane")))
            .unwrap_or_default();
        let authored = loops.authored().expect("the datamodel still answers");
        let turns = loops.turns();
        for pane in &live {
            access.lifecycle().expect("lifecycle").close(*pane);
        }

        // ⚠ THE CONTROLS FIRST. Without these the counts below could both be about a run that never
        // got past the dialog, or one whose echo trail had wrapped.
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Turns),
            "the peer never says the marker, so this run must end on the DOCUMENT's turn budget — \
             anything else and the counts below are about a different run: walked {walk:?}",
        );
        assert_eq!(
            live.len(),
            1,
            "⚠⚠ and exactly ONE pane must survive the run: the replacement, with the session it \
             replaced closed. Two would mean an agent was left running",
        );
        assert_eq!(
            (turns, outcome.screened),
            (Some(TURNS), 2),
            "⚠⚠ every turn spent, and the rule fired TWICE — once per session. The replacement is a \
             FRESH agent that has never been turned down, so this fixture (which asks on its first \
             prompt whatever it is told) asks again; what the adoption buys is that the second time \
             costs no restart. Walked {walk:?}",
        );
        assert!(
            typed.contains("North star: "),
            "⚠ and the echo trail must still hold the FIRST prompt of that session, or these counts \
             are a question about `ECHO_TRAIL_CAP` rather than about the loop: {typed:?}",
        );

        // ── ⚠⚠⚠ THE CLAIM, AS A RELATION BETWEEN THE TWO COUNTS ──
        let said = typed.matches(INSTEAD).count();
        let asked = typed.matches(AIM).count();
        assert!(
            asked > 0 && said >= asked,
            "⚠⚠⚠ ITEM 148: no prompt may ask for the milestone without carrying what overrides it. \
             Before `reflecting` existed these were ONE and SIX. The instruction is asked-for {asked} \
             times and said {said} times. Walked {walk:?}",
        );
        assert!(
            authored.turn.contains(AIM) && authored.turn.contains(INSTEAD),
            "⚠⚠⚠ AND THE SHARPER HALF: what the loop will say NEXT carries both, so the relation \
             above is not a history — it is the loop's standing behaviour: {:?}",
            authored.turn,
        );
        assert!(
            authored.start.contains(INSTEAD),
            "⚠⚠⚠ AND `start_prompt` MOST OF ALL: it is what the next replacement session is greeted \
             with, and that agent remembers nothing of having been redirected: {:?}",
            authored.start,
        );
    }

    /// ⚠⚠⚠ **THE STATE THE DOOR USED TO REFUSE IS ONE A RUN NOW WALKS THROUGH** — register item 6,
    /// and item 148's answer end to end.
    ///
    /// # ⚠⚠⚠ What this gate was, and why it is the same gate
    ///
    /// It measured a REFUSAL's premise: [`AiLoop::new`] turned away `reflect_every < max_turns`
    /// because such a brief reaches `reflecting`, and this drove the loop through [`OuterLoop`] — the
    /// layer under the door — to prove the state was genuinely reachable rather than argued for from
    /// reading the document. The premise held. **Now the state is built, so the same drive is kept
    /// and the assertion moves from *it arrives* to *it gets through*** — the standing rule that a
    /// gate which measured a defect is repurposed, never deleted.
    ///
    /// # ⚠⚠⚠ The five things a session replacement has to get right, all asserted here
    ///
    /// 1. **THE WALK**, as four separate states and not one: `reflecting` decides, `restarting`
    ///    replaces, `resuming` waits for the fresh agent, `priming` recomposes. A driver that held
    ///    the phase itself would replace the pane once per pump.
    /// 2. **A DIFFERENT PANE**, and the old one GONE. A replacement that left the previous session
    ///    running would leave two agents on one milestone, both spending tokens.
    /// 3. **THE SAME COMMAND, DIRECTORY AND SIZE** — read off the pane rather than supplied, so a
    ///    caller cannot be given a `claude` where they launched `claude --resume`, or the daemon's
    ///    directory where the work is.
    /// 4. ⚠⚠⚠ **THE PROMPTS CARRY THE STANDING INSTRUCTION**, which is what the whole restart is
    ///    FOR: the measurement gate above counts one delivery against six re-issues of the milestone
    ///    it overrides, and after a reflection the instruction is in `start_prompt` — the one a FRESH
    ///    agent, which remembers nothing, is greeted with.
    /// 5. **`driving()` NAMES THE NEW PANE.** A cancelled run signals what `driving` answers, so a
    ///    stale answer means the model in the live pane goes on working while the run reports having
    ///    stopped it.
    #[test]
    fn a_reflection_replaces_the_session_and_the_new_one_is_told_what_was_learned() {
        /// The dialog the peer raises, which the rule below quotes.
        const ASKS: &str = "Which way should I build this?";
        /// The standing instruction. ⚠ Distinctive enough to find in a composed prompt.
        const INSTEAD: &str = "not that way; do the smallest verifiable thing and report";

        let (workspace, pane) = crate::testing::standin_agent_refusing(true, u32::MAX, None);
        let access = crate::testing::supervised_asking(&workspace);
        // ⚠ Read BEFORE the replacement: these are what the fresh pane has to match, and the pane
        // that carries them is about to be closed.
        let (argv, cwd, size) = {
            let guard = workspace.lock().expect("the workspace mutex");
            let old = guard.pane(pane).expect("the pane the loop was given");
            (old.argv().to_vec(), old.pty().cwd(), old.pty().dimensions())
        };

        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                screen_rules: Some(
                    crate::screen::ScreenRules::of(vec![
                        crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                            .expect("both halves are non-empty"),
                    ])
                    .expect("a non-empty list"),
                ),
                // ⚠ THE BUDGET IS OFF (equal pair), so the reflection below is caused by the
                // STANDING INSTRUCTION and by nothing else. A gate that left `reflect_every` small
                // could not tell the correctness edge from the budget one.
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        assert_eq!(
            loops.driving(),
            Some(pane),
            "⚠ the control: the loop must be driving the pane it was GIVEN before it replaces it, or \
             the assertion at the end is true for the wrong reason",
        );

        // ⚠⚠ PUMPED THROUGH THE PLUGIN, one step at a time, so the gate can STOP at the far side of
        // the restart. Driven to convergence this fixture would ask again from its fresh session and
        // reflect again for ever, which is honest behaviour and a different claim.
        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let mut replaced = None;
        while replaced.is_none() {
            assert!(
                walked.len() < 40,
                "⚠ this gate's own bound. Walked: {walked:?}",
            );
            let before = loops.state();
            let step = loops
                .step(&access, &run)
                .expect("every step of a replacement must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
            // The far side: the machine is composing again, on a pane that is not the one it started
            // over.
            if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                replaced = Some(loops.driving().expect("a loop mid-run drives its pane"));
            }
        }
        let fresh = replaced.expect("the loop reached `priming` through a replacement");
        let authored = loops.authored().expect("the datamodel still answers");
        let held = {
            let guard = workspace.lock().expect("the workspace mutex");
            (
                guard.pane(pane).is_some(),
                guard
                    .pane(fresh)
                    .map(|new| (new.argv().to_vec(), new.pty().cwd(), new.pty().dimensions())),
            )
        };
        access.lifecycle().expect("lifecycle").close(fresh);

        // ── 1. THE WALK, as five states ──
        //
        // ⚠⚠⚠ `reviewing` JOINED THIS LIST AND DID NOT REPLACE ANYTHING IN IT. The run still
        // reflects, still replaces, still resumes; the review is one transition inserted where the
        // only thing it could ever change — what a session starts knowing — is still ahead of it.
        //
        // ⚠ `ReviewNone` is exact rather than loose, and it is not an accident of this fixture:
        // `ended` is written by the REPLACEMENT, so on the way to a run's first restart there are
        // no closed sessions to read and a review that answered anything else would be reporting on
        // a transcript that does not exist.
        //
        // ⚠⚠ AND THE FIRST OF THEM NAMES ITS CAUSE, exactly, because this fixture arranged one:
        // the budget is off, so `instruction` is the only reason this run can be reflecting for —
        // register item 261, held here as well as in its own gate, since a run that reflected for
        // the BUDGET would satisfy every other assertion in this test.
        for edge in [
            format!(
                "Judging --Judge--> Reflecting — {}",
                crate::outer::ReflectReason::Instruction.noted()
            ),
            "Reflecting --ReflectApplied--> Reviewing".to_owned(),
            "Reviewing --ReviewNone--> Restarting".to_owned(),
            "Restarting --SessionReplaced--> Resuming".to_owned(),
            "Resuming --SessionReady--> Priming".to_owned(),
        ] {
            assert!(
                walked.iter().any(|note| note == &edge),
                "⚠⚠⚠ the replacement must be these FIVE acts and the run's journal must say so — \
                 `{edge}` is missing. Walked {walked:?}",
            );
        }

        // ── 2. A DIFFERENT PANE, and the old one gone ──
        assert_ne!(
            fresh, pane,
            "⚠⚠⚠ a restart must open a NEW session; the same pane back means nothing was replaced",
        );
        assert!(
            !held.0,
            "⚠⚠⚠ and the OLD one must be gone. A replacement that left it running leaves two agents \
             working on one milestone, both spending somebody's tokens",
        );

        // ── 3. THE SAME COMMAND, DIRECTORY AND SIZE ──
        assert_eq!(
            held.1,
            Some((argv, cwd, size)),
            "⚠⚠⚠ the fresh session must be the SAME program in the SAME directory at the SAME size. \
             The pane is the only authority on what it was running, which is why `respawn` takes no \
             argv — a loop that supplied an agent NAME would restart `claude` where somebody had \
             launched `claude --resume`, in the daemon's directory rather than the repository",
        );

        // ── 4. ⚠⚠⚠ THE PROMPTS NOW CARRY WHAT THE RUN LEARNED ──
        assert!(
            authored.start.contains(INSTEAD),
            "⚠⚠⚠ THE WHOLE POINT. `start_prompt` is what the FRESH agent is greeted with, and it \
             remembers nothing of having been redirected — so a loop that carried its standing \
             instructions only in `turn_prompt` would hand its replacement a clean slate and the \
             original milestone: {:?}",
            authored.start,
        );
        assert!(
            authored.turn.contains(INSTEAD),
            "⚠⚠ and every later turn carries it too, which is what makes it STANDING rather than \
             said once: {:?}",
            authored.turn,
        );

        // ── 5. `driving()` NAMES THE LIVE PANE ──
        assert_eq!(
            loops.driving(),
            Some(fresh),
            "⚠⚠⚠ and a cancelled run must interrupt the session that EXISTS. This type held a `pane` \
             field until this round, which was correct for exactly as long as a loop could not \
             replace its own",
        );
    }

    /// ⚠⚠⚠ **A REFLECTION WITH NOTHING TO CARRY DOES NOT PAY FOR A RESTART** — the other exit of
    /// `reflecting`, and the one that stops the feature eating every run that uses it.
    ///
    /// # ⚠⚠⚠ Why this is the sharper half of the pair
    ///
    /// Closing an agent's pane and opening a fresh one throws away that session's whole context. It
    /// is worth it to make a standing instruction stick, and it is worth NOTHING when there is no
    /// instruction to make stick — so a driver that answered `reflect.applied` whenever it was asked
    /// would replace the session on every multiple of `reflect_every`, for ever, having changed
    /// nothing. The document already has the word for that (*"Nothing worth changing: carry on
    /// without paying for a restart"*); this is the gate that says the driver uses it.
    ///
    /// ⚠⚠ **THE PANE IS THE ASSERTION.** A run that reflected and restarted still converges and still
    /// reports the same turn count, so the outcome cannot tell the two apart — only the pane can, and
    /// only by being the SAME one it started with.
    ///
    /// ⚠ `reflect_every: 1` is the sharpest arrangement of the budget: due after the very first turn,
    /// and again after every one. This brief is what the door refused until this round.
    #[test]
    fn a_reflection_with_nothing_to_carry_does_not_pay_for_a_restart() {
        let (workspace, pane) = standin_agent(3);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 1,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a brief that reflects after every turn is one this build drives");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let live = access.pane_ids();
        for pane in &live {
            access.lifecycle().expect("lifecycle").close(*pane);
        }

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the control: a loop reflecting after every turn must still reach its milestone: \
             walked {walk:?}",
        );
        assert!(
            walk.iter()
                .any(|note| note == "Reflecting --ReflectNone--> Working"),
            "⚠⚠⚠ it must really have REFLECTED and found nothing — an assertion about no restart is \
             worth nothing if the loop never reached `reflecting` at all. Walked {walk:?}",
        );
        assert!(
            !walk.iter().any(|note| note.contains("Restarting")),
            "⚠⚠⚠ AND NOTHING WAS REPLACED. A reflection that restarts whenever it is asked throws \
             away an agent's whole context on every multiple of `reflect_every`, having changed \
             nothing. Walked {walk:?}",
        );
        assert_eq!(
            live,
            vec![pane],
            "⚠⚠⚠ THE PANE IS THE ASSERTION: the outcome and the turn count are identical whether or \
             not a session was replaced, so the only witness is that this is the pane the run \
             started with",
        );
    }

    /// ⚠⚠⚠ **THE RUN OUTLIVES ITS AGENT'S CONTEXT: A REFLECTION ASKS THE AGENT WHAT COMES NEXT,
    /// AND THE REPLACEMENT SESSION IS BRIEFED ON THE ANSWER.**
    ///
    /// # ⚠⚠⚠ What this gate measured before the feature existed, and why it was kept
    ///
    /// It was written as a MEASUREMENT of register item 167 and its drive has not changed. What it
    /// asserted then was the defect, in three parts:
    ///
    /// * the loop reflected (`Reflecting --ReflectNone--> Working`) and **replaced nothing**, because
    ///   the only thing a reflection could carry was a standing instruction — so a run whose agent
    ///   was never screened could not get a fresh session at all;
    /// * the milestone label never reached the pane: **the agent was never asked**;
    /// * and every later prompt still named the milestone the CALLER wrote, for as long as the run
    ///   lasted.
    ///
    /// That is what makes a bounded run bounded by one agent's context: whatever the work turned out
    /// to need, the loop could only ever ask again for the checkpoint somebody wrote before it
    /// started. A run meant to carry on across sessions — pay this debt, then the next one — would
    /// re-issue the first one for ever.
    ///
    /// ⚠⚠ **THE PEER IS THE INSTRUMENT AND IT WAS WILLING THE WHOLE TIME.**
    /// [`standin_agent_reflecting`](crate::testing::standin_agent_reflecting) answers the milestone
    /// label with a next milestone and a next reference. Before the reflection turn it was never
    /// spoken to, which is the honest shape of the defect: nothing was broken, something was never
    /// asked.
    ///
    /// ⚠⚠⚠ **AND THE ECHO IS THE TRAP, DELIBERATELY.** The prompt that asks for the label carries the
    /// label, the peer paints every line it is handed, and the pane is 80 columns — so the driver's
    /// reader meets its own instruction on screen before the answer. The assertions name the peer's
    /// words rather than *"something was adopted"*, so a reader that took the first match would
    /// adopt a milestone made of the prompt and fail here.
    #[test]
    fn a_reflection_asks_the_agent_what_comes_next_and_the_replacement_is_told() {
        /// The milestone the CALLER wrote. ⚠ Distinctive, and it must not survive the reflection.
        const ORIGINAL: &str = "the checkpoint whoever started this run wrote down";
        /// What the agent decides instead, once it is asked.
        const PROPOSED: &str = "the debt the agent picked after doing the work";
        /// And what it says the next session should read first.
        const READ_NEXT: &str = "the register entry it had just been reading";

        let (workspace, pane) = crate::testing::standin_agent_reflecting(9, PROPOSED, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                milestone: ORIGINAL.to_string(),
                // ⚠ The peer needs NINE prompts to say the marker and the budget reflects after
                // TWO, so the reflection is reached by the turn budget alone — no screening, no
                // standing instruction, which is exactly the run the defect was about.
                reflect_every: 2,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let mut replaced = None;
        // ⚠ THE FIRST SESSION'S SCREEN, KEPT AS THE RUN GOES. `restarting` CLOSES this pane, so a
        // read taken after the assertion loop would be about a pane that no longer exists — and the
        // claim is that the label reached the session that was ASKED.
        let mut asked = String::new();
        while replaced.is_none() && walked.len() < 60 {
            let before = loops.state();
            let step = loops
                .step(&access, &run)
                .expect("every step of a reflection must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
            let showing = access.pane_full_lines(pane).unwrap_or_default().join("\n");
            if !showing.trim().is_empty() {
                asked = showing;
            }
            if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                replaced = Some(loops.driving().expect("a loop mid-run drives its pane"));
            }
            if matches!(
                loops.state(),
                AiLoopState::Converged
                    | AiLoopState::Exhausted
                    | AiLoopState::Failed
                    | AiLoopState::Cancelled
                    | AiLoopState::Blocked
            ) {
                break;
            }
        }
        let authored = loops.authored().expect("the datamodel still answers");
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        // ── THE CONTROL: it really did reflect, and it really did replace ──
        assert!(
            walked
                .iter()
                .any(|note| note == "Reflecting --ReflectApplied--> Reviewing"),
            "⚠⚠⚠ the reflection must have been ADOPTED — every assertion below is about what a \
             replacement session was told, and is worth nothing if the run never reached one. \
             Walked {walked:?}",
        );
        assert!(
            walked
                .iter()
                .any(|note| note.starts_with("Reviewing --Review") && note.ends_with("Restarting")),
            "⚠⚠⚠ AND THE REVIEW MUST NOT BE ABLE TO STOP THE RUN. `reviewing` sits between the \
             reflection and the replacement, and EVERY ending it has leads to `restarting`: a \
             review is advice about work already finished, so a reviewer that found nothing, could \
             open no record, or broke outright must cost this run one transition and no more. This \
             holds the property `reviewing` has no edge to `failed` for — and it is deliberately \
             loose about WHICH review ending, because which one it was is not this gate's claim. \
             Walked {walked:?}",
        );
        assert!(
            replaced.is_some_and(|fresh| fresh != pane),
            "⚠⚠⚠ and a NEW session must exist to be told: a reflection that decided a milestone \
             and kept the old context has improved nothing a fresh agent will read. Walked \
             {walked:?}",
        );

        // ── 1. THE AGENT WAS ASKED, IN THE DOCUMENT'S OWN WORDS ──
        assert!(
            authored
                .reflect
                .contains(crate::testing::REFLECTION_MILESTONE_LABEL),
            "⚠⚠⚠ the composed `reflect_prompt` must carry the label the peer answers, or this gate \
             is measuring a fixture that agrees with a private word: {:?}",
            authored.reflect,
        );
        assert!(
            authored
                .reflect
                .contains(crate::testing::REFLECTION_ECHO_SLICE),
            "⚠⚠⚠ and it must still carry the words the peer paints back behind the label. That row \
             is this gate's ECHO — the shape a wrapped prompt has on a pane of another width — and \
             it only tests the reader's echo rule for as long as it really is a slice of what the \
             loop said. Prompt: {:?}",
            authored.reflect,
        );
        assert!(
            asked.contains(crate::testing::REFLECTION_MILESTONE_LABEL),
            "⚠⚠⚠ and it must have REACHED THE PANE. This is the half that was missing: a loop that \
             reflects without asking can only ever carry what its author wrote",
        );

        // ── 2. THE ANSWER IS WHAT THE REPLACEMENT IS BRIEFED WITH ──
        assert!(
            authored.start.contains(PROPOSED),
            "⚠⚠⚠ THE WHOLE POINT. `start_prompt` is what the FRESH agent is greeted with, and it \
             must name the milestone THE AGENT chose — not the one the caller wrote before any of \
             the work had been done: {:?}",
            authored.start,
        );
        assert!(
            authored.start.contains(READ_NEXT),
            "⚠⚠ and what to read first, which is the other half of what a fresh context needs: {:?}",
            authored.start,
        );
        assert!(
            !authored.start.contains(ORIGINAL),
            "⚠⚠⚠ and the caller's original milestone must be GONE. A run that carried both would \
             hand a fresh agent two checkpoints and no way to tell which is current: {:?}",
            authored.start,
        );
        assert!(
            authored.turn.contains(PROPOSED),
            "⚠⚠ every later turn of the replacement session works toward the same new milestone: \
             {:?}",
            authored.turn,
        );

        // ── 3. AND IT IS THE AGENT'S ANSWER, NOT THE TWO THINGS THAT LOOK LIKE ONE ──
        assert!(
            !authored
                .start
                .contains(crate::testing::REFLECTION_ECHO_SLICE),
            "⚠⚠⚠ the reader must DISCOUNT WHAT THIS LOOP ITSELF SAID. The peer paints a row that \
             opens with the label and carries a verbatim slice of the prompt — which is exactly \
             what a wrapped echo looks like — and adopting it would make the run's own goal out of \
             the instruction it had just sent: {:?}",
            authored.start,
        );
        assert!(
            !authored
                .start
                .contains(crate::testing::REFLECTION_PROVISIONAL),
            "⚠⚠ and where the agent named more than one, the LAST is its answer: an agent asked \
             for two lines writes a paragraph first, and the row it settled on is the one below: \
             {:?}",
            authored.start,
        );
    }

    /// ⚠⚠⚠ **A GUARD READS ECMASCRIPT TRUTH, NOT LUA TRUTH** — the fact this workspace's own
    /// document asserted the opposite of, in writing, with no gate.
    ///
    /// # ⚠⚠⚠ The claim that was wrong, and what it was holding up
    ///
    /// `ai_loop.scxml` said, as the stated reason for publishing a boolean beside a rule's name:
    /// *"it is Lua, where the only false values are `nil` and `false`; an empty string is TRUE, so
    /// a guard reading the rule's name directly would fire on every blocked turn of a run that
    /// matched nothing."*
    ///
    /// That is Lua's rule and not this engine's. The generator wraps every `cond` it emits as
    /// `_scxml_truthy(...)`, and the runtime defines that in one line — `nil/false -> false,
    /// 0/0.0/"" -> false, else true`. **So the failure that paragraph predicted cannot happen**, and
    /// a design was defended by a fact about the engine that nothing had ever asked the engine.
    ///
    /// ⚠⚠ **HOW IT WAS FOUND IS THE POINT.** Nobody re-read that paragraph. A NEW document needed a
    /// guard over a count, a gate asserted what `0` does, and the answer disagreed with what the old
    /// document said about `""`. **Writing a second thing is what audits the first.**
    ///
    /// ⚠ The design it was defending stands on its own: a name and a verdict are two facts, and
    /// *nothing matched* is not *nobody judged*. What is gone is the false reason.
    #[test]
    fn a_guard_reads_ecmascript_truth_and_not_lua_truth() {
        use crate::sm::context_review_sm::{
            ContextReviewEvent, ContextReviewPolicy, ContextReviewState,
        };

        /// Raise `read.done` carrying `records` and say whether the guard took the transition.
        fn guard_fired(records: &str) -> bool {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let mut engine = Engine::new(ContextReviewPolicy::new(lua));
            engine.initialize();
            engine.raise_external(
                ContextReviewEvent::ReadDone,
                &format!(r#"{{"records": {records}}}"#),
                "",
            );
            engine.step();
            engine.get_current_state() != ContextReviewState::Reading
        }

        assert!(
            !guard_fired("0"),
            "⚠⚠⚠ ZERO IS FALSE HERE. In Lua it is TRUE — so a document written against Lua's rules \
             would have a guard fire on a count of nothing, which is the exact shape of a review \
             that found no records deciding it had found some",
        );
        assert!(
            !guard_fired(r#""""#),
            "⚠⚠⚠ AND SO IS THE EMPTY STRING — the case `ai_loop.scxml` asserted was TRUE, in \
             writing, as the reason for a design",
        );
        assert!(
            !guard_fired("null"),
            "⚠ and an absent value, which is the only one every rule set agrees about",
        );

        // ── THE CONTROLS: a guard that never fires is not a guard ──
        assert!(
            guard_fired("3"),
            "⚠⚠ the control. A gate where nothing fires would pass for an engine that had stopped \
             reading `_event.data` at all, which is the failure this whole family is about",
        );
        assert!(
            guard_fired(r#""some""#),
            "⚠ and a non-empty string is true, which is what makes the empty one's falsity a \
             statement about the VALUE rather than about strings",
        );
    }

    /// ⚠⚠⚠ **THE CONTEXT REVIEW WALKS, AND EVERY WAY OUT OF IT IS REACHABLE** — the child machine
    /// `ai_loop.scxml` will invoke before it replaces a session.
    ///
    /// # ⚠⚠⚠ What this gate is for, and what it deliberately is not
    ///
    /// `context_review.scxml` is a DOCUMENT and this asserts the document: that its states are
    /// wired the way its prose says, that each of its three early exits can actually be taken, and
    /// that a run which finds a habit ends carrying something while one that finds none ends
    /// carrying nothing. **It says nothing about whether the analysis is any good** — that is the
    /// `Warmup` axis's job, on real sessions, and no unit test can stand in for it.
    ///
    /// ⚠⚠ **THE EXITS ARE THE POINT.** Three of the five states can end the review early — no
    /// records, no habit, no usable answer — and they are separate states rather than one because a
    /// reader of a finished review has to be able to tell *there was nothing to look at* from
    /// *there was nothing worth carrying*. A machine whose failure paths all collapse into one is a
    /// machine that cannot be diagnosed, which is this crate's most expensive shape.
    ///
    /// ⚠ Driven event by event rather than through a driver, because the driver's half does not
    /// exist yet: what is being settled here is that the DOCUMENT is drivable at all before
    /// anything is written against it.
    #[test]
    fn the_context_review_walks_and_each_of_its_endings_is_reachable() {
        use crate::sm::context_review_sm::{
            ContextReviewEvent, ContextReviewPolicy, ContextReviewState,
        };

        /// Walk a fresh review through `events`, each carrying `data`, and say where it landed.
        fn walked(events: &[(ContextReviewEvent, &str)]) -> ContextReviewState {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let mut engine = Engine::new(ContextReviewPolicy::new(lua));
            engine.initialize();
            for (event, data) in events {
                engine.raise_external(*event, data, "");
                engine.step();
            }
            engine.get_current_state()
        }

        // ── THE WHOLE WALK: records found, a habit counted, an answer given, a file written ──
        assert_eq!(
            walked(&[
                (ContextReviewEvent::ReadDone, r#"{"records": 3}"#),
                (ContextReviewEvent::CountDone, r#"{"habits": 2}"#),
                (
                    ContextReviewEvent::AskDone,
                    r#"{"carry": "wait for the build, do not poll it"}"#
                ),
                (ContextReviewEvent::WriteDone, ""),
            ]),
            ContextReviewState::Carried,
            "⚠⚠⚠ a review that found records, counted a habit, got an answer and kept it must end \
             CARRYING — this is the only path that hands the next session anything",
        );

        // ── AND THE THREE EARLY EXITS, each its own reason ──
        assert_eq!(
            walked(&[(ContextReviewEvent::ReadNone, "")]),
            ContextReviewState::Nothing,
            "⚠⚠ NO RECORDS AT ALL is the ordinary case, not an error: an agent whose transcript is \
             off writes none, which is exactly what a nested-agent marker causes",
        );
        assert_eq!(
            walked(&[
                (ContextReviewEvent::ReadDone, r#"{"records": 3}"#),
                (ContextReviewEvent::CountNone, ""),
            ]),
            ContextReviewState::Nothing,
            "⚠⚠ NOTHING REPEATED ENOUGH TO NAME — the answer a healthy run should give most of the \
             time, and it must not look like a failure",
        );
        assert_eq!(
            walked(&[
                (ContextReviewEvent::ReadDone, r#"{"records": 3}"#),
                (ContextReviewEvent::CountDone, r#"{"habits": 2}"#),
                (ContextReviewEvent::AskNone, ""),
            ]),
            ContextReviewState::Nothing,
            "⚠⚠⚠ AND AN UNUSABLE ANSWER CARRIES NOTHING. The failure of this mechanism must be its \
             own absence — never a line invented to fill the slot, which is the rule `judged` \
             already follows when it answers false on silence",
        );

        // ── THE GUARD IS REAL: records of zero is not records ──
        assert_eq!(
            walked(&[(ContextReviewEvent::ReadDone, r#"{"records": 0}"#)]),
            ContextReviewState::Reading,
            "⚠⚠⚠ `cond=\"_event.data.records\"` must actually read the number. ⚠ THIS DATAMODEL IS \
             LUA, where the only false values are nil and false — so a ZERO that took the guard \
             would be the same class of bug the loop's own `judged` boolean exists to avoid, and \
             this is the gate that says which way it goes here",
        );
    }

    /// ⚠⚠⚠ **CAN A DOCUMENT IN THIS TREE OWN A CHILD MACHINE?** — asked of the engine, before
    /// anything is designed on the answer.
    ///
    /// # ⚠⚠⚠ Why this is a gate and not a paragraph
    ///
    /// The analysis a loop needs before it can improve its own context is a PROCESS — open the
    /// closed sessions' records, count what recurs, ask a narrow question, write one file — and a
    /// process with steps is a machine rather than a function. W3C's answer for *a machine that
    /// runs another machine* is `<invoke>`, and the whole design rests on whether this tree has it.
    ///
    /// **It was nearly designed on a guess, twice, in opposite directions.** First reading said no,
    /// from a comment in the pinned engine's W3C manifest (*"the seed has no `<invoke>` tests"*) —
    /// which is a fact about a TEST SUITE and not about the generator. The owner said it was
    /// supported; the generator's own filters said so too. **Neither is this crate compiling and
    /// running one**, which is the only thing that settles it — the same rule `ai_loop.scxml`
    /// already carries about `===` and `JSON.stringify`: *the engine will answer; the name at the
    /// top of the document will not.*
    ///
    /// # ⚠⚠ The three separate answers, and why each one matters
    ///
    /// 1. **IT COMPILES.** The generator emits a typed child (`Option<Box<Engine<ChildPolicy>>>`),
    ///    a pending-invoke pass that starts it, a read of the child's `<donedata>`, and a cancel on
    ///    the way out of the invoking state — the parent owning the child's lifecycle, which is the
    ///    property that makes a sub-machine worth having at all.
    /// 2. **THE CHILD RUNS**, without anybody driving it: the parent reaches `heard`, which is only
    ///    reachable on `done.invoke.probe`.
    /// 3. ⚠⚠⚠ **AND ITS ANSWER CROSSES.** `<donedata>` arriving as `_event.data` is the whole
    ///    reason to prefer a child machine over a function call, and it is the half most likely to
    ///    be missing quietly — a child that ran and told the parent nothing would look identical
    ///    from the outside until somebody tried to use the answer.
    #[test]
    fn a_document_here_can_invoke_a_child_machine_and_hear_what_it_answered() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(crate::sm::probe_parent_sm::ProbeParentPolicy::new(lua));
        engine.initialize();
        // ⚠ The invoke is DEFERRED (W3C SCXML 6.4: a static invoke starts after the macrostep that
        // entered its state), so the parent is stepped rather than read straight after `initialize`.
        for _ in 0..8 {
            engine.step();
        }

        assert_eq!(
            engine.get_current_state(),
            crate::sm::probe_parent_sm::ProbeParentState::Heard,
            "⚠⚠⚠ the parent must have HEARD its child finish. `heard` is reachable only on \
             `done.invoke.probe`, so anything else means the child never ran, never finished, or \
             finished without telling the parent — and a sub-machine nobody hears from is a \
             function call with extra steps",
        );

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let carried = engine
            .policy()
            .script_engine
            .get_variable(&session, "carried");
        assert!(
            matches!(&carried, Ok(ScriptValue::String(said)) if said == "the child ran"),
            "⚠⚠⚠ AND THE CHILD'S OWN ANSWER MUST CROSS. `<donedata>` reaching the parent as \
             `_event.data` is the whole reason to prefer a child MACHINE over a function: a child \
             that ran and answered nothing looks identical from outside until somebody needs what \
             it worked out. Got {carried:?}",
        );
    }

    /// ⚠⚠⚠ **A LOOK THAT FOUND NOTHING IS NOT A TRANSITION, AND THE JOURNAL MAY NOT SAY IT WAS.**
    ///
    /// # ⚠⚠⚠ The document is the single source of truth, and the journal was contradicting it
    ///
    /// `null` is not an event of `ai_loop.scxml` — there is no `<transition event="null">` in it.
    /// It is the sentinel [`OuterLoop`](crate::outer::OuterLoop)'s `watch` answers when a pass over
    /// the pane found nothing, and `advance` returns on it **before touching the machine**. So the
    /// machine did not move, no transition fired, and there is nothing of the DOCUMENT's to report.
    ///
    /// The journal reported it anyway, as `Working --Null--> Working`, which is the exact shape it
    /// uses for a real transition. **Measured cost: this round's supervisor read a run's thirteen
    /// journal entries as thirteen steps and told the owner so.** Nine were transitions; four were
    /// looks. The owner asked whether that state was real or invented, and the honest answer was
    /// that the product had invented it.
    ///
    /// ⚠⚠ **THE ASSERTION IS ON THE SHAPE, NOT ON THE WORDING.** What must never happen again is a
    /// non-event wearing a transition's arrow — so the gate demands the arrow's ABSENCE, and leaves
    /// whoever rewrites the sentence free to say it better.
    #[test]
    fn a_look_that_found_nothing_is_not_written_down_as_a_step() {
        let looked = AiLoop::walked(
            AiLoopState::Working,
            AiLoopEvent::Null,
            AiLoopState::Working,
            None,
            None,
        );
        assert!(
            !looked.contains("-->"),
            "⚠⚠⚠ THE MACHINE DID NOT MOVE, so the journal must not draw the arrow it draws for a \
             transition. `null` is not an event this document has; the driver returns on it before \
             the machine is touched. A reader — human or the run's own supervisor — counts arrows: \
             {looked:?}",
        );
        assert!(
            !looked.contains("Null"),
            "⚠⚠ and it must not name the sentinel as though it were one of the document's events, \
             which is what sent a reader looking for it in the scxml: {looked:?}",
        );
        assert!(
            looked.contains("Working"),
            "⚠ it must still say WHERE the loop was, or a run that is stuck somewhere is \
             indistinguishable from one that is stuck somewhere else: {looked:?}",
        );

        // ── THE CONTROL: a real transition still draws the arrow ──
        let moved = AiLoop::walked(
            AiLoopState::Judging,
            AiLoopEvent::Judge,
            AiLoopState::Reflecting,
            None,
            None,
        );
        assert_eq!(
            moved, "Judging --Judge--> Reflecting",
            "⚠⚠⚠ and the fix must not have been *stop drawing arrows*. A transition the document \
             really took is the thing this journal exists to record",
        );
        // ⚠ AND THE ARROW IS STILL THE WHOLE LINE WHEN NOTHING ELSE IS KNOWN. A driver that could
        // not read `reflect_reason` reports the edge it took and no cause, rather than an empty
        // clause after a dash that a reader has to decide the meaning of.
    }

    /// ⚠⚠⚠ **A REACHED MILESTONE ASKS WHAT IS NEXT — IT DOES NOT END THE RUN** — and the run ends
    /// only when the agent says the NORTH STAR is reached.
    ///
    /// # ⚠⚠⚠ The live run that made this necessary
    ///
    /// The owner started a real debt-repayment loop against a real `claude`: *"keep going until
    /// every debt is repaid"*. The agent paid ONE item, wrote a live-gated feature, committed it,
    /// said `MILESTONE REACHED` — and the run **converged after a single working turn**, because
    /// `judging`'s first guard sent a reached milestone straight to `closing`. Nothing was broken.
    /// **The document simply had no edge from *this step is done* to *what is the next one*, so a
    /// run could only ever be as long as its first checkpoint.**
    ///
    /// # ⚠⚠ The three things this asserts, and why the third is the sharp one
    ///
    /// 1. **A REACHED MILESTONE REFLECTS.** `Judging --Judge--> Reflecting`, not `Closing`.
    /// 2. **AND THE RUN CARRIES ON INTO THE NEXT ONE** — the agent names it, the session is
    ///    replaced, and the fresh one is briefed with what the agent chose.
    /// 3. ⚠⚠⚠ **AND A RUN WHOSE AGENT HAS NOTHING FURTHER STILL ENDS.** This is the half that is
    ///    easy to lose: a reflection asked because the milestone was reached, which names no
    ///    successor, must not go back to `working` — that asks an agent to reach a checkpoint it has
    ///    just reached, for ever. **The livelock is real and this gate is what stands between the
    ///    feature and it.**
    #[test]
    fn a_reached_milestone_asks_what_is_next() {
        /// What the agent decides to do after the first checkpoint falls.
        const NEXT: &str = "the second debt, chosen after the first was paid";
        /// And where its replacement should start reading.
        const READ_NEXT: &str = "the register entry for it";

        // ⚠ ONE working prompt and it says the marker — so the FIRST judgement is `done`, which is
        // the exact arrangement that used to converge a run before it had done anything else.
        let (workspace, pane) = crate::testing::standin_agent_reflecting(1, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠ THE BUDGET IS OFF (equal pair), so the reflection below is caused by the MILESTONE
            // being reached and by nothing else — a gate that left `reflect_every` small could not
            // tell this edge from the budget one.
            &brief_for(40),
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let mut replaced = None;
        while replaced.is_none() && walked.len() < 60 {
            let before = loops.state();
            let step = loops
                .step(&access, &run)
                .expect("every step of a reached milestone must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
            if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                replaced = Some(loops.driving().expect("a loop mid-run drives its pane"));
            }
            if matches!(
                loops.state(),
                AiLoopState::Converged
                    | AiLoopState::Exhausted
                    | AiLoopState::Failed
                    | AiLoopState::Cancelled
                    | AiLoopState::Blocked
            ) {
                break;
            }
        }
        let authored = loops.authored().expect("the datamodel still answers");
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        assert!(
            !walked.iter().any(|note| note.contains("--> Closing")),
            "⚠⚠⚠ A REACHED MILESTONE MUST NOT END THE RUN. This is the whole change: a loop asked to \
             keep going until everything is done paid ONE item, said the marker, and converged — \
             the document had no edge from *this step is done* to *what is next*. Walked {walked:?}",
        );
        // ⚠⚠ THE CAUSE IS ASSERTED WITH THE EDGE, and this fixture is the only one that can: the
        // budget is off and nothing is screened, so `milestone` is the only reason a reflection
        // here can have — register item 261, and the arm of it whose word the driver's own
        // livelock guard also reads.
        let reflected = format!(
            "Judging --Judge--> Reflecting — {}",
            crate::outer::ReflectReason::Milestone.noted()
        );
        assert!(
            walked.iter().any(|note| note == &reflected),
            "⚠⚠⚠ and it must go to the state that DECIDES what comes next, which is the one already \
             built for it — SAYING that a reached milestone is what sent it there. Walked {walked:?}",
        );
        assert!(
            replaced.is_some_and(|fresh| fresh != pane),
            "⚠⚠ and the run must carry on into the next milestone with a fresh session, or *keep \
             going* means nothing. Walked {walked:?}",
        );
        assert!(
            authored.start.contains(NEXT),
            "⚠⚠⚠ and the replacement is briefed with the milestone THE AGENT named after finishing \
             the first — which is what makes a run longer than one checkpoint: {:?}",
            authored.start,
        );
    }

    /// ⚠⚠⚠ **AND A RUN WHOSE AGENT HAS NOTHING FURTHER STILL ENDS** — the other half of
    /// [`a_reached_milestone_asks_what_is_next`], and the one that stands between this feature and a
    /// livelock.
    ///
    /// A reflection asked because the milestone was reached, whose agent names no successor, cannot
    /// go back to `working`: the milestone it would be sent to work on is the one just reported
    /// reached, so the agent says the marker again, and the loop turns over for ever having done
    /// nothing. **Measured as the shape it would take**: every gate here that drives
    /// [`standin_agent`](crate::testing::standin_agent) — a peer that says the marker and has no
    /// opinion about what is next — would have run to its budget instead of converging.
    ///
    /// ⚠ The peer is the ORDINARY one, deliberately. What this asserts is that the commonest agent
    /// in this crate still reaches `converged`, so the feature above cannot have been bought by
    /// making every simple run hang.
    #[test]
    fn a_reflection_with_no_successor_after_a_reached_milestone_ends_the_run() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠⚠ THE LIVELOCK. A reflection that follows a REACHED milestone and finds no successor \
             must end the run — sent back to `working` it asks an agent to reach a checkpoint it \
             has just reached, and the run turns over until its budget. Walked {walk:?}",
        );
        // ⚠⚠⚠ REPURPOSED RATHER THAN DELETED (register item 267). This asserted the bare arrow by
        // EQUALITY, which is what made it the canary for the defect: the moment the ending's own
        // word was added it went red, printing the very walk that proves the clause is there. What
        // it holds now is the same claim plus the one it could not make — that a run which ended
        // for THIS reason says so. `the_walk_says_which_ending_closed_the_run` holds the other
        // ending against it; here the point is that the livelock's own exit is the one named.
        let ended = format!(
            "Reflecting --ReflectDone--> Closing — {}",
            crate::outer::DoneReason::NoSuccessor.noted()
        );
        assert!(
            walk.iter().any(|note| note == &ended),
            "⚠⚠ and it must end THROUGH the reflection, which is now the only route to a closing \
             report: a run's account is written once, about the whole run — SAYING that nobody \
             declared the north star met, only that this agent had no next checkpoint. Walked \
             {walk:?}",
        );
    }

    /// ⚠⚠⚠ **THE WALK SAYS WHICH OF THE TWO ENDINGS CLOSED THE RUN** — register item 267, which is
    /// 261's and 265's class a third time and the one their method would not have found.
    ///
    /// # ⚠⚠⚠ What was measured, and why counting the document's edges misses it
    ///
    /// Both of its elders are *several `<transition>`s, one arrow*, and both were found by reading
    /// `ai_loop.scxml` and counting doors. `closing` has ONE door. What arrives through it is two
    /// different runs, because [`OuterLoop::reflect`](crate::outer::OuterLoop) raises `reflect.done`
    /// from two `return`s:
    ///
    /// | what happened | what a reader should do |
    /// |---|---|
    /// | the agent said `north_star_marker` | weigh its closing account against what was asked |
    /// | a reached milestone whose reflection named no successor | look at that milestone and decide — **nobody said the north star was met** |
    ///
    /// Both publish `Verdict::Converged`, and both wrote `Reflecting --ReflectDone--> Closing` and
    /// nothing else. ⚠⚠ The second is the sharper loss: it is a run that quietly STOPPED. The
    /// livelock guard ends it because there is nothing left to ask THIS agent for, which is a fact
    /// about one session's imagination rather than about the job.
    ///
    /// # ⚠⚠⚠ The control, and why this gate needs one its elders did not
    ///
    /// `reflect_prompt`'s last line ENDS with the marker it asks for, so a pane that wrapped that
    /// line exactly at the marker would converge a run on its own instruction and arm 1 below would
    /// go green having measured the ECHO. ⚠ When this gate was written `said_marker` had no echo
    /// discount at all — unlike [`OuterLoop::proposed`](crate::outer::OuterLoop) — and the hazard
    /// was registered as item 270. **It is paid**: the marker is read off a logical LINE and a line
    /// that is the tail of the question, broken, is discounted.
    ///
    /// **The control is the same prompt with the answer taken away**: [`standin_agent_reflecting`]
    /// is asked the identical reflection, paints the identical echo, and answers with a successor
    /// instead of the marker. Its run must never reach `closing` at all. ⚠ It proves this pane's
    /// width is safe and not that every width is — which is
    /// [`a_reflection_on_a_pane_that_breaks_the_north_star_line_does_not_close_the_run`]'s claim,
    /// on a pane 67 columns wide because 134 is 2×67.
    ///
    /// ⚠⚠ AND THE TWO ARMS MUST COVER THE WHOLE VOCABULARY, asserted rather than assumed — a
    /// `DoneReason` arm no run here reaches is a sentence nobody has ever read.
    #[test]
    fn the_walk_says_which_ending_closed_the_run() {
        use crate::outer::DoneReason;

        /// The one edge this gate is about — one arrow, two runs.
        const THE_EDGE: &str = "Reflecting --ReflectDone--> Closing";
        /// What the control peer proposes when it is asked, so that it does NOT end the run.
        const NEXT: &str = "the debt this run picked after the last one";
        /// And where it says the replacement should start reading.
        const READ_NEXT: &str = "the register entry for it";

        /// Drive a loop to its ending and hand back what it wrote down, with its outcome.
        fn run_of<A: PaneAccess>(loops: &mut AiLoop, access: &A) -> (OutcomeState, Vec<String>) {
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 60,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(loops, access, &RunContext::uncancellable());
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
            (outcome.state, walk)
        }

        // ── ARM 1: THE AGENT SAID THE NORTH STAR WAS REACHED ──
        // ⚠ The first peer in this crate ever to say it. Two working turns, then the reflection it
        // is sent asks whether the whole thing is finished, and it says that it is.
        let (workspace, pane) = crate::testing::standin_agent_finishing(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (declared_state, declared_walk) = run_of(&mut loops, &access);

        // ── ARM 2: A REACHED MILESTONE WHOSE REFLECTION NAMED NO SUCCESSOR ──
        // ⚠ The ORDINARY peer — it says the milestone marker and has no opinion about what is next,
        // which is precisely the run the livelock guard ends.
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (no_successor_state, no_successor_walk) = run_of(&mut loops, &access);

        // ── THE CONTROL: THE SAME PROMPT, THE SAME ECHO, NO MARKER ──
        let (workspace, pane) = crate::testing::standin_agent_reflecting(2, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (_, echoed_walk) = run_of(&mut loops, &access);
        assert!(
            !echoed_walk.iter().any(|note| note.starts_with(THE_EDGE)),
            "⚠⚠⚠ THE CONTROL: this peer is asked the SAME reflection and paints the SAME echo of \
             it, and answers with a successor rather than the marker — so it must never reach \
             `closing`. If it does, `reflect_prompt`'s own last line has been read as the agent \
             saying the north star was reached — that line ENDS with the marker, which is why \
             `said_marker` discounts a line that is the tail of the question broken — and arm 1 \
             below is measuring this loop's own instruction. Walked {echoed_walk:?}",
        );

        // ── ARM 3: A PERSON ASKED IT TO STAND DOWN, AND IT FINISHED THE MILESTONE FIRST ──
        //
        // ⚠⚠⚠ THE SAME PEER AS ARM 2, and that is the whole design of this arm. It says the
        // milestone marker and has no opinion about what comes next — left alone it ends
        // `no_successor`, which arm 2 just measured. The ONLY difference here is that somebody spoke
        // to the run. If the two came out with the same word, the order would be doing nothing and
        // the orders region would be decoration.
        //
        // ⚠⚠ THE ORDER IS GIVEN BEFORE THE FIRST PUMP, which is the honest hard case: it stands
        // through every working turn and has to still be standing when `judging` finally asks. An
        // order given just before the milestone would prove far less — it would not distinguish *the
        // region held it* from *the event happened to arrive at the right moment*.
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        loops.stand_down();
        let (stood_down_state, stood_down_walk) = run_of(&mut loops, &access);

        // ⚠⚠⚠ EACH ARM CARRIES THE ARROW IT ARRIVES BY, because they are not all the same one any
        // more. The two reflection endings pass through `reflecting` — the run asked what was next
        // and the answer ended it. A STAND-DOWN does not: the order is already standing when the
        // milestone lands, so `judging` closes directly and **the run never spends a reflection turn
        // it was told not to need.** A gate that kept one arrow for all three would be asserting that
        // a stood-down run reflects first, which is a model call nobody asked for.
        let arms = [
            (
                "the agent declared it",
                DoneReason::Declared,
                &declared_walk,
                "Reflecting --ReflectDone--> Closing",
            ),
            (
                "no successor was named",
                DoneReason::NoSuccessor,
                &no_successor_walk,
                "Reflecting --ReflectDone--> Closing",
            ),
            (
                "a person asked it to stand down",
                DoneReason::StoodDown,
                &stood_down_walk,
                "Judging --Judge--> Closing",
            ),
        ];

        // ── BOTH REALLY CONVERGED, which is what makes the ambiguity worth closing ──
        for (label, state, walk) in [
            ("the agent declared it", declared_state, &declared_walk),
            (
                "no successor was named",
                no_successor_state,
                &no_successor_walk,
            ),
            // ⚠⚠⚠ A STAND-DOWN CONVERGES TOO, and that is the claim, not an accident of the loop.
            // The run banked the milestone and took its account — so the word a reader gets is
            // `Converged`, exactly as for the other two, and the walk is what says a PERSON ended it
            // rather than the work running out. A stand-down that reported `Cancelled` would tell a
            // reader the turn was thrown away when it was finished.
            (
                "a person asked it to stand down",
                stood_down_state,
                &stood_down_walk,
            ),
        ] {
            assert_eq!(
                state,
                OutcomeState::Converged,
                "⚠⚠⚠ the control for {label}: BOTH endings publish `Verdict::Converged` — that is \
                 exactly why the walk had to be the thing that tells them apart. An arm that ended \
                 any other way is not the run this gate is describing. Walked {walk:?}",
            );
        }

        // ── THE CONTROL ON THE VOCABULARY: these two are all of it ──
        let covered: std::collections::BTreeSet<DoneReason> =
            arms.iter().map(|(_, ending, _, _)| *ending).collect();
        assert_eq!(
            covered,
            DoneReason::ALL.into_iter().collect(),
            "⚠⚠⚠ the control: this gate must arrange EVERY way a run can be closed. An arm no run \
             here reaches is a word nothing renders and a sentence nobody has read — `Pumped::\
             Unbuilt`'s finding (register item 260) arriving before the fact",
        );

        // ── AND THE WALK SAYS WHICH ──
        let mut lines: Vec<&str> = Vec::new();
        for (label, ending, walk, the_edge) in arms {
            let mut found = walk.iter().filter(|note| note.starts_with(the_edge));
            let line = found.next().unwrap_or_else(|| {
                panic!(
                    "⚠ the control for {label}: this run must have taken {the_edge:?}, or what \
                     follows is about an edge it never took. Walked {walk:?}"
                )
            });
            assert!(
                found.next().is_none(),
                "⚠ the control for {label}: a run closes ONCE, and a second such line means the \
                 line read below is not the one whose cause is being asserted. Walked {walk:?}",
            );
            assert_eq!(
                line,
                &format!("{the_edge} — {}", ending.noted()),
                "⚠⚠⚠ REGISTER ITEM 267: this run closed because {label}, and the one line its walk \
                 wrote about ending must say so. Two runs with opposite remedies — *weigh the \
                 account it wrote* against *nobody said the job was done, look at the milestone* — \
                 were one arrow and nothing else, and both reported `converged`. Walked {walk:?}",
            );
            // ⚠⚠⚠ AND EXACTLY ONE LINE CARRIES IT — `done_reason` is a datamodel variable, so a
            // reader that took it on every pass instead of on the ENTERING edge would write *the
            // agent declared it* onto every step of the closing turn that followed. ⚠ The colon is
            // what makes this the ending's own heading rather than a mention inside some other
            // sentence.
            let heading = format!("{}: ", ending.word());
            assert_eq!(
                walk.iter().filter(|note| note.contains(&heading)).count(),
                1,
                "⚠⚠⚠ {label}: {heading:?} must head exactly ONE line of this walk — the step that \
                 arrived at `closing`. More than one is a level being written down as a series of \
                 findings. Walked {walk:?}",
            );
            lines.push(line);
        }
        let distinct: std::collections::BTreeSet<&str> = lines.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            lines.len(),
            "⚠⚠⚠ AND THE TWO LINES MUST DIFFER FROM ONE ANOTHER. Naming the ending is worth nothing \
             if two causes still render one string — which is exactly what {THE_EDGE:?} did for \
             both of them before this gate existed: {lines:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE WIDTH THE GATE ABOVE LEFT AS A RESIDUE IS MEASURED HERE** — register item 270,
    /// the half of it that only a whole run can say.
    ///
    /// # ⚠⚠⚠ What the control above proves, and what it does not
    ///
    /// `the_walk_says_which_ending_closed_the_run` carries a peer that is asked the same reflection,
    /// paints the same echo and never says the marker — and its own note says what that leaves
    /// open: *"a control proves this pane's width is safe and not that every width is."* **This is
    /// every other width**, or the one that matters: `reflect_prompt`'s last line is 152 characters
    /// with `north_star_marker` starting at 134, so a pane **67 columns** wide breaks it exactly
    /// there and puts the marker alone on a row of a screen where the agent has said nothing.
    ///
    /// ⚠⚠⚠ **THE ARITHMETIC IS ASSERTED OFF THE PRODUCT'S OWN COMPOSED TEXT**, not written into a
    /// comment. If somebody rewords that clause the width this gate chose stops being hostile, and
    /// what says so is the first assertion below rather than a silent green.
    ///
    /// ⚠ What it is NOT: a claim about the composer's break, which no width can undo — that is
    /// `a_composer_that_re_wraps_the_question_onto_the_marker_is_not_an_agent_saying_it`, one crate
    /// module over and about the same predicate.
    #[test]
    fn a_reflection_on_a_pane_that_breaks_the_north_star_line_does_not_close_the_run() {
        /// 134 is 2×67 and 152−134 is 18, so the echo of that sentence ends on a row that is the
        /// marker and nothing else.
        const FATAL: u16 = 67;
        /// What the peer proposes instead of declaring the job finished.
        const NEXT: &str = "the debt after this one";
        /// And where it says the replacement should start.
        const READ_NEXT: &str = "the register entry for it";
        /// The one edge a run must not take on a screen it wrote itself.
        const THE_EDGE: &str = "Reflecting --ReflectDone--> Closing";

        let (workspace, pane) =
            crate::testing::standin_agent_reflecting_at(FATAL, 2, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();

        // ── THE FIXTURE'S OWN CLAIM, CHECKED AGAINST THE PRODUCT ──
        let reflect = loops
            .authored()
            .expect("a primed machine holds its composed prompts")
            .reflect;
        let last = reflect
            .lines()
            .last()
            .expect("the reflection prompt has lines")
            .to_owned();
        assert!(
            last.is_ascii(),
            "⚠ the arithmetic below counts CHARACTERS as columns, which is only true while this \
             clause is ASCII: {last:?}",
        );
        let at = last
            .find(crate::testing::NORTH_STAR_SAID)
            .expect("the document asks for the marker this fixture answers with");
        assert!(
            at.is_multiple_of(usize::from(FATAL)) && last.len() - at <= usize::from(FATAL),
            "⚠⚠⚠ THE FIXTURE MUST STAGE THE HAZARD OR THIS GATE MEASURES NOTHING: at {FATAL} \
             columns the marker has to start a row and finish on it, which needs its offset \
             ({at}) to be a multiple of the width and the remainder ({}) to fit. Somebody has \
             reworded the clause — pick a width that is hostile to the new one rather than \
             deleting this: {last:?}",
            last.len() - at,
        );

        // ── AND THE RUN MUST NOT HAVE CLOSED ON IT ──
        assert!(
            !walk.iter().any(|note| note.starts_with(THE_EDGE)),
            "⚠⚠⚠ REGISTER ITEM 270: this peer answered the reflection with a SUCCESSOR and never \
             said the north star was reached — so the only thing on that pane carrying those three \
             words is the loop's own question, broken across rows by a terminal that wraps where it \
             likes. A run that closes here reports the whole job finished on the strength of having \
             asked whether it was. Walked {walk:?}",
        );
        assert!(
            walk.iter().any(|note| note.contains("Reflecting")),
            "⚠ the control: this run has to have REFLECTED at all, or the assertion above is about \
             an edge nothing came near. Walked {walk:?}",
        );
        // ⚠⚠ AND THE OUTCOME SAYS IT TOO, which is the half a caller reads. `converged` is the one
        // word this run may not end on: both doors of `closing` mean *the work is finished*, and
        // this peer has said the opposite every time it was asked.
        assert_ne!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠⚠ REGISTER ITEM 270, as the caller sees it: this run must not report itself \
             CONVERGED, because nothing but its own question ever said the north star was reached. \
             Walked {walk:?}",
        );

        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }
    }

    /// ⚠⚠⚠ **A QUESTION NOBODY WROTE A RULE FOR PAUSES THE RUN, AND A PERSON'S ANSWER RESUMES IT** —
    /// `awaiting_human`, the last state of `ai_loop.scxml` this driver had not built.
    ///
    /// # ⚠⚠⚠ What was there before, and why it was wrong rather than incomplete
    ///
    /// The driver answered `Pumped::Unbuilt` for this state and the [`Driver`] stopped the run. The
    /// scope note that shipped with it read *"a rule that claims nothing ends the run exactly as an
    /// unanswered dialog always has"* — a true sentence about the DRIVER and a false one about the
    /// machine. `awaiting_human` has seven edges and six of them are ways to carry on; the document
    /// says the loop *"stops prompting and waits"*, and that *"if the person answers the dialog, the
    /// agent goes back to work and the loop resumes where it was."*
    ///
    /// **A state machine with no input stays in the state.** Ending the run instead was the driver
    /// deciding something the document does not say — which is the one thing this whole arrangement
    /// exists to prevent.
    ///
    /// # ⚠⚠ The two halves, and why the first one needs a control
    ///
    /// 1. **IT WAITS.** Pumped repeatedly with the dialog up and nobody there, the machine stays in
    ///    `awaiting_human` and the run does not end. ⚠ The control is that it is pumped MANY times:
    ///    a single pump would pass for a driver that ends the run on its second look.
    /// 2. **A PERSON'S KEYSTROKE MOVES IT ON.** The person presses the key the peer is waiting for,
    ///    the agent finishes the turn it was blocked in, and `awaiting_human --turn.done--> judging`
    ///    carries the run to its milestone. ⚠ The keystroke is written as
    ///    [`Hand::APerson`](sprag_terminal::Hand::APerson), which is what a person's hand at a real
    ///    pane looks like to this product.
    #[test]
    fn a_question_no_rule_claims_pauses_the_run_and_a_person_resumes_it() {
        /// How many pumps the loop must sit through before anybody touches the pane. ⚠ Large enough
        /// that a driver ending the run *eventually* fails here rather than passing.
        const WAITED: usize = 12;

        // ⚠ The peer asks ONE question and takes Escape for an answer — so the "person" below is
        // pressing the key this dialog is actually waiting for, not a key the fixture invented.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, 2, None);
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            // ⚠⚠ NO SCREEN RULES AT ALL — this is the shipped shape, whose placeholder claims
            // nothing. The dialog therefore reaches `screening`, no rule takes it, and
            // `screen.none` leads here. That is the commonest way a real run meets this state.
            // ⚠ A person IS expected, and says so here — see the `WhenStill` note below.
            &Brief {
                // ⚠⚠⚠ A PERSON IS DECLARED, AND THIS GATE IS WHERE THAT STOPPED BEING OPTIONAL.
                // With `Attended::NoOne` — every other gate's value, and the old default — a
                // person's hand at the pane is a TAKEOVER for ever after: measured here as
                // `AwaitingHuman --TurnDone--> Judging --Judge--> Working
                // --TurnInterrupted--> AwaitingHuman`, round and round, because the barrier went on
                // reporting the keystroke that unblocked the dialog. That is the honest reading of
                // `NoOne` (*nobody is watching, so a hand means somebody took the pane*) and the
                // wrong contract for a run whose whole point is that a person may answer it.
                // `WhenStill` is what says the pane is the run's again once they have finished.
                await_person_ms: Some(30_000),
                handback_still_ms: Some(300),
                // ⚠ A SHORTER TURN BOUND THAN THE OTHER GATES', and it is about this gate's COST
                // rather than its claim: a pump that finds nothing blocks for the turn's whole
                // patience, and this one deliberately pumps many times with nothing happening.
                // ⚠ It stays above `supervised_asking`'s 300 ms settle, or no turn could ever be
                // seen to end.
                turn_within_ms: Some(1_000),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        let mut walked: Vec<String> = Vec::new();
        let step = |loops: &mut AiLoop, walked: &mut Vec<String>| {
            let step = loops
                .step(&access, &run)
                .expect("every step of a paused run must be readable");
            if let Some(note) = step.note.clone() {
                walked.push(note);
            }
        };

        // ── 1. IT REACHES THE PAUSE ──
        let mut reached = false;
        for _ in 0..40 {
            step(&mut loops, &mut walked);
            if loops.state() == AiLoopState::AwaitingHuman {
                reached = true;
                break;
            }
        }
        assert!(
            reached,
            "⚠ the control: a dialog no rule claims must reach `awaiting_human`, or what follows is \
             about a state the run never entered. Walked {walked:?}",
        );

        // ── 2. AND IT STAYS THERE, FOR AS LONG AS NOBODY COMES ──
        for look in 0..WAITED {
            step(&mut loops, &mut walked);
            assert_eq!(
                loops.state(),
                AiLoopState::AwaitingHuman,
                "⚠⚠⚠ A STATE MACHINE WITH NO INPUT STAYS IN THE STATE. Nobody has touched this pane \
                 and the document's every exit from `awaiting_human` is something that HAPPENS — so \
                 look {look} of {WAITED} must find the loop exactly where the last one left it. \
                 This driver used to answer `Unbuilt` here and the run was over. Walked {walked:?}",
            );
        }

        // ── 3. THE PERSON ANSWERS, AND THE RUN CARRIES ON ──
        crate::testing::person_types(&access, pane, &[27]);
        // ⚠⚠⚠ WALL CLOCK, AND IT IS LOAD-BEARING RATHER THAN A SLEEP TO BE TIDIED AWAY. A real
        // supervisor's verdict SETTLES — `supervised_asking` models that with a 300 ms window — and
        // a gate that pumps in a tight loop polls FASTER than the window it is waiting out: sixty
        // pumps went by in under 300 ms, every one of them reading the answered dialog as still
        // blocked, and the run looked stuck when it was merely being asked too quickly. A real
        // [`Driver`] paces itself; a `while` loop does not.
        //
        // ⚠ The samples are kept because they are what the failure message needs: *what did the
        // supervisor actually say* is the first question of any turn that does not end.
        let mut samples: Vec<String> = Vec::new();
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(200));
            let seen = access
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(pane));
            samples.push(format!(
                "{:?}/{:?}",
                seen.as_ref().map(|s| s.state),
                seen.as_ref().map(|s| s.seq)
            ));
        }
        for _ in 0..60 {
            step(&mut loops, &mut walked);
            if matches!(
                loops.state(),
                AiLoopState::Converged
                    | AiLoopState::Exhausted
                    | AiLoopState::Failed
                    | AiLoopState::Cancelled
                    | AiLoopState::Blocked
            ) {
                break;
            }
        }
        let reached = loops.state();
        // ⚠ WHAT THE ASSERTION ACTUALLY SAW, printed rather than theorised about — the recorded
        // rule that a green (or red) mutation is re-read from the screen before it is diagnosed.
        let showing = access.pane_full_lines(pane).unwrap_or_default().join("\n");
        let seen = access
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(pane));
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        assert!(
            walked
                .iter()
                .any(|note| note == "AwaitingHuman --TurnDone--> Judging"),
            "⚠⚠⚠ THE DOCUMENT'S OWN EDGE: *\"the person answered (or typed a turn themselves) and it \
             completed\"*. Without it the run either never noticed the keystroke or had already been \
             ended by the driver. The pane was showing:\n{showing}\nThe supervisor said \
             {seen:?}\nSamples over the second after the keystroke: {samples:?}\nWalked {walked:?}",
        );
        // ⚠⚠⚠ **AND THAT LINE SAYS NOTHING ABOUT A REFUSAL, WHICH IS A CLAIM AND NOT A SPELLING.**
        // The equality above happens to hold it; this says out loud what it is holding, because a
        // gate nobody wrote for a hazard is a gate that gets loosened by whoever next finds it
        // inconvenient. Since register item 240 a walk names the refusal a pass ARRIVED AT — and
        // the notice a paused run is holding is NOT cleared when the person answers (only the next
        // prompt clears it), so a journal composed from what the loop holds rather than from what
        // the pass found would put *no standing instruction claims this dialog* on the very edge
        // that person's answer caused. **Measured, by mutating `pump` to read the level:** thirteen
        // consecutive `AwaitingHuman: looked` lines carried it and then so did this one.
        assert!(
            !walked.iter().any(|note| {
                note.starts_with("AwaitingHuman")
                    && crate::consent::Refusal::ALL
                        .iter()
                        .any(|why| note.contains(&format!("{}: ", why.wire_str())))
            }),
            "⚠⚠⚠ A REFUSAL IS REPORTED BY THE PASS THAT FOUND IT, ONCE. No line the loop wrote \
             while it was WAITING may carry one: the dialog was answered by a person, and a reader \
             told otherwise is being sent to write a standing rule about a question that is gone. \
             Walked {walked:?}",
        );
        assert_eq!(
            reached,
            AiLoopState::Converged,
            "⚠⚠ and the run must go on to REACH ITS MILESTONE. A pause that resumes into anything \
             else has moved the loop somewhere the person's answer did not ask for. Walked \
             {walked:?}",
        );
    }

    /// ⚠⚠⚠ **A REPLACEMENT SESSION THAT COMES UP ASKING ENDS THE RUN, AND THE RUN SAYS SO** — the
    /// `resuming` edge a person meeting this feature is likeliest to hit.
    ///
    /// # ⚠⚠⚠ Why this is the arm worth a fixture of its own
    ///
    /// A fresh agent CLI does not always come up ready to be typed at. A trust prompt, a model picker,
    /// a sign-in — `sprag-detect` holds captures of five such screens from two real agents, and every
    /// one of them is shown BEFORE the agent works. So the pane a `restarting` opens can be a pane
    /// nothing this run holds may answer, and the document is explicit about what that means:
    /// *"a session that will not come back is a failed run, not a stuck one."*
    ///
    /// ⚠⚠ **AND THE SENTENCE IS THE ASSERTION.** This round's own sweep found the defect: `resuming`
    /// recorded the question it was refused by, and `AiLoop::ended`'s `failed` arm knew only about a
    /// datamodel that had stopped answering — so the run's single sentence said *"it reached `failed`
    /// and recorded no reason"* about a run holding the reason, and the question was dropped. A caller
    /// would have been told a path nobody wrote had been taken.
    ///
    /// ⚠ The peer cannot know which of its lives it is in, because a replacement runs the same argv on
    /// purpose. It asks the FILESYSTEM — see [`standin_agent_refusing`](crate::testing).
    #[test]
    fn a_replacement_session_that_comes_up_asking_ends_the_run_saying_what_it_asked() {
        /// The dialog the peer raises on its FIRST life, which the rule below claims.
        const ASKS: &str = "Which way should I build this?";
        /// The redirect. ⚠ It leaves the milestone unmet, so the judgement after it reflects.
        const INSTEAD: &str = "not that way; report and wait";

        // ⚠ ONE FILE, not a directory, so the cleanup below is a single removal and a panic leaks
        // nothing a person has to go and find. The name carries the pid and a nanosecond count for the
        // reason every fixture path in this workspace does: two runs of this suite must not share it.
        let marker = std::env::temp_dir().join(format!(
            "sprag-second-life-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos()),
        ));
        let _ = std::fs::remove_file(&marker);
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, 1, Some(&marker));
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                screen_rules: Some(
                    crate::screen::ScreenRules::of(vec![
                        crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                            .expect("both halves are non-empty"),
                    ])
                    .expect("a non-empty list"),
                ),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 60,
            max_cost: None,
            max_duration: Some(Duration::from_secs(90)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        for pane in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(pane);
        }
        let _ = std::fs::remove_file(&marker);

        assert!(
            walk.iter()
                .any(|note| note == "Restarting --SessionReplaced--> Resuming"),
            "⚠ THE CONTROL: the run must have got as far as opening a replacement, or what follows is \
             about a session that was never replaced. Walked {walk:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⚠⚠⚠ a replacement that came up asking something nothing could answer must END the run. \
             Carrying on would mean typing a prompt at a pane showing somebody else's question, which \
             is the one thing this crate's barrier exists to prevent. Walked {walk:?}",
        );
        let said = format!("{:?}", outcome.failure);
        assert!(
            said.contains("came up asking") && said.contains("Which way"),
            "⚠⚠⚠ AND THE SENTENCE MUST CARRY THE QUESTION. Until this round it said *it reached \
             `failed` and recorded no reason* — about a run that had recorded one, holding the very \
             dialog its caller needs to see: {said:?}",
        );
    }

    /// ⚠⚠⚠ **A CANCELLED LOOP STOPS THE AGENT'S TURN IT SET GOING** — [`Plugin::driving`]'s whole
    /// reason, on the plugin whose peer is the most expensive one in this tree.
    ///
    /// A run has two endings that can land while a step is blocked — somebody cancels it, or its
    /// deadline passes — and both can arrive while a real agent is mid-turn. A plugin that answered
    /// `None` here would have the Driver report `cancelled` while the model it prompted went on
    /// spending somebody's tokens on a question nothing is waiting for.
    ///
    /// ⚠⚠ **THE STOP IS THE ASSERTION**, not the word `cancelled`: a run reports `cancelled` with
    /// or without this, and [`Stopped`] is the only thing that says what became of the work.
    #[test]
    fn a_cancelled_loop_stops_the_turn_its_agent_was_taking() {
        /// ⚠⚠⚠ A BUDGET NO 600-MILLISECOND RUN CAN SPEND, and it has to be said why it is this large.
        ///
        /// This gate asked for FORTY turns and passed for two rounds — because the stand-in's
        /// supervisor counted words on a screen, so `seq` stopped growing at the first scroll and
        /// every turn after the third burned the whole five-second turn patience. The run was still
        /// inside turn one when the cancel landed. With the counter read off the peer instead
        /// ([`peer_seq`](crate::testing)) a turn takes milliseconds, forty of them fit inside the
        /// cancel window, and the run reported `exhausted — turns`: **the correct answer to a
        /// question this gate is not asking.**
        ///
        /// So the ceilings are put out of reach and the cancel is the only ending available, which is
        /// what *"a person's stop is the run's ending, above every ceiling"* needs in order to mean
        /// anything.
        const UNSPENDABLE: i64 = 1_000_000;

        // ⚠ NEVER says the marker, so the run is still mid-loop when the cancel lands.
        let (workspace, pane) = standin_agent(u32::MAX);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(UNSPENDABLE), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        let cancel = Arc::new(AtomicBool::new(false));
        let run = RunContext::new(Arc::clone(&cancel));
        let raiser = {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || {
                let armed = Instant::now();
                // Long enough that the loop is past `idle` and actually driving the peer, short
                // enough that the gate stays cheap.
                while armed.elapsed() < Duration::from_millis(600) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                cancel.store(true, Ordering::Release);
                // ⚠ WHEN the flag went up, so the gate below can measure the run's reaction rather
                // than the fixture's sleep. Returned rather than shared, because the only reader
                // wants it after the join anyway.
                Instant::now()
            })
        };
        let outcome = Driver::new(Guardrails {
            // ⚠ The substrate's ceilings are put out of reach for `UNSPENDABLE`'s reason: the
            // iteration count is the one a fast peer reaches first, and this gate is about neither.
            max_iterations: u32::try_from(UNSPENDABLE).expect("a positive budget"),
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &run);
        let returned = Instant::now();
        let raised = raiser.join().expect("the canceller");
        let honoured = returned.saturating_duration_since(raised);

        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "a person's stop is the run's ending, above every ceiling: {:?}",
            outcome.state,
        );
        // ⚠⚠⚠⚠ **HOW LONG THIS LOOP TAKES TO HONOUR A CANCEL** — the number
        // `sprag_host::runs::RunRegistry::JOIN_DEADLINE` is chosen against, for the run kind the
        // daemon actually drives. Register item 305 measured the ORCHESTRATOR (2.7 - 10.5 ms over a
        // real pane) and reasoned that a loop is the same shape because its waits are the same
        // `poll_until`. Reasoned is not measured, and a loop's step is the heavier one: it composes
        // a prompt, delivers it, waits on a turn contract, and on the way out signals the agent's
        // job (`Stopped::Job`, asserted below). Measured here at **0.8 - 11.6 ms** (four samples,
        // 2026-08-17).
        //
        // ⚠⚠⚠⚠ **AND WHICH PATH THAT NUMBER DESCRIBES WAS ESTABLISHED BY MUTATION, NOT BY GUESSING
        // — the first draft of this comment guessed WRONG.** It said the number is dominated by one
        // 10 ms poll of the cancel flag, as the orchestrator's is. Raising `POLL_INTERVAL` ninety-
        // fold, to 900 ms, leaves this gate GREEN and unmoved: the loop here is cycling between
        // steps against a stand-in that answers in milliseconds, so what honours the flag is the
        // DRIVER'S LOOP TOP, which consults it before every step and never sleeps. The same
        // mutation puts the orchestrator's measurement at **892 ms** (`rpc`'s
        // `a_running_run_honours_cancel_well_inside_the_join_deadline`), because THAT run is parked
        // in a wait for a sentinel that never comes. **The two gates measure the two paths**, and a
        // shutdown's deadline has to clear both.
        //
        // ⚠⚠ The bound is a REQUIREMENT and not a reading, and deliberately not a fraction of
        // `JOIN_DEADLINE` — that constant lives in another crate this one must not depend on, and a
        // bound written as a fraction of the thing it defends could never catch it moving (item
        // 391). Half a second is forty times the measured worst and an order of magnitude inside
        // the five-second deadline, so this reddens long before a shutdown would begin detaching
        // live loops.
        assert!(
            honoured < Duration::from_millis(500),
            "the loop took {honoured:?} to come back after the flag went up — a shutdown's join \
             deadline is chosen against this number, so it has to be measured when it moves",
        );
        assert!(
            matches!(outcome.stopped, Some(Stopped::Job(_))),
            "⚠⚠⚠ the pane's job must have been SIGNALLED. Anything else means the loop's door \
             closed on a room its agent is still working in: {:?}",
            outcome.stopped,
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A RUN STOPPED WITH A KEY IN SOMEBODY'S DIALOG PRESSES NOTHING ELSE AND KEEPS WHAT IT
    /// KNOWS** — register item 241, which was three facts held by a comment.
    ///
    /// `Refusal::Unwitnessed` is what the barrier answers when *the key went out, the run ended,
    /// and nobody looked*. The state that answer lands the machine in is `screening`, and
    /// `screening`'s act would do BOTH of the things that arm exists to prevent:
    ///
    /// * re-head it `no_rule` through [`Unanswered::unscreened`] — burying the one fact a reader of
    ///   a stopped run has, which is that nothing was established about the peer at all;
    /// * and, where a rule DOES claim the dialog, press the refusing key into a question this run
    ///   has stopped being allowed to touch.
    ///
    /// # ⚠⚠⚠ Why the three facts had to be held TOGETHER, and by a run
    ///
    /// Neither happens, and the reason is a conjunction: the barrier builds that arm **only when
    /// the run has ended**; `screening` is the **NEXT** pump; and the Driver asks
    /// `ended_from_outside` at its loop top **and after every unconverged step**, so that pump
    /// never comes. Each fact lives in a different file, each is true on its own, and the claim is
    /// the AND of them — which is exactly the shape no unit gate can hold. `screen::tests` measures
    /// the refusal, `driver::tests` measures a pre-raised cancel, and between them sat the sentence
    /// nobody was measuring: **a stopped run types nothing further.**
    ///
    /// ⚠ The step that notices answers `Continue`, so the ask that fires here is the POST-STEP one
    /// and not the loop top — which is why neither of those two sites can be mutated alone to make
    /// this red: each covers for the other. What is being asserted is the guarantee, not one of its
    /// two implementations.
    ///
    /// ⚠⚠ **AND THE COUNTERFACTUAL IS RUN, so the hazard is a measurement rather than an
    /// argument.** Each arm takes the very step the Driver refused to take — one `Plugin::step`, by
    /// hand, on the run that is already over — and asserts that it DOES the damage. Without it this
    /// gate would be four assertions about a run that stopped, passing for a product in which
    /// `screening` was harmless all along.
    ///
    /// ⚠ The peer is `standin_agent_refusing`'s **un-dismissable** one, and the choice is the
    /// staging: its dialog never leaves the screen whatever arrives, so what `screening` would do
    /// on the next pump is a fact and not a race with a peer that might already have moved on. A
    /// consent quotes it because what is being measured is the ANSWERING act's stopped arm, not
    /// whether that particular question is one a caller should authorise.
    #[test]
    fn a_run_stopped_at_its_peers_dialog_types_nothing_further() {
        /// The stand-in's dialog, quoted by the consent that answers it and by the rule the second
        /// arm arms.
        const ASKS: &str = "Which way should I build this?";
        /// The option the consent takes — the one the peer's own marker is already standing on, so
        /// the key that goes in is the one whose landing place `Question::selected` proves.
        const TAKES: &str = "The quick one";
        /// What the second arm's standing instruction would say, if it ever got to say it.
        const INSTEAD: &str = "neither; do the smallest verifiable thing and report";

        /// One run against the un-dismissable peer, with a consent that answers its dialog and
        /// whatever standing instructions the author left — **cancelled by the double at the first
        /// key this run presses into that dialog**.
        fn stopped_at_the_dialog(
            screen_rules: Option<crate::screen::ScreenRules>,
        ) -> (
            AiLoop,
            crate::driver::Outcome,
            crate::testing::StopsAtTheKey,
            Vec<String>,
        ) {
            let (workspace, pane) = crate::testing::standin_agent_refusing(false, 1, None);
            let stopping = crate::testing::StopsAtTheKey::at_a_dialog(
                crate::testing::supervised_asking(&workspace),
            );
            let consent = crate::consent::Consents::of(vec![
                crate::consent::Consent::parse(ASKS.to_owned(), TAKES.to_owned())
                    .expect("both needles are non-empty"),
            ])
            .expect("a non-empty consent list");
            let mut loops = AiLoop::new(
                engine(),
                pane,
                // ⚠ BOTH AUTHORITIES ON ONE BRIEF NOW — the rule that refuses and the clause that
                // approves are the same kind of thing, and this gate is about them meeting.
                &Brief {
                    screen_rules,
                    may_answer: Some(consent),
                    ..brief_for(40)
                },
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &stopping, &stopping.run());
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            (loops, outcome, stopping, walk)
        }

        /// The four facts every arm shares — where the run stopped, what it still knows, and the
        /// ledger of what it typed afterwards.
        fn it_stopped_holding_the_finding(
            loops: &AiLoop,
            outcome: &crate::driver::Outcome,
            stopping: &crate::testing::StopsAtTheKey,
            walk: &[String],
        ) {
            assert_eq!(
                outcome.state,
                OutcomeState::Cancelled,
                "⚠⚠⚠ THE DRIVER'S HALF: the run was stopped while a key it had just pressed was on \
                 the pseudoterminal, and nothing about the dialog underneath makes that a different \
                 ending. Walked {walk:?}",
            );
            assert_eq!(
                loops.state(),
                AiLoopState::Screening,
                "⚠⚠⚠ THE MACHINE'S HALF, and the fact this whole gate turns on: the blocked turn \
                 moved the document INTO `screening` and the act that state performs never ran — \
                 it is the NEXT pump, and there was not one. A loop found anywhere else is a loop \
                 whose screening key has already been pressed. Walked {walk:?}",
            );
            let asking = loops.asking();
            assert_eq!(
                asking.why(),
                crate::consent::Refusal::Unwitnessed,
                "⚠⚠⚠ AND THE FINDING SURVIVED INTACT. `no_rule` here is the burial this arm exists \
                 to stop — it would send a reader to write a standing instruction about a dialog \
                 nobody has established anything about, when the remedy is to READ THE PANE. \
                 Walked {walk:?}: {asking:?}",
            );
            assert!(
                asking.bytes() > 0,
                "⚠⚠ THE CONTROL: the answering key really did go in, or `unwitnessed` is a word \
                 about an act that never began and every assertion here is about nothing: {asking:?}",
            );
            assert!(
                stopping.typed_after_the_stop().is_empty(),
                "⚠⚠⚠ **AND NOTHING FURTHER WAS TYPED.** A question that may still be up reads the \
                 next keystroke as an answer to itself — a live probe measured exactly that \
                 approving a file write — so a run that has stopped may press nothing at all. It \
                 pressed: {:?}. Walked {walk:?}",
                stopping.typed_after_the_stop(),
            );
        }

        /// Take the step the Driver refused to take, on a run that is already over.
        fn the_pump_that_never_came(
            loops: &mut AiLoop,
            stopping: &crate::testing::StopsAtTheKey,
        ) -> Verdict {
            loops
                .step(stopping, &stopping.run())
                .expect("a stopped run's pump is not an error")
                .verdict
        }

        fn close(stopping: &crate::testing::StopsAtTheKey) {
            for id in stopping.pane_ids() {
                stopping.lifecycle().expect("lifecycle").close(id);
            }
        }

        // ── THE SHIPPED LOOP: NO RULE CLAIMS THE DIALOG ──
        let (mut unarmed, outcome, stopping, walk) = stopped_at_the_dialog(None);
        it_stopped_holding_the_finding(&unarmed, &outcome, &stopping, &walk);

        // ⚠⚠⚠ AND WHAT THE PUMP THAT NEVER CAME WOULD HAVE DONE TO IT.
        let _ = the_pump_that_never_came(&mut unarmed, &stopping);
        assert_eq!(
            unarmed.asking().why(),
            crate::consent::Refusal::NoRule,
            "⚠⚠⚠ THE HAZARD, MEASURED: one more pump and `Unanswered::unscreened` re-heads the \
             finding as *nothing claimed this dialog*, which is a sentence about the AUTHOR'S \
             rules and not about a run that stopped holding a key. If this ever stops being true \
             the assertion above has been passing for free",
        );
        assert!(
            stopping.typed_after_the_stop().is_empty(),
            "⚠ and the unarmed arm's damage is the re-heading ALONE — with no rule to fire, \
             `screening` returns before the refusing key, so this half of the hazard needs the \
             armed arm below to be measured at all: {:?}",
            stopping.typed_after_the_stop(),
        );
        close(&stopping);

        // ── AND THE SAME RUN WITH ONE STANDING INSTRUCTION QUOTING THAT DIALOG ──
        let rules = crate::screen::ScreenRules::of(vec![
            crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                .expect("both halves are non-empty"),
        ])
        .expect("a non-empty list");
        let (mut armed, armed_outcome, armed_stopping, armed_walk) =
            stopped_at_the_dialog(Some(rules));
        it_stopped_holding_the_finding(&armed, &armed_outcome, &armed_stopping, &armed_walk);

        // ⚠⚠⚠ AND HERE THE PUMP THAT NEVER CAME WOULD HAVE PRESSED A KEY.
        let _ = the_pump_that_never_came(&mut armed, &armed_stopping);
        let pressed = armed_stopping.typed_after_the_stop();
        assert!(
            pressed.iter().any(|keys| keys == crate::screen::REFUSES),
            "⚠⚠⚠ THE OTHER HALF OF THE HAZARD, MEASURED: with a rule that claims the dialog, the \
             very next pump puts {:?} into a question this run had already stopped being allowed \
             to touch — and the peer here is the one whose dialog never goes, so the key lands on \
             a menu that is still up. It typed: {pressed:?}",
            crate::screen::REFUSES,
        );
        // ⚠⚠ AND THE LOOP READS THE SCREENING HALF OF THE SAME WORD — register item 240's first
        // half. `screen::refuse` answers `Refused::StillUp` carrying its OWN `Unwitnessed` here (a
        // key pressed by a run that is over, watched by nobody), and until this line the only gate
        // over that arm was `screen`'s own: how `outer::screen` carried it into the loop's notice
        // was measured by nothing.
        assert_eq!(
            armed.asking().why(),
            crate::consent::Refusal::Unwitnessed,
            "⚠⚠ the refusing key's own stopped arm must reach the loop's notice as itself: \
             {:?}",
            armed.asking(),
        );
        close(&armed_stopping);
    }

    /// ⚠⚠⚠ **THE WALK NAMES THE REFUSAL THAT PAUSED THE RUN, NOT ONLY THE EDGE IT LEFT BY** —
    /// register item 240's second half, the JOURNAL one.
    ///
    /// # ⚠⚠⚠ The defect, in its own words
    ///
    /// `screen.none` is ONE edge with several causes behind it, and the journal wrote the same six
    /// words for every one of them: `Screening --ScreenNone--> AwaitingHuman`. Three runs whose
    /// remedies are three DIFFERENT things —
    ///
    /// * [`Refusal::NoRule`](crate::consent::Refusal::NoRule): *go and quote this dialog in
    ///   `screen_rules`*;
    /// * [`Refusal::NotDismissed`](crate::consent::Refusal::NotDismissed): *your agent did not take
    ///   the key that refuses a call, and the dialog is still up*;
    /// * [`Refusal::Unwitnessed`](crate::consent::Refusal::Unwitnessed): *this run ended holding a
    ///   key nobody watched land; READ THE PANE* —
    ///
    /// left walks that were byte-for-byte identical on the line that mattered.
    ///
    /// ⚠⚠ The pair that makes it a defect rather than a terseness is the two arms `screen::refuse`
    /// answers `StillUp` with. `not_dismissed` is a fact about the AGENT and `unwitnessed` is a
    /// fact about THIS RUN's ending — R394 built the second one precisely because publishing the
    /// first about a run nobody watched is this crate's favourite defect — and the walk went on
    /// publishing neither.
    ///
    /// ⚠⚠⚠ **AND THE VERDICT IS NOT A SUBSTITUTE FOR THE LINE.** The step that walks this edge
    /// answers `Verdict::Continue`: `awaiting_human` is not a final state, so nothing structural on
    /// that step carries the finding. What a later step reports is a LATER reading — the third run
    /// below never reaches one at all, because the Driver ends it — so the line is the only place
    /// the fact can live.
    ///
    /// # ⚠ Why all three runs share one peer, one dialog and one brief
    ///
    /// The peer is `standin_agent_refusing`'s **un-dismissable** one in every arm, so the SCREEN is
    /// the same in all three and the only thing that varies is what the run was given and how it
    /// ended. A gate whose arms used different fixtures would be comparing three walks about three
    /// situations; these three are one situation with three causes, which is the whole subject.
    #[test]
    fn the_walk_says_which_refusal_left_the_run_waiting_for_a_person() {
        /// The stand-in's dialog, quoted by the standing instruction two of the arms are given.
        const ASKS: &str = "Which way should I build this?";
        /// What that instruction would say, if the dialog ever went away to let it be said.
        const INSTEAD: &str = "neither; do the smallest verifiable thing and report";
        /// The document's edge every arm below leaves `screening` by.
        const THE_EDGE: &str = "Screening --ScreenNone--> AwaitingHuman";

        /// One run against the un-dismissable peer: what the loop ended up holding, and its walk.
        ///
        /// `stops_at_the_key` is the third arm's whole difference — a double that ends the run at
        /// the first key pressed into a dialog, which for a run holding no consent is
        /// `screening`'s own [`crate::screen::REFUSES`].
        fn paused_run(
            screen_rules: Option<crate::screen::ScreenRules>,
            stops_at_the_key: bool,
        ) -> (OutcomeState, crate::consent::Refusal, Vec<String>) {
            let (workspace, pane) = crate::testing::standin_agent_refusing(false, 2, None);
            let stopping = crate::testing::StopsAtTheKey::at_a_dialog(
                crate::testing::supervised_asking(&workspace),
            );
            let run = if stops_at_the_key {
                stopping.run()
            } else {
                RunContext::uncancellable()
            };
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &Brief {
                    screen_rules,
                    ..brief_for(40)
                },
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &stopping, &run);
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            for id in stopping.pane_ids() {
                stopping.lifecycle().expect("lifecycle").close(id);
            }
            (outcome.state, loops.asking().why(), walk)
        }

        /// The one line this whole gate is about, off `walk` — and a failure that prints the walk
        /// rather than an index, because *which line* is the first question of any red here.
        fn the_line<'a>(label: &str, walk: &'a [String]) -> &'a str {
            let mut lines = walk.iter().filter(|note| note.starts_with(THE_EDGE));
            let found = lines.next().unwrap_or_else(|| {
                panic!(
                    "⚠ the control for {label}: this run must have LEFT `screening` by \
                     {THE_EDGE:?}, or what follows is about an edge it never took. Walked {walk:?}"
                )
            });
            assert!(
                lines.next().is_none(),
                "⚠ the control for {label}: the run took that edge more than once, so the line \
                 read below is not the one the refusal belongs to. Walked {walk:?}",
            );
            found
        }

        let rules = || {
            crate::screen::ScreenRules::of(vec![
                crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                    .expect("both halves are non-empty"),
            ])
            .expect("a non-empty list")
        };

        // ── THREE CAUSES, ONE EDGE ──
        let (unclaimed_end, unclaimed_why, unclaimed_walk) = paused_run(None, false);
        let (ignored_end, ignored_why, ignored_walk) = paused_run(Some(rules()), false);
        let (unwatched_end, unwatched_why, unwatched_walk) = paused_run(Some(rules()), true);

        // ⚠⚠ AND THE THIRD ARM ENDS DIFFERENTLY FROM THE OTHER TWO, which is exactly why its
        // finding has nowhere else to live: the first two runs wait out their (absent) person and
        // report `blocked` WITH the question, so a caller reads the refusal off the outcome. The
        // third is stopped by the double at the refusing key, so its outcome is `cancelled` — an
        // ending that carries no question at all — and the walk is the ONLY record of what it had
        // found one step earlier.
        assert!(
            matches!(unclaimed_end, OutcomeState::Blocked(Some(_)))
                && matches!(ignored_end, OutcomeState::Blocked(Some(_)))
                && unwatched_end == OutcomeState::Cancelled,
            "⚠⚠ the control on the three endings: {unclaimed_end:?} / {ignored_end:?} / \
             {unwatched_end:?}",
        );

        // ⚠⚠ THE CONTROL, AND IT IS THE EXPENSIVE HALF OF THIS GATE: three runs that all reported
        // the same refusal would make every assertion below true for the wrong reason.
        let arms = [
            ("no rule claims the dialog", unclaimed_why, &unclaimed_walk),
            ("the agent ignored the key", ignored_why, &ignored_walk),
            ("the run ended at the key", unwatched_why, &unwatched_walk),
        ];
        assert_eq!(
            (unclaimed_why, ignored_why, unwatched_why),
            (
                crate::consent::Refusal::NoRule,
                crate::consent::Refusal::NotDismissed,
                crate::consent::Refusal::Unwitnessed,
            ),
            "⚠⚠⚠ the control: the three arms must really have arrived at three DIFFERENT refusals, \
             or this gate is comparing one cause with itself. Walked {:?} / {:?} / {:?}",
            unclaimed_walk,
            ignored_walk,
            unwatched_walk,
        );

        // ── AND THE WALK SAYS WHICH ──
        for (label, why, walk) in arms {
            let line = the_line(label, walk);
            assert!(
                line.contains(why.wire_str()),
                "⚠⚠⚠ THE JOURNAL HALF OF REGISTER ITEM 240: this run stopped because {label}, and \
                 the one line its walk wrote about leaving `screening` does not carry the word for \
                 it ({:?}). A person reading a paused run's journal has to be able to tell *quote \
                 the dialog in a rule* from *your agent ignored the key* from *nobody watched the \
                 key land* — three different remedies behind one edge. The line was {line:?}; \
                 walked {walk:?}",
                why.wire_str(),
            );
            assert!(
                line.contains(why.describe()),
                "⚠⚠ AND THE SENTENCE TRAVELS WITH THE WORD. `Refusal::describe` is where each arm's \
                 REMEDY is written, and a word alone sends its reader off to look one up: {line:?}",
            );
            // ⚠⚠⚠ AND EXACTLY ONE LINE CARRIES IT — a walk is a record of EDGES, and this is the
            // half that says so. Composed from *what is the loop holding now* instead of *what did
            // this pass arrive at*, the reason is true on every later step too: measured, the
            // paused run wrote its own reason into THIRTEEN consecutive `AwaitingHuman: looked,
            // nothing had happened` lines and then onto `AwaitingHuman --TurnDone--> Judging` —
            // the edge a PERSON'S ANSWER causes, claiming the dialog was still unclaimed. ⚠ The
            // colon is what makes this count the refusal's own heading rather than a mention of
            // the word inside some other arm's detail.
            let heading = format!("{}: ", why.wire_str());
            assert_eq!(
                walk.iter().filter(|note| note.contains(&heading)).count(),
                1,
                "⚠⚠⚠ {label}: {heading:?} must head exactly ONE line of this walk — the step that \
                 arrived at it. More than one is a level being written down as a series of \
                 findings, which fills a bounded journal with one fact and puts that fact on edges \
                 it is not true of. Walked {walk:?}",
            );
        }
        let lines = [
            the_line("no rule claims the dialog", &unclaimed_walk),
            the_line("the agent ignored the key", &ignored_walk),
            the_line("the run ended at the key", &unwatched_walk),
        ];
        let distinct: std::collections::BTreeSet<&str> = lines.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            lines.len(),
            "⚠⚠⚠ AND THE THREE LINES MUST DIFFER FROM ONE ANOTHER. Naming the refusal is worth \
             nothing if two causes still render one string — which is exactly what {THE_EDGE:?} did \
             for all three of them before this gate existed: {lines:?}",
        );
    }

    /// ⚠⚠⚠ **THE WALK SAYS WHY THE RUN STOPPED TO REFLECT** — register item 261, which is item
    /// 240's class one state over and which `ai_loop.scxml` had been confessing to in prose.
    ///
    /// # ⚠⚠⚠ What was measured, and where the fact already was
    ///
    /// `judging` has three edges into `reflecting` and they are three different runs:
    ///
    /// | what happened | what a reader should do |
    /// |---|---|
    /// | the agent said the milestone was reached | look at the checkpoint it chose next |
    /// | a standing instruction fired | look at the instruction, and at the dialog behind it |
    /// | the reflection budget came round | nothing — this is the loop's own housekeeping |
    ///
    /// All three rendered `Judging --Judge--> Reflecting`, byte for byte. And the reason was
    /// already computed: each of those transitions carries an `<assign location="reflect_reason">`
    /// whose value NOTHING read but a livelock guard — **debt item 49's shape, a value stored and
    /// never used**, with the document's own comment saying so: *"which one fired is not published
    /// anywhere."*
    ///
    /// # ⚠⚠ Why three runs and not three calls to the renderer
    ///
    /// The renderer will say whatever it is handed. What has to be true is that a run which
    /// reflects for one reason PRODUCES that word — so each arm arranges its cause and nothing
    /// else, and the controls below are what stop three runs reflecting for the same reason and
    /// this gate passing on it:
    ///
    /// * the **milestone** arm's peer says the marker after one prompt, with the budget off;
    /// * the **instruction** arm's peer raises a dialog a `screen_rule` claims, with the budget
    ///   off — and `Screening --ScreenMatched--> Working` must be in its walk, because a run that
    ///   never screened cannot have reflected for an instruction and an arm asserting otherwise
    ///   would be measuring nothing;
    /// * the **budget** arm's peer needs nine prompts and never asks anything, with
    ///   `reflect_every: 2` — so nothing but the count can have sent it there, and the other two
    ///   arms' walks must contain no `Screening` at all.
    ///
    /// ⚠⚠⚠ AND THE THREE COVER THE WHOLE VOCABULARY, asserted rather than assumed: a
    /// `ReflectReason` arm no run here reaches would be a word rendered by nobody, and
    /// `every_edge_into_reflecting_says_why_in_a_word_this_driver_knows` holds the other end of
    /// that against the document itself.
    #[test]
    fn the_walk_says_why_a_run_stopped_to_reflect() {
        use crate::outer::ReflectReason;

        /// The one edge this gate is about — one arrow, three meanings.
        const THE_EDGE: &str = "Judging --Judge--> Reflecting";
        /// What a reflecting stand-in proposes when it is finally asked.
        const NEXT: &str = "the debt this run picked after the last one";
        /// And where it says the replacement should start reading.
        const READ_NEXT: &str = "the register entry for it";
        /// The dialog the refusing stand-in raises, quoted by the instruction arm's rule.
        const ASKS: &str = "Which way should I build this?";
        /// What that rule says to do instead.
        const INSTEAD: &str = "neither; do the smallest verifiable thing and report";

        /// Pump until the loop has replaced its session `sessions` times, ended, or run past this
        /// gate's own patience — and hand back everything the run wrote down.
        ///
        /// ⚠ STOPPING AT A REPLACEMENT is what keeps a walk to a known number of reflections:
        /// driven on, the budget arm would reflect again every two turns and *exactly one line*
        /// below would be a claim about this gate's stamina rather than about the product.
        fn walk_of<A: PaneAccess>(loops: &mut AiLoop, access: &A, sessions: usize) -> Vec<String> {
            let run = RunContext::uncancellable();
            let mut walked: Vec<String> = Vec::new();
            let mut replaced = 0;
            while walked.len() < 120 {
                let before = loops.state();
                let step = loops
                    .step(access, &run)
                    .expect("every step of a reflecting run must be readable");
                if let Some(note) = step.note.clone() {
                    walked.push(note);
                }
                if loops.state() == AiLoopState::Priming && before == AiLoopState::Resuming {
                    replaced += 1;
                }
                if replaced == sessions || AiLoop::is_final(loops.state()) {
                    break;
                }
            }
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
            walked
        }

        /// The one line each arm is about, off its walk — and a failure that prints the whole
        /// walk, because *which line* is the first question of any red here.
        fn the_line<'a>(label: &str, walk: &'a [String]) -> &'a str {
            let mut lines = walk.iter().filter(|note| note.starts_with(THE_EDGE));
            let found = lines.next().unwrap_or_else(|| {
                panic!(
                    "⚠ the control for {label}: this run must have taken {THE_EDGE:?}, or what \
                     follows is about an edge it never took. Walked {walk:?}"
                )
            });
            assert!(
                lines.next().is_none(),
                "⚠ the control for {label}: the run reflected more than once, so the line read \
                 below is not the one whose cause is being asserted. Walked {walk:?}",
            );
            found
        }

        // ── ARM 1: THE AGENT SAID THE MILESTONE WAS REACHED ──
        // ⚠ ONE working prompt and it says the marker, with `brief_for`'s equal pair leaving the
        // budget off — so `done` is the only guard that can fire.
        let (workspace, pane) = crate::testing::standin_agent_reflecting(1, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let reached_walk = walk_of(&mut loops, &access, 1);
        let reached_screened = loops.screened();

        // ── ARM 2: A STANDING INSTRUCTION FIRED ──
        // ⚠ The budget is off here too, so `screened > screened_carried` is the only guard left —
        // and it can only be true if `screening` really carried the rule out.
        let (workspace, pane) = crate::testing::standin_agent_refusing(true, u32::MAX, None);
        let access = crate::testing::supervised_asking(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                screen_rules: Some(
                    crate::screen::ScreenRules::of(vec![
                        crate::screen::ScreenRule::parse(ASKS.to_owned(), INSTEAD.to_owned())
                            .expect("both halves are non-empty"),
                    ])
                    .expect("a non-empty list"),
                ),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let instructed_walk = walk_of(&mut loops, &access, 1);
        let instructed_screened = loops.screened();

        // ── ARM 3: THE BUDGET CAME ROUND ──
        // ⚠ NINE prompts before the marker and a reflection due after TWO, and this peer asks
        // nothing — so neither of the guards above can be what sent it.
        let (workspace, pane) = crate::testing::standin_agent_reflecting(9, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 2,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let budget_walk = walk_of(&mut loops, &access, 1);
        let budget_screened = loops.screened();

        let arms = [
            (
                "the milestone was reached",
                ReflectReason::Milestone,
                &reached_walk,
            ),
            (
                "a standing instruction fired",
                ReflectReason::Instruction,
                &instructed_walk,
            ),
            ("the budget came round", ReflectReason::Budget, &budget_walk),
        ];

        // ── THE CONTROL THE INSTRUCTION ARM STANDS ON: `screening` REALLY FIRED ──
        //
        // ⚠⚠⚠ ASKED OF THE MACHINE'S OWN COUNTER, which is the very quantity the guard reads
        // (`cond="screened > screened_carried"`), and not of the walk. `screen.matched` reports a
        // `Verdict::Screened` whose note is the REFUSAL's own sentence rather than the edge, so a
        // walk-shaped control here would be asserting how that step happens to render — a second
        // authority on a fact the datamodel holds exactly.
        assert!(
            instructed_screened.is_some_and(|screened| screened > 0),
            "⚠⚠⚠ the control: this arm's whole claim is that a STANDING INSTRUCTION sent the run \
             to `reflecting`, and `screened` is incremented by `screening` carrying one out. This \
             run's counter says {instructed_screened:?}, so nothing was carried out and a green \
             below would be measuring some other guard. Walked {instructed_walk:?}",
        );
        assert!(
            instructed_walk
                .iter()
                .any(|note| note.starts_with("Working --TurnBlocked--> Screening")),
            "⚠⚠ and its journal must show the run going there, or the counter above moved by some \
             route this gate is not describing. ⚠ `starts_with` because that edge carries register \
             item 240's own clause — the refusal the pass arrived at. Walked {instructed_walk:?}",
        );
        for (label, screened, walk) in [
            ("the milestone was reached", reached_screened, &reached_walk),
            ("the budget came round", budget_screened, &budget_walk),
        ] {
            assert_eq!(
                screened,
                Some(0),
                "⚠⚠ AND THE OTHER TWO ARMS MUST NEVER SCREEN — {label}: any `screened` above zero \
                 makes `screened > screened_carried` true and takes the instruction edge, which is \
                 the guard tested ABOVE this one. Walked {walk:?}",
            );
        }

        // ── THE CONTROL ON THE VOCABULARY: these three are all of it ──
        let covered: std::collections::BTreeSet<ReflectReason> =
            arms.iter().map(|(_, reason, _)| *reason).collect();
        assert_eq!(
            covered,
            ReflectReason::ALL.into_iter().collect(),
            "⚠⚠⚠ the control: this gate must arrange EVERY reason a reflection can have. An arm \
             no run here reaches is a word nothing renders and a sentence nobody has read — and \
             the document half of that is \
             `every_edge_into_reflecting_says_why_in_a_word_this_driver_knows`",
        );

        // ── AND THE WALK SAYS WHICH ──
        for (label, reason, walk) in arms {
            let line = the_line(label, walk);
            assert_eq!(
                line,
                format!("{THE_EDGE} — {}", reason.noted()),
                "⚠⚠⚠ REGISTER ITEM 261: this run reflected because {label}, and the one line its \
                 walk wrote about leaving `judging` must say so. Three causes with three different \
                 remedies — *look at the checkpoint the agent chose*, *look at the instruction and \
                 the dialog behind it*, *nothing, this is housekeeping* — were one arrow and \
                 nothing else, while `reflect_reason` held the answer the whole time. Walked \
                 {walk:?}",
            );
            // ⚠⚠⚠ AND EXACTLY ONE LINE CARRIES IT — the same half as register item 240's, for the
            // same reason and against a sharper trap: `reflect_reason` is a datamodel variable, so
            // a reader that took it on every pass instead of on the ENTERING edge would write *the
            // budget came round* onto every step of the restart that followed, and go on doing it
            // until the next reflection overwrote it. ⚠ The colon is what makes this the reason's
            // own heading rather than a mention of the word inside some other sentence.
            let heading = format!("{}: ", reason.word());
            assert_eq!(
                walk.iter().filter(|note| note.contains(&heading)).count(),
                1,
                "⚠⚠⚠ {label}: {heading:?} must head exactly ONE line of this walk — the step that \
                 ENTERED `reflecting`. More than one is a level being written down as a series of \
                 edges, which puts a cause on transitions it is not the cause of. Walked {walk:?}",
            );
        }
        let lines = [
            the_line("the milestone was reached", &reached_walk),
            the_line("a standing instruction fired", &instructed_walk),
            the_line("the budget came round", &budget_walk),
        ];
        let distinct: std::collections::BTreeSet<&str> = lines.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            lines.len(),
            "⚠⚠⚠ AND THE THREE LINES MUST DIFFER FROM ONE ANOTHER. Naming the cause is worth \
             nothing if two of them still render one string — which is exactly what {THE_EDGE:?} \
             did for all three before this gate existed: {lines:?}",
        );

        // ── ⚠⚠⚠ AND THE SAME REASON TWICE IS TWO EDGES, NOT ONE ──
        //
        // This is the arm that says why `because` is read ON ENTRY rather than as the DIFF that
        // register item 240 built one round earlier, and it is the only thing standing between
        // this feature and that mistake. `reflect_reason` is a level: a run whose budget comes
        // round again writes the SAME word a second time, so a reader comparing *what did this
        // change* would report the second reflection as no reflection at all — silently, and on
        // the shape a long run has most of, since a bounded loop's ordinary life is one budgeted
        // reflection after another with nothing else happening.
        //
        // ⚠ TWO REPLACEMENTS, so the run is genuinely round the loop twice: the first reflection
        // restarts the session, the fresh peer works two more turns, and the budget falls due
        // again against a `screened` that has not moved and an agent that has said nothing.
        let (workspace, pane) = crate::testing::standin_agent_reflecting(9, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: 2,
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let twice_walk = walk_of(&mut loops, &access, 2);
        let twice: Vec<&String> = twice_walk
            .iter()
            .filter(|note| note.starts_with(THE_EDGE))
            .collect();
        assert_eq!(
            twice.len(),
            2,
            "⚠ the control: this run must have reflected TWICE — one reflection cannot show that a \
             repeat is reported. Walked {twice_walk:?}",
        );
        let budgeted = format!("{THE_EDGE} — {}", ReflectReason::Budget.noted());
        for (nth, line) in twice.iter().enumerate() {
            assert_eq!(
                **line,
                budgeted,
                "⚠⚠⚠ REFLECTION {} OF 2: a run's second budgeted reflection must say `budget` \
                 exactly as its first did. A cause read as a DIFF — *has this changed since the \
                 last pass* — reports it once and leaves the rest of a long run's reflections \
                 unexplained, which is the ordinary shape of a bounded loop rather than an edge \
                 case. Walked {twice_walk:?}",
                nth + 1,
            );
        }
        assert_eq!(
            loops.screened(),
            Some(0),
            "⚠ and the control on both of them: a screened dialog would have taken the \
             instruction edge instead. Walked {twice_walk:?}",
        );
    }

    /// A machine plus the engine its datamodel lives in, and the session id that
    /// engine files those variables under.
    ///
    /// Both halves are handed back because they answer different questions: the
    /// ENGINE holds `<data>` a script datamodel evaluates, and the POLICY holds the
    /// data SCE was able to lower into typed Rust fields. A gate that reads only one
    /// of them cannot tell those two apart, which is the whole subject below.
    fn started() -> (Engine<AiLoopPolicy>, Arc<dyn IScriptEngine>, String) {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(AiLoopPolicy::new(Arc::clone(&lua)));
        engine.initialize();
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel must have opened a script session");
        (engine, lua, session)
    }

    /// Raise `event` at `engine` carrying `standing` as `_event.data.standing`, then step.
    ///
    /// ⚠⚠ THE TWO REFLECTION EDGES BOTH ADOPT THAT LIST, so a gate driving the machine by hand has to
    /// send it: `process_event` carries no data at all, and a document-level gate that used it would
    /// assign nil over the variable `priming` composes and then assert about the states anyway. **A
    /// fixture must reach a state by the door the product uses** — the driver's `Raise::carrying` is
    /// this, one layer up.
    fn reflected(engine: &mut Engine<AiLoopPolicy>, event: AiLoopEvent, standing: &str) {
        engine.raise_external(
            event,
            &serde_json::json!({"standing": standing}).to_string(),
            "",
        );
        engine.step();
    }

    /// ⚠⚠⚠ **HOW THE MACHINE TELLS ITS DRIVER WHAT TO DO — asked of the ENGINE, because the
    /// answer decides the driver's whole shape and the document cannot settle it.**
    ///
    /// `ai_loop.scxml` reads as though it were giving instructions: `priming` does
    /// `<send event="prompt.start"/>`, `restarting` does `<send event="session.replace"/>`, and
    /// seven such sends between them name every effect an outer driver has to perform. So the
    /// obvious driver is EVENT-DRIVEN: subscribe to the machine's sends, do what each one says.
    ///
    /// **That driver cannot be written, and this gate is where that was established rather than
    /// assumed.** A targetless `<send>` is W3C SCXML 6.2's *external event to SELF*: the generated
    /// code calls `raise_external_with_meta` on the machine's OWN queue, and no transition in this
    /// document listens for any of the seven — so they are raised and dropped. The one handle that
    /// looks like a subscription, `Engine::get_external_queue_handle`, is for `#_parent` sends out
    /// of `<invoke>`d CHILD machines and **mints a fresh empty queue on every call**.
    ///
    /// So the driver is **STATE-DRIVEN**: it reads `get_current_state()` and acts on where the
    /// machine IS, and the machine's own published ingress partition is what says this is the
    /// intended shape — `prompt.sent` (the driver's ANSWER) is externally drivable, while
    /// `prompt.start` (the supposed instruction) is not. The sends are documentation of intent
    /// that the compiler carries; the STATE is the contract.
    ///
    /// ⚠ Written as an assertion rather than as a comment because R376 paid for exactly this
    /// distinction one round ago: reading SCE's generated source said the opposite of what running
    /// it says. Whatever this gate reports is the thing to build against.
    #[test]
    fn the_machine_instructs_its_driver_through_its_state_not_through_its_sends() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: `start` must land in the state whose onentry sends `prompt.start`",
        );

        // ── the door that looks like a subscription ──
        let drained = engine.get_external_queue_handle();
        let seen = drained.lock().expect("the queue mutex").len();
        assert_eq!(
            seen, 0,
            "⚠⚠⚠ `prompt.start` WAS just sent, and this handle shows {seen} events. If it ever \
             shows one, the driver below is the wrong shape — it should subscribe rather than \
             read state, and every effect it performs should be keyed on a send",
        );

        // ── what the machine says a driver may tell it ──
        let ingress = AiLoopEvent::EXTERNALLY_DRIVABLE_EVENTS;
        assert!(
            ingress.contains(&AiLoopEvent::PromptSent),
            "the driver's ANSWER — *I have sent it* — must be something the machine accepts from \
             outside, or a driver cannot report having acted at all: {ingress:?}",
        );
        assert!(
            ingress.contains(&AiLoopEvent::Brief),
            "⚠⚠ and so must the one thing a caller TELLS the machine rather than reports to it. \
             `brief` is how somebody who did not edit this file says what the run is for; a \
             machine that does not accept it from outside is one whose template nothing can fill \
             in, which is exactly the state this round found it in: {ingress:?}",
        );
        assert!(
            !ingress.contains(&AiLoopEvent::PromptStart),
            "⚠⚠ and the supposed INSTRUCTION is not an ingress event, which is the machine saying \
             the same thing from the other side: nobody outside sends `prompt.start`, so nothing \
             outside is meant to receive it either. It is the STATE that instructs: {ingress:?}",
        );

        // ── and the state is a complete instruction on its own ──
        //
        // Every effect the seven sends name is recoverable from where the machine is, which is
        // what makes the state-driven driver whole rather than a degradation of the other one.
        for (state, effect) in [
            (AiLoopState::Priming, "deliver the start prompt"),
            (AiLoopState::Screening, "match the dialog against the rules"),
            (AiLoopState::AwaitingHuman, "raise a pane attention"),
            (AiLoopState::Reflecting, "write the improvements"),
            (
                AiLoopState::Restarting,
                "close the pane and open a fresh one",
            ),
        ] {
            assert_ne!(
                state,
                AiLoopState::Working,
                "each effect state must be distinguishable from the one where the driver only \
                 watches, or *{effect}* would have to be inferred from something else",
            );
        }
    }

    /// ⚠⚠⚠ **HOW `judging`'s GOAL-MET GUARD ACTUALLY READS ITS DATA** — the one fact an outer
    /// driver must send with an event, asked of the engine because getting it wrong is silent.
    ///
    /// `judging`'s first transition is `<transition event="judge" cond="_event.data.done"
    /// target="reflecting"/>`, so *did the agent say it was done* travels as event DATA rather than
    /// as a datamodel variable. Every other event on this machine's ingress surface is bare.
    ///
    /// ⚠⚠ **THE TARGET MOVED AND THE CLAIM DID NOT.** It used to be `closing`, and a reached
    /// milestone ended the run — measured against a real agent as a debt-repayment loop that paid
    /// ONE item and converged. What this gate is about is the GUARD, not the destination: that
    /// `_event.data.done` is read at all, and that `false` does not take the finished road. Both
    /// readings are pinned below, in the machine's own terms.
    ///
    /// The driver's first attempt sent `{"done": false}` as the event data and the machine went to
    /// `closing` anyway — a loop that converges on the turn its agent has NOT finished, reporting
    /// success and asking for a closing summary of work that did not happen. **The screen said the
    /// marker was absent and the machine converged regardless**, which is exactly the class of
    /// silent wrongness this project keeps paying for.
    ///
    /// So the two readings are pinned side by side here, in the machine's own terms, and whatever
    /// this gate reports is what the driver is built against.
    #[test]
    fn the_goal_met_guard_separates_a_finished_agent_from_an_unfinished_one() {
        /// Walk a fresh machine to `judging` and raise `judge` carrying `data`.
        fn judged(data: &str) -> AiLoopState {
            let (mut engine, _lua, _session) = started();
            engine.process_event(AiLoopEvent::Start);
            engine.process_event(AiLoopEvent::PromptSent);
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "the control: one completed turn is judged",
            );
            engine.raise_external(AiLoopEvent::Judge, data, "");
            engine.step();
            engine.get_current_state()
        }

        assert_eq!(
            judged("{\"done\": true}"),
            AiLoopState::Reflecting,
            "an agent that said the milestone was reached sends the loop to the state that decides \
             what the NEXT one is — not to its closing report, which would make every run exactly \
             as long as its first checkpoint",
        );
        assert_eq!(
            judged("{\"done\": false}"),
            AiLoopState::Working,
            "⚠⚠⚠ AND AN AGENT THAT DID NOT MUST TAKE ANOTHER TURN. Converging here reports a \
             milestone reached on the strength of a screen that does not say so — the driver \
             measured exactly this and the machine converged on turn one",
        );
    }

    /// ⚠⚠⚠ **THE OUTER LOOP IS A MACHINE NOW, AND THIS IS WHAT THAT BUYS.**
    ///
    /// The topology the document draws is the topology the compiler enforces. This
    /// drives the two edges the last two rounds spent themselves on — R372's *a
    /// person took the pane* and R373's *they gave it back* — through the OUTER
    /// machine rather than through prose about it.
    ///
    /// The point is not that the transitions work; SCE's own W3C suite covers that.
    /// It is that these transitions EXIST TO BE DRIVEN AT ALL. Before this round
    /// `working --turn.interrupted--> awaiting_human` was a sentence in an XML
    /// comment, and the Rust that implements the same idea was gated against its own
    /// hand-written vocabulary with nothing joining the two.
    /// ⚠⚠⚠ **AN ORDER REACHES THE ORDERS REGION AND LEAVES THE WORK WHERE IT WAS** — the smallest
    /// claim the stand-down handle rests on, and the one that tells a broken HANDLE apart from a
    /// broken GUARD.
    ///
    /// Without it, a run that failed to stand down has two indistinguishable causes: the order never
    /// arrived, or it arrived and `In()` could not see it. Those have opposite fixes.
    #[test]
    fn an_order_moves_the_orders_region_and_nothing_else() {
        let (mut engine, _lua, _session) = started();
        let before = engine.get_active_states();
        assert!(
            before.contains(&AiLoopState::Standing),
            "the control: a fresh run is resting under no orders. active = {before:?}",
        );

        engine.process_event(AiLoopEvent::StandDown);

        let after = engine.get_active_states();
        assert!(
            after.contains(&AiLoopState::StandingDown),
            "⚠⚠⚠ THE ORDER NEVER REACHED THE ORDERS REGION. Whatever `judging` then decides is about \
             a run nobody spoke to — so a handle that looked wired would do nothing, silently. \
             active = {after:?}",
        );
        assert!(
            after.contains(&AiLoopState::Idle),
            "⚠⚠⚠ AND THE WORK REGION MUST NOT HAVE MOVED. That is the entire difference between \
             standing a run down and cancelling it: the turn in flight is untouched. active = \
             {after:?}",
        );
    }

    #[test]
    fn the_outer_loop_runs_the_edges_the_last_two_rounds_built() {
        let (mut engine, _lua, _session) = started();
        // ⚠⚠⚠ THE ACTIVE SET, NOT `get_current_state()` — and this line is the probe's warning
        // arriving on the loop's own document. The flattening call answered `Idle` while the machine
        // was flat and answers `Running`, the parallel root, now that it has regions. Nothing about
        // the run changed; what changed is that a single-state reader has no stable meaning once
        // there is more than one thing going on. `OuterLoop::state` reads the WORK region by name
        // for exactly this reason, and a gate that kept asking the flattening call would be holding
        // the reader the driver just stopped using.
        assert!(
            engine.get_active_states().contains(&AiLoopState::Idle),
            "the document's `initial` inside the work region. active = {:?}",
            engine.get_active_states(),
        );

        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "a started loop primes a session before it prompts it",
        );

        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // R372: a person reached into the pane. The loop stops driving.
        engine.process_event(AiLoopEvent::TurnInterrupted);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::AwaitingHuman,
            "⚠ the edge R372 built the product half of",
        );

        // R373: they let go. The loop takes the pane back and prompts again.
        engine.process_event(AiLoopEvent::Resume);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠ and the edge R373 built the product half of. `orchestration.scxml` \
             says this one was left out because *when has somebody stopped typing* \
             had no measured answer; `Handback::WhenStill` is that answer, and this \
             is the machine that was waiting for it",
        );
    }

    /// ⚠⚠⚠ **THE OUTER BUDGET IS ENFORCED BY THE MACHINE — debt 60's third item,
    /// and the first of the three that could be paid at all.**
    ///
    /// `max_turns` and `reflect_every` have sat in the document's datamodel since
    /// `95207ad` with nothing reading them: the register recorded them as *"already
    /// in the datamodel"*, which was true and meant only that the numbers were
    /// written down. A number nothing compares against is a comment.
    ///
    /// Now `judging` is a state a compiler emitted, so the three guards in it are
    /// three branches with a priority order the DOCUMENT fixed, and this walks the
    /// whole authored budget through them:
    ///
    /// * `reflect_every` (8) fires at turns 8, 16, 24 and 32 — the loop stops to
    ///   improve itself and `reflecting` resets the counter on entry;
    /// * `max_turns` (40) ends the run — and it wins at turn 40 even though
    ///   `turns_since_reflect` has also come round, because the document orders the
    ///   `max_turns` transition FIRST. **That precedence is the assertion**: a run at
    ///   its ceiling must end rather than pay for one more restart it has no turns
    ///   left to use.
    ///
    /// ⚠ THE SEQUENCE IS COLLECTED, NOT SPOT-CHECKED. Asserting only the ending
    /// would pass for a machine that reflected on every turn, or never; asserting
    /// only a reflect point would pass for one that never stopped.
    #[test]
    fn the_outer_budget_the_document_authors_is_the_one_the_machine_enforces() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // Where the loop went after each completed turn, in order.
        let mut decisions: Vec<(u32, AiLoopState)> = Vec::new();
        let mut turn = 0_u32;
        while engine.get_current_state() == AiLoopState::Working {
            turn += 1;
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "a completed turn is judged, always: turn {turn}",
            );
            // No `_event.data.done`, so the goal-met guard is falsy and the budget
            // guards are what decide. The peer saying the done marker is a
            // different gate; this one is about the two NUMBERS.
            engine.process_event(AiLoopEvent::Judge);
            decisions.push((turn, engine.get_current_state()));

            // A reflection that finds nothing to change returns to `working`
            // without paying for a restart — the document's `reflect.none` edge,
            // and what keeps this walk going to the ceiling.
            //
            // ⚠⚠ RAISED WITH DATA, because that edge adopts the normalised
            // standing list (`_event.data.standing`) and the driver always
            // carries it. `process_event` sends none, which would assign nil over
            // the very variable `priming` composes — a fixture reaching the state
            // by a door production does not use.
            if engine.get_current_state() == AiLoopState::Reflecting {
                reflected(&mut engine, AiLoopEvent::ReflectNone, "");
            }
            assert!(turn <= 100, "the ceiling must be reachable: {decisions:?}");
        }

        let reflected: Vec<u32> = decisions
            .iter()
            .filter(|(_, state)| *state == AiLoopState::Reflecting)
            .map(|(turn, _)| *turn)
            .collect();
        assert_eq!(
            reflected,
            vec![8, 16, 24, 32],
            "`reflect_every` is 8 and the counter resets on entry to `reflecting`, \
             so the loop stops to improve itself on exactly these turns — and NOT \
             at 40, where the ceiling takes precedence: {decisions:?}",
        );
        assert_eq!(
            decisions.last(),
            Some(&(40, AiLoopState::Stopping)),
            "⚠⚠⚠ `max_turns` is 40 and its transition is written BEFORE the \
             reflect one, so the fortieth turn ends the run instead of restarting \
             a session that has no turns left to spend: {decisions:?}",
        );
        // ⚠⚠⚠ AND THE LAST TURN IS AN ACCOUNT, NOT A REPRIEVE. `stopping` asks the agent where it
        // got to (register item 201), so the budget's ending is one state further than it was —
        // and the thing that must not have changed is that the budget still ENDS the run. A
        // `stopping` with any edge back into the working cycle would come round to `judging`, take
        // this same transition, and ask for another account for ever.
        assert!(
            !engine.is_in_final_state(),
            "a run out of turns is asked for its account before it ends: {decisions:?}",
        );
        engine.process_event(AiLoopEvent::TurnDone);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Exhausted,
            "⚠⚠⚠ and the account's turn ends the run on the DOCUMENT's budget: {decisions:?}",
        );
        assert!(
            engine.is_in_final_state(),
            "and `exhausted` is a final state, not a pause",
        );
    }

    /// ⚠⚠⚠ **AN ACCOUNT THAT COULD NOT BE HAD DOES NOT CHANGE THE ENDING** — `stopping`'s whole
    /// shape, asserted against the document that decides it.
    ///
    /// # ⚠⚠⚠ Why this is the load-bearing claim and not a tidiness one
    ///
    /// The verdict is already decided by the guard that reached `stopping`: the run is out of turns.
    /// The account is a courtesy on top of it, so **every way that last turn can end has to arrive
    /// at the same place**. The alternative is not untidy, it is UNBOUNDED — `turns >= max_turns`
    /// stays true for the rest of the run, so an edge from here back into the working cycle would
    /// come round to `judging`, take that same transition, and ask for another account for ever.
    /// `closing`'s two are exactly such edges (`turn.blocked --> screening`,
    /// `turn.interrupted --> awaiting_human`), so copying `closing` is the mistake that is
    /// available, and it costs a run that never stops.
    ///
    /// ⚠⚠ THE TWO ENDINGS DRIVEN HERE ARE THE ONES A REAL PANE PRODUCES. A peer that stops to ask
    /// during its closing question and a person who takes the pane are both ordinary — R383
    /// measured the first against a live agent — and neither says anything about the run's budget.
    ///
    /// ⚠ `fail` and `cancel` are deliberately NOT folded in: those are facts about the RUN rather
    /// than about this turn, and a person cancelling here must not be told their own act was a
    /// budget running out. The gate asserts that separation too.
    #[test]
    fn an_account_that_cannot_be_had_does_not_change_the_ending() {
        /// A fresh machine sitting in `stopping`, reached the way a run reaches it: one turn
        /// judged, with the budget spent.
        fn out_of_turns() -> Engine<AiLoopPolicy> {
            let (mut engine, _lua, _session) = started();
            // ⚠ `max_turns` is the document's default here; one turn is enough only because the
            // gate below asserts the state it landed in rather than assuming it.
            engine.process_event(AiLoopEvent::Start);
            engine.process_event(AiLoopEvent::PromptSent);
            while engine.get_current_state() != AiLoopState::Stopping {
                assert_eq!(
                    engine.get_current_state(),
                    AiLoopState::Working,
                    "the walk to a spent budget goes through `working`",
                );
                engine.process_event(AiLoopEvent::TurnDone);
                engine.process_event(AiLoopEvent::Judge);
                if engine.get_current_state() == AiLoopState::Reflecting {
                    reflected(&mut engine, AiLoopEvent::ReflectNone, "");
                }
            }
            engine
        }

        for ending in [
            AiLoopEvent::TurnDone,
            AiLoopEvent::TurnBlocked,
            AiLoopEvent::TurnInterrupted,
        ] {
            let mut engine = out_of_turns();
            engine.process_event(ending);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Exhausted,
                "⚠⚠⚠ A RUN OUT OF TURNS ENDS `exhausted` HOWEVER ITS LAST QUESTION WENT. {ending:?} \
                 took it somewhere else — and every somewhere else on this document leads back to \
                 `judging`, where `turns >= max_turns` is still true and asks for another account, \
                 for ever",
            );
        }

        // ⚠⚠ AND THE RUN'S OWN TWO ENDINGS OUTRANK THE TURN'S. A person who cancels here stopped
        // the run; reporting `exhausted` would tell them their act was a budget running out.
        for (ending, landing) in [
            (AiLoopEvent::Cancel, AiLoopState::Cancelled),
            (AiLoopEvent::Fail, AiLoopState::Failed),
        ] {
            let mut engine = out_of_turns();
            engine.process_event(ending);
            assert_eq!(
                engine.get_current_state(),
                landing,
                "⚠⚠ {ending:?} is a fact about the RUN and not about the account's turn",
            );
        }
    }

    /// ⚠⚠⚠ **A STANDING INSTRUCTION IS REFLECTED ON AT THE NEXT JUDGEMENT, AND ONCE** — the
    /// `screened > screened_carried` guard, asked of the MACHINE.
    ///
    /// # ⚠⚠⚠ The two halves, and why neither alone would do
    ///
    /// * **AT THE NEXT JUDGEMENT.** Without this guard a screened loop waits out `reflect_every`
    ///   before adopting what it was told — the document ships 8 — and every turn in between asks for
    ///   the original milestone again. That is register item 148, whose live agent wrote *"루프가 매
    ///   턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절 … 진전이 없습니다"*.
    /// * ⚠⚠⚠ **AND ONCE.** `screened_carried` is set on ENTRY to `reflecting`, not on the way out, so
    ///   a reflection that changes nothing does not send the run straight back. Set on the exit
    ///   instead, `reflect.none` would return to `working`, the very next judgement would see the same
    ///   inequality, and the loop would judge no further turn for the rest of its budget. **That is a
    ///   livelock the state names cannot show**, which is why the second half is asserted here rather
    ///   than left to read as obvious.
    ///
    /// ⚠ Driven with `raise_external`, because both edges carry data — see [`reflected`].
    #[test]
    fn a_standing_instruction_is_reflected_on_at_the_next_judgement_and_once() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // ⚠ THE CONTROL: with nothing screened, a judged turn goes straight back to work. The
        // document ships `reflect_every: 8`, so nothing else can send this turn to `reflecting`.
        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "the control: an unscreened turn is not a reason to reflect, or the assertion below is \
             about the turn budget",
        );

        // The peer asks, a rule claims it, and the driver reports what it said.
        engine.process_event(AiLoopEvent::TurnBlocked);
        assert_eq!(engine.get_current_state(), AiLoopState::Screening);
        engine.raise_external(
            AiLoopEvent::ScreenMatched,
            &serde_json::json!({"text": "do it another way"}).to_string(),
            "",
        );
        engine.step();
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "a claimed dialog returns to work — the peer has just been handed its answer",
        );

        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Reflecting,
            "⚠⚠⚠ THE FIRST HALF: the judgement straight after a standing instruction fired must \
             reflect, at turn TWO of a document whose `reflect_every` is 8",
        );

        // Nothing worth changing — back to work without a restart.
        reflected(&mut engine, AiLoopEvent::ReflectNone, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);
        engine.process_event(AiLoopEvent::TurnDone);
        engine.process_event(AiLoopEvent::Judge);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠⚠⚠ THE SECOND HALF: the SAME instruction must not send the run back to `reflecting` \
             for ever. `screened_carried` is set on entry, so this judgement sees the two counts \
             equal — set on the way out instead, this loop would never judge another turn and no \
             state name would show it",
        );
    }

    /// ⚠⚠⚠ **THE AUTHORED HALF OF THE DOCUMENT SURVIVES — AND THE ROUND HAD TO RUN
    /// IT TO FIND THAT OUT, BECAUSE READING SAID THE OPPOSITE.**
    ///
    /// `ai_loop.scxml` declares `datamodel="ecmascript"`. At the pinned SCE rev
    /// there is exactly ONE [`IScriptEngine`] — `LuaEngine`; `sce-rust-runtime`'s
    /// own manifest calls QuickJS *"future"*, and SCE's build special-cases only the
    /// datamodel string `"null"`, routing every other value to whatever engine the
    /// consumer supplies. So the document's ECMAScript is evaluated by **Lua**, and
    /// the generated init strings show a PARTIAL rewrite: the object/array literal
    /// in `screen_rules` is turned into Lua table syntax (`[…]` → `{…}`, `key:` →
    /// `key =`), while `start_prompt`'s `'…' + north_star + '\n' + …` is passed
    /// through verbatim — and in Lua `+` is arithmetic, not concatenation.
    ///
    /// From that reading this gate was written to assert the prompts DO NOT arrive.
    /// **It failed, and the failure is the finding**: the composed prompt comes back
    /// whole. The engine handles the concatenation; the mismatch visible in the
    /// generated source is not a defect a caller can reach.
    ///
    /// So this asserts what is true, over the three shapes the authored half is made
    /// of, each of which a different part of the loop depends on:
    ///
    /// * `north_star` — a bare literal. The control: if this one fails, the gate is
    ///   not reading the datamodel at all and nothing below means anything.
    /// * `start_prompt` — a COMPOSED string, and **as of this round the composition
    ///   is an `<assign>` in `priming`'s `onentry` rather than a `<data expr>`**. That
    ///   is a shape this gate had never driven, and the document's own caveat says a
    ///   shape not driven here is a shape nobody has measured — so the walk to
    ///   `priming` below is the point, not a detour around it.
    /// * `screen_rules` — a LIST OF OBJECTS, the shape debt 60's `screening` is
    ///   built out of, and the only one whose syntax the codegen rewrote.
    /// * `max_turns` — a scalar, which the outer `judging` budget compares against.
    #[test]
    fn the_whole_authored_surface_crosses_into_the_datamodel() {
        let (mut engine, lua, session) = started();

        // ── the control: a bare literal crosses unharmed ──
        let north_star = lua.get_variable(&session, "north_star");
        assert!(
            matches!(&north_star, Ok(ScriptValue::String(text)) if text.contains("edit me")),
            "⚠ THE CONTROL FAILED, so nothing below means anything: a bare string \
             literal must reach the datamodel. Got {north_star:?}",
        );

        // ── the SECOND control, and it is what makes the composition below a claim
        //    about `priming` rather than about `<data>`: nothing is composed yet.
        let unprimed = lua.get_variable(&session, "start_prompt");
        assert!(
            matches!(&unprimed, Ok(ScriptValue::String(text)) if text.is_empty()),
            "⚠ a machine that has not primed must hold no composed prompt, or the walk \
             below proves nothing about where the composition happens: {unprimed:?}",
        );

        // ── a COMPOSED string, built by an `<assign expr>` on the way into `priming` ──
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: the composition runs on entry to `priming`",
        );
        let start_prompt = lua.get_variable(&session, "start_prompt");
        let Ok(ScriptValue::String(start_prompt)) = &start_prompt else {
            panic!("the prompt `priming` sends must be a composed string: {start_prompt:?}");
        };
        assert!(
            start_prompt.starts_with(COMPOSED_START_PROMPT),
            "the `+` chain must have concatenated, not added: {start_prompt:?}",
        );
        assert!(
            start_prompt.contains("Report what you did and what is left."),
            "and every clause of it must be there, not just the first: \
             {start_prompt:?}",
        );
        // ⚠⚠ AND ONE `<assign>` MUST HAVE SEEN THE ONE BEFORE IT. `done_instruction` is
        // composed first and the two working prompts end with it, so executable content
        // running out of document order would append the PREVIOUS entry's instruction —
        // correct for every entry but the first, and silent.
        assert!(
            start_prompt.trim_end().ends_with("MILESTONE REACHED"),
            "⚠⚠ `done_instruction` must have been composed BEFORE the prompt that ends \
             with it: {start_prompt:?}",
        );

        // ── THE TWO ENDING QUESTIONS: plain literals, unlike everything above, and the pair a
        //    run's account is read against ──
        //
        // ⚠⚠⚠ THEY MUST DIFFER, AND EACH MUST BE RECOGNISABLE WITHOUT THE OTHER. `closing` asks an
        // agent that GOT THERE to summarise; `stopping` asks one that ran out of turns where it got
        // to. A document that asked the same thing twice would be `stop_prompt` reduced to a copy —
        // the design decision item 201 named — and a caller reading a finished run could not tell
        // which ending it was looking at from the transcript.
        //
        // ⚠⚠ AND THIS IS WHAT HOLDS THE FIXTURES IN STEP. Every stand-in peer keys its answer on a
        // verbatim slice of one of these, and the reporting peer paints a second slice as its
        // wrapped echo. Reworded apart from here, a peer stops recognising the question, its turn
        // never ends, and the gate reports a wall clock instead of a budget — measured, four gates
        // at once, the round `stopping` was built.
        for (variable, question, echo) in [
            (
                crate::outer::Owed::End.variable(),
                crate::testing::Accounts::ForARunThatGotThere.question(),
                crate::testing::Accounts::ForARunThatGotThere.echo_slice(),
            ),
            (
                crate::outer::Owed::Stop.variable(),
                crate::testing::Accounts::ForARunThatRanOutOfTurns.question(),
                crate::testing::Accounts::ForARunThatRanOutOfTurns.echo_slice(),
            ),
        ] {
            let Ok(ScriptValue::String(prompt)) = lua.get_variable(&session, variable) else {
                panic!("`{variable}` must be a string the driver can deliver");
            };
            assert!(
                prompt.contains(question) && prompt.contains(echo),
                "⚠⚠⚠ `{variable}` no longer carries what every stand-in keys on ({question:?}) or \
                 the fragment its wrapped echo is staged from ({echo:?}). A peer that cannot \
                 recognise the question never answers it: {prompt:?}",
            );
            let other = if variable == crate::outer::Owed::End.variable() {
                crate::testing::Accounts::ForARunThatRanOutOfTurns.question()
            } else {
                crate::testing::Accounts::ForARunThatGotThere.question()
            };
            assert!(
                !prompt.contains(other),
                "⚠⚠⚠ THE TWO ENDINGS MUST ASK DISTINGUISHABLE QUESTIONS. `{variable}` also carries \
                 the OTHER ending's needle ({other:?}), so nothing reading a finished run — a \
                 fixture or a person — can tell which question was asked: {prompt:?}",
            );
        }

        // ── a LIST OF OBJECTS: the shape `screening` reads its rules out of, and
        //    the one whose SYNTAX the codegen rewrote on the way in ──
        let rules = lua.get_variable(&session, "screen_rules");
        let rules = match &rules {
            Ok(ScriptValue::Array(rules)) => rules,
            other => panic!(
                "⚠⚠ `screening` cannot be built on a datamodel that cannot hold its \
                 rules. The engine answered {other:?}",
            ),
        };
        // ⚠⚠⚠ EMPTY, AND THAT IS THE TEMPLATE'S WHOLE POINT — read from the DATAMODEL rather than
        // from the file's text, which is the half the purity gate cannot reach.
        //
        // It used to declare ONE rule, and before that a `(edit me)` placeholder. The rule was
        // right for THIS repository and wrong for a file other repositories copy: a standing
        // instruction here is answered on behalf of an author this file has never met, in a
        // language they may not read. So the rule moved to a KIND document
        // ([`crate::kind::LoopKind`]) and what it claimed moved with it — the needle assertions
        // that used to live here are now asked of the kind that ships one.
        //
        // ⚠⚠ AN EMPTY LIST IS NEUTRAL, NOT SAFE. A loop meeting an unclaimed dialog reports
        // `no_rule` and waits for somebody, and a live run once stood at exactly that for an hour
        // and died at a ceiling. **That is an argument for writing a kind, not for shipping one
        // inside the template.**
        assert!(
            rules.is_empty(),
            "⚠⚠⚠ THE TEMPLATE SHIPS NO STANDING INSTRUCTION. Whatever is here authorises — or here, \
             refuses — on behalf of every repository that copies this file, and the author of a \
             clause cannot know whose agent will read it. A rule belongs in the adopting \
             repository's own kind document. Got {rules:?}",
        );

        // ── a scalar: what the outer `judging` budget compares against ──
        //
        // ⚠ Read through the SCRIPT SESSION rather than off the policy, and not by
        // choice. SCE lowered every scalar `<data>` into a typed Rust field
        // (`max_turns: i64`, initialised to 40) AND emitted no accessor for any of
        // them — only `session_id` is `pub`. So a consumer cannot ask the machine
        // what its own budget is; the interpreter's copy is the only readable one.
        // That is what makes the guard below the ONLY way to observe the budget.
        let max_turns = lua.get_variable(&session, "max_turns");
        assert!(
            matches!(&max_turns, Ok(ScriptValue::Int(40))),
            "the authored budget must cross as a number: {max_turns:?}",
        );
    }

    /// ⚠⚠⚠ **A PERSON'S OWN LANGUAGE REACHES THE DATAMODEL BY BOTH ROUTES INTO IT** — the gate
    /// that found SCE PR-87, repurposed onto the fix rather than deleted with it.
    ///
    /// # What it measured before, and why the two-seam shape is kept
    ///
    /// `OuterLoop::brief` sends a person's prose in as event data and reads it back out. An em
    /// dash went in and `â\u{80}\u{94}` came out — the three UTF-8 bytes of U+2014, each widened
    /// into its own `char`. Read off either end that is a guess: the widening could have been the
    /// payload becoming `_event.data`, or [`IScriptEngine::get_variable`] converting a Lua value
    /// back. **Asking the SAME engine, session and variable through the TWO DIFFERENT ARRIVAL
    /// ROUTES is what made it arithmetic**: a document literal was clean and event data was not,
    /// so the converter was the only thing left. `sce-rust-lua`'s `json_to_lua_table` walked the
    /// payload with `bytes[i] as char`.
    ///
    /// Upstream's fix went further than the report asked — the Lua-source rewrite was replaced by
    /// a real JSON decoder, which also restored escapes and arrays (valid JSON containing either
    /// had been silently demoting `_event.data` to a string).
    ///
    /// So the structure stays and the verdicts flip. It is still the only place that asks both
    /// routes, and the routes are not interchangeable: **`screen_rules` in the shipped template is
    /// Korean**, so seam one is about text `screening` will read the day it is built, while seam
    /// two is about every brief a caller will ever supply.
    #[test]
    fn a_non_ascii_string_reaches_the_datamodel_by_either_route() {
        let (_engine, lua, session) = started();

        // ── seam one: a literal in a DOCUMENT, initialised by `<data expr>` ──
        //
        // ⚠⚠ IT READS THE KIND DOCUMENT NOW, and the move is the point rather than an accident of
        // refactoring. The Korean prose used to be authored in the TEMPLATE, and it left when the
        // template stopped deciding for the repositories that copy it — a reply in one author's
        // language is exactly the thing a template must not carry. The SEAM did not move: this is
        // still a literal a person wrote into a `.scxml`, initialised by `<data expr>`, read back
        // through `IScriptEngine`.
        //
        // ⚠ SAME ENGINE, deliberately — `lua` is handed to the kind rather than a fresh one built
        // beside it. The two seams exist to tell an arrival route apart from a reader, and a second
        // engine would put a third difference between them and blunt the whole comparison.
        let kind =
            crate::kind::LoopKind::debt(Arc::clone(&lua)).expect("the kind's document opens");
        let rules = kind
            .screen_rules()
            .expect("the kind's rules must be readable")
            .expect("the kind ships a rule, or seam one has no subject");
        let text = rules
            .rules()
            .first()
            .map(crate::screen::ScreenRule::text)
            .expect("the control: the kind ships at least one rule");
        assert!(
            text.starts_with("비용 무시하고"),
            "⚠⚠⚠ SEAM ONE: a non-ASCII literal AUTHORED IN A DOCUMENT does not survive the \
             datamodel. The reply a kind screens with is Korean, so the day `screening` is built it \
             would send an agent bytes nobody wrote. Got {text:?}",
        );

        // ── seam two: a string arriving as EVENT DATA, assigned by a transition ──
        //
        // `idle`'s `brief` transition is the one place this document takes a string from outside,
        // and it is the path `OuterLoop::brief` uses. Same engine, same session, same variable
        // kind — the ONLY difference from seam one is how the value got there.
        let mut engine = _engine;
        let sent = "북극성 — ship it";
        engine.raise_external(
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": sent,
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
            "",
        );
        engine.step();
        let held = lua.get_variable(&session, "north_star");
        let Ok(ScriptValue::String(held)) = &held else {
            panic!("the control: the brief must have assigned something at all: {held:?}");
        };
        assert_eq!(
            held, sent,
            "⚠⚠⚠ SEAM TWO: a non-ASCII string does not survive EVENT DATA, where the same \
             characters authored in the document (asserted above) do. So the mangling is in the \
             payload -> `_event.data` conversion and NOT in `get_variable`. Every brief a caller \
             supplies crosses this seam. This is SCE PR-87 returning",
        );
        // ⚠ THE OLD DAMAGE, DERIVED rather than pasted, kept as the NEGATIVE control. A regression
        // would not merely differ from what was sent — it would be exactly a Latin-1 widening of
        // the UTF-8 bytes, because that is what `bytes[i] as char` does. Naming the shape is what
        // stops a DIFFERENT breakage being read as this one having come back.
        let latin1_widened: String = sent.bytes().map(char::from).collect();
        assert_ne!(
            held, &latin1_widened,
            "the exact shape of PR-87 must not be reachable again: every byte its own char",
        );
        assert_ne!(
            sent,
            latin1_widened.as_str(),
            "⚠ THE CONTROL FOR THE CONTROL: the probe text must actually CONTAIN non-ASCII, or the \
             assertion above holds for a string nothing could have damaged",
        );

        // ── and the two things upstream's fix restored beyond what was reported ──
        //
        // The rewrite required JSON's grammar to coincide with Lua's. Escapes and arrays do not,
        // so valid JSON carrying either had been demoting `_event.data` to a STRING — the field
        // read back nil and no error was raised anywhere. Neither shape is used by this document
        // today, which is exactly why they get a gate: nothing else here would notice them break.
        engine.raise_external(
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": "a \"quoted\" line\nand a second one",
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
            "",
        );
        engine.step();
        assert!(
            matches!(
                lua.get_variable(&session, "north_star"),
                Ok(ScriptValue::String(ref held))
                    if held == "a \"quoted\" line\nand a second one",
            ),
            "a JSON escape must decode rather than be handed to a Lua parser: {:?}",
            lua.get_variable(&session, "north_star"),
        );
    }

    /// ⚠⚠⚠⚠ **A LOOP WHOSE PEER IS DEAD SAYS SO ON ITS FIRST PASS, IN THE DOCUMENT'S OWN WORD** —
    /// register items 323, 326, 327 and 329, and **this gate is a REPURPOSING rather than a new
    /// one**.
    ///
    /// # What it used to hold, and why it is kept
    ///
    /// It measured how far a loop could walk toward the wedge, because item 309's headline claimed
    /// the 43-hour wedge was reachable from a live `ai_loop` and nobody had checked. The answer was
    /// **259 bytes on pass 1 and then silence** — 1.5% of the 16,896-byte wall — because an
    /// orchestrator types its stimulus at the START OF EVERY STEP and a loop types once per TURN,
    /// so when the turn stops ending the typing stops with it. That measurement stands and is why
    /// item 309's headline is narrowed: the march is `Orchestrator`'s.
    ///
    /// ⚠⚠⚠ **AND IT WAS NEVER AN ALL-CLEAR, WHICH IS WHAT THIS ROUND PAID.** What the old number
    /// bounded was the BYTES. The run still sat at that dead peer reporting nothing wrong until its
    /// own clock ended it — half an hour of shipped bound, per pass — because
    /// [`INNER_SESSION_ENDS`] could not tell a dead agent from a thinking one (item 323). *Going
    /// quiet at a dead peer is not the same as noticing.*
    ///
    /// # What it asserts now
    ///
    /// **Zero bytes, on every pass, and `PeerGone` naming this pane on the FIRST one** — arrived at
    /// by the edge `Idle --PeerGone--> PeerGone`, which is the whole shape of the repair: the start
    /// prompt is delivered by the transition out of `idle`, so the refusal at the door is met
    /// before a byte moves and the DOCUMENT is told. Then it stays: `peer_gone` is final.
    ///
    /// ⚠⚠⚠ **THE ARROW IS ASSERTED AND NOT JUST THE WORD.** Before the document had this
    /// transition, the same loop over the same pane reported `Priming --PromptSent--> Working`
    /// charging `Bytes(0)` — the machine had already moved past a prompt the door refused, and then
    /// waited in `working` for an answer to a question nobody was asked. **A run can reach the
    /// right ending by the wrong walk**, and only the arrow tells them apart.
    ///
    /// # ⚠⚠ Why the barrier is declined rather than declared
    ///
    /// A loop that declares `settles(claude)` at a pane whose child is gone is refused BEFORE it
    /// types — a real answer, and a safe one, but a different story. The wedge belongs to a run
    /// already past its barrier when its peer died, so this one starts with the barrier down and
    /// its first prompt goes in exactly as a live run's would.
    ///
    /// # ⚠⚠⚠⚠ WHAT THE OLD BOUND RESTED ON, found by mutating the fixture — AND WHAT HOLDS IT NOW
    ///
    /// **The supervisor going silent about a process that is gone.** Swapping this pane's absent
    /// supervisor for one that kept answering `Idle` with a rising `seq` — a plausible cache, or a
    /// reading taken from anything other than the process table — made the same loop over the same
    /// dead pane type on passes 1, 4, 6 and 8, *the live peer's pattern exactly*, and from there it
    /// accumulated like an orchestrator. Register item 329 recorded that nothing held the
    /// condition.
    ///
    /// ⚠⚠⚠⚠ **IT IS HELD BY THE DOOR, AND DELIBERATELY NOT BY THE TURN CONTRACT.**
    /// [`Completion::ended`](crate::completion::Completion) still asks the caller's contract BEFORE
    /// `pane_eof`, and the first draft of this round had it the other way round: `Agent`'s own gate
    /// then went from `converged` with the peer's reply to `failed` with the pane, because *the
    /// peer answered and then left* is one instant where both readings are true. So a supervisor
    /// that lied could still end ONE turn here. What it can no longer do is get a byte out — the
    /// next prompt meets the refusal at [`PaneAccess`](crate::access::PaneAccess)`::inject` and
    /// this gate's own assertion is that nothing reaches the pseudoterminal on any pass. **The
    /// bound moved from a property nobody had written down to a refusal that is asserted**, and
    /// that is what pays item 329 rather than a coverage loss.
    #[test]
    fn a_loop_whose_peer_is_dead_stops_typing_instead_of_marching_to_the_wall() {
        /// What `writing_to_a_dead_pane_comes_back` measured on this host — the thing a plugin has to
        /// reach to wedge a machine. ⚠ A kernel's number; here to be COMPARED against.
        const WALL: u64 = 16_896;
        /// Passes after the first prompt has gone in. Enough that "it stopped" is not "it had not
        /// got round to it yet".
        const QUIET_PASSES: usize = 6;

        let workspace = std::sync::Arc::new(std::sync::Mutex::new(sprag_terminal::Workspace::new(
            (80, 16),
        )));
        let pane = {
            let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exit 0");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 16)
                .expect("spawn pane")
        };
        // ⚠ NO SUPERVISOR, which is what a pane with no process actually has: nothing to ask about
        // an agent that is not there. That is the reading `completion.rs` measured — `Settles` on
        // an unarmed evaluator is never satisfied — reached here through the plugin's own door.
        let access = crate::access::WorkspacePaneAccess::new(std::sync::Arc::clone(&workspace));
        let began = Instant::now();
        while access.pane_eof(pane) != Some(true) && began.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "⚠ THE FIXTURE: the child must be gone, or nothing below is about a dead peer",
        );

        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                // ⚠ SHORT, because every pass at a dead peer waits out the whole bound — which is
                // the cost this gate is otherwise made of.
                turn_within_ms: Some(200),
                ..brief_for(1_000_000)
            },
            &AiLoopSpec {
                ready_when: None,
                ..standin_spec()
            },
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        /// Which passes TYPED, and how much — the whole measurement, taken the same way from both
        /// peers below so the two answers are comparable.
        fn typing(
            loops: &mut AiLoop,
            access: &dyn PaneAccess,
            run: &RunContext,
            passes: usize,
        ) -> (Vec<(usize, u64)>, u64) {
            let mut typed_on = Vec::new();
            let mut spent = 0;
            for pass in 1..=passes {
                let step = loops
                    .step(access, run)
                    .expect("every pass must stay readable");
                if step.cost.amount() > 0 {
                    typed_on.push((pass, step.cost.amount()));
                }
                spent += step.cost.amount();
            }
            (typed_on, spent)
        }
        // ⚠⚠⚠⚠ THE VERY FIRST PASS, AND THE DOCUMENT IS WHAT SAYS SO. This gate measured 259 bytes
        // on pass 1 before any of this existed — the loop's start prompt, going into a pane nobody
        // was reading — and then seven silent passes over a run that reported nothing wrong.
        let first = loops
            .step(&access, &run)
            .expect("⚠⚠⚠ a pass at a dead peer must answer with a VERDICT rather than an error");
        assert_eq!(
            (first.verdict.clone(), first.note.as_deref()),
            (Verdict::PeerGone(pane), Some("Idle --PeerGone--> PeerGone"),),
            "⚠⚠⚠⚠ THE DOCUMENT'S OWN EDGE, ON THE FIRST PASS. The start prompt is delivered by the \
             transition out of `idle`, so this is the loop meeting the refusal at the door and \
             telling `ai_loop.scxml` rather than walking on. ⚠⚠⚠ MEASURED BEFORE the document had \
             the word: the same loop reported `Priming --PromptSent--> Working` charging \
             `Bytes(0)` — the machine had already moved PAST a prompt that never went in, and then \
             waited in `working` for an answer to a question nobody had been asked",
        );
        // ⚠⚠ AND IT STAYS THERE. `peer_gone` is FINAL, so every later pass is `Pumped::Ended` and
        // answers the same word — which is what makes *the run stops* a property of the document
        // rather than of a caller who happened to stop asking.
        let (typed_on, spent) = typing(&mut loops, &access, &run, QUIET_PASSES + 1);
        assert!(
            typed_on.is_empty() && spent == 0 && first.cost.amount() == 0,
            "⚠⚠⚠⚠ NOT A BYTE may reach a pane whose program has exited, on any pass. That is the \
             wedge itself: newline-terminated input into a dead pseudoterminal blocks FOR EVER at \
             {WALL} bytes, holding the pane's writer lock, and a blocked write cannot be \
             cancelled. Pass 1 spent {}, and the rest typed on {typed_on:?} for {spent} bytes",
            first.cost.amount(),
        );

        // ── THE CONTROL, and without it *"it typed nothing"* is indistinguishable from a gate that
        //    cannot see a loop type at all ──
        //
        // ⚠⚠⚠ THE SAME MEASUREMENT, THE SAME NUMBER OF PASSES, over a peer whose turns END. A loop
        // types once per turn, so a peer that answers is a loop that types again and again — and
        // that is what makes the silence above a fact about the DEAD peer rather than about this
        // harness or about a prompt that was never composed.
        let (live_workspace, live_pane) = standin_agent(1_000_000);
        let live = supervised(&live_workspace);
        crate::testing::started(&live, live_pane, "AGENT-READY");
        let mut alive = AiLoop::new(
            engine(),
            live_pane,
            &Brief {
                turn_within_ms: Some(2_000),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let (live_typed, live_spent) = typing(&mut alive, &live, &run, QUIET_PASSES + 2);
        assert!(
            !live_typed.is_empty() && live_spent > 0,
            "⚠⚠⚠ THE CONTROL FAILED: over a peer that ANSWERS, this loop must type. If it does not, \
             then the refusals above are this harness never getting a prompt composed rather than \
             a run declining to write into a pane nobody is reading — live {live_typed:?}",
        );
        assert!(
            live_typed.len() > 1,
            "⚠⚠ and it must type MORE THAN ONCE, or *a loop types once per turn* is not what is \
             being compared against: {live_typed:?}",
        );

        println!(
            "\n== an ai_loop at a pane whose peer is dead ==\n  dead peer: 3 passes, 0 bytes, \
             every one refused as PeerGone naming pane {}\n  live peer, {} passes: typed on \
             {live_typed:?}, {live_spent} bytes\n  it used to put 259 bytes in before going \
             quiet; the wall is {WALL} bytes\n",
            pane.0,
            QUIET_PASSES + 2,
        );
        access.lifecycle().expect("lifecycle").close(pane);
        live.lifecycle().expect("lifecycle").close(live_pane);
    }
}
