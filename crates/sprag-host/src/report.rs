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

use std::fmt;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use sprag_terminal::Ended;

use crate::keymap::BoundAction;

sprag_terminal::closed_set! {
    /// How much a message matters: the ORDER two of them are resolved by, and the word a surface
    /// marks one with.
    ///
    /// # It is an ORDER, and that is the whole of what it decides
    ///
    /// A severity does NOT change how long a message stays up. `display-time` is one number and it
    /// means what it says — R316 shipped that contract and a user who set 750 ms would file a bug
    /// against a build that showed them a sentence for three seconds. The rival hardcodes 8s/5s/3s
    /// per kind and reads no user setting at all (`sync_toast_deadline`, `app/api.rs`, read at
    /// `9a4ce5e1`), which is the trade this type refuses.
    ///
    /// What the order decides is [`Message::over`]: **a lower severity never takes the row from a
    /// live higher one**. A build failure standing on the row is not wiped by a note arriving a
    /// tenth of a second later, and no caller has to co-operate for that to hold.
    ///
    /// # [`Alert`](Severity::Alert) is not on a timer, and that is the point of having three
    ///
    /// A timer is a bet that the person is looking. `Note` and `Warn` take it — they are things it
    /// is fine to miss. An `Alert` is the case where missing it is the failure, so it has NO
    /// deadline: it stays until a keystroke acknowledges it (see
    /// [`Message::waits_to_be_acknowledged`]), which is the same "cleared on visit" model
    /// `sprag-gui`'s attention marker already uses. tmux has no such state, and the rival's most
    /// urgent toast is an eight-second one: step away from the desk and it is gone with no trace.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
    pub enum Severity {
        /// Something happened that is worth a glance and nothing more. The default, because a
        /// caller that did not think about severity has not claimed urgency.
        #[default]
        Note,
        /// Something did not work. Every [`Report`] this client builds for itself is one of these:
        /// a key that named a session which is not there, an edge with nowhere to go.
        Warn,
        /// Something needs the person. Stays on the row until a keystroke acknowledges it.
        Alert,
    }
}

impl Severity {
    /// The lower-case word this severity is spelled with — on the wire, on the command line, and
    /// as the mark a surface puts in front of the sentence.
    ///
    /// ONE spelling for all three, so `sprag display-message -s alert` and the row a person reads
    /// cannot come to use different words for one state.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warn => "warn",
            Self::Alert => "alert",
        }
    }

    /// The severity `word` names, or [`None`] — the reverse of [`word`](Self::word), and DERIVED
    /// from it by walking [`ALL`](Self::ALL) rather than by a second `match` that could disagree.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Every spelling this type accepts, for a usage line and a refusal that lists the candidates.
    #[must_use]
    pub fn words() -> String {
        Self::ALL.map(Self::word).join("|")
    }

    /// Whether a message already at `self` KEEPS the row when one at `arriving` lands on it.
    ///
    /// **The one comparison, and both places that resolve two messages call it.** [`Message::over`]
    /// asks it of what is on the row and [`Announcement::over`] asks it of what is waiting to be
    /// collected, one step earlier; written twice they would be two rules that must agree and
    /// nothing to make them, which is the shape this project keeps paying to remove.
    ///
    /// Strictly greater, so an EQUAL severity does not keep the row — a second refusal replaces the
    /// first rather than being swallowed by it.
    #[must_use]
    pub fn outranks(self, arriving: Self) -> bool {
        self > arriving
    }

    /// When a message of this severity, started at `now`, stops showing — or [`None`] when it waits
    /// to be acknowledged instead.
    ///
    /// Three arms and the FIRST one is the reason this is a function rather than a field:
    /// `display-time 0` is the option's documented way to put the silence back, and it has to reach
    /// every severity or the setting would be honoured for two of three. A zero deadline is a
    /// message that has already expired, which is the shape [`Message::showing`] already answers
    /// `None` for — so the silence needs no second code path anywhere.
    #[must_use]
    pub fn deadline(self, now: Moment, display_time: Duration) -> Option<Moment> {
        if display_time.is_zero() {
            return Some(now);
        }
        match self {
            Self::Alert => None,
            Self::Note | Self::Warn => Some(now + display_time),
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// Serialised as the word [`Severity::word`] gives, and read back through
/// [`Severity::parse`] — **written by hand for exactly that reason**.
///
/// `#[serde(rename_all = "lowercase")]` would have been shorter and would have introduced a SECOND
/// spelling of every variant: derive's, on the wire, beside this type's own, on the command line
/// and in front of the sentence. Two tables that must agree and nothing to make them, which is the
/// hazard this project spends whole rounds removing. Here the wire cannot disagree with the CLI
/// because there is one function.
impl serde::Serialize for Severity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.word())
    }
}

impl<'de> serde::Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `String` and NOT `&str`, and a live test is what settled it: a borrowed `&str` can only be
        // read from a deserializer that owns its buffer, so it works from `from_str` and FAILS from
        // `from_value` — which is the path the wire client takes. The symptom was a message the
        // daemon reported delivering and no client ever painted.
        let word = String::deserialize(deserializer)?;
        Self::parse(&word).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown severity \"{word}\"; it is one of {}",
                Self::words(),
            ))
        })
    }
}

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

/// A sentence somebody else asked a client to show a person — validated at the door.
///
/// # Why a TYPE and not a `String`
///
/// A [`Report`] is built by the client, from its own vocabulary, out of an action it dispatched: it
/// cannot contain anything the client did not write. A message from `sprag display-message` is the
/// opposite — arbitrary bytes chosen by an agent, a hook, or a script, on their way to being
/// **written into somebody's terminal**. A newline forges a second row; an `ESC` is an escape
/// sequence the person's own emulator obeys. `sprag-tui` happens to paint through termwiz's
/// `Change::Text`, which renders control characters inert by contract — but that is one surface's
/// property, and the day a second surface writes the same string without it, the hole is silent.
/// So the rule lives on the value, and no surface has to remember it.
///
/// This is [`PaneName`](sprag_terminal::PaneName)'s discipline (R295) applied to a payload rather
/// than to an address, down to the reason each rule exists being on the variant that enforces it.
/// The rival sanitises too (`sanitized_notification_text`, read at `9a4ce5e1`) — silently, by
/// TRUNCATING to 80 bytes and dropping what it does not like, so a caller whose message was cut is
/// told `shown` and never learns. A refusal that names the rule is the difference.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MessageText(String);

/// Why a proposed [`MessageText`] was refused — one variant per rule, so the caller is told which
/// one they broke rather than a disjunction of three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageTextError {
    /// Nothing but whitespace. A message nobody can read is not a message, and a caller that sent
    /// one by accident and a caller that meant "clear the row" are indistinguishable — so neither
    /// is served, and clearing stays what it has always been: waiting.
    Blank,
    /// Longer than [`MessageText::MAX_BYTES`]. Carries the length offered.
    TooLong(usize),
    /// Contains a control character. See [`MessageText`] for why this one has teeth.
    Control,
}

impl MessageTextError {
    /// The longest [`rule`](Self::rule) any variant answers, so a caller building a sentence AROUND
    /// one can prove its own length rather than hoping.
    ///
    /// Derived from the words below by a test (`the_rule_names_stay_inside_their_own_bound`) rather
    /// than counted here, because a constant nothing checks is the drifting-array defect wearing a
    /// different hat.
    pub const LONGEST_RULE: usize = 18;

    /// Which rule was broken, as a SHORT noun phrase — two or three words, for a sentence that has
    /// to say why inside a row it is already sharing.
    ///
    /// **[`Display`](fmt::Display) is the other audience and that is why there are two.** A caller at
    /// a command line gets the paragraph: they wrote the message, they can fix it, and the reason a
    /// newline is refused is exactly what they need. A reader of a STATUS ROW cannot fix anything —
    /// the words came from a child in one of their panes — so what they need is the pane and the
    /// rule, in a line that still fits. [`crate::attention`] is the caller, and the bound above is
    /// what lets its fallback be provably showable instead of merely short enough so far.
    #[must_use]
    pub const fn rule(self) -> &'static str {
        match self {
            Self::Blank => "no words",
            Self::TooLong(_) => "too long",
            Self::Control => "control characters",
        }
    }
}

impl fmt::Display for MessageTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "a message cannot be blank"),
            Self::TooLong(len) => write!(
                f,
                "a message is at most {} bytes, and that one is {len}",
                MessageText::MAX_BYTES,
            ),
            Self::Control => write!(
                f,
                "a message cannot contain control characters (a newline would forge a second row \
                 of the status line, and an escape would be obeyed by the terminal it is painted \
                 into)",
            ),
        }
    }
}

impl MessageText {
    /// The most bytes a message may carry.
    ///
    /// **A row, not a paragraph.** Both surfaces show one line — `sprag-tui` reserves a single
    /// bottom row and `sprag-gui` overlays a one-line strip — so a message longer than a wide
    /// terminal is bytes nobody will ever see, and accepting it would be promising a display this
    /// product does not have. 200 is comfortably past the 80 columns a status row is usually given
    /// and short of the point where the promise becomes false.
    pub const MAX_BYTES: usize = 200;

    /// Check `text` against every rule and keep it, or say which rule it broke.
    ///
    /// # Errors
    /// [`MessageTextError`], one variant per rule.
    pub fn parse(text: &str) -> Result<Self, MessageTextError> {
        if text.trim().is_empty() {
            return Err(MessageTextError::Blank);
        }
        if text.len() > Self::MAX_BYTES {
            return Err(MessageTextError::TooLong(text.len()));
        }
        if text.chars().any(char::is_control) {
            return Err(MessageTextError::Control);
        }
        Ok(Self(text.to_owned()))
    }

    /// The words, as a validated string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One message the daemon is holding for one client: the words and how much they matter.
///
/// The unit [`crate::AttachmentRegistry`] queues and a client collects. It is DELIBERATELY the same
/// pair a [`Report`] carries, and [`Report::said`] is where the two meet: a client cannot paint what
/// a person told it differently from what its own keyboard told it, because by the time either
/// reaches a surface they are one type.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Announcement {
    /// The words, already checked against every rule a terminal row imposes.
    pub text: MessageText,
    /// How much they matter.
    pub severity: Severity,
}

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
    /// One line, already in the words the user reads, and how much it matters.
    Line {
        /// The words.
        line: String,
        /// The order two of these are resolved by — see [`Severity`].
        severity: Severity,
    },
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
            |(kind, name)| Self::at(Severity::Warn, format!("no {kind} called \"{name}\"")),
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
        Self::at(Severity::Warn, format!("{action}: nowhere to go"))
    }

    /// A pane act reached a place that holds no pane.
    ///
    /// # The SUBJECT is missing, which is not [`nowhere`](Self::nowhere)'s missing TARGET
    ///
    /// `nowhere` is *"this act ran and found nowhere to go"* — the act had a subject and no
    /// destination. This is the other end: there is nothing here to act ON, so the act never ran at
    /// all. A row that reported `nowhere` for it would tell a person their pane had nowhere to go
    /// when the truth is that they were not standing on one.
    ///
    /// # It takes no argument, and that is what makes it safe for a surface with no binding
    ///
    /// Every other refusal here spells itself from a [`BoundAction`] so no call site can pass the
    /// wrong noun ([`no_such`](Self::no_such) states the rule). A GUI palette or context-menu row
    /// the KEYBOARD cannot reach has no such action to spell from — `join-pane` is
    /// [`Keystroke::NotBuilt`](crate::vocabulary::Keystroke::NotBuilt), so
    /// `Command::JoinInto::bound()` is [`None`] — and before this those rows could only stay
    /// SILENT. There is exactly one noun in this sentence and it is in the constructor's name, so
    /// the wrong-noun failure the argument-taking constructors guard against cannot occur.
    ///
    /// **It is a fallback and it is outranked**: a daemon that refused the act and SAID WHY (R325,
    /// stored by every `scene/invoke` through one funnel) wins over it at
    /// `sprag-gui`'s `message::preferred`. So this is the sentence for the case the wire never saw
    /// — a slot whose pane has gone — and never a generic word over a stated reason.
    // No `#[must_use]` here and none is missing: `Report` is `#[must_use]` as a TYPE, so every
    // constructor inherits it and clippy rejects the redundant attribute — which is the check that
    // the rule lives in one place.
    pub fn no_pane() -> Self {
        Self::at(Severity::Warn, "no pane here to act on".to_owned())
    }

    /// What somebody ELSE asked this client to show — `sprag display-message`, arriving from the
    /// daemon rather than from this keyboard.
    ///
    /// **The whole reason it is this constructor and not a second type.** A message the daemon
    /// routed and a refusal this client built for itself are the same thing to the surface that
    /// paints them, so they become one value HERE, at the door, rather than at two rendering sites
    /// that could come to disagree about the row's height, its lifetime or its mark. Making the
    /// wrong thing unrepresentable: there is no path by which a client shows a person's message in
    /// a way its own reports are not shown.
    ///
    /// The words are a [`MessageText`], so they have already been checked against the rules a
    /// terminal row imposes; nothing downstream re-checks and nothing downstream may forget to.
    ///
    /// No `#[must_use]` here and none is missing: [`Report`] carries one, with the sentence a caller
    /// who drops it needs to read. A second attribute would be a second wording of the same rule.
    pub fn said(announcement: &Announcement) -> Self {
        Self::at(announcement.severity, announcement.text.to_string())
    }

    /// The session under this client was DESTROYED and the client was moved — *"session "0" was
    /// destroyed; now on "beta""*.
    ///
    /// # Why this is the exception to *"a landing is not a message"*
    ///
    /// This module's own docs cut landings out of what a client says out loud, on the grounds that
    /// both frontends paint WHERE THEY ARE permanently, so a sentence repeating it would be noise
    /// over a fact already on screen. That argument holds for a landing the person ASKED FOR. It
    /// does not hold here, for the reason [`crate::wake::Lost`] is a value at all: nobody pressed
    /// anything. The screen changes under a person who did nothing, and the half that no repaint
    /// can carry is not where they are — it is **what happened to where they were**, which is gone
    /// from every list the moment it becomes true.
    ///
    /// [`Severity::Warn`], the same weight [`cascaded`](Self::cascaded) gives a kill that took more
    /// than it was asked for, and for the same reason: a person whose session was destroyed under
    /// them needs telling, while it is not the kind that waits to be acknowledged.
    ///
    /// # A client that is LEAVING says nothing
    ///
    /// [`Lost::Detached`](crate::wake::Lost::Detached) is [`on_screen`](Self::on_screen), and that
    /// is a claim rather than an omission: the client has no screen left to say it on. A row
    /// painted onto a surface that is being torn down in the same frame is a flash, not a message,
    /// and the honest place for that sentence is the shell the client is handing the terminal back
    /// to — a different surface, and not this one's to write.
    pub fn lost_session(lost: &crate::wake::Lost) -> Self {
        match lost {
            crate::wake::Lost::Moved { was, now } => Self::at(
                Severity::Warn,
                format!("session {was:?} was destroyed; now on {now:?}"),
            ),
            crate::wake::Lost::Detached { .. } => Self::on_screen(),
        }
    }

    /// A kill that reached PAST what the person named — *"the session went with it"*.
    ///
    /// [`Report::on_screen`] when it stopped exactly where they asked, which is the common case and
    /// needs no words: the window they killed is gone from a strip they are looking at.
    ///
    /// # Why a kill needs this and the other verbs do not
    ///
    /// A mux is nested, so `kill-window` can end a SESSION and `kill-session` can end the SERVER —
    /// and neither is discoverable by re-reading, because what would answer the question is the
    /// thing that went. Measured on a live client at `d1833df` with `detach-on-destroy next`:
    /// `prefix &` on a session's last window destroyed that session, moved the person to a
    /// neighbouring one, and **left the status row naming the session that had just died**. The
    /// daemon had said `{"ended":"session"}` on the same reply and both display clients dropped it,
    /// because `kill_window` answered `()` — the last two acting methods in [`crate::HostClient`]
    /// that did.
    ///
    /// # The clause is [`Ended::beyond`]'s and not this function's
    ///
    /// One wording for every surface (`sprag kill-window` already prints it), which is the rule
    /// that type's own doc states. What is decided HERE is only that a kill which cascaded is worth
    /// a row at all — and at [`Severity::Warn`], because a person who asked to end a window and
    /// ended a session needs telling, while it is not the kind that waits to be acknowledged.
    ///
    /// No `#[must_use]` here and none is missing, [`said`](Self::said)'s note: [`Report`] carries
    /// one already, with the sentence a caller who drops it needs to read.
    pub fn cascaded(reached: Ended, named: Ended) -> Self {
        reached
            .beyond(named)
            .map_or_else(Self::on_screen, |beyond| Self::at(Severity::Warn, beyond))
    }

    /// A spoken report at `severity` — the one private constructor the three public ones share, so
    /// a new sentence cannot be added without choosing how much it matters.
    fn at(severity: Severity, line: String) -> Self {
        Self(Said::Line { line, severity })
    }

    /// The line to show, or [`None`] when this action had nothing to say.
    #[must_use]
    pub fn says(&self) -> Option<&str> {
        match &self.0 {
            Said::Nothing => None,
            Said::Line { line, .. } => Some(line),
        }
    }

    /// How much this report matters, or [`None`] when it says nothing at all.
    ///
    /// [`None`] rather than a default, because "silent" is not a severity: a caller asking how
    /// urgent a report is that has no words has asked a question with no answer, and inventing
    /// [`Severity::Note`] for it would let a surface mark an empty row.
    #[must_use]
    pub fn severity(&self) -> Option<Severity> {
        match &self.0 {
            Said::Nothing => None,
            Said::Line { severity, .. } => Some(*severity),
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
    /// How much they matter — the order [`Message::over`] resolves two of these by.
    severity: Severity,
    /// The [`Moment`] after which this message is no longer shown, or [`None`] when it waits to be
    /// acknowledged instead ([`Severity::Alert`]).
    until: Option<Moment>,
}

impl Message {
    /// Start showing `report`, or [`None`] if it had nothing to say.
    ///
    /// `now` is passed in rather than read here so a test can drive the whole lifetime without
    /// sleeping — the discipline `sprag-detect`'s settle window already follows, and the reason
    /// this type has no clock of its own. Callers read it from [`now`].
    #[must_use]
    pub fn of(report: &Report, now: Moment, display_time: Duration) -> Option<Self> {
        let severity = report.severity()?;
        report.says().map(|line| Self {
            line: line.to_owned(),
            severity,
            until: severity.deadline(now, display_time),
        })
    }

    /// The line, while `now` is inside its lifetime; [`None`] once it has expired.
    ///
    /// Asked on every paint rather than removed by a timer, so an expiry needs no second authority:
    /// a client that never repaints again shows a stale line to nobody.
    #[must_use]
    pub fn showing(&self, now: Moment) -> Option<&str> {
        match self.until {
            None => Some(self.line.as_str()),
            Some(until) => (now < until).then_some(self.line.as_str()),
        }
    }

    /// How much this message matters — what a surface marks it with.
    ///
    /// READ BY BOTH FRONTS, which is the point: `sprag-gui` picks the strip's container role from it
    /// and `sprag-tui` prefixes [`mark`](Self::mark) onto its row. The audit that asked *which new
    /// method has no caller* found this one with none, next to a doc claiming a surface marked
    /// something — a claim with nothing behind it, which is the shape this project spends rounds
    /// removing.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// The word a surface puts in front of this message, or [`None`] for one that needs none.
    ///
    /// **Only an [`Alert`](Severity::Alert) is marked**, and the reason is what marking is FOR: a
    /// note and a warning explain themselves and go away, so a word in front of them is noise over a
    /// sentence the user can already read. An alert is the one that STAYS, and a person who did not
    /// see it arrive needs to know why their row is not clearing. Derived here so both fronts mark
    /// it identically and neither writes a word of its own.
    #[must_use]
    pub const fn mark(&self) -> Option<&'static str> {
        match self.severity {
            Severity::Alert => Some(Severity::Alert.word()),
            Severity::Note | Severity::Warn => None,
        }
    }

    /// When this message stops being shown — what a client blocking on input must wake at, so the
    /// row clears on time rather than at the next keystroke. [`None`] for a message that waits to
    /// be acknowledged, which is a client that must go on blocking rather than one that must wake.
    #[must_use]
    pub const fn until(&self) -> Option<Moment> {
        self.until
    }

    /// Whether this message stays until a person touches a key rather than until a clock says so.
    ///
    /// Read by a client's key path, which is the ONLY thing that may clear one — see [`Severity`]
    /// for why an alert does not take the timer's bet.
    #[must_use]
    pub const fn waits_to_be_acknowledged(&self) -> bool {
        self.until.is_none()
    }

    /// Which message a client shows once `self` arrives while `showing` is up.
    ///
    /// **A lower severity never takes the row from a live higher one.** That is the whole of what
    /// [`Severity`]'s order decides, and putting it here rather than at each surface is what makes
    /// it hold for a message that arrives from the daemon, a message a keystroke produced, and any
    /// pair of the two — three cases, one rule, no call site free to get it wrong.
    ///
    /// An EXPIRED message outranks nothing: `showing` is consulted through
    /// [`showing`](Self::showing), so a `Warn` whose deadline has passed does not go on suppressing
    /// notes from beyond its own lifetime.
    ///
    /// Equal severities resolve to the ARRIVING one, which is what makes a second refusal replace
    /// the first rather than being swallowed by it — the rival answers `Busy` and shows the second
    /// message to nobody (`handle_notification_show`, read at `9a4ce5e1`).
    #[must_use]
    pub fn over(self, showing: Option<Self>, now: Moment) -> Self {
        match showing {
            Some(current)
                if current.showing(now).is_some() && current.severity.outranks(self.severity) =>
            {
                current
            }
            _ => self,
        }
    }
}

impl Announcement {
    /// Which message waits for a client that has not collected either.
    ///
    /// [`Message::over`]'s rule, asked one step earlier — and with one fewer question, because an
    /// undelivered message has not started its lifetime, so there is no expiry to consult. The
    /// comparison itself is [`Severity::outranks`], shared with the row, so the daemon's slot and
    /// the client's row cannot come to rank two messages differently.
    ///
    /// **This is why the daemon holds ONE message per client rather than a queue.** A queue would
    /// need a bound, and a bound needs a rule for what to throw away — a third rule, which would be
    /// the silent drop this round exists to remove. One slot resolved by the rule already written
    /// down has no capacity to exceed and nothing to discard unaccountably: a note that loses to a
    /// live alert lost by the rule a caller can read.
    #[must_use]
    pub fn over(self, waiting: Option<Self>) -> Self {
        match waiting {
            Some(current) if current.severity.outranks(self.severity) => current,
            _ => self,
        }
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
    use crate::keymap::SelectWindowBind;
    use crate::keymap::SwitchClientAsk;
    use sprag_terminal::OrderStep;

    /// A session destroyed under this client names BOTH ends: the one that went, and the one it
    /// was moved to. The first half is the one no re-read can recover — a destroyed session is
    /// gone from every list the daemon serves — so a sentence that named only the landing would be
    /// telling a person something their status row already says.
    #[test]
    fn a_destroyed_session_is_named_along_with_where_the_client_went() {
        let moved = crate::wake::Lost::Moved {
            was: "work".to_owned(),
            now: "beta".to_owned(),
        };
        assert_eq!(
            Report::lost_session(&moved).says(),
            Some("session \"work\" was destroyed; now on \"beta\""),
        );
        assert_eq!(
            Report::lost_session(&moved).severity(),
            Some(Severity::Warn)
        );
    }

    /// A client that is LEAVING says nothing, and that is a claim rather than an omission: there is
    /// no screen left to say it on, so a row painted here would be a flash on a surface being torn
    /// down in the same frame.
    #[test]
    fn a_client_that_is_leaving_says_nothing_about_it() {
        let detached = crate::wake::Lost::Detached {
            was: "work".to_owned(),
        };
        assert_eq!(Report::lost_session(&detached).says(), None);
    }

    /// The two sentences a destroy can produce are DIFFERENT sentences, and the difference is who
    /// did it. A gesture that cascaded says what the person's own key reached; a destroy from
    /// elsewhere says what happened to them. Held together here so neither can drift into the
    /// other's wording — a passive *"was destroyed"* answering a key somebody just pressed is the
    /// 150 ms flash R326 measured and removed.
    #[test]
    fn a_gesture_that_cascaded_and_a_destroy_from_elsewhere_are_not_one_sentence() {
        let cascaded = Report::cascaded(Ended::Session, Ended::Window);
        let elsewhere = Report::lost_session(&crate::wake::Lost::Moved {
            was: "work".to_owned(),
            now: "beta".to_owned(),
        });
        assert_eq!(cascaded.says(), Some("the session went with it"));
        assert!(
            !cascaded
                .says()
                .is_some_and(|line| line.contains("destroyed")),
            "a person's own kill is not reported to them in the passive",
        );
        assert_ne!(cascaded.says(), elsewhere.says());
    }

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
            ask: SelectWindowBind::Named("logs".into()),
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
                ask: SelectWindowBind::Named("logs".into()),
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
        assert_eq!(message.until(), Some(now + Duration::from_millis(750)));
    }

    /// The order is the DECLARATION order, and it is what [`Message::over`] resolves by — so a
    /// reordering of the variant list is caught here rather than by a message quietly losing a
    /// precedence fight in production.
    #[test]
    fn the_severities_run_from_a_note_up_to_an_alert() {
        assert_eq!(
            Severity::ALL,
            [Severity::Note, Severity::Warn, Severity::Alert],
        );
        assert!(Severity::Note < Severity::Warn);
        assert!(Severity::Warn < Severity::Alert);
        assert_eq!(Severity::default(), Severity::Note);
        assert!(Severity::Alert.outranks(Severity::Note));
        assert!(
            !Severity::Warn.outranks(Severity::Warn),
            "EQUAL does not outrank: a second refusal replaces the first",
        );
    }

    /// The word a caller types, the word on the wire and the word in front of the sentence are ONE
    /// function, checked over the whole type rather than over the three that happened to be
    /// written down.
    #[test]
    fn every_severity_round_trips_through_its_own_word() {
        for severity in Severity::ALL {
            assert_eq!(Severity::parse(severity.word()), Some(severity));
            assert_eq!(severity.to_string(), severity.word());
            let json = serde_json::to_string(&severity).expect("a severity serialises");
            assert_eq!(json, format!("\"{}\"", severity.word()));
            assert_eq!(
                serde_json::from_str::<Severity>(&json).expect("and reads back"),
                severity,
            );
            // ...AND from an owned `Value`, which is a DIFFERENT deserializer and the one the wire
            // client actually uses. A `&str` impl passes the line above and fails this one — it
            // cannot borrow out of a `Value` — and the symptom was a message the daemon reported
            // delivering that no client ever painted. Pinned so the cheaper spelling cannot come
            // back.
            let value = serde_json::to_value(severity).expect("a severity is a value");
            assert_eq!(
                serde_json::from_value::<Severity>(value).expect("and reads back from one"),
                severity,
            );
        }
        assert_eq!(Severity::parse("shout"), None);
        assert_eq!(Severity::words(), "note|warn|alert");
    }

    /// A word this type does not know is a REFUSAL that lists what it would have taken — the wire's
    /// half of the same sentence the CLI prints.
    #[test]
    fn an_unknown_severity_on_the_wire_names_the_ones_that_exist() {
        let refusal = serde_json::from_str::<Severity>("\"shout\"")
            .expect_err("an unknown severity is refused");
        let said = refusal.to_string();
        assert!(said.contains("shout"), "it names what was offered: {said}");
        assert!(
            said.contains("note|warn|alert"),
            "and what it would have taken: {said}",
        );
    }

    /// **An alert has NO deadline**, which is the whole reason there are three severities rather
    /// than one line of colour — and the two below it keep `display-time` exactly as R316 shipped
    /// it.
    #[test]
    fn only_an_alert_waits_for_a_person_rather_than_a_clock() {
        let now = now();
        let display_time = Duration::from_millis(750);
        assert_eq!(
            Severity::Note.deadline(now, display_time),
            Some(now + display_time),
        );
        assert_eq!(
            Severity::Warn.deadline(now, display_time),
            Some(now + display_time),
        );
        assert_eq!(Severity::Alert.deadline(now, display_time), None);
    }

    /// **`display-time 0` reaches EVERY severity**, including the one that otherwise never expires
    /// — or the option would be honoured for two states of three and an alert would sit on the row
    /// of a user who asked for silence, forever.
    #[test]
    fn a_zero_display_time_silences_the_alert_too() {
        let now = now();
        for severity in Severity::ALL {
            assert_eq!(
                severity.deadline(now, Duration::ZERO),
                Some(now),
                "{severity} must expire immediately when the user asked for no messages",
            );
        }
        let alert = Message::of(&announced(Severity::Alert), now, Duration::ZERO)
            .expect("a message is still built");
        assert_eq!(alert.showing(now), None);
        assert!(
            !alert.waits_to_be_acknowledged(),
            "silence is an expiry, not a message waiting for a keystroke that would never clear it",
        );
    }

    /// **A note does not take the row from a live warning, and a warning does take it from a live
    /// note** — the one rule that makes precedence a property rather than a discipline.
    #[test]
    fn a_lower_severity_never_takes_the_row_from_a_live_higher_one() {
        let now = now();
        let display_time = Duration::from_millis(750);
        let warn = Message::of(&refusal(), now, display_time).expect("a refusal speaks");
        let note =
            Message::of(&announced(Severity::Note), now, display_time).expect("so does a note");

        assert_eq!(
            note.clone().over(Some(warn.clone()), now).showing(now),
            warn.showing(now),
            "the note arrived while the warning was live, so the warning keeps the row",
        );
        assert_eq!(
            warn.clone().over(Some(note.clone()), now).showing(now),
            warn.showing(now),
            "and the warning takes it from the note",
        );
    }

    /// An EXPIRED message outranks nothing — otherwise a warning would go on suppressing notes from
    /// beyond its own lifetime, which is a row that has stopped being shown still deciding what is.
    #[test]
    fn a_message_that_has_expired_wins_no_precedence_fight() {
        let now = now();
        let display_time = Duration::from_millis(750);
        let warn = Message::of(&refusal(), now, display_time).expect("a refusal speaks");
        let note =
            Message::of(&announced(Severity::Note), now, display_time).expect("so does a note");
        let after = now + display_time;

        assert_eq!(
            note.clone().over(Some(warn), after).showing(after),
            note.showing(after),
            "the warning's deadline had passed, so the note takes the row",
        );
    }

    /// Two of the same severity resolve to the ARRIVING one, so a second refusal replaces the first
    /// rather than being swallowed — which is what the rival answers `Busy` to and shows nobody.
    #[test]
    fn an_equal_severity_replaces_what_is_showing() {
        let now = now();
        let display_time = Duration::from_millis(750);
        let first = Message::of(&refusal(), now, display_time).expect("a refusal speaks");
        let second =
            Message::of(&announced(Severity::Warn), now, display_time).expect("and another");

        assert_eq!(
            second.clone().over(Some(first), now).showing(now),
            second.showing(now),
        );
    }

    /// A message arriving onto an EMPTY row is shown, which is the case a rule about two messages
    /// must not accidentally exclude.
    #[test]
    fn a_message_arriving_onto_an_empty_row_is_the_one_shown() {
        let now = now();
        let note = Message::of(&announced(Severity::Note), now, Duration::from_millis(750))
            .expect("a note speaks");
        assert_eq!(note.clone().over(None, now).showing(now), note.showing(now));
    }

    /// The daemon's slot ranks two UNDELIVERED messages by the same rule the row does, so a client
    /// is never handed the message it would then have refused to show.
    #[test]
    fn the_waiting_slot_ranks_two_messages_the_way_the_row_would() {
        let alert = say("your turn", Severity::Alert);
        let note = say("a note", Severity::Note);
        assert_eq!(note.clone().over(Some(alert.clone())), alert);
        assert_eq!(alert.clone().over(Some(note.clone())), alert);
        assert_eq!(note.clone().over(None), note);
        assert_eq!(
            note.clone().over(Some(say("older", Severity::Note))),
            note,
            "equal severities resolve to the arriving one, exactly as the row does",
        );
    }

    /// What a person told this client and what its own keyboard told it are ONE type by the time
    /// either reaches a surface — so a client has no way to paint them differently.
    #[test]
    fn a_routed_message_and_a_key_report_are_the_same_value() {
        let report = Report::said(&say("deploy finished", Severity::Alert));
        assert_eq!(report.says(), Some("deploy finished"));
        assert_eq!(report.severity(), Some(Severity::Alert));
        assert_eq!(Report::no_such(&ghost()).severity(), Some(Severity::Warn));
        assert_eq!(Report::on_screen().severity(), None);
    }

    /// A plain sentence is kept exactly as it was given — no truncation, no stripping, nothing the
    /// caller is not told about.
    #[test]
    fn a_plain_sentence_survives_validation_unchanged() {
        let text = MessageText::parse("build finished: 0 errors, 3 warnings")
            .expect("a plain sentence is a message");
        assert_eq!(text.as_str(), "build finished: 0 errors, 3 warnings");
        assert_eq!(text.to_string(), "build finished: 0 errors, 3 warnings");
    }

    /// **A control character is REFUSED, and this is the rule with teeth**: the words are written
    /// into somebody's terminal, so a newline would forge a second row and an escape would be
    /// obeyed. One fixture per character class a surface would otherwise have had to defend against
    /// on its own.
    #[test]
    fn a_message_carrying_a_control_character_is_refused_by_name() {
        for hostile in ["two\nrows", "clear: \u{1b}[2J", "a\rb", "a\u{7}b"] {
            assert_eq!(
                MessageText::parse(hostile),
                Err(MessageTextError::Control),
                "{hostile:?} must not reach a terminal row",
            );
        }
        let said = MessageTextError::Control.to_string();
        assert!(
            said.contains("newline") && said.contains("escape"),
            "the refusal says WHY, not just no: {said}",
        );
    }

    /// **The two audiences get two lengths, and the SHORT one is bounded** — which is what lets
    /// [`crate::attention`]'s fallback sentence prove it fits a row instead of hoping.
    ///
    /// The bound is asserted rather than trusted, and the reason is that it was WRONG: the first
    /// version of that fallback embedded the `Display` paragraph, which pushed a refusal sentence to
    /// 216 bytes — over the very limit it was reporting — and the `expect` beside it claimed the
    /// case was unreachable. A test found it. Every rule is checked here, not the one that broke.
    #[test]
    fn the_rule_names_stay_inside_their_own_bound() {
        for broken in [
            MessageTextError::Blank,
            MessageTextError::TooLong(MessageText::MAX_BYTES + 1),
            MessageTextError::Control,
        ] {
            assert!(
                broken.rule().len() <= MessageTextError::LONGEST_RULE,
                "{broken:?}'s rule name is {} bytes, past the declared bound",
                broken.rule().len(),
            );
            assert!(
                broken.to_string().len() > broken.rule().len(),
                "the two audiences must not have collapsed into one wording: {broken:?}",
            );
        }
    }

    /// A blank message is refused rather than shown as an empty row, and whitespace-only counts as
    /// blank — a caller that sent `"   "` meaning "clear" and one that sent it by accident cannot be
    /// told apart.
    #[test]
    fn a_blank_message_is_refused() {
        for blank in ["", " ", "\t \t"] {
            assert_eq!(MessageText::parse(blank), Err(MessageTextError::Blank));
        }
    }

    /// The length bound is on BYTES and the refusal carries the length offered, so a caller learns
    /// by how much rather than merely that it was too long.
    #[test]
    fn a_message_longer_than_a_row_is_refused_with_its_length() {
        let just_fits = "x".repeat(MessageText::MAX_BYTES);
        assert!(MessageText::parse(&just_fits).is_ok());
        let one_too_many = "x".repeat(MessageText::MAX_BYTES + 1);
        assert_eq!(
            MessageText::parse(&one_too_many),
            Err(MessageTextError::TooLong(MessageText::MAX_BYTES + 1)),
        );
        let said = MessageTextError::TooLong(MessageText::MAX_BYTES + 1).to_string();
        assert!(
            said.contains(&MessageText::MAX_BYTES.to_string()),
            "the refusal names the bound: {said}",
        );
    }

    /// A refusal, for the precedence fixtures above.
    fn refusal() -> Report {
        Report::no_such(&ghost())
    }

    /// The action every refusal fixture in this module is built from.
    fn ghost() -> BoundAction {
        BoundAction::SwitchClient {
            ask: SwitchClientAsk::Named("ghost".into()),
        }
    }

    /// An announcement at `severity`, for the fixtures above.
    fn say(text: &str, severity: Severity) -> Announcement {
        Announcement {
            text: MessageText::parse(text).expect("a plain sentence"),
            severity,
        }
    }

    /// A routed message at `severity`, as a [`Report`].
    fn announced(severity: Severity) -> Report {
        Report::said(&say("the deploy finished", severity))
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
