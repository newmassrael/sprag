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

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError, SchemaField,
};
use serde_json::{Map, Value, json};
use sprag_plugin::{
    Agent, AgentSpec, Ceiling, Cost, Dialogue, DialogueSpec, Driver, Guardrails, OrchestrationSpec,
    Orchestrator, Outcome, OutcomeState, Pipe, PipeSpec, Plugin, ReadyWhen, ReplyFormat,
    RunContext, WorkspacePaneAccess,
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

/// The answer key naming a run.
const RUN_ID_KEY: &str = "id";
/// The answer key carrying the pane whose occupant asked for a run — absent for a run nobody
/// claims, on [`sprag_terminal::Pane::opened_by`]'s terms.
const RUN_OPENED_BY_KEY: &str = "opened_by";
/// The answer key naming WHICH GUARDRAIL exhausted a run — absent unless one did.
///
/// Its vocabulary is [`sprag_plugin::Ceiling`]'s own words, so the host never spells a variant and
/// a fourth ceiling reaches the wire by being added to that type.
pub const RUN_CEILING_KEY: &str = "ceiling";
/// The answer key carrying WHAT EACH STEP DID — the last [`sprag_plugin::JOURNAL_LIMIT`] of them.
///
/// A run reported its total and its terminal state and nothing about the steps between, so a loop
/// that failed to converge could not be diagnosed at all. ⚠ Compare its length against
/// `iterations` to tell a truncated journal from a complete one.
pub const RUN_JOURNAL_KEY: &str = "journal";

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
        let id = self.spawn_run(label, opened_by, plugin, guardrails);
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
                let ready_within = opt_millis(map, "ready_timeout_ms")?;
                let label = format!("orchestrator pane={}", pane.0);
                let spec = OrchestrationSpec {
                    stimulus,
                    sentinel,
                    ready_when,
                    ready_within,
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
                    ready_within: opt_millis(map, "ready_timeout_ms")?,
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
                    spec.eof = map["eof"].as_bool().ok_or(InvokeError::TypeMismatch)?;
                }
                if let Some(timeout) = opt_millis(map, "timeout_ms")? {
                    spec.timeout = timeout;
                }
                spec.ready_when = opt_ready_when(map)?;
                spec.ready_within = opt_millis(map, "ready_timeout_ms")?;
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
        mut plugin: PluginKind,
        guardrails: Guardrails,
    ) -> RunId {
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        // The cancel flag is shared two ways: the run's RunContext reads it, and
        // the registry holds a clone so a `cancel`/shutdown can set it.
        let cancel = Arc::new(AtomicBool::new(false));
        let run_ctx = RunContext::new(Arc::clone(&cancel));
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
            *lock(&worker_state) = RunState::Done { outcome, output };
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
            state,
            handle,
            progress,
            cancel,
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
fn agent_state_source(
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
                asking: (state == sprag_detect::AgentState::Blocked)
                    .then(|| sprag_detect::question(screen, sprag_detect::DIALOG_WINDOW))
                    .flatten(),
                state,
                agent: facts.agent,
                authority,
                seq: facts.seq,
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

impl ExternalIntrospect for PluginsExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::action(RUN_ACTION, "action"),
                    SchemaField::action(CANCEL_ACTION, "action"),
                    SchemaField::new(RUNS_SLOT, "list"),
                    SchemaField::new(PLUGINS_SLOT, "list"),
                    SchemaField::new(GUARDRAIL_DEFAULTS_SLOT, "object"),
                    SchemaField::new(crate::wire::ACTION_GRAMMAR_SLOT, "object"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            RUNS_SLOT => {
                let mut registry = lock(&self.runs);
                registry.sweep(); // reap finished threads before reporting
                let entries = registry.snapshot().iter().map(run_to_json).collect();
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
}

impl PluginKind {
    fn as_plugin(&mut self) -> &mut dyn Plugin {
        match self {
            PluginKind::Orchestrator(orchestrator) => orchestrator,
            PluginKind::Pipe(pipe) => pipe,
            PluginKind::Agent(agent) => agent,
            PluginKind::Dialogue(dialogue) => dialogue.as_mut(),
        }
    }

    /// This plugin's default cost ceiling, in its natural unit: the byte-relay
    /// plugins spend injected bytes; the dialogue spends LLM tokens. The unit
    /// also sizes a bare `max_cost` from the wire.
    fn default_cost(&self) -> Cost {
        match self {
            PluginKind::Orchestrator(_) | PluginKind::Pipe(_) | PluginKind::Agent(_) => {
                Cost::Bytes(DEFAULT_MAX_BYTES)
            }
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
fn run_to_json(run: &RunSummary) -> Value {
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
    if let Some(opener) = run.opened_by {
        entry[RUN_OPENED_BY_KEY] = json!(opener);
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
    match outcome.state {
        OutcomeState::Converged => "converged",
        // ⚠ THE STATE WORD IS UNCHANGED by the ceiling, deliberately: folding the ceiling into the
        // word (`exhausted_duration`) would change the value space of a key old readers decode
        // whole, which is a wire break no address or shape pin can see (R342).
        OutcomeState::Exhausted(_) => "exhausted",
        OutcomeState::Failed => "failed",
        OutcomeState::Cancelled => "cancelled",
    }
}

/// Which ceiling stopped it, or [`None`] when no ceiling did — [`outcome_word`]'s companion.
#[must_use]
pub fn outcome_ceiling(outcome: &Outcome) -> Option<&'static str> {
    match outcome.state {
        OutcomeState::Exhausted(ceiling) => Some(ceiling.wire_str()),
        _ => None,
    }
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
        Some("exhausted") => OutcomeState::Exhausted(match ceiling {
            Some(word) if word == Ceiling::Cost.wire_str() => Ceiling::Cost,
            Some(word) if word == Ceiling::Duration.wire_str() => Ceiling::Duration,
            _ => Ceiling::Iterations,
        }),
        _ => OutcomeState::Failed,
    }
}

fn outcome_to_json(outcome: &Outcome) -> Value {
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
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_detect::{AgentState, Report, Ruleset, built_ins};
    use sprag_plugin::PaneAccess;
    use sprag_terminal::CommandBuilder;
    use std::time::Instant;

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
            20,
            "one call per published word: the ONE plugin word that selects each of the four forms, \
             the two reply formats on each of a dialogue's two endpoints, and the readiness \
             barrier's FOUR `match` words on each of the three plugins that inject — the last two \
             being `runs` and `settles`, which ask the pane's terminal and its supervisor rather \
             than its screen",
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
            9,
            "one probe per open string argument of every form: an orchestrator's stimulus, \
             sentinel and ready_when, a PIPE's ready_when, an agent's prompt and ready_when, and \
             a dialogue's seed and two labels",
        );
    }

    /// ⚠⚠ **A DECLARED ARGUMENT IS ONE THIS SURFACE ACTUALLY READS** — the gate that lets this table
    /// be hand-written, over a verb whose four forms were transcribed from a parser by eye.
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
            36,
            "one probe per OPTIONAL declared argument of every form, nesting included — required \
             ones are deliberately not driven, because `null` for something the grammar demands is \
             malformed rather than declined",
        );
    }

    #[test]
    fn a_declared_argument_is_one_the_plugin_host_reads() {
        assert_eq!(
            grammar_gate(sprag_conformance::a_declared_argument_is_one_the_daemon_reads)
                .count_or_panic(),
            56,
            "one probe per declared argument of every FORM, nesting included: THIRTEEN for an \
             orchestrator, TWELVE for a pipe, FOURTEEN for an agent, sixteen for a dialogue, and \
             one to cancel. ⚠ Eleven are the READINESS BARRIER on the THREE plugins that inject, \
             each carrying `ready_when` AND its two nested fields: a marker alone could not say \
             whether text already on the screen is evidence, so the value became an object",
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
            18,
            "one per nested field of every form: THREE guardrail fields on each of the four run \
             forms, since a run is bounded in steps, in spend and in time, PLUS the readiness \
             barrier's `match` and `marker` on each of the three that inject",
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
                    .map(run_to_json)
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
