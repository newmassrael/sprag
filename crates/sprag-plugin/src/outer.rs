//! **THE OUTER DRIVER** — what makes [`ai_loop.scxml`]'s machine act on a real pane.
//!
//! R376 compiled that document for the first time; what it bought was a machine nothing drove.
//! Its three unbuilt states were recorded as needing *"the outer DRIVER — the thing that raises
//! `turn.done` / `turn.blocked` / `turn.interrupted` from what a pane is doing"*. This is that
//! thing, and R377 built the half of it that could not be written before: a turn's end can now say
//! WHICH ending it was ([`Over`]), which is what those first two events are made of.
//!
//! # ⚠⚠⚠ Why the driver is STATE-DRIVEN and not event-driven
//!
//! The document reads like a list of instructions — `priming` does `<send event="prompt.start"/>`,
//! `restarting` does `<send event="session.replace"/>`, and seven such sends name every effect a
//! driver has to perform. Subscribing to them is the obvious design **and it cannot be built**:
//!
//! * a targetless `<send>` is W3C SCXML 6.2's *external event to SELF*, so the generated code
//!   raises it onto the machine's OWN queue, where no transition in this document listens and it
//!   is dropped;
//! * the one handle that looks like a subscription, `Engine::get_external_queue_handle`, exists for
//!   `#_parent` sends out of `<invoke>`d CHILD machines and **mints a fresh empty queue per call**.
//!
//! `the_machine_instructs_its_driver_through_its_state_not_through_its_sends` is where that was
//! established by running it rather than by reading the codegen — R376's lesson, one round old.
//!
//! The machine's own published ingress partition says the same thing from the other side:
//! `prompt.sent` (the driver's ANSWER) is externally drivable and `prompt.start` (the supposed
//! instruction) is not. **Nobody outside sends it, so nobody outside is meant to receive it.**
//!
//! # ⚠⚠ Which leaves ONE thing the state cannot say, and it is why [`Owed`] exists
//!
//! Four different transitions arrive at `working`, and they do not agree about whether a prompt
//! goes with them: `judging --judge-->`, `awaiting_human --resume-->` and
//! `reflecting --reflect.none-->` each carry `<send event="prompt.turn"/>`, while
//! `screening --screen.matched-->` deliberately carries none — the agent has just been handed its
//! answer and is already working, so prompting it there would type over a peer mid-turn.
//!
//! A driver reading only *"I am in `working`"* cannot tell those apart. What it does know is
//! **which event it just raised**, because it raised it — so the document's table is recovered
//! from the EVENT, as an exhaustive match a new transition cannot be added past.
//!
//! # ⚠ Why the script engine is a PARAMETER
//!
//! `ai_loop.scxml` declares `datamodel="ecmascript"`, so constructing its machine needs an
//! [`IScriptEngine`], and at the pinned SCE rev the only one is `sce-rust-lua` — which this crate
//! carries as a **dev-dependency**, deliberately, so the daemon does not link mlua and its C Lua
//! toolchain for a machine nothing in the product constructs yet.
//!
//! Taking the engine as an argument is what keeps that true while this is built and measured. The
//! day a mouth constructs an [`OuterLoop`] in the daemon, that decision has to be retaken — and
//! this is the constructor that will force whoever does it to notice.
//!
//! [`ai_loop.scxml`]: ../../ai_loop.scxml
//! [`IScriptEngine`]: sce_rust_runtime::IScriptEngine

use std::sync::Arc;
use std::time::Duration;

use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};
use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::completion::{Completion, DoneWhen, Over, Turn};
use crate::deliver::{Delivered, Delivery, deliver};
use crate::readiness::{Reached, Readiness, ReadyWhen};
use crate::run::RunContext;
use crate::sm::ai_loop::{AiLoopEvent, AiLoopPolicy, AiLoopState};

/// How much of a long prompt has to be read back off the pane before it counts as delivered.
///
/// [`Agent`](crate::agent::Agent)'s number, for its reason: an agent's prompt box is a BOX, so a
/// prompt longer than the pane is wide arrives on screen in pieces and no single run of it is
/// findable. This is the point at which a leading fragment stops being a coincidence.
const CONFIRM_WHOLE_UP_TO: usize = 40;

/// The contract `DoneWhen` a loop drives its inner session with, named once.
///
/// An agent CLI answers and goes on waiting, which is [`DoneWhen::Settles`] exactly — and it is
/// the arm the outer loop makes load-bearing, where every gate before R377 drove
/// [`DoneWhen::Exits`] because `Settles` needed a supervisor.
///
/// ⚠ ITS ONLY READER IS A TEST, which R355's rule calls a comment rather than a constant. It stays
/// because the reader it is waiting for is the construction site in the daemon — see the module
/// doc — and it is registered as owed rather than left to look settled.
pub const INNER_SESSION_ENDS: DoneWhen = DoneWhen::Settles;

/// **WHAT THE DOCUMENT AUTHORS**, read out of the machine's own datamodel rather than retyped.
///
/// ⚠ Read through the SCRIPT SESSION and not off the policy, and not by choice: SCE lowers every
/// scalar `<data>` into a private Rust field and emits no accessor for any of them, so the
/// interpreter's copy is the only readable one. That is PR-86's third ask, and until it lands this
/// is how a consumer asks the machine what it was authored with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authored {
    /// Sent once, into a freshly opened session.
    pub start: String,
    /// Sent on every turn after the first.
    pub turn: String,
    /// Sent once before the loop reports converged.
    pub end: String,
    /// What the agent says when it has reached the milestone.
    pub done_marker: String,
}

impl Authored {
    /// Read the four authored strings out of `engine`'s datamodel.
    ///
    /// [`None`] for a datamodel that does not hold them as strings, which is a machine this driver
    /// cannot drive — and saying so here is what stops a run being started against one.
    fn read(script: &Arc<dyn IScriptEngine>, session: &str) -> Option<Self> {
        let text = |name: &str| match script.get_variable(session, name) {
            Ok(ScriptValue::String(value)) => Some(value),
            _ => None,
        };
        Some(Self {
            start: text("start_prompt")?,
            turn: text("turn_prompt")?,
            end: text("end_prompt")?,
            done_marker: text("done_marker")?,
        })
    }
}

/// **WHETHER THE TRANSITION THIS DRIVER JUST CAUSED OWES THE PEER A PROMPT** — the document's own
/// `<send>` table, recovered from the event because the state cannot carry it.
///
/// See the module doc: four transitions reach `working` and only three of them prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owed {
    /// Nothing to say — the peer is already working, or is not the thing being waited on.
    Nothing,
    /// The `start_prompt`, into a session that has never been prompted.
    Start,
    /// The `turn_prompt` — another turn on the same session.
    Turn,
    /// The `end_prompt` — the closing report.
    End,
}

impl Owed {
    /// What the document says goes with arriving at `landed` by raising `raised`.
    ///
    /// # ⚠⚠ The two halves of `ai_loop.scxml`'s sends, and why only one needs the event
    ///
    /// `prompt.start` and `prompt.end` are **onentry** sends — `priming`'s and `closing`'s — so
    /// arriving at those states is the whole condition, whichever transition brought you. Both are
    /// reached more than one way (`priming` from `idle` and from `restarting`), and keying them on
    /// the event would have needed that list kept in step by hand.
    ///
    /// `prompt.turn` is a **transition** send, on three of the four edges into `working`, and that
    /// is the one place the arrival state is not enough. So the event decides there and only
    /// there.
    ///
    /// ⚠ EXHAUSTIVE over the machine's whole event vocabulary in that arm on purpose: an edge
    /// added into `working` lands here as a variant that no longer compiles, which is the only
    /// mechanism that stops a driver silently not saying something the author wrote.
    const fn on(raised: AiLoopEvent, landed: AiLoopState) -> Self {
        match landed {
            AiLoopState::Priming => Self::Start,
            AiLoopState::Closing => Self::End,
            AiLoopState::Working => match raised {
                // `judging --judge-->`, `awaiting_human --resume-->` and
                // `reflecting --reflect.none-->` each carry `<send event="prompt.turn"/>`.
                AiLoopEvent::Judge | AiLoopEvent::Resume | AiLoopEvent::ReflectNone => Self::Turn,
                // ⚠⚠ `priming --prompt.sent-->` carries none because the START prompt is already
                // in the pane, and `screening --screen.matched-->` carries none DELIBERATELY: the
                // peer has just been handed its answer by the driver's own keystroke and is
                // working on it. A prompt on either edge types over a peer mid-turn, which is the
                // failure class this crate keeps paying for.
                AiLoopEvent::PromptSent
                | AiLoopEvent::ScreenMatched
                | AiLoopEvent::Cancel
                | AiLoopEvent::ErrorExecution
                | AiLoopEvent::Fail
                | AiLoopEvent::Hold
                | AiLoopEvent::NotifyHuman
                | AiLoopEvent::PromptEnd
                | AiLoopEvent::PromptStart
                | AiLoopEvent::PromptTurn
                | AiLoopEvent::ReflectApplied
                | AiLoopEvent::ReflectBegin
                | AiLoopEvent::ScreenBegin
                | AiLoopEvent::ScreenNone
                | AiLoopEvent::SessionReady
                | AiLoopEvent::SessionReplace
                | AiLoopEvent::Start
                | AiLoopEvent::TurnBlocked
                | AiLoopEvent::TurnDone
                | AiLoopEvent::TurnInterrupted
                | AiLoopEvent::Unattended
                | AiLoopEvent::Null => Self::Nothing,
            },
            AiLoopState::Idle
            | AiLoopState::Judging
            | AiLoopState::Screening
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting
            | AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::Blocked => Self::Nothing,
        }
    }
}

/// **WHAT ONE PASS OF THE DRIVER DID** — and, when it could not, exactly what it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pumped {
    /// An effect was performed, the machine was told, and it moved. Keep pumping.
    Moved {
        /// Where it was.
        from: AiLoopState,
        /// What the driver raised.
        raised: AiLoopEvent,
        /// Where it is now.
        to: AiLoopState,
        /// **HOW MANY BYTES THIS PASS PUT INTO THE PANE** — nought for a pass that only watched.
        ///
        /// ⚠⚠ Reported because the alternative is dropping it, and the one thing every bounded run
        /// in this crate can say is what it spent. The compiler said so first: `inject` answers a
        /// `#[must_use]` [`Written`](crate::access::Written) and the first draft threw it away —
        /// R316's rule, which this workspace has paid for at eight call sites before.
        ///
        /// ⚠ Nothing CONSUMES it yet: the outer loop has no [`Guardrails`](crate::driver::Guardrails)
        /// equivalent, which is registered debt. This is the fact a ceiling would be built on, and
        /// it is carried from the round that could first produce it rather than added later.
        spent: u64,
    },
    /// **THE MACHINE IS IN A STATE THIS DRIVER CANNOT SERVE YET.**
    ///
    /// Not a failure and not a stall: the state compiles, the machine is correctly in it, and the
    /// effect it names has no implementation here. Returned rather than ignored so a caller learns
    /// WHICH of the unbuilt states its run reached — `screening` and `reflecting`/`restarting` are
    /// registered debt with named prerequisites, and a run that silently spun in one of them would
    /// report the same thing as a run that never got there.
    Unbuilt(AiLoopState),
    /// The loop reached one of the document's five final states.
    Ended(AiLoopState),
}

/// A run of `ai_loop.scxml`'s machine against one pane.
pub struct OuterLoop {
    /// The compiled document.
    machine: Engine<AiLoopPolicy>,
    /// The engine its `<data>` lives in, and the session id it files them under.
    script: Arc<dyn IScriptEngine>,
    session: String,
    /// The four authored strings.
    authored: Authored,
    /// The inner session's pane.
    pane: PaneId,
    /// The barrier the pane must clear before anything is typed into it.
    ready: Readiness,
    /// What makes the inner agent's turn over, and how long one may take.
    turn: Turn,
    /// **WHETHER THE INNER AGENT PAINTS THE PROMPT BOX IT IS TYPED INTO.**
    ///
    /// [`AgentSpec::shows_the_prompt`](crate::agent::AgentSpec::shows_the_prompt)'s knob, and this
    /// loop needs it for the same measured reason. [`deliver`] reads the prompt back off the
    /// SCREEN before it presses Enter, and withholds the press when the text is demonstrably
    /// absent — which is right, and is what stops a submit landing on a pane that swallowed the
    /// question.
    ///
    /// A real agent CLI renders each character into its prompt box as it arrives, so the read-back
    /// succeeds. A peer that only paints once it has a whole LINE cannot be confirmed before the
    /// newline that would submit it, so confirming first is a deadlock: **measured here, where the
    /// single-line `end_prompt` was never submitted and the loop sat in `closing` until its
    /// bound.** The multi-line prompts got through only because their own embedded newlines made
    /// the peer paint mid-delivery, which is an accident of how they are authored.
    shows_the_prompt: bool,
    /// This turn's evaluator, armed before the prompt goes in.
    done: Completion,
}

impl OuterLoop {
    /// Drive the machine `script` evaluates against `pane`, waiting `turn` for each of the inner
    /// agent's turns and holding the pane to `ready_when` before typing anything.
    ///
    /// [`None`] when the machine's datamodel does not carry the four authored strings — see
    /// [`Authored::read`].
    ///
    /// ⚠ The engine is the CALLER's — see the module doc. Constructing one in the daemon is the
    /// decision that makes `sce-rust-lua` a real dependency, and it has not been taken.
    #[must_use]
    pub fn new(
        script: Arc<dyn IScriptEngine>,
        pane: PaneId,
        ready_when: Option<ReadyWhen>,
        turn: Turn,
        shows_the_prompt: bool,
    ) -> Option<Self> {
        let mut machine = Engine::new(AiLoopPolicy::new(Arc::clone(&script)));
        machine.initialize();
        let session = machine.policy().session_id.clone()?;
        let authored = Authored::read(&script, &session)?;
        Some(Self {
            done: Completion::new(turn.when()),
            // ⚠ NO CONSENTS AND NOBODY WATCHING, and both are the machine's job rather than the
            // barrier's. `screening` is where this document answers a dialog — from the person's
            // standing rules, as a state a reader can see happened — and `awaiting_human` is where
            // it waits. A consent given to the barrier would answer dialogs one level below the
            // machine that exists to decide about them.
            ready: Readiness::new(ready_when, None, None, crate::readiness::Attended::NoOne),
            machine,
            script,
            session,
            authored,
            pane,
            turn,
            shows_the_prompt,
        })
    }

    /// Where the machine is.
    #[must_use]
    pub fn state(&self) -> AiLoopState {
        self.machine.get_current_state()
    }

    /// What the document was authored with.
    #[must_use]
    pub const fn authored(&self) -> &Authored {
        &self.authored
    }

    /// How many turns the machine counts having taken — its own number, not one kept out here.
    ///
    /// ⚠ Read from the interpreter for [`Authored::read`]'s reason: SCE emits no accessor for a
    /// lowered scalar `<data>`. A caller that kept its own tally would be a second authority on
    /// the one fact the document's budget guards compare against.
    #[must_use]
    pub fn turns(&self) -> Option<i64> {
        match self.script.get_variable(&self.session, "turns") {
            Ok(ScriptValue::Int(turns)) => Some(turns),
            _ => None,
        }
    }

    /// **ONE PASS**: perform what the machine's current state asks for, then tell it what happened.
    ///
    /// The whole driver is this function and the two tables it consults — [`Owed`] for what a
    /// transition says, and the match below for what a state asks.
    pub fn pump(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Pumped, PaneError> {
        let from = self.state();
        if self.machine.is_in_final_state() {
            return Ok(Pumped::Ended(from));
        }
        let raised = match from {
            // Nothing has happened yet. Starting the loop is the caller's act, and it is the one
            // event with no pane behind it.
            AiLoopState::Idle => AiLoopEvent::Start,

            // A session exists and has not been prompted. The prompt itself was already delivered
            // by whichever transition brought us here — see `advance`.
            AiLoopState::Priming => AiLoopEvent::PromptSent,

            // ⚠⚠⚠ THE STATE THE WHOLE ROUND WAS ABOUT. The inner agent is working and the driver
            // watches its pane; what the turn ENDS ON is what the machine is told.
            AiLoopState::Working | AiLoopState::Closing => self.watch(panes, run)?,

            // One turn has landed. The document decides in priority order and the only thing it
            // needs from out here is whether the agent said it was done — `judge`'s first guard is
            // `_event.data.done`, which is the one event on this surface that carries data.
            AiLoopState::Judging => AiLoopEvent::Judge,

            // ⚠⚠ REGISTERED DEBT, NOT AN OVERSIGHT — and each has its own reason, kept where a
            // reader meets it:
            //
            // * `screening` has two OWNER decisions in front of it: the document matches a dialog
            //   by KIND (`design-decision`, …) and `sprag-detect` classifies no kinds, and the
            //   rules want Escape-then-type where `Consents` can only SELECT an offered option.
            // * `reflecting` + `restarting` need the session REPLACE lifecycle — close the pane,
            //   write the improvements, open a fresh one that reads them on the way up.
            //
            // Reported rather than skipped: a driver that treated these as no-ops would take the
            // loop somewhere the author did not write.
            state @ (AiLoopState::Screening
            | AiLoopState::AwaitingHuman
            | AiLoopState::Reflecting
            | AiLoopState::Restarting) => return Ok(Pumped::Unbuilt(state)),

            // `is_in_final_state` answered above; these are the same five, and naming them keeps
            // the match exhaustive without a wildcard that would swallow a sixth.
            state @ (AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::Blocked) => return Ok(Pumped::Ended(state)),
        };
        let (to, spent) = self.advance(panes, run, raised)?;
        Ok(Pumped::Moved {
            from,
            raised,
            to,
            spent,
        })
    }

    /// **WATCH THE INNER AGENT'S TURN, AND SAY HOW IT ENDED** — the translation debt 74 named.
    ///
    /// The barrier is asked first, because a person reaching into the pane outranks everything the
    /// peer is doing: `ai_loop.scxml` is explicit that somebody who is typing is already dealing
    /// with whatever is on that screen.
    fn watch(
        &mut self,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Result<AiLoopEvent, PaneError> {
        match self.ready.reached(panes, self.pane, run)? {
            // A PERSON TOOK THE PANE. R372's product half, reaching the machine at last.
            Reached::Interrupted(_) => return Ok(AiLoopEvent::TurnInterrupted),
            // The run ended underneath — cancelled, or out of time.
            Reached::RunEnded(_) => return Ok(AiLoopEvent::Cancel),
            // The peer is asking before this turn even started, which the machine has an answer
            // for: it is the same question `turn.blocked` sends to `screening`.
            Reached::Asking(_) => return Ok(AiLoopEvent::TurnBlocked),
            // ⚠ The barrier answers these only for a run that declared consents or an attendant,
            // and this one declares neither — see `new`. They mean the pane is mid-transition, so
            // the honest event is none at all and the next pump asks again.
            Reached::Answered(_) | Reached::Attended(_) | Reached::HandedBack(_) => {
                return Ok(AiLoopEvent::Null);
            }
            Reached::Yes => {}
        }
        // ⚠⚠⚠ AND THIS IS THE WHOLE POINT OF R377. Before `Over` existed the two endings a real
        // agent's turn actually has — *it answered* and *it stopped to ask* — were one answer out
        // here, so `turn.done` and `turn.blocked` had no producer and this function could not be
        // written.
        Ok(
            match self.done.wait(panes, self.pane, self.patience(), run) {
                Over::Yes => AiLoopEvent::TurnDone,
                Over::Asking(_) => AiLoopEvent::TurnBlocked,
                // The peer is still working after the turn's own bound. NOT an event: the machine has
                // no *the turn overran* transition, and inventing one out here would put a decision in
                // the driver that belongs in the document. The run's own clock bounds it, and the next
                // pump asks again.
                Over::NotYet => AiLoopEvent::Null,
                Over::RunEnded => AiLoopEvent::Cancel,
            },
        )
    }

    /// How long one of the inner agent's turns may take — the caller's [`Turn`], or the run's own
    /// clock where they declined a bound.
    fn patience(&self) -> Duration {
        self.turn.within().unwrap_or(Duration::MAX)
    }

    /// Tell the machine `raised` happened, then pay whatever the transition it took owes the peer.
    ///
    /// ⚠ THE ORDER IS THE CONTRACT. The prompt is delivered AFTER the machine has moved, because
    /// which prompt is owed depends on where it landed — `judge` reaches `working`, `closing`,
    /// `reflecting` and `exhausted`, and only two of those are spoken to.
    fn advance(
        &mut self,
        panes: &dyn PaneAccess,
        run: &RunContext,
        raised: AiLoopEvent,
    ) -> Result<(AiLoopState, u64), PaneError> {
        // ⚠ `Null` is W3C SCXML 3.13's eventless sentinel and must never be injected: it is what
        // `watch` answers when nothing happened, and the machine stays put.
        if raised == AiLoopEvent::Null {
            return Ok((self.state(), 0));
        }
        // ⚠⚠ `judge` IS THE ONE EVENT THAT CARRIES DATA, and `process_event` cannot send any — it
        // is `raise_external(event, "", "")` followed by a macrostep. So the goal-met guard is
        // reached through the raise that takes a payload, and every other event through the
        // convenience call that does not.
        if raised == AiLoopEvent::Judge {
            let done = self.said_done(panes);
            self.machine
                .raise_external(raised, &format!("{{\"done\": {done}}}"), "");
            self.machine.step();
        } else {
            self.machine.process_event(raised);
        }
        let landed = self.state();
        let text = match Owed::on(raised, landed) {
            Owed::Nothing => return Ok((landed, 0)),
            Owed::Start => &self.authored.start,
            Owed::Turn => &self.authored.turn,
            Owed::End => &self.authored.end,
        };
        let spent = self.say(panes, run, &text.clone())?;
        Ok((landed, spent))
    }

    /// Put `text` in the pane and arm this turn's completion contract.
    ///
    /// ⚠⚠⚠ ARMED BEFORE A BYTE GOES IN, which is [`Completion::begin`]'s whole guarantee: an agent
    /// waiting to be spoken to is AT REST, so a contract armed after the injection is satisfied by
    /// the stillness the turn was addressed TO — and the loop would judge a turn the agent had not
    /// started.
    fn say(
        &mut self,
        panes: &dyn PaneAccess,
        run: &RunContext,
        text: &str,
    ) -> Result<u64, PaneError> {
        self.done = Completion::new(self.turn.when());
        self.done.begin(panes, self.pane);
        if !self.shows_the_prompt {
            // The WRITE, not the delivery — see [`shows_the_prompt`](Self::shows_the_prompt). A
            // peer that paints nothing until it is submitted cannot be confirmed before the submit.
            let mut keys = crate::access::KeyStroke::text(text);
            keys.push(crate::access::KeyStroke::named("Enter"));
            return Ok(panes.inject(self.pane, &keys)?.bytes());
        }
        let delivered = deliver(
            panes,
            run,
            self.pane,
            text,
            &Delivery {
                // A prompt longer than the pane is wide arrives in pieces — see the constant.
                confirm: (text.chars().count() > CONFIRM_WHOLE_UP_TO)
                    .then(|| text.chars().take(CONFIRM_WHOLE_UP_TO).collect::<String>()),
                then_press: vec![crate::access::KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        )?;
        // ⚠ A prompt the pane never took is a REFUSAL, not a turn to wait out. The alternative is
        // a loop that waits its whole bound for an answer to a question that was never asked, and
        // then judges the screen anyway — this crate's most expensive failure class.
        if let Delivered::Unconfirmed { attempts, written } = delivered {
            return Err(PaneError::NeverTook {
                attempts,
                written: written.bytes(),
            });
        }
        Ok(delivered.written().bytes())
    }

    /// Whether the pane is showing what the document calls done — the one fact `judging` needs
    /// from out here.
    ///
    /// ⚠ Read off the COLLAPSED screen, so a marker the agent wrapped across rows is still found.
    #[must_use]
    pub fn said_done(&self, panes: &dyn PaneAccess) -> bool {
        panes
            .pane_collapsed(self.pane)
            .is_some_and(|seen| seen.contains(&self.authored.done_marker))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::testing::started;
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::Mutex;

    /// A stand-in AGENT CLI: long-lived, echo off, answers every prompt and says the document's
    /// done marker once it has taken `turns_before_done` turns.
    ///
    /// ⚠⚠ **ECHO OFF AND A READINESS MARKER THAT ENDS ITS ROW** — both recorded hazards. With echo
    /// on, the line discipline paints the prompt before the program reads a byte and every wait
    /// ends on the kernel's work rather than the peer's; with the marker mid-row, the first
    /// stimulus merges onto it and reads as the pane's own output.
    ///
    /// ⚠ It is `read`-driven, so it consumes what is typed at it. A stand-in that merely SLEEPS
    /// does not stand in for an agent: nothing eats the stimulus, so it waits in the pty buffer
    /// and the run converges either way.
    ///
    /// ⚠⚠⚠ **IT ANSWERS ONE PROMPT, NOT ONE LINE, AND THE FIRST RUN OF THIS GATE IS WHY.** The
    /// authored prompts are MULTI-LINE — `start_prompt` is four clauses joined with `\n` — so a
    /// `read line` stand-in took one delivery as four turns and said the done marker during the
    /// first one. The peer therefore keys on each prompt's LAST clause, which is the honest shape:
    /// a real agent CLI takes a whole prompt box and answers it once.
    ///
    /// ⚠⚠ **AND THE PRODUCT QUESTION THAT FOUND IS REGISTERED, NOT FIXED HERE**: what a newline
    /// INSIDE an authored prompt does to a peer that submits on Enter is a live question about
    /// delivery, and this fixture is not the place to answer it.
    ///
    /// ⚠⚠⚠ **IT PAINTS WHAT IT READS, AND THE SECOND RUN OF THIS GATE IS WHY.** With echo off and
    /// nothing painted, [`deliver`] can never confirm the prompt arrived, so it RETYPES it — and a
    /// peer counting prompts saw two where the driver sent one, converging a turn early. A real
    /// agent CLI paints the prompt into its own box, which is the whole reason `deliver` reads the
    /// screen back; a stand-in that stayed silent was testing the retry path, not the loop.
    fn standin_agent(prompts_before_done: u32) -> (Arc<Mutex<Workspace>>, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 16))));
        let script = format!(
            "stty -echo; printf 'AGENT-READY\\n'; n=0; \
             while read line; do \
               printf '%s\\n' \"$line\"; \
               case \"$line\" in \
                 *eport*|*Summarise*) ;; \
                 *) continue;; \
               esac; \
               n=$((n+1)); \
               if [ $n -ge {prompts_before_done} ]; then printf 'MILESTONE REACHED\\n'; \
               else printf 'ACK %s\\n' \"$n\"; fi; \
             done"
        );
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 16)
                .expect("spawn pane")
        };
        started(
            &WorkspacePaneAccess::new(Arc::clone(&workspace)),
            pane,
            "AGENT-READY",
        );
        (workspace, pane)
    }

    /// The supervision a real host would provide, derived from the peer's OWN output.
    ///
    /// # ⚠⚠⚠ Why `seq` carries the whole signal and the STATE is always at rest
    ///
    /// A shell peer with echo off paints nothing between reading a prompt and answering it, so
    /// there is no moment at which a screen-derived detector could honestly call it *working* —
    /// and a fixture that claimed otherwise would be inventing evidence the pane does not carry.
    ///
    /// What it can say truthfully is HOW MANY answers the peer has produced, which is exactly what
    /// [`AgentObservation::seq`](crate::access::AgentObservation::seq) means: published changes.
    /// So this reports `Idle` always and lets the count do the work — **which puts the whole weight
    /// on [`DoneWhen::Settles`]'s arming**, the discipline that stops a peer's rest from BEFORE a
    /// turn reading as its answer. A driver that dropped the arming would end every turn instantly
    /// against this fixture, and the gate would say so.
    /// ⚠⚠⚠ **AND IT IS HELD MONOTONIC BY HAND, WHICH THE SECOND STALL OF THIS GATE PAID FOR.**
    /// The count is read off the COLLAPSED SCREEN, so it is a claim about the terminal's SIZE as
    /// much as about the peer: once the pane had scrolled, `ACK 1` left the grid and the count went
    /// DOWN — and `seq > began_at` can never be satisfied again by a number that shrank. R375
    /// recorded exactly this trap about counting from a screen; a real detector's `seq` never
    /// decreases while the pane lives, and this is what makes the stand-in honest about that.
    fn supervised(workspace: &Arc<Mutex<Workspace>>) -> WorkspacePaneAccess {
        let source = {
            let workspace = Arc::clone(workspace);
            let high = Arc::new(std::sync::atomic::AtomicU64::new(0));
            Arc::new(move |id: PaneId| {
                let screen = WorkspacePaneAccess::new(Arc::clone(&workspace))
                    .pane_collapsed(id)
                    .unwrap_or_default();
                let answers =
                    (screen.matches("ACK").count() + screen.matches("MILESTONE").count()) as u64;
                let seq = high
                    .fetch_max(answers, std::sync::atomic::Ordering::SeqCst)
                    .max(answers);
                Some(crate::access::AgentObservation {
                    state: sprag_detect::AgentState::Idle,
                    agent: Some("claude".to_string()),
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq,
                    asking: None,
                })
            })
        };
        WorkspacePaneAccess::new(Arc::clone(workspace)).with_agent_state(Some(source))
    }

    /// ⚠⚠⚠ **THE OUTER LOOP DRIVES A REAL PANE — debt 74's driver, end to end.**
    ///
    /// Everything before this round drove `ai_loop.scxml`'s machine by hand-feeding it events. The
    /// register's entry said what was missing: *"the states compile and are driven as a MACHINE;
    /// nothing drives them FROM A PANE."* This pumps the whole authored turn cycle against a live
    /// pseudoterminal running a stand-in agent, and asserts three separable things:
    ///
    /// * **the loop CONVERGES**, which means every seam held — the prompts crossed out of the
    ///   datamodel, the delivery reached the peer, the peer's turns ended on
    ///   [`DoneWhen::Settles`], `judging` read the done marker, and `closing` got its report;
    /// * **the machine's own `turns` counter matches what the peer was actually asked**, so the
    ///   document's budget is comparing against reality rather than against a driver's tally;
    /// * **the run never touched an unbuilt state**, which is what makes the first two claims about
    ///   the built path rather than about a driver that skipped something.
    ///
    /// # ⚠⚠ What this gate is NOT
    ///
    /// It is not a measurement against a real agent — debt 64c, still open, and the reason this
    /// fixture is honest about being a stand-in. What it establishes is that the SEAM exists and
    /// carries a whole loop; how a `claude` session behaves inside it is the next round's question
    /// and needs tens of seconds per turn to ask.
    #[test]
    fn the_outer_loop_drives_a_real_pane_from_idle_to_converged() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let (workspace, pane) = standin_agent(2);
        let access = supervised(&workspace);
        let mut loops = OuterLoop::new(
            Arc::clone(&lua),
            pane,
            Some(ReadyWhen::Settles("claude".to_string())),
            Turn::lasting(INNER_SESSION_ENDS, Some(Duration::from_secs(5)))
                .expect("a non-zero bound"),
            // ⚠ This peer paints only once it has a whole LINE, so it cannot be confirmed before
            // the newline that submits — see `shows_the_prompt`. A real agent CLI renders into its
            // prompt box as the characters arrive and takes the other path.
            false,
        )
        .expect("the document's datamodel must carry its four authored strings");

        assert_eq!(loops.state(), AiLoopState::Idle, "the document's `initial`");
        assert!(
            loops.authored().start.contains("North star"),
            "⚠ THE CONTROL: the authored prompts must have crossed out of the datamodel, or the \
             loop below is delivering nothing: {:?}",
            loops.authored(),
        );
        // ⚠⚠⚠ AND THE MARKER IS ITS OWN CONTROL, because the way it fails is invisible. `said_done`
        // asks whether the screen CONTAINS the marker, and every string contains the empty one — so
        // a `done_marker` that arrived empty makes the loop converge on its first turn, reporting
        // a milestone reached against a screen that says nothing of the kind. The driver did
        // exactly that, and this is the assertion that would have named it in one line.
        assert_eq!(
            loops.authored().done_marker,
            "MILESTONE REACHED",
            "the document's own marker, whole — an empty one is `contains` answering yes to \
             everything: {:?}",
            loops.authored(),
        );
        assert!(
            !loops.said_done(&access),
            "⚠⚠ and the agent has not said it yet, which is what makes the convergence below a \
             claim about the agent rather than about the predicate",
        );

        let run = RunContext::uncancellable();
        let mut walked: Vec<(AiLoopState, AiLoopEvent, AiLoopState)> = Vec::new();
        let mut spent_total = 0_u64;
        let ended = loop {
            // ⚠ Well above the five passes the authored happy path takes and well below the
            // document's own 40-turn ceiling, so a stall is caught by THIS bound and a run that
            // reached `exhausted` would still be the machine's own doing.
            assert!(
                walked.len() < 16,
                "the loop must reach a final state rather than pumping forever: {walked:?}",
            );
            match loops
                .pump(&access, &run)
                .expect("the pane must stay readable")
            {
                Pumped::Moved {
                    from,
                    raised,
                    to,
                    spent,
                } => {
                    spent_total += spent;
                    walked.push((from, raised, to));
                }
                Pumped::Unbuilt(state) => panic!(
                    "⚠⚠ this run reached {state:?}, which no driver serves yet — so the \
                     convergence below would be a claim about a path the author did not write. \
                     Walked: {walked:?}",
                ),
                Pumped::Ended(state) => break state,
            }
        };

        assert_eq!(
            ended,
            AiLoopState::Converged,
            "⚠⚠⚠ the whole authored cycle: prime, work, judge, work, judge, close, converge. \
             Walked: {walked:?}",
        );
        assert_eq!(
            loops.turns(),
            Some(2),
            "⚠⚠ THE MACHINE'S OWN COUNTER, not the driver's. The peer was prompted twice before \
             it said the marker, and the document's budget guards compare against this number — a \
             driver keeping its own tally would be a second authority on it. Walked: {walked:?}",
        );

        // The peer really did receive three prompts: two turns and the closing report.
        let typed = access
            .input_echo()
            .and_then(|echo| echo.pane_recent_input(pane))
            .unwrap_or_default();
        assert!(
            typed.contains("North star") && typed.contains("Summarise what changed"),
            "⚠⚠ the START prompt and the END prompt are different text and both must have reached \
             the pane — a driver that sent one prompt for every state would pass every assertion \
             above. Typed: {typed:?}",
        );
        // ⚠⚠ AND THE RUN SAYS WHAT IT SPENT. Every bounded run in this crate can, and the outer
        // loop could not until the compiler objected to the dropped `Written`. The claim is the
        // RELATION, not a byte count: what reached the pane is what the three authored prompts
        // weigh, so a driver that silently sent nothing — or sent something else — cannot pass.
        let authored_weight = (loops.authored().start.len()
            + loops.authored().turn.len()
            + loops.authored().end.len()) as u64;
        assert!(
            spent_total >= authored_weight,
            "⚠⚠ the three prompts weigh {authored_weight} bytes and the run reports spending \
             {spent_total}. A loop whose spend is less than what it typed is not reporting its \
             own cost, which is the one thing a bounded run always owes. Walked: {walked:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }
}
