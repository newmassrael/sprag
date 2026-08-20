//! **WHAT A RUN'S ENDED SESSIONS DID OVER AND OVER** — the arithmetic `context_review.scxml` decides
//! on, kept apart from the machine that decides.
//!
//! # ⚠⚠⚠ The division of labour is the document's, not this module's idea
//!
//! `context_review.scxml` says it in its own header, and it is the arrangement `ai_loop.scxml`
//! already uses for a judged dialog:
//!
//! ```text
//!   the driver   opens the records, counts commands and line ranges, and
//!                publishes totals as event data
//!   this file    when to look, how many repeats are too many, what to ask,
//!                where the answer may be written
//! ```
//!
//! So **no threshold is written here.** [`habits_in`] takes the limit it is given; a default in this
//! file would be a policy nobody can read, which is the thing the split exists to prevent.
//!
//! # ⚠⚠⚠ COUNT ACTS, NOT NAMES — measured, and it corrected the design twice
//!
//! The document records what counting the wrong unit cost. Counting reads by FILE PATH said 45% of a
//! session was re-reading; the same session counted by LINE RANGE was 18%, because a 2,600-line file
//! read in eleven pieces is eleven reads of one path and **nothing repeated at all**. What survived
//! is the count that was measured rather than assumed: **618 Bash calls, 44% of them repeats**, and
//! the top five all one thing — *is the build finished yet?*
//!
//! So a Bash act is its whole COMMAND. Two calls are the same act when the string a shell would run
//! is the same string, and never merely because both were `Bash`.
//!
//! # ⚠⚠ A record is deduplicated by message id, and skipping that multiplies every count
//!
//! A streamed reply appears in the record many times and every fragment repeats the whole envelope —
//! usage and content both. [`crate::spend::spend_in`] carries the same rule for the same reason:
//! counting rows rather than messages multiplies a session's tool calls by however long its answers
//! were. **A repeat count taken off rows would be an artefact of verbosity.**
//!
//! # What this module deliberately does NOT decide
//!
//! ⚠ Whether a repeat is BAD. `git status` twenty times is not the same finding as tailing one log
//! eighty-four times, and this cannot tell them apart — which is why the document sends the
//! candidates to a model and asks for one line, and why the model *never chooses which candidates
//! there are*.

use std::sync::Arc;

use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};
use serde_json::Value;

use crate::sm::context_review_sm::{ContextReviewEvent, ContextReviewPolicy, ContextReviewState};

/// **ONE ACT A SESSION DID MORE THAN ONCE**, and how many times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repeat {
    /// The act itself — for a shell call, the whole command a shell would run.
    ///
    /// ⚠ It is the act and not a label for it. A reader deciding whether eighty-four of these
    /// mattered needs the thing that was actually run; a digest would answer *something was
    /// repeated* and leave nobody able to say what to do instead.
    pub act: String,
    /// How many distinct messages issued it.
    pub times: u64,
}

/// **WHAT ONE ENDED SESSION'S RECORD SAYS ABOUT ITSELF** — everything counted, nothing summarised.
///
/// # ⚠⚠⚠ Why this keeps the parts rather than a verdict
///
/// The whole value of a review is that a LATER one can be compared with it. A structure that kept
/// only *"three habits found"* fixes, at the moment of least knowledge, which questions can ever be
/// asked of this session again — and the session's pane is gone, so nothing can go back and count
/// differently. Keeping the parts is what makes *did it get better?* a question with an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counted {
    /// Distinct messages that issued at least one tool call.
    pub messages: u64,
    /// Calls this build could NAME an act for, deduplicated by message — the numerator of
    /// [`repeats`](Self::repeats).
    pub calls: u64,
    /// **CALLS WHOSE ACT THIS BUILD CANNOT NAME**, kept rather than dropped.
    ///
    /// ⚠⚠⚠ They are not folded into [`calls`](Self::calls), and that is the whole point of the
    /// field. An unnamed call has no act to be the same as or different from, so counting it would
    /// add to the numerator of a repeat rate while adding nothing to its denominator — every
    /// session using a tool this module cannot name would report repetition it never did.
    ///
    /// ⚠ A large number here is a finding about THIS MODULE, not about the session: it means a run
    /// spent its calls on tools whose repetition nobody is counting.
    pub unnamed: u64,
    /// Distinct acts seen — the denominator a repeat rate is taken against.
    pub acts: u64,
    /// Acts issued at least the caller's limit of times, **most repeated first**.
    ///
    /// ⚠ Empty is a real answer and the one a healthy session gives. See `context_review.scxml`'s
    /// `count.none`, which is an exit rather than a failure.
    pub repeated: Vec<Repeat>,
}

impl Counted {
    /// How many calls were spent re-issuing an act this record had already issued.
    ///
    /// ⚠ It counts every act's calls beyond its first, over ALL acts and not only the ones that met
    /// a limit — the rate the document's own measurement quotes (*44% of 618*) is this one, and a
    /// rate taken over the reported candidates alone would answer a different question while looking
    /// like the same number.
    #[must_use]
    pub const fn repeats(&self) -> u64 {
        self.calls.saturating_sub(self.acts)
    }
}

/// The tools whose act is their COMMAND.
const BY_COMMAND: &[&str] = &["Bash", "BashOutput"];

/// The tools whose act is a FILE AND THE PART OF IT that was asked for.
const BY_RANGE: &[&str] = &["Read"];

/// **COUNT WHAT A RECORD DID MORE THAN ONCE**, reporting acts issued at least `limit` times.
///
/// `limit` is the caller's — see the module docs, and `context_review.scxml`'s `repeat_limit`, where
/// the judgement lives and is argued.
///
/// ⚠ A `limit` of 0 or 1 reports every act, which is a listing rather than a finding. It is not
/// refused: the document owns that number, and a driver second-guessing it would be the second
/// author this split exists to avoid.
#[must_use]
pub fn habits_in(text: &str, limit: u64) -> Counted {
    let mut seen: Vec<String> = Vec::new();
    let mut acts: Vec<(String, u64)> = Vec::new();
    let mut counted = Counted::default();

    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        // ⚠⚠⚠ ONE MESSAGE, ONE COUNT — see the module docs. A streamed reply repeats its whole
        // content array, so a record read row-by-row multiplies every act by the length of the
        // answer that carried it.
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

        let blocks = row
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut issued = false;
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(name) = block.get("name").and_then(Value::as_str) else {
                continue;
            };
            // ⚠ The message issued a call whatever this build can make of it, so `messages` counts
            // it either way. Only the act-bearing half reaches `calls` — see [`Counted::unnamed`].
            issued = true;
            let Some(act) = act_of(name, block.get("input")) else {
                counted.unnamed += 1;
                continue;
            };
            counted.calls += 1;
            if let Some(entry) = acts.iter_mut().find(|(known, _)| *known == act) {
                entry.1 += 1;
            } else {
                acts.push((act, 1));
            }
        }
        if issued {
            counted.messages += 1;
        }
    }

    counted.acts = acts.len() as u64;
    // ⚠ Most repeated first, and ties broken by the act itself so the same record always answers the
    // same list. A review whose order moved between runs would make two readings look different when
    // nothing had changed.
    acts.retain(|(_, times)| *times >= limit);
    acts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counted.repeated = acts
        .into_iter()
        .map(|(act, times)| Repeat { act, times })
        .collect();
    counted
}

/// **WHAT A CALL COUNTS AS** — the act, not the tool.
///
/// # ⚠⚠⚠ There is no fallback to the tool's NAME, and the absence is the design
///
/// Naming an unrecognised call after its tool is the measured error this whole module exists
/// downstream of: every `Read` in a session becomes one act repeated fifty times, and the review
/// reports re-reading that never happened — 45% where the truth was 18%. A fallback cannot fail
/// loudly, because a wrong act still counts, still sorts, and still reads like a finding.
///
/// So an unrecognised tool answers [`None`] and is counted as
/// [`Counted::unnamed`](Counted::unnamed) — visible, and excluded from a rate it would corrupt.
///
/// ⚠⚠ A `Read` is its FILE AND RANGE together. Eleven reads of one 2,600-line file at eleven
/// different offsets are eleven acts and no repetition at all; eleven reads of the same window are
/// one act repeated, and that is a finding. Counting the path alone cannot tell those apart, and
/// the pair is the smallest thing that can.
///
/// ⚠ An absent `offset`/`limit` is the whole file, and is spelled so — `-` rather than `0`, because
/// a read that asked for no window and a read that asked for offset 0 are different requests and a
/// shared spelling would merge them.
fn act_of(name: &str, input: Option<&Value>) -> Option<String> {
    if BY_COMMAND.contains(&name) {
        let command = input?.get("command")?.as_str()?.trim();
        return (!command.is_empty()).then(|| command.to_owned());
    }
    if BY_RANGE.contains(&name) {
        let input = input?;
        let path = input.get("file_path")?.as_str()?.trim();
        if path.is_empty() {
            return None;
        }
        let part = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_u64)
                .map_or_else(|| "-".to_owned(), |at| at.to_string())
        };
        return Some(format!("{path}#{}+{}", part("offset"), part("limit")));
    }
    None
}

// ── the driver ──────────────────────────────────────────────────────────────────────────────────

/// **WHAT ONE ENDED SESSION'S RECORD WAS MEASURED TO BE.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewed {
    /// The name the session filed its record under — the door, kept so a later reader can open the
    /// same transcript and count something this build did not think to.
    ///
    /// ⚠ Read off the RECORD's own file name and not off what anybody called the session: those
    /// two came apart in the field, and only the first opens anything (register item 431).
    pub identity: String,
    /// What it did more than once. See [`Counted`].
    pub counted: Counted,
    /// What it was charged to read, or [`None`] where the record carried no billed request.
    ///
    /// ⚠ This is the axis a turn count cannot stand in for: what one request adds to the context is
    /// 861 tokens at the median and 633,749 at the maximum, so *how many turns* predicts *how much
    /// context* not at all. `cache_read` is written by the agent on every request and is exact.
    pub context: Option<u64>,
}

/// **HOW A REVIEW ENDED** — the document's three finals, kept apart because the parent may care.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// A line for the next session, from `carried`.
    Carried(String),
    /// The review ran and found nothing worth naming — `nothing`, and the answer a healthy run
    /// should give most of the time.
    Nothing,
    /// It could not finish. ⚠ Not the same as [`Nothing`](Self::Nothing): one is an answer and the
    /// other is the absence of one, and a caller that merged them could not tell a clean run from a
    /// broken reviewer.
    Failed,
}

/// **A WHOLE REVIEW** — its ending, and everything counted on the way there.
///
/// ⚠⚠⚠ THE COUNTS ARE HERE EVEN WHEN THE ENDING CARRIES NOTHING. That is the difference between a
/// review and a suggestion box: `Nothing` with three sessions measured is the reading a later
/// comparison needs, and discarding it would keep only the reviews that found fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// How it ended.
    pub ending: Ending,
    /// The state that gave up, for an ending that gave up — `"reading"`, `"counting"` or
    /// `"asking"`. The document has ONE `nothing` final reached from three places, so without this
    /// a reader cannot tell *no records* from *no habits* from *no usable answer*.
    pub gave_up_at: Option<&'static str>,
    /// One entry per record opened, in the order the run closed them.
    pub sessions: Vec<Reviewed>,
}

impl Review {
    /// How many candidates met the document's limit, over every session measured.
    #[must_use]
    pub fn habits(&self) -> u64 {
        self.sessions
            .iter()
            .map(|session| session.counted.repeated.len() as u64)
            .sum()
    }

    /// The ledger line this review leaves behind — one JSON object, one line.
    ///
    /// ⚠ Everything counted, nothing summarised: see [`Reviewed`]. A digest would be smaller and
    /// would make the next comparison impossible, which is the only thing the file is for.
    #[must_use]
    pub fn ledger_line(&self) -> String {
        let sessions: Vec<Value> = self
            .sessions
            .iter()
            .map(|session| {
                serde_json::json!({
                    "identity": session.identity,
                    "messages": session.counted.messages,
                    "calls": session.counted.calls,
                    "unnamed": session.counted.unnamed,
                    "acts": session.counted.acts,
                    "repeats": session.counted.repeats(),
                    "context": session.context,
                    "repeated": session.counted.repeated.iter().map(|repeat| {
                        serde_json::json!({ "act": repeat.act, "times": repeat.times })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::json!({
            "ending": match &self.ending {
                Ending::Carried(_) => "carried",
                Ending::Nothing => "nothing",
                Ending::Failed => "failed",
            },
            "gave_up_at": self.gave_up_at,
            "carry": match &self.ending {
                Ending::Carried(line) => Some(line.as_str()),
                _ => None,
            },
            "habits": self.habits(),
            "sessions": sessions,
        })
        .to_string()
    }
}

/// **WHERE A BARE LEDGER NAME LIVES** — under `state`, the directory the driver was HANDED.
///
/// ⚠ Mechanism, not policy: `context_review.scxml` decides whether to keep the counts and what to
/// call them, and this decides where a machine's state goes — the same split
/// [`crate::spend::record_of`] already makes when it resolves an agent's own record under
/// `$HOME/.claude/projects` from a name the agent chose.
///
/// # ⚠⚠⚠⚠⚠ Why the directory is a PARAMETER, where this crate used to read `$XDG_STATE_HOME`
///
/// The document says a bare name is resolved *"against the daemon's state directory"*, and this
/// resolved it against the AMBIENT one — which is the daemon's only when a daemon is what is
/// running. Under `cargo test` it is **the home of whoever ran the suite**.
///
/// ⚠⚠ Measured 2026-08-19, not argued: one `cargo test -p sprag-plugin --lib` appended THIRTY
/// lines to `$XDG_STATE_HOME/sprag/context-review.jsonl`. Twenty-nine came from [`crate::outer`]
/// gates that walk `reviewing` and have no idea this file exists — so *"the test that forgot"* was
/// never the shape of it, and CI's `ambient-home-guard` had been red on exactly this.
///
/// **A library that resolves the ambient environment behind its caller decides something that is
/// not its to decide, and no call site can opt out of it.** So the directory is handed in — by
/// [`crate::AiLoopSpec::review_ledger`], which a daemon fills from its own state directory — and
/// [`None`] keeps nothing. The suite cannot write a home this crate can no longer name.
fn ledger_path(named: &str, state: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    if named.is_empty() {
        return None;
    }
    let named = std::path::Path::new(named);
    if named.is_absolute() {
        return Some(named.to_path_buf());
    }
    Some(state?.join(named))
}

/// A run of `context_review.scxml` against one run's closed sessions.
///
/// # ⚠⚠⚠ Why this is a TOP-LEVEL machine and not the `<invoke>` the debt register expected
///
/// Measured in this tree, against the generated code rather than argued: an invoked child engine is
/// stored in a PRIVATE field of its parent's policy (`child_probe`), and `Engine::policy()` answers
/// a shared reference. So nothing outside the parent can read an invoked child's state or raise an
/// event on it.
///
/// That is fatal here and harmless for `probe_child.scxml`, which is the difference worth writing
/// down: the probe SENDS ITSELF the event it transitions on, so it needs nobody. This document does
/// not — `<send event="read.begin"/>` is a MARKER for a driver, exactly as `ai_loop.scxml`'s
/// `<send event="prompt.turn"/>` is, and this workspace's rule is that **a machine instructs its
/// driver through its STATE**. A driver that cannot see the state cannot answer it, and the review
/// would sit in `reading` for ever — taking the loop's every session replacement down with it.
///
/// ⚠⚠ WHAT THE `<invoke>` WOULD HAVE GIVEN AND THIS OWES A GATE INSTEAD: the parent cancelling the
/// child by leaving the state. Here the caller owns this value, so dropping it is that cancellation
/// — enforced by ownership rather than by the engine, which is weaker until something holds it.
pub struct ContextReview {
    machine: Engine<ContextReviewPolicy>,
    script: Arc<dyn IScriptEngine>,
    session: String,
    /// **THE STATE DIRECTORY A BARE `ledger_into` RESOLVES UNDER**, or [`None`] to keep nothing.
    ///
    /// ⚠ Held rather than read from the environment, and [`ledger_path`] carries the whole reason.
    state: Option<std::path::PathBuf>,
}

impl ContextReview {
    /// A review over the compiled document, keeping its counts under `state` — or [`None`] when
    /// the document's script session cannot be opened, or when opening it raised an error the
    /// document itself never got to answer.
    ///
    /// ⚠⚠ `state` is where a BARE `ledger_into` lands, which is the shipped document's; [`None`]
    /// keeps nothing at all. An absolute `ledger_into` is honoured whatever this says. This
    /// module's own `ledger_path` is where the whole argument for a parameter over an ambient read
    /// is written down.
    ///
    /// ⚠⚠⚠ **OPENED THROUGH [`crate::document::opened`]** — register item 505. A review whose own
    /// start-up failed used to come back a working review with an empty datamodel, and the loop
    /// would then act on habits it never counted. The document answers the errors its four states
    /// can raise (`_event.data.records` on a `read.done` carrying no data is the reachable one); the
    /// door answers what is raised before any state can. [`None`] rather than a sentence, because a
    /// review is optional to the run in a way a kind is not — the loop carries on unreviewed, which
    /// is the same answer it gives for a record it cannot read.
    #[must_use]
    pub fn new(script: Arc<dyn IScriptEngine>, state: Option<std::path::PathBuf>) -> Option<Self> {
        let machine =
            crate::document::opened(ContextReviewPolicy::new(Arc::clone(&script))).ok()?;
        let session = machine.policy().session_id.clone()?;
        Some(Self {
            state,
            machine,
            script,
            session,
        })
    }

    /// **THE SCRIPT SESSION THE DOCUMENT'S OWN `<data>` LIVES IN** — a fixture door, and labelled
    /// one because clippy proved it is: nothing in this crate's library code calls it.
    ///
    /// # ⚠⚠ Why a door only a test uses is nevertheless the right one here
    ///
    /// This workspace's rule is that *a door the fixture can reach is not a door production uses*,
    /// and the danger it names is a fixture that BYPASSES the product. This bypasses nothing: the
    /// only thing reached through it is `ledger_into`, set to the same absolute path an author
    /// would write into `context_review.scxml`, on the route that file documents. The gate then
    /// exercises exactly the production path.
    ///
    /// ⚠ The alternative was worse and was rejected: pointing the review somewhere harmless by
    /// setting `XDG_STATE_HOME` mutates process-global state that this crate's other tests run
    /// concurrently with, and a green suite that depends on which test got there first is not a
    /// measurement. The honest cost of writing into a real user's state directory during a test is
    /// not payable either.
    ///
    /// ⚠ It is `cfg(test)` rather than `allow(dead_code)` so that the day production wants it, the
    /// attribute has to come off and somebody has to read this.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    /// A number the document authored, or `fallback` when it holds none this can read.
    ///
    /// ⚠ A missing number is NOT defaulted silently into a policy of the driver's own: the fallback
    /// is passed by the caller at the one call site, so the value a reader has to find is in this
    /// file next to the question rather than buried in a getter.
    fn number(&self, name: &str, fallback: u64) -> u64 {
        match self.script.get_variable(&self.session, name) {
            Ok(ScriptValue::Int(held)) if held >= 0 => held.unsigned_abs(),
            Ok(ScriptValue::Double(held)) if held >= 0.0 => held as u64,
            _ => fallback,
        }
    }

    /// A string the document authored, or empty.
    fn text(&self, name: &str) -> String {
        match self.script.get_variable(&self.session, name) {
            Ok(ScriptValue::String(held)) => held,
            _ => String::new(),
        }
    }

    /// **RUN THE REVIEW OVER `ended`** — the RECORDS this run has closed, oldest first.
    ///
    /// The document decides how far back to look and how many repeats are too many; this opens the
    /// records, counts, and answers each state's effect. See the module docs for the split.
    ///
    /// # ⚠⚠⚠⚠ Why this takes paths where it used to take names
    ///
    /// It resolved each record from the session NAME through [`crate::spend::record_of`], and that
    /// is a derivation: the name a pane was launched with is not necessarily the one its agent
    /// filed under. Register item 431 measured the gap — a pane born `--session-id 97f5ffd9-…`
    /// wrote as `3f4ffa52-…`, and no record of the first name exists anywhere — so a review of a
    /// run's own closed sessions opened NOTHING and reported `reading` as the state it gave up in.
    /// The caller now knows which file each session was writing (its agent said so), so there is
    /// nothing left here to guess.
    ///
    /// ⚠ `asking` is answered `ask.none` — a second agent is not wired here yet, and a review that
    /// invented a line would be worse than one that carries none. The counts still land.
    pub fn run(&mut self, ended: &[std::path::PathBuf]) -> Review {
        let look_back = self.number("look_back", 3) as usize;
        let limit = self.number("repeat_limit", 8);

        // ⚠ The MOST RECENT `look_back`, not the first: a review is about what the run has been
        // doing lately, and a run long enough to need one has closed more sessions than it looks at.
        let records: Vec<&std::path::PathBuf> = ended.iter().rev().take(look_back).rev().collect();

        let mut sessions: Vec<Reviewed> = Vec::new();
        let mut gave_up_at = None;
        let mut ending = Ending::Failed;

        // ⚠⚠ BOUNDED. The document is small and every path to a final is short, so a walk that has
        // not ended by here is a machine that is not going to — and a driver that looped for ever
        // on it would hang the run that asked, which is the failure this whole shape exists to
        // avoid.
        for _ in 0..64 {
            match self.machine.get_current_state() {
                ContextReviewState::Reading => {
                    for record in &records {
                        let Ok(text) = std::fs::read_to_string(record) else {
                            continue;
                        };
                        sessions.push(Reviewed {
                            // ⚠ READ OFF THE RECORD'S OWN NAME rather than taken from the caller:
                            // the file is what the agent filed, so its stem is the identity the
                            // agent actually used — which is the one a later reader needs to open
                            // it again, and not necessarily the one the pane was launched with.
                            identity: record
                                .file_stem()
                                .map(|stem| stem.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            counted: habits_in(&text, limit),
                            context: Some(crate::spend::spend_in(&text))
                                .and_then(|spend| (spend.requests > 0).then_some(spend.context)),
                        });
                    }
                    if sessions.is_empty() {
                        gave_up_at = Some("reading");
                        self.raise(ContextReviewEvent::ReadNone, &Value::Null);
                    } else {
                        self.raise(
                            ContextReviewEvent::ReadDone,
                            &serde_json::json!({ "records": sessions.len() }),
                        );
                    }
                }
                ContextReviewState::Counting => {
                    // ⚠ The counting already happened, in `reading` — one pass over each record
                    // rather than two. What this state decides is whether anything MET the limit,
                    // which is the document's question and not the arithmetic's.
                    let habits: u64 = sessions
                        .iter()
                        .map(|session| session.counted.repeated.len() as u64)
                        .sum();
                    if habits == 0 {
                        gave_up_at = Some("counting");
                        self.raise(ContextReviewEvent::CountNone, &Value::Null);
                    } else {
                        self.raise(
                            ContextReviewEvent::CountDone,
                            &serde_json::json!({ "habits": habits }),
                        );
                    }
                }
                ContextReviewState::Asking => {
                    gave_up_at = Some("asking");
                    self.raise(ContextReviewEvent::AskNone, &Value::Null);
                }
                ContextReviewState::Writing => {
                    // The ADVICE, to the path the author named — empty by default, and then this
                    // is a review that reports and changes no file.
                    let into = self.text("write_into");
                    self.raise(
                        if into.is_empty() {
                            ContextReviewEvent::WriteSkipped
                        } else {
                            ContextReviewEvent::WriteDone
                        },
                        &Value::Null,
                    );
                }
                ContextReviewState::Carried => {
                    ending = Ending::Carried(self.text("carry"));
                    break;
                }
                ContextReviewState::Nothing => {
                    ending = Ending::Nothing;
                    break;
                }
                ContextReviewState::Failed => {
                    ending = Ending::Failed;
                    break;
                }
            }
            self.machine.step();
        }

        let review = Review {
            ending,
            gave_up_at,
            sessions,
        };
        self.keep(&review);
        review
    }

    /// Raise `event` on the machine and let the document route it.
    fn raise(&mut self, event: ContextReviewEvent, data: &Value) {
        self.machine.raise_external(event, &data.to_string(), "");
    }

    /// **KEEP THE COUNTS**, whatever the ending was — see `ledger_into`.
    ///
    /// ⚠ A failure to write is not a failure of the review. The counts are for a later comparison,
    /// and a run that could not open its state directory has still done its work; turning that into
    /// a `fail` would stop a loop over a file nobody was waiting on.
    fn keep(&self, review: &Review) {
        let Some(path) = ledger_path(&self.text("ledger_into"), self.state.as_deref()) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            return;
        };
        use std::io::Write as _;
        let _ = writeln!(file, "{}", review.ledger_line());
    }
}

#[cfg(test)]
mod ledger_tests {
    use std::sync::Arc;

    use sce_rust_runtime::{IScriptEngine, ScriptValue};

    use super::{ContextReview, Ending, ledger_path};

    /// ⚠⚠⚠ **A REVIEW THAT FOUND NOTHING STILL LEAVES ITS COUNTS BEHIND** — the one property that
    /// makes *did this get better?* a question with an answer.
    ///
    /// # Why this is the gate and not «the advice was written»
    ///
    /// `nothing` is the exit a HEALTHY run takes, and it is the majority of readings. A ledger
    /// written only on `carried` would retain exactly the reviews that found fault and discard
    /// every one that found none — so the file would show a run's problems and never its progress,
    /// and two readings could not be compared because only one kind was kept.
    ///
    /// ⚠ The destination is set through the DOCUMENT's own `ledger_into`, on the absolute-path
    /// route that file documents. There is no test-only door here: this is how a caller who wants
    /// the counts somewhere particular says so.
    #[test]
    fn a_review_that_carries_nothing_still_keeps_what_it_counted() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        // ⚠ NO STATE DIRECTORY, and the `ledger_into` below is absolute, so what this measures is
        // the route a caller who names a file takes — see [`ledger_path`].
        let mut review =
            ContextReview::new(Arc::clone(&lua), None).expect("the document opens a session");

        let into = std::env::temp_dir()
            .join("sprag-review-gate")
            .join("a_review_that_carries_nothing_still_keeps_what_it_counted.jsonl");
        let _ = std::fs::remove_file(&into);
        lua.set_variable(
            review.session(),
            "ledger_into",
            ScriptValue::String(into.to_string_lossy().into_owned()),
        )
        .expect("the document's own knob is writable");

        // A run that has closed no sessions — the honest state of every run's FIRST review, since
        // `ended` is written by the replacement this state runs before.
        let answered = review.run(&[]);

        assert_eq!(
            (answered.ending.clone(), answered.gave_up_at),
            (Ending::Nothing, Some("reading")),
            "⚠⚠ with no closed session there is no record to open, so the document's `read.none` \
             exit is the right one — and `gave_up_at` is what tells it from «found no habit», \
             which the single `nothing` final cannot. Got {answered:?}",
        );

        let kept = std::fs::read_to_string(&into).unwrap_or_else(|why| {
            panic!(
                "⚠⚠⚠ THE COUNTS MUST LAND EVEN WHEN THE ENDING CARRIES NOTHING. A review that \
                 measures and discards can report the same thing for ever and nobody can ask \
                 whether it is better than last time — which makes «improve the loop's own \
                 context» a sentence with no measurement under it. Reading {into:?}: {why}"
            )
        });
        let lines: Vec<&str> = kept.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "⚠ one review, one line — the file is APPENDED to so that two readings sit next to \
             each other. Got {lines:?}",
        );
        let row: serde_json::Value =
            serde_json::from_str(lines[0]).expect("⚠ the ledger line must be one JSON object");
        assert_eq!(
            (row["ending"].as_str(), row["gave_up_at"].as_str()),
            (Some("nothing"), Some("reading")),
            "⚠⚠ and the line must say which ending it was and where it gave up, or a later reader \
             cannot tell a clean run from a reviewer that could open nothing. Got {row}",
        );

        // ⚠ APPENDED, not truncated: the second review must not erase the first, which is the
        // whole arrangement — a file with only the newest reading answers no comparison at all.
        review.run(&[]);
        let kept = std::fs::read_to_string(&into).expect("the ledger is still there");
        assert_eq!(
            kept.lines().count(),
            2,
            "⚠⚠⚠ a second review must APPEND. Truncating leaves exactly one reading, and one \
             reading can never be compared with anything",
        );
        let _ = std::fs::remove_file(&into);
    }

    /// ⚠⚠⚠⚠⚠ **A REVIEW THAT WAS HANDED NO STATE DIRECTORY WRITES NOTHING ANYWHERE** — the
    /// property that keeps this whole suite out of the home of whoever ran it.
    ///
    /// # Why the shipped document alone cannot decide this
    ///
    /// `context_review.scxml` authors `ledger_into` as a BARE name (`context-review.jsonl`), which
    /// is right: an author names the file, and cannot know which machine's state directory it
    /// belongs under. [`ledger_path`] used to answer that question by reading `$XDG_STATE_HOME`
    /// itself — so *the daemon's state directory* silently meant *the ambient one*, and under
    /// `cargo test` there is no daemon. **Measured 2026-08-19: `cargo test -p sprag-plugin --lib`
    /// appended THIRTY lines to `$XDG_STATE_HOME/sprag/context-review.jsonl`**, twenty-nine of them
    /// from [`crate::outer`] gates that walk `reviewing` and never heard of this file. That is the
    /// write CI's `ambient-home-guard` had been failing on, and no test could have caught it: the
    /// variable is process-global, so a test can neither observe nor isolate what its neighbours do
    /// to it — which is why that guard is a separate process and this gate is about the SEAM.
    ///
    /// ⚠⚠ So the subject here is an ABSENCE, and it is asserted the only way an absence can be:
    /// against a directory this gate owns and can therefore prove is still empty. A version that
    /// re-read the environment fails it — [`None`] is the only thing that can mean *keep nothing*
    /// once the crate has no second place to look.
    #[test]
    fn a_review_handed_no_state_directory_keeps_its_counts_nowhere() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut review =
            ContextReview::new(Arc::clone(&lua), None).expect("the document opens a session");

        // The shipped `ledger_into` is left exactly as the document authors it — a bare name — so
        // this is the run every caller who says nothing about a ledger gets.
        let bare = review.text("ledger_into");
        assert_eq!(
            bare, "context-review.jsonl",
            "⚠⚠⚠ THE PREMISE OF THIS GATE IS THE SHIPPED DOCUMENT'S OWN DEFAULT. If the author \
             stopped authoring a bare name, the write this measures cannot happen any more and \
             this gate would be quietly measuring nothing. Got {bare:?}",
        );

        let answered = review.run(&[]);
        assert_eq!(
            answered.ending,
            Ending::Nothing,
            "⚠ the review itself still runs and answers — keeping no counts is not failing",
        );

        // ⚠ The question a bare name is resolved against, asked directly: with nothing handed in
        // there is no path at all, which is what stops the write before a directory is even made.
        assert_eq!(
            ledger_path(&bare, None),
            None,
            "⚠⚠⚠⚠⚠ A BARE LEDGER NAME WITH NO STATE DIRECTORY MUST RESOLVE TO NOWHERE. Anything \
             else is this crate choosing a home on its caller's behalf, and the home it chose was \
             the developer's: thirty lines of somebody's `~/.local/state` per suite run.",
        );
    }

    /// ⚠⚠⚠⚠ **AND THE COUNTS DO LAND WHEN A RUN NAMES WHERE THEY GO** — the other half, without
    /// which *keep nothing* would be a fix that simply broke the feature.
    ///
    /// ⚠⚠ This is the DAEMON's route, not a fixture's: `sprag-host` passes its own
    /// `durability::state_dir()` as [`crate::AiLoopSpec::review_ledger`], the document's bare
    /// `ledger_into` is left standing, and the file lands under the directory that was handed in.
    /// A gate that only proved the absence above would pass just as well against a `keep` that had
    /// been deleted.
    #[test]
    fn a_bare_ledger_name_lands_under_the_state_directory_the_run_was_given() {
        let state = std::env::temp_dir().join(format!("sprag-review-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut review = ContextReview::new(Arc::clone(&lua), Some(state.clone()))
            .expect("the document opens a session");

        review.run(&[]);

        // ⚠ The document's own file name, under the caller's directory — neither half invented
        // here. A driver that joined its own name would land somewhere this assertion is not.
        let landed = state.join("context-review.jsonl");
        let kept = std::fs::read_to_string(&landed).unwrap_or_else(|why| {
            panic!(
                "⚠⚠⚠⚠ THE COUNTS MUST LAND UNDER THE DIRECTORY THE RUN NAMED. A daemon says where \
                 its state lives exactly once, and if that does not reach the ledger then the \
                 review measures a run nobody can compare with the next one. Reading {landed:?}: \
                 {why}"
            )
        });
        assert_eq!(
            kept.lines().count(),
            1,
            "⚠⚠ one review, one line. Got {kept:?}",
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    /// ⚠⚠⚠⚠⚠ **A REVIEW OPENS THE RECORD IT IS HANDED, WITHOUT DERIVING A PATH FROM A NAME** —
    /// register item 431 reaching its second reader.
    ///
    /// # ⚠⚠⚠⚠ What the derived road did to this state, and why no gate saw it
    ///
    /// This resolved each closed session through `crate::spend::record_of`, which searches
    /// `$HOME/.claude/projects` for `<name>.jsonl`. The name a run holds is the one its pane was
    /// LAUNCHED with, and an agent may file under another: measured 2026-08-17, a pane born as
    /// session `97f5ffd9` reported `3f4ffa52`, and no `97f5ffd9` record exists anywhere. So every
    /// door this state was handed opened onto nothing — `sessions` empty, `gave_up_at: "reading"` —
    /// **which is exactly what the gate above asserts for a run that has closed NO sessions at
    /// all.** The healthy state and the broken one are the same reading, and that is why the defect
    /// could sit here unseen.
    ///
    /// ⚠⚠ So this gate's subject is the OTHER answer: a review handed one real record must open it,
    /// report what it cost, and name the session by **the record's own file name** rather than by
    /// anything a caller called it.
    #[test]
    fn a_review_opens_the_record_it_was_handed_rather_than_one_derived_from_a_name() {
        /// One closed session's record: two billed requests and a repeated act, so both halves of a
        /// review — what it cost and what it did twice — have something to find.
        const WRITTEN: &str = concat!(
            r#"{"type":"assistant","message":{"id":"m1","usage":{"input_tokens":0,"#,
            r#""cache_read_input_tokens":100,"cache_creation_input_tokens":0,"output_tokens":1},"#,
            r#""content":[{"type":"tool_use","name":"Read","input":{"file_path":"/a"}}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"m2","usage":{"input_tokens":0,"#,
            r#""cache_read_input_tokens":466013,"cache_creation_input_tokens":0,"output_tokens":1},"#,
            r#""content":[{"type":"tool_use","name":"Read","input":{"file_path":"/a"}}]}}"#,
        );

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        // ⚠⚠ NO STATE DIRECTORY AND NO `ledger_into` OF ITS OWN, which is this gate's subject only
        // in passing and was a real write until 2026-08-19: the shipped `ledger_into` is a BARE
        // name, so this line used to append to `$XDG_STATE_HOME/sprag/context-review.jsonl` — the
        // home of whoever ran the suite. It keeps nothing now. See [`ledger_path`].
        let mut review =
            ContextReview::new(Arc::clone(&lua), None).expect("the document opens a session");

        let home = std::env::temp_dir().join(format!("sprag-review-record-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("a directory to file the record in");
        // ⚠ NOT under `$HOME/.claude/projects`, which is the point: a reader that still derived a
        // path from this stem would search a tree this file is not in and find nothing.
        let record = home.join("3f4ffa52-what-the-agent-filed.jsonl");
        std::fs::write(&record, WRITTEN).expect("a closed session's record");

        let answered = review.run(std::slice::from_ref(&record));
        let _ = std::fs::remove_dir_all(&home);

        assert_eq!(
            answered.sessions.len(),
            1,
            "⚠⚠⚠⚠ THE RECORD IS RIGHT THERE AND THE REVIEW MUST OPEN IT. Zero sessions here is \
             the reading a run that closed NOTHING gives, which is how this went unnoticed for as \
             long as the path was derived. Got {answered:?}",
        );
        assert_eq!(
            answered.sessions[0].context,
            Some(466_013),
            "⚠⚠⚠ and it must report what that session was charged on its last request — the axis \
             a turn count cannot stand in for. Got {:?}",
            answered.sessions[0],
        );
        assert_eq!(
            answered.sessions[0].identity, "3f4ffa52-what-the-agent-filed",
            "⚠⚠ and it names the session by THE RECORD'S OWN FILE NAME, which is the identity the \
             agent actually filed under — the only one that opens this file again. Got {:?}",
            answered.sessions[0],
        );
        assert_ne!(
            answered.gave_up_at,
            Some("reading"),
            "⚠⚠⚠ a review that opened a record did not give up in `reading`: that exit means \
             THERE WAS NOTHING TO OPEN, and reporting it here is the defect wearing the healthy \
             run's clothes. Got {answered:?}",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Counted, habits_in};

    /// One assistant message issuing `calls`, all under the same message `id`.
    fn message(id: &str, calls: &[&str]) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","content":[{}]}}}}"#,
            calls.join(",")
        )
    }

    fn shell(command: &str) -> String {
        format!(r#"{{"type":"tool_use","name":"Bash","input":{{"command":"{command}"}}}}"#)
    }

    fn read(path: &str, offset: Option<u64>, limit: Option<u64>) -> String {
        let part = |at: Option<u64>, key: &str| {
            at.map_or_else(String::new, |at| format!(r#","{key}":{at}"#))
        };
        format!(
            r#"{{"type":"tool_use","name":"Read","input":{{"file_path":"{path}"{}{}}}}}"#,
            part(offset, "offset"),
            part(limit, "limit"),
        )
    }

    fn acts(counted: &Counted) -> Vec<(String, u64)> {
        counted
            .repeated
            .iter()
            .map(|repeat| (repeat.act.clone(), repeat.times))
            .collect()
    }

    /// ⚠⚠⚠ **A STREAMED REPLY REPEATS ITS WHOLE ENVELOPE, AND COUNTING ROWS MULTIPLIES EVERY ACT.**
    ///
    /// This is the defect [`crate::spend::spend_in`] carries the same rule against, and it is worth
    /// its own gate because it fails in the direction that LOOKS like a finding: a session whose
    /// answers were long reports repetition proportional to how much it wrote. Delete the
    /// `seen`/`id` check and this record — one command, issued once, in a reply that streamed three
    /// times — reports three calls and a repeat.
    #[test]
    fn a_streamed_reply_does_not_multiply_what_it_repeats() {
        let one = message("msg-1", &[shell("cargo test").as_str()]);
        let text = format!("{one}\n{one}\n{one}");

        let counted = habits_in(&text, 2);

        assert_eq!(
            (counted.messages, counted.calls, counted.acts),
            (1, 1, 1),
            "⚠⚠⚠ three rows of ONE message are one message, one call and one act. Counting rows \
             would make a session's repeat count a measure of how long its answers were rather \
             than of what it did twice. Got {counted:?}",
        );
        assert_eq!(
            counted.repeats(),
            0,
            "⚠⚠⚠ and nothing was repeated: the command was issued once. A non-zero rate here is \
             the verbosity artefact reported as a habit. Got {counted:?}",
        );
    }

    /// ⚠⚠⚠ **A SHELL ACT IS ITS COMMAND.** Two different commands are two acts however alike their
    /// tool is; the same command twice is the finding.
    #[test]
    fn a_shell_act_is_its_command_and_not_its_tool() {
        let text = [
            message("m1", &[shell("cargo test").as_str()]),
            message("m2", &[shell("git status").as_str()]),
            message("m3", &[shell("cargo test").as_str()]),
        ]
        .join("\n");

        let counted = habits_in(&text, 2);

        assert_eq!(
            counted.acts, 2,
            "⚠⚠⚠ two distinct commands are two acts. Counting by TOOL collapses them into one \
             `Bash` repeated three times, which is the shape of the error this module is built \
             against. Got {counted:?}",
        );
        assert_eq!(
            acts(&counted),
            vec![("cargo test".to_owned(), 2)],
            "⚠⚠ only the command issued twice meets a limit of two, and it is reported as the \
             command itself — a reader deciding what to do instead needs the thing that was run. \
             Got {counted:?}",
        );
    }

    /// ⚠⚠⚠ **THE 45%-VERSUS-18% ERROR, AS A GATE.** A large file read in pieces is many acts and no
    /// repetition; the same window read twice is one act repeated. Counting the PATH cannot tell
    /// those apart, and answering `Read` for both is worse still.
    #[test]
    fn a_read_is_its_file_and_its_range_together() {
        let pieces = [
            message("m1", &[read("/big.rs", Some(1), Some(500)).as_str()]),
            message("m2", &[read("/big.rs", Some(501), Some(500)).as_str()]),
            message("m3", &[read("/big.rs", Some(1001), Some(500)).as_str()]),
        ]
        .join("\n");

        let counted = habits_in(&pieces, 2);

        assert_eq!(
            (counted.acts, counted.repeats()),
            (3, 0),
            "⚠⚠⚠ eleven reads of one file at eleven offsets are eleven acts and NOTHING repeated \
             — the measurement that corrected this design. A file-path act reports this session as \
             two-thirds re-reading when it re-read nothing. Got {counted:?}",
        );
        assert!(
            counted.repeated.is_empty(),
            "⚠⚠ and no candidate is carried, so the review answers `count.none` — the exit a \
             healthy session should take. Got {counted:?}",
        );

        let twice = [
            message("m1", &[read("/big.rs", Some(1), Some(500)).as_str()]),
            message("m2", &[read("/big.rs", Some(1), Some(500)).as_str()]),
        ]
        .join("\n");

        assert_eq!(
            acts(&habits_in(&twice, 2)),
            vec![("/big.rs#1+500".to_owned(), 2)],
            "⚠⚠⚠ the SAME window twice is the finding the range exists to keep visible. A design \
             that dropped ranges to avoid the false positive above would lose this with it",
        );
    }

    /// ⚠⚠⚠ **A CALL THIS BUILD CANNOT NAME IS KEPT, AND KEPT OUT OF THE RATE.**
    ///
    /// The tempting shortcut is to name it after its tool. That is silent and wrong: every `Edit`
    /// becomes one act repeated, and a session that edited forty different files is reported as
    /// having done the same thing forty times.
    #[test]
    fn a_call_this_build_cannot_name_is_counted_apart() {
        let edit = r#"{"type":"tool_use","name":"Edit","input":{"file_path":"/a.rs"}}"#;
        let text = [
            message("m1", &[edit]),
            message("m2", &[edit]),
            message("m3", &[edit]),
        ]
        .join("\n");

        let counted = habits_in(&text, 2);

        assert_eq!(
            (counted.unnamed, counted.calls, counted.acts),
            (3, 0, 0),
            "⚠⚠⚠ three unnameable calls are three `unnamed`, no `calls` and no acts. Folding them \
             into `calls` gives `repeats() == 3` for a session that repeated nothing this module \
             can even see. Got {counted:?}",
        );
        assert_eq!(
            counted.repeats(),
            0,
            "⚠⚠⚠ THE RATE MUST NOT MOVE ON CALLS THAT HAVE NO ACT. This is the arithmetic the \
             separate field exists to protect. Got {counted:?}",
        );
        assert_eq!(
            counted.messages, 3,
            "⚠ but the messages are still counted: they DID issue calls, and a reader comparing \
             `unnamed` against `messages` is how the gap in this module's own coverage becomes \
             visible instead of looking like a quiet session. Got {counted:?}",
        );
    }

    /// ⚠⚠⚠ **THE LIMIT IS THE DOCUMENT'S AND THIS MODULE HOLDS NO OPINION.** The same record
    /// answers differently at two limits — which is what makes `repeat_limit` a number somebody can
    /// change in `context_review.scxml` and see the effect of.
    #[test]
    fn the_limit_is_the_callers_and_the_same_record_answers_differently() {
        let text = [
            message("m1", &[shell("tail log").as_str()]),
            message("m2", &[shell("tail log").as_str()]),
            message("m3", &[shell("tail log").as_str()]),
        ]
        .join("\n");

        assert_eq!(
            acts(&habits_in(&text, 3)),
            vec![("tail log".to_owned(), 3)],
            "⚠⚠ three issues meet a limit of three",
        );
        assert!(
            habits_in(&text, 4).repeated.is_empty(),
            "⚠⚠⚠ and do not meet a limit of four. A threshold baked into this module would make \
             the document's `repeat_limit` decorative — the exact 'policy nobody can read' that \
             `context_review.scxml` splits driver from document to prevent",
        );
        assert_eq!(
            habits_in(&text, 4).repeats(),
            2,
            "⚠⚠⚠ AND THE RATE IS INDEPENDENT OF THE LIMIT. `repeats()` is taken over ALL acts, so \
             a run that reported no candidates still knows what it spent re-issuing — the \
             document's own 44%-of-618 is this number and not a count of candidates",
        );
    }
}
