//! WHEN a message is copied out of sprag to reach a person who is not looking at it — the policy
//! both display clients perform, and neither one owns.
//!
//! # Why this is here rather than in a client
//!
//! [`Forward`] was born in `sprag-tui` (R319), because the terminal client was the only one that
//! could copy anything out: it has a host terminal to write an `OSC 9` to. `sprag-gui` has a
//! desktop instead, and the DELIVERY is completely different — a program the window runs rather
//! than bytes down a pipe — but the QUESTION is identical, it is spelled by one option in one file
//! ([`crate::options::NOTIFY_OUTWARD`]), and a person who sets it expects it to mean the same thing
//! whichever client reads it next.
//!
//! Two copies of a three-value enum is the exact shape the debt sweep keeps catching — *two
//! frontends calling different methods for one action* — so the policy lives once, in the crate
//! that owns the option table, and each front keeps only its own transport. That also lets
//! [`crate::options::NOTIFY_OUTWARD_VALUES`] be DERIVED from the type rather than held level with
//! it by a test, which is what `window-size` already does and what `detach-on-destroy` cannot.

use sprag_vt::Urgency;

use crate::options::{NOTIFY_OUTWARD, Options};
use crate::report::Severity;

sprag_vt::closed_set! {
    /// WHEN a message is copied out of the client that received it, to reach a person who is not
    /// looking — the `notify-outward` option's three values.
    ///
    /// Three and not a switch, because the middle one is the point and the outer two are the
    /// answers a person gives when it is wrong for them. `off` is the silence sprag had before this
    /// existed; `always` is for a client that cannot tell where the person is (a terminal that
    /// reports no focus — DEC 1004 is not universal) or a person who wants the notification
    /// regardless.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub enum Forward {
        /// Never copy a message out. The client's own surface is the only delivery.
        Off,
        /// Copy it out only while the person is AWAY — the default, for the reason the module docs
        /// give: a message a person can already read is not news.
        #[default]
        Unfocused,
        /// Copy every message out, whether or not the person is looking.
        Always,
    }
}

impl Forward {
    /// The word this policy is spelled with in `config.toml` and in `show-options`.
    ///
    /// ONE spelling, exactly as [`crate::report::Severity::word`] is — and here the option table
    /// itself is built from these, so the two cannot be brought out of step at all.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Unfocused => "unfocused",
            Self::Always => "always",
        }
    }

    /// The policy `word` names, DERIVED from [`word`](Self::word) by walking the closed set rather
    /// than by a second `match` that could disagree with it.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|policy| policy.word() == word)
    }

    /// The policy the user's file puts in force.
    ///
    /// A value this build does not know leaves the DEFAULT standing rather than ending the client:
    /// every stored value came through the option table's own canonicalisation, so this cannot
    /// happen for a file sprag wrote — and a display client must not lose a person's panes over an
    /// internal inconsistency a test already forbids.
    #[must_use]
    pub fn of(options: &Options) -> Self {
        options
            .get(NOTIFY_OUTWARD)
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    /// Whether this policy needs the client to know WHERE THE PERSON IS at all.
    ///
    /// What each front does with the answer differs — the terminal client gates DEC private mode
    /// 1004 and its own read-ahead on it, the windowed one already has the WM's answer for free —
    /// but the question is the policy's, so it is answered once here. A person who set `off` or
    /// `always` has asked something whose answer does not depend on where they are, and a client
    /// that can stop asking should.
    #[must_use]
    pub const fn needs_focus(self) -> bool {
        matches!(self, Self::Unfocused)
    }
}

sprag_vt::closed_set! {
    /// WHERE THE PERSON IS, as the client that took the trouble to find out reports it.
    ///
    /// Two arms and not three: an "unknown" arm would be a state every client that cannot ask sits
    /// in forever, and every reader would then have to decide what to do about it — which is the
    /// decision this type makes ONCE, by starting at [`Person::Here`]. What reports this is a
    /// CHANGE (a terminal's focus report, a window manager's activation), so a client that has been
    /// told nothing has been told nothing has changed.
    ///
    /// Whether a client tracks the answer AT ALL is a separate question, spelled as an
    /// `Option<Person>` by the loop that owns it: `None` says the answer was never asked for, which
    /// is not the same as *"the person is here"* and must not be readable as it. That is why this is
    /// two arms plus an `Option` rather than three arms — see [`follows`].
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub enum Person {
        /// Looking at this client. The value a client starts at, for the reason above: claiming the
        /// person is absent on no evidence would notify somebody sitting right in front of it.
        #[default]
        Here,
        /// Somewhere else — another window, another application, another screen.
        Away,
    }
}

/// Whether a message must be copied out, given the policy in force and where the person is.
///
/// `person` is [`None`] when the client never found out, which is **not** the same as the person
/// being here: it is the state of a client whose policy does not need the answer.
/// [`Forward::Unfocused`] therefore answers `false` for it — unreachable by construction (that
/// policy is exactly the one that asks), and answered rather than panicked because a display client
/// must not die over its own bookkeeping.
#[must_use]
pub fn follows(policy: Forward, person: Option<Person>) -> bool {
    match policy {
        Forward::Off => false,
        Forward::Always => true,
        Forward::Unfocused => person == Some(Person::Away),
    }
}

/// How loud a message of this severity is to whatever shows it OUTSIDE sprag — the outward half of
/// R318's projection.
///
/// # The two directions are not inverses, and that is deliberate
///
/// [`crate::attention`] maps a child's [`Urgency`] onto [`Severity`] on the way IN, collapsing `Low`
/// and `Normal` onto [`Severity::Note`]. This maps back, and `Note` becomes [`Urgency::Normal`]
/// rather than `Low` — so a child that said `u=0` is forwarded as `u=1`.
///
/// That is not information lost by accident. `u=0` means *background information; miss it and
/// nothing is lost*, and by the time a message is being copied out to a person who has left the
/// room, the product has already decided it is worth interrupting them for — the surface showed it,
/// the policy said follow them. Forwarding that as *ignorable* would be the notification
/// contradicting the decision that produced it. So `Low` is a value sprag can RECEIVE and never
/// SENDS, which is the honest shape of a projection between two scales that mean different things.
///
/// [`Severity::Alert`] is the one that matters and it is exact: an alert holds the surface until a
/// person touches a key, and `critical` is *a person is needed* in both the protocols that spell
/// this scale. Those are the same claim.
///
/// # Why it is HERE and not in a client
///
/// Because both of them project it, and they render the answer differently: the terminal client
/// writes [`Urgency::digit`] into kitty's `OSC 99`, the windowed one hands [`Urgency::word`] to the
/// desktop's notifier. One projection with two renderings is right; two projections that could
/// disagree about what a warning is worth is the duplication [`Forward`] moved here to avoid.
#[must_use]
pub const fn urgency_of(severity: Severity) -> Urgency {
    match severity {
        // A warning is *something did not work*, which is not *a person is needed* — `Alert` is the
        // arm that claims that, and it is the only one that gets the critical urgency.
        Severity::Note | Severity::Warn => Urgency::Normal,
        Severity::Alert => Urgency::Critical,
    }
}

#[cfg(test)]
mod tests {
    use sprag_vt::Urgency;

    use super::{Forward, Person, follows, urgency_of};
    use crate::options::{NOTIFY_OUTWARD, NOTIFY_OUTWARD_VALUES, Options};
    use crate::report::Severity;

    /// The default policy is the one the module argues for, read through the OPTION TABLE rather
    /// than from this enum's own `Default` — so the file's default and the type's cannot drift.
    #[test]
    fn the_option_in_force_with_the_user_silent_is_the_focus_policy() {
        assert_eq!(Forward::of(&Options::default()), Forward::Unfocused);
        assert_eq!(Forward::default(), Forward::Unfocused);
    }

    /// **The option's vocabulary IS this enum's**, both ways.
    ///
    /// The table is now BUILT from [`Forward::word`], so the halves cannot be edited apart — what
    /// this pins is the part a derivation cannot: that every offered word round-trips through the
    /// canonicalisation a user's file actually goes through, in the order `show-options` lists it.
    #[test]
    fn the_options_vocabulary_is_exactly_the_policy_set() {
        let from_policy: Vec<&str> = Forward::ALL.iter().map(|policy| policy.word()).collect();
        assert_eq!(
            NOTIFY_OUTWARD_VALUES, &from_policy,
            "the option must offer exactly the policies, in the same order",
        );
        for word in NOTIFY_OUTWARD_VALUES {
            assert!(
                Forward::parse(word).is_some(),
                "{word:?} is offered by the option and parses to no policy",
            );
        }
        let mut options = Options::default();
        for policy in Forward::ALL {
            options
                .set(NOTIFY_OUTWARD, policy.word())
                .expect("an offered value is acceptable");
            assert_eq!(Forward::of(&options), policy);
        }
    }

    /// **Only the middle policy asks where the person is**, and the outer two are why that matters:
    /// each front pays for the answer (a terminal mode and a read-ahead; a WM subscription), and a
    /// person who does not need it should not.
    #[test]
    fn the_two_unconditional_policies_need_no_answer_about_the_person() {
        assert!(Forward::Unfocused.needs_focus());
        assert!(!Forward::Off.needs_focus());
        assert!(!Forward::Always.needs_focus());
    }

    /// The whole decision table, every policy against every state a client can be in — including
    /// the one a client that never asked reports.
    #[test]
    fn a_message_follows_only_a_person_the_policy_asked_about() {
        for person in [None, Some(Person::Here), Some(Person::Away)] {
            assert!(!follows(Forward::Off, person), "off never forwards");
            assert!(follows(Forward::Always, person), "always always forwards");
        }
        assert!(follows(Forward::Unfocused, Some(Person::Away)));
        assert!(!follows(Forward::Unfocused, Some(Person::Here)));
        // A client that never asked must not forward under the policy that needs the answer: it
        // would be guessing, and the guess reaches somebody's desktop.
        assert!(!follows(Forward::Unfocused, None));
    }

    /// A fresh client says the person is HERE, which is the honest reading: nothing has been
    /// reported since it started asking, and the alternative notifies somebody who is sitting in
    /// front of it.
    #[test]
    fn a_client_that_has_just_started_asking_says_the_person_is_here() {
        assert_eq!(Person::default(), Person::Here);
    }

    /// **An ALERT is the one severity that asks for the critical urgency**, and `Low` is a value
    /// this projection never produces.
    ///
    /// Asserted over the whole severity set rather than on the interesting case, so a fourth
    /// severity could not be added without deciding what a person outside sprag should be told.
    #[test]
    fn only_an_alert_asks_for_the_critical_urgency() {
        for severity in Severity::ALL {
            let want = match severity {
                Severity::Note | Severity::Warn => Urgency::Normal,
                Severity::Alert => Urgency::Critical,
            };
            assert_eq!(urgency_of(severity), want, "{severity:?}");
        }
        assert!(
            !Severity::ALL.map(urgency_of).contains(&Urgency::Low),
            "a message being copied out has already been judged worth interrupting somebody for",
        );
    }
}
