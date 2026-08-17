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
    /// **WHAT THIS SESSION HAD TO READ BEFORE IT CHANGED ANYTHING** — or [`None`] where it changed
    /// nothing at all. See [`Warmup`].
    pub warmup: Option<Warmup>,
    /// **WHAT A RESTART RE-PAYS** — the cache this session had to WRITE on its very first billed
    /// request, before there was anything to read back. Zero for a session with no request yet.
    ///
    /// # ⚠⚠⚠ Why the first request and not an average
    ///
    /// It is a toll, not a rate. Measured on a near-empty project, a session's first turn cost 4.7
    /// times a later one and cache writing was 88% of that first turn — and it is charged again
    /// every time a session is replaced. Averaging it over the session would hide exactly the thing
    /// a restart decision needs to see.
    ///
    /// ⚠⚠ **AND IT GROWS WITH THE AUTHOR'S OWN BASE CONTEXT**, so the 4.7 is a floor rather than a
    /// figure to reuse: a repository with a large standing instruction file pays more. That is
    /// precisely why this is READ per session instead of being written down as a constant.
    pub cold: u64,
    /// **THE PART OF THE CONTEXT A RESTART CANNOT ESCAPE** — the cache read of this session's SECOND
    /// billed request. Zero for a session that has not made two.
    ///
    /// # ⚠⚠⚠ Why the second request, which is the whole subtlety
    ///
    /// The first request has nothing to read back; the second is the earliest one that shows the
    /// standing cost of the session — the system prompt and the tool definitions, about 38,500
    /// tokens where this was measured. A restart pays that again rather than escaping it, so it is
    /// the SUBTRAHEND: what a restart can actually discard is `context - floor`, and nothing below
    /// that line is available however long the session runs.
    ///
    /// ⚠⚠ Measured, the discardable part was 31% of this floor even at the session's most expensive
    /// turn, which is why a restart is so hard to pay for: writing a cache costs twenty times
    /// reading one, so a restart must save twenty times what [`cold`](Self::cold) rewrites.
    pub floor: u64,
}

/// **THE WARM-UP: what a session spent getting to the point where it could act.**
///
/// # ⚠⚠⚠ Why this number and not another
///
/// A loop's only lever over context is what its NEXT session starts with — a running agent's
/// context cannot be pruned. So the question *"did carrying something across the boundary help?"*
/// has exactly one honest form: **did the next session reach its first change having read less?**
/// Everything before that first change is orientation, and orientation is what a distillation is
/// for.
///
/// ⚠⚠ **MEASURED BEFORE ANYTHING WAS BUILT ON IT**, over three real sessions of this repo:
///
/// | session | context at the first change | tool calls to get there | calls in the whole session |
/// |---|---|---|---|
/// | `fc98f60a` | 128,030 | **18** | 658 |
/// | `196efb19` | 127,929 | 34 | 312 |
/// | `e8aa7127` | 158,141 | 40 | 246 |
///
/// So a session of this project spends **roughly 130-160k tokens and 18-40 tool calls before it
/// changes a byte**. The number exists, it varies, and it is on the axis a distillation claims to
/// move.
///
/// ⚠⚠⚠ **AND IT IS AN AXIS, NOT YET A VERDICT.** Those three sessions did different work, so the
/// spread across them says nothing about any feature. What it can settle is a BEFORE and AFTER on
/// comparable work — which is why this exists before the thing it is meant to judge, rather than
/// after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Warmup {
    /// The accumulated context on the request that carried the session's FIRST change.
    pub context: u64,
    /// How many tool calls the session had made by then, that first change included.
    ///
    /// ⚠ The cheaper half of the pair, and the one that survives a change in how usage is
    /// accounted: it counts acts rather than tokens.
    pub calls: u64,
}

/// **WHICH TOOL NAMES COUNT AS CHANGING SOMETHING** — the moment a session stops orienting itself
/// and starts working.
///
/// ⚠⚠ **A LIST WITH NO GLOB DECIDES ALONE**, and this one is a claim about another program's tool
/// vocabulary. A writing tool this does not name makes the warm-up read `None` (the session never
/// changed anything) or land on a LATER change — both of which understate nothing and overstate
/// nothing, but say the wrong thing quietly. The residue is stated rather than guessed around:
/// `Bash` is deliberately absent, because a shell command is as often a question as an edit, and a
/// rule that counted it would mark almost every session's third call as the moment work began.
const CHANGES: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

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
    let mut calls = 0_u64;
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
        // ⚠⚠⚠ THE TOLL A RESTART RE-PAYS, taken on the FIRST billed request and never updated: what
        // this session had to WRITE into cache before it could read anything back. See `cold`.
        if spend.requests == 1 {
            spend.cold = field("cache_creation_input_tokens");
        }
        // ⚠⚠⚠ THE FLOOR, taken on the SECOND and never updated — see `floor` for why the second and
        // not the first. Cheap to keep and impossible to recover later: by the time a caller wants
        // it the record has been read past.
        if spend.requests == 2 {
            spend.floor = cached;
        }
        spend.cached = cached;
        // Everything the model was charged to READ on this request: what was sent, what was served
        // from cache, and what was written into cache on the way.
        spend.context = field("input_tokens") + cached + field("cache_creation_input_tokens");

        // ⚠⚠⚠ THE WARM-UP IS COUNTED HERE, INSIDE THE SAME DEDUPLICATION, and that is not tidiness:
        // a streamed reply repeats its whole envelope, so tool calls counted per ROW would multiply
        // by however long the answer was — exactly what the doc above says of usage. One message,
        // one count.
        for block in row
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            calls += 1;
            // ⚠ THE FIRST ONE WINS AND IS NEVER OVERWRITTEN. The question is *what did it cost to
            // get STARTED*, so a later change must not move the answer.
            if spend.warmup.is_none()
                && block
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| CHANGES.contains(&name))
            {
                spend.warmup = Some(Warmup {
                    context: spend.context,
                    calls,
                });
            }
        }
    }
    spend
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// One assistant row: `context` tokens of cache, and whatever tool calls `names` makes.
    fn turn(id: &str, context: u64, names: &[&str]) -> String {
        let blocks: Vec<String> = names
            .iter()
            .map(|name| format!(r#"{{"type":"tool_use","name":"{name}","input":{{}}}}"#))
            .collect();
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","usage":{{"input_tokens":0,"cache_read_input_tokens":{context},"cache_creation_input_tokens":0,"output_tokens":1}},"content":[{}]}}}}"#,
            blocks.join(",")
        )
    }

    /// ⚠⚠⚠ **THE WARM-UP: WHAT A SESSION READ BEFORE IT CHANGED ANYTHING** — the axis any claim
    /// about carrying context across a session boundary has to be settled on.
    ///
    /// # ⚠⚠ What this asserts, and what it deliberately does not
    ///
    /// It asserts the READER, over a record whose numbers are known because they were written here.
    /// It says nothing about whether 130,000 is a lot — that is a question about somebody's work,
    /// and the answer only exists as a BEFORE and an AFTER on comparable work.
    ///
    /// ⚠⚠⚠ **THE STREAMING TRAP IS THE SHARP ONE.** A streamed reply repeats its whole envelope
    /// row after row, which is why `spend_in` deduplicates by message id — and tool calls counted
    /// per ROW would be multiplied by however long the answer happened to be. A warm-up of *"forty
    /// calls"* that was really four would make every comparison meaningless in the direction that
    /// looks like data.
    #[test]
    fn the_warm_up_is_what_was_read_before_the_first_change() {
        let record = [
            turn("m1", 1_000, &["Read", "Bash"]),
            turn("m2", 5_000, &["Read"]),
            // ⚠ THE SAME MESSAGE, TWICE — a streamed reply's repeat. It must count ONCE.
            turn("m3", 9_000, &["Grep", "Edit"]),
            turn("m3", 9_000, &["Grep", "Edit"]),
            turn("m4", 40_000, &["Edit"]),
        ]
        .join("\n");

        let spend = spend_in(&record);
        assert_eq!(
            spend.warmup,
            Some(Warmup {
                context: 9_000,
                calls: 5,
            }),
            "⚠⚠⚠ the warm-up is the context on the request that carried the FIRST change, and the \
             calls made up to and including it — three before it (Read, Bash, Read), then Grep, \
             then the Edit. A reader that took the LAST change would answer 40,000, and a reader \
             that counted the streamed repeat would answer seven calls: {:?}",
            spend.warmup,
        );

        assert_eq!(
            spend_in(&turn("m1", 8_000, &["Read", "Bash", "Grep"])).warmup,
            None,
            "⚠⚠ a session that changed NOTHING has no warm-up, and that must not read as zero — \
             zero is what a session that started work instantly would look like, and these are \
             opposite facts",
        );

        assert_eq!(
            spend_in(&[turn("m1", 3_000, &["Write"]), turn("m2", 7_000, &["Edit"])].join("\n"))
                .warmup,
            Some(Warmup {
                context: 3_000,
                calls: 1,
            }),
            "⚠ every writing tool starts the work, not `Edit` alone — see `CHANGES`",
        );
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
                // ⚠ These rows carry no content at all, so nothing was ever changed — see
                // `the_warm_up_is_what_was_read_before_the_first_change` for why that is `None`
                // rather than zero.
                warmup: None,
                // The FIRST request's cache write and the SECOND's cache read: taken once each and
                // never updated, which is what makes them a toll and a floor rather than a running
                // total. The duplicated `msg_1` rows must not move either.
                cold: 10,
                floor: 200,
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
                warmup: None,
                // ⚠ ONE billed request, so there is a toll and NO floor: `floor` is the SECOND
                // request's read and this record never reaches one. Zero here is *not yet known*,
                // which is why the discardable amount degrades to the whole context rather than to
                // a negative number.
                cold: 0,
                floor: 0,
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
