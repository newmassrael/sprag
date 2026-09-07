//! `Answer` — *answer the question ONE pane's peer is asking, and stop*.
//!
//! The fifth plugin, and the only one that is not a loop. The other four exist to keep a peer
//! WORKING; this one exists for the moment a peer stops and asks, when the decision has already
//! been made by whoever is watching.
//!
//! # ⚠⚠⚠ Why a surface that can SAY what a peer is asking must also be able to answer it
//!
//! R365 gave a blocked pane a verdict, R366 gave a run a [`Consent`](crate::consent::Consent), and R367 put the question
//! itself on the pane-level surface — so an agent watching a sibling pane can read the dialog, the
//! options, and which one a bare Enter would take. What it could not do was answer. The only door
//! was the RUN surface's `may_answer`, which is a consent declared BEFORE a loop starts, and a
//! supervisor that has just read the question on its neighbour's screen has no loop to declare it
//! on.
//!
//! What that left is the shape this crate keeps finding defects in: **the unsafe act was the only
//! reachable one.** A caller who could see `2. No, and tell Claude what to do differently` and
//! wanted to answer it had `send_keys` — a raw digit and a raw Enter, with none of
//! [`Consent`](crate::consent::Consent)'s protections:
//!
//! * the number is read off a screen the caller looked at some time ago, and
//!   [`Choice::number`](sprag_detect::Choice::number) is a screen fact — a list that has scrolled
//!   or re-rendered does not offer the same digit;
//! * the Enter lands wherever the peer is by the time it arrives, which after a dialog that
//!   submits on its number is the NEXT dialog;
//! * nothing checks that the peer took it, and nothing records that a machine answered at all.
//!
//! So the answer is not a new keystroke path. It is this plugin, which reaches a pane through the
//! same [`Readiness`] barrier every other injecting plugin passes through, carrying the same
//! [`Consent`](crate::consent::Consent) the run surface takes — *"there is one door, and what it may type is what the caller
//! wrote down"*, now with four plugins behind it instead of three.
//!
//! # What makes this DIFFERENT from a one-iteration `orchestrator`
//!
//! An orchestrator with a stimulus is a plugin that TYPES ITS OWN TEXT and treats a question as a
//! reason to stop. This one has no stimulus at all: the only bytes it can ever emit are the ones
//! [`Consents::covers`] authorised, and it converges the moment it has answered. A caller cannot
//! use it to drive a pane, which is what makes it safe to point at a pane that is already blocked.
//!
//! # ⚠⚠ The three endings, and why none of them needed a new OUTCOME word
//!
//! * **It answered** — [`Verdict::Answered`] and then [`Verdict::Converged`]. The run reports
//!   `converged` with [`Outcome::answered`](crate::driver::Outcome::answered) `1`.
//! * **The peer is asking and the consent does not authorise an option on it** —
//!   [`Verdict::Blocked`], which is terminal. `blocked`, with the question and the
//!   [`Refusal`](crate::consent::Refusal) that says which of the reasons it was.
//! * **The peer is NOT asking** — `converged` with `answered` still `0`. Nothing was typed, and the
//!   count is what says so: it is published on every terminal state precisely so *"this run
//!   answered nothing"* is a claim a reader gets affirmatively rather than by not finding a key.
//!   ⚠ Inventing a sixth outcome word for it would move a value space every journal reader decodes
//!   whole, to say something two fields already say together.
//!
//! # ⛔⛔⛔⛔⛔ TWO OF THOSE THREE CONVERGE, AND THE ROW SAID ONE WORD FOR BOTH — register item 912
//!
//! The paragraph above is right that no sixth `OutcomeState` was wanted and wrong about what that
//! settled. `converged` is what BECAME of the run; it is not what the run converged ON — and the
//! two endings that reach it here are opposites a reader has to act on differently: *your consent
//! answered the dialog you quoted* against *by the time this ran, nobody was asking*. Measured over
//! the loop's own store at 2026-09-06T00:11:08Z: **four `answer` runs converged, at three separate
//! builds including the current one, and not one of them named an ending** — while every `ai_loop`
//! run converging since id 56 named one, 39 for 39.
//!
//! That is register item 706's defect one plugin over, and item 903's gate had already promised the
//! column — `every_ending_names_a_column_that_says_why_it_happened` maps
//! [`OutcomeState::Converged`](crate::driver::OutcomeState::Converged) to `done_reason` — while
//! checking only that the column EXISTS on the record. A plugin that never fills it passes that
//! gate, which is this workspace's rule 6: an ending nobody classified is a RED, not a pass.
//!
//! So [`Closed`] is this plugin's own closed vocabulary and [`Plugin::ended_because`] publishes it.
//! ⚠ The word is READ FROM THE LATCH THE CONVERGING STEP SET, never from what this plugin meant to
//! do — `OuterLoop::closing_because`'s rule, which is what makes it a report and not a claim. A run
//! that answered and was then cancelled has taken NO ending here, and its latch is empty.

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::consent::Consents;
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness};
use crate::run::RunContext;

sprag_vt::closed_set! {
/// ⛔⛔⛔⛔⛔ **WHICH OF [`Answer`]'s TWO CONVERGING ENDINGS CLOSED THE RUN** — register item 912,
/// and the vocabulary [`Plugin::ended_because`] publishes for this plugin.
///
/// # ⚠⚠⚠ Why a word, when the module doc argues no new word was needed
///
/// That argument is about [`OutcomeState`](crate::driver::OutcomeState), and it holds: both endings
/// below are genuinely `converged`, and a sixth outcome word would move a value space every journal
/// reader decodes whole. What it does not settle is the question a reader actually asks of an
/// `answer` run — *did my consent land?* — which the outcome word cannot answer because it is the
/// same word either way. Register item 594 measured that exact collapse one field over, and item
/// 706's repair is the one reused here: the fact already exists, in the step's own note, as prose.
/// **Lifting the word out of the sentence and giving it a key is the whole of it.**
///
/// ⚠⚠ [`Outcome::answered`](crate::driver::Outcome::answered) is NOT this, and folding the two
/// would be the second authority [`Plugin::ended_because`]'s doc warns about. That counter says how
/// many decisions this run took on somebody's behalf and is published on EVERY terminal state — a
/// cancelled run that had already answered carries `1`. This says which ending CLOSED the run, and
/// a run that was cancelled took none. Read together they separate *it answered and stopped* from
/// *it answered and something else stopped it*; read as one they cannot.
///
/// ⚠ The vocabulary is this plugin's own and no driver interprets it —
/// [`Plugin::ended_because`]'s rule, and the reason this type lives here rather than beside the
/// trait.
///
/// ⚠ A [`closed_set!`](sprag_vt::closed_set) rather than a bare enum with a hand-written `ALL`,
/// which is what [`DoneReason`](crate::outer::DoneReason) still is: the array is COUNTED from the
/// variant list, so a third ending cannot be added and left out of the gate below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Closed {
    /// **THE CONSENT ANSWERED THE QUESTION, AND THIS RUN WAS ASKED FOR EXACTLY ONE.** The
    /// dialog the caller quoted is gone, an option they named was taken, and
    /// [`Outcome::answered`](crate::driver::Outcome::answered) is `1`.
    Answered,
    /// **NOBODY WAS ASKING BY THE TIME THIS RAN, SO THERE WAS NOTHING TO ANSWER.** Not one byte
    /// was typed and [`Outcome::answered`](crate::driver::Outcome::answered) is `0`.
    ///
    /// ⚠⚠ **THE READER'S REMEDY IS THE OPPOSITE OF [`Answered`](Self::Answered)'s**, which is
    /// why this is a word and not a zero to be inferred: a supervisor who read `blocked`,
    /// decided, and called is being told their decision did not land, and the reason is a RACE
    /// — the person sitting at that pane answered first, or the peer moved on. Nothing here
    /// failed; nothing here acted either.
    NothingToAnswer,
}
}

impl Closed {
    /// **THE WORD THIS PLUGIN PUBLISHES** as [`Outcome::done_reason`](crate::driver::Outcome::done_reason).
    ///
    /// ⚠ Non-empty and distinct across the arms, and a gate says so rather than this line —
    /// [`DoneReason::word`](crate::outer::DoneReason::word)'s arrangement, for its reason: a word
    /// that collided or emptied would read back as some other ending, or as no ending at all.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::NothingToAnswer => "nothing_to_answer",
        }
    }

    /// The ending named by `word`, or [`None`] for a word outside the closed set.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|closed| closed.word() == word)
    }

    /// **WHAT A READER OF THE RUN SHOULD DO ABOUT IT** — prose, and deliberately not the arm's own
    /// word, for [`DoneReason::describe`](crate::outer::DoneReason::describe)'s reason.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Answered => {
                "the option this run was consented to take was taken and the dialog is gone, so \
                 the decision landed — this run was asked for exactly one answer and gave it"
            }
            Self::NothingToAnswer => {
                "nothing was asking by the time this run reached the pane, so NOTHING WAS TYPED \
                 and the decision did not land — the person at that pane answered first, or the \
                 peer moved on; read the pane again before deciding whether to call once more"
            }
        }
    }
}

/// A one-shot answer to whatever ONE pane's peer is asking, on a [`Consent`](crate::consent::Consent) the caller wrote.
///
/// ⚠ It holds a [`Readiness`] with NO readiness condition, and that is not a shortcut. A pane whose
/// program has not started cannot be showing a dialog, so there is nothing for a barrier to wait
/// for — what this needs from that type is the other half of it, the one door to a blocked peer.
pub struct Answer {
    /// The pane whose peer is being answered.
    pane: PaneId,
    /// The one door to a keystroke, carrying the caller's consent.
    door: Readiness,
    /// Whether the answer has been given, so the next step converges rather than looking again.
    ///
    /// ⚠⚠ **A LATCH, because this is not a guard.** [`Verdict::Answered`] means *keep going* for
    /// the loop plugins, and rightly: a fifty-turn run whose peer asks on every turn answers on
    /// every turn. This plugin was asked to answer THE question that was on the screen, and going
    /// round again would make it stand watch over a pane nobody asked it to watch — answering a
    /// SECOND dialog the caller never saw, on a consent written for the first.
    given: bool,
    /// ⛔⛔⛔⛔⛔ **WHICH ENDING THIS RUN CLOSED UNDER, ONCE IT HAS** — register item 912, and
    /// [`None`] for every run still going and every run something else stopped.
    ///
    /// ⚠⚠⚠ **SET AT THE TWO SITES THAT PUBLISH [`Verdict::Converged`] AND NOWHERE ELSE**, which is
    /// what keeps it a report. Latching it where the answer is TAKEN would have been one line
    /// shorter and wrong: a run that answers and is then cancelled ended on nobody's terms, and
    /// [`Outcome::done_reason`](crate::driver::Outcome::done_reason) reserves [`None`] for exactly
    /// that case. The driver reads this immediately after each step and keeps the last non-`None`,
    /// so a word written early would outlive the ending it was about.
    ///
    /// ⚠⚠ **NOT DERIVABLE FROM [`given`](Self::given), which is why it is a second field rather
    /// than a second reading of one.** That latch says *the answer was taken* and is true for a
    /// whole step before anything converges; this says *this run is over, on this ground*. The pair
    /// `(given, closed)` has three reachable states and each is a different fact.
    closed: Option<Closed>,
}

impl Answer {
    /// Answer `pane`'s peer under `consent`, once.
    #[must_use]
    pub fn new(pane: PaneId, consent: Consents) -> Self {
        Self {
            pane,
            // ⚠ `None` readiness, `None` timeout: see the struct's own note. The consent is the
            // whole content of this plugin, so it is not an `Option` here as it is on the others —
            // a run that may answer nothing has nothing to do.
            //
            // ⚠⚠⚠ AND `Attended::NoOne`, WHICH IS A DECISION AND NOT A DEFAULT. The three looping
            // plugins can be told a person is watching their pane, because they are declared in
            // advance and left alone. This one is CALLED BY that person — a supervisor quoting the
            // dialog they are looking at — so the caller is the human a wait would be waiting for,
            // and waiting would leave them blocked on their own answer. It is `may_answer`'s
            // one-clause shape at this door for the same reason.
            door: Readiness::new(None, None, Some(consent), crate::readiness::Attended::NoOne),
            given: false,
            closed: None,
        }
    }
}

impl Plugin for Answer {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        if self.given {
            // ⚠ NOTHING IS READ HERE. The barrier already waited for the peer to LEAVE the
            // question before it reported an answer, so there is no further evidence to collect —
            // and a second look would be this plugin forming an opinion about a dialog that
            // appeared after the one it was sent to answer.
            //
            // ⛔ AND THE ENDING IS NAMED HERE, at the step that takes it — register item 912. The
            // note below has always said which of the two this is; `Closed` is that same fact with
            // a key on it, so a reader asking *did my consent land* stops parsing a sentence.
            self.closed = Some(Closed::Answered);
            return Ok(Step::new(Cost::Bytes(0), Verdict::Converged)
                .noting("the answer was taken, and this run was asked for exactly one"));
        }
        // ⚠⚠ A `match`, never `== Reached::Yes`. R365 measured three plugins comparing this
        // against a single variant, so a barrier that learned a new answer was IGNORED by all of
        // them and the run fell through to a keystroke. Exhaustive means a fifth answer cannot
        // reach a pane unread.
        // ⚠⚠⚠ A PEER THAT HAS LEFT IS A VERDICT, NOT A FAILURE — register item 336(c), and the
        // reason is sharpest at THIS plugin: answering is an act taken on somebody's consent, and
        // *the program you authorised me to answer had already gone* is a different thing to be
        // told than *the run failed*. The refusal's own sentence rides along either way; what
        // changes is that `peer_gone` is a WORD the run's outcome carries, in the same vocabulary
        // as `converged`. ⚠ Every other `PaneError` still propagates.
        let reached = match self.door.reached(panes, self.pane, run) {
            Ok(reached) => reached,
            Err(PaneError::PeerGone(pane)) => {
                let note = PaneError::PeerGone(pane).to_string();
                return Ok(Step::new(Cost::Bytes(0), Verdict::PeerGone(pane)).noting(note));
            }
            Err(other) => return Err(other),
        };
        match reached {
            // The peer is not asking. Nothing was typed and nothing is charged — see the module
            // doc for why the run still converges and what says it answered nothing.
            //
            // ⛔ AND THE OTHER ENDING IS NAMED HERE — register item 912, the sibling of the latch
            // at the top of this method. These are the only two places this plugin converges, and
            // naming the ending at each is what makes *nothing was asking* reach a row rather than
            // stopping at a note nobody parses.
            Reached::Yes => {
                self.closed = Some(Closed::NothingToAnswer);
                Ok(
                    Step::new(Cost::Bytes(0), Verdict::Converged).noting(format!(
                        "pane {} is not asking anything, so there was nothing to answer",
                        self.pane.0
                    )),
                )
            }
            // ⚠⚠ UNREACHABLE FROM HERE, AND SAID SO RATHER THAN CLAIMED TESTED. This barrier is
            // built with NO readiness condition, so `Readiness::reached` never enters the wait
            // that produces this answer: it either finds the peer asking (and answers), or reports
            // `Yes` off the latch `Readiness::new` set at construction. There is no state a gate
            // could build to reach this arm, so no gate does.
            //
            // ⚠ It is still written, and as `Continue` rather than a panic, because the compiler
            // requires the arm and a barrier that later learns to wait would arrive here for real.
            // `Continue` hands the ending to the Driver's loop top, which is the one place that
            // knows WHY a run stopped — the same deferral the three looping plugins make.
            Reached::RunEnded(why) => Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting(format!("the run ended before the pane was read: {why}"))),
            // ⚠⚠ UNREACHABLE FROM HERE FOR THE ARM ABOVE'S REASON, and a stronger one: this
            // barrier is built with `Attended::NoOne`, so it never waits for a person and never
            // reports that one came. Written, not panicked on, because the compiler requires it
            // and because a later round that decides this door SHOULD wait must land somewhere
            // honest — and `Continue` is honest: a person answered, so there is nothing left for
            // this run to answer, and the next step's `given` latch converges it.
            Reached::Attended(attention) => {
                Ok(Step::new(Cost::Bytes(attention.bytes()), Verdict::Continue)
                    .noting(attention.describe()))
            }
            // ⚠⚠ REACHABLE HERE, unlike the arm above it. This plugin passes `Attended::NoOne`, so
            // no wait can produce `Attended` — but the interruption check is not a wait: it reads a
            // fact about the pane on every step, and this tool's caller is a person who may well be
            // typing into that pane with their other hand. Terminal, for the same reason as
            // everywhere else: the answer this run was about to send is exactly what must not land
            // underneath somebody choosing an option by hand.
            Reached::Interrupted(interruption) => {
                let note = interruption.describe();
                Ok(Step::new(Cost::Bytes(0), Verdict::TakenOver(interruption)).noting(note))
            }
            // ⚠⚠ UNREACHABLE HERE, and for a reason worth stating rather than the one above it.
            // This door is built with `Attended::NoOne`, whose `handback()` is `Handback::Never` by
            // construction, so the wait this arm reports cannot be entered from this plugin at all.
            // ⚠ The type still forces the arm, and `Continue` is the honest landing: a person who
            // took the pane and gave it back has changed nothing about the answer this tool was
            // asked to give, and the next step meets the same barrier.
            Reached::HandedBack(handover) => {
                Ok(Step::new(Cost::Bytes(0), Verdict::Continue).noting(handover.describe()))
            }
            // Asking, and the consent did not name one option on it. Terminal, carrying the
            // question and the reason — which for this plugin is the whole answer the caller
            // wanted, since the reason is what they can act on.
            Reached::Asking(asking) => {
                let note = format!("nothing was answered: {}", asking.explain());
                Ok(Step::new(Cost::Bytes(asking.bytes()), Verdict::Blocked(asking)).noting(note))
            }
            Reached::Answered(answered) => {
                self.given = true;
                let (note, cost) = (answered.describe(), answered.bytes);
                Ok(Step::new(Cost::Bytes(cost), Verdict::Answered(answered)).noting(note))
            }
        }
    }

    /// ⚠ **NOTHING**, and the reason is this plugin's whole shape rather than a default it
    /// inherited.
    ///
    /// [`Plugin::driving`] asks what a run cut short must STOP, and the answer is the pane whose
    /// job this run set going. This one sets nothing going: it presses at most two keys at a peer
    /// somebody else started, and a cancel landing mid-answer must not stop that peer — it is
    /// somebody's agent, mid-turn, and interrupting it because an answer was cancelled would be
    /// this product ending work it never started.
    fn driving(&self) -> Option<PaneId> {
        None
    }

    /// ⛔⛔⛔⛔⛔ **WHICH OF THIS PLUGIN'S TWO CONVERGING ENDINGS CLOSED THE RUN** — register item
    /// 912, and [`None`] until one of them has.
    ///
    /// ⚠ Read off the latch the converging step set, never recomputed from `given` or from the
    /// pane: this plugin's whole subject is a dialog that may already be gone, so a reading taken
    /// after the fact would be about a different screen. See [`Closed`].
    fn ended_because(&self) -> Option<&'static str> {
        self.closed.map(Closed::word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::asking_peer;

    /// A one-clause consent for the fixture's permission question, authorising the option
    /// carrying `answer`.
    fn consent_to(answer: &str) -> Consents {
        Consents::of(vec![
            crate::consent::Consent::parse(
                "Do you want to proceed?".to_string(),
                answer.to_string(),
            )
            .expect("two needles"),
        ])
        .expect("a non-empty list")
    }

    /// ⚠⚠⚠ **THE ANSWER IS GIVEN ONCE, AND THE SECOND STEP DOES NOT LOOK AGAIN.**
    ///
    /// The latch is the difference between *"answer this question"* and *"stand watch over this
    /// pane"*, and only the first is what a caller asked for. Without it, a peer that shows a
    /// SECOND dialog after taking the first answer would be answered again — on a consent written
    /// for a question the caller has already seen, against one they have not.
    ///
    /// Driven through a real pty peer whose marker is already on option 1, so the answer is an
    /// Enter and nothing else, and the fixture reports which byte moved it.
    ///
    /// ⚠ REVERT-PROOF: drop the `given` latch and the second step reads the pane again, which
    /// against this fixture (whose `took` screen is not a menu) converges with the WRONG note —
    /// `is not asking anything` — for a run that plainly answered something.
    /// ⚠⚠⚠⚠ **A QUESTION WHOSE PROGRAM HAS ALREADY LEFT IS `peer_gone`, NOT A FAILED RUN** —
    /// register item 336(c), which gave the word to `Orchestrator` and `AiLoop` and left this
    /// plugin propagating.
    ///
    /// The reason is sharpest here of all five: answering is an act taken on somebody's CONSENT,
    /// and *the program you authorised me to answer had already gone* is a different thing to be
    /// told than *the run failed*. Both carry the same sentence; only one puts the ending in the
    /// vocabulary `converged` and `blocked` are in, where a journal can be asked which runs stopped
    /// that way.
    ///
    /// ⚠⚠ The fixture is a peer that DRAWS its menu and then exits — so the screen still shows a
    /// question the barrier reads and tries to answer, while the pane is provably at EOF. That is
    /// the arrangement in which the door types at nobody, and it is what `PaneAccess::inject`
    /// refuses.
    #[test]
    fn a_question_whose_peer_has_gone_is_answered_with_the_word_for_it() {
        let (access, pane) = crate::testing::peer_running(
            "printf 'Bash command\\r\\nDo you want to proceed?\\r\\n'; \
             printf '\\342\\235\\257 1. Yes\\r\\n  2. Yes, and do not ask again\\r\\n'; \
             printf '  3. No, and tell me what to do\\r\\n'"
                .to_string(),
        );
        let began = std::time::Instant::now();
        while access.pane_eof(pane) != Some(true)
            && began.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            access.pane_eof(pane),
            Some(true),
            "⚠ THE FIXTURE: the peer must have EXITED, or this is about a living pane",
        );
        assert!(
            crate::readiness::peer_asking(&access, pane).is_none()
                || access
                    .pane_collapsed(pane)
                    .unwrap_or_default()
                    .contains("1."),
            "⚠ and its question must still be ON the screen for the door to read",
        );

        let mut plugin = Answer::new(pane, consent_to("Yes"));
        let step = plugin
            .step(&access, &RunContext::uncancellable())
            .expect("⚠⚠⚠ a peer that has gone must be a VERDICT, not an error a reader greps");
        assert_eq!(
            step.verdict,
            Verdict::PeerGone(pane),
            "⚠⚠⚠⚠ THE REFUSAL PROPAGATED. A run told `failed` sends its caller looking for a bug in \
             the answering; the fact is that the program they consented to answer was already gone, \
             and `peer_gone` is the word for it — naming THIS pane, so a caller with several knows \
             which. Got {step:?}",
        );
        assert!(
            step.note.clone().unwrap_or_default().contains("pane"),
            "and the sentence names the pane it is about: {step:?}",
        );
    }

    #[test]
    fn an_answer_is_given_once_and_the_run_then_converges() {
        let (access, pane) = asking_peer("either");
        let run = RunContext::uncancellable();
        let mut plugin = Answer::new(pane, consent_to("Yes"));

        let first = plugin
            .step(&access, &run)
            .expect("the answer is not an error");
        let Verdict::Answered(answered) = &first.verdict else {
            panic!("`Yes` is option 1's whole label, so exactly one option carries it: {first:?}");
        };
        // ⛔⛔⛔⛔⛔ REGISTER ITEM 912, AND THE HALF THAT IS EASIEST TO GET WRONG: the answer has
        // been TAKEN and this run has still ended on nobody's terms. A latch set here would ride
        // the driver's `or_else` all the way to a cancelled run's row, where
        // `Outcome::done_reason` reserves `None` for exactly this case.
        assert_eq!(
            plugin.ended_because(),
            None,
            "⚠⚠⚠ the answer was taken and the run is NOT over — a run cancelled in this gap closed \
             under no ending of this plugin's, and naming one would publish an ending that never \
             happened: {first:?}",
        );
        assert_eq!(answered.chose.number, 1);
        assert_eq!(
            first.cost,
            Cost::Bytes(1),
            "one keystroke, and it is the Enter"
        );
        assert!(
            first
                .note
                .as_deref()
                .is_some_and(|note| note.contains("Yes") && note.contains("Enter")),
            "the journal line names the option in WORDS and says which keys took it: {:?}",
            first.note,
        );

        let second = plugin
            .step(&access, &run)
            .expect("converging is not an error");
        assert_eq!(
            second.verdict,
            Verdict::Converged,
            "⚠⚠⚠ the run was asked for ONE answer: {second:?}",
        );
        assert_eq!(
            second.cost,
            Cost::Bytes(0),
            "and it spends nothing getting there",
        );
        assert!(
            second
                .note
                .as_deref()
                .is_some_and(|note| note.contains("exactly one")),
            "⚠⚠ the second step must say the run is DONE, not report on the pane — a note about \
             what the pane is showing now would mean it had looked: {:?}",
            second.note,
        );
        // ⛔⛔⛔⛔⛔ AND THE ENDING REACHES A KEY, NOT ONLY THAT SENTENCE — register item 912. The
        // note above has always carried this fact; what a consumer asking *did my consent land*
        // had was a string to parse and a spelling to keep in step by hand, which is register item
        // 594's cost arriving one plugin over.
        assert_eq!(
            plugin.ended_because(),
            Some(Closed::Answered.word()),
            "⚠⚠⚠ the run converged HAVING ANSWERED and must say so in the column item 903's gate \
             already promised a converged run: {second:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PANE THAT IS NOT ASKING IS NOT ANSWERED, AND THE RUN SAYS SO WITHOUT CHARGING.**
    ///
    /// The race this plugin lives inside: a supervisor reads `blocked`, decides, and calls — and by
    /// then the peer may have been answered by the person sitting there. A plugin that typed
    /// anything here would be putting the caller's digit into whatever the pane became.
    ///
    /// ⚠ `converged` with nothing spent is the honest report, and
    /// [`Outcome::answered`](crate::driver::Outcome::answered) `0` is what makes it distinguishable
    /// from the run that did answer — asserted here as the cost and the note, and end to end
    /// through the count.
    #[test]
    fn a_pane_that_is_not_asking_is_left_alone() {
        let (access, pane) = crate::testing::silent_peer();
        let run = RunContext::uncancellable();
        let mut plugin = Answer::new(pane, consent_to("Yes"));
        let step = plugin
            .step(&access, &run)
            .expect("a pane with no question is not an error");
        assert_eq!(
            step.verdict,
            Verdict::Converged,
            "there was nothing to answer: {step:?}",
        );
        // ⛔⛔⛔⛔⛔ AND THE ROW SAYS WHICH OF THE TWO CONVERGING ENDINGS THIS IS — register item
        // 912, and this is the arm the item was filed over. A supervisor who read `blocked`,
        // decided, and called is being told their decision DID NOT LAND, and until this word
        // existed that arrived as `converged` — byte-identical to the run that answered.
        assert_eq!(
            plugin.ended_because(),
            Some(Closed::NothingToAnswer.word()),
            "⚠⚠⚠ NOTHING WAS TYPED and the run still reports `converged`. The outcome word is the \
             same one the answering run publishes, so this is the only place the difference can \
             reach a reader: {step:?}",
        );
        assert_ne!(
            Closed::NothingToAnswer.word(),
            Closed::Answered.word(),
            "⚠⚠ THE CONTROL: two endings sharing a word would report *your consent landed* for a \
             run that typed nothing, which is the defect this pair exists to end",
        );
        assert_eq!(
            step.cost,
            Cost::Bytes(0),
            "⚠⚠⚠ NOT ONE BYTE. A consent authorises an option on a question, and there is no \
             question here for it to be about",
        );
        assert!(
            step.note
                .as_deref()
                .is_some_and(|note| note.contains("not asking")),
            "and the run says which of the two zero-answer endings this is: {:?}",
            step.note,
        );
        // ⚠⚠ AND THE PANE IS THE WITNESS, not the cost this plugin reported about itself. A claim
        // about what was SENT has to be read off what was RECEIVED — R366 measured a gate that
        // watched only the outcome passing a run that typed a key it did not need.
        std::thread::sleep(std::time::Duration::from_millis(80));
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            screen.contains("AT REST") && !screen.contains("SAW"),
            "⚠⚠⚠ the peer prints `SAW <byte>` for anything typed at it, and it must have nothing \
             to print: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A CONSENT THAT DOES NOT AUTHORISE AN OPTION ENDS THE RUN WITH THE QUESTION.**
    ///
    /// The reason travels with it, because *"I gave no consent"* and *"I gave one and it did not
    /// fire"* have completely different remedies — and at this door the first is unrepresentable
    /// (the consent is the call), so every refusal a caller can meet here is one they can fix by
    /// re-reading the dialog.
    #[test]
    fn a_consent_that_names_no_option_stops_with_the_question_and_the_reason() {
        let (access, pane) = asking_peer("either");
        let run = RunContext::uncancellable();
        let step = Answer::new(pane, consent_to("Maybe"))
            .step(&access, &run)
            .expect("a refusal is not an error");
        let Verdict::Blocked(unanswered) = &step.verdict else {
            panic!("no option carries `Maybe`: {step:?}");
        };
        assert_eq!(unanswered.why(), crate::consent::Refusal::NotOffered);
        assert_eq!(step.cost, Cost::Bytes(0), "and nothing was typed");
        assert!(
            unanswered
                .question()
                .is_some_and(|question| question.choices.len() == 3),
            "the question comes back with it — that is what the caller has to answer",
        );
        std::thread::sleep(std::time::Duration::from_millis(80));
        let screen = access.pane_collapsed(pane).unwrap_or_default();
        assert!(
            !screen.contains("SAW") && !screen.contains("TOOK"),
            "⚠⚠⚠ NOT ONE KEY, and the pane is the witness: {screen:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **THIS PLUGIN STOPS NOTHING.** A run cut short must not interrupt the peer it answered:
    /// that peer is somebody's agent, mid-turn, and it was already working before this run existed.
    #[test]
    fn an_answer_run_has_no_job_of_its_own_to_stop() {
        assert_eq!(Answer::new(PaneId(7), consent_to("Yes")).driving(), None);
    }

    /// ⛔⛔⛔⛔⛔ **AND THE WORD REACHES THE RUN'S OWN ENDING, NOT ONLY THIS PLUGIN'S METHOD** —
    /// register item 912's done-when ⑴, which asks for the ROW rather than the reader one step
    /// behind it.
    ///
    /// # ⚠⚠⚠ Why the two gates above are not this one
    ///
    /// They drive `step` and ask [`Plugin::ended_because`]. Between that answer and a row a person
    /// reads there is a [`Driver`](crate::driver::Driver) that latches the word with `or_else` and
    /// an [`Outcome`](crate::driver::Outcome) that carries it — and the whole of item 912 is a fact
    /// that existed at one end and did not arrive at the other. A gate that stops at the plugin
    /// measures the half that was never in doubt.
    ///
    /// ⚠ `Answer` is the plugin the item was filed against, so this drives THAT one end to end;
    /// the same road is asserted for `orchestrator` and `agent` inside their own converging gates.
    #[test]
    fn a_run_that_converged_because_nothing_was_asking_says_so_in_its_outcome() {
        let (access, pane) = crate::testing::silent_peer();
        let outcome = crate::driver::Driver::new(crate::driver::Guardrails {
            max_iterations: Some(4),
            max_cost: None,
            max_duration: Some(std::time::Duration::from_secs(20)),
        })
        .run(
            &mut Answer::new(pane, consent_to("Yes")),
            &access,
            &RunContext::uncancellable(),
        );
        assert_eq!(
            (
                outcome.state.clone(),
                outcome.done_reason.as_deref(),
                outcome.answered,
            ),
            (
                crate::driver::OutcomeState::Converged,
                Some(Closed::NothingToAnswer.word()),
                0,
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 912: the row a person reads has to carry the ground, and \
             `converged` alone is the word an answering run publishes too. ⚠ `answered` is beside \
             it rather than folded into it — that counter says how many decisions this run took on \
             somebody's behalf and is published on every ending, and a cancelled run that had \
             already answered carries 1: {outcome:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⛔⛔⛔⛔⛔ **EVERY ENDING THIS PLUGIN CAN CLOSE UNDER HAS A WORD OF ITS OWN, AND THE WORD
    /// READS BACK** — register item 912, and the vocabulary half of it.
    ///
    /// # ⚠⚠ What each clause is FOR, rather than a list of properties
    ///
    /// * **Non-empty**, because an empty word reaches
    ///   [`Outcome::done_reason`](crate::driver::Outcome::done_reason) as `Some("")` and every
    ///   reader that tests truthiness reads it back as *no ending at all* —
    ///   [`DoneReason::word`](crate::outer::DoneReason::word) carries the same clause for the same
    ///   reason, learned against a Lua datamodel where `''` is TRUE.
    /// * **Distinct**, because two endings sharing a word is precisely the collapse this type was
    ///   built to end: it would report *your consent landed* for a run that typed nothing.
    /// * **Round-trips through [`Closed::named`]**, because the word survives the daemon in a
    ///   durable log and comes back as a string; a reader that cannot turn it back into an ending
    ///   has a key it can print and not one it can ask questions of.
    /// * **An unknown word is [`None`]** rather than the closest arm, which is
    ///   `outcome_from_words`'s rule: naming a neighbour would send somebody to fix a thing that
    ///   was never wrong.
    ///
    /// ⚠ The POPULATION is [`Closed::ALL`], which `closed_set!` counts from the variant list — so
    /// a third ending cannot be added and left out of this gate, which is the residue a
    /// hand-written array leaves.
    #[test]
    fn every_ending_this_plugin_converges_under_has_a_word_of_its_own_that_reads_back() {
        let words: std::collections::BTreeSet<&str> = Closed::ALL
            .iter()
            .map(|closed| Closed::word(*closed))
            .collect();
        assert_eq!(
            words.len(),
            Closed::ALL.len(),
            "⛔⛔⛔ TWO ENDINGS SHARE A WORD, which is the collapse this vocabulary exists to end: \
             {words:?}",
        );
        for closed in Closed::ALL {
            assert!(
                !closed.word().is_empty(),
                "⛔ an empty word reads back as NO ending at all: {closed:?}",
            );
            assert_eq!(
                Closed::named(closed.word()),
                Some(closed),
                "⛔⛔ the word survives the daemon as a string and has to come back as an ENDING, \
                 or a reader has a key it can print and not one it can ask about: {closed:?}",
            );
            assert!(
                !closed.describe().is_empty(),
                "⚠ and a reader who does not know the word needs the sentence: {closed:?}",
            );
        }
        assert_eq!(
            Closed::named("converged"),
            None,
            "⚠⚠ AND A WORD FROM OUTSIDE THIS SET IS NOT THE NEAREST ARM. `converged` is the \
             OUTCOME word every one of these endings publishes, and answering it here would fold \
             the two vocabularies this plugin keeps apart",
        );
    }
}
