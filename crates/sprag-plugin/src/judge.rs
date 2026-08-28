//! **A SECOND AGENT, ASKED ONE YES-OR-NO QUESTION ABOUT ONE DIALOG** — what
//! `ai_loop.scxml`'s `cond="_event.data.judged"` is decided by, once per rule in its
//! `judged_rules`.
//!
//! # ⚠⚠⚠ Why quoting could not reach this, measured
//!
//! A [`ScreenRule`](crate::screen::ScreenRule) claims a dialog by quoting it, and R383 established
//! that quoting covers the population it had measured. A later measurement found the case it
//! cannot: these two dialogs, captured verbatim from a live `claude`, ask the identical question.
//!
//! ```text
//! Do you want to create PROBE.txt?     <- writes the word "ready"
//! Do you want to create DESIGN.txt?    <- commits 256 lines that chose JSON over SQLite
//! ```
//!
//! **No needle separates them**, because what differs is what the action MEANS, and that lives in
//! the diff the dialog carries rather than in its wording. A cheap judge told them apart 3 times
//! out of 3, on all four captured dialogs, at 4-6 s per call.
//!
//! # ⚠⚠ Why the verdict is published rather than evaluated in the guard
//!
//! SCXML does not promise how often it evaluates a `cond`, and the pinned engine has no seam to
//! register a host function on ([`IScriptEngine`](sce_rust_runtime) exposes `execute_script`,
//! `evaluate_expression`, `validate_expression` and the variable accessors, and nothing else). So
//! a guard cannot do this and must not try: the driver judges ONCE, when it raises the event, and
//! the document reads `_event.data.judged`. The driver measures; the document decides — the same
//! arrangement `_event.data.done` has always used.
//!
//! # ⚠⚠⚠ The failure direction is the safety property
//!
//! No rules, no judge, a timeout, a crash, a reply that is not a verdict — every one of them
//! answers [`None`], and the caller publishes `false`. **Silence is never a yes.** A `true` on
//! silence would refuse the agent's tool call on nobody's decision, which is the act
//! [`REFUSES`] is structured to keep out of reach.
//!
//! [`REFUSES`]: crate::screen::REFUSES

use std::time::{Duration, Instant};

use sprag_detect::Question;

use crate::access::PaneAccess;
use crate::completion::{Completion, DoneWhen, Over};
use crate::run::RunContext;
use crate::screen::REFUSES;

/// **WHO JUDGES, AND HOW LONG IT MAY TAKE.**
///
/// ⚠ The argv is the CALLER's, not this crate's. Which agent judges — and at what price — is a
/// choice with a real bill attached, made once per run by whoever starts it, exactly as
/// [`Dialogue`](crate::dialogue)'s endpoints are. This crate hard-codes no model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgeSpec {
    /// The judge's argv template. The rendered question is APPENDED as the last argument, so a
    /// print-mode CLI reads it positionally and there is no cooked-mode echo to strip.
    pub argv: Vec<String>,
    /// How long one judgement may take before it is abandoned as [`None`].
    ///
    /// ⚠ It sits in the critical path of a BLOCKED turn: the agent is stopped at its dialog and
    /// nothing moves until this returns. Measured at 4-6 s against a cheap model, so a bound of
    /// tens of seconds is patience and not generosity.
    pub within: Duration,
}

impl JudgeSpec {
    /// # ⚠⚠⚠⚠ NEITHER OF THESE IS A WIRE KEY TODAY, AND BOTH SAID THEY WERE
    ///
    /// They are the names a judge WOULD take if a caller could declare one — and no caller can.
    /// The daemon's `ai_loop` form publishes seventeen arguments and neither of these is among
    /// them (`crate::wire`'s own pinned shape lists every one), nothing outside this file reads
    /// either constant, and the only [`AiLoopSpec::judge`](crate::ai_loop) a run ever carries is
    /// `None`. **The judging capability exists and is reachable only from in-process gates.**
    ///
    /// Register item 314 read the bound below as item 300's next duration — *a duration is a
    /// judgement the document should make, not a caller argument* — but that rule bites on
    /// arguments a caller can PASS, and there is no such argument here. What is true is smaller and
    /// different: two constants that named a surface they are not on. Corrected rather than
    /// deleted, because the names are the ones a door would use and the door is a decision.
    ///
    /// The datamodel key of the judge's argv, and the name a wire argument would take.
    pub const ARGV_KEY: &'static str = "judge";
    /// The name a wire argument for the bound would take — see [`ARGV_KEY`](Self::ARGV_KEY).
    pub const WITHIN_KEY: &'static str = "judge_timeout_ms";

    /// The size of the pane a judgement runs in.
    ///
    /// ⚠ Small on purpose and not tuned: nothing reads this pane as a SCREEN.
    ///
    /// ⚠⚠⚠⚠⚠ **THIS COMMENT USED TO GO ON "so the only thing a width could do is wrap a one-word
    /// answer", AND THAT SENTENCE COST ITEM 517.** A judge does not always answer in one word — the
    /// field beside the verdict exists precisely because it answers in paragraphs
    /// ([`Judgement::explained`]) — and while the reply was read from the pane's full TEXT, this
    /// width was silently the ceiling on every reason a run could record. It is now read by address
    /// ([`spoke`]), which is what makes the number back into what it claims to be: a size nothing
    /// depends on.
    const PANE: (u16, u16) = (80, 24);
}

/// **ONE JUDGED DECISION**: what to call it, the sentence a judge answers, and what to say once the
/// dialog it claims has been refused.
///
/// # ⚠⚠⚠ A list of these, so a decision is one element to add and one to delete
///
/// Shaped after [`ScreenRule`](crate::screen::ScreenRule) deliberately. The two differ in ONE
/// thing — whether the claim is quoted or judged — and an author who has written one has written
/// the other. A decision welded in as a pair of scalars would be a decision nobody can add a
/// second of.
///
/// ⚠ [`name`](Self::name) exists so a DOCUMENT can fork per decision: the driver publishes it as
/// `_event.data.rule` beside the boolean the shipped guard reads. Nothing in this crate branches
/// on it — it is there for the author, which is the point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgedRule {
    name: String,
    criterion: String,
    text: String,
}

impl JudgedRule {
    /// The datamodel and wire key of the rule's name, inside one element of [`JudgedRules::KEY`].
    pub const NAME_KEY: &'static str = "name";
    /// The datamodel and wire key of the criterion.
    pub const JUDGE_KEY: &'static str = "judge";
    /// The datamodel and wire key of what the agent is told.
    pub const TEXT_KEY: &'static str = "text";

    /// A rule called `name`, claiming the dialogs a judge says `criterion` holds of, answered by
    /// refusing the call and saying `text`.
    ///
    /// # Errors
    ///
    /// [`Malformed`](crate::screen::Malformed), naming which field to change. An empty criterion
    /// is [`ClaimsEverything`](crate::screen::Malformed::ClaimsEverything) for a sharper reason
    /// than a quote's: an empty quote is carried by every dialog, and an empty criterion is a
    /// question a judge cannot answer at all, so a rule holding one would spend a model call per
    /// blocked turn to learn nothing.
    pub fn parse(
        name: String,
        criterion: String,
        text: String,
    ) -> Result<Self, crate::screen::Malformed> {
        if name.trim().is_empty() || criterion.trim().is_empty() {
            return Err(crate::screen::Malformed::ClaimsEverything);
        }
        if text.is_empty() {
            return Err(crate::screen::Malformed::SaysNothing);
        }
        Ok(Self {
            name,
            criterion,
            text,
        })
    }

    /// What this decision is called — published as `_event.data.rule`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The sentence a judge answers about one dialog.
    #[must_use]
    pub fn criterion(&self) -> &str {
        &self.criterion
    }

    /// What the agent is told once the call is refused.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// **WHAT A LOOP JUDGES**, as a list of independent [`JudgedRule`]s.
///
/// ⚠ EMPTY IS REPRESENTABLE HERE where [`ScreenRules`](crate::screen::ScreenRules) refuses it, and
/// the difference is not an inconsistency. A list of quotes is read from a document that ships one
/// as a placeholder, so *"I wrote no rules"* and *"I wrote an empty list"* had to be distinguished.
/// This list ships EMPTY as its default — declining a second agent is the ordinary state of a run —
/// so empty is the answer rather than an ambiguity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JudgedRules {
    rules: Vec<JudgedRule>,
}

impl JudgedRules {
    /// The datamodel variable — and wire argument — these are authored in.
    pub const KEY: &'static str = "judged_rules";

    /// A list of judged decisions, in the order the author wrote them.
    #[must_use]
    pub fn of(rules: Vec<JudgedRule>) -> Self {
        Self { rules }
    }

    /// Every rule, in document order.
    #[must_use]
    pub fn rules(&self) -> &[JudgedRule] {
        &self.rules
    }

    /// The FIRST rule a judge says claims `question`, or [`None`] when none does.
    ///
    /// ⚠⚠⚠ EACH RULE COSTS A MODEL CALL, so the order an author writes them in is the order they
    /// are PAID for, and the first match stops the rest being asked. That is a real reason to put
    /// the likeliest decision first, and it is worth saying because no other rule list in this
    /// crate has a per-element price.
    ///
    /// ⚠ A rule whose judge says nothing is skipped and the next is tried — silence about one
    /// criterion says nothing about another.
    ///
    /// ⚠⚠ **AND THE REASON FOR THAT SILENCE IS DELIBERATELY DROPPED HERE, WHICH IS NOT THE SAME
    /// DECISION THE MILESTONE CHECK TOOK** — register item 593. This walk asks up to one model call
    /// PER RULE and takes the first that claims the dialog, so a run with five rules can produce
    /// five silences of five different kinds while still ending on a rule that answered. There is
    /// no single reason to report and no place to put five; what a caller acts on here is *no rule
    /// claimed this dialog*. The milestone check has exactly one question and one answer, so its
    /// silence has a reason worth carrying — see `crate::outer::OuterLoop::checked`.
    #[must_use]
    pub fn claiming(
        &self,
        panes: &dyn PaneAccess,
        run: &RunContext,
        question: &Question,
        spec: &JudgeSpec,
    ) -> Option<(&JudgedRule, Judgement)> {
        self.rules.iter().find_map(|rule| {
            judges(panes, run, rule.criterion(), question, spec)
                .ok()
                .filter(|judged| judged.holds)
                .map(|judged| (rule, judged))
        })
    }
}

/// **WHAT A JUDGE SAID**, kept whole rather than reduced to the bool, because a run that refused
/// its agent's tool call on this has to be able to show what decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
    /// Whether the criterion holds.
    pub holds: bool,
    /// The verdict word actually read — what the reply's FIRST WORD was.
    ///
    /// ⚠ Carried because the reply is not guaranteed to be one word. Measured: the same model
    /// answers some dialogs with a bare verdict and others with a paragraph after it, so a reader
    /// comparing whole replies called a stable verdict unstable. The first word is the verdict and
    /// the rest is commentary — this is the part that was read.
    pub said: String,
    /// **WHAT THE JUDGE WENT ON TO SAY AFTER ITS VERDICT**, and [`None`] where it said only the
    /// word.
    ///
    /// # ⚠⚠⚠⚠⚠ The field that exists because a refusal loop could not be diagnosed by anybody
    ///
    /// [`said`](Self::said) above is the verdict and the rest was **read and thrown away** — and the
    /// rest is the REASON. Register item 461, measured by failing to measure: a live run was refused
    /// NINE times over seventeen iterations, and the only record anywhere was one fixed sentence,
    /// *"an independent process shown the same disagreed"*, nine times over. Nothing on disk, nothing
    /// in the walk, nothing in the agent's next prompt. **The round sent to pay item 449 was
    /// instructed to measure why the check was refusing before changing anything, and against the
    /// shipped product that instruction could not be carried out at all.**
    ///
    /// ⚠⚠ **THE FIRST-WORD RULE ITSELF IS RIGHT AND STAYS.** It is measured (a model answers some
    /// dialogs with a bare verdict and others with a paragraph) and nothing here loosens it: the
    /// verdict is still decided by the first word alone, and a judge that explains itself changes no
    /// verdict by doing so. What changes is that the paragraph is KEPT beside the word instead of
    /// dropped.
    ///
    /// ⚠ **ONE LINE, and that is a rule rather than a length.** What a walk carries is a line a
    /// person reads, so this is the first line of whatever followed the verdict — the reply is
    /// trimmed FIRST, so a judge that puts its reason on the line below the word (`NO\nBecause …`)
    /// is read exactly like one that puts it beside (`NO because …`). Inventing a byte ceiling here
    /// would be a number nobody chose; a line is the unit the reader already has.
    ///
    /// # ⚠⚠⚠⚠⚠ AND THAT PARAGRAPH WAS RIGHT ABOUT THE RULE AND WRONG ABOUT THE READER — item 517
    ///
    /// *"A number nobody chose"* is exactly what it shipped. The line it took was a rendered ROW,
    /// off a pane `JudgeSpec::PANE` columns wide, so the rule *one line* meant **the first
    /// eighty-odd characters, cut mid-word** — a byte ceiling after all, inherited from a pane's
    /// geometry instead of being chosen, and invisible because nothing in the field said it had been
    /// cut. Measured on a live loop: four refusals over one criterion, every record of why severed
    /// at the same broken word.
    ///
    /// The rule is unchanged and no ceiling was invented to replace it. What changed is underneath:
    /// `spoke` reads the pane's LOGICAL lines, so *one line* is now one line **the judge wrote**.
    /// The boundary is the judge's own newline — stated here, rather than whatever the terminal did
    /// to fit the sentence on a screen nobody sized for this.
    ///
    /// ⚠⚠ **THE RESIDUE THAT CREATES, stated rather than left to be discovered**: the 80 columns
    /// were also, accidentally, the only thing bounding this field, so it is now **as long as the
    /// judge's line**. A checker that answers in one enormous unbroken line puts that whole string
    /// where a person reads one. Accepted deliberately and on this crate's own terms — nothing is
    /// LOST, which is the direction every rule around here fails in, and this crate's `report`
    /// module answers a bound it could not avoid by REPORTING it rather than by inventing one.
    /// If a ceiling is ever wanted it has to be chosen, named, and made to say
    /// so when it bites; inheriting one from a pane is what this item was.
    pub explained: Option<String>,
    /// How long the agent stood blocked waiting for it.
    pub took: Duration,
}

/// ⛔⛔⛔ **WHY A CHECK SAID NOTHING THIS RUN COULD READ** — register item 593.
///
/// # The decision was right and the REPORT was not
///
/// [`asked_of_another`]'s own doc states the safety property and states it correctly: *no argv, no
/// lifecycle, a spawn that failed, a process that outran `within`, a run that ended underneath, or
/// a reply whose first word is not a verdict — all of them answer `None`, and the CALLER decides
/// what nothing means.* **Silence is never a yes.** Nothing about that changes here.
///
/// What that collapse cost is a different thing: **a person told only *the checker was silent* has
/// no way to act.** A checker that would not start wants its argv looked at; one that outran its
/// bound wants a longer `within` or a faster judge; one that answered prose wants its prompt fixed
/// — three remedies behind one word. Measured 2026-08-22 on this repository's own loop: a run
/// converged carrying *"the check said nothing this run could read — it would not start, outran its
/// bound, or answered something that is not a verdict"*, and the round that read it could not tell
/// which, so the milestone rested on the working agent's own word for a reason nobody could fix.
///
/// ⚠⚠⚠⚠⚠ **THIS IS R358's RULE, WHICH THIS CRATE ALREADY PAID FOR ONCE**: *when a diagnostic
/// cannot know, it must not claim* — and the remedy there was not a cleverer check but an HONEST
/// one, three findings instead of one word covering three. Counting the exits of one function found
/// **six**, not three.
///
/// ⚠⚠ IT IS NOT A VERDICT AND MUST NEVER BE ROUTED ON. `crate::outer::Checked` is what the document
/// matches; this says why the driver could not give it one, and it belongs beside that word exactly
/// as `Checked`'s own `explained` field does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unheard {
    /// The caller named no checker at all — an empty argv, or a criterion that is only whitespace.
    ///
    /// ⚠ Reachable from [`judges`] and NOT from the milestone check, whose caller answers
    /// `Checked::NotAsked` before it gets here. Two doors, one function, and only one of them
    /// screens this out — which is why it is a variant rather than an unreachable case.
    Unasked,
    /// **THIS HOST CANNOT OPEN A PANE**, so nothing could be asked of anybody. A fact about the
    /// deployment and not about the checker — the distinction
    /// [`Stopped::Unsupported`](crate::driver::Stopped) draws one door over.
    NoPane,
    /// **THE CHECKER WOULD NOT START**, carrying what the spawn said. The one item 593 suspected
    /// and could not confirm.
    Unstarted(String),
    /// **IT WAS ASKED AND THE WAIT DID NOT END IN AN ANSWER**, carrying how that wait ended — it
    /// outran its bound, the run was stopped underneath it, or its pane went in a way
    /// [`crate::completion::Over`] has a word for.
    ///
    /// ⚠ The word is that type's, so this never spells one: a second vocabulary for how a wait
    /// ended is the two-authorities defect this crate keeps naming.
    Unfinished(crate::completion::Over),
    /// **ITS PANE COULD NOT ACCOUNT FOR WHAT IT HANDED BACK** — the retained history had evicted
    /// lines, or the numbering changed underneath the read, so the verdict may not be the first
    /// word of what the judge actually wrote.
    Unaccountable,
    /// **IT ANSWERED, AND WHAT IT SAID IS NOT A VERDICT**, carrying the first word it did say.
    ///
    /// ⚠ The word is kept because it is the whole diagnosis: `Error:` is a broken checker, `Yes,`
    /// with a comma is a prompt that needs tightening, and an empty string is a judge that printed
    /// nothing at all. One remedy each.
    NotAVerdict(String),
}

/// ⛔⛔⛔⛔⛔ **WHICH OF THE TWO SILENCES THIS IS** — register item 741, and the word a document
/// routes a silence's DISPOSITION on.
///
/// # ⚠⚠⚠⚠⚠ Measured: the loop's own sentence and its behaviour disagreed
///
/// `crate::outer::Checked::Silent` says *"Silence is not agreement"*, and across this repository's
/// whole run log a silent check left `judging` by **exactly the doors an agreeing one did** —
/// `Reflecting` 15, `Closing` 2, `Stopping` 2, against an agreement's `Reflecting` 96, `Closing`
/// 11, `Stopping` 1 — while `Disputing`, the one door that buys another turn, was reached by a
/// silence **zero** times in nineteen. Two runs banked a milestone nothing had verified.
///
/// # ⚠⚠⚠ Why the split is THIS one, and why it needed no new mechanism
///
/// [`Unheard`] already carries six arms, and they divide cleanly along the line a remedy needs:
/// five of them mean **no verdict was ever produced** — nothing was asked, no pane, it would not
/// start, the wait ended without an answer, the pane could not account for what it handed back —
/// and one means **it answered, and what it said is not a verdict.** Asking again is the remedy for
/// the first and is useless for the second, which is why one word could not carry both.
///
/// **Measured over the same nineteen**: [`Unheard::NotAVerdict`] 15, [`Unheard::Unfinished`] 4
/// (`NotYet` ×3, `RunEnded` ×1). The other four arms did not occur.
///
/// ⚠⚠ **WHAT THIS DOES NOT SPLIT, STATED RATHER THAN HIDDEN.** Of those 15 unreadable answers, six
/// first words — `Permission` ×3, `You've` ×2, `Trim` — say the checker was stopped by a dialog, a
/// usage limit or a hook **before it could judge at all**, which is [`Unanswered`](Self::Unanswered)
/// in every sense but the one this type can see. [`Unheard::NotAVerdict`] keeps only the FIRST WORD
/// of the reply, so nothing downstream can tell that from a verdict in the wrong shape. Registered
/// as its own item rather than guessed at here: a needle list over somebody's prose is exactly the
/// widening `debt_loop.scxml`'s own `service_needles` comment refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Silence {
    /// **NO VERDICT WAS EVER PRODUCED** — the checker was not asked, could not be started, did not
    /// finish, or its pane could not account for the answer. Nothing is known about the milestone,
    /// and **asking again is a remedy**: the fault is in the asking, not in the answer.
    Unanswered,
    /// **IT ANSWERED, AND WHAT IT SAID IS NOT A VERDICT.** Asking again gets the same shape, so the
    /// remedy is the checker's PROMPT or its program — never a retry.
    Unreadable,
}

impl Silence {
    /// Every arm, so the runs that produce them and the document that disposes of them are one
    /// list — [`crate::outer::Checked::ALL`]'s rule, one level down.
    pub const ALL: [Self; 2] = [Self::Unanswered, Self::Unreadable];

    /// **THE WORD THIS DRIVER PUBLISHES** as `_event.data.silence`.
    ///
    /// ⚠ A word and never an empty string, for `Checked::wire_str`'s measured reason: this
    /// datamodel is Lua, where `''` is TRUE.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Unanswered => "unanswered",
            Self::Unreadable => "unreadable",
        }
    }
}

impl Unheard {
    /// ⛔⛔⛔⛔ **WHICH OF THE TWO SILENCES THIS ONE IS** — register item 741.
    ///
    /// ⚠⚠ The `match` is EXHAUSTIVE and has no catch-all on purpose: a seventh way of hearing
    /// nothing does not slip into whichever class happens to be listed last, it fails to compile
    /// until somebody says which remedy it wants. That is this workspace's rule that an
    /// unclassified thing is a RED rather than a pass, taken at the one place the classification
    /// happens.
    #[must_use]
    pub const fn silence(&self) -> Silence {
        match self {
            Self::Unasked
            | Self::NoPane
            | Self::Unstarted(_)
            | Self::Unfinished(_)
            | Self::Unaccountable => Silence::Unanswered,
            Self::NotAVerdict(_) => Silence::Unreadable,
        }
    }

    /// **WHAT A READER OF THE RUN SHOULD DO ABOUT IT** — prose, and deliberately not the arm's own
    /// name, exactly as `crate::outer::Checked::describe` is.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Unasked => "no checker was named, so nothing was asked".to_owned(),
            Self::NoPane => {
                "this host cannot open a pane, so no checker could be started at all — the remedy \
                 is the deployment's, not the checker's"
                    .to_owned()
            }
            Self::Unstarted(why) => {
                format!("the checker would not start ({why}) — look at the argv it was given")
            }
            Self::Unfinished(over) => {
                format!(
                    "the checker was started and never answered: the wait ended {over:?} — give it \
                     longer, or a faster judge"
                )
            }
            Self::Unaccountable => {
                "its pane could not account for what it handed back, so the first word may not be \
                 the judge's own — the verdict was discarded rather than guessed at"
                    .to_owned()
            }
            Self::NotAVerdict(said) => {
                format!(
                    "the checker answered {said:?}, which is not YES or NO — fix its prompt or its \
                     program, because nothing here can turn that into a verdict"
                )
            }
        }
    }
}

/// Ask `spec`'s agent whether `criterion` holds of `question`.
///
/// [`Err`] when there is no lifecycle to spawn into, the judge could not be started, it did not
/// finish inside [`JudgeSpec::within`], the run ended underneath, or its first word was not a
/// verdict. Every one of those is *this judge said nothing* — see the module doc for why that must
/// not be read as either answer, and [`Unheard`] for why they are told apart.
///
/// ⚠ NO `#[must_use]` HERE, and its absence is the type doing the work: `Result` carries that
/// attribute itself, so the reason can no longer be dropped without the compiler saying so —
/// which is precisely what register item 593 was about one layer up. Clippy said so first.
pub fn judges(
    panes: &dyn PaneAccess,
    run: &RunContext,
    criterion: &str,
    question: &Question,
    spec: &JudgeSpec,
) -> Result<Judgement, Unheard> {
    if criterion.trim().is_empty() {
        return Err(Unheard::Unasked);
    }
    // ⚠ NO DIRECTORY, and that is a decision rather than an omission — register item 710. What this
    // judge is asked about is a DIALOG that is already rendered into the question: the whole of the
    // evidence is the text, there is nothing on a filesystem for it to open, and pointing it at a
    // repository would suggest otherwise. The milestone check is the caller with something to read.
    asked_of_another(
        panes,
        run,
        &spec.argv,
        None,
        &render(criterion, question),
        spec.within,
    )
}

/// **PUT ONE YES-OR-NO QUESTION TO A PROCESS NOBODY IN THIS RUN IS**, bounded, and hand back what
/// its first word was — or [`None`] where it said nothing this can read.
///
/// # ⚠⚠⚠⚠ Why this is its own function, with two callers and a name of its own
///
/// It is the whole of what makes a verdict INDEPENDENT, and independence is the property two
/// different questions in this crate need for the same measured reason:
///
/// * a dialog's meaning cannot be told from its wording ([`judges`], and the module doc's two
///   captured dialogs that ask the identical question);
/// * **a milestone cannot be certified by the agent that worked on it** (register item 428): the
///   literature it cites measured 92 of 100 runs recorded `succeeded` where `succeeded` meant *the
///   branch was pushed*, with 18 of the 100 being repairs of runs already recorded that way. The
///   remedy is named twice and is the same both times — *an independent check, **not the model's own
///   say-so***, and *a DIFFERENT agent, in a NEW session, shown only the artifact*.
///
/// A fresh process in a fresh pane, handed one rendered question and nothing else, is exactly that
/// shape. What it must NOT become is a second implementation of it — this crate has paid for two
/// answers to one question often enough to name it a class.
///
/// # ⚠⚠⚠ Every silence is [`None`], and that direction is the safety property
///
/// No argv, no lifecycle, a spawn that failed, a process that outran `within`, a run that ended
/// underneath, or a reply whose first word is not a verdict — **none of them is a verdict**, and
/// the CALLER decides what nothing means. **Silence is never a yes**, and it is never a no either:
/// a `false` here would look exactly like a considered verdict against, which is a different fact.
///
/// ⚠⚠⚠⚠⚠ **AND SINCE REGISTER ITEM 593 THEY ARE TOLD APART** — [`Unheard`]. The decision is
/// unchanged (every one of them is still *no verdict*); what changed is that the caller learns
/// WHICH, because the six have six different remedies and a person handed one word had none of
/// them. ⚠ `Result` and not `Option`, so a caller cannot go on treating the absence as one thing:
/// the type makes the reason impossible to drop silently.
pub fn asked_of_another(
    panes: &dyn PaneAccess,
    run: &RunContext,
    argv: &[String],
    cwd: Option<&std::path::Path>,
    question: &str,
    within: Duration,
) -> Result<Judgement, Unheard> {
    let (reply, took) = said_by_another(panes, run, argv, cwd, question, within)?;
    verdict_in(&reply, question, took)
}

/// **SPAWN `argv`, HAND IT `question`, AND ANSWER WHAT IT SAID** — everything
/// [`asked_of_another`] does except deciding what the words MEAN.
///
/// # ⚠⚠⚠⚠⚠ Why this is its own function
///
/// The judge is not the only thing that will ever ask a second agent one question and read one
/// reply. `context_review.scxml`'s `asking` state has been waiting for exactly this since it was
/// written — its own prose says *"no second agent is wired"* — and register item 502 is the bill:
/// its economic door has never been walked by a run because nothing can produce `ask.done`.
///
/// ⚠⚠ **THE SPLIT IS AT THE MEANING, WHICH IS WHERE THE TWO CALLERS DIFFER AND NOWHERE ELSE.** A
/// judgement is a FIRST WORD (`YES`/`NO`) with an explanation under it; a review's answer is A
/// LINE. Everything before that — an empty argv, a host with no lifecycle, a spawn that failed, a
/// process that outran `within`, a run that ended underneath, a pane whose history was lost — is
/// the same question with the same six answers, and [`Unheard`] already names them.
///
/// ⚠ **SO THIS IS A REFACTOR AND NOT A NEW BEHAVIOUR.** Every early return below stood in
/// `asked_of_another` and returns the same variant it did; what moved is only where the boundary
/// is drawn. The judge's own gates are what say so.
///
/// ⚠⚠ **IT HANDS BACK THE CLOCK TOO**, because the clock starts at the spawn and only this side of
/// the split can see it — a caller that timed the call from outside would be timing its own
/// bookkeeping as well ([`Judgement::took`] is what that number is for).
pub(crate) fn said_by_another(
    panes: &dyn PaneAccess,
    run: &RunContext,
    argv: &[String],
    cwd: Option<&std::path::Path>,
    question: &str,
    within: Duration,
) -> Result<(String, Duration), Unheard> {
    if argv.is_empty() {
        return Err(Unheard::Unasked);
    }
    let Some(life) = panes.lifecycle() else {
        return Err(Unheard::NoPane);
    };
    let mut argv = argv.to_vec();
    argv.push(question.to_owned());

    let began = Instant::now();
    // ⛔⛔⛔⛔⛔ **IN THE DIRECTORY THE CALLER NAMED, AND THAT IS REGISTER ITEM 710.** This used to
    // call `spawn`, whose documentation said a pane with no directory *takes the daemon's*. It does
    // not: it lands in `$HOME`. So the one process in this system whose job is to VERIFY somebody
    // else's work was started where none of that work is — measured 2026-08-26, a checker at
    // `/home/coin` answering about a repository at `/home/coin/scxml-core-engine`, whose files it
    // could not have opened had it tried.
    //
    // ⚠⚠ The answer it gave was RIGHT, and that is the sharper half of the finding: it read the
    // text it was shown and agreed, correctly. **A verdict that could only come from the claimant's
    // own account is not an independent check** — it is item 428's defect wearing the remedy's
    // clothes. Pointing the process at the work is what makes reading possible; what it reads is
    // then between it and the question.
    //
    // ⚠ `None` stays honest: a caller with nothing on disk to point at (a dialog judge) says so, and
    // the pane lands wherever a pane with no opinion lands.
    let pane = match life.spawn_in(&argv, cwd, JudgeSpec::PANE.0, JudgeSpec::PANE.1) {
        Ok(pane) => pane,
        // ⚠ THE SPAWN'S OWN SENTENCE, not a word this function invents: item 593 suspected a cwd
        // the checker could not read and had no way to confirm it, because the one thing that knew
        // was thrown away here.
        Err(why) => return Err(Unheard::Unstarted(why.to_string())),
    };
    // From here every exit path closes the pane. A judge left running would hold a pty and a
    // process for the rest of the run, once per blocked turn.
    // ⚠ NO SILENCE BOUND. A judgement is one short-lived peer answering one question, and this call
    // already treats everything but `Yes` as *no verdict came back* — see
    // [`Over::Silent`](crate::completion::Over::Silent)'s own count of this site.
    let over = Completion::new(DoneWhen::Exits).wait(panes, pane, within, None, run);
    let reply = spoke(panes, pane);
    life.close(pane);

    if over != Over::Yes {
        // ⚠ THE WAIT'S OWN WORD travels: `Over` already tells an expired bound from a stopped run
        // from a peer that went, and re-spelling that here would be a second vocabulary for one
        // fact — the defect this crate names every time it finds two authorities.
        return Err(Unheard::Unfinished(over));
    }
    // ⚠⚠⚠⚠⚠ THE ADDRESS COULD NOT ACCOUNT FOR WHAT IT HANDED BACK, SO THERE IS NO VERDICT HERE.
    //
    // `lost` means the pane's retained history evicted lines before this read, and `restarted` means
    // the numbering changed underneath it — in either case the text may not open where the reply
    // does. That matters more here than almost anywhere: the verdict is the FIRST WORD, so a reply
    // read with its opening missing does not produce a wrong reason, it produces **a wrong verdict**
    // — the first word of the middle of a sentence, put through the YES/NO match. Silence is the
    // safe direction and this function is built on it, so an unaccountable read takes it.
    let Some(reply) = reply else {
        return Err(Unheard::Unaccountable);
    };
    Ok((reply, began.elapsed()))
}

/// **WHAT A REPLY MEANS TO A JUDGE** — the half [`said_by_another`] deliberately does not decide.
///
/// `question` is the one that was asked, and it is needed here rather than only at the spawn: a
/// checker that ECHOES its argv sends this run's own prompt back, and the echo has to be cut off
/// what the judge is quoted as saying. `took` is the asking's own clock, handed over rather than
/// re-taken — see [`said_by_another`].
fn verdict_in(reply: &str, question: &str, took: Duration) -> Result<Judgement, Unheard> {
    // ⚠⚠ SPLIT ONCE, so the verdict and what follows it are read off the SAME reply at the same
    // moment. Taking the first word here and re-scanning the text for the rest would be two readers
    // of one string, free to disagree about where the word ended.
    let trimmed = reply.trim_start();
    let (first, rest) =
        trimmed.split_at(trimmed.find(char::is_whitespace).unwrap_or(trimmed.len()));
    if first.is_empty() {
        // ⚠ AN EMPTY FIRST WORD IS A CHECKER THAT PRINTED NOTHING, and it is kept as such rather
        // than folded into the arm below: an empty string in that variant would render as `""`,
        // which reads like a judge that said something unquotable instead of one that said nothing.
        return Err(Unheard::NotAVerdict(String::new()));
    }
    let said = first.trim_matches(|c: char| !c.is_ascii_alphabetic());
    // ⚠⚠⚠⚠⚠ AND WHAT IT WENT ON TO SAY — see [`Judgement::explained`]. TRIMMED BEFORE the line is
    // taken, so a judge that puts its reason under the verdict reads the same as one that puts it
    // beside: `NO\nBecause …` and `NO because …` both answer *Because …*.
    //
    // ⚠⚠⚠⚠ **AND THE QUESTION WE SENT IS CUT OFF FIRST, BECAUSE AN ECHO IS NOT A STATEMENT.** The
    // rendered question travels as the LAST ARGV, which a print-mode CLI reads positionally and
    // never prints — but a checker spelled `/bin/echo NO` prints its arguments, so everything after
    // the verdict is this run's own prompt coming back. Quoting that into a walk as *"it said"*
    // would put the product's words in the judge's mouth, on a line a person acts on. This crate
    // already refuses the same shape one door over, where a readiness marker found in what was
    // TYPED is never evidence ([`ReadyWhen::Prints`](crate::readiness::ReadyWhen::Prints)).
    //
    // ⚠ Cut at the question's FIRST LINE, which is where the echo begins wherever it lands — a
    // checker that echoes on the verdict's own line and one that echoes below it are the same case.
    // The residue, stated: a judge that opens by quoting that line loses everything after it. That
    // is a false SILENCE, which is the safe direction — the same one the first-word rule takes.
    let opening = question.lines().next().unwrap_or_default();
    let spoken = match opening.is_empty() {
        true => rest,
        false => rest.split(opening).next().unwrap_or_default(),
    };
    let explained = spoken
        .trim()
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned);
    let holds = match said.to_ascii_uppercase().as_str() {
        "YES" => true,
        "NO" => false,
        // ⚠ Anything else is NOT a no. A judge that replied with a sentence, an error, or a
        // refusal has not answered, and the caller must be able to tell that from a measured
        // `false` — a `false` here would look exactly like a judge that considered the dialog and
        // decided against, which is a different fact.
        //
        // ⚠⚠ THE WORD IT DID SAY TRAVELS, which is what makes this actionable: `Error:` names a
        // broken checker, `Yes,` names a prompt that needs tightening, and the two used to arrive
        // as the same silence.
        _ => return Err(Unheard::NotAVerdict(said.to_owned())),
    };
    Ok(Judgement {
        holds,
        said: said.to_owned(),
        explained,
        took,
    })
}

/// **WHAT THE JUDGE SAID, READ BY ADDRESS** — its pane's logical lines from birth, or [`None`] where
/// the read could not account for itself.
///
/// # ⚠⚠⚠⚠⚠ Why this is not `pane_full_text`, which is what it used to be — register item 517
///
/// A pane's full text is the RENDERING, and a rendering is broken at the pane's width. This one is
/// [`JudgeSpec::PANE`] columns wide, so a judge that explained itself in a sentence longer than that
/// had its reason **cut mid-word** by the terminal before any rule in this file ran.
/// [`Judgement::explained`] then took *the first line*, which meant the first eighty-odd characters.
/// Measured on a live loop: a run was refused four times over one criterion and every record of why
/// stopped at the same broken word.
///
/// This is [`report`](crate::report)'s measurement, and its conclusion, applied one door over — *the
/// rendering is the degradation; read the address* — and register item 270's argument verbatim:
/// asking **"is the reason the whole ROW?"** is asking about a width nobody chose. A LOGICAL line is
/// what the child wrote, and reflow is defined as preserving it.
///
/// # ⚠⚠⚠ The cursor is BIRTH, not a mark, and that removes a race rather than a line of code
///
/// [`Since::mark`](crate::report::Since::mark) is the right reader when a pane pre-exists its
/// stimulus — the caller marks, then injects. Here the pane IS the stimulus: it is spawned for this
/// one question, nothing else ever writes to it, and the child can have printed and exited before
/// `spawn` returns. A mark taken after that would sit PAST the whole reply and read empty. `0` is
/// the pane's first line, so *everything this pane has produced* is exactly the judge's answer.
///
/// # ⚠⚠ The unfinished line is included, and this is the one caller that has earned it
///
/// [`Produced::partial`](crate::report::Produced::partial) is withheld from readers who cannot show
/// the child has stopped. This one can: [`DoneWhen::Exits`] is what was waited on. So a judge
/// spelled `printf 'NO because …'` — no trailing newline, which is a whole class of small
/// checkers — is read, where a line-only reader would have called it silent.
fn spoke(panes: &dyn PaneAccess, pane: sprag_terminal::PaneId) -> Option<String> {
    let Some(since) = panes
        .output_lines()
        .and_then(|stream| stream.pane_lines_since(pane, 0))
    else {
        // ⚠ THE DEGRADATION, named rather than silently equivalent — a host that cannot number its
        // lines has only the rendering to offer, and this is the behaviour every judgement had
        // before the address existed. The width defect above is present in this arm and cannot be
        // fixed from here; what a host owes to escape it is `PaneOutputLines`.
        return panes.pane_full_text(pane);
    };
    if since.lost > 0 || since.restarted {
        return None;
    }
    let mut said = since.lines.join("\n");
    if !since.partial.is_empty() {
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(&since.partial);
    }
    Some(said)
}

/// The question put to the judge: the author's criterion, the dialog, its options, and **the one
/// distinction the options alone do not carry**.
///
/// # ⚠⚠⚠ THE OPTIONS ARE BOTH THE SIGNAL AND THE CONFUSION, MEASURED AT BOTH POLES
///
/// ```text
///                        owner's label   with options   without options
/// permission to write            NO       F T T           F F F
/// which files to delete         YES       T T T           F T F
/// ```
///
/// **Neither pole works.** Withholding them removes every false YES and takes the target case with
/// it — `Which report files should I delete?` is two lines, and without its options there is
/// nothing in it that says the outcomes differ. Including them recovers the target and starts
/// reading `1. Yes / 2. Yes, allow all edits / 3. No` as *a set of options on offer*, which is
/// what the criterion asks about.
///
/// So the options stay and the prompt names the difference they cannot show on their own: a menu
/// whose entries are all variations of **yes-or-no about one act** is a permission, however many
/// entries it has. That sentence is doing the work, not the presence of the list.
fn render(criterion: &str, question: &Question) -> String {
    let mut out = String::new();
    out.push_str(
        "An AI agent working in a terminal has stopped and is showing the dialog below.\n\n\
         Decide whether this statement is true of it:\n",
    );
    out.push_str(criterion);
    out.push_str(
        "\n\nOne distinction the options below do not show on their own: when every option is a \
         variation of yes-or-no about ONE action the agent has already settled on — accept it, \
         accept it and stop asking, decline it — that is a permission, however many entries it \
         has. Options that lead to materially different outcomes are a choice.\
         \n\nReply with exactly one word: YES or NO. Do not explain.\n\nThe dialog:\n",
    );
    for line in &question.asked {
        out.push_str(line);
        out.push('\n');
    }
    for choice in &question.choices {
        out.push_str(&format!("  {}. {}\n", choice.number, choice.label));
    }
    out
}

/// What a judged refusal did, beside [`Screened`](crate::screen::Screened) and for its reason: a
/// decision taken on somebody's behalf has to be reportable in the run's own vocabulary.
///
/// ⚠ It carries the CRITERION where `Screened` carries a rule's quote. Neither is the dialog — both
/// are the author's own words for why this dialog was theirs — and a reader auditing a run needs to
/// know which of the two authorities acted, because only one of them is re-readable in the
/// document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirected {
    /// The question as it stood when it was turned down.
    pub question: Question,
    /// What the rule that claimed it is called — the author's own name for this decision.
    pub rule: String,
    /// The criterion the judge was asked about.
    pub criterion: String,
    /// The verdict word the judge replied with.
    pub said: String,
    /// What the agent was told to do instead.
    pub told: String,
    /// PTY bytes the whole act cost: [`REFUSES`] and the redirect together.
    pub bytes: u64,
}

impl Redirected {
    /// ONE LINE for the run's journal.
    ///
    /// ⚠ It says **refused** out loud, `Screened::describe`'s reason: the act's name describes the
    /// judging and not the consequence, and the consequence — a tool call the agent asked for did
    /// not happen — is the part a person auditing this run needs.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "rule {:?} judged {:?} ({}), refused with {REFUSES} and told the agent: {:?}",
            self.rule, self.criterion, self.said, self.told,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{KeyStroke, PaneError, PaneRow, Written};
    use sprag_detect::Choice;
    use sprag_terminal::PaneId;
    use std::sync::{Arc, Mutex};

    /// A host with panes but NO LIFECYCLE — so a judgement that tried to spawn would answer `None`
    /// for that reason and not for the one under test.
    ///
    /// ⚠ `pane_full_text` panics rather than answering, because reaching it means a judge was
    /// spawned and waited on. The tests below are about the paths that must ask NOBODY, so being
    /// reached at all is the failure.
    struct AsksNobody;

    impl PaneAccess for AsksNobody {
        fn pane_ids(&self) -> Vec<PaneId> {
            Vec::new()
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            None
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            None
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            unreachable!("a judgement that asks nobody must not read a judge's reply")
        }
        fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
    }

    fn question() -> Question {
        Question {
            asked: vec!["Do you want to create DESIGN.txt?".to_owned()],
            choices: vec![
                Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                Choice {
                    number: 3,
                    label: "No".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// The judge is shown the author's words, the dialog, its options, **and the distinction the
    /// options cannot carry**.
    ///
    /// ⚠⚠⚠ That last clause is the assertion worth having. Measured at both poles, the options
    /// alone recover the target case and cost false YES on permissions; the sentence about
    /// yes-or-no menus is what separates them. A change that dropped it would pass every other
    /// test in this module and quietly return the judge to a 2-in-3 false YES rate — see
    /// [`render`].
    #[test]
    fn the_question_carries_the_criterion_the_dialog_its_options_and_the_distinction() {
        let put = render("going ahead would commit a design decision", &question());
        assert!(put.contains("going ahead would commit a design decision"));
        assert!(put.contains("Do you want to create DESIGN.txt?"));
        assert!(
            put.contains("YES or NO"),
            "the verdict vocabulary is stated: {put}"
        );
        assert!(put.contains("1. Yes"), "the options are the signal: {put}");
        assert!(put.contains("3. No"), "{put}");
        assert!(
            put.contains("variation of yes-or-no"),
            "and the one thing the options cannot say about themselves: {put}",
        );
    }

    /// **AN EMPTY CRITERION IS HOW A RUN DECLINES A JUDGE**, and it must cost nothing.
    ///
    /// ⚠ Asserted through a `PaneAccess` with NO lifecycle, so a judge that tried to spawn would
    /// panic on the `expect` rather than quietly returning `None` for the wrong reason — the check
    /// is that nothing was reached, not merely that the answer was `None`.
    #[test]
    fn an_empty_criterion_asks_nobody() {
        let spec = JudgeSpec {
            argv: vec!["claude".to_owned(), "-p".to_owned()],
            within: Duration::from_secs(30),
        };
        for criterion in ["", "   ", "\n"] {
            assert_eq!(
                judges(
                    &AsksNobody,
                    &RunContext::uncancellable(),
                    criterion,
                    &question(),
                    &spec
                ),
                Err(Unheard::Unasked),
                "⚠⚠ {criterion:?} declines the judge entirely — and since register item 593 it \
                 says so with the word for *nobody was asked*, which is a DECISION the author took \
                 rather than a checker that went wrong",
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **AND THE SIX FALL INTO EXACTLY TWO REMEDIES, WHICH IS WHAT A DOCUMENT DISPOSES
    /// OF** — register item 741.
    ///
    /// # ⚠⚠⚠⚠ Why the classification needs a gate of its own
    ///
    /// The document's gate feeds `silence` in on the event, the way the driver does, so it measures
    /// the DISPOSITION and cannot see this function at all: collapse every arm to one class and
    /// that gate stays green while every run gets the wrong remedy. That is this workspace's rule
    /// that a value handed to a helper measures the helper — so the classification is walked here,
    /// over [`Unheard`]'s own arms, with both classes standing up.
    ///
    /// ⚠⚠ **BOTH SIDES ARE ASSERTED, NOT JUST THE SPLIT.** A `match` that answered `Unanswered` to
    /// everything and one that answered `Unreadable` to everything are both wrong, and only naming
    /// which arm belongs where catches either. Measured on this repository's whole run log:
    /// `NotAVerdict` 15, `Unfinished` 4 — the two classes are the population, not a hypothesis.
    #[test]
    fn every_way_of_hearing_nothing_falls_into_one_of_two_remedies() {
        // ⚠ THE WHOLE VOCABULARY, spelled out: an arm added to `Unheard` fails to compile against
        // this list until somebody says which remedy it wants, which is the same refusal
        // `Unheard::silence`'s exhaustive `match` makes one layer down.
        let every: [(Unheard, Silence); 6] = [
            (Unheard::Unasked, Silence::Unanswered),
            (Unheard::NoPane, Silence::Unanswered),
            (
                Unheard::Unstarted("no such program".to_owned()),
                Silence::Unanswered,
            ),
            (
                Unheard::Unfinished(crate::completion::Over::NotYet),
                Silence::Unanswered,
            ),
            (Unheard::Unaccountable, Silence::Unanswered),
            (
                Unheard::NotAVerdict("Permission".to_owned()),
                Silence::Unreadable,
            ),
        ];
        for (heard, want) in &every {
            assert_eq!(
                heard.silence(),
                *want,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 741: {heard:?} was classed as the wrong kind of silence, so \
                 the document will put the wrong remedy to the agent — *ask the checker again* at \
                 a checker that answered prose, or *fix the prompt* at one that never ran. Those \
                 are not degrees of the same advice, they are opposite work",
            );
        }
        // ── AND BOTH CLASSES ARE ACTUALLY PRODUCED, or the split is a constant ──────────────
        for class in Silence::ALL {
            assert!(
                every.iter().any(|(_, is)| *is == class),
                "⚠⚠⚠ THE CONTROL: {class:?} must be reachable from some arm, or this gate is \
                 asserting a function that answers one word and the document's second clause is \
                 dead prose nobody can reach",
            );
        }
        assert_eq!(
            Silence::ALL.map(Silence::wire_str),
            ["unanswered", "unreadable"],
            "⚠⚠ and the words are the ones `ai_loop.scxml` compares against — the document spells \
             `_event.data.silence == 'unanswered'`, and a renamed word here would take the else \
             branch for every silence in silence",
        );
    }

    /// ⛔⛔⛔ **A CHECK THAT SAID NOTHING SAYS *WHICH* NOTHING, AND THE SIX ARE TOLD APART** —
    /// register item 593, and R358's rule paid a second time: *when a diagnostic cannot know, it
    /// must not claim.*
    ///
    /// # What one word cost, measured on this repository's own loop
    ///
    /// A run converged carrying *"the check said nothing this run could read — it would not start,
    /// outran its bound, or answered something that is not a verdict"*, and the round reading it
    /// **could not tell which**, so the milestone rested on the working agent's own word for a
    /// reason nobody could act on. Three remedies behind one word — and counting the exits of
    /// [`asked_of_another`] found six, not three.
    ///
    /// ⚠⚠⚠⚠⚠ **THE DECISION IS UNCHANGED AND THAT IS ASSERTED FIRST.** Every arm here is still *no
    /// verdict*: silence is never a yes and never a no. A round that widened the ANSWER while
    /// paying off the REPORT would have turned a broken checker into an agreement, which is the one
    /// thing this module exists to prevent — so the first assertion of every arm is `is_err`.
    ///
    /// ⚠⚠ **EACH ARM IS STAGED BY A DIFFERENT REAL FAILURE**, not by constructing the variant: a
    /// checker whose program does not exist, one that prints something that is not a verdict, and
    /// one that prints nothing at all. A gate that built `Unheard::Unstarted(…)` by hand would
    /// assert that this file can spell its own type.
    #[test]
    fn a_check_that_said_nothing_says_which_nothing() {
        // A real workspace, which is what gives every arm below a lifecycle to spawn into — the
        // one thing `AsksNobody` cannot provide, and the difference between staging a failure and
        // asserting that this host has no panes.
        let host = crate::access::WorkspacePaneAccess::new(Arc::new(Mutex::new(
            sprag_terminal::Workspace::new((80, 24)),
        )));
        let ask = |argv: Vec<String>| {
            judges(
                &host,
                &RunContext::uncancellable(),
                "anything at all",
                &question(),
                &JudgeSpec {
                    argv,
                    within: Duration::from_secs(20),
                },
            )
        };

        // ── THE CHECKER WOULD NOT START ── item 593's own prime suspect, staged: a program that is
        // not there at all is the closest a fixture can get to the cwd it could not read.
        let unstarted = ask(vec!["/nonexistent/checker-593".to_owned()]);
        assert!(
            unstarted.is_err(),
            "⚠⚠⚠ THE DECISION FIRST: a checker that never ran has not agreed. Got {unstarted:?}",
        );
        let Err(Unheard::Unstarted(why)) = &unstarted else {
            panic!(
                "⛔⛔⛔ ITEM 593: a checker whose program does not exist must be reported as one \
                 that WOULD NOT START — that is the remedy *look at the argv*, and it is a \
                 different remedy from every other silence. Got {unstarted:?}"
            );
        };
        assert!(
            !why.trim().is_empty(),
            "⚠⚠ and it carries what the spawn said, because *it would not start* without a reason \
             is the same dead end one word further in: {why:?}",
        );

        // ── IT ANSWERED, AND WHAT IT SAID IS NOT A VERDICT ──
        let prose = ask(vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf 'Error: no API key\\n'".to_owned(),
        ]);
        assert!(
            prose.is_err(),
            "⚠⚠⚠ THE DECISION FIRST: a reply that is not YES or NO is not a NO. Got {prose:?}",
        );
        assert_eq!(
            prose,
            Err(Unheard::NotAVerdict("Error".to_owned())),
            "⛔⛔⛔ ITEM 593: this checker RAN and ANSWERED, and the remedy is its prompt or its \
             program — the opposite end of the diagnosis from one that would not start. The word \
             it said travels because that word IS the diagnosis: {prose:?}",
        );

        // ── IT PRINTED NOTHING AT ALL ── the control on the arm above: same road, same exit, and
        // the reported word must be EMPTY rather than absent. A judge that printed nothing and one
        // that printed `Error` are both `NotAVerdict`, and only the payload separates them.
        let mute = ask(vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "exit 0".to_owned(),
        ]);
        assert_eq!(
            mute,
            Err(Unheard::NotAVerdict(String::new())),
            "⚠⚠⚠ a checker that printed NOTHING is still *it answered no verdict*, with an empty \
             word — and if this reads as some other arm, the read below the wait is deciding \
             something the parse should: {mute:?}",
        );

        // ── AND THE THREE ARE DISTINCT ── without this the arms above could all be one variant
        // wearing three payloads, which is the collapse this item is about, one level in.
        assert_ne!(
            unstarted, prose,
            "⚠⚠⚠⚠⚠ THE WHOLE OF ITEM 593: a checker that would not start and one that answered \
             prose must not arrive as the same thing. They did, for a whole round, and the person \
             reading it had three remedies and one word",
        );
        assert_ne!(
            prose, mute,
            "⚠⚠ and the payload has to separate the two that share an arm, or the word it carries \
             is decoration",
        );
        assert!(
            host.pane_ids().is_empty(),
            "⚠⚠ AND EVERY ARM CLOSED ITS PANE. A judge left running holds a pty and a process for \
             the rest of the run — once per silence, which is the case nobody watches: {:?}",
            host.pane_ids(),
        );
    }

    /// An empty argv is the other way a run has no judge: nobody to ask.
    #[test]
    fn a_judge_with_no_command_asks_nobody() {
        assert_eq!(
            judges(
                &AsksNobody,
                &RunContext::uncancellable(),
                "anything at all",
                &question(),
                &JudgeSpec {
                    argv: Vec::new(),
                    within: Duration::from_secs(30)
                },
            ),
            Err(Unheard::Unasked),
            "⚠⚠ an empty argv is the OTHER way a caller names no checker, and it must reach the \
             same word — a reader told *nobody was asked* should not have to know which of the two \
             fields was left empty",
        );
    }

    /// ⚠⚠⚠ **A STOPPED RUN GETS NO JUDGEMENT — AND THAT IS WHAT KEEPS `redirecting` OUT OF ITS
    /// REACH** — the half of R395's claim that lives one door over from `screening`.
    ///
    /// # ⚠⚠⚠ Why this is about a state in another file
    ///
    /// `ai_loop.scxml` has TWO states that press [`REFUSES`] into somebody's dialog: `screening`,
    /// on a quoted rule, and `redirecting`, on a judgement. R395's loop-level gate holds *a stopped
    /// run types nothing further* for the first. The second is reached only when this function
    /// answers `Some` with `holds`, and **it is reached on the pump that NOTICED** — by which time
    /// a run cancelled inside the answering wait is already over. So the sentence *"a stopped run
    /// cannot even get to the other door"* rests entirely on the arm below, and it was held by
    /// reading [`Completion::wait`] rather than by anything that runs.
    ///
    /// ⚠⚠ **IT IS A PAIR, and the control half is the first thing in this module to spawn a judge
    /// at all.** Every other gate here measures a path that asks NOBODY, so `spawn` → wait → read
    /// the reply → parse the verdict had no offline coverage: a judge that could not be started, or
    /// whose reply was read from the wrong place, would have answered `None` and looked exactly
    /// like the arm under test.
    ///
    /// ⚠ The judge is `/bin/sh`, so the pair differs in the RUN and in nothing else — same argv,
    /// same criterion, same dialog. A judge that exits instantly is also the hardest case for the
    /// claim: there is no window in which the cancel could win a race it should not have to.
    #[test]
    fn a_stopped_run_gets_no_judgement_however_fast_the_judge_answers() {
        /// Answers the verdict and exits at once — [`DoneWhen::Exits`] is what the judge waits on.
        fn instant_judge() -> JudgeSpec {
            JudgeSpec {
                argv: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf 'YES\\n'".to_owned(),
                ],
                within: Duration::from_secs(20),
            }
        }
        /// A real workspace, which is what gives the judgement a lifecycle to spawn into.
        fn host() -> crate::access::WorkspacePaneAccess {
            crate::access::WorkspacePaneAccess::new(Arc::new(Mutex::new(
                sprag_terminal::Workspace::new((80, 24)),
            )))
        }
        const CRITERION: &str = "going ahead would commit a design decision";

        // ── THE CONTROL: a live run really does get a verdict out of this judge ──
        let alive = host();
        let judged = judges(
            &alive,
            &RunContext::uncancellable(),
            CRITERION,
            &question(),
            &instant_judge(),
        );
        let Ok(verdict) = judged else {
            panic!(
                "⚠⚠⚠ the control must reach a verdict, or the arm below is silent for a reason \
                 that has nothing to do with the run: spawn, wait, read and parse all have to \
                 work. It said {judged:?}",
            );
        };
        assert!(
            verdict.holds && verdict.said.eq_ignore_ascii_case("YES"),
            "⚠⚠ and the verdict must be the one the judge actually said: {verdict:?}",
        );
        assert!(
            alive.pane_ids().is_empty(),
            "⚠⚠ AND THE JUDGE'S PANE IS GONE. One left running holds a pty and a process for the \
             rest of the run, once per blocked turn: {:?}",
            alive.pane_ids(),
        );

        // ── AND THE SAME EVERYTHING, ON A RUN THAT IS ALREADY OVER ──
        let stopped = host();
        let cancelled = RunContext::new(Arc::new(std::sync::atomic::AtomicBool::new(true)));
        let said = judges(
            &stopped,
            &cancelled,
            CRITERION,
            &question(),
            &instant_judge(),
        );
        assert!(
            said.is_err(),
            "⚠⚠⚠ a run that is over may not collect a judgement, and this is not a nicety: an \
             `Ok` here sends the loop to `redirecting`, whose act presses {REFUSES:?} into a \
             dialog the run has stopped being allowed to touch. **Silence is never a yes**, and a \
             stopped run's silence least of all. It said {said:?}",
        );
        // ⚠⚠⚠⚠⚠ **AND IT SAYS WHICH SILENCE** — register item 593, asserted here because this is
        // the one arm of it a fixture can stage deterministically. The wait is what ended without
        // an answer, so the reason has to be the WAIT's own word rather than a spawn's or a
        // parser's: a `NotAVerdict` here would mean the judge really did answer and was misread,
        // and an `Unstarted` would mean the run's cancellation was never consulted at all.
        assert!(
            matches!(said, Err(Unheard::Unfinished(_))),
            "⚠⚠⚠ ITEM 593: the reason a stopped run gets no verdict is that its WAIT did not end \
             in one, and a reader has to be told that rather than *the checker was silent*. Got \
             {said:?}",
        );
        assert!(
            stopped.pane_ids().is_empty(),
            "⚠⚠ and it leaves nothing behind either — the judge is closed on every exit path, \
             including this one: {:?}",
            stopped.pane_ids(),
        );
    }

    /// The line a run's journal carries names the criterion, the verdict and the redirect.
    #[test]
    fn a_redirect_describes_what_it_refused_and_what_it_said() {
        let line = Redirected {
            question: question(),
            rule: "design".to_owned(),
            criterion: "commits a design decision".to_owned(),
            said: "YES".to_owned(),
            told: "Reconsider and take the long-term-correct approach.".to_owned(),
            bytes: 61,
        }
        .describe();
        assert!(line.contains("commits a design decision"), "{line}");
        assert!(
            line.contains("design"),
            "the rule that fired is named: {line}"
        );
        assert!(line.contains("YES"), "{line}");
        assert!(
            line.contains("Escape"),
            "the key that refused is named: {line}"
        );
        assert!(line.contains("Reconsider"), "{line}");
    }

    /// **REGISTER ITEM 517's GATE: a reason longer than the judge's pane is read back WHOLE.**
    ///
    /// # ⚠⚠⚠⚠⚠ The pane is 80 columns, and 80 is a number nobody chose
    ///
    /// [`Judgement::explained`] used to be the first LINE of what followed the verdict, defended as
    /// *"a rule rather than a length"* — a line being the unit the reader already had, where a byte
    /// ceiling would have been invented. **The line it took was a rendered ROW.** The reply is read
    /// out of a pane [`JudgeSpec::PANE`] columns wide, so the terminal had already broken the
    /// sentence at the width, and *the first line of the reply* meant *the first eighty-odd
    /// characters, cut mid-word*.
    ///
    /// This is [`report`](crate::report)'s measurement one door over, and register item 270's
    /// argument verbatim: asking *"is the reason the whole ROW?"* is asking about a width nobody
    /// chose. The reader is now the ADDRESS — the pane's own logical lines — so what comes back is
    /// what the judge WROTE.
    ///
    /// ⚠⚠ It is driven through a REAL workspace and a REAL `/bin/sh`, not a stub, because the
    /// wrapping under test is the terminal's. A fake that handed back an unwrapped string would
    /// pass with the defect present, which is the only way this gate could be worth writing.
    #[test]
    fn a_reason_longer_than_the_judges_pane_is_read_back_whole() {
        /// The tail of [`REASON`], past the first row of an 80-column pane. Asserted separately so
        /// a failure says WHICH half arrived.
        const TAIL: &str = "variations of one yes-or-no act";
        /// What the judge says after its verdict — one logical line, deliberately longer than the
        /// pane is wide, ending in [`TAIL`].
        const REASON: &str = "because the options differ in outcome rather than being \
                              variations of one yes-or-no act";

        let host = crate::access::WorkspacePaneAccess::new(Arc::new(Mutex::new(
            sprag_terminal::Workspace::new((80, 24)),
        )));
        // The fixture, asserted rather than assumed: a reason that fits on one row would make every
        // claim below true of a case this item is not about.
        assert!(
            REASON.len() > usize::from(JudgeSpec::PANE.0),
            "⚠ the reason must OUTRUN the pane to be evidence: {} chars in {} columns",
            REASON.len(),
            JudgeSpec::PANE.0,
        );

        let judged = judges(
            &host,
            &RunContext::uncancellable(),
            "going ahead would commit a design decision",
            &question(),
            &JudgeSpec {
                argv: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    format!("printf 'NO {REASON}\\n'"),
                ],
                within: Duration::from_secs(20),
            },
        );
        let Ok(verdict) = judged else {
            panic!(
                "⚠⚠⚠ the judge must reach a verdict at all, or nothing below is about the reason. \
                 It said {judged:?}"
            );
        };
        // The CONTROL: the verdict itself is unchanged by any of this. The first-word rule is right
        // and this gate does not loosen it — a judge that explains itself decides nothing different.
        assert!(
            !verdict.holds && verdict.said.eq_ignore_ascii_case("NO"),
            "⚠⚠ the verdict is still the first word alone: {verdict:?}",
        );

        let explained = verdict.explained.as_deref().expect(
            "⚠⚠⚠ a judge that gave a reason must have one recorded — item 461's whole point",
        );
        assert!(
            explained.contains(TAIL),
            "⚠⚠⚠⚠⚠ THE REASON WAS CUT AT THE PANE'S WIDTH. What a walk carries is what a person \
             acts on, and a sentence severed mid-word at column {} reads as though the judge \
             stopped there. Wanted the tail {TAIL:?}; got {explained:?}",
            JudgeSpec::PANE.0,
        );
        assert!(
            explained.contains(REASON),
            "⚠⚠⚠⚠ and WHOLE, not merely ending right: {explained:?}",
        );
    }

    /// **THE TWO ARMS OF [`spoke`] THE GATE ABOVE CANNOT REACH**, both shipped in the same round as
    /// item 517's fix and neither exercised by a real workspace: a host with no address at all, and
    /// an address that cannot account for what it handed back.
    ///
    /// ⚠⚠⚠ The second is the one worth a test rather than a comment. `lost`/`restarted` answering
    /// [`None`] is not tidiness — the verdict is the reply's FIRST WORD, so a read whose opening was
    /// evicted does not yield a short reason, it yields **the first word of the middle of a
    /// sentence put through the YES/NO match**. That is a fabricated verdict, and the direction this
    /// whole file is built on is that silence is safer than one of those.
    ///
    /// ⚠ [`spoke`] is called directly. Going through [`asked_of_another`] would need a lifecycle to
    /// spawn into, and every one of these stubs would answer `None` for that reason instead of the
    /// one under test — the same trap [`AsksNobody`] is shaped to avoid.
    #[test]
    fn a_read_that_cannot_account_for_itself_is_no_verdict_and_no_address_is_the_degradation() {
        /// A host answering one canned [`LinesSince`], or none at all when `numbers` is false.
        struct Host {
            numbers: bool,
            since: sprag_vt::LinesSince,
        }
        impl PaneAccess for Host {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(0)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                None
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
                None
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                None
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some("THE RENDERING".to_owned())
            }
            fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
                Err(PaneError::UnknownPane(PaneId(0)))
            }
            fn output_lines(&self) -> Option<&dyn crate::access::PaneOutputLines> {
                self.numbers.then_some(self)
            }
        }
        impl crate::access::PaneOutputLines for Host {
            fn pane_lines_since(&self, _id: PaneId, _cursor: u64) -> Option<sprag_vt::LinesSince> {
                Some(self.since.clone())
            }
        }
        let since = |lost: u64, restarted: bool, partial: &str| sprag_vt::LinesSince {
            lines: vec!["NO because the judge said so".to_owned()],
            next: 1,
            lost,
            partial: partial.to_owned(),
            restarted,
        };

        // ── The CONTROL: an accountable address is read, so the arms below fail for their own
        // reason and not because this stub cannot be read at all.
        let whole = Host {
            numbers: true,
            since: since(0, false, ""),
        };
        assert_eq!(
            spoke(&whole, PaneId(0)).as_deref(),
            Some("NO because the judge said so"),
            "⚠⚠ a clean address is the judge's own lines",
        );

        // ── AND THE UNFINISHED LINE IS PART OF IT, which is what makes a checker spelled
        // `printf 'NO …'` — no trailing newline — readable rather than silent.
        let unfinished = Host {
            numbers: true,
            since: sprag_vt::LinesSince {
                lines: Vec::new(),
                next: 0,
                lost: 0,
                partial: "NO and it never pressed return".to_owned(),
                restarted: false,
            },
        };
        assert_eq!(
            spoke(&unfinished, PaneId(0)).as_deref(),
            Some("NO and it never pressed return"),
            "⚠⚠⚠ a judge that exits without a newline has still spoken — `DoneWhen::Exits` is what \
             earns this line, and dropping it would call a whole class of small checkers silent",
        );

        // ── THE ARMS. An eviction and a renumbering each mean *the text may not open where the
        // reply does*, and the first word is the verdict.
        for (what, since) in [
            ("lines were evicted before the read", since(3, false, "")),
            ("the addresses restarted underneath", since(0, true, "")),
        ] {
            let host = Host {
                numbers: true,
                since,
            };
            assert_eq!(
                spoke(&host, PaneId(0)),
                None,
                "⚠⚠⚠⚠⚠ {what}: this must be SILENCE, not a verdict read off a decapitated reply",
            );
        }

        // ── AND THE DEGRADATION, which is a host that cannot number its lines at all. It gets the
        // rendering — item 517's defect included — because that is all such a host has to offer,
        // and the point of naming it is that it is not silently equivalent to the arm above.
        let blind = Host {
            numbers: false,
            since: since(0, false, ""),
        };
        assert_eq!(
            spoke(&blind, PaneId(0)).as_deref(),
            Some("THE RENDERING"),
            "⚠⚠ a host with no address still answers — the fallback is a degradation, not a refusal",
        );
    }
}
