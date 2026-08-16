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

use sce_rust_runtime::{Engine, IScriptEngine};

use crate::consent::Consents;
use crate::outer::{NotScreenable, OuterLoop};
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
}
