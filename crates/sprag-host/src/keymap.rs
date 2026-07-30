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
//! ## The ROUTING is here too, because both frontends must agree
//!
//! [`Keymap::route`] is the whole state machine: whether a keystroke is the prefix, what an armed key
//! means, and — through [`Routed::next`] — when the one-key mode ends. It lives beside the table
//! rather than in either client, because the two clients decode keys differently (termwiz events in
//! the terminal, pinion key names in the GUI) and agree about everything after that. A second
//! implementation would be two answers to "what does this user's table say".
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

use sprag_input::Modifiers;
use sprag_terminal::SplitDir;

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
    /// One key is both bound and unbound by the same file.
    BoundAndUnbound(String),
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
        }
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

/// A client's prefix key and the table of commands that follow it.
///
/// Ordered rather than hashed: a keymap holds a handful of entries and is read once per keystroke,
/// so a linear scan is not a cost — and the ORDER is load-bearing for `list-keys`, which has to show
/// a user the table they wrote.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Keymap {
    /// The key that says "the next keystroke is mine".
    prefix: KeySpec,
    /// What each key means after the prefix, in declaration order.
    binds: Vec<(KeySpec, BoundAction)>,
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
        Self {
            prefix: key("C-b"),
            binds: vec![
                (key("C-b"), BoundAction::SendPrefix),
                (
                    key("\""),
                    BoundAction::SplitWindow {
                        dir: SplitDir::Vertical,
                        before: false,
                    },
                ),
                (
                    key("%"),
                    BoundAction::SplitWindow {
                        dir: SplitDir::Horizontal,
                        before: false,
                    },
                ),
                (key("d"), BoundAction::DetachClient),
                (key("o"), BoundAction::SelectNextPane),
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
        let moves = |(key, action): &(KeySpec, BoundAction)| {
            *key == previous && *action == BoundAction::SendPrefix
        };
        if self.binds.iter().any(moves) {
            self.binds.retain(|(key, _)| *key != next);
            if let Some(slot) = self.binds.iter_mut().find(|bind| moves(bind)) {
                // Retargeted in place rather than removed and re-pushed, so `list-keys` shows it
                // where the user's file put it instead of at the end.
                slot.0 = next;
            }
        }
        Ok(())
    }

    /// Bind `key` to `action`, replacing whatever it meant before — tmux's `bind-key`.
    ///
    /// Replacing IN PLACE rather than appending keeps the order a user wrote: rebinding `%` leaves
    /// it where it was in their file instead of moving it to the end of `list-keys`.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownKey`] / [`KeyError::UnknownAction`] / [`KeyError::BadFlags`].
    pub fn bind(&mut self, key: &str, action: &str) -> Result<(), KeyError> {
        let key = KeySpec::parse(key)?;
        let action = BoundAction::parse(action)?;
        match self.binds.iter_mut().find(|(bound, _)| *bound == key) {
            Some(slot) => slot.1 = action,
            None => self.binds.push((key, action)),
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
    pub fn unbind(&mut self, key: &str) -> Result<(), KeyError> {
        let key = KeySpec::parse(key)?;
        self.binds.retain(|(bound, _)| *bound != key);
        Ok(())
    }

    /// Whether a keystroke a client just decoded is the prefix.
    #[must_use]
    pub fn is_prefix(&self, name: &str, mods: Modifiers) -> bool {
        self.prefix.matches(name, mods)
    }

    /// What a keystroke means AFTER the prefix, or [`None`] for an unbound key.
    ///
    /// An unbound key is the caller's to swallow: a user who typed the prefix meant to address the
    /// client, so delivering their mistake to a shell would run something they did not ask for.
    #[must_use]
    pub fn action(&self, name: &str, mods: Modifiers) -> Option<BoundAction> {
        self.binds
            .iter()
            .find(|(key, _)| key.matches(name, mods))
            .map(|(_, action)| *action)
    }

    /// Every binding, in the order a user would read them.
    pub fn binds(&self) -> impl Iterator<Item = (&KeySpec, BoundAction)> {
        self.binds.iter().map(|(key, action)| (key, *action))
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
    #[must_use]
    pub fn route(&self, mode: PrefixMode, name: &str, mods: Modifiers) -> Routed {
        if mode == PrefixMode::AfterPrefix {
            return self.action(name, mods).map_or(Routed::Swallow, Routed::Act);
        }
        if self.is_prefix(name, mods) {
            return Routed::Prefix;
        }
        Routed::ToPane
    }
}

/// Where the next keystroke goes.
///
/// Two states rather than a `bool` because the prefix is not a modifier — it is a mode the client
/// enters and leaves, and `after_prefix: true` at a call site says nothing about which way round
/// that is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PrefixMode {
    /// The steady state: keys are the program's.
    #[default]
    ToPane,
    /// The prefix was just pressed, so the next key is a command to this client.
    AfterPrefix,
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
    Act(BoundAction),
    /// Nothing at all — an unbound key after the prefix.
    Swallow,
}

impl Routed {
    /// Where the NEXT keystroke goes after this one.
    ///
    /// **The mode is one key long, and this is the one definition of that.** Total on purpose: a
    /// client that derives its next mode from here cannot arm the mode by accident, and — because
    /// every outcome but [`Routed::Prefix`] answers [`PrefixMode::ToPane`] — cannot forget to
    /// disarm it either. Both frontends have more ways to leave a keystroke than to route one (the
    /// GUI has five surfaces that consume a key before the pane is even resolved), so the rule has
    /// to live somewhere neither of them can partially implement.
    #[must_use]
    pub fn next(&self) -> PrefixMode {
        match self {
            Self::Prefix => PrefixMode::AfterPrefix,
            Self::ToPane | Self::Act(_) | Self::Swallow => PrefixMode::ToPane,
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
            .map(|(key, action)| format!("{key} {action}"))
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
        keymap.bind("c", "split-window -h").expect("binds");
        assert_eq!(
            keymap.action("d", Modifiers::default()),
            Some(BoundAction::DetachClient),
            "the defaults survive",
        );
        assert_eq!(
            keymap.action("c", Modifiers::default()),
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
        let before: Vec<String> = keymap.binds().map(|(key, _)| key.to_string()).collect();
        keymap.bind("%", "detach-client").expect("binds");
        let after: Vec<String> = keymap.binds().map(|(key, _)| key.to_string()).collect();
        assert_eq!(before, after, "the order is the user's, not the edit's");
        assert_eq!(
            keymap.action("%", Modifiers::default()),
            Some(BoundAction::DetachClient),
        );
    }

    /// Unbinding removes a default and is IDEMPOTENT — but a typo in the key is still refused.
    #[test]
    fn unbinding_removes_a_default_and_repeats_harmlessly() {
        let mut keymap = Keymap::default();
        keymap.unbind("o").expect("unbinds");
        assert_eq!(keymap.action("o", Modifiers::default()), None);
        keymap.unbind("o").expect("unbinding twice is not an error");
        assert!(matches!(keymap.unbind("Up"), Err(KeyError::UnknownKey(_))));
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
            keymap.action("a", ctrl),
            Some(BoundAction::SendPrefix),
            "prefix prefix types the prefix, whatever the prefix is",
        );
        assert_eq!(
            keymap.action("b", ctrl),
            None,
            "and the old key means nothing"
        );
        // In place: the user's order is not disturbed by a move they did not ask for.
        assert_eq!(
            keymap.binds().next().map(|(key, _)| key.to_string()),
            Some("C-a".to_owned()),
        );
    }

    /// Only the SELF-SEND follows. A user's own binding on the old prefix key is a choice about that
    /// key, and stays where they put it.
    #[test]
    fn a_users_binding_on_the_old_prefix_key_does_not_follow() {
        let mut keymap = Keymap::default();
        keymap.bind("C-b", "detach-client").expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keymap.action("b", ctrl),
            Some(BoundAction::DetachClient),
            "their binding stayed on their key",
        );
        assert_eq!(
            keymap.action("a", ctrl),
            None,
            "and nothing was invented on the new one",
        );
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
            keymap.route(PrefixMode::ToPane, "d", none),
            Routed::ToPane,
            "a bare `d` is a letter, not a detach",
        );
        let armed = keymap.route(PrefixMode::ToPane, "b", ctrl());
        assert_eq!(armed, Routed::Prefix);
        assert_eq!(armed.next(), PrefixMode::AfterPrefix);
        let acted = keymap.route(PrefixMode::AfterPrefix, "d", none);
        assert_eq!(acted, Routed::Act(BoundAction::DetachClient));
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
            Routed::Act(BoundAction::DetachClient),
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
            keymap.route(PrefixMode::AfterPrefix, "z", Modifiers::default()),
            Routed::Swallow,
        );
        // ...and a MODIFIED command key is a different key, so `prefix Ctrl-D` is not a detach.
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, "d", ctrl()),
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
            keymap.route(PrefixMode::AfterPrefix, "b", ctrl()),
            Routed::Act(BoundAction::SendPrefix),
        );
    }

    /// Moving onto a key that already meant something REPLACES it — one key means one thing, and a
    /// keymap holding two answers for `C-a` would be a table that could not be printed.
    #[test]
    fn the_self_send_takes_over_the_new_prefix_key() {
        let mut keymap = Keymap::default();
        keymap.bind("C-a", "detach-client").expect("binds");
        keymap.set_prefix("C-a").expect("sets");
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(keymap.action("a", ctrl), Some(BoundAction::SendPrefix));
        assert_eq!(
            keymap
                .binds()
                .filter(|(key, _)| key.to_string() == "C-a")
                .count(),
            1,
            "one key, one entry",
        );
    }
}
