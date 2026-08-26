//! The `Orchestrator` plugin — a fixed-stimulus drive loop (plugin #1).
//!
//! Each step injects a fixed stimulus into one pane, waits for the pane to
//! react (via the producer's damage `generation`s), and converges when a
//! sentinel appears in the pane's output. It is the first [`Plugin`] consumer
//! of the [`PaneAccess`] extension API; the guardrails live in the [`Driver`].
//!
//! [`Driver`]: crate::driver::Driver

use std::time::Duration;

use sprag_terminal::PaneId;

#[cfg(test)]
use crate::access::{JobLeader, PaneDoing};
use crate::access::{KeyStroke, PaneAccess, PaneError, RowTrail};
use crate::completion::{Completion, Over, Stands, Turn};
use crate::consent::Consents;
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Attended, Reached, Readiness, ReadyWhen};
use crate::run::{Look, RunContext, Waited, park_until};

/// How long a step waits for the pane to react before judging on the current
/// screen.
const OBSERVE_TIMEOUT: Duration = Duration::from_millis(500);

/// What the orchestrator drives toward (the guardrails live in [`Guardrails`]).
///
/// [`Guardrails`]: crate::driver::Guardrails
#[derive(Clone, Debug)]
pub struct OrchestrationSpec {
    /// Text injected into the pane each step (followed by Enter).
    pub stimulus: String,
    /// Convergence condition: succeed once the pane's collapsed text contains
    /// this. `None` runs until a guardrail.
    pub sentinel: Option<String>,
    /// What the pane must SHOW before the first stimulus is injected — see
    /// [`Readiness`], which is where this barrier lives and why it exists. `None`
    /// starts driving immediately, which is right for a pane already running the
    /// program.
    pub ready_when: Option<ReadyWhen>,
    /// How long to wait for [`ready_when`](Self::ready_when), or `None` for
    /// [`DEFAULT_READY_TIMEOUT`](crate::readiness::DEFAULT_READY_TIMEOUT).
    ///
    /// The caller's, because how long a program takes to start is the thing that
    /// varies most between the programs this drives — `cat` is instant, an agent
    /// takes seconds, a cold test runner minutes — and the caller who names the
    /// marker is exactly the one who knows.
    pub ready_within: Option<Duration>,
    /// WHAT THIS RUN MAY ANSWER if the peer stops to ask — `None` answers nothing, which is what
    /// this loop did for its whole life before the contract existed.
    ///
    /// ⚠ This is the loop that MEASURED the failure the contract is about: a peer that blocked
    /// after the first step was typed at three more times, each stimulus landing on a menu as a
    /// SELECTION, and the run reported `Exhausted(Iterations)`. See
    /// [`Consents`].
    pub may_answer: Option<Consents>,
    /// WHETHER ANYBODY IS WATCHING this pane, and for how long — see [`Attended`]. The other half
    /// of [`may_answer`](Self::may_answer): what this run may answer itself, and who answers what
    /// it may not.
    pub attended: Attended,
    /// WHAT MAKES THE PEER'S TURN OVER, and how long it may take — see [`Turn`].
    ///
    /// # ⚠⚠⚠ `None` is a step that ends on a 500 ms clock, which is what this plugin always did
    ///
    /// And it is the defect [`Turn`] carries the measurement for: without a contract the step's
    /// wait is a private half-second constant, so a peer slower than that is typed at again, and
    /// again, for as long as it takes to think. **Absent stays exactly that behaviour** — an added
    /// argument whose absence changed what a run did would make every existing caller's run mean
    /// something they did not ask for, and it would do it silently.
    ///
    /// ⚠⚠ Which is also what keeps the measurement honest: the gate that measures the defect goes
    /// on measuring it, because the old behaviour is still a declared option rather than a deleted
    /// one. R372's rule, and this is the third round it has paid.
    pub turn: Option<Turn>,
}

/// A fixed-stimulus drive plugin over one pane.
pub struct Orchestrator {
    pane: PaneId,
    spec: OrchestrationSpec,
    /// What the pane's rows HELD before the last stimulus, so the observe-wait keys on *this*
    /// step's reply. ⚠ Text and not damage generations — see [`RowTrail`].
    ///
    /// ⚠⚠ **THE FALLBACK ONLY**, since register item 639: a row comparison cannot survive a scroll,
    /// and [`Orchestrator::heard`] carries what that cost. A host that publishes line addresses is
    /// read through [`spoken`](Self::spoken) instead.
    baseline: RowTrail,
    /// **THE LINE ADDRESS THIS STEP STARTED FROM** — everything the pane has completed after it is
    /// what this step provoked, and everything before it was already there.
    ///
    /// ⚠ A cursor and not a snapshot, which is the whole reason it survives a scroll: a logical
    /// line's address is minted at the pane's birth and reflow is defined as preserving it. See
    /// [`heard`](Self::heard).
    spoken: u64,
    /// The barrier this run must clear before it types anything — see [`Readiness`].
    ready: Readiness,
    /// What makes THIS step's turn over, armed at the step that typed it — `None` for a run that
    /// declared no [`Turn`] and so ends its steps on [`OBSERVE_TIMEOUT`].
    done: Option<Completion>,
}

impl Orchestrator {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: OrchestrationSpec) -> Self {
        Self {
            ready: Readiness::new(
                spec.ready_when.clone(),
                spec.ready_within,
                spec.may_answer.clone(),
                spec.attended,
            ),
            done: spec.turn.as_ref().map(|turn| Completion::new(turn.when())),
            pane,
            spec,
            baseline: RowTrail::default(),
            // ⚠ Zero, and the first mark walks it forward past whatever the pane already said —
            // see [`Plugin::step`]. A cursor invented here would be a claim about a pane this
            // plugin has not looked at yet.
            spoken: 0,
        }
    }

    /// Wait (bounded, cancellable) for THE ANSWER THIS STEP ASKED FOR.
    ///
    /// # ⚠⚠⚠ Why the wait is keyed on the SENTINEL and not on "the peer said something"
    ///
    /// A run that named a sentinel has said what it is waiting for. Ending the wait on the first
    /// row the peer produces asks a different question — *has it begun* rather than *has it
    /// finished* — and the two differ for every peer that says anything before its answer: an AI
    /// CLI painting a spinner or a tool-use line, a build printing what it is compiling, a shell
    /// reporting a job. The step then judged the sentinel against a screen that had not reached it,
    /// read `continue`, and typed the stimulus AGAIN at a peer that was part-way through answering
    /// the first one. Measured: a peer that says one line of its own before replying was prompted
    /// TWICE for one question, and the second prompt landed while it was still answering.
    ///
    /// ⚠⚠ Which is [`reaction`](Self::reaction)'s own lesson one turn further on. That one stopped
    /// the terminal's ECHO from ending the wait; this one stops the peer's PREAMBLE from ending it.
    /// Both are the same mistake — treating movement as an answer — and the fixtures that came
    /// first hid it, because a peer that replies in a single write has no preamble to see.
    ///
    /// ⚠ WITHOUT a sentinel there is nothing named, so the question really is *did the peer
    /// produce a row of its own* and the wait is unchanged.
    ///
    /// ⚠ It still FAILS SAFE, and more of the time than before: a sentinel that never arrives costs
    /// the rest of the step's wait and no more, because the verdict is judged off the collapsed
    /// screen after the wait either way. A convergence can be reached late; it can never be lost.
    ///
    /// ⚠⚠ IT ANSWERS **WHICH** THING ENDED IT, where it used to answer only *whether* something
    /// did. [`Arrival`] is why: two of the endings need the step to say different things, and one
    /// of them — the peer stopping to ASK — could not be represented at all while this returned a
    /// [`Waited`].
    fn observe(&self, panes: &dyn PaneAccess, run: &RunContext) -> Arrival {
        // ⚠ Seeded with what an unfired wait means, so reading it back after the poll needs no
        // `expect` — [`Completion::wait`]'s shape, for its reason.
        let mut arrival = Arrival::Nothing;
        // ⚠⚠⚠⚠⚠ **PARKED ON THE PANE, NOT POLLED AT IT** — register item 632, which is item 280's
        // defect left standing in this plugin after the loop's was paid. [`patience`](Self::patience)
        // answers [`Duration::MAX`] for a contract with no bound of its own, and this wait used to
        // render the pane's screen and run a detector over it every
        // [`POLL_INTERVAL`](crate::run::POLL_INTERVAL) until the RUN's deadline or cancel arrived:
        // 98 looks a second, ~360,000 an hour, at a peer that had said nothing.
        //
        // ⚠⚠⚠⚠⚠ **ONE READING OF THE CONTRACT PER ROUND, AND BOTH TERMS ARE SERVED FROM IT** —
        // register item 637. The contract's deadline and the contract's ending used to be two
        // calls, each reading the pane's supervisor for itself; the order between them was
        // load-bearing for `park_until`'s lost-wakeup reason — *a verdict that publishes between
        // the two reads leaves a deadline already past, which buys one more look*. One reading has
        // no between, so the two terms are the same instant by construction and the ordering is no
        // longer a thing an edit can get wrong.
        //
        // ⚠⚠ [`None`] IS *THIS STEP DECLARED NO CONTRACT*, and it is the only reading of it: the
        // reading is taken from the contract itself, so a step with one always has a reading and a
        // step without one can never be handed a stale one.
        let waited = park_until(run, panes, self.pane, self.patience(), || {
            let stands = self.done.as_ref().map(|done| done.stands(panes, self.pane));
            let settling = Self::settling(stands.as_ref());
            match self.arrived(panes, stands) {
                Some(seen) => {
                    arrival = seen;
                    Look::Holds
                }
                None => settling.not_yet(),
            }
        });
        match waited {
            Waited::Ready => arrival,
            Waited::TimedOut => Arrival::Nothing,
            Waited::Stopped => Arrival::RunEnded,
        }
    }

    /// How long one step waits — the caller's [`Turn::within`] when they declared a contract, and
    /// [`OBSERVE_TIMEOUT`] when they did not.
    ///
    /// ⚠⚠⚠ THE CONSTANT IS ONLY EVER A FALLBACK NOW, and that is the whole shape of the fix: a
    /// number nobody chose cannot be right for both a shell that answers in milliseconds and an
    /// agent that thinks for a minute. See [`Turn`].
    /// ⚠ [`Duration::MAX`] for a contract with no bound of its own, and it is not a constant in
    /// disguise: [`park_until`] compares ELAPSED against it and consults the run's deadline and
    /// cancel flag on every pass, so it means *no bound beyond the run's* and cannot overflow.
    ///
    /// ⚠⚠⚠ **AND IT IS NO LONGER AN HOUR OF LOOKING** — register item 632. While this wait polled,
    /// `Duration::MAX` meant *render this pane's screen a hundred times a second until the run
    /// ends*; parked, it means what it says. See [`observe`](Self::observe).
    fn patience(&self) -> Duration {
        self.spec.turn.as_ref().map_or(OBSERVE_TIMEOUT, |turn| {
            turn.within().unwrap_or(Duration::MAX)
        })
    }

    /// **WHEN [`arrived`](Self::arrived) CAN CHANGE ITS ANSWER WITH THE PANE STANDING STILL** —
    /// register item 632, and it is answerable term by term rather than being the open question the
    /// item feared.
    ///
    /// The union has three terms and only ONE of them has a clock:
    ///
    /// * the **SENTINEL** is `contains` over the collapsed SCREEN. It cannot change while the
    ///   screen does not.
    /// * the **row trail** ([`reaction`](Self::reaction)) compares this step's baseline against the
    ///   pane's rows. Same: a function of the pane's bytes.
    /// * the **CONTRACT** ([`Stands::over`]) rests on a supervisor's published verdict, and a
    ///   verdict SETTLES — it changes a window after the last output, with nothing to announce it.
    ///
    /// So the deadline is the contract's, taken from the same reading its ENDING was
    /// ([`Stands::settles`]) rather than derived here: a second reading of one pane's settling is
    /// exactly how two answers about it come to disagree, and this way a term added to
    /// [`Stands::over`] with a clock of its own arrives here in the same edit.
    ///
    /// ⚠ [`Settling::Nothing`](crate::access::Settling::Nothing) with no contract is a CLAIM and a
    /// true one: with no contract the union is screen-only, so nothing but the pane can move it.
    fn settling(stands: Option<&Stands>) -> crate::access::Settling {
        stands.map_or(crate::access::Settling::Nothing, |stands| stands.settles)
    }

    /// Whether what this step is waiting for has happened — see [`observe`](Self::observe).
    ///
    /// The three terms, in the order they are asked:
    ///
    /// * **the SENTINEL, when the caller named one.** Read off the COLLAPSED screen, which is the
    ///   same text and the same `contains` the verdict judges with — keying the wait on anything
    ///   the verdict does not read would let a step end on evidence its own judgement then
    ///   disagreed with. Asked FIRST because it is the run's whole goal: a peer still working when
    ///   its sentinel is already on the pane has nothing left to say that this run wants.
    /// * **the TURN being over**, when the caller declared a [`Turn`]. Not a preamble and not a
    ///   timer — it is the strongest evidence there is that nothing more is coming, which is why it
    ///   may end the wait early without repeating R374's mistake.
    /// * **the peer having produced a row of its own**, when the caller named neither. Unchanged,
    ///   and it is the only one of the three that is a guess.
    ///
    /// ⚠⚠ THE CONTRACT TERM NOW HAS TWO ENDINGS, and the second one is why this answers an
    /// [`Arrival`] rather than a `bool`: a peer that stops to ASK has finished its turn, and the
    /// union used to be unable to say so — the term simply stayed false and the step waited out
    /// its whole patience. See [`Over`].
    fn arrived(&self, panes: &dyn PaneAccess, stands: Option<Stands>) -> Option<Arrival> {
        if let Some(sentinel) = self.spec.sentinel.as_deref()
            && panes
                .pane_collapsed(self.pane)
                .is_some_and(|seen| seen.contains(sentinel))
        {
            return Some(Arrival::Sentinel);
        }
        match (stands, self.spec.sentinel.as_deref()) {
            // ⚠ CARRIED WHOLE rather than re-encoded into arms of this type's own. A second
            // spelling of *how did the turn end* beside [`Over`] is the shape this crate keeps
            // finding defects in, and it would go stale the day that type learns a fifth ending.
            //
            // ⚠⚠ `Some` HERE MEANS *THIS STEP DECLARED A CONTRACT*, which is what the contract's
            // own reading is evidence of — so the arms below cannot fall out of step with
            // `self.done` the way a separate `is_some()` test could.
            (Some(stands), _) => stands.over.map(Arrival::Turn),
            // A sentinel with no contract: the wait is the sentinel's alone (R374).
            (None, Some(_)) => None,
            (None, None) => match self.reaction(panes) {
                // ⚠ The rows that decided it travel with the ending, so the step's own account can
                // name them — see [`Reaction::Answered`].
                Reaction::Answered(spoke) => Some(Arrival::Reacted(spoke)),
                Reaction::EchoOnly | Reaction::None => None,
            },
        }
    }

    /// What the pane has done since this step's baseline.
    ///
    /// # ⚠⚠ Why the ECHO had to stop counting as a reaction
    ///
    /// A pty in cooked mode echoes what is injected before the program behind it
    /// has read a byte. Keying the wait on "any row changed" therefore ended EVERY
    /// step in microseconds against EVERY ordinary pane: the screen was judged
    /// before the peer had said anything, no sentinel was there, and the loop took
    /// another turn — re-prompting a peer that was still answering the last one. A
    /// peer replying in 200ms, well inside one step's [`OBSERVE_TIMEOUT`], was
    /// measured burning all three of a run's turns in 30 MILLISECONDS and reported
    /// `exhausted`. `max_iterations` was bounding a loop that had never once
    /// waited for a reply.
    ///
    /// ⚠ It FAILS SAFE. A real answer misread as an echo only costs the rest of
    /// the step's wait: the verdict is judged off the collapsed screen after the
    /// wait either way, so a convergence can be reached late but never lost.
    fn reaction(&self, panes: &dyn PaneAccess) -> Reaction {
        match panes
            .output_lines()
            .and_then(|source| source.pane_lines_since(self.pane, self.spoken))
        {
            Some(said) => self.heard(&said),
            // ⚠ The documented degradation, and the only path a host with no line addresses has.
            None => self.heard_on_the_rows(panes),
        }
    }

    /// **WHAT THE PEER SAID, FROM THE LINES IT ACTUALLY PRODUCED** — register item 639's repair.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the ROWS could never answer this, measured live
    ///
    /// [`RowTrail`] compares a pane's rows BY INDEX, so **a scroll reports as changed every row
    /// whose new text is simply its neighbour's old text** — nothing was written, and the rows come
    /// back as news. Over a pane running `cat`, which can answer nothing, the run's own journal
    /// recorded it marching upward one row a step:
    ///
    /// ```text
    /// 1. the peer answered: ["ld/sprag-…/crates/sprag-mcp$ prin", "tf 'ECHO-READY\n'; exec cat", "ECHO-READY"]
    /// 2. the peer answered: ["tf 'ECHO-READY\n'; exec cat", "ECHO-READY"]
    /// 3. the peer answered: ["ECHO-READY"]
    /// ```
    ///
    /// Every one of those is a wrapped SHELL PROMPT and a readiness marker that were on the pane
    /// before the step began. `PaneOutputLines`' own documentation names the hazard — *"a resize
    /// re-wraps and renumbers every one, a repaint changes none of them, and scrolling drops the
    /// ones nobody came back for"* — and this is the fourth defect this workspace has paid for it.
    ///
    /// A LOGICAL LINE is what the child produced, addressed from the pane's birth, and reflow is
    /// defined as preserving it. So a cursor taken before the stimulus separates *what this step
    /// provoked* from *what was already there*, and no amount of scrolling can blur the two.
    ///
    /// # ⚠⚠⚠ Three readings, and each absence means something different
    ///
    /// * **`lost` above zero** is the pane outrunning its retained history. That is not silence —
    ///   a peer that says more than the screen can keep has said something — so it answers
    ///   [`Answered`](Reaction::Answered) and says so in place of the text it cannot show.
    /// * **`partial`** is deliberately NOT consulted. It is half a sentence, the child has not
    ///   said it is finished, and this crate's own rule is that a consumer must earn the right to
    ///   use it. The cost is bounded and named in this function's caller: a peer that answers with
    ///   no trailing newline is read as silent for the rest of the step's wait, and the verdict is
    ///   judged off the collapsed screen afterwards either way.
    /// * **no complete lines at all** is [`None`](Reaction::None) — the pane produced nothing.
    ///
    /// ⚠⚠ **THE RESIDUE, STATED**: a pane sitting at a shell PROMPT has an unfinished line, and the
    /// echo of the stimulus COMPLETES it — so the first line this sees is `…$ echo bounded`, which
    /// is not a piece of the stimulus and reads as an answer. That is a different case from the one
    /// measured here (this plugin injects into a pane whose program is already running, which
    /// [`Readiness`] is the barrier for), it is UNMEASURED, and inventing a rule for it now would
    /// be guessing. It is filed rather than hidden.
    fn heard(&self, said: &sprag_vt::LinesSince) -> Reaction {
        if said.lost > 0 {
            return Reaction::Answered(vec![format!(
                "{} line(s) the pane outran before this step could read them",
                said.lost
            )]);
        }
        let mut heard = false;
        let mut spoke = Vec::new();
        for line in &said.lines {
            let text = line.trim();
            if text.is_empty() {
                continue;
            }
            heard = true;
            if !self.spec.stimulus.contains(text) {
                spoke.push(line.clone());
            }
        }
        if !spoke.is_empty() {
            return Reaction::Answered(spoke);
        }
        if heard {
            Reaction::EchoOnly
        } else {
            Reaction::None
        }
    }

    /// [`heard`](Self::heard)'s fallback for a host that publishes no line addresses — the ROW
    /// comparison this plugin used until register item 639, kept because a host without
    /// [`PaneOutputLines`](crate::access::PaneOutputLines) has nothing else.
    ///
    /// ⚠⚠ **IT IS A DEGRADATION AND NOT AN EQUIVALENT**, and the difference is the defect above: on
    /// a pane that SCROLLS this reads rows that merely moved as things the peer said. Named here so
    /// a host that lands on it knows what it is buying.
    fn heard_on_the_rows(&self, panes: &dyn PaneAccess) -> Reaction {
        let changed = self.baseline.fresh(panes, self.pane);
        if changed.is_empty() {
            return Reaction::None;
        }
        // A changed row is the ECHO when what it holds is a piece of what was just typed — the
        // `contains` covers a stimulus the pane wrapped across rows. A blank row is no evidence of
        // an answer either.
        let spoke: Vec<String> = changed
            .into_iter()
            .filter(|line| !line.trim().is_empty() && !self.spec.stimulus.contains(line.trim()))
            .collect();
        if spoke.is_empty() {
            return Reaction::EchoOnly;
        }
        Reaction::Answered(spoke)
    }
}

/// **WHAT ENDED A STEP'S WAIT** — see [`Orchestrator::arrived`], whose three terms these are the
/// answers to, plus the two ways a wait ends having found none of them.
///
/// ⚠ It is a type rather than a [`Waited`] because the endings are not degrees of one another: a
/// sentinel is the run's goal, a finished turn is a peer with nothing left to say, an ASK is a peer
/// that will say nothing further until somebody decides something, and a step has to report each
/// of those differently. While this was a `Waited`, the third was indistinguishable from *the peer
/// is still thinking* — and the step went on waiting for it.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Arrival {
    /// The sentinel the caller named is on the pane. The run's whole goal, so it is asked first.
    Sentinel,
    /// The peer's turn ended on the contract the caller declared — [`Over`] says HOW.
    Turn(Over),
    /// The peer produced a row of its own, where the caller named neither sentinel nor contract —
    /// carrying the rows that said so. See [`Reaction::Answered`].
    Reacted(Vec<String>),
    /// None of them, inside the step's own patience.
    Nothing,
    /// THE RUN ended underneath — cancelled, or out of time. The Driver's loop top says which.
    RunEnded,
}

/// What a pane has done since a step's baseline — the three cases a step must tell apart, because
/// two of them are the same absence of an answer with different remedies.
///
/// ⚠ NOT [`Copy`] any more, and that is the evidence arriving rather than a cost: the answering arm
/// now carries the rows it was reached on.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Reaction {
    /// Nothing on the pane changed at all: the peer is not listening, or is not there.
    None,
    /// Only the stimulus came back — the terminal's own echo, not the peer.
    EchoOnly,
    /// Something the peer produced — **and WHAT**, so a reader is not left with a verdict whose
    /// evidence nobody kept.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the evidence rides on the verdict, measured
    ///
    /// This word decides a step for every run that named no sentinel and no contract, and its
    /// journal line said only *"the peer answered; no sentinel yet"*. On 2026-08-24 that line went
    /// red in CI over a pane running `cat` — which cannot answer anything — and **two rounds of
    /// hypotheses about which rows had convinced it died** because none of them had been kept: a
    /// torn read, then a scroll, each refuted by a probe that could not reproduce what the product
    /// had plainly done. The screen was recoverable only by teaching the TEST to dump it.
    ///
    /// So the rows travel with the verdict, the way [`Over::Silent`](crate::completion::Over) has
    /// always carried its [`Silence`](crate::completion::Silence). A step that says the peer spoke
    /// can now say what it heard.
    Answered(Vec<String>),
}

impl Plugin for Orchestrator {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // ⚠⚠ NOT ONE BYTE UNTIL THE PANE IS READY. Injecting into a pane whose program has not
        // started is spending a turn on the shell that is still there — see [`Readiness`], which
        // owns this barrier and the `NeverReady` failure. Latched, so it costs nothing after the
        // first step.
        // ⚠⚠ A `match`, NOT an `== Reached::RunEnded`. All three injecting plugins compared
        // against one variant, so a barrier that learned a new answer would have been IGNORED by
        // every one of them and fallen through to the keystroke — which is how `Asking` was added
        // and compiled clean. Exhaustive here means a fourth answer cannot reach a pane unread.
        match self.ready.reached(panes, self.pane, run)? {
            Reached::Yes => {}
            // Nothing was injected, so nothing is charged; the Driver's loop top says which of the
            // two ways the run ended it was.
            // ⚠⚠⚠ AND IT SAYS WHAT IT WAS WAITING FOR. The note said only that a barrier was in the
            // way, so a run that spent its whole clock never becoming ready published
            // `exhausted: duration` with `Bytes(0)` — advice to raise a budget, about a pane that
            // would never have come up however long it waited. See [`Reached::RunEnded`].
            Reached::RunEnded(why) => {
                return Ok(Step::new(Cost::Bytes(0), Verdict::Continue).noting(format!(
                    "the run ended while waiting for the pane to be ready: {why}"
                )));
            }
            // The peer is showing a question. Typing the stimulus here would SELECT rather than
            // say anything — see [`Verdict::Blocked`].
            Reached::Asking(asking) => {
                let note = format!("the peer stopped to ASK: {}", asking.explain());
                return Ok(
                    Step::new(Cost::Bytes(asking.bytes()), Verdict::Blocked(asking)).noting(note),
                );
            }
            // The peer asked and the caller had consented to exactly this answer. The step is
            // SPENT on that: the peer is now acting on a decision, and the next step asks the
            // barrier again rather than typing a stimulus into a pane mid-transition.
            Reached::Answered(answered) => {
                let (note, cost) = (answered.describe(), answered.bytes);
                return Ok(Step::new(Cost::Bytes(cost), Verdict::Answered(answered)).noting(note));
            }
            // A PERSON answered what this run could not, so the step is spent on having waited and
            // the next one meets the barrier again.
            //
            // ⚠⚠ `Continue`, and NOT the fifth verdict word. `Answered` means THIS RUN decided
            // something on a caller's behalf, and that is the whole reason it is indexed: a journal
            // where a human's answer and a machine's read the same has lost the one distinction
            // that makes an approval traceable. The tally stays put and the note says what
            // happened, which is R369's ruling about the sixth outcome word applied one level down.
            Reached::Attended(attention) => {
                return Ok(Step::new(Cost::Bytes(attention.bytes()), Verdict::Continue)
                    .noting(attention.describe()));
            }
            // A PERSON TOOK THE PANE. Nothing is injected and nothing is charged — the whole point
            // is that this run stopped typing — and the verdict is terminal: the pane is theirs now.
            Reached::Interrupted(interruption) => {
                let note = interruption.describe();
                return Ok(Step::new(Cost::Bytes(0), Verdict::TakenOver(interruption)).noting(note));
            }
            // AND THEY GAVE IT BACK. Nothing was injected and nothing is charged; the step is spent
            // on the wait, and the next one meets the barrier again — so a dialog they left up, or
            // a program they started, is read by the questions that already exist rather than by
            // this arm guessing.
            //
            // ⚠⚠ `Continue`, NOT a verdict of its own, and for `Attended`'s reason one door over: a
            // journal in which a person's act and a machine's decision read alike has lost the
            // distinction that makes an approval traceable. The note says what happened.
            Reached::HandedBack(handover) => {
                return Ok(Step::new(Cost::Bytes(0), Verdict::Continue).noting(handover.describe()));
            }
        }

        // Baseline before acting, so observe() waits for this step's reply.
        //
        // ⚠⚠⚠⚠⚠ **BOTH MARKS, AND THE LINE CURSOR IS THE ONE THAT SURVIVES A SCROLL** — register
        // item 639. Walking the cursor past everything the pane has ALREADY completed is what
        // separates *what this step provoked* from *what was on the pane when it began*; the row
        // trail beside it is the fallback for a host that publishes no line addresses, and it
        // cannot make that separation at all. See [`Orchestrator::heard`].
        self.baseline = RowTrail::mark(panes, self.pane);
        self.spoken = panes
            .output_lines()
            .and_then(|source| source.pane_lines_since(self.pane, self.spoken))
            .map_or(self.spoken, |said| said.next);
        // ⚠⚠⚠ AND THE COMPLETION CONTRACT ARMS HERE, IN THE SAME BREATH AND FOR THE SAME REASON:
        // before a byte goes in. A peer waiting to be spoken to is AT REST, so a contract armed
        // after the injection can be satisfied by the stillness this step was addressed TO — and
        // the step would end before the peer had written a word. See [`Completion::begin`], which
        // is where `Agent` learned it.
        if let Some(done) = self.done.as_mut() {
            done.begin(panes, self.pane);
        }

        // Act: inject the stimulus + Enter.
        let mut keys = KeyStroke::text(&self.spec.stimulus);
        keys.push(KeyStroke::named("Enter"));
        // ⚠⚠⚠⚠ AND THE DOOR CAN REFUSE, WHICH IS THE 43 HOURS. This plugin is the one the preserved
        // stack showed inside the wedge (register item 310): it re-types its stimulus at the START
        // OF EVERY STEP, at the same pane, for ever — measured at 5 bytes and 509 ms a step, so
        // **3,380 steps, about 29 minutes, to a pseudoterminal that blocks and never returns**
        // (item 325). Nothing about the walk looks wrong on the way, which is why nobody saw it.
        //
        // ⚠⚠⚠ IT IS A VERDICT AND NOT A `?`. Propagating would end the run `failed` with the same
        // sentence, and the difference is what a JOURNAL can be asked: a step that says `peer_gone`
        // names the ending in the same vocabulary as `converged` and `blocked`, so *which of my
        // runs stopped because its agent's process left?* is a question rather than a grep over
        // free text. `Verdict::PeerGone`'s own doc holds why none of the other seven words fits.
        //
        // ⚠⚠ EVERY OTHER `PaneError` STILL PROPAGATES: an unknown pane and an unencodable key are
        // faults of the run, and the Driver's `failed` is where a reader is sent to fix one.
        // ⚠⚠⚠⚠⚠ **THE WRITE YIELDS TO THE PERSON THE BARRIER ALREADY CLEARED THIS PANE OF** —
        // register item 586. The barrier above asked *has somebody reached in?*; everything between
        // that question and this line is window, and it is not a narrow one — `Completion::begin`
        // reads the pane's supervisor, which is a lock and a detector here and a round trip over a
        // wire. **Measured at 20% and 30% of runs on two passes of one day**, counted at the WRITE
        // (`by_a_program` 23 when the person touched the keyboard, 24 when the run ended).
        //
        // ⚠⚠⚠ THE BARRIER'S OWN WATERMARK AND NOT A FRESH READ: a number taken here would ask *has
        // anyone written since a moment ago*, which forgives exactly the write this is about. A
        // host that cannot count hands answers `None` and the injection is the plain one — the
        // documented degradation, which is what every host had before this line.
        let injected = match self.ready.cleared_at() {
            Some(seen) => panes.inject_yielding_to_a_person(self.pane, &keys, seen),
            None => panes.inject(self.pane, &keys),
        };
        let cost = match injected {
            Ok(written) => written.bytes(),
            // ⚠⚠⚠⚠ THE PERSON WON THE RACE, and this run ends the way the barrier would have ended
            // it one step later: `TakenOver`, with the writes they made. Nothing was typed, so
            // nothing is charged. Reporting a FAILURE here would tell its reader to fix something,
            // and what happened is somebody doing what they are entitled to do.
            Err(PaneError::TakenOver(_)) => {
                // ⚠⚠ THE COUNT IS RE-READ AND MAY BE ABSENT, and the fallback is ONE rather than
                // zero: this door refused precisely because a person's write had been counted, so
                // *at least one* is a fact the arm already holds. Zero would say nobody reached in,
                // about the very event that produced the refusal.
                let interruption = self
                    .ready
                    .interruption(panes, self.pane)
                    .unwrap_or_else(|| crate::readiness::Interruption::of(1));
                let note = interruption.describe();
                return Ok(Step::new(Cost::Bytes(0), Verdict::TakenOver(interruption)).noting(note));
            }
            Err(PaneError::PeerGone(pane)) => {
                let note = PaneError::PeerGone(pane).to_string();
                return Ok(Step::new(Cost::Bytes(0), Verdict::PeerGone(pane)).noting(note));
            }
            Err(other) => return Err(other),
        };

        // Perceive, then judge against the collapsed (wrap-safe) screen text.
        // If the RUN ended mid-observe — cancelled, or out of time — don't judge:
        // return Continue so the Driver's loop top decides the terminal state,
        // rather than a spurious Converged off a screen nobody finished reading.
        let seen = self.observe(panes, run);
        if seen == Arrival::RunEnded {
            return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                .noting("the run ended while watching for the pane to react"));
        }
        let observed = panes.pane_collapsed(self.pane).unwrap_or_default();
        let verdict = if self
            .spec
            .sentinel
            .as_ref()
            .is_some_and(|sentinel| observed.contains(sentinel.as_str()))
        {
            Verdict::Converged
        } else {
            Verdict::Continue
        };
        // ⚠ A STIMULUS THE PANE NEVER REACTED TO IS THE FINDING, and it is invisible in the
        // outcome: the step costs the same bytes and reads `continue` either way, so a hundred
        // iterations against a pane that is not listening look exactly like a hundred against one
        // that is.
        let note = match (&seen, &verdict) {
            (_, Verdict::Converged) => "the sentinel appeared".to_string(),
            // ⚠⚠⚠ THE STEP ENDS THE INSTANT ITS PEER ASKS, and it ends with a NOTE rather than a
            // verdict of its own.
            //
            // `Verdict::Blocked` is the BARRIER's to give. It carries an
            // [`Unanswered`](crate::consent::Unanswered) — a refusal built with the caller's
            // consents in hand — and this is not where those are read: the next step's barrier is,
            // and it already answers the dialog, waits for the person the caller said was
            // watching, or blocks the run, whichever they declared. A second place that decided
            // about a blocked pane would be exactly the second door
            // [`Readiness`](crate::readiness::Readiness) exists to prevent.
            //
            // What this step does is STOP WAITING, and that is the whole cost: the question is
            // already on the screen, so every further millisecond of patience is spent on a peer
            // that cannot answer it. See [`Over::Asking`].
            (Arrival::Turn(Over::Asking(asking)), _) => match asking {
                Some(question) => format!(
                    "the peer stopped to ASK, so its turn is over: {}{}",
                    question.asked.join(" "),
                    question
                        .selected()
                        .map_or_else(String::new, |choice| format!(
                            " — a bare Enter here would answer {:?}",
                            choice.label
                        )),
                ),
                // ⚠ A REAL CASE AND NOT A GAP: an agent can block on something that is not a
                // numbered list, and the remedy is a person. See `AgentObservation::asking`.
                None => "the peer stopped to ASK and this host cannot read the question as a \
                         menu, so its turn is over and a person has to look"
                    .to_string(),
            },
            // The two ways a step can end with no answer are different findings with different
            // remedies: a pane showing NOTHING is one nobody is listening on, while one that
            // echoed and said no more is a peer that heard and did not reply.
            (Arrival::Nothing, _) => match self.reaction(panes) {
                Reaction::Answered(spoke) => {
                    format!("the pane answered as the step's wait ran out: {spoke:?}")
                }
                Reaction::EchoOnly => {
                    "the stimulus was echoed back and THE PEER SAID NOTHING".to_string()
                }
                Reaction::None => "the pane did not react to the stimulus at all".to_string(),
            },
            // ⚠⚠⚠⚠⚠ **AND IT NAMES WHAT IT HEARD.** This line used to say only *the peer answered*,
            // and when it went red over a `cat` pane — which answers nothing — the rows that had
            // convinced it were gone. An account a reader cannot check is one they have to
            // reproduce, and reproducing a race is what cost this two rounds. See
            // [`Reaction::Answered`].
            (Arrival::Reacted(spoke), _) => {
                format!("the peer answered; no sentinel yet: {spoke:?}")
            }
            _ => "the peer answered; no sentinel yet".to_string(),
        };
        Ok(Step::new(Cost::Bytes(cost), verdict).noting(note))
    }

    /// THE PANE THIS DRIVES, so a run cut short takes the reaction it asked for down with it.
    ///
    /// This plugin types a stimulus and waits for the peer to react, and the peer is a program
    /// somebody else started in a pane this plugin does not own. A run cancelled inside that wait
    /// leaves the peer working on the stimulus — the same residue an
    /// [`Agent`](crate::agent::Agent)'s prompt leaves, in the plugin whose whole purpose is to make
    /// a pane do something.
    fn driving(&self) -> Option<PaneId> {
        Some(self.pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState};
    use crate::readiness::Handback;
    use crate::testing::{STANDIN_READS_TTY, started};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A workspace with one pane running `script`, wrapped as pane-access.
    fn sh_access(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    /// A workspace with one live `cat` pane, wrapped as pane-access.
    fn cat_access(cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        sh_access("cat", cols, rows)
    }

    /// What a pane that cannot react runs: echo off, a readiness marker, then a reader that
    /// discards. The marker is the load-bearing part — see [`await_ready`].
    const DEAF: &str = "stty -echo; printf DEAF-READY; exec cat >/dev/null";

    /// A silent program's argv, and the ONE readiness spec both ways of reaching it are driven
    /// with.
    ///
    /// ⚠⚠ **THE SYMMETRY IS THE CLAIM, SO IT IS A SHARED FUNCTION RATHER THAN A SENTENCE.** Two
    /// gates below start this program two entirely different ways — a shell that is typed at until
    /// it `exec`s, and a pane OPENED running it (`open_pane`'s `cmd`, reached here through
    /// [`PaneLifecycle::spawn`]) — and both converge on this identical value. A prose claim that
    /// one value serves both shapes is a claim; a value neither gate can vary is the fact.
    ///
    /// `tr` is the fixture because it is `cat` with a witness: silent until fed, then provably
    /// itself, because `PING` is not a spelling the pty's echo of `ping` can produce.
    const SILENT_PROGRAM: [&str; 3] = ["tr", "a-z", "A-Z"];

    fn drive_the_silent_program() -> OrchestrationSpec {
        OrchestrationSpec {
            stimulus: "ping".to_string(),
            sentinel: Some("PING".to_string()),
            // No marker at all: the pane's TERMINAL says what is running in it.
            ready_when: Some(ReadyWhen::Runs(SILENT_PROGRAM[0].to_string())),
            // ⚠ BOUNDED, though both fixtures are ready well inside it. Left unbounded this
            // inherits the two-MINUTE default, so a mutation that makes the barrier never clear
            // costs the suite two minutes to report what it can report in fifteen seconds. A
            // gate's failing path has a running time too.
            ready_within: Some(Duration::from_secs(15)),
            may_answer: None,
            attended: Attended::NoOne,
            turn: None,
        }
    }

    fn run(
        access: &WorkspacePaneAccess,
        plugin: &mut Orchestrator,
        guardrails: Guardrails,
    ) -> crate::driver::Outcome {
        run_any(access, plugin, guardrails)
    }

    /// [`run`] over ANY access, for the gates that drive a host with a capability withheld.
    fn run_any(
        access: &dyn PaneAccess,
        plugin: &mut Orchestrator,
        guardrails: Guardrails,
    ) -> crate::driver::Outcome {
        Driver::new(guardrails).run(plugin, access, &crate::run::RunContext::uncancellable())
    }

    #[test]
    fn exhausts_after_max_iterations() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
        assert_eq!(outcome.iterations, 3);
        assert!(outcome.failure.is_none());
    }

    /// ⚠⚠⚠⚠ **A CANCELLED ORCHESTRATOR TYPES NOTHING MORE** — the claim a SHUTDOWN's deadline rests
    /// on, for the plugin the daemon's `run` verb actually drives.
    ///
    /// `RunRegistry::JOIN_DEADLINE` is five seconds because the one thing a run's worker can be
    /// inside that cannot see its cancel flag is a pane write, bounded at 500 ms — **once**. The
    /// *once* is this sentence: no injection may START after the flag is up, or the structural
    /// worst case is `n` writes and the deadline's margin is `10/n` rather than ten. That was read
    /// off the loop rather than measured, and this is the measurement.
    ///
    /// ⚠⚠ Its four siblings already record it (`readiness` twice, `screen`, `ai_loop`) and this was
    /// the injecting path with no ledger — the one the host runs, and the one the shutdown's own
    /// numbers were taken against. The flag rides in ON the first keystroke, so the ordering is a
    /// fact of the double rather than of the scheduler; see [`crate::testing::StopsAtTheKey`].
    ///
    /// # ⚠⚠⚠ WHAT HOLDS IT IS TWO CLAUSES, NOT ONE — measured rather than assumed
    ///
    /// [`Driver`] asks `ended_from_outside` BEFORE a step and again inside the step's `Continue`
    /// arm, and **either one alone keeps this green**: delete the pre-step check and the post-step
    /// one ends the run before a second injection; delete the post-step one and the loop top does.
    /// Only removing BOTH reddens this (the run exhausts its fifty iterations instead, typing
    /// forty-nine more times). That is redundancy rather than a hole — each clause has its own
    /// gates, `driver_ends_cancelled_without_running_a_step` for the first and four others for the
    /// second — but a reader who assumed this gate pins one of them would be wrong, so it is
    /// written down.
    #[test]
    fn a_cancelled_orchestrator_types_nothing_more() {
        let (access, pane) = cat_access(20, 4);
        // ⚠ A sentinel the pane can never show, so the step is INSIDE its wait when the flag lands
        // rather than already finished — a run that had converged would type nothing for a reason
        // that has nothing to do with the cancel.
        let stopping = crate::testing::StopsAtTheKey::nth(access, 1);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("NEVER-SHOWN".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 50,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut orch, &stopping, &stopping.run());

        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "the double's flag is what must have ended this run, or the ledger below is about \
             something else",
        );
        assert!(
            stopping.typed_after_the_stop().is_empty(),
            "⚠⚠⚠⚠ A CANCELLED RUN WENT ON TYPING, so a shutdown can be inside more than one \
             uncancellable pane write and `JOIN_DEADLINE`'s margin is smaller than its doc says \
             it is. It pressed: {:?}",
            stopping.typed_after_the_stop(),
        );
    }

    /// ⚠⚠⚠ **A PEER THAT BLOCKS MID-RUN IS TYPED INTO ANYWAY, AND WHAT IT IS SHOWING IS A MENU.**
    ///
    /// The readiness barrier is LATCHED — `reached` returns early on `seen` — and that is right for
    /// the question it asks: *has the program started?* is answered once and stays answered. But it
    /// is the only thing standing between this loop and the pane, so nothing re-asks a DIFFERENT
    /// question whose answer changes under the run: **is the peer waiting for me, or waiting on
    /// something it asked?**
    ///
    /// An agent that stops to ask — a tool-permission dialog, a trust prompt — shows a BOTTOM-
    /// ANCHORED NUMBERED CHOICE LIST, and a numbered list consumes keystrokes. This loop's every
    /// injection is a stimulus followed by ENTER, and `Question::selected` is documented as *"where
    /// a bare Enter would land, and so the answer a caller gets by doing nothing"*. So the next
    /// iteration does not deliver text to a peer: **it picks whatever option is highlighted.**
    ///
    /// The pane here is `claude`, `Idle` when the run starts and `Blocked` from the first step on —
    /// the shape of an agent that pops a permission dialog while working. The claim is what the run
    /// does with its remaining iterations.
    /// A pane that echoes, supervised by a source that reports the agent BLOCKED once the pane has
    /// been given something — which is what a real one does, since the dialog is a REACTION to the
    /// work — or working forever when `ever_asks` is false.
    ///
    /// ⚠⚠ **KEYED ON THE PANE'S OWN RECORD OF WHAT WAS TYPED INTO IT**, not on a call counter, so
    /// the fixture does not depend on how many times anything happens to look — and so the barrier
    /// genuinely latches on an at-rest peer first, which is the precondition the whole subject
    /// rests on. Borrowed from `a_loop_keeps_typing_into_a_peer_that_stopped_to_ask`, which
    /// established the shape.
    fn peer_that_asks_when_prompted(
        ever_asks: bool,
    ) -> (Arc<Mutex<Workspace>>, WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((60, 12))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("printf 'UP\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 60, 12)
                .expect("spawn pane")
        };
        started(
            &WorkspacePaneAccess::new(Arc::clone(&workspace)),
            pane,
            "UP",
        );
        let source = {
            let workspace = Arc::clone(&workspace);
            Arc::new(move |id: PaneId| {
                let prompted = workspace
                    .lock()
                    .unwrap()
                    .pane(id)
                    .is_some_and(|p| p.pty().echo_trail().contains("ping"));
                // ⚠ THE SUBJECT AND THE CONTROL DIFFER HERE AND NOWHERE ELSE. Both peers are
                // given the same stimulus, both stop being at rest when they get it, and only one
                // of them raises a dialog about it. Everything downstream — the pane, the
                // contract, the guardrails, the barrier — is identical.
                let state = match (prompted, ever_asks) {
                    (false, _) => sprag_detect::AgentState::Idle,
                    (true, true) => sprag_detect::AgentState::Blocked,
                    (true, false) => sprag_detect::AgentState::Working,
                };
                Some(crate::access::AgentObservation {
                    state,
                    agent: Some("claude".to_string()),
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq: u64::from(prompted) + 1,
                    asked_seq: u64::from(prompted) + 1,
                    reports: 0,
                    asked: None,
                    said: None,
                    said_seq: 0,
                    noticed: None,
                    transcript: None,
                    settling: crate::access::Settling::Nothing,
                    reporter: crate::access::ReporterVoice::Speaking,
                    asking: (state == sprag_detect::AgentState::Blocked).then(|| {
                        sprag_detect::Question {
                            asked: vec!["Do you want to edit lib.rs?".to_string()],
                            choices: vec![
                                sprag_detect::Choice {
                                    number: 1,
                                    label: "Yes".to_string(),
                                    selected: true,
                                },
                                sprag_detect::Choice {
                                    number: 2,
                                    label: "No".to_string(),
                                    selected: false,
                                },
                            ],
                        }
                    }),
                })
            })
        };
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));
        (workspace, access, pane)
    }

    /// The spec both halves of the measurement below are driven with — ONE value, so the only
    /// difference between the two runs is the peer.
    ///
    /// ⚠ `Turn::lasting(Settles, None)` is the contract with NO bound of its own: *wait for my
    /// peer*, bounded by the run's clock alone. It is the spelling an outer AI loop wants, because
    /// only the run knows how long a session may take — and it is the spelling under which a
    /// question used to cost the most, since the step's patience became `Duration::MAX`.
    fn wait_for_the_agents_turn() -> OrchestrationSpec {
        OrchestrationSpec {
            stimulus: "ping".to_string(),
            sentinel: None,
            ready_when: Some(crate::readiness::ReadyWhen::Settles("claude".to_string())),
            ready_within: Some(Duration::from_secs(5)),
            may_answer: None,
            attended: Attended::NoOne,
            turn: Turn::lasting(crate::completion::DoneWhen::Settles, None),
        }
    }

    /// ⚠⚠⚠ **A RUN WHOSE PEER STOPS TO ASK STOPS WAITING — measured against the control that is
    /// the same run with the same peer NOT raising a dialog.**
    ///
    /// This is the unit measurement in `completion.rs` at the level a caller actually meets, and
    /// the level the outer AI loop drives: a whole [`Driver`] run, its own clock, its own reported
    /// outcome.
    ///
    /// The two runs differ in ONE fact — whether the peer raises a dialog about the stimulus — and
    /// they differ in what they cost by the whole run:
    ///
    /// * **the peer that ASKS**: the step's wait ends the moment the question is up, so the next
    ///   step's barrier reports it and the run ends `blocked` in milliseconds.
    /// * **the peer that stays WORKING** (the control): nothing ends the wait, so the run spends
    ///   its entire duration ceiling and reports `exhausted: duration`.
    ///
    /// # ⚠⚠ What makes the control impossible rather than merely slower
    ///
    /// R358's rule about a gate that asks WHICH of two ceilings fired. The control's peer is
    /// `Working` forever and its pane produces nothing after the echo, so no term of the step's
    /// union can ever be satisfied: the run CANNOT end any way but on its clock. The subject's
    /// peer is blocked from the moment it is prompted, so its wait CANNOT run long. The gap
    /// between them is arithmetic, not luck, and a slower box widens it.
    ///
    /// ⚠ Before [`Over::Asking`] existed **both** of these were the control: an ask was invisible
    /// to the step's wait, so the run that is now milliseconds was the run that is now three
    /// seconds. That is the measurement, kept as a live comparison rather than as a number in a
    /// comment.
    #[test]
    fn a_run_stops_waiting_the_moment_its_peer_stops_to_ask() {
        /// The run's whole clock, which is also the step's patience under a contract with no bound
        /// of its own. Long enough that spending it is unmistakable.
        const CEILING: Duration = Duration::from_secs(3);
        /// What the subject has to beat. A third of the ceiling: far outside any polling interval,
        /// far inside the ceiling.
        const AT_ONCE: Duration = Duration::from_secs(1);

        let guardrails = || Guardrails {
            max_iterations: 4,
            max_cost: None,
            max_duration: Some(CEILING),
        };

        // ── THE CONTROL: the peer takes the stimulus and never comes back ──
        let (_workspace, access, pane) = peer_that_asks_when_prompted(false);
        let mut orch = Orchestrator::new(pane, wait_for_the_agents_turn());
        let started_at = Instant::now();
        let control = run(&access, &mut orch, guardrails());
        let control_cost = started_at.elapsed();
        assert!(
            matches!(control.state, OutcomeState::Exhausted(Ceiling::Duration)),
            "⚠ THE CONTROL FAILED, so the comparison below is not the one this gate names: a peer \
             that neither answers nor asks must leave the run to end on its own clock. Got \
             {control:?}",
        );
        assert!(
            control_cost >= CEILING,
            "and it must genuinely have SPENT that clock, or `at once` below is not a \
             discriminator: {control_cost:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // ── THE SUBJECT: the same peer, same pane, same contract — it raises a dialog ──
        let (_workspace, access, pane) = peer_that_asks_when_prompted(true);
        let mut orch = Orchestrator::new(pane, wait_for_the_agents_turn());
        let started_at = Instant::now();
        let outcome = run(&access, &mut orch, guardrails());
        let asking_cost = started_at.elapsed();
        assert!(
            matches!(outcome.state, OutcomeState::Blocked(_)),
            "⚠⚠ the run must REPORT that its peer is asking. `exhausted` tells a reader to raise a \
             budget and it is what this run answered before the turn's end could see a question — \
             about a pane where the one true thing is that somebody has to answer one. Got \
             {outcome:?}",
        );
        assert!(
            asking_cost < AT_ONCE,
            "⚠⚠⚠ THE NUMBER, AT RUN LEVEL: the same run against the same peer costs \
             {control_cost:?} when nothing can end the wait and {asking_cost:?} when the peer \
             stops to ask. The whole difference is that the end of a turn now reads the question \
             the start of one has read since R366; without it this run spends the ceiling too, and \
             for an outer AI loop the ceiling is how long a session may take",
        );
        assert!(
            outcome.iterations >= 2,
            "⚠ and it did not get there by refusing to start: the barrier passed, a stimulus went \
             in, and it is the step AFTER the ask that reports it. {outcome:?}",
        );

        let typed = access
            .input_trail()
            .and_then(|echo| echo.pane_recent_input(pane))
            .unwrap_or_default();
        assert_eq!(
            typed.matches("ping").count(),
            1,
            "⚠⚠ EXACTLY ONE stimulus, still. Ending the wait early must not turn into taking more \
             turns: into a numbered choice list a stimulus is not text delivery, it is SELECTION, \
             and each one ends with the Enter that confirms whatever option the agent highlighted. \
             Typed {typed:?}, outcome {outcome:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    #[test]
    fn a_loop_keeps_typing_into_a_peer_that_stopped_to_ask() {
        // Echoes what it is given, so what the pane RECEIVED is readable afterwards.
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("printf 'UP\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let plain = WorkspacePaneAccess::new(Arc::clone(&workspace));
        started(&plain, pane, "UP");

        // ⚠⚠ THE PEER BLOCKS WHEN IT IS GIVEN SOMETHING, which is what a real one does: the
        // dialog is a REACTION to the work. Keyed on the pane's own record of what was typed into
        // it rather than on a call counter, so the fixture does not depend on how many times the
        // barrier happens to look — and so the barrier genuinely LATCHES on an at-rest peer first,
        // which is the precondition the whole subject rests on.
        let source = {
            let workspace = Arc::clone(&workspace);
            Arc::new(move |id: PaneId| {
                let asking = workspace
                    .lock()
                    .unwrap()
                    .pane(id)
                    .is_some_and(|p| p.pty().echo_trail().contains("ping"));
                Some(crate::access::AgentObservation {
                    state: if asking {
                        sprag_detect::AgentState::Blocked
                    } else {
                        sprag_detect::AgentState::Idle
                    },
                    agent: Some("claude".to_string()),
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq: if asking { 2 } else { 1 },
                    asked_seq: if asking { 2 } else { 1 },
                    reports: 0,
                    asking: None,
                    asked: None,
                    said: None,
                    said_seq: 0,
                    noticed: None,
                    transcript: None,
                    settling: crate::access::Settling::Nothing,
                    reporter: crate::access::ReporterVoice::Speaking,
                })
            })
        };
        let access =
            WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(source));

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                // The STRONGEST barrier this product has — the agent must be at rest and named.
                ready_when: Some(crate::readiness::ReadyWhen::Settles("claude".to_string())),
                ready_within: Some(Duration::from_secs(5)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: None,
            },
        );

        let typed = access
            .input_trail()
            .and_then(|echo| echo.pane_recent_input(pane))
            .unwrap_or_default();
        assert!(
            matches!(outcome.state, OutcomeState::Blocked(_)),
            "⚠⚠⚠ the run must REPORT that its peer is asking. `exhausted` tells a reader to raise \
             a budget and `failed` tells them to fix something; neither says the one thing that is \
             true — somebody has to answer a question. Outcome: {outcome:?}",
        );
        let fed = typed.matches("ping").count();
        assert_eq!(
            fed, 1,
            "⚠⚠⚠ EXACTLY ONE stimulus — the one sent while the peer was still at rest. Every \
             iteration after it met a pane that had stopped to ask, and into a numbered choice \
             list a stimulus is not text delivery, it is SELECTION: each ends with Enter, which \
             confirms whatever option the agent had highlighted. Typed {fed}. \
             Outcome: {outcome:?}, typed: {typed:?}",
        );
    }

    #[test]
    fn converges_on_sentinel() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("ping".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 10,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
        assert!(
            outcome.iterations >= 1,
            "iterations: {}",
            outcome.iterations
        );
    }

    #[test]
    fn converges_on_a_wrapped_sentinel() {
        // A 4-column pane wraps the 6-char echo across rows; the collapsed
        // match still finds "abcdef".
        let (access, pane) = cat_access(4, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "abcdef".to_string(),
                sentinel: Some("abcdef".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 10,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Converged);
    }

    #[test]
    fn cost_budget_also_terminates() {
        let (access, pane) = cat_access(20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(), // "ping" + Enter = 5 bytes/step
                sentinel: None,
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: u32::MAX,
                max_cost: Some(Cost::Bytes(12)),
                max_duration: None,
            },
        );
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Cost));
        assert!(
            matches!(outcome.cost, Some(Cost::Bytes(n)) if n >= 12),
            "cost: {:?}",
            outcome.cost
        );
    }

    /// ⚠⚠ **A PANE IS NOT READY WHEN IT IS OPEN**, and a run told what ready looks like spends no
    /// turn before it.
    ///
    /// The pane here is born a shell and becomes the peer a second later — the ordinary shape of
    /// *open a pane, start `claude` in it, drive it*. A run that starts immediately injects into
    /// the SHELL, which executes the stimulus as a command; by the time the peer exists its turns
    /// are gone and the guardrails have counted them.
    ///
    /// Both halves, because either alone is weak: the run must CONVERGE (so the wait ended and the
    /// driving worked), and the STAND-IN SHELL must never have been fed — it says so itself.
    ///
    /// ⚠⚠ THE FIXTURE HAD TO BE REBUILT TWICE, AND THE SECOND TIME IS WHY IT MEASURES ANYTHING.
    /// Its first form let the pane merely SLEEP before becoming the peer, and a mutation that
    /// ignored `ready_when` entirely still passed: nothing consumed the early stimulus, so it sat
    /// in the pty buffer and the peer read it when it started. A pane that is not ready has to be
    /// one that EATS what it is given, which is what a real shell does with a stimulus meant for
    /// something else.
    ///
    /// The second form said `while read early; …&` and **still ate nothing**, for a reason no
    /// reading of it shows: a background job of a NON-INTERACTIVE shell gets its stdin from
    /// `/dev/null`, so the stand-in was reading end-of-file while the injection sat in the pty
    /// exactly as before. Both halves passed for the same reason they had passed before the first
    /// rebuild. [`STANDIN_READS_TTY`] is what fixes it — reopening the controlling terminal is the
    /// only way a background reader here can be given the pane's own input.
    #[test]
    fn a_run_told_what_ready_looks_like_injects_nothing_before_it() {
        // A stand-in shell that consumes and NAMES anything typed at it, for two seconds — longer
        // than this run can take unaided (two turns, each floored by the 500ms observe). Then it
        // is killed, the peer announces itself and `exec`s, so what answers afterwards is
        // unambiguously the peer.
        let (access, pane) = sh_access(
            &format!(
                "while read early; do echo \"SHELL-ATE $early\"; done {STANDIN_READS_TTY} & \
                 sleep 2; kill $! 2>/dev/null; printf 'PEER-UP\\n'; \
                 exec sh -c 'while read l; do echo \"PEER-SAW $l\"; done'"
            ),
            40,
            8,
        );
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-SAW ping".to_string()),
                ready_when: Some(ReadyWhen::Prints("PEER-UP".to_string())),
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        // ⚠ WHICH OF THE TWO ASSERTIONS BELOW FIRES DEPENDS ON THE STAND-IN, and both were
        // measured against a run with the barrier removed. A stand-in that ANSWERS (this one names
        // what it ate) ends each observe at once, so a barrier-less run burns every turn in
        // milliseconds and never reaches the peer — the CONVERGED half fails, in 70ms. A stand-in
        // that merely swallows would floor each step instead, the run would outlive it, and the
        // SHELL-ATE half is what catches it. Keep both: they are the same defect seen from the two
        // ends, and neither covers the other.
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the run waited for the peer to come up and then drove it",
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("SHELL-ATE"),
            "NOTHING may have been injected while the pane was still the stand-in shell — every \
             `SHELL-ATE` is a turn the run spent on a peer that did not exist yet: {screen:?}",
        );
    }

    /// ⚠⚠ **A MARKER YOU TYPED IS NEVER EVIDENCE, AND THE ANSWER DOES NOT DEPEND ON TIMING.**
    ///
    /// A pane echoes the command line that started the program, so a marker appearing in that line
    /// is on screen before the program exists — and the echo is ordinary output once it reaches the
    /// grid. Under the whole-screen match the barrier cleared on it in 50 MILLISECONDS and the run
    /// spent every turn on the shell (`…exec cat'$ pingATE pingpingATE ping`).
    ///
    /// The generation baseline alone did not close it, it only moved the failure: the echo arrives
    /// ASYNCHRONOUSLY, so whether it counted as "produced after arming" depended on scheduling.
    /// **The same call converged or fed the shell depending on how the machine was loaded.**
    ///
    /// So the pane remembers what was written into it and such a marker is refused outright. The
    /// run ends `NeverReady` NAMING it — an ambiguous marker answered honestly and identically
    /// every time — instead of driving something that was never listening.
    ///
    /// ⚠ BOTH HALVES START THE RUN THE SAME WAY AND DIFFER ONLY IN THE WAIT, which is the point:
    /// the echo having landed or not must not change the answer.
    #[test]
    fn a_marker_that_is_in_what_the_caller_typed_is_never_evidence() {
        // The command line MENTIONS the banner, because the caller wrote both.
        let started = format!(
            "sh -c 'while read e; do echo \"ATE $e\"; done {STANDIN_READS_TTY} & sleep 2; \
             kill $! 2>/dev/null; printf \"TOOL-UP\\n\"; exec cat'"
        );
        let drive = |wait_for_the_echo: bool| {
            let (access, pane) = sh_access("exec sh", 80, 10);
            let mut typed = KeyStroke::text(&started);
            typed.push(KeyStroke::named("Enter"));
            let _typed = access.inject(pane, &typed).expect("start the tool");
            if wait_for_the_echo {
                let start = std::time::Instant::now();
                while start.elapsed() < Duration::from_secs(5)
                    && !access
                        .pane_collapsed(pane)
                        .is_some_and(|text| text.contains("TOOL-UP"))
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let mut orch = Orchestrator::new(
                pane,
                OrchestrationSpec {
                    stimulus: "ping".to_string(),
                    sentinel: None,
                    ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                    ready_within: Some(Duration::from_millis(400)),
                    may_answer: None,
                    attended: Attended::NoOne,
                    turn: None,
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut orch, &access, &RunContext::uncancellable());
            let screen = access.pane_collapsed(pane).unwrap_or_default();
            (outcome, screen)
        };

        for waited in [true, false] {
            let (outcome, screen) = drive(waited);
            // ⚠ `instead` is deliberately NOT asserted here: this fixture's pane is mid-`exec` at
            // the moment the barrier gives up, so the job that owns its terminal is the starting
            // shell or the program it became depending on the clock. Pinning it would make a
            // diagnostic field decide a gate about REFUSING AN ECHO, which is a different claim.
            assert!(
                matches!(
                    &outcome.failure,
                    Some(PaneError::NeverReady { wanted, .. })
                        if wanted == &ReadyWhen::Prints("TOOL-UP".to_string()),
                ),
                "an ambiguous marker is refused and NAMED, whether or not its echo had landed \
                 (waited: {waited}): {outcome:?} {screen:?}",
            );
            assert!(
                !screen.contains("ATE ping"),
                "and NOTHING was typed at the stand-in that was still there (waited: {waited}): \
                 {screen:?}",
            );
        }
    }

    /// ⚠⚠ **AND A MARKER THE PROGRAM COMPOSES CONVERGES WITH NO WAIT AT ALL** — the other half,
    /// and the one that says the remedy above is not simply "refuse everything".
    ///
    /// The banner here is assembled by the program (`printf "PEER-%s" UP`), so it cannot appear in
    /// the line the caller typed and the echo cannot be mistaken for it — by CONSTRUCTION, not by
    /// timing. The run starts in the same breath as the write, with no quiescing and no sleep, and
    /// still waits for the peer rather than for its own echo.
    #[test]
    fn a_marker_the_program_composes_needs_no_wait_before_the_run() {
        let (access, pane) = sh_access("exec sh", 80, 10);
        let started = format!(
            "sh -c 'while read e; do echo \"ATE $e\"; done {STANDIN_READS_TTY} & sleep 2; \
             kill $! 2>/dev/null; printf \"PEER-%s\\n\" UP; \
             exec sh -c \"while read l; do echo SAW \\$l; done\"'"
        );
        let mut typed = KeyStroke::text(&started);
        typed.push(KeyStroke::named("Enter"));
        let _typed = access.inject(pane, &typed).expect("start the tool");
        // ⚠ NO WAIT, deliberately — the echo of the line above is still in flight.

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("SAW ping".to_string()),
                ready_when: Some(ReadyWhen::Prints("PEER-UP".to_string())),
                ready_within: Some(Duration::from_secs(10)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(20)),
            },
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("ATE ping"),
            "no turn may be spent on the stand-in that was still there: {screen:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "and the peer is driven once it exists: {screen:?}",
        );
    }

    /// ⚠⚠ **AND `shows` IS NOT THE SAME BUG KEPT AROUND** — it is the answer to the other question,
    /// and it is the ONLY answer there.
    ///
    /// A program already running has already said everything it is going to say until it is fed.
    /// This pane prints its prompt and then goes quiet; a barrier demanding NEW output would wait
    /// for ever against it, which is why the whole-screen match had to stay reachable rather than
    /// be tightened away.
    ///
    /// The pause is what makes this measure anything: the banner is over and done with before the
    /// run looks, so a `Prints` barrier here would have nothing to find.
    #[test]
    fn a_program_already_at_its_prompt_is_ready_by_what_it_shows() {
        let (access, pane) = sh_access(
            "printf 'REPL-READY\n'; exec sh -c 'while read l; do echo \"GOT $l\"; done'",
            40,
            8,
        );
        // Wait for the banner to be OVER, so nothing new arrives after the barrier arms.
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains("REPL-READY"))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("GOT ping".to_string()),
                ready_when: Some(ReadyWhen::Shows("REPL-READY".to_string())),
                ready_within: Some(Duration::from_millis(500)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: Some(Duration::from_secs(20)),
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "a pane whose program is already at its prompt is ready by what it SHOWS — demanding \
             new output would wait for a line this program will never print unasked: {outcome:?}",
        );
    }

    /// ⚠⚠ **A PROGRAM THAT PRINTS NOTHING IS WAITED FOR BY WHAT OWNS THE TERMINAL** — the case the
    /// two screen kinds cannot answer at all, and the reason [`ReadyWhen::Runs`] exists.
    ///
    /// The fixture is the ordinary AI-loop shape with one change that removes every marker: the
    /// program that finally comes up **says nothing when it starts**. `tr` is `cat` with a witness
    /// — silent until fed, then provably itself, because `PING` is not a spelling the pty's echo of
    /// `ping` can produce. Most things this drives are in that class (a REPL launched quiet, a
    /// relay, any tool that speaks only when spoken to), and for all of them `Prints` waits for a
    /// line that will never come and `Shows` has nothing to look for but the caller's own echo.
    ///
    /// **THREE HALVES, and the first is the control that makes the other two mean anything:**
    ///
    /// 1. the pane produces NOTHING between the stand-in dying and the program being ready — so the
    ///    set of markers a caller could have named is empty, and this is a gap in the QUESTION
    ///    rather than a marker chosen badly;
    /// 2. the stand-in was never fed — every `ATE` is a turn spent on a shell;
    /// 3. the run CONVERGED, so the wait ended and the driving worked.
    ///
    /// ⚠ MUTATION-MEASURED, and the order of the last two is what the measurement bought. With the
    /// `Runs` arm answering `true` unconditionally the run drives the stand-in, which is half 2;
    /// with it answering `false` the pane is never ready, nothing is ever injected and only half 3
    /// can see it. **Asserted screen-first for that reason** — the reverse order was tried and half
    /// 3 fired on BOTH mutations, hiding the more specific diagnosis behind a generic one.
    #[test]
    fn a_program_that_prints_nothing_is_ready_when_it_owns_the_terminal() {
        // The stand-in eats for two seconds — longer than this run takes unaided — then `exec`s a
        // program that prints NOT ONE BYTE until it is spoken to.
        let (access, pane) = sh_access(
            &format!(
                "while read early; do echo \"ATE $early\"; done {STANDIN_READS_TTY} & \
                 sleep 2; kill $! 2>/dev/null; exec tr a-z A-Z"
            ),
            40,
            8,
        );
        // ⚠ HALF 1, THE CONTROL — read BEFORE the run, while the stand-in is still there. A silent
        // program cannot be waited for by anything that reads the screen, and this is that claim
        // measured rather than asserted: the pane is blank now and stays blank until it is driven.
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "the fixture's program must print NOTHING on startup, or this gate is about a marker \
             the caller chose badly rather than about a program that has none",
        );
        let mut orch = Orchestrator::new(pane, drive_the_silent_program());
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("ATE"),
            "NOTHING may have been injected while the pane's terminal still belonged to the \
             stand-in shell — every `ATE` is a turn the run spent on a program that had not \
             started: {screen:?}",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the run waited for the program to take the terminal and then drove it: {outcome:?}",
        );
    }

    /// ⚠⚠ **A PANE OPENED RUNNING THE PROGRAM IS READY BY THE SAME VALUE, WITH NO WINDOW AT ALL.**
    ///
    /// `open_pane` has taken a `cmd` since the daemon's argv path was fixed, and NOTHING PREFERRED
    /// IT: every loop gate in this workspace opened a shell and typed into it, which is how the
    /// echo hazard got three rounds of attention. This is the other shape, and the point is that it
    /// needs no new spelling — [`drive_the_silent_program`] is shared with the gate above verbatim,
    /// so the two cannot drift into two answers.
    ///
    /// **Opening the pane running the program is the shape to prefer**, and the reason is visible
    /// here rather than argued: there is no shell to be typed at, so there is no window in which an
    /// injection can be eaten, and no echo of a starting command line for a marker to be confused
    /// with. The barrier is not a wait — it is a confirmation that the pane is what the caller
    /// asked for. A `Prints` marker could not make that claim at all: the program says nothing, so
    /// on this shape it would wait out its bound and fail.
    ///
    /// ⚠ THE THIRD HALF IS THE ONE THAT MAKES THIS MORE THAN A CONVERGENCE TEST. A run that
    /// converged might still have waited seconds for a barrier it should have cleared at once, so
    /// the elapsed time is asserted too — well under the 500ms floor one observe step costs, which
    /// is the cheapest bound that could not be met by accident.
    #[test]
    fn a_pane_opened_running_the_program_is_ready_by_the_same_value() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let access = WorkspacePaneAccess::new(workspace);
        let argv: Vec<String> = SILENT_PROGRAM.iter().map(|a| (*a).to_string()).collect();
        let pane = access
            .lifecycle()
            .expect("this access spawns panes")
            .spawn(&argv, 40, 8)
            .expect("open a pane RUNNING the program, rather than a shell to type it into");

        let mut ready = crate::readiness::Readiness::new(
            drive_the_silent_program().ready_when,
            drive_the_silent_program().ready_within,
            None,
            Attended::NoOne,
        );
        let started = std::time::Instant::now();
        let reached = ready
            .reached(&access, pane, &RunContext::uncancellable())
            .expect("a pane opened running the program is ready for it");
        assert_eq!(
            reached,
            crate::readiness::Reached::Yes,
            "the pane IS the program — the barrier confirms it rather than waiting for it",
        );

        let mut orch = Orchestrator::new(pane, drive_the_silent_program());
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 6,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "and driving it works, off the identical spec the shell-and-type gate uses: \
             {outcome:?}",
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a pane that is ALREADY the program must not be waited for — this shape has no \
             starting window, and paying one would mean the barrier is watching the wrong thing",
        );
    }

    /// ⚠⚠ **NO AMOUNT OF TYPING THE NAME MAKES A PANE READY** — the discriminator against both
    /// screen kinds, and the structural claim [`ReadyWhen::Runs`] is worth having for.
    ///
    /// `Shows` is satisfied by any text on the pane, and the pty puts the caller's own command line
    /// there before the program exists; `Prints` had to grow an echo trail, a damage baseline and a
    /// refusal rule to survive the same input, and still answers only for programs that speak.
    /// This kind is not a better predicate over the screen — it does not read the screen, so the
    /// hazard is not narrowed, it is ABSENT.
    ///
    /// The fixture types the name as LOUDLY as a pane can carry it: as a command that echoes it
    /// back, so it is both in what was typed AND in what the program printed, freshly, after the
    /// barrier armed. `Shows` would clear on it and so would `Prints`.
    ///
    /// ⚠ The pane runs `cat`, so the barrier's answer is *"a job named `tr` never owned this
    /// terminal"* — and the failure NAMES what did, which is the correction a caller who guessed
    /// the wrong program name needs.
    #[test]
    fn typing_a_program_name_at_a_pane_never_makes_it_ready() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        // `cat` echoes: after this the word is in the echo trail AND on the screen as fresh output.
        let mut typed = KeyStroke::text("tr");
        typed.push(KeyStroke::named("Enter"));
        let _typed = access.inject(pane, &typed).expect("type the name");
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .matches("tr")
                .count()
                < 2
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .matches("tr")
                .count()
                >= 2,
            "the fixture must get the name onto the screen TWICE — the pty's echo and the \
             program's copy — or it has not put the screen kinds in a position to be fooled",
        );

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Runs("tr".to_string())),
                ready_within: Some(Duration::from_millis(300)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // Far longer than the readiness bound, so the RUN's clock provably cannot end this.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane running `cat` is not ready for `tr`, however many times the word is on its \
             screen: {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Runs("tr".to_string()),
            "cat",
            "and the failure NAMES what owned the terminal instead, which is the whole correction \
             for a caller who guessed the program's name wrong",
        );
    }

    /// ⚠⚠ **A READINESS THAT NEVER COMES STOPS THE RUN AND SAYS WHAT IT WAITED FOR** — the other
    /// half, and the one that decides whether the argument is a bound or a hope.
    ///
    /// Driving on would inject into whatever IS there and report turns against a peer that was
    /// never listening. The run fails instead, naming the text, in a sentence rather than a Rust
    /// variant.
    #[test]
    fn a_readiness_that_never_comes_ends_the_run_naming_what_it_waited_for() {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Prints("NEVER-PRINTED".to_string())),
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: Some(Duration::from_millis(300)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "the run's own clock is what bounds waiting to be ready — not a number the plugin \
             invented, and not the turn ceiling",
        );
        assert_eq!(
            outcome.iterations, 1,
            "and it spent ONE step doing it rather than burning the turn budget against a pane \
             that was never ready",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "not one byte was injected into a pane that never became ready",
        );
    }

    /// ⚠⚠ **A PANE THAT NEVER COMES UP FAILS THE RUN AND NAMES WHAT IT WAITED FOR** — the arm that
    /// had no test at all, and could not have had one until the wait's bound became the caller's.
    ///
    /// The gate above ends by the RUN's clock, which is a different finding: *the run was out of
    /// time* says nothing about the pane. This one is *this pane never came up*, and it is the
    /// answer a caller needs, because it is the one that names the marker they got wrong.
    ///
    /// ⚠ It was unreachable rather than untested. `ready_within` was hard-wired to two minutes, so
    /// any gate short enough to run had a run deadline shorter than the readiness bound, and
    /// [`Waited::Stopped`] won every time — `NeverReady` was constructed in one place and read by
    /// nothing. A bound the CALLER names is what makes the arm reachable in 200ms, and it is also
    /// the right product answer: how long a program takes to start is the caller's knowledge.
    ///
    /// The three halves are three different claims: the run FAILED (not exhausted), it carries the
    /// TYPED cause naming the marker, and NOTHING was injected into the pane that never came up.
    #[test]
    fn a_pane_that_never_becomes_ready_fails_the_run_and_names_the_marker() {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: Some(ReadyWhen::Prints("NEVER-PRINTED".to_string())),
                ready_within: Some(Duration::from_millis(200)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // ⚠ FAR LONGER than the readiness bound, so the run's own clock provably cannot be
            // what ends this — that is the other gate, and it reaches a different arm.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut orch, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane that never becomes ready is a FAILURE of the run, not a ceiling it reached: \
             {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Prints("NEVER-PRINTED".to_string()),
            // ⚠ The pane runs `exec cat`, so `cat` IS the job that owns its terminal — a caller
            // reading this learns the pane was never going to print, which is the correction, and
            // it arrives without them reading the screen.
            "cat",
            "and the cause is typed, carries the QUESTION the caller asked, and names what the \
             pane was running instead",
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "not one byte was injected into a pane that never became ready",
        );
    }

    /// ⚠⚠ **THE FAILURE AN AGENT READS IS A SENTENCE**, and the list it was derived from was typed
    /// by hand.
    ///
    /// A run's `failure` is published to its caller as this text
    /// ([`plugins.rs`](../../sprag_host/plugins/index.html) does `.map(ToString::to_string)`), and
    /// it was `format!("{e:?}")` until R358 — `Write("Broken pipe (os error 32)")`, a Rust variant
    /// name and its debug payload, reaching the one reader who cannot look up what a variant means.
    ///
    /// The fix had no test, so a reverted `to_string()` would have broken nothing and the leak
    /// would have come back unnoticed. Its gate then said it was *"derived from a list of every
    /// variant"* while holding **an array of five literals** — a hand-written list is the one a new
    /// thing is left out of, and this one guards the sentence an agent reads. It walks
    /// [`PaneError::ALL`] now, so a SIXTH variant is covered the moment it is declared and cannot be
    /// added without naming an inhabitant.
    ///
    /// ⚠ AND THE SENTENCES MUST BE DISTINCT. A catch-all message would satisfy every shape claim
    /// below while telling five different failures apart from none of them, so the count of
    /// sentences is asserted against the count of variants — the one thing a walker measures that
    /// a per-item check cannot.
    #[test]
    fn every_pane_failure_reads_as_a_sentence_rather_than_a_rust_variant() {
        let every = PaneError::ALL;
        let distinct: std::collections::BTreeSet<String> =
            every.iter().map(ToString::to_string).collect();
        assert_eq!(
            distinct.len(),
            every.len(),
            "two failures read as the SAME sentence, so the reader cannot tell them apart: \
             {distinct:?}",
        );
        for error in &every {
            let said = error.to_string();
            let debug = format!("{error:?}");
            assert_ne!(
                said, debug,
                "the published text is the DEBUG form, which is the leak itself",
            );
            // A variant name is `CamelCase` with no space; a sentence has spaces and starts lower.
            assert!(
                said.contains(' ') && said.starts_with(char::is_lowercase),
                "a failure an agent reads must be prose, not {said:?}",
            );
            assert!(
                !said.contains('(') || !said.contains("::"),
                "and must not carry a Rust path: {said:?}",
            );
        }
        // The PAYLOAD has to survive into the sentence, or the prose is prose about nothing — this
        // is the half that a "polite" catch-all message would silently fail.
        assert!(
            PaneError::Write("Broken pipe (os error 32)".to_string())
                .to_string()
                .contains("Broken pipe (os error 32)"),
            "the cause the operating system gave must reach the reader",
        );
        let never_ready = PaneError::NeverReady {
            wanted: ReadyWhen::Runs("claude".to_string()),
            instead: PaneDoing::Job(JobLeader::known_as("sh".to_string())),
            already_showing: false,
        }
        .to_string();
        assert!(
            never_ready.contains("claude"),
            "a readiness that never came must name what it waited for, or the caller cannot tell \
             which marker they got wrong: {never_ready:?}",
        );
        // ⚠⚠ AND WHAT THE PANE WAS DOING INSTEAD, which is the half that turns a two-minute
        // mystery into a correction. A caller who waited for `claude` against a pane still sitting
        // at a shell learns BOTH facts from one sentence.
        assert!(
            never_ready.contains("sh"),
            "and what owned the terminal instead: {never_ready:?}",
        );
        assert!(
            PaneError::UnknownPane(PaneId(7)).to_string().contains('7'),
            "and an unknown pane must name the id that was asked for",
        );

        // ⚠⚠ AND EVERY [`PaneDoing`] ARM, for the same reason and from its own `ALL`: the
        // diagnostic half is a CLAUSE that continues the failure's sentence, so each must either
        // continue it or say nothing. `Unknown` says nothing ON PURPOSE — a host with no view of
        // the process table has no business appending a guess — and that is exactly the arm a
        // "must be non-empty" check would have got wrong.
        for doing in &PaneDoing::ALL {
            let clause = doing.to_string();
            assert!(
                clause.is_empty() || clause.starts_with("; "),
                "a diagnostic that does not continue the sentence it is appended to reads as two \
                 fragments: {clause:?}",
            );
            // ⚠⚠ AND THE ACCESSOR AGREES WITH THE SENTENCE. `PaneDoing::leader` is how a caller
            // asks the diagnostic a QUESTION instead of comparing it to a spelling, so an arm
            // whose clause names a program that owned the terminal must hand that program over,
            // and one whose clause names none must hand over nothing. Two ways of saying the same
            // fact drifting apart is precisely what put a macOS caller's `/bin/sh` pane in the
            // sentence as `"bash"`.
            assert_eq!(
                doing.leader().is_some(),
                clause.contains("belonged to"),
                "the sentence and the accessor disagree about whether a program owned the \
                 terminal: {clause:?}",
            );
            let whole = PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: doing.clone(),
                already_showing: false,
            }
            .to_string();
            assert!(
                whole.contains("claude") && whole.starts_with(char::is_lowercase),
                "and the whole failure stays one sentence with the question in it: {whole:?}",
            );
        }
    }

    /// ⚠⚠ **A PEER THAT ANSWERS IS WAITED FOR; ITS OWN ECHO IS NOT AN ANSWER.**
    ///
    /// A pty in cooked mode echoes what is injected before the program has read a byte of it. If
    /// that echo satisfies the observe-wait, then EVERY turn against EVERY ordinary pane ends in
    /// microseconds, the screen is judged before the peer has said anything, and the loop takes
    /// another turn — spamming a peer that was already thinking. `max_iterations` then bounds a
    /// run that never waited for one reply.
    ///
    /// The peer here answers in 200ms, comfortably inside one step's [`OBSERVE_TIMEOUT`]. A loop
    /// that waits for its peer converges on the FIRST turn. A loop that races its own echo burns
    /// all three turns before the answer lands and reports `exhausted` about a peer that replied.
    #[test]
    fn a_turn_waits_for_the_peer_and_not_for_the_echo_of_what_it_typed() {
        // Reads a line, thinks, then answers. The kernel echoes the injected line long before the
        // `sleep` is over, which is exactly the difference under test.
        let (access, pane) = sh_access(
            "while read line; do sleep 0.2; echo PEER-REPLIED; done",
            40,
            8,
        );
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-REPLIED".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers well inside one step's observe timeout, so a loop that waits for it \
             converges; this run gave up after {} turns against a peer that was replying",
            outcome.iterations,
        );
        assert_eq!(
            outcome.iterations, 1,
            "and it converges on the FIRST turn — a second turn means the first was judged on a \
             screen holding nothing but the echo of what it had just typed, and the peer was \
             prompted again while it was still answering",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A PANE THAT ONLY ECHOES MUST NEVER READ AS A PEER THAT ANSWERED — AT ANY INSTANT,
    /// NOT MERELY AT THE ONE A STEP HAPPENS TO SAMPLE.**
    ///
    /// # ⚠⚠⚠⚠⚠ Found in CI as a flake, and given a rate before it was given a fix
    ///
    /// `sprag-mcp`'s `an_agent_starts_a_bounded_loop_and_reads_how_it_ended` drives a `cat` pane
    /// through the real daemon and demands the journal say *"the stimulus was echoed back and THE
    /// PEER SAID NOTHING"*. On 2026-08-24 it failed on macOS with *"the peer answered; no sentinel
    /// yet"* — and, measured on linux, **4 runs in 20 at HEAD and 2 in 20 with this plugin's wait
    /// forced back to polling**. So it is not the parking repair: it is older, and both cadences
    /// hit it.
    ///
    /// # ⚠⚠⚠⚠ Why the two gates above cannot see it
    ///
    /// Both declare a SENTINEL, so `arrived` ends their step on `Arrival::Sentinel` and
    /// [`reaction`](Orchestrator::reaction) is only consulted afterwards for the note. A run with
    /// **no sentinel and no contract** — which is what the MCP verb's default is — ends its step
    /// on `reaction` ITSELF. That arm, over a pure-echo peer, was gated nowhere in this crate.
    ///
    /// # ⚠⚠⚠ A PROBE INSIDE, not twenty runs outside
    ///
    /// The defect is a RACE, and one step samples one moment of it: reproducing it from the outside
    /// means running the whole loop until it happens to land. So this arms the baseline exactly as
    /// [`Plugin::step`] does, injects the same stimulus, and then asks
    /// [`reaction`](Orchestrator::reaction) **as fast as it can be asked** for the whole window a
    /// step would have waited — thousands of samples instead of one, and it keeps the SCREEN that
    /// fooled it rather than only the verdict.
    ///
    /// ⚠ It is deliberately a statement about the PREDICATE and not about a run: a run that
    /// converges by luck is not evidence, and a predicate that is true at no instant cannot be
    /// raced by any cadence a caller picks.
    #[test]
    fn a_pane_that_only_echoes_never_reads_as_answered_at_any_instant() {
        /// What is typed. ⚠ Two words, because a single token cannot show the failure this is
        /// about: the defect is a row that holds MORE than the stimulus, and a longer stimulus is
        /// what gives the terminal something to tear.
        const STIMULUS: &str = "echo bounded";
        /// How long to sample — comfortably past the point at which both the pty's echo and
        /// `cat`'s copy of the line have landed, so the whole of the interesting window is covered.
        const WATCH: Duration = Duration::from_millis(800);

        // ⚠⚠⚠⚠⚠ **THE PANE HAS TEXT ON IT THAT WILL SCROLL, AND THAT IS THE WHOLE FIXTURE.** Two
        // earlier probes could not reproduce this and each green was the finding rather than a
        // pass: a roomy fresh `cat` pane never scrolls, so [`RowTrail`] is never asked the question
        // it gets wrong. The live pane is a real shell — its PROMPT wraps across three rows at
        // forty columns (`developer@…:~/remote-bui` / `ld/sprag-…$ prin` / `tf 'ECHO-READY…`) —
        // and six echoed lines push all of it upward.
        //
        // ⚠⚠ **A SCROLL MOVES CONTENT BETWEEN ROWS WITHOUT ANY PROGRAM WRITING IT**, so a row
        // comparison reports as CHANGED a row whose new text is simply its neighbour's old text.
        // `BETA` is not a piece of the stimulus, so the step reads it as the peer speaking.
        // `PaneOutputLines`' own documentation names this exact hazard — *"a resize re-wraps and
        // renumbers every one, a repaint changes none of them, and scrolling drops the ones nobody
        // came back for"* — and this is the fourth time this workspace has paid for it.
        // ⚠⚠⚠⚠ **THE ROWS MUST MARCH, NOT VANISH — which is what two earlier fixtures got wrong.**
        // A three-row pane scrolled its markers straight off and left only the echo behind, and a
        // roomy one never scrolled at all; both were green, and each green was the finding rather
        // than a pass. The live pane fills EXACTLY, so one added line shifts every remaining row up
        // by one — and the run's own account showed that marching, three rows then two then one.
        //
        // Four markers in five rows is that shape with nothing to spare: after the `printf` the
        // screen holds `M1..M4` and the cursor's row, and the first echoed line pushes `M1` off
        // while `M2..M4` all move.
        const MARKERS: [&str; 4] = ["M1", "M2", "M3", "M4"];
        let (access, pane) = sh_access("printf 'M1\\nM2\\nM3\\nM4\\n'; exec cat", 40, 5);
        // ⚠ Every marker must be ON SCREEN before the baseline is marked, or there is nothing for
        // the echo to push and the fixture has quietly become an earlier probe's.
        crate::testing::screen_showing(&access, pane, MARKERS[3]);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: STIMULUS.to_string(),
                // ⚠ NEITHER, and that is the point: with either one the step ends elsewhere and
                // this predicate is never what decides.
                sentinel: None,
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );

        // ── armed exactly as `step` arms it, then the same keystrokes ──
        // ⚠ BOTH marks, exactly as `step` takes them — a probe that armed only one would be
        // measuring a plugin no caller can build.
        orch.baseline = crate::access::RowTrail::mark(&access, pane);
        orch.spoken = access
            .output_lines()
            .and_then(|source| source.pane_lines_since(pane, orch.spoken))
            .map_or(orch.spoken, |said| said.next);
        let mut keys = crate::access::KeyStroke::text(STIMULUS);
        keys.push(crate::access::KeyStroke::named("Enter"));
        // ⚠ The written count is read rather than dropped: a pane that took NO bytes would leave
        // this probe watching a window nothing was ever said into.
        let typed = access
            .inject(pane, &keys)
            .expect("the pane takes the stimulus");
        assert!(
            typed.bytes() > 0,
            "⚠⚠ the stimulus must actually have been written, or the window below is empty by \
             construction: {typed:?}",
        );

        let deadline = Instant::now() + WATCH;
        let mut fooled: Vec<String> = Vec::new();
        let mut misread = 0_u64;
        let mut samples = 0_u64;
        let mut echoed = false;
        while Instant::now() < deadline {
            samples += 1;
            match orch.reaction(&access) {
                // ⚠ BOTH kept: the rows the product read as the peer's, and the whole screen they
                // were read off. The first says WHAT convinced it, the second says what the pane
                // looked like — and two rounds of hypotheses died for want of exactly this pair.
                Reaction::Answered(spoke) => {
                    misread += 1;
                    // ⚠⚠ DISTINCT readings only, and that is not thrift: this predicate is asked
                    // tens of thousands of times in the window, so keeping every one made a
                    // 160-kilobyte failure message that nobody could read. What a reader needs is
                    // WHICH readings happened, and the count says how often.
                    let seen = format!(
                        "{spoke:?} on {:?}",
                        access.pane_collapsed(pane).unwrap_or_default()
                    );
                    if !fooled.contains(&seen) {
                        fooled.push(seen);
                    }
                }
                Reaction::EchoOnly => echoed = true,
                Reaction::None => {}
            }
        }
        access.lifecycle().expect("lifecycle").close(pane);

        // ⚠⚠⚠⚠ **THE DEFECT IS ASSERTED BEFORE THE CONTROL, and the order is a repair.** Written
        // the other way round, a run in which `reaction` answered `Answered` from its very first
        // sample never sets `echoed` — so the CONTROL fires and reports *nothing arrived*, about a
        // pane that had in fact been misread from the first instant. A control exists to stop a
        // vacuous green, and a defect is never vacuous.
        assert!(
            fooled.is_empty(),
            "⛔⛔⛔⛔⛔ AN ECHO WAS READ AS A PEER'S ANSWER. This pane runs `cat`: everything on it \
             is either the pty's echo of what was typed or `cat` writing the same bytes back, and \
             NOTHING ON A SCREEN can tell those two apart — so *the peer said nothing* is the only \
             honest reading and `reaction` must never answer otherwise. It did, at {misread} of \
             {samples} sampled instants. A run with no sentinel ends its step on exactly this \
             predicate, so what this costs live is a turn of a bounded budget spent on an answer \
             nobody gave. The DISTINCT readings that fooled it: {fooled:?}",
        );
        // ⚠⚠ THE CONTROL, second: the stimulus must actually have come back, or a clean run means
        // only that nothing reached the pane and the assertion above passed over an empty window.
        assert!(
            echoed,
            "⚠⚠⚠ THE CONTROL: over {samples} samples the pane never once read as having echoed \
             the stimulus, so nothing arrived and the green above is vacuous",
        );
    }

    /// ⚠⚠⚠ **A PEER THAT SAYS SOMETHING BEFORE IT ANSWERS IS STILL ANSWERING** — the sibling of the
    /// gate above, and the case that separates *"wait past your own echo"* from *"wait for what you
    /// named"*.
    ///
    /// The gate above taught the wait to discount the terminal's echo. What it left standing is
    /// that ANY other change ends the step: the wait's predicate is *did the peer produce a row*,
    /// while the run's question is *did the thing I named appear*. A peer that prints one line of
    /// its own before its answer therefore ends the step at that line, the sentinel is judged
    /// against a screen it has not reached yet, and the loop takes another turn — typing the same
    /// stimulus at a peer that is part-way through answering the first one.
    ///
    /// ⚠⚠⚠ **AND EVERY REAL PEER THIS PRODUCT EXISTS TO DRIVE DOES EXACTLY THAT.** An AI CLI paints
    /// a spinner, a tool-use line, a token count — all of it before the answer. So does a build, a
    /// test run, a shell that reports a job. The fixtures that came first replied in ONE write, and
    /// that is the only reason the defect had never been seen.
    ///
    /// The peer here counts the prompts it is given and stamps every line with the number, so the
    /// witness is not the tally but the pane: `WORKING 2` is a second prompt this run had no reason
    /// to send.
    #[test]
    fn a_turn_waits_for_the_sentinel_it_named_and_not_for_the_first_thing_that_moves() {
        // Says `WORKING <n>` the instant it is prompted, thinks, and only then answers. Both lines
        // are the PEER's, so neither is an echo and the gate above cannot see this at all.
        let (access, pane) = sh_access(
            "n=0; while read line; do n=$((n+1)); echo \"WORKING $n\"; sleep 0.25; \
             echo \"PEER-REPLIED $n\"; done",
            40,
            8,
        );
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("PEER-REPLIED".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 3,
                max_cost: None,
                max_duration: None,
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers well inside one step's observe timeout: {outcome:?}",
        );
        assert_eq!(
            outcome.iterations, 1,
            "⚠⚠⚠ and it took ONE turn. A second turn is the run typing at a peer that had already \
             begun answering the first — the step ended on the peer's `WORKING` line, judged the \
             sentinel against a screen that had not reached it, and prompted again: {outcome:?}",
        );
        // ⚠⚠⚠ THE PANE IS THE WITNESS, and it is read AFTER the peer has had the time a second
        // answer would need. The run ends the moment it sees the sentinel, so a second stimulus
        // already queued in the pty would land after the outcome was decided — a tally read at the
        // run's end cannot see it and would call this gate green while the peer worked twice.
        std::thread::sleep(Duration::from_millis(600));
        let witness = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !witness.contains("WORKING 2"),
            "⚠⚠⚠ THE PEER WAS PROMPTED TWICE FOR ONE QUESTION, and the second prompt reached it \
             while it was still answering the first. For an agent session that is one wasted turn \
             of a bounded budget and one interrupted answer: {witness:?}",
        );
    }

    /// How long the peer in the gate below THINKS before it answers — the one number that decides
    /// what that gate is about, so it is named rather than buried in a shell string (R373's rule
    /// about a fixture's clock).
    ///
    /// ⚠⚠⚠ IT IS LOAD-BEARING BECAUSE IT IS LONGER THAN [`OBSERVE_TIMEOUT`], and that is the whole
    /// case: a peer that answers INSIDE one step's wait is the case already gated above, and every
    /// fixture in this file was one until now. **A real agent session is not.** Three seconds is a
    /// fast one — a `claude` turn that runs a tool is tens of seconds — and it is kept this short
    /// only so the gate does not cost the suite a minute. The defect scales with the ratio, so the
    /// number this measures is a FLOOR on what a real session would see.
    const PEER_THINKS_FOR: Duration = Duration::from_secs(3);

    /// ⚠⚠⚠ **A TURN WAITS AS LONG AS ITS PEER TAKES, AND NOT AS LONG AS ONE STEP'S TIMEOUT** — the
    /// measurement debt 64 was registered for, and the sharpest ai-loop defect left open.
    ///
    /// # ⚠⚠⚠ What the gate above fixed, and what it could not reach
    ///
    /// A step now waits for the sentinel it named — for [`OBSERVE_TIMEOUT`], which is **500 ms**.
    /// Past that the wait ends, the verdict is `continue`, and the next step TYPES THE STIMULUS
    /// AGAIN. So the run's turn boundary is a fixed timer rather than the peer's own turn, and
    /// against anything slower than half a second the loop prompts a peer that is still answering —
    /// then again, and again, twice a second for as long as the peer takes to think.
    ///
    /// ⚠⚠⚠ **AND THE PEER THIS PRODUCT EXISTS TO DRIVE IS AN AGENT SESSION THAT THINKS FOR TENS OF
    /// SECONDS.** Every prompt after the first is a real turn of that agent's bounded budget, spent
    /// answering a question it had already been asked.
    ///
    /// ⚠⚠ **THE PRODUCT ALREADY KNOWS HOW TO DO THIS, IN THE OTHER PLUGIN.** [`Agent`] does not use
    /// a timer at all: it arms a [`Completion`] and asks
    /// [`DoneWhen::Settles`](crate::DoneWhen::Settles) whether the agent has MOVED AND COME BACK TO
    /// REST. `Orchestrator` — the plugin the MCP `orchestrate` verb drives, and the one the outer
    /// AI loop drives — has no completion contract at all. That asymmetry is the debt.
    ///
    /// [`Agent`]: crate::agent::Agent
    /// [`Completion`]: crate::completion::Completion
    /// [`Turn`]: crate::completion::Turn
    #[test]
    fn a_run_with_no_turn_contract_re_prompts_a_peer_that_is_still_thinking() {
        let (access, pane) = slow_peer();
        let mut orch = Orchestrator::new(pane, slowly("AGENT-REPLIED", None));
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                // ⚠ FAR ABOVE what one question needs, because the defect is measured in turns
                // SPENT and a tight ceiling would hide it as `exhausted` instead.
                max_iterations: 100,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers well inside the run's clock: {outcome:?}",
        );
        // ⚠⚠⚠ MORE THAN ONE, NOT SIX. Six is what this box measured and the count is a function of
        // the ratio between the peer's thinking and `OBSERVE_TIMEOUT`, so pinning it would make
        // this gate a claim about the machine. What is not machine-dependent is that the run went
        // round AT ALL against a peer it had already asked.
        assert!(
            outcome.iterations > 1,
            "⚠⚠⚠ THE DEFECT IS GONE FROM THE UNCONTRACTED PATH, which means the sibling below is no
             longer measured against anything. A run with no [`Turn`] must still end its steps on \
             {OBSERVE_TIMEOUT:?} — that is what its absence promises every caller who wrote their \
             request before the contract existed: {outcome:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE FIX: A RUN THAT SAYS HOW ITS PEER FINISHES ASKS A SLOW PEER EXACTLY ONCE.**
    ///
    /// The sibling of the measurement above, against the same peer thinking for the same time. What
    /// changed is one declared argument: [`Turn`] — *what makes my peer's turn over, and how long it
    /// may take*. The step's wait stops being a constant nobody chose.
    ///
    /// ⚠⚠ THREE CLAIMS, because the first two alone are satisfied by a run that got lucky:
    /// ONE turn, ONE stimulus on the pane, and the peer's own tally read from the SCROLLBACK after
    /// it has had the time a second question would have taken.
    ///
    /// [`Turn`]: crate::completion::Turn
    #[test]
    fn a_run_that_names_how_its_peer_finishes_asks_a_slow_peer_exactly_once() {
        let (access, pane) = slow_peer();
        let mut orch = Orchestrator::new(
            pane,
            slowly(
                "AGENT-REPLIED",
                // ⚠ WELL ABOVE the peer's thinking time, so this bound is a BACKSTOP and not the
                // decision — which is the whole difference from the constant it replaces. The
                // contract is what ends the step; this is what stops a peer that never finishes
                // from holding the run for ever.
                Turn::lasting(crate::DoneWhen::Exits, Some(PEER_THINKS_FOR * 4)),
            ),
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 100,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers well inside the run's clock: {outcome:?}",
        );
        assert_eq!(
            outcome.iterations, 1,
            "⚠⚠⚠ THE RUN SPENT {} TURNS ON ONE QUESTION. Its peer thought for {PEER_THINKS_FOR:?} \
             and this run had declared how long a turn may take, so a second turn means the \
             contract was not consulted: {outcome:?}",
            outcome.iterations,
        );
        assert_eq!(
            outcome.cost,
            Some(Cost::Bytes(5)),
            "⚠⚠ and ONE stimulus reached the pane — `ping` and its Enter, once. The turn count \
             says how often the loop decided to speak; this says how much of it the peer was \
             actually given: {outcome:?}",
        );

        // ⚠⚠ THE PEER'S OWN TALLY, read from the SCROLLBACK rather than the screen, and after it
        // has had the time a second question would need. A pane ten rows tall SCROLLS, so the first
        // measurement of this defect could only see the two `PROMPTED` lines that had survived.
        std::thread::sleep(PEER_THINKS_FOR + Duration::from_millis(500));
        let witness = access.pane_full_text(pane).unwrap_or_default();
        assert!(
            !witness.contains("PROMPTED 2"),
            "⚠⚠⚠ AND THE PEER IS THE WITNESS: it was never asked a second time. A pane that shows \
             `PROMPTED 2` is one that answered a question nobody meant to ask twice: {witness:?}",
        );
    }

    /// A peer that acknowledges every prompt, thinks for [`PEER_THINKS_FOR`], and answers — the one
    /// fixture both gates above drive, so *"the same peer"* is a fact rather than a claim.
    ///
    /// ⚠ It COUNTS what it was asked and stamps every line with the number, because a tally taken
    /// from the outcome cannot see this: the run ends the moment the FIRST answer appears, while
    /// the prompts it already queued are still waiting to be read.
    fn slow_peer() -> (WorkspacePaneAccess, PaneId) {
        sh_access(
            &format!(
                "n=0; while read line; do n=$((n+1)); echo \"PROMPTED $n\"; \
                 sleep {}; echo \"AGENT-REPLIED $n\"; done",
                PEER_THINKS_FOR.as_secs(),
            ),
            40,
            10,
        )
    }

    /// The spec both gates above drive, differing in `turn` alone — so what the pair measures is
    /// that argument and nothing else.
    fn slowly(sentinel: &str, turn: Option<Turn>) -> OrchestrationSpec {
        OrchestrationSpec {
            stimulus: "ping".to_string(),
            sentinel: Some(sentinel.to_string()),
            ready_when: None,
            ready_within: None,
            may_answer: None,
            attended: Attended::NoOne,
            turn,
        }
    }

    /// ⚠⚠⚠ **THE CONTROL, AND THE WHOLE ARGUMENT: THE OTHER PLUGIN DOES NOT DO THIS.**
    ///
    /// The same peer, thinking for the same [`PEER_THINKS_FOR`], driven by [`Agent`] instead. That
    /// adapter has no timer deciding when a turn is over — it arms a [`Completion`] and asks the
    /// contract. One prompt, one answer.
    ///
    /// ⚠⚠⚠ **SO THE DEFECT ABOVE IS NOT "PANES ARE HARD", IT IS AN ASYMMETRY BETWEEN TWO PLUGINS
    /// IN THIS CRATE** — and the one WITHOUT the contract is the one the MCP `orchestrate` verb and
    /// the outer AI loop drive. Until this existed that sentence was a source reading; the two
    /// numbers beside each other are what make it a measurement.
    ///
    /// ⚠ [`DoneWhen::Exits`] rather than `Settles` so the contrast needs no supervisor: what is
    /// under test is that SOMETHING OTHER THAN A TIMER decides, not which contract was chosen. The
    /// peer therefore answers once and leaves, which is a one-shot tool's shape.
    ///
    /// [`Agent`]: crate::agent::Agent
    /// [`Completion`]: crate::completion::Completion
    /// [`DoneWhen::Exits`]: crate::DoneWhen::Exits
    #[test]
    fn the_adapter_with_a_completion_contract_asks_the_same_slow_peer_exactly_once() {
        let (access, pane) = sh_access(
            &format!(
                "n=0; read line; n=$((n+1)); echo \"PROMPTED $n\"; \
                 sleep {}; echo \"AGENT-REPLIED $n\"",
                PEER_THINKS_FOR.as_secs(),
            ),
            40,
            10,
        );
        let mut agent = crate::agent::Agent::new(
            pane,
            crate::agent::AgentSpec {
                // ⚠ WELL ABOVE the peer's thinking time, so this bound is not what ends the turn.
                // It is the same bound `Orchestrator` has; the difference is that here it is a
                // BACKSTOP and there it is the decision.
                timeout: PEER_THINKS_FOR * 4,
                ..crate::agent::AgentSpec::new("ping")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(
            &mut agent,
            &access,
            &crate::run::RunContext::uncancellable(),
        );

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "the peer answers and leaves: {outcome:?}",
        );
        assert_eq!(
            outcome.iterations, 1,
            "⚠⚠⚠ ONE TURN against the peer that cost the sibling above six. Nothing about the pane \
             changed between the two runs — only which plugin was asked to decide when a turn was \
             over: {outcome:?}",
        );
        let witness = access.pane_full_text(pane).unwrap_or_default();
        assert!(
            !witness.contains("PROMPTED 2"),
            "and the peer was asked once: {witness:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THIS PLUGIN'S OWN WAIT PARKS ON THE PANE TOO** — register item 632, which is
    /// register item 280's defect found still standing HERE after the loop's copy of it was paid.
    ///
    /// # ⚠⚠⚠⚠ What it cost, and why it survived the round that fixed the other one
    ///
    /// [`observe`](Orchestrator::observe) waited with `poll_until(run, self.patience(), ...)`, and
    /// [`patience`](Orchestrator::patience) answers [`Duration::MAX`] for a contract that declares
    /// no bound of its own — *no bound beyond the run's*, which is the right meaning and was the
    /// wrong implementation. Polled, it meant **render this pane's screen and run a detector over
    /// it a hundred times a second until the run's deadline or a cancel arrives**: the same 98/s
    /// item 280 measured, ~360,000 an hour, every one of them taking the workspace lock that every
    /// other client reads through.
    ///
    /// It survived because 280 was paid inside `ai_loop`, and this is a DIFFERENT plugin behind the
    /// same MCP verb. Nothing in the repair reached it and nothing went red.
    ///
    /// # ⚠⚠⚠ THE RATIO IS THE CLAIM, and a ceiling alone would be a tolerance
    ///
    /// One step waited out at two patiences a factor of four apart, over a pane that says nothing.
    /// **A polling wait costs four times as many looks in the long arm however slowly it polls**;
    /// a parked one costs the same in both. ⚠ The control is that each arm really waited its whole
    /// patience — a wait that returns at once costs no looks either, and is the opposite defect
    /// (`a_run_with_no_turn_contract_re_prompts_a_peer_that_is_still_thinking` is what that one
    /// costs).
    ///
    /// ⚠⚠ The contract is [`DoneWhen::Exits`] over a live pane, so `arrived` is false throughout
    /// and the wait ends on the patience — which is what makes the two arms comparable at all.
    ///
    /// # ⚠⚠⚠⚠⚠ MEASURED 2026-08-24, BOTH SIDES OF THE REPAIR
    ///
    /// | patience | looks POLLED | looks PARKED |
    /// |---|---|---|
    /// | 400 ms | 120 | **3** |
    /// | 1,600 ms | 471 | **3** |
    ///
    /// # ⛔⛔⛔ AND THE THIRD ARM EXISTS BECAUSE A MUTATION OF MINE DID NOTHING
    ///
    /// The first mutant tried against this gate was
    /// [`settling`](Orchestrator::settling)'s `map_or` DEFAULT — and it changed **nothing**,
    /// because this fixture declares a contract, so the default arm is never taken. A gate whose
    /// subject is reachable only through a branch the fixture never enters is a green that says
    /// less than it looks like.
    ///
    /// So the third arm asks the wiring DIRECTLY, with no clock in it: an observation carrying
    /// [`Settling::At`](crate::access::Settling::At) must come back out of
    /// [`settling`](Orchestrator::settling) unchanged. That kills the whole family — a `settling`
    /// that answers `Nothing` for everything, one that drops the contract's answer, one that
    /// invents a deadline of its own.
    #[test]
    fn a_step_waiting_out_its_patience_does_not_re_read_the_pane_while_it_waits() {
        /// The short arm's patience.
        const SHORT: Duration = Duration::from_millis(400);
        /// The long arm's, four times it.
        const LONG: Duration = Duration::from_millis(1_600);
        /// How many looks one waited-out step may cost however long it lasts — **measured 3 in
        /// both arms**, so this is set well above the reading rather than at it.
        const CEILING: u64 = 16;
        /// What the long arm may exceed the short one by. At the poll interval the gap between
        /// these two arms is ~120 looks, so nothing this size can hide a poll.
        const SLACK: u64 = 8;

        /// Wait out one step over a silent pane, answering **how many looks it cost**, how long it
        /// took, and how it ended.
        fn waited_out(patience: Duration) -> (u64, Duration, Arrival) {
            let (access, pane) = cat_access(40, 10);
            let counted = crate::testing::Counted::new(access);
            let orchestrator = Orchestrator::new(
                pane,
                OrchestrationSpec {
                    stimulus: "ping".to_string(),
                    // ⚠ A sentinel this pane can never show, so the SENTINEL term is false for the
                    // whole wait rather than deciding it early.
                    sentinel: Some("NEVER-ARRIVES".to_string()),
                    ready_when: None,
                    ready_within: None,
                    may_answer: None,
                    attended: Attended::NoOne,
                    turn: Turn::lasting(crate::completion::DoneWhen::Exits, Some(patience)),
                },
            );
            let run = RunContext::uncancellable();
            let entered = counted.looks();
            let began = Instant::now();
            let arrival = orchestrator.observe(&counted, &run);
            let took = began.elapsed();
            let looked = counted.looks() - entered;
            counted.lifecycle().expect("lifecycle").close(pane);
            (looked, took, arrival)
        }

        let (short_looks, short_took, short_end) = waited_out(SHORT);
        let (long_looks, long_took, long_end) = waited_out(LONG);

        // ── the control: both arms really waited out their patience ──
        assert_eq!(
            (short_end, long_end),
            (Arrival::Nothing, Arrival::Nothing),
            "⚠⚠⚠ THE CONTROL: this pane shows no sentinel and its program has not exited, so both \
             waits must end on the patience. Anything else measured a different wait",
        );
        assert!(
            short_took >= SHORT && long_took >= LONG,
            "⚠⚠⚠ AND NEITHER MAY SKIP IT — a step that returns at once costs no looks either and \
             would pass every assertion below while committing the opposite defect: re-prompting a \
             peer that is still thinking. short {short_took:?} of {SHORT:?}; long {long_took:?} of \
             {LONG:?}",
        );

        // ── the claim: LOOKING DOES NOT FOLLOW THE CLOCK ──
        assert!(
            long_looks <= short_looks + SLACK,
            "⚠⚠⚠⚠⚠ THIS STEP IS POLLING THE PANE. A {LONG:?} wait cost {long_looks} looks where a \
             {SHORT:?} one cost {short_looks} — the count follows the CLOCK. `patience()` answers \
             `Duration::MAX` for a contract with no bound, so at this rate a run waiting on a \
             thinking peer renders its screen ~360,000 times an hour. Register item 632, and item \
             280's own question: *why is it LOOKING at all?*",
        );
        assert!(
            long_looks <= CEILING && short_looks <= CEILING,
            "⚠⚠⚠ AND A WAITED-OUT STEP COSTS A HANDFUL OF LOOKS, NOT HUNDREDS. short \
             {short_looks}, long {long_looks}, ceiling {CEILING}",
        );

        // ── ⛔ THE WIRING, ASKED WITH NO CLOCK IN IT — see this gate's own doc for why ──
        let (bare, supervised_pane) = cat_access(40, 10);
        /// The instant the fixture's supervisor claims its verdict flips at. Any value will do:
        /// what is under test is that it survives the journey, not what it is.
        const AHEAD: Duration = Duration::from_secs(42);
        let publishes_at = Instant::now() + AHEAD;
        let access = bare.with_agent_state(Some(Arc::new(move |_id: PaneId| {
            Some(crate::access::AgentObservation {
                state: sprag_detect::AgentState::Working,
                agent: Some("claude".to_string()),
                authority: crate::access::Authority::Reported {
                    source: "test".to_string(),
                },
                seq: 1,
                asked_seq: 1,
                reports: 1,
                asking: None,
                asked: None,
                said: None,
                said_seq: 0,
                noticed: None,
                transcript: None,
                // ⚠ THE ONE FIELD THIS ARM IS ABOUT.
                settling: crate::access::Settling::At(publishes_at),
                reporter: crate::access::ReporterVoice::Speaking,
            })
        })));
        let step = Orchestrator::new(
            supervised_pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("NEVER-ARRIVES".to_string()),
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: Turn::lasting(crate::completion::DoneWhen::Settles, Some(SHORT)),
            },
        );
        // ⚠ THROUGH THE ONE READING the step's own wait takes — register item 637. The deadline
        // and the ending come off the same `Stands`, so this asks exactly what a round asks.
        let stands = step
            .done
            .as_ref()
            .map(|done| done.stands(&access, supervised_pane));
        let forwarded = Orchestrator::settling(stands.as_ref());
        access
            .lifecycle()
            .expect("lifecycle")
            .close(supervised_pane);
        assert_eq!(
            forwarded,
            crate::access::Settling::At(publishes_at),
            "⛔⛔⛔⛔⛔ THE SUPERVISOR'S DEADLINE IS NOT REACHING THIS STEP'S WAIT. \
             `Orchestrator::settling` must hand the CONTRACT's answer through untouched: a \
             `Nothing` here is a claim that the verdict stands until the pane moves, which parks a \
             run straight through a settling peer, and a deadline of this step's own invention \
             would be the old lag wearing a new type. Got {forwarded:?}",
        );
    }

    /// ⚠⚠⚠ **A RUN WHOSE CLOCK ENDED IT AT THE BARRIER SAYS WHAT IT WAS STILL WAITING FOR** — and
    /// this is THE SIGNATURE OF THIS WORKSPACE'S LONGEST-RUNNING FLAKE, reproduced on purpose.
    ///
    /// # ⚠⚠⚠ The hypothesis this gate exists to settle
    ///
    /// `.claude/remote-build.toml` has carried this for rounds: a `sprag-plugin --lib` run at
    /// thirty threads fails with `Exhausted(Duration)`, `iterations: 1`, `Bytes(0)` — a DIFFERENT
    /// member of the class each time, and each one green when run alone. The suspicion written
    /// beside it was [`ReadyWhen::Prints`]' arming baseline, and it was explicitly filed as *a
    /// hypothesis, not a finding: nobody has instrumented the arming count on the failing side*.
    ///
    /// This is the instrument. The window a loaded machine opens by accident — the peer's
    /// announcement landing before the barrier's first look — is opened deliberately by waiting for
    /// it, and the run then produces **that signature exactly**. The hypothesis is a finding.
    ///
    /// # ⚠⚠ And what a reader was told about it
    ///
    /// Nothing. `Exhausted(Duration)` with `Bytes(0)` is advice to raise a time budget, about a
    /// barrier that would never have cleared however long it waited — and the caller's remedy is
    /// not a bigger number, it is a different question ([`ReadyWhen::Shows`]). Which of the two
    /// unsatisfied endings they land in is decided by whichever of `ready_within` and the run's own
    /// clock is shorter, so the SAME mistake was diagnosed or silent depending on arithmetic
    /// nobody wrote down. Both endings carry the diagnosis now — see [`Reached::RunEnded`].
    #[test]
    fn a_run_the_clock_ended_at_the_barrier_says_what_it_was_waiting_for() {
        let (access, pane) = sh_access("printf 'BANNER\\n'; exec cat", 40, 8);
        // THE WINDOW, OPENED ON PURPOSE. Under load the scheduler opens this one by itself.
        crate::testing::screen_showing(&access, pane, "BANNER");
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("A SENTINEL THIS PANE NEVER PRINTS".to_string()),
                ready_when: Some(ReadyWhen::Prints("BANNER".to_string())),
                // ⚠ LONGER THAN THE RUN'S CLOCK, which is what puts this on the silent path. The
                // flaky fixtures have it this way round by taking the default and not thinking
                // about it, which is exactly how a caller gets here.
                ready_within: Some(Duration::from_secs(30)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 10,
            max_cost: None,
            max_duration: Some(Duration::from_millis(400)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut orch, &access, &crate::run::RunContext::uncancellable());

        // ⚠⚠⚠ THE RECORDED SIGNATURE, ASSERTED. If these three ever stop describing this state,
        // the note in `remote-build.toml` is about something else and this gate must be re-read.
        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "the flake's own ending: {outcome:?}",
        );
        assert_eq!(outcome.iterations, 1, "and its turn count: {outcome:?}");
        assert_eq!(
            outcome.cost,
            Some(Cost::Bytes(0)),
            "⚠⚠ and NOTHING WAS EVER TYPED, which is the half that says a barrier and not a peer \
             is what this run spent its clock on: {outcome:?}",
        );

        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let said = notes.join(" | ");
        assert!(
            said.contains("BANNER"),
            "⚠⚠⚠ the run must name WHAT it was waiting for. Without it the whole report is \
             `exhausted: duration` over `Bytes(0)`, which sends its reader to raise a budget that \
             was never the bound: {said:?}",
        );
        assert!(
            said.contains("already on its screen"),
            "⚠⚠⚠ and the fact that ends the search — the marker the barrier was waiting to be \
             printed is ON THE PANE. This is the sentence that would have closed a hypothesis that \
             stood for rounds: {said:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A PANE THAT CANNOT REACT PUTS A FLOOR UNDER EVERY STEP**, which is the only thing that
    /// lets a gate ask WHICH ceiling stopped a run without racing the machine it runs on.
    ///
    /// Against a pane that echoes, a step ends the instant the echo lands, so a run's turn count
    /// is a function of how fast the box is: the same one-second run took 97 turns here and would
    /// take a different number anywhere else. Deaf, every step waits [`OBSERVE_TIMEOUT`] out in
    /// full, so the turns a timed run can fit are arithmetic — and a slower box only makes the
    /// floor higher, never lower.
    ///
    /// Both halves are asserted because either alone is a weaker claim than it reads as:
    ///
    /// * The pane really is DEAF — the step notes say so. Without this the run below could be
    ///   ending by the clock for the ordinary reason, and the floor this gate is about would be
    ///   absent with nothing to notice it.
    /// * The turns it fitted are FAR below the iteration ceiling it also asked for, so `duration`
    ///   is the only ceiling that was ever in reach.
    #[test]
    fn a_deaf_pane_floors_every_step_so_the_clock_is_the_only_ceiling_in_reach() {
        // `stty -echo` stops the kernel echoing the injection; the reader discards what it reads.
        // Once ready, nothing this run does can reach the screen.
        let (access, pane) = sh_access(DEAF, 20, 4);
        // ⚠⚠⚠ THE PEER IS UP BEFORE THE CLOCK STARTS. This run's whole point is arithmetic over a
        // 1.2-second budget, and the readiness wait used to be INSIDE it: on a loaded box the
        // startup ate the budget and the journal's last step was the readiness wait where the
        // assertion below demands the observe wait. That is a red about the machine wearing the
        // shape of a red about the product, and it is the one shape every load-marginal failure in
        // this crate has taken.
        started(&access, pane, "DEAF-READY");
        // ⚠ The readiness barrier is still the PRODUCT's (`ready_when`, below) rather than a helper
        // this test kept to itself — so the gate drives the same wait a caller gets. It changes
        // KIND rather than going away: `Shows` reads a marker already on the screen, which is the
        // state `started` leaves the pane in, while `Prints` would wait for a second announcement
        // this peer never makes. See `testing::started`.
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("A SENTINEL THIS PANE NEVER PRINTS".to_string()),
                ready_when: Some(ReadyWhen::Shows("DEAF-READY".to_string())),
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(1_200)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut orch, &access, &crate::run::RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "a hundred turns were on offer and the clock is what ran out",
        );
        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        assert!(
            !notes.iter().any(|note| note.contains("the pane reacted")),
            "no step may have found this pane reacting, or the floor this gate rests on is not \
             there: {notes:?}; the pane shows {:?}",
            access.pane_collapsed(pane),
        );
        assert_eq!(
            notes.last().map(String::as_str),
            Some("the run ended while watching for the pane to react"),
            "AND THE LAST STEP IS ONE THE CLOCK CUT MID-OBSERVE — the deadline reaching inside a \
             step, which is the whole difference between this ceiling and the two that are decided \
             between them. A run whose final step ran its observe out in full would end by the \
             same `duration` and prove only the loop top: {notes:?}",
        );
        assert!(
            outcome.iterations <= 4,
            "a step floored at {OBSERVE_TIMEOUT:?} cannot fit more than a handful into 1.2s — \
             {} turns says the floor is missing",
            outcome.iterations,
        );
    }

    /// ⚠⚠⚠ **ONE TURN ASKS MORE THAN ONE QUESTION, AND A RUN MAY BE GIVEN CONSENT TO ALL OF THEM.**
    ///
    /// The measurement this gate was written for: an agent turn that runs a command and then edits
    /// a file asks *"Do you want to proceed?"* and then *"Do you want to make this edit?"* — two
    /// questions, in the agent's own different words. With a consent that can name only ONE of
    /// them, an unattended run answers the first and stops at the second under
    /// [`Refusal::OtherQuestion`](crate::consent::Refusal::OtherQuestion) — correct, honest, and
    /// still a run that a person has to come back to, which is the case the whole contract exists
    /// to serve.
    ///
    /// So the contract takes a LIST, and this drives one: both questions are answered, the peer
    /// finishes its turn, and the run converges on the sentinel it prints.
    ///
    /// ⚠ **THE PANE IS THE WITNESS, not the tally.** `TURN COMPLETE` carries what the peer took on
    /// each question and which byte took it — so this asserts the run chose `1` on both and never
    /// touched the two options that mean *stop asking me* and *no*. A gate reading only
    /// `answered: 2` would pass for a run that approved the wrong thing twice.
    ///
    /// ⚠ REVERT-PROOF: give the run only the first clause and it ends `blocked` after ONE answer.
    #[test]
    fn one_run_answers_every_question_of_a_turn_it_was_consented_to() {
        let (access, pane) = crate::testing::two_question_peer();
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "carry on".to_string(),
                sentinel: Some("TURN COMPLETE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: Consents::of(vec![
                    crate::consent::Consent::parse(
                        "Do you want to proceed?".to_string(),
                        "Yes".to_string(),
                    )
                    .expect("two needles"),
                    crate::consent::Consent::parse(
                        "Do you want to make this edit?".to_string(),
                        "Yes".to_string(),
                    )
                    .expect("two needles"),
                ]),
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 8,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            },
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "⚠⚠⚠ the turn asked twice and the caller had decided about both: {outcome:?}",
        );
        assert_eq!(
            outcome.answered, 2,
            "⚠⚠ BOTH questions, and the tally is what a reader of a long run has instead of a \
             journal that reaches back that far: {outcome:?}",
        );
        let screen = crate::testing::screen_showing(&access, pane, "TURN COMPLETE");
        assert!(
            screen.contains("TOOK-1-1-") && screen.contains("TOOK-2-1-"),
            "⚠⚠⚠ THE PEER SAYS WHAT IT TOOK, and it must be option 1 on BOTH questions — a run \
             that answered `Yes, and do not ask again` or `No` would report the same tally: \
             {screen:?}",
        );
        assert!(
            !screen.contains("SAW-"),
            "⚠⚠ AND NOT ONE KEY THE DIALOGS IGNORED. Every byte this run sent was one the peer \
             acted on, which is what `Taken` is about: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A QUESTION NO CLAUSE IS ABOUT STILL STOPS THE RUN** — the arm that must not move when
    /// the contract learns to hold several clauses.
    ///
    /// The control for the gate above, and the more important of the two. A list is a widening, and
    /// every widening in this contract is in the direction of answering something the caller did
    /// not picture — so the run given consent to the FIRST question only must answer exactly that
    /// one and hand the second to a person, with the reason that says which.
    #[test]
    fn a_second_question_no_clause_covers_still_ends_the_run() {
        let (access, pane) = crate::testing::two_question_peer();
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "carry on".to_string(),
                sentinel: Some("TURN COMPLETE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: Consents::of(vec![
                    crate::consent::Consent::parse(
                        "Do you want to proceed?".to_string(),
                        "Yes".to_string(),
                    )
                    .expect("two needles"),
                ]),
                attended: Attended::NoOne,
                turn: None,
            },
        );
        let outcome = run(
            &access,
            &mut orch,
            Guardrails {
                max_iterations: 8,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            },
        );
        let OutcomeState::Blocked(Some(unanswered)) = &outcome.state else {
            panic!(
                "⚠⚠⚠ the second question is one the caller never wrote a clause about, and a run \
                 that answered it anyway is the defect this contract exists to make \
                 unrepresentable: {outcome:?}",
            );
        };
        assert_eq!(
            unanswered.why(),
            crate::consent::Refusal::OtherQuestion,
            "and the reason is the one the caller can act on — a clause is missing, not wrong",
        );
        assert_eq!(
            outcome.answered, 1,
            "the FIRST question was covered and answered: {outcome:?}",
        );
        assert!(
            unanswered
                .question()
                .is_some_and(|asked| asked.asked.join(" ").contains("make this edit")),
            "and the question that stopped it comes back, in the agent's own words: {unanswered:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// The budget the INTERRUPTION PAIR below is driven under — one number, because the two gates
    /// are one claim read from both sides and a pair kept in step by hand drifts.
    ///
    /// # ⚠⚠⚠ Why it is far above where the run is expected to stop
    ///
    /// It was FOUR, and four was also roughly the step a run stops at when a person types into it.
    /// So `iterations < 4` asked *"did it stop before its budget"* of a run whose stop and whose
    /// budget were the same step, and the answer depended on whether the person's thread won a race
    /// against the run's step cadence. It won on Linux and lost on macOS, where the gate went red
    /// **about a product that had behaved correctly**: the run stopped at the FIRST barrier after
    /// the person's key — that barrier was just the fourth, so `Bytes(15)` says it typed three
    /// times and stopped, exactly as the contract requires.
    ///
    /// ⚠⚠⚠ **A CONTROL WITH NO MARGIN IS NOT A CONTROL.** What the assertion has to separate is *it
    /// stopped because somebody took the pane* from *it stopped because it ran out of turns*, and
    /// those two are only distinguishable when the second is a long way off. The distance is the
    /// whole instrument; the number itself is arbitrary.
    ///
    /// ⚠ The handless control pays for all forty and asserts it reached them, which is what makes
    /// the other half mean something — the same fixture, the same person, one capability withheld.
    const INTERRUPTION_BUDGET: u32 = 40;

    /// ⚠⚠⚠ **A HOST THAT CANNOT SAY WHOSE KEYSTROKES THESE WERE GOES ON DRIVING — THE ABSENCE IS
    /// NOT READ AS A PERSON.**
    ///
    /// # ⚠⚠ What this test WAS, and why it is worth more as a control
    ///
    /// It was the measurement taken before any of this existed, and its number is the round's
    /// headline: **with a person's key in the pane, the run typed its stimulus at them twice more
    /// and reported `Exhausted(Iterations)` as though it were alone at the keyboard.** Once the
    /// barrier could ask, that gate could only assert a defect that was gone.
    ///
    /// So it drives the same person and the same peer against a host with
    /// [`PaneAccess::hands`](crate::access::PaneAccess::hands) withheld
    /// ([`HandlessAccess`](crate::testing::HandlessAccess)) — which is every host that has not
    /// implemented the capability, and the exact condition whose SAFE direction the contract claims
    /// in prose. **The old behaviour is the required one here**: an absence of evidence that
    /// somebody is present must never become evidence that they are, or the first host without the
    /// capability stops driving every pane it has.
    ///
    /// ⚠ The person still types through the pane's own door — see
    /// [`crate::testing::person_types`]. A gate whose person used the RUN's door would have assumed
    /// the answer, since those are the same call.
    #[test]
    fn a_host_that_cannot_name_the_hand_keeps_driving() {
        let (workspace, pane) = crate::testing::silent_peer();
        let access = crate::testing::HandlessAccess(workspace);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );

        // The person waits until the run has plainly started (its first stimulus has been seen by
        // the peer, byte by byte), then types one key of their own: `X`, byte 88. Through the
        // pane's own door, which this host simply has no way to distinguish.
        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access.0, pane, "SAW 112");
                crate::testing::person_types(&access.0, pane, b"X");
            });
            let outcome = run_any(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: INTERRUPTION_BUDGET,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        assert_eq!(
            outcome.state,
            OutcomeState::Exhausted(Ceiling::Iterations),
            "⚠⚠⚠ a host with no way to name the hand must DRIVE, exactly as it did before this \
             contract existed. Reporting `taken_over` here would mean the absence of the \
             capability had been read as a person being present, which would stop every run on \
             every host that has not implemented it: {outcome:?}",
        );
        assert_eq!(
            outcome.iterations, INTERRUPTION_BUDGET,
            "⚠⚠ and it drove every one of them, so the claim above is about driving rather than \
             about which word a stopped run chose — and it is what puts the DISTANCE under the \
             sibling's `stopped early`: {outcome:?}",
        );
        access.0.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **AND THE FIX: A RUN STOPS DRIVING A PANE A PERSON HAS TAKEN, AND SAYS SO IN A WORD OF
    /// ITS OWN.**
    ///
    /// The sibling of the measurement above, against the same peer and the same person. What
    /// changed is that the pane now records WHOSE hand each write came from
    /// ([`sprag_terminal::Hand`]), so the barrier every injecting plugin passes through can ask.
    ///
    /// ⚠⚠ It asserts the WORD and not only the stopping. A run that stopped and reported
    /// `exhausted` would pass a weaker gate while telling its reader to raise a budget — about a
    /// pane somebody else is typing into, which is advice that would make things worse.
    #[test]
    fn a_run_stops_driving_a_pane_a_person_has_taken() {
        let (access, pane) = crate::testing::silent_peer();
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );

        // ⚠⚠⚠⚠⚠ **THE WRITE-ORDER WITNESS, TAKEN AT THE MOMENT THE PERSON REACHES IN** — register
        // item 586, and the axis the screen witness at the end of this gate does not have.
        //
        // `PaneHands` counts a write WHEN IT IS MADE and records whose hand it was, so *did the run
        // type after the person did* is answerable without asking the screen anything. The screen
        // cannot answer it: this peer reads ONE BYTE AT A TIME with `dd` and echoes what it got, so
        // a stimulus WRITTEN before the person's key can still be ECHOED after it — and the witness
        // at the end of this gate counts `SAW 112` after `SAW 88` on exactly that echo. Which of
        // the two is failing 20% of runs is what this pair measures; the repository's own rule
        // (`a-screen-match-is-evidence-only-if-the-terminal-did-not-paint-it`) is why it is asked.
        let typed_at: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let watermark = Arc::clone(&typed_at);
        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "SAW 112");
                // ⚠ READ BEFORE THE KEY, never after: a run write landing between this read and the
                // keystroke must count as BEFORE, or the arm accuses the product of a write the
                // person's own thread raced it to.
                *watermark.lock().expect("the watermark mutex") = access
                    .hands()
                    .and_then(|hands| hands.pane_hands(pane))
                    .map(sprag_terminal::Hands::by_a_program);
                crate::testing::person_types(&access, pane, b"X");
            });
            let outcome = run(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: INTERRUPTION_BUDGET,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        let Some(interruption) = (match &outcome.state {
            OutcomeState::TakenOver(interruption) => *interruption,
            other => panic!(
                "⚠⚠⚠ a person typed into this pane and the run must report THAT, not {other:?} — \
                 `exhausted` tells its reader to raise a budget and `blocked` tells them to answer \
                 a question, and here they must do neither: {outcome:?}",
            ),
        }) else {
            panic!(
                "⚠⚠ and the outcome carries what they did, or nobody can tell one keystroke \
                    from a person taking over the session: {outcome:?}"
            );
        };
        assert!(
            interruption.writes() >= 1,
            "⚠ at least the one key they typed: {interruption:?}",
        );

        // ⚠⚠⚠ THE CONTROL THAT MAKES THE ASSERTION ABOVE MEAN SOMETHING: the run STOPPED EARLY. A
        // run that reached its ceiling and merely relabelled the outcome would satisfy everything
        // above it, and would still have typed over the person every turn it had.
        //
        // ⚠⚠⚠ AND THE DISTANCE IS THE INSTRUMENT — see [`INTERRUPTION_BUDGET`]. This asked
        // `< 4` of a run expected to stop at about the fourth step, which is a control whose
        // margin was zero: it held while the person's thread won a race against the run's step
        // cadence, and macOS is where it lost. The sibling above spends the whole budget, so
        // stopping this far short of it cannot be the budget.
        assert!(
            outcome.iterations < INTERRUPTION_BUDGET,
            "⚠⚠⚠ the run must have stopped BEFORE its ceiling — it ran {} of \
             {INTERRUPTION_BUDGET} iterations, which means it went on typing and only the word \
             changed: {outcome:?}",
            outcome.iterations,
        );
        // ⚠⚠ WAIT FOR THE PERSON'S OWN BYTE TO BE ON THE SCREEN BEFORE READING THE WITNESS. The
        // run ends the moment it NOTICES their write, and the peer echoes it whenever it next gets
        // round to reading — so a witness taken at the run's end is a race with the pty, and this
        // gate lost it once in two runs on a loaded machine. It is not a weaker claim: if `SAW 88`
        // never arrives the wait ends anyway and the split still answers `None`.
        // ⚠⚠⚠⚠⚠ **THE WRITE-ORDER WITNESS IS ASKED FIRST**, because it is the one that can tell a
        // PRODUCT defect from a reading of the screen. `by_a_program` counts a write when it is
        // made, so this is *did the run type again after the person reached in* with no terminal in
        // the way. Register item 586.
        let wrote_before = typed_at
            .lock()
            .expect("the watermark mutex")
            .expect("this host counts hands, or the arm below is about a fixture");
        let wrote_after = access
            .hands()
            .and_then(|hands| hands.pane_hands(pane))
            .map(sprag_terminal::Hands::by_a_program)
            .expect("the pane is still there");
        assert_eq!(
            wrote_after, wrote_before,
            "⛔⛔⛔⛔⛔ THE RUN TYPED AFTER THE PERSON REACHED IN, counted at the WRITE rather than \
             read off the screen: {wrote_before} program writes when they touched the keyboard, \
             {wrote_after} when the run ended. This is the product, not a terminal's ordering",
        );

        crate::testing::screen_showing(&access, pane, "SAW 88");
        let witness = access.pane_full_text(pane).unwrap_or_default();
        let after = witness
            .split_once("SAW 88")
            .map(|(_, rest)| rest.matches("SAW 112").count());
        assert_eq!(
            after,
            Some(0),
            "⚠⚠⚠ AND THE PANE IS THE WITNESS: not one stimulus reached it after the person's key. \
             The outcome word is a claim; this is the evidence: {witness:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE MEASUREMENT: A PERSON TAKES THE PANE, FINISHES, LETS GO — AND THE RUN NEVER COMES
    /// BACK.**
    ///
    /// R372 taught a run to STOP for a person, which was half of what `ai_loop.scxml` asks for. The
    /// document's `awaiting_human` is a WAITING state with four ways out and only one of them ends
    /// the run; the product had one way out and it was the ending. A supervisor who typed one key
    /// into a pane a loop was driving had to restart the loop by hand.
    ///
    /// This is that gap with a number on it, taken before anything was built for it: **the goal was
    /// one turn away, the person had let go, and the run ended holding THIRTY-SEVEN of its forty
    /// iterations unspent** — `"…TURN 2HANDED BACK"` and nothing after it.
    ///
    /// ⚠⚠ It is kept as the CONTROL for [`Handback::Never`], which is still the default and still
    /// the right answer for a run nobody is watching. What used to be the whole behaviour is now one
    /// of two, and this gate is what says the other one had to be asked for.
    #[test]
    fn a_person_who_lets_go_of_the_pane_does_not_get_the_run_back() {
        // ⚠ SLOW TURNS: this reading must not be able to end `exhausted`, or the measurement below
        // is about a budget rather than about a person.
        let (access, pane) = crate::testing::work_needing_a_person(true);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("WORK DONE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );

        // The person reaches in as soon as the run has plainly started, does their one thing, and
        // stops. From that instant the pane is theirs and nobody is typing into it.
        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "SAW 112");
                crate::testing::person_types(&access, pane, b"X");
            });
            let outcome = run(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: 40,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        assert!(
            matches!(outcome.state, OutcomeState::TakenOver(_)),
            "the pre-condition of the measurement: the run stopped for them (R372): {outcome:?}",
        );
        let screen = crate::testing::screen_showing(&access, pane, "HANDED BACK");
        assert!(
            screen.contains("HANDED BACK"),
            "⚠ and the person's own act reached the peer, so the only thing between this run and \
             its goal is ONE more turn: {screen:?}",
        );
        assert!(
            !screen.contains("WORK DONE"),
            "⚠⚠⚠ THE MEASUREMENT: the goal is one turn away, the run holds {} unspent iterations \
             of 40, the person has let go — and the sentinel is not there and never will be. A \
             supervisor restarts this loop by hand: {screen:?}",
            40 - outcome.iterations,
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **AND THE FIX: A PERSON TAKES THE PANE, FINISHES, AND THE RUN GETS IT BACK.**
    ///
    /// The sibling of the measurement above, against the same peer and the same person, with the
    /// one thing added that the caller has to say: how long a still hand means they are done
    /// ([`Handback::WhenStill`]). `ai_loop.scxml`'s `awaiting_human` → `working` edge, taken.
    ///
    /// # ⚠⚠⚠ The three assertions, and why NONE of them alone would do
    ///
    /// The peer cannot converge until a person's byte has reached it, so a run that NEVER NOTICED
    /// the interruption — one that typed straight over them — converges here too. `Converged` alone
    /// would therefore be green for the product that has no handback at all, and green for the one
    /// that had no interruption check either. So:
    ///
    /// * **it converged** — the goal the measurement could not reach;
    /// * **a step was SPENT on the wait, and says so in the journal** — this is the one that
    ///   separates a run that waited from a run that never stopped. The note is the barrier's own
    ///   sentence, so a build that reported the handback without waiting for it would have to lie in
    ///   the journal to pass;
    /// * **it drove the pane EXACTLY ONCE between their letting go and the goal** — the screen is
    ///   the witness, so the sentinel this run converged on is one its own keystroke produced,
    ///   rather than a burst catching up for the turns it missed.
    ///
    /// ⚠ The claim is bounded to the span BETWEEN the two markers on purpose. This peer answers a
    /// stimulus with five lines, and the orchestrator judges its sentinel once the pane has
    /// REACTED — so the step that produced `WORK DONE` can be judged off `SAW 112` and one more
    /// stimulus goes in before the next look converges. That is this plugin's own observe/judge
    /// seam, visible here because the fixture prints more than one line, and it has nothing to do
    /// with a handback: measured identically on a run nobody ever interrupted.
    #[test]
    fn a_person_who_lets_go_hands_the_pane_back_and_the_run_finishes() {
        // ⚠ SLOW TURNS, exactly as the control above — the two readings differ in the handback and
        // in nothing else.
        let (access, pane) = crate::testing::work_needing_a_person(true);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("WORK DONE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                // ⚠ A stillness of 400 ms and a patience of twenty seconds — the two are different
                // questions and this gate is about neither number: what it needs is a stillness the
                // person's ONE key is plainly on the far side of, and a patience so generous that
                // whatever ends this wait, it is not the bound.
                attended: Attended::of(
                    Duration::from_secs(20),
                    Handback::of(Duration::from_millis(400)).expect("a positive stillness"),
                )
                .expect("a positive patience"),
                turn: None,
            },
        );

        let cell = crate::driver::ProgressCell::default();
        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "SAW 112");
                crate::testing::person_types(&access, pane, b"X");
            });
            let outcome = Driver::new(Guardrails {
                max_iterations: 40,
                max_cost: None,
                max_duration: Some(Duration::from_secs(60)),
            })
            .reporting_to(Arc::clone(&cell))
            .run(&mut orch, &access, &crate::run::RunContext::uncancellable());
            watcher.join().expect("the person's thread");
            outcome
        });

        assert!(
            matches!(outcome.state, OutcomeState::Converged),
            "⚠⚠⚠ the person finished and let go, so the run must have carried on and reached the \
             goal it was one turn from. This is the measurement's number closed: {outcome:?}",
        );
        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        assert!(
            notes
                .iter()
                .any(|note| note.contains("their hand went still")),
            "⚠⚠⚠ AND A STEP WAS SPENT WAITING FOR THEM. Without this the gate is satisfied by a run \
             that never noticed the person at all — this peer converges for one that types straight \
             over them, because it is their BYTE that unlocks the sentinel: {notes:?}",
        );
        let witness = access.pane_full_text(pane).unwrap_or_default();
        let between = witness
            .split_once("HANDED BACK")
            .and_then(|(_, rest)| rest.split_once("WORK DONE"))
            .map(|(drove, _)| drove.matches("SAW 112").count());
        assert_eq!(
            between,
            Some(1),
            "⚠⚠⚠ AND THE PANE IS THE WITNESS: between the person letting go and the goal, EXACTLY \
             ONE stimulus. Not a burst catching up for the turns it missed, and not zero — the \
             sentinel this run converged on is one its OWN keystroke produced. The outcome word is \
             a claim; this is the evidence: {witness:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PERSON WHO NEVER LETS GO KEEPS THE PANE, AND THE RUN ENDS EXACTLY AS IT WOULD HAVE.**
    ///
    /// [`Handback::WhenStill`]'s doc says a pane still theirs when the patience runs out ends the
    /// run *"exactly as `Never` would have, with the same word and the whole episode's writes
    /// counted"*. A comment stating a premise is a claim, and this is the third of the three the
    /// handback contract makes — the one that says waiting is BOUNDED.
    ///
    /// ⚠⚠ It was written because a mutation asked for it: replacing the timed-out arm with a
    /// handback left every other gate here green. The two gates above both have a person who
    /// FINISHES, so neither can see what happens to one who does not.
    #[test]
    fn a_person_who_keeps_the_pane_past_the_patience_keeps_it() {
        let (access, pane) = crate::testing::work_needing_a_person(true);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("WORK DONE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                attended: Attended::of(
                    // ⚠ ONE SECOND of patience against somebody who types for three, and a
                    // stillness they never reach. The wait is meant to run out here, which is what
                    // no other gate in this file arranges.
                    Duration::from_secs(1),
                    Handback::of(Duration::from_secs(5)).expect("a positive stillness"),
                )
                .expect("a positive patience"),
                turn: None,
            },
        );

        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "SAW 112");
                for _ in 0..15 {
                    crate::testing::person_types(&access, pane, b"X");
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
            let outcome = run(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: 40,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        let Some(interruption) = (match &outcome.state {
            OutcomeState::TakenOver(interruption) => *interruption,
            other => panic!(
                "⚠⚠⚠ the patience ran out with the pane STILL THEIRS, so the run must end on the \
                 word that says so. {other:?} means a run that gave up waiting and then typed \
                 underneath somebody who had not stopped — which is worse than never having \
                 offered to wait: {outcome:?}",
            ),
        }) else {
            panic!("⚠ and it carries what they did: {outcome:?}");
        };
        assert!(
            interruption.writes() >= 2,
            "⚠⚠ AND THE WHOLE EPISODE IS COUNTED, not just the write that first stopped the run. A \
             report of `1` here would be the number read before the wait, which would make a run \
             that waited a second indistinguishable from one that never waited at all: \
             {interruption:?}",
        );
        assert!(
            outcome.iterations < 40,
            "⚠ and it stopped early rather than driving to its ceiling: {outcome:?}",
        );
        let witness = access.pane_full_text(pane).unwrap_or_default();
        assert!(
            !witness.contains("WORK DONE"),
            "⚠⚠⚠ AND THE PANE IS THE WITNESS: the goal was one turn away the whole time and the \
             run did not take it. Reaching it here would mean the run typed into a pane whose \
             person was mid-sentence: {witness:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **AND THE RUN DOES NOT TYPE INTO THE GAP BETWEEN THEIR WORDS.**
    ///
    /// The sibling gate above has the person type ONCE, which is the case a handback exists for and
    /// is also the case a build that ignored the stillness entirely would pass: one keystroke, then
    /// silence, so *"resume the moment you notice"* and *"resume once their hand is still"* give the
    /// same answer. This is the reading that tells them apart — somebody WORKING in the pane, whose
    /// keystrokes come with ordinary human pauses between them.
    ///
    /// [`Handback::WhenStill`] and [`Handback::of`] both say in prose that a stillness too short
    /// means a run that types between a person's words. A comment stating a premise is a claim, and
    /// this is the claim's gate: five keystrokes 100 ms apart against a two-second stillness, and
    /// **not one stimulus may land between the first and the last**.
    #[test]
    fn a_person_still_typing_does_not_lose_the_pane_between_keystrokes() {
        // ⚠⚠⚠ BRISK TURNS, and this is the gate's whole discriminator. A peer that takes a second
        // per turn makes the RUN's step cadence longer than a person's pauses, so a build with no
        // stillness rule at all cannot type into one and the gate passes over it — measured, on
        // this gate's first form.
        let (access, pane) = crate::testing::work_needing_a_person(false);
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: Some("WORK DONE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: None,
                attended: Attended::of(
                    Duration::from_secs(20),
                    // ⚠ TWO SECONDS against gaps of 100 ms — an order of magnitude, so what this
                    // gate reads is the RULE and not a race with the machine it runs on.
                    Handback::of(Duration::from_secs(2)).expect("a positive stillness"),
                )
                .expect("a positive patience"),
                turn: None,
            },
        );

        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "SAW 112");
                for _ in 0..5 {
                    crate::testing::person_types(&access, pane, b"X");
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
            let outcome = run(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: 40,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        assert!(
            matches!(outcome.state, OutcomeState::Converged),
            "⚠ they finished and the run carried on, as in the gate above: {outcome:?}",
        );
        let witness = access.pane_full_text(pane).unwrap_or_default();
        let typing = witness
            .split_once("HANDED BACK")
            .and_then(|(_, rest)| rest.rsplit_once("HANDED BACK"))
            .map(|(during, _)| during.matches("SAW 112").count());
        assert_eq!(
            typing,
            Some(0),
            "⚠⚠⚠ NOT ONE STIMULUS WHILE THEY WERE STILL TYPING. Between their first keystroke and \
             their last the pane is theirs, and a run that resumed in a 100 ms pause typed into \
             the middle of somebody's sentence — which is the defect `Handback::of` refuses a \
             stillness of zero to prevent, asserted here instead of promised: {witness:?}",
        );
        assert_eq!(
            witness.matches("HANDED BACK").count(),
            5,
            "⚠⚠ AND THE CONTROL: all five keystrokes reached the peer, so the span above is the \
             whole of their typing rather than a window that closed early: {witness:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A RUN SOMEBODY IS WATCHING WAITS FOR THEM, AND GOES ON WHERE IT LEFT OFF.**
    ///
    /// The sibling of the gate above, and the whole difference between an unattended run and a
    /// supervised one. Same peer, same missing clause — but here a PERSON is at the pane and
    /// answers the dialog a moment later, which is what the inner session of a supervised loop
    /// looks like: it is on somebody's screen and they can read every turn as it happens.
    ///
    /// Measured before [`Attended`] existed: the run reported `blocked` in under a second and the
    /// person's answer landed in a pane nobody was driving any more. The turn they were supervising
    /// stopped at the halfway mark for no reason but the absence of a way to say *"wait for me"*.
    #[test]
    fn a_watched_run_waits_for_the_person_and_resumes() {
        let (access, pane) = crate::testing::two_question_peer();
        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "carry on".to_string(),
                sentinel: Some("TURN COMPLETE".to_string()),
                ready_when: None,
                ready_within: Some(Duration::from_secs(15)),
                may_answer: Consents::of(vec![
                    crate::consent::Consent::parse(
                        "Do you want to proceed?".to_string(),
                        "Yes".to_string(),
                    )
                    .expect("two needles"),
                ]),
                // ⚠ `Handback::Never`: this gate is about a person ANSWERING a question the run
                // stopped on, which is the opposite direction from one TAKING the pane, and a
                // handback declared here would put a second wait in front of the one it measures.
                attended: Attended::of(Duration::from_secs(20), Handback::Never)
                    .expect("a positive patience"),
                turn: None,
            },
        );

        // The person, at the keyboard of the pane they are supervising. They answer the question
        // the run has no clause for — the second one — and nothing else.
        let outcome = std::thread::scope(|watching| {
            let watcher = watching.spawn(|| {
                crate::testing::screen_showing(&access, pane, "make this edit");
                let _typed = access
                    .inject(pane, &crate::access::KeyStroke::text("2"))
                    .expect("the person types");
            });
            let outcome = run(
                &access,
                &mut orch,
                Guardrails {
                    max_iterations: 8,
                    max_cost: None,
                    max_duration: Some(Duration::from_secs(60)),
                },
            );
            watcher.join().expect("the person's thread");
            outcome
        });

        assert!(
            matches!(outcome.state, OutcomeState::Converged),
            "⚠⚠⚠ the person answered the question the run could not, so the turn FINISHED — a run \
             that reports `blocked` here ended while its supervisor was still typing: {outcome:?}",
        );
        assert_eq!(
            outcome.answered, 1,
            "⚠⚠⚠ and the tally counts what THIS RUN answered, which is the first question only. \
             A person's answer is not the machine's, and a run that counts it has lost the one \
             distinction that makes an approval traceable: {outcome:?}",
        );
        let screen = crate::testing::screen_showing(&access, pane, "TURN COMPLETE");
        assert!(
            screen.contains("TOOK-2-2-VIA-50"),
            "⚠⚠ and the second question was taken with the PERSON's option (2), not one this run \
             chose while waiting: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠⚠ **THIS PLUGIN NO LONGER TYPES INTO A PANE IT CAN ASK ABOUT** — register items 310,
    /// 320, 324 and 325, and **this gate is a REPURPOSING rather than a new one**.
    ///
    /// # What it used to assert, and why it is kept
    ///
    /// It measured the defect, because nothing in this workspace had: **5 bytes and 509 ms a step,
    /// so 3,380 steps — about 29 minutes — from a dead peer to the 16,896-byte wall**
    /// `writing_to_a_dead_pane_comes_back` measures in `sprag-terminal`. Not a burst; a patient march,
    /// which is why the 43 hours went unnoticed while they were being spent. **A gate that measures
    /// a defect goes red when you fix it — repurpose it, do not delete it**, so the same fixture
    /// now holds the opposite claim and the numbers stay in the sentence it prints.
    ///
    /// # What it asserts now
    ///
    /// Every step answers [`Verdict::PeerGone`] naming this pane, and **zero bytes reach the
    /// pseudoterminal**. All four halves matter: a refusal that still wrote would reach the wall
    /// anyway, only slower; a refusal that did not name the pane is the failure R396-R399 spent
    /// four rounds on; the note a caller reads has to carry the reason, or the next reader wraps it
    /// in a retry; and it must be a VERDICT rather than an `Err`, which is the whole of register
    /// item 326 — a run stopped this way is a word in the same closed set as `converged`, so
    /// *which of my runs stopped because its agent's process left?* is a question a journal
    /// answers instead of a grep over free text.
    ///
    /// ⚠⚠⚠ **THE REFUSAL IS AT `PaneAccess::inject` AND NOT IN THIS PLUGIN**, which is why one
    /// change covers `Agent`, `Pipe`, `Dialogue` and the AI loop as well: that function is *"the
    /// door a PLUGIN types through"* by its own doc, and a guard per plugin would be four copies
    /// of one decision plus a fifth plugin arriving unprotected. **What each plugin still owns is
    /// the WORD it turns that refusal into**, which is why there is a `match` here and in `AiLoop`
    /// and not a shared helper: this one stops the run, and the loop's tells its DOCUMENT.
    ///
    /// ⚠⚠ **AND THE EVIDENCE NEEDED NO NEW MACHINERY** — item 324's corrected sentence:
    /// `pane_eof` already answered, and two readers already consulted it
    /// ([`DoneWhen::Exits`](crate::completion::DoneWhen::Exits), [`Pipe`](crate::pipe::Pipe)); the
    /// hand on the keyboard did not.
    ///
    /// # ⚠⚠ What this gate does NOT hold, stated rather than left to be assumed
    ///
    /// That the run ENDS. `Verdict::PeerGone` is terminal at the Driver, and this gate steps the
    /// plugin by hand precisely so that the claim is about the PLUGIN's answer — four steps in a
    /// row, all identical, which is what shows the refusal does not wear off. The run-level ending
    /// is the Driver's own business and is held there.
    #[test]
    fn a_step_refuses_a_pane_this_run_can_already_know_is_dead() {
        /// What `writing_to_a_dead_pane_comes_back` measured on this host. ⚠ A kernel's number, not
        /// sprag's — it is here to be DIVIDED BY, and the projection it feeds is printed rather
        /// than asserted, so a different host changes the report and not the verdict.
        const WALL: u64 = 16_896;
        /// Enough steps to show the cost is per-step and linear, few enough to stay far short of
        /// the wall.
        const STEPS: u64 = 4;

        let (access, pane) = sh_access("exit 0", 20, 4);
        let began = Instant::now();
        while access.pane_eof(pane) != Some(true) && began.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "⚠ THE FIXTURE: the child must be gone, or nothing below is about a dead pane",
        );

        let mut orch = Orchestrator::new(
            pane,
            OrchestrationSpec {
                stimulus: "ping".to_string(),
                sentinel: None,
                // ⚠ NO BARRIER. A dead pane can never clear one, so a run that declared a barrier
                // would be refused before it typed — which is a DIFFERENT (and safe) story, and
                // not the one that wedged a machine. The wedge was reached by a run already past
                // its barrier when its peer died.
                ready_when: None,
                ready_within: None,
                may_answer: None,
                attended: Attended::NoOne,
                turn: None,
            },
        );

        let run = crate::run::RunContext::uncancellable();
        let mut spent = 0;
        for step in 1..=STEPS {
            let taken = orch.step(&access, &run).unwrap_or_else(|error| {
                panic!(
                    "⚠⚠⚠ step {step} must answer with a VERDICT and not an error. `peer_gone` is a \
                     word in the same vocabulary as `converged` — a run stopped this way is a fact \
                     a journal can be asked about, where an error is free text a reader greps: \
                     {error}"
                )
            });
            assert_eq!(
                taken.verdict,
                Verdict::PeerGone(pane),
                "⚠⚠⚠⚠ step {step} TYPED at a pane whose program has exited, or stopped for some \
                 other reason. That is the defect this gate was written to measure and now guards \
                 against: at 5 bytes a step it is 3,380 steps — about 29 minutes — to a wedged \
                 machine. And the verdict must name THIS pane, because a run stopped without \
                 saying which one is the defect R396-R399 was four rounds of",
            );
            let said = taken.note.clone().unwrap_or_default();
            assert!(
                said.contains(&format!("pane {}", pane.0)) && said.contains("blocks for ever"),
                "⚠⚠ and the SENTENCE a caller reads must carry the pane and the reason, or a \
                 reader adds a retry: {said:?}",
            );
            spent += taken.cost.amount();
        }

        assert_eq!(
            spent, 0,
            "⚠⚠⚠⚠ nothing may reach that pseudoterminal at all. A refusal that still wrote would \
             walk to the {WALL}-byte wall exactly as before, only more slowly",
        );
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "⚠ and the evidence the refusal stands on was answerable throughout — it is not a \
             reading anybody had to go and fetch (register items 311, 324)",
        );

        println!(
            "\n== an orchestrator at a pane whose child is dead ==\n  {STEPS} steps, 0 bytes \
             typed, every one refused as PeerGone naming pane {}\n  it used to type 5 bytes a \
             step: {} steps to the {WALL}-byte wall, about 29 minutes\n",
            pane.0,
            WALL.div_ceil(5),
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }
}
