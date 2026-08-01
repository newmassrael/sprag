//! Installing sprag's agent hook into an agent's OWN configuration.
//!
//! [`crate::host::PANE_ENV_VAR`] told a pane's child which pane it is in, and
//! [`crate::agent::AgentRegistry::report`] gave that child somewhere to say what it is doing.
//! Nothing calls either. This module writes the call into the agent's configuration, so a `claude`
//! started in a sprag pane reports its own state instead of being guessed at from its screen.
//!
//! ## What gets written, and why it is not a script
//!
//! The rival's installer writes a shell script and points the agent's config at it, because its
//! report carries a session id, a transcript path and a token count, and its hook parses the
//! payload with `python3` to assemble them. sprag's report is one verb with no metadata, so a
//! script buys nothing and costs three things: a mode bit and a second file to version, a Windows
//! twin, and an interpreter dependency whose absence makes an install SUCCEED and then report
//! nothing forever.
//!
//! So the agent's config names this binary directly — `sprag hook claude` — at the absolute path
//! the install resolved, because the agent's environment need not have `sprag` on `PATH`.
//!
//! ## The payload still has to be read
//!
//! A hook that reported on the event alone would be wrong in a way that lasts. A report OUTRANKS
//! the screen ([`crate::agent::AgentRegistry::report`]), so where a bad scrape is corrected by the
//! next sweep, a bad report stands until something releases it. Two payloads must therefore be
//! ignored rather than mapped: one carrying an `agent_id` (a SUBAGENT's event, not the pane's), and
//! `SubagentStop`, which is a completion event that can arrive after the pane's own turn has
//! already stopped and would otherwise revive an idle pane. `SubagentStop` is ignored by being
//! ABSENT from [`Target::events`] — the table is the only place an event has a meaning, so an event
//! nobody listed cannot acquire one by accident.
//!
//! ## One table, two readers
//!
//! [`Target::events`] is both the list of events an install writes into the config AND the mapping
//! [`report_for`] applies to a payload. That is deliberate: the state a hook reports can then be
//! corrected by a new release of sprag WITHOUT rewriting the user's file, which is what a version
//! ladder in the installed footprint would otherwise be for. What the config holds is a list of
//! event names and ONE command string.
//!
//! ## What "preserving the file" means here
//!
//! Not merely "do not clobber the user's other keys" — a parse/edit/serialise round trip preserves
//! those and still destroys comment, order and layout. This module preserves:
//!
//! * **key order**, through `serde_json`'s `preserve_order` feature (enabled at the workspace root
//!   for this module; without it every object in the user's file comes back alphabetised);
//! * **indentation**, by reading the file's own first indent and re-emitting with it; and
//! * **the trailing newline**, or its absence.
//!
//! What it does not preserve is a file that was not pretty-printed to begin with (a compact or
//! hand-mixed layout is re-emitted at one indent). That is why nothing here writes without being
//! asked: the caller renders [`Plan::changes`], and [`Plan::apply`] keeps a `.sprag-backup` copy of
//! a file it did not create.
//!
//! ## Recognising our own entries
//!
//! An entry is sprag's when its command ends in `hook <target>` and the program before it is named
//! `sprag` — the SUBCOMMAND identifies us, not the path, so a binary that moved since the install
//! is still recognised (and re-installing updates the path in place rather than adding a second
//! entry). sprag's entry always sits in a group of its OWN — never joined to somebody else's, whose
//! `matcher` would silently narrow our hook to one tool — so an uninstall removes a whole group it
//! is sure of, and an event that still holds another hook keeps it untouched.
//!
//! The one thing this cannot preserve is an event array that was ALREADY EMPTY before the install:
//! after removing our group the file is in exactly the state it would be in had the event never
//! been there, so the prune removes it. That is chosen rather than overlooked — the alternative
//! leaves an empty array under every installed event forever — and what is lost is a key's
//! presence, not a setting.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sprag_detect::AgentState;

/// What a hook payload means for the pane the agent is running in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Report this state, naming the agent — the report outranks whatever the screen argues.
    Report(AgentState),
    /// Hand the pane back to screen inference. The agent is gone while its pane's child (a shell)
    /// lives, which is the one exit the sweep's own EOF rule cannot see.
    Release,
}

/// An agent whose configuration sprag knows how to instrument.
///
/// The fields are data, so a further agent is a further `const` rather than further code — the
/// property that decides whether this abstraction was the right one.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// The token a user types (`sprag install-hooks claude`) and the config command carries.
    pub name: &'static str,
    /// What it is called in a sentence.
    pub label: &'static str,
    /// The agent name a report publishes, which is what sets the pane's identity. It matches the
    /// detection manifest's name so a reported pane and a scraped one describe the same agent.
    pub agent: &'static str,
    /// The agent's own config directory, relative to `$HOME`.
    dir: &'static str,
    /// The file inside it that holds the hooks.
    file: &'static str,
    /// Every event installed, and what a payload for it means. See the module docs: an event
    /// absent here is an event with no meaning, which is how `SubagentStop` is refused.
    pub events: &'static [(&'static str, Outcome)],
}

/// Claude Code — `~/.claude/settings.json`.
pub const CLAUDE: Target = Target {
    name: "claude",
    label: "Claude Code",
    agent: "claude",
    dir: ".claude",
    file: "settings.json",
    events: &[
        // The turn starts, and every step inside it. `PreToolUse` and `PostToolUse` both mean
        // working: the pane is not at rest between a tool call and its result.
        ("UserPromptSubmit", Outcome::Report(AgentState::Working)),
        ("PreToolUse", Outcome::Report(AgentState::Working)),
        ("PostToolUse", Outcome::Report(AgentState::Working)),
        // The agent has asked the human something. This is the event a permission prompt raises,
        // which is why no attempt is made to recognise a "permission-shaped" tool call: the agent
        // already tells us, and guessing at `tool_input` would be a second, worse answer.
        ("Notification", Outcome::Report(AgentState::Blocked)),
        ("Stop", Outcome::Report(AgentState::Idle)),
        // Not a state: the agent has exited and has no further claim on the pane.
        ("SessionEnd", Outcome::Release),
    ],
};

/// Every target an install can name.
pub const TARGETS: &[Target] = &[CLAUDE];

/// The target `name` addresses, or `None` for a token that is not one.
#[must_use]
pub fn target(name: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|target| target.name == name)
}

impl Target {
    /// The file an install edits, or `None` when `$HOME` names nowhere to look.
    #[must_use]
    pub fn path(&self) -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .map(|home| home.join(self.dir).join(self.file))
    }

    /// The command an installed entry runs: this binary, at an absolute path, plus the subcommand
    /// that reads a payload for this target.
    #[must_use]
    pub fn command(&self, exe: &Path) -> String {
        format!("{} hook {}", exe.display(), self.name)
    }

    /// Whether `command` is one of ours — see the module docs on why this matches the SUBCOMMAND
    /// rather than the path.
    #[must_use]
    pub fn owns(&self, command: &str) -> bool {
        let Some(program) = command.strip_suffix(&format!(" hook {}", self.name)) else {
            return false;
        };
        Path::new(program.trim().trim_matches('"'))
            .file_name()
            .is_some_and(|program| program == "sprag")
    }
}

/// What a payload means, or `None` when this hook has nothing to say about it.
///
/// `None` covers three cases that are one case to the caller — it exits without reporting: a
/// subagent's event, an event no [`Target::events`] entry names, and a payload that is not an
/// object at all. The last is worth folding in rather than erroring: a hook is not a place to
/// diagnose an agent's output format.
#[must_use]
pub fn report_for(target: &Target, payload: &Value) -> Option<Outcome> {
    if payload
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return None;
    }
    let event = payload.get("hook_event_name")?.as_str()?;
    target
        .events
        .iter()
        .find(|(name, _)| *name == event)
        .map(|(_, outcome)| *outcome)
}

/// Where a target stands: whether the agent is on this machine at all, and how much of the
/// integration is in place.
///
/// Reported as counts rather than a verdict so the caller renders the sentence. A partial install
/// is a real state — a config edited by hand, or an install interrupted — and it is what a
/// re-install repairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The target described.
    pub target: &'static str,
    /// The file that was examined.
    pub path: PathBuf,
    /// Whether the agent's own config directory exists — the only evidence available here that the
    /// agent is installed at all.
    pub present: bool,
    /// How many of [`Target::events`] carry our command.
    pub installed: usize,
    /// How many there are.
    pub total: usize,
}

impl Status {
    /// Whether every event is wired.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.installed == self.total
    }
}

/// Read `target`'s configuration and report where the integration stands.
///
/// A file that does not parse is NOT an error here: `list-hooks` answering "I cannot tell" for a
/// broken file is more useful than a listing that refuses to print. It reports zero installed
/// events, and the install that follows is the thing that refuses, with the parse error.
///
/// # Errors
///
/// [`HookError::NoHome`] when `$HOME` names nowhere.
pub fn status(target: &'static Target) -> Result<Status, HookError> {
    Ok(status_at(target, target.path().ok_or(HookError::NoHome)?))
}

/// [`status`] against a named file.
///
/// The split is not for tests alone, though it is what lets them run without touching a
/// process-global `$HOME`: path RESOLUTION and the edit MECHANISM fail for different reasons and a
/// caller that already knows the path should not be able to trip the first.
fn status_at(target: &'static Target, path: PathBuf) -> Status {
    let present = path.parent().is_some_and(Path::is_dir);
    let installed = read(&path)
        .ok()
        .flatten()
        .and_then(|text| parse(&text).ok())
        .map_or(0, |root| {
            target
                .events
                .iter()
                .filter(|(event, _)| holds_ours(&root, target, event))
                .count()
        });
    Status {
        target: target.name,
        path,
        present,
        installed,
        total: target.events.len(),
    }
}

/// An edit to an agent's configuration that has been derived but not applied.
///
/// The separation is what makes "always ask" mean anything: [`changes`](Self::changes) is rendered
/// FROM the edit that will be written, not described beside it, so what the user is shown and what
/// reaches the disk cannot come apart.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The target this edits.
    pub target: &'static str,
    /// The file it edits.
    pub path: PathBuf,
    /// One line per change, in the order they were derived. Empty means there is nothing to do.
    pub changes: Vec<String>,
    text: String,
    original: Option<String>,
}

impl Plan {
    /// Whether the file is already in the state the caller asked for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Where [`apply`](Self::apply) keeps the file's previous contents.
    #[must_use]
    pub fn backup_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(".sprag-backup");
        PathBuf::from(name)
    }

    /// Write the edit, keeping a copy of what was there.
    ///
    /// Returns the backup's path when one was written — a file this module CREATED has no previous
    /// contents to keep. The backup exists because the atomic replace below protects against a torn
    /// write, and nothing protects against a correct write the user did not want; this is the one
    /// file in the operation sprag did not create.
    ///
    /// # Errors
    ///
    /// [`HookError::Unwritable`] when the backup or the file itself cannot be replaced.
    pub fn apply(&self) -> Result<Option<PathBuf>, HookError> {
        if self.is_empty() {
            return Ok(None);
        }
        let backup = match &self.original {
            Some(original) => {
                let backup = self.backup_path();
                crate::config::write_atomic(&backup, original).map_err(|error| {
                    HookError::Unwritable(format!("{}: {error}", backup.display()))
                })?;
                Some(backup)
            }
            None => None,
        };
        crate::config::write_atomic(&self.path, &self.text)
            .map_err(|error| HookError::Unwritable(format!("{}: {error}", self.path.display())))?;
        Ok(backup)
    }
}

/// Derive the edit that wires `target`'s hooks to `exe`.
///
/// Idempotent by construction: an event that already carries one of our entries is left alone, and
/// one that carries an entry of ours with a DIFFERENT command (a binary that moved since the
/// install) has that command corrected in place rather than joined by a second.
///
/// # Errors
///
/// [`HookError::NoHome`] when `$HOME` names nowhere, [`HookError::Unreadable`] when the file exists
/// and cannot be read, [`HookError::Malformed`] when it does not parse as a JSON object — a config
/// this cannot make sense of is one it has no business rewriting.
pub fn plan_install(target: &'static Target, exe: &Path) -> Result<Plan, HookError> {
    install_at(target, target.path().ok_or(HookError::NoHome)?, exe)
}

/// [`plan_install`] against a named file — see [`status_at`] on why the split exists.
fn install_at(target: &'static Target, path: PathBuf, exe: &Path) -> Result<Plan, HookError> {
    let original = read(&path)?;
    let mut root = parse(original.as_deref().unwrap_or_default())?;
    let command = target.command(exe);
    let mut changes = Vec::new();

    for (event, _) in target.events {
        let entries = ensure_event(&mut root, target, event);
        match entries.iter_mut().find(|entry| is_ours(entry, target)) {
            Some(entry) => {
                if entry["command"] != Value::String(command.clone()) {
                    entry["command"] = Value::String(command.clone());
                    changes.push(format!("~ hooks.{event}  ->  {command}"));
                }
            }
            None => {
                entries.push(command_entry(&command));
                changes.push(format!("+ hooks.{event}  ->  {command}"));
            }
        }
    }

    Ok(Plan {
        target: target.name,
        text: render(&root, original.as_deref()),
        path,
        changes,
        original,
    })
}

/// Derive the edit that removes `target`'s sprag hooks, leaving everything else alone.
///
/// # Errors
///
/// As [`plan_install`].
pub fn plan_uninstall(target: &'static Target) -> Result<Plan, HookError> {
    uninstall_at(target, target.path().ok_or(HookError::NoHome)?)
}

/// [`plan_uninstall`] against a named file — see [`status_at`] on why the split exists.
fn uninstall_at(target: &'static Target, path: PathBuf) -> Result<Plan, HookError> {
    let original = read(&path)?;
    let mut root = parse(original.as_deref().unwrap_or_default())?;
    let mut changes = Vec::new();

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
                continue;
            };
            let mut removed = false;
            for group in groups.iter_mut() {
                let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                    continue;
                };
                let before = entries.len();
                entries.retain(|entry| !is_ours(entry, target));
                removed |= entries.len() != before;
            }
            if !removed {
                continue;
            }
            changes.push(format!("- hooks.{event}"));
            // Prune what this pass emptied, and accept the one case it cannot tell apart. A group
            // emptied here was OURS (see `ensure_event`), so dropping it is exact. An event array
            // left empty afterwards is NOT decidable: a user who had `"Stop": []` before the
            // install and one who had no `Stop` at all leave the file in the same state here.
            // Pruning is the better of the two, because the alternative leaves an empty array under
            // every event forever, and an empty container carries no configuration — what is lost
            // is the key's presence, not a setting.
            groups.retain(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|entries| !entries.is_empty())
            });
            if groups.is_empty() {
                hooks.remove(&event);
            }
        }
        if hooks.is_empty() && !changes.is_empty() {
            root.remove("hooks");
        }
    }

    Ok(Plan {
        target: target.name,
        text: render(&root, original.as_deref()),
        path,
        changes,
        original,
    })
}

/// The `hooks` array of OUR group under one event, brought into being if it is not there.
///
/// sprag never adds its entry to somebody else's group, and the reason is not tidiness: a group may
/// carry a `matcher`, which filters every entry inside it. Joining a group matched on one tool
/// would silently narrow sprag's hook to that tool — a defect with no symptom except a pane that
/// stops reporting. Our own group carries no matcher, so it fires for everything, and it doubles as
/// the record of what an uninstall may remove whole.
fn ensure_event<'a>(
    root: &'a mut Map<String, Value>,
    target: &Target,
    event: &str,
) -> &'a mut Vec<Value> {
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let groups = hooks
        .as_object_mut()
        .expect("just made an object")
        .entry(event.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !groups.is_array() {
        *groups = Value::Array(Vec::new());
    }
    let groups = groups.as_array_mut().expect("just made an array");
    let ours = groups.iter().position(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|entries| entries.iter().any(|entry| is_ours(entry, target)))
    });
    let index = ours.unwrap_or_else(|| {
        groups.push(serde_json::json!({ "hooks": [] }));
        groups.len() - 1
    });
    groups[index]
        .get_mut("hooks")
        .and_then(Value::as_array_mut)
        .expect("ours holds an array")
}

/// Whether a single hook entry is sprag's.
fn is_ours(entry: &Value, target: &Target) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| target.owns(command))
}

/// Whether any entry under `event` is ours.
fn holds_ours(root: &Map<String, Value>, target: &Target, event: &str) -> bool {
    root.get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|entries| entries.iter().any(|entry| is_ours(entry, target)))
            })
        })
}

/// One hook entry in the shape the agent reads.
fn command_entry(command: &str) -> Value {
    serde_json::json!({ "type": "command", "command": command })
}

/// The file's text, or `None` when it does not exist.
fn read(path: &Path) -> Result<Option<String>, HookError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HookError::Unreadable(format!(
            "{}: {error}",
            path.display()
        ))),
    }
}

/// The file's top-level object. An absent or empty file is an empty one, which is a user who has
/// not written this config yet rather than an error.
fn parse(text: &str) -> Result<Map<String, Value>, HookError> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text) {
        Ok(Value::Object(root)) => Ok(root),
        Ok(_) => Err(HookError::Malformed(
            "its top level is not a JSON object".to_owned(),
        )),
        Err(error) => Err(HookError::Malformed(error.to_string())),
    }
}

/// Serialise `root` in the layout `original` was written in.
///
/// The indent is the file's own first one, so a four-space config comes back four-space; the
/// trailing newline follows what was there. A file being created takes two spaces and a newline,
/// which is what the agents whose files these are write themselves.
fn render(root: &Map<String, Value>, original: Option<&str>) -> String {
    let indent = original.and_then(first_indent).unwrap_or("  ".to_owned());
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&Value::Object(root.clone()), &mut serializer)
        .expect("a Value serialises into a Vec");
    let mut text = String::from_utf8(out).expect("serde_json emits UTF-8");
    if original.is_none_or(|original| original.ends_with('\n')) {
        text.push('\n');
    }
    text
}

/// The leading whitespace of the file's first indented line — one level of its own indent.
fn first_indent(text: &str) -> Option<String> {
    text.lines()
        .map(|line| line.len() - line.trim_start().len())
        .zip(text.lines())
        .find(|(width, line)| *width > 0 && !line.trim().is_empty())
        .map(|(width, line)| line[..width].to_owned())
}

/// What can go wrong reaching into a file sprag does not own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    /// `$HOME` names nowhere absolute, so there is no config directory to find.
    NoHome,
    /// The file is there and could not be read.
    Unreadable(String),
    /// The file is there and is not JSON sprag can edit. Reported rather than reshaped.
    Malformed(String),
    /// The edit could not be written.
    Unwritable(String),
    /// No target goes by that name.
    UnknownTarget(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => write!(f, "no absolute $HOME to find an agent's config under"),
            Self::Unreadable(why) => write!(f, "cannot read {why}"),
            Self::Malformed(why) => write!(
                f,
                "will not edit it — {why}. fix the file, or move it aside and re-run"
            ),
            Self::Unwritable(why) => write!(f, "cannot write {why}"),
            Self::UnknownTarget(name) => {
                write!(f, "no agent called {name:?}. known: ")?;
                for (index, target) in TARGETS.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", target.name)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for HookError {}

impl From<HookError> for io::Error {
    fn from(error: HookError) -> Self {
        let kind = match error {
            HookError::NoHome | HookError::UnknownTarget(_) => io::ErrorKind::InvalidInput,
            HookError::Unreadable(_) | HookError::Unwritable(_) => io::ErrorKind::PermissionDenied,
            HookError::Malformed(_) => io::ErrorKind::InvalidData,
        };
        Self::new(kind, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Where the installed binary is pretended to live. Absolute, because that is what an install
    /// resolves and what the recognition rule reads back.
    const EXE: &str = "/usr/local/bin/sprag";

    /// A settings file in a temporary directory, removed on drop.
    ///
    /// Nothing here touches `$HOME`: the plan functions take a path (see [`status_at`]), so these
    /// tests mutate no process-global state and cannot be raced by the other tests in this crate
    /// that fall back to `$HOME` when their own XDG variable is unset.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(text: Option<&str>) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sprag-hooks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("a temp dir");
            let fixture = Self(dir);
            if let Some(text) = text {
                std::fs::write(fixture.path(), text).expect("write the fixture");
            }
            fixture
        }

        fn path(&self) -> PathBuf {
            self.0.join("settings.json")
        }

        fn text(&self) -> Option<String> {
            std::fs::read_to_string(self.path()).ok()
        }

        /// Install, apply, uninstall, apply — the whole round trip a user performs.
        fn round_trip(&self) {
            install_at(&CLAUDE, self.path(), Path::new(EXE))
                .expect("a plan")
                .apply()
                .expect("the install writes");
            uninstall_at(&CLAUDE, self.path())
                .expect("a plan")
                .apply()
                .expect("the uninstall writes");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// P1, and the only real test of what "preserve the file" was decided to mean.
    ///
    /// Read TWICE with the input changed — a two-space file and a four-space one — because a
    /// renderer that emitted its own fixed indent would pass the first and fail the second, and a
    /// single fixture cannot tell the difference between preserving the layout and happening to
    /// share it.
    ///
    /// REVERT-PROOF: drop the `preserve_order` feature from the workspace and the keys come back
    /// alphabetised (`hooks` before `permissions`), turning both halves red; render with a fixed
    /// `"  "` indent and the four-space half alone goes red.
    #[test]
    fn an_install_then_uninstall_leaves_the_file_byte_identical() {
        for indent in ["  ", "    "] {
            let original = format!(
                "{{\n{indent}\"permissions\": {{\n{indent}{indent}\"allow\": [\n\
                 {indent}{indent}{indent}\"Bash\"\n{indent}{indent}]\n{indent}}},\n\
                 {indent}\"model\": \"opus\"\n}}\n"
            );
            let fixture = Fixture::new(Some(&original));
            fixture.round_trip();
            assert_eq!(
                fixture.text().as_deref(),
                Some(original.as_str()),
                "a {} -space file did not come back as it went in",
                indent.len()
            );
        }
    }

    /// P2. The preservation claim is about the user's OTHER content, so the fixture has some — on
    /// the very event the installer touches, which is where a careless `retain` would take it.
    #[test]
    fn a_foreign_hook_on_the_same_event_survives_both_halves() {
        let original = r#"{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "my-own-thing"
          }
        ]
      }
    ]
  }
}
"#;
        let fixture = Fixture::new(Some(original));

        install_at(&CLAUDE, fixture.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written");
        let installed = fixture.text().expect("a file");
        assert!(installed.contains("my-own-thing"), "{installed}");
        assert!(installed.contains("sprag hook claude"), "{installed}");

        uninstall_at(&CLAUDE, fixture.path())
            .expect("a plan")
            .apply()
            .expect("written");
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// P3. Re-installing is how an upgrade happens, so it must not accumulate.
    #[test]
    fn installing_twice_leaves_one_entry_and_the_second_plan_is_empty() {
        let fixture = Fixture::new(None);
        install_at(&CLAUDE, fixture.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written");
        let second = install_at(&CLAUDE, fixture.path(), Path::new(EXE)).expect("a plan");
        assert!(second.is_empty(), "{:?}", second.changes);
        assert_eq!(
            fixture
                .text()
                .expect("a file")
                .matches("hook claude")
                .count(),
            CLAUDE.events.len(),
            "one entry per event, no more"
        );
    }

    /// The recognition rule identifies the SUBCOMMAND, not the path, so a binary that moved since
    /// the install is updated in place rather than joined by a second entry that also fires.
    #[test]
    fn a_binary_that_moved_is_corrected_in_place() {
        let fixture = Fixture::new(None);
        install_at(&CLAUDE, fixture.path(), Path::new("/old/place/sprag"))
            .expect("a plan")
            .apply()
            .expect("written");
        let plan = install_at(&CLAUDE, fixture.path(), Path::new(EXE)).expect("a plan");
        assert_eq!(
            plan.changes.len(),
            CLAUDE.events.len(),
            "{:?}",
            plan.changes
        );
        plan.apply().expect("written");
        let text = fixture.text().expect("a file");
        assert!(!text.contains("/old/place/sprag"), "{text}");
        assert_eq!(text.matches("hook claude").count(), CLAUDE.events.len());
    }

    /// P5. A subagent's event is not the pane's, and a report OUTRANKS the screen, so mapping one
    /// would park a wrong verdict on the pane until something released it.
    ///
    /// The control is the same payload without `agent_id`: without it this passes on a mapping that
    /// answers `None` to everything.
    #[test]
    fn a_subagents_payload_reports_nothing_but_the_panes_own_does() {
        let subagent = serde_json::json!({ "hook_event_name": "Stop", "agent_id": "sub-1" });
        assert_eq!(report_for(&CLAUDE, &subagent), None);

        let pane = serde_json::json!({ "hook_event_name": "Stop" });
        assert_eq!(
            report_for(&CLAUDE, &pane),
            Some(Outcome::Report(AgentState::Idle))
        );
    }

    /// `SubagentStop` is refused by being absent from the table rather than by a branch — the
    /// property that makes the table the only place an event acquires a meaning.
    #[test]
    fn an_event_the_table_does_not_name_means_nothing() {
        for event in ["SubagentStop", "PreCompact", ""] {
            let payload = serde_json::json!({ "hook_event_name": event });
            assert_eq!(report_for(&CLAUDE, &payload), None, "{event}");
        }
        assert_eq!(report_for(&CLAUDE, &serde_json::json!({})), None);
        assert_eq!(report_for(&CLAUDE, &Value::Null), None);
    }

    /// Every event that IS named maps to something a reporter may say — the vocabulary has one
    /// definition (`AgentState::wire_str`), and a table entry that could not be spoken would be a
    /// hook that fires and is refused.
    #[test]
    fn every_named_event_maps_onto_the_reportable_vocabulary() {
        for (event, outcome) in CLAUDE.events {
            let payload = serde_json::json!({ "hook_event_name": event });
            assert_eq!(report_for(&CLAUDE, &payload), Some(*outcome), "{event}");
            if let Outcome::Report(state) = outcome {
                assert!(
                    state.wire_str().is_some(),
                    "{event} reports an unsayable state"
                );
            }
        }
    }

    /// P7. A config sprag cannot make sense of is one it has no business rewriting, and the refusal
    /// leaves the file exactly as it was.
    #[test]
    fn a_file_that_does_not_parse_is_refused_and_left_alone() {
        let original = "{ \"hooks\": ";
        let fixture = Fixture::new(Some(original));
        let error = install_at(&CLAUDE, fixture.path(), Path::new(EXE)).expect_err("refused");
        assert!(matches!(error, HookError::Malformed(_)), "{error:?}");
        assert_eq!(fixture.text().as_deref(), Some(original));

        let error = uninstall_at(&CLAUDE, fixture.path()).expect_err("refused");
        assert!(matches!(error, HookError::Malformed(_)), "{error:?}");
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// The limit of the preservation claim, asserted so it is recorded rather than discovered.
    ///
    /// An event array that was ALREADY EMPTY does not survive a round trip: once our group is
    /// removed the file is in exactly the state it would be in had the event never been listed, so
    /// nothing can tell the two apart. What is preserved is CONFIGURATION — an empty array holds
    /// none — and an uninstall that touched nothing still touches nothing.
    #[test]
    fn an_empty_event_does_not_survive_the_round_trip_and_an_untouched_file_does() {
        let original = "{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}\n";
        let fixture = Fixture::new(Some(original));

        // Nothing of ours is in it, so an uninstall on its own changes nothing at all.
        let plan = uninstall_at(&CLAUDE, fixture.path()).expect("a plan");
        assert!(plan.is_empty(), "{:?}", plan.changes);
        assert_eq!(fixture.text().as_deref(), Some(original));

        fixture.round_trip();
        assert_eq!(fixture.text().as_deref(), Some("{}\n"));
    }

    /// The `matcher` trap: a group can filter every entry inside it, so sprag's hook goes in a group
    /// of its own. Joining the foreign group here would narrow sprag's reporting to `Bash` alone —
    /// a defect whose only symptom is a pane that quietly stops reporting.
    #[test]
    fn our_entry_never_joins_a_group_that_carries_a_matcher() {
        let original = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "my-own-thing"
          }
        ]
      }
    ]
  }
}
"#;
        let fixture = Fixture::new(Some(original));
        install_at(&CLAUDE, fixture.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written");

        let root: Value = serde_json::from_str(&fixture.text().expect("a file")).expect("JSON");
        let groups = root["hooks"]["PreToolUse"]
            .as_array()
            .expect("an array of groups");
        assert_eq!(groups.len(), 2, "ours is beside theirs, not inside it");
        let ours = groups
            .iter()
            .find(|group| {
                group["hooks"]
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(|entry| is_ours(entry, &CLAUDE)))
            })
            .expect("our group");
        assert!(ours.get("matcher").is_none(), "{ours}");

        uninstall_at(&CLAUDE, fixture.path())
            .expect("a plan")
            .apply()
            .expect("written");
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// A file sprag created has no previous contents to keep, and one it edited does. The backup is
    /// the answer to a correct write the user did not want.
    #[test]
    fn a_backup_is_kept_for_a_file_we_did_not_create() {
        let created = Fixture::new(None);
        assert_eq!(
            install_at(&CLAUDE, created.path(), Path::new(EXE))
                .expect("a plan")
                .apply()
                .expect("written"),
            None
        );

        let original = "{\n  \"model\": \"opus\"\n}\n";
        let edited = Fixture::new(Some(original));
        let backup = install_at(&CLAUDE, edited.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written")
            .expect("a backup");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("the backup"),
            original
        );
    }

    /// What `list-hooks` reads: a directory that is not there, one that is, and one wired up.
    #[test]
    fn the_status_counts_what_is_actually_wired() {
        let fixture = Fixture::new(None);
        let before = status_at(&CLAUDE, fixture.path());
        assert!(before.present, "the fixture's directory exists");
        assert_eq!((before.installed, before.total), (0, CLAUDE.events.len()));
        assert!(!before.complete());

        install_at(&CLAUDE, fixture.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written");
        assert!(status_at(&CLAUDE, fixture.path()).complete());

        let absent = status_at(&CLAUDE, PathBuf::from("/nowhere/at/all/settings.json"));
        assert!(!absent.present);
    }

    /// The recognition rule, read at its edges: ours needs BOTH the subcommand and a program called
    /// `sprag`, so a user's own script that happens to end in the same words is not ours to remove.
    #[test]
    fn a_command_is_ours_only_when_sprag_runs_the_subcommand() {
        assert!(CLAUDE.owns("/usr/local/bin/sprag hook claude"));
        assert!(CLAUDE.owns("sprag hook claude"));
        assert!(CLAUDE.owns("\"/opt/my apps/sprag\" hook claude"));
        assert!(!CLAUDE.owns("/usr/local/bin/sprag hook codex"));
        assert!(!CLAUDE.owns("my-own-thing hook claude"));
        assert!(!CLAUDE.owns("/usr/local/bin/sprag report-agent working"));
        assert!(!CLAUDE.owns(""));
    }

    /// The limitation, asserted rather than assumed: a file that was not pretty-printed comes back
    /// pretty-printed. Byte identity is a claim about files in the layout the agents themselves
    /// write, and this is where it stops.
    #[test]
    fn a_compact_file_is_re_emitted_pretty_and_the_content_survives() {
        let fixture = Fixture::new(Some("{\"model\":\"opus\"}"));
        fixture.round_trip();
        let text = fixture.text().expect("a file");
        assert_ne!(text, "{\"model\":\"opus\"}");
        assert_eq!(
            serde_json::from_str::<Value>(&text).expect("still JSON"),
            serde_json::json!({ "model": "opus" })
        );
    }
}
