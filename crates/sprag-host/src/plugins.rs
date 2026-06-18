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
//! Runs are guardrail-bounded by construction (a `run` omitting guardrails gets
//! [`DEFAULT_GUARDRAILS`], never unbounded — loop safety is first-class). Target
//! panes are validated at submit time, so a typo is a synchronous `Rejected`,
//! not an async `Failed`.

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pinion_core::external::{
    ExternalIntrospect, IntrospectSchema, IntrospectValue, InterveneError, InvokeError,
};
use serde_json::{json, Map, Value};
use sprag_plugin::{
    Agent, AgentSpec, Dialogue, DialogueSpec, Driver, Guardrails, OrchestrationSpec, Orchestrator,
    Outcome, OutcomeState, Pipe, Plugin, ReplyFormat, RunContext, WorkspacePaneAccess,
};
use sprag_terminal::{PaneId, Workspace};

use crate::external::{
    as_object, lock, opt_dim, opt_str, require_pane_id, require_str, rpc_external_impl,
};
use crate::runs::{RunId, RunRegistry, RunState};

const RUN_ACTION: &str = "run";
const CANCEL_ACTION: &str = "cancel";
const RUNS_SLOT: &str = "runs";
const PLUGINS_SLOT: &str = "plugins";

/// The bundled plugins a `run` can name.
const PLUGINS: &[&str] = &["orchestrator", "pipe", "agent", "dialogue"];

/// Conservative guardrails for a `run` that omits them — never unbounded
/// (the README makes loop safety first-class).
const DEFAULT_GUARDRAILS: Guardrails = Guardrails {
    max_iterations: 100,
    max_cost: 64 * 1024,
};

/// The plugin host as a pinion `External`: starts background plugin runs over
/// the shared [`Workspace`] and reports their outcomes as scene-as-data.
pub struct PluginsExternal {
    workspace: Arc<Mutex<Workspace>>,
    runs: Arc<Mutex<RunRegistry>>,
}

impl PluginsExternal {
    /// Build the host over the shared workspace + run registry.
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>, runs: Arc<Mutex<RunRegistry>>) -> Self {
        Self { workspace, runs }
    }

    /// `run` action: build the named plugin, validate its target panes, spawn
    /// it on a background thread, and return its run id.
    fn run(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let guardrails = parse_guardrails(map)?;
        let (plugin, label) = self.build_plugin(map)?;
        let id = self.spawn_run(label, plugin, guardrails);
        Ok(IntrospectValue::Int(i64::try_from(id.0).unwrap_or(i64::MAX)))
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
            Err(InvokeError::Rejected)
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
                Ok((PluginKind::Orchestrator(Orchestrator::new(pane, spec)), label))
            }
            "pipe" => {
                let src = require_pane_id(map, "src")?;
                let dst = require_pane_id(map, "dst")?;
                self.require_pane(src)?;
                self.require_pane(dst)?;
                Ok((PluginKind::Pipe(Pipe::new(src, dst)), format!("pipe {}->{}", src.0, dst.0)))
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
                    spec.timeout = Duration::from_millis(v.as_u64().ok_or(InvokeError::TypeMismatch)?);
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
                if let Some(label) = opt_str(map, "label_a")? {
                    spec.label_a = label.to_string();
                }
                if let Some(label) = opt_str(map, "label_b")? {
                    spec.label_b = label.to_string();
                }
                if let Some(format) = parse_reply_format(map, "format_a")? {
                    spec.format_a = format;
                }
                if let Some(format) = parse_reply_format(map, "format_b")? {
                    spec.format_b = format;
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
                    spec.endpoint_a.first().map_or("?", String::as_str),
                    spec.endpoint_b.first().map_or("?", String::as_str),
                );
                Ok((PluginKind::Dialogue(Dialogue::new(spec)), label))
            }
            _ => Err(InvokeError::Rejected), // unknown plugin
        }
    }

    fn require_pane(&self, pane: PaneId) -> Result<(), InvokeError> {
        if lock(&self.workspace).pane(pane).is_some() {
            Ok(())
        } else {
            Err(InvokeError::Rejected)
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
        let access = WorkspacePaneAccess::new(Arc::clone(&self.workspace));
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

impl fmt::Debug for PluginsExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginsExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(PluginsExternal);

impl ExternalIntrospect for PluginsExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(&[
            (RUN_ACTION, "action"),
            (CANCEL_ACTION, "action"),
            (RUNS_SLOT, "list"),
            (PLUGINS_SLOT, "list"),
        ])
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

    fn invoke(&mut self, path: &str, args: IntrospectValue) -> Result<IntrospectValue, InvokeError> {
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
    Dialogue(Dialogue),
}

impl PluginKind {
    fn as_plugin(&mut self) -> &mut dyn Plugin {
        match self {
            PluginKind::Orchestrator(orchestrator) => orchestrator,
            PluginKind::Pipe(pipe) => pipe,
            PluginKind::Agent(agent) => agent,
            PluginKind::Dialogue(dialogue) => dialogue,
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
                Err(InvokeError::Rejected)
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
        Some(_) => Err(InvokeError::Rejected),
    }
}

/// Read the optional `guardrails` sub-object, defaulting omitted fields to
/// [`DEFAULT_GUARDRAILS`].
fn parse_guardrails(map: &Map<String, Value>) -> Result<Guardrails, InvokeError> {
    let Some(value) = map.get("guardrails") else {
        return Ok(DEFAULT_GUARDRAILS);
    };
    let Value::Object(g) = value else {
        return Err(InvokeError::TypeMismatch);
    };
    let max_iterations = match g.get("max_iterations") {
        None => DEFAULT_GUARDRAILS.max_iterations,
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)?,
    };
    let max_cost = match g.get("max_cost") {
        None => DEFAULT_GUARDRAILS.max_cost,
        Some(v) => v.as_u64().ok_or(InvokeError::TypeMismatch)?,
    };
    Ok(Guardrails {
        max_iterations,
        max_cost,
    })
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
    json!({
        "state": state,
        "iterations": outcome.iterations,
        "cost": outcome.cost,
        "failure": outcome.failure.as_ref().map(|e| format!("{e:?}")),
    })
}
