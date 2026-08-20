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

/// One loop kind's authored decisions, read off its own document.
///
/// It holds the script SESSION rather than the values, for the reason `pump` re-reads the template
/// on every pass: a value copied at construction is a value that can no longer be corrected, and
/// what an author wrote is the authority for as long as the run lasts.
pub struct LoopKind {
    /// The engine is kept alive because the session id below is only meaningful while it is —
    /// dropping the machine closes the script session and every read after it answers nothing.
    #[allow(
        dead_code,
        reason = "held to keep `session` valid; see the field's own note"
    )]
    machine: Engine<crate::sm::debt_loop::DebtLoopPolicy>,
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
}

impl std::fmt::Display for NoKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDatamodel => f.write_str(
                "this loop kind's document opened no script session, so it holds no decisions — a \
                 kind must declare `datamodel=\"ecmascript\"`",
            ),
        }
    }
}

impl LoopKind {
    /// **THE DEBT-REPAYMENT KIND** — `debt_loop.scxml`, this repository's own.
    ///
    /// ⚠ The machine is initialised and never stepped. Its one state is final on entry: a kind is a
    /// datamodel, and initialising is what evaluates the `<data>` expressions into the session this
    /// then reads.
    ///
    /// # Errors
    ///
    /// [`NoKind::NoDatamodel`] when the document opened no script session.
    pub fn debt(script: Arc<dyn IScriptEngine>) -> Result<Self, NoKind> {
        let mut machine = Engine::new(crate::sm::debt_loop::DebtLoopPolicy::new(Arc::clone(
            &script,
        )));
        machine.initialize();
        let session = machine
            .policy()
            .session_id
            .clone()
            .ok_or(NoKind::NoDatamodel)?;
        Ok(Self {
            machine,
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
        OuterLoop::authored_text_in(&self.script, &self.session, "closing_rules")
    }

    /// **HOW MANY TURNS A RUN OF THIS KIND MAY TAKE**, or [`None`] where this kind says nothing and
    /// the template's own number stands.
    ///
    /// ⚠⚠ [`Counted::Never`] is a DECISION and not an absence — a debt run's job is a list nobody
    /// has finished, so it ends on its work rather than on a count. See this kind's document for
    /// what that costs and why the spelling is a word.
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
        match OuterLoop::authored_count_in(&self.script, &self.session, "reflect_every") {
            Some(Counted::Of(cadence)) => Some(cadence),
            _ => None,
        }
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
        OuterLoop::authored_number_in(&self.script, &self.session, "context_ceiling")
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
        OuterLoop::authored_number_in(&self.script, &self.session, "reflect_after_refusals")
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
    /// # ⚠⚠⚠ THE NEEDLE DECIDES, AND THE OTHER TWO ONLY SHAPE IT
    ///
    /// A document with no needle has declined however carefully it filled the other two, so this
    /// answers [`None`] on that alone. The wait and the word then fall back to the template's own
    /// values rather than to numbers invented here — a kind that names the failure and says nothing
    /// about the remedy has asked for the default remedy, not for a broken one.
    ///
    /// ⚠⚠ A NEGATIVE OR UNREADABLE WAIT IS THE DEFAULT AND NOT AN ERROR, on
    /// [`turn_budget`](Self::turn_budget)'s terms: this is a document a person edits, and the
    /// failure mode worth preventing is a run that spins re-asking a service that is already
    /// refusing. Falling back to ten minutes is the direction to be wrong in.
    #[must_use]
    pub fn service_outage(&self) -> Option<crate::outer::ServiceOutage> {
        let needle = match self.script.get_variable(&self.session, "service_needle") {
            Ok(ScriptValue::String(text)) if !text.trim().is_empty() => text,
            _ => return None,
        };
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
            needle,
            every_ms: every_ms.unwrap_or(crate::outer::DEFAULT_SERVICE_RETRY_MS),
            text: text.unwrap_or_else(|| crate::outer::DEFAULT_SERVICE_RETRY_TEXT.to_owned()),
        })
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
    /// * **AND THE REMEDY IS THE OWNER'S**: ten minutes, and a word that means carry on rather
    ///   than the milestone again — the session survived the outage holding everything it had.
    #[test]
    fn the_debt_kind_names_what_its_peer_prints_when_its_service_fails() {
        let outage = debt().service_outage().expect(
            "⚠⚠⚠ this repository measured its peer's 529 and must carry it, or the run \
                     that paid 28 minutes to find it learns nothing",
        );
        assert!(
            outage.needle.contains("529"),
            "⚠⚠⚠⚠ THE CODE IS THE STABLE HALF and the apology around it is not: {:?}",
            outage.needle,
        );
        assert!(
            outage.needle.len() < "API Error: 529 Overloaded. This is a server-side issue".len(),
            "⚠⚠⚠ IT MUST BE THE SHORT HEAD. The message arrives WRAPPED across rows, and a needle \
             carrying the whole sentence depends on where the terminal broke it: {:?}",
            outage.needle,
        );
        assert_ne!(
            outage.needle.trim(),
            "API Error",
            "⚠⚠ NOT THE WHOLE FAMILY: a 400 is this run's own fault and waiting it out forever is \
             a worse ending than stopping",
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
}
