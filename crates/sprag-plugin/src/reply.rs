//! Parsing a Claude CLI result envelope (`claude -p --output-format json`) out
//! of a pane's raw output — the reply text and the real token cost that replace
//! the byte-count proxy.
//!
//! This is *perception* of an external tool's structured output — the same kind
//! of work the emulator does parsing PTY bytes — not serialization of sprag's
//! own types. So `serde_json` here does not breach the substrate's serde-free
//! contract (the host still owns rendering sprag types onto the RPC wire), and
//! the public surface stays plain: [`AgentReply`] (the tool-agnostic decoded
//! shape) carries no derives, the envelope is Value-walked rather than
//! deserialized into a DTO.
//!
//! Robustness is the whole point. Every field is optional and any malformed,
//! truncated, or unrecognized input degrades to `None`, so a broken envelope
//! never breaks a run — the caller falls back to the raw text and the
//! byte-count cost. The function never panics.

use serde_json::Value;

/// A decoded agent reply — the normalized result of parsing a structured
/// AI-tool output, independent of which tool produced it. The *shape* is
/// tool-agnostic (reply text + real token cost); a per-tool parser (today
/// [`parse_claude_json`]) maps that tool's specific envelope onto it. A second
/// adapter adds its own parser, not a new result type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentReply {
    /// The reply text, trimmed; `None` if the output carried no reply field
    /// (the caller falls back to the raw captured text).
    pub text: Option<String>,
    /// Real billed tokens for this turn (input + output), the cost a
    /// token-denominated run bills; `None` if the output reported no usage.
    /// Cache-creation / cache-read tokens are excluded: they are large and
    /// bursty (a single cache-warm turn can dwarf the input+output count), so
    /// counting them would make a token budget meaningless.
    pub tokens: Option<u64>,
    /// The tool's conversation/session id, if it reports one. A multi-turn
    /// adapter reuses it to resume server-side context (e.g. `claude --resume
    /// <id>`) instead of resending the whole transcript. `None` for outputs
    /// that carry no session. A present id signals a healthy, continuable
    /// session; its absence on a resume turn is the cue to start fresh.
    pub session_id: Option<String>,
}

/// Parse a Claude `--output-format json` envelope from `raw` output bytes.
///
/// Tolerates framing noise: the `{`…`}` span is extracted first, so a trailing
/// newline, a kernel `\r\n` (ONLCR over a PTY), or stray bytes outside the
/// object are ignored. Returns `None` — and the caller degrades to the raw text
/// and the byte-count cost — when the bytes hold no JSON object, the JSON is
/// malformed (a truncated/oversized capture), or the object is not a result
/// envelope (no `result` and no `usage` key). Never panics.
#[must_use]
pub fn parse_claude_json(raw: &[u8]) -> Option<AgentReply> {
    let text = String::from_utf8_lossy(raw);
    let span = json_object_span(&text)?;
    let value: Value = serde_json::from_str(span).ok()?;
    let object = value.as_object()?;
    // Require an envelope signature so a random JSON object is not claimed as a
    // reply. A Claude result envelope always carries `result` and/or `usage`.
    if !object.contains_key("result") && !object.contains_key("usage") {
        return None;
    }
    Some(AgentReply {
        text: object
            .get("result")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string()),
        tokens: parse_tokens(&value),
        session_id: object
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Billed tokens = `usage.input_tokens + usage.output_tokens`, or `None` if
/// `usage` is absent (neither count present). Cache-creation / cache-read counts
/// are excluded on purpose: they are large and bursty, so folding them in would
/// void a token budget (one cache-warm turn could blow any sane ceiling).
fn parse_tokens(value: &Value) -> Option<u64> {
    let usage = value.get("usage")?;
    let input = usage.get("input_tokens").and_then(Value::as_u64);
    let output = usage.get("output_tokens").and_then(Value::as_u64);
    match (input, output) {
        (None, None) => None,
        (input, output) => Some(input.unwrap_or(0) + output.unwrap_or(0)),
    }
}

/// The substring from the first `{` to the last `}` inclusive — the JSON object
/// embedded in a stream that may carry leading or trailing framing bytes.
fn json_object_span(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end >= start).then(|| &text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A faithful slice of a real `claude -p --output-format json` line.
    const REAL: &str = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"PONG","stop_reason":"end_turn","session_id":"abc","total_cost_usd":0.228448,"usage":{"input_tokens":5900,"cache_creation_input_tokens":19043,"cache_read_input_tokens":15626,"output_tokens":5}}"#;

    #[test]
    fn parses_the_real_envelope() {
        let reply = parse_claude_json(REAL.as_bytes()).expect("a reply");
        assert_eq!(reply.text.as_deref(), Some("PONG"));
        // input + output only; cache (19043 + 15626) is excluded by design.
        assert_eq!(reply.tokens, Some(5905));
        assert_eq!(reply.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn preserves_escaped_newlines_in_result() {
        // The envelope is one physical line; the reply's newlines are escaped.
        let raw = r#"{"result":"line1\nline2","usage":{"input_tokens":1,"output_tokens":2}}"#;
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.text.as_deref(), Some("line1\nline2"));
        assert_eq!(reply.tokens, Some(3));
    }

    #[test]
    fn tolerates_framing_noise_around_the_object() {
        // A leading stray byte and a trailing CRLF (ONLCR over a PTY) — the
        // brace-span extraction handles both.
        let raw = format!("\r{REAL}\r\n");
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.text.as_deref(), Some("PONG"));
    }

    #[test]
    fn truncated_json_is_none() {
        // A capture cut off mid-envelope (timeout / over-cap): no closing brace.
        let raw = &REAL[..REAL.len() - 10];
        assert!(parse_claude_json(raw.as_bytes()).is_none());
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse_claude_json(b"not json at all").is_none());
        assert!(parse_claude_json(b"").is_none());
    }

    #[test]
    fn a_non_envelope_object_is_none() {
        // Valid JSON, but not a result envelope (no `result`, no `usage`).
        assert!(parse_claude_json(br#"{"foo":1,"bar":"baz"}"#).is_none());
    }

    #[test]
    fn error_envelope_still_extracts_text_and_tokens() {
        // An envelope carrying `is_error:true` (a tool-side error). We no longer
        // model that flag, but the unrecognized key must not derail parsing: the
        // reply text and real tokens still extract, so a tool-side error stays a
        // reply to record (continuable via its session_id), not a parse failure.
        let raw = r#"{"is_error":true,"result":"rate limited","usage":{"input_tokens":4,"output_tokens":0}}"#;
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.text.as_deref(), Some("rate limited"));
        assert_eq!(reply.tokens, Some(4));
    }

    #[test]
    fn missing_usage_yields_no_tokens() {
        let raw = r#"{"result":"hi"}"#;
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.text.as_deref(), Some("hi"));
        assert_eq!(reply.tokens, None);
        // No session_id key -> None (the cue a resume turn uses to start fresh).
        assert_eq!(reply.session_id, None);
    }

    #[test]
    fn extracts_session_id_when_present() {
        let raw = r#"{"result":"hi","session_id":"sess-123","usage":{"input_tokens":1,"output_tokens":1}}"#;
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn usage_without_result_keeps_tokens() {
        // A degenerate envelope with usage but no result: the caller falls back
        // to the raw text for the message but still bills the real tokens.
        let raw = r#"{"usage":{"input_tokens":7,"output_tokens":3}}"#;
        let reply = parse_claude_json(raw.as_bytes()).expect("a reply");
        assert_eq!(reply.text, None);
        assert_eq!(reply.tokens, Some(10));
    }
}
