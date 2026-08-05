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
//! who wants one extra binding does not have to re-declare the ones they already had.

use std::fmt;
use std::time::{Duration, Instant};

use sprag_input::Modifiers;
use sprag_terminal::{OrderStep, PaneDir, SplitDir, WindowPlace};

use crate::wire::SelectWindowAsk;

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
                "{verb:?} is not an action (there are: {})",
                BoundAction::VOCABULARY.join(", ")
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
    /// **SHIFT is not a modifier on a printable CHARACTER, and R306 measured what pretending
    /// otherwise costs.** A character key already carries its shift state in the character: a user
    /// pressing `Shift+5` on a US layout produces `%`, and `sprag-gui` reports it as the W3C key
    /// `"%"` with winit's `shift_key()` ALSO set (`winit_modifiers_to_pinion`), where `sprag-tui`
    /// reads a raw pty byte and reports `"%"` with no modifier at all — the same keystroke, two
    /// spellings, and an exact modifier comparison matches only the second. So `prefix %`, a tmux
    /// default this project has shipped since the keymap existed, did not fire in the GUI on a real
    /// keyboard, and every test missed it because both the unit tests and the pixel smoke synthesize
    /// the key WITHOUT the flag a keyboard sets. Shift is therefore masked off BOTH sides for a
    /// character, and compared exactly for a NAMED key (`S-Tab` is a different key from `Tab`, and
    /// nothing about `Tab` carries the shift).
    ///
    /// Case is compared exactly for a character too, with ONE exception: a lone ASCII letter under
    /// `Ctrl`. A terminal sends `Ctrl-B` as the C0 byte `0x02`, which decodes as lowercase, while a
    /// `CSI u` terminal reports whichever case the layout produced — two spellings of one keystroke,
    /// and the C0 byte cannot say which. Without `Ctrl` the case is the CHARACTER and folding it
    /// would make `P` and `p` one binding, which is exactly how `prefix P` silently stole
    /// `prefix p`'s window walk while this was being written.
    #[must_use]
    pub fn matches(&self, name: &str, mods: Modifiers) -> bool {
        let character = is_character(&self.name) && is_character(name);
        let shifted = |mods: Modifiers| Modifiers {
            shift: !character && mods.shift,
            ..mods
        };
        shifted(self.mods) == shifted(mods) && same_key(&self.name, name, self.mods.ctrl)
    }
}

/// Whether two key names are the same key — a lone ASCII letter under `Ctrl` case-insensitively,
/// everything else exactly. See [`KeySpec::matches`] for why `ctrl` is what decides it.
fn same_key(spec: &str, typed: &str, ctrl: bool) -> bool {
    if ctrl && is_ascii_letter(spec) && is_ascii_letter(typed) {
        return spec.eq_ignore_ascii_case(typed);
    }
    spec == typed
}

/// Whether `name` is one printable CHARACTER — the keys whose shift state is the character itself,
/// as opposed to the named keys ([`sprag_input::NAMED_KEYS`]) where `Shift` is a modifier a user
/// really does hold.
fn is_character(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| !c.is_control()) && chars.next().is_none()
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

/// What a [`BoundAction`] acts on — the four subjects this vocabulary has.
///
/// A grouping axis, and it is an enum rather than a string because it is READ: the help view orders
/// its sections by it, so "which group is this in" has to be a decision with a fixed set of answers
/// and a fixed order rather than a label each caller invents.
///
/// The order is the declaration order, and it is the reading order of the view: the CLIENT's own
/// actions first, because `list-keys` and `detach-client` are what a user who is lost needs and the
/// top of a list is where they will look; then outward through the containment the product itself
/// uses, pane inside window inside session.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ActionSubject {
    /// This client and its keyboard — detach, send the prefix, show the keys.
    Client,
    /// One pane: splitting, focusing, swapping, sizing, zooming, naming.
    Pane,
    /// One window: creating, walking to, killing, naming.
    Window,
    /// The session itself.
    Session,
}

impl ActionSubject {
    /// Every subject, in the reading order the view uses.
    ///
    /// An array literal for [`PaneDir::ALL`]'s reason and with [`PaneDir::ALL`]'s residual: Rust has
    /// no stable way to derive it, so a fifth subject left out of this would compile and simply
    /// never render a section. The test that walks every [`BoundAction`] and asserts its subject is
    /// in here is what catches that.
    pub const ALL: [Self; 4] = [Self::Client, Self::Pane, Self::Window, Self::Session];

    /// What the view calls this group.
    #[must_use]
    pub fn heading(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Pane => "pane",
            Self::Window => "window",
            Self::Session => "session",
        }
    }
}

/// WHICH session a [`BoundAction::SwitchClient`] moves this client to.
///
/// [`SelectWindowAsk`]'s twin one level up, and the extra arm is where the two differ: a window
/// ring has no "the one I was on before" to go back to, because a window's identity is only
/// meaningful inside its session, while a CLIENT visits sessions and the daemon remembers.
///
/// The wire counterpart is [`crate::wire::AttachAsk`], and this is deliberately NOT that type:
/// [`Ask`](Self::Ask) has no wire form at all (a prompt is a client-side act that produces a
/// [`Named`](Self::Named) later), and `AttachAsk` has a `Scoped` arm no keystroke can mean. Two
/// grammars because two vocabularies, each complete at its own surface — the shape
/// [`SelectWindowAsk`] and [`sprag_terminal::WindowPlace`] already take.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SwitchClientAsk {
    /// `-n` / `-p` — one step along the daemon's session order, wrapping.
    Step(OrderStep),
    /// `-l` — the session this client was viewing BEFORE this one (tmux `switch-client -l`).
    LastViewed,
    /// `-t <session>` — the session with this NAME, fixed in the config file.
    ///
    /// A `String` and not a [`sprag_terminal::SessionName`], on this vocabulary's standing rule
    /// that a client never owns a name's grammar: the daemon parses it, and a binding that refused
    /// a name at CONFIG-LOAD time would reject a config for naming a session the user is about to
    /// create.
    Named(String),
    /// `-t` with no name — ASK which session, then go there.
    ///
    /// The one thing `select-window` has no counterpart for, and it is honest here for
    /// [`BoundAction::MoveWindowBefore`]'s reason: the argument is an ADDRESS the user picks per
    /// press, not the whole content of the act the way a rename's is.
    Ask,
}

impl fmt::Display for ActionSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.heading())
    }
}

/// The first word of an action's spelling, for a caller holding the whole line.
///
/// One definition so [`BoundAction::verb`] and the vocabulary's own entries are cut the same way —
/// `"resize-pane -L|-R|-U|-D [N]"` and `"resize-pane -L 5"` have to yield the same word or the view
/// that joins them would report a bound verb as unbound.
pub(crate) fn verb_of(spelling: &str) -> &str {
    spelling.split_whitespace().next().unwrap_or("")
}

/// What a bound key does.
///
/// Every variant is an action a CLIENT can carry out on its own focus. Nothing here needs the
/// daemon to have a current pane, which is what keeps the vocabulary honest: an action sprag could
/// not perform would be a binding that parsed and then did nothing.
/// **Not `Copy`, since R305.** The vocabulary carries DATA now — `select-window -t <window>` names
/// a window, because a window's name is its address and that is how `prefix 1` is expressed. A
/// keystroke still cannot carry a target that says which PANE it acts on (`split-window`'s rule);
/// what it can carry is where it is going.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BoundAction {
    /// `detach-client` — give the terminal back and leave the session running.
    DetachClient,
    /// `send-prefix` — type the PREFIX key into the pane, which is what keeps the prefix reachable
    /// by a program that binds it (readline's backward-char, for one).
    ///
    /// The prefix, not the key that was pressed: a user who rebinds this to `a` means `prefix a` to
    /// send `C-b`, not to send `a`.
    SendPrefix,
    /// `list-keys` — show what the keys do, on the screen the user is already looking at (tmux's
    /// `prefix ?`).
    ///
    /// The vocabulary's own reflection, and the only action here whose subject is this list. It is
    /// client-local like [`DetachClient`](Self::DetachClient) and for the same reason a keybinding
    /// is: the table being shown is the CLIENT's, read from its config, so the answer needs no
    /// daemon and cannot be wrong about a session it is not attached to.
    ///
    /// **Why an action rather than a chord each frontend picks.** The key that opens the help is
    /// the one binding a user is likeliest to need before they know how to change it — so it has to
    /// appear in the list it opens, and a chord held privately by a frontend could not. Putting it
    /// in the vocabulary also makes it rebindable and unbindable by the same file as everything
    /// else, which is what `list-keys` claims to be printing.
    ListKeys,
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
    /// `select-pane -L|-R|-U|-D` — move to the pane ADJACENT in that direction.
    ///
    /// The sibling of [`SelectNextPane`](Self::SelectNextPane) and not a refinement of it: that one
    /// walks the pane POOL in paint order, this one walks the ARRANGEMENT. The two answer different
    /// questions, and only this one is a statement about where the panes are on screen.
    ///
    /// **Adjacency is not resolved here, and not in either client.** The client sends the
    /// direction; the daemon walks its own arrangement and moves the session's active pane under
    /// one lock — see
    /// [`HostClient::select_toward`](crate::HostClient::select_toward). A binding that resolved a
    /// neighbour itself would be a second answer to `sprag select-pane -L`, derived from a mirror
    /// that can be one revision behind the tiling it is naming.
    SelectPaneToward {
        /// Which way to move — tmux's `-L` / `-R` / `-U` / `-D`.
        dir: PaneDir,
    },
    /// `swap-pane -L|-R|-U|-D` — trade places with the pane ADJACENT in that direction.
    ///
    /// [`SelectPaneToward`](Self::SelectPaneToward)'s twin: the same walk over the same arrangement,
    /// moving the PANE instead of the cursor. Everything that arm's docs say about where adjacency
    /// is resolved holds here verbatim — the direction travels and the daemon walks its own tree,
    /// where the rival's key path reads the rectangles of the frame it last composed
    /// (`directional_pane_swap_from_view`, herdr `9a4ce5e1`) and only falls back to its API when
    /// that lookup fails.
    ///
    /// **It carries no origin**, unlike the CLI verb and the MCP tool: a keystroke can only ever
    /// mean "the pane I am on", which is the same reason
    /// [`SelectPaneToward`](Self::SelectPaneToward) carries none.
    SwapPaneToward {
        /// Which way the pane moves — tmux's `-L` / `-R` / `-U` / `-D` spelling, though not tmux's
        /// MEANING: `swap-pane -U` there swaps with the previous pane in index order, where this is
        /// the pane above in the arrangement. sprag has no index-order swap to give the flag its
        /// tmux reading, and one vocabulary for the four directions is worth more than a flag that
        /// means two things.
        dir: PaneDir,
    },
    /// `resize-pane -L|-R|-U|-D [N]` — move the boundary that bounds the focused pane on that axis
    /// (tmux's `prefix C-Arrow` and `prefix M-Arrow`, and the eight `-r` defaults this vocabulary
    /// had no verb for until R307).
    ///
    /// **The direction moves the BOUNDARY, not the pane.** Whether the focused pane grows or
    /// shrinks follows from which side of that boundary it sits on, which is what makes one rule
    /// cover both and is exactly how tmux behaves. See
    /// [`crate::wire::RESIZE_PANE_ACTION`].
    ///
    /// Unlike every other directional arm here it carries a DISTANCE, and that is the one argument
    /// a binding may fix: `split-window`'s pane target is refused because a keystroke acts where
    /// the user is, and a name is refused because it is what the user is about to type — a
    /// distance is neither. It is the whole content of the difference between tmux's two default
    /// families, one cell on `C-Arrow` and five on `M-Arrow`, and a vocabulary that could not
    /// spell it would need two verbs to say one thing.
    ResizePaneToward {
        /// Which way the BOUNDARY travels — tmux's `-L` / `-R` / `-U` / `-D`.
        dir: PaneDir,
        /// How many cells. Never zero: a binding that moved nothing is one the user cannot tell
        /// from a broken one, so [`parse`](Self::parse) refuses it rather than accepting a key
        /// that does nothing.
        cells: u16,
    },
    /// `zoom-pane [-Z|-u]` — fill the window with the focused pane alone, or give the arrangement
    /// back (tmux's `resize-pane -Z`, bound to `prefix z`).
    ZoomPane {
        /// `None` TOGGLES, which is what makes one key a switch; `Some(true)` is `-Z` and
        /// `Some(false)` is `-u`.
        ///
        /// The tri-state is carried rather than collapsed to a toggle because the CLI verb has both
        /// explicit flags, and a binding vocabulary that accepted the verb while refusing its flags
        /// would promise half a grammar — the thing `select-pane`'s own arm exists to avoid. Here
        /// the whole grammar is built, so the whole grammar is taken.
        on: Option<bool>,
    },
    /// `kill-pane` — end the focused pane and everything running in it; the window's LAST pane
    /// ends the WINDOW, and so on up the chain (tmux `prefix x`).
    ///
    /// **The chain is what makes this a different verb from
    /// [`KillWindow`](Self::KillWindow) rather than a smaller one.** A user pressing this is
    /// usually closing one of several panes, and once in a while is ending their session without
    /// having said the word "session" — which is exactly why it ships GUARDED (below) and why the
    /// question [`prompt::Ask::of`](crate::prompt::Ask::of) builds off it names what is actually
    /// about to go, read from the live arrangement rather than written into the binding.
    ///
    /// **Bound to `prefix x` as `confirm-before kill-pane`, which is tmux's own default**
    /// (`bind-key x confirm-before -p "kill-pane #P? (y/n)" kill-pane`). [`KillWindow`](Self::KillWindow)
    /// is shipped the same way on `prefix &`, for the reason stated there: a destructive key a
    /// user's fingers already carry must arrive with the guard those fingers expect.
    ///
    /// It names no pane, on [`NewWindow`](Self::NewWindow)'s rule — a keystroke acts where the user
    /// is.
    KillPane,
    /// `new-window` — create a window in this session, born with a shell, and select it (tmux
    /// `prefix c`).
    ///
    /// **The first arm of this vocabulary that CREATES anything.** Before R305 a key could
    /// rearrange what was already there and nothing else, while the GUI's palette offered the whole
    /// lifecycle and `sprag-tui` (which has no palette) offered none of it.
    ///
    /// It takes no arguments, where the CLI verb takes `{name?, cmd?, cwd?}`: each of those is a
    /// string a keystroke cannot carry, and a binding that fixed one in the config file would make
    /// every press produce the same name — the second one refused. Same rule as `split-window`'s
    /// missing pane target.
    NewWindow,
    /// `select-window -n|-p|-t <name>` — make another window of this session current (tmux
    /// `prefix n` / `prefix p`, and `select-window -t`).
    ///
    /// The RING is walked by the daemon ([`HostClient::select_window_toward`](crate::HostClient::select_window_toward)),
    /// never by the client that pressed the key — the same authority split
    /// [`SelectPaneToward`](Self::SelectPaneToward) states one level down, and for the same reason:
    /// a client walking its own window mirror would be a second answer to `sprag select-window -n`,
    /// derived from a list that can be a revision behind.
    ///
    /// The `-t` arm carries a NAME because a window's name is its address and a binding that names
    /// one is how `prefix 1` is expressed. That is not the pane target `split-window` refuses: a
    /// pane target would say WHICH PANE a keystroke acts on, where this says which window to go to,
    /// which is the whole content of the verb.
    SelectWindow {
        /// Which window — a step along the ring, or one by name.
        ask: SelectWindowAsk,
    },
    /// `move-window --first|--last|-n|-p|--before <w>|--after <w>` — move THIS session's current
    /// window to a place in the session's order (tmux `move-window`; the near-universal user
    /// bindings `prefix <` / `prefix >` ship as the two step forms).
    ///
    /// [`SelectWindow`](Self::SelectWindow)'s companion, and the pair is where this project draws
    /// the distinction its own docs left implicit: the select WALKS a ring and wraps, this
    /// ARRANGES a sequence and stops at the ends.
    ///
    /// It names no window, on [`NewWindow`](Self::NewWindow)'s rule: a keystroke acts where the
    /// user is. The PLACE, unlike a name to rename to, is exactly the kind of argument a config
    /// CAN fix — `bind < move-window -p` means the same thing every time it is pressed — which is
    /// why this arm carries one where [`RenameWindow`](Self::RenameWindow) carries a decision to
    /// ask.
    MoveWindow {
        /// Where the current window goes.
        place: WindowPlace,
    },
    /// `move-window --before` with NO anchor — ask which window to move the current one in front
    /// of, then do it (tmux's own `prefix .`, which is `command-prompt "move-window -t '%%'"`).
    ///
    /// **A missing argument is what makes this ask**, which is the one place this vocabulary lets a
    /// verb have both forms: `move-window --before logs` fixes the anchor in the config and
    /// `move-window --before` asks for it. That is honest here and would not be for a rename,
    /// because an anchor is an ADDRESS the user picks per press while a rename's argument is the
    /// whole content of the act.
    ///
    /// BEFORE and not AFTER, because that is the drop-target reading a person already has from
    /// dragging a tab: the window you name is the one it lands in front of. The tail of the order
    /// is reached by `--last`, which needs no name at all.
    MoveWindowBefore,
    /// `kill-window` — end this session's CURRENT window and everything running in it; the
    /// session's LAST window ends the SESSION (tmux `prefix &`).
    ///
    /// **BINDABLE AND UNBOUND BY DEFAULT**, and the reason is tmux's own default: `prefix &` is
    /// `confirm-before -p "kill-window #W? (y/n)" kill-window`, so the key a tmux user has in their
    /// fingers is guarded by a prompt. sprag has no `confirm-before` and `sprag-tui` has no prompt
    /// surface, so shipping the spelling without the guard would hand those fingers a destructive
    /// verb they expect to be asked about. Offering it to a user who ASKS for it is the honest half.
    ///
    /// It names no window, on [`NewWindow`](Self::NewWindow)'s rule: a keystroke acts where the user
    /// is.
    KillWindow,
    /// `rename-window` — ask for the current window's new name, then rename it (tmux `prefix ,`).
    ///
    /// **The first arm that cannot be carried out by the keystroke alone.** Every other verb in
    /// this vocabulary either takes no argument or takes one a config can fix; a name is neither,
    /// because a binding that fixed one would rename every window to the same string. So the arm
    /// does not carry a name — it carries the DECISION TO ASK, and
    /// [`prompt::Ask::of`](crate::prompt::Ask::of) turns it into the question.
    ///
    /// That is also why there is no `command-prompt` verb here. tmux spells this
    /// `command-prompt -I "#W" -p "(rename-window) " "rename-window '%%'"`: a format language to
    /// name the window, a template, and a substitution that re-parses text the user typed. The
    /// question and the seed are DERIVED from the live state at the moment the key is pressed, so
    /// there is nothing for a format language to do; and the answer fills a typed slot, so there is
    /// nothing for the quoting to protect.
    ///
    /// It names no window, on [`NewWindow`](Self::NewWindow)'s rule.
    RenameWindow,
    /// `rename-session` — ask for this session's new name, then rename it (tmux `prefix $`).
    ///
    /// [`RenameWindow`](Self::RenameWindow)'s twin one level up, and the one that moves an ADDRESS:
    /// the session name is what every `-t` takes and what every attached client holds. The daemon
    /// carries the change channel and the attachments across with it (R302/R303), which is why a
    /// client can ask for this without knowing anything about who else is watching.
    RenameSession,
    /// `rename-pane` — ask for the focused pane's new name, then rename it.
    ///
    /// The one rename with a TARGET, because the one with an identity to target: a
    /// [`PaneId`](sprag_terminal::PaneId) is registry-unique and does not move, where a window and a
    /// session are addressed by the very name being changed. R295 settled that a pane name is an
    /// ADDRESS rather than a decoration; this is the gesture that gives a human one.
    ///
    /// **Bound to `prefix P` by default, and that key comes from the RIVAL.** tmux has no
    /// pane-rename verb at all, so there is no key to inherit from it; herdr binds
    /// `prefix+shift+p` (`rename_pane`, `src/config/model.rs` at `9a4ce5e1`). Where the primary
    /// parity target is silent, taking the other one's key beats inventing a third — a herdr user's
    /// fingers already carry it, and the alternative was spending one of a shrinking set of free
    /// letters on a guess.
    RenamePane,
    /// `switch-client -n|-p|-l|-t [<session>]` — point THIS client at another session of the same
    /// daemon (tmux `prefix (` / `prefix )` / `prefix L`).
    ///
    /// # The first arm whose subject is the CLIENT rather than what it is looking at
    ///
    /// It is [`DetachClient`](Self::DetachClient)'s twin, which is why they share
    /// [`ActionSubject::Client`]: one gives the terminal back, the other points it somewhere else,
    /// and NEITHER changes any session. Filing it under
    /// [`ActionSubject::Session`] would put it beside
    /// `rename-session`, which is a verb that edits a session — a different kind of act that only
    /// shares a noun.
    ///
    /// # Why the vocabulary needed it at all
    ///
    /// The keys existed and the vocabulary did not have them. `sprag-gui` held a private
    /// `SessionChord` table with three hard-coded chords, so `sprag list-keys` could not name them
    /// (R308 derives every row from this enum), a config file could not rebind or unbind them, and
    /// `sprag-tui` had no way to reach another session AT ALL. The step among them was resolved by
    /// walking that client's own `sessions` poll mirror and attaching BY NAME — R304's defect
    /// reached by a different route, one door from the arm R304 fixed.
    ///
    /// # What each arm may carry
    ///
    /// `-n`/`-p` carry a DIRECTION and no origin, on [`SelectPaneToward`](Self::SelectPaneToward)'s
    /// rule: a keystroke can only ever mean "from where I am", and the daemon holds where that is.
    /// `-l` carries nothing, because the answer is the daemon's visit history and a client that
    /// held its own would be R304 again. `-t <session>` carries a NAME, which is exactly the kind
    /// of argument a config CAN fix — `bind 1 switch-client -t work` means the same thing every
    /// press, and it is how the rival's indexed `switch_workspace 1..9` is said here. `-t` with the
    /// name LEFT OFF asks for one, [`MoveWindowBefore`](Self::MoveWindowBefore)'s rule.
    SwitchClient {
        /// Which session to move to.
        ask: SwitchClientAsk,
    },
    /// `confirm-before <action>` — ask a yes/no question naming what will be destroyed, and carry
    /// `action` out only if the answer is yes (tmux's `prefix &` guard).
    ///
    /// # Why a WRAPPER and not a property of the destructive verb
    ///
    /// The GUI's command catalog decides destructiveness per COMMAND, because a palette row is the
    /// client's own vocabulary and a user who types four letters and presses Enter has not aimed at
    /// anything. A BINDING is the opposite: it is the user's own sentence, so `bind & kill-window`
    /// and `bind & confirm-before kill-window` have to mean different things or the config does not
    /// mean what it says. sprag ships the guarded spelling as the DEFAULT — tmux's key with tmux's
    /// guard — and leaves the bare verb bindable by anyone who wants it.
    ///
    /// The wrapped action is [`Box`]ed because an action that contains an action is a recursive
    /// type, and refused if it would ask a question of its own ([`asks`](Self::asks)): a prompt
    /// that opens a prompt is a grammar this vocabulary does not have a surface for, and a
    /// `confirm-before rename-window` would ask twice for something that destroys nothing.
    ConfirmBefore {
        /// What to do if the answer is yes.
        action: Box<BoundAction>,
    },
}

/// tmux's spelling of "the next pane", which is the only `-t` target form a binding may carry.
///
/// The rest of tmux's target grammar (`-t :=2`, `-t {left-of}`, session/window addressing) is H5's,
/// and accepting a fragment of it here would promise a grammar sprag has not built. The DIRECTIONAL
/// forms are not part of it — they are flags ([`flag_of`]), not targets.
const NEXT_PANE_TARGET: [&str; 2] = ["-t", ":.+"];

/// tmux's directional flag for `dir` — `-L` / `-R` / `-U` / `-D`.
///
/// **An exhaustive `match`, and that is the point.** This was a `[(&str, PaneDir); 4]` table with a
/// reverse lookup that `.expect()`ed every direction to be in it: a fifth [`PaneDir`] variant broke
/// neither the array's length nor `PaneDir::ALL`'s, so the first thing to notice would have been
/// `sprag list-keys` PANICKING inside a [`Display`](fmt::Display) impl. Written as a match, that
/// same variant fails to COMPILE here, which is where the vocabulary is decided.
///
/// It stays ONE table for both directions — [`direction_of`] is this function searched, so a parse
/// and a render cannot drift apart while every test still passes (the shape R296 found copy-pasted
/// between a search slot and a wait, and R297 found in `sprag bind-key`'s own copy of the action
/// list).
///
/// The four DIRECTIONS themselves are spelled once more, on
/// [`PaneDir::from_wire`](sprag_terminal::PaneDir::from_wire) — this is the FLAG spelling a shell
/// and a config take, that one is the WIRE's. Two vocabularies, deliberately, each written once.
///
/// **The residual, stated:** [`direction_of`] searches [`PaneDir::ALL`], which is an array literal
/// no compiler checks for completeness. A new variant therefore fails to compile HERE (so it cannot
/// be rendered wrongly) and would fail to PARSE silently if it were also left out of `ALL`. Rust
/// has no stable way to derive that array, so the gap is named rather than hidden.
///
/// PRIVATE, unlike [`direction_of`]: rendering a flag is this module's own job (the `Display` below),
/// and nothing outside it asks. A `pub` with no caller outside the crate is the shape this project has
/// now recorded three times and is not adding a fourth.
#[must_use]
fn flag_of(dir: PaneDir) -> &'static str {
    match dir {
        PaneDir::Left => "-L",
        PaneDir::Right => "-R",
        PaneDir::Up => "-U",
        PaneDir::Down => "-D",
    }
}

/// The [`PaneDir`] a directional flag names (`-L` / `-R` / `-U` / `-D`), or [`None`] for anything
/// that is not one.
///
/// The inverse of this module's ONE flag table — an exhaustive `match`, private because rendering a
/// flag is the keymap's own job — and DERIVED from it rather than tabulated beside it, so a parse and
/// a render cannot drift apart while every test still passes.
///
/// Public because the flag spelling has a SECOND parser: `sprag select-pane -L` reaches the same
/// action from a shell. That one held its own `"-L" => "left"` table, which mapped a flag straight
/// to a WIRE word and so bypassed [`PaneDir`] altogether — a third spelling of one vocabulary,
/// checked by nothing. One parser, two callers.
#[must_use]
pub fn direction_of(flag: &str) -> Option<PaneDir> {
    PaneDir::ALL.into_iter().find(|dir| flag_of(*dir) == flag)
}

impl BoundAction {
    /// Every form a binding may name, in the shell's own spelling.
    ///
    /// **The ONE enumeration of this vocabulary.** It is the only place a user learns what a
    /// binding can say, and it is read from two surfaces — [`KeyError::UnknownAction`]'s report,
    /// which a config file's typo reaches, and `sprag bind-key`'s, which an empty action reaches.
    /// Those two each held their own copy until R297, and the CLI's had been stale since R289 added
    /// `zoom-pane`: a verb that exists and is absent from the list nobody reads twice is a verb
    /// nobody finds.
    ///
    /// Kept beside [`parse`](Self::parse) rather than derived from it because the two answer
    /// different questions — this one names the FORMS, including the flag grammar a parser
    /// expresses as control flow — and
    /// [`the_vocabulary_lists_every_verb_a_binding_takes`](self) holds them together.
    pub const VOCABULARY: [&'static str; 18] = [
        "detach-client",
        "send-prefix",
        "list-keys",
        "split-window -h|-v [-b]",
        "select-pane -L|-R|-U|-D|-t :.+",
        "swap-pane -L|-R|-U|-D",
        "resize-pane -L|-R|-U|-D [N]",
        "zoom-pane [-Z|-u]",
        "kill-pane",
        "new-window",
        "select-window -n|-p|-t <window>",
        "move-window --first|--last|-n|-p|--before [<window>]|--after <window>",
        "kill-window",
        "rename-window",
        "rename-session",
        "rename-pane",
        "switch-client -n|-p|-l|-t [<session>]",
        "confirm-before <action>",
    ];

    /// Whether carrying this out puts a QUESTION on the user's screen first.
    ///
    /// Derived here rather than listed at each surface, and read by two callers that must agree:
    /// [`parse`](Self::parse) refuses to wrap an asking action in another ask, and
    /// [`prompt::Ask::of`](crate::prompt::Ask::of) builds the question. A third arm that asks and is
    /// left out of this would compile and then nest a prompt inside a prompt — so this is the one
    /// place the property is decided, exhaustively.
    #[must_use]
    pub fn asks(&self) -> bool {
        match self {
            Self::RenameWindow
            | Self::RenameSession
            | Self::RenamePane
            | Self::MoveWindowBefore
            | Self::ConfirmBefore { .. } => true,
            // The ONE arm whose answer depends on its argument, and that is what `-t` with the
            // name left off means. It also gives `confirm-before switch-client -t` its refusal for
            // free, through the no-nested-prompt rule `parse` reads this for.
            Self::SwitchClient { ask } => matches!(ask, SwitchClientAsk::Ask),
            Self::DetachClient
            | Self::SendPrefix
            | Self::ListKeys
            | Self::SplitWindow { .. }
            | Self::SelectNextPane
            | Self::SelectPaneToward { .. }
            | Self::SwapPaneToward { .. }
            | Self::ResizePaneToward { .. }
            | Self::ZoomPane { .. }
            | Self::NewWindow
            | Self::SelectWindow { .. }
            | Self::MoveWindow { .. }
            | Self::KillPane
            | Self::KillWindow => false,
        }
    }

    /// Whether carrying this out needs a pane the CLIENT can NAME.
    ///
    /// A keystroke means "the pane I am on", and a client whose focus is not on a pane has no such
    /// pane — so an action that needs one must do NOTHING rather than act on a pane the user was
    /// not looking at. [`prompt::Ask::of`](crate::prompt::Ask::of) reads this to decide whether to
    /// open a question at all: a prompt with no subject is worse than no prompt, because the user
    /// answers it and nothing happens.
    ///
    /// **It sees through the guard**, like [`reaches`](Self::reaches) and for the same reason:
    /// `confirm-before kill-pane` needs exactly what `kill-pane` needs, and a wrapper that reported
    /// `false` would ask "Kill this pane?" with no pane to kill.
    ///
    /// Exhaustive for [`asks`](Self::asks)' reason. The verbs that act on the focused pane WITHOUT
    /// naming it are `false` here on purpose — `split-window`, `zoom-pane` and the directional
    /// arms send no id and the DAEMON resolves its own active pane, which is the authority split
    /// those arms' docs state. Only the two whose client-side signature takes a
    /// [`PaneId`](sprag_terminal::PaneId) need
    /// one here.
    #[must_use]
    pub fn needs_pane(&self) -> bool {
        match self {
            Self::RenamePane | Self::KillPane => true,
            Self::ConfirmBefore { action } => action.needs_pane(),
            // A client verb, so it acts wherever the focus is — including nowhere. A user whose
            // pointer is on the session rail must still be able to press `prefix )`.
            Self::SwitchClient { .. }
            | Self::DetachClient
            | Self::SendPrefix
            | Self::ListKeys
            | Self::SplitWindow { .. }
            | Self::SelectNextPane
            | Self::SelectPaneToward { .. }
            | Self::SwapPaneToward { .. }
            | Self::ResizePaneToward { .. }
            | Self::ZoomPane { .. }
            | Self::NewWindow
            | Self::SelectWindow { .. }
            | Self::MoveWindow { .. }
            | Self::MoveWindowBefore
            | Self::KillWindow
            | Self::RenameWindow
            | Self::RenameSession => false,
        }
    }

    /// What this action ACTS ON — the axis a reader groups a keymap by.
    ///
    /// Exhaustive on purpose, for [`asks`](Self::asks)' reason one property over: a wildcard would
    /// make some group the silent default for every verb added later, and a verb filed under the
    /// wrong subject is a verb a user does not find in the list that exists to show it to them.
    /// The compiler asks instead.
    ///
    /// [`ConfirmBefore`](Self::ConfirmBefore) takes the subject of what it GUARDS: `confirm-before
    /// kill-window` is a window verb that asks first, and filing it under the guard would put the
    /// only bound kill in the client's group, away from every other window key.
    #[must_use]
    pub fn subject(&self) -> ActionSubject {
        match self {
            Self::DetachClient | Self::SendPrefix | Self::ListKeys => ActionSubject::Client,
            // tmux's NAME says window and the subject is the pane: it divides the focused pane and
            // puts a shell in the half it opens, leaving the window's own identity untouched. The
            // spelling is inherited (see [`SplitWindow`](Self::SplitWindow)); the grouping follows
            // what the verb does.
            Self::SplitWindow { .. }
            | Self::SelectNextPane
            | Self::SelectPaneToward { .. }
            | Self::SwapPaneToward { .. }
            | Self::ResizePaneToward { .. }
            | Self::ZoomPane { .. }
            | Self::KillPane
            | Self::RenamePane => ActionSubject::Pane,
            Self::NewWindow
            | Self::SelectWindow { .. }
            | Self::MoveWindow { .. }
            | Self::MoveWindowBefore
            | Self::KillWindow
            | Self::RenameWindow => ActionSubject::Window,
            Self::RenameSession => ActionSubject::Session,
            // The CLIENT, with `detach-client` — see the arm's own doc. It changes no session; it
            // changes which one this client is looking at.
            Self::SwitchClient { .. } => ActionSubject::Client,
            Self::ConfirmBefore { action } => action.subject(),
        }
    }

    /// The first word of the canonical spelling — `split-window` for every form of the split.
    ///
    /// DERIVED from [`Display`](fmt::Display) rather than matched a second time, which is the whole
    /// point: a second list of first words is the drift [`VOCABULARY`](Self::VOCABULARY)'s own doc
    /// records (`bind-key`'s copy of the vocabulary was stale for eight rounds). One impl decides
    /// how an action spells itself, and everything that needs part of that spelling cuts it up.
    #[must_use]
    pub fn verb(&self) -> String {
        verb_of(&self.to_string()).to_owned()
    }

    /// Every verb this binding puts WITHIN REACH — its own, and a wrapper's argument too.
    ///
    /// [`verb`](Self::verb) answers what an action is called; this answers what pressing the key
    /// gets you, and the two differ for exactly one arm today. `prefix &` is
    /// `confirm-before kill-window`, so a view that counted only the outer verb would tell a user
    /// that `kill-window` is bound to nothing while the key that kills their window sits three
    /// lines above — which is what the test for it measured before this existed.
    ///
    /// Exhaustive rather than a wildcard for [`subject`](Self::subject)'s reason: the failure mode
    /// of forgetting a WRAPPER here is silent and is a lie about the table, so a second wrapper
    /// added later must be made to say what it reaches rather than defaulting to nothing.
    #[must_use]
    pub fn reaches(&self) -> Vec<String> {
        match self {
            Self::ConfirmBefore { action } => {
                let mut verbs = vec![self.verb()];
                verbs.extend(action.reaches());
                verbs
            }
            Self::DetachClient
            | Self::SendPrefix
            | Self::ListKeys
            | Self::SplitWindow { .. }
            | Self::SelectNextPane
            | Self::SelectPaneToward { .. }
            | Self::SwapPaneToward { .. }
            | Self::ResizePaneToward { .. }
            | Self::ZoomPane { .. }
            | Self::NewWindow
            | Self::SelectWindow { .. }
            | Self::MoveWindow { .. }
            | Self::MoveWindowBefore
            | Self::KillPane
            | Self::KillWindow
            | Self::RenameWindow
            | Self::RenameSession
            | Self::RenamePane
            | Self::SwitchClient { .. } => vec![self.verb()],
        }
    }

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
        let bad = |why: &str| KeyError::BadFlags {
            action: action.to_owned(),
            why: why.to_owned(),
        };
        // `confirm-before` is resolved BEFORE the flag vector is built, because its argument is a
        // whole ACTION and not a flag list: the rest of the line is the same grammar again, parsed
        // by the same function. That recursion is the point — one vocabulary, so a verb added later
        // is wrappable the day it exists, without this arm being told about it.
        if verb == "confirm-before" {
            let rest = words.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                return Err(bad(
                    "needs an action to guard, e.g. `confirm-before kill-window`",
                ));
            }
            let inner = Self::parse(&rest)?;
            if inner.asks() {
                return Err(bad(
                    "cannot guard an action that already asks a question of its own",
                ));
            }
            return Ok(Self::ConfirmBefore {
                action: Box::new(inner),
            });
        }
        let flags: Vec<&str> = words.collect();
        match verb {
            // The three argument-less client-local verbs, sharing one arm because they share one
            // grammar. `list-keys` takes none deliberately where the CLI verb of that name now
            // takes `-N`: a binding opens the view, and which view is a thing the surface decides,
            // not a thing a config file has an opinion about.
            "detach-client" | "send-prefix" | "list-keys" => {
                if !flags.is_empty() {
                    return Err(bad("takes no arguments"));
                }
                Ok(match verb {
                    "detach-client" => Self::DetachClient,
                    "send-prefix" => Self::SendPrefix,
                    _ => Self::ListKeys,
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
            "zoom-pane" => {
                let mut on = None;
                for flag in flags {
                    match flag {
                        "-Z" | "-u" => {
                            if on.is_some() {
                                return Err(bad("-Z and -u name one state; give only one"));
                            }
                            on = Some(flag == "-Z");
                        }
                        // `split-window`'s rule, and for the same reason: a keystroke acts where
                        // the user is, so the CLI verb's pane argument is the one thing a binding
                        // must not carry.
                        other => {
                            return Err(bad(&format!(
                                "{other:?} is not a flag a binding takes (a binding zooms the \
                                 FOCUSED pane, so it names none)"
                            )));
                        }
                    }
                }
                // Bare `zoom-pane` is the TOGGLE, unlike bare `split-window` which is refused: the
                // CLI verb reads it the same way, so one string means one thing at both surfaces.
                Ok(Self::ZoomPane { on })
            }
            // Matched on the whole flag vector rather than folded one flag at a time, because the
            // grammar is two shapes and not a set: `-t :.+` is TWO words that mean one thing, so a
            // per-flag loop would have to re-join them.
            "select-pane" => match flags.as_slice() {
                target if target == NEXT_PANE_TARGET => Ok(Self::SelectNextPane),
                [flag] if let Some(dir) = direction_of(flag) => Ok(Self::SelectPaneToward { dir }),
                // `split-window`'s and `zoom-pane`'s refusal, one axis over: naming two of a
                // mutually exclusive set is a typo with two readings, so neither is guessed.
                [first, second]
                    if direction_of(first).is_some() && direction_of(second).is_some() =>
                {
                    Err(bad("-L/-R/-U/-D name one direction; give only one"))
                }
                _ => Err(bad(
                    "a binding moves by DIRECTION (-L/-R/-U/-D) or to the next pane (-t :.+); \
                     the rest of tmux's target grammar is not built",
                )),
            },
            // The select's directional arm, with the two things a binding must not carry left out:
            // no `-t` target (a keystroke acts where the user is, `split-window`'s rule) and no
            // partner pane id (same rule, and the CLI verb has both).
            "swap-pane" => match flags.as_slice() {
                [flag] if let Some(dir) = direction_of(flag) => Ok(Self::SwapPaneToward { dir }),
                [first, second]
                    if direction_of(first).is_some() && direction_of(second).is_some() =>
                {
                    Err(bad("-L/-R/-U/-D name one direction; give only one"))
                }
                _ => Err(bad(
                    "a binding swaps by DIRECTION (-L/-R/-U/-D); the pane id and the partner the \
                     CLI verb takes are what a keystroke cannot carry, since it acts where the \
                     user is",
                )),
            },
            // The ONE arm that takes a number, and the one flag vector that is not a set: the
            // distance follows the direction, so a per-flag loop would have to remember which flag
            // it was reading a number for.
            "resize-pane" => match flags.as_slice() {
                [flag] if let Some(dir) = direction_of(flag) => Ok(Self::ResizePaneToward {
                    dir,
                    // The bare form is tmux's own default distance, so `resize-pane -L` means what
                    // `sprag resize-pane -L` means and what tmux's `-L` means — one string, one
                    // reading, at three surfaces.
                    cells: crate::wire::ResizeAsk::CELLS_DEFAULT,
                }),
                // BEFORE the distance arm, and the order is load-bearing: `-R` is not a number, so
                // a distance arm reached first would report the second DIRECTION as a bad distance
                // — a message about the wrong word. The test that found this is the reason the two
                // arms are written in this order rather than the order they were thought of in.
                [first, second]
                    if direction_of(first).is_some() && direction_of(second).is_some() =>
                {
                    Err(bad("-L/-R/-U/-D name one direction; give only one"))
                }
                [flag, count] if let Some(dir) = direction_of(flag) => {
                    let cells = count
                        .parse::<u16>()
                        .ok()
                        .filter(|cells| *cells > 0)
                        .ok_or_else(|| {
                            bad(&format!(
                                "{count:?} is not a distance in cells (1 or more); a key that \
                                 moved nothing is one a user cannot tell from a broken one"
                            ))
                        })?;
                    Ok(Self::ResizePaneToward { dir, cells })
                }
                // tmux's `-Z` is sprag's `zoom-pane`, and saying so is worth a sentence: a tmux
                // user's fingers know `resize-pane -Z`, and an unknown-flag message would leave
                // them looking for a flag rather than for a verb.
                ["-Z"] | ["-u"] => Err(bad(
                    "sprag spells tmux's resize-pane -Z as its own verb: bind `zoom-pane`",
                )),
                _ => Err(bad(
                    "a binding moves a boundary by DIRECTION and an optional distance in cells \
                     (-L/-R/-U/-D [N]); the pane id and the exact -x/-y rectangle the CLI verb \
                     takes are what a keystroke cannot carry",
                )),
            },
            // `select-window`'s shape one LEVEL up, plus the two arms a window ring cannot have:
            // `-l` (only a client visits things) and a bare `-t` (only an ADDRESS may be left for
            // the user to supply — `move-window --before`'s rule).
            "switch-client" => match flags.as_slice() {
                ["-n"] => Ok(Self::SwitchClient {
                    ask: SwitchClientAsk::Step(OrderStep::Next),
                }),
                ["-p"] => Ok(Self::SwitchClient {
                    ask: SwitchClientAsk::Step(OrderStep::Previous),
                }),
                ["-n", "-p"] | ["-p", "-n"] => {
                    Err(bad("-n and -p name one direction; give only one"))
                }
                ["-l"] => Ok(Self::SwitchClient {
                    ask: SwitchClientAsk::LastViewed,
                }),
                ["-t"] => Ok(Self::SwitchClient {
                    ask: SwitchClientAsk::Ask,
                }),
                ["-t", session] => Ok(Self::SwitchClient {
                    ask: SwitchClientAsk::Named((*session).to_owned()),
                }),
                // Named rather than folded into the catch-all: tmux's `-c` targets ANOTHER client,
                // which this daemon cannot do — its clients drive their own re-attach — and a user
                // who typed it is owed that fact rather than a list of the flags that do work.
                ["-c", ..] => Err(bad(
                    "sprag has no -c: a client re-attaches itself, so a binding switches the \
                     client that pressed the key and no other",
                )),
                _ => Err(bad(
                    "a binding steps along the session ring (-n/-p), goes back to the last one \
                     (-l), names one (-t <session>) or asks for one (-t)",
                )),
            },
            // The three lifecycle verbs that take nothing, refused loudly for a flag rather than
            // ignoring it: a user who wrote one meant something by it, and this vocabulary has no
            // reading for it.
            "new-window" | "kill-window" | "kill-pane" => {
                if !flags.is_empty() {
                    return Err(bad(
                        "takes no arguments (a binding acts on the session, window and pane the \
                         user is on, so it names none of them)",
                    ));
                }
                Ok(match verb {
                    "new-window" => Self::NewWindow,
                    "kill-window" => Self::KillWindow,
                    _ => Self::KillPane,
                })
            }
            // The three verbs that ASK. Each takes no argument for the same reason `new-window`
            // takes none and a stronger one besides: the name is what the user is about to type, so
            // a binding carrying one would rename everything it touches to the same string.
            "rename-window" | "rename-session" | "rename-pane" => {
                if !flags.is_empty() {
                    return Err(bad(
                        "takes no arguments (the name is what the prompt asks for, and a binding \
                         that fixed one would give everything it renames the same name)",
                    ));
                }
                Ok(match verb {
                    "rename-window" => Self::RenameWindow,
                    "rename-session" => Self::RenameSession,
                    _ => Self::RenamePane,
                })
            }
            // tmux's own spelling for this, refused with the reason rather than as an unknown verb:
            // a user pasting a line out of their `.tmux.conf` is the likeliest way anyone types
            // `command-prompt` at sprag, and "unknown action" would send them looking for a typo.
            "command-prompt" => Err(bad(
                "sprag has no command-prompt: the rename verbs ASK by themselves (bind , \
                 rename-window), deriving the question and the current name from the live session, \
                 so there is no template to substitute into",
            )),
            // `select-pane`'s shape one level up: matched on the whole flag vector, because `-t
            // <window>` is TWO words that mean one thing.
            "select-window" => match flags.as_slice() {
                ["-n"] => Ok(Self::SelectWindow {
                    ask: SelectWindowAsk::Step(OrderStep::Next),
                }),
                ["-p"] => Ok(Self::SelectWindow {
                    ask: SelectWindowAsk::Step(OrderStep::Previous),
                }),
                ["-n", "-p"] | ["-p", "-n"] => {
                    Err(bad("-n and -p name one direction; give only one"))
                }
                ["-t", window] => Ok(Self::SelectWindow {
                    ask: SelectWindowAsk::Named((*window).to_owned()),
                }),
                _ => Err(bad(
                    "a binding steps along the window ring (-n/-p) or names one window (-t                      <window>)",
                )),
            },
            // `select-window`'s shape one verb over, and the ONE arm in this vocabulary where a
            // MISSING argument is a different action rather than an error: `--before <window>`
            // fixes the anchor in the config, `--before` on its own asks for it per press. That is
            // only honest because an anchor is an address the user picks each time — a rename's
            // argument is the whole content of the act, which is why that verb has no fixed form.
            "move-window" => match flags.as_slice() {
                ["--first"] => Ok(Self::MoveWindow {
                    place: WindowPlace::First,
                }),
                ["--last"] => Ok(Self::MoveWindow {
                    place: WindowPlace::Last,
                }),
                ["-n"] => Ok(Self::MoveWindow {
                    place: WindowPlace::Step(OrderStep::Next),
                }),
                ["-p"] => Ok(Self::MoveWindow {
                    place: WindowPlace::Step(OrderStep::Previous),
                }),
                ["--before"] => Ok(Self::MoveWindowBefore),
                ["--before", window] => Ok(Self::MoveWindow {
                    place: WindowPlace::Before((*window).to_owned()),
                }),
                ["--after", window] => Ok(Self::MoveWindow {
                    place: WindowPlace::After((*window).to_owned()),
                }),
                ["--after"] => Err(bad(
                    "move-window --after needs a window to anchor to; --before on its own is the \
                     form that ASKS, and --last reaches the end with no name at all",
                )),
                _ => Err(bad(
                    "a binding moves a window to --first, --last, one place along (-n/-p), or \
                     beside a named one (--before <window> / --after <window>); --before alone asks",
                )),
            },
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
            Self::ListKeys => f.write_str("list-keys"),
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
            Self::SelectPaneToward { dir } => write!(f, "select-pane {}", flag_of(*dir)),
            Self::SwapPaneToward { dir } => write!(f, "swap-pane {}", flag_of(*dir)),
            // The distance is ALWAYS printed, even when it is the default one: `list-keys` exists
            // to answer "what does this key do", and two keys differing only in how far they move
            // must not render identically.
            Self::ResizePaneToward { dir, cells } => {
                write!(f, "resize-pane {} {cells}", flag_of(*dir))
            }
            Self::ZoomPane { on } => f.write_str(match on {
                None => "zoom-pane",
                Some(true) => "zoom-pane -Z",
                Some(false) => "zoom-pane -u",
            }),
            Self::KillPane => f.write_str("kill-pane"),
            Self::NewWindow => f.write_str("new-window"),
            Self::KillWindow => f.write_str("kill-window"),
            Self::RenameWindow => f.write_str("rename-window"),
            Self::RenameSession => f.write_str("rename-session"),
            Self::RenamePane => f.write_str("rename-pane"),
            // Rendered by rendering the action it wraps, so a nested spelling round-trips through
            // [`BoundAction::parse`] the way every other one does — `list-keys` prints what a user
            // could type back.
            Self::ConfirmBefore { action } => write!(f, "confirm-before {action}"),
            Self::SelectWindow { ask } => match ask {
                SelectWindowAsk::Step(OrderStep::Next) => f.write_str("select-window -n"),
                SelectWindowAsk::Step(OrderStep::Previous) => f.write_str("select-window -p"),
                SelectWindowAsk::Named(window) => write!(f, "select-window -t {window}"),
            },
            // The same four flags a shell takes, so a user reads a row out of `list-keys` and types
            // it back unchanged. The bare `-t` renders BARE, which is what makes the asking form
            // round-trip through `parse` rather than becoming a name-less `-t <>`.
            Self::SwitchClient { ask } => match ask {
                SwitchClientAsk::Step(OrderStep::Next) => f.write_str("switch-client -n"),
                SwitchClientAsk::Step(OrderStep::Previous) => f.write_str("switch-client -p"),
                SwitchClientAsk::LastViewed => f.write_str("switch-client -l"),
                SwitchClientAsk::Named(session) => write!(f, "switch-client -t {session}"),
                SwitchClientAsk::Ask => f.write_str("switch-client -t"),
            },
            // The FLAGS are the CLI verb's own, not a second spelling: a user reads `move-window
            // -p` out of `list-keys` and types it into a shell unchanged. `-n`/`-p` are
            // `select-window`'s two letters for the same two directions, which is the whole reason
            // the place reuses `OrderStep` rather than inventing a pair.
            Self::MoveWindow { place } => match place {
                WindowPlace::First => f.write_str("move-window --first"),
                WindowPlace::Last => f.write_str("move-window --last"),
                WindowPlace::Step(OrderStep::Next) => f.write_str("move-window -n"),
                WindowPlace::Step(OrderStep::Previous) => f.write_str("move-window -p"),
                WindowPlace::Before(window) => write!(f, "move-window --before {window}"),
                WindowPlace::After(window) => write!(f, "move-window --after {window}"),
            },
            // The ANCHORLESS form, which is what makes it ask. It must not print as
            // `move-window --before ` with a trailing space: `list-keys` output is pasted back
            // into `bind-key`, and a trailing space would parse as the same thing only by luck.
            Self::MoveWindowBefore => f.write_str("move-window --before"),
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
        self.action.clone()
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
    /// tmux's own defaults, for the actions sprag's clients have — plus ONE derived set, named as
    /// such.
    ///
    /// Verified against `tmux 3.2a`'s `list-keys -T prefix` on this machine rather than recalled:
    /// `C-b send-prefix`, `" split-window`, `% split-window -h`, `d detach-client`,
    /// `o select-pane -t :.+`, and `-r Up/Down/Left/Right select-pane -U/-D/-L/-R`. The one
    /// divergence is `"`, spelled `-v` here for the reason [`BoundAction::parse`] gives; the arrow
    /// keys carry sprag's own names for the reason the module docs give.
    ///
    /// **The four SHIFTED arrows are not tmux's and this is where that is said.** tmux's only swap
    /// defaults are `{` and `}` (`swap-pane -U` / `-D`), and there those flags mean the PREVIOUS and
    /// NEXT pane in index order — a verb sprag does not have, so those keys would carry a different
    /// meaning under a spelling a tmux user already knows. The shifted arrow is derived from the
    /// four lines above it instead: same key, same direction, moving the pane rather than the
    /// cursor.
    fn default() -> Self {
        let key = |spec: &str| KeySpec::parse(spec).expect("a default key spec is well formed");
        let bind = |spec: &str, action| Bind {
            table: KeyTable::Prefix,
            key: key(spec),
            action,
            repeat: false,
        };
        // tmux's `-r`. Split out rather than given as a fourth argument to `bind` because five of
        // the six calls below would then carry a `false` that says nothing.
        let repeating = |spec: &str, action| Bind {
            repeat: true,
            ..bind(spec, action)
        };
        // A ROOT binding: no prefix, the key acts wherever it is pressed. See the table's own
        // comment below for why three of them ship.
        let rooted = |spec: &str, action| Bind {
            table: KeyTable::Root,
            ..bind(spec, action)
        };
        let switching = |ask| BoundAction::SwitchClient { ask };
        let toward = |dir| BoundAction::SelectPaneToward { dir };
        let swapping = |dir| BoundAction::SwapPaneToward { dir };
        let sizing = |dir, cells| BoundAction::ResizePaneToward { dir, cells };
        Self {
            prefix: key("C-b"),
            repeat_time: DEFAULT_REPEAT_TIME,
            // THE ROOT TABLE SHIPS WITH TWO, and until R314 it shipped empty — which is also
            // tmux's state for keyboard keys (its default root table holds mouse bindings only,
            // measured from `list-keys -T root`). The change is deliberate and it takes NOTHING
            // from a user that was theirs: `sprag-gui` already stole exactly these three chords
            // from the pane, in a private `SessionChord` table that `list-keys` could not name and
            // a config file could not unbind. Moving them here is what makes them visible and
            // rebindable, and it gives `sprag-tui` — which had NO way to reach another session —
            // the same three keys.
            //
            // They are root rather than prefix because that is the gesture they already were, and
            // because the prefix set below carries tmux's own three for the same verb: a tmux
            // user's fingers find `prefix (` / `prefix )` / `prefix L`, and a herdr or GUI user's
            // find the chords. One vocabulary, two idioms, no second table.
            //
            // THE SIXTEEN REPEATING BINDINGS are the four arrows, the four shifted ones, and
            // `resize-pane`'s eight — which is every default tmux marks `-r` among the actions
            // this vocabulary has, plus the shifted set derived from the first four. `-r` appears
            // where tmux puts it and, for the swap, where the same argument puts it.
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
                // tmux's own key for this, and the one binding whose absence hid every other one:
                // until R308 a user's only way to learn what these keys do was to leave the
                // terminal and run `sprag list-keys` in a shell. `?` is tmux's `list-keys -N` and
                // is free in every table here.
                bind("?", BoundAction::ListKeys),
                bind("o", BoundAction::SelectNextPane),
                // tmux's `prefix z`, and herdr's `prefix+z` — the one key every multiplexer user
                // already has in their fingers. The TOGGLE form, so the same key both fills the
                // window and gives the arrangement back.
                bind("z", BoundAction::ZoomPane { on: None }),
                // `x` — tmux's key for `kill-pane`, WITH tmux's own guard, which tmux writes as
                // `confirm-before -p "kill-pane #P? (y/n)" kill-pane` (R309). The guard is not
                // decoration on the most-pressed destructive key in a multiplexer: this is the one
                // verb whose blast radius is not visible from its NAME, because a window's last
                // pane takes the window, and a session's last window takes the session. What the
                // question says is DERIVED from the live arrangement
                // ([`crate::prompt::Ask::of`]), so it names the escalation the user is actually
                // about to cause rather than the one a config file guessed at.
                bind(
                    "x",
                    BoundAction::ConfirmBefore {
                        action: Box::new(BoundAction::KillPane),
                    },
                ),
                // THE WINDOW LEVEL, on tmux's own three keys (R305). `c` is the key a tmux user
                // presses more than any other after the splits, and before this round it was
                // `Routed::Swallow` — it silently did nothing.
                //
                bind("c", BoundAction::NewWindow),
                // `&` — tmux's key for `kill-window`, WITH tmux's own guard (R306). R305 left this
                // unbound because there was no prompt surface to guard it with, and shipping a
                // destructive verb on the key a tmux user's fingers already know, without the
                // question those fingers expect, was the wrong half of that trade. The bare verb
                // stays bindable for anyone who wants it: `confirm-before` is a wrapper precisely
                // so a config can say either thing.
                bind(
                    "&",
                    BoundAction::ConfirmBefore {
                        action: Box::new(BoundAction::KillWindow),
                    },
                ),
                // THE THREE RENAMES. `,` and `$` are tmux's own keys for exactly these verbs; `P`
                // is herdr's (`prefix+shift+p`), taken because tmux has no pane-rename verb at all
                // and inheriting the other parity target's key beats inventing a third — see
                // [`BoundAction::RenamePane`].
                bind(",", BoundAction::RenameWindow),
                bind("$", BoundAction::RenameSession),
                bind("P", BoundAction::RenamePane),
                // NOT repeating, where the arrows are: tmux marks `next-window`/`previous-window`
                // `-r` and sprag does not, because a held window key walks a RING with no edge to
                // stop at — three unintended repeats put the user two windows past where they meant
                // to be, with a different pane set each time. The arrows repeat because a pane walk
                // STOPS at the arrangement's edge.
                bind(
                    "n",
                    BoundAction::SelectWindow {
                        ask: SelectWindowAsk::Step(OrderStep::Next),
                    },
                ),
                bind(
                    "p",
                    BoundAction::SelectWindow {
                        ask: SelectWindowAsk::Step(OrderStep::Previous),
                    },
                ),
                // THE WINDOW ORDER, on the three keys the two parity targets between them settle
                // (R310). Before this round the order was WALKED by the two keys above, PAINTED by
                // the GUI's window strip, and changeable by nothing anywhere in the product.
                //
                // `<` and `>` are not tmux DEFAULTS — tmux ships `swap-window` unbound — but they
                // are the binding a tmux user writes for it, and the glyphs say which way along a
                // strip drawn left to right. They are taken rather than invented for the same
                // reason `P` was: where a convention exists, inheriting it beats a third spelling.
                //
                // NOT repeating, and for the WINDOW keys' reason above rather than the arrows':
                // a held `>` walks a window past every other one and stops nowhere a user aimed
                // at. The move has ENDS, but they are the ends of a list the user cannot see the
                // whole of while the key is held.
                bind(
                    "<",
                    BoundAction::MoveWindow {
                        place: WindowPlace::Step(OrderStep::Previous),
                    },
                ),
                bind(
                    ">",
                    BoundAction::MoveWindow {
                        place: WindowPlace::Step(OrderStep::Next),
                    },
                ),
                // `.` IS tmux's own default for this verb — `command-prompt "move-window -t '%%'"`
                // — and it is the only tmux default in this table that asks a question tmux has to
                // build out of a template. Here the anchor is a WINDOW NAME rather than an index,
                // so the answer fills a typed slot and there is nothing to quote.
                bind(".", BoundAction::MoveWindowBefore),
                // THE SESSION LEVEL, on tmux's own three keys (R314). Before this round the whole
                // vocabulary stopped at the window: a user could rearrange every pane and window
                // they could see and had no way to say "my OTHER session" at all, unless they were
                // holding a mouse in `sprag-gui`.
                //
                // `(` and `)` are tmux's `switch-client -p` / `-n` verbatim, and `L` is its
                // `switch-client -l`. NOT repeating, for `prefix n`/`p`'s reason one level up and
                // more so: a held session key walks a ring whose every step swaps the user's whole
                // screen, and three unintended repeats land them two workspaces from where they
                // meant to be.
                //
                // tmux's `prefix s` is `choose-tree`, an interactive list, and it is deliberately
                // LEFT UNBOUND: `switch-client -t` (the asking form) is a NAME PROMPT, so putting
                // it on those fingers would answer a gesture with a different one. It stays
                // bindable for anyone who wants it — R305's rule for `kill-window`, applied to a
                // key whose meaning rather than whose guard is missing.
                bind("(", switching(SwitchClientAsk::Step(OrderStep::Previous))),
                bind(")", switching(SwitchClientAsk::Step(OrderStep::Next))),
                bind("L", switching(SwitchClientAsk::LastViewed)),
                // TWO of the three chords `sprag-gui` held privately until R314, in the ONE table
                // now. Same keys, same acts — what changes is that `sprag list-keys` names them,
                // `prefix ?` paints them, a config file can unbind them, and `sprag-tui` has them.
                //
                // THE THIRD, `Ctrl+Shift+L`, IS DELIBERATELY NOT HERE, and the reason is a property
                // of this vocabulary rather than an oversight. `KeySpec::matches` masks Shift off a
                // CHARACTER key (R306: a character already carries its shift state, and pretending
                // otherwise stopped `prefix %` firing in the GUI on a real keyboard) and folds an
                // ASCII letter's case under `Ctrl` (a terminal sends `Ctrl-L` as the C0 byte
                // `0x0c`, which cannot say which case was typed). So `C-S-L` is not a key this
                // vocabulary can tell from `C-l`, and binding it would take the shell's
                // clear-screen from every pane in every client. THE VERB IS NOT LOST — `prefix L`
                // above is tmux's own key for it and is exact, and `switch-client -l` binds to
                // anything a user prefers. The GUI's private chord could tell the two apart because
                // winit reports the modifier; `sprag-tui` reads a byte that does not, and a
                // vocabulary both clients share cannot rest on what only one of them can see.
                //
                // The two PAGE chords have no such problem: `PageUp`/`PageDown` are NAMED keys, so
                // Shift is compared EXACTLY, and `C-S-PageUp` stays disjoint from the Ctrl-only and
                // Shift-only page chords `sprag-gui` keeps for its own scroll and focus-cycle.
                rooted(
                    "C-S-PageDown",
                    switching(SwitchClientAsk::Step(OrderStep::Next)),
                ),
                rooted(
                    "C-S-PageUp",
                    switching(SwitchClientAsk::Step(OrderStep::Previous)),
                ),
                // tmux's own order (`Up Down Left Right`), and its own `-r`: holding the prefix
                // table open is what makes `prefix Left Left Left` walk three panes instead of one.
                repeating("ArrowUp", toward(PaneDir::Up)),
                repeating("ArrowDown", toward(PaneDir::Down)),
                repeating("ArrowLeft", toward(PaneDir::Left)),
                repeating("ArrowRight", toward(PaneDir::Right)),
                // THE SAME ARROW, WITH SHIFT: take the pane with you instead of leaving it.
                //
                // Derived from the table above rather than copied from either rival, and that is
                // the argument for it. tmux's ONLY swap defaults are `{` and `}` — `swap-pane -U`
                // and `-D`, which there mean the PREVIOUS and NEXT pane in index order, not up and
                // down; sprag has no index-order swap, so binding those keys would give a tmux
                // user's fingers a different verb under the same spelling. herdr binds
                // `prefix+shift+h/j/k/l`, a vim vocabulary this map does not otherwise speak.
                // Shift-plus-the-focus-key is instead the relationship every tiling window manager
                // already uses for move-versus-focus, and it composes with the four keys sprag has
                // already chosen.
                //
                // `-r` for the arrows' own reason: moving a pane three cells is three presses, and
                // holding the prefix table open is what makes them one gesture.
                repeating("S-ArrowUp", swapping(PaneDir::Up)),
                repeating("S-ArrowDown", swapping(PaneDir::Down)),
                repeating("S-ArrowLeft", swapping(PaneDir::Left)),
                repeating("S-ArrowRight", swapping(PaneDir::Right)),
                // THE EIGHT `-r` DEFAULTS THE COMMENT ABOVE SAID THIS VOCABULARY HAD NO VERB FOR,
                // and they are tmux's own keys, flags and distances: `C-Up/Down/Left/Right` move
                // the boundary one cell and `M-` the same four move it five
                // (`tmux 3.2a`, `list-keys -T prefix`, read on this machine).
                //
                // Both families are `-r` there and here, and this is the verb that needs it most:
                // a boundary is dragged to where it looks right, which is a dozen presses, not one.
                // The two distances exist for the same gesture at two speeds — coarse first, then
                // one cell at a time — which is the whole reason the arm carries a NUMBER.
                //
                // They sit under the prefix rather than in the root table even though `C-Arrow` is
                // free there, because a root binding takes the chord away from the program in the
                // pane for good, and `C-Left`/`C-Right` are word-motion in readline.
                repeating("C-ArrowUp", sizing(PaneDir::Up, 1)),
                repeating("C-ArrowDown", sizing(PaneDir::Down, 1)),
                repeating("C-ArrowLeft", sizing(PaneDir::Left, 1)),
                repeating("C-ArrowRight", sizing(PaneDir::Right, 1)),
                repeating("M-ArrowUp", sizing(PaneDir::Up, 5)),
                repeating("M-ArrowDown", sizing(PaneDir::Down, 5)),
                repeating("M-ArrowLeft", sizing(PaneDir::Left, 5)),
                repeating("M-ArrowRight", sizing(PaneDir::Right, 5)),
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
        self.bound(table, name, mods)
            .map(|bind| bind.action.clone())
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

    /// What a user PRESSES to reach `bind` — `C-b z` for a prefix binding, `F1` for a root one.
    ///
    /// The literal chord, with this keymap's own prefix spelled out, rather than the word `prefix`:
    /// a reader with a rebound prefix must see their own key, and a view that said `prefix z` would
    /// be asking them to look the prefix up somewhere else. That is the failure `sprag list-keys`
    /// already avoids by printing the prefix on its first line — R235's `send-prefix` stranded on an
    /// abandoned key was invisible until the two were read together.
    ///
    /// Here rather than at a surface because every surface that shows a chord must show the same
    /// one: the help view, the CLI's notes form and the GUI palette's hint column all call this.
    #[must_use]
    pub fn chord(&self, bind: &Bind) -> String {
        match bind.table() {
            KeyTable::Prefix => format!("{} {}", self.prefix, bind.key()),
            KeyTable::Root => bind.key().to_string(),
        }
    }

    /// The chord that reaches `action`, or [`None`] when no key does.
    ///
    /// The FIRST such binding in reading order when several reach it, which is what a hint column
    /// wants: one chord to teach, chosen the same way every time rather than by whichever the
    /// caller's own iteration happened to reach.
    #[must_use]
    pub fn chord_of(&self, action: &BoundAction) -> Option<String> {
        self.binds
            .iter()
            .find(|bind| &bind.action == action)
            .map(|bind| self.chord(bind))
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
            action: bind.action.clone(),
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
#[derive(Clone, PartialEq, Eq, Debug)]
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

    /// A lone ASCII letter under `Ctrl` compares case-insensitively, because a terminal chooses the
    /// case there: the C0 byte for `Ctrl-B` decodes lowercase while a `CSI u` terminal reports the
    /// layout's case. WITHOUT `Ctrl` the case is the character, so `d` and `D` are two keys.
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
            // All four directions, because the flag table is read in BOTH directions (a flag to a
            // `PaneDir` on the way in, the same table back on the way out) and a round trip is what
            // pins that the two readings are of one table rather than of two that agree today.
            (
                "select-pane -L",
                BoundAction::SelectPaneToward { dir: PaneDir::Left },
            ),
            (
                "select-pane -R",
                BoundAction::SelectPaneToward {
                    dir: PaneDir::Right,
                },
            ),
            (
                "select-pane -U",
                BoundAction::SelectPaneToward { dir: PaneDir::Up },
            ),
            (
                "select-pane -D",
                BoundAction::SelectPaneToward { dir: PaneDir::Down },
            ),
            // All four again for the SWAP, and the point of asserting the set twice is that the two
            // verbs must not collapse into one: they share a flag table and a direction type, so a
            // parse that fell through to the select would round-trip its own answer and pass.
            (
                "swap-pane -L",
                BoundAction::SwapPaneToward { dir: PaneDir::Left },
            ),
            (
                "swap-pane -R",
                BoundAction::SwapPaneToward {
                    dir: PaneDir::Right,
                },
            ),
            (
                "swap-pane -U",
                BoundAction::SwapPaneToward { dir: PaneDir::Up },
            ),
            (
                "swap-pane -D",
                BoundAction::SwapPaneToward { dir: PaneDir::Down },
            ),
            // All three states of the zoom, because the bare form is the TOGGLE here — the one
            // place this vocabulary reads a bare verb as a meaning rather than refusing it, and the
            // round trip is what pins that the three do not collapse into one another.
            ("zoom-pane", BoundAction::ZoomPane { on: None }),
            ("zoom-pane -Z", BoundAction::ZoomPane { on: Some(true) }),
            ("zoom-pane -u", BoundAction::ZoomPane { on: Some(false) }),
            // THE WINDOW LEVEL (R305). Both steps AND a named window, because the three arms print
            // through one `Display` and a parse that read `-n` and `-p` as the same step would
            // round-trip its own answer.
            ("kill-pane", BoundAction::KillPane),
            ("new-window", BoundAction::NewWindow),
            ("kill-window", BoundAction::KillWindow),
            (
                "select-window -n",
                BoundAction::SelectWindow {
                    ask: SelectWindowAsk::Step(OrderStep::Next),
                },
            ),
            (
                "select-window -p",
                BoundAction::SelectWindow {
                    ask: SelectWindowAsk::Step(OrderStep::Previous),
                },
            ),
            (
                "select-window -t logs",
                BoundAction::SelectWindow {
                    ask: SelectWindowAsk::Named("logs".to_owned()),
                },
            ),
            // THE VERBS THAT ASK (R306). The three renames take no argument at all, so what a round
            // trip pins here is that none of them GREW one: a `Display` that printed the name it was
            // about would produce a string this parser refuses.
            ("rename-window", BoundAction::RenameWindow),
            ("rename-session", BoundAction::RenameSession),
            ("rename-pane", BoundAction::RenamePane),
            // The GUARD, whose spelling contains another action's — and this round trip is
            // load-bearing rather than tidy: `sprag-gui` holds a guarded action AS this string
            // (`confirm::Guarded::Bound`, because a reactive `Signal` must serialize and a
            // `BoundAction` carries three types from two crates that have no serde), so a `Display`
            // that drifted from `parse` would leave a user's `y` performing nothing at all.
            (
                "confirm-before kill-window",
                BoundAction::ConfirmBefore {
                    action: Box::new(BoundAction::KillWindow),
                },
            ),
            (
                "confirm-before split-window -h",
                BoundAction::ConfirmBefore {
                    action: Box::new(BoundAction::SplitWindow {
                        dir: SplitDir::Horizontal,
                        before: false,
                    }),
                },
            ),
        ];
        for (text, action) in cases {
            assert_eq!(BoundAction::parse(text), Ok(action.clone()), "{text:?}");
            assert_eq!(
                action.to_string(),
                text,
                "and prints back as it was written"
            );
        }
    }

    /// The three ASKING verbs take no argument, a guard refuses to guard another question, and
    /// tmux's `command-prompt` is refused with the reason rather than as an unknown verb.
    ///
    /// One test for the three refusals because they are one decision seen from three sides: a name
    /// is what the PROMPT asks for, so it cannot be in the binding — which is why there is no
    /// `command-prompt` to put it in, and why a guard wrapping a prompt would be asking twice.
    #[test]
    fn the_asking_verbs_carry_no_name_and_no_prompt_guards_a_prompt() {
        for action in ["rename-window main", "rename-session x", "rename-pane -t 3"] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { why, .. })
                    if why.contains("the name is what the prompt asks for")),
                "{action:?} must be refused with the reason",
            );
        }
        // A guard on a verb that asks would put a question on top of a question. The MOVE prompt
        // is here because it is the only asking verb whose asking is decided by an ABSENT argument
        // — `move-window --before logs` is guardable and `move-window --before` is not — so it is
        // the one arm where `asks` can be got wrong without any other test noticing.
        for action in [
            "confirm-before rename-window",
            "confirm-before move-window --before",
            "confirm-before confirm-before kill-window",
        ] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { why, .. })
                    if why.contains("already asks a question")),
                "{action:?} must be refused",
            );
        }
        assert!(
            matches!(BoundAction::parse("confirm-before"), Err(KeyError::BadFlags { why, .. })
                if why.contains("needs an action to guard")),
        );
        // A pasted tmux line is told WHY, not that its verb is unknown — `UnknownAction` would send
        // the user looking for a typo in a spelling that is correct for tmux.
        let pasted = BoundAction::parse("command-prompt -I \"#W\" \"rename-window '%%'\"");
        assert!(
            matches!(&pasted, Err(KeyError::BadFlags { why, .. })
                if why.contains("rename verbs ASK by themselves")),
            "{pasted:?}",
        );
        // THE CONTROL: the verbs themselves parse bare, so the refusals above are about the
        // ARGUMENT and not about the vocabulary missing the verb.
        for action in ["rename-window", "rename-session", "rename-pane"] {
            assert!(BoundAction::parse(action).is_ok(), "{action:?} parses bare");
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
        // The zoom takes the same two refusals: a pane a keystroke cannot mean, and two flags that
        // name one state. Its BARE form is deliberately NOT among them — see the round-trip test.
        for action in ["zoom-pane 3", "zoom-pane -Z -u", "zoom-pane -x"] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { .. })),
                "{action:?} should be refused",
            );
        }
        let named = BoundAction::parse("zoom-pane 3").expect_err("a binding names no pane");
        assert!(
            named.to_string().contains("FOCUSED pane"),
            "and it says where a binding acts: {named}",
        );
    }

    /// `select-pane` takes TWO shapes and nothing between them: the four directional flags, and the
    /// one `-t` target. An unbuilt target form is refused with what IS built, rather than promising
    /// a grammar sprag does not have.
    #[test]
    fn a_directional_flag_or_the_next_pane_target_and_nothing_else() {
        assert_eq!(
            BoundAction::parse("select-pane -t :.+"),
            Ok(BoundAction::SelectNextPane)
        );
        assert_eq!(
            BoundAction::parse("select-pane -U"),
            Ok(BoundAction::SelectPaneToward { dir: PaneDir::Up })
        );
        for action in [
            "select-pane",
            "select-pane -t :.-",
            "select-pane -t :=2",
            // A direction and the next-pane target name two different questions, so a line asking
            // both is a typo with no obvious reading — the rule `split-window -h -v` already has.
            "select-pane -L -t :.+",
            // ...and a pane ID is the argument no binding may carry, here as everywhere: a
            // keystroke acts where the user is.
            "select-pane 3",
            "select-pane -x",
        ] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { .. })),
                "{action:?} should be refused",
            );
        }
        // Two directions get their OWN sentence rather than the general one, because the mistake is
        // legible: the user knows the flags and named two.
        let both = BoundAction::parse("select-pane -L -R").expect_err("one direction only");
        assert!(
            both.to_string().contains("give only one"),
            "and it says which mistake it was: {both}",
        );

        // The SWAP takes ONE of those two shapes — the directions — and the refusals say so. Its
        // `-t :.+` is not "unbuilt but coming": a swap with the next pane in PAINT order is a
        // different verb from a swap with the pane beside you, and this vocabulary has only the
        // second.
        assert_eq!(
            BoundAction::parse("swap-pane -U"),
            Ok(BoundAction::SwapPaneToward { dir: PaneDir::Up })
        );
        for action in [
            "swap-pane",
            "swap-pane -t :.+",
            // The partner and the origin the CLI verb takes: both are ids, and a keystroke acts
            // where the user is.
            "swap-pane 3",
            "swap-pane 3 -L",
            "swap-pane -L -t :.+",
            "swap-pane -x",
        ] {
            assert!(
                matches!(BoundAction::parse(action), Err(KeyError::BadFlags { .. })),
                "{action:?} should be refused",
            );
        }
        let swap_both = BoundAction::parse("swap-pane -L -R").expect_err("one direction only");
        assert!(
            swap_both.to_string().contains("give only one"),
            "and it says which mistake it was: {swap_both}",
        );
        let carried = BoundAction::parse("swap-pane 3").expect_err("a binding names no pane");
        assert!(
            carried.to_string().contains("where the user is"),
            "and it says where a binding acts: {carried}",
        );
    }

    /// **The vocabulary a user is shown is the vocabulary the parser has.**
    ///
    /// Two surfaces print [`BoundAction::VOCABULARY`] and neither re-spells it — the CLI's own copy
    /// was stale for eight rounds before it became one const, and a second list is checked by
    /// nothing. This is what checks the one that is left, in both directions: every listed form
    /// names a verb `parse` accepts, and every action `parse` can PRODUCE prints back under a
    /// listed verb.
    ///
    /// REVERT-PROOF: drop any entry from the const and the second loop fails on that variant; add
    /// one for a verb nothing implements and the first loop fails on it.
    /// How many arms [`BoundAction`] has. Bumped by hand, and [`arm_of`] is what makes that safe:
    /// a variant added without touching this fails to compile there.
    const ARMS: usize = 20;

    /// Which arm a value is, as an index — an EXHAUSTIVE match, and the only reason the census
    /// below is a check rather than a list somebody maintains.
    ///
    /// A twentieth variant stops this compiling, and adding its line here makes
    /// [`one_of_every_action`] fail until an instance of it is written down. That is the pair the
    /// vocabulary, the subject and the reach tests all rest on: three properties that must hold for
    /// EVERY action, checked against a census the compiler forces to stay complete.
    fn arm_of(action: &BoundAction) -> usize {
        match action {
            BoundAction::DetachClient => 0,
            BoundAction::SendPrefix => 1,
            BoundAction::ListKeys => 2,
            BoundAction::SplitWindow { .. } => 3,
            BoundAction::SelectNextPane => 4,
            BoundAction::SelectPaneToward { .. } => 5,
            BoundAction::SwapPaneToward { .. } => 6,
            BoundAction::ResizePaneToward { .. } => 7,
            BoundAction::ZoomPane { .. } => 8,
            BoundAction::KillPane => 9,
            BoundAction::NewWindow => 10,
            BoundAction::SelectWindow { .. } => 11,
            BoundAction::MoveWindow { .. } => 12,
            BoundAction::MoveWindowBefore => 13,
            BoundAction::KillWindow => 14,
            BoundAction::RenameWindow => 15,
            BoundAction::RenameSession => 16,
            BoundAction::RenamePane => 17,
            BoundAction::SwitchClient { .. } => 18,
            BoundAction::ConfirmBefore { .. } => 19,
        }
    }

    /// One value of every arm — the census the whole-vocabulary tests walk.
    ///
    /// Panics if it is not complete, so a caller never silently tests eighteen of nineteen arms.
    fn one_of_every_action() -> Vec<BoundAction> {
        let every = vec![
            BoundAction::DetachClient,
            BoundAction::SendPrefix,
            BoundAction::ListKeys,
            BoundAction::SplitWindow {
                dir: SplitDir::Horizontal,
                before: false,
            },
            BoundAction::SelectNextPane,
            BoundAction::SelectPaneToward { dir: PaneDir::Left },
            BoundAction::SwapPaneToward { dir: PaneDir::Up },
            BoundAction::ResizePaneToward {
                dir: PaneDir::Right,
                cells: 5,
            },
            BoundAction::ZoomPane { on: None },
            BoundAction::KillPane,
            BoundAction::NewWindow,
            BoundAction::SelectWindow {
                ask: SelectWindowAsk::Step(OrderStep::Next),
            },
            BoundAction::MoveWindow {
                place: WindowPlace::Step(OrderStep::Previous),
            },
            BoundAction::MoveWindowBefore,
            BoundAction::KillWindow,
            BoundAction::RenameWindow,
            BoundAction::RenameSession,
            BoundAction::RenamePane,
            BoundAction::SwitchClient {
                ask: SwitchClientAsk::Step(OrderStep::Next),
            },
            BoundAction::ConfirmBefore {
                action: Box::new(BoundAction::KillWindow),
            },
        ];
        let mut seen: Vec<usize> = every.iter().map(arm_of).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen,
            (0..ARMS).collect::<Vec<_>>(),
            "the census is not one of every arm",
        );
        every
    }

    /// Every action has a subject, and it is one of the four the view renders.
    ///
    /// The array `ActionSubject::ALL` is a literal no compiler checks — the same residual
    /// `PaneDir::ALL` carries — so this is what would catch a fifth subject added to the enum and
    /// left out of it: a group that exists and renders nowhere.
    #[test]
    fn every_action_has_a_subject_the_view_renders() {
        for action in one_of_every_action() {
            let subject = action.subject();
            assert!(
                ActionSubject::ALL.contains(&subject),
                "{action} is filed under {subject}, which the view has no section for",
            );
        }
    }

    /// Every action reaches at least its own verb, and every verb it reaches is in the vocabulary.
    ///
    /// The second half is the one that matters: a wrapper reporting a verb nobody is told about
    /// would put a row in the view that `bind-key` refuses.
    #[test]
    fn every_verb_an_action_reaches_is_one_the_vocabulary_names() {
        for action in one_of_every_action() {
            let reached = action.reaches();
            assert!(
                reached.contains(&action.verb()),
                "{action} does not reach its own verb",
            );
            for verb in reached {
                assert!(
                    BoundAction::VOCABULARY
                        .iter()
                        .any(|form| verb_of(form) == verb),
                    "{action} reaches {verb:?}, which the vocabulary does not name",
                );
            }
        }
    }

    /// The guard reaches two verbs where every other action reaches one.
    #[test]
    fn a_guard_reaches_what_it_guards() {
        let guarded = BoundAction::ConfirmBefore {
            action: Box::new(BoundAction::KillWindow),
        };
        assert_eq!(
            guarded.reaches(),
            vec!["confirm-before".to_owned(), "kill-window".to_owned()]
        );
        assert_eq!(guarded.subject(), BoundAction::KillWindow.subject());
    }

    /// A hint column asks for the chord in force and gets the first one, or nothing.
    #[test]
    fn a_chord_can_be_looked_up_by_the_action_it_runs() {
        let mut keymap = Keymap::default();
        assert_eq!(
            keymap.chord_of(&BoundAction::ZoomPane { on: None }),
            Some("C-b z".to_owned())
        );
        assert_eq!(
            keymap.chord_of(&BoundAction::ZoomPane { on: Some(true) }),
            None,
            "the toggle is bound and the explicit form is not; they must not be confused",
        );
        keymap.set_prefix("C-a").expect("C-a is a key");
        assert_eq!(
            keymap.chord_of(&BoundAction::ZoomPane { on: None }),
            Some("C-a z".to_owned()),
            "a hint must teach the chord this user actually has",
        );
        keymap.unbind(KeyTable::Prefix, "z").expect("z is bound");
        assert_eq!(keymap.chord_of(&BoundAction::ZoomPane { on: None }), None);
    }

    #[test]
    fn the_vocabulary_lists_every_verb_a_binding_takes() {
        for form in BoundAction::VOCABULARY {
            let verb = form.split_whitespace().next().expect("a form names a verb");
            assert!(
                !matches!(
                    BoundAction::parse(verb),
                    Err(KeyError::UnknownAction(_)) | Err(KeyError::UnknownKey(_))
                ),
                "{verb:?} is listed but is not a verb the parser has",
            );
        }
        for action in one_of_every_action() {
            let printed = action.to_string();
            let verb = printed.split_whitespace().next().expect("an action prints");
            assert!(
                BoundAction::VOCABULARY
                    .iter()
                    .any(|form| form.split_whitespace().next() == Some(verb)),
                "{printed:?} is a binding nobody is told about",
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
            // The list is the ONLY place a user learns what a binding can say, so a verb that
            // exists and is absent from it is a verb nobody finds.
            "zoom-pane",
        ] {
            assert!(
                message.contains(known),
                "{message:?} should mention {known}"
            );
        }
    }

    /// Every way the WINDOW verbs can be written wrong, each refused with a sentence that says what
    /// the vocabulary actually is — and the three CONTROLS that keep the test from passing
    /// vacuously (the well-formed spellings still parse).
    ///
    /// REVERT-PROOF: accept a flag on `new-window` and the first row passes silently; fold `-n -p`
    /// into "the last one wins" and the ambiguity row stops refusing, which is the shape
    /// `select-pane` and `zoom-pane` already refuse one level down.
    #[test]
    fn the_window_verbs_refuse_what_a_keystroke_cannot_carry() {
        let bad = |text: &str| match BoundAction::parse(text) {
            Err(KeyError::BadFlags { why, .. }) => why,
            other => panic!("{text:?} should be refused for its flags, got {other:?}"),
        };
        // A keystroke acts on the window the user is on, so neither verb takes a target.
        assert!(bad("new-window logs").contains("takes no arguments"));
        assert!(bad("kill-window -t logs").contains("takes no arguments"));
        // Two directions is a typo with two readings, so neither is guessed.
        assert!(bad("select-window -n -p").contains("give only one"));
        // A `-t` with nothing after it names no window.
        assert!(bad("select-window -t").contains("names one window"));
        assert!(bad("select-window").contains("steps along the window ring"));

        // THE CONTROLS: the well-formed spellings parse, so the refusals above are about the
        // grammar rather than about a verb the parser never learned.
        for good in [
            "new-window",
            "kill-window",
            "select-window -n",
            "select-window -p",
            "select-window -t logs",
        ] {
            assert!(BoundAction::parse(good).is_ok(), "{good:?} must parse");
        }
    }

    /// The ONE arm that carries a NUMBER: it parses, it round-trips, it refuses a distance that is
    /// not one, and the bare form is tmux's own default of a single cell.
    ///
    /// The ROUND TRIP is the load-bearing half. `list-keys` prints what a user could type back, and
    /// a distance dropped from the rendering would make every `M-` row print as its `C-` twin —
    /// four keys that look identical in the one place a user goes to find out what a key does.
    #[test]
    fn the_resize_binding_carries_a_distance_and_round_trips() {
        let parsed = |spec: &str| BoundAction::parse(spec).expect("a well-formed resize binding");
        assert_eq!(
            parsed("resize-pane -L"),
            BoundAction::ResizePaneToward {
                dir: PaneDir::Left,
                cells: 1,
            },
            "the bare form is tmux's own default distance, so one string has one reading here, in \
             `sprag resize-pane -L`, and in tmux",
        );
        assert_eq!(
            parsed("resize-pane -D 5"),
            BoundAction::ResizePaneToward {
                dir: PaneDir::Down,
                cells: 5,
            },
        );
        for spelling in [
            "resize-pane -L 1",
            "resize-pane -R 12",
            "resize-pane -U 5",
            "resize-pane -D 200",
        ] {
            assert_eq!(
                parsed(spelling).to_string(),
                spelling,
                "what `list-keys` prints must parse back to the same action",
            );
        }
        // The bare form renders WITH the distance it means, which is the one place the round trip
        // is not the identity — and the honest direction to break it in.
        assert_eq!(parsed("resize-pane -L").to_string(), "resize-pane -L 1");

        let bad = |spec: &str| match BoundAction::parse(spec) {
            Err(KeyError::BadFlags { why, .. }) => why,
            other => panic!("{spec:?} must be refused as bad flags, got {other:?}"),
        };
        assert!(
            bad("resize-pane -L 0").contains("cannot tell from a broken one"),
            "a key that moves nothing is refused rather than accepted as a no-op",
        );
        assert!(bad("resize-pane -L x").contains("not a distance in cells"));
        assert!(bad("resize-pane -L -3").contains("not a distance in cells"));
        assert!(bad("resize-pane -L -R").contains("give only one"));
        assert!(bad("resize-pane").contains("moves a boundary by DIRECTION"));
        assert!(bad("resize-pane -x 40").contains("moves a boundary by DIRECTION"));
        // tmux's own spelling of the ZOOM, which sprag has as a verb of its own: a user whose
        // fingers know `resize-pane -Z` is sent to the verb rather than to the flag list.
        assert!(bad("resize-pane -Z").contains("bind `zoom-pane`"));

        // It does NOT ask, so `confirm-before` may guard it — the property `asks` decides once for
        // every arm, checked here because a new arm is exactly when it can be got wrong.
        assert!(!parsed("resize-pane -L 5").asks());
        // ...and the ANCHORED move is guardable where the anchorless one above is not, which is the
        // control that the refusal is about the ASK and not about the verb.
        assert!(!parsed("move-window --before logs").asks());
        assert!(parsed("move-window --before").asks());
        assert!(
            BoundAction::parse("confirm-before move-window --before logs").is_ok(),
            "a move with its anchor fixed asks nothing, so a guard may wrap it",
        );
        assert_eq!(
            BoundAction::parse("confirm-before resize-pane -L 5").expect("guardable"),
            BoundAction::ConfirmBefore {
                action: Box::new(parsed("resize-pane -L 5")),
            },
        );
    }

    /// The defaults ARE tmux's table for the actions sprag's clients have — and the rows that are
    /// NOT tmux's are named where they are added, which is why this asserts the whole list in order.
    ///
    /// The REPEAT flag is asserted with each row rather than in a test of its own, because `-r` is
    /// half of what a default binding IS: tmux's four arrows repeat and its other five do not, and
    /// a table that got the keys right and the flags wrong would still be the wrong table.
    ///
    /// The three `C-S-` rows are ROOT bindings (R314) and print without a prefix for that reason;
    /// `the_root_table_holds_exactly_the_session_chords` is what says which TABLE they are in,
    /// since this projection cannot show it.
    #[test]
    fn the_defaults_are_tmuxs_table() {
        let keymap = Keymap::default();
        assert_eq!(keymap.prefix().to_string(), "C-b");
        let printed: Vec<String> = keymap
            .binds()
            .map(|bind| {
                format!(
                    "{}{} {}",
                    if bind.repeats() { "-r " } else { "" },
                    bind.key(),
                    bind.action()
                )
            })
            .collect();
        assert_eq!(
            printed,
            vec![
                "C-b send-prefix",
                "\" split-window -v",
                "% split-window -h",
                "d detach-client",
                "? list-keys",
                "o select-pane -t :.+",
                // tmux's KEY, spelled with sprag's own verb: tmux says `resize-pane -Z` and sprag's
                // shell says `zoom-pane`, and this table is parsed from the string the shell takes.
                "z zoom-pane",
                // tmux's `x`, WITH tmux's own guard (R309) — tmux writes it
                // `confirm-before -p "kill-pane #P? (y/n)" kill-pane`. The guard is not politeness
                // on this key: a window's last pane takes the WINDOW, and its session's last window
                // takes the SESSION, so the one verb here whose blast radius its name does not
                // carry is also the one a user presses most often.
                "x confirm-before kill-pane",
                // tmux's THREE WINDOW keys (R305), the first rows in this table that reach past the
                // pane: `c` creates a window and selects it, `n`/`p` walk the ring. tmux's own keys
                // and tmux's own verbs.
                //
                // NOT `-r`, where tmux marks its `next-window`/`previous-window` so: a held window
                // key walks a ring with no edge to stop at, so three unintended repeats leave the
                // user two windows away with a different pane set. The arrows above repeat because
                // a pane walk STOPS at the arrangement's edge — the flag follows the shape of what
                // is being walked, which is why it is not simply copied from tmux row by row.
                //
                "c new-window",
                // tmux's `&`, WITH tmux's own guard (R306). R305 left this key unbound because
                // there was no prompt surface to guard it with; there is one now, and the bare verb
                // stays bindable for anyone who wants no question.
                "& confirm-before kill-window",
                // THE THREE RENAMES (R306) — the first rows here whose verb cannot be carried out
                // by the keystroke alone, because a name is a string a key does not carry. `,` and
                // `$` are tmux's own keys; `P` is herdr's, taken because tmux has no pane-rename
                // verb to inherit a key from.
                ", rename-window",
                "$ rename-session",
                "P rename-pane",
                "n select-window -n",
                "p select-window -p",
                "< move-window -p",
                "> move-window -n",
                ". move-window --before",
                // tmux's three session keys (R314), then the two ROOT chords `sprag-gui` used
                // to hold privately. The chords print without a prefix because they need none.
                "( switch-client -p",
                ") switch-client -n",
                "L switch-client -l",
                "C-S-PageDown switch-client -n",
                "C-S-PageUp switch-client -p",
                // tmux's four `-r` rows, read from `list-keys -T prefix` on tmux 3.2a. Its own
                // spelling is `Up`/`Down`/`Left`/`Right`; sprag's key vocabulary is the WIRE's, so
                // one keystroke has one name across the config, the CLI and both frontends.
                "-r ArrowUp select-pane -U",
                "-r ArrowDown select-pane -D",
                "-r ArrowLeft select-pane -L",
                "-r ArrowRight select-pane -R",
                // NOT tmux's — DERIVED from the four above, and the only rows in this table that
                // are. tmux binds `{` / `}` to `swap-pane -U` / `-D`, which there mean the previous
                // and next pane in INDEX order; sprag has no index-order swap, so those keys would
                // mean something else under a spelling a tmux user already knows. Shift-plus-the-
                // focus-key is the move-versus-focus relationship every tiling window manager uses.
                "-r S-ArrowUp swap-pane -U",
                "-r S-ArrowDown swap-pane -D",
                "-r S-ArrowLeft swap-pane -L",
                "-r S-ArrowRight swap-pane -R",
                // tmux's OTHER eight `-r` rows (R307) — its keys, its flags and its two distances,
                // read from the same `list-keys -T prefix`. Until this round they were the one
                // family of tmux defaults this vocabulary had no verb for, which the table's own
                // comment said in so many words.
                //
                // The DISTANCE is printed even where it is the default, because two keys that
                // differ only in how far they move must not render identically in `list-keys`.
                "-r C-ArrowUp resize-pane -U 1",
                "-r C-ArrowDown resize-pane -D 1",
                "-r C-ArrowLeft resize-pane -L 1",
                "-r C-ArrowRight resize-pane -R 1",
                "-r M-ArrowUp resize-pane -U 5",
                "-r M-ArrowDown resize-pane -D 5",
                "-r M-ArrowLeft resize-pane -L 5",
                "-r M-ArrowRight resize-pane -R 5",
            ],
        );
        // The MODIFIER is what tells the four families apart, and it is asserted as a fact about
        // the lookup rather than as a rendering: a bare arrow must still SELECT. Without this a key
        // vocabulary that dropped the modifier would print the table above and bind SIXTEEN rows
        // onto four keys, with the first match winning silently.
        //
        // All four are checked on ONE key, which is what makes the check discriminating: the four
        // rows differ in the modifier alone, so a lookup that compared anything less would answer
        // the same action for every one of them.
        let pressed = |mods: Modifiers| keymap.action(KeyTable::Prefix, "ArrowLeft", mods);
        let with = |apply: fn(&mut Modifiers)| {
            let mut mods = Modifiers::default();
            apply(&mut mods);
            mods
        };
        assert_eq!(
            pressed(Modifiers::default()),
            Some(BoundAction::SelectPaneToward { dir: PaneDir::Left }),
        );
        assert_eq!(
            pressed(with(|mods| mods.shift = true)),
            Some(BoundAction::SwapPaneToward { dir: PaneDir::Left }),
        );
        assert_eq!(
            pressed(with(|mods| mods.ctrl = true)),
            Some(BoundAction::ResizePaneToward {
                dir: PaneDir::Left,
                cells: 1,
            }),
        );
        assert_eq!(
            pressed(with(|mods| mods.alt = true)),
            Some(BoundAction::ResizePaneToward {
                dir: PaneDir::Left,
                cells: 5,
            }),
            "the two resize families differ ONLY in the distance, so a table that dropped the \
             number would bind eight keys to one action",
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
        // `k` because the default table does not bind it — which this assertion IS: bind it later
        // and this fails here, naming the reason, rather than quietly testing a bound key.
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, now(), "k", Modifiers::default()),
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

    /// **The SHIPPED table repeats, with nothing bound.** Every other repeat test here binds `-r`
    /// itself, so all of them would pass over a default table that carried the flag nowhere — which
    /// is exactly what this one shipped until R297 gave the arrows to it.
    ///
    /// `prefix ArrowLeft ArrowLeft` walks TWO panes for one prefix, and the third press is still
    /// inside the window. That is what a user reaching across a four-pane layout does, and it is
    /// the whole reason tmux puts `-r` on these four and on nothing else this vocabulary has.
    ///
    /// REVERT-PROOF: build the four arrows with `bind` instead of `repeating` and the second press
    /// routes `ToPane` — the arrow reaches the user's shell as an escape sequence.
    #[test]
    fn the_arrow_defaults_repeat_out_of_the_box() {
        let keymap = Keymap::default();
        let base = Instant::now();
        let left = BoundAction::SelectPaneToward { dir: PaneDir::Left };
        let mut mode = PrefixMode::AfterPrefix;
        let mut at = base;
        for press in 1..=3 {
            let routed = keymap.route(mode, at, "ArrowLeft", Modifiers::default());
            assert_eq!(
                routed,
                Routed::Act {
                    action: left.clone(),
                    again: Some(at + DEFAULT_REPEAT_TIME),
                },
                "press {press} of a held prefix-arrow",
            );
            mode = routed.next();
            at += Duration::from_millis(100);
        }
        // ...and the OTHER prefix defaults do not, so the flag is a property of these four rather
        // than of the table. A second `z` after a zoom is a swallowed key, not a second zoom.
        let zoom = keymap.route(PrefixMode::AfterPrefix, base, "z", Modifiers::default());
        assert_eq!(
            zoom,
            Routed::Act {
                action: BoundAction::ZoomPane { on: None },
                again: None,
            },
        );
        assert_eq!(zoom.next(), PrefixMode::ToPane);
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
        // `K` rather than a bound letter, and the second assertion is what keeps that honest — a
        // lone ASCII letter matches case-insensitively on purpose (`same_key`), so `Z` is `z`, which
        // the default table now binds to the zoom.
        assert_eq!(
            keymap.route(armed, base, "K", Modifiers::default()),
            Routed::ToPane,
        );
        assert_eq!(
            keymap.route(PrefixMode::AfterPrefix, base, "K", Modifiers::default()),
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
    ///
    /// ⚠ The second assertion used to read *"every bind is in the PREFIX table"*, which was only an
    /// assertion at all while the root table shipped EMPTY — R314 puts two session chords in it,
    /// and that spelling would have failed for a reason with nothing to do with the claim. Rewritten
    /// around what it always meant: the refused KEY is not in the table. It discriminates the same
    /// way (an accepted bind lands `F5` and the count moves) and no longer rests on a fact this
    /// vocabulary is free to change.
    #[test]
    fn a_root_binding_cannot_repeat() {
        let mut keymap = Keymap::default();
        let refused = keymap
            .bind(KeyTable::Root, "F5", "detach-client", true)
            .expect_err("repeat has no prefix table to hold open");
        assert!(matches!(refused, KeyError::RepeatInRoot(_)));
        assert!(
            !keymap.binds().any(|bind| bind.key().to_string() == "F5"),
            "and the refused binding did not land",
        );
        assert!(
            keymap.binds().any(|bind| bind.table() == KeyTable::Root),
            "the CONTROL: the root table is NOT empty, so the assertion above is about F5 rather \
             than about the table having nothing in it",
        );
    }

    /// The ROOT table holds EXACTLY the two session chords, and every other default is behind the
    /// prefix (R314).
    ///
    /// `the_defaults_are_tmuxs_table` prints keys and actions and cannot show a TABLE, so a chord
    /// that silently moved to the prefix — where `C-S-PageDown` would need `C-b` first and stop
    /// being the gesture `sprag-gui` users already have — would pass it unchanged. This is what
    /// says which table.
    ///
    /// It asserts the SET both ways round: nothing else leaks into root (where it would steal a key
    /// from every program running in a pane, with no prefix to opt out of), and none of the three
    /// leaks out of it.
    ///
    /// REVERT-PROOF: spell any of the three with `bind` instead of `rooted` and the first
    /// assertion drops a row; add a fourth root default and it grows one.
    #[test]
    fn the_root_table_holds_exactly_the_session_chords() {
        let keymap = Keymap::default();
        let in_table = |table| {
            keymap
                .binds()
                .filter(move |bind| bind.table() == table)
                .map(|bind| format!("{} {}", bind.key(), bind.action()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            in_table(KeyTable::Root),
            vec![
                "C-S-PageDown switch-client -n",
                "C-S-PageUp switch-client -p",
            ],
            "the root table holds the three chords sprag-gui used to hold privately, and nothing \
             else: a root binding takes its key from every program in every pane",
        );
        assert!(
            in_table(KeyTable::Prefix)
                .iter()
                .all(|row| !row.starts_with("C-S-")),
            "and none of them is ALSO behind the prefix, which would be two keys for one act",
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
