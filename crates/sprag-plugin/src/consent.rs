//! The answering contract — *what a run may answer when its peer stops to ASK*.
//!
//! The third of the turn's three declared contracts, and the last one that was still a hard-coded
//! rule. [`ReadyWhen`](crate::readiness::ReadyWhen) says when a turn may START,
//! [`DoneWhen`](crate::completion::DoneWhen) says what makes it OVER, and both are the caller's to
//! choose because only they know. What happens when the peer INTERRUPTS the turn with a question of
//! its own was neither declared nor chooseable: the run stopped, always, and reported the question.
//!
//! # ⚠⚠⚠ Why stopping was the right first answer, and why it is not the whole one
//!
//! An agent that stops to ask shows a bottom-anchored NUMBERED CHOICE LIST, and a numbered list
//! consumes keystrokes: what a loop types into one is not text, it is a SELECTION. Every injection
//! these plugins make ends with Enter, and [`Question::selected`] is *"where a bare Enter would
//! land, and so the answer a caller gets by doing nothing"* — so a loop that kept going would
//! confirm whatever option the agent had highlighted, which on a tool-permission dialog is an
//! approval nobody read. R365 measured that loop and stopped it.
//!
//! Stopping is honest and it is also a dead end for the unattended case: a run that must be watched
//! by a person at every dialog is a run a person may as well have driven. The gap between *"never
//! answer"* and *"answer whatever comes up"* is where this type lives, and the whole design is
//! about making the second of those unreachable.
//!
//! # ⚠⚠⚠ A TURN ASKS MORE THAN ONCE, so the contract is a LIST of clauses
//!
//! The first version of this took ONE clause, and one clause covers one question. Measured against
//! a turn shaped like a real one — an agent that runs a command and then edits a file asks *"Bash
//! command … Do you want to proceed?"* and then *"Edit file … Do you want to make this edit?"* —
//! an unattended run answered the first and stopped at the second under [`Refusal::OtherQuestion`].
//! Honest, and still a run a person has to come back to, which is the case this whole contract
//! exists to serve.
//!
//! So a run is given [`Consents`]: a NON-EMPTY LIST of independent clauses, each of the form *"if
//! the peer is asking THIS, you may answer THAT"*. Nothing about a single clause changes — the
//! widening is that a caller may write down more than one decision before leaving, which is exactly
//! what leaving unattended requires them to do.
//!
//! ⚠ **AND A LIST CAN CONTRADICT ITSELF**, which is the one thing a single clause could not. Two
//! clauses about the same question naming different options is a caller who has said both *yes* and
//! *no*, and picking between them would be this product deciding — so it is
//! [`Refusal::Contradicted`] and nothing is typed. See [`Consents::covers`], where the whole
//! precedence is derived.
//!
//! # What a consent is, and the three things it is deliberately NOT
//!
//! A [`Consent`] is TWO NEEDLES: one for the question, one for the option. It authorises exactly
//! one answer to exactly one question, in the agent's own words, decided before the dialog exists.
//!
//! * **NOT A NUMBER.** *"Always press 2"* is the shape that makes this dangerous. A number means a
//!   different thing in every dialog — [`Choice::number`] is read off the screen precisely because
//!   *"a list that has scrolled does not start at one"* — so a consent spelled as a digit is a
//!   consent to whatever happens to be second, which is not something a person can have agreed to.
//! * **NOT A DEFAULT.** There is no consent unless the caller wrote one. A run with none behaves
//!   exactly as every run did before this existed, and that is the arm the gates hold hardest.
//! * **NOT A FALLBACK.** Every way the consent fails to name ONE option — no clause matched, no
//!   option carried the words, SEVERAL did — ends the run with the question reported and nothing
//!   typed. There is no *"closest match"* and no *"the marker was on it anyway"*, because both of
//!   those are the product deciding, and the product is the one party here with no standing to.
//!
//! # ⚠⚠ The ambiguity rule is the load-bearing one
//!
//! Measured on the real dialogs [`sprag_detect::question`] was built from: an agent offers
//! `1. Yes`, `2. Yes, and don't ask again`, `3. No`. A substring policy that took the FIRST match
//! for `"Yes"` would authorise option 1 today and option 2 the day an agent reorders its list —
//! and option 2 is *"stop asking me"*, which is the one answer that disables every future consent
//! check. So a needle carried by more than one option answers NOTHING and says so
//! ([`Refusal::Ambiguous`]).
//!
//! That alone would make the commonest real consent unexpressible, since `"Yes"` is a substring of
//! `"Yes, and don't ask again"` and no shorter word distinguishes them. Hence the two tiers in
//! [`Consent::covers`]: an option whose label IS the answer wins outright, and the substring tier is
//! only consulted when no label matches exactly. The exact tier is the caller's way to say *"that
//! one, the whole of it"*, and it is the reason the rule is strict without being useless.

use sprag_detect::{Choice, Question};

/// WHAT A RUN MAY ANSWER when its peer stops to ask — one question, one option, both in the agent's
/// own words.
///
/// Constructed through [`parse`](Self::parse) so an empty needle cannot exist: see there for why an
/// empty one is a different catastrophe in each field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consent {
    /// Text the QUESTION must carry for this consent to be about it.
    asked: String,
    /// Text the OPTION must carry for this consent to authorise it.
    answer: String,
}

impl Consent {
    /// The wire key of the QUESTION needle, inside one element of
    /// [`Consents::WIRE_KEY`](Consents::WIRE_KEY).
    pub const ASKED_KEY: &'static str = "asked";
    /// The wire key of the OPTION needle, inside one element of
    /// [`Consents::WIRE_KEY`](Consents::WIRE_KEY).
    pub const ANSWER_KEY: &'static str = "answer";

    /// A consent to answer a question carrying `asked` with the option carrying `answer`, or `None`
    /// when either needle is empty.
    ///
    /// # ⚠⚠ An empty needle is REFUSED, and the two are not the same mistake
    ///
    /// [`ReadyWhen::parse`](crate::readiness::ReadyWhen::parse)'s rule, and here the stakes are the
    /// argument's whole point:
    ///
    /// * an empty `asked` is carried by EVERY question, so the consent stops being about a question
    ///   at all and becomes *"answer anything that offers this option"*;
    /// * an empty `answer` is carried by EVERY option, so every question with two or more choices —
    ///   which is every question, since a one-option list is not a menu — is
    ///   [`Ambiguous`](Refusal::Ambiguous) and nothing is ever answered.
    ///
    /// One of those types at a dialog the caller never saw and the other is a barrier that only
    /// looks like one. Both are a `String` admitting fewer values than its type, which is R352's
    /// shape, and the predicate lives here so the parser and the publication share it.
    #[must_use]
    pub fn parse(asked: String, answer: String) -> Option<Self> {
        if asked.is_empty() || answer.is_empty() {
            return None;
        }
        Some(Self { asked, answer })
    }

    /// The text the question must carry.
    #[must_use]
    pub fn asked(&self) -> &str {
        &self.asked
    }

    /// The text the authorised option must carry.
    #[must_use]
    pub fn answer(&self) -> &str {
        &self.answer
    }

    /// The ONE option of `question` this consent authorises, or why it authorises none.
    ///
    /// # The order of the checks is the order of the remedies
    ///
    /// The question is tested first, because *"this consent is about a different dialog"* is a
    /// complete answer and the option needle has no meaning until it holds. Then the two tiers:
    ///
    /// 1. **An option whose label IS the answer.** The caller quoted the whole thing, so there is
    ///    nothing left to be ambiguous about — see the module doc for why this tier exists at all.
    /// 2. **Exactly one option whose label CONTAINS it.** Fewer is [`NotOffered`](Refusal::NotOffered),
    ///    more is [`Ambiguous`](Refusal::Ambiguous), and neither is a near-miss to be resolved.
    ///
    /// ⚠ **CASE-SENSITIVE, deliberately.** A consent is written by reading the agent's own dialog,
    /// so its words are available exactly; folding case widens what a needle covers, and every
    /// widening here is in the direction of answering something the caller did not picture.
    pub fn covers<'q>(&self, question: &'q Question) -> Result<&'q Choice, Refusal> {
        // ⚠ The question's lines are joined with a space rather than tested one at a time: an
        // agent's sentence wraps across the lines of its own box, so a needle spanning the break
        // would be carried by no single line while being plainly on the screen.
        if !question.asked.join(" ").contains(&self.asked) {
            return Err(Refusal::OtherQuestion);
        }
        let exact: Vec<&Choice> = question
            .choices
            .iter()
            .filter(|choice| choice.label == self.answer)
            .collect();
        if let [only] = exact[..] {
            return Ok(only);
        }
        if !exact.is_empty() {
            // Two options with the SAME label. Nothing the caller can write tells them apart, so
            // there is no consent that reaches either of them.
            return Err(Refusal::Ambiguous);
        }
        let held: Vec<&Choice> = question
            .choices
            .iter()
            .filter(|choice| choice.label.contains(&self.answer))
            .collect();
        match held[..] {
            [only] => Ok(only),
            [] => Err(Refusal::NotOffered),
            _ => Err(Refusal::Ambiguous),
        }
    }
}

/// WHAT A RUN MAY ANSWER ACROSS A WHOLE TURN — a non-empty list of independent [`Consent`] clauses.
///
/// # ⚠⚠ Why the list is a type and not a `Vec<Consent>`
///
/// Two reasons, and both are about what a bare vector would admit.
///
/// * **An EMPTY list must not exist.** It is [`Consent::parse`]'s argument one level out: a consent
///   list with no clauses authorises nothing, which is precisely what NOT sending the key already
///   says — and on the `answer` form, whose whole content is the consent, it would be a run
///   occupying a slot to do what not calling it does. [`of`](Self::of) is the only public
///   constructor from a vector, and it refuses.
/// * **The precedence lives somewhere.** Deciding what several clauses say about ONE question is
///   the new reasoning this round added, and it is exactly the kind that ends up copied into each
///   caller when it has no home. [`covers`](Self::covers) is that home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consents {
    /// The clauses, in the caller's own order. Never empty — see [`Consents::of`].
    ///
    /// ⚠ The ORDER is preserved and deliberately not USED: no clause outranks another, because a
    /// list whose first clause wins is a list where adding a rule silently disables an earlier
    /// one. Two clauses that disagree about a question are [`Refusal::Contradicted`], not a race.
    clauses: Vec<Consent>,
}

impl Consents {
    /// The wire key this contract is sent under — published from here so the grammar, the parser
    /// and the help text are one word.
    ///
    /// ⚠ It stayed `may_answer` across the change from one clause to a list: the sentence *what
    /// this run may answer* is as true of a list as of a clause, and renaming it would have been a
    /// second break for callers on top of the shape.
    pub const WIRE_KEY: &'static str = "may_answer";

    /// The clauses of `clauses`, or `None` when there are none.
    ///
    /// ⚠ Takes the vector by value and refuses the empty one, rather than offering a builder that
    /// can be left empty — the type's whole guarantee is that a `Consents` in hand authorises
    /// something, and a builder would push the check to whoever remembered.
    #[must_use]
    pub fn of(clauses: Vec<Consent>) -> Option<Self> {
        (!clauses.is_empty()).then_some(Self { clauses })
    }

    // ⚠ NO ACCESSOR FOR THE CLAUSES, and the absence is measured rather than an oversight. The
    // obvious `clauses(&self) -> &[Consent]` was written, and the sweep for its readers found only
    // TESTS — nothing this product does needs to read a consent back, because a run reports what it
    // ANSWERED (`Outcome::answered`) and WHY it did not (`Refusal`), never what it was allowed to.
    // R357's rule: build the consumer before the surface. A gate that wants to check what the
    // parser built compares whole values, which is a stronger claim than a field-by-field read.
    //
    // ⚠ The day a `contradicted` run wants to name WHICH clauses collided, that reader arrives with
    // its answer key — and it is registered as owed rather than half-built here.

    /// The ONE option of `question` these consents authorise, or why they authorise none.
    ///
    /// # ⚠⚠⚠ The precedence, and why each step of it is the conservative one
    ///
    /// Every clause is asked about the question independently, and the answers are combined so that
    /// **anything less than one unambiguous option types nothing**:
    ///
    /// 1. **A clause that is about a DIFFERENT question does not apply.** This is the whole point of
    ///    a list, and it is why [`Refusal::OtherQuestion`] is not fatal per clause.
    /// 2. **An AMBIGUOUS clause is fatal**, even when another clause resolves cleanly. Ambiguity
    ///    means the clause names SEVERAL options and might have authorised any of them — setting it
    ///    aside to act on a different clause would be this run choosing which of the caller's own
    ///    words to honour, and the option it silently dropped could be *"and don't ask again"*.
    /// 3. **Clauses that resolve to DIFFERENT options are [`Refusal::Contradicted`].** A caller with
    ///    a broad *"proceed → Yes"* and a narrow *"write to /etc → No"* has written a rule and an
    ///    exception, and NOTHING on this wire says which outranks which. Answering either one would
    ///    be a precedence policy the caller never chose. ⚠ Resolving to the SAME option is not a
    ///    contradiction: two clauses agreeing is a caller saying one thing twice.
    /// 4. **Exactly one option survives** — that is the answer.
    /// 5. **Nothing resolved.** [`Refusal::NotOffered`] when some clause was about this question,
    ///    [`Refusal::OtherQuestion`] when none was. The existing two-arm distinction, unchanged: one
    ///    remedy is to widen a needle, the other is to write a clause at all.
    pub fn covers<'q>(&self, question: &'q Question) -> Result<&'q Choice, Refusal> {
        let mut applicable = false;
        let mut chosen: Option<&'q Choice> = None;
        for clause in &self.clauses {
            match clause.covers(question) {
                Ok(choice) => {
                    applicable = true;
                    match chosen {
                        None => chosen = Some(choice),
                        // ⚠ By NUMBER, which is this question's own identity for an option: two
                        // clauses reaching the same row of the same menu agree, however differently
                        // they spelled the needle that got them there.
                        Some(already) if already.number == choice.number => {}
                        Some(_) => return Err(Refusal::Contradicted),
                    }
                }
                // Step 1: not about this question, so it has nothing to say about it.
                Err(Refusal::OtherQuestion) => {}
                // Step 2: fatal on sight. See the precedence note above for why this one cannot be
                // set aside the way a `NotOffered` clause can.
                Err(Refusal::Ambiguous) => return Err(Refusal::Ambiguous),
                // About this question and authorising nothing on it. It counts as APPLICABLE so the
                // report can say `not_offered` — *you wrote a clause about this dialog and none of
                // its options carries your answer* — instead of sending the caller to write a
                // clause they already wrote.
                Err(Refusal::NotOffered) => applicable = true,
                // ⚠ A reason a CLAUSE cannot produce today: `Unreadable`, `NotTaken`, `NoConsent`
                // and `Contradicted` are all decided about the RUN or the PANE and never inside
                // `Consent::covers`. It is carried straight out rather than folded into either
                // bucket, because a reason this list cannot interpret is one it must not summarise
                // — and returning is the direction that types nothing.
                Err(beyond_this_list) => return Err(beyond_this_list),
            }
        }
        match (chosen, applicable) {
            (Some(only), _) => Ok(only),
            (None, true) => Err(Refusal::NotOffered),
            (None, false) => Err(Refusal::OtherQuestion),
        }
    }
}

/// WHY a blocked peer was left for a person — the sentence a run owes whoever reads it.
///
/// # ⚠⚠ Why a run that was GIVEN a consent and still stopped is the case that needs this
///
/// Without a reason, the two outcomes a caller has to tell apart look identical: *"I gave no
/// consent, so of course it stopped"* and *"I gave a consent and it did not fire"*. The second is
/// either a typo in a needle or a dialog the caller had not pictured, and both are things they can
/// fix — but only if the run says which. This is the same argument
/// [`Ceiling`](crate::driver::Ceiling) makes for carrying WHICH ceiling exhausted a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The pane is blocked and this host could not read a menu on it.
    ///
    /// ⚠ The one arm that is not about the consent: it is
    /// [`AgentObservation::asking`](crate::access::AgentObservation::asking)'s `None`, which until
    /// now was published as an absence and explained nowhere. An agent can block on something that
    /// is not a numbered list — a free-text prompt, a paged view, a confirmation drawn as prose —
    /// and no consent can name an option a screen does not offer. **The remedy is a person**, and
    /// this is the word that says so.
    Unreadable,
    /// ⚠⚠ **THE ANSWER WAS TYPED AND THE PEER WENT ON ASKING.**
    ///
    /// The only arm that costs bytes, and the only one about what happened AFTER a decision to
    /// answer. The consent named one option, the run took it the one provable way
    /// ([`Taken`]), and the peer did not move off the question inside the answering bound.
    ///
    /// It is a refusal and not a failure because the instruction it carries is the same as every
    /// other arm's: **the run stops and a person looks at the pane.** What it is NOT is a reason to
    /// try again — a second keystroke into a dialog that ignored the first is how a loop comes to
    /// type at a menu, which is the whole class this contract exists inside.
    NotTaken,
    /// The run was given no consent, so it answers nothing. The default, and the state every run
    /// before this contract existed was permanently in.
    NoConsent,
    /// The consent is about a different question than the one on screen.
    OtherQuestion,
    /// The question is the right one and no option on it carries the authorised answer.
    NotOffered,
    /// SEVERAL options carry the authorised answer, so picking one would be this product choosing.
    ///
    /// ⚠ The arm the module doc is about. It is not a degenerate case: `Yes` / `Yes, and don't ask
    /// again` is the measured shape of a real permission dialog, and the second option is the one
    /// that turns off every future question.
    Ambiguous,
    /// ⚠⚠⚠ **THE CALLER'S OWN CONSENTS DISAGREE ABOUT THIS QUESTION.**
    ///
    /// Two or more clauses are about the dialog on screen and they authorise DIFFERENT options —
    /// which is what a broad rule and a narrow exception look like when nothing declares which
    /// outranks the other. The natural shape is real: *"proceed → Yes"* beside *"write to /etc →
    /// No"*, on a question that carries both phrases.
    ///
    /// It is separate from [`Ambiguous`](Self::Ambiguous) because the remedies are different
    /// things. Ambiguous is one needle that is too short for the menu, and the fix is to quote more
    /// of the option. This is two decisions that cannot both be honoured, and the fix is for the
    /// CALLER to decide which they meant — a choice this product would be usurping if it applied
    /// first-wins, longest-needle, or any other rule nobody wrote down.
    Contradicted,
}

impl Refusal {
    /// The words this vocabulary publishes, in this type's own order.
    ///
    /// Projected from [`ALL`](Self::ALL) at every mouth rather than retyped, so an arm added to the
    /// type reaches the wire in the compile that adds it.
    pub const WIRE_WORDS: &'static [&'static str] = &{
        let mut words = [""; Refusal::ALL.len()];
        let mut at = 0;
        while at < Refusal::ALL.len() {
            words[at] = Refusal::ALL[at].wire_str();
            at += 1;
        }
        words
    };

    /// Every arm, so the published vocabulary and the readers below are one list.
    pub const ALL: [Self; 7] = [
        Self::Unreadable,
        Self::NotTaken,
        Self::NoConsent,
        Self::OtherQuestion,
        Self::NotOffered,
        Self::Ambiguous,
        Self::Contradicted,
    ];

    /// This reason's word on the wire.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::NotTaken => "not_taken",
            Self::NoConsent => "no_consent",
            Self::OtherQuestion => "other_question",
            Self::NotOffered => "not_offered",
            Self::Ambiguous => "ambiguous",
            Self::Contradicted => "contradicted",
        }
    }

    /// The reason named by `word`, or `None` for a word outside the closed set.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|why| why.wire_str() == word)
    }

    /// The SENTENCE a person reads beside the question — what to do, not what happened.
    ///
    /// ⚠ Each one names a remedy, because a reason that leaves the reader with nothing to do is a
    /// diagnostic and not a report. [`Unreadable`](Self::Unreadable) is the only arm whose remedy is
    /// not a change to the call.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Unreadable => {
                "the peer is blocked on something this host cannot read as a numbered menu, so no \
                 consent can name an option on it — hand the pane to a person"
            }
            Self::NotTaken => {
                "the run typed the option the consent authorised and did not see the peer take it, \
                 so the pane may be sitting on that dialog still — hand it to a person rather than \
                 typing at it again"
            }
            Self::NoConsent => {
                "the run was given no consent to answer anything, so it stopped rather than \
                 selecting for somebody"
            }
            Self::OtherQuestion => {
                "the run's consent is about a different question than the one the peer is asking"
            }
            Self::NotOffered => {
                "the peer is asking the question the consent names and no option on it carries the \
                 authorised answer"
            }
            Self::Ambiguous => {
                "more than one option carries the authorised answer, so choosing between them \
                 would be this run deciding rather than the caller"
            }
            Self::Contradicted => {
                "more than one consent is about this question and they authorise different \
                 options, so nothing but the caller can say which of their own rules wins here"
            }
        }
    }
}

/// A blocked peer the run did NOT answer: what it is asking, when this host can read it, and why
/// nothing was typed.
///
/// # ⚠ The invariant, held by construction
///
/// [`Refusal::Unreadable`] is exactly the case with no question, and the two constructors are the
/// only way to build one — so *"unreadable, and here is the question"* and *"the consent did not
/// match, and there was no question"* are both unrepresentable rather than merely unlikely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unanswered {
    question: Option<Question>,
    why: Refusal,
    bytes: u64,
}

impl Unanswered {
    /// A peer blocked on something this host cannot read as a menu.
    #[must_use]
    pub const fn unreadable() -> Self {
        Self {
            question: None,
            why: Refusal::Unreadable,
            bytes: 0,
        }
    }

    /// A question this host READ and the run did not type at, and why.
    ///
    /// ⚠ `why` is taken as a [`Refusal`] rather than as one of the non-`Unreadable` arms because a
    /// further reason will not be `Unreadable` either, and a second type holding all-but-one
    /// variant is a copy that has to be kept in step. The constructor is the authority: pass
    /// `Unreadable` here and the question is dropped, which is the one collapse that keeps the
    /// invariant true.
    #[must_use]
    pub fn refused(question: Question, why: Refusal) -> Self {
        match why {
            Refusal::Unreadable => Self::unreadable(),
            _ => Self {
                question: Some(question),
                why,
                bytes: 0,
            },
        }
    }

    /// An answer that WAS typed and that the peer did not take — [`Refusal::NotTaken`], carrying
    /// what it cost.
    ///
    /// # ⚠⚠ Why the bytes have to travel with the refusal
    ///
    /// Every other arm here reports a step that spent nothing, and the plugins say so
    /// (`Cost::Bytes(0)`). This one typed at the pane. A run that charged zero for it would
    /// under-report its own spend against the caller's cost ceiling — small in bytes and wrong in
    /// kind, since the whole selling point of a bounded run is that what it spent is what it says.
    #[must_use]
    pub fn not_taken(question: Question, bytes: u64) -> Self {
        Self {
            question: Some(question),
            why: Refusal::NotTaken,
            bytes,
        }
    }

    /// What the peer is asking, or `None` when this host could not read it.
    #[must_use]
    pub const fn question(&self) -> Option<&Question> {
        self.question.as_ref()
    }

    /// Why the run is not typing its own text at this pane.
    #[must_use]
    pub const fn why(&self) -> Refusal {
        self.why
    }

    /// PTY bytes this step spent — non-zero only for [`Refusal::NotTaken`].
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// HOW an authorised option was taken — the three keystroke shapes, each provable at the moment it
/// was used.
///
/// # ⚠⚠⚠ Why this is three arms and not *"type the number and press Enter"*
///
/// A number and an Enter is what a person does, and it is unsafe for a machine because the two
/// keystrokes have independent effects. If the number SELECTS AND SUBMITS — which the measured
/// agents do — then the Enter that follows lands on whatever the peer showed next, which may be
/// another dialog. If the number only MOVES THE MARKER, the Enter is required. Nothing about a
/// dialog says which it is, and a run that guesses is a run that sometimes confirms a question it
/// never read.
///
/// So the run never sends an Enter it cannot justify. Each arm is a state of the peer's own marker
/// at the instant the key was sent, and [`Question::selected`] — *"where a bare Enter would land"* —
/// is the fact each one is justified by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// The peer's marker was ALREADY on the authorised option, so a bare Enter took it — and could
    /// take nothing else. No number is typed at all: there is nothing to move.
    Selected,
    /// The option's NUMBER was typed and the peer left the question. **No Enter was sent**, because
    /// none was needed and one sent anyway would have gone to whatever came next.
    Numbered,
    /// The number moved the peer's marker ONTO the authorised option, and Enter then committed it.
    ///
    /// ⚠ The marker having moved is the proof the peer processed the number — which is also what
    /// makes the Enter safe, since it is now landing on the option the consent named.
    NumberedThenConfirmed,
    /// The peer IGNORED the Enter its own marker justified, so the option's number was typed too.
    ///
    /// # ⚠⚠⚠ The arm an END-TO-END run had to measure
    ///
    /// [`Selected`](Self::Selected) is the commonest case by far — the caller authorises `Yes` and
    /// `Yes` is the option the agent has highlighted — and against a menu with number hotkeys and
    /// no Enter handling it was the one case that could never be answered: the run pressed the one
    /// key that dialog does not read and reported [`Refusal::NotTaken`]. **Every unit gate passed**,
    /// because each of them supplied the other side of the conversation; the first run through a
    /// real daemon against a real pane found it in one go.
    ///
    /// ⚠ The escalation is bounded by the same evidence as everything else here: it happens only
    /// while the screen still shows THAT question with the marker still on THAT option, so an Enter
    /// that had in fact landed would have taken the dialog away and left nothing to escalate into.
    SelectedThenNumbered,
}

impl Taken {
    /// This shape as a clause naming what was typed.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Selected => "its own marker was already there, so Enter alone took it",
            Self::Numbered => "typing the number took it outright, so no Enter was sent",
            Self::NumberedThenConfirmed => {
                "the number moved its marker onto the option and Enter then committed it"
            }
            Self::SelectedThenNumbered => {
                "it did not take the Enter its own marker justified, so the number was typed too"
            }
        }
    }
}

/// A blocked peer the run DID answer, on the caller's consent — what was asked, what was picked,
/// and what it cost.
///
/// # ⚠⚠ Why this is a value and not a log line
///
/// An answer given by a machine on somebody's behalf is the one act in this crate that a person may
/// need to audit after the fact, and a run that gave one must be able to SAY so in the same
/// vocabulary it says everything else. Carried as data, it reaches the step journal, the run's
/// verdict word and a report; written as a formatted string it would have reached only whichever of
/// those the author remembered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    /// The question as it stood when the answer was given.
    pub question: Question,
    /// The option the consent authorised, as it was on the screen.
    pub chose: Choice,
    /// Which of the three provable keystroke shapes took it — see [`Taken`].
    pub how: Taken,
    /// PTY bytes the answer cost, counted like every other injection this crate makes.
    pub bytes: u64,
}

impl Answered {
    /// ONE LINE for the run's journal — the option that was picked, in the agent's own words, and
    /// how it was taken.
    ///
    /// ⚠ The LABEL and not only the number, because a number is meaningless a day later: the
    /// dialog it indexed is gone, and *"answered 2"* cannot be audited by anybody.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "answered the peer with {}. {:?} — {}",
            self.chose.number,
            self.chose.label,
            self.how.describe(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured shape of a real tool-permission dialog — the one the ambiguity rule is about.
    fn permission() -> Question {
        Question {
            asked: vec![
                "Bash command".to_owned(),
                "rm -rf build/".to_owned(),
                "Do you want to proceed?".to_owned(),
            ],
            choices: vec![
                Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                Choice {
                    number: 2,
                    label: "Yes, and don't ask again for rm commands".to_owned(),
                    selected: false,
                },
                Choice {
                    number: 3,
                    label: "No, and tell Claude what to do differently".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// ⚠⚠⚠ **`"Yes"` MUST NOT REACH `"Yes, and don't ask again"`** — the defect this whole type
    /// exists to make unrepresentable.
    ///
    /// A substring policy taking the first match authorises option 1 today. The day an agent
    /// reorders its list — or adds a fourth option above it — the same consent authorises *"stop
    /// asking me"*, which is the one answer that disables every future consent check. The caller
    /// wrote three letters and got a permanent grant.
    ///
    /// ⚠ REVERT-PROOF: delete the exact-label tier from [`Consent::covers`] and this fails as
    /// [`Refusal::Ambiguous`] — the whole reason that tier is there.
    #[test]
    fn an_exact_label_wins_over_the_longer_option_that_contains_it() {
        let consent = Consent::parse("Do you want to proceed?".to_owned(), "Yes".to_owned())
            .expect("two needles");
        let question = permission();
        let chosen = consent
            .covers(&question)
            .expect("the consent authorises one");
        assert_eq!(
            chosen.number, 1,
            "⚠⚠⚠ `Yes` is the WHOLE of option 1's label and a PREFIX of option 2's — the exact \
             match is the caller saying `that one, the whole of it`, and taking option 2 here is a \
             machine granting a standing permission nobody typed",
        );
        assert_eq!(chosen.label, "Yes");
    }

    /// ⚠⚠⚠ **A NEEDLE CARRIED BY TWO OPTIONS ANSWERS NEITHER.**
    ///
    /// The other half of the same rule, and the half a first-match policy passes vacuously. `"and"`
    /// is on options 2 and 3 of the measured dialog — one grants a standing permission and the
    /// other refuses the command — so there is no defensible way to pick, and the honest answer is
    /// that the run stops and says the consent was ambiguous.
    ///
    /// ⚠ The two options this straddles are OPPOSITES, which is why *"pick the first"* is not a
    /// tolerable approximation.
    #[test]
    fn a_needle_two_options_carry_authorises_neither() {
        let consent = Consent::parse("proceed".to_owned(), "and".to_owned()).expect("two needles");
        assert_eq!(
            consent.covers(&permission()),
            Err(Refusal::Ambiguous),
            "⚠⚠⚠ `and` is in `and don't ask again` AND in `and tell Claude what to do \
             differently` — a grant and a refusal. A policy that resolved this would be the \
             product choosing between opposites on the caller's behalf",
        );
    }

    /// The substring tier DOES work when it names one option — otherwise the exact tier would be
    /// the only tier and a caller would have to quote a whole wrapped sentence to say anything.
    #[test]
    fn a_needle_only_one_option_carries_authorises_that_one() {
        let consent = Consent::parse("proceed".to_owned(), "don't ask again".to_owned())
            .expect("two needles");
        assert_eq!(
            consent
                .covers(&permission())
                .expect("one option carries it")
                .number,
            2,
        );
    }

    /// ⚠⚠ **A CONSENT IS ABOUT A QUESTION, and the option needle has no meaning until that holds.**
    ///
    /// The failure this closes is the one that makes a consent dangerous at all: `"Yes"` authorised
    /// for *"overwrite the draft?"* must not answer *"delete the production database?"*. Same
    /// option, same word, different question — and the option needle alone cannot tell them apart.
    #[test]
    fn a_consent_for_one_question_does_not_answer_another() {
        let consent = Consent::parse("overwrite the draft".to_owned(), "Yes".to_owned())
            .expect("two needles");
        assert_eq!(
            consent.covers(&permission()),
            Err(Refusal::OtherQuestion),
            "⚠⚠ the option is on offer and the QUESTION is not the one consented to — a consent \
             that ignored this would answer every dialog that happens to offer the same word",
        );
    }

    /// A question the consent IS about, offering nothing it authorises — a different remedy from
    /// [`Refusal::OtherQuestion`], and the reason the two are separate arms.
    #[test]
    fn a_question_that_offers_nothing_authorised_is_not_the_wrong_question() {
        let consent =
            Consent::parse("proceed".to_owned(), "Maybe".to_owned()).expect("two needles");
        assert_eq!(consent.covers(&permission()), Err(Refusal::NotOffered));
    }

    /// ⚠⚠ **AN EMPTY NEEDLE IS REFUSED IN BOTH FIELDS**, and the two are different catastrophes —
    /// see [`Consent::parse`]. Asserted as a construction refusal rather than as a matching quirk,
    /// because a value that cannot exist needs no rule downstream.
    #[test]
    fn an_empty_needle_is_not_a_consent() {
        assert!(
            Consent::parse(String::new(), "Yes".to_owned()).is_none(),
            "an empty question needle is carried by EVERY question: the consent stops being about \
             a question at all",
        );
        assert!(
            Consent::parse("proceed".to_owned(), String::new()).is_none(),
            "an empty option needle is carried by EVERY option, so every real menu is ambiguous \
             and the argument is a barrier that only looks like one",
        );
        assert!(Consent::parse(String::new(), String::new()).is_none());
    }

    /// ⚠⚠ **EVERY PUBLISHED WORD ROUND-TRIPS, AND EVERY ARM HAS A REMEDY.**
    ///
    /// [`Refusal::WIRE_WORDS`] is what the wire advertises and [`Refusal::parse`] is what reads it
    /// back — one vocabulary, two spellings, which is the shape R353 measured going wrong in this
    /// workspace. Driven from the published list so a sixth arm fails here the moment its word is
    /// published without a reader.
    ///
    /// ⚠ The sentence half matters as much: a reason a person cannot act on is a diagnostic, and
    /// [`Refusal::describe`] is an exhaustive match, so a new arm compiles only once somebody has
    /// written what to DO about it.
    #[test]
    fn every_published_refusal_round_trips_and_names_a_remedy() {
        assert_eq!(
            Refusal::WIRE_WORDS.len(),
            Refusal::ALL.len(),
            "the published list is a projection of the type, never a second list",
        );
        for word in Refusal::WIRE_WORDS {
            let why = Refusal::parse(word)
                .unwrap_or_else(|| panic!("{word:?} is published and the parser refuses it"));
            assert_eq!(why.wire_str(), *word, "and it spells back the word it read");
            let said = why.describe();
            assert!(
                said.len() > 40 && said.starts_with(char::is_lowercase),
                "{word:?} must say what to do about it, as a clause: {said:?}",
            );
        }
        assert!(
            Refusal::parse("maybe").is_none(),
            "a word outside the set is refused, or the published list is a false statement",
        );
        assert_eq!(
            Refusal::ALL.len(),
            7,
            "the seven reasons a blocked peer goes unanswered: this host could not READ the menu, \
             the answer was typed and NOT TAKEN, the caller consented to NOTHING, the consent is \
             about ANOTHER question, the question does not OFFER the answer, SEVERAL options \
             carry it, or the caller's own consents CONTRADICT each other about it",
        );
    }

    /// ⚠⚠ **THE ONE REFUSAL THAT COSTS BYTES CARRIES THEM.** Every other arm reports a step that
    /// typed nothing; this one typed at the pane and the peer did not move. A run that charged
    /// zero for it would under-report its own spend against the caller's ceiling.
    #[test]
    fn the_refusal_that_typed_something_is_the_only_one_that_charges_for_it() {
        assert_eq!(Unanswered::not_taken(permission(), 3).bytes(), 3);
        assert_eq!(
            Unanswered::not_taken(permission(), 3).why(),
            Refusal::NotTaken
        );
        for why in Refusal::ALL {
            if why == Refusal::NotTaken {
                continue;
            }
            assert_eq!(
                Unanswered::refused(permission(), why).bytes(),
                0,
                "{why:?} is a step that typed NOTHING, and charging for it would be as wrong as \
                 the under-charge the other way",
            );
        }
    }

    /// ⚠ **EVERY KEYSTROKE SHAPE SAYS WHAT IT TYPED**, and the three are genuinely different acts:
    /// one sends no number, one sends no Enter, one sends both. A reader of a run's journal has to
    /// be able to tell which — an Enter this product sent on somebody's behalf is the fact worth
    /// auditing.
    #[test]
    fn each_way_of_taking_an_option_says_which_keys_it_used() {
        for how in [
            Taken::Selected,
            Taken::Numbered,
            Taken::NumberedThenConfirmed,
            Taken::SelectedThenNumbered,
        ] {
            let said = Answered {
                question: permission(),
                chose: permission().choices[0].clone(),
                how,
                bytes: 1,
            }
            .describe();
            assert!(
                said.contains("Yes") && said.contains('1'),
                "the label and the number are what make it auditable: {said:?}",
            );
            assert!(
                said.contains("Enter") || said.contains("marker"),
                "{how:?} must say what it did about Enter — that is the keystroke a person needs \
                 to know a machine sent: {said:?}",
            );
        }
        assert_ne!(
            Taken::Numbered.describe(),
            Taken::NumberedThenConfirmed.describe(),
            "sending an Enter and deliberately NOT sending one are different records",
        );
    }

    /// ⚠⚠ **`Unreadable` AND A QUESTION CANNOT BOTH BE TRUE**, and the constructor is what makes
    /// that so rather than a comment asking callers to be careful.
    #[test]
    fn an_unreadable_refusal_can_never_carry_a_question() {
        let dropped = Unanswered::refused(permission(), Refusal::Unreadable);
        assert_eq!(dropped, Unanswered::unreadable());
        assert!(
            dropped.question().is_none(),
            "`unreadable` MEANS there was no question to read — carrying one would make the \
             report contradict itself",
        );
        let kept = Unanswered::refused(permission(), Refusal::NotOffered);
        assert_eq!(kept.why(), Refusal::NotOffered);
        assert!(
            kept.question().is_some(),
            "and every other reason keeps the question, which is the thing a person has to answer",
        );
    }

    // ── THE LIST ──────────────────────────────────────────────────────────────────────────────
    //
    // ⚠⚠ Every gate below is over the SAME measured permission dialog the single-clause gates use,
    // so what differs between them is only what the caller wrote down.

    /// Clauses, from `(asked, answer)` pairs.
    fn consents(pairs: &[(&str, &str)]) -> Consents {
        Consents::of(
            pairs
                .iter()
                .map(|(asked, answer)| {
                    Consent::parse((*asked).to_owned(), (*answer).to_owned()).expect("two needles")
                })
                .collect(),
        )
        .expect("a non-empty list")
    }

    /// ⚠⚠⚠ **A CLAUSE ABOUT ANOTHER QUESTION DOES NOT BLOCK THE ONE THAT APPLIES** — the whole
    /// reason a list exists.
    ///
    /// A turn asks several questions, so most of a caller's clauses are about a dialog that is not
    /// on the screen right now. If an inapplicable clause counted against the answer, a list would
    /// be strictly worse than the single clause it replaced: the more decisions a caller wrote
    /// down, the less any of them could do.
    ///
    /// ⚠ Driven both ways round, because a list that only worked when the applicable clause came
    /// FIRST would be an order-dependence nobody declared.
    #[test]
    fn a_clause_about_a_different_question_stands_aside_for_the_one_that_applies() {
        let question = permission();
        for pairs in [
            [
                ("Do you want to make this edit?", "Yes"),
                ("proceed", "Yes"),
            ],
            [
                ("proceed", "Yes"),
                ("Do you want to make this edit?", "Yes"),
            ],
        ] {
            let chosen = consents(&pairs)
                .covers(&question)
                .expect("one clause is about this question and names one option");
            assert_eq!(
                chosen.number, 1,
                "⚠⚠⚠ the edit clause is about a dialog that is not on screen, and a list where \
                 that costs the applicable clause its answer would punish a caller for planning \
                 ahead: {pairs:?}",
            );
        }
    }

    /// ⚠⚠⚠ **TWO CLAUSES THAT AUTHORISE DIFFERENT OPTIONS ANSWER NEITHER.**
    ///
    /// The failure a list makes possible and a single clause could not: a caller with a broad rule
    /// and a narrow exception has written both, and nothing on this wire says which outranks which.
    /// Every resolution — first wins, last wins, longest needle wins — is a precedence policy the
    /// caller never chose, and here the two options are `Yes` and `No, and tell Claude what to do
    /// differently`, which are opposites.
    ///
    /// ⚠ REVERT-PROOF: make `Consents::covers` take the first clause that resolves and this fails,
    /// answering `Yes` to a caller who also said `No`.
    #[test]
    fn consents_that_disagree_about_one_question_authorise_nothing() {
        assert_eq!(
            consents(&[("proceed", "Yes"), ("rm -rf", "No, and tell")]).covers(&permission()),
            Err(Refusal::Contradicted),
            "⚠⚠⚠ `Yes` and `No` on one dialog is a caller who has to decide, not a run that gets \
             to",
        );
        // And the same pair the other way round: the answer cannot depend on the order they were
        // written in, or the contradiction is a race rather than a report.
        assert_eq!(
            consents(&[("rm -rf", "No, and tell"), ("proceed", "Yes")]).covers(&permission()),
            Err(Refusal::Contradicted),
        );
    }

    /// ⚠⚠ **TWO CLAUSES THAT REACH THE SAME OPTION ARE NOT A CONTRADICTION** — the control for the
    /// gate above, and the case that keeps `Contradicted` from being a tax on writing rules.
    ///
    /// The needles are spelled differently and both name option 1. A caller who says one thing
    /// twice has not disagreed with themselves, and identity is decided by the OPTION the clauses
    /// land on rather than by the words that got them there.
    #[test]
    fn consents_that_reach_one_option_by_different_words_agree() {
        let question = permission();
        let chosen = consents(&[("proceed", "Yes"), ("Bash command", "Yes")])
            .covers(&question)
            .expect("both clauses name option 1");
        assert_eq!(chosen.number, 1);
    }

    /// ⚠⚠⚠ **AN AMBIGUOUS CLAUSE IS FATAL EVEN WHEN ANOTHER CLAUSE RESOLVES.**
    ///
    /// The step of the precedence that is easiest to get wrong, and the one that would quietly undo
    /// the rule the whole type was built for. `"and"` sits on option 2 (*grant a standing
    /// permission*) and option 3 (*refuse*), so the clause carrying it might have authorised either
    /// — and a list that set it aside to act on a clean `Yes` beside it would be answering while
    /// holding an instruction it admits it cannot read.
    ///
    /// ⚠ REVERT-PROOF: treat an ambiguous clause as merely inapplicable and this answers option 1,
    /// which is the first-match policy the single-clause design refused to have.
    #[test]
    fn an_ambiguous_clause_is_not_set_aside_by_a_clause_that_resolves() {
        assert_eq!(
            consents(&[("proceed", "Yes"), ("proceed", "and")]).covers(&permission()),
            Err(Refusal::Ambiguous),
            "⚠⚠⚠ a clause that might have named `and don't ask again` cannot be silently dropped \
             because another clause was easier to read",
        );
    }

    /// ⚠⚠ **THE TWO WAYS NOTHING RESOLVES STAY DIFFERENT REASONS**, which is the distinction the
    /// single-clause contract already made and the list must not collapse.
    ///
    /// A caller told *"other_question"* goes and writes a clause; one told *"not_offered"* goes and
    /// re-reads the dialog they already wrote one about. Getting the second when the first is true
    /// sends them looking for a typo in a clause that does not exist.
    #[test]
    fn a_list_still_tells_no_clause_applies_from_no_option_carries_it() {
        assert_eq!(
            consents(&[("delete the database?", "Yes"), ("overwrite?", "No")])
                .covers(&permission()),
            Err(Refusal::OtherQuestion),
            "no clause is about the dialog on screen",
        );
        assert_eq!(
            consents(&[("delete the database?", "Yes"), ("proceed", "Maybe")])
                .covers(&permission()),
            Err(Refusal::NotOffered),
            "⚠⚠ one clause IS about it and the option it names is not on offer — a different \
             remedy, and the reason both arms survived the widening",
        );
    }

    /// ⚠⚠ **AN EMPTY LIST IS NOT A CONSENT**, held at construction like an empty needle.
    ///
    /// A list with no clauses authorises nothing, which is exactly what omitting the argument
    /// already says — and on the `answer` form, whose entire content is the consent, it would be a
    /// run occupying a slot to do what not calling it does.
    ///
    /// ⚠ The other half is the case every existing caller is in: a list of ONE must behave exactly
    /// as the single clause did, or the widening moved something it had no business moving.
    #[test]
    fn an_empty_list_of_consents_cannot_be_built_and_one_clause_behaves_as_it_always_did() {
        assert!(Consents::of(Vec::new()).is_none());
        let question = permission();
        assert_eq!(
            consents(&[("proceed", "Yes")])
                .covers(&question)
                .expect("the single clause answers")
                .number,
            1,
        );
    }

    /// ⚠ **AN AUDIT LINE NAMES THE OPTION IN WORDS.** *"answered 2"* cannot be checked by anybody
    /// once the dialog is gone, which for an approval given on somebody's behalf is the whole
    /// point of recording it.
    #[test]
    fn an_answer_describes_itself_by_label_and_not_only_by_number() {
        let said = Answered {
            question: permission(),
            chose: permission().choices[1].clone(),
            how: Taken::NumberedThenConfirmed,
            bytes: 2,
        }
        .describe();
        assert!(
            said.contains("don't ask again") && said.contains('2'),
            "the label is what makes the record auditable, and the number is what was typed: \
             {said:?}",
        );
    }
}
