//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire — `scene/snapshot` and the other read
//! methods `pinion_rpc::dispatch` answers from a static scene — over a
//! line-delimited transport, projecting the live [`TerminalSession`] screen
//! fresh for each request. This is the runnable form of the headless data
//! path (DESIGN.md §1/§3): an external AI peer reads the terminal as data
//! with no GPU and no shell event loop.
//!
//! Input injection (keys -> PTY bytes) is not wired here yet: the TextGrid
//! is a paint-opaque data leaf, so `scene/key` does not reach the PTY. Key
//! encoding is sprag-owned and a later round (PINION-REQUIREMENTS R2.6).

use std::io::{self, BufRead, Write};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{dispatch, DispatchContext};

use crate::TerminalSession;

/// Answer one JSON-RPC `request` against the session's current screen,
/// returning the response JSON (`None` for a notification with no reply).
///
/// Pumps pending PTY output into the emulator first, then projects a fresh
/// `Scene::TextGrid` and dispatches against it. `previews` and `revision`
/// are server-scoped handles the caller owns across requests (the
/// `scene/snapshot` read path leaves them untouched).
#[must_use]
pub fn handle_request(
    session: &mut TerminalSession,
    previews: &PreviewLedger,
    revision: &SceneRevision,
    request: &str,
) -> Option<String> {
    session.pump();
    let mut scene = crate::scene(session.screen());
    let mut ctx = DispatchContext::new(&mut scene, previews, revision);
    dispatch(&mut ctx, request)
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
    session: &mut TerminalSession,
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
    use crate::CommandBuilder;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    /// The server loop answers a real `scene/snapshot` request with the
    /// live screen content — PTY output read as data over the RPC wire.
    #[test]
    fn serve_answers_scene_snapshot_with_live_screen() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf hi");
        command.env("TERM", "dumb");
        let mut session = TerminalSession::spawn(command, 20, 4).expect("spawn pty session");

        let start = Instant::now();
        while !session.is_eof() && start.elapsed() < Duration::from_secs(5) {
            session.pump_blocking(Duration::from_millis(200));
        }

        let request = r#"{"jsonrpc":"2.0","id":1,"method":"scene/snapshot","params":{"path":""}}"#;
        let input = Cursor::new(format!("{request}\n").into_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve(&mut session, input, &mut output).expect("serve loop");

        let response = String::from_utf8(output).expect("utf8 response");
        let value: serde_json::Value =
            serde_json::from_str(response.trim()).expect("valid json-rpc response");
        assert_eq!(value["id"], 1);
        assert!(value.get("error").is_none(), "unexpected error: {response}");
        // The live screen's row text travels in the snapshot result.
        assert!(
            value["result"].to_string().contains("hi"),
            "expected 'hi' in result, got: {}",
            value["result"]
        );
    }
}
