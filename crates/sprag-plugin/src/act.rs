//! **THE ACTS THE LOOP'S DOCUMENT DECLARES AND THIS HOST CARRIES OUT** — register item 470,
//! stage 2.
//!
//! # The line this module draws
//!
//! SCXML is designed not to perform I/O, so item 470's test was never *Rust versus document*:
//!
//! > **DECISIONS in the document, EFFECTS in the host.** Can a reader say what this loop DOES from
//! > `ai_loop.scxml` alone?
//!
//! Typing bytes at a pane is an EFFECT and stays here. *"the sentence this state puts to the peer
//! is asking it to account for the run rather than to do more work"* is a DECISION, and until this
//! module existed it was a twenty-eight-arm Rust table keyed by the document's own states —
//! `Owed::asked_for_an_account`, a second copy of the topology, which is the shape item 470
//! measured.
//!
//! # How an act leaves the document
//!
//! W3C SCXML 6.2.5 makes a `<send>`'s `type` an extensible identifier, and SCE `e0fdd46b` opened
//! the other half: a host DECLARES the types it serves at build time (`build.rs`'s `HOST_TYPES`)
//! and REGISTERS a handler for each at run time. Both halves are required — a declared type nobody
//! registered raises `error.execution` exactly as an undeclared one does, which is right, because
//! from the document's side an act nobody performed is one fact.
//!
//! So the document says WHAT and WITH WHAT:
//!
//! ```xml
//! <onentry>
//!   <send type="x-sprag-host" event="prompt.say">
//!     <param name="text" expr="end_prompt"/>
//!     <param name="asks" expr="'account'"/>
//!   </send>
//! </onentry>
//! ```
//!
//! and this module answers WHO — [`Serving`] records the act, and the driver carries it out on the
//! pass that follows. ⚠ The handler cannot perform the effect itself: it is called from inside the
//! engine's own `<onentry>` execution, with the engine mutably borrowed, and a pane is not
//! reachable from there. An act is therefore RECORDED here and PERFORMED by
//! [`crate::outer::OuterLoop`], which is the same request/reply shape `probe.rs` measured.
//!
//! # ⚠⚠⚠⚠⚠ Why an act nobody serves is REFUSED rather than ignored
//!
//! This is the failure item 470 named and [`crate::document`] measured: an act that quietly does
//! nothing is indistinguishable from one that worked. A mutation put one unserved-type `<send>` in
//! `priming` and a real run walked into `working` and then took **eleven** eventless passes, going
//! nowhere, with every other gate in this crate green.
//!
//! So [`Serving`] answers an act it does not perform with `error.execution` — the same event W3C
//! SCXML 6.2 gives an unsupported `type`, because it is the same fact one level in — and
//! `ai_loop.scxml` already answers that on its `work` region by ending the run `failed` with the
//! error's name in the account. The refusal is also kept HERE, named, because the document's own
//! `fault` records the event and cannot say which act it was.
//!
//! ⚠⚠ **AND AN ARGUMENT OUTSIDE ITS VALUE SPACE IS REFUSED ON THE SAME TERMS.** That is not
//! generosity about spelling: it is what replaces the compiler. The Rust table this module retired
//! was EXHAUSTIVE on purpose — *"a future state that asks its agent for something and forgets to
//! say so here would publish NOTHING and look exactly like a state whose turn was work; a variant
//! that no longer compiles is the only thing that catches it."* A document cannot be made to fail
//! to compile, so the closed value space plus a refusal is the guard that stands in its place.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use sce_rust_runtime::{Engine, StatePolicy};

/// **THE EVENT I/O PROCESSOR TYPE THIS CRATE SERVES** — W3C SCXML 6.2.5.
///
/// ⚠ It must equal the type `build.rs` declares to codegen. A registration for a type the build
/// did not declare is inert by design: the generated send site emits a refusal instead of a
/// dispatch, and nothing here is ever called.
pub const HOST: &str = "x-sprag-host";

/// The event this host answers an act it will not perform with.
///
/// W3C SCXML 6.2 gives a `type` the platform does not support `error.execution`; an ACT the
/// platform does not perform is the same fact one level in, so it gets the same word. See this
/// module's own documentation, and `ai_loop.scxml`'s `work` region, which is what answers it.
const REFUSED: &str = "error.execution";

/// **AN ACT A DOCUMENT MAY ASK THIS HOST TO PERFORM.**
///
/// ⚠ The variants are the vocabulary and the `<send event="…">` names are how a document reaches
/// them. There is deliberately no catch-all: an act this list does not name is one nobody serves,
/// and [`Serving`] refuses it rather than dropping it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// `prompt.say` — put a sentence to the run's peer, and open a turn with it.
    ///
    /// Arguments: `text` (what to say) and `asks` (what the sentence is asking the peer for — see
    /// [`Asks`]). Both are required, because a prompt with neither is not a prompt.
    Say,
    /// `pass.do` — carry out what this pass of the driver is for.
    ///
    /// Argument: `does` — which effect, from the closed space [`Does`] names. Required: an act that
    /// does not say what it is is one this host would perform as a shrug.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this act is declared on a TRANSITION and never on a state entry
    ///
    /// Register item 470, stage 3. The other act moves a SENTENCE, which a state says once on the
    /// way in; this one moves *what the driver does while it is here*, and a state is looked at
    /// many times over one entry — `working` is pumped for as long as its peer keeps working. So
    /// the document answers it on `<transition event="pass">`, W3C SCXML 3.13's transition with no
    /// `target`: the driver asks on every pass, the machine does not move, and no `<onentry>`
    /// re-runs. `probe.rs`'s
    /// `a_transition_can_ask_this_host_for_an_act_and_its_arguments_reach_it` is what measured that
    /// road, re-entry counter and all, before any of this was written.
    ///
    /// ⚠⚠ **AND A STATE THAT ANSWERS NOTHING IS NOT A STATE THAT DOES NOTHING.** The driver reports
    /// `Pumped::Unbuilt` — *this driver has no act for what it is looking at* — rather than
    /// carrying on, because a pass that silently did nothing is the silence this whole module
    /// exists to end.
    Pass,
    /// `end.publish` — this ending publishes this word to whoever is running the loop.
    ///
    /// Argument: `publishes` — which ending, from the closed space [`Publishes`] names.
    ///
    /// # ⚠⚠⚠⚠⚠ Why an ENDING declares an act at all, when nothing is left to do
    ///
    /// Register item 470, stage 3. `AiLoop::ended` mapped each of the loop's seven endings to a
    /// `Verdict` with a `match` over all twenty-eight states — the same second copy of the topology
    /// as every other match this stage has retired, and keyed on ids written in a file the driver
    /// does not parse. The EFFECT (building a verdict, reading the run's own ceilings and panes for
    /// its payload) stays here; what moves is the DECISION *which word does this ending publish*.
    ///
    /// # ⚠⚠⚠⚠ Why this one is READ rather than TAKEN, unlike the other two
    ///
    /// An ending is entered ONCE and asked about MANY TIMES. `OuterLoop::pumping` answers
    /// `Pumped::Ended` at the top of every pass after the machine completes, and each of those
    /// passes needs the same word — so a slot emptied by the first reader would leave every later
    /// one with nothing, on a run that has not changed. The other two acts are performed once and
    /// must be taken; this one is a FACT ABOUT THE RUN and is read, on `Serving::refused`'s terms.
    ///
    /// ⚠ Which is also why no `Overrun` can arise from it in a real run: one ending is entered, so
    /// the slot is written once. A document declaring a second would be refused exactly as the
    /// other acts are, which is the arrangement rather than an exception to it.
    End,
    /// `account.ask` — whether this state's agent can be asked where the run got to.
    ///
    /// Argument: `can` — which answer, from the closed space [`Accounts`] names.
    ///
    /// # ⚠⚠⚠⚠⚠ Declared on a TARGETLESS transition, and never on a state entry
    ///
    /// Register item 470, stage 3. This is [`Act::Pass`]'s shape for [`Act::Pass`]'s reason: the
    /// question is asked at a moment nobody can predict — when one of the RUN's ceilings falls due
    /// — and the state it is asked about may have been entered long before. A state entry could
    /// only answer it once, at a time that has nothing to do with the asking.
    ///
    /// # ⚠⚠ What it does NOT carry, and why that is the line item 470 draws
    ///
    /// Not the window. A run that CAN be asked gets two of its caller's own turns, and that number
    /// is the caller's (`turn_within_ms`, or the substrate's published default) — a quantity this
    /// document neither knows nor should. The document says *whether, and if not why not*; the
    /// driver prices it.
    ///
    /// ⚠ Nor the ENDINGS: a finished machine is told apart by having published one
    /// ([`Serving::published`]), not by naming seven states here. Only the fifteen states a driver
    /// actually drives declare this.
    Account,
    /// `arrival.note` — which of the run's own readings belongs on the arrow this pass just drew.
    ///
    /// Argument: `note` — which reading, from the closed space [`Notes`] names.
    ///
    /// # ⚠⚠⚠⚠⚠ It is the ARROW's word, and it is the last of item 470's copies
    ///
    /// Register item 470, stage 3. `OuterLoop::pumping` chose it from a `match` over all
    /// twenty-eight states: six naming a reading and twenty-two written out to say *this arrow
    /// needs no word*. `Because` exists for doors an arrow cannot tell apart — a state reached from
    /// several places, for reasons that disagree about the remedy — so which reading applies is a
    /// property of the DOOR, and a door is a thing the document has and the driver does not.
    ///
    /// # ⚠⚠⚠⚠ Why one of the six is declared on an EDGE and the other five on `In(…)`
    ///
    /// Five of them are answered by WHERE THE PASS ARRIVED: every door into `reflecting`,
    /// `stopping`, `closing` and `restarting` wants that state's own reading, and `disputing` has
    /// exactly one door. Those are `In(…)` questions, asked the way `pass.do` is asked.
    ///
    /// ⚠⚠ **`working` is not**, and it is the arm the old match needed an `if from ==` for.
    /// `working` is entered from four states and only a JUDGEMENT has a reading behind it; the
    /// other three doors would be handed a value belonging to some earlier turn. So the two
    /// `judging -> working` edges declare it **on themselves**, which is the document saying *this
    /// door* rather than the driver reconstructing which door it must have been.
    ///
    /// ⚠ The READING stays with the driver: a ceiling, a reflect reason, a done reason, a restart
    /// reason and a judgement are five different Rust values it latched as it went. The document
    /// says WHICH of them the arrow wants.
    Note,
}

impl Act {
    /// Every act this host serves.
    ///
    /// ⚠ The one list. [`Act::of`] reads it rather than spelling a second `match`, so an act added
    /// to the enum is served the moment it names itself.
    pub const ALL: [Self; 5] = [Self::Say, Self::Pass, Self::End, Self::Account, Self::Note];

    /// The name a document calls this act by — its own `<send event="…">`.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Say => "prompt.say",
            Self::Pass => "pass.do",
            Self::End => "end.publish",
            Self::Account => "account.ask",
            Self::Note => "arrival.note",
        }
    }

    /// The act `name` asks for, or [`None`] for one nobody here serves.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|act| act.named() == name)
    }
}

/// **WHAT A SENTENCE PUT TO THE PEER IS ASKING IT FOR** — [`Act::Say`]'s `asks` argument.
///
/// # ⚠⚠⚠ Why a WORD with a closed space, and not a boolean
///
/// A boolean would answer *is this an account* and nothing else, and the next question the document
/// wants to ask about a prompt would arrive as a second boolean beside it — two flags for one fact,
/// which is the shape this register keeps paying for. A word names what the prompt IS, and a value
/// outside this space is REFUSED rather than read as `false`.
///
/// ⚠⚠⚠⚠⚠ **AND THE THIRD VALUE IS THAT ARGUMENT BEING USED RATHER THAN THE SPACE GROWING** —
/// 2026-08-26, `reflecting`'s act. While the space held two words a reader could still take it for
/// `asked_for_an_account`'s boolean under another spelling; [`Self::Direction`] is a question that
/// is neither of the other two and could not have been expressed as one, which is what the comment
/// above predicted the day the space was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asks {
    /// `work` — the ordinary turn: do the next piece of the job.
    Work,
    /// `account` — say where the run got to.
    ///
    /// ⚠⚠ It is a COURTESY TURN over a verdict already reached, and what makes it worth naming is
    /// that its answer is READ BACK: the run publishes it as the agent's account of itself. A turn
    /// asking for work produces no such record.
    Account,
    /// `direction` — say where the work should go NEXT.
    ///
    /// # ⚠⚠⚠⚠⚠ Why `reflecting` is neither of the two above, argued rather than assigned
    ///
    /// **It is not `work`.** `ai_loop.scxml` says so about its own state, in four separate
    /// sentences: the turn is not judged, it does not spend `max_turns`, it cannot converge a run,
    /// and the prompt asks the agent for an answer *from what you already have; do not use a tool*.
    /// A reflection is the loop asking about ITSELF, and the one argument the act carries would
    /// have said the opposite of the file that carries it.
    ///
    /// **It is not `account` either, and this is the one that would have been harmless today and
    /// wrong.** An account's defining property is above: the run PUBLISHES the answer as the
    /// agent's account of itself. A reflection's answer is read back too — by
    /// `OuterLoop::proposed`, into the milestone the REPLACEMENT session is briefed with — and the
    /// run carries on. Nothing collects a reflection as a report today only because `reflecting`
    /// has its own arm in the driver's `pump`, i.e. because of a decision keyed by STATE, which is
    /// the copy register item 470 exists to remove. Encoding the wrong word and relying on that arm
    /// to keep it harmless would leave the defect behind the very thing being taken away.
    ///
    /// ⚠ **WHAT IT NAMES IS THE DOCUMENT'S OWN SENTENCE FOR THE STATE**: *where the work should go
    /// next is a judgement about the work and this driver cannot make one*. So the loop asks the
    /// only party that can, and what comes back is a direction rather than progress or a report.
    Direction,
}

impl Asks {
    /// Every value this argument may hold.
    pub const ALL: [Self; 3] = [Self::Work, Self::Account, Self::Direction];

    /// The word a document writes for it.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Account => "account",
            Self::Direction => "direction",
        }
    }

    /// What `word` asks for, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|asks| asks.named() == word)
    }
}

/// **WHAT ONE PASS OF THE DRIVER IS FOR** — [`Act::Pass`]'s `does` argument.
///
/// # ⚠⚠⚠⚠⚠ What this space replaced, and why it is a space rather than a table
///
/// Register item 470, stage 3. `OuterLoop::pump` chose what to do from a `match` over all
/// twenty-eight states of `ai_loop.scxml` — *this state watches a turn, that one waits an outage
/// out, that one asks a person* — which is a SECOND COPY OF THE TOPOLOGY, decided in Rust, keyed by
/// the document's own state names. It is the largest thing item 470 was filed about.
///
/// Every word here names an EFFECT, and effects are the host's half of item 470's line: writing
/// bytes at a pty, parsing a screen, spawning a process. What moved is the DECISION — *which of
/// them does this state want* — which each state now says for itself. A state added to the document
/// costs this crate no Rust at all: it declares one of these words, and one that declares none is
/// refused where the machine can hear it.
///
/// ⚠⚠ **THE SPACE IS CLOSED AND A WORD OUTSIDE IT IS REFUSED**, for [`Asks`]'s reason exactly: the
/// match this replaced was exhaustive on purpose, a document cannot be made to fail to compile, and
/// a refusal is what stands in the compiler's place.
///
/// ⚠ **IT IS NOT ONE WORD PER STATE, AND THAT IS THE POINT.** Three states watch a turn
/// (`working`, `closing`, `stopping`) and two report a sentence already delivered (`priming`,
/// `disputing`) — so this space is smaller than the match it replaced, and the difference is
/// exactly the decisions that were being made twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    /// `ready` — look at the peer and say whether it can be spoken to yet.
    ///
    /// ⚠ The one word whose effect can decline to produce an event at all: a peer that is not ready
    /// leaves the machine where it is, because a prompt typed at a program that is still booting is
    /// read back off the pseudoterminal's own echo and called delivered (measured, R379).
    Ready,
    /// `sent` — the sentence this state was entered with has gone in; say so.
    ///
    /// ⚠⚠ It performs NOTHING, and that is the honest reading rather than a gap: the prompt was
    /// delivered by the act the entry declared, so what is left of the pass is to tell the machine.
    /// Two states use it and they are the two whose whole job is to have spoken.
    Sent,
    /// `watch` — watch the turn the peer is taking and say how it ended.
    Watch,
    /// `judge` — read the finished turn off the pane, have the claim checked, and report what was
    /// found.
    Judge,
    /// `screen` — match the dialog the peer has raised against the author's standing instructions.
    Screen,
    /// `wait` — the peer's service failed and the only treatment is time.
    Wait,
    /// `reflect` — watch the reflection turn and read the direction out of what it answered.
    ///
    /// ⚠ Distinct from [`Self::Watch`] because a reflection is not judged, does not spend the
    /// document's `max_turns`, and its answer is read back into the brief of the session that
    /// replaces this one — see [`Asks::Direction`], which is the sentence half of the same fact.
    Reflect,
    /// `review` — ask what the sessions this run has already closed did.
    Review,
    /// `replace` — put a fresh session in the seat.
    Replace,
    /// `resume` — set the replacement session going.
    Resume,
    /// `attend` — a person is expected; wait for one.
    Attend,
    /// `redirect` — the work needs pointing somewhere else.
    Redirect,
}

impl Does {
    /// Every effect a pass may be for.
    pub const ALL: [Self; 12] = [
        Self::Ready,
        Self::Sent,
        Self::Watch,
        Self::Judge,
        Self::Screen,
        Self::Wait,
        Self::Reflect,
        Self::Review,
        Self::Replace,
        Self::Resume,
        Self::Attend,
        Self::Redirect,
    ];

    /// The word a document writes for it.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Sent => "sent",
            Self::Watch => "watch",
            Self::Judge => "judge",
            Self::Screen => "screen",
            Self::Wait => "wait",
            Self::Reflect => "reflect",
            Self::Review => "review",
            Self::Replace => "replace",
            Self::Resume => "resume",
            Self::Attend => "attend",
            Self::Redirect => "redirect",
        }
    }

    /// What `word` asks a pass to do, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|does| does.named() == word)
    }
}

/// **WHICH ENDING A FINISHED RUN PUBLISHES** — [`Act::End`]'s `publishes` argument.
///
/// # ⚠⚠⚠⚠⚠ What this space replaced
///
/// Register item 470, stage 3. `AiLoop::ended` chose a `Verdict` from a `match` over all
/// twenty-eight states of `ai_loop.scxml`: seven arms naming an ending and twenty-one written out
/// to say *not an ending* so the match stayed exhaustive without a wildcard. Every ending now says
/// its own word on its own `<onentry>`, and the driver matches over THIS space — a vocabulary of
/// seven outcomes, not a copy of a topology.
///
/// # ⚠⚠⚠⚠ The word moves and the PAYLOAD does not, which is the line item 470 draws
///
/// A verdict's payload is a fact about the RUN, not about the document: which ceiling fell
/// (`Exhausted`), what question was left on the pane (`Blocked`), which pane the dead peer had
/// (`PeerGone`). The driver latched those as it went and is the only thing that holds them, so it
/// keeps building them. What it stops doing is deciding, from a state's NAME, which of the seven
/// this is.
///
/// ⚠⚠ **THE SPACE IS CLOSED AND A WORD OUTSIDE IT IS REFUSED**, for [`Asks`]'s and [`Does`]'s
/// reason exactly: the match this replaced was exhaustive on purpose, a document cannot be made to
/// fail to compile, and a refusal is what stands in the compiler's place.
///
/// ⚠ **IT IS ONE WORD PER ENDING HERE, unlike [`Does`]**, and that is a property of this document
/// rather than a rule: the seven endings are seven different things to tell a caller. If two ever
/// published the same word, this space would shrink and the finals would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publishes {
    /// `converged` — the agent said the word, and the report landed.
    Converged,
    /// `exhausted` — a budget ran out. WHICH budget is the run's own fact and stays with the driver.
    Exhausted,
    /// `failed` — the document's own content raised an error, and this document stops on that.
    Failed,
    /// `cancelled` — the run was ended from outside.
    ///
    /// ⚠ The driver still answers this one `Verdict::Continue`, deliberately: only the Driver can
    /// tell a person's stop from a deadline, and that is an EFFECT decision rather than the word.
    Cancelled,
    /// `blocked` — the run stopped and a person is what it needs.
    Blocked,
    /// `peer_gone` — the program this run was driving has exited.
    PeerGone,
    /// `abandoned` — a person held the loop and did not come back inside its own bound.
    ///
    /// ⚠⚠ The one ending the `orders` region reaches on its own, which is why it is here rather
    /// than folded into [`Self::Blocked`]: both mean *a person is what this run needs*, and only
    /// this one means a person was already there and left.
    Abandoned,
}

impl Publishes {
    /// Every ending this host publishes a word for.
    pub const ALL: [Self; 7] = [
        Self::Converged,
        Self::Exhausted,
        Self::Failed,
        Self::Cancelled,
        Self::Blocked,
        Self::PeerGone,
        Self::Abandoned,
    ];

    /// The word a document publishes this ending as.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Converged => "converged",
            Self::Exhausted => "exhausted",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Blocked => "blocked",
            Self::PeerGone => "peer_gone",
            Self::Abandoned => "abandoned",
        }
    }

    /// Which ending `word` publishes, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ends| ends.named() == word)
    }
}

/// **WHAT A STOP WOULD STILL HAVE TO REACH AT AN ENDING** — [`Act::End`]'s `signals` argument.
///
/// # ⚠⚠⚠⚠⚠ What this space replaced, and why the answer is an ENDING's to give
///
/// Register item 470, stage 3. `AiLoop::driving` answered *which pane would a stop have to signal*
/// with a `match` over all twenty-eight states: **three arms answering `None` and twenty-five
/// answering the pane**. All three of the three are ENDINGS — `converged`, `exhausted`, `peer_gone`
/// — so the twenty-five were not a decision at all, they were one fact (*a run in flight has a
/// pane*) written out twenty-five times to keep the match exhaustive.
///
/// So the question is only ever asked about an ending, and it rides the act an ending already
/// declares rather than needing one of its own — the way `prompt.say` carries `text` AND `asks`.
///
/// # ⚠⚠⚠⚠ Why a word and not a boolean, when there are two of them
///
/// [`Asks`]'s reason, and it is sharper here. The question is not *is a model busy* but **what
/// would a stop have to reach**, and the answers name things: the pane this run is driving, or
/// nothing. A third answer is expressible in that vocabulary and is not in a flag's — an ending
/// whose stop belongs somewhere other than the pane the loop is driving. A boolean would have to be
/// re-read as a different question the day that arrives.
///
/// ⚠⚠ **AND THE DIRECTION THAT FAILS SAFE IS [`Self::Pane`]**, which is why this space is closed
/// and a word outside it is refused rather than defaulted: a needless interrupt costs a peer one
/// keystroke it was waiting at anyway, and a missed one leaves a model spending somebody's tokens
/// on a question nothing is waiting for. A default of `Nothing` would buy the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signals {
    /// `pane` — the pane this loop is driving, which a stop must reach.
    ///
    /// ⚠ The pane is read off the loop at the moment of asking and never carried here: a run
    /// REPLACES its inner session as it goes, so a pane id travelling in a `<param>` would name a
    /// closed pane. What the document decides is *whether there is one to reach*; which one it is
    /// is the driver's own moving fact.
    Pane,
    /// `nothing` — no model can be mid-turn at this ending, so a stop has nothing to reach.
    ///
    /// ⚠⚠ Declared by exactly three of the seven endings, and each has its own argument. Two are
    /// arguments that the peer is AT REST because a turn completed (`converged`, `exhausted`); the
    /// third is the one case where the product has LOOKED — `peer_gone` is only reached because the
    /// pane's child was seen to have exited.
    Nothing,
}

impl Signals {
    /// Every answer an ending may give about what a stop would reach.
    pub const ALL: [Self; 2] = [Self::Pane, Self::Nothing];

    /// The word a document says this answer with.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Nothing => "nothing",
        }
    }

    /// What `word` says a stop would reach, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|signals| signals.named() == word)
    }
}

/// **WHETHER THIS STATE'S AGENT CAN BE ASKED WHERE THE RUN GOT TO** — [`Act::Account`]'s `can`
/// argument.
///
/// # ⚠⚠⚠⚠⚠ What this space replaced
///
/// Register item 470, stage 3. `AiLoop::ask_for_an_account` answered it with a `match` over all
/// twenty-eight states of `ai_loop.scxml`: eight granting a window and twenty naming a reason not
/// to.
///
/// # ⚠⚠⚠⚠ Each word is a fact about the PANE, not a policy
///
/// The refusals are not a ranking of how important the states are. Each names something true about
/// the pane that makes the question unaskable — nobody was ever asked anything, somebody else's
/// hand is in it, the agent that did the work is being replaced, the service is not answering. A
/// caller reading a run's journal gets the reason, and *"no account"* alone would have sent them to
/// look at the wrong thing.
///
/// ⚠⚠ **THE WINDOW IS NOT IN THIS SPACE.** [`Self::Within`] says the agent CAN be asked; how long
/// it gets is two of the caller's own turns, a number this document neither holds nor should. The
/// word moves, the quantity stays with the driver — the same line every act in this module draws.
///
/// ⚠ **AND NEITHER ARE THE ENDINGS.** A finished machine is told apart by having published one
/// ([`Serving::published`]) rather than by seven more words here, so this space covers only the
/// states a driver actually drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accounts {
    /// `within` — a turn is in flight or one has just landed, so the agent can be asked.
    Within,
    /// `never_asked` — the loop never got its pane, so its agent was asked nothing to account for.
    NeverAsked,
    /// `not_ours` — the pane is showing somebody else's question, or somebody is typing in it.
    ///
    /// ⚠⚠ Asking here would ANSWER that dialog or type under a hand, which is the one thing this
    /// driver must never do — the same refusal `screen.none` and `turn.interrupted` already make.
    NotOurs,
    /// `between_sessions` — the agent that did the work is being replaced and its successor has
    /// done none of it.
    BetweenSessions,
    /// `service_down` — the agent's service was not answering, so the run was waiting an outage out.
    ///
    /// ⚠ Its own word rather than folded into [`Self::NotOurs`]: typing here is ALLOWED — nobody's
    /// hand is in the pane — and it would still buy nothing, because the answer would have to come
    /// back from the same service that just refused a turn. What a reader needs is the outage.
    ServiceDown,
}

impl Accounts {
    /// Every answer a driven state may give about being asked for an account.
    pub const ALL: [Self; 5] = [
        Self::Within,
        Self::NeverAsked,
        Self::NotOurs,
        Self::BetweenSessions,
        Self::ServiceDown,
    ];

    /// The word a document says this answer with.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Within => "within",
            Self::NeverAsked => "never_asked",
            Self::NotOurs => "not_ours",
            Self::BetweenSessions => "between_sessions",
            Self::ServiceDown => "service_down",
        }
    }

    /// What `word` answers, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|can| can.named() == word)
    }
}

/// **WHICH OF THE RUN'S OWN READINGS BELONGS ON THIS ARROW** — [`Act::Note`]'s `note` argument.
///
/// # ⚠⚠⚠⚠⚠ What this space replaced, and what a `Because` is for
///
/// Register item 470, stage 3, the last of the seven. `OuterLoop::pumping` chose it from a `match`
/// over all twenty-eight states: six naming a reading and twenty-two written out to say *this arrow
/// needs no word*. A `Because` exists **only for doors an arrow cannot tell apart** — a state
/// reached from several places for reasons that disagree about the remedy — so a one-doored state
/// answers nothing and is right to, because `From --Event--> To` is already the whole story.
///
/// ⚠⚠ **THE READING STAYS WITH THE DRIVER, AND THAT IS THE LINE.** A ceiling, a reflect reason, a
/// done reason, a restart reason and a judgement are five different Rust values the driver latched
/// as it went. What moves is *which of them this arrow wants*, which is a property of the door.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notes {
    /// `reflected` — why the run stopped to reflect.
    Reflected,
    /// `stopped` — which ceiling sent the machine to `stopping`.
    ///
    /// ⚠⚠ Four ceilings arrive on two edges, and a reader could otherwise tell them apart only by
    /// whether the Driver's own note PRECEDED the arrow — i.e. by the absence of a key.
    Stopped,
    /// `closed` — which of the two runs through one transition this was.
    ///
    /// ⚠ A many-doored state whose doors are not `<transition>`s at all but two returns in the
    /// driver's own `reflect`, which is why counting the document's edges would not have found it.
    Closed,
    /// `restarted` — which of three disagreeing reasons bought the replacement.
    Restarted,
    /// `judged` — what the judgement that opened this turn saw.
    ///
    /// ⚠⚠⚠ The one word declared on EDGES rather than by arrival: `working` is entered from four
    /// states and only a judgement has a reading behind it, so the other three doors would be
    /// handed a value belonging to some earlier turn. `disputing` carries the same word by arrival
    /// instead, because exactly one edge reaches it and that edge comes from `judging`.
    Judged,
}

impl Notes {
    /// Every reading an arrival may ask for.
    pub const ALL: [Self; 5] = [
        Self::Reflected,
        Self::Stopped,
        Self::Closed,
        Self::Restarted,
        Self::Judged,
    ];

    /// The word a document names this reading with.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Reflected => "reflected",
            Self::Stopped => "stopped",
            Self::Closed => "closed",
            Self::Restarted => "restarted",
            Self::Judged => "judged",
        }
    }

    /// Which reading `word` names, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|note| note.named() == word)
    }
}

/// **ONE ACT THE DOCUMENT ASKED FOR, WITH THE ARGUMENTS IT SENT.**
///
/// ⚠ A variant per act rather than one struct with every act's arguments on it: the two acts share
/// no argument, and a struct would have to hold each of them as *present for one act and meaningless
/// for the other* — a shape where the wrong reader gets an answer instead of a refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// [`Act::Say`] — put this sentence to the peer.
    Say {
        /// The sentence to put to the peer, as the document composed it.
        text: String,
        /// What that sentence is asking for.
        asks: Asks,
    },
    /// [`Act::Pass`] — carry this pass out.
    Pass {
        /// Which effect the pass is for.
        does: Does,
        /// **WHAT THE SCREEN SAYS WHEN THE PEER'S SERVICE FAILED**, where this pass is one that
        /// could meet it — see [`Does::Watch`].
        ///
        /// # ⚠⚠⚠⚠⚠ Why the document HANDS this over instead of the driver FETCHING it
        ///
        /// Register item 470, stage 2's other half. The driver used to read `service_needle`
        /// straight out of the script session with a private `text_of` — *behind the machine's
        /// back*, in the register's own words. Nothing in `ai_loop.scxml` said the value was
        /// consulted, so a reader of the document could not tell that a blocked turn is matched
        /// against it at all. It now rides the `watch` pass, where the matching happens.
        ///
        /// ⚠⚠ [`None`] IS A REAL ANSWER AND NOT A MISSING ONE: only the passes that WATCH a turn
        /// can meet an outage, so every other `pass.do` declares no needle and this is `None` for
        /// them. An empty string is also a real answer — the template ships one, and it declines
        /// the whole behaviour.
        needle: Option<String>,
    },
    /// [`Act::End`] — publish this ending.
    End {
        /// Which ending the run reached, as the document publishes it.
        publishes: Publishes,
        /// What a stop would still have to reach at this ending.
        signals: Signals,
    },
    /// [`Act::Account`] — whether this state's agent can be asked for an account.
    Account {
        /// Which answer, and where it is a refusal, which one.
        can: Accounts,
    },
    /// [`Act::Note`] — which reading this arrow wants.
    Note {
        /// The reading.
        note: Notes,
    },
}

impl Asked {
    /// Which act this is.
    ///
    /// ⚠ Derived rather than carried beside the arguments: an act and its arguments that could
    /// disagree are two authorities on one fact, which is the shape this register keeps paying for.
    #[must_use]
    pub const fn act(&self) -> Act {
        match self {
            Self::Say { .. } => Act::Say,
            Self::Pass { .. } => Act::Pass,
            Self::End { .. } => Act::End,
            Self::Account { .. } => Act::Account,
            Self::Note { .. } => Act::Note,
        }
    }
}

/// **WHY THIS HOST WOULD NOT PERFORM AN ACT.**
///
/// ⚠ Kept as a value rather than only sent back as an event, because the event cannot say which:
/// `error.execution` is one word, and a document that ends `failed` on it records the word and not
/// the act. See [`Serving::refused`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The document named an act nobody here serves.
    Unserved {
        /// The `<send event="…">` as the document wrote it.
        named: String,
    },
    /// The act needs an argument the send did not carry.
    Missing {
        /// The act.
        act: Act,
        /// The `<param name="…">` that was owed.
        argument: &'static str,
    },
    /// The act's argument carried a value its space does not hold.
    Unreadable {
        /// The act.
        act: Act,
        /// The `<param name="…">` that was outside its space.
        argument: &'static str,
        /// What the document said, so a reader repairs the file rather than guessing.
        said: String,
        /// Every word the space DOES hold, carried from the site that knows which space it was.
        ///
        /// ⚠⚠ Carried rather than looked up where the sentence is written, and the reason is that
        /// two acts now have closed-space arguments: a formatter deciding which vocabulary to
        /// print from the act and the argument name would need a fallback for the pairing nobody
        /// builds, and a fallback that prints an EMPTY space is a refusal that tells a reader
        /// their word was outside a space with nothing in it.
        holds: Vec<&'static str>,
    },
    /// The act's argument arrived EMPTY, which is a value its space does not hold.
    ///
    /// # ⚠⚠⚠⚠⚠ Separate from [`Self::Missing`] because the REPAIR is separate
    ///
    /// A missing `<param>` is a document that never wrote one. An empty `<param>` is a document
    /// that wrote an expression which EVALUATED to nothing — so the two send a reader to different
    /// files, and folding them would name the wrong one.
    ///
    /// # ⚠⚠⚠⚠ And it is not a nicety — measured 2026-08-25, register item 470, stage 2
    ///
    /// `say` with an empty `text` types a BARE SUBMIT at the peer: a turn nobody was asked to take,
    /// which is the exact fault item 446 spent four rounds on. Nothing produced one while the
    /// driver looked the sentence up itself — the driver's own `Authored::read` refuses a machine
    /// that cannot answer, at construction. ⚠ Named rather than linked: it is crate-private, and a
    /// public doc that links a private item does not build. **The moment the sentence travelled as
    /// `<param expr="start_prompt"/>` instead, a datamodel that had stopped answering produced it
    /// on the spot**, and this host performed it: one byte delivered, reported as a turn.
    ///
    /// ⚠ Found by `outer::tests::a_datamodel_that_stops_answering_refuses_the_loop_or_fails_the_run`
    /// going red, which is the gate doing the job it was written for one architecture earlier.
    Empty {
        /// The act.
        act: Act,
        /// The `<param name="…">` that carried nothing.
        argument: &'static str,
    },
    /// A second act arrived while one nobody had carried out was still waiting.
    ///
    /// ⚠⚠⚠ **REFUSED RATHER THAN QUEUED, AND REFUSED RATHER THAN OVERWRITTEN.** This host performs
    /// one act per pass of the driver, so a document declaring two would have one of them silently
    /// not happen — a sentence nobody said, in a run that looks exactly like one with less to say.
    /// The FIRST is kept and the second is refused, because the first is the one the document
    /// asked for earlier and the run has already been shaped by everything before it.
    Overrun {
        /// The act still waiting to be carried out.
        held: Act,
        /// The act that arrived on top of it.
        arriving: Act,
    },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unserved { named } => write!(
                f,
                "`<send type=\"{HOST}\" event=\"{named}\">` names an act this host does not \
                 perform; it serves {:?}",
                Act::ALL.map(Act::named),
            ),
            Self::Missing { act, argument } => write!(
                f,
                "`{}` needs a `<param name=\"{argument}\">` and this send carried none",
                act.named(),
            ),
            Self::Unreadable {
                act,
                argument,
                said,
                holds,
            } => write!(
                f,
                "`{}`'s `<param name=\"{argument}\">` said {said:?}, which is not one of {holds:?}",
                act.named(),
            ),
            Self::Empty { act, argument } => write!(
                f,
                "`{}`'s `<param name=\"{argument}\">` evaluated to nothing, and an act with no \
                 {argument} is one this host would perform as silence",
                act.named(),
            ),
            Self::Overrun { held, arriving } => write!(
                f,
                "`{}` was declared while `{}` was still waiting to be carried out, and this host \
                 performs one act per pass",
                arriving.named(),
                held.named(),
            ),
        }
    }
}

/// What a host act's arguments arrive as — SCE keeps every value of a repeated `<param>` name.
type Params = HashMap<String, Vec<String>>;

/// **WHAT THIS HOST SERVES, AND WHAT ITS DOCUMENT HAS ASKED FOR** — shared with the engine that
/// dispatches to it.
///
/// # ⚠⚠ Why the record is shared rather than returned
///
/// The handler runs INSIDE the engine, during the `<onentry>` that declared the act, with the
/// engine mutably borrowed. It cannot reach a pane and it cannot hand anything back to the driver
/// by return value, because its return value belongs to the machine (the events the act produced).
/// So what it does is WRITE DOWN the request, and the driver reads it on the pass that follows —
/// which is the same request/reply shape as any other outside-the-machine act.
#[derive(Clone, Default)]
pub struct Serving(Arc<Mutex<Book>>);

impl std::fmt::Debug for Serving {
    /// ⚠ Named rather than dumped: this is shared mutable state a formatter must not block on, and
    /// what a reader of a driver's `Debug` wants from it is that it exists.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Serving")
    }
}

/// [`Serving`]'s record.
///
/// # ⚠⚠⚠⚠⚠ One slot PER ACT, and the second one was a run's answer rather than a design
///
/// It was a single slot while this host served a single act, on the rule *the driver carries out
/// one act per pass*. [`Act::Pass`] broke that rule truthfully: the two acts answer two different
/// questions asked at two different moments of one pass — *what is this pass for* before the work,
/// *what does the edge you just took owe the peer* after it — so a pass legitimately carries out
/// one of each.
///
/// ⚠⚠ **AND THE RUN THAT SAID SO IS `resume`.** A person letting go of a held loop raises `resume`
/// OUTSIDE the pass's own machinery, and that edge declares a sentence; the sentence therefore
/// waits in the slot while the pass that follows asks what it is for. With one slot the pass act
/// arrived on top of it and was refused as an overrun — three gates went red at once, all of them
/// holds, and every one of them ended `failed` on `error.execution`.
///
/// ⚠ **WHAT DID NOT CHANGE IS THE REFUSAL.** A second act of the SAME kind over one nobody carried
/// out is still refused rather than queued or overwritten, which is the whole of what `Overrun`
/// was for: an overwrite is a sentence nobody said in a run that reads like one with less to say.
#[derive(Default)]
struct Book {
    /// The sentence act ([`Act::Say`]) nothing has carried out yet.
    saying: Option<Asked>,
    /// The pass act ([`Act::Pass`]) nothing has carried out yet.
    passing: Option<Asked>,
    /// The ending act ([`Act::End`]) — the word this run's ending publishes.
    ///
    /// ⚠⚠⚠ NEVER EMPTIED, unlike the two above, and it is not a slot in the same sense. An ending
    /// is entered once and asked about on every pass that follows it, because `OuterLoop::pumping`
    /// answers `Pumped::Ended` at the top of each one. A reader that took this would leave every
    /// later pass with nothing on a run whose ending had not changed. See `Serving::published`.
    ending: Option<Asked>,
    /// The account act ([`Act::Account`]) nothing has read yet.
    ///
    /// ⚠ TAKEN like the two above and unlike `ending`, because the driver raises `account`
    /// immediately before reading this: the answer is about the state the machine is in AT THE
    /// MOMENT OF ASKING, and a slot left full would answer a later ask with an earlier state's word.
    accounting: Option<Asked>,
    /// The arrival act ([`Act::Note`]) nothing has read yet.
    ///
    /// ⚠ TAKEN, because it is about the ARROW this pass drew and nothing else: a slot left full
    /// would put an earlier arrival's reading on a later arrow. Declared two ways — by arrival on a
    /// targetless transition, and on the two `judging -> working` edges — and both write here, so
    /// the take is what keeps them from accumulating.
    noting: Option<Asked>,
    /// Every act this host would not perform, in the order they were asked for.
    refused: Vec<Refused>,
}

impl Book {
    /// The slot `act`'s requests wait in.
    ///
    /// ⚠ A `match` rather than a map, so an act added to [`Act`] does not compile until somebody
    /// has said where its requests wait — the one place this module still gets a compiler.
    const fn slot(&mut self, act: Act) -> &mut Option<Asked> {
        match act {
            Act::Say => &mut self.saying,
            Act::Pass => &mut self.passing,
            Act::End => &mut self.ending,
            Act::Account => &mut self.accounting,
            Act::Note => &mut self.noting,
        }
    }
}

impl Serving {
    /// A host serving [`Act::ALL`] and asked for nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// **SERVE `machine`'S HOST ACTS.**
    ///
    /// ⚠⚠⚠ CALLED BEFORE `initialize`, and the order is load-bearing rather than tidy: a document
    /// whose INITIAL state declares an act would have it dispatched during initialisation, and a
    /// handler registered afterwards is a handler registered after the act it was for. `probe.rs`
    /// measured that; [`crate::document::opened`] is where the two are kept in that order for every
    /// document this crate drives.
    pub fn on<P: StatePolicy>(&self, machine: &mut Engine<P>) {
        let book = Arc::clone(&self.0);
        machine.register_event_processor(HOST, move |request| {
            let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
            // ⚠⚠ THE SLOT IS CONSULTED BEFORE THE ARGUMENTS ARE — for the act that ARRIVED, which
            // is what tells its slot from the other one — so a second act is refused for BEING
            // second rather than for whatever else might also be wrong with it. Two reasons
            // reported as one is the fold this register keeps paying for.
            let answer = match read(&request.event_name, &request.params) {
                Ok(arriving) => match book.slot(arriving.act()) {
                    Some(waiting) => Err(Refused::Overrun {
                        held: waiting.act(),
                        arriving: arriving.act(),
                    }),
                    empty => {
                        *empty = Some(arriving);
                        Ok(())
                    }
                },
                Err(why) => Err(why),
            };
            let held = &mut *book;
            match answer {
                Ok(()) => Vec::new(),
                Err(why) => {
                    let said = why.to_string();
                    held.refused.push(why);
                    vec![sce_rust_runtime::host_processor::HostSendResponse {
                        event_name: REFUSED.to_owned(),
                        // ⚠ The sentence travels as the event's data so a document that WANTS to
                        // route on it can, without this host inventing a second error class. The
                        // loop's own document routes on the name alone and ends `failed`.
                        event_data: said,
                    }]
                }
            }
        });
    }

    /// **THE `act` THE DOCUMENT ASKED FOR AND NOTHING HAS CARRIED OUT**, taken.
    ///
    /// Taking rather than reading: an act is performed once. A second pass over the same slot would
    /// put the same sentence to the peer twice.
    ///
    /// ⚠⚠ THE CALLER NAMES WHICH ACT IT IS ANSWERING, and that is not a convenience: the two acts
    /// are asked for at two different moments of one pass, so a taker that returned *whatever is
    /// waiting* would hand one caller the other's work — see this host's own `Book`, which is
    /// named rather than linked because it is crate-private and a public doc that links a private
    /// item does not build here. A caller that has no act for what it finds is one that took the
    /// wrong thing, and it cannot get here.
    pub fn taken(&self, act: Act) -> Option<Asked> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slot(act)
            .take()
    }

    /// **WHICH ENDING THIS RUN PUBLISHED**, or [`None`] for a run that has not reached one.
    ///
    /// # ⚠⚠⚠⚠⚠ Read rather than taken, which is the whole reason this is not [`Self::taken`]
    ///
    /// An ending is entered ONCE and asked about MANY TIMES: `OuterLoop::pumping` answers
    /// `Pumped::Ended` at the top of every pass after the machine completes, and each of those
    /// passes builds the same verdict. Taking would hand the FIRST reader the word and every later
    /// one [`None`] — on a run whose ending had not changed — which is the shape that would look
    /// like an ending the document forgot to declare.
    ///
    /// ⚠⚠ It is therefore a FACT ABOUT THE RUN rather than work waiting to be done, and it is kept
    /// on [`Self::refused`]'s terms: nothing is served by forgetting it.
    #[must_use]
    pub fn published(&self) -> Option<Publishes> {
        match self.0.lock().unwrap_or_else(PoisonError::into_inner).ending {
            Some(Asked::End { publishes, .. }) => Some(publishes),
            _ => None,
        }
    }

    /// **WHAT A STOP WOULD STILL HAVE TO REACH**, or [`None`] for a run that has not ended.
    ///
    /// ⚠⚠ [`None`] IS NOT [`Signals::Nothing`] AND THE CALLER MUST NOT FOLD THEM. This answers
    /// nothing at all while the run is in flight — which is exactly when a stop most certainly does
    /// have a pane to reach — so a reader that treated an absent answer as *nothing to signal*
    /// would leave a live model running after a cancel. See [`Signals`] for the direction that
    /// fails safe.
    ///
    /// ⚠ Read rather than taken, for [`Self::published`]'s reason: it rides the same act, and that
    /// act is a fact about the run rather than work waiting to be done.
    #[must_use]
    pub fn signalling(&self) -> Option<Signals> {
        match self.0.lock().unwrap_or_else(PoisonError::into_inner).ending {
            Some(Asked::End { signals, .. }) => Some(signals),
            _ => None,
        }
    }

    /// **EVERY ACT THIS HOST WOULD NOT PERFORM**, in the order they were asked for.
    ///
    /// ⚠ Read rather than taken: a refusal is a fact about the document, and a run that met one has
    /// already been ended by it. Nothing is served by forgetting.
    #[must_use]
    pub fn refused(&self) -> Vec<Refused> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .refused
            .clone()
    }
}

/// What `named` asks for, with `params` read as the act's arguments.
///
/// # Errors
///
/// [`Refused`] for an act nobody serves, an argument it needs and the send did not carry, or a
/// value outside the argument's space.
fn read(named: &str, params: &Params) -> Result<Asked, Refused> {
    let Some(act) = Act::of(named) else {
        return Err(Refused::Unserved {
            named: named.to_owned(),
        });
    };
    match act {
        Act::Say => {
            let text = argument(params, act, "text")?;
            // ⚠⚠⚠⚠⚠ AN EMPTY SENTENCE IS NOT A SHORT ONE — see [`Refused::Empty`]. `asks` needs no
            // such line: its space is closed, so `Asks::of("")` already answers [`None`] below and
            // the refusal a reader gets there names the space it missed.
            if text.is_empty() {
                return Err(Refused::Empty {
                    act,
                    argument: "text",
                });
            }
            let said = argument(params, act, "asks")?;
            let Some(asks) = Asks::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "asks",
                    said,
                    holds: Asks::ALL.map(Asks::named).to_vec(),
                });
            };
            Ok(Asked::Say { text, asks })
        }
        // ⚠⚠⚠ NO EMPTY CHECK OF ITS OWN, and that is the closed space doing the work `text`'s
        // needs a line for: `Does::of("")` already answers [`None`], so an argument that evaluated
        // to nothing is refused below with the space it missed printed beside it.
        Act::Pass => {
            let said = argument(params, act, "does")?;
            let Some(does) = Does::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "does",
                    said,
                    holds: Does::ALL.map(Does::named).to_vec(),
                });
            };
            // ⚠⚠⚠ OPTIONAL, AND THAT IS NOT A WEAKER RULE THAN `does`'s. A needle is meaningful
            // only where the pass could MEET an outage — `watch` — so a document that declared one
            // on `judge` or `attend` would be saying something nothing reads. What must never be
            // optional is the word that says WHAT the pass is; this qualifies one of its answers.
            Ok(Asked::Pass {
                does,
                needle: params
                    .get("needle")
                    .and_then(|values| values.first())
                    .cloned(),
            })
        }
        // ⚠ NO EMPTY CHECK OF ITS OWN, for `pass.do`'s reason: `Publishes::of("")` answers [`None`]
        // already, so an argument that evaluated to nothing is refused below with the space it
        // missed printed beside it.
        Act::End => {
            let said = argument(params, act, "publishes")?;
            let Some(publishes) = Publishes::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "publishes",
                    said,
                    holds: Publishes::ALL.map(Publishes::named).to_vec(),
                });
            };
            // ⚠⚠ REQUIRED, not defaulted, and the direction is why: the safe answer is `pane`, so a
            // default would be safe and a MISSING one would be silent. An ending that forgot to say
            // is a document defect and must arrive as one — see [`Signals`].
            let said = argument(params, act, "signals")?;
            let Some(signals) = Signals::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "signals",
                    said,
                    holds: Signals::ALL.map(Signals::named).to_vec(),
                });
            };
            Ok(Asked::End { publishes, signals })
        }
        // ⚠ NO EMPTY CHECK OF ITS OWN, for `does`'s reason: `Accounts::of("")` answers [`None`],
        // so an argument that evaluated to nothing is refused below with the space it missed.
        Act::Account => {
            let said = argument(params, act, "can")?;
            let Some(can) = Accounts::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "can",
                    said,
                    holds: Accounts::ALL.map(Accounts::named).to_vec(),
                });
            };
            Ok(Asked::Account { can })
        }
        // ⚠ NO EMPTY CHECK OF ITS OWN, for `does`'s reason: `Notes::of("")` answers [`None`].
        Act::Note => {
            let said = argument(params, act, "note")?;
            let Some(note) = Notes::of(&said) else {
                return Err(Refused::Unreadable {
                    act,
                    argument: "note",
                    said,
                    holds: Notes::ALL.map(Notes::named).to_vec(),
                });
            };
            Ok(Asked::Note { note })
        }
    }
}

/// `params`' value for `argument`, or the refusal for an act whose send did not carry it.
///
/// ⚠ The FIRST value of a repeated name, and the repetition is not an error: W3C SCXML 6.2 permits
/// it and SCE keeps every value in document order, so refusing here would be this host deciding
/// something the specification allows. What it must not do is silently read the last one.
fn argument(params: &Params, act: Act, argument: &'static str) -> Result<String, Refused> {
    params
        .get(argument)
        .and_then(|values| values.first())
        .cloned()
        .ok_or(Refused::Missing { act, argument })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use sce_rust_runtime::{IScriptEngine, ScriptValue};

    use super::{
        Accounts, Act, Asked, Asks, Does, Notes, Publishes, Refused, Serving, Signals, read,
    };
    use crate::sm::probe_send_type_sm::ProbeSendTypePolicy;

    /// The act `probe_send_type.scxml` addresses to this host — a name [`Act`] does not serve.
    ///
    /// ⚠ That is what makes the document usable as the subject here, and it is not a coincidence
    /// this gate arranged: the probe was written to ask whether a host CAN be reached at all, so it
    /// picked a name of its own. An act vocabulary that ever grew this name would make the gate
    /// below silently about nothing — which is why the assertion says so out loud.
    const NOT_AN_ACT: &str = "reached.host";

    /// How many passes of the engine's scheduler a reply needs to reach the document.
    ///
    /// ⚠ A host handler's answer goes on the EXTERNAL queue (W3C SCXML C.1), which `step` does not
    /// poll. `probe.rs` measured the same thing and ticks for the same reason.
    const TICKS: usize = 8;

    /// What `machine`'s datamodel holds for `name`, as a number.
    fn count(engine: &sce_rust_runtime::Engine<ProbeSendTypePolicy>, name: &str) -> i64 {
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        match engine.policy().script_engine.get_variable(&session, name) {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number this document holds: {other:?}"),
        }
    }

    /// ⚠⚠⚠⚠⚠ **AN ACT NOBODY SERVES IS REFUSED, AND A DOCUMENT CAN HEAR THE REFUSAL** — register
    /// item 470, and the failure it names.
    ///
    /// # ⚠⚠⚠ Why silence is the thing being ruled out, rather than an error being the thing wanted
    ///
    /// A handler that answered an unknown act with an empty list would be doing exactly what SCE
    /// says an empty list means — *performed, nothing to report* — and the document would carry on
    /// as if the act had happened. [`crate::document`] measured what that costs with a mutation: one
    /// `<send>` naming a type nobody serves in `priming`, and a real run walked into `working` and
    /// then took **eleven eventless passes**, going nowhere, with every other gate green.
    ///
    /// # ⚠⚠⚠⚠ The control is the SAME document and the SAME send, served by somebody who does
    ///
    /// Without it, `errors == 1` is consistent with a document that raises an error whatever
    /// happens, and `landed == 0` with a `<send>` that delivers nothing here at all. The axis
    /// between the two halves is exactly one thing — whether the host performs this act — so what is
    /// read is attributable to the host rather than to the engine or the file.
    ///
    /// ⚠⚠ AND THE DOCUMENT'S OWN THIRD COUNTER IS A CONTROL INSIDE EACH HALF: `plain` is an
    /// untyped `<send>` in the same `onentry`, so a `landed` of zero cannot be read as *this
    /// document sends nothing*.
    #[test]
    fn an_act_this_host_does_not_serve_is_refused_where_the_document_can_hear_it() {
        assert!(
            Act::of(NOT_AN_ACT).is_none(),
            "⚠⚠⚠ THE PREMISE: this gate is about an act nobody serves, and {NOT_AN_ACT:?} has \
             become one this host performs. Point the gate at a name that is still outside \
             {:?}, or it is measuring nothing.",
            Act::ALL.map(Act::named),
        );

        // ── THE SUBJECT: the product's own host, through the product's own door ──
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let serving = Serving::new();
        let mut refusing =
            crate::document::opened(ProbeSendTypePolicy::new(Arc::clone(&lua)), &serving)
                .expect("this document answers its own `error.execution`, so the door admits it");
        for _ in 0..TICKS {
            refusing.tick();
        }

        // ── THE CONTROL: the same document and the same send, served by a host that performs it ──
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut served = sce_rust_runtime::Engine::new(ProbeSendTypePolicy::new(Arc::clone(&lua)));
        served.register_event_processor(super::HOST, |request| {
            vec![sce_rust_runtime::host_processor::HostSendResponse {
                event_name: request.event_name,
                event_data: String::new(),
            }]
        });
        served.initialize();
        for _ in 0..TICKS {
            served.tick();
        }

        assert_eq!(
            (count(&served, "plain"), count(&refusing, "plain")),
            (1, 1),
            "⚠⚠⚠ THE CONTROL INSIDE EACH HALF: the untyped `<send>` beside the typed one must \
             deliver in BOTH, or nothing below is about acts at all",
        );
        assert_eq!(
            (count(&served, "landed"), count(&served, "errors")),
            (1, 0),
            "⚠⚠⚠ THE STAGED CONTROL: a host that DOES perform {NOT_AN_ACT:?} makes the act's own \
             event arrive and raises no refusal. If this half does not hold, the half below says \
             nothing about serving.",
        );

        assert_eq!(
            count(&refusing, "errors"),
            1,
            "⚠⚠⚠⚠⚠ THE CLAIM: this host does not perform {NOT_AN_ACT:?} and the DOCUMENT must be \
             told so. A zero here is the silence item 470 is about — an act nobody performed, \
             reported to the machine as one that worked.",
        );
        assert_eq!(
            count(&refusing, "landed"),
            0,
            "⚠⚠ and the act's own event must NOT arrive: a refusal that also delivered would leave \
             the document holding both answers",
        );

        // ── AND THE HOST KEEPS ITS OWN RECORD, because the event cannot say WHICH act ──
        let refused = serving.refused();
        assert_eq!(
            refused,
            vec![Refused::Unserved {
                named: NOT_AN_ACT.to_owned(),
            }],
            "⚠⚠⚠ the document ends `failed` naming `error.execution`, which is one word for every \
             refusal there could ever be. The act's own name has to survive somewhere or nobody can \
             repair the file.",
        );
        assert!(
            refused[0].to_string().contains(NOT_AN_ACT),
            "⚠⚠ and the sentence a person reads must NAME it: {}",
            refused[0],
        );
        // ⚠ BOTH SLOTS, because a refusal recorded in the OTHER one is exactly as silent as a
        // refusal recorded in the one this gate happens to name — see [`Book`].
        assert!(
            Act::ALL.iter().all(|act| serving.taken(*act).is_none()),
            "⚠⚠⚠⚠ AND A REFUSED ACT MUST NOT BE RECORDED AS ONE TO CARRY OUT. A host that refused \
             the machine and queued the work anyway would do it on the next pass, to a run the \
             document had already failed.",
        );
    }

    /// ⚠⚠⚠⚠ **AN ACT THIS HOST SERVES IS REFUSED TOO WHEN ITS ARGUMENTS ARE NOT ONES IT CAN
    /// PERFORM** — and this is the guard that replaced a compiler.
    ///
    /// `Owed::asked_for_an_account` was an EXHAUSTIVE match over the document's states, and its own
    /// comment said what the exhaustiveness bought: *a state that asks its agent for something and
    /// forgets to say so would publish NOTHING and look exactly like a state whose turn was work*.
    /// A document cannot be made to fail to compile, so what stands in its place is that `asks` is
    /// REQUIRED and its value space is CLOSED — and both of those are only worth anything if a
    /// breach is REFUSED rather than defaulted.
    ///
    /// ⚠ Asked of [`read`] directly rather than through an engine, and the reason is that the
    /// engine cannot be asked: no document this crate ships names `prompt.say` with a bad argument,
    /// and a gate that added one would be testing a document written to fail. The WIRING claim —
    /// that a refusal reaches the machine at all — is the gate above's, driven end to end.
    #[test]
    fn an_act_whose_arguments_this_host_cannot_perform_is_refused_and_never_defaulted() {
        let asking = |act: Act, pairs: &[(&str, &str)]| {
            let mut params: HashMap<String, Vec<String>> = HashMap::new();
            for (name, value) in pairs {
                params
                    .entry((*name).to_owned())
                    .or_default()
                    .push((*value).to_owned());
            }
            read(act.named(), &params)
        };
        let with = |pairs: &[(&str, &str)]| asking(Act::Say, pairs);
        let asks_of = |read: Result<Asked, Refused>| match read.expect("a well-formed act") {
            Asked::Say { asks, .. } => asks,
            other => panic!("`prompt.say` is what was asked for: {other:?}"),
        };

        // ── THE STAGED CONTROL: the well-formed act, so every refusal below is about the breach ──
        assert_eq!(
            asks_of(with(&[
                ("text", "where did you get to?"),
                ("asks", "account")
            ])),
            Asks::Account,
            "⚠⚠⚠ THE CONTROL: the act this host serves, with the arguments the document writes, \
             must be READ — otherwise the refusals below are consistent with a host that refuses \
             everything",
        );

        // ⚠⚠⚠⚠ AND THE CONTROL IS THE WHOLE VOCABULARY, not one word of it — 2026-08-26, the round
        // that added a THIRD. What this holds is that `named` and `of` agree over every word the
        // space claims: a value whose spelling nobody can read back is one a document may write and
        // this host would refuse, and it would look complete in the file that defines it.
        //
        // ⚠⚠⚠⚠⚠ WHAT IT CANNOT SEE, SAID HERE RATHER THAN LEFT TO BE ASSUMED: a variant DROPPED
        // from `ALL` while the enum keeps it. `Asks::of` walks `ALL`, so the word stops being
        // readable and this loop stops iterating it in the same edit — the assertion is blind to
        // exactly the mutation it reads as though it caught. **What catches that is the run**:
        // `outer::tests::a_reflection_is_told_what_to_say_and_what_it_asks_for_by_its_own_document`
        // drives a document that writes `direction`, and a word the space no longer holds is
        // refused where the machine can hear it. Measured in both directions, 2026-08-26.
        for asks in Asks::ALL {
            assert_eq!(
                asks_of(with(&[("text", "a sentence"), ("asks", asks.named())])),
                asks,
                "⚠⚠⚠⚠ `{}` is in `Asks::ALL` and this door does not read it back as itself",
                asks.named(),
            );
        }

        // ⚠⚠⚠⚠⚠ AND THE SAME OF THE SECOND ACT'S SPACE — register item 470, stage 3. `does` is
        // what replaced `pump`'s twenty-eight-arm state match, so the argument that carries it is
        // load-bearing in exactly the way `asks` is: a word this door cannot read back is one a
        // state may declare and this host would refuse, leaving the run `failed` at a state whose
        // effect the driver still has.
        for does in Does::ALL {
            assert_eq!(
                match asking(Act::Pass, &[("does", does.named())])
                    .expect("every word this space holds is one a state may declare")
                {
                    Asked::Pass { does, .. } => does,
                    other => panic!("`pass.do` is what was asked for: {other:?}"),
                },
                does,
                "⚠⚠⚠⚠ `{}` is in `Does::ALL` and this door does not read it back as itself",
                does.named(),
            );
        }
        // ⚠⚠⚠⚠⚠ AND THE SAME OF THE THIRD ACT'S SPACE — register item 470, stage 3, the fourth
        // match. `publishes` is what replaced `AiLoop::ended`'s twenty-eight-arm state match, and it
        // is the most load-bearing of the three: a word this door cannot read back is an ENDING the
        // run reached and could not report, on a machine that is already over and has no pass left
        // to notice.
        //
        // ⚠⚠ THE COUNT IS PINNED BESIDE THE WALK, and that is not belt-and-braces. A loop over
        // `ALL` decides how many times it asserts from the very list it is checking, so a variant
        // dropped from `ALL` shrinks the control with the space and this reads green over a word
        // nobody can publish any more. The seven are the document's seven `<final>` elements, and
        // that is the number this must not lose quietly.
        assert_eq!(
            Publishes::ALL.len(),
            7,
            "⚠⚠⚠⚠⚠ `ai_loop.scxml` declares SEVEN `<final>` elements and this space must hold a \
             word for each. A space that shrank while the document did not is an ending that \
             reaches this door and is refused, which the machine has no pass left to hear",
        );
        for publishes in Publishes::ALL {
            assert_eq!(
                match asking(
                    Act::End,
                    &[("publishes", publishes.named()), ("signals", "pane")]
                )
                .expect("every word this space holds is one an ending may declare")
                {
                    Asked::End { publishes, .. } => publishes,
                    other => panic!("`end.publish` is what was asked for: {other:?}"),
                },
                publishes,
                "⚠⚠⚠⚠ `{}` is in `Publishes::ALL` and this door does not read it back as itself",
                publishes.named(),
            );
        }

        // ⚠⚠⚠⚠⚠ AND THE SECOND ARGUMENT OF THAT SAME ACT — register item 470, stage 3, the fifth
        // match. `signals` is what replaced `AiLoop::driving`'s twenty-eight-arm state match, and a
        // word this door cannot read back is an ending whose stop reaches NOTHING because the
        // refusal left the driver with no answer at all.
        assert_eq!(
            Signals::ALL.len(),
            2,
            "⚠⚠⚠ the space this walk asserts over decides how many times it asserts, so its size is \
             pinned beside it: a variant dropped from `ALL` would shrink the control with the space",
        );
        for signals in Signals::ALL {
            assert_eq!(
                match asking(
                    Act::End,
                    &[("publishes", "converged"), ("signals", signals.named())]
                )
                .expect("every word this space holds is one an ending may declare")
                {
                    Asked::End { signals, .. } => signals,
                    other => panic!("`end.publish` is what was asked for: {other:?}"),
                },
                signals,
                "⚠⚠⚠⚠ `{}` is in `Signals::ALL` and this door does not read it back as itself",
                signals.named(),
            );
        }
        // ⚠⚠⚠⚠⚠ AND THE FOURTH ACT'S SPACE — register item 470, stage 3, the last match. `can` is
        // what replaced `ask_for_an_account`'s twenty-eight-arm state match, and a word this door
        // cannot read back is a ceiling falling due on a run that can say nothing about why.
        assert_eq!(
            Accounts::ALL.len(),
            5,
            "⚠⚠⚠ the space this walk asserts over decides how many times it asserts, so its size \
             is pinned beside it: a variant dropped from `ALL` would shrink the control with it",
        );
        for can in Accounts::ALL {
            assert_eq!(
                match asking(Act::Account, &[("can", can.named())])
                    .expect("every word this space holds is one a state may declare")
                {
                    Asked::Account { can } => can,
                    other => panic!("`account.ask` is what was asked for: {other:?}"),
                },
                can,
                "⚠⚠⚠⚠ `{}` is in `Accounts::ALL` and this door does not read it back as itself",
                can.named(),
            );
        }
        assert_eq!(
            asking(Act::Account, &[("can", "")]),
            Err(Refused::Unreadable {
                act: Act::Account,
                argument: "can",
                said: String::new(),
                holds: Accounts::ALL.map(Accounts::named).to_vec(),
            }),
            "⚠⚠⚠⚠ AN ACCOUNT ANSWER THAT EVALUATED TO NOTHING IS REFUSED BY THE SPACE ITSELF, on \
             `does`'s terms: the empty string is not one of the five, and a default here would \
             grant or refuse a window on a state nobody decided about",
        );

        // ⚠⚠⚠⚠⚠ AND THE FIFTH ACT'S SPACE — register item 470, stage 3, the last match. `note` is
        // what replaced `pumping`'s twenty-eight-arm arrival match, and a word this door cannot
        // read back is an arrow rendered between doors a reader cannot then tell apart.
        assert_eq!(
            Notes::ALL.len(),
            5,
            "⚠⚠⚠ the space this walk asserts over decides how many times it asserts, so its size \
             is pinned beside it: a variant dropped from `ALL` would shrink the control with it",
        );
        for note in Notes::ALL {
            assert_eq!(
                match asking(Act::Note, &[("note", note.named())])
                    .expect("every word this space holds is one an arrival may declare")
                {
                    Asked::Note { note } => note,
                    other => panic!("`arrival.note` is what was asked for: {other:?}"),
                },
                note,
                "⚠⚠⚠⚠ `{}` is in `Notes::ALL` and this door does not read it back as itself",
                note.named(),
            );
        }
        assert_eq!(
            asking(Act::Note, &[("note", "")]),
            Err(Refused::Unreadable {
                act: Act::Note,
                argument: "note",
                said: String::new(),
                holds: Notes::ALL.map(Notes::named).to_vec(),
            }),
            "⚠⚠⚠⚠ AN ARRIVAL WORD THAT EVALUATED TO NOTHING IS REFUSED BY THE SPACE ITSELF, on \
             `does`'s terms: the empty string is not one of the five, and a default here would put \
             some other door's reading on this arrow",
        );

        assert_eq!(
            asking(Act::End, &[("publishes", "converged")]),
            Err(Refused::Missing {
                act: Act::End,
                argument: "signals",
            }),
            "⚠⚠⚠⚠⚠ AN OMITTED `signals` MUST NOT DEFAULT, and the direction is exactly why this is \
             worth a line: the SAFE answer is `pane`, so a default would be safe and a missing one \
             would be SILENT. An ending that forgot to say is a document defect and has to arrive \
             as one",
        );
        assert_eq!(
            asking(Act::End, &[("publishes", "")]),
            Err(Refused::Unreadable {
                act: Act::End,
                argument: "publishes",
                said: String::new(),
                holds: Publishes::ALL.map(Publishes::named).to_vec(),
            }),
            "⚠⚠⚠⚠ AN ENDING WHOSE WORD EVALUATED TO NOTHING IS REFUSED BY THE SPACE ITSELF, on \
             `does`'s terms exactly: the empty string is not one of the seven, and a default here \
             would report some other ending's verdict for it",
        );
        assert_eq!(
            asking(Act::Pass, &[]),
            Err(Refused::Missing {
                act: Act::Pass,
                argument: "does",
            }),
            "⚠⚠⚠⚠⚠ AN OMITTED `does` MUST NOT DEFAULT, and this is the guard that replaced the \
             compiler: the match this act took over was EXHAUSTIVE, so a state the driver had no \
             act for could not be added. A default here would give every such state some other \
             state's effect, quietly.",
        );
        assert_eq!(
            asking(Act::Pass, &[("does", "")]),
            Err(Refused::Unreadable {
                act: Act::Pass,
                argument: "does",
                said: String::new(),
                holds: Does::ALL.map(Does::named).to_vec(),
            }),
            "⚠⚠⚠⚠ AND A `<param expr=\"…\">` THAT EVALUATED TO NOTHING IS REFUSED BY THE SPACE \
             ITSELF, which is why this act needs no `Empty` arm of its own: the empty string is \
             not one of the twelve words, and the refusal a reader gets names them.",
        );
        assert_eq!(
            asking(Act::Pass, &[("does", "Watch")]),
            Err(Refused::Unreadable {
                act: Act::Pass,
                argument: "does",
                said: "Watch".to_owned(),
                holds: Does::ALL.map(Does::named).to_vec(),
            }),
            "⚠⚠⚠ and a word outside the space is refused rather than read as the nearest one",
        );
        assert_eq!(
            asking(Act::Pass, &[("does", "watch")])
                .expect("the well-formed act")
                .act(),
            Act::Pass,
            "⚠⚠ THE STAGED CONTROL FOR THE THREE ABOVE: this act reads at all, so the refusals are \
             about the arguments rather than about `pass.do` being unserved",
        );

        assert_eq!(
            with(&[("text", "carry on")]),
            Err(Refused::Missing {
                act: Act::Say,
                argument: "asks",
            }),
            "⚠⚠⚠⚠⚠ AN OMITTED `asks` MUST NOT DEFAULT. This is the exact shape the deleted \
             exhaustive match caught for free: a state that asks for something and does not say \
             what would collect no account and look identical to one asking for work.",
        );
        assert_eq!(
            with(&[("asks", "account")]),
            Err(Refused::Missing {
                act: Act::Say,
                argument: "text",
            }),
            "⚠⚠ and a sentence with no words is not a prompt — it would open a turn by pressing \
             Enter at a peer",
        );
        assert_eq!(
            with(&[("text", ""), ("asks", "account")]),
            Err(Refused::Empty {
                act: Act::Say,
                argument: "text",
            }),
            "⚠⚠⚠⚠⚠ AND THE `<param>` THAT IS PRESENT AND EMPTY IS THE ONE A DOCUMENT ACTUALLY \
             PRODUCES, which the assertion above cannot reach: no document omits `text`, but every \
             `<param expr=\"…\">` over a datamodel that has stopped answering evaluates to \
             nothing. Measured 2026-08-25 — this host performed one, typed a bare submit, and \
             reported ONE BYTE as a turn. ⚠ `Missing` is the wrong answer here even though it \
             refuses: it would send a reader looking for a `<param>` that is right there.",
        );
        assert_eq!(
            with(&[("text", "carry on"), ("asks", "Account")]),
            Err(Refused::Unreadable {
                act: Act::Say,
                argument: "asks",
                said: "Account".to_owned(),
                holds: Asks::ALL.map(Asks::named).to_vec(),
            }),
            "⚠⚠⚠ AND A WORD OUTSIDE THE SPACE IS REFUSED RATHER THAN READ AS THE OTHER ONE. A \
             capital is what a person writes; reading it as `work` would silently drop an account \
             the document asked for.",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A SECOND ACT OVER ONE NOBODY CARRIED OUT IS REFUSED, NOT SWALLOWED.**
    ///
    /// This host performs one act per pass of the driver, so a document declaring two would have
    /// one of them simply not happen — a sentence nobody said, in a run that reads exactly like one
    /// with less to say. That is the same silence as an act nobody serves, arriving by a different
    /// door, and it gets the same answer.
    ///
    /// ⚠⚠ **THE FIRST IS KEPT AND THE SECOND REFUSED**, which is a decision rather than an
    /// accident: the earlier act is the one the run has already been shaped by, and a host that
    /// preferred the newer one would silently discard whichever the document meant first.
    ///
    /// ⚠ Driven through a REAL engine over a real document, so what is measured is the handler as
    /// the engine calls it — the same road the wiring gate above takes. The act is the loop's own
    /// `prompt.say`, put twice through the one door a document has.
    #[test]
    fn a_second_act_over_one_nobody_carried_out_is_refused_and_the_first_is_kept() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let serving = Serving::new();
        let mut machine = sce_rust_runtime::Engine::new(ProbeSendTypePolicy::new(lua));
        serving.on(&mut machine);

        let say = |text: &str| sce_rust_runtime::HostSendRequest {
            processor_type: super::HOST.to_owned(),
            event_name: Act::Say.named().to_owned(),
            params: [
                ("text".to_owned(), vec![text.to_owned()]),
                ("asks".to_owned(), vec![Asks::Work.named().to_owned()]),
            ]
            .into_iter()
            .collect(),
            ..sce_rust_runtime::HostSendRequest::default()
        };

        // ⚠ THE STAGED CONTROL: the first act is performed and answers with no event of its own,
        // which is what makes the refusal below attributable to it being SECOND.
        assert!(
            machine
                .perform_host_send(say("the first question"))
                .is_some_and(|raised| raised.is_empty()),
            "⚠⚠⚠ THE CONTROL: the first act must be PERFORMED — a host that refused this one too \
             would make the assertion below true for the wrong reason",
        );
        let second = machine
            .perform_host_send(say("the second question"))
            .expect("a registered host is asked");

        assert_eq!(
            second.len(),
            1,
            "⚠⚠⚠⚠⚠ a second act must reach the MACHINE as a refusal. An empty answer is SCE's \
             *performed, nothing to report*, and a sentence that was never said reported as one \
             that was is the whole defect this module is for. Got {second:?}",
        );
        assert_eq!(
            second[0].event_name,
            super::REFUSED,
            "⚠⚠ and the refusal is the document's own error class, so a file that answers \
             `error.execution` — as `ai_loop.scxml` does — ends the run rather than drifting",
        );
        assert_eq!(
            serving.refused(),
            vec![Refused::Overrun {
                held: Act::Say,
                arriving: Act::Say,
            }],
            "⚠⚠⚠ and this host keeps which refusal it was, because the event is one word",
        );

        // ── AND THE FIRST IS THE ONE THAT SURVIVED ──
        let carried = serving
            .taken(Act::Say)
            .expect("the first act is still to be done");
        assert_eq!(
            carried,
            Asked::Say {
                text: "the first question".to_owned(),
                asks: Asks::Work,
            },
            "⚠⚠⚠⚠ THE SECOND MUST NOT HAVE OVERWRITTEN THE FIRST. An overwrite refuses the machine \
             and then performs the act it refused, which is worse than either answer alone.",
        );
        assert!(
            serving.taken(Act::Say).is_none(),
            "⚠⚠ and an act is carried out ONCE — a slot that answered twice would put the same \
             sentence to the peer again on the next pass",
        );
    }
}
