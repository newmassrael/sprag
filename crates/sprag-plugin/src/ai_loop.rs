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

use sce_rust_runtime::{IScriptEngine, StatePolicy};
use sprag_terminal::PaneId;

use crate::sm::ai_loop::AiLoopPolicy;

use crate::access::{PaneAccess, PaneError};
use crate::act::Publishes;
use crate::consent::Unanswered;
use crate::driver::Ceiling;
use crate::outer::{
    AiLoopEvent, AiLoopSpec, AiLoopState, Brief, Briefed, DoneReason, Noticed, OuterLoop, Pumped,
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
    ///
    /// ⚠⚠⚠⚠⚠ **AND IT USED TO MEAN TWO OTHER THINGS AS WELL** — register item 510, and the reason
    /// the two arms below exist. `OuterLoop::new` returned `Option`, so a document the door had
    /// REFUSED and a machine carrying no script session both arrived here as an absence and were
    /// reported with this sentence. **A reader was sent to look for a missing `<data>` by a
    /// refusal that had nothing to do with one.** The word means only what it says now, and the
    /// compiler is what keeps it that way.
    Undrivable,
    /// ⚠⚠⚠⚠⚠ **THE DOCUMENT RAISED AN ERROR IT ANSWERS NOWHERE, WHILE IT WAS BEING BUILT** —
    /// register item 510, and [`Faulted`](Self::Faulted)'s twin on the other side of the door.
    ///
    /// The pair is worth keeping apart because the *diagnosis* differs even where the repair is in
    /// the same file. [`Faulted`](Self::Faulted) is a document that answered: the machine came back
    /// in `failed` with the class named, and what to fix is the clause. This is a document that did
    /// NOT — the error was raised where no state could match it, so there is no machine to read and
    /// the document is ALSO missing the edge that should have caught it. Register item 509 is the
    /// channel for the same fact mid-run.
    ///
    /// ⚠ The payload is `crate::document::Faulted` whole, because it already carries the sentence
    /// a person acts on — which error, how many, and that the rest of that block never ran.
    Unanswered(crate::document::Faulted),
    /// ⚠⚠ **THE MACHINE CAME BACK CARRYING NO SCRIPT SESSION**, so nothing can read its datamodel —
    /// register item 510.
    ///
    /// ⚠ Not [`Undrivable`](Self::Undrivable), and the difference is which file to open: that one
    /// is a datamodel missing four strings, and this is a build with no datamodel to miss them
    /// from. The engine pinned under this build is what a reader should look at.
    Sessionless,
    /// The brief did not reach the machine, and what the machine said about it.
    ///
    /// ⚠ [`Briefed::Took`] is not representable here: this arm is only built from the other two.
    Brief(Briefed),
    /// ⚠⚠ **THE BRIEF ALLOWS NO TURN AT ALL**, so the run could only judge itself `exhausted`
    /// before its agent has answered anything.
    ///
    /// Refusing here rather than mid-run is the difference between something the caller can act on
    /// before anything happens and a run that prompts a live agent and then stops with no answer
    /// for it. ⚠ A DECLINED budget is not a budget of zero — see the reader at the refusal.
    ///
    /// # ⚠⚠⚠⚠⚠ This was `Unbuilt(AiLoopState)` until 2026-08-26 R100, and the reversal is measured
    ///
    /// It carried the state a bad brief would reach, and its own note said why: *"the variant stays
    /// a STATE rather than becoming a sentence about turn budgets … the next state this build does
    /// not serve gets the same treatment."* **That premise is gone.** Since stage 3, this driver
    /// holds no list of states at all: a state it cannot drive is one the DOCUMENT answers nothing
    /// for, discovered at RUN time and reported as [`crate::access::PaneError::Undrivable`] naming
    /// the missing `In('…')` arm. Nothing can reach a construction-time refusal for an unserved
    /// state any more, so the one arm left was never about a state — it is about a NUMBER.
    ///
    /// ⚠⚠ **AND CARRYING THE STATE COST TWO COPIES OF THE TOPOLOGY**: this construction and the
    /// `match` in `sprag-host` that read the state back to choose a sentence. A word with no
    /// payload puts the sentence where the refusal is decided and lets the compiler check that
    /// every caller answers it — which is what the state match was standing in for.
    NoTurns,
    /// ⚠⚠ **THE LOOP'S STANDING INSTRUCTIONS ARE NOT ONES THIS BUILD CAN CARRY OUT**, and which.
    ///
    /// The rules live in the document's authored half and reach the datamodel either from the file
    /// or through the [`Brief`]. A rule that claims every dialog would refuse every tool call the
    /// agent ever asks about, and one that says nothing leaves it turned down with nothing to do —
    /// so both are answered here, before a byte is typed, exactly as an unreachable state is.
    Screening(crate::outer::NotScreenable),
    /// ⚠⚠⚠⚠⚠ **THE DOCUMENT'S OWN CONTENT DID NOT EXECUTE**, and which class of error said so —
    /// register item 505.
    ///
    /// An expression the document cannot evaluate raises `error.execution` while the datamodel is
    /// being initialised; the `work` region answers it by ending the run, so the machine handed back
    /// by [`OuterLoop::new`] has already reached `failed`. Refusing HERE rather than letting the
    /// brief be turned down is the difference between *the clause you wrote did not evaluate* and
    /// *the machine would not take your brief* — the second is true and sends the reader to the
    /// wrong file.
    ///
    /// ⚠ The payload is the event's own name (`"error.execution"`), read back out of the datamodel
    /// the edge assigned it to. The class is the repair: this document's content is the author's,
    /// and a `<send>` nobody served is the host's.
    Faulted(String),
}

/// **WHY THE LOOP COULD NOT BE BUILT, IN THE DOOR'S VOCABULARY** — register item 510.
///
/// ⚠⚠⚠ A `From` rather than a closure at the one call site, and the reason is that a closure is
/// not a thing a gate can hold. This mapping is the whole of what the item bought — three refusals
/// keeping three sentences where they used to collapse into one — so it has to be assertable
/// without first breaking a document a run would have to be driven against.
impl From<crate::outer::Unopened> for NotStarted {
    fn from(why: crate::outer::Unopened) -> Self {
        match why {
            crate::outer::Unopened::Faulted(faulted) => Self::Unanswered(faulted),
            crate::outer::Unopened::Sessionless => Self::Sessionless,
            crate::outer::Unopened::Undrivable => Self::Undrivable,
        }
    }
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

/// **WHAT ONE PASS OF THE DRIVER DISCOVERED, BESIDE THE EDGE IT TOOK** — everything
/// [`AiLoop::walked`] composes after the arrow, and nothing else.
///
/// # ⚠⚠⚠ One argument because the list is what grows
///
/// Each of these arrived a round apart, each for the same reason — an edge is not its own cause,
/// its own verdict, its own evidence or its own finding — and each was a fresh parameter until there
/// were five and the signature had eight. [`Because`](crate::outer::Because)'s doc had already
/// named that shape as a debt of this crate; the sixth fact is a field here and no change to
/// `walked` at all.
///
/// ⚠ [`Default`] is derived and is the honest reading for a pass that discovered nothing: every
/// field is *this pass has nothing to say about that*, which is what [`None`] means at each of them.
#[derive(Clone, Copy, Debug, Default)]
struct Learned<'a> {
    /// The refusal this pass ARRIVED AT — never one it was already holding.
    found: Option<&'a crate::consent::Unanswered>,
    /// WHY this pass's edge was taken, for a state several edges reach with several meanings.
    because: Option<crate::outer::Because>,
    /// A record this run's agent named and nothing could read — register item 431(a).
    unreadable: Option<&'a std::path::Path>,
    /// What an independent check said about the milestone this judgement claimed — item 428.
    checked: Option<crate::outer::Checked>,
    /// Whether the turn this pass ENDED produced anything — register item 719, and [`None`] on
    /// every pass that ended no turn.
    made: Option<crate::outer::Made>,
    /// What that check said BESIDE its verdict — register item 461.
    ///
    /// ⚠ Borrowed, like [`found`](Self::found) and [`unreadable`](Self::unreadable) beside it: this
    /// struct is `Copy` so a caller cannot half-fill it, and an owned string here would take that
    /// away for a value the renderer only reads.
    explained: Option<&'a str>,
    /// WHICH READER that check was shown — register item 448, and the fact that makes its verdict
    /// appealable.
    shown: Option<crate::outer::Evidence>,
    /// What proved this pass's delivery arrived, when that is not what the run was already told —
    /// register item 434.
    witnessed: Option<crate::deliver::Witnessed>,
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
        // ⚠⚠⚠ THE SECOND REFUSAL THAT USED TO BE HERE IS GONE, and its going was a round's headline:
        // `reflect_every < max_turns` was refused because it reaches `reflecting`, and *"the
        // session-replace lifecycle behind it is registered debt"*. It is built. The gate that
        // measured the refusal's premise — that a run really does reach that state — is kept and
        // now measures the walk THROUGH it, which is the standing rule for a gate whose defect has
        // been paid.
        // ⚠⚠⚠⚠⚠ **THE REASON SURVIVES THE DOOR** — register item 510. This was
        // `.ok_or(NotStarted::Undrivable)`, which answered one sentence for three different
        // refusals; the constructor carries which one now, and each keeps its own.
        let mut inner = OuterLoop::new(script, pane, spec).map_err(NotStarted::from)?;
        // ⚠⚠⚠⚠⚠ THE DOCUMENT'S OWN CONTENT FAILED WHILE IT WAS BEING BUILT — register item 505, and
        // asked HERE because the answer is already in the machine by the time this line runs. An
        // expression that cannot be evaluated raises `error.execution` during initialisation, the
        // `work` region answers it by going to `failed`, and the brief below would then be refused
        // by a machine that has already ended — reported as `NotStarted::Brief`, which sends a
        // caller to look at the brief they wrote instead of at the clause that did not evaluate.
        // The fault is the earlier fact and it is the one worth saying.
        if let Some(error) = inner.fault() {
            return Err(NotStarted::Faulted(error));
        }
        match inner.brief(brief) {
            Briefed::Took => {}
            refused => return Err(NotStarted::Brief(refused)),
        }
        // ⚠⚠⚠⚠ ASKED AFTER THE BRIEF, AND IT USED TO BE ASKED BEFORE THE MACHINE EXISTED — register
        // item 312. The old note gave the reason honestly: *"the answer is arithmetic on the
        // caller's own numbers and needs nothing else"*. That stopped being true the moment
        // `max_turns` became declinable, because a caller who declines it has no number of their
        // own and the document's is only readable through a datamodel — which is to say, only after
        // there is a machine and a brief has been taken.
        //
        // ⚠⚠⚠ AND ASKING IN ONE PLACE IS WHAT MAKES THE TWO CASES ONE. A caller's `0` and a
        // document authoring `0` are the same run — one that can only judge itself `exhausted`
        // before its agent has answered anything — and they now meet the same refusal, carrying the
        // same sentence, instead of one being caught here and the other reaching a live agent.
        //
        // ⚠⚠ The cost of the move is a briefing round trip on a run that is about to be refused.
        // Nothing has been spoken to at this point: the pane is untouched and no agent exists yet.
        // ⚠⚠⚠ A DECLINED BUDGET IS NOT A BUDGET OF ZERO, and the two must not meet the same refusal.
        // *No turns at all* is a run that can only judge itself exhausted before its agent has
        // answered anything, which is what this refuses. *Never bounded on turns* is an author
        // saying the run ends some other way — `converged`, a guardrail, a stand-down — and it is a
        // document to obey. `None` remains the third thing: a budget no reader can make sense of.
        match inner.turn_budget() {
            Some(crate::outer::Counted::Never) => {}
            Some(crate::outer::Counted::Of(turns)) if turns >= 1 => {}
            _ => return Err(NotStarted::NoTurns),
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

    /// **WHERE THIS RUN'S REVIEWS KEEP THEIR COUNTS**, or [`None`] for a run that keeps none.
    ///
    /// ⚠⚠⚠ Here for [`consenting`](Self::consenting)'s reason exactly, and it is the same failure
    /// one field over: **a carrier nothing can observe is a carrier that can quietly drop what it
    /// carries.** Only the daemon knows its own state directory, it says so in one line
    /// (`sprag_host::plugins`), and a run built without that line looks identical from out here —
    /// it comes up configured, reviews normally, and keeps counts nobody can compare with the next
    /// run's. This is what lets a gate see the difference.
    #[must_use]
    pub fn keeping_counts_in(&self) -> Option<&std::path::Path> {
        self.inner.keeping_counts_in()
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

    /// **IS AN ORDER TO STAND DOWN STANDING, AS THE DOCUMENT HOLDS IT?** — see
    /// [`OuterLoop::standing_down`] for why the document's answer and the host's flag are different
    /// authorities.
    #[must_use]
    pub fn standing_down(&self) -> bool {
        self.inner.standing_down()
    }

    /// **WHAT THIS RUN'S MACHINE WAS HANDED AND NEVER LOOKED AT** — see [`OuterLoop::unseen`] for
    /// the three outcomes it separates, and why register item 605 needed it to exist.
    #[must_use]
    pub fn unseen(&self) -> Option<crate::sm::ai_loop::AiLoopEvent> {
        self.inner.unseen()
    }

    /// **WHAT THIS RUN TOLD ITS MACHINE THAT THE DATAMODEL COULD NOT READ** — see
    /// [`OuterLoop::unreadable_payload`] for why a run can work and mean nothing.
    #[must_use]
    pub fn unreadable_payload(&self) -> Option<crate::sm::ai_loop::AiLoopEvent> {
        self.inner.unreadable_payload()
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

    /// **WHAT THIS RUN'S OWN DOCUMENT RAISED THAT NOTHING WAS LEFT TO ANSWER** — register item 511,
    /// and the reading a LIVE run had no door to be asked for.
    ///
    /// # ⚠⚠⚠⚠⚠ Why two delegations were the whole of what was owed
    ///
    /// Register item 505 made an unanswered `error.execution` END a run that used to limp through
    /// it — a live behaviour change on an unattended loop. What that payment did NOT cover, and
    /// registered as item 511, was the READING: one real run driven end to end with this and
    /// [`fault`](Self::fault) both taken off it, because the paths a stand-in never takes are
    /// `screening` with a real dialog, `service_down`, and `reviewing` over a real transcript.
    /// **Every live gate in this tree holds an `AiLoop`, and an `AiLoop` answered neither** — the
    /// two readings existed only on the driver behind it, so the run a person can actually drive
    /// had no way to be asked. The item was owed for a week for the want of these two lines.
    ///
    /// ⚠ A LEVEL rather than an event, [`OuterLoop::swallowed`]'s stance: the machine is read each
    /// time it is asked, so a supervisor arriving mid-run gets the same answer as one who watched
    /// from the start.
    ///
    /// ⚠⚠ [`None`] is the healthy reading and what every run of today's document takes — its
    /// `error.execution` edge covers every state that runs content (register item 509), so this is
    /// the NET under that edge rather than a live path. `Some` is the day the document stopped
    /// covering itself, and the payload names the class, the count, and that the rest of the block
    /// that raised it never ran.
    #[must_use]
    pub fn swallowed(&self) -> Option<crate::document::Faulted> {
        self.inner.swallowed()
    }

    /// **WHICH ERROR OF ITS OWN PUT THIS RUN'S MACHINE IN `failed`** — [`OuterLoop::fault`] at the
    /// door, and register item 511's other half.
    ///
    /// ⚠ The COUNTERPART of [`swallowed`](Self::swallowed) rather than a second spelling of it:
    /// that counts what nobody answered, this names what the document DID answer. A run can carry
    /// one, both, or — every healthy run — neither.
    #[must_use]
    pub fn fault(&self) -> Option<String> {
        self.inner.fault()
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
    ///
    /// # ⚠⚠⚠ Why the facts arrive as one argument — and what said so
    ///
    /// [`Because`](crate::outer::Because)'s own doc predicted this: a second field beside the first
    /// makes the third state a third field *"and the walk that composes them a longer and longer
    /// list of `did this one happen` — the flat driver this crate already owes for."* The fifth fact
    /// (register item 434) made the argument list EIGHT, which is where `clippy` stopped it — so the
    /// debt that doc registered is paid here rather than silenced with an `allow`. A sixth fact is
    /// now a field on [`Learned`] and no change to this signature at all.
    fn walked(
        from: AiLoopState,
        raised: AiLoopEvent,
        to: AiLoopState,
        learned: Learned<'_>,
    ) -> String {
        let Learned {
            found,
            because,
            unreadable,
            checked,
            made,
            explained,
            shown,
            witnessed,
        } = learned;
        let mut note = if raised == AiLoopEvent::Null {
            format!("{from:?}: looked, nothing had happened")
        } else {
            format!("{from:?} --{raised:?}--> {to:?}")
        };
        if let Some(reason) = because {
            note = format!("{note} — {}", reason.noted());
        }
        // ⚠⚠⚠⚠⚠ **WHAT THE TURN THIS EDGE ENDED ACTUALLY PRODUCED** — register item 719, straight
        // after the cause because it is about the act this edge REPORTS, exactly where the verdict
        // below sits for the judgement the next edge makes. The two never land on one line: a turn
        // ends on `turn.done` and a claim is checked on `judge`.
        //
        // ⚠⚠ SAID ON EVERY MEASURED TURN AND NOT ONLY ON THE EMPTY ONE, which is the rule three
        // clauses below it are already written on: telling two facts apart by the ABSENCE of a
        // sentence is the reading this workspace has burned wire numbers over, and a person
        // scanning a run for the moment it stopped getting anywhere needs the productive turns to
        // say so too. ⚠ The unmeasured answer says nothing, and [`Made::describe`] holds why.
        if let Some(outcome) = made.and_then(crate::outer::Made::describe) {
            note = format!("{note} — {outcome}");
        }
        // ⚠⚠⚠⚠ THE CLAIM'S VERDICT COMES STRAIGHT AFTER THE CAUSE, and it is APPENDED rather than
        // substituted — register item 428, learned from three neighbouring gates in one run. The
        // milestone edge's cause is *the agent said the milestone was reached*, which is true and is
        // exactly the claim in question; a verdict that replaced it would drop one true thing to
        // make room for another. Both, in a fixed order, is this function's own rule.
        if let Some(verdict) = checked {
            note = format!("{note} — {}", verdict.describe());
            // ⚠⚠⚠⚠⚠ AND WHAT THE CHECKER SAID IN ITS OWN WORDS, straight after the sentence this
            // crate writes about it — register item 461. **Quoted, and attributed**: everything
            // else on this line is the product speaking, and this is a model's prose arriving in a
            // report a person will act on, so a reader has to be able to see where one stops and
            // the other starts. ⚠ INSIDE the verdict's arm, because a reason with no verdict beside
            // it would be a sentence about a judgement this line never says was made.
            if let Some(words) = explained {
                note = format!("{note} — it said: {words:?}");
            }
            // ⚠⚠⚠⚠⚠ AND WHAT IT WAS LOOKING AT — register item 448, and the line that makes the two
            // above worth reading. A live run was refused EIGHT times with an identical sentence,
            // and the question nobody could answer from the outside was not *what did it decide*
            // or *why did it say so* but **was it shown anything at all**: `turn_produced` falls
            // back to the pane, item 441 measured that reader going permanently blind against a
            // repainting agent, and a real checker handed an empty artifact answers a clean `NO`.
            //
            // ⚠⚠⚠ INSIDE the verdict's arm, on `explained`'s rule exactly: an instrument named
            // where no judgement is claimed would describe a reading that never happened. ⚠ It is
            // said on BOTH verdicts, never only on the refusal — telling two facts apart by the
            // absence of a sentence is the reading this workspace has burned wire numbers over.
            if let Some(reader) = shown {
                note = format!("{note} — it was shown {}", reader.named());
            }
        }
        // ⚠⚠⚠⚠ **WHAT PROVED THE PROMPT THIS EDGE DELIVERED ACTUALLY ARRIVED** — register item 434,
        // straight after the cause and the verdict because it is about the act this edge PERFORMED,
        // where the two facts below are about what the pass ran into and what it could not measure.
        //
        // ⚠⚠⚠ IT IS SAID ON EVERY DELIVERY AND NOT ONLY ON THE INTERESTING ONE. Publishing only
        // `Account` would be telling two facts apart by the ABSENCE of a sentence — the reading this
        // workspace has burned wire numbers over — and the ordinary answer is one line a reader
        // skips, where a missing one is a question they cannot answer without the transcript.
        if let Some(evidence) = witnessed {
            note = format!("{note} — {}", evidence.noted());
        }
        if let Some(unanswered) = found {
            note = format!("{note} — {}", unanswered.noted());
        }
        // ⚠⚠⚠⚠ AND THE THIRD FACT — register item 431(a) — LAST, because it is about the run's own
        // instruments rather than about the edge: everything above says what the loop did, and this
        // says what it could not measure while doing it.
        //
        // ⚠⚠ IT IS SAID ONCE PER BROKEN RECORD, which is the caller's `take` and not this function's
        // business — see [`OuterLoop::took_unaccountable`]. A sentence on every step would fill a
        // bounded journal with one fact, which is measured (item 277) rather than feared.
        if let Some(record) = unreadable {
            note = format!(
                "{note} — its agent states it is writing {} and nothing here could read that file, \
                 so this run's context, cold and floor are zeros it could not measure rather than a \
                 session that has spent nothing",
                record.display(),
            );
        }
        note
    }

    // ⚠⚠⚠⚠⚠ **`is_final` STOOD HERE AND IT IS GONE** — register item 470, stage 3. It named all
    // twenty-eight states of the document in one exhaustive `match` to answer *is this an ending*,
    // which is the THIRD place that sentence was said: the document's own `<final>` elements say
    // it, `OuterLoop::pumping` asks the engine, and this said it again in Rust off a state name.
    // `OuterLoop::finished` is the one reader now, and its doc carries the arrangement the answer
    // rests on.
    //
    // ⚠⚠ THE COMPILER RATCHET THIS TRADED AWAY, stated rather than hidden: the match was
    // exhaustive, so an eighth final broke this build. Nothing breaks now — a new `<final>` is
    // answered correctly by the engine the moment the document gains it, which is the point, and
    // the price is that a final placed INSIDE the `<parallel>` would be silently mis-answered.
    // That is what `finished`'s doc names and what
    // `every_ending_this_document_declares_sits_outside_the_parallel` measures, in this file's own
    // proving module.

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
        Self::account_of(self.inner.noticed())
    }

    /// [`left_behind`](Self::left_behind)'s words, as a function of the notice ALONE.
    ///
    /// ⚠⚠⚠ **SPLIT OUT SO THE SENTENCES CAN BE ASKED FOR DIRECTLY** — register item 724. The notice
    /// is the driver's private field one module over, so a gate on the WORDS had either to widen
    /// that field's visibility for a test or to drive a whole run to reach a format string; the
    /// first trades production encapsulation for a gate and the second measures the clock and calls
    /// it the report. Nothing here reads `self`, which is the point: these sentences are a pure
    /// function of what was noticed, and now they are written as one.
    fn account_of(noticed: Option<&Noticed>) -> Option<String> {
        match noticed {
            Some(Noticed::Asking(unanswered)) => Some(format!(
                " — no account: the agent stopped to ask ({unanswered:?}) and the question is still \
                 on the pane, unanswered by this run"
            )),
            Some(Noticed::Interrupted(who)) => Some(format!(
                " — no account: somebody took the pane ({who:?}) before the agent answered"
            )),
            // ⚠⚠⚠ THE ONE ENDING WITH NOTHING ON THE PANE TO POINT AT — register item 458. The two
            // above leave a reader something to go and look at; this leaves a screen that has not
            // moved, so the sentence has to carry the whole fact or the run's account says a turn
            // simply stopped. Both numbers, for `Noticed::Silent`'s reason.
            Some(Noticed::Silent(silence)) => Some(format!(
                " — no account: nothing spoke for the pane for {:?}, after {} report(s), so the \
                 turn was never seen to end",
                silence.within, silence.reports
            )),
            // ⚠⚠⚠⚠ AND THE ONE WHERE NOTHING IS WRONG WITH THE RUN AT ALL — register item 447. The
            // agent's work is intact and its session is where it left it; what ran out is patience
            // with an upstream service. A reader told only *blocked* would go looking for a
            // question, and there is none.
            // ⚠⚠⚠ AND WHICH OUTAGE IT WAS, since register item 724 gave the two doors budgets that
            // differ by a factor of six. The count alone stopped being readable that day: twelve is
            // twice past one ceiling and a third of the way to the other, and the reader this
            // sentence exists for has no other page to check.
            Some(Noticed::ServiceDown {
                retried,
                waited,
                resumes,
            }) => Some(format!(
                " — no account: {} and this run had already waited it out {retried} time(s), the \
                 last for {waited:?}, so nothing was asked",
                if *resumes {
                    "the peer said it was continuing on its own"
                } else {
                    "the peer's service was down"
                }
            )),
            _ => None,
        }
    }

    /// **ASK THE DOCUMENT WHAT IT PUBLISHED AND BUILD THE VERDICT FOR IT** — the one door into
    /// [`Self::ended`], and the only place `state` and the published word meet.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a finished machine that published nothing is REPORTED, never guessed
    ///
    /// Register item 470, stage 3. Until this round the verdict came off `state`'s own name, so
    /// this case could not arise: every state had an arm, including the twenty-one that are not
    /// endings at all. Now the ENGINE says the machine is over and the DOCUMENT says what to call
    /// it, and a `<final>` that declares no `end.publish` is a gap between those two answers.
    ///
    /// ⚠⚠ That gap is exactly the silence [`crate::act`] exists to end — *an act that quietly does
    /// nothing is indistinguishable from one that worked* — so it ends the run with the state's own
    /// name in the sentence, which is what sends a reader to the `<final>` that is missing its
    /// block. Defaulting to any of the seven words would publish some other ending's verdict for a
    /// run that reached this one.
    ///
    /// # Errors
    ///
    /// [`PaneError::Undrivable`] for an ending the document did not publish, and for whatever
    /// [`Self::ended`] itself refuses.
    fn ending(&self, state: AiLoopState, spent: u64, note: String) -> Result<Step, PaneError> {
        let Some(publishes) = self.inner.published() else {
            return Err(PaneError::Undrivable(format!(
                "the machine is finished and its document published no ending: {state:?} declares \
                 no `<send type=\"x-sprag-host\" event=\"end.publish\">` on its entry, or the word \
                 it carried was refused. The run is over and nothing here can say what to call it, \
                 and naming one of the seven would report some other ending's verdict. The pass \
                 that ended the run: {note}"
            )));
        };
        self.ended(publishes, spent, note)
    }

    /// The verdict for a run whose document has published an ending — see [`Publishes`].
    ///
    /// # ⚠⚠⚠⚠⚠ The WORD is the document's and the PAYLOAD is this run's, which is item 470's line
    ///
    /// Register item 470, stage 3, fourth match. This chose the verdict from a `match` over all
    /// twenty-eight states of `ai_loop.scxml` — seven arms naming an ending and twenty-one written
    /// out to say *not an ending* so the match stayed exhaustive without a wildcard — which is a
    /// second copy of the topology, decided in Rust, keyed on ids nothing here parses. Every
    /// `<final>` now declares its own word on its `<onentry>` and [`OuterLoop::published`] is where
    /// it arrives.
    ///
    /// ⚠⚠ What did NOT move, and must not: which ceiling fell, what question was left on the pane,
    /// which pane the dead peer had. Those are facts about the RUN, latched by the driver as it
    /// went, and building a verdict out of them is an EFFECT. The match below is over a vocabulary
    /// of seven outcomes rather than over a topology of twenty-eight states.
    ///
    /// # Errors
    ///
    /// [`PaneError::Undrivable`] for the document's `failed`, carrying the clause the driver
    /// recorded when it raised `fail`.
    fn ended(&self, publishes: Publishes, spent: u64, mut note: String) -> Result<Step, PaneError> {
        let verdict = match publishes {
            // The agent said the word, `closing` got its report, and the report landed.
            Publishes::Converged => Verdict::Converged,
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
            Publishes::Exhausted => {
                if let Some(unfinished) = self.left_behind() {
                    note.push_str(&unfinished);
                }
                Verdict::Exhausted(self.inner.stopped_short_by().unwrap_or(Ceiling::Turns))
            }
            // ⚠⚠⚠⚠ REACHED FROM `awaiting_human` BY `unattended`, WHICH `attend` NOW PRODUCES —
            // this comment said *"which nothing produces yet (registered debt)"* for as long as
            // that was true and outlived it, which is item 437's class caught in its own file.
            //
            // ⚠⚠⚠⚠⚠ AND `awaiting_human` HAS THREE CAUSES THAT END HERE AS ONE WORD, so the SENTENCE
            // is where they are told apart — a question nobody could answer (`Unanswered`), a peer
            // that stopped speaking (register item 458), and an upstream service that never came
            // back (item 447). `blocked` is the honest verdict for all three: the run stopped and a
            // person is what it needs. What is NOT honest is `asking()`'s `Unanswered::unreadable`
            // standing in for the other two, so the note carries what the verdict cannot.
            Publishes::Blocked => {
                if let Some(unfinished) = self.left_behind() {
                    note.push_str(&unfinished);
                }
                Verdict::Blocked(self.asking())
            }
            // ⚠⚠⚠⚠⚠ THE ONE ENDING THE `orders` REGION REACHES ON ITS OWN — register item 534.
            // A person said *wait, let me look*, and did not come back inside the document's
            // `hold_within_ms`. It is beside `blocked` here because the two are the pair a reader
            // most needs told apart: both are *a person is what this run needs*, and only one of
            // them means a person was never there.
            //
            // ⚠⚠⚠ THE UNFINISHED WORK IS APPENDED, on `blocked`'s and `exhausted`'s terms and for a
            // sharper version of their reason. A held run stopped mid-goal by definition — the
            // hold is *between turns* — so whatever the last judged turn left behind is the whole
            // of what the person who let go needs to read before deciding whether to start again.
            Publishes::Abandoned => {
                if let Some(unfinished) = self.left_behind() {
                    note.push_str(&unfinished);
                }
                Verdict::Abandoned
            }
            // ⚠⚠⚠ `cancel` IS RAISED ONLY WHEN THE RUN ITSELF HAS ENDED — `watch` answers it for
            // `Reached::RunEnded` and `Over::RunEnded`, both of which mean this run's context was
            // cancelled or its deadline passed. Both facts are MONOTONE, so the Driver's own
            // `ended_from_outside` is guaranteed to fire at the very next loop top and end the run
            // with the word for whichever it was. Reporting `Continue` here is therefore not a
            // stall: it hands the ending to the one authority that can tell a person's stop from a
            // clock running out, which is a distinction this plugin cannot make and must not guess.
            Publishes::Cancelled => Verdict::Continue,
            // ⚠⚠⚠⚠⚠ THE DOCUMENT'S OWN CONTENT FAILED, AND THAT IS THE FIRST THING TO SAY —
            // register item 505. Asked before the notice below because the two are different
            // authorities and only one of them can be right about this run: `fault` is written by
            // the machine's own `error.execution` edge, so a non-empty one means the ENGINE raised
            // the event and the DOCUMENT answered it. Nothing the driver noticed earlier — a
            // question on the pane, a peer that went quiet — caused that, and reporting the notice
            // instead would send a reader to look at the pane over a clause that did not execute.
            //
            // ⚠⚠⚠⚠⚠ AND IT CARRIES THE PASS'S OWN LINE, WHICH IS MEASURED RATHER THAN TIDY. The
            // first draft of this arm said *"the walk's last arrow names the state"* and the walk
            // does not: a failing step returns `Err`, so the Driver records the FAILURE and the
            // note this function was handed is dropped on the floor. Worse, the arrow would not
            // have named the error either — `raised` is what the DRIVER sent in (`Judge`), because
            // the engine's own `error.execution` never reaches this driver as an event. Measured:
            // the journal's last line was `Working --TurnDone--> Judging` and the state the guard
            // failed in appeared nowhere. So the line travels IN the sentence, which is the one
            // channel a failed run has.
            Publishes::Failed => {
                // ⚠⚠⚠⚠ INSIDE THIS ARM AND NOT BESIDE IT, and item 470's ratchet is what said so:
                // a guarded second `AiLoopState::Failed` arm is a twelfth state-keyed site in the
                // driver, and the gate counted it the moment it existed. It is the right answer
                // rather than an appeasement — `failed` has ONE renderer, and which fact it reads
                // first is that renderer's own ordering, not a second decision about the state.
                if let Some(error) = self.inner.fault() {
                    return Err(PaneError::Undrivable(format!(
                        "its own content raised {error} and this document answers that by stopping \
                         — a failed expression, a guard that could not be evaluated or a `<send>` \
                         naming a type nobody serves, and W3C SCXML 3.8 abandons the rest of the \
                         block it was in. The pass that ended the run: {note}"
                    )));
                }
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
                    // ⚠⚠⚠⚠⚠ THE PEER WOULD NOT TAKE A QUESTION IN EITHER SESSION — register item
                    // 446. The first refusal bought a replacement (the document says so in
                    // `restart_reason`); this is the replacement refusing too, which no further
                    // restart reaches. Measured on a live run: the text lands on the pane, the
                    // submit never becomes a question, and Enter, `Ctrl-C` and an interrupt all
                    // leave the draft standing while ordinary typed text in the same pane submits.
                    // ⛔⛔⛔⛔⛔ AND THE OTHER WAY IN IS THE OPPOSITE FINDING — register item 719.
                    // The arm above says two SESSIONS refused and nobody can say the text is why;
                    // this one says the TEXT is why, because these exact bytes were refused, bought
                    // a replacement, and were typed into the fresh session verbatim. The remedies
                    // differ completely — go and look at that pane, against go and shorten that
                    // brief — so they are two sentences and not one with a clause.
                    //
                    // ⚠ **UNMEASURED, STATED**: no gate drives a run to `failed` and reads either
                    // of these sentences, and the arm below has never had one. What IS gated is
                    // the ROUTING that picks between them — the driver's answer
                    // (`the_text_a_refusal_cost_a_session_outlives_the_session_it_bought`) and the
                    // document's (`the_bound_on_a_refused_prompt_is_spent_and_returned_by_every_
                    // brief_that_lands`) — because `noticed` is `OuterLoop`'s private slot and a
                    // fixture that could set it would be product surface built for a `format!`.
                    Some(Noticed::Unasked {
                        attempts,
                        written,
                        retyped: crate::outer::Retyped::Again(bytes),
                    }) => format!(
                        "it delivered the same {bytes} bytes of text that had already cost it a \
                         session: {written} bytes went on the pane, it pressed {attempts} time(s), \
                         and the composer would not take this text the second time either. \
                         Replacing the session is the only recovery this loop has for a question \
                         that was never asked, and it has now been spent on these exact bytes and \
                         changed nothing — so what is left is the PROMPT. Shorten it, or split it: \
                         a brief that a composer folds away is one nobody can submit, whichever \
                         session it is typed into"
                    ),
                    Some(Noticed::Unasked {
                        attempts,
                        written,
                        retyped: crate::outer::Retyped::First,
                    }) => format!(
                        "it put {written} bytes on the pane and pressed {attempts} time(s), and \
                         neither the session it started with nor the one it opened to replace it \
                         ever reported being asked — a peer that will not take a question is not \
                         something another restart reaches, so this is a person's to look at"
                    ),
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
            Publishes::PeerGone => Verdict::PeerGone(self.inner.pane()),
            // ⚠⚠⚠⚠⚠ **TWENTY-ONE ARMS STOOD BELOW THIS ONE AND ALL TWENTY-ONE ARE GONE** — register
            // item 470, stage 3, fourth match. They named every state of `ai_loop.scxml` that is
            // NOT an ending — the thirteen the work region drives, the regions and orders — written
            // out to say `Continue` so the match stayed exhaustive without a wildcard. Not one of
            // them was a decision: they answered *what verdict does a state that is not an ending
            // publish*, which is a question this function is never asked any more.
            //
            // ⚠⚠ THEY WERE NOT DEAD CODE EITHER, AND THAT IS WHY THIS IS A REAL MOVE RATHER THAN A
            // DELETION. They were the cost of keying the answer on a STATE: any state at all could
            // be named here, so every state had to be. Keyed on [`Publishes`] the question cannot
            // be asked about a non-ending — there is no word for one — so the arms do not need
            // answering, they need not to exist. The reader bug they guarded against is now caught
            // one level out, where the caller finds a finished machine whose document published
            // nothing and reports that rather than guessing.
        };
        // ⚠⚠⚠⚠⚠ AND WHAT NOTHING ANSWERED REACHES A PERSON HERE — register item 505's residue, on
        // the one channel that survives the run. The document's `error.execution` edge covers the
        // states it is active in; a CASCADE — a handler that fails the same way every time — is the
        // failure that adding the edge created, and an error raised where no state of the document
        // is left to match it has nobody to answer it. Both are silent everywhere else: W3C SCXML
        // 3.12.2 drops the first, and the engine cuts the second without the configuration ever
        // moving. The note is retained in the run's journal and published with it.
        //
        // ⚠⚠⚠⚠ MEASURED FIRING, which is the only reason it is trusted. With the region's edge
        // deleted (mutation, this round) the run came back CONVERGED with three errors swallowed —
        // a broken `max_turns` guard reporting success — and this note is what said so, in the
        // journal, on the converging step: *"it raised error.execution and answers no error at all
        // … (3 in total)"*. ⚠ With the edge in place nothing in today's document can reach it: every
        // state that runs content is inside the region, so this is the NET under that edge rather
        // than a live path, and the day a document stops covering itself it is what speaks.
        if let Some(swallowed) = self.inner.swallowed() {
            note.push_str(&format!(
                " — ⚠ SWALLOWED BY THIS RUN'S OWN DOCUMENT: {swallowed}, and no state of it was \
                 left to answer that"
            ));
        }
        Ok(Step::new(Cost::Bytes(spent), verdict).noting(note))
    }

    /// **A MACHINE SITTING IN A STATE ITS OWN DOCUMENT ASKED NOTHING FOR** — one answer, and it
    /// ends the run.
    ///
    /// # ⚠⚠⚠ Why it ENDS the run instead of pumping again
    ///
    /// [`Pumped::Unbuilt`] is advisory — *"a caller that ignores it pumps again"* — and a caller
    /// that does is a loop watching a state nothing will ever move it out of, until a guardrail
    /// bites and reports `exhausted — iterations` about a run that took no turn. That is the
    /// registered cost of an advisory answer, and this is where it is paid: the run stops, and the
    /// word it stops on is the one whose remedy is real.
    ///
    /// # ⚠⚠⚠⚠⚠ TWO ARMS STOOD HERE AND BOTH ARE GONE — register item 470, stage 3's last
    ///
    /// They were keyed on `AiLoopState::AwaitingHuman` and decided a verdict for it: *a person took
    /// the pane* and *the peer is asking and nothing got the run past it*. Both were written when
    /// this driver had no act for that state. `attend` was then built, so the document answers its
    /// `pass` — and the arms could not fire.
    ///
    /// ⚠⚠ **THAT WAS AN ARGUMENT UNTIL THIS ROUND, AND THIS REGISTER HAS BEEN BITTEN BY AN
    /// UNREACHABILITY ARGUMENT THAT AGED.** So it is measured instead:
    /// `every_driven_state_says_what_a_pass_of_it_is_for` drives the document to each state, raises
    /// `pass`, and reads what this host was handed — `awaiting_human` among them.
    ///
    /// ⚠⚠⚠ **AND WHAT THEY WERE IS WORSE THAN DEAD CODE.** If a document ever DID drop that state's
    /// `pass` arm, those arms would have substituted a DRIVER decision for the line an author
    /// forgot — silently, with a run that looked driven. The sentence below names the missing line
    /// instead, which is the one repair a person can act on.
    ///
    /// ⚠ The COST those arms charged (`Cost::Bytes` of what a refusal typed) is not lost with them:
    /// the roads that really report `Blocked` and `TakenOver` are [`Pumped::NotReady`]'s own arms,
    /// which is where a dialog this run met is charged.
    fn unbuilt(&self, state: AiLoopState) -> PaneError {
        // ⚠⚠⚠⚠⚠ THE SENTENCE CHANGED WITH REGISTER ITEM 470's STAGE 3, because what makes a state
        // undrivable did. It used to be a state the DRIVER had no arm for, and the repair it named
        // was this build's own gap. Now the driver has no list of states at all: it asks
        // `ai_loop.scxml` what a pass is for, and the states it cannot drive are the ones that
        // answer nothing. So the file to open is the DOCUMENT, and the line to look for is the one
        // that is missing.
        PaneError::Undrivable(format!(
            "it reached {state:?}, and this run's document asked for no act on the pass that \
             looked at it — `ai_loop.scxml`'s `work` region answers `pass` with a `<send \
             type=\"x-sprag-host\" event=\"pass.do\">` for every state a run can be driven in, and \
             there is no `In('…')` arm for this one"
        ))
    }
}

impl Plugin for AiLoop {
    /// ⚠⚠⚠⚠⚠ **WHERE THIS LOOP IS, IN THE DOCUMENT'S OWN WORD** — register item 543.
    ///
    /// `get_state_name` is GENERATED by SCE from `ai_loop.scxml`, so the word handed out here is
    /// the state's `id` as written in the document and **cannot drift from it**. A hand-written
    /// match over the twenty-eight variants was the obvious way to do this and would have been a
    /// second spelling of the document, ageing quietly the first time a state was renamed — this
    /// crate's own recorded failure shape. The product already had the answer; asking beat building.
    ///
    /// ⚠⚠ Which is also why the word is safe to PERSIST beside
    /// [`STATECHARTS_FINGERPRINT`](crate::STATECHARTS_FINGERPRINT): both come from the same
    /// compiled document, so a record carrying the pair can be checked rather than trusted.
    fn at(&self) -> Option<&'static str> {
        Some(AiLoopPolicy::get_state_name(self.state()))
    }

    /// ⚠⚠ THE WHOLE PLACE, beside the word above and for the reason the trait gives: `at` answers a
    /// person and this answers an engine. Register item 543.
    ///
    /// ⚠ [`None`] for a place that cannot be written down — a datamodel holding a value no flat
    /// list of words can carry. The answer is then the honest one a run whose machine was never
    /// saved already gets: no place, and a restart reports it `interrupted` rather than resuming it
    /// missing what it knew.
    fn place(&self) -> Option<Vec<String>> {
        self.inner.configuration().in_words()
    }

    /// ⚠⚠⚠⚠⚠ **AND BACK AGAIN — the one plugin in this crate that can be put where it was.**
    /// Register item 543's fourth brick: the words a run log carried become a placed machine here.
    ///
    /// Two refusals and they are kept apart on the trait's own terms: `from_words` answers *these
    /// are not my document's words* (a promotion changed the `.scxml`, which is item 544's ordinary
    /// case), and the engine answers *these words are mine and this is not a place I can be in* —
    /// which can only be this build having written a record its own engine rejects.
    ///
    /// ⚠ Nothing is stepped and nothing is entered: `OuterLoop::resume_at` is `enter_at`, whose
    /// whole contract is that `<onentry>` does not re-fire. A resume that re-typed its prompts
    /// would be a second run wearing the first one's id.
    fn resume_at(&mut self, place: &[String]) -> crate::plugin::Resumption {
        let Some(place) = crate::outer::LoopPlace::from_words(place) else {
            return crate::plugin::Resumption::NotThisDocument;
        };
        match self.inner.resume_at(&place) {
            Ok(()) => crate::plugin::Resumption::Placed,
            Err(why) => crate::plugin::Resumption::Refused(why.in_words()),
        }
    }

    /// ⚠ DELEGATED and never re-counted here — register item 591. The driver that puts the prompts
    /// in is the only thing that sees what proved each one arrived, so a second tally at this layer
    /// would be a number that agrees with the first until the day it does not.
    fn deliveries(&self) -> crate::plugin::Deliveries {
        self.inner.deliveries()
    }

    /// ⚠ DELEGATED for `deliveries`' reason — register item 601. The driver that puts the claim to
    /// a checker is the only thing that sees what came back, so a tally at this layer would be a
    /// second authority on one fact.
    fn checks(&self) -> crate::plugin::Checks {
        self.inner.checks()
    }

    /// ⚠ DELEGATED for `deliveries`' reason — register item 719. The driver that put the brief in
    /// is the only thing that read it back out of the datamodel, and a size measured at this layer
    /// would be measuring the REQUEST rather than what the machine holds — a second authority on
    /// one quantity, and the one that cannot see a crossing which mangled the text.
    fn briefed(&self) -> Option<crate::Briefing> {
        self.inner.briefed()
    }

    /// ⚠⚠⚠⚠⚠ **BOTH, AND THIS IS THE ONLY PLUGIN THAT MAY SAY SO** — register items 539 and 597.
    ///
    /// `OuterLoop::pump` is the single reader of `RunContext::held` and `RunContext::stood_down` in
    /// this workspace, and two standing ratchets count exactly that. So this answer is not a claim
    /// about intent: it is the same measurement those ratchets make, said in the place the host can
    /// ask before it accepts an order.
    ///
    /// ⚠⚠ **NO `_` ARM.** A third [`StandingOrder`](crate::plugin::StandingOrder) must fail to
    /// compile here rather than inherit a silent `false` from a plugin that would in fact be the
    /// one expected to read it.
    fn honours(&self, order: crate::plugin::StandingOrder) -> bool {
        match order {
            crate::plugin::StandingOrder::Hold | crate::plugin::StandingOrder::StandDown => true,
        }
    }

    /// ONE PUMP of the machine, reported in the substrate's own terms.
    ///
    /// ⚠⚠ A MOVE INTO A FINAL STATE IS JUDGED IN THE SAME STEP THAT MADE IT, never on the pump
    /// after. The Driver checks its ceilings after every unconverged step, so a loop that reported
    /// `Continue` on the step that reached `converged` would be told it had run out of iterations
    /// on the very step that finished the work — *"a step that saw the goal SAW IT"*, which is the
    /// Driver's own rule read from this side.
    /// **EVERY TRANSITION THE LAST PASS TOOK** — see [`crate::plugin::Plugin::walked`].
    ///
    /// ⚠⚠ Read straight off the loop rather than threaded through `Pumped::Moved`. `pump` empties
    /// the slot at the top of every pass, so what is in it belongs to the pass that just ran — and
    /// a field on `Pumped` would put the same fact at eight construction sites, which is the
    /// "every exit remembering" shape this crate keeps paying for.
    fn walked(&self) -> Vec<crate::plugin::Edge> {
        self.inner.walked().to_vec()
    }

    /// **HOW MANY TURNS THIS RUN BANKED** — see [`crate::plugin::Plugin::banked`].
    ///
    /// ⚠⚠⚠⚠⚠ **IT IS THE DOCUMENT'S OWN COUNTER AND NOT A TALLY KEPT HERE.** `ai_loop.scxml`
    /// raises `turns + 1` on the two `turn.done` edges and nowhere else — the exact moment a turn
    /// is over and its account is in hand — so this is the same number `max_turns` is compared
    /// against. A second count out here would be a second authority on the one quantity a run's
    /// ending is decided by, which is register item 445's whole argument.
    ///
    /// ⚠⚠ **READABLE AFTER THE RUN HAS ENDED**, which is what makes it the right fact for the
    /// sentence a person reads: `turns` is a `<data>` variable, so it survives the machine reaching
    /// a final state and exiting every region.
    ///
    /// ⚠ [`None`] where the datamodel has stopped answering — a run whose script session is gone
    /// cannot claim a count, and claiming zero would report *nothing was banked* for a reading
    /// failure. That is the distinction [`crate::plugin::Banked`] exists to keep.
    fn banked(&self) -> Option<crate::plugin::Banked> {
        self.turns().and_then(|turns| {
            u32::try_from(turns).ok().map(|completed| {
                crate::plugin::Banked {
                    completed,
                    // The word this document uses for one, in the prompts it composes and in the
                    // `max_turns` bound an author writes. ⚠ Borrowed, which is the case this
                    // `Cow` is cheap for: a LIVE plugin pays nothing, and only a word read back
                    // from a daemon's log arrives owned.
                    unit: std::borrow::Cow::Borrowed("turn"),
                }
            })
        })
    }

    /// ⛔⛔⛔⛔⛔ **WHICH OF THIS LOOP'S THREE ENDINGS IT CLOSED UNDER** — register item 706's third
    /// requirement, and the word that until now existed only inside a sentence.
    ///
    /// # ⚠⚠⚠⚠⚠ Why lifting a word out of prose is the whole repair
    ///
    /// [`DoneReason::noted`](crate::outer::DoneReason::noted) already renders `word(): describe()`
    /// into the walk, so every consumer HAD the word — inside a line of prose, behind a parse, and
    /// spelled by a vocabulary they would have to keep in step by hand. Register item 594 measured
    /// what that costs one field over: `sprag stand-down` promises *its work is kept*, and the row
    /// a person then read said `converged`, byte-identical to a run nobody had ordered anything of.
    ///
    /// ⚠⚠⚠ **READ FROM THE DOCUMENT, NEVER REMEMBERED FROM THE RAISE** —
    /// `OuterLoop::closing_because`'s rule, which is what makes this a report and not a claim: what
    /// a reader is told is what the machine was told, so a run whose transition never fired cannot
    /// be published as having declared anything. ⚠ Named rather than linked, because that method is
    /// `pub(crate)` and this doc is public — the reach it needed was into this crate, not out of it.
    ///
    /// ⚠⚠ [`None`] for every run that has not reached `closing` — which is every live run, every
    /// cancelled one, and every one that hit a ceiling — and for a datamodel that has stopped
    /// answering. Both are *this loop names no ending*, which is the honest answer in each case;
    /// [`crate::driver::Outcome::done_reason`] carries that distinction to the wire by omitting the
    /// key rather than publishing a null.
    fn ended_because(&self) -> Option<&'static str> {
        self.inner.closing_because().map(DoneReason::word)
    }

    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        match self.inner.pump(panes, run)? {
            Pumped::Moved {
                from,
                raised,
                to,
                spent,
                witnessed,
                found,
                because,
                unreadable,
                checked,
                made,
                explained,
                shown,
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
                let note = Self::walked(
                    from,
                    raised,
                    to,
                    Learned {
                        found: found.as_ref(),
                        because,
                        unreadable: unreadable.as_deref(),
                        checked,
                        made,
                        explained: explained.as_deref(),
                        shown,
                        witnessed,
                    },
                );
                // ⚠⚠⚠⚠⚠ **WHETHER THAT ARRIVAL WAS AN ENDING IS THE DOCUMENT'S TO SAY** — register
                // item 470, stage 3, and this line is where the last copy of the answer stood. The
                // engine holds the `<final>` elements, so `finished` is the question asked of the
                // thing that knows; `to` is still what the ending is REPORTED as, which is a
                // different question and the one `ended` answers.
                //
                // ⚠⚠ ASKED OF THE MACHINE RATHER THAN OF `to`, and the two cannot drift: every
                // `Pumped::Moved` reads `to` immediately after its own last raise, and nothing
                // between there and here advances the machine — `took_screening` above is a take.
                if self.inner.finished() {
                    self.ending(to, spent, note)
                } else {
                    Ok(Step::new(Cost::Bytes(spent), Verdict::Continue).noting(note))
                }
            }
            Pumped::Ended(state) => {
                self.ending(state, 0, format!("the loop is already in {state:?}"))
            }
            Pumped::Unbuilt(state) => Err(self.unbuilt(state)),
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
        // ⚠⚠⚠⚠⚠ **TWENTY-EIGHT ARMS STOOD HERE AND ALL TWENTY-EIGHT ARE GONE** — register item 470,
        // stage 3. Three of them answered `None` and **twenty-five answered the pane**; all three
        // of the three are ENDINGS, so the twenty-five were never a decision — they were one fact,
        // *a run in flight has a pane*, written out twenty-five times to keep the match exhaustive.
        // Every ending now says it for itself, on the act it already declares.
        //
        // ⚠⚠ `None` FROM `signalling` IS A RUN THAT HAS NOT ENDED, and it must reach the pane arm:
        // that is exactly when a stop most certainly does have something to reach. Folding it in
        // with `Signals::Nothing` would leave a live model running after a cancel — which is the
        // failure this whole method exists to prevent, so the fold is spelled out rather than left
        // to a reader of `_`.
        match self.inner.signalling() {
            Some(crate::act::Signals::Nothing) => None,
            Some(crate::act::Signals::Pane) | None => Some(self.inner.pane()),
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
            _ if self.inner.published().is_some() => Accounting::Cannot(format!(
                "the loop had already ended in {state:?} when its {} ceiling fell due",
                ceiling.wire_str(),
            )),
            // ⚠⚠⚠⚠⚠ **AND EVERYTHING BELOW IS THE DOCUMENT'S** — register item 470, stage 3, the
            // last of the driver's copies of the topology. Twenty-eight arms stood here: eight
            // granting a window and twenty naming a reason not to. Each state now answers for
            // itself on a targetless `<transition event="account">`, and what is left is a match
            // over `crate::act::Accounts` — a vocabulary of five answers, not a topology.
            _ => match self.inner.asked_of_this_account() {
                // ⚠⚠⚠ THE WINDOW IS PRICED HERE AND NOWHERE ELSE. The document says the agent CAN
                // be asked; how long it gets is two of the CALLER's own turns, which is a quantity
                // no document holds — see the doc above for why a live run had to price it.
                Some(crate::act::Accounts::Within) => {
                    self.inner.stop_short(ceiling);
                    // ⚠⚠⚠ TWO TURNS, AND A LIVE RUN IS WHAT PRICED THE SECOND ONE — the doc above.
                    Accounting::Within(
                        self.inner
                            .turn_within()
                            .unwrap_or(DEFAULT_REPLY_TIMEOUT)
                            .saturating_mul(2),
                    )
                }
                Some(crate::act::Accounts::NeverAsked) => Accounting::Cannot(
                    "the loop never got its pane, so its agent was never asked anything and has \
                     nothing to account for"
                        .to_owned(),
                ),
                Some(crate::act::Accounts::NotOurs) => Accounting::Cannot(
                    "the pane is not this run's to type in: it is showing a question nothing here \
                     could answer, or somebody is typing in it — asking where the run got to would \
                     answer that dialog or type under their hand"
                        .to_owned(),
                ),
                Some(crate::act::Accounts::BetweenSessions) => Accounting::Cannot(format!(
                    "the run is between sessions ({state:?}): the agent that did the work is \
                     being replaced, and its successor has done none of it"
                )),
                // ⚠⚠⚠ THE PANE IS THIS RUN'S AND THE AGENT IS STILL UNREACHABLE, which is why the
                // document gives this its own word rather than folding it into either neighbour.
                // Typing here is allowed — unlike `not_ours`, nobody's hand is in the pane and no
                // dialog would read the Enter — and it would still buy nothing: the account has to
                // come back from the SERVICE that just refused a turn, so asking spends the
                // ceiling's last seconds waiting for the same outage to answer. **What a reader
                // needs is the outage, and saying so is more use than a blank report.**
                Some(crate::act::Accounts::ServiceDown) => Accounting::Cannot(format!(
                    "its agent's service was not answering when the {} ceiling fell due, so the \
                     run was waiting the outage out rather than working; asking where it got to \
                     would have to reach the same service",
                    ceiling.wire_str(),
                )),
                // ⚠⚠⚠⚠ **A STATE THAT DECLARED NOTHING, AND IT IS REPORTED RATHER THAN GUESSED.**
                // The arm this replaced named the parallel root, the two region roots and the
                // orders — states `Self::state` reads the WORK region precisely so as never to
                // return — and called them all *already ended*, which was never true of any of
                // them. `Unbuilt`'s sentence is the honest one: this driver has no answer for what
                // it is looking at, and a ceiling that fell due on a reader bug should say so.
                None => Accounting::Cannot(format!(
                    "its document declares no `account.ask` for {state:?}, so nothing can say \
                     whether its agent could be asked where the run got to when the {} ceiling \
                     fell due",
                    ceiling.wire_str(),
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    // ⚠ `StatePolicy` is what carries the COMPILED TOPOLOGY — `is_descendant_of`, `is_compound_state`
    // and the two name tables — and `every_driven_state_says_what_a_pass_of_it_is_for` derives its
    // population from that rather than from a list written here (register item 749).
    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue, StatePolicy};

    use super::{AiLoop, Learned, NotStarted};
    use crate::access::PaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState, ProgressCell, Stopped};
    // ⚠ `OuterLoop` and `Pumped` are gone from here, and their going is a fact: the gate that used
    // them drove the layer UNDER the door in order to reach a state the door refused. The door no
    // longer refuses it, so the PLUGIN reaches it, which is the only height a caller has.
    use crate::outer::{
        AiLoopSpec, Brief, Checked, Evidence, INNER_SESSION_ENDS, Noticed, Unstated,
    };
    use crate::plugin::{Accounting, Cost, Plugin, Resumption, Verdict};
    use crate::readiness::ReadyWhen;
    use crate::run::RunContext;
    use crate::sm::ai_loop::{AiLoopEvent, AiLoopPolicy, AiLoopState};
    use crate::testing::{screen_showing, standin_agent, standin_agent_that_leaves, supervised};

    /// The document's own composed prompt, as a person reading the file expects it.
    const COMPOSED_START_PROMPT: &str = "North star: ";

    /// ⛔⛔⛔⛔⛔ **A REFUSAL THE DOOR EXPLAINED IS NOT REPORTED AS THE ONE IT DID NOT** — register
    /// item 510, and the sentence that used to name the wrong half of the right file.
    ///
    /// # ⚠⚠⚠⚠⚠ What was wrong, and why it was a number rather than a fix for seven days
    ///
    /// [`crate::outer::OuterLoop::new`] refuses in three places and returned [`Option`], so all
    /// three arrived here as one absence and were reported as `NotStarted::Undrivable` — *"this
    /// build's document does not carry the strings a loop is driven by"*. That sends a reader to
    /// look for a missing `<data>`. It is true of ONE of the three. For a document the door
    /// REFUSED, the truth is that an error was raised where nothing could answer it, and item 505
    /// had already built the sentence saying so — [`crate::document::Faulted`] — which could not
    /// get past the signature.
    ///
    /// # ⚠⚠⚠⚠ The fault is REAL, which is what stops this being a test of its own fixture
    ///
    /// `probe_unanswered.scxml` raises `error.execution` and answers no error at all, so
    /// [`crate::document::opened`] hands back a genuine [`crate::document::Faulted`] — the same
    /// value the door would produce on the day `ai_loop.scxml` stopped answering its own. A
    /// hand-built struct would assert that this mapping moves a payload; this asserts that the
    /// payload a real refusal carries survives it.
    ///
    /// ⚠⚠ **AND THE THREE MUST BE THREE.** Two of them mapping to one arm is precisely the defect,
    /// so the claim is that the set has three distinct members — a mapping that collapsed any pair
    /// again passes every other assertion here.
    #[test]
    fn a_door_that_refused_a_document_says_so_rather_than_blaming_its_datamodel() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        // ⚠ THE PRODUCT'S OWN ROAD AND THE PRODUCT'S OWN HOST — `document`'s door, not a hand
        // `Engine::new`, which is this crate's recorded fixture failure shape.
        let faulted = crate::document::opened(
            crate::sm::probe_unanswered_sm::ProbeUnansweredPolicy::new(lua),
            &crate::act::Serving::new(),
        )
        .err()
        .expect(
            "⚠⚠⚠ `probe_unanswered.scxml` raises an error it answers nowhere, so the door must \
             refuse it — without a real refusal this gate would be about a struct somebody typed",
        );

        let refused = NotStarted::from(crate::outer::Unopened::Faulted(faulted.clone()));
        assert_eq!(
            refused,
            NotStarted::Unanswered(faulted.clone()),
            "⛔⛔⛔ ITEM 510: a document the door refused must arrive as its own refusal. It used \
             to arrive as `Undrivable`, which is a claim about a `<data>` block and sends whoever \
             reads it to the wrong place with nothing to find",
        );
        let NotStarted::Unanswered(carried) = &refused else {
            panic!("the arm above is what it is: {refused:?}");
        };
        // ⚠⚠⚠⚠ **THE FAULT'S OWN SENTENCE, WHICH IS THE ITEM'S DONE-WHEN IN ONE ASSERTION.** Not a
        // sentence composed here: `Faulted`'s `Display` names the class (who repairs it), the
        // count, and that an error abandons the rest of its block — the fact that makes a
        // half-composed `onentry` read as a slow peer.
        let said = carried.to_string();
        assert!(
            said.contains("error.execution") && said.contains("never ran"),
            "⛔⛔⛔⛔ ITEM 510's done-when: the fault's OWN sentence must survive the door. A \
             mapping that kept the arm and dropped the payload would pass everything above and \
             leave a reader with a word and no diagnosis. Said: {said:?}",
        );

        // ⚠⚠⚠ AND THE OTHER TWO KEEP THEIR OWN. `Undrivable` is the arm whose sentence was always
        // true — it must still mean only that — and a machine with no script session is a third
        // fact about a different file (the engine, not the document).
        let three = [
            refused.clone(),
            NotStarted::from(crate::outer::Unopened::Sessionless),
            NotStarted::from(crate::outer::Unopened::Undrivable),
        ];
        assert_eq!(
            (three[1].clone(), three[2].clone()),
            (NotStarted::Sessionless, NotStarted::Undrivable),
            "⚠⚠⚠ each refusal keeps its own word, or the collapse this item paid for comes back \
             wearing a different pair",
        );
        for (one, two) in [(0, 1), (0, 2), (1, 2)] {
            assert_ne!(
                three[one], three[two],
                "⛔⛔⛔⛔⛔ ITEM 510: THREE REFUSALS, THREE ANSWERS. Two of them landing on one \
                 arm is the whole defect — a caller then reads one sentence for two different \
                 repairs, and one of the two is always wrong. Got {three:?}",
            );
        }
    }

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

    /// **THE HOLD CEILING EVERY GATE HERE DECLARES**, in milliseconds — item 534's key, written out
    /// for the two above's reason.
    ///
    /// ⚠⚠⚠ IT IS DELIBERATELY LONGER THAN THE WHOLE SUITE, which is the opposite choice to
    /// `GATE_TURN_MS`'s and the right one for this key: no gate here holds a run, so a ceiling that
    /// could elapse would end runs measuring something else — and it would do it by TIME, which is
    /// the flakiest way for a suite to fail. The gate that DOES hold a run names its own small
    /// number, which is the arrangement this constant exists to make visible.
    ///
    /// ⚠ Zero is refused by the brief, so a fixture cannot spell *"no ceiling"* here even by
    /// accident — see `OuterLoop::hold_within`.
    const GATE_HOLD_MS: i64 = 3_600_000;

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
            // ⚠⚠⚠ AND NO LEDGER: a stand-in that kept counts would keep them under the AMBIENT
            // state home, which is the home of whoever ran the suite. See
            // [`AiLoopSpec::review_ledger`], where that used to happen with no way to say no.
            review_ledger: None,
            // ⚠⚠ AND NOBODY IS ASKED. A stand-in that acquired an asker would spawn a second
            // process on every session replacement these gates walk through — the judge's argument
            // one field over, and register item 502's own reason for making this declinable.
            review_asks: None,
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
            // ⚠ These fixtures measure a BOUNDED run; the declined budget is a kind's decision and
            // has its own gate rather than being folded in here.
            closing_rules: None,
            working_rules: None,
            unverified_rules: None,
            context_ceiling: None,
            reflect_after_refusals: None,
            milestone_check: None,
            // ⚠ DECLINED, like the two above it: a stand-in peer has no service to fail, so a
            // needle here would be quoting words nothing in these fixtures ever prints. The
            // outage path has its own gates, which arm it deliberately.
            service: None,
            max_turns: Some(crate::outer::Counted::Of(max_turns)),
            // ⚠ EQUAL, which is what makes `reflecting` unreachable — `judging` tests the turn
            // budget first. `AiLoop::new` refuses anything smaller, and the gate below drives that.
            //
            // ⚠⚠ WRITTEN OUT RATHER THAN LEFT `None`, though `None` now resolves to exactly this
            // (item 312). These fixtures state the arrangement they are measuring; a gate that let
            // the resolution supply it would be asserting about a default it never named.
            reflect_every: Some(max_turns),
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
            // ⚠⚠ AND THE HOLD CEILING, WRITTEN RATHER THAN INHERITED for the reason above it —
            // register item 534. The shipped document authors FOUR HOURS, which never fires in a
            // 74-second suite and would therefore be a number these gates assert nothing about;
            // naming it here is what lets the one gate that holds a run name a small one and mean
            // something by it. See `GATE_HOLD_MS`.
            hold_within_ms: Some(GATE_HOLD_MS),
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

    /// ⚠⚠⚠⚠⚠ **EVERY PAYLOAD THIS LOOP HANDS ITS MACHINE IS ONE THE DATAMODEL CAN READ** —
    /// consuming SCE's `undecodable_payloads`, 2026-08-23.
    ///
    /// # ⚠⚠⚠⚠ The failure it catches is a run that converges and decides nothing
    ///
    /// W3C SCXML B.2.8.1's third rung — *otherwise, the Processor MUST treat the content as a
    /// space-normalized string literal* — is the right answer to a payload the datamodel cannot
    /// parse, and it is SILENT. `judging` reads `_event.data.done` and four keys beside it; a
    /// `judge` whose payload stopped parsing leaves all five nil, every guard reads false, the
    /// ordinary edge is taken and the run converges. **Nothing raises, nothing fails, and the
    /// verdict the agent gave is gone** — the same shape as register item 483's abandoned block,
    /// one layer further out, and with no error event to catch it.
    ///
    /// Upstream measured exactly this on three independent Lua implementations and asked this loop
    /// to look (SCE reply, 2026-08-23). [`AiLoop::unreadable_payload`] is the reading that makes
    /// looking possible; before it, no gate in this crate could have gone red for it.
    ///
    /// ⚠⚠ **ASKED OF A LIVE RUN, not of the fixture constants beside it.** [`TURN`] and
    /// [`ORDINARY`] are COPIES of what the driver sends, so a gate over them stays green on the
    /// day the product's own `serde_json::json!` starts emitting something else — which is the
    /// only way this can actually break.
    #[test]
    fn every_payload_this_loop_sends_is_one_its_datamodel_can_read() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut loops, &access, &RunContext::uncancellable());

        // ⚠ THE FIXTURE'S OWN PRECONDITION: a run that stopped early sent fewer payloads than this
        // gate means to cover, so a clean reading below would be about the ones it never sent.
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠ this run must reach the ending it is written around, or the reading below covers a \
             walk that did not happen: {:?}",
            outcome.state,
        );
        assert_eq!(
            loops.unreadable_payload(),
            None,
            "⚠⚠⚠⚠⚠ THE DATAMODEL COULD NOT READ WHAT THE DRIVER SENT ON THIS EVENT, so every \
             `_event.data.<key>` the document reads for it is empty — and the run converged \
             anyway, which is precisely why nothing else in this crate could have said so",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // ── THE CONTROL: the same document, one payload that announces structure and will not parse ──
        //
        // ⚠⚠⚠⚠⚠ Without it, `None` above is also what a reader wired to nothing answers. Lua's own
        // table syntax is the shape chosen on purpose: it opens with `{`, so the ladder ATTEMPTS a
        // structured read, and only an attempt that failed is the reading a driver may act on —
        // prose arrives as text quietly and must not count (W3C test 562).
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        carried(&mut engine, &host, AiLoopEvent::Judge, "{done=true}");
        assert_eq!(
            engine.last_undecodable_payload(),
            Some(AiLoopEvent::Judge),
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, so the reading above says nothing about this run — a \
             payload in Lua's table syntax has to come back as the one reading that names a \
             problem, or this whole gate is blind",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A RUN'S WALK IS THE MACHINE'S REAL PATH — EVERY EDGE, AND THEY CHAIN** — register
    /// item 614, and the reading item 605 needed and did not have.
    ///
    /// # ⚠⚠⚠⚠ What a journal used to be able to hide
    ///
    /// A step published its path as PROSE, one line, whatever the pass actually did. Two things
    /// followed and both were measured on 2026-08-23:
    ///
    /// * **A pass that raised two events published one.** The pass below raises `judge` in
    ///   `judging`, lands in `working`, then cannot deliver that turn's prompt and raises
    ///   `peer.gone`. The journal carried only the second, so a reader could see where the run
    ///   ended and had no way to learn when it left `judging`.
    /// * **The one it published named the wrong state.** `from` was read at the top of the pass,
    ///   so the line read `Judging --PeerGone--> PeerGone` for an event the machine answered from
    ///   `working`. Item 605 spent four rounds and five guard rewrites on that sentence.
    ///
    /// # ⚠⚠⚠ Why the chain is the invariant worth asserting
    ///
    /// *Every edge is present* and *`from` is read at the raise* are two rules, and a walk that
    /// obeys both is exactly a walk in which each edge begins where the last one ended — within a
    /// step and across steps. One property catches both failures, and it cannot rot: it is checked
    /// against the walk itself rather than against a list of states kept here.
    ///
    /// ⚠⚠ **THE WORDS ARE THE DOCUMENT'S.** Every one comes from SCE's generated
    /// `get_state_name` / `get_event_name`, so a state renamed in `ai_loop.scxml` renames itself
    /// here and this gate keeps asking the same question.
    #[test]
    fn a_runs_walk_carries_every_edge_its_machine_took_and_they_chain() {
        let (workspace, pane) = standin_agent_that_leaves();
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let run = RunContext::uncancellable();
        let mut pumped = 0;
        while loops.state() != AiLoopState::Judging && pumped < 40 {
            loops.step(&access, &run).expect("a live pane takes a pass");
            pumped += 1;
        }
        assert_eq!(
            loops.state(),
            AiLoopState::Judging,
            "⚠⚠ THE FIXTURE'S PRECONDITION: this loop must bank a turn within {pumped} passes, or \
             the two-event pass this gate is about never happens",
        );
        loops.stand_down();

        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 120,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &run);
        let journal = progress.lock().expect("the progress cell").journal.clone();
        // ⚠⚠ THE FIXTURE'S SECOND PRECONDITION, and the reason this outcome is READ rather than
        // dropped: the two-event pass only happens on the run that ENDS here. A run that converged
        // or was stopped by a ceiling never raised `judge` and `peer.gone` in one pass, so every
        // assertion below would be about some other walk.
        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⚠⚠ this fixture's agent leaves after banking a turn, so the run must end at \
             `peer_gone`; a {:?} run took a different path and this gate is measuring it by \
             mistake. Journal: {journal:?}",
            outcome.state,
        );
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        // ── THE CONTROL FIRST: a walk that is EMPTY chains vacuously ──
        //
        // ⚠⚠⚠⚠⚠ Every assertion below is a `for` over edges, and a journal carrying none passes
        // all of them while saying nothing at all. That is exactly what this field looked like
        // before it existed, so the gate has to refuse it.
        let edges: Vec<_> = journal.iter().flat_map(|step| step.walked.iter()).collect();
        assert!(
            !edges.is_empty(),
            "⚠⚠⚠⚠⚠ THE WALK IS EMPTY, so nothing below is being checked. A run that took steps \
             took transitions; a journal that carries none is the prose-only journal this field \
             replaced. Journal: {journal:?}",
        );

        // ── THE SUBJECT: the pass that raised TWO events published both ──
        let two = journal
            .iter()
            .find(|step| step.walked.len() > 1)
            .unwrap_or_else(|| {
                panic!(
                    "⛔ NO PASS PUBLISHED MORE THAN ONE EDGE. This fixture's fatal pass raises \
                     `judge` and then `peer.gone`, so a journal of one-edge steps means the walk \
                     is back to one line per pass and register item 614 has regressed. Journal: \
                     {journal:?}"
                )
            });
        assert_eq!(
            (two.walked[0].from, two.walked[0].raised, two.walked[0].to),
            ("judging", "judge", "working"),
            "⚠⚠⚠⚠⚠ THE EDGE THAT USED TO VANISH. It is the one that says WHEN the run left \
             `judging`, which is the question item 605 could not answer for four rounds. Journal: \
             {journal:?}",
        );

        // ── AND THE WHOLE WALK IS A CHAIN, within each step and across them ──
        let mut previous: Option<&crate::plugin::Edge> = None;
        for edge in &edges {
            if let Some(before) = previous {
                assert_eq!(
                    before.to, edge.from,
                    "⚠⚠⚠⚠⚠ THE WALK IS NOT A PATH: `{}` ends in `{}` and the next edge starts from \
                     `{}`. Either an edge is missing, or a `from` was read somewhere other than at \
                     its own raise — the two failures item 614 exists for. Journal: {journal:?}",
                    before.raised, before.to, edge.from,
                );
            }
            previous = Some(edge);
        }

        // ── AND THE STEP'S OWN SENTENCE NAMES THE SAME STATE ITS LAST EDGE DOES ──
        //
        // ⚠⚠⚠⚠ **THE LAST EDGE, AND FINDING THAT OUT IS WHY THIS CHECK IS WORTH HAVING.** Written
        // against the FIRST edge it went red on the two-event pass with *the note says `Working
        // --PeerGone--> PeerGone` and the edge was raised from `judging`* — because the arm reports
        // the raise IT made, which is the pass's last. A pass that raised `judge` and then
        // `peer.gone` describes the second, and the first is the one only `walked` carries.
        //
        // ⚠⚠⚠⚠⚠ **TWO PUBLICATIONS OF ONE FACT, HELD AGAINST EACH OTHER** — register item 470's
        // shape, and the reason this is not the prose-judging item 611 forbids: the SENTENCE is
        // the artefact under test here, not the evidence. `Pumped::Moved` carries a `from` of its
        // own and `walk` reads another at the raise, so the two can disagree — and when they do it
        // is the sentence that is wrong, because the edge was read at the moment the event went in.
        //
        // ⚠⚠⚠ **THAT DISAGREEMENT IS WHAT COST REGISTER ITEM 605 FOUR ROUNDS.** Two arms of `pump`
        // raise after `pumping` has run, and both once stamped their line with the state the PASS
        // opened in. Fixing one left the other, and the comment beside the fix said *every other
        // arm* — so nobody counted them until 2026-08-23. This is the check that would have.
        // ⚠⚠ THE TWO VOCABULARIES ARE NORMALISED RATHER THAN ASSUMED EQUAL: the sentence spells a
        // state with Rust's `Debug` (`PeerGone`) and the edge with the document's own id
        // (`peer_gone`), which are the same word in two alphabets. Folding case and underscores is
        // what lets the check be about the STATE rather than about a spelling convention.
        let plainly = |word: &str| word.to_lowercase().replace('_', "");
        for step in &journal {
            let (Some(note), Some(reported)) = (step.note.as_deref(), step.walked.last()) else {
                continue;
            };
            assert!(
                plainly(note).starts_with(&plainly(reported.from)),
                "⚠⚠⚠⚠⚠ THE STEP'S SENTENCE AND THE RAISE IT REPORTS NAME DIFFERENT STATES: the note \
                 says {note:?} and that event was raised from {:?}. The edge is the one read AT the \
                 raise, so a mismatch means some arm of `pump` is stamping its line with the state \
                 the pass OPENED in — item 605's defect, wearing its original costume",
                reported.from,
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **A JUDGING RUN WHOSE PANE LEFT ITS POOL DIES AT THE DOOR IT TYPES THROUGH** —
    /// register item 682, reproduced end to end at the height the live runs died at.
    ///
    /// # ⚠⚠⚠⚠⚠ The three runs this is the reconstruction of
    ///
    /// 2026-08-25: runs `0`, `1` and `3` each ended `failed: there is no pane N`, and all three
    /// carried `Working --TurnDone--> Judging` as their LAST walk entry with nothing after it.
    /// Register item 680 gave that failure a line in the journal; this gate is what makes the line
    /// say something a diagnosis can use, by producing the identical ending from a known cause.
    ///
    /// **The cause staged here is the one the live evidence leaves standing.** Pane `5` was still
    /// alive in window `pinion` while the run driving it died saying `there is no pane 5`, so the
    /// pane did not die — it is **no longer in the pool the run captured**
    /// (`SessionScope::workspace` is a WINDOW's pane pool, and a run holds that `Arc` for its
    /// whole life). [`Workspace::close`](sprag_terminal::Workspace::close) is how a pane leaves
    /// one — it is what `respawn`, `break-pane`, `join-pane`, `move-pane`, `swap` and
    /// `kill-window` each remove a pane with — and it hands the pane BACK, still running, which is
    /// what this fixture binds.
    ///
    /// # ⭐⭐⭐⭐⭐ THE CALL THIS NAMES, which is what register item 682 was missing
    ///
    /// **`deliver` → [`PaneAccess::inject`] → `WorkspacePaneAccess::typing`** — the one door every
    /// plugin types through, and one of only two on that surface that can raise this word (the
    /// other is `pane_stop_job`, reached only by a cancel, and these runs were not cancelled).
    ///
    /// ⚠⚠⚠⚠ **AND THE PLACE THE JOURNAL REPORTS IS `working`, NOT `judging` — measured, and it
    /// corrected this gate's first draft.** [`Plugin::at`](crate::plugin::Plugin::at) is read when
    /// the pass RETURNS, so it names where the machine ended: the judging pass raises `judge`, the
    /// document carries the loop into `working`, and the run dies delivering that turn's prompt.
    /// That is also why the live runs looked as though they died *in* judging — their last
    /// SUCCESSFUL walk entry was `Working --TurnDone--> Judging`, so the failing step is the one
    /// that BEGAN there. The failure line's own edge (`judging --judge--> working`) is what says
    /// so, and asserting it is what separates this pass from every ordinary working turn.
    ///
    /// # ⚠⚠⚠ Why the pane is taken away at `judging` and not anywhere else
    ///
    /// Because that is where the live runs were, and because it is the first pass after the
    /// opening prompt that TYPES: `judging` reads the turn, finds no marker, and the document's
    /// `judge` edge carries the loop back to `working` with the next prompt to deliver. A pane
    /// removed before the first prompt would fail somewhere else entirely and would prove nothing
    /// about these runs.
    ///
    /// ⚠⚠ **THE CONTROL IS THE SAME PASS ON A POOL THAT STILL HOLDS THE PANE**, and it runs first.
    /// Without it a green here is satisfied by a fixture that could never take a judging pass at
    /// all, which is this workspace's most expensive recorded way to measure nothing.
    ///
    /// ⚠ What this gate does NOT claim: it does not say what removed the pane in production. That
    /// is register item 682's remaining half, and naming it here would be prose ahead of the code.
    #[test]
    fn a_judging_run_whose_pane_left_its_pool_dies_where_the_live_runs_died() {
        /// Pump `loops` until the document is in `judging`, answering how many passes it took.
        ///
        /// ⚠ A shared bound rather than two spellings of one number: the control and the
        /// measurement must reach the SAME place the same way, or they are not comparable.
        fn to_judging(loops: &mut AiLoop, access: &dyn PaneAccess, run: &RunContext) -> usize {
            let mut pumped = 0;
            while loops.state() != AiLoopState::Judging && pumped < 40 {
                loops.step(access, run).expect("a live pane takes a pass");
                pumped += 1;
            }
            assert_eq!(
                loops.state(),
                AiLoopState::Judging,
                "⚠⚠ THE FIXTURE'S PRECONDITION: this loop must bank a turn within {pumped} passes, \
                 or the pass this gate is about never happens",
            );
            pumped
        }

        let run = RunContext::uncancellable();

        // ── 1. THE CONTROL, AND IT COMES FIRST: the judging pass TAKES on a pool that holds it ──
        {
            let (workspace, pane) = standin_agent(9);
            let access = supervised(&workspace);
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            to_judging(&mut loops, &access, &run);
            loops.step(&access, &run).expect(
                "⚠⚠⚠⚠ THE CONTROL FAILED: a judging pass over a pane its pool STILL HOLDS must \
                 take. If it cannot, arm 2's failure is a fact about this fixture rather than \
                 about the pane going missing, and this gate measures nothing",
            );
            assert_ne!(
                loops.state(),
                AiLoopState::Judging,
                "⚠⚠⚠ and it must have MOVED — a pass that judged and stayed put would mean the \
                 delivery arm 2 is about was never reached even in the healthy case",
            );
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
        }

        // ── 2. THE MEASUREMENT: the same pass, after the pane has LEFT the pool ──
        let (workspace, pane) = standin_agent(9);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        to_judging(&mut loops, &access, &run);

        // ⚠⚠⚠⚠ BOUND, never dropped — see the doc. A dropped pane runs the pty's blocking
        // kill/wait, and the run would then meet `PeerGone`, which is a DIFFERENT sentence and a
        // different defect.
        let moved = workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .close(pane)
            .expect("the pool held this loop's pane a statement ago");
        assert!(
            !moved.pty().is_eof(),
            "⚠⚠⚠⚠⚠ THE FIXTURE'S WHOLE PRECONDITION: the pane must still be RUNNING after it left \
             the pool — that is the live shape, where pane 5 was alive in `pinion` while its run \
             said `there is no pane 5`",
        );

        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            // ⚠ SMALL ON PURPOSE: the very first step this driver takes is the judging pass, so a
            // run that needs more than a handful of iterations to die is not dying of this.
            max_iterations: 4,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &run);
        let journal = progress.lock().expect("the progress cell").journal.clone();

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⚠⚠⚠⚠⚠ THE LIVE RUNS' ENDING. A run whose pane left its pool must FAIL — a run that \
             converged or ran out of iterations reached some other ending and every assertion \
             below would be about it. Journal: {journal:?}",
        );

        let last = journal.last().unwrap_or_else(|| {
            panic!(
                "⛔ the failed run wrote NOTHING to its journal — register item 680's line is \
                 gone, and with it the only place this diagnosis can be read. Outcome: {outcome:?}"
            )
        });
        let note = last
            .note
            .as_deref()
            .expect("a failure line carries a sentence");
        assert!(
            note.contains(&format!("there is no pane {}", pane.0)),
            "⚠⚠⚠⚠⚠ **AND IT IS THE LIVE RUNS' SENTENCE, BYTE FOR BYTE** — this is the whole link \
             between this reconstruction and runs 0, 1 and 3. A different word here means the \
             ending reproduced is not theirs: {note:?}",
        );
        // ⭐⭐⭐⭐⭐ **AND HERE IS THE CALL, NAMED** — the whole point of register item 682.
        //
        // The place is `working`, NOT `judging`, and that one word is the diagnosis. `Plugin::at`
        // is read when the pass returns, so it names where the machine ENDED: the judging pass
        // raised `judge`, the document carried the loop into `working`, and the run died
        // **delivering that turn's prompt** — `deliver` → [`PaneAccess::inject`] →
        // `WorkspacePaneAccess::typing`, the one door every plugin types through and one of only
        // two on that surface that can say this word.
        //
        // ⚠⚠⚠ It is also why the live runs looked like they died "in judging": their last
        // SUCCESSFUL walk entry was `Working --TurnDone--> Judging`, so the failing step is the one
        // that began there — and the edge it walked before dying is asserted below, which is what
        // separates *this* working pass from every ordinary one.
        assert!(
            note.contains("working"),
            "⚠⚠⚠⚠ **AND WHERE IT WAS**, in the document's own word. This is the half register item \
             680 built and item 682 needed: without it a reader knows a call failed and not which \
             of twenty-eight states was taking it: {note:?}",
        );
        assert_eq!(
            last.walked,
            vec![crate::plugin::Edge {
                from: "judging",
                raised: "judge",
                to: "working",
            }],
            "⚠⚠⚠⚠⚠ **AND THE EDGE IS WHAT TIES THE DEATH TO THE JUDGEMENT.** `working` alone would \
             also describe an ordinary turn; this says the pass that died is the one the JUDGE sent \
             there, which is the pass all three live runs were on. A failure line carrying no edge \
             would put item 682 back where item 680 found it. Journal: {journal:?}",
        );
        // ⭐⭐⭐⭐⭐ **AND THE READING REGISTER ITEM 682's REPAIR (a) ADDED — ON A REAL RUN.**
        //
        // `driver::tests::a_run_whose_pane_went_missing_says_it_left_rather_than_that_it_never_was`
        // holds the clause against stand-in plugins, which pins the RULE and says nothing about
        // whether a live `ai_loop` reaches it: the guard is `Driver::driving`, refreshed only after
        // a step COMPLETES, so it depends on this document actually banking a pass before it dies.
        // **This is the wiring**, and it is asserted here rather than there because this is the
        // gate that reconstructs the incident.
        assert!(
            note.contains("one window's pool") && note.contains("another window"),
            "⚠⚠⚠⚠⚠ A REAL RUN MUST CARRY THE READING, not just a stand-in. Without it a person who \
             goes and finds the pane alive — as they did, in window `pinion`, with its child up \
             for 2h40m — concludes the run is lying about its own death: {note:?}",
        );
        assert_ne!(
            outcome.state,
            OutcomeState::Cancelled,
            "⚠⚠⚠ and it was NOT cancelled, which is what rules out the OTHER door that can say \
             this word — `pane_stop_job`, reached only by a cancel. See the two-door exhaustion in \
             `access::tests::\
             a_pane_that_left_its_pool_is_unknown_at_the_typing_door_and_silent_at_every_reader`",
        );

        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }
    }

    /// ⛔⛔⛔⛔⛔ **A RUN ENDS THE SAME WAY WHETHER ITS PANE MOVED OR WAS CLOSED — one sentence,
    /// two causes, and that is why register item 682 could not be closed from a run's record.**
    ///
    /// # ⚠⚠⚠⚠⚠ The question this settles, and the one it proves unanswerable from here
    ///
    /// A run holds ONE pane pool for its whole life — `SessionScope::workspace`, which is a
    /// WINDOW's pool, chosen from the session's CURRENT window at the moment `orchestrate` was
    /// called and never re-derived. So `there is no pane N` means exactly *that id is not in the
    /// pool I captured*, and a pane leaves a pool through [`sprag_terminal::Workspace::close`] and
    /// nowhere else. Production reaches it four ways, and only three can take a pane out from under
    /// a live run: a cross-window MOVE (`break-pane` / `join-pane` / `move-pane` / `swap`), a
    /// `respawn`, and a plain `close`.
    ///
    /// **This gate measures that the run cannot tell them apart.** Both arms drive the real loop to
    /// the real judging pass and let the real driver end it; the sentences are compared BYTE FOR
    /// BYTE, and they are equal. That is not a defect being asserted for its own sake — it is the
    /// reason a deterministic failure stayed a hypothesis for a day, and the reason the removal was
    /// finally named by a fact from OUTSIDE the run:
    ///
    /// ⭐ **the pane's child had been running for 2h40m, continuously, straight through the run's
    /// death** (2026-08-25, pane id 5 in window `pinion`, pid 3433363, started 09:28:52). A `close`
    /// and a `respawn` both kill it. **Only the move leaves it alone** — which is what
    /// `sprag_terminal`'s `break_pane_moves_a_live_child_rather_than_replacing_it` holds at the
    /// door. So the removal in that incident was a MOVE, and the two arms below are why nothing in
    /// the run's own journal could have said so.
    ///
    /// # ⚠⚠⚠ Why the arms stage `close` + `adopt` rather than calling `break_pane`
    ///
    /// Because a run cannot see a window. What reaches it is the POOL, and `close` + `adopt` is
    /// precisely the pair `Session::break_pane`, `join_pane_at`, `move_pane` and `swap_panes` are
    /// each built from — the source pool loses the pane, another gains it, and nothing is
    /// signalled. The registry-level gate named above is what holds that wiring, so the two
    /// together cover door → pool → run without either one asserting the other's half.
    ///
    /// ⚠ The destination is a [`sibling`](sprag_terminal::Workspace::sibling) pool, because that is
    /// what `break_pane` mints for the window it is opening — a fresh pool sharing the id counter.
    #[test]
    fn a_run_ends_the_same_way_whether_its_pane_moved_or_was_closed() {
        /// Drive a fresh loop over a live stand-in until the document is in `judging`.
        ///
        /// ⚠⚠ THE CONTROL LIVES HERE: every pass up to `judging` is `expect`ed to TAKE, so a
        /// failure in either arm below is what the removal did rather than a loop that never ran.
        fn judging_over_a_live_pane() -> (
            Arc<std::sync::Mutex<sprag_terminal::Workspace>>,
            sprag_terminal::PaneId,
            crate::access::WorkspacePaneAccess,
            AiLoop,
        ) {
            let (workspace, pane) = standin_agent(9);
            let access = supervised(&workspace);
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let run = RunContext::uncancellable();
            let mut pumped = 0;
            while loops.state() != AiLoopState::Judging && pumped < 40 {
                loops.step(&access, &run).expect("a live pane takes a pass");
                pumped += 1;
            }
            assert_eq!(
                loops.state(),
                AiLoopState::Judging,
                "⚠⚠ THE FIXTURE'S PRECONDITION: this loop must bank a turn within {pumped} passes",
            );
            (workspace, pane, access, loops)
        }

        /// Let the driver take the judging pass, and answer the sentence the run died with.
        fn dies_saying(access: &crate::access::WorkspacePaneAccess, loops: &mut AiLoop) -> String {
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                // ⚠ The very first step this driver takes is the judging pass, so a run needing
                // more than a handful of iterations to die is not dying of the removal.
                max_iterations: 4,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(loops, access, &RunContext::uncancellable());
            let journal = progress.lock().expect("the progress cell").journal.clone();
            assert_eq!(
                outcome.state,
                OutcomeState::Failed,
                "⚠⚠⚠ a run whose pane has left its pool must FAIL — a run that converged or spent \
                 its iterations reached some other ending. Journal: {journal:?}",
            );
            journal
                .last()
                .and_then(|step| step.note.clone())
                .expect("register item 680's failure line carries a sentence")
        }

        /// Every pane the pool still holds, closed — the fixtures' own tidying.
        fn tidy(access: &crate::access::WorkspacePaneAccess) {
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
        }

        /// Lock a pool, recovering the guard if a holder panicked.
        fn held(
            pool: &std::sync::Mutex<sprag_terminal::Workspace>,
        ) -> std::sync::MutexGuard<'_, sprag_terminal::Workspace> {
            pool.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        // ── ARM 1: THE PANE MOVED — `close` then `adopt`, and its child is never signalled ──
        let (moved, moved_pane) = {
            let (source, pane, access, mut loops) = judging_over_a_live_pane();
            let destination = Arc::new(std::sync::Mutex::new(held(&source).sibling()));
            let taken = held(&source)
                .close(pane)
                .expect("the run's pool held its pane a statement ago");
            held(&destination).adopt(taken);
            let said = dies_saying(&access, &mut loops);
            assert!(
                !held(&destination)
                    .pane(pane)
                    .expect("the destination pool adopted it")
                    .pty()
                    .is_eof(),
                "⚠⚠⚠⚠⚠ THE LIVE SHAPE: a moved pane is STILL RUNNING while the run that was \
                 driving it says there is no such pane. If this ever kills the child, the process \
                 age that named the removal stops being evidence",
            );
            tidy(&access);
            (said, pane)
        };

        // ── ARM 2: THE PANE WAS CLOSED — removed and DROPPED, which kills its child ──
        //
        // ⚠⚠⚠ DROPPED ON PURPOSE, and it is the whole difference between the arms: `close` hands
        // the pane back, and it is the caller keeping it (arm 1) or letting it go (here) that
        // decides whether a child survives. A `respawn` is this arm plus a fresh spawn.
        let (closed, closed_pane) = {
            let (source, pane, access, mut loops) = judging_over_a_live_pane();
            drop(
                held(&source)
                    .close(pane)
                    .expect("the run's pool held its pane a statement ago"),
            );
            let said = dies_saying(&access, &mut loops);
            tidy(&access);
            (said, pane)
        };

        // ── THE MEASUREMENT: TWO CAUSES, ONE SENTENCE ──
        assert_eq!(
            moved_pane, closed_pane,
            "⚠⚠ THE FIXTURES MUST NAME THE SAME PANE, or the comparison below is about two ids \
             rather than about two causes",
        );
        assert!(
            moved.contains(&format!("there is no pane {}", moved_pane.0)),
            "⚠⚠⚠⚠ and it must be the LIVE RUNS' sentence, or neither arm is about them: {moved:?}",
        );
        assert_eq!(
            moved, closed,
            "⭐⭐⭐⭐⭐ **THE FINDING.** A pane that was MOVED and a pane that was CLOSED end a run \
             with the identical line — same word, same place, same walk. Nothing a run records \
             separates them, which is exactly why register item 682 needed a pane's PROCESS AGE, \
             measured hours later from outside the run, to name which removal it had been",
        );

        // ⚠⚠⚠⚠⚠ **AND WHAT REPAIR (a) DID AND DID NOT CHANGE, said here so the equality above is
        // not read as *nothing was learned*.**
        //
        // Both lines now carry the run's own reading — *a workspace is one window's pool; it may
        // still be open in another window* — which is what stops a reader who finds the pane alive
        // concluding the run lied. That is a real gain and it is asserted, not assumed.
        //
        // **It does not break the equality, and could not.** *Moved* and *closed* differ in what
        // became of the pane's CHILD, and a pool cannot see a pane it no longer holds — the ISP
        // boundary `sprag_host::plugin_host` keeps means this layer has one window's pool and no
        // way to ask about another. So the discriminator is genuinely outside: it took an
        // operating-system fact (a pid's age) to settle it. **A future repair that wanted this
        // equality to break would have to give the run a reader it does not have today**, which is
        // a decision about that boundary rather than a sentence somebody forgot to write.
        assert!(
            moved.contains("one window's pool") && closed.contains("one window's pool"),
            "⚠⚠⚠⚠ BOTH endings must carry the reading register item 682's repair (a) added — the \
             equality above is *the two causes are indistinguishable*, NOT *the run says nothing*. \
             Moved: {moved:?}; closed: {closed:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN WHOSE PANE MOVED TO ANOTHER WINDOW KEEPS DRIVING IT, AND ONE WHOSE PANE WAS
    /// CLOSED STILL DIES** — register item 682, and the repair the gate above predicted.
    ///
    /// # ⚠⚠⚠⚠⚠ The equation above is the OLD truth, and this is why it stood
    ///
    /// `a_run_ends_the_same_way_whether_its_pane_moved_or_was_closed` says a moved pane and a
    /// closed one end a run with the identical line, and its own note says what would break that:
    /// *"a future repair that wanted this equality to break would have to give the run a reader it
    /// does not have today"*. This is that reader — [`crate::access::PaneElsewhere`], the daemon's
    /// answer to *which pool holds this pane*, arriving as an opaque `Fn` on register item 689's
    /// terms so the plugin layer still learns nothing about session trees.
    ///
    /// ⚠⚠ **THE EQUATION GATE STAYS AND STAYS GREEN**, which is not a contradiction: it builds its
    /// access with NO hook, and a host that installs none is one whose pool is the whole world.
    /// The two gates together are the claim — *without the reader the two removals are one
    /// sentence, with it they are two outcomes* — and either alone is half of it.
    ///
    /// # What was measured, and what it cost
    ///
    /// Runs 0, 1 and 3 of this repository's own loop died `failed: there is no pane N` while the
    /// panes they named were open: pane 5's `claude` had been running for **9,632 seconds across
    /// the death**. Moving a pane between windows is `close` + `adopt` — the pane is never touched
    /// — so a person rearranging their windows was killing somebody's run, and the run's own record
    /// could not say so.
    ///
    /// ⚠⚠⚠ **THE CLOSED ARM IS NOT A FORMALITY, IT IS THE ESCAPE HATCH THIS REPAIR COULD HAVE
    /// OPENED.** A hook that answered *somewhere* for a pane nobody holds would make a run type
    /// into a pane that is gone — the failure `PeerGone` exists for — and every assertion about the
    /// moved arm would still pass. So the two arms are driven side by side, and a closed pane must
    /// still end the run with the live runs' own sentence.
    #[test]
    fn a_run_follows_its_pane_to_another_window_and_still_dies_when_it_is_closed() {
        /// Drive a fresh loop over a live stand-in until the document is in `judging`, with the
        /// daemon's *where else is this pane* reader installed over `destination`.
        ///
        /// ⚠⚠ THE HOOK IS INSTALLED BEFORE THE MOVE and answers `None` until there is something to
        /// answer — which is how a daemon's own hook behaves. A fixture that attached it afterwards
        /// would be staging a host that grew a new reader mid-run.
        fn judging_with_the_reader() -> (
            Arc<std::sync::Mutex<sprag_terminal::Workspace>>,
            sprag_terminal::PaneId,
            Arc<std::sync::Mutex<sprag_terminal::Workspace>>,
            crate::access::WorkspacePaneAccess,
            AiLoop,
        ) {
            let (workspace, pane) = standin_agent(9);
            // ⚠⚠⚠⚠⚠ THE OTHER WINDOW IS A `sibling()` OF THIS RUN'S POOL, AND A MUTATION IS WHY
            // THAT MATTERS. A pool built independently draws its own id counter, so its first pane
            // is ALSO id 0 — the run's own id — and a fallback that ignored the id entirely would
            // hand back a stranger's pane and look correct. `sibling` shares `next_id` precisely
            // because *every window of a session must answer from one place*, so this stages the
            // world the product can actually produce (register items 617 / 642).
            let destination = Arc::new(std::sync::Mutex::new(
                workspace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .sibling(),
            ));
            // ⚠⚠ AND IT ALREADY HOLDS SOMEBODY ELSE'S PANE, which is what makes *found the pane*
            // and *found a pane* two different answers in the closed arm below.
            {
                let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
                command.arg("-c");
                command.arg("exec cat");
                destination
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .spawn(command, "a stranger".to_string(), 80, 24)
                    .expect("the other window opens a pane of its own");
            }
            let elsewhere: crate::access::PaneElsewhere = {
                let destination = Arc::clone(&destination);
                Arc::new(move |id| {
                    let holds = destination
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .pane(id)
                        .is_some();
                    holds.then(|| Arc::clone(&destination))
                })
            };
            let access = supervised(&workspace).with_panes_elsewhere(Some(elsewhere));
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let run = RunContext::uncancellable();
            let mut pumped = 0;
            while loops.state() != AiLoopState::Judging && pumped < 40 {
                loops.step(&access, &run).expect("a live pane takes a pass");
                pumped += 1;
            }
            assert_eq!(
                loops.state(),
                AiLoopState::Judging,
                "⚠⚠ THE FIXTURE'S PRECONDITION: this loop must bank a turn within {pumped} passes",
            );
            (workspace, pane, destination, access, loops)
        }

        /// Let the driver take the judging pass, and answer what became of the run.
        fn driven(
            access: &crate::access::WorkspacePaneAccess,
            loops: &mut AiLoop,
        ) -> (OutcomeState, Vec<String>) {
            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 4,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(loops, access, &RunContext::uncancellable());
            let said = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            (outcome.state, said)
        }

        fn held(
            pool: &std::sync::Mutex<sprag_terminal::Workspace>,
        ) -> std::sync::MutexGuard<'_, sprag_terminal::Workspace> {
            pool.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        // ── ARM 1: MOVED — and the run goes on driving the pane it was given ────────────────
        let moved_pane = {
            let (source, pane, destination, access, mut loops) = judging_with_the_reader();
            let taken = held(&source)
                .close(pane)
                .expect("the run's pool held its pane a statement ago");
            held(&destination).adopt(taken);
            assert!(
                held(&source).pane(pane).is_none(),
                "⚠⚠⚠⚠ THE PREMISE: the run's own pool must NOT hold the pane any more, or this arm \
                 measures a run that never lost anything and the hook is never consulted",
            );
            let (ended, said) = driven(&access, &mut loops);
            assert!(
                !said
                    .iter()
                    .any(|note| note.contains(&format!("there is no pane {}", pane.0))),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 682: a run was killed by somebody MOVING ITS PANE between \
                 windows. The pane is open, its program is running, and the only thing that changed \
                 is which window's membership list holds it — a `close` + `adopt` that never \
                 touches the pane. Three of this repository's own runs died exactly here. \
                 Journal: {said:?}",
            );
            assert_ne!(
                ended,
                OutcomeState::Failed,
                "⚠⚠⚠ and it must not have failed for some other reason either — a run that dies \
                 differently is still a run the move killed. Journal: {said:?}",
            );
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
            // ⚠ DROPPED, not ignored: `close` hands the pane BACK, and it is that drop which ends
            // the child this arm moved into the other window.
            drop(held(&destination).close(pane));
            pane
        };

        // ── ARM 2: CLOSED — and the reader must NOT make a dead pane look alive ────────────
        let closed_pane = {
            let (source, pane, _destination, access, mut loops) = judging_with_the_reader();
            drop(
                held(&source)
                    .close(pane)
                    .expect("the run's pool held its pane a statement ago"),
            );
            let (ended, said) = driven(&access, &mut loops);
            assert_eq!(
                ended,
                OutcomeState::Failed,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 682's ESCAPE HATCH: a pane that was CLOSED must still end \
                 the run. A reader that answered *somewhere* for a pane nobody holds would let a \
                 run type into a pane that is gone, and every assertion in arm 1 would still pass \
                 — which is this workspace's rule that an exemption must not disarm its own gate. \
                 Journal: {said:?}",
            );
            assert!(
                said.iter()
                    .any(|note| note.contains(&format!("there is no pane {}", pane.0))),
                "⚠⚠⚠ and with the live runs' own sentence, or this arm ended somewhere else: \
                 {said:?}",
            );
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
            pane
        };

        assert_eq!(
            moved_pane, closed_pane,
            "⚠⚠ THE FIXTURES MUST NAME THE SAME PANE, or the two arms are about two ids rather \
             than about two removals",
        );

        // ── ARM 3: A SLOPPY READER — the hook names a pool that does NOT hold the pane ─────
        //
        // ⚠⚠⚠⚠⚠ THE SECOND BELT, AND A MUTATION IS WHY IT EXISTS. Arm 2 is guarded by the HOOK
        // answering `None`, so it never reaches this surface's own id lookup — replacing that
        // lookup with *whatever this pool holds first* left both arms above green. The production
        // hook is one function (`sprag_host::pools_of`) and a stale or careless one is exactly the
        // shape that would resurrect a closed pane, so the surface must not trust a pool it is
        // handed: the answer is about THIS pane or it is no answer.
        {
            let (source, pane, destination, _access, mut loops) = judging_with_the_reader();
            let sloppy: crate::access::PaneElsewhere = {
                let destination = Arc::clone(&destination);
                Arc::new(move |_| Some(Arc::clone(&destination)))
            };
            let access = supervised(&source).with_panes_elsewhere(Some(sloppy));
            drop(
                held(&source)
                    .close(pane)
                    .expect("the run's pool held its pane a statement ago"),
            );
            assert!(
                held(&destination).pane(pane).is_none() && !held(&destination).panes().is_empty(),
                "⚠⚠⚠⚠ THE PREMISE: the pool this sloppy reader names must hold SOME pane and NOT \
                 this one, or *found the pane* and *found a pane* are the same answer again",
            );
            let (ended, said) = driven(&access, &mut loops);
            assert_eq!(
                ended,
                OutcomeState::Failed,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 682: this surface TRUSTED a pool it was handed and typed \
                 into a stranger's pane. The reader answers *which pool holds this pane*, and a \
                 pool that does not hold it is not an answer — a run that took one would be \
                 driving somebody else's agent, which is worse than the death this repair is \
                 about. Journal: {said:?}",
            );
            for live in access.pane_ids() {
                access.lifecycle().expect("lifecycle").close(live);
            }
            let strangers: Vec<sprag_terminal::PaneId> = held(&destination)
                .panes()
                .iter()
                .map(sprag_terminal::Pane::id)
                .collect();
            for live in strangers {
                drop(held(&destination).close(live));
            }
        }
    }

    /// ⛔⛔⛔⛔ **A RUN PUBLISHES WHERE IT IS AS A FIELD, IN THE DOCUMENT'S OWN WORD** — register
    /// item 543, stage 3a.
    ///
    /// # The channel this fact did not have
    ///
    /// [`a_loop_run_converges_under_the_driver_that_bounds_it`] reads the walk out of
    /// `journal[..].note` with `contains("Judging")` — and that is the honest shape of what was
    /// available: **a substring match on a human sentence**.
    /// Everything a program wanted to know about where a run is had to be recovered that way, from
    /// a journal bounded to [`JOURNAL_LIMIT`](crate::driver::JOURNAL_LIMIT) steps that
    /// `RunRegistry::persistable` deliberately does not save. So the position was unreadable, then
    /// truncated, then gone — and gone precisely at the restart where a person wants it.
    ///
    /// ⚠⚠⚠⚠⚠ **THE WORD IS THE DOCUMENT'S, AND THIS ASSERTS THAT AGAINST THE `.scxml` ITSELF.**
    /// `Plugin::at` hands back SCE's generated `get_state_name`, so what is published is the state's
    /// `id` as written in `ai_loop.scxml` rather than a second spelling of it maintained here. A
    /// hand-written match over the twenty-eight variants would pass every assertion below except
    /// the last one, and would age silently the first time a state was renamed — which is this
    /// crate's own recorded failure shape. Reading the source is what makes that unsayable.
    ///
    /// ⚠⚠ It is also why the word is safe to PERSIST: `sprag_host`'s record carries
    /// [`STATECHARTS_FINGERPRINT`](crate::STATECHARTS_FINGERPRINT) beside it, so a successor daemon
    /// can tell a position in its own vocabulary from one in a document it does not have.
    #[test]
    fn a_run_publishes_where_its_machine_is_in_the_documents_own_word() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        // ⚠ BEFORE THE DRIVER HAS STEPPED IT, the cell says nothing — a position is what a
        // COMPLETED step establishes, and a default that claimed `idle` would be this crate
        // asserting where a run is before anything has asked.
        let progress = ProgressCell::default();
        assert_eq!(
            progress.lock().expect("the progress cell").at,
            None,
            "⚠⚠ an unstarted run must publish no position rather than a plausible one",
        );

        let outcome = Driver::new(Guardrails {
            max_iterations: 40,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &RunContext::uncancellable());
        let at = progress.lock().expect("the progress cell").at;

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠ the control: this fixture must reach the end it is written around, or the position \
             asserted below is some other run's",
        );
        assert_eq!(
            at,
            Some("converged"),
            "⛔⛔⛔⛔ REGISTER ITEM 543: a run must publish WHERE IT IS as a field. `None` means \
             the Driver never asked its plugin, so the only account of a run's position stays a \
             sentence in an unpersisted journal — and an interrupted run goes on being unable to \
             tell *waiting on me* from *killed mid-turn*",
        );

        // ⚠⚠⚠ AND THE WORD IS THE DOCUMENT'S OWN `id`, checked against the file rather than
        // against another copy of the same table. This is what a hand-written state-to-word match
        // would fail — it would answer `"converged"` today and go on answering it after the
        // document renamed the state, which is a gate that agrees with the wrong thing.
        let document = include_str!("ai_loop.scxml");
        let spelled = format!("id=\"{}\"", at.expect("just asserted Some"));
        assert!(
            document.contains(&spelled),
            "⛔⛔⛔⛔ the published position must be spelled the way the DOCUMENT spells it: \
             `ai_loop.scxml` holds no {spelled}, so this word is a second spelling maintained \
             somewhere in this crate and free to drift from the machine it claims to describe",
        );

        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠⚠ **A RUN WHOSE OWN CONTENT FAILS ENDS, AND THE SENTENCE NAMES THE ERROR** — register
    /// item 505, and the gate the whole item exists for.
    ///
    /// # ⚠⚠⚠⚠⚠ The silence this measures the end of
    ///
    /// W3C SCXML 3.12.2 puts an `error.*` on the internal queue and IGNORES it when nothing matches;
    /// W3C 3.8 abandons the rest of the block that raised it. Between them, a document that answered
    /// no error ran on with half of a state's `onentry` executed and **nothing anywhere said so** —
    /// measured 2026-08-20 by mutation, where one unserved-type `<send>` in `priming` made a real run
    /// take eleven eventless passes in `working`, going nowhere, with every other gate in this crate
    /// green. A person watching had a run that looked like a slow agent.
    ///
    /// # ⚠⚠⚠ How a failure is produced without asking the product for one
    ///
    /// [`OuterLoop::break_a_clause`] writes a STRING over `max_turns`, which stands in for an author
    /// editing `<data id="max_turns" expr="'fourty'"/>` into the file — the one party who can put a
    /// value of the wrong shape in this datamodel, since every caller's road is a typed [`Brief`]
    /// field. `judging`'s first guard then evaluates `turns >= max_turns`, SCE lowers `>=` to raw Lua
    /// `>=`, and comparing a number with a string raises. What runs after that is all product: the
    /// engine's raise, the region's `error.execution` edge, the document's own `fault`, and the
    /// driver's reporting.
    ///
    /// ⚠⚠ THREE CLAIMS, and each fails on its own:
    ///
    /// * the run **ENDS** — before this round it sat in `judging` for ever, because a guard that
    ///   cannot be evaluated takes no transition and raises no event the driver knows;
    /// * the run's failure **NAMES THE CLASS** (`error.execution`), which is what says who repairs
    ///   it: the document's own content, not the pane and not the request;
    /// * the walk **NAMES THE STATE** it happened in, which is the other half of the diagnosis and
    ///   is deliberately not copied into the datamodel.
    #[test]
    fn a_run_whose_own_expression_fails_stops_and_says_which_error_it_was() {
        let (workspace, pane) = standin_agent(4);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");

        // ⚠ THE AUTHOR'S BAD EDIT, stood in for. It happens AFTER the brief, because a brief
        // assigns `max_turns` and would overwrite it — which is itself the honest order: an author's
        // file is read before a caller's argument, and this is the value the run ends up holding.
        loops.inner.break_a_clause("max_turns", "fourty");

        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            // ⚠⚠ THE CONTROL FOR THE WHOLE CLAIM. A run that does NOT answer its own error stalls,
            // and a stall ends `exhausted — iterations` at this ceiling: the two outcomes are what
            // this gate tells apart, so the ceiling must be reachable within the clock below.
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
        access.lifecycle().expect("lifecycle").close(pane);

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⚠⚠⚠⚠⚠ a document whose own guard cannot be evaluated must END the run — and the \
             control was MEASURED rather than imagined: with the region's `error.execution` edge \
             deleted, this very run came back `Converged`. The budget guard failed three times, so \
             `max_turns` never applied at all, and a run under a broken ceiling reported SUCCESS. \
             That is worse than the stall item 505 was filed about, and it is what this word is \
             holding the line on. Walked {walk:?}",
        );
        let said = format!("{:?}", outcome.failure);
        assert!(
            said.contains("error.execution"),
            "⚠⚠⚠⚠ AND THE SENTENCE MUST NAME THE CLASS. Without it the reader is told a loop was \
             `Undrivable` and has to guess between a pane that went away, a peer that would not be \
             asked and a clause that did not evaluate — three unrelated repairs: {said:?}",
        );
        assert!(
            said.contains("stopping"),
            "⚠⚠ and that this document ANSWERS such an error by stopping, which is the decision the \
             `.scxml` now carries and the reason the run is over rather than stuck: {said:?}",
        );
        assert!(
            said.contains("Judging"),
            "⚠⚠⚠ AND IT MUST NAME THE STATE THE ERROR HAPPENED IN. Measured on this gate's first \
             run: the JOURNAL cannot carry it — a failing step returns `Err`, so the walk line is \
             dropped, and the line would have named the driver's own `Judge` rather than the \
             engine's error anyway. The class with no place to look is half a diagnosis: {said:?} \
             — walked {walk:?}",
        );

        // ── ⚠⚠⚠⚠⚠ AND THE DOOR REGISTER ITEM 511 OPENED CARRIES THIS, NOT JUST A ZERO ──
        //
        // `AiLoop::fault` and `AiLoop::swallowed` exist so a LIVE run can be asked what its own
        // document raised, and every live gate that asks reads the healthy answer — `None`. A
        // delegation guarded only by `assert_eq!(x, None)` is guarded by nothing: a body replaced
        // with a bare `None` leaves all three of those gates green. This is the run where the
        // reading is NOT empty, so it is the only one that can tell the door from a constant.
        assert_eq!(
            loops.fault().as_deref(),
            Some("error.execution"),
            "⚠⚠⚠⚠⚠ THE DOOR MUST REACH THE MACHINE. This run's document answered its own failing \
             guard by stopping, so the class is there to be read — and an `AiLoop` is what every \
             live gate holds, so a door that does not carry it here carries nothing there either, \
             silently and for ever. Walked {walk:?}",
        );
        assert_eq!(
            loops.fault(),
            loops.inner.fault(),
            "⚠⚠⚠ and the two spellings of one fact must agree: a second authority on which error \
             stopped a run is the failure register item 505 built `fault` to end, not one to \
             re-create at the door. Walked {walk:?}",
        );
        // ⚠⚠ `swallowed` is the COUNTERPART and reads `None` here on purpose: this document DOES
        // answer `error.execution`, so nothing was left unanswered and nothing cascaded. Register
        // item 509 is the open debt for making that reading FIRE in a shipped document, and it is
        // that item's to pay — what is asserted here is that the door reaches the same machine.
        assert_eq!(
            loops.swallowed(),
            loops.inner.swallowed(),
            "⚠⚠⚠ and item 511's other door must reach the same machine — {:?} at the door against \
             {:?} behind it. Walked {walk:?}",
            loops.swallowed(),
            loops.inner.swallowed(),
        );
        assert_eq!(
            loops.swallowed(),
            None,
            "⚠⚠ a run whose error the document ANSWERED must have swallowed nothing: a reading \
             here would mean the `error.execution` edge had stopped covering the state that \
             raised, which is the shape register item 509 is about. Walked {walk:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A RUN WHOSE AGENT WRITES SOMEWHERE NOTHING CAN READ SAYS SO, ONCE** — register item
    /// 431(a), the half its own done-when called *"nothing fails LOUDLY"*.
    ///
    /// # ⚠⚠⚠ What the silence was
    ///
    /// `context`/`cold`/`floor` degrade to `0` together, and `0` means *do not decide on this* — which
    /// reads exactly like a healthy session that has not been billed yet. So a loop whose agent
    /// states a path this host cannot read reported the same three numbers, every turn, for its whole
    /// life, and the register's own sentence about the defect it already paid for applies again: **a
    /// zero is a number that could not be read, not a small one.**
    ///
    /// ⚠⚠ The stated path is what makes it a FAULT rather than a guess having failed (431's paid
    /// half): the agent said where it writes, so a read that fails is the deployment being wrong — a
    /// container, another host, a hook reporting a path relative to somewhere else — and none of that
    /// is diagnosable by anybody who is not told WHICH FILE.
    ///
    /// # ⚠⚠ ONCE, which is the other half of the claim
    ///
    /// The record is read every judged turn, so a sentence per turn would fill a bounded journal with
    /// one fact — item 277 measured exactly that, where ~99,987 looks erased the transition that
    /// explained a whole ending. The driver hands it over TAKEN, so the walk carries it on one step.
    ///
    /// ⚠ The control is the same run under a supervisor that states nothing: no record is named, so
    /// nothing failed to be read, and a sentence there would be about a file nobody mentioned.
    #[test]
    fn a_record_the_run_could_not_read_is_named_in_the_walk_once() {
        /// The one clause a reader is owed — asserted rather than the whole sentence, so a reword
        /// does not fail this gate while a SILENCE does.
        const SAYS: &str = "could read that file";

        let walk_of = |record: Option<&std::path::Path>| {
            let (workspace, pane) = standin_agent(2);
            let access = match record {
                Some(record) => crate::testing::supervised_writing(&workspace, record),
                None => supervised(&workspace),
            };
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let progress = ProgressCell::default();
            // ⚠ The OUTCOME is not this gate's subject — the walk is — but it is read rather than
            // discarded, because a run that failed for some other reason would leave a walk this
            // gate would then be reading as evidence about a record.
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &access, &RunContext::uncancellable());
            assert_eq!(
                outcome.state,
                OutcomeState::Converged,
                "this run must converge, or its walk is about something else: {outcome:?}",
            );
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            access.lifecycle().expect("lifecycle").close(pane);
            walk
        };

        // ⚠ NEVER CREATED, and under a directory that is not there either — so no ordering of this
        // suite can accidentally make it readable.
        let missing = std::env::temp_dir()
            .join(format!("sprag-unread-{}", std::process::id()))
            .join("never-written.jsonl");
        let told = walk_of(Some(&missing));
        let said: Vec<&String> = told.iter().filter(|note| note.contains(SAYS)).collect();

        assert_eq!(
            said.len(),
            1,
            "⚠⚠⚠⚠⚠ ITEM 431(a): a run that cannot read the record its agent NAMED must say so — \
             exactly once, because it is one broken record and not one per turn. Walked {told:?}",
        );
        assert!(
            said[0].contains(&missing.display().to_string()),
            "⚠⚠⚠ AND IT MUST NAME THE FILE. The remedy is to go and look at it, and a reader who is \
             not told where cannot: {:?}",
            said[0],
        );

        // ⚠⚠⚠ THE CONTROL: the same run, the same peer, and a supervisor that states no record. Zeros
        // here mean *nothing has been billed yet*, which is not a fault — a sentence would send a
        // reader after a file nobody ever mentioned.
        let untold = walk_of(None);
        assert!(
            !untold.iter().any(|note| note.contains(SAYS)),
            "⚠⚠ nothing was named, so nothing failed to be read: {untold:?}",
        );
    }

    /// ⛔⛔⛔⛔ **THE WALK SAYS WHICH TURNS PRODUCED NOTHING** — register item 719, and the reader
    /// this half is for is the PERSON watching a long loop.
    ///
    /// # ⚠⚠⚠ Why the line was not already enough, measured on a real walk
    ///
    /// `Working --TurnDone--> Judging` is what the journal said for every one of the **110
    /// iterations in 51 minutes** item 719 is made of, on a run that committed nothing and left the
    /// tree unchanged. It is the same line a turn that finished a milestone writes. So the whole
    /// diagnosis of that run had to be done from OUTSIDE it — by counting commits — and the number
    /// that would have answered it was being parsed out of the agent's own record on every one of
    /// those turns and thrown away.
    ///
    /// ⚠⚠ The sibling gate `a_turn_that_produced_nothing_is_told_apart_from_one_that_did` holds the
    /// VERDICT and the datamodel key at the driver's own layer. This one holds the SENTENCE, and it
    /// is not a re-spelling: the note is composed a layer up, in [`AiLoop::walked`], out of a field
    /// that has to cross [`Pumped::Moved`] to get there — and a clause dropped anywhere on that road
    /// is invisible to a gate that reads the pump.
    ///
    /// # ⚠⚠ Two arms differing in ONE fact, and the control is the interesting one
    ///
    /// The same peer, the same brief, the same record on disk — and in the moving arm one more
    /// billed request is appended between the turns. A control that merely stated no record would
    /// prove nothing here: it would be silent for the ordinary reason (`Made::Unmeasured` says
    /// nothing at all), so a driver that had lost the clause entirely would pass it.
    #[test]
    fn the_walk_says_which_turns_produced_nothing() {
        /// The clause a reader is owed — asserted rather than the whole sentence, so a reword does
        /// not fail this gate while a SILENCE does. `a_record_the_run_could_not_read_is_named_in_
        /// the_walk_once`'s rule, one fact over.
        const EMPTY: &str = "THIS TURN PRODUCED NOTHING";
        /// And the clause the productive turn owes, on the same rule.
        const WORKED: &str = "tokens of output";
        /// What the moving arm's extra request writes. ⚠ Not `1`: a count that could be confused
        /// with the fixture's own per-request output would let a wrong reader look right.
        const WROTE: u64 = 37;

        let sample = crate::testing::MEASURED_HERE;
        let still = sample.transcript();
        let grown = sample.after_a_turn_producing(WROTE);

        /// Step one arm until the peer has ended two turns, and hand back every note it wrote.
        fn walk_of(record: &std::path::Path, grow_to: Option<&str>) -> Vec<String> {
            use crate::plugin::Plugin as _;

            // ⚠ The peer answers many prompts before its marker, so nothing it says ends these
            // runs before the second turn this gate is about.
            let (workspace, pane) = standin_agent(9);
            let access = crate::testing::supervised_writing(&workspace, record);
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let run = RunContext::uncancellable();
            let mut walk: Vec<String> = Vec::new();
            let mut turns = 0_usize;
            // ⚠⚠ STEPPED BY HAND RATHER THAN THROUGH THE `Driver`, and that is the one thing this
            // gate needs that its neighbour does not: an agent's record GROWS WHILE IT WORKS, and
            // the only way to stage that between two turns is to be holding the loop between them.
            while walk.len() < 40 && turns < 2 {
                let step = loops.step(&access, &run).expect("the pane stays readable");
                let Some(note) = step.note else {
                    continue;
                };
                let ended = note.contains("--TurnDone-->");
                walk.push(note);
                if !ended {
                    continue;
                }
                turns += 1;
                // ⚠ THE ONE THING THE ARMS DIFFER BY, applied after the first turn has been judged
                // and before the second ends.
                if turns == 1
                    && let Some(text) = grow_to
                {
                    std::fs::write(record, text).expect("the record the session is writing");
                }
            }
            access.lifecycle().expect("lifecycle").close(pane);
            assert_eq!(
                turns, 2,
                "⚠⚠⚠ THE PREMISE: the peer must really ANSWER, twice. A run that ended no turn \
                 has nothing for either clause to be said about, and both assertions below would \
                 pass on the silence: {walk:?}",
            );
            walk
        }

        let home = std::env::temp_dir().join(format!("sprag-walk-produced-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("a directory to file the record in");
        let stuck_at = home.join("what-the-stuck-session-said.jsonl");
        let moving_at = home.join("what-the-working-session-said.jsonl");
        std::fs::write(&stuck_at, &still).expect("the agent's own record");
        std::fs::write(&moving_at, &still).expect("the agent's own record");

        let stuck = walk_of(&stuck_at, None);
        let moving = walk_of(&moving_at, Some(&grown));
        let _ = std::fs::remove_dir_all(&home);

        let named: Vec<&String> = stuck.iter().filter(|note| note.contains(EMPTY)).collect();
        assert_eq!(
            named.len(),
            1,
            "⛔⛔⛔⛔ REGISTER ITEM 719: the turn whose agent wrote nothing must be NAMED in the \
             walk, and exactly once — there was one such turn. Every line of the 110-iteration run \
             this item is made of read `Working --TurnDone--> Judging`, which is also what a turn \
             that finished the work writes. Walked {stuck:?}",
        );
        assert!(
            named[0].contains("--TurnDone-->"),
            "⚠⚠ and it must be said ON THE EDGE THAT ENDED THE TURN, not carried onto a later step \
             — a verdict about one turn printed on the next is R396's thirteen identical lines: \
             {:?}",
            named[0],
        );
        assert!(
            !stuck.iter().any(|note| note.contains(WORKED)),
            "⚠⚠ and this arm's agent produced nothing at all, so no turn of it may claim output: \
             {stuck:?}",
        );

        // ⛔⛔ THE CONTROL, and it is what makes the claim above attributable: the same run whose
        // agent DID write something must say so, in the same place, rather than being silent.
        let worked: Vec<&String> = moving.iter().filter(|note| note.contains(WORKED)).collect();
        assert_eq!(
            worked.len(),
            1,
            "⛔⛔⛔ the productive turn must say what it produced. Telling two facts apart by the \
             ABSENCE of a sentence is the reading this workspace has burned wire numbers over — and \
             a driver that had lost this clause altogether would pass the claim above. Walked \
             {moving:?}",
        );
        assert!(
            worked[0].contains(&WROTE.to_string()),
            "⚠⚠⚠ AND IT MUST BE THE DIFFERENCE, not the session's total: the record held {} tokens \
             of output before this turn and {WROTE} more after it, so a reader answering with the \
             total would say {} here: {:?}",
            crate::spend::spend_in(&still).produced,
            crate::spend::spend_in(&grown).produced,
            worked[0],
        );
        assert!(
            !moving.iter().any(|note| note.contains(EMPTY)),
            "⚠⚠ and no turn of the moving arm may be called empty: {moving:?}",
        );
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
        //
        // ⚠⚠⚠ AND THE POPULATION IS THE CEILINGS THAT ASK FOR AN ACCOUNT, WHICH IS ASSERTED RATHER
        // THAN ASSUMED — register item 534. `Ceiling::Hold` reaches its ending with nobody at the
        // pane and asks nothing, so it has no clause; skipping it silently would let a LATER
        // account-asking ceiling be skipped the same way, so the skip is a claim the loop below
        // checks against `asks_for_an_account` and refuses to make on its own.
        for ceiling in Ceiling::ALL {
            let Some(needle) = crate::testing::stop_said(ceiling) else {
                assert!(
                    !ceiling.asks_for_an_account(),
                    "⚠⚠⚠ {ceiling:?} is asked for an account and the fixture has no clause for it, \
                     so `stopping` composes `nil` into a live agent's prompt — register item 264's \
                     measured failure, arriving through the door item 534 opened"
                );
                continue;
            };
            for other in Ceiling::ALL {
                let Some(theirs) = crate::testing::stop_said(other) else {
                    continue;
                };
                assert!(
                    ceiling == other || !needle.contains(theirs),
                    "⚠⚠ the fixture's needles must be mutually exclusive, or the assertions below \
                     cannot tell one ceiling's clause from another: {needle:?} contains {theirs:?}",
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
            // ⚠⚠ EVERY ROW HERE DRIVES A CEILING THAT ASKS FOR AN ACCOUNT — the table above holds
            // only guardrails and the document's budget — so a row whose clause is missing is a
            // table that grew a ceiling reaching `stopping` without one. Register item 534.
            let clause = crate::testing::stop_said(ceiling).unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠ this table drives {ceiling:?} into `stopping`, so it MUST have a clause: \
                     `Ceiling::asks_for_an_account` says {}, and a run asked `nil` is item 264's \
                     measured failure",
                    ceiling.asks_for_an_account()
                )
            });
            assert!(
                asked.contains(clause),
                "⚠⚠⚠ REGISTER ITEM 264: a run stopped by its {ceiling:?} ceiling was asked \
                 {asked:?}, which does not name that ceiling. This sentence is TYPED INTO THE \
                 AGENT'S PANE in the turn that asks it what a run picking this up should do first, \
                 so the agent reasons from whatever it says. Expected the clause {clause:?}",
            );
            for other in Ceiling::ALL {
                // ⚠ A ceiling with no clause cannot be named by a prompt, so there is nothing to
                // find absent — and skipping it silently is safe here BECAUSE the premise loop at
                // the top of this test already refused the case where that skip would hide a defect.
                let Some(theirs) = crate::testing::stop_said(other) else {
                    continue;
                };
                assert!(
                    other == ceiling || !asked.contains(theirs),
                    "⚠⚠⚠ AND IT NAMED A CEILING THAT DID NOT STOP IT. A run stopped by \
                     {ceiling:?} was asked {asked:?}, which carries {other:?}'s clause \
                     ({theirs:?}) — the exact defect item 264 is about, since the agent cannot \
                     check.",
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
    /// # ⛔⛔⛔⛔⛔ THIS SECTION WAS WRONG, AND IT WAS WRONG IN THE DIRECTION THAT COSTS ROUNDS
    ///
    /// **Re-measured 2026-08-23 by the same mutation it names: restoring
    /// `Over::Asking(_) => return Null` turns this gate RED, at 2000 steps.** The arm IS reached,
    /// by this fixture, on this path — the walk goes `Working --TurnBlocked--> Screening
    /// --ScreenNone--> AwaitingHuman`, and `attend`'s `Completion::wait` then answers `Asking`
    /// because the dialog is still up.
    ///
    /// ⚠⚠⚠⚠⚠ **THE PARAGRAPH BELOW IS KEPT AS THE RECORD OF A CLAIM THAT AGED**, because that is
    /// the lesson worth more than the correction. It said *"which was measured, not assumed"* —
    /// and a measurement is a fact about the tree it was taken in. Nothing made it announce that
    /// it had stopped being one, and it sat here telling every later round *do not bother, this
    /// fixture cannot reach that arm*. Register item 416's shape, inside a gate's own doc.
    ///
    /// ⚠ Whether it was false when written or became false since cannot be settled from here, and
    /// saying so is the honest form: what IS settled is the reading at this pin.
    ///
    /// # ⚠⚠⚠ (SUPERSEDED) WHAT THIS GATE DOES **NOT** HOLD, AND THE ARM IT CANNOT REACH
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

    /// ⛔⛔⛔⛔ **AND IT MUST NOT RE-READ THE PANE WHILE IT WAITS** — register item 280, the half of
    /// the owner's question its neighbour above cannot reach.
    ///
    /// # ⚠⚠⚠⚠ THE QUESTION THAT WAS STILL UNANSWERED
    ///
    /// The register's head records three questions, each one layer under the last: *"with no input
    /// it should just WAIT"*, then *"why 100,000 in one hour?"*, then **"why is it LOOKING at all?
    /// isn't the looking itself the defect?"** Item 279 answered the middle one — the wait is ONE
    /// step now, and [`a_run_waiting_for_a_person_spends_one_step_on_the_whole_wait`] holds it.
    ///
    /// **The last one was never answered, and no gate could see it.** A step count says nothing
    /// about what happens inside a step: [`poll_until`](crate::run::poll_until) re-evaluates its
    /// predicate every [`POLL_INTERVAL`](crate::run::POLL_INTERVAL) — ten milliseconds — and the
    /// predicates a wait on a pane is built from render that pane's screen and run a detector over
    /// it. One step of the shipped hour of patience is **three hundred and sixty thousand screen
    /// reads**, against ~100,000 driver rounds before the repair. The number the owner asked about
    /// did not fall; it moved down one layer and grew.
    ///
    /// ⚠⚠ **AND THE COST IS NOT THE READER'S ALONE.** `WorkspacePaneAccess` takes the workspace
    /// mutex for every one of them, and `PaneForegroundJob`'s own comment already measured what a
    /// holder polling at this interval does to everyone else: a concurrent reader's median went
    /// from 0.8 us to 687 us and its p99 to 41.8 ms. A run waiting politely for a person is,
    /// underneath, a hundred lock acquisitions a second on the structure every client reads.
    ///
    /// # ⚠⚠⚠ WHY IT IS TWO WAITS AND NOT ONE, and why a single ceiling would not be a claim
    ///
    /// A ceiling on one wait is a TOLERANCE — it can be met by making [`POLL_INTERVAL`] longer,
    /// which is the cleverer-cadence repair item 280 refuses by name (*"a cleverer interval is
    /// **also** logic nobody wrote down"*). So this measures the same wait at two patiences a
    /// factor of four apart and asserts the count does not follow the clock. **A driver that polls
    /// fails on the RATIO however slowly it polls**; only a wait that ends on the pane MOVING is
    /// flat in the duration.
    ///
    /// ⚠ The absolute ceiling is asserted too, and it is the weaker of the two on purpose: a
    /// measurement whose two arms both happened to be enormous would satisfy a ratio.
    ///
    /// ⚠⚠ **THE CONTROL IS THAT THE WAIT HAPPENED AT ALL** — both arms must reach `awaiting_human`
    /// and must last at least the patience they were promised. A run that skips the wait looks
    /// exactly like a run that parks through it, counted in looks alone, and it is the opposite
    /// defect: a person promised an hour who was given no time at all.
    ///
    /// # ⚠⚠⚠ MEASURED, 2026-08-23, both sides of the repair
    ///
    /// | patience | looks BEFORE | looks AFTER |
    /// |---|---|---|
    /// | 400 ms | 43 | **5** |
    /// | 1,600 ms | 157 | **5** |
    ///
    /// Before, the count followed the clock at 98 a second, which is
    /// [`POLL_INTERVAL`](crate::run::POLL_INTERVAL) and nothing about the pane; after, it is the
    /// same number in both arms. On the hour of patience `ai_loop.scxml` ships that is ~353,000
    /// screen reads against five. ⚠ `CEILING` is set three times the reading rather than at it:
    /// the RATIO is this gate's claim and the ceiling is its backstop, so leaving a loaded runner
    /// room to paint one extra frame costs nothing a poll could hide in.
    ///
    /// # ⛔⛔⛔ WHAT THIS GATE CANNOT SEE, AND THE GATE THAT CAN — established by mutation
    ///
    /// **A park that never wakes passes every assertion here.** Deleting the revision bump from
    /// `sprag_terminal`'s pty reader leaves this GREEN: the wait costs no looks precisely because
    /// it has stopped noticing anything, both arms still last their patience, and the count is
    /// still flat. That is the WORST regression this repair can have and it is invisible from
    /// here, because *cheap* and *deaf* look identical when all you count is looks.
    ///
    /// The gate that answers it is
    /// [`readiness::tests::a_person_who_answers_is_not_waited_out_by_the_supervisors_own_hysteresis`](crate::readiness),
    /// which types a real keystroke at a real dialog and asserts the wait comes back. Under the
    /// same mutation it fails with *"nobody came in 10s"* — about a person who answered.
    ///
    /// ⚠⚠ **SO THE PROPERTY IS HELD BY A PAIR AND BY NEITHER ALONE**, and it is written here
    /// rather than left to be rediscovered: this one says the wait is cheap, that one says it is
    /// awake, and a repair that satisfied only one of them would be a defect wearing a green tick.
    #[test]
    fn a_run_waiting_for_a_person_does_not_re_read_the_pane_while_it_waits() {
        /// ⚠ Far above the two patiences below, so neither wait can end on the TURN's clock — the
        /// distinction its neighbour above paid register item 297 to learn.
        const TURN_BOUND: Duration = Duration::from_secs(8);
        /// The short arm's patience.
        const SHORT: Duration = Duration::from_millis(400);
        /// The long arm's, four times it. ⚠ The FACTOR is what this gate reads, not either number:
        /// a polling wait costs four times as many looks here, and a parked one costs the same.
        const LONG: Duration = Duration::from_millis(1_600);
        /// How many looks a wait may cost however long it lasts. A parked wait looks when it
        /// arrives and when it leaves; this leaves room for a handful more without leaving room
        /// for a poll.
        const CEILING: u64 = 16;
        /// What the long arm may exceed the short one by. ⚠ NOT ZERO, and not a fraction: two
        /// live panes are not bit-identical, and a fixed small slack keeps a real difference of
        /// one or two looks from being read as a cadence. At the poll interval the gap between
        /// these two arms is ~120 looks, so nothing this size can hide a poll.
        const SLACK: u64 = 8;

        /// Walk a loop to `awaiting_human` over a peer that stopped to ask, then wait it out —
        /// answering **how many looks the WAIT cost** and **how long it took**.
        ///
        /// ⚠ The count is taken as a DIFFERENCE across the wait, so everything the walk to the
        /// state spent is somebody else's number.
        fn waited_out(patience: Duration, turn: Duration) -> (u64, Duration, String) {
            let (workspace, pane) = crate::testing::standin_agent_asking(
                crate::testing::Asks::OnItsFirstPromptAfterWorking,
            );
            let access =
                crate::testing::Counted::new(crate::testing::supervised_asking(&workspace));
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &Brief {
                    await_person_ms: Some(patience.as_millis() as i64),
                    handback_still_ms: Some(50),
                    turn_within_ms: Some(turn.as_millis() as i64),
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
            let reached = loops.state();
            let entered = access.looks();
            let began = std::time::Instant::now();
            let mut spent = 0_u32;
            while loops.state() == AiLoopState::AwaitingHuman && spent < 2_000 {
                loops
                    .step(&access, &run)
                    .expect("a waiting run is still readable");
                spent += 1;
            }
            let took = began.elapsed();
            let looked = access.looks() - entered;
            access.lifecycle().expect("lifecycle").close(pane);
            assert_eq!(
                reached,
                AiLoopState::AwaitingHuman,
                "⚠ the control: the loop must be WAITING for a person before its looking can be \
                 counted. Walked {walked:?}",
            );
            (looked, took, format!("{walked:?}"))
        }

        let (short_looks, short_took, short_walk) = waited_out(SHORT, TURN_BOUND);
        let (long_looks, long_took, long_walk) = waited_out(LONG, TURN_BOUND);

        // ── the control: both arms really waited, so there is a wait to have looked during ──
        assert!(
            short_took >= SHORT && long_took >= LONG,
            "⚠⚠⚠ NEITHER ARM MAY SKIP THE WAIT — a run that gives a person no time at all costs \
             no looks either, and would pass every assertion below while committing the opposite \
             defect. short {short_took:?} of {SHORT:?} ({short_walk}); long {long_took:?} of \
             {LONG:?} ({long_walk})",
        );
        assert!(
            long_took < TURN_BOUND,
            "⚠⚠ and both must leave on the PERSON's clock rather than the turn's — {long_took:?} \
             is past the {TURN_BOUND:?} turn bound, so this measured a different wait",
        );

        // ── the claim: LOOKING DOES NOT FOLLOW THE CLOCK ──
        assert!(
            long_looks <= short_looks + SLACK,
            "⚠⚠⚠⚠⚠ THE WAIT IS RE-DERIVING THE SAME UNCHANGED PANE. A {LONG:?} wait cost \
             {long_looks} looks where a {SHORT:?} one cost {short_looks} — the count is following \
             the CLOCK, so this run is polling a screen rather than waiting for it to move. On the \
             shipped hour of patience that rate is ~360,000 screen reads and as many takes of the \
             workspace lock, for a person who has not touched the keyboard. Register item 280, and \
             the owner's own question: *why is it LOOKING at all?*",
        );
        assert!(
            long_looks <= CEILING && short_looks <= CEILING,
            "⚠⚠⚠ AND A WAIT COSTS A HANDFUL OF LOOKS, NOT HUNDREDS. short {short_looks}, long \
             {long_looks}, ceiling {CEILING}. ⚠ This is the weaker of the two assertions — two \
             enormous arms can satisfy a ratio — and it is here for exactly that case",
        );
    }

    /// ⛔⛔⛔ **A RUN A PERSON IS HOLDING MUST NOT SPEND ITS ITERATION BUDGET WHILE THEY HOLD IT** —
    /// register item 522, measured on a live run and on no fixture until this one.
    ///
    /// # What it cost, measured rather than argued
    ///
    /// `sprag hold-run` parks a run in `awaiting_human` — immediately, which is `ai_loop.scxml`'s
    /// `hold` transition on `working` and not a turn boundary — and twenty-four minutes later run
    /// 18 was over: **`exhausted (iterations)`**, its walk from step 99,938 to the end reading
    /// `AwaitingHuman: looked, nothing had happened`. A state whose document declares
    /// `await_person_ms = 1 hour` killed the run in less than half of one, and the number that did
    /// it was `--max_iterations 100000` — the ceiling the loop's own launch recipe recommends.
    ///
    /// ⚠⚠⚠⚠⚠ **THE HOLD IS THE ONE ORDER A PERSON CAN TAKE BACK.** A hold that ends the run is not
    /// a slower `cancel`; it is a worse one, because `resume` was still on the table and the person
    /// was told to expect it. Three surfaces said it waited — the CLI's own sentence, the loop
    /// skill, and the datamodel's hour — while the driver spent the budget underneath all three.
    ///
    /// # ⚠⚠⚠ Why its neighbour below cannot hold this, and why neither can hold both
    ///
    /// [`a_run_waiting_for_a_person_spends_one_step_on_the_whole_wait`] measures the same
    /// arithmetic on a DIFFERENT arm — `Over::Asking`, reached because a peer stopped to ask. A
    /// held run never reaches it: `attend` reads [`RunContext::held`] FIRST and returns before any
    /// wait at all, which it is entitled to do (their patience must not run while they hold it) and
    /// which is exactly the shape the asking arm was fixed out of. **The two fixes are independent,
    /// so killing either leaves the other's gate green** — measured, and the reason this is its own
    /// gate rather than a second assertion on that one.
    ///
    /// ⚠⚠ WHAT THIS GATE ASSERTS IS STEPS, NOT ELAPSED TIME, for that neighbour's reason: a driver
    /// that spun for exactly as long would pass a duration assertion. The wait must be ONE wait.
    /// The elapsed time is asserted too, because one step and no waiting is the opposite defect —
    /// a run that carried on underneath the person who held it.
    #[test]
    fn a_held_run_does_not_spend_an_iteration_on_every_look() {
        /// The document's patience, far longer than the hold below, so *left because the person let
        /// go* and *left because their patience ran out* are different numbers.
        const PATIENCE: Duration = Duration::from_secs(30);
        /// How long the person keeps it. Long enough that a spinning driver takes hundreds of steps
        /// to cross it, short enough to keep the gate cheap.
        const HELD_FOR: Duration = Duration::from_millis(500);
        /// ⚠ Far above the two steps a parked run may take, and low enough that a spinning driver
        /// — measured at ~69 looks a second on the live run — trips it in seconds rather than
        /// minutes.
        const GIVE_UP_AFTER: u32 = 200;

        let (workspace, pane) = standin_agent(4);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                // ⚠⚠ A PERSON IS EXPECTED HERE. `brief_for` says nobody is, and a run nobody
                // attends takes `unattended` the moment it parks — an ending, where this gate is
                // about a run that must NOT end.
                await_person_ms: Some(PATIENCE.as_millis() as i64),
                handback_still_ms: Some(50),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");

        // ⚠ The hold is the HOST's flag — raised and lowered by whoever ran `sprag hold-run` — so
        // the gate holds the same `Arc` the run reads, which is how a person is staged at all.
        let hold = Arc::new(AtomicBool::new(false));
        let run = RunContext::uncancellable().held_by(Arc::clone(&hold));

        let mut walked: Vec<String> = Vec::new();
        for _ in 0..40 {
            if loops.state() == AiLoopState::Working {
                break;
            }
            let step = loops
                .step(&access, &run)
                .expect("every step of a starting run must be readable");
            if let Some(note) = step.note {
                walked.push(note);
            }
        }
        assert_eq!(
            loops.state(),
            AiLoopState::Working,
            "⚠ the control: the run must be WORKING before a person can hold it — `hold` is a \
             transition of `working` and of nowhere else, so a gate that held a run somewhere else \
             would measure a pass that never parked. Walked {walked:?}",
        );

        // ── THE PERSON HOLDS IT, READS THE PANE, AND LETS IT GO ──
        // ⚠⚠ ARMED BEFORE THE FIRST MEASURED STEP, because that step is the one that carries the
        // order in AND parks: a releasing thread spawned after it would be waited out by the very
        // park being measured.
        hold.store(true, Ordering::Release);
        let releasing = {
            let hold = Arc::clone(&hold);
            std::thread::spawn(move || {
                std::thread::sleep(HELD_FOR);
                hold.store(false, Ordering::Release);
            })
        };

        let began = Instant::now();
        let mut spent = 0_u32;
        loop {
            loops
                .step(&access, &run)
                .expect("a held run is still readable");
            spent += 1;
            if loops.state() != AiLoopState::AwaitingHuman || spent >= GIVE_UP_AFTER {
                break;
            }
        }
        let took = began.elapsed();
        releasing.join().expect("the person's own thread");

        assert!(
            spent <= 2,
            "⛔⛔⛔ A HELD RUN IS SPENDING ITS ITERATION BUDGET ON LOOKING. Crossing a {HELD_FOR:?} \
             hold took {spent} steps over {took:?}, so the driver is re-deriving an unchanged \
             screen instead of parking on the one condition that ends a hold — the person letting \
             go. Every one of those steps is an iteration charged against a ceiling the document \
             cannot see, and on run 18 that arithmetic ended the run: `exhausted (iterations)` \
             after 24 minutes, at ~69 looks a second, under an `await_person_ms` of one hour.",
        );
        assert!(
            took >= HELD_FOR,
            "⚠⚠⚠ AND THE HOLD MUST ACTUALLY HAVE HELD. The run left after {took:?} of a \
             {HELD_FOR:?} hold, which is the opposite defect and just as wrong: a person who was \
             promised the run would wait got one that carried on underneath them.",
        );
        // ⚠⚠⚠⚠ **AND IT MUST HAVE LEFT ON THE PERSON'S HAND, NOT ON THEIR CLOCK** — the assertion
        // this gate passed its own mutation without. A park that waits out the whole
        // `await_person_ms` and only THEN looks at the order spends two steps, which is everything
        // the count above asks for, and leaves a person who lifted the hold watching a dead pane
        // for the rest of the shipped HOUR. Only a bound an order of magnitude above the hold can
        // tell *the person let go* from *the patience ran out* — which is why `PATIENCE` and
        // `HELD_FOR` are set that far apart, exactly as the gate below sets its own two.
        assert!(
            took < PATIENCE,
            "⚠⚠⚠⚠ THE RUN CAME BACK ON THE PATIENCE, NOT ON THE ORDER. {took:?} is past the \
             {HELD_FOR:?} the hold actually lasted and at the {PATIENCE:?} the document allows a \
             person, so the park is not watching the hold at all — it is sleeping out its bound \
             and noticing afterwards. `resume-run` would look ignored for the rest of that hour.",
        );
        assert_ne!(
            loops.state(),
            AiLoopState::AwaitingHuman,
            "⚠⚠ AND THE HOLD MUST COME OFF. `resume` is what makes this the one order a person can \
             take back; a run still parked after the flag dropped is a `cancel` wearing a kinder \
             word.",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔ **A HOLD NOBODY EVER LIFTS MUST REACH AN ENDING, AND ONE LIFTED IN TIME MUST NOT** —
    /// register item 534, which is the gate above's own residue and the reason it is a separate
    /// number rather than a paragraph inside a closed entry.
    ///
    /// # What the fix above left behind, measured rather than argued
    ///
    /// Item 522 stopped a held run burning its iteration budget, by parking it on the one condition
    /// that ends a hold: the person letting go. What it did not give it was a way to END if they
    /// never do. Three facts compose into a run that is immortal:
    ///
    /// * a held run's patience is deliberately NOT spent (the gate above asserts exactly that);
    /// * `unattended` is refused for it by the document's own `cond="!In('held')"`;
    /// * [`Guardrails::max_duration`] is an `Option`, so a run launched without one has no deadline
    ///   for `poll_until` to answer — and `max_iterations` cannot bound a step that never returns.
    ///
    /// **So before this ceiling existed a run held by somebody who then went home parked on its
    /// pane, holding a daemon slot, until a person cancelled it by hand.** Worse than the defect it
    /// replaced in one respect a reader must not lose: the old behaviour at least ENDED, wrongly and
    /// loudly, at 24 minutes.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the pair is the claim and neither arm is a fact about this machine
    ///
    /// Either assertion alone is satisfiable by a driver that is simply wrong in the other
    /// direction. A `hold` that ends the run the moment it arrives passes the first arm and is a
    /// `cancel` wearing a kinder word — the exact thing this document spent a whole state refusing.
    /// A `hold` bounded by `Duration::MAX` passes the second and is the defect. **The two arms differ
    /// in ONE thing: whether the person comes back inside the ceiling.**
    ///
    /// ⚠⚠⚠⚠ AND THE POPULATION IS THE UNATTENDED RUN, WHICH IS THE WHOLE OF ITEM 534. Both arms are
    /// briefed with nobody watching (`brief_for`'s `await_person_ms: 0`), because that is the shape
    /// that parked for ever: with a person declared the wait is at least re-taken every hour, and
    /// with `Attended::NoOne` the bound was `Duration::MAX` outright. A gate that declared a
    /// supervisor would be measuring the case that was less broken.
    ///
    /// ⚠⚠⚠ IT IS DRIVEN THROUGH THE `Driver` AND NOT PUMPED BY HAND, because the ending's WORD is
    /// half the repayment: a held run used to report `exhausted — iterations`, sending its reader to
    /// raise a step budget that would have bought it nothing. `Ceiling::Hold` is what makes the
    /// sentence true, and only a real run produces it.
    #[test]
    fn a_hold_nobody_lifts_ends_the_run_and_one_lifted_in_time_does_not() {
        /// How long the document lets a hold last, in the arm where nobody comes back. Short enough
        /// to keep the gate cheap, and an order of magnitude above the sub-millisecond passes the
        /// stand-in takes so the ending is the ceiling's rather than a scheduling accident.
        const CEILING: Duration = Duration::from_millis(300);
        /// The ceiling in the arm where somebody DOES come back — far above the hold below, so
        /// *left because the person let go* and *left because the ceiling fell due* are different
        /// numbers rather than one number read twice. The gate above's `PATIENCE`/`HELD_FOR` split.
        const ROOMY: Duration = Duration::from_secs(30);
        /// How long the person keeps it in that arm. Well inside `ROOMY`.
        const HELD_FOR: Duration = Duration::from_millis(150);

        // ── ARM ONE: NOBODY EVER COMES BACK ──
        //
        // ⚠⚠ HELD FROM THE FIRST STEP, and that is not a shortcut past the *between turns* rule —
        // it is the driver's own arrangement being used as documented. `hold` is raised on EVERY
        // held pass precisely because a raise consumed in `idle`, where the document has no word
        // for being held, was the first draft's defect; so the order lands the moment `working`
        // exists, with no thread and no race for this gate to lose.
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                hold_within_ms: Some(CEILING.as_millis() as i64),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            // ⚠⚠⚠ `max_duration` IS `None` **ON PURPOSE, AND IT IS THIS ARM'S CONTROL**: a run
            // WITH a deadline would have ended on it eventually, so a gate that named one could
            // not tell this ceiling from that one. It is also the shipped shape — `sprag
            // orchestrate` requires no `max_seconds`, which is exactly why item 534 could happen.
            //
            // ⚠⚠⚠⚠ AND `max_iterations` IS SMALL **SO THE MUTATION FAILS FAST**, which is measured
            // rather than guessed. A correct run parks and ends in under ten passes. Under the
            // defect this gate exists for — a ceiling re-measured from each pass instead of from
            // the moment the hold began — every pass waits the whole ceiling out, so a
            // `max_iterations` of 4,000 turns a red into TWENTY MINUTES of green-looking CI
            // (measured: `rc=124` at a 400-second timeout). Forty passes bound that to about
            // twelve seconds and land it as `Exhausted(Iterations)`, which is a different word
            // from the one asserted and therefore a legible failure rather than a hang.
            max_iterations: 40,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&progress))
        .run(
            &mut loops,
            &access,
            &RunContext::uncancellable().held_by(Arc::new(AtomicBool::new(true))),
        );
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Hold),
            "⛔⛔⛔⛔ REGISTER ITEM 534: a run held by somebody who never came back did not end as \
             abandoned. It answered {:?} instead. With no `max_duration` and no patience there is \
             nothing else left to end it, so a run that reaches here by any other word either \
             parked for ever (the defect) or was ended by a ceiling that says something false \
             about why. Walked {walk:?}",
            outcome.state,
        );
        assert_eq!(
            loops.state(),
            AiLoopState::Abandoned,
            "⚠⚠⚠ AND THE DOCUMENT MUST AGREE WITH THE RUN'S WORD, or the two are counting \
             different things — the `orders` region reached its own final and `Self::state` fell \
             back to the flattened configuration, which is the mechanism `blocked` already relies \
             on. Walked {walk:?}",
        );
        assert!(
            Ceiling::Hold.describe().contains("did not come back"),
            "⚠⚠ AND THE SENTENCE A PERSON READS MUST SAY WHAT HAPPENED. The whole complaint against \
             the old ending was its prose, not its arithmetic: `exhausted — iterations` is a true \
             sentence about a step budget and a false one about this run.",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // ── ARM TWO: THE SAME ORDER, TAKEN BACK IN TIME ──
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                hold_within_ms: Some(ROOMY.as_millis() as i64),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");
        let hold = Arc::new(AtomicBool::new(true));
        let releasing = {
            let hold = Arc::clone(&hold);
            std::thread::spawn(move || {
                std::thread::sleep(HELD_FOR);
                hold.store(false, Ordering::Release);
            })
        };
        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 4_000,
            max_cost: None,
            max_duration: None,
        })
        .reporting_to(Arc::clone(&progress))
        .run(
            &mut loops,
            &access,
            &RunContext::uncancellable().held_by(Arc::clone(&hold)),
        );
        releasing.join().expect("the person's own thread");
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
            "⛔⛔⛔⛔ AND A HOLD IS STILL THE ONE ORDER A PERSON CAN TAKE BACK. Held for \
             {HELD_FOR:?} under a {ROOMY:?} ceiling and let go, this run answered {:?} — so the \
             ceiling is ending runs whose person DID come back, which makes `hold` a `cancel` that \
             took the scenic route and undoes the whole reason the order exists. Walked {walk:?}",
            outcome.state,
        );
        assert_ne!(
            loops.state(),
            AiLoopState::Abandoned,
            "⚠⚠ and the document must not be sitting in the ending either",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔⛔ **AN OUTAGE ENDS A RUN ON THE OUTAGE BUDGET, NEVER ON THE ITERATION ONE** —
    /// register item 746, and the half [`crate::outer::OuterLoop::wait_out_service`]'s own gate
    /// cannot reach: which CEILING a real run comes to rest on.
    ///
    /// # ⚠⚠⚠⚠ What was measured, and why the arithmetic had a third factor nobody could see
    ///
    /// Register item 724 gave the self-resuming door a budget of `service_resumes_max` (36) waits
    /// of `service_retry_ms` (10 minutes) — **six hours**, and ratcheted the product so the two
    /// files cannot drift apart. Run 58, 2026-08-29, hit an account limit and died at
    /// `03:23:14 exhausted (iterations) after 100000 iterations` having spent **two** of those
    /// thirty-six. Forty-eight minutes of outage, seven per cent of the design.
    ///
    /// The third factor is `max_iterations`: not in either of item 724's two files, not in any
    /// document at all, but a **wire argument the launcher passes**. So the six hours that ratchet
    /// defends were being silently cut to whatever number the caller happened to write, and the
    /// item's own note forbids the obvious answer — *raise it* is not a repair, because nobody can
    /// say what it should be.
    ///
    /// # ⚠⚠⚠ Two arms, differing in ONE thing, and what each alone would let through
    ///
    /// * **THE HEADLINE — an outage the budget covers is SURVIVED.** The peer's service goes out
    ///   mid-turn, comes back inside the budget, and the run reaches its milestone. Alone, this
    ///   passes for a driver with no ceiling at all.
    /// * ⚠⚠⚠⚠ **AND ONE THAT OUTLASTS THE BUDGET ENDS ON `service_resumes_max`.** Same peer, same
    ///   numbers, same run — it simply never speaks again — and the ending must be `blocked`
    ///   through `awaiting_human`, which is the edge item 724 built. `exhausted` here is the very
    ///   defect: it names a knob the caller can raise and says nothing about a service that never
    ///   came back.
    ///
    /// # ⚠⚠⚠⚠⚠ THE PREMISE IS ASSERTED INSIDE, because a roomy ceiling passes both arms
    ///
    /// A gate whose `max_iterations` is generous is green over the unfixed driver too, and it would
    /// go on being green for ever — the vacuity item 657 named. So the ceiling is asserted to be
    /// **below what the outage window is worth in polls**: `RETRIES × WINDOW` at
    /// [`POLL_INTERVAL`](crate::run::POLL_INTERVAL) is what a driver that merely LOOKED once per
    /// poll would spend crossing this outage, and the unfixed one spent far more than that because
    /// it did not sleep at all. ⚠ It is a LOWER bound on the defect's cost and that is the honest
    /// direction: if a ceiling this tight survives, no looser reading of the spin can have happened.
    ///
    /// ⚠⚠ The other direction is asserted too — the ceiling must be roomy enough that the arms are
    /// about the OUTAGE and not about a run that could never have finished anything. That is what
    /// the surviving arm reaching `converged` says.
    ///
    /// ⚠ The two numbers with no wire key are AUTHORED, which is what `with_bound` in
    /// [`crate::outer`]'s own gates does and for its reason: the shipped silence bound is ten
    /// minutes and the shipped budget is thirty-six waits, so a gate that inherited them would not
    /// terminate. What it must not do is restate the CEILING, and it does not — that is read off
    /// the running document, so nobody can make these arms vacuous by editing it.
    #[test]
    fn an_outage_ends_a_run_on_the_outage_budget_and_never_on_the_iteration_one() {
        /// How long this gate's document lets nothing speak. Generous next to a stand-in's
        /// delivery, because the peer has to get its outage message onto the pane before the bound
        /// falls due — the control below is what says it did.
        const QUIET: Duration = Duration::from_millis(500);
        /// The document's retry window. It is what the outage COSTS in wall clock, so it is also
        /// what the premise assertion divides by [`crate::run::POLL_INTERVAL`].
        const WINDOW: Duration = Duration::from_millis(400);
        /// How much of the budget each arm has left when it meets its outage — the same in both,
        /// because the arms must differ in exactly one thing.
        const RETRIES_LEFT: i64 = 3;
        /// How long the service is out in the arm that survives: about one cycle of the loop's own
        /// `quiet + window`, comfortably inside `RETRIES_LEFT` of them.
        const OUTAGE: Duration = Duration::from_millis(900);
        /// ⚠⚠ THE CEILING BOTH ARMS RUN UNDER, and the number this whole gate is about. Measured:
        /// a surviving run costs about twenty passes here. It is asserted below to be under what
        /// the outage is worth in polls, so it cannot be quietly raised into vacuity.
        const ITERATIONS: u32 = 60;

        /// Run a real loop, under a real Driver, against a peer whose service fails mid-turn —
        /// `recovers_after` being the one thing the two arms differ in.
        fn outage(
            recovers_after: Option<Duration>,
        ) -> (OutcomeState, Vec<String>, Duration, i64, u32) {
            let (workspace, pane) =
                crate::testing::standin_agent_whose_service_fails(recovers_after);
            let access = supervised(&workspace);
            let mut loops = AiLoop::new(
                engine(),
                pane,
                &Brief {
                    // ⚠ THE PEER'S OWN WORDS, spelled once in the fixture and quoted here — the
                    // needle is what a KIND authors, so a gate that invented one would be arming a
                    // channel against a sentence nothing ever prints.
                    service: Some(crate::outer::ServiceOutage {
                        needles: vec![crate::testing::SERVICE_IS_DOWN.to_string()],
                        every_ms: WINDOW.as_millis() as u64,
                        text: "continue".to_string(),
                    }),
                    ..brief_for(40)
                },
                &standin_spec(),
            )
            .expect("a well-briefed loop over a live pane starts");
            // ⚠⚠ AUTHORED AFTER THE BRIEF, which is where the document's own numbers stand: these
            // two have no wire key by design (see `Quiet::DOCUMENT_KEY` and `service_resumes_max`),
            // and the shipped ten minutes and thirty-six waits are a gate that never returns.
            loops.inner.author_number(
                crate::completion::Quiet::DOCUMENT_KEY,
                QUIET.as_millis() as i64,
            );
            // ⚠⚠⚠ THE CEILING IS READ, NEVER WRITTEN — item 724's own discipline. An arm that
            // spelled `36` here would keep passing the day somebody lowered the budget, which is
            // exactly the stale fixture this register keeps paying for.
            let ceiling = loops
                .inner
                .reads_number("service_resumes_max")
                .expect("the document must declare `service_resumes_max` as a number");
            loops
                .inner
                .author_number("service_retried", ceiling - RETRIES_LEFT);

            let progress = ProgressCell::default();
            let began = Instant::now();
            let outcome = Driver::new(Guardrails {
                max_iterations: ITERATIONS,
                max_cost: None,
                // ⚠ Far above what either arm takes, so the wall clock is never the ending: this
                // gate is about which of the OTHER two ceilings a run comes to rest on.
                max_duration: Some(Duration::from_secs(120)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &access, &RunContext::uncancellable());
            let took = began.elapsed();
            let walk: Vec<String> = progress
                .lock()
                .expect("the progress cell")
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            access.lifecycle().expect("lifecycle").close(pane);
            (outcome.state, walk, took, ceiling, outcome.iterations)
        }

        /// **THE CLAIM, IN ONE LINE: THE COUNT FOLLOWS THE RETRIES AND NOT THE CLOCK.** A run that
        /// spent fewer iterations than its own elapsed time is worth in [`crate::run::POLL_INTERVAL`]s
        /// cannot have been asking in a loop — which is the arithmetic register item 522's own gate
        /// makes about looks, made here about the budget that actually ended run 58.
        fn slower_than_polling(iterations: u32, took: Duration) -> bool {
            u128::from(iterations) * crate::run::POLL_INTERVAL.as_millis() < took.as_millis()
        }

        // ── THE PREMISE, ASKED OF THE NUMBERS BEFORE EITHER ARM LEANS ON THEM ──
        //
        // ⚠⚠⚠⚠⚠ What a driver that merely LOOKED once per poll interval would spend crossing this
        // outage. The unfixed one spent far more — it returned without sleeping at all — so this is
        // a floor on the defect's cost and the honest direction to be conservative in.
        let spun = (WINDOW.as_millis() as i64 * RETRIES_LEFT)
            / crate::run::POLL_INTERVAL.as_millis() as i64;
        assert!(
            i64::from(ITERATIONS) < spun,
            "⚠⚠⚠⚠⚠ THE CEILING IS TOO ROOMY FOR THIS GATE TO MEAN ANYTHING. {ITERATIONS} \
             iterations against an outage worth at least {spun} polls — so a driver that spun \
             through the whole wait would still have finished inside the budget, and both arms \
             below would pass over the very defect item 746 registered. Widen the outage or \
             tighten the ceiling; do NOT raise `max_iterations`, which is the workaround the item \
             forbids by name",
        );

        // ── THE HEADLINE: an outage the budget covers is survived ──
        let (survived, walk, took, ceiling, spent) = outage(Some(OUTAGE));
        assert!(
            ceiling > RETRIES_LEFT,
            "⚠⚠⚠ THE PREMISE OF BOTH ARMS: the document's budget must be bigger than the headroom \
             this gate leaves, or `service_retried` was authored to a negative number and neither \
             arm is about a ceiling at all. Ceiling {ceiling}, headroom {RETRIES_LEFT}",
        );
        // ⚠⚠⚠⚠ THE CONTROL, AND WITHOUT IT THIS GATE MEASURES A HAPPY RUN. The peer's outage
        // message has to be ON THE PANE before the silence bound falls due, or `peer.silent`
        // carries `service: false`, the run goes to `awaiting_human` and ends `blocked` — which is
        // the second arm's expected ending, reached for entirely the wrong reason.
        assert!(
            walk.iter().any(|note| note.contains("ServiceDown")),
            "⚠⚠⚠⚠⚠ THE RUN NEVER MET AN OUTAGE, so nothing below is about one. The peer prints its \
             service message and then stops speaking; if the {QUIET:?} silence bound falls due \
             before that message is painted, the driver reads a plain silence instead. Walked \
             {walk:?}",
        );
        assert_eq!(
            survived,
            OutcomeState::Converged,
            "⛔⛔⛔⛔⛔ AN OUTAGE INSIDE THE BUDGET MUST BE SURVIVED. The peer's service was out for \
             {OUTAGE:?} — about one of the {ceiling} waits item 724 budgeted — and came back, and \
             this run answered {survived:?} instead of reaching its milestone. `exhausted \
             (iterations)` here is run 58 exactly: 48 minutes of outage, 100,000 iterations, two \
             retries spent. Walked {walk:?} in {took:?} over {spent} iteration(s)",
        );
        assert!(
            slower_than_polling(spent, took),
            "⚠⚠⚠⚠⚠ THE COUNT IS FOLLOWING THE CLOCK. {spent} iterations in {took:?} is at least \
             one per {:?}, which is what ASKING looks like rather than what waiting looks like — \
             and on the shipped ten-minute window that rate is the hundred thousand run 58 spent. \
             The claim of item 746 is that an outage costs one iteration per `service_retry_ms`, \
             not one per poll. Walked {walk:?}",
            crate::run::POLL_INTERVAL,
        );

        // ── ⚠⚠⚠⚠ AND THE OTHER ARM: AN OUTAGE THAT OUTLASTS THE BUDGET ENDS ON THE BUDGET ──
        let (gave_up, walk, took, _, spent) = outage(None);
        assert!(
            walk.iter()
                .any(|note| note.contains("ServiceDown --ServiceRetry--> AwaitingHuman")),
            "⚠⚠⚠⚠⚠ THE RUN DID NOT END ON THE OUTAGE'S OWN CEILING. That pair is the only way out \
             of `service_down` when the budget is spent — item 724 built it, and a morning reader \
             meeting it knows a server was down and nobody came. Walked {walk:?} in {took:?}",
        );
        // ⚠⚠ THE VARIANT AND NOT ITS PAYLOAD. `AiLoop::asking` stands an `Unanswered` in for the
        // two doors into `awaiting_human` that never held a question — its own doc says so — and
        // what this arm is about is which CEILING ended the run, not what the sentence beside it
        // carries.
        assert!(
            matches!(gave_up, OutcomeState::Blocked(_)),
            "⛔⛔⛔⛔⛔ A PEER THAT NEVER CAME BACK MUST BE REPORTED AS ONE, and this run answered \
             {gave_up:?}. `exhausted` sends its reader to raise `max_iterations` about a service \
             that is down — the sentence run 58 published — where `blocked` sends them to look at \
             the peer. Walked {walk:?}",
        );
        // ⚠⚠⚠ AND THE PREMISE OF THE ARM ABOVE, MEASURED RATHER THAN ARGUED: this run really did
        // spend long enough that the unfixed driver would have blown the ceiling. Without it the
        // arm could pass over a run that gave up in milliseconds for some unrelated reason.
        assert!(
            took.as_millis() as i64 >= WINDOW.as_millis() as i64 * RETRIES_LEFT,
            "⚠⚠⚠⚠ THE RUN DID NOT ACTUALLY WAIT OUT ITS BUDGET. {took:?} is less than the \
             {RETRIES_LEFT} waits of {WINDOW:?} the document owes an outage before it gives up, so \
             the ending above was reached without the waiting this gate is about",
        );
        assert!(
            slower_than_polling(spent, took),
            "⚠⚠⚠⚠ AND THE ARM THAT GAVE UP SPENT ITS BUDGET AT THE DOCUMENT'S RATE TOO. {spent} \
             iterations in {took:?} is one per {:?} or faster, so this run reached the outage \
             ceiling by luck of a roomy `max_iterations` rather than because waiting is cheap. \
             Walked {walk:?}",
            crate::run::POLL_INTERVAL,
        );
    }

    /// ⚠⚠⚠⚠ **THE TURN'S OWN READ NOTICES A DIALOG THE BARRIER MISSED** — `Over::Asking`, the arm
    /// register item 297 called unreachable through four fixtures.
    ///
    /// # ⚠⚠⚠ Why its neighbour above cannot hold this, and why no peer can
    ///
    /// [`OuterLoop::watch`] asks the readiness barrier FIRST and consults the turn's `Completion`
    /// only when that says nothing — and the barrier's `peer_asking` fires on `Blocked` alone where
    /// `Completion::asked` also demands the addressed agent and a moved sequence. The barrier's
    /// condition is a strict SUPERSET, so a dialog that is up when a pass begins is always caught
    /// there. Two mutations proved the arm is nonetheless live: stub the pre-check and the
    /// neighbouring gate arrives through this arm (green); stub it AND kill the arm and it goes red.
    ///
    /// So the arm is a RACE-WINDOW handler — it fires when the peer stops to ask **between** the
    /// two reads of one pass — and staging that needs a supervisor that answers the two reads
    /// differently, which is [`DialogBetweenTheReads`](crate::testing::DialogBetweenTheReads).
    ///
    /// ⚠⚠ What this gate holds is the ROUTE, not the wait: that the loop notices a peer which went
    /// quiet asking after its barrier had already passed, and blocks rather than spinning out the
    /// turn's whole bound. The wait itself is its neighbour's.
    #[test]
    fn a_peer_that_asks_after_the_barrier_passed_is_still_noticed() {
        let (workspace, pane) = crate::testing::standin_agent(u32::MAX);
        let (dialog, access) = crate::testing::DialogBetweenTheReads::over(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                await_person_ms: Some(200),
                handback_still_ms: Some(50),
                // ⚠ Long, so a run that fell through to `Over::NotYet` would spin here rather than
                // block — the difference this gate is about.
                turn_within_ms: Some(30_000),
                ..brief_for(1_000_000)
            },
            &standin_spec(),
        )
        .expect("a well-briefed loop over a live pane starts");

        let run = RunContext::uncancellable();
        // Under way with its barrier passed and nothing asking.
        for _ in 0..8 {
            if loops.state() == AiLoopState::Working {
                break;
            }
            loops.step(&access, &run).expect("a working run steps");
        }
        assert_eq!(
            loops.state(),
            AiLoopState::Working,
            "⚠ the control: the loop must be WORKING with its barrier behind it before a dialog \
             raised inside a pass can be about the turn's own read",
        );

        // THE WINDOW: from here the next supervisor read still answers *working* — the barrier's —
        // and every read after it carries the dialog.
        dialog.raise();
        let mut walked: Vec<String> = Vec::new();
        let mut passes = 0_u32;
        for _ in 0..8 {
            if loops.state() != AiLoopState::Working {
                break;
            }
            passes += 1;
            if let Some(note) = loops.step(&access, &run).expect("a blocked run steps").note {
                walked.push(note);
            }
        }

        assert_ne!(
            loops.state(),
            AiLoopState::Working,
            "⚠⚠⚠⚠ THE TURN'S READ NOTICED NOTHING. Walked {walked:?}",
        );
        assert_eq!(
            passes, 1,
            "⚠⚠⚠⚠ IT TOOK {passes} PASSES, AND THE ARM IS WORTH EXACTLY ONE. `Over::Asking` exists \
             so the pass that MEETS the dialog is the pass that raises it; without it the barrier \
             of the NEXT pass catches the same dialog and the run blocks one pump later. That is \
             the whole difference — measured, not assumed — and this number is the only thing that \
             can tell the two routes apart, since both end with the run out of `working`. \
             Walked {walked:?}",
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
                reflect_every: Some(8),
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
            NotStarted::NoTurns,
            "⚠⚠ THE REFUSAL MUST NAME THE KNOB, and it is a NUMBER rather than a state. It carried \
             `AiLoopState::Exhausted` until R100, which cost two copies of the topology — this \
             construction and a `match` in `sprag-host` reading the state back to pick a sentence \
             — for a variant only ever built one way",
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

    /// ⚠⚠⚠⚠ **A DECLINED BUDGET IS THE DOCUMENT'S OWN, AND THIS IS WHERE THE NUMBER IS VISIBLE** —
    /// item 312, the half `sprag-host`'s door gate cannot make.
    ///
    /// That door answers *the call is accepted*; it holds no datamodel, so it cannot say WHICH
    /// budget a declining caller got. Here the session exists, and the claim is arithmetic: send a
    /// brief with `max_turns: None` and the run is bounded by `ai_loop.scxml`'s own `expr="40"` —
    /// the number that was unreachable from every caller while the key was required.
    ///
    /// ⚠⚠⚠ **AND `reflect_every` FOLLOWS IT**, which is the coupling that made this item more than
    /// a type change. Its default IS the budget (equal makes `reflecting` unreachable, since
    /// `judging` tests the turn budget first), so it could be defaulted at the daemon's door only
    /// while the budget was mandatory there. Both resolve together now, and a caller who declines
    /// both must land on `40 / 40` rather than on `40` against a stale zero.
    ///
    /// ⚠⚠ **THE CONTROL IS THE POINT OF THE PAIR**: a caller who names a budget must still get
    /// theirs, or *"the document decides"* would have quietly become *"the document overrides"*.
    #[test]
    fn a_declined_budget_is_the_documents_own() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);

        let deferred = AiLoop::new(
            engine(),
            pane,
            &Brief {
                max_turns: None,
                reflect_every: None,
                ..brief_for(3)
            },
            &standin_spec(),
        )
        .expect("⚠⚠⚠ ITEM 312: a caller may decline the budget and let the document decide");
        assert_eq!(
            deferred.inner.turn_budget(),
            Some(crate::outer::Counted::Of(40)),
            "⚠⚠⚠⚠ the declining caller must be bounded by the DOCUMENT's own `expr=\"40\"`. Any \
             other number means the resolution invented one, which is the failure this item is \
             about wearing a different face",
        );
        assert_eq!(
            deferred.inner.authored_number("reflect_every"),
            Some(40),
            "⚠⚠⚠ and `reflect_every` must have followed the budget it defaults to, not been left \
             at whatever the document ships — an unequal pair reflects when nobody asked",
        );

        // ⚠⚠⚠ THE CONTROL. Without it, a product that ignored `max_turns` outright and always used
        // the document's would satisfy every assertion above.
        let named = AiLoop::new(engine(), pane, &brief_for(3), &standin_spec())
            .expect("a caller who names a budget is obeyed");
        assert_eq!(
            named.inner.turn_budget(),
            Some(crate::outer::Counted::Of(3)),
            "a caller's own number must still win over the document's",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠⚠ **A CALLER'S CAPACITY CEILING REACHES THE DATAMODEL, AND A DECLINED ONE IS THE
    /// DOCUMENT'S OWN** — register item 492, and the arithmetic half `sprag-host`'s door cannot
    /// make.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this gate is the whole item
    ///
    /// `reviewing` guards every deciding edge on `context_ceiling > 0`, and until this round
    /// **nothing could make that number anything but 0**: the template ships `0` on purpose, the
    /// kind document had authored `800000` since 2026-08-18 with nobody to read it, and there was
    /// no `Brief` field, no wire key and no `<assign>`. Item 477's live measurement is the far end
    /// of that — **eight of eight** `reviewing` exits took the fall-back, which is that state never
    /// once deciding in 97 iterations.
    ///
    /// ⚠⚠⚠ So the assertion is not *the resolution prefers the caller*. It is **the number arrives
    /// at all**, which is the thing that was never true.
    ///
    /// ⚠⚠ **THE CONTROL IS THE POINT OF THE PAIR**, exactly as it is for the budget one door up: a
    /// product that ignored the field and always used the document's would satisfy the first half
    /// alone, and one that always took the caller's would satisfy the second.
    #[test]
    fn a_capacity_ceiling_crosses_from_the_caller_and_a_declined_one_is_the_documents() {
        /// A number no document in this tree ships, so *carried* and *defaulted* are different
        /// answers — the fixture rule its neighbours are written under.
        const NAMED: i64 = 424_242;
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);

        let carried = AiLoop::new(
            engine(),
            pane,
            &Brief {
                context_ceiling: Some(NAMED),
                ..brief_for(3)
            },
            &standin_spec(),
        )
        .expect("a caller naming a ceiling starts a run");
        assert_eq!(
            carried.inner.authored_number("context_ceiling"),
            Some(NAMED),
            "⚠⚠⚠⚠⚠ ITEM 492: the caller's ceiling must REACH the datamodel `reviewing` reads. \
             `None` here is the defect this item is about — a state guarded on a number nothing \
             could set",
        );

        // ⚠⚠⚠ THE CONTROL. Without it a product that took the caller's number and ALSO overwrote a
        // declining caller's with it would pass everything above.
        let deferred = AiLoop::new(
            engine(),
            pane,
            &Brief {
                context_ceiling: None,
                ..brief_for(3)
            },
            &standin_spec(),
        )
        .expect("a caller declining a ceiling starts a run too");
        let documents = deferred
            .inner
            .authored_number("context_ceiling")
            .expect("⚠ the template declares the key, so a declining caller reads a number");
        assert_ne!(
            documents, NAMED,
            "⚠⚠⚠ a declining caller must NOT be handed the last caller's ceiling — the echo exists \
             so the document's own number survives, not so a number leaks between runs",
        );
        assert_eq!(
            documents, 0,
            "⚠⚠ and the TEMPLATE's own number is 0, which is its stated decision: a caller who has \
             not thought about capacity is not given a number somebody guessed for them. ⚠ A kind \
             document is what changes this, and it is read at the daemon's door rather than here — \
             see `sprag_plugin::kind`'s own gate",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠⚠ **AND THE PATIENCE A REFUSING CHECK IS GIVEN CROSSES THE SAME WAY** — register item
    /// 494, the twin of the gate above it.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a twin gate and not one gate over two numbers
    ///
    /// Because the DEFECT was a twin and nobody saw the second half. The template says *"it is the
    /// KIND's to author, like `max_turns` and `reflect_every`"* about exactly two of its `<data>`,
    /// item 492 measured one of them and built its whole road, and the identical defect stayed
    /// standing on the other for a further round — reader, field, key and `<assign>` all absent,
    /// with a `#[cfg(test)]` constant and a `set_variable` in a gate for its only writer. **A
    /// premise that produces one defect produces the rest of its class**, and a gate per instance
    /// is what says which instances have actually been paid.
    ///
    /// ⚠⚠ The pair-with-a-control shape is its neighbour's and for its reason: a product that
    /// ignored the field would satisfy the first half alone, and one that always took the caller's
    /// would satisfy the control.
    #[test]
    fn a_callers_refusal_patience_crosses_and_a_declined_one_is_the_documents() {
        /// A number no document in this tree ships — the template's is 3 and the debt kind's is 2 —
        /// so *carried* and *defaulted* cannot be the same answer.
        const NAMED: i64 = 7;
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);

        let carried = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_after_refusals: Some(NAMED),
                ..brief_for(3)
            },
            &standin_spec(),
        )
        .expect("a caller naming a refusal patience starts a run");
        assert_eq!(
            carried.inner.authored_number("reflect_after_refusals"),
            Some(NAMED),
            "⚠⚠⚠⚠⚠ ITEM 494: the caller's patience must REACH the datamodel `judging` reads. \
             `None` here is the defect this item is about — a number the template invites a kind to \
             author and nothing but a test's own `set_variable` could ever write",
        );

        // ⚠⚠⚠ THE CONTROL, on the ceiling gate's terms: a product that took the caller's number and
        // also overwrote a declining caller's with it would pass everything above.
        let deferred = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_after_refusals: None,
                ..brief_for(3)
            },
            &standin_spec(),
        )
        .expect("a caller declining a patience starts a run too");
        let documents = deferred
            .inner
            .authored_number("reflect_after_refusals")
            .expect("⚠ the template declares the key, so a declining caller reads a number");
        assert_ne!(
            documents, NAMED,
            "⚠⚠⚠ a declining caller must NOT be handed the last caller's patience — the echo \
             exists so the document's own number survives, not so a number leaks between runs",
        );
        assert_eq!(
            documents, 3,
            "⚠⚠ and the TEMPLATE's own number is 3, which item 449 argued and item 448 changed the \
             ground under: three refusals used to be three turns spent answering a question nobody \
             had asked, and they are three INFORMED attempts now. ⚠ A kind document is what \
             changes this — this repository's says 2 — and it is read at the daemon's door rather \
             than here, so `sprag_plugin::kind`'s own gate holds that half",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠ **A DECLINED BUDGET REACHES THE DATAMODEL AS A WORD, AND THE RUN STARTS** — the
    /// resolution half of the kind's *"this loop does not end on turns"*.
    ///
    /// # ⚠⚠⚠ What this holds and what it deliberately does not
    ///
    /// It holds the RESOLUTION: a brief carrying [`Counted::Never`] leaves the document holding the
    /// word rather than a number, and `AiLoop::new`'s *at least 1* check — which refuses a budget of
    /// zero — lets it through. Those are two different refusals that were one before: *no turns at
    /// all* is a run that can only judge itself exhausted, and *never bounded on turns* is an author
    /// saying the run ends some other way.
    ///
    /// ⚠⚠ **IT DOES NOT HOLD THE DOOR'S WIRING**, and that was measured rather than assumed:
    /// deleting `.or_else(|| kind.turn_budget())` from `plugins.rs` left the entire workspace
    /// GREEN. What would catch it is an observable of the RESOLVED budget on a run started through
    /// the wire, and `turn_budget` is crate-private — so the residue was registered rather than
    /// papered over with a gate that re-implements the line it is checking.
    ///
    /// ✅ **AND THAT RESIDUE IS PAID (register item 492), so this paragraph is history rather than a
    /// warning.** The observable it asked for exists: `sprag_host`'s door resolves a `Brief` and
    /// hands it back (`ai_loop_brief`), which is `pub` and is exactly what the wire produces, so
    /// `a_kind_documents_judgements_reach_a_run_that_named_none_of_them` holds all EIGHT
    /// fall-throughs — the budget included. **Re-measured on that round: the same deletion now goes
    /// red.** ⚠ Kept rather than deleted because the shape it names recurs: a wiring nothing can
    /// observe is a wiring nothing holds, and the fix was to hand the value back instead of
    /// consuming it in place.
    ///
    /// ⚠ The cadence is named here for the reason the kind names one: with no budget there is no
    /// number for reflection to borrow, and the driver refuses the pair rather than guessing.
    #[test]
    fn a_declined_budget_crosses_as_a_word_and_the_run_is_not_refused() {
        let (workspace, pane) = crate::testing::standin_agent(9);
        let access = supervised(&workspace);
        let unbounded = AiLoop::new(
            engine(),
            pane,
            &Brief {
                max_turns: Some(crate::outer::Counted::Never),
                reflect_every: Some(5),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect(
            "⚠⚠⚠⚠ a run whose author declined the turn ceiling must START. Before the decline \
             existed the budget was refused unless it was a number of at least one, so *never* and \
             *zero* met the same door — and the second is a run that can only judge itself \
             exhausted before its agent has answered anything",
        );
        assert_eq!(
            unbounded.inner.turn_budget(),
            Some(crate::outer::Counted::Never),
            "⚠⚠⚠⚠ AND THE DOCUMENT MUST HOLD THE WORD, not a number the resolution invented. The \
             template's guard reads `max_turns != 'never'` before it compares, so a number here \
             would restore the ceiling the author declined — silently, and only visible as a run \
             that stopped mid-milestone saying its budget was spent",
        );

        // ⚠⚠⚠ THE CONTROL: declining the budget must not become declining every budget. Without
        // this, a resolution that answered `Never` for everybody would satisfy the assertion above.
        let bounded = AiLoop::new(engine(), pane, &brief_for(7), &standin_spec())
            .expect("a caller who names a number is still obeyed");
        assert_eq!(
            bounded.inner.turn_budget(),
            Some(crate::outer::Counted::Of(7)),
            "a named budget must survive the decline being possible",
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
                        .input_trail()
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
            .input_trail()
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
            // ⚠⚠ AND THE REPLACEMENT NAMES ITS OWN DOOR, exactly, because this fixture arranged
            // one: no `context_ceiling` is authored, so neither of `reviewing`'s two questions can
            // be asked and the run takes the fall-back — register item 445, held here as well as in
            // its own gate, since a run replaced for either of the other two reasons would satisfy
            // every other assertion in this test.
            format!(
                "Reviewing --ReviewNone--> Restarting — {}",
                crate::outer::RestartReason::NobodyCouldSay.noted()
            ),
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
                reflect_every: Some(1),
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

    /// ⚠⚠⚠⚠ **THE REFLECTION IS HANDED THE FOUR NUMBERS A SPLIT DECISION NEEDS** — and they are
    /// numbers rather than a threshold, which is the whole design.
    ///
    /// # ⚠⚠⚠ Why this is carried and not guarded on
    ///
    /// A restart pays for itself only above roughly 15,000-21,000 tokens of discardable context,
    /// and re-measurement put this loop's own eight-turn cadence at 18,736 — INSIDE the band. A
    /// threshold placed anywhere in a band the workload sits inside decides by rounding while
    /// reading as a measured policy. So the document composes the quantities into the prompt and
    /// the agent being asked what to do next weighs them. `ai_loop.scxml`'s `context` entry argues
    /// it at length; this gate is what holds the argument to a mechanism.
    ///
    /// # ⚠⚠⚠⚠ What is actually being measured here, which is NOT the wording
    ///
    /// The four values cross a Lua datamodel as NUMBERS into a string concatenation, and this
    /// document had never done that before — every other composed prompt joins strings. The
    /// generator emits `_scxml_add(<string>, context)`, which READS correct and proves nothing:
    /// this repository has already paid twice for a generated expression that compiled, ran, and
    /// silently did nothing. **A failed `<assign>` leaves the previous value**, so the sentence
    /// simply would not be there and no other gate in this suite would notice.
    ///
    /// # ⚠⚠⚠⚠ WHAT THIS GATE DOES NOT HOLD, MEASURED BY MUTATING IT RATHER THAN ASSUMED
    ///
    /// The fixture's session has no transcript, so all four values read `0` — the degraded reading,
    /// which the document names in the same sentence (*"a zero is a number that could not be
    /// read"*). **At zero a live read and a literal are the same text**, and two mutations proved
    /// it: replacing `+ floor +` with a literal `0` left this GREEN, and dividing one value to make
    /// it a float left it green too. So this gate holds exactly one thing — **the `<assign>`
    /// succeeds with a number in it, and the sentence reaches the prompt** — which is the failure
    /// that would otherwise be silent, because a failed assign leaves the previous value and no
    /// other gate here reads this string.
    ///
    /// ⚠⚠⚠ **THE FIXTURE MANUFACTURES ITS OWN AGREEMENT** and that is registered rather than
    /// papered over: it is the same shape as the restore-argv fixture whose two readings agreed
    /// because its pane ran a program that was not an agent. What would separate them is a session
    /// with a real transcript, or a walk past ONE restart — `restarts` becomes 1 while the other
    /// three stay 0, and a literal could no longer stand in for it. That the VALUES are the right
    /// ones is `spend.rs`'s claim and is gated there, on records with distinct numbers.
    #[test]
    fn the_reflection_carries_what_a_restart_would_cost_and_what_it_would_discard() {
        let (workspace, pane) = crate::testing::standin_agent_reflecting(
            9,
            "the next checkpoint",
            "what the next session must know",
        );
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(
            engine(),
            pane,
            &Brief {
                reflect_every: Some(2),
                ..brief_for(40)
            },
            &standin_spec(),
        )
        .expect("a briefed loop over a live pane starts");

        // ⚠ The prompt is composed in `reflecting`'s `onentry`, so a loop that has only been
        // briefed holds an EMPTY one — correct, and measured the hard way when this gate first read
        // it before stepping. Walk until the composition has happened.
        let run = RunContext::uncancellable();
        let mut reached = false;
        for _ in 0..60 {
            loops
                .step(&access, &run)
                .expect("every step of a reflection must be readable");
            if loops.state() == AiLoopState::Reflecting {
                reached = true;
                break;
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
        let authored = loops.authored().expect("the datamodel answers");
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }
        assert!(
            reached,
            "⚠⚠⚠ THE CONTROL: the run must actually reach `reflecting`, or the prompt read below \
             is the empty one every un-stepped loop holds and this gate asserts nothing",
        );

        assert!(
            authored.reflect.contains(
                "in the tokens the bill is actually denominated in: 0 read on its last request, \
                 of which 0 is a floor no restart escapes, leaving 0 that replacing this session \
                 would discard. Replacing it wrote 0 of cache to begin with and would write that \
                 again, and this run has already bought 0 replacements."
            ),
            "⚠⚠⚠⚠ THE SENTENCE CARRYING THE FOUR NUMBERS MUST REACH THE PROMPT. If it is missing \
             the `<assign>` failed and left the previous value — a number reaching a Lua string \
             concatenation is new in this document, and a generated expression that compiles and \
             quietly does nothing has cost this repository two wrong conclusions already. ⚠ This \
             does NOT hold that each number is READ rather than written literally: the fixture's \
             four are all `0`, and the mutation proving that is recorded in this gate's own doc. \
             Composed:\n{}",
            authored.reflect,
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
    ///
    /// ⚠⚠⚠⚠ **ITS `#[test]` USED TO SIT ABOVE THIS DOC, AND A SUPERVISING SESSION INSERTED A WHOLE
    /// TEST BETWEEN THE TWO** on 2026-08-17 — which put two `#[test]` on the inserted item, orphaned
    /// this one, and cost the loop sharing this tree FOUR refused commits
    /// (`duplicate_macro_attributes` under `-D warnings`) before anybody read the reason. The index
    /// already carried the rule: a test goes ABOVE the attribute, never between one and its item.
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
                reflect_every: Some(2),
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
                .any(|note| note.starts_with("Reviewing --Review")
                    && note.contains("--> Restarting")),
            "⚠⚠⚠ AND THE REVIEW MUST NOT BE ABLE TO STOP THE RUN. `reviewing` sits between the \
             reflection and the replacement, and EVERY ending it has CARRIES ON — to `restarting`, \
             or back to `working` where there is room and the economics say keep the session: a \
             review is advice about work already finished, so a reviewer that found nothing, could \
             open no record, or broke outright must cost this run one transition and no more. This \
             holds the property `reviewing` has no edge to `failed` for — and it is deliberately \
             loose about WHICH review ending, because which one it was is not this gate's claim. \
             ⚠ The line is matched by CONTAINS and no longer by `ends_with`, because a replacement \
             now names the decision that caused it (register item 445) — a suffix match here would \
             have gone red for a product that got better at saying why. \
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

    /// ⚠⚠⚠⚠⚠ **A REFUSAL AND A REFUSAL ABOUT NOTHING ARE THE SAME WORD, SO THE WALK HAS TO SAY
    /// WHICH READER THE CHECK WAS SHOWN** — register item 448.
    ///
    /// # ⚠⚠⚠⚠ What could not be answered from outside a running daemon, measured
    ///
    /// A live run was refused **eight times with an identical walk line**. The rounds sent to
    /// diagnose it eliminated a stale milestone and a thin account by hand, and the surviving
    /// hypothesis was that the check had been handed NOTHING — `turn_produced` falls back to the
    /// pane when the agent's own account is unavailable, item 441 measured that reader going
    /// permanently blind against a repainting agent, and a real checker shown an empty artifact was
    /// measured answering a clean `NO`.
    ///
    /// **Settling it needed one `eprintln!` inside the daemon, and that could not be run**: the
    /// daemon owns the run under investigation, so instrumenting it ends the thing being
    /// investigated. *A loop cannot instrument the driver that is driving it.* This line is that
    /// probe, published.
    ///
    /// # ⚠⚠⚠ The three arms
    ///
    /// * The refusal says which reader — the whole point.
    /// * ⚠⚠⚠⚠ **AND SO DOES THE AGREEMENT.** Saying it only on the refusal would tell the two
    ///   apart by the ABSENCE of a sentence, which is the reading this workspace has burned wire
    ///   numbers over — and an agreement reached off a blind pane is a milestone certified on
    ///   nothing, which is worse than a refusal.
    /// * ⚠⚠⚠ **AND A PASS THAT CHECKED NOTHING NAMES NO READER**, because an instrument named where
    ///   no judgement was made describes a reading that did not happen.
    #[test]
    fn a_walk_says_which_reader_the_check_was_shown_and_not_only_what_it_decided() {
        /// One judgement's line, for a check that answered `verdict` off `shown`.
        fn line(verdict: Option<crate::outer::Checked>, shown: Option<Evidence>) -> String {
            AiLoop::walked(
                AiLoopState::Judging,
                AiLoopEvent::Judge,
                AiLoopState::Working,
                Learned {
                    checked: verdict,
                    shown,
                    ..Learned::default()
                },
            )
        }

        let blind = line(
            Some(Checked::Failed),
            Some(Evidence::Pane(Unstated::Unsupervised)),
        );
        assert!(
            blind.contains(Evidence::Pane(Unstated::Unsupervised).named()),
            "⚠⚠⚠⚠⚠ THE REFUSAL MUST SAY WHAT IT WAS LOOKING AT. Eight identical refusals were \
             read by three rounds and none could say whether the check had been shown the agent's \
             work or a pane whose addresses had frozen — and those are opposite findings wearing \
             one word. Got {blind:?}",
        );
        let stated = line(Some(Checked::Failed), Some(Evidence::Statement));
        assert_ne!(
            blind, stated,
            "⚠⚠⚠⚠ AND THE TWO READERS MUST NOT RENDER THE SAME, or the line is decoration: a \
             refusal off the agent's own account is a verdict about the WORK, and one off a blind \
             pane is a verdict about nothing",
        );

        // ── AND ON THE AGREEMENT TOO ──
        let agreed = line(
            Some(Checked::Passed),
            Some(Evidence::Pane(Unstated::Unsupervised)),
        );
        assert!(
            agreed.contains(Evidence::Pane(Unstated::Unsupervised).named()),
            "⚠⚠⚠⚠ AN AGREEMENT REACHED OFF A BLIND PANE IS A MILESTONE CERTIFIED ON NOTHING, and \
             it is the more dangerous of the two — it ENDS runs. Publishing the reader only beside \
             a refusal would tell the two apart by the absence of a sentence: {agreed:?}",
        );

        // ── THE CONTROL: nothing was checked, so nothing was shown ──
        let unchecked = line(None, None);
        assert!(
            !unchecked.contains("was shown"),
            "⚠⚠⚠ THE CONTROL FAILED. A pass that made no judgement has no reading to describe, and \
             a line claiming one would send a reader looking for a check that never ran — which is \
             the class this whole item is inside: {unchecked:?}",
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
        // ⚠ A look discovers nothing and delivers nothing, so every fact is absent.
        let looked = AiLoop::walked(
            AiLoopState::Working,
            AiLoopEvent::Null,
            AiLoopState::Working,
            Learned::default(),
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
            Learned::default(),
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

    /// ⚠⚠⚠⚠⚠ **A WALK SAYS WHAT PROVED THE PROMPT ARRIVED, AND SAYS SOMETHING DIFFERENT FOR THE
    /// ROAD WHERE THE PANE CARRIES NOTHING** — register item 434.
    ///
    /// # ⚠⚠⚠⚠ What a walk that did not say it costs, measured
    ///
    /// `Delivered::Confirmed` and `Delivered::Reported` are both successes and they say OPPOSITE
    /// things about the pane — one has the prompt painted on it and the other has none of it — and
    /// `OuterLoop::say` answered `Ok(bytes)` for both. So a supervisor reading a run had no way to
    /// know whether looking at the pane would show them the prompt, which is item 423's disease in a
    /// fourth place: the result written down and the grounds dropped.
    ///
    /// **Measured on 2026-08-18**: a live run reached `reflecting`, delivered the driver's own
    /// reflection prompt and replaced its session — and which road that delivery took had to be
    /// reconstructed afterwards from arithmetic on the walk's byte totals against the agent's
    /// transcript (2 × 1,314 + 1 = the 2,629 the walk reported ⇒ two injections and one submit ⇒
    /// the first was swallowed and the screen carried the second). Nobody supervising a run does
    /// that, and it is only possible while the transcript still exists.
    ///
    /// # ⚠⚠ What is asserted: the DIFFERENCE, and that the ordinary road is not silent
    ///
    /// Asserting only that the account road says something would pass for a channel that said the
    /// same sentence for every delivery — and asserting only that the two differ would pass for one
    /// that said nothing at all on both. So: the account line names the fold, the painted line does
    /// not, and neither is empty. ⚠ The clauses are asserted rather than the whole sentence, so a
    /// reword does not fail this gate while a SILENCE does.
    #[test]
    fn a_walk_says_what_proved_the_prompt_arrived() {
        let walked = |evidence| {
            AiLoop::walked(
                AiLoopState::Priming,
                AiLoopEvent::PromptSent,
                AiLoopState::Working,
                Learned {
                    witnessed: Some(evidence),
                    ..Learned::default()
                },
            )
        };
        let bare = AiLoop::walked(
            AiLoopState::Priming,
            AiLoopEvent::PromptSent,
            AiLoopState::Working,
            Learned::default(),
        );

        let account = walked(crate::deliver::Witnessed::Account);
        assert!(
            account.contains("NOWHERE ON THAT SCREEN"),
            "⚠⚠⚠⚠⚠ ITEM 434: the prompt is not on that pane and a person sent to look for it must \
             be told so IN THE WALK, which is the only thing they have: {account:?}",
        );

        // ── AND THE ORDINARY ROAD IS NOT SILENT, which is what stops the line above being read
        //    off the ABSENCE of a sentence rather than off its presence ──
        let painted = walked(crate::deliver::Witnessed::Painted);
        assert!(
            painted.len() > bare.len() && painted.contains("painted"),
            "⚠⚠⚠ a delivery the pane DID paint says so too. Telling the two roads apart by which \
             one carries a clause is reading a fact off an absence — the reading this workspace has \
             burned wire numbers over: {painted:?} against the bare {bare:?}",
        );
        assert_ne!(
            painted, account,
            "⚠⚠⚠⚠ and the two sentences are DIFFERENT, or the channel exists and answers the same \
             thing whichever road was walked",
        );

        // ── AND A PASS THAT DELIVERED NOTHING SAYS NOTHING ──
        assert_eq!(
            bare, "Priming --PromptSent--> Working",
            "⚠⚠ a pass with no delivery must add no clause about one, or every look in a walk \
             grows a sentence about a prompt that was not sent: {bare:?}",
        );
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
        // ⚠⚠⚠ AND THE CLAIM'S VERDICT IS PART OF THE LINE — register item 428. This document authors
        // no `milestone_check`, so the edge carries `NotAsked`: the milestone rests on the working
        // agent's own word, and a walk that did not say so is what that item is about.
        let reflected = format!(
            "Judging --Judge--> Reflecting — {} — {}",
            crate::outer::ReflectReason::Milestone.noted(),
            crate::outer::Checked::NotAsked.describe(),
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
        // for THIS reason says so. `the_walk_and_the_ending_both_say_which_close_it_was` holds the other
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
    ///
    /// # ⛔⛔⛔⛔⛔ And since register item 706's third requirement, the ENDING carries the word too
    ///
    /// Everything above is about the walk — a SENTENCE, composed for a person. That left every
    /// consumer parsing prose for a word the loop already knew, and register item 594 measured
    /// what it costs: `sprag stand-down` promises *its work is kept*, and the row the person then
    /// read said `converged`, byte-identical to a run nobody had ordered anything of.
    ///
    /// So each arm now asserts twice — the walk still says which ending in its own sentence, and
    /// [`crate::driver::Outcome::done_reason`] says which ending in one word a reader takes as a
    /// key. ⚠⚠ The two assertions sit beside the `Converged` control on purpose: that control is
    /// the reason the second one is needed at all, because it is what proves `state` cannot tell
    /// these three runs apart.
    ///
    /// ⚠ And the echo control gained a clause of its own: a run that never reached `closing` must
    /// name NO ending, or a word hard-wired anywhere would satisfy all three arms.
    #[test]
    fn the_walk_and_the_ending_both_say_which_close_it_was() {
        use crate::outer::DoneReason;

        /// The one edge this gate is about — one arrow, two runs.
        const THE_EDGE: &str = "Reflecting --ReflectDone--> Closing";
        /// What the control peer proposes when it is asked, so that it does NOT end the run.
        const NEXT: &str = "the debt this run picked after the last one";
        /// And where it says the replacement should start reading.
        const READ_NEXT: &str = "the register entry for it";

        /// Drive a loop to its ending and hand back what it wrote down, with its whole outcome.
        ///
        /// ⚠⚠ The WHOLE outcome since register item 706's third requirement, not just its state
        /// word: the ending now carries the reason as a key of its own, and a fixture that kept
        /// only `state` could not tell whether it did.
        fn run_of<A: PaneAccess>(
            loops: &mut AiLoop,
            access: &A,
        ) -> (crate::driver::Outcome, Vec<String>) {
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
            (outcome, walk)
        }

        // ── ARM 1: THE AGENT SAID THE NORTH STAR WAS REACHED ──
        // ⚠ The first peer in this crate ever to say it. Two working turns, then the reflection it
        // is sent asks whether the whole thing is finished, and it says that it is.
        let (workspace, pane) = crate::testing::standin_agent_finishing(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (declared_end, declared_walk) = run_of(&mut loops, &access);

        // ── ARM 2: A REACHED MILESTONE WHOSE REFLECTION NAMED NO SUCCESSOR ──
        // ⚠ The ORDINARY peer — it says the milestone marker and has no opinion about what is next,
        // which is precisely the run the livelock guard ends.
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (no_successor_end, no_successor_walk) = run_of(&mut loops, &access);

        // ── THE CONTROL: THE SAME PROMPT, THE SAME ECHO, NO MARKER ──
        let (workspace, pane) = crate::testing::standin_agent_reflecting(2, NEXT, READ_NEXT);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let (echoed_end, echoed_walk) = run_of(&mut loops, &access);
        // ⛔⛔⛔⛔⛔ AND ITS ENDING NAMES NO REASON — register item 706's third requirement, and this
        // is the control that makes the three assertions below mean anything. A run that did NOT
        // close on its own terms must publish no word at all: without this clause a `done_reason`
        // hard-wired to any constant would satisfy all three arms and say nothing.
        //
        // ⚠⚠ `None` here is *this run named no ending* and never *it ended for no reason* — the
        // distinction the wire keeps by OMITTING the key rather than publishing a null.
        assert_eq!(
            echoed_end.done_reason, None,
            "⚠⚠⚠⚠ THE CONTROL FOR THE WORD: this peer answers with a successor, so its run never \
             reaches `closing` — and an ending that names a reason anyway is one reporting a \
             transition that never fired. Walked {echoed_walk:?}",
        );
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
        let (stood_down_end, stood_down_walk) = run_of(&mut loops, &access);

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
        for (label, ending, end, walk) in [
            (
                "the agent declared it",
                DoneReason::Declared,
                &declared_end,
                &declared_walk,
            ),
            (
                "no successor was named",
                DoneReason::NoSuccessor,
                &no_successor_end,
                &no_successor_walk,
            ),
            // ⚠⚠⚠ A STAND-DOWN CONVERGES TOO, and that is the claim, not an accident of the loop.
            // The run banked the milestone and took its account — so the word a reader gets is
            // `Converged`, exactly as for the other two, and the walk is what says a PERSON ended it
            // rather than the work running out. A stand-down that reported `Cancelled` would tell a
            // reader the turn was thrown away when it was finished.
            (
                "a person asked it to stand down",
                DoneReason::StoodDown,
                &stood_down_end,
                &stood_down_walk,
            ),
        ] {
            assert_eq!(
                end.state,
                OutcomeState::Converged,
                "⚠⚠⚠ the control for {label}: BOTH endings publish `Verdict::Converged` — that is \
                 exactly why the walk had to be the thing that tells them apart. An arm that ended \
                 any other way is not the run this gate is describing. Walked {walk:?}",
            );
            // ⛔⛔⛔⛔⛔ **AND THE ENDING ITSELF CARRIES THE WORD, NOT ONLY THE SENTENCE** — register
            // item 706's third requirement, asserted on the line that has just proved the three
            // runs are indistinguishable by `state`.
            //
            // ⚠⚠⚠⚠ THE PREVIOUS LINE IS THIS ONE'S WHOLE ARGUMENT. All three converge, so a
            // consumer asking *did the stand-down I gave land?* had `converged` and a walk note to
            // parse — register item 594's collapse, and the reason every watcher rebuilt a parser
            // over prose somebody else composed. The walk clauses below still hold the SENTENCE to
            // its wording; this holds the KEY, and a reader needs no parse to reach it.
            //
            // ⚠⚠ It is `Plugin::ended_because` that is being measured here and not a fixture's
            // hand: these outcomes came out of `Driver::run` over a real `AiLoop` whose document
            // took the transition a moment ago, so a wiring deleted anywhere between the datamodel
            // and this field turns this red.
            assert_eq!(
                end.done_reason.as_deref(),
                Some(ending.word()),
                "⚠⚠⚠⚠⚠ ITEM 706 ③: this run closed because {label}, and its ENDING must say so in \
                 one word — every one of these three publishes `converged`, so a reader with only \
                 `state` cannot tell a person's order landing from a run running out of things to \
                 propose. Walked {walk:?}",
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
            // ⚠⚠⚠ THE STAND-DOWN ARM CARRIES A CLAIM'S VERDICT TOO — register item 428. That
            // ending is reached straight out of `judging` by an agent that SAID the marker, so the
            // edge says what checked it (nothing: no document under test authors a
            // `milestone_check`). The other ending comes through `reflecting`, where no claim was
            // judged, so nothing is appended — and that difference is itself part of the assertion.
            let verdict = if the_edge == "Judging --Judge--> Closing" {
                format!(" — {}", crate::outer::Checked::NotAsked.describe())
            } else {
                String::new()
            };
            assert_eq!(
                line,
                &format!("{the_edge} — {}{verdict}", ending.noted()),
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

    /// ⛔⛔⛔ **THE PROMISE A PERSON IS TOLD ABOUT A STAND-DOWN NAMES THE WORD `sprag runs` REALLY
    /// PRINTS, AND AN ORDER GIVEN TO A MOVING LOOP IS STILL HONOURED** — register item 594, and
    /// register item 522's shape found one order over.
    ///
    /// # What shipped, and why nothing could go red on it
    ///
    /// `sprag stand-down`'s own doc said the run *"converges reporting `stood_down`"*. **There are
    /// two vocabularies in that clause and they do not overlap.** `stood_down` is
    /// [`DoneReason::StoodDown`]'s word, and that type's doc records what becomes of it: it is
    /// *"rendered through `noted` into a walk and nowhere else"*, so **nothing publishes it as a
    /// wire key or value**. What `sprag runs` prints is an [`OutcomeState`], whose vocabulary is six
    /// words with no `stood_down` among them. A person sent to watch for it was watching a surface
    /// that could never say it — and the repayment skill had by then copied the word into a table
    /// defining it as a run state, which is how a run reported `cancelled after 56 iterations` read
    /// as *the opposite of the promise* rather than as *a promise nobody could check*.
    ///
    /// ⚠⚠⚠⚠⚠ **THE ENDING IS MEASURED, NOT SPELLED.** The run below is stood down before its first
    /// pump and driven to its close, and the sentence is held against the word **that run actually
    /// ended with**. A gate comparing the sentence to an `OutcomeState::Converged` written into this
    /// file would agree with whatever this file believed, which is the failure it exists to catch.
    ///
    /// ⚠⚠ Its neighbour owns the other half — that a stood-down run converges AT ALL, and that the
    /// walk says a person ended it. That one owns only the crossing between what the machine does
    /// and what a person was told it would do.
    ///
    /// ⛔⛔⛔⛔ **AND THIS ONE OWNS THE HALF NEITHER OF THEM ASKED: AN ORDER GIVEN TO A LOOP THAT IS
    /// ALREADY MOVING** — register item 598's round, 2026-08-22.
    ///
    /// Every stand-down gate in this file raises the order BEFORE the first pump, so the machine is
    /// in `idle` when it arrives. **A person cannot do that.** They read `sprag runs`, see a loop
    /// working, and speak — so the order lands in whatever state the run happens to be in, which is
    /// never `idle`. This drives that, and it is not a hypothetical hazard: `OuterLoop::hold`'s own
    /// comment records the same thing being MEASURED for the order beside this one — *"the order
    /// landed while the loop was still in `idle`, where no such edge exists, and the run drove on
    /// with the person's word already spent."*
    #[test]
    fn an_order_that_arrives_after_the_loop_has_started_is_still_honoured() {
        // ⚠⚠⚠⚠⚠ EVERY MOMENT, NOT ONE. A person's order lands wherever the run happens to be, and
        // which state that is is not something they choose or can even see. So the experiment is a
        // SWEEP over the loop's own early life — pump `delay` passes, then speak — and the states
        // it covers are whichever ones the product actually passes through, which is a list the
        // document decides rather than one this gate keeps.
        //
        // ⚠⚠ ONE FRESH LOOP PER DELAY, because a stand-down is one-way by construction: the orders
        // region has no edge back, so a single loop could be asked this question exactly once.
        let mut seen: Vec<AiLoopState> = Vec::new();
        for delay in 0..6u32 {
            let (workspace, pane) = standin_agent(40);
            let access = supervised(&workspace);
            let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
                .expect("a well-briefed loop over a live pane starts");
            let run = RunContext::uncancellable();

            // ⚠ THROUGH `Plugin::step`, the Driver's own one-pass entry point rather than a door
            // opened for this gate: what the order meets is a loop stepped the ordinary way.
            for _ in 0..delay {
                loops.step(&access, &run).expect("a live pane takes a pass");
            }
            let arrived_at = loops.state();
            loops.stand_down();

            let progress = ProgressCell::default();
            let outcome = Driver::new(Guardrails {
                max_iterations: 120,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .reporting_to(Arc::clone(&progress))
            .run(&mut loops, &access, &run);
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
                "⛔⛔⛔⛔ AN ORDER GIVEN AFTER {delay} PASSES — the loop was in {arrived_at:?} — WAS \
                 NOT HONOURED. `sprag stand-down` promises the run stops at its next milestone with \
                 its work kept, and this run reached {:?} instead. A person can only ever give this \
                 order to a MOVING run; an order the document hears only from some states is an \
                 order whose landing is decided by timing nobody controls. Walked {walk:?}",
                outcome.state,
            );
            assert!(
                walk.iter()
                    .any(|note| note.starts_with("Judging --Judge--> Closing")),
                "⚠⚠⚠ AND BY THE STAND-DOWN'S OWN EDGE, after {delay} passes from {arrived_at:?}. A \
                 convergence reached some other way would satisfy the assertion above while saying \
                 nothing about the order: `standin_agent(40)` needs forty prompts to reach its \
                 milestone on its own, so a run that got there without this edge did not stand \
                 down. Walked {walk:?}",
            );
            seen.push(arrived_at);
        }

        // ⚠⚠⚠⚠ THE CONTROL ON THE SWEEP ITSELF: it has to have covered more than one moment. Six
        // delays that all landed in the same state would be six copies of the gate this replaced,
        // passing while saying nothing about the states in between.
        seen.sort_unstable_by_key(|state| format!("{state:?}"));
        seen.dedup();
        assert!(
            seen.len() > 1,
            "⚠⚠⚠ every delay put the order in the same state ({seen:?}), so this swept nothing — \
             the loop is not moving between passes and the claim above is one moment restated",
        );
        // ⚠⚠⚠⚠⚠ AND `judging` HAS TO BE ONE OF THEM. That state is where a standing order is
        // ACTED ON, so an order arriving there is the one case where the region's activity and the
        // guard that reads it are exercised in the same breath. A sweep that stopped short of it
        // would leave the sharpest moment untested while looking thorough — and register item 604's
        // whole investigation turns on whether `In('standing_down')` is true at exactly this state.
        assert!(
            seen.contains(&AiLoopState::Judging),
            "⚠⚠⚠ the sweep never reached `judging` ({seen:?}); widen the delays until it does, or \
             the moment this gate most needs to cover is the one it skips",
        );
    }

    /// ⛔⛔⛔⛔ **A TURN THAT WAS BANKED IS NOT LOST BECAUSE THE AGENT THEN LEFT** — register item
    /// 604, measured at the host door on 2026-08-22 and driven here where it is deterministic.
    ///
    /// # What a person is told, and what happened
    ///
    /// `sprag stand-down` promises *"it stops at its next milestone, and its work is kept"*. This
    /// run's turn COMPLETES — the loop reaches `judging`, which is only reachable by a completed
    /// turn, so the work really is banked. Then the agent exits, **which is what an agent that has
    /// finished its work does**. The document took `Judging --PeerGone--> PeerGone`, the run ended
    /// `failed`, and `sprag-host`'s `stand_down_sentence` therefore told the person *"it was cut
    /// short, so the turn it had going was NOT banked"*.
    ///
    /// ⚠⚠⚠⚠⚠ **THE RELIEVED ANSWER AND THE ALARMING ONE WERE SWAPPED**, which is the one direction
    /// a report must never be wrong in — and register item 594 exists because this same pair was
    /// unreadable once already.
    ///
    /// # ⛔⛔⛔⛔ THIS GATE PINS THE DEFECT. IF IT GOES RED, THE DEFECT IS FIXED — delete it, delete
    /// the comment block it names in `ai_loop.scxml`, and pay item 604.
    ///
    /// The edge that fixes this belongs at `judging`'s `peer.gone`, guarded on the order. It was
    /// written and **five guard expressions all read false** with the order provably standing —
    /// [`AiLoop::standing_down`] says the machine holds it immediately before the run — ending with
    /// a trivially true arithmetic guard that names no state and reads no event data. It is not
    /// shadowing either: deleting the unguarded sibling does not help, and the run still reaches
    /// `peer_gone` from an ancestor. Nor is it the raise path. And the same guard shape works one
    /// edge above, on `judge`, at this very state.
    ///
    /// ⚠ An unverified product change is worse than a measured defect, so what is committed is the
    /// measurement.
    ///
    /// # Why it is driven from here rather than from the host
    ///
    /// The host gate that measured it first depends on when a wire call lands relative to a worker
    /// thread. Here the peer's departure and the loop's state are both staged by this test, so
    /// there is no timing left in the experiment at all — which is what turned a round of guessing
    /// into a 30 ms reproduction.
    #[test]
    fn a_banked_turn_survives_an_agent_that_leaves_under_a_standing_order() {
        let (workspace, pane) = standin_agent_that_leaves();
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let run = RunContext::uncancellable();

        // ⚠⚠⚠ PUMPED TO `judging` AND NOWHERE ELSE, because that state IS the claim: reaching it
        // means `working` raised `turn.done`, so the turn is over and its account is in hand. A
        // peer that leaves while the loop is still in `working` really has cost that turn, and this
        // gate must not be able to pass by accident on that case.
        let mut pumped = 0;
        while loops.state() != AiLoopState::Judging && pumped < 40 {
            loops.step(&access, &run).expect("a live pane takes a pass");
            pumped += 1;
        }
        assert_eq!(
            loops.state(),
            AiLoopState::Judging,
            "⚠⚠⚠ THE FIXTURE'S OWN PRECONDITION: this loop must reach `judging` within {pumped} \
             passes, or nothing below is about a banked turn",
        );

        // ⚠⚠ THE AGENT HAS ALREADY LEFT BY NOW — `standin_agent_that_leaves` answers once and
        // exits, so reaching `judging` above and the peer's departure are the same moment, which is
        // what a real agent finishing its work looks like. Nothing here kills anything: a pane
        // CLOSED and a program EXITED are different facts, and only the second is this claim.
        loops.stand_down();
        // ⚠⚠⚠⚠⚠ THE PROBE, INSIDE. Everything below is about what the document DECIDES with the
        // order, and that question is only worth asking once the order is known to have been HEARD.
        // Without this line a red below has two causes and the reader cannot tell them apart —
        // which cost this investigation a whole round.
        assert!(
            loops.standing_down(),
            "⚠⚠⚠ the document did not hear the order at all, so nothing below is about what it \
             decides with one. The loop was in {:?}",
            loops.state(),
        );

        let progress = ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 120,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .reporting_to(Arc::clone(&progress))
        .run(&mut loops, &access, &run);
        let walk: Vec<String> = progress
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        // ⚠⚠⚠⚠⚠ **WAS `peer.gone` EVEN LOOKED AT?** — the reading upstream asked for, and the one
        // that eliminates a whole candidate. SCE's reply of 2026-08-23 drove this very document at
        // this crate's own pin and had the guarded edge FIRE, leaving two explanations for what is
        // measured here: the machine was not in `judging` when the event was dequeued, or it had
        // stopped and never dequeued it at all. [`AiLoop::unseen`] answers the second, and the
        // answer is that the event WAS dequeued — so the guard really was in the picture, and the
        // next round's suspect is the ACTIVE STATE at the moment the driver raises.
        assert_eq!(
            loops.unseen(),
            None,
            "⚠⚠⚠⚠⚠ THE MACHINE REFUSED SOMETHING. Then item 605 is not about a guard at all: the \
             run had already ended and the event above was never dequeued, which is the reading \
             every earlier attempt was missing. Walked {walk:?}",
        );
        // ── THE CONTROL, and this gate is worth nothing without it ──
        //
        // ⚠⚠⚠⚠⚠ A reader that answers `None` because it is BLIND answers `None` here too. The run
        // is over and `peer_gone` is final, so W3C SCXML Appendix D's main event loop has exited:
        // an order arriving now is one the machine cannot look at, and the reading MUST turn into
        // it. That is what makes the `None` above a fact about this run rather than the plumbing.
        //
        // ⚠ It costs nothing that the order is spent: `outcome` and `walk` are already read, and a
        // stand-down against a machine that has stopped is precisely the thing being staged.
        loops.stand_down();
        assert_eq!(
            loops.unseen(),
            Some(crate::sm::ai_loop::AiLoopEvent::StandDown),
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, so the `None` above says nothing. Either this reader cannot \
             see a refused event at all, or this run did not actually end — and both make the \
             measurement above unreadable. Walked {walk:?}",
        );
        // ⚠⚠⚠⚠⚠ THE SAME TWO READERS, AFTER. Both said the order was standing a moment ago and the
        // guard that had to agree did not, so the remaining question is whether the fact SURVIVED
        // the run — a session re-initialised mid-run would put the datamodel back to its authored
        // `false` and would explain every guard form failing alike.
        for live in access.pane_ids() {
            access.lifecycle().expect("lifecycle").close(live);
        }

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "⛔ IF THIS IS RED, GOOD — a banked turn whose agent then left is no longer reported as \
             a failure. Delete this gate, delete the comment block it is named in inside \
             `ai_loop.scxml`, and pay register item 604. Walked {walk:?}",
        );
        // ⚠⚠⚠⚠⚠ **AND THE WALK NAMES `working`, WHICH IS THE WHOLE MECHANISM.** Measured
        // 2026-08-23, the moment `pump` stopped reading `from` at the top of the pass: this line
        // used to read `Judging --PeerGone--> PeerGone`, and that sentence is what register item
        // 605 spent four rounds and five guard rewrites believing.
        //
        // What actually happens: the order is standing and the turn IS banked, and the driver
        // still judges that turn as an ordinary one and asks for another — `judging --judge-->
        // working` — and only then does sending that prompt discover the agent has gone. So
        // `peer.gone` is answered from `working`, where a turn in flight really is lost, and the
        // guarded edge at `judging` is never reached because the machine is no longer there.
        //
        // ⚠⚠⚠ THE PASS RAISED TWO EVENTS AND THE WALK CARRIES ONE. `judging --judge--> working`
        // is nowhere in this journal, which is its own defect and is registered as one.
        assert!(
            walk.iter()
                .any(|note| note == "Working --PeerGone--> PeerGone"),
            "⛔ IF THIS IS RED, GOOD — the driver has stopped handing another turn to a run that \
             was told to stand down at the milestone it had just reached. Read the walk before \
             deleting anything: a `Judging --PeerGone--> ...` here would mean `from` went back to \
             being read at the top of the pass. Walked {walk:?}",
        );
    }

    /// ⛔⛔⛔ **THE SENTENCE A PERSON IS TOLD NAMES THE WORD THEIR RUN REALLY ENDS WITH** —
    /// register item 594, and the neighbour above owns the rest of that item's argument.
    ///
    /// ⚠⚠⚠⚠⚠ **THE ENDING IS MEASURED, NOT SPELLED.** The run below is stood down before its first
    /// pump and driven to its close, and the sentence is held against the word **that run actually
    /// ended with**. A gate comparing the sentence to an `OutcomeState::Converged` written into
    /// this file would agree with whatever this file believed, which is the failure it exists to
    /// catch.
    #[test]
    fn the_promise_about_a_stand_down_names_the_word_a_stood_down_run_reports() {
        use crate::outer::{DoneReason, STAND_DOWN_TAKES_EFFECT};

        // ── THE ENDING, MEASURED ── arm 3's fixture: the order stands from before the first pump,
        // so it is still standing when `judging` finally asks.
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        loops.stand_down();
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
        // ⚠⚠⚠ THE CONTROL ON THE FIXTURE: this arm is worth nothing unless the run really took the
        // stand-down's own edge. Its neighbour asserts the whole line; here the arrow is enough,
        // and without it a run that expired or failed would hand this gate a word to agree with.
        assert!(
            walk.iter()
                .any(|note| note.starts_with("Judging --Judge--> Closing")),
            "⚠⚠⚠ the control: this run must have closed straight out of `judging`, which is what a \
             STANDING order does. Any other arrow means the word measured below belongs to some \
             other ending. Walked {walk:?}",
        );
        let reported = outcome.state.wire_str();

        // 1. ── THE SENTENCE NAMES THAT WORD ──
        assert!(
            STAND_DOWN_TAKES_EFFECT.contains(reported),
            "⛔⛔⛔ ITEM 594: a run a person stood down really ends {reported:?}, and the sentence \
             they were told does not contain that word — so they are watching `sprag runs` for \
             something it will not print. Sentence {STAND_DOWN_TAKES_EFFECT:?}. Walked {walk:?}",
        );

        // 2. ── AND NAMES NO OTHER OUTCOME WORD ── a sentence hedging across the vocabulary would
        // satisfy the assertion above while telling a person to accept any ending at all.
        let named: Vec<&str> = OutcomeState::WIRE_WORDS
            .iter()
            .copied()
            .filter(|word| STAND_DOWN_TAKES_EFFECT.contains(word))
            .collect();
        assert_eq!(
            named,
            vec![reported],
            "⚠⚠⚠ the promise must name ONE ending — the one a stood-down run reaches. Naming \
             several is a sentence that cannot be wrong, which is a sentence that says nothing. \
             Sentence {STAND_DOWN_TAKES_EFFECT:?}",
        );

        // 3. ── AND IT MAY NOT SPEND THE DOCUMENT'S VOCABULARY ON A PERSON ── walked rather than
        // spelled, so a FOURTH `DoneReason` invented later is covered by this gate on the day it
        // exists rather than on the day somebody remembers to come back here.
        for ending in DoneReason::ALL {
            if OutcomeState::WIRE_WORDS.contains(&ending.word()) {
                continue;
            }
            assert!(
                !STAND_DOWN_TAKES_EFFECT.contains(ending.word()),
                "⛔⛔⛔ ITEM 594, EXACTLY: the promise spends {:?}, which is a `DoneReason` — it \
                 reaches a walk and NOTHING else, so no reader of `sprag runs` can ever see it. \
                 That is the word the shipped doc told people to wait for. Sentence \
                 {STAND_DOWN_TAKES_EFFECT:?}",
                ending.word(),
            );
        }

        // 4. ── AND IT NAMES THE COMMAND THAT ANSWERS ── R455's rule: a sentence that delegates
        // must delegate somewhere that replies. `sprag-host`'s `a_stood_down_run_publishes_the_
        // order_and_says_when_the_ending_did_not_honour_it` is what holds that end of it; before
        // that gate this clause was a promise about a surface publishing nothing at all.
        assert!(
            STAND_DOWN_TAKES_EFFECT.contains("sprag runs"),
            "⚠⚠⚠ the promise must say WHERE the answer appears. A person who is told the work is \
             kept and not told what would show them has to go and read a pane. Sentence \
             {STAND_DOWN_TAKES_EFFECT:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE WIDTH THE GATE ABOVE LEFT AS A RESIDUE IS MEASURED HERE** — register item 270,
    /// the half of it that only a whole run can say.
    ///
    /// # ⚠⚠⚠ What the control above proves, and what it does not
    ///
    /// `the_walk_and_the_ending_both_say_which_close_it_was` carries a peer asked the same reflection,
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
                .and_then(|supervisor| supervisor.pane_agent_state(pane).seen());
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
            .and_then(|supervisor| supervisor.pane_agent_state(pane).seen());
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
        // ⚠⚠⚠⚠ **THE LOOP MUST HAVE REACHED FOR THE JOB — AND WHAT IT GETS BACK IS THE PLATFORM'S**
        // (register item 151, a macOS red diagnosed and never reproduced off macOS until here).
        //
        // Stopping a pane's own program under the narrow reach is decided by asking the kernel
        // whether the signal would KILL it, and that question has an answer only where a
        // disposition can be read: `/proc/<pid>/status` on Linux, and on macOS a `kinfo_proc` that
        // `libc` declares no type for. So macOS answers *cannot tell* and `stop.rs` refuses rather
        // than guessing — deliberately, and its own gates state that divergence with this same
        // `cfg!`. This one did not: it demanded `Stopped::Job(_)` on every platform, so a macOS
        // runner reported `Unreached(NotStopped(CannotTellIfItWouldEnd))` as a failure of the LOOP
        // when it is an absent capability of the HOST.
        //
        // ⚠⚠ What is platform-independent, and what this therefore asserts on both: the run
        // REACHED for the work it set going — `stopped` is `Some(_)`, never `None`, which is what
        // *"the loop's door closed on a room its agent is still working in"* would look like.
        assert!(
            outcome.stopped.is_some(),
            "⚠⚠⚠ the loop ended without reaching for the job at all, which is its door closing on \
             a room its agent is still working in: {:?}",
            outcome.stopped,
        );
        // ⚠⚠⚠⚠⚠ **AND WHAT THE LOOP SAYS A STOP MUST REACH, WHICHEVER WAY THE RACE FELL** —
        // register item 470, stage 3. `AiLoop::driving` asks the ending (`Signals`), and a
        // cancelled run can arrive at that question by TWO roads: the Driver sees the flag at its
        // loop top and ends the run from OUTSIDE with the document still mid-work (`None`), or the
        // pass in flight pumps the machine into `cancelled` first (`Some(Pane)`). **Both say there
        // is a pane to reach, which is the claim.**
        //
        // ⚠⚠ **THIS WAS AN `assert_eq!(…, None)` AND IT WAS A FLAKY GATE OF MY OWN MAKING.** It was
        // written from ONE observation of the first road and read as an invariant; a later run took
        // the second and it went red saying *the fixture moved* about a fixture that had not. An
        // equality over a value the fixture does not control is a gate that fails for being right.
        //
        // ⚠ What must never be true is the third reading. `Signals::Nothing` here would be the loop
        // reporting nothing to stop while its agent is mid-turn — a live model left running after a
        // cancel, which is exactly what folding `None` in with `Nothing` would produce and what the
        // Linux assertion below then catches as `Stopped::Nothing`.
        assert_ne!(
            loops.inner.signalling(),
            Some(crate::act::Signals::Nothing),
            "⚠⚠⚠⚠⚠ THE LOOP SAYS A CANCEL HAS NOTHING TO REACH WHILE ITS AGENT IS MID-TURN. Every \
             road into this question — no ending published yet, or the document's own `cancelled` \
             — answers that there IS a pane, and this is the one answer that would leave somebody's \
             model spending tokens on a question nothing is waiting for",
        );
        if cfg!(target_os = "linux") {
            assert!(
                matches!(outcome.stopped, Some(Stopped::Job(_))),
                "⚠⚠⚠ on a host that CAN read a disposition the pane's job must have been \
                 SIGNALLED: {:?}",
                outcome.stopped,
            );
        }
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
                if replaced == sessions || loops.inner.finished() {
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
                reflect_every: Some(2),
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

        // ── THE CONTROL ON THE VOCABULARY: every reason a BRIEF can produce ──
        //
        // ⚠⚠⚠⚠ ONE REASON IS EXCEPTED BY NAME AND ONLY ONE, so a FIFTH word still breaks this gate:
        // `ReflectReason::ALL` is the glob, and the exception is a list of one. `Capacity` is
        // authored in the DOCUMENT (`context_ceiling`) rather than through a `Brief`, and this gate
        // reaches its arms only through briefs — so the run that renders that word is
        // `a_session_past_its_ceiling_reflects_without_being_asked` in `outer.rs`, which drives it
        // through a real pump and asserts this same rendering. Register item 424(b).
        //
        // ⚠⚠ It is an EXCEPTION rather than a loosening: without it this assertion would have to
        // become *"a subset of ALL"*, which is satisfied by a gate that arranges nothing.
        // ⚠⚠⚠ A SECOND EXCEPTION, AND IT CARRIES THE SAME DEBT THE FIRST ONE DOES: `Refused` needs
        // the document's `reflect_after_refusals` authored small enough to reach inside a bounded
        // walk, which no `Brief` can say — so the run that renders that word is
        // `a_claim_refused_to_the_ceiling_reflects_rather_than_buying_another_turn` in `outer.rs`,
        // which drives a real pump against a real checker and asserts this same rendering, with the
        // ceiling-out-of-reach control beside it. Register item 449.
        // ⚠⚠⚠ A THIRD EXCEPTION, ON THE SECOND ONE'S TERMS EXACTLY — register item 741. `Unverified`
        // is `Refused`'s neighbour: it needs a streak against a ceiling, and it needs a CHECKER that
        // answers nothing, which is a stand-in this gate's peers do not include. The run that
        // renders it is
        // `a_check_that_said_nothing_readable_leaves_by_its_own_door_with_its_own_answer`, which
        // drives the document to that edge with the ceiling at one and reads the word back off the
        // datamodel; the driver's half — that the word is one it can read back at all — is
        // `every_edge_into_reflecting_says_why_in_a_word_this_driver_knows`.
        const AUTHORED_IN_THE_DOCUMENT: [ReflectReason; 3] = [
            ReflectReason::Capacity,
            ReflectReason::Refused,
            ReflectReason::Unverified,
        ];
        let covered: std::collections::BTreeSet<ReflectReason> =
            arms.iter().map(|(_, reason, _)| *reason).collect();
        assert_eq!(
            covered,
            ReflectReason::ALL
                .into_iter()
                .filter(|reason| !AUTHORED_IN_THE_DOCUMENT.contains(reason))
                .collect(),
            "⚠⚠⚠ the control: this gate must arrange EVERY reason a reflection can have that a \
             BRIEF can produce. An arm no run here reaches is a word nothing renders and a sentence \
             nobody has read — and the document half of that is \
             `every_edge_into_reflecting_says_why_in_a_word_this_driver_knows`",
        );

        // ── AND THE WALK SAYS WHICH ──
        for (label, reason, walk) in arms {
            let line = the_line(label, walk);
            // ⚠⚠⚠ THE MILESTONE ARM CARRIES A SECOND TRUE FACT — register item 428: its agent
            // CLAIMED something, so the edge says what checked the claim (here: nothing, because no
            // document under test authors a `milestone_check`). The other two arms carry no claim —
            // a standing instruction and a spent budget are the loop's own housekeeping — so nothing
            // is appended to them, and that difference is itself the assertion.
            let verdict = if reason == ReflectReason::Milestone {
                format!(" — {}", crate::outer::Checked::NotAsked.describe())
            } else {
                String::new()
            };
            assert_eq!(
                line,
                format!("{THE_EDGE} — {}{verdict}", reason.noted()),
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
                reflect_every: Some(2),
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
    ///
    /// # ⚠⚠⚠⚠⚠ THROUGH THE PRODUCT'S DOOR, AND FIVE GATES MEASURED WHY ON 2026-08-25
    ///
    /// This used to be `Engine::new` + `initialize` by hand, and it was invisible for as long as
    /// the document asked nothing of its host. The moment `closing` and `stopping` declared their
    /// first act (register item 470, stage 2), every gate here that drove a run to an ending read
    /// `failed` — because SCE refuses a declared type with no handler registered, which is the
    /// engine deciding correctly about an engine no product ever builds.
    ///
    /// ⚠⚠ **THAT IS THIS CRATE'S OWN RECORDED FAILURE SHAPE — a hand fixture measuring a shape the
    /// product cannot produce.** [`crate::document::opened`] is the one road, and the reason it is
    /// a road rather than a checklist is exactly this: a step a fixture has to remember is one the
    /// next fixture will not. The neighbouring `reflected` helper already says the rule in its own
    /// words — *a fixture must reach a state by the door the product uses*.
    ///
    /// # ⚠⚠⚠⚠⚠ AND THE SAME SHAPE CAME BACK ONE ACT LATER, WHICH IS WHY THE HOST IS HANDED BACK
    ///
    /// Registering a handler was only half of being a host. When `priming` declared its act too
    /// (register item 470, stage 2, 2026-08-25), **seven gates here went red at once for the second
    /// time** — every walk that reached a run's ending. The handler RECORDS an act and
    /// [`crate::act::Serving::taken`] is what carries it out, so a walk that only stepped left the
    /// first act sitting in the slot for the whole run, and the run's SECOND act was refused for
    /// overrunning it (`Refused::Overrun`) — correctly, and into the document's own `fail`.
    ///
    /// ⚠⚠⚠ **A HOST THAT NEVER PERFORMS IS NOT A HOST, and the remedy is the same one as last
    /// time: not a rule, a door.** [`carried`] is now the only way anything here advances this
    /// machine, and it performs the act on the far side of the step exactly as
    /// [`crate::outer::OuterLoop::advance`] does on the far side of a pass. That is why this
    /// returns the handle: the walk IS the driver's other half, and it cannot be that with no way
    /// to reach the slot.
    fn started() -> (
        Engine<AiLoopPolicy>,
        crate::act::Serving,
        Arc<dyn IScriptEngine>,
        String,
    ) {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let host = crate::act::Serving::new();
        let engine = crate::document::opened(AiLoopPolicy::new(Arc::clone(&lua)), &host)
            .expect("this document answers its own errors, so the door admits it");
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel must have opened a script session");
        (engine, host, lua, session)
    }

    /// Raise `event` at `engine` carrying `standing` as `_event.data.standing`, then step.
    ///
    /// ⚠⚠ THE TWO REFLECTION EDGES BOTH ADOPT THAT LIST, so a gate driving the machine by hand has to
    /// send it: `process_event` carries no data at all, and a document-level gate that used it would
    /// assign nil over the variable `priming` composes and then assert about the states anyway. **A
    /// fixture must reach a state by the door the product uses** — the driver's `Raise::carrying` is
    /// this, one layer up.
    fn reflected(
        engine: &mut Engine<AiLoopPolicy>,
        host: &crate::act::Serving,
        event: AiLoopEvent,
        standing: &str,
    ) {
        carried(
            engine,
            host,
            event,
            &serde_json::json!({"standing": standing}).to_string(),
        );
    }

    /// **WHAT THE DRIVER PUTS ON `turn.done`** — three numbers, and a zero is a record that could
    /// not be read rather than a small one; then whether the turn produced anything, which is the
    /// one key here that is a DIFFERENCE rather than a reading. `judging`'s `onentry` assigns all
    /// four.
    ///
    /// ⚠ `false` for the fourth, which is what a run whose record it cannot read publishes — the
    /// honest value for a fixture that puts no record anywhere, and never `0`: this datamodel is
    /// Lua, where `0` is TRUE (register item 719, and `JUDGED` two doc comments down for the same
    /// rule).
    const TURN: &str = r#"{"context": 0, "cold": 0, "floor": 0, "produced": false}"#;

    /// **WHAT THE DRIVER PUTS ON `prompt.unasked`** — whether these exact bytes have already cost
    /// this run a session (register item 719), and `false` for the ordinary first refusal.
    ///
    /// ⚠⚠ EVERY RAISE OF THIS EVENT CARRIES IT, which is why the constant exists rather than an
    /// empty string at nine call sites. The document's first `prompt.unasked` edge reads
    /// `_event.data.retyped`, and W3C SCXML has nothing to index when the payload is empty: the
    /// entry raises `error.execution`, `work`'s own error edge answers it, and the run ends
    /// `failed` — measured here the moment the edge was added, with `priming` reporting `Failed`
    /// where `Restarting` was expected. [`TURN`] one doc up learned the same lesson under register
    /// item 505.
    ///
    /// ⚠ `false` and never `""`: this datamodel is Lua, where the empty string is TRUE, so an
    /// empty spelling would make every refusal look like a repeat.
    const UNASKED: &str = r#"{"retyped":false}"#;

    /// **THE SAME EVENT SAYING THE TEXT HAS BEEN HERE BEFORE** — [`UNASKED`] with the word the
    /// driver publishes when the bytes it just delivered are the bytes a replacement was already
    /// spent on. See `Retyped`.
    const UNASKED_AGAIN: &str = r#"{"retyped":"again"}"#;

    /// **WHAT THE DRIVER PUTS ON `judge` AFTER AN ORDINARY TURN** — six keys, every one of them
    /// `false`, which is `OuterLoop::pump`'s own shape for *the agent worked and declared nothing*.
    ///
    /// ⚠ `false` and never `0` or `""`: this datamodel is Lua, where both of those are TRUE.
    ///
    /// ⛔ THE SIXTH ARRIVED WITH REGISTER ITEM 741, and a gate is what said so:
    /// `a_payload_a_fixture_shares_under_a_name_is_the_drivers_own` compares these against the keys
    /// the DOCUMENT reads, so a fixture short of one walks a state the product never walks — which
    /// is the exact hazard this round met from the other side, when a payload missing
    /// `reflect_after_refusals` assigned nil and ended a run `failed`.
    const ORDINARY: &str = r#"{"done": false, "checked": false, "explained": false, "unheard": false, "silence": false, "stop_short": false}"#;

    /// **THE SAME PAYLOAD WITH THE AGENT SAYING THE WORD** — [`ORDINARY`]'s six keys with `done`
    /// true, which is the only difference between a turn that banks and one that closes the run.
    ///
    /// ⚠ Spelled out beside its sibling rather than built from it: what a fixture sends the machine
    /// is the thing under test, and a payload assembled by string surgery is one a reader cannot
    /// check against `OuterLoop::pump`'s own by eye.
    const DONE: &str = r#"{"done": true, "checked": false, "explained": false, "unheard": false, "silence": false, "stop_short": false}"#;

    /// **THE SAME PAYLOAD SAYING ONE OF THE RUN'S OWN CEILINGS FELL DUE** — `judging`'s FIRST arm,
    /// which asks the agent for an account rather than for more work.
    const STOP_SHORT: &str = r#"{"done": false, "checked": false, "explained": false, "unheard": false, "silence": false, "stop_short": true}"#;

    /// Raise a DATA-CARRYING event the way the driver does, then step.
    ///
    /// # ⚠⚠⚠⚠⚠ Why fifteen fixtures changed to this in register item 505's round
    ///
    /// [`reflected`] above had already written the rule — *a fixture must reach a state by the door
    /// the product uses* — and fifteen sites in this file were still raising `turn.done` and `judge`
    /// through `process_event`, which carries **no `_event.data` at all**. `judging`'s `onentry`
    /// reads three keys off it and every guard in that state reads one, so those raises were asking
    /// the datamodel to index nil.
    ///
    /// **That raised `error.execution` on every one of them, and W3C SCXML 3.12.2 dropped it**: the
    /// gates ran on a `judging` whose entry block had been ABANDONED after its first assignment
    /// (W3C 3.8), and stayed green. They were only found when the document grew an edge that answers
    /// its own errors and seven of them went red at once. ⚠ There is no separate ratchet for this
    /// class and none is needed: the edge IS the detector, and the next fixture written this way
    /// fails on the state it lands in.
    ///
    /// # ⚠⚠⚠⚠⚠ AND WHY EVERY OTHER DOOR CLOSED IN ITEM 470's ROUND
    ///
    /// This is now the ONLY thing in this module that advances the machine — `engine.step()` and
    /// `engine.process_event()` are gone from every walk, and `process_event` was exactly
    /// `raise_external(e, "", "") + step()` so nothing about those walks changed but the door.
    ///
    /// The reason is [`started`]'s: a document that asks its host for an act needs the act
    /// PERFORMED, not merely recorded, and the third line below is where a document-level walk
    /// performs it. Leaving that to each fixture is how the same class of defect arrived twice —
    /// so the third line lives with the step, and a fixture cannot have one without the other.
    ///
    /// ⚠⚠ **THE ACT IS DROPPED ON PURPOSE AND THAT IS WHAT PERFORMING IT MEANS HERE.** These are
    /// ROUTING gates: there is no pane, so the sentence has nowhere to go, and the whole of the
    /// host's obligation is to empty the slot before the next one arrives — which is precisely
    /// what [`crate::outer::OuterLoop::advance`] does per pass. A gate that cares WHAT was asked
    /// for reads it off a real driver instead, over in `outer` — that gate is the one named
    /// *a run is told its first sentence and what it asks for by its own document*.
    fn carried(
        engine: &mut Engine<AiLoopPolicy>,
        host: &crate::act::Serving,
        event: AiLoopEvent,
        data: &str,
    ) {
        engine.raise_external(event, data, "");
        engine.step();
        let _performed = host.taken(crate::act::Act::Say);
        // ⚠⚠⚠⚠⚠ **AND THE ARRIVAL'S WORD, WHICH THIS HELPER LEARNED TO DRAIN THE HARD WAY** —
        // register item 470, stage 3. The two `judging -> working` edges declare `arrival.note`
        // beside their prompt, and a slot nobody empties is an OVERRUN the moment the same edge is
        // taken twice: `error.execution`, and the run ends `failed`. Four gates went red at once
        // saying `left: Failed, right: Working` on the SECOND judgement, which is exactly that
        // shape.
        //
        // ⚠⚠ IT IS THE DRIVER'S BEHAVIOUR AND NOT AN INDULGENCE. `OuterLoop::pumping` takes this
        // slot on every pass that moved, so a fixture that drives the machine by hand and never
        // empties it is testing a shape the product cannot produce — which is this workspace's own
        // recorded failure mode for hand fixtures, met from the other side.
        let _noted = host.taken(crate::act::Act::Note);
    }

    /// ⚠⚠⚠ **HOW THE MACHINE TELLS ITS DRIVER WHAT TO DO — asked of the ENGINE, because the
    /// answer decides the driver's whole shape and the document cannot settle it.**
    ///
    /// `ai_loop.scxml` reads as though it were giving instructions: `restarting` does
    /// `<send event="session.replace"/>`, `screening` does `<send event="screen.begin"/>`, and such
    /// sends between them name effects an outer driver has to perform. So the obvious driver is
    /// EVENT-DRIVEN: subscribe to the machine's sends, do what each one says.
    ///
    /// ⚠⚠⚠⚠ **THE STATE THIS GATE WAS WRITTEN ABOUT NO LONGER HAS ONE OF THOSE SENDS, AND THE
    /// COMPILER IS WHAT SAID SO** — register item 470, stage 2. It read `priming` does
    /// `<send event="prompt.start"/>` until `priming` began declaring
    /// `<send type="x-sprag-host" event="prompt.say">` instead; the generated enum stopped minting
    /// `PromptStart` and the assertion below stopped compiling. A HOST-served send is the third
    /// kind and is not what this gate is about: it is not addressed to the machine at all, so it is
    /// not raised onto a queue nobody reads, and for the states that use it the document really is
    /// the instruction. What this gate still measures is the ANNOUNCED sends that remain.
    ///
    /// **That driver cannot be written, and this gate is where that was established rather than
    /// assumed.** A targetless `<send>` is W3C SCXML 6.2's *external event to SELF*: the generated
    /// code calls `raise_external_with_meta` on the machine's OWN queue, and no transition in this
    /// document listens for any of them — so they are raised and dropped. The one handle that
    /// looks like a subscription, `Engine::get_external_queue_handle`, is for `#_parent` sends out
    /// of `<invoke>`d CHILD machines and **mints a fresh empty queue on every call**.
    ///
    /// So the driver is **STATE-DRIVEN**: it reads `get_current_state()` and acts on where the
    /// machine IS, and the machine's own published ingress partition is what says this is the
    /// intended shape — `prompt.sent` (the driver's ANSWER) is externally drivable, while
    /// `session.replace` (a supposed instruction) is not. The sends are documentation of intent
    /// that the compiler carries; the STATE is the contract.
    ///
    /// ⚠ Written as an assertion rather than as a comment because R376 paid for exactly this
    /// distinction one round ago: reading SCE's generated source said the opposite of what running
    /// it says. Whatever this gate reports is the thing to build against.
    #[test]
    fn the_machine_instructs_its_driver_through_its_state_not_through_its_sends() {
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: `start` must land in the state that asks this host for its first prompt",
        );

        // ── the door that looks like a subscription ──
        let drained = engine.get_external_queue_handle();
        let seen = drained.lock().expect("the queue mutex").len();
        assert_eq!(
            seen, 0,
            "⚠⚠⚠ `priming`'s `<onentry>` WAS just run, and this handle shows {seen} events. If it \
             ever \
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
        // ⚠⚠⚠⚠⚠ **THE ASSERTION THAT STOOD HERE IS GONE BECAUSE ITS SUBJECT IS** — register item
        // 470, and this is the second time this line has been retired by the thing it was about.
        //
        // It read `!ingress.contains(&AiLoopEvent::PromptStart)` until `priming` stopped announcing
        // a name and began declaring `<send type="x-sprag-host" event="prompt.say">`; the generated
        // enum stopped minting the variant and it stopped compiling. It was rewritten onto
        // `SessionReplace` — `restarting`'s announced send — with the note *"the day `restarting`'s
        // act moves too, this stops compiling instead of going quietly green on a name nothing
        // mints."*
        //
        // ⭐ **THAT DAY CAME, AND THE COMPILER IS WHAT SAID SO.** The document's last five untyped
        // `<send>`s are deleted; every effect they named the driver already performs through
        // `pass.do`. There is no supposed instruction left to point at, so the claim cannot be
        // written as *this one is not an ingress event* any more — it is now the stronger and
        // simpler fact that there are NONE, which is a property of the DOCUMENT and is pinned
        // where the document is measured: `sprag-gate`'s
        // `the_document_announces_nothing_to_a_machine_that_is_not_listening`, at zero.
        //
        // ⚠ Recorded rather than deleted silently, because this test's whole subject is *the sends
        // look like instructions and are not* — and the sends being gone is that argument having
        // been won, not the question going away.

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

    /// ⚠⚠⚠⚠⚠ **EVERY ENDING THIS DOCUMENT DECLARES SITS OUTSIDE THE `<parallel>`** — the
    /// arrangement [`OuterLoop::finished`] rests on, measured here rather than assumed there.
    ///
    /// # ⚠⚠⚠⚠ What this replaces, and why it is deliberately not the same ratchet
    ///
    /// `AiLoop::is_final` answered *is this an ending* until register item 470's stage 3, by naming
    /// all twenty-eight states in one exhaustive `match`. That match was a COMPILE-TIME ratchet: an
    /// eighth `<final>` added to the document broke the build, which is how `peer_gone` and
    /// `abandoned` both arrived. Removing it — the sentence was already said by the document's
    /// `<final>` elements and by `pumping`'s check, so the copy was the third telling — buys that a
    /// new ending is answered CORRECTLY the moment the document gains one, and costs that ratchet.
    ///
    /// ⚠⚠ **THE FAILURE MODE THE TRADE OPENS IS THE ONE THIS TEST HOLDS.** A `<final>` placed
    /// INSIDE the parallel still parses, still reads as an ending to anybody skimming the file, and
    /// turns `is_in_final_state` into *did EVERY region complete* — so a run that had plainly ended
    /// would be pumped for ever, silently, with nothing failing to build. The document states the
    /// arrangement beside itself; this measures that the statement is still true.
    ///
    /// ⚠ **AND THE LIVE HALF IS HERE TOO**, because a structural read alone would be a second copy
    /// of the same reasoning: the machine is driven to a real ending and asked.
    #[test]
    fn every_ending_this_document_declares_sits_outside_the_parallel() {
        /// The `id` of every `<final>` declared in `text`.
        fn finals(text: &str) -> Vec<&str> {
            const OPENS: &str = "<final id=\"";
            text.match_indices(OPENS)
                .filter_map(|(at, _)| text[at + OPENS.len()..].split('"').next())
                .collect()
        }

        let document = crate::outer::DOCUMENT;
        let (regions, endings) = document.split_once("</parallel>").expect(
            "`ai_loop.scxml` runs its work and orders regions in a `<parallel>` and this read \
             found no end to one: a probe pointed at nothing must never read as clean",
        );

        let misplaced = finals(regions);
        assert!(
            misplaced.is_empty(),
            "⚠⚠⚠⚠⚠ AN ENDING WAS DECLARED INSIDE THE `<parallel>`: {misplaced:?}. \
             `OuterLoop::finished` asks the engine whether the MACHINE is finished, and a final \
             inside a region makes that question *did every region complete* — which the orders \
             region never does. The run would end and the driver would pump it for ever. Move the \
             `<final>` out, beside the other endings, where a transition into it exits every region",
        );

        let declared = finals(endings);
        assert!(
            declared.len() >= 7,
            "this document has had seven endings since register item 534 and this read found \
             {declared:?}: a walk that stopped finding them is measuring nothing",
        );

        // ⚠⚠⚠⚠ THE LIVE HALF — the control first, on the state the run spends its life in, then
        // the ending. Both readings come off ONE machine driven the way a run drives it, so what
        // is measured is the engine's answer and not this test's opinion of the file.
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert!(
            !engine.is_in_final_state(),
            "⚠⚠⚠ THE CONTROL FAILED — `working` is where a run spends its turns, and a reader that \
             called it finished would end every run on its first pass",
        );
        carried(&mut engine, &host, AiLoopEvent::PeerGone, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::PeerGone,
            "the fixture: this event is what reaches an ending from `working`",
        );
        assert!(
            engine.is_in_final_state(),
            "⚠⚠⚠⚠⚠ THE ENGINE DOES NOT CALL THIS DOCUMENT'S ENDING AN ENDING, so the one reader of \
             that fact answers `false` for a run that is over — which is the arrangement above \
             having been broken, seen from the other side",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE FOUR ENDINGS A RUN CAN REACH FROM `working` SAY WHAT A STOP WOULD STILL HAVE
    /// TO REACH** — register item 470, stage 3, and this gate exists because a mutation went GREEN.
    ///
    /// # ⚠⚠⚠⚠ What a green mutation named, and why nothing was watching
    ///
    /// `AiLoop::driving` used to answer *which pane would a stop have to signal* from a
    /// twenty-eight-arm state match. That moved into the document as `end.publish`'s `signals`
    /// argument, and then flipping `cancelled` from `pane` to `nothing` — a change that leaves a
    /// live model running after a cancel — was **green across the whole suite**. Every `driving`
    /// gate in `driver.rs` is over a hand-written STUB plugin, never this one; the live cancel gate
    /// never enters the document's `cancelled` at all, because the Driver ends a cancelled run from
    /// OUTSIDE, at its loop top. So six of the seven words had no eye on them whatever.
    ///
    /// # ⚠⚠ Why it reads the HOST's record rather than the document's text
    ///
    /// Asserting the file says `'pane'` would be one copy of the document checked against another.
    /// What is read here is what the RUNNING machine handed this host when it entered the ending —
    /// the same reader `OuterLoop::signalling` uses — so a `<param>` that stopped being evaluated,
    /// a word outside the space, and an `<onentry>` that never fired all land here as a failure.
    ///
    /// ⚠⚠⚠⚠⚠ **THE PASS THAT WATCHES A TURN IS HANDED WHAT AN OUTAGE LOOKS LIKE** — register item
    /// 470, stage 2's other half: *the document TELLS instead of the driver FETCHING*.
    ///
    /// # ⚠⚠⚠⚠ What this replaced, and why nothing could have seen it
    ///
    /// `OuterLoop::service_failed` read `service_needle` out of the script session with a private
    /// `text_of` — the register's *behind the machine's back*. Nothing in `ai_loop.scxml` said the
    /// value was consulted, so a reader of the document could not tell that a blocked turn is
    /// matched against it at all. It now rides `pass.do` where `does` is `watch`.
    ///
    /// # ⚠⚠⚠ `None` AND `Some("")` ARE DIFFERENT ANSWERS, and that is the whole assertion
    ///
    /// The template ships an EMPTY needle, which declines the behaviour — so `Some("")` is the
    /// correct reading for an unbriefed loop and is what a caller's own words replace. `None` means
    /// the document declared no `<param>` at all, which is the move being undone. **Measured
    /// 2026-08-26: deleting that `<param>` left the entire `sprag-plugin` suite GREEN**, which is
    /// why this test exists rather than being assumed covered.
    ///
    /// # ⚠⚠⚠⚠⚠ AND IT NOW HOLDS ALL THREE OF THE CARRIED NUMBERS
    ///
    /// `within`, `awaits` and `stills` — `ready_timeout_ms`, `await_person_ms` and
    /// `handback_still_ms` — ride every one of the twelve `pass.do` sends, and this reads them off
    /// the same host record. The struct literal is EXHAUSTIVE, which is what makes that cheap: an
    /// argument added to the act without an assertion here does not compile.
    ///
    /// ⚠⚠ **THE UNBRIEFED HALF PINS THE TEMPLATE'S OWN NUMBERS AND THE BRIEFED HALF PINS A
    /// CALLER'S**, and both are needed: the first catches a `<param>` deleted, the second catches
    /// one that stopped being an `expr` — a literal in the document would satisfy the first for
    /// every run and drop every adopting repository's own numbers on the floor.
    #[test]
    fn the_pass_that_watches_a_turn_is_told_what_an_outage_looks_like() {
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠⚠ THE FIXTURE: `watch` is the pass this state asks for, and the needle rides it",
        );

        // ── THE TEMPLATE'S OWN VALUE FIRST: declared, and empty, which declines the behaviour ──
        carried(&mut engine, &host, AiLoopEvent::Pass, "");
        // ⚠⚠⚠ MEASURED RATHER THAN GUESSED: an empty list has no element to say whether it is a
        // list or a map, so how this pairing renders `may_answer`'s shipped `[]` is the engine's
        // answer and not something to assume. What must hold is that it is one of the two spellings
        // `OuterLoop::answers_of` reads as *this document approves nothing*.
        //
        // ⚠ TAKEN ONCE. `Serving::taken` REMOVES the act from its slot, so a second call answers
        // `None` and would read as the document never asking.
        let shipped = host.taken(crate::act::Act::Pass);
        let (shipped_clauses, shipped_needles) = match &shipped {
            Some(crate::act::Asked::Pass {
                answers, needles, ..
            }) => (
                answers.clone().unwrap_or_default(),
                needles.clone().unwrap_or_default(),
            ),
            other => {
                panic!("⚠⚠ THE FIXTURE: a `watch` pass must have been asked for. Got {other:?}")
            }
        };
        // ⚠⚠⚠ THE NEEDLES ARE A LIST NOW (register item 715), so the template's DECLINE crosses as
        // an empty table — and how an empty table renders is the engine's answer, exactly as it is
        // for `may_answer` two lines down. What must hold is that it is one of the two spellings
        // `OuterLoop::service_needles_of` reads as *this document declines*.
        assert!(
            matches!(shipped_needles.as_str(), "[]" | "{}"),
            "⚠⚠⚠⚠⚠ AN EMPTY NEEDLE LIST MUST CROSS AS SOMETHING THE DRIVER READS AS *DECLINED*. \
             Anything else and an unbriefed run would route blocked turns into a ten-minute wait \
             on words nobody authored. Got {shipped_needles:?}",
        );
        assert!(
            matches!(shipped_clauses.as_str(), "[]" | "{}"),
            "⚠⚠⚠⚠⚠ AN EMPTY CLAUSE LIST MUST CROSS AS SOMETHING `answers_of` READS AS *APPROVES \
             NOTHING*. Anything else and an unbriefed run would arrive with the barrier holding \
             whatever it had rather than the document's own answer — and those two differ by \
             whether a caller's approvals can be silently kept alive. Got {shipped_clauses:?}",
        );
        assert_eq!(
            shipped,
            Some(crate::act::Asked::Pass {
                does: crate::act::Does::Watch,
                needles: Some(shipped_needles),
                // ⚠⚠ AND THE BARRIER'S BOUND RIDES THE SAME ACT — register item 470, stage 2's
                // other half. This is `ready_timeout_ms`'s shipped value, carried rather than
                // fetched. `None` would be the `<param>` gone from all twelve `pass.do` sends.
                within: Some("180000".to_owned()),
                // ⚠⚠⚠ AND SO DOES WHO IS EXPECTED AT THE PANE — the third and last of item 470's
                // datamodel back doors. These are `await_person_ms` and `handback_still_ms` as the
                // template ships them: an hour of patience and fifteen seconds of stillness. Either
                // reading `None` is that `<param>` gone from all twelve sends, and the driver back
                // to fetching a person's patience out of the datamodel with nothing in the
                // document to say the number it authors is the number it waits.
                awaits: Some("3600000".to_owned()),
                stills: Some("15000".to_owned()),
                // ⚠⚠⚠⚠ AND THE LAST BACK DOOR'S ARGUMENT: `may_answer`, which the template ships
                // EMPTY — this loop approves nothing until a caller or a KIND document says so.
                //
                // ⚠⚠ THE ONE SPELLING PINNED BY BYTES ANYWHERE HERE, and only because an EMPTY
                // list has no keys and therefore no ordering to be fragile about. The briefed half
                // below carries words, and it is read STRUCTURALLY for exactly that reason.
                answers: Some(shipped_clauses),
            }),
            "⚠⚠⚠⚠⚠ THE `watch` PASS MUST CARRY THE NEEDLES, and an unbriefed document's are the \
             EMPTY LIST it ships. `None` here is the `<param>` being gone — the driver would then \
             be back to fetching `service_needles` out of the datamodel, which no reading of the \
             document could reveal",
        );

        // ── AND A CALLER'S OWN WORDS REACH THE SAME PASS ──
        //
        // ⚠⚠ A SECOND MACHINE, BRIEFED BEFORE IT STARTS, and the first draft got this wrong: the
        // `brief` transition belongs to `idle`, so a brief raised at `working` is an event nothing
        // handles and every `<assign>` in it is skipped. The gate then read the template's empty
        // needle and reported it as the briefed one being dropped — a true failure with a false
        // diagnosis, which is worse than a red.
        // ⚠⚠⚠ TWO OF THEM, since register item 715 made this a LIST — and two rather than one so
        // that a crossing which carried only the head would be red here rather than green.
        let outage = ["API Error: 529 Overloaded", "Usage limit reached"];
        // ⚠ DELIBERATELY NOT THE SHIPPED 180000, so the assertion below cannot be satisfied by the
        // document's own default standing in for a value the brief was supposed to replace.
        let bound = 4321;
        // ⚠ AND NEITHER OF THESE IS THE SHIPPED PAIR, for `bound`'s reason exactly. They are also
        // deliberately UNEQUAL TO EACH OTHER, so a reader that carried one number twice — one
        // `<param>` copied and its `expr` left pointing at its neighbour's key — cannot satisfy
        // both assertions with one value.
        let patience = 7654;
        let stillness = 321;
        // ⚠⚠ AND THE CLAUSE LIST, WHICH THE TEMPLATE SHIPS EMPTY — so a briefed one is the ONLY
        // way this argument can hold words at all, and reading them back off the act is what says
        // the list crossed rather than merely the `<param>` existing.
        let approved = serde_json::json!([{ "asked": "trust the files here", "answer": "yes" }]);
        let (mut engine, host, _lua, _session) = started();
        carried(
            &mut engine,
            &host,
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": "n",
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
                "service_needles": outage
                    .iter()
                    .map(|says| serde_json::json!({ "says": says }))
                    .collect::<Vec<_>>(),
                "ready_timeout_ms": bound,
                "await_person_ms": patience,
                "handback_still_ms": stillness,
                "may_answer": approved,
            })
            .to_string(),
        );
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        carried(&mut engine, &host, AiLoopEvent::Pass, "");
        let briefed = host.taken(crate::act::Act::Pass);
        // ⚠⚠⚠⚠⚠ **DESTRUCTURED RATHER THAN COMPARED WHOLE, AND EXHAUSTIVELY (no `..`)** — so an
        // argument added to this act without an assertion here still does not compile, which is
        // what the struct literal above buys. The reason the equality could not stay is `answers`:
        // it carries WORDS now, and the crossing alphabetises the keys of every object it renders.
        // **That ordering is the serialiser's to choose, so pinning these bytes would be a gate
        // that goes red for a harmless change** — the trap register item 470 keeps paying for.
        let Some(crate::act::Asked::Pass {
            does,
            needles,
            within,
            awaits,
            stills,
            answers,
        }) = briefed
        else {
            panic!(
                "⚠⚠ THE FIXTURE: a briefed `watch` pass must have been asked for. Got {briefed:?}"
            )
        };
        assert_eq!(
            (does, within, awaits, stills),
            (
                crate::act::Does::Watch,
                // ⚠ A BRIEFED BOUND REACHES THE SAME ACT, and this brief sets one: the arguments
                // are carried together and a reader of one must see the others move.
                Some(bound.to_string()),
                // ⚠⚠ AND A BRIEFED PERSON REACHES IT TOO: a caller's patience is the number
                // `awaiting_human` waits out, and until this act carried it the only way it reached
                // the barrier was a read no reader of `ai_loop.scxml` could have found.
                Some(patience.to_string()),
                Some(stillness.to_string()),
            ),
            "⚠⚠⚠⚠ THE BRIEFED NUMBERS MUST REACH THE PASS THAT READS THEM",
        );

        // ── ⭐ AND THE BRIEFED NEEDLES REACH IT WITH THEIR WORDS ──
        //
        // ⚠⚠ READ AS STRUCTURE rather than pinned by bytes, for `answers`' measured reason below:
        // the crossing alphabetises the keys of every object it renders, and that ordering is the
        // serialiser's to choose. What must hold is that BOTH sentences arrived — a `<param>` that
        // became a literal, or a crossing that kept only the first element, drops an adopting
        // repository's own words on the floor and the run goes back to dying on them.
        let carried_needles = needles.unwrap_or_default();
        let read_back: Vec<String> = serde_json::from_str::<serde_json::Value>(&carried_needles)
            .ok()
            .and_then(|held| match held {
                serde_json::Value::Array(items) => Some(
                    items
                        .iter()
                        .filter_map(|item| {
                            item.get("says")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠⚠⚠ THE BRIEFED NEEDLE LIST DID NOT CROSS AS SOMETHING THIS DRIVER CAN \
                     WALK. `OuterLoop::service_needles_of` parses exactly this string, so \
                     whatever it could not read here it cannot read on a live run: \
                     {carried_needles:?}"
                )
            });
        assert_eq!(
            read_back,
            outage.map(str::to_owned).to_vec(),
            "⚠⚠⚠⚠ A BRIEFED NEEDLE SET MUST REACH THE PASS THAT WOULD MEET IT, WHOLE. If this \
             reads empty, the `<param>` is a literal rather than the datamodel's value; if it \
             reads one element, the set is being flattened somewhere between the brief and the \
             pass and item 715 is only half paid",
        );

        // ── ⭐ AND THE CLAUSE LIST REACHES IT WITH ITS WORDS ──
        //
        // This is the argument a round wrote off as uncarryable. Read as STRUCTURE: what must hold
        // is that the caller's clause ARRIVED, with both of its strings — a crossing that handed
        // over a handle, an empty string or `[]` would leave a run approving nothing while its
        // caller's brief plainly approved something, and the run would look configured.
        let said = answers.unwrap_or_default();
        let held: serde_json::Value = serde_json::from_str(&said).unwrap_or_else(|why| {
            panic!(
                "⚠⚠⚠⚠⚠ THE BRIEFED CLAUSE LIST DID NOT CROSS AS SOMETHING THIS DRIVER CAN WALK. \
                 `OuterLoop::answers_of` parses exactly this string, so whatever it could not read \
                 leaves the barrier holding what it had — and a caller's approvals never arrive. \
                 Got {said:?}: {why}",
            )
        });
        assert_eq!(
            held, approved,
            "⚠⚠⚠⚠⚠ THE CALLER'S OWN CLAUSE MUST BE WHAT THE ACT CARRIES, whole. Compared as \
             PARSED VALUES rather than as text, so this says the clause survived and says nothing \
             about which order the crossing writes an object's keys in",
        );
    }

    /// ⚠⚠⚠⚠⚠ **EVERY WORD A DRIVEN STATE ANSWERS ABOUT BEING ASKED FOR AN ACCOUNT** — register
    /// item 470, stage 3, and this gate exists for the same reason as the one below it: a mutation
    /// went GREEN.
    ///
    /// # ⚠⚠⚠⚠ What was unwatched, and why nothing noticed
    ///
    /// `AiLoop::ask_for_an_account` chose between granting an account window and twenty ways of
    /// refusing one from a twenty-eight-arm state match. That moved into the document as
    /// `account.ask`'s `can` argument — and then flipping `service_down` to say `within` was green
    /// across the whole suite, because **`ask_for_an_account` is only ever called when one of the
    /// RUN's own ceilings falls due**, and no gate trips a ceiling while the machine is waiting an
    /// outage out. The word was carried across from the old match and never proven.
    ///
    /// # ⚠⚠ It asks the DOCUMENT the way the driver does, and reads the HOST's record
    ///
    /// `OuterLoop::asked_of_this_account` raises `account` and takes what this host was handed.
    /// Raising the same event against a machine driven to each state measures the same thing
    /// without needing a live pane and a real ceiling — and it is the running document that
    /// answers, not this test's reading of the file.
    ///
    /// ⚠ **THE STATES WITH NO WORD ARE PART OF THE CLAIM.** Five words cover the fifteen states a
    /// driver drives; the endings answer nothing here because a finished machine is told apart by
    /// having PUBLISHED one, and `None` is what `ask_for_an_account` turns into *this driver has no
    /// answer for what it is looking at* rather than a guess.
    #[test]
    fn every_driven_state_says_whether_its_agent_can_be_asked_for_an_account() {
        /// A turn that ended because the peer's SERVICE was not answering — `working`'s own first
        /// `turn.blocked` guard, and the only road to `service_down`.
        const BLOCKED_BY_SERVICE: &str = r#"{"service": true, "judged": false}"#;

        let started_at = |events: &[(AiLoopEvent, &str)]| {
            let (mut engine, host, _lua, _session) = started();
            for (event, data) in events {
                carried(&mut engine, &host, *event, data);
            }
            (engine, host)
        };

        for (route, state, can) in [
            // ⚠ NOTHING RAISED AT ALL: the loop never got its pane, so its agent was never asked
            // anything and has nothing to account for.
            (vec![], AiLoopState::Idle, crate::act::Accounts::NeverAsked),
            (
                vec![(AiLoopEvent::Start, ""), (AiLoopEvent::PromptSent, "")],
                AiLoopState::Working,
                crate::act::Accounts::Within,
            ),
            // ⚠⚠ SOMEBODY ELSE HAS THE PANE. Asking here would answer their dialog or type under
            // their hand, which is the one thing this driver must never do.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnInterrupted, ""),
                ],
                AiLoopState::AwaitingHuman,
                crate::act::Accounts::NotOurs,
            ),
            // ⚠⚠ THE AGENT THAT DID THE WORK IS BEING REPLACED and its successor has done none of
            // it, so there is nobody to ask where the run got to.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, DONE),
                ],
                AiLoopState::Reflecting,
                crate::act::Accounts::BetweenSessions,
            ),
            // ⭐⭐ THE ONE THE GREEN MUTATION WAS ABOUT. Typing here is ALLOWED — nobody's hand is in
            // the pane — and it would still buy nothing, because the answer has to come back from
            // the same service that just refused a turn.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnBlocked, BLOCKED_BY_SERVICE),
                ],
                AiLoopState::ServiceDown,
                crate::act::Accounts::ServiceDown,
            ),
        ] {
            let (mut engine, host) = started_at(&route);
            // ⚠⚠⚠ THE ACTIVE SET AND NOT `get_current_state`, because this document has REGIONS:
            // the flattening call answers the parallel ROOT for a machine that has not left `idle`,
            // which is `probe_parallel.scxml`'s measured finding and the reason `OuterLoop::state`
            // filters to the work region by name. A fixture keyed on the flattened value would have
            // been asserting about `running` while believing it asked about `idle`.
            let active = engine.get_active_states();
            assert!(
                active.contains(&state),
                "⚠⚠⚠ THE FIXTURE: this route is written to reach {state:?} and the word below is \
                 that state's. active = {active:?}, refused acts: {:?}",
                host.refused(),
            );
            assert_eq!(
                host.taken(crate::act::Act::Account),
                None,
                "⚠⚠⚠⚠ THE CONTROL: nothing has answered this question before it is asked. An act \
                 already waiting would mean the reading below belongs to some earlier pass",
            );

            carried(&mut engine, &host, AiLoopEvent::Account, "");
            assert_eq!(
                host.taken(crate::act::Act::Account),
                Some(crate::act::Asked::Account { can }),
                "⚠⚠⚠⚠⚠ {state:?} ANSWERS THE WRONG THING ABOUT BEING ASKED FOR AN ACCOUNT. Each \
                 word names something true about the PANE that makes the question askable or not — \
                 nobody was ever asked anything, somebody else's hand is in it, the agent is being \
                 replaced, the service is not answering — and a caller reading the run's journal \
                 gets that reason instead of a blank report",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **EVERY DRIVEN STATE SAYS WHAT A PASS OF IT IS FOR** — the twin of the gate above,
    /// and the fact register item 470's last driver arms were standing on.
    ///
    /// # ⚠⚠⚠⚠ What this measures that nothing measured before
    ///
    /// `AiLoop::unbuilt` carried two arms keyed on `AiLoopState::AwaitingHuman`, deciding a verdict
    /// for a run sitting in it. They were written when this driver had no act for that state — and
    /// `attend` was built, so the document answers its `pass` now and those arms could not fire.
    /// **That was an ARGUMENT, not a measurement, and this register has already been bitten by an
    /// unreachability argument that aged.** So the fact is asserted instead of reasoned: the
    /// document is driven to each state and asked, and what is read is the HOST's record.
    ///
    /// ⚠⚠ **A STATE THAT STOPPED ANSWERING WOULD REACH `unbuilt`**, and with the arms gone the run
    /// stops with the document's own missing line named. That is the right answer and the arms were
    /// the wrong one: they substituted a driver decision for a line an author forgot, which is the
    /// one thing this whole arrangement exists to prevent.
    ///
    /// ⚠ `..` IN THE PATTERN IS DELIBERATE HERE. What this gate is about is the WORD, and the
    /// arguments riding beside it have their own exhaustive gate
    /// (`the_pass_that_watches_a_turn_is_told_what_an_outage_looks_like`) — two gates pinning the
    /// same five fields would make every new argument cost two edits and buy one.
    ///
    /// # ⛔⛔⛔⛔⛔ AND THE WORD `EVERY` WAS A CLAIM NOTHING MEASURED — register item 749
    ///
    /// This gate walked SEVEN of the document's sixteen driven states and was named for all of
    /// them. Register item 741 then added a sixteenth, `unverified`, and forgot to give it a
    /// `<transition event="pass">` arm — **and every gate in this crate stayed green**, because the
    /// table below is a hand-written list and nothing contrasted it with the document. What that
    /// cost was measured live: a checker that stops at a permission dialog answers nothing, the run
    /// reaches `unverified`, the driver answers `Pumped::Unbuilt`, and the round ends `failed` with
    /// its work unbanked (watching-zenoh's watcher, run 68, 2026-08-29).
    ///
    /// ⚠⚠⚠⚠⚠ **SO THE LIST IS NO LONGER ALLOWED TO BE SHORT.** Two contrasts stand at the bottom of
    /// this function, and neither is a second copy of anything:
    ///
    /// * **the population is derived, not written** — every `id` the DOCUMENT declares, classified
    ///   by the COMPILED topology (`is_descendant_of(_, work)` and atomic), which is SCE's own
    ///   reading of the same file;
    /// * **that set must equal the `In(…)` names in the document's `pass` table** — the arm exists;
    /// * **and it must equal the states this table actually WALKED** — the arm answers.
    ///
    /// ⚠⚠ THERE IS NO EXEMPTION LIST AND THERE MUST NOT BE ONE. A driven state nobody classified is
    /// RED rather than skipped, which is the whole shape of the defect being paid off here: the way
    /// `unverified` got through was by being in no list at all.
    #[test]
    fn every_driven_state_says_what_a_pass_of_it_is_for() {
        /// A turn that ended because the peer's SERVICE was not answering — `working`'s own first
        /// `turn.blocked` guard, and the only road to `service_down`.
        const BLOCKED_BY_SERVICE: &str = r#"{"service": true, "judged": false}"#;
        /// A blocked turn that is neither an outage nor a decision — an ordinary tool dialog, which
        /// is `working`'s LAST `turn.blocked` arm and the only road to `screening`.
        const BLOCKED_BY_DIALOG: &str = r#"{"service": false, "judged": false}"#;
        /// And one the driver's judge called a DESIGN decision — the middle arm, and the only road
        /// to `redirecting`. ⚠ Nothing in the product publishes a `true` for this key yet; the
        /// document has the route and says so, and a fixture is how a route with no producer is
        /// still measured rather than argued about.
        const BLOCKED_BY_DESIGN: &str = r#"{"service": false, "judged": true}"#;
        /// **A MILESTONE AN INDEPENDENT CHECK REFUSED** — [`DONE`]'s six keys with the word in
        /// `checked`, which is `judging`'s road to `disputing`.
        ///
        /// ⚠ NOT SPELLED `REFUSED`, and a gate is what said so:
        /// `no_payload_key_is_spelled_by_a_name_this_workspace_disagrees_about` refuses a payload
        /// name four other declarations in this workspace already answer to, because a claim about
        /// what the driver sends would then rest on whichever was read last.
        const CHECK_SAID_NO: &str = r#"{"done": true, "checked": "failed", "explained": false, "unheard": false, "silence": false, "stop_short": false}"#;
        /// **AND ONE IT COULD NOT ANSWER AT ALL** — the same shape with `silent`, and the road to
        /// `unverified`. ⛔ `silence` carries the CLASS because that state's `onentry` branches on
        /// it; a payload short of it takes the readable arm on a word this document has not met,
        /// which is the wrong half of register item 741 and not what this route is for.
        const CHECK_SAID_NOTHING: &str = r#"{"done": true, "checked": "silent", "explained": false, "unheard": false, "silence": "unanswered", "stop_short": false}"#;
        /// **WHAT THE DRIVER PUTS ON `reflect.applied`** (`OuterLoop::reflect`) — the three slots a
        /// reflection may rewrite, and the road from `reflecting` to `reviewing`. ⚠ All three are
        /// sent because that transition assigns UNCONDITIONALLY: a fixture short of one puts nil
        /// over a slot the later prompts compose, which is [`TURN`]'s lesson under item 505.
        const APPLIED: &str = r#"{"milestone": "the next thing", "reference": "", "standing": ""}"#;

        // **THE STATES THIS WALK ACTUALLY REACHED**, which is what the contrast at the bottom
        // holds the table to. Filled from `AiLoopPolicy::get_state_name` rather than from a
        // spelling written here, so it says the same words the document does.
        let mut walked: std::collections::BTreeSet<&'static str> =
            std::collections::BTreeSet::new();

        for (route, state, does) in [
            (vec![], AiLoopState::Idle, crate::act::Does::Ready),
            (
                vec![(AiLoopEvent::Start, "")],
                AiLoopState::Priming,
                crate::act::Does::Sent,
            ),
            (
                vec![(AiLoopEvent::Start, ""), (AiLoopEvent::PromptSent, "")],
                AiLoopState::Working,
                crate::act::Does::Watch,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                ],
                AiLoopState::Judging,
                crate::act::Does::Judge,
            ),
            // ⭐⭐⭐ **THE ONE THIS GATE WAS WRITTEN FOR.** Two arms of `AiLoop::unbuilt` decided a
            // verdict for this state on the grounds that no act existed for it. One does.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnInterrupted, ""),
                ],
                AiLoopState::AwaitingHuman,
                crate::act::Does::Attend,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, DONE),
                ],
                AiLoopState::Reflecting,
                crate::act::Does::Reflect,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnBlocked, BLOCKED_BY_SERVICE),
                ],
                AiLoopState::ServiceDown,
                crate::act::Does::Wait,
            ),
            // ── ⛔ THE NINE THIS TABLE NEVER WALKED, AND THE LAST OF THEM IS ITEM 749 ──────────
            //
            // Every one of these was a driven state the gate named `every` was silent about. They
            // are written in the order a run meets them rather than alphabetically, so a reader
            // can check each route against the document's own edges.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnBlocked, BLOCKED_BY_DIALOG),
                ],
                AiLoopState::Screening,
                crate::act::Does::Screen,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnBlocked, BLOCKED_BY_DESIGN),
                ],
                AiLoopState::Redirecting,
                crate::act::Does::Redirect,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, CHECK_SAID_NO),
                ],
                AiLoopState::Disputing,
                crate::act::Does::Sent,
            ),
            // ⛔⛔⛔⛔⛔ **REGISTER ITEM 749, AND THE ROUTE IS THE ONE A REAL ROUND TOOK.** A run
            // said it reached its milestone, the independent check answered something that was not
            // a verdict, and the document sent it here. Until this round the word below did not
            // exist: `unverified` declared no `pass` arm, so this very walk ended `Unbuilt`.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, CHECK_SAID_NOTHING),
                ],
                AiLoopState::Unverified,
                crate::act::Does::Sent,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, STOP_SHORT),
                ],
                AiLoopState::Stopping,
                crate::act::Does::Watch,
            ),
            // ⚠ A BANKED TURN UNDER A STANDING-DOWN ORDER, which is the only road to `closing` —
            // `every_ending_says_what_a_stop_must_still_reach`'s own fixture, one region over.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::StandDown, ""),
                    (AiLoopEvent::Judge, DONE),
                ],
                AiLoopState::Closing,
                crate::act::Does::Watch,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, DONE),
                    (AiLoopEvent::ReflectApplied, APPLIED),
                ],
                AiLoopState::Reviewing,
                crate::act::Does::Review,
            ),
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, DONE),
                    (AiLoopEvent::ReflectApplied, APPLIED),
                    (AiLoopEvent::ReviewNone, ""),
                ],
                AiLoopState::Restarting,
                crate::act::Does::Replace,
            ),
            // ⚠ THE REPLACEMENT AND THE WAIT ARE TWO STATES, which is the document's own reason for
            // `resuming` existing at all — so reaching it costs one more event than `restarting`.
            (
                vec![
                    (AiLoopEvent::Start, ""),
                    (AiLoopEvent::PromptSent, ""),
                    (AiLoopEvent::TurnDone, TURN),
                    (AiLoopEvent::Judge, DONE),
                    (AiLoopEvent::ReflectApplied, APPLIED),
                    (AiLoopEvent::ReviewNone, ""),
                    (AiLoopEvent::SessionReplaced, ""),
                ],
                AiLoopState::Resuming,
                crate::act::Does::Resume,
            ),
        ] {
            walked.insert(AiLoopPolicy::get_state_name(state));
            let (mut engine, host, _lua, _session) = started();
            for (event, data) in &route {
                carried(&mut engine, &host, *event, data);
            }
            // ⚠⚠⚠ THE ACTIVE SET AND NOT `get_current_state`, for the account gate's measured
            // reason: this document has REGIONS, and the flattening call answers the parallel ROOT
            // for a machine that has not left `idle`.
            let active = engine.get_active_states();
            assert!(
                active.contains(&state),
                "⚠⚠⚠ THE FIXTURE: this route is written to reach {state:?} and the word below is \
                 that state's. active = {active:?}, refused acts: {:?}",
                host.refused(),
            );
            assert_eq!(
                host.taken(crate::act::Act::Pass),
                None,
                "⚠⚠⚠⚠ THE CONTROL: nothing has asked for a pass before this one is raised. An act \
                 already waiting would mean the reading below belongs to some earlier step",
            );

            carried(&mut engine, &host, AiLoopEvent::Pass, "");
            let taken = host.taken(crate::act::Act::Pass);
            let Some(crate::act::Asked::Pass { does: said, .. }) = taken else {
                panic!(
                    "⚠⚠⚠⚠⚠ {state:?} ASKED FOR NOTHING ON ITS PASS, so a run that reached it would \
                     be reported `Unbuilt` and STOPPED — and until this gate existed, two arms of \
                     `AiLoop::unbuilt` quietly answered for it instead of the document. Got \
                     {taken:?}, refused acts: {:?}",
                    host.refused(),
                )
            };
            assert_eq!(
                said, does,
                "⚠⚠⚠⚠ {state:?} SAYS ITS PASS IS FOR THE WRONG THING. The word chooses the effect \
                 this driver performs, so a state answering its neighbour's is a run doing \
                 something nobody asked for — silently, because both words are served",
            );
        }

        // ══ ⛔⛔⛔⛔⛔ AND NOW THE HALF THAT WAS MISSING: IS THE TABLE ABOVE COMPLETE? ═══════════
        //
        // Register item 749. Everything above this line measures the states somebody REMEMBERED to
        // write down, which is precisely the property that was true while `unverified` had no arm
        // and every round with a blocked checker ended `failed`.
        let document = crate::outer::DOCUMENT;
        let declared = declared_state_ids(document);

        // ⚠⚠⚠⚠⚠ THE PREMISE, ASSERTED INSIDE THE GATE RATHER THAN ASSUMED BY IT. Both sets below
        // are read out of the document by a scan, and a scan that found NOTHING would make every
        // contrast trivially true — a gate blind to its own eye, which is the shape this register
        // keeps meeting. So the walk above is what proves the scan can see: each state this gate
        // actually drove must be an id the scan read back out of the file.
        for name in &walked {
            assert!(
                declared.contains(name),
                "⚠⚠⚠⚠⚠ THE SCAN IS BLIND: this gate drove a real engine into `{name}` and the read \
                 of `ai_loop.scxml` did not find that id. Every contrast below is therefore \
                 comparing empty sets and would pass whatever the document said. Read back \
                 {} ids: {declared:?}",
                declared.len(),
            );
        }

        // ⚠⚠⚠⚠⚠ **THE POPULATION IS DERIVED, NOT WRITTEN.** A state a run can be DRIVEN in is an
        // id this document declares that the COMPILER put inside the `work` region and left atomic
        // — SCE's own reading of the same file, so this cannot drift from the topology the way a
        // list in Rust does. The finals fall out on their own: this document keeps them OUTSIDE
        // the parallel, so `get_parent` answers `None` for every one of them.
        //
        // ⚠⚠ AND AN ID NOBODY CLASSIFIED IS RED RATHER THAN SKIPPED. A `<state>` the compiled
        // topology does not know is a document and a build that have come apart, and passing it
        // over is the exemption that would let the next `unverified` through.
        let driven: std::collections::BTreeSet<&str> = declared
            .iter()
            .copied()
            .filter(|id| {
                let state = AiLoopPolicy::get_state_from_name(id).unwrap_or_else(|| {
                    panic!(
                        "⛔⛔⛔ `{id}` IS A STATE THIS DOCUMENT DECLARES AND THE COMPILED TOPOLOGY \
                         DOES NOT KNOW. The generated enum comes from this very file, so the two \
                         disagreeing means the build is holding a different document than the one \
                         being read here — and nothing below can be trusted until that is true",
                    )
                });
                AiLoopPolicy::is_descendant_of(state, AiLoopState::Work)
                    && !AiLoopPolicy::is_compound_state(state)
                    && !AiLoopPolicy::is_parallel_state(state)
            })
            .collect();

        // ⭐⭐⭐⭐⭐ **CONTRAST ONE — THE ARM EXISTS.** This is register item 749's own sentence: the
        // set of states a run can be driven in, against the set the `pass` table names. `unverified`
        // was in the first and not the second for a whole day of runs, and no gate anywhere asked.
        let armed: std::collections::BTreeSet<&str> =
            states_the_pass_table_names(document).into_iter().collect();
        assert_eq!(
            driven, armed,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 749: the `pass` table and the states a run can be driven in \
             have come apart. A state in the LEFT set and not the right declares no act, so a run \
             that reaches it is answered by nobody — `Pumped::Unbuilt`, and the round ends \
             `failed` with its work unbanked. A name in the RIGHT set and not the left is an arm \
             for somewhere this document cannot be, which is a guard that can never fire",
        );

        // ⭐⭐⭐ **CONTRAST TWO — THE ARM ANSWERS.** An `In(…)` in the document proves a line was
        // written; only a walk proves the line WORKS. So the table above must reach every driven
        // state, and a state added to this document costs a route here or this gate is red.
        assert_eq!(
            driven, walked,
            "⛔⛔⛔⛔ THE WORD `EVERY` IN THIS GATE'S NAME IS A CLAIM, AND IT JUST FAILED. A driven \
             state this table does not walk is one whose arm is asserted by nobody: it could name \
             its neighbour's act, or none at all, and every gate in this crate would stay green — \
             which is exactly what happened to `unverified` between register items 741 and 749",
        );
    }

    /// **EVERY `id` `ai_loop.scxml` DECLARES AS A STATE**, in document order.
    ///
    /// ⚠ It scans for the attribute rather than parsing XML, which is `outer::declared_data_ids`'s
    /// argument verbatim: an id is an NCName so it cannot contain a quote, and a real parser here
    /// would be a second reading of a document this crate already compiles. What the ids are FOR is
    /// decided by the compiled topology, not by which of the three spellings declared them.
    fn declared_state_ids(document: &str) -> Vec<&str> {
        ["<state id=\"", "<final id=\"", "<parallel id=\""]
            .into_iter()
            .flat_map(|needle| {
                document.match_indices(needle).filter_map(|(at, found)| {
                    let rest = &document[at + found.len()..];
                    rest.find('"').map(|end| &rest[..end])
                })
            })
            .collect()
    }

    /// **EVERY STATE THE `pass` TABLE NAMES IN AN `In('…')`**, which is the set of states that
    /// document answers a pass for.
    ///
    /// ⚠⚠ THE START TAG ONLY, and the quote tracking is why: a `cond` may hold `&gt;` but it may
    /// also hold a bare `>`, and a scan that stopped at the first one would read half a guard and
    /// call the rest of the file part of it.
    fn states_the_pass_table_names(document: &str) -> Vec<&str> {
        document
            .match_indices("<transition event=\"pass\"")
            .flat_map(|(at, _)| {
                let rest = &document[at..];
                let mut quoted = false;
                let end = rest
                    .char_indices()
                    .find(|&(_, ch)| match ch {
                        '"' => {
                            quoted = !quoted;
                            false
                        }
                        '>' => !quoted,
                        _ => false,
                    })
                    .map_or(rest.len(), |(end, _)| end);
                let tag = &rest[..end];
                tag.match_indices("In('")
                    .filter_map(|(at, found)| {
                        let rest = &tag[at + found.len()..];
                        rest.find('\'').map(|end| &rest[..end])
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// ⭐ **ALL SEVEN, and the last three cost one intermediate state each.** `converged` is reached
    /// through `closing` (a banked turn with the order STANDING DOWN), `exhausted` through
    /// `stopping` (the document's own `max_turns` spent), and `abandoned` from `held` in the ORDERS
    /// region — which is two events from a started machine and never touches `working` at all.
    /// ⚠ That last one is why this gate is not named for `working`: three of the seven are reached
    /// from somewhere else, and a name claiming otherwise would be the gate lying about its own
    /// coverage.
    #[test]
    fn every_ending_says_what_a_stop_must_still_reach() {
        // ── THE THREE THAT NEED AN INTERMEDIATE STATE, each driven the way a run reaches it ──
        //
        // ⚠⚠ `abandoned` NEVER ENTERS `working`. It is the one ending the ORDERS region reaches on
        // its own: a person holds the loop and does not come back inside the document's bound. So
        // it is driven from `standing`, and the fixture asserts it got there.
        let held = |engine: &mut Engine<AiLoopPolicy>, host: &crate::act::Serving| {
            carried(engine, host, AiLoopEvent::Start, "");
            carried(engine, host, AiLoopEvent::PromptSent, "");
            carried(engine, host, AiLoopEvent::Hold, "");
            let active = engine.get_active_states();
            assert!(
                active.contains(&AiLoopState::Held),
                "⚠⚠⚠ THE FIXTURE: `abandon` is raised from `held` and nowhere else. active = \
                 {active:?}",
            );
        };
        // ⚠ A BANKED TURN WITH THE ORDER STANDING DOWN is what reaches `closing`, and `closing`'s
        // own completed turn is what converges. Both halves are the document's, not this test's.
        let closing = |engine: &mut Engine<AiLoopPolicy>, host: &crate::act::Serving| {
            carried(engine, host, AiLoopEvent::Start, "");
            carried(engine, host, AiLoopEvent::PromptSent, "");
            carried(engine, host, AiLoopEvent::TurnDone, TURN);
            carried(engine, host, AiLoopEvent::StandDown, "");
            carried(engine, host, AiLoopEvent::Judge, DONE);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Closing,
                "⚠⚠⚠ THE FIXTURE: only a done judgement under a standing-down order reaches \
                 `closing`, and `converged` is only reachable through it",
            );
        };
        // ⚠⚠ ONE JUDGEMENT CARRYING `stop_short`, which is the document's own FIRST arm out of
        // `judging` — *a ceiling of the run's fell due, so ask for an account*. It is the edge a
        // real run takes to `stopping`, and it is one event rather than a whole spent budget.
        //
        // ⚠ THE LONG ROUTE WAS TRIED FIRST AND REFUSED ITSELF: judging ordinary turns until
        // `max_turns` runs out walks through `reflecting` (`reflect_every`), and driving that with
        // `reflect.none` reached `failed` with NO refused act — the document's own content raising
        // `error.execution` on a path a real reflection would have prepared. The fixture said so
        // rather than measuring `exhausted` on a machine that never got there, which is the
        // fixture working; this route avoids the question instead of pretending to answer it.
        let stopping = |engine: &mut Engine<AiLoopPolicy>, host: &crate::act::Serving| {
            carried(engine, host, AiLoopEvent::Start, "");
            carried(engine, host, AiLoopEvent::PromptSent, "");
            carried(engine, host, AiLoopEvent::TurnDone, TURN);
            carried(engine, host, AiLoopEvent::Judge, STOP_SHORT);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Stopping,
                "⚠⚠⚠ THE FIXTURE: a judgement carrying `stop_short` is what asks for an account, \
                 and `exhausted` is only reachable through the state it reaches",
            );
        };

        for (reach, route, publishes, signals) in [
            (
                Some(&held as &dyn Fn(&mut Engine<AiLoopPolicy>, &crate::act::Serving)),
                vec![AiLoopEvent::Abandon],
                crate::act::Publishes::Abandoned,
                // ⚠⚠ THE ARM WHERE THE FAIL-SAFE DOES REAL WORK: a hold PARKS the loop and leaves
                // the turn its agent is in the middle of alone, so the last thing anybody knows is
                // that it was working — and nobody has looked since.
                crate::act::Signals::Pane,
            ),
            (
                Some(&closing as &dyn Fn(&mut Engine<AiLoopPolicy>, &crate::act::Serving)),
                vec![AiLoopEvent::TurnDone],
                crate::act::Publishes::Converged,
                crate::act::Signals::Nothing,
            ),
            (
                Some(&stopping as &dyn Fn(&mut Engine<AiLoopPolicy>, &crate::act::Serving)),
                vec![AiLoopEvent::TurnDone],
                crate::act::Publishes::Exhausted,
                crate::act::Signals::Nothing,
            ),
        ] {
            let (mut engine, host, _lua, _session) = started();
            reach.expect("each row above carries its own route")(&mut engine, &host);
            assert_eq!(
                host.signalling(),
                None,
                "⚠⚠⚠⚠ THE CONTROL: nothing has been published before the ending is entered",
            );
            for event in route {
                carried(&mut engine, &host, event, "");
            }
            assert!(
                engine.is_in_final_state(),
                "the fixture: this route must reach an ending. Saw {:?}",
                engine.get_current_state(),
            );
            assert_eq!(
                host.published(),
                Some(publishes),
                "⚠⚠⚠ the ending this route reaches must publish its own word",
            );
            assert_eq!(
                host.signalling(),
                Some(signals),
                "⚠⚠⚠⚠⚠ `{publishes:?}` SAYS THE WRONG THING ABOUT WHAT A STOP MUST REACH",
            );
        }

        // ── AND THE FOUR THAT ARE ONE OR TWO EVENTS FROM `working` ──
        for (route, publishes, signals) in [
            (
                vec![AiLoopEvent::Fail],
                crate::act::Publishes::Failed,
                crate::act::Signals::Pane,
            ),
            // ⚠⚠⚠ THE ONE THE GREEN MUTATION WAS ABOUT. `cancel` is reached because the run ended
            // WHILE THE AGENT WAS MID-TURN, so the peer is working and a stop has somewhere to go.
            (
                vec![AiLoopEvent::Cancel],
                crate::act::Publishes::Cancelled,
                crate::act::Signals::Pane,
            ),
            // ⚠⚠ THE ONE ANSWERED ON EVIDENCE RATHER THAN ON THE FAIL-SAFE: this state is only
            // reached because the pane's child was SEEN to have exited.
            (
                vec![AiLoopEvent::PeerGone],
                crate::act::Publishes::PeerGone,
                crate::act::Signals::Nothing,
            ),
            (
                vec![AiLoopEvent::TurnInterrupted, AiLoopEvent::Unattended],
                crate::act::Publishes::Blocked,
                crate::act::Signals::Pane,
            ),
        ] {
            let (mut engine, host, _lua, _session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Working,
                "⚠⚠⚠ THE FIXTURE: every route below is raised from `working` and nowhere else",
            );
            assert_eq!(
                host.signalling(),
                None,
                "⚠⚠⚠⚠ THE CONTROL, and it is the arm a cancelled run actually takes: a machine \
                 that has not reached an ending has published NOTHING, which `AiLoop::driving` \
                 folds in with *there IS a pane*. If this ever answers here, that fold has lost \
                 the case it was written for",
            );

            for event in route {
                carried(&mut engine, &host, event, "");
            }
            assert!(
                engine.is_in_final_state(),
                "the fixture: this route must reach an ending, or nothing below is about one. \
                 Saw {:?}",
                engine.get_current_state(),
            );
            assert_eq!(
                host.published(),
                Some(publishes),
                "⚠⚠⚠ the ending this route reaches must publish its own word",
            );
            assert_eq!(
                host.signalling(),
                Some(signals),
                "⚠⚠⚠⚠⚠ THIS ENDING SAYS THE WRONG THING ABOUT WHAT A STOP MUST REACH. \
                 `{publishes:?}` declares `{signals:?}` because of what is true of its PANE, and \
                 the direction fails safe: a needless interrupt costs a peer one keystroke it was \
                 waiting at anyway, and a missed one leaves a model spending somebody's tokens on \
                 a question nothing is waiting for",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **A PEER THAT WENT SILENT AND A PEER THAT IS GONE ARE TWO WORDS AND TWO
    /// DESTINATIONS** — register item 458's edge, and the document's own answer to *is this
    /// recoverable*.
    ///
    /// # ⚠⚠⚠⚠ Why this is a document gate and not a driver one
    ///
    /// The driver's gate proves it RAISES `peer.silent`. What the raise is WORTH is a decision the
    /// `.scxml` takes and nothing else can: `peer.gone` reaches a FINAL state, so the run is over
    /// and its pane is beyond help, while this must not — the thing that stopped speaking may be
    /// the peer's REPORTER, which a rebuild replaces under a running daemon, and no reading of a
    /// pane can separate the two. **Both events are raised from `working` by the same driver in the
    /// same pass, so if the document sent them to the same place the driver could not tell.**
    ///
    /// ⚠⚠⚠ **AND THE RECOVERY IS THE OTHER HALF, WHICH IS WHAT MAKES BEING WRONG CHEAP.** A model
    /// that thinks for the whole silence bound without calling a tool moves no counter either — the
    /// residue this ceiling is named with — so a run called silent has to be able to walk on the
    /// moment its peer answers. That is `awaiting_human --turn.done--> judging`, driven here rather
    /// than asserted about, because it is the sentence the whole design rests on.
    #[test]
    fn a_peer_that_went_silent_is_recoverable_where_a_peer_that_is_gone_is_not() {
        /// Reach `working` the way a run does, raise `left` with `data`, and say where it went
        /// **and whether the engine calls that an ending** — the second half read off the same
        /// machine, in the same breath, because a reading taken later is a second authority on
        /// one fact.
        ///
        /// ⚠⚠⚠⚠ **`data` IS AN ARGUMENT SINCE REGISTER ITEM 715**, and it was `""` before. That
        /// item gave `peer.silent` a guard, so this fixture was handing a data-carrying event on
        /// with nothing and the datamodel was asked to index nil — W3C SCXML 3.8 abandons the rest
        /// of the block, which is a state half-entered in the voice of one that worked. **This
        /// gate stayed GREEN through it**; what found it was `sprag-gate`'s own ratchet
        /// (`no_data_carrying_event_is_handed_on_without_its_data`), announcing the new subject.
        fn from_working(left: AiLoopEvent, data: &str) -> (AiLoopState, bool) {
            let (mut engine, host, _lua, _session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Working,
                "the fixture: both events below are raised from `working` and nowhere else",
            );
            carried(&mut engine, &host, left, data);
            (engine.get_current_state(), engine.is_in_final_state())
        }

        // ⚠ `service: false` IS WHAT THE DRIVER PUBLISHES for a peer that printed nothing, which is
        // exactly the peer this gate is about — see `OuterLoop::pumping`'s `PeerSilent` arm.
        let quiet = serde_json::json!({"service": false}).to_string();
        let silent = from_working(AiLoopEvent::PeerSilent, &quiet);
        // ⚠ `peer.gone` reads no `_event.data` at all, so it is handed on bare exactly as before.
        let gone = from_working(AiLoopEvent::PeerGone, "");

        assert_eq!(
            silent.0,
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠⚠ NOTHING IS SPEAKING FOR THE PANE AND THE REMEDY IS A PERSON. That state already \
             notifies one and already ends the run if none comes, on the caller's own \
             `await_person_ms` — so the edge buys the whole answer. A document with no edge at all \
             leaves the machine in `working`, which is the product as measured: fourteen minutes \
             of nothing, and a person ending it by hand",
        );

        // ⚠⚠⚠⚠ THE CONTROL, AND IT IS THE POINT: the same state, the same driver, the other word.
        assert_eq!(
            gone.0,
            AiLoopState::PeerGone,
            "⚠⚠⚠ THE CONTROL FAILED — a gone process cannot come back, so its run ENDS, and if \
             both words landed in the same place the driver would have no way to say which it \
             found. Two facts, two destinations",
        );
        // ⚠⚠⚠⚠⚠ AND THE DIFFERENCE IS ASKED OF THE ENGINE, which is the only thing that knows —
        // register item 470, stage 3. This used to read a Rust list of finals (`AiLoop::is_final`),
        // i.e. it checked one copy of the document against another copy of the document; now it
        // drives the real machine and asks it, so a `<final>` moved INSIDE the `<parallel>` fails
        // here rather than being answered by a list that never noticed.
        assert!(
            !silent.1 && gone.1,
            "⚠⚠⚠⚠ and the difference has to be the one that matters: silence PAUSES a run and a \
             dead peer ENDS one. A `peer.silent` that reached a final state would turn a reporter \
             somebody rebuilt into a lost run. Measured: silence -> {:?} (final: {}), gone -> \
             {:?} (final: {})",
            silent.0,
            silent.1,
            gone.0,
            gone.1,
        );

        // ── THE RECOVERY: the peer speaks again, and the run has lost nothing ──
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        carried(&mut engine, &host, AiLoopEvent::PeerSilent, &quiet);
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Judging,
            "⚠⚠⚠⚠⚠ A TURN THAT COMES BACK AFTER THE SILENCE IS JUDGED LIKE ANY OTHER. This is what \
             makes the bound safe to author generously and safe to be WRONG about: a model that \
             thought for the whole bound without calling a tool costs the run one notification and \
             nothing else. Without this edge the ceiling would trade a 24-hour hang for a run that \
             a slow turn kills",
        );
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
            let (mut engine, host, _lua, _session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "the control: one completed turn is judged",
            );
            carried(&mut engine, &host, AiLoopEvent::Judge, data);
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

    /// ⛔⛔⛔⛔ **A STOOD-DOWN RUN WHOSE PEER LEAVES AFTER A BANKED TURN MUST CLOSE, NOT FAIL** —
    /// register item 604, asked of the DOCUMENT by hand.
    ///
    /// # ⚠⚠⚠⚠⚠ Why by hand, when a driver gate for this already exists
    ///
    /// [`a_banked_turn_survives_an_agent_that_leaves_under_a_standing_order`] drives the same claim
    /// through `OuterLoop::pump`, and **every one of item 605's five guard rewrites was measured
    /// that way** — five attempts, one altitude. Upstream then drove THIS document at this crate's
    /// own pinned SCE revision, by hand, and the guarded edge FIRED (SCE reply, 2026-08-23). Two
    /// runs of one document disagree, and nothing here had ever asked the document alone.
    ///
    /// ⚠⚠⚠ **THE CONTROL IS THE NEIGHBOURING GUARD, NOT A SECOND COPY OF THIS ONE.** `judging`
    /// already carries a `judge` transition guarded on `_event.data.done` AND `In('standing_down')`,
    /// and that one works. Firing it from the SAME state under the SAME order is what proves
    /// `In('standing_down')` reads TRUE here — so a red below is about `peer.gone`'s edge and
    /// cannot be about an order that never arrived. Without the control those two have opposite
    /// fixes and identical symptoms, which is the shape that cost item 605 four rounds.
    #[test]
    fn a_stood_down_run_whose_peer_leaves_after_a_banked_turn_converges_rather_than_fails() {
        /// A fresh machine in `judging` — a turn banked — with a stand-down order standing.
        fn banked_and_stood_down() -> (Engine<AiLoopPolicy>, crate::act::Serving) {
            let (mut engine, host, _lua, _session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            carried(&mut engine, &host, AiLoopEvent::StandDown, "");
            let active = engine.get_active_states();
            assert!(
                active.contains(&AiLoopState::Judging)
                    && active.contains(&AiLoopState::StandingDown),
                "⚠⚠⚠ THE FIXTURE'S OWN PRECONDITION: the turn has to be BANKED (`judging` is only \
                 reachable by a completed turn) with the order STANDING, or nothing below is item \
                 604's situation at all. active = {active:?}",
            );
            (engine, host)
        }

        // ── THE CONTROL FIRST: same state, same order, the guard that is known to work ──
        let (mut control, control_host) = banked_and_stood_down();
        carried(
            &mut control,
            &control_host,
            AiLoopEvent::Judge,
            r#"{"done": true, "checked": false, "explained": false, "unheard": false, "stop_short": false}"#,
        );
        let control_active = control.get_active_states();
        assert!(
            control_active.contains(&AiLoopState::Closing),
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, so nothing below is readable: `In('standing_down')` is not \
             reading true at `judging` even on the edge that already works, and the subject would \
             be red for the ORDER rather than for the EVENT. active = {control_active:?}",
        );

        // ── THE SUBJECT: the agent finished its work and then left, which is what a finished
        // agent does. The turn is already in the bank; the run must be told so.
        //
        // ⚠⚠⚠ **`converged` AND NOT `closing`**, though `closing` is where the neighbouring
        // `judge` edge sends the same order. `closing` exists to ASK a question — its `onentry`
        // sends `prompt.end` — and at this moment the document already knows the party that would
        // answer is gone. Entering a state to do something known to be impossible is what the
        // `prompt.unasked --> converged` edge inside `closing` is already there to repair, one
        // pass later and at the cost of typing at a dead pane. ⚠ The peer leaving DURING the
        // closing question is a different moment and still ends `peer_gone`; that is its own item.
        let (mut subject, subject_host) = banked_and_stood_down();
        carried(&mut subject, &subject_host, AiLoopEvent::PeerGone, "");
        let active = subject.get_active_states();
        assert!(
            active.contains(&AiLoopState::Converged),
            "⛔⛔⛔⛔ ITEM 604, AT THE DOCUMENT: a BANKED turn whose agent then left must CONVERGE — \
             the person asked for a stop at the next milestone and got one. This sends the run \
             through `peer_gone` instead, which is what makes `stand_down_sentence` tell them the \
             work they were promised was kept was lost. active = {active:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **WHOSE DISCRETION IS A STAND-DOWN HONOURED AT** — the question item 604's driver
    /// half turns on, asked of the document rather than argued about in the driver.
    ///
    /// `sprag stand-down` promises *"it stops at its next milestone, and its work is kept"*, and
    /// `judging` honours it on ONE edge: `_event.data.done && In('standing_down')`. Both halves of
    /// that guard matter, and the second one is not the interesting half — the first is. It makes
    /// the order land **only when the AGENT declares a milestone**, so a run that has been told to
    /// stop keeps being handed turns until its agent volunteers one.
    ///
    /// ⚠⚠⚠ **THAT IS WHERE ITEM 604'S MEASURED WALK COMES FROM.** The live run in
    /// [`a_banked_turn_survives_an_agent_that_leaves_under_a_standing_order`] is stood down at
    /// `judging`, judged as an ordinary turn because its agent declared nothing, sent back to
    /// `working` for another turn, and only there discovers the agent has left. The guarded
    /// `peer.gone` edge at `judging` cannot help a run that is no longer in `judging`.
    ///
    /// ⚠⚠ **THIS GATE DOES NOT DECIDE WHICH READING IS RIGHT.** It pins which one the document
    /// implements today, so that a change in either direction has to move this line and say why.
    #[test]
    fn a_standing_order_lands_only_on_a_turn_whose_agent_declared_a_milestone() {
        /// Drive a fresh machine to `judging` with a stand-down order standing, then judge the
        /// turn with `data`, and say where the work region ended up.
        fn judged_under_orders(data: &str) -> Vec<AiLoopState> {
            let (mut engine, host, _lua, _session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            carried(&mut engine, &host, AiLoopEvent::StandDown, "");
            carried(&mut engine, &host, AiLoopEvent::Judge, data);
            engine.get_active_states()
        }

        // ── THE CONTROL: the agent DID declare one, and the order lands ──
        let declared = judged_under_orders(
            r#"{"done": true, "checked": false, "explained": false, "unheard": false, "stop_short": false}"#,
        );
        assert!(
            declared.contains(&AiLoopState::Closing),
            "⚠⚠⚠ THE CONTROL: an order standing over a turn whose agent said it reached the \
             milestone must wind the run up. A red here means the order is not landing at all and \
             the subject below is measuring nothing. active = {declared:?}",
        );

        // ── THE SUBJECT: the agent declared nothing, and the run is handed another turn ──
        let silent = judged_under_orders(ORDINARY);
        assert!(
            silent.contains(&AiLoopState::Working),
            "⚠⚠⚠⚠⚠ IF THIS IS RED THE DOCUMENT HAS CHANGED WHAT A STAND-DOWN MEANS — it now stops \
             a run at the end of the turn it was ordered during, rather than waiting for the agent \
             to volunteer a milestone. That is a defensible reading and it is NOT the one this \
             document was written with, so whatever moved it owes item 604's walk a re-measurement: \
             the driver's `working` raise is downstream of exactly this edge. active = {silent:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **EVERY STATE THAT OWES THE PEER A PROMPT ANSWERS FOR A QUESTION THAT NEVER WENT IN**
    /// — register item 446's remedy, made total over the states that can meet its condition.
    ///
    /// # ⚠⚠⚠⚠⚠ The defect: one state had the remedy and six can meet the condition
    ///
    /// 446 measured the fault on a live daemon — the text lands on the pane, the submit never
    /// becomes a question, the session's counter never moves, and a fresh session takes the
    /// identical prompt — and built the answer on `priming`, where the FIRST prompt is refused.
    /// But the driver raises `prompt.unasked` **at the state the delivery landed in**, and six
    /// states can be that: `priming`, `working` (the turn prompt, on four edges into it),
    /// `disputing`, `reflecting`, `closing` and `stopping`. Five had no edge for it at all.
    ///
    /// What that cost, exactly: the event is raised, no transition takes it, **the machine stays
    /// put**, and the driver goes back to watching a pane for a turn nobody was asked to take. A run
    /// a single session replacement would have saved spends its wall clock instead — and the
    /// condition is precisely the one that keeps happening to the same session, so *the next prompt
    /// is refused too* is the expected case rather than the unlucky one.
    ///
    /// # ⚠⚠⚠ Why the answers are not all the same, and why two states say so themselves
    ///
    /// The remedy is a session replacement, and a run that is already ENDING must not buy one:
    /// `closing` and `stopping` are asking for an account of work that is already done, so a fresh
    /// agent would be asked to summarise work it never did. They answer with the ending they were
    /// always going to reach, carrying no account. Everything else takes the region's rule.
    ///
    /// ⚠⚠ **DRIVEN AT THE DOCUMENT, EVENT BY EVENT.** What is being asserted is the machine's
    /// ROUTING, and a pane fixture would put a delivery, a supervisor and a peer's timing between
    /// the claim and the answer — three things that can fail on their own. The driver's own half
    /// (raising the event at all) is `pump`'s funnel and is gated where it lives.
    ///
    /// # ⛔⛔⛔⛔⛔ AND THE WORD `EVERY` WAS A CLAIM NOTHING MEASURED — register item 750
    ///
    /// The table below is a list somebody types, and it used to be headed by a literal `7` whose
    /// stated authority was `Owed::on` — a function register item 470's third stage had already
    /// deleted. Item 741 added the seventh entry by REMEMBERING to, and the same round forgot the
    /// `pass` arm for the same state (item 749, measured live as a run that ended `failed` with its
    /// work unbanked) and left the driver's own two prompt lists a prompt short besides. **Four
    /// hand-written lists enumerate this document's prompts and not one was held against the file.**
    ///
    /// So the population is DERIVED at the bottom of this function, on item 749's pattern: a state
    /// that owes the peer a prompt is a state one of this document's `prompt.say` acts leaves a run
    /// in — an `<onentry>` means that state, a transition body means where that transition lands,
    /// and **neither is RED rather than skipped**. The other three lists are held by
    /// `every_driven_state_says_what_a_pass_of_it_is_for` and by
    /// `outer::tests::every_prompt_this_document_sends_is_one_the_door_checks_and_a_caller_can_read`,
    /// whose doc carries the table naming all four.
    #[test]
    fn every_state_that_owes_a_prompt_answers_a_question_that_was_never_taken() {
        /// Walk a fresh machine through `events`, each carrying `data`, and say where it landed.
        fn walked(events: &[(AiLoopEvent, &str)]) -> AiLoopState {
            let (mut engine, host, _lua, _session) = started();
            for (event, data) in events {
                carried(&mut engine, &host, *event, data);
            }
            engine.get_current_state()
        }

        /// The walk into `judging`, which four of the six states are reached through.
        ///
        /// ⚠⚠ `turn.done` CARRIES ITS THREE NUMBERS, which this walk left empty until register item
        /// 505: `judging`'s `onentry` reads them off `_event.data`, so an empty payload asked the
        /// datamodel to index nil, raised `error.execution`, and had the entry block abandoned after
        /// its first assignment — silently, because nothing answered the error. This gate was one of
        /// the seven that went red the moment the document did.
        const TO_JUDGING: [(AiLoopEvent, &str); 3] = [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptSent, ""),
            (AiLoopEvent::TurnDone, TURN),
        ];

        /// `events`, then one more — spelled as a function because every case below is *reach the
        /// state, then refuse its prompt*, and a reader should see the two halves apart.
        fn then<'a>(
            events: &[(AiLoopEvent, &'a str)],
            last: (AiLoopEvent, &'a str),
        ) -> Vec<(AiLoopEvent, &'a str)> {
            let mut all = events.to_vec();
            all.push(last);
            all
        }

        /// One state that can owe a prompt: its name, the walk that reaches it, and the state it
        /// must answer a never-taken question with.
        type Refused<'a> = (&'a str, Vec<(AiLoopEvent, &'a str)>, AiLoopState);

        // ⛔⛔⛔⛔⛔ **NO NUMBER STANDS HERE ANY MORE, AND THAT IS REGISTER ITEM 750.**
        //
        // It used to read `[Refused; 7]`, and the comment above it said the seven were `Owed::on`'s
        // — a function item 470's third stage had already deleted, so the literal derived from
        // nothing and the only thing keeping it honest was somebody remembering. Item 741 added the
        // seventh by remembering; the same round forgot the `pass` arm for the same state (item
        // 749) and two more lists besides. **A list that grows by memory is a list that will be
        // green about a hole**, so this one is contrasted with the document at the bottom of this
        // function instead of counted here.
        let cases: Vec<Refused> = vec![
            // The one 446 built, and it now goes through the REGION's rule rather than its own
            // edge — so a green here is also the proof that a child with no answer inherits one.
            (
                "priming",
                vec![(AiLoopEvent::Start, "")],
                AiLoopState::Restarting,
            ),
            (
                "working",
                vec![(AiLoopEvent::Start, ""), (AiLoopEvent::PromptSent, "")],
                AiLoopState::Restarting,
            ),
            (
                "disputing",
                then(
                    &TO_JUDGING,
                    (
                        AiLoopEvent::Judge,
                        "{\"done\": true, \"checked\": \"failed\"}",
                    ),
                ),
                AiLoopState::Restarting,
            ),
            // ⛔ REGISTER ITEM 741, and it is `disputing`'s neighbour in every way that matters
            // here: entered from `judging` by one edge, composes a prompt on entry, hands back to
            // `working`. What differs is the fact that got it there.
            (
                "unverified",
                then(
                    &TO_JUDGING,
                    (
                        AiLoopEvent::Judge,
                        "{\"done\": true, \"checked\": \"silent\", \"silence\": \"unreadable\"}",
                    ),
                ),
                AiLoopState::Restarting,
            ),
            (
                "reflecting",
                then(&TO_JUDGING, (AiLoopEvent::Judge, "{\"done\": true}")),
                AiLoopState::Restarting,
            ),
            (
                "closing",
                then(
                    &then(&TO_JUDGING, (AiLoopEvent::Judge, "{\"done\": true}")),
                    (
                        AiLoopEvent::ReflectDone,
                        "{\"done_reason\": \"north_star\"}",
                    ),
                ),
                // ⚠⚠⚠ NOT `restarting`: the run GOT THERE, and the report is a courtesy on top of a
                // verdict already reached. A replacement here would open a session that never did
                // the work and ask it to summarise it.
                AiLoopState::Converged,
            ),
            (
                "stopping",
                then(
                    &TO_JUDGING,
                    (AiLoopEvent::Judge, "{\"stop_short\": \"turns\"}"),
                ),
                // ⚠⚠⚠ NOT `restarting`, for `closing`'s reason and the state's own: every way this
                // turn can end arrives at `exhausted`, because the ending was decided by the guard
                // that got here and no last question can un-end it.
                AiLoopState::Exhausted,
            ),
        ];

        // ══ ⛔⛔⛔⛔⛔ AND THE HALF THAT WAS MISSING: IS THE TABLE ABOVE COMPLETE? ═══════════════
        //
        // Register item 750, and it is item 749's contrast one list over. Everything below this
        // line used to be true only of the states somebody REMEMBERED to write down, which is
        // precisely the property that was true while this table walked six of seven.
        //
        // ⚠⚠⚠⚠⚠ **THE POPULATION IS DERIVED, NOT WRITTEN.** A state that owes the peer a prompt is
        // a state one of this document's `prompt.say` acts LEAVES A RUN IN — the document's own
        // rule, written at `priming`: *the driver raises `prompt.unasked` at the state the delivery
        // landed in*. `outer::prompts_the_document_says` reads the acts and answers where each
        // lands, and a prompt sent from neither an `<onentry>` nor a transition body is RED rather
        // than skipped.
        let document = crate::outer::DOCUMENT;
        let declared: std::collections::BTreeSet<&str> =
            declared_state_ids(document).into_iter().collect();
        let mut owes: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for act in crate::outer::prompts_the_document_says(document) {
            let Some(state) = act.lands_in.state() else {
                panic!(
                    "⛔⛔⛔ REGISTER ITEM 750: this document sends a prompt from somewhere that is \
                     neither a state's `<onentry>` nor a transition's body, so nothing can say \
                     which state owes the answer when the peer will not take it: {act:?}",
                );
            };
            // ⚠⚠⚠⚠⚠ THE PREMISE, ASSERTED INSIDE THE GATE RATHER THAN ASSUMED BY IT: the scan
            // must be reading this file's own ids. A reader answering names the document does not
            // declare would make the contrast below compare two sets of junk, and a gate blind to
            // its own eye is the shape this register keeps meeting.
            assert!(
                declared.contains(state),
                "⚠⚠⚠⚠⚠ THE SCAN IS BLIND: a prompt act lands in `{state}`, and the read of \
                 `ai_loop.scxml` found no such `id`. Read back {} ids",
                declared.len(),
            );
            owes.insert(state);
        }
        // ⭐⭐⭐ **CONTRAST — THE CASE EXISTS.** The set of states a prompt can be refused at,
        // against the set this table walks. `unverified` was in the first and not the second for
        // exactly as long as it was missing from the `pass` table, and for the same reason.
        let listed: std::collections::BTreeSet<&str> =
            cases.iter().map(|(state, _, _)| *state).collect();
        assert_eq!(
            owes, listed,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 750: this table and the states that owe the peer a prompt have \
             come apart. A state in the LEFT set and not the right is one whose answer to a \
             question that never went in is asserted by NOBODY — it may stay put, which is the \
             defect item 446 measured: the event is raised, no transition takes it, and the driver \
             goes back to watching a pane for a turn nobody was asked to take. A name in the RIGHT \
             set and not the left is a case about a state this document never prompts from",
        );

        for (state, walk, answer) in cases {
            // ⚠⚠⚠⚠ THE CONTROL, PER CASE, AND IT IS NOT A FORMALITY: a walk that did not reach the
            // state would assert the region's rule from wherever it stopped, and every one of these
            // cases would pass from `idle`. This is the arrangement `probe_*.scxml` calls staging
            // the hazard before measuring it.
            let reached = walked(&walk);
            assert_eq!(
                format!("{reached:?}").to_lowercase(),
                state,
                "the walk must reach `{state}` or the claim below is about somewhere else: {walk:?}",
            );
            assert_eq!(
                // ⚠ CARRYING ITS WORD, for `TO_JUDGING`'s reason one screen up: the first
                // `prompt.unasked` edge reads `_event.data.retyped`, so an empty payload makes the
                // guard index nil and every case here ends `failed` on an error nobody meant. This
                // is the ordinary refusal — a text this run has not had refused before.
                walked(&then(&walk, (AiLoopEvent::PromptUnasked, UNASKED))),
                answer,
                "⚠⚠⚠⚠⚠ `{state}` OWES THE PEER A PROMPT, SO IT MUST ANSWER FOR ONE THAT NEVER WENT \
                 IN. Staying put is what five of these six did: the event is raised at the state the \
                 delivery landed in, nothing takes it, and the driver goes back to watching a pane \
                 for a turn nobody was asked to take",
            );
        }

        // ── ONE FREE REPLACEMENT, AND A QUESTION THAT LANDED BUYS ANOTHER ──
        //
        // ⚠⚠⚠ THE SECOND REFUSAL IN A ROW IS A PERSON'S. A peer that will not take the question in
        // the session opened to fix exactly that is not something a further restart reaches.
        // ⚠⚠ AND BOTH REFUSALS SAY THE TEXT IS NEW, which is what keeps this arm about the
        // COUNTER. A second refusal carrying `retyped` would end the run through the edge above
        // it and this assertion would be green about somewhere else entirely.
        let twice = [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptUnasked, UNASKED),
            (AiLoopEvent::SessionReplaced, ""),
            (AiLoopEvent::SessionReady, ""),
            (AiLoopEvent::PromptUnasked, UNASKED),
        ];
        assert_eq!(
            walked(&twice),
            AiLoopState::Failed,
            "⚠⚠⚠⚠ TWICE RUNNING IS NOT A RECOVERY, IT IS A SPIN: {twice:?}",
        );

        // ⚠⚠⚠⚠⚠ AND THE CONTROL THAT MAKES THE BOUND A BOUND RATHER THAN A ONE-SHOT — the identical
        // walk with ONE question taken in the middle. `unasked_since_taken` is cleared by a prompt
        // that landed, so the budget is one replacement per question that went in and not one per
        // run. Without this arm a document that never cleared the counter would be green above and
        // would end the second real outage of every long run.
        let cleared = [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptUnasked, UNASKED),
            (AiLoopEvent::SessionReplaced, ""),
            (AiLoopEvent::SessionReady, ""),
            (AiLoopEvent::PromptSent, ""),
            // ⚠ CARRYING ITS NUMBERS, for `TO_JUDGING`'s reason: an empty `turn.done` makes
            // `judging`'s entry index nil and this walk ends `failed` on an error nobody meant.
            (AiLoopEvent::TurnDone, TURN),
            (AiLoopEvent::Judge, "{\"done\": false}"),
            (AiLoopEvent::PromptUnasked, UNASKED),
        ];
        assert_eq!(
            walked(&cleared),
            AiLoopState::Restarting,
            "⚠⚠⚠⚠ A QUESTION THAT LANDED MUST CLEAR THE BUDGET: {cleared:?}",
        );

        // ── ⛔⛔⛔⛔⛔ AND THE SENTENCE A PERSON READS MUST DESCRIBE *THIS* GUARD ──
        //
        // Register item 742, and it belongs in THIS test rather than in one of its own — that is
        // the whole repair. The driver's prose said *"if it happens again in the session opened for
        // it, the run stops"*, which is a DIFFERENT condition from the two walks above: the counter
        // is cleared by a question that lands, so the `cleared` walk is a second refusal in the
        // replacement session that correctly does NOT stop. A watcher read the sentence, watched
        // that run, and filed the product as broken; two sessions spent a round finding out the
        // only wrong thing was the line.
        //
        // ⚠⚠⚠ A GATE THAT ONLY READ THE SENTENCE WOULD BE VACUOUS — it would stay green while
        // somebody changed the guard out from under it. Sitting here, a mutation to either edge
        // reddens the walks above and this clause travels with them: **the sentence is asserted
        // against walks that were just run, not against a remembered claim.**
        let said = crate::outer::RestartReason::Unasked.describe();
        assert!(
            said.contains("TWICE RUNNING"),
            "⚠⚠⚠⚠⚠ THE SENTENCE MUST NAME THE CONDITION THE WALKS ABOVE JUST DEMONSTRATED — a \
             refusal with none taken in between. This is the only explanation a person ever reads \
             for this ending, and prose that names a different condition sends an honest reader to \
             report the product as broken: {said:?}",
        );
        assert!(
            !said.contains("in the session opened for it"),
            "⚠⚠⚠⚠ AND IT MUST NOT NAME THE CONDITION THE DOCUMENT REFUSED. `unasked_since_taken` \
             is cleared by `prompt.sent`, so *again in the session opened for it* is false of the \
             `cleared` walk two assertions up — which passes, and must: {said:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE BOUND ON A REFUSED PROMPT COUNTS REFUSALS IN A ROW, AND THE CHURN REGISTER
    /// ITEM 719 MEASURED IS NOT A ROW** — the corrected diagnosis, pinned.
    ///
    /// # ⚠⚠⚠⚠⚠ What the item believed, and what the document actually does
    ///
    /// Item 719 filed *"a recovery that re-types the input that caused the failure — churn with a
    /// period of ONE TURN"* and read it as a cycle nothing bounds. **Asked, the document answers
    /// otherwise**: `prompt.unasked` has a guard (`unasked_since_taken`), one free replacement, and
    /// a second refusal goes to `failed`. The gate above holds both halves.
    ///
    /// **So the bound exists. It simply does not count what 719 is about.** It is cleared by
    /// `priming`'s `prompt.sent` — the document says so in its own words, *"a question that landed
    /// is what makes the next refusal a NEW fact rather than the same one continuing"* — and the
    /// prompt that lands in item 719's run is the BRIEF, retyped in full into every replacement.
    /// The turn prompt is the one refused. So every cycle clears the counter before spending it:
    ///
    /// ```text
    /// priming's brief LANDS  ->  counter := 0
    /// the turn prompt is refused  ->  replacement, counter := 1
    /// the replacement's brief LANDS  ->  counter := 0        <- the bound is spent and returned
    /// the turn prompt is refused  ->  replacement, counter := 1
    /// ...
    /// ```
    ///
    /// **One replacement per turn, for as long as the run lasts.** Item 719 measured two in about
    /// nine minutes, at 9,025 bytes retyped each time, and could not say why the document's own
    /// guard had not stopped it. This is why.
    ///
    /// # ⚠⚠⚠ Why that is a finding and not a defect in the guard
    ///
    /// The guard answers a different question — *will this peer take a question at all?* — and its
    /// answer is right: a session that takes one is not the session a further restart cannot reach.
    /// What has no counter at all is **the same TEXT being re-delivered after each replacement**,
    /// which is 719's own done-when line. Naming which of the two is missing is what this pins, and
    /// it is what the repair has to be built on: a bound on refusals in a row cannot express it,
    /// however it is tuned.
    ///
    /// ⚠⚠ **DRIVEN AT THE DOCUMENT, EVENT BY EVENT** — its neighbour's argument exactly. What is
    /// asserted is the machine's own arithmetic, and a pane fixture would put a delivery, a
    /// supervisor and a peer's timing between the claim and the answer.
    ///
    /// # ⚠⚠⚠⚠⚠ AND THIS IS THAT DAY — what changed, said by the gate that predicted it
    ///
    /// This doc used to end *"it asserts the PRESENT behaviour, which is the churn … the day the
    /// loop stops repeating itself, this gate is what says exactly which sentence changed."* The
    /// answer is: **not one sentence of the arithmetic above.** The counter still spends its budget
    /// and still has it handed back by every brief that lands, and the loop below still asserts
    /// exactly that, because it is still the right answer to the question that counter asks.
    ///
    /// What ended the churn is a SIBLING EDGE keyed on the TEXT — `prompt.unasked` carrying
    /// `_event.data.retyped`, read from the driver's own memory of what it last delivered and
    /// failed to get asked (`OuterLoop::retyping`). The final arm of this gate drives the identical
    /// cycle with that word set and finds the run ENDED, which is the announcement this doc
    /// promised: the repair is a second question, not a tuning of the first.
    ///
    /// ⚠ So the loop below is no longer a pin on an open defect. It is the CONTROL for the arm
    /// under it — the proof that what stops the cycle is the text and not the count.
    #[test]
    fn the_bound_on_a_refused_prompt_is_spent_and_returned_by_every_brief_that_lands() {
        /// How many replacement cycles to drive. Three rather than two: the guard's own budget is
        /// ONE, so a second replacement already exceeds it and a third says the excess is not a
        /// fencepost.
        const CYCLES: usize = 3;

        // ⚠⚠⚠⚠⚠ **THE WIRE, PINNED FIRST, BECAUSE NOTHING ELSE JOINS ITS TWO HALVES.** Every gate
        // in this file drives the document BY HAND and every gate in `outer.rs` asks the driver
        // directly, so a key renamed on one side leaves both green while every real refusal
        // evaluates a nil — `error.execution`, answered by `work`'s own edge, and the run ends
        // `failed` on a fault nobody meant. That is not hypothetical: it is what these gates did
        // the moment the guard was added and their payloads were still empty.
        assert_eq!(
            (UNASKED, UNASKED_AGAIN),
            (
                crate::outer::Retyped::First.wire().as_str(),
                crate::outer::Retyped::Again(9_025).wire().as_str(),
            ),
            "⚠⚠⚠⚠⚠ THE PAYLOADS BELOW MUST BE THE ONES `OuterLoop::pump` SENDS, or every \
             assertion in this file is about a wire the product does not have",
        );

        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");

        let mut walked = Vec::new();
        for cycle in 0..CYCLES {
            // ⚠ THE BRIEF LANDS. This is the whole mechanism: `priming`'s `prompt.sent` clears
            // `unasked_since_taken`, and in item 719's run the brief is exactly what the peer takes
            // — 9,025 bytes of it, retyped into every session this loop opens.
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            carried(&mut engine, &host, AiLoopEvent::Judge, "{\"done\": false}");
            // ⚠ AND THE TURN PROMPT DOES NOT. Same peer, same session, one prompt later.
            // ⚠⚠ SAYING THE TEXT IS NEW EVERY TIME, which is what keeps this loop about the
            // COUNTER: the arm at the foot of this gate is where the same cycle says otherwise.
            carried(&mut engine, &host, AiLoopEvent::PromptUnasked, UNASKED);
            walked.push(format!("cycle {cycle}: {:?}", engine.get_current_state()));
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Restarting,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 719, PINNED: the run must still be BUYING A REPLACEMENT on \
                 cycle {cycle}, because the brief it just landed returned the one free replacement \
                 the guard had already spent. A `failed` here would mean the bound had begun to \
                 count what this item is about — which would be the repair, and this gate is where \
                 it would be announced. Walked {walked:?}",
            );
            carried(&mut engine, &host, AiLoopEvent::SessionReplaced, "");
            carried(&mut engine, &host, AiLoopEvent::SessionReady, "");
        }

        // ⚠⚠⚠⚠⚠ THE CONTROL, AND IT IS WHAT MAKES THE LOOP ABOVE A FINDING RATHER THAN A CLAIM
        // THAT NOTHING IS BOUNDED. The identical walk with the brief NOT landing between the two
        // refusals ends the run — one refusal, one replacement, and the second is a person's. So
        // the guard is real, it fires, and every cycle above is it being handed back its budget.
        let (mut spun, host, _lua, _session) = started();
        for (event, data) in [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptUnasked, UNASKED),
            (AiLoopEvent::SessionReplaced, ""),
            (AiLoopEvent::SessionReady, ""),
            (AiLoopEvent::PromptUnasked, UNASKED),
        ] {
            carried(&mut spun, &host, event, data);
        }
        assert_eq!(
            spun.get_current_state(),
            AiLoopState::Failed,
            "⚠⚠⚠⚠ THE CONTROL: two refusals with nothing taken between them must END the run. \
             Without this the loop above would be reporting that `prompt.unasked` is unbounded, \
             which is false and is what item 719 assumed",
        );

        // ── AND THE REPAIR: THE SAME CYCLE, WITH THE DRIVER SAYING IT IS THE SAME TEXT ──
        //
        // ⛔⛔⛔⛔⛔ REGISTER ITEM 719's DONE-WHEN. Everything above is unchanged and every step of
        // this walk is the loop's above, one key different: the refusal says these bytes have
        // already cost this run a session. The FIRST cycle must now end the run, where the loop
        // above buys a replacement on all three.
        //
        // ⚠⚠⚠ IT IS THE SAME WALK ON PURPOSE. A shorter one — refuse, refuse — would be green
        // against the counter alone and would prove nothing about the churn, because two refusals
        // in a row already end the run (the control just above). What makes this a measurement is
        // that the brief LANDS in between, handing the counter back its budget exactly as it does
        // in the cycle this item measured on a live daemon.
        let (mut same, host, _lua, _session) = started();
        for (event, data) in [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptSent, ""),
            (AiLoopEvent::TurnDone, TURN),
            (AiLoopEvent::Judge, "{\"done\": false}"),
            (AiLoopEvent::PromptUnasked, UNASKED_AGAIN),
        ] {
            carried(&mut same, &host, event, data);
        }
        assert_eq!(
            same.get_current_state(),
            AiLoopState::Failed,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 719: the run must END on text it has already bought a session \
             for. A `Restarting` here is the churn itself — the replacement re-types the same \
             bytes into the same composer, meets the same refusal, and buys another replacement, \
             with a period of ONE TURN and no ending anywhere. Measured on the owner's daemon: two \
             replacements in about nine minutes at 9,025 bytes each",
        );

        // ⚠⚠⚠⚠⚠ AND THE CONTROL THAT SAYS THE WORD IS WHAT DID IT, not the walk. The identical
        // five steps with `retyped: false` — the loop at the top of this gate, on its first cycle —
        // still buy a replacement. Without this arm the assertion above would be green against a
        // document that had simply stopped restarting at all.
        let (mut fresh, host, _lua, _session) = started();
        for (event, data) in [
            (AiLoopEvent::Start, ""),
            (AiLoopEvent::PromptSent, ""),
            (AiLoopEvent::TurnDone, TURN),
            (AiLoopEvent::Judge, "{\"done\": false}"),
            (AiLoopEvent::PromptUnasked, UNASKED),
        ] {
            carried(&mut fresh, &host, event, data);
        }
        assert_eq!(
            fresh.get_current_state(),
            AiLoopState::Restarting,
            "⚠⚠⚠⚠ THE CONTROL: a refusal of text this run has NOT delivered before must still buy \
             the replacement that was measured to work — a fresh session took the identical prompt \
             where the wedged one would not. A run that failed here would have lost the recovery \
             register item 446 built, which is a real one for every cause but this",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A SESSION WITH ROOM LEFT IS STILL REPLACED ONCE REPLACING IT HAS PAID FOR
    /// ITSELF** — register item 424(a), the ECONOMIC half of the owner's question *"왜 고정이야?
    /// 컨텍스트 넘길 타이밍이 계산되어야 하는 거 아니야?"*
    ///
    /// # ⚠⚠⚠⚠ The measurement that made this writable, because the document refused it twice
    ///
    /// `context`'s entry refused an economic guard on a stated ground — the workload sits INSIDE
    /// the 15,000-21,000 band where a restart begins to pay, and *"a threshold placed anywhere in a
    /// band the workload sits inside decides by rounding"* — and named its own release: *"a guard
    /// remains writable the day a workload sits clearly outside the band, and what would justify
    /// one is a RE-MEASUREMENT, not a preference."*
    ///
    /// **Re-measured over 214 local agent sessions of 20 billed requests or more**, read exactly as
    /// [`crate::spend::spend_in`] reads one: the discardable part grows **72,212 tokens per TURN at
    /// the median** (120,928 at p75) against the **2,342** the settled conclusion was computed from.
    /// Five turns discard 361,061 at the median, and **211 of the 214 sessions are ABOVE the band
    /// after five turns while NOT ONE is inside it** — at five turns or at eight. The band argument
    /// was true about the workload it was measured on and this is a different one.
    ///
    /// # ⚠⚠⚠ What is asserted, and why each case is here
    ///
    /// Six runs differing in the numbers `reviewing` reads and in nothing else. The threshold is
    /// not authored: it is the break-even between a cache WRITE and a cache READ (twenty to one),
    /// with both sides measured by the run about itself.
    ///
    /// ⚠⚠ **THE ZERO CASES ARE THE ONES THAT MATTER MOST.** `cold` and `floor` degrade to 0 with
    /// `context` and for the same cause, and an unreadable number that could BUY a replacement
    /// would make the loop hand over exactly when it can see least — item 431's disease, arriving
    /// through a new door.
    #[test]
    fn a_session_worth_replacing_is_replaced_before_its_ceiling() {
        use sce_rust_runtime::ScriptValue;

        /// What one `reviewing` decision was given, in the document's own four names.
        struct Read {
            ceiling: i64,
            context: i64,
            floor: i64,
            cold: i64,
        }

        /// Walk a fresh machine to `reviewing`, hand it `read` as the session's own numbers, raise
        /// `event`, and say where it landed.
        ///
        /// ⚠⚠ REACHED BY THE DOOR THE PRODUCT USES — a judged milestone, then an applied
        /// reflection — rather than by dropping the machine into the state, because a fixture that
        /// bypasses the way in keeps passing after the way in is nailed shut (item 428's lesson).
        fn reviewed(read: &Read, event: AiLoopEvent) -> AiLoopState {
            let (mut engine, host, lua, session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            carried(&mut engine, &host, AiLoopEvent::Judge, "{\"done\": true}");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Reflecting,
                "the control: a claimed milestone asks what the next one is",
            );
            reflected(&mut engine, &host, AiLoopEvent::ReflectApplied, "");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Reviewing,
                "the control: an applied reflection reaches the state that decides where the next \
                 milestone is taken",
            );

            // ⚠⚠⚠ WRITTEN HERE RATHER THAN CARRIED ON `judge`, and that is what makes this gate
            // about the DECISION rather than about the driver's reader: `judging`'s onentry copies
            // these three out of the event, so a run driven by hand would be asserting whatever
            // `_event.data` happened to hold. These are the values the document holds when it
            // decides.
            for (name, value) in [
                ("context_ceiling", read.ceiling),
                ("context", read.context),
                ("floor", read.floor),
                ("cold", read.cold),
            ] {
                lua.set_variable(&session, name, ScriptValue::Int(value))
                    .expect("the document's own numbers are writable");
            }

            carried(&mut engine, &host, event, "{\"carried\": \"\"}");
            engine.get_current_state()
        }

        // The median session this was measured on: a floor of ~52,000 and a cold start of ~31,000,
        // so the break-even sits at `context` of about 671,000 — under a ceiling of 800,000.
        const FLOOR: i64 = 52_000;
        const COLD: i64 = 31_000;
        const CEILING: i64 = 800_000;
        /// `context - floor` is 348,000 against a toll of 620,000: nothing like worth it yet.
        const SMALL: i64 = 400_000;
        /// `context - floor` is 648,000 against the same toll: past the break-even, ceiling to
        /// spare.
        const GROWN: i64 = 700_000;

        for event in [AiLoopEvent::ReviewDone, AiLoopEvent::ReviewNone] {
            // ── THE CLAIM ── room left, and replacing has already paid for itself.
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: CEILING,
                        context: GROWN,
                        floor: FLOOR,
                        cold: COLD,
                    },
                    event,
                ),
                AiLoopState::Restarting,
                "⚠⚠⚠⚠⚠ ITEM 424(a) ({event:?}): this session can discard {} tokens where its \
                 replacement re-writes {COLD}, and a cache write costs twenty times a cache read — \
                 so the replacement has already paid for itself and every later request in this \
                 session is charged the whole {GROWN} again. A loop that hands over only at its \
                 CEILING keeps paying that for another {} tokens of growth",
                GROWN - FLOOR,
                CEILING - GROWN,
            );

            // ── THE CONTROL THE CLAIM STANDS ON ── the same run, one number smaller, must KEEP the
            // session. Without this the assertion above is satisfied by a guard that replaces
            // always, which is the behaviour item 424(b) was paid to end.
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: CEILING,
                        context: SMALL,
                        floor: FLOOR,
                        cold: COLD,
                    },
                    event,
                ),
                // ⚠⚠⚠ `Priming` IS THE KEPT SESSION — register item 523. `restarting` is what
                // REPLACES one (it counts `restarts` on the way in); this state composes the
                // prompts and sends one, which is what a run that just adopted a new milestone
                // owes its agent. The claim this control makes is *the session survives*, and it
                // still reads it: `Restarting` here would be a replacement nobody bought.
                AiLoopState::Priming,
                "⚠⚠⚠⚠ THE CONTROL ({event:?}): {} of discardable context against a toll of {} is \
                 not worth a replacement, and the next milestone belongs in the session that \
                 already holds the work. A gate whose two cases both replace is measuring nothing",
                SMALL - FLOOR,
                20 * COLD,
            );

            // ── AND CAPACITY STILL OUTRANKS IT ── past the ceiling, the fall-back replaces
            // whatever the economics say. The two axes point opposite ways and only one of them
            // can lose the session.
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: SMALL,
                        context: GROWN,
                        floor: FLOOR,
                        cold: COLD,
                    },
                    event,
                ),
                AiLoopState::Restarting,
                "⚠⚠ ({event:?}) a session past its ceiling is replaced, and this economic edge \
                 must not be able to keep one that capacity has already condemned",
            );

            // ── ⚠⚠⚠⚠ AND THE TWO ZEROES, WHICH ARE THE SHARP END ──
            //
            // An unreadable `cold` or `floor` reads 0, and 0 means *do not decide on this*. If
            // either could take the economic edge, the loop would hand over precisely when it can
            // see least: with `cold` at 0 the toll is 0 and everything clears it, and with `floor`
            // at 0 the whole reading looks discardable.
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: CEILING,
                        context: GROWN,
                        floor: FLOOR,
                        cold: 0,
                    },
                    event,
                ),
                // ⚠ THE KEPT-SESSION DOOR, which is `priming` since item 523 — see the control above.
                AiLoopState::Priming,
                "⚠⚠⚠⚠ ({event:?}) AN UNREADABLE `cold` IS NOT A FREE RESTART. Twenty times nothing \
                 is nothing, so a guard that did not refuse the zero would replace every session \
                 whose record it could not read — item 431's disease arriving through a new door",
            );
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: CEILING,
                        context: GROWN,
                        floor: 0,
                        cold: COLD,
                    },
                    event,
                ),
                // ⚠ THE KEPT-SESSION DOOR, which is `priming` since item 523 — see the control above.
                AiLoopState::Priming,
                "⚠⚠⚠ ({event:?}) AND AN UNREADABLE `floor` IS NOT A SESSION MADE ENTIRELY OF \
                 DISCARDABLE CONTEXT. {GROWN} would clear the toll of {} on its own, and the part \
                 no restart escapes would have been counted as the part it drops",
                20 * COLD,
            );

            // ── AND AN UNAUTHORED CEILING CHANGES NOTHING, which is the shipped document ──
            assert_eq!(
                reviewed(
                    &Read {
                        ceiling: 0,
                        context: GROWN,
                        floor: FLOOR,
                        cold: COLD,
                    },
                    event,
                ),
                AiLoopState::Restarting,
                "⚠⚠ ({event:?}) with no ceiling authored there is no *room* to have, so every \
                 reflection replaces exactly as it did before either guard existed",
            );
        }
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
        let (mut engine, host, _lua, _session) = started();
        let before = engine.get_active_states();
        assert!(
            before.contains(&AiLoopState::Standing),
            "the control: a fresh run is resting under no orders. active = {before:?}",
        );

        carried(&mut engine, &host, AiLoopEvent::StandDown, "");

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

    /// ⚠⚠⚠⚠⚠ **A RUN SOMEBODY IS HOLDING IS NOT A RUN NOBODY CAME TO** — register item 470's first
    /// stage, driven at the document rather than argued about in the driver.
    ///
    /// # What moved, and why it had to
    ///
    /// The rule is older than this gate: `OuterLoop::attend` reads the person's hold flag and
    /// returns before it can raise `Unattended`. That works, and it is in the wrong place. A
    /// decision that lives in the driver is one the DOCUMENT cannot be asked about — so a second
    /// driver, or a caller reading the machine to find out what a run will do, gets no answer, and
    /// the run's own walk never records that the ending was refused or why.
    ///
    /// ⚠⚠⚠ **AND THE FLAG CANNOT CLOSE THE WINDOW THE GUARD CLOSES.** The driver reads the flag at
    /// one instant and the machine processes events at another. A hold that arrives between those
    /// two moments met a run already ending as unattended. Here the order and the ending are two
    /// events in one queue, resolved in the order they really arrived — which is a thing a flag
    /// read outside the machine cannot express at all.
    ///
    /// # ⚠⚠ Both halves, because a guard that only ever refuses is a broken edge
    ///
    /// The refusal is the interesting direction and the permission is the one that catches a
    /// mistake in it: a `cond` that evaluated false always — a negation this engine did not
    /// support, an id that never matched — would look exactly like a working guard from the
    /// refusing side alone, and would leave `unattended` unable to end ANY run. That is a worse
    /// defect than the one being fixed, and it is silent, so the lift is driven here too.
    #[test]
    fn a_held_run_refuses_the_unattended_ending_and_takes_it_once_the_hold_is_lifted() {
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");

        let working = engine.get_active_states();
        assert!(
            working.contains(&AiLoopState::Working) && working.contains(&AiLoopState::Standing),
            "the control: a run that nobody has spoken to is working under no orders. \
             active = {working:?}",
        );

        // ── THE PERSON HOLDS IT: the work parks AND the order is recorded beside it ──
        carried(&mut engine, &host, AiLoopEvent::Hold, "");
        let held = engine.get_active_states();
        assert!(
            held.contains(&AiLoopState::AwaitingHuman),
            "the control: `hold` parks the WORK region, which is the half that already existed. \
             active = {held:?}",
        );
        assert!(
            held.contains(&AiLoopState::Held),
            "⚠⚠⚠⚠⚠ THE ORDER NEVER REACHED THE ORDERS REGION, so the guard below has nothing to \
             read and would refuse nothing. `awaiting_human` cannot say WHY it is waiting — a \
             person typing, a peer gone quiet and an unreadable dialog park a run in the same \
             state — and this state is the entire difference. active = {held:?}",
        );

        // ── THE ENDING ARRIVES ANYWAY, AND MUST BE REFUSED ──
        carried(&mut engine, &host, AiLoopEvent::Unattended, "");
        let after = engine.get_active_states();
        assert!(
            !after.contains(&AiLoopState::Blocked),
            "⚠⚠⚠⚠⚠ A HELD RUN ENDED AS ONE NOBODY CAME TO. `blocked` is `<final>`, so this is not a \
             wrong label on a live run — the run is OVER, and its reader is sent looking for \
             somebody who is standing right there reading the pane. active = {after:?}",
        );
        assert!(
            after.contains(&AiLoopState::AwaitingHuman),
            "⚠⚠⚠ and it must still be WAITING rather than have gone somewhere else quietly: a \
             guard that refused the edge by moving the run would be a third ending nobody asked \
             for. active = {after:?}",
        );

        // ── THE HOLD IS LIFTED, AND THE SAME EVENT MUST NOW END THE RUN ──
        carried(&mut engine, &host, AiLoopEvent::Resume, "");
        let resumed = engine.get_active_states();
        assert!(
            resumed.contains(&AiLoopState::Standing),
            "⚠⚠⚠⚠ THE ORDER DID NOT COME BACK OFF. `hold` is the one order a person can take back, \
             and an order with no way home is a slower `cancel` wearing a kinder word. \
             active = {resumed:?}",
        );

        carried(&mut engine, &host, AiLoopEvent::TurnInterrupted, "");
        carried(&mut engine, &host, AiLoopEvent::Unattended, "");
        let ended = engine.get_active_states();
        assert!(
            ended.contains(&AiLoopState::Blocked),
            "⚠⚠⚠⚠⚠ `unattended` CANNOT END A RUN AT ALL ANY MORE. Nobody is holding this one — the \
             order came off two events ago — so the guard evaluated false with nothing to refuse, \
             which means it is not reading what it thinks it reads. Every run that legitimately \
             runs out of patience now waits forever, and no test that only holds a run would ever \
             see it. active = {ended:?}",
        );
    }

    #[test]
    fn the_outer_loop_runs_the_edges_the_last_two_rounds_built() {
        let (mut engine, host, _lua, _session) = started();
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

        carried(&mut engine, &host, AiLoopEvent::Start, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "a started loop primes a session before it prompts it",
        );

        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // R372: a person reached into the pane. The loop stops driving.
        carried(&mut engine, &host, AiLoopEvent::TurnInterrupted, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::AwaitingHuman,
            "⚠ the edge R372 built the product half of",
        );

        // R373: they let go. The loop takes the pane back and prompts again.
        carried(&mut engine, &host, AiLoopEvent::Resume, "");
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
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // Where the loop went after each completed turn, in order.
        let mut decisions: Vec<(u32, AiLoopState)> = Vec::new();
        let mut turn = 0_u32;
        while engine.get_current_state() == AiLoopState::Working {
            turn += 1;
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "a completed turn is judged, always: turn {turn}",
            );
            // No `_event.data.done`, so the goal-met guard is falsy and the budget
            // guards are what decide. The peer saying the done marker is a
            // different gate; this one is about the two NUMBERS.
            carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
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
                reflected(&mut engine, &host, AiLoopEvent::ReflectNone, "");
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
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
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
        fn out_of_turns() -> (Engine<AiLoopPolicy>, crate::act::Serving) {
            let (mut engine, host, _lua, _session) = started();
            // ⚠ `max_turns` is the document's default here; one turn is enough only because the
            // gate below asserts the state it landed in rather than assuming it.
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            while engine.get_current_state() != AiLoopState::Stopping {
                assert_eq!(
                    engine.get_current_state(),
                    AiLoopState::Working,
                    "the walk to a spent budget goes through `working`",
                );
                carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
                carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
                if engine.get_current_state() == AiLoopState::Reflecting {
                    reflected(&mut engine, &host, AiLoopEvent::ReflectNone, "");
                }
            }
            (engine, host)
        }

        for ending in [
            AiLoopEvent::TurnDone,
            AiLoopEvent::TurnBlocked,
            AiLoopEvent::TurnInterrupted,
        ] {
            let (mut engine, host) = out_of_turns();
            carried(&mut engine, &host, ending, "");
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
            let (mut engine, host) = out_of_turns();
            carried(&mut engine, &host, ending, "");
            assert_eq!(
                engine.get_current_state(),
                landing,
                "⚠⚠ {ending:?} is a fact about the RUN and not about the account's turn",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **A BLOCKED TURN THAT IS THE PEER'S SERVICE FAILING WAITS, WHERE A QUESTION WOULD
    /// HAVE STOPPED THE RUN** — the third branch on `turn.blocked`, asked of the MACHINE.
    ///
    /// # ⚠⚠⚠⚠ What this is a gate against, measured
    ///
    /// A live run worked 28m37s on 2026-08-19 and its turn ended with `API Error: 529 Overloaded.
    /// This is a server-side issue, usually temporary — try again in a moment.` Every step after
    /// that was correct: `working` to `screening`, `screening` to `awaiting_human` because no rule
    /// claims a thing that is not a dialog, `awaiting_human` to `blocked` because the run was told
    /// nobody is watching — and `blocked` is `<final>`. **The screen printed its own remedy and the
    /// host filed it as «nobody could say».**
    ///
    /// # ⚠⚠⚠ The three halves, and why none alone would do
    ///
    /// * **THE CONTROL FIRST.** A blocked turn that is NOT a service failure must still reach
    ///   `screening`. Without it this gate would pass on a document that routed EVERY blocked turn
    ///   into a ten-minute wait, which is the failure that would be hardest to see live — a run
    ///   that answers no dialogs and looks merely slow.
    /// * ⚠⚠⚠⚠ **AND THE ORDER IS ASSERTED WITH BOTH KEYS TRUE**, which is the half a reader would
    ///   skip. `service` sits ABOVE `judged` in the document, so an outage that a judge would also
    ///   have called a design decision waits rather than being redirected. Written the other way
    ///   round the machine still compiles, every state still exists, and a 529 gets a model asked
    ///   about it and then a redirect typed at a service that is not answering.
    /// * ⚠⚠ **AND THE COUNTER MOVES ON THE WAY BACK.** `service_retried` is what tells a reader
    ///   afterwards whether a nine-hour run was hard work or a bad afternoon upstream; a state name
    ///   cannot say it, and an edge that forgot the `<assign>` would look identical here.
    ///
    /// ⚠ Driven with `raise_external`, because the edge routes on `_event.data` — see [`reflected`]
    /// for why a gate must reach its state by the door the product uses.
    #[test]
    fn a_turn_blocked_by_the_peers_service_waits_instead_of_asking_for_a_person() {
        // ⚠ THE CONTROL, and it is first on purpose: an ordinary blocked turn must be untouched by
        // everything this round added.
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);
        carried(
            &mut engine,
            &host,
            AiLoopEvent::TurnBlocked,
            &serde_json::json!({"service": false, "judged": false, "rule": ""}).to_string(),
        );
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Screening,
            "⚠⚠⚠ THE CONTROL: a blocked turn that is not an outage still goes to the rules and \
             then to a person. Without this, a document routing EVERY blocked turn into the wait \
             would pass every other assertion here",
        );

        // ⚠⚠⚠⚠ AND NOW THE OUTAGE — with `judged` ALSO true, so this asserts the ORDER and not
        // merely that a branch exists.
        let (mut engine, host, lua, session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);
        carried(
            &mut engine,
            &host,
            AiLoopEvent::TurnBlocked,
            &serde_json::json!({"service": true, "judged": true, "rule": "security"}).to_string(),
        );
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::ServiceDown,
            "⚠⚠⚠⚠ AN OUTAGE OUTRANKS A DESIGN VERDICT. Both keys are true here, so a document \
             that asked `judged` first would send a service failure to `redirecting` and type a \
             reconsider-this at a service that is not answering",
        );

        carried(&mut engine, &host, AiLoopEvent::ServiceRetry, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "the wait is over and the driver has spoken, so the run is working again — this state \
             is not an ending, which is the whole of what it exists to say",
        );
        let retried = lua.get_variable(&session, "service_retried");
        assert!(
            matches!(retried, Ok(ScriptValue::Int(1))),
            "⚠⚠ THE COUNTER MOVED. A reader asking afterwards why a run took all night has \
             nothing else to tell a bad afternoon upstream from hard work, and an edge missing \
             its `<assign>` would reach `working` looking exactly like this one: {retried:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A CLAIM NOBODY COULD CHECK LEAVES BY A DIFFERENT DOOR THAN ONE A CHECK AGREED
    /// WITH, AND THE TWO SILENCES GET DIFFERENT ANSWERS** — register item 741.
    ///
    /// # What was measured, and why prose was not enough
    ///
    /// `Checked::Silent`'s own sentence is *"Silence is not agreement"*. Across this repository's
    /// entire run log the behaviour said otherwise — a silent check left `judging` by exactly the
    /// doors an agreeing one did (`Reflecting` 15 / `Closing` 2 / `Stopping` 2 against 96 / 11 / 1),
    /// while `Disputing`, the one door that buys another turn, was reached by **none of nineteen**.
    /// Two runs banked a milestone nothing had verified. The word was published, the walk printed
    /// it in prose, and no edge read it.
    ///
    /// # ⚠⚠⚠⚠⚠ Both classes, side by side, or this gate is vacuous
    ///
    /// A silence is two facts wearing one word: a checker that produced **no verdict** wants asking
    /// again, and one that **answered something that is not a verdict** wants its prompt fixed.
    /// Standing up one of them would prove only that some door exists — the claim is that the
    /// DISPOSITION differs, and only the pair can say so. So both are driven here, and the two
    /// composed prompts are asserted to differ AND to carry their own clause.
    ///
    /// ⚠⚠ THE CONTROL IS THE AGREEING CLAIM, driven through the same walk: without it a document
    /// that sent EVERY judgement to `unverified` would satisfy everything above.
    ///
    /// ⚠ The clauses are handed in the way the DRIVER hands them (`start`'s payload), and that a
    /// KIND authors them is a claim about the door — `plugins`'s
    /// `a_kind_says_what_a_silent_checker_owes_and_the_door_carries_both_halves` holds that end.
    #[test]
    fn a_check_that_said_nothing_readable_leaves_by_its_own_door_with_its_own_answer() {
        /// What a kind might author for a checker that never produced a verdict.
        const UNANSWERED: &str = "ASK-THE-CHECKER-AGAIN";
        /// And for one that answered something that is not a verdict.
        const UNREADABLE: &str = "FIX-THE-CHECKERS-PROMPT";
        /// And for one that never got as far as judging — register item 752.
        const UNWELL: &str = "WAIT-THEN-ASK-AGAIN";
        /// The `start` payload a driver sends, carrying both clauses — the pair is one decision, so
        /// a fixture that sent one would be staging a document this door refuses.
        fn briefed() -> String {
            serde_json::json!({
                "unanswered_rule": UNANSWERED,
                "unreadable_rule": UNREADABLE,
                "unwell_rule": UNWELL,
                // ⚠⚠⚠⚠ AND THE CEILING, WHICH THIS FIXTURE LEARNED TO SEND THE HARD WAY. The
                // `brief` transition assigns UNCONDITIONALLY — the driver always sends every key,
                // echoing the template's own values back — so a payload that omits one assigns
                // **nil** over a number the guard below then compares against, and the run ends
                // `failed` on an `error.execution` nobody meant. A fixture must send what the
                // driver sends; the number itself is immaterial to this gate, only that the first
                // silence is under it.
                "reflect_after_refusals": 3,
            })
            .to_string()
        }
        /// Reach `judging`, then judge with `data`, and answer with the state and what the
        /// `unverified` prompt came to.
        fn judged(data: &serde_json::Value) -> (AiLoopState, String) {
            let (mut engine, host, lua, session) = started();
            // ⚠⚠ THE CLAUSES ARRIVE ON `brief` AND NOT ON `start`, which is the door the DRIVER
            // uses: `OuterLoop::brief` sends this event before the run begins, and a fixture that
            // put the payload on `start` would leave every authored value at the template's empty
            // string and then assert about the composition anyway.
            carried(&mut engine, &host, AiLoopEvent::Brief, &briefed());
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "⚠⚠ THE PREMISE: the walk must reach `judging`, or every claim below is about \
                 somewhere else",
            );
            carried(&mut engine, &host, AiLoopEvent::Judge, &data.to_string());
            let said = match lua.get_variable(&session, "unverified_prompt") {
                Ok(ScriptValue::String(said)) => said,
                other => panic!("the composed prompt must be readable: {other:?}"),
            };
            (engine.get_current_state(), said)
        }

        // ── THE CONTROL: a claim a check AGREED with still goes to `reflecting` ──────────────
        let (agreed, _) = judged(&serde_json::json!({ "done": true, "checked": "passed" }));
        assert_eq!(
            agreed,
            AiLoopState::Reflecting,
            "⚠⚠⚠ THE CONTROL FAILED: a verified milestone must go on reflecting exactly as it \
             always did. A document that sent every judgement through the new door would satisfy \
             every assertion below while having changed nothing about SILENCE",
        );

        // ── AND A CLAIM NOBODY COULD CHECK DOES NOT ────────────────────────────────────────
        let (unanswered, asking) = judged(&serde_json::json!({
            "done": true,
            "checked": "silent",
            "silence": "unanswered",
            "explained": "the checker was started and never answered",
        }));
        assert_eq!(
            unanswered,
            AiLoopState::Unverified,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 741: a milestone nothing verified left by the door a VERIFIED \
             one leaves by. That is this document's own sentence — *Silence is not agreement* — \
             being false of its behaviour, and it is how two runs banked a claim no independent \
             process had ever agreed with",
        );
        let (unreadable, fixing) = judged(&serde_json::json!({
            "done": true,
            "checked": "silent",
            "silence": "unreadable",
            "explained": "the checker answered \"Permission\", which is not YES or NO",
        }));
        assert_eq!(
            unreadable,
            AiLoopState::Unverified,
            "⛔⛔⛔ and so must the other silence — one door, because the run is in the same place: \
             it claimed a milestone and nothing checked it",
        );

        // ── AND THE ANSWER IS NOT THE SAME ANSWER ─────────────────────────────────────────
        assert!(
            asking.contains(UNANSWERED) && !asking.contains(UNREADABLE),
            "⛔⛔⛔⛔ REGISTER ITEM 741: a checker that produced NO VERDICT was told to fix its \
             prompt, or told nothing at all. Asking again is the remedy for this one — 4 of this \
             repository's 19 silences were a wait that ended without an answer — and a run sent to \
             tighten a prompt would change the one thing that was working. Got:\n{asking}",
        );
        assert!(
            fixing.contains(UNREADABLE) && !fixing.contains(UNANSWERED),
            "⛔⛔⛔⛔ and the checker that ANSWERED must not be told to ask again: it would get the \
             same shape back, which is 15 of those 19. Got:\n{fixing}",
        );
        assert_ne!(
            asking, fixing,
            "⚠⚠⚠ THE WHOLE ITEM IN ONE LINE: two silences with one answer is the collapse register \
             item 593 measured one layer down, met again at the disposition",
        );

        // ── ⛔⛔⛔⛔⛔ AND THE THIRD, WHICH IS REGISTER ITEM 752 — AND WHICH ONLY A WALK CAN SHOW ──
        //
        // ⚠⚠⚠⚠⚠ **THE DOCUMENT COMPILING IS NOT THE DOCUMENT BRANCHING.** `unverified`'s entry now
        // carries this workspace's FIRST `<elseif>`, and a compiler that parsed it and ignored it
        // would send every `unwell` down the `<else>` — telling a checker stopped by a usage limit
        // to fix its prompt, silently, which is the exact defect item 752 is about. Nothing but
        // driving the real machine can tell those apart, so this arm drives it.
        let (unwell, waiting) = judged(&serde_json::json!({
            "done": true,
            "checked": "silent",
            "silence": "unwell",
            "explained": "the checker was asked for a structured answer and printed a notice",
        }));
        assert_eq!(
            unwell,
            AiLoopState::Unverified,
            "⛔⛔⛔ the third silence leaves by the same door as the other two — the run is in the \
             same place, having claimed a milestone nothing checked",
        );
        assert!(
            waiting.contains(UNWELL)
                && !waiting.contains(UNANSWERED)
                && !waiting.contains(UNREADABLE),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 752: a checker STOPPED BEFORE IT COULD JUDGE was handed \
             another class's remedy. `<elseif>` is this document's first, so the likeliest cause \
             is a compiler that parsed the arm and never takes it — in which case every usage \
             limit this loop meets is filed as a prompt to tighten. Got:\n{waiting}",
        );

        // ── AND THE RUN CAN LEAVE, WHICH IS WHAT KEEPS THE NEW DOOR FROM BEING A TRAP ──────
        //
        // ⚠⚠⚠⚠⚠ A checker at a permission dialog answers nothing EVERY time it is asked, so
        // without the ceiling this door would hand the run back to `working` for ever — register
        // item 449's measured shape, which cost nine refusals and a person pressing Escape. Driven
        // here rather than assumed: the brief sets the ceiling to ONE, so the first silence is
        // already the Nth.
        //
        // ⚠⚠ AND IT IS WHERE THE WORD `unverified` IS ASSIGNED, which is why this arm also stands
        // in for that reason in `the_walk_says_why_a_run_stopped_to_reflect` — that gate reaches
        // its arms through real pumps against stand-in peers, and a stand-in CHECKER that answers
        // nothing is a fixture it does not have. The driver's half — that the word is one it can
        // read back — is `every_edge_into_reflecting_says_why_in_a_word_this_driver_knows`.
        let (mut engine, host, lua, session) = started();
        carried(
            &mut engine,
            &host,
            AiLoopEvent::Brief,
            &serde_json::json!({
                "unanswered_rule": UNANSWERED,
                "unreadable_rule": UNREADABLE,
                "unwell_rule": UNWELL,
                "reflect_after_refusals": 1,
            })
            .to_string(),
        );
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        carried(
            &mut engine,
            &host,
            AiLoopEvent::Judge,
            &serde_json::json!({
                "done": true,
                "checked": "silent",
                "silence": "unanswered",
            })
            .to_string(),
        );
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Reflecting,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 741 / 449: a run whose checker never answers must be able to \
             LEAVE. With the ceiling at one this silence is already the Nth, and a document that \
             sent it back to `working` anyway would spin on a permission dialog until a person \
             pressed Escape — which is exactly what item 449 watched happen to refusals",
        );
        assert!(
            matches!(
                lua.get_variable(&session, "reflect_reason"),
                Ok(ScriptValue::String(ref said))
                    if said == crate::outer::ReflectReason::Unverified.word()
            ),
            "⚠⚠⚠ and it must say WHY in the word this driver reads back, or the reflection reports \
             the PREVIOUS one's cause — `reflect_reason` is not cleared on entry. Read {:?}",
            lua.get_variable(&session, "reflect_reason"),
        );

        // ── AND NEITHER SAYS THE MILESTONE WAS REFUSED ───────────────────────────────────
        //
        // ⚠⚠⚠⚠ THE LEDGER'S OWN WARNING, HELD AS AN ASSERTION: *do not simply flip unreadable into
        // a refusal — one permission dialog kills a round.* `dispute_opening` is what a refusal
        // says, and neither of these may be carrying it.
        for said in [&asking, &fixing] {
            assert!(
                !said.contains("did not agree"),
                "⛔⛔⛔⛔⛔ A SILENCE IS BEING PUT TO THE AGENT AS A REFUSAL. Three of the nineteen \
                 measured silences were a checker sitting at a permission dialog — telling an \
                 agent its work was rejected because nobody could read the checker sends it to \
                 re-do work no one disputed. Got:\n{said}",
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **THE DOOR AN OUTAGE CAME IN AT DECIDES WHETHER ANYTHING MAY BE TYPED ON THE WAY
    /// OUT** — register item 715, and the judgement this document records rather than a tidy-up.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the word is HARMFUL and not merely unnecessary
    ///
    /// A 529 ENDS a turn: the composer is empty, the service has to be asked again, and
    /// `service_retry_text` is what asks it. A USAGE LIMIT ends nothing — and the sentence the peer
    /// prints says so in both directions at once: *"continuing automatically at 3:30am · **esc or
    /// type to cancel**"*. **A keystroke during that window cancels the recovery the wait is
    /// waiting for.** So a run that waited its ten minutes and then said `continue` would destroy
    /// exactly what it was waiting for, on the most predictable interruption it meets.
    ///
    /// That is also why the peer's stated resume TIME is not parsed: a clock even slightly early
    /// types into the cancel window, and it would be a fourth clock nothing keeps in step with
    /// `service_retry_ms`, `service_retry_max` and `await_person_ms`. The sentence is read for the
    /// FACT — *this peer resumes itself* — and the fact chooses an edge that says nothing.
    ///
    /// # ⚠⚠⚠ Four arms, and each of the three controls is a way this passes for nothing
    ///
    /// * **THE SILENT DOOR TYPES NOTHING**, which is the headline.
    /// * ⚠⚠⚠⚠ **AND THE BLOCKED DOOR STILL SPEAKS.** Without this arm, deleting the prompt from
    ///   BOTH edges passes — and a 529's only way back to work is something being said, so that is
    ///   the `blocked` `service_down` exists to prevent, reintroduced silently.
    /// * ⚠⚠ **AND A SILENCE THAT IS NOT AN OUTAGE STILL REACHES A PERSON**, or the guard could be
    ///   the constant `true` and every one of item 458's silences would become a ten-minute wait.
    /// * ⚠⚠ **AND THE COUNTER MOVES ON BOTH**, because a reader in the morning asking why a run
    ///   spent the night in one state must get the same number whichever door it came in at.
    #[test]
    fn an_outage_that_ended_no_turn_is_left_alone_where_one_that_ended_a_turn_is_asked_again() {
        /// Reach `service_down` by `door`, retry, and say where it landed, whether the document
        /// asked for a sentence on the way, and what the counter reads.
        ///
        /// ⚠ Raised by hand rather than through `carried`, because that helper DRAINS the sentence
        /// slot — which is right for a routing gate and is the very fact this one is about.
        fn through(door: AiLoopEvent, data: &str) -> (AiLoopState, Option<String>, ScriptValue) {
            let (mut engine, host, lua, session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Working,
                "the fixture: both doors below are taken from `working` and nowhere else",
            );
            engine.raise_external(door, data, "");
            engine.step();
            let reached = engine.get_current_state();
            let _entering = host.taken(crate::act::Act::Say);
            let _noted = host.taken(crate::act::Act::Note);
            engine.raise_external(AiLoopEvent::ServiceRetry, "", "");
            engine.step();
            let said = match host.taken(crate::act::Act::Say) {
                Some(crate::act::Asked::Say { text, .. }) => Some(text),
                _ => None,
            };
            let _noted = host.taken(crate::act::Act::Note);
            assert_eq!(
                reached,
                AiLoopState::ServiceDown,
                "the fixture: this arm is about the way OUT, so it must have got in",
            );
            (
                engine.get_current_state(),
                said,
                lua.get_variable(&session, "service_retried")
                    .unwrap_or(ScriptValue::Undefined),
            )
        }

        let outage = serde_json::json!({"service": true, "judged": false, "rule": ""}).to_string();

        // ── THE HEADLINE: a peer that never stopped its turn is left alone ──
        let (landed, said, retried) = through(AiLoopEvent::PeerSilent, &outage);
        assert_eq!(
            landed,
            AiLoopState::Working,
            "the wait is over and the run carries on watching the turn the peer never dropped",
        );
        assert_eq!(
            said, None,
            "⛔⛔⛔⛔⛔ NOTHING MAY BE TYPED AT A PEER THAT SAID A KEYSTROKE CANCELS ITS RECOVERY. \
             `Usage limit reached · continuing automatically at 3:30am · esc or type to cancel` is \
             the peer's own sentence, and a run that answers it with `continue` cancels the very \
             thing it waited ten minutes for. Got {said:?}",
        );
        assert!(
            matches!(retried, ScriptValue::Int(1)),
            "⚠⚠ AND THE COUNTER MOVES ON THIS DOOR TOO, or a morning reader cannot tell a night \
             spent waiting out limits from one spent working: {retried:?}",
        );

        // ── ⚠⚠⚠⚠ CONTROL ONE: the door where the turn ENDED must still speak ──
        let (spoke_landed, spoke_said, spoke_retried) = through(AiLoopEvent::TurnBlocked, &outage);
        assert_eq!(
            spoke_landed,
            AiLoopState::Working,
            "the same way back to work"
        );
        assert_eq!(
            spoke_said.as_deref(),
            Some("continue"),
            "⚠⚠⚠⚠⚠ THE CONTROL THAT MAKES THE HEADLINE MEAN ANYTHING. A 529 leaves an EMPTY \
             composer, so this state's only way back to work is something being SAID — delete the \
             prompt from both edges and the headline still passes while a 529 goes back to costing \
             a run its life, which is the `blocked` this state exists to prevent",
        );
        assert!(
            matches!(spoke_retried, ScriptValue::Int(1)),
            "and the counter moves here as it always did: {spoke_retried:?}",
        );

        // ── ⚠⚠ CONTROL TWO: a silence that is NOT an outage still reaches a person ──
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        carried(
            &mut engine,
            &host,
            AiLoopEvent::PeerSilent,
            &serde_json::json!({"service": false}).to_string(),
        );
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠ THE CONTROL: nothing spoke for the pane and nothing on it says why, which is \
             register item 458's silence and still wants a person. A guard that were the constant \
             `true` would turn every one of those into a ten-minute wait, repeated, and trade a \
             visible ending for an invisible one",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AN OUTAGE THAT NEVER CLEARS REACHES A PERSON INSTEAD OF RETRYING ALL NIGHT** —
    /// register item 447's second half, asked of the MACHINE.
    ///
    /// # ⚠⚠⚠⚠ What the state above it fixed, and what it left open
    ///
    /// `service_down` exists because a 529 used to cost a run its life: the outage was filed under
    /// *a human judgement is required*, and an unattended run reaches `blocked`, which is
    /// `<final>`. That is paid, and the gate above this one holds it. **What it did not say is what
    /// happens when the service never comes back** — and the answer was «wait, type, be refused,
    /// wait» until `max_seconds`, which the shipped kind authors at TWENTY-FOUR HOURS. A run that
    /// survives a server hiccup and then burns a night on a server outage has traded one failure
    /// for a quieter one.
    ///
    /// # ⚠⚠⚠ The three arms, and what each alone would let through
    ///
    /// * **THE CEILING.** At the authored count the retry reaches `awaiting_human` — which
    ///   notifies, and ends the run on the caller's own `await_person_ms` rather than on a clock
    ///   invented for outages.
    /// * ⚠⚠⚠⚠ **THE CONTROL, BELOW THE CEILING**, without which a document that sent EVERY retry to
    ///   a person would pass — and that is strictly worse than the defect: it turns the first
    ///   hiccup back into the 28 minutes this whole state was built to stop losing.
    /// * ⚠⚠⚠ **AND `'never'`**, because the document promises `max_turns`' own idiom for *do not
    ///   bound this*. A run that asked for no ceiling must not get one.
    ///
    /// ⚠⚠⚠⚠⚠ **THE `'never'` ARM IS A REGRESSION GUARD ON THE ENGINE AND NOT A TEST OF THE GUARD'S
    /// TEXT — MEASURED, AND SAID HERE BECAUSE A CONTROL CAN BE VACUOUS.** Deleting `service_retry_max
    /// != 'never'` from the document leaves this arm GREEN: at the pinned engine `999 >= 'never'`
    /// already answers false, so the numeric half decides it alone. What this arm therefore holds is
    /// the BEHAVIOUR (*a run that authored no ceiling never reaches a person over an outage*) across
    /// an engine whose coercion rules could change, which is worth having — but a reader who counted
    /// it as proof that the clause does something would be wrong, and that is the mistake this
    /// paragraph exists to stop. The other two arms both red under their own mutations.
    ///
    /// ⚠⚠ The counter is asserted on BOTH sides of the ceiling: it must move on the way back to
    /// work and must NOT move on the way to a person, because nothing was retried — an edge that
    /// carried the `<assign>` on both would tell a morning reader the run tried once more than it
    /// did.
    #[test]
    fn an_outage_that_never_clears_reaches_a_person_instead_of_retrying_all_night() {
        /// Reach `service_down`, author `retried` and `max`, then retry — and say where it landed
        /// and what the counter reads afterwards.
        ///
        /// ⚠ `max` is a [`ScriptValue`] rather than a number, because the whole third arm is that
        /// this document's «no ceiling» is the STRING `'never'`.
        fn retried_at(
            retried: i64,
            max: ScriptValue,
        ) -> (AiLoopState, Result<ScriptValue, String>) {
            let (mut engine, host, lua, session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(
                &mut engine,
                &host,
                AiLoopEvent::TurnBlocked,
                &serde_json::json!({"service": true, "judged": false, "rule": ""}).to_string(),
            );
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::ServiceDown,
                "the fixture: every arm below starts from the outage state",
            );
            lua.set_variable(&session, "service_retried", ScriptValue::Int(retried))
                .expect("the document's own counter is writable");
            lua.set_variable(&session, "service_retry_max", max)
                .expect("the document's own ceiling is writable");
            carried(&mut engine, &host, AiLoopEvent::ServiceRetry, "");
            let counter = lua
                .get_variable(&session, "service_retried")
                .map_err(|error| format!("{error:?}"));
            (engine.get_current_state(), counter)
        }

        // ── THE HEADLINE: the ceiling is reached and the run asks for a person ──
        let (exhausted, still_at) = retried_at(6, ScriptValue::Int(6));
        assert_eq!(
            exhausted,
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠⚠ SIX WAITS OF TEN MINUTES IS AN HOUR, AND AN HOUR OF REFUSAL IS NOT A HICCUP. \
             Left in `working` this run types `continue` at a service that is not answering, is \
             refused, waits again — until a 24-hour ceiling the document cannot see. Reaching a \
             person is what makes the run's ending say something true",
        );
        assert!(
            matches!(still_at, Ok(ScriptValue::Int(6))),
            "⚠⚠ and NOTHING WAS RETRIED, so the counter must not move: this edge carries no \
             `<assign>` and an edge that carried one would tell a morning reader the run tried a \
             seventh time. Got {still_at:?}",
        );

        // ── CONTROL ONE: below the ceiling, the outage is still just an outage ──
        let (waiting, moved) = retried_at(5, ScriptValue::Int(6));
        assert_eq!(
            waiting,
            AiLoopState::Working,
            "⚠⚠⚠⚠ THE CONTROL FAILED, and it is the expensive way to be wrong: a document that \
             sends every retry to a person has put the 28 minutes back. The FIRST hiccup must cost \
             a wait and nothing else",
        );
        assert!(
            matches!(moved, Ok(ScriptValue::Int(6))),
            "and the counter moves on the way back to work, which is the only thing that tells a \
             nine-hour run apart from a bad afternoon upstream: {moved:?}",
        );

        // ── CONTROL TWO: `'never'` is this document's own word for no ceiling ──
        let (unbounded, _) = retried_at(999, ScriptValue::String("never".to_string()));
        assert_eq!(
            unbounded,
            AiLoopState::Working,
            "⚠⚠⚠ THE CONTROL FAILED. `max_turns` spells *do not bound this* as `'never'` and this \
             guard promises the same word; a comparison that read the string as a number would \
             ceiling on the first retry of every run that asked for no ceiling — the opposite of \
             what its author wrote",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A PEER THAT SAID IT IS COMING BACK IS WAITED FOR PAST THE HOUR A REFUSING SERVER
    /// GETS** — register item 724, which is the SURVIVAL item 715 named and did not pay.
    ///
    /// # ⚠⚠⚠⚠ What 715 left, in one paragraph
    ///
    /// 715 made a usage limit REACH `service_down`: the peer prints that it is continuing
    /// automatically, `peer.silent` carries it in, and the way back out types nothing because a
    /// keystroke would cancel the recovery. Both doors were then counted by ONE ceiling whose
    /// argument, written above its `<data>`, is the shape of a 529 — six waits of ten minutes,
    /// because *a server refusing for an hour is not a hiccup*. Measured 2026-08-27 in the peer's
    /// own transcripts, a usage-limit window runs to FIVE hours. So the run got the outage's name
    /// right and still walked out on it after one, which is the whole of item 724.
    ///
    /// # ⚠⚠⚠ The three arms, and what each alone would let through
    ///
    /// * **THE HEADLINE**: at a count that exhausts the 529 ceiling, the self-resuming door is
    ///   still at work. ⚠ The count is READ OFF THE SHIPPED DOCUMENT rather than written here, so
    ///   nobody can make this arm vacuous by lowering `service_retry_max` underneath it.
    /// * ⚠⚠⚠ **AND IT IS A CEILING, NOT AN ABSENCE**: at `service_resumes_max` the same door
    ///   reaches a person. A repair that simply dropped the guard for this door passes the headline
    ///   and then burns a night on a peer that printed the notice and died — the failure the older
    ///   ceiling exists to stop, moved one door over rather than answered.
    /// * ⚠⚠⚠⚠ **THE CONTROL (R27), AND IT IS WHAT MAKES THE HEADLINE MEAN ANYTHING**: the OTHER
    ///   door, at the same count, still reaches a person. Without it, raising the single shared
    ///   ceiling from six to thirty-six passes both arms above — and that is the workaround item
    ///   724 forbids by name, because it would report *the server never came back* six times later
    ///   for the case the number was actually written for.
    ///
    /// ⚠⚠ The two ceilings' PRODUCT with the wait — *how many hours is this, really* — is not
    /// asked here, because the wait lives in the kind's document and not this one. It is ratcheted
    /// where both are in scope: `the_budget_for_a_peer_that_resumes_itself_outlasts_the_longest_limit_measured`.
    #[test]
    fn a_peer_that_resumes_itself_is_waited_for_past_the_hour_a_refusing_server_gets() {
        /// Reach `service_down` by one of the two doors, author the counter, retry — and say where
        /// it landed, alongside the two ceilings AS THE DOCUMENT SHIPS THEM.
        ///
        /// `resumes` picks the door, and it is the same distinction `service_resumes_itself`
        /// records: `true` is `peer.silent` carrying a service, the peer that printed it is
        /// continuing on its own; `false` is `turn.blocked`, which is the 529's door and means the
        /// turn ENDED.
        /// **THE OUTAGE THAT ACTUALLY HAPPENED**, 2026-08-27: run 5 was cut off at 01:0x and its
        /// peer printed an 03:30 reset. It is the arm's subject rather than a round number — item
        /// 724's done-when is that a MEASURED window is survived, and a fixture picked for
        /// convenience would leave that unsaid.
        const RUN_5_OUTAGE: Duration = Duration::from_secs(2 * 60 * 60 + 30 * 60);

        fn retried_at(resumes: bool, retried: i64) -> (AiLoopState, i64, i64, i64) {
            let (mut engine, host, lua, session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            if resumes {
                carried(
                    &mut engine,
                    &host,
                    AiLoopEvent::PeerSilent,
                    &serde_json::json!({"service": true}).to_string(),
                );
            } else {
                carried(
                    &mut engine,
                    &host,
                    AiLoopEvent::TurnBlocked,
                    &serde_json::json!({"service": true, "judged": false, "rule": ""}).to_string(),
                );
            }
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::ServiceDown,
                "the fixture: BOTH doors must reach the outage state, or no arm below is about a \
                 ceiling at all",
            );
            // ⚠⚠ READ, NOT RESTATED. An arm that wrote `6` here would keep passing the day the
            // document changed its mind, which is the failure this whole register calls a stale
            // fixture. The names are literals because the DRIVER does not read these — the
            // document decides both ceilings alone — and a typo reads as nil and panics by name.
            let held = |name: &str| match lua.get_variable(&session, name) {
                Ok(ScriptValue::Int(held)) => held,
                other => panic!("the document must declare `{name}` as a number: {other:?}"),
            };
            let ceilings = (held("service_retry_max"), held("service_resumes_max"));
            // ⚠⚠ AND HOW MANY RETRIES A MEASURED OUTAGE IS, computed from the document's own wait
            // rather than written down: item 724's arms are about HOURS, and a count only means
            // hours through this number.
            let waits_in_run_5 = RUN_5_OUTAGE.as_millis() as i64 / held("service_retry_ms");
            lua.set_variable(&session, "service_retried", ScriptValue::Int(retried))
                .expect("the document's own counter is writable");
            carried(&mut engine, &host, AiLoopEvent::ServiceRetry, "");
            (
                engine.get_current_state(),
                ceilings.0,
                ceilings.1,
                waits_in_run_5,
            )
        }

        // ── THE PREMISE, asked of the document before any arm leans on it ──
        let (fresh, typed_at, resumes_max, run_5_waits) = retried_at(true, 0);
        assert_eq!(
            fresh,
            AiLoopState::Working,
            "the fixture: an outage nowhere near either ceiling goes back to work",
        );
        assert!(
            resumes_max > typed_at,
            "⚠⚠⚠⚠⚠ THE PREMISE OF EVERY ARM BELOW: the two doors must hold DIFFERENT budgets. \
             Given one number wearing two names, the headline and the control below cannot \
             disagree, and the gate would be green over exactly the defect item 724 registered — \
             a usage limit measured in hours paid out of a budget argued from a 529. Got \
             {typed_at} and {resumes_max}",
        );

        // ── THE HEADLINE: the hour that ends a 529 does not end a limit ──
        let (outlasting, _, _, _) = retried_at(true, typed_at);
        assert_eq!(
            outlasting,
            AiLoopState::Working,
            "⛔⛔⛔⛔⛔ THE PEER SAID IT IS COMING BACK AND THE RUN MUST STILL BE THERE WHEN IT \
             DOES. At this count a 529 is out of budget and rightly so — the loop has been typing \
             at a service that keeps refusing. This peer was typed at NOTHING and is returning on \
             its own; leaving over it is item 715's measured divergence, where the agent resumed \
             at 3:30 and worked two hours with no run driving it, so nothing judged the work",
        );

        // ── AND THE WINDOW THAT ACTUALLY HAPPENED IS SURVIVED, which is item 724's done-when ──
        //
        // ⚠⚠ The count above is the OLD CEILING, which proves the split fires; this one is RUN 5,
        // which is the only thing that proves the split is big enough. A budget raised to seven
        // would pass the headline and lose the very outage the item was registered on.
        assert!(
            run_5_waits > typed_at,
            "⚠⚠⚠⚠⚠ THE PREMISE OF THE ARM BELOW, and the measurement item 724 exists for: run 5's \
             outage must be LONGER than the budget it was paid out of, or surviving it says \
             nothing. {run_5_waits} waits against a ceiling of {typed_at}",
        );
        let (survived, _, _, _) = retried_at(true, run_5_waits);
        assert_eq!(
            survived,
            AiLoopState::Working,
            "⛔⛔⛔⛔⛔ THE MEASURED OUTAGE MUST BE SURVIVED, not merely named. Run 5 was cut off at \
             01:0x for an 03:30 reset — {run_5_waits} waits of the document's own length — and the \
             run must still be at work at the end of it, because the peer was",
        );

        // ── AND IT IS A CEILING: a peer that printed the notice and died still reaches a person ──
        let (exhausted, _, _, _) = retried_at(true, resumes_max);
        assert_eq!(
            exhausted,
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠ AN UNBOUNDED WAIT IS THE OTHER WAY TO LOSE A NIGHT, and the ceiling above this \
             door exists for the same reason the older one does. Six hours is long enough that a \
             measured limit window cannot outlast it and short enough that the run still fails \
             VISIBLY, well inside `max_seconds`",
        );

        // ── ⚠⚠⚠ THE CONTROL (R27): the other door is unchanged, so this is a split and not a raise ──
        let (refusing, _, _, _) = retried_at(false, typed_at);
        assert_eq!(
            refusing,
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, AND IT IS THE ONE THAT SEPARATES THE REPAIR FROM THE \
             WORKAROUND. Raising the single shared ceiling would pass both arms above while \
             telling a morning reader six times later than it used to that a server never came \
             back. The 529's number is argued from the 529's shape and must not move because a \
             DIFFERENT failure needed longer",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AN OUTAGE ONE DOOR WAITED OUT DOES NOT SPEND THE OTHER DOOR'S BUDGET** — register
    /// item 729, and the half that makes item 724's two ceilings actually two.
    ///
    /// # ⚠⚠⚠⚠⚠ Two ceilings drawing on one counter are not two ceilings
    ///
    /// `service_retried` is declared *how many times THIS RUN has waited out its peer's service*,
    /// and it is never reset — the document's own words, for a reader asking whether a run took
    /// nine hours because the work was hard or because the service was down. Item 447 then also
    /// pointed a ceiling at it, which was survivable while both doors shared one number of six.
    /// Item 724 gave one door thirty-six, and the collision bit: **a run that had just survived a
    /// five-hour usage limit arrived at its next 529 already twenty waits deep, and a server that
    /// hiccupped once got no retry at all** — reported as *a person is needed*, about a service
    /// that was working.
    ///
    /// # ⚠⚠⚠ What is driven here, and what each arm alone would let through
    ///
    /// * **THE HEADLINE**: spend the self-resuming door's whole budget, complete a turn, then meet
    ///   a 529 — and the 529 must get its own retries. ⚠ The premise is asserted rather than
    ///   assumed: the waits spent first must EXCEED the other door's ceiling, or the arm passes on
    ///   a document that never fixed anything (register item 657's lesson).
    /// * ⚠⚠⚠⚠ **THE CONTROL, AND IT IS THE ONE THAT STOPS THIS BEING «THE CEILING WAS REMOVED»**:
    ///   the same 529, spent WITHOUT a completed turn in between, must still reach a person. That
    ///   is the same run, the same counter and the same ceiling — the single difference is the
    ///   `turn.done` — so a repair that simply widened or deleted the guard fails here.
    /// * ⚠⚠ **AND THE REPORT IS UNTOUCHED**: `service_retried` must still read the RUN TOTAL after
    ///   all of it. The whole design is that the report keeps its declared meaning while the
    ///   budget gets its own; a repair that reset the counter would pass both arms above and
    ///   quietly answer *nine hours of outage* with *one*.
    #[test]
    fn an_outage_one_door_waited_out_does_not_spend_the_other_doors_budget() {
        /// Drive a run into `service_down` by the self-resuming door, spend `waits` there, then
        /// optionally let a turn COMPLETE, then meet a 529 and retry once.
        ///
        /// Hands back where that 529 landed and what the run total reads afterwards.
        ///
        /// ⚠ The waits are authored onto the counter rather than pumped one at a time: what is
        /// under test is the ARITHMETIC the guards do, and thirty-six real retries would measure
        /// the clock instead.
        fn after_an_outage_of(waits: i64, then_a_turn_completes: bool) -> (AiLoopState, i64) {
            let (mut engine, host, lua, session) = started();
            carried(&mut engine, &host, AiLoopEvent::Start, "");
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(
                &mut engine,
                &host,
                AiLoopEvent::PeerSilent,
                &serde_json::json!({"service": true}).to_string(),
            );
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::ServiceDown,
                "the fixture: the first outage must be the self-resuming one",
            );
            lua.set_variable(&session, "service_retried", ScriptValue::Int(waits))
                .expect("the document's own counter is writable");
            carried(&mut engine, &host, AiLoopEvent::ServiceRetry, "");
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Working,
                "the fixture: {waits} waits must be INSIDE the self-resuming door's budget, or \
                 this run never gets to the second outage at all",
            );
            if then_a_turn_completes {
                // ⚠⚠ THE PEER ANSWERED, which is the only proof this document ever gets that the
                // service came back — and the moment item 729 hangs everything on. Driven through
                // the real event rather than by writing the watermark, because *which moment ends
                // an outage* is exactly the claim under test.
                carried(
                    &mut engine,
                    &host,
                    AiLoopEvent::TurnDone,
                    &serde_json::json!({"context": 0, "cold": 0, "floor": 0}).to_string(),
                );
                assert_eq!(
                    engine.get_current_state(),
                    AiLoopState::Judging,
                    "the fixture: a completed turn reaches the state whose entry carries the mark",
                );
                carried(
                    &mut engine,
                    &host,
                    AiLoopEvent::Judge,
                    &serde_json::json!({"done": false, "checked": "", "rule": ""}).to_string(),
                );
                carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            }
            // ── THE SECOND OUTAGE, at the OTHER door ──
            carried(
                &mut engine,
                &host,
                AiLoopEvent::TurnBlocked,
                &serde_json::json!({"service": true, "judged": false, "rule": ""}).to_string(),
            );
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::ServiceDown,
                "the fixture: the second outage must be the typed-at one, or the arms compare one \
                 door with itself",
            );
            carried(&mut engine, &host, AiLoopEvent::ServiceRetry, "");
            let total = match lua.get_variable(&session, "service_retried") {
                Ok(ScriptValue::Int(total)) => total,
                other => panic!("the run total must stay a number: {other:?}"),
            };
            (engine.get_current_state(), total)
        }

        // Waits to spend at the first door: past the OTHER door's ceiling, inside this one's.
        //
        // ⚠⚠ Read off the shipped document below rather than written here, for the reason every
        // arm of the gate above this one is: a constant would keep passing the day either ceiling
        // moved, and the whole point is the relationship between them.
        let (typed_at, resumes_max) = {
            let (_, host, lua, session) = started();
            drop(host);
            let held = |name: &str| match lua.get_variable(&session, name) {
                Ok(ScriptValue::Int(held)) => held,
                other => panic!("the document must declare `{name}` as a number: {other:?}"),
            };
            (held("service_retry_max"), held("service_resumes_max"))
        };
        let spent = resumes_max - 1;
        assert!(
            spent > typed_at,
            "⚠⚠⚠⚠⚠ THE PREMISE, and without it this gate measures nothing: the first outage must \
             spend MORE than the second door's whole ceiling, or a document that never separated \
             the budgets would pass every arm below. Spent {spent} against a ceiling of {typed_at}",
        );

        // ── THE HEADLINE: a turn completed in between, so the 529 gets its own budget ──
        let (fresh_budget, total_after) = after_an_outage_of(spent, true);
        assert_eq!(
            fresh_budget,
            AiLoopState::Working,
            "⛔⛔⛔⛔⛔ THE SECOND OUTAGE MUST GET ITS OWN RETRIES. This run waited out a usage \
             limit, the peer came back, it worked, and then a server hiccupped once — and a \
             document that counts the whole run against a ceiling of {typed_at} sends it to a \
             person without trying, reporting that somebody is needed about a service that is up",
        );
        // ⚠ TWO retries are driven above — one out of each outage — on top of the waits authored
        // onto the counter, and the total must have counted BOTH. Spelled as the sum rather than
        // as a literal so the arm still reads as a claim about the report when the ceilings move.
        assert_eq!(
            total_after,
            spent + 2,
            "⚠⚠⚠⚠ AND THE RUN TOTAL COUNTED BOTH OUTAGES, which is the half that keeps the report \
             honest. `service_retried` is declared as how much of THIS RUN was outage and a reader \
             asks it at the end; a repair that RESET the counter instead of marking it would pass \
             the arm above and then answer «nine hours of outage» with «one»",
        );

        // ── ⚠⚠⚠ THE CONTROL: no completed turn, so it is one long outage and the ceiling holds ──
        let (still_bounded, _) = after_an_outage_of(spent, false);
        assert_eq!(
            still_bounded,
            AiLoopState::AwaitingHuman,
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, AND IT IS WHAT SEPARATES «THE BUDGET IS SCOPED» FROM «THE \
             CEILING IS GONE». Same run, same counter, same ceiling as the headline — the only \
             difference is that no turn completed, so nothing ever proved the peer came back. A \
             document that widened or dropped the guard passes the headline and fails here",
        );
    }

    /// ⚠⚠⚠⚠ **AND THE SENTENCE A PERSON READS SAYS WHICH OUTAGE THE COUNT IS AGAINST** — register
    /// item 724's own residue, paid in the same round rather than registered.
    ///
    /// # ⚠⚠⚠⚠⚠ The defect this exists to stop was CREATED by the repair above it
    ///
    /// Before 724 both doors were counted by one ceiling of six, so *waited it out N time(s)*
    /// carried its own scale: a reader who knew the ceiling knew what N meant. The split gives the
    /// self-resuming door thirty-six, and the two now differ by a factor of six — so **twelve** is
    /// twice past the budget at one door and a third of the way through it at the other, printed
    /// into the same slot of the same sentence. That is register item 718's shape exactly (*two
    /// quantities wearing one slot, and both watchers read it wrong*), and it would have been minted
    /// by the item that fixed the budget.
    ///
    /// ⚠⚠ **`assert_ne` IS THE LOAD-BEARING ONE.** A branch that named the door in a comment, or
    /// that produced the same words for both, would satisfy a `contains` for either half; what a
    /// reader needs is that the two situations do not read alike. The `contains` arms then say
    /// which is which, so a build that merely made them differ cannot pass by swapping them.
    #[test]
    fn the_account_of_an_outage_says_which_of_the_two_it_was() {
        /// The sentence this build leaves behind for an outage that arrived at one of the doors.
        ///
        /// ⚠ Asked of [`AiLoop::account_of`] rather than driven: these words are a pure function of
        /// the notice, and driving six hours of outage to reach a format string would measure the
        /// clock and call it the report. That the notice's `resumes` is READ OFF THE DOCUMENT
        /// rather than guessed by the driver is a different claim, gated where the retry is.
        fn left_after(resumes: bool) -> String {
            AiLoop::account_of(Some(&Noticed::ServiceDown {
                retried: 12,
                waited: Duration::from_secs(600),
                resumes,
            }))
            .expect("an outage the run walked away from leaves an account behind")
        }

        let (resuming, refusing) = (left_after(true), left_after(false));
        assert_ne!(
            resuming, refusing,
            "⛔⛔⛔⛔⛔ TWELVE MEANS TWO DIFFERENT THINGS AND THE PAGE MUST SAY WHICH. Since item \
             724 the doors hold ceilings of thirty-six and six, so one of these runs is a third of \
             the way through its budget and the other is twice past it — and a reader given the \
             same sentence for both has item 718's defect, freshly minted by the item that made \
             the budgets differ",
        );
        assert!(
            resuming.contains("continuing on its own") && resuming.contains("12 time(s)"),
            "⚠⚠⚠ the self-resuming door's account must say the peer is COMING BACK, which is what \
             makes twelve waits read as patience rather than as a run flogging a dead service: \
             {resuming:?}",
        );
        assert!(
            refusing.contains("service was down") && refusing.contains("12 time(s)"),
            "⚠⚠ and the other must still say what it always said, or this repair traded one \
             unreadable sentence for another: {refusing:?}",
        );
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
        let (mut engine, host, _lua, _session) = started();
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // ⚠ THE CONTROL: with nothing screened, a judged turn goes straight back to work. The
        // document ships `reflect_every: 8`, so nothing else can send this turn to `reflecting`.
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "the control: an unscreened turn is not a reason to reflect, or the assertion below is \
             about the turn budget",
        );

        // The peer asks, a rule claims it, and the driver reports what it said.
        carried(
            &mut engine,
            &host,
            AiLoopEvent::TurnBlocked,
            r#"{"service": false, "judged": false, "rule": ""}"#,
        );
        assert_eq!(engine.get_current_state(), AiLoopState::Screening);
        carried(
            &mut engine,
            &host,
            AiLoopEvent::ScreenMatched,
            &serde_json::json!({"text": "do it another way"}).to_string(),
        );
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "a claimed dialog returns to work — the peer has just been handed its answer",
        );

        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Reflecting,
            "⚠⚠⚠ THE FIRST HALF: the judgement straight after a standing instruction fired must \
             reflect, at turn TWO of a document whose `reflect_every` is 8",
        );

        // Nothing worth changing — back to work without a restart.
        reflected(&mut engine, &host, AiLoopEvent::ReflectNone, "");
        assert_eq!(engine.get_current_state(), AiLoopState::Working);
        carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
        carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠⚠⚠ THE SECOND HALF: the SAME instruction must not send the run back to `reflecting` \
             for ever. `screened_carried` is set on entry, so this judgement sees the two counts \
             equal — set on the way out instead, this loop would never judge another turn and no \
             state name would show it",
        );
    }

    /// ⛔⛔⛔ **A MILESTONE RE-TYPED INTO A REPLACEMENT SESSION SAYS HOW OLD IT IS** — register
    /// item 592, and the number is re-read at the moment somebody is briefed rather than frozen
    /// with the text.
    ///
    /// # What a snapshot in a milestone cost, measured
    ///
    /// A live run's brief read `Milestone: 전수 라운드(pid 3988592, 현재 18/21 CAUGHT …)` beside
    /// `What to carry: the fix is done, do not build it again`. `ps -p 3988592` found **nothing**;
    /// the run went **55 iterations and committed nothing**, because its agent was told to carry on
    /// with a process that did not exist and told not to rebuild it.
    ///
    /// ⚠⚠⚠⚠⚠ **THE DEFECT IS NOT THAT THE pid DIED.** It is that the milestone HOLDS a snapshot and
    /// is then re-typed VERBATIM at every re-priming (`reflect_every` makes that every few turns),
    /// so a number true when it was written is presented as true now, for ever. This document
    /// cannot check somebody else's pid and must not pretend to — what it can say, and what nothing
    /// said before, is HOW OLD the claim is.
    ///
    /// # ⚠⚠⚠⚠ Why the assertion is about a REPLACEMENT and not about the first briefing
    ///
    /// `priming` is entered twice: once at the start, and again on `session.ready`, which is the
    /// door a replacement session comes through. **The first briefing cannot be stale** — the
    /// milestone was set moments before — so a gate that read only that one would be green over a
    /// composition that had frozen the number. The walk below takes turns FIRST and is briefed
    /// AFTER, which is the only arrangement where a frozen number and a live one differ.
    ///
    /// ⚠⚠ **AND THE CONTROL IS THE FIRST BRIEFING**, asserted to carry NO age clause at all: a
    /// fresh milestone has nothing to warn anybody about, and a clause printed on every run's
    /// opening prompt is noise on the common path — which is exactly what gets skimmed past on the
    /// rare one.
    #[test]
    fn a_milestone_re_typed_into_a_replacement_session_says_how_old_it_is() {
        /// What the composed clause says when the milestone has aged — the words a briefed agent
        /// acts on, and the reason it is not merely a number.
        const AGED: &str = "turns ago; check it still describes live facts";

        let (mut engine, host, lua, session) = started();
        let composed =
            |lua: &Arc<dyn IScriptEngine>| match lua.get_variable(&session, "start_prompt") {
                Ok(ScriptValue::String(text)) => text,
                other => panic!("`priming` must compose a string prompt: {other:?}"),
            };

        // ── THE CONTROL: THE FIRST BRIEFING, WHERE THE MILESTONE IS BRAND NEW ──
        carried(&mut engine, &host, AiLoopEvent::Start, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: the composition runs on entry to `priming`",
        );
        let first = composed(&lua);
        assert!(
            !first.contains(AGED),
            "⚠⚠⚠ THE CONTROL: a milestone set moments ago has nothing to warn anybody about, and a \
             clause on EVERY opening prompt is noise on the common path — which is what gets \
             skimmed on the rare one. Got {first:?}",
        );

        // ── TURNS PASS, AND THEN A SESSION IS REPLACED UNDER THE SAME MILESTONE ──
        //
        // ⚠ Each turn is driven through the door the driver uses, carrying what `judging`'s entry
        // reads: an empty raise makes that block index nil and the walk ends `failed` on an error
        // nobody meant — the trap `carried`'s own doc records.
        for _ in 0..3 {
            carried(&mut engine, &host, AiLoopEvent::PromptSent, "");
            carried(&mut engine, &host, AiLoopEvent::TurnDone, TURN);
            carried(&mut engine, &host, AiLoopEvent::Judge, ORDINARY);
        }
        // ⚠ AND THIS ONE CARRIES ITS WORD FOR THE SAME REASON, one edge further on: the first
        // `prompt.unasked` guard reads `_event.data.retyped`, so an empty raise fails the guard's
        // own expression and this walk would end `failed` instead of reaching the replacement.
        carried(&mut engine, &host, AiLoopEvent::PromptUnasked, UNASKED);
        carried(&mut engine, &host, AiLoopEvent::SessionReplaced, "");
        carried(&mut engine, &host, AiLoopEvent::SessionReady, "");
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "⚠⚠⚠ the fixture must have come back through `priming`, or what is read below is the \
             FIRST briefing and this gate is about nothing",
        );

        // ── AND THE REPLACEMENT IS TOLD THE TEXT IS OLD ──
        let again = composed(&lua);
        assert!(
            again.contains(AGED),
            "⛔⛔⛔ ITEM 592: this session is being briefed with a milestone authored several turns \
             ago and is not told so. A milestone holding a pid or a progress fraction is a claim \
             that was true once; re-typing it verbatim presents it as true now, and a run spent 55 \
             iterations on exactly that. Got {again:?}",
        );
        assert!(
            again.contains("set 3 turns ago"),
            "⚠⚠⚠⚠⚠ AND THE NUMBER IS THE LIVE ONE. Three turns were taken before this briefing, so \
             a composition that froze the age with the TEXT would say `0` here and a briefing at \
             turn fifty would still say `0` — which is the defect wearing the fix's clothes. Got \
             {again:?}",
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
        let (mut engine, host, lua, session) = started();

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
        carried(&mut engine, &host, AiLoopEvent::Start, "");
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
        let (_engine, host, lua, session) = started();

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
        carried(
            &mut engine,
            &host,
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": sent,
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
        );
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
        carried(
            &mut engine,
            &host,
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": "a \"quoted\" line\nand a second one",
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
        );
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
    /// [`Completion::stands`](crate::completion::Completion) still asks the caller's contract BEFORE
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
            (
                Verdict::PeerGone(pane),
                Some("Priming --PeerGone--> PeerGone"),
            ),
            "⚠⚠⚠⚠ THE DOCUMENT'S OWN EDGE, ON THE FIRST PASS. This is the loop meeting the refusal \
             at the door and telling `ai_loop.scxml` rather than walking on. ⚠⚠⚠ MEASURED BEFORE \
             the document had the word: the same loop reported `Priming --PromptSent--> Working` \
             charging `Bytes(0)` — the machine had already moved PAST a prompt that never went in, \
             and then waited in `working` for an answer to a question nobody had been asked",
        );
        // ⚠⚠⚠⚠⚠ **IT SAYS `priming`, AND IT SAID `idle` UNTIL 2026-08-23.** The pass raises
        // `start` first — `idle --start--> priming` is what SENDS the start prompt — and only then
        // does delivering it meet the dead peer, so `priming` is where `peer.gone` is answered
        // from. `pump` used to stamp this line with the state the PASS BEGAN in, and the older
        // spelling here was that lie written down as a guarantee.
        //
        // ⚠⚠⚠ THE SAME LIE COST REGISTER ITEM 605 FOUR ROUNDS one state over, where it read
        // `Judging --PeerGone--> PeerGone` for a raise the machine answered from `working`. Two
        // gates had it pinned; this is the second.
        //
        // ⚠⚠ WHAT THE PASS DID NOT SAY IS STILL MISSING: it raised `start` AND `peer.gone` and
        // this journal carries one of them. Register item 614.
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

    /// ⛔⛔⛔⛔⛔ **A PLUGIN IS PUT BACK WHERE A LOG SAID IT WAS, THROUGH THE DOOR A HOST HAS** —
    /// register item 543's fourth brick.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is not `outer`'s gate said again one layer up
    ///
    /// `outer::tests::a_loop_says_where_its_machine_is_and_can_be_put_back_there` drives
    /// `OuterLoop` and hands it a `LoopPlace` — a type from this crate's insides. **A daemon has
    /// neither.** It holds `Vec<String>` off a run log and a plugin it built from a request, and
    /// the only thing it may say to one is what [`Plugin`] publishes. So the question this gate
    /// asks is the daemon's question: *does the door I actually have put a machine back?*
    ///
    /// It measures four answers, because [`Resumption`] is four for a reason:
    ///
    /// 1. **the claim** — a loop handed the words another loop wrote answers `Placed` and IS there;
    /// 2. **the control that the claim is not vacuous** — a fresh loop is somewhere ELSE, so
    ///    *"it came back"* is not *"a new machine is where a new machine already is"*;
    /// 3. **a foreign document is refused AND NOTHING MOVES** — half-placing a machine and then
    ///    reporting a refusal would be the worst of the four outcomes, and it is the one no reader
    ///    could detect;
    /// 4. **a plugin with no machine says so** — `NoMachine`, not a quiet success, because a boot
    ///    that read a place onto a `pipe` as *placed* would report a resume that never happened.
    #[test]
    fn a_plugins_machine_is_put_back_where_the_words_of_a_log_say_it_was() {
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let run = RunContext::uncancellable();

        // ── THE PREMISE: drive a loop OFF the place a fresh one is in, through the plugin door ──
        let mut ran = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let mut passes = 0;
        while ran.state() != AiLoopState::Judging && passes < 40 {
            ran.step(&access, &run).expect("a live pane takes a pass");
            passes += 1;
        }
        let saved = ran.place().expect("a loop says where its machine is");
        let fresh = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document")
            .place()
            .expect("a loop says where its machine is");
        assert_ne!(
            saved, fresh,
            "⚠⚠ THE PREMISE FAILED: this loop never left the place a FRESH one is already in, so \
             the resume below would be asking whether a new machine is where a new machine already \
             is — which is true of a door that does nothing at all. Compared against a fresh \
             loop rather than a state name spelled here, so the document may rename its states. \
             Took {passes} passes.",
        );

        // ── THE CLAIM: a loop that never took those passes is put there ─────────────────────
        let mut resumed = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        assert_eq!(
            resumed.resume_at(&saved),
            Resumption::Placed,
            "⛔⛔⛔⛔ REGISTER ITEM 543: the words a run log carries were refused by the very build \
             that wrote them. Everything downstream of this — a daemon that restarts without \
             killing its runs, and so a promotion nobody has to schedule around — rests on this \
             one call.",
        );
        assert_eq!(
            resumed.place().as_deref(),
            Some(saved.as_slice()),
            "⛔⛔⛔⛔ and it must be THERE. `Placed` carries no words on purpose — where a plugin \
             is, is `place`'s answer — so a door that reported success while leaving the machine \
             at its initial configuration would be caught here and nowhere else.",
        );

        // ── CONTROL 1: a place from a document this build does not have ─────────────────────
        //
        // ⚠⚠⚠⚠⚠ THE FORGERY IS OTHERWISE REAL, which item 543's own round had to learn twice: a
        // list of one junk word is refused for the WRONG reason (its head is not among the states
        // it names), so it stays green against a decoder that has stopped checking names at all.
        // Keeping a real configuration behind a renamed head makes the NAME the only thing that
        // can produce the refusal.
        let mut renamed = saved.clone();
        renamed[0] = "a-state-this-document-does-not-have".to_owned();
        let mut untouched = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        assert_eq!(
            untouched.resume_at(&renamed),
            Resumption::NotThisDocument,
            "⚠⚠⚠ THE CONTROL: a word this build cannot spell must be refused, never defaulted. A \
             run silently resumed at a state nobody chose spends a peer's tokens doing the wrong \
             work — worse than the honest `interrupted` a person is told today.",
        );
        assert_eq!(
            untouched.place(),
            Some(fresh.clone()),
            "⛔⛔⛔ AND A REFUSED RESUME MUST NOT HAVE MOVED IT. A door that placed what it could \
             and then said no would leave a machine half in a document it does not belong to, and \
             the refusal its caller logged would be the reason nobody went looking.",
        );

        // ── CONTROL 2: a plugin with no machine does not claim to have been placed ───────────
        let mut relay = crate::pipe::Pipe::new(crate::pipe::PipeSpec {
            src: pane,
            dst: pane,
            ready_when: None,
            ready_within: None,
            may_answer: None,
            attended: crate::readiness::Attended::NoOne,
        });
        assert_eq!(
            relay.resume_at(&saved),
            Resumption::NoMachine,
            "⚠⚠⚠ THE SECOND CONTROL: a plugin that relays bytes has no place to be put back in, \
             and must say which of the four answers that is. A boot reading `Placed` off a `pipe` \
             would put a run back on the row as running with nothing resumed.",
        );

        // ── CONTROL 3: words this document CAN spell that are not a place it can BE in ───────
        //
        // ⚠⚠⚠⚠⚠ This is the fourth answer, and it is the one no other arm here can reach. The
        // decoder only asks whether the words are spellable and whether the head is among them, so
        // a set that is spellable and NOT ancestor-closed gets past it and the ENGINE refuses —
        // which is a different author of a different problem, and the whole reason `Refused`
        // carries a sentence rather than being a second `NotThisDocument`.
        //
        // ⚠⚠ **AND THE SENTENCE MUST BE IN THE LOG'S OWN VOCABULARY.** `ConfigurationRejection`'s
        // `Debug` prints the GENERATED enum's identifiers, which are not the names the record is
        // written in, so a refusal spelled that way names a state the reader never saw. That is
        // what `refusal_in_words` exists for and this is the only thing that measures it.
        let mut broken = saved.clone();
        let dropped = broken
            .iter()
            .position(|word| *word != saved[0])
            .expect("a place holds more than its head");
        let dropped = broken.remove(dropped);
        let mut unclosed = AiLoop::new(engine(), pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        match unclosed.resume_at(&broken) {
            Resumption::Refused(why) => {
                assert!(
                    !why.contains('{'),
                    "⚠⚠⚠ the engine's refusal reached a reader spelled as a Rust value. The words \
                     a run log holds come from `get_state_name`; a sentence naming \
                     `CurrentNotActive {{ current: Idle }}` names something nobody wrote down and \
                     nobody can go and look for. Said: {why:?}",
                );
                assert!(
                    saved.iter().any(|word| why.contains(word)),
                    "⚠⚠ and it must name a word from the place it was given, or the reader cannot \
                     tell WHICH part of their record was wrong. Dropped {dropped:?}; said {why:?}",
                );
            }
            other => panic!(
                "⚠⚠ THE THIRD CONTROL'S PREMISE FAILED: dropping {dropped:?} out of {saved:?} was \
                 meant to leave words this document spells that are not a configuration it can \
                 hold, and the answer was {other:?} instead. Pick a different member — the arm \
                 being measured is the ENGINE's refusal, which nothing else here reaches.",
            ),
        }

        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔⛔ **A RESUMED LOOP IS PLACED RATHER THAN WALKED, AND DOES NOT RE-OPEN WITH ITS FIRST
    /// PROMPT** — register item 543's hard half, in the sentence the item has been written in since
    /// it was filed: *"`onentry` actions must NOT re-fire (the loop would re-type its prompts)"*.
    ///
    /// # ⚠⚠⚠⚠⚠ Two claims, because either alone is satisfied by something broken
    ///
    /// *It did not re-type* is also true of a loop that cannot reach its pane. *It was placed* is
    /// also true of a placement that then walked itself back down. So this asserts both: nothing
    /// was WALKED on the way in (`walked()` is empty, which is what `enter_at` buys), and the pane
    /// never receives the words a fresh loop opens with.
    ///
    /// ⚠⚠ **THE NEEDLE IS THE BRIEF'S OWN NORTH STAR**, taken from `brief_for` rather than spelled
    /// here, and the CONTROL is a fresh loop's pane really showing it. Without that control the
    /// absence below is *the stand-in never came up* and would be green for a fixture that typed
    /// nothing anywhere.
    ///
    /// # ⛔⛔⛔⛔⛔ WHAT THIS GATE MEASURED THAT NOBODY HAD ASKED — the datamodel does not cross
    ///
    /// The first draft asserted a resumed pass delivers NO prompt, and measured **one delivery of
    /// one byte** where a fresh pass delivers 259. Both halves of that are findings:
    ///
    /// * *one delivery* is CORRECT and the draft's claim was wrong — a resumed loop carrying on
    ///   from `judging` moves to `working`, and putting the NEXT turn's prompt is the whole point.
    /// * *one byte* is a **defect this brick does not fix**: the prompt it delivered is EMPTY. The
    ///   words a working prompt is composed from are assigned by entry actions, and `enter_at`
    ///   deliberately does not re-run them — so a machine put back without its datamodel asks its
    ///   peer a blank question.
    ///
    /// Item 543's own scope names both halves (*"the active configuration **plus** the
    /// datamodel"*). This brick pays the first. The second is held by the `#[ignore]`d gate below,
    /// which is red on purpose and names what will make it green.
    #[test]
    fn a_resumed_loop_is_placed_rather_than_walked_and_does_not_re_open_with_its_prompt() {
        let run = RunContext::uncancellable();

        // ── WHERE A RUN GOT TO: drive one past its opening prompt and keep its place ─────────
        let (worked_in, worked) = standin_agent(2);
        let working = supervised(&worked_in);
        let mut ran = AiLoop::new(engine(), worked, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let mut passes = 0;
        while ran.state() != AiLoopState::Judging && passes < 40 {
            ran.step(&working, &run).expect("a live pane takes a pass");
            passes += 1;
        }
        assert_eq!(
            ran.state(),
            AiLoopState::Judging,
            "⚠⚠ THE FIXTURE'S PRECONDITION: this loop must get past its opening prompt within \
             {passes} passes, or the place saved below is one where typing is still owed and the \
             claim measures nothing.",
        );
        let saved = ran.place().expect("a loop says where its machine is");
        working.lifecycle().expect("lifecycle").close(worked);

        // ── THE CONTROL: a FRESH loop opens with its prompt, and the pane SHOWS it ───────────
        //
        // ⚠ The needle is the brief's own north star, so a gate renaming nothing still holds when
        // the document rewords the prompt around it.
        let needle = brief_for(40).north_star;
        let (fresh_in, fresh_pane) = standin_agent(2);
        let freshly = supervised(&fresh_in);
        let mut fresh = AiLoop::new(engine(), fresh_pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        let typed = fresh
            .step(&freshly, &run)
            .expect("a live pane takes a pass")
            .cost
            .amount();
        let opened_with = screen_showing(&freshly, fresh_pane, &needle);
        assert!(
            opened_with.contains(&needle),
            "⚠⚠⚠ THE CONTROL FAILED: a loop starting at the top never put its opening words into \
             its pane ({typed} bytes typed), so the ABSENCE the claim below asserts would be true \
             of this whole fixture and would say nothing about resuming. Screen: {opened_with:?}",
        );
        freshly.lifecycle().expect("lifecycle").close(fresh_pane);

        // ── THE CLAIM: put back, and neither walked in nor re-opening ────────────────────────
        let (resumed_in, resumed_pane) = standin_agent(2);
        let resuming = supervised(&resumed_in);
        let mut resumed = AiLoop::new(engine(), resumed_pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        assert_eq!(
            resumed.resume_at(&saved),
            Resumption::Placed,
            "⚠⚠ the place saved above must be one this build accepts, or the pass below is a \
             fresh loop's and the claim is measuring the control twice",
        );
        assert!(
            resumed.walked().is_empty(),
            "⛔⛔⛔⛔ REGISTER ITEM 543: the loop arrived by WALKING. A machine driven back to a \
             place fires every `<onentry>` on the way, which is what types a prompt — so a resume \
             that walks is a second run wearing the first one's id however right its state name \
             looks. Walked: {:?}",
            resumed.walked(),
        );
        let spent = resumed
            .step(&resuming, &run)
            .expect("a live pane takes a pass")
            .cost
            .amount();
        let after = screen_showing(&resuming, resumed_pane, &needle);
        assert!(
            !after.contains(&needle),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a RESUMED run put its OPENING words to the peer again \
             ({spent} bytes, where a fresh pass types {typed}). The damage is not a wasted call: \
             the answer to that question is already on the screen the run was resumed over, so it \
             pays the peer to say what was already said and then judges the echo. Screen: {after:?}",
        );
        resuming.lifecycle().expect("lifecycle").close(resumed_pane);
    }

    /// ⛔⛔⛔⛔⛔ **AND THE PROMPT A RESUMED LOOP PUTS IS A REAL ONE** — register item 543's fifth
    /// brick, end to end through a live pane.
    ///
    /// # ⚠⚠⚠⚠⚠ What it measures, and why the byte count alone was not enough
    ///
    /// The gate above resumes a loop and requires it not to re-type its OPENING words. That was
    /// paid, and it left the other half exposed: a machine placed by `enter_at` has not run the
    /// entry actions that COMPOSE its prompts, so before the datamodel crossed the log a resumed
    /// loop's very first delivery was ONE BYTE — an empty question, and its peer answers something
    /// the run then judges as though it were about the work. **A blank prompt is worse than a
    /// re-typed one**, and worse than the honest `interrupted` a restart reports today.
    ///
    /// So this asks for the bytes AND for the words. `turn_prompt` is `<assign>`ed as
    /// `'Continue toward: ' + milestone + …` by an entry action, which makes the brief's own
    /// milestone a needle that can only reach that pane through the run log: the resuming loop was
    /// built from the same document and briefed with the same brief, and it still could not
    /// compose that sentence, because composing it is the writing `enter_at` skips.
    ///
    /// ⚠⚠ **THE CONTROL IS THAT IT DELIVERED AT ALL.** A run that put nothing to its peer would
    /// satisfy *the prompt was not empty* vacuously, which is exactly what a resume that refused to
    /// start looks like from out here.
    #[test]
    fn a_resumed_loop_composes_a_real_prompt_and_not_an_empty_one() {
        let run = RunContext::uncancellable();
        let (worked_in, worked) = standin_agent(2);
        let working = supervised(&worked_in);
        let mut ran = AiLoop::new(engine(), worked, &brief_for(40), &standin_spec())
            .expect("a well-briefed loop over a live pane starts");
        let mut passes = 0;
        while ran.state() != AiLoopState::Judging && passes < 40 {
            ran.step(&working, &run).expect("a live pane takes a pass");
            passes += 1;
        }
        assert_eq!(
            ran.state(),
            AiLoopState::Judging,
            "⚠⚠ THE FIXTURE'S PRECONDITION: the run must get past its opening prompt within \
             {passes} passes, or the place saved below is one where nothing has been composed yet \
             and the claim measures nothing.",
        );
        let saved = ran.place().expect("a loop says where its machine is");
        working.lifecycle().expect("lifecycle").close(worked);

        // ⚠ THE DOCUMENT'S OWN COMPOSITION, not a sentence spelled here: whatever a turn prompt is
        // reworded to say, it opens by naming the milestone it is continuing toward.
        let needle = format!("Continue toward: {}", brief_for(40).milestone);

        let (resumed_in, resumed_pane) = standin_agent(2);
        let resuming = supervised(&resumed_in);
        let mut resumed = AiLoop::new(engine(), resumed_pane, &brief_for(40), &standin_spec())
            .expect("a loop builds from the same document");
        assert_eq!(
            resumed.resume_at(&saved),
            Resumption::Placed,
            "⚠⚠ the place saved above must be one this build accepts, or what follows is a fresh \
             loop's opening and says nothing about resuming",
        );
        let spent = resumed
            .step(&resuming, &run)
            .expect("a live pane takes a pass")
            .cost
            .amount();
        let made = resumed.deliveries().made;
        let after = screen_showing(&resuming, resumed_pane, &needle);
        resuming.lifecycle().expect("lifecycle").close(resumed_pane);

        // ── THE CONTROL: it put something to its peer ───────────────────────────────────────
        assert!(
            made >= 1,
            "⚠⚠⚠ THE CONTROL FAILED: the resumed loop delivered nothing at all, so *the prompt it \
             put was not empty* is true of a run that never spoke. {spent} byte(s) typed.",
        );

        // ── THE CLAIM: a real question, in the run's own words ──────────────────────────────
        assert!(
            spent > 1,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a resumed loop delivered {made} prompt(s) worth {spent} \
             byte(s) — an EMPTY question. `enter_at` does not re-run the entry actions that compose \
             a prompt, so a machine put back without the datamodel they wrote has its place and \
             none of its words.",
        );
        assert!(
            after.contains(&needle),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: the resumed loop typed {spent} byte(s) that are not its \
             own question. `{needle}` is composed by an entry action this resume deliberately did \
             not run, so it can only have reached this pane by crossing the run log — and it did \
             not. Screen: {after:?}",
        );
    }
}
