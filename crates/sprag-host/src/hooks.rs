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
//! Two agents differ in that table and not in code: `claude` raises `Notification` when it needs
//! the human and `codex` raises `PermissionRequest`, and that is the whole of the difference
//! between them. Both name their payload's event in `hook_event_name` and both mark a subagent's
//! event with `agent_id`, so [`report_for`] reads either without knowing which it has.
//!
//! ## What "preserving the file" means here
//!
//! Not merely "do not clobber the user's other keys" — a parse/edit/serialise round trip preserves
//! those and still destroys comment, order and layout. What is preserved, per format:
//!
//! * **JSON** — key order, through `serde_json`'s `preserve_order` feature (enabled at the
//!   workspace root for this module; without it every object in the user's file comes back
//!   alphabetised); indentation, by reading the file's own first indent and re-emitting with it;
//!   and the trailing newline, or its absence. What it does NOT preserve is a file that was not
//!   pretty-printed to begin with, or a comment, because JSON has none.
//! * **TOML** — everything, including comments, because [`toml_edit`] edits the parsed document in
//!   place rather than re-emitting it. [`crate::config`] already trusts it for the same job on the
//!   user's own config.
//!
//! That difference is why nothing here writes without being asked: the caller renders
//! [`Plan::changes`], and [`Plan::apply`] keeps a `.sprag-backup` copy of a file it did not create.
//!
//! ## Writing the file is not always the whole install
//!
//! An agent may refuse to RUN what its own config now names. `codex` hashes each configured hook
//! and holds it until the user reviews it, and it has a feature switch that turns every hook off at
//! once. Neither is sprag's to decide: [`Target::follow_up`] says what the user must still do, and
//! [`Status::disabled_by`] reports a switch that would make an otherwise complete install dead.
//! What is never done is answering for the user — the trust record is not forged and not read, so
//! nothing here can report a hook as running because it guessed that somebody approved it.
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
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

/// What a hook payload means for the pane the agent is running in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Report this state, naming the agent — the report outranks whatever the screen argues.
    Report(AgentState),
    /// Hand the pane back to screen inference. The agent is gone while its pane's child (a shell)
    /// lives, which is the one exit the sweep's own EOF rule cannot see.
    Release,
}

/// The language an agent's configuration is written in.
///
/// It selects a DOCUMENT, not a code path: everything this module decides — that our entry sits in
/// a group of its own, that an entry is ours by its subcommand, that a re-install corrects rather
/// than duplicates, that an uninstall prunes only what it emptied — is written once and reads
/// either document through one private seam. What differs between the two is tree surgery over
/// incompatible node types, and that is the only thing that does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A JSON object, edited through `serde_json` and re-emitted at the file's own indent.
    Json,
    /// A TOML document, edited in place through [`toml_edit`], which preserves even comments.
    Toml,
}

/// An agent whose configuration sprag knows how to instrument.
///
/// The fields are data, so a further agent is a further `const` rather than further code — the
/// property that decides whether this abstraction was the right one, and the reason a second target
/// was built before a third.
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
    /// The variable that relocates that directory, when the agent has one.
    ///
    /// Not a nicety: an agent reads its config from where this points, so ignoring it installs into
    /// a file the agent never opens while every event still reads as wired. See [`Target::path`].
    home_var: Option<&'static str>,
    /// The file inside it that holds the hooks.
    file: &'static str,
    /// How that file is written.
    format: Format,
    /// A dotted path to the agent's own switch for hooks, when it has one. A `false` there means
    /// every hook installed is inert — see [`Status::disabled_by`].
    disable_switch: Option<&'static str>,
    /// The flag this agent takes ONE launch's configuration on, when it has one.
    ///
    /// The difference between instrumenting a MACHINE and instrumenting a LAUNCH. Everything else
    /// in this module edits the user's own configuration file, which reaches every run of that agent
    /// anywhere — right for a user who asked for it, and not something sprag may do on their behalf
    /// because they opened a pane. This flag is how sprag instruments the agent it is starting
    /// ITSELF, leaving every other copy on the machine untouched: see [`Target::session_args`].
    ///
    /// `None` says this agent has no such door and its users go through `install-hooks`. That is
    /// codex's answer TODAY rather than forever, and the reason is a MEASUREMENT that came back
    /// unable to discriminate rather than an assumption. Its per-run override is `-c key=value` with
    /// the value parsed as TOML, so the shape is spellable; what nobody has established is whether
    /// codex HONOURS a hook it was handed that way, and `--strict-config` cannot answer it —
    /// `codex doctor` accepts a deliberately bogus `-c this_key_does_not_exist=42` with output
    /// identical to the control, so it does not validate overrides at all. Settling it needs a real
    /// codex session. An unverified `Some` would be worse than an honest `None`: it would append a
    /// flag to somebody's editor session and find out at their expense.
    session_flag: Option<&'static str>,
    /// What the user must still do after the file is written, when writing it is not enough.
    ///
    /// Printed by the installer and by `list-hooks`. It exists because an agent may hold a hook it
    /// has not shown its user, and a status that ignored that would report a hook as installed
    /// while it never runs.
    pub follow_up: Option<&'static str>,
    /// Every event installed, and what a payload for it means. See the module docs: an event
    /// absent here is an event with no meaning, which is how `SubagentStop` is refused.
    pub events: &'static [(&'static str, Outcome)],
}

/// Claude Code — `~/.claude/settings.json`, relocatable with `CLAUDE_CONFIG_DIR`.
pub const CLAUDE: Target = Target {
    name: "claude",
    label: "Claude Code",
    agent: "claude",
    dir: ".claude",
    home_var: Some("CLAUDE_CONFIG_DIR"),
    file: "settings.json",
    format: Format::Json,
    disable_switch: None,
    // Verified against `claude --help` on the box that wrote this: "--settings <file-or-json> —
    // Path to a settings JSON file or a JSON string to load additional settings from". The JSON
    // form is what makes this a launch and not a file: nothing is written, so nothing is left
    // behind to version, clean up, or point at a daemon that has since gone.
    session_flag: Some("--settings"),
    follow_up: None,
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

/// OpenAI's `codex` CLI — `~/.codex/config.toml`, relocatable with `CODEX_HOME`.
///
/// The events were read from the JSON Schemas `codex` embeds for its own hook payloads
/// (`<event>.command.input`), not from a description of them. Its schema is Claude's with one
/// substitution, which is why the two targets differ by a table rather than by a branch.
pub const CODEX: Target = Target {
    name: "codex",
    label: "codex",
    agent: "codex",
    dir: ".codex",
    home_var: Some("CODEX_HOME"),
    file: "config.toml",
    format: Format::Toml,
    // Verified against `codex features list` with an empty config as the control: this reads true
    // when unset, so sprag does not write it. It reads false only if the user turned hooks off, and
    // then every entry below is inert — which is worth saying and not worth overriding.
    disable_switch: Some("features.hooks"),
    // See `session_flag`: codex's per-run overrides are `-c key=value` over TOML, and no one has
    // run whether a hooks table can be spelled that way. Its users go through `install-hooks`.
    session_flag: None,
    // codex hashes each configured hook and holds it until its user has seen it. Writing the file
    // is therefore only half the install, and the half sprag must not do for them.
    follow_up: Some(
        "codex asks before it runs a hook it has not seen: start it and accept the review prompt, \
         or the entries above stay installed and idle",
    ),
    events: &[
        ("UserPromptSubmit", Outcome::Report(AgentState::Working)),
        ("PreToolUse", Outcome::Report(AgentState::Working)),
        ("PostToolUse", Outcome::Report(AgentState::Working)),
        // codex's own name for the moment it needs the human — it has no `Notification`, and this
        // is the only row in which the two targets' tables differ in kind.
        ("PermissionRequest", Outcome::Report(AgentState::Blocked)),
        ("Stop", Outcome::Report(AgentState::Idle)),
        ("SessionEnd", Outcome::Release),
    ],
};

/// Every target an install can name.
pub const TARGETS: &[Target] = &[CLAUDE, CODEX];

/// The target `name` addresses, or `None` for a token that is not one.
#[must_use]
pub fn target(name: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|target| target.name == name)
}

impl Target {
    /// The file an install edits.
    ///
    /// The agent's own override wins over `$HOME`, because the agent reads its configuration from
    /// wherever that variable points: honouring `$HOME` alone would write into a file the agent
    /// never opens, and every event would still read as wired. That is the same shape as an
    /// installed entry naming a binary that has since moved ([`Status::missing_program`]) — a
    /// complete install that cannot work — and it is why this is checked rather than assumed.
    ///
    /// # Errors
    ///
    /// [`HookError::NoHome`] when neither the override nor `$HOME` names anywhere, and
    /// [`HookError::AmbiguousHome`] when the override is set to a RELATIVE path: the agent would
    /// resolve it against whatever directory it was started in, so there is no one file this could
    /// mean, and guessing would write into the wrong one.
    pub fn path(&self) -> Result<PathBuf, HookError> {
        Ok(self.dir_path()?.join(self.file))
    }

    /// [`path`](Self::path)'s directory half, reading the environment.
    fn dir_path(&self) -> Result<PathBuf, HookError> {
        self.dir_from(
            self.home_var.and_then(std::env::var_os),
            std::env::var_os("HOME"),
        )
    }

    /// [`dir_path`](Self::dir_path)'s DECISION, separated from the environment it reads.
    ///
    /// Split so the rule can be proven without setting a process-global variable: a test that did
    /// would race every other test in this crate that reads one, and a rule about which of two
    /// paths wins should not need a whole process to state it.
    fn dir_from(
        &self,
        relocated: Option<std::ffi::OsString>,
        home: Option<std::ffi::OsString>,
    ) -> Result<PathBuf, HookError> {
        if let Some(var) = self.home_var
            && let Some(set) = relocated.filter(|value| !value.is_empty())
        {
            let dir = PathBuf::from(set);
            return if dir.is_absolute() {
                Ok(dir)
            } else {
                Err(HookError::AmbiguousHome(var.to_owned()))
            };
        }
        home.map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .map(|home| home.join(self.dir))
            .ok_or(HookError::NoHome)
    }

    /// The command an installed entry runs: this binary, at an absolute path, plus the subcommand
    /// that reads a payload for this target.
    ///
    /// The path is SHELL-QUOTED, because an entry's `command` is a command LINE the agent runs
    /// through a shell — so an unquoted `/home/a dir/sprag` is the agent trying to run `/home/a`.
    /// Measured rather than reasoned about: `sh -c "<path> hook claude"` exits 127 with
    /// *"/home/a: not found"*, and nothing downstream can tell that from an agent whose user never
    /// installed anything. The reader that has to recover the path again is
    /// [`program_of`](Self::program_of), through the stated inverse.
    #[must_use]
    pub fn command(&self, exe: &Path) -> String {
        format!(
            "{} hook {}",
            crate::shellword::shell_quote(&exe.display().to_string()),
            self.name
        )
    }

    /// The program an installed command runs, when that command is one of ours.
    ///
    /// See the module docs on why this matches the SUBCOMMAND rather than the path — and
    /// [`status`], which needs the path back out to tell an entry that still WORKS from one that
    /// merely still parses.
    #[must_use]
    pub fn program_of(&self, command: &str) -> Option<String> {
        let program = crate::shellword::shell_unquote(
            command
                .strip_suffix(&format!(" hook {}", self.name))?
                .trim(),
        );
        Path::new(&program)
            .file_name()
            .is_some_and(|name| name == "sprag")
            .then_some(program)
    }

    /// Whether `command` is one of ours.
    #[must_use]
    pub fn owns(&self, command: &str) -> bool {
        self.program_of(command).is_some()
    }

    /// The arguments that instrument ONE launch of this agent, whose own argv is `argv` and whose
    /// reports reach the daemon through the `sprag` at `exe`.
    ///
    /// `None` when the launch must be left exactly as its caller wrote it: an agent with no
    /// per-launch door (no `session_flag` in its table), or an `argv` that already carries that flag
    /// in either of the two spellings a command line has for one. The
    /// second is a refusal and not an oversight — a caller passing its own settings has said what
    /// this launch is configured by, and a second copy of the same flag is a precedence question no
    /// agent's manual answers the same way twice. The cost is stated rather than hidden: such a
    /// launch reports nothing, and its supervisor is told so by
    /// [`sprag_plugin::Authority`] rather than left to assume.
    ///
    /// It is asked HERE rather than by the source that calls this, because a caller that has to
    /// remember a refusal is the caller that forgets it (R349).
    ///
    /// # One table, three readers
    ///
    /// The module docs called [`Target::events`] one table with two readers — what an install
    /// WRITES and what [`report_for`] MEANS. This is the third, and the reason it is derived here
    /// rather than spelled out at the launch site is R344's rule: a second reader that built the
    /// same document from its own idea of the events would parse perfectly and report the wrong
    /// thing. An event added to the table is instrumented on both paths or on neither.
    ///
    /// The DOCUMENT is the same object an install puts in the user's file, cut down to sprag's own
    /// group — built by the same private entry renderer, so the timeout and the one command per
    /// event cannot differ between the two. It
    /// travels as a JSON string rather than a temporary file so that a launch leaves nothing on
    /// disk: a file would have to outlive the agent, be cleaned up after a daemon that was killed,
    /// and be readable by whoever the agent runs as.
    #[must_use]
    pub fn session_args(&self, argv: &[String], exe: &Path) -> Option<Vec<String>> {
        let flag = self.session_flag?;
        // BOTH spellings of a flag that is already there. A command line may join a flag to its
        // value with `=`, and a check that read only the separated form would append a second
        // `--settings` to `claude --settings={...}` — the exact collision this refusal exists to
        // avoid, met by the one spelling nobody tests with.
        let joined = format!("{flag}=");
        if argv
            .iter()
            .any(|arg| arg == flag || arg.starts_with(&joined))
        {
            return None;
        }
        let command = self.command(exe);
        let mut hooks = Map::new();
        for (event, _) in self.events {
            hooks.insert(
                (*event).to_owned(),
                serde_json::json!([{ "hooks": [json_entry(&command)] }]),
            );
        }
        Some(vec![
            flag.to_owned(),
            serde_json::json!({ "hooks": Value::Object(hooks) }).to_string(),
        ])
    }
}

/// How long the AGENT waits for one of these hooks before giving up on it, in seconds.
///
/// The second half of a defence whose first half is the client's own read deadline, and it exists
/// because this hook runs in the agent's CRITICAL PATH: an agent waits for its hooks, so a sprag
/// daemon that accepts a connection and then wedges would stall somebody's editing session. Our own
/// deadline is the tighter of the two and trips first, in silence; this is the backstop for the
/// cases a client-side deadline cannot cover, and it is written into the entry because only the
/// agent can enforce it.
pub const AGENT_TIMEOUT_SECS: u64 = 5;

/// A reporter's own clock, in nanoseconds since boot.
///
/// [`crate::agent::AgentRegistry::report`] judges freshness per SOURCE, so what a hook needs is a
/// number that never goes backwards between two events of one session. A wall clock is the obvious
/// choice and the wrong one: NTP steps it, and a report arriving with a smaller `seq` than its
/// predecessor is REFUSED — the pane would then hold a stale state until the next event. This
/// cannot be stepped, and on Linux it keeps running across suspend, which a laptop closing its lid
/// mid-turn makes an ordinary case rather than a theoretical one.
///
/// It restarts at zero on a reboot, which is sound because nothing survives one: the daemon holding
/// the previous numbers dies with it, and [`crate::durability`] deliberately does not persist a
/// tracker's report — a verdict is derived state, not workspace state.
///
/// `None` only if the platform refuses the clock, which is a reason to say nothing rather than to
/// report with a number that means something else.
#[must_use]
pub fn report_seq() -> Option<u64> {
    // Linux's BOOTTIME includes time suspended; every other unix gets the closest it has. Both are
    // monotonic, which is the property being bought.
    #[cfg(target_os = "linux")]
    const CLOCK: libc::clockid_t = libc::CLOCK_BOOTTIME;
    #[cfg(not(target_os = "linux"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes a `timespec` through the pointer, and `now` is a live local of
    // exactly that type. The clock id is a constant the platform defines.
    if unsafe { libc::clock_gettime(CLOCK, &raw mut now) } != 0 {
        return None;
    }
    u64::try_from(now.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(now.tv_nsec).ok()?)
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
    /// An installed entry naming an absolute program that is no longer on disk.
    ///
    /// The difference between an entry that still PARSES and one that still WORKS. Recognition
    /// matches the subcommand rather than the path (so a moved binary can be repaired in place),
    /// and the cost of that choice is exactly this: without checking, a `sprag` that has since been
    /// moved or removed reads as `installed` while every hook it left behind fails silently. A bare
    /// program name is resolved on `PATH` at hook time and is not a claim this can falsify, so only
    /// an absolute path is checked.
    pub missing_program: Option<PathBuf>,
    /// The agent's own switch for hooks, named here when the config turns it OFF.
    ///
    /// The third way a complete install can be a dead one, beside a gone binary and a hook the
    /// agent has not yet shown its user: an agent that can disable every hook at once, whose user
    /// has. Reported and never written, because a switch in the agent's own namespace is theirs to
    /// hold — and because it reads as ON unless somebody said otherwise, so writing it would be
    /// dead weight in every install but the one where it overrides a decision.
    pub disabled_by: Option<&'static str>,
}

impl Status {
    /// Whether every event is wired.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.installed == self.total
    }

    /// Whether something is wired that could not run as things stand.
    ///
    /// The two causes are deliberately one question: to a user, an install that cannot fire is one
    /// state however it got there, and a caller that had to ask twice would one day ask once.
    #[must_use]
    pub fn inert(&self) -> bool {
        self.installed > 0 && (self.missing_program.is_some() || self.disabled_by.is_some())
    }

    /// Whether this agent ALREADY reports on its own — every event wired, and nothing stopping any
    /// of them from firing.
    ///
    /// The question [`launch_args`] asks before instrumenting a launch, and it is the CONJUNCTION
    /// rather than either half: a complete install whose binary has moved reports nothing, so
    /// treating `complete()` alone as reporting would leave exactly the user whose integration is
    /// broken with no reporting at all — and no way to tell, since their config says it is wired.
    #[must_use]
    pub fn reporting(&self) -> bool {
        self.complete() && !self.inert()
    }
}

/// What sprag adds to `argv` so the agent it names reports its own turn boundaries — the whole of
/// the decision [`crate::pane_args_source`] makes at a pane's birth, reading this machine.
///
/// Empty for everything that is not an agent with a per-launch door, which is nearly every pane
/// ever opened: the source is consulted on every birth and this is what it answers for a shell.
///
/// # Why an agent whose OWN config already reports is left alone
///
/// The two mechanisms are exclusive by construction rather than by hoping they compose. An agent
/// merges the settings it is handed with the settings it reads, so an `install-hooks` user whose
/// launch was also instrumented would run every hook TWICE — two processes and two round trips per
/// event, in the agent's critical path, for a level that is idempotent and would look identical
/// either way. A user who asked for the machine-wide install gets exactly what they asked for, and
/// sprag adds nothing on top of it.
///
/// An install that cannot RUN does not count as reporting — see [`Status::reporting`], which is
/// where that conjunction lives and is tested.
#[must_use]
pub fn launch_args(argv: &[String], exe: &Path) -> Vec<String> {
    launch_args_from(argv, exe, |target| {
        status(target).is_ok_and(|status| status.reporting())
    })
}

/// [`launch_args`]'s DECISION, separated from the machine it reads.
///
/// Split for the reason [`Target::dir_from`] is: what a launch carries and whether a user's own
/// config already reports fail for different reasons, and the rule should be provable without a
/// `$HOME` full of agent configuration — a test that wrote one would be a test of this module's
/// file surgery, which has its own.
fn launch_args_from(
    argv: &[String],
    exe: &Path,
    already_reports: impl Fn(&'static Target) -> bool,
) -> Vec<String> {
    // The PROGRAM decides, by its basename, so `/usr/local/bin/claude` and `claude` are one agent
    // and `sh -c claude` is not: an argv sprag did not write is one whose words it cannot read, and
    // appending a flag to a shell's would hand the shell an argument meant for something else.
    let Some(target) = argv
        .first()
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .and_then(target)
    else {
        return Vec::new();
    };
    if already_reports(target) {
        return Vec::new();
    }
    target.session_args(argv, exe).unwrap_or_default()
}

/// Read `target`'s configuration and report where the integration stands.
///
/// A file that does not parse is NOT an error here: `list-hooks` answering "I cannot tell" for a
/// broken file is more useful than a listing that refuses to print. It reports zero installed
/// events, and the install that follows is the thing that refuses, with the parse error.
///
/// # Errors
///
/// As [`Target::path`].
pub fn status(target: &'static Target) -> Result<Status, HookError> {
    Ok(status_at(target, target.path()?))
}

/// [`status`] against a named file.
///
/// The split is not for tests alone, though it is what lets them run without touching a
/// process-global `$HOME`: path RESOLUTION and the edit MECHANISM fail for different reasons and a
/// caller that already knows the path should not be able to trip the first.
fn status_at(target: &'static Target, path: PathBuf) -> Status {
    let present = path.parent().is_some_and(Path::is_dir);
    let doc = read(&path)
        .ok()
        .flatten()
        .and_then(|text| Doc::parse(target.format, &text).ok())
        .unwrap_or_else(|| Doc::empty(target.format));
    let mut installed = 0;
    let mut missing_program = None;
    for (event, _) in target.events {
        let commands = doc.ours_under(target, event);
        if commands.is_empty() {
            continue;
        }
        installed += 1;
        if missing_program.is_none() {
            missing_program = commands.iter().find_map(|command| {
                let program = PathBuf::from(target.program_of(command)?);
                (program.is_absolute() && !program.exists()).then(|| program.to_path_buf())
            });
        }
    }
    Status {
        target: target.name,
        path,
        present,
        installed,
        total: target.events.len(),
        missing_program,
        disabled_by: target
            .disable_switch
            .filter(|switch| doc.bool_at(switch) == Some(false)),
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
/// As [`Target::path`]; [`HookError::Unreadable`] when the file exists and cannot be read; and
/// [`HookError::Malformed`] when it does not parse, or parses into something that is not the shape
/// its own agent reads — a config this cannot make sense of is one it has no business rewriting.
pub fn plan_install(target: &'static Target, exe: &Path) -> Result<Plan, HookError> {
    install_at(target, target.path()?, exe)
}

/// [`plan_install`] against a named file — see [`status_at`] on why the split exists.
fn install_at(target: &'static Target, path: PathBuf, exe: &Path) -> Result<Plan, HookError> {
    let original = read(&path)?;
    let mut doc = Doc::parse(target.format, original.as_deref().unwrap_or_default())?;
    let command = target.command(exe);
    let mut changes = Vec::new();

    for (event, _) in target.events {
        match doc.put(target, event, &command)? {
            Placement::Unchanged => {}
            Placement::Updated => changes.push(format!("~ hooks.{event}  ->  {command}")),
            Placement::Added => changes.push(format!("+ hooks.{event}  ->  {command}")),
        }
    }

    Ok(Plan {
        target: target.name,
        text: doc.render(original.as_deref()),
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
    uninstall_at(target, target.path()?)
}

/// [`plan_uninstall`] against a named file — see [`status_at`] on why the split exists.
fn uninstall_at(target: &'static Target, path: PathBuf) -> Result<Plan, HookError> {
    let original = read(&path)?;
    let mut doc = Doc::parse(target.format, original.as_deref().unwrap_or_default())?;
    let changes = doc
        .take_ours(target)
        .into_iter()
        .map(|event| format!("- hooks.{event}"))
        .collect();

    Ok(Plan {
        target: target.name,
        text: doc.render(original.as_deref()),
        path,
        changes,
        original,
    })
}

/// What [`Doc::put`] did to one event, which is also what the plan reports for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// The entry we wanted is already exactly there.
    Unchanged,
    /// An entry of ours was there and said something else — a binary that has since moved, or one
    /// written by a sprag that predates a field. Corrected in place rather than joined by a second.
    Updated,
    /// Nothing of ours was under this event.
    Added,
}

/// A parsed agent configuration, in whichever language its agent writes it.
///
/// The four operations below are every question this module asks of a config file. Each is answered
/// twice, once per format, and those answers sit next to each other on purpose: two implementations
/// of one policy stay honest when they can be read together, and drift when they are pages apart.
/// Everything ELSE — which events to write, how an entry is recognised, what a re-install corrects,
/// what an uninstall prunes, how the plan is rendered and the file replaced — is written once,
/// above, and never asks which format it has.
enum Doc {
    /// Claude Code's `settings.json` and anything else shaped like it.
    Json(Map<String, Value>),
    /// codex's `config.toml`. Boxed because a `DocumentMut` carries a whole table inline and the
    /// other variant is a handful of words.
    Toml(Box<DocumentMut>),
}

impl Doc {
    /// A configuration with nothing in it — a user who has not written this file yet.
    fn empty(format: Format) -> Self {
        match format {
            Format::Json => Self::Json(Map::new()),
            Format::Toml => Self::Toml(Box::default()),
        }
    }

    /// Parse `text`. An absent or blank file is [`empty`](Self::empty) rather than an error.
    fn parse(format: Format, text: &str) -> Result<Self, HookError> {
        if text.trim().is_empty() {
            return Ok(Self::empty(format));
        }
        match format {
            Format::Json => match serde_json::from_str(text) {
                Ok(Value::Object(root)) => Ok(Self::Json(root)),
                Ok(_) => Err(HookError::Malformed(
                    "its top level is not a JSON object".to_owned(),
                )),
                Err(error) => Err(HookError::Malformed(error.to_string())),
            },
            Format::Toml => text
                .parse::<DocumentMut>()
                .map(|doc| Self::Toml(Box::new(doc)))
                .map_err(|error| HookError::Malformed(error.to_string())),
        }
    }

    /// The boolean at a dotted path, when there is one there.
    ///
    /// `None` for absent and for present-but-not-a-boolean alike, because the only caller asks
    /// whether the user turned something OFF, and neither of those is that.
    fn bool_at(&self, path: &str) -> Option<bool> {
        match self {
            Self::Json(root) => {
                let mut segments = path.split('.');
                let mut value = root.get(segments.next()?)?;
                for segment in segments {
                    value = value.get(segment)?;
                }
                value.as_bool()
            }
            Self::Toml(doc) => {
                let mut item = doc.as_item();
                for segment in path.split('.') {
                    item = item.as_table_like()?.get(segment)?;
                }
                item.as_bool()
            }
        }
    }

    /// Every sprag command installed under `event`, in file order.
    ///
    /// Tolerant by construction: this is what a listing reads, and a config shaped in a way we
    /// cannot edit still deserves the honest answer "nothing of ours is in it" rather than a
    /// refusal to print. The strict reading happens in [`put`](Self::put), which is where being
    /// wrong would cost the user their file.
    fn ours_under(&self, target: &Target, event: &str) -> Vec<String> {
        let owned = |command: &str| target.owns(command).then(|| command.to_owned());
        match self {
            Self::Json(root) => root
                .get("hooks")
                .and_then(Value::as_object)
                .and_then(|hooks| hooks.get(event))
                .and_then(Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
                        .flatten()
                        .filter_map(|entry| entry.get("command").and_then(Value::as_str))
                        .filter_map(owned)
                        .collect()
                })
                .unwrap_or_default(),
            Self::Toml(doc) => doc
                .get("hooks")
                .and_then(Item::as_table_like)
                .and_then(|hooks| hooks.get(event))
                .and_then(Item::as_array_of_tables)
                .map(|groups| {
                    groups
                        .iter()
                        .filter_map(|group| group.get("hooks"))
                        .filter_map(Item::as_array_of_tables)
                        .flatten()
                        .filter_map(|entry| entry.get("command"))
                        .filter_map(|command| command.as_str())
                        .filter_map(owned)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Put our entry under `event`, in a group of our OWN.
    ///
    /// sprag never adds its entry to somebody else's group, and the reason is not tidiness: a group
    /// may carry a `matcher`, which filters every entry inside it — in TOML exactly as in JSON.
    /// Joining a group matched on one tool would silently narrow sprag's hook to that tool, a defect
    /// with no symptom except a pane that stops reporting. Our own group carries no matcher, so it
    /// fires for everything, and it doubles as the record of what an uninstall may remove whole.
    ///
    /// An entry of ours already there is normalised WHOLE rather than field by field, so one
    /// written by an older sprag — naming a binary that has since moved, or predating a field — is
    /// brought up to date by the same re-install that repairs a moved path.
    ///
    /// # Errors
    ///
    /// [`HookError::Malformed`] when the file holds something else where the hooks belong. That is
    /// a refusal rather than a repair on purpose: overwriting it would silently discard whatever
    /// the user actually had there, and this module's whole premise is that the file is theirs.
    fn put(&mut self, target: &Target, event: &str, command: &str) -> Result<Placement, HookError> {
        let wrong = |what: &str| {
            HookError::Malformed(format!(
                "its `{what}` is not the shape {} reads, so sprag cannot add to it",
                target.label
            ))
        };
        match self {
            Self::Json(root) => {
                let hooks = root
                    .entry("hooks")
                    .or_insert_with(|| Value::Object(Map::new()));
                let hooks = hooks.as_object_mut().ok_or_else(|| wrong("hooks"))?;
                let groups = hooks
                    .entry(event.to_owned())
                    .or_insert_with(|| Value::Array(Vec::new()));
                let groups = groups
                    .as_array_mut()
                    .ok_or_else(|| wrong(&format!("hooks.{event}")))?;
                let index = match groups
                    .iter()
                    .position(|group| json_group_is_ours(group, target))
                {
                    Some(index) => index,
                    None => {
                        groups.push(serde_json::json!({ "hooks": [] }));
                        groups.len() - 1
                    }
                };
                let entries = groups[index]
                    .get_mut("hooks")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| wrong(&format!("hooks.{event}")))?;
                let wanted = json_entry(command);
                match entries.iter_mut().find(|entry| json_is_ours(entry, target)) {
                    Some(entry) if *entry == wanted => Ok(Placement::Unchanged),
                    Some(entry) => {
                        *entry = wanted;
                        Ok(Placement::Updated)
                    }
                    None => {
                        entries.push(wanted);
                        Ok(Placement::Added)
                    }
                }
            }
            Self::Toml(doc) => {
                let hooks = doc.entry("hooks").or_insert_with(|| {
                    let mut table = Table::new();
                    // Implicit, so the file gets `[[hooks.Stop]]` and never a bare `[hooks]` header
                    // above it — the layout codex writes for itself.
                    table.set_implicit(true);
                    Item::Table(table)
                });
                let hooks = hooks.as_table_mut().ok_or_else(|| wrong("hooks"))?;
                let groups = hooks
                    .entry(event)
                    .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
                let groups = groups
                    .as_array_of_tables_mut()
                    .ok_or_else(|| wrong(&format!("hooks.{event}")))?;
                let ours = groups
                    .iter()
                    .position(|group| toml_group_is_ours(group, target));
                let index = match ours {
                    Some(index) => index,
                    None => {
                        groups.push(Table::new());
                        groups.len() - 1
                    }
                };
                let entries = groups
                    .get_mut(index)
                    .expect("the group just located")
                    .entry("hooks")
                    .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
                let entries = entries
                    .as_array_of_tables_mut()
                    .ok_or_else(|| wrong(&format!("hooks.{event}")))?;
                let ours = entries.iter().position(|entry| toml_is_ours(entry, target));
                match ours {
                    Some(index) if toml_entry_says(entries.get(index), command) => {
                        Ok(Placement::Unchanged)
                    }
                    Some(index) => {
                        *entries.get_mut(index).expect("the entry just located") =
                            toml_entry(command);
                        Ok(Placement::Updated)
                    }
                    None => {
                        entries.push(toml_entry(command));
                        Ok(Placement::Added)
                    }
                }
            }
        }
    }

    /// Take our entries back out, and report the events they were under.
    ///
    /// A group is dropped only when removing OUR entry is what emptied it — so a group somebody
    /// else left empty is not collateral, and a group we shared with a hand-added entry keeps that
    /// entry and its own header. What cannot be told apart is an event array that was ALREADY
    /// empty: once our group is gone, a file that had `Stop = []` before the install and one that
    /// never listed `Stop` are in the same state. Pruning is chosen, because the alternative leaves
    /// an empty array under every installed event forever, and what is lost is a key's presence
    /// rather than a setting.
    fn take_ours(&mut self, target: &Target) -> Vec<String> {
        let mut touched = Vec::new();
        match self {
            Self::Json(root) => {
                let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
                    return touched;
                };
                for event in hooks.keys().cloned().collect::<Vec<String>>() {
                    let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
                        continue;
                    };
                    let mut removed = false;
                    for index in (0..groups.len()).rev() {
                        let Some(entries) =
                            groups[index].get_mut("hooks").and_then(Value::as_array_mut)
                        else {
                            continue;
                        };
                        let before = entries.len();
                        entries.retain(|entry| !json_is_ours(entry, target));
                        if entries.len() == before {
                            continue;
                        }
                        removed = true;
                        if entries.is_empty() {
                            groups.remove(index);
                        }
                    }
                    if !removed {
                        continue;
                    }
                    touched.push(event.clone());
                    if groups.is_empty() {
                        hooks.remove(&event);
                    }
                }
                if hooks.is_empty() && !touched.is_empty() {
                    root.remove("hooks");
                }
            }
            Self::Toml(doc) => {
                let Some(hooks) = doc.get_mut("hooks").and_then(Item::as_table_mut) else {
                    return touched;
                };
                for event in hooks
                    .iter()
                    .map(|(key, _)| key.to_owned())
                    .collect::<Vec<String>>()
                {
                    let Some(groups) = hooks.get_mut(&event).and_then(Item::as_array_of_tables_mut)
                    else {
                        continue;
                    };
                    let mut removed = false;
                    for index in (0..groups.len()).rev() {
                        let Some(entries) = groups
                            .get_mut(index)
                            .and_then(|group| group.get_mut("hooks"))
                            .and_then(Item::as_array_of_tables_mut)
                        else {
                            continue;
                        };
                        let before = entries.len();
                        entries.retain(|entry| !toml_is_ours(entry, target));
                        if entries.len() == before {
                            continue;
                        }
                        removed = true;
                        if entries.is_empty() {
                            groups.remove(index);
                        }
                    }
                    if !removed {
                        continue;
                    }
                    touched.push(event.clone());
                    if groups.is_empty() {
                        hooks.remove(&event);
                    }
                }
                if hooks.is_empty() && !touched.is_empty() {
                    doc.remove("hooks");
                }
            }
        }
        touched
    }

    /// The text to write.
    ///
    /// TOML ignores `original` because it never left it: [`toml_edit`] edits the document that was
    /// parsed, so comment, order, spacing and the trailing newline are the file's own by
    /// construction. JSON has to be re-emitted, so it takes the file's own first indent and its
    /// trailing newline back from `original` — which is the whole of what a JSON file can carry.
    fn render(&self, original: Option<&str>) -> String {
        match self {
            Self::Json(root) => {
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
            Self::Toml(doc) => doc.to_string(),
        }
    }
}

/// Whether a JSON hook entry is sprag's.
fn json_is_ours(entry: &Value, target: &Target) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| target.owns(command))
}

/// Whether a JSON group is one sprag put there.
fn json_group_is_ours(group: &Value, target: &Target) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|entries| entries.iter().any(|entry| json_is_ours(entry, target)))
}

/// One JSON hook entry in the shape the agent reads, carrying the timeout only the agent can
/// enforce.
fn json_entry(command: &str) -> Value {
    serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": AGENT_TIMEOUT_SECS,
    })
}

/// Whether a TOML hook entry is sprag's.
fn toml_is_ours(entry: &Table, target: &Target) -> bool {
    entry
        .get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| target.owns(command))
}

/// Whether a TOML group is one sprag put there.
fn toml_group_is_ours(group: &Table, target: &Target) -> bool {
    group
        .get("hooks")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|entries| entries.iter().any(|entry| toml_is_ours(entry, target)))
}

/// The TOML twin of [`json_entry`]. `command` is the only handler kind codex actually runs — its
/// own diagnostics say `prompt` and `agent` hooks, and `async`, are not supported yet — so this
/// writes that one and nothing speculative beside it.
fn toml_entry(command: &str) -> Table {
    let mut entry = Table::new();
    entry["type"] = toml_edit::value("command");
    entry["command"] = toml_edit::value(command);
    entry["timeout"] = toml_edit::value(i64::try_from(AGENT_TIMEOUT_SECS).unwrap_or(i64::MAX));
    entry
}

/// Whether a TOML entry already says exactly what [`toml_entry`] would.
///
/// Asked AGAINST [`toml_entry`] rather than by re-listing its fields, so the entry sprag writes has
/// one definition and this cannot drift from it. The length check is what makes it a whole-entry
/// comparison: a stray key means the entry is not the one we would write, and a re-install replaces
/// it whole.
fn toml_entry_says(entry: Option<&Table>, command: &str) -> bool {
    let wanted = toml_entry(command);
    entry.is_some_and(|entry| {
        entry.len() == wanted.len()
            && wanted
                .iter()
                .all(|(key, value)| entry.get(key).is_some_and(|item| toml_same(item, value)))
    })
}

/// Whether two TOML items say the same thing, ignoring the whitespace around them.
///
/// The decor has to go: a value read from a file carries the spacing it was written with, and a
/// comparison that counted that would call an entry different for having been laid out differently
/// — so every re-install would rewrite the user's file to say what it already said, and no install
/// would ever report that there was nothing to do.
fn toml_same(item: &Item, wanted: &Item) -> bool {
    let bare = |item: &Item| {
        item.as_value().map(|value| {
            let mut value = value.clone();
            value.decor_mut().clear();
            value.to_string()
        })
    };
    let item = bare(item);
    item.is_some() && item == bare(wanted)
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
    /// The agent's own config-directory variable is set to a RELATIVE path.
    ///
    /// Refused rather than resolved: the agent resolves it against whatever directory it was
    /// started in, which is not this one, so there is no single file this could mean and any guess
    /// writes into the wrong one. Carries the variable's name, because that is what the user fixes.
    AmbiguousHome(String),
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
            Self::AmbiguousHome(var) => write!(
                f,
                "${var} is a relative path, and the agent resolves it against whatever directory \
                 it starts in — set it to an absolute path"
            ),
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
            HookError::NoHome | HookError::AmbiguousHome(_) | HookError::UnknownTarget(_) => {
                io::ErrorKind::InvalidInput
            }
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

    /// One target's config file in a temporary directory, removed on drop.
    ///
    /// Nothing here touches `$HOME`: the plan functions take a path (see [`status_at`]), so these
    /// tests mutate no process-global state and cannot be raced by the other tests in this crate
    /// that fall back to `$HOME` when their own XDG variable is unset.
    struct Fixture(PathBuf, &'static Target);

    impl Fixture {
        fn new(target: &'static Target, text: Option<&str>) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sprag-hooks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("a temp dir");
            let fixture = Self(dir, target);
            if let Some(text) = text {
                std::fs::write(fixture.path(), text).expect("write the fixture");
            }
            fixture
        }

        fn path(&self) -> PathBuf {
            self.0.join(self.1.file)
        }

        fn text(&self) -> Option<String> {
            std::fs::read_to_string(self.path()).ok()
        }

        /// Install and write, returning what reached the disk.
        fn install(&self, exe: &str) -> String {
            install_at(self.1, self.path(), Path::new(exe))
                .expect("a plan")
                .apply()
                .expect("the install writes");
            self.text().expect("a file")
        }

        /// A stand-in binary that is really on disk, for the tests whose subject is NOT whether the
        /// program went missing — [`Status::missing_program`] would otherwise answer for them.
        fn program(&self) -> PathBuf {
            let program = self.0.join("sprag");
            std::fs::write(&program, "").expect("a stand-in binary");
            program
        }

        /// Install, apply, uninstall, apply — the whole round trip a user performs.
        fn round_trip(&self) {
            self.install(EXE);
            uninstall_at(self.1, self.path())
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
            let fixture = Fixture::new(&CLAUDE, Some(&original));
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
        let fixture = Fixture::new(&CLAUDE, Some(original));

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
        let fixture = Fixture::new(&CLAUDE, None);
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
        let fixture = Fixture::new(&CLAUDE, None);
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
        let fixture = Fixture::new(&CLAUDE, Some(original));
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
        let fixture = Fixture::new(&CLAUDE, Some(original));

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
        let fixture = Fixture::new(&CLAUDE, Some(original));
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
                    .is_some_and(|entries| entries.iter().any(|entry| json_is_ours(entry, &CLAUDE)))
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
        let created = Fixture::new(&CLAUDE, None);
        assert_eq!(
            install_at(&CLAUDE, created.path(), Path::new(EXE))
                .expect("a plan")
                .apply()
                .expect("written"),
            None
        );

        let original = "{\n  \"model\": \"opus\"\n}\n";
        let edited = Fixture::new(&CLAUDE, Some(original));
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
        let fixture = Fixture::new(&CLAUDE, None);
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

    /// An install whose binary has since gone is BROKEN, not installed.
    ///
    /// This is the cost of recognising the subcommand rather than the path — the choice that lets a
    /// moved binary be repaired in place. Without the check, every event reads as wired while every
    /// hook it left behind fails, which is the worst answer a status can give: a confident wrong
    /// one. The control is an install pointing at a program that IS there.
    #[test]
    fn a_binary_that_is_gone_reads_as_broken_rather_than_installed() {
        let gone = Fixture::new(&CLAUDE, None);
        let nowhere = gone.0.join("gone").join("sprag");
        install_at(&CLAUDE, gone.path(), &nowhere)
            .expect("a plan")
            .apply()
            .expect("written");
        let status = status_at(&CLAUDE, gone.path());
        assert!(status.complete(), "every event is wired");
        assert_eq!(
            status.missing_program.as_deref(),
            Some(nowhere.as_path()),
            "and every one of them would fail",
        );

        let live = Fixture::new(&CLAUDE, None);
        let program = live.0.join("sprag");
        std::fs::write(&program, "").expect("a stand-in binary");
        install_at(&CLAUDE, live.path(), &program)
            .expect("a plan")
            .apply()
            .expect("written");
        let status = status_at(&CLAUDE, live.path());
        assert!(status.complete());
        assert_eq!(status.missing_program, None, "this one is really there");
    }

    /// The timeout only the AGENT can enforce is on every entry.
    ///
    /// This hook runs in the agent's critical path, so the client's own read deadline is only half
    /// the defence: it cannot cover a `sprag` that wedges before it reaches the socket at all.
    #[test]
    fn every_installed_entry_carries_the_agent_side_timeout() {
        let fixture = Fixture::new(&CLAUDE, None);
        install_at(&CLAUDE, fixture.path(), Path::new(EXE))
            .expect("a plan")
            .apply()
            .expect("written");
        let root: Value = serde_json::from_str(&fixture.text().expect("a file")).expect("JSON");
        for (event, _) in CLAUDE.events {
            let entry = root["hooks"][event][0]["hooks"][0].clone();
            assert_eq!(entry["timeout"], Value::from(AGENT_TIMEOUT_SECS), "{event}");
        }
    }

    /// An entry written by an older sprag is brought up to date by the same re-install that repairs
    /// a moved path — the reason our own entry is normalised whole rather than field by field.
    #[test]
    fn an_entry_missing_the_timeout_is_repaired_by_a_re_install() {
        let fixture = Fixture::new(
            &CLAUDE,
            Some(
                r#"{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/local/bin/sprag hook claude"
          }
        ]
      }
    ]
  }
}
"#,
            ),
        );
        let plan = install_at(&CLAUDE, fixture.path(), Path::new(EXE)).expect("a plan");
        assert!(
            plan.changes.iter().any(|change| change.starts_with('~')),
            "the existing entry is corrected, not duplicated: {:?}",
            plan.changes,
        );
        plan.apply().expect("written");
        let root: Value = serde_json::from_str(&fixture.text().expect("a file")).expect("JSON");
        assert_eq!(root["hooks"]["Stop"].as_array().expect("groups").len(), 1);
        assert_eq!(
            root["hooks"]["Stop"][0]["hooks"][0]["timeout"],
            Value::from(AGENT_TIMEOUT_SECS),
        );
    }

    /// The reporter's clock is monotonic AND is not the wall clock.
    ///
    /// The second half is what makes this a test rather than a restatement: monotonicity alone
    /// passes on the wall clock, which is the implementation being ruled out, and "is it steppable
    /// by NTP" cannot be asked by a process that may not step it.
    ///
    /// It is measured against a FIXED anchor rather than against `SystemTime::now()`. Comparing the
    /// two live clocks proves nothing — a realtime sample taken first is below a realtime sample
    /// taken second, so that comparison passes on exactly the implementation it claims to exclude,
    /// which the revert-proof is what caught. An uptime, by contrast, would have to exceed half a
    /// century to reach a date already past.
    #[test]
    fn the_reporters_clock_is_monotonic_and_is_not_the_wall_clock() {
        /// 2020-01-01T00:00:00Z in nanoseconds since the unix epoch — a moment already gone, so any
        /// wall clock reads above it and any plausible uptime reads far below.
        const ANCHOR_NANOS: u64 = 1_577_836_800 * 1_000_000_000;

        let first = report_seq().expect("a clock");
        let second = report_seq().expect("a clock");
        assert!(second >= first, "{first} then {second}");
        assert!(
            first < ANCHOR_NANOS,
            "a boot-relative count cannot be past 2020: {first} — this is the wall clock",
        );
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

    /// P1 for the TOML half, and it is a STRONGER claim than the JSON one.
    ///
    /// `toml_edit` edits the document that was parsed rather than re-emitting it, so what has to
    /// survive here is everything JSON cannot carry: a comment, a blank line, a table the user
    /// wrote before ours and one they wrote after. This is the property the rival spends a
    /// hand-written line editor on.
    ///
    /// REVERT-PROOF: rebuild the document from a serialised round trip instead of editing it in
    /// place and the comment is the first thing to go.
    #[test]
    fn a_codex_config_comes_back_with_its_comments_and_layout_intact() {
        let original = "# what I set, and why\nmodel = \"gpt-5.6\"\n\n\
                        [tui]\n# two blank lines follow on purpose\ntheme = \"dark\"\n\n\n\
                        [features]\nweb_search = true\n";
        let fixture = Fixture::new(&CODEX, Some(original));
        fixture.round_trip();
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// P4 — the rendered entry, pinned to the text `codex` itself was shown to load.
    ///
    /// This is the only proof of the one thing no downstream check can catch. `codex` accepts an
    /// unknown event name SILENTLY: `[[hooks.PreToolUsee]]` parses, installs, and never fires. So
    /// the shape is not asserted through our own reader — which would agree with us either way —
    /// but pinned as literal text, and that literal text was handed to the installed `codex` and
    /// loaded without complaint. If the rendering drifts, this is what says so.
    #[test]
    fn the_rendered_codex_entry_is_the_text_codex_itself_loads() {
        let fixture = Fixture::new(&CODEX, None);
        let written = fixture.install(EXE);
        let expected: String = CODEX
            .events
            .iter()
            .map(|(event, _)| {
                format!(
                    "[[hooks.{event}]]\n\n[[hooks.{event}.hooks]]\n\
                     type = \"command\"\ncommand = \"{EXE} hook codex\"\ntimeout = 5\n"
                )
            })
            .collect::<Vec<String>>()
            .join("\n");
        assert_eq!(written, expected);
    }

    /// P2 for TOML. A group's `matcher` filters everything inside it in codex exactly as in Claude,
    /// so ours goes beside theirs and never into it — and their entry, their matcher and their
    /// comment all survive the uninstall.
    #[test]
    fn our_codex_entry_never_joins_a_group_that_carries_a_matcher() {
        let original = "[[hooks.PreToolUse]]\n# only for shell commands\nmatcher = \"Bash\"\n\n\
                        [[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"my-own-thing\"\n";
        let fixture = Fixture::new(&CODEX, Some(original));
        let installed = fixture.install(EXE);

        let doc: DocumentMut = installed.parse().expect("still TOML");
        let groups = doc["hooks"]["PreToolUse"]
            .as_array_of_tables()
            .expect("groups");
        assert_eq!(
            groups.len(),
            2,
            "ours is beside theirs, not inside it:\n{installed}"
        );
        let ours = groups
            .iter()
            .find(|group| toml_group_is_ours(group, &CODEX))
            .expect("our group");
        assert!(ours.get("matcher").is_none(), "{ours}");

        uninstall_at(&CODEX, fixture.path())
            .expect("a plan")
            .apply()
            .expect("written");
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// P3 for TOML: a re-install is how an upgrade happens, and a binary that moved is corrected in
    /// place rather than joined by a second entry that also fires.
    #[test]
    fn installing_into_codex_twice_leaves_one_entry_and_a_moved_binary_is_corrected() {
        let fixture = Fixture::new(&CODEX, None);
        fixture.install(EXE);
        assert!(
            install_at(&CODEX, fixture.path(), Path::new(EXE))
                .expect("a plan")
                .is_empty(),
            "the second install has nothing to do"
        );

        let moved = fixture.install("/somewhere/else/sprag");
        assert!(!moved.contains(EXE), "{moved}");
        assert_eq!(moved.matches("hook codex").count(), CODEX.events.len());
    }

    /// P5. The agent's own switch for hooks, off, makes a COMPLETE install a dead one — and the
    /// control is the same file without it, or this would pass on a status that always says
    /// "disabled". Reported and never written: see [`Target::disable_switch`].
    ///
    /// Both halves install a program that really exists, because the OTHER way an install goes
    /// inert would otherwise answer for the control.
    #[test]
    fn the_agents_own_switch_being_off_is_reported_rather_than_overridden() {
        let off = Fixture::new(&CODEX, Some("[features]\nhooks = false\n"));
        off.install(&off.program().to_string_lossy());
        let status = status_at(&CODEX, off.path());
        assert!(status.complete(), "every event is wired");
        assert_eq!(status.disabled_by, Some("features.hooks"));
        assert!(status.inert(), "and not one of them can fire");
        assert!(
            off.text().expect("a file").contains("hooks = false"),
            "the user's switch is left exactly as they set it",
        );

        let on = Fixture::new(&CODEX, None);
        on.install(&on.program().to_string_lossy());
        let status = status_at(&CODEX, on.path());
        assert_eq!(
            status.disabled_by, None,
            "unset means on, and is not reported"
        );
        assert!(!status.inert());
    }

    /// P6. A subagent's event is not the pane's, and the two agents encode "no subagent" the two
    /// ways a generated schema can — absent, and an explicit null. Both must read the same, or the
    /// filter would drop every event one of them ever sends.
    ///
    /// The control is a payload that IS a subagent's: without it this passes on a filter that never
    /// refuses anything.
    #[test]
    fn a_subagents_payload_is_refused_however_the_agent_encodes_it() {
        for target in [&CLAUDE, &CODEX] {
            let (event, outcome) = target.events[0];
            for absent in [
                serde_json::json!({ "hook_event_name": event }),
                serde_json::json!({ "hook_event_name": event, "agent_id": Value::Null }),
                serde_json::json!({ "hook_event_name": event, "agent_id": "" }),
            ] {
                assert_eq!(
                    report_for(target, &absent),
                    Some(outcome),
                    "{}: {absent}",
                    target.name
                );
            }
            let subagent = serde_json::json!({ "hook_event_name": event, "agent_id": "sub-1" });
            assert_eq!(report_for(target, &subagent), None, "{}", target.name);
        }
    }

    /// The two targets differ by a TABLE and not by a branch — the claim slice 3a made and could
    /// not test with one target.
    ///
    /// The substitution is the whole difference: codex raises `PermissionRequest` where Claude
    /// raises `Notification`, and neither knows the other's word for it. Asserted in both
    /// directions, because "codex maps PermissionRequest" alone would also pass on a table that
    /// mapped everything.
    #[test]
    fn the_two_targets_differ_by_one_row_and_agree_on_the_rest() {
        let blocked = Some(Outcome::Report(AgentState::Blocked));
        let event = |target, name: &str| {
            report_for(target, &serde_json::json!({ "hook_event_name": name }))
        };
        assert_eq!(event(&CODEX, "PermissionRequest"), blocked);
        assert_eq!(event(&CLAUDE, "Notification"), blocked);
        assert_eq!(
            event(&CODEX, "Notification"),
            None,
            "codex has no such event"
        );
        assert_eq!(
            event(&CLAUDE, "PermissionRequest"),
            None,
            "and Claude has no such event"
        );
        // codex's subagent events are refused the same way Claude's are: by being absent.
        for name in [
            "SubagentStop",
            "SubagentStart",
            "PreCompact",
            "SessionStart",
        ] {
            assert_eq!(event(&CODEX, name), None, "{name}");
        }
        // Everything else is the same table, read the same way.
        for (event, outcome) in CODEX.events {
            let payload = serde_json::json!({ "hook_event_name": event });
            assert_eq!(report_for(&CODEX, &payload), Some(*outcome), "{event}");
            if let Outcome::Report(state) = outcome {
                assert!(
                    state.wire_str().is_some(),
                    "{event} reports an unsayable state"
                );
            }
        }
    }

    /// P7 for TOML: a config sprag cannot make sense of is one it has no business rewriting.
    #[test]
    fn a_codex_config_that_does_not_parse_is_refused_and_left_alone() {
        let original = "model = \"gpt-5.6\"\n[tui\n";
        let fixture = Fixture::new(&CODEX, Some(original));
        let error = install_at(&CODEX, fixture.path(), Path::new(EXE)).expect_err("refused");
        assert!(matches!(error, HookError::Malformed(_)), "{error:?}");
        assert_eq!(fixture.text().as_deref(), Some(original));

        let error = uninstall_at(&CODEX, fixture.path()).expect_err("refused");
        assert!(matches!(error, HookError::Malformed(_)), "{error:?}");
        assert_eq!(fixture.text().as_deref(), Some(original));
    }

    /// Something else where the hooks belong is REFUSED, not replaced — in both formats.
    ///
    /// The alternative is to overwrite it, which would silently discard whatever the user actually
    /// had there. This module's whole premise is that the file is theirs, so a shape it cannot add
    /// to is a shape it declines to touch, and the file is unchanged on disk.
    #[test]
    fn something_else_where_the_hooks_belong_is_refused_rather_than_overwritten() {
        for (target, original) in [
            (&CLAUDE, "{\n  \"hooks\": \"see my other file\"\n}\n"),
            (&CODEX, "hooks = \"see my other file\"\n"),
        ] {
            let fixture = Fixture::new(target, Some(original));
            let error = install_at(target, fixture.path(), Path::new(EXE)).expect_err("refused");
            assert!(
                matches!(error, HookError::Malformed(_)),
                "{}: {error:?}",
                target.name
            );
            assert_eq!(fixture.text().as_deref(), Some(original), "{}", target.name);
        }
    }

    /// P8. An agent reads its configuration from wherever its own variable points, so that wins
    /// over `$HOME` — and a RELATIVE one is refused rather than guessed at, because the agent would
    /// resolve it against a directory that is not this one.
    ///
    /// The control is the unset case: without it this passes on a resolver that always returns the
    /// override. Read through [`Target::dir_from`] rather than the process environment, so it
    /// cannot race the other tests in this crate that read `$HOME`.
    #[test]
    fn an_agents_own_config_directory_variable_wins_over_home() {
        use std::ffi::OsString;
        let home = || Some(OsString::from("/home/somebody"));
        for (target, expected) in [
            (&CLAUDE, "/home/somebody/.claude"),
            (&CODEX, "/home/somebody/.codex"),
        ] {
            assert_eq!(
                target.dir_from(None, home()),
                Ok(PathBuf::from(expected)),
                "{}: unset falls back to $HOME",
                target.name,
            );
            assert_eq!(
                target.dir_from(Some(OsString::from("/elsewhere/cfg")), home()),
                Ok(PathBuf::from("/elsewhere/cfg")),
                "{}: set relocates it",
                target.name,
            );
            assert_eq!(
                target.dir_from(Some(OsString::new()), home()),
                Ok(PathBuf::from(expected)),
                "{}: set to nothing is not set",
                target.name,
            );
            assert_eq!(
                target.dir_from(Some(OsString::from("cfg")), home()),
                Err(HookError::AmbiguousHome(
                    target.home_var.expect("both have one").to_owned()
                )),
                "{}: a relative one names no single file",
                target.name,
            );
            assert_eq!(target.dir_from(None, None), Err(HookError::NoHome));
        }
    }

    /// The limitation, asserted rather than assumed: a file that was not pretty-printed comes back
    /// pretty-printed. Byte identity is a claim about files in the layout the agents themselves
    /// write, and this is where it stops.
    #[test]
    fn a_compact_file_is_re_emitted_pretty_and_the_content_survives() {
        let fixture = Fixture::new(&CLAUDE, Some("{\"model\":\"opus\"}"));
        fixture.round_trip();
        let text = fixture.text().expect("a file");
        assert_ne!(text, "{\"model\":\"opus\"}");
        assert_eq!(
            serde_json::from_str::<Value>(&text).expect("still JSON"),
            serde_json::json!({ "model": "opus" })
        );
    }

    /// ONE TABLE, THREE READERS: what a launch's document says about an event is byte-for-byte what
    /// an INSTALL writes into the user's file for it.
    ///
    /// The gate R344's rule asks for. Two renderers now build sprag's hooks — one editing somebody's
    /// config, one composing a `--settings` document — and the failure mode of a second reader is
    /// not a crash: it is a document that parses perfectly and reports the wrong thing, or reports
    /// on a different set of events than the install does. Reading both out of the SAME fixture and
    /// comparing the entries is the only thing that keeps them one mechanism.
    ///
    /// It compares the ENTRY under each event rather than the whole file, because the two documents
    /// legitimately differ in one way: the file is the user's and holds their other keys, and the
    /// launch document is sprag's alone.
    #[test]
    fn a_launch_document_says_exactly_what_an_install_would_write() {
        let fixture = Fixture::new(&CLAUDE, None);
        let installed: Value =
            serde_json::from_str(&fixture.install(EXE)).expect("the installed file is JSON");
        let launch = CLAUDE
            .session_args(&[], Path::new(EXE))
            .expect("claude takes a per-launch document");
        assert_eq!(launch[0], "--settings", "the flag comes first: {launch:?}");
        let document: Value =
            serde_json::from_str(&launch[1]).expect("the launch document is JSON");

        for (event, _) in CLAUDE.events {
            assert_eq!(
                document["hooks"][event], installed["hooks"][event],
                "the two readers disagree about {event}",
            );
        }
        assert_eq!(
            document["hooks"]
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            installed["hooks"]
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            "and about WHICH events there are",
        );
    }

    /// A launch already carrying the flag is left exactly as its caller wrote it.
    ///
    /// Not politeness: two `--settings` on one command line is a precedence question the agent's
    /// manual does not answer, and sprag guessing at it would silently replace configuration
    /// somebody chose. The cost — that launch reports nothing — is what `Authority` exists to make
    /// visible.
    #[test]
    fn a_launch_that_brings_its_own_settings_keeps_them() {
        for argv in [
            vec!["claude", "--settings", "/home/me/mine.json"],
            // The spelling the first draft of this rule missed, found by asking the debt question of
            // the round's own code: a command line may join a flag to its value with `=`, and a
            // reader that knew only the separated form would append the second `--settings` this
            // refusal exists to prevent.
            vec!["claude", "--settings={\"hooks\":{}}"],
        ] {
            let argv = argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
            assert_eq!(
                CLAUDE.session_args(&argv, Path::new(EXE)),
                None,
                "{argv:?} already says what configures it",
            );
            assert!(launch_args_from(&argv, Path::new(EXE), |_| false).is_empty());
        }
    }

    /// What a launch carries is decided by the program, by its BASENAME, and by nothing else.
    ///
    /// The three answers in one place because they are one rule: an absolute path to an agent is
    /// that agent, an agent with no per-launch door is left to `install-hooks`, and everything else
    /// — which is nearly every pane ever opened — is launched untouched.
    #[test]
    fn only_a_recognised_agent_with_a_per_launch_door_carries_anything() {
        let carried = |argv: &[&str]| {
            launch_args_from(
                &argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
                Path::new(EXE),
                |_| false,
            )
        };
        assert_eq!(
            carried(&["/usr/local/bin/claude", "-p", "hello"]).first(),
            Some(&"--settings".to_owned()),
            "an agent named by its absolute path is still that agent",
        );
        assert!(
            carried(&["codex"]).is_empty(),
            "an agent with no per-launch door is left to install-hooks",
        );
        assert!(
            carried(&["/bin/sh", "-c", "claude"]).is_empty(),
            "a SHELL is not an agent, whatever it goes on to run"
        );
        assert!(carried(&["/bin/bash"]).is_empty());
        assert!(
            carried(&[]).is_empty(),
            "and a launch with no program at all"
        );
    }

    /// An agent whose OWN configuration already reports is not instrumented a second time.
    #[test]
    fn an_agent_that_already_reports_is_not_instrumented_twice() {
        let argv = ["claude".to_owned()];
        assert!(
            launch_args_from(&argv, Path::new(EXE), |target| target.name == "claude").is_empty(),
            "the user ran install-hooks; sprag adds nothing on top of it",
        );
        assert!(
            !launch_args_from(&argv, Path::new(EXE), |_| false).is_empty(),
            "and the control: with nothing installed the launch is instrumented",
        );
    }

    /// "Already reporting" is COMPLETE AND ABLE TO RUN, and the second half is what a `complete()`
    /// test alone would get wrong.
    ///
    /// Three fixtures, because the three answers must differ: nothing installed, a whole install
    /// whose binary is on disk, and the same install whose binary has since moved. A user in the
    /// third state has a config that says it is wired and an agent that reports nothing, and it is
    /// exactly the user sprag must still instrument.
    #[test]
    fn an_installed_hook_that_cannot_run_is_not_an_agent_that_reports() {
        for target in TARGETS {
            let fixture = Fixture::new(target, None);
            assert!(
                !status_at(target, fixture.path()).reporting(),
                "{}: nothing is installed",
                target.name,
            );

            let program = fixture.program();
            fixture.install(&program.display().to_string());
            assert!(
                status_at(target, fixture.path()).reporting(),
                "{}: every event is wired and the binary is there",
                target.name,
            );

            std::fs::remove_file(&program).expect("the binary moves away");
            let moved = status_at(target, fixture.path());
            assert!(
                moved.complete(),
                "{}: the file still says it is wired",
                target.name
            );
            assert!(
                !moved.reporting(),
                "{}: but nothing it names can run, so this agent reports nothing",
                target.name,
            );
        }
    }

    /// A sprag whose own path a shell would re-interpret still RUNS — measured through a shell,
    /// because that is what an agent does with an entry's `command`.
    ///
    /// The defect this closes was found by asking the debt question of this round's own code, and it
    /// was already live: `sh -c "/tmp/a dir/sprag hook claude"` exits 127 with *"/tmp/a: not
    /// found"*, and downstream nothing can tell that from an agent whose user never installed
    /// anything. It mattered more after per-launch instrumentation than before, because the path is
    /// no longer one a user typed into `install-hooks` — the daemon derives it from where it was
    /// built or installed, so nobody chose it and nobody would look at it.
    ///
    /// **Run rather than compared.** An assertion that the rendered string equals some expected
    /// quoting would be this test agreeing with the renderer; only handing it to `sh` says whether a
    /// shell reaches the program. Every one of the 35 tests here was green before the fix.
    #[test]
    fn a_hook_command_reaches_a_program_whose_path_a_shell_would_split() {
        let fixture = Fixture::new(&CLAUDE, None);
        let awkward = fixture.0.join("a dir");
        std::fs::create_dir_all(&awkward).expect("a directory whose name has a space");
        let program = awkward.join("sprag");
        let marker = fixture.0.join("it-ran");
        std::fs::write(
            &program,
            format!("#!/bin/sh\nprintf ok > '{}'\n", marker.display()),
        )
        .expect("a stand-in binary");
        std::fs::set_permissions(
            &program,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("make it executable");

        let command = CLAUDE.command(&program);
        let ran = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .status()
            .expect("the shell runs");
        assert!(
            ran.success() && marker.exists(),
            "a shell could not reach the program in {command:?}",
        );

        // ...and the entry it writes is still recognised as ours, which is what makes a re-install
        // correct it in place rather than adding a second one.
        assert_eq!(
            CLAUDE.program_of(&command).as_deref(),
            Some(program.display().to_string().as_str()),
            "the path must come back out of {command:?}",
        );
        assert!(CLAUDE.owns(&command));
    }

    /// The same path, all the way through an install: recognised, idempotent, and reported as a
    /// program that is really there.
    ///
    /// `Status::missing_program` is the reader that would go wrong the other way — a path it failed
    /// to unquote is a path it cannot find, so every install into a directory with a space would
    /// report itself broken.
    #[test]
    fn an_install_under_an_awkward_path_is_idempotent_and_reads_as_present() {
        for target in TARGETS {
            let fixture = Fixture::new(target, None);
            let awkward = fixture.0.join("a dir");
            std::fs::create_dir_all(&awkward).expect("a directory whose name has a space");
            let program = awkward.join("sprag");
            std::fs::write(&program, "").expect("a stand-in binary");

            let first = fixture.install(&program.display().to_string());
            let status = status_at(target, fixture.path());
            assert!(
                status.complete() && status.missing_program.is_none(),
                "{}: an installed path with a space is still a path that exists: {status:?}",
                target.name,
            );

            let second = fixture.install(&program.display().to_string());
            assert_eq!(
                first, second,
                "{}: a re-install must correct in place, not add a second entry",
                target.name,
            );
        }
    }
}
