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
    /// The flag this agent takes ONE launch's MCP servers on, when it has one.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a daemon hands its agent an MCP server at all (register item 444)
    ///
    /// [`session_flag`](Self::session_flag) is the same idea one surface over, and the argument
    /// there is the whole argument here: **an image should hand its agent ITS OWN sibling rather
    /// than trust whatever somebody installed.** A pane's hook is never stale because there is no
    /// second copy to keep in step — it is `sprag_bin()`, the sibling of the running daemon,
    /// written into the launch. sprag's agent-facing MCP server had no such treatment: it came from
    /// the user's own scope (`~/.claude.json`), so it was whatever was installed, whenever it was
    /// installed. Measured 2026-08-18 by asking the two binaries for their rosters — the installed
    /// one answered a fraction of what the tree serves, and **nothing anywhere could say so**: a
    /// verb the product HAS reads to an agent as *no such tool*, which reads as *the product cannot
    /// do this*.
    ///
    /// # ⚠⚠ It ADDS, and that is measured rather than assumed
    ///
    /// Measured against the real `claude` (2.1.234) on the box that wrote this, with stub servers
    /// so the answer could not be guessed from a roster:
    ///
    /// * a server this flag names under a key the user's own config also uses **wins** — a launch
    ///   inside a pane therefore reaches this image's server and not the installed one, which is
    ///   the entire point;
    /// * every server the user configured under **another** key survives untouched.
    ///
    /// That second half is why this type's `mcp_only_flag` is a refusal rather than something sprag
    /// passes, and **that too is measured rather than read off a help text**: the same launch with
    /// `--strict-mcp-config` added answered with sprag's server ALONE — the other server, which the
    /// arm above had just shown surviving, was gone. So the flag does not merely narrow what sprag
    /// contributes; it deletes what a person configured, and **a pane is not permission to do that.**
    ///
    /// `None` says this agent has no such door, which is codex's answer today for
    /// [`session_flag`](Self::session_flag)'s reason exactly — its per-run overrides are `-c
    /// key=value` over TOML, nobody has established that an MCP server can be spelled that way, and
    /// an unverified `Some` would find out at somebody's expense.
    mcp_flag: Option<&'static str>,
    /// The flag by which this agent's user says *use ONLY the MCP servers I named*, when it has one
    /// — read as a REFUSAL and never written.
    ///
    /// It is here because its presence is a decision sprag must not overrule. A launch carrying it
    /// has asked for an MCP environment holding exactly what its caller passed, and a launch
    /// carrying it with no `mcp_flag` beside it has asked for NONE. Injecting into either would
    /// answer a question its caller already answered — the rule
    /// [`session_args`](Self::session_args) follows about `--settings`, met one flag over.
    ///
    /// ⚠⚠⚠ **WHAT THE FLAG DOES IS MEASURED, and it is why sprag never sends it** — see `mcp_flag`'s
    /// second arm: adding it to an otherwise identical launch left the agent holding sprag's server
    /// and nothing else, the user's own server having vanished. Read as a refusal, it protects that
    /// person; sent, it would be sprag doing the deleting.
    ///
    /// ⚠ Deliberately a field rather than a word inside [`mcp_args`](Self::mcp_args): this type's
    /// premise is that a further agent is a further `const` rather than further code, and a spelling
    /// baked into the method would be claude's spelling imposed on every agent that came later.
    mcp_only_flag: Option<&'static str>,
    /// The flag this agent takes a caller-chosen SESSION IDENTITY on, when it has one.
    ///
    /// # ⚠⚠⚠ Why sprag names the session rather than finding out what it was called
    ///
    /// An agent files everything it records about a run — its transcript, and the per-request token
    /// counts a cost signal is denominated in — under a name of its own choosing, in a directory
    /// keyed by the cwd it started in. Recovering that name from outside is three inferences
    /// (live cwd, to a directory, to the newest file in it) and **each one fails by silently reading
    /// a different session rather than by failing.** Naming it first replaces all three with a
    /// lookup: the file is called what sprag called it.
    ///
    /// The rule is `claudedocs/INSIGHT-LOOP-SCORING-AND-COST-SIGNALS.md`'s, one level down: an identity
    /// must be minted rather than recovered, or *"did we do this twice"* cannot be asked.
    ///
    /// # ⚠⚠ It is MINTED PER BIRTH, and that is what makes it safe
    ///
    /// Measured: a second launch carrying an id already in use is refused outright — `Error: Session
    /// ID … is already in use.` So an identity must never be *replayed as a NAME*: this module is
    /// consulted by [`crate::pane_args_source`] at every pane birth, a pane's recorded argv is
    /// captured BEFORE instrumentation, and a respawn therefore re-enters here and is named afresh.
    /// That is the same reason the instrumentation itself is not stored — a stored one *"would point
    /// a fresh agent at a dead socket"*.
    ///
    /// ⚠⚠⚠ **THIS PARAGRAPH USED TO END *«so an identity must never be STORED and replayed»*, and
    /// that was one word too strong.** What the refusal forbids is claiming a name that is in use;
    /// it says nothing about RE-ENTERING one. A durability restore does exactly that, through
    /// [`resume_flag`](Self::resume_flag): the process holding the name is gone, its transcript is
    /// still on disk, and resuming is the only way the work survives a daemon that had to be
    /// replaced to adopt new code. So the identity IS stored — in
    /// [`Pane::agent_session`](sprag_terminal::Pane::agent_session), beside the recorded argv and
    /// deliberately not in it, because the argv is what a REPLACEMENT re-runs and a replacement must
    /// still be named afresh. **Storing and replaying are two acts, and only one of them was ever
    /// refused.**
    ///
    /// `None` for an agent with no such door, which is codex today: it is not enough that a flag
    /// exists, the record it names has to be findable, and nobody has established codex's.
    identity_flag: Option<&'static str>,
    /// The flag this agent RE-ENTERS a session it has already been given by, when it has one.
    ///
    /// [`identity_flag`](Self::identity_flag)'s opposite number, and the pair is exclusive by
    /// construction: one CLAIMS a name for a new conversation, the other JOINS the conversation that
    /// name already has. `identity_args` refuses to mint when it sees this flag, so a launch carrying
    /// a resume is instrumented and not renamed.
    ///
    /// The one caller is a durability RESTORE. Everything else that starts an agent wants a fresh
    /// one — a person opening a pane, and above all `ai_loop.scxml`'s `restarting`, which replaces
    /// its inner session precisely to throw the accumulated context away.
    ///
    /// ⚠ `None` wherever [`identity_flag`](Self::identity_flag) is `None`, and not by coincidence:
    /// with no name minted there is no name recorded, so there would be nothing to resume. An agent
    /// that gains one gains both, in the same round, against the same measurement.
    resume_flag: Option<&'static str>,
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
    // Verified against `claude --help` on the box that wrote this: "--mcp-config <configs...> —
    // Load MCP servers from JSON files or strings", and then against the agent itself: a stub
    // server passed this way under the key the machine's own config already used was the one the
    // agent got, and a differently-named server from that config was still there beside it.
    mcp_flag: Some("--mcp-config"),
    // Read from the same `--help`: "--strict-mcp-config — Only use MCP servers from --mcp-config".
    // Named here so a launch that carries it is left alone; sprag never passes it.
    mcp_only_flag: Some("--strict-mcp-config"),
    // Verified against `claude --help` and then against a live session: "--session-id <uuid> — Use
    // a specific session ID for the conversation (must be a valid UUID)". The record it writes is
    // named for it — `~/.claude/projects/<dir>/<uuid>.jsonl` — which is the whole reason this field
    // exists, and it is fixed by the live gate
    // `a_minted_session_identity_names_the_record_a_live_agent_writes`.
    identity_flag: Some("--session-id"),
    // Verified against `claude --help` on the box that wrote this: "-r, --resume [value] — Resume a
    // conversation by session ID, or open interactive picker with optional search term". The VALUE
    // form is what a restore needs; the bare form opens a picker at a pane nobody is watching.
    resume_flag: Some("--resume"),
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
        //
        // ⚠⚠⚠ THE ONE ROW THIS TABLE DOES NOT DECIDE ALONE. A notice carries its own KIND, and
        // exactly one of those kinds — [`IDLE_NOTICE`] — means the opposite of this word. See
        // [`report_for`]: every other kind, and an absent one, is answered here.
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
    // `None` for `session_flag`'s reason, one surface over: codex's per-run override is `-c
    // key=value` parsed as TOML, and whether an MCP server handed to it that way is one codex
    // actually starts has not been run. A `Some` nobody measured would put a flag on somebody's
    // editing session in exchange for a server that may never be spawned.
    mcp_flag: None,
    // `None` wherever `mcp_flag` is, and not by coincidence: this field exists to refuse an
    // injection, and an agent that is never injected into has nothing to refuse.
    mcp_only_flag: None,
    // See `identity_flag`. A flag that names a session is not enough on its own — what sprag needs
    // is the RECORD that name reaches, and nobody has established where codex files one or whether
    // it can be named from outside. An unverified `Some` here would put a flag on somebody's
    // session in exchange for a lookup that finds nothing.
    identity_flag: None,
    // `None` because the line above is: nothing names a codex session, so nothing records one, so
    // there is nothing here to re-enter. The two move together or a restore would resume a name that
    // was never written down.
    resume_flag: None,
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

/// The key sprag's own MCP server takes in the roster of an agent this daemon launched — see
/// [`Target::mcp_args`].
///
/// ⚠⚠⚠ **It is the same word a person's own installed entry uses, and that is the point rather
/// than a collision to be avoided.** Measured against the real agent: a server passed on the launch
/// under a key the user's config also holds is the one the agent gets. So a pane's agent reaches
/// the server of the image that made its pane even on the machine that HAS a stale install — which
/// is the only machine the injection matters on. A distinct key would leave both in the roster,
/// with two spellings of every verb and nothing to say which one answers about this daemon.
///
/// ⚠ The residue, stated rather than hidden: a person who deliberately pointed their own `sprag`
/// entry somewhere else does not get it inside a sprag pane. Everywhere else they do — the
/// injection is per-launch, so it reaches exactly the agents this daemon starts — and inside a pane
/// the daemon's own sibling is the answer their entry was trying to be.
pub const MCP_SERVER: &str = "sprag";

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

    /// The arguments that hand ONE launch of this agent the MCP server at `server` — sprag's own
    /// agent-facing surface, taken from the image that is making this pane.
    ///
    /// `None` when the launch must be left exactly as its caller wrote it, and the refusals are the
    /// substance:
    ///
    /// * this agent has no per-launch MCP door — this type's `mcp_flag` is `None`;
    /// * the argv already carries that flag, in either of the two spellings a command line has for
    ///   one. The caller has said what MCP servers this launch has; a second copy is a precedence
    ///   question no agent's manual answers the same way twice — [`session_args`](Self::session_args)'
    ///   rule, and it is one rule because it is one mistake;
    /// * the argv carries this type's `mcp_only_flag`. That flag says *only what I named*, so a
    ///   launch carrying it has asked for a stated MCP environment — and one carrying it alone has
    ///   asked for an empty one. Adding to either overrules a decision somebody made;
    /// * `server` is not UTF-8. It travels as a JSON string, and a lossy conversion would name a
    ///   DIFFERENT file — the one failure mode worse than not injecting at all.
    ///
    /// # What the document says, and what it deliberately does not
    ///
    /// One server, under [`MCP_SERVER`], naming an absolute program and nothing else. It does not
    /// pass an environment: a pane's child already carries [`crate::PANE_ENV_VAR`] and the daemon's
    /// address ([`crate::pane_env_source`]), the agent inherits them, and the server the agent
    /// spawns inherits them in turn — so the server reaches the daemon that made the pane by the
    /// same route everything else in that pane does. Naming them here would be a second copy of
    /// that publication, free to drift.
    ///
    /// ⚠ It is a JSON STRING rather than a file for `session_args`' reason: a launch leaves nothing
    /// on disk to outlive the agent, be cleaned up after a killed daemon, or be readable by whoever
    /// the agent runs as.
    #[must_use]
    pub fn mcp_args(&self, argv: &[String], server: &Path) -> Option<Vec<String>> {
        let flag = self.mcp_flag?;
        let settled = |name: &str| {
            let joined = format!("{name}=");
            argv.iter()
                .any(|arg| arg == name || arg.starts_with(&joined))
        };
        if settled(flag) || self.mcp_only_flag.is_some_and(settled) {
            return None;
        }
        let command = server.to_str()?;
        Some(vec![
            flag.to_owned(),
            serde_json::json!({
                "mcpServers": { MCP_SERVER: { "type": "stdio", "command": command } },
            })
            .to_string(),
        ])
    }

    /// The arguments that NAME one launch of this agent, so what it records about itself can be
    /// found again — see this type's `identity_flag`.
    ///
    /// `None` when the launch must be left to name itself: an agent with no such flag, or an `argv`
    /// that has already settled the question. **The refusals are the substance here**, and each is a
    /// different sentence:
    ///
    /// * the caller already passed `--session-id`. They said which session this is; a second copy is
    ///   a precedence question, and sprag's answer would silently win over a person's.
    /// * the caller passed `--resume`, `--continue` or `--fork-session`. Those name a session by
    ///   CONTINUING one, so a minted name is not merely redundant — it contradicts the argument
    ///   beside it, and what an agent does with a contradiction is its business rather than
    ///   something to find out on somebody's editing session.
    ///
    /// ⚠ Separate from [`session_args`](Self::session_args) rather than folded into it, because the
    /// two refuse independently: a launch whose own config already reports still wants naming, and a
    /// launch that is resuming still wants instrumenting. Folding them would make each one's refusal
    /// suppress the other's answer, which is a bug shaped exactly like a missing feature.
    ///
    /// `mint` is injected for `launch_args_from`'s reason — a decision should be provable without
    /// the randomness it consumes.
    #[must_use]
    pub fn identity_args(&self, argv: &[String], mint: impl Fn() -> String) -> Option<Vec<String>> {
        let flag = self.identity_flag?;
        let settled = |name: &str| {
            let joined = format!("{name}=");
            argv.iter()
                .any(|arg| arg == name || arg.starts_with(&joined))
        };
        if settled(flag)
            || ["--resume", "-r", "--continue", "-c", "--fork-session"]
                .iter()
                .any(|other| settled(other))
        {
            return None;
        }
        Some(vec![flag.to_owned(), mint()])
    }

    /// The conversation `argv` NAMES — read from whichever of this agent's two naming flags it
    /// carries, so a launch that was named and a launch that RESUMED a name answer alike.
    ///
    /// Both are read because a restore produces the second and a chained restore has to find it
    /// again: a pane that came back resuming `X` is still in conversation `X`, and a reader that
    /// only knew the minting flag would let the name evaporate on the second restart.
    #[must_use]
    pub fn named_session(&self, argv: &[String]) -> Option<String> {
        [self.identity_flag, self.resume_flag]
            .into_iter()
            .flatten()
            .find_map(|flag| sprag_plugin::identity_in(argv, flag))
    }

    /// What to add to a launch so it RE-ENTERS `session` instead of being named afresh — `None` when
    /// this agent has no such door, or when `argv` already settles the question.
    ///
    /// The refusals mirror [`identity_args`](Self::identity_args)'s, and for the same reason: a
    /// caller who already said which conversation this is has said it, and a second answer would be
    /// sprag's silently winning over theirs.
    #[must_use]
    pub fn resume_args(&self, argv: &[String], session: &str) -> Option<Vec<String>> {
        let flag = self.resume_flag?;
        if session.is_empty() {
            return None;
        }
        let settled = |name: &str| {
            let joined = format!("{name}=");
            argv.iter()
                .any(|arg| arg == name || arg.starts_with(&joined))
        };
        if [self.identity_flag, self.resume_flag]
            .into_iter()
            .flatten()
            .any(settled)
        {
            return None;
        }
        Some(vec![flag.to_owned(), session.to_owned()])
    }
}

/// The conversation a LAUNCHED argv is in, or `None` for everything that is not a named agent —
/// what [`crate::pane_identity_source`] answers with, and the one part of an instrumented launch a
/// durability snapshot keeps.
///
/// Shown the argv AFTER instrumentation, because the name is something the instrumenting added.
#[must_use]
pub fn launched_identity(argv: &[String]) -> Option<String> {
    agent_of(argv).and_then(|target| target.named_session(argv))
}

/// What to add to a RESTORED launch so it re-enters `session` — empty for a pane that is not a named
/// agent, for an agent with no resume door, or for an argv that already names its own conversation.
///
/// [`launch_args`]'s counterpart on the restore path: that one says *report your turns through this
/// daemon*, this one says *and it is THIS conversation you are continuing*. They compose because
/// `identity_args` stands down when it sees a resume — so a restored agent is instrumented afresh
/// and named not at all.
#[must_use]
pub fn resume_args(argv: &[String], session: &str) -> Vec<String> {
    agent_of(argv)
        .and_then(|target| target.resume_args(argv, session))
        .unwrap_or_default()
}

/// The agent `argv` launches, by its program's basename — [`launch_args_from`]'s first step, shared
/// so the three readers of an argv cannot disagree about what it is running.
fn agent_of(argv: &[String]) -> Option<&'static Target> {
    argv.first()
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .and_then(target)
}

/// A v4 UUID from the kernel's randomness — the name sprag gives one agent session.
///
/// # ⚠⚠ Why this is here and public rather than private to its caller
///
/// So the live gate that measures the claim mints exactly as the product does. **A fixture's reader
/// must be the product's reader** (R383), and a fixture with its own id generator would be proving
/// that ITS ids reach a record.
///
/// No `uuid` dependency for sixteen bytes and a format string; the only property required is that
/// the agent accepts it, which is `must be a valid UUID`.
///
/// ⚠ An unreadable `/dev/urandom` yields the nil UUID rather than a panic. The safe direction: a
/// launch is never lost over a name, and a nil id is refused by the agent the second time it is
/// used, which surfaces as a failed birth rather than as two sessions sharing a record.
#[must_use]
pub fn mint_session_id() -> String {
    use std::io::Read as _;

    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut urandom| urandom.read_exact(&mut bytes))
        .is_err()
    {
        bytes = [0u8; 16];
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32],
    )
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

/// **WHERE A HOOK LEAVES WORD THAT IT COULD NOT REPORT** — one file per pane, whose EXISTENCE is
/// the whole message.
///
/// # ⚠⚠⚠ Why silence needed a breadcrumb at all, measured
///
/// A hook swallows every failure and always exits 0, and that rule is right for the world it was
/// written for: this runs inside EVERY session of the agent, including ones in a terminal that has
/// nothing to do with sprag, and a multiplexer that makes somebody's agent print errors because its
/// own daemon is down is not shippable.
///
/// ⚠⚠ **BUT THE CODE ALREADY DRAWS THAT LINE AND THE RULE IGNORED IT.** A stranger's session never
/// resolves [`crate::PANE_ENV_VAR`] — it is not in a pane. So a failure AFTER that variable has
/// been read is not a stranger's, it is sprag's own, and swallowing it is what cost an hour on
/// 2026-08-16: the loop bumped `WIRE_PROTOCOL` 35 → 36 and the rebuild replaced the hook binary
/// (it is HARDLINKED to `target/debug/sprag`), while the daemon stayed at 35. Every report was
/// refused at `client/hello` with nobody able to see it, the last state it managed to say —
/// `working` — outranks the screen and never expires, and the turn could not end. The pane held
/// `MILESTONE REACHED` for over an hour while the journal repeated *looked, nothing had happened*.
///
/// ⚠⚠⚠ **THE SAME SKEW ON A CLI TAKES FIVE MINUTES TO DIAGNOSE**, because the daemon's refusal
/// names the problem AND the fix on stderr. A hook's stderr goes nowhere. This is register item 281
/// — *the product's best diagnostic sentence is the one it hides* — one client over.
///
/// # ⚠⚠ Why a file, and why its existence rather than its contents
///
/// The daemon is by definition unreachable when this is written, so the breadcrumb cannot be a
/// report. It is read by whoever asks about the pane later, over a client that DOES match. Written
/// on failure and REMOVED on success, so a file that is there means *this pane's reporter is
/// currently mute* — a health fact about the reporter, never a state of the agent. Nothing here may
/// ever be read as an agent's state: that is what the refused report was for.
#[must_use]
pub fn hook_trouble_path(pane: u64) -> PathBuf {
    crate::durability::state_dir().join(format!("hook-mute.{pane}"))
}

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
    // ⚠⚠⚠⚠⚠ AN IDLE NAG IS THE PANE'S REST, AND IT IS THE ONE REPORT THAT ARRIVES WHEN NO TURN
    // BOUNDARY DOES — see [`IDLE_NOTICE`] for both measurements this rests on. The table answers
    // `blocked` for every notice because the event NAME is all it reads; the payload says which KIND
    // of notice it is, and this kind says the agent is waiting for input with nothing in flight,
    // which is [`AgentState::Idle`] in this crate's own words.
    //
    // Reporting `blocked` here was the defect: a false `blocked` sends an unattended run through
    // `screening` and `awaiting_human` to the `<final>` `blocked`. Reporting NOTHING was the half-fix
    // that followed it, and it threw away the only evidence a pane whose turn DIED ever produces —
    // register item 458. **Measured 2026-08-19 on a live pane whose turn ended without its `Stop`
    // ever reaching this daemon: the nag was the only thing that spoke.**
    if is_idle_notice(payload) {
        return Some(Outcome::Report(AgentState::Idle));
    }
    let event = payload.get("hook_event_name")?.as_str()?;
    target
        .events
        .iter()
        .find(|(name, _)| *name == event)
        .map(|(_, outcome)| *outcome)
}

/// **WHAT A SUBMIT PAYLOAD STATES ABOUT THE TURN IT OPENS** — the two facts a screen cannot supply.
///
/// # ⚠⚠⚠⚠ Why this exists at all: the program was already talking and nothing listened
///
/// [`report_for`] reduces a whole payload to one word — *working* — and the rest is dropped. Two of
/// the things dropped are the answers to questions this workspace has spent rounds failing to
/// obtain from a terminal:
///
/// * **`prompt`** is the agent's own statement of what it was asked. Delivery has been confirmed by
///   hunting a fragment of the typed text on the pane's SCREEN, and every failure of that oracle
///   bought another predicate — 40 chars became 40 COLUMNS, the head became the TAIL, an exact
///   match became a whitespace-insensitive one — while item 223's gate already recorded that
///   tightening it is ruled out and that the answer is *"evidence from the PROGRAM rather than the
///   screen"*. **This is that evidence**, and it settles the question the screen cannot: a composer
///   that concatenated somebody else's text reports a prompt that is not the one that was sent.
/// * **`transcript_path`** is where the agent is writing. The spend reader resolves that path from
///   a session id and has been measured answering 0 for a session whose transcript exists (register
///   item 431) — **the agent states it outright.**
///
/// ⚠⚠ CAPTURED, not inferred: a real `claude` 2.1.233 was run with a recording hook and the payload
/// carried `session_id`, `transcript_path`, `cwd`, `prompt_id`, `permission_mode`,
/// `hook_event_name` and `prompt`. The gate below uses that capture as its fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The prompt the agent says it received, verbatim.
    pub prompt: String,
    /// Where the agent says it is writing this session's transcript.
    ///
    /// [`None`] where the payload does not carry one: this is a fact to USE when offered and never
    /// one to demand, because an agent that reports its turn honestly while writing no transcript
    /// is a working agent, not a broken one.
    pub transcript: Option<PathBuf>,
}

/// What `payload` STATES, for the one event that opens a turn — or [`None`] for anything else.
///
/// ⚠⚠⚠ **IT JUDGES NOTHING**, which is what keeps it beside [`report_for`] rather than inside it:
/// that one answers *what state does this put the agent in*, a decision this crate owns, and this
/// one answers *what did the agent say*, which is the agent's to state and nobody else's to infer.
/// A reader that did both would have to be consulted about a state it has no business deciding.
///
/// ⚠ A submit with no `prompt` is [`None`] rather than an empty one: *the agent was asked nothing*
/// is a claim, and a payload that omits the key has not made it.
#[must_use]
pub fn asked_in(payload: &Value) -> Option<Asked> {
    if payload.get("hook_event_name")?.as_str()? != SUBMIT_EVENT {
        return None;
    }
    // A subagent's turn is not the pane's turn — the same exclusion `report_for` opens with, and
    // for the same reason: what a sub-agent was asked says nothing about the prompt this pane took.
    if payload
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return None;
    }
    Some(Asked {
        prompt: payload.get("prompt")?.as_str()?.to_owned(),
        transcript: payload
            .get("transcript_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
    })
}

/// The event that opens a turn, named once — [`asked_in`]'s subject and a row in every
/// [`Target::events`] table below.
const SUBMIT_EVENT: &str = "UserPromptSubmit";

/// The event that ENDS a turn, named once — [`said_in`]'s subject, and a row in both
/// [`Target::events`] tables where it means *the agent is at rest*.
const REST_EVENT: &str = "Stop";

/// **WHAT THE AGENT SAYS IT ANSWERED**, off the one event that ends a turn — or [`None`] anywhere
/// else.
///
/// # ⚠⚠⚠⚠⚠ Why a driver needs this and cannot get it from the pane
///
/// [`asked_in`] is the other end of the same turn, and this is the half register item 441 spent six
/// rounds failing to read off a screen. What was measured (2026-08-18, on the running daemon, both
/// readers at one instant): a `claude` pane's whole logical-line count stood at **37 and never
/// moved** while the agent wrote reply after reply, so every read since any mark answered **0
/// complete lines** — honestly, with nothing lost and no restart to report. A full-screen agent
/// holds the alternate screen and REPAINTS, so nothing is shed; once its composer settles, the
/// address cannot advance again. The judge went deaf for the rest of the run, and the reading it
/// got — *nothing was produced* — is the same reading a peer that truly said nothing produces.
///
/// **The agent states it outright.** `Stop` carries `last_assistant_message`: the final message of
/// the turn, as the program itself has it, before any terminal has drawn a cell of it.
///
/// ⚠⚠ CAPTURED, not inferred — the fixture is `captured_rest` in this module's tests (⚠ NAMED
/// rather than linked: a `#[cfg(test)]` item is not in scope for rustdoc, and the doc gate refuses
/// the link). A real `claude` 2.1.234, recorded by
/// putting a logging wrapper at the path the agent's own configuration names, so every payload was
/// written down and still handed on. The same capture is what settled that `Stop` fires **once per
/// TURN and not once per message** (fifteen tool calls in one turn raised exactly one), which is
/// the premise this reader's *the turn ended* meaning rests on.
///
/// ⚠ A `Stop` with no `last_assistant_message` is [`None`] rather than an empty statement, for
/// [`asked_in`]'s reason one end over: *the agent answered nothing* is a claim, and a payload that
/// omits the key has not made it.
#[must_use]
pub fn said_in(payload: &Value) -> Option<String> {
    if payload.get("hook_event_name")?.as_str()? != REST_EVENT {
        return None;
    }
    // A subagent's answer is not the pane's answer — the same exclusion `report_for` and `asked_in`
    // open with. `SubagentStop` is excluded by being absent from `Target::events`; this excludes a
    // `Stop` that a sub-agent raised.
    if payload
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return None;
    }
    Some(payload.get(REST_STATEMENT)?.as_str()?.to_owned())
}

/// What the agent calls its own closing message inside a [`REST_EVENT`] payload.
///
/// ⚠ Its spelling is the AGENT's, not ours — the wire key this ends up under is
/// [`crate::wire::AGENT_SAID_KEY`], and the two are deliberately not the same word: one is a
/// schema this crate reads and the other is a name it publishes.
const REST_STATEMENT: &str = "last_assistant_message";

/// The event an agent raises when it wants a person's attention — see [`NOTICE_KIND`] for why the
/// event NAME alone is not enough to say what it wants them for.
const NOTICE_EVENT: &str = "Notification";

/// **THE AGENT'S OWN CLASSIFICATION OF WHY IT RAISED A NOTICE**, inside a [`NOTICE_EVENT`] payload.
///
/// # ⚠⚠⚠⚠⚠ The field this product was throwing away, and what that cost
///
/// [`report_for`] read the event NAME and answered [`AgentState::Blocked`] for every notice, on the
/// reasoning — written in `CLAUDE`'s own table — that *"the agent has asked the human something …
/// the agent already tells us"*. It does tell us, and it tells us MORE than that word carries.
///
/// **Captured live 2026-08-19**, twice, with a logging wrapper at the path the agent's own
/// configuration names:
///
/// ```text
/// {"hook_event_name":"Notification",
///  "message":"Claude is waiting for your input",
///  "notification_type":"idle_prompt", …}
/// ```
///
/// Both captures arrived AFTER that turn's `Stop`, with the agent's answer already stated
/// (`said` had moved). **So the pane was at rest and this product reported it as blocked on a
/// question nobody could read** — which is the exact sentence a run then carries into `screening`,
/// `awaiting_human` and, for a run told nobody is watching, the `<final>` `blocked`.
///
/// ⚠⚠⚠ **ONLY [`IDLE_NOTICE`] IS SPECIAL-CASED, AND THAT IS A MEASUREMENT BOUNDARY RATHER THAN A
/// DESIGN.** A permission dialog surely carries some other value here, and this round did not
/// manage to raise one — the probe's agent answered in prose instead of reaching for a tool. So
/// every other value, and an absent field, keep the meaning they have always had. **Naming a word
/// nobody has seen is the mistake this module's own history records paying for.**
const NOTICE_KIND: &str = "notification_type";

/// What the agent calls the notice's prose, inside a [`NOTICE_EVENT`] payload.
const NOTICE_TEXT: &str = "message";

/// The one [`NOTICE_KIND`] this crate has measured a MEANING for: the agent has nothing in flight
/// and would like another prompt.
///
/// # ⚠⚠⚠⚠⚠ It is the pane's REST, and it is the only thing that speaks when no turn boundary does
///
/// Every other row of [`Target::events`] is a TURN BOUNDARY — a submit, a tool call, a stop — so a
/// turn that DIED or was INTERRUPTED raises none of them, and the last thing the pane said stands
/// for ever. **Register item 458 is that class, and it was seen twice in one day**: a turn killed by
/// an API `529` left `blocked` behind it and a FRESH run waited on that report for six minutes; a
/// turn a person interrupted with Escape left `working` behind it and its driver polled *"looked,
/// nothing had happened"* against a turn that could never end. Both were ended by a HUMAN, which is
/// the whole complaint — nothing expires a report, because the only thing that could speak is the
/// agent and these are exactly the cases where it does not.
///
/// This notice is what speaks anyway. It is raised by the agent's own IDLENESS TIMER and not by
/// anything about a turn, so it arrives precisely where the boundaries do not — which is what makes
/// it worth reading rather than a seventh boundary that would go missing with the rest.
///
/// **Measured end to end 2026-08-19**, with a live agent whose `Stop` was dropped on its way to the
/// daemon — item 344's ordinary case, where a rebuilt or refused reporter leaves the pane's last
/// `working` standing *"for ever, because the thing that would have said otherwise can no longer
/// speak"*. The pane read `working` with the turn already over, and this notice arrived **60.02 s
/// later and moved it to `idle`**. That is what this row buys: a lost boundary stops being permanent.
///
/// # ⚠⚠⚠⚠⚠ What it does NOT reach, measured in the same session
///
/// **A turn a person INTERRUPTS with Escape is not covered, and cannot be from here.** Escape
/// restores the prompt into the COMPOSER, and this notice is suppressed while the composer holds
/// text — so an interrupted turn emits **no payload of any kind**: no `Stop`, and no nag either. A
/// pane measured this way stayed `working seq=6` for **fourteen minutes**, and clearing the composer
/// by hand did not re-arm the timer. There is nothing left for a report to expire against, so that
/// half of register item 458 belongs to the WAIT that depends on the report and not to this table.
/// Reading it off the screen instead is what 441 and 452 rule out.
///
/// # ⚠⚠⚠⚠ Why `idle` is the safe reading here, MEASURED rather than reasoned
///
/// A false `idle` is the worst answer this table can give. A run that believes it types its next
/// prompt into whatever the pane is showing, and if that is a numbered menu the keystroke SELECTS —
/// so the claim was measured twice, once against the agent's own image (`claude 2.1.235`) and once
/// live:
///
/// * **A dialog carries a DIFFERENT word.** The kinds are a closed set in that image, and the
///   permission dialog's is `permission_prompt` — beside `worker_permission_prompt`,
///   `agent_needs_input`, `agent_completed`, `elicitation_*`, `quota_auto_resume_*`,
///   `push_notification`, `computer_use_*` and `auth_success`. This word is not among them.
/// * **And the agent SUPPRESSES this notice while a dialog is open**: its trigger is an idleness
///   effect guarded on the dialog store being empty, at `messageIdleNotifThresholdMs` = 60_000.
///
/// **Captured live 2026-08-19**: an edit-permission dialog was left standing for **155 seconds**,
/// 2.6× that threshold, and raised `permission_prompt` and no idle notice at all. In the same
/// session an idle notice arrived with no `Stop` anywhere before it, which is the other half — the
/// nag does not wait for a turn to have ended tidily.
///
/// ⚠⚠ **ONLY THIS ONE WORD IS READ.** Every other value, and an absent field, keeps the meaning the
/// event NAME has always had — `blocked`, which is the right answer for `permission_prompt` and the
/// safe one for a word whose meaning nobody here has measured. Naming a word nobody has SEEN is the
/// mistake this module's history records paying for, and acting on a word whose MEANING was only
/// guessed is that mistake one step in.
const IDLE_NOTICE: &str = "idle_prompt";

/// **WHAT AN AGENT SAID WHEN IT ASKED FOR ATTENTION** — the notice's own words, or [`None`] for a
/// payload that is not one.
///
/// It travels the way [`said_in`] and [`asked_in`] do, and for the same reason: the alternative is
/// a host that knows a peer wants something and cannot say what. `awaiting_human`'s report reads
/// *"the peer is blocked on something this host cannot read as a numbered menu"* — true, and the
/// peer had said what it was in a field one layer up.
///
/// ⚠ A subagent's notice is not the pane's, the exclusion [`report_for`] and [`said_in`] both open
/// with.
#[must_use]
pub fn noticed_in(payload: &Value) -> Option<String> {
    if payload.get("hook_event_name")?.as_str()? != NOTICE_EVENT {
        return None;
    }
    if payload
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return None;
    }
    Some(payload.get(NOTICE_TEXT)?.as_str()?.to_owned())
}

/// Whether this payload is the agent saying it is merely IDLE — see [`IDLE_NOTICE`].
fn is_idle_notice(payload: &Value) -> bool {
    payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .is_some_and(|event| event == NOTICE_EVENT)
        && payload
            .get(NOTICE_KIND)
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == IDLE_NOTICE)
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
pub fn launch_args(argv: &[String], exe: &Path, mcp: Option<&Path>) -> Vec<String> {
    launch_args_from(
        argv,
        exe,
        mcp,
        |target| already_reports(status(target)),
        mint_session_id,
    )
}

/// Whether an agent's OWN config already reports, from a reading of this machine that may have
/// failed — and **what sprag does when it could not tell**.
///
/// # The decision, which used to be an `is_ok_and` and nobody's
///
/// [`status`] resolves a path before it reads anything, and that resolution has its own failures:
/// no absolute `$HOME` ([`HookError::NoHome`]), or an agent config-directory variable set to a
/// RELATIVE path ([`HookError::AmbiguousHome`]), which sprag refuses to guess at because the agent
/// resolves it against a directory this process is not in.
///
/// **An unreadable machine answers `false`, so the launch IS instrumented.** That is the safe
/// direction of the two, and it is a choice rather than a fallout:
///
/// * answering `false` costs, at worst, a DOUBLE report — the user's own install and this launch's
///   both firing. The level is idempotent, so what they see is right and what it costs is two
///   processes per event.
/// * answering `true` costs, at worst, NO report at all, silently, for a user whose supervision
///   looks configured. Nothing downstream can tell that from an agent that simply had no turns.
///
/// A supervisor that says too much is a nuisance; one that says nothing is indistinguishable from a
/// quiet agent. **The residue is named rather than hidden**: if that unresolvable directory really
/// does hold sprag's hooks, this double-reports, and `Authority` is where a reader sees it.
fn already_reports(read: Result<Status, HookError>) -> bool {
    read.is_ok_and(|status| status.reporting())
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
    mcp: Option<&Path>,
    already_reports: impl Fn(&'static Target) -> bool,
    mint: impl Fn() -> String,
) -> Vec<String> {
    // The PROGRAM decides, by its basename, so `/usr/local/bin/claude` and `claude` are one agent
    // and `sh -c claude` is not: an argv sprag did not write is one whose words it cannot read, and
    // appending a flag to a shell's would hand the shell an argument meant for something else.
    let Some(target) = agent_of(argv) else {
        return Vec::new();
    };
    // ⚠⚠ THREE DECISIONS, ASKED SEPARATELY. Instrumentation says *report your turns through this
    // daemon*; identity says *and this is what this session is called*; the server says *and these
    // are the verbs of the image you are running inside*. They refuse for unrelated reasons — see
    // `Target::identity_args` and `Target::mcp_args` — so a launch may take any of them, all, or
    // none, and the one that is refused must not take the others down with it.
    let mut extra = if already_reports(target) {
        Vec::new()
    } else {
        target.session_args(argv, exe).unwrap_or_default()
    };
    extra.extend(target.identity_args(argv, mint).unwrap_or_default());
    // ⚠ `None` is THIS IMAGE HAS NO SIBLING TO HAND OVER, and it is silence rather than a fallback
    // on purpose — see `crate::mcp_beside`, which is where that decision is made and argued.
    extra.extend(
        mcp.and_then(|server| target.mcp_args(argv, server))
            .unwrap_or_default(),
    );
    extra
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

    /// Where the MCP server beside it is pretended to live — a SIBLING of [`EXE`], because that is
    /// the only relationship [`Target::mcp_args`] is ever handed, and a fixture that put it
    /// somewhere else would gate the rule against a case the daemon cannot produce.
    const MCP: &str = "/usr/local/bin/sprag-mcp";

    /// **THE PAYLOAD A REAL AGENT SENT**, captured 2026-08-17 from `claude` 2.1.233 by installing a
    /// hook whose whole body was `cat > payload.json` and asking it one question.
    ///
    /// ⚠⚠⚠ A CAPTURE RATHER THAN A HAND-WRITTEN OBJECT, and the difference is the point: every
    /// other fixture in this module states what this crate BUILDS, and this one states what the
    /// agent SENDS. A payload invented here would gate this reader against my belief about the
    /// agent's schema, which is exactly the belief that needs checking — the two facts below were
    /// assumed absent for rounds while they were arriving on every turn.
    ///
    /// Verbatim but for the values, which are shortened; the KEYS and their shapes are as captured.
    fn captured_submit() -> Value {
        serde_json::json!({
            "session_id": "2987c0d6-b456-4847-8a90-8e4d701d97a1",
            "transcript_path": "/home/coin/.claude/projects/-tmp-probe/2987c0d6.jsonl",
            "cwd": "/tmp/probe",
            "prompt_id": "c47d7f47-933f-469e-bd8e-efc61818894f",
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "reply with the single word: pong",
        })
    }

    /// **THE GATE FOR THE EVIDENCE A SCREEN CANNOT GIVE** — the agent states what it was asked and
    /// where it is writing, and until now both were dropped on the floor.
    ///
    /// See [`Asked`] for what each is worth. The claims here are deliberately about the CAPTURE:
    /// what this reader must survive is the agent's schema, not this crate's idea of it.
    #[test]
    fn a_submit_payload_states_the_prompt_and_the_transcript_it_is_writing() {
        let asked = asked_in(&captured_submit()).expect("a submit states what it was asked");
        assert_eq!(
            asked.prompt, "reply with the single word: pong",
            "⚠⚠⚠⚠ THE PROMPT IS THE AGENT'S OWN STATEMENT OF WHAT IT RECEIVED — the evidence item \
             223's gate says the screen cannot give. It is what the delivery path now confirms a \
             folded paste on (item 421): a screen that moved without showing the text is settled by \
             this field, where four rounds of screen predicates could settle nothing",
        );
        assert_eq!(
            asked.transcript,
            Some(PathBuf::from(
                "/home/coin/.claude/projects/-tmp-probe/2987c0d6.jsonl"
            )),
            "⚠⚠⚠ AND WHERE IT IS WRITING — register item 431 measured the spend reader answering 0 \
             for a session whose transcript existed, because it resolves that path from an id. The \
             agent hands it over",
        );

        // ⚠⚠ THE CONTROLS. Each says this reader is not simply answering yes.
        let stop = serde_json::json!({ "hook_event_name": "Stop", "prompt": "not this one" });
        assert_eq!(
            asked_in(&stop),
            None,
            "⚠ only the event that OPENS a turn states a prompt; a `Stop` carrying the key must not \
             be read as one, or a turn's end would confirm its beginning",
        );
        let mut subagent = captured_submit();
        subagent["agent_id"] = serde_json::json!("sub-1");
        assert_eq!(
            asked_in(&subagent),
            None,
            "⚠⚠ and a SUBAGENT's turn is not the pane's — the same exclusion `report_for` opens \
             with. What a sub-agent was asked says nothing about the prompt this pane took",
        );
        let mut promptless = captured_submit();
        promptless
            .as_object_mut()
            .expect("an object")
            .remove("prompt");
        assert_eq!(
            asked_in(&promptless),
            None,
            "⚠ a submit with no `prompt` states nothing rather than states an empty one: *the agent \
             was asked nothing* is a claim, and a payload that omits the key has not made it",
        );
        let mut no_transcript = captured_submit();
        no_transcript["transcript_path"] = serde_json::json!("");
        assert_eq!(
            asked_in(&no_transcript).expect("still a submit").transcript,
            None,
            "⚠⚠ but a MISSING transcript is not a missing turn: this is a fact to use when offered \
             and never one to demand, or an agent writing no transcript would stop being able to \
             report the prompt it took",
        );
    }

    /// **THE PAYLOAD A REAL AGENT SENT AT THE END OF A TURN**, captured 2026-08-18 from `claude`
    /// 2.1.234 by putting a logging wrapper at the path the agent's own configuration names, so
    /// every payload was recorded verbatim and still handed on to the client behind it.
    ///
    /// ⚠⚠⚠ A CAPTURE, for [`captured_submit`]'s reason: what this reader must survive is the
    /// agent's schema. Two of these keys are the answer to register item 441 —
    /// `last_assistant_message` is the reply a repainting pane could not be read for, and
    /// `prompt_id` is the same id the submit above carries, which is what makes the two ends of one
    /// turn nameable at all.
    ///
    /// Verbatim but for the values, which are shortened; the KEYS and their shapes are as captured.
    fn captured_rest() -> Value {
        serde_json::json!({
            "session_id": "3a9c8559-735c-43ab-ba65-498684aa97da",
            "transcript_path": "/home/coin/.claude/projects/-home-coin-sprag/3a9c8559.jsonl",
            "cwd": "/home/coin/sprag",
            "prompt_id": "8ee4c9d6-00d6-4ec4-a46f-fb2eb4306818",
            "permission_mode": "auto",
            "effort": { "level": "xhigh" },
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "last_assistant_message": "the five sentences\nMILESTONE REACHED",
            "background_tasks": [],
            "session_crons": [],
        })
    }

    /// **THE NOTICE A REAL AGENT RAISED WHEN IT HAD NOTHING IN FLIGHT**, captured 2026-08-19 from a
    /// live pane by the same logging-wrapper recipe as [`captured_rest`]. Recorded TWICE, in two
    /// separate turns, with the same two values.
    ///
    /// ⚠⚠⚠ The two keys this product was dropping are both here: `notification_type` says WHICH
    /// kind of notice it is, and `message` says it in words. Verbatim but for the ids.
    fn captured_idle_notice() -> Value {
        serde_json::json!({
            "session_id": "9fa9e858-9f06-48b3-b209-7719596c1eb6",
            "transcript_path": "/home/coin/.claude/projects/-home-coin-sprag/9fa9e858.jsonl",
            "cwd": "/home/coin/sprag",
            "prompt_id": "1ede3513-323e-48d5-905e-4be58e0a3dec",
            "hook_event_name": "Notification",
            "message": "Claude is waiting for your input",
            "notification_type": "idle_prompt",
        })
    }

    /// **THE KIND A PERMISSION DIALOG CARRIES**, captured live 2026-08-19 — the word register item
    /// 452 recorded as missing and could not raise.
    ///
    /// It has no branch in [`report_for`] and must not get one: the event's own row already answers
    /// `blocked`, which is the right answer for it. What it is for is being THIS gate's control, so
    /// the boundary around [`IDLE_NOTICE`] is drawn against the word the product actually sends
    /// rather than an invented one — the difference between *"a kind I made up still blocks"* and
    /// *"the dialog still blocks"*.
    const CAPTURED_DIALOG_NOTICE: &str = "permission_prompt";

    /// ⚠⚠⚠⚠⚠ **AN IDLE NAG IS THE REST NO TURN BOUNDARY ANNOUNCED** — register item 458's first
    /// half, and the completion of 452's.
    ///
    /// Every other row of [`Target::events`] is a turn boundary, so a turn that DIES raises none of
    /// them and the pane's last report stands for ever: measured, a fresh run waited six minutes on
    /// one. This notice is raised by the agent's own idleness timer instead, so it is the only thing
    /// that speaks in exactly that case — **measured live on a pane whose turn ended without its
    /// `Stop` ever reaching this daemon.** 452 stopped it being read as a false `blocked`; this is
    /// the other half, where it delivers the true `idle`.
    ///
    /// # ⚠⚠⚠⚠ The control is the DIALOG'S OWN WORD, and it must still block
    ///
    /// A false `idle` is the worst answer here — a run types its next prompt into whatever is on the
    /// pane, and a numbered menu takes it as a SELECTION. Two independent measurements say this word
    /// is not a dialog's: the dialog's is [`CAPTURED_DIALOG_NOTICE`], and the agent suppresses the
    /// nag entirely while a dialog is open (one stood 155 s, 2.6× the 60 s threshold, and raised no
    /// idle notice).
    ///
    /// ⚠⚠ **EACH ARM KILLS ITS OWN MUTATION**, which is what makes three of them rather than one:
    /// the first dies if the notice is dropped or read as `blocked`, the second if the KIND stops
    /// being read at all, and the third if an ABSENT kind is taken for a claim of rest.
    #[test]
    fn an_idle_notice_reports_the_rest_and_a_real_dialog_still_blocks() {
        assert_eq!(
            report_for(&CLAUDE, &captured_idle_notice()),
            Some(Outcome::Report(AgentState::Idle)),
            "⚠⚠⚠⚠⚠ THE PANE IS AT REST AND THIS IS THE ONLY THING THAT SAYS SO. A turn killed by \
             an API error raises no `Stop`, so dropping this leaves the pane's last `working` \
             standing for ever and a fresh run waiting on it — register item 458",
        );

        // ⚠⚠⚠⚠ THE CONTROL, one field changed, and the field carries the word the product SENDS.
        let mut permission = captured_idle_notice();
        permission[NOTICE_KIND] = serde_json::json!(CAPTURED_DIALOG_NOTICE);
        assert_eq!(
            report_for(&CLAUDE, &permission),
            Some(Outcome::Report(AgentState::Blocked)),
            "⚠⚠⚠⚠⚠ THE DIALOG MUST STILL BLOCK. A notice read as `idle` because it is a notice \
             would let a run type its next prompt at a numbered menu, where the keystroke SELECTS",
        );

        // ⚠⚠⚠ AND A KIND NOBODY HERE HAS INTERPRETED KEEPS THE EVENT'S OWN MEANING. The image ships
        // ten more of these words; this crate has measured the meaning of exactly one.
        let mut unread = captured_idle_notice();
        unread[NOTICE_KIND] = serde_json::json!("some_kind_nobody_here_has_seen");
        assert_eq!(
            report_for(&CLAUDE, &unread),
            Some(Outcome::Report(AgentState::Blocked)),
            "⚠⚠⚠ a notice of an UNMEASURED kind must still mean the agent wants somebody — \
             widening the rest to every notice is how a real dialog would stop being seen",
        );

        // ⚠⚠ AND A NOTICE WITH NO KIND AT ALL, which is what an older agent sends.
        let mut kindless = captured_idle_notice();
        kindless
            .as_object_mut()
            .expect("an object")
            .remove(NOTICE_KIND);
        assert_eq!(
            report_for(&CLAUDE, &kindless),
            Some(Outcome::Report(AgentState::Blocked)),
            "⚠⚠ an absent classification is not a claim to be idle; the event's own meaning stands",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE AGENT STILL USES THE WORD THIS CRATE ACTS ON** — the one gate here that asks the
    /// INSTALLED AGENT rather than a payload this repository captured.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the captured fixtures cannot answer this, which is the whole reason it exists
    ///
    /// Every other gate in this module drives [`captured_idle_notice`] — a payload frozen on
    /// 2026-08-19. **A frozen payload stays green for ever**, including on the day the agent renames
    /// the field or the value. That is register item 428's shape at this door: a fixture that
    /// bypasses the product's own source of truth passes while the door is nailed shut.
    ///
    /// ⚠⚠⚠⚠ **AND THE FAILURE WOULD BE SILENT AND EXPENSIVE.** [`report_for`] reads
    /// [`NOTICE_KIND`]; if that key or [`IDLE_NOTICE`] moves, `is_idle_notice` answers `false`, the
    /// notice falls through to [`Target::events`], and a resting pane is reported **`blocked`** —
    /// which is the defect item 452 measured, arriving back with no red anywhere. An unattended run
    /// then takes `screening` → `awaiting_human` → the `<final>` `blocked`.
    ///
    /// # ⚠⚠⚠ Why it is `#[ignore]`d, and why that is not a loophole
    ///
    /// It reads a program that is not this repository's and may not be installed at all — a
    /// contributor without `claude`, and every hosted runner. This workspace's standing arrangement
    /// for a gate that needs the real world is exactly this: `-- --ignored --test-threads=1`, run
    /// locally as part of a round. ⚠ It REFUSES rather than skipping when the agent is absent: a
    /// gate asked to check the live vocabulary and finding no agent has not checked anything, and
    /// passing there is the vacuity this whole test is about.
    #[test]
    #[ignore = "reads the installed agent's own image; run with --ignored"]
    fn the_installed_agent_still_speaks_the_words_this_crate_reads() {
        /// Bytes at a time, with an overlap, because the image is hundreds of megabytes and a
        /// needle can straddle any boundary a reader picks.
        const CHUNK: usize = 1 << 20;

        let Ok(found) = std::process::Command::new("sh")
            .args(["-c", "command -v claude"])
            .output()
        else {
            panic!("⚠ this gate needs a shell to locate the agent");
        };
        let path = String::from_utf8_lossy(&found.stdout).trim().to_owned();
        assert!(
            !path.is_empty(),
            "⚠⚠⚠ NO `claude` ON PATH, so the live vocabulary could not be read at all. This gate \
             refuses rather than passing: an agent it cannot find is an agent it has not checked, \
             and a green here would be the frozen-fixture vacuity it exists to end",
        );
        // The launcher on this platform is a symlink to a versioned image; the words are in the
        // image, so the link is followed rather than read.
        let image = std::fs::canonicalize(&path).unwrap_or_else(|why| {
            panic!("⚠ the agent at {path:?} could not be resolved to an image: {why}")
        });

        let carries = |needle: &str| -> bool {
            use std::io::Read as _;
            let Ok(mut file) = std::fs::File::open(&image) else {
                return false;
            };
            let mut window = vec![0_u8; CHUNK + needle.len()];
            let mut carried = 0_usize;
            loop {
                let Ok(read) = file.read(&mut window[carried..]) else {
                    return false;
                };
                if read == 0 {
                    return false;
                }
                let filled = carried + read;
                if window[..filled]
                    .windows(needle.len())
                    .any(|at| at == needle.as_bytes())
                {
                    return true;
                }
                // Keep the tail so a needle split across two reads is still found.
                carried = needle.len().saturating_sub(1).min(filled);
                window.copy_within(filled - carried..filled, 0);
            }
        };

        assert!(
            carries(NOTICE_KIND),
            "⚠⚠⚠⚠⚠ THE AGENT AT {image:?} NO LONGER CARRIES {NOTICE_KIND:?}. `report_for` reads \
             that key to tell an idle nag from a dialog; without it EVERY notice becomes `blocked` \
             again, including the one that means the pane is at REST — register items 452 and 458. \
             Re-capture the payload, and re-read what the kinds are now",
        );
        assert!(
            carries(IDLE_NOTICE),
            "⚠⚠⚠⚠⚠ THE AGENT AT {image:?} NO LONGER CARRIES {IDLE_NOTICE:?}. This is the one value \
             this crate ACTS on: without it a resting pane is reported `blocked`, and an unattended \
             run walks that to the `<final>` `blocked` with nothing anywhere going red. Find what \
             the idle notice is called now — `grep -a -o 'notificationType:\"[a-z_]*\"' <image>` is \
             how the closed set was read in the first place",
        );
        // ⚠⚠⚠ AND THE CONTROL: a word nobody has ever put in that image must NOT be found, or this
        // gate would pass on a reader that answers `true` for anything — the shape a search over a
        // 300MB file makes very easy to ship.
        assert!(
            !carries("sprag_never_wrote_this_word_into_any_agent"),
            "⚠⚠⚠ the control: this search must be capable of saying NO, or both claims above are \
             about a function that always agrees",
        );
    }

    /// ⚠⚠⚠⚠ **AND THE NOTICE'S WORDS SURVIVE** — the half that answers *what does the peer want*.
    ///
    /// `awaiting_human` reports *"the peer is blocked on something this host cannot read as a
    /// numbered menu"*. True of the screen, and the peer had said it in a field one layer up.
    #[test]
    fn a_notice_states_in_words_what_the_agent_wants() {
        assert_eq!(
            noticed_in(&captured_idle_notice()).as_deref(),
            Some("Claude is waiting for your input"),
            "⚠⚠⚠ the peer's own words, taken before a terminal has drawn a cell of them",
        );

        // ⚠⚠⚠⚠⚠ THE EVENT CONTROL IS THE NOTICE WITH ITS EVENT CHANGED AND NOTHING ELSE — the
        // vacuity `a_rest_payload_states_the_answer_the_agent_gave` records paying for. A control
        // built from a payload that lacks `message` would return `None` at the wrong `?`.
        let mut wrong_event = captured_idle_notice();
        wrong_event["hook_event_name"] = serde_json::json!(REST_EVENT);
        assert_eq!(
            noticed_in(&wrong_event),
            None,
            "⚠ only a notice states what the agent wants attention FOR",
        );

        // A subagent's notice is not the pane's.
        let mut subagent = captured_idle_notice();
        subagent["agent_id"] = serde_json::json!("sub-1");
        assert_eq!(
            noticed_in(&subagent),
            None,
            "⚠⚠ the exclusion `report_for` and `said_in` both open with",
        );

        // ⚠ A notice that carries no words has not said any.
        let mut wordless = captured_idle_notice();
        wordless
            .as_object_mut()
            .expect("an object")
            .remove(NOTICE_TEXT);
        assert_eq!(
            noticed_in(&wordless),
            None,
            "⚠ absent is not empty — an empty statement is a claim this payload did not make",
        );
    }

    /// **THE GATE FOR THE OTHER END OF THE TURN** — the agent states what it ANSWERED, and until now
    /// that arrived on every turn and was dropped on the floor.
    ///
    /// See [`said_in`] for what it is worth: a full-screen agent's pane was measured with its whole
    /// logical-line count frozen at 37 while the agent wrote reply after reply, so the reader the
    /// judge uses answered `0 complete lines` for ever. This is the same fact, stated by the
    /// program instead of scraped off what it painted.
    #[test]
    fn a_rest_payload_states_the_answer_the_agent_gave() {
        assert_eq!(
            said_in(&captured_rest()).as_deref(),
            Some("the five sentences\nMILESTONE REACHED"),
            "⚠⚠⚠⚠ THE ANSWER IS THE AGENT'S OWN STATEMENT OF WHAT IT SAID — the evidence register \
             item 441 measured a pane being unable to give: 37 logical lines, never moving, while \
             the marker stood alone on the screen",
        );

        // ⚠⚠ THE CONTROLS. Each says this reader is not simply answering yes.
        //
        // ⚠⚠⚠⚠⚠ THE EVENT CONTROL IS THE REST PAYLOAD WITH ITS EVENT CHANGED AND NOTHING ELSE, and
        // the first draft of it was VACUOUS: it used the real submit capture, which carries no
        // statement at all, so this reader answered `None` on the missing key and the event check
        // was never exercised. The mutation proved it — reading EVERY event kept this gate green.
        // A control has to differ from the passing case in the ONE field it is about.
        let mut wrong_event = captured_rest();
        wrong_event["hook_event_name"] = serde_json::json!(SUBMIT_EVENT);
        assert_eq!(
            said_in(&wrong_event),
            None,
            "⚠ only the event that ENDS a turn states an answer; a submit read as one would judge \
             a turn on the answer to the turn before it",
        );
        assert_eq!(
            said_in(&captured_submit()),
            None,
            "⚠ and the REAL submit, which states no answer at all — the world where a turn's \
             opening is asked what it answered",
        );
        let mut subagent = captured_rest();
        subagent["agent_id"] = serde_json::json!("sub-1");
        assert_eq!(
            said_in(&subagent),
            None,
            "⚠⚠ and a SUBAGENT's answer is not the pane's — the same exclusion `report_for` and \
             `asked_in` open with",
        );
        let mut silent = captured_rest();
        silent
            .as_object_mut()
            .expect("an object")
            .remove("last_assistant_message");
        assert_eq!(
            said_in(&silent),
            None,
            "⚠ a rest with no statement states NOTHING rather than states an empty answer: a \
             consumer told `Some(\"\")` would judge a turn against a claim the agent never made",
        );
    }

    /// The identity a test's launch is named with — FIXED, so what a rule answers is a function of
    /// the argv and not of the round it was run in. [`mint_session_id`] is measured separately, by
    /// the one thing that can measure it: a live agent accepting it.
    const MINTED: &str = "00000000-0000-4000-8000-000000000001";

    /// A fixed minter, for the rules that do not care which name they carry.
    fn fixed() -> String {
        MINTED.to_owned()
    }

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
            // ⚠ The launch still gets NAMED. The two decisions are independent — a caller who said
            // what configures this run has not said what it is called — so this asserts the
            // absence of the flag it refused rather than the absence of everything.
            let carried = launch_args_from(&argv, Path::new(EXE), None, |_| false, fixed);
            assert!(
                !carried.iter().any(|arg| arg == "--settings"),
                "{argv:?} already says what configures it, so sprag adds no second copy: {carried:?}",
            );
            assert_eq!(
                carried,
                vec!["--session-id".to_owned(), MINTED.to_owned()],
                "and what it does carry is the name, which that refusal has nothing to do with",
            );
        }
    }

    /// **AN AGENT THIS DAEMON LAUNCHES TALKS TO THE MCP SERVER OF THE IMAGE THAT MADE ITS PANE** —
    /// register item 444, at the decision that produces it.
    ///
    /// The whole claim is the PATH: not *an* sprag server, but the sibling of the binary doing the
    /// launching, so there is no second image on the machine to keep in step and nothing to install.
    /// A document naming a bare program name would satisfy every other assertion here and reopen the
    /// item — it is `PATH` that decided which server an agent got, and `PATH` on this machine held
    /// one three weeks behind the tree.
    ///
    /// ⚠ It asserts the SHAPE around that path as well, because each part is separately capable of
    /// producing a launch that starts nothing: the flag immediately before its value, one server
    /// rather than a roster, the key [`MCP_SERVER`] publishes, and the transport spelled out.
    ///
    /// ⚠⚠⚠ **AND IT ASSERTS AN ABSENCE THAT IS THE ITEM'S OWN «MUST NOT BREAK»**: sprag never adds
    /// [`Target::mcp_only_flag`]. That flag would make this the ONLY server the agent has, deleting
    /// every other one its user configured — mnemosyne among them on the machine this was written
    /// on. Injection ADDS; it does not take the roster over.
    #[test]
    fn a_launch_is_handed_the_mcp_server_beside_the_image_that_makes_its_pane() {
        let carried = launch_args_from(
            &["claude".to_owned()],
            Path::new(EXE),
            Some(Path::new(MCP)),
            |_| false,
            fixed,
        );
        let at = carried
            .iter()
            .position(|arg| arg == "--mcp-config")
            .unwrap_or_else(|| panic!("the launch carries the MCP flag: {carried:?}"));
        let document: Value = serde_json::from_str(&carried[at + 1])
            .unwrap_or_else(|why| panic!("the value beside the flag is JSON ({why}): {carried:?}"));

        assert_eq!(
            document["mcpServers"][MCP_SERVER]["command"], MCP,
            "⚠⚠⚠ the server is the SIBLING OF THIS IMAGE, by absolute path — a bare name would be \
             resolved on PATH, which is what handed agents a three-week-old roster: {document}",
        );
        assert_eq!(
            document["mcpServers"][MCP_SERVER]["type"], "stdio",
            "the transport is stated rather than left to a default: {document}",
        );
        assert_eq!(
            document["mcpServers"]
                .as_object()
                .expect("an object of servers")
                .keys()
                .collect::<Vec<_>>(),
            vec![MCP_SERVER],
            "⚠⚠ exactly one server, under the key this module publishes: {document}",
        );
        assert!(
            !carried.iter().any(|arg| arg == "--strict-mcp-config"),
            "⚠⚠⚠⚠ sprag ADDS a server; it never says «only mine», which would delete every server \
             this agent's user configured: {carried:?}",
        );
    }

    /// **A LAUNCH THAT HAS ALREADY SETTLED ITS MCP SERVERS IS LEFT EXACTLY AS ITS CALLER WROTE IT**
    /// — three spellings, one rule, and the third is the one that is easy to miss.
    ///
    /// `--mcp-config` in either spelling says *these are my servers*, and a second copy is the
    /// precedence question [`Target::session_args`] refuses for `--settings`. `--strict-mcp-config`
    /// ALONE says something stronger and stranger: *only what I named*, having named nothing — an
    /// agent asked for an empty MCP environment, and a daemon that filled it would be overruling the
    /// one instruction on that command line.
    ///
    /// ⚠ Each case also asserts that the OTHER two decisions still fire. Folding the refusals
    /// together is a bug shaped exactly like a missing feature: a caller who said which servers this
    /// launch has has not said how it reports or what it is called.
    #[test]
    fn a_launch_that_says_which_mcp_servers_it_has_keeps_them() {
        for argv in [
            vec!["claude", "--mcp-config", "/home/me/servers.json"],
            // The joined spelling, for `session_args`' measured reason: a reader that knew only the
            // separated form would append the second flag this refusal exists to prevent.
            vec!["claude", "--mcp-config={\"mcpServers\":{}}"],
            // ⚠ NOT a spelling of the flag above — a different flag, whose meaning is a refusal.
            vec!["claude", "--strict-mcp-config"],
        ] {
            let argv = argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
            assert_eq!(
                CLAUDE.mcp_args(&argv, Path::new(MCP)),
                None,
                "{argv:?} already says which MCP servers it has",
            );
            let carried = launch_args_from(
                &argv,
                Path::new(EXE),
                Some(Path::new(MCP)),
                |_| false,
                fixed,
            );
            assert!(
                !carried.iter().any(|arg| arg == "--mcp-config"),
                "{argv:?} settles the question, so sprag adds no second answer: {carried:?}",
            );
            assert_eq!(
                carried,
                vec![
                    "--settings".to_owned(),
                    CLAUDE
                        .session_args(&argv, Path::new(EXE))
                        .expect("claude takes a per-launch document")[1]
                        .clone(),
                    "--session-id".to_owned(),
                    MINTED.to_owned(),
                ],
                "and what it does carry is the reporting and the name, which this refusal has \
                 nothing to do with",
            );
        }
    }

    /// **AN IMAGE WITH NO SERVER BESIDE IT HANDS OVER NOTHING, AND INSTRUMENTS THE LAUNCH ANYWAY.**
    ///
    /// `None` reaches here from [`crate::mcp_beside`], which refuses to fall back to `PATH` — see
    /// its doc for why an unknown-vintage server is worse than none. What this fixes in place is the
    /// consequence: such a launch is EXACTLY what it was before item 444, user scope and all, rather
    /// than an agent that also lost its hooks or its name because one arm answered `None`.
    #[test]
    fn an_image_with_no_server_beside_it_still_instruments_and_names_the_launch() {
        let carried = launch_args_from(
            &["claude".to_owned()],
            Path::new(EXE),
            None,
            |_| false,
            fixed,
        );
        assert!(
            !carried.iter().any(|arg| arg == "--mcp-config"),
            "there is no sibling to name, so nothing is named: {carried:?}",
        );
        assert!(
            carried.iter().any(|arg| arg == "--settings"),
            "⚠⚠ and the launch is still instrumented: {carried:?}",
        );
        assert!(
            carried.iter().any(|arg| arg == "--session-id"),
            "⚠⚠ and still named: {carried:?}",
        );
    }

    /// What a launch carries is decided by the program, by its BASENAME, and by nothing else.
    ///
    /// The three answers in one place because they are one rule: an absolute path to an agent is
    /// that agent, an agent with no per-launch door is left to `install-hooks`, and everything else
    /// — which is nearly every pane ever opened — is launched untouched.
    ///
    /// ⚠ An MCP server IS offered here, so the emptiness assertions below are about every door at
    /// once: a `codex` that grew an injection it has no measured flag for, or a shell handed a
    /// `--mcp-config` meant for its child, fails here rather than at somebody's keyboard.
    #[test]
    fn only_a_recognised_agent_with_a_per_launch_door_carries_anything() {
        let carried = |argv: &[&str]| {
            launch_args_from(
                &argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
                Path::new(EXE),
                Some(Path::new(MCP)),
                |_| false,
                fixed,
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
        let reporting = launch_args_from(
            &argv,
            Path::new(EXE),
            None,
            |target| target.name == "claude",
            fixed,
        );
        assert!(
            !reporting.iter().any(|arg| arg == "--settings"),
            "the user ran install-hooks; sprag adds no hooks on top of it: {reporting:?}",
        );
        // ⚠ NAMING SURVIVES IT, and this is the assertion that would have caught the two decisions
        // being folded together. A user who installed hooks machine-wide has said how their agent
        // REPORTS; they have said nothing about what this one session is called, and sprag still
        // needs to be able to find what it records.
        assert_eq!(
            reporting,
            vec!["--session-id".to_owned(), MINTED.to_owned()],
            "an agent that already reports is still named",
        );
        assert!(
            launch_args_from(&argv, Path::new(EXE), None, |_| false, fixed)
                .iter()
                .any(|arg| arg == "--settings"),
            "and the control: with nothing installed the launch is instrumented",
        );
    }

    /// **A LAUNCH THAT HAS ALREADY SETTLED WHICH SESSION IT IS, IS NOT RENAMED** — and the four
    /// spellings of settling it are one rule.
    ///
    /// Measured, which is why the refusal exists at all: a second launch carrying an id already in
    /// use is refused outright — `Error: Session ID … is already in use.` — so a name sprag adds on
    /// top of one the caller chose does not merely lose a precedence argument, it can cost the
    /// launch. `--resume`, `--continue` and `--fork-session` settle it the other way, by naming a
    /// session to CONTINUE.
    #[test]
    fn a_launch_that_already_names_its_session_is_left_to_its_own_name() {
        let named = |argv: &[&str]| {
            CLAUDE.identity_args(
                &argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
                fixed,
            )
        };
        for argv in [
            vec![
                "claude",
                "--session-id",
                "de305d54-75b4-431b-adb2-eb6b9e546014",
            ],
            // The joined spelling, for `session_args`' measured reason: a reader that knew only the
            // separated form would append a second one.
            vec![
                "claude",
                "--session-id=de305d54-75b4-431b-adb2-eb6b9e546014",
            ],
            vec!["claude", "--resume", "de305d54-75b4-431b-adb2-eb6b9e546014"],
            vec!["claude", "-r"],
            vec!["claude", "--continue"],
            vec!["claude", "-c"],
            vec!["claude", "--fork-session"],
        ] {
            assert_eq!(
                named(&argv),
                None,
                "{argv:?} has already said which session this is",
            );
        }
        assert_eq!(
            named(&["claude", "-p", "hello"]),
            Some(vec!["--session-id".to_owned(), MINTED.to_owned()]),
            "and the control: a launch that says nothing about it is named",
        );
        assert_eq!(
            CODEX.identity_args(&["codex".to_owned()], fixed),
            None,
            "an agent whose record sprag cannot find is not given a name it could not use",
        );
    }

    /// **THE FLAG THAT NAMES A SESSION IS THE FLAG THAT FINDS IT** — one string, two crates, and
    /// this is the only place both are visible.
    ///
    /// `sprag-host` WRITES it onto an agent's command line at a pane's birth; `sprag-plugin` READS
    /// it back off the running process to learn what that session is spending. The host depends on
    /// the plugin and not the reverse, so the constant cannot be shared — and drift between them is
    /// SILENT in the worst direction: the loop would find no identity, report no spend, and be
    /// indistinguishable from an agent that had not started yet.
    #[test]
    fn the_flag_that_names_a_session_is_the_flag_that_finds_it() {
        assert_eq!(
            CLAUDE.identity_flag,
            Some(sprag_plugin::CLAUDE_IDENTITY_FLAG),
            "the writer and the reader must name the same argument",
        );
    }

    /// ⚠⚠⚠ **A RESTORED LAUNCH IS INSTRUMENTED AFRESH AND NAMED NOT AT ALL** — the composition the
    /// whole restore-resumes design rests on, and the one nothing else holds.
    ///
    /// A durability restore hands the launch a resume of the name the pane was recorded under. Two
    /// independent decisions then have to go opposite ways in the same call:
    ///
    /// * **`session_args` must still fire.** The hooks name THIS daemon's binary, and the daemon that
    ///   wrote the snapshot is gone — a restored agent that kept the old instrumentation would report
    ///   to a socket that no longer exists, which is `spawn_restored`'s own stated reason for asking
    ///   the source instead of trusting the stored argv.
    /// * **`identity_args` must NOT.** A second name on a launch that already names its conversation
    ///   is sprag's answer silently winning over the restore's — and the agent refuses an id already
    ///   in use outright, so this is not a preference, it is whether the pane comes up at all.
    ///
    /// They are asked separately and refuse for unrelated reasons, so nothing about the code makes
    /// them agree; only this does. ⚠ A mutation that drops `--resume` from `identity_args`' refusal
    /// list turns every restored agent into a fresh one AND, at the same time, into one the agent
    /// itself rejects — a failure that reaches a person's screen as a pane that will not open.
    #[test]
    fn a_restored_launch_is_instrumented_afresh_and_named_not_at_all() {
        const RESUMED: &str = "d8be3b14-3f26-4220-96f5-c57a462ea383";
        let argv = vec![
            "claude".to_owned(),
            "--resume".to_owned(),
            RESUMED.to_owned(),
        ];
        let carried = launch_args_from(
            &argv,
            Path::new(EXE),
            Some(Path::new(MCP)),
            |_| false,
            || MINTED.to_owned(),
        );

        assert!(
            carried.iter().any(|arg| arg == "--settings"),
            "⚠⚠⚠ a restored agent must be instrumented by the daemon doing the restoring — the \
             recorded instrumentation names a socket that is gone. Got {carried:?}",
        );
        // ⚠ AND THE SERVER, for the same reason one flag up: what a restore re-runs is a recorded
        // argv, and the daemon that records it can be replaced by one built from other code. The
        // server an agent talks to has to be the image that is driving it NOW, not the one that
        // opened the pane before the machine was rebooted.
        assert!(
            carried.iter().any(|arg| arg == "--mcp-config"),
            "⚠⚠⚠ a restored agent talks to the MCP server of the daemon restoring it: {carried:?}",
        );
        assert!(
            !carried.iter().any(|arg| arg == "--session-id"),
            "⚠⚠⚠ and it must NOT be named again: it already says which conversation it is \
             continuing, and the agent refuses an id already in use. Got {carried:?}",
        );
        assert!(
            !carried.iter().any(|arg| arg == MINTED),
            "⚠⚠ nor may a mint reach it by any other spelling: {carried:?}",
        );
    }

    /// **A RESUME IS REFUSED FOR AN ARGV THAT ALREADY NAMES ITS CONVERSATION**, by either spelling —
    /// [`Target::identity_args`]'s refusals, mirrored, because the two would otherwise contradict
    /// each other on the same launch.
    ///
    /// ⚠ And an EMPTY name is refused, which is not the same case: a snapshot from a build that
    /// recorded no name loads as `None`, but one that recorded an empty string would sail through and
    /// hand the agent a bare `--resume` — whose own help says it *"open[s] interactive picker"*, at a
    /// pane nobody is watching, on a daemon that has just restarted every pane at once.
    #[test]
    fn a_resume_is_refused_for_an_argv_that_already_names_its_conversation() {
        const WANTED: &str = "d8be3b14-3f26-4220-96f5-c57a462ea383";
        const HELD: &str = "de305d54-75b4-431b-adb2-eb6b9e546014";

        assert_eq!(
            CLAUDE.resume_args(&["claude".to_owned()], WANTED),
            Some(vec!["--resume".to_owned(), WANTED.to_owned()]),
            "the control: a bare launch takes the resume, or every assertion below is vacuous",
        );
        for held in [
            vec![
                "claude".to_owned(),
                "--session-id".to_owned(),
                HELD.to_owned(),
            ],
            vec![format!("--session-id={HELD}")].into_iter().fold(
                vec!["claude".to_owned()],
                |mut argv, arg| {
                    argv.push(arg);
                    argv
                },
            ),
            vec!["claude".to_owned(), "--resume".to_owned(), HELD.to_owned()],
        ] {
            assert_eq!(
                CLAUDE.resume_args(&held, WANTED),
                None,
                "⚠⚠ a launch that already names its conversation keeps the name it was given — \
                 sprag's answer must not silently win over the caller's: {held:?}",
            );
        }
        assert_eq!(
            CLAUDE.resume_args(&["claude".to_owned()], ""),
            None,
            "⚠⚠⚠ AND AN EMPTY NAME IS NOT A NAME. A bare `--resume` opens an interactive PICKER, \
             which on a restore means every restored agent sitting at a menu nobody is watching",
        );
    }

    /// ⚠⚠⚠ **EVERY AGENT THAT CAN BE NAMED CAN BE RESUMED, AND ONLY THOSE** — held over
    /// [`TARGETS`] rather than over the two spellings that exist today.
    ///
    /// The pair is an invariant rather than a coincidence, and it fails in both directions:
    ///
    /// * a `resume_flag` with no `identity_flag` resumes a name **nothing ever wrote down** — the
    ///   snapshot field is filled from the identity, so it would always be `None` and the flag would
    ///   be prose;
    /// * an `identity_flag` with no `resume_flag` is the defect this round exists to fix, one agent
    ///   over: its sessions are named, recorded, and then abandoned at every daemon restart.
    ///
    /// **A list with no glob decides alone** (R376/R381) — the day a third agent is added, this is
    /// what asks whether both halves were answered.
    #[test]
    fn every_agent_that_can_be_named_can_be_resumed() {
        let split: Vec<&str> = TARGETS
            .iter()
            .filter(|target| target.identity_flag.is_some() != target.resume_flag.is_some())
            .map(|target| target.name)
            .collect();
        assert!(
            split.is_empty(),
            "⚠⚠⚠ {split:?} can be named but not resumed, or resumed but not named. Naming is what \
             WRITES the record a resume re-enters, so the two are one decision — an agent gains \
             both in the same round, against the same measurement, or neither",
        );
        assert!(
            TARGETS.iter().any(|target| target.resume_flag.is_some()),
            "⚠ the control: at least one agent must be resumable, or the invariant above is \
             satisfied by a product in which nothing works",
        );
    }

    /// **THE NAME OF A LAUNCH IS FOUND WHETHER IT WAS MINTED OR RESUMED** — what keeps a name alive
    /// across the SECOND restart.
    ///
    /// A restored pane's argv says `--resume X`, not `--session-id X`. A reader that knew only the
    /// minting flag would answer `None` for it, the snapshot would record no name, and the next
    /// restart would mint a fresh one — so the work would survive exactly one daemon replacement and
    /// then be lost, which is a worse failure than losing it every time because it looks fixed.
    #[test]
    fn the_name_of_a_launch_is_found_whether_it_was_minted_or_resumed() {
        const NAME: &str = "d8be3b14-3f26-4220-96f5-c57a462ea383";
        for spelling in [
            vec![
                "claude".to_owned(),
                "--session-id".to_owned(),
                NAME.to_owned(),
            ],
            vec!["claude".to_owned(), "--resume".to_owned(), NAME.to_owned()],
            vec![
                "/usr/local/bin/claude".to_owned(),
                format!("--resume={NAME}"),
            ],
        ] {
            assert_eq!(
                launched_identity(&spelling),
                Some(NAME.to_owned()),
                "a launch in conversation {NAME} must say so however it got there: {spelling:?}",
            );
        }
        assert_eq!(
            launched_identity(&["claude".to_owned()]),
            None,
            "and an unnamed launch names nothing — the case every pane that is not an agent is in",
        );
        assert_eq!(
            launched_identity(&[
                "sh".to_owned(),
                "-c".to_owned(),
                format!("claude --resume {NAME}"),
            ]),
            None,
            "⚠⚠ AND A SHELL IS NOT AN AGENT, however its script reads. sprag did not write that \
             argv, so it cannot read its words — the same rule `launch_args_from` refuses on, and \
             the reason a restore's shell fallback is never handed a resume",
        );
    }

    /// The free [`resume_args`] refuses for everything that is not a named agent — the decisive case
    /// on the restore path, where a NON-allowlisted argv falls back to a plain shell.
    ///
    /// The host reads the BUILT command rather than the recorded argv for exactly this: a pane whose
    /// agent is no longer allowlisted comes back as `sh`, and a shell handed `--resume` is a shell
    /// handed an argument meant for something else.
    #[test]
    fn a_launch_that_is_not_an_agent_takes_no_resume() {
        const NAME: &str = "d8be3b14-3f26-4220-96f5-c57a462ea383";
        assert_eq!(
            resume_args(&["claude".to_owned()], NAME),
            vec!["--resume".to_owned(), NAME.to_owned()],
            "the control: an agent takes one, or the refusals below are vacuous",
        );
        for not_an_agent in [
            vec!["sh".to_owned()],
            vec!["/bin/bash".to_owned(), "-c".to_owned(), "cat".to_owned()],
            vec!["codex".to_owned()],
            Vec::new(),
        ] {
            assert!(
                resume_args(&not_an_agent, NAME).is_empty(),
                "⚠⚠ {not_an_agent:?} names no conversation this daemon can re-enter, so nothing \
                 may be appended to it — `codex` included, and that is `resume_flag`'s `None` \
                 doing its job rather than an oversight",
            );
        }
    }

    /// A minted identity is a valid UUID, which is the only property the agent states.
    ///
    /// ⚠ Shape only. That an agent ACCEPTS one and files its record under it is a claim about
    /// another program, and it is fixed where such claims belong — the live gate
    /// `a_minted_session_identity_names_the_record_a_live_agent_writes`.
    #[test]
    fn a_minted_identity_is_shaped_like_a_uuid() {
        let one = mint_session_id();
        let two = mint_session_id();
        assert_ne!(
            one, two,
            "each birth is named afresh, or the second is refused"
        );
        for minted in [&one, &two] {
            let groups: Vec<&str> = minted.split('-').collect();
            assert_eq!(
                groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12],
                "{minted} is not shaped like a UUID",
            );
            assert!(
                minted.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "{minted} holds something that is not a hex digit",
            );
            assert!(
                groups[2].starts_with('4'),
                "{minted} does not say version 4"
            );
            assert!(
                ['8', '9', 'a', 'b'].contains(&groups[3].chars().next().unwrap_or(' ')),
                "{minted} does not carry the RFC 4122 variant",
            );
        }
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

    /// **A machine sprag cannot read is one it instruments** — the arm `is_ok_and` used to swallow.
    ///
    /// [`already_reports`] answers a question about the user's own config, and the reading can fail
    /// before it reads anything: `$HOME` names nowhere absolute, or the agent's config-directory
    /// variable is RELATIVE and sprag refuses to guess which directory it means. Both come back as
    /// an `Err`, and what sprag does with one is a DECISION — instrument, and risk a double report —
    /// not a fallout of how the expression was spelled.
    ///
    /// All three arms, because two of them agree on the answer for different reasons and a test of
    /// either alone would not notice the third changing: an install that reports says `true`, one
    /// that does not says `false`, and an unreadable machine says `false` WITH the error it failed
    /// on. Driven with plain values rather than a `$HOME` full of agent configuration — the point of
    /// separating the decision from the machine is that it can be asked without one.
    ///
    /// REVERT-PROOF: make the error arm answer `true` (the other safe-looking direction, and the one
    /// that silently turns supervision off for that user) and the last case fails.
    #[test]
    fn a_machine_that_cannot_be_read_is_one_sprag_instruments() {
        for target in TARGETS {
            let fixture = Fixture::new(target, None);
            let program = fixture.program();
            fixture.install(&program.display().to_string());
            assert!(
                already_reports(Ok(status_at(target, fixture.path()))),
                "{}: a whole install that can run reports on its own",
                target.name,
            );

            let bare = Fixture::new(target, None);
            assert!(
                !already_reports(Ok(status_at(target, bare.path()))),
                "{}: nothing is installed, so sprag instruments the launch",
                target.name,
            );

            for unreadable in [
                HookError::NoHome,
                HookError::AmbiguousHome("CLAUDE_CONFIG_DIR".to_owned()),
            ] {
                let why = unreadable.to_string();
                assert!(
                    !already_reports(Err(unreadable)),
                    "{}: sprag could not tell ({why}), so it instruments rather than going quiet",
                    target.name,
                );
            }
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
    ///
    /// ⚠⚠⚠⚠ **The stand-in is LINKED from a tracked file, never written here** — register item 467.
    /// A file any process holds open for writing cannot be executed, and this harness runs its
    /// cases on THREADS of one process, so a sibling forking to spawn a program inherits the write
    /// handle and holds it until its own exec. The shell below is what execs it, so the window was
    /// real. The double leaves its marker beside its own directory (`$0`-relative), which is what
    /// lets a tracked file serve a fixture path chosen at run time.
    #[test]
    fn a_hook_command_reaches_a_program_whose_path_a_shell_would_split() {
        let fixture = Fixture::new(&CLAUDE, None);
        let program = sprag_gate::doubles::Doubles::of(env!("CARGO_MANIFEST_DIR"))
            .set("hooks")
            .link("sprag", &fixture.0.join("a dir").join("sprag"));
        let marker = fixture.0.join("it-ran");

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
