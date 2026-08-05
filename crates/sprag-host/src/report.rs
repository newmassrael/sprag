//! What a bound action DID — the value its dispatch produces, so that a key which changed nothing
//! cannot go without saying so.
//!
//! # The defect this type removes
//!
//! Measured at `a7938b0` by running the shipped binaries rather than by reading them: a key bound
//! to `switch-client -t ghost`, where no session is called `ghost`, leaves a real `sprag-tui` on a
//! real pseudoterminal with **the screen byte-for-byte unchanged** — the same screen an UNBOUND key
//! leaves. The control that can move it is `prefix s`, whose chooser paints. So a user cannot tell
//! a mistyped session name in their config from a broken build, and neither frontend has anywhere
//! to tell them.
//!
//! The cause is a signature. Both dispatchers returned `()`, and [`Option`] is not `#[must_use]`,
//! so `slots.switch_session_named(&name);` compiles clean while discarding the only fact the daemon
//! answered. Eight call sites across two frontends did exactly that, and the comment beside two of
//! them said so in words: *"Both discard the landing (a keystroke has nowhere to paint a
//! refusal)"*.
//!
//! # The fix is the RETURN TYPE, not a call at each site
//!
//! The rival hand-emits: herdr's `execute_tui_navigate_action` (`app/input/navigate.rs`, read at
//! `9a4ce5e1`) dispatches **45** `NavigateAction` variants, returns `()`, and sets **zero** toasts
//! — their toast surface exists and is real, but nothing connects it to the keyboard, so a hand
//! that forgets is a key that is silent forever. A `Report` cannot be forgotten: it is
//! `#[must_use]`, every arm of an exhaustive `match` must produce one, and the only silent
//! constructor is [`Report::on_screen`], which is a sentence a reader can disagree with rather than
//! an absence nobody can see.
//!
//! # Where the words live
//!
//! Here, and DERIVED from the vocabulary. A sentence built at the call site is a sentence the other
//! frontend spells differently — R308's finding one surface over — so [`Report::no_such`] takes the
//! [`BoundAction`] and reads its own [`subject`](BoundAction::subject), and [`Report::nowhere`]
//! reads its own spelling. Adding a verb to the vocabulary therefore adds its report wording with
//! no second table to update.
//!
//! # What is NOT reported, and why that is not a hole
//!
//! A LANDING is not a message. `switch-client -n` that arrives somewhere changes what the client is
//! showing, and both frontends paint WHERE THEY ARE permanently — `sprag-tui` in its status line,
//! `sprag-gui` in its session tabs. A message repeating it would be noise over a fact already on
//! the screen. What no repaint can carry is the NEGATIVE: nothing moved, and the reason. That is
//! the whole of what a [`Report`] says out loud.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::keymap::BoundAction;

/// A monotonic instant, expressed as the time since this process started.
///
/// **Not [`Instant`], and the reason is a bound rather than a preference**: `sprag-gui` holds its
/// live message in a reactive cell, which requires the value to be serde-representable — the same
/// bound [`crate::chooser::Pick`] records for the same surface — and an `Instant` has no meaningful
/// serialised form. An offset from one base read at first use is monotonic exactly as `Instant` is,
/// comparable, and a value a test can pass in without sleeping.
pub type Moment = Duration;

/// The base every [`Moment`] is measured from — read once, at whichever call comes first.
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The clock deadlines are measured on.
///
/// One reader, so the two frontends and this module's own tests cannot be measuring from different
/// bases. Monotonic: it is [`Instant`] underneath, which is what makes a deadline immune to a
/// user's wall clock moving.
#[must_use]
pub fn now() -> Moment {
    STARTED.elapsed()
}

/// How long a message stays up when the options table is silent, in milliseconds.
///
/// **tmux's own `display-time`, measured on this machine rather than recalled**: `tmux 3.2a`'s
/// `show-options -g display-time` answers `750`. Spelled here AND as
/// [`options::DISPLAY_TIME`](crate::options::DISPLAY_TIME)'s default;
/// `the_display_time_default_is_the_reports_own` holds the two together — the treatment
/// `repeat-time` gets against the keymap and `history-limit` against the emulator.
pub const DEFAULT_DISPLAY_TIME: u64 = 750;

/// What one bound action DID, as the client that pressed the key must report it.
///
/// Produced by a frontend's dispatch and consumed by its message surface. Both frontends build the
/// same sentence because neither of them writes one.
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "a key's outcome that nobody paints is exactly the defect this type exists to remove — \
              show it, or say `Report::on_screen()` and be read disagreeing"]
pub struct Report(Said);

/// A [`Report`]'s content — private, so the only ways to build one are the named constructors and
/// the only way to read one is [`Report::says`].
///
/// Two arms and not three: "it worked" and "it opened a surface" are the same fact to a message
/// line, and a third arm distinguishing them would be a state nothing renders — the fiction R315
/// deleted three methods for.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Said {
    /// Nothing to say out loud.
    Nothing,
    /// One line, already in the words the user reads.
    Line(String),
}

impl Report {
    /// The action's whole effect is the screen the user is already looking at — a pane appeared,
    /// the focus moved, a surface opened, this client landed on the session its status line now
    /// names.
    ///
    /// **The one silent constructor, and it is a claim.** Naming it says *there is nothing a
    /// sentence could add here*, which a reader can check against what the frontend paints; the
    /// alternative it replaces — a dispatch arm that simply returned — said nothing at all and
    /// could not be checked by anybody.
    pub const fn on_screen() -> Self {
        Self(Said::Nothing)
    }

    /// The action named something the daemon does not have.
    ///
    /// **Both halves of the sentence are read off the ACTION** ([`BoundAction::names`]) rather than
    /// passed in, so a call site can pass neither the wrong noun nor the wrong name:
    /// `switch-client -t ghost` says *no session called "ghost"* and `select-window -t logs` says
    /// *no window called "logs"*, with no table here to keep in step with the vocabulary.
    ///
    /// It is [`names`](BoundAction::names) and NOT [`subject`](BoundAction::subject), which is a
    /// distinction this constructor was written without and a test found: `switch-client` is
    /// grouped under the CLIENT, so the first draft of this line said *no client called "ghost"*.
    ///
    /// An action that names nothing cannot have been refused for a name, and says
    /// [`nowhere`](Self::nowhere) instead — which is TRUE of any action that reached here, rather
    /// than a fourth sentence invented for a state no caller builds.
    pub fn no_such(action: &BoundAction) -> Self {
        action.names().map_or_else(
            || Self::nowhere(action),
            |(kind, name)| Self(Said::Line(format!("no {kind} called \"{name}\""))),
        )
    }

    /// The action ran, found nowhere to go, and moved nothing.
    ///
    /// Spelled with the action's OWN canonical form, so the line a user reads and the `config.toml`
    /// line that caused it are the same text: *`switch-client -l: nowhere to go`*,
    /// *`select-pane -L: nowhere to go`*. That is [`BoundAction::verb`]'s discipline one cut wider
    /// — the whole spelling rather than its first word, because which DIRECTION found nothing is
    /// the useful half.
    pub fn nowhere(action: &BoundAction) -> Self {
        Self(Said::Line(format!("{action}: nowhere to go")))
    }

    /// The line to show, or [`None`] when this action had nothing to say.
    #[must_use]
    pub fn says(&self) -> Option<&str> {
        match &self.0 {
            Said::Nothing => None,
            Said::Line(line) => Some(line),
        }
    }
}

/// A [`Report`] a client is currently showing, and when it stops.
///
/// The DEADLINE is shared for the same reason the sentence is: two frontends holding one message
/// for different lengths of time is two products. What is not shared is where the row is painted —
/// `sprag-tui` owns a status line and `sprag-gui` overlays a strip — which is [`crate::prompt`]'s
/// stated split: what must not differ belongs to the command, what must differ belongs to the
/// surface.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Message {
    /// The words, already built.
    line: String,
    /// The [`Moment`] after which this message is no longer shown.
    until: Moment,
}

impl Message {
    /// Start showing `report`, or [`None`] if it had nothing to say.
    ///
    /// `now` is passed in rather than read here so a test can drive the whole lifetime without
    /// sleeping — the discipline `sprag-detect`'s settle window already follows, and the reason
    /// this type has no clock of its own. Callers read it from [`now`].
    #[must_use]
    pub fn of(report: &Report, now: Moment, display_time: Duration) -> Option<Self> {
        report.says().map(|line| Self {
            line: line.to_owned(),
            until: now + display_time,
        })
    }

    /// The line, while `now` is inside its lifetime; [`None`] once it has expired.
    ///
    /// Asked on every paint rather than removed by a timer, so an expiry needs no second authority:
    /// a client that never repaints again shows a stale line to nobody.
    #[must_use]
    pub fn showing(&self, now: Moment) -> Option<&str> {
        (now < self.until).then_some(self.line.as_str())
    }

    /// When this message stops being shown — what a client blocking on input must wake at, so the
    /// row clears on time rather than at the next keystroke.
    #[must_use]
    pub const fn until(&self) -> Moment {
        self.until
    }
}

/// How long a message stays up, from the options table in force.
///
/// One reader for both frontends, so a client that forgot to apply the user's `display-time` would
/// be a client that did not call this at all.
#[must_use]
pub fn display_time(options: &crate::options::Options) -> Duration {
    Duration::from_millis(
        options
            .number(crate::options::DISPLAY_TIME)
            .map_or(DEFAULT_DISPLAY_TIME, u64::from),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::SwitchClientAsk;
    use crate::wire::SelectWindowAsk;
    use sprag_terminal::OrderStep;

    /// The measured defect, in one line: the action a live `sprag-tui` carried out silently now
    /// carries a sentence naming the session that is not there.
    ///
    /// **`session`, not `client`** — this action's GROUPING subject is the client, and the first
    /// draft of [`Report::no_such`] read it. That is what [`BoundAction::names`] exists to
    /// separate, and this assertion is what caught it.
    #[test]
    fn a_switch_to_a_session_that_does_not_exist_names_it() {
        let action = BoundAction::SwitchClient {
            ask: SwitchClientAsk::Named("ghost".into()),
        };
        assert_eq!(
            Report::no_such(&action).says(),
            Some("no session called \"ghost\""),
        );
        assert_eq!(
            action.subject().to_string(),
            "client",
            "the grouping subject is deliberately NOT the noun above — if these ever agree, this \
             test has stopped discriminating and `names` needs a different witness",
        );
    }

    /// The noun is the action's, not the caller's — the same constructor one level down says
    /// `window`, which is the whole reason it is not passed in.
    #[test]
    fn the_noun_of_a_refusal_is_read_off_the_action() {
        let window = BoundAction::SelectWindow {
            ask: SelectWindowAsk::Named("logs".into()),
        };
        assert_eq!(
            Report::no_such(&window).says(),
            Some("no window called \"logs\""),
        );
    }

    /// An action carrying no name degrades to the TRUE sentence rather than to an invented one, so
    /// the constructor is total without a fiction in it.
    #[test]
    fn a_refusal_for_an_action_that_names_nothing_falls_back_to_what_is_true() {
        let nameless = BoundAction::SwitchClient {
            ask: SwitchClientAsk::LastViewed,
        };
        assert_eq!(nameless.names(), None);
        assert_eq!(
            Report::no_such(&nameless).says(),
            Report::nowhere(&nameless).says(),
        );
    }

    /// A guard reports about the verb it GUARDS, so `confirm-before select-window -t logs` says
    /// what the select would have said.
    #[test]
    fn a_guarded_action_reports_on_what_it_guards() {
        let guarded = BoundAction::ConfirmBefore {
            action: Box::new(BoundAction::SelectWindow {
                ask: SelectWindowAsk::Named("logs".into()),
            }),
        };
        assert_eq!(
            guarded.names(),
            Some((crate::keymap::ActionSubject::Window, "logs")),
        );
    }

    /// A step with nowhere to go is spelled the way the user's config spells it.
    #[test]
    fn nowhere_is_spelled_as_the_binding_is() {
        let last = BoundAction::SwitchClient {
            ask: SwitchClientAsk::LastViewed,
        };
        assert_eq!(
            Report::nowhere(&last).says(),
            Some("switch-client -l: nowhere to go"),
        );
        let step = BoundAction::SwitchClient {
            ask: SwitchClientAsk::Step(OrderStep::Next),
        };
        assert_eq!(
            Report::nowhere(&step).says(),
            Some("switch-client -n: nowhere to go"),
        );
    }

    /// The silent constructor says nothing, which is what lets a surface ask one question of every
    /// outcome instead of matching on the kind.
    #[test]
    fn what_is_on_screen_says_nothing() {
        assert_eq!(Report::on_screen().says(), None);
    }

    /// A message is built only from a report that speaks, so a surface holding `Option<Message>`
    /// never has to decide whether to paint an empty line.
    #[test]
    fn a_silent_report_starts_no_message() {
        let now = now();
        assert!(Message::of(&Report::on_screen(), now, Duration::from_millis(750)).is_none());
    }

    /// The lifetime is driven by the clock the caller passes, so the expiry is testable without a
    /// sleep — and the boundary is exclusive at the deadline itself.
    #[test]
    fn a_message_stops_showing_at_its_deadline() {
        let now = now();
        let action = BoundAction::SwitchClient {
            ask: SwitchClientAsk::Named("ghost".into()),
        };
        let message = Message::of(&Report::no_such(&action), now, Duration::from_millis(750))
            .expect("a refusal speaks");
        assert_eq!(
            message.showing(now + Duration::from_millis(749)),
            Some("no session called \"ghost\""),
        );
        assert_eq!(message.showing(now + Duration::from_millis(750)), None);
        assert_eq!(message.until(), now + Duration::from_millis(750));
    }

    /// The option's default and this module's constant are one number, held together here rather
    /// than by a comment — `repeat-time`'s treatment against the keymap.
    #[test]
    fn the_display_time_default_is_the_reports_own() {
        let declared: u64 = crate::options::spec(crate::options::DISPLAY_TIME)
            .expect("display-time is an option")
            .default
            .parse()
            .expect("its default is a number");
        assert_eq!(declared, DEFAULT_DISPLAY_TIME);
    }

    /// A user's `display-time` reaches the surface through one reader, and a silent table falls
    /// back to the default rather than to zero — which would be a message nobody can read.
    #[test]
    fn display_time_follows_the_options_table() {
        let table = crate::options::Options::default();
        assert_eq!(
            display_time(&table),
            Duration::from_millis(DEFAULT_DISPLAY_TIME),
        );
    }
}
