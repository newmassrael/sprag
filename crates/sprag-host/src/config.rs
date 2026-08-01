//! The USER's own configuration: the commands available in every pane, and the keys a client
//! answers to.
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

//! ## Two audiences, three readers, ONE file shape
//!
//! [`load`] answers the commands question; [`keymap`] and [`options()`](fn@options) answer the
//! client's. The split
//! is by CONSUMER, not by convenience: a declared command is PASTED INTO A PANE, which is a daemon
//! operation, so [`UserConfig`] crosses the wire to the palette — while a keybinding and a client
//! policy are what ONE client does with one keyboard and one attachment, which the daemon has no
//! reason to hold and two clients may legitimately disagree about. Putting either in the wire DTO
//! would send it somewhere it is not wanted.
//!
//! What all three share is ONE private description of the file's shape. That sharing is not an
//! optimisation: it is what keeps `deny_unknown_fields` honest. An `[options]` table the commands
//! reader had never heard of would make the whole file invalid for a user who only wanted to
//! rebind a key.
//!
//! The two CLIENT readers are one act of validation (`build`, private) because the keymap's prefix IS
//! an option: [`crate::options`] holds it, and the keymap is built FROM that table rather than from a
//! second key in the file. One home in the file, one derivation out of it.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value, value};

use crate::keymap::{BoundAction, KeyError, KeySpec, KeyTable, Keymap};
use crate::options::{self, OptionSetting, OptionSpec, Options};
use crate::project::{ProjectAction, ProjectError, validate_declared};
use crate::window::WindowSize;

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
///
/// [`Unwritable`](Self::Unwritable) sits BESIDE that wrapper rather than inside it, because a
/// project file is only ever read: [`ProjectError`] has no shape for a failed write, and borrowing
/// `Unreadable` for one would tell a user whose disk is full to go and check that their file parses.
/// The rule this type exists for is that a report names the right thing — which is the right FILE
/// here and, now that there is a second direction, the right ACTION too.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConfigError {
    /// The file's CONTENT could not be used: unreadable, not TOML, or declaring something invalid.
    Content(ProjectError),
    /// The file could not be WRITTEN — produced only by the editing verbs ([`bind_key`],
    /// [`unbind_key`], [`set_option`], [`unset_option`]).
    Unwritable(String),
}

/// So a caller that reports errors through `Box<dyn Error>` — every binary here — can carry one
/// without restating its message. The `source` is deliberately absent: [`ProjectError`] is the
/// payload, not a cause, and `Display` already says which file and what is wrong with it.
impl std::error::Error for ConfigError {}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content(ProjectError::Unreadable(why)) => {
                write!(f, "cannot read {CONFIG_FILE}: {why}")
            }
            Self::Content(ProjectError::Malformed(why)) => {
                write!(f, "{CONFIG_FILE} is not valid TOML: {why}")
            }
            Self::Content(ProjectError::Invalid(why)) => write!(f, "{CONFIG_FILE}: {why}"),
            Self::Unwritable(why) => write!(f, "cannot write {CONFIG_FILE}: {why}"),
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
    Ok(UserConfig {
        path: path.to_path_buf(),
        commands: validate_declared(read_file(path)?.command).map_err(ConfigError::Content)?,
    })
}

/// The user's KEYMAP: [`Keymap::default`] with whatever [`CONFIG_FILE`] declares layered over it.
///
/// The defaults are the answer when there is no file, when there is no `HOME` to find one under,
/// and when the file declares no keys — all three are "the user has not said otherwise", which is
/// not a condition to report. A file that EXISTS and is broken is reported, exactly as [`load`]
/// reports it, and reported WHOLE: a keymap assembled from the half of a file that parsed would be a
/// table the user never wrote.
///
/// # Errors
///
/// [`ConfigError`] when the file exists and cannot be read, is not valid TOML, or declares a key,
/// an action, or a bind/unbind pair that cannot be used.
pub fn keymap() -> Result<Keymap, ConfigError> {
    Ok(client_config()?.1)
}

/// The user's OPTIONS: [`Options::default`] with whatever [`CONFIG_FILE`] declares layered over it.
///
/// The same three-way silence [`keymap`] treats as "the user has not said otherwise" — no file, no
/// config directory, no `[options]` table — and the same whole-file refusal for one that exists and
/// cannot be used. Read on every call, so `sprag show-options` in a shell describes the file as it is
/// now rather than as it was when something started.
///
/// # Errors
///
/// [`ConfigError`] on exactly [`keymap`]'s conditions — it is one validation over one document.
pub fn options() -> Result<Options, ConfigError> {
    Ok(client_config()?.0)
}

/// What a pane runs when NO command was specified: the user's
/// [`default-command`](crate::options::DEFAULT_COMMAND) if they set one, else `$SHELL` —
/// tmux's `default-command` falling through to its `default-shell`.
///
/// # Why this is one function and not four
///
/// Four places resolve "no command specified": the daemon's `spawn` / `split` / `new_session` /
/// `new_window` actions (through one spec parser), the in-process host's own `new_pane` and
/// `new_window`, the windowed client's boot pane, and `sprag-term`'s standalone boot. A setting
/// honoured by some of them is the asymmetry R237 named — `prefix %` working over a socket and doing
/// nothing in process — so there is one resolver and every birth calls it.
///
/// **The restore path is deliberately NOT one of them.** `exact_or_shell`'s fallback to a shell is not
/// "no command was specified": it is "the recorded command is REFUSED", a security decision about
/// re-running what a pane was doing. Substituting a user's `default-command` there would answer a
/// different question with this one's answer.
///
/// # Why a broken config does not refuse the pane
///
/// The daemon has no screen, and the file's problem is already reported to any client that opens a
/// palette ([`load`]). Refusing to birth a pane over a typo somewhere else in the file would cost the
/// user the one surface that could tell them — so this logs and falls through, the rule
/// [`history_limit`](crate::history_limit) states for a malformed env var.
#[must_use]
pub fn default_pane_command() -> (sprag_terminal::CommandBuilder, String) {
    let command = match options() {
        Ok(options) => options
            .get(options::DEFAULT_COMMAND)
            .unwrap_or_default()
            .to_owned(),
        Err(error) => {
            tracing::warn!(
                target: "sprag_host::config",
                %error,
                "using the default shell for a pane with no command",
            );
            String::new()
        }
    };
    if command.is_empty() {
        return sprag_terminal::default_shell_command();
    }
    sprag_terminal::shell_command_line(&command)
}

/// The policy that decides a session's window size when several clients are attached — the user's
/// [`window-size`](crate::options::WINDOW_SIZE), or [`WindowSize::DEFAULT`] if they have not set one.
///
/// Read from the file on every call, like [`default_pane_command`] and for the same reason: the
/// daemon is a reader of the user's config, not a holder of it, so `set-option window-size` takes
/// effect on the next arbitration with nothing to restart and nothing to invalidate. The cost is one
/// file read per window change, against a reflow of every pane in the session.
///
/// A broken config logs and falls through to the default rather than refusing to answer: a session
/// whose window could not be computed would have no size at all, which is worse than the behaviour
/// the user had before they typed the typo.
#[must_use]
pub fn window_size() -> WindowSize {
    let value = match options() {
        Ok(options) => options
            .get(options::WINDOW_SIZE)
            .unwrap_or_default()
            .to_owned(),
        Err(error) => {
            tracing::warn!(
                target: "sprag_host::config",
                %error,
                "using the default window-size policy",
            );
            String::new()
        }
    };
    // A value the registry validated but this enum does not know would be a table offering a policy
    // nothing performs — the defect `WINDOW_SIZE_VALUES` exists to make unreachable. Falling back is
    // the total answer; the vocabulary test in `options` is what keeps it unreachable.
    WindowSize::parse(&value).unwrap_or(WindowSize::DEFAULT)
}

/// How many logical lines of scrollback a pane BORN NOW should retain — the user's
/// [`history-limit`](crate::options::HISTORY_LIMIT), or the emulator's own default if they have not
/// set one.
///
/// Read from the file on every call, like [`default_pane_command`] and [`window_size`], so a user who
/// raises the setting gets it on their next pane with nothing to restart. One file read per pane
/// BIRTH — a rate bounded by how fast a person can open panes, against a setting that would otherwise
/// need a daemon restart to take effect.
///
/// A broken config logs and falls through to the default rather than refusing the pane, the rule
/// [`default_pane_command`] states at length: the daemon has no screen to report the problem on, and
/// a user who cannot open a pane cannot open the palette that would tell them what is wrong.
///
/// The registry has already refused anything that is not a number, so a value that fails to parse
/// here would mean the table and this reader disagree — falling back is the total answer, and
/// `every_option_default_is_a_value_that_option_accepts` is what keeps it unreachable.
#[must_use]
pub fn history_limit_lines() -> usize {
    let configured = match options() {
        Ok(options) => options.number(options::HISTORY_LIMIT),
        Err(error) => {
            tracing::warn!(
                target: "sprag_host::config",
                %error,
                "using the default history-limit for a pane",
            );
            None
        }
    };
    configured.map_or(sprag_vt::DEFAULT_SCROLLBACK_LINES, |lines| lines as usize)
}

/// The agent-state settle window in force — the user's
/// [`agent-settle-time`](crate::options::AGENT_SETTLE_TIME), or the detector's own default if they
/// have not set one.
///
/// Read from the file on every call, like every other option reader here, so `set-option` takes effect
/// with nothing to restart. **But WHERE it is called is the cost decision, and it is not this
/// function's to make.** [`window_size`] prices its own file read as one per WINDOW CHANGE and
/// [`history_limit_lines`] as one per pane BIRTH; both are rare events, and both say so. The settle
/// window's reader is the pane list, which is served on every client wake — so
/// [`AgentRegistry::observe`](crate::AgentRegistry::observe) calls this only for a pane that will
/// actually consult the window (one being built, or one with a candidate waiting), and a workspace of
/// settled panes reads the file zero times. Calling it unconditionally from that path would be a file
/// read per output batch per session, which is the one shape this option must not take.
///
/// A broken config logs and falls through to the default, the rule [`default_pane_command`] states at
/// length. Refusing to answer would leave a pane with no agent state at all, which is worse for the
/// user than the window they had before they typed the typo.
#[must_use]
pub fn agent_settle() -> sprag_detect::Hysteresis {
    let configured = match options() {
        Ok(options) => options.number(options::AGENT_SETTLE_TIME),
        Err(error) => {
            tracing::warn!(
                target: "sprag_host::config",
                %error,
                "using the default agent-settle-time",
            );
            None
        }
    };
    sprag_detect::Hysteresis {
        settle: configured.map_or(sprag_detect::DEFAULT_SETTLE, |millis| {
            std::time::Duration::from_millis(u64::from(millis))
        }),
    }
}

/// The client-side halves of the user's config, or the defaults when there is nothing to read.
fn client_config() -> Result<(Options, Keymap), ConfigError> {
    let Some(path) = config_path() else {
        return Ok((Options::default(), Keymap::default()));
    };
    if !path.is_file() {
        return Ok((Options::default(), Keymap::default()));
    }
    build(&read_file(&path)?)
}

/// The client-side halves of the user's config — the [`Keymap`] and the [`Options`] — holding on to
/// [`CONFIG_FILE`]'s text so it can notice the file CHANGED.
///
/// # Why ONE holder for two tables
///
/// They come out of one file. A second holder would mean a second read of the same bytes, a second
/// staleness verdict, and two answers to "what does the file say right now" that can differ for as
/// long as one of them has not looked — for a file whose whole point is that an edit takes effect
/// immediately. One holder makes that class of disagreement unrepresentable.
///
/// # Why a client holds this rather than a bare `Keymap`
///
/// The file IS the live table. `sprag bind-key` ([`bind_key`]) edits it and a running client must
/// act on that without being restarted — and the same mechanism gives the user who edits their
/// config in an EDITOR the reload tmux spells `source-file`, with nothing to invoke.
///
/// The alternative was a runtime table living somewhere else — in the daemon, or in a message sent
/// to a client — and it is the one thing that cannot work here: `sprag list-keys` reads this file
/// with NO DAEMON, so a binding the file did not know about would make that verb print a table
/// nobody is using, with no way for a user to see the difference.
///
/// # Why the check is a read and not a watch — and not a timestamp either
///
/// [`refresh`](Self::refresh) is called by a client from a wake it ALREADY has — the keystroke
/// whose meaning the table decides — so it adds no thread, no timer, and no wake. That matters: the
/// terminal client's loop is a pure `select` with no tick, and a keymap watcher would be the first
/// thing to put a heartbeat into it.
///
/// What it compares is the file's TEXT, not its `(mtime, len)`. A stamp is the cheaper check and it
/// has a hole: two writes inside one timestamp tick that leave the length alone are one keystroke
/// apart in a real edit (`split-window -h` and `split-window -v` differ in neither). The failure
/// that hole produces is a rebind that sometimes does not take — nondeterministic, and blamed on
/// everything except the thing that caused it. The exact check costs one read of a file measured in
/// hundreds of bytes, against a routing decision and a repaint that both follow it.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Where the file would be, or `None` when there is no config directory to hold one — in which
    /// case there is nothing to re-read and the defaults are final.
    path: Option<PathBuf>,
    /// The exact text the tables below were built from; `None` when there was no file at all.
    text: Option<String>,
    /// The last keymap read SUCCESSFULLY. Retained across a failed re-read (see
    /// [`refresh`](Self::refresh)).
    keymap: Keymap,
    /// The last options read successfully — retained across a failed re-read with the keymap, since
    /// they are one document and a client that kept half of a file would be honouring a config
    /// nobody wrote.
    options: Options,
    /// Why the file's own declarations are NOT the table in force, if they are not — the error from
    /// the last read that actually looked at the content.
    ///
    /// Here rather than in the caller, and that is the whole point: a client that kept its own copy
    /// would have to decide what an unchanged file means, and the honest answer is "no news", which is
    /// indistinguishable from "nothing is wrong" once the fact lives somewhere else. Cleared by a read
    /// that succeeds, set by one that fails, and left ALONE by a read that found the file unmoved.
    unusable: Option<ConfigError>,
}

impl ClientConfig {
    /// Read the user's config now, remembering the file it came from.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] on exactly the conditions [`keymap`] reports: the file exists and cannot be
    /// read, is not valid TOML, or declares something unusable. A client fails to START on those,
    /// because the one screen able to show the message is the one it has not yet replaced.
    pub fn load() -> Result<Self, ConfigError> {
        match Self::read(config_path()) {
            (_, Some(error)) => Err(error),
            (file, None) => Ok(file),
        }
    }

    /// The user's tables, or the DEFAULTS and the reason theirs could not be used.
    ///
    /// **For a client with no screen to fail on.** [`load`](Self::load)'s answer to a broken file is
    /// to refuse to start, which is right for a terminal client — the screen able to show the message
    /// is the one it has not yet replaced. A windowed client has no such screen, and refusing to open
    /// a window over one bad line would take a whole session view away with the reason going to a log
    /// the user will not read. It reports through a surface of its own instead, and needs a usable
    /// table to report FROM.
    ///
    /// The broken text is REMEMBERED, so [`refresh`](Self::refresh) notices the fix and does not
    /// re-report the same file on every keystroke until then — the same once-only rule a failed
    /// re-read follows.
    pub fn load_usable() -> (Self, Option<ConfigError>) {
        Self::read(config_path())
    }

    /// The config declared by `path`, watched at `path` — [`load_usable`](Self::load_usable) aimed at
    /// a file the caller names instead of at the user's own.
    ///
    /// The holder stopped hard-coding which file it watches when this arrived, and the honest reason
    /// is that a client's USE of it could not otherwise be exercised from outside this crate: the
    /// only other way in is `$XDG_CONFIG_HOME`, which is process-global, so a frontend's test would
    /// have to mutate the environment its siblings are reading. Everything else is identical —
    /// the same reader, the same fall back to the defaults, the same remembered text.
    pub fn at(path: &std::path::Path) -> (Self, Option<ConfigError>) {
        Self::read(Some(path.to_owned()))
    }

    /// Read `path` and build what it declares, keeping the pieces either caller needs: the holder
    /// (always usable — the defaults when the file could not be used) and the reason, if any.
    fn read(path: Option<PathBuf>) -> (Self, Option<ConfigError>) {
        let (text, unreadable) = read_text(path.as_deref());
        let built = match &text {
            Some(text) => parse_file(text).and_then(|file| build(&file)),
            None => Ok((Options::default(), Keymap::default())),
        };
        let ((options, keymap), error) = match built {
            Ok(tables) => (tables, unreadable),
            Err(error) => ((Options::default(), Keymap::default()), Some(error)),
        };
        (
            Self {
                keymap,
                options,
                path,
                text,
                unusable: error.clone(),
            },
            error,
        )
    }

    /// The keymap as it was last read.
    #[must_use]
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// The options as they were last read.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Why the table in force is NOT the one the file declares, if it is not — [`None`] when the file
    /// (or its absence) is being honoured.
    ///
    /// A client with a surface to report on asks this rather than remembering what a read once told it,
    /// and the difference is not cosmetic: a re-read of an UNCHANGED broken file has nothing to say, so
    /// a caller keeping its own copy would either clear a report that still holds or re-announce one
    /// the user has already seen. Both were written before this existed.
    #[must_use]
    pub fn unusable(&self) -> Option<&ConfigError> {
        self.unusable.as_ref()
    }

    /// Re-read the file if its content has changed, and say whether EITHER table moved.
    ///
    /// `Ok(false)` is the steady state. `Ok(true)` means the file changed AND the new tables differ
    /// — a file rewritten without any change to what it MEANS answers `false`, so a caller acting
    /// on this is never acting on an edit that was not one.
    ///
    /// A file that has become UNREADABLE (a mode changed underneath) answers `Ok(false)` and
    /// changes nothing: this cannot tell whether the content moved, so the honest report is that it
    /// did not look, and the client keeps a table that works. The next attach reports it properly,
    /// because [`load`](Self::load) has a screen to say it on.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when the file changed and the new content cannot be used. **The previous
    /// table is KEPT** — a client owns the screen and has nowhere to print, and swapping in the
    /// defaults would silently take a user's own bindings away because they typo'd a line in an
    /// editor. The remembered text advances anyway, so one broken save is reported ONCE rather than
    /// on every keystroke until it is fixed.
    pub fn refresh(&mut self) -> Result<bool, ConfigError> {
        let Some(path) = self.path.as_deref() else {
            return Ok(false);
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            // DELETED is a user saying "I have no config", which is the state `load` answers with
            // the defaults — so it means the same thing here.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Ok(false),
        };
        if text == self.text {
            return Ok(false);
        }
        self.text = text;
        // The verdict is recorded either way, because this is the read that LOOKED: a success means
        // the file is being honoured now, a failure means it is not, and the early return above means
        // an unchanged file never overwrites either.
        let next = match &self.text {
            Some(text) => parse_file(text).and_then(|file| build(&file)),
            None => Ok((Options::default(), Keymap::default())),
        };
        match next {
            Ok((options, keymap)) => {
                self.unusable = None;
                // Both swaps run before either verdict is combined. `a() || b()` would short-circuit
                // and leave the OPTIONS unswapped on any keystroke that moved the keymap — a stale
                // table that only appears when the other one changed.
                let keymap_moved = std::mem::replace(&mut self.keymap, keymap) != self.keymap;
                let options_moved = std::mem::replace(&mut self.options, options) != self.options;
                Ok(keymap_moved || options_moved)
            }
            Err(error) => {
                self.unusable = Some(error.clone());
                Err(error)
            }
        }
    }
}

/// [`CONFIG_FILE`]'s text, or why it could not be had.
///
/// A path that is not a file is NOT an error: it is a user who has written no config, which every
/// reader here answers with the defaults. Shared by the two holders so that answer is one decision
/// rather than two that can drift — a daemon deciding a missing file is a problem while a client
/// decides it is not would be a disagreement about the same absent file.
fn read_text(path: Option<&Path>) -> (Option<String>, Option<ConfigError>) {
    match path {
        Some(path) if path.is_file() => match std::fs::read_to_string(path) {
            Ok(text) => (Some(text), None),
            Err(error) => (
                None,
                Some(ConfigError::Content(ProjectError::Unreadable(
                    error.to_string(),
                ))),
            ),
        },
        _ => (None, None),
    }
}

/// The DAEMON's half of [`CONFIG_FILE`] — the agent manifests, held so that an edit can be noticed.
///
/// # Why this is HELD, when every other reader in this module reads per call
///
/// [`options`](fn@options) and [`window_size`] read the file on every call, deliberately: the daemon
/// is a reader of the user's config rather than a holder of it, so `set-option` takes effect with
/// nothing to restart and nothing to invalidate. [`window_size`] prices that honestly as one file
/// read per WINDOW CHANGE — a rare-event justification — and [`agent_settle`] narrows the same read
/// to the panes actually waiting on a window.
///
/// A manifest list cannot be bought on those terms, and [`sprag_detect::built_ins`] says why in its
/// own docs: a manifest owns compiled [`Regex`](regex::Regex)es, so a reader that rebuilt the list
/// per evaluation would recompile every pattern of every agent on a path served once per client
/// wake. Every other setting in this file costs a READ; this one costs a parse and a compile.
///
/// # So when is it re-read
///
/// From a wake that already exists, which is [`ClientConfig`]'s answer transposed one process over.
/// A client re-reads on the keystroke whose meaning the table decides. The daemon re-reads on the
/// agent waker's sweep — a loop that already runs, already walks every session, and already takes
/// the locks — so this adds no thread, no timer and no wake, which is the property that argument
/// exists to protect.
///
/// The consequence is a LATENCY and is stated as one rather than left to be discovered: an edit
/// takes effect within one sweep. That is the shape slice 3's discovery contract already has.
///
/// # What "changed" means
///
/// The file's TEXT, exactly as [`ClientConfig`] compares it and for the reason recorded there — a
/// `(mtime, len)` stamp misses two writes inside one timestamp tick that leave the length alone.
///
/// Text is coarser than MEANING, and here that is not a choice: a [`Manifest`](sprag_detect::Manifest)
/// holds compiled patterns and cannot be compared for equality at all, so a file rewritten without
/// changing what it says still replaces the ruleset. What that costs is one re-evaluation per pane —
/// the cost of a single client poll — against a rewrite that happens when a person saves a file.
#[derive(Debug)]
pub struct AgentManifests {
    /// Where the file would be, or `None` when there is no config directory to hold one — in which
    /// case there is nothing to re-read and the built-ins are final.
    path: Option<PathBuf>,
    /// The exact text [`rules`](Self::rules) was built from; `None` when there was no file at all.
    text: Option<String>,
    /// The last list read successfully, KEPT across a failed re-read.
    rules: sprag_detect::Ruleset,
    /// Why the list in force is not the one the file declares, if it is not.
    unusable: Option<ConfigError>,
}

impl AgentManifests {
    /// The user's manifests now, remembering the file they came from.
    ///
    /// Never fails. A daemon has no screen to report on and detection is not what a session is FOR,
    /// so a typo in an `[[agent]]` entry must not take a user's terminal away — the rule
    /// [`default_pane_command`] and [`window_size`] already follow one table over. The reason is kept
    /// on [`unusable`](Self::unusable) for a surface that can show it, and logged once here so a user
    /// whose manifests are silently the built-ins has somewhere to find out why.
    #[must_use]
    pub fn load() -> Self {
        Self::at(config_path().as_deref())
    }

    /// The manifests declared by `path`, watched at `path`.
    ///
    /// Aimed at a caller's own file rather than at the user's, for [`ClientConfig::at`]'s reason: the
    /// only other way in is `$XDG_CONFIG_HOME`, which is process-global, so a test would otherwise
    /// have to mutate the environment its siblings are reading.
    #[must_use]
    pub fn at(path: Option<&Path>) -> Self {
        let (text, unreadable) = read_text(path);
        let (rules, unusable) = match declared_in(text.as_deref()) {
            Ok(manifests) => (sprag_detect::Ruleset::new(manifests), unreadable),
            Err(error) => (sprag_detect::Ruleset::default(), Some(error)),
        };
        if let Some(error) = &unusable {
            report_manifests(error);
        }
        Self {
            path: path.map(Path::to_path_buf),
            text,
            rules,
            unusable,
        }
    }

    /// The manifests in force.
    #[must_use]
    pub fn rules(&self) -> &sprag_detect::Ruleset {
        &self.rules
    }

    /// Why the list in force is NOT the one the file declares, if it is not — `None` when the file
    /// (or its absence) is being honoured.
    #[must_use]
    pub fn unusable(&self) -> Option<&ConfigError> {
        self.unusable.as_ref()
    }

    /// Re-read the file if its content has changed, and say whether the ruleset was REPLACED.
    ///
    /// `false` is the steady state, and it is what the caller acts on: a replaced ruleset carries a
    /// new [`Ruleset::revision`](sprag_detect::Ruleset::revision), which is a quiescence-key input,
    /// so every remembered pane owes an evaluation. Answering `true` when nothing moved would cost
    /// the workspace an evaluation per pane for no reason; answering `false` when it did would leave
    /// every quiet pane holding a verdict the user has just edited away.
    ///
    /// A file that has become UNREADABLE answers `false` and changes nothing: this cannot tell
    /// whether the content moved, so the honest report is that it did not look. A file that changed
    /// and cannot be USED keeps the previous list and records the reason — the rule
    /// [`ClientConfig::refresh`] states, and the same once-only reporting, because the remembered
    /// text advances either way.
    pub fn refresh(&mut self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            // DELETED is a user saying "I have no manifests of my own", which is the state the
            // built-ins answer.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return false,
        };
        if text == self.text {
            return false;
        }
        self.text = text;
        match declared_in(self.text.as_deref()) {
            Ok(manifests) => {
                self.unusable = None;
                self.rules = sprag_detect::Ruleset::new(manifests);
                true
            }
            Err(error) => {
                report_manifests(&error);
                self.unusable = Some(error);
                false
            }
        }
    }
}

/// The manifests `text` declares, layered over the built-ins — or the built-ins alone when there is
/// no file.
///
/// One reader for the first read and for every re-read, so a daemon that has been running cannot
/// come to a different conclusion about a file than one that just started.
fn declared_in(text: Option<&str>) -> Result<Vec<sprag_detect::Manifest>, ConfigError> {
    match text {
        Some(text) => parse_file(text).and_then(|file| {
            declared_manifests(&file.agent)
                .map_err(|why| ConfigError::Content(ProjectError::Invalid(why)))
        }),
        None => Ok(sprag_detect::built_ins()),
    }
}

/// Say why the manifests in force are not the user's.
///
/// Once per edit that broke them, which falls out of the caller rather than being counted: the
/// remembered text advances on a failed re-read, so an unchanged broken file never reaches here
/// twice.
fn report_manifests(error: &ConfigError) {
    tracing::warn!(
        target: "sprag_host::config",
        %error,
        "using the built-in agent manifests",
    );
}

/// One declared option value as the text [`OptionKind::canonicalise`](crate::options::OptionKind)
/// reads, or why this is not a value at all.
///
/// A string and an integer are the two spellings a person uses (`prefix = "C-a"`, `gui-font = 20`),
/// and both reach the same validation — so the option's KIND decides what is acceptable, not the TOML
/// type the user happened to write. Anything else (a float, a bool, an array, a table) is refused
/// while NAMING the option, because serde's own "invalid type" message would say which type it wanted
/// without saying which setting it was reading.
fn declared_value(name: &str, value: &toml::Value) -> Result<String, String> {
    match value {
        toml::Value::String(text) => Ok(text.clone()),
        toml::Value::Integer(number) => Ok(number.to_string()),
        other => Err(format!(
            "{name}: a {} is not a value an option takes (write a string or a number)",
            other.type_str()
        )),
    }
}

/// Everything a CLIENT reads out of the file: the options in force, and the keymap they help produce.
///
/// ONE function because it is ONE act of validation. A caller able to get a keymap out of a file
/// whose `[options]` are broken would be acting on half a document the user never wrote — the rule
/// [`keymap`] states for a half-parsed keymap, one table over.
///
/// The order is load-bearing: the options are built FIRST because the prefix comes out of them, so
/// the file has exactly one place that says what the prefix is and the keymap is downstream of it.
fn build(file: &UserConfigFile) -> Result<(Options, Keymap), ConfigError> {
    let invalid = |why: String| ConfigError::Content(ProjectError::Invalid(why));
    let mut options = Options::default();
    for (name, value) in &file.options {
        let value = declared_value(name, value).map_err(invalid)?;
        options
            .set(name, &value)
            .map_err(|error| invalid(error.to_string()))?;
    }
    let mut keymap = Keymap::default();
    // Unconditional, and there is nothing left to branch on: `Options` answers for every registered
    // option (its default when the file is silent), and `set_prefix` returns early when the value is
    // unchanged. The "did the user name a prefix" question this used to ask now has no reader.
    if let Some(prefix) = options.get(options::PREFIX) {
        keymap
            .set_prefix(prefix)
            .map_err(|error: KeyError| invalid(error.to_string()))?;
    }
    // The second option the keymap is built FROM, on the same terms as the prefix: one place in the
    // file says how long a repeat lasts, and the table is downstream of it.
    if let Some(millis) = options.number(options::REPEAT_TIME) {
        keymap.set_repeat_time(u64::from(millis));
    }
    for bind in &file.bind {
        let table = bind.table().map_err(|error| invalid(error.to_string()))?;
        keymap
            .bind(table, &bind.key, &bind.action, bind.repeat)
            .map_err(|error| invalid(error.to_string()))?;
    }
    for unbind in &file.unbind {
        // Refused rather than resolved by precedence. Applying binds before unbinds is one
        // defensible order and applying them in file order is another, so a file that says both
        // about one key has not said what it wants — and a user who has to remember which array
        // wins has been given a puzzle instead of a keymap.
        //
        // PER TABLE, since slice 4: `%` in the root table and `%` in the prefix table are different
        // keys that happen to share a spelling, so binding one and unbinding the other says two
        // things about two keys rather than two things about one.
        let table = unbind.table().map_err(|error| invalid(error.to_string()))?;
        let key = KeySpec::parse(&unbind.key).map_err(|error| invalid(error.to_string()))?;
        let contradicted = file.bind.iter().any(|bind| {
            bind.table().is_ok_and(|bound| bound == table)
                && KeySpec::parse(&bind.key).is_ok_and(|bound| bound == key)
        });
        if contradicted {
            return Err(invalid(
                KeyError::BoundAndUnbound(key.to_string()).to_string(),
            ));
        }
        keymap
            .unbind(table, &unbind.key)
            .map_err(|error| invalid(error.to_string()))?;
    }
    Ok((options, keymap))
}

/// The `[[bind]]` array's name in the file, and the field names of one entry.
///
/// Spelled here as well as on [`DeclaredBind`] because a writer cannot ask a `serde` derive what it
/// called a field. Nothing HOLDS the two together — except that [`edit_config`] reads its own output
/// back through the reader before writing it, so a drift makes the very first edit fail with
/// `deny_unknown_fields` rather than silently producing a file nothing honours.
const BIND_ARRAY: &str = "bind";
/// The `[[unbind]]` array's name — see [`BIND_ARRAY`].
const UNBIND_ARRAY: &str = "unbind";
/// The `key` field of a `[[bind]]` / `[[unbind]]` entry — see [`BIND_ARRAY`].
const KEY_FIELD: &str = "key";
/// The `action` field of a `[[bind]]` entry — see [`BIND_ARRAY`].
const ACTION_FIELD: &str = "action";
/// The `table` field of a `[[bind]]` / `[[unbind]]` entry — see [`BIND_ARRAY`].
const TABLE_FIELD: &str = "table";
/// The `repeat` field of a `[[bind]]` entry — see [`BIND_ARRAY`].
const REPEAT_FIELD: &str = "repeat";

/// Bind `key` to `action` in the user's [`CONFIG_FILE`] — tmux's `bind-key`. Returns the file it
/// wrote.
///
/// # Why this WRITES, when tmux's `bind-key` does not
///
/// tmux's runtime bind mutates the server's table and leaves `~/.tmux.conf` untouched, so a binding
/// a user liked is one they then have to remember to write down. That is not a preference tmux
/// expressed — its config is an imperative SCRIPT, and a fact cannot be written back into a script.
/// sprag's is declarative TOML, which is a structure an edit can land in.
///
/// It also has to be this way here: `sprag list-keys` reads this file with NO DAEMON, so a binding
/// that lived anywhere else would make that verb print a table nobody is using.
///
/// The edit is comment-preserving (the file is hand-maintained), lands atomically (a config nobody
/// has a backup of must not be truncated by a crash), and is REFUSED both when the file already
/// cannot be read and when the result would not read back — a writer has no business rewriting a
/// config it does not understand, and none at all producing one nothing can use.
///
/// # Why this takes a PARSED key and action
///
/// Every error this function can report is then about the FILE, which is what makes the report
/// trustworthy: a mistyped key is the CALLER's argument, and rendering it through [`ConfigError`]
/// would prefix it with `config.toml` and send a user to fix a file that is fine. That is the same
/// misdirection this error type was invented to prevent, one level in.
///
/// # Errors
///
/// [`ConfigError::Content`] when the file already cannot be read or used;
/// [`ConfigError::Unwritable`] when it cannot be replaced.
pub fn bind_key(
    table: KeyTable,
    key: &KeySpec,
    action: BoundAction,
    repeat: bool,
) -> Result<PathBuf, ConfigError> {
    edit_config(move |doc| {
        // The contradiction slice 1 REFUSES (`BoundAndUnbound`) is what an unbind left in place
        // would make: this key is being given a meaning, so a declaration that it has none is not
        // a second opinion to keep, it is the same statement retracted. Scoped to the same TABLE,
        // because that is the scope the refusal itself now has.
        remove_named(doc, UNBIND_ARRAY, table, key)?;
        let entries = tables_mut(doc, BIND_ARRAY)?;
        // Bound to a `let` rather than used as the `if let` scrutinee: the iterator is a boxed
        // trait object, so as a temporary it would live — with its immutable borrow — past the
        // point the `else` arm needs the array mutably.
        let existing = entries.iter().position(|entry| names(entry, table, key));
        if let Some(index) = existing {
            // Retargeted IN PLACE rather than removed and appended, the same rule
            // [`Keymap::bind`] follows: a rebound key keeps the position the user gave it, in
            // their file as well as in `list-keys`.
            if let Some(entry) = entries.get_mut(index) {
                entry[ACTION_FIELD] = value(action.to_string());
                set_or_clear(entry, REPEAT_FIELD, repeat_field(repeat));
            }
        } else {
            let mut entry = Table::new();
            entry[KEY_FIELD] = value(key.to_string());
            entry[ACTION_FIELD] = value(action.to_string());
            set_or_clear(&mut entry, TABLE_FIELD, table_field(table));
            set_or_clear(&mut entry, REPEAT_FIELD, repeat_field(repeat));
            entries.push(entry);
        }
        Ok(())
    })
}

/// Make `key` mean nothing in the user's [`CONFIG_FILE`] — tmux's `unbind-key`. Returns the file it
/// wrote.
///
/// Two file edits, because the keymap is the defaults LAYERED with the file: the user's own
/// `[[bind]]` for this key is removed, and an `[[unbind]]` is added **only if the DEFAULT keymap
/// binds it**. Without that condition every unbind would leave a line suppressing nothing, in a
/// file whose whole point is that a human reads it.
///
/// IDEMPOTENT, like [`Keymap::unbind`]: unbinding a key that already means nothing rewrites the
/// file to the same content.
///
/// # Errors
///
/// As [`bind_key`], and it takes a parsed key for the same reason.
pub fn unbind_key(table: KeyTable, key: &KeySpec) -> Result<PathBuf, ConfigError> {
    edit_config(move |doc| {
        remove_named(doc, BIND_ARRAY, table, key)?;
        // A key the defaults never bound now means nothing already: removing the user's own
        // binding was the whole edit, and an `[[unbind]]` would be a line about a key no table
        // mentions. Asked of the same TABLE — sprag's defaults are all in the prefix table, so an
        // unbind in the root table never needs a suppressing line.
        if Keymap::default()
            .action(table, key.name(), key.mods())
            .is_none()
        {
            return Ok(());
        }
        let entries = tables_mut(doc, UNBIND_ARRAY)?;
        if entries.iter().all(|entry| !names(entry, table, key)) {
            let mut entry = Table::new();
            entry[KEY_FIELD] = value(key.to_string());
            set_or_clear(&mut entry, TABLE_FIELD, table_field(table));
            entries.push(entry);
        }
        Ok(())
    })
}

/// The `[options]` table's name in the file — see [`BIND_ARRAY`] for why a writer spells it out.
const OPTIONS_TABLE: &str = "options";

/// Set an option in the user's [`CONFIG_FILE`] — tmux's `set-option`. Returns the file it wrote.
///
/// Like [`bind_key`] this EDITS the user's file rather than a runtime table, for the reason slice 2
/// established: the file IS the live table, so `show-options` and an attached client cannot give
/// different answers. And like it, this needs no daemon — every option here is a client's.
///
/// It takes an [`OptionSetting`] rather than a name and a value, so every error it can report is
/// about the FILE. A mistyped option name or value is the caller's, refused where the caller can be
/// told so.
///
/// # Errors
///
/// [`ConfigError::Content`] when the file already cannot be read or used;
/// [`ConfigError::Unwritable`] when it cannot be replaced.
pub fn set_option(setting: &OptionSetting) -> Result<PathBuf, ConfigError> {
    edit_config(|doc| {
        let table = table_mut(doc, OPTIONS_TABLE)?;
        // A number is written UNQUOTED, because that is how a person writes one in a file they
        // maintain by hand — and because the reader accepts both, a writer that quoted it would make
        // every edited file differ from every hand-written one for no reason a user could see.
        table[setting.spec().name] = match setting.as_number() {
            Some(number) => value(i64::from(number)),
            None => value(setting.value()),
        };
        Ok(())
    })
}

/// Remove an option from the user's [`CONFIG_FILE`], so its default is in force again — tmux's
/// `set-option -u`. Returns the file it wrote.
///
/// IDEMPOTENT, like [`unbind_key`]: unsetting an option the file never mentioned rewrites it to the
/// same content. An `[options]` table left EMPTY by the last unset is kept rather than removed —
/// the same rule that refuses to rewrite an inline array, one table over: an edit that deletes a
/// header the user wrote by hand has reformatted a file it was not asked about.
///
/// # Errors
///
/// As [`set_option`].
pub fn unset_option(spec: &'static OptionSpec) -> Result<PathBuf, ConfigError> {
    edit_config(|doc| {
        // Checked first so that unsetting an option in a file with no `[options]` cannot bring the
        // table into being — [`remove_named`]'s rule.
        if doc.get(OPTIONS_TABLE).is_none() {
            return Ok(());
        }
        table_mut(doc, OPTIONS_TABLE)?.remove(spec.name);
        Ok(())
    })
}

/// The document's `[name]` table, created empty if the file has none.
///
/// An inline `name = {…}` is REFUSED rather than replaced, for [`tables_mut`]'s reason: it reads back
/// identically, so the file is not broken, and rewriting it would rearrange a file the user wrote by
/// hand.
fn table_mut<'a>(
    doc: &'a mut DocumentMut,
    name: &'static str,
) -> Result<&'a mut Table, ConfigError> {
    doc.entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            ConfigError::Unwritable(format!(
                "its `{name}` is written as an inline table; an edit only writes the [{name}] \
                 form, so change it by hand first"
            ))
        })
}

/// Whether a `[[bind]]` / `[[unbind]]` entry names `key`.
///
/// Compared PARSED rather than as text: `C-o` and `^o` are one keystroke, so an edit that matched
/// only the spelling it was handed would leave behind an entry the READER treats as the same key —
/// which for a bind is a stale action and for an unbind is the contradiction slice 1 refuses.
fn names(entry: &Table, table: KeyTable, key: &KeySpec) -> bool {
    // The TABLE is half the identity of a binding, not a property of one: `%` in the root table and
    // `%` in the prefix table are two keys. An editor that matched on the key alone would rewrite
    // the wrong entry — silently, and only for users who had bound both.
    let same_table = declared_table(entry.get(TABLE_FIELD).and_then(Item::as_str))
        .is_ok_and(|declared| declared == table);
    same_table
        && entry
            .get(KEY_FIELD)
            .and_then(Item::as_str)
            .and_then(|spec| KeySpec::parse(spec).ok())
            .is_some_and(|spec| spec == *key)
}

/// Write `table` / `repeat` onto a `[[bind]]` entry, or REMOVE the field when it is the default.
///
/// Removing rather than writing `table = "prefix"` matters on a REBIND: a key that repeated and no
/// longer does would otherwise keep a `repeat = true` line that nothing honours, which is a file
/// saying one thing and a keymap doing another. It also keeps a hand-maintained file free of lines
/// stating the default.
fn set_or_clear(entry: &mut Table, field: &str, declare: Option<Value>) {
    match declare {
        Some(declared) => entry[field] = value(declared),
        None => {
            entry.remove(field);
        }
    }
}

/// The `table = …` an entry needs, or [`None`] when it is the default and the field should go.
fn table_field(table: KeyTable) -> Option<Value> {
    (table != KeyTable::Prefix).then(|| Value::from(table.as_str()))
}

/// The `repeat = …` an entry needs, or [`None`] when it does not repeat.
fn repeat_field(repeat: bool) -> Option<Value> {
    repeat.then(|| Value::from(true))
}

/// The document's `[[name]]` array of tables, created empty if the file has none.
///
/// An inline `name = [{…}]` is REFUSED rather than replaced. It reads back identically, so the file
/// is not broken — but rewriting it as `[[name]]` would rearrange a file the user wrote by hand,
/// and a config edit that reformats what it did not ask about is one nobody can trust twice.
fn tables_mut<'a>(
    doc: &'a mut DocumentMut,
    name: &'static str,
) -> Result<&'a mut ArrayOfTables, ConfigError> {
    doc.entry(name)
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| {
            ConfigError::Unwritable(format!(
                "its `{name}` is written as an inline array; an edit only writes the [[{name}]] \
                 form, so change it by hand first"
            ))
        })
}

/// Remove every `[[name]]` entry naming `key`, and say whether any went.
///
/// Absent means nothing to remove — checked first so that asking about an array the file does not
/// have cannot bring one into being.
fn remove_named(
    doc: &mut DocumentMut,
    name: &'static str,
    table: KeyTable,
    key: &KeySpec,
) -> Result<bool, ConfigError> {
    if doc.get(name).is_none() {
        return Ok(false);
    }
    let entries = tables_mut(doc, name)?;
    let before = entries.len();
    entries.retain(|entry| !names(entry, table, key));
    Ok(entries.len() != before)
}

/// Apply `edit` to the user's [`CONFIG_FILE`] and write it back, or change nothing at all.
///
/// Shared by every writer here — a binding and an option are two edits to one document, and a second
/// copy of this would be a second answer to what a valid config is.
///
/// # The two validations, and why both are needed
///
/// The file is read through the ORDINARY reader BEFORE the edit: a config this reader cannot make
/// sense of is one a writer has no business rewriting, and refusing tells the user what is wrong
/// with it instead of quietly reshaping it.
///
/// The result is read back through the same reader AFTER: an edit whose output the reader would
/// refuse must never reach the disk, because the file it would leave behind breaks every client
/// AND the `list-keys` that would have explained why.
///
/// A missing file is not an error — it is a user who has no config yet, and the edit creates one.
fn edit_config(
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), ConfigError>,
) -> Result<PathBuf, ConfigError> {
    let path = config_path().ok_or_else(|| {
        ConfigError::Unwritable(
            "neither XDG_CONFIG_HOME nor HOME names a directory to keep it in".to_owned(),
        )
    })?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(ConfigError::Content(ProjectError::Unreadable(
                error.to_string(),
            )));
        }
    };
    build(&parse_file(&text)?)?;
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|error| ConfigError::Content(ProjectError::Malformed(error.to_string())))?;
    edit(&mut doc)?;
    let edited = doc.to_string();
    build(&parse_file(&edited)?)?;
    write_config(&path, &edited)?;
    Ok(path)
}

/// Replace [`CONFIG_FILE`] with `text`, atomically.
///
/// A sibling temp is written, synced and renamed over the target, so an interrupted write leaves
/// the previous good config rather than a truncated one — this is the user's own file and there is
/// no second copy of it. The temp is keyed on the PID because any number of `sprag` processes may
/// run at once; the daemon's snapshot can use a fixed suffix only because one flock owns it.
///
/// The target's permissions are carried over when it already exists, so an edit never quietly
/// changes the mode a user chose for their own file; a new one takes whatever the umask gives.
fn write_config(path: &Path, text: &str) -> Result<(), ConfigError> {
    let unwritable =
        |what: &str, error: &std::io::Error| ConfigError::Unwritable(format!("{what}: {error}"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| unwritable("cannot create its directory", &error))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let replace = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        if let Ok(existing) = std::fs::metadata(path) {
            // Best effort: a filesystem that will not take the mode is not a reason to refuse an
            // edit the user asked for.
            let _ = file.set_permissions(existing.permissions());
        }
        file.write_all(text.as_bytes())?;
        // Durable before the rename, so a power loss cannot strand an empty file where a config was.
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    replace().map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        unwritable("cannot replace it", &error)
    })
}

/// Read + parse [`CONFIG_FILE`] at `path`, without interpreting any of its tables.
///
/// Shared by both readers so the file is parsed under ONE shape: `deny_unknown_fields` means a table
/// one reader did not know about would invalidate the file for the other.
fn read_file(path: &Path) -> Result<UserConfigFile, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| ConfigError::Content(ProjectError::Unreadable(error.to_string())))?;
    parse_file(&text)
}

/// Parse [`CONFIG_FILE`]'s text under the one shared shape.
///
/// Split out from [`read_file`] because the WRITERS need it without a file: what they check is the
/// text they are about to write, which has no path yet.
fn parse_file(text: &str) -> Result<UserConfigFile, ConfigError> {
    toml::from_str(text)
        .map_err(|error| ConfigError::Content(ProjectError::Malformed(error.to_string())))
}

/// The file's shape as written by a human — the same `[[command]]` entries a project declares, so a
/// user who has written one config can write the other, plus the keymap's three tables.
///
/// `deny_unknown_fields` for the reason the project file has it: a typo'd table that silently did
/// nothing would leave the author believing their config was accepted.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UserConfigFile {
    /// `[[command]]` entries; defaulted, so a config that declares none is valid.
    #[serde(default)]
    command: Vec<crate::project::DeclaredAction>,
    /// The `[options]` table — every named setting that is not a binding.
    ///
    /// A MAP rather than a field per option, so [`crate::options::OPTIONS`] stays the single list of
    /// what exists: a struct field would mean adding an option in two places and letting them drift.
    /// `deny_unknown_fields` cannot police a map's keys, so an unknown option NAME is refused by
    /// [`Options::set`] instead — which answers with the real names, rather than with serde's
    /// "unknown field".
    ///
    /// The VALUES are `toml::Value` rather than `String` because a number is written as a number:
    /// `gui-font = 20` is how a person writes a size, and demanding `"20"` would be the parser's
    /// convenience imposed on a hand-maintained file. [`declared_value`] renders the two spellings
    /// this accepts into the one string [`OptionKind::canonicalise`](crate::options::OptionKind)
    /// takes, and refuses the rest by NAMING the option.
    #[serde(default)]
    options: std::collections::BTreeMap<String, toml::Value>,
    /// `[[bind]]` entries, layered over the defaults in file order.
    #[serde(default)]
    bind: Vec<DeclaredBind>,
    /// `[[unbind]]` entries, removing a default.
    #[serde(default)]
    unbind: Vec<DeclaredUnbind>,
    /// `[[agent]]` entries, layered over [`sprag_detect::built_ins`] in file order.
    ///
    /// Read by the DAEMON rather than by a client, which is why nothing in [`build`] touches it: the
    /// keymap and the options are what a client needs to interpret a keystroke, and the manifests are
    /// what the detector needs to read a screen. One file, two readers, and each one validates only
    /// what it is going to use — so a broken `[[bind]]` cannot stop the daemon from detecting agents,
    /// and a broken `[[agent]]` cannot stop a client from starting.
    #[serde(default)]
    agent: Vec<DeclaredAgent>,
}

/// One `[[bind]]` entry — tmux's `bind-key [-n] [-r] key command`.
///
/// The two optional fields are why slice 1 made this an array of TABLES rather than a `key = action`
/// map: *"a map has nowhere to put a second field"*. They arrive here with no format change.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredBind {
    /// The key spec, e.g. `%` or `C-o`.
    key: String,
    /// The action, spelled as the shell spells it, e.g. `split-window -h`.
    action: String,
    /// Which table — `"prefix"` (the default) or `"root"`, tmux's `-T`. An unknown name is refused
    /// by [`KeyTable::parse`] rather than by serde, so the message can list the tables that exist.
    #[serde(default)]
    table: Option<String>,
    /// tmux's `-r`: hold the prefix table open for `repeat-time` after this acts.
    #[serde(default)]
    repeat: bool,
}

/// One `[[unbind]]` entry — tmux's `unbind-key [-n] key`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredUnbind {
    /// The key spec to remove.
    key: String,
    /// Which table to remove it from — see [`DeclaredBind::table`].
    #[serde(default)]
    table: Option<String>,
}

impl DeclaredBind {
    /// The table this entry names, defaulting to the prefix table.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownTable`] for a name that is not one of sprag's.
    fn table(&self) -> Result<KeyTable, KeyError> {
        declared_table(self.table.as_deref())
    }
}

impl DeclaredUnbind {
    /// The table this entry names, defaulting to the prefix table.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownTable`] for a name that is not one of sprag's.
    fn table(&self) -> Result<KeyTable, KeyError> {
        declared_table(self.table.as_deref())
    }
}

/// One `[[agent]]` entry — an agent manifest the user declares, layered over
/// [`sprag_detect::built_ins`].
///
/// # The layering, and why its grain is a RULE
///
/// H2's D5 settled the shape for the keymap — the defaults are a table and the file layers over it,
/// so a user corrects one binding without redeclaring the rest — and H3's D6 adopts it unchanged.
/// Where the two differ is the unit: a keymap's is a key, a manifest's is a rule. So an entry naming
/// a BUILT-IN agent layers into it rule by rule, matched on [`sprag_detect::Rule::id`], which R252
/// already made a stable name for exactly this use. A rule the built-in does not have is appended;
/// one it does have is replaced IN PLACE, keeping the position the built-in gave it — the treatment
/// [`bind_key`] gives a rebound key, and for the same reason: a corrected rule should stay where its
/// reader expects to find it.
///
/// An entry naming an agent no built-in declares is a NEW manifest, and it goes at the FRONT of the
/// list. Order is load-bearing there and only there: [`sprag_detect::detect`] offers a pane to
/// manifests until one CLAIMS it, so the front is what "the user's file wins" means for
/// identification.
///
/// # Why there is no way to remove a built-in AGENT, derived rather than omitted
///
/// The obvious fourth verb — drop `codex` entirely — has nothing left to do once the three above
/// exist, and that is a property of the code rather than a judgement about how much anyone wants it:
///
/// * A user's own manifest is consulted FIRST, so a built-in can never pre-empt a claim the user's
///   file makes on the same pane.
/// * A pane a manifest claims but no rule matches publishes NOTHING — `AgentRegistry::observe`
///   carries the absence through [`sprag_detect::AgentState::wire_str`] returning `None` — so
///   `disable`-ing an agent's rules already removes it from the wire completely, while keeping the
///   honest "I know what this is and not what it is doing" that a rule author debugging needs.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredAgent {
    /// The agent's name. A built-in's name layers into that built-in; any other name declares a new
    /// agent. Carried out on the verdict, so it is what a person reads in a pane list.
    name: String,
    /// `[[agent.fingerprint]]` — what identifies a pane as this agent's. ANY one is enough, which is
    /// why identification widens by APPENDING: a user adding a fingerprint for their wrapper script
    /// should not have to restate the ones that already work.
    #[serde(default)]
    fingerprint: Vec<DeclaredFingerprint>,
    /// `[[agent.rule]]` — what the pane is DOING, once it is known to be this agent's.
    #[serde(default)]
    rule: Vec<DeclaredRule>,
    /// Rule ids to drop from the built-in — `[[unbind]]`'s counterpart, one grain down.
    ///
    /// A rule that fires wrongly is usually CORRECTED by redeclaring its id, and that is the common
    /// case. This is for the one correction that cannot be written as a better pattern: the rule
    /// should not exist on this box at all.
    #[serde(default)]
    disable: Vec<String>,
}

/// One `[[agent.fingerprint]]` — a conjunction of matches; any one fingerprint claims the pane.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredFingerprint {
    /// Every match that must hold.
    ///
    /// An ARRAY rather than a table keyed by region, and the built-ins are what settle it: `codex`'s
    /// one fingerprint is a composer line in the bottom 3 rows AND a footer shape in the bottom 1, so
    /// a region-keyed table would have nowhere to put the second — the same reason the keymap's
    /// bindings are an array of tables rather than a map.
    all: Vec<DeclaredMatch>,
}

/// One `[[agent.rule]]` — a state, its evidence, and how strongly it outranks a competing rule.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredRule {
    /// The stable name this rule is corrected, disabled and EXPLAINED by. It rides the verdict (D7),
    /// so it is what answers "why does this pane say working".
    id: String,
    /// `working`, `blocked` or `idle` — see [`declared_state`].
    state: String,
    /// Higher wins; ties break by declaration order. Defaulted to zero, which is BELOW every
    /// built-in rule, so a rule meant to outrank one has to say so in the file rather than by
    /// accident of where it was written.
    #[serde(default)]
    priority: i32,
    /// Every match that must hold — a rule is made specific by conjunction rather than by one
    /// unreadable regex.
    all: Vec<DeclaredMatch>,
}

/// One match — which text to read, and the ONE test to judge it by.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredMatch {
    /// `title`, or `bottom:N` — see [`declared_region`].
    region: String,
    /// The region's text begins with this.
    #[serde(default)]
    starts_with: Option<String>,
    /// The region's text contains this anywhere.
    #[serde(default)]
    contains: Option<String>,
    /// The region's text matches this pattern. Compiled HERE, so a pattern that cannot compile is
    /// reported against the file rather than never matching for the life of the daemon.
    #[serde(default)]
    regex: Option<String>,
}

/// The `region = …` spelling: the OSC title, or the last N non-empty rows.
///
/// A single spec string rather than a region name beside a separate line count, because the count
/// belongs to exactly one of the regions: two fields would make `region = "title", lines = 4` a
/// state the reader has to refuse, and the spelling that cannot express it needs no refusal. This is
/// `KeySpec::parse`'s shape — a spec parsed at the edge, refusing an unknown one by NAMING what
/// exists.
fn declared_region(spec: &str) -> Result<sprag_detect::Region, String> {
    if spec == "title" {
        return Ok(sprag_detect::Region::Title);
    }
    if let Some(count) = spec.strip_prefix("bottom:") {
        let lines: u16 = count
            .parse()
            .map_err(|_| format!("region \"{spec}\": {count} is not a number of rows"))?;
        if lines == 0 {
            return Err(format!(
                "region \"{spec}\": a window of no rows reads no text, so nothing could match it",
            ));
        }
        return Ok(sprag_detect::Region::BottomLines(lines));
    }
    Err(format!(
        "region \"{spec}\": write \"title\" for the pane's title, or \"bottom:N\" for its last N \
         non-empty rows",
    ))
}

/// The `state = …` spelling, read out of the wire vocabulary itself.
///
/// The list comes from [`sprag_detect::AgentState::wire_str`] rather than being spelled again here,
/// so the file accepts exactly the states the wire can carry and the two cannot drift. `unknown`
/// falls out as unwritable without a second rule saying so — it has no wire token, because it IS the
/// absence of one — and that is the right answer: a rule concluding "no answer" would out-rank the
/// rules that have one while saying nothing, and a manifest that matches nothing already reaches
/// `Unknown` on its own.
fn declared_state(spec: &str) -> Result<sprag_detect::AgentState, String> {
    const STATES: [sprag_detect::AgentState; 3] = [
        sprag_detect::AgentState::Working,
        sprag_detect::AgentState::Blocked,
        sprag_detect::AgentState::Idle,
    ];
    STATES
        .into_iter()
        .find(|state| state.wire_str() == Some(spec))
        .ok_or_else(|| {
            let known: Vec<&str> = STATES.into_iter().filter_map(|s| s.wire_str()).collect();
            format!("state \"{spec}\": write one of {}", known.join(", "))
        })
}

impl DeclaredMatch {
    /// This entry as a matcher, with its pattern compiled.
    fn build(&self) -> Result<sprag_detect::Match, String> {
        Ok(sprag_detect::Match::new(
            declared_region(&self.region)?,
            self.test()?,
        ))
    }

    /// The one test this entry names.
    ///
    /// Written as a total match over the three fields rather than as a count followed by an unwrap,
    /// so "exactly one" is the shape of the code and not a fact asserted about it — and so the two
    /// ways to get it wrong get different messages.
    fn test(&self) -> Result<sprag_detect::Test, String> {
        match (&self.starts_with, &self.contains, &self.regex) {
            (Some(prefix), None, None) => Ok(sprag_detect::Test::StartsWith(prefix.clone())),
            (None, Some(needle), None) => Ok(sprag_detect::Test::Contains(needle.clone())),
            (None, None, Some(pattern)) => regex::Regex::new(pattern)
                .map(sprag_detect::Test::Regex)
                .map_err(|error| format!("regex {pattern:?} does not compile: {error}")),
            (None, None, None) => Err(format!(
                "the match on region \"{}\" names no test — give it one of starts_with, contains \
                 or regex",
                self.region,
            )),
            _ => Err(format!(
                "the match on region \"{}\" names more than one test — a match reads its region \
                 exactly one way",
                self.region,
            )),
        }
    }
}

/// The matches of one conjunction, refusing an empty one.
///
/// An empty conjunction HOLDS — `all` over nothing is true — so an empty fingerprint would claim
/// every pane in the workspace and an empty rule would fire on every screen. `Fingerprint`'s own
/// docs call that the manifest author's error rather than something the type can prevent; at the
/// FILE edge it can be prevented, which is the same argument that compiles a pattern here rather
/// than letting it fail silently forever.
fn declared_matches(
    all: &[DeclaredMatch],
    whose: &str,
) -> Result<Vec<sprag_detect::Match>, String> {
    if all.is_empty() {
        return Err(format!(
            "{whose}: `all` is empty, and a conjunction of no conditions holds for every pane",
        ));
    }
    all.iter().map(DeclaredMatch::build).collect()
}

impl DeclaredFingerprint {
    /// This entry as a fingerprint.
    fn build(&self, agent: &str) -> Result<sprag_detect::Fingerprint, String> {
        Ok(sprag_detect::Fingerprint::all(declared_matches(
            &self.all,
            &format!("agent \"{agent}\": a fingerprint"),
        )?))
    }
}

impl DeclaredRule {
    /// This entry as a rule.
    fn build(&self, agent: &str) -> Result<sprag_detect::Rule, String> {
        if self.id.is_empty() {
            return Err(format!(
                "agent \"{agent}\": a rule needs an id — it is what a verdict names when somebody \
                 asks why the pane reads the way it does",
            ));
        }
        Ok(sprag_detect::Rule {
            id: self.id.clone(),
            state: declared_state(&self.state)?,
            all: declared_matches(
                &self.all,
                &format!("agent \"{agent}\": rule \"{}\"", self.id),
            )?,
            priority: self.priority,
        })
    }
}

impl DeclaredAgent {
    /// Apply this entry to `manifest` — disable, then widen, then correct.
    ///
    /// The order is the file's meaning rather than an implementation detail: `disable` is about the
    /// rules that were ALREADY there, so it runs before this entry's own rules are laid down, and a
    /// file that both disables and declares one id is refused rather than resolved by whichever ran
    /// first. That refusal is `build`'s treatment of a key that is both bound and unbound, one table
    /// over — a file that says two things about one rule has not said what it wants.
    fn layer_onto(&self, manifest: &mut sprag_detect::Manifest) -> Result<(), String> {
        for id in &self.disable {
            if self.rule.iter().any(|rule| &rule.id == id) {
                return Err(format!(
                    "agent \"{}\": rule \"{id}\" is both declared and disabled",
                    self.name,
                ));
            }
            let known: Vec<&str> = manifest.rules.iter().map(|rule| rule.id.as_str()).collect();
            if !known.contains(&id.as_str()) {
                return Err(format!(
                    "agent \"{}\": there is no rule \"{id}\" to disable (it has {})",
                    self.name,
                    if known.is_empty() {
                        "none".to_owned()
                    } else {
                        known.join(", ")
                    },
                ));
            }
            manifest.rules.retain(|rule| &rule.id != id);
        }
        for fingerprint in &self.fingerprint {
            manifest.any.push(fingerprint.build(&self.name)?);
        }
        for declared in &self.rule {
            let rule = declared.build(&self.name)?;
            match manifest
                .rules
                .iter_mut()
                .find(|existing| existing.id == rule.id)
            {
                // In place, so a corrected rule keeps the position the built-in gave it — and so a
                // second entry for one agent corrects the first rather than shadowing it.
                Some(existing) => *existing = rule,
                None => manifest.rules.push(rule),
            }
        }
        Ok(())
    }
}

/// [`sprag_detect::built_ins`] with the file's `[[agent]]` entries layered over them.
///
/// The result is what every pane in the daemon is evaluated against, in the order a pane is offered
/// to them: the user's own agents first, then the built-ins in their own order.
///
/// A half-applied layering never escapes: an entry mutates the manifest it is laying into and can
/// still fail afterwards, and the caller discards the whole result — which is [`keymap`]'s rule
/// ("reported WHOLE"), holding here for the same reason. A list assembled from the half of a file
/// that parsed would be rules the user never wrote.
fn declared_manifests(declared: &[DeclaredAgent]) -> Result<Vec<sprag_detect::Manifest>, String> {
    let mut manifests = sprag_detect::built_ins();
    // How many entries at the FRONT are the user's own, so several new agents keep file order
    // instead of each one displacing the last.
    let mut user_agents = 0usize;
    for agent in declared {
        if agent.name.is_empty() {
            return Err("an [[agent]] needs a name — it is what a pane list shows".to_owned());
        }
        let index = match manifests.iter().position(|m| m.name == agent.name) {
            Some(index) => index,
            None => {
                if agent.fingerprint.is_empty() {
                    return Err(format!(
                        "agent \"{}\": a new agent needs at least one [[agent.fingerprint]], or \
                         no pane could ever be recognised as it",
                        agent.name,
                    ));
                }
                manifests.insert(
                    user_agents,
                    sprag_detect::Manifest {
                        name: agent.name.clone(),
                        any: Vec::new(),
                        rules: Vec::new(),
                    },
                );
                user_agents += 1;
                user_agents - 1
            }
        };
        agent.layer_onto(&mut manifests[index])?;
    }
    Ok(manifests)
}

/// A declared `table = …`, or [`KeyTable::Prefix`] when the entry is silent.
///
/// Silence means the prefix table because that is where every one of sprag's defaults lives and what
/// a `bind-key` with no `-T` means in tmux — the field is the departure, not the norm.
fn declared_table(name: Option<&str>) -> Result<KeyTable, KeyError> {
    name.map_or(Ok(KeyTable::Prefix), KeyTable::parse)
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
    use sprag_input::Modifiers;
    // `Screen` is reached through the port trait, which the agent tests one module over also import
    // for the same reason: a manifest is judged against a screen, and the screen comes from an
    // emulator that has been painted.
    use sprag_vt::VtPort as _;

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

    /// No file, and a file that declares no keys, both mean the DEFAULT keymap — not an error and
    /// not an empty table.
    #[test]
    fn a_user_who_has_said_nothing_about_keys_gets_the_defaults() {
        with_config(None, || {
            assert_eq!(
                keymap().expect("no file is not an error"),
                Keymap::default()
            );
        });
        with_config(Some("[[command]]\nname = \"a\"\nrun = [\"x\"]\n"), || {
            assert_eq!(
                keymap().expect("a keyless config is valid"),
                Keymap::default()
            );
        });
    }

    /// **The cross-reader test.** A user who only wanted to rebind a key must not break the palette:
    /// `deny_unknown_fields` means an unknown table invalidates the WHOLE file, so the commands
    /// reader has to know the keymap's tables exist even though it ignores them.
    ///
    /// REVERT-PROOF: drop `keys`/`bind`/`unbind` from `UserConfigFile` and `load()` fails here with
    /// "unknown field", i.e. rebinding a key would empty the user's command palette.
    #[test]
    fn declaring_keys_does_not_invalidate_the_commands_half() {
        let text = "[options]\nprefix = \"C-a\"\n\n[[bind]]\nkey = \"c\"\naction = \"split-window -h\"\n\n\
                    [[unbind]]\nkey = \"o\"\n\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n";
        with_config(Some(text), || {
            let config = load().expect("the file exists").expect("and is valid");
            assert_eq!(config.commands.len(), 1, "the commands still read");
            let keymap = keymap().expect("and so do the keys");
            assert_eq!(keymap.prefix().to_string(), "C-a");
            assert!(
                keymap
                    .action(KeyTable::Prefix, "c", Modifiers::default())
                    .is_some(),
                "the declared bind is there",
            );
            assert_eq!(
                keymap.action(KeyTable::Prefix, "o", Modifiers::default()),
                None,
                "the unbound default is gone",
            );
            assert_eq!(
                keymap.action(KeyTable::Prefix, "d", Modifiers::default()),
                Some(crate::keymap::BoundAction::DetachClient),
                "and every default the file did not mention survives",
            );
        });
    }

    /// Within `[[bind]]`, file order is total and the later entry wins — the rule that makes a
    /// declarative file behave like tmux's sequence of `bind-key` commands.
    #[test]
    fn the_later_binding_of_one_key_wins() {
        let text = "[[bind]]\nkey = \"x\"\naction = \"detach-client\"\n\
                    [[bind]]\nkey = \"x\"\naction = \"select-pane -t :.+\"\n";
        with_config(Some(text), || {
            assert_eq!(
                keymap()
                    .expect("valid")
                    .action(KeyTable::Prefix, "x", Modifiers::default()),
                Some(crate::keymap::BoundAction::SelectNextPane),
            );
        });
    }

    /// A key both bound and unbound is REFUSED rather than resolved by precedence — the file has not
    /// said what it wants, and the report names the key and the file to fix.
    #[test]
    fn a_key_both_bound_and_unbound_is_refused() {
        let text = "[[bind]]\nkey = \"C-o\"\naction = \"detach-client\"\n\
                    [[unbind]]\nkey = \"^o\"\n";
        with_config(Some(text), || {
            let message = keymap().expect_err("contradictory").to_string();
            assert!(message.contains(CONFIG_FILE), "{message:?}");
            assert!(
                message.contains("C-o") && message.contains("both bound and unbound"),
                "the report names the key: {message:?}",
            );
        });
    }

    /// The file's `table` and `repeat` fields reach the keymap, and the default of each is the one
    /// a silent entry means.
    #[test]
    fn the_file_declares_a_table_and_a_repeat() {
        let text = "[[bind]]\nkey = \"F5\"\naction = \"detach-client\"\ntable = \"root\"\n\
                    [[bind]]\nkey = \"o\"\naction = \"select-pane -t :.+\"\nrepeat = true\n\
                    [[bind]]\nkey = \"x\"\naction = \"detach-client\"\n";
        with_config(Some(text), || {
            let keymap = keymap().expect("valid");
            let bind = |key: &str| {
                keymap
                    .binds()
                    .find(|bind| bind.key().to_string() == key)
                    .expect("declared")
            };
            assert_eq!(bind("F5").table(), KeyTable::Root);
            assert!(!bind("F5").repeats());
            assert!(bind("o").repeats());
            assert_eq!(bind("o").table(), KeyTable::Prefix, "silence means prefix");
            assert_eq!(bind("x").table(), KeyTable::Prefix);
            assert!(!bind("x").repeats(), "and silence means no repeat");
        });
    }

    /// A `table` the file spells wrong is refused, and the report names the file and the tables that
    /// exist. Never defaulted — a binding silently moved into the prefix table is one the user would
    /// see in `list-keys` and believe they had asked for.
    #[test]
    fn an_unknown_table_in_the_file_names_the_file() {
        let text = "[[bind]]\nkey = \"x\"\naction = \"detach-client\"\ntable = \"copy-mode\"\n";
        with_config(Some(text), || {
            let message = keymap().expect_err("no such table").to_string();
            assert!(message.contains(CONFIG_FILE), "{message:?}");
            assert!(
                message.contains("copy-mode") && message.contains("root"),
                "the report names what was asked for and what exists: {message:?}",
            );
        });
    }

    /// A root binding that asks to repeat is refused from the FILE too, not only from the CLI —
    /// the two are one rule with two front doors.
    #[test]
    fn a_repeating_root_binding_in_the_file_is_refused() {
        let text =
            "[[bind]]\nkey = \"F5\"\naction = \"detach-client\"\ntable = \"root\"\nrepeat = true\n";
        with_config(Some(text), || {
            let message = keymap().expect_err("cannot repeat").to_string();
            assert!(
                message.contains("repeat") && message.contains("prefix"),
                "the report names the mechanism: {message:?}",
            );
        });
    }

    /// **The bound-and-unbound contradiction is PER TABLE.** `%` in the root table and `%` in the
    /// prefix table are two keys, so binding one and unbinding the other says two things about two
    /// keys — which is a config, not a puzzle.
    ///
    /// REVERT-PROOF: compare the key alone and this valid file is refused, with a message naming a
    /// contradiction the user did not write.
    #[test]
    fn bound_in_one_table_and_unbound_in_the_other_is_not_a_contradiction() {
        let text = "[[bind]]\nkey = \"%\"\naction = \"detach-client\"\ntable = \"root\"\n\
                    [[unbind]]\nkey = \"%\"\n";
        with_config(Some(text), || {
            let keymap = keymap().expect("two statements about two keys");
            let none = Modifiers::default();
            assert_eq!(
                keymap.action(KeyTable::Root, "%", none),
                Some(crate::keymap::BoundAction::DetachClient),
            );
            assert_eq!(
                keymap.action(KeyTable::Prefix, "%", none),
                None,
                "and the prefix table's default was the one taken away",
            );
        });

        // ...while the SAME table still is one.
        let same = "[[bind]]\nkey = \"%\"\naction = \"detach-client\"\ntable = \"root\"\n\
                    [[unbind]]\nkey = \"%\"\ntable = \"root\"\n";
        with_config(Some(same), || {
            let message = keymap().expect_err("contradictory").to_string();
            assert!(message.contains("both bound and unbound"), "{message:?}");
        });
    }

    /// `repeat-time` reaches the keymap out of the options table, the same way the prefix does.
    #[test]
    fn the_repeat_time_option_reaches_the_keymap() {
        with_config(Some("[options]\nrepeat-time = 120\n"), || {
            assert_eq!(
                keymap().expect("valid").repeat_time(),
                std::time::Duration::from_millis(120),
            );
        });
        with_config(Some(""), || {
            assert_eq!(
                keymap().expect("valid").repeat_time(),
                crate::keymap::DEFAULT_REPEAT_TIME,
                "and a silent file gets the default the keymap itself ships",
            );
        });
    }

    /// The two spellings of the repeat default must not drift: the options table's string and
    /// [`crate::keymap::DEFAULT_REPEAT_TIME`] are the same number.
    ///
    /// Both have to exist — `Keymap::default()` answers with no config file at all, which is what
    /// `sprag list-keys` runs on — so the guard is a test rather than one constant. The treatment
    /// `history-limit` gets against the emulator's own default.
    #[test]
    fn the_repeat_time_default_is_the_keymaps_own() {
        let declared: u64 = options::spec(options::REPEAT_TIME)
            .expect("a registered option")
            .default
            .parse()
            .expect("a number");
        assert_eq!(
            std::time::Duration::from_millis(declared),
            crate::keymap::DEFAULT_REPEAT_TIME,
        );
    }

    /// The two spellings of the settle window must not drift: the options table's string and
    /// [`sprag_detect::DEFAULT_SETTLE`] are the same duration.
    ///
    /// Both have to exist — a `Tracker` built with `Hysteresis::default()` answers with no config file
    /// anywhere, which is what every unit test in `sprag-detect` runs on — so the guard is a test
    /// rather than one constant. The treatment `repeat-time` gets against the keymap and
    /// `history-limit` against the emulator.
    #[test]
    fn the_agent_settle_default_is_the_detectors_own() {
        let declared: u64 = options::spec(options::AGENT_SETTLE_TIME)
            .expect("a registered option")
            .default
            .parse()
            .expect("a number");
        assert_eq!(
            std::time::Duration::from_millis(declared),
            sprag_detect::DEFAULT_SETTLE,
        );
    }

    /// And the reader in force agrees with both, on a file that says nothing — the assertion that
    /// would catch a default parsed but never applied.
    #[test]
    fn a_silent_file_gets_the_detectors_settle_window() {
        with_config(None, || {
            assert_eq!(agent_settle().settle, sprag_detect::DEFAULT_SETTLE);
        });
    }

    /// The text `config.toml` holds right now, for the writer tests.
    fn written() -> String {
        std::fs::read_to_string(config_path().expect("a config path")).expect("the file exists")
    }

    /// Parse a key spec in a test, where a malformed one is the test's own bug.
    fn key(spec: &str) -> KeySpec {
        KeySpec::parse(spec).unwrap_or_else(|error| panic!("{spec:?}: {error}"))
    }

    /// Parse an action in a test.
    fn action(text: &str) -> BoundAction {
        BoundAction::parse(text).unwrap_or_else(|error| panic!("{text:?}: {error}"))
    }

    /// **A binding lands in the file and the rest of the file SURVIVES.** This is the whole reason
    /// the edit goes through `toml_edit` rather than serializing a struct back out: the file is
    /// hand-maintained, and a config tool that silently ate a user's comments is one nobody uses
    /// twice.
    ///
    /// REVERT-PROOF: re-serialize the parsed `UserConfigFile` instead of editing the document, and
    /// the comment, the blank lines and the `[options]` table's inline note all disappear.
    #[test]
    fn a_bound_key_lands_in_the_file_and_the_rest_of_it_survives() {
        let text = "# keep me\n[options]\nprefix = \"C-a\"  # and me\n\n\
                    [[command]]\nname = \"top\"\nrun = [\"htop\"]\n";
        with_config(Some(text), || {
            bind_key(
                KeyTable::Prefix,
                &key("c"),
                action("split-window -h"),
                false,
            )
            .expect("binds");
            let after = written();
            assert!(after.contains("# keep me"), "{after:?}");
            assert!(after.contains("# and me"), "{after:?}");
            assert!(after.contains("[[command]]"), "{after:?}");
            // ...and the binding is really there, read back through the ordinary reader.
            let keymap = keymap().expect("the written file is valid");
            assert_eq!(
                keymap.action(KeyTable::Prefix, "c", Modifiers::default()),
                Some(crate::keymap::BoundAction::SplitWindow {
                    dir: sprag_terminal::SplitDir::Horizontal,
                    before: false
                }),
            );
            assert_eq!(keymap.prefix().to_string(), "C-a", "and nothing else moved");
        });
    }

    /// Binding a key the file UNBOUND removes the unbind — otherwise the edit would leave behind
    /// exactly the contradiction the reader refuses (`BoundAndUnbound`), i.e. a `bind-key` that
    /// made the whole config unusable.
    ///
    /// REVERT-PROOF: drop the `remove_named(UNBIND_ARRAY)` call and this fails at the WRITE, in
    /// `edit_config`'s read-back — which is that guard doing its job.
    #[test]
    fn binding_a_key_the_file_unbound_takes_the_unbind_out() {
        with_config(Some("[[unbind]]\nkey = \"o\"\n"), || {
            bind_key(KeyTable::Prefix, &key("o"), action("detach-client"), false).expect("binds");
            assert!(!written().contains("[[unbind]]"), "{:?}", written());
            assert_eq!(
                keymap()
                    .expect("valid")
                    .action(KeyTable::Prefix, "o", Modifiers::default()),
                Some(crate::keymap::BoundAction::DetachClient),
            );
        });
    }

    /// **Unbinding records an `[[unbind]]` only when a DEFAULT would otherwise come back.** A key
    /// the user themselves bound needs the bind removed and nothing else; writing a line to
    /// suppress a table that no longer mentions the key would be noise in a file a human reads.
    #[test]
    fn unbinding_suppresses_a_default_and_only_a_default() {
        with_config(
            Some("[[bind]]\nkey = \"c\"\naction = \"detach-client\"\n"),
            || {
                unbind_key(KeyTable::Prefix, &key("c")).expect("unbinds");
                let after = written();
                assert!(!after.contains("[[bind]]"), "the binding went: {after:?}");
                assert!(
                    !after.contains("[[unbind]]"),
                    "`c` is not a default, so there is nothing to suppress: {after:?}"
                );
                // A DEFAULT does get recorded, because the layering would restore it otherwise.
                unbind_key(KeyTable::Prefix, &key("o")).expect("unbinds");
                assert!(written().contains("[[unbind]]"), "{:?}", written());
                assert_eq!(
                    keymap()
                        .expect("valid")
                        .action(KeyTable::Prefix, "o", Modifiers::default()),
                    None,
                );
            },
        );
    }

    /// An edit matches a key by what it MEANS, not by how the file happened to spell it. `^o` and
    /// `C-o` are one keystroke, so an entry written one way must be found by the other.
    ///
    /// REVERT-PROOF: compare the raw strings in `names` and this leaves TWO `[[bind]]` entries for
    /// one key — a table the reader resolves by file order while `list-keys` prints both.
    #[test]
    fn an_edit_finds_a_key_however_the_file_spelled_it() {
        with_config(
            Some("[[bind]]\nkey = \"^o\"\naction = \"detach-client\"\n"),
            || {
                bind_key(KeyTable::Prefix, &key("C-o"), action("send-prefix"), false)
                    .expect("binds");
                let after = written();
                assert_eq!(after.matches("action =").count(), 1, "one entry: {after:?}");
                assert!(after.contains("send-prefix"), "{after:?}");
                // ...and an unbind reaches it through the other spelling too.
                unbind_key(KeyTable::Prefix, &key("^o")).expect("unbinds");
                assert!(!written().contains("[[bind]]"), "{:?}", written());
            },
        );
    }

    /// Rebinding replaces the entry WHERE IT WAS, so a user's file keeps the order they gave it —
    /// the same rule `Keymap::bind` applies to the table.
    #[test]
    fn rebinding_a_key_replaces_it_in_place() {
        let text = "[[bind]]\nkey = \"a\"\naction = \"detach-client\"\n\
                    [[bind]]\nkey = \"b\"\naction = \"send-prefix\"\n";
        with_config(Some(text), || {
            bind_key(
                KeyTable::Prefix,
                &key("a"),
                action("select-pane -t :.+"),
                false,
            )
            .expect("binds");
            let after = written();
            let a = after.find("key = \"a\"").expect("a is there");
            let b = after.find("key = \"b\"").expect("b is there");
            assert!(a < b, "the user's order survived: {after:?}");
            assert_eq!(after.matches("[[bind]]").count(), 2);
        });
    }

    /// A written binding carries `table` / `repeat` only when they are NOT the default, and a
    /// rebind that drops a flag REMOVES the line rather than leaving it.
    ///
    /// REVERT-PROOF for the removal half: write the field unconditionally and a key that stops
    /// repeating keeps `repeat = true` in the user's file — a config saying one thing while the
    /// keymap does another, discovered only when someone reads the file back.
    #[test]
    fn a_written_binding_states_only_what_is_not_the_default() {
        with_config(Some(""), || {
            bind_key(KeyTable::Root, &key("F5"), action("detach-client"), false).expect("binds");
            let after = written();
            assert!(after.contains("table = \"root\""), "{after:?}");
            assert!(
                !after.contains("repeat"),
                "no flag it did not ask for: {after:?}"
            );

            bind_key(
                KeyTable::Prefix,
                &key("o"),
                action("select-pane -t :.+"),
                true,
            )
            .expect("binds");
            let after = written();
            assert!(after.contains("repeat = true"), "{after:?}");
            assert_eq!(
                after.matches("table =").count(),
                1,
                "the prefix table is silence, not `table = \"prefix\"`: {after:?}",
            );

            bind_key(KeyTable::Prefix, &key("o"), action("detach-client"), false).expect("rebinds");
            let after = written();
            assert!(
                !after.contains("repeat"),
                "the flag went with the binding it was on: {after:?}",
            );
            assert_eq!(after.matches("[[bind]]").count(), 2, "{after:?}");
        });
    }

    /// **An edit reaches one TABLE's entry, not every entry with that key.** Binding `%` in the root
    /// table must leave the `%` a user already had in the prefix table exactly where it was.
    ///
    /// REVERT-PROOF: match on the key alone in `names` and this rewrites the prefix entry instead of
    /// adding a root one — silently changing a binding the user did not name, and only for users who
    /// had bothered to bind both.
    #[test]
    fn an_edit_reaches_one_tables_entry() {
        let text = "[[bind]]\nkey = \"%\"\naction = \"detach-client\"\n";
        with_config(Some(text), || {
            bind_key(KeyTable::Root, &key("%"), action("send-prefix"), false).expect("binds");
            let after = written();
            assert_eq!(after.matches("[[bind]]").count(), 2, "{after:?}");
            assert!(
                after.contains("detach-client"),
                "the first survived: {after:?}"
            );
            assert!(
                after.contains("send-prefix"),
                "and the second landed: {after:?}"
            );

            unbind_key(KeyTable::Root, &key("%")).expect("unbinds");
            let after = written();
            assert!(
                after.contains("detach-client"),
                "removing the root one left the prefix one: {after:?}",
            );
            assert_eq!(after.matches("[[bind]]").count(), 1, "{after:?}");
        });
    }

    /// **A file this reader cannot use is not a file a writer may rewrite.** The edit is refused,
    /// the report names what is wrong with it, and not one byte moves.
    ///
    /// The discriminating case is the SECOND one, and finding that out is the story of this test.
    /// The first — an unrelated key bound while some other line is broken — is caught by the
    /// read-back AFTER the edit just as well as by the check before it, so with the pre-check
    /// removed it still passed: two correct filters in series, the second hiding the first.
    ///
    /// What only the pre-check can catch is an edit that REMOVES the broken thing. Retargeting `x`
    /// overwrites the unusable action, so the result reads back clean and would be WRITTEN —
    /// silently repairing a line the user never named, in a file every client is currently
    /// refusing to start against, and leaving them believing it is fine.
    ///
    /// REVERT-PROOF: drop the pre-edit `build` and the second half fails. The first half
    /// does not, which is the point.
    #[test]
    fn an_edit_refuses_a_config_it_cannot_read_and_changes_nothing() {
        let text = "[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n";
        for edited in [key("c"), key("x")] {
            with_config(Some(text), || {
                let message = bind_key(KeyTable::Prefix, &edited, action("detach-client"), false)
                    .expect_err("refused")
                    .to_string();
                assert!(message.contains(CONFIG_FILE), "{message:?}");
                assert!(message.contains("is not an action"), "{message:?}");
                assert_eq!(written(), text, "the file is untouched");
            });
        }
    }

    /// With no file at all, an edit CREATES one — a user's first `bind-key` is exactly the moment
    /// they have no config yet.
    #[test]
    fn an_edit_creates_the_file_when_there_is_none() {
        with_config(None, || {
            assert!(load().is_none(), "nothing to start with");
            bind_key(
                KeyTable::Prefix,
                &key("C-x"),
                action("detach-client"),
                false,
            )
            .expect("binds");
            assert_eq!(
                keymap().expect("valid").action(
                    KeyTable::Prefix,
                    "x",
                    Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    }
                ),
                Some(crate::keymap::BoundAction::DetachClient),
            );
        });
    }

    /// Unbinding is IDEMPOTENT at the file level too: asking twice leaves the same content, so a
    /// script that runs its config on every shell start does not grow the file each time.
    #[test]
    fn unbinding_twice_writes_the_same_file() {
        with_config(Some(""), || {
            unbind_key(KeyTable::Prefix, &key("o")).expect("unbinds");
            let once = written();
            unbind_key(KeyTable::Prefix, &key("o")).expect("unbinds again");
            assert_eq!(written(), once);
        });
    }

    /// **A running client's table FOLLOWS the file.** This is what makes `sprag bind-key` a runtime
    /// command rather than a setting for the next attach.
    ///
    /// REVERT-PROOF: have `refresh` return `Ok(false)` unconditionally and the second assertion
    /// fails — the client keeps routing `c` to nothing while `list-keys` shows it bound, which is
    /// the exact divergence the whole design exists to prevent.
    #[test]
    fn a_running_table_follows_the_file() {
        with_config(Some(""), || {
            let mut live = ClientConfig::load().expect("loads");
            assert_eq!(
                live.keymap()
                    .action(KeyTable::Prefix, "c", Modifiers::default()),
                None
            );

            bind_key(KeyTable::Prefix, &key("c"), action("detach-client"), false).expect("binds");
            assert!(live.refresh().expect("re-reads"), "the table moved");
            assert_eq!(
                live.keymap()
                    .action(KeyTable::Prefix, "c", Modifiers::default()),
                Some(crate::keymap::BoundAction::DetachClient),
            );
            // An unchanged file is not a change, so a caller never acts on an edit that was not one.
            assert!(!live.refresh().expect("re-reads"), "and stays put");
        });
    }

    /// The PREFIX follows too — which is the half `sprag bind-key` cannot reach (rebinding it is
    /// tmux's `set-option`, and sprag has no options table), so the editor is the only way in.
    #[test]
    fn a_running_table_follows_a_hand_edited_prefix() {
        with_config(Some(""), || {
            let mut live = ClientConfig::load().expect("loads");
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            assert!(live.keymap().is_prefix("b", ctrl), "the default to start");
            std::fs::write(
                config_path().expect("a path"),
                "[options]\nprefix = \"C-a\"\n",
            )
            .expect("the user's editor saves");
            assert!(live.refresh().expect("re-reads"));
            assert!(live.keymap().is_prefix("a", ctrl), "the gate moved");
            assert!(!live.keymap().is_prefix("b", ctrl));
        });
    }

    /// **A broken save KEEPS the last good table**, and is reported ONCE rather than on every
    /// keystroke that follows. A client owns the screen and cannot print, so the alternative —
    /// falling back to the defaults — would silently take a user's own bindings away because they
    /// typo'd a line in an editor.
    #[test]
    fn a_broken_save_keeps_the_last_good_table_and_reports_it_once() {
        with_config(
            Some("[[bind]]\nkey = \"c\"\naction = \"detach-client\"\n"),
            || {
                let mut live = ClientConfig::load().expect("loads");
                std::fs::write(config_path().expect("a path"), "[[bind]]\nkey = [\n")
                    .expect("a half-typed save");
                assert!(live.refresh().is_err(), "the save is reported");
                assert_eq!(
                    live.keymap()
                        .action(KeyTable::Prefix, "c", Modifiers::default()),
                    Some(crate::keymap::BoundAction::DetachClient),
                    "and the working table is kept",
                );
                assert!(
                    live.refresh().is_ok(),
                    "reported once, not on every keystroke until it is fixed",
                );
            },
        );
    }

    /// A DELETED config means the defaults — the same thing "there never was one" means, so a
    /// client that outlives an `rm` does not keep bindings the file no longer declares.
    #[test]
    fn a_deleted_config_returns_the_defaults() {
        with_config(Some("[[unbind]]\nkey = \"d\"\n"), || {
            let mut live = ClientConfig::load().expect("loads");
            assert_eq!(
                live.keymap()
                    .action(KeyTable::Prefix, "d", Modifiers::default()),
                None
            );
            std::fs::remove_file(config_path().expect("a path")).expect("the user deletes it");
            assert!(live.refresh().expect("re-reads"));
            assert_eq!(
                live.keymap()
                    .action(KeyTable::Prefix, "d", Modifiers::default()),
                Some(crate::keymap::BoundAction::DetachClient),
            );
        });
    }

    /// A broken KEY or ACTION is refused whole, and the report names `config.toml` — the same
    /// contract the commands half already has, through the same wrapper.
    #[test]
    fn a_broken_key_or_action_is_refused_and_the_report_names_this_file() {
        for (text, expected) in [
            ("[options]\nprefix = \"C-\"\n", "is not a key"),
            (
                "[[bind]]\nkey = \"Up\"\naction = \"detach-client\"\n",
                "is not a key",
            ),
            (
                "[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n",
                "is not an action",
            ),
            (
                "[[bind]]\nkey = \"x\"\naction = \"split-window\"\n",
                "needs -h",
            ),
            ("[[unbind]]\nkey = \"BSpace\"\n", "is not a key"),
            (
                "[[bind]]\nkey = \"x\"\naciton = \"detach-client\"\n",
                "not valid TOML",
            ),
        ] {
            with_config(Some(text), || {
                let message = keymap().expect_err("is refused").to_string();
                assert!(message.contains(CONFIG_FILE), "{message:?}");
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

    /// A temporary directory holding a `config.toml`, removed on drop — for the tests that name their
    /// file rather than reaching it through the environment.
    #[cfg(test)]
    struct NamedConfig(PathBuf);

    #[cfg(test)]
    impl NamedConfig {
        fn new(text: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sprag-keymap-at-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&dir).expect("temp config dir");
            let config = Self(dir);
            config.write(text);
            config
        }

        fn path(&self) -> PathBuf {
            self.0.join(CONFIG_FILE)
        }

        fn write(&self, text: &str) {
            std::fs::write(self.path(), text).expect("write config");
        }
    }

    #[cfg(test)]
    impl Drop for NamedConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// [`ClientConfig::load_usable`]'s contract, exercised through [`ClientConfig::at`]: a file that
    /// cannot be used yields a WORKING table plus the reason, the SAME file is not re-reported, and a
    /// fix is noticed.
    ///
    /// The middle claim is the one that costs something to get right. **REVERT-PROOF: leave `text` as
    /// `None` when the build fails** (i.e. do not remember the broken bytes) and the second assertion
    /// fails — `refresh` sees content where it remembered none, rebuilds, and reports the same typo on
    /// every keystroke until it is fixed, which is the report-once rule the type exists to keep.
    #[test]
    fn an_unusable_file_keeps_a_working_table_reports_once_and_notices_the_fix() {
        let config = NamedConfig::new("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
        let (mut file, error) = ClientConfig::at(&config.path());
        let error = error.expect("an unusable file reports why").to_string();
        assert!(
            error.contains(CONFIG_FILE) && error.contains("kill-server"),
            "{error:?}"
        );
        assert_eq!(
            file.keymap(),
            &Keymap::default(),
            "and the table is usable meanwhile",
        );
        assert_eq!(
            file.refresh(),
            Ok(false),
            "the same broken file is not re-reported",
        );
        // ...and that silence is NOT "nothing is wrong": the verdict stands until a read that looked
        // at the content says otherwise. A caller keeping its own copy of the reason had to guess
        // which of the two an `Ok(false)` meant, and guessed wrong — a surface showing the report
        // cleared it by asking twice.
        assert!(
            file.unusable().is_some(),
            "an unmoved broken file is still broken",
        );
        config.write("[[bind]]\nkey = \"x\"\naction = \"detach-client\"\n");
        assert_eq!(file.refresh(), Ok(true), "the fix is noticed");
        assert_eq!(file.unusable(), None, "...and the verdict clears with it");
        assert_eq!(
            file.keymap()
                .action(KeyTable::Prefix, "x", Modifiers::default()),
            Some(crate::keymap::BoundAction::DetachClient),
        );
    }

    /// A file that does not exist is not an error — it is "the user has said nothing" — and it is
    /// watched anyway, so writing one for the first time takes effect without a restart.
    #[test]
    fn a_file_that_is_not_there_yet_is_still_watched() {
        let config = NamedConfig::new("");
        std::fs::remove_file(config.path()).expect("remove it again");
        let (mut file, error) = ClientConfig::at(&config.path());
        assert_eq!(error, None);
        assert_eq!(file.keymap(), &Keymap::default());
        config.write("[options]\nprefix = \"C-a\"\n");
        assert_eq!(file.refresh(), Ok(true));
        assert_eq!(file.keymap().prefix().to_string(), "C-a");
    }
    /// An edit that moves the KEYMAP must not leave the OPTIONS behind.
    ///
    /// The trap this pins is a short-circuit: writing the two verdicts as
    /// `replace(keymap) != keymap || replace(options) != options` never runs the second swap on any
    /// re-read that changed the keymap, so the holder would answer with a NEW table and a STALE
    /// option — and only for edits that touched both, which is what makes it the kind of bug that
    /// ships. REVERT-PROOF: combine the two with `||` in `refresh` and this test fails on the
    /// `detach-on-destroy` assertion while every other test in this module still passes.
    #[test]
    fn an_edit_that_moves_both_tables_moves_both() {
        let config = NamedConfig::new("[options]\nprefix = \"C-a\"\n");
        let (mut file, error) = ClientConfig::at(&config.path());
        assert_eq!(error, None);
        assert_eq!(file.options().get(options::DETACH_ON_DESTROY), Some("on"));

        config.write("[options]\nprefix = \"C-o\"\ndetach-on-destroy = \"next\"\n");
        assert_eq!(file.refresh(), Ok(true));
        assert_eq!(
            file.keymap().prefix().to_string(),
            "C-o",
            "the keymap moved"
        );
        assert_eq!(
            file.options().get(options::DETACH_ON_DESTROY),
            Some("next"),
            "and so did the option, in the same re-read",
        );
    }

    /// An option changing ALONE is a change: the holder reports it, so a client acting on `Ok(true)`
    /// is not told to ignore an edit that was one.
    #[test]
    fn an_option_moving_alone_is_reported_as_a_change() {
        let config = NamedConfig::new("");
        let (mut file, _) = ClientConfig::at(&config.path());
        config.write("[options]\ndetach-on-destroy = \"off\"\n");
        assert_eq!(file.refresh(), Ok(true));
        assert_eq!(file.options().get(options::DETACH_ON_DESTROY), Some("off"));
        assert_eq!(file.keymap(), &Keymap::default(), "the keymap did not move");
    }

    /// An option the registry does not know is refused, and the report names THIS file — the other
    /// half of the split the CLI's argument errors are on. A user who mistyped a name in their editor
    /// has to be sent to the file; one who mistyped it on a command line must not be.
    #[test]
    fn an_unknown_option_in_the_file_names_the_file() {
        with_config(Some("[options]\nprefixx = \"C-a\"\n"), || {
            let error = options().expect_err("an unknown option is refused");
            let message = error.to_string();
            assert!(
                message.starts_with(CONFIG_FILE) && message.contains("prefixx"),
                "got {message:?}",
            );
        });
    }

    /// A value the option will not take is refused WHOLE, like a bad binding: the keymap half of a
    /// file whose options are broken is a table the user never wrote.
    #[test]
    fn a_bad_option_value_in_the_file_refuses_the_whole_config() {
        with_config(
            Some(
                "[options]\ndetach-on-destroy = \"maybe\"\n\n[[bind]]\nkey = \"c\"\naction = \"detach-client\"\n",
            ),
            || {
                assert!(options().is_err(), "the options half refuses");
                assert!(
                    keymap().is_err(),
                    "and so does the keymap half — one document, one verdict",
                );
            },
        );
    }

    /// An option edit is REFUSED when the file is already broken, and changes nothing.
    ///
    /// A writer has no business rewriting a config it cannot understand. Which of `edit_config`'s two
    /// validations refuses this is deliberately NOT asserted: R236 measured that the post-edit
    /// read-back catches the same input, so a test claiming the pre-check would be claiming more than
    /// it can see. What is pinned is the pair — refused, and the file untouched.
    #[test]
    fn an_option_edit_refuses_a_config_it_cannot_read_and_changes_nothing() {
        let broken = "[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n";
        with_config(Some(broken), || {
            let setting = OptionSetting::parse(options::PREFIX, "C-a").expect("a valid setting");
            assert!(set_option(&setting).is_err(), "the edit is refused");
            let path = config_path().expect("a path");
            assert_eq!(
                std::fs::read_to_string(path).expect("still there"),
                broken,
                "and the file is untouched",
            );
        });
    }

    /// An unset that empties `[options]` leaves the HEADER the user wrote.
    ///
    /// Not tidiness deferred — the rule that refuses to rewrite an inline array, one table over: an
    /// edit that deletes a header nobody asked about has reformatted a file the user maintains, and a
    /// config editor that does that cannot be trusted twice. The empty table means exactly what its
    /// absence means, so nothing is lost.
    #[test]
    fn an_unset_that_empties_the_table_keeps_the_users_header() {
        with_config(Some("# mine\n[options]\nprefix = \"C-a\"\n"), || {
            let spec = options::spec(options::PREFIX).expect("prefix is an option");
            let path = unset_option(spec).expect("the unset lands");
            let text = std::fs::read_to_string(path).expect("read it back");
            assert!(!text.contains("C-a"), "the value went: {text:?}");
            assert!(text.contains("[options]"), "the header stayed: {text:?}");
            assert!(text.contains("# mine"), "and the comment: {text:?}");
        });
    }

    /// An option lands INSIDE `[options]`, and the read-back is what makes that safe.
    ///
    /// The structural claim the behavioural tests do not make: a key written at the document ROOT
    /// reads back identically to a human and is refused by the file's shape, so the failure mode is a
    /// writer that cannot write rather than a config that stops working. REVERT-PROOF: assign to the
    /// document root instead of the table and this fails on the write — measured, and measured through
    /// THIS suite rather than only through the CLI's, which is where it was first read as a pass.
    #[test]
    fn a_set_option_lands_in_the_options_table() {
        with_config(
            Some("# mine\n[[bind]]\nkey = \"c\"\naction = \"detach-client\"\n"),
            || {
                let setting = OptionSetting::parse(options::PREFIX, "^a").expect("a valid setting");
                let path = set_option(&setting).expect("the edit lands");
                let text = std::fs::read_to_string(&path).expect("read it back");
                let table = text
                    .find("[options]")
                    .unwrap_or_else(|| panic!("the table was created: {text:?}"));
                let key = text
                    .find("prefix = \"C-a\"")
                    .unwrap_or_else(|| panic!("the canonical value was written: {text:?}"));
                assert!(key > table, "and it is inside the table: {text:?}");
                assert!(
                    text.contains("# mine"),
                    "the user's comment survives: {text:?}"
                );
                // The same reader a client uses agrees, so the file is not merely well-shaped.
                assert_eq!(keymap().expect("usable").prefix().to_string(), "C-a");
            },
        );
    }
    /// A pane with no command runs the user's `default-command`, and the SHELL when they set none.
    ///
    /// The label is what the assertion reads, because it is what a user sees in a sidebar and what
    /// `sprag panes` prints — a pane running `htop` that introspects as `bash` is one they cannot find.
    #[test]
    fn a_pane_with_no_command_runs_the_users_default_command() {
        with_config(
            Some("[options]\ndefault-command = \"exec /usr/bin/htop -d 5\"\n"),
            || {
                let (_command, label) = default_pane_command();
                assert_eq!(label, "htop", "labelled by what it RUNS, not by the shell");
            },
        );
        // Silent user: the shared `$SHELL` fallback, unchanged.
        with_config(Some(""), || {
            let (_command, label) = default_pane_command();
            let (_shell, shell_label) = sprag_terminal::default_shell_command();
            assert_eq!(label, shell_label);
        });
    }

    /// A config that cannot be USED does not cost the user their pane.
    ///
    /// The daemon has no screen and the file's problem is already reported to any client that opens a
    /// palette, so refusing to birth a pane over a typo elsewhere in the file would take away the one
    /// surface that could explain it. The same rule a malformed env count follows.
    #[test]
    fn a_broken_config_still_births_a_pane() {
        with_config(
            Some("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n"),
            || {
                assert!(options().is_err(), "the file really is unusable");
                let (_command, label) = default_pane_command();
                let (_shell, shell_label) = sprag_terminal::default_shell_command();
                assert_eq!(label, shell_label, "and the pane gets a shell");
            },
        );
    }

    // ---- H3 slice 4: the `[[agent]]` manifests ------------------------------------------------

    /// The manifests a file declares, or why it could not be used — the reader every test below
    /// goes through, so none of them can pass against a path the daemon does not take.
    fn manifests_from(text: &str) -> Result<Vec<sprag_detect::Manifest>, ConfigError> {
        declared_in(Some(text))
    }

    fn why(text: &str) -> String {
        manifests_from(text)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| panic!("expected a refusal, got a usable list"))
    }

    fn named<'a>(
        manifests: &'a [sprag_detect::Manifest],
        name: &str,
    ) -> &'a sprag_detect::Manifest {
        manifests
            .iter()
            .find(|manifest| manifest.name == name)
            .unwrap_or_else(|| panic!("no manifest named {name}"))
    }

    fn rule_ids(manifest: &sprag_detect::Manifest) -> Vec<&str> {
        manifest.rules.iter().map(|rule| rule.id.as_str()).collect()
    }

    fn painted(lines: &[&str]) -> sprag_vt::Emulator {
        let mut em = sprag_vt::Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    /// THE SLICE'S OWN GATE, in the words the design uses: *a user manifest correcting a built-in
    /// rule*.
    ///
    /// It asserts both halves of "correcting", because only one of them is about layering. The rule
    /// named is REPLACED — the file's pattern decides, not the built-in's — and every other rule of
    /// that agent is still there, which is what "without redeclaring the rest" means and what a
    /// wholesale replacement would quietly lose.
    #[test]
    fn a_user_manifest_corrects_one_built_in_rule_and_keeps_the_rest() {
        let manifests = manifests_from(
            r#"
            [[agent]]
            name = "claude"

            [[agent.rule]]
            id = "idle-glyph"
            state = "idle"
            priority = 10
            all = [ { region = "title", starts_with = "@" } ]
            "#,
        )
        .expect("a corrected rule is usable");

        let claude = named(&manifests, "claude");
        assert_eq!(
            rule_ids(claude),
            ["dialog-choice-list", "spinner-glyph", "idle-glyph"],
            "the built-in's other rules survive, and the corrected one keeps its position",
        );

        // The correction is the file's, not the built-in's: `✳` no longer reads as idle and `@`
        // does. Asserted through `detect` rather than by reading the rule back, because what the
        // user changed is what the pane SAYS.
        let em = painted(&["❯", "  ⏸ manual mode on · ? for shortcuts"]);
        assert_eq!(
            sprag_detect::detect(em.screen(), Some("✳ Claude Code"), &manifests).state,
            sprag_detect::AgentState::Unknown,
            "the built-in's own pattern is gone",
        );
        assert_eq!(
            sprag_detect::detect(em.screen(), Some("@ Claude Code"), &manifests).state,
            sprag_detect::AgentState::Idle,
            "and the file's pattern is what decides",
        );
    }

    /// The format has to be able to say what the built-ins say, and `codex` is the one that proves
    /// it: its single fingerprint is a composer line in the bottom 3 rows AND a footer shape in the
    /// bottom 1 — two matches on the SAME region with different windows, which is exactly what a
    /// region-keyed table could not have held.
    ///
    /// Asserted behaviourally rather than structurally. A field-by-field comparison would prove the
    /// declaration and the built-in have the same SHAPE; running both against the same screen proves
    /// they mean the same thing, which is the property a user redeclaring an agent needs.
    #[test]
    fn the_format_expresses_the_conjunction_the_built_in_codex_needs() {
        let declared = manifests_from(
            r#"
            [[agent]]
            name = "mycodex"

            [[agent.fingerprint]]
            all = [
              { region = "bottom:3", regex = '(?m)^›\s' },
              { region = "bottom:1", regex = '(?m)^\s*\S+\s+\S+\s+·\s+/' },
            ]

            [[agent.rule]]
            id = "no-working-signal"
            state = "idle"
            priority = 10
            all = [ { region = "title", regex = '\S' } ]
            "#,
        )
        .expect("the conjunction is expressible");

        let em = painted(&["› write me a test", "gpt-5.6 high · /home/coin/sprag"]);
        let mine = sprag_detect::detect(em.screen(), Some("sprag"), &declared);
        assert_eq!(mine.agent.as_deref(), Some("mycodex"));
        assert_eq!(mine.state, sprag_detect::AgentState::Idle);
        assert_eq!(
            mine.state,
            sprag_detect::detect(em.screen(), Some("sprag"), &sprag_detect::built_ins()).state,
            "the declared agent reads the screen exactly as the built-in does",
        );

        // The conjunction is doing the work: drop the footer row and NEITHER claims the pane.
        let composer_only = painted(&["› write me a test"]);
        assert_eq!(
            sprag_detect::detect(composer_only.screen(), Some("sprag"), &declared).agent,
            None,
            "one half of the conjunction is not the fingerprint",
        );
    }

    /// A new agent goes in FRONT of the built-ins, because `detect` stops at the first manifest that
    /// claims the pane — so the front is the only position at which a user's file can win.
    #[test]
    fn a_new_agent_is_offered_before_the_built_ins() {
        let manifests = manifests_from(
            r#"
            [[agent]]
            name = "wrapper"

            [[agent.fingerprint]]
            all = [ { region = "bottom:4", contains = "? for shortcuts" } ]

            [[agent.rule]]
            id = "wrapper-idle"
            state = "idle"
            all = [ { region = "title", starts_with = "✳" } ]
            "#,
        )
        .expect("a new agent is usable");

        assert_eq!(
            manifests
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["wrapper", "claude", "codex"],
        );

        // The fingerprint deliberately collides with `claude`'s footer one: the user's file wins.
        let em = painted(&["❯", "  ⏸ manual mode on · ? for shortcuts"]);
        assert_eq!(
            sprag_detect::detect(em.screen(), Some("✳ Claude Code"), &manifests)
                .agent
                .as_deref(),
            Some("wrapper"),
        );
    }

    /// Several new agents keep FILE order at the front rather than each one displacing the last.
    #[test]
    fn several_new_agents_keep_the_order_the_file_wrote_them_in() {
        let manifests = manifests_from(
            r#"
            [[agent]]
            name = "first"
            [[agent.fingerprint]]
            all = [ { region = "title", starts_with = "1" } ]

            [[agent]]
            name = "second"
            [[agent.fingerprint]]
            all = [ { region = "title", starts_with = "2" } ]
            "#,
        )
        .expect("two new agents are usable");
        assert_eq!(
            manifests
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "claude", "codex"],
        );
    }

    /// `disable` is `[[unbind]]`'s counterpart one grain down: it removes a built-in rule outright,
    /// for the correction that cannot be written as a better pattern.
    #[test]
    fn a_disabled_rule_is_gone_and_the_agent_still_reads_its_other_states() {
        let manifests = manifests_from(
            r#"
            [[agent]]
            name = "claude"
            disable = ["idle-glyph"]
            "#,
        )
        .expect("a disabled rule is usable");

        let claude = named(&manifests, "claude");
        assert_eq!(rule_ids(claude), ["dialog-choice-list", "spinner-glyph"]);

        let em = painted(&["❯", "  ⏸ manual mode on · ? for shortcuts"]);
        let verdict = sprag_detect::detect(em.screen(), Some("✳ Claude Code"), &manifests);
        assert_eq!(
            verdict.state,
            sprag_detect::AgentState::Unknown,
            "the rule that read this screen is gone",
        );
        assert_eq!(
            verdict.agent.as_deref(),
            Some("claude"),
            "and the pane is still known to be claude's — 'I know what this is and not what it is \
             doing' is the fact `disable` leaves standing",
        );
    }

    /// A rule id that is both declared and disabled is REFUSED rather than resolved by whichever
    /// happens to run first — `build`'s treatment of a key that is both bound and unbound, one
    /// table over. A file that says two things about one rule has not said what it wants.
    #[test]
    fn a_rule_both_declared_and_disabled_is_refused() {
        let why = why(r#"
            [[agent]]
            name = "claude"
            disable = ["idle-glyph"]

            [[agent.rule]]
            id = "idle-glyph"
            state = "idle"
            all = [ { region = "title", starts_with = "@" } ]
            "#);
        assert!(why.contains("idle-glyph"), "{why}");
        assert!(why.contains("declared and disabled"), "{why}");
    }

    /// A `disable` naming a rule that is not there is a TYPO, and a typo that silently did nothing
    /// would leave the author believing their config was accepted — `deny_unknown_fields`'s own
    /// argument, applied to a value rather than to a field name.
    #[test]
    fn disabling_a_rule_that_does_not_exist_is_refused_and_says_which_exist() {
        let why = why(r#"
            [[agent]]
            name = "claude"
            disable = ["idle-glyf"]
            "#);
        assert!(why.contains("idle-glyf"), "{why}");
        assert!(
            why.contains("idle-glyph"),
            "the message lists the ids that do exist: {why}",
        );
    }

    /// An empty conjunction HOLDS, so an empty fingerprint would claim every pane in the workspace
    /// and an empty rule would fire on every screen. `Fingerprint`'s docs call that the author's
    /// error rather than something the TYPE can prevent; the file edge can prevent it, and does.
    #[test]
    fn an_empty_conjunction_is_refused_rather_than_holding_for_every_pane() {
        let fingerprint = why(r#"
            [[agent]]
            name = "greedy"
            [[agent.fingerprint]]
            all = []
            "#);
        assert!(fingerprint.contains("every pane"), "{fingerprint}");

        let rule = why(r#"
            [[agent]]
            name = "claude"
            [[agent.rule]]
            id = "always"
            state = "working"
            all = []
            "#);
        assert!(rule.contains("every pane"), "{rule}");
    }

    /// A new agent with no fingerprint could never claim a pane, so the file has declared something
    /// that cannot do anything. Refused, rather than accepted and inert.
    #[test]
    fn a_new_agent_with_no_fingerprint_is_refused() {
        let why = why(r#"
            [[agent]]
            name = "ghost"
            [[agent.rule]]
            id = "ghost-idle"
            state = "idle"
            all = [ { region = "title", starts_with = "x" } ]
            "#);
        assert!(why.contains("fingerprint"), "{why}");
    }

    /// An entry that only CORRECTS a built-in needs no fingerprint of its own — the check above is
    /// about a new agent, and this is the other side of it.
    #[test]
    fn correcting_a_built_in_needs_no_fingerprint() {
        let manifests = manifests_from(
            r#"
            [[agent]]
            name = "codex"
            [[agent.rule]]
            id = "spinner-glyph"
            state = "working"
            priority = 20
            all = [ { region = "title", starts_with = "*" } ]
            "#,
        )
        .expect("correcting a built-in is usable");
        // Against the BUILT-IN's own count rather than a literal. The claim is that a correction
        // leaves the fingerprint list alone, and a hard-coded number states an incidental fact
        // instead — it went red the round `codex` gained an onboarding fingerprint, which is a
        // change this test has no opinion about.
        assert_eq!(
            named(&manifests, "codex").any.len(),
            sprag_detect::codex().any.len(),
            "a correction leaves the fingerprints exactly as the built-in declares them",
        );
    }

    /// Each of the four ways to write a match wrongly, refused with the file's own words.
    #[test]
    fn a_match_names_exactly_one_region_and_exactly_one_test() {
        let agent =
            |m: &str| format!("[[agent]]\nname = \"x\"\n[[agent.fingerprint]]\nall = [ {m} ]\n");

        let none = why(&agent(r#"{ region = "title" }"#));
        assert!(none.contains("names no test"), "{none}");

        let both = why(&agent(
            r#"{ region = "title", contains = "a", starts_with = "b" }"#,
        ));
        assert!(both.contains("more than one test"), "{both}");

        let region = why(&agent(r#"{ region = "middle", contains = "a" }"#));
        assert!(region.contains("bottom:N"), "{region}");

        let rows = why(&agent(r#"{ region = "bottom:0", contains = "a" }"#));
        assert!(rows.contains("no rows"), "{rows}");
    }

    /// A pattern that cannot compile is refused HERE, with the file named — not left to never match
    /// on every pane for the life of the daemon. The same choice `Test`'s docs describe for the
    /// compiled variant, and the same one the keymap makes for a key spec.
    #[test]
    fn a_pattern_that_does_not_compile_is_refused_with_the_pattern_quoted() {
        let why = why(r#"
            [[agent]]
            name = "x"
            [[agent.fingerprint]]
            all = [ { region = "title", regex = "([unclosed" } ]
            "#);
        assert!(why.contains("does not compile"), "{why}");
        assert!(why.contains("([unclosed"), "{why}");
    }

    /// The states the file accepts are the states the WIRE carries, read out of the vocabulary
    /// itself — so `unknown` is unwritable without a second rule saying so, because it has no wire
    /// token at all.
    #[test]
    fn a_rule_may_conclude_only_a_state_the_wire_can_carry() {
        let why = why(r#"
            [[agent]]
            name = "claude"
            [[agent.rule]]
            id = "nothing"
            state = "unknown"
            all = [ { region = "title", contains = "x" } ]
            "#);
        assert!(why.contains("working"), "{why}");
        assert!(why.contains("blocked"), "{why}");
        assert!(why.contains("idle"), "{why}");
    }

    /// A typo'd table is refused rather than silently doing nothing — the reason the file has
    /// `deny_unknown_fields` at all, asserted for the new tables rather than assumed of them.
    #[test]
    fn a_typo_in_an_agent_table_is_refused() {
        let why = why(r#"
            [[agent]]
            name = "claude"
            [[agent.rules]]
            id = "x"
            state = "idle"
            all = [ { region = "title", contains = "x" } ]
            "#);
        assert!(why.contains("rules"), "{why}");
    }

    /// A file with no `[[agent]]` at all is the built-ins, unchanged — the same three-way silence
    /// `keymap` treats as "the user has not said otherwise".
    #[test]
    fn a_file_that_declares_no_agent_is_the_built_ins() {
        let manifests = manifests_from("[options]\nprefix = \"C-a\"\n").expect("usable");
        assert_eq!(
            manifests
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            sprag_detect::built_ins()
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
        );
    }

    // ---- H3 slice 4: the holder ---------------------------------------------------------------

    /// A file the caller names, so the holder's behaviour is exercised without mutating the
    /// process-global environment its sibling tests are reading — `ClientConfig::at`'s reason.
    fn manifest_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sprag-manifests-{}-{name}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// The holder's whole contract: an edit REPLACES the ruleset, and anything that is not an edit
    /// does not. Both directions matter and for different reasons — a replacement that did not
    /// happen leaves every quiet pane holding a verdict the user has edited away, and one that
    /// happens anyway costs the workspace an evaluation per pane on every sweep forever.
    #[test]
    fn an_edit_replaces_the_ruleset_and_an_unchanged_file_does_not() {
        let path = manifest_file("edit");
        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"idle-glyph\"]\n",
        )
        .expect("write");

        let mut held = AgentManifests::at(Some(&path));
        let first = held.rules().revision();
        assert_eq!(rule_ids(named(held.rules().manifests(), "claude")).len(), 2);

        assert!(!held.refresh(), "an unchanged file is not an edit");
        assert_eq!(held.rules().revision(), first, "and nothing is replaced");

        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"spinner-glyph\"]\n",
        )
        .expect("rewrite");
        assert!(held.refresh(), "an edit is an edit");
        assert_ne!(
            held.rules().revision(),
            first,
            "a replaced ruleset is a new one, which is what makes every pane owe an evaluation",
        );
        assert_eq!(
            rule_ids(named(held.rules().manifests(), "claude")),
            ["dialog-choice-list", "idle-glyph"],
        );
        let _ = std::fs::remove_file(&path);
    }

    /// An edit that BREAKS the file keeps the manifests that were working and says why.
    ///
    /// `ClientConfig::refresh`'s rule, and the daemon needs it more: there is no screen to print on,
    /// and swapping in the built-ins would silently take a user's own agents away because they
    /// typo'd a line in an editor.
    #[test]
    fn a_broken_edit_keeps_the_manifests_that_worked() {
        let path = manifest_file("broken");
        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"idle-glyph\"]\n",
        )
        .expect("write");
        let mut held = AgentManifests::at(Some(&path));
        let working = held.rules().revision();
        assert!(held.unusable().is_none());

        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"nope\"]\n",
        )
        .expect("rewrite");
        assert!(!held.refresh(), "a broken edit replaces nothing");
        assert_eq!(held.rules().revision(), working, "the working list is kept");
        assert_eq!(
            rule_ids(named(held.rules().manifests(), "claude")),
            ["dialog-choice-list", "spinner-glyph"],
        );
        let reported = held.unusable().expect("the reason is kept for a surface");
        assert!(reported.to_string().contains("nope"), "{reported}");

        // And the fix is noticed, so one bad save is not permanent.
        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"spinner-glyph\"]\n",
        )
        .expect("fix");
        assert!(held.refresh(), "the fix is an edit too");
        assert!(held.unusable().is_none(), "and the reason is cleared");
        let _ = std::fs::remove_file(&path);
    }

    /// A file that never existed is the built-ins, and DELETING one is a user saying they have no
    /// manifests of their own — the same answer, reached from the other side.
    #[test]
    fn a_deleted_file_falls_back_to_the_built_ins() {
        let path = manifest_file("deleted");
        std::fs::write(
            &path,
            "[[agent]]\nname = \"claude\"\ndisable = [\"idle-glyph\"]\n",
        )
        .expect("write");
        let mut held = AgentManifests::at(Some(&path));
        assert_eq!(rule_ids(named(held.rules().manifests(), "claude")).len(), 2);

        std::fs::remove_file(&path).expect("delete");
        assert!(held.refresh(), "the deletion is an edit");
        assert_eq!(
            rule_ids(named(held.rules().manifests(), "claude")).len(),
            3,
            "the built-ins are back",
        );
    }

    /// With no config directory at all there is nothing to watch, and the built-ins are final.
    #[test]
    fn no_config_path_is_the_built_ins_and_no_reads() {
        let mut held = AgentManifests::at(None);
        assert_eq!(held.rules().len(), sprag_detect::built_ins().len());
        assert!(!held.refresh());
    }
}
