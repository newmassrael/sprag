//! **STANDING INSTRUCTIONS FOR DIALOGS A PERSON ALREADY DECIDED ABOUT** — `ai_loop.scxml`'s
//! `screen_rules`, carried out.
//!
//! # ⚠⚠⚠ What this is, next to [`consent`](crate::consent)
//!
//! Two authorities, two acts, and keeping them apart is the whole design:
//!
//! | | [`Consents`](crate::consent::Consents) | [`ScreenRules`] |
//! |---|---|---|
//! | whose | the CALLER's, for this run | the AUTHOR's, standing across every run of a loop |
//! | where | an argument on the call | the loop document's own `<data>` |
//! | the act | **take an offered option** | **refuse the call and say what to do instead** |
//! | can it approve? | yes — that is what it is for | ⚠ **no, structurally** — see below |
//!
//! A consent says *"if it asks this, pick that"*. A screen rule says *"if it asks this, turn it
//! down and tell it this instead"* — which is the thing a person does all day and which no
//! consent can express, because the answer is not on the menu.
//!
//! # ⚠⚠⚠ A RULE DOES NOT CHOOSE THE KEY, AND THAT IS THE SAFETY PROPERTY
//!
//! The document used to author `keys` beside each rule. It does not any more, and the reason is
//! measured rather than argued: `what_a_key_does_to_a_live_agents_permission_dialog` pressed
//! Escape, Tab and `3` at a live `claude` 2.1.232's permission dialog and found
//!
//! * **Escape REFUSES THE TOOL CALL** (`User rejected write to PROBE.txt`, and the file was never
//!   created) — the same outcome as the offered `3. No`, in 25 ms;
//! * **Tab does NOT open a text box.** It leaves the dialog up and rewrites option 1 into *"Yes,
//!   and tell Claude what to do next"* — and the probe that typed into that **had its file
//!   written**.
//!
//! So a rule that could name its own key could name the key that approves, and *"nobody gets a
//! standing permission by writing a rule that happened to match"* would be a hope rather than a
//! property. [`REFUSES`] is the product's key, and a rule authors only WHICH dialog and WHAT TO
//! SAY. **An approval has its own door and this is not it.**
//!
//! ⚠ The residue, stated: [`REFUSES`] is `claude`'s answer at one version. Which key refuses is a
//! fact about an agent, so its long-term home is that agent's manifest in `sprag-detect` — and
//! `codex` is unmeasured. What stops that being silent is [`Refusal::NotDismissed`]: an agent that
//! does not take this key leaves the dialog up, nothing is typed, and the run says so.
//!
//! # ⚠⚠⚠ THE TWO PROOFS ARE ORDERED, AND NEITHER IS SUFFICIENT ALONE
//!
//! 1. **The question is gone.** Asked of the SCREEN and not of the pane's STATE, because a
//!    detector's verdict settles — see [`crate::readiness`]'s `Arrival::LeftTheQuestion`, which
//!    this crate has already paid for once against a live daemon.
//! 2. **The composer took the text**, read back off the screen before Enter — [`deliver`]'s
//!    guarantee, which [`OuterLoop::say`](crate::outer::OuterLoop) already performs.
//!
//! ⚠⚠ The second is not a substitute for the first, and the same probe is why: in its `Tab` arm
//! `deliver` reported `Confirmed { attempts: 1 }` about text it had genuinely read back off the
//! screen, **and the Enter behind it approved a file write**. A read-back proves the pane painted
//! what was typed; only the first proof says what an Enter will then mean.
//!
//! [`deliver`]: crate::deliver::deliver

use std::time::Duration;

use sprag_detect::Question;
use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::consent::{Refusal, Unanswered, question_carries};
use crate::readiness::left_the_question;
use crate::run::{RunContext, Waited, poll_until};

/// **THE KEY THAT TURNS A TOOL CALL DOWN**, and the product's rather than a caller's.
///
/// Measured against a live `claude` 2.1.232: it removes the dialog in ~25 ms and the call is
/// reported `User rejected` — identical to selecting the offered `3. No`, except that it needs no
/// matching option to exist, which is precisely the case a consent cannot reach.
///
/// See the module doc for why a rule may not name this itself.
pub const REFUSES: &str = "Escape";

/// How long the dialog has to leave the screen after [`REFUSES`] goes in.
///
/// ⚠ Sized from the SETTLE WINDOW rather than from a repaint, `readiness`'s measured rule: the
/// slowest party is not the pane but the supervisor, whose verdict is published only once a
/// candidate has held for [`sprag_detect::DEFAULT_SETTLE`]. A bound sized for the repaint races the
/// thing it is waiting for, and an end-to-end run has already lost that race once.
///
/// ⚠ The live measurement came back in 25 ms, three orders inside this. That is the point: a peer
/// that has not moved in this long did not take the key, which is a fact worth reporting rather
/// than a bound worth raising.
const DISMISSED_WITHIN: Duration = Duration::from_secs(sprag_detect::DEFAULT_SETTLE.as_secs() + 2);

/// **WHY A LIST OF STANDING INSTRUCTIONS COULD NOT BE BUILT** — each one a thing the author wrote
/// and can fix.
///
/// # ⚠⚠ Why this is a `Result` where [`Consent::parse`](crate::consent::Consent::parse) is an
/// `Option`
///
/// A consent's two needles can only be wrong by being EMPTY, which is one mistake wearing one
/// shape. A rule can be wrong in ways that look right on the page — a quote nothing carries is
/// indistinguishable from a good one until a dialog arrives — so the refusal has to say which of
/// them it is, and it has to arrive at the DOOR rather than mid-run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// The rule claims no dialog: an empty `when` is carried by EVERY question, so the rule stops
    /// being about a dialog at all and becomes *"refuse everything"* — the one shape whose damage
    /// is unbounded, since every rule here refuses a call.
    ClaimsEverything,
    /// The rule says nothing. A refusal with no instruction leaves the agent turned down and
    /// un-redirected: the turn ends, nothing was asked, and the loop waits out its bound on a peer
    /// with nothing to do. ⚠ Refusing WITHOUT redirecting is a real thing to want and it already
    /// has a door — a consent naming the dialog's own *No*.
    SaysNothing,
}

impl Malformed {
    /// The sentence a person reads, naming what to change.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::ClaimsEverything => {
                "a screen rule with an empty `when` is carried by every dialog, so it would refuse \
                 every tool call this loop's agent ever asks about — quote the words of the dialog \
                 it is meant to claim"
            }
            Self::SaysNothing => {
                "a screen rule with an empty `text` turns the call down and tells the agent \
                 nothing, which leaves it with no next thing to do — say what to do instead, or \
                 use a consent naming the dialog's own refusal if turning it down is all you want"
            }
        }
    }
}

/// ONE STANDING INSTRUCTION: which dialog it claims, and what to say once the call is turned down.
///
/// Constructed through [`parse`](Self::parse), so neither field can be empty — see [`Malformed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRule {
    /// Text the DIALOG must carry for this rule to be about it.
    when: String,
    /// What the agent is told once the call has been refused.
    text: String,
}

impl ScreenRule {
    /// The datamodel and wire key of the dialog quote, inside one element of
    /// [`ScreenRules::WIRE_KEY`].
    pub const WHEN_KEY: &'static str = "when";
    /// The datamodel and wire key of the reply, inside one element of [`ScreenRules::WIRE_KEY`].
    pub const TEXT_KEY: &'static str = "text";

    /// A rule claiming dialogs carrying `when`, answered by refusing the call and saying `text`.
    ///
    /// # Errors
    ///
    /// [`Malformed`], which names which of the two fields to change.
    pub fn parse(when: String, text: String) -> Result<Self, Malformed> {
        if when.is_empty() {
            return Err(Malformed::ClaimsEverything);
        }
        if text.is_empty() {
            return Err(Malformed::SaysNothing);
        }
        Ok(Self { when, text })
    }

    /// The text a dialog must carry for this rule to claim it.
    #[must_use]
    pub fn when(&self) -> &str {
        &self.when
    }

    /// What the agent is told once the call is refused.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether this rule is about `question`.
    ///
    /// ⚠⚠ THROUGH THE SAME MATCHER A CONSENT'S `asked` USES, and that is one authority rather than
    /// a coincidence. R383 measured that a caller can cover a working agent's dialogs by QUOTING
    /// them — which is what took the dialog-KIND taxonomy out of this document — and the moment
    /// two things in this crate match a dialog by quoted text, they must agree about what carrying
    /// a quote means. `question_carries` is where that lives.
    #[must_use]
    pub fn claims(&self, question: &Question) -> bool {
        question_carries(question, &self.when)
    }
}

/// **WHAT A LOOP SCREENS**, as a non-empty list of independent [`ScreenRule`]s.
///
/// ⚠ Empty is not representable, [`Consents`](crate::consent::Consents)' argument exactly: a list
/// with no rules screens nothing, which is what holding no list already says, and the difference
/// between *"I wrote no rules"* and *"I wrote an empty list of rules"* is one nobody can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRules {
    rules: Vec<ScreenRule>,
}

impl ScreenRules {
    /// The datamodel variable — and wire argument — these are authored in.
    pub const WIRE_KEY: &'static str = "screen_rules";

    /// A list of standing instructions, or [`None`] when there are none to hold.
    #[must_use]
    pub fn of(rules: Vec<ScreenRule>) -> Option<Self> {
        (!rules.is_empty()).then_some(Self { rules })
    }

    /// Every rule, in the order the author wrote them.
    #[must_use]
    pub fn rules(&self) -> &[ScreenRule] {
        &self.rules
    }

    /// The FIRST rule that claims `question`, or [`None`] when none does.
    ///
    /// # ⚠⚠⚠ Document order decides, where [`Consents::covers`](crate::consent::Consents::covers)
    /// refuses to
    ///
    /// Two consents naming different OPTIONS of one dialog are
    /// [`Refusal::Contradicted`], because picking either
    /// would be this product applying a precedence policy the caller never wrote — and the option
    /// it silently dropped could be *"and don't ask again"*.
    ///
    /// Neither half of that argument holds here, and both halves matter:
    ///
    /// * **The dangerous outcome does not exist.** Every rule performs the same act on the dialog —
    ///   [`REFUSES`] — so two rules claiming one question DISAGREE ONLY ABOUT WHAT TO SAY NEXT.
    ///   There is no branch on which one of them grants something.
    /// * **The precedence IS written down.** A person authoring a list of standing instructions in
    ///   a file is writing an ordered list, which is how every rule set a person hand-writes
    ///   behaves. Refusing to act on an ordered list would be refusing to read it as one.
    ///
    /// ⚠ Which rule fired is carried out in [`Screened::when`], so an ordered list is auditable
    /// rather than merely convenient: a reader can see the rule that claimed the dialog and the
    /// ones it shadowed are in the same document, in order.
    #[must_use]
    pub fn claiming(&self, question: &Question) -> Option<&ScreenRule> {
        self.rules.iter().find(|rule| rule.claims(question))
    }
}

/// **A DIALOG A STANDING INSTRUCTION GOT THE RUN PAST** — what was refused, on whose rule, and what
/// the agent was told instead.
///
/// # ⚠⚠ Why this is a value and not a log line
///
/// [`Answered`](crate::consent::Answered)'s argument, and here it is sharper. The measurement says
/// this act **refuses a tool call** the agent asked for — a decision taken on somebody's behalf,
/// with an effect outside the loop — so it has to be reportable in the run's own vocabulary rather
/// than left in prose. A run whose journal spells that `continue` is a run in which refusals are
/// indexed by nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screened {
    /// The question as it stood when it was turned down.
    pub question: Question,
    /// The quote of the rule that claimed it — WHICH standing instruction fired.
    pub when: String,
    /// What the agent was told to do instead.
    pub said: String,
    /// PTY bytes the whole act cost: the refusing key and the redirect together.
    pub bytes: u64,
}

impl Screened {
    /// ONE LINE for the run's journal — the rule that fired, and what the agent was told.
    ///
    /// ⚠ It says **refused** out loud. The act's own name (*screened*) describes the matching and
    /// not the consequence, and the consequence is the part a person auditing this run needs: a
    /// tool their agent asked to make did not happen.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "refused the peer's call with {REFUSES} on the rule quoting {:?}, and told it {:?}",
            self.when, self.said,
        )
    }
}

/// **REFUSE THE CALL `question` IS ASKING ABOUT** — the first half of a screening act, up to and
/// including the proof that the dialog is gone.
///
/// The SECOND half — saying the rule's text — is deliberately not here: it is a prompt into an
/// agent's composer, which is [`OuterLoop::say`](crate::outer::OuterLoop)'s job and already carries
/// the delivery discipline (`deliver`'s read-back, the turn contract armed before a byte goes in,
/// the cost counted). A second delivery path written here would be that discipline spelled twice.
///
/// # Errors
///
/// [`PaneError`] from the injection itself — an unencodable key or a write failure, which is a
/// failure of the run rather than a refusal of the rule.
pub(crate) fn refuse(
    panes: &dyn PaneAccess,
    pane: PaneId,
    question: &Question,
    run: &RunContext,
) -> Result<Refused, PaneError> {
    // ⚠⚠⚠ THE EVIDENCE IS CHECKED BEFORE THE KEY, not only after it, and the reason is that
    // [`REFUSES`] IS NOT HARMLESS ON A PANE WITH NO DIALOG. The question reaching here was read by
    // the barrier on the PREVIOUS pump, and in that gap a person can answer it or the agent can
    // withdraw it — and Escape typed at an agent's composer discards whatever they were in the
    // middle of writing. Every other act in this crate justifies its keystroke by the state of the
    // screen at the instant it presses; this one had the check on only one side of it.
    if left_the_question(panes, pane, question) {
        return Ok(Refused::AlreadyGone);
    }
    let bytes = panes.inject(pane, &[KeyStroke::named(REFUSES)])?.bytes();
    // ⚠⚠⚠ THE FIRST PROOF, and the `Tab` arm of the live probe is what it is for. A dialog that is
    // still up takes an Enter as an answer to ITSELF — measured, with the file written — so
    // nothing may be typed until this holds.
    match poll_until(run, DISMISSED_WITHIN, || {
        left_the_question(panes, pane, question)
    }) {
        Waited::Ready => Ok(Refused::Gone { bytes }),
        // ⚠ A run ending underneath does not un-press the key, and it does not get the redirect
        // said either. Both endings are the same sentence to whoever reads the run, and both carry
        // what was spent.
        Waited::TimedOut | Waited::Stopped => Ok(Refused::StillUp(Unanswered::refused_after(
            question.clone(),
            Refusal::NotDismissed,
            bytes,
        ))),
    }
}

/// What [`refuse`] did — and, when the dialog would not go, the run's report about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The call is turned down and the question is off the screen. **Nothing has been said yet.**
    Gone {
        /// What the refusing key cost.
        bytes: u64,
    },
    /// ⚠⚠ The key went in and the dialog is still there, so **nothing was typed**. See
    /// [`Refusal::NotDismissed`].
    StillUp(Unanswered),
    /// ⚠⚠⚠ **THE DIALOG HAD ALREADY GONE, SO NOTHING WAS PRESSED AT ALL** — not a refusal, and not
    /// a screening either.
    ///
    /// The question this act was given was read by the barrier one pump earlier; in the gap a
    /// person can answer it or the agent can withdraw it. Pressing anyway would put an Escape into
    /// an agent's COMPOSER, which discards whatever was being typed there — so this arm exists to
    /// make *nothing happened* something the driver can report, rather than a choice between two
    /// false ones. It reaches the machine as `screen.moot`.
    AlreadyGone,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule that cannot be built is a rule nobody can be surprised by later.
    #[test]
    fn a_rule_that_would_claim_everything_or_say_nothing_is_refused_at_construction() {
        assert_eq!(
            ScreenRule::parse(String::new(), "think again".to_owned()),
            Err(Malformed::ClaimsEverything),
            "⚠⚠⚠ an empty quote is carried by every dialog, and every rule REFUSES a call — so \
             this one is `turn down everything my agent ever asks to do`",
        );
        assert_eq!(
            ScreenRule::parse("proceed".to_owned(), String::new()),
            Err(Malformed::SaysNothing),
            "⚠⚠ and a refusal with no instruction leaves the agent with nothing to do next",
        );
        assert!(
            ScreenRule::parse("proceed".to_owned(), "think again".to_owned()).is_ok(),
            "the control: both halves present is the rule this type exists for",
        );
    }

    /// ⚠⚠ **A RULE CLAIMS A DIALOG BY QUOTING IT, THROUGH THE SAME MATCHER A CONSENT USES** — over
    /// the three dialogs captured from a live agent, so this is not a claim about a composed
    /// screen.
    ///
    /// R383 took the dialog-KIND taxonomy out of this design by measuring that quoting works. The
    /// risk that measurement leaves behind is TWO matchers — a consent's and a rule's — drifting
    /// apart while both are described as *"quoting the dialog"*. The assertion is that one needle
    /// reaches all three, exactly as the consent gate next door asserts for its own.
    #[test]
    fn one_rule_quoting_the_agent_claims_every_dialog_it_raised_while_working() {
        let rule = ScreenRule::parse(
            "Do you want to".to_owned(),
            "비용 무시하고 가장 장기적으로 올바른 방법으로 다시 생각하고 진행".to_owned(),
        )
        .expect("both halves are non-empty");
        for (label, rows) in crate::testing::CLAUDE_WORKING_DIALOGS {
            let question = crate::testing::parsed_dialog(rows)
                .unwrap_or_else(|| panic!("{label}: the shipping parser reads every capture"));
            assert!(
                rule.claims(&question),
                "⚠⚠ {label}: a rule quoting the agent's own words must claim this dialog, or a \
                 person cannot write a standing instruction without enumerating tools. \
                 asked={:?}",
                question.asked,
            );
        }
    }

    /// ⚠⚠ **A LIST DECIDES BY DOCUMENT ORDER, AND SAYS WHICH RULE IT WAS** — the decision
    /// [`ScreenRules::claiming`] documents, driven rather than described.
    #[test]
    fn two_rules_about_one_dialog_are_decided_by_the_order_they_were_written_in() {
        let question = crate::testing::parsed_dialog(crate::testing::CLAUDE_WRITE_DIALOG)
            .expect("the shipping parser reads the capture");
        let narrow =
            ScreenRule::parse("create PROBE.txt".to_owned(), "not that file".to_owned()).unwrap();
        let broad =
            ScreenRule::parse("Do you want to".to_owned(), "think again".to_owned()).unwrap();

        assert!(
            narrow.claims(&question) && broad.claims(&question),
            "the control: BOTH rules must be about this dialog, or the order below decides nothing",
        );
        assert_eq!(
            ScreenRules::of(vec![narrow.clone(), broad.clone()])
                .expect("a non-empty list")
                .claiming(&question)
                .map(ScreenRule::text),
            Some("not that file"),
            "⚠⚠ the first rule that claims a dialog is the one that fires",
        );
        assert_eq!(
            ScreenRules::of(vec![broad, narrow])
                .expect("a non-empty list")
                .claiming(&question)
                .map(ScreenRule::text),
            Some("think again"),
            "⚠⚠⚠ and turning the list round turns the answer round, which is what makes this an \
             ORDERED list rather than a set the product picks from",
        );
    }

    /// ⚠ A list with nothing in it is not representable — see [`ScreenRules::of`].
    #[test]
    fn an_empty_list_of_rules_cannot_be_built() {
        assert_eq!(ScreenRules::of(Vec::new()), None);
    }

    /// ⚠⚠⚠ **A DIALOG THAT HAS ALREADY GONE IS NOT PRESSED AT** — the window between the barrier's
    /// reading and this act, and why [`REFUSES`] is not harmless outside a dialog.
    ///
    /// The question `screening` is handed was read by the barrier on the PREVIOUS pump. In the gap
    /// a person can answer it, or the agent can withdraw it — and Escape typed at an agent's
    /// COMPOSER discards whatever they were in the middle of writing. Every other act in this crate
    /// justifies its keystroke by the state of the screen at the instant it presses; this one had
    /// the check on only one side of it until the round that built it swept its own work.
    ///
    /// ⚠⚠ **THE ASSERTION IS A NEGATIVE**: the pane is unchanged. A gate that only checked the
    /// return value would pass for an act that pressed the key and then noticed.
    #[test]
    fn a_dialog_that_has_already_gone_is_not_pressed_at() {
        let (access, pane) = crate::testing::silent_peer();
        let before = access.pane_collapsed(pane).expect("a readable pane");
        // A real captured dialog, which this pane is emphatically not showing — the shape the
        // barrier would have handed over one pump ago.
        let stale = crate::testing::parsed_dialog(crate::testing::CLAUDE_WRITE_DIALOG)
            .expect("the shipping parser reads the capture");

        assert_eq!(
            refuse(&access, pane, &stale, &RunContext::uncancellable())
                .expect("the pane stays readable"),
            Refused::AlreadyGone,
            "⚠⚠ a question that is not on the pane cannot be refused, and saying so is what lets \
             the machine hear *nothing happened* instead of a lie",
        );
        assert_eq!(
            access.pane_collapsed(pane).expect("a readable pane"),
            before,
            "⚠⚠⚠ AND NOTHING WAS SENT. This peer echoes every byte it is given, so an Escape that \
             went in would be visible — and on a real agent it would have thrown away whatever a \
             person was typing into the composer",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **THE KEY THIS ACT PRESSES IS ONE THIS BUILD CAN ACTUALLY SEND** — asked of the shipping
    /// encoder, offline, because the alternative is finding out from a live agent.
    ///
    /// [`REFUSES`] is a `&str` handed to [`KeyStroke::named`], which takes anything; the encoding
    /// happens at the injection, where an unknown name is [`PaneError::Encode`] — a RUN failure,
    /// mid-dialog, on a pane somebody's agent is waiting at. `sprag_input::is_key_name` is the same
    /// list the encoder consults, so a rename of `Escape` upstream, or a typo here, is a red in
    /// milliseconds instead of a live session.
    #[test]
    fn the_key_that_refuses_a_call_is_one_this_build_can_encode() {
        assert!(
            sprag_input::is_key_name(REFUSES),
            "⚠⚠⚠ {REFUSES:?} is not a key name this build encodes, so every screening act would \
             fail as `PaneError::Encode` with a dialog on the screen and a person's agent waiting",
        );
    }
}
