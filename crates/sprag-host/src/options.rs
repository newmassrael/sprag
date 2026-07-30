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
//! ## Why every option here is the CLIENT's
//!
//! The daemon has no reason to hold one, for the reason [`crate::config`] gives for a keybinding: a
//! prefix key, a detach policy and a font size are what ONE client does with one keyboard, one
//! attachment and one window, and two clients may legitimately differ. So nothing here crosses the
//! wire, and `sprag show-options` answers on a machine with no session running — the property
//! `list-keys` has, and for the same reason.
//!
//! An option is NOT the place for an operator's control. `SPRAG_RESTORE_HISTORY` bounds what a pane's
//! output writes to disk and `SPRAG_OSC52` decides whether a program may read the user's clipboard;
//! both stay in the environment deliberately, because `config.toml` is the USER's file and a client
//! re-reads it live — an exposure limit that a user can edit, and that takes effect without the
//! daemon restarting, is not an exposure limit.

use std::collections::BTreeMap;

use crate::keymap::KeySpec;

/// The values an option accepts, which is also how one is VALIDATED.
///
/// Two kinds because sprag has two shapes of setting, and each carries its vocabulary with it: a key
/// is whatever [`KeySpec`] parses (so the option space and the keymap can never drift apart), and a
/// choice is a fixed list (so a bad value can be answered with the alternatives rather than with a
/// type name).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionKind {
    /// A keystroke spec — `C-a`, `%`, `F1`. Validated by [`KeySpec::parse`], so this option's
    /// vocabulary IS the keymap's and neither has to be kept in step with the other.
    Key,
    /// One of a fixed set of names, matched case-insensitively and stored lowercase.
    Choice(&'static [&'static str]),
}

impl OptionKind {
    /// `value` canonicalised, or why it cannot be used — the ONE validation, so the file reader, the
    /// CLI and [`Options::set`] cannot disagree about what is acceptable.
    ///
    /// # Errors
    ///
    /// The reason, phrased to be read after `NAME: ` — [`KeySpec`]'s own complaint for a key, and the
    /// list of alternatives for a choice.
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

/// [`DETACH_ON_DESTROY`]'s values, in tmux's documented order.
///
/// The vocabulary lives HERE and the policy lives in the client that acts on it (`sprag-client`
/// parses one of these into its own enum), because a crate holding a display client's behaviour
/// cannot be depended on by this one. A test in that crate holds the two together: every name here
/// must parse to a distinct policy there, or the table offers a value nothing performs.
pub const DETACH_ON_DESTROY_VALUES: &[&str] = &["on", "off", "no-detached", "next", "previous"];

/// Every option sprag has, sorted by name so `show-options` output is stable.
///
/// An option earns its place by having a live CONSUMER — a setting nothing reads is exactly the
/// defect this table exists to remove, one indirection further along. tmux's remaining hundred are
/// not absent because the table cannot hold them; they are absent because sprag has no behaviour for
/// them to govern yet.
pub const OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: DETACH_ON_DESTROY,
        kind: OptionKind::Choice(DETACH_ON_DESTROY_VALUES),
        default: "on",
    },
    OptionSpec {
        name: PREFIX,
        kind: OptionKind::Key,
        default: "C-b",
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
}

impl Default for Options {
    /// Every option at its [`OptionSpec::default`].
    fn default() -> Self {
        Self {
            values: OPTIONS
                .iter()
                .map(|spec| (spec.name, spec.default.to_owned()))
                .collect(),
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
        self.values.insert(setting.spec.name, setting.value);
        Ok(())
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
}
