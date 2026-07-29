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

//! ## Two settings, two readers, ONE file shape
//!
//! [`load`] answers the commands question and [`keymap`] answers the keys one, because they have
//! different consumers: a declared command is PASTED INTO A PANE, which is a daemon operation, so
//! [`UserConfig`] crosses the wire to the palette — while a keybinding is what one client does with
//! one keyboard, which the daemon has no reason to hold and two clients may legitimately disagree
//! about. Putting the keymap in the wire DTO would send it somewhere it is not wanted.
//!
//! What they share is ONE private description of the file's shape. That sharing is not an
//! optimisation: it is what keeps `deny_unknown_fields` honest. A `[keys]` table the commands
//! reader had never heard of would make the whole file invalid for a user who only wanted to
//! rebind a key.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::keymap::{BoundAction, KeyError, KeySpec, Keymap};
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
    /// The file could not be WRITTEN — produced only by [`bind_key`] and [`unbind_key`].
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
    let Some(path) = config_path() else {
        return Ok(Keymap::default());
    };
    if !path.is_file() {
        return Ok(Keymap::default());
    }
    build_keymap(&read_file(&path)?)
}

/// The user's [`Keymap`], holding on to [`CONFIG_FILE`]'s text so it can notice the file CHANGED.
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
pub struct KeymapFile {
    /// Where the file would be, or `None` when there is no config directory to hold one — in which
    /// case there is nothing to re-read and the defaults are final.
    path: Option<PathBuf>,
    /// The exact text `keymap` was built from; `None` when there was no file at all.
    text: Option<String>,
    /// The last table read SUCCESSFULLY. Retained across a failed re-read (see
    /// [`refresh`](Self::refresh)).
    keymap: Keymap,
}

impl KeymapFile {
    /// Read the user's keymap now, remembering the file it came from.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] on exactly the conditions [`keymap`] reports: the file exists and cannot be
    /// read, is not valid TOML, or declares something unusable. A client fails to START on those,
    /// because the one screen able to show the message is the one it has not yet replaced.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path();
        let text = match path.as_deref() {
            Some(path) if path.is_file() => {
                Some(std::fs::read_to_string(path).map_err(|error| {
                    ConfigError::Content(ProjectError::Unreadable(error.to_string()))
                })?)
            }
            _ => None,
        };
        Ok(Self {
            keymap: match &text {
                Some(text) => build_keymap(&parse_file(text)?)?,
                None => Keymap::default(),
            },
            path,
            text,
        })
    }

    /// The table as it was last read.
    #[must_use]
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// Re-read the file if its content has changed, and say whether the TABLE moved.
    ///
    /// `Ok(false)` is the steady state. `Ok(true)` means the file changed AND the new table differs
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
        let next = match &self.text {
            Some(text) => build_keymap(&parse_file(text)?)?,
            None => Keymap::default(),
        };
        Ok(std::mem::replace(&mut self.keymap, next) != self.keymap)
    }
}

/// Layer a file's declarations over the default keymap.
fn build_keymap(file: &UserConfigFile) -> Result<Keymap, ConfigError> {
    let invalid = |error: KeyError| ConfigError::Content(ProjectError::Invalid(error.to_string()));
    let mut keymap = Keymap::default();
    if let Some(prefix) = file.keys.as_ref().and_then(|keys| keys.prefix.as_deref()) {
        keymap.set_prefix(prefix).map_err(invalid)?;
    }
    for bind in &file.bind {
        keymap.bind(&bind.key, &bind.action).map_err(invalid)?;
    }
    for unbind in &file.unbind {
        // Refused rather than resolved by precedence. Applying binds before unbinds is one
        // defensible order and applying them in file order is another, so a file that says both
        // about one key has not said what it wants — and a user who has to remember which array
        // wins has been given a puzzle instead of a keymap.
        let key = KeySpec::parse(&unbind.key).map_err(invalid)?;
        if file
            .bind
            .iter()
            .any(|bind| KeySpec::parse(&bind.key).is_ok_and(|bound| bound == key))
        {
            return Err(invalid(KeyError::BoundAndUnbound(key.to_string())));
        }
        keymap.unbind(&unbind.key).map_err(invalid)?;
    }
    Ok(keymap)
}

/// The `[[bind]]` array's name in the file, and the field names of one entry.
///
/// Spelled here as well as on [`DeclaredBind`] because a writer cannot ask a `serde` derive what it
/// called a field. Nothing HOLDS the two together — except that [`edit_keys`] reads its own output
/// back through the reader before writing it, so a drift makes the very first edit fail with
/// `deny_unknown_fields` rather than silently producing a file nothing honours.
const BIND_ARRAY: &str = "bind";
/// The `[[unbind]]` array's name — see [`BIND_ARRAY`].
const UNBIND_ARRAY: &str = "unbind";
/// The `key` field of a `[[bind]]` / `[[unbind]]` entry — see [`BIND_ARRAY`].
const KEY_FIELD: &str = "key";
/// The `action` field of a `[[bind]]` entry — see [`BIND_ARRAY`].
const ACTION_FIELD: &str = "action";

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
pub fn bind_key(key: &KeySpec, action: BoundAction) -> Result<PathBuf, ConfigError> {
    edit_keys(|doc| {
        // The contradiction slice 1 REFUSES (`BoundAndUnbound`) is what an unbind left in place
        // would make: this key is being given a meaning, so a declaration that it has none is not
        // a second opinion to keep, it is the same statement retracted.
        remove_named(doc, UNBIND_ARRAY, key)?;
        let tables = tables_mut(doc, BIND_ARRAY)?;
        // Bound to a `let` rather than used as the `if let` scrutinee: the iterator is a boxed
        // trait object, so as a temporary it would live — with its immutable borrow — past the
        // point the `else` arm needs the array mutably.
        let existing = tables.iter().position(|table| names(table, key));
        if let Some(index) = existing {
            // Retargeted IN PLACE rather than removed and appended, the same rule
            // [`Keymap::bind`] follows: a rebound key keeps the position the user gave it, in
            // their file as well as in `list-keys`.
            if let Some(table) = tables.get_mut(index) {
                table[ACTION_FIELD] = value(action.to_string());
            }
        } else {
            let mut table = Table::new();
            table[KEY_FIELD] = value(key.to_string());
            table[ACTION_FIELD] = value(action.to_string());
            tables.push(table);
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
pub fn unbind_key(key: &KeySpec) -> Result<PathBuf, ConfigError> {
    edit_keys(|doc| {
        remove_named(doc, BIND_ARRAY, key)?;
        // A key the defaults never bound now means nothing already: removing the user's own
        // binding was the whole edit, and an `[[unbind]]` would be a line about a key no table
        // mentions.
        if Keymap::default().action(key.name(), key.mods()).is_none() {
            return Ok(());
        }
        let tables = tables_mut(doc, UNBIND_ARRAY)?;
        if tables.iter().all(|table| !names(table, key)) {
            let mut table = Table::new();
            table[KEY_FIELD] = value(key.to_string());
            tables.push(table);
        }
        Ok(())
    })
}

/// Whether a `[[bind]]` / `[[unbind]]` entry names `key`.
///
/// Compared PARSED rather than as text: `C-o` and `^o` are one keystroke, so an edit that matched
/// only the spelling it was handed would leave behind an entry the READER treats as the same key —
/// which for a bind is a stale action and for an unbind is the contradiction slice 1 refuses.
fn names(entry: &Table, key: &KeySpec) -> bool {
    entry
        .get(KEY_FIELD)
        .and_then(Item::as_str)
        .and_then(|spec| KeySpec::parse(spec).ok())
        .is_some_and(|spec| spec == *key)
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
    key: &KeySpec,
) -> Result<bool, ConfigError> {
    if doc.get(name).is_none() {
        return Ok(false);
    }
    let tables = tables_mut(doc, name)?;
    let before = tables.len();
    tables.retain(|table| !names(table, key));
    Ok(tables.len() != before)
}

/// Apply `edit` to the user's [`CONFIG_FILE`] and write it back, or change nothing at all.
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
fn edit_keys(
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
    build_keymap(&parse_file(&text)?)?;
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|error| ConfigError::Content(ProjectError::Malformed(error.to_string())))?;
    edit(&mut doc)?;
    let edited = doc.to_string();
    build_keymap(&parse_file(&edited)?)?;
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
    /// The `[keys]` table — client-wide key settings that are not a binding.
    #[serde(default)]
    keys: Option<DeclaredKeys>,
    /// `[[bind]]` entries, layered over the defaults in file order.
    #[serde(default)]
    bind: Vec<DeclaredBind>,
    /// `[[unbind]]` entries, removing a default.
    #[serde(default)]
    unbind: Vec<DeclaredUnbind>,
}

/// The `[keys]` table.
///
/// A table rather than a bare `prefix = "C-b"` at the file's top level, because the top level is
/// where the file's SECTIONS live: a setting with no table would be the one thing a reader could not
/// tell apart from a typo'd table name.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredKeys {
    /// The key that says "the next keystroke is the client's" — tmux's `prefix`. Absent means the
    /// default, `C-b`.
    prefix: Option<String>,
}

/// One `[[bind]]` entry — tmux's `bind-key key command`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredBind {
    /// The key spec, e.g. `%` or `C-o`.
    key: String,
    /// The action, spelled as the shell spells it, e.g. `split-window -h`.
    action: String,
}

/// One `[[unbind]]` entry — tmux's `unbind-key key`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredUnbind {
    /// The key spec to remove.
    key: String,
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
        let text = "[keys]\nprefix = \"C-a\"\n\n[[bind]]\nkey = \"c\"\naction = \"split-window -h\"\n\n\
                    [[unbind]]\nkey = \"o\"\n\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n";
        with_config(Some(text), || {
            let config = load().expect("the file exists").expect("and is valid");
            assert_eq!(config.commands.len(), 1, "the commands still read");
            let keymap = keymap().expect("and so do the keys");
            assert_eq!(keymap.prefix().to_string(), "C-a");
            assert!(
                keymap.action("c", Modifiers::default()).is_some(),
                "the declared bind is there",
            );
            assert_eq!(
                keymap.action("o", Modifiers::default()),
                None,
                "the unbound default is gone",
            );
            assert_eq!(
                keymap.action("d", Modifiers::default()),
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
                keymap().expect("valid").action("x", Modifiers::default()),
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
    /// the comment, the blank lines and the `[keys]` table's inline note all disappear.
    #[test]
    fn a_bound_key_lands_in_the_file_and_the_rest_of_it_survives() {
        let text = "# keep me\n[keys]\nprefix = \"C-a\"  # and me\n\n\
                    [[command]]\nname = \"top\"\nrun = [\"htop\"]\n";
        with_config(Some(text), || {
            bind_key(&key("c"), action("split-window -h")).expect("binds");
            let after = written();
            assert!(after.contains("# keep me"), "{after:?}");
            assert!(after.contains("# and me"), "{after:?}");
            assert!(after.contains("[[command]]"), "{after:?}");
            // ...and the binding is really there, read back through the ordinary reader.
            let keymap = keymap().expect("the written file is valid");
            assert_eq!(
                keymap.action("c", Modifiers::default()),
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
    /// `edit_keys`'s read-back — which is that guard doing its job.
    #[test]
    fn binding_a_key_the_file_unbound_takes_the_unbind_out() {
        with_config(Some("[[unbind]]\nkey = \"o\"\n"), || {
            bind_key(&key("o"), action("detach-client")).expect("binds");
            assert!(!written().contains("[[unbind]]"), "{:?}", written());
            assert_eq!(
                keymap().expect("valid").action("o", Modifiers::default()),
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
                unbind_key(&key("c")).expect("unbinds");
                let after = written();
                assert!(!after.contains("[[bind]]"), "the binding went: {after:?}");
                assert!(
                    !after.contains("[[unbind]]"),
                    "`c` is not a default, so there is nothing to suppress: {after:?}"
                );
                // A DEFAULT does get recorded, because the layering would restore it otherwise.
                unbind_key(&key("o")).expect("unbinds");
                assert!(written().contains("[[unbind]]"), "{:?}", written());
                assert_eq!(
                    keymap().expect("valid").action("o", Modifiers::default()),
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
                bind_key(&key("C-o"), action("send-prefix")).expect("binds");
                let after = written();
                assert_eq!(after.matches("action =").count(), 1, "one entry: {after:?}");
                assert!(after.contains("send-prefix"), "{after:?}");
                // ...and an unbind reaches it through the other spelling too.
                unbind_key(&key("^o")).expect("unbinds");
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
            bind_key(&key("a"), action("select-pane -t :.+")).expect("binds");
            let after = written();
            let a = after.find("key = \"a\"").expect("a is there");
            let b = after.find("key = \"b\"").expect("b is there");
            assert!(a < b, "the user's order survived: {after:?}");
            assert_eq!(after.matches("[[bind]]").count(), 2);
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
    /// REVERT-PROOF: drop the pre-edit `build_keymap` and the second half fails. The first half
    /// does not, which is the point.
    #[test]
    fn an_edit_refuses_a_config_it_cannot_read_and_changes_nothing() {
        let text = "[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n";
        for edited in [key("c"), key("x")] {
            with_config(Some(text), || {
                let message = bind_key(&edited, action("detach-client"))
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
            bind_key(&key("C-x"), action("detach-client")).expect("binds");
            assert_eq!(
                keymap().expect("valid").action(
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
            unbind_key(&key("o")).expect("unbinds");
            let once = written();
            unbind_key(&key("o")).expect("unbinds again");
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
            let mut live = KeymapFile::load().expect("loads");
            assert_eq!(live.keymap().action("c", Modifiers::default()), None);

            bind_key(&key("c"), action("detach-client")).expect("binds");
            assert!(live.refresh().expect("re-reads"), "the table moved");
            assert_eq!(
                live.keymap().action("c", Modifiers::default()),
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
            let mut live = KeymapFile::load().expect("loads");
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            assert!(live.keymap().is_prefix("b", ctrl), "the default to start");
            std::fs::write(config_path().expect("a path"), "[keys]\nprefix = \"C-a\"\n")
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
                let mut live = KeymapFile::load().expect("loads");
                std::fs::write(config_path().expect("a path"), "[[bind]]\nkey = [\n")
                    .expect("a half-typed save");
                assert!(live.refresh().is_err(), "the save is reported");
                assert_eq!(
                    live.keymap().action("c", Modifiers::default()),
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
            let mut live = KeymapFile::load().expect("loads");
            assert_eq!(live.keymap().action("d", Modifiers::default()), None);
            std::fs::remove_file(config_path().expect("a path")).expect("the user deletes it");
            assert!(live.refresh().expect("re-reads"));
            assert_eq!(
                live.keymap().action("d", Modifiers::default()),
                Some(crate::keymap::BoundAction::DetachClient),
            );
        });
    }

    /// A broken KEY or ACTION is refused whole, and the report names `config.toml` — the same
    /// contract the commands half already has, through the same wrapper.
    #[test]
    fn a_broken_key_or_action_is_refused_and_the_report_names_this_file() {
        for (text, expected) in [
            ("[keys]\nprefix = \"C-\"\n", "is not a key"),
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
}
