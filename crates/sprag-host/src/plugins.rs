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
//! iteration ceiling (the liveness floor) plus the plugin's default cost ceiling
//! in its unit, never unbounded — loop safety is first-class. (A print-mode Text
//! dialogue accumulates `Tokens(0)`, so iterations are its sole effective bound.)
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
    Agent, AgentSpec, Cost, Dialogue, DialogueSpec, Driver, Guardrails, OrchestrationSpec,
    Orchestrator, Outcome, OutcomeState, Pipe, Plugin, ReplyFormat, RunContext,
    WorkspacePaneAccess,
};
use sprag_terminal::{PaneId, Workspace};

use crate::external::{
    as_object, lock, opt_dim, opt_str, refused, require_pane_id, require_str, rpc_external_impl,
};
use crate::runs::{RunId, RunRegistry, RunState};

const RUN_ACTION: &str = "run";
const CANCEL_ACTION: &str = "cancel";
const RUNS_SLOT: &str = "runs";
const PLUGINS_SLOT: &str = "plugins";

/// The bundled plugins a `run` can name.
const PLUGINS: &[&str] = &["orchestrator", "pipe", "agent", "dialogue"];

/// The default iteration ceiling for a `run` that omits guardrails — never
/// unbounded (the README makes loop safety first-class), and the floor that
/// bounds every run regardless of its cost unit.
const DEFAULT_MAX_ITERATIONS: u32 = 100;
/// The default cost ceiling for a byte-relay plugin (Orchestrator/Pipe/Agent),
/// in injected PTY bytes.
const DEFAULT_MAX_BYTES: u64 = 64 * 1024;
/// The default cost ceiling for the token-denominated Dialogue plugin, in real
/// input+output tokens (cache tokens are excluded — see `reply::parse_tokens`).
/// A COARSE backstop, not the primary bound: at the default 100-iteration cap
/// (~2k tokens/turn for a real dialogue) the iteration cap bites first, and a
/// print-mode Text dialogue reports `Tokens(0)` so only iterations bound it.
/// This ceiling exists to stop a single pathological high-token turn; tune it to
/// the model's pricing if a dollar-aware bound is ever needed.
const DEFAULT_MAX_TOKENS: u64 = 200_000;

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
    ) -> Self {
        Self {
            workspace,
            runs,
            on_pane_exit,
            on_attention,
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
        let id = self.spawn_run(label, plugin, guardrails);
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
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
        match require_str(map, "plugin")? {
            "orchestrator" => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                let stimulus = require_str(map, "stimulus")?.to_string();
                let sentinel = opt_str(map, "sentinel")?.map(str::to_string);
                let label = format!("orchestrator pane={}", pane.0);
                let spec = OrchestrationSpec { stimulus, sentinel };
                Ok((
                    PluginKind::Orchestrator(Orchestrator::new(pane, spec)),
                    label,
                ))
            }
            "pipe" => {
                let src = require_pane_id(map, "src")?;
                let dst = require_pane_id(map, "dst")?;
                self.require_pane(src)?;
                self.require_pane(dst)?;
                Ok((
                    PluginKind::Pipe(Pipe::new(src, dst)),
                    format!("pipe {}->{}", src.0, dst.0),
                ))
            }
            "agent" => {
                let pane = require_pane_id(map, "pane")?;
                self.require_pane(pane)?;
                let prompt = require_str(map, "prompt")?.to_string();
                let mut spec = AgentSpec::new(prompt);
                if let Some(v) = map.get("eof") {
                    spec.eof = v.as_bool().ok_or(InvokeError::TypeMismatch)?;
                }
                if let Some(v) = map.get("timeout_ms") {
                    spec.timeout =
                        Duration::from_millis(v.as_u64().ok_or(InvokeError::TypeMismatch)?);
                }
                let label = format!("agent pane={}", pane.0);
                Ok((PluginKind::Agent(Agent::new(pane, spec)), label))
            }
            "dialogue" => {
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
                if let Some(v) = map.get("timeout_ms") {
                    spec.timeout =
                        Duration::from_millis(v.as_u64().ok_or(InvokeError::TypeMismatch)?);
                }
                let label = format!(
                    "dialogue {}<->{}",
                    spec.endpoints[0].argv.first().map_or("?", String::as_str),
                    spec.endpoints[1].argv.first().map_or("?", String::as_str),
                );
                Ok((PluginKind::Dialogue(Box::new(Dialogue::new(spec))), label))
            }
            other => Err(refused(format!(
                "this daemon has no plugin called {other:?}"
            ))),
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
    fn spawn_run(&self, label: String, mut plugin: PluginKind, guardrails: Guardrails) -> RunId {
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
        let handle = thread::spawn(move || {
            let outcome = Driver::new(guardrails).run(plugin.as_plugin(), &access, &run_ctx);
            // The worker still owns the plugin after the run, so it can read any
            // content the plugin captured (an AI adapter's reply) for the host.
            let output = plugin.as_plugin().captured();
            *lock(&worker_state) = RunState::Done { outcome, output };
        });
        lock(&self.runs).submit(label, state, handle, cancel)
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
                    SchemaField::new(RUN_ACTION, "action"),
                    SchemaField::new(CANCEL_ACTION, "action"),
                    SchemaField::new(RUNS_SLOT, "list"),
                    SchemaField::new(PLUGINS_SLOT, "list"),
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
            PLUGINS_SLOT => Some(IntrospectValue::Json(json!(PLUGINS))),
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
    match opt_str(map, key)? {
        None => Ok(None),
        Some("text") => Ok(Some(ReplyFormat::Text)),
        Some("claude_json") => Ok(Some(ReplyFormat::ClaudeJson)),
        Some(other) => Err(refused(format!(
            "{key:?} is {other:?}: it must be \"text\" or \"claude_json\""
        ))),
    }
}

/// Read the optional `guardrails` sub-object. `max_iterations` defaults to
/// [`DEFAULT_MAX_ITERATIONS`] (always present — the liveness floor). The cost
/// bound is self-describing: `max_bytes` xor `max_tokens` in the plugin's unit
/// (omitted → the plugin's default ceiling). NB a `Tokens(0)`-only run (a
/// print-mode Text dialogue) accumulates no measured cost, so its cost ceiling
/// never binds and `max_iterations` is its sole effective bound — by design.
fn parse_guardrails(
    map: &Map<String, Value>,
    default_cost: Cost,
) -> Result<Guardrails, InvokeError> {
    let Some(value) = map.get("guardrails") else {
        return Ok(Guardrails {
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_cost: Some(default_cost),
        });
    };
    let Value::Object(g) = value else {
        return Err(InvokeError::TypeMismatch);
    };
    let max_iterations = match g.get("max_iterations") {
        None => DEFAULT_MAX_ITERATIONS,
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)?,
    };
    Ok(Guardrails {
        max_iterations,
        max_cost: parse_max_cost(g, default_cost)?,
    })
}

/// Parse the optional cost bound: `max_bytes` XOR `max_tokens` (a run has ONE
/// cost unit), or the plugin's default when neither is given. The chosen unit
/// must match the plugin's — so a guardrail cannot be misloaded into the wrong
/// currency. Both keys present, a non-integer, or the wrong unit → a synchronous
/// [`InvokeError`] (a misloaded spend guardrail is a submit-time error, never a
/// silently looser-by-a-factor bound).
fn parse_max_cost(g: &Map<String, Value>, default_cost: Cost) -> Result<Option<Cost>, InvokeError> {
    let bound = match (g.get("max_bytes"), g.get("max_tokens")) {
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

/// Render one run's `(id, label, state)` as JSON for `query("runs")`.
fn run_to_json((id, label, state): &(RunId, String, RunState)) -> Value {
    let state_json = match state {
        RunState::Running => json!({ "status": "running" }),
        RunState::Done { outcome, output } => json!({
            "status": "done",
            "outcome": outcome_to_json(outcome),
            "output": output,
        }),
        RunState::Panicked(message) => json!({ "status": "panicked", "error": message }),
    };
    json!({ "id": id.0, "label": label, "state": state_json })
}

/// Render a plugin [`Outcome`] as JSON (serialization is a host concern, so the
/// pinion-free substrate stays serde-free).
fn outcome_to_json(outcome: &Outcome) -> Value {
    let state = match outcome.state {
        OutcomeState::Converged => "converged",
        OutcomeState::Exhausted => "exhausted",
        OutcomeState::Failed => "failed",
        OutcomeState::Cancelled => "cancelled",
    };
    // Cost is self-describing on the wire: the scalar amount plus its unit label
    // (both from `Cost` itself, so the host never names a variant), so a peer
    // reads it without knowing which plugin ran. A `null` unit means no measured
    // step (e.g. cancelled before any step ran).
    let (cost, unit) = outcome
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    json!({
        "state": state,
        "iterations": outcome.iterations,
        "cost": cost,
        "unit": unit,
        "failure": outcome.failure.as_ref().map(|e| format!("{e:?}")),
    })
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
}
