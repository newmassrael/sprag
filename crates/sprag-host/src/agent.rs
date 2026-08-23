//! The daemon's agent-state memory: one [`Tracker`] per pane, and the manifest list they share.
//!
//! H3 slice 3. [`sprag_detect`] answers from one screen and one title, and [`Tracker`] adds what a
//! single frame cannot know (the settle window, the quiescence gate, the identity a modal covers).
//! This module is where those trackers LIVE, and the three reasons it is here rather than anywhere
//! more obvious are all facts about this daemon rather than preferences:
//!
//! * **Not on `sprag_terminal::Pane`.** The producer crate depends on the emulator and the PTY and
//!   nothing else, on purpose; `sprag-grid` — the pure-read crate H3's design cites as its exact
//!   precedent — is likewise consumed on this side of that line and never by the producer. A
//!   detector is a scene fact.
//! * **Not on [`WorkspaceExternal`](crate::WorkspaceExternal).** That external is rebuilt for every
//!   JSON-RPC request, so a map held as one of its fields would be born empty and dropped per poll.
//!   The hysteresis would be inert while every individual verdict still looked right, which is the
//!   worst available failure: silent, and invisible to any test that reads one verdict.
//! * **Keyed by [`PaneId`] alone**, which is sound because every window's `Workspace` in a daemon
//!   draws ids from ONE shared counter — so an id names a pane across the whole daemon, not within a
//!   session. That is what lets one registry serve every session and lets a pane keep its memory
//!   across a break/join.
//!
//! # Who drives it
//!
//! Two callers, one tracker, no second path to a verdict:
//!
//! * The **pane-list query** observes every pane it is about to describe, inside the workspace lock
//!   it already holds. That is what makes a verdict resting on present evidence — a dialog — reach a
//!   person on the output that painted it.
//! * The **settle waker** observes the panes whose [`Tracker::pending_deadline`] has passed. Without
//!   it, a verdict resting on an ABSENCE could never confirm itself: the pane list is served when a
//!   client asks, a client asks when the scene revision moves, and the last thing to move it was the
//!   output that STOPPED. See `sprag-term`'s waker for the whole argument.
//!
//! Both calls go through [`AgentRegistry::observe`], so there is one arbitration and one memory. The
//! second call in a pass is a no-op by construction — the quiescence gate skips it, and a pending
//! candidate's window is measured from when it was first seen rather than from the last look — and
//! `two_observations_of_one_unchanged_pane_publish_once` asserts that rather than trusting it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sprag_detect::{Choice, Hysteresis, Question, Report, ReportOutcome, Ruleset, Tracker};
use sprag_terminal::PaneId;
use sprag_vt::Screen;

use crate::external::lock;

/// How often the settle waker SWEEPS: discover panes nobody has asked about, bring a manifest edit
/// to the panes it invalidates, and forget the panes that are gone.
///
/// Five seconds bounds how long after a pane's birth its state can be unknown to a caller who never
/// asks twice — the daemon's durability snapshot uses the same period for the analogous reason, and
/// the two are deliberately unlinked because one bounds a loss window and this one bounds a
/// discovery window.
///
/// # Why it lives HERE and not beside the loop that reads it
///
/// It was `sprag-term`'s private constant until R260 measured what a sweep costs, at which point
/// `sprag-latency` needed the same number to say what fraction of a period the work occupies — and
/// one binary cannot see another's private item, so the alternative was a second spelling of five
/// seconds with a comment asking the next person not to let them drift. A cadence the agent
/// subsystem's own module documents belongs to the subsystem.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Trackers visited by a nearest-deadline scan, process-wide.
static DEADLINE_VISITS: AtomicU64 = AtomicU64::new(0);

/// What this module's deadline bookkeeping has cost the process so far.
///
/// # Why a counter, for a walk that is obviously short
///
/// [`AgentRegistry::next_deadline`] answers a question about the WHOLE registry, so it visits every
/// tracker. That is correct and it is cheap once. It stopped being cheap when it was called from
/// [`AgentClock::observe`], which the pane list calls once per pane: N looks each walking N entries
/// is 2N^2 tracker visits per client wake. R255 measured the term rather than arguing about it —
/// 2.70 to 3.35 ns per remembered pane per look, linear in the entry count against a control that
/// ruled out cache locality — and R256 removed it by asking the O(1) question instead.
///
/// The removal changes NO answer, which is exactly the class R255 recorded as needing a count: an
/// exact optimisation has no behavioural observable, so nothing would go red the day somebody
/// restores the tidier-looking whole-registry read. `sprag_grid::work` and `sprag_detect::work` are
/// the same instrument for the same reason. The number is monotonic and process-wide; read it twice
/// and take the DELTA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentWork {
    /// Trackers visited by a nearest-deadline scan.
    ///
    /// The scans that remain are the settle waker's, and there are TWO per wake rather than the one
    /// this said before R260 counted them: [`AgentClock::park_until_due`] reads the registry to
    /// choose how long to sleep, and the loop reads it again on waking to decide whether anything is
    /// actually due. The second cannot be dropped — a candidate appearing is exactly what cuts the
    /// sleep short, so the answer from before the park is stale after it. Per wake, not per pane,
    /// which is the distinction that matters and the one the pane list violated.
    pub deadline_visits_total: u64,
}

/// Read the meter. See [`AgentWork`] for why the answer is only meaningful as a delta.
#[must_use]
pub fn work() -> AgentWork {
    AgentWork {
        deadline_visits_total: DEADLINE_VISITS.load(Ordering::Relaxed),
    }
}

/// One pane's published agent state, in the shape the pane list puts on the wire.
///
/// Produced only for a pane with a KNOWN state: [`AgentRegistry::observe`] returns `None` where the
/// verdict is `Unknown`, so D8's additive rule — the key is absent entirely for a pane no manifest
/// claims — is carried by the type instead of by a caller remembering to check. A workspace with no
/// agents cannot accidentally grow a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    /// `working` / `blocked` / `idle`, from [`sprag_detect::AgentState::wire_str`] — never a
    /// spelling invented here.
    pub state: &'static str,
    /// Which manifest claims the pane, `None` while a rule fired but no manifest is identified.
    pub agent: Option<String>,
    /// Which rule fired. D7: a gate that cannot say what it saw cannot be diagnosed, and this is
    /// what `explain` reads — it is the same value the detector produced, not a recomputation.
    pub rule: Option<String>,
    /// Increments on a published CHANGE, so a client tells "still blocked" from "blocked again"
    /// without diffing strings — `notification_seq`'s treatment.
    pub seq: u64,
    /// **HOW MANY QUESTIONS THIS PANE HAS BEEN ASKED**, counted on the agent's own statement — see
    /// [`sprag_detect::Tracker::asked_seq`] for why [`seq`](Self::seq) cannot answer it.
    ///
    /// ⚠⚠⚠ It is the fact a SUPERVISOR needs and no other reader does: *did the peer take the
    /// question I just typed?* A submit that arrives while the pane is already `working` moves no
    /// verdict, so `seq` stands still and the supervisor has no way to tell its own prompt from the
    /// silence it was typed into — register item 441, where that cost a live loop thirty-three
    /// turns against an agent that was working the whole time.
    pub asked_seq: u64,
    /// **HOW MANY ANSWERS THIS PANE'S AGENT HAS STATED** — [`asked_seq`](Self::asked_seq)'s other
    /// end, counted the same way and needed for the same kind of reason.
    ///
    /// ⚠⚠⚠ It is what DATES a statement to a turn. A supervisor snapshots it when it asks and reads
    /// [`said`](Self::said) as this turn's answer only once it has moved — without which the text
    /// standing in the tracker could be the previous turn's, which is precisely the confusion
    /// register item 441 is made of at the other end.
    pub said_seq: u64,
    /// **HOW MANY REPORTS THIS PANE HAS ACCEPTED**, whatever they said — register item 458.
    ///
    /// ⚠⚠⚠⚠ The one counter that moves while a turn is merely WORKING: the three above it stand
    /// still through a turn calling tool after tool, which is the same reading as a turn that was
    /// interrupted and will never report again. Measured — a pane read `working seq=6 asked=2
    /// said=0` for fourteen minutes after an Escape, and a driver polled it toward a 24-hour clock.
    pub reports: u64,
    /// WHO said so, when a process inside the pane reported it rather than a rule inferring it —
    /// `None` for a scraped verdict.
    ///
    /// The counterpart of [`rule`](Self::rule) for the other kind of evidence, and additive for the
    /// same reason: a client that knows nothing about reports reads exactly the pre-report shape. A
    /// reported verdict carries no `rule` and a scraped one carries no `source`, so which authority
    /// answered is never a guess.
    pub source: Option<String>,
    /// **WHICH BUILD THE REPORTER IS**, when it said — see [`crate::wire::AGENT_BUILD_KEY`] for the
    /// hazard this exists to make visible.
    ///
    /// # ⚠⚠⚠⚠ Three cases, and a surface that renders two of them has re-introduced the defect
    ///
    /// The reporter is a SEPARATE PROCESS that any `cargo build` replaces under a running daemon, so
    /// *"is this reporter my image?"* is a live question after every rebuild. This carries the raw
    /// answer rather than a verdict, deliberately: the daemon's own [`crate::wire::BUILD`] is the
    /// other half of the comparison and every reader already has it, so publishing the FACT lets a
    /// reader judge — the same treatment [`source`](Self::source) gets, and the reason neither is a
    /// bool.
    ///
    /// * `Some(b)` where `b == crate::wire::BUILD` — the reporter is this daemon's image.
    /// * `Some(b)` otherwise — **it is not**, and `b` names what it is.
    /// * `None` — **it did not say.** NOT agreement. Every reporter older than the key answers this,
    ///   and so does a person typing `sprag report-agent`.
    ///
    /// ⚠ Additive on the pane row exactly as `source` is: absent for a pane whose verdict was
    /// scraped, and absent for a reporter that says nothing, so a workspace of shells is
    /// byte-identical to the shape before it existed.
    pub reporter_build: Option<String>,
    /// WHAT THE PANE IS ASKING, for a pane this look found `blocked` and whose menu this build
    /// could read — `None` for every other pane.
    ///
    /// # ⚠⚠⚠ Why the registry carries it rather than each caller re-deriving it
    ///
    /// The question is a fact about THE SCREEN THIS VERDICT WAS REACHED ON, and the only place both
    /// are in hand at once is inside [`observe`](AgentRegistry::observe). A caller that reads the
    /// state here and parses the menu itself is reading two moments and calling them one — and it
    /// cannot even be given the chance, because the pane list builds this map under the workspace
    /// guard and renders it after the guard has dropped, with no screen left to consult. That is
    /// exactly why the pane-level surface published `blocked` and no question for four rounds while
    /// the RUN surface published both: the run path had a screen and the pane path did not.
    ///
    /// Deriving it once here also removes the duplication that had already appeared —
    /// [`crate::plugins`]'s own observation re-read the same screen for the same parse — so the two
    /// surfaces cannot come to disagree about what a pane is asking.
    ///
    /// ⚠ Only computed for a `blocked` verdict, which is what keeps it off the hot path: a settled
    /// workspace of working and idle panes pays nothing, and a blocked pane pays one bottom-anchored
    /// parse of a screen that is already mapped.
    pub asking: Option<Question>,
    /// **THE LAST PROMPT THE AGENT ITSELF SAID IT WAS ASKED**, carried from its own submit hook.
    ///
    /// Beside [`asking`](Self::asking) and the opposite kind of fact: that one is what THIS BUILD
    /// read off a screen, and this one is what the AGENT said. A supervisor confirming that its
    /// question arrived can only use the second — a screen renders text a run delivered and text a
    /// composer already held as the same pixels.
    pub asked: Option<String>,
    /// **THE LAST ANSWER THE AGENT ITSELF SAID IT GAVE**, carried from the hook that ends a turn.
    ///
    /// ⚠⚠⚠⚠ The reason this exists is a measurement rather than a preference (register item 441):
    /// a full-screen agent repaints in place, so its pane's logical-line addresses stop advancing
    /// and *what did this turn print* answers `0` for the rest of the session — while the agent
    /// goes on answering, and a person reading the pane sees every reply. The words are on the
    /// screen and cannot be read OFF it; the program has them.
    ///
    /// ⚠ Undated on its own — pair it with [`said_seq`](Self::said_seq).
    pub said: Option<String>,
    /// **WHY THE AGENT ITSELF SAID IT WANTS A PERSON**, carried from the hook it raises to ask.
    ///
    /// ⚠⚠⚠⚠ [`asking`](Self::asking)'s other kind, and it answers precisely where that one gives up:
    /// `asking` is `None` on a blocked pane whose question this build could not parse as a numbered
    /// menu, and until this field existed that pane's whole account was *something is wrong, go
    /// look*. The peer had said what it wanted, in the payload that produced the word `blocked`
    /// (register item 452).
    ///
    /// ⚠ NOT undated the way [`said`](Self::said) is, and needs no counter: the tracker replaces it
    /// on every report rather than carrying it, so a sentence standing here belongs to the report in
    /// force. See `sprag_detect::Report::noticed`.
    pub noticed: Option<String>,
    /// **WHERE THE AGENT SAID IT IS WRITING ITS TRANSCRIPT** — stated, never derived from an id.
    pub transcript: Option<String>,
    /// **WHEN THIS VERDICT CHANGES WITH NO FURTHER OUTPUT** — [`Tracker::pending_deadline`] as it
    /// stood on the look that produced these facts, and `None` when nothing is pending.
    ///
    /// # ⚠⚠⚠ THE ONE FIELD HERE THAT IS NOT ON THE WIRE, and it cannot be
    ///
    /// An [`Instant`] is meaningless outside this process — it is not a wall clock and has no
    /// serialisation — so this rides on the struct the pane list is built from without ever
    /// reaching the pane list. That is stated rather than left to be discovered, because the type's
    /// own headline says *in the shape the pane list puts on the wire* and this is the exception.
    ///
    /// ⚠⚠ **IT IS HERE RATHER THAN BEHIND A SECOND CALL** because the alternative loses the race
    /// it exists to win: `AgentRegistry::pending_deadline(id)` is public and one hash lookup, and a
    /// caller that observed and THEN asked would hold a deadline fresher than its own verdict.
    /// Published from the same borrow of the same tracker, the pair cannot disagree.
    ///
    /// Register item 630. Spent by `sprag_plugin::run::park_until` through
    /// `sprag_plugin::AgentObservation::settles_at`: a wait whose predicate rests on this verdict
    /// used to poll the whole settle window at ~200 screen reads a change, and now takes one look
    /// at the instant named here.
    pub settles_at: Option<Instant>,
}

/// A [`Question`] in the shape BOTH surfaces put it on the wire — the ONE renderer, so a pane's
/// `asking` and a run's cannot come to differ.
///
/// The keys are [`crate::wire`]'s (see [`crate::wire::ASKING_KEY`] for why they are shared), and
/// nothing is invented here: `asked` is the parser's lines and each choice is its number, its label
/// and whether the agent's own marker is on it. A caller reads one shape whichever surface it came
/// from.
#[must_use]
pub fn question_json(question: &Question) -> serde_json::Value {
    serde_json::json!({
        crate::wire::ASKED_KEY: question.asked,
        crate::wire::CHOICES_KEY: question
            .choices
            .iter()
            .map(|choice| serde_json::json!({
                crate::wire::CHOICE_NUMBER_KEY: choice.number,
                crate::wire::CHOICE_LABEL_KEY: choice.label,
                crate::wire::CHOICE_SELECTED_KEY: choice.selected,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The [`Question`] behind [`question_json`], read back — the parse a client outside this process
/// needs, written HERE so the renderer and the reader are one edit apart.
///
/// ⚠⚠ A menu with no `choices` array is not a question: the parser's own rule is that ONE numbered
/// line is a prompt echo, so a shape carrying none of them has nothing a caller could answer and
/// reads as absent rather than as an empty menu.
fn question_of(value: &serde_json::Value) -> Option<Question> {
    let choices: Vec<Choice> = value[crate::wire::CHOICES_KEY]
        .as_array()?
        .iter()
        .filter_map(|choice| {
            Some(Choice {
                number: u32::try_from(choice[crate::wire::CHOICE_NUMBER_KEY].as_u64()?).ok()?,
                label: choice[crate::wire::CHOICE_LABEL_KEY].as_str()?.to_owned(),
                // ABSENT is `false` — no marker seen — and never *"assume this one"*: a caller told
                // the wrong option is where a bare Enter lands cannot tell a consent from an
                // accident.
                selected: choice[crate::wire::CHOICE_SELECTED_KEY]
                    .as_bool()
                    .unwrap_or(false),
            })
        })
        .collect();
    if choices.is_empty() {
        return None;
    }
    Some(Question {
        asked: value[crate::wire::ASKED_KEY]
            .as_array()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(|line| line.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        choices,
    })
}

/// **ONE PANE'S AGENT VERDICT, IN THE SHAPE EVERY SURFACE PUBLISHES IT** — the object the
/// [`PANES_SLOT`](crate::wire::PANES_SLOT) entry carries under `agent` and the whole answer of
/// [`AGENT_FIELD`](crate::wire::AGENT_FIELD).
///
/// # ⚠⚠⚠⚠⚠ ONE BUILDER, because the listing and the address must not drift
///
/// The pane list built this inline until register item 557 gave the verdict an address of its own.
/// A second literal would have been a second answer to the same question, differing first in
/// whichever key one of them forgot — and the reader that has to parse it back
/// ([`verdict_of`]) sits directly below, so a key added here and nowhere else is one edit
/// from being visible rather than a round.
///
/// # What is always present, and what is present only when it was stated
///
/// The four counters and the state are ALWAYS written: zero is a real answer for each of them
/// (*nothing has ever been asked here*), and an absent key would be read as an older daemon instead
/// of as the fact. Everything else is additive — present only where somebody said it — so a pane
/// nobody has reported about is byte-identical to the wire shape before each key existed.
#[must_use]
pub fn verdict_json(facts: &AgentFacts) -> serde_json::Value {
    let mut value = serde_json::json!({
        crate::wire::AGENT_STATE_KEY: facts.state,
        crate::wire::AGENT_SEQ_KEY: facts.seq,
        crate::wire::AGENT_ASKED_SEQ_KEY: facts.asked_seq,
        crate::wire::AGENT_SAID_SEQ_KEY: facts.said_seq,
        // ⚠⚠⚠⚠⚠ THE COUNTER THAT MOVES WHILE A TURN IS MERELY WORKING — register item 458. The
        // three above stand still through a turn calling tool after tool, which reads exactly like
        // a turn nothing will ever end; this one is the peer's reporter being alive. It reached
        // `AgentFacts` when 458 was paid and reached no wire until 557, so every out-of-process
        // supervisor was telling a slow peer from a dead one without it.
        crate::wire::AGENT_REPORTS_KEY: facts.reports,
    });
    if let Some(name) = &facts.agent {
        value[crate::wire::AGENT_NAME_KEY] = serde_json::json!(name);
    }
    if let Some(rule) = &facts.rule {
        value[crate::wire::AGENT_RULE_KEY] = serde_json::json!(rule);
    }
    // WHO said so, for a verdict that was REPORTED rather than inferred — `rule`'s counterpart on
    // the other kind of evidence. A reported verdict carries no rule and a scraped one carries no
    // source, so a reader never has to guess which authority answered.
    if let Some(source) = &facts.source {
        value[crate::wire::AGENT_SOURCE_KEY] = serde_json::json!(source);
    }
    // WHICH BUILD that reporter is, when it said. ABSENT means the reporter did not say, never that
    // it matches (`crate::wire::AGENT_BUILD_KEY`).
    if let Some(build) = &facts.reporter_build {
        value[crate::wire::AGENT_BUILD_KEY] = serde_json::json!(build);
    }
    // WHAT THE PANE IS ASKING — the question, its options, and which one a bare Enter would take.
    // ⚠⚠ Its ABSENCE on a `blocked` pane is a claim too: this daemon looked and could not read a
    // menu there. The remedy is a person.
    if let Some(question) = &facts.asking {
        value[crate::wire::ASKING_KEY] = question_json(question);
    }
    // ⚠⚠ The agent's own account of the turn — what it was asked, what it answered, and why it
    // wants a person. Carried through untouched: this layer states, and the reader judges.
    if let Some(asked) = &facts.asked {
        value[crate::wire::AGENT_ASKED_KEY] = serde_json::json!(asked);
    }
    if let Some(said) = &facts.said {
        value[crate::wire::AGENT_SAID_KEY] = serde_json::json!(said);
    }
    if let Some(noticed) = &facts.noticed {
        value[crate::wire::AGENT_NOTICED_KEY] = serde_json::json!(noticed);
    }
    // WHERE IT WRITES, stated rather than derived from a session id — the derivation that was
    // measured answering `0` for a transcript that existed (register item 431).
    if let Some(transcript) = &facts.transcript {
        value[crate::wire::AGENT_TRANSCRIPT_KEY] = serde_json::json!(transcript);
    }
    value
}

/// WHAT A READER LEARNS FROM THE VERDICT AT AN ADDRESS — three answers, because two of them used to
/// be one and the collapse was register item 564.
///
/// # ⚠⚠⚠⚠⚠ *Not an agent* and *a word I cannot spell* are not the same fact
///
/// A `null` means the daemon looked and no manifest claims that pane: **carry on, it is a shell.**
/// A state word this build's vocabulary does not hold means the daemon is AHEAD of this driver:
/// nothing here can say what that pane is doing, and the honest instruction is **ask a person** —
/// the same one [`PaneAccess::supervision`](sprag_plugin::PaneAccess::supervision) answering `None`
/// carries. Reading the second as the first is how a supervisor concludes *"a shell"* about a pane
/// running an agent it has never heard of, and it goes live the day a daemon and a driver are
/// different builds — which is the ordinary state after any rebuild here (register item 412).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The daemon looked and no manifest claims this pane. **Carry on.**
    NotAnAgent,
    /// A verdict this build can read.
    Seen(Box<sprag_plugin::AgentObservation>),
    /// A verdict in a word this build does not know, carried VERBATIM so a person can be told which
    /// one. **A skew, not an absence.**
    Unspellable(String),
}

/// [`verdict_json`] read back — see [`Verdict`] for why the answer has three arms.
///
/// ⚠⚠ The authority is DERIVED from which of the two evidence keys is present, exactly as the
/// in-process source derives it: a reported verdict carries [`AGENT_SOURCE_KEY`](crate::wire) and a
/// scraped one carries [`AGENT_RULE_KEY`](crate::wire). A shape carrying neither is a scrape whose
/// rule went unnamed, which is what `Scraped { rule: None }` means.
#[must_use]
pub fn verdict_of(value: &serde_json::Value) -> Verdict {
    let Some(word) = value[crate::wire::AGENT_STATE_KEY].as_str() else {
        // No state at all — a `null`, or a shape carrying no verdict. Not an agent.
        return Verdict::NotAnAgent;
    };
    let Some(state) = sprag_detect::AgentState::from_wire(word) else {
        // ⚠⚠⚠⚠⚠ NEVER a fallback variant. A supervisor handed a guessed state would act on a
        // verdict nobody made; handed the WORD, a person can see which build is ahead.
        return Verdict::Unspellable(word.to_owned());
    };
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    Verdict::Seen(Box::new(sprag_plugin::AgentObservation {
        state,
        agent: text(crate::wire::AGENT_NAME_KEY),
        authority: match text(crate::wire::AGENT_SOURCE_KEY) {
            Some(source) => sprag_plugin::Authority::Reported { source },
            None => sprag_plugin::Authority::Scraped {
                rule: text(crate::wire::AGENT_RULE_KEY),
            },
        },
        // ⚠ A MISSING counter reads as zero, which is this wire's own rule for it: the four are
        // always written, so an absent one is an older daemon — and *nothing has happened yet* is
        // the reading that makes a supervisor wait rather than conclude.
        seq: value[crate::wire::AGENT_SEQ_KEY].as_u64().unwrap_or(0),
        asked_seq: value[crate::wire::AGENT_ASKED_SEQ_KEY]
            .as_u64()
            .unwrap_or(0),
        said_seq: value[crate::wire::AGENT_SAID_SEQ_KEY].as_u64().unwrap_or(0),
        reports: value[crate::wire::AGENT_REPORTS_KEY].as_u64().unwrap_or(0),
        asking: question_of(&value[crate::wire::ASKING_KEY]),
        asked: text(crate::wire::AGENT_ASKED_KEY),
        said: text(crate::wire::AGENT_SAID_KEY),
        noticed: text(crate::wire::AGENT_NOTICED_KEY),
        transcript: text(crate::wire::AGENT_TRANSCRIPT_KEY),
        // ⚠⚠⚠⚠⚠ **`Unknown`, AND SPELLING IT `Nothing` WOULD PLANT A LOST WAKEUP** — register
        // items 630 and 631. This verdict comes off a WIRE that carries no deadline: the daemon's
        // tracker may well have a candidate publishing two seconds from now, and nothing readable
        // here says so. `Settling::Nothing` is a CLAIM — *park on the pane and look no more* — and
        // a driver that believed it from here would sleep straight through the publish it was
        // waiting for.
        //
        // ⚠⚠ It costs the old rate and no more: `RemotePaneAccess` publishes no
        // `PaneChanges` either (item 631), so every wait over it asks again each slice regardless.
        // The day that surface becomes parkable, THIS is the line that has to move first — and
        // until it does, the type is what stops the defect being silent.
        settling: sprag_plugin::Settling::Unknown,
    }))
}

/// Every pane's agent-state memory, plus the one ruleset they are all evaluated against.
#[derive(Debug, Default)]
pub struct AgentRegistry {
    /// Compiled ONCE per edit of the user's config, not per evaluation. A caller that rebuilt this
    /// on the hot path would recompile every pattern of every agent on a path served once per client
    /// wake, which is why [`sprag_detect::built_ins`] is a named list rather than a literal at each
    /// use and why [`crate::config::AgentManifests`] holds the file rather than reading it.
    rules: Ruleset,
    trackers: HashMap<PaneId, Tracker>,
    /// Why the ruleset above is not the one the user's file declares, if it is not — already
    /// RENDERED, because only the end holding the file knows which file it is about.
    manifest_report: Option<String>,
}

impl AgentRegistry {
    /// A registry over `rules`, with no pane remembered yet.
    ///
    /// Takes the ruleset rather than calling [`sprag_detect::built_ins`] itself, which is what lets
    /// the user's `config.toml` manifests layer over the built-ins without this type learning about
    /// files: [`crate::config::AgentManifests`] does the reading, and hands the result in.
    #[must_use]
    pub fn new(rules: Ruleset) -> Self {
        Self {
            rules,
            trackers: HashMap::new(),
            // Nothing has said these are not the user's. The holder that could say so
            // ([`crate::config::AgentManifests`]) is read by the daemon's waker, which publishes
            // through [`set_manifest_report`](Self::set_manifest_report) before it parks — so this
            // default is "not asked yet" for exactly as long as a construction takes, and is the
            // final answer for a registry nobody feeds a file to at all (a test, an in-process
            // host).
            manifest_report: None,
        }
    }

    /// Swap in a ruleset the user has just edited, KEEPING every pane's memory.
    ///
    /// The trackers survive on purpose, and the two halves of the memory survive for different
    /// reasons. `seq` must not restart, or every client would read a manifest edit as a state change
    /// on every pane at once. The remembered IDENTITY survives because R252 made it a manifest NAME
    /// rather than a position in the list, in as many words *because slice 4 reloads that list from a
    /// file and a name survives a reload* — so a `codex` pane whose modal is covering its fingerprint
    /// is still a `codex` pane after the edit.
    ///
    /// What does NOT survive is the quiescence skip: the new ruleset carries a new
    /// [`Ruleset::revision`](sprag_detect::Ruleset::revision), which is one of the key's terms, so
    /// every remembered pane is [`stale`](Self::stale) until it has been looked at again.
    pub fn reload(&mut self, rules: Ruleset) {
        self.rules = rules;
    }

    /// Say why the ruleset in force is NOT the one the user's file declares — `None` when it is, or
    /// when there is no file to disagree with.
    ///
    /// Separate from [`reload`](Self::reload), and the asymmetry is the whole reason this exists as
    /// its own call. A BROKEN edit replaces no ruleset —
    /// [`AgentManifests::refresh`](crate::config::AgentManifests::refresh) answers `false` and keeps
    /// the last list that worked — and that is precisely the moment this moves from `None` to
    /// `Some`. A report folded into `reload` would be published on every case except the one it is
    /// for.
    ///
    /// The sentence is rendered by the caller because only the caller knows which file it describes,
    /// the rule [`HostClient::global_commands`](crate::HostClient::global_commands) states for a
    /// report that crosses the wire. This type still reads no file and holds no path: it is handed
    /// the sentence exactly as it is handed the [`Ruleset`].
    pub fn set_manifest_report(&mut self, report: Option<String>) {
        self.manifest_report = report;
    }

    /// Why the ruleset in force is not the user's, if it is not — what a client with a surface
    /// paints.
    #[must_use]
    pub fn manifest_report(&self) -> Option<&str> {
        self.manifest_report.as_deref()
    }

    /// Whether this pane's published verdict was reached under a ruleset that has since been
    /// replaced.
    ///
    /// The waker's third reason to ask about a pane, beside due and unknown. A pane can be settled,
    /// known, and waiting on nothing, and still owe an evaluation — because the input that moved was
    /// not on its screen. A pane nobody has ever observed is NOT stale; it is unknown, which is a
    /// different question with a different answer.
    #[must_use]
    pub fn stale(&self, id: PaneId) -> bool {
        self.trackers
            .get(&id)
            .and_then(Tracker::evaluated_under)
            .is_some_and(|revision| revision != self.rules.revision())
    }

    /// Whether this pane owes an evaluation — the settle waker's whole test for "ask about this one,
    /// skip the rest".
    ///
    /// Three reasons, and a pane can be more than one at once on the sweep that first sees it:
    ///
    /// * **DUE** — the window has closed on a candidate, so what publishes it is the CLOCK. This is the
    ///   third input a pending transition has, and the only one that arrives from no screen and no
    ///   client.
    /// * **UNKNOWN** — nobody has ever looked at this pane, so it has no state to be waiting on and is
    ///   invisible to every other question here. Only a sweep can give it one.
    /// * **STALE** — the pane is settled and known and its answer was reached under a ruleset the user
    ///   has since edited. The input that moved was not on its screen, so nothing else will ever bring
    ///   it back.
    /// * **RELEASED** — a report was in force and has been dropped, so the published verdict is one
    ///   nobody stands behind any more. Served on ANY wake rather than only a sweep, unlike the two
    ///   above: a release is one pane's own event, caused by something a person just did or by that
    ///   pane's agent going away, and the answer it invalidates is the pane's whole published state.
    ///   A ruleset edit is neither of those — it is bulk and it is not urgent.
    ///
    /// None of the three applies to a settled pane under unchanged rules, which is every pane in a
    /// quiet workspace — so the answer this returns in the steady state is `false`, and the screen read
    /// behind it never happens.
    ///
    /// `sweep` distinguishes the waker's two kinds of wake. A deadline that came due is served on any
    /// wake; discovery and staleness ride the sweep interval, because both are answers to "what has
    /// changed while nobody was asking" and neither is urgent.
    ///
    /// # Why this is a method here rather than a predicate at the call site
    ///
    /// It was the call site's, inline in the waker's loop, for three slices — and a rule composed
    /// inside a `thread::spawn` closure in a binary is reachable by nothing: no unit test could witness
    /// the composition of the three questions below (each was tested alone), and no instrument could
    /// price the one piece of the sweep that runs once per pane per interval. R260 measured this path,
    /// which is what required it to be callable. The three questions stay separate because the waker
    /// still asks `is_due` on its own in a debug assertion, and because they are what this composition
    /// is made of.
    #[must_use]
    pub fn owes_evaluation(&self, id: PaneId, now: Instant, sweep: bool) -> bool {
        self.is_due(id, now) || self.owes_look(id) || (sweep && (!self.knows(id) || self.stale(id)))
    }

    /// Take a reading of one pane and return what is published for it, or `None` for a pane no
    /// manifest claims.
    ///
    /// # When `window` is called
    ///
    /// At most once, and only when this pane will actually consult the settle window — three cases,
    /// and the third was found by a test rather than reasoned out:
    ///
    /// 1. A pane never seen before, whose tracker has to be built with a window.
    /// 2. A pane with a candidate ALREADY waiting, so this look's publish-or-not decision is made
    ///    against the window the user currently has set.
    /// 3. A pane whose candidate this look has just CREATED. It has not consulted the window yet, and
    ///    it is about to hand the waker a deadline to sleep on; leaving that deadline derived from
    ///    whatever the policy happened to be would make a `set-option` take effect one wake late — and
    ///    for a pane going quiet, that wake is the only one coming.
    ///
    /// A pane that has published and is waiting on nothing does not call it at all, which is the
    /// steady state of every settled pane in the workspace.
    ///
    /// That matters because the window is a user OPTION and this project's options are read from the
    /// file on every call — the daemon is a reader of the user's config, not a holder of it, so
    /// `set-option` needs nothing restarted. `config::window_size` prices that honestly as one file
    /// read per WINDOW CHANGE, which is a rare-event justification; this path is served on every
    /// client wake, so the same read placed unconditionally would be a file read per output batch per
    /// session. Tying it to "somebody is actually waiting on the window" keeps the quiet workspace at
    /// zero reads without caching a control the user expects to be live.
    pub fn observe(
        &mut self,
        id: PaneId,
        screen: &Screen,
        title: Option<&str>,
        now: Instant,
        window: impl FnOnce() -> Hysteresis,
    ) -> Option<AgentFacts> {
        // `window` is `FnOnce`, so the option is read at most once however many of the three cases
        // apply to this call.
        let mut source = Some(window);
        let tracker = match self.trackers.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let tracker = entry.into_mut();
                if tracker.pending_deadline().is_some()
                    && let Some(read) = source.take()
                {
                    tracker.set_policy(read());
                }
                tracker
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let read = source.take().expect("the source is untouched on this arm");
                entry.insert(Tracker::new(read()))
            }
        };
        let verdict = tracker.observe(screen, title, &self.rules, now);
        let state = verdict.state.wire_str();
        // Read HERE, off the screen this verdict was reached on and while it is still in hand — see
        // [`AgentFacts::asking`] for why no caller can be left to do it. Keyed on the VERDICT's own
        // variant rather than on the rule that fired: `blocked` is the claim a reader acts on, and a
        // manifest may reach it by more than one rule.
        let asking = (verdict.state == sprag_detect::AgentState::Blocked)
            .then(|| sprag_detect::question(screen, sprag_detect::DIALOG_WINDOW))
            .flatten();
        let agent = verdict.agent.clone();
        let rule = verdict.rule.clone();
        // Case 3: this look created the candidate, so the deadline it now implies has never been
        // measured against the user's window. Correct it before anyone sleeps on it.
        if tracker.pending_deadline().is_some()
            && let Some(read) = source.take()
        {
            tracker.set_policy(read());
        }
        Some(AgentFacts {
            state: state?,
            asking,
            agent,
            rule,
            seq: tracker.seq(),
            asked_seq: tracker.asked_seq(),
            said_seq: tracker.said_seq(),
            reports: tracker.reports(),
            source: tracker.reported_source().map(str::to_owned),
            reporter_build: tracker.reported_build().map(str::to_owned),
            // ⚠ Taken from the TRACKER rather than from this look: they are stated by the agent on
            // its own hook and survive every screen the pane has painted since. A look cannot
            // produce them and must not clear them.
            asked: tracker.reported_asked().map(str::to_owned),
            said: tracker.reported_said().map(str::to_owned),
            noticed: tracker.reported_noticed().map(str::to_owned),
            transcript: tracker.reported_transcript().map(str::to_owned),
            // ⚠⚠⚠⚠⚠ READ AFTER THE POLICY CORRECTION ABOVE, NOT BEFORE IT. Case 3 can have just
            // re-read the user's window and moved this candidate's deadline EARLIER; a value taken
            // before it would be the default's deadline, and every waiter parked on it would sleep
            // past the instant the user asked for. Register item 630.
            settles_at: tracker.pending_deadline(),
        })
    }

    /// Take a REPORT for one pane — the push half of the agent surface, and the only input here that
    /// outranks the screen.
    ///
    /// Builds the pane's memory if this is the first thing ever heard about it, which is why `window`
    /// is taken on the same terms [`observe`](Self::observe) takes it: a reported pane may later be
    /// released back to the screen, and the tracker it is released INTO must hold the window the user
    /// currently has set rather than a default frozen at the report. It is read at most once, and not
    /// at all for a pane already known — a report is not a reason to read the config file.
    ///
    /// The answer is [`ReportOutcome`] plus the published `seq`, so one round trip tells a reporter
    /// that it was heard (`accepted`), whether anybody needs waking (`changed`), and which published
    /// generation its report became.
    /// [`Report::owner`], when given, is the process group whose continued existence keeps the
    /// report standing — see [`orphaned`](Self::orphaned) for what asks about it later, and
    /// [`crate::sweep_once`] for why a report needs such a thing at all.
    pub fn report(
        &mut self,
        id: PaneId,
        report: Report,
        window: impl FnOnce() -> Hysteresis,
    ) -> (ReportOutcome, u64) {
        let tracker = self
            .trackers
            .entry(id)
            .or_insert_with(|| Tracker::new(window()));
        let outcome = tracker.report(report);
        (outcome, tracker.seq())
    }

    /// Release the pane named by `id` back to the screen, answering whether a report was in force.
    ///
    /// A pane nobody has reported, and a pane nobody has ever observed, both answer `false` — there
    /// is nothing to release, and inventing a tracker to say so would give the pane a memory its
    /// first look has not earned.
    pub fn release(&mut self, id: PaneId) -> bool {
        self.trackers
            .get_mut(&id)
            .is_some_and(Tracker::release_report)
    }

    /// Whether this pane's published verdict is a report — the test the daemon uses to decide whether
    /// a pane whose child has EXITED still has an authority to drop.
    #[must_use]
    pub fn reported(&self, id: PaneId) -> bool {
        self.trackers
            .get(&id)
            .is_some_and(|tracker| tracker.reported_source().is_some())
    }

    /// Whether this pane holds a report whose OWNER is gone — an authority with nobody behind it.
    ///
    /// The owner is a process group, so the question is asked of the OS and not of any state kept
    /// here: a group that no longer exists cannot correct, contradict or withdraw what it said, and
    /// a report that outlives its reporter is exactly the confident wrong answer this whole
    /// mechanism must not produce.
    ///
    /// `false` for a pane with no report and for a report that named no owner — the second is a
    /// person at a command line, whose report is theirs to withdraw and nobody else's to expire.
    #[must_use]
    pub fn orphaned(&self, id: PaneId) -> bool {
        self.trackers
            .get(&id)
            .and_then(Tracker::reported_owner)
            .and_then(|owner| u32::try_from(owner).ok())
            .is_some_and(|owner| !process_group_exists(owner))
    }

    /// Whether this pane owes a look no screen event will ask for — today, exactly a released pane.
    #[must_use]
    pub fn owes_look(&self, id: PaneId) -> bool {
        self.trackers.get(&id).is_some_and(Tracker::owes_look)
    }

    /// Whether ANY pane owes such a look — the waker's guard, beside [`any_due`](Self::any_due).
    ///
    /// A walk of the trackers, and per WAKE rather than per pane, which is the distinction R255
    /// measured and the reason this is not a per-pane caller's question ([`owes_look`](Self::owes_look)
    /// is that one, at one hash lookup). It is deliberately NOT counted in [`AgentWork`]: that meter
    /// says what the DEADLINE bookkeeping costs, and folding a second kind of walk into it would
    /// corrupt the number R255 and R256 are pinned against.
    #[must_use]
    pub fn any_owes_look(&self) -> bool {
        self.trackers.values().any(Tracker::owes_look)
    }

    /// When the earliest waiting candidate would publish, or `None` when nothing is waiting.
    ///
    /// This is the whole of what the settle waker needs to know to sleep: with nothing pending there
    /// is no clock to serve, which is what keeps M3's "a quiet workspace costs nothing" true of the
    /// confirmation and not only of the evaluation.
    /// **This walks every tracker**, so it belongs to callers asking about the registry as a whole —
    /// the waker, once per wake. A per-PANE caller wants [`pending_deadline`](Self::pending_deadline)
    /// instead: the pane list calls one of these per pane, and a whole-registry read there is what
    /// made the term quadratic (R255 measured it, [`AgentWork`] carries the count that keeps it gone).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        DEADLINE_VISITS.fetch_add(self.trackers.len() as u64, Ordering::Relaxed);
        self.trackers
            .values()
            .filter_map(Tracker::pending_deadline)
            .min()
    }

    /// When THIS pane's candidate would publish, or `None` when it has none — one hash lookup.
    #[must_use]
    pub fn pending_deadline(&self, id: PaneId) -> Option<Instant> {
        self.trackers.get(&id).and_then(Tracker::pending_deadline)
    }

    /// Whether this pane has a candidate whose window has closed by `now` — the waker's test for
    /// "this one needs asking, the rest do not".
    #[must_use]
    pub fn is_due(&self, id: PaneId, now: Instant) -> bool {
        self.pending_deadline(id)
            .is_some_and(|deadline| deadline <= now)
    }

    /// Whether ANY pane's window has closed by `now`.
    ///
    /// The waker's guard against a pointless walk: it is woken the moment a candidate APPEARS (so it
    /// can re-plan its sleep around a nearer deadline), and that wake is not a reason to take every
    /// workspace lock — the deadline is still ahead. Cheap, because it reads only what it already
    /// holds.
    #[must_use]
    pub fn any_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| deadline <= now)
    }

    /// Whether this pane has ever been observed.
    ///
    /// The sweep's test for "this one has no tracker yet, so nothing about it can be waiting". A
    /// candidate is only ever created by an observation, so a pane nobody has looked at has no state
    /// and no deadline — it is invisible to every other method here, which is why discovering it needs
    /// a question of its own.
    #[must_use]
    pub fn knows(&self, id: PaneId) -> bool {
        self.trackers.contains_key(&id)
    }

    /// The published `seq` for a pane, or 0 for one never observed.
    ///
    /// The waker compares this across an [`observe`](Self::observe) to learn whether the verdict
    /// MOVED, and so whether the session's clients have anything to be woken for. Reading the seq
    /// rather than re-deriving "did it change" keeps one definition of a published change.
    #[must_use]
    pub fn seq(&self, id: PaneId) -> u64 {
        self.trackers.get(&id).map_or(0, Tracker::seq)
    }

    /// Forget every pane not in `live`.
    ///
    /// **`live` must be a DAEMON-WIDE census.** The pane-list query walks one session's panes, and
    /// pruning against that would forget every other session's — so the hot path never calls this.
    /// The waker does, because it already walks every session to find due panes, so the census needs
    /// no walk of its own.
    ///
    /// That is the sense in which it is a by-product, and R260 measured the sense in which it is not:
    /// BUILDING the set is 2.9x to 3.0x this call at sixty-four panes, and the largest single term in
    /// a sweep over a big workspace. An insert per pane is not free merely because the loop around it
    /// was already going to run. Both numbers are microseconds against a five-second period, so this
    /// is a correction to the claim and not a case for changing the code.
    ///
    /// Without this a tracker outlives its pane and the map grows for the life of the daemon, one
    /// entry per pane ever opened. Bounded memory is the whole of the requirement; the latency of
    /// forgetting is irrelevant, which is why it rides the waker's cadence rather than the query's.
    pub fn retain_live(&mut self, live: &HashSet<PaneId>) {
        self.trackers.retain(|id, _| live.contains(id));
    }

    /// How many panes are remembered — for the test that pruning happens at all.
    #[must_use]
    pub fn len(&self) -> usize {
        self.trackers.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty()
    }
}

/// The [`AgentRegistry`] plus the signal that says a deadline now exists — the shared handle every
/// caller holds.
///
/// # Why the signal cannot live apart from the memory
///
/// This type exists because the first version of the waker had D9's own bug one level up, and it took
/// a live drive against the daemon to see it. The waker computed its sleep from whatever was pending
/// when it last LOOKED. At daemon start nothing is pending, so it slept for the prune interval — and
/// then a pane-list query created a candidate with a two-second deadline that the sleeping thread had
/// no way to hear about. The mechanism built to confirm verdicts nothing else would confirm was itself
/// waiting for an event nothing produced.
///
/// The fix is not a shorter sleep. A waker that woke every few hundred milliseconds to ask "is
/// anything pending yet" would take every workspace lock several times a second forever on an idle
/// daemon, which is exactly the cost M3 measured away and R218–R220 built the projection and the fetch
/// to avoid. So the appearance of a candidate is an EVENT, and the thread waits on it: with nothing
/// pending the waker is blocked and costs nothing at all, and it learns about a new deadline at the
/// instant the deadline is created.
///
/// Pairing the [`Condvar`] with the [`Mutex`] in one type is what makes that unforgettable. A caller
/// holding the two separately can lock, create a candidate, and neglect to signal — and the failure
/// is silent, because every individual verdict still looks right. Here [`observe`](Self::observe) is
/// the only way to create a candidate and it signals for you.
///
/// # Why a RELOAD does not signal, when a candidate does
///
/// A reload gives every remembered pane an evaluation to owe, so the symmetrical shape would be a
/// `reload` here that takes the lock and notifies. It would be signalling NOBODY. The only reloader
/// is the waker itself, re-reading the user's file in its own sweep — and a thread cannot notify
/// itself: it is awake, its `sweep` is true, and the walk that follows serves the stale panes in the
/// same pass. A signal there would be a mechanism with no event, which is the defect this type
/// exists to prevent rather than a second instance of it to ship.
///
/// So the reload goes through [`with`](Self::with) like the waker's other work. If one ever arrives
/// from ANOTHER thread, a wake is not enough on its own: the loop also needs a reason to do work when
/// nothing is due and no sweep is owed. That reason lives in `sprag-term`'s `ask` predicate, which is
/// where "this pane owes an evaluation" is decided, and [`AgentRegistry::stale`] is the question it
/// asks.
#[derive(Debug, Default)]
pub struct AgentClock {
    state: Mutex<AgentRegistry>,
    /// Signalled when a pane gains a pending candidate. The waker's `wait_timeout` parks on this, so
    /// "there is now a deadline" reaches it without a poll.
    appeared: Condvar,
}

impl AgentClock {
    /// A clock over `rules`, with no pane remembered yet.
    #[must_use]
    pub fn new(rules: Ruleset) -> Self {
        Self {
            state: Mutex::new(AgentRegistry::new(rules)),
            appeared: Condvar::new(),
        }
    }

    /// [`AgentRegistry::observe`], signalling the waker if this look gave the pane a deadline the
    /// waker may not have planned around.
    ///
    /// The signal is on the EDGE — the pane's deadline changed and there is now one — rather than on
    /// every look at a pending pane. A repainting pane would otherwise notify on every client wake,
    /// and each of those notifications costs the waker a trip round its loop to conclude that the
    /// deadline it already knew about has not moved.
    ///
    /// # Why THIS PANE's deadline is the right question, where the registry's minimum was the
    /// expensive one
    ///
    /// The waker must be woken whenever the registry's nearest deadline becomes EARLIER while it is
    /// parked, and the first version of this asked exactly that, by reading the minimum over every
    /// tracker before and after. It is correct and it is quadratic: the pane list calls this once per
    /// pane, so N looks each walking N entries is 2N^2 tracker visits per client wake. R255 measured
    /// the term at 2.70 to 3.35 ns per remembered pane per look — invisible at three panes, larger
    /// than the whole pane list at sixty-four.
    ///
    /// The cheap question is sound because of what `observe` can touch: it mutates exactly ONE
    /// tracker, so every other pane's deadline is the same after as before. The minimum can therefore
    /// only have moved if THIS pane's deadline moved — which makes "this pane's deadline changed and
    /// is now `Some`" a strict SUPERSET of "the minimum moved and is now `Some`". Nothing the old
    /// comparison would have signalled is lost.
    ///
    /// What the superset adds is a wake in the case where this pane's new deadline is not the nearest
    /// one, and that costs the waker a trip round its loop to park again — which its own docs already
    /// record as harmless.
    ///
    /// The trade is measured and it is not free: two hash lookups cost about 20 ns more than two
    /// walks of a ONE-entry registry, so a workspace below about eight panes pays a bounded constant
    /// for a term that was otherwise unbounded. There is a version with no lookups at all —
    /// [`AgentRegistry::observe`] holds the tracker already and could hand its deadline back — and it
    /// is deliberately not taken, because the registry's docs make a point of knowing nothing about
    /// the signal. Asking the memory a question about the MEMORY and leaving the clock to decide
    /// about WAKING is the seam; 20 ns is what it costs, and no instrument in this project can see it
    /// above the pane list that carries it.
    ///
    /// The wrong cheap version is the one that looks even simpler: a `None` ->
    /// `Some` edge. It would miss a pane whose candidate is ALREADY waiting when the user shortens
    /// `agent-settle-time`, because [`Tracker::pending_deadline`] is derived from the policy and moves
    /// EARLIER under the waiting candidate — a deadline the waker is already parked past.
    /// `a_shortened_window_wakes_a_waker_parked_on_the_old_one` is that case, and it is why the test
    /// compares deadlines rather than presence.
    pub fn observe(
        &self,
        id: PaneId,
        screen: &Screen,
        title: Option<&str>,
        now: Instant,
        window: impl FnOnce() -> Hysteresis,
    ) -> Option<AgentFacts> {
        let mut state = lock(&self.state);
        let before = state.pending_deadline(id);
        let facts = state.observe(id, screen, title, now, window);
        let after = state.pending_deadline(id);
        drop(state);
        if after != before && after.is_some() {
            self.appeared.notify_all();
        }
        facts
    }

    /// [`AgentRegistry::report`] under the lock. No signal: a report needs the waker for nothing.
    ///
    /// It publishes on the spot and it CLEARS any pending candidate, so the only thing it can do to
    /// the waker's plan is remove a deadline from it — and a waker parked on a deadline that has
    /// stopped existing wakes, finds nothing due, and parks again. The wake a report does need is the
    /// CLIENTS', and that one is the caller's to send beside the event it records, where the two land
    /// together (`ChannelRegistry::announce`).
    pub fn report(
        &self,
        id: PaneId,
        report: Report,
        window: impl FnOnce() -> Hysteresis,
    ) -> (ReportOutcome, u64) {
        lock(&self.state).report(id, report, window)
    }

    /// [`AgentRegistry::release`] under the lock, SIGNALLING the waker when a report was dropped.
    ///
    /// This is the arrival [`observe`](Self::observe)'s docs anticipated — a reason to work that
    /// comes from ANOTHER thread. A release makes the pane owe a look, and the look needs a screen,
    /// which only the waker's pass reads; without the signal the correction would wait for whatever
    /// happened to wake the thread next, up to a whole sweep interval, for a pane a person is
    /// watching. The predicate side of the same arrival is
    /// [`AgentRegistry::any_owes_look`] in the waker's guard: a wake with nothing to do would
    /// otherwise be sent straight back to the park.
    pub fn release(&self, id: PaneId) -> bool {
        let released = lock(&self.state).release(id);
        if released {
            self.appeared.notify_all();
        }
        released
    }

    /// Read the memory under its lock — the waker's walk, and nothing else.
    pub fn with<R>(&self, f: impl FnOnce(&mut AgentRegistry) -> R) -> R {
        f(&mut lock(&self.state))
    }

    /// Park until there is a deadline to serve, or until `cap` elapses, whichever comes first —
    /// returning EARLY if a candidate appears in the meantime.
    ///
    /// `cap` bounds the wait so the caller still gets a turn for work that is not deadline-driven
    /// (forgetting the panes that are gone). With a deadline already past this returns at once, so a
    /// caller that has fallen behind does not add a wait to the lateness.
    pub fn park_until_due(&self, cap: Duration) {
        let state = lock(&self.state);
        let wait = state
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .filter(|remaining| *remaining < cap)
            .unwrap_or(cap);
        if wait.is_zero() {
            return;
        }
        // A spurious wake is harmless: the caller re-reads `any_due` and parks again.
        let _unused = self
            .appeared
            .wait_timeout(state, wait)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Whether a process group still exists, asked of the OS.
///
/// `kill(-pgid, 0)` delivers nothing and reports only whether the group is there — the standard way
/// to ask, and one syscall, which is what lets [`AgentRegistry::orphaned`] sit beside the sweep's
/// `is_eof` load without changing that loop's cost shape.
///
/// **`EPERM` means the group EXISTS** and this process may not signal it, so it answers `true`. Only
/// `ESRCH` is an absence. Reading the return value alone would retire a live agent's report the
/// moment it ran as another user.
///
/// A pgid below 2 is refused rather than asked about, because `kill`'s negation reads those as
/// commands rather than questions: `-0` is "my own process group" and `-1` is "every process I may
/// signal". Neither is a group any pane's terminal is owned by, and the guard is here so that a
/// nonsense value coming the other way cannot be turned into one.
fn process_group_exists(pgid: u32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else {
        return false;
    };
    if pgid < 2 {
        return false;
    }
    // SAFETY: `kill` with signal 0 performs the permission and existence checks and delivers
    // nothing. Both arguments are plain integers and the guard above keeps the negation off the
    // process-group wildcards.
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_detect::DEFAULT_SETTLE;
    use sprag_vt::{Emulator, VtPort};

    /// A `claude` pane the footer fingerprint claims, with no title at all — the smallest screen
    /// these tests need, since the RULES' fidelity to a real agent is slice 1's business.
    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    /// The smallest screen the dialog rule fires on.
    const DIALOG: &[&str] = &["❯ 1. Yes", "  2. No"];

    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    fn repaint(em: &mut Emulator, lines: &[&str]) {
        em.advance(b"\x1b[2J\x1b[H");
        em.advance(lines.join("\r\n").as_bytes());
    }

    /// A reported verdict reaches the pane list naming its SOURCE and no rule; a scraped one is the
    /// other way round. That is how a reader tells an authority from an inference.
    #[test]
    fn a_reported_verdict_names_who_said_it_and_a_scraped_one_names_the_rule() {
        let mut reg = AgentRegistry::new(Ruleset::new(vec![sprag_detect::claude()]));
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        let scraped = reg
            .observe(
                PaneId(1),
                em.screen(),
                Some("⠂ x"),
                base,
                Hysteresis::default,
            )
            .expect("the footer claims this pane");
        assert_eq!(scraped.state, "working");
        assert!(scraped.rule.is_some(), "a scrape says which rule fired");
        assert_eq!(scraped.source, None, "and names no reporter");

        let (outcome, published) = reg.report(
            PaneId(1),
            Report {
                state: sprag_detect::AgentState::Idle,
                agent: Some("claude".to_owned()),
                source: "herdr:claude".to_owned(),
                seq: Some(7),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            Hysteresis::default,
        );
        assert!(outcome.accepted && outcome.changed);
        let reported = reg
            .observe(
                PaneId(1),
                em.screen(),
                Some("⠂ x"),
                base,
                Hysteresis::default,
            )
            .expect("still claimed");
        assert_eq!(reported.state, "idle", "the report outranks the screen");
        assert_eq!(reported.source.as_deref(), Some("herdr:claude"));
        assert_eq!(reported.rule, None, "a report fired no rule");
        assert_eq!(
            reported.seq, published,
            "and the answer carries that generation"
        );
    }

    /// ⚠⚠⚠⚠⚠ **A DAEMON CAN SAY WHICH BUILD ITS REPORTER IS, AND A REPORTER THAT SAYS NOTHING IS
    /// NOT ONE THAT MATCHES** — register item 412's product half, and the three cases
    /// [`AgentFacts::reporter_build`] exists to keep apart.
    ///
    /// The hook is a SEPARATE PROCESS that any `cargo build` replaces under a running daemon, so
    /// *"is this reporter my image?"* is live after every rebuild — and until this key nothing
    /// anywhere could answer it. Item 344 is the loud version of the same skew (a bumped hook
    /// refused at the protocol check, leaving the last `working` true for ever); this is the quiet
    /// one, where the reports are accepted and the code producing them is not the daemon's.
    ///
    /// # ⚠⚠⚠⚠ The third arm is the one that would have re-introduced the defect
    ///
    /// A reporter that states no build must answer `None` **even when a previous reporter stated
    /// one**. Its two neighbours in `Reported` (`asked`, `transcript`) are carried forward on
    /// purpose — they are EVENTS, stated only on the report that opens a turn — and copying that
    /// treatment here would have been the natural mistake: a hook replaced by a foreign one would go
    /// on answering under the identity of the reporter it displaced, which is precisely the
    /// substitution this key exists to expose. So this is a LEVEL about whoever is reporting now,
    /// and it is REPLACED.
    #[test]
    fn a_reporter_says_which_build_it_is_and_silence_is_never_agreement() {
        let mut reg = AgentRegistry::default();
        let report = |reg: &mut AgentRegistry, source: &str, build: Option<&str>| {
            reg.report(
                PaneId(3),
                Report {
                    state: sprag_detect::AgentState::Working,
                    agent: None,
                    source: source.to_owned(),
                    seq: None,
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: build.map(str::to_owned),
                },
                Hysteresis::default,
            );
            reg.observe(
                PaneId(3),
                painted(CLAUDE_FOOTER).screen(),
                Some("⠂ x"),
                Instant::now(),
                Hysteresis::default,
            )
            .expect("a reported pane is claimed")
            .reporter_build
        };

        // ── 1. A reporter that STATES a build: the daemon can name it. ──
        assert_eq!(
            report(&mut reg, "hook:claude", Some("deadbeef1234")).as_deref(),
            Some("deadbeef1234"),
            "⚠⚠⚠ the daemon must be able to say which build reported, or `this reporter is not my \
             image` is a sentence nothing can produce",
        );

        // ── 2. A DIFFERENT reporter that states NOTHING: `None`, not the one before it. ──
        assert_eq!(
            report(&mut reg, "cli", None),
            None,
            "⚠⚠⚠⚠⚠ SILENCE IS NOT INHERITANCE. `asked` and `transcript` are carried across reports \
             because they are events about a turn; this is a level about the CURRENT reporter, and \
             carrying it would let a hook that replaced another answer under the displaced one's \
             identity — the exact substitution this key exists to expose. Deleting the plain \
             `build` assignment in `Tracker::report` (making it `or_else` its neighbours' way) \
             reddens here and nowhere else",
        );

        // ── 3. And it comes back when a reporter states one again, so this is a level and not a
        //       one-way latch that a single silent report would have wedged shut. ──
        assert_eq!(
            report(&mut reg, "hook:claude", Some("cafe5678")).as_deref(),
            Some("cafe5678"),
            "a level tracks the current reporter in both directions",
        );
    }

    /// A report about a pane nobody has ever looked at BUILDS the memory, reading the settle window
    /// once to do it — because that window is what the pane will be released back into.
    #[test]
    fn a_report_builds_the_memory_and_reads_the_window_once() {
        let mut reg = AgentRegistry::default();
        let mut reads = 0_u32;
        let report = |reg: &mut AgentRegistry, reads: &mut u32| {
            reg.report(
                PaneId(3),
                Report {
                    state: sprag_detect::AgentState::Working,
                    agent: None,
                    source: "hook".to_owned(),
                    seq: None,
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                },
                || {
                    *reads += 1;
                    Hysteresis::default()
                },
            )
        };

        assert!(!reg.knows(PaneId(3)), "nothing has looked at this pane");
        report(&mut reg, &mut reads);
        assert!(reg.knows(PaneId(3)), "the report gave it a memory");
        assert_eq!(reads, 1, "which needed a window");
        report(&mut reg, &mut reads);
        assert_eq!(
            reads, 1,
            "a report is not a reason to re-read the user's config"
        );
    }

    /// A RELEASED pane owes an evaluation on ANY wake, where a stale one waits for the sweep.
    ///
    /// The contrast is the point: staleness is bulk and unhurried (a config edit touched every pane),
    /// while a release is one pane's own event and invalidates its whole published state. Without the
    /// `sweep` argument being ignored for this reason, a release would take up to a sweep interval to
    /// show — for a pane somebody is watching.
    #[test]
    fn a_released_pane_owes_a_look_on_any_wake() {
        let mut reg = AgentRegistry::new(Ruleset::new(vec![sprag_detect::claude()]));
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();
        reg.observe(
            PaneId(1),
            em.screen(),
            Some("⠂ x"),
            base,
            Hysteresis::default,
        );
        reg.report(
            PaneId(1),
            Report {
                state: sprag_detect::AgentState::Idle,
                agent: None,
                source: "hook".to_owned(),
                seq: None,
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            Hysteresis::default,
        );
        let settled = base + DEFAULT_SETTLE * 2;
        assert!(
            !reg.owes_evaluation(PaneId(1), settled, true),
            "a reported pane owes nothing — not even on a sweep",
        );
        assert!(!reg.any_owes_look());

        assert!(reg.release(PaneId(1)), "a report was in force");
        assert!(
            reg.owes_evaluation(PaneId(1), settled, false),
            "and a released pane is served on a wake that is not a sweep",
        );
        assert!(reg.any_owes_look(), "which is what the waker's guard reads");

        reg.observe(
            PaneId(1),
            em.screen(),
            Some("⠂ x"),
            settled,
            Hysteresis::default,
        );
        assert!(!reg.any_owes_look(), "the look served it");
        assert!(!reg.reported(PaneId(1)));
    }

    /// Releasing a pane nobody reported — or one nobody has ever seen — answers `false` and creates
    /// nothing.
    #[test]
    fn releasing_what_nobody_reported_is_a_clean_no() {
        let mut reg = AgentRegistry::new(Ruleset::new(vec![sprag_detect::claude()]));
        let em = painted(CLAUDE_FOOTER);
        assert!(!reg.release(PaneId(9)), "no such pane");
        assert!(
            !reg.knows(PaneId(9)),
            "and asking did not give it a memory it has not earned",
        );
        reg.observe(
            PaneId(1),
            em.screen(),
            Some("⠂ x"),
            Instant::now(),
            Hysteresis::default,
        );
        assert!(!reg.release(PaneId(1)), "observed, but never reported");
    }

    /// The window is read for a pane never seen and for one with a candidate waiting, and NOT for a
    /// pane that has published and is waiting on nothing — the property that keeps a file read off the
    /// hot path. COUNTED, because "we only read it when needed" is exactly the kind of claim that
    /// quietly stops being true.
    ///
    /// Note what the sequence below had to be corrected to say: a FIRST sighting of a resting pane
    /// leaves a candidate pending, because the settle window applies to a first publication too. A
    /// pane is not settled because it has been looked at once.
    #[test]
    fn the_settle_window_is_read_only_when_a_pane_will_consult_it() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();
        let mut reads = 0_u32;

        let observe = |reg: &mut AgentRegistry, em: &Emulator, now, reads: &mut u32| {
            reg.observe(PaneId(1), em.screen(), Some("✳ Claude Code"), now, || {
                *reads += 1;
                Hysteresis::default()
            })
        };

        // First sighting: the tracker has to be built, so the window is read.
        observe(&mut reg, &em, base, &mut reads);
        assert_eq!(reads, 1, "a pane never seen needs a window to build with");

        // That first look left `Idle` waiting, so the window is live and read again — and this look
        // publishes it.
        let settled = base + DEFAULT_SETTLE;
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "a waiting candidate consults the window");
        assert_eq!(reg.next_deadline(), None, "and this look published it");

        // NOW the pane is settled: published, nothing pending, nothing moved. No read.
        observe(&mut reg, &em, settled, &mut reads);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "a settled, known pane does not touch the file");

        // A dialog publishes on sight, so it leaves nothing waiting and needs no window either.
        repaint(&mut em, DIALOG);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "an active verdict is published without a window");

        // The dialog goes: a return to rest is an absence, so a candidate waits and the window is
        // live again.
        repaint(&mut em, CLAUDE_FOOTER);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 3, "a fresh candidate re-reads the user's window");
    }

    /// D8's additive rule, carried by the return type: a pane no manifest claims produces nothing at
    /// all, so the wire shape for a workspace without agents cannot drift.
    #[test]
    fn a_pane_no_manifest_claims_produces_no_facts() {
        let mut reg = AgentRegistry::default();
        let em = painted(&["$ ls", "file.txt"]);
        let facts = reg.observe(
            PaneId(7),
            em.screen(),
            None,
            Instant::now(),
            Hysteresis::default,
        );
        assert_eq!(facts, None);
    }

    /// Both callers — the query and the waker — go through one tracker, so a second look at an
    /// unchanged pane must change nothing. Two clients polling one wake is exactly this.
    #[test]
    fn two_observations_of_one_unchanged_pane_publish_once() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();
        // The footer has to be SEEN before the dialog covers it, or the pane is claimed by nothing
        // and the dialog is a choice list in an anonymous pane — R251's finding, and the reason the
        // identity memory exists.
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        repaint(&mut em, DIALOG);

        let first = reg
            .observe(PaneId(1), em.screen(), None, base, Hysteresis::default)
            .expect("the dialog rule fires");
        let second = reg
            .observe(PaneId(1), em.screen(), None, base, Hysteresis::default)
            .expect("and again");

        assert_eq!(first.state, "blocked");
        assert_eq!(first, second, "the second look is not a second publication");
        assert_eq!(reg.seq(PaneId(1)), 1);
    }

    /// The waker's two questions, against the same pane: when to come back, and who is due now.
    #[test]
    fn a_waiting_candidate_is_due_at_its_deadline_and_not_before() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        repaint(&mut em, DIALOG);
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        assert_eq!(
            reg.next_deadline(),
            None,
            "a verdict published on sight leaves the waker nothing to do",
        );

        repaint(&mut em, CLAUDE_FOOTER);
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        assert_eq!(reg.next_deadline(), Some(base + DEFAULT_SETTLE));
        assert!(!reg.is_due(PaneId(1), base + DEFAULT_SETTLE / 2));
        assert!(reg.is_due(PaneId(1), base + DEFAULT_SETTLE));

        // And the seq moves only when the waker's own observation publishes.
        let before = reg.seq(PaneId(1));
        reg.observe(
            PaneId(1),
            em.screen(),
            None,
            base + DEFAULT_SETTLE,
            Hysteresis::default,
        );
        assert_eq!(
            reg.seq(PaneId(1)),
            before + 1,
            "the waker published the rest"
        );
        assert_eq!(reg.next_deadline(), None);
    }

    /// A tracker must not outlive its pane, or the map grows for the life of the daemon.
    #[test]
    fn a_pane_absent_from_the_census_is_forgotten() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let now = Instant::now();
        for id in [1, 2, 3] {
            reg.observe(PaneId(id), em.screen(), None, now, Hysteresis::default);
        }
        assert_eq!(reg.len(), 3);

        reg.retain_live(&HashSet::from([PaneId(1), PaneId(3)]));
        assert_eq!(reg.len(), 2, "the pane the census did not mention is gone");
        assert_eq!(
            reg.seq(PaneId(2)),
            0,
            "and it is forgotten, not merely hidden"
        );
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use std::sync::Arc;

    use sprag_detect::DEFAULT_SETTLE;
    use sprag_vt::{Emulator, VtPort};

    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    /// The defect this type exists for, as a test: a parked waker must learn about a candidate that
    /// appears AFTER it parked.
    ///
    /// The first waker computed its sleep once and slept for the prune interval, because at daemon
    /// start nothing is pending — and the query that created a two-second candidate had no way to
    /// reach it. That shipped-looking, fully-reviewed thread published nothing at all, and only a live
    /// drive against the binary showed it.
    ///
    /// So the assertion is about TIME: park with a cap far longer than the test's patience, create a
    /// candidate from another thread, and require the park to return anyway. Remove the `notify_all`
    /// in `observe` and this blocks for the whole cap.
    #[test]
    fn a_parked_waker_returns_when_a_candidate_appears() {
        let clock = Arc::new(AgentClock::default());
        // Far longer than any deadline in this test, so a park that returns can only have been woken.
        let cap = Duration::from_secs(60);

        let parked = Arc::clone(&clock);
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = std::thread::spawn(move || {
            parked.park_until_due(cap);
            let _ = tx.send(());
        });

        // Give the thread time to actually be inside the wait before the candidate appears. If it has
        // not parked yet the test still passes for the right reason — the park would compute the
        // deadline it can now see — so this sleep cannot make a broken implementation look correct.
        std::thread::sleep(Duration::from_millis(50));

        let em = painted(CLAUDE_FOOTER);
        let facts = clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            Instant::now(),
            Hysteresis::default,
        );
        assert_eq!(facts, None, "a resting verdict does not publish on sight");
        assert!(
            clock.with(|state| state.next_deadline()).is_some(),
            "and it leaves a deadline for the waker to serve",
        );

        rx.recv_timeout(Duration::from_secs(5))
            .expect("the parked waker was never told a candidate appeared");
        waker.join().expect("the waker thread");
    }

    /// A park with a deadline already past must not add a wait to the lateness.
    #[test]
    fn a_park_returns_at_once_when_a_deadline_has_already_passed() {
        let clock = AgentClock::default();
        let em = painted(CLAUDE_FOOTER);
        // Dated in the PAST, so the candidate's window has already closed by the time anyone parks.
        let long_ago = Instant::now() - DEFAULT_SETTLE * 2;
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            long_ago,
            Hysteresis::default,
        );
        assert!(clock.with(|state| state.any_due(Instant::now())));

        let start = Instant::now();
        clock.park_until_due(Duration::from_secs(60));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "a due deadline is served now, not after the cap: waited {:?}",
            start.elapsed(),
        );
    }

    /// THE CASE THAT DECIDES WHAT THE EDGE IS COMPARED ON, and the reason `observe` reads a deadline
    /// rather than a presence.
    ///
    /// A candidate is already waiting when the user shortens `agent-settle-time`. Nothing appears —
    /// the pane was pending before and is pending after — but the deadline moves EARLIER, because
    /// `Tracker::pending_deadline` is derived from `since + settle` rather than stored, exactly so
    /// that a shortened window moves the wait it is already serving. A waker parked on the old
    /// deadline would otherwise sleep straight through the new one.
    ///
    /// This is what a `None` -> `Some` edge would miss, and it is the whole difference between the
    /// cheap question being sound and being a regression.
    /// A RELEASE wakes a parked waker, because the pane it released needs a screen and only the
    /// waker's pass reads one.
    ///
    /// The park here can only return because it was TOLD: a report leaves nothing pending, so
    /// `park_until_due` sleeps for its whole cap, and the cap is longer than this test's patience. Same
    /// instrument as `a_shortened_window_wakes_a_waker_parked_on_the_old_one`, pointed at the other
    /// arrival — the one `AgentClock::observe`'s docs anticipated, from another thread.
    #[test]
    fn a_release_wakes_a_parked_waker() {
        let clock = Arc::new(AgentClock::default());
        clock.report(
            PaneId(1),
            Report {
                state: sprag_detect::AgentState::Working,
                agent: None,
                source: "hook".to_owned(),
                seq: None,
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            Hysteresis::default,
        );
        assert_eq!(
            clock.with(|state| state.next_deadline()),
            None,
            "a report leaves nothing pending, so the park below has no deadline to return on",
        );

        let parked = Arc::clone(&clock);
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = std::thread::spawn(move || {
            parked.park_until_due(Duration::from_secs(600));
            let _ = tx.send(());
        });
        std::thread::sleep(Duration::from_millis(50));

        assert!(clock.release(PaneId(1)), "a report was in force");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("a parked waker was never told a pane had been released");
        waker.join().expect("the waker thread");
        assert!(
            clock.with(|state| state.any_owes_look()),
            "and it has a reason to act on the wake, which the guard reads",
        );
    }

    #[test]
    fn a_shortened_window_wakes_a_waker_parked_on_the_old_one() {
        let clock = Arc::new(AgentClock::default());
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let now = Instant::now();
        // A window far longer than this test's patience, so the park below can only return because
        // it was TOLD the deadline moved.
        let long = Hysteresis {
            settle: Duration::from_secs(600),
        };
        clock.observe(PaneId(1), em.screen(), title, now, || long);
        assert!(
            clock
                .with(|state| state.pending_deadline(PaneId(1)))
                .is_some(),
            "the first look leaves a candidate waiting on the long window",
        );

        let parked = Arc::clone(&clock);
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = std::thread::spawn(move || {
            parked.park_until_due(Duration::from_secs(600));
            let _ = tx.send(());
        });
        std::thread::sleep(Duration::from_millis(50));

        // The user shortens the window. The pane is pending before and pending after, so nothing
        // APPEARED — and the deadline it is waiting on just moved into the past.
        let short = Hysteresis {
            settle: Duration::from_millis(1),
        };
        clock.observe(PaneId(1), em.screen(), title, now, || short);
        assert!(
            clock.with(|state| state.is_due(PaneId(1), Instant::now())),
            "the shortened window puts the candidate's deadline behind us",
        );

        rx.recv_timeout(Duration::from_secs(5))
            .expect("a waker parked on the old deadline was never told the new one is nearer");
        waker.join().expect("the waker thread");
    }

    /// The wake is on the EDGE. A pane already pending, looked at again, must not notify — otherwise a
    /// repainting pane sends the waker round its loop once per client wake for a deadline it already
    /// knows about.
    #[test]
    fn re_observing_an_already_pending_pane_does_not_wake_the_waker() {
        let clock = AgentClock::default();
        let em = painted(CLAUDE_FOOTER);
        let now = Instant::now();
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            now,
            Hysteresis::default,
        );
        let deadline = clock.with(|state| state.next_deadline());
        assert!(
            deadline.is_some(),
            "the first look leaves a candidate waiting"
        );

        // Nothing has moved, so this look reaches the same candidate with the same `since`. A park
        // must therefore still be waiting on the same instant rather than having been woken.
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            now,
            Hysteresis::default,
        );
        assert_eq!(
            clock.with(|state| state.next_deadline()),
            deadline,
            "the deadline did not move, so there was nothing to announce",
        );
    }

    /// `claude`, with its idle rule rewritten to conclude something else — a user's correction, in
    /// the smallest form that changes an answer.
    fn rewritten_claude() -> Ruleset {
        let mut claude = sprag_detect::claude();
        claude
            .rules
            .iter_mut()
            .find(|rule| rule.id == "idle-glyph")
            .expect("the built-in has an idle rule")
            .state = sprag_detect::AgentState::Working;
        Ruleset::new(vec![claude])
    }

    /// THE RELOAD CASE, and the one the waker's third `ask` reason exists for.
    ///
    /// A settled pane is not due and is not unknown, so under slice 3's two reasons nobody would
    /// ever look at it again. The input that moved is not on its screen — which is precisely the
    /// pane a user edits a manifest to fix, because a quiet pane is where a wrong verdict is visible
    /// and stuck.
    #[test]
    fn a_reload_makes_a_settled_pane_owe_an_evaluation() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let id = PaneId(1);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        reg.observe(id, em.screen(), title, base, Hysteresis::default);
        let settled = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE,
                Hysteresis::default,
            )
            .expect("a claude pane publishes");
        assert_eq!(settled.state, "idle");
        assert!(
            !reg.stale(id),
            "this pane was evaluated under the rules that are in force",
        );

        reg.reload(rewritten_claude());
        assert!(
            reg.stale(id),
            "a replaced ruleset is an input the pane's own screen cannot report",
        );

        let after = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE * 2,
                Hysteresis::default,
            )
            .expect("still a claude pane");
        assert_eq!(
            after.state, "working",
            "the correction reaches a pane that has not painted a single row since",
        );
        assert!(!reg.stale(id), "and the debt is settled by looking");
    }

    /// The memory SURVIVES a reload, and the two halves survive for different reasons.
    ///
    /// `seq` must not restart, or a manifest edit would read on the wire as a state change on every
    /// pane at once. The identity must not be dropped, because R252 made it a manifest NAME rather
    /// than a position in the list *because slice 4 reloads that list from a file* — this is the
    /// round that gets to assert the reason was real.
    #[test]
    fn a_reload_keeps_the_seq_a_client_diffs_and_the_identity_a_modal_covers() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let id = PaneId(1);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        reg.observe(id, em.screen(), title, base, Hysteresis::default);
        let settled = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE,
                Hysteresis::default,
            )
            .expect("published");

        reg.reload(rewritten_claude());
        let after = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE * 2,
                Hysteresis::default,
            )
            .expect("published");

        assert_eq!(
            after.seq,
            settled.seq + 1,
            "the seq counts on from what the client last read, rather than starting over",
        );
        assert_eq!(after.agent.as_deref(), Some("claude"));
        assert_eq!(
            reg.len(),
            1,
            "one pane, one tracker — the reload built no new map"
        );
    }

    /// A pane nobody has ever observed is UNKNOWN, not stale, and the waker asks about the two for
    /// different reasons. Collapsing them would make `stale` answer for panes it knows nothing
    /// about, which is the sweep's other job wearing this one's name.
    #[test]
    fn a_pane_nobody_has_observed_is_unknown_rather_than_stale() {
        let mut reg = AgentRegistry::default();
        let never = PaneId(7);
        assert!(!reg.knows(never));
        assert!(!reg.stale(never), "there is no earlier answer to be stale");

        reg.reload(rewritten_claude());
        assert!(
            !reg.stale(never),
            "and a reload does not invent one for a pane that was never looked at",
        );
    }

    /// The waker's whole test for "ask about this one", composed. Each of the three questions it
    /// asks has had its own test since the slice that added it; the COMPOSITION had none at any
    /// level, because it was a closure inside a `thread::spawn` in a binary until R260 needed it
    /// callable to price it.
    ///
    /// The truth table is the claim. A DUE pane is served on any wake, because a deadline that has
    /// passed is the only thing here that is already late. Discovery and staleness are gated on the
    /// SWEEP: both answer "what changed while nobody was asking", and one pane's deadline coming due
    /// is not a reason to go looking at every other pane in the daemon.
    #[test]
    fn the_sweep_gate_decides_which_of_the_three_reasons_are_urgent() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let base = Instant::now();
        let due_at = base + DEFAULT_SETTLE;

        // DUE: one look leaves a candidate waiting, and its window closes at `due_at`.
        let pending = PaneId(1);
        reg.observe(pending, em.screen(), title, base, Hysteresis::default);
        assert!(
            reg.owes_evaluation(pending, due_at, false),
            "a deadline that has passed is served on ANY wake — the clock is the only input left",
        );
        assert!(
            !reg.owes_evaluation(pending, base, false),
            "and not before it passes, or the settle window would mean nothing",
        );

        // UNKNOWN: no tracker at all, so nothing else here can see this pane.
        let never = PaneId(7);
        assert!(
            !reg.owes_evaluation(never, due_at, false),
            "discovery is not urgent: a pane nobody has asked about waits for the sweep",
        );
        assert!(
            reg.owes_evaluation(never, due_at, true),
            "and the sweep is the only thing that will ever give it a state",
        );

        // STALE: settled under rules the user has since replaced.
        let settled = PaneId(2);
        reg.observe(settled, em.screen(), title, base, Hysteresis::default);
        reg.observe(settled, em.screen(), title, due_at, Hysteresis::default)
            .expect("a claude pane publishes");
        reg.reload(rewritten_claude());
        assert!(
            !reg.owes_evaluation(settled, due_at, false),
            "a reload is not urgent either — nothing about this pane is late",
        );
        assert!(
            reg.owes_evaluation(settled, due_at, true),
            "but only the sweep can bring the correction to a pane that never paints again",
        );
    }

    /// The property the sweep's whole cost argument rests on: a settled pane under unchanged rules
    /// owes NOTHING, on either kind of wake. This is the answer every pane in a quiet workspace
    /// gives every five seconds for the life of the daemon, so it is the one R260 priced — and the
    /// screen read behind it is what never happens.
    ///
    /// Asserted at an instant far past the settle window rather than at the one that published,
    /// because the steady state a sweep meets is an old one: a candidate that lingered after
    /// publishing would make every later sweep read a screen, and the failure would be invisible at
    /// `due_at`.
    #[test]
    fn a_settled_pane_under_unchanged_rules_owes_nothing_on_either_wake() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let id = PaneId(1);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        reg.observe(id, em.screen(), title, base, Hysteresis::default);
        reg.observe(
            id,
            em.screen(),
            title,
            base + DEFAULT_SETTLE,
            Hysteresis::default,
        )
        .expect("a claude pane publishes");

        let much_later = base + DEFAULT_SETTLE * 100;
        assert!(
            !reg.owes_evaluation(id, much_later, false),
            "nothing is waiting on the clock",
        );
        assert!(
            !reg.owes_evaluation(id, much_later, true),
            "and the sweep finds it known and evaluated under the rules in force",
        );
    }

    /// The question `orphaned` puts to the OS, at its edges.
    ///
    /// The live half needs both answers or it proves nothing: a group that exists and one that does
    /// not, told apart by the same call. The refused half is the pair of values `kill`'s negation
    /// reads as commands — `-0` is "my own process group" and `-1` is "everything I may signal" — so
    /// a nonsense owner arriving from anywhere must be declined rather than asked about, and the
    /// test that says so is the only thing standing between a bad number and a signal.
    ///
    /// `EPERM` — a group that exists but belongs to another user, which is what an agent started
    /// under `sudo` leaves in a pane — answers `true` in the code above and is NOT exercised here:
    /// an unprivileged test cannot make a process it may not signal. Named rather than left to be
    /// assumed, because getting it wrong would retire such an agent's report on its first sweep.
    #[test]
    fn a_process_group_exists_until_it_does_not_and_the_wildcards_are_refused() {
        let mine = u32::try_from(unsafe { libc::getpgrp() }).expect("our own process group");
        assert!(
            process_group_exists(mine),
            "our own group is there to be found",
        );

        let mut gone = std::process::Command::new("/bin/sleep");
        gone.arg("300");
        // SAFETY: `setpgid` is async-signal-safe and runs in the forked child before `exec`.
        unsafe {
            use std::os::unix::process::CommandExt as _;
            gone.pre_exec(|| match libc::setpgid(0, 0) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            });
        }
        let mut gone = gone.spawn().expect("a process in its own group");
        let pgid = gone.id();
        assert!(
            process_group_exists(pgid),
            "CONTROL: it is there while it runs"
        );
        gone.kill().expect("kill it");
        gone.wait()
            .expect("and reap it, so the group is really gone");
        assert!(!process_group_exists(pgid), "and gone once it is");

        for wildcard in [0, 1] {
            assert!(
                !process_group_exists(wildcard),
                "{wildcard} is a command to kill, not a group to ask about",
            );
        }
    }
}
