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

/// The methods the headless host answers: pure reads over the pane scene
/// (`scene/snapshot`, `scene/query`) plus the `scene/invoke` input channel.
/// Anything else gets a JSON-RPC method-not-found error.
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
pub fn handle_request(
    workspace: &Arc<Mutex<Workspace>>,
    previews: &PreviewLedger,
    revision: &SceneRevision,
    request_json: &str,
) -> Option<String> {
    let mut scene = crate::workspace_scene(workspace);
    let mut ctx = DispatchContext::new(&mut scene, previews, revision);
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
pub fn serve(
    workspace: &Arc<Mutex<Workspace>>,
    input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let previews = PreviewLedger::default();
    let revision = SceneRevision::default();
    for line in input.lines() {
        let line = line?;
        let request = line.trim();
        if request.is_empty() {
            continue;
        }
        if let Some(response) = handle_request(workspace, &previews, &revision, request) {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::{CommandBuilder, PaneId};
    use std::io::Cursor;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        command
    }

    /// A workspace with one initial pane running `script`.
    fn workspace_with(script: &str, cols: u16, rows: u16) -> Arc<Mutex<Workspace>> {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        workspace
            .lock()
            .unwrap()
            .spawn(sh(script), "sh".to_string(), cols, rows)
            .expect("spawn pane");
        workspace
    }

    /// Block (bounded) until pane 0's child has closed its PTY, so the reader
    /// thread has applied all of its output.
    fn wait_for_pane0_eof(workspace: &Arc<Mutex<Workspace>>) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let eof = workspace
                .lock()
                .unwrap()
                .pane(PaneId(0))
                .is_none_or(|p| p.session().is_eof());
            if eof {
                break;
            }
            sleep(Duration::from_millis(20));
        }
    }

    fn serve_one(workspace: &Arc<Mutex<Workspace>>, request: &str) -> serde_json::Value {
        let input = Cursor::new(format!("{request}\n").into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(workspace, input, &mut output).expect("serve loop");
        let response = String::from_utf8(output).expect("utf8 response");
        serde_json::from_str(response.trim()).expect("valid json-rpc response")
    }

    fn invoke_key(workspace: &Arc<Mutex<Workspace>>, pane: u64, key: &str) {
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/pane_{pane}/sprag_input/external/key","args":{{"key":"{key}"}}}}}}"#
        );
        let value = serve_one(workspace, &request);
        assert!(value.get("error").is_none(), "invoke error: {value}");
    }

    /// Poll the live snapshot until it contains `needle` (the reader thread
    /// applies echoed input asynchronously).
    fn wait_for_snapshot(workspace: &Arc<Mutex<Workspace>>, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let snap = serve_one(
                workspace,
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
        let workspace = workspace_with("printf hi", 20, 4);
        wait_for_pane0_eof(&workspace);
        let value = serve_one(
            &workspace,
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
        let workspace = workspace_with("printf hi", 20, 4);
        let value = serve_one(
            &workspace,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/key","params":{"key":"a"}}"#,
        );
        assert_eq!(value["id"], 2);
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn serve_injects_key_into_a_pane() {
        // End-to-end input into pane 0: scene/invoke encodes "h"/"i" to PTY
        // bytes; the line discipline echoes them onto the pane's grid.
        let workspace = workspace_with("cat", 20, 4);
        invoke_key(&workspace, 0, "h");
        invoke_key(&workspace, 0, "i");
        assert!(wait_for_snapshot(&workspace, "hi"), "injected 'hi' never appeared");
    }

    #[test]
    fn serve_spawns_addresses_and_closes_panes() {
        // Multiplex lifecycle over the wire: spawn a 2nd pane, address it,
        // list panes, close one.
        let workspace = workspace_with("cat", 20, 4);
        let spawned = serve_one(
            &workspace,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{"cmd":["cat"],"cols":20,"rows":4}}}"#,
        );
        assert_eq!(spawned["result"].as_i64(), Some(1), "new pane id: {spawned}");

        // Input addressed to pane 1 echoes onto pane 1's grid.
        invoke_key(&workspace, 1, "Z");
        assert!(wait_for_snapshot(&workspace, "Z"), "pane 1 never echoed 'Z'");

        // The control surface lists both panes.
        let panes = serve_one(
            &workspace,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
        );
        assert_eq!(panes["result"].as_array().map(Vec::len), Some(2));

        // Closing pane 0 leaves one pane.
        let closed = serve_one(
            &workspace,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_mux/external/close","args":{"id":0}}}"#,
        );
        assert!(closed.get("error").is_none(), "close error: {closed}");
        assert_eq!(workspace.lock().unwrap().panes().len(), 1);
    }
}
