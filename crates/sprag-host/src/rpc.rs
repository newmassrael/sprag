//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire over a line-delimited transport,
//! assembling the live [`Workspace`] panes into a fresh scene for each
//! request. This is the runnable form of the headless data path
//! (DESIGN.md §1/§3): an external AI peer reads the terminals as data and
//! drives input / pane lifecycle, with no GPU and no shell event loop.
//!
//! ## Method boundary (enforced, not incidental)
//!
//! [`handle_request`] gates to an explicit [`SUPPORTED_METHODS`] allowlist
//! and returns a JSON-RPC method-not-found error for everything else.
//!
//! Reads (`scene/snapshot`, `scene/query`) and input (`scene/invoke`)
//! operate on the same per-request pane scene. Input does *not* go through
//! pinion's `scene/key` (which enqueues a `DeferredInput` for an embedder
//! drain a headless host has no equivalent for); it rides the canonical
//! `scene/invoke` action channel against the pane's engine `External`, whose
//! handler encodes the key (sprag-owned, R2.6) and writes to the live PTY
//! (R1.7). The scene is rebuilt and discarded per request, but the mutation
//! target — the PTY — lives in the session behind the External's
//! `SessionHandle`, so the write reaches live state even though the scene
//! does not persist.

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{dispatch, dispatch_parsed, parse_request, DispatchContext, Request};
use sprag_terminal::Workspace;

use crate::external::lock;
use crate::runs::RunRegistry;

/// The long-lived host state threaded through the serve loop: the shared pane
/// workspace, the background plugin-run registry, and pinion's per-session
/// dispatch ledgers. Bundled so the per-request handler signature stays stable
/// as future control surfaces are added.
pub struct HostState {
    workspace: Arc<Mutex<Workspace>>,
    runs: Arc<Mutex<RunRegistry>>,
    previews: PreviewLedger,
    revision: SceneRevision,
}

impl HostState {
    /// Build host state over a shared workspace, with a fresh run registry.
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        Self {
            workspace,
            runs: Arc::new(Mutex::new(RunRegistry::default())),
            previews: PreviewLedger::default(),
            revision: SceneRevision::default(),
        }
    }

    /// The shared pane workspace.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        &self.workspace
    }

    /// The shared background plugin-run registry.
    #[must_use]
    pub fn runs(&self) -> &Arc<Mutex<RunRegistry>> {
        &self.runs
    }
}

/// The methods the headless host answers: pure reads over the pane scene
/// (`scene/snapshot`, `scene/query`) plus the `scene/invoke` input + plugin
/// channels. Anything else gets a JSON-RPC method-not-found error.
pub const SUPPORTED_METHODS: &[&str] = &["scene/snapshot", "scene/query", "scene/invoke"];

/// Answer one JSON-RPC `request_json` against the workspace's current panes,
/// returning the response JSON (`None` for a notification with no reply).
///
/// Assembles a fresh workspace scene (`Container[panes… + control External]`)
/// from the live workspace, then either dispatches an allowlisted method
/// ([`SUPPORTED_METHODS`]: the reads plus `scene/invoke` input + pane
/// lifecycle), rejects a non-allowlisted method with a method-not-found
/// error, or lets `dispatch` produce the canonical parse-error reply for
/// malformed input.
#[must_use]
pub fn handle_request(state: &HostState, request_json: &str) -> Option<String> {
    let mut scene = crate::workspace_scene(&state.workspace, &state.runs);
    let mut ctx = DispatchContext::new(&mut scene, &state.previews, &state.revision);
    match parse_request(request_json) {
        Ok(request) if SUPPORTED_METHODS.contains(&request.method.as_str()) => {
            dispatch_parsed(&mut ctx, request)
        }
        Ok(request) => Some(method_not_supported(&request)),
        // Malformed: let dispatch emit the canonical JSON-RPC parse error.
        Err(_) => dispatch(&mut ctx, request_json),
    }
}

/// Build the JSON-RPC method-not-found (-32601) reply for a well-formed but
/// non-allowlisted request, naming the supported set.
fn method_not_supported(request: &Request) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id,
        "error": {
            "code": -32601,
            "message": format!(
                "sprag-term host: '{}' is unsupported; use scene/snapshot, scene/query, or scene/invoke",
                request.method
            ),
        }
    })
    .to_string()
}

/// Run the request/response loop: read newline-delimited JSON-RPC requests
/// from `input` and write each response (newline-terminated) to `output`,
/// until `input` reaches EOF. Blank lines are skipped.
///
/// # Errors
///
/// Returns an IO error if reading a request line or writing a response
/// fails.
pub fn serve(state: &HostState, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let request = line.trim();
        if request.is_empty() {
            continue;
        }
        if let Some(response) = handle_request(state, request) {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    // Shutdown: cancel in-flight plugin runs first so they abort promptly
    // (a slow AI turn would otherwise block join), then join so their worker
    // threads and child panes reap before serve returns.
    {
        let mut runs = lock(&state.runs);
        runs.cancel_all();
        runs.join_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external::lock;
    use sprag_terminal::{CommandBuilder, PaneId};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        command
    }

    /// Host state with one initial pane running `script`.
    fn host_with(script: &str, cols: u16, rows: u16) -> HostState {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        lock(&workspace)
            .spawn(sh(script), "sh".to_string(), cols, rows)
            .expect("spawn pane");
        HostState::new(workspace)
    }

    /// One request through the dispatch path (no serve loop / shutdown join), so
    /// the `HostState` persists across calls and a background run is not joined
    /// between requests.
    fn serve_one(state: &HostState, request: &str) -> serde_json::Value {
        let response = handle_request(state, request).expect("a response");
        serde_json::from_str(response.trim()).expect("valid json-rpc response")
    }

    /// Block (bounded) until pane 0's child has closed its PTY.
    fn wait_for_pane0_eof(state: &HostState) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let eof = lock(state.workspace())
                .pane(PaneId(0))
                .is_none_or(|p| p.session().is_eof());
            if eof {
                break;
            }
            sleep(Duration::from_millis(20));
        }
    }

    fn invoke_key(state: &HostState, pane: u64, key: &str) {
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/pane_{pane}/sprag_input/external/key","args":{{"key":"{key}"}}}}}}"#
        );
        let value = serve_one(state, &request);
        assert!(value.get("error").is_none(), "invoke error: {value}");
    }

    /// Poll the live snapshot until it contains `needle`.
    fn wait_for_snapshot(state: &HostState, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let snap = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":9,"method":"scene/snapshot","params":{"path":""}}"#,
            );
            if snap["result"].to_string().contains(needle) {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn serve_answers_scene_snapshot_with_live_screen() {
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert_eq!(value["id"], 1);
        assert!(value.get("error").is_none(), "unexpected error: {value}");
        // The grid text nests under workspace -> pane_0 -> TextGrid.
        assert!(
            value["result"].to_string().contains("hi"),
            "expected 'hi' in result, got: {}",
            value["result"]
        );
    }

    #[test]
    fn serve_rejects_scene_key_in_favor_of_scene_invoke() {
        // Input rides scene/invoke against a pane's engine External, not
        // pinion's widget-oriented scene/key — so scene/key stays unsupported.
        let state = host_with("printf hi", 20, 4);
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/key","params":{"key":"a"}}"#,
        );
        assert_eq!(value["id"], 2);
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn serve_injects_key_into_a_pane() {
        let state = host_with("cat", 20, 4);
        invoke_key(&state, 0, "h");
        invoke_key(&state, 0, "i");
        assert!(wait_for_snapshot(&state, "hi"), "injected 'hi' never appeared");
    }

    #[test]
    fn serve_spawns_addresses_and_closes_panes() {
        // Multiplex lifecycle over the wire: spawn a 2nd pane, address it,
        // list panes, close one.
        let state = host_with("cat", 20, 4);
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{"cmd":["cat"],"cols":20,"rows":4}}}"#,
        );
        assert_eq!(spawned["result"].as_i64(), Some(1), "new pane id: {spawned}");

        invoke_key(&state, 1, "Z");
        assert!(wait_for_snapshot(&state, "Z"), "pane 1 never echoed 'Z'");

        let panes = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
        );
        assert_eq!(panes["result"].as_array().map(Vec::len), Some(2));

        let closed = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_mux/external/close","args":{"id":0}}}"#,
        );
        assert!(closed.get("error").is_none(), "close error: {closed}");
        assert_eq!(lock(state.workspace()).panes().len(), 1);
    }

    /// Poll `query("runs")` until run 0 reports `done`, returning its outcome
    /// state (or `None` on timeout).
    fn wait_for_run_done(state: &HostState) -> Option<String> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let runs = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":7,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
            );
            let run = &runs["result"][0];
            if run["state"]["status"] == "done" {
                return run["state"]["outcome"]["state"].as_str().map(str::to_string);
            }
            sleep(Duration::from_millis(20));
        }
        None
    }

    /// Poll `query("runs")` until run 0 reports `done`, returning its full
    /// `state` JSON (outcome + any captured `output`), or `None` on timeout.
    fn wait_for_run0_state(state: &HostState) -> Option<serde_json::Value> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let runs = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":7,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
            );
            let run = &runs["result"][0];
            if run["state"]["status"] == "done" {
                return Some(run["state"].clone());
            }
            sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn runs_an_agent_plugin_to_done_capturing_its_reply() {
        // The agent adapter over a one-shot fake AI (read the prompt until EOF,
        // reply deterministically). The reply is surfaced as the run's `output`.
        let state = host_with("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"agent","pane":0,"prompt":"ping"}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("agent run reached done");
        assert_eq!(run_state["outcome"]["state"], "converged");
        assert!(
            run_state["output"]
                .as_str()
                .is_some_and(|o| o.contains("REPLY[ping]")),
            "expected the captured reply in output, got: {}",
            run_state["output"]
        );
    }

    #[test]
    fn runs_a_dialogue_plugin_to_done_with_a_transcript() {
        // Two count-fake endpoints: each replies with the newline-count of its
        // prompt, which grows as the transcript accumulates — proving the host
        // run passes the WHOLE history each turn. Each turn spawns a transient
        // pane that must be reaped, so only the host's initial pane survives.
        let state = host_with("cat", 40, 6);
        let endpoint = serde_json::json!([
            "/bin/sh",
            "-c",
            "n=$(printf '%s' \"$1\" | wc -l | tr -d ' '); printf 'saw%s\\n' \"$n\"",
            "_"
        ]);
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "scene/invoke",
            "params": {
                "path": "/sprag_plugins/external/run",
                "args": {
                    "plugin": "dialogue",
                    "endpoint_a": endpoint,
                    "endpoint_b": endpoint,
                    "seed": "count upward",
                    "cols": 40, "rows": 6,
                    "guardrails": { "max_iterations": 3, "max_cost": 1048576 }
                }
            }
        })
        .to_string();
        let started = serve_one(&state, &request);
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("dialogue run reached done");
        assert_eq!(run_state["outcome"]["state"], "exhausted");
        // The transcript alternates labels and the reported line-counts strictly
        // increase (the history accumulates each turn) — asserted on the trend,
        // not exact counts, so the prompt format can change freely.
        let output = run_state["output"].as_str().unwrap_or_default();
        assert!(
            output.contains("A: saw") && output.contains("B: saw"),
            "expected an alternating accumulating transcript, got: {output:?}"
        );
        let counts: Vec<u32> = output
            .match_indices("saw")
            .map(|(i, _)| {
                output[i + 3..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .expect("a saw count")
            })
            .collect();
        assert!(
            counts.len() == 3 && counts.windows(2).all(|w| w[0] < w[1]),
            "history must accumulate (strictly increasing): {counts:?}"
        );
        // Only the initial pane remains — every per-turn pane was reaped.
        assert_eq!(lock(state.workspace()).panes().len(), 1, "dialogue leaked a pane");
    }

    #[test]
    fn full_text_query_includes_scrolled_off_lines() {
        // Read-path parity: an external RPC peer reads the same full output
        // (scrollback + visible) the in-process capture path sees, so a scrolled
        // reply is not invisible over the wire.
        let state = host_with("seq 1 30", 20, 4);
        wait_for_pane0_eof(&state);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/full_text"}}"#,
        );
        let text = resp["result"].as_str().unwrap_or_default();
        assert!(text.contains("\n5\n"), "scrolled-off line 5 missing over RPC: {text:?}");
        assert!(text.contains("\n30"), "last line missing over RPC: {text:?}");
    }

    #[test]
    fn lists_the_agent_among_available_plugins() {
        let state = host_with("cat", 20, 4);
        let plugins = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/sprag_plugins/external/plugins"}}"#,
        );
        let names = plugins["result"].as_array().expect("a plugins array");
        assert!(
            names.iter().any(|n| n == "agent"),
            "expected 'agent' in plugins, got: {}",
            plugins["result"]
        );
        assert!(
            names.iter().any(|n| n == "dialogue"),
            "expected 'dialogue' in plugins, got: {}",
            plugins["result"]
        );
    }

    #[test]
    fn runs_an_orchestrator_plugin_in_the_background_to_convergence() {
        // cat echoes the stimulus, so the orchestrator converges on the sentinel.
        let state = host_with("cat", 20, 4);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"ping","sentinel":"ping","guardrails":{"max_iterations":5,"max_cost":4096}}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");
        assert_eq!(wait_for_run_done(&state).as_deref(), Some("converged"));
    }

    #[test]
    fn run_with_an_unknown_pane_is_rejected_synchronously() {
        // Submit-time validation: a missing pane is a synchronous Rejected,
        // not an async Failed the peer has to poll for.
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":99,"stimulus":"x"}}}"#,
        );
        assert!(rejected.get("error").is_some(), "expected a rejection: {rejected}");
    }

    #[test]
    fn a_running_plugin_does_not_block_the_serve_loop() {
        // A `sleep` pane never echoes, so each orchestrator step burns its full
        // observe timeout — the run takes ~1s. Meanwhile an immediate snapshot
        // must still return promptly, proving the run is off the serve path.
        let state = host_with("sleep 5", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":2,"max_cost":1048576}}}}"#,
        );
        let start = Instant::now();
        let snap = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert!(snap.get("error").is_none(), "snapshot error: {snap}");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "snapshot blocked behind the run: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cancels_a_running_plugin_over_rpc() {
        // A sleep pane never echoes, so the orchestrator loops until cancelled.
        let state = host_with("sleep 30", 20, 4);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_cost":1073741824}}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        // An unknown run id is a synchronous rejection.
        let bad = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":999}}}"#,
        );
        assert!(bad.get("error").is_some(), "unknown id should reject: {bad}");

        // Cancel the live run; it then reaches done = cancelled.
        let cancelled = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":0}}}"#,
        );
        assert!(cancelled.get("error").is_none(), "cancel error: {cancelled}");
        assert_eq!(wait_for_run_done(&state).as_deref(), Some("cancelled"));
    }

    #[test]
    fn shutdown_cancels_in_flight_runs_promptly() {
        // The serve-shutdown path: cancel_all() then join_all(). With a sleep
        // pane and no cancel, join would block on the looping orchestrator;
        // cancelling first makes shutdown return promptly with the run reaped.
        let state = host_with("sleep 30", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_cost":1073741824}}}}"#,
        );

        let start = Instant::now();
        {
            let mut runs = lock(state.runs());
            runs.cancel_all();
            runs.join_all();
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown blocked on the in-flight run: {:?}",
            start.elapsed()
        );

        let runs = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
        );
        assert_eq!(runs["result"][0]["state"]["outcome"]["state"], "cancelled");
    }
}
