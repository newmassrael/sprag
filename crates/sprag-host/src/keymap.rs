//! The user's KEYMAP: which keystroke a client treats as its own, and what it means.
//!
//! A multiplexer's keys are contested. Once keystrokes reach a pane's child every key is spoken for
//! — `q` is a program's quit, `Ctrl-C` its interrupt — so a client's own commands live behind a
//! PREFIX, and which prefix, and which command keys follow it, are the user's to decide. Until this
//! module they were a `match` inside `sprag-tui`'s binary, and that binary's own docs said so.
//!
//! ## What a binding binds TO
//!
//! A [`BoundAction`] is TYPED and is parsed from the same string the shell takes: `split-window
//! -h`, `detach-client`. One vocabulary rather than three (a CLI verb, a tmux verb, and a
//! per-frontend enum), the spelling a tmux user already has, and — because the parse happens when
//! the config is READ — a mistyped action is an error that names itself rather than a key that
//! silently does nothing.
//!
//! A binding names no pane, and that is the point. `sprag split-window -h PANE` needs the pane
//! spelled out because the DAEMON has no current pane; a client does — its focus — so a binding acts
//! there. This is the one direction the binding vocabulary is RICHER than the CLI's, not poorer.
//!
//! ## How a key is spelled
//!
//! tmux's modifier prefixes, verbatim from its manual: `C-` (or `^`) for Ctrl, `S-` for Shift, `M-`
//! for Alt/Meta. `Super-` is sprag's own, because tmux has no fourth modifier and
//! [`Modifiers`] does.
//!
//! The KEY itself is spelled the way sprag's wire spells it — [`sprag_input::NAMED_KEYS`], i.e.
//! `ArrowUp` and `Backspace`, not tmux's `Up` and `BSpace`. Adopting tmux's names would mean a
//! second key vocabulary plus a mapping table between the two, and the divergence is invisible for
//! every default binding because all of them are plain characters.
//!
//! ## Two tables: behind the prefix, and in front of it
//!
//! [`KeyTable::Prefix`] holds the keys that mean something AFTER the prefix — tmux's default table
//! and where all of sprag's own defaults are. [`KeyTable::Root`] holds keys that act with no prefix
//! at all (tmux's `-n`), which therefore never reach the pane. One key can be in both and mean two
//! different things, which is what makes the table half of a binding's identity rather than a
//! property of one.
//!
//! ## The ROUTING is here too, because both frontends must agree
//!
//! [`Keymap::route`] is the whole state machine: whether a keystroke is the prefix, which table it is
//! looked up in, and — through [`Routed::next`] — when the mode ends. It lives beside the table
//! rather than in either client, because the two clients decode keys differently (termwiz events in
//! the terminal, pinion key names in the GUI) and agree about everything after that. A second
//! implementation would be two answers to "what does this user's table say".
//!
//! Its ORDER is measured against `tmux 3.2a` rather than reasoned out, and one step of it is
//! surprising: the prefix is checked BEFORE the root table, so a root binding on the prefix key does
//! not fire. [`Keymap::route`]'s own doc records the probes.
//!
//! ## Repeat is a deadline, not a timer
//!
//! A [`Bind::repeats`] binding (tmux's `-r`) leaves the prefix table armed for
//! [`Keymap::repeat_time`] instead of one keystroke, so `prefix o o o` runs the binding three times.
//! Nothing in sprag observes the moment that window closes — there is no key-table indicator to
//! repaint — so the deadline is simply compared against the next keystroke's arrival, and no client
//! grows a timer, a thread or a tick for it. [`Keymap::route`] takes the instant as a parameter,
//! which is also what lets a test assert a window without sleeping through one.
//!
//! What each client keeps for itself is PERFORMING an action, which has nothing in common between
//! them: a split is a wire request in one and the same request through a `SlotView` in the other, and
//! `detach-client` is a loop `break` in one and a quit sink in the other.
//!
//! ## Defaults are a keymap, not a fallback
//!
//! [`Keymap::default`] IS tmux's table (verified against `tmux 3.2a`'s own `list-keys -T prefix`).
//! A config file LAYERS over it — [`Keymap::bind`] then, where asked, [`Keymap::unbind`] — so a user
//! who wants one extra binding does not have to re-declare the four they already had.

use std::fmt;
use std::time::{Duration, Instant};

use sprag_input::Modifiers;
use sprag_terminal::SplitDir;

/// tmux's own `repeat-time` default, and the one sprag takes when the options table is silent.
///
/// Read from `tmux 3.2a`'s `show-options -g repeat-time` on this machine rather than recalled. It
/// lives here rather than only in [`crate::options`] because [`Keymap::default`] has to answer
/// without a config file at all — `sprag list-keys` runs on a machine with no daemon and no
/// `config.toml`.
pub const DEFAULT_REPEAT_TIME: Duration = Duration::from_millis(500);

/// Why a keymap declaration could not be used.
///
/// Typed rather than a message string because two of these are reported by a CLI verb as well as by
/// the config reader, and a caller that has to match on the text of an error message is a caller
/// that will one day match on a typo.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeyError {
    /// The spec named no key at all, or named one nothing in sprag's vocabulary can produce.
    UnknownKey(String),
    /// The action's verb is not one any client has.
    UnknownAction(String),
    /// The verb is known, but these flags are not usable in a binding.
    BadFlags {
        /// The action as the user wrote it, so the report can quote it back.
        action: String,
        /// What is wrong with it, in the user's terms.
        why: String,
    },
    /// One key is both bound and unbound by the same file, in the same table.
    BoundAndUnbound(String),
    /// The name is not one of sprag's key tables.
    UnknownTable(String),
    /// A root-table binding asked to repeat, which cannot mean anything.
    RepeatInRoot(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(spec) => write!(
                f,
                "{spec:?} is not a key (modifiers are C-, ^, S-, M-, Super-; \
                 a key is one character or a name like Enter, Escape, ArrowUp, F5)"
            ),
            Self::UnknownAction(verb) => write!(
                f,
                "{verb:?} is not an action (there are: detach-client, send-prefix, \
                 split-window -h|-v [-b], select-pane -t :.+)"
            ),
            Self::BadFlags { action, why } => write!(f, "{action:?}: {why}"),
            Self::BoundAndUnbound(key) => {
                write!(f, "{key} is both bound and unbound; say only one")
            }
            Self::UnknownTable(name) => write!(
                f,
                "{name:?} is not a key table (there are: {:?}, {:?})",
                KeyTable::Prefix.as_str(),
                KeyTable::Root.as_str()
            ),
            // Names the MECHANISM rather than the rule, because the rule on its own reads as an
            // arbitrary refusal: repeat is a window during which the PREFIX table stays armed, and a
            // root binding is reached without the prefix, so there is nothing for it to hold open.
            // tmux 3.2a accepts this combination and it does nothing there — measured, and refused
            // here for the reason every other silent declaration in this file is refused.
            Self::RepeatInRoot(key) => write!(
                f,
                "{key} cannot repeat: repeat holds the {:?} table open for the next keystroke, \
                 and a {:?} binding is reached without the prefix",
                KeyTable::Prefix.as_str(),
                KeyTable::Root.as_str()
            ),
        }
    }
}

/// Which of sprag's two key tables a binding lives in — tmux's `-T`.
///
/// # Why an enum and not a string
///
/// A table name reaches this module from three places (the CLI's `-T`/`-n`, the config file's
/// `table = …`, and [`Keymap`]'s own defaults) and every one of them has to reject an unknown name
/// rather than create a table nobody consults. Parsing once, into a closed type, is what makes the
/// refusal impossible to forget at the fourth call site.
///
/// There are exactly two because a THIRD needs a way to switch to it: tmux's custom tables are
/// reachable only through `switch-client -T`, which is a bound action that changes the client's
/// mode. That is a different mechanism, and refusing an unknown name by name is what keeps it open.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KeyTable {
    /// Keys pressed AFTER the prefix — the default, and where every one of sprag's own defaults is.
    #[default]
    Prefix,
    /// Keys pressed WITHOUT the prefix, which therefore never reach the pane — tmux's `-n`.
    Root,
}

impl KeyTable {
    /// tmux's own name for this table, which is the spelling the CLI, the config file and
    /// `list-keys` all use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Root => "root",
        }
    }

    /// Parse tmux's table name.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownTable`] for anything else — never a silent fallback to
    /// [`KeyTable::Prefix`], which would take a binding a user aimed at the root table and quietly
    /// put it behind the prefix.
    pub fn parse(name: &str) -> Result<Self, KeyError> {
        match name {
            "prefix" => Ok(Self::Prefix),
            "root" => Ok(Self::Root),
            other => Err(KeyError::UnknownTable(other.to_owned())),
        }
    }
}

impl fmt::Display for KeyTable {
    /// Through [`pad`](fmt::Formatter::pad), not `write_str`, so `{:6}` actually pads.
    ///
    /// `list-keys` aligns its columns with a width, and a `Display` that writes the string directly
    /// ignores every formatting flag it is given — which put the key column two characters left on
    /// every `root` line and only there. Found by READING the output, not by reading this function.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A keystroke as sprag's wire spells one: a key name plus the four modifiers held with it.
///
/// The name is stored rather than borrowed because a keymap outlives the config text it was read
/// from, and a keymap is a handful of entries read once — the allocation is not on any path a
/// keystroke takes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeySpec {
    /// The key's name in the wire's vocabulary — a single character, or one of
    /// [`sprag_input::NAMED_KEYS`].
    name: String,
    /// The modifiers held with it. Matched EXACTLY: see [`KeySpec::matches`].
    mods: Modifiers,
}

/// The modifier prefixes a key spec may carry, longest-unambiguous first.
///
/// `^` has no dash, which is tmux's own spelling and the reason the stripper below has to check that
/// something FOLLOWS a prefix before treating it as one: `"^"` on its own is the caret key.
const MOD_PREFIXES: &[(&str, Modifier)] = &[
    ("C-", Modifier::Ctrl),
    ("^", Modifier::Ctrl),
    ("S-", Modifier::Shift),
    ("M-", Modifier::Alt),
    ("Super-", Modifier::Super),
];

/// Which modifier a prefix sets — an index into [`Modifiers`]'s four flags, so the stripper can name
/// what it found without four near-identical branches at the point it finds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

impl KeySpec {
    /// Parse a key spec — `C-b`, `%`, `S-Tab`, `M-ArrowUp`, `F5`.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownKey`] when what is left after the modifiers is not a key sprag can name.
    /// Refusing here rather than accepting a name nothing produces is the whole reason the
    /// vocabulary is a public list: a binding that can never fire is exactly the silent failure the
    /// config file's `deny_unknown_fields` already exists to prevent.
    pub fn parse(spec: &str) -> Result<Self, KeyError> {
        let mut rest = spec;
        let mut mods = Modifiers::default();
        // A prefix is only a prefix when a key follows it. `"^"` alone is the caret character and
        // `"C-"` alone names nothing, so both fall through to the vocabulary check below — which
        // accepts the first and refuses the second, each for the right reason.
        while let Some((modifier, tail)) = MOD_PREFIXES.iter().find_map(|(text, modifier)| {
            let tail = rest.strip_prefix(text)?;
            (!tail.is_empty()).then_some((*modifier, tail))
        }) {
            match modifier {
                Modifier::Ctrl => mods.ctrl = true,
                Modifier::Alt => mods.alt = true,
                Modifier::Shift => mods.shift = true,
                Modifier::Super => mods.sup = true,
            }
            rest = tail;
        }
        if !sprag_input::is_key_name(rest) {
            return Err(KeyError::UnknownKey(spec.to_owned()));
        }
        Ok(Self {
            name: rest.to_owned(),
            mods,
        })
    }

    /// This key's name in the wire's vocabulary — what
    /// [`HostClient::send_key`](crate::HostClient::send_key) takes.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The modifiers held with it.
    #[must_use]
    pub fn mods(&self) -> Modifiers {
        self.mods
    }

    /// Whether a keystroke a client just decoded is this one.
    ///
    /// **Modifiers match exactly, and that is what replaces a special case.** The old hardcoded
    /// table needed a rule of its own — "a command key with a modifier on it is a slip" — so that
    /// `Ctrl-D` could not detach and `Ctrl-O` could not move focus. Exact matching makes that rule a
    /// consequence: `Ctrl-D` is simply not the key `d` is bound to. It also makes `C-o` BINDABLE,
    /// which the special case could not express at all.
    ///
    /// Case is the one thing not compared exactly, and only for a single ASCII letter. A terminal
    /// sends `Ctrl-B` as the C0 byte `0x02`, which decodes as lowercase, while a terminal using the
    /// `CSI u` encoding reports whichever case the layout produced — two spellings of one keystroke.
    /// Shift is not affected: it is a MODIFIER here, so a bound `d` still does not match `Shift-D`.
    #[must_use]
    pub fn matches(&self, name: &str, mods: Modifiers) -> bool {
        self.mods == mods && same_key(&self.name, name)
    }
}

/// Whether two key names are the same key, comparing a lone ASCII letter case-insensitively.
fn same_key(spec: &str, typed: &str) -> bool {
    if is_ascii_letter(spec) && is_ascii_letter(typed) {
        return spec.eq_ignore_ascii_case(typed);
    }
    spec == typed
}

/// Whether `name` is exactly one ASCII letter — the only names whose case is a terminal's choice
/// rather than the user's.
fn is_ascii_letter(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next().is_none()
}

impl fmt::Display for KeySpec {
    /// The canonical spelling, which is what `list-keys` prints and what a round trip through
    /// [`KeySpec::parse`] reproduces. `C-` rather than `^`, so one keystroke has one written form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, text) in [
            (self.mods.ctrl, "C-"),
            (self.mods.alt, "M-"),
            (self.mods.shift, "S-"),
            (self.mods.sup, "Super-"),
        ] {
            if held {
                f.write_str(text)?;
            }
        }
        f.write_str(&self.name)
    }
}

/// What a bound key does.
///
/// Every variant is an action a CLIENT can carry out on its own focus. Nothing here needs the
/// daemon to have a current pane, which is what keeps the vocabulary honest: an action sprag could
/// not perform would be a binding that parsed and then did nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoundAction {
    /// `detach-client` — give the terminal back and leave the session running.
    DetachClient,
    /// `send-prefix` — type the PREFIX key into the pane, which is what keeps the prefix reachable
    /// by a program that binds it (readline's backward-char, for one).
    ///
    /// The prefix, not the key that was pressed: a user who rebinds this to `a` means `prefix a` to
    /// send `C-b`, not to send `a`.
    SendPrefix,
    /// `split-window -h|-v [-b]` — divide the focused pane and put a new shell in the half it opens.
    SplitWindow {
        /// tmux's `-h` lays the panes side by SIDE, which is [`SplitDir::Horizontal`]. The flag
        /// names the LAYOUT, not the line drawn between them.
        dir: SplitDir,
        /// tmux's `-b`: put the new pane on the near side (left of, or above) instead of the far one.
        before: bool,
    },
    /// `select-pane -t :.+` — move focus to the next pane in paint order.
    SelectNextPane,
}

/// tmux's spelling of "the next pane", which is the only target form a binding may carry today.
///
/// The rest of tmux's target grammar (`-t :=2`, `-t {left-of}`, session/window addressing) is H5's,
/// and accepting a fragment of it here would promise a grammar sprag has not built.
const NEXT_PANE_TARGET: [&str; 2] = ["-t", ":.+"];

impl BoundAction {
    /// Parse an action as the shell spells it — `split-window -h`, `detach-client`.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownAction`] for a verb no client has, [`KeyError::BadFlags`] for a known verb
    /// a binding cannot carry out as written.
    pub fn parse(action: &str) -> Result<Self, KeyError> {
        let mut words = action.split_whitespace();
        let verb = words
            .next()
            .ok_or_else(|| KeyError::UnknownAction(action.to_owned()))?;
        let flags: Vec<&str> = words.collect();
        let bad = |why: &str| KeyError::BadFlags {
            action: action.to_owned(),
            why: why.to_owned(),
        };
        match verb {
            "detach-client" | "send-prefix" => {
                if !flags.is_empty() {
                    return Err(bad("takes no arguments"));
                }
                Ok(if verb == "detach-client" {
                    Self::DetachClient
                } else {
                    Self::SendPrefix
                })
            }
            "split-window" => {
                let mut dir = None;
                let mut before = false;
                for flag in flags {
                    match flag {
                        "-h" | "-v" => {
                            if dir.is_some() {
                                return Err(bad("-h and -v name one axis; give only one"));
                            }
                            dir = Some(if flag == "-h" {
                                SplitDir::Horizontal
                            } else {
                                SplitDir::Vertical
                            });
                        }
                        "-b" => before = true,
                        // A pane id is the one argument the CLI verb needs and a binding must not
                        // have: a keystroke acts where the user is.
                        other => {
                            return Err(bad(&format!(
                                "{other:?} is not a flag a binding takes (a binding splits the \
                                 FOCUSED pane, so it names none)"
                            )));
                        }
                    }
                }
                // Bare `split-window` is refused rather than defaulted. sprag's CLI reads the bare
                // form as the DIRECTION-LESS append (its daemon has no current pane to be relative
                // to), and a client's split always has a direction — so one string would mean two
                // things. tmux's own `"` binding is bare; sprag's default keymap spells it `-v`,
                // which is what tmux's bare form MEANS.
                let dir = dir.ok_or_else(|| {
                    bad("needs -h (side by side) or -v (stacked); sprag's bare split-window is the direction-less append, which a client cannot do")
                })?;
                Ok(Self::SplitWindow { dir, before })
            }
            "select-pane" => {
                if flags == NEXT_PANE_TARGET {
                    Ok(Self::SelectNextPane)
                } else {
                    Err(bad(
                        "the only target a binding takes is `-t :.+` (the next pane); \
                         the rest of tmux's target grammar is not built",
                    ))
                }
            }
            _ => Err(KeyError::UnknownAction(verb.to_owned())),
        }
    }
}

impl fmt::Display for BoundAction {
    /// The canonical spelling — what `list-keys` prints, and what parses back to this action.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DetachClient => f.write_str("detach-client"),
            Self::SendPrefix => f.write_str("send-prefix"),
            Self::SplitWindow { dir, before } => {
                f.write_str(match dir {
                    SplitDir::Horizontal => "split-window -h",
                    SplitDir::Vertical => "split-window -v",
                })?;
                if *before {
                    f.write_str(" -b")?;
                }
                Ok(())
            }
            Self::SelectNextPane => write!(
                f,
                "select-pane {} {}",
                NEXT_PANE_TARGET[0], NEXT_PANE_TARGET[1]
            ),
        }
    }
}

/// One binding: a key, in a table, meaning an action — and whether it repeats.
///
/// A record rather than the `(KeySpec, BoundAction)` pair this used to be, because with a table and
/// a repeat flag the tuple's positions stop saying what they are. Every field is read together at the
/// one place a keystroke is routed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bind {
    /// Which table the key has to be pressed in to mean this.
    table: KeyTable,
    /// The keystroke.
    key: KeySpec,
    /// What it does.
    action: BoundAction,
    /// tmux's `-r`: after this acts, the prefix table stays armed for
    /// [`repeat_time`](Keymap::repeat_time) rather than one keystroke.
    repeat: bool,
}

impl Bind {
    /// Which table this binding is in.
    #[must_use]
    pub fn table(&self) -> KeyTable {
        self.table
    }

    /// The keystroke it is on.
    #[must_use]
    pub fn key(&self) -> &KeySpec {
        &self.key
    }

    /// What it does.
    #[must_use]
    pub fn action(&self) -> BoundAction {
        self.action
    }

    /// Whether it repeats — tmux's `-r`.
    #[must_use]
    pub fn repeats(&self) -> bool {
        self.repeat
    }
}

/// A client's prefix key and the tables of commands it routes.
///
/// Ordered rather than hashed: a keymap holds a handful of entries and is read once per keystroke,
/// so a linear scan is not a cost — and the ORDER is load-bearing for `list-keys`, which has to show
/// a user the table they wrote.
///
/// ONE list for both tables rather than one per table, because the file's `[[bind]]` is one array
/// and `list-keys` shows a user their own declaration order: two lists would need a merge that has to
/// reconstruct it. The [`table`](Bind::table) field is what a lookup filters on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Keymap {
    /// The key that says "the next keystroke is mine".
    prefix: KeySpec,
    /// How long a [`repeat`](Bind::repeats) binding holds the prefix table open — tmux's
    /// `repeat-time`, and like [`prefix`](Self::prefix) it is built FROM the options table rather
    /// than declared beside it.
    repeat_time: Duration,
    /// Every binding, in declaration order, across both tables.
    binds: Vec<Bind>,
}

impl Default for Keymap {
    /// tmux's own defaults, for the actions sprag's clients have.
    ///
    /// Verified against `tmux 3.2a`'s `list-keys -T prefix` on this machine rather than recalled:
    /// `C-b send-prefix`, `" split-window`, `% split-window -h`, `d detach-client`,
    /// `o select-pane -t :.+`. The one divergence is `"`, spelled `-v` here for the reason
    /// [`BoundAction::parse`] gives.
    fn default() -> Self {
        let key = |spec: &str| KeySpec::parse(spec).expect("a default key spec is well formed");
        let bind = |spec: &str, action| Bind {
            table: KeyTable::Prefix,
            key: key(spec),
            action,
            repeat: false,
        };
        Self {
            prefix: key("C-b"),
            repeat_time: DEFAULT_REPEAT_TIME,
            // The ROOT table ships empty, which is also tmux's state for keyboard keys: its own
            // default root table holds mouse bindings only (measured from `list-keys -T root`).
            // And no default REPEATS: tmux's `-r` defaults sit on `select-pane -U/-D/-L/-R` and
            // `resize-pane`, directional actions this vocabulary does not have, so putting `-r` on
            // `o` would be sprag inventing a default tmux never had.
            binds: vec![
                bind("C-b", BoundAction::SendPrefix),
                bind(
                    "\"",
                    BoundAction::SplitWindow {
                        dir: SplitDir::Vertical,
                        before: false,
                    },
                ),
                bind(
                    "%",
                    BoundAction::SplitWindow {
                        dir: SplitDir::Horizontal,
                        before: false,
                    },
                ),
                bind("d", BoundAction::DetachClient),
                bind("o", BoundAction::SelectNextPane),
            ],
        }
    }
}

impl Keymap {
    /// The prefix key.
    #[must_use]
    pub fn prefix(&self) -> &KeySpec {
        &self.prefix
    }

    /// Replace the prefix — and move the self-send with it.
    ///
    /// # Why a binding follows this
    ///
    /// [`BoundAction::SendPrefix`] on the prefix key is not an independent choice: it exists so the
    /// prefix stays reachable by the program in the pane, which is a statement about WHICHEVER key
    /// the prefix is. Leaving it on the old key is what tmux does, and it is why every tmux user who
    /// rebinds their prefix has to remember `bind C-a send-prefix` as a second step — sprag models
    /// the prefix as a field rather than a server option, so it can simply be right.
    ///
    /// Narrow on purpose: ONLY a `send-prefix` sitting on the old prefix moves. A user's own binding
    /// on that key is their choice about that key and stays where they put it. If the new prefix key
    /// already meant something, the self-send takes it over — one key means one thing, the rule
    /// [`Keymap::bind`] already applies — and a later `[[bind]]` can still override it, because a
    /// config's `[options]` table is read before its bindings.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownKey`] for a spec that names no key.
    pub fn set_prefix(&mut self, spec: &str) -> Result<(), KeyError> {
        let next = KeySpec::parse(spec)?;
        let previous = std::mem::replace(&mut self.prefix, next.clone());
        if previous == next {
            return Ok(());
        }
        // Scoped to the PREFIX table on both sides. A `send-prefix` a user put in the ROOT table is
        // a key that sends the prefix without the prefix, which is a statement about THAT key rather
        // than about whichever key the prefix currently is — so it does not follow, and it must not
        // be the thing displaced either.
        let moves = |bind: &Bind| {
            bind.table == KeyTable::Prefix
                && bind.key == previous
                && bind.action == BoundAction::SendPrefix
        };
        if self.binds.iter().any(moves) {
            self.binds
                .retain(|bind| bind.table != KeyTable::Prefix || bind.key != next);
            if let Some(slot) = self.binds.iter_mut().find(|bind| moves(bind)) {
                // Retargeted in place rather than removed and re-pushed, so `list-keys` shows it
                // where the user's file put it instead of at the end.
                slot.key = next;
            }
        }
        Ok(())
    }

    /// How long a [`repeat`](Bind::repeats) binding holds the prefix table open.
    #[must_use]
    pub fn repeat_time(&self) -> Duration {
        self.repeat_time
    }

    /// Set the repeat window — tmux's `repeat-time`, in milliseconds.
    ///
    /// Zero is a DECISION rather than an absence, and it is the one tmux takes too (`repeat-time 0`
    /// is accepted there): a window that has already closed when it opens, so a `-r` binding acts
    /// exactly once. Nothing here has to special-case it — [`PrefixMode::armed`] compares an instant
    /// that is already in the past.
    pub fn set_repeat_time(&mut self, millis: u64) {
        self.repeat_time = Duration::from_millis(millis);
    }

    /// Bind `key` to `action`, replacing whatever it meant before — tmux's `bind-key`.
    ///
    /// Replacing IN PLACE rather than appending keeps the order a user wrote: rebinding `%` leaves
    /// it where it was in their file instead of moving it to the end of `list-keys`.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownKey`] / [`KeyError::UnknownAction`] / [`KeyError::BadFlags`].
    pub fn bind(
        &mut self,
        table: KeyTable,
        key: &str,
        action: &str,
        repeat: bool,
    ) -> Result<(), KeyError> {
        let key = KeySpec::parse(key)?;
        let action = BoundAction::parse(action)?;
        if repeat && table == KeyTable::Root {
            return Err(KeyError::RepeatInRoot(key.to_string()));
        }
        match self
            .binds
            .iter_mut()
            .find(|bound| bound.table == table && bound.key == key)
        {
            Some(slot) => {
                slot.action = action;
                slot.repeat = repeat;
            }
            None => self.binds.push(Bind {
                table,
                key,
                action,
                repeat,
            }),
        }
        Ok(())
    }

    /// Remove whatever `key` was bound to — tmux's `unbind-key`.
    ///
    /// IDEMPOTENT: unbinding a key that was not bound is not an error. The post-state is exactly
    /// what was asked for, and a config that breaks because a later sprag stopped shipping a default
    /// would be punishing the user for defending against it.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownKey`] for a spec that names no key — a typo is still a typo.
    pub fn unbind(&mut self, table: KeyTable, key: &str) -> Result<(), KeyError> {
        let key = KeySpec::parse(key)?;
        self.binds
            .retain(|bound| bound.table != table || bound.key != key);
        Ok(())
    }

    /// Whether a keystroke a client just decoded is the prefix.
    #[must_use]
    pub fn is_prefix(&self, name: &str, mods: Modifiers) -> bool {
        self.prefix.matches(name, mods)
    }

    /// What a keystroke means in `table`, or [`None`] for a key that is not bound there.
    ///
    /// A key can be bound in BOTH tables and mean two different things — tmux allows exactly that
    /// (measured: `bind -n C-b` sits beside `bind C-b send-prefix`) — so the table is not a filter
    /// applied to one answer, it is part of the question.
    #[must_use]
    pub fn action(&self, table: KeyTable, name: &str, mods: Modifiers) -> Option<BoundAction> {
        self.bound(table, name, mods).map(|bind| bind.action)
    }

    /// The binding `name`+`mods` has in `table`, whole — the lookup [`route`](Self::route) uses,
    /// which needs the repeat flag as well as the action.
    fn bound(&self, table: KeyTable, name: &str, mods: Modifiers) -> Option<&Bind> {
        self.binds
            .iter()
            .find(|bind| bind.table == table && bind.key.matches(name, mods))
    }

    /// Every binding, in the order a user would read them, across both tables.
    pub fn binds(&self) -> impl Iterator<Item = &Bind> {
        self.binds.iter()
    }

    /// Route one keystroke, given the mode the PREVIOUS one left behind.
    ///
    /// `name` and `mods` are the wire's spelling of the key — [`sprag_input::NAMED_KEYS`] or a
    /// single character — which is what both frontends already have by the time they ask: a
    /// keystroke a client cannot name is not one a binding could have named either.
    ///
    /// An unbound key AFTER the prefix is [`Routed::Swallow`] rather than passed through, which is
    /// tmux's behaviour and the safer of the two: a user who typed the prefix meant to address the
    /// client, so delivering their mistake to a shell would run something they did not ask for.
    ///
    /// # The order, and why it is this one
    ///
    /// Every step below was MEASURED against `tmux 3.2a` driving a real client on a pty, because two
    /// of them are runtime behaviour its manual does not state:
    ///
    /// 1. **Armed** — after the prefix, or inside a repeat window — look in [`KeyTable::Prefix`].
    /// 2. **The prefix itself**, BEFORE the root table. A root binding on the prefix key does not
    ///    fire in tmux: `bind -n C-b …` leaves `C-b` arming the prefix, with a root binding on
    ///    another key firing normally as the control. The natural implementation looks the root
    ///    table up first and gets this backwards.
    /// 3. **[`KeyTable::Root`]** — a binding with no prefix, which therefore takes the key from the
    ///    pane. That is the whole of what `-n` means.
    /// 4. Otherwise the pane.
    ///
    /// An unbound key inside a REPEAT window falls through to 2-4 instead of being swallowed —
    /// measured (typing `ZQ` inside the window puts `ZQ` in the shell). That asymmetry with the
    /// one-key mode is deliberate in tmux and is why [`PrefixMode`] has three states rather than a
    /// deadline hung off the second.
    ///
    /// `now` is a parameter rather than a call inside here so a repeat window can be tested by
    /// passing an instant instead of sleeping through one.
    #[must_use]
    pub fn route(&self, mode: PrefixMode, now: Instant, name: &str, mods: Modifiers) -> Routed {
        if mode.armed(now) {
            if let Some(bind) = self.bound(KeyTable::Prefix, name, mods) {
                return self.act(bind, now);
            }
            if mode == PrefixMode::AfterPrefix {
                return Routed::Swallow;
            }
        }
        if self.is_prefix(name, mods) {
            return Routed::Prefix;
        }
        if let Some(bind) = self.bound(KeyTable::Root, name, mods) {
            return self.act(bind, now);
        }
        Routed::ToPane
    }

    /// Carry out `bind`, opening a repeat window from `now` if it asked for one.
    ///
    /// The window is measured from THIS keystroke rather than from the first of a run, so every
    /// repeat re-arms it — measured against tmux, where three presses at 0/400/800 ms under a 500 ms
    /// `repeat-time` all reach the binding and none reaches the pane.
    fn act(&self, bind: &Bind, now: Instant) -> Routed {
        Routed::Act {
            action: bind.action,
            again: bind.repeat.then(|| now + self.repeat_time),
        }
    }
}

/// Where the next keystroke goes.
///
/// States rather than a `bool` because the prefix is not a modifier — it is a mode the client
/// enters and leaves, and `after_prefix: true` at a call site says nothing about which way round
/// that is.
///
/// [`Repeating`](Self::Repeating) is a third state and not a deadline attached to
/// [`AfterPrefix`](Self::AfterPrefix), because the two differ in what an UNBOUND key does: after the
/// prefix it is swallowed, inside a repeat window it falls through to the pane. That is tmux's
/// behaviour, measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PrefixMode {
    /// The steady state: keys are the program's.
    #[default]
    ToPane,
    /// The prefix was just pressed, so the next key is a command to this client.
    AfterPrefix,
    /// A [`repeat`](Bind::repeats) binding just acted, so the prefix table stays armed until
    /// `until` without the prefix being pressed again — tmux's `-r` plus `repeat-time`.
    Repeating {
        /// When the window closes. Compared against a passed-in instant on the next keystroke, which
        /// is the ONLY thing that ever observes it: sprag paints no key-table indicator, so nothing
        /// happens at the moment a window expires and a timer would have nothing to wake for.
        until: Instant,
    },
}

impl PrefixMode {
    /// Whether the prefix table is live for a keystroke arriving at `now`.
    #[must_use]
    pub fn armed(self, now: Instant) -> bool {
        match self {
            Self::ToPane => false,
            Self::AfterPrefix => true,
            Self::Repeating { until } => now <= until,
        }
    }
}

/// What a client should do with a keystroke — the answer [`Keymap::route`] gives.
///
/// Carries no key: a caller that asked about a keystroke still has it, and threading it back out
/// would force one representation on two frontends that decode differently (termwiz `KeyEvent` in
/// the terminal client, a pinion key name in the GUI).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Routed {
    /// Send it to the focused pane — the steady state.
    ToPane,
    /// It was the PREFIX: swallow it, and the NEXT keystroke is a command to this client.
    Prefix,
    /// Carry out a bound command of the client's own.
    Act {
        /// What to do.
        action: BoundAction,
        /// When the repeat window this opens closes, or [`None`] for a binding that does not repeat.
        ///
        /// Carried OUT of the routing rather than computed by the caller so [`Routed::next`] stays
        /// the one definition of the next mode: a frontend that added `now + repeat_time` itself
        /// would be a second author of that rule, and there are two frontends.
        again: Option<Instant>,
    },
    /// Nothing at all — an unbound key after the prefix.
    Swallow,
}

impl Routed {
    /// Where the NEXT keystroke goes after this one.
    ///
    /// **The mode is one key long unless a binding asked otherwise, and this is the one definition
    /// of that.** Total on purpose: a client that derives its next mode from here cannot arm the
    /// mode by accident, and — because every outcome but [`Routed::Prefix`] and a repeating
    /// [`Routed::Act`] answers [`PrefixMode::ToPane`] — cannot forget to disarm it either. Both
    /// frontends have more ways to leave a keystroke than to route one (the GUI has five surfaces
    /// that consume a key before the pane is even resolved), so the rule has to live somewhere
    /// neither of them can partially implement.
    #[must_use]
    pub fn next(&self) -> PrefixMode {
        match self {
            Self::Prefix => PrefixMode::AfterPrefix,
            Self::Act {
                again: Some(until), ..
            } => PrefixMode::Repeating { until: *until },
            Self::ToPane | Self::Act { again: None, .. } | Self::Swallow => PrefixMode::ToPane,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The modifier prefixes are tmux's, read from its manual: `C-` or `^` for Ctrl, `S-` for
    /// Shift, `M-` for Alt. `Super-` is sprag's own fourth.
    #[test]
    fn the_modifier_prefixes_are_tmuxs() {
        let cases = [
            (
                "C-b",
                "b",
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            ),
            (
                "^b",
                "b",
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            ),
            (
                "S-Tab",
                "Tab",
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            ),
            (
                "M-x",
                "x",
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            (
                "Super-a",
                "a",
                Modifiers {
                    sup: true,
                    ..Modifiers::default()
                },
            ),
            (
                "C-M-a",
                "a",
                Modifiers {
                    ctrl: true,
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            ("d", "d", Modifiers::default()),
        ];
        for (spec, name, mods) in cases {
            let key = KeySpec::parse(spec).unwrap_or_else(|e| panic!("{spec:?}: {e}"));
            assert_eq!((key.name(), key.mods()), (name, mods), "{spec:?}");
        }
    }

    /// **A prefix is only a prefix when a key follows it.** `^` and `-` are keys a user can bind,
    /// and a stripper that took every leading `^` would make the caret unbindable while accepting
    /// `C-`, which names nothing at all.
    ///
    /// REVERT-PROOF for the `!tail.is_empty()` guard: drop it and `"^"` parses as Ctrl-nothing,
    /// which fails the vocabulary check — so the caret key becomes a config error.
    #[test]
    fn a_lone_modifier_character_is_the_key_itself() {
        let caret = KeySpec::parse("^").expect("the caret is a key");
        assert_eq!((caret.name(), caret.mods()), ("^", Modifiers::default()));
        let dash = KeySpec::parse("-").expect("the dash is a key");
        assert_eq!((dash.name(), dash.mods()), ("-", Modifiers::default()));
        let ctrl_dash = KeySpec::parse("C--").expect("Ctrl and the dash key");
        assert_eq!(ctrl_dash.name(), "-");
        assert!(ctrl_dash.mods().ctrl);
        // `C-` names no key, and is refused rather than read as a bare Ctrl.
        assert!(matches!(KeySpec::parse("C-"), Err(KeyError::UnknownKey(_))));
    }

    /// A key name nothing can produce is refused when the config is READ. The alternative — accept
    /// it and let it never match — is the silent failure this whole check exists to prevent.
    #[test]
    fn a_key_name_outside_the_wire_vocabulary_is_refused() {
        // tmux's own spellings, which sprag does not adopt (see the module docs).
        for spec in ["Up", "BSpace", "DC", "Entre", ""] {
            assert!(
                matches!(KeySpec::parse(spec), Err(KeyError::UnknownKey(_))),
                "{spec:?} should be refused",
            );
        }
        for spec in ["ArrowUp", "Backspace", "Delete", "Enter", "F5", "Space"] {
            assert!(KeySpec::parse(spec).is_ok(), "{spec:?} should parse");
        }
    }

    /// Every spec round-trips through its canonical spelling, which is what makes `list-keys`
    /// output something a user can paste back into their config.
    #[test]
    fn a_key_spec_round_trips_through_its_canonical_spelling() {
        for spec in ["C-b", "d", "%", "\"", "S-Tab", "C-M-S-Super-F12", "-"] {
            let key = KeySpec::parse(spec).unwrap_or_else(|e| panic!("{spec:?}: {e}"));
            let printed = key.to_string();
            assert_eq!(
                KeySpec::parse(&printed).as_ref(),
                Ok(&key),
                "{spec:?} printed as {printed:?}",
            );
        }
        // `^` is accepted on input and printed as `C-`: one keystroke, one written form.
        assert_eq!(KeySpec::parse("^b").expect("parses").to_string(), "C-b");
    }

    /// **Modifiers match EXACTLY, which is what replaces the old "a modified command key is a slip"
    /// rule.** `Ctrl-D` is not the key `d` is bound to, so a program's end-of-file survives.
    ///
    /// REVERT-PROOF: compare only the names and `Ctrl-D` detaches — the case the hardcoded table
    /// needed a special rule for.
    #[test]
    fn a_modifier_makes_it_a_different_key() {
        let d = KeySpec::parse("d").expect("parses");
        assert!(d.matches("d", Modifiers::default()));
        assert!(!d.matches(
            "d",
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            }
        ));
        // ...and the converse: a bound `C-o` is not reached by a bare `o`.
        let ctrl_o = KeySpec::parse("C-o").expect("parses");
        assert!(!ctrl_o.matches("o", Modifiers::default()));
    }

    /// A lone ASCII letter compares case-insensitively, because a terminal chooses the case: the C0
    /// byte for `Ctrl-B` decodes lowercase while a `CSI u` terminal reports the layout's case.
    ///
    /// Shift is unaffected — it is a modifier here, so a bound `d` still does not match `Shift-D`.
    #[test]
    fn a_lone_letter_is_case_insensitive_but_shift_still_is_not() {
        let ctrl_b = KeySpec::parse("C-b").expect("parses");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert!(ctrl_b.matches("b", ctrl));
        assert!(ctrl_b.matches("B", ctrl), "one keystroke, two spellings");
        let d = KeySpec::parse("d").expect("parses");
        assert!(!d.matches(
            "D",
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        ));
        // A NAMED key is not folded: `Enter` and `enter` are not two spellings of one key, they are
        // one name and one typo — and the typo was already refused at parse time.
        let enter = KeySpec::parse("Enter").expect("parses");
        assert!(!enter.matches("enter", Modifiers::default()));
    }

    /// The action vocabulary is the shell's own spelling, and every action round-trips.
    #[test]
    fn actions_parse_from_the_shells_spelling_and_round_trip() {
        let cases = [
            ("detach-client", BoundAction::DetachClient),
            ("send-prefix", BoundAction::SendPrefix),
            (
                "split-window -h",
                BoundAction::SplitWindow {
                    dir: SplitDir::Horizontal,
                    before: false,
                },
            ),
            (
                "split-window -v -b",
                BoundAction::SplitWindow {
                    dir: SplitDir::Vertical,
                    before: true,
                },
            ),
            ("select-pane -t :.+", BoundAction::SelectNextPane),
        ];
        for (text, action) in cases {
            assert_eq!(BoundAction::parse(text), Ok(action), "{text:?}");
            assert_eq!(
                action.to_string(),
                text,
                "and prints back as it was written"
            );
        }
    }

    /// **THE INVERSION, pinned in the vocabulary.** tmux's `-h` lays the panes side by SIDE, so it
    /// must become [`SplitDir::Horizontal`] and `-v` must become [`SplitDir::Vertical`].
    ///
    /// Asserted as a PAIR because the failure it guards is the two being SWAPPED, which either
    /// assertion alone lets through: a vocabulary that mapped both to one direction still splits.
    /// R227 recorded exactly this shape of miss on the CLI verb.
    #[test]
    fn the_split_flags_carry_tmuxs_directions_and_not_each_others() {
        assert_eq!(
            BoundAction::parse("split-window -h"),
            Ok(BoundAction::SplitWindow {
                dir: SplitDir::Horizontal,
                before: false
            }),
        );
        assert_eq!(
            BoundAction::parse("split-window -v"),
            Ok(BoundAction::SplitWindow {
                dir: SplitDir::Vertical,
                before: false
            }),
        );
    }

    /// A binding names no pane, and bare `split-window` is refused rather than defaulted — the two
    /// ways one string could quietly come to mean two things.
    #[test]
    fn a_binding_takes_no_pane_and_no_bare_split() {
        for action in ["split-window", "split-window -b", "split-window -h 3"] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { .. })),
                "{action:?} should be refused",
            );
        }
        assert!(matches!(
            BoundAction::parse("split-window -h -v"),
            Err(KeyError::BadFlags { .. })
        ));
    }

    /// An unbuilt target form is refused with what IS built, rather than promising a grammar sprag
    /// does not have.
    #[test]
    fn only_the_next_pane_target_is_accepted() {
        assert_eq!(
            BoundAction::parse("select-pane -t :.+"),
            Ok(BoundAction::SelectNextPane)
        );
        for action in ["select-pane", "select-pane -t :.-", "select-pane -t :=2"] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { .. })),
                "{action:?} should be refused",
            );
        }
    }

    /// A verb no client has is named back to the user with the ones that exist.
    #[test]
    fn an_unknown_verb_is_refused_and_the_report_lists_what_exists() {
        let error = BoundAction::parse("kill-server").expect_err("not a binding action");
        assert_eq!(error, KeyError::UnknownAction("kill-server".to_owned()));
        let message = error.to_string();
        for known in [
            "detach-client",
            "send-prefix",
            "split-window",
            "select-pane",
        ] {
            assert!(
                message.contains(known),
                "{message:?} should mention {known}"
            );
        }
    }

    /// The defaults ARE tmux's table for the actions sprag's clients have.
    #[test]
    fn the_defaults_are_tmuxs_table() {
        let keymap = Keymap::default();
        assert_eq!(keymap.prefix().to_string(), "C-b");
        let printed: Vec<String> = keymap
            .binds()
            .map(|bind| format!("{} {}", bind.key(), bind.action()))
            .collect();
        assert_eq!(
            printed,
            vec![
                "C-b send-prefix",
                "\" split-window -v",
                "% split-window -h",
                "d detach-client",
                "o select-pane -t :.+",
            ],
        );
    }

    /// A file LAYERS over the defaults: one added binding does not erase the other four.
    ///
    /// REVERT-PROOF: build the keymap from an empty table instead and a user who bound one key
    /// would lose `d`, `%`, `"` and `o` without being told.
    #[test]
    fn a_binding_layers_over_the_defaults_rather_than_replacing_them() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "c", "split-window -h", false)
            .expect("binds");
        assert_eq!(
            keymap.action(KeyTable::Prefix, "d", Modifiers::default()),
            Some(BoundAction::DetachClient),
            "the defaults survive",
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "c", Modifiers::default()),
            Some(BoundAction::SplitWindow {
                dir: SplitDir::Horizontal,
                before: false
            }),
        );
    }

    /// Rebinding replaces in place, so `list-keys` shows the key where the user left it.
    #[test]
    fn rebinding_a_key_replaces_it_where_it_was() {
        let mut keymap = Keymap::default();
        let before: Vec<String> = keymap.binds().map(|bind| bind.key().to_string()).collect();
        keymap
            .bind(KeyTable::Prefix, "%", "detach-client", false)
            .expect("binds");
        let after: Vec<String> = keymap.binds().map(|bind| bind.key().to_string()).collect();
        assert_eq!(before, after, "the order is the user's, not the edit's");
        assert_eq!(
            keymap.action(KeyTable::Prefix, "%", Modifiers::default()),
            Some(BoundAction::DetachClient),
        );
    }

    /// Unbinding removes a default and is IDEMPOTENT — but a typo in the key is still refused.
    #[test]
    fn unbinding_removes_a_default_and_repeats_harmlessly() {
        let mut keymap = Keymap::default();
        keymap.unbind(KeyTable::Prefix, "o").expect("unbinds");
        assert_eq!(
            keymap.action(KeyTable::Prefix, "o", Modifiers::default()),
            None
        );
        keymap
            .unbind(KeyTable::Prefix, "o")
            .expect("unbinding twice is not an error");
        assert!(matches!(
            keymap.unbind(KeyTable::Prefix, "Up"),
            Err(KeyError::UnknownKey(_))
        ));
    }

    /// The prefix is the user's, and rebinding it moves the gate.
    #[test]
    fn the_prefix_is_replaceable() {
        let mut keymap = Keymap::default();
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert!(keymap.is_prefix("a", ctrl));
        assert!(!keymap.is_prefix("b", ctrl), "the old prefix is now free");
        assert!(matches!(
            keymap.set_prefix("C-"),
            Err(KeyError::UnknownKey(_))
        ));
    }

    /// **The self-send FOLLOWS the prefix.** Found by running `sprag list-keys` against a real
    /// config rather than by reading this file: with `prefix = "C-a"` the table still said
    /// `C-b send-prefix`, so `prefix prefix` — the only way to type the prefix into the pane —
    /// silently did nothing, and the key a program had bound became unreachable.
    ///
    /// REVERT-PROOF: assign `self.prefix` alone and `C-a C-a` is an unbound key that gets swallowed,
    /// which is exactly the state that shipped in the live run.
    #[test]
    fn the_self_send_follows_the_prefix() {
        let mut keymap = Keymap::default();
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keymap.action(KeyTable::Prefix, "a", ctrl),
            Some(BoundAction::SendPrefix),
            "prefix prefix types the prefix, whatever the prefix is",
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "b", ctrl),
            None,
            "and the old key means nothing"
        );
        // In place: the user's order is not disturbed by a move they did not ask for.
        assert_eq!(
            keymap.binds().next().map(|bind| bind.key().to_string()),
            Some("C-a".to_owned()),
        );
    }

    /// Only the SELF-SEND follows. A user's own binding on the old prefix key is a choice about that
    /// key, and stays where they put it.
    #[test]
    fn a_users_binding_on_the_old_prefix_key_does_not_follow() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "C-b", "detach-client", false)
            .expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keymap.action(KeyTable::Prefix, "b", ctrl),
            Some(BoundAction::DetachClient),
            "their binding stayed on their key",
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "a", ctrl),
            None,
            "and nothing was invented on the new one",
        );
    }

    /// An instant for a routing test that is not about timing.
    ///
    /// Every repeat test takes its own `let base = Instant::now()` and does ARITHMETIC on it, so no
    /// assertion in this module waits for a window to close — a test that slept through 500 ms would
    /// be a test that fails on a loaded machine.
    fn now() -> Instant {
        Instant::now()
    }

    /// A non-repeating [`Routed::Act`], which is what almost every binding produces.
    fn acting(action: BoundAction) -> Routed {
        Routed::Act {
            action,
            again: None,
        }
    }

    /// Ctrl, as every routing test below spells it.
    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    /// The steady state and the mode: a bare command key is the program's, the prefix arms, and the
    /// armed key is the client's.
    #[test]
    fn the_prefix_arms_exactly_one_key() {
        let keymap = Keymap::default();
        let none = Modifiers::default();
        assert_eq!(
            keymap.route(PrefixMode::ToPane, now(), "d", none),
            Routed::ToPane,
            "a bare `d` is a letter, not a detach",
        );
        let armed = keymap.route(PrefixMode::ToPane, now(), "b", ctrl());
        assert_eq!(armed, Routed::Prefix);
        assert_eq!(armed.next(), PrefixMode::AfterPrefix);
        let acted = keymap.route(PrefixMode::AfterPrefix, now(), "d", none);
        assert_eq!(acted, acting(BoundAction::DetachClient));
        assert_eq!(acted.next(), PrefixMode::ToPane, "and the mode is spent");
    }

    /// **The mode ends on the NEXT key whatever that key turns out to be.** Every outcome but the
    /// prefix itself disarms, which is what lets a client with many ways to consume a keystroke get
    /// the rule right by asking rather than by remembering.
    ///
    /// REVERT-PROOF: answer `AfterPrefix` for `Swallow` and one mistyped command key leaves the
    /// client eating every keystroke that follows, with no way for the user to get out.
    #[test]
    fn only_the_prefix_leaves_the_mode_armed() {
        for routed in [
            Routed::ToPane,
            acting(BoundAction::DetachClient),
            Routed::Swallow,
        ] {
            assert_eq!(routed.next(), PrefixMode::ToPane, "{routed:?}");
        }
        assert_eq!(Routed::Prefix.next(), PrefixMode::AfterPrefix);
    }

    /// An unbound command key is SWALLOWED rather than delivered: a user who typed the prefix meant
    /// to address the client, so passing their mistake on would run something they did not ask for.
    #[test]
    fn an_unbound_command_key_is_swallowed() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "z", Modifiers::default()),
            Routed::Swallow,
        );
        // ...and a MODIFIED command key is a different key, so `prefix Ctrl-D` is not a detach.
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "d", ctrl()),
            Routed::Swallow,
        );
    }

    /// **After the prefix, the prefix key is the TABLE's, not a re-arm.** `prefix prefix` types a
    /// literal prefix into the pane — the only way a program that binds `C-b` can still receive it.
    ///
    /// REVERT-PROOF: check `is_prefix` before the armed branch and the two prefixes arm the mode
    /// twice instead of sending anything, so the key becomes unreachable by any program.
    #[test]
    fn the_prefix_twice_is_the_self_send_and_not_a_second_arm() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "b", ctrl()),
            acting(BoundAction::SendPrefix),
        );
    }

    /// Moving onto a key that already meant something REPLACES it — one key means one thing, and a
    /// keymap holding two answers for `C-a` would be a table that could not be printed.
    #[test]
    fn the_self_send_takes_over_the_new_prefix_key() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "C-a", "detach-client", false)
            .expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keymap.action(KeyTable::Prefix, "a", ctrl),
            Some(BoundAction::SendPrefix)
        );
        assert_eq!(
            keymap
                .binds()
                .filter(|bind| bind.key().to_string() == "C-a")
                .count(),
            1,
            "one key, one entry",
        );
    }

    /// A binding in the ROOT table acts with NO prefix, and the key therefore never reaches the
    /// pane. That is the whole of what tmux's `-n` means.
    ///
    /// REVERT-PROOF: drop the root lookup from `route` and this answers `ToPane` — the binding is
    /// still in the table, still printed by `list-keys`, and does nothing at all. Which is exactly
    /// the "a bound key that silently does nothing" failure this module was built to prevent.
    #[test]
    fn a_root_binding_acts_without_the_prefix_and_takes_the_key_from_the_pane() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "F5", "detach-client", false)
            .expect("binds");
        assert_eq!(
            keymap.route(PrefixMode::ToPane, now(), "F5", Modifiers::default()),
            acting(BoundAction::DetachClient),
        );
        assert_eq!(
            keymap.route(PrefixMode::ToPane, now(), "F6", Modifiers::default()),
            Routed::ToPane,
            "and a key nobody bound is still the program's",
        );
    }

    /// **The PREFIX beats a root binding on the same key** — measured against `tmux 3.2a` driving a
    /// real client on a pty, because the manual does not say and the natural implementation gets it
    /// backwards.
    ///
    /// The probe: `bind -n C-b display-message ROOT`, then press `C-b` and a prefix-table key. The
    /// prefix binding fired and the root one never did, with a root binding on a DIFFERENT key
    /// firing as the control — so "the root binding did not run" is not explained by root bindings
    /// being broken.
    ///
    /// REVERT-PROOF: look the root table up before the prefix check and a user who binds anything to
    /// their own prefix key loses the prefix entirely, with every command key after it going to the
    /// shell.
    #[test]
    fn the_prefix_beats_a_root_binding_on_the_same_key() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "C-b", "detach-client", false)
            .expect("binds");
        assert_eq!(
            keymap.route(PrefixMode::ToPane, now(), "b", ctrl()),
            Routed::Prefix,
            "the prefix still arms",
        );
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "d", Modifiers::default()),
            acting(BoundAction::DetachClient),
            "and the table behind it is reachable",
        );
    }

    /// The root table is not consulted after the prefix: `prefix F5` is an unbound COMMAND key,
    /// which is swallowed, not a root binding reached by a longer route.
    #[test]
    fn a_root_binding_is_not_reachable_through_the_prefix() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "F5", "detach-client", false)
            .expect("binds");
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "F5", Modifiers::default()),
            Routed::Swallow,
        );
    }

    /// One key in two tables is TWO bindings, and each edit reaches exactly one of them.
    #[test]
    fn the_tables_hold_one_key_separately() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "%", "detach-client", false)
            .expect("binds");
        let none = Modifiers::default();
        assert_eq!(
            keymap.action(KeyTable::Root, "%", none),
            Some(BoundAction::DetachClient),
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "%", none),
            Some(BoundAction::SplitWindow {
                dir: SplitDir::Horizontal,
                before: false,
            }),
            "the shipped default is a different binding that happens to share a spelling",
        );
        keymap.unbind(KeyTable::Root, "%").expect("unbinds");
        assert_eq!(keymap.action(KeyTable::Root, "%", none), None);
        assert!(
            keymap.action(KeyTable::Prefix, "%", none).is_some(),
            "and unbinding one did not reach the other",
        );
    }

    /// A table sprag does not have is refused BY NAME, never defaulted.
    #[test]
    fn an_unknown_table_is_refused_by_name() {
        assert_eq!(KeyTable::parse("root"), Ok(KeyTable::Root));
        assert_eq!(KeyTable::parse("prefix"), Ok(KeyTable::Prefix));
        let refused = KeyTable::parse("copy-mode").expect_err("no such table");
        assert!(matches!(refused, KeyError::UnknownTable(_)));
        let message = refused.to_string();
        assert!(
            message.contains("copy-mode") && message.contains("root") && message.contains("prefix"),
            "it says what was asked for and what exists: {message}",
        );
    }

    /// **`-r` holds the prefix table open, so the next key acts without a second prefix** — tmux's
    /// repeat, and the reason its own arrow bindings carry the flag.
    ///
    /// The window is arithmetic on a passed-in instant, so this test does not sleep.
    ///
    /// REVERT-PROOF: answer `PrefixMode::ToPane` for a repeating act and the second press is a
    /// letter in the user's shell instead of a second command.
    #[test]
    fn a_repeat_binding_holds_the_prefix_table_open() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "o", "select-pane -t :.+", true)
            .expect("binds");
        let base = Instant::now();
        let first = keymap.route(PrefixMode::AfterPrefix, base, "o", Modifiers::default());
        assert_eq!(
            first,
            Routed::Act {
                action: BoundAction::SelectNextPane,
                again: Some(base + DEFAULT_REPEAT_TIME),
            },
        );
        let armed = first.next();
        assert_eq!(
            armed,
            PrefixMode::Repeating {
                until: base + DEFAULT_REPEAT_TIME
            },
        );
        let inside = base + Duration::from_millis(100);
        assert_eq!(
            keymap.route(armed, inside, "o", Modifiers::default()),
            Routed::Act {
                action: BoundAction::SelectNextPane,
                again: Some(inside + DEFAULT_REPEAT_TIME),
            },
            "and EVERY repeat re-arms the window from itself — measured against tmux, where three \
             presses at 0/400/800ms under a 500ms repeat-time all reach the binding",
        );
    }

    /// The window closes on its own, with nothing watching it: a key arriving after the deadline is
    /// routed as though the prefix had never been pressed.
    ///
    /// **This is why there is no timer.** Nothing in sprag observes the moment a window expires, so
    /// a deadline compared on arrival is indistinguishable from a timer that fires — and the
    /// terminal client's loop stays the pure `select` R226 measured.
    #[test]
    fn a_repeat_window_closes_without_anything_watching_it() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "o", "select-pane -t :.+", true)
            .expect("binds");
        let base = Instant::now();
        let armed = PrefixMode::Repeating { until: base };
        assert!(armed.armed(base), "the deadline itself is still inside");
        let after = base + Duration::from_millis(1);
        assert!(!armed.armed(after));
        assert_eq!(
            keymap.route(armed, after, "o", Modifiers::default()),
            Routed::ToPane,
            "a letter, once the window has closed",
        );
    }

    /// **An unbound key INSIDE a repeat window reaches the pane** — it is not swallowed the way an
    /// unbound key after the prefix is.
    ///
    /// Measured: with a `-r` binding armed in tmux, typing `ZQ` inside the window puts `ZQ` in the
    /// shell. That asymmetry is the whole reason `Repeating` is a state of its own rather than a
    /// deadline hung off `AfterPrefix`.
    ///
    /// REVERT-PROOF: fall into the `Swallow` arm for a repeat window as well, and every character a
    /// user types within half a second of a repeating command vanishes with no way to tell why.
    #[test]
    fn an_unbound_key_inside_a_repeat_window_still_reaches_the_pane() {
        let keymap = Keymap::default();
        let base = Instant::now();
        let armed = PrefixMode::Repeating {
            until: base + DEFAULT_REPEAT_TIME,
        };
        assert_eq!(
            keymap.route(armed, base, "Z", Modifiers::default()),
            Routed::ToPane,
        );
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, base, "Z", Modifiers::default()),
            Routed::Swallow,
            "where the SAME key after the prefix is swallowed",
        );
    }

    /// `repeat-time 0` is a decision, not an absence: the binding acts exactly once.
    ///
    /// The zero case needs no branch anywhere — the window opens already closed, and
    /// [`PrefixMode::armed`] compares an instant that is in the past by the time the next key
    /// arrives. R245's `history-limit` lesson, on a duration.
    #[test]
    fn repeat_time_zero_acts_exactly_once() {
        let mut keymap = Keymap::default();
        keymap.set_repeat_time(0);
        keymap
            .bind(KeyTable::Prefix, "o", "select-pane -t :.+", true)
            .expect("binds");
        let base = Instant::now();
        let acted = keymap.route(PrefixMode::AfterPrefix, base, "o", Modifiers::default());
        assert_eq!(acted.next(), PrefixMode::Repeating { until: base });
        assert_eq!(
            keymap.route(
                acted.next(),
                base + Duration::from_nanos(1),
                "o",
                Modifiers::default()
            ),
            Routed::ToPane,
            "the second press is the program's",
        );
    }

    /// A ROOT binding cannot repeat, and the refusal names the mechanism rather than the rule.
    ///
    /// tmux accepts this combination and does nothing with it (measured). Accepting it here would be
    /// a `-r` a user can see in `list-keys` and never observe.
    #[test]
    fn a_root_binding_cannot_repeat() {
        let mut keymap = Keymap::default();
        let refused = keymap
            .bind(KeyTable::Root, "F5", "detach-client", true)
            .expect_err("repeat has no prefix table to hold open");
        assert!(matches!(refused, KeyError::RepeatInRoot(_)));
        assert!(
            keymap.binds().all(|bind| bind.table() == KeyTable::Prefix),
            "and the refused binding did not land",
        );
    }

    /// Rebinding a repeating key without `-r` STOPS it repeating — the flag is part of the binding
    /// being replaced, not a property that accumulates.
    #[test]
    fn rebinding_without_repeat_takes_the_repeat_away() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Prefix, "o", "select-pane -t :.+", true)
            .expect("binds");
        keymap
            .bind(KeyTable::Prefix, "o", "detach-client", false)
            .expect("rebinds");
        assert!(
            keymap
                .binds()
                .filter(|bind| bind.key().to_string() == "o")
                .all(|bind| !bind.repeats()),
            "the flag went with the binding it was on",
        );
    }

    /// The self-send follows the prefix WITHIN the prefix table only. A `send-prefix` a user put in
    /// the root table is a statement about that key — "send the prefix without pressing it" — so it
    /// stays where they put it.
    ///
    /// **The prefix table's own self-send is unbound FIRST, and that line is the test.** Written
    /// without it this passed with the table check deleted, because the retarget takes the FIRST
    /// match and the shipped `C-b send-prefix` is ahead of anything a user adds — so the root
    /// binding survived by position rather than by rule. Measured, then fixed.
    ///
    /// REVERT-PROOF: drop the table check from `moves` and rebinding the prefix silently retargets a
    /// root binding the user aimed at one specific key.
    #[test]
    fn the_self_send_follows_only_inside_the_prefix_table() {
        let mut keymap = Keymap::default();
        keymap.unbind(KeyTable::Prefix, "C-b").expect("unbinds");
        keymap
            .bind(KeyTable::Root, "C-b", "send-prefix", false)
            .expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        assert_eq!(
            keymap.action(KeyTable::Root, "b", ctrl()),
            Some(BoundAction::SendPrefix),
            "their root binding stayed on their key",
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "a", ctrl()),
            None,
            "and nothing was invented in the prefix table on the way",
        );
    }

    /// The OTHER half of moving the prefix: taking over the new key clears the PREFIX table's
    /// entry for it, and must not reach into the root table.
    ///
    /// A separate test because it needs the opposite setup — the shipped self-send has to still be
    /// there for the takeover to run at all — and the two reverts are two different lines.
    ///
    /// REVERT-PROOF: drop the table check from the `retain` and a root binding on the key the user
    /// moved their prefix to is DELETED, silently, by an edit about a different table.
    #[test]
    fn moving_the_prefix_does_not_sweep_a_root_binding_off_the_new_key() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "C-a", "detach-client", false)
            .expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        assert_eq!(
            keymap.action(KeyTable::Root, "a", ctrl()),
            Some(BoundAction::DetachClient),
            "the root binding on the new prefix key is a different key and stays",
        );
        assert_eq!(
            keymap.action(KeyTable::Prefix, "a", ctrl()),
            Some(BoundAction::SendPrefix),
            "while the self-send did move onto it",
        );
    }
}
