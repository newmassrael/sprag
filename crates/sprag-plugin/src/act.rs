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
}

impl Act {
    /// Every act this host serves.
    ///
    /// ⚠ The one list. [`Act::of`] reads it rather than spelling a second `match`, so an act added
    /// to the enum is served the moment it names itself.
    pub const ALL: [Self; 1] = [Self::Say];

    /// The name a document calls this act by — its own `<send event="…">`.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Say => "prompt.say",
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
}

impl Asks {
    /// Every value this argument may hold.
    pub const ALL: [Self; 2] = [Self::Work, Self::Account];

    /// The word a document writes for it.
    #[must_use]
    pub const fn named(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Account => "account",
        }
    }

    /// What `word` asks for, or [`None`] for a word this space does not hold.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|asks| asks.named() == word)
    }
}

/// **ONE ACT THE DOCUMENT ASKED FOR, WITH THE ARGUMENTS IT SENT.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// The act.
    pub act: Act,
    /// [`Act::Say`]'s `text` — the sentence to put to the peer, as the document composed it.
    pub text: String,
    /// [`Act::Say`]'s `asks` — what that sentence is asking for.
    pub asks: Asks,
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
            } => write!(
                f,
                "`{}`'s `<param name=\"{argument}\">` said {said:?}, which is not one of {:?}",
                act.named(),
                Asks::ALL.map(Asks::named),
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
#[derive(Default)]
struct Book {
    /// The act the document has asked for and nothing has carried out yet.
    ///
    /// ⚠ ONE, not a queue, and that is a claim about this host rather than a simplification: the
    /// driver carries out one act per pass, so a second one arriving before the first is taken is
    /// an act nobody could perform. It is REFUSED (`Refused::Overrun`) instead of overwriting the
    /// slot, because an overwrite is precisely the silence this module exists to end.
    asked: Option<Asked>,
    /// Every act this host would not perform, in the order they were asked for.
    refused: Vec<Refused>,
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
            // ⚠⚠ THE SLOT IS CONSULTED BEFORE THE ARGUMENTS ARE, so a second act is refused for
            // BEING second rather than for whatever else might also be wrong with it. Two reasons
            // reported as one is the fold this register keeps paying for.
            let answer = match (&book.asked, read(&request.event_name, &request.params)) {
                (Some(waiting), Ok(arriving)) => Err(Refused::Overrun {
                    held: waiting.act,
                    arriving: arriving.act,
                }),
                (_, answered) => answered,
            };
            let held = &mut *book;
            match answer {
                Ok(asked) => {
                    held.asked = Some(asked);
                    Vec::new()
                }
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

    /// **THE ACT THE DOCUMENT ASKED FOR AND NOTHING HAS CARRIED OUT**, taken.
    ///
    /// Taking rather than reading: an act is performed once. A second pass over the same slot would
    /// put the same sentence to the peer twice.
    pub fn taken(&self) -> Option<Asked> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .asked
            .take()
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
                });
            };
            Ok(Asked { act, text, asks })
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

    use super::{Act, Asks, Refused, Serving, read};
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
        assert!(
            serving.taken().is_none(),
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
        let with = |pairs: &[(&str, &str)]| {
            let mut params: HashMap<String, Vec<String>> = HashMap::new();
            for (name, value) in pairs {
                params
                    .entry((*name).to_owned())
                    .or_default()
                    .push((*value).to_owned());
            }
            read(Act::Say.named(), &params)
        };

        // ── THE STAGED CONTROL: the well-formed act, so every refusal below is about the breach ──
        assert_eq!(
            with(&[("text", "where did you get to?"), ("asks", "account")])
                .expect("a well-formed act is performed")
                .asks,
            Asks::Account,
            "⚠⚠⚠ THE CONTROL: the act this host serves, with the arguments the document writes, \
             must be READ — otherwise the refusals below are consistent with a host that refuses \
             everything",
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
        let carried = serving.taken().expect("the first act is still to be done");
        assert_eq!(
            carried.text, "the first question",
            "⚠⚠⚠⚠ THE SECOND MUST NOT HAVE OVERWRITTEN THE FIRST. An overwrite refuses the machine \
             and then performs the act it refused, which is worse than either answer alone.",
        );
        assert!(
            serving.taken().is_none(),
            "⚠⚠ and an act is carried out ONCE — a slot that answered twice would put the same \
             sentence to the peer again on the next pass",
        );
    }
}
