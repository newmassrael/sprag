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
//! # ⚠⚠⚠⚠ AND THAT ROAD IS ONLY OPEN FOR AN AGENT SPRAG ITSELF LAUNCHED
//!
//! A minted name is a claim about **who started the process**, and nothing else here owns that
//! fact. Measured 2026-08-17 on the machine this loop runs on: `claude` 2.1.233 honours
//! `--session-id` exactly (a probe filed its record under the name it was given), and **not one of
//! the twenty-five live `claude` processes carried the flag at all**. An agent a person started, or
//! a pane re-made by hand after a failed delivery, has no minted name — so the whole road answers
//! `None` and the loop reads a zero it cannot distinguish from a small session (register item 431).
//!
//! ⇒ [`spend_at`] is the road that does not depend on who launched the agent: the agent STATES
//! where it writes on its own submit hook, and a statement needs no derivation. This one stays as
//! the fallback for a peer with no hooks, which is the only peer that states nothing.
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

/// The argument `claude` takes its PERMISSION MODE on — what decides whether the agent answers its
/// own prompts or stands every one of them in front of a person.
///
/// # ⛔⛔⛔⛔⛔ Why a flag, and not the mode showing on the screen — register item 995
///
/// The mode is also a RUNTIME state: a person cycles it with `S-Tab` and the footer says
/// `⏵⏵ auto mode on`. Reading that footer answers *what is this session doing now*, and the loop's
/// question is a different one, because the loop REPLACES its session. `PaneLifecycle::respawn`
/// re-runs the argv the pane is currently running — argv is the ONE thing a replacement inherits —
/// and a keystroke is not in it. So a mode cycled by hand dies with the process that held it, and
/// every replacement is born from whatever the agent remembers instead.
///
/// ⇒ **The flag is the only spelling of this fact that outlives a replacement.** A run whose argv
/// names the mode is one whose replacements are all born in it; a run whose argv does not is one
/// nobody can answer for, however green the footer looked when a watcher last looked at it.
///
/// ⚠⚠ Measured 2026-09-09 on run 271, which is what opened item 995: two panes made the same way
/// minutes apart were born in DIFFERENT modes, and the replacement the loop made for itself was
/// born `manual` and stood every dialog in front of a person until the run ended at 522
/// iterations. The watcher's own check (`(auto|manual) mode on`, then `S-Tab` to cycle) is a
/// runtime repair and so could not survive the first replacement it was applied before.
///
/// ⚠ Read by [`identity_in`] exactly as the identity flag above is, because the two are the same
/// shape of fact — a value the launcher put on the command line and the loop takes back off the
/// running process — and one reader is what stops them drifting.
pub const CLAUDE_MODE_FLAG: &str = "--permission-mode";

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
///
/// # ⚠⚠⚠⚠ AND THE NAME IS STILL A DERIVATION, WHICH WAS MEASURED READING NOTHING
///
/// The name this resolves is the one the SESSION WAS LAUNCHED WITH, and an agent may file under a
/// different one. Measured 2026-08-17 (register item 431): a pane born
/// `--session-id 97f5ffd9-…` reported `3f4ffa52-…` while it worked; there is no `97f5ffd9-…`
/// record anywhere under `$HOME/.claude/projects`, and the `3f4ffa52-…` record was 3,422,727 bytes
/// with `"cache_read_input_tokens":466013` on its last request. **A loop composed `context: 0` out
/// of a file holding 466,013**, because the two names are not the same name.
///
/// So this is the FALLBACK and [`spend_at`] is the road: where the agent has STATED where it
/// writes — its submit hook carries `transcript_path`, and
/// [`AgentObservation::transcript`](crate::access::AgentObservation::transcript) carries it here —
/// there is nothing left to derive. This stays for the peer that states nothing, which is every
/// agent whose hooks are not installed.
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
///
/// ⚠⚠⚠ **THE NAME IS A DERIVATION AND [`spend_at`] IS NOT** — see [`record_of`], which carries what
/// deriving cost. A caller holding a path the agent STATED must not come through here.
#[must_use]
pub fn spend_of(identity: &str) -> Option<Spend> {
    spend_at(&record_of(identity)?)
}

/// What the session writing `record` has been charged, or `None` where that file cannot be read.
///
/// # ⚠⚠⚠⚠ The reader for a path somebody STATED, which is the only kind that cannot be wrong
///
/// [`record_of`]'s doc holds the measurement: a name is what a session was LAUNCHED with, and an
/// agent that files under another name leaves every reader of the derived path answering about a
/// file that does not exist — silently, as a zero. An agent's own submit hook states
/// `transcript_path` outright, so a caller that has one has nothing to guess.
///
/// ⚠ [`None`] means **this file could not be read**, and it is worth keeping apart from
/// `Some(Spend::default())`, which means *the record is there and holds no billed request yet*. A
/// caller that flattened the two would report a session it cannot see as one that has spent
/// nothing.
#[must_use]
pub fn spend_at(record: &std::path::Path) -> Option<Spend> {
    Some(spend_in(&std::fs::read_to_string(record).ok()?))
}

/// What one agent session has been charged to read, as of its most recent billed request.
///
/// ⚠ NOT [`Copy`] since register item 988: [`refused`](Self::refused) carries the words the service
/// said, and a refusal that arrives as a `bool` cannot tell a weekly limit from a five-hundred.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// tokens in the plain agent session this was first measured on and a median of **21,350**
    /// across the 250 transcripts of the repository that runs this loop (2026-08-20, register item
    /// 493: the quantity belongs to a TOOL SET, so a number carried across populations mis-prices
    /// the trade). A restart pays that again rather than escaping it, so it is
    /// the SUBTRAHEND: what a restart can actually discard is `context - floor`, and nothing below
    /// that line is available however long the session runs.
    ///
    /// ⚠⚠ Measured, the discardable part was 31% of this floor even at the session's most expensive
    /// turn, which is why a restart is so hard to pay for: writing a cache costs twenty times
    /// reading one, so a restart must save twenty times what [`cold`](Self::cold) rewrites.
    pub floor: u64,
    /// ⛔⛔⛔⛔⛔ **WHAT THE SERVICE SAID WHEN IT REFUSED THE NEWEST REQUEST**, or [`None`] where the
    /// newest one was answered — register item 988.
    ///
    /// # ⛔⛔⛔⛔⛔ A zero with two meanings, and it cost a run
    ///
    /// [`produced`](Self::produced) does not move when a session is refused, because a refusal is
    /// not billed: the row carries `input 0 · output 0 · cache_read 0`. It also does not move when a
    /// session sits there doing nothing. **Those are opposite facts and the loop read them as one**
    /// — register item 878 replaces a session that writes nothing and then fails the run, which is
    /// right for a dead peer and wrong for a refused one.
    ///
    /// ⇒ Measured 2026-09-09, run 270's replacement session
    /// (`~/.claude/projects/-home-coin-watching-zenoh/6844d6a1-…jsonl`): three rows at 03:22:23,
    /// 03:23:23 and 03:24:24 — one per prompt the loop sent, 60 seconds apart — each
    /// `isApiErrorMessage: true` and each saying *"You've hit your weekly limit · resets Sep 11, 6am
    /// (Asia/Seoul)"*. The run declared the session silent twice and died. Its FIRST answered
    /// request came 45 minutes later, and that session went on to commit 1,648 lines.
    ///
    /// # ⚠⚠⚠ Why the flag and not the model spelling, and why the newest row rather than a count
    ///
    /// The row is also `model: "<synthetic>"`, and that is corroborating rather than load-bearing:
    /// `isApiErrorMessage` is the field whose whole job is to say *this is not the model speaking*,
    /// while a model NAME is a spelling that can change under this reader without notice.
    ///
    /// ⚠⚠ It is a LEVEL about the newest billed row and never a total: an answered row CLEARS it. A
    /// count would say *this session was refused at some point*, which is true of a session that has
    /// been working happily for an hour — and the question a turn asks is whether the service is
    /// refusing NOW.
    ///
    /// ⚠ The words are carried rather than a `bool`, on this workspace's own rule about refusals: a
    /// run that says *the service refused* without saying what arrived leaves the next reader unable
    /// to tell a weekly limit from a five-hundred, and those want different acts from a person.
    ///
    /// ⛔ **THE [`Option`] ANSWERS *WAS IT REFUSED* AND THE [`String`] ANSWERS *WHAT IT SAID*.** A
    /// refusal that carried no text is `Some("")`: dropping it to [`None`] for want of words would
    /// report the row as ANSWERED, which is the one thing this field exists to deny.
    pub refused: Option<String>,
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
        // ⛔⛔⛔⛔⛔ **A ROW THE SERVICE REFUSED IS NOT A MEASUREMENT OF ANYTHING** — register item
        // 988, and it is decided here, before a single number moves.
        //
        // A refusal looks like a billed request and was never billed: `input 0 · output 0 ·
        // cache_read 0`, which is how it reaches this line at all. Counted, it would say the
        // session's accumulated context had dropped to ZERO — and `context` is a LEVEL taken from
        // the newest row, so one refusal would wipe the number every restart decision is priced on.
        // The session's context did not change; the session was not asked.
        //
        // ⚠⚠ SO IT MOVES NOTHING AND IS RECORDED AS ITSELF. `requests` is *distinct BILLED
        // requests* by its own doc, and this was not one; `cold` and `floor` are keyed on the first
        // and second of them, so a refused row counted there would take a toll off a row that was
        // never charged.
        //
        // ⚠ The flag and not the `model` spelling — see the field.
        //
        // ⛔⛔⛔ AND THE [`Option`] ANSWERS *WAS IT REFUSED*, THE [`String`] ANSWERS *WHAT IT SAID* —
        // so a refusal that carried no text at all is `Some("")` and never `None`. Dropping it to
        // `None` for want of words would report *the service answered this row*, which is the one
        // thing this field exists to deny; the reader of the words says so instead.
        if row
            .get("isApiErrorMessage")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            spend.refused = Some(
                row.pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|said| !said.is_empty())
                    .collect::<Vec<&str>>()
                    .join(" "),
            );
            continue;
        }
        // ⚠⚠ AND AN ANSWERED ROW CLEARS IT, which is what makes the field *the newest row was a
        // refusal* rather than *this session was refused at some point* — true for ever of a
        // session that has since been working happily for an hour.
        spend.refused = None;
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
                // ⚠ NOTHING REFUSED THESE ROWS — register item 988, and this is the CONTROL for
                // that field: a record of ordinary answered requests must read as *the service is
                // answering*, or a run would treat every turn as an outage.
                refused: None,
            },
        );
    }

    /// ⛔⛔⛔⛔⛔ **A SESSION THE SERVICE REFUSED IS NOT A SESSION THAT WROTE NOTHING** — register
    /// item 988, and the zero that cost a run.
    ///
    /// # ⛔⛔⛔⛔⛔ Both arms off ONE record, because the fact is a LEVEL
    ///
    /// A refusal is billed for nothing (`input 0 · output 0 · cache_read 0`), so it looks exactly
    /// like an idle session to [`Spend::produced`] — and register item 878 replaces a session that
    /// produced nothing and then fails the run. Run 270 died that way while its peer was being told
    /// *"You've hit your weekly limit"* once per prompt.
    ///
    /// So the arms below assert, off one walk: the refusal is CARRIED with its words, and an
    /// answered row after it **clears** the level. A reader that only ever set it would report a
    /// session that has been working for an hour as refused; one that never set it is today's
    /// defect.
    ///
    /// ⚠⚠ **THE CONTROL IS THE SAME ROWS WITHOUT THE FLAG** — `a_streamed_reply_counts_once` reads
    /// `refused: None` off ordinary answered rows, which is what says this arm is about the flag and
    /// not about zeros.
    #[test]
    fn a_row_the_service_refused_is_carried_with_its_words_and_cleared_by_the_next_answer() {
        // ⚠ THE REAL SHAPE, cut from run 270's own record: `isApiErrorMessage` at the TOP level,
        // `<synthetic>` as the model, and a usage block of zeros that still carries a cache read —
        // which is what makes the row reach the billed-request path at all.
        let refused = r#"
{"type":"assistant","isApiErrorMessage":true,"message":{"id":"msg_1","model":"<synthetic>","content":[{"type":"text","text":"You've hit your weekly limit · resets Sep 11, 6am (Asia/Seoul)"}],"usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}
"#;
        let said = spend_in(refused);
        assert_eq!(
            said.refused.as_deref(),
            Some("You've hit your weekly limit · resets Sep 11, 6am (Asia/Seoul)"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 988: the row says the service refused it and says what it \
             said, and a run that reads only the zero kills a session that was never allowed to \
             answer. Run 270 did exactly that, three prompts in a row",
        );
        assert_eq!(
            (said.requests, said.produced),
            (0, 0),
            "⛔⛔⛔ AND IT IS NOT A BILLED REQUEST — this field's own word. Nothing was charged, so \
             counting it would take `cold` or `floor` off a row that was never paid for",
        );

        // ⛔⛔⛔⛔⛔ THE SHARP ARM: a refusal must not DISTURB a reading that already happened. The
        // levels (`context`, `cached`) are taken from the newest row, and a refusal's zeros
        // arriving there would report a session whose accumulated context had dropped to nothing —
        // wiping the number every restart decision is priced on, one row after it was measured.
        let measured = crate::testing::MEASURED_HERE.transcript();
        let then_refused = format!("{measured}\n{}", refused.trim());
        let (before, after) = (spend_in(&measured), spend_in(&then_refused));
        assert_eq!(
            (
                after.requests,
                after.context,
                after.cached,
                after.produced,
                after.cold,
                after.floor
            ),
            (
                before.requests,
                before.context,
                before.cached,
                before.produced,
                before.cold,
                before.floor
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 988: a refused row must leave every number exactly as the last \
             ANSWERED row left it. {before:?} then {after:?}",
        );
        assert!(
            after.refused.is_some() && before.refused.is_none(),
            "⚠⚠ and the only difference the refusal makes is that it is REPORTED — otherwise this \
             arm would be green for a reader that ignored the row entirely",
        );

        // ⚠⚠ AND AN ANSWER CLEARS IT: run 270's session was answered 45 minutes later, and a run
        // reading the refusal for ever after would be as wrong in the other direction.
        let recovered = format!(
            "{then_refused}\n{}",
            r#"{"type":"assistant","message":{"id":"msg_9","usage":{"input_tokens":2,"cache_read_input_tokens":27536,"cache_creation_input_tokens":0,"output_tokens":146}}}"#
        );
        let back = spend_in(&recovered);
        assert_eq!(
            back.refused, None,
            "⛔⛔⛔ AN ANSWERED ROW CLEARS IT. Held instead of overwritten, this would say *refused* \
             about every later turn of a session that recovered — the same fossil defect one field \
             over",
        );
        assert_eq!(
            (back.requests, back.produced - before.produced),
            (before.requests + 1, 146),
            "⚠ and the answered row is counted, so nothing about the counting was lost on the way",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A REFUSAL THAT CARRIED NO WORDS IS STILL A REFUSAL** — register item 988, and the
    /// one thing [`Spend::refused`]'s doc insists on that nothing was measuring.
    ///
    /// # ⛔⛔⛔⛔⛔ The hatch was measured OPEN, which is why this exists
    ///
    /// The field's doc says it outright — *the [`Option`] answers `was it refused` and the
    /// [`String`] answers `what it said`*, so a wordless refusal is `Some("")` and never [`None`].
    /// Measured 2026-09-09 by making exactly that substitution (an `Option::filter` dropping the
    /// empty text): **all 611 tests of this crate's lib stayed green.** The sentence was prose, and
    /// prose does not hold a hatch shut.
    ///
    /// ⇒ What the open hatch cost, in this field's own terms: [`None`] means *the newest row was
    /// ANSWERED*. A service refusing without a quotable sentence would have read as a service
    /// answering, the turn would have gone back to being [`Made::Nothing`](crate::outer::Made),
    /// and register item 878 would replace the session and fail the run — which is the entire
    /// defect this field was added to close, surviving inside its own repair.
    ///
    /// ⚠⚠ **THE ASSERTION IS ON THE [`Option`] AND NOT ON THE TEXT**, because those are the two
    /// different questions: `Some("")` and `Some("You've hit your weekly limit")` are ONE answer to
    /// *was it refused* and two answers to *what did it say*. A run with no sentence to quote says
    /// so; it does not fall silent about the refusal.
    ///
    /// ⚠ Three shapes rather than one, because the words go missing three ways and a reader that
    /// handled only the shape it was written against would leave the other two answering `None`.
    #[test]
    fn a_refusal_that_carried_no_words_is_still_a_refusal_and_not_an_answer() {
        // ⚠ EVERY ARM IS RUN 270's ROW with only its `content` changed — the flag, the synthetic
        // model and the all-zero usage are held fixed, so the one thing that can move the answer is
        // the absence of a sentence.
        for (shape, row) in [
            (
                "no content key at all",
                r#"{"type":"assistant","isApiErrorMessage":true,"message":{"id":"msg_1","model":"<synthetic>","usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#,
            ),
            (
                "content holding no text block",
                r#"{"type":"assistant","isApiErrorMessage":true,"message":{"id":"msg_1","model":"<synthetic>","content":[{"type":"thinking"}],"usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#,
            ),
            (
                "a text block that is only whitespace",
                r#"{"type":"assistant","isApiErrorMessage":true,"message":{"id":"msg_1","model":"<synthetic>","content":[{"type":"text","text":"   "}],"usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#,
            ),
        ] {
            let said = spend_in(row);
            assert_eq!(
                said.refused.as_deref(),
                Some(""),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 988: a refusal with {shape} must still READ as a refusal, \
                 with no words to quote. `None` here says the service ANSWERED this row — the one \
                 thing this field exists to deny — and hands the turn straight back to the silence \
                 rule that killed run 270",
            );
        }

        // ⚠⚠ AND THE CONTROL, or the arms above would be green for a reader that called every row
        // refused: the same row WITHOUT the flag is an ordinary answered request, and it is the
        // flag that separates them rather than the empty content.
        let unflagged = r#"{"type":"assistant","message":{"id":"msg_1","usage":{"input_tokens":1,"cache_read_input_tokens":50,"output_tokens":7}}}"#;
        let answered = spend_in(unflagged);
        assert_eq!(
            (answered.refused, answered.requests, answered.produced),
            (None, 1, 7),
            "⚠⚠ THE CONTROL: an answered row reads as answered and is BILLED, so this gate is \
             about `isApiErrorMessage` and not about a build that called every row a refusal",
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
                refused: None,
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
