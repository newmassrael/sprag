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
use std::ops::ControlFlow;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{
    DispatchContext, Request, RpcFrame, RpcIngress, RpcReply, WaiterRegistry, dispatch,
    dispatch_parsed, parse_request, try_async_wait_for,
};
use sprag_terminal::Workspace;

use crate::host::Host;
use crate::runs::RunRegistry;

/// The long-lived host state threaded through the serve loop: the booted [`Host`]
/// (the single [`Workspace`] owner), the background plugin-run registry, pinion's
/// per-session dispatch ledgers, and the async `scene/waitFor` waiter registry.
/// Bundled so the per-request handler signature stays stable as future control
/// surfaces are added.
///
/// ## Change-notification (PR-50 §6.3, R115a)
///
/// The [`SceneRevision`] is the ONE scene-version token, shared (`Arc`) with the
/// pane `on_dirty` hooks: a pane's output [`bump`](SceneRevision::bump)s it, which
/// (a) advances the OCC token and (b) fires the wake observer installed in
/// [`new`](Self::new) — `move |n| waiters.wake(n)` — so any parked async
/// `scene/waitFor` reply fires. A wire client thus blocks on `scene/waitFor`
/// until a pane produces output *it did not cause*, instead of busy-polling
/// `scene/snapshot`. The registry parks no version counter of its own; the
/// revision is the single source of truth (pinion's [`WaiterRegistry`] contract).
pub struct HostState {
    host: Host,
    runs: Arc<Mutex<RunRegistry>>,
    previews: PreviewLedger,
    /// The one scene-version token, shared with the pane `on_dirty` bumpers.
    revision: Arc<SceneRevision>,
    /// Parked async `scene/waitFor` replies, woken off `revision`'s observer.
    waiters: Arc<WaiterRegistry>,
}

impl HostState {
    /// Build host state over a booted [`Host`], sharing `revision` — the ONE
    /// scene-version token the pane `on_dirty` hooks bump. Installs the async
    /// `scene/waitFor` wake observer on it (`move |n| waiters.wake(n)`), so a
    /// revision bump (a pane's output) wakes every parked waiter. A fresh run
    /// registry and waiter registry are created here.
    #[must_use]
    pub fn new(host: Host, revision: Arc<SceneRevision>) -> Self {
        let waiters = Arc::new(WaiterRegistry::new());
        // The wake half of the no-lost-wakeup discipline: a revision bump (an OCC
        // mutation OR a pane's external output via on_dirty) fires this, draining
        // and replying to every waiter the new revision surpassed.
        let wake = Arc::clone(&waiters);
        // `set_observer` is install-once (pinion): the FIRST caller wins, later ones
        // no-op and return false. This wake seam is the ONLY thing that fires parked
        // `scene/waitFor` replies, so a silent install-failure would hang every wait
        // forever with no error. Assert we won the install — a fresh revision per
        // HostState makes this always true today; the assert catches a future refactor
        // that reuses an already-observed revision (exactly the silent-failure class
        // the textbook bar wants caught at the wiring point).
        assert!(
            revision.set_observer(move |n| {
                wake.wake(n);
            }),
            "HostState requires a fresh SceneRevision: its wake observer must install \
             (an already-observed revision would leave scene/waitFor parked forever)",
        );
        Self {
            host,
            runs: Arc::new(Mutex::new(RunRegistry::default())),
            previews: PreviewLedger::default(),
            revision,
            waiters,
        }
    }

    /// The CURRENT window's pane workspace (resolved out of the [`Host`]'s
    /// [`SessionRegistry`](sprag_terminal::SessionRegistry)), for the scene-as-data
    /// assembly and the control / plugin externals. A cloned `Arc` (not a borrow), so
    /// each per-request scene assembly reflects the then-current window.
    #[must_use]
    pub fn workspace(&self) -> Arc<Mutex<Workspace>> {
        self.host.workspace()
    }

    /// The shared background plugin-run registry.
    #[must_use]
    pub fn runs(&self) -> &Arc<Mutex<RunRegistry>> {
        &self.runs
    }

    /// The one scene-version token (the async `scene/waitFor` / OCC baseline).
    #[must_use]
    pub fn revision(&self) -> &SceneRevision {
        &self.revision
    }

    /// The async `scene/waitFor` waiter registry.
    #[must_use]
    pub fn waiters(&self) -> &WaiterRegistry {
        &self.waiters
    }
}

/// The pane `on_dirty` hook that bumps `revision` on every batch of PTY output —
/// the change-notification recipe a wire server boots each pane with. Passed as the
/// `on_dirty` of [`Host::spawn`](crate::Host::spawn); the bump advances the OCC
/// token AND wakes any parked async `scene/waitFor` (the observer [`HostState::new`]
/// installs). The single home for this closure so the "a pane's output bumps THIS
/// revision" invariant is not hand-rewritten per boot site (the server binary and
/// the tests share it); a client that spawns a pane against a different revision than
/// the one `HostState` observes would silently never wake, so it lives in one place.
#[must_use]
pub fn bump_on_dirty(revision: &Arc<SceneRevision>) -> Box<dyn Fn() + Send> {
    let revision = Arc::clone(revision);
    Box::new(move || {
        revision.bump();
    })
}

/// The methods the headless host answers: pure reads over the pane scene
/// (`scene/snapshot`, `scene/query`), the `scene/invoke` input + plugin channels,
/// and the async change-notification pair (`scene/revision` reads the current
/// scene-version token; `scene/waitFor {since}` blocks until it advances — the
/// async form is intercepted before dispatch, in the per-frame `dispatch_one`).
/// Anything else gets a JSON-RPC method-not-found error.
pub const SUPPORTED_METHODS: &[&str] = &[
    "scene/snapshot",
    "scene/query",
    "scene/invoke",
    "scene/revision",
    "scene/waitFor",
];

/// Answer one JSON-RPC `request_json` string against the workspace's current
/// panes, returning the response JSON (`None` for a notification with no reply).
///
/// Parses, then delegates a well-formed request to [`handle_parsed`]; a malformed
/// request lets pinion's `dispatch` emit the canonical JSON-RPC parse error. This
/// is the string entry point (the tests + the malformed path use it); the live
/// dispatch owner (`dispatch_one`) has already parsed the frame and calls
/// [`handle_parsed`] directly, so a valid request is parsed exactly once.
#[must_use]
pub fn handle_request(state: &HostState, request_json: &str) -> Option<String> {
    match parse_request(request_json) {
        Ok(request) => handle_parsed(state, request),
        Err(_) => {
            // Malformed: assemble a ctx only for the canonical parse-error reply.
            let mut scene =
                crate::workspace_scene(&state.workspace(), &state.runs, &state.revision);
            let mut ctx = DispatchContext::new(&mut scene, &state.previews, state.revision());
            dispatch(&mut ctx, request_json)
        }
    }
}

/// Answer one already-parsed JSON-RPC `request` against the workspace's current
/// panes — the dispatch core shared by the string entry ([`handle_request`]) and
/// the live dispatch owner (`dispatch_one`, which parses once to intercept async
/// `scene/waitFor` and hands the parsed request straight here). Assembles a fresh
/// workspace scene (`Container[panes… + control External]`), then dispatches an
/// allowlisted method ([`SUPPORTED_METHODS`]) or rejects a non-allowlisted one with
/// a method-not-found error. Only the async `scene/waitFor` form is handled earlier
/// (in `dispatch_one`); the v0 since-less form falls through here to pinion's
/// synchronous handler.
#[must_use]
pub fn handle_parsed(state: &HostState, request: Request) -> Option<String> {
    let mut scene = crate::workspace_scene(&state.workspace(), &state.runs, &state.revision);
    let mut ctx = DispatchContext::new(&mut scene, &state.previews, state.revision());
    if SUPPORTED_METHODS.contains(&request.method.as_str()) {
        dispatch_parsed(&mut ctx, request)
    } else {
        Some(method_not_supported(&request))
    }
}

/// Build the JSON-RPC method-not-found (-32601) reply for a well-formed but
/// non-allowlisted request, naming the supported set. The list is derived from
/// [`SUPPORTED_METHODS`] (not re-typed), so the const stays the single source.
fn method_not_supported(request: &Request) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id,
        "error": {
            "code": -32601,
            "message": format!(
                "sprag-term host: '{}' is unsupported; use one of: {}",
                request.method,
                SUPPORTED_METHODS.join(", "),
            ),
        }
    })
    .to_string()
}

/// An [`RpcIngress`] that funnels frames from any transport into the host's
/// single dispatch owner via a channel.
///
/// The GUI dispatches on pinion-shell's winit event loop; the headless host
/// has no event loop, so it owns one dispatch thread ([`dispatch_frames`]) and
/// every transport -- stdin and the always-on socket -- submits through this
/// into that one owner. Serialising dispatch this way means a concurrent
/// socket connection and a stdin line share one consistent [`HostState`] view,
/// the same single-owner discipline pinion's UI thread gives the GUI.
pub struct FrameIngress {
    tx: Sender<RpcFrame>,
}

impl FrameIngress {
    /// Wrap the sending half of the dispatch owner's channel.
    #[must_use]
    pub fn new(tx: Sender<RpcFrame>) -> Self {
        Self { tx }
    }
}

impl RpcIngress for FrameIngress {
    fn submit(&self, frame: RpcFrame) {
        // A closed channel means the dispatch owner has exited; drop the frame
        // (its reply never fires, so the client's connection simply closes).
        let _ = self.tx.send(frame);
    }
}

/// The single dispatch owner: pull [`RpcFrame`]s and dispatch each against
/// `state` through the same [`handle_request`] core, routing the response back
/// to the frame's originating transport via its reply sink. One thread, so all
/// dispatch is serialised over the shared [`HostState`]. Runs until every
/// sender has dropped (the channel closes) -- for a server with an always-on
/// socket that is process lifetime.
pub fn dispatch_frames(state: &HostState, rx: Receiver<RpcFrame>) {
    for frame in rx {
        dispatch_one(state, frame);
    }
}

/// Dispatch one frame against `state` — the per-frame body of [`dispatch_frames`],
/// split out so the async `scene/waitFor` park/wake path is unit-testable without
/// standing up the channel loop.
///
/// Parses the frame ONCE. An async `scene/waitFor {since}` is intercepted BEFORE
/// the synchronous core: [`try_async_wait_for`] either answers it immediately (the
/// scene already advanced past `since`) or PARKS its reply in the waiter registry —
/// in which case the reply fires LATER, off this dispatch thread, on the scene bump
/// that wakes it ([`HostState`] installed the wake observer). A non-`waitFor` frame
/// (or a since-less v0 `waitFor`) is handed straight to [`handle_parsed`] with the
/// already-parsed request — no re-parse. A malformed frame goes to [`handle_request`]
/// for the canonical parse-error reply. Parking does not build the workspace scene,
/// so a blocked wait costs nothing until a pane actually produces output.
fn dispatch_one(state: &HostState, frame: RpcFrame) {
    let RpcFrame { request, reply } = frame;
    match parse_request(&request) {
        Ok(parsed) => {
            match try_async_wait_for(&parsed, state.revision(), state.waiters(), reply) {
                // Parked (or answered immediately) by the registry — nothing more to do.
                ControlFlow::Break(()) => {}
                // Not an async waitFor: dispatch the ALREADY-parsed request (no re-parse).
                ControlFlow::Continue(reply) => {
                    if let Some(response) = handle_parsed(state, parsed) {
                        reply.send(response);
                    }
                }
            }
        }
        // Malformed: the string entry emits the canonical JSON-RPC parse error.
        Err(_) => {
            if let Some(response) = handle_request(state, &request) {
                reply.send(response);
            }
        }
    }
}

/// Read newline-delimited JSON-RPC requests from `input` and submit each as an
/// [`RpcFrame`] whose reply writes the response (newline-terminated) to stdout,
/// through `tx` into the dispatch owner. Returns when `input` reaches EOF -- the
/// stdin transport ends, but any other transport (the socket) keeps the server
/// alive. Blank lines are skipped.
pub fn stdin_frames(input: impl BufRead, tx: &Sender<RpcFrame>) {
    for line in input.lines() {
        let Ok(text) = line else {
            break;
        };
        let request = text.trim();
        if request.is_empty() {
            continue;
        }
        let reply = RpcReply::new(|response| {
            let mut out = io::stdout().lock();
            if writeln!(out, "{response}").is_ok() {
                let _ = out.flush();
            }
        });
        if tx.send(RpcFrame::new(request.to_owned(), reply)).is_err() {
            break;
        }
    }
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

    /// Host state with one initial pane running `script`, wired the way a wire
    /// server boots: the pane's `on_dirty` bumps the shared [`SceneRevision`], so
    /// its output wakes any parked async `scene/waitFor` (the change-notification
    /// path R115a serves).
    fn host_with(script: &str, cols: u16, rows: u16) -> HostState {
        let revision = Arc::new(SceneRevision::new());
        let host = Host::new((cols, rows));
        // The SAME boot recipe prod uses (sprag-term.rs) — the shared `bump_on_dirty`
        // helper, so the test exercises the real "pane output bumps THIS revision" wire.
        host.spawn(
            sh(script),
            "sh".to_string(),
            cols,
            rows,
            Some(bump_on_dirty(&revision)),
        )
        .expect("spawn pane");
        HostState::new(host, revision)
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
            let eof = lock(&state.workspace())
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
        assert!(
            wait_for_snapshot(&state, "hi"),
            "injected 'hi' never appeared"
        );
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
        assert_eq!(
            spawned["result"].as_i64(),
            Some(1),
            "new pane id: {spawned}"
        );

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
        assert_eq!(lock(&state.workspace()).panes().len(), 1);
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
                return run["state"]["outcome"]["state"]
                    .as_str()
                    .map(str::to_string);
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
                    "guardrails": { "max_iterations": 3, "max_tokens": 1048576 }
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
        assert_eq!(
            lock(&state.workspace()).panes().len(),
            1,
            "dialogue leaked a pane"
        );
    }

    #[test]
    fn runs_a_claude_json_dialogue_with_real_token_cost() {
        // A JSON fake emits a one-line `--output-format json` envelope with
        // fixed usage; `format_*: claude_json` makes the run parse it off the
        // RAW source for the real token cost and the clean reply text — the
        // round's whole point, surfaced over RPC.
        let state = host_with("cat", 40, 6);
        let endpoint = serde_json::json!([
            "/bin/sh",
            "-c",
            "printf '%s' '{\"result\":\"hi there\",\"usage\":{\"input_tokens\":30,\"output_tokens\":20}}'",
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
                    "seed": "go",
                    "format_a": "claude_json",
                    "format_b": "claude_json",
                    "cols": 40, "rows": 6,
                    "guardrails": { "max_iterations": 2, "max_tokens": 1048576 }
                }
            }
        })
        .to_string();
        let started = serve_one(&state, &request);
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("dialogue run reached done");
        assert_eq!(run_state["outcome"]["state"], "exhausted");
        // Two turns × (30 + 20) tokens — the real billed cost over RPC.
        assert_eq!(
            run_state["outcome"]["cost"].as_u64(),
            Some(100),
            "{run_state}"
        );
        assert_eq!(
            run_state["outcome"]["unit"], "tokens",
            "cost unit must be tokens: {run_state}"
        );
        let output = run_state["output"].as_str().unwrap_or_default();
        // The clean `result` is the transcript, not the raw envelope.
        assert!(
            output.contains("hi there"),
            "clean reply missing: {output:?}"
        );
        assert!(
            !output.contains("input_tokens"),
            "raw envelope leaked: {output:?}"
        );
        assert_eq!(
            lock(&state.workspace()).panes().len(),
            1,
            "dialogue leaked a pane"
        );
    }

    #[test]
    fn rejects_an_unknown_reply_format() {
        // A bad `format_*` is a synchronous Rejected (a typo, not an async Fail).
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"dialogue","endpoint_a":["true"],"endpoint_b":["true"],"seed":"x","format_a":"yaml"}}}"#,
        );
        assert!(
            rejected.get("error").is_some(),
            "expected a rejection: {rejected}"
        );
    }

    #[test]
    fn rejects_a_wrong_unit_guardrail() {
        // The dialogue is token-denominated; a `max_bytes` bound is the wrong
        // currency for it and must be a synchronous Rejected — never a silently
        // ignored or mis-unit bound (the guardrail is the spend defence).
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"dialogue","endpoint_a":["true"],"endpoint_b":["true"],"seed":"x","guardrails":{"max_bytes":4096}}}}"#,
        );
        assert!(
            rejected.get("error").is_some(),
            "wrong-unit guardrail must reject: {rejected}"
        );
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
        assert!(
            text.contains("\n5\n"),
            "scrolled-off line 5 missing over RPC: {text:?}"
        );
        assert!(
            text.contains("\n30"),
            "last line missing over RPC: {text:?}"
        );
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
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"ping","sentinel":"ping","guardrails":{"max_iterations":5,"max_bytes":4096}}}}"#,
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
        assert!(
            rejected.get("error").is_some(),
            "expected a rejection: {rejected}"
        );
    }

    #[test]
    fn a_running_plugin_does_not_block_the_serve_loop() {
        // A `sleep` pane never echoes, so each orchestrator step burns its full
        // observe timeout — the run takes ~1s. Meanwhile an immediate snapshot
        // must still return promptly, proving the run is off the serve path.
        let state = host_with("sleep 5", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":2,"max_bytes":1048576}}}}"#,
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
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_bytes":1073741824}}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        // An unknown run id is a synchronous rejection.
        let bad = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":999}}}"#,
        );
        assert!(
            bad.get("error").is_some(),
            "unknown id should reject: {bad}"
        );

        // Cancel the live run; it then reaches done = cancelled.
        let cancelled = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":0}}}"#,
        );
        assert!(
            cancelled.get("error").is_none(),
            "cancel error: {cancelled}"
        );
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
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_bytes":1073741824}}}}"#,
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

    // ─── R115a: async change-notification (scene/revision + scene/waitFor) ───

    /// A recording reply sink: collects whatever the reply is sent, so a test can
    /// assert what (if anything) a parked / immediate `scene/waitFor` fired.
    fn recording_reply(sink: &Arc<Mutex<Vec<String>>>) -> RpcReply {
        let sink = Arc::clone(sink);
        RpcReply::new(move |response| sink.lock().unwrap().push(response))
    }

    /// One frame through the real per-frame dispatch body (`dispatch_one`) with a
    /// recording reply, so the async park/immediate paths are exercised exactly as
    /// the serve loop runs them.
    fn dispatch_recording(state: &HostState, request: &str, sink: &Arc<Mutex<Vec<String>>>) {
        dispatch_one(
            state,
            RpcFrame::new(request.to_owned(), recording_reply(sink)),
        );
    }

    #[test]
    fn scene_revision_reports_the_current_token() {
        // The non-blocking read a wire client bootstraps its waitFor `since` from.
        let state = host_with("cat", 20, 4);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/revision","params":{}}"#,
        );
        assert!(
            resp.get("error").is_none(),
            "scene/revision never errors: {resp}"
        );
        let reported = resp["result"]["revision"]
            .as_u64()
            .expect("a numeric revision");
        assert_eq!(
            reported,
            state.revision().current(),
            "reads the one shared token"
        );
    }

    #[test]
    fn async_wait_for_parks_then_a_scene_bump_wakes_it() {
        // The park/wake integration: dispatch_one routes a `scene/waitFor {since}`
        // into the registry (park at the current revision), and the wake observer
        // HostState installed fires the parked reply on the next bump. Deterministic
        // (a direct bump stands in for a pane's on_dirty), no pane-timing.
        let state = host_with("cat", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            1,
            "parked at the current revision"
        );
        assert!(sink.lock().unwrap().is_empty(), "not answered while parked");

        let new = state.revision().bump();
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "the bump drained the parked waiter"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "the parked reply fired on the bump");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["id"], 5);
        assert_eq!(v["result"]["changed"], true);
        assert_eq!(v["result"]["revision"], new);
    }

    #[test]
    fn async_wait_for_answers_immediately_when_the_scene_already_advanced() {
        // A stale baseline (`since` < current) is answered at dispatch, not parked —
        // so a client that fell behind catches up without blocking.
        let state = host_with("cat", 20, 4);
        state.revision().bump();
        let current = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            r#"{"jsonrpc":"2.0","id":6,"method":"scene/waitFor","params":{"since":0}}"#,
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "a stale baseline does not park"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "answered immediately");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["revision"], current);
    }

    #[test]
    fn a_panes_output_wakes_a_parked_async_wait_for() {
        // The end-to-end wire-client path, headless: block on scene/waitFor, then
        // the pane produces output with NO client input, its on_dirty bumps the
        // shared revision, and the parked reply fires — the change-driven repaint
        // signal a wire GUI long-polls. Bounded poll (no wall-clock assertion).
        let state = host_with("sleep 0.2; printf X", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if !sink.lock().unwrap().is_empty() {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        let responses = sink.lock().unwrap();
        assert_eq!(
            responses.len(),
            1,
            "the pane's own output woke the parked waiter"
        );
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["changed"], true);
        assert!(
            v["result"]["revision"].as_u64().unwrap() > since,
            "woke at a revision past the client's baseline",
        );
    }

    #[test]
    fn a_mux_spawn_wakes_a_parked_async_wait_for() {
        // Round 1 rail, through the REAL dispatch: a pane-SET change (a mux `spawn`,
        // not pane output) wakes a parked waiter — the pane-lifecycle
        // change-notification a mirror long-polls to learn the host gained a pane.
        // Deterministic: the spawn's set-change bump fires the parked reply
        // synchronously on this thread, so no pane-timing / wall-clock is involved.
        let state = host_with("cat", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":8,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            1,
            "parked at the current revision"
        );
        // Spawn a second pane over the real `/sprag_mux` control surface. `cat`
        // produces no output on its own, so the ONLY bump is the spawn's set-change.
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":9,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{"cmd":["cat"]}}}"#,
        );
        assert!(spawned.get("error").is_none(), "spawn error: {spawned}");
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "the spawn's set-change bump drained the parked waiter"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "the parked reply fired on the spawn");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["changed"], true);
        assert!(
            v["result"]["revision"].as_u64().unwrap() > since,
            "woke at a revision past the client's baseline",
        );
    }

    // ─── R115b: pane cells over the wire (the client's per-frame data read) ───

    /// The pane `cells` frame at `offset`, over the full dispatch path.
    fn cells_frame(state: &HostState, offset: u64) -> serde_json::Value {
        serve_one(
            state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/pane_0/sprag_input/external/cells","args":{{"offset":{offset}}}}}}}"#
            ),
        )
    }

    #[test]
    fn pane_cells_action_returns_a_deserializable_grid_frame() {
        // The wire client's per-frame read: the `cells` action returns a JSON frame
        // whose `cells` deserialize back into the EXACT GridBuffer the host projected
        // (PR-49 round-trip), carrying the pane content, plus the scroll facts that
        // ride with it.
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let frame = cells_frame(&state, 0);
        assert!(frame.get("error").is_none(), "cells error: {frame}");
        let result = &frame["result"];
        assert!(
            result["scrollback_len"].is_u64(),
            "scroll facts present: {result}"
        );
        assert_eq!(result["visible_rows"], 4);

        let cells: pinion_core::GridBuffer = serde_json::from_value(result["cells"].clone())
            .expect("GridBuffer deserializes off the wire");
        assert_eq!(
            (cells.cols(), cells.rows()),
            (20, 4),
            "buffer dims match the pane"
        );
        // "hi" is on row 0 — the wire buffer carries the exact projected content.
        assert_eq!(cells.cell(0, 0).map(|c| c.cluster.as_ref()), Some("h"));
        assert_eq!(cells.cell(1, 0).map(|c| c.cluster.as_ref()), Some("i"));
    }

    #[test]
    fn pane_cells_action_honors_the_scrollback_offset() {
        // 40 lines into a 4-row pane: most scroll off into history. The live view
        // (offset 0) and a scrolled-up view differ, proving the offset param reaches
        // the projection over the wire.
        let state = host_with("seq 1 40", 20, 4);
        wait_for_pane0_eof(&state);

        let live = cells_frame(&state, 0);
        assert!(
            live["result"]["scrollback_len"].as_u64().unwrap() > 0,
            "lines scrolled off into history: {live}",
        );
        let live_cells: pinion_core::GridBuffer =
            serde_json::from_value(live["result"]["cells"].clone()).unwrap();

        let scrolled = cells_frame(&state, 20);
        let scrolled_cells: pinion_core::GridBuffer =
            serde_json::from_value(scrolled["result"]["cells"].clone()).unwrap();

        assert_ne!(
            live_cells, scrolled_cells,
            "a scrollback offset changes the projected buffer",
        );
    }
}
