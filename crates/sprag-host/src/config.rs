//! The USER's own configuration: the commands available in every pane, everywhere.
//!
//! [`crate::project`]'s `.sprag.toml` answers "what does THIS repository want run"; this answers
//! "what do *I* want run, wherever I am" — `lazygit`, `htop`, a personal deploy script. cmux ships
//! the same pair (a per-workspace config beside a user one); this is the second half of it, derived
//! against sprag's seams rather than ported.
//!
//! ## One vocabulary, two sources
//!
//! A declared command is a declared command: this module reuses [`ProjectAction`] and
//! [`crate::project`]'s validation wholesale rather than growing a parallel definition, so `run` is
//! an argv here for exactly the reasons it is one there, every cap is the same cap, and a client
//! that can paint one can paint the other with no new code.
//!
//! What differs is only the two things that genuinely differ:
//!
//! * **WHERE it is found.** Not discovered by walking up from a pane — there is nothing to walk from.
//!   One fixed path ([`config_path`]), so this file governs every pane on the host including the ones
//!   in no project at all, which is precisely the case a project config cannot serve.
//! * **What an error NAMES.** [`ConfigError`] exists so a report says `config.toml`, never
//!   `.sprag.toml`. Both configs can be broken at once and both are reported into the same palette;
//!   a user who cannot tell which file to fix has been told nothing useful.
//!
//! ## Trust
//!
//! This file is the user's OWN, so it is not the untrusted input a repository's is. That changes the
//! posture not at all: a global command is still pasted at a prompt rather than executed, still shown
//! before it runs, and still never run on open. The rules in [`crate::project`] were not concessions
//! to a hostile repository — they are what makes a declared command legible — so they hold here too,
//! and a single treatment means neither surface has to ask where a row came from before acting.

use std::path::{Path, PathBuf};

use crate::project::{ProjectAction, ProjectError, validate_declared};

/// The user's config file name, under [`config_dir`].
///
/// `config.toml` rather than `commands.toml`: this is where sprag's user-level settings belong, and
/// commands are the first of them. The file's TABLES are what say which is which — a later setting
/// arrives as a new one, not as a second file to find.
pub const CONFIG_FILE: &str = "config.toml";

/// sprag's user configuration directory: `$XDG_CONFIG_HOME/sprag`, falling back to `~/.config/sprag`.
///
/// Mirrors [`sprag_state_dir`](crate::durability) exactly one level over — STATE is what sprag writes
/// (the durability snapshot), CONFIG is what the user writes — so the two never land in one directory
/// and a `rm -rf` of either says what it destroys. An `XDG_CONFIG_HOME` that is not absolute is
/// IGNORED rather than honoured, the same rule the state dir applies: the spec requires absolute, and
/// a relative one would resolve against whatever directory the daemon happened to start in.
///
/// Unlike the state dir there is no `/tmp` fallback. A state dir must always resolve, because sprag
/// has something to write; a config dir that cannot be located simply means the user has no config,
/// which [`load`] reports as `None` — the same answer as not having written one.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("sprag"))
}

/// The user config file's full path, or `None` when neither `XDG_CONFIG_HOME` nor `HOME` is set.
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join(CONFIG_FILE))
}

/// The user's declared commands, in file order.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserConfig {
    /// The file these came from — carried for the same reason a [`Project`](crate::Project) carries
    /// its root: a client showing a command should be able to say where it was declared.
    pub path: PathBuf,
    /// The declared commands, in file order.
    pub commands: Vec<ProjectAction>,
}

/// Why the user's config could not be used.
///
/// A distinct type from [`ProjectError`] rather than a reuse, for one reason that is worth the
/// wrapper: the message a user reads has to name the file to fix, and a shared `Display` could only
/// name one of the two. Wrapping keeps the validation vocabulary single while making it impossible to
/// render a global problem as a project one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConfigError(pub ProjectError);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            ProjectError::Unreadable(why) => write!(f, "cannot read {CONFIG_FILE}: {why}"),
            ProjectError::Malformed(why) => write!(f, "{CONFIG_FILE} is not valid TOML: {why}"),
            ProjectError::Invalid(why) => write!(f, "{CONFIG_FILE}: {why}"),
        }
    }
}

/// The user's config, or `None` when there is no [`CONFIG_FILE`] to read.
///
/// `Some(Err(_))` means the file EXISTS and is unusable — the same three-way answer
/// [`project::load`](crate::project::load) gives, and for the same reason: a user who wrote a config
/// with a typo needs to hear about it, whereas never having written one is not a problem to report.
///
/// Read on every call, like a project's: a client asks when it opens a palette, and an edited config
/// should take effect the next time it is asked rather than at the next daemon restart.
#[must_use]
pub fn load() -> Option<Result<UserConfig, ConfigError>> {
    let path = config_path()?;
    if !path.is_file() {
        return None;
    }
    Some(read_config(&path))
}

/// Read + validate the user config at `path`.
fn read_config(path: &Path) -> Result<UserConfig, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| ConfigError(ProjectError::Unreadable(error.to_string())))?;
    let parsed: UserConfigFile = toml::from_str(&text)
        .map_err(|error| ConfigError(ProjectError::Malformed(error.to_string())))?;
    Ok(UserConfig {
        path: path.to_path_buf(),
        commands: validate_declared(parsed.command).map_err(ConfigError)?,
    })
}

/// The file's shape as written by a human — the same `[[command]]` entries a project declares, so a
/// user who has written one config can write the other.
///
/// `deny_unknown_fields` for the reason the project file has it: a typo'd table that silently did
/// nothing would leave the author believing their config was accepted.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UserConfigFile {
    /// `[[command]]` entries; defaulted, so a config that declares none is valid.
    #[serde(default)]
    command: Vec<crate::project::DeclaredAction>,
}

/// Point `XDG_CONFIG_HOME` at a fresh temporary directory holding `text` as the user config
/// (or at an empty one when `text` is `None`), run `body`, then restore the environment.
///
/// Serialised on a mutex because the environment is process-global and these tests mutate it —
/// two running at once would read each other's config. `set_var`/`remove_var` are `unsafe` since
/// Rust 2024 (another thread reading the environment concurrently is UB); the mutex is what makes
/// the call sound here, and no other test in this crate touches `XDG_CONFIG_HOME`.
#[cfg(test)]
pub(crate) fn with_config<T>(text: Option<&str>, body: impl FnOnce() -> T) -> T {
    use std::sync::{Mutex, OnceLock};
    static ENV: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = ENV
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let dir = std::env::temp_dir().join(format!("sprag-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("temp config dir");
    if let Some(text) = text {
        std::fs::write(dir.join("sprag").join(CONFIG_FILE), text).expect("write config");
    }
    let previous = std::env::var_os("XDG_CONFIG_HOME");
    // SAFETY: serialised by the mutex above; no other test in this crate reads or writes
    // XDG_CONFIG_HOME, and `body` runs on this thread.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
    let out = body();
    unsafe {
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_path_sits_under_xdg_config_home() {
        with_config(None, || {
            let path = config_path().expect("XDG_CONFIG_HOME is set");
            assert!(path.ends_with(format!("sprag/{CONFIG_FILE}")), "{path:?}");
        });
    }

    /// A relative `XDG_CONFIG_HOME` is IGNORED (the spec requires absolute), falling back to `HOME`
    /// — never resolved against whatever directory the daemon started in.
    #[test]
    fn a_relative_xdg_config_home_is_ignored() {
        with_config(None, || {
            // SAFETY: inside `with_config`, which holds the environment mutex and restores after.
            unsafe { std::env::set_var("XDG_CONFIG_HOME", "relative/path") };
            let path = config_path().expect("HOME provides the fallback");
            assert!(
                !path.starts_with("relative"),
                "a relative XDG_CONFIG_HOME must not be honoured: {path:?}"
            );
        });
    }

    #[test]
    fn no_config_file_is_not_an_error() {
        // The distinction that matters, exactly as a project's: never having written one is not a
        // problem to report, whereas a broken one is.
        with_config(None, || assert!(load().is_none()));
    }

    #[test]
    fn declared_commands_are_read_in_file_order_with_the_projects_own_rules() {
        with_config(
            Some(
                "[[command]]\nname = \"top\"\nrun = [\"htop\"]\n\
                 [[command]]\nname = \"git\"\ntitle = \"Git UI\"\nrun = [\"lazygit\"]\n",
            ),
            || {
                let config = load().expect("a config exists").expect("it parses");
                let names: Vec<&str> = config.commands.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, vec!["top", "git"], "file order is the offered order");
                assert_eq!(
                    config.commands[0].title, "top",
                    "an omitted title falls back to the name, as in a project"
                );
                assert_eq!(config.commands[1].title, "Git UI");
                assert_eq!(
                    config.commands[1].command_line(),
                    "lazygit",
                    "and it renders through the same quoting SSOT"
                );
            },
        );
    }

    /// A broken user config is REFUSED WHOLE and the report names `config.toml` — not
    /// `.sprag.toml`, which is the point of the wrapper type.
    ///
    /// REVERT-PROOF: render through `ProjectError`'s own `Display` instead and every message names
    /// the wrong file, sending the user to fix a file that is fine.
    #[test]
    fn a_broken_config_is_refused_and_the_report_names_this_file() {
        for (text, expected) in [
            ("[[command]]\nname = \"a\"\nrun = [\n", "not valid TOML"),
            ("[[command]]\nname = \"a\"\nrun = []\n", "empty `run`"),
            (
                "[[command]]\nname = \"a\"\nrun = [\"x\"]\n[[command]]\nname = \"a\"\nrun = [\"y\"]\n",
                "both named",
            ),
            (
                "[[command]]\nname = \"a\"\ntittle = \"A\"\nrun = [\"x\"]\n",
                "not valid TOML",
            ),
        ] {
            with_config(Some(text), || {
                let message = load()
                    .expect("the file exists")
                    .expect_err("and is refused")
                    .to_string();
                assert!(
                    message.starts_with(CONFIG_FILE) || message.contains(CONFIG_FILE),
                    "the report names the file to fix: {message:?}"
                );
                assert!(
                    !message.contains(crate::project::PROJECT_FILE),
                    "and never the OTHER config: {message:?}"
                );
                assert!(
                    message.contains(expected),
                    "...and says what is wrong: {message:?} should mention {expected:?}"
                );
            });
        }
    }

    #[test]
    fn a_config_declaring_no_commands_is_valid() {
        with_config(Some("# nothing yet\n"), || {
            let config = load()
                .expect("the file exists")
                .expect("an empty config is valid");
            assert!(config.commands.is_empty());
        });
    }
}
