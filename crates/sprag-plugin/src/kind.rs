//! **THE DECISIONS ONE LOOP KIND RUNS UNDER** — a document beside the template rather than inside
//! it.
//!
//! # ⚠⚠⚠ What this exists to move, and what it cost to leave it where it was
//!
//! `ai_loop.scxml` is a TEMPLATE: other repositories copy it. Everything that makes a run a DEBT run
//! rather than a feature run is `<data>` — consents, standing instructions, prompt wording — and
//! while those lived in the template, **this repository's standing yesses authorised every run of it,
//! for everybody, until somebody edited the file.** The template's own comment said so and the
//! clauses stayed anyway, which is the evidence that purity cannot be left to attention:
//! [`crate::outer`]'s
//! `a_template_names_nothing_of_this_repository_and_decides_nothing_for_the_next_one` is the gate
//! that measures it, and this module is half of what turns that gate green.
//!
//! # ⚠⚠⚠ Why a SIBLING document and not a parent
//!
//! The design this replaces had the kind document `<invoke>`ing the template and filling it with
//! `<param>`. Both halves of that were proven by the probe — and the question nobody asked was
//! whether the DRIVER could still drive an invoked child. **It cannot**: at the pinned SCE the
//! generated parent owns its child as a private field with no accessor, so an invoked `ai_loop`
//! could not be read, pumped, or sent an event. The refutation is recorded in this crate's `probe`
//! module, compiler error and all.
//!
//! So the two documents stand SIDE BY SIDE. The driver holds the template, reads this, and carries
//! the values over at `start` — where the template's own unconditional `<assign>` was already
//! waiting for them. **The driver decides nothing; it transports.** That is the governing rule
//! satisfied rather than bent: every decision is still in an `.scxml`, and the one it is in is the
//! one whose author owns it.
//!
//! ⚠⚠ This is also the shape this repository already chose once without noticing. `context_review`
//! argues at length that an analysis with steps must be a machine, and its header says the loop will
//! `<invoke>` it; what was BUILT is `ContextReview` in [`crate::review`] — a second engine the driver
//! drives directly, beside the first. Nesting was the intent; siblings are what works.
//!
//! # ⚠ What happens when a kind acquires STEPS
//!
//! Nothing here has to change shape. A kind that must analyse, consult a judge or wait for a person
//! before the first prompt becomes a machine the driver drives to completion first — which is
//! exactly what `ContextReview` already is. A kind is a datamodel until it is a machine, and neither
//! of those is a parent.

use std::sync::Arc;

use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

use crate::consent::Consents;
use crate::outer::{Counted, NotScreenable, OuterLoop};
use crate::screen::ScreenRules;

/// 🎯🎯🎯🎯🎯 **THE CLAUSES A KIND DOCUMENT MAY HOLD, AS ONE INTERFACE OVER ANY OF THEM** —
/// register item 848.
///
/// # ⚠⚠⚠⚠⚠ Why this had to exist before *which kind* could be a run argument
///
/// Codegen emits a POLICY TYPE PER DOCUMENT, so a field typed `Engine<DebtLoopPolicy>` is a struct
/// that can hold exactly one kind — and while [`LoopKind`] held that field, the driver could only
/// ever construct that one. A wire key with one legal value is not a choice, which is why the
/// construction site's own note said naming the kind was scope rather than design, and why the
/// hardcoding survived: **there was nothing else to name.**
///
/// ⚠⚠ What that cost is not a missing feature. `successor_check` — the program that says whether a
/// run may re-aim itself — is a KIND's clause, and this repository's kind points it at THIS
/// repository's record. One hardcoded kind therefore meant a run in any other tree would be judged
/// by a checker that has never heard of it, and refused, because a proposal naming no item of a
/// record is a `NO`. Item 847 made *nobody classified this* audible; this makes it FALSE for a run
/// that never asked for a classifier.
///
/// # ⚠⚠⚠ Why an interface over the generated accessors rather than a reader per clause
///
/// Half of [`LoopKind`]'s readers already go through the script session by id, and those are
/// document-agnostic already. The other half moved to the GENERATED accessors on purpose
/// (SCE PR-86's R-86.4, consumed 2026-08-20) — the codegen types the clause from the literal its
/// author wrote, so a document that spells a number as a word stops compiling instead of reading
/// back as a surprise. Giving that up to erase one type would trade a compile error for a run-time
/// one, so what is erased is the ENGINE and not the typing: each policy implements this, and a
/// method's body is still the accessor codegen wrote for that document.
///
/// ⚠⚠⚠ **A CLAUSE A DOCUMENT DOES NOT DECLARE HAS NO ACCESSOR, so its method answers [`None`] —
/// and that is the one place an implementation can go stale against its document.** It is held by
/// `a_kind_declares_exactly_what_its_readers_read`, which pins each document's declared ids: a
/// clause added to a document whose implementation still answers `None` is a red there, naming the
/// id and the file.
pub(crate) trait KindDocument {
    /// The script session the document's `<data>` were evaluated into, or [`None`] for a document
    /// that opened none — which is [`NoKind::NoDatamodel`] at the door.
    fn session_id(&self) -> Option<String>;
    /// `closing_rules`, or [`None`] where this document does not declare it.
    fn closing_rules(&self) -> Option<String>;
    /// `working_rules`, or [`None`] where this document does not declare it.
    fn working_rules(&self) -> Option<String>;
    /// `unanswered_rule`, or [`None`] where this document does not declare it.
    fn unanswered_rule(&self) -> Option<String>;
    /// `unreadable_rule`, or [`None`] where this document does not declare it.
    fn unreadable_rule(&self) -> Option<String>;
    /// `unwell_rule`, or [`None`] where this document does not declare it.
    fn unwell_rule(&self) -> Option<String>;
    /// `reference`, or [`None`] where this document does not declare it.
    fn reference(&self) -> Option<String>;
    /// `works_in`, or [`None`] where this document does not declare it.
    fn works_in(&self) -> Option<String>;
    /// `stands_in`, or [`None`] where this document does not declare it.
    fn stands_in(&self) -> Option<String>;
    /// `keeps`, or [`None`] where this document does not declare it.
    fn keeps(&self) -> Option<String>;
    /// `hold_within_ms`, or [`None`] where this document does not declare it.
    fn hold_within_ms(&self) -> Option<i64>;
    /// `reflect_every`, or [`None`] where this document does not declare it.
    fn reflect_every(&self) -> Option<i64>;
    /// `context_ceiling`, or [`None`] where this document does not declare it.
    fn context_ceiling(&self) -> Option<i64>;
    /// `reflect_after_refusals`, or [`None`] where this document does not declare it.
    fn reflect_after_refusals(&self) -> Option<i64>;
}

impl KindDocument for Engine<crate::sm::debt_loop::DebtLoopPolicy> {
    fn session_id(&self) -> Option<String> {
        self.policy().session_id.clone()
    }
    fn closing_rules(&self) -> Option<String> {
        self.policy().closing_rules()
    }
    fn working_rules(&self) -> Option<String> {
        self.policy().working_rules()
    }
    fn unanswered_rule(&self) -> Option<String> {
        self.policy().unanswered_rule()
    }
    fn unreadable_rule(&self) -> Option<String> {
        self.policy().unreadable_rule()
    }
    fn unwell_rule(&self) -> Option<String> {
        self.policy().unwell_rule()
    }
    fn reference(&self) -> Option<String> {
        self.policy().reference()
    }
    fn works_in(&self) -> Option<String> {
        self.policy().works_in()
    }
    fn stands_in(&self) -> Option<String> {
        self.policy().stands_in()
    }
    fn keeps(&self) -> Option<String> {
        self.policy().keeps()
    }
    fn hold_within_ms(&self) -> Option<i64> {
        self.policy().hold_within_ms()
    }
    fn reflect_every(&self) -> Option<i64> {
        self.policy().reflect_every()
    }
    fn context_ceiling(&self) -> Option<i64> {
        self.policy().context_ceiling()
    }
    fn reflect_after_refusals(&self) -> Option<i64> {
        self.policy().reflect_after_refusals()
    }
}

/// ⚠⚠⚠ **ELEVEN OF THESE FOURTEEN ANSWER `None` BECAUSE THE DOCUMENT DECLARES NOTHING**, which is
/// the whole of what an unclaimed kind is — not a kind with cautious values, a kind with no values,
/// so the template's own numbers and the caller's own arguments stand exactly as they would have.
///
/// ⚠⚠ Each `None` here is a fact about that file rather than a choice made in Rust, and
/// `a_kind_declares_exactly_what_its_readers_read` is what keeps it one: the day that document
/// declares a fourth id, the pin goes red naming it, and this implementation is what the red sends
/// a reader to.
impl KindDocument for Engine<crate::sm::unclaimed_loop::UnclaimedLoopPolicy> {
    fn session_id(&self) -> Option<String> {
        self.policy().session_id.clone()
    }
    fn closing_rules(&self) -> Option<String> {
        None
    }
    fn working_rules(&self) -> Option<String> {
        None
    }
    fn unanswered_rule(&self) -> Option<String> {
        None
    }
    fn unreadable_rule(&self) -> Option<String> {
        None
    }
    fn unwell_rule(&self) -> Option<String> {
        None
    }
    fn reference(&self) -> Option<String> {
        None
    }
    fn works_in(&self) -> Option<String> {
        None
    }
    fn stands_in(&self) -> Option<String> {
        None
    }
    fn keeps(&self) -> Option<String> {
        None
    }
    fn hold_within_ms(&self) -> Option<i64> {
        None
    }
    fn reflect_every(&self) -> Option<i64> {
        None
    }
    fn context_ceiling(&self) -> Option<i64> {
        None
    }
    fn reflect_after_refusals(&self) -> Option<i64> {
        None
    }
}

/// One loop kind's authored decisions, read off its own document.
///
/// It holds the script SESSION rather than the values, for the reason `pump` re-reads the template
/// on every pass: a value copied at construction is a value that can no longer be corrected, and
/// what an author wrote is the authority for as long as the run lasts.
pub struct LoopKind {
    /// The engine is kept alive because the session id below is only meaningful while it is —
    /// dropping the machine closes the script session and every read after it answers nothing.
    ///
    /// ⚠⚠⚠ **BEHIND [`KindDocument`] RATHER THAN TYPED TO ONE POLICY** — register item 848. While
    /// this was `Engine<DebtLoopPolicy>` there was exactly one kind a run could be started under,
    /// so *which kind* could not be an argument and the driver named this repository's own for
    /// everybody.
    machine: Box<dyn KindDocument + Send>,
    /// WHICH kind this is, in the word a caller names it by — see [`LoopKind::named`].
    named: &'static str,
    script: Arc<dyn IScriptEngine>,
    session: String,
}

/// Why a kind document could not be opened — distinct from a kind whose CONTENTS are unreadable
/// ([`NotScreenable`]), because an author can fix the second and only a build can fix the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoKind {
    /// The machine initialised but opened no script session, so there is no datamodel to read. A
    /// document compiled with `datamodel="null"` reaches this, which is a build-time mistake rather
    /// than an authoring one.
    NoDatamodel,
    /// ⚠⚠⚠⚠⚠ **A CLAUSE IN THE KIND DOCUMENT COULD NOT BE EVALUATED, AND NOTHING IN IT CAN SAY SO**
    /// — register item 505.
    ///
    /// Every decision in a kind is a `<data>` expression, and W3C SCXML 5.2/5.3 raises
    /// `error.execution` for one that cannot be evaluated. This document's only state is `<final>`,
    /// so there is nowhere a transition could answer that — see the document's own note — and W3C
    /// 3.12.2 then drops the event. What was LEFT was a driver reading whatever the datamodel
    /// happened to hold: a consent list that came out empty, a standing instruction that is not
    /// there, and a run proceeding under decisions its author did not make.
    ///
    /// ⚠⚠ So the refusal is the door's, and it belongs here rather than in
    /// [`NoDatamodel`](Self::NoDatamodel): that one says *this was built wrong* and a build fixes
    /// it; this one says *what you wrote did not evaluate* and the author fixes it.
    Faulted(crate::document::Faulted),
    /// 🎯🎯🎯🎯🎯 **A RUN NAMED A KIND THIS BUILD HAS NO DOCUMENT FOR** — register item 848.
    ///
    /// ⚠⚠ It is a REFUSAL rather than a fall-through, and that is the item rather than a taste: the
    /// state this replaced was a driver naming one kind for every caller, so *anything unrecognised
    /// runs under this repository's* is exactly the escape hatch that made a checker pointed at
    /// this repository's record the judge of everybody's work.
    Unknown(String),
}

impl std::fmt::Display for NoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDatamodel => f.write_str(
                "this loop kind's document opened no script session, so it holds no decisions — a \
                 kind must declare `datamodel=\"ecmascript\"`",
            ),
            Self::Unknown(named) => write!(
                f,
                "no loop kind of this build is called {named:?}, and a run cannot start under a \
                 document nobody has. What this build holds: {}. ⚠ A kind is not a label — it names \
                 the checker that decides whether a run may re-aim itself, and a checker reads one \
                 tree's own record, so falling through to another kind's would have this run judged \
                 by a document that has never heard of its work. Name {:?} for a run that should \
                 hold no decisions but the template's and your own",
                LoopKind::KINDS.join(", "),
                LoopKind::UNCLAIMED,
            ),
            Self::Faulted(faulted) => write!(
                f,
                "this loop kind's document did not evaluate its own decisions: {faulted}. A kind is \
                 a datamodel with one final state, so nothing in it can answer an error — the \
                 clause that failed left the driver with whatever the datamodel happened to hold, \
                 and a run started on that would run under decisions nobody authored",
            ),
        }
    }
}

impl LoopKind {
    /// 🎯🎯🎯🎯🎯 **THE KIND WORD A CALLER NAMES `debt_loop.scxml` BY** — register item 848.
    pub const DEBT: &'static str = "debt";

    /// 🎯🎯🎯🎯🎯 **THE KIND WORD A CALLER NAMES `unclaimed_loop.scxml` BY** — the kind that holds
    /// no decisions, for a run whose tree has not written one of its own.
    pub const UNCLAIMED: &'static str = "unclaimed";

    /// **EVERY KIND THIS BUILD CAN START A RUN UNDER**, in the words [`named`](Self::named) takes.
    ///
    /// ⚠⚠⚠ **PUBLISHED rather than private**, because the wire's own refusal has to say what the
    /// legal answers ARE: a door that rejects a word without naming the alternatives sends a caller
    /// to read the daemon's source, and item 848's whole complaint is about a choice nobody could
    /// see. It is also what the argument's published grammar is built from, so the list a client is
    /// shown and the list the door accepts cannot drift.
    pub const KINDS: &'static [&'static str] = &[Self::DEBT, Self::UNCLAIMED];

    /// 🎯🎯🎯🎯🎯 **THE KIND A RUN NAMED, RESOLVED TO ITS DOCUMENT** — register item 848, and the
    /// only road a run should reach a kind by.
    ///
    /// # ⛔⛔⛔⛔⛔ What the absent argument used to mean, and why there is no default here
    ///
    /// There was one kind, so the driver named it: every run of the template, in every tree, ran
    /// under THIS repository's document. That is not a strict default — a kind names the checker
    /// that decides whether a run may re-aim itself, and this repository's checker reads this
    /// repository's record, so a run elsewhere would be told *no* by a document that has never
    /// heard of its work.
    ///
    /// ⛔ **SO AN UNKNOWN WORD IS A REFUSAL AND SO IS SILENCE**, and the caller's own door is where
    /// the second one is answered — this function is never handed a *nothing*. A fall-through here
    /// would be the escape hatch the whole item is about: *unclassified is a red, not a pass*.
    ///
    /// # Errors
    ///
    /// [`NoKind::Unknown`] for a word no document answers to, and [`NoKind::NoDatamodel`] /
    /// [`NoKind::Faulted`] on the terms every kind's own construction states.
    pub fn named(kind: &str, script: Arc<dyn IScriptEngine>) -> Result<Self, NoKind> {
        match kind {
            Self::DEBT => Self::debt(script),
            Self::UNCLAIMED => Self::unclaimed(script),
            _ => Err(NoKind::Unknown(kind.to_string())),
        }
    }

    /// WHICH KIND THIS IS, in the word [`named`](Self::named) takes — what a run reports it started
    /// under, so *which document decided this* is answerable from the run rather than from a build.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.named
    }

    /// **THE DEBT-REPAYMENT KIND** — `debt_loop.scxml`, this repository's own.
    ///
    /// ⚠ The machine is initialised and never stepped. Its one state is final on entry: a kind is a
    /// datamodel, and initialising is what evaluates the `<data>` expressions into the session this
    /// then reads.
    ///
    /// # Errors
    ///
    /// [`NoKind::NoDatamodel`] when the document opened no script session, and
    /// [`NoKind::Faulted`] when initialising it raised an error the document has no state to answer
    /// — see [`crate::document::opened`], which is the road every driven document is initialised
    /// through and the only party that can answer for this one.
    pub fn debt(script: Arc<dyn IScriptEngine>) -> Result<Self, NoKind> {
        // ⚠ A KIND DECLARES NO ACT — it is a datamodel a driver reads, and its one state is final
        // on entry — so what goes through the door is a host that is asked for nothing. It is still
        // REGISTERED rather than skipped: a `<send type="x-sprag-host">` added to this document
        // later must meet a host that refuses it by name, not the engine's bare `error.execution`.
        let machine = crate::document::opened(
            crate::sm::debt_loop::DebtLoopPolicy::new(Arc::clone(&script)),
            &crate::act::Serving::new(),
        )
        .map_err(NoKind::Faulted)?;
        Self::over(Box::new(machine), Self::DEBT, script)
    }

    /// 🎯🎯🎯🎯🎯 **THE KIND NOBODY HAS CLAIMED** — `unclaimed_loop.scxml`, and the second legal
    /// answer that makes *which kind* a question at all (register item 848).
    ///
    /// ⚠⚠ It holds NO decisions, so a run started under it gets the template's own values and the
    /// caller's own arguments and nothing else — in particular no checker, which is what makes item
    /// 847's *nobody classified this* a true sentence about a real run instead of about a fixture.
    ///
    /// # Errors
    ///
    /// [`debt`](Self::debt)'s exactly, about this document.
    pub fn unclaimed(script: Arc<dyn IScriptEngine>) -> Result<Self, NoKind> {
        let machine = crate::document::opened(
            crate::sm::unclaimed_loop::UnclaimedLoopPolicy::new(Arc::clone(&script)),
            &crate::act::Serving::new(),
        )
        .map_err(NoKind::Faulted)?;
        Self::over(Box::new(machine), Self::UNCLAIMED, script)
    }

    /// The half every kind's constructor shares: take the session the document evaluated its
    /// clauses into, or refuse a document that opened none.
    fn over(
        machine: Box<dyn KindDocument + Send>,
        named: &'static str,
        script: Arc<dyn IScriptEngine>,
    ) -> Result<Self, NoKind> {
        let session = machine.session_id().ok_or(NoKind::NoDatamodel)?;
        Ok(Self {
            machine,
            named,
            script,
            session,
        })
    }

    /// **WHICH DIALOGS A RUN OF THIS KIND MAY ANSWER**, or [`None`] for a kind that answers none.
    ///
    /// Read through `OuterLoop::consents_in` — the template's own reader — because a kind and a
    /// template that disagreed about what a clause IS would be two spellings of one rule, and this
    /// workspace has already recorded what a rule with two copies costs once they drift.
    ///
    /// # Errors
    ///
    /// [`NotScreenable`] when the document holds something this driver cannot read as a clause list.
    pub fn consents(&self) -> Result<Option<Consents>, NotScreenable> {
        OuterLoop::consents_in(&self.script, &self.session)
    }

    /// **WHAT A RUN OF THIS KIND TURNS DOWN, AND WHAT IT SAYS INSTEAD**, or [`None`] for a kind that
    /// screens nothing. [`consents`](Self::consents)' reader, one door along.
    ///
    /// # Errors
    ///
    /// [`NotScreenable`] when the document holds something this driver cannot read as a rule list.
    pub fn screen_rules(&self) -> Result<Option<ScreenRules>, NotScreenable> {
        OuterLoop::rules_in(&self.script, &self.session)
    }

    /// **THE TEXT THIS KIND ADDS TO ITS CLOSING QUESTION**, or [`None`] for a kind that adds none.
    ///
    /// ⚠ Read as a plain string rather than through a list reader, because it is APPENDED to a
    /// sentence the template owns: a kind may extend what its runs are asked at the end, and cannot
    /// replace it. That asymmetry is the whole shape — the template keeps the account it needs from
    /// every ending, the repository adds what only it can know it wants.
    #[must_use]
    pub fn closing_rules(&self) -> Option<String> {
        // ⚠ EMPTY READS AS NOTHING, which the generated accessor does not decide: the template
        // ships `''` for the slots a kind may fill, so *declared but empty* is *this document adds
        // nothing*. That polarity belongs to this reader, not to codegen.
        self.machine.closing_rules().filter(|said| !said.is_empty())
    }

    /// **THE RULES EVERY SESSION OF THIS KIND WORKS UNDER**, or [`None`] for a kind that holds its
    /// runs to none.
    ///
    /// # ⚠⚠⚠⚠⚠ Register item 738, and what it was measured against
    ///
    /// [`closing_rules`](Self::closing_rules)' opening bracket, on exactly its terms: one is what a
    /// repository asks of an ENDING, this is what it asks of every turn before that, and neither
    /// belongs in a file other repositories copy. It has no wire key for the same reason that one
    /// does not — *a caller who could override it could delete it by naming nothing*.
    ///
    /// ⚠⚠ **WHAT WAS HAPPENING INSTEAD IS THE MEASUREMENT.** This repository's supervisor typed ten
    /// standing rules into `north_star` BY HAND on every launch, out of that session's context, and
    /// when the session ended they existed nowhere — so the next launch retyped them, slightly
    /// differently. **The more conscientious the supervisor, the larger the copy**, which is why
    /// this is a defect rather than an inconvenience.
    ///
    /// ⚠ It is NOT the template's `standing`, one letter away and a different thing: that one is
    /// empty until a screen rule FIRES and accumulates what a run was redirected to mid-flight. See
    /// the template's `start_prompt`, where the two are composed side by side and the reason is
    /// written down.
    #[must_use]
    pub fn working_rules(&self) -> Option<String> {
        // ⚠ EMPTY READS AS NOTHING, on `closing_rules`' own polarity: the template ships `''` for
        // the slots a kind may fill, so *declared but empty* is *this document holds its runs to
        // nothing*.
        self.machine.working_rules().filter(|said| !said.is_empty())
    }

    /// ⛔⛔⛔⛔⛔ **WHAT THIS KIND DOES ABOUT A CHECKER THAT SAID NOTHING READABLE** — register item
    /// 741, or [`None`] where this document says nothing about either silence.
    ///
    /// # ⚠⚠⚠⚠ Why the pair comes back together or not at all
    ///
    /// Because they are one decision and the run meets exactly one of them: which clause it needs
    /// is [`crate::judge::Silence`]'s answer, and a kind that authored *ask again* while leaving
    /// *fix the prompt* empty would answer 4 of this repository's 19 measured silences and say
    /// nothing about the other 15. Returning a half-filled pair would make that a value rather than
    /// the omission it is — so a document that fills one and not the other is REFUSED here, and the
    /// caller's own refusal names which clause is missing.
    ///
    /// ⚠ EMPTY READS AS NOTHING on [`working_rules`](Self::working_rules)' polarity: the template
    /// ships `''` for both, so *declared and empty* is *this document says nothing about its
    /// checker's silences*, which is the shipped behaviour and an honest answer.
    ///
    /// # Errors
    ///
    /// [`NotScreenable`] when the document holds exactly one of the two, naming the empty one — a
    /// half-authored decision is the state this type exists to make unrepresentable.
    pub fn unverified_rules(&self) -> Result<Option<crate::outer::UnverifiedRules>, NotScreenable> {
        let policy = &self.machine;
        let unanswered = policy.unanswered_rule().filter(|said| !said.is_empty());
        let unreadable = policy.unreadable_rule().filter(|said| !said.is_empty());
        // ⚠⚠⚠ THE THIRD, REGISTER ITEM 752 — see [`crate::judge::Silence::Unwell`]. It joins the
        // all-or-nothing rule rather than defaulting: a kind that says what to do about a checker
        // that misphrased and nothing about one that was stopped before it judged would leave the
        // commonest interruption in an unattended loop's life answered by an empty sentence.
        let unwell = policy.unwell_rule().filter(|said| !said.is_empty());
        match (unanswered, unreadable, unwell) {
            (None, None, None) => Ok(None),
            (Some(unanswered), Some(unreadable), Some(unwell)) => {
                Ok(Some(crate::outer::UnverifiedRules {
                    unanswered,
                    unreadable,
                    unwell,
                }))
            }
            // ⚠⚠ THE HALF-AUTHORED DOCUMENT IS A RED AND NOT A DEFAULT — this workspace's rule that
            // an unclassified thing does not pass. The empty one is NAMED, because *one of your
            // clauses is missing* sends an author to a file and *your rules were ignored* sends
            // them nowhere.
            (None, _, _) => Err(NotScreenable::Missing("unanswered_rule")),
            (_, None, _) => Err(NotScreenable::Missing("unreadable_rule")),
            (_, _, None) => Err(NotScreenable::Missing("unwell_rule")),
        }
    }

    /// **WHERE A RUN OF THIS KIND STARTS READING**, or [`None`] where this kind names nothing and
    /// the caller's own reference is the only one.
    ///
    /// # ⚠⚠⚠⚠ Why the fall-through stops HERE rather than reaching the template
    ///
    /// Register item 738. The template ships `'(edit me) paths, URLs or repos to consult'`, and that
    /// placeholder is not a friendly default — R380 measured a live agent reading three of its five
    /// clauses as `(edit me)`, because a part nobody filled in is composed into the prompt exactly
    /// as written. So a run whose caller named none and whose kind authors none is REFUSED at the
    /// door naming the key: not starting is a better ending than briefing an agent with an
    /// instruction to edit a file.
    ///
    /// ⚠⚠ **AND THIS ONE IS OVERRIDABLE WHERE [`working_rules`](Self::working_rules) IS NOT**, which
    /// is the asymmetry rather than an inconsistency. The rules are what this repository holds its
    /// runs to; the reference is what a session reads FIRST, and `reflecting` rewrites it every time
    /// it replaces a session — *what the last session had to work out*. A value a reflection may
    /// rewrite mid-run is not one a document can own outright; what it can own is where a run that
    /// has learnt nothing yet begins.
    #[must_use]
    pub fn reference(&self) -> Option<String> {
        self.machine.reference().filter(|said| !said.is_empty())
    }

    /// **WHAT MAKES A PANE READY FOR THIS KIND'S FIRST PROMPT**, or [`None`] for a kind that names
    /// no peer of its own.
    ///
    /// # ⚠⚠⚠⚠⚠ The one clause of a kind that does NOT travel through the datamodel
    ///
    /// Register item 738's third layer. Item 300 drew the line every other reader here sits on the
    /// far side of: what makes a pane ready is a PREDICATE ABOUT THE PEER and is the same for every
    /// run against it, while how long anybody waits for it is a judgement. Predicates ride on
    /// [`AiLoopSpec`](crate::AiLoopSpec); judgements are written into `<data>`. **This changes who
    /// AUTHORS the predicate, not which side of that line it is on** — which is why there is no
    /// `<data id="ready_when">` in the template and no `<assign>` for it, and why the value is read
    /// straight off this document's own session.
    ///
    /// ⚠⚠⚠ **IT IS NOT A SECOND AUTHOR OF THE AGENT'S NAME.** A caller who names an `agent` still
    /// gets the barrier DERIVED from it by [`AiLoopSpec::driving`](crate::AiLoopSpec::driving),
    /// exactly as before; this is what a launch that named NO agent gets. The order the door
    /// resolves in is *what the caller spelled, then what the caller implied by naming a program,
    /// then this* — so a run driving `codex` gets `codex`, because a caller saying so is more
    /// specific than a document's standing default.
    ///
    /// ⚠⚠ **AND THE WORD IS PARSED BY [`ReadyWhen::parse`](crate::ReadyWhen::parse), NOT BY A
    /// MATCH WRITTEN HERE.** That is
    /// the one author of which words exist and of *an empty marker is refused*; a second one in this
    /// file is how a document comes to be admitted that the wire would turn down.
    ///
    /// # Errors
    ///
    /// [`NotScreenable::Unreadable`] when the document holds something that is not a barrier this
    /// driver can carry out — a bare string, a missing `match`, a word outside
    /// [`ReadyWhen::WIRE_WORDS`](crate::ReadyWhen::WIRE_WORDS). ⚠ A kind that declares nothing at
    /// all is [`None`] and not an
    /// error: naming no peer is a legitimate thing for a kind to do, and the caller's `agent` is
    /// then the only answer.
    pub fn ready_when(&self) -> Result<Option<crate::ReadyWhen>, NotScreenable> {
        use crate::ReadyWhen;

        let Ok(held) = self.script.get_variable(&self.session, "ready_when") else {
            return Err(NotScreenable::Unreadable);
        };
        let fields = match held {
            ScriptValue::Object(fields) => fields,
            ScriptValue::Null | ScriptValue::Undefined => return Ok(None),
            _ => return Err(NotScreenable::Unreadable),
        };
        let text_of = |key: &str| match fields.get(key) {
            Some(ScriptValue::String(held)) => Some(held.clone()),
            _ => None,
        };
        let (Some(matched), Some(marker)) = (
            text_of(ReadyWhen::MATCH_KEY),
            text_of(ReadyWhen::MARKER_KEY),
        ) else {
            return Err(NotScreenable::Unreadable);
        };
        ReadyWhen::parse(&matched, marker)
            .ok_or(NotScreenable::Unreadable)
            .map(Some)
    }

    /// **THE NUMBERS THIS KIND AUTHORS UNDER `id`**, as the document spells them — or [`None`]
    /// where this kind declares no such clause.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this hands back a MAP where every other reader here hands back a decision
    ///
    /// Register item 738, layer 1. The clause it exists for is `guardrails`, and its field names
    /// are **the wire's**, published by `PluginGrammar::guardrail_fields` one crate up — which this
    /// crate cannot see and must not guess at. A reader here that named `max_bytes` and its two
    /// neighbours would be a second author of a set the wire already publishes, and the day a
    /// fourth guardrail is added the two spellings would drift with nothing saying so.
    ///
    /// So the split is: **this crate reads the SHAPE, the crate that owns the vocabulary reads the
    /// MEANING.** A key no guardrail admits is refused up there, naming what the object takes —
    /// which is the same refusal `parse_guardrails` already makes of a caller, applied to a
    /// document. ⚠ An unclassified key must never be a pass: a bound silently dropped is a run
    /// with no bound at all, and it answers success.
    ///
    /// ⚠⚠ **NUMBERS ONLY, AND A NON-NUMBER IS AN ERROR RATHER THAN A SKIP.** A clause a person
    /// edits is a clause a person mistypes, and a value this reader could not carry would otherwise
    /// leave the caller with the daemon's default while the document plainly names something else
    /// — the exact shape item 492 measured on `context_ceiling`.
    ///
    /// # Errors
    ///
    /// [`NotScreenable::Unreadable`] when the id holds something that is not an object of numbers.
    pub fn authored_numbers(
        &self,
        id: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, i64>>, NotScreenable> {
        let Ok(held) = self.script.get_variable(&self.session, id) else {
            return Err(NotScreenable::Unreadable);
        };
        let fields = match held {
            ScriptValue::Object(fields) => fields,
            ScriptValue::Null | ScriptValue::Undefined => return Ok(None),
            _ => return Err(NotScreenable::Unreadable),
        };
        let mut read = std::collections::BTreeMap::new();
        for (name, value) in &fields {
            let number = match value {
                ScriptValue::Int(held) => *held,
                // ⚠ A script datamodel holds a bare integer literal as a double on some engines,
                // so refusing one here would refuse a document that is written correctly.
                ScriptValue::Double(held) if held.fract() == 0.0 => *held as i64,
                _ => return Err(NotScreenable::Unreadable),
            };
            read.insert(name.clone(), number);
        }
        Ok(Some(read))
    }

    /// **HOW LONG A PERSON MAY HOLD A RUN OF THIS KIND**, in milliseconds — or [`None`] where this
    /// kind says nothing and the template's own number stands.
    ///
    /// # ⚠⚠⚠⚠ Register item 738, layer 1, and it is here because the GATE asked
    ///
    /// [`crate::driver::Ceiling`] names five things that can end a run, and this item's gate walks
    /// that set and asks this document for the bound each one fires on — with **no exemption arm**,
    /// because a ceiling nobody classified is a red rather than a pass. Four of the five were
    /// reachable by a kind once the guardrails were; `Hold` was the fifth, and leaving it to the
    /// template would have been the escape hatch that disarms the gate.
    ///
    /// ⚠⚠ It reads as a plain number on [`context_ceiling`](Self::context_ceiling)'s terms: there
    /// is already a spelling for *decline* (declare nothing) and a second one would be two ways to
    /// say one thing. ⚠ Zero is refused at the door, where `hold_within_ms`'s own rule lives —
    /// *hold this run and end it at once* is `cancel` spelled wrong.
    #[must_use]
    pub fn hold_within_ms(&self) -> Option<i64> {
        self.machine.hold_within_ms()
    }

    /// **WHERE A RUN OF THIS KIND WORKS**, or [`None`] for a kind that does not care where its
    /// pane stands — register item 738, layer 4.
    ///
    /// # ⛔⛔⛔⛔⛔ What it costs when nobody says it
    ///
    /// A pane opened without a directory stands in `$HOME`, and an agent starting there asks *"Is
    /// this a project you created or one you trust?"* — a dialog this loop cannot answer, because
    /// its consents cover editing and running commands and nothing else. Measured 2026-08-25: one
    /// pane `blocked rule=dialog-choice-list` in `/home/coin` while three siblings standing in
    /// their own repositories were `working`, all from the same restart.
    ///
    /// ⛔ **AND THE OBVIOUS REPAIR IS THE WRONG ONE.** Adding that dialog to the consents automates
    /// a FALSE answer: *yes, I trust this folder* consents to every repository on the machine,
    /// while the true fact is narrower — *this pane is not standing in the tree this run is about*.
    /// Saying the true thing needs the tree to be written down, which is what this clause is.
    ///
    /// ⚠⚠ **SO IT BUYS A REFUSAL RATHER THAN AN ANSWER**, taken at the door where somebody is still
    /// watching. Item 684's whole cost was a run already started, waiting for a person who was not
    /// there. ⚠ [`None`] is the shipped state for a kind that says nothing, which is right for a
    /// document other repositories copy — and it means no check at all rather than a check that
    /// passes.
    #[must_use]
    pub fn works_in(&self) -> Option<String> {
        self.machine.works_in().filter(|said| !said.is_empty())
    }

    /// **WHICH WINDOW A RUN OF THIS KIND STANDS IN**, or [`None`] for a kind that does not care
    /// whose screen its pane appears on — register item 754, and [`works_in`](Self::works_in)'s
    /// other half.
    ///
    /// # ⛔⛔⛔⛔⛔ What it costs when nobody says it
    ///
    /// A request that narrows no window acts in its session's CURRENT one, so a pane is born
    /// wherever a person is looking. Measured 2026-08-29 off a live daemon, by the owner reading
    /// the screen: this repository's loop pane `inner750` was in the `pinion` window, between
    /// `outer-pinion` and `inner-pinion`. Its watcher had called `split-window` more than ten times
    /// that day naming no window, and the panes had gone to three different windows — every call
    /// succeeding, because from the daemon's side every one of them had.
    ///
    /// ⛔ **AND THE OBVIOUS REPAIR IS THE WRONG ONE**, refused by the owner in words: *a watcher
    /// that remembers to name the window* is a rule held in whoever is on shift, which is what item
    /// 738 exists to end. The birth side is answered structurally instead (`split-window -w`, and a
    /// bare call standing where its CALLER stands); this clause is what NOTICES when that is
    /// bypassed, at the door, which is the last moment anybody is watching.
    ///
    /// ⚠⚠ **IT IS A PREDICATE AND NOT A WINDOW NAME**, on `works_in`'s measured reason: the same
    /// document is compiled into every checkout, and the four repositories sharing this daemon name
    /// their windows nothing like their trees. What is portable is *a window belongs to one tree* —
    /// see the document for the symptom and the four controls that predicate separates.
    ///
    /// ⚠ [`None`] is the shipped state for a kind that says nothing, which is right for a document
    /// other repositories copy — and it means no check at all rather than a check that passes.
    #[must_use]
    pub fn stands_in(&self) -> Option<String> {
        self.machine.stands_in().filter(|said| !said.is_empty())
    }

    /// **WHICH DIMENSION A RUN OF THIS KIND MUST KEEP WHOLE**, or [`None`] for a kind that does not
    /// care how its pane was divided — register item 772, and [`stands_in`](Self::stands_in)'s
    /// sibling one axis in.
    ///
    /// # ⛔⛔⛔⛔⛔ What it costs when nobody says it
    ///
    /// The owner asked it, reading the screen: *"어떤건 세로로 split 됐고 어떤건 가로로 split 되어
    /// 있어, 결정론적이여야되는거아니야?"* Measured the same night across four windows of one daemon,
    /// four loops of this kind: three were `left|right` (inner pane **73 rows** × 168 cols) and one
    /// was `top|bottom` (**36 rows** × 338). Item 765's budget is counted in ROWS — a reply's first
    /// row is its label, and the first thing scrolling takes — so one of the four had **half the
    /// budget of the others** and nobody had decided that.
    ///
    /// ⛔ **AND THE ROOT WAS NOT A HABIT.** That watcher's launch procedure carried
    /// `split-window -v` frozen into it while the other three carried `-h`, and the skill spells
    /// `-h` as an example without ever saying why. Telling a watcher *use `-h`* does not reach a
    /// brief that already says `-v` — which is exactly why the decision is the document's (item
    /// 738's conclusion, item 754's shape).
    ///
    /// ⚠⚠ **IT IS A DIMENSION AND NOT A DIRECTION**, on `stands_in`'s measured reason: the same
    /// document is compiled into every checkout, and `-h` is one launcher's grammar. *This kind
    /// keeps its rows* is the fact behind it — true of a run driven by any surface, and the one a
    /// door can check against a pane it did not open.
    ///
    /// ⚠ [`None`] is the shipped state for a kind that says nothing, and it means **no check at
    /// all** rather than a check that passes.
    #[must_use]
    pub fn keeps(&self) -> Option<String> {
        self.machine.keeps().filter(|said| !said.is_empty())
    }

    /// **HOW MANY TURNS A RUN OF THIS KIND MAY TAKE**, or [`None`] where this kind says nothing and
    /// the template's own number stands.
    ///
    /// ⚠⚠ [`Counted::Never`] is a DECISION and not an absence — a debt run's job is a list nobody
    /// has finished, so it ends on its work rather than on a count. See this kind's document for
    /// what that costs and why the spelling is a word.
    /// ⚠⚠⚠⚠ **AND THIS ONE KEEPS THE INTERPRETING READER while its neighbours moved to the
    /// generated accessors** — SCE PR-86's R-86.4, consumed 2026-08-20 where it fits and not where
    /// it does not.
    ///
    /// The codegen emits `pub fn max_turns(&self) -> Option<String>` for THIS document, because
    /// this document authors the word. [`Counted`] is a UNION — a word or a number — and no
    /// accessor typed from one document's literal can carry it: a kind that wrote `7` would get an
    /// `Option<i64>` accessor and this call would stop compiling. The others moved precisely
    /// because their contract IS one type; this one's contract is the union, so the reader that
    /// implements the union stays.
    #[must_use]
    pub fn turn_budget(&self) -> Option<Counted> {
        OuterLoop::authored_count_in(&self.script, &self.session, "max_turns")
    }

    /// **HOW OFTEN A RUN OF THIS KIND STOPS TO IMPROVE ITSELF**, or [`None`] where it says nothing.
    ///
    /// ⚠⚠⚠ A kind that declines the turn budget MUST answer this, and the driver refuses the pair
    /// rather than guessing: the template's default for reflection is *the number that makes the
    /// reflect guard unreachable*, which only exists while there IS a budget. Declining one without
    /// naming a cadence asks for a loop that runs for ever and never improves itself.
    #[must_use]
    pub fn reflect_every(&self) -> Option<i64> {
        self.machine.reflect_every()
    }

    /// **HOW MUCH A SESSION OF THIS KIND MAY HAVE READ** before the next milestone is taken in a
    /// fresh one — or [`None`] where this kind says nothing and the template's own number stands.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this reader had to exist, measured rather than argued
    ///
    /// Register item 492. The template's own comment has always said *"it is the KIND's to author,
    /// like `max_turns` and `reflect_every`"* — and those two had a reader here while this one had
    /// none, so **the sentence named a channel that did not exist**. What that cost is item 477's
    /// measurement: on a live run at 97 iterations, every one of the eight `reviewing` exits took
    /// the fall-back, because `context_ceiling` was 0 and nothing anywhere could make it anything
    /// else. **The one decision that state exists to make had never been reachable.**
    ///
    /// ⚠⚠ **ZERO IS A DECISION HERE, NOT AN ABSENCE**, and it is the same zero the template
    /// documents: *no ceiling was authored, so every reflection replaces*. A kind that means that
    /// writes `0`; a kind that has not thought about capacity declares nothing and inherits the
    /// template's, which is the same behaviour by a different road. ⚠ That is why this reads as a
    /// plain number and not as a [`Counted`]: there is already a spelling for *decline*, and adding
    /// the word `never` beside it would be two spellings of one decision.
    #[must_use]
    pub fn context_ceiling(&self) -> Option<i64> {
        self.machine.context_ceiling()
    }

    /// **HOW MANY TIMES IN A ROW A CHECK MAY REFUSE A RUN OF THIS KIND'S CLAIM** before it stops
    /// buying turns and reflects — or [`None`] where this kind says nothing and the template's own
    /// number stands.
    ///
    /// # ⚠⚠⚠⚠⚠ It is [`context_ceiling`](Self::context_ceiling)'s twin, found by sweeping the class
    ///
    /// Register item 494. The template says *"IT IS THE KIND'S TO AUTHOR, like `max_turns` and
    /// `reflect_every`"* about exactly TWO of its numbers; item 492 measured the instance and built
    /// the road for one of them. Sweeping the sentence instead — which is what 492 should have
    /// started with — found the identical defect still standing one `<data>` up. **A premise that
    /// produces one defect produces the rest of its class**, and the class is what a ratchet can
    /// close: `crates/sprag-gate/src/authored.rs` derives the claimed ids from that sentence and
    /// refuses any this type cannot read, so the third one nobody has written yet cannot happen
    /// quietly.
    ///
    /// ⚠⚠ **WHAT THE MISSING CHANNEL COST IS AN ARGUMENT A KIND COULD NOT MAKE.** Item 449 authored
    /// the shipped `3` while refusals were MUTE — the agent was not told it had been refused at all
    /// — so three was three turns spent answering a question nobody had asked. Item 448 gave every
    /// refusal the check's own words, and the template's own comment draws the consequence: *"a kind
    /// that finds it slack now has a fact it did not have then"*. It had no way to act on it.
    ///
    /// ⚠ **ZERO IS A DECISION HERE**, and it is the template's own reading of it: a reflection on
    /// the first refusal, *"a choice this document allows and does not recommend"*. So this reads as
    /// a plain number for [`context_ceiling`](Self::context_ceiling)'s reason — a decline already
    /// has a spelling and a second one would be two ways to say the same thing.
    #[must_use]
    pub fn reflect_after_refusals(&self) -> Option<i64> {
        self.machine.reflect_after_refusals()
    }

    /// 🎯🎯🎯🎯🎯 **HOW FAR A RUN OF THIS KIND MAY RE-AIM ITSELF AWAY FROM THE CHECKPOINT IT WAS
    /// GIVEN** — or [`None`] where this kind says nothing and the template's own number stands. The
    /// owner's decision of 2026-09-02, register item 833(2).
    ///
    /// # ⚠⚠⚠⚠⚠ Why a KIND is the party that decides this, and not a caller or the template
    ///
    /// It is a rule about **how work is done in a particular repository**. A template other
    /// repositories copy cannot know whether a run that finds a second thing while paying the first
    /// should take it — that depends on what the debt looks like where it is running. And a CALLER
    /// must not decide it, on `milestone_check`'s argument exactly: one who could name it could
    /// delete the cap by spelling `never` on a launch nobody reviewed, so there is no wire key.
    ///
    /// ⚠⚠ **WHAT ITS ABSENCE COST, MEASURED.** With no cap at all, this repository's loop closed
    /// eleven register items in twenty-two commits on 2026-09-02 and **nine of the eleven had been
    /// registered the same day**, while the forty-one items standing that morning lost exactly one.
    /// The population went UP, 41 to 50 — a loop paying its own debt for ever, which is the report
    /// the owner's decision answers.
    ///
    /// ⚠ **IT READS THROUGH THE INTERPRETING READER, like [`turn_budget`](Self::turn_budget) and
    /// unlike the three above it.** [`Counted`] is a UNION — a number, or the word `never` — and no
    /// accessor typed from one document's literal can carry it: a kind that wrote `1` would get an
    /// `Option<i64>` accessor and a kind that wrote `'never'` an `Option<String>`, so a reader
    /// typed off the codegen would stop compiling the day a repository changed its mind.
    #[must_use]
    pub fn reaim_max(&self) -> Option<Counted> {
        OuterLoop::authored_count_in(&self.script, &self.session, "reaim_max")
    }

    /// **WHO DECIDES A MILESTONE OF THIS KIND WAS REACHED**, as an argv — or [`None`] where this
    /// kind says nothing and the working agent's own word stands.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a kind must be able to say this, and could not until now
    ///
    /// Register item 428's mechanism shipped with the slot on the TEMPLATE, empty, and empty means
    /// nobody checks — the right default for a document other repositories copy. The item's own
    /// residue said *"no caller can declare one"*, and driving a live debt run found the sharper
    /// version: **no KIND could declare one either**. The template's `''` stood on every run this
    /// repository has ever driven, so every `converged (declared)` meant only *the agent said so* —
    /// measured 2026-08-18, the walk reading *"NOTHING CHECKED THAT CLAIM"* while the kind's own
    /// document authored a checker.
    ///
    /// ⚠⚠ It travels like [`closing_rules`](Self::closing_rules) and NOT like a wire argument, which
    /// keeps 428's own decision: what certifies a repository's work is its document's business, and
    /// a caller that could override it could delete the check by naming nothing.
    ///
    /// ⚠ Whitespace is what the driver splits on, so a check whose argument contains a space cannot
    /// be spelled here. Registered rather than hidden — the question the driver appends is the last
    /// argument, and everything before it is a program and its flags.
    #[must_use]
    pub fn milestone_check(&self) -> Option<String> {
        match self.script.get_variable(&self.session, "milestone_check") {
            Ok(ScriptValue::String(argv)) if !argv.trim().is_empty() => Some(argv),
            _ => None,
        }
    }

    /// 🎯🎯🎯🎯🎯 **WHO DECIDES A CHECKPOINT PROPOSED FOR THIS KIND IS ONE TO TAKE NEXT**, as an
    /// argv — or [`None`] where this kind says nothing and every proposal is taken. Register item
    /// 839.
    ///
    /// # ⛔⛔⛔⛔⛔ The half the cap could not reach
    ///
    /// [`reaim_max`](Self::reaim_max) bounds how MANY times a run may change direction. Nothing
    /// bounded WHERE it could go, so a run with a budget of one could still spend it on anything at
    /// all — and this repository's answer to *what should be worked on next* was **prose in a
    /// prompt fragment**: `working_rules`, appended to a session's first message, read by no `cond`
    /// and held by no gate. Measured 2026-09-02, on the line beneath a rule of its own saying
    /// reasoning written as prose is measured by nobody.
    ///
    /// ⚠⚠⚠ **THE MEANING IS THE KIND'S AND THE MACHINE IS THE TEMPLATE'S**, which is what makes
    /// this a slot rather than a branch anywhere. What may be taken next is a rule about one
    /// repository's own work — for this one, *while anything is ranked most severe, take from
    /// those*, which its own register can be asked — and the template that other repositories copy
    /// may not carry that vocabulary at all.
    ///
    /// ⚠⚠ It travels like [`milestone_check`](Self::milestone_check) and NOT like a wire argument,
    /// on that field's argument and [`reaim_max`](Self::reaim_max)'s: a caller who could name this
    /// could delete the whole bound by naming nothing.
    ///
    /// ⚠ Whitespace is what the driver splits on, exactly as above: the proposal it appends is the
    /// last argument, and everything before it is a program and its flags.
    /// 🎯🎯🎯🎯🎯 **HOW MANY TIMES A RUN OF THIS KIND MAY ASK ITS AGENT AGAIN** when the checkpoint
    /// it finished is done and the successor it named was turned away — or [`None`] where this kind
    /// says nothing and the template's own number stands. Register item 840.
    ///
    /// # ⛔⛔⛔⛔⛔ What it is bounding, and why the bound must exist
    ///
    /// A reflection whose checkpoint was REACHED cannot go back to work, so a refused proposal used
    /// to END the run — and because the re-aiming budget counted CHANGES rather than DEPTH, a
    /// capped run could not move to an unrelated next thing either. The owner's answer is that it
    /// asks its agent again, carrying the refusal's own words; **the machine choosing the next
    /// checkpoint was refused**, on register item 659's measurement that a loop always chasing the
    /// sharpest thing never finishes an axis.
    ///
    /// ⚠⚠ Asking again without a bound is a livelock — an agent that keeps naming refused things
    /// turns for ever — the `REASK_MAX` constant's own doc names the register item, and is NAMED rather
    /// than linked because it is private. At this bound
    /// the run closes, which banks the work.
    ///
    /// ⚠ **ZERO IS A DECISION A KIND MAY MEAN**: never ask again, close on the first refusal, which
    /// is what every run did before this existed. Read as a plain number for
    /// [`reflect_after_refusals`](Self::reflect_after_refusals)' reason — and **unbounded has no
    /// spelling here at all**, because unbounded is the failure rather than a choice.
    #[must_use]
    pub fn reask_max(&self) -> Option<i64> {
        OuterLoop::authored_number_in(&self.script, &self.session, "reask_max")
    }

    #[must_use]
    pub fn successor_check(&self) -> Option<String> {
        match self.script.get_variable(&self.session, "successor_check") {
            Ok(ScriptValue::String(argv)) if !argv.trim().is_empty() => Some(argv),
            _ => None,
        }
    }

    /// **WHAT THIS REPOSITORY'S PEER PRINTS WHEN ITS SERVICE FAILS, AND WHAT TO DO ABOUT IT** — or
    /// [`None`] when this document declines, which is the template's shipped state.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a kind holds this and the template does not
    ///
    /// The sentence a peer prints when it is unwell is that peer's, at one version, in one
    /// language. `screen_rules`' own reason applies unchanged: **a template does not know whose
    /// agent it will be talking to**, and a needle written into one would quote words at
    /// repositories whose runs talk to something else entirely.
    ///
    /// # ⚠⚠⚠ THE NEEDLES DECIDE, AND THE OTHER TWO ONLY SHAPE IT
    ///
    /// A document with no needle has declined however carefully it filled the other two, so this
    /// answers [`None`] on that alone. The wait and the word then fall back to the template's own
    /// values rather than to numbers invented here — a kind that names the failure and says nothing
    /// about the remedy has asked for the default remedy, not for a broken one.
    ///
    /// ⚠⚠ **AND IT IS A SET SINCE REGISTER ITEM 715.** One string could only ever be one sentence,
    /// and that was the whole of why a usage limit — the most predictable stop an unattended loop
    /// meets — could not reach the state built for outages. A blank element is dropped rather than
    /// carried, because [`str::contains`] answers TRUE for the empty string.
    ///
    /// ⚠⚠ A NEGATIVE OR UNREADABLE WAIT IS THE DEFAULT AND NOT AN ERROR, on
    /// [`turn_budget`](Self::turn_budget)'s terms: this is a document a person edits, and the
    /// failure mode worth preventing is a run that spins re-asking a service that is already
    /// refusing. Falling back to ten minutes is the direction to be wrong in.
    #[must_use]
    pub fn service_outage(&self) -> Option<crate::outer::ServiceOutage> {
        let needles = self.service_needles();
        if needles.is_empty() {
            return None;
        }
        let every_ms = match self.script.get_variable(&self.session, "service_retry_ms") {
            Ok(ScriptValue::Int(held)) if held > 0 => u64::try_from(held).ok(),
            _ => None,
        };
        let text = match self
            .script
            .get_variable(&self.session, "service_retry_text")
        {
            Ok(ScriptValue::String(word)) if !word.is_empty() => Some(word),
            _ => None,
        };
        Some(crate::outer::ServiceOutage {
            needles,
            every_ms: every_ms.unwrap_or(crate::outer::DEFAULT_SERVICE_RETRY_MS),
            text: text.unwrap_or_else(|| crate::outer::DEFAULT_SERVICE_RETRY_TEXT.to_owned()),
        })
    }

    /// **THE WORDS THIS KIND'S PEER PRINTS WHEN ITS SERVICE FAILS**, read off its own datamodel —
    /// empty where the document declines, which is the template's shipped state.
    ///
    /// # ⚠⚠⚠⚠ Why it is a list and why a blank element is dropped
    ///
    /// Register item 715. One string could only ever be one sentence, and this repository's peer
    /// says *I am not answering right now* in a family of them — a 529, a usage limit, and whatever
    /// it invents next. ⚠ A blank `says` is DROPPED rather than carried, because
    /// [`str::contains`] answers TRUE for the empty string and one stray element would send every
    /// blocked turn of every run into the wait.
    ///
    /// ⚠⚠ AN UNREADABLE LIST IS AN EMPTY ONE — *this document declines*, which is exactly the
    /// behaviour of every run before item 447 existed. The direction to be wrong in is the one that
    /// cannot type at somebody's agent, which is [`consents`](Self::consents)' own rule seen from
    /// the other side.
    ///
    /// ⚠ The reading itself lives beside `consents_in` and `rules_in`, for the reason those two
    /// state: **the reader of an author's list must be ONE reader**, or a template and a kind can
    /// disagree about what an element is.
    fn service_needles(&self) -> Vec<String> {
        OuterLoop::service_needles_in(&self.script, &self.session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debt() -> LoopKind {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        LoopKind::debt(lua).expect("the debt kind's document must open a script session")
    }

    /// ⚠⚠⚠ **THE DECISIONS THE TEMPLATE GAVE UP ARE HELD HERE, WHOLE** — the other half of the
    /// purity gate, and the half that says the split did not simply lose them.
    ///
    /// A template emptied of its consents and a repository that answers no dialogs are the same
    /// green on the purity gate and opposite outcomes for a run: the document's own comment records
    /// that an empty `may_answer` met `Do you want to make this edit?` on the first milestone and
    /// stood there until an iteration ceiling ended the run. **So emptying the template is only
    /// correct if something else holds them**, and this is that something.
    ///
    /// ⚠ Asserted through the same reader the template's own values go through, so a kind that
    /// crossed the Lua datamodel differently would be caught here rather than at a live dialog.
    #[test]
    fn the_debt_kind_holds_the_consents_a_debt_run_needs() {
        let kind = debt();
        let held = kind
            .consents()
            .expect("the kind's clause list must be readable")
            .expect("a debt run answers dialogs, so the list is not absent");
        assert_eq!(
            held.clauses().len(),
            2,
            "⚠⚠⚠ BOTH CLAUSES OR NEITHER. A run allowed to edit but not to run commands cannot test \
             what it edited, and a milestone this loop calls reached is one it has verified — so one \
             consent is not half a working loop, it is a loop that stops at the other dialog. Got \
             {held:?}",
        );
        for asked in ["Do you want to make this edit", "Bash command"] {
            assert!(
                held.clauses().iter().any(|clause| clause.asked() == asked),
                "the clause about {asked:?} must be here, since the template no longer has it: \
                 {held:?}",
            );
        }
        // ⚠ NEITHER NEEDLE MAY BE `Yes`: both dialogs carry it on two options, and a needle that
        // reaches two authorises neither — measured, and the reason each clause names the text only
        // ONE option carries.
        for clause in held.clauses() {
            assert_ne!(
                clause.answer(),
                "Yes",
                "⚠⚠ `Yes` names two options in both dialogs, so a clause answering it answers \
                 nothing at all: {clause:?}",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **AND EVERY CLAUSE MUST RESOLVE ON A DIALOG THE CURRENT AGENT ACTUALLY DRAWS** —
    /// register item 525, and the gate whose absence cost a live run five hours.
    ///
    /// # What was measured
    ///
    /// `debt_loop.scxml` authorised an edit by naming *"allow all edits during this session"* — text
    /// only one option carried, chosen carefully for exactly that reason. **The current `claude`
    /// build does not carry it at all**: captured 2026-08-21 from a real agent in a real pane
    /// ([`CLAUDE_EDIT_DIALOG_NOW`](crate::testing::CLAUDE_EDIT_DIALOG_NOW)), option 2 now reads
    /// *"Yes, and switch to accept edits (auto-approve file edits and common file commands) for this
    /// session"*. The clause matched NOTHING, and no gate anywhere compared it against a dialog.
    ///
    /// ⚠⚠⚠ **THE GATE THAT EXISTED WAS ABOUT A CLAUSE NOBODY SHIPS.**
    /// `consent::tests::one_clause_covers_every_dialog_a_live_agent_raised_while_working` runs a
    /// HAND-WRITTEN clause against the captures and is a fine test of `covers` — but the clauses
    /// that arm an unattended run are the KIND's, and they went unchecked. *A rule nothing measures
    /// against reality is a rule that stops being true silently.*
    ///
    /// ⚠⚠ It asserts EXACTLY ONE option, which is both halves of the danger: zero is a clause that
    /// authorises nothing (this incident), and two is a clause that authorises neither
    /// ([`Refusal::Ambiguous`](crate::consent::Refusal::Ambiguous), the case the document already
    /// reasoned about). ⚠ Only the dialogs a clause CLAIMS are asked of it — a clause about `Bash
    /// command` has nothing to say about an edit dialog, and demanding otherwise would make the gate
    /// wrong rather than strict.
    #[test]
    fn every_clause_the_kind_authors_resolves_on_a_dialog_this_agent_really_draws() {
        let kind = debt();
        let held = kind
            .consents()
            .expect("the kind's clause list must be readable")
            .expect("a debt run answers dialogs, so the list is not absent");

        let mut claimed = 0_usize;
        for (label, rows) in crate::testing::CLAUDE_DIALOGS_NOW {
            let question = crate::testing::parsed_dialog(rows).unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠ {label}: the SHIPPING parser must read a dialog captured from the agent \
                     this repository runs today — a miss here is the detector going stale against \
                     the program it is written for",
                )
            });
            for clause in held.clauses() {
                if !question
                    .asked
                    .iter()
                    .any(|line| line.contains(clause.asked()))
                {
                    continue;
                }
                claimed += 1;
                let chose = clause.covers(&question).unwrap_or_else(|why| {
                    panic!(
                        "⚠⚠⚠⚠⚠ {label}: THIS KIND'S OWN CLAUSE CLAIMS THIS DIALOG AND RESOLVES TO \
                         NOTHING ({why:?}). An unattended debt run meets this dialog on its first \
                         edit; a clause that answers nothing leaves it standing there — measured \
                         2026-08-21 at five hours and twenty minutes. Re-take the needle against \
                         the capture beside this gate, and if the wording moved again, RE-CAPTURE \
                         rather than guessing. Clause {clause:?}, options {:?}",
                        question
                            .choices
                            .iter()
                            .map(|choice| choice.label.as_str())
                            .collect::<Vec<_>>(),
                    )
                });
                assert!(
                    chose.label.contains(clause.answer()),
                    "the option this clause resolved to must be the one carrying its words: \
                     {chose:?}",
                );
            }
        }
        assert!(
            claimed > 0,
            "⚠⚠⚠⚠ VACUOUS: not one clause claimed one captured dialog, so this gate asserted \
             nothing at all. Either the captures stopped matching what the kind is about, or the \
             kind stopped naming the dialogs it must answer — both are the failure this exists for",
        );
    }

    /// **AND THE STANDING INSTRUCTION, IN THE AUTHOR'S OWN LANGUAGE** — the half that only a
    /// repository can write, which is why it left the template.
    ///
    /// ⚠ The non-ASCII assertion is not decoration. PR-87 was a round in which `sce-rust-lua`
    /// rewrote payloads with `bytes[i] as char` and a person's own language came back mangled with
    /// nothing raised anywhere. This is the same crossing, in a new document — so the new document
    /// gets the same question asked of it rather than inheriting an answer given about another file.
    #[test]
    fn the_debt_kind_holds_its_standing_instruction_in_the_authors_language() {
        let kind = debt();
        let held = kind
            .screen_rules()
            .expect("the kind's rule list must be readable")
            .expect("a debt run screens dialogs, so the list is not absent");
        assert!(
            held.rules()
                .iter()
                .any(|rule| !rule.text().is_ascii() && !rule.text().is_empty()),
            "⚠⚠⚠ a reply in the author's OWN LANGUAGE must survive the crossing into this document \
             too — PR-87 is what happens when it does not, and it fails silently: {held:?}",
        );
        assert!(
            held.rules()
                .iter()
                .all(|rule| !rule.when().is_empty() && !rule.text().is_empty()),
            "a rule missing either half claims nothing or says nothing: {held:?}",
        );
    }

    /// ⚠⚠⚠ **WHAT THIS KIND'S NEEDLE MAY AND MAY NOT SWALLOW** — moved here from the template's own
    /// gate, because the template no longer ships a needle and this is where one lives now.
    ///
    /// # Why it is asserted at all, and against SCREEN LINES
    ///
    /// The widening that reads best — `Do you want to`, the whole family — was written, run, and
    /// **MEASURED WRONG**: it claims the COMMAND dialog too, so a loop carrying it is refused every
    /// `cargo test` it asks for and told to think again. A loop that may edit but may not run cannot
    /// verify a milestone it claims to have reached. Running is a consent's job, one door up.
    ///
    /// ⚠ The two it must not claim are spelled as the LINES the peers actually raise, so a reworded
    /// product breaks this rather than passing quietly. And the check runs both ways: a needle that
    /// stopped claiming the dialog it exists for would leave this kind screening nothing while
    /// looking configured.
    ///
    /// ⚠⚠ **THIS KIND SHIPS BOTH HALVES, SO THE TWO MUST NOT OVERLAP.** A rule reaching a dialog its
    /// own `may_answer` answers is this document arguing with itself — and the loser is the run,
    /// which gets told to reconsider every tool call it makes. Nothing but this asks whether the two
    /// lists in one file agree.
    #[test]
    fn this_kinds_needle_claims_its_own_dialog_and_none_a_consent_answers() {
        let kind = debt();
        let rules = kind.screen_rules().expect("readable").expect("present");
        let consents = kind.consents().expect("readable").expect("present");

        for rule in rules.rules() {
            for spoken_for in ["Do you want to proceed?", "Do you want to make this edit?"] {
                assert!(
                    !spoken_for.contains(rule.when()),
                    "⚠⚠⚠ A STANDING INSTRUCTION MUST NOT CLAIM A DIALOG A CONSENT ANSWERS. {:?} is \
                     carried by {spoken_for:?} — the command and edit dialogs are `may_answer`'s, \
                     and a rule reaching one turns a loop that works into a loop that is told to \
                     think again about every tool call it makes",
                    rule.when(),
                );
            }
            // ⚠ AND THE SAME QUESTION ASKED OF THIS FILE'S OWN OTHER LIST, which is the half no
            // gate could ask while the two lived in different documents.
            for clause in consents.clauses() {
                assert!(
                    !clause.asked().contains(rule.when()),
                    "⚠⚠⚠ THIS KIND REFUSES A DIALOG IT ALSO AUTHORISES. {:?} reaches the consent \
                     for {:?}; one of the two is wrong and the run pays for it every turn",
                    rule.when(),
                    clause.asked(),
                );
            }
        }
        assert!(
            rules
                .rules()
                .iter()
                .any(|rule| "Do you want to create PROBE.txt?".contains(rule.when())),
            "⚠⚠ and it must still claim the one left to it — the file that does not exist yet, which \
             no quote can tell from a design decision (`judged_rules`' argument): {rules:?}",
        );
    }

    /// ⚠⚠⚠⚠ **THIS KIND DOES NOT END ON TURNS, AND THEREFORE MUST NAME A CADENCE.**
    ///
    /// # Why the pair is one gate and not two
    ///
    /// The template's default for reflection is *the number that makes the reflect guard
    /// unreachable* — it borrows `max_turns`, because `judging` tests the budget first. That number
    /// only exists while there IS a budget. A kind that declines one and says nothing about
    /// reflection is therefore asking for a loop that runs for ever and never once stops to improve
    /// itself, and `OuterLoop::brief` refuses that pairing rather than guessing at it.
    ///
    /// **So the decline is only safe while the cadence is beside it**, and holding them apart would
    /// let either be deleted with the other still green.
    ///
    /// ⚠⚠⚠ THE DECLINE IS A WORD, and that is not style. `probe_absent` measured the alternatives:
    /// an id declared and left EMPTY reads `Ok(Null)` — and so does an id nobody declares at all —
    /// so a kind that FORGOT this key would be granted an unbounded run. A boolean has the same
    /// disease, absent and `false` being equally falsy. Only a value that is neither a number nor
    /// nil can carry a decision between documents.
    #[test]
    fn the_debt_kind_declines_the_turn_budget_and_names_a_reflection_cadence() {
        let kind = debt();
        assert_eq!(
            kind.turn_budget(),
            Some(Counted::Never),
            "⚠⚠⚠⚠ a debt run's job is a list nobody has finished, so it ends on its WORK — the north \
             star declared, a person standing it down, a guardrail — and not on a count. Two live \
             runs ended `exhausted (turns)` mid-milestone, and what the document said about it was \
             true and useless: a sentence about a number, never about the work",
        );
        let cadence = kind.reflect_every().expect(
            "⚠⚠⚠⚠ AND A KIND THAT DECLINES THE BUDGET MUST SAY THIS. Without it the template would \
             borrow a default from a budget that is not there, which is a loop that never improves \
             itself — the driver refuses the pair, so a run of this kind would not start at all",
        );
        assert!(
            cadence >= 1,
            "a cadence of zero or less reflects on every judgement or on none: {cadence}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND IT NAMES A CAPACITY CEILING, WHICH NOTHING READ UNTIL REGISTER ITEM 492** —
    /// the sharpest shape a missing channel takes: **a number authored, with a dated measurement
    /// and three paragraphs of reasoning, that no reader existed for.**
    ///
    /// # ⚠⚠⚠⚠ What was actually wrong, and it is not what the item was filed as
    ///
    /// 492 was filed as *"no caller can author a ceiling"*. Measured before building: this
    /// document had authored one **since 2026-08-18**, against a dated live measurement of its own
    /// loop's session. What was missing was every step after the authoring — no reader here, no
    /// `Brief` field, no wire key, no `<assign>` in the template. So item 477's live finding
    /// (**eight of eight** `reviewing` exits taking the fall-back) was not a caller who had not
    /// thought about capacity. **It was a decision that had been made and could not travel.**
    ///
    /// ⚠⚠⚠ *A decision no channel carries is a decision nobody made* — `milestone_check`'s own
    /// finding at a number instead of an argv, and this is the second time this repository has paid
    /// for it in the same document.
    ///
    /// ⚠⚠ The assertion is that a ceiling is READABLE and USABLE, not that it is any particular
    /// number: the number is this kind's judgement and belongs in its document, and a gate pinning
    /// it here would be a second place it lives. What must hold is that the arithmetic
    /// `reviewing` does is possible at all — `context_ceiling > 0` is the guard on every one of
    /// that state's deciding edges, so a zero here is the whole defect back.
    #[test]
    fn the_debt_kind_names_a_capacity_ceiling_and_it_can_be_read() {
        let kind = debt();
        let ceiling = kind.context_ceiling().expect(
            "⚠⚠⚠⚠⚠ ITEM 492: this kind's document authors `context_ceiling` and until this reader \
             existed nothing could carry it — so `reviewing`'s guards saw 0 on every run this \
             repository has ever driven, and the state never once decided (item 477 measured eight \
             of eight)",
        );
        assert!(
            ceiling > 0,
            "⚠⚠⚠⚠ and it must be a number `reviewing` can decide on. Every deciding edge in that \
             state is guarded on `context_ceiling > 0` — a zero is not a small ceiling, it is the \
             fall-back this item exists to get a run out of. Read {ceiling}",
        );
    }

    /// **AND IT ASKS FOR ITS OWN SWEEP AT THE END** — the clause the template ships empty.
    ///
    /// ⚠⚠⚠ Why a repository needs this at all: a run that noticed a defect and neither fixed nor
    /// registered it has SPENT the finding, and the next round pays to find it again. This
    /// repository's own record says both halves — *the sweep starts with «no» every time and every
    /// time it found something*, and *a recorded lesson is not an applied one*.
    ///
    /// ⚠⚠ The assertion is on the DEMAND rather than on the wording, which is prose and would make
    /// this a test agreeing with a renderer. What must hold is that both answers are open: a run
    /// that can only report a FIX cannot honestly say *"I ran out of room"*, and a clause that
    /// forced one would make hiding an unfixed finding the way to look finished.
    #[test]
    fn the_debt_kind_asks_its_endings_to_sweep_and_lets_either_answer_close_it() {
        let kind = debt();
        let clause = kind
            .closing_rules()
            .expect("this kind adds to its closing question");
        let lower = clause.to_lowercase();
        for demand in ["sweep", "pay", "register"] {
            assert!(
                lower.contains(demand),
                "⚠⚠⚠ the clause must ask for the sweep AND leave both endings open — paying it or \
                 registering it are each a complete answer, and a clause that named only the fix \
                 would make an honest *I could not* impossible to give. Missing {demand:?} in \
                 {clause:?}",
            );
        }
        assert!(
            clause.starts_with(' '),
            "⚠⚠ it is APPENDED to the template's own sentence, so it must not run into the last \
             word of it: {clause:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THIS KIND KNOWS WHAT ITS PEER SAYS WHEN ITS SERVICE FAILS, AND THE TEMPLATE DOES
    /// NOT** — the split, asserted from the side that has to hold the words.
    ///
    /// The needle is quoted from a live run: 2026-08-19, pane 99, after 28m37s of work, `API
    /// Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a
    /// moment.` Before the state this feeds existed, that ended the run at `blocked`, which is
    /// `<final>`.
    ///
    /// # ⚠⚠⚠ What each assertion is against
    ///
    /// * **THE NEEDLE IS NAMED AT ALL.** A kind that declines answers [`None`], and the whole
    ///   behaviour is off — which is the template's shipped state and a perfectly good one for a
    ///   repository that never measured its peer. This repository did measure, so a `None` here is
    ///   the finding going back out of the product.
    /// * ⚠⚠⚠⚠ **AND IT IS THE CODE, NOT THE PROSE.** The tail of that message (*"usually
    ///   temporary…"*) is wording that has changed before; `529` is the status. A needle quoting
    ///   the sentence would stop matching the day somebody rewrote the apology, and this gate is
    ///   what says the short head was a decision rather than an accident.
    /// * ⚠⚠ **AND IT IS NOT THE WHOLE FAMILY.** `API Error` alone would swallow a 400, which is
    ///   this run's own fault and will still be its fault in ten minutes — waiting one out forever
    ///   is a worse ending than stopping.
    /// * ⚠⚠⚠⚠⚠ **AND THE USAGE LIMIT IS HERE, WHICH IS REGISTER ITEM 715.** The list held ONE
    ///   string, so the most predictable interruption in an unattended loop's life reached none of
    ///   this machinery: the peer printed *"Usage limit reached · continuing automatically at
    ///   3:30am"*, stopped speaking, the machine heard `peer.silent`, and the run was `blocked` an
    ///   hour later while the peer resumed on its own and kept working.
    /// * ⚠⚠⚠ **AND NOT THE RECOVERY NOTICE**, which is the same family and the opposite fact:
    ///   `Usage limit reset · continuing automatically` says the outage is OVER, so the trailing
    ///   `at` in the needle is load-bearing and is asserted rather than commented.
    /// * **AND THE REMEDY IS THE OWNER'S**: ten minutes, and a word that means carry on rather
    ///   than the milestone again — the session survived the outage holding everything it had.
    #[test]
    fn the_debt_kind_names_what_its_peer_prints_when_its_service_fails() {
        let outage = debt().service_outage().expect(
            "⚠⚠⚠ this repository measured its peer's 529 and must carry it, or the run \
                     that paid 28 minutes to find it learns nothing",
        );
        assert!(
            outage.needles.iter().any(|says| says.contains("529")),
            "⚠⚠⚠⚠ THE CODE IS THE STABLE HALF and the apology around it is not: {:?}",
            outage.needles,
        );
        for says in &outage.needles {
            assert!(
                says.len() < "API Error: 529 Overloaded. This is a server-side issue".len(),
                "⚠⚠⚠ EVERY ELEMENT MUST BE A SHORT HEAD. The messages arrive WRAPPED across rows, \
                 and a needle carrying a whole sentence depends on where the terminal broke it: \
                 {says:?}",
            );
            assert_ne!(
                says.trim(),
                "API Error",
                "⚠⚠ NOT THE WHOLE FAMILY: a 400 is this run's own fault and waiting it out \
                 forever is a worse ending than stopping",
            );
            // ⚠⚠⚠⚠⚠ AND NOT THE BARE WORD THIS LOOP WRITES ABOUT — register item 715. The
            // fallback reads the PANE, and a debt run's own agent edits a register that discusses
            // usage limits by name, so `limit` alone is a needle a run can print BY WORKING. That
            // is not a hypothetical about some other repository: it is this one.
            assert_ne!(
                says.trim(),
                "limit",
                "⚠⚠⚠ a needle this loop's own work puts on the screen stops the loop for working",
            );
        }
        // ⚠⚠⚠⚠ AND THE USAGE LIMIT IS NAMED AT ALL, which is what item 715 measured missing: the
        // most predictable stop in an unattended loop's life reached none of this machinery
        // because the needle was ONE string and that string was the 529.
        assert!(
            outage
                .needles
                .iter()
                .any(|says| says.contains("continuing automatically")),
            "⚠⚠⚠⚠⚠ THE PEER'S OWN RECOVERY SENTENCE MUST BE HERE. Without it a usage limit is \
             filed as `peer.silent`, the run reaches `blocked` an hour later, and the peer goes on \
             working with nobody driving it — measured on run 5, 2026-08-27: {:?}",
            outage.needles,
        );
        // ⚠⚠⚠ AND IT DOES NOT MATCH THE PEER SAYING IT IS BACK. `Usage limit reset · continuing
        // automatically` is the RECOVERY notice, in the same transcripts, and a needle that
        // swallowed it would file a healthy peer as a broken one. The trailing `at` is what
        // separates them, and this is the assertion that says so rather than a comment.
        assert!(
            !outage
                .needles
                .iter()
                .any(|says| "Usage limit reset · continuing automatically".contains(says.as_str())),
            "⚠⚠⚠⚠ A NEEDLE MUST NOT MATCH THE RECOVERY NOTICE: {:?}",
            outage.needles,
        );
        assert_eq!(
            outage.every_ms, 600_000,
            "ten minutes, the owner's number — asking again sooner is the load that caused it",
        );
        assert_eq!(
            outage.text, "continue",
            "⚠⚠ NOT THE MILESTONE AGAIN: the session still holds its brief and its half-finished \
             turn, so re-asking the whole question spends the context the outage did not take",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THIS KIND HOLDS ITS RUNS TO RULES, AND THEY ARE THE ONES A PERSON WAS RETYPING** —
    /// register item 738, layer 2.
    ///
    /// # ⚠⚠⚠⚠ What the assertion is, and what it deliberately is not
    ///
    /// The rules are PROSE and this repository's own, so pinning their wording here would be a test
    /// agreeing with a renderer — `closing_rules`' gate makes the same choice one method up. What
    /// must hold is the property the item was filed on: **the rules exist in the DOCUMENT rather
    /// than in a supervisor's memory**, and they are shaped to be composed into a prompt.
    ///
    /// ⚠⚠⚠ **THE SIZE FLOOR IS THE MEASUREMENT AND NOT A STYLE RULE.** The launch this item was
    /// registered against carried about 2 KB of hand-typed standing rules in `north_star`; a clause
    /// that came back as a sentence would mean the block had been quietly emptied to a token, which
    /// reads identically to a kind that authors none once it reaches a prompt. So the floor is well
    /// under what was measured and well over what an accident produces.
    #[test]
    fn the_debt_kind_holds_its_runs_to_rules_that_live_in_the_document() {
        let kind = debt();
        let rules = kind.working_rules().expect(
            "⚠⚠⚠⚠⚠ ITEM 738: this repository holds every turn of its debt runs to standing rules, \
             and until they were written here they were typed BY HAND into `north_star` on every \
             launch — out of one session's context, and gone when that session ended",
        );
        assert!(
            rules.len() > 200,
            "⚠⚠⚠ the rules a run works under, not a token standing in for them: the launch this \
             item was registered against carried about 2 KB of them and a clause this short is a \
             block that was emptied rather than authored. Read {} bytes",
            rules.len(),
        );
        assert!(
            rules.ends_with('\n'),
            "⚠⚠⚠⚠ IT CARRIES ITS OWN LINE TERMINATOR, like the template's `standing` it is composed \
             beside: `start_prompt` concatenates it with no separator, so a block that did not end \
             a line would run the next clause of the prompt onto the last rule: {rules:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND IT SAYS WHERE A RUN OF IT STARTS READING** — register item 738, layer 2's other
    /// half, and the one that could not be authored while the wire demanded it.
    ///
    /// `reference` was `require_str` at the door, so omitting the key was MALFORMED rather than
    /// deferring: item 312's finding at a string instead of a count — **a required judgement is a
    /// decision the document is structurally forbidden from making.** What filled the gap was a
    /// person retyping the ledger's path into every launch.
    ///
    /// ⚠⚠ The assertion is that the value is READABLE and is not the template's placeholder. Where
    /// this repository's ledger lives is this document's business and a path pinned here would be a
    /// second place it lives; what must hold is that a caller who names none does not get
    /// `(edit me)`, which R380 measured reaching a live agent.
    #[test]
    fn the_debt_kind_says_where_a_run_of_it_starts_reading() {
        let kind = debt();
        let reference = kind.reference().expect(
            "⚠⚠⚠⚠⚠ ITEM 738: a debt run's only prior art is the ledger, and a kind that names none \
             leaves the door with nothing to fall through to — so every launch has to retype the \
             path out of somebody's memory",
        );
        assert!(
            !reference.contains("edit me"),
            "⚠⚠⚠⚠ NOT THE TEMPLATE'S PLACEHOLDER. `(edit me) paths, URLs or repos to consult` is \
             composed into the prompt exactly as written — R380 measured a live agent reading \
             three of five clauses that way — so a kind that echoed it would be worse than one \
             that authored nothing: {reference:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND IT KNOWS WHEN ITS PANE IS READY TO BE TYPED INTO** — register item 738, layer 3.
    ///
    /// # ⚠⚠⚠ The one clause of a kind that is not in the template's datamodel
    ///
    /// Item 300 put the readiness barrier on the run SPEC and the durations in `<data>`, on a line
    /// this does not cross: what makes a pane ready is a predicate about the peer, how long anybody
    /// waits for it is a judgement. This changes who AUTHORS the predicate. The measurement that
    /// asked for it: every launch spelled `--match settles --marker claude`, which
    /// `AiLoopSpec::driving` already derives from `--agent claude` — **the same fact typed twice,
    /// every time, and neither copy written down anywhere.**
    ///
    /// ⚠⚠ The assertion is on the KIND of barrier rather than on the marker's spelling: which peer
    /// this repository drives is its document's business, and `Settles` is the claim — the only
    /// barrier word that asks the operating system instead of the screen, and so the only one an
    /// agent that prints nothing on startup can be waited for by.
    #[test]
    fn the_debt_kind_knows_when_its_pane_is_ready() {
        let barrier = debt()
            .ready_when()
            .expect("its barrier must be one this driver can carry out")
            .expect(
                "⚠⚠⚠⚠⚠ ITEM 738: a kind that names no barrier leaves a launch that named no \
                 `agent` with nothing — and a loop with no barrier types its first prompt into \
                 whatever the pane happens to be running (R379 measured that costing a whole run)",
            );
        assert!(
            matches!(barrier, crate::ReadyWhen::Settles(_)),
            "⚠⚠⚠⚠ `settles` IS THE CLAIM. `prints` and `shows` are predicates over TEXT, and an \
             agent CLI that starts quietly emits none to predicate over; `settles` asks the \
             operating system which program owns the pane's terminal, which cannot be echoed by \
             what the loop itself typed. Read {barrier:?}",
        );
        assert!(
            !barrier.marker().is_empty(),
            "⚠⚠ and a marker `ReadyWhen::parse` would have refused cannot arrive here: an empty \
             one names no process, so the barrier could never clear",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THIS KIND SAYS WHAT ACTUALLY KILLS ITS RUNS** — register item 738, layer 1, and
    /// the layer that had cost real work rather than only real typing.
    ///
    /// # ⚠⚠⚠⚠⚠ The measurement, taken in this daemon's own registry
    ///
    /// `state/sprag/sprag-loop.runs.json`, 2026-08-28, 49 runs: **eight ended `exhausted (cost)`**,
    /// every one between 65,809 and 68,658 bytes — the daemon's 64 KiB default — while the largest
    /// run that CONVERGED spent **516,020** bytes over 1,231 iterations. A backstop that ends one
    /// run in six is not a backstop; it is the ceiling that bites first, and it bites mid-round
    /// with the work uncommitted in the tree.
    ///
    /// # ⚠⚠⚠ What is asserted, and what is deliberately NOT
    ///
    /// Not the numbers. What `debt_loop.scxml` chooses is that document's business and a figure
    /// pinned here would be a second place it lives — `context_ceiling`'s gate makes the same
    /// choice. What must hold is the property the item was filed on: **the clause exists, it names
    /// all three, and its cost bound is bigger than the largest run this daemon has ever recorded
    /// converging.** The last is the only one that would have caught the defect, so it is the one
    /// with the number in it.
    ///
    /// ⚠⚠ The MEANING of the keys is `sprag-host`'s, checked against the wire's own publication —
    /// see [`LoopKind::authored_numbers`]. This gate reads the shape, which is all this crate can
    /// honestly claim about a vocabulary it does not own.
    #[test]
    fn the_debt_kind_says_what_actually_kills_its_runs() {
        let named = debt()
            .authored_numbers("guardrails")
            .expect("its guardrail clause must be an object of numbers")
            .expect(
                "⛔⛔⛔⛔⛔ ITEM 738: a debt run is ended by three bounds this document could not \
                 reach, so it was ended by this daemon's constants — 8 of 49 recorded runs died at \
                 the 64 KiB default, mid-round, with their work uncommitted",
            );
        for key in ["max_bytes", "max_iterations", "max_seconds"] {
            assert!(
                named.contains_key(key),
                "⚠⚠⚠⚠ ALL THREE OR THE GATE IS HALF A GATE: a clause that names two leaves the \
                 third to whoever remembered to type it, which is the whole failure. Missing \
                 {key:?} in {named:?}",
            );
        }
        let bytes = named["max_bytes"];
        assert!(
            bytes > 516_020,
            "⛔⛔⛔ AND THE COST BOUND MUST EXCEED THE LARGEST RUN THIS DAEMON HAS EVER RECORDED \
             CONVERGING — run 17, 1,231 iterations, 516,020 bytes. A ceiling under that cuts the \
             work this loop exists to do, which is what the 64 KiB default did eight times. Read \
             {bytes}",
        );
        // ⚠⚠ AND THE TWO BOUNDS MUST AGREE RATHER THAN ONE BEING DECORATIVE. At the rate that same
        // run measured — 516,020 bytes over 1,231 steps, about 419 a step — the step ceiling and
        // the byte ceiling should bite at roughly the same place. Two bounds that fire an order of
        // magnitude apart mean one of them is not a decision, and a run stopped by a number nobody
        // reasoned about is this item's own defect wearing the other ceiling's name.
        let implied = named["max_iterations"].saturating_mul(419);
        assert!(
            implied * 4 > bytes && bytes * 4 > implied,
            "⚠⚠⚠ the step and byte ceilings must bite within a factor of four of each other at \
             this loop's own measured 419 bytes per step: {} steps implies {implied} bytes against \
             a bound of {bytes}",
            named["max_iterations"],
        );
    }

    /// ⚠⚠⚠⚠ **AND HOW LONG SOMEBODY MAY HOLD ONE** — register item 738, layer 1's fifth ceiling,
    /// and it is in the document because a GATE asked rather than because anybody noticed.
    ///
    /// `Ceiling::ALL` names five things that end a run, and this item's gate walks that set with no
    /// exemption arm: a ceiling nobody classified is a RED and not a pass. Four became reachable by
    /// a kind once its guardrails did; `Hold` was the fifth, and writing the exemption instead
    /// would have been the escape hatch that disarms the gate.
    ///
    /// ⚠⚠ The assertion is that it is READABLE and is a bound a hold can actually end on. What the
    /// number is, and why it equals this kind's own wall-clock budget, is argued where it is
    /// authored — a hold longer than the run's own life can never be handed back to a live run.
    #[test]
    fn the_debt_kind_bounds_how_long_a_person_may_hold_it() {
        let held = debt().hold_within_ms().expect(
            "⚠⚠⚠⚠ ITEM 738: a kind that names no hold ceiling leaves the fifth thing that can end \
             its runs to a number nobody in this repository chose",
        );
        assert!(
            held > 0,
            "⚠⚠ zero is `cancel` spelled wrong and the door refuses it, so a document that meant \
             it has said something no run can obey: {held}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **EVERY WORD THIS BUILD PUBLISHES AS A KIND OPENS A DOCUMENT** — register item
    /// 848, and the control on everything below it.
    ///
    /// [`LoopKind::KINDS`] is what the wire's grammar publishes and what a client is shown, and
    /// [`LoopKind::named`] is what the door resolves. A word in one and not the other is an agent
    /// building a call from the published vocabulary that this daemon then refuses — a defect this
    /// workspace has already met once, at `done_when`'s `settles`.
    #[test]
    fn every_kind_word_this_build_publishes_is_one_a_run_can_start_under() {
        assert!(
            LoopKind::KINDS.len() >= 2,
            "⛔⛔⛔ ITEM 848's OWN PREMISE: with one legal value, *which kind* is not a question a \
             caller can answer — it is a key that can only be spelled one way, which is the \
             hardcoded default wearing an argument's clothes",
        );
        for word in LoopKind::KINDS {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let kind = LoopKind::named(word, lua).unwrap_or_else(|why| {
                panic!(
                    "⛔ the grammar publishes {word:?} as a kind a caller may name, and the door \
                     answers: {why}"
                )
            });
            assert_eq!(
                kind.name(),
                *word,
                "⚠⚠ a kind that reports a different word than the one it was asked for makes \
                 *which document decided this* unanswerable from the run",
            );
        }
    }

    /// 🎯🎯🎯🎯🎯 **A WORD NO DOCUMENT ANSWERS TO IS REFUSED, AND THE REFUSAL NAMES THE
    /// ALTERNATIVES** — register item 848's second clause.
    ///
    /// ⛔ The state this replaced was a driver that named one kind for every caller, so *anything
    /// unrecognised runs under this repository's* is exactly the escape hatch the item is about:
    /// **an unclassified run is a red rather than a pass.**
    #[test]
    fn a_kind_nobody_wrote_a_document_for_is_refused_by_name() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let why = LoopKind::named("feature", lua)
            .err()
            .expect("⛔⛔⛔ ITEM 848: a kind word this build has no document for must not resolve");
        assert_eq!(why, NoKind::Unknown("feature".to_string()));
        let said = why.to_string();
        for word in LoopKind::KINDS {
            assert!(
                said.contains(word),
                "⚠⚠ a door that rejects a word without naming the legal ones sends a caller to \
                 read this daemon's source, which is the invisible choice item 848 is about. \
                 Missing {word:?} from: {said}",
            );
        }
    }

    /// 🎯🎯🎯🎯🎯 **THE UNCLAIMED KIND NAMES NO CHECKER, AND THAT IS THE CLAUSE ITEM 848 IS
    /// ABOUT** — what makes item 847's *nobody classified this* a true sentence about a real run
    /// rather than about a fixture.
    ///
    /// # ⛔⛔⛔⛔⛔ What the hardcoded kind did to a run in another tree
    ///
    /// `successor_check` is a PROGRAM, and this repository's kind points it at this repository's
    /// own record by absolute path. A run elsewhere, judged by it, proposes work that names no item
    /// of that record — and the checker's answer for that is `NO`. So the silent default was not a
    /// stricter setting: it was a run refused its own work by a document that has never heard of
    /// it. The unclaimed kind names no checker at all, which is the honest state for a tree that
    /// has not written one.
    ///
    /// ⚠⚠ Both directions are asserted in one test on purpose: *this kind is empty* is only
    /// meaningful beside *and the other one is not*, which is the vacuous-green shape this
    /// workspace keeps meeting (item 799).
    #[test]
    fn the_unclaimed_kind_names_no_checker_where_this_repositorys_does() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let unclaimed = LoopKind::unclaimed(lua).expect("the unclaimed kind's document must open");
        assert_eq!(
            unclaimed.successor_check(),
            None,
            "⛔⛔⛔ ITEM 848: a kind that holds no decisions must hold no CHECKER, or a run that \
             asked to be governed by nothing is governed by whatever this clause names",
        );
        let mine = debt().successor_check().expect(
            "⚠⚠ the control: this repository's own kind DOES name one, or the assertion above is \
             green because nothing anywhere names a checker",
        );
        assert!(
            mine.contains("--admits"),
            "⚠ the control's control — this repository's checker is the classifier item 839 built, \
             and a kind naming some other program would make the comparison above meaningless: \
             {mine}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **AND IT HOLDS NOTHING ELSE EITHER** — register item 848, sweeping the class
    /// rather than the instance above.
    ///
    /// A kind that answered *no checker* while quietly carrying this repository's standing yesses,
    /// working rules, ceilings or window would be the same defect one clause over. Every reader
    /// [`LoopKind`] has is asked, and the ones that can refuse are asked through their refusal.
    #[test]
    fn the_unclaimed_kind_decides_nothing_at_all() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = LoopKind::unclaimed(lua).expect("the unclaimed kind's document must open");
        // ⚠ The three ids it DOES declare are declared EMPTY, and empty reads as *this document
        // adds nothing* — the polarity every reader here already applies.
        assert_eq!(
            kind.consents().expect("an empty list is readable"),
            None,
            "⛔ a standing yes here would authorise every unclaimed run, for everybody",
        );
        assert_eq!(
            kind.screen_rules().expect("an empty list is readable"),
            None,
            "⛔ a standing instruction here would be typed into the next author's agent",
        );
        assert_eq!(
            kind.ready_when()
                .expect("a document that names no barrier is readable"),
            None,
        );
        assert_eq!(kind.closing_rules(), None);
        assert_eq!(kind.working_rules(), None);
        assert_eq!(
            kind.unverified_rules()
                .expect("a document that declares none of the three is not half-authored"),
            None,
        );
        assert_eq!(kind.reference(), None);
        assert_eq!(kind.milestone_check(), None);
        assert_eq!(kind.reask_max(), None);
        assert_eq!(kind.reaim_max(), None);
        assert_eq!(kind.turn_budget(), None);
        assert_eq!(kind.reflect_every(), None);
        assert_eq!(kind.context_ceiling(), None);
        assert_eq!(kind.reflect_after_refusals(), None);
        assert_eq!(kind.hold_within_ms(), None);
        assert_eq!(kind.service_outage(), None);
        assert_eq!(kind.works_in(), None);
        assert_eq!(kind.stands_in(), None);
        assert_eq!(kind.keeps(), None);
    }

    /// ⚠⚠⚠⚠⚠ **A KIND DECLARES EXACTLY WHAT ITS READERS READ** — register item 848, and the one
    /// place a [`KindDocument`] implementation can go stale against its own document.
    ///
    /// Codegen emits an accessor per declared `<data>`, so a clause a document does not declare has
    /// no accessor and its method is written `None` BY HAND. That hand-written `None` is a fact
    /// about the file — until somebody adds the clause to the file and not to the implementation,
    /// at which point the document says one thing and every run reads another, silently.
    ///
    /// ⚠⚠ So the declared ids are PINNED per document, read as text off the file an author edits.
    /// A clause added anywhere goes red here naming the id, and the red's remedy is the
    /// implementation beside it.
    #[test]
    fn a_kind_declares_exactly_what_its_readers_read() {
        /// Every `<data id="…">` a document declares, in the order the file has them.
        fn declared(document: &str) -> Vec<String> {
            document
                .match_indices("<data id=\"")
                .filter_map(|(at, opener)| {
                    document[at + opener.len()..]
                        .split_once('"')
                        .map(|(id, _)| id.to_string())
                })
                .collect()
        }

        let unclaimed = declared(include_str!("unclaimed_loop.scxml"));
        assert_eq!(
            unclaimed,
            ["may_answer", "screen_rules", "judged_rules"],
            "⛔⛔⛔⛔⛔ ITEM 848: `unclaimed_loop.scxml`'s clauses moved, and its `KindDocument` \
             implementation answers `None` for every id it does not declare BY HAND. A clause \
             added to that document with no reader beside it is a decision an author wrote and no \
             run will ever read. Add the accessor to the implementation, then move this pin",
        );
        // ⚠ The control: the pin above is only meaningful while a document that DOES declare its
        // clauses is read by the same needle. This repository's own kind is that document, and the
        // count is what says the reader still sees what it used to.
        let mine = declared(include_str!("debt_loop.scxml"));
        assert_eq!(
            mine.len(),
            26,
            "⚠⚠ the needle stopped seeing this repository's own kind, so the assertion above is \
             green about a document nobody read: {mine:?}",
        );
    }
}
