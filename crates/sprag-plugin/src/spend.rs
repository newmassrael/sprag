//! **WHAT AN INNER AGENT SESSION IS ACTUALLY BEING CHARGED TO READ**, recovered from the record the
//! agent keeps about itself.
//!
//! # ⚠⚠⚠ Why a loop cannot answer this from anything it already has
//!
//! `ai_loop` reports [`Cost::Bytes`](crate::Cost::Bytes) — the bytes it typed — and bounds itself
//! with [`Ceiling::Turns`](crate::Ceiling::Turns). Measurement says neither tracks the bill:
//!
//! * across forty local agent sessions, **cache read is 99.0% of tokens and 78.1% of cost**, while
//!   `input + output` — the only part a prompt's size resembles — is **10.3% of cost**. The
//!   component a byte count stands for is also the one that *falls* as a session grows;
//! * a turn is not a unit of anything: what one billed request adds to the context is **861 tokens
//!   at the median and 633,749 at the maximum**, and predicting context from a turn count is out by
//!   19% at the median, 63% at p90 and 30× at worst.
//!
//! The quantity that does track it — accumulated context — is written by the agent on every request
//! and, until this module, read by nobody. Both figures are in
//! `claudedocs/INSIGHT-LOOP-SCORING-AND-COST-SIGNALS.md`.
//!
//! # ⚠⚠ The identity is what makes it findable, and it is MINTED rather than recovered
//!
//! An agent files its record under a name of its own choosing, in a directory keyed by the cwd it
//! started in. Every route to that from outside is a guess — the live cwd drifts the moment the
//! agent works in a subdirectory, the spawn cwd is stored nowhere, and taking the newest file in a
//! directory races any other session in the same repository — and **all three fail by silently
//! reading somebody else's record rather than by failing.**
//!
//! So sprag names the session at its birth (`sprag_host::hooks::identity_flag`) and reads the name
//! back off the running process — [`PaneForegroundJob`](crate::access::PaneForegroundJob)'s
//! `JobProcess::argv`, which already answers on both platforms sprag builds for. The file is called
//! what sprag called it, and no directory is involved.
//!
//! # This is a per-tool adapter, exactly as [`reply`](crate::reply) is
//!
//! `claude`'s record is JSONL under `~/.claude/projects/<dir>/<session>.jsonl`. A second agent adds
//! its own reader, not a new result type: [`Spend`] is the tool-agnostic shape.

use serde_json::Value;

/// The argument `claude` takes a caller-chosen session identity on.
///
/// # ⚠⚠⚠ TWO COPIES OF ONE STRING, AND A GATE RATHER THAN A HOPE
///
/// The WRITER is `sprag_host::hooks::CLAUDE.identity_flag`, which puts it on an agent's command
/// line at that pane's birth; this is the READER, which takes it back off the running process. They
/// are in different crates because the host depends on the plugin and not the other way round, so
/// the string cannot simply be shared — and two copies of one rule that drift apart fail SILENTLY:
/// the loop would find no identity, report no spend, and look exactly like an agent that had not
/// started yet.
///
/// So the agreement is asserted where both are visible, by `sprag-host`'s
/// `the_flag_that_names_a_session_is_the_flag_that_finds_it`. Change one and that goes red.
pub const CLAUDE_IDENTITY_FLAG: &str = "--session-id";

/// Where `claude` files what it records about one session, given that session's identity.
///
/// # ⚠⚠ Why the directory is not derived, when `claude` derives one
///
/// The agent files under a directory named for the cwd it started in. Reproducing that name from
/// outside is a guess with a silent failure — the live cwd drifts, the spawn cwd is stored nowhere,
/// and the newest file in a directory belongs to whichever session wrote last. **The identity makes
/// the directory irrelevant**: the file is NAMED for it, so every project directory is searched for
/// that one name and the first hit is the record. No cwd, no recency, no slug.
///
/// `None` when `$HOME` is unset, the projects directory does not exist, or no record carries that
/// name — which is the ordinary state of a session that has been started and has not yet been asked
/// anything, since nothing is written until there is something to record.
#[must_use]
pub fn record_of(identity: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let projects = std::path::PathBuf::from(home)
        .join(".claude")
        .join("projects");
    let wanted = format!("{identity}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&wanted))
        .find(|candidate| candidate.is_file())
}

/// What the session named `identity` has been charged, or `None` if it has written nothing yet.
///
/// ⚠ The read is a whole-file parse on every call, which is what makes it the CALLER's business how
/// often to ask. A record grows with the session, and a loop that consulted this on every poll
/// rather than once a turn would spend more reading about its agent than its agent spends thinking.
#[must_use]
pub fn spend_of(identity: &str) -> Option<Spend> {
    let record = record_of(identity)?;
    Some(spend_in(&std::fs::read_to_string(record).ok()?))
}

/// What one agent session has been charged to read, as of its most recent billed request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spend {
    /// Distinct billed requests seen in the record.
    ///
    /// Deduplicated by the message id: a streamed reply appears many times and every fragment
    /// repeats the same usage, so counting rows would multiply this by however long the answer was.
    pub requests: u64,
    /// **THE ACCUMULATED CONTEXT** on the most recent request: everything the model was charged to
    /// read, cache included. The quantity a restart discards and a budget should be denominated in.
    pub context: u64,
    /// Of [`context`](Self::context), the part served from cache rather than sent.
    pub cached: u64,
    /// What the session has produced, over all its requests. The only component that is neither
    /// re-read nor re-sent, and so the only one that does not grow with the conversation.
    pub produced: u64,
}

/// The session identity in `argv`, if `flag` names one.
///
/// Both spellings a command line has for one argument, for the reason the flag's own writer
/// records: a value may be joined with `=`, and a reader that knew only the separated form would
/// find nothing on half the launches that carry it.
#[must_use]
pub fn identity_in(argv: &[String], flag: &str) -> Option<String> {
    let joined = format!("{flag}=");
    let mut words = argv.iter();
    while let Some(word) = words.next() {
        if let Some(value) = word.strip_prefix(&joined) {
            return (!value.is_empty()).then(|| value.to_owned());
        }
        if word == flag {
            return words.next().filter(|next| !next.is_empty()).cloned();
        }
    }
    None
}

/// Read `record` — one agent session's JSONL — and answer what it has been charged.
///
/// ⚠ **EVERY MALFORMED LINE IS SKIPPED AND NOTHING PANICS.** The file is written by another process
/// while this one reads it, so a truncated final line is ordinary rather than exceptional; a reader
/// that refused the whole record over one would answer nothing for the case it exists to serve.
#[must_use]
pub fn spend_in(text: &str) -> Spend {
    let mut seen: Vec<String> = Vec::new();
    let mut spend = Spend::default();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = row.pointer("/message/usage") else {
            continue;
        };
        // The presence of a cache-read count is what marks a row as a BILLED request rather than as
        // a fragment carrying a partial envelope. A row without it is not a request this can price.
        let Some(cached) = usage.get("cache_read_input_tokens").and_then(Value::as_u64) else {
            continue;
        };
        let id = row
            .pointer("/message/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !id.is_empty() && seen.iter().any(|already| already == id) {
            continue;
        }
        if !id.is_empty() {
            seen.push(id.to_owned());
        }
        let field = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
        spend.requests += 1;
        spend.produced += field("output_tokens");
        spend.cached = cached;
        // Everything the model was charged to READ on this request: what was sent, what was served
        // from cache, and what was written into cache on the way.
        spend.context = field("input_tokens") + cached + field("cache_creation_input_tokens");
    }
    spend
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// Both spellings of one argument, and the absences that are not it.
    #[test]
    fn an_identity_is_read_from_either_spelling() {
        assert_eq!(
            identity_in(&owned(&["claude", "--session-id", "abc"]), "--session-id"),
            Some("abc".to_owned()),
        );
        assert_eq!(
            identity_in(&owned(&["claude", "--session-id=abc"]), "--session-id"),
            Some("abc".to_owned()),
            "the joined spelling, which is half of the launches that carry it",
        );
        assert_eq!(
            identity_in(&owned(&["claude", "--model", "opus"]), "--session-id"),
            None,
        );
        assert_eq!(
            identity_in(&owned(&["claude", "--session-id"]), "--session-id"),
            None,
            "a flag with nothing after it names no session",
        );
        assert_eq!(
            identity_in(&owned(&["claude", "--session-id", ""]), "--session-id"),
            None,
            "and neither does an empty value",
        );
        assert_eq!(
            identity_in(
                &owned(&["claude", "--settings", "--session-id", "abc"]),
                "--session-id",
            ),
            Some("abc".to_owned()),
            "a flag that follows another flag's position is still the flag",
        );
    }

    /// **A STREAMED REPLY IS ONE REQUEST**, however many rows it left behind.
    ///
    /// The defect this pins is not hypothetical: the fragments repeat the whole usage object, so a
    /// reader that counted rows would report a session as having made as many requests as its
    /// longest answer had chunks — and would still get `context` right, which is what makes it the
    /// kind of wrong that survives a casual look.
    #[test]
    fn a_streamed_reply_counts_once() {
        let record = r#"
{"type":"assistant","message":{"id":"msg_1","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":10,"output_tokens":5}}}
{"type":"assistant","message":{"id":"msg_1","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":10,"output_tokens":5}}}
{"type":"assistant","message":{"id":"msg_2","usage":{"input_tokens":3,"cache_read_input_tokens":200,"cache_creation_input_tokens":20,"output_tokens":7}}}
"#;
        assert_eq!(
            spend_in(record),
            Spend {
                requests: 2,
                context: 223,
                cached: 200,
                produced: 12,
            },
        );
    }

    /// Everything a record holds that is not a billed request, and a line that is not JSON at all.
    #[test]
    fn a_record_being_written_while_it_is_read_is_ordinary() {
        let record = r#"
{"type":"user","message":{"content":"hello"}}
{"type":"assistant","message":{"id":"msg_1","usage":{"output_tokens":5}}}
{"type":"assistant","message":{"id":"msg_2","usage":{"input_tokens":1,"cache_read_input_tokens":50,"output_tokens":2}}}
{"type":"assistant","message":{"id":"msg_3","usa
"#;
        assert_eq!(
            spend_in(record),
            Spend {
                requests: 1,
                context: 51,
                cached: 50,
                produced: 2,
            },
            "a usage with no cache read is not a billed request, and a half-written last line is \
             the ordinary state of a file another process is appending to",
        );
        assert_eq!(spend_in(""), Spend::default(), "and an empty record");
    }

    /// `context` is the LAST request's, not a sum: it is a level, and summing levels answers a
    /// question nobody asked.
    #[test]
    fn context_is_a_level_and_produced_is_a_total() {
        let record = r#"
{"type":"assistant","message":{"id":"a","usage":{"input_tokens":1,"cache_read_input_tokens":10,"output_tokens":100}}}
{"type":"assistant","message":{"id":"b","usage":{"input_tokens":1,"cache_read_input_tokens":20,"output_tokens":200}}}
"#;
        let spend = spend_in(record);
        assert_eq!(spend.context, 21, "the level the session has reached");
        assert_eq!(spend.produced, 300, "and the total it has written");
    }
}
