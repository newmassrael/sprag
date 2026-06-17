//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire over a line-delimited transport,
//! projecting the live [`TerminalSession`] screen fresh for each request.
//! This is the runnable form of the headless data path (DESIGN.md §1/§3):
//! an external AI peer reads the terminal as data with no GPU and no shell
//! event loop.
//!
//! ## Read-only boundary (enforced, not incidental)
//!
//! The host is read-only this round: input encoding (keys -> PTY bytes) is
//! sprag-owned and a later round (PINION-REQUIREMENTS R2.6). Rather than let
//! a mutating method (`scene/key`, `scene/intervene`, ...) dispatch against
//! a scene that is rebuilt and discarded every request — silently reporting
//! success while doing nothing to the PTY — [`handle_request`] gates to an
//! explicit [`READ_METHODS`] allowlist and returns a JSON-RPC
//! method-not-found error for everything else.

use std::io::{self, BufRead, Write};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{dispatch, dispatch_parsed, parse_request, DispatchContext, Request};
use sprag_terminal::TerminalSession;

/// The methods the headless read-only host answers: pure reads over a static
/// scene. Anything else gets a JSON-RPC method-not-found error (input
/// injection is sprag-owned, a later round — see the module docs).
pub const READ_METHODS: &[&str] = &["scene/snapshot", "scene/query"];

/// Answer one JSON-RPC `request_json` against the session's current screen,
/// returning the response JSON (`None` for a notification with no reply).
///
/// Projects a fresh `Scene::TextGrid` from the live screen, then either
/// dispatches an allowlisted read method, rejects a non-allowlisted method
/// with a method-not-found error, or lets `dispatch` produce the canonical
/// parse-error reply for malformed input.
#[must_use]
pub fn handle_request(
    session: &TerminalSession,
    previews: &PreviewLedger,
    revision: &SceneRevision,
    request_json: &str,
) -> Option<String> {
    let mut scene = session.with_screen(crate::scene);
    let mut ctx = DispatchContext::new(&mut scene, previews, revision);
    match parse_request(request_json) {
        Ok(request) if READ_METHODS.contains(&request.method.as_str()) => {
            dispatch_parsed(&mut ctx, request)
        }
        Ok(request) => Some(method_not_supported(&request)),
        // Malformed: let dispatch emit the canonical JSON-RPC parse error.
        Err(_) => dispatch(&mut ctx, request_json),
    }
}

/// Build the JSON-RPC method-not-found (-32601) reply for a well-formed but
/// non-allowlisted request, naming the read-only boundary.
fn method_not_supported(request: &Request) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id,
        "error": {
            "code": -32601,
            "message": format!(
                "read-only host: '{}' is unsupported (input injection is a later round, R2.6)",
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
    fn serve_rejects_mutating_methods_with_method_not_found() {
        let session = run_to_eof("printf hi", 20, 4);
        let value = serve_one(
            &session,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/key","params":{"key":"a"}}"#,
        );
        assert_eq!(value["id"], 2);
        assert_eq!(value["error"]["code"], -32601);
    }
}
