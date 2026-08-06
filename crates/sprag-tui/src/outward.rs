//! A message FOLLOWING the person out of the room — the copy that reaches them once the row they
//! would have read is in a window they are not looking at.
//!
//! # The defect this module removes
//!
//! R317 gave a message an address and R318 gave a pane's own child a voice, and both end at the same
//! place: a sentence painted on this client's bottom row. That row is the whole delivery, and it
//! rests on an assumption nothing had ever checked — that somebody is looking at this terminal. A
//! person who starts a build and switches to their browser has the words delivered to a window they
//! cannot see, which is R318's *"every layer was carrying it and nothing was obliged to read it"*
//! one layer further out. The pane told the daemon, the daemon told the client, the client painted
//! it, and the person still does not know their build finished.
//!
//! # What replaces it
//!
//! The client asks its terminal to say when it loses focus ([`crate::focus`]), and a message that
//! arrives while the person is away is ALSO written to that terminal as a desktop notification —
//! `OSC 9`, or kitty's `OSC 99` which carries an URGENCY. The terminal emulator then does what it
//! does with a notification: a toast, a dock badge, a taskbar flash. Nothing about the row changes;
//! this is a copy, sent only when the primary delivery cannot land.
//!
//! # Why forwarding is CONDITIONAL, and why that is the whole design
//!
//! Because an unconditional forward is a worse product than no forward at all. A person watching
//! their panes would get every message twice — once on the row and once as a toast over the window
//! they are already reading it in — and the setting they would reach for is `off`, which is the
//! silence this whole front exists to remove. So the default policy is
//! [`Forward::Unfocused`]: exactly the messages a person could not have seen, and nothing else.
//!
//! # The rival
//!
//! herdr (`9a4ce5e1`) is AHEAD here and this module exists because of them: `terminal_notify`
//! emits `OSC 9` / `OSC 99` by detected terminal with tmux passthrough, `platform/{linux,macos,
//! windows}` does the OS-native one, and their agent-state path even suppresses a notification for a
//! pane the person can see (`active_tab_suppresses_notifications`, which reads their client's own
//! `outer_terminal_focus`). sprag had none of it. Where this goes past them, each checked against
//! their source rather than assumed:
//!
//! * **The URGENCY survives.** `build_osc99_notification` hardcodes `i=1:d=0` and never emits a
//!   `u=` key, so a child that said *a person is needed* is forwarded as an ordinary notification.
//!   Here the chain is faithful end to end: kitty `u=2` in a pane → [`Severity::Alert`] on the row →
//!   `u=2` out to the person's terminal. Their API caller cannot express urgency at all.
//! * **The person's focus is read PER CLIENT.** `foreground_client_outer_focus` reads one client —
//!   whichever their server last promoted — so with two terminals attached, one person's window
//!   decides for both. Each sprag client owns its own [`Person`] and its own policy, which is what
//!   R317's per-client mailbox already made true of the row.
//! * **Their forward is unconditional on the path an API reaches**, where their agent path is not:
//!   `handle_notification_show_api` consults a rate limit and never asks about focus. So the one
//!   surface a script drives is the one that toasts a person who is looking straight at it.
//! * **A refusal is a sentence, not a filter.** `sanitize_text` strips escapes and truncates to 80
//!   bytes and answers `shown`; a [`MessageText`] that broke a rule was already turned into a
//!   sentence NAMING the rule before it got here, so what is forwarded is what the person reads.
//!
//! And one honest trade the other way: they also do the OS-NATIVE notification, which reaches a
//! person whose terminal understands no OSC at all. sprag does not, deliberately — a `notify-send`
//! runs on the machine the CLIENT is on, and the client this module lives in is the one people run
//! over ssh, where that machine is a server with nobody in front of it. An `OSC` reaches the terminal
//! the person is actually sitting at, through the same ssh pipe the panes come down.

use std::fmt::Write as _;
use std::io::Write;

use sprag_host::options::{NOTIFY_OUTWARD, Options};
use sprag_host::report::{Announcement, MessageText, Severity};
use sprag_vt::Urgency;
use termwiz::escape::csi::{CSI, DecPrivateMode, DecPrivateModeCode, Mode};

use crate::focus::Person;

sprag_vt::closed_set! {
    /// WHEN a message is copied out to the terminal this client is running in — the
    /// `notify-outward` option's three values.
    ///
    /// Three and not a switch, because the middle one is the point and the outer two are the
    /// answers a person gives when it is wrong for them. `off` is the silence sprag had before this
    /// existed; `always` is for a terminal that reports no focus (mode 1004 is not universal) or a
    /// person who wants the notification regardless.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub enum Forward {
        /// Never copy a message out. The row is the only delivery.
        Off,
        /// Copy it out only while the person is [`Person::Away`] — the default, for the reason the
        /// module docs give: a message a person can already read is not news.
        #[default]
        Unfocused,
        /// Copy every message out, whether or not the person is looking.
        Always,
    }
}

impl Forward {
    /// The word this policy is spelled with in `config.toml` and in `show-options`.
    ///
    /// ONE spelling, exactly as [`Severity::word`] is: the option's vocabulary
    /// ([`sprag_host::options::NOTIFY_OUTWARD_VALUES`]) is checked against these by a test in this
    /// crate, because the table lives in a crate that cannot depend on this one — the arrangement
    /// `detach-on-destroy` documents.
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

    /// Whether this policy needs the terminal to REPORT focus at all.
    ///
    /// The gate on DEC private mode 1004 and, with it, on [`crate::focus`]'s read-ahead: a person
    /// who set `off` or `always` has asked a question whose answer does not depend on where they
    /// are, so their terminal is never asked and their own `Alt-[` is never read ahead of. The cost
    /// of the decoder is paid only by the policy that needs it.
    #[must_use]
    pub const fn needs_focus(self) -> bool {
        matches!(self, Self::Unfocused)
    }
}

/// How the terminal this client is running in takes a desktop notification.
///
/// # Why `OSC 9` is sent to EVERY terminal rather than to a detected list
///
/// Because this client already does exactly that with a sequence of the same class, and has since
/// before this module existed: it sets the window TITLE (`OSC 0` / `OSC 2`) on whatever terminal it
/// was started in, with no detection at all. A terminal that turned an unrecognised OSC into visible
/// garbage would therefore already be broken by sprag's title — so the safety of `OSC 9` here is a
/// measured property of shipped behaviour and not a hope about strangers' parsers.
///
/// That is the difference from the rival's `detect_backend`, which answers `None` for everything but
/// ghostty, iTerm2, kitty and WezTerm and returns `Ok(false)` — a person on gnome-terminal, konsole,
/// alacritty, foot or xterm gets silence with no way to ask for anything else. Detection is used
/// HERE only to UPGRADE: kitty's own protocol carries the urgency, and nothing else does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Form {
    /// `OSC 9 ; <text>` — the widely understood single-string notification.
    #[default]
    Osc9,
    /// kitty's `OSC 99` desktop-notification protocol, which carries `u=<urgency>`.
    Osc99,
}

/// This client's outward notification: the policy, who to say it is from, and what its terminal
/// understands.
///
/// Built once at start-up because every input is fixed for the client's life: the option comes from
/// the file the client already read, and the terminal it is talking to cannot change underneath it.
#[derive(Clone, Debug)]
pub struct Outward {
    /// When to forward — see [`Forward`].
    policy: Forward,
    /// The session this client is showing, so a person with four of them is told WHICH one wants
    /// them. Spelled `[name]` in the sentence, which is the status row's own prefix rather than a
    /// second wording invented for the notification.
    session: String,
    /// What the host terminal understands.
    form: Form,
    /// Whether this client is running inside tmux, so the sequence has to be passed THROUGH it.
    ///
    /// tmux consumes an OSC it does not model, so a notification written straight out would reach
    /// tmux and stop. The `DCS tmux;` wrapper is how tmux itself documents forwarding one — subject
    /// to its `allow-passthrough`, which is off by default in current tmux: with it off the wrapper
    /// is discarded whole, which is the same silence as not sending it and never garbage on a
    /// person's screen.
    tmux: bool,
}

impl Outward {
    /// Read the policy from `options` and the terminal from `env` — a lookup rather than
    /// [`std::env::var`] so both halves are testable, which the rival's `detect_backend` is not.
    ///
    /// `session` is the session this client is attached to.
    #[must_use]
    pub fn of(options: &Options, session: String, env: impl Fn(&str) -> Option<String>) -> Self {
        // kitty announces itself two ways and both are its own: the window id it exports into every
        // child, and its terminfo name. Either is enough, and asking for both is what makes this
        // work inside a shell that scrubbed one of them.
        let kitty = env("KITTY_WINDOW_ID").is_some()
            || env("TERM").is_some_and(|term| term == "xterm-kitty");
        Self {
            policy: Forward::of(options),
            session,
            form: if kitty { Form::Osc99 } else { Form::Osc9 },
            tmux: env("TMUX").is_some(),
        }
    }

    /// Whether this client should ask its terminal to report focus — [`Forward::needs_focus`],
    /// re-exposed because the loop holds the policy through this type.
    #[must_use]
    pub const fn needs_focus(&self) -> bool {
        self.policy.needs_focus()
    }

    /// Ask the terminal to report focus changes (DEC private mode 1004), or stop asking.
    ///
    /// Named through termwiz's own escape vocabulary rather than spelled as `\x1b[?1004h`, which is
    /// the rule the mouse mirror already follows for the modes it owns: the number appears nowhere
    /// in sprag's source.
    ///
    /// **Turning it off on the way out is not tidiness.** termwiz restores what IT set and it did
    /// not set this, so a client that exited with 1004 still on would leave the user's shell being
    /// told about every window switch — which a shell renders as `^[[I` at the prompt.
    pub fn watch_focus(watch: bool, out: &mut impl Write) {
        let mode = DecPrivateMode::Code(DecPrivateModeCode::FocusTracking);
        let mut sequence = String::new();
        let _ = write!(
            sequence,
            "{}",
            if watch {
                CSI::Mode(Mode::SetDecPrivateMode(mode))
            } else {
                CSI::Mode(Mode::ResetDecPrivateMode(mode))
            }
        );
        let _ = out.write_all(sequence.as_bytes());
        let _ = out.flush();
    }

    /// Whether a message must be copied out, given where the person is.
    ///
    /// `person` is [`None`] when this client never asked its terminal to report focus, which is
    /// **not** the same as the person being here: it is the state of a client whose policy does not
    /// need the answer. [`Forward::Unfocused`] therefore answers `false` for it — unreachable by
    /// construction (that policy is exactly the one that asks), and answered rather than panicked
    /// because a display client must not die over its own bookkeeping.
    #[must_use]
    pub fn follows(&self, person: Option<Person>) -> bool {
        match self.policy {
            Forward::Off => false,
            Forward::Always => true,
            Forward::Unfocused => person == Some(Person::Away),
        }
    }

    /// The sentence a person reads in the notification: the session, spelled as the status row
    /// spells it, and the words the row would have shown.
    ///
    /// # The prefix is DROPPED rather than sanitised when it will not fit a message
    ///
    /// A session name is the user's own string, and it is on its way into an OSC written to somebody
    /// else's terminal — the same subject [`MessageText`] exists for. So the composed sentence is
    /// re-validated, and a name that breaks a rule (a control character, or a length that pushes the
    /// whole line past the cap) falls back to the announcement's own text, which is a value already
    /// known to be safe. There is no `expect` here to be wrong about: the fallback is the input.
    fn sentence(&self, announcement: &Announcement) -> MessageText {
        MessageText::parse(&format!(
            "[{}] {}",
            self.session,
            announcement.text.as_str()
        ))
        .unwrap_or_else(|_| announcement.text.clone())
    }

    /// Copy `announcement` out to the host terminal if the policy and `person` call for it.
    ///
    /// A write that fails is dropped, for [`crate::focus`]'s reason and the mouse mirror's: the only
    /// place this client could report it is the screen it is painting a person's panes onto, and a
    /// terminal that will not take a notification will not take a diagnostic either.
    pub fn forward(
        &self,
        person: Option<Person>,
        announcement: &Announcement,
        out: &mut impl Write,
    ) {
        if !self.follows(person) {
            return;
        }
        let sequence = self.sequence(announcement);
        let _ = out.write_all(&sequence);
        let _ = out.flush();
    }

    /// The bytes that ask the host terminal to show `announcement`.
    ///
    /// Separate from [`forward`](Self::forward) so the whole encoding — the form, the urgency, the
    /// sentence and the tmux wrapper — is testable without a terminal.
    fn sequence(&self, announcement: &Announcement) -> Vec<u8> {
        let said = self.sentence(announcement);
        // ST (`ESC \`) and not BEL, in both forms: it is the terminator both protocols document, and
        // the one a `DCS tmux;` wrapper can carry — a BEL inside the wrapper would end nothing and
        // reach the outer terminal as a bell.
        let raw = match self.form {
            Form::Osc9 => format!("\u{1b}]9;{}\u{1b}\\", said.as_str()),
            // ONE chunk, with no `i=` identifier and therefore no `d=` continuation: a message is
            // one line by construction ([`MessageText`] caps it at a row), so splitting it into a
            // title and a body would be inventing a division the sentence does not have — and the
            // chunked form needs a notification IDENTITY whose reuse rule this client cannot check
            // against a real kitty. The urgency, which is the reason this form exists at all, needs
            // neither.
            Form::Osc99 => format!(
                "\u{1b}]99;u={};{}\u{1b}\\",
                // The DIGIT comes from the emulator's own spelling of the urgency it parses, so the
                // scale this client writes out and the scale a pane's child writes in are one table.
                String::from_utf8_lossy(urgency_of(announcement.severity).digit()),
                said.as_str(),
            ),
        };
        if self.tmux {
            through_tmux(&raw)
        } else {
            raw.into_bytes()
        }
    }
}

/// kitty's `u=` digit for a message of this severity — the OUTWARD half of R318's projection.
///
/// # The two directions are not inverses, and that is deliberate
///
/// [`sprag_host::attention`] maps a child's [`Urgency`] onto [`Severity`] on the way IN, collapsing
/// `Low` and `Normal` onto [`Severity::Note`]. This maps back, and `Note` becomes
/// [`Urgency::Normal`] rather than `Low` — so a child that said `u=0` is forwarded as `u=1`.
///
/// That is not information lost by accident. `u=0` means *background information; miss it and
/// nothing is lost*, and by the time a message is being copied out to a person who has left the
/// room, the product has already decided it is worth interrupting them for — the row showed it, the
/// policy said follow them. Forwarding that as *ignorable* would be the sequence contradicting the
/// decision that produced it. So `Low` is a value this client can RECEIVE and never SENDS, which is
/// the honest shape of a projection between two scales that mean different things.
///
/// [`Severity::Alert`] is the one that matters and it is exact: an alert holds the row until a person
/// touches a key, and `u=2` is kitty's *the child says a person is needed*. Those are the same claim.
fn urgency_of(severity: Severity) -> Urgency {
    match severity {
        // A warning is *something did not work*, which is not *a person is needed* — `Alert` is the
        // arm that claims that, and it is the only one that gets kitty's critical.
        Severity::Note | Severity::Warn => Urgency::Normal,
        Severity::Alert => Urgency::Critical,
    }
}

/// `raw` wrapped so tmux passes it through to the terminal tmux is itself running in.
///
/// Every `ESC` inside the payload is DOUBLED, which is tmux's own documented escaping for its
/// passthrough DCS: a single `ESC` would end the wrapper at the payload's own terminator and leave
/// its tail to be printed.
fn through_tmux(raw: &str) -> Vec<u8> {
    let mut wrapped = Vec::with_capacity(raw.len() + 16);
    wrapped.extend_from_slice(b"\x1bPtmux;");
    for byte in raw.bytes() {
        if byte == 0x1b {
            wrapped.push(0x1b);
        }
        wrapped.push(byte);
    }
    wrapped.extend_from_slice(b"\x1b\\");
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An [`Outward`] built from an env table, so a test names what the terminal claims to be
    /// instead of reaching into the process's own environment.
    fn outward(policy: Forward, env: &[(&str, &str)]) -> Outward {
        let mut options = Options::default();
        options
            .set(NOTIFY_OUTWARD, policy.word())
            .expect("a policy is a value the option takes");
        let owned: Vec<(String, String)> = env
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Outward::of(&options, "work".to_owned(), move |want| {
            owned
                .iter()
                .find(|(key, _)| key == want)
                .map(|(_, value)| value.clone())
        })
    }

    fn said(text: &str, severity: Severity) -> Announcement {
        Announcement {
            text: MessageText::parse(text).expect("a legal message"),
            severity,
        }
    }

    /// The whole 3x3 of policy against where the person is — because the middle row is the feature
    /// and the outer two are what a person sets when it is wrong for them, and a table is the only
    /// way to say that no cell was decided by accident.
    #[test]
    fn a_message_follows_the_person_exactly_when_the_policy_says_so() {
        for policy in Forward::ALL {
            for person in [None, Some(Person::Here), Some(Person::Away)] {
                let want = match (policy, person) {
                    (Forward::Off, _) => false,
                    (Forward::Always, _) => true,
                    (Forward::Unfocused, seen) => seen == Some(Person::Away),
                };
                assert_eq!(
                    outward(policy, &[]).follows(person),
                    want,
                    "{policy:?} with the person {person:?}",
                );
            }
        }
    }

    /// Only the policy that reads the answer asks the question — so a person on `off` or `always`
    /// pays neither the mode nor the read-ahead that mode makes necessary.
    #[test]
    fn only_the_focus_policy_asks_the_terminal_about_focus() {
        assert!(outward(Forward::Unfocused, &[]).needs_focus());
        assert!(!outward(Forward::Off, &[]).needs_focus());
        assert!(!outward(Forward::Always, &[]).needs_focus());
    }

    /// The default policy is the one the module argues for, read through the OPTION TABLE rather
    /// than from this enum's own `Default` — so the file's default and the type's cannot drift.
    #[test]
    fn the_option_in_force_with_the_user_silent_is_the_focus_policy() {
        assert_eq!(Forward::of(&Options::default()), Forward::Unfocused);
        assert_eq!(Forward::default(), Forward::Unfocused);
    }

    /// **The option's vocabulary IS this enum's**, both ways: every word the table offers parses to
    /// a distinct policy, and every policy is offered. The arrangement `detach-on-destroy`
    /// documents — the table lives in a crate that cannot depend on this one, so nothing but a test
    /// holds them together.
    #[test]
    fn the_options_vocabulary_is_exactly_the_policy_set() {
        let offered = sprag_host::options::NOTIFY_OUTWARD_VALUES;
        let from_policy: Vec<&str> = Forward::ALL.iter().map(|policy| policy.word()).collect();
        assert_eq!(
            offered, &from_policy,
            "the option must offer exactly the policies, in the same order",
        );
        for word in offered {
            assert!(
                Forward::parse(word).is_some(),
                "{word:?} is offered by the option and parses to no policy",
            );
        }
        // ...and the canonicalisation a user's file goes through preserves them, which is the round
        // trip the value actually takes.
        let mut options = Options::default();
        for policy in Forward::ALL {
            options
                .set(NOTIFY_OUTWARD, policy.word())
                .expect("an offered value is acceptable");
            assert_eq!(Forward::of(&options), policy);
        }
    }

    /// An ordinary terminal gets `OSC 9`, and the sentence names the SESSION the way the status row
    /// names it — so a person with four sessions is told which one wants them.
    #[test]
    fn an_ordinary_terminal_is_sent_an_osc_9_naming_the_session() {
        let sequence =
            outward(Forward::Always, &[]).sequence(&said("pane 3: done", Severity::Note));
        assert_eq!(
            String::from_utf8(sequence).expect("utf8"),
            "\u{1b}]9;[work] pane 3: done\u{1b}\\",
        );
    }

    /// **kitty gets the URGENCY, which is the property no rival has** — and `alert` is the arm that
    /// carries it, because an alert is the row that waits for a person.
    ///
    /// Asserted over the whole severity set rather than on the one interesting case, so a fourth
    /// severity could not be added without deciding what a person's terminal should be told.
    #[test]
    fn kitty_is_sent_the_severity_as_an_urgency() {
        let kitty = outward(Forward::Always, &[("TERM", "xterm-kitty")]);
        for severity in Severity::ALL {
            let want = match severity {
                Severity::Alert => 2,
                Severity::Note | Severity::Warn => 1,
            };
            let sequence =
                String::from_utf8(kitty.sequence(&said("pane 1: build", severity))).expect("utf8");
            assert_eq!(
                sequence,
                format!("\u{1b}]99;u={want};[work] pane 1: build\u{1b}\\"),
                "{severity:?} must reach the person's desktop as u={want}",
            );
        }
    }

    /// kitty announces itself two ways, and either is enough — a shell that scrubbed the window id
    /// still leaves the terminfo name, and a `TERM` rewritten by a nested program still leaves the
    /// window id.
    #[test]
    fn either_of_kittys_own_announcements_selects_its_protocol() {
        for env in [
            vec![("KITTY_WINDOW_ID", "1")],
            vec![("TERM", "xterm-kitty")],
            vec![("KITTY_WINDOW_ID", "3"), ("TERM", "xterm-256color")],
        ] {
            let sequence = String::from_utf8(
                outward(Forward::Always, &env).sequence(&said("x", Severity::Note)),
            )
            .expect("utf8");
            assert!(
                sequence.starts_with("\u{1b}]99;"),
                "{env:?} must select kitty's own protocol: {sequence:?}",
            );
        }
        // The control: a terminal that claims neither gets the widely-understood form.
        let plain = String::from_utf8(
            outward(Forward::Always, &[("TERM", "xterm-256color")])
                .sequence(&said("x", Severity::Note)),
        )
        .expect("utf8");
        assert!(plain.starts_with("\u{1b}]9;"), "{plain:?}");
    }

    /// Inside tmux the sequence is PASSED THROUGH, with every `ESC` doubled — a single one would end
    /// the wrapper at the payload's own terminator and print the tail.
    #[test]
    fn inside_tmux_the_notification_is_passed_through_it() {
        let sequence = outward(Forward::Always, &[("TMUX", "/tmp/tmux-1000/default,42,0")])
            .sequence(&said("pane 0: done", Severity::Note));
        assert_eq!(
            sequence,
            b"\x1bPtmux;\x1b\x1b]9;[work] pane 0: done\x1b\x1b\\\x1b\\".to_vec(),
        );
    }

    /// A session name that would break the row's own rules loses the PREFIX and keeps the words —
    /// the words are what the person needs, and they are already a validated value.
    ///
    /// Two ways in: a control character (which would end the OSC early and inject the rest into
    /// somebody's terminal) and a name so long the composed line passes the cap.
    #[test]
    fn a_hostile_session_name_costs_the_prefix_and_not_the_message() {
        for hostile in [
            "work\u{7}",
            "w\u{1b}]9;forged",
            &"n".repeat(MessageText::MAX_BYTES),
        ] {
            let mut options = Options::default();
            options
                .set(NOTIFY_OUTWARD, Forward::Always.word())
                .expect("always is a policy");
            let outward = Outward::of(&options, hostile.to_owned(), |_| None);
            let sequence =
                String::from_utf8(outward.sequence(&said("pane 2: it finished", Severity::Note)))
                    .expect("utf8");
            assert_eq!(
                sequence, "\u{1b}]9;pane 2: it finished\u{1b}\\",
                "{hostile:?} must cost the prefix and nothing else",
            );
        }
    }

    /// The mode is asked for and given back, and NEITHER is spelled as a number anywhere in sprag —
    /// so this pins what termwiz's own vocabulary renders for the mode this client depends on.
    #[test]
    fn the_focus_mode_is_asked_for_and_given_back() {
        let mut on = Vec::new();
        Outward::watch_focus(true, &mut on);
        assert_eq!(String::from_utf8(on).expect("utf8"), "\u{1b}[?1004h");
        let mut off = Vec::new();
        Outward::watch_focus(false, &mut off);
        assert_eq!(String::from_utf8(off).expect("utf8"), "\u{1b}[?1004l");
    }

    /// The write half, at both answers: a policy that says follow writes the sequence, and one that
    /// does not writes NOTHING — not a truncated sequence, not a bare terminator.
    #[test]
    fn forwarding_writes_the_sequence_and_declining_writes_nothing() {
        let announcement = said("pane 0: bell", Severity::Note);
        let mut sent = Vec::new();
        outward(Forward::Unfocused, &[]).forward(Some(Person::Away), &announcement, &mut sent);
        assert_eq!(
            String::from_utf8(sent).expect("utf8"),
            "\u{1b}]9;[work] pane 0: bell\u{1b}\\",
        );

        let mut silent = Vec::new();
        outward(Forward::Unfocused, &[]).forward(Some(Person::Here), &announcement, &mut silent);
        assert!(
            silent.is_empty(),
            "a person who is looking at the row gets no second copy: {silent:?}",
        );

        let mut off = Vec::new();
        outward(Forward::Off, &[]).forward(Some(Person::Away), &announcement, &mut off);
        assert!(
            off.is_empty(),
            "`off` is the silence sprag had before: {off:?}"
        );
    }
}
