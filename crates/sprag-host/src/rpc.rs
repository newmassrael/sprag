//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire over a line-delimited transport,
//! projecting the live [`TerminalSession`] screen fresh for each request.
//! This is the runnable form of the headless data path (DESIGN.md §1/§3):
//! an external AI peer reads the terminal as data with no GPU and no shell
//! event loop.
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

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{dispatch, dispatch_parsed, parse_request, DispatchContext, Request};
use sprag_terminal::TerminalSession;

/// The methods the headless host answers: pure reads over the pane scene
/// (`scene/snapshot`, `scene/query`) plus the `scene/invoke` input channel.
/// Anything else gets a JSON-RPC method-not-found error.
pub const SUPPORTED_METHODS: &[&str] = &["scene/snapshot", "scene/query", "scene/invoke"];

/// Answer one JSON-RPC `request_json` against the session's current pane,
/// returning the response JSON (`None` for a notification with no reply).
///
/// Assembles a fresh pane scene (`Container[TextGrid + External]`) from the
/// live session, then either dispatches an allowlisted method
/// ([`SUPPORTED_METHODS`]: the reads plus `scene/invoke` input), rejects a
/// non-allowlisted method with a method-not-found error, or lets `dispatch`
/// produce the canonical parse-error reply for malformed input.
#[must_use]
pub fn handle_request(
    session: &TerminalSession,
    previews: &PreviewLedger,
    revision: &SceneRevision,
    request_json: &str,
) -> Option<String> {
    let mut scene = crate::pane_scene(session);
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
    session: &TerminalSession,
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
        if let Some(response) = handle_request(session, &previews, &revision, request) {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::CommandBuilder;
    use std::io::Cursor;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// Spawn a one-shot command and block (bounded) until it has closed,
    /// so the reader thread has applied all of its output.
    fn run_to_eof(script: &str, cols: u16, rows: u16) -> TerminalSession {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let session = TerminalSession::spawn(command, cols, rows).expect("spawn pty session");
        let start = Instant::now();
        while !session.is_eof() && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        session
    }

    fn serve_one(session: &TerminalSession, request: &str) -> serde_json::Value {
        let input = Cursor::new(format!("{request}\n").into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(session, input, &mut output).expect("serve loop");
        let response = String::from_utf8(output).expect("utf8 response");
        serde_json::from_str(response.trim()).expect("valid json-rpc response")
    }

    #[test]
    fn serve_answers_scene_snapshot_with_live_screen() {
        let session = run_to_eof("printf hi", 20, 4);
        let value = serve_one(
            &session,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert_eq!(value["id"], 1);
        assert!(value.get("error").is_none(), "unexpected error: {value}");
        assert!(
            value["result"].to_string().contains("hi"),
            "expected 'hi' in result, got: {}",
            value["result"]
        );
    }

    #[test]
    fn serve_rejects_scene_key_in_favor_of_scene_invoke() {
        // Input rides scene/invoke against the engine External, not pinion's
        // widget-oriented scene/key — so scene/key stays unsupported.
        let session = run_to_eof("printf hi", 20, 4);
        let value = serve_one(
            &session,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/key","params":{"key":"a"}}"#,
        );
        assert_eq!(value["id"], 2);
        assert_eq!(value["error"]["code"], -32601);
    }

    /// Spawn a long-lived `cat` on the PTY (it echoes injected input back via
    /// the line discipline, so keystrokes appear on the screen).
    fn spawn_cat(cols: u16, rows: u16) -> TerminalSession {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        TerminalSession::spawn(command, cols, rows).expect("spawn pty session")
    }

    fn invoke_key(session: &TerminalSession, key: &str) {
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/sprag_input/external/key","args":{{"key":"{key}"}}}}}}"#
        );
        let value = serve_one(session, &request);
        assert!(value.get("error").is_none(), "invoke error: {value}");
    }

    #[test]
    fn serve_injects_key_via_scene_invoke() {
        // End-to-end input: scene/invoke encodes "h" and "i" to PTY bytes and
        // writes them; the line discipline echoes them, so the live snapshot
        // shows "hi" once the reader thread has applied the echo.
        let session = spawn_cat(20, 4);
        invoke_key(&session, "h");
        invoke_key(&session, "i");

        let start = Instant::now();
        let mut echoed = false;
        while !echoed && start.elapsed() < Duration::from_secs(5) {
            let snap = serve_one(
                &session,
                r#"{"jsonrpc":"2.0","id":9,"method":"scene/snapshot","params":{"path":""}}"#,
            );
            echoed = snap["result"].to_string().contains("hi");
            if !echoed {
                sleep(Duration::from_millis(20));
            }
        }
        assert!(echoed, "injected 'hi' never appeared in the snapshot");
    }
}
