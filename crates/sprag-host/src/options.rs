//! The user's named SETTINGS — tmux's options — and the one table that can enumerate them.
//!
//! [`crate::keymap`] answers "what does this key do"; this answers "what is this client's policy",
//! for the settings that are neither a key nor a command. tmux's `set-option` / `show-options`.
//!
//! ## Why a registry, and not a struct with a field per setting
//!
//! A struct can hold the settings. It cannot ENUMERATE them, and enumeration is the whole point of
//! an options table: a user who does not already know an option's name has no way to find it, and a
//! setting nobody can discover is one nobody can use. So the names, the values they take and what
//! they mean with the user silent are DATA ([`OPTIONS`]) rather than fields — `show-options` walks
//! it, `set-option` looks a name up in it, and a mistyped name is answered with the list.
//!
//! sprag's settings were spelled as environment variables before this table existed, which is the
//! same defect in a worse form: an env var cannot be listed, cannot be validated, and cannot be
//! changed without restarting the process that read it.
//!
//! ## Why the registry holds no VALUE
//!
//! [`OPTIONS`] is a description of the option space; [`Options`] is what the user's file says. Keeping
//! them apart is what makes a default a single fact: an option the file never mentions has the value
//! [`OptionSpec::default`] gives it, computed nowhere and stored nowhere, so there is no second copy
//! to go stale. `show-options` therefore prints every option — set or not — which is what a user
//! needs in order to discover one, and what the file alone cannot tell them.
//!
//! ## Why a value is stored CANONICALISED
//!
//! `^a` and `C-a` are one keystroke, `Off` and `off` are one policy. A value is validated by parsing
//! it and stored as the parse's own spelling, so the file, `show-options` and the routing table can
//! never disagree about what the user chose — the rule
//! [`bind_key`](crate::config::bind_key) already follows for a key.
//!
//! ## No option crosses the WIRE — each process reads the file itself
//!
//! Some of these are one CLIENT's: a prefix key, a detach policy and a font size are what one client
//! does with one keyboard, one attachment and one window, and two clients may legitimately differ. But
//! [`DEFAULT_COMMAND`] is the DAEMON's, because that is where a pane is born, and an option about what
//! a pane runs cannot live anywhere else — and [`WINDOW_SIZE`] is the daemon's for the opposite
//! reason: it settles what several clients CANNOT be allowed to differ about, so no one of them can
//! hold it.
//!
//! So the invariant is not "every option is a client's" — an earlier version of this doc said that,
//! and `default-command` is the counter-example that corrected it. The invariant that actually holds,
//! and the one worth keeping, is that **nothing here crosses the wire**: every process that needs an
//! option reads the user's file itself. The daemon already does exactly that for `[[command]]`
//! ([`Host::global_commands`](crate::HostClient::global_commands), read from disk on every call), so
//! this adds a READER to a file it was reading, not a dependency it did not have.
//!
//! What that buys is the property `list-keys` has: `sprag show-options` answers on a machine with no
//! session running, and no verb here has to ask a daemon what it thinks a setting is.
//!
//! An option is NOT the place for an operator's control. `SPRAG_RESTORE_HISTORY` bounds what a pane's
//! output writes to disk and `SPRAG_OSC52` decides whether a program may read the user's clipboard;
//! both stay in the environment deliberately, because `config.toml` is the USER's file and a client
//! re-reads it live — an exposure limit that a user can edit, and that takes effect without the
//! daemon restarting, is not an exposure limit.

use crate::outward::Forward;
use crate::window::WindowSize;
use std::collections::{BTreeMap, BTreeSet};

use crate::keymap::KeySpec;

/// The values an option accepts, which is also how one is VALIDATED.
///
/// Each kind carries its vocabulary WITH it, so a bad value can be answered with the alternatives
/// rather than with a type name: a key is whatever [`KeySpec`] parses (the option space and the keymap
/// therefore cannot drift apart), a choice is a fixed list, and a number is an integer at or above a
/// floor the option itself names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionKind {
    /// A keystroke spec — `C-a`, `%`, `F1`. Validated by [`KeySpec::parse`], so this option's
    /// vocabulary IS the keymap's and neither has to be kept in step with the other.
    Key,
    /// One of a fixed set of names, matched case-insensitively and stored lowercase.
    Choice(&'static [&'static str]),
    /// An integer at or above `min`.
    ///
    /// The floor is per option because zero is not one thing. For a SIZE it is nonsense — a glyph of
    /// no pixels is not a smaller glyph, so [`GUI_FONT`] floors at 1 and a caller asking for it is
    /// never handed a size of nothing. For a RETENTION COUNT it is a decision — "keep no history" is
    /// exactly what a user who wants a pane that remembers nothing has asked for, and tmux's own
    /// `history-limit` takes it — so [`HISTORY_LIMIT`] floors at 0. This is the distinction
    /// [`Command`](Self::Command) already draws for the empty string: an empty value is a value where
    /// emptiness means something, and an absence where it does not.
    ///
    /// A floor rather than a `zero: bool` because it is the honest shape of the constraint and costs
    /// a future option with a real minimum nothing. Still no upper bound: a bound nothing measures is
    /// taste, and a value too large for a `u32` is refused as not being a number, which is the type's
    /// own bound and not an invented one.
    Number { min: u32 },
    /// A shell COMMAND LINE, empty for none.
    ///
    /// The one kind whose vocabulary is genuinely open, and saying so is the honest thing rather than
    /// a gap: the grammar is the user's SHELL's, and a validator of ours would be a second, poorer
    /// one. Whether the command exists is the shell's answer too, and it lands in the pane where the
    /// user reads it — which is a better report than any refusal here could be.
    ///
    /// Empty is VALID and means "no command", the state tmux's own `default-command` uses to mean
    /// "run the shell". So this is the kind where an empty value is a value — the same judgement
    /// [`Number`](Self::Number) makes per option with its floor, and for the same reason: emptiness
    /// is a decision for some settings and an absence for others, and only the setting knows which.
    Command,
}

impl OptionKind {
    /// `value` canonicalised, or why it cannot be used — the ONE validation, so the file reader, the
    /// CLI and [`Options::set`] cannot disagree about what is acceptable.
    ///
    /// # Errors
    ///
    /// The reason, phrased to be read after `NAME: ` — [`KeySpec`]'s own complaint for a key, the list
    /// of alternatives for a choice, and what a number has to be.
    pub fn canonicalise(self, value: &str) -> Result<String, String> {
        match self {
            Self::Key => KeySpec::parse(value)
                .map(|key| key.to_string())
                .map_err(|error| error.to_string()),
            Self::Choice(choices) => {
                let folded = value.trim().to_ascii_lowercase();
                choices
                    .iter()
                    .find(|choice| **choice == folded)
                    .map(|choice| (*choice).to_owned())
                    .ok_or_else(|| format!("{value:?} is not one of: {}", choices.join(", ")))
            }
            // Re-rendered from the PARSE rather than trimmed, so `007` and ` 7 ` are stored as `7` —
            // the same rule a key follows, and what keeps one value one string in the file.
            Self::Number { min } => match value.trim().parse::<u32>() {
                Ok(number) if number >= min => Ok(number.to_string()),
                // The complaint NAMES the floor rather than saying "above zero" for every option,
                // because the floor is now the thing that differs between them and a reader who is
                // told the wrong one will try the wrong value next.
                _ if min == 0 => Err(format!("{value:?} is not a number")),
                _ => Err(format!("{value:?} is not a number {min} or above")),
            },
            // Trimmed and otherwise untouched: the INSIDE of the line is the shell's grammar, so
            // normalising any of it would be this table editing a command it does not parse.
            Self::Command => Ok(value.trim().to_owned()),
        }
    }

    /// `value` as `show-options` should PRINT it in the `name value` form.
    ///
    /// A [`Command`](Self::Command) is shell-quoted, which is how tmux prints a string option and the
    /// only way an empty one can appear at all: `default-command ''` says something, while a line
    /// ending in a space says nothing a reader can see. Every other kind is a single bare word by
    /// construction, so quoting it would only add noise.
    ///
    /// `show-options -v` deliberately does NOT use this: a script reading one value wants the value,
    /// not a rendering of it.
    #[must_use]
    pub fn render(self, value: &str) -> String {
        match self {
            Self::Command => crate::shellword::shell_quote(value),
            _ => value.to_owned(),
        }
    }
}

/// One option: its NAME, the values it takes, and what it means with the user silent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OptionSpec {
    /// tmux's own spelling, so a tmux user needs to learn nothing — and the file's key, so
    /// `show-options` output and `config.toml` are one vocabulary.
    pub name: &'static str,
    /// What values it takes.
    pub kind: OptionKind,
    /// The value in force when the file does not mention it.
    ///
    /// A `&'static str` rather than a computed value because [`OPTIONS`] is a `const`, which means a
    /// default that ALSO lives somewhere else is spelled twice — [`prefix`](OPTIONS)'s lives on
    /// [`Keymap::default`](crate::keymap::Keymap::default). Nothing in the type system holds the two
    /// together; a test does (`the_registry_defaults_are_the_defaults_in_force`), which is the same
    /// treatment [`crate::config`]'s field names get.
    pub default: &'static str,
}

/// The key that says "the next keystroke is the client's" — tmux's `prefix`.
///
/// Named rather than spelled at each use, so the one string that ties [`OPTIONS`], the file reader
/// and the keymap together is written once.
pub const PREFIX: &str = "prefix";

/// How a client reacts when its own attached session is destroyed — tmux's `detach-on-destroy`.
pub const DETACH_ON_DESTROY: &str = "detach-on-destroy";

/// What a pane runs when no command was specified — tmux's `default-command`.
///
/// The FIRST option a daemon reads, and the reason the invariant in this module's docs is about the
/// WIRE rather than about clients: a pane is born in the daemon, so a setting about what it runs has
/// to be read there. Nothing crosses the wire for it — the daemon reads the user's file itself, as it
/// already does for `[[command]]`.
pub const DEFAULT_COMMAND: &str = "default-command";

/// The windowed client's glyph size in pixels.
///
/// `gui-`prefixed because it governs ONE frontend and tmux has no twin: a terminal client's font is
/// its terminal emulator's, not sprag's. The prefix is what keeps that legible in a flat namespace —
/// a user reading `show-options` can see which of their clients a setting reaches.
pub const GUI_FONT: &str = "gui-font";

/// How many lines of scrolled-off output a pane keeps — tmux's `history-limit`.
///
/// A DAEMON-side option like [`DEFAULT_COMMAND`] and for the same reason: the scrollback lives in the
/// emulator, which lives in the daemon, so the setting is read where the pane is BORN. Nothing crosses
/// the wire for it.
///
/// Read at each birth rather than cached at boot, exactly as [`DEFAULT_COMMAND`] is, so a user who
/// raises it gets deeper history on their next pane without restarting the daemon. A LIVE pane keeps
/// the limit it was born with — which is tmux's model, and the honest one here: lowering the limit
/// re-applied to existing panes would DESTROY retained output as a side effect of editing a config
/// file, and the only way to offer that safely is a verb the user aims at a pane, which is a
/// different feature from an option.
///
/// It also sets the default depth of what SURVIVES a restart, because
/// [`history_limit`](crate::history_limit) derives the persistence budget from it — a pane configured
/// to keep 50,000 lines whose saved history stopped at 1,000 would lose the difference at every
/// reboot, silently.
pub const HISTORY_LIMIT: &str = "history-limit";

/// The most memory one pane may use before the kernel throttles it, in MEBIBYTES (R337).
///
/// A DAEMON-side option like [`HISTORY_LIMIT`] and read at each BIRTH for its reason: raising a
/// ceiling reaches the user's next pane rather than their next daemon, and a live pane keeps what
/// it was born with. Lowering a ceiling under a running build would be a config edit that changes
/// what somebody's program is allowed to do mid-run, which is a verb aimed at a pane and not an
/// option.
///
/// **Zero is a DECISION**, [`HISTORY_LIMIT`]'s distinction again and with the opposite polarity:
/// `0` means NO CEILING, because that is the state a person returns to by clearing the setting and
/// a ceiling of literally zero bytes is not a pane. Uncapped is also the default, deliberately — a
/// number invented without a person is a number nobody can explain when their build slows down.
///
/// MEBIBYTES rather than bytes because it is what a person types; the kernel is told bytes. It maps
/// to `memory.high`, which THROTTLES and reclaims, never `memory.max`, which OOM-kills: a ceiling
/// set to protect the other panes should not be a way to lose the pane it is set on. Enforced only
/// where a share is (see [`sprag_terminal::Enforcement`]); a host that cannot enforce says so once
/// at start-up rather than pretending per pane.
pub const PANE_MEMORY_LIMIT: &str = "pane-memory-limit";

/// The most processes one pane may have alive at once (R337).
///
/// [`PANE_MEMORY_LIMIT`]'s twin in every respect — daemon-side, read at birth, `0` meaning no
/// ceiling, uncapped by default — over `pids.max`. What it is FOR is the fork storm: one pane's
/// runaway `make -j` taking the pid budget its neighbours need is the failure a weight cannot
/// prevent, because a weight shares a resource that is contended and a pid table is a resource that
/// simply runs out.
///
/// The mechanism landed at R336 with no caller, which is the shape the debt question hunts (*an
/// answer nobody reads*); this is the person who reads it.
pub const PANE_PROCESS_LIMIT: &str = "pane-process-limit";

/// How long a client shows what a key just DID, in milliseconds — tmux's `display-time`.
///
/// A CLIENT-side option like [`PREFIX`] and [`REPEAT_TIME`], and the reason is theirs sharpened: a
/// message is a sentence one PERSON reads, and two people attached to one session read at different
/// speeds. Nothing crosses the wire.
///
/// Zero is a DECISION and not an absence, the fourth setting to draw [`HISTORY_LIMIT`]'s
/// distinction: `display-time 0` is a message that has already expired, so a client reports
/// nothing. That is a legitimate thing to want from a user who has memorised their own bindings,
/// and it is the one value that puts back the silence this option's consumer exists to remove —
/// which is why it is reachable only by asking for it. tmux accepts `0` and refuses a negative
/// value, which is `Number { min: 0 }` exactly.
///
/// The default is [`report::DEFAULT_DISPLAY_TIME`](crate::report::DEFAULT_DISPLAY_TIME) rather than
/// a number spelled twice; `the_display_time_default_is_the_reports_own` holds the two together.
pub const DISPLAY_TIME: &str = "display-time";

/// How long a repeating binding holds the prefix table open, in milliseconds — tmux's
/// `repeat-time`.
///
/// A CLIENT-side option like [`PREFIX`], and the second one the keymap is built FROM rather than
/// beside: a `-r` binding is a statement about one keyboard's timing, and two clients attached to one
/// session may legitimately disagree about it. Nothing crosses the wire.
///
/// Zero is a DECISION and not an absence — the distinction [`HISTORY_LIMIT`] drew for retention, here
/// for a duration: `repeat-time 0` is a window that has already closed, so a `-r` binding acts exactly
/// once. tmux accepts `0` and refuses a negative value, which is `Number { min: 0 }` exactly.
pub const REPEAT_TIME: &str = "repeat-time";

/// How long an agent-state verdict resting on an ABSENCE must hold before it is published, in
/// milliseconds — H3's settle window.
///
/// A DAEMON-side option like [`WINDOW_SIZE`], and the reason is stronger here than "the daemon owns
/// the state": the verdict is computed once per pane and put on the pane list for every client
/// (H3's D2), so a per-client window would be several clients disagreeing about what one published
/// fact means. Nothing about it crosses the wire — what crosses is the settled `agent` key.
///
/// Zero is a DECISION, the third setting to draw [`HISTORY_LIMIT`]'s distinction: `0` means publish
/// every reading as it arrives, which is hysteresis turned off. That is a legitimate thing to want
/// from a detector you are debugging, and it is exactly what the measurements say NOT to ship as the
/// default — R249's M2 measured the working spinner alternating at about 1 Hz, and a window shorter
/// than that animation publishes the flicker the window exists to absorb.
///
/// The default is [`sprag_detect::DEFAULT_SETTLE`] rather than a number spelled here, held to it by
/// `the_agent_settle_default_is_the_detectors_own` — the treatment [`HISTORY_LIMIT`] gets against the
/// emulator and [`REPEAT_TIME`] against the keymap.
/// Whether a client TOO NARROW for a pane re-wraps that pane's lines into the width it can show,
/// instead of showing a slice of them — `on` / `off`.
///
/// A CLIENT-side option like [`PREFIX`] and [`REPEAT_TIME`], and here the reason is not merely that
/// two clients may disagree: the pane is not involved at all. A pty has ONE winsize, so a shared
/// pane can never be re-laid-out for one watcher (R346); what a client owns is its own PICTURE of
/// it, and this says how that picture is drawn. Nothing crosses the wire, no other client can see
/// it, and the child is never told.
///
/// **The default is `on`, and that is a change of behaviour ONLY where the old one was measured
/// broken.** It engages when a pane is wider than the whole of a client's screen, which for a solo
/// user never happens — their client IS the window. For the case it does cover, R349 drove what
/// `off` gives: a 60-column client watching a 100-column window could read sixty columns of a
/// 78-character line, and WHICH sixty was decided by where the cursor happened to be — the line's
/// first nineteen columns while typing it, its last twenty-two once Enter moved the cursor away.
/// There is no key that reaches the rest, because the view is pinned to the cursor.
///
/// `off` is what a person asks for when they want the pane's true geometry — column-aligned output
/// keeps its columns, and the part that does not fit is simply not shown. It is a real preference
/// and it is the one this option exists for; it is not the safe default, because the thing it is
/// safe from is a reshape and the thing it costs is text nobody can reach.
///
/// It never applies to the ALTERNATE screen: a program there owns absolute cell positions at the
/// width it was told, and that refusal is [`sprag_grid::rewrap`]'s own rather than a setting
/// anybody can turn off.
pub const REWRAP: &str = "rewrap";

pub const AGENT_SETTLE_TIME: &str = "agent-settle-time";

/// [`DETACH_ON_DESTROY`]'s values, in tmux's documented order.
///
/// The vocabulary lives HERE and the policy lives in the client that acts on it (`sprag-client`
/// parses one of these into its own enum), because a crate holding a display client's behaviour
/// cannot be depended on by this one. A test in that crate holds the two together: every name here
/// must parse to a distinct policy there, or the table offers a value nothing performs.
pub const DETACH_ON_DESTROY_VALUES: &[&str] = &["on", "off", "no-detached", "next", "previous"];

/// The values of a plain switch — tmux's own two words, and the vocabulary
/// [`option_is_on`](crate::config::option_is_on) reads.
///
/// Shared by every switch rather than spelled per option, so a third one cannot arrive accepting
/// `true` / `yes` while the two below take `on` / `off`.
pub const ON_OFF: &[&str] = &["on", "off"];

/// Whether a pane's BELL reaches the people looking at that session — tmux's `monitor-bell`.
///
/// tmux's own name and tmux's own default (`on`), so a tmux user needs to learn nothing. What
/// differs is the ACTION, and the difference is a property of the product rather than a setting:
/// tmux has `bell-action` because it can pass the bell through to the terminal it is running in,
/// and a daemon serving a GUI window and a terminal client cannot ring anything — so sprag's bell
/// is always the visual one. Turning this off is how a user asks for the silence they had before.
pub const MONITOR_BELL: &str = "monitor-bell";

/// Whether a pane's desktop-style NOTIFICATION (`OSC 9` / `OSC 777;notify` / `OSC 99`) reaches the
/// people looking at that session.
///
/// tmux has no counterpart: it passes these escapes through to the outer terminal and models
/// nothing, so there is nothing to switch. The name follows [`MONITOR_BELL`]'s family because it is
/// the same question about the other attention source, and the default is `on` for the reason the
/// round exists — measured at `3114923`, a child raising one reached a live client's screen NOWHERE,
/// and a feature that ships off by default is the same silence with a switch beside it.
///
/// A user who finds their build tool chatty turns this off and keeps the bell, or the reverse; the
/// two sources are separate options because the emulator keeps their sequences separate, and one
/// switch over both would make a chatty notifier cost the user their bell.
pub const MONITOR_NOTIFICATION: &str = "monitor-notification";

/// Whether a message a display client shows also follows the PERSON out of the client — and when.
///
/// tmux has no counterpart: it can pass a pane's notification escape through to the outer terminal
/// untouched, which is not the same act at all (it forwards what a CHILD wrote, unconditionally and
/// without knowing what it says). This forwards what sprag DECIDED to tell a person, as a
/// notification of its own, only when the surface it painted cannot have been read.
///
/// Not `tui-`prefixed, unlike [`GUI_FONT`]: the question — *should a message reach the person when
/// they are not looking at this client?* — is frontend-independent, and only the MECHANISM belongs
/// to a front. **Both perform it**, each in its own medium and from the same policy word:
///
/// * `sprag-tui` writes the terminal it is running in an `OSC 9` (kitty's `OSC 99` where that
///   carries the urgency), because its machine may be a server reached over ssh and the terminal at
///   the far end of that pipe is where the person is sitting.
/// * `sprag-gui` asks the DESKTOP, because a process that has opened a window is by construction on
///   the machine the person is at. It reads where they are from the window manager rather than from
///   a terminal mode, so it needs nothing of the person's terminal to be true.
///
/// The values and the default live on [`Forward`], in this crate, which is
/// what lets [`NOTIFY_OUTWARD_VALUES`] be derived from the policy instead of held level with it by a
/// cross-crate test — the opposite of the arrangement [`DETACH_ON_DESTROY_VALUES`] documents, and
/// the reason that one still needs its test.
pub const NOTIFY_OUTWARD: &str = "notify-outward";

/// [`NOTIFY_OUTWARD`]'s values, in order of increasing loudness.
///
/// DERIVED from [`Forward`], which is the policy both display clients
/// perform — so the words a user may write and the words a client acts on are one list, the way
/// [`WINDOW_SIZE_VALUES`] already is. It was a second spelling held level by a cross-crate test
/// until the policy moved into this crate; the test that remains checks what a derivation cannot
/// (that each word survives this table's own canonicalisation).
pub const NOTIFY_OUTWARD_VALUES: &[&str] = &[
    Forward::Off.word(),
    Forward::Unfocused.word(),
    Forward::Always.word(),
];

/// How big a session's window is when several clients of different sizes are attached — tmux's
/// `window-size`.
///
/// A DAEMON-side option like [`DEFAULT_COMMAND`], and for the same kind of reason: the window is a
/// fact about every client at once, so no one client can decide it. What crosses the wire is the
/// arbitrated SIZE ([`crate::wire::WINDOW_SIZE_SLOT`]), never this setting.
///
/// It arbitrates over the clients that report an area, which is BOTH of them: `sprag-tui` reports
/// its terminal, and `sprag-gui` reports the cells its tiled panes measured
/// (`sprag_terminal::fit_window` — its chrome is per pane, so its surface is not the answer). So
/// this option reaches a window as well as a terminal, which it did not when it was first written.
pub const WINDOW_SIZE: &str = "window-size";

/// [`WINDOW_SIZE`]'s values — [`WindowSize`]'s own names, taken from the enum rather than re-spelled
/// here, so the option's vocabulary and the policy that performs it cannot drift. The same rule
/// [`PREFIX`] follows by validating through `KeySpec`, and the opposite of
/// [`DETACH_ON_DESTROY_VALUES`], whose policy lives in a crate this one cannot depend on.
///
/// `manual` is here on the same terms as the other three — it names a rule the daemon performs, by
/// reading the size `sprag resize-window` pinned on the window. What it does with NOTHING pinned is
/// [`arbitrate`](crate::window::arbitrate)'s to state, not this table's: an empty source is not an
/// unperformed rule.
pub const WINDOW_SIZE_VALUES: &[&str] = &[
    WindowSize::Largest.name(),
    WindowSize::Smallest.name(),
    WindowSize::Latest.name(),
    WindowSize::Manual.name(),
];

/// Every option sprag has, sorted by name so `show-options` output is stable.
///
/// An option earns its place by having a live CONSUMER — a setting nothing reads is exactly the
/// defect this table exists to remove, one indirection further along. tmux's remaining hundred are
/// not absent because the table cannot hold them; they are absent because sprag has no behaviour for
/// them to govern yet.
pub const OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: AGENT_SETTLE_TIME,
        // Floors at 0, where zero MEANS "publish on every reading" — hysteresis off — rather than
        // "unset". See the name's own doc for why that is a decision and not a gap.
        kind: OptionKind::Number { min: 0 },
        // The detector's own default, so a user who has said nothing gets the window the
        // measurements chose. Spelled here AND as `sprag_detect::DEFAULT_SETTLE`;
        // `the_agent_settle_default_is_the_detectors_own` holds the two together.
        default: "2000",
    },
    OptionSpec {
        name: DEFAULT_COMMAND,
        kind: OptionKind::Command,
        // Empty, exactly as tmux's is: with nothing said, a pane runs the user's shell. The default
        // is therefore not "a shell" spelled here — `$SHELL` is `default_shell_command`'s, and
        // spelling it twice would put this table in the business of naming a program it never reads.
        default: "",
    },
    OptionSpec {
        name: DETACH_ON_DESTROY,
        kind: OptionKind::Choice(DETACH_ON_DESTROY_VALUES),
        default: "on",
    },
    OptionSpec {
        name: DISPLAY_TIME,
        // Floors at 0, where zero MEANS "report nothing" rather than "unset" — see the name's own
        // doc for why that is a decision a user has to ask for.
        kind: OptionKind::Number { min: 0 },
        // tmux's own default, read from `tmux 3.2a`'s `show-options -g display-time` on this
        // machine rather than recalled. Spelled here AND as `report::DEFAULT_DISPLAY_TIME`;
        // `the_display_time_default_is_the_reports_own` holds the two together, the treatment
        // `repeat-time` gets against the keymap.
        default: "750",
    },
    OptionSpec {
        name: GUI_FONT,
        // Floors at 1: a glyph of no pixels is not a smaller glyph.
        kind: OptionKind::Number { min: 1 },
        default: "20",
    },
    OptionSpec {
        name: HISTORY_LIMIT,
        // Floors at 0, where zero MEANS "keep nothing" rather than "unset" — see the kind's own doc.
        kind: OptionKind::Number { min: 0 },
        // The emulator's own default, so a user who has said nothing gets exactly the retention
        // sprag had before this option existed. Spelled here AND as
        // `sprag_vt::DEFAULT_SCROLLBACK_LINES`; `the_history_limit_default_is_the_emulators_own`
        // holds the two together, the treatment `prefix` gets against the keymap.
        default: "1000",
    },
    OptionSpec {
        name: MONITOR_BELL,
        kind: OptionKind::Choice(ON_OFF),
        // tmux's own default, so a tmux user's bell behaves as they expect.
        default: "on",
    },
    OptionSpec {
        name: MONITOR_NOTIFICATION,
        kind: OptionKind::Choice(ON_OFF),
        default: "on",
    },
    OptionSpec {
        name: NOTIFY_OUTWARD,
        kind: OptionKind::Choice(NOTIFY_OUTWARD_VALUES),
        // The middle value: exactly the messages a person could not have seen. `off` is the silence
        // sprag had before it existed and `always` is for a terminal that reports no focus — both
        // are answers to this one being wrong for somebody, which is why it is the default rather
        // than the safe-looking `off`.
        default: "unfocused",
    },
    OptionSpec {
        name: PANE_MEMORY_LIMIT,
        // Floors at 0, where zero MEANS "no ceiling" rather than "unset" — the name's own doc says
        // why that polarity is a decision and not a gap.
        kind: OptionKind::Number { min: 0 },
        // Uncapped. A number here would be a ceiling nobody chose, imposed on every pane of every
        // session, and the person who hit it would have no way to know what had happened.
        default: "0",
    },
    OptionSpec {
        name: PANE_PROCESS_LIMIT,
        kind: OptionKind::Number { min: 0 },
        // Uncapped, on `pane-memory-limit`'s argument and more sharply: a pid ceiling shows up as
        // `fork: retry`, which is the least explicable error a shell produces.
        default: "0",
    },
    OptionSpec {
        name: PREFIX,
        kind: OptionKind::Key,
        default: "C-b",
    },
    OptionSpec {
        name: REPEAT_TIME,
        // Floors at 0, which is where tmux floors it too: `repeat-time 0` is accepted there and
        // `-1` is refused with "value is too small". Measured, not recalled.
        kind: OptionKind::Number { min: 0 },
        // tmux's own default. Spelled here AND as `keymap::DEFAULT_REPEAT_TIME`, which
        // `Keymap::default` needs because `sprag list-keys` answers with no config file at all;
        // `the_repeat_time_default_is_the_keymaps_own` holds the two together, the treatment
        // `history-limit` gets against the emulator.
        default: "500",
    },
    OptionSpec {
        name: REWRAP,
        kind: OptionKind::Choice(ON_OFF),
        // ON, because the case it covers is one this project MEASURED as text a person cannot
        // reach — see the name's own doc for the numbers and for why `off` is still a real want.
        default: "on",
    },
    OptionSpec {
        name: WINDOW_SIZE,
        kind: OptionKind::Choice(WINDOW_SIZE_VALUES),
        // What the code did before the option existed, so a solo user's panes do not change size
        // the day this ships.
        default: WindowSize::DEFAULT.name(),
    },
];

/// The spec for `name`, or `None` when no option is called that.
#[must_use]
pub fn spec(name: &str) -> Option<&'static OptionSpec> {
    OPTIONS.iter().find(|spec| spec.name == name)
}

/// Every option's name, for an error that has to say what the alternatives are.
#[must_use]
pub fn names() -> String {
    OPTIONS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why an option could not be set.
///
/// Two variants because they are two different mistakes with two different fixes, and a caller
/// renders them differently: the CLI reports either as an ARGUMENT problem (naming no file), while
/// the file reader wraps both as a problem with `config.toml`. That split is why this type does not
/// mention a file itself.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OptionError {
    /// No option has this name. Carries the list, because a user who mistyped one needs to see the
    /// real ones rather than be told the name they already typed.
    Unknown(String),
    /// The option exists and will not take this value.
    Value {
        /// The option named.
        name: &'static str,
        /// Why, from [`OptionKind::canonicalise`].
        why: String,
    },
}

impl std::error::Error for OptionError {}

impl std::fmt::Display for OptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(name) => {
                write!(f, "there is no option {name:?} (there are: {})", names())
            }
            Self::Value { name, why } => write!(f, "{name}: {why}"),
        }
    }
}

/// A VALIDATED option and value — the only way to name a setting to a writer.
///
/// The point is what it makes impossible. [`crate::config::set_option`] takes one of these rather
/// than two strings, so every error that writer can report is about the FILE: a mistyped option name
/// or value is refused HERE, by the caller that owns the mistake, and never rendered with a
/// `config.toml` prefix that would send a user to fix a file that is fine. Exactly the rule
/// [`bind_key`](crate::config::bind_key) states for a key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OptionSetting {
    /// The option named.
    spec: &'static OptionSpec,
    /// Its value, canonicalised by [`OptionKind::canonicalise`].
    value: String,
}

impl OptionSetting {
    /// Validate `name` and `value` together — the ONE canonicalisation site, used by the CLI and by
    /// the file reader alike, so a value the reader accepts is one the writer would have written.
    ///
    /// # Errors
    ///
    /// [`OptionError::Unknown`] when no option has that name, [`OptionError::Value`] when it will not
    /// take that value.
    pub fn parse(name: &str, value: &str) -> Result<Self, OptionError> {
        let spec = spec(name).ok_or_else(|| OptionError::Unknown(name.to_owned()))?;
        let value = spec
            .kind
            .canonicalise(value)
            .map_err(|why| OptionError::Value {
                name: spec.name,
                why,
            })?;
        Ok(Self { spec, value })
    }

    /// The option this sets.
    #[must_use]
    pub fn spec(&self) -> &'static OptionSpec {
        self.spec
    }

    /// The canonical value, as it should appear in the file and in `show-options`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The value as a NUMBER when this option takes one, so a writer can put `20` in the file rather
    /// than `"20"`.
    ///
    /// That distinction is the user's, not the parser's: the file is hand-maintained and a quoted
    /// number is not how a person writes one. The reader accepts both spellings for the same reason.
    #[must_use]
    pub fn as_number(&self) -> Option<u32> {
        matches!(self.spec.kind, OptionKind::Number { .. })
            .then(|| self.value.parse().ok())
            .flatten()
    }
}

/// The options IN FORCE: every [`OPTIONS`] entry's default, with whatever the user's file declares
/// layered over it.
///
/// Always complete — [`get`](Self::get) answers for every registered option whether or not the file
/// mentions it — because the alternative is a caller that has to remember the default at each read,
/// which is the second copy this module exists to avoid.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Options {
    /// Canonical value per option name, keyed by the registry's own `&'static str` so a name that is
    /// not an option cannot be stored. `BTreeMap` for the iteration order: `show-options` output is
    /// sorted by name, like tmux's.
    values: BTreeMap<&'static str, String>,
    /// **THE NAMES SOMEBODY ACTUALLY CHOSE**, as opposed to the ones standing at their registry
    /// default — see [`chosen`](Self::chosen).
    picked: BTreeSet<&'static str>,
}

impl Default for Options {
    /// Every option at its [`OptionSpec::default`], and nothing chosen.
    fn default() -> Self {
        Self {
            values: OPTIONS
                .iter()
                .map(|spec| (spec.name, spec.default.to_owned()))
                .collect(),
            picked: BTreeSet::new(),
        }
    }
}

impl Options {
    /// The value in force for `name`, or `None` when no option is called that.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
    /// Set `name` to `value`, canonicalised.
    ///
    /// # Errors
    ///
    /// [`OptionError::Unknown`] when no option has that name, [`OptionError::Value`] when it will not
    /// take that value. The stored value is unchanged either way — an invalid set leaves the previous
    /// one in force rather than a half-applied table.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), OptionError> {
        let setting = OptionSetting::parse(name, value)?;
        self.picked.insert(setting.spec.name);
        self.values.insert(setting.spec.name, setting.value);
        Ok(())
    }

    /// **THE VALUE SOMEBODY CHOSE FOR `name`**, or [`None`] where it is standing at the registry's
    /// default — as distinct from [`get`](Self::get), which always answers.
    ///
    /// # ⚠⚠⚠⚠ Why the distinction has to exist, measured
    ///
    /// [`get`](Self::get) is *"what is in force"* and is deliberately total: a caller must never have
    /// to remember a default, which is the second copy this module exists to prevent. But a caller
    /// that wants to supply its OWN default cannot use it — **the answer is never absent**, so the
    /// caller's arm is unreachable and a default written there is dead code that reads as live.
    ///
    /// That is exactly what happened. `detach-on-destroy` is right as `on` for a terminal client and
    /// wrong for a window, so the window was given its own fallback through
    /// `options.get(..).map_or(mine, parse)` — and `get` answered `Some("on")` from this table every
    /// time, so the window's fallback never ran. **Its gate passed**, because the gate called the
    /// fallback directly instead of through here. The defect reached the owner as *a window that
    /// closes when you close a session*, twice, after being reported fixed.
    ///
    /// ⚠⚠⚠ SO THE QUESTION A PER-CALLER DEFAULT MUST ASK IS *"did anybody choose this"*, and only
    /// this answers it. An option nobody set reads [`None`] here and the caller's own default is
    /// reachable; an option a person set to the same value as the default still reads [`Some`], and
    /// their choice wins — which is the half a *"compare against the default"* trick gets wrong.
    #[must_use]
    pub fn chosen(&self, name: &str) -> Option<&str> {
        self.picked
            .contains(name)
            .then(|| self.values.get(name).map(String::as_str))
            .flatten()
    }

    /// The value in force for a [`OptionKind::Number`] option, or `None` when `name` is not one.
    ///
    /// Cannot fail for a Number option: every stored value came through
    /// [`canonicalise`](OptionKind::canonicalise), which parsed it, and the registry's own default is
    /// checked by `every_option_default_is_a_value_that_option_accepts`. A caller still handles `None`
    /// rather than unwrapping, because a windowed client must not lose its window over an internal
    /// inconsistency a test already forbids.
    #[must_use]
    pub fn number(&self, name: &str) -> Option<u32> {
        matches!(spec(name)?.kind, OptionKind::Number { .. })
            .then(|| self.get(name)?.parse().ok())
            .flatten()
    }

    /// Every option and its value in force, sorted by name — what `show-options` prints.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_defaults_are_the_defaults_in_force() {
        // The drift guard `OptionSpec::default` names: a `const` table cannot compute a default that
        // another type owns, so `prefix`'s is spelled twice and only this holds them together. A
        // registry default that disagreed would make `show-options` report a prefix no client uses —
        // which is the whole failure this front keeps finding.
        let keymap = crate::keymap::Keymap::default();
        assert_eq!(
            spec("prefix").expect("prefix is an option").default,
            keymap.prefix().to_string(),
            "the registry's prefix default must be the keymap's own",
        );
        // Same drift guard, one option along: the table spells the retention default and so does the
        // emulator that enforces it. A disagreement would make `show-options history-limit` report a
        // depth no pane actually keeps.
        assert_eq!(
            spec(HISTORY_LIMIT)
                .expect("history-limit is an option")
                .default,
            sprag_vt::DEFAULT_SCROLLBACK_LINES.to_string(),
            "the registry's history-limit default must be the emulator's own retention",
        );
    }

    #[test]
    fn zero_is_a_value_for_a_retention_count_and_not_for_a_size() {
        // The reason `Number` carries a floor at all. A user asking for a pane that remembers
        // nothing has made a decision, and tmux's `history-limit 0` takes it; a glyph of no pixels
        // is not a smaller glyph. One kind cannot answer both, so the option names its own floor.
        let mut options = Options::default();
        options
            .set(HISTORY_LIMIT, "0")
            .expect("history-limit 0 keeps no history — a decision, not an absence");
        assert_eq!(options.get(HISTORY_LIMIT), Some("0"));
        assert_eq!(options.number(HISTORY_LIMIT), Some(0));

        let refused = Options::default()
            .set(GUI_FONT, "0")
            .expect_err("a zero-pixel glyph is not a size");
        assert!(
            refused.to_string().contains("1 or above"),
            "the complaint must name the floor that was missed, not a generic one: {refused}",
        );
    }

    #[test]
    fn a_history_limit_above_the_default_is_accepted_whole() {
        // The point of the option is raising it, so the path a raise takes is asserted rather than
        // assumed: canonicalised, stored, and readable back as the number the caller will use.
        let mut options = Options::default();
        options.set(HISTORY_LIMIT, " 50000 ").expect("a line count");
        assert_eq!(
            options.get(HISTORY_LIMIT),
            Some("50000"),
            "stored as the parse's own spelling, like every other value",
        );
        assert_eq!(options.number(HISTORY_LIMIT), Some(50_000));
    }

    #[test]
    fn every_option_default_is_a_value_that_option_accepts() {
        // A default that its own `kind` refuses would be a table that cannot be read back: the file
        // reader canonicalises every value it meets, including one written out by an unset.
        for spec in OPTIONS {
            let canonical = spec
                .kind
                .canonicalise(spec.default)
                .unwrap_or_else(|why| panic!("{}'s default is unusable: {why}", spec.name));
            assert_eq!(
                canonical, spec.default,
                "{}'s default must be spelled canonically",
                spec.name
            );
        }
    }

    #[test]
    fn the_registry_is_sorted_and_has_no_duplicate_name() {
        // Sorted because `show-options` walks it and a listing whose order depends on edit history is
        // one a script cannot diff. Distinct because `spec` finds the FIRST match, so a duplicate
        // would be an entry nothing can reach.
        let names: Vec<&str> = OPTIONS.iter().map(|spec| spec.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "OPTIONS must be sorted by name and unique");
    }

    #[test]
    fn a_key_option_is_stored_as_the_keymap_spells_it() {
        let mut options = Options::default();
        options.set("prefix", "^a").expect("^a is a key");
        assert_eq!(
            options.get("prefix"),
            Some("C-a"),
            "a caret spelling must be stored as the keymap's own",
        );
    }

    #[test]
    fn a_choice_is_matched_case_insensitively_and_stored_lowercase() {
        let mut options = Options::default();
        options
            .set("detach-on-destroy", "  Off ")
            .expect("Off is a choice");
        assert_eq!(options.get("detach-on-destroy"), Some("off"));
    }

    #[test]
    fn an_unknown_option_is_refused_with_the_list() {
        let mut options = Options::default();
        let error = options
            .set("prefixx", "C-a")
            .expect_err("prefixx is not an option");
        assert_eq!(error, OptionError::Unknown("prefixx".to_owned()));
        let message = error.to_string();
        assert!(
            message.contains("prefix") && message.contains("detach-on-destroy"),
            "the report must list the real options, got {message:?}",
        );
    }

    #[test]
    fn a_bad_value_names_the_option_and_the_alternatives() {
        let mut options = Options::default();
        let error = options
            .set("detach-on-destroy", "maybe")
            .expect_err("maybe is not a policy");
        let message = error.to_string();
        assert!(
            message.starts_with("detach-on-destroy: ") && message.contains("no-detached"),
            "got {message:?}",
        );
        assert_eq!(
            options.get("detach-on-destroy"),
            Some("on"),
            "a refused set must leave the previous value in force",
        );
    }
    #[test]
    fn a_number_is_stored_as_the_parse_renders_it() {
        let mut options = Options::default();
        options.set(GUI_FONT, "  028 ").expect("028 is a number");
        assert_eq!(
            options.get(GUI_FONT),
            Some("28"),
            "padding and leading zeroes are the user's spelling, not the value",
        );
        assert_eq!(options.number(GUI_FONT), Some(28));
    }

    #[test]
    fn a_number_option_refuses_zero_and_anything_that_is_not_one() {
        // `gui-font` floors at 1, so zero is refused here along with the non-numbers — and the
        // complaint NAMES that floor rather than saying "above zero" for every option, now that
        // `history-limit` sits in the same kind with a floor of 0.
        let mut options = Options::default();
        for refused in ["0", "-4", "huge", "", "1.5", "4px"] {
            let error = options
                .set(GUI_FONT, refused)
                .expect_err("{refused} is not a size");
            assert!(
                error.to_string().contains("1 or above"),
                "{refused:?} must be refused as a number: {error}",
            );
        }
        assert_eq!(
            options.number(GUI_FONT),
            Some(20),
            "and none of them displaced the default",
        );
    }

    #[test]
    fn only_a_number_option_answers_as_a_number() {
        // A caller asking the wrong KIND for a number gets `None` rather than a parse of a key spec.
        let options = Options::default();
        assert_eq!(options.number(PREFIX), None);
        assert_eq!(options.number(DETACH_ON_DESTROY), None);
        assert_eq!(options.number("not-an-option"), None);
    }

    #[test]
    fn the_window_size_vocabulary_is_exactly_the_policy_set() {
        // `WINDOW_SIZE_VALUES` names its three entries by hand (an array literal cannot fold over
        // `ALL`), so this is the guard that keeps the two from drifting: a policy added to the enum
        // and not offered here would be unreachable from the file, and a name offered here that no
        // policy answers to would be a value `set-option` accepts and nothing performs.
        let from_policy: Vec<&str> = WindowSize::ALL.iter().map(|policy| policy.name()).collect();
        assert_eq!(WINDOW_SIZE_VALUES, from_policy.as_slice());
        // And every offered value survives the option's own canonicalisation back into a policy —
        // the round trip a user's file actually takes.
        for value in WINDOW_SIZE_VALUES {
            let stored = OptionKind::Choice(WINDOW_SIZE_VALUES)
                .canonicalise(value)
                .expect("an offered value is acceptable");
            assert!(
                WindowSize::parse(&stored).is_some(),
                "{value:?} is offered but does not parse to a policy"
            );
        }
    }
}
