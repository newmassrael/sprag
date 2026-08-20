//! The plugin-host control surface — start and observe plugin runs over RPC.
//!
//! `PluginsExternal` is the seam where the pinion-aware host drives the
//! pinion-free plugin substrate ([`sprag_plugin`]). An external AI peer:
//!
//! * `invoke("run", {plugin, …args, guardrails?})` → starts a plugin on a
//!   background thread (so the long, blocking `Driver::run` never freezes the
//!   serve loop) and gets a run id back immediately;
//! * `query("runs")` → observes each run's terminal `Outcome` as scene-as-data;
//! * `query("plugins")` → the available plugin set.
//!
//! Runs are guardrail-bounded by construction: a `run` always gets the default
//! iteration ceiling (the liveness floor), the default WALL-CLOCK deadline, and
//! the plugin's default cost ceiling in its unit — never unbounded, on any of the
//! three axes, because loop safety is first-class. (A print-mode Text dialogue
//! accumulates `Tokens(0)`, so its cost ceiling never binds and the other two are
//! its effective bounds.) Whichever binds first ends the run, and the outcome says
//! WHICH.
//! Target panes are validated at submit time, so a typo is a synchronous
//! `Rejected`, not an async `Failed`.
//!
//! # ⚠⚠⚠ The `ai_loop` form is the door register item 65 had been holding open
//!
//! Five rounds built the outer AI loop's statechart, its driver and its measurement against a live
//! `claude`, and at the end of them **nothing in the daemon constructed one and no surface started
//! one**. It is a plugin like the others now, which is what gives it everything above for free —
//! a run id, the three guardrails, a cancel flag, a journal and a durable record — and what makes
//! `sce-rust-lua` a real dependency of this crate: the loop's document has a script datamodel, so
//! starting one means building an interpreter for it HERE. That trade is written out in the
//! manifest beside the dependency.
//!
//! ⚠ Its own budget is NOT a guardrail. `max_turns` counts the inner agent's turns and one of
//! those is many steps of the loop driving it, so it travels in the brief and a run stopped by it
//! reports the ceiling `turns` — a word whose remedy is in the request rather than in `guardrails`.

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
    ReadRefusal, SchemaField,
};
use serde_json::{Map, Value, json};
use sprag_plugin::{
    Agent, AgentSpec, Attended, Brief, Ceiling, Consent, Consents, Cost, Dialogue, DialogueSpec,
    DoneWhen, Driver, Guardrails, Handback, OrchestrationSpec, Orchestrator, Outcome, OutcomeState,
    Pipe, PipeSpec, Plugin, Readiness, ReadyWhen, ReplyFormat, RunContext, ScreenRule, ScreenRules,
    Turn, WorkspacePaneAccess,
};
use sprag_terminal::{PaneId, Workspace};

use crate::external::{
    as_object, declined, lock, opt_dim, opt_str, refused, require_pane_id, require_str,
    rpc_external_impl,
};
use crate::runs::{RunId, RunRegistry, RunState, RunSummary};

/// The plugin-host external's action that STARTS a run.
pub const RUN_ACTION: &str = "run";
/// The plugin-host external's action that raises a run's cancel flag.
pub const CANCEL_ACTION: &str = "cancel";
/// The plugin-host external's action that asks a run to finish its milestone and then stop.
///
/// ⚠⚠⚠ A SECOND VERB RATHER THAN A MODE ON [`CANCEL_ACTION`], because the outcomes are opposite: a
/// cancel loses the turn in flight and this one banks it. ⚠ ADDING AN ACTION IS ADDITIVE — an older
/// client simply cannot reach it — so this does not earn a `WIRE_PROTOCOL` bump. The residue,
/// stated: a client newer than its daemon gets `UnknownPath` for it, which is the daemon saying it
/// does not serve that address, and is the answer that case should get.
pub const STAND_DOWN_ACTION: &str = "stand_down";
/// **HALT A RUN BETWEEN TURNS, OR LET IT GO** — the third thing a person may say to a run, and the
/// only one they can take back (register item 9).
///
/// Its two neighbours both END the run and differ in what that costs: `cancel` loses the turn in
/// flight, `stand_down` banks the milestone. Neither is *wait, let me read this* — and
/// `ai_loop.scxml` has carried the edge for it (`hold` → `awaiting_human`) since R378 with nothing
/// in the product able to raise it.
///
/// ⚠ ADDITIVE, on [`STAND_DOWN_ACTION`]'s own terms and for its reason: an older client cannot
/// reach an address it does not know, so this earns no `WIRE_PROTOCOL` bump. A client newer than its
/// daemon gets `UnknownPath`, which is the daemon saying it does not serve that address.
pub const HOLD_RUN_ACTION: &str = "hold_run";
/// The slot reporting every run this daemon holds.
pub const RUNS_SLOT: &str = "runs";
/// The slot listing the plugins a `run` may name.
pub const PLUGINS_SLOT: &str = "plugins";
/// The slot publishing the guardrail bound a `run` that names none is given.
///
/// # Why a number a client could compile in is served over the wire
///
/// [`DEFAULT_MAX_ITERATIONS`] and its two siblings are this DAEMON's policy, and a client is not
/// necessarily this daemon's build — the whole argument `show-grammar` makes about the request
/// grammar, applied to the one fact a client needs in order to bound a loop it did not choose the
/// bounds for. The agent-facing mouth turns these into its CEILING (an agent may tighten a bound
/// and not loosen it), and a ceiling read from a constant compiled six weeks ago would be a
/// different ceiling from the one the daemon enforces.
pub const GUARDRAIL_DEFAULTS_SLOT: &str = "guardrail_defaults";

/// The REQUEST key carrying the consent LIST — [`Consents::WIRE_KEY`], re-exported.
///
/// # ⚠⚠ Why a projection rather than a literal at each mouth
///
/// The mouths that build a `run` call are not all able to depend on `sprag-plugin`: `sprag-mcp`
/// carries it as a DEV-dependency only, and a tool schema written from a literal `"asked"` would be
/// a second definition of a name whose whole job is to be the same word on both sides of the wire.
/// These three are `const` projections of the type that owns them, so a rename there is a compile
/// error here rather than a mouth that quietly stops matching.
///
/// ⚠ They are the REQUEST-side names and are deliberately separate from [`RUN_ASKED_KEY`], which
/// spells the same word about a different thing: that one is the QUESTION'S OWN LINES in an answer,
/// this one is the NEEDLE a caller sends. One is what the pane said; the other is what the caller
/// will accept. Merging them because they read alike is how two concepts come to move together.
pub const CONSENT_KEY: &str = Consents::WIRE_KEY;
/// The [`CONSENT_KEY`] ELEMENT'S needle naming WHICH QUESTION — [`Consent::ASKED_KEY`],
/// re-exported. ⚠ It lives inside one member of the list, not beside it.
pub const CONSENT_ASKED_KEY: &str = Consent::ASKED_KEY;
/// The [`CONSENT_KEY`] ELEMENT'S needle naming WHICH OPTION — [`Consent::ANSWER_KEY`],
/// re-exported.
pub const CONSENT_ANSWER_KEY: &str = Consent::ANSWER_KEY;

/// The answer key naming a run.
const RUN_ID_KEY: &str = "id";
/// The answer key carrying the pane whose occupant asked for a run — absent for a run nobody
/// claims, on [`sprag_terminal::Pane::opened_by`]'s terms.
const RUN_OPENED_BY_KEY: &str = "opened_by";
/// The answer key naming WHICH BUILD DROVE a run — absent when nothing recorded one.
///
/// # ⚠⚠⚠⚠⚠ What it is for: a walk is evidence about the daemon's build, not about the tree
///
/// A daemon outlives its clients, so the ordinary state after a day's work is a daemon running code
/// the tree has already replaced. Every other column here describes what a run DID; this one says
/// which code did it, and without it a run driven by a daemon that predates a fix reads exactly
/// like one that carries it (register item 438, measured 2026-08-18 at the cost of a round).
///
/// ⚠⚠ **Absent is not "this build".** It is a run restored from a log written before the field
/// existed — see [`crate::runs::RunSummary::build`]. Filling it in with the reader's own build
/// would date a dead daemon's work to its successor.
///
/// ⚠ An added ANSWER key earns no `WIRE_PROTOCOL` bump (that constant's own rule at version 5:
/// absent-not-wrong to an old reader), and no pin covers a slot's answer shape.
pub const RUN_BUILD_KEY: &str = "build";
/// The answer key naming WHICH GUARDRAIL exhausted a run — absent unless one did.
///
/// Its vocabulary is [`sprag_plugin::Ceiling`]'s own words, so the host never spells a variant and
/// a fourth ceiling reaches the wire by being added to that type.
pub const RUN_CEILING_KEY: &str = "ceiling";
/// The outcome key carrying WHAT THE PEER IS ASKING, present only on a run that ended `blocked`
/// and only where this host could read the question — see [`outcome_question`].
/// ⚠ ONE STRING, shared with the pane-level surface ([`crate::wire::ASKING_KEY`]) since R367: the
/// same question is published in both places and a caller moves between them.
pub const RUN_ASKING_KEY: &str = crate::wire::ASKING_KEY;
/// The [`RUN_ASKING_KEY`] member holding the question's own lines, in reading order.
pub const RUN_ASKED_KEY: &str = crate::wire::ASKED_KEY;
/// The [`RUN_ASKING_KEY`] member holding the options, in screen order — each `{number, label,
/// selected}`.
///
/// ⚠ `selected` is where a bare Enter would land, which is the difference between confirming a
/// tool call and declining it. Carried rather than left for a caller to infer.
pub const RUN_CHOICES_KEY: &str = crate::wire::CHOICES_KEY;
/// The [`RUN_ASKING_KEY`] member saying WHY the run did not answer, from
/// [`sprag_plugin::Refusal`]'s own words.
///
/// # ⚠⚠ Present on EVERY blocked run, including the ones with no question
///
/// It is the only member of `asking` that is never absent, and that is deliberate: a run that was
/// GIVEN a consent and stopped anyway looks identical to one that was given none, and the two have
/// completely different remedies — fix a needle, or write a consent. `unreadable` also carries the
/// case that has no question at all (`sprag_plugin::Unanswered::unreadable`), which was published
/// as an absence and explained nowhere until R366.
pub const RUN_WHY_KEY: &str = "why";
/// The outcome key counting HOW MANY of its peer's questions a run answered on the caller's
/// consent — always present, `0` for the runs that answered none.
///
/// # ⚠⚠ Why this is not absence-is-the-claim like its neighbours
///
/// [`RUN_CEILING_KEY`] and [`RUN_STOPPED_KEY`] are absent when they have nothing to say, because
/// their absence is *nothing of this kind happened* and a reader loses nothing. Here the absence
/// would be the same sentence a `0` is — and this is a count of DECISIONS TAKEN ON SOMEBODY'S
/// BEHALF, so *"this run answered nothing"* is a claim a reader must be able to get affirmatively
/// rather than by not finding a key.
pub const RUN_ANSWERED_KEY: &str = "answered";
/// The answer key carrying WHAT EACH STEP DID — the last [`sprag_plugin::JOURNAL_LIMIT`] of them.
///
/// A run reported its total and its terminal state and nothing about the steps between, so a loop
/// that failed to converge could not be diagnosed at all. ⚠ Compare its length against
/// `iterations` to tell a truncated journal from a complete one.
pub const RUN_JOURNAL_KEY: &str = "journal";
/// The answer key carrying WHAT BECAME OF THE WORK a run had going — absent unless the run was CUT
/// SHORT (cancelled, or out of time), which are the only endings that can land while a step is
/// still blocked on a peer this run set going.
///
/// ⚠⚠ Its presence is itself the claim, the rule [`RUN_CEILING_KEY`] follows. A `cancelled` outcome
/// with no answer here is consistent with two opposite states of the world — the work stopped, or
/// the work is still running and still spending — and the one a caller must act on is the second.
/// Its text is [`sprag_plugin::Stopped`]'s own sentence, so the host never spells a variant.
pub const RUN_STOPPED_KEY: &str = "stopped";

sprag_vt::closed_set! {
    /// WHERE A RUN HAS GOT TO — the `status` word inside a run's `state`.
    ///
    /// # ⚠⚠ Why this became a type on the round that added a word to it
    ///
    /// The four words were string literals inside `run_to_json`, so the vocabulary a peer decodes
    /// had no declaration anywhere — and `an_answers_value_space_cannot_widen_under_the_protocol_number`
    /// pins value spaces by walking each closed set's `ALL`. A vocabulary with no type is invisible
    /// to it: adding `interrupted` moved a value space a peer fails the WHOLE document on, and the
    /// pin that exists to catch exactly that could not see it. R353's mouse words were in this state
    /// in two crates; these were in one renderer.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RunStatus {
        /// A worker is still driving the plugin.
        Running,
        /// It reached a terminal state; `outcome` says which.
        Done,
        /// Its worker panicked (defensive — a plugin step should not).
        Panicked,
        /// ⚠ The daemon driving it died. Added at `WIRE_PROTOCOL` 21.
        Interrupted,
    }
}

impl RunStatus {
    /// This status's word on the wire — the ONE mapping, exhaustive so a fifth cannot reach a
    /// client without one.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Panicked => "panicked",
            Self::Interrupted => "interrupted",
        }
    }
}

sprag_vt::wire_words!(RunStatus: wire_str);

sprag_vt::closed_set! {
    /// WHICH BUNDLED PLUGIN a `run` names — the `plugin` discriminator's whole vocabulary.
    ///
    /// # Why the discriminator is a type
    ///
    /// It was a hand-written `const PLUGINS: &[&str]` beside a `match` over the same four string
    /// literals, so the list a client reads out of the `plugins` slot and the words `build_plugin`
    /// admits were two definitions of one vocabulary — the shape a fifth plugin is left out of, and
    /// the shape [`sprag_input::MouseButton`] was in until R353 (there in two crates, here in one
    /// file). They are one array now, and adding a variant reaches the wire in the compile that adds
    /// it.
    ///
    /// ⚠ Distinct from `PluginKind`, which CARRIES a built plugin: this is the NAME a request sends,
    /// and it exists on its own because a name is what a schema can publish and a built `Dialogue`
    /// is not.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum PluginName {
        /// Drive one pane with a stimulus until a sentinel appears.
        Orchestrator,
        /// Relay one pane's output into another's input.
        Pipe,
        /// Prompt an agent in a pane and collect its reply.
        Agent,
        /// Run two endpoints against each other, turn by turn.
        Dialogue,
        /// ANSWER the question one pane's peer has stopped to ask, once, and stop.
        ///
        /// ⚠ The one that is not a loop, and the reason it is a plugin at all rather than a
        /// synchronous verb: answering takes a keystroke, a look at what the peer did with it, and
        /// possibly a second keystroke — seconds of waiting close to the panes. A wire action doing
        /// that would block the serve loop; a run does it on its own thread and hands back
        /// everything the run registry already gives (an id, a cancel flag, a journal, and the
        /// count of decisions taken on somebody's behalf).
        Answer,
        /// ⚠⚠⚠ **RUN `ai_loop.scxml` AGAINST AN AGENT IN A PANE** — the outer loop, as a run
        /// somebody can start.
        ///
        /// The one plugin whose behaviour is AUTHORED rather than written in Rust: what it prompts
        /// with, when it stops, how many turns it may take and what it does with a blocked peer
        /// are a statechart document, and this is the driver that makes that document act on a
        /// pane. See [`sprag_plugin::ai_loop`].
        ///
        /// ⚠ It is the only form that takes a BRIEF, because it is the only plugin whose job is
        /// not in its arguments. An `agent` run carries the prompt it will send; a loop carries
        /// what it is FOR, and composes each turn's prompt from that in the document's own words.
        AiLoop,
    }
}

impl PluginName {
    /// HOW TO CALL THIS PLUGIN — the `run` form that selects it.
    ///
    /// # ⚠⚠ Why the form belongs to the type and not to a list beside it
    ///
    /// The four forms were a hand-written array in [`crate::wire::PluginGrammar`], and the type's
    /// doc above claimed a variant *"reaches the wire in the compile that adds it"*. That was true
    /// of the WORD and false of the form: a fifth plugin would have been published as a legal
    /// `plugin` value with nothing anywhere saying what to send it, and every gate over that table
    /// would have passed, because a gate over a declaration cannot see one nobody made.
    ///
    /// Exhaustive, so the compiler asks the question instead. `PluginGrammar::RUN` is now a
    /// projection of `ALL` through this.
    #[must_use]
    pub const fn form(self) -> sprag_rpc::CallForm {
        match self {
            Self::Orchestrator => crate::wire::PluginGrammar::ORCHESTRATOR_FORM,
            Self::Pipe => crate::wire::PluginGrammar::PIPE_FORM,
            Self::Agent => crate::wire::PluginGrammar::AGENT_FORM,
            Self::Dialogue => crate::wire::PluginGrammar::DIALOGUE_FORM,
            Self::Answer => crate::wire::PluginGrammar::ANSWER_FORM,
            Self::AiLoop => crate::wire::PluginGrammar::AI_LOOP_FORM,
        }
    }

    /// This plugin's word in a `run` request's `plugin`.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Pipe => "pipe",
            Self::Agent => "agent",
            Self::Dialogue => "dialogue",
            Self::Answer => "answer",
            // ⚠ `ai_loop` AND NOT `loop`, and the distinction is not decoration: every plugin on
            // this surface is a loop, so the shorter word would claim to be THE one. This is the
            // document's own name, which is what the whole tree already calls it.
            Self::AiLoop => "ai_loop",
        }
    }

    /// The plugin a `plugin` word names, or [`None`] for a word no plugin spells.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

sprag_vt::wire_words!(PluginName: wire_str);

/// The default iteration ceiling for a `run` that omits guardrails — never
/// unbounded (the README makes loop safety first-class), and the floor that
/// bounds every run regardless of its cost unit.
///
/// Published on [`GUARDRAIL_DEFAULTS_SLOT`], which is what makes it one number with one reader
/// rather than a constant every mouth compiles in for itself.
pub const DEFAULT_MAX_ITERATIONS: u32 = 100;
/// The default cost ceiling for a byte-relay plugin (Orchestrator/Pipe/Agent),
/// in injected PTY bytes.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024;
/// The default cost ceiling for the token-denominated Dialogue plugin, in real
/// input+output tokens (cache tokens are excluded — see `reply::parse_tokens`).
/// A COARSE backstop, not the primary bound: at the default 100-iteration cap
/// (~2k tokens/turn for a real dialogue) the iteration cap bites first, and a
/// print-mode Text dialogue reports `Tokens(0)` so only iterations bound it.
/// This ceiling exists to stop a single pathological high-token turn; tune it to
/// the model's pricing if a dollar-aware bound is ever needed.
pub const DEFAULT_MAX_TOKENS: u64 = 200_000;

/// THE DEFAULT WALL-CLOCK CEILING for a run that names none, in seconds — one hour.
///
/// Never absent, on [`DEFAULT_MAX_ITERATIONS`]'s exact terms: a run this daemon starts is bounded
/// in time whether or not the caller thought about time, because the README makes loop safety
/// first-class and a bound you have to remember is a bound somebody forgets.
///
/// # Why an hour, and why it is a backstop rather than the primary bound
///
/// The two per-step ceilings bite first for every plugin this build ships. A byte-relay run does
/// its hundred iterations in seconds. A dialogue's turn is bounded by
/// [`sprag_plugin::run::DEFAULT_REPLY_TIMEOUT`] at two minutes, so an hour is
/// roughly thirty full-length turns — beyond any dialogue the iteration ceiling allows to be slow.
/// What an hour catches is the case neither of the others can see: a run whose steps have stopped
/// making progress but have not stopped, which counts no iterations and spends nothing while it
/// holds a pane.
///
/// It is a CEILING as well as a default at the agent-facing mouth, so raising it for an agent is
/// the person's to do — see `tool_orchestrate`.
pub const DEFAULT_MAX_SECONDS: u64 = 3600;

/// The plugin host as a pinion `External`: starts background plugin runs over
/// the shared [`Workspace`] and reports their outcomes as scene-as-data.
pub struct PluginsExternal {
    workspace: Arc<Mutex<Workspace>>,
    runs: Arc<Mutex<RunRegistry>>,
    /// The daemon's opaque pane-exit death-signal ([`crate::spawn_reaper`]), or `None` off a
    /// daemon — passed to each pane a plugin spawns so it feeds the reaper. Registry-free, so
    /// carrying it does not breach the plugin layer's session-tree-free boundary.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The daemon's attention ROUTER ([`crate::attention`]), on exactly the terms above, so a pane a
    /// PLUGIN spawns can ask for a person like any other. `None` off a daemon.
    ///
    /// The router rather than one closure, for the reason [`crate::DaemonShared::attention`] states:
    /// a hook is minted per birth so the reader thread running it takes no lock.
    on_attention: Option<Arc<crate::attention::AttentionRouter>>,
    /// WHAT TO DO WHEN A RUN ENDS — the daemon's announce, as an opaque `Fn(RunId)`.
    ///
    /// # Why the loop's door needed this to be a door at all
    ///
    /// `orchestrate` exists so an agent does not spend its turns driving a loop. Without an event,
    /// the only way to learn a run finished is to ask again — and for an agent every ask is a turn,
    /// which is the cost the feature removes, paid one level up. So the worker announces.
    ///
    /// ⚠ **AN OPAQUE `Fn`, on the exact terms of the three hooks above it.** Announcing means
    /// naming a SESSION channel, and the session tree is what this surface is deliberately free of
    /// (Interface Segregation — see [`crate::workspace_scene`]). The scope that built this external
    /// closed over its own session name; what crosses the boundary is a call with a run id in it.
    on_run_end: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    /// The daemon's agent-state memory ([`crate::AgentClock`]), or `None` off a daemon — what lets
    /// a plugin SUPERVISE the agent in a pane instead of guessing from its text.
    ///
    /// The same memory the pane list reads, deliberately: a plugin holding a detector of its own
    /// would be a second authority answering the same question about the same pane, free to
    /// disagree with the row a person is looking at. It crosses into the plugin layer as an opaque
    /// `Fn` ([`agent_state_source`]), so that layer stays registry-free.
    agents: Option<Arc<crate::AgentClock>>,
}

impl PluginsExternal {
    /// Build the host over the shared workspace + run registry, plus the daemon's
    /// `on_pane_exit` death-signal (`None` off a daemon).
    #[must_use]
    pub fn new(
        workspace: Arc<Mutex<Workspace>>,
        runs: Arc<Mutex<RunRegistry>>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
        on_attention: Option<Arc<crate::attention::AttentionRouter>>,
        agents: Option<Arc<crate::AgentClock>>,
        on_run_end: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    ) -> Self {
        Self {
            workspace,
            runs,
            on_pane_exit,
            on_attention,
            on_run_end,
            agents,
        }
    }

    /// `run` action: build the named plugin, validate its target panes, spawn
    /// it on a background thread, and return its run id.
    fn run(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        // Build the plugin first: it determines the run's cost UNIT, which the
        // guardrails are then sized in (a bare `max_cost` is read in that unit).
        let (plugin, label) = self.build_plugin(map)?;
        let guardrails = parse_guardrails(map, plugin.default_cost())?;
        let opened_by = self.parse_opener(map)?;
        // WHO is in that seat, asked of the daemon rather than taken from the request — see
        // `session_in`. This is what survives the daemon, so it is resolved while the pane is still
        // here to answer.
        let opened_by_session = self.session_in(opened_by);
        let id = self.spawn_run(label, opened_by, opened_by_session, plugin, guardrails);
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// Parse the OPTIONAL `opened_by` — the pane whose occupant is asking for this run.
    ///
    /// The multiplexer's [`parse_opener`](crate::workspace::WorkspaceExternal) rule, verbatim and
    /// for its reason: a caller with a stale `SPRAG_PANE` — a process that outlived its own pane —
    /// would otherwise stamp a provenance naming a pane that does not exist, and nothing would ever
    /// prune it. A non-integer is a MALFORMED request; a pane this daemon does not hold is a
    /// well-formed one it will not honour.
    fn parse_opener(&self, map: &Map<String, Value>) -> Result<Option<u64>, InvokeError> {
        let opener = match map.get(RUN_OPENED_BY_KEY) {
            None | Some(Value::Null) => return Ok(None),
            Some(value) => value.as_u64().ok_or(InvokeError::TypeMismatch)?,
        };
        self.require_pane(PaneId(opener)).map_err(|_| {
            refused(format!(
                "no pane {opener} in this workspace, so nothing can be opened by it"
            ))
        })?;
        Ok(Some(opener))
    }

    /// **WHICH CONVERSATION IS SITTING IN `pane`**, or [`None`] when nothing agent-shaped is.
    ///
    /// Read HERE rather than sent by the caller, on `RunRecord::build`'s argument: it is a fact
    /// about what the daemon is holding, so letting it travel with the request would let a caller
    /// name a conversation it is not in and be answered that conversation's runs. The asker names
    /// its SEAT (`opened_by`, which this daemon then validates); who is in that seat is the
    /// daemon's to say.
    fn session_in(&self, pane: Option<u64>) -> Option<String> {
        let pane = pane?;
        lock(&self.workspace)
            .pane(PaneId(pane))
            .and_then(sprag_terminal::Pane::agent_session)
            .map(str::to_owned)
    }

    /// **WHICH SEAT IS CURRENTLY HOLDING `session`**, or [`None`] when nobody in this workspace is.
    ///
    /// The reverse of [`session_in`](Self::session_in), and the read side of
    /// [`crate::runs::RunRegistry::restore`]'s first rule: a restored run kept the conversation
    /// that asked for it and lost the seat, so the seat is found again here — from whoever is
    /// holding that conversation NOW.
    ///
    /// ⚠⚠⚠⚠ **A LEVEL, RE-DERIVED ON EVERY READ, NEVER A STAMP.** The moment `ai_loop`'s
    /// `restarting` replaces a session the pane holds a FRESH conversation, and this stops matching
    /// on its own — where a value written once at boot would go on claiming an owner that no longer
    /// exists, with nothing to correct it. The cost is a scan of one workspace's panes per read of
    /// the `runs` slot, which is the same order as the slot's own rendering.
    ///
    /// ⚠ Scoped to THIS workspace, which is the conservative half and deliberate: a conversation
    /// sitting in some other scope's pane is not something this reader can be answered about.
    fn seat_of(&self, session: &str) -> Option<u64> {
        lock(&self.workspace)
            .panes()
            .iter()
            .find(|pane| pane.agent_session() == Some(session))
            .map(|pane| pane.id().0)
    }

    /// `cancel` action: raise the cancel flag for run `id`. A synchronous
    /// `Rejected` if no run has that id; the run itself ends `Cancelled`
    /// asynchronously (observe it via `query("runs")`).
    fn cancel(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        if lock(&self.runs).cancel(RunId(id)) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(format!("no run {id} is in flight")))
        }
    }

    /// **ASK A RUN TO FINISH WHAT IT IS DOING AND THEN STOP** — [`STAND_DOWN_ACTION`].
    ///
    /// The one thing a person could say to a run used to be `cancel`, which stops it mid-turn and
    /// throws that turn away. This is the other sentence: the milestone the agent is working toward
    /// is finished, its account is taken, and the run converges. **The work is banked rather than
    /// lost**, which is the whole reason it is a different verb.
    ///
    /// ⚠ It only raises a flag. The worker carries it into the loop document at its next pass, and
    /// the DOCUMENT decides — at its own next milestone — what standing down means. Nothing here
    /// interrupts anything.
    fn stand_down(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        if lock(&self.runs).stand_down(RunId(id)) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(format!("no run {id} is in flight")))
        }
    }

    /// **HALT A RUN BETWEEN TURNS, OR LET IT GO AGAIN** — [`HOLD_RUN_ACTION`], and the word a person
    /// did not have (register item 9).
    ///
    /// `cancel` loses the turn and `stand_down` ends the run; neither of them is *wait, let me read
    /// this*. `ai_loop.scxml` has carried the edge for it since R378 with nothing able to raise it.
    ///
    /// ⚠⚠⚠ **IT TAKES `held` WHERE ITS TWO NEIGHBOURS TAKE NOTHING**, and that asymmetry is the
    /// meaning rather than an inconsistency: those two are LATCHES on purpose, and this is a level a
    /// person raises and lowers. The document's `resume` is the way back it was built with.
    ///
    /// ⚠ It only moves a flag. The worker carries it into the document at its next pass and the
    /// DOCUMENT decides; nothing here interrupts anything, and a held run is still running.
    fn hold_run(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        // ⚠ ABSENT MEANS *hold it* — the direction somebody typing this verb by hand almost always
        // means, and the one a caller that omitted the key cannot have meant to invert. Malformed is
        // refused rather than defaulted, this surface's rule for every optional it reads.
        let held = match map.get("held") {
            None | Some(Value::Null) => true,
            Some(value) => value.as_bool().ok_or(InvokeError::TypeMismatch)?,
        };
        if lock(&self.runs).hold(RunId(id), held) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(format!("no run {id} is in flight")))
        }
    }

    /// Parse the plugin discriminator + its args, validating target panes
    /// exist (fail fast → synchronous `Rejected`).
    fn build_plugin(&self, map: &Map<String, Value>) -> Result<(PluginKind, String), InvokeError> {
        // THROUGH THE TYPE, so a word this refuses is a word the wire does not publish. ⚠ A word no
        // plugin spells is a MALFORMED request (`TypeMismatch`), not a rejected one: that is this
        // wire's taxonomy for every other closed vocabulary it reads, and it was the odd one out here
        // — `refused("this daemon has no plugin called …")` carried a friendlier message and put a
        // grammar refusal in the class reserved for "read, and could not be honoured". The message's
        // job belongs to the published vocabulary now, and the completeness gate can only SEE a
        // vocabulary that refuses as malformed.
        let named =
            PluginName::from_wire(require_str(map, "plugin")?).ok_or(InvokeError::TypeMismatch)?;
        match named {
            PluginName::Orchestrator => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                let stimulus = require_str(map, "stimulus")?.to_string();
                let sentinel = opt_str(map, "sentinel")?.map(str::to_string);
                let ready_when = opt_ready_when(map)?;
                let ready_within = opt_millis(map, Readiness::WIRE_KEY)?;
                let label = format!("orchestrator pane={}", pane.0);
                let spec = OrchestrationSpec {
                    stimulus,
                    sentinel,
                    ready_when,
                    ready_within,
                    may_answer: opt_may_answer(map)?,
                    attended: opt_attended(map)?,
                    turn: opt_turn(map)?,
                };
                Ok((
                    PluginKind::Orchestrator(Orchestrator::new(pane, spec)),
                    label,
                ))
            }
            PluginName::Pipe => {
                let src = require_pane_id(map, "src")?;
                let dst = require_pane_id(map, "dst")?;
                self.require_pane(src)?;
                self.require_pane(dst)?;
                let spec = PipeSpec {
                    src,
                    dst,
                    ready_when: opt_ready_when(map)?,
                    ready_within: opt_millis(map, Readiness::WIRE_KEY)?,
                    may_answer: opt_may_answer(map)?,
                    attended: opt_attended(map)?,
                };
                Ok((
                    PluginKind::Pipe(Pipe::new(spec)),
                    format!("pipe {}->{}", src.0, dst.0),
                ))
            }
            PluginName::Agent => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                let prompt = require_str(map, "prompt")?.to_string();
                let mut spec = AgentSpec::new(prompt);
                if !declined(map, "eof") {
                    // `Some`, and the wrapper carries meaning: a caller who SAID so overrides what
                    // the completion contract would have implied — see `AgentSpec::eof`.
                    spec.eof = Some(map["eof"].as_bool().ok_or(InvokeError::TypeMismatch)?);
                }
                if !declined(map, "shows_prompt") {
                    spec.shows_the_prompt = map["shows_prompt"]
                        .as_bool()
                        .ok_or(InvokeError::TypeMismatch)?;
                }
                if let Some(timeout) = opt_millis(map, "timeout_ms")? {
                    spec.timeout = timeout;
                }
                if let Some(done_when) = opt_done_when(map)? {
                    spec.done_when = done_when;
                }
                spec.ready_when = opt_ready_when(map)?;
                spec.ready_within = opt_millis(map, Readiness::WIRE_KEY)?;
                spec.may_answer = opt_may_answer(map)?;
                spec.attended = opt_attended(map)?;
                let label = format!("agent pane={}", pane.0);
                Ok((PluginKind::Agent(Agent::new(pane, spec)), label))
            }
            PluginName::Dialogue => {
                // Dialogue creates its own per-turn panes, so there is no target
                // pane to validate; the endpoints are argv templates.
                let endpoint_a = require_string_array(map, "endpoint_a")?;
                let endpoint_b = require_string_array(map, "endpoint_b")?;
                let seed = require_str(map, "seed")?.to_string();
                let mut spec = DialogueSpec::new(endpoint_a, endpoint_b, seed);
                // The wire keys stay flat (endpoint_a/label_a/format_a) — the
                // Endpoint struct is an in-Rust cohesion fix, not a protocol
                // change; the host bridges the flat keys into endpoints[0/1].
                if let Some(label) = opt_str(map, "label_a")? {
                    spec.endpoints[0].label = label.to_string();
                }
                if let Some(label) = opt_str(map, "label_b")? {
                    spec.endpoints[1].label = label.to_string();
                }
                if let Some(format) = parse_reply_format(map, "format_a")? {
                    spec.endpoints[0].format = format;
                }
                if let Some(format) = parse_reply_format(map, "format_b")? {
                    spec.endpoints[1].format = format;
                }
                let (default_cols, default_rows) = lock(&self.workspace).default_size();
                spec.cols = opt_dim(map, "cols")?.unwrap_or(default_cols);
                spec.rows = opt_dim(map, "rows")?.unwrap_or(default_rows);
                if let Some(timeout) = opt_millis(map, "timeout_ms")? {
                    spec.timeout = timeout;
                }
                // ⚠ NO readiness barrier here, and the absence is measured rather than an
                // oversight: a dialogue passes each turn's prompt as an ARGV ARGUMENT of the pane
                // it spawns for that turn and never injects a byte, so there is no window in which
                // a shell could be typed into. The three plugins that DO inject all take one.
                let label = format!(
                    "dialogue {}<->{}",
                    spec.endpoints[0].argv.first().map_or("?", String::as_str),
                    spec.endpoints[1].argv.first().map_or("?", String::as_str),
                );
                Ok((PluginKind::Dialogue(Box::new(Dialogue::new(spec))), label))
            }
            PluginName::Answer => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                // ⚠⚠ REQUIRED, alone among the forms — see
                // [`PluginGrammar::MUST_ANSWER`](crate::wire::PluginGrammar::MUST_ANSWER). A run
                // with nothing to answer would occupy a run slot to do what not calling does.
                // Read through the SAME parser the optional key uses, so the two spellings of this
                // contract cannot come to admit different objects.
                let consent = opt_may_answer(map)?.ok_or_else(|| {
                    refused(format!(
                        "an `answer` run needs a {} — [{{{}: …, {}: …}}], quoting the peer's own \
                         words. Without one there is nothing it may type, which is what not \
                         calling it already does.",
                        Consents::WIRE_KEY,
                        Consent::ASKED_KEY,
                        Consent::ANSWER_KEY,
                    ))
                })?;
                let label = format!("answer pane={}", pane.0);
                Ok((
                    PluginKind::Answer(sprag_plugin::Answer::new(pane, consent)),
                    label,
                ))
            }
            PluginName::AiLoop => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                // ⚠⚠⚠ THE CONSTRUCTION SITE THE OUTER DRIVER'S DOC HAS NAMED SINCE R378. Building a
                // concrete `IScriptEngine` here is what made `sce-rust-lua` a real dependency of
                // this crate; the manifest carries the argument. It is per RUN and not shared: a
                // datamodel is a run's own state, and two loops sharing one interpreter would be two
                // runs sharing their north star.
                let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
                    Arc::new(sce_rust_lua::LuaEngine::new());
                // ⚠⚠⚠ AND THE DECISIONS THIS REPOSITORY'S RUNS RUN UNDER, read off THIS
                // repository's own document.
                //
                // The template used to author them, which meant sprag's standing yesses authorised
                // every run of a file other repositories copy. They moved to `debt_loop.scxml`, so
                // something has to carry them across — and this is that something. **It decides
                // nothing**: it reads one document and hands the values to another, which is the
                // whole of what the governing rule permits a driver to do with a decision.
                //
                // ⚠⚠ WHICH KIND IS NOT A WIRE ARGUMENT YET, and that is scope rather than design.
                // There is one kind, so naming it would be a key with one legal value — and adding
                // an ARGUMENT is a wire bump. The day a second kind exists, that bump is what pays
                // for it.
                let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
                    .map_err(|why| refused(why.to_string()))?;
                // ⚠⚠⚠⚠⚠ RESOLVED BY A FUNCTION THAT HANDS THE BRIEF BACK — register item 492. It
                // was a hundred inline lines here, and the eight fall-throughs to the kind document
                // inside it were held by NOTHING: `sprag_plugin`'s own gate had already measured
                // that deleting one of them left the whole workspace green, and the ceiling's round
                // measured it again with the same answer. A `Brief` is the observable that fixes
                // that, and this is the only call site.
                let brief = ai_loop_brief(map, &kind)?;
                // ⚠ THE AGENT'S NAME IS REQUIRED and the barrier is derived from it, because a
                // loop's first prompt goes into a pane whose program may still be starting — see
                // `AI_LOOP_FORM`. A caller whose peer needs a different barrier overrides it.
                let mut spec = sprag_plugin::AiLoopSpec::driving(require_str(map, "agent")?);
                if let Some(ready_when) = opt_ready_when(map)? {
                    spec.ready_when = Some(ready_when);
                }
                // ⚠⚠ READ AS TWO INDEPENDENT KEYS, where the `agent` form's `opt_turn` refuses a
                // bound with no `done_when` beside it. That rule is right there and wrong here:
                // an `agent` run's default contract is `exits`, so a bare bound would be bounding
                // something the caller did not choose — a loop's default is
                // `INNER_SESSION_ENDS`, the contract this document makes load-bearing, so a bare
                // bound bounds exactly the turn the caller is thinking about.
                // ⚠⚠⚠ AND THE INDEPENDENCE IS NOW STRUCTURAL RATHER THAN A CHOICE MADE HERE: the
                // bound cannot be spelled on this spec at all, so the two keys could not be read
                // together even by a caller who wanted them to be.
                spec.done_when = opt_done_when(map)?.unwrap_or(sprag_plugin::INNER_SESSION_ENDS);
                // ⚠⚠⚠⚠⚠ WHERE THIS RUN'S REVIEWS KEEP THEIR COUNTS, AND THIS IS THE ONLY PLACE
                // THAT KNOWS. `sprag-plugin` used to read `$XDG_STATE_HOME` itself, one library
                // down, which made *the daemon's state directory* mean *the home of whoever ran
                // the process* — so the whole suite appended to a developer's `~/.local/state`
                // (measured 2026-08-19: thirty lines from one crate, the write CI's
                // `ambient-home-guard` was red on). The derivation is
                // [`crate::durability::state_dir`], the one this daemon files every other durable
                // artifact under, so the counts land beside the snapshot and the run registry
                // rather than in a second directory of their own.
                //
                // ⚠⚠ NOT a wire key. A caller does not choose where this machine keeps its files,
                // and the document already owns the two decisions that ARE a caller's: whether to
                // keep counts at all and what to call the file (`ledger_into`, which overrides
                // this outright when it is authored absolute).
                spec.review_ledger = Some(crate::durability::state_dir());
                if !declined(map, "shows_prompt") {
                    spec.shows_the_prompt = map["shows_prompt"]
                        .as_bool()
                        .ok_or(InvokeError::TypeMismatch)?;
                }
                // ⚠⚠⚠ THE ANSWERING CONTRACT, read through the SAME two parsers every other
                // injecting form uses. A loop is the form that needs it most and was the only one
                // without it: every kind of real work its agent does raises a permission dialog,
                // and a loop that met one with nothing declared stopped having judged no turns.
                // ⚠⚠⚠ IT IS ON THE BRIEF NOW, not the spec: a consent is a decision somebody made
                // in advance and in writing, which is what this document holds — the same move
                // `screen_rules` made, and the end of refusal and approval living in two worlds.
                let label = format!("ai_loop pane={}", pane.0);
                let loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
                    .map_err(|why| refused(ai_loop_refusal(&why)))?;
                Ok((PluginKind::AiLoop(Box::new(loops)), label))
            }
        }
    }

    fn require_pane(&self, pane: PaneId) -> Result<(), InvokeError> {
        if lock(&self.workspace).pane(pane).is_some() {
            Ok(())
        } else {
            Err(refused(format!("no pane {} in this workspace", pane.0)))
        }
    }

    /// Spawn the plugin on a background thread that drives it to a terminal
    /// state and writes that into a shared cell; register it.
    fn spawn_run(
        &self,
        label: String,
        opened_by: Option<u64>,
        opened_by_session: Option<String>,
        mut plugin: PluginKind,
        guardrails: Guardrails,
    ) -> RunId {
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        // The cancel flag is shared two ways: the run's RunContext reads it, and
        // the registry holds a clone so a `cancel`/shutdown can set it.
        let cancel = Arc::new(AtomicBool::new(false));
        // ⚠⚠ THE SECOND THING A PERSON CAN SAY TO A RUN, and it needs its own flag: *finish what you
        // are doing and then stop* is not a softer cancel, it is the opposite outcome — the turn in
        // flight is banked rather than lost. See `RunRecord::order`.
        let order = Arc::new(AtomicBool::new(false));
        // ⚠⚠⚠ AND THE THIRD, which is the only one a person can take back — see `RunRecord::hold`.
        // A flag of its own rather than a mode on `order` because that one is a latch by design.
        let hold = Arc::new(AtomicBool::new(false));
        let run_ctx = RunContext::new(Arc::clone(&cancel))
            .ordered_by(Arc::clone(&order))
            .held_by(Arc::clone(&hold));
        let access = WorkspacePaneAccess::new(Arc::clone(&self.workspace))
            .with_pane_exit(self.on_pane_exit.clone())
            // The detector, as an opaque per-pane read. A run that never supervises never calls
            // it, and a host that has none hands `None` — which is what makes "this build cannot
            // supervise" a different answer from "this pane is not an agent".
            .with_agent_state(self.agents.as_ref().map(|agents| {
                agent_state_source(
                    Arc::clone(&self.workspace),
                    Arc::clone(agents),
                    crate::config::agent_settle,
                )
            }))
            // The router becomes a MINTER at this boundary: the plugin layer asks for a hook per
            // pane and never learns what a router is, which is the same opaque-`Fn` discipline the
            // death signal beside it follows.
            .with_attention(self.on_attention.as_ref().map(|router| {
                let router = Arc::clone(router);
                Arc::new(move || router.signal()) as sprag_plugin::access::AttentionMinter
            }));
        let on_end = self.on_run_end.clone();
        // The id BEFORE the thread, because the announcement names it and the worker cannot ask the
        // registry for its own id without taking the lock the registry is being written under.
        let id = lock(&self.runs).reserve();
        // The cell the driver writes its counters into, shared with the registry so `runs` can
        // answer them while the run is still spending.
        let progress = sprag_plugin::ProgressCell::default();
        let worker_progress = Arc::clone(&progress);
        let handle = thread::spawn(move || {
            let outcome = Driver::new(guardrails).reporting_to(worker_progress).run(
                plugin.as_plugin(),
                &access,
                &run_ctx,
            );
            // The worker still owns the plugin after the run, so it can read any
            // content the plugin captured (an AI adapter's reply) for the host.
            let output = plugin.as_plugin().captured();
            *lock(&worker_state) = RunState::Done {
                outcome: Box::new(outcome),
                output,
            };
            // ⚠ AFTER the state is written, never before: a client woken by this asks `runs`
            // immediately, and an announcement that raced the write would answer `running` about a
            // run the wake said had finished — the client would then park again on an event that
            // has already fired. The order is the whole correctness of the wake.
            if let Some(announce) = on_end {
                announce(id);
            }
        });
        lock(&self.runs).submit(crate::runs::NewRun {
            id,
            label,
            opened_by,
            opened_by_session,
            state,
            handle,
            progress,
            cancel,
            order,
            hold,
        })
    }
}

/// The daemon's detector, as the opaque per-pane read the plugin layer takes.
///
/// # Why this is a closure and not a type the plugin crate could hold
///
/// The verdict a plugin reads has to be the SAME verdict the pane list shows a person, or a
/// supervisor and a human looking at one pane are being told different things about it. That
/// arbitration lives in [`crate::AgentClock`], which sits beside the session tree — and the plugin
/// layer is session-tree-free by decision. Handing across an `Fn(PaneId)` keeps both: one
/// authority, and a substrate that still knows nothing about registries, manifests or settle
/// windows. It is the discipline the pane-exit and attention hooks beside it already follow.
///
/// # What it does per call, and what it does not
///
/// A pull, and it is meant to be pulled: the screen is read under the workspace lock (the detector's
/// own lock nested inside it, never the reverse — the order [`crate::WorkspaceExternal`] documents),
/// and [`AgentClock::observe`](crate::AgentClock::observe) applies the quiescence gate, so a pane
/// whose screen and title have not moved costs no rule evaluation however often a plugin steps.
///
/// The QUESTION is parsed only for a pane that is actually blocked. That is not only thrift: a menu
/// still painted behind a working agent is scenery, and handing it to a supervisor would invite an
/// answer to a question nobody asked. It is read in [`sprag_detect::DIALOG_WINDOW`], the window the
/// built-in manifests block in — a user manifest that declares a wider one may block on a menu this
/// does not enumerate, and the supervisor then sees `asking: None` and hands the pane to a person,
/// which is the right answer to a question it cannot read.
/// # Why the settle window is a parameter
///
/// It is [`crate::config::agent_settle`] on every real host, and it is INJECTED for the reason R331
/// recorded against `window_size`: the only other way in is `$XDG_CONFIG_HOME`, which is
/// process-global, so a test of this path would otherwise assert whatever the developer's
/// `config.toml` happens to say — and a test whose subject is a TIMED transition would be asserting
/// it about a timing it did not choose.
/// ⚠ VISIBLE TO THE CRATE so the live-agent measurement can drive the loop through the REAL
/// detector — see `crate::live_agent`. It is still built in one place and handed out as an opaque
/// `Fn`, which is the property that mattered; what changed is that the one other reader in this
/// crate is a gate rather than the run path.
pub(crate) fn agent_state_source(
    workspace: Arc<Mutex<Workspace>>,
    agents: Arc<crate::AgentClock>,
    window: fn() -> sprag_detect::Hysteresis,
) -> sprag_plugin::AgentStateSource {
    Arc::new(move |id: PaneId| {
        let guard = lock(&workspace);
        let pane = guard.pane(id)?;
        // The CHILD's own title, never the pane's name — the rule the pane list states and for its
        // reason: a name is chosen by whoever asked for the pane, so reading one here would let
        // anyone who can name a pane forge an agent identity.
        let title = pane.title();
        pane.pty().with_screen(|screen| {
            let facts = agents.observe(
                id,
                screen,
                title.as_deref(),
                std::time::Instant::now(),
                window,
            )?;
            let state = sprag_detect::AgentState::from_wire(facts.state)?;
            let authority = match facts.source {
                Some(source) => sprag_plugin::Authority::Reported { source },
                None => sprag_plugin::Authority::Scraped { rule: facts.rule },
            };
            Some(sprag_plugin::AgentObservation {
                // The REGISTRY's parse, not a second one taken here. It reads the same screen at the
                // same instant, and having two sites derive it is how the run surface and the pane
                // surface would come to disagree about what one pane is asking (R367 moved it).
                asking: facts.asking,
                // The agent's own account, carried through untouched — see `AgentObservation::asked`
                // for what a supervisor can do with it that no screen read can.
                asked: facts.asked,
                // ⚠⚠⚠⚠ AND WHAT IT ANSWERED — the half a driver was reading off a pane that cannot
                // be read for it (register item 441). Carried through untouched, exactly like the
                // question above: this layer states, and the plugin judges.
                said: facts.said,
                // ⚠⚠⚠⚠⚠ AND WHY IT WANTS A PERSON — the half `asking` above is `None` for, which is
                // precisely the case a run has to hand to one (register item 452). Carried through
                // untouched on the same terms: this layer states, the plugin decides what to do about
                // it, and neither invents a sentence the peer did not say.
                noticed: facts.noticed,
                transcript: facts.transcript,
                state,
                agent: facts.agent,
                authority,
                seq: facts.seq,
                // ⚠⚠⚠ AND THE COUNT OF QUESTIONS BESIDE THE COUNT OF STATE CHANGES — register item
                // 441. They move for two different reasons and a supervisor needs the second one:
                // `seq` cannot say whether the peer took the prompt just typed at it, because a
                // submit into an already-`working` pane publishes nothing.
                asked_seq: facts.asked_seq,
                // ⚠⚠⚠⚠⚠ AND THE COUNT THAT MOVES WHILE A TURN IS MERELY WORKING — register item
                // 458. The three counters beside it stand still through a turn calling tools, which
                // reads exactly like a turn nothing will ever end; this one is the peer's reporter
                // being alive, carried through untouched like every other stated fact here.
                reports: facts.reports,
                // ⚠⚠ AND THE COUNT THAT DATES THE ANSWER, without which the text above cannot be
                // told from the previous turn's — see `AgentObservation::said_seq`.
                said_seq: facts.said_seq,
            })
        })
    })
}

impl fmt::Debug for PluginsExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginsExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(PluginsExternal);

impl PluginsExternal {
    fn read(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            RUNS_SLOT => {
                // ⚠ The snapshot is taken and the registry lock RELEASED before any seat is
                // re-derived: `seat_of` takes the workspace lock, and holding the run registry
                // across it would invert the workspace-then-registry order the host keeps.
                let runs = {
                    let mut registry = lock(&self.runs);
                    registry.sweep(); // reap finished threads before reporting
                    registry.snapshot()
                };
                let entries = runs
                    .iter()
                    .map(|run| {
                        // A run THIS daemon issued already names its seat. One it inherited kept
                        // only the conversation, so the seat is found again from whoever holds that
                        // conversation now — `seat_of`, and see `RunRegistry::restore`'s rule 1.
                        let seat = run.opened_by.or_else(|| {
                            run.opened_by_session
                                .as_deref()
                                .and_then(|session| self.seat_of(session))
                        });
                        run_to_json(run, seat)
                    })
                    .collect();
                Some(IntrospectValue::Json(Value::Array(entries)))
            }
            // The same array the `run` grammar publishes as its `plugin` vocabulary —
            // one definition, two readers.
            PLUGINS_SLOT => Some(IntrospectValue::Json(json!(PluginName::WIRE_WORDS))),
            // THE BOUND A RUN THAT NAMES NONE IS GIVEN, keyed exactly as the `guardrails` argument
            // spells it, so a client that reads a ceiling here can send it back without a mapping.
            GUARDRAIL_DEFAULTS_SLOT => Some(IntrospectValue::Json(json!({
                "max_iterations": DEFAULT_MAX_ITERATIONS,
                "max_seconds": DEFAULT_MAX_SECONDS,
                "max_bytes": DEFAULT_MAX_BYTES,
                "max_tokens": DEFAULT_MAX_TOKENS,
            }))),
            // HOW TO CALL THIS SURFACE'S TWO VERBS — its own `PLUGINS_GRAMMAR`, answered by
            // the surface that serves them (see `ACTION_GRAMMAR_SLOT`).
            crate::wire::ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(
                crate::wire::ActionGrammar::answer(crate::wire::PLUGINS_GRAMMAR),
            )),
            _ => None,
        }
    }
}

impl ExternalIntrospect for PluginsExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::action(RUN_ACTION, "action"),
                    SchemaField::action(CANCEL_ACTION, "action"),
                    SchemaField::action(STAND_DOWN_ACTION, "action"),
                    SchemaField::action(HOLD_RUN_ACTION, "action"),
                    SchemaField::new(RUNS_SLOT, "list"),
                    SchemaField::new(PLUGINS_SLOT, "list"),
                    SchemaField::new(GUARDRAIL_DEFAULTS_SLOT, "object"),
                    SchemaField::new(crate::wire::ACTION_GRAMMAR_SLOT, "object"),
                ]
            },
        )
    }

    /// ⚠⚠ **THE IDENTITY MIGRATION, and `UnknownPath` is what a `None` ALWAYS MEANT.**
    ///
    /// pinion R1674 widened a read's failure from an absence into a REFUSAL with a reason
    /// (`ReadRefusal`), and its dispatch maps `UnknownPath` onto the very fault a `None` produced
    /// before it (`QueryError::UnknownIntrospectPath`). So wrapping the reading below preserves
    /// this surface's wire behaviour exactly, which is what a pin bump owes its callers.
    ///
    /// ⚠ The three RICHER arms — `NoSuchMember`, `Unavailable`, `QueryTypeMismatch` — are the
    /// point of the upstream change and are NOT adopted here. Each is a per-path decision about
    /// what this surface knows, and several of them supersede reasoning this file already wrote
    /// down; taking them in the same edit as a pin bump would ship refusal sentences nobody
    /// derived. Registered as owed rather than guessed.
    fn query(&self, path: &str) -> Result<IntrospectValue, ReadRefusal> {
        self.read(path).ok_or(ReadRefusal::UnknownPath)
    }

    /// The reading itself — see [`query`](Self::query) for why it still answers an
    /// `Option` and what that `None` becomes.
    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots: starting a run is an action (invoke `run`).
        Err(InterveneError::UnknownPath)
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            RUN_ACTION => self.run(&args),
            CANCEL_ACTION => self.cancel(&args),
            STAND_DOWN_ACTION => self.stand_down(&args),
            HOLD_RUN_ACTION => self.hold_run(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// A bundled plugin chosen at `run` time. An enum (not `Box<dyn Plugin>`) so the
/// worker thread moves a concrete `Send` value and the match stays explicit.
enum PluginKind {
    Orchestrator(Orchestrator),
    Pipe(Pipe),
    Agent(Agent),
    // Boxed: a `Dialogue` carries two embedded SCE session engines, so it is far
    // larger than the byte-relay plugins; boxing the one big variant keeps the
    // enum small instead of every value paying its footprint.
    Dialogue(Box<Dialogue>),
    Answer(sprag_plugin::Answer),
    // Boxed for the `Dialogue` reason above: an `AiLoop` owns a compiled `ai_loop.scxml` engine
    // and the script interpreter its datamodel lives in.
    AiLoop(Box<sprag_plugin::AiLoop>),
}

impl PluginKind {
    fn as_plugin(&mut self) -> &mut dyn Plugin {
        match self {
            PluginKind::Orchestrator(orchestrator) => orchestrator,
            PluginKind::Pipe(pipe) => pipe,
            PluginKind::Agent(agent) => agent,
            PluginKind::Dialogue(dialogue) => dialogue.as_mut(),
            PluginKind::Answer(answer) => answer,
            PluginKind::AiLoop(loops) => loops.as_mut(),
        }
    }

    /// This plugin's default cost ceiling, in its natural unit: the byte-relay
    /// plugins spend injected bytes; the dialogue spends LLM tokens. The unit
    /// also sizes a bare `max_cost` from the wire.
    fn default_cost(&self) -> Cost {
        match self {
            PluginKind::Orchestrator(_)
            | PluginKind::Pipe(_)
            | PluginKind::Agent(_)
            // ⚠ Bytes, and the ceiling never binds: the most an answer can spend is two
            // keystrokes. It is here because a run's cost unit is its plugin's, and a plugin with
            // no unit would be a hole in the one guarantee the guardrails make.
            | PluginKind::Answer(_)
            // ⚠ BYTES, and it is the loop's real currency rather than a fallback: what an
            // `ai_loop` spends on its peer is the prompts it types, and the model's tokens are
            // spent by the AGENT in the pane, which this daemon neither bills nor can count. The
            // budget that bounds an agent's spend is `max_turns`, and it is in the brief.
            | PluginKind::AiLoop(_) => Cost::Bytes(DEFAULT_MAX_BYTES),
            PluginKind::Dialogue(_) => Cost::Tokens(DEFAULT_MAX_TOKENS),
        }
    }
}

/// A required argv array (`["program", "args"…]`) of strings, non-empty.
/// A missing/non-array value is a [`InvokeError::TypeMismatch`]; an empty array
/// is a [`InvokeError::Rejected`] (an endpoint needs at least its program).
/// Read an optional millisecond duration argument.
///
/// One spelling for the three `*_ms` arguments a run form takes, so a bound named on the wire is
/// converted the same way wherever it is named. A present-but-not-a-number value is a MALFORMED
/// request rather than a silently ignored one — the class R358 closed for argument NAMES, held
/// here for their values.
/// Read the optional `ready_when` barrier — an object naming WHICH QUESTION its marker asks.
///
/// # ⚠⚠ A bare string is REFUSED, deliberately
///
/// This argument was a needle, and the needle was matched against the whole screen — satisfied by
/// text that was already there, most often the ECHO OF THE COMMAND LINE THAT STARTED THE PROGRAM.
/// Reading an old caller's string as either of the two kinds would answer their question with the
/// other one and never say so, which is the silent-reinterpretation failure the wire refuses
/// everywhere else. The shape is what moved, so `WIRE_PROTOCOL` moved with it (21 → 22) and a
/// pre-bump call meets a grammar refusal at the door.
///
/// A word outside [`ReadyWhen::WIRE_WORDS`] is MALFORMED rather than rejected — R353's rule, and
/// what lets a completeness probe SEE that the vocabulary is closed.
fn opt_ready_when(map: &Map<String, Value>) -> Result<Option<ReadyWhen>, InvokeError> {
    if declined(map, "ready_when") {
        return Ok(None);
    }
    let object = map["ready_when"]
        .as_object()
        .ok_or(InvokeError::TypeMismatch)?;
    let matched = require_str(object, "match")?;
    let marker = require_str(object, "marker")?.to_string();
    ReadyWhen::parse(matched, marker)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `may_answer` consents — WHAT THIS RUN MAY ANSWER if its peer stops to ask.
/// Absent (or `null`) is a run that answers nothing, which is what every run did before the key
/// existed.
///
/// ⚠⚠ **BOTH NEEDLES ARE REQUIRED AND NEITHER MAY BE EMPTY.** An empty `asked` is carried by every
/// question and an empty `answer` by every option, so each of them turns a narrow consent into
/// something else — see [`Consent::parse`](sprag_plugin::Consent::parse), which owns the predicate
/// so the parser and the publication cannot drift. A caller who sends one has made a MALFORMED
/// request (R353's rule), which is why this is a `TypeMismatch` rather than a friendly refusal.
///
/// # ⚠⚠⚠ A LIST, and an EMPTY one is malformed rather than an omission
///
/// One turn asks more than one question, so the value is an ARRAY of clauses — see
/// [`PluginGrammar::MAY_ANSWER`](crate::wire::PluginGrammar::MAY_ANSWER) for the measurement. The
/// empty array is refused rather than read as *"no consent"*: `[]` and an absent key would then be
/// two spellings of one meaning, and the one that arrives by accident — a client that built its
/// clause list from a filter and matched nothing — is exactly the one a caller would want told
/// about. [`Consents::of`](sprag_plugin::Consents::of) owns that predicate, as `Consent::parse`
/// owns the needle's.
fn opt_may_answer(map: &Map<String, Value>) -> Result<Option<Consents>, InvokeError> {
    if declined(map, Consents::WIRE_KEY) {
        return Ok(None);
    }
    let listed = map[Consents::WIRE_KEY]
        .as_array()
        .ok_or(InvokeError::TypeMismatch)?;
    let mut clauses = Vec::with_capacity(listed.len());
    for clause in listed {
        let object = clause.as_object().ok_or(InvokeError::TypeMismatch)?;
        let asked = require_str(object, Consent::ASKED_KEY)?.to_string();
        let answer = require_str(object, Consent::ANSWER_KEY)?.to_string();
        clauses.push(Consent::parse(asked, answer).ok_or(InvokeError::TypeMismatch)?);
    }
    Consents::of(clauses)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `screen_rules` — WHAT THIS LOOP TURNS DOWN AND WHAT IT SAYS INSTEAD.
///
/// [`opt_may_answer`]'s shape, for the other authority: a consent takes an option the peer OFFERED,
/// and a screen rule refuses the call and redirects the agent in words. Absent (or `null`) is
/// [`None`], which the loop reads as *"keep whatever the document's author wrote"* — NOT as an
/// empty list, which is why [`ScreenRules`] cannot be empty and an empty array is malformed here.
///
/// ⚠⚠ A rule's own refusals are the plugin's ([`sprag_plugin::Malformed`]) and reach the caller as a
/// type mismatch, exactly as a `Consent` with an empty needle does. A rule that claims every dialog
/// would refuse every tool call the agent ever asks about, so the door is where it is turned away.
fn opt_screen_rules(map: &Map<String, Value>) -> Result<Option<ScreenRules>, InvokeError> {
    if declined(map, ScreenRules::WIRE_KEY) {
        return Ok(None);
    }
    let listed = map[ScreenRules::WIRE_KEY]
        .as_array()
        .ok_or(InvokeError::TypeMismatch)?;
    let mut rules = Vec::with_capacity(listed.len());
    for rule in listed {
        let object = rule.as_object().ok_or(InvokeError::TypeMismatch)?;
        let when = require_str(object, ScreenRule::WHEN_KEY)?.to_string();
        let text = require_str(object, ScreenRule::TEXT_KEY)?.to_string();
        rules.push(ScreenRule::parse(when, text).map_err(|_| InvokeError::TypeMismatch)?);
    }
    ScreenRules::of(rules)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `await_person_ms` — WHETHER ANYBODY IS WATCHING the pane this run drives, and
/// for how long. Absent (or `null`) is [`Attended::NoOne`], which is what every run did before the
/// key existed and is the conservative half of the contract.
///
/// ⚠⚠ **ZERO IS MALFORMED, not a quiet `NoOne`** — [`opt_may_answer`]'s empty-array rule exactly,
/// and for the same reason: two spellings of one behaviour make the caller who reached the first by
/// arithmetic (a deadline already past, a config that defaulted to 0) silently get the other.
/// [`Attended::of`] owns the predicate, so the parser and the type cannot drift.
fn opt_attended(map: &Map<String, Value>) -> Result<Attended, InvokeError> {
    let handback = opt_handback(map)?;
    let Some(patience) = opt_millis(map, Attended::WIRE_KEY)? else {
        // ⚠⚠⚠ A HANDBACK WITH NOBODY WATCHING IS MALFORMED, NOT A QUIET `NoOne`. The pair is one
        // request — *"a person is at this pane, and here is when a pane they take comes back"* —
        // and the caller who sends only the second half has plainly asked for a run that waits.
        // Answering `NoOne` would give them a run that ENDS on the first keystroke, which is the
        // opposite, and the type they are addressing cannot even express what they sent
        // ([`Handback`] lives inside [`Attended::APerson`]). So they are told.
        return if handback == Handback::Never {
            Ok(Attended::NoOne)
        } else {
            Err(InvokeError::TypeMismatch)
        };
    };
    Attended::of(patience, handback).ok_or(InvokeError::TypeMismatch)
}

/// Read the optional `handback_still_ms` — WHEN A PANE THIS RUN'S PERSON TAKES BECOMES THIS RUN'S
/// AGAIN. Absent (or `null`) is [`Handback::Never`]: the run ends when somebody takes the pane,
/// which is what every run did before the key existed and is the conservative half.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`opt_attended`]'s rule and [`Handback::of`]'s predicate: *"the pane is
/// mine again the instant they pause"* is not something a caller can mean, since every person pauses
/// between keystrokes, and one who reached zero by arithmetic would get a run that typed into the
/// gap between their words.
/// Read the optional `hold_within_ms` — HOW LONG SOMEBODY MAY HOLD THIS RUN before it ends as
/// abandoned. Absent (or `null`) is [`None`]: the loop document's own ceiling stands, which is what
/// *"omitting a duration key means the document decides"* means everywhere else on this form.
///
/// ⚠⚠⚠ **IT IS NOT PART OF THE `await_person_ms` / `handback_still_ms` PAIR, AND THAT IS REGISTER
/// ITEM 534's WHOLE POINT.** Those two are one request about a person who is EXPECTED, and
/// [`Handback`] living inside [`Attended::APerson`] is what enforces it. A hold is an order, and a
/// run nobody is watching can be given one — which is exactly the population that used to park for
/// ever, so a ceiling read through that contract would have been unreachable where it was needed.
/// It is therefore read alone, and sending it without either of the others is well-formed.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`opt_attended`]'s rule: *"hold this run and end it at once"* is
/// `cancel` spelled wrong, so the two would be two spellings of one behaviour — and the caller who
/// reached zero by arithmetic is the one who has to be told. There is deliberately no spelling for
/// *"no ceiling"*: an unbounded hold is the defect this key closes, not a configuration.
fn opt_hold_within(map: &Map<String, Value>) -> Result<Option<Duration>, InvokeError> {
    let Some(within) = opt_millis(map, sprag_plugin::HOLD_WITHIN_KEY)? else {
        return Ok(None);
    };
    if within.is_zero() {
        return Err(InvokeError::TypeMismatch);
    }
    Ok(Some(within))
}

fn opt_handback(map: &Map<String, Value>) -> Result<Handback, InvokeError> {
    let Some(still) = opt_millis(map, Handback::WIRE_KEY)? else {
        return Ok(Handback::Never);
    };
    Handback::of(still).ok_or(InvokeError::TypeMismatch)
}

/// Read the LOOPING forms' optional turn contract — WHAT MAKES THE PEER'S TURN OVER AND HOW LONG IT
/// MAY TAKE. Absent is `None`: the step ends on the plugin's own 500 ms constant, which is what
/// every run did before the pair existed.
///
/// # ⚠⚠⚠ The pair is ONE request, and half of it is malformed
///
/// [`opt_attended`]'s rule exactly, for the same reason one door over. `done_when` with no
/// `turn_within_ms` is a caller who said *"my peer finishes like this"* and left the run with no
/// idea how long to allow — and the type they are addressing cannot express it ([`Turn`] holds
/// both). `turn_within_ms` with no `done_when` is a bound on a contract that does not exist, which
/// would silently become *"wait this long, then type again anyway"* — a different behaviour from
/// the one they asked for, in the direction of doing more.
///
/// ⚠ Answering a quiet `None` to either half would give the caller the 500 ms timer they were
/// plainly trying to get away from, so they are told instead.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`Turn::lasting`]'s predicate: *"wait no time at all for my peer to
/// finish"* is not something a caller can mean.
fn opt_turn(map: &Map<String, Value>) -> Result<Option<Turn>, InvokeError> {
    let within = opt_millis(map, Turn::WIRE_KEY)?;
    let Some(when) = opt_done_when(map)? else {
        // A bound with nothing to bound.
        return if within.is_some() {
            Err(InvokeError::TypeMismatch)
        } else {
            Ok(None)
        };
    };
    Turn::lasting(when, within)
        .map(Some)
        .ok_or(InvokeError::TypeMismatch)
}

/// Read the `ai_loop` form's optional `turn_within_ms` — HOW LONG ONE OF THE INNER AGENT'S TURNS
/// MAY TAKE, as a number for the document to hold. Absent is [`None`]: **the document decides**.
///
/// # ⚠⚠⚠ Why this exists beside [`opt_turn`] instead of calling it
///
/// A loop no longer builds a [`Turn`] at all — the bound is `ai_loop.scxml`'s since register item
/// 300, and only `done_when` is left on its spec — so the pairing [`opt_turn`] enforces cannot
/// apply here. It never did: an `agent` run's default contract is `exits`, so a bare bound would
/// bound something the caller did not choose, where a loop's default is [`INNER_SESSION_ENDS`] and
/// a bare bound bounds exactly the turn the caller is thinking about.
///
/// # ⚠⚠⚠ What it keeps, and what would have gone silently wrong without it
///
/// **ZERO IS STILL MALFORMED, and [`Turn::lasting`] is still who says so.** This form used to build
/// a `Turn` and hand the refusal straight back; with the bound moved to the document, a zero would
/// have flowed into `<data>` and been read there as *the author declines a bound* — turning a
/// request the wire REFUSED into a run, which is the direction R385 registered as earning a
/// protocol bump. The type is asked rather than the rule re-typed, so there is still one owner of
/// *"wait no time at all for my peer to finish is not a thing a caller can mean"*.
///
/// [`INNER_SESSION_ENDS`]: sprag_plugin::INNER_SESSION_ENDS
fn opt_ai_loop_turn_ms(map: &Map<String, Value>) -> Result<Option<i64>, InvokeError> {
    let Some(within) = opt_millis(map, Turn::WIRE_KEY)? else {
        return Ok(None);
    };
    Turn::lasting(sprag_plugin::INNER_SESSION_ENDS, Some(within))
        .ok_or(InvokeError::TypeMismatch)?;
    Ok(Some(within.as_millis() as i64))
}

/// Parse the `agent` form's optional `done_when` — WHAT MAKES THE TURN OVER. Absent (or `null`)
/// leaves the spec's default, which is [`DoneWhen::Exits`] and is what this adapter did
/// unconditionally before the argument existed.
///
/// ⚠ A BARE WORD, read through the type's own [`DoneWhen::parse`], so the set this accepts and the
/// set the wire publishes are one list. The first draft took an object with a companion `agent`
/// and two conformance gates refused it — see [`PluginGrammar::DONE_WHEN`](crate::wire::PluginGrammar::DONE_WHEN).
fn opt_done_when(map: &Map<String, Value>) -> Result<Option<DoneWhen>, InvokeError> {
    let Some(word) = opt_str(map, "done_when")? else {
        return Ok(None);
    };
    DoneWhen::parse(word)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// A required COUNT — `max_turns` and its kind.
///
/// ⚠ `i64` because that is what a script datamodel holds and what
/// [`Brief`] carries; reading it as a `u32` here and widening would put a
/// second opinion about the range between the caller and the document that enforces it. A negative
/// or absurd number is refused by the loop's own door, which is where the reason lives.
fn require_count(map: &Map<String, Value>, key: &str) -> Result<i64, InvokeError> {
    map.get(key)
        .and_then(Value::as_i64)
        .ok_or(InvokeError::TypeMismatch)
}

/// The same count, optional — absent (or `null`) is [`None`].
fn opt_count(map: &Map<String, Value>, key: &str) -> Result<Option<i64>, InvokeError> {
    if declined(map, key) {
        return Ok(None);
    }
    require_count(map, key).map(Some)
}

/// **WHAT A LOOP RUN IS FOR, RESOLVED FROM THE CALLER'S REQUEST AND THIS REPOSITORY'S KIND** —
/// every judgement a `Brief` carries, in the one place both roads to it meet.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not the inline block it was until register item 492
///
/// Eight of a brief's fields fall back to the kind document, and **not one of those fall-throughs
/// was held by anything.** The residue was registered rather than hidden — `sprag_plugin`'s
/// `a_declined_budget_crosses_as_a_word_and_the_run_is_not_refused` says it in its own doc:
/// *"deleting `.or_else(|| kind.turn_budget())` from `plugins.rs` leaves the entire workspace
/// GREEN. What would catch it is an observable of the RESOLVED budget on a run started through the
/// wire, and `turn_budget` is crate-private"* — and it was measured again on item 492's round, for
/// the ceiling, with the same answer.
///
/// A `Brief` is that observable. It is `pub` in `sprag_plugin`, it is exactly what the door
/// resolves, and handing it back instead of consuming it in place is the whole difference between a
/// wiring nothing checks and one a gate can read. ⚠⚠ **It is not a gate re-implementing the line it
/// checks**: this IS the line, and the test asks the real function what a real request plus the
/// real kind document resolve to.
///
/// ⚠ The engine and the pane stay with the caller: this resolves JUDGEMENTS, and which pane a run
/// drives is a binding.
///
/// # Errors
///
/// [`InvokeError::TypeMismatch`] for a malformed argument, and [`refused`]'s sentence when this
/// repository's own kind document holds a list this driver cannot read.
fn ai_loop_brief(
    map: &Map<String, Value>,
    kind: &sprag_plugin::kind::LoopKind,
) -> Result<Brief, InvokeError> {
    // ⚠⚠⚠⚠ DECLINABLE SINCE ITEM 312 — see `AI_LOOP_FORM`. A caller who names no budget is
    // deferring to `ai_loop.scxml`'s own, which is resolved where the document can be read
    // (`OuterLoop::brief`) and not here, because this door has no datamodel.
    let max_turns = opt_count(map, "max_turns")?;
    let kind_consents = kind.consents().map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a consent list this driver \
                         cannot read ({why:?}); a run cannot start on decisions nobody can check"
        ))
    })?;
    let kind_rules = kind.screen_rules().map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a rule list this driver cannot \
                         read ({why:?}); a run cannot start on decisions nobody can check"
        ))
    })?;
    Ok(Brief {
        // ⚠⚠ NO WIRE KEY, DELIBERATELY. What a repository asks its own runs at the end
        // is its document's business; a caller that could override it could delete the
        // sweep this repository's record says it pays for twice over when it is missing.
        closing_rules: kind.closing_rules(),
        // ⚠⚠⚠ NO WIRE KEY EITHER, and for the same reason one line up — register item
        // 428. What certifies this repository's work is its document's business; a
        // caller who could name the checker could delete it by naming nothing, which is
        // the self-certification the whole item is about.
        milestone_check: kind.milestone_check(),
        // ⚠⚠⚠ NO WIRE KEY EITHER, on the two lines above's terms. What this
        // repository's peer prints when its SERVICE fails is its document's business,
        // and a caller who could name the needle could delete the wait by naming
        // nothing — turning a ten-minute outage back into the dead run that paid for
        // this. See `ServiceOutage`, whose doc carries the measurement.
        service: kind.service_outage(),
        north_star: require_str(map, "north_star")?.to_string(),
        milestone: require_str(map, "milestone")?.to_string(),
        reference: require_str(map, "reference")?.to_string(),
        // ⚠⚠⚠ ABSENT MEANS "WHAT THIS REPOSITORY'S KIND DOCUMENT SAYS", and only then
        // the template's own number. A debt run ends on its work rather than on a turn
        // count, and that decision is the kind's to make — it reaches here as
        // `Counted::Never` rather than as a number nobody could write.
        max_turns: max_turns
            .map(sprag_plugin::Counted::Of)
            .or_else(|| kind.turn_budget()),
        // ⚠⚠ ABSENT STILL MEANS "NEVER, ON THE BUDGET", spelled as the one number that
        // makes the budget guard unreachable rather than as a magic zero: `judging`
        // tests `turns >= max_turns` BEFORE `turns_since_reflect >= reflect_every`, so
        // an equal pair exhausts first.
        //
        // ⚠⚠⚠⚠ BUT THE `unwrap_or(max_turns)` THAT SAID SO IS NO LONGER HERE — item 312.
        // The default IS the budget, and the budget may now be the document's, which
        // this door cannot read. So both resolve together in `OuterLoop::brief`, where
        // the datamodel is; carrying `None` through is what lets them.
        //
        // ⚠⚠⚠ IT IS NO LONGER A REFUSAL TO NAME A SMALLER ONE — `reflecting` and the
        // session-replace lifecycle behind it are built. The default is kept as it was
        // ON PURPOSE rather than moved to the document's `8`: a restart closes a pane a
        // person may be reading and opens another, and a caller who has said nothing
        // about reflection has not asked for that. What they DO get without asking is a
        // reflection when a standing instruction fires, which is the correctness edge
        // (item 148) and not a budget — `screened > screened_carried` is not spelled
        // here because no caller sets it.
        // ⚠⚠⚠ AND THE KIND ANSWERS THIS TOO, which it MUST when it declines the budget:
        // the template's default for reflection is *the number that makes the reflect
        // guard unreachable*, and that number only exists while there is a budget to
        // borrow it from. `OuterLoop::brief` refuses the pair rather than guessing.
        reflect_every: opt_count(map, "reflect_every")?.or_else(|| kind.reflect_every()),
        // ⚠⚠⚠⚠⚠ REGISTER ITEM 492, and the same three-step fall-through its two
        // neighbours have: the caller's number, then THIS repository's kind document,
        // then the template's own — resolved in `OuterLoop::brief`, which is the only
        // place that can read the last of those.
        //
        // ⚠⚠ Until this line the ceiling had NO road at all. The template's comment
        // said it was the kind's to author while no kind could; item 477 measured what
        // that cost at the far end, where `reviewing` took the fall-back eight times
        // out of eight because the number was 0 on every run ever driven.
        context_ceiling: opt_count(map, "context_ceiling")?.or_else(|| kind.context_ceiling()),
        // ⚠⚠⚠⚠⚠ REGISTER ITEM 494 — the line above's TWIN, and the reason it is a
        // separate item rather than a detail of 492: the template says the number is
        // the kind's to author about exactly TWO of its `<data>`, 492 measured the
        // instance, and the identical defect was still standing one of them up. **A
        // premise that produces one defect produces the rest of its class**, and the
        // ratchet in `sprag-gate`'s `authored` module is what closes the class.
        //
        // ⚠⚠ Same three-step fall-through as its three neighbours: the caller's
        // number, then THIS repository's kind document, then the template's own —
        // resolved in `OuterLoop::brief`, the only place that can read the last.
        reflect_after_refusals: opt_count(map, "reflect_after_refusals")?
            .or_else(|| kind.reflect_after_refusals()),
        // ⚠⚠ ABSENT MEANS "WHAT THE DOCUMENT'S AUTHOR WROTE", not *"screen nothing"*.
        // The rules live in the loop template, so a caller who says nothing about
        // screening is not overriding it — and the driver echoes the document's own
        // rules back through the brief rather than deleting them.
        // ⚠⚠ ABSENT MEANS "WHAT THIS REPOSITORY'S KIND DOCUMENT SAYS", and it used to
        // mean *"what the template's author wrote"*. The template no longer writes any
        // — a standing instruction there is answered on behalf of every repository that
        // copies it — so the fallback moved with the values. A caller who says nothing
        // about screening is still not overriding anything.
        screen_rules: opt_screen_rules(map)?.or(kind_rules),
        may_answer: opt_may_answer(map)?.or(kind_consents),
        // ⚠⚠⚠ THE SAME TWO KEYS, NOW WRITTEN INTO THE DOCUMENT instead of into the
        // spec. `awaiting_human`'s only run-ending exit is *nobody came within the
        // patience*, so the patience is the loop DOCUMENT's own data — the argument
        // `Brief::screen_rules` already makes, applied to the other half of one state.
        //
        // ⚠⚠ THE PAIR IS STILL VALIDATED AS A PAIR. `opt_attended` owns *a call that
        // sends the stillness alone is malformed*, and reading the two keys separately
        // here would have quietly dropped that refusal; the values are taken back OUT
        // of what it built rather than parsed a second time.
        //
        // ⚠⚠⚠ AND OMITTING THEM NOW MEANS *THE DOCUMENT DECIDES*, where it used to mean
        // `Attended::NoOne` — a run that ended at the first dialog it could not answer.
        // That is the change, stated: a caller who wants that says so by authoring it,
        // and the shipped document's own number is what an unspecified run now gets.
        await_person_ms: opt_attended(map)?
            .patience()
            .map(|patience| patience.as_millis() as i64),
        handback_still_ms: opt_attended(map)?
            .handback()
            .stillness()
            .map(|still| still.as_millis() as i64),
        // ⚠⚠⚠⚠ AND HOW LONG A HOLD MAY LAST — register item 534, and it is read on its OWN rather
        // than through `opt_attended` above, which is the whole shape of the item. Those two keys
        // are one request about somebody EXPECTED, and this is a bound on an order a run nobody is
        // watching can also be given: item 534's entire population is the unattended runs, the ones
        // that parked for ever, so routing it through the *is anybody watching* contract would have
        // put the ceiling exactly where it could not reach.
        //
        // ⚠⚠⚠ ZERO IS MALFORMED, on `await_person_ms`'s own rule and refused by the same reader:
        // *hold this run and end it at once* is `cancel` spelled wrong, and a caller who reached
        // zero by arithmetic gets told rather than obeyed. ⚠ Absent means THE DOCUMENT DECIDES,
        // like the two keys above and unlike their pre-item-300 selves.
        hold_within_ms: opt_hold_within(map)?.map(|held| held.as_millis() as i64),
        // ⚠⚠⚠ AND THE LAST TWO JUDGEMENTS, ON THE SAME ROUTE. Each of them arrived
        // paired with a PREDICATE — `ready_timeout_ms` with `ready_when`,
        // `turn_within_ms` with `done_when` — and register item 300 measured that the
        // pair is one fact plus one decision: what makes a pane ready and how a program
        // signals a turn is over are read off WHICH PROGRAM is in the pane; three
        // minutes and half an hour are read off nobody. **A wire pairing is not evidence
        // of a shared owner.** The predicates stay on the spec below; these two write
        // `<data>`.
        //
        // ⚠⚠ THE WIRE FORM IS UNCHANGED — both keys are still accepted, still optional,
        // still milliseconds. What changed is where the number lands, and what OMITTING
        // one means: it used to be the substrate's default, and it is now *the document
        // decides*, which is `await_person_ms`'s change one round earlier.
        ready_timeout_ms: opt_millis(map, Readiness::WIRE_KEY)?
            .map(|within| within.as_millis() as i64),
        turn_within_ms: opt_ai_loop_turn_ms(map)?,
    })
}

/// **WHY A LOOP DID NOT START, IN A SENTENCE THE CALLER CAN ACT ON.**
///
/// ⚠⚠ Every arm names the KNOB or the FILE, because each of these is refused before anything
/// happens and the whole value of refusing early is that the caller can fix it and call again. A
/// refusal that said only *"the loop could not be started"* would cost them the run they were
/// spared.
fn ai_loop_refusal(why: &sprag_plugin::NotStarted) -> String {
    match why {
        sprag_plugin::NotStarted::Undrivable => {
            "this build's `ai_loop.scxml` does not carry the strings a loop is driven by, so no \
             run could be started against it — the document, or the statechart engine pinned under \
             it, is not the one this driver was written for"
                .to_owned()
        }
        sprag_plugin::NotStarted::Unbuilt(sprag_plugin::AiLoopState::Exhausted) => {
            "`max_turns` must be at least 1: a loop allowed no turns judges itself exhausted \
             before its agent has answered anything"
                .to_owned()
        }
        sprag_plugin::NotStarted::Unbuilt(state) => {
            format!("a loop briefed this way reaches {state:?}, which this build does not drive")
        }
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::NotHeld { part, held }) => {
            format!(
                "the loop's datamodel did not hold {part} as it was sent{}, so nothing was \
                 started rather than an agent being prompted with something nobody wrote",
                match held {
                    Some(held) => format!(" (it holds {held:?})"),
                    None => " (it holds nothing a reader can name)".to_owned(),
                },
            )
        }
        // Neither is reachable from here — the machine is built one line above the brief, so it is
        // in `idle`, and `Took` is the success this function is not called for. Said rather than
        // collapsed into a wildcard: a sentence nobody can produce is cheaper than a match that
        // stops being exhaustive when the type grows.
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::TooLate(state)) => {
            format!("the loop was already in {state:?} when it was briefed")
        }
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::Took) => {
            "the loop took its brief and did not start anyway".to_owned()
        }
        sprag_plugin::NotStarted::Screening(sprag_plugin::NotScreenable::Malformed { at, why }) => {
            format!(
                "screen rule {at} (counting from zero) is not one this build can carry out: {}",
                why.describe(),
            )
        }
        // ⚠⚠⚠⚠⚠ THE DOCUMENT'S OWN CONTENT DID NOT EXECUTE — register item 505. Every other arm
        // here names something the CALLER sent; this one names the FILE, and the difference matters
        // to whoever reads it: nothing the request said can fix a clause that will not evaluate. The
        // class is carried verbatim because it says who repairs it — `error.execution` is the
        // document's own content and `error.communication` is a `<send>` this host did not serve.
        sprag_plugin::NotStarted::Faulted(error) => {
            format!(
                "this build's `ai_loop.scxml` raised {error} while its datamodel was being \
                 initialised, so the document stopped itself before a run began — a clause in it \
                 could not be evaluated, and no argument of this request can change that. Nothing \
                 was prompted"
            )
        }
        sprag_plugin::NotStarted::Screening(sprag_plugin::NotScreenable::Unreadable) => {
            format!(
                "this loop's `{}` is not a list of {{{}: …, {}: …}} objects, so nothing could be \
                 read as a standing instruction — the document, or what was sent for it, is not \
                 the shape `screening` carries out",
                ScreenRules::WIRE_KEY,
                ScreenRule::WHEN_KEY,
                ScreenRule::TEXT_KEY,
            )
        }
    }
}

fn opt_millis(map: &Map<String, Value>, key: &str) -> Result<Option<Duration>, InvokeError> {
    if declined(map, key) {
        return Ok(None);
    }
    Ok(Some(Duration::from_millis(
        map[key].as_u64().ok_or(InvokeError::TypeMismatch)?,
    )))
}

fn require_string_array(map: &Map<String, Value>, key: &str) -> Result<Vec<String>, InvokeError> {
    match map.get(key) {
        Some(Value::Array(items)) => {
            let argv = items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
                .ok_or(InvokeError::TypeMismatch)?;
            if argv.is_empty() {
                Err(refused(format!(
                    "{key:?} is empty: an endpoint needs at least its program"
                )))
            } else {
                Ok(argv)
            }
        }
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse an optional reply-format key (`"text"` | `"claude_json"`) into a
/// [`ReplyFormat`]. Absent → `None` (the spec keeps its default); an unknown
/// string → [`InvokeError::Rejected`].
fn parse_reply_format(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<ReplyFormat>, InvokeError> {
    // THROUGH THE TYPE, whose `WIRE_WORDS` this verb publishes. ⚠ A word outside the vocabulary is
    // `TypeMismatch` — a malformed request — where this answered `Rejected` with a sentence naming the
    // two words. That sentence was the only place the vocabulary was written down; it is in the
    // published grammar now, and the class matters twice over: it is what every other closed
    // vocabulary on this wire answers, and a `Rejected` is invisible to the completeness gate, which
    // can only see an argument the daemon refuses AS MALFORMED.
    match opt_str(map, key)? {
        None => Ok(None),
        Some(word) => ReplyFormat::from_wire(word)
            .map(Some)
            .ok_or(InvokeError::TypeMismatch),
    }
}

/// Read the optional `guardrails` sub-object — the THREE ceilings a run is bounded
/// by, each defaulted so an omitted one is still a bound.
///
/// `max_iterations` defaults to [`DEFAULT_MAX_ITERATIONS`] and `max_seconds` to
/// [`DEFAULT_MAX_SECONDS`] (both always present — the liveness floor). The cost
/// bound is self-describing: `max_bytes` xor `max_tokens` in the plugin's unit
/// (omitted → the plugin's default ceiling). NB a `Tokens(0)`-only run (a
/// print-mode Text dialogue) accumulates no measured cost, so its cost ceiling
/// never binds and the other two are its effective bounds — by design.
///
/// ⚠⚠ **A KEY THIS OBJECT DOES NOT DECLARE IS A MALFORMED REQUEST**, which is not
/// how the rest of this wire treats an unknown key. The asymmetry is the whole
/// point and it is stated on
/// [`guardrail_fields`](crate::wire::PluginGrammar::guardrail_fields): ignoring
/// an ordinary argument makes a verb do LESS than asked and the caller can see
/// that in the result; ignoring a BOUND makes the run do more, without limit, and
/// answers success.
fn parse_guardrails(
    map: &Map<String, Value>,
    default_cost: Cost,
) -> Result<Guardrails, InvokeError> {
    // ⚠ DECLINED, not merely absent — see [`declined`](crate::external::declined). A client whose
    // language serialises an absent optional as `null` sends `"guardrails": null` on every
    // unguarded run, and answering `TypeMismatch` there refuses a well-formed call.
    if declined(map, "guardrails") {
        return Ok(Guardrails {
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_cost: Some(default_cost),
            max_duration: Some(Duration::from_secs(DEFAULT_MAX_SECONDS)),
        });
    }
    let Value::Object(g) = &map["guardrails"] else {
        return Err(InvokeError::TypeMismatch);
    };
    // AGAINST THE PUBLICATION, not against a list kept here: the keys this parser honours and the
    // keys the grammar advertises are one set, so neither can grow without the other.
    let declared = crate::wire::PluginGrammar::guardrail_fields(default_cost.unit());
    if let Some(unknown) = g
        .keys()
        .find(|key| !declared.iter().any(|field| field.name == key.as_str()))
    {
        return Err(refused(format!(
            "{unknown:?} is not a guardrail of a run that spends {}. It takes: {}. A bound this \
             daemon does not know would have been ignored, and an ignored bound is not a bound.",
            default_cost.unit(),
            declared
                .iter()
                .map(|field| field.name)
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    // ⚠ The SAME declined rule inside the nest. A nested optional is an optional.
    let max_iterations = if declined(g, "max_iterations") {
        DEFAULT_MAX_ITERATIONS
    } else {
        g["max_iterations"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)?
    };
    let max_seconds = if declined(g, "max_seconds") {
        DEFAULT_MAX_SECONDS
    } else {
        g["max_seconds"].as_u64().ok_or(InvokeError::TypeMismatch)?
    };
    Ok(Guardrails {
        max_iterations,
        max_cost: parse_max_cost(g, default_cost)?,
        max_duration: Some(Duration::from_secs(max_seconds)),
    })
}

/// Parse the optional cost bound: `max_bytes` XOR `max_tokens` (a run has ONE
/// cost unit), or the plugin's default when neither is given. The chosen unit
/// must match the plugin's — so a guardrail cannot be misloaded into the wrong
/// currency. Both keys present, a non-integer, or the wrong unit → a synchronous
/// [`InvokeError`] (a misloaded spend guardrail is a submit-time error, never a
/// silently looser-by-a-factor bound).
fn parse_max_cost(g: &Map<String, Value>, default_cost: Cost) -> Result<Option<Cost>, InvokeError> {
    // ⚠⚠ A DECLINED KEY IS NOT A GIVEN ONE, and here that is load-bearing rather than tidy: the
    // XOR below refuses BOTH-given, so a client declining one unit with `null` would have been told
    // it had named two cost units when it had named one.
    let bound = match (
        (!declined(g, "max_bytes")).then(|| &g["max_bytes"]),
        (!declined(g, "max_tokens")).then(|| &g["max_tokens"]),
    ) {
        (Some(_), Some(_)) => {
            return Err(refused(
                "max_bytes and max_tokens were both given: a run has one cost unit",
            ));
        }
        (Some(v), None) => Cost::Bytes(v.as_u64().ok_or(InvokeError::TypeMismatch)?),
        (None, Some(v)) => Cost::Tokens(v.as_u64().ok_or(InvokeError::TypeMismatch)?),
        (None, None) => return Ok(Some(default_cost)),
    };
    if bound.unit() != default_cost.unit() {
        return Err(refused(format!(
            "this plugin spends {}, so a {} bound cannot guard it",
            default_cost.unit(),
            bound.unit()
        )));
    }
    Ok(Some(bound))
}

/// Render one run as JSON for `query("runs")`.
///
/// `seat` is the pane to publish as `opened_by`, already resolved by the caller: `run.opened_by`
/// for a run this daemon issued, and the pane currently holding `run.opened_by_session` for one it
/// inherited from a predecessor. Taken as a parameter rather than read off `run` because the second
/// answer needs the workspace, which this function has no business holding.
///
/// # ⚠⚠⚠ Why re-deriving `opened_by` earns no [`sprag_rpc::WIRE_PROTOCOL`] bump
///
/// Written down because NOT bumping is a judgement too, and this one is close enough to the line to
/// deserve its reasoning rather than its conclusion.
///
/// Nothing about the key moved. `opened_by` still means exactly what it meant — *the pane whose
/// occupant asked for this run* — it still carries a pane id, and it is still OMITTED rather than
/// sent as `null` when nobody claims the run. What changed is that a restored run can now be
/// answered at all, where before the daemon had thrown away the only thing that could have answered
/// it. That is the daemon answering a question it already published MORE OFTEN, which is the
/// widened-value-space case the bump rule explicitly declines.
///
/// ⚠⚠ **The rule it comes closest to is *"reading the absence of an answer key as a guarantee"***,
/// and it is worth saying why that one does not fire: no reader treats a missing `opened_by` as
/// *"this run is nobody's, for ever"*. The agent-facing filter (`sprag-mcp`'s `own_runs`) compares
/// it to its own pane and a miss simply means *not mine* — which is the same sentence before and
/// after. A reader that had encoded *absent ⇒ unclaimable* would be the one to break, and none does.
///
/// ⚠ Ask [`sprag_rpc::WIRE_PROTOCOL`]'s own doc rather than this paragraph if the question comes up
/// again; this records the judgement taken for THIS change, not the rule.
fn run_to_json(run: &RunSummary, seat: Option<u64>) -> Value {
    // ⚠ THE SAME THREE KEYS THE OUTCOME USES (`iterations`, `cost`, `unit`), so a reader that polls
    // a running run and then reads its outcome meets ONE vocabulary rather than two. A run that has
    // not finished a step yet answers zero with a null unit, which is the same shape a run that was
    // cancelled before any step reports — both mean "nothing measured yet".
    let (cost, unit) = run
        .progress
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    let state_json = match &run.state {
        RunState::Running => json!({
            "status": RunStatus::Running.wire_str(),
            "iterations": run.progress.iterations,
            "cost": cost,
            "unit": unit,
            // ⚠⚠ AND THE ANSWER TALLY MID-FLIGHT, under the outcome's own name. The comment above
            // says these keys exist so a reader who polls a run and then reads its outcome meets
            // ONE vocabulary — and this is the key where the polling matters MOST: the other two
            // are watched to tell progress from stuck, this one is watched to see a decision being
            // taken on your behalf while there is still time to cancel.
            RUN_ANSWERED_KEY: run.progress.answered,
        }),
        RunState::Done { outcome, output } => json!({
            "status": RunStatus::Done.wire_str(),
            "outcome": outcome_to_json(outcome),
            "output": output,
        }),
        RunState::Panicked(message) => {
            json!({ "status": RunStatus::Panicked.wire_str(), "error": message })
        }
        // ⚠ A FOURTH STATUS WORD, which is why `WIRE_PROTOCOL` moved: `status` is a value space a
        // peer decodes whole, so an added word is a break no address or shape pin can see (R342).
        // The counters it reached are still here — what it managed before its daemon died is the
        // only thing a reader can still learn about it.
        RunState::Interrupted => json!({
            "status": RunStatus::Interrupted.wire_str(),
            "iterations": run.progress.iterations,
            "cost": cost,
            "unit": unit,
        }),
    };
    // `opened_by` is OMITTED for a run nobody claims rather than sent as `null`, the rule
    // `ArgGrammar::to_answer` follows for an absent vocabulary: a reader tells silence from a claim
    // by the key's absence, and "a person started this" is a silence.
    let mut entry = json!({
        RUN_ID_KEY: run.id.0,
        "label": run.label,
        "state": state_json,
        // ⚠ THE JOURNAL SITS BESIDE THE STATE, NOT INSIDE IT, because it is the one fact that
        // means the same thing whether the run is still going or over: these are the steps it
        // took. Nesting it under `running` would have made a finished run's account vanish at
        // exactly the moment somebody wants to read it.
        RUN_JOURNAL_KEY: run.progress.journal.iter().map(step_to_json).collect::<Vec<_>>(),
    });
    // ⚠⚠ THE SEAT IS THE CALLER'S TO RESOLVE, NOT THIS FUNCTION'S, and that is the shape of the
    // fix rather than an accident of plumbing: for a run this daemon issued it is `run.opened_by`,
    // and for one it INHERITED it is whoever is currently holding the conversation that asked
    // (`PluginsExternal::seat_of`). Only the caller can see the workspace, so only the caller can
    // answer the second — see `crate::runs::RunRegistry::restore`'s rule 1.
    if let Some(opener) = seat {
        entry[RUN_OPENED_BY_KEY] = json!(opener);
    }
    // ⚠⚠⚠ AND THE BUILD FOLLOWS THE SAME OMIT-RATHER-THAN-NULL RULE, for a reason of its own:
    // absent means NOTHING RECORDED WHICH BUILD THIS WAS — a run restored from a log written before
    // the field existed — and a reader that filled that in with the daemon it is talking to would
    // date a dead daemon's work to its successor. See `crate::runs::RunSummary::build`.
    if let Some(build) = &run.build {
        entry[RUN_BUILD_KEY] = json!(build);
    }
    entry
}

/// Render one journal entry as JSON.
///
/// The step's OWN cost with its own unit, so a reader can find the expensive step rather than only
/// the total — and the plugin's `note` verbatim, which the host does not interpret (the Driver does
/// not either; see [`sprag_plugin::Step::note`]). A step with nothing to say OMITS the key, the
/// rule `run_to_json` follows for `opened_by`: absence is silence, not an empty claim.
fn step_to_json(step: &sprag_plugin::StepRecord) -> Value {
    let mut entry = json!({
        "iteration": step.iteration,
        "cost": step.cost.amount(),
        "unit": step.cost.unit(),
        "verdict": step.verdict.wire_str(),
    });
    if let Some(note) = &step.note {
        entry["note"] = json!(note);
    }
    entry
}

/// Render a plugin [`Outcome`] as JSON (serialization is a host concern, so the
/// pinion-free substrate stays serde-free).
/// An outcome's terminal word — the ONE mapping, read by the wire renderer AND by the durable run
/// log, so a run reloaded from disk cannot come back under a different word than it went out under.
#[must_use]
pub fn outcome_word(outcome: &Outcome) -> &'static str {
    // ⚠⚠ THROUGH THE TYPE, which is where every other variant→name mapping on this wire lives
    // (`Cost::unit`, `Ceiling::wire_str`, `Verdict::wire_str`) and where this one did NOT until
    // R366. Spelled here, the host could name an outcome the type had renamed, and there was no
    // list for the answers pin to walk — so the pin hand-wrote five variants and said so.
    outcome.state.wire_str()
}

/// Which ceiling stopped it, or [`None`] when no ceiling did — [`outcome_word`]'s companion.
#[must_use]
pub fn outcome_ceiling(outcome: &Outcome) -> Option<&'static str> {
    match &outcome.state {
        OutcomeState::Exhausted(ceiling) => Some(ceiling.wire_str()),
        _ => None,
    }
}

/// WHAT THE PEER IS ASKING, for a run that ended [`OutcomeState::Blocked`] — the question's own
/// text and its options, or [`None`] when there is no question to publish.
///
/// # ⚠⚠ Why the OPTIONS and not just the sentence
///
/// A caller reading this has to answer it, and the answer is a NUMBER. Publishing only the prose
/// would leave them to parse the choices back off a screen this host has already parsed — and to
/// guess which one a bare Enter would take, which is the difference between confirming a tool call
/// and declining it. `selected` is that fact, carried rather than inferred.
///
/// ⚠ `None` for a blocked run is a real answer and not a gap: an agent can block on something that
/// is not a numbered list. Its remedy is the one
/// [`AgentObservation::asking`](sprag_plugin::AgentObservation::asking) states — hand the pane to a
/// person — and a caller can tell the two apart because the key is ABSENT rather than empty.
#[must_use]
pub fn outcome_question(outcome: &Outcome) -> Option<Value> {
    let OutcomeState::Blocked(Some(unanswered)) = &outcome.state else {
        return None;
    };
    // ⚠ WHY is unconditional and the question is not. A run that was given a consent and stopped
    // anyway is indistinguishable from one that was given none without it, and those are two
    // different things for the caller to fix — see [`RUN_WHY_KEY`].
    let mut asking = json!({ RUN_WHY_KEY: unanswered.why().wire_str() });
    if let Some(question) = unanswered.question() {
        // The SHARED renderer, so this surface and the pane list cannot come to spell one question
        // two ways — see [`crate::wire::ASKING_KEY`]. `why` is merged over it rather than passed in
        // because it is the one member a RUN owes and a pane does not.
        let Value::Object(rendered) = crate::agent::question_json(question) else {
            unreachable!("the shared renderer answers an object");
        };
        for (key, value) in rendered {
            asking[key] = value;
        }
    }
    Some(asking)
}

/// THE SENTENCE behind an `asking.why` word — what a person or an agent is told to DO about a run
/// that stopped on its peer's question, or the word itself when this build does not know it.
///
/// # ⚠⚠ Why the mouths read this and not the type
///
/// [`sprag_plugin::Refusal`] owns the sentence, and both mouths must say the SAME one — which is
/// the whole reason it lives on the type rather than in a renderer. But the agent-facing mouth
/// depends on this crate and not on the plugin crate, so reaching the type would mean a second
/// binary carrying the whole plugin layer to read six strings. The host already owns every other
/// wire↔type projection a mouth needs ([`outcome_word`], [`outcome_from_words`]); this is one more,
/// and it delegates rather than spelling a variant.
///
/// ⚠ An UNKNOWN word answers itself rather than nothing. A newer daemon may name a reason an older
/// mouth predates, and printing the raw word is honest where silence would be a run that stopped
/// for no stated cause — the rule [`RUN_CEILING_KEY`] follows for the same reason.
#[must_use]
pub fn refusal_sentence(word: &str) -> String {
    sprag_plugin::Refusal::parse(word)
        .map_or_else(|| word.to_owned(), |why| why.describe().to_owned())
}

/// [`outcome_word`] / [`outcome_ceiling`] READ BACK — how a restored run recovers the state it was
/// written out under.
///
/// ⚠ An unreadable pair answers [`OutcomeState::Failed`] rather than guessing a happier one: a
/// record this build cannot parse is one it must not report as having converged.
#[must_use]
pub fn outcome_from_words(word: Option<&str>, ceiling: Option<&str>) -> OutcomeState {
    match word {
        Some("converged") => OutcomeState::Converged,
        Some("cancelled") => OutcomeState::Cancelled,
        // ⚠ The QUESTION is not restored. It was read off a pane that a restart has outlived, and
        // a question re-published from a durable record would be a claim about a screen nobody has
        // looked at since. The WORD survives, which is what tells a reader the run wants an answer.
        Some("blocked") => OutcomeState::Blocked(None),
        // ⚠⚠ READ THROUGH THE TYPE'S OWN LIST, not a match over the words this file knows. It
        // matched two by hand and answered `Iterations` for everything else, so the fourth ceiling
        // (`turns`, the loop's own budget) would have come back from a restart as *"you ran out of
        // steps"* — a false sentence pointing at a guardrail that run never met.
        Some("exhausted") => OutcomeState::Exhausted(
            ceiling
                .and_then(Ceiling::from_wire)
                // A record with no ceiling word at all predates the key or was truncated; name the
                // one bound EVERY run has, which is still true of anything that got here.
                .unwrap_or(Ceiling::Iterations),
        ),
        _ => OutcomeState::Failed,
    }
}

/// A run's OUTCOME as a client receives it — the projection both mouths render from.
///
/// ⚠ `pub` for the reason [`outcome_word`] beside it is: a mouth's gate has to drive the DAEMON's
/// renderer rather than a hand-written copy of its answer shape, or the gate passes while the two
/// drift. That is the two-readers defect this crate has paid for repeatedly, and a fixture spelling
/// `{"state": …, "asking": …}` itself would be a fresh instance of it.
#[must_use]
pub fn outcome_to_json(outcome: &Outcome) -> Value {
    let (state, ceiling) = (outcome_word(outcome), outcome_ceiling(outcome));
    // Cost is self-describing on the wire: the scalar amount plus its unit label
    // (both from `Cost` itself, so the host never names a variant), so a peer
    // reads it without knowing which plugin ran. A `null` unit means no measured
    // step (e.g. cancelled before any step ran).
    let (cost, unit) = outcome
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    let mut answer = json!({
        "state": state,
        "iterations": outcome.iterations,
        "cost": cost,
        "unit": unit,
        // ⚠ ALWAYS, including `0` — see `RUN_ANSWERED_KEY`. A decision taken on somebody's behalf
        // must be readable as a claim and not inferred from a key nobody wrote.
        RUN_ANSWERED_KEY: outcome.answered,
        // ⚠ THE SENTENCE, not the variant. This was `format!("{e:?}")` — `Write("Broken pipe (os
        // error 32)")` reaching an agent, which is R283's leak on the loop's own answer.
        "failure": outcome.failure.as_ref().map(ToString::to_string),
    });
    // WHICH CEILING, present only when there was one — so the key's presence is itself the claim,
    // the rule `run_to_json` follows for `opened_by`. `exhausted` with no ceiling beside it told a
    // caller to change something without saying what, and the three ceilings have three different
    // remedies.
    if let Some(ceiling) = ceiling {
        answer[RUN_CEILING_KEY] = json!(ceiling);
    }
    // AND WHAT THE PEER IS ASKING, present only when there is a question to publish — the same
    // presence-is-the-claim rule. A `blocked` run with no `asking` beside it is one whose peer
    // stopped on something this host could not read, which is a different remedy: a person.
    if let Some(asking) = outcome_question(outcome) {
        answer[RUN_ASKING_KEY] = asking;
    }
    // AND WHAT BECAME OF THE WORK, present only for a run that was cut short — see
    // `RUN_STOPPED_KEY`. The SENTENCE and not the variant, for the reason `failure` above is one.
    if let Some(stopped) = &outcome.stopped {
        answer[RUN_STOPPED_KEY] = json!(stopped.to_string());
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_detect::{AgentState, Report, Ruleset, built_ins};
    use sprag_plugin::PaneAccess;
    use sprag_terminal::CommandBuilder;
    use std::time::Instant;

    /// ⚠⚠⚠ **AND THE TURN CONTRACT REACHES A RUN THROUGH THE DOOR PRODUCTION USES.**
    ///
    /// The plugin crate gates what the contract DOES; this asks whether a caller can get one, which
    /// R373 paid dearly for learning to ask separately — that round's whole feature was unreachable
    /// from every production path while its unit gates were green.
    ///
    /// ⚠⚠ Both runs are started through `RUN_ACTION` — the verb the MCP `orchestrate` tool and the
    /// outer AI loop call — against the same peer thinking for the same three seconds, differing in
    /// the two keys alone. The uncontracted one spends turns re-asking; the contracted one asks
    /// once. **The pair is the claim**: either number alone would be a fact about this machine.
    #[test]
    fn a_turn_contract_sent_over_the_wire_stops_a_run_re_asking_a_slow_peer() {
        /// One `orchestrator` run against a peer that thinks for three seconds, with `extra`
        /// merged into the request — and the turn count it ended on.
        fn turns_taken(extra: Value) -> u32 {
            let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("while read l; do echo THINKING; sleep 3; echo PEER-REPLIED; done");
            command.env("TERM", "xterm-256color");
            let pane = lock(&workspace)
                .spawn(command, "peer".to_string(), 80, 24)
                .expect("spawn the peer");
            let registry = Arc::new(Mutex::new(RunRegistry::default()));
            let mut external = PluginsExternal::new(
                Arc::clone(&workspace),
                Arc::clone(&registry),
                None,
                None,
                None,
                None,
            );
            let mut request = json!({
                "plugin": "orchestrator",
                "pane": pane.0,
                "stimulus": "ping",
                "sentinel": "PEER-REPLIED",
                "guardrails": { "max_iterations": 100, "max_seconds": 60 },
            });
            let object = request.as_object_mut().expect("an object");
            for (key, value) in extra.as_object().expect("an object") {
                object.insert(key.clone(), value.clone());
            }
            let started = external
                .invoke(RUN_ACTION, IntrospectValue::Json(request))
                .expect("a well-formed run");
            let IntrospectValue::Int(id) = started else {
                panic!("a run answers its id: {started:?}");
            };
            let entry = ended(
                &registry,
                u64::try_from(id).expect("a run id is not negative"),
                Duration::from_secs(40),
            );
            assert_eq!(
                entry["state"]["outcome"]["state"],
                json!("converged"),
                "the peer answers well inside the run's clock: {entry:?}",
            );
            u32::try_from(
                entry["state"]["outcome"]["iterations"]
                    .as_u64()
                    .expect("a turn count"),
            )
            .expect("a turn count fits")
        }

        let uncontracted = turns_taken(json!({}));
        assert!(
            uncontracted > 1,
            "⚠⚠⚠ THE CONTROL: a run that names no turn contract still ends its steps on the \
             plugin's 500 ms constant, so it re-asks a peer that thinks for three seconds. It took \
             {uncontracted}, and if that is ever 1 the comparison below is measuring nothing",
        );
        let contracted = turns_taken(json!({
            "done_when": "exits",
            sprag_plugin::Turn::WIRE_KEY: 12_000,
        }));
        assert_eq!(
            contracted, 1,
            "⚠⚠⚠ AND THE SAME REQUEST PLUS TWO KEYS ASKS ONCE. The uncontracted run took \
             {uncontracted} turns at the same peer; this one took {contracted}. Nothing else \
             differs, so what the pair measures is the contract arriving over the wire",
        );
    }

    /// A pane running a stand-in agent: announces itself, then echoes back every line it is given.
    ///
    /// ⚠ ECHO OFF, so what appears on the screen is what the PROGRAM printed rather than what the
    /// line discipline painted — the difference between measuring a delivery and measuring the
    /// kernel.
    /// ⚠⚠⚠⚠⚠ **THE SAME AGENT FINDS THE RUNS IT STARTED AFTER A RESTART, AND A STRANGER IN ITS
    /// SEAT DOES NOT** — [`crate::runs::RunRegistry::restore`]'s rule 1, driven end to end.
    ///
    /// This is the payoff of the round that re-took that rule, and neither of the `runs.rs` gates
    /// can see it: they prove the conversation SURVIVES, and this proves it is USED — that a
    /// successor turns a surviving conversation back into the seat the agent-facing filter reads.
    /// ⚠ It goes through `PluginsExternal::read(RUNS_SLOT)`, the product's own door, because a gate
    /// that called `seat_of` directly would be green whether or not anything in the product ever
    /// asked it.
    ///
    /// The staging is the whole argument, so it is spelled out:
    ///
    /// * the predecessor's run recorded a CONVERSATION and pane 0 as its seat;
    /// * the successor restores it — seat dropped, conversation kept;
    /// * a pane is then born into the successor **holding that same conversation**, which is what
    ///   `restore_command`'s `--resume <uuid>` does in the product;
    /// * the run must now name THAT pane, whatever id it happens to have.
    ///
    /// ⚠⚠⚠⚠ **THE SEAT IS DELIBERATELY A DIFFERENT NUMBER FROM THE ORIGINAL.** The successor's
    /// pane comes out of a fresh workspace counter, so if this passed by carrying the old id it
    /// would pass for the wrong reason — the very confusion (a seat mistaken for an identity) the
    /// rule exists to end. Asserting the NEW id is what makes it a re-derivation.
    ///
    /// ⚠⚠⚠ **AND THE CONTROL IS A STRANGER IN THE SAME SEAT**, without which "the conversation is
    /// matched" and "any occupant inherits" are the same green — and the second is the hole the old
    /// rule was conservatively guarding against. A pane holding a DIFFERENT conversation must leave
    /// the run unclaimed.
    #[test]
    fn a_restored_run_finds_the_seat_its_own_conversation_is_sitting_in() {
        const RESUMED: &str = "13cac637-d86c-4fa3-8411-785d552cee16";
        const A_STRANGER: &str = "00000000-0000-0000-0000-000000000000";

        // What a predecessor daemon left on disk: unfinished, and it remembers WHO asked.
        let log = crate::runs::RunLog {
            version: crate::runs::RUN_LOG_VERSION,
            runs: vec![crate::runs::PersistedRun {
                id: 0,
                label: "agent pane=0".to_owned(),
                iterations: 1,
                cost: None,
                unit: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                opened_by_session: Some(RESUMED.to_owned()),
            }],
        };

        // A successor: its own registry, its own workspace, its own pane counter.
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        lock(&registry).restore(&log);
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));

        // ── THE CONTROL FIRST, so a later pass cannot be the stranger's ──
        let stranger = resumed_pane(&workspace, A_STRANGER);
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );
        let listed = read_runs(&external);
        assert!(
            listed[0].get(RUN_OPENED_BY_KEY).is_none(),
            "⚠⚠⚠ A STRANGER IN THE SEAT MUST NOT INHERIT THE RUN. This is the hole the old rule \
             guarded by dropping provenance outright, and it stays shut: pane {} holds a different \
             conversation, so nothing here is its. Entry: {:?}",
            stranger.0,
            listed[0],
        );

        // ── AND NOW THE ASKER ITSELF, resumed into the successor ──
        let mine = resumed_pane(&workspace, RESUMED);
        assert_ne!(
            mine.0, 0,
            "the re-derived seat must be a DIFFERENT id from the one the run was started under, or \
             this gate could pass by carrying the old number",
        );
        let listed = read_runs(&external);
        assert_eq!(
            listed[0].get(RUN_OPENED_BY_KEY).and_then(Value::as_u64),
            Some(mine.0),
            "⚠⚠⚠⚠⚠ THE AGENT MUST FIND ITS OWN RUN. It came back `--resume`d into the same \
             conversation, so the successor can say which seat that conversation is in — which is \
             the whole of `RunRegistry::restore`'s rule 1. Entry: {:?}",
            listed[0],
        );
    }

    /// `query("runs")` as a client reads it — through the product's own door.
    fn read_runs(external: &PluginsExternal) -> Vec<Value> {
        let IntrospectValue::Json(Value::Array(entries)) =
            external.read(RUNS_SLOT).expect("the runs slot answers")
        else {
            panic!("the runs slot answers a JSON array");
        };
        entries
    }

    /// A pane holding `session` as its conversation — what `restore_command`'s `--resume <uuid>`
    /// produces, staged through the workspace's own identity source rather than by writing the
    /// field, so the pane is named the way the product names one.
    fn resumed_pane(workspace: &Arc<Mutex<Workspace>>, session: &str) -> PaneId {
        let named = session.to_owned();
        lock(workspace).set_pane_identity_source(Arc::new(move |_| Some(named.clone())));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("exec cat");
        command.env("TERM", "dumb");
        lock(workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the resumed agent")
    }

    fn echoing_agent_pane(workspace: &Arc<Mutex<Workspace>>) -> PaneId {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(
            "stty -echo; printf 'AGENT-READY\\n'; while read l; do printf '%s\\n' \"$l\"; done",
        );
        command.env("TERM", "dumb");
        lock(workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the stand-in agent")
    }

    /// The `run` request that starts a loop, with `extra` merged over it.
    fn ai_loop_request(pane: PaneId, extra: Value) -> Value {
        let mut request = json!({
            "plugin": "ai_loop",
            "pane": pane.0,
            "agent": "claude",
            "north_star": "SPRAG-NORTH-STAR-CROSSED-THE-WIRE",
            "milestone": "say the marker",
            "reference": "this gate",
            "max_turns": 3,
            // ⚠ The barrier is `shows` rather than the `settles` a real agent gets, and that is
            // this FIXTURE's honesty rather than the product's default: a `/bin/sh` stand-in is not
            // an agent any detector will name, so waiting for one to settle would be waiting for a
            // verdict nothing can produce. The `agent` key above still travels, which is the point
            // — it is what the barrier would be derived from.
            //
            // ⚠⚠ AND `shows` RATHER THAN `prints`, which the first run of this gate paid for: the
            // pane announces itself when it is SPAWNED and the run is asked for afterwards, so
            // `prints` — *more occurrences than when this run started watching* — can never be
            // satisfied by a marker that is already there. The refusal says so in its own sentence;
            // this fixture is the case that sentence was written for.
            "ready_when": { "match": "shows", "marker": "AGENT-READY" },
            // ⚠ FALSE for the peer's reason, not the product's: this stand-in paints only whole
            // lines, so a delivery cannot be confirmed on screen before the newline that submits it.
            "shows_prompt": false,
            "guardrails": { "max_iterations": 200, "max_seconds": 30 },
        });
        let object = request.as_object_mut().expect("an object");
        for (key, value) in extra.as_object().expect("an object") {
            object.insert(key.clone(), value.clone());
        }
        request
    }

    /// ⚠⚠⚠ **A PERSON CAN START AN AI LOOP, AND WHAT THEY BRIEFED IT WITH REACHES THE AGENT** —
    /// register item 65, which R380 called *"the single biggest thing between this loop and a
    /// user"*.
    ///
    /// Five rounds built `ai_loop.scxml`'s machine, gave its turns two endings, wrote its driver
    /// and measured all of it against a live `claude` — and **nothing in the daemon constructed one
    /// and no surface started one.** Every one of those measurements ran inside a test.
    ///
    /// This one goes through `RUN_ACTION`, the verb the MCP mouth and the CLI both call, and
    /// asserts the thing that could not be asserted before: **the caller's own north star is on the
    /// agent's screen.** That single string crossing is the whole chain — the request grammar
    /// parsed it, the daemon built a real script engine for it, the brief crossed into the
    /// document's datamodel as an event, `priming` composed a prompt out of it, and the driver
    /// delivered that prompt into a live pseudoterminal.
    ///
    /// ⚠⚠⚠ **A RUN THAT NAMES NO CONSENTS GETS THIS REPOSITORY'S OWN** — the carrying, gated at the
    /// one place it happens.
    ///
    /// # Why this needs a gate of its own
    ///
    /// The clauses used to be authored in `ai_loop.scxml`, and a run that named none got the
    /// document's. That made this repository's standing yesses authorise every run of a file other
    /// repositories copy, so they moved to `debt_loop.scxml` — and the template now ships an EMPTY
    /// list. **Something has to carry them across, and a carrier nothing observes is a carrier that
    /// can quietly drop what it carries.** What that looks like from outside is a run that comes up
    /// perfectly configured and stops at its first permission dialog: measured once already, on a
    /// live loop that stood there until an iteration ceiling ended it.
    ///
    /// ⚠ The count is asserted against what the KIND holds rather than against `2`, so an author
    /// adding a third clause to their own document does not have to come and edit a number here —
    /// and so this cannot pass by agreeing with a literal that drifted.
    ///
    /// ⚠⚠⚠⚠⚠ **A LOOP THIS DAEMON STARTS KEEPS ITS REVIEWS' COUNTS IN THIS DAEMON'S STATE
    /// DIRECTORY** — the one line only the daemon can write, gated where dropping it would be
    /// invisible.
    ///
    /// # ⚠⚠⚠ Why the library must NOT answer this and once did
    ///
    /// `context_review.scxml` authors a bare file name and says a driver resolves it *"against the
    /// daemon's state directory"*. `sprag-plugin` implemented that by reading `$XDG_STATE_HOME`
    /// itself — so under `cargo test`, where there is no daemon, *the daemon's state directory*
    /// meant **the home of whoever ran the suite**. Measured 2026-08-19: thirty lines per
    /// `cargo test -p sprag-plugin --lib`, and 179 standing in a shared build machine's real
    /// `~/.local/state/sprag/context-review.jsonl`. CI's `ambient-home-guard` had been failing on
    /// exactly that write.
    ///
    /// The library cannot name a home any more, which is the fix. **What that moves here is the
    /// power to forget**: a daemon that drops the assignment builds a run which comes up looking
    /// perfectly configured, reviews normally, and keeps counts nobody can ever compare with the
    /// next run's — [`sprag_plugin::AiLoop::keeping_counts_in`]'s whole reason, and the same shape
    /// as the consents gate below it.
    ///
    /// ⚠⚠ Compared against [`crate::durability::state_dir`] rather than against a literal, because
    /// a literal here would be a SECOND derivation of the path — the exact duplication that
    /// function exists to prevent — and would drift the day the state directory moves.
    #[test]
    fn a_loop_this_daemon_starts_keeps_its_counts_in_this_daemons_state_directory() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );

        let asked = ai_loop_request(pane, json!({}));
        let (built, _label) = external
            .build_plugin(asked.as_object().expect("an object"))
            .expect("a plain ai_loop request is well-formed");
        let PluginKind::AiLoop(loops) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };

        let expected = crate::durability::state_dir();
        assert!(
            expected.is_absolute(),
            "⚠⚠⚠ THE CONTROL: a state directory that is not absolute would make the assertion \
             below pass while the counts landed relative to whatever directory the daemon happened \
             to be started in. Got {expected:?}",
        );
        assert_eq!(
            loops.keeping_counts_in(),
            Some(expected.as_path()),
            "⚠⚠⚠⚠⚠ A RUN THIS DAEMON BUILT MUST CARRY THIS DAEMON'S STATE DIRECTORY. `None` here \
             is the daemon's one line gone: nothing fails, no run stops, and the loop simply stops \
             keeping the readings that make *is this getting better?* a question with an answer. \
             Any OTHER directory is a second derivation of a path this daemon already owns",
        );
    }

    /// ⚠⚠ AND THE CONTROL IS THE OTHER DIRECTION: a caller who DOES name consents must still win.
    /// Without it, "the kind is consulted" and "the kind always wins" are the same green, and the
    /// second one silently discards what a caller asked for.
    #[test]
    fn a_run_that_names_no_consents_gets_this_repositorys_own() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );

        // What this repository's kind document holds — the authority the assertion compares to.
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let owed = sprag_plugin::kind::LoopKind::debt(script)
            .expect("this repository's kind document must open")
            .consents()
            .expect("its clause list must be readable")
            .expect("a debt run answers dialogs");
        assert!(
            !owed.clauses().is_empty(),
            "the control: the kind must ship clauses, or every assertion below is vacuous",
        );

        let asked = ai_loop_request(pane, json!({}));
        let (built, _label) = external
            .build_plugin(asked.as_object().expect("an object"))
            .expect("a run that names no consents is well-formed");
        let PluginKind::AiLoop(loops) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };
        let carried = loops
            .consenting()
            .expect("the run's clause list must be readable")
            .expect(
                "⚠⚠⚠ THE RUN CAME UP ANSWERING NOTHING. The template ships an empty list on \
                     purpose and the kind document holds the clauses; a run that reaches its first \
                     permission dialog with none stops there and waits for somebody who is not \
                     watching",
            );
        assert_eq!(
            carried.clauses().len(),
            owed.clauses().len(),
            "⚠⚠⚠ every clause this repository authored must reach the run. Carried {carried:?}, \
             authored {owed:?}",
        );
        for clause in owed.clauses() {
            assert!(
                carried
                    .clauses()
                    .iter()
                    .any(|got| got.asked() == clause.asked() && got.answer() == clause.answer()),
                "⚠⚠ and each one whole — a clause that arrived with half its text claims a dialog \
                 it cannot answer: {clause:?} missing from {carried:?}",
            );
        }

        // ── THE CONTROL: A CALLER WHO NAMES CONSENTS STILL WINS ──
        let named = ai_loop_request(
            pane,
            json!({ Consents::WIRE_KEY: [{ Consent::ASKED_KEY: "only this", Consent::ANSWER_KEY: "and only this" }] }),
        );
        let (built, _label) = external
            .build_plugin(named.as_object().expect("an object"))
            .expect("a run that names its own consents is well-formed");
        let PluginKind::AiLoop(loops) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };
        let carried = loops
            .consenting()
            .expect("readable")
            .expect("a caller's own list is not nothing");
        assert_eq!(
            carried.clauses().len(),
            1,
            "⚠⚠⚠ A CALLER'S OWN CONSENTS MUST WIN OVER THE KIND'S. Falling back is what an ABSENT \
             key means; overriding a present one would discard what somebody asked for, and would \
             make the assertion above pass for the wrong reason. Got {carried:?}",
        );
    }

    /// ⚠⚠ **AND IT IS CANCELLED RATHER THAN RUN TO CONVERGENCE**, deliberately. Convergence needs a
    /// supervisor that can call this peer's turns over, which is `sprag-plugin`'s own gate against
    /// its `supervised` fixture. What is measured HERE is the door, and a gate that also waited for
    /// an ending would be two claims wearing one name.
    #[test]
    fn a_loop_started_over_the_wire_prompts_its_agent_with_what_the_caller_briefed() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );

        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");

        let access = sprag_plugin::WorkspacePaneAccess::new(Arc::clone(&workspace));
        let began = Instant::now();
        let mut screen = String::new();
        while began.elapsed() < Duration::from_secs(20) {
            screen = access.pane_collapsed(pane).unwrap_or_default();
            if screen.contains("SPRAG-NORTH-STAR-CROSSED-THE-WIRE") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            screen.contains("SPRAG-NORTH-STAR-CROSSED-THE-WIRE"),
            "⚠⚠⚠ the caller's own north star must be on the agent's screen — that string is the \
             whole chain from a wire request to a prompt in a pseudoterminal. Screen: {screen:?}, \
             run: {:?}",
            // ⚠ The run's own record, because a screen that is missing the prompt cannot say WHY:
            // a refused barrier and a machine that never left `idle` look identical from here.
            lock(&registry)
                .snapshot()
                .first()
                .map(|run| run_to_json(run, run.opened_by)),
        );

        assert!(
            lock(&registry).cancel(RunId(id)),
            "the run this call started is one the registry can stop",
        );
        let entry = ended(&registry, id, Duration::from_secs(30));
        assert_eq!(
            entry["state"]["outcome"]["state"],
            json!("cancelled"),
            "⚠⚠ AND IT IS THE RUN REGISTRY'S OWN CANCEL that ends it, not a bound this gate \
             invented — a loop is a run like any other the day it is a plugin: {entry:?}",
        );
        assert_eq!(
            entry["label"],
            json!(format!("ai_loop pane={}", pane.0)),
            "a reader of `runs` must be able to see WHICH pane a loop is driving: {entry:?}",
        );
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⚠⚠⚠ **A BRIEF THIS BUILD CANNOT DRIVE TO THE END IS REFUSED AT THE DOOR, NAMING THE KNOB** —
    /// and the DOCUMENT'S OWN SHIPPED NUMBERS ARE NO LONGER ONE OF THEM.
    ///
    /// # ⚠⚠⚠ What this gate asserted until `restarting` was built
    ///
    /// `ai_loop.scxml` ships `reflect_every: 8` beside `max_turns: 40`, so the DEFAULT numbers walk
    /// into `reflecting` at turn eight — and this surface REFUSED them, because the session-replace
    /// lifecycle behind that state did not exist. A caller who copied the numbers off the document got
    /// a sentence telling them to raise `reflect_every`.
    ///
    /// It is built, so **the shipped pair is now a RUN**, and that is asserted here rather than left
    /// as an absence: a refusal that quietly stopped happening would leave the wire's own grammar
    /// documenting a constraint nothing enforces.
    ///
    /// ⚠⚠ The refusal that REMAINS is the other end of the same arithmetic — a loop allowed no turn at
    /// all — and it is still SYNCHRONOUS and still carries a sentence, which is this surface's own
    /// rule: a caller's mistake is answered at the door with what to change, never as an `outcome` a
    /// minute later.
    #[test]
    fn a_loop_briefed_into_an_unbuilt_state_is_refused_with_the_knob_that_fixes_it() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );

        // ⚠⚠⚠ A LOOP ALLOWED NO TURN, which is the one arm of this refusal that is left.
        let refused = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({ "max_turns": 0 }))),
            )
            .expect_err("a loop allowed no turns can only judge itself exhausted");
        let sentence = refused
            .reason()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            sentence.contains("max_turns"),
            "⚠⚠⚠ the refusal must name the knob, because a caller cannot act on a sentence that \
             names none: {sentence:?}",
        );
        assert!(
            lock(&registry).snapshot().is_empty(),
            "⚠⚠ AND NO RUN SLOT WAS TAKEN. A refusal that had already registered a run would have \
             spent the thing refusing early exists to save",
        );

        // ⚠⚠⚠ AND THE DOCUMENT'S OWN SHIPPED PAIR IS A RUN. This used to be the REFUSED case; the
        // session-replace lifecycle it needed is built, and a caller copying the template's numbers
        // gets the loop the template describes.
        external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(
                    pane,
                    json!({ "max_turns": 40, "reflect_every": 8 }),
                )),
            )
            .expect(
                "⚠⚠⚠ the template's own `reflect_every: 8` against `max_turns: 40` must START — it \
                 reaches `reflecting`, and `reflecting` is served",
            );
        lock(&registry).cancel_all();
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THIS REPOSITORY'S KIND DOCUMENT REACHES A RUN THAT NAMED NOTHING** — register item
    /// 492, and the gate that closes a hole the register had already measured and registered.
    ///
    /// # ⚠⚠⚠⚠⚠ What was green while it was broken
    ///
    /// Nine of a brief's fields fall back to `debt_loop.scxml`, and **deleting any one of those
    /// fall-throughs left the entire workspace GREEN.** That was measured for `max_turns` on item
    /// 312's round and written into `sprag_plugin`'s own gate as a registered residue; item 492's
    /// round measured it again for `context_ceiling` and got the same answer. The consequence is
    /// not hypothetical: the kind had authored a ceiling since 2026-08-18, nothing carried it, and
    /// item 477 measured `reviewing` taking the fall-back **eight times out of eight** on a live
    /// run — a state that never once decided, with every gate over it passing.
    ///
    /// # ⚠⚠⚠ Why this can exist now and could not before
    ///
    /// The residue named its own blocker: *"what would catch it is an observable of the RESOLVED
    /// value on a run started through the wire"*, and the driver's readers are crate-private to
    /// `sprag_plugin`. `ai_loop_brief` is that observable — the door's own resolution, handed back
    /// instead of consumed in place. **This asks the real function what a real request plus the real
    /// kind document resolve to**, which is why it is not a test re-implementing the line it checks.
    ///
    /// ⚠⚠ It asserts the AGREEMENT rather than a number: what the kind's document says is that
    /// document's business, and a number pinned here would be a second place it lives. What must
    /// hold is that the two are the same value and that it is one `reviewing` can decide on.
    #[test]
    fn a_kind_documents_judgements_reach_a_run_that_named_none_of_them() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // The fixture minus every key the kind is meant to answer, which is the only way to ask
        // whose value arrived.
        let mut declining = ai_loop_request(PaneId(1), json!({}));
        declining
            .as_object_mut()
            .expect("an object")
            .remove("max_turns")
            .expect("the fixture supplies the key this gate declines");
        let map = declining.as_object().expect("an object");

        let brief = ai_loop_brief(map, &kind).expect("a well-formed request resolves");

        let ceiling = brief.context_ceiling.expect(
            "⚠⚠⚠⚠⚠ ITEM 492: a run that named no ceiling must arrive holding THIS REPOSITORY'S — \
             the kind document has authored one since 2026-08-18 and until the door carried it, \
             `reviewing` decided on 0 on every run anybody has ever driven",
        );
        assert_eq!(
            Some(ceiling),
            kind.context_ceiling(),
            "and it must be the kind's own number rather than one this door invented",
        );
        assert!(
            ceiling > 0,
            "⚠⚠⚠ and a number `reviewing` can decide on: every deciding edge in that state is \
             guarded on `context_ceiling > 0`, so a zero is the fall-back this item exists to get a \
             run out of. Read {ceiling}",
        );

        // ⚠⚠⚠ AND ITS NEIGHBOURS ON THE SAME ROAD, which is what makes this a CLASS gate rather
        // than a second copy of the ceiling's. Each of these was equally unheld, and the register's
        // own measurement was taken against the first of them.
        assert_eq!(
            brief.max_turns,
            kind.turn_budget(),
            "⚠⚠⚠⚠ the BUDGET is the one the residue was measured on (item 312): a debt run ends on \
             its work, and that decision is a word in the kind's document that nothing carried",
        );
        assert_eq!(
            brief.reflect_every,
            kind.reflect_every(),
            "⚠⚠ and the cadence, which a kind that declines the budget MUST answer or no run of it \
             starts at all",
        );
        assert_eq!(
            brief.milestone_check,
            kind.milestone_check(),
            "⚠⚠⚠ and WHO CERTIFIES A MILESTONE — item 428's second half, where a live run judged \
             `NOTHING CHECKED THAT CLAIM` while this document named a checker",
        );
        assert_eq!(
            brief.closing_rules,
            kind.closing_rules(),
            "and what a run of this kind owes at its ending",
        );
        assert_eq!(
            brief.service.is_some(),
            kind.service_outage().is_some(),
            "and what its peer prints when the service fails, which turned a dead run into a wait",
        );
        assert!(
            brief.may_answer.is_some(),
            "⚠⚠⚠ and the standing yesses: an empty consent list met `Do you want to make this \
             edit?` on the first milestone and stood there until a ceiling ended the run",
        );
        // ⚠⚠⚠⚠⚠ AND THE CEILING'S TWIN — register item 494, and the reason this gate's CLASS
        // framing earned its keep: the sentence in the template that sent a reader to
        // `context_ceiling` says the same thing about this number, and it went a whole further
        // round with no reader, no field and no key.
        assert_eq!(
            brief.reflect_after_refusals,
            kind.reflect_after_refusals(),
            "⚠⚠⚠⚠⚠ ITEM 494: how patient to be with a refusing check is this document's judgement \
             about its own checker — a whole `claude -p` per claimed milestone here — and until the \
             door carried it, `judging` spent the template's three on every run anybody has ever \
             driven",
        );
        assert_eq!(
            brief.reflect_after_refusals,
            Some(2),
            "⚠⚠ and this repository's document says 2 rather than the template's 3, which is what \
             makes the line above evidence instead of two `None`s agreeing: item 448 gave every \
             refusal the check's own words, so the template's own comment says a kind that finds \
             three slack now has a fact it did not have then",
        );

        // ⚠⚠⚠ THE CONTROL. Without it a door that IGNORED every caller and always used the kind's
        // values would satisfy every assertion above — which is the opposite defect and just as
        // silent.
        let named = ai_loop_request(PaneId(1), json!({ "context_ceiling": 4242 }));
        let brief = ai_loop_brief(named.as_object().expect("an object"), &kind)
            .expect("a caller naming a ceiling resolves");
        assert_eq!(
            brief.context_ceiling,
            Some(4242),
            "a caller's own number must still win over the kind document's",
        );
    }

    /// ⚠⚠⚠⚠ **A CALLER CAN DECLINE THE BUDGET AND LET THE DOCUMENT DECIDE** — item 312, PAID, and
    /// this gate is the one that measured the defect, turned around rather than deleted.
    ///
    /// # What it said the round before
    ///
    /// `max_turns` was `required` on `AI_LOOP_FORM`, so `ai_loop.scxml`'s own
    /// `<data id="max_turns" expr="40"/>` was unreachable from every caller there is: omitting the
    /// key was malformed rather than deferring. **A required judgement is a decision the document
    /// is structurally forbidden from making** — a harder case than item 300's two durations, which
    /// were already optional and so already meant *the document decides* when left out.
    ///
    /// ⚠⚠ The refusal also named nothing: `require_count` answered a bare
    /// [`InvokeError::TypeMismatch`], so somebody who declined the key learnt neither that it was
    /// mandatory nor that a 40 was waiting — while every neighbouring refusal here names the knob
    /// or the file. Both halves go together, because the key is declinable now.
    ///
    /// ⚠⚠⚠ **WHAT THIS DOOR CAN AND CANNOT SAY.** It answers *the call is accepted*; it cannot see
    /// the datamodel, so *the run is bounded by 40* is asserted where the document lives —
    /// `sprag_plugin`'s `a_declined_budget_is_the_documents_own`. Neither gate is the whole claim.
    #[test]
    fn a_caller_can_decline_max_turns_and_let_the_document_decide() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );

        // The well-formed request this whole module uses, with the one key taken back out — which
        // is the only way to ask the question, since the fixture supplies it like every caller.
        let mut declined = ai_loop_request(pane, json!({}));
        declined
            .as_object_mut()
            .expect("an object")
            .remove("max_turns")
            .expect("the fixture supplies the key this gate declines");

        external
            .invoke(RUN_ACTION, IntrospectValue::Json(declined))
            .expect(
                "⚠⚠⚠⚠ ITEM 312: declining `max_turns` must DEFER to the document rather than be \
                 malformed. This expectation is the inverse of the one that measured the defect, \
                 and it is deliberately the same call",
            );
        assert_eq!(
            lock(&registry).snapshot().len(),
            1,
            "⚠⚠ and a deferred budget starts a real run, not a nothing that reports success",
        );

        // ⚠⚠⚠ THE CONTROL, AND IT IS THE HALF THAT SURVIVED THE FIX UNCHANGED. Making the key
        // declinable must not stop a caller who names a number from being obeyed — and without
        // this, a product that ignored `max_turns` entirely would satisfy everything above.
        external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({ "max_turns": 3 }))),
            )
            .expect("a caller who names their own budget is obeyed");
        lock(&registry).cancel_all();
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⚠⚠⚠ **HALF OF THE TURN CONTRACT IS MALFORMED, IN BOTH DIRECTIONS.**
    ///
    /// `done_when` says what makes the peer's turn over; `turn_within_ms` says how long it may
    /// take. The two halves are NOT symmetric, and a conformance gate is what taught that:
    ///
    /// * **a bound with no contract** is REFUSED. It would quietly become *"wait this long and then
    ///   type at it anyway"*, which is the 500 ms timer the caller was plainly trying to get away
    ///   from, with a bigger number — doing MORE than asked, silently.
    /// * **a contract with no bound is a RUN**, and the first draft refused it.
    ///   `every_published_word_is_a_word_the_plugin_host_accepts` named that immediately: the wire
    ///   publishes `done_when`'s two words, so an agent that enumerates the vocabulary sends the
    ///   word ALONE and must be served rather than told its own call is malformed. **That gate has
    ///   now caught this same argument twice** — the first time was its companion at version 25.
    ///   Alone it means what it says: wait for the peer to finish, bounded by the run's own clock.
    ///
    /// # ⚠⚠ Why no per-argument harness could have caught the refused half
    ///
    /// [`a_handback_for_a_run_nobody_is_watching_is_malformed`]'s reason exactly, one contract
    /// over: the conformance sweeps drive ONE argument at a time — wrong type, declined, absent —
    /// and this request is well-typed, well-spelt, and wrong only in what it is missing.
    #[test]
    fn a_turn_contract_missing_half_of_itself_is_malformed() {
        let paired = json!({
            "done_when": "exits",
            sprag_plugin::Turn::WIRE_KEY: 12_000,
        });
        assert!(
            matches!(
                opt_turn(paired.as_object().expect("an object")),
                Ok(Some(_))
            ),
            "⚠ THE CONTROL FIRST: the pair these keys exist in is accepted, or the refusals below \
             are about a parser that refuses everything",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ "done_when": "exits" })
                        .as_object()
                        .expect("an object")
                ),
                Ok(Some(_)),
            ),
            "⚠⚠⚠ AND THE CONTRACT ALONE IS A RUN, not a refusal — this wire PUBLISHES the word, so \
             an agent that enumerated the vocabulary sends exactly this and must be served",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ sprag_plugin::Turn::WIRE_KEY: 12_000 })
                        .as_object()
                        .expect("an object")
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ and a bound with no contract is REFUSED rather than read as a bigger timer, which \
             is the behaviour the caller was getting away from",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ "done_when": "exits", sprag_plugin::Turn::WIRE_KEY: 0 })
                        .as_object()
                        .expect("an object")
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠ and a bound of ZERO is malformed — `await_person_ms`'s rule: *wait no time at all \
             for my peer to finish* is not a thing a caller can mean",
        );
        assert!(
            matches!(
                opt_turn(json!({}).as_object().expect("an object")),
                Ok(None)
            ),
            "⚠⚠⚠ AND THE DEFAULT IS SILENCE MEANING TODAY'S BEHAVIOUR: a caller who names neither \
             gets the step timeout their request has always got, or an added argument would have \
             changed what every existing call does",
        );
    }

    /// ⚠⚠⚠ **THE LOOP'S BOUND MOVED INTO ITS DOCUMENT AND THE WIRE'S ANSWERS DID NOT MOVE WITH
    /// IT** — the residue register item 300's move could have left, asked directly.
    ///
    /// Nothing on the `ai_loop` form builds a [`Turn`] any more: `done_when` binds a run to its
    /// peer and stays on the spec, `turn_within_ms` is a judgement and writes `<data>`. Three
    /// answers had to survive that, and only the first is obvious:
    ///
    /// * **A BOUND ALONE IS A RUN HERE**, where [`opt_turn`] refuses it. That asymmetry is older
    ///   than this round and its reason is the loop's default contract: an `agent` run defaults to
    ///   `exits`, so a bare bound bounds something nobody chose, and a loop defaults to
    ///   [`INNER_SESSION_ENDS`](sprag_plugin::INNER_SESSION_ENDS) — the contract its document makes
    ///   load-bearing — so a bare bound bounds exactly the turn the caller means.
    /// * **ZERO IS STILL REFUSED.** This is the one the move could have broken silently: the old
    ///   code handed the number to `Turn::lasting`, which refuses zero, and a `<data>` reads zero
    ///   as *the author declines a bound*. Had the number simply flowed through, a request the
    ///   wire REFUSED would have become a RUN — the direction R385 registered as earning a
    ///   protocol bump, arrived at by deleting a constructor rather than by deciding anything.
    /// * **AND SILENCE IS STILL SILENCE**, which is what lets the document decide.
    ///
    /// ⚠ It is the `ai_loop` form's own reader that is asked, because that is the only place the
    /// three answers are decided; [`opt_turn`] still serves the forms that build a `Turn`.
    #[test]
    fn a_loops_turn_bound_travels_to_its_document_without_changing_the_wires_answers() {
        let ms = |value: Value| {
            opt_ai_loop_turn_ms(
                json!({ sprag_plugin::Turn::WIRE_KEY: value })
                    .as_object()
                    .expect("an object"),
            )
        };
        assert!(
            matches!(ms(json!(12_000)), Ok(Some(12_000))),
            "⚠ THE CONTROL: a bound ALONE is a run on this form and reaches the document as the \
             number sent — no `done_when` beside it, which is `opt_turn`'s rule and not this one",
        );
        assert!(
            matches!(ms(json!(0)), Err(InvokeError::TypeMismatch)),
            "⚠⚠⚠ AND ZERO IS STILL REFUSED. In the document a zero means *no bound of my own*, so \
             a parser that let it through would turn a refusal into a run — silently, by having \
             stopped calling the constructor that owned the rule",
        );
        assert!(
            matches!(
                opt_ai_loop_turn_ms(json!({}).as_object().expect("an object")),
                Ok(None),
            ),
            "⚠⚠ and silence is silence: a caller who names no bound is not overriding the \
             document, which is the whole point of the move",
        );
        assert!(
            matches!(ms(json!(null)), Ok(None)),
            "⚠ and an explicitly declined key is the same as an absent one, which is what every \
             other optional argument on this surface does",
        );
    }

    /// ⛔⛔⛔ **A HOLD CEILING REACHES THE DOCUMENT, IS REFUSED AT ZERO, AND NEEDS NO PERSON BESIDE
    /// IT** — register item 534, on the door a caller actually calls.
    ///
    /// # ⚠⚠⚠⚠ The third assertion is the one the item is about
    ///
    /// The first two are this surface's ordinary rules restated. The third is the whole finding:
    /// `hold_within_ms` is **well-formed with no `await_person_ms` beside it**, which is the exact
    /// opposite of `handback_still_ms`'s rule in the gate below. Those two are one request about
    /// somebody EXPECTED, enforced by [`Handback`] living inside `Attended::APerson` — and a hold is
    /// an ORDER, which a run nobody is watching can be given. **That population is item 534's
    /// entire population**: the runs that parked for ever were the unattended ones, so a parser
    /// that demanded a watching person here would have refused the ceiling exactly where it was
    /// needed and left the defect standing behind a well-intentioned pairing rule.
    ///
    /// ⚠⚠ ZERO IS REFUSED for `await_person_ms`'s reason, sharpened: *hold this run and end it at
    /// once* is `cancel` spelled wrong, so accepting it would give a caller who reached zero by
    /// arithmetic a run that dies the first time anybody pauses it to read a pane.
    ///
    /// ⚠ AND SILENCE IS SILENCE, which is what lets `ai_loop.scxml` decide — the same answer every
    /// other optional duration on this form gives since register item 300.
    #[test]
    fn a_hold_ceiling_travels_alone_and_a_zero_one_is_refused() {
        let sent = |body: Value| opt_hold_within(body.as_object().expect("an object"));
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 900_000 })),
                Ok(Some(within)) if within == Duration::from_millis(900_000),
            ),
            "⚠ THE CONTROL: a ceiling a caller sends must reach the document as the number sent, or \
             the key is decoration",
        );
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 0 })),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ AND ZERO IS REFUSED. *Hold this run and end it at once* is `cancel` spelled wrong, \
             and a caller who arrived at zero by arithmetic must be told rather than handed a run \
             that dies the first time somebody pauses it",
        );
        // ⚠⚠⚠⚠ THE ITEM'S OWN ASSERTION: no person declared, and the request stands.
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 60_000 })),
                Ok(Some(_)),
            ),
            "⛔⛔⛔ REGISTER ITEM 534: a hold ceiling sent WITHOUT `await_person_ms` must be \
             well-formed. A hold is an order and not a contract about who is watching — and the \
             runs that parked for ever were precisely the unattended ones, so pairing this key \
             with a person would refuse the ceiling in the only population that needed it",
        );
        assert!(
            matches!(sent(json!({})), Ok(None)),
            "⚠⚠ and silence is silence: a caller who names no ceiling defers to `ai_loop.scxml`'s \
             own, which is what every optional duration on this form has meant since item 300",
        );
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: null })),
                Ok(None)
            ),
            "⚠ and an explicitly declined key is the same as an absent one",
        );
    }

    /// ⚠⚠⚠ **HALF OF A PAIRED REQUEST IS MALFORMED — `handback_still_ms` WITH NOBODY WATCHING.**
    ///
    /// A caller who sends it alone has plainly asked for a run that waits for a person. There is no
    /// `Attended` value that can carry their request ([`Handback`] lives inside `APerson`), and the
    /// two answers a daemon could give instead are both worse than a refusal: `NoOne` hands them a
    /// run that ENDS on the first keystroke — the opposite of what they sent, silently — and
    /// inventing a patience would be a bound nobody chose, on a run somebody may be waiting on.
    ///
    /// # ⚠⚠ Why no per-argument harness could have caught this
    ///
    /// The three conformance sweeps this surface runs drive ONE argument at a time: at the wrong
    /// type, declined as `null`, absent. This rule is about a PAIR — well-typed, well-spelt, and
    /// wrong only in what it is missing — so it is the shape those sweeps are blind to by
    /// construction, and it needs a gate of its own.
    #[test]
    fn a_handback_for_a_run_nobody_is_watching_is_malformed() {
        let paired = json!({
            sprag_plugin::Attended::WIRE_KEY: 20_000,
            sprag_plugin::Handback::WIRE_KEY: 400,
        });
        assert!(
            matches!(
                opt_attended(paired.as_object().expect("an object")),
                Ok(Attended::APerson { .. }),
            ),
            "⚠ THE CONTROL FIRST: the pair this key exists in is accepted, or the refusal below is \
             about a parser that refuses everything",
        );
        let alone = json!({ sprag_plugin::Handback::WIRE_KEY: 400 });
        assert!(
            matches!(
                opt_attended(alone.as_object().expect("an object")),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ and the half-request is REFUSED rather than quietly answered `NoOne`, which would \
             give the caller a run that ends on the first keystroke while their call asked the \
             daemon to wait",
        );
        let zero = json!({
            sprag_plugin::Attended::WIRE_KEY: 20_000,
            sprag_plugin::Handback::WIRE_KEY: 0,
        });
        assert!(
            matches!(
                opt_attended(zero.as_object().expect("an object")),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠ and a stillness of ZERO is malformed too — `await_person_ms`'s own rule, for its \
             reason: every person pauses between keystrokes, so a run given zero would type into \
             the gap between their words",
        );
        assert!(
            matches!(
                opt_attended(json!({}).as_object().expect("an object")),
                Ok(Attended::NoOne),
            ),
            "⚠ and neither key is still `NoOne`, which is what every run did before either existed",
        );
    }

    /// No settle window at all — the injected policy this path takes as a parameter, so a test of a
    /// TIMED transition is not asserting about a timing the developer's `config.toml` chose.
    fn instant_window() -> sprag_detect::Hysteresis {
        sprag_detect::Hysteresis {
            settle: Duration::ZERO,
        }
    }

    /// A REAL pane whose child paints `bytes` and then holds its pty open.
    ///
    /// A live PTY and the live emulator, not a synthetic screen: the subject here is the whole path
    /// from a child's output to what a plugin is told, and the two ends of it are exactly what a
    /// hand-built `Screen` would skip.
    fn pane_painting(bytes: &str) -> (Arc<Mutex<Workspace>>, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%b' '{bytes}'; exec cat"));
        command.env("TERM", "xterm-256color");
        let id = lock(&workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the pane");
        (workspace, id)
    }

    /// The `claude` permission dialog R249 captured, as the bytes a child would print: the OSC
    /// title first (the IDLE glyph, which is what makes this a test about arbitration), then the
    /// dialog.
    const PERMISSION_SCREEN: &str = "\\033]0;\\342\\234\\263 Remove temporary directory\\007\
         \\r\\n Do you want to allow Claude to fetch this content?\
         \\r\\n \\342\\235\\257 1. Yes\
         \\r\\n   2. Yes, and don'\\''t ask again for example.com\
         \\r\\n   3. No, and tell Claude what to do differently (esc)";

    /// A `claude` at rest, and the same pane a moment later with the braille spinner in its title —
    /// the two screens a turn passes between, in the bytes a child prints.
    ///
    /// The MARKER on each is what a test waits for without observing: the title is not on the
    /// screen, so a fixture that waited for the title would have to ask the detector, and asking the
    /// detector is the sampling these tests are about.
    const CLAUDE_AT_REST: &str =
        "\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[Hat rest %s\\r\\n";
    const CLAUDE_WORKING: &str =
        "\\033]2;\\342\\240\\213 Claude Code\\007\\033[2J\\033[Hworking\\r\\n";

    /// A pane that paints ON COMMAND: it announces `GO`, then paints the next of `screens` for each
    /// Enter it is sent.
    ///
    /// **It says when its terminal is ready** (R347): a `sh -c` peer takes milliseconds to reach its
    /// `stty`, and a test that injected before then would have its Enter echoed back into the pane.
    ///
    /// **Its line discipline stays CANONICAL**, unlike R347's peer, and that is deliberate: `read`
    /// wants a line, every act here is an Enter, and canonical mode is what turns the carriage
    /// return a keystroke encodes into the newline the shell is waiting for. Echo is off, so the
    /// keystroke itself paints nothing and the screen holds only what the script printed.
    ///
    /// The point of a pane that paints on command rather than on a timer: a turn's boundaries become
    /// the TEST's to place, so "a turn that began and ended between two looks" is an assertion and
    /// not a race.
    fn pane_painting_in_turn(screens: &[String]) -> (Arc<Mutex<Workspace>>, PaneId) {
        let mut script = String::from("stty -echo; printf 'GO\\r\\n'");
        for screen in screens {
            script.push_str(&format!("; read -r _; printf '%b' '{screen}'"));
        }
        script.push_str("; exec cat");
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "xterm-256color");
        let id = lock(&workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the pane");
        (workspace, id)
    }

    /// Wait for `needle` on the pane's screen WITHOUT asking the detector anything.
    ///
    /// The distinction this whole pair of tests rests on: [`settle`] polls the supervision source,
    /// and every such poll is a look at the screen. A test about what a look MISSES cannot wait by
    /// looking.
    fn wait_for_screen(access: &WorkspacePaneAccess, id: PaneId, needle: &str) {
        let start = Instant::now();
        let mut last = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            last = access.pane_collapsed(id).unwrap_or_default();
            if last.contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the pane never painted {needle:?}; its screen was {last:?}");
    }

    /// Send one Enter to the pane — the act that advances [`pane_painting_in_turn`] to its next
    /// screen.
    fn advance(access: &WorkspacePaneAccess, id: PaneId) {
        let written = access
            .inject(id, &[sprag_plugin::KeyStroke::named("Enter")])
            .expect("the pane takes a keystroke");
        assert!(
            written.bytes() > 0,
            "an Enter that wrote nothing would advance nothing",
        );
    }

    /// Poll the source until `ready`, or give up — the pane's child has to run and its bytes have to
    /// reach the emulator, and neither is synchronous.
    fn settle(
        source: &sprag_plugin::AgentStateSource,
        id: PaneId,
        ready: impl Fn(&sprag_plugin::AgentObservation) -> bool,
    ) -> sprag_plugin::AgentObservation {
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < Duration::from_secs(10) {
            if let Some(seen) = source(id) {
                if ready(&seen) {
                    return seen;
                }
                last = Some(seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the pane never reached the state this test is about; last: {last:?}");
    }

    fn source(
        workspace: &Arc<Mutex<Workspace>>,
        agents: &Arc<crate::AgentClock>,
    ) -> sprag_plugin::AgentStateSource {
        agent_state_source(Arc::clone(workspace), Arc::clone(agents), instant_window)
    }

    /// An outcome with `state`, and nothing else that matters here.
    fn finished(state: OutcomeState, answered: u32) -> Outcome {
        Outcome {
            state,
            iterations: 1,
            cost: None,
            failure: None,
            stopped: None,
            answered,
            screened: 0,
        }
    }

    /// The measured shape of a real permission dialog, as a run's outcome carries it.
    fn a_question() -> sprag_detect::Question {
        sprag_detect::Question {
            asked: vec!["Do you want to proceed?".to_owned()],
            choices: vec![
                sprag_detect::Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                sprag_detect::Choice {
                    number: 2,
                    label: "No".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// ⚠⚠⚠ **A BLOCKED RUN ALWAYS SAYS WHY IT DID NOT ANSWER, EVEN WHEN IT HAS NO QUESTION.**
    ///
    /// Two runs that stop on the same dialog look identical to a client unless the reason travels:
    /// one was given no consent (fix: write one) and one was given a consent that named nothing on
    /// offer (fix: the needle). Those are different actions, and until R366 the answer carried
    /// neither.
    ///
    /// ⚠ The `unreadable` half is the one with NO question at all — a pane blocked on something
    /// this host cannot parse as a menu. It published as an ABSENCE and explained nowhere; the
    /// remedy (a person) lived in a doc comment. Here the key is present and the word says so.
    #[test]
    fn a_blocked_run_publishes_why_it_did_not_answer_with_or_without_the_question() {
        let refused = finished(
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::refused(
                a_question(),
                sprag_plugin::Refusal::NotOffered,
            ))),
            0,
        );
        let answer = outcome_to_json(&refused);
        assert_eq!(answer["state"], "blocked");
        let asking = &answer[RUN_ASKING_KEY];
        assert_eq!(asking[RUN_WHY_KEY], "not_offered");
        assert_eq!(asking[RUN_ASKED_KEY][0], "Do you want to proceed?");
        assert_eq!(
            asking[RUN_CHOICES_KEY][0]["selected"], true,
            "and where a bare Enter would land, which is what a person answering it needs",
        );

        // ⚠ NO QUESTION, and the key that says so is the one that is never absent.
        let unreadable = finished(
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::unreadable())),
            0,
        );
        let answer = outcome_to_json(&unreadable);
        assert_eq!(answer[RUN_ASKING_KEY][RUN_WHY_KEY], "unreadable");
        assert!(
            answer[RUN_ASKING_KEY].get(RUN_ASKED_KEY).is_none(),
            "the question is ABSENT rather than empty — a caller tells `this host could not read \
             it` from `it had no lines` by the key's presence: {answer}",
        );
        assert!(
            sprag_plugin::Refusal::parse(
                answer[RUN_ASKING_KEY][RUN_WHY_KEY]
                    .as_str()
                    .expect("a word"),
            )
            .is_some(),
            "and every word published here is one the type spells, never a literal: {answer}",
        );
    }

    /// ⚠⚠ **EVERY OUTCOME SAYS HOW MANY DECISIONS THE RUN TOOK ON SOMEBODY'S BEHALF** — including
    /// `0`, and including the runs that did not end well.
    ///
    /// The key's neighbours (`ceiling`, `stopped`) are absent when they have nothing to say,
    /// because their absence means *nothing of this kind happened* and costs a reader nothing.
    /// This one is a count of APPROVALS, so *"this run answered nothing"* has to be readable as a
    /// claim rather than inferred from a key nobody wrote — and a run that answered a dialog and
    /// then hit its iteration ceiling has to report both.
    #[test]
    fn every_outcome_says_how_many_of_its_peers_questions_it_answered() {
        for state in [
            OutcomeState::Converged,
            OutcomeState::Cancelled,
            OutcomeState::Failed,
            OutcomeState::Exhausted(Ceiling::Iterations),
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::unreadable())),
        ] {
            let quiet = outcome_to_json(&finished(state.clone(), 0));
            assert_eq!(
                quiet[RUN_ANSWERED_KEY], 0,
                "⚠ PRESENT and zero, not absent — see `RUN_ANSWERED_KEY`: {quiet}",
            );
            let spoke = outcome_to_json(&finished(state, 3));
            assert_eq!(
                spoke[RUN_ANSWERED_KEY], 3,
                "and a run that answered says so whatever became of it afterwards: {spoke}",
            );
        }
    }

    /// ⚠⚠ **THE CONSENT IS READ THROUGH THE TYPE, so what this surface accepts and what the type
    /// admits are one predicate.**
    ///
    /// The two needles are open strings on the wire, which makes the EMPTY one the whole risk: an
    /// empty `asked` is carried by every question and an empty `answer` by every option, so either
    /// turns a narrow consent into something the caller did not write. `Consent::parse` owns that
    /// refusal and this holds the parser to it — R352's shape, where a `String` argument admits
    /// fewer values than its type.
    ///
    /// ⚠ And an absent key is a run that answers NOTHING, which is the default the whole feature
    /// rests on.
    ///
    /// # ⚠⚠⚠ The shape is a LIST, and the two ways that can go wrong are BOTH malformed
    ///
    /// An EMPTY list, because `[]` and an absent key would otherwise be two spellings of *"answer
    /// nothing"* — and the one that arrives by accident (a client whose clause list came from a
    /// filter that matched nothing) is exactly the caller who wants telling. And the PRE-BUMP
    /// OBJECT, which is what a version-28 client sends a version-29 daemon: it must meet the
    /// grammar at the door rather than be read as a one-clause list, because a shape this wire
    /// quietly reinterprets is one no version number can protect.
    #[test]
    fn the_consent_this_surface_reads_is_the_one_the_type_admits() {
        let asked = "Do you want to proceed?";
        let clause = |asked: &str, answer: &str| json!({ Consent::ASKED_KEY: asked, Consent::ANSWER_KEY: answer });
        let good = json!({ Consents::WIRE_KEY: [clause(asked, "Yes")] });
        assert_eq!(
            opt_may_answer(good.as_object().expect("an object")).expect("a well-formed consent"),
            Consents::of(vec![
                Consent::parse(asked.to_owned(), "Yes".to_owned()).expect("two needles"),
            ]),
            "the surface builds exactly what the type would",
        );

        let many = json!({
            Consents::WIRE_KEY: [clause(asked, "Yes"), clause("make this edit", "Yes")],
        });
        assert_eq!(
            opt_may_answer(many.as_object().expect("an object")).expect("two well-formed clauses"),
            Consents::of(vec![
                Consent::parse(asked.to_owned(), "Yes".to_owned()).expect("two needles"),
                Consent::parse("make this edit".to_owned(), "Yes".to_owned()).expect("two needles"),
            ]),
            "⚠⚠⚠ EVERY clause arrives, and IN THE CALLER'S ORDER — a parser that kept only the \
             first would leave an unattended run stopping at the second question of every turn, \
             which is the defect the list exists to close. Compared WHOLE rather than by count, so \
             a parser that read two clauses and built them from one object fails here too",
        );

        for (label, sent) in [
            ("absent", json!({})),
            (
                "declined as null",
                json!({ Consents::WIRE_KEY: Value::Null }),
            ),
        ] {
            assert_eq!(
                opt_may_answer(sent.as_object().expect("an object")).expect("well-formed"),
                None,
                "⚠⚠ {label} is a run that may answer NOTHING — the default every run had before \
                 this key existed, and the reason answering is opt-in",
            );
        }

        for (label, sent) in [
            (
                "an empty question needle",
                json!({ Consents::WIRE_KEY: [clause("", "Yes")] }),
            ),
            (
                "an empty option needle",
                json!({ Consents::WIRE_KEY: [clause(asked, "")] }),
            ),
            (
                "no option needle at all",
                json!({ Consents::WIRE_KEY: [{ Consent::ASKED_KEY: asked }] }),
            ),
            (
                "a bare string where the list goes",
                json!({ Consents::WIRE_KEY: "Yes" }),
            ),
            (
                "an EMPTY list, which is not a second spelling of the default",
                json!({ Consents::WIRE_KEY: [] }),
            ),
            (
                "a bare string INSIDE the list",
                json!({ Consents::WIRE_KEY: ["Yes"] }),
            ),
            (
                "⚠⚠⚠ the PRE-BUMP object a version-28 client sends",
                json!({ Consents::WIRE_KEY: clause(asked, "Yes") }),
            ),
            (
                "one good clause beside a malformed one",
                json!({ Consents::WIRE_KEY: [clause(asked, "Yes"), clause("", "Yes")] }),
            ),
        ] {
            assert!(
                matches!(
                    opt_may_answer(sent.as_object().expect("an object")),
                    Err(InvokeError::TypeMismatch),
                ),
                "⚠⚠⚠ {label} is a MALFORMED request and must meet the grammar at the door — \
                 accepting it would authorise an answer to a question the caller never named",
            );
        }
    }

    /// A plugin reads what the agent in its pane is DOING, and what it is blocked ON — through the
    /// extension API, off a live pane, with no second detector anywhere.
    ///
    /// This is the whole of the supervision requirement in one assertion. Before it, a plugin's
    /// view of a blocked agent was the pane's text: it could see the dialog and had to re-derive
    /// what the daemon had already decided, and every plugin author would have re-derived it
    /// differently.
    ///
    /// The title is the IDLE glyph, deliberately — that is what a real blocked `claude` shows
    /// (R249's measurement, and the reason `Rule::priority` exists), so a surface that read the
    /// title alone would report this pane at rest while it waits for a person.
    #[test]
    fn a_plugin_reads_a_blocked_agents_state_and_the_question_it_is_blocked_on() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);

        let seen = settle(&read, id, |o| o.state == AgentState::Blocked);
        assert_eq!(seen.agent.as_deref(), Some("claude"));
        assert_eq!(
            seen.authority,
            sprag_plugin::Authority::Scraped {
                rule: Some("dialog-choice-list".to_owned()),
            },
            "a screen-read verdict must say so, and say which rule said it",
        );
        assert!(
            !seen.authority.is_exact(),
            "a scrape is a sample of an animation, and a supervisor must be able to know that",
        );

        let asking = seen.asking.as_ref().expect("the question it is blocked on");
        assert_eq!(
            asking
                .choices
                .iter()
                .map(|c| (c.number, c.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "Yes"),
                (2, "Yes, and don't ask again for example.com"),
                (3, "No, and tell Claude what to do differently (esc)"),
            ],
        );
        assert_eq!(asking.selected().map(|c| c.number), Some(1));
        assert!(
            asking
                .asked
                .iter()
                .any(|line| line.contains("allow Claude to fetch")),
            "the sentence a policy classifies: {:?}",
            asking.asked,
        );
        assert!(asking.choice(4).is_none(), "a number nobody offered");

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// The two authorities, on ONE pane, told apart by the type.
    ///
    /// The same screen answers `blocked` by SCRAPING it and `working` because the process inside
    /// said so — and a report outranks the screen, which is exactly why a consumer must be able to
    /// see which one it has. A supervisor treating a scrape as a turn boundary is treating a sample
    /// of a spinner as an event.
    #[test]
    fn a_report_from_inside_the_pane_is_marked_exact_and_a_scrape_is_not() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);

        let scraped = settle(&read, id, |o| o.state == AgentState::Blocked);
        assert!(!scraped.authority.is_exact());

        let (outcome, _) = agents.report(
            id,
            Report {
                state: AgentState::Working,
                agent: Some("claude".to_owned()),
                source: "claude-hook".to_owned(),
                seq: Some(1),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            instant_window,
        );
        assert!(outcome.accepted, "the hook's report must be taken");

        let reported = read(id).expect("the pane is still an agent's");
        assert_eq!(reported.state, AgentState::Working);
        assert_eq!(
            reported.authority,
            sprag_plugin::Authority::Reported {
                source: "claude-hook".to_owned(),
            },
        );
        assert!(
            reported.authority.is_exact(),
            "the process inside the pane said so; nothing was sampled",
        );
        assert!(
            reported.asking.is_none(),
            "a working pane is not waiting on the menu still painted behind it",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A pane the AGENT ITSELF reported blocked still carries the question off its screen.
    ///
    /// The branch the exact path most needs and the one a design could easily lose: a report is the
    /// authority on the STATE and says nothing about the menu, because the hook fires on an event
    /// and the options are pixels. If the question were tied to the verdict's provenance, the
    /// accurate path would be the blind one — the supervisor would know a person is needed and not
    /// what for, exactly when it has the best information it will ever have.
    #[test]
    fn a_pane_its_own_agent_reported_blocked_still_carries_the_question() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        settle(&read, id, |o| o.state == AgentState::Blocked);

        let (outcome, _) = agents.report(
            id,
            Report {
                state: AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "claude-hook".to_owned(),
                seq: Some(1),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            instant_window,
        );
        assert!(outcome.accepted);

        let seen = read(id).expect("an agent");
        assert!(
            seen.authority.is_exact(),
            "the state came from inside the pane",
        );
        let asking = seen
            .asking
            .as_ref()
            .expect("...and the question still came from the screen");
        assert_eq!(asking.choices.len(), 3);
        assert_eq!(asking.selected().map(|c| c.number), Some(1));

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A turn that begins and ends BETWEEN two pulls is still visible — which is the whole reason
    /// this surface is a level and not an event stream.
    ///
    /// The measurement this answers was taken against a rival that publishes agent state as change
    /// EVENTS: a one-second turn produced no event at all, and the supervising machine waited
    /// forever for a turn that had already finished. Here the second pull reads `idle` — the same
    /// value the first one did, so the STATE really is no help — and `seq` says two changes
    /// happened in between. Nothing was lost; it was carried as a level.
    #[test]
    fn a_turn_that_starts_and_ends_between_two_pulls_is_not_lost() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        settle(&read, id, |o| o.state == AgentState::Blocked);

        // The pull a supervisor takes before the turn.
        let hook = |state: AgentState, seq: u64| Report {
            state,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };
        agents.report(id, hook(AgentState::Idle, 1), instant_window);
        let before = read(id).expect("an agent");
        assert_eq!(before.state, AgentState::Idle);

        // A whole turn, entirely between the two pulls: the agent starts and finishes.
        agents.report(id, hook(AgentState::Working, 2), instant_window);
        agents.report(id, hook(AgentState::Idle, 3), instant_window);

        let after = read(id).expect("an agent");
        assert_eq!(
            after.state, before.state,
            "the STATE is the same at both pulls, so it cannot be what tells them apart",
        );
        assert!(
            after.seq > before.seq,
            "a turn happened between the pulls and the level must carry that: {} -> {}",
            before.seq,
            after.seq,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// THE PREMISE, MEASURED: a turn the pane really performed, between two looks, leaves the scrape
    /// with nothing to report — the same answer it gives for a pane that never worked at all.
    ///
    /// This is the control the SCE requirement's §5 rests on, and it is here because the requirement
    /// arrived as a claim about somebody else's observer (*"a one-second turn produced no observable
    /// working state at all"*) and a project rule says a handed-over premise is measured before it is
    /// built for. Measured here against sprag's own detector, driving the screens a real `claude`
    /// paints, it reproduces — and the mechanism is sharper than "the sample rate is too low":
    ///
    /// **A scrape's evidence is DESTROYED by the next paint.** The working state lives in the pane's
    /// TITLE, a terminal holds one, and the agent overwrites it the instant the turn ends. So it is
    /// not that a look is unlikely to land inside a short turn; it is that after the turn there is
    /// nothing left for any number of looks to find. No poll interval closes this, which is why the
    /// answer is the agent reporting rather than sprag sampling harder — see the twin below.
    ///
    /// The turn here is not even short: it lasts as long as this test takes to paint it. What makes
    /// it invisible is only that nobody looked DURING it, which is the case a supervisor cannot
    /// prevent and cannot detect.
    #[test]
    fn a_turn_the_scrape_did_not_look_during_leaves_no_trace_of_having_happened() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
            CLAUDE_AT_REST.replace("%s", "two"),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");

        // The pull a supervisor takes before the turn.
        let before = settle(&read, id, |o| o.state == AgentState::Idle);
        assert!(!before.authority.is_exact(), "this pane reports nothing");

        // The whole turn, and nobody looks: the agent starts working...
        advance(&access, id);
        wait_for_screen(&access, id, "working");
        // ...and finishes.
        advance(&access, id);
        wait_for_screen(&access, id, "at rest two");

        // The pull a supervisor takes after it.
        let after = read(id).expect("the pane is still an agent's");
        assert_eq!(
            after.state,
            AgentState::Idle,
            "the pane is at rest, which is true and is not the question",
        );
        assert_eq!(
            after.seq, before.seq,
            "the turn happened and the scrape can say nothing about it: {} -> {}",
            before.seq, after.seq,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// THE ANSWER, on the same pane painting the same turn: the agent's own hook reports each
    /// boundary, and the turn is there to be read afterwards.
    ///
    /// The twin of the test above, differing in exactly one thing — whether the agent said anything —
    /// so what it proves is attributable. Both pulls read `idle`, exactly as before; the difference
    /// is entirely in `seq`, which moved by the two changes the turn is made of.
    ///
    /// This is what `--settings` buys ([`crate::pane_args_source`]): the report is made AT the
    /// boundary by the process that alone knows where the boundary is, so it does not depend on
    /// anybody looking, and it survives the next paint.
    #[test]
    fn the_same_turn_reported_by_the_agent_is_still_there_to_be_read() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
            CLAUDE_AT_REST.replace("%s", "two"),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let hook = |state: AgentState, seq: u64| Report {
            state,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");
        agents.report(id, hook(AgentState::Idle, 1), instant_window);
        let before = read(id).expect("an agent");
        assert_eq!(before.state, AgentState::Idle);

        // The same turn, with nobody looking — and the agent saying so at each edge.
        advance(&access, id);
        agents.report(id, hook(AgentState::Working, 2), instant_window);
        wait_for_screen(&access, id, "working");
        advance(&access, id);
        agents.report(id, hook(AgentState::Idle, 3), instant_window);
        wait_for_screen(&access, id, "at rest two");

        let after = read(id).expect("an agent");
        assert_eq!(
            after.state, before.state,
            "the STATE is the same at both pulls, exactly as in the scraped twin",
        );
        assert_eq!(
            after.seq,
            before.seq + 2,
            "and the two edges of the turn are still there: {} -> {}",
            before.seq,
            after.seq,
        );
        assert!(
            after.authority.is_exact(),
            "an answer that came from inside the pane: {:?}",
            after.authority,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// ⚠⚠⚠⚠⚠ **A SUPERVISOR IS TOLD HOW MANY TIMES A PANE HAS BEEN SPOKEN FOR** — register item
    /// 458, at the seam where the count crosses from the tracker into the surface a plugin reads.
    ///
    /// # ⚠⚠⚠⚠ Why this gate is in THIS crate and could not be written in `sprag-plugin`
    ///
    /// The silence ceiling is decided in the plugin, and every gate there builds an
    /// `AgentObservation` — so all of them would go on passing if this line stopped carrying the
    /// number. **A fixture that supplies the very field the product omits cannot see the omission**;
    /// that is item 428's shape, and item 459 is a live example of exactly it one crate over. So
    /// the assertion belongs where the ADAPTER is, driven through
    /// [`AgentClock::report`](crate::AgentClock::report) — the door a hook's payload actually
    /// arrives by.
    ///
    /// # ⚠⚠⚠ What makes it the right number rather than any number
    ///
    /// The two reports below say the SAME THING TWICE — `working`, then `working` — which is
    /// precisely a turn calling tool after tool. So `seq` cannot move, and if it did this would be
    /// measuring the wrong counter. What must move is this one, because it counts REPORTS and not
    /// verdicts, and it is the only sign of life a turn like that leaves.
    #[test]
    fn a_supervisor_is_told_how_many_times_a_pane_has_been_spoken_for() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let hook = |seq: u64| Report {
            state: AgentState::Working,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");

        // ── THE CONTROL FIRST: a pane nothing has reported for ──
        //
        // ⚠⚠⚠⚠⚠ ITS ANSWER IS ZERO AND ALWAYS WILL BE, and that is why a caller may not read zero
        // as silence: it means *this pane has no reporter to be silent*, not *nobody is speaking*.
        // Without this reading, `reports` moving below could be a counter that starts anywhere.
        let scraped = read(id).expect("the pane is an agent's from its screen alone");
        assert_eq!(
            (scraped.reports, scraped.authority.is_exact()),
            (0, false),
            "a pane read from its SCREEN has been spoken for by nobody, and says so through its \
             authority as well as its count: {scraped:?}",
        );

        agents.report(id, hook(1), instant_window);
        let first = read(id).expect("an agent");
        // The same turn, still working, still calling tools — one more report saying nothing new.
        agents.report(id, hook(2), instant_window);
        let second = read(id).expect("an agent");

        assert_eq!(
            (second.seq, second.state),
            (first.seq, first.state),
            "⚠⚠⚠ THE FIXTURE: these two reports must publish NOTHING, or this gate is about `seq` \
             after all. A turn calling tool after tool reports `working` every time and the \
             verdict never moves — which is the whole reason a fourth counter had to exist",
        );
        assert_eq!(
            (first.asked_seq, first.said_seq),
            (second.asked_seq, second.said_seq),
            "and neither counter of STATEMENTS moves either: a turn in flight has stated no \
             question and no answer, so all three stand still together",
        );
        assert_eq!(
            (first.reports, second.reports),
            (1, 2),
            "⚠⚠⚠⚠⚠ AND THIS ONE MOVES, EVERY REPORT, WHATEVER IT SAID. It is the only thing left \
             that separates a peer working slowly from a peer that has stopped speaking, and a \
             supervisor that never receives it is back where the fourteen measured minutes were: \
             `working seq=6 asked=2 said=0`, indistinguishable from a turn nothing will ever end. \
             Got {first:?} then {second:?}",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A host with no detector says it cannot supervise, and that is a DIFFERENT answer from a pane
    /// that is not an agent's.
    ///
    /// Collapsing the two would let a supervisor conclude "no agents here" from a build that never
    /// looked — the same class of confident wrong answer `Landing::Unplaced` and
    /// `AgentState::Unknown` are each shaped to avoid.
    #[test]
    fn a_host_with_no_detector_says_so_rather_than_reporting_no_agents() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);

        let blind = WorkspacePaneAccess::new(Arc::clone(&workspace));
        assert!(
            blind.supervision().is_none(),
            "a host with no detector must not answer questions about agents at all",
        );

        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let seeing = WorkspacePaneAccess::new(Arc::clone(&workspace))
            .with_agent_state(Some(source(&workspace, &agents)));
        let supervision = seeing
            .supervision()
            .expect("a host WITH a detector supervises");
        // ...and on that host, a pane no manifest claims is the other answer: `None` for this pane,
        // from a surface that exists.
        assert!(
            supervision.pane_agent_state(PaneId(9999)).is_none(),
            "a pane nobody knows is not an agent",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// **REQ §5, the door**: a pane a PLUGIN spawns is told which pane it is and where the daemon
    /// listens — so the agent inside it can report its own turn boundaries instead of being guessed
    /// at from its screen.
    ///
    /// The exact/approximate split is only worth having if the EXACT half is reachable, and the
    /// exact half is a hook inside the agent's own process calling back: it needs the pane's id and
    /// the daemon's address, both published into the child's environment at birth. Every other pane
    /// gets them because the mux surface spawns it. A plugin's pane goes through a different door,
    /// and R337 is this project's record of what that costs — "two doors" onto pane birth turned out
    /// to be FIVE, and the one this layer owns carried a comment claiming the host filled something
    /// in that the host did not.
    ///
    /// So it is asserted rather than trusted to the structure. What the child prints is what the
    /// child was given; the reporting half on the other end of that address is `hooks.rs`'s and is
    /// tested there.
    #[test]
    fn a_pane_a_plugin_spawns_is_told_which_pane_it_is_and_where_to_report() {
        let socket = std::path::Path::new("/tmp/sprag-plugin-door.probe");
        let workspace = Arc::new(Mutex::new(Workspace::new((60, 6))));
        lock(&workspace).set_pane_env_source(crate::pane_env_source(socket));

        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let address = sprag_rpc::HOST_SOCKET.path_env;
        let pane = access
            .lifecycle()
            .expect("the plugin surface spawns panes")
            .spawn(
                &[
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    format!(
                        "printf 'PANE=%s AT=%s' \"${{{}-unset}}\" \"${{{address}-unset}}\"; exec cat",
                        crate::PANE_ENV_VAR,
                    ),
                ],
                60,
                6,
            )
            .expect("spawn");

        let want = format!("PANE={} AT={}", pane.0, socket.display());
        let start = Instant::now();
        let mut seen = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            seen = access.pane_collapsed(pane).unwrap_or_default();
            if seen.contains(&want) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            seen.contains(&want),
            "a plugin-spawned pane's child must know its own pane and the daemon's address; \
             wanted {want:?}, screen was {seen:?}",
        );
        let closed = lock(&workspace).close(pane);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }
    /// A live plugin host over a workspace holding two panes — the fixture the three grammar gates
    /// drive, plus its own non-vacuity counts.
    ///
    /// TWO panes because `pipe` names a `src` and a `dst`, and a fixture holding one of a thing cannot
    /// tell an argument that resolved from one the verb ignored (the mux fixture's rule, one surface
    /// along).
    fn grammar_gate(
        claim: impl Fn(
            &'static [crate::wire::ActionGrammar],
            sprag_conformance::Invoke<'_>,
        ) -> sprag_conformance::Driven,
    ) -> sprag_conformance::Driven {
        let (workspace, _first) = pane_painting("");
        {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            lock(&workspace)
                .spawn(command, "second".to_string(), 80, 24)
                .expect("a second pane the addressing arguments can name");
        }
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
        );
        claim(crate::wire::PLUGINS_GRAMMAR, &mut |action, args| {
            external.invoke(action, args)
        })
    }

    /// ⚠⚠ **EVERY WORD THIS SURFACE PUBLISHES IS A WORD IT ACCEPTS.**
    ///
    /// ⚠ Some of these calls START A RUN, which is what makes the claim real: the run is spawned on a
    /// background thread against a pane the fixture holds, and the registry goes out of scope with the
    /// test. A `plugin` word that got as far as spawning is a word the parser read.
    #[test]
    fn every_published_word_is_a_word_the_plugin_host_accepts() {
        assert_eq!(
            grammar_gate(sprag_conformance::every_published_word_is_accepted).count_or_panic(),
            32,
            "one call per published word: the ONE plugin word that selects each of the SIX forms, \
             the two reply formats on each of a dialogue's two endpoints, the readiness barrier's \
             FOUR `match` words on each of the four plugins that inject — the last two being \
             `runs` and `settles`, which ask the pane's terminal and its supervisor rather than \
             its screen — and `done_when`'s TWO words on EACH of the three forms that now take it. \
             ⚠⚠⚠ THE SEVEN NEWEST ARE THE `ai_loop` FORM'S, and this gate caught the same argument \
             a THIRD time on it: `agent` was published as declinable and read with `require_str`, \
             so a caller building the minimal call this grammar describes was answered \
             `TypeMismatch`. It is required now, which is what it always was. \
             ⚠⚠⚠ Those four are why this gate is worth its own line, and it has caught the SAME \
             argument TWICE. `done_when`'s first draft published `settles` and the parser REFUSED \
             it, because that draft needed a companion `agent` the vocabulary could not demand. \
             The orchestrator's copy repeated it exactly: its first draft required \
             `turn_within_ms` alongside, so an agent that enumerated this vocabulary would have \
             built a call the daemon rejected. **A published word must be servable ALONE** — which \
             is why a turn contract with no bound is a run bounded by the run's own clock rather \
             than a refusal.",
        );
    }

    /// ⚠⚠ **AN ARGUMENT THIS SURFACE CONSTRAINS PUBLISHES WHAT IT ADMITS** — and it is why the two
    /// bad-word arms answer `TypeMismatch` now.
    ///
    /// A vocabulary the daemon refuses as `Rejected` is INVISIBLE to this gate: the probe comes back
    /// refused for a reason the gate cannot read as a grammar refusal, so a closed argument would look
    /// open and pass. Both of this surface's vocabularies were in that state — `plugin` and
    /// `format_a`/`format_b` each answered a friendly sentence — so the gate could not have held them
    /// even after they were published.
    #[test]
    fn an_argument_the_plugin_host_constrains_publishes_what_it_admits() {
        assert_eq!(
            grammar_gate(sprag_conformance::a_constrained_argument_publishes_what_it_admits)
                .count_or_panic(),
            26,
            "one probe per open string argument of every form. ⚠⚠ THE NEWEST TWO ARE A SCREEN \
             RULE's `when` and `text`, open for the consent needles' reason exactly: `when` quotes \
             the AGENT's own dialog and `text` is the AUTHOR's own prose about their own work, so \
             a closed vocabulary at either could only ever be sprag's guess. THE OLD SENTENCE \
             FOLLOWS. An orchestrator's stimulus, \
             sentinel and ready_when, a PIPE's ready_when, an agent's prompt and ready_when, and \
             a dialogue's seed and two labels — PLUS the ANSWERING CONTRACT's two needles on each \
             of the FOUR forms that inject, the newest pair being the loop's. \
             ⚠⚠ The five before them are the `ai_loop` FORM'S OWN: its \
             three BRIEF strings, the `agent` its barrier is derived from, and its own \
             `ready_when` marker. The brief's three are open for the consent needles' reason \
             turned around — a north star is a PERSON's prose about their own work, so a closed \
             vocabulary there could only ever be sprag's guess at what somebody is trying to do. \
             ⚠ Both of those are open on purpose and it is the \
             one place on this surface where that is a safety property rather than a convenience: \
             a consent quotes the AGENT's own words, so a closed vocabulary here could only ever \
             be sprag's guess at what dialogs say",
        );
    }

    /// ⚠⚠ **A DECLARED ARGUMENT IS ONE THIS SURFACE ACTUALLY READS** — the gate that lets this table
    /// be hand-written, over a verb whose forms were transcribed from a parser by eye.
    ///
    /// ⚠ The number moved by twelve when the loop got a door, and both halves are the point: four
    /// `opened_by` arguments (one per form) and **eight nested `guardrails` fields the claim could
    /// not see before it learned to walk them**. `max_iterations` and each form's cost key are now
    /// each driven at the wrong type inside their parent, which is what turns the nested grammar
    /// from a published claim into a held one.
    /// ⚠⚠ **EVERY OPTIONAL ARGUMENT OF THIS SURFACE MAY BE DECLINED AS `null`** — the class a
    /// hand-written check cannot close, because it is the arguments nobody thought about that are
    /// wrong.
    ///
    /// Found live: `sentinel: null` answered `TypeMismatch` while `ready_when: null` and
    /// `ready_timeout_ms: null` did not, so the SAME request was well-formed or malformed depending
    /// on which optional the client declined. A client whose language serialises absence as `null`
    /// — most of them — could not start an orchestrator run at all without a sentinel.
    #[test]
    fn an_optional_argument_of_a_run_may_be_declined_as_null() {
        assert_eq!(
            grammar_gate(sprag_conformance::an_optional_argument_may_be_declined_as_null)
                .count_or_panic(),
            74,
            "one probe per OPTIONAL declared argument of every form, nesting included — required \
             ones are deliberately not driven, because `null` for something the grammar demands is \
             malformed rather than declined. ⚠⚠⚠⚠⚠ THE NEWEST IS `hold_within_ms` (item 534), and \
             declining it means what declining the two duration keys beside it means since item \
             300: THIS DOCUMENT DECIDES — `ai_loop.scxml`'s own four hours. ⚠⚠ Zero is NOT a value \
             a caller may mean here, unlike `context_ceiling` and `reflect_after_refusals` below: \
             *hold this run and end it at once* is `cancel` spelled wrong, so it is refused rather \
             than obeyed — and that rule is about a VALUE, which no per-argument sweep can see (see \
             `a_hold_ceiling_travels_alone_and_a_zero_one_is_refused`). ⚠ It is also the one key \
             here that needs NO person declared beside it, which is the whole of item 534: the runs \
             that parked for ever were the unattended ones. THE OLD SENTENCE FOLLOWS. THE NEWEST IS \
             `reflect_after_refusals` (item \
             494), and declining it means what declining `context_ceiling` beside it means, one \
             number over: the caller's, then THIS repository's KIND document, then the template's \
             own `expr=\"3\"`. ⚠⚠ It is here because the CLASS was swept rather than the instance — \
             the template claims two of its numbers for the kind, 492 paid one, and the other was \
             still authorable by nobody. ⚠ Zero is a value a caller may MEAN (reflect on the first \
             refusal), so there is no decline word beside it. THE OLD SENTENCE FOLLOWS. \
             THE NEWEST IS `context_ceiling` (item 492), and \
             declining it means what declining `max_turns` means with one more step in the chain: \
             the caller's number, then THIS repository's KIND document, then the template's own \
             `expr=\"0\"`. ⚠⚠ Its arrival is the item itself rather than a detail of it — the kind \
             document had authored a ceiling since 2026-08-18 and nothing could carry it, so \
             `reviewing` guarded every deciding edge on a number that was 0 on every run anybody \
             has ever driven (item 477 measured eight exits out of eight taking the fall-back). \
             ⚠ Zero is a value a caller may MEAN here, so unlike `max_turns` there is no decline \
             word beside it. THE OLD SENTENCE FOLLOWS. THE NEWEST IS `max_turns`, and it is the one \
             argument on this surface that has ever moved from REQUIRED to declinable (item 312): \
             the document authors `expr=\"40\"` and, while the key was mandatory, no caller could \
             let it decide — so a judgement the owner's rule puts in the `.scxml` was one the \
             `.scxml` was structurally forbidden from making. ⚠⚠ Its arrival here is the point of \
             this sweep rather than a detail of it: declining it now has to mean the same thing \
             declining anything else here means, `null` included, and nothing but a per-argument \
             probe would have said so. ⚠⚠⚠ THE ONE BEFORE IT IS `screen_rules`, and declining it \
             means something no other optional here means: NOT *screen nothing*, but *keep whatever \
             the loop document's author wrote*. The rules live in the template, so a caller who \
             says nothing about screening is not overriding one who did — and the driver echoes the \
             document's own rules back through the brief rather than deleting them. ⚠⚠ ITS TWO \
             NESTED FIELDS ARE **NOT** AMONG THESE, and this gate is what said so rather than a \
             reading of the grammar: a nested field is REQUIRED inside its object, so `null` for it \
             is malformed and not declined — exactly as the consent's two needles are absent here. \
             THE OLD SENTENCE FOLLOWS. THE THREE BEFORE IT ARE THE ANSWERING CONTRACT ON \
             THE LOOP, and their declinability is the whole default: a loop that names no consent \
             answers nothing and reports the question, which is what every loop did before the \
             keys existed — and what was measured costing it every turn it had. \
             ⚠⚠ The eleven before them are the `ai_loop` FORM'S own, and \
             what is NOT among them is the point: the brief's four and the `agent` are REQUIRED, \
             because a loop with no purpose and a loop with no barrier are both runs nobody can \
             mean. ⚠⚠ `reflect_every` IS declinable, and its default is STILL `max_turns` — which \
             used to mean *the one number that keeps the run inside the states this build drives* and \
             now means something else entirely: `reflecting` is served, so that default is a CHOICE \
             rather than a limit. A restart closes a pane somebody may be reading, so a caller who \
             said nothing about reflection has not asked for one; what they get anyway is the \
             reflection a STANDING INSTRUCTION triggers, which is a correctness edge, not a budget. \
             ⚠⚠⚠ The TWO before them are the orchestrator's turn \
             contract, `done_when` and `turn_within_ms`, and their declinability IS the default \
             that keeps every existing caller working: a run that names neither ends its steps on \
             the same 500 ms constant it always did. ⚠ Declinable ALONE is all this drives; that \
             HALF the pair may not be sent alone is a rule no per-argument sweep can see — see \
             `a_turn_contract_missing_half_of_itself_is_malformed`. ⚠⚠⚠ The THREE before them are \
             `handback_still_ms` on each \
             LOOPING form, and its declinability is the default that keeps every existing caller \
             working: a run that names no stillness ends when somebody takes its pane, which is \
             what every run did before the key existed. ⚠ Declinable ALONE is all this drives; that \
             it may not be SENT alone is a rule about a PAIR, which no per-argument sweep can see \
             — see `a_handback_for_a_run_nobody_is_watching_is_malformed`. ⚠⚠ The THREE before \
             them are `await_person_ms` on each \
             LOOPING form, and its declinability is the whole default in the same way the \
             consent's is: a run that names no patience is unattended and ends when its peer asks \
             something no clause covers, which is what every run did before the key existed. \
             ⚠ It is not on the `answer` form, which is CALLED BY the person a wait would wait \
             for. ⚠ The three before them are `may_answer` on each injecting \
             form, and its declinability is the whole default: a run that names no consent answers \
             nothing and reports the question, which is what every run did before the key existed. \
             ⚠⚠ The FIVE this round added are the `answer` form's own optionals — its `opened_by` \
             and its three guardrail fields — and NOT its consent, which is the one argument on \
             this surface that a form REQUIRES: `may_answer` is declinable on the looping \
             forms and mandatory on the one whose whole content it is",
        );
    }

    #[test]
    fn a_declared_argument_is_one_the_plugin_host_reads() {
        assert_eq!(
            grammar_gate(sprag_conformance::a_declared_argument_is_one_the_daemon_reads)
                .count_or_panic(),
            119,
            "one probe per declared argument of every FORM, nesting included: TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, THIRTY-ONE to run an AI loop, one to cancel, and ONE TO STAND \
             A RUN DOWN. ⚠⚠⚠⚠⚠ THE NEWEST IS THE LOOP'S THIRTY-FIRST, `hold_within_ms` (item 534), \
             and this gate is what makes it more than a declaration for its two predecessors' \
             reason: a published argument the host does not READ is a key the surface swallows \
             while the run reports `ok`. ⚠⚠ IT IS ON THIS FORM ALONE, unlike the two person keys it \
             sits beside — the ceiling is a `<data>` in `ai_loop.scxml` and that document is the \
             only thing in this workspace that reads a hold at all, so declaring it on the other \
             three LOOPING forms would advertise an argument they swallow, which is the exact \
             defect this gate exists to catch. THE OLD SENTENCE FOLLOWS. THE LOOP'S THIRTIETH WAS \
             `reflect_after_refusals` (item \
             494), and this gate is what makes it more than a declaration for the same reason it \
             did for its twin: a published argument the host does not READ is a key the surface \
             swallows while the run reports `ok`. ⚠⚠ The twin is the point — the template claims \
             exactly two of its numbers for the KIND to author, item 492 built the road for one, \
             and the identical defect was still standing one `<data>` up with a GATE for its only \
             writer. THE OLD SENTENCE FOLLOWS. THE NEWEST IS THE LOOP'S TWENTY-NINTH, \
             `context_ceiling` (item \
             492), and this gate is the one that makes it more than a declaration: a published \
             argument the host does not READ is a key the surface swallows while the run reports \
             `ok`. That is the whole shape of the item — the number existed in the kind's document \
             since 2026-08-18 and nothing carried it, so `reviewing` decided on 0 for every run \
             this repository has ever driven. THE OLD SENTENCE FOLLOWS. ⚠⚠⚠ THE NEWEST IS THAT \
             LAST ONE — the second thing anybody can say to a \
             run, and the first that does not throw the turn in flight away. It takes a run id and \
             nothing else, exactly as `cancel` does, and it is a SEPARATE verb for that reason \
             rather than in spite of it: the two shapes are identical and the outcomes are \
             opposite, so a mode flag on one of them would let a caller lose a milestone by \
             mistyping a boolean. THE OLD SENTENCE FOLLOWS. one probe per declared argument of \
             every FORM, nesting included: TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, TWENTY-EIGHT to run an AI loop, and one to cancel. ⚠⚠⚠ THE \
             NEWEST THREE ARE `screen_rules` AND ITS TWO NESTED FIELDS — the loop author's standing \
             instructions, and the SECOND authority over one dialog. A consent takes an option the \
             peer OFFERED, which structurally cannot cover the question a loop meets when its \
             agent wants a DECISION (*the quick way or the thorough way?* offers nothing anybody \
             could authorise in advance); a rule refuses the call and says what to do instead. ⚠ It \
             names no KEY, and that is a safety property rather than a simplification: the key is \
             the product's, measured, and a rule that could name its own could name the one that \
             APPROVES — a live probe pressed `Tab` and had the agent's file written. THE OLD \
             SENTENCE FOLLOWS. TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, TWENTY-FIVE to run an AI loop, and one to cancel. \
             ⚠⚠⚠ THE NEWEST FIVE ARE THE ANSWERING CONTRACT REACHING THE LOOP — `may_answer` with \
             its two needles, `await_person_ms` and `handback_still_ms`. It was the ONE injecting \
             form without them, on the argument that answering a dialog belongs to a state in the \
             document; that state is unbuilt, and the cost was measured as a loop whose agent \
             asked one permission question stopping with ZERO turns judged. \
             ⚠⚠⚠ THE NEWEST TWENTY ARE THE `ai_loop` FORM, the door register item 65 had been \
             holding open since R378 — five rounds built that loop's machine, its driver and its \
             live measurement, and nothing in the daemon constructed one. FOUR of the twenty are \
             the BRIEF (`north_star`, `milestone`, `reference`, `max_turns`), which is the one \
             thing on this whole surface that no other form has: every other plugin is told what \
             to TYPE, and a loop is told what it is FOR and composes each turn's prompt from that \
             itself. ⚠ `agent` is required beside them for a measured reason — a loop with no \
             barrier types its first prompt into whatever the pane happens to be running, which \
             R379 measured costing a whole run. \
             ⚠⚠⚠ The two before them are the ORCHESTRATOR's \
             TURN CONTRACT — `done_when`, which the `agent` form already had, and `turn_within_ms` \
             — and they are on that form because it is where the defect was MEASURED: without them \
             a step ends on a 500 ms constant, so a peer that thinks for three seconds was asked \
             its one question SIX times, every prompt after the first landing while it was still \
             answering. The `agent` adapter never had that defect, because it asks a contract \
             instead of a clock; this is that contract offered to the plugin the MCP verb and the \
             outer AI loop actually drive. ⚠ NOT on `pipe`, which is a scope cut and not a \
             judgement — a relay's destination has turns too. ⚠⚠⚠ The THREE before them are \
             `handback_still_ms`, on each form that LOOPS and on none that does not — the second \
             half of `turn.interrupted`, which shipped with only the first: a run learnt to STOP \
             for a person and had no way to be given the pane back. It is not on the `answer` \
             form for its neighbour's reason, doubled: that form is CALLED BY the person, so a run \
             waiting for their hand to go still would be waiting for its own caller to stop \
             calling it. ⚠⚠ The THREE before them are `await_person_ms`, on \
             each form that LOOPS and on none that does not — the other half of the answering \
             contract: what the run may answer itself, and who answers what it may not. The \
             `answer` form is the one injecting form without it, because its caller IS the person \
             a wait would be waiting for. ⚠ The TEN before them are the whole `answer` form, \
             which is the answering contract with NO LOOP AROUND IT: a pane, a consent, and the \
             bounds every run carries. It declares no stimulus and no readiness barrier, and both \
             absences are the design — the only bytes it can emit are the ones the consent \
             authorised, and a pane whose program has not started cannot be showing a dialog. \
             ⚠ The nine before them are the ANSWERING CONTRACT on the three forms that \
             inject — `may_answer` and its two needles — which completes the turn's three declared \
             contracts: when it may START (`ready_when`), what makes it OVER (`done_when`), and \
             what the run may ANSWER if the peer interrupts it with a question of its own. ⚠ The \
             agent's `done_when` is the one argument of the lot that is a BARE word. ⚠ Eleven are \
             the READINESS BARRIER on the THREE plugins that inject, each carrying `ready_when` \
             AND its two nested fields: a marker alone could not say whether text already on the \
             screen is evidence, so the value became an object",
        );
    }

    /// ⚠⚠ **THE NESTED GUARDRAILS CAN BE OFFERED ONE FLAG AT A TIME** — the property both new mouths
    /// rest on, driven over the declarations rather than assumed by the code that flattens them.
    ///
    /// A `max_iterations` that collided with a top-level argument would make `--max-iterations`
    /// mean two things, and the mouth would pick one silently. This is the only place that can say
    /// it does not, because the collision is a property of the TABLE and no call exhibits it.
    #[test]
    fn the_plugin_hosts_nested_arguments_flatten_without_collision() {
        assert_eq!(
            sprag_conformance::a_flattened_nested_argument_collides_with_nothing(
                crate::wire::PLUGINS_GRAMMAR
            )
            .count_or_panic(),
            26,
            "one per FLATTENED nested field of every form: THREE guardrail fields on each of the \
             SIX run forms, since a run is bounded in steps, in spend and in time, PLUS the \
             readiness barrier's `match` and `marker` on each of the four that inject. \
             ⚠⚠ THE FIVE NEWEST ARE THE `ai_loop` FORM'S: a loop injects, so it takes the barrier \
             every injecting form takes, and it spends BYTES — the prompts it types — so its \
             guardrail object is the byte-relay one. What it does NOT take is a cost bound on the \
             agent's own spend, which this daemon neither bills nor can count; that budget is \
             `max_turns`, and it is in the brief rather than in the guardrails. \
             ⚠⚠ THE CONSENT'S `asked`/`answer` ARE NOT COUNTED, and the drop of eight is R370's \
             design rather than a lost check: `may_answer` is a LIST of clauses now, and a list is \
             the one nested shape that cannot be flattened — N loose `asked`s beside N loose \
             `answer`s cannot say which pairs with which — so both flattening mouths offer it \
             whole. Its fields are never flags, so there is nothing for them to collide with. \
             ⚠ What DOES still run is the mirror: `may_answer` is a top-level flag now, and a \
             field of another nest sharing that name is caught here",
        );
    }

    /// ⚠⚠ **A RUN CAN ONLY BE OPENED BY A PANE THIS DAEMON HOLDS** — the arm the type gate cannot
    /// reach, and the one that keeps the provenance prunable.
    ///
    /// `a_declared_argument_is_one_the_plugin_host_reads` drives `opened_by` at the wrong TYPE and
    /// gets `TypeMismatch`; a well-formed number naming a pane that does not exist is a different
    /// answer and a different branch. Without it a caller with a stale `SPRAG_PANE` — a process
    /// that outlived its own pane — would stamp a run with a provenance nothing can ever resolve,
    /// and the agent-facing mouth would filter on a pane number that means nothing.
    ///
    /// The multiplexer states this rule for a pane's own `opened_by`; this is the same rule one
    /// level up, and it is asserted rather than inherited because they are two parsers.
    #[test]
    fn a_run_opened_by_a_pane_this_daemon_does_not_hold_is_refused() {
        let (workspace, pane) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
        );
        let mut ask = |opener: u64| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    RUN_OPENED_BY_KEY: opener,
                })),
            )
        };
        let refused = ask(9999).expect_err("a pane nobody holds cannot have opened a run");
        assert!(
            format!("{refused:?}").contains("no pane 9999"),
            "it is a well-formed request this host will not honour, and it says which pane: \
             {refused:?}",
        );

        // THE CONTROL: the same call naming a pane that IS here starts a run, so the refusal is
        // about the opener and not about the shape of the request.
        assert!(
            ask(pane.0).is_ok(),
            "a real pane may open a run, or the refusal above is about something else",
        );
    }

    /// ⚠⚠⚠ **AN ARGUMENT THIS SURFACE DOES NOT DECLARE IS SWALLOWED, NOT REFUSED** — measured,
    /// because it is what decides whether an ADDED argument earns a protocol number.
    ///
    /// The rule this project reasons from is that an addition is additive when an older daemon
    /// **refuses it loudly by name**: the caller learns it is talking to a stale peer, and no
    /// silent difference of behaviour survives. R363 measured exactly that for an added ACTION —
    /// an unknown verb comes back `UnknownPath`, which every mouth renders as skew.
    ///
    /// An added ARGUMENT is the opposite, and this is the gate that says so rather than a comment
    /// asserting it. The plugin host reads the keys it knows and walks past the rest, so a request
    /// carrying a key an older daemon has never heard of is ACCEPTED, the run starts, and it
    /// converges — under the behaviour the key was sent to change. That is version 17's failure and
    /// version 23's (`shows_prompt`): *the request is accepted, the run converges, and the answer
    /// is byte-identical either way.*
    ///
    /// ⚠ So every argument added to this surface owes a `WIRE_PROTOCOL` bump, and this gate is the
    /// evidence for the next person who has to decide.
    #[test]
    fn an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused() {
        let (workspace, pane) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
        );
        let accepted = external.invoke(
            RUN_ACTION,
            IntrospectValue::Json(json!({
                "plugin": "orchestrator",
                "pane": pane.0,
                "stimulus": "x",
                // A key no version of this surface has ever declared, standing in for one a FUTURE
                // client sends to a daemon that predates it.
                "a_key_from_a_later_protocol": "surprise",
            })),
        );
        assert!(
            accepted.is_ok(),
            "⚠⚠⚠ the request carrying an unknown key was ACCEPTED. A client that sent it to buy \
             different behaviour got the old behaviour and a successful answer, which is why an \
             added ARGUMENT cannot be additive the way an added ADDRESS or ACTION is: {accepted:?}",
        );
    }

    /// ⚠ **NO VERB OF THIS SURFACE TAKES NOTHING, ASSERTED RATHER THAN ASSUMED** — the tripwire that
    /// makes `a_nullary_form_is_a_verb_that_needs_nothing` start holding it the day one does.
    ///
    /// The claim exists because the GUI's five nullary verbs needed it, and R353's `FormKind` doc had
    /// said sprag had none of them. A number here is what keeps that from being a statement about the
    /// surfaces somebody happened to be looking at.
    #[test]
    fn no_verb_of_this_surface_is_nullary_yet() {
        let (workspace, _first) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                crate::wire::PLUGINS_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            0,
            "every verb this surface serves takes arguments, so the claim drives nothing — and the \
             number is what says so",
        );
    }

    /// A live plugin host over one pane, and its registry — the fixture the two duration gates
    /// share. The registry is handed back because a run's ending is read off it.
    fn host_with_a_pane() -> (PluginsExternal, Arc<Mutex<RunRegistry>>, PaneId) {
        let (workspace, pane) = pane_painting("");
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external =
            PluginsExternal::new(workspace, Arc::clone(&registry), None, None, None, None);
        (external, registry, pane)
    }

    /// Poll the registry until run `id` has left `running`, and answer its rendered JSON.
    ///
    /// Bounded well above the ceiling under test, so a run that IGNORED its deadline fails here
    /// with a timeout rather than hanging the suite.
    fn ended(registry: &Arc<Mutex<RunRegistry>>, id: u64, within: Duration) -> Value {
        let start = Instant::now();
        loop {
            let entry = {
                let mut held = lock(registry);
                held.sweep();
                held.snapshot()
                    .iter()
                    .find(|run| run.id.0 == id)
                    // The seat as the record itself names it: this helper watches runs THIS
                    // registry issued, so there is no inherited conversation to re-derive from.
                    .map(|run| run_to_json(run, run.opened_by))
            };
            if let Some(entry) = &entry
                && entry["state"]["status"] != json!("running")
            {
                return entry.clone();
            }
            assert!(
                start.elapsed() < within,
                "run {id} was still running after {:?}: {entry:?}",
                start.elapsed(),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// ⚠⚠ **AN EXPLICIT `null` IS AN OMISSION, NOT A MALFORMED VALUE** — the arm the conformance
    /// walk cannot reach, and which was asserted nowhere.
    ///
    /// That walk drives every declared argument at the WRONG TYPE to prove the parser refuses it,
    /// so it reaches [`InvokeError::TypeMismatch`] for a string where an int belongs. `null` is the
    /// one value that must NOT be refused: a client serialising an absent optional from a language
    /// where absence IS `null` — which is most of them — sends it on every call, and a daemon that
    /// answered `TypeMismatch` would reject well-formed runs from an entire class of client.
    ///
    /// ⚠ Both spellings, in one call: the two `*_ms` bounds and the barrier itself. `ready_when`
    /// carries the rule too, and it is the one that matters most — a nested UNIT read as malformed
    /// rather than absent is a run refused for declining an optional feature.
    #[test]
    fn an_explicitly_null_optional_reads_as_absent_rather_than_malformed() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": null,
                    "ready_when": null,
                    "ready_timeout_ms": null,
                    "guardrails": { "max_iterations": 1, "max_seconds": 5 },
                })),
            )
            .expect(
                "an optional spelled `null` is one the caller declined — refusing it would reject \
                 every client whose language serialises absence that way",
            );
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        // ⚠ AND IT REALLY RAN. A parse that accepted `null` and then quietly built a different spec
        // would pass the line above; the run has to reach an ending of its own.
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        assert!(
            entry["state"]["outcome"]["state"].is_string(),
            "the run built from the declined optionals ran to an ending of its own: {entry:?}",
        );
    }

    /// ⚠⚠ **THE `failure` A CLIENT READS IS A SENTENCE ABOUT THE PANE, NOT A RUST VARIANT** — the
    /// wire half, and the half that decides whether the fix R358 made is worth anything.
    ///
    /// This key was `format!("{e:?}")`, so a failed run published `Write("Broken pipe (os error
    /// 32)")` to an agent that has no way to look up what `Write` is. The remedy — a `Display`
    /// impl, published with `ToString::to_string` — had a gate nowhere, so reverting one call
    /// would have broken nothing and the leak would have come straight back.
    ///
    /// Driven through the readiness failure because it is the one a caller can provoke on purpose:
    /// a marker the pane never prints, with `ready_timeout_ms` short enough that the RUN's clock is
    /// provably not what ended it. Three claims: the run FAILED, the text names the marker, and it
    /// does not read as Rust.
    #[test]
    fn a_failed_run_publishes_a_sentence_about_the_pane_rather_than_a_rust_variant() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "ready_when": {
                        "match": "prints",
                        "marker": "A MARKER THIS PANE NEVER PRINTS",
                    },
                    "ready_timeout_ms": 200,
                    // Far above the readiness bound, so neither ceiling can be what ends this.
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 60 },
                })),
            )
            .expect("a run that names a readiness barrier is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let outcome = &entry["state"]["outcome"];

        assert_eq!(
            outcome["state"],
            json!("failed"),
            "a pane that never becomes ready FAILS the run — it is not a ceiling the run reached, \
             and a client that read `exhausted` here would go looking for a budget it never hit: \
             {entry:?}",
        );
        let said = outcome["failure"]
            .as_str()
            .unwrap_or_else(|| panic!("a failed run publishes its cause as text: {entry:?}"));
        assert!(
            said.contains("A MARKER THIS PANE NEVER PRINTS"),
            "and the text names what the run waited for, which is the only thing that tells the \
             caller WHICH marker they got wrong: {said:?}",
        );
        assert!(
            said.contains(' ') && said.starts_with(char::is_lowercase),
            "it has to read as prose to the agent that receives it, not as a Rust variant and its \
             debug payload: {said:?}",
        );
        assert!(
            !said.contains("NeverReady"),
            "the variant name is the leak itself: {said:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE COMMONEST READINESS MISTAKE IS NAMED ON THE WIRE: THE MARKER WAS ALREADY
    /// THERE.**
    ///
    /// The sibling of the gate above, and the one that matters to a caller who did nothing wrong on
    /// purpose. `prints` means *more occurrences than when this run started watching*, so a pane
    /// that announced itself on the way up can never satisfy it — and **opening a pane and asking
    /// for a run are two separate calls**, which is the normal order and the whole window.
    ///
    /// What came back named the JOB (*"its terminal belonged to `cat`"*): true, about a question
    /// the caller had not asked, and silent on the one fact that corrects the call.
    ///
    /// ⚠⚠⚠ **AND IT IS DRIVEN THROUGH THE WIRE'S OWN DOOR RATHER THAN THE BARRIER'S.** The plugin
    /// crate gates the sentence where it is built; this asks whether it SURVIVES to the `failure`
    /// key a client reads, which is a different question and the one R373 paid for learning to ask
    /// separately.
    #[test]
    fn a_readiness_marker_the_pane_had_already_printed_is_named_as_such_on_the_wire() {
        let (workspace, pane) = pane_painting("BANNER\\r\\n");
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
        );
        // ⚠ THE WINDOW EVERY REAL CALLER HAS, OPENED ON PURPOSE. Waiting for the announcement here
        // is what makes this deterministic rather than a race the fast machine happens to win.
        wait_for_screen(
            &WorkspacePaneAccess::new(Arc::clone(&workspace)),
            pane,
            "BANNER",
        );
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "ready_when": { "match": "prints", "marker": "BANNER" },
                    "ready_timeout_ms": 300,
                    // Both ceilings out of reach, so the readiness bound is provably what ended it.
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 60 },
                })),
            )
            .expect("a run that names a readiness barrier is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let said = entry["state"]["outcome"]["failure"]
            .as_str()
            .unwrap_or_else(|| panic!("a failed run publishes its cause as text: {entry:?}"))
            .to_string();
        assert!(
            said.contains("already on its screen"),
            "⚠⚠⚠ the client must be told the marker IS THERE. Without it they are told what owns \
             the terminal — true, and about a question they did not ask — and the fact that \
             corrects their call never leaves the daemon: {said:?}",
        );
        assert!(
            said.contains("\"shows\""),
            "⚠⚠ and the question that WOULD have read it, in the same wire word they would have to \
             send: {said:?}",
        );
    }

    /// ⚠⚠ **A RUN ASKED TO STOP AFTER A SECOND STOPS AFTER A SECOND** — the wire half of the
    /// duration ceiling, end to end through the verb a client actually calls.
    ///
    /// The iteration ceiling is put out of reach (a hundred thousand steps this pane will never
    /// take) so that the ONLY bound that can end this run is the clock. Before the ceiling existed
    /// the same call was answered `Ok` and bounded by iterations instead — which is the exact
    /// failure this gate is shaped around: not a refusal, an ANSWER OF SUCCESS over a bound nobody
    /// applied.
    ///
    /// ⚠ The `ceiling` key is the second half and not a decoration. A run that stopped at a second
    /// and reported only `exhausted` would be indistinguishable, to every reader on this wire, from
    /// one that ran out of turns.
    #[test]
    fn a_run_asked_to_stop_after_a_second_stops_at_the_clock_and_says_so() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 1 },
                })),
            )
            .expect("a run bounded in time is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        let took = Instant::now();
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let outcome = &entry["state"]["outcome"];

        assert_eq!(
            outcome["state"],
            json!("exhausted"),
            "a run out of time is exhausted by a guardrail, not converged or failed: {entry:?}",
        );
        assert_eq!(
            outcome[RUN_CEILING_KEY],
            json!("duration"),
            "and the guardrail it names is the CLOCK — the iteration ceiling was a hundred \
             thousand and this pane never took a hundred thousand steps: {entry:?}",
        );
        assert!(
            took.elapsed() < Duration::from_secs(10),
            "it must stop near the second it was given, not at some other bound: {:?}",
            took.elapsed(),
        );
        // ⚠⚠ AND WHAT BECAME OF THE WORK reaches the wire beside it. A run out of time ends while a
        // step may still be blocked on the peer it set going, so `exhausted — duration` alone is
        // consistent with the work having stopped AND with it running on; a caller cannot act on
        // that. This key is the difference, and the ORCHESTRATOR names its pane
        // (`Plugin::driving`), so a run against one must carry it.
        let stopped = outcome[RUN_STOPPED_KEY]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stopped.is_empty(),
            "a run cut short must say what became of its work, or `exhausted` is half an answer: \
             {entry:?}",
        );
        assert!(
            stopped.contains(' ') && stopped.starts_with(char::is_lowercase),
            "and it reads as prose to the agent that receives it, not as a Rust variant: \
             {stopped:?}",
        );
        assert!(
            !stopped.contains("Stopped") && !stopped.contains("Signalled"),
            "the variant name is the leak itself: {stopped:?}",
        );

        // ⚠ AND THE COST CEILING'S OWN WORD, driven to the wire rather than asserted at the type.
        // `iterations` reaches it through both mouths' end-to-end gates and `duration` through the
        // block above; without this the third word would be the one no test ever spelled — and a
        // ceiling that reaches an agent under the wrong name is worse than one that reaches it
        // under none.
        let spent = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                    "guardrails": { "max_iterations": 100_000, "max_bytes": 1 },
                })),
            )
            .expect("a run bounded in bytes is a well-formed run");
        let IntrospectValue::Int(spent) = spent else {
            panic!("a run answers its id: {spent:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(spent).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        assert_eq!(
            entry["state"]["outcome"][RUN_CEILING_KEY],
            json!("cost"),
            "the SPEND ceiling names itself `cost` — the concept, not `max_bytes` the knob, \
             because the same ceiling is set by `max_tokens` on a run that spends tokens and one \
             answer cannot be two argument names: {entry:?}",
        );
    }

    /// ⚠⚠ **A BOUND THIS DAEMON DOES NOT KNOW IS REFUSED, WHERE EVERY OTHER UNKNOWN KEY ON THIS
    /// WIRE IS IGNORED** — and the asymmetry is the claim, so both halves are driven here.
    ///
    /// Ignoring an ordinary argument makes a verb do LESS than it was asked, and the caller can see
    /// that in the result. Ignoring a bound makes the run do MORE — without limit — and answers
    /// success. `guardrails: {"max_secnods": 5}` was a run with no time ceiling, no way to find
    /// out, and a typo for a cause.
    ///
    /// ⚠ THE CONTROL is the same call with the key spelled right: it must be ACCEPTED. Without it
    /// this gate would also pass over a parser that refused every guardrail object there is.
    /// ⚠⚠ **EVERY DECLARED GUARDRAIL IS ONE THE PARSER ACTUALLY READS** — the direction the gate
    /// beside this one cannot see.
    ///
    /// [`parse_guardrails`] refuses a key the publication does not name, so a bound this daemon
    /// cannot honour is never silently ignored. The MIRROR of that was uncaught: a field ADDED to
    /// `GUARDRAILS_BYTES`/`GUARDRAILS_TOKENS` and never wired into the parser is ACCEPTED by that
    /// same refusal loop — it is declared, after all — and then read by nobody. The caller is told
    /// about a bound, sends it, gets a success, and has no bound. **That is the exact failure the
    /// refusal above exists to prevent, arriving through the other door.**
    ///
    /// ⚠ THE PROBE IS A WRONG TYPE, because it is what separates the two. Every honoured field is
    /// type-checked (`as_u64().ok_or(TypeMismatch)`), so a string where an int is declared must be
    /// REFUSED — and a declared field nobody reads cannot refuse anything. Sending a well-formed
    /// value would be accepted either way and would measure nothing.
    ///
    /// ⚠ Derived from [`PluginGrammar::guardrail_fields`], never from a list here: a fourth
    /// guardrail is covered the day it is declared, which is the day it can go unread.
    #[test]
    fn every_declared_guardrail_is_one_the_parser_actually_reads() {
        let (mut external, _registry, pane) = host_with_a_pane();
        let mut ask = |guardrails: Value| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "guardrails": guardrails,
                })),
            )
        };

        let declared =
            crate::wire::PluginGrammar::guardrail_fields(sprag_plugin::Cost::Bytes(0).unit());
        assert!(
            !declared.is_empty(),
            "the walk must have something to drive, or it reports a clean surface having asked \
             nothing",
        );
        for field in declared {
            let said = format!("{:?}", ask(json!({ field.name: "not a number" })));
            assert!(
                said.contains("Err"),
                "`{}` is DECLARED as a guardrail and the parser does not read it: a string where \
                 an int is published was accepted, so a caller sending this bound gets a success \
                 and no bound — {said}",
                field.name,
            );
        }
    }

    #[test]
    fn a_guardrail_this_daemon_does_not_know_is_refused_rather_than_ignored() {
        let (mut external, _registry, pane) = host_with_a_pane();
        let mut ask = |guardrails: Value| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "guardrails": guardrails,
                })),
            )
        };

        let refused = ask(json!({ "max_secnods": 5 }))
            .expect_err("a bound this daemon cannot honour must not be answered with success");
        let said = format!("{refused:?}");
        assert!(
            said.contains("max_secnods") && said.contains("max_seconds"),
            "the refusal names the key it did not know AND what it takes instead, or a caller \
             cannot fix a typo from it: {said}",
        );

        // THE CONTROL — the same shape, spelled as the grammar publishes it.
        assert!(
            ask(json!({ "max_seconds": 5 })).is_ok(),
            "the declared spelling must be accepted, or the refusal above is about guardrails in \
             general rather than about an unknown one",
        );

        // ⚠ AND THE REFUSAL IS PER UNIT, because the published forms are: a byte-relay plugin is
        // not offered `max_tokens`, so naming one is naming a bound that cannot guard this run.
        let wrong_unit = ask(json!({ "max_tokens": 5 }))
            .expect_err("a token bound is not a guardrail of a run that spends bytes");
        assert!(
            format!("{wrong_unit:?}").contains("bytes"),
            "and it says which unit this run spends: {wrong_unit:?}",
        );
    }
}
