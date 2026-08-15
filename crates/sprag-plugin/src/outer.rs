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
//! # ⚠⚠ Which leaves ONE thing the state cannot say, and it is why `Owed` exists
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
//! [`IScriptEngine`], and at the pinned SCE rev the only one is `sce-rust-lua` — which THIS crate
//! still carries as a **dev-dependency**, because nothing here names a concrete engine.
//!
//! ⚠⚠ **THE DECISION IT WAS DEFERRING HAS BEEN TAKEN.** R381 built the door
//! ([`AiLoop`](crate::ai_loop::AiLoop)), so `sprag-host` constructs a `LuaEngine` per run and the
//! daemon links mlua and its C Lua toolchain — the cost the parameter was buying time on, paid
//! because the alternative was a loop that runs end to end against a live agent with no way for
//! anybody to start one.
//!
//! ⚠⚠⚠ **AND THE PARAMETER EARNED ITS KEEP TWICE OVER**: it is what keeps this crate free of an
//! engine, AND it is the seam a gate enters through. Three of this driver's refusals are only
//! reachable by handing in an engine that answers differently about one variable — the witness in
//! this module's own tests. **The decision that kept a dependency out of the product is the same
//! one that made the driver's refusals testable.**
//!
//! # ⚠⚠ Why this module COMPILES, where R378 left it `#[cfg(test)]`
//!
//! It was gated to tests so the daemon would not link mlua. That reason does not hold, and reading
//! the paragraph above is what says so: the engine is a PARAMETER, so nothing here names a concrete
//! [`IScriptEngine`] and nothing here pulls `sce-rust-lua` in. The gate was buying a guarantee the
//! signature already gave.
//!
//! What it COST is what made the difference worth undoing. The one real supervisor in this
//! workspace — `sprag-detect` behind the daemon's per-pane tracker, hysteresis, settle window and
//! all — lives in `sprag-host`, which depends on this crate and could therefore not see a driver
//! compiled only for this crate's own tests. So every measurement of the outer loop had to invent
//! its own supervisor out of a fixture, **which is exactly the thing debt 64c says has never been
//! measured**. A module private to its own tests cannot be driven by the crate that owns the
//! evidence.
//!
//! ⚠ The other two decisions R378 named were *"nothing in the daemon CONSTRUCTS one of these, and
//! no surface starts a loop"*. **Both are paid** — see [`AiLoop`](crate::ai_loop::AiLoop), which
//! is this driver wrapped as a [`Plugin`](crate::plugin::Plugin) so the substrate's own
//! [`Driver`](crate::driver::Driver) bounds it, and `run {plugin: "ai_loop"}` on the wire.
//!
//! [`ai_loop.scxml`]: ../../ai_loop.scxml
//! [`IScriptEngine`]: sce_rust_runtime::IScriptEngine

use std::sync::Arc;
use std::time::{Duration, Instant};

use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};
use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::completion::{Completion, DoneWhen, Over, Turn};
use crate::consent::Unanswered;
use crate::deliver::{Delivered, Delivery, deliver};
use crate::readiness::{Reached, Readiness, ReadyWhen};
use crate::run::RunContext;
use crate::screen::{Malformed, Refused, ScreenRule, ScreenRules, Screened};
use crate::sm::ai_loop::AiLoopPolicy;

/// The machine's own vocabulary, re-exported because [`Pumped`] is made of it.
///
/// ⚠ `sm` is `pub(crate)` — generated code, and not a module anyone outside should reach into — so
/// without this a caller could receive a [`Pumped::Moved`] and have no way to NAME what it holds.
/// A public answer a consumer cannot spell is not a public answer.
pub use crate::sm::ai_loop::{AiLoopEvent, AiLoopState};

/// How much of a long prompt has to be read back off the pane before it counts as delivered,
/// **IN SCREEN COLUMNS**.
///
/// [`Agent`](crate::agent::Agent)'s number, for its reason: an agent's prompt box is a BOX, so a
/// prompt longer than the pane is wide arrives on screen in pieces and no single run of it is
/// findable. This is the point at which a leading fragment stops being a coincidence.
///
/// # ⚠⚠⚠ Columns, not characters — and a live run is what said so
///
/// This was a count of `char`s, and the two are the same number only for narrow text. A Korean
/// prompt's first forty characters occupy about **eighty columns**, so a needle that reads as *"forty
/// wide"* demanded a row twice that wide — measured on a real loop against a real `claude`, whose
/// first run died with `the pane never took the prompt: 3 injections put 2370 bytes on its
/// pseudoterminal and none of them ever appeared on it` **while the prompt was plainly on the
/// screen**. The pane was 38 columns; the needle needed 68.
///
/// ⚠ The refusal was also a LIE, and that cost more than the defect: the text HAD arrived, and what
/// had failed was reading it back. [`Delivered::Unconfirmed`] is *"nothing here can confirm it"* and
/// the sentence promised *"none of them ever appeared"*. See [`PaneError::NeverTook`].
const CONFIRM_WHOLE_UP_TO: usize = 40;

/// **THE LEADING RUN OF `text` THAT FITS IN [`CONFIRM_WHOLE_UP_TO`] SCREEN COLUMNS**, or [`None`]
/// where the whole prompt already does and can be confirmed entire.
///
/// ⚠⚠ The width authority is [`sprag_vt::char_columns`] — the same one the emulator's print path
/// classifies a glyph with — so this cannot disagree with what the pane will actually draw. A second
/// width model here would be a second answer to *"how wide is this?"*, which is the shape this
/// workspace keeps paying for.
///
/// ⚠ It never splits a `char`, so a wide glyph that would straddle the bound is left out entirely:
/// a needle ending mid-glyph is not text any pane ever painted.
fn confirmable(text: &str) -> Option<String> {
    let mut columns = 0_usize;
    let mut prefix = String::new();
    for ch in text.chars() {
        columns += sprag_vt::char_columns(ch);
        if columns > CONFIRM_WHOLE_UP_TO {
            return Some(prefix);
        }
        prefix.push(ch);
    }
    None
}

/// The contract `DoneWhen` a loop drives its inner session with, named once.
///
/// An agent CLI answers and goes on waiting, which is [`DoneWhen::Settles`] exactly — and it is
/// the arm the outer loop makes load-bearing, where every gate before R377 drove
/// [`DoneWhen::Exits`] because `Settles` needed a supervisor.
///
/// ⚠ It had no reader but a test, which R355's rule calls a comment rather than a constant, and it
/// was registered as owed on the argument that *"the reader it is waiting for is the construction
/// site in the daemon"*. **That reader arrived**: [`AiLoopSpec::driving`] builds a real agent's
/// turn contract out of it, and the daemon's `ai_loop` form uses it as the default a caller who
/// names no `done_when` gets.
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
    /// **WHAT THE LOOP ASKS ITS AGENT BEFORE REPLACING ITS SESSION** — `reflecting`'s own prompt.
    ///
    /// ⚠ A consumer previewing a loop reads this to see what its agent will be asked to decide,
    /// which is the one prompt whose ANSWER changes what the run is about.
    pub reflect: String,
    /// What the agent says when it has reached the milestone.
    pub done_marker: String,
}

impl Authored {
    /// Read the five authored strings out of `engine`'s datamodel.
    ///
    /// [`None`] for a datamodel that does not hold them as strings, which is a machine this driver
    /// cannot drive — and saying so here is what stops a run being started against one.
    ///
    /// ⚠⚠ IT ASKS WHETHER THEY ARE THERE, NOT WHETHER THEY SAY ANYTHING. Three of the four are
    /// composed by `priming`'s `onentry`, so a machine still sitting in `idle` holds them empty and
    /// that is correct rather than broken — see [`OuterLoop::authored`].
    fn read(script: &Arc<dyn IScriptEngine>, session: &str) -> Option<Self> {
        let text = |name: &str| match script.get_variable(session, name) {
            Ok(ScriptValue::String(value)) => Some(value),
            _ => None,
        };
        Some(Self {
            start: text(Owed::Start.variable())?,
            turn: text(Owed::Turn.variable())?,
            end: text(Owed::End.variable())?,
            reflect: text(Owed::Reflect.variable())?,
            done_marker: text(DONE_MARKER)?,
        })
    }
}

/// The datamodel variable holding the word the agent says when it is finished.
const DONE_MARKER: &str = "done_marker";

/// The datamodel variable holding the word the agent says when there is **nothing left at all** —
/// asked for by the reflection, and the only thing that reaches `closing`.
///
/// ⚠ Two markers because they are two claims: `DONE_MARKER` ends a MILESTONE and this ends the RUN.
/// A loop that converged on the first was exactly as long as its first checkpoint, measured on a
/// real run that paid one debt and stopped.
const NORTH_STAR_MARKER: &str = "north_star_marker";

/// The datamodel variable saying **WHY this reflection was asked for** — written by whichever
/// `judge` transition reached `reflecting`. See the document, and register item 179.
const REFLECT_REASON: &str = "reflect_reason";

/// [`REFLECT_REASON`]'s value when the agent has just SAID THE MILESTONE IS REACHED.
///
/// ⚠⚠ The one reason whose reflection may not go back to work. A reflection asked because the
/// budget came round, or because a standing instruction fired, returns to a milestone that is still
/// ahead of the run; this one returns to a milestone the agent has just declared BEHIND it — so a
/// reflection that names no successor has to end the run rather than ask for it again. Spelled here
/// and in the document, and the two are held together by `a_reached_milestone_asks_what_is_next`.
const REACHED: &str = "milestone";

/// **THE LABEL A REFLECTION'S ANSWER OPENS ITS FIRST LINE WITH**, authored in the document beside
/// the prompt that asks for it — see [`OuterLoop::proposed`].
///
/// ⚠ Read from the datamodel at the moment the answer is parsed, for [`OuterLoop::authored`]'s
/// reason: an author may edit it, and a driver holding a construction-time copy would look for a
/// label nobody was asked for.
const MILESTONE_MARKER: &str = "milestone_marker";
/// See [`MILESTONE_MARKER`].
const REFERENCE_MARKER: &str = "reference_marker";

/// **THE PARTS A REFLECTION HANDS BACK**, named once because three readers agree through them: the
/// brief's read-back, `reflecting`'s own read, and the document's `reflect.applied` assignments.
///
/// ⚠ `north_star` is deliberately absent from this list and from that transition — a run that may
/// rewrite its own destination cannot be said to have reached it.
const MILESTONE: &str = "milestone";
/// See [`MILESTONE`].
const REFERENCE: &str = "reference";

/// The datamodel variable holding **the standing instructions this run has already carried out** —
/// one labelled line each, accumulated by `screen.matched` and composed into both working prompts by
/// `priming`. See the document, and [`OuterLoop::reflect`].
const STANDING: &str = "standing";

/// **WHAT THIS PARTICULAR LOOP IS FOR** — the template's parts, supplied by whoever starts the run.
///
/// # ⚠⚠⚠ Why this type exists, measured
///
/// `ai_loop.scxml` ships `(edit me)` placeholders and says *"a GUI fills these in"*. No GUI did,
/// and neither did anything else: the only prompt any caller could make the loop send was
///
/// ```text
/// North star: (edit me) the outcome this loop exists to reach
/// Milestone: (edit me) the next checkpoint on the way there
/// Reference: (edit me) paths, URLs or repos to consult
/// ```
///
/// — three of the five clauses a live agent reads. It could not be retro-fitted from out here
/// either: the prompts were COMPOSED from these parts at `<datamodel>` init, so writing a part
/// after `initialize()` left the composed prompt stale, and the session id those writes would need
/// is not on this surface at all.
///
/// So the parts travel as the machine's own `brief` event and the document composes from them in
/// `priming` — see [`OuterLoop::brief`].
///
/// # ⚠ What it deliberately does NOT carry
///
/// `model` is authored above the same line and is not here: it belongs to the session-replace
/// lifecycle this driver does not serve yet, and a door built for a consumer that does not exist is
/// the extension point this workspace already recorded as an anti-pattern. Registered as owed, not
/// forgotten.
///
/// ⚠⚠ `screen_rules` USED TO BE IN THAT SENTENCE and is now a field, because the state it belongs
/// to is built. `screen_permissions` is not here because it no longer exists — see the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brief {
    /// Where this loop is ultimately going. Never rewritten by reflection.
    pub north_star: String,
    /// The step being worked on now. Reflection may rewrite this.
    pub milestone: String,
    /// Prior art the agent should read before deciding anything.
    pub reference: String,
    /// How many turns the run may take before the document calls it `exhausted`.
    pub max_turns: i64,
    /// How often the loop stops to improve its own setup.
    pub reflect_every: i64,
    /// **STANDING INSTRUCTIONS FOR DIALOGS THIS CALLER HAS ALREADY DECIDED ABOUT** — the authored
    /// `screen_rules`, supplied by somebody who did not edit the file.
    ///
    /// # ⚠⚠ Why this travels with the brief and not on [`AiLoopSpec`]
    ///
    /// [`AiLoopSpec::may_answer`] is the caller's consent and it is a construction argument,
    /// because the barrier holds it and the barrier is built once. These are the loop DOCUMENT's
    /// own data: the author writes them in the file, `screening` reads them out of the datamodel at
    /// the moment it acts, and a reflection may one day rewrite them. A field on the spec would be
    /// a SECOND place the same rules live, and the failure of letting two copies drift is silent.
    ///
    /// ⚠ [`None`] means *keep what the document says*, and it is not the same as an empty list —
    /// which is why the field is an `Option` and [`ScreenRules`] cannot be empty. A caller who says
    /// nothing about screening gets the author's rules; one who supplies rules replaces them.
    pub screen_rules: Option<ScreenRules>,
}

/// **HOW A LOOP DRIVES THE PANE IT RUNS IN** — the three declared contracts a turn-owning plugin
/// takes, plus the one fact about the peer that decides how a prompt is delivered.
///
/// # ⚠⚠ Why a struct, where this was five positional arguments
///
/// It reached six the moment the barrier's own patience became the caller's, and two of those six
/// were adjacent `Option`s of different types: `OuterLoop::new(lua, pane, None, None, turn, false)`
/// says nothing at all about which `None` is the barrier and which is how long to wait for it. That
/// is [`NewRun`](../../sprag_host/runs/struct.NewRun.html)'s argument one crate over, and this
/// crate's own for [`AgentSpec`](crate::agent::AgentSpec) and
/// [`OrchestrationSpec`](crate::orchestrator::OrchestrationSpec) before it.
///
/// # ⚠⚠⚠ Why the [`Brief`] is NOT in here
///
/// It is the one thing a caller supplies that is not a construction argument. The parts travel as
/// the machine's own `brief` EVENT so the document composes from them in `priming` — see
/// [`OuterLoop::brief`] — and the machine answers whether it took them. A field on this struct
/// would put the brief where a reader expects it to be applied silently, and its refusal
/// ([`Briefed::NotHeld`]) is precisely the answer that must not be swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiLoopSpec {
    /// What makes the pane ready for the loop's FIRST prompt.
    ///
    /// ⚠ [`None`] means *go ahead immediately*, and against an agent CLI that is almost always
    /// wrong — R379 measured a loop typing its first prompt into a pane whose agent had existed for
    /// ten milliseconds, with the pseudoterminal's own echo confirming the delivery. The honest
    /// value for an agent is [`ReadyWhen::Settles`] naming it.
    pub ready_when: Option<ReadyWhen>,
    /// How long the barrier waits for that, or [`None`] for the substrate's default.
    pub ready_within: Option<Duration>,
    /// What makes ONE of the inner agent's turns over, and how long one may take.
    ///
    /// [`INNER_SESSION_ENDS`] is the contract this loop makes load-bearing.
    pub turn: Turn,
    /// Whether the inner agent paints the prompt box it is typed into — see
    /// [`OuterLoop::shows_the_prompt`](OuterLoop#structfield.shows_the_prompt).
    pub shows_the_prompt: bool,
    /// **WHAT THIS RUN MAY ANSWER IF ITS AGENT STOPS TO ASK**, quoting the agent's own words.
    ///
    /// # ⚠⚠⚠ Why the loop takes this, when the document has a state for the same job
    ///
    /// It did not, and the omission was argued rather than measured: *"answering a dialog is
    /// `screening`'s job, and a consent given to the barrier would answer dialogs one level below
    /// the machine that exists to decide about them."* That is a true sentence about a state
    /// **nothing drives**, and what it cost was measured the round this field was added — a loop
    /// whose agent asked one permission question stopped with **zero turns judged**, and no
    /// argument on the whole form could have covered it, where `orchestrator`, `agent` and `pipe`
    /// all take one.
    ///
    /// ⚠⚠ **THE TWO AUTHORITIES ARE DIFFERENT AND BOTH ARE REAL.** `screen_rules` are AUTHORED
    /// into the document and decide by dialog KIND, standing across every run of that loop; a
    /// consent is the CALLER's, decides by quoted text, and belongs to THIS run. The second is
    /// built, measured and shared with three other plugins; the first is blocked on two owner
    /// decisions. A question no consent covers still reaches the machine's own `turn.blocked`, so
    /// this does not close the door `screening` will come in by — it stops the run dying on the
    /// step in front of it.
    ///
    /// ⚠ [`None`] is *answer nothing*, which is what every loop did before this field and is still
    /// the right default: a run that types into a menu nobody authorised is the failure class this
    /// whole contract exists inside.
    pub may_answer: Option<crate::consent::Consents>,
    /// **WHETHER A PERSON IS WATCHING THE PANE, AND FOR HOW LONG** — the other half of
    /// [`may_answer`](Self::may_answer), and the loop document's own `awaiting_human` supplied by
    /// the substrate.
    ///
    /// ⚠⚠ `ai_loop.scxml` has always asked for exactly this: `awaiting_human` is a WAITING state
    /// with four exits and only one ends the run. R371 measured a supervisor who answered a dialog
    /// forty milliseconds after the run had already reported `blocked`; R373 measured a person who
    /// typed ONE key into a pane a loop was driving and left **thirty-seven of forty iterations
    /// unspent**. Both are this argument's absence.
    ///
    /// ⚠ [`Attended::NoOne`](crate::readiness::Attended::NoOne) is the default and means the run
    /// ends rather than waiting, which is the only honest answer for a pane on a screen nobody is
    /// looking at.
    pub attended: crate::readiness::Attended,
    /// **WHO ANSWERS THE DOCUMENT'S `judged_rules`**, or [`None`] for a run that asked for nobody.
    ///
    /// # ⚠⚠⚠ Two halves, and neither implies the other
    ///
    /// The AUTHOR writes the rules into the document — what makes a dialog theirs to turn down.
    /// The CALLER supplies this — which agent answers them, at what price. Rules with nobody to
    /// ask change nothing; a judge with no rules is asked nothing. **Both are needed and each is
    /// somebody else's to give**, which is why they are not one argument.
    ///
    /// ⚠ [`None`] is the default and costs exactly nothing: no pane spawned, no model asked, every
    /// blocked turn to `screening` as before. A run that did not ask for a second agent must not
    /// acquire one, so this crate names no model of its own.
    pub judge: Option<crate::judge::JudgeSpec>,
}

impl AiLoopSpec {
    /// The spec for driving a real agent CLI called `agent`.
    ///
    /// ⚠ Both of the knobs it fixes are true of every agent CLI and of nothing else: one settles
    /// rather than exiting, so the barrier and the turn contract are both
    /// [`Settles`](ReadyWhen::Settles); and one renders each character into its prompt box as it
    /// arrives, so a delivery can be confirmed on screen before the Enter that submits it.
    #[must_use]
    pub fn driving(agent: &str) -> Self {
        Self {
            ready_when: Some(ReadyWhen::Settles(agent.to_owned())),
            ready_within: None,
            // ⚠ NO PER-TURN BOUND, the honest default for an agent: how long one of its turns may
            // take is the caller's to say, and the run's own clock and cancel bound it meanwhile.
            // `lasting` refuses only a ZERO bound, so this is never `None`.
            turn: Turn::lasting(INNER_SESSION_ENDS, None)
                .expect("a turn with no bound is never the zero one `lasting` refuses"),
            shows_the_prompt: true,
            // ⚠ NEITHER IS DERIVABLE FROM THE AGENT'S NAME. What a run may answer on somebody's
            // behalf, and whether anybody is at the pane, are the caller's alone — a default here
            // would be this constructor deciding something nobody said.
            may_answer: None,
            attended: crate::readiness::Attended::NoOne,
            // ⚠ NOR IS THIS. A judge is a second agent with a bill; naming one here would have
            // every loop built from this constructor quietly acquire one.
            judge: None,
        }
    }
}

/// **WHAT THE MACHINE DID WITH A [`Brief`]** — see [`OuterLoop::brief`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub enum Briefed {
    /// The machine holds every part, read back out of its own datamodel.
    Took,
    /// **A BRIEF ONLY REACHES A LOOP THAT HAS NOT STARTED.** The document's `brief` transition is
    /// on `idle` alone: a run already driving an agent adopts new parts through `reflecting`,
    /// which replaces the session, because changing what a run is for underneath a working agent
    /// is not an assignment. Carries where the machine actually was.
    TooLate(AiLoopState),
    /// **THE EVENT WAS TAKEN AND THE DATAMODEL DOES NOT HOLD WHAT WAS SENT.**
    ///
    /// ⚠ Read back rather than assumed, because everything that could go wrong between here and
    /// the datamodel is silent: the event is raised into a queue, the assignment is evaluated by a
    /// script engine this crate does not own, and a failed `<assign>` raises `error.execution` at
    /// the machine rather than an error out here. A driver that reported success on having SENT
    /// the brief would report success on a loop about to prompt an agent with `(edit me)`.
    ///
    /// ⚠⚠⚠ IT IS NOT HYPOTHETICAL, AND THE READ-BACK IS THE ONLY THING THAT CATCHES IT. At the
    /// SCE rev before this one, a brief holding any non-ASCII character came back mangled — the
    /// engine's JSON path decoded UTF-8 as Latin-1. **Nothing else in the product could have
    /// noticed**: the event was accepted, the assignment succeeded, no error was raised anywhere,
    /// and the loop would have prompted a live agent with the mojibake and reported success. It
    /// was found by this arm, filed upstream as PR-87 and fixed there.
    ///
    /// ⚠⚠ THE MACHINE IS SENT TO `failed` when this is answered. A brief the engine could not
    /// carry has already been assigned — the mangled text is in the datamodel — so a caller that
    /// ignored the answer would start a run about something nobody wrote. `fail` is the
    /// document's own word for a run that cannot go on, and using it means the refusal cannot be
    /// walked past by pumping.
    NotHeld {
        /// The datamodel variable that did not come back.
        part: &'static str,
        /// What it held instead, when it held anything a reader could name.
        held: Option<String>,
    },
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
    /// The `reflect_prompt` — **what should this run do next?**, asked of the agent that has been
    /// doing the work, and answered into the session that replaces it.
    Reflect,
}

impl Owed {
    /// The datamodel variable this prompt is read out of.
    ///
    /// ⚠ ONE LIST DECIDES BOTH READS. [`Authored::read`] validates a machine through these names
    /// and [`OuterLoop::advance`] delivers through them, so a rename in the document breaks both
    /// at once instead of leaving a driver that validates one variable and sends another.
    ///
    /// # Panics
    ///
    /// Never: [`Self::Nothing`] is filtered by the caller's match before this is reached, and the
    /// alternative — an `Option` every call site unwraps — would put the same impossibility one
    /// layer further from where it is decided.
    const fn variable(self) -> &'static str {
        match self {
            Self::Start => "start_prompt",
            Self::Turn => "turn_prompt",
            Self::End => "end_prompt",
            Self::Reflect => "reflect_prompt",
            Self::Nothing => panic!("`Owed::Nothing` names no prompt; the caller matches it first"),
        }
    }

    /// What the document says goes with arriving at `landed` by raising `raised`.
    ///
    /// # ⚠⚠ The two halves of `ai_loop.scxml`'s sends, and why only one needs the event
    ///
    /// `prompt.start`, `prompt.end` and `prompt.reflect` are **onentry** sends — `priming`'s,
    /// `closing`'s and `reflecting`'s — so arriving at those states is the whole condition,
    /// whichever transition brought you. All three are reached more than one way (`priming` from
    /// `idle` and from `restarting`; `reflecting` from the correctness edge and from the budget
    /// one), and keying them on the event would have needed that list kept in step by hand.
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
            // ⚠⚠⚠ A REFLECTION IS A TURN, so arriving here OWES the agent a question — see
            // [`OuterLoop::reflect`]. Before it did, this state sat in the silent list below and a
            // reflection could only ever carry what the document's author had written.
            AiLoopState::Reflecting => Self::Reflect,
            AiLoopState::Working => match raised {
                // `judging --judge-->`, `awaiting_human --resume-->` and
                // `reflecting --reflect.none-->` each carry `<send event="prompt.turn"/>`.
                AiLoopEvent::Judge | AiLoopEvent::Resume | AiLoopEvent::ReflectNone => Self::Turn,
                // ⚠⚠ `priming --prompt.sent-->` carries none because the START prompt is already
                // in the pane, and `screening --screen.matched-->` carries none DELIBERATELY: the
                // peer has just been handed its answer by the driver's own keystroke and is
                // working on it. A prompt on either edge types over a peer mid-turn, which is the
                // failure class this crate keeps paying for.
                // ⚠⚠ `screening --screen.moot-->` carries none for a sharper version of
                // `ScreenMatched`'s reason: NOTHING was pressed and the turn was never
                // interrupted, so the peer is still working on the prompt it already has and this
                // loop's `Completion` is still armed from it. A prompt here would be a second
                // question inside one turn.
                // ⚠⚠ `redirecting --redirect.done-->` carries none for exactly `ScreenMatched`'s
                // reason, and it is the same act: the peer has just been refused and told what to
                // do instead by this driver's own keystrokes, so it is working on that. A prompt
                // here would type over a peer that has just been spoken to.
                AiLoopEvent::RedirectDone
                | AiLoopEvent::RedirectBegin
                | AiLoopEvent::RedirectNone
                | AiLoopEvent::ScreenMoot
                | AiLoopEvent::PromptSent
                | AiLoopEvent::ScreenMatched
                | AiLoopEvent::Brief
                | AiLoopEvent::Cancel
                | AiLoopEvent::ErrorExecution
                | AiLoopEvent::Fail
                | AiLoopEvent::Hold
                | AiLoopEvent::NotifyHuman
                | AiLoopEvent::PromptEnd
                | AiLoopEvent::PromptStart
                | AiLoopEvent::PromptTurn
                | AiLoopEvent::ReflectApplied
                // ⚠ `reflecting --reflect.done-->` reaches `closing`, never here: it is the
                // agent saying there is nothing left, and what `closing` owes is the END prompt,
                // which this table answers by the STATE.
                | AiLoopEvent::ReflectDone
                | AiLoopEvent::PromptReflect
                | AiLoopEvent::ScreenBegin
                | AiLoopEvent::ScreenNone
                | AiLoopEvent::SessionReady
                | AiLoopEvent::SessionReplace
                | AiLoopEvent::SessionReplaced
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
            | AiLoopState::Redirecting
            | AiLoopState::AwaitingHuman
            | AiLoopState::Restarting
            | AiLoopState::Resuming
            | AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::Blocked => Self::Nothing,
        }
    }
}

/// **WHAT ONE PUMP DECIDED TO RAISE, AND THE DATA THE EVENT CARRIES.**
///
/// # ⚠⚠ Why the payload is built where the FACT is read, rather than in [`OuterLoop::advance`]
///
/// Three of this machine's events carry `_event.data`, and each of the facts behind them is read
/// somewhere different: `judge`'s `done` off the PANE, `screen.matched`'s `said` off the RULE that
/// fired, and `reflect.applied`'s parts out of the DATAMODEL. `advance` used to special-case the
/// first by name and call `said_done` itself, which is the shape that cannot take a second
/// data-carrying event without a second special case — and a driver that raised one of the other two
/// through the convenience call would assign `nil` over an author's data and report success.
///
/// So a pump answers what happened AND what to say about it, and `advance` only delivers.
struct Raise {
    /// The event.
    event: AiLoopEvent,
    /// Its `_event.data`, already serialised, for the events whose guards or assignments read one.
    ///
    /// ⚠ [`None`] and `Some("{}")` are the same thing to the machine and deliberately not the same
    /// thing here: `process_event` is the call that sends no data at all, and using it wherever
    /// nothing is owed keeps every pre-existing event on exactly the path it was already on.
    data: Option<String>,
}

impl From<AiLoopEvent> for Raise {
    /// An event with nothing to say — every arm of the pump but three.
    fn from(event: AiLoopEvent) -> Self {
        Self { event, data: None }
    }
}

impl Raise {
    /// `event`, carrying `data` as its `_event.data`.
    ///
    /// ⚠ Built by the JSON writer and never by `format!`, for [`OuterLoop::brief`]'s measured reason:
    /// every value that crosses here is a person's prose — a rule's redirect, a milestone — so it
    /// holds quotes, newlines and non-ASCII, and a hand-spliced object ends early on the first one.
    fn carrying(event: AiLoopEvent, data: serde_json::Value) -> Self {
        Self {
            event,
            data: Some(data.to_string()),
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
        /// ⚠⚠ AND IT IS CONSUMED NOW. It was carried by the round that could first produce it,
        /// with nothing reading it and the reason registered as debt — *"the outer loop has no
        /// `Guardrails` equivalent"*. [`AiLoop`](crate::ai_loop::AiLoop) reports it as a step's
        /// [`Cost::Bytes`](crate::plugin::Cost::Bytes), so the same `max_cost` ceiling that bounds
        /// every other byte-relay run bounds a loop, and a run's published spend is real.
        spent: u64,
    },
    /// **THE MACHINE IS IN A STATE THIS DRIVER CANNOT SERVE YET.**
    ///
    /// Not a failure and not a stall: the state compiles, the machine is correctly in it, and the
    /// effect it names has no implementation here. Returned rather than ignored so a caller learns
    /// WHICH state its run reached, because a run that silently spun in one would report the same
    /// thing as a run that never got there.
    ///
    /// ⚠⚠⚠ THERE IS ONE LEFT, and its scope cut is declared rather than implied: `awaiting_human`
    /// waits for a person, and this driver has neither producer (`hold`, `unattended`) for the two
    /// events that would end that wait. `screening` was built at R384 and
    /// `reflecting`/`restarting`/`resuming` at R385, so the *"registered debt with named
    /// prerequisites"* this doc used to list is down to that one.
    ///
    /// ⚠⚠ THE CALLER THAT ACTS ON IT is `AiLoop::unbuilt`, and what it does is worth reading beside
    /// this: the answer is not *"unimplemented"* to whoever reads the run — a peer that stopped to
    /// ask and a person at the pane are the same two facts every other plugin reports, so they get
    /// the same two words.
    Unbuilt(AiLoopState),
    /// **THE PANE IS NOT THE LOOP'S TO TYPE INTO YET**, and why — see
    /// `OuterLoop::start_ready`.
    ///
    /// Only ever answered from `idle`, before a byte has been sent: a pane that is showing somebody
    /// a question the loop did not provoke, or that a person is typing in. Both are states a pump
    /// later may resolve, which is why this is not an error — a pane that never came up at all is,
    /// and it is the barrier's own [`PaneError::NeverReady`].
    ///
    /// ⚠ Advisory, exactly as [`Unbuilt`](Self::Unbuilt) is: a caller that ignores it pumps again.
    /// What it must not do is what this driver did before R379 measured it, which is type the first
    /// prompt into a pane whose agent had not started.
    NotReady(Reached),
    /// The loop reached one of the document's five final states.
    Ended(AiLoopState),
}

/// **WHAT A PUMP SAW THAT THE MACHINE'S STATE CANNOT CARRY** — the fact behind the last event
/// this driver raised.
///
/// # ⚠⚠⚠ Why the driver remembers it instead of a consumer re-reading the pane
///
/// Three of the machine's transitions are caused by something a consumer has to be able to
/// REPORT, and the event that carries them is a bare word: `turn.blocked` says a peer stopped to
/// ask and not WHAT it is asking, `turn.interrupted` says a person is here and not how much they
/// typed. The driver had both in its hand — [`Reached::Asking`] carries the whole
/// [`Unanswered`], [`Reached::Interrupted`] the
/// [`Interruption`](crate::readiness::Interruption) — and dropped them on the floor.
///
/// A consumer could ask the pane again. It must not: the screen moves, so a second read is a
/// second authority on one fact, and this workspace has paid for that shape before (R367 moved a
/// question to ONE parse for exactly this reason). What is carried here is what the driver
/// actually decided on, at the instant it decided.
///
/// ⚠ It is CLEARED when a prompt goes in, because a new turn is a new
/// question: a notice left over from the previous turn would be published as this one's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Noticed {
    /// **THE PEER STOPPED TO ASK** — the question, and why this run answered nothing.
    ///
    /// ⚠⚠ **THE REFUSAL IS THE BARRIER'S OWN**, whichever door the question arrived by — see
    /// [`OuterLoop::barrier_says`](OuterLoop), which both of them go through. This doc used to say
    /// the reason was always `no_consent` *"because answering a dialog is `screening`'s job in the
    /// document and not the barrier's"*, and that sentence outlived its own truth twice: R382 gave
    /// the loop a caller's consents, and R384 built `screening`.
    ///
    /// ⚠ So a run's report can now name any of the ten reasons, and the last authority to look at
    /// the dialog is the one whose word heads it — `screening`'s, when a run got that far, with
    /// what the consents said kept underneath in free text
    /// ([`Unanswered::unscreened`](crate::consent::Unanswered::unscreened)).
    Asking(crate::consent::Unanswered),
    /// **A PERSON TOOK THE PANE**, and how much they wrote into it.
    Interrupted(crate::readiness::Interruption),
    /// **A JUDGED RULE CLAIMED A DIALOG AND THIS RUN TURNED IT DOWN** — beside
    /// [`Screened`](Self::Screened), and separate for the reason that one is separate from a
    /// hand-answered run: two authorities can act here, and a reader of a finished run must be
    /// able to tell which did. A quote can be re-read in the document forever; a judgement
    /// happened once, to one dialog, and this is its only trace.
    Redirected(crate::judge::Redirected),
    /// **THE DATAMODEL STOPPED ANSWERING** for this variable, so the machine was sent to `failed`.
    ///
    /// Names the variable rather than the fact alone: the run's failure sentence is the only thing
    /// its caller gets, and *"a prompt could not be read"* does not say which of four.
    Undrivable(&'static str),
    /// **THE PEER ASKED AND THIS RUN ANSWERED IT**, on a consent the caller declared before the
    /// run started — see [`AiLoopSpec::may_answer`].
    ///
    /// ⚠⚠ NOT TERMINAL, and one a consumer must report exactly ONCE. The three above it are read
    /// at the end of a run; this is a decision taken on somebody's behalf DURING one, and a run
    /// whose journal spells that `continue` is a run in which approvals are indexed by nothing.
    /// [`OuterLoop::took_answer`] is how a reporter consumes it.
    Answered(crate::consent::Answered),
    /// **THE PEER ASKED, A STANDING INSTRUCTION REFUSED THE CALL AND TOLD IT WHAT TO DO INSTEAD** —
    /// see [`crate::screen`].
    ///
    /// ⚠⚠ THE SECOND NON-TERMINAL NOTICE, and it is a separate arm from [`Answered`](Self::Answered)
    /// for the reason [`Verdict::Screened`](crate::plugin::Verdict::Screened) is a separate word:
    /// they are OPPOSITE decisions. One takes an option the peer offered; this one turns the peer's
    /// call down. [`OuterLoop::took_screening`] consumes it.
    Screened(Screened),
}

/// **WHY THE AUTHOR'S STANDING INSTRUCTIONS COULD NOT BE READ** — a loop that cannot be screened,
/// and which part of the document says so.
///
/// ⚠ Answered at the DOOR ([`AiLoop::new`](crate::ai_loop::AiLoop::new)) rather than when a dialog
/// arrives, this crate's house rule: a caller's — or an author's — mistake is a synchronous refusal
/// naming what to change, not a run that prompts a live agent and then meets its own document being
/// unreadable half an hour later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotScreenable {
    /// A rule the author wrote is not one this build can carry out — WHICH one, and why.
    ///
    /// ⚠ The index is carried because the reasons are about a rule's own fields and a document may
    /// hold several: *"a screen rule with an empty `when`"* does not say which line to go and look
    /// at.
    Malformed {
        /// Its position in the authored list, counting from zero.
        at: usize,
        /// What is wrong with it.
        why: Malformed,
    },
    /// `screen_rules` is not a list of objects carrying `when` and `text` at all — a datamodel this
    /// driver cannot read as standing instructions, which is the same class of answer
    /// `Authored::read`'s [`None`] gives about the prompts.
    Unreadable,
}

/// **THE THINGS THAT ARE TRUE OF ONE PANE**, held together so a replacement cannot carry one of
/// them over.
///
/// # ⚠⚠⚠ Why this is a struct and not three fields with a careful comment
///
/// `restarting` closes the inner session and opens a fresh one, and every value in here is a
/// statement about the pane that has just gone:
///
/// * the BARRIER's `seen` LATCHES, so carried over it reports *already ready* about an agent that has
///   existed for ten milliseconds — R379's measured defect, whose whole cost was that a prompt went
///   into a booting program;
/// * its `hands_at` is a watermark of how often a PERSON had written into the old pane, so the fresh
///   pane's own startup writes read as an interruption;
/// * the `judged` trail's rows are the old pane's.
///
/// ⚠⚠⚠ **AND A MUTATION MEASURED THAT NO STAND-IN CATCHES IT.** Deleting the barrier's re-arm from
/// the replacement left every gate in this crate GREEN, for exactly the reason R379 recorded: a `sh`
/// peer's pseudoterminal BUFFERS a prompt typed before the program has started, so the program reads
/// it late and the run converges either way. A real agent CLI does not — it is already painting its
/// own screen — which is why the first draft of the outer driver *did* type into a booting `claude`
/// and sat in `working` for as long as anyone let it.
///
/// So the invariant is not defended by a comment or by a flag. [`Session::replacing`] returns a WHOLE
/// session, so the compiler asks for every field, and a replacement that forgot one would not build.
struct Session {
    /// The inner session's pane.
    pane: PaneId,
    /// The barrier the pane must clear before anything is typed into it.
    ready: Readiness,
    /// **WHAT THE PANE HELD BEFORE THIS TURN'S PROMPT WENT IN** —
    /// [`said_done`](OuterLoop::said_done)'s arming, and [`Completion::begin`]'s discipline applied to
    /// TEXT rather than to a supervisor's verdict.
    ///
    /// Marked at the same moment the contract is, for the same reason: a marker that was on the
    /// screen before this turn started is not this turn's answer.
    judged: crate::access::RowTrail,
    /// **WHERE THIS TURN'S OUTPUT BEGINS**, as an ADDRESS into the pane's logical lines — what the
    /// closing report is read from, and per-pane for the same reason everything else here is: a
    /// replacement pane numbers its lines from one again, so an address carried over would name
    /// somewhere in the middle of a session that has been closed.
    ///
    /// ⚠⚠⚠ NOT the trail above, and the difference was MEASURED against a live agent rather than
    /// argued: a sixty-line reply on a forty-row pane came back through the rendering opening at
    /// `LINE-29`, and through this opening at `LINE-1`. See [`crate::report`].
    since: crate::report::Since,
    /// **THE NAME THIS SESSION FILES ITS OWN RECORD UNDER**, latched the first time it can be read.
    ///
    /// # ⚠⚠ Why it is latched rather than read when wanted
    ///
    /// It is recovered from the pane's FOREGROUND JOB, which is live state: the leader is whatever
    /// owns the terminal now, so an agent that runs a pager or an editor in the foreground would be
    /// asked about the wrong process and answer nothing. Latching means the run keeps the name it
    /// learned while its agent was the thing in front, and a momentary child cannot take the
    /// session's identity away from it.
    ///
    /// ⚠ `None` is not an error and never ends a run. It means *this build could not name that
    /// session* — no `PaneForegroundJob`, a pane whose agent a person launched with its own name,
    /// an agent sprag does not instrument — and the only thing lost is knowing what it spends.
    identity: Option<String>,
}

impl Session {
    /// The same CONTRACTS over `fresh`, with everything that was true of the pane it replaces
    /// forgotten — see the type.
    ///
    /// What is kept is what the CALLER declared: the readiness condition, the patience, the consents
    /// and who is expected. A replacement session is the same run under the same terms.
    fn replacing(&self, fresh: PaneId) -> Self {
        Self {
            pane: fresh,
            ready: self.ready.rearmed(),
            judged: crate::access::RowTrail::default(),
            // ⚠ A LINE ADDRESS IS A FACT ABOUT ONE PANE. The replacement numbers its own lines from
            // the beginning, so the predecessor's cursor would point into the middle of it.
            since: crate::report::Since::default(),
            // ⚠⚠⚠ A REPLACEMENT IS A DIFFERENT SESSION AND MUST BE NAMED AFRESH. Carrying the old
            // name over would point every later reading at the record of a session that has been
            // closed — the spend would freeze at whatever the predecessor last spent and look like
            // an agent doing nothing. Measured upstream of this: a launch handed a name already in
            // use is refused outright, so the two really are distinct sessions and not one resumed.
            identity: None,
        }
    }

    /// The name this session's agent files its record under, learning it if it can — see
    /// [`Session::identity`].
    fn identify(&mut self, panes: &dyn PaneAccess) -> Option<&str> {
        if self.identity.is_none() {
            self.identity = panes
                .foreground_job()
                .and_then(|jobs| jobs.pane_foreground_leader(self.pane))
                .and_then(|leader| {
                    crate::spend::identity_in(&leader.argv, crate::spend::CLAUDE_IDENTITY_FLAG)
                });
        }
        self.identity.as_deref()
    }
}

/// A run of `ai_loop.scxml`'s machine against one pane.
pub struct OuterLoop {
    /// The compiled document.
    machine: Engine<AiLoopPolicy>,
    /// The engine its `<data>` lives in, and the session id it files them under.
    script: Arc<dyn IScriptEngine>,
    session: String,
    /// **THE INNER SESSION THIS LOOP IS DRIVING** — its pane, its barrier and its baseline, which are
    /// one value because `restarting` replaces all three at once. See [`Session`].
    driving: Session,
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
    /// **WHO ANSWERS THIS RUN'S `judged_rules`**, or [`None`] for a run that asked for nobody.
    judge: Option<crate::judge::JudgeSpec>,
    /// **THE RULE A JUDGE JUST CLAIMED THIS DIALOG WITH**, held between raising `turn.blocked` and
    /// carrying the refusal out in `redirecting`.
    ///
    /// ⚠ It is the DRIVER's and not the document's on purpose. The document routes on a boolean;
    /// what to SAY is the rule's own `text`, and putting it through the datamodel and back would
    /// make the words a run types into somebody's dialog a round trip through a script engine that
    /// has already been measured mangling non-ASCII once.
    claimed: Option<crate::judge::JudgedRule>,
    /// **WHEN THIS RUN STARTED WAITING FOR A PERSON**, or [`None`] when it is not waiting.
    ///
    /// ⚠⚠ [`attend`](Self::attend) is the only reader AND the only writer, which is what keeps it
    /// honest: it is set on the first look at a wait and cleared on every exit from one, so no other
    /// state of the machine can leave a stale anchor behind. The alternative — a duration threaded
    /// through the pump — would put the phase in the driver, which is exactly what `restarting` and
    /// `resuming` are two states to avoid.
    awaiting: Option<Instant>,
    /// This turn's evaluator, armed before the prompt goes in.
    done: Completion,
    /// What the last pump saw behind the event it raised — see [`Noticed`].
    ///
    /// ⚠ NOT part of [`Session`], deliberately: this is per-TURN and the three values in there are
    /// per-PANE. A replacement clears it for a different reason — a question the old session asked is
    /// not the new one's — and so does every prompt.
    noticed: Option<Noticed>,
    /// **WHAT THE AGENT WROTE WHEN IT WAS ASKED TO ACCOUNT FOR THE RUN** — `closing`'s turn, read
    /// off the pane and published as [`Plugin::captured`](crate::plugin::Plugin::captured).
    ///
    /// # ⚠⚠⚠ Why this belongs to the RUN and not to the session that wrote it
    ///
    /// It is the only thing a run produces that a person could not have got by watching. The
    /// machine's walk is in the journal, the turn count is the document's counter, the ending is
    /// the outcome — and none of them says *what happened*. The agent's own account does, and
    /// **before this it reached nobody**: a run that changed a dozen files answered its caller with
    /// the single word `converged`.
    ///
    /// ⚠⚠⚠ **AND A RUN NOW OUTLIVES THE SESSION THAT WROTE ANY GIVEN PART OF IT.** Since
    /// `restarting`, the pane the closing report lands on is not the pane the run started with, and
    /// every earlier session has been CLOSED — so scrolling back is not available to anybody, and
    /// this field is the only place the run's own account can survive its sessions. That is why it
    /// is here rather than in [`Session`]: the sessions are what it has to outlive.
    ///
    /// ⚠ [`None`] until `closing`'s turn ends, and [`None`] FOR EVER on a run that never closed —
    /// exhausted, cancelled, failed or blocked. Those endings ask for no report, so publishing one
    /// would mean publishing a WORK turn's output as the run's account, which is a different claim
    /// than the one the agent was asked to make.
    reported: Option<String>,
}

impl OuterLoop {
    /// Drive the machine `script` evaluates against `pane`, on the contracts `spec` declares.
    ///
    /// [`None`] when the machine's datamodel does not carry the four authored strings — see
    /// `Authored::read`.
    ///
    /// ⚠ The engine is the CALLER's — see the module doc. The daemon takes
    /// [`AiLoop`](crate::ai_loop::AiLoop)'s door and constructs one there, which is the decision
    /// that made `sce-rust-lua` a real dependency of the host.
    #[must_use]
    pub fn new(script: Arc<dyn IScriptEngine>, pane: PaneId, spec: &AiLoopSpec) -> Option<Self> {
        let mut machine = Engine::new(AiLoopPolicy::new(Arc::clone(&script)));
        machine.initialize();
        let session = machine.policy().session_id.clone()?;
        // ⚠ VALIDATION, NOT A SNAPSHOT — the answer is dropped. A machine that does not carry the
        // four strings is one this driver cannot drive and refusing here is what stops a run being
        // started against it; keeping the values would be the staleness this round removed.
        Authored::read(&script, &session)?;
        Some(Self {
            done: Completion::new(spec.turn.when()),
            noticed: None,
            // ⚠⚠⚠ THE CALLER'S CONSENTS AND THEIR ATTENDANT REACH THE BARRIER, and that reverses a
            // decision this constructor used to argue: *"answering a dialog is `screening`'s job,
            // and a consent given to the barrier would answer dialogs one level below the machine
            // that exists to decide about them."* True of a state NOTHING DRIVES, and the cost was
            // measured — a loop whose agent asked one permission question stopped with zero turns
            // judged. See [`AiLoopSpec::may_answer`], which holds the whole argument: two different
            // authorities, one of them built, and a question no consent covers still reaches the
            // machine's own `turn.blocked`.
            //
            // ⚠ THE BARRIER'S OWN PATIENCE IS THE CALLER'S, because a loop's first prompt waits for
            // an agent CLI to come up and that is tens of seconds on a cold start — where the
            // default was chosen for a shell. Absent still means the default; what changed is that
            // a caller who knows their peer is slow can say so, instead of the run reporting
            // `NeverReady` about a program that was still loading.
            driving: Session {
                pane,
                ready: Readiness::new(
                    spec.ready_when.clone(),
                    spec.ready_within,
                    spec.may_answer.clone(),
                    spec.attended,
                ),
                judged: crate::access::RowTrail::default(),
                since: crate::report::Since::default(),
                // Learned on the first look at a pane whose agent is up, not here: at construction
                // the child may not have `exec`d yet, and a `None` cached now would be indistinguishable
                // from one that will never be answerable.
                identity: None,
            },
            machine,
            script,
            session,
            turn: spec.turn.clone(),
            shows_the_prompt: spec.shows_the_prompt,
            judge: spec.judge.clone(),
            claimed: None,
            awaiting: None,
            reported: None,
        })
    }

    /// **TELL THE MACHINE WHAT THIS RUN IS FOR** — the template filled in by a caller that did not
    /// edit the file.
    ///
    /// The parts travel as the document's own `brief` event, so the composition stays where the
    /// author wrote it: `priming`'s `onentry` builds the prompts out of whatever the parts hold at
    /// the moment a session is about to be spoken to. That is what makes this reach a prompt at
    /// all — writing the parts directly, if this surface even exposed the session id, would leave
    /// the composed prompts exactly as stale as they were before.
    ///
    /// ⚠⚠ THE ANSWER IS READ BACK OUT OF THE DATAMODEL rather than inferred from having sent the
    /// event — see [`Briefed::NotHeld`].
    pub fn brief(&mut self, brief: &Brief) -> Briefed {
        let at = self.state();
        if at != AiLoopState::Idle {
            return Briefed::TooLate(at);
        }
        // ⚠ Built by the JSON writer rather than by `format!`. A north star is a person's prose:
        // it holds quotes, newlines and non-ASCII, and a hand-spliced payload would either lose
        // the brief or end the object early — the second of which reaches the datamodel as a
        // PARTIAL brief and prompts an agent with half a sentence.
        // ⚠⚠⚠ THE RULES ARE ALWAYS SENT, AND A CALLER WHO SUPPLIED NONE GETS THE DOCUMENT'S OWN
        // ECHOED BACK. The transition's `<assign>` is unconditional — SCXML executable content
        // has no *"only if the caller said so"* without a construct this document has never been
        // measured with — so omitting the key would assign nil and DELETE the author's rules for
        // every caller that did not happen to care about screening.
        //
        // ⚠ The echo is not free and that is the point: what comes back is read back below like
        // every other part, so the round trip through a JSON payload and a Lua table is PROVEN on
        // every run rather than assumed on the ones that use it. PR-87 was a round in which
        // exactly that crossing was silently lossy.
        // ⚠⚠⚠ AND AN UNREADABLE AUTHORED LIST IS REFUSED RATHER THAN ECHOED AS NOTHING. The first
        // draft wrote `self.screening().unwrap_or_default()`, which turned *"this document's rules
        // are a shape I cannot read"* into `null` — assigned over the author's own data, read back
        // as agreeing, and started. `AiLoop::new`'s door check runs AFTER the brief, so it would
        // then find the wiped list perfectly readable: **the one refusal that exists for the
        // document's own rules was unreachable, by the code that was supposed to carry them.**
        let rules = match (&brief.screen_rules, self.screening()) {
            (Some(supplied), _) => Some(supplied.clone()),
            (None, Ok(authored)) => authored,
            (None, Err(why)) => {
                // The document is what it is; nothing here can mend it, and `fail` is what stops a
                // caller pumping past the answer — the same treatment a part that did not come
                // back gets, for the same reason.
                self.machine.process_event(AiLoopEvent::Fail);
                return Briefed::NotHeld {
                    part: ScreenRules::WIRE_KEY,
                    held: Some(format!("{why:?}")),
                };
            }
        };
        let payload = serde_json::json!({
            "north_star": brief.north_star,
            "milestone": brief.milestone,
            "reference": brief.reference,
            "max_turns": brief.max_turns,
            "reflect_every": brief.reflect_every,
            ScreenRules::WIRE_KEY: rules.as_ref().map(|rules| {
                rules
                    .rules()
                    .iter()
                    .map(|rule| {
                        serde_json::json!({
                            ScreenRule::WHEN_KEY: rule.when(),
                            ScreenRule::TEXT_KEY: rule.text(),
                        })
                    })
                    .collect::<Vec<_>>()
            }),
        });
        self.machine
            .raise_external(AiLoopEvent::Brief, &payload.to_string(), "");
        self.machine.step();

        let held = self.held_as_briefed(brief, rules.as_ref());
        if held != Briefed::Took {
            // The mangled or missing part is already in the datamodel; there is no un-assigning it
            // from out here. `fail` is what the document says happens to a run that cannot go on,
            // and it is what stops a caller pumping past this answer.
            self.machine.process_event(AiLoopEvent::Fail);
        }
        held
    }

    /// Whether every part of `brief` came back out of the datamodel unchanged — with `rules` the
    /// standing instructions actually sent, which are the caller's or the document's own echo.
    fn held_as_briefed(&self, brief: &Brief, rules: Option<&ScreenRules>) -> Briefed {
        for (part, sent) in [
            ("north_star", &brief.north_star),
            (MILESTONE, &brief.milestone),
            (REFERENCE, &brief.reference),
        ] {
            match self.script.get_variable(&self.session, part) {
                Ok(ScriptValue::String(held)) if &held == sent => {}
                Ok(ScriptValue::String(held)) => {
                    return Briefed::NotHeld {
                        part,
                        held: Some(held),
                    };
                }
                _ => return Briefed::NotHeld { part, held: None },
            }
        }
        for (part, sent) in [
            ("max_turns", brief.max_turns),
            ("reflect_every", brief.reflect_every),
        ] {
            match self.script.get_variable(&self.session, part) {
                Ok(ScriptValue::Int(held)) if held == sent => {}
                Ok(ScriptValue::Int(held)) => {
                    return Briefed::NotHeld {
                        part,
                        held: Some(held.to_string()),
                    };
                }
                _ => return Briefed::NotHeld { part, held: None },
            }
        }
        // ⚠⚠⚠ AND THE STANDING INSTRUCTIONS, READ BACK THROUGH THE PRODUCT'S OWN READER — the one
        // `screening` will use when a dialog arrives, not a second walk of the same table. A brief
        // that reported success on rules `screening` cannot read is a run that stops on the first
        // dialog with an answer sitting in its datamodel.
        //
        // ⚠ This is the only part whose crossing is a LIST OF OBJECTS, and it is the part most
        // likely to carry a person's own language — both routes PR-87 was about, on one value.
        match (self.screening(), rules) {
            (Ok(held), wanted) if held.as_ref() == wanted => {}
            (Ok(held), _) => {
                return Briefed::NotHeld {
                    part: ScreenRules::WIRE_KEY,
                    held: Some(format!("{held:?}")),
                };
            }
            (Err(why), _) => {
                return Briefed::NotHeld {
                    part: ScreenRules::WIRE_KEY,
                    held: Some(format!("{why:?}")),
                };
            }
        }
        Briefed::Took
    }

    /// One of the document's own strings, as the datamodel holds it NOW.
    fn text_of(&self, name: &str) -> Option<String> {
        match self.script.get_variable(&self.session, name) {
            Ok(ScriptValue::String(value)) => Some(value),
            _ => None,
        }
    }

    /// Where the machine is.
    #[must_use]
    pub fn state(&self) -> AiLoopState {
        self.machine.get_current_state()
    }

    /// **WHAT THE LAST PUMP SAW BEHIND THE EVENT IT RAISED** — see [`Noticed`].
    ///
    /// [`None`] while the loop is driving a turn nothing has interrupted, which is every pump of a
    /// run that is going well.
    #[must_use]
    pub const fn noticed(&self) -> Option<&Noticed> {
        self.noticed.as_ref()
    }

    /// **TAKE THE APPROVAL THIS RUN JUST GAVE**, leaving every other notice where it is.
    ///
    /// # ⚠⚠ Why only this arm, and why taking rather than reading
    ///
    /// [`Noticed::Answered`] is the one notice that is not terminal: it is a decision taken on
    /// somebody's behalf DURING a run, and a consumer publishes it. A reporter that merely LOOKED
    /// would publish the same approval on every pump until the next prompt cleared it, and a tally
    /// built on that would count one decision as many.
    ///
    /// The other three arms say why a run is ENDING and are read through
    /// [`noticed`](Self::noticed) at that ending — so a `take` that emptied the field would
    /// destroy the question a `blocked` outcome is supposed to publish. **This one leaves them
    /// alone**, which is what makes the two uses safe to sit on one field.
    pub fn took_answer(&mut self) -> Option<crate::consent::Answered> {
        match self.noticed {
            Some(Noticed::Answered(_)) => match self.noticed.take() {
                Some(Noticed::Answered(answered)) => Some(answered),
                // Unreachable: the arm was just matched. Answering `None` rather than panicking
                // keeps a driver bug from taking a live agent's pane down with it.
                _ => None,
            },
            _ => None,
        }
    }

    /// **TAKE THE REFUSAL THIS RUN JUST GAVE ON THE AUTHOR'S BEHALF**, leaving every other notice
    /// where it is — [`took_answer`](Self::took_answer)'s contract for the other decision.
    pub fn took_screening(&mut self) -> Option<Screened> {
        match self.noticed {
            Some(Noticed::Screened(_)) => match self.noticed.take() {
                Some(Noticed::Screened(screened)) => Some(screened),
                // Unreachable: the arm was just matched. Answering `None` rather than panicking
                // keeps a driver bug from taking a live agent's pane down with it.
                _ => None,
            },
            _ => None,
        }
    }

    /// **THE AUTHOR'S STANDING INSTRUCTIONS, AS THE DATAMODEL HOLDS THEM NOW** — or [`None`] for a
    /// loop that screens nothing.
    ///
    /// # ⚠⚠ Read live, for [`authored`](Self::authored)'s reason
    ///
    /// A snapshot taken in [`new`](Self::new) cannot see a [`brief`](Self::brief), so a caller who
    /// supplied their own rules would have them assigned into the datamodel and then screened
    /// against the document's. That is the exact staleness the composed prompts were carrying
    /// before R380, met one field over.
    ///
    /// # ⚠⚠⚠ Why the LIST is read through the script session at all
    ///
    /// PR-86's third ask — SCE emits no read accessor for a lowered scalar `<data>` — is why every
    /// string in this driver goes through the interpreter. Measured for this one: a COMPOSITE
    /// `<data>` is not lowered into a Rust field at all, so the interpreter is not a workaround
    /// here but the only representation there is, and the missing accessor bounds what the POLICY
    /// can answer rather than what a driver can see.
    ///
    /// # Errors
    ///
    /// [`NotScreenable`], naming the rule or the shape.
    pub fn screening(&self) -> Result<Option<ScreenRules>, NotScreenable> {
        let Ok(held) = self
            .script
            .get_variable(&self.session, ScreenRules::WIRE_KEY)
        else {
            return Err(NotScreenable::Unreadable);
        };
        let items = match held {
            ScriptValue::Array(items) => items,
            // ⚠ A datamodel that answers NOTHING for this variable is a loop that screens nothing,
            // which is a legitimate thing to be — an author may delete the list. Anything else
            // (a string, a number, a bare object) is a document whose author meant something this
            // driver cannot read, and guessing at it is how a rule fires on a dialog nobody wrote
            // it for.
            ScriptValue::Null | ScriptValue::Undefined => return Ok(None),
            _ => return Err(NotScreenable::Unreadable),
        };
        let mut rules = Vec::with_capacity(items.len());
        for (at, item) in items.iter().enumerate() {
            let ScriptValue::Object(fields) = item else {
                return Err(NotScreenable::Unreadable);
            };
            let text_of = |key: &str| match fields.get(key) {
                Some(ScriptValue::String(held)) => Some(held.clone()),
                _ => None,
            };
            let (Some(when), Some(text)) =
                (text_of(ScreenRule::WHEN_KEY), text_of(ScreenRule::TEXT_KEY))
            else {
                return Err(NotScreenable::Unreadable);
            };
            rules.push(
                ScreenRule::parse(when, text)
                    .map_err(|why| NotScreenable::Malformed { at, why })?,
            );
        }
        Ok(ScreenRules::of(rules))
    }

    /// **THE JUDGED DECISIONS THE DOCUMENT CARRIES**, read live for
    /// [`authored`](Self::authored)'s reason.
    ///
    /// ⚠⚠ AN ABSENT OR EMPTY LIST IS AN EMPTY LIST, not a refusal — declining to judge is the
    /// ordinary state of a run and its default. Anything that is neither (a string, a number, a
    /// bare object) is a document whose author meant something this driver cannot read, and
    /// guessing at that is how a decision fires on a dialog nobody wrote it for. That is
    /// [`NotScreenable::Unreadable`], the same answer `screening` gives.
    fn judged_rules(&self) -> Result<crate::judge::JudgedRules, NotScreenable> {
        use crate::judge::{JudgedRule, JudgedRules};

        let Ok(held) = self.script.get_variable(&self.session, JudgedRules::KEY) else {
            return Err(NotScreenable::Unreadable);
        };
        let items = match held {
            ScriptValue::Array(items) => items,
            ScriptValue::Null | ScriptValue::Undefined => return Ok(JudgedRules::default()),
            _ => return Err(NotScreenable::Unreadable),
        };
        let mut rules = Vec::with_capacity(items.len());
        for (at, item) in items.iter().enumerate() {
            let ScriptValue::Object(fields) = item else {
                return Err(NotScreenable::Unreadable);
            };
            let text_of = |key: &str| match fields.get(key) {
                Some(ScriptValue::String(held)) => Some(held.clone()),
                _ => None,
            };
            let (Some(name), Some(criterion), Some(text)) = (
                text_of(JudgedRule::NAME_KEY),
                text_of(JudgedRule::JUDGE_KEY),
                text_of(JudgedRule::TEXT_KEY),
            ) else {
                return Err(NotScreenable::Unreadable);
            };
            rules.push(
                JudgedRule::parse(name, criterion, text)
                    .map_err(|why| NotScreenable::Malformed { at, why })?,
            );
        }
        Ok(JudgedRules::of(rules))
    }

    /// What the document says NOW.
    ///
    /// # ⚠⚠⚠ Read live, because a snapshot is the defect this replaced
    ///
    /// This used to be a field, filled once in [`new`](Self::new). That is the same mistake the
    /// document made by composing in `<data>`: a value taken at construction cannot see a
    /// [`brief`](Self::brief), and it cannot see `priming` compose the prompts either — so a
    /// driver holding one would deliver the empty string a machine in `idle` carries and report
    /// having sent a prompt.
    ///
    /// ⚠ A LOOP IN `idle` HOLDS THREE EMPTY PROMPTS, and that is the honest answer rather than a
    /// gap: nothing has been primed, so nothing has been composed. What a caller can read before
    /// then is the parts it supplied.
    ///
    /// [`None`] when the datamodel no longer answers with the four strings — a machine that has
    /// stopped being drivable mid-run, which [`pump`](Self::pump) turns into the document's own
    /// `fail`.
    #[must_use]
    pub fn authored(&self) -> Option<Authored> {
        Authored::read(&self.script, &self.session)
    }

    /// How many turns the machine counts having taken — its own number, not one kept out here.
    ///
    /// ⚠ Read from the interpreter for `Authored::read`'s reason: SCE emits no accessor for a
    /// lowered scalar `<data>`. A caller that kept its own tally would be a second authority on
    /// the one fact the document's budget guards compare against.
    #[must_use]
    pub fn turns(&self) -> Option<i64> {
        match self.script.get_variable(&self.session, "turns") {
            Ok(ScriptValue::Int(turns)) => Some(turns),
            _ => None,
        }
    }

    /// **WHAT THE DOCUMENT HOLDS AS ITS SESSION'S ACCUMULATED CONTEXT** — its own `context`,
    /// assigned on entry to `judging` from the number this driver put on `turn.done`.
    ///
    /// # ⚠⚠⚠ Read from the MACHINE, not from the driver that supplied it
    ///
    /// The same rule as [`turns`](Self::turns) and for a sharper reason: this driver computed the
    /// number, so answering from a field out here would let a reporter agree with the driver about
    /// a value the DOCUMENT never took. That is exactly the shape R381 measured — a value published
    /// as optional and read as required — and the only reading worth publishing is the one a guard
    /// would see.
    ///
    /// ⚠ `Some(0)` means *the session could not be named, or has written nothing yet*, and is not a
    /// claim that nothing has accumulated. See `context_now`.
    #[must_use]
    pub fn context(&self) -> Option<i64> {
        match self.script.get_variable(&self.session, "context") {
            Ok(ScriptValue::Int(context)) => Some(context),
            _ => None,
        }
    }

    /// **HOW MANY CALLS THE DOCUMENT COUNTS A STANDING INSTRUCTION HAVING TURNED DOWN** — its own
    /// `screened`, incremented on `screen.matched`.
    ///
    /// # ⚠⚠⚠ Why this exists when the run already has a tally
    ///
    /// It did not, and the omission is the shape this workspace keeps paying for. The document
    /// counts screenings and [`Outcome::screened`](crate::driver::Outcome::screened) counts
    /// `Verdict::Screened` steps — **two authorities over one fact**, and the machine's was read by
    /// nothing, which is the definition of a number that is a comment (R355's rule, and debt 49's
    /// shape exactly).
    ///
    /// ⚠ The two are not merged, because they are counted for different reasons and by different
    /// parties: the DOCUMENT's is what a future `judging` guard could compare against, the way
    /// `max_turns` compares against `turns`; the RUN's is what a person auditing the run reads. What
    /// makes two safe is that a gate asserts they AGREE — see
    /// `a_loop_carries_out_the_standing_instruction_its_author_wrote`.
    #[must_use]
    pub fn screened(&self) -> Option<i64> {
        match self.script.get_variable(&self.session, "screened") {
            Ok(ScriptValue::Int(screened)) => Some(screened),
            _ => None,
        }
    }

    /// **ONE PASS**: perform what the machine's current state asks for, then tell it what happened.
    ///
    /// The whole driver is this function and the two tables it consults — `Owed` for what a
    /// transition says, and the match below for what a state asks.
    pub fn pump(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Pumped, PaneError> {
        let from = self.state();
        if self.machine.is_in_final_state() {
            return Ok(Pumped::Ended(from));
        }
        let raised: Raise = match from {
            // Nothing has happened yet. Starting the loop is the caller's act — but the transition
            // it causes DELIVERS THE START PROMPT, so the pane has to be ready first.
            //
            // ⚠⚠⚠ IT WAS NOT, AND R379 MEASURED WHAT THAT COSTS AGAINST A LIVE AGENT. This driver
            // built a `Readiness` in `new` and consulted it only in `watch`, which runs in
            // `working` — i.e. AFTER the start prompt had already gone in. The loop therefore typed
            // its first prompt into a pane whose agent had existed for ten milliseconds: the
            // pseudoterminal's own line discipline echoed the text, `deliver` read that echo back
            // and called the delivery confirmed, Enter went to a program that was still booting,
            // and the run then sat in `working` for as long as anyone let it. **Measured: the whole
            // walk was `Idle -> Priming -> Working` in 0.01s and then `Working --Null--> Working`
            // forever.**
            //
            // It was invisible to every stand-in because the fixtures wait for a readiness marker
            // themselves before the run begins — `testing::started`, whose own doc says it takes
            // startup out of the run's budget. The harness was clearing the barrier the product
            // was not.
            //
            // ⚠ `Readiness` LATCHES, so this costs one look per pump after the first.
            AiLoopState::Idle => match self.start_ready(panes, run)? {
                None => AiLoopEvent::Start.into(),
                Some(seen) => return Ok(Pumped::NotReady(seen)),
            },

            // A session exists and has not been prompted. The prompt itself was already delivered
            // by whichever transition brought us here — see `advance`.
            AiLoopState::Priming => AiLoopEvent::PromptSent.into(),

            // ⚠⚠⚠ THE STATE THE WHOLE ROUND WAS ABOUT. The inner agent is working and the driver
            // watches its pane; what the turn ENDS ON is what the machine is told.
            // ⚠⚠ A TURN THAT ENDED CARRIES WHAT THE SESSION HAS BEEN CHARGED TO READ, and only that
            // one ending does: `turn.blocked` and `turn.interrupted` are answers about a peer that
            // is still mid-turn, so a number attached to them would be a level nobody had reached.
            // The other endings keep going through `into()`, which sends no data at all.
            AiLoopState::Working | AiLoopState::Closing => match self.watch(panes, run)? {
                AiLoopEvent::TurnDone => {
                    // ⚠⚠⚠ THE ONLY MOMENT THE CLOSING REPORT EXISTS TO BE TAKEN. The next event
                    // lands in `converged`, which is final — the Driver stops stepping, and by the
                    // time anybody could ask, the run is over. Taken here, on the state rather than
                    // on the event, because `turn.done` is what four states raise and only this
                    // one's turn was a request for an account.
                    if from == AiLoopState::Closing {
                        self.reported = self.account(panes);
                    }
                    Raise::carrying(
                        AiLoopEvent::TurnDone,
                        serde_json::json!({ "context": self.context_now(panes) }),
                    )
                }
                // ⚠⚠⚠ THE VERDICT IS TAKEN HERE, ONCE, and the document decides on it — see
                // `working`'s `cond="_event.data.judged"`. A guard cannot do this: the pinned
                // engine has no seam to register a host function on, and SCXML does not promise
                // how often it evaluates one, so a judgement inside a `cond` would be a model
                // call of unknown multiplicity inside a microstep.
                AiLoopEvent::TurnBlocked => {
                    let rule = self.judged(panes, run);
                    Raise::carrying(
                        AiLoopEvent::TurnBlocked,
                        // ⚠ A BOOLEAN BESIDE THE NAME, because this datamodel is Lua and the only
                        // false values there are `nil` and `false`. A guard reading the name alone
                        // would fire on the empty string, i.e. on every blocked turn of every run
                        // that matched nothing.
                        serde_json::json!({
                            "judged": rule.is_some(),
                            "rule": rule.unwrap_or_default(),
                        }),
                    )
                }
                other => other.into(),
            },

            // One turn has landed. The document decides in priority order, and what it needs from
            // out here is whether the agent said it was done — `judge`'s first guard reads
            // `_event.data.done`.
            AiLoopState::Judging => Raise::carrying(
                AiLoopEvent::Judge,
                serde_json::json!({"done": self.said_done(panes)}),
            ),

            // ⚠⚠⚠ THE AUTHOR'S STANDING INSTRUCTIONS, CARRIED OUT. A dialog no consent covered is
            // here; whether one of the document's own rules claims it — and what happens if one
            // does — is [`screen`](Self::screen).
            AiLoopState::Screening => self.screen(panes, run)?,

            // ⚠⚠⚠ THE LOOP IMPROVES ITS OWN SETUP AND THEN REPLACES THE SESSION THAT READS IT —
            // three states, because the three things that happen are genuinely different acts and
            // the document says which by where it is.
            // ⚠⚠⚠ A REFLECTION IS A TURN AND THIS WATCHES IT, which is why this arm takes the pane:
            // the question was delivered by the transition that landed here (`Owed::Reflect`), and
            // what the agent answers is what the replacement session is briefed with.
            AiLoopState::Reflecting => self.reflect(panes, run)?,
            AiLoopState::Restarting => self.replace(panes)?,
            AiLoopState::Resuming => self.resume(panes, run)?,

            // ⚠⚠⚠ THE RUN IS PAUSED AND A PERSON IS EXPECTED. It WAITS — see [`attend`](Self::attend).
            //
            // ⚠⚠⚠ IT USED TO END THE RUN HERE, and that was the driver deciding something the
            // document does not say. `awaiting_human` has SEVEN edges and six of them are ways to
            // carry on; the driver answered `Pumped::Unbuilt` and the [`Driver`] stopped. So a loop
            // whose agent asked one question no rule claimed was over — *"a rule that claims nothing
            // ends the run exactly as an unanswered dialog always has"* was written as a scope note
            // and read as a design, and the machine plainly said otherwise the whole time.
            AiLoopState::AwaitingHuman => self.attend(panes, run)?,

            // ⚠⚠⚠ THE DOCUMENT HAS THE ROUTE AND THIS DRIVER HAS NOT BUILT THE ACT YET, reported
            // as such rather than skipped. `working`'s `cond="_event.data.design"` is what reaches
            // here, and nothing yet publishes a `true` for it — the judge that would is the next
            // piece — so today this state is unreachable and says so if it is ever reached.
            //
            // ⚠ `Unbuilt` and not a no-op, for `awaiting_human`'s reason: a driver that treated an
            // undriven state as *carry on* would take the loop somewhere the author did not write,
            // and a route that silently does nothing is worse than one that is missing.
            AiLoopState::Redirecting => self.redirect(panes, run)?,

            // `is_in_final_state` answered above; these are the same five, and naming them keeps
            // the match exhaustive without a wildcard that would swallow a sixth.
            state @ (AiLoopState::Converged
            | AiLoopState::Exhausted
            | AiLoopState::Failed
            | AiLoopState::Cancelled
            | AiLoopState::Blocked) => return Ok(Pumped::Ended(state)),
        };
        // Kept before `advance` takes the payload: what a consumer reports is the EVENT, and the
        // data is the driver's way of telling the machine a fact it could not read for itself.
        let event = raised.event;
        let (to, spent) = self.advance(panes, run, raised)?;
        Ok(Pumped::Moved {
            from,
            raised: event,
            to,
            spent,
        })
    }

    /// The pane this loop is driving **NOW** — which is not the pane it was started over, once a
    /// reflection has replaced the inner session.
    ///
    /// ⚠⚠⚠ A CONSUMER MUST NOT KEEP ITS OWN COPY. [`Plugin::driving`](crate::plugin::Plugin::driving)
    /// is what a cancelled run signals, and a copy taken at construction names a pane that has been
    /// closed — so the model in the pane that replaced it would go on spending somebody's tokens
    /// while the run reported having stopped it. That is the exact failure `driving` exists to
    /// prevent, reintroduced one field away from it.
    #[must_use]
    pub const fn pane(&self) -> PaneId {
        self.driving.pane
    }

    /// **THE PANE MUST BE READY BEFORE THE FIRST PROMPT** — see the `Idle` arm of [`pump`](Self::pump).
    ///
    /// # ⚠⚠ Why every answer but `Yes` ends the run here, where `watch` turns three of them into
    /// events
    ///
    /// `watch` is asking about a pane the loop is already driving, and the machine has a
    /// transition for each thing that can happen to one: a person took it, it stopped to ask, the
    /// run ended. From `idle` the document has exactly ONE edge, `start`, so there is nowhere for
    /// any of those answers to go — and each of them says the pane is not the loop's to type into:
    ///
    /// * a pane already ASKING is showing somebody a question the loop did not provoke (a fresh
    ///   agent's *"do you trust this folder?"* is exactly this, measured), and the run has said
    ///   nothing it could be an answer to;
    /// * a pane a PERSON is typing in is theirs;
    /// * and a pane that never came up is the barrier's own [`PaneError::NeverReady`], which says
    ///   which of the four questions was asked and what the pane was doing instead.
    ///
    /// Not starting is the honest answer to both, and it is the direction that does not type into
    /// somebody else's screen. A pane that simply never came up is already the barrier's own
    /// [`PaneError::NeverReady`], which says which of the four questions was asked and what the
    /// pane was doing instead — this function does not re-spell that.
    ///
    /// [`None`] means *go ahead*.
    ///
    /// # Errors
    ///
    /// [`PaneError`] when the pane cannot be read, when the barrier's own bound expires, or when
    /// the run ended underneath.
    fn start_ready(
        &mut self,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Result<Option<Reached>, PaneError> {
        match self.driving.ready.reached(panes, self.driving.pane, run)? {
            Reached::Yes => Ok(None),
            Reached::RunEnded(why) => Err(why),
            other => Ok(Some(other)),
        }
    }

    /// **WHAT THE BARRIER SAYS ABOUT THIS PANE, AS THE MACHINE'S OWN EVENT** — or [`None`] for
    /// *carry on*.
    ///
    /// # ⚠⚠⚠ Why this is a function, and why [`watch`](Self::watch) calls it TWICE
    ///
    /// A question reaches this driver by two different doors: the barrier reads one off the pane at
    /// the top of a pump, and the turn's own [`Completion`] answers [`Over::Asking`] when a turn
    /// ENDS because the peer stopped to ask. They are the same fact arriving at two moments, and
    /// R377 established that the readings genuinely differ — a menu the peer has only just painted
    /// is invisible to the first and plain to the second.
    ///
    /// What the second door used to do was decide for itself: it built the refusal by hand, with
    /// [`Refusal::NoConsent`](crate::consent::Refusal::NoConsent) written in as a literal. That was
    /// true while a loop could hold no consents and became a **false statement about the caller**
    /// the moment one could — a run whose clause had a typo in its needle was told it had given no
    /// consent, which is the one confusion `Refusal`'s vocabulary exists to prevent. Worse, the
    /// answer never got taken at all: the turn ended, the machine went to `screening`, and the
    /// barrier — the one thing holding the caller's clauses — was never asked.
    ///
    /// So both doors lead here, and here asks the barrier. **One authority for what a run may
    /// answer**, which is R367's rule about one parse of one question, applied to the decision
    /// rather than to the reading.
    fn barrier_says(
        &mut self,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Result<Option<AiLoopEvent>, PaneError> {
        Ok(
            match self.driving.ready.reached(panes, self.driving.pane, run)? {
                Reached::Yes => None,
                // A PERSON TOOK THE PANE. R372's product half, reaching the machine at last.
                Reached::Interrupted(who) => {
                    self.noticed = Some(Noticed::Interrupted(who));
                    Some(AiLoopEvent::TurnInterrupted)
                }
                // The run ended underneath — cancelled, or out of time.
                Reached::RunEnded(_) => Some(AiLoopEvent::Cancel),
                // The peer is asking and NOTHING this run holds answers it — the barrier's own reason
                // rides along, so a caller learns whether to write a clause or fix the one they wrote.
                Reached::Asking(unanswered) => {
                    self.noticed = Some(Noticed::Asking(unanswered));
                    Some(AiLoopEvent::TurnBlocked)
                }
                // ⚠⚠⚠ THE RUN ANSWERED ITS PEER'S QUESTION, on a consent the caller declared. NOT an
                // event: the machine's `turn.blocked` is for a question NOBODY answered, and raising it
                // here would send a loop to `screening` about a dialog that is already gone. The turn
                // simply carries on — which is the whole point of the contract — and the fact is
                // RECORDED because a decision taken on somebody's behalf has to be reportable in the
                // run's own vocabulary rather than left in prose.
                Reached::Answered(answered) => {
                    self.noticed = Some(Noticed::Answered(answered));
                    Some(AiLoopEvent::Null)
                }
                // ⚠ The pane is mid-transition — a person is dealing with a dialog this run may not
                // answer, or has just handed the pane back. Both are states the next pump asks about
                // again, so the honest event is none at all.
                Reached::Attended(_) | Reached::HandedBack(_) => Some(AiLoopEvent::Null),
            },
        )
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
        if let Some(event) = self.barrier_says(panes, run)? {
            return Ok(event);
        }
        // ⚠⚠⚠ AND THIS IS THE WHOLE POINT OF R377. Before `Over` existed the two endings a real
        // agent's turn actually has — *it answered* and *it stopped to ask* — were one answer out
        // here, so `turn.done` and `turn.blocked` had no producer and this function could not be
        // written.
        Ok(
            match self
                .done
                .wait(panes, self.driving.pane, self.patience(), run)
            {
                Over::Yes => AiLoopEvent::TurnDone,
                // ⚠⚠⚠ THE SECOND DOOR, ANSWERED BY THE FIRST — see [`barrier_says`](Self::barrier_says).
                // The question this ending carries is DROPPED rather than re-published, which is
                // deliberate: the barrier reads the pane again and its reading is the one the
                // answer (or the refusal) was decided from, so publishing this one would put a
                // second parse of one question into the run's report.
                //
                // ⚠ `None` from the barrier means the menu is gone — the peer moved on between the
                // two reads. Nothing happened that the machine has a word for, and the next pump
                // asks again.
                Over::Asking(_) => self.barrier_says(panes, run)?.unwrap_or(AiLoopEvent::Null),
                // The peer is still working after the turn's own bound. NOT an event: the machine has
                // no *the turn overran* transition, and inventing one out here would put a decision in
                // the driver that belongs in the document. The run's own clock bounds it, and the next
                // pump asks again.
                Over::NotYet => AiLoopEvent::Null,
                Over::RunEnded => AiLoopEvent::Cancel,
            },
        )
    }

    /// **WAIT FOR THE PERSON** — `awaiting_human`'s whole effect, and the last state of
    /// `ai_loop.scxml` this driver had not built.
    ///
    /// # ⚠⚠⚠ A state machine with no input STAYS IN THE STATE
    ///
    /// That sentence is the whole of this function and it is worth writing down, because what stood
    /// here before was `Pumped::Unbuilt` — the driver ENDING a run the document had merely paused.
    /// The document is unambiguous: `awaiting_human` sends a notification and then holds, and every
    /// way out of it is something that HAPPENS (the person answers and a turn completes; they wave
    /// it on; the run is cancelled; nobody comes). None of those is *time passed and the driver gave
    /// up*.
    ///
    /// So the mapping is:
    ///
    /// * the turn the person unblocked COMPLETED — `turn.done`, carrying what the session has been
    ///   charged to read, exactly as `working`'s does, because `judging` reads it on entry from both
    ///   doors;
    /// * the run was cancelled — `cancel`, which is the caller's act and not this driver's;
    /// * **anything else — including a dialog that is still up, and a person mid-keystroke — is
    ///   [`AiLoopEvent::Null`]**: nothing happened, the machine stays where it is, and the next pump
    ///   asks again.
    ///
    /// ⚠⚠⚠ **THE BARRIER IS NOT ASKED HERE, AND THAT IS THE DOCUMENT'S INSTRUCTION RATHER THAN AN
    /// OPTIMISATION.** [`watch`](Self::watch) asks it first because *"a person reaching into the pane
    /// outranks everything the peer is doing"* — true in `working`, where nobody was expected. Here
    /// somebody IS expected: this state exists because the run asked for them. So the hand that
    /// [`Readiness`] would report as an INTERRUPTION is the very event
    /// this state is waiting for, and consulting it means the answer never lands.
    ///
    /// **Measured**: with the barrier asked, a person pressing the key the dialog was waiting for
    /// moved nothing — `Reached::Interrupted` came back on every poll from then on, so
    /// `Completion` was never even consulted and the run sat in `awaiting_human` for the whole gate.
    /// The document says the opposite twice over: `turn.interrupted` here is a SELF-LOOP (*"stay put;
    /// do not start prompting underneath somebody who is typing"*), and `turn.done` is *"the person
    /// answered (or typed a turn themselves) and it completed."*
    ///
    /// ⚠⚠ **AND THE SELF-LOOP IS NOT PRODUCED, DELIBERATELY.** `Null` leaves the machine in exactly
    /// the same state and reports no transition, where raising the event would write a step into the
    /// run's journal every poll saying a state changed when it did not. The two are
    /// indistinguishable to the machine and only one of them is honest to a reader.
    ///
    /// ⚠⚠⚠ **AND `unattended` IS THE CALLER'S NUMBER, NOT A CLOCK THIS DRIVER CHOSE.** The document
    /// ends a wait with *"nobody came within the driver's patience"*, and how long that is, is
    /// exactly what [`Attended`](crate::readiness::Attended) already declares:
    ///
    /// * [`Attended::NoOne`](crate::readiness::Attended::NoOne) — **nobody is watching**, the default and what every run said
    ///   before this contract existed. Waiting for a person the caller has told us will not come is
    ///   waiting for nothing, so the wait ends at once and the run reports `blocked` WITH the
    ///   question. That is the answer this driver used to give by refusing to build the state at
    ///   all; it is now the document's own edge, and the difference is that the machine says it.
    /// * [`Attended::APerson`](crate::readiness::Attended::APerson) — wait up to their `patience`,
    ///   then `unattended`.
    ///
    /// A driver that invented a duration here would end somebody's run on a number nobody chose;
    /// one that never ended would make *waiting* and *dead* the same thing to every reader.
    ///
    /// ⚠ The anchor is this function's alone — set on the first look at a wait and cleared on every
    /// exit from one — so no other code can leave it stale.
    ///
    /// # Errors
    ///
    /// Whatever watching the pane answers.
    fn attend(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Raise, PaneError> {
        // ⚠⚠⚠ ASKED BEFORE THE WAIT, AND A pty GATE IS WHY. A caller who declared `NoOne` has said
        // there is nobody to wait FOR, so waiting even one turn's patience is not patience — it is
        // the run carrying on underneath somebody who has taken the pane. Measured:
        // `a_person_at_a_real_keyboard_who_is_not_waited_for_keeps_the_pane` put a real keystroke in
        // front of a peer that was one turn from its goal, and with the wait first the run TOOK that
        // turn — *"the pane is the witness: their byte reached the peer, so the goal was ONE turn
        // away, and the run did not take it"* is the claim, and ordering these two lines is the
        // whole of it.
        let Some(patience) = self.driving.ready.attended().patience() else {
            self.awaiting = None;
            return Ok(AiLoopEvent::Unattended.into());
        };
        let since = *self.awaiting.get_or_insert_with(Instant::now);
        let raised = match self
            .done
            .wait(panes, self.driving.pane, self.patience(), run)
        {
            Over::Yes => Raise::carrying(
                AiLoopEvent::TurnDone,
                serde_json::json!({ "context": self.context_now(panes) }),
            ),
            Over::RunEnded => AiLoopEvent::Cancel.into(),
            // ⚠ The question is still on the screen, or the peer has not finished what the person
            // unblocked. Neither is a thing that HAPPENED — so unless the caller's patience has run
            // out, the machine stays exactly where it is.
            Over::Asking(_) | Over::NotYet if since.elapsed() < patience => {
                return Ok(AiLoopEvent::Null.into());
            }
            Over::Asking(_) | Over::NotYet => AiLoopEvent::Unattended.into(),
        };
        self.awaiting = None;
        Ok(raised)
    }

    /// **CARRY OUT A STANDING INSTRUCTION ON THE DIALOG THAT IS UP** — `screening`'s whole effect.
    ///
    /// # ⚠⚠⚠ The act, and why it is in this order
    ///
    /// 1. **What is being screened comes from the NOTICE**, never from a fresh read of the pane.
    ///    The barrier already parsed this question and decided about it; reading again would be a
    ///    second authority on one fact, which R367 moved this crate away from.
    /// 2. **A rule has to claim it.** None does — or the loop holds none — and the run stops with
    ///    [`Refusal::NoRule`](crate::consent::Refusal::NoRule) naming the dialog, so an author
    ///    learns what to quote.
    /// 3. **The call is refused** with the product's own key, and the dialog must be PROVABLY GONE.
    /// 4. **Only then is anything typed**, through [`say`](Self::say) — the same delivery, turn
    ///    contract and cost accounting every prompt this loop sends goes through.
    ///
    /// ⚠⚠⚠ **STEP 4 MAY NOT BE REORDERED IN FRONT OF STEP 3, AND THAT IS MEASURED.** A live probe
    /// pressed `Tab` at a real permission dialog, which leaves it up, then typed into what was left
    /// — `deliver` read the text back off the screen and reported `Confirmed`, and the Enter behind
    /// it **approved the file write the agent had asked about**. A read-back proves the pane painted
    /// what was typed; only *the question is gone* says what an Enter will then MEAN.
    ///
    /// ⚠ The record is written AFTER `say`, because `say` clears the notice — a new turn is a new
    /// question, and the three things armed per turn must not come apart.
    /// **WHICH OF THE AUTHOR'S JUDGED RULES CLAIMS THE DIALOG THE AGENT IS SHOWING**, by its name,
    /// or [`None`] when none does — what `working`'s `cond="_event.data.judged"` decides on.
    ///
    /// # ⚠⚠⚠ Every road to *"do not know"* ends at NONE, and that is the safety property
    ///
    /// No rules authored, no judge supplied, a question nothing could parse, a judge that timed
    /// out or replied with something that is not a verdict — every one of them answers [`None`],
    /// and `None` sends the dialog to `screening` and then to the person. **That is exactly what a
    /// run with no rules at all does, so the mechanism's failure mode is its own absence.**
    ///
    /// A match on silence would press [`REFUSES`](crate::screen::REFUSES) at somebody's dialog on
    /// nobody's decision — the act `screen.rs` removed the `keys` field to keep out of a rule's
    /// reach. It must not arrive by this door either.
    ///
    /// ⚠ The rule that claimed it is kept in [`claimed`](Self#structfield.claimed) for
    /// [`redirect`](Self::redirect) to carry out. The document is told the NAME and nothing else:
    /// what to say is prose, and a round trip through this script engine is what PR-87 measured
    /// mangling non-ASCII.
    fn judged(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Option<String> {
        self.claimed = None;
        let spec = self.judge.clone()?;
        let rules = self.judged_rules().ok()?;
        if rules.rules().is_empty() {
            return None;
        }
        // ⚠ The question comes from the NOTICE `watch` just recorded, never from a fresh read of
        // the pane — `screen`'s rule, for its reason: a second look is a dialog's worth of time
        // later, and what is judged must be what ended the turn.
        let Some(Noticed::Asking(unanswered)) = &self.noticed else {
            return None;
        };
        let question = unanswered.question().cloned()?;
        let (rule, _judged) = rules.claiming(panes, run, &question, &spec)?;
        let name = rule.name().to_owned();
        self.claimed = Some(rule.clone());
        Some(name)
    }

    /// **A DIALOG A JUDGE CLAIMED, REFUSED AND REDIRECTED** — [`screen`](Self::screen)'s act, on the
    /// other authority.
    ///
    /// ⚠⚠ A method of its own and not an arm of `screen`, because a reader of a finished run has to
    /// be able to tell WHICH authority acted. A rule's quote can be re-read in the document forever;
    /// a judgement happened once, to one dialog, and [`Noticed::Redirected`] is its only trace.
    ///
    /// ⚠ `redirect.none` is `screen.none`'s exit and for its measured reason: the refusing key may
    /// not take the dialog off the screen, and a dialog still up reads an Enter as an answer to
    /// itself. Nothing is typed and the person is woken.
    fn redirect(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Raise, PaneError> {
        let question = match &self.noticed {
            Some(Noticed::Asking(unanswered)) => unanswered.question().cloned(),
            _ => None,
        };
        let (Some(question), Some(rule)) = (question, self.claimed.take()) else {
            // Reaching here means the document routed on a verdict whose subject this driver can no
            // longer see. `screen`'s class, and its answer: the person.
            self.noticed = Some(Noticed::Asking(Unanswered::unreadable()));
            return Ok(AiLoopEvent::RedirectNone.into());
        };

        match crate::screen::refuse(panes, self.driving.pane, &question, run)? {
            Refused::StillUp(unanswered) => {
                self.noticed = Some(Noticed::Asking(unanswered));
                Ok(AiLoopEvent::RedirectNone.into())
            }
            // ⚠ The dialog went before anything was pressed, so there is no question left to
            // publish — `ScreenMoot`'s reason. The peer is still working on the prompt it has, which
            // is why `redirect.done` sends no prompt either.
            Refused::AlreadyGone => {
                self.noticed = None;
                Ok(AiLoopEvent::RedirectDone.into())
            }
            Refused::Gone { bytes } => {
                let spent = self.say(panes, run, rule.text())?;
                self.noticed = Some(Noticed::Redirected(crate::judge::Redirected {
                    question,
                    rule: rule.name().to_owned(),
                    criterion: rule.criterion().to_owned(),
                    // ⚠ `judged` reduced the verdict to the bool the document routed on; the word
                    // itself is not carried back here, and inventing one would be a second
                    // authority on what the judge said.
                    said: "YES".to_owned(),
                    told: rule.text().to_owned(),
                    bytes: bytes + spent,
                }));
                Ok(AiLoopEvent::RedirectDone.into())
            }
        }
    }

    fn screen(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Raise, PaneError> {
        // ⚠⚠ THE BARRIER'S OWN REFUSAL IS CARRIED, not just its question: it says what the
        // CONSENTS made of this dialog, and that reason has a different remedy from anything
        // screening can report. See [`Unanswered::unscreened`].
        let unanswered = match self.noticed.as_ref() {
            Some(Noticed::Asking(unanswered)) => unanswered.clone(),
            _ => Unanswered::unreadable(),
        };
        // ⚠ A blocked pane whose question nothing could read reaches here too — `barrier_says`
        // records `Unanswered::unreadable()` for it. No rule can quote what nobody parsed, and the
        // honest answer is the one that arm already carries: the remedy is a person.
        let Some(question) = unanswered.question().cloned() else {
            self.noticed = Some(Noticed::Asking(Unanswered::unreadable()));
            return Ok(AiLoopEvent::ScreenNone.into());
        };
        let rules = match self.screening() {
            Ok(rules) => rules,
            // ⚠ The door refuses an unreadable rule list before a run starts, so arriving here
            // means the datamodel stopped answering MID-RUN — the same class as a prompt that
            // cannot be read at the moment of delivery, and it gets that class's answer.
            Err(_) => {
                self.noticed = Some(Noticed::Undrivable(ScreenRules::WIRE_KEY));
                return Ok(AiLoopEvent::Fail.into());
            }
        };
        let Some(rule) = rules.as_ref().and_then(|rules| rules.claiming(&question)) else {
            // ⚠⚠⚠ RE-HEADED, NOT REPLACED. A caller reading `no_rule` alone would be sent to write
            // a standing instruction about a dialog whose own `Yes` a consent could have taken,
            // which is the commoner case by far.
            self.noticed = Some(Noticed::Asking(Unanswered::unscreened(unanswered)));
            return Ok(AiLoopEvent::ScreenNone.into());
        };
        // Owned before the act, because saying the rule's text borrows `self` mutably and the rule
        // lives in a list read out of the datamodel.
        let (when, said) = (rule.when().to_owned(), rule.text().to_owned());

        match crate::screen::refuse(panes, self.driving.pane, &question, run)? {
            // ⚠⚠ NOTHING WAS TYPED — see the paragraph above, and `Refusal::NotDismissed`.
            Refused::StillUp(unanswered) => {
                self.noticed = Some(Noticed::Asking(unanswered));
                Ok(AiLoopEvent::ScreenNone.into())
            }
            // ⚠⚠⚠ AND NOTHING WAS PRESSED. The notice is CLEARED because there is no longer a
            // question to publish — leaving it would have the next `Blocked` this run reports be
            // about a dialog that is already gone.
            Refused::AlreadyGone => {
                self.noticed = None;
                Ok(AiLoopEvent::ScreenMoot.into())
            }
            Refused::Gone { bytes } => {
                let spent = self.say(panes, run, &said)?;
                // ⚠⚠⚠ THE INSTRUCTION IS HANDED TO THE MACHINE, NOT ONLY TO THE PEER, and that is
                // register item 148's answer. Said once, it reached the pane and nothing else; the
                // document keeps it in `standing` so `priming` can compose it into every later
                // prompt, and the very next judgement reflects because of it.
                //
                // ⚠ THE KEY IS THE RULE'S OWN `text`, not a second name for one value: the document
                // reads `_event.data.text`, the wire calls it `text`, and `ScreenRule::TEXT_KEY` is
                // what both are spelled from.
                let carried = Raise::carrying(
                    AiLoopEvent::ScreenMatched,
                    serde_json::json!({ScreenRule::TEXT_KEY: &said}),
                );
                self.noticed = Some(Noticed::Screened(Screened {
                    question,
                    when,
                    said,
                    // ⚠ BOTH HALVES. The refusing key and the redirect are one act to whoever pays
                    // for it, and a cost ceiling that could not see what a run typed into somebody's
                    // dialog would be a ceiling with a hole in it.
                    bytes: bytes + spent,
                }));
                Ok(carried)
            }
        }
    }

    /// **ADOPT WHAT THIS RUN HAS LEARNED ABOUT ITSELF** — `reflecting`'s effect, and register item
    /// 148's answer.
    ///
    /// # ⚠⚠⚠ What a reflection actually changes, and why the predicate is the PROMPT
    ///
    /// The one thing this build has to carry across a session boundary is the standing instructions
    /// the loop has already carried out. `screening` types one at its peer and the turn goes on, so
    /// it lives exactly as long as that agent's context — measured as ONE delivery against SIX
    /// re-issues of the milestone it overrides, with the live agent reporting the deadlock in words.
    /// The document keeps them in `standing`, and `priming` composes them into both working prompts;
    /// `priming` is reached only through a restart, which is why adopting one is a session
    /// replacement rather than an assignment.
    ///
    /// So the question this asks is not *"has anything been screened?"* but **"does what I am about
    /// to say already carry what I have learned?"** — `turn_prompt.contains(standing)`. That answers
    /// both cases with one predicate and no bookkeeping:
    ///
    /// * nothing screened, so `standing` is empty and every string contains it — `reflect.none`, and
    ///   a reflection due purely on the turn budget costs nothing;
    /// * screened, adopted, and now due again on the budget — the prompt already carries it, so
    ///   again `reflect.none`. A run that reflected on *"is `standing` non-empty?"* would restart its
    ///   session every `reflect_every` turns for ever, having nothing to change.
    ///
    /// ⚠⚠⚠ **AND IT ASKS THE AGENT WHERE THE WORK GOES NEXT** — the half that makes a run outlive
    /// its agent's CONTEXT rather than only its constraints.
    ///
    /// The reflection is a TURN: `reflecting`'s entry owes `reflect_prompt` (see [`Owed::Reflect`]),
    /// this watches for that turn to end exactly as `working` does, and [`proposed`](Self::proposed)
    /// reads the answer back. Whatever the agent named becomes the milestone the REPLACEMENT session
    /// is briefed with.
    ///
    /// Until that existed the two parts the document's `reflect.applied` assigns were read back and
    /// handed over UNCHANGED, on the true argument that *where the work should go next is a judgement
    /// this driver cannot make*. It still cannot — so it asks the only party that can, and carries
    /// the answer without editing it.
    ///
    /// ⚠⚠ **AN AGENT THAT NAMES NOTHING CHANGES NOTHING**: the milestone and the reference are echoed
    /// as they were, and the run carries on toward the checkpoint it already had. That is the safe
    /// direction — the alternative is a run whose goal is rewritten by a reader that guessed — and it
    /// is also `brief`'s measured reason for echoing rather than omitting: the transition's
    /// `<assign>` is unconditional, so a missing key assigns `nil` and DELETES the milestone.
    ///
    /// # Errors
    ///
    /// Whatever watching the pane answers — the reflection turn is a turn on somebody's terminal.
    fn reflect(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Raise, PaneError> {
        // ⚠ EVERY OTHER ENDING IS THE DOCUMENT'S. `turn.blocked` and `turn.interrupted` both reach
        // `awaiting_human` from here — a reflection cannot be screened (no history state to resume
        // it into) and must not be prompted over — and `Null` leaves the machine where it is.
        let ended = self.watch(panes, run)?;
        if ended != AiLoopEvent::TurnDone {
            return Ok(ended.into());
        }
        // ⚠⚠⚠ ASKED FIRST, because it is the only answer that ends the RUN. The reflection puts two
        // questions in one prompt — *what is the next checkpoint* and *is the whole thing finished*
        // — and an agent that says the second has nothing to say to the first. Reading the milestone
        // first would take a run whose agent had just declared itself finished and hand its
        // replacement a checkpoint.
        if self.said_marker(panes, NORTH_STAR_MARKER) {
            return Ok(AiLoopEvent::ReflectDone.into());
        }
        let (Some(standing), Some(next)) =
            (self.text_of(STANDING), self.text_of(Owed::Turn.variable()))
        else {
            // ⚠ The datamodel has stopped answering mid-run — the same class as a prompt that
            // cannot be read at the moment of delivery, and it gets that class's answer.
            self.noticed = Some(Noticed::Undrivable(STANDING));
            return Ok(AiLoopEvent::Fail.into());
        };
        let learned = once_each(&standing);
        let (Some(milestone), Some(reference)) = (self.text_of(MILESTONE), self.text_of(REFERENCE))
        else {
            self.noticed = Some(Noticed::Undrivable(MILESTONE));
            return Ok(AiLoopEvent::Fail.into());
        };
        let decided = self.proposed(panes, MILESTONE_MARKER);
        // ⚠⚠⚠ THE MILESTONE IS BEHIND THE RUN AND THE AGENT NAMED NO SUCCESSOR, so there is nothing
        // left to ask for. Going back to `working` here is a loop asking an agent to reach a
        // checkpoint it has just said it reached — for ever, and that livelock is what this reason
        // exists to prevent (see [`REACHED`]). ⚠ It is the SAFE direction as well as the terminating
        // one: the thing the caller asked for was met, so ending reports the truth, where continuing
        // would report a budget running out on work nobody had left.
        if decided.is_none()
            && self
                .text_of(REFLECT_REASON)
                .is_some_and(|reason| reason == REACHED)
        {
            return Ok(AiLoopEvent::ReflectDone.into());
        }
        // ⚠⚠ NOTHING NEW AND NOTHING LEARNED, so a restart would throw away an agent's whole
        // context having changed nothing — the document's own words, and the predicate is still
        // *"does what I am about to say already carry what I have learned?"*. ⚠ The tidied standing
        // list is assigned on BOTH exits: dropping it here would leave a duplicate behind, and a
        // duplicate is a `standing` the prompts do not carry, which is another restart.
        if decided.is_none() && next.contains(&learned) {
            return Ok(Raise::carrying(
                AiLoopEvent::ReflectNone,
                serde_json::json!({STANDING: learned}),
            ));
        }
        Ok(Raise::carrying(
            AiLoopEvent::ReflectApplied,
            serde_json::json!({
                MILESTONE: decided.unwrap_or(milestone),
                REFERENCE: self.proposed(panes, REFERENCE_MARKER).unwrap_or(reference),
                STANDING: learned,
            }),
        ))
    }

    /// **WHAT THE AGENT NAMED BEHIND `marker`'s LABEL IN THE TURN JUST ENDED**, or [`None`] where it
    /// named nothing a reader may trust.
    ///
    /// # ⚠⚠⚠ Two rules, and each closes a hole the other cannot
    ///
    /// A reflection's answer is a SENTENCE the agent writes, so unlike [`said_done`](Self::said_done)
    /// the label cannot be required to stand alone as the whole row. That weaker shape is paid for
    /// twice:
    ///
    /// * **THE ROW OPENS WITH THE LABEL** — nothing before it but decoration. An agent CLI prints its
    ///   replies behind a bullet and inside a box, so leading `●`, `│`, `>` and whitespace are
    ///   allowed and a letter or a digit is not. What this closes is the label named in the MIDDLE of
    ///   a sentence, which is exactly how the prompt itself names it.
    /// * ⚠⚠⚠ **AND THE ANSWER IS NOT SOMETHING THIS LOOP SAID** — the candidate is rejected when the
    ///   prompt just delivered contains it. What this closes is the ECHO: the prompt that asks for the
    ///   label carries the label, an agent's terminal paints the prompt, and a pane wraps where it
    ///   likes — so a row opening with the label can be the loop reading its own instruction back.
    ///   R379 measured that exact class on `done_marker` and it converged a run that had proved
    ///   nothing.
    ///
    /// ⚠ **THE LAST MATCH WINS**, because the echo is painted BEFORE the reply that answers it, and
    /// because an agent asked for two lines writes a paragraph first: with the echo discounted this
    /// decides between two things the agent itself said, and the later one is its conclusion.
    /// ⚠⚠ **THE RESIDUE, STATED**: an agent that answers first and then SUMMARISES itself is read
    /// the other way round, and nothing separates the two orders from out here. The prompt asks for
    /// *"exactly two lines and nothing else"*, so the tie-break only ever decides a reply that broke
    /// the contract — and between two answers the same agent wrote, which is the mildest version of
    /// this failure.
    ///
    /// ⚠⚠⚠ **BOTH RULES ARE MEASURED, AND NEITHER WAS UNTIL THE HAZARD WAS STAGED.** The peer's
    /// first draft answered with its two real lines alone, and dropping either rule left the gate
    /// green — the prompt's own wrap happened to break the label across two rows at 80 columns.
    /// `standin_agent_reflecting` now paints both
    /// hazards deliberately, and the mutations bite: without the echo discount the run adopts
    /// *"and then the next checkpoint in one line…"* as its own milestone, and with the FIRST match
    /// it adopts *"a checkpoint it thought better of"*.
    ///
    /// ⚠ **IT FAILS SAFE.** An answer this cannot read leaves the milestone as it was — one more turn
    /// toward a checkpoint somebody chose — where the direction it refuses to fail in is rewriting
    /// what a run is FOR out of text nobody meant as an answer.
    fn proposed(&self, panes: &dyn PaneAccess, marker: &str) -> Option<String> {
        let label = self.text_of(marker)?;
        let asked = self.text_of(Owed::Reflect.variable())?;
        if label.trim().is_empty() {
            return None;
        }
        self.driving
            .judged
            .fresh(panes, self.driving.pane)
            .iter()
            .filter_map(|row| opens_with(row, &label))
            .rfind(|said| !said.is_empty() && !asked.contains(said.as_str()))
    }

    /// **CLOSE THE INNER SESSION AND OPEN A FRESH ONE** — `restarting`'s effect.
    ///
    /// The replacement runs the same command in the same directory at the same size, because the
    /// PANE is the authority on what it was running and this asks it — see
    /// [`PaneLifecycle::respawn`](crate::access::PaneLifecycle::respawn).
    ///
    /// ⚠⚠⚠ EVERYTHING KEYED TO THE OLD PANE IS RE-ARMED HERE, and each of the four would be a live
    /// defect on its own:
    ///
    /// * the BARRIER, because `seen` latches — carried over, a fresh agent that has existed for ten
    ///   milliseconds is *already ready* and its first prompt is typed into a booting program,
    ///   which is R379's measured defect reintroduced by a struct field;
    /// * the pane the loop is DRIVING, so a cancelled run interrupts the session that exists rather
    ///   than one that has been closed;
    /// * the `judged` trail, whose rows are the old pane's;
    /// * and the NOTICE, because a question the old session asked is not the new one's.
    ///
    /// ⚠ The turn's [`Completion`] is not re-armed here and must not be: `say` arms it immediately
    /// before every prompt, which is the only moment at which arming is honest.
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when this host cannot replace panes at all, or when the replacement
    /// cannot start — and in that case the run still holds the pane it had, because the old one is
    /// closed only after the new one exists.
    fn replace(&mut self, panes: &dyn PaneAccess) -> Result<Raise, PaneError> {
        let lifecycle = panes.lifecycle().ok_or_else(|| {
            PaneError::Spawn(
                "this host cannot open panes, so a loop cannot replace its inner session"
                    .to_owned(),
            )
        })?;
        // ⚠⚠⚠ ONE ASSIGNMENT, and that is the point — see [`Session`]. Setting the pane and leaving
        // the barrier behind is a defect NO STAND-IN IN THIS CRATE CATCHES (measured, as a mutation
        // that left every gate green), so the shape is what forbids it: `replacing` answers a WHOLE
        // session and the compiler asks for every field of it.
        self.driving = self
            .driving
            .replacing(lifecycle.respawn(self.driving.pane)?);
        // ⚠ NOT part of that value, and cleared here for its own reason: a question the old session
        // asked is not the new one's, so publishing it would report the closed pane's dialog as this
        // one's.
        self.noticed = None;
        Ok(AiLoopEvent::SessionReplaced.into())
    }

    /// **WAIT FOR THE REPLACEMENT SESSION'S AGENT TO COME UP** — `resuming`'s effect, and the same
    /// barrier `idle` clears before the very first prompt.
    ///
    /// ⚠⚠ A DIALOG OR A PERSON AT A FRESH PANE ENDS THE RUN, where the same answers mid-loop are
    /// ordinary transitions. The document has one edge out of here and it is `session.ready`: a
    /// replacement session showing a question nobody answered is a session that did not come up, and
    /// *"a session that will not come back is a failed run, not a stuck one"* is what the author
    /// wrote. The notice carries which of the two it was, so the run's own sentence says so.
    ///
    /// ⚠ The barrier's other answers — it answered a startup dialog on the caller's consent, or a
    /// person is mid-handback — are `Null`, so the machine stays in `resuming` and the next pump asks
    /// again. That is why replacing and waiting are two states: this one is safe to re-enter, and
    /// replacing is not.
    ///
    /// # Errors
    ///
    /// [`PaneError::NeverReady`] when the fresh session's agent never appears within the caller's
    /// patience, naming what the pane was doing instead.
    fn resume(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Raise, PaneError> {
        Ok(
            match self.driving.ready.reached(panes, self.driving.pane, run)? {
                Reached::Yes => AiLoopEvent::SessionReady.into(),
                Reached::RunEnded(_) => AiLoopEvent::Cancel.into(),
                Reached::Asking(unanswered) => {
                    self.noticed = Some(Noticed::Asking(unanswered));
                    AiLoopEvent::Fail.into()
                }
                Reached::Interrupted(who) => {
                    self.noticed = Some(Noticed::Interrupted(who));
                    AiLoopEvent::Fail.into()
                }
                Reached::Answered(answered) => {
                    self.noticed = Some(Noticed::Answered(answered));
                    AiLoopEvent::Null.into()
                }
                Reached::Attended(_) | Reached::HandedBack(_) => AiLoopEvent::Null.into(),
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
        raised: Raise,
    ) -> Result<(AiLoopState, u64), PaneError> {
        let Raise { event, data } = raised;
        // ⚠ `Null` is W3C SCXML 3.13's eventless sentinel and must never be injected: it is what
        // `watch` answers when nothing happened, and the machine stays put.
        if event == AiLoopEvent::Null {
            return Ok((self.state(), 0));
        }
        // ⚠⚠ `process_event` CANNOT SEND DATA — it is `raise_external(event, "", "")` followed by a
        // macrostep — so an event whose guard or assignments read `_event.data` has to go through
        // the raise that takes a payload. Which events those are is decided by whoever read the
        // fact, not here; see [`Raise`].
        match data {
            Some(data) => {
                self.machine.raise_external(event, &data, "");
                self.machine.step();
            }
            None => self.machine.process_event(event),
        }
        let landed = self.state();
        let owed = Owed::on(event, landed);
        if owed == Owed::Nothing {
            return Ok((landed, 0));
        }
        // ⚠⚠⚠ READ AT THE MOMENT OF DELIVERY, which is the whole point of the order this function
        // documents. `priming`'s `onentry` composes the prompts out of the parts, and it has just
        // run — the machine moved above. A driver reading a construction-time copy would send the
        // template's `(edit me)` however carefully the caller had briefed it.
        let Some(text) = self.text_of(owed.variable()) else {
            // ⚠ THE DOCUMENT'S OWN ANSWER, not one invented out here. A machine whose datamodel has
            // stopped holding its prompts cannot be driven, and `fail` -> `failed` is what this
            // document says happens to a run that cannot go on. Inventing a `Pumped` arm for it
            // would put a terminal decision in the driver.
            //
            // ⚠⚠ WHICH VARIABLE IS RECORDED, because the state it lands in cannot say. `failed` is
            // reached from six transitions and a consumer meeting one has no way back to the
            // cause; this is the same argument `Noticed` makes for the other two.
            self.noticed = Some(Noticed::Undrivable(owed.variable()));
            self.machine.process_event(AiLoopEvent::Fail);
            return Ok((self.state(), 0));
        };
        let spent = self.say(panes, run, &text)?;
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
        self.done.begin(panes, self.driving.pane);
        // ⚠ A NEW TURN IS A NEW QUESTION — see [`Noticed`]. Cleared beside the two other things
        // armed per turn rather than anywhere else, so the three cannot come apart.
        self.noticed = None;
        // ⚠⚠ THE SAME MOMENT, AND FOR THE SAME REASON — see `judged` and `said_done`. Marked
        // BEFORE the injection so the pane's echo of this prompt counts as fresh output: it has to
        // be REJECTED on what it says rather than hidden by where the baseline was taken, or the
        // rule would depend on a race between the terminal and this line.
        self.driving.judged = crate::access::RowTrail::mark(panes, self.driving.pane);
        // ⚠⚠ AND THE THIRD MARK IN THE SAME BREATH, for the third time for the same reason. This
        // one addresses the pane's LOGICAL LINES rather than its rendering, so what it bounds is
        // *everything this turn printed* rather than *what is still on the grid* — see
        // [`crate::report`], where a live agent measured the difference at twenty-eight lines.
        // Marked on EVERY prompt and read only after the closing one, because *what did this turn
        // produce* is the same question whichever turn asks it, and a mark taken only in `closing`
        // would be a fourth thing to keep in step with the other three.
        self.driving.since = crate::report::Since::mark(panes, self.driving.pane);
        if !self.shows_the_prompt {
            // The WRITE, not the delivery — see [`shows_the_prompt`](Self::shows_the_prompt). A
            // peer that paints nothing until it is submitted cannot be confirmed before the submit.
            let mut keys = crate::access::KeyStroke::text(text);
            keys.push(crate::access::KeyStroke::named("Enter"));
            return Ok(panes.inject(self.driving.pane, &keys)?.bytes());
        }
        let delivered = deliver(
            panes,
            run,
            self.driving.pane,
            text,
            &Delivery {
                // A prompt longer than the pane is wide arrives in pieces — see the constant.
                confirm: confirmable(text),
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

    /// **WHAT THE INNER SESSION HAS BEEN CHARGED TO READ**, as of its most recent billed request —
    /// the quantity a cost policy is denominated in, and `0` when this run cannot name its session.
    ///
    /// # ⚠⚠⚠ Why a loop needs this and cannot get it from anything it already holds
    ///
    /// `turns` counts turns and [`Cost::Bytes`](crate::Cost) counts what was typed, and measurement
    /// says neither tracks the bill: across forty local agent sessions **cache read is 99.0% of
    /// tokens and 78.1% of cost**, while what a prompt's size resembles is **10.3% of cost** and is
    /// the component that FALLS as a session grows. And a turn is not a unit: one billed request
    /// adds 861 tokens of context at the median and **633,749 at the maximum**, so predicting this
    /// from `turns` is out by 63% at p90. See `claudedocs/INSIGHT-LOOP-SCORING-AND-COST-SIGNALS.md`.
    ///
    /// # ⚠⚠ ZERO IS A DEGRADATION AND NOT A MEASUREMENT, which is why it is safe
    ///
    /// A run that cannot name its session, or whose agent has written nothing yet, reads `0`. Every
    /// consumer must therefore treat `0` as *"do not decide on this"* rather than as *"nothing has
    /// accumulated"* — the two are indistinguishable here on purpose, because the alternative is
    /// refusing to drive an agent over a number that is only ever an optimisation.
    fn context_now(&mut self, panes: &dyn PaneAccess) -> u64 {
        self.driving
            .identify(panes)
            .and_then(crate::spend::spend_of)
            .map_or(0, |spend| spend.context)
    }

    /// Whether **THE AGENT SAID, IN THIS TURN,** what the document calls done — the one fact
    /// `judging` needs from out here.
    ///
    /// # ⚠⚠⚠ Two ways this can be answered by the loop's own words, and both were live
    ///
    /// It used to be `pane_collapsed().contains(marker)`, and R379 made that wrong twice over in
    /// one change. Once the document started ASKING the agent for the marker — which it must, or
    /// nothing ever says it — the marker is in the PROMPT, the agent's terminal paints the prompt,
    /// and a whole-screen `contains` reads the loop's own instruction as the agent's answer. The
    /// driver's own gate said so in its own words: `Judging -> Judge -> Closing` on the FIRST
    /// judge, `turns() == 1` where the peer had been prompted once and had answered nothing.
    ///
    /// So two independent things are required, and each closes a hole the other cannot:
    ///
    /// * **FRESH** — the row changed since this turn's prompt was delivered (`Self::judged`).
    ///   What this closes is a marker left on the screen by an EARLIER turn, or by whoever had the
    ///   pane before this run: `Completion::begin`'s discipline, applied to text. It does NOT
    ///   close the echo, because the echo is fresh output too.
    /// * **STANDING ALONE** — the marker is the whole row, save for decoration. What this closes is
    ///   the echo, and it is not a trick: it is exactly what `done_instruction` ASKS FOR (*"make
    ///   the last line of your reply exactly …"*), so the check enforces the contract the prompt
    ///   states rather than a second, weaker one. It does not close a stale marker, which stands
    ///   alone perfectly well.
    ///
    /// ⚠ The alternative considered first was this crate's existing echo rule —
    /// [`Orchestrator`](crate::orchestrator)'s *"a changed row is the ECHO when what it holds is a
    /// piece of what was just typed"*. It is the right rule for the peers that plugin drives and
    /// the wrong one here: an agent CLI decorates its echo (`❯ ` before it, a box around it), so
    /// the typed text does not `contains` the row it produced, and the discount silently stops
    /// discounting. Reused where it fits; not reused where the fixture would have been the only
    /// thing it worked against.
    ///
    /// ⚠ **IT FAILS SAFE.** A marker the agent decorated past recognising costs one more turn; the
    /// direction this rule refuses to fail in is converging a run that proved nothing, which is
    /// this crate's most expensive failure class and is what it did before.
    ///
    /// ⚠ The marker is read from the datamodel at the moment the question is asked, for
    /// [`authored`](Self::authored)'s reason. A datamodel that cannot answer leaves the loop
    /// judging that the agent did NOT say it, which is the direction this predicate already fails
    /// in: one more turn, never a convergence nobody earned.
    #[must_use]
    pub fn said_done(&self, panes: &dyn PaneAccess) -> bool {
        self.said_marker(panes, DONE_MARKER)
    }

    /// Whether the agent said, IN THIS TURN, the word the datamodel holds under `variable`.
    ///
    /// [`said_done`](Self::said_done)'s whole rule, named once because a second marker arrived and
    /// the two must be read identically: the run's convergence and its continuation would otherwise
    /// rest on two subtly different notions of *the agent said it*. See `said_done` for the two
    /// pieces of evidence (FRESH, and STANDING ALONE) and what each closes.
    fn said_marker(&self, panes: &dyn PaneAccess, variable: &str) -> bool {
        let Some(marker) = self.text_of(variable) else {
            return false;
        };
        self.driving
            .judged
            .fresh(panes, self.driving.pane)
            .iter()
            .any(|row| stands_alone(row, &marker))
    }

    /// **THE ACCOUNT THE AGENT JUST WROTE**, off the pane the closing turn ran on — or [`None`]
    /// where it wrote nothing a person would read as one.
    ///
    /// ⚠ THE PROMPT IS READ BACK OUT OF THE DATAMODEL rather than remembered from the delivery, for
    /// [`authored`](Self::authored)'s reason and one of its own: what has to be discounted is what
    /// the agent was ASKED, and the document is the only authority on that. A datamodel that has
    /// stopped answering costs the echo discount and not the report — the direction that keeps a
    /// line nobody meant rather than dropping one somebody wrote.
    ///
    /// ⚠⚠ **WHAT AN ACCOUNT IS OUT OF WHAT A PANE PRINTED IS [`crate::report`]'s**, not this
    /// driver's — the echo discount, the furniture at the edges, and the sentence a truncated
    /// report has to carry about itself. Kept there because every one of those rules was decided by
    /// a measurement of what a live agent's pane holds, and the measurement, the rules and their
    /// gates read together or not at all.
    fn account(&self, panes: &dyn PaneAccess) -> Option<String> {
        crate::report::account(
            &self.driving.since.taken(panes, self.driving.pane),
            &self.text_of(Owed::End.variable()).unwrap_or_default(),
        )
    }

    /// **WHAT THE AGENT WROTE WHEN IT WAS ASKED TO ACCOUNT FOR THE RUN** — `closing`'s turn, read
    /// off the pane it ran on.
    ///
    /// [`None`] on a run that never reached `closing`: exhausted, cancelled, failed and blocked ask
    /// the agent for nothing, so there is no account to hand back and the last WORK turn's output
    /// is a different claim. It is the run's rather than the session's, because since `restarting`
    /// a run outlives the sessions any part of it was written in — see
    /// [`AiLoop::captured`](crate::plugin::Plugin::captured).
    #[must_use]
    pub fn report(&self) -> Option<&str> {
        self.reported.as_deref()
    }
}

/// **`standing`'S LINES, EACH KEPT ONCE, IN THE ORDER THEY WERE FIRST LEARNED.**
///
/// # ⚠⚠⚠ Why a loop needs this at all
///
/// `screen.matched` APPENDS the rule's text, because an SCXML `<assign>` cannot ask whether the
/// value is already there without a construct this document has never been measured with. So an
/// agent that asks the same thing twice — which is exactly what an agent that has been turned down
/// once tends to do — puts the same line in twice.
///
/// Left alone that is not merely untidy. `reflect` decides whether to REPLACE THE SESSION by asking
/// whether the prompts already carry what has been learned, and a duplicated line is something they
/// do not carry: **the run would close its agent's pane and open a fresh one every time the same
/// question came back.** Normalising here is what makes the predicate stable.
///
/// ⚠ ORDER IS PRESERVED and the first occurrence wins, for the reason `ScreenRules` gives about
/// document order: these are a person's instructions, and a list somebody wrote is one whose order
/// is part of what it says.
fn once_each(standing: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in standing.lines().filter(|line| !line.trim().is_empty()) {
        if !kept.contains(&line) {
            kept.push(line);
        }
    }
    // ⚠ THE TRAILING TERMINATOR IS PART OF THE VALUE, not decoration: `priming` composes this
    // straight into the middle of a prompt, so a block that did not end its own line would run into
    // the clause after it. An EMPTY list must stay empty for the same reason — that is the case
    // where the prompt has no extra line at all.
    if kept.is_empty() {
        return String::new();
    }
    let mut once = kept.join("\n");
    once.push('\n');
    once
}

/// What `row` says after `label`, when the row OPENS with it — see [`OuterLoop::proposed`].
///
/// ⚠ The decoration a row may carry in front of the label is deliberately a SET rather than
/// *"anything not alphanumeric"*, which is [`stands_alone`]'s rule and the wrong one here. That rule
/// would accept a wrapped echo beginning `"NEXT MILESTONE: …` — a quote mark is not alphanumeric —
/// and the whole reason this reader is careful is that the prompt naming the label is on the screen
/// too. What an agent CLI actually puts in front of its own text is a bullet, a box edge or a
/// prompt glyph, and that is the list.
fn opens_with(row: &str, label: &str) -> Option<String> {
    /// What an agent CLI draws in front of a line of its own reply.
    const DECORATION: &[char] = &['●', '⏺', '│', '|', '>', '❯', '*', '-', '•', ' ', '\t'];
    Some(
        row.trim_matches(DECORATION)
            .strip_prefix(label)?
            .trim()
            .to_owned(),
    )
}

/// Whether `row` is `marker` and nothing else a reader would call words.
///
/// Leading decoration is allowed and anything alphanumeric is not: an agent prints its own replies
/// behind a bullet (`● MILESTONE REACHED`), and the row that has to be rejected is the one where
/// the marker sits at the end of a SENTENCE — the loop's own instruction, read back. See
/// [`OuterLoop::said_done`].
fn stands_alone(row: &str, marker: &str) -> bool {
    !marker.is_empty()
        && row
            .trim()
            .strip_suffix(marker)
            .is_some_and(|before| !before.chars().any(char::is_alphanumeric))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::testing::{standin_agent, started, supervised};
    use sce_rust_runtime::helpers::io_processors::IoProcessorDescriptor;
    use sce_rust_runtime::scripting::i_script_engine::{NativeMethod, StateQueryCallback};
    use sce_rust_runtime::{ScriptResult, SetCurrentEventArgs};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// ⚠⚠⚠ **THE CONFIRMATION NEEDLE IS MEASURED IN SCREEN COLUMNS, NOT IN CHARACTERS** — and a
    /// live run against a real `claude` is what measured the difference.
    ///
    /// # ⚠⚠⚠ What this cost, in the exact words the product printed
    ///
    /// A loop was started on a real agent pane with a Korean brief. Its first run died at once:
    ///
    /// ```text
    /// failed: the pane never took the prompt: 3 injections put 2370 bytes on its pseudoterminal
    /// and none of them ever appeared on it, so nothing was submitted and no reply is this run's
    /// ```
    ///
    /// **The prompt was plainly on the screen** — the pane's own capture ended
    /// `❯ reached AND verified, make the last line of your reply exactly: / MILESTONE REACHED`.
    /// The pane was 38 columns wide, and the needle — the prompt's first FORTY CHARACTERS, most of
    /// them Korean — needed about SIXTY-EIGHT. No row of that pane could ever have carried it, so no
    /// delivery to that pane could ever be confirmed, in any language whose glyphs are wide.
    ///
    /// [`CONFIRM_WHOLE_UP_TO`]'s own doc had always said what it meant (*"a prompt longer than the
    /// pane is WIDE"*), and the code counted characters. For ASCII the two are the same number, which
    /// is why every gate in this crate agreed with the defect.
    ///
    /// ⚠ **THE ASSERTION IS THE RATIO, NOT A LENGTH.** A gate demanding "twenty characters" would be
    /// this test agreeing with an arithmetic I did in my head; what the product owes is that the two
    /// needles OCCUPY THE SAME SCREEN, whatever they are made of.
    #[test]
    fn a_confirmation_needle_is_bounded_by_columns_so_a_wide_language_is_not_asked_for_twice_the_pane()
     {
        /// Wide glyphs, and a real sentence rather than one repeated syllable — a needle is a
        /// prefix, and a fixture whose every character is identical cannot tell a prefix from a
        /// coincidence.
        const KOREAN: &str = "부채를 전부 상환한다 비용 무시하고 가장 장기적으로 올바른 방법으로 구현하고 테스트로 증명한다";
        /// The same claim, in a language whose glyphs are one column.
        const ASCII: &str =
            "pay every debt, ignore the cost, build it the way that lasts and prove it with a test";

        let wide =
            confirmable(KOREAN).expect("a prompt longer than the bound is confirmed by part");
        let narrow = confirmable(ASCII).expect("the same");
        let columns = |text: &str| text.chars().map(sprag_vt::char_columns).sum::<usize>();

        assert_eq!(
            columns(&wide),
            columns(&narrow),
            "⚠⚠⚠ THE TWO NEEDLES MUST TAKE THE SAME AMOUNT OF SCREEN. What has to fit on one row of \
             somebody's terminal is COLUMNS, so a needle counted in characters asks a Korean pane \
             for twice the width it asks an English one for — measured live as a loop that could \
             never confirm a delivery on a 38-column pane and reported that the pane had never \
             taken a prompt it was visibly holding. Wide {wide:?} / narrow {narrow:?}",
        );
        assert!(
            columns(&wide) <= CONFIRM_WHOLE_UP_TO,
            "⚠⚠ and neither may exceed the bound: {} columns of {CONFIRM_WHOLE_UP_TO} in {wide:?}",
            columns(&wide),
        );
        assert!(
            wide.chars().count() < narrow.chars().count(),
            "⚠ the control: a wide language must therefore get FEWER characters, or this gate would \
             pass for a needle that counts neither — {} vs {}",
            wide.chars().count(),
            narrow.chars().count(),
        );
        assert!(
            KOREAN.starts_with(wide.as_str()) && ASCII.starts_with(narrow.as_str()),
            "⚠ and each must still be a LEADING run of what was typed, or it is not a read-back of \
             anything",
        );
        assert_eq!(
            confirmable("short enough"),
            None,
            "⚠ a prompt that already fits is confirmed WHOLE, which is the stronger evidence and \
             what `Delivery::confirm`'s `None` means",
        );
    }

    /// **A REAL SCRIPT ENGINE THAT DISAGREES ABOUT ONE VARIABLE** — the witness this driver's
    /// refusals need, and the only thing that can produce them.
    ///
    /// # ⚠⚠⚠ Why a stand-in ENGINE, where every other fixture here stands in for a PANE
    ///
    /// Three of [`OuterLoop`]'s answers exist because a datamodel can fail to hold what it was
    /// given: [`Briefed::NotHeld`], `Authored::read`'s [`None`], and the `fail` [`advance`] raises
    /// when a prompt cannot be read at the moment of delivery. **None of them is reachable from
    /// the public surface**, and that is not an oversight in the gates — it is the same privacy
    /// that made debt A-1 unfixable from outside: the session id belongs to the loop, so nothing
    /// out here can reach into the datamodel and take something away.
    ///
    /// They were not hypothetical either. `Briefed::NotHeld` is what caught **SCE PR-87** — a
    /// non-ASCII brief silently mangled crossing into `_event.data`, with the event accepted, the
    /// assignment successful and no error raised anywhere. Upstream fixed it, which removed the
    /// one mechanism that drove the arm; **retiring the arm with it would have retired the only
    /// thing in this driver that turns a silent engine defect into a refusal.**
    ///
    /// So the witness is R356's shape, one layer down: not an absence, but **a peer that answers
    /// with a DIFFERENT VALUE**. It delegates every one of the engine's twenty-three methods to a
    /// real `LuaEngine` and lies about exactly one read, on demand.
    ///
    /// ⚠ `lying` is a switch rather than a constructor argument because two of the three arms need
    /// the engine to be TRUTHFUL while the loop is built — `OuterLoop::new` reads all four authored
    /// strings — and to lie afterwards. A wrapper that lied from birth could only ever drive the
    /// constructor's refusal.
    struct Disagreeing {
        inner: Arc<dyn IScriptEngine>,
        /// The variable this engine will not answer honestly about.
        about: &'static str,
        /// What it answers instead, once `lying` is on.
        instead: ScriptValue,
        lying: AtomicBool,
    }

    impl Disagreeing {
        fn about(name: &'static str, instead: ScriptValue) -> Arc<Self> {
            Arc::new(Self {
                inner: Arc::new(sce_rust_lua::LuaEngine::new()),
                about: name,
                instead,
                lying: AtomicBool::new(false),
            })
        }

        fn start_lying(&self) {
            self.lying.store(true, Ordering::SeqCst);
        }
    }

    impl IScriptEngine for Disagreeing {
        fn get_variable(&self, session_id: &str, name: &str) -> ScriptResult<ScriptValue> {
            if name == self.about && self.lying.load(Ordering::SeqCst) {
                return Ok(self.instead.clone());
            }
            self.inner.get_variable(session_id, name)
        }

        // ── everything else is the real engine, verbatim ──
        fn execute_script(&self, session_id: &str, script: &str) -> ScriptResult<ScriptValue> {
            self.inner.execute_script(session_id, script)
        }
        fn evaluate_expression(&self, session_id: &str, expr: &str) -> ScriptResult<ScriptValue> {
            self.inner.evaluate_expression(session_id, expr)
        }
        fn validate_expression(&self, session_id: &str, expr: &str) -> ScriptResult<bool> {
            self.inner.validate_expression(session_id, expr)
        }
        fn set_variable(
            &self,
            session_id: &str,
            name: &str,
            value: ScriptValue,
        ) -> ScriptResult<()> {
            self.inner.set_variable(session_id, name, value)
        }
        fn set_variable_as_dom(&self, session_id: &str, name: &str, xml: &str) -> ScriptResult<()> {
            self.inner.set_variable_as_dom(session_id, name, xml)
        }
        fn has_variable(&self, session_id: &str, name: &str) -> bool {
            self.inner.has_variable(session_id, name)
        }
        fn is_variable_pre_initialized(&self, session_id: &str, name: &str) -> bool {
            self.inner.is_variable_pre_initialized(session_id, name)
        }
        fn setup_system_variables(
            &self,
            session_id: &str,
            session_name: &str,
            io: &[IoProcessorDescriptor],
        ) -> ScriptResult<()> {
            self.inner
                .setup_system_variables(session_id, session_name, io)
        }
        fn set_current_event(
            &self,
            session_id: &str,
            args: SetCurrentEventArgs<'_>,
        ) -> ScriptResult<()> {
            self.inner.set_current_event(session_id, args)
        }
        fn register_global_function(&self, name: &str, callback: NativeMethod) -> bool {
            self.inner.register_global_function(name, callback)
        }
        fn bind_native_object(
            &self,
            session_id: &str,
            object_name: &str,
            methods: Vec<(String, NativeMethod)>,
        ) -> bool {
            self.inner
                .bind_native_object(session_id, object_name, methods)
        }
        fn get_engine_info(&self) -> String {
            self.inner.get_engine_info()
        }
        fn get_memory_usage(&self) -> usize {
            self.inner.get_memory_usage()
        }
        fn collect_garbage(&self) {
            self.inner.collect_garbage();
        }
        fn set_state_query_callback(&self, session_id: &str, callback: Option<StateQueryCallback>) {
            self.inner.set_state_query_callback(session_id, callback);
        }
        fn initialize(&self) -> bool {
            self.inner.initialize()
        }
        fn shutdown(&self) {
            self.inner.shutdown();
        }
        fn is_initialized(&self) -> bool {
            self.inner.is_initialized()
        }
        fn reset(&self) {
            self.inner.reset();
        }
        fn create_session(&self, session_id: &str) {
            self.inner.create_session(session_id);
        }
        fn destroy_session(&self, session_id: &str) {
            self.inner.destroy_session(session_id);
        }
        fn has_session(&self, session_id: &str) -> bool {
            self.inner.has_session(session_id)
        }
    }

    /// The spec these gates drive with — the two knobs each one is actually about, and the two
    /// that are the same for every stand-in here.
    ///
    /// ⚠ `shows_the_prompt` is FALSE throughout, and it is a fact about the fixtures rather than a
    /// convenience: a `/bin/sh` peer paints only once it has a whole LINE, so a delivery cannot be
    /// confirmed on screen before the newline that would submit it — confirming first is a
    /// deadlock. A real agent CLI renders each character into its prompt box as it arrives and
    /// takes the other path, which is what [`AiLoopSpec::driving`] is for.
    fn spec(ready_when: Option<ReadyWhen>, turn: Turn) -> AiLoopSpec {
        AiLoopSpec {
            ready_when,
            ready_within: None,
            turn,
            shows_the_prompt: false,
            // ⚠ ANSWERS NOTHING AND NOBODY IS WATCHING — the default every run has, and the one
            // these gates want: what they are about is the driver, not the answering contract.
            // `ai_loop`'s own gates drive the other half.
            may_answer: None,
            attended: crate::readiness::Attended::NoOne,
            // ⚠ AND NO JUDGE, for the same reason: a judge would put a spawned agent in the middle
            // of every blocked turn these gates drive.
            judge: None,
        }
    }

    /// The turn contract these gates use, bounded at `within`.
    fn turn_of(within: Duration) -> Turn {
        Turn::lasting(INNER_SESSION_ENDS, Some(within)).expect("a non-zero bound")
    }

    /// A pane running `cat` — a peer that takes whatever is typed and says nothing of its own.
    fn quiet_pane() -> (Arc<Mutex<Workspace>>, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 8)
                .expect("spawn pane")
        };
        (workspace, pane)
    }

    /// ⚠⚠⚠ **THE LOOP STOPS ON A WORD, AND SOMETHING HAS TO ASK THE AGENT FOR IT.**
    ///
    /// `judging`'s first guard is `_event.data.done`, which the driver answers by looking for
    /// [`Authored::done_marker`] on the pane. Everything downstream of that — `closing`, the report,
    /// `converged` — happens only if the agent says it.
    ///
    /// **Nothing in the shipped document ever told the agent to.** `start_prompt` and `turn_prompt`
    /// compose the north star, the milestone, the reference and *"report what you did"*; the marker
    /// appears in the datamodel and in the driver's `contains`, and in no sentence any agent reads.
    /// So the only ways a live run could converge were coincidence and R358's rule about a fixture
    /// that manufactures the answer it is testing for: the stand-in below says `MILESTONE REACHED`
    /// because it was written to, and it is the ONLY reason the loop gate above ever finished.
    ///
    /// ⚠ A run against a real agent therefore could not converge at all. It would take all forty of
    /// the document's turns — real minutes of a real agent each — and end `exhausted`, which is the
    /// word for *the budget ran out* rather than for *nobody ever asked*.
    ///
    /// # ⚠⚠ Why the claim is `contains`, and made of the document's OWN marker
    ///
    /// Not a fixed sentence: an author is free to write the instruction however they like, in any
    /// language, and this is the one thing that must survive their editing. Reading the marker out
    /// of the datamodel and asking whether the prompt names it keeps ONE definition — change the
    /// marker and the prompt that carries it is the thing that has to change with it.
    #[test]
    fn the_document_asks_the_agent_for_the_word_the_loop_stops_on() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut loops = OuterLoop::new(lua, pane, &spec(None, turn_of(Duration::from_secs(1))))
            .expect("the document's datamodel must carry its four authored strings");
        // ⚠⚠ THE PROMPTS ARE COMPOSED IN `priming`, so a loop still in `idle` holds them empty and
        // this question cannot be asked of it. One pump is what makes them exist — and it is the
        // same pump that delivers, so what is asserted below is the text the peer was actually
        // sent rather than a copy assembled beside it.
        let run = RunContext::uncancellable();
        let primed = loops
            .pump(&access, &run)
            .expect("the pane must be readable");
        assert!(
            matches!(
                primed,
                Pumped::Moved {
                    to: AiLoopState::Priming,
                    ..
                }
            ),
            "the control: starting a loop must reach the state that composes its prompts, or the \
             assertions below are about an empty datamodel. Got {primed:?}",
        );
        let authored = loops
            .authored()
            .expect("a primed machine must still answer with its four strings");

        assert!(
            !authored.done_marker.is_empty(),
            "⚠ THE CONTROL: an empty marker makes every assertion below trivially true, and makes \
             `said_done`'s `contains` answer yes to every screen ever painted: {authored:?}",
        );
        // ⚠ THE PROMPTS THAT CAN BE THE LAST THING THE AGENT SEES BEFORE `judging` READS THE
        // SCREEN, and only those. `end_prompt` is the closing report, asked once the loop has
        // already decided it is done, so it owes nothing here.
        for (which, prompt) in [("start", &authored.start), ("turn", &authored.turn)] {
            assert!(
                prompt.contains(&authored.done_marker),
                "⚠⚠⚠ the `{which}_prompt` never names {:?}, so an agent reading it is never told \
                 what to say when it is finished — and `judging` waits for exactly that word. A \
                 live run cannot converge; it can only spend the whole `max_turns` budget and end \
                 `exhausted`. The prompt was: {prompt:?}",
                authored.done_marker,
            );
        }
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A CALLER CAN SAY WHAT THE LOOP IS FOR, AND THAT IS WHAT THE AGENT IS ASKED** — the
    /// gate that was the measurement of debt A-1.
    ///
    /// # The defect it was written against, in its own words
    ///
    /// `ai_loop.scxml` ships `(edit me)` placeholders and says *"a GUI fills these in"*. Nothing
    /// did. Asked of the only door there was — [`OuterLoop::new`] plus a construction-time
    /// `authored()` — the whole of what a caller could make this loop send was:
    ///
    /// ```text
    /// North star: (edit me) the outcome this loop exists to reach
    /// Milestone: (edit me) the next checkpoint on the way there
    /// Reference: (edit me) paths, URLs or repos to consult
    /// Report what you did and what is left.
    /// When the milestone is fully reached AND verified, make the last line of your reply
    /// exactly: MILESTONE REACHED
    /// ```
    ///
    /// Three of the five clauses a live agent reads. It could not be repaired from outside either:
    /// the session id was private, and writing a part after `initialize()` would not have helped,
    /// because the prompts were composed from the parts AT init.
    ///
    /// # ⚠⚠ What this asserts, and why each half is separable
    ///
    /// * **the parts are held** — [`Briefed::Took`] is read back out of the datamodel, so a brief
    ///   the script engine dropped cannot report success;
    /// * **the composed prompt carries them** — which is the half a read-back cannot reach, and
    ///   the half the old arrangement failed: the parts were writable in principle and the prompt
    ///   built from them was already fixed.
    /// * **and no placeholder survives into it**, which is the failure stated positively. A
    ///   composition that dropped a part would leave the template's own text in the prompt, and a
    ///   gate checking only that the brief appears would pass with the placeholder beside it.
    #[test]
    fn a_briefed_loop_prompts_an_agent_with_what_it_was_briefed_with() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut loops = OuterLoop::new(
            Arc::clone(&lua),
            pane,
            &spec(None, turn_of(Duration::from_secs(1))),
        )
        .expect("the document's datamodel must carry its four authored strings");

        // ⚠ PROSE, not identifiers. A north star is what a person types: quotes that would splice
        // a hand-built payload apart, an apostrophe, a newline that would end the line early, a
        // brace that would close the object, an em dash and **the language this repository's owner
        // actually writes in**. A brief that survives this survives a real one.
        //
        // ⚠⚠ IT WAS ASCII FOR ONE ROUND AND THAT WAS A LIMIT, NOT A CHOICE: at the previous SCE
        // pin a non-ASCII brief was mangled crossing into `_event.data` and `brief` refused it.
        // Upstream fixed it (PR-87) and this widened back, which is what the retired gate's own
        // failure message asked for.
        let brief = Brief {
            north_star: "ship \"sprag\" 1.0 — don't break the wire {yet}".to_string(),
            milestone: "바깥 루프가\n혼자 돈다".to_string(),
            reference: "~/herdr, ~/ghostty, 그리고 DESIGN.md §5".to_string(),
            max_turns: 3,
            reflect_every: 99,
            // ⚠ The document's own rules, kept: these gates are about the PARTS crossing, and a
            // caller that supplies none is the case the echo has to survive.
            screen_rules: None,
        };
        assert_eq!(
            loops.brief(&brief),
            Briefed::Took,
            "the machine must hold every part of a brief it accepted, read back out of its own \
             datamodel",
        );

        let run = RunContext::uncancellable();
        let primed = loops
            .pump(&access, &run)
            .expect("the pane must be readable");
        assert!(
            matches!(
                primed,
                Pumped::Moved {
                    to: AiLoopState::Priming,
                    ..
                }
            ),
            "the control: the brief must not have stopped the loop starting. Got {primed:?}",
        );
        let start = loops
            .authored()
            .expect("a primed machine answers with its four strings")
            .start;

        for (part, text) in [
            ("north_star", &brief.north_star),
            ("milestone", &brief.milestone),
            ("reference", &brief.reference),
        ] {
            assert!(
                start.contains(text.as_str()),
                "⚠⚠⚠ `{part}` was briefed and the composed prompt does not carry it, so the agent \
                 is asked to work on something nobody said. Prompt:\n{start}",
            );
        }
        assert!(
            !start.contains("(edit me)"),
            "⚠⚠⚠ a placeholder survived into the prompt a live agent reads, which is the defect \
             stated positively — a composition that dropped one part leaves the template's own \
             text where that part should be. Prompt:\n{start}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A BRIEF THE DATAMODEL DOES NOT HOLD EXACTLY IS REFUSED, NOT DELIVERED.**
    ///
    /// # Why this gate exists at all, and why it is not the one it replaced
    ///
    /// Its predecessor drove the same refusal through **SCE PR-87** — a non-ASCII string did not
    /// survive arriving as event data, and [`OuterLoop::brief`]'s read-back is the only thing that
    /// caught it. Upstream landed the fix, so that mechanism is gone, **and deleting the gate with
    /// it would have left `Briefed::NotHeld` with no driver at all** — retiring the one piece of
    /// this driver that turns somebody else's silent bug into a refusal.
    ///
    /// So the question was asked again of the engine, on the other axis a brief carries: **what
    /// does the datamodel do with a number it cannot hold exactly?** Whatever it answers, the
    /// product's rule is the same one — a budget that came back different is a run that would be
    /// bounded by a number nobody chose, and `max_turns` is the ceiling that decides when the loop
    /// stops driving a real agent.
    ///
    /// # ⚠⚠ What is asserted, and what is deliberately NOT
    ///
    /// Not that the engine is wrong to round. It is a script datamodel and `i64::MAX` is outside
    /// what a double holds exactly; that is a fact about the engine, not a defect. What is
    /// asserted is that **the driver notices** — names the part, carries what the machine holds,
    /// and sends the run to `failed` rather than leaving it startable on a budget it did not get.
    #[test]
    fn a_brief_the_datamodel_does_not_hold_exactly_is_refused_rather_than_delivered() {
        let engine = Disagreeing::about(
            "north_star",
            ScriptValue::String("something else entirely".to_string()),
        );
        let (workspace, pane) = quiet_pane();
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut loops = OuterLoop::new(
            Arc::clone(&engine) as Arc<dyn IScriptEngine>,
            pane,
            &spec(None, turn_of(Duration::from_secs(1))),
        )
        .expect("the document's datamodel must carry its four authored strings");
        // ⚠ TRUTHFUL UNTIL NOW, and that is the control: the loop was built against an engine
        // that answered honestly, so the refusal below is about the read-back and not about
        // construction having quietly failed.
        engine.start_lying();

        let brief = Brief {
            north_star: "what the caller asked for".to_string(),
            milestone: "m".to_string(),
            reference: "r".to_string(),
            max_turns: 3,
            reflect_every: 99,
            // ⚠ The document's own rules, kept: these gates are about the PARTS crossing, and a
            // caller that supplies none is the case the echo has to survive.
            screen_rules: None,
        };

        // ── the value came back DIFFERENT: the shape SCE PR-87 produced ──
        let answer = loops.brief(&brief);
        let Briefed::NotHeld { part, held } = answer else {
            panic!(
                "⚠⚠⚠ the loop reports holding a brief its engine answers differently about. This \
                 is the arm that caught SCE PR-87 — a silently mangled brief, with the event \
                 accepted, the assignment successful and no error raised anywhere. Got {answer:?}",
            );
        };
        assert_eq!(
            part, "north_star",
            "the refusal must name the part that did not survive, or a caller cannot tell which \
             of five to rewrite",
        );
        assert_eq!(
            held.as_deref(),
            Some("something else entirely"),
            "⚠ and it must carry what the machine holds INSTEAD, or a caller is told a part was \
             wrong and not what the agent would have been asked to work on",
        );

        // ── and the refusal is a refusal: the run cannot be pumped past it ──
        let run = RunContext::uncancellable();
        let pumped = loops
            .pump(&access, &run)
            .expect("the pane must be readable");
        assert_eq!(
            pumped,
            Pumped::Ended(AiLoopState::Failed),
            "⚠⚠⚠ a loop whose brief the datamodel did not take must not go on to drive an agent. \
             The value is already assigned, so an answer a caller could walk past is a run about \
             something nobody wrote",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A PART THE DATAMODEL ANSWERS ABOUT WITH SOMETHING THAT IS NOT TEXT IS ALSO REFUSED**,
    /// and the refusal says it could not name what is held.
    ///
    /// [`Briefed::NotHeld`]'s `held` is an [`Option`] for exactly this: *the value came back
    /// different* and *the value did not come back as a value at all* are different failures, and
    /// a caller rewriting a brief needs to know which. Without this the `None` arm is a shape
    /// nothing produces.
    #[test]
    fn a_part_that_comes_back_as_nothing_is_refused_and_says_it_cannot_name_what_is_held() {
        let engine = Disagreeing::about("milestone", ScriptValue::Undefined);
        let (workspace, pane) = quiet_pane();
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut loops = OuterLoop::new(
            Arc::clone(&engine) as Arc<dyn IScriptEngine>,
            pane,
            &spec(None, turn_of(Duration::from_secs(1))),
        )
        .expect("the document's datamodel must carry its four authored strings");
        engine.start_lying();

        assert_eq!(
            loops.brief(&Brief {
                north_star: "n".to_string(),
                milestone: "m".to_string(),
                reference: "r".to_string(),
                max_turns: 3,
                reflect_every: 99,
                screen_rules: None,
            }),
            Briefed::NotHeld {
                part: "milestone",
                held: None,
            },
            "a part that is not a string must be refused with `held: None` — a caller told only \
             that something was wrong cannot tell a mangled value from a missing one",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A MACHINE WHOSE DATAMODEL DOES NOT CARRY THE AUTHORED STRINGS IS REFUSED AT
    /// CONSTRUCTION, AND ONE THAT STOPS CARRYING THEM MID-RUN FAILS THE RUN.**
    ///
    /// Two arms with one cause, and neither had a driver before this witness existed:
    ///
    /// * `Authored::read` answers [`None`] and [`OuterLoop::new`] refuses. That is what stops a
    ///   run being started against a machine this driver cannot drive.
    /// * [`advance`](OuterLoop::advance) reads the owed prompt **at the moment of delivery** — the
    ///   whole point of the round that removed the construction-time snapshot — so a datamodel
    ///   that stops answering AFTER the loop was built has to be caught there instead. It raises
    ///   the document's own `fail`, because a machine that cannot say what to send is a run that
    ///   cannot go on, and inventing a `Pumped` arm for it would put a terminal decision in the
    ///   driver.
    ///
    /// ⚠ The two are driven by the SAME lie, switched on at different moments. That is what makes
    /// this a claim about WHEN the prompt is read rather than two coincidences.
    #[test]
    fn a_datamodel_that_stops_answering_refuses_the_loop_or_fails_the_run() {
        // ── at construction ──
        let engine = Disagreeing::about("start_prompt", ScriptValue::Undefined);
        let (workspace, pane) = quiet_pane();
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        engine.start_lying();
        let turn = || {
            Turn::lasting(INNER_SESSION_ENDS, Some(Duration::from_secs(1)))
                .expect("a non-zero bound")
        };
        assert!(
            OuterLoop::new(
                Arc::clone(&engine) as Arc<dyn IScriptEngine>,
                pane,
                &spec(None, turn()),
            )
            .is_none(),
            "⚠⚠ a machine that does not answer with its four authored strings must be refused \
             here, or a run is started against one this driver cannot drive",
        );

        // ── and mid-run, which is a different claim: the prompt is read WHEN IT IS DELIVERED ──
        let engine = Disagreeing::about("start_prompt", ScriptValue::Undefined);
        let mut loops = OuterLoop::new(
            Arc::clone(&engine) as Arc<dyn IScriptEngine>,
            pane,
            &spec(None, turn()),
        )
        .expect("the control: a truthful engine must build a loop");
        engine.start_lying();

        let run = RunContext::uncancellable();
        let pumped = loops
            .pump(&access, &run)
            .expect("the pane must be readable");
        assert_eq!(
            pumped,
            Pumped::Moved {
                from: AiLoopState::Idle,
                raised: AiLoopEvent::Start,
                to: AiLoopState::Failed,
                spent: 0,
            },
            "⚠⚠⚠ the machine moved to `priming`, the prompt could not be read, and the document's \
             own `fail` is what must happen — with nothing typed into the pane. A driver that sent \
             an empty prompt here would report a delivery of nought bytes as a turn",
        );
        // ⚠ `said_done`'s own unreadable-marker arm is NOT asserted here, and saying so is the
        // point: this engine lies about `start_prompt`, not about `done_marker`, so a `false` from
        // it would be the empty screen answering rather than the datamodel. Driving that arm needs
        // a pane that DOES carry the marker plus a liar aimed at it — registered, not faked.
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A BRIEF IS ONLY TAKEN BEFORE THE RUN STARTS**, which is the document's own rule and
    /// not the driver's caution.
    ///
    /// `ai_loop.scxml` puts `brief` on `idle` alone: a loop already driving an agent adopts new
    /// parts through `reflecting`, which writes them and REPLACES the session, because a session
    /// reads its context on the way up and cannot be asked to re-read it. Assigning underneath a
    /// working agent would change what the run is for without the agent ever learning.
    ///
    /// ⚠ The answer has to SAY SO rather than be silently dropped: an unhandled event on this
    /// machine is a no-op, so a caller briefing a started loop would otherwise be told nothing and
    /// go on believing the run was about something it is not.
    #[test]
    fn a_brief_that_arrives_after_the_run_started_is_refused_and_says_where_it_was() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let mut loops = OuterLoop::new(
            Arc::clone(&lua),
            pane,
            &spec(None, turn_of(Duration::from_secs(1))),
        )
        .expect("the document's datamodel must carry its four authored strings");
        let first = Brief {
            north_star: "the one the run is actually for".to_string(),
            milestone: "step one".to_string(),
            reference: "none".to_string(),
            max_turns: 3,
            reflect_every: 99,
            // ⚠ The document's own rules, kept: these gates are about the PARTS crossing, and a
            // caller that supplies none is the case the echo has to survive.
            screen_rules: None,
        };
        assert_eq!(loops.brief(&first), Briefed::Took, "the control");

        let run = RunContext::uncancellable();
        loops
            .pump(&access, &run)
            .expect("the pane must be readable");
        assert_eq!(
            loops.state(),
            AiLoopState::Priming,
            "the control: the run has started",
        );

        let second = Brief {
            north_star: "something else entirely".to_string(),
            ..first.clone()
        };
        assert_eq!(
            loops.brief(&second),
            Briefed::TooLate(AiLoopState::Priming),
            "a brief arriving after the run started must be refused, and must name where the \
             machine was — a caller cannot otherwise tell it from one that was taken",
        );
        let held = loops
            .authored()
            .expect("a primed machine answers with its four strings")
            .start;
        assert!(
            held.contains(&first.north_star) && !held.contains(&second.north_star),
            "⚠⚠ and the refusal must be a refusal: the run goes on being about what it was \
             briefed with. Prompt:\n{held}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE FIRST PROMPT WAITS FOR THE PANE** — the defect R379 found by driving a live agent,
    /// reproduced here where it costs no agent turns.
    ///
    /// The driver built a barrier in `new` and consulted it only in `watch`, which runs once the
    /// machine is in `working` — **after** the transition that delivers the start prompt. So the
    /// loop typed into whatever was in the pane at the instant it was asked to start.
    ///
    /// # ⚠⚠ Why no stand-in had ever shown it, and what makes this one able to
    ///
    /// Every fixture in this module calls [`started`] before the run, which is right — it takes
    /// process startup out of the measurement — and it means **the HARNESS was clearing the barrier
    /// the PRODUCT was not.** This peer is deliberately slow to come up and the gate does NOT
    /// pre-wait for it, so the barrier is the only thing that can.
    ///
    /// ⚠ The claim is a DISTANCE, not an ordering: the peer cannot announce itself for a second, so
    /// a driver that skips the barrier returns in milliseconds and one that honours it cannot. A
    /// gate asserting only *"it was delivered eventually"* would pass either way.
    #[test]
    fn the_loop_does_not_type_its_first_prompt_before_the_pane_is_ready() {
        /// How long the peer takes to come up. Far outside the poll interval, far inside the
        /// gate's own patience.
        const SLOW: Duration = Duration::from_millis(1_000);

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("sleep 1; stty -echo; printf 'PEER-READY\\n'; exec cat");
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        // ⚠ NO `started` HERE. That is the whole point — see the doc above.

        let mut loops = OuterLoop::new(
            lua,
            pane,
            &spec(
                Some(ReadyWhen::Prints("PEER-READY".to_string())),
                turn_of(Duration::from_millis(200)),
            ),
        )
        .expect("the document's datamodel must carry its four authored strings");

        let began = std::time::Instant::now();
        let pumped = loops
            .pump(&access, &RunContext::uncancellable())
            .expect("the pane must stay readable");
        let waited = began.elapsed();

        assert!(
            matches!(
                pumped,
                Pumped::Moved {
                    to: AiLoopState::Priming,
                    ..
                },
            ),
            "the loop starts by priming: {pumped:?}",
        );
        assert!(
            waited >= SLOW - Duration::from_millis(100),
            "⚠⚠⚠ the start prompt went in after {waited:?}, and this peer cannot read a byte for \
             {SLOW:?}. A driver that does not clear its own barrier types into a program that is \
             still starting — measured against a live agent as a whole run that delivered in 10 ms \
             and then sat in `working` until somebody stopped it.",
        );
        assert!(
            access
                .pane_collapsed(pane)
                .is_some_and(|seen| seen.contains("PEER-READY")),
            "⚠ THE CONTROL: the peer really did announce itself, so the wait above was the barrier \
             doing its job rather than the fixture being slow for some other reason",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **THE LOOP MUST NOT CONVERGE ON ITS OWN INSTRUCTION, NOR ON AN OLD TURN'S MARKER.**
    ///
    /// The two halves of [`OuterLoop::said_done`]'s rule, each driven by the case the OTHER half
    /// cannot answer — which is what stops one of them being quietly deleted later:
    ///
    /// * a pane showing the loop's own prompt, marker and all, is **not** an agent that finished;
    /// * a pane showing a marker from BEFORE this turn is not this turn's answer either;
    /// * and a pane where the agent really did end its reply with the marker **is**.
    ///
    /// ⚠ Driven through `say`, not by hand, because arming is the whole mechanism: the baseline
    /// and the delivery have to happen in that order, and a gate that marked its own baseline would
    /// be testing its own arrangement.
    #[test]
    fn a_marker_the_loop_typed_or_that_predates_the_turn_is_not_the_agent_saying_it() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        // A peer that PAINTS BACK EVERY LINE IT READS and says nothing of its own — the echo, with
        // no answer behind it. Exactly what an agent's composer does to a prompt.
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(
                "stty -echo; printf 'PARROT-READY\\n'; while read line; do \
                 printf '%s\\n' \"$line\"; done",
            );
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 80, 24)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        started(&access, pane, "PARROT-READY");

        let mut loops = OuterLoop::new(lua, pane, &spec(None, turn_of(Duration::from_millis(200))))
            .expect("the document's datamodel must carry its four authored strings");
        let run = RunContext::uncancellable();

        // ── THE ECHO ── the whole start prompt, which NAMES the marker, painted straight back.
        //
        // ⚠ Delivered by ONE PUMP rather than by a hand-built `say`, and that is not a
        // convenience: the prompts are composed in `priming`, so the pump is what makes the text
        // exist at all — and it arms the baseline through the same call the driver uses in
        // production, which is what the note above means by not testing its own arrangement.
        let primed = loops
            .pump(&access, &run)
            .expect("the parrot stays readable");
        assert!(
            matches!(
                primed,
                Pumped::Moved {
                    to: AiLoopState::Priming,
                    ..
                }
            ),
            "the control: the start prompt is delivered by reaching `priming`. Got {primed:?}",
        );
        let authored = loops
            .authored()
            .expect("a primed machine answers with its four strings");
        let marker = authored.done_marker.clone();
        started(&access, pane, &marker);
        assert!(
            !loops.said_done(&access),
            "⚠⚠⚠ the marker on that screen is the LOOP'S OWN INSTRUCTION read back — the peer has \
             said nothing. A judge satisfied here converges a run in which no agent ever did \
             anything, which is what this driver did the moment the document started asking for \
             the marker at all. Screen: {:?}",
            access.pane_collapsed(pane),
        );

        // ── THE ANSWER ── the same peer, now ending a reply with the marker and nothing else.
        // ⚠ The answer is ASSERTED rather than dropped — R378 paid for exactly this `#[must_use]`
        // one round ago, and a write of nothing would leave the wait below spinning to its deadline
        // with no clue why.
        let typed = access
            .inject(pane, &{
                let mut keys = crate::access::KeyStroke::text(&marker);
                keys.push(crate::access::KeyStroke::named("Enter"));
                keys
            })
            .expect("the parrot takes a line");
        assert!(
            typed.bytes() > 0,
            "a marker that reached no pane cannot be read back off one",
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !loops.said_done(&access) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            loops.said_done(&access),
            "⚠⚠ and a row that IS the marker must be read as one, or the rule above is satisfied \
             by a judge that never says yes to anything. Screen: {:?}",
            access.pane_collapsed(pane),
        );

        // ── THE STALE MARKER ── a NEW turn begins on a screen that already shows it. This is the
        //    half `stands_alone` cannot answer: that row stands alone perfectly well, and it
        //    belongs to a turn that is over.
        loops
            .say(&access, &run, &authored.turn.clone())
            .expect("the parrot takes the next prompt");
        assert!(
            !loops.said_done(&access),
            "⚠⚠⚠ that marker was on the screen BEFORE this turn's prompt went in, so it is the \
             previous turn's answer being counted twice — the arming discipline `Completion::begin` \
             exists for, applied to text. Screen: {:?}",
            access.pane_collapsed(pane),
        );

        access.lifecycle().expect("lifecycle").close(pane);
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
            &spec(
                Some(ReadyWhen::Settles("claude".to_string())),
                turn_of(Duration::from_secs(5)),
            ),
        )
        .expect("the document's datamodel must carry its four authored strings");

        assert_eq!(loops.state(), AiLoopState::Idle, "the document's `initial`");
        // ⚠⚠ THE RUN IS TOLD WHAT IT IS FOR, as any caller must. Before this door existed the only
        // thing a loop could ask an agent to do was the template's `(edit me)`.
        let brief = Brief {
            north_star: "the stand-in answers two prompts and then says the marker".to_string(),
            milestone: "reach it".to_string(),
            reference: "this gate".to_string(),
            max_turns: 40,
            reflect_every: 99,
            // ⚠ The document's own rules, kept: these gates are about the PARTS crossing, and a
            // caller that supplies none is the case the echo has to survive.
            screen_rules: None,
        };
        assert_eq!(loops.brief(&brief), Briefed::Took, "the parts must be held");
        // ⚠⚠⚠ AND NOTHING IS COMPOSED YET, which is the CONTROL for the whole arrangement. The
        // prompts are built in `priming`; a loop in `idle` holding them already would mean the
        // composition had happened at `<datamodel>` init, and the brief above could not have
        // reached the text below however correctly it was stored.
        let before = loops
            .authored()
            .expect("an unstarted machine still declares its four strings");
        assert!(
            before.start.is_empty() && before.turn.is_empty(),
            "⚠⚠⚠ a loop that has not primed must hold no composed prompt, or a brief cannot reach \
             one: {before:?}",
        );
        // ⚠⚠⚠ AND THE MARKER IS ITS OWN CONTROL, because the way it fails is invisible. `said_done`
        // asks whether a row IS the marker, and every row ends with the empty one — so a
        // `done_marker` that arrived empty makes the loop converge on its first turn, reporting
        // a milestone reached against a screen that says nothing of the kind. The driver did
        // exactly that, and this is the assertion that would have named it in one line.
        //
        // ⚠ It is NOT composed, so it is readable before priming — which is what makes it usable
        // as a control here at all.
        assert_eq!(
            before.done_marker, "MILESTONE REACHED",
            "the document's own marker, whole — an empty one is a suffix of every row: {before:?}",
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
                // ⚠ The barrier the loop now clears BEFORE its first prompt (R379). This fixture's
                // peer is up and supervised as `claude` at rest, so `Yes` is the only honest
                // answer here; anything else means the fixture stopped standing in for a started
                // agent and the walk below would be about a pane nobody had spoken to.
                Pumped::NotReady(seen) => panic!(
                    "the stand-in agent must be ready — it printed its marker before this run \
                     began. Got {seen:?}, walked: {walked:?}",
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
        assert!(
            typed.contains(&brief.north_star),
            "⚠⚠⚠ and what reached the pane must be what this run was BRIEFED with, not the \
             template it was composed from. Typed: {typed:?}",
        );
        // ⚠⚠ AND THE RUN SAYS WHAT IT SPENT. Every bounded run in this crate can, and the outer
        // loop could not until the compiler objected to the dropped `Written`. The claim is the
        // RELATION, not a byte count: what reached the pane is what the three authored prompts
        // weigh, so a driver that silently sent nothing — or sent something else — cannot pass.
        let composed = loops
            .authored()
            .expect("a converged machine still answers with its four strings");
        let authored_weight =
            (composed.start.len() + composed.turn.len() + composed.end.len()) as u64;
        assert!(
            spent_total >= authored_weight,
            "⚠⚠ the three prompts weigh {authored_weight} bytes and the run reports spending \
             {spent_total}. A loop whose spend is less than what it typed is not reporting its \
             own cost, which is the one thing a bounded run always owes. Walked: {walked:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **WHAT SHAPE THE AUTHORED `screen_rules` CROSS THE DATAMODEL IN** — asked of the engine
    /// before anything is built to read them, because the whole of `screening` rests on the answer.
    ///
    /// The document declares them as an ECMAScript array of objects and the engine that evaluates
    /// it is LUA — `ai_loop.scxml`'s own measured warning, with the codegen rewriting `[...]` into
    /// `{...}` and `key:` into `key =` on the way in. A Lua table is one construct for both a list
    /// and a map, so *"an array of three objects"* is a **prediction** about what
    /// [`IScriptEngine::get_variable`] hands back, and this workspace has been wrong about exactly
    /// this kind of prediction before.
    ///
    /// ⚠⚠ **AND IT SETTLES WHETHER PR-86's THIRD ASK BLOCKS THIS.** SCE emits no read accessor for a
    /// lowered SCALAR `<data>`, which is why every string in this driver is read through the script
    /// session. Measured here: a COMPOSITE `<data>` is not lowered at all, so the interpreter route
    /// reads it whole — the missing accessor bounds what the policy can answer, not what a driver
    /// can see.
    ///
    /// ⚠ The non-ASCII half is asserted too, and not for symmetry: the replies in that list are
    /// Korean, and PR-87 was a round in which non-ASCII crossed one route into this datamodel and
    /// not the other. This is the AUTHORED route, on a value shape nothing had read before.
    #[test]
    fn the_authored_screen_rules_cross_the_datamodel_as_a_readable_list() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut machine = Engine::new(AiLoopPolicy::new(Arc::clone(&lua)));
        machine.initialize();
        let session = machine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");

        let Ok(ScriptValue::Array(rules)) = lua.get_variable(&session, "screen_rules") else {
            panic!(
                "⚠⚠⚠ the document's standing instructions must reach a driver as a LIST. Anything \
                 else and `screening` cannot read what the author wrote: {:?}",
                lua.get_variable(&session, "screen_rules"),
            );
        };
        assert!(
            !rules.is_empty(),
            "⚠ the control: the document ships rules, so an empty list here would make every \
             assertion below vacuous",
        );
        for (at, rule) in rules.iter().enumerate() {
            let ScriptValue::Object(fields) = rule else {
                panic!("⚠⚠ rule {at} must cross as an object with named fields, not {rule:?}");
            };
            for field in [ScreenRule::WHEN_KEY, ScreenRule::TEXT_KEY] {
                assert!(
                    matches!(fields.get(field), Some(ScriptValue::String(held)) if !held.is_empty()),
                    "⚠⚠ rule {at} must carry a non-empty {field:?} — a rule missing either half is \
                     one that claims nothing or says nothing: {fields:?}",
                );
            }
            assert!(
                fields.get("keys").is_none(),
                "⚠⚠⚠ and it must NOT carry a key of its own. The key that refuses a call is the \
                 product's and was measured; a rule able to name one could name the key that \
                 APPROVES, which a live probe demonstrated by having a file written: {fields:?}",
            );
        }
        assert!(
            rules.iter().any(|rule| matches!(
                rule,
                ScriptValue::Object(fields)
                    if matches!(fields.get("text"), Some(ScriptValue::String(text))
                        if !text.is_ascii())
            )),
            "⚠⚠⚠ and a reply in the author's OWN LANGUAGE must survive the crossing. The shipped \
             replies are Korean; PR-87 was a round in which non-ASCII reached this datamodel by one \
             route mangled and by the other whole, and this is the route nothing had measured",
        );
    }

    /// ⚠⚠ **THE STANDING LIST IS NORMALISED ONCE EACH AND IN THE ORDER IT WAS LEARNED** —
    /// [`once_each`], whose two claims are load-bearing for different reasons.
    ///
    /// * **ONCE EACH** decides whether a reflection pays for a restart: `screen.matched` appends, so
    ///   an agent that asks the same thing twice puts the same line in twice, and a duplicate is
    ///   something the prompts do not carry. A run whose agent kept asking one question would replace
    ///   its session every time it did. ⚠ A mutation that kept duplicates goes red at two loop gates,
    ///   so that half is covered end to end; this states it directly.
    /// * **IN ORDER** is not covered anywhere else, because no gate makes two DIFFERENT rules fire in
    ///   one run. These are a person's instructions, and `ScreenRules` already says why a list somebody
    ///   wrote has its order as part of what it says — a normaliser that sorted or reversed would hand
    ///   an agent the author's words in an order the author did not choose.
    #[test]
    fn the_standing_list_keeps_each_instruction_once_in_the_order_it_was_learned() {
        assert_eq!(
            super::once_each(""),
            "",
            "⚠ nothing learned stays NOTHING, and not a blank line: this value is composed straight \
             into the middle of a prompt, so an empty list that ended a line would put one there",
        );
        assert_eq!(
            super::once_each("second\nfirst\nsecond\n"),
            "second\nfirst\n",
            "the duplicate goes and the FIRST occurrence keeps its place",
        );
        assert_eq!(
            super::once_each("a\n\n   \nb\n"),
            "a\nb\n",
            "and blank lines are not instructions — the accumulation ends every line, so a list \
             assigned back after a reflection would otherwise grow one empty entry per pass",
        );
    }
}
