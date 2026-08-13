//! `Answer` — *answer the question ONE pane's peer is asking, and stop*.
//!
//! The fifth plugin, and the only one that is not a loop. The other four exist to keep a peer
//! WORKING; this one exists for the moment a peer stops and asks, when the decision has already
//! been made by whoever is watching.
//!
//! # ⚠⚠⚠ Why a surface that can SAY what a peer is asking must also be able to answer it
//!
//! R365 gave a blocked pane a verdict, R366 gave a run a [`Consent`], and R367 put the question
//! itself on the pane-level surface — so an agent watching a sibling pane can read the dialog, the
//! options, and which one a bare Enter would take. What it could not do was answer. The only door
//! was the RUN surface's `may_answer`, which is a consent declared BEFORE a loop starts, and a
//! supervisor that has just read the question on its neighbour's screen has no loop to declare it
//! on.
//!
//! What that left is the shape this crate keeps finding defects in: **the unsafe act was the only
//! reachable one.** A caller who could see `2. No, and tell Claude what to do differently` and
//! wanted to answer it had `send_keys` — a raw digit and a raw Enter, with none of
//! [`Consent`]'s protections:
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
//! [`Consent`] the run surface takes — *"there is one door, and what it may type is what the caller
//! wrote down"*, now with four plugins behind it instead of three.
//!
//! # What makes this DIFFERENT from a one-iteration `orchestrator`
//!
//! An orchestrator with a stimulus is a plugin that TYPES ITS OWN TEXT and treats a question as a
//! reason to stop. This one has no stimulus at all: the only bytes it can ever emit are the ones
//! [`Consent::covers`] authorised, and it converges the moment it has answered. A caller cannot
//! use it to drive a pane, which is what makes it safe to point at a pane that is already blocked.
//!
//! # ⚠⚠ The three endings, and why none of them needed a new word
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

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::consent::Consent;
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness};
use crate::run::RunContext;

/// A one-shot answer to whatever ONE pane's peer is asking, on a [`Consent`] the caller wrote.
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
}

impl Answer {
    /// Answer `pane`'s peer under `consent`, once.
    #[must_use]
    pub fn new(pane: PaneId, consent: Consent) -> Self {
        Self {
            pane,
            // ⚠ `None` readiness, `None` timeout: see the struct's own note. The consent is the
            // whole content of this plugin, so it is not an `Option` here as it is on the others —
            // a run that may answer nothing has nothing to do.
            door: Readiness::new(None, None, Some(consent)),
            given: false,
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
            return Ok(Step::new(Cost::Bytes(0), Verdict::Converged)
                .noting("the answer was taken, and this run was asked for exactly one"));
        }
        // ⚠⚠ A `match`, never `== Reached::Yes`. R365 measured three plugins comparing this
        // against a single variant, so a barrier that learned a new answer was IGNORED by all of
        // them and the run fell through to a keystroke. Exhaustive means a fifth answer cannot
        // reach a pane unread.
        match self.door.reached(panes, self.pane, run)? {
            // The peer is not asking. Nothing was typed and nothing is charged — see the module
            // doc for why the run still converges and what says it answered nothing.
            Reached::Yes => Ok(
                Step::new(Cost::Bytes(0), Verdict::Converged).noting(format!(
                    "pane {} is not asking anything, so there was nothing to answer",
                    self.pane.0
                )),
            ),
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
            Reached::RunEnded => Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting("the run ended before the pane was read")),
            // Asking, and the consent did not name one option on it. Terminal, carrying the
            // question and the reason — which for this plugin is the whole answer the caller
            // wanted, since the reason is what they can act on.
            Reached::Asking(asking) => {
                let note = format!("nothing was answered: {}", asking.why().describe());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::asking_peer;

    /// A consent for the fixture's permission question, authorising the option carrying `answer`.
    fn consent_to(answer: &str) -> Consent {
        Consent::parse("Do you want to proceed?".to_string(), answer.to_string())
            .expect("two needles")
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
        let step = Answer::new(pane, consent_to("Yes"))
            .step(&access, &run)
            .expect("a pane with no question is not an error");
        assert_eq!(
            step.verdict,
            Verdict::Converged,
            "there was nothing to answer: {step:?}",
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
}
